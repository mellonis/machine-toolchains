//! `unused-tape`: a machine tape no rule ever touches and no reuse ever
//! binds. A tape is untouched when, across every rule of the machine world,
//! its pattern cell is a wildcard (or the pattern omits it), its write cell
//! keeps the current symbol (`-`, or the write vector omits it), and its move
//! cell stays (`.`, or the move vector omits it) — the tape's head never
//! reads a distinguishing symbol, never writes, and never moves. It must also
//! never be passed as a binding argument to a `call`/`graft`/`bind`, where a
//! spliced or called subgraph could touch it out of the machine's own view.
//!
//! An argument-less `call`, `graft` or `bind` into another unit binds the
//! machine's tapes by position, so a machine making one touches every tape.
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
pub(crate) fn bound_tape_names(world: &ResolvedWorld) -> HashSet<&str> {
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
pub(crate) fn position_is_inert(world: &ResolvedWorld, pos: usize) -> bool {
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

/// Whether `world` reuses another unit with no argument list — a
/// `call`/`bind` whose target is external, or a `graft` whose target is
/// not one of this unit's graphs — which binds its tapes by position and
/// may therefore touch every one of them (docs/tmt/lint.md (unused-tape)).
pub(crate) fn makes_transparent_reuse(ctx: &LintContext, world: &ResolvedWorld) -> bool {
    let call = world.calls.iter().any(|c| {
        matches!(
            &c.target,
            ResolvedCallTarget::Routine { external: true, args, .. } if args.is_empty()
        )
    });
    let bind = world.binds.iter().any(|b| b.external && b.args.is_empty());
    let graft = world
        .grafts
        .iter()
        .any(|g| g.args.is_empty() && ctx.resolved.worlds.iter().all(|w| w.name != g.target));
    call || bind || graft
}

pub(crate) fn check(ctx: &LintContext, out: &mut Vec<Diagnostic>) {
    let Some(idx) = ctx.resolved.entry_world else {
        return;
    };
    let world = &ctx.resolved.worlds[idx];
    if makes_transparent_reuse(ctx, world) {
        return;
    }
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

    const UT: &str = "\
alphabet bin { '_', '^', '$', '0', '1' }
machine {
  tape t: bin;
  entry state go { [*] -> call std::binaryNumbers::plusOne() then stop; }
}
";

    const UT_NEAR: &str = "\
alphabet bin { '_', '^', '$', '0', '1' }
machine {
  tape t: bin;
  tape u: bin;
  entry state go { [*, *] -> call std::binaryNumbers::plusOne(num = t) then stop; }
}
";

    /// Mutation: drop the transparent early return from `check`.
    #[test]
    fn a_transparent_call_into_another_unit_touches_every_tape() {
        assert!(findings(UT).is_empty(), "{:?}", findings(UT));
    }

    /// Mutation: treat every external call as transparent (ignore whether
    /// the argument list is empty).
    #[test]
    fn a_bound_call_touches_only_the_tapes_it_names() {
        let f = findings(UT_NEAR);
        assert_eq!(f.len(), 1, "{f:?}");
        assert!(f[0].contains("`u`"), "{f:?}");
    }
}
