//! Shared pattern-cell helpers, source-level over the resolved worlds — no
//! expansion is run: the glyph labels a pattern cell matches over its
//! tape's alphabet, a rule's dispatch band, and the per-tape glyph sets a
//! slice of rules accepts. Three call sites read this module: the
//! coverage-based lint rules (`dead-rule`, `state-may-trap`) and the
//! compiler's static head-contract check (`enters`/`leaves`,
//! docs/tmt/language.md (head-position clauses)) — one computation shared
//! by lint and the compiler rather than a copy kept in step with it by
//! hand.

use std::collections::HashSet;

use crate::parser::{PatternCell, PatternCellKind, Rule, SymLit};

/// The glyph label a symbol literal denotes. A numeric literal's identity is
/// its value's decimal string (`05` and `5` both label `"5"`), matching the
/// alphabet-resolution rule (docs/tmt/language.md (alphabets)).
pub(crate) fn glyph_label(s: &SymLit) -> String {
    match s {
        SymLit::Glyph { value, .. } => value.clone(),
        SymLit::Number { value, .. } => value.to_string(),
    }
}

/// The number a symbol label names, if it names one: the label is the
/// canonical decimal spelling of a value a number literal can denote — so
/// `'7'` and `7`, one symbol with the label `7`, both name 7, while `'07'`,
/// `'+7'` and `'x'` name none.
pub(crate) fn label_number(label: &str) -> Option<i64> {
    let value: u32 = label.parse().ok()?;
    (value.to_string() == label).then_some(i64::from(value))
}

/// Why a range written in a pattern cell or a contract clause has no walk
/// over the alphabet it is written against.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DeclaredRangeMiss {
    /// This endpoint's label is not a symbol of the alphabet.
    Endpoint(String),
    /// Both endpoints are symbols, but the second comes first in the
    /// alphabet's declared order.
    Reversed,
}

/// The symbols a range written in a pattern cell or a contract clause
/// stands for: `glyphs` — the alphabet it is written against, in declared
/// order — from `lo`'s position to `hi`'s, inclusive. There is no
/// succession to walk here, only the order the alphabet declares, so a
/// multi-character endpoint is as good as a single scalar, and no member
/// can fall outside by construction; the only failures are an endpoint the
/// alphabet lacks and a pair written against the declared order
/// (docs/tmt/language.md (pattern ranges)). A range in an alphabet or set BODY is
/// the other walk — Unicode succession, which is what creates an order —
/// and is `compiler::expand_range`'s.
pub(crate) fn declared_range<'g>(
    lo: &SymLit,
    hi: &SymLit,
    glyphs: &'g [String],
) -> Result<&'g [String], DeclaredRangeMiss> {
    let position = |s: &SymLit| {
        let label = glyph_label(s);
        glyphs
            .iter()
            .position(|g| *g == label)
            .ok_or(DeclaredRangeMiss::Endpoint(label))
    };
    let (from, to) = (position(lo)?, position(hi)?);
    if from > to {
        return Err(DeclaredRangeMiss::Reversed);
    }
    Ok(&glyphs[from..=to])
}

/// The glyph labels a pattern cell matches over `tape_glyphs` (its tape's
/// alphabet, position order): a wildcard matches the whole alphabet, a single
/// its one label, a range the declared-order run between its endpoints
/// ([`declared_range`]), a set its members as resolution filled them in.
/// `None` when a range has no walk over the alphabet or a set cell is not
/// yet resolved — the caller then declines to reason about the cell.
pub(crate) fn cell_labels(cell: &PatternCell, tape_glyphs: &[String]) -> Option<Vec<String>> {
    match &cell.kind {
        PatternCellKind::Wildcard => Some(tape_glyphs.to_vec()),
        PatternCellKind::Single(s) => Some(vec![glyph_label(s)]),
        PatternCellKind::Range { lo, hi } => declared_range(lo, hi, tape_glyphs)
            .ok()
            .map(<[String]>::to_vec),
        PatternCellKind::SetRef { resolved, .. } => resolved
            .as_ref()
            .map(|r| r.members.iter().map(|m| m.label.clone()).collect()),
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

/// The per-tape glyph sets `rules` accepts, one set per tape position: the
/// UNION, across every rule in the slice, of what [`cell_labels`] returns
/// for that rule's cell at that position. `None` when any rule's arity does
/// not match `tape_glyphs`'s width or carries an unresolvable range cell —
/// the same "cannot prove, so decline" posture `cell_labels` itself takes.
///
/// Called with a whole state's rules this is exactly what the state accepts
/// at each tape position — the coverage question `state-may-trap` and the
/// compiler's `enters` check both ask. Called with a single rule (a
/// one-element slice) it degenerates to that rule's own per-tape sets —
/// `dead-rule`'s and `state-may-trap`'s per-rule question — so one
/// computation answers both shapes.
pub(crate) fn accepted_glyphs(
    rules: &[Rule],
    tape_glyphs: &[&[String]],
) -> Option<Vec<HashSet<String>>> {
    let mut out: Vec<HashSet<String>> = vec![HashSet::new(); tape_glyphs.len()];
    for rule in rules {
        if rule.pattern.cells.len() != tape_glyphs.len() {
            return None;
        }
        for (slot, (cell, glyphs)) in out
            .iter_mut()
            .zip(rule.pattern.cells.iter().zip(tape_glyphs))
        {
            slot.extend(cell_labels(cell, glyphs)?);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use mtc_core::diagnostics::Span;

    use crate::compiler::glyph_label as compiler_glyph_label;
    use crate::parser::SymLit;

    use super::{DeclaredRangeMiss, declared_range, glyph_label};

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

    fn labels(glyphs: &[&str]) -> Vec<String> {
        glyphs.iter().map(|g| g.to_string()).collect()
    }

    /// A range written against an alphabet walks the alphabet's declared
    /// order, derived here by hand: over `'_', 'a', 'z', 'b'`, `'a'..'z'`
    /// is the two symbols between those positions — never the 26 of
    /// Unicode succession, and never `'b'`, which comes after `'z'`.
    /// Mutation: walking succession and keeping the members the alphabet
    /// carries answers `a, b, z`.
    #[test]
    fn a_range_is_the_declared_run_between_its_endpoints() {
        let alphabet = labels(&["_", "a", "z", "b"]);
        assert_eq!(
            declared_range(&glyph("a"), &glyph("z"), &alphabet),
            Ok(&alphabet[1..=2])
        );
        assert_eq!(
            declared_range(&glyph("z"), &glyph("b"), &alphabet),
            Ok(&alphabet[2..=3])
        );
        assert_eq!(
            declared_range(&glyph("a"), &glyph("a"), &alphabet),
            Ok(&alphabet[1..=1])
        );
    }

    /// The two failures: an endpoint the alphabet lacks, named, and a pair
    /// the declared order reverses — including one Unicode succession would
    /// call ascending.
    #[test]
    fn a_missing_endpoint_or_a_reversed_pair_has_no_walk() {
        let alphabet = labels(&["_", "b", "a"]);
        assert_eq!(
            declared_range(&glyph("a"), &glyph("c"), &alphabet),
            Err(DeclaredRangeMiss::Endpoint("c".to_string()))
        );
        assert_eq!(
            declared_range(&glyph("x"), &glyph("a"), &alphabet),
            Err(DeclaredRangeMiss::Endpoint("x".to_string()))
        );
        assert_eq!(
            declared_range(&glyph("a"), &glyph("b"), &alphabet),
            Err(DeclaredRangeMiss::Reversed)
        );
    }

    /// No succession is walked, so an endpoint of several characters is a
    /// symbol like any other, and a number endpoint is looked up by its
    /// label (`05` is the symbol `5`).
    #[test]
    fn multi_character_and_numeric_endpoints_are_looked_up_by_label() {
        let alphabet = labels(&["_", "ab", "cd", "ef"]);
        assert_eq!(
            declared_range(&glyph("ab"), &glyph("cd"), &alphabet),
            Ok(&alphabet[1..=2])
        );
        let digits = labels(&["_", "5", "x", "7"]);
        let five_written_05 = SymLit::Number {
            value: 5,
            written: "05".to_string(),
            span: Span::new(1, 1, 1, 3),
        };
        assert_eq!(
            declared_range(&five_written_05, &num(7), &digits),
            Ok(&digits[1..=3])
        );
    }

    /// Both sides label a numeric literal by its VALUE's decimal string
    /// (the `05` ≡ `5` rule, docs/tmt/language.md (alphabets)).
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
}
