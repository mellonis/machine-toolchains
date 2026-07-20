//! `state-may-trap` (OPT-IN): a state whose rules leave some input unmatched
//! and that has no catch-all, so the match engine traps (NoTransition) on that
//! input. Off by default — the totality lint is deliberately noisy (a
//! deliberately-partial state is idiomatic). Enable it per run with `--warn
//! state-may-trap`; it is never turned on by removing an allow.
//!
//! Soundness: the rule PROVES a gap before firing. It builds each rule's
//! per-cell match set over the tape alphabets and enumerates the full input
//! product, flagging only when a concrete tuple matches no rule. A state with
//! a catch-all (an all-wildcard rule) trivially covers everything and is never
//! flagged; a state carrying a rule with an unresolvable range is skipped
//! (coverage cannot be proven, so no gap is claimed); and a product too large
//! to enumerate cheaply is skipped rather than guessed. Every path errs toward
//! silence — a false positive is never emitted. Report-only.

use std::collections::HashSet;

use mtc_core::diagnostics::Diagnostic;

use crate::lint::LintContext;
use crate::lint::patterns::{Band, band, cell_labels};

/// The largest input product this rule will enumerate; above it the state is
/// skipped (coverage left unproven) rather than guessed at.
const MAX_ENUMERATED: usize = 1 << 16;

/// Each rule's per-cell match set over the tape alphabets, or `None` if any
/// rule's arity mismatches or carries an unresolvable range cell.
fn rule_sets(
    state: &crate::parser::State,
    tape_glyphs: &[&[String]],
) -> Option<Vec<Vec<HashSet<String>>>> {
    state
        .rules
        .iter()
        .map(|rule| {
            if rule.pattern.cells.len() != tape_glyphs.len() {
                return None;
            }
            rule.pattern
                .cells
                .iter()
                .zip(tape_glyphs)
                .map(|(cell, glyphs)| cell_labels(cell, glyphs).map(|v| v.into_iter().collect()))
                .collect::<Option<Vec<HashSet<String>>>>()
        })
        .collect()
}

/// True when some input tuple over `tape_glyphs` matches no rule.
fn has_gap(rules: &[Vec<HashSet<String>>], tape_glyphs: &[&[String]]) -> bool {
    let cards: Vec<usize> = tape_glyphs.iter().map(|g| g.len()).collect();
    let total: usize = cards.iter().product();
    if total == 0 || total > MAX_ENUMERATED {
        return false; // nothing to enumerate, or too large to prove a gap
    }
    for n in 0..total {
        // Decode `n` into a per-tape glyph via mixed-radix over the alphabets.
        let mut rem = n;
        let mut tuple: Vec<&str> = Vec::with_capacity(cards.len());
        for (k, &card) in cards.iter().enumerate() {
            tuple.push(tape_glyphs[k][rem % card].as_str());
            rem /= card;
        }
        let matched = rules
            .iter()
            .any(|rule| rule.iter().zip(&tuple).all(|(set, g)| set.contains(*g)));
        if !matched {
            return true;
        }
    }
    false
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
            // A catch-all (all-wildcard) rule covers every input — no trap.
            if state
                .rules
                .iter()
                .any(|r| band(&r.pattern.cells) == Band::CatchAll)
            {
                continue;
            }
            let Some(rules) = rule_sets(state, &tape_glyphs) else {
                continue;
            };
            if has_gap(&rules, &tape_glyphs) {
                out.push(Diagnostic {
                    code: "state-may-trap",
                    span: state.name_span,
                    message: format!(
                        "state `{}` may trap — its rules do not cover every input and there is no catch-all",
                        state.name
                    ),
                    fix: None,
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::lint::{LintOptions, lint};

    fn warn_opts() -> LintOptions {
        LintOptions {
            allow: Vec::new(),
            warn: vec!["state-may-trap".to_string()],
        }
    }

    fn count(src: &str, opts: LintOptions) -> usize {
        lint(src, opts)
            .unwrap()
            .diagnostics
            .iter()
            .filter(|d| d.code == "state-may-trap")
            .count()
    }

    // The '_' cell is unmatched and there is no catch-all.
    const PARTIAL: &str = "\
alphabet bit { '_', '1' }
machine {
  tape t: bit;
  entry state s { ['1'] -> stop; }
}
";

    #[test]
    fn off_by_default_even_on_a_partial_state() {
        assert_eq!(count(PARTIAL, LintOptions::default()), 0);
    }

    #[test]
    fn warn_enables_it_and_a_gap_fires() {
        let f = count(PARTIAL, warn_opts());
        assert_eq!(f, 1);
    }

    #[test]
    fn a_total_state_is_quiet_even_when_warned() {
        // Every symbol has a rule — no gap.
        let src = "\
alphabet bit { '_', '1' }
machine {
  tape t: bit;
  entry state s { ['_'] -> stop; ['1'] -> stop; }
}
";
        assert_eq!(count(src, warn_opts()), 0);
    }

    #[test]
    fn a_catch_all_state_is_quiet_even_when_warned() {
        let src = "\
alphabet bit { '_', '1' }
machine {
  tape t: bit;
  entry state s { ['1'] -> stop; [*] -> stop; }
}
";
        assert_eq!(count(src, warn_opts()), 0);
    }

    #[test]
    fn allow_beats_warn() {
        // Naming it in both `warn` and `allow` keeps it off.
        let opts = LintOptions {
            allow: vec!["state-may-trap".to_string()],
            warn: vec!["state-may-trap".to_string()],
        };
        assert_eq!(count(PARTIAL, opts), 0);
    }
}
