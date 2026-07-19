//! dispatch-select: pick the compact branch lowering for a state whose match is
//! exactly "one selective row, then an all-wildcard catch-all". Such a state is
//! flagged [`IrDispatch::Branch`]; codegen then emits `rd; mtc T<n>; jm <first>`
//! with a ONE-row match table (the selective row) and the catch-all inline as
//! the fall-through — no dispatch table (crate::codegen). The pass changes no IR
//! shape beyond the flag: the two rules stay exactly as they are.
//!
//! A state qualifies iff all of:
//!   * it lives in the `machine` world (see the mono-linkability note below);
//!   * it has exactly two rules;
//!   * the second rule's pattern is all-wildcard (a `[*,…]` catch-all);
//!   * the first rule's pattern is NOT all-wildcard (a selective row);
//!   * its dispatch is still the canonical [`IrDispatch::Table`] (idempotence —
//!     a state already switched to `Branch` is left alone).
//!
//! Why the two-row guards are exactly these: `jm`/`jnm` test the match register
//! only (MR≠0 means "some row matched"), so a branch can express exactly "the
//! selective row matched → its block; else → the catch-all". A state whose
//! second row is NOT an all-wildcard catch-all needs the trap-on-no-match
//! behaviour a `djmp` gives (MR=0 traps NoTransition), which a fall-through
//! cannot express — so it keeps the table. A state whose FIRST row is already
//! all-wildcard has a shadowed second row: once `dead_rows` drops it, one
//! all-wildcard row remains, which codegen lowers straight-line — so it is not a
//! branch candidate either.
//!
//! **Machine world only — mono-linkability.** The compiled object is
//! mode-independent: one object links under all three call mechanisms (mono /
//! frames / hybrid). Under `--call-mech=mono` a HOLEY binding stamps unmapped-
//! read trap rows into the CALLEE's match table and routes hole symbols through
//! a dispatch jump; a callee that reads its match through a conditional branch
//! instead would misroute those holes, so the linker refuses (a holey binding
//! needs the callee's match consumed by a dispatch jump). A routine can be such
//! a holey-binding callee, so flipping a routine's state to the `jm` form would
//! make the same object fail a mono link where the table form succeeded — a
//! behavioural change the equivalence contract forbids. The `machine` world is
//! the one world nothing ever `call`s, so its states are never a binding callee
//! and are always safe to branch; routines are left to the table form. Flipping
//! a routine that is only bindless- or fully-bound (never holey) would also be
//! safe, but proving that needs cross-world call-site analysis — a recorded
//! trigger for a later program-level widening of this pass. Part of the `-O1`
//! pipeline (optimizer/mod.rs), after `dead_rows` (which can reduce a state to
//! the two-row shape this pass targets).

use crate::ir::{IrCell, IrDispatch, IrWorld, IrWorldKind};

/// Whether a match pattern is all-wildcard (`[*,…]` — matches every input).
fn all_wildcard(pattern: &[IrCell]) -> bool {
    pattern.iter().all(|c| matches!(c, IrCell::Wildcard))
}

pub fn run(w: &mut IrWorld) -> u32 {
    // Only the machine world is provably never a binding callee, so only its
    // states are safe to branch under a mono link (see the module doc). Routines
    // keep the table form.
    if w.kind != IrWorldKind::Machine {
        return 0;
    }
    let mut changes = 0u32;
    for st in &mut w.states {
        // Idempotence: only the canonical table form is a candidate. A state
        // already flagged `Branch` stays put, so a second pass reports no change
        // and the fixpoint converges.
        if st.dispatch != IrDispatch::Table {
            continue;
        }
        if st.rules.len() == 2
            && all_wildcard(&st.rules[1].pattern)
            && !all_wildcard(&st.rules[0].pattern)
        {
            st.dispatch = IrDispatch::Branch;
            changes += 1;
        }
    }
    changes
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compiler::analyze;
    use crate::expand::expand;
    use crate::ir::{IrProgram, lower, validate_world};

    fn ir_of(src: &str) -> IrProgram {
        let a = analyze(src).unwrap_or_else(|e| panic!("analyze: {e}"));
        let ex = expand(&a.resolved).unwrap_or_else(|e| panic!("expand: {e}"));
        lower(&ex, &a.resolved)
            .unwrap_or_else(|e| panic!("lower: {e}"))
            .0
    }

    #[test]
    fn a_selective_row_then_a_catch_all_flips_to_branch() {
        // scan: ['a'] -> … goto scan  (selective, exact) ; [*] -> stop (catch-all).
        // The canonical two-row branch shape.
        let mut ir = ir_of(
            "alphabet ab { '_', 'a', 'b' }
machine {
  tape t: ab;
  entry state scan {
    ['a'] -> write ['b'] move [>] goto scan;
    [*]   -> stop;
  }
}",
        );
        let m = &mut ir.worlds[0];
        assert_eq!(m.states[0].dispatch, IrDispatch::Table, "canonical before");
        assert_eq!(run(m), 1, "the two-row state flips");
        assert_eq!(m.states[0].dispatch, IrDispatch::Branch);
        // No shape change beyond the flag: both rules survive untouched.
        assert_eq!(m.states[0].rules.len(), 2);
        validate_world(m).unwrap();
    }

    #[test]
    fn selecting_is_idempotent() {
        // A second application finds the state already Branch and does nothing —
        // the fixpoint would otherwise never converge.
        let mut ir = ir_of(
            "alphabet ab { '_', 'a', 'b' }
machine {
  tape t: ab;
  entry state scan {
    ['a'] -> write ['b'] move [>] goto scan;
    [*]   -> stop;
  }
}",
        );
        let m = &mut ir.worlds[0];
        assert_eq!(run(m), 1);
        assert_eq!(run(m), 0, "re-running the pass reports no further change");
        assert_eq!(m.states[0].dispatch, IrDispatch::Branch);
    }

    #[test]
    fn a_routine_state_is_not_flipped_even_when_it_qualifies() {
        // The exact two-row shape, but in a ROUTINE: a routine can be a
        // holey-binding callee whose mono lowering needs table dispatch, so the
        // pass leaves routine states as tables (mono-linkability — see the
        // module doc). Only the machine world's matching state flips.
        let mut ir = ir_of(
            "alphabet bits { '_', '0', '1' }
namespace mylib {
  export routine plusOne(tape num: bits) {
    entry state inc {
      ['1'] -> write ['0'] move [<] goto inc;
      [*]   -> write ['1'] return;
    }
  }
}
use mylib::plusOne;
machine {
  tape ctl: bits;
  entry state main { ['1'] -> call plusOne(num = ctl) then done; [*] -> stop; }
  state done { [*] -> stop; }
}",
        );
        let inc = ir
            .worlds
            .iter_mut()
            .find(|w| w.name == "mylib::plusOne")
            .expect("the routine world");
        assert_eq!(run(inc), 0, "a routine's qualifying state is not flipped");
        assert_eq!(inc.states[0].dispatch, IrDispatch::Table);

        // The machine's own two-row `main` state DOES flip.
        let m = ir
            .worlds
            .iter_mut()
            .find(|w| w.kind == IrWorldKind::Machine)
            .expect("the machine world");
        assert_eq!(run(m), 1, "the machine world's matching state flips");
    }

    #[test]
    fn a_straight_line_state_is_not_a_candidate() {
        // A single all-wildcard row is codegen's straight-line shape, not a
        // branch. The pass leaves it as the canonical table hint (unused there).
        let mut ir = ir_of(
            "alphabet ab { '_', 'a' }
machine { tape t: ab; entry state s { [*] -> stop; } }",
        );
        let m = &mut ir.worlds[0];
        assert_eq!(run(m), 0);
        assert_eq!(m.states[0].dispatch, IrDispatch::Table);
    }

    #[test]
    fn a_three_row_state_keeps_the_table() {
        // Three exact rows: only a two-row shape branches; this keeps the
        // match/dispatch table.
        let mut ir = ir_of(
            "alphabet bits { '_', '0', '1' }
machine {
  tape num: bits;
  entry state inc {
    ['1'] -> write ['0'] move [<] goto inc;
    ['0'] -> write ['1'] stop;
    ['_'] -> write ['1'] stop;
  }
}",
        );
        let m = &mut ir.worlds[0];
        assert_eq!(run(m), 0, "a three-row state is not a branch candidate");
        assert_eq!(m.states[0].dispatch, IrDispatch::Table);
    }

    #[test]
    fn a_two_row_state_without_a_catch_all_keeps_the_table() {
        // Two exact rows, no catch-all: a non-match must trap NoTransition via
        // `djmp` (MR=0), which a fall-through cannot express — keep the table.
        let mut ir = ir_of(
            "alphabet ab { '_', 'a', 'b' }
machine {
  tape t: ab;
  entry state s {
    ['a'] -> stop;
    ['b'] -> halt;
  }
}",
        );
        let m = &mut ir.worlds[0];
        assert_eq!(run(m), 0, "no all-wildcard second row → not a candidate");
        assert_eq!(m.states[0].dispatch, IrDispatch::Table);
    }

    #[test]
    fn a_two_row_state_whose_first_row_is_all_wildcard_is_not_flipped() {
        // First row all-wildcard shadows the second: `dead_rows` will reduce this
        // to a single all-wildcard row (straight-line), so it is not a branch
        // candidate. Built by lowering a valid two-row state, then widening the
        // first row to all-wildcard (an arrangement the front end need not accept
        // directly).
        let mut ir = ir_of(
            "alphabet ab { '_', 'a', 'b' }
machine {
  tape t: ab;
  entry state s {
    ['a'] -> stop;
    [*]   -> halt;
  }
}",
        );
        let m = &mut ir.worlds[0];
        m.states[0].rules[0].pattern = vec![IrCell::Wildcard];
        assert_eq!(run(m), 0, "a leading all-wildcard row is not a candidate");
        assert_eq!(m.states[0].dispatch, IrDispatch::Table);
    }
}
