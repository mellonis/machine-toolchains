//! jump-threading (state-graph form): an inbound reference to an EMPTY
//! forwarder state — a state whose single all-wildcard row has no write, no
//! move, no `debugger`, and whose only action is a `goto` — is retargeted to
//! the forwarder's own destination. Chains collapse in one application (the
//! resolver chases them transitively); a cycle of empty forwarders is a
//! deliberate infinite loop (`state spin { [*] -> goto spin; }`) and is
//! preserved untouched. The forwarders themselves stay in place: they become
//! unreachable, and `dce` deletes them (single responsibility). Part of the
//! `-O1` pipeline (optimizer/mod.rs).
//!
//! The `!debugger` guard is the brk barrier for this pass: a forwarder-shaped
//! row carrying a `brk` is an observability pause point, so threading through
//! it (eliding the pause) is forbidden. A forwarder always carries a `goto`,
//! and lowering marks only trap rows `synthesized`, so a forwarder is never
//! synthesized — no `synthesized` check is needed.

use std::collections::{HashMap, HashSet};

use crate::ir::{IrCell, IrState, IrThen, IrTransition, IrWorld};

/// The destination of an empty forwarder, or `None` if `st` is not one.
fn forwards_to(st: &IrState) -> Option<u32> {
    if st.rules.len() != 1 {
        return None;
    }
    let r = &st.rules[0];
    if let IrTransition::Goto { state } = r.transition
        && r.write.is_none()
        && r.moves.is_none()
        && !r.debugger
        && r.pattern.iter().all(|c| matches!(c, IrCell::Wildcard))
    {
        Some(state)
    } else {
        None
    }
}

pub fn run(w: &mut IrWorld) -> u32 {
    let forward: HashMap<u32, u32> = w
        .states
        .iter()
        .filter_map(|st| forwards_to(st).map(|t| (st.id, t)))
        .collect();
    if forward.is_empty() {
        return 0;
    }
    let resolve = |start: u32| -> u32 {
        let mut seen = HashSet::new();
        let mut cur = start;
        while let Some(&next) = forward.get(&cur) {
            if !seen.insert(cur) {
                return start; // a forwarder cycle: keep the loop as written
            }
            cur = next;
        }
        cur
    };

    let mut changes = 0u32;
    // The world entry is an inbound reference too.
    let new_entry = resolve(w.entry);
    if new_entry != w.entry {
        w.entry = new_entry;
        changes += 1;
    }
    for st in &mut w.states {
        for r in &mut st.rules {
            match &mut r.transition {
                IrTransition::Goto { state } => {
                    let new = resolve(*state);
                    if new != *state {
                        *state = new;
                        changes += 1;
                    }
                }
                IrTransition::CallThen { then, .. } => {
                    if let IrThen::Goto { state } = then {
                        let new = resolve(*state);
                        if new != *state {
                            *state = new;
                            changes += 1;
                        }
                    }
                }
                // `TailCall` has no in-world target to thread (its target is
                // another world), like the terminators.
                IrTransition::TailCall { .. }
                | IrTransition::Return
                | IrTransition::Stop
                | IrTransition::Halt
                | IrTransition::TrapRead
                | IrTransition::TrapWrite => {}
            }
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

    /// analyze → expand → lower to the IR the passes transform.
    fn ir_of(src: &str) -> IrProgram {
        let a = analyze(src).unwrap_or_else(|e| panic!("analyze: {e}"));
        let ex = expand(&a.resolved).unwrap_or_else(|e| panic!("expand: {e}"));
        lower(&ex, &a.resolved)
            .unwrap_or_else(|e| panic!("lower: {e}"))
            .0
    }

    #[test]
    fn forwarder_chain_collapses_to_final_target() {
        // go(0) → fwd(1) → done(2); done is the real work (a terminator).
        let mut ir = ir_of(
            "alphabet ab { '_', 'a' }
machine {
  tape t: ab;
  entry state go { [*] -> goto fwd; }
  state fwd      { [*] -> goto done; }
  state done     { [*] -> stop; }
}",
        );
        let m = &mut ir.worlds[0];
        assert_eq!(m.entry, 0);
        let changes = run(m);
        // The entry threads past both forwarders to `done`, and go's own edge
        // does too — two retargets.
        assert_eq!(changes, 2);
        assert_eq!(m.entry, 2, "entry now targets done directly");
        assert_eq!(
            m.states[0].rules[0].transition,
            IrTransition::Goto { state: 2 }
        );
        // The forwarders are left in place for dce to remove.
        assert_eq!(m.states.len(), 3);
        validate_world(m).unwrap();
    }

    #[test]
    fn empty_self_loop_is_preserved() {
        let mut ir = ir_of(
            "alphabet ab { '_', 'a' }
machine { tape t: ab; entry state spin { [*] -> goto spin; } }",
        );
        let m = &mut ir.worlds[0];
        assert_eq!(run(m), 0, "a forwarder cycle is a deliberate infinite loop");
        assert_eq!(m.entry, 0);
        assert_eq!(
            m.states[0].rules[0].transition,
            IrTransition::Goto { state: 0 }
        );
    }

    #[test]
    fn a_brk_bearing_forwarder_is_not_threaded_through() {
        // go(0) is a plain forwarder → brkfwd(1); brkfwd carries a `debugger`,
        // so it is NOT a forwarder — threading stops at it, its brk row and
        // goto survive.
        let mut ir = ir_of(
            "alphabet ab { '_', 'a' }
machine {
  tape t: ab;
  entry state go { [*] -> goto brkfwd; }
  state brkfwd   { [*] -> debugger goto done; }
  state done     { [*] -> stop; }
}",
        );
        let m = &mut ir.worlds[0];
        let changes = run(m);
        assert_eq!(changes, 1, "only the entry threads past go");
        assert_eq!(m.entry, 1, "entry stops at the brk-bearing forwarder");
        assert!(m.states[1].rules[0].debugger, "the brk row survives");
        assert_eq!(
            m.states[1].rules[0].transition,
            IrTransition::Goto { state: 2 }
        );
        validate_world(m).unwrap();
    }

    #[test]
    fn states_with_actions_or_multiple_rows_are_not_forwarders() {
        // go has two rows; work writes; done is a terminator — none forward, so
        // the pass has nothing to do and returns early.
        let mut ir = ir_of(
            "alphabet ab { '_', 'a' }
machine {
  tape t: ab;
  entry state go { ['a'] -> goto work; [*] -> stop; }
  state work     { [*] -> write ['a'] goto done; }
  state done     { [*] -> stop; }
}",
        );
        assert_eq!(run(&mut ir.worlds[0]), 0);
    }
}
