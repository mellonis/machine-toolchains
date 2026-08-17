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

    let mut changes = 0u32;
    // Forwarder retargeting has nothing to do when no state forwards, but
    // marking below still applies — so this is a guard around the
    // resolve-dependent block, not an early return from the whole pass.
    if !forward.is_empty() {
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
                    // `TailCall` has no in-world target to thread (its target
                    // is another world), like the terminators.
                    IrTransition::TailCall { .. }
                    | IrTransition::Return
                    | IrTransition::Stop
                    | IrTransition::Halt
                    | IrTransition::TrapRead
                    | IrTransition::TrapWrite => {}
                }
            }
        }
    }

    // Dispatch-target threading (docs/tmt/optimizer.md (dispatch-target
    // threading)): a bare rule's entry can name its destination state
    // directly, so codegen skips the one-jmp stub. Marking only — the
    // emission change lives in codegen, keyed off `direct`.
    for st in &mut w.states {
        for r in &mut st.rules {
            if !r.direct
                && r.write.is_none()
                && r.moves.is_none()
                && !r.debugger
                && matches!(r.transition, IrTransition::Goto { .. })
            {
                r.direct = true;
                changes += 1;
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
    use crate::ir::{IrDispatch, IrProgram, IrRule, lower, validate_world};

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
        // does too — two retargets. Both forwarders' own rows are also bare
        // gotos in their own right, so marking adds two more changes on top.
        assert_eq!(changes, 4);
        assert_eq!(m.entry, 2, "entry now targets done directly");
        assert_eq!(
            m.states[0].rules[0].transition,
            IrTransition::Goto { state: 2 }
        );
        assert!(m.states[0].rules[0].direct, "go's retargeted row is bare");
        assert!(m.states[1].rules[0].direct, "fwd's own row is bare too");
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
        // The cycle itself is untouched (no retarget) — the one change is
        // marking spin's own row `direct`, which is orthogonal to the loop
        // shape: codegen still just jumps back to spin's own dispatch.
        assert_eq!(run(m), 1, "a forwarder cycle is a deliberate infinite loop");
        assert_eq!(m.entry, 0);
        assert_eq!(
            m.states[0].rules[0].transition,
            IrTransition::Goto { state: 0 }
        );
        assert!(m.states[0].rules[0].direct);
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
        // The entry threads past go (1 retarget), and go's own row — already
        // pointing straight at brkfwd, no retarget needed — is also a bare
        // goto in its own right, so marking adds one more change.
        assert_eq!(changes, 2, "the entry threads past go, then go's row marks");
        assert_eq!(m.entry, 1, "entry stops at the brk-bearing forwarder");
        assert!(m.states[0].rules[0].direct, "go's own row is bare");
        assert!(m.states[1].rules[0].debugger, "the brk row survives");
        assert!(
            !m.states[1].rules[0].direct,
            "the brk row itself stays unmarked"
        );
        assert_eq!(
            m.states[1].rules[0].transition,
            IrTransition::Goto { state: 2 }
        );
        validate_world(m).unwrap();
    }

    #[test]
    fn states_with_actions_or_multiple_rows_are_not_forwarders() {
        // go has two rows; work writes; done is a terminator — none of the
        // three STATES forward (forwarder retargeting has nothing to do).
        // But marking is per-rule, not per-state: go's first row is itself a
        // bare goto, so it gets marked even though go as a whole isn't a
        // forwarder.
        let mut ir = ir_of(
            "alphabet ab { '_', 'a' }
machine {
  tape t: ab;
  entry state go { ['a'] -> goto work; [*] -> stop; }
  state work     { [*] -> write ['a'] goto done; }
  state done     { [*] -> stop; }
}",
        );
        let m = &mut ir.worlds[0];
        assert_eq!(run(m), 1);
        assert!(m.states[0].rules[0].direct, "go's bare row marks");
        assert!(!m.states[0].rules[1].direct, "the stop row has no goto");
        assert!(!m.states[1].rules[0].direct, "work's row carries a write");
    }

    #[test]
    fn a_bare_goto_rule_is_marked_direct() {
        // `go` carries two rows so it is not itself a whole-state forwarder
        // (`forwards_to` requires exactly one row) — this isolates marking
        // from the forwarder-retargeting phase above.
        let mut ir = ir_of(
            "alphabet ab { '_', 'a' }
machine {
  tape t: ab;
  entry state go {
    ['a'] -> goto done;
    [*]   -> stop;
  }
  state done { [*] -> stop; }
}",
        );
        let m = &mut ir.worlds[0];
        assert_eq!(run(m), 1, "the one bare goto row marks direct");
        assert!(m.states[0].rules[0].direct);
        assert!(
            !m.states[0].rules[1].direct,
            "the stop row has no goto to mark"
        );
        validate_world(m).unwrap();
    }

    #[test]
    fn a_debugger_rule_is_not_marked() {
        // A bare-shaped goto that also carries `debugger` stays unmarked: the
        // brk barrier applies to marking the same way it applies to
        // forwarder threading (module doc).
        let mut ir = ir_of(
            "alphabet ab { '_', 'a' }
machine {
  tape t: ab;
  entry state go { [*] -> debugger goto done; }
  state done     { [*] -> stop; }
}",
        );
        let m = &mut ir.worlds[0];
        assert_eq!(run(m), 0);
        assert!(!m.states[0].rules[0].direct);
    }

    #[test]
    fn non_goto_transitions_are_not_marked() {
        // Stop / Halt / CallThen come straight from source. TailCall and
        // TrapRead have no source spelling (TailCall is optimizer-only,
        // TrapRead is a graft-hole synthesis), so they're appended by hand —
        // the same technique dead_rows.rs uses for its trap-row test. Return
        // needs a routine world and is checked separately below.
        let mut ir = ir_of(
            "alphabet ab { '_', 'a' }
use lib::ext;
machine {
  tape t: ab;
  entry state go { [*] -> call ext() then done; }
  state stopper  { [*] -> stop; }
  state halter   { [*] -> halt; }
  state done     { [*] -> stop; }
}",
        );
        let m = &mut ir.worlds[0];
        let bare_rule = |transition: IrTransition, synthesized: bool| IrRule {
            pattern: vec![IrCell::Wildcard],
            write: None,
            moves: None,
            debugger: false,
            transition,
            synthesized,
            direct: false,
            line: 0,
        };
        let tail_id = m.states.len() as u32;
        m.states.push(IrState {
            id: tail_id,
            name: "tail".into(),
            line: 0,
            rules: vec![bare_rule(
                IrTransition::TailCall {
                    target: "lib::ext".into(),
                },
                false,
            )],
            dispatch: IrDispatch::default(),
        });
        m.states.push(IrState {
            id: tail_id + 1,
            name: "trapper".into(),
            line: 0,
            rules: vec![bare_rule(IrTransition::TrapRead, true)],
            dispatch: IrDispatch::default(),
        });
        assert_eq!(run(m), 0);
        for st in &m.states {
            for r in &st.rules {
                assert!(
                    !r.direct,
                    "state {} rule stays !direct (transition {:?})",
                    st.name, r.transition
                );
            }
        }
        validate_world(m).unwrap();

        let mut routine_ir = ir_of(
            "alphabet ab { '_', 'a' }
routine r(tape t: ab) { entry state s { [*] -> return; } }
machine { tape t: ab; entry state go { [*] -> stop; } }",
        );
        let idx = routine_ir
            .worlds
            .iter()
            .position(|w| w.name == "r")
            .expect("routine world");
        let rw = &mut routine_ir.worlds[idx];
        assert_eq!(run(rw), 0);
        assert!(!rw.states[0].rules[0].direct);
        validate_world(rw).unwrap();
    }

    #[test]
    fn a_rule_with_a_write_or_move_is_not_marked() {
        let mut ir = ir_of(
            "alphabet ab { '_', 'a' }
machine {
  tape t: ab;
  entry state go {
    [*] -> write ['a'] goto writer;
  }
  state writer {
    ['a'] -> move [>] goto done;
    [*]   -> stop;
  }
  state done { [*] -> stop; }
}",
        );
        let m = &mut ir.worlds[0];
        assert_eq!(run(m), 0);
        assert!(!m.states[0].rules[0].direct, "the write rule stays !direct");
        assert!(!m.states[1].rules[0].direct, "the move rule stays !direct");
    }

    #[test]
    fn marking_is_idempotent_for_the_fixpoint() {
        let mut ir = ir_of(
            "alphabet ab { '_', 'a' }
machine {
  tape t: ab;
  entry state go {
    ['a'] -> goto done;
    [*]   -> stop;
  }
  state done { [*] -> stop; }
}",
        );
        let m = &mut ir.worlds[0];
        assert!(run(m) > 0, "the first application marks the bare row");
        assert_eq!(
            run(m),
            0,
            "the second application re-marks nothing — the fixpoint driver converges"
        );
        validate_world(m).unwrap();
    }
}
