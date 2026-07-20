//! dead-rows: within one state, delete a match row that can never fire because
//! an earlier, higher-priority row in the SAME dispatch band already covers
//! every input it would match. The change count is the number of rows deleted.
//! Part of the `-O1` pipeline (optimizer/mod.rs), before `dispatch_select`
//! (deleting the last row of a three-row state can expose the two-row
//! selective-then-catch-all shape that pass targets).
//!
//! # Cover
//!
//! Row `W` covers row `R` cell-wise iff, at every tape position, `W`'s cell is a
//! wildcard, or both cells are the SAME concrete index:
//!   ∀i.  W[i] == `*`  ∨  (W[i] == Index(a) ∧ R[i] == Index(a)).
//! When `W` covers `R`, every input `R` matches, `W` matches too — `W`'s match
//! set is a superset of `R`'s.
//!
//! Only SINGLE-row cover is computed: a row jointly covered by two-or-more
//! earlier rows (whose union of match sets contains it) is NOT deleted. Exact
//! rows are pairwise disjoint (a front-end guarantee), so union-cover would need
//! wildcard-bearing rows, and single-row cover already catches the shadowing the
//! front end warns about (its `shadowed-rule` check, on byte-identical
//! wildcard-bearing rows). Deeper union-cover analysis is a recorded trigger.
//!
//! # Why "same band"
//!
//! Codegen does NOT lower rows in source order. It re-bands a conditional
//! state's rows into `[exact rows, sorted] ++ [partial rows, source order] ++
//! [catch-all rows, source order]` and the match engine takes the FIRST row that
//! matches in that emitted order (crate::codegen; docs/tmt/isa.md (match and
//! dispatch)). So an earlier SOURCE row shadows a later one it covers
//! only when both land in the same band, where source order equals the emitted
//! (runtime) order:
//!   * two exact rows cannot cover each other (front-end disjointness) — vacuous;
//!   * within the partial band, and within the catch-all band, an earlier row
//!     that covers a later one genuinely shadows it (source order preserved);
//!   * ACROSS bands the shadow is false — a source-earlier catch-all does NOT
//!     shadow a later exact row, because codegen emits the exact row first and
//!     the exact row wins. Deleting the exact row there would change behaviour.
//!
//! A covering `W` always has at least as many wildcards as `R` (its match set is
//! larger), so `W`'s band is never EARLIER than `R`'s; requiring the SAME band
//! is therefore exactly the sound subset — it never deletes a row that would
//! win at runtime, so `-O0` and `-O1` stay observably identical (the equivalence
//! contract, optimizer/mod.rs).
//!
//! # brk and traps
//!
//! A dead row that carries a `debugger` (`brk`) is deleted like any other. This
//! mirrors dce: the brk barrier forbids eliding a REACHABLE pause, but a row an
//! earlier same-band row always shadows can never match, so its `brk` can never
//! fire — deleting it changes nothing observable, exactly as dce deletes an
//! unreachable state that holds a `brk`. Synthesized trap rows are treated
//! uniformly by pattern: the compiler prepends read-hole trap rows first (their
//! one concrete cell is a hole symbol no mapped rule carries at that position),
//! so a trap row never covers a real row in practice; when a trap row IS covered
//! by an earlier same-band row, deleting it just removes an unreachable trap and
//! keeps the trap-on-synthesized-row invariant (nothing is added).
//!
//! # State shape
//!
//! Deleting rows changes no state ids or transition targets (rows carry no ids;
//! transitions name state ids, which are untouched), so no renumber is needed
//! and `validate_world` keeps holding. The first row is never covered by an
//! earlier row, so at least one row always survives. Dropping the last of three
//! rows can flip codegen's straight-line classification or expose the two-row
//! branch shape — that is fine: later passes and codegen see the new shape, and
//! the fixpoint reruns.

use crate::ir::{IrCell, IrWorld};

/// A row's dispatch band, mirroring codegen's classification (crate::codegen
/// `conditional`): all-concrete is `Exact`, all-wildcard is `CatchAll`, a mix is
/// `Partial`. Only within the `Partial` and `CatchAll` bands does source order
/// equal the emitted (runtime) order, so cover-shadowing is sound only there.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Band {
    Exact,
    Partial,
    CatchAll,
}

fn band(pattern: &[IrCell]) -> Band {
    if pattern.iter().all(|c| matches!(c, IrCell::Index { .. })) {
        Band::Exact
    } else if pattern.iter().all(|c| matches!(c, IrCell::Wildcard)) {
        Band::CatchAll
    } else {
        Band::Partial
    }
}

/// Whether `w` covers `r` cell-wise (every input `r` matches, `w` matches too).
fn covers(w: &[IrCell], r: &[IrCell]) -> bool {
    w.iter().zip(r).all(|(wc, rc)| match (wc, rc) {
        (IrCell::Wildcard, _) => true,
        (IrCell::Index { index: a }, IrCell::Index { index: b }) => a == b,
        (IrCell::Index { .. }, IrCell::Wildcard) => false,
    })
}

pub fn run(w: &mut IrWorld) -> u32 {
    let mut deleted = 0u32;
    for st in &mut w.states {
        let n = st.rules.len();
        // Walk top-down with an accumulated cover set: a row is dead iff an
        // earlier KEPT row in the same band covers it. (Cover is transitive
        // within a band, so restricting to kept rows matches checking every
        // earlier row.) The first row is never dead — no earlier row exists.
        let mut keep = vec![true; n];
        for k in 0..n {
            let bk = band(&st.rules[k].pattern);
            let dead = (0..k).any(|j| {
                keep[j]
                    && band(&st.rules[j].pattern) == bk
                    && covers(&st.rules[j].pattern, &st.rules[k].pattern)
            });
            if dead {
                keep[k] = false;
            }
        }
        let before = st.rules.len();
        if keep.iter().any(|kept| !kept) {
            let mut i = 0;
            st.rules.retain(|_| {
                let kept = keep[i];
                i += 1;
                kept
            });
        }
        deleted += (before - st.rules.len()) as u32;
    }
    deleted
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compiler::analyze;
    use crate::expand::expand;
    use crate::ir::{IrProgram, IrRule, IrTransition, IrWrite, lower, validate_world};

    fn ir_of(src: &str) -> IrProgram {
        let a = analyze(src).unwrap_or_else(|e| panic!("analyze: {e}"));
        let ex = expand(&a.resolved).unwrap_or_else(|e| panic!("expand: {e}"));
        lower(&ex, &a.resolved)
            .unwrap_or_else(|e| panic!("lower: {e}"))
            .0
    }

    /// A concrete match cell at `index`.
    fn sym(index: u32) -> IrCell {
        IrCell::Index { index }
    }

    #[test]
    fn a_same_band_partial_shadows_a_later_partial() {
        // arity 3: `[a,*,*]` (row 0, partial) covers `[a,b,*]` (row 1, partial) —
        // same partial band, row 0 earlier → row 1 is dead. `[*,*,*]` (row 2,
        // catch-all) survives. Reduces the state to the two-row branch shape.
        let mut ir = ir_of(
            "alphabet abc { '_', 'a', 'b' }
machine {
  tape x: abc;
  tape y: abc;
  tape z: abc;
  entry state s {
    ['a', *, *]   -> move [>, ., .] goto s;
    ['a', 'b', *] -> move [>, ., .] goto s;
    [*, *, *]     -> stop;
  }
}",
        );
        let m = &mut ir.worlds[0];
        assert_eq!(m.states[0].rules.len(), 3);
        assert_eq!(run(m), 1, "the shadowed partial row is deleted");
        assert_eq!(m.states[0].rules.len(), 2);
        // The survivors are the selective `[a,*,*]` and the catch-all `[*,*,*]`.
        assert_eq!(
            m.states[0].rules[0].pattern,
            vec![sym(1), IrCell::Wildcard, IrCell::Wildcard]
        );
        assert_eq!(
            m.states[0].rules[1].pattern,
            vec![IrCell::Wildcard, IrCell::Wildcard, IrCell::Wildcard]
        );
        validate_world(m).unwrap();
    }

    #[test]
    fn a_cross_band_cover_does_not_delete() {
        // A source-earlier catch-all `[*]` (row 0) "covers" a later exact `['a']`
        // (row 1) cell-wise — but they are in different bands: codegen emits the
        // exact row first, so on 'a' the exact row WINS at runtime. Deleting it
        // would change behaviour, so the band guard must keep it.
        let mut ir = ir_of(
            "alphabet ab { '_', 'a' }
machine {
  tape t: ab;
  entry state s {
    [*]   -> halt;
    ['a'] -> stop;
  }
}",
        );
        let m = &mut ir.worlds[0];
        assert_eq!(run(m), 0, "a cross-band cover is not a runtime shadow");
        assert_eq!(m.states[0].rules.len(), 2);
    }

    #[test]
    fn a_prepended_trap_row_kills_an_identically_shaped_real_row() {
        // A synthesized trap row, prepended first, that covers a later real row
        // in the SAME band: the trap fires first, so the real row is unreachable
        // and its deletion is correct. Built by injecting a synthesized partial
        // `[1,*] -> trap #0` ahead of a real partial `[1,*] -> goto` of the same
        // pattern (the front end never emits this — a trap symbol never appears
        // in a mapped rule — so the arrangement is constructed directly).
        let mut ir = ir_of(
            "alphabet ab { '_', 'a', 'b' }
machine {
  tape x: ab;
  tape y: ab;
  entry state s {
    ['a', *] -> move [>, .] goto s;
    [*, *]   -> stop;
  }
}",
        );
        let m = &mut ir.worlds[0];
        let real = m.states[0].rules[0].clone();
        assert_eq!(real.pattern, vec![sym(1), IrCell::Wildcard]);
        let trap = IrRule {
            pattern: vec![sym(1), IrCell::Wildcard],
            write: None,
            moves: None,
            debugger: false,
            transition: IrTransition::TrapRead,
            synthesized: true,
            line: 0,
        };
        m.states[0].rules.insert(0, trap);
        // Now: [trap [1,*]], [real [1,*] goto], [catch-all [*,*] stop].
        assert_eq!(m.states[0].rules.len(), 3);
        assert_eq!(run(m), 1, "the real row shadowed by the trap is deleted");
        assert_eq!(m.states[0].rules.len(), 2);
        // The trap row survives as the (only) partial row; the goto is gone.
        assert_eq!(m.states[0].rules[0].transition, IrTransition::TrapRead);
        assert!(
            !m.states[0]
                .rules
                .iter()
                .any(|r| matches!(r.transition, IrTransition::Goto { .. })),
            "the shadowed goto row was deleted"
        );
        validate_world(m).unwrap();
    }

    #[test]
    fn union_cover_alone_does_not_delete() {
        // The single-cover-only near-miss: `[0]` and `[1]` JOINTLY cover the
        // catch-all `[*]` (the alphabet is exactly {0,1}), but no SINGLE row
        // does, so the catch-all survives — union cover is not computed. (They
        // are also in different bands; either way it must not be deleted.)
        let mut ir = ir_of(
            "alphabet ab { '_', 'a' }
machine {
  tape t: ab;
  entry state s {
    ['_'] -> stop;
    ['a'] -> halt;
    [*]   -> halt;
  }
}",
        );
        let m = &mut ir.worlds[0];
        assert_eq!(run(m), 0, "no single row covers the catch-all");
        assert_eq!(m.states[0].rules.len(), 3);
    }

    #[test]
    fn a_dead_row_carrying_a_debugger_is_deleted() {
        // A row an earlier same-band row shadows can never match, so its `brk`
        // can never fire — it is deleted like any dead row (the dce precedent).
        // Built by injecting a second catch-all carrying a `debugger` after the
        // first (two all-wildcard rows are the catch-all band; the front end
        // would only warn, but codegen would not assemble two of them — so the
        // pass is tested on the IR directly).
        let mut ir = ir_of(
            "alphabet ab { '_', 'a' }
machine {
  tape t: ab;
  entry state s {
    ['a'] -> write ['_'] move [>] goto s;
    [*]   -> stop;
  }
}",
        );
        let m = &mut ir.worlds[0];
        let brk_dup = IrRule {
            pattern: vec![IrCell::Wildcard],
            write: Some(vec![IrWrite::Keep]),
            moves: None,
            debugger: true,
            transition: IrTransition::Halt,
            synthesized: false,
            line: 0,
        };
        m.states[0].rules.push(brk_dup);
        // Now: [`['a']`], [`[*]` stop], [`[*]` debugger halt]. The last catch-all
        // is shadowed by the first catch-all (same band).
        assert!(
            m.states[0].rules.iter().any(|r| r.debugger),
            "the brk row is present before dead_rows"
        );
        assert_eq!(run(m), 1, "the shadowed debugger row is deleted");
        assert!(
            !m.states[0].rules.iter().any(|r| r.debugger),
            "the unreachable brk row was deleted"
        );
        validate_world(m).unwrap();
    }

    #[test]
    fn a_state_with_no_shadowing_is_untouched() {
        // Three disjoint exact rows: no row covers another, nothing is deleted.
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
        assert_eq!(run(&mut ir.worlds[0]), 0);
    }

    #[test]
    fn then_goto_targets_are_untouched_by_row_deletion() {
        // A defensive check that deleting a row leaves `call … then goto` resume
        // ids and the world entry intact (rows carry no ids; only states do).
        let mut ir = ir_of(
            "alphabet ab { '_', 'a', 'b' }
machine {
  tape x: ab;
  tape y: ab;
  tape z: ab;
  entry state s {
    ['a', *, *]   -> move [>, ., .] goto done;
    ['a', 'b', *] -> move [>, ., .] goto done;
    [*, *, *]     -> stop;
  }
  state done { [*, *, *] -> stop; }
}",
        );
        let m = &mut ir.worlds[0];
        let entry_before = m.entry;
        assert_eq!(run(m), 1);
        assert_eq!(m.entry, entry_before, "the entry id is unchanged");
        let s = &m.states[m.entry as usize];
        // The surviving selective row still gotos `done` (its id preserved).
        let done_id = m.states.iter().find(|st| st.name == "done").unwrap().id;
        assert_eq!(s.rules[0].transition, IrTransition::Goto { state: done_id });
        validate_world(m).unwrap();
    }
}
