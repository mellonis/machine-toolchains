//! Shared pattern-cell helpers for the coverage-based rules (`dead-rule`,
//! `binding-product-threshold`, `state-may-trap`): the glyph labels a pattern
//! cell matches over its tape's alphabet, and a rule's dispatch band. All
//! source-level over the resolved worlds — no expansion is run.

use crate::parser::{PatternCell, PatternCellKind, SymLit};

/// The glyph label a symbol literal denotes. A numeric literal's identity is
/// its value's decimal string (`05` and `5` both label `"5"`), matching the
/// alphabet-resolution rule (docs/language.md (alphabets), once it lands).
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
/// codegen; docs/formats.md (match and dispatch tables)): all-wildcard is
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
