//! `docs/tmt/lint.md`'s per-rule `### heading` rows, set-compared against
//! this crate's own rule registries — modelled on
//! `tests/error_code_docs.rs`'s catalog guard, which does the same for
//! `docs/tmt/cli.md`'s compile-error table. Crate-INTERNAL (a `#[cfg(test)]`
//! module here, not a file under `crates/turing-machine/tests/`) because
//! [`super::RULES`], [`super::OPT_IN_RULES`] and [`super::tma::TMA_RULES`]
//! are `pub(crate)` — unlike `CompileErrorKind::CODES`, which
//! `error_code_docs.rs` reaches because it is `pub` at the crate root —
//! so an external integration test cannot see them without first widening
//! that visibility, which this guard does not need.
//!
//! Two catalogs are covered: the `.tmc` rules (`## The \`.tmc\` rules`,
//! [`super::RULES`] union [`super::OPT_IN_RULES`]) and the `.tmc` crate's
//! own `.tma` ADDITIONS (`## The \`.tma\` additions`, [`super::tma::TMA_RULES`]).
//! The page's THIRD section, `## The arch-agnostic rules on \`.tma\``, is
//! deliberately NOT covered: it is core's shared assembly-lint catalog,
//! described here in prose (one heading, `unused-label`, naming a
//! cross-cutting behavior rather than tabulating core's own rule-by-rule
//! catalog), not a per-rule heading list this page owns — there is no
//! reachable registry of "every core asm rule this page should list" to
//! compare against from here; `docs/core.md (assembly lint)` is that
//! catalog's own home.

#[cfg(test)]
mod tests {
    use crate::lint::tma::TMA_RULES;
    use crate::lint::{OPT_IN_RULES, RULES};

    fn doc() -> String {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/tmt/lint.md");
        std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()))
    }

    /// The lines strictly between `heading` (a `## `-level line, matched
    /// verbatim) and the next `## `-level line (or EOF) — deliberately
    /// stops at `## `, not `### `, so a rule's own `### ` heading never
    /// ends the section early.
    fn section<'a>(doc: &'a str, heading: &str) -> Vec<&'a str> {
        let mut lines = doc.lines();
        for line in lines.by_ref() {
            if line.trim_end() == heading {
                break;
            }
        }
        lines.take_while(|l| !l.starts_with("## ")).collect()
    }

    /// Every `### CODE …` heading's own CODE — the first whitespace-
    /// delimited token after `### `, which is exactly the rule code on
    /// every heading in the two sections this guard reads: an annotation
    /// like `(opt-in)` or `` (`.tmc`) `` always follows a space, and no
    /// code itself contains one or is backtick-wrapped.
    fn heading_codes(lines: &[&str]) -> Vec<String> {
        lines
            .iter()
            .filter(|l| l.starts_with("### "))
            .map(|l| {
                l.trim_start_matches("### ")
                    .split_whitespace()
                    .next()
                    .expect("a heading has at least one word")
                    .trim_matches('`')
                    .to_string()
            })
            .collect()
    }

    /// Mutation it catches: registering a new default-on or opt-in `.tmc`
    /// rule (adding it to [`RULES`]/[`OPT_IN_RULES`]) with no matching
    /// `### ` row on this page, or leaving a row behind after retiring a
    /// rule — either direction fails the `assert_eq!` below.
    #[test]
    fn the_tmc_rule_headings_match_the_registry_both_directions() {
        let doc = doc();
        let mut documented = heading_codes(&section(&doc, "## The `.tmc` rules"));
        documented.sort_unstable();
        let mut registered: Vec<String> = RULES
            .iter()
            .chain(OPT_IN_RULES)
            .map(|(c, _)| c.to_string())
            .collect();
        registered.sort_unstable();
        assert_eq!(
            documented, registered,
            "docs/tmt/lint.md (## The `.tmc` rules) and RULES/OPT_IN_RULES disagree"
        );
    }

    /// The same guard over the `.tmc` crate's own `.tma` additions —
    /// [`TMA_RULES`], the arch-specific rules this crate registers ON TOP
    /// of core's shared assembly-lint catalog (never that catalog
    /// itself — see the module doc).
    #[test]
    fn the_tma_additions_headings_match_the_registry_both_directions() {
        let doc = doc();
        let mut documented = heading_codes(&section(&doc, "## The `.tma` additions"));
        documented.sort_unstable();
        let mut registered: Vec<String> = TMA_RULES.iter().map(|(c, _)| c.to_string()).collect();
        registered.sort_unstable();
        assert_eq!(
            documented, registered,
            "docs/tmt/lint.md (## The `.tma` additions) and TMA_RULES disagree"
        );
    }
}
