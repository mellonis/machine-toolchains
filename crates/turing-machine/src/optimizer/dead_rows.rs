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
//!
//! # Identical-effect subsumption
//!
//! Same-band cover (above) proves a row can NEVER fire — dead code. A second,
//! independent check proves a row's EFFECT is redundant even where it DOES
//! fire: row `R` is deleted when a later-emitted row `W` covers it (the same
//! cell-wise relation, now compared across the FULL emitted order — codegen's
//! order (crate::codegen `emitted_row_order`), not source order, and bands no
//! longer matter, because the claim isn't "R never wins", it's "R's win is
//! indistinguishable from W's win") and:
//!
//!   1. **normalized effect equality** — a `None` write/`moves` normalizes to
//!      all-`Keep`/all-`Stay` first, then each tape compares with a `Keep ≡
//!      Index(a)` rule: on a tape where `R` matches the CONCRETE symbol `a`,
//!      writing `a` back and keeping are the same net effect (the cell can
//!      only hold `a` at the moment `R` fires there). The rule does NOT apply
//!      on a tape where `R` itself is a wildcard — there Keep and `write a`
//!      differ across the inputs `R` matches, so only literal write equality
//!      counts. Transitions compare by `==`; the `direct` flag is a codegen
//!      lowering hint (which label a dispatch entry names), never part of the
//!      effect;
//!   2. **no debugger, no synthesized** on either row — a `brk` or a
//!      compiler-synthesized trap row is never folded into another row's
//!      identity;
//!   3. **no intermediate capture** — every row strictly between `R` and `W`
//!      in EMITTED order is pattern-disjoint from `R` (some tape where both
//!      are concrete and differ). If an intermediate row could match one of
//!      `R`'s inputs, deleting `R` would hand that input to the intermediate,
//!      not to `W` — soundness needs the row that actually serves `R`'s
//!      inputs post-deletion to be `W` itself.
//!
//! Cover and effect equality both compose transitively along a subsumption
//! chain (`R` folded into `W`, `W` in turn folded into `W'` in the same pass
//! run): if `W` covers `R`, cover forces `R` concrete with the same index
//! everywhere `W` is concrete, so a row disjoint from `W` is also disjoint
//! from `R`, and effect equality chains the same way through the shared
//! reference index. Deleting a whole chain in one call is therefore sound —
//! the pass does not special-case it, the proof just holds. This runs AFTER
//! same-band cover has already removed every provably-unreachable row, so
//! only rows the match engine can actually select are ever considered.
//!
//! **The volatile barrier** (docs/tmt/language.md (volatile tapes),
//! optimizer/mod.rs (volatile barrier)) narrows rule 1 on a volatile tape:
//! the `Keep ≡ Index(a)` fold is withheld there, and only LITERAL write
//! equality counts. `Keep` and `write a` share a net cell VALUE when `R`
//! matches `a`, but codegen emits no `wrmv` write micro-op at all for an
//! all-`Keep` action while `Index(a)` always does (crate::codegen — "an
//! all-keep write with an all-stay move emits nothing") — on a device band
//! the write OPERATION, not just the value it leaves behind, is externally
//! observable, so folding them would drop or add a bus transaction the
//! external world can see. Literal equality (both `Keep`, or both the same
//! `Index`) keeps the emitted instruction identical either way, so deleting
//! `R` changes nothing about the band's access sequence even there.
//!
//! **The `dispatch_select` interaction.** A [`crate::ir::IrDispatch::Branch`]-
//! flagged state is codegen's committed two-row `jm` shape (`branch()`
//! indexes `st.rules[0]`/`st.rules[1]` unconditionally); same-band cover can
//! never reach one (the selective and catch-all rows are always in different
//! bands), but this cross-band check could — a state whose selective row
//! happens to become effect-identical to its catch-all only AFTER
//! `dispatch_select` already flagged it (a later fixpoint round, e.g.
//! `jump_threading` retargeting one of the two `Goto`s onto the other) would
//! otherwise lose a row while still carrying the `Branch` flag, corrupting
//! that invariant. Subsumption is skipped entirely on a non-`Table` state; the
//! `Table`-flagged case this pass exists for cannot be reached that way
//! anyway, since `dead_rows` always runs, in the SAME round, before
//! `dispatch_select` ever sees the state.

use crate::codegen::emitted_row_order;
use crate::ir::{IrCell, IrDispatch, IrMove, IrRule, IrState, IrWorld, IrWrite};

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

/// Whether `rw` (row `r`'s write cell on some tape) and `ww` (the comparison
/// row's write cell on the same tape) have the SAME net effect there, given
/// `r`'s own match cell on that tape and whether the tape is `volatile`.
/// Literal equality always counts. When the tape is NOT volatile and `r`'s
/// match cell is the CONCRETE symbol `a`, `Keep` also counts equal to
/// `Index(a)` — writing `a` back is indistinguishable from keeping when the
/// cell can only hold `a` at the moment this write fires. On a volatile tape
/// that fold is withheld (docs/tmt/optimizer.md (volatile barrier)): only
/// literal equality counts, because `Keep` and `Index(a)` emit different
/// instructions (no `wrmv` write vs. one), and on a device band the write
/// OPERATION is externally observable even when the value it leaves behind
/// would be the same.
fn wr_eq(rw: IrWrite, ww: IrWrite, r_cell: &IrCell, volatile: bool) -> bool {
    if rw == ww {
        return true;
    }
    if volatile {
        return false;
    }
    let IrCell::Index { index: a } = *r_cell else {
        return false;
    };
    matches!(
        (rw, ww),
        (IrWrite::Keep, IrWrite::Index { index }) | (IrWrite::Index { index }, IrWrite::Keep)
            if index == a
    )
}

/// Whether rows `r` and `w` have the same effect: an identical transition
/// (`==`; `direct` is a lowering hint, never part of the effect), and per
/// tape, the same write (normalized `None` to all-`Keep`, compared via
/// [`wr_eq`] with that tape's `volatile[k]`) and the same move (normalized
/// `None` to all-`Stay`, compared literally — a move has no keep-like
/// shorthand to fold, volatile or not).
fn effect_eq(r: &IrRule, w: &IrRule, volatile: &[bool]) -> bool {
    if r.transition != w.transition {
        return false;
    }
    (0..volatile.len()).all(|k| {
        let rw = r.write.as_ref().map_or(IrWrite::Keep, |v| v[k]);
        let ww = w.write.as_ref().map_or(IrWrite::Keep, |v| v[k]);
        let rm = r.moves.as_ref().map_or(IrMove::Stay, |v| v[k]);
        let wm = w.moves.as_ref().map_or(IrMove::Stay, |v| v[k]);
        wr_eq(rw, ww, &r.pattern[k], volatile[k]) && rm == wm
    })
}

/// Whether `a` and `b` are pattern-disjoint: some tape where both are
/// concrete and differ, so no input can match both rows.
fn pattern_disjoint(a: &[IrCell], b: &[IrCell]) -> bool {
    a.iter()
        .zip(b)
        .any(|(x, y)| matches!((x, y), (IrCell::Index { index: p }, IrCell::Index { index: q }) if p != q))
}

/// Identical-effect subsumption (docs/tmt/optimizer.md (row subsumption)):
/// delete emitted-earlier row `R` when a later row `W` covers it with
/// identical effect and no row between them, in emitted order, can capture
/// `R`'s inputs. Walks [`emitted_row_order`] — the SAME order codegen's
/// match/dispatch table uses — not source order. `volatile[k]` gates the
/// write-equality fold on tape `k` (see the module doc's volatile-barrier
/// section). Skipped entirely on a `Branch`-flagged state (the module doc's
/// `dispatch_select` interaction section) — codegen's two-row `jm` lowering
/// there is not proof against losing a row.
fn subsume_identical_effect(st: &mut IrState, volatile: &[bool]) -> u32 {
    if st.dispatch != IrDispatch::Table {
        return 0;
    }
    let order = emitted_row_order(st);
    // `dead[k]` is written only for the CURRENT outer position; every
    // candidate `j` searched below sits at a later position and so is always
    // still `false` here — a `W` this call ALSO ends up deleting (folded
    // itself into a still-later `W'`) is used freely, not guarded against.
    // That is deliberate, not an oversight: the module doc's chain-soundness
    // paragraph is exactly the argument for why using such a `W` is sound.
    let mut dead = vec![false; st.rules.len()];
    for (pos_i, &i) in order.iter().enumerate() {
        if st.rules[i].debugger || st.rules[i].synthesized {
            continue;
        }
        let subsumed = ((pos_i + 1)..order.len()).any(|pos_j| {
            let j = order[pos_j];
            if st.rules[j].debugger || st.rules[j].synthesized {
                return false;
            }
            if !covers(&st.rules[j].pattern, &st.rules[i].pattern) {
                return false;
            }
            if !effect_eq(&st.rules[i], &st.rules[j], volatile) {
                return false;
            }
            // No row strictly between R and this W may capture R's inputs —
            // otherwise deleting R would hand those inputs to the
            // intermediate row, not to W.
            !order[(pos_i + 1)..pos_j]
                .iter()
                .any(|&m| !pattern_disjoint(&st.rules[m].pattern, &st.rules[i].pattern))
        });
        if subsumed {
            dead[i] = true;
        }
    }
    let before = st.rules.len();
    if dead.iter().any(|&d| d) {
        let mut k = 0;
        st.rules.retain(|_| {
            let kept = !dead[k];
            k += 1;
            kept
        });
    }
    (before - st.rules.len()) as u32
}

pub fn run(w: &mut IrWorld) -> u32 {
    let mut deleted = 0u32;
    // Hoisted out of the `&mut w.states` loop below (borrow-checker
    // friendly) — `w.tapes` itself is never mutated by this pass.
    let volatile: Vec<bool> = w.tapes.iter().map(|t| t.volatile).collect();
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

        // Identical-effect subsumption (docs/tmt/optimizer.md (row
        // subsumption)): delete emitted-earlier row R when a later row W
        // covers it with identical effect and no row between them can
        // capture R's inputs.
        deleted += subsume_identical_effect(st, &volatile);
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
            direct: false,
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
        // `['a']` writes `'_'` — a DIFFERENT effect than the catch-all's keep —
        // so identical-effect subsumption (which would otherwise legitimately
        // fold `['a']` into an effect-identical catch-all) stays out of this
        // test's way; it has its own tests above.
        let mut ir = ir_of(
            "alphabet ab { '_', 'a' }
machine {
  tape t: ab;
  entry state s {
    ['_'] -> stop;
    ['a'] -> write ['_'] halt;
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
            direct: false,
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

    #[test]
    fn a_specific_row_with_identical_effect_is_subsumed_by_the_catch_all() {
        // ['0'] -> write ['1'] goto t;  [*] -> write ['1'] goto t;
        // The exact row dies; the catch-all serves its inputs identically.
        let mut ir = ir_of(
            "alphabet ab { '_', '0', '1' }
machine {
  tape n: ab;
  entry state s {
    ['0'] -> write ['1'] goto t;
    [*]   -> write ['1'] goto t;
  }
  state t { [*] -> stop; }
}",
        );
        let m = &mut ir.worlds[0];
        assert_eq!(m.states[0].rules.len(), 2);
        assert_eq!(
            run(m),
            1,
            "the exact row is subsumed by the identical-effect catch-all"
        );
        assert_eq!(m.states[0].rules.len(), 1);
        assert_eq!(
            m.states[0].rules[0].pattern,
            vec![IrCell::Wildcard],
            "the surviving row is the catch-all"
        );
        validate_world(m).unwrap();
    }

    #[test]
    fn keep_vs_writing_the_matched_symbol_back_counts_as_identical() {
        // ['0'] -> write ['0'] goto t;  [*] -> goto t;   (write None = keep)
        // R writes its own matched symbol; W keeps. Identical on R's inputs.
        let mut ir = ir_of(
            "alphabet ab { '_', '0' }
machine {
  tape n: ab;
  entry state s {
    ['0'] -> write ['0'] goto t;
    [*]   -> goto t;
  }
  state t { [*] -> stop; }
}",
        );
        let m = &mut ir.worlds[0];
        assert_eq!(
            run(m),
            1,
            "writing the matched symbol back counts as identical to keep"
        );
        assert_eq!(m.states[0].rules.len(), 1);
        validate_world(m).unwrap();
    }

    #[test]
    fn differing_effect_survives() {
        // ['0'] -> write ['1'] goto t;  [*] -> goto t;   — R stays.
        let mut ir = ir_of(
            "alphabet ab { '_', '0', '1' }
machine {
  tape n: ab;
  entry state s {
    ['0'] -> write ['1'] goto t;
    [*]   -> goto t;
  }
  state t { [*] -> stop; }
}",
        );
        let m = &mut ir.worlds[0];
        assert_eq!(run(m), 0, "a differing write effect blocks subsumption");
        assert_eq!(m.states[0].rules.len(), 2);
    }

    #[test]
    fn an_intermediate_overlapping_row_blocks_subsumption() {
        // Emitted order R(exact), M(partial overlapping R), W(catch-all), all
        // three targeting `t`: R and W share an identical write effect, but M
        // sits between them with a DIFFERENT write effect and overlaps R's
        // input (matches `['0', *]`, so it captures R's `['0', '0']`).
        // Deleting R would hand its input to M — not W — so R must stay.
        let mut ir = ir_of(
            "alphabet ab { '_', '0', '1', '2' }
machine {
  tape x: ab;
  tape y: ab;
  entry state s {
    ['0', '0'] -> write ['1', '1'] goto t;
    ['0', *]   -> write ['2', '2'] goto t;
    [*, *]     -> write ['1', '1'] goto t;
  }
  state t { [*, *] -> stop; }
}",
        );
        let m = &mut ir.worlds[0];
        assert_eq!(m.states[0].rules.len(), 3);
        assert_eq!(
            run(m),
            0,
            "the overlapping intermediate row blocks the exact row's subsumption"
        );
        assert_eq!(m.states[0].rules.len(), 3);
    }

    #[test]
    fn debugger_on_either_row_blocks_subsumption() {
        // R carries `debugger`: its brk must fire, so it is never folded into
        // another row's identity even with an identical effect available later.
        let mut ir_r = ir_of(
            "alphabet ab { '_', '0' }
machine {
  tape n: ab;
  entry state s {
    ['0'] -> debugger goto t;
    [*]   -> goto t;
  }
  state t { [*] -> stop; }
}",
        );
        let m_r = &mut ir_r.worlds[0];
        assert_eq!(run(m_r), 0, "R's debugger blocks subsumption");
        assert_eq!(m_r.states[0].rules.len(), 2);

        // W carries `debugger`: a later row's pause is likewise never used as
        // an earlier row's silent stand-in.
        let mut ir_w = ir_of(
            "alphabet ab { '_', '0' }
machine {
  tape n: ab;
  entry state s {
    ['0'] -> goto t;
    [*]   -> debugger goto t;
  }
  state t { [*] -> stop; }
}",
        );
        let m_w = &mut ir_w.worlds[0];
        assert_eq!(run(m_w), 0, "W's debugger blocks subsumption");
        assert_eq!(m_w.states[0].rules.len(), 2);
    }

    #[test]
    fn dead_rows_same_band_cover_still_works() {
        // The pre-existing same-band-cover behavior, pinned unchanged now
        // that identical-effect subsumption runs in the same pass: `[a,*,*]`
        // covers `[a,b,*]` (same partial band) and deletes it; the surviving
        // selective row and the catch-all `stop` have different transitions,
        // so the NEW subsumption logic does not fold them together too.
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
        assert_eq!(run(m), 1, "only the shadowed partial row is deleted");
        assert_eq!(m.states[0].rules.len(), 2);
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
    fn volatile_tape_blocks_the_keep_equals_matched_write_fold() {
        // Same shape as `keep_vs_writing_the_matched_symbol_back_counts_as_identical`,
        // but on a `volatile tape`: `Keep` and `write ['0']` emit different
        // instructions (no `wrmv` write vs. one), and on a device band the
        // write OPERATION is observable even when it would leave the same
        // value behind — so the fold is withheld and R survives.
        let mut ir = ir_of(
            "alphabet ab { '_', '0' }
machine {
  volatile tape n: ab;
  entry state s {
    ['0'] -> write ['0'] goto t;
    [*]   -> goto t;
  }
  state t { [*] -> stop; }
}",
        );
        let m = &mut ir.worlds[0];
        assert_eq!(
            run(m),
            0,
            "a volatile tape withholds the Keep-equals-matched-write fold"
        );
        assert_eq!(m.states[0].rules.len(), 2);
    }

    #[test]
    fn a_branch_flagged_state_is_left_untouched_even_with_identical_effect() {
        // `dispatch_select`'s committed two-row `jm` shape (Branch): codegen
        // indexes `st.rules[0]`/`st.rules[1]` unconditionally, so subsumption
        // must never touch it even when the two rows are effect-identical
        // (constructed directly — no `.tmc` source authors the flag; this
        // exact shape WOULD be subsumed under the default `Table` dispatch,
        // per `a_specific_row_with_identical_effect_is_subsumed_by_the_catch_all`'s
        // sibling case).
        let mut ir = ir_of(
            "alphabet ab { '_', '0' }
machine {
  tape n: ab;
  entry state s {
    ['0'] -> goto t;
    [*]   -> goto t;
  }
  state t { [*] -> stop; }
}",
        );
        let m = &mut ir.worlds[0];
        m.states[0].dispatch = IrDispatch::Branch;
        assert_eq!(
            run(m),
            0,
            "a Branch-flagged state is never touched by subsumption"
        );
        assert_eq!(m.states[0].rules.len(), 2);
        assert_eq!(m.states[0].dispatch, IrDispatch::Branch);
    }
}
