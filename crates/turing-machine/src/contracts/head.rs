//! The run-time half of a head-position clause (docs/tmt/language.md
//! (head-position clauses)): one synthesized check state per declared
//! clause, planted over the LOWERED IR of a debug build.
//!
//! **What a check state is.** For a tape parameter declaring `enters { … }`
//! or `leaves { … }`, a state whose rows are the declared symbols on that
//! one tape (wildcards everywhere else) handing control onward, plus an
//! all-wildcard catch-all whose transition is the contract terminal
//! ([`IrTransition::TrapContract`], codegen's `trap #2`). `|set| + 1` rows,
//! and several contracted tapes CHAIN — each state's passing rows go to the
//! next one — rather than forming a cross-tape product.
//!
//! **Where they go.** The `enters` chain becomes the world's entry and ends
//! by going to the state that used to be it, so the check reads the head
//! before a single body row runs. The `leaves` chain is planted before
//! every `return` — the rule's own, and a call's `then return`, which is
//! the same moment — and ends by returning itself. Planting it at the
//! return rather than at the body rows before it is what makes the check
//! read the REAL cell: any move the body made has already happened.
//!
//! Three shapes deliberately get no `leaves` check, each for a reason the
//! language already states. `stop`/`halt` end the whole run rather than
//! returning, so no head position was ever promised at them. `goto <state
//! parameter>` leaves through one of the routine's own exits, and a
//! routine's exit parameters are not `leaves` rows — only `return` is
//! (docs/tmt/language.md (head-position clauses)), which is the identical
//! rule the static half reads. And a graph, below.
//!
//! **Routines only.** A graph's clauses are checked statically and travel
//! into its header, but a graph has no IR world of its own: `ir::lower`
//! runs after `expand` has spliced every graph into its hosts. Planting a
//! check for one would mean rewriting bodies before the splice, which moves
//! the spliced bytes and the graph digest a grafting consumer re-checks.
//! So this rewrite is scoped to routine worlds, and a graph's clause
//! remains a static-plus-declaration promise.
//!
//! **Opt-in per clause.** A tape without a clause gets no state; a routine
//! whose tapes declare none is not touched at all, so a module that
//! declares nothing compiles to the same bytes it always did.

use std::collections::HashSet;

use crate::ir::{
    IrCell, IrDispatch, IrProgram, IrRule, IrState, IrTape, IrThen, IrTransition, IrWorld,
    IrWorldKind, fresh_state_name,
};

/// Plant every declared head-position check in `program`. The caller gates
/// this on `--strip-asserts` being off: stripping is a COMPILER decision,
/// so a stripped build never builds these states at all — nothing later in
/// the pipeline filters them out, and the IR of a stripped build carries no
/// trace of them.
pub(crate) fn synthesize(program: &mut IrProgram) {
    for world in &mut program.worlds {
        synthesize_world(world);
    }
}

fn synthesize_world(w: &mut IrWorld) {
    if w.kind != IrWorldKind::Routine {
        return;
    }
    let enters = declared(w, |t| t.enters.as_ref());
    let leaves = declared(w, |t| t.leaves.as_ref());
    if enters.is_empty() && leaves.is_empty() {
        return;
    }

    let arity = w.arity as usize;
    // The routine's own declaration line: where the parameter that made the
    // promise is written, and so the position a debug build maps the trap
    // to. Both the state and its rows carry it, since codegen keys the
    // debug line map off the row's own line.
    let line = w.line;
    let mut used: HashSet<String> = w.states.iter().map(|s| s.name.clone()).collect();
    // Every state the world had before anything was planted. The `leaves`
    // rewrite addresses exactly these — a chain's own rows must keep the
    // way out they were built to take.
    let body = w.states.len();

    // A routine that declares `leaves` and never returns — everything
    // reaches a `stop`, a `halt` or one of its own exits — gets no chain
    // at all rather than an unreachable one.
    if !leaves.is_empty()
        && w.states[..body]
            .iter()
            .flat_map(|s| &s.rules)
            .any(returns_from_routine)
    {
        let id = chain(
            &mut w.states,
            &mut used,
            arity,
            line,
            &leaves,
            "leaves",
            IrTransition::Return,
        );
        for rule in w.states[..body].iter_mut().flat_map(|s| &mut s.rules) {
            if returns_from_routine(rule) {
                redirect(rule, id);
            }
        }
    }

    if !enters.is_empty() {
        let id = chain(
            &mut w.states,
            &mut used,
            arity,
            line,
            &enters,
            "enters",
            IrTransition::Goto { state: w.entry },
        );
        w.entry = id;
    }
}

/// One tape's declared clause, resolved from glyphs back to the symbol
/// indices a match row is written in.
struct Clause {
    tape: usize,
    name: String,
    indices: Vec<u32>,
}

/// Every tape of `w` whose `pick` clause is declared, in signature order.
///
/// A clause is carried on [`IrTape`] as GLYPHS (the form the interface
/// publishes), so each is read back to its index through the tape's own
/// glyph table — the same table the clause was written from one stage
/// earlier. A glyph that is not in the table cannot be matched on, and a
/// clause that loses any member would silently check LESS than it
/// declared, so such a clause is dropped whole rather than narrowed. It is
/// unreachable on compiler-produced IR (both fields come from the same
/// alphabet); only a hand-edited document could reach it.
fn declared(w: &IrWorld, pick: impl Fn(&IrTape) -> Option<&Vec<String>>) -> Vec<Clause> {
    w.tapes
        .iter()
        .enumerate()
        .filter_map(|(tape, t)| {
            let glyphs = pick(t)?;
            let indices: Vec<u32> = glyphs
                .iter()
                .filter_map(|g| t.glyphs.iter().position(|x| x == g))
                .map(|i| i as u32)
                .collect();
            if indices.len() != glyphs.len() {
                debug_assert!(
                    false,
                    "tape `{}` declares a clause glyph its own table has no index for",
                    t.name
                );
                return None;
            }
            Some(Clause {
                tape,
                name: t.name.clone(),
                indices,
            })
        })
        .collect()
}

/// Whether `rule` RETURNS from the routine — as its own terminal
/// transition, or as the resume point of a call it makes, which is the
/// same moment control goes back to the caller. A rule carries at most one
/// of the two (a `CallThen` transition is not itself a return), so the
/// answer is a single bit.
///
/// Everything else stays inside or never comes back: a `goto`, a call
/// resuming at a state, a tail call (whose callee returns to the ORIGINAL
/// caller, never here), `stop`/`halt` in either position, and `goto <state
/// parameter>` — see the module doc for why the last of those is not a
/// `leaves` row.
fn returns_from_routine(rule: &IrRule) -> bool {
    matches!(
        &rule.transition,
        IrTransition::Return
            | IrTransition::CallThen {
                then: Some(IrThen::Return),
                ..
            }
    )
}

/// Send `rule`'s return through the check chain at `to` instead: the
/// terminal becomes a `goto`, and a call's resume point becomes a resume
/// AT the chain — whose own last rows then return. A call whose resume
/// moves this way can no longer be a tail call (the `tail_call` pass keys
/// on a `return` resume), which costs a contracted routine that one pass
/// and changes nothing observable.
fn redirect(rule: &mut IrRule, to: u32) {
    match &mut rule.transition {
        IrTransition::Return => rule.transition = IrTransition::Goto { state: to },
        IrTransition::CallThen { then, .. } => *then = Some(IrThen::Goto { state: to }),
        _ => unreachable!("`returns_from_routine` selected this rule"),
    }
}

/// Append one check state per clause — the LAST clause first, so each is
/// built knowing the id of the state its passing rows hand control to —
/// and return the id of the first, the one control must reach. The last
/// state's passing rows carry `finally` itself.
///
/// The states are appended in push order, so ids stay dense, and each name
/// is freshened against every name already spoken for. There is no
/// reserved state-name namespace in this crate, so a source state called
/// `enters_num` keeps its name and the synthesized one takes
/// `enters_num_1`; the name is a label a reader may see, never a key
/// anything resolves by. It deliberately does NOT repeat the routine —
/// the enclosing `.func` already names it, and a mangled routine name
/// carries `::`, which is not a legal bare assembly label.
fn chain(
    states: &mut Vec<IrState>,
    used: &mut HashSet<String>,
    arity: usize,
    line: u32,
    clauses: &[Clause],
    kind: &str,
    finally: IrTransition,
) -> u32 {
    let mut onward = finally;
    let mut head = None;
    for clause in clauses.iter().rev() {
        let id = states.len() as u32;
        let mut rules: Vec<IrRule> = clause
            .indices
            .iter()
            .map(|&index| IrRule {
                pattern: only(arity, clause.tape, index),
                write: None,
                moves: None,
                debugger: false,
                transition: onward.clone(),
                // `synthesized` gates traps, and a passing row carries
                // none — the same reading `ir::Forwarders` gives its own
                // minted rows, and what keeps `jump_threading`'s "a
                // forwarder is never synthesized" invariant true.
                synthesized: false,
                direct: false,
                line,
            })
            .collect();
        rules.push(IrRule {
            pattern: vec![IrCell::Wildcard; arity],
            write: None,
            moves: None,
            debugger: false,
            transition: IrTransition::TrapContract,
            synthesized: true,
            direct: false,
            line,
        });
        states.push(IrState {
            id,
            name: fresh_state_name(used, &format!("{kind}_{}", clause.name)),
            line,
            rules,
            dispatch: IrDispatch::Table,
        });
        onward = IrTransition::Goto { state: id };
        head = Some(id);
    }
    head.expect("a chain is built only for a non-empty clause list")
}

/// An arity-wide pattern naming one symbol on `tape` and any symbol
/// everywhere else — a check reads ONE tape, and reading a second would
/// make the rows a cross-tape product.
fn only(arity: usize, tape: usize, index: u32) -> Vec<IrCell> {
    let mut pattern = vec![IrCell::Wildcard; arity];
    pattern[tape] = IrCell::Index { index };
    pattern
}
