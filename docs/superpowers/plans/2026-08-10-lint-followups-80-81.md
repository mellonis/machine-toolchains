# Lint Follow-ups: Range Cross-Guard + Comment Unreachability Pin Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close the two follow-up issues from the declared-contracts final review: a drift guard pinning the lint's range attribution to the compiler's range expansion (issue: range_labels↔expand_range cross-guard), and a pin proving `duplicate-map-source`'s quickfix can never meet a comment (issue: quickfix deletes interior comments — investigated and found UNREACHABLE; the deliverable is the proof, not a guard).

**Architecture:** Both tasks are test-only (plus two `pub(crate)` visibility widenings and one module-doc section). Task 1 adds a unit-level lockstep invariant `range_labels(lo,hi) == expand_range(lo,hi,span).ok()` (matrix + proptest) in `lint/patterns.rs`, comparing the lint's enumeration directly against the compiler's expansion. Task 2 documents and test-pins the three structural reasons a comment can never sit inside a `duplicate-map-source` deletion span (verified empirically: both interior-comment shapes are assemble fatals — `bad-frame` — before the lint runs; a trailing comment after `)` is outside every deletion span).

**Tech Stack:** Rust, proptest (already a dev-dep of `mtc-turing-machine`), cargo test.

## Global Constraints

- Published content (code comments, docs) is forge-agnostic: NO issue/PR numbers, no "Task N", no `spec §N`, no `docs/superpowers/` paths in code comments. Cite durable pages as `docs/<page>.md (keyword)`.
- No Claude/AI attribution anywhere (commits, comments, docs).
- Conventional commits with scope (`test(turing-machine): …`).
- If `git commit` fails on GPG signing, commit with `git -c commit.gpgsign=false commit …` — the controller re-signs before merge. NEVER `--amend` an existing commit; fixes are new commits.
- Quality gates before reporting DONE: `cargo test -p mtc-turing-machine`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo fmt --check`, and `cargo +1.97.0 clippy --workspace --all-targets -- -D warnings` (the CI runs a newer stable than the local default; the side toolchain is installed).
- `-O0` bit-identity, PM-1 byte-identity: untouched by construction (test-only changes; the two visibility widenings change no behavior).

---

### Task 1: Cross-guard the lint's range attribution against the compiler's expansion

The overlap lint (and any future clause-aware rule) attributes range-element findings via `lint/patterns.rs::range_labels`, while the resolved sets it reasons over come from the compiler's `expand_range`. The two agree today; nothing pins them. A future divergence would surface as a mis-attributed finding span. This task pins them with an exact lockstep invariant.

**Files:**
- Modify: `crates/turing-machine/src/compiler.rs` — `fn glyph_label` and `fn expand_range` (adjacent, around lines 758–809) become `pub(crate) fn`. No other change. Both are already called from within `compiler.rs`, so no dead-code fallout.
- Modify: `crates/turing-machine/src/lint/patterns.rs` — append a `#[cfg(test)] mod tests` at the end of the file (the file currently has none).

**Interfaces:**
- Consumes: `crate::compiler::{expand_range, glyph_label}` (after the visibility widening); `crate::parser::SymLit` (variants: `Glyph { value: String, span: Span }`, `Number { value: u32, written: String, span: Span }`); `mtc_core::diagnostics::Span` (`Span::new(l1, c1, l2, c2)`).
- Produces: nothing new for later tasks — Task 2 is independent.

- [ ] **Step 1: Widen visibility in `compiler.rs`**

Change `fn glyph_label(` → `pub(crate) fn glyph_label(` and `fn expand_range(` → `pub(crate) fn expand_range(`. Do not touch their bodies or docs.

- [ ] **Step 2: Add the test module to `lint/patterns.rs`**

Append at the end of the file:

```rust
#[cfg(test)]
mod tests {
    use mtc_core::diagnostics::Span;
    use proptest::prelude::*;

    use crate::compiler::{expand_range, glyph_label as compiler_glyph_label};
    use crate::parser::SymLit;

    use super::{glyph_label, range_labels};

    fn num(value: u32) -> SymLit {
        SymLit::Number {
            value,
            written: value.to_string(),
            span: Span::new(1, 1, 1, 2),
        }
    }

    fn glyph(value: &str) -> SymLit {
        SymLit::Glyph {
            value: value.to_string(),
            span: Span::new(1, 1, 1, 2),
        }
    }

    /// The one invariant: the lint's enumeration answers exactly when the
    /// compiler's expansion succeeds, and with the same labels. A future
    /// divergence in range handling would mis-attribute finding spans while
    /// the overlap decision (computed from the resolved sets alone) stayed
    /// right — this is the pin that surfaces it as a test failure instead.
    fn assert_in_lockstep(lo: &SymLit, hi: &SymLit) {
        let span = Span::new(1, 1, 1, 2);
        assert_eq!(
            range_labels(lo, hi),
            expand_range(lo, hi, span).ok(),
            "lo={lo:?} hi={hi:?}"
        );
    }

    #[test]
    fn the_matrix_pins_lint_attribution_to_the_compilers_expansion() {
        // Numeric: ascending, single-value, descending.
        assert_in_lockstep(&num(3), &num(7));
        assert_in_lockstep(&num(5), &num(5));
        assert_in_lockstep(&num(7), &num(3));
        // Glyph: ascending, single-value, descending.
        assert_in_lockstep(&glyph("a"), &glyph("f"));
        assert_in_lockstep(&glyph("a"), &glyph("a"));
        assert_in_lockstep(&glyph("f"), &glyph("a"));
        // The surrogate gap: both walkers must skip it identically.
        assert_in_lockstep(&glyph("\u{D7F0}"), &glyph("\u{E010}"));
        // Endpoints resolution rejects: multi-scalar, empty, mixed kinds.
        assert_in_lockstep(&glyph("ab"), &glyph("c"));
        assert_in_lockstep(&glyph("a"), &glyph(""));
        assert_in_lockstep(&num(1), &glyph("a"));
        assert_in_lockstep(&glyph("a"), &num(1));
    }

    /// Derivation-first check of the gap-crossing range itself, so the
    /// lockstep assert above cannot be satisfied by two walkers sharing the
    /// same wrong answer: 0xD7F0..=0xE010 spans 0x821 code points, 0x800 of
    /// them the surrogate gap, leaving 16 labels below it and 17 at or
    /// above 0xE000 — 33 in all, none of them a surrogate.
    #[test]
    fn the_gap_crossing_range_is_derived_not_observed() {
        let labels = range_labels(&glyph("\u{D7F0}"), &glyph("\u{E010}")).unwrap();
        assert_eq!(labels.len(), 33);
        assert_eq!(labels.first().unwrap(), "\u{D7F0}");
        assert_eq!(labels.last().unwrap(), "\u{E010}");
        assert!(
            labels
                .iter()
                .all(|l| l.chars().all(|c| !(0xD800..=0xDFFF).contains(&(c as u32))))
        );
    }

    /// The single-literal leg of the same drift family: both sides label a
    /// numeric literal by its VALUE's decimal string (the `05` ≡ `5` rule,
    /// docs/tmt/language.md (alphabets)).
    #[test]
    fn glyph_label_matches_the_compilers() {
        for lit in [glyph("a"), glyph("ab"), glyph(""), num(0), num(5)] {
            assert_eq!(glyph_label(&lit), compiler_glyph_label(&lit), "{lit:?}");
        }
        let five = SymLit::Number {
            value: 5,
            written: "05".to_string(),
            span: Span::new(1, 1, 1, 3),
        };
        assert_eq!(glyph_label(&five), "5");
        assert_eq!(compiler_glyph_label(&five), "5");
    }

    proptest! {
        /// Arbitrary numeric endpoints (span-bounded so an expansion stays
        /// small) stay in lockstep — ascending and descending alike.
        #[test]
        fn numeric_endpoints_stay_in_lockstep(lo in 0u32..=1000, delta in -60i64..=60) {
            let hi = (i64::from(lo) + delta).clamp(0, i64::from(u32::MAX)) as u32;
            assert_in_lockstep(&num(lo), &num(hi));
        }

        /// Arbitrary glyph endpoints stay in lockstep. The delta bound
        /// (±3000, wider than the 2048-wide surrogate gap) lets generated
        /// pairs straddle the gap; a target landing IN the gap is not a
        /// valid endpoint and is discarded.
        #[test]
        fn glyph_endpoints_stay_in_lockstep(lo in any::<char>(), delta in -3000i64..=3000) {
            let target = (i64::from(lo as u32) + delta).clamp(0, 0x10FFFF) as u32;
            prop_assume!(char::from_u32(target).is_some());
            let hi = char::from_u32(target).unwrap();
            assert_in_lockstep(&glyph(&lo.to_string()), &glyph(&hi.to_string()));
        }
    }
}
```

- [ ] **Step 3: Run the module's tests**

Run: `cargo test -p mtc-turing-machine --lib patterns::`
Expected: all new tests PASS (5 test functions).

- [ ] **Step 4: Run the quality gates**

Run: `cargo test -p mtc-turing-machine && cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --check && cargo +1.97.0 clippy --workspace --all-targets -- -D warnings`
Expected: all green.

- [ ] **Step 5: Commit**

```bash
git add crates/turing-machine/src/compiler.rs crates/turing-machine/src/lint/patterns.rs
git commit -m "test(turing-machine): cross-guard the lint's range attribution against the compiler's expansion"
```

(If GPG signing fails, use `git -c commit.gpgsign=false commit -m …`.)

---

### Task 2: Pin why `duplicate-map-source`'s fix can never meet a comment

The follow-up issue hypothesized the rule's quickfix could silently delete an interior comment. Investigation disproved it: the deletion span lies strictly inside the clause's `(..)` group, and the interior of a well-formed group can never hold a comment — a `;` before the `)` comments the closer out (malformed directive), a trailing comma followed by a comment does not continue the list, and an own-line comment between continuation lines breaks the fold. Every such shape is an assemble fatal (`bad-frame`, verified empirically), and the lint runs behind the fatal gate on BOTH entry routes (CLI `lint_tma` and the language service's `lint_tma_cst` — the `?` on core's `lint_cst` gates the TM additions too). A comment after the `)` is the one comment a duplicate-carrying `.map` can hold, and it sits outside every deletion span. This task writes that reasoning into the module doc and pins each shape with a test, so a future loosening of the continuation rules fails loudly and forces re-evaluation.

**Files:**
- Modify: `crates/turing-machine/src/lint/tma/rules/duplicate_map_source.rs` — one new module-doc section + three new tests inside the existing `#[cfg(test)] mod tests`.

**Interfaces:**
- Consumes: the file's existing test helpers `program(map_tail: &str) -> String`, `diagnostics(src: &str) -> Vec<Diagnostic>`, `assert_fix_no_op(dup_tail: &str, winner_tail: &str)`, and `crate::lint::tma::lint_tma`.
- Produces: nothing — terminal task.

- [ ] **Step 1: Add the module-doc section**

Insert AFTER the existing `# What it sees, and the fix` section's final paragraph (the one ending "…every pair here is already well-formed.") a new section:

```rust
//! # Why the fix never meets a comment
//!
//! The fix's deletion span lies strictly inside the clause's `(..)` group,
//! and the interior of a well-formed group can never hold a comment: a `;`
//! before the `)` comments the closer out (the directive is malformed), a
//! trailing comma followed by a comment does not continue the list, and an
//! own-line comment between continuation lines breaks the fold
//! (docs/formats.md (assembly text)). Every such shape is an assemble
//! fatal, and the fatal gate runs before this rule on both routes — so the
//! deletion can never swallow a comment, and no comment-withholding guard
//! is needed here. A comment after the `)` is the one comment a
//! duplicate-carrying `.map` can hold, and it sits outside every deletion
//! span. The tests pin each shape; if the continuation rules ever loosen,
//! they fail and this reasoning must be revisited.
```

- [ ] **Step 2: Write the three pinning tests (expect the first two to pass immediately — they pin existing behavior)**

Add inside the existing `mod tests`, after `the_fix_removes_a_shadowed_mapping_across_the_line_break`:

```rust
    /// A trailing comma followed by a comment does not continue the list
    /// (docs/formats.md (assembly text)) — the directive is malformed and
    /// the assemble fatal gate rejects it before this rule can run. If
    /// this ever starts assembling, a comment can reach a deletion span
    /// and the fix must learn to withhold itself (module head).
    #[test]
    fn a_comma_followed_by_a_comment_is_an_assemble_fatal() {
        let src = program("rmap=(1->2, ; note\n            1->3)");
        assert!(lint_tma(&src, &[]).is_err());
    }

    /// An own-line comment between continuation lines breaks the fold —
    /// same fatal gate, same consequence: the rule never sees the group.
    #[test]
    fn an_own_line_comment_inside_the_group_is_an_assemble_fatal() {
        let src = program("rmap=(1->2,\n    ; note\n            1->3)");
        assert!(lint_tma(&src, &[]).is_err());
    }

    /// A comment after the `)` is the one comment a duplicate-carrying
    /// `.map` can hold, and it sits outside every deletion span: the fix
    /// leaves it byte-for-byte in place.
    #[test]
    fn a_trailing_comment_after_the_group_survives_the_fix() {
        assert_fix_no_op("rmap=(1->2, 1->3) ; note", "rmap=(1->3) ; note");
    }
```

- [ ] **Step 3: Run the rule's tests**

Run: `cargo test -p mtc-turing-machine --lib duplicate_map_source::`
Expected: all PASS (12 tests: 9 existing + 3 new).

- [ ] **Step 4: Run the quality gates**

Run: `cargo test -p mtc-turing-machine && cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --check && cargo +1.97.0 clippy --workspace --all-targets -- -D warnings`
Expected: all green.

- [ ] **Step 5: Commit**

```bash
git add crates/turing-machine/src/lint/tma/rules/duplicate_map_source.rs
git commit -m "test(turing-machine): pin why duplicate-map-source's fix can never meet a comment"
```

(If GPG signing fails, use `git -c commit.gpgsign=false commit -m …`.)
