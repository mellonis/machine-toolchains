//! `.tmc` lint layer: hygiene findings over the compiler's analysis, the
//! front-end mirror of the `.pmc` lint layer in the sibling PM-1 crate.
//! Library-only — the CLI renders (docs/cli.md (thin-renderer rule)). Strict
//! channel split: lint reports lint findings ONLY; the compile warnings stay on
//! the compile channel (`tmt compile`) and are never re-reported here — with
//! one deliberate exception, the `unused-import`/`unused-routine` re-exposure
//! (below), which surfaces existing hygiene warnings under allow control so a
//! `tmt lint` run and its allow-list cover them too.
//!
//! # What the rules see
//!
//! Rules read one [`crate::compiler::Analysis`] — tokens, the flat program, the
//! resolved module (worlds / alphabets / docs), and analyze's own non-fatal
//! diagnostics. The lint runs only over the resolution stage's output, never
//! `expand`/`lower`: the two later-stage hygiene warnings this layer also
//! carries (`unused-routine` and `binding-product-threshold`, which the
//! compiler raises during IR lowering and expansion respectively) are
//! re-detected here at source level over `Resolved`, not harvested by running
//! those stages (which could fatal on input the resolve stage accepted).
//!
//! # Staged-seam limitation
//!
//! Lint runs on successfully-*analyzed* input: `lint()` bails with a fatal if
//! resolution does not complete. A source that fatals partway through
//! resolution therefore yields no lint findings at all, even for the earlier,
//! unaffected declarations — the resolve stage stops at the first offending
//! span rather than accumulating. This mainly matters for the future editor
//! service (which wants findings on broken-in-the-middle documents); the
//! batch CLI reports the fatal and moves on, so it is not a `tmt lint` defect.
//! Not fixed here.

pub(crate) mod patterns;
pub mod rules;

use mtc_core::diagnostics::Diagnostic;

use crate::compiler::{self, CompileError, Resolved};

#[derive(Debug, Clone, Default)]
pub struct LintOptions {
    /// Rule codes to suppress. Unknown codes are an error (typo protection).
    pub allow: Vec<String>,
    /// Opt-in rule codes to ENABLE (the default-off rules, e.g.
    /// `state-may-trap`). Explicit enablement, never allow-removal; unknown
    /// codes are an error, same as `allow`.
    pub warn: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct LintReport {
    /// Lint findings only, source-ordered by span start (stable).
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug)]
pub enum LintError {
    /// Lint requires a program that parses and resolves.
    Compile(CompileError),
    /// `--allow`/`--warn` named a code no rule declares.
    UnknownAllowCode(String),
}

impl std::fmt::Display for LintError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LintError::Compile(e) => write!(f, "{e}"),
            LintError::UnknownAllowCode(code) => {
                write!(f, "unknown lint rule `{code}`")
            }
        }
    }
}

impl std::error::Error for LintError {}

impl From<CompileError> for LintError {
    fn from(e: CompileError) -> Self {
        LintError::Compile(e)
    }
}

/// Everything a rule may read. Rules never mutate the analysis. Every rule
/// works off the resolved module (worlds with source-form rules, alphabets,
/// the doc map); the `unused-import` re-exposure additionally reads analyze's
/// own non-fatal diagnostics.
pub(crate) struct LintContext<'a> {
    /// The resolved module — worlds (rules in source form), alphabets, and the
    /// top-level doc map.
    pub resolved: &'a Resolved,
    /// analyze's own non-fatal diagnostics. The `unused-import` rule re-exposes
    /// its entries under allow control (the compile channel keeps them too).
    pub diagnostics: &'a [Diagnostic],
}

/// A lint rule: reads the analysis context, pushes any findings.
type Rule = fn(&LintContext, &mut Vec<Diagnostic>);

/// The default-on rule table. One entry per rule, keyed by its defect-named
/// code; registration order is irrelevant (findings are sorted by span).
pub(crate) const RULES: &[(&str, Rule)] = &[
    ("leftover-debugger", rules::leftover_debugger::check),
    ("unused-import", rules::unused_import::check),
    ("unused-routine", rules::unused_routine::check),
    ("unused-graph", rules::unused_graph::check),
    ("unused-binding", rules::unused_binding::check),
    ("unused-graft-instance", rules::unused_graft_instance::check),
    ("deprecated-call", rules::deprecated_call::check),
    ("dead-rule", rules::dead_rule::check),
    (
        "redundant-identity-pairs",
        rules::redundant_identity_pairs::check,
    ),
    (
        "binding-product-threshold",
        rules::binding_product_threshold::check,
    ),
    (
        "writes-through-collapse",
        rules::writes_through_collapse::check,
    ),
];

/// The opt-in rule table: off by default, run only when `--warn` names the
/// code (the totality lints, deliberately noisy). In the known-code namespace
/// (so a shared allow-list may still name one) but never run unless enabled.
pub(crate) const OPT_IN_RULES: &[(&str, Rule)] =
    &[("state-may-trap", rules::state_may_trap::check)];

/// True when `code` names any rule in this crate's `.tmc` tables OR core's
/// arch-agnostic asm rule table (`mtc_core::asm::lint::RULES`) — the shared
/// allow namespace. One `tmt.json` serves both languages, so a `.tma`-only
/// code must not error when validated for a `.tmc` file, and vice versa.
pub(crate) fn known_code(code: &str) -> bool {
    RULES.iter().any(|(c, _)| *c == code)
        || OPT_IN_RULES.iter().any(|(c, _)| *c == code)
        || mtc_core::asm::lint::RULES.iter().any(|(c, _)| *c == code)
}

/// `--allow`/`--warn` codes must each name a real rule (typo protection), over
/// the shared namespace. Split out of `lint()` so a caller (the future editor
/// service, `tmt.json` loading) can validate an allow-list up front,
/// independently of running the rules over any particular analysis.
pub(crate) fn validate_allow(codes: &[String]) -> Result<(), LintError> {
    for code in codes {
        if !known_code(code) {
            return Err(LintError::UnknownAllowCode(code.clone()));
        }
    }
    Ok(())
}

/// Run every enabled, non-allowed rule over `ctx`, source-ordered by span
/// start (stable). The default table runs unless allowed; an opt-in rule runs
/// only when `warn` names it and `allow` does not. Split out of `lint()` so the
/// editor service can lint an `Analysis` it already has, instead of re-running
/// `compiler::analyze`.
pub(crate) fn run_rules(ctx: &LintContext, allow: &[String], warn: &[String]) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    for (code, rule) in RULES {
        if allow.iter().any(|a| a == code) {
            continue;
        }
        rule(ctx, &mut diagnostics);
    }
    for (code, rule) in OPT_IN_RULES {
        if !warn.iter().any(|w| w == code) || allow.iter().any(|a| a == code) {
            continue;
        }
        rule(ctx, &mut diagnostics);
    }
    diagnostics.sort_by_key(|d| d.span.start); // stable; Pos is Ord
    diagnostics
}

/// The glyph labels of a resolved alphabet by mangled name, in position order.
pub(crate) fn alphabet_glyphs<'a>(resolved: &'a Resolved, mangled: &str) -> Option<&'a [String]> {
    resolved.alphabets.get(mangled).map(|a| a.glyphs.as_slice())
}

pub fn lint(source: &str, options: LintOptions) -> Result<LintReport, LintError> {
    validate_allow(&options.allow)?;
    validate_allow(&options.warn)?;
    let analysis = compiler::analyze(source)?;
    let ctx = LintContext {
        resolved: &analysis.resolved,
        diagnostics: &analysis.diagnostics,
    };
    let diagnostics = run_rules(&ctx, &options.allow, &options.warn);
    Ok(LintReport { diagnostics })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_program_with_no_rules_yields_empty_report() {
        let src = "\
alphabet bit { '_', '1' }
machine {
  tape t: bit;
  entry state s { [*] -> stop; }
}
";
        let report = lint(src, LintOptions::default()).unwrap();
        assert!(report.diagnostics.is_empty(), "{:?}", report.diagnostics);
    }

    #[test]
    fn unknown_allow_code_is_an_error() {
        let err = lint(
            "machine { }",
            LintOptions {
                allow: vec!["no-such-rule".into()],
                warn: Vec::new(),
            },
        )
        .unwrap_err();
        assert!(matches!(err, LintError::UnknownAllowCode(ref c) if c == "no-such-rule"));
        assert!(err.to_string().contains("no-such-rule"));
    }

    #[test]
    fn fatal_parse_error_propagates() {
        let err = lint("machine {", LintOptions::default()).unwrap_err();
        assert!(matches!(err, LintError::Compile(_)));
    }

    #[test]
    fn validate_allow_accepts_known_codes_and_rejects_unknown_ones() {
        assert!(validate_allow(&["leftover-debugger".to_string()]).is_ok());
        // The opt-in rule is a known code too (a shared allow-list may name it).
        assert!(validate_allow(&["state-may-trap".to_string()]).is_ok());
        assert!(validate_allow(&[]).is_ok());
        let err = validate_allow(&["no-such-rule".to_string()]).unwrap_err();
        assert!(matches!(err, LintError::UnknownAllowCode(ref c) if c == "no-such-rule"));
    }

    #[test]
    fn validate_allow_also_accepts_asm_only_codes() {
        // "unreachable-code" names no `.tmc` rule — it's asm-only
        // (`mtc_core::asm::lint::RULES`). A `tmt.json` shared by both
        // languages must not choke on it while validating for `.tmc`.
        assert!(!RULES.iter().any(|(c, _)| *c == "unreachable-code"));
        assert!(
            mtc_core::asm::lint::RULES
                .iter()
                .any(|(c, _)| *c == "unreachable-code")
        );
        assert!(validate_allow(&["unreachable-code".to_string()]).is_ok());
    }
}
