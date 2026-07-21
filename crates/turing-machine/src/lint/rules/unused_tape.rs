//! `unused-tape`: a machine tape no rule ever touches and no reuse ever
//! binds. A tape is untouched when, across every rule of the machine world,
//! its pattern cell is a wildcard (or the pattern omits it), its write cell
//! keeps the current symbol (`-`, or the write vector omits it), and its move
//! cell stays (`.`, or the move vector omits it) — the tape's head never
//! reads a distinguishing symbol, never writes, and never moves. It must also
//! never be passed as a binding argument to a `call`/`graft`/`bind`, where a
//! spliced or called subgraph could touch it out of the machine's own view.
//!
//! New on the lint channel (the deferred hygiene family). Detected
//! source-level over `Resolved`; the machine world is the only world with
//! free-standing tape declarations (routine/graph tapes are signature
//! parameters, obliged by their callers).
//!
//! No fix ships: a tape is a vector position, so deleting one narrows the
//! arity of every pattern / write / move vector in the world at once — not a
//! safe single-span textual edit. The finding is worth surfacing regardless
//! of tidiness: an untouched tape still costs a cell in every emitted row.

use std::collections::HashSet;

use mtc_core::diagnostics::Diagnostic;

use crate::compiler::{ResolvedCallTarget, ResolvedWorld};
use crate::lint::LintContext;
use crate::parser::{BindingArg, BindingValue, MoveDir, PatternCellKind, WriteCellKind};

/// Extend `names` with every bare binding-argument target in `args`. A tape
/// passed to a reuse is bound by its bare name; over-approximating (a state
/// continuation shares this bare shape) can only spare a tape, never wrongly
/// flag one.
fn collect_arg_targets<'a>(args: &'a [BindingArg], names: &mut HashSet<&'a str>) {
    for arg in args {
        if let BindingValue::Named { target, .. } = &arg.value {
            names.insert(target.as_str());
        }
    }
}

/// Every tape name the machine world hands to a reuse (`graft`/`bind`/`call`).
fn bound_tape_names(world: &ResolvedWorld) -> HashSet<&str> {
    let mut names: HashSet<&str> = HashSet::new();
    for graft in &world.grafts {
        collect_arg_targets(&graft.args, &mut names);
    }
    for bind in &world.binds {
        collect_arg_targets(&bind.args, &mut names);
    }
    for call in &world.calls {
        if let ResolvedCallTarget::Routine { args, .. } = &call.target {
            collect_arg_targets(args, &mut names);
        }
    }
    names
}

/// True when position `pos` of the machine world's rules is never
/// distinguished, written, or moved.
fn position_is_inert(world: &ResolvedWorld, pos: usize) -> bool {
    for state in &world.states {
        for rule in &state.rules {
            // Pattern: a Single/Range at this position reads a distinguishing
            // symbol; a wildcard or an omitted cell does not.
            if let Some(cell) = rule.pattern.cells.get(pos)
                && !matches!(cell.kind, PatternCellKind::Wildcard)
            {
                return false;
            }
            // Write: anything but keep (`-`) writes; an omitted vector or cell
            // keeps.
            if let Some(wv) = &rule.write
                && let Some(cell) = wv.cells.get(pos)
                && !matches!(cell.kind, WriteCellKind::Keep)
            {
                return false;
            }
            // Move: anything but stay (`.`) moves; an omitted vector or cell
            // stays.
            if let Some(mv) = &rule.mov
                && let Some(cell) = mv.cells.get(pos)
                && !matches!(cell.dir, MoveDir::Stay)
            {
                return false;
            }
        }
    }
    true
}

pub(crate) fn check(ctx: &LintContext, out: &mut Vec<Diagnostic>) {
    let Some(idx) = ctx.resolved.entry_world else {
        return;
    };
    let world = &ctx.resolved.worlds[idx];
    let bound = bound_tape_names(world);
    for (pos, tape) in world.tapes.iter().enumerate() {
        if bound.contains(tape.name.as_str()) {
            continue;
        }
        if position_is_inert(world, pos) {
            out.push(Diagnostic {
                code: "unused-tape",
                span: tape.span,
                message: format!("tape `{}` is never read, written, or moved", tape.name),
                fix: None,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::lint::{LintOptions, lint};

    fn findings(src: &str) -> Vec<String> {
        lint(src, LintOptions::default())
            .unwrap()
            .diagnostics
            .into_iter()
            .filter(|d| d.code == "unused-tape")
            .map(|d| d.message)
            .collect()
    }

    #[test]
    fn a_tape_no_rule_touches_fires() {
        // `scratch` is all-`*` in the pattern, `-` in the write, `.` in the
        // move, and never a binding argument — dead weight.
        let src = "\
alphabet bit { '_', '1' }
machine {
  tape work: bit;
  tape scratch: bit;
  entry state s { ['1', *] -> write ['_', -] move [>, .] goto s; [*, *] -> stop; }
}
";
        let f = findings(src);
        assert_eq!(f.len(), 1, "{f:?}");
        assert!(f[0].contains("scratch"), "{f:?}");
    }

    #[test]
    fn an_all_wildcard_tape_passed_as_a_binding_argument_is_quiet() {
        // `scratch` is never read/written/moved by any machine rule, but it is
        // handed to the graft — a spliced subgraph owns it now, so silent.
        let src = "\
alphabet bit { '_', '1' }
graph flip(tape t: bit, state done) {
  entry state w { ['_'] -> write ['1'] move [>] done; [*] -> done; }
}
machine {
  tape work: bit;
  tape scratch: bit;
  entry graft flip(t = scratch, done = go) as f;
  state go { ['1', *] -> write ['_', -] move [>, .] goto go; [*, *] -> stop; }
}
";
        assert!(findings(src).is_empty(), "{:?}", findings(src));
    }
}
