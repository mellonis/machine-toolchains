//! tail-merge (state-graph form): whole-STATE dedup. Two states in one world
//! whose rule lists are IDENTICAL — same length, and for each row the same
//! pattern, write, moves, `debugger`, `synthesized`, and transition (LINE
//! provenance ignored) — AND the same [`IrDispatch`] hint collapse to one: the
//! lower-id state is kept, every reference to the other (the world entry, a
//! `goto`, a `call … then goto` resume) is retargeted onto the keeper, the
//! duplicate is dropped, and the survivors are densely renumbered (the shared
//! `renumber_dense`). The change count is the number of states removed. Part of
//! the `-O1` pipeline (optimizer/mod.rs), AFTER tail-call.
//!
//! Because two transitions are equal only when their in-world target ids are
//! equal, states that differ only in where they `goto` never merge — including
//! two structurally-identical self-loops (their `goto` targets are their own
//! distinct ids). That is conservative but always sound: an equal transition
//! (including its numeric target) guarantees identical behaviour after the
//! retarget, so a merge never changes the run. It mirrors the `.pmc` pass's
//! id-based `term` comparison.
//!
//! **The brk barrier.** A state containing a `debugger` row is NEVER merged —
//! not as keeper, not as duplicate. Collapsing two identical brk-bearing states
//! would fuse two distinct observable pause addresses into one, changing the
//! debugging surface, which the equivalence contract's brk barrier (optimizer/
//! mod.rs) forbids. (This is stricter than the `.pmc` block-merge, which treats
//! `brk` as an ordinary equal op and would merge such blocks; the TM optimizer
//! holds the barrier here instead.)
//!
//! **Return-chaining (the `.pmc` pass's part b) does not transpose.** In the
//! `.pmc` CFG it shares one physical terminal between an adjacent pair of empty
//! Return blocks via a fall-through edge. The TM IR has no such notion: a
//! terminal is a per-rule transition, not a separate block, and codegen already
//! elides fall-through for `goto`s while emitting a one-byte `ret`/`stp`/`hlt`
//! inline (routing to a shared terminal through a multi-byte `jmp` would be
//! strictly larger). The one shape it could target — two states each being a
//! single all-wildcard `return` — is already collapsed by the whole-state dedup
//! above, so part (b) is subsumed, not ported.

use crate::ir::{IrRule, IrState, IrThen, IrTransition, IrWorld};

use super::renumber_dense;

/// Whether a state carries any `debugger` row (the brk-barrier veto).
fn has_debugger(st: &IrState) -> bool {
    st.rules.iter().any(|r| r.debugger)
}

/// Two rows are equivalent for merging iff every field but the source line
/// matches. `pattern`/`write`/`moves`/`transition` compare structurally
/// (their `PartialEq`); the transition's in-world target id must match exactly.
fn rule_equiv(a: &IrRule, b: &IrRule) -> bool {
    a.pattern == b.pattern
        && a.write == b.write
        && a.moves == b.moves
        && a.debugger == b.debugger
        && a.synthesized == b.synthesized
        && a.transition == b.transition
}

/// Two states are mergeable iff their dispatch hint matches and their rule
/// lists are row-for-row equivalent. (Callers must first exclude debugger
/// states.)
fn state_equiv(a: &IrState, b: &IrState) -> bool {
    a.dispatch == b.dispatch
        && a.rules.len() == b.rules.len()
        && a.rules.iter().zip(&b.rules).all(|(x, y)| rule_equiv(x, y))
}

/// Retarget every reference to state id `from` (the world entry, `goto`s, and
/// `call … then goto` resumes) onto `to`.
fn retarget(w: &mut IrWorld, from: u32, to: u32) {
    let fix = |s: &mut u32| {
        if *s == from {
            *s = to;
        }
    };
    if w.entry == from {
        w.entry = to;
    }
    for st in &mut w.states {
        for r in &mut st.rules {
            match &mut r.transition {
                IrTransition::Goto { state } => fix(state),
                IrTransition::CallThen { then, .. } => {
                    if let IrThen::Goto { state } = then {
                        fix(state);
                    }
                }
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

pub fn run(w: &mut IrWorld) -> u32 {
    let mut changes = 0u32;
    // Find one mergeable pair, retarget onto the keeper, drop the duplicate,
    // repeat. Removing the duplicate leaves ids temporarily non-dense; the
    // final `renumber_dense` restores the `id == position` invariant. Each
    // iteration shrinks the state set, so the loop terminates. Re-scanning from
    // scratch lets a merge that made two OTHER states identical be caught next.
    loop {
        let mut found: Option<(usize, usize)> = None; // (keeper pos, dup pos)
        'scan: for i in 0..w.states.len() {
            if has_debugger(&w.states[i]) {
                continue;
            }
            for j in (i + 1)..w.states.len() {
                if has_debugger(&w.states[j]) {
                    continue;
                }
                if state_equiv(&w.states[i], &w.states[j]) {
                    found = Some((i, j));
                    break 'scan;
                }
            }
        }
        let Some((keep_pos, dup_pos)) = found else {
            break;
        };
        let keeper_id = w.states[keep_pos].id;
        let dup_id = w.states[dup_pos].id;
        retarget(w, dup_id, keeper_id);
        w.states.remove(dup_pos); // dup_pos > keep_pos ≥ 0, never the last keeper
        changes += 1;
    }
    if changes > 0 {
        renumber_dense(w);
    }
    changes
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compiler::analyze;
    use crate::expand::expand;
    use crate::ir::{IrDispatch, IrProgram, lower, validate_world};

    fn ir_of(src: &str) -> IrProgram {
        let a = analyze(src).unwrap_or_else(|e| panic!("analyze: {e}"));
        let ex = expand(&a.resolved).unwrap_or_else(|e| panic!("expand: {e}"));
        lower(&ex, &a.resolved)
            .unwrap_or_else(|e| panic!("lower: {e}"))
            .0
    }

    #[test]
    fn two_identical_states_merge_and_references_retarget() {
        // `x` and `y` are both `[*] -> stop` — identical. Merge keeps the lower
        // id and retargets `go`'s `[*] -> goto y` onto it; `y` is dropped and
        // the survivors renumber densely.
        let mut ir = ir_of(
            "alphabet ab { '_', 'a' }
machine {
  tape t: ab;
  entry state go { ['a'] -> goto x; [*] -> goto y; }
  state x { [*] -> stop; }
  state y { [*] -> stop; }
}",
        );
        let m = &mut ir.worlds[0];
        assert_eq!(m.states.len(), 3);
        assert_eq!(run(m), 1, "one duplicate state removed");
        assert_eq!(m.states.len(), 2);
        // Both of go's edges now point at the surviving `x` (renumbered to 1).
        let go = &m.states[m.entry as usize];
        let x_id = m.states.iter().find(|s| s.name == "x").unwrap().id;
        for r in &go.rules {
            assert_eq!(r.transition, IrTransition::Goto { state: x_id });
        }
        assert!(m.states.iter().all(|s| s.name != "y"), "y merged away");
        validate_world(m).unwrap();
    }

    #[test]
    fn identical_states_that_both_carry_a_debugger_do_not_merge() {
        // The brk barrier: two identical brk-bearing states are NOT merged —
        // two observable pause addresses must not collapse to one.
        let mut ir = ir_of(
            "alphabet ab { '_', 'a' }
machine {
  tape t: ab;
  entry state go { ['a'] -> goto x; [*] -> goto y; }
  state x { [*] -> debugger stop; }
  state y { [*] -> debugger stop; }
}",
        );
        let m = &mut ir.worlds[0];
        assert_eq!(run(m), 0, "debugger-bearing states are barred from merging");
        assert_eq!(m.states.len(), 3, "all three states survive");
        let brk_states = m
            .states
            .iter()
            .filter(|s| s.rules.iter().any(|r| r.debugger))
            .count();
        assert_eq!(brk_states, 2, "both pause points survive");
        validate_world(m).unwrap();
    }

    #[test]
    fn states_with_differing_transitions_do_not_merge() {
        // The near-miss: `x` stops, `y` halts — same pattern/action, different
        // terminal. They must NOT merge.
        let mut ir = ir_of(
            "alphabet ab { '_', 'a' }
machine {
  tape t: ab;
  entry state go { ['a'] -> goto x; [*] -> goto y; }
  state x { [*] -> stop; }
  state y { [*] -> halt; }
}",
        );
        let m = &mut ir.worlds[0];
        assert_eq!(run(m), 0, "different terminals are not equivalent");
        assert_eq!(m.states.len(), 3);
    }

    #[test]
    fn states_differing_only_in_the_dispatch_hint_do_not_merge() {
        // Same rules, but the dispatch hint differs — `dispatch_select` may set
        // one to Branch. The hint is part of state identity, so no merge.
        let mut ir = ir_of(
            "alphabet ab { '_', 'a' }
machine {
  tape t: ab;
  entry state go { ['a'] -> goto x; [*] -> goto y; }
  state x { [*] -> stop; }
  state y { [*] -> stop; }
}",
        );
        let m = &mut ir.worlds[0];
        let y_pos = m.states.iter().position(|s| s.name == "y").unwrap();
        m.states[y_pos].dispatch = IrDispatch::Branch;
        assert_eq!(run(m), 0, "a differing dispatch hint blocks the merge");
        assert_eq!(m.states.len(), 3);
    }

    #[test]
    fn fully_distinct_world_is_untouched() {
        let mut ir = ir_of(
            "alphabet ab { '_', 'a' }
machine {
  tape t: ab;
  entry state scan { ['a'] -> move [>] goto scan; [*] -> stop; }
}",
        );
        assert_eq!(run(&mut ir.worlds[0]), 0);
    }
}
