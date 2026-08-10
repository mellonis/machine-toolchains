//! Shared pattern-cell helpers for the coverage-based rules (`dead-rule`,
//! `binding-product-threshold`, `state-may-trap`): the glyph labels a pattern
//! cell matches over its tape's alphabet, and a rule's dispatch band. All
//! source-level over the resolved worlds — no expansion is run.

use crate::parser::{PatternCell, PatternCellKind, SymLit};

/// The glyph label a symbol literal denotes. A numeric literal's identity is
/// its value's decimal string (`05` and `5` both label `"5"`), matching the
/// alphabet-resolution rule (docs/tmt/language.md (alphabets)).
pub(crate) fn glyph_label(s: &SymLit) -> String {
    match s {
        SymLit::Glyph { value, .. } => value.clone(),
        SymLit::Number { value, .. } => value.to_string(),
    }
}

fn single_scalar(g: &str) -> Option<char> {
    let mut chars = g.chars();
    let first = chars.next()?;
    chars.next().is_none().then_some(first)
}

/// Enumerate a pattern range's glyph labels (inclusive, ascending). `None` when
/// the endpoints are descending, mixed-kind, or a non-single-scalar glyph — the
/// cases resolution would reject or a lint cannot prove over. Mirrors the
/// alphabet range expansion so the two agree on a range's membership.
pub(crate) fn range_labels(lo: &SymLit, hi: &SymLit) -> Option<Vec<String>> {
    match (lo, hi) {
        (SymLit::Number { value: l, .. }, SymLit::Number { value: h, .. }) => {
            (l <= h).then(|| (*l..=*h).map(|v| v.to_string()).collect())
        }
        (SymLit::Glyph { value: l, .. }, SymLit::Glyph { value: h, .. }) => {
            let (lc, hc) = (single_scalar(l)?, single_scalar(h)?);
            (lc as u32 <= hc as u32).then(|| {
                (lc as u32..=hc as u32)
                    .filter_map(char::from_u32)
                    .map(|c| c.to_string())
                    .collect()
            })
        }
        _ => None,
    }
}

/// The glyph labels a pattern cell matches over `tape_glyphs` (its tape's
/// alphabet, position order): a wildcard matches the whole alphabet, a single
/// its one label, a range its enumerated labels. `None` when a range is
/// unresolvable — the caller then declines to reason about the cell.
pub(crate) fn cell_labels(cell: &PatternCell, tape_glyphs: &[String]) -> Option<Vec<String>> {
    match &cell.kind {
        PatternCellKind::Wildcard => Some(tape_glyphs.to_vec()),
        PatternCellKind::Single(s) => Some(vec![glyph_label(s)]),
        PatternCellKind::Range { lo, hi } => range_labels(lo, hi),
    }
}

/// A rule's dispatch band, mirroring codegen's row classification (crate::
/// codegen; docs/tmt/isa.md (match and dispatch)): all-wildcard is
/// `CatchAll`, wildcard-free is `Exact`, a mix is `Partial`. Source order
/// equals emitted (runtime) order only WITHIN the `Partial` and `CatchAll`
/// bands, so order-aware shadow reasoning is sound only there.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Band {
    Exact,
    Partial,
    CatchAll,
}

pub(crate) fn band(cells: &[PatternCell]) -> Band {
    let wild = |c: &PatternCell| matches!(c.kind, PatternCellKind::Wildcard);
    if cells.iter().all(wild) {
        Band::CatchAll
    } else if cells.iter().any(wild) {
        Band::Partial
    } else {
        Band::Exact
    }
}

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
        // Endpoints resolution rejects: multi-scalar, empty, mixed kinds —
        // each class at BOTH positions, since the two sides carry
        // independent single_scalar copies and a position-blind matrix
        // cannot catch an endpoint-asymmetric drift.
        assert_in_lockstep(&glyph("ab"), &glyph("c"));
        assert_in_lockstep(&glyph("a"), &glyph("bc"));
        assert_in_lockstep(&glyph("a"), &glyph(""));
        assert_in_lockstep(&glyph(""), &glyph("a"));
        assert_in_lockstep(&num(1), &glyph("a"));
        assert_in_lockstep(&glyph("a"), &num(1));
        // A non-canonical `written` at a range endpoint: the `05` ≡ `5`
        // identity must hold there too, not just on the single-literal path.
        let five_written_05 = SymLit::Number {
            value: 5,
            written: "05".to_string(),
            span: Span::new(1, 1, 1, 3),
        };
        assert_in_lockstep(&five_written_05, &num(7));
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
        // No separate no-surrogate assertion: a Rust `char` can never hold
        // one, so the type system already guarantees it.
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

        /// Arbitrary glyph endpoints stay in lockstep over the broad endpoint
        /// space `any::<char>()` draws from; the delta bound (±3000, wider
        /// than the 2048-wide surrogate gap) merely permits a generated pair
        /// to straddle the gap, it does not aim for it — `any::<char>()`
        /// rarely lands near `0xD800`, so gap coverage here is incidental,
        /// not guaranteed. The gap itself is covered deterministically by
        /// the matrix's gap case and the derivation test above. A target
        /// landing IN the gap is not a valid endpoint and is discarded.
        #[test]
        fn glyph_endpoints_stay_in_lockstep(lo in any::<char>(), delta in -3000i64..=3000) {
            let target = (i64::from(lo as u32) + delta).clamp(0, 0x10FFFF) as u32;
            prop_assume!(char::from_u32(target).is_some());
            let hi = char::from_u32(target).unwrap();
            assert_in_lockstep(&glyph(&lo.to_string()), &glyph(&hi.to_string()));
        }
    }
}
