//! dce: delete states unreachable from the world entry, walking the intra-world
//! edges only — a `goto` and a `call … then goto` resume target. A `call`'s
//! target names ANOTHER world, so it is not an intra-world edge (whole uncalled
//! worlds are dropped by the linker's reachability pass, not here). Deletion is
//! reachability-only, so a reachable transition can never be left dangling.
//!
//! The TM IR requires dense ids (`state.id == position`; validate_world), so a
//! deletion RENUMBERS the survivors densely (preserving emission order) and
//! retargets the world entry, every `goto`, and every `call … then goto` to the
//! new ids. The change count is the number of states deleted.
//!
//! An unreachable state that carries a `debugger` (`brk`) row is deleted like
//! any other. The brk barrier forbids motion across a REACHABLE brk; an
//! unreachable brk can never fire, so removing the dead state that holds it
//! changes nothing observable — the `.pmc` dce deletes such blocks the same
//! way. Part of the `-O1` pipeline (optimizer/mod.rs).

use std::collections::HashSet;

use crate::ir::{IrThen, IrTransition, IrWorld};

use super::renumber_dense;

pub fn run(w: &mut IrWorld) -> u32 {
    // Reachability from the entry over intra-world edges.
    let mut seen: HashSet<u32> = HashSet::new();
    let mut work = vec![w.entry];
    while let Some(id) = work.pop() {
        if !seen.insert(id) {
            continue;
        }
        for r in &w.states[id as usize].rules {
            match &r.transition {
                IrTransition::Goto { state } => work.push(*state),
                IrTransition::CallThen { then, .. } => {
                    if let IrThen::Goto { state } = then {
                        work.push(*state);
                    }
                }
                // A `TailCall` leaves the world (its target is another world),
                // like the terminators — no intra-world successor.
                IrTransition::TailCall { .. }
                | IrTransition::Return
                | IrTransition::Stop
                | IrTransition::Halt
                | IrTransition::TrapRead
                | IrTransition::TrapWrite => {}
            }
        }
    }
    if seen.len() == w.states.len() {
        return 0;
    }

    let deleted = (w.states.len() - seen.len()) as u32;

    // Drop the unreachable states (survivors keep emission order), then assign
    // dense ids and retarget the entry + every surviving edge. Every edge
    // target is a reachable (surviving) state, so the shared renumber never
    // misses a target.
    w.states.retain(|st| seen.contains(&st.id));
    renumber_dense(w);
    deleted
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
    fn unreachable_state_deleted_and_survivors_renumbered() {
        // a(0) → c(2); b(1) is unreachable. Deleting b renumbers c from 2 to 1,
        // and a's surviving edge must follow the remap (2 → 1) — the real guard
        // (validate_world alone would accept an in-range but WRONG target).
        let mut ir = ir_of(
            "alphabet ab { '_', 'a' }
machine {
  tape t: ab;
  entry state a { [*] -> goto c; }
  state b        { [*] -> halt; }
  state c        { [*] -> stop; }
}",
        );
        let m = &mut ir.worlds[0];
        assert_eq!(m.states.len(), 3);
        assert_eq!(run(m), 1, "one state deleted");
        assert_eq!(m.states.len(), 2);
        assert_eq!(m.entry, 0);
        assert_eq!(m.states[0].name, "a");
        assert_eq!(
            m.states[0].rules[0].transition,
            IrTransition::Goto { state: 1 },
            "a's edge remapped from old id 2 to new id 1"
        );
        assert_eq!(m.states[1].name, "c");
        assert_eq!(m.states[1].id, 1, "c densely renumbered");
        validate_world(m).unwrap();
    }

    #[test]
    fn fully_reachable_world_is_untouched() {
        let mut ir = ir_of(
            "alphabet ab { '_', 'a' }
machine {
  tape t: ab;
  entry state scan { ['a'] -> move [>] goto scan; [*] -> stop; }
}",
        );
        assert_eq!(run(&mut ir.worlds[0]), 0);
    }

    #[test]
    fn unreachable_brk_state_is_deleted() {
        // The dce-vs-brk contract: an unreachable state carrying a `debugger`
        // row is deleted like any other — an unreachable brk can never fire.
        let mut ir = ir_of(
            "alphabet ab { '_', 'a' }
machine {
  tape t: ab;
  entry state go { [*] -> stop; }
  state dead      { [*] -> debugger halt; }
}",
        );
        let m = &mut ir.worlds[0];
        assert!(
            m.states.iter().any(|s| s.rules.iter().any(|r| r.debugger)),
            "the brk row is present before dce"
        );
        assert_eq!(run(m), 1);
        assert_eq!(m.states.len(), 1);
        assert!(
            !m.states.iter().any(|s| s.rules.iter().any(|r| r.debugger)),
            "the unreachable brk state was deleted"
        );
        validate_world(m).unwrap();
    }
}
