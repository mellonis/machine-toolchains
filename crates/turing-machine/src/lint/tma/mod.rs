//! `.tma` lint layer: the TM-1 assembly hygiene findings, the front-end
//! mirror of the `.pma` lint route in the sibling PM-1 crate — with one
//! structural difference. Where `.pma` adds no rules of its own and calls
//! `mtc_core::asm::lint::lint` directly, TM-1's sectioned dialect (match
//! tables, `.rept` macros, frame descriptors) carries defects the five
//! arch-agnostic core rules cannot see, so this layer runs a few additional
//! rules of its own.
//!
//! # The merge seam
//!
//! Core's `mtc_core::asm::lint::lint` stays CLOSED — it runs only its own
//! five rules and exposes no extension hook. [`lint_tma`] calls it (with
//! `tm1_syntax()`, exactly as the `.pma` route does) for those five plus the
//! fatal gate, then runs the TM additions ([`TMA_RULES`]) over the same asm
//! CST and merges both diagnostic streams into one source-ordered report.
//! Because core's `lint` never hands its own CST back (and cannot be made
//! to — core is a closed dependency here), the additions re-parse with
//! `parse_asm_cst_with` under the identical `tm1_syntax()` caps; identical
//! caps yield an identical parse, so "the same CST" holds in substance.
//!
//! # unused-label runs unmodified on the `.tma` path
//!
//! `unused-label` is core's arch-agnostic rule, run here exactly as the
//! four other core rules are — nothing on this path suppresses it. It once
//! had to be: core counted only in-function jump/call operands as
//! references, so a code label reached only through a `.targets` / `.target`
//! dispatch entry or a `.exits` frame descriptor — references that live in
//! the lowered table section, not in any operand — looked unused, and a
//! dispatch-table program tripped a false finding on nearly every label (the
//! flagship brainfuck UTM: 400 of them, all reachable code). Core now feeds
//! its lint rules the lowered tables, so the rule counts a dispatch or exit
//! target as a reference and flags only genuinely dead labels — no `.tma`
//! special-casing remains. The four additions below are the defects core
//! genuinely cannot detect.
//!
//! # The allow namespace
//!
//! The four TM codes join the crate's shared allow namespace via
//! [`super::known_code`] (one more union arm over [`TMA_RULES`]), so a single
//! `tmt.json` `lint.allow` serves both languages: a `.tma`-only code does not
//! error when validated for a `.tmc` file, and vice versa. There is no
//! `--warn` opt-in tier on the `.tma` side — all four additions are
//! default-on.
//!
//! # The duplicate-`.map` finding
//!
//! A `.map` clause that repeats a source symbol (`rmap=(1->2, 1->3)`) is
//! silently accepted by the assembler, last write winning — the emitted
//! object is identical to the one `1->3` alone produces. `duplicate-map-source`
//! flags it (with a fix that removes the shadowed pair), and it lives here
//! rather than in the language server so that the one `lint_tma` call reaches
//! both `tmt lint` and the editor; a server-only check would raise a
//! diagnostic in the editor the command line never reports.

pub(crate) mod rules;

use mtc_core::asm::AsmError;
use mtc_core::asm::cst::{AsmCst, parse_asm_cst_with};
use mtc_core::diagnostics::Diagnostic;

use crate::asm::tm1_syntax;

/// Everything a `.tma` rule may read: the source text and the parsed asm
/// CST (shaped under `tm1_syntax()` caps — sections, table directives,
/// `.rept` blocks, frame descriptors, vector operands). Rules never mutate.
pub(crate) struct TmaLintContext<'a> {
    pub source: &'a str,
    pub cst: &'a AsmCst,
}

/// A `.tma` lint rule: reads the CST context, pushes any findings.
type TmaRule = fn(&TmaLintContext, &mut Vec<Diagnostic>);

/// The TM-1 assembly additions to core's five arch-agnostic rules. All
/// default-on; keyed by defect-named kebab code; registration order is
/// irrelevant (findings sort by span). These are the defects the sectioned
/// dialect introduces that the core rules cannot see.
pub(crate) const TMA_RULES: &[(&str, TmaRule)] = &[
    (
        "shadowed-wildcard-rows",
        rules::shadowed_wildcard_rows::check,
    ),
    ("retx-exit-bounds", rules::retx_exit_bounds::check),
    ("rept-var-unused", rules::rept_var_unused::check),
    ("duplicate-map-source", rules::duplicate_map_source::check),
];

/// Lint one `.tma` source: core's five rules (with the fatal gate — a full
/// assemble refuses structural and semantic errors alike) plus the TM
/// additions, merged and source-ordered. `Err` is the assemble fatal, which
/// the CLI renders as a per-file error (the batch continues), mirroring the
/// `.pma` route. Does NOT validate `allow` codes — the driver owns that over
/// the shared cross-language namespace, same as core's `lint`.
pub fn lint_tma(source: &str, allow: &[String]) -> Result<Vec<Diagnostic>, AsmError> {
    let syntax = tm1_syntax();
    let mut diagnostics = mtc_core::asm::lint::lint(&syntax, source, allow)?;
    let cst = parse_asm_cst_with(source, syntax.caps);
    let ctx = TmaLintContext { source, cst: &cst };
    for (code, rule) in TMA_RULES {
        if allow.iter().any(|a| a == code) {
            continue;
        }
        rule(&ctx, &mut diagnostics);
    }
    diagnostics.sort_by_key(|d| d.span.start); // stable; Pos is Ord
    Ok(diagnostics)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The full frames + tables surface, clean: no TM addition fires and the
    /// five core rules pass.
    const CLEAN: &str = "\
.routine main, tapes=2, alpha=(2, 2)
.section tables
T0: .row [1, *]
    .row [*, *]
D0: .targets hit, miss
.section code
.func main
        rd
        mtc  T0
        djmp D0
hit:    stp
miss:   hlt
";

    #[test]
    fn a_clean_tma_yields_no_findings() {
        // CLEAN dispatches djmp → hit/miss; those labels are reached only
        // through `D0`'s `.targets`, so unused-label (now live on this path)
        // counts them as used, and with no TM addition tripping the report is
        // empty.
        let report = lint_tma(CLEAN, &[]).unwrap();
        assert!(report.is_empty(), "{report:?}");
    }

    #[test]
    fn a_core_rule_fires_through_the_merged_entry() {
        // Dead code after `stp` is core's `unreachable-code` rule — proves the
        // core call is wired into the merged report.
        let src = "\
.routine main, tapes=2, alpha=(2, 2)
.section code
.func main
        stp
        nop
";
        let report = lint_tma(src, &[]).unwrap();
        assert!(
            report.iter().any(|d| d.code == "unreachable-code"),
            "{report:?}"
        );
    }

    #[test]
    fn unused_label_counts_dispatch_and_exit_targets_as_references() {
        // The positive control for the un-suppression: hit/miss are reached
        // only through `D0`'s `.targets`, done/other only through `F0`'s
        // `.exits` — references that live in the table section, named by no
        // operand. The rule is LIVE on this path now (not suppressed), and it
        // must count every one as used, flagging none.
        let referenced = "\
.routine main, tapes=2, alpha=(2, 2)
.routine helper, tapes=2, alpha=(2, 2)
.section tables
T0: .row [1, 1]
    .row [*, *]
D0: .targets hit, miss
F0: .frame tapes=(1, 0)
    .exits done, other
.section code
.func main
        rd
        mtc     T0
        djmp    D0
hit:    call.m  helper, F0
done:   stp
other:  hlt
miss:   hlt
.func helper
        wr      [1, -]
        retx    #1
";
        let report = lint_tma(referenced, &[]).unwrap();
        assert!(
            report.iter().all(|d| d.code != "unused-label"),
            "table-referenced labels must not be flagged: {report:?}"
        );

        // The discriminating half: `gone` is reached by no operand and by no
        // table, so the (now live) rule must still flag it — proving it is
        // doing real work, not merely inert. seen/other stay unflagged as
        // dispatch targets.
        let dead = "\
.routine main, tapes=2, alpha=(2, 2)
.section tables
T0: .row [1, 1]
    .row [*, *]
D0: .targets seen, other
.section code
.func main
        rd
        mtc     T0
        djmp    D0
seen:   stp
other:  hlt
gone:   hlt
";
        let report = lint_tma(dead, &[]).unwrap();
        let unused: Vec<&str> = report
            .iter()
            .filter(|d| d.code == "unused-label")
            .map(|d| d.message.as_str())
            .collect();
        assert_eq!(unused.len(), 1, "exactly one dead label: {report:?}");
        assert!(unused[0].contains("`gone`"), "{}", unused[0]);
    }

    #[test]
    fn an_assemble_fatal_propagates_as_err() {
        let err = lint_tma(".func main\n        bogus\n", &[]).unwrap_err();
        assert!(matches!(
            err.kind,
            mtc_core::asm::AsmErrorKind::UnknownMnemonic(_)
        ));
    }
}
