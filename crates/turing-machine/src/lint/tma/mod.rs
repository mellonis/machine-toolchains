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
//! # unused-label is suppressed on the `.tma` path
//!
//! `unused-label` is core's existing arch-agnostic rule, not reimplemented
//! here — but it is **structurally unreliable on `.tma`** and is therefore
//! suppressed on this path (by injecting its code into the allow list handed
//! to core). Core's rule counts only in-function jump/call name operands as
//! references; it cannot see a code label reached through a `.targets` /
//! `.target` dispatch entry or listed in a `.exits` frame descriptor, because
//! those references live in the lowered table section, which core's
//! `AsmLintContext` does not expose. On any dispatch-table program every
//! djmp/exit target would be flagged as unused (the flagship brainfuck UTM
//! trips ~400 such false findings, all reachable code). A CST-level filter
//! cannot rescue it: core lowers and expands `.rept` before flagging, so a
//! finding names the expanded label (`Linc0`..`Linc126`) while the CST
//! carries only the verbatim template (`Linc{v}`) — matching them would mean
//! reimplementing core's substitution evaluator. The code stays in the shared
//! allow namespace (core's `RULES`), so allow-validation is unaffected; a
//! maintainer wanting the check back once core can surface the lowered
//! table/exit references drops the injection. The three additions below are
//! the defects core genuinely cannot detect.
//!
//! # The allow namespace
//!
//! The three TM codes join the crate's shared allow namespace via
//! [`super::known_code`] (one more union arm over [`TMA_RULES`]), so a single
//! `tmt.json` `lint.allow` serves both languages: a `.tma`-only code does not
//! error when validated for a `.tmc` file, and vice versa. There is no
//! `--warn` opt-in tier on the `.tma` side — all three additions are
//! default-on.
//!
//! # A known gap this layer should close
//!
//! A `.map` clause that repeats a source symbol (`rmap=(1->2, 1->3)`) is
//! silently accepted by the assembler, last write winning — the emitted
//! object is identical to the one `1->3` alone produces. That is worth a
//! finding, and it belongs here rather than in the language server: a rule
//! added to this layer reaches both `tmt lint` and the editor through the
//! one `lint_tma` call, whereas a server-only check would raise a
//! diagnostic in the editor that the command line never reports.

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
];

/// Lint one `.tma` source: core's five rules (with the fatal gate — a full
/// assemble refuses structural and semantic errors alike) plus the TM
/// additions, merged and source-ordered. `Err` is the assemble fatal, which
/// the CLI renders as a per-file error (the batch continues), mirroring the
/// `.pma` route. Does NOT validate `allow` codes — the driver owns that over
/// the shared cross-language namespace, same as core's `lint`.
pub fn lint_tma(source: &str, allow: &[String]) -> Result<Vec<Diagnostic>, AsmError> {
    let syntax = tm1_syntax();
    // Suppress core's `unused-label` on this path — it cannot see `.targets`/
    // `.target`/`.exits` label references and so false-flags every dispatch
    // and exit target (module doc, "unused-label is suppressed on the .tma
    // path"). The code remains a valid shared-namespace allow code.
    let mut core_allow = allow.to_vec();
    if !core_allow.iter().any(|a| a == "unused-label") {
        core_allow.push("unused-label".to_string());
    }
    let mut diagnostics = mtc_core::asm::lint::lint(&syntax, source, &core_allow)?;
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
        // CLEAN has a dispatch table (djmp → hit/miss); with `unused-label`
        // suppressed on this path and no TM addition tripping, the report is
        // empty — which also proves the suppression (hit/miss would otherwise
        // be flagged).
        let report = lint_tma(CLEAN, &[]).unwrap();
        assert!(report.is_empty(), "{report:?}");
    }

    #[test]
    fn a_core_rule_fires_through_the_merged_entry() {
        // Dead code after `stp` is core's `unreachable-code` rule — proves the
        // core call is wired into the merged report. (`unused-label` can no
        // longer serve as the witness — it is suppressed on the `.tma` path.)
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
    fn unused_label_is_suppressed_on_the_tma_path() {
        // Every code label here is reached only through a `.targets` dispatch
        // entry or a `.exits` frame descriptor — references core's rule
        // cannot see. It must NOT flag any of them (module doc, the core-gap
        // decision).
        let src = "\
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
        let report = lint_tma(src, &[]).unwrap();
        assert!(
            report.iter().all(|d| d.code != "unused-label"),
            "unused-label must be suppressed on .tma: {report:?}"
        );
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
