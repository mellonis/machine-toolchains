//! `binding-product-threshold`: a rule whose range cells expand to a large
//! cartesian product of match rows. The compiler already raises this during
//! range expansion; this rule RE-EXPOSES it on the lint channel under allow
//! control, computing the product source-level (a wildcard and a single each
//! contribute one row; a range contributes one per member present in the
//! tape's alphabet) rather than running expansion. Shares the compiler's
//! cutoff so the two agree.

use mtc_core::diagnostics::Diagnostic;

use crate::expand::PRODUCT_THRESHOLD;
use crate::lint::LintContext;
use crate::lint::patterns::{glyph_label, range_labels};
use crate::parser::{PatternCell, PatternCellKind};

/// How many match rows one cell contributes, mirroring the expander's per-cell
/// option count: a wildcard stays one row, a concrete single is one (zero when
/// it is not on the tape — a dead cell), a range is one per in-alphabet member.
fn factor(cell: &PatternCell, tape_glyphs: &[String]) -> usize {
    match &cell.kind {
        PatternCellKind::Wildcard => 1,
        PatternCellKind::Single(s) => usize::from(tape_glyphs.contains(&glyph_label(s))),
        PatternCellKind::Range { lo, hi } => match range_labels(lo, hi) {
            Some(labels) => labels.iter().filter(|l| tape_glyphs.contains(l)).count(),
            // An unresolvable range: under-count to one, never over-report.
            None => 1,
        },
    }
}

pub(crate) fn check(ctx: &LintContext, out: &mut Vec<Diagnostic>) {
    for world in &ctx.resolved.worlds {
        let Some(tape_glyphs) = world
            .tapes
            .iter()
            .map(|t| crate::lint::alphabet_glyphs(ctx.resolved, &t.alphabet))
            .collect::<Option<Vec<_>>>()
        else {
            continue;
        };
        for state in &world.states {
            for rule in &state.rules {
                if rule.pattern.cells.len() != tape_glyphs.len() {
                    continue;
                }
                let product = rule
                    .pattern
                    .cells
                    .iter()
                    .zip(&tape_glyphs)
                    .fold(1usize, |acc, (cell, glyphs)| {
                        acc.saturating_mul(factor(cell, glyphs))
                    });
                if product > PRODUCT_THRESHOLD {
                    out.push(Diagnostic {
                        code: "binding-product-threshold",
                        span: rule.span,
                        message: format!(
                            "rule expands to {product} match rows (over {PRODUCT_THRESHOLD}) — the binding product is large"
                        ),
                        fix: None,
                    });
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::lint::{LintOptions, lint};

    fn count(src: &str) -> usize {
        lint(src, LintOptions::default())
            .unwrap()
            .diagnostics
            .iter()
            .filter(|d| d.code == "binding-product-threshold")
            .count()
    }

    // A 21-symbol alphabet; a two-range rule expands to 21 * 21 = 441 rows.
    const OVER: &str = "\
alphabet big { 0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20 }
machine {
  tape a: big;
  tape b: big;
  entry state s {
    [0..20, 0..20] -> stop;
    [*, *]         -> stop;
  }
}
";

    #[test]
    fn a_large_range_product_fires() {
        assert_eq!(count(OVER), 1);
    }

    #[test]
    fn wildcards_and_singles_do_not_multiply() {
        // All-wildcard and single-symbol rows stay one row each — never over.
        let src = "\
alphabet big { 0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20 }
machine {
  tape a: big;
  tape b: big;
  entry state s {
    [*, *]  -> move [>, .] goto s;
    [5, 10] -> stop;
    [*, 3]  -> stop;
  }
}
";
        assert_eq!(count(src), 0);
    }

    #[test]
    fn allow_suppresses_the_finding() {
        let report = lint(
            OVER,
            LintOptions {
                allow: vec!["binding-product-threshold".to_string()],
                warn: Vec::new(),
            },
        )
        .unwrap();
        assert!(
            report
                .diagnostics
                .iter()
                .all(|d| d.code != "binding-product-threshold")
        );
    }
}
