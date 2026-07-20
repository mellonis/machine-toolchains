//! `dead-rule`: within one state, a rule an earlier, higher-priority rule in
//! the SAME dispatch band already covers — it can never fire.
//!
//! # Cover
//!
//! Rule `W` covers rule `R` cell-wise iff, at every tape position, the glyph
//! set `W` matches there is a SUPERSET of the set `R` matches (a wildcard
//! matches the whole alphabet; a single its one glyph; a range its span). When
//! that holds at every position, every input `R` matches, `W` matches too.
//!
//! # Same band only
//!
//! Codegen does NOT dispatch rows in source order — it re-bands a state into
//! `[exact] ++ [partial] ++ [catch-all]` and takes the first match in THAT
//! order (crate::codegen; docs/formats.md (match and dispatch tables)). So an
//! earlier SOURCE rule shadows a later one it covers only when both land in the
//! same band, where source order equals runtime order — within the partial
//! band and within the catch-all band. The exact band is excluded: two
//! wildcard-free rules that overlap are an exact-row conflict the compiler
//! rejects outright, not a silent shadow, so this rule leaves that case to the
//! compiler. A covering rule always has at least as many wildcards as the rule
//! it covers, so it is never in a LATER band; requiring the same band is the
//! sound subset. Report-only.

use std::collections::HashSet;

use mtc_core::diagnostics::Diagnostic;

use crate::lint::LintContext;
use crate::lint::patterns::{Band, band, cell_labels};

/// A rule's per-cell match sets plus its band, or `None` when its arity does
/// not match the world's tapes or a range cell is unresolvable (the lint then
/// neither covers with it nor reports it).
type RuleSets = Option<(Band, Vec<HashSet<String>>)>;

fn covers(w: &[HashSet<String>], r: &[HashSet<String>]) -> bool {
    w.iter().zip(r).all(|(ws, rs)| ws.is_superset(rs))
}

pub(crate) fn check(ctx: &LintContext, out: &mut Vec<Diagnostic>) {
    for world in &ctx.resolved.worlds {
        // The alphabet label set per tape position; skip the world if any tape
        // alphabet is missing (never expected past a clean analysis).
        let Some(tape_glyphs) = world
            .tapes
            .iter()
            .map(|t| crate::lint::alphabet_glyphs(ctx.resolved, &t.alphabet))
            .collect::<Option<Vec<_>>>()
        else {
            continue;
        };

        for state in &world.states {
            let sets: Vec<RuleSets> = state
                .rules
                .iter()
                .map(|rule| {
                    if rule.pattern.cells.len() != tape_glyphs.len() {
                        return None;
                    }
                    let cells: Option<Vec<HashSet<String>>> = rule
                        .pattern
                        .cells
                        .iter()
                        .zip(&tape_glyphs)
                        .map(|(cell, glyphs)| {
                            cell_labels(cell, glyphs).map(|v| v.into_iter().collect())
                        })
                        .collect();
                    cells.map(|c| (band(&rule.pattern.cells), c))
                })
                .collect();

            for j in 0..state.rules.len() {
                let Some((band_j, ref sets_j)) = sets[j] else {
                    continue;
                };
                if band_j == Band::Exact {
                    continue;
                }
                let shadowed = (0..j).any(|i| {
                    matches!(&sets[i], Some((band_i, sets_i)) if *band_i == band_j && covers(sets_i, sets_j))
                });
                if shadowed {
                    out.push(Diagnostic {
                        code: "dead-rule",
                        span: state.rules[j].span,
                        message: format!(
                            "this rule is unreachable — an earlier rule in `{}` already covers it",
                            state.name
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
            .filter(|d| d.code == "dead-rule")
            .count()
    }

    #[test]
    fn a_catch_all_shadows_a_later_catch_all() {
        // Two all-wildcard rows: the second can never fire.
        let src = "\
alphabet bit { '_', '1' }
machine {
  tape t: bit;
  entry state s {
    [*] -> move [>] goto s;
    [*] -> stop;
  }
}
";
        assert_eq!(count(src), 1);
    }

    #[test]
    fn a_partial_wildcard_shadows_a_later_partial_it_covers() {
        // Two tapes; `[*, '1']` (partial) precedes `[*, '1']` (partial, same
        // pattern) — the later is dead within the partial band.
        let src = "\
alphabet bit { '_', '1' }
machine {
  tape a: bit;
  tape b: bit;
  entry state s {
    [*, '1'] -> move [>, .] goto s;
    [*, '1'] -> stop;
    [*, *]   -> stop;
  }
}
";
        assert_eq!(count(src), 1);
    }

    #[test]
    fn a_range_shadows_a_later_single_it_contains() {
        // `[*, 0..2]` covers `[*, 1]`; both partial (tape a wildcard).
        let src = "\
alphabet num { 0, 1, 2 }
machine {
  tape a: num;
  tape b: num;
  entry state s {
    [*, 0..2] -> move [>, .] goto s;
    [*, 1]    -> stop;
    [*, *]    -> stop;
  }
}
";
        assert_eq!(count(src), 1);
    }

    #[test]
    fn a_catch_all_does_not_shadow_a_later_exact_row() {
        // The exact `['1']` is emitted BEFORE the source-earlier catch-all, so
        // it is NOT dead — different bands, no finding.
        let src = "\
alphabet bit { '_', '1' }
machine {
  tape t: bit;
  entry state s {
    [*]   -> move [>] goto s;
    ['1'] -> stop;
  }
}
";
        assert_eq!(count(src), 0);
    }

    #[test]
    fn disjoint_partial_rows_are_quiet() {
        // `['1', *]` and `[*, '1']` cover neither other.
        let src = "\
alphabet bit { '_', '1' }
machine {
  tape a: bit;
  tape b: bit;
  entry state s {
    ['1', *] -> move [>, .] goto s;
    [*, '1'] -> move [., >] goto s;
    [*, *]   -> stop;
  }
}
";
        assert_eq!(count(src), 0);
    }

    #[test]
    fn allow_suppresses_the_finding() {
        let src = "\
alphabet bit { '_', '1' }
machine {
  tape t: bit;
  entry state s {
    [*] -> move [>] goto s;
    [*] -> stop;
  }
}
";
        let report = lint(
            src,
            LintOptions {
                allow: vec!["dead-rule".to_string()],
                warn: Vec::new(),
            },
        )
        .unwrap();
        assert!(report.diagnostics.iter().all(|d| d.code != "dead-rule"));
    }
}
