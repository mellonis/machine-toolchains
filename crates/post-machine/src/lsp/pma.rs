//! The `.pma` language service: implements `mtc_core::lsp::LanguageService`
//! over the assembly front end (docs/lsp.md). Mirrors `PmcLanguageService`'s
//! staging (total CST, then a fatal-or-lint split), reusing the shared
//! config-resolution and code-actions machinery from `lsp/mod.rs` rather
//! than duplicating it. Library-only — rendering and stdio belong to the
//! CLI (docs/cli.md (thin-renderer rule)).
//!
//! `.pma` has no separate compile-warning channel the way `.pmc` does:
//! `mtc_core::asm::lint::lint` alone gives both the fatal gate (a lower or
//! assemble failure) and the lint findings, in one call.
//!
//! `completion`/`definition`/`document_symbols`/`semantic_tokens` are
//! stubbed here (`None`/empty) — a later task fills them in over the CST.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::SystemTime;

use mtc_core::asm::cst::{AsmCst, parse_asm_cst};
use mtc_core::asm::{AsmError, format_asm, lint};
use mtc_core::diagnostics::{Diagnostic, Pos, Span};
use mtc_core::lsp::{
    Action, Candidate, DefTarget, LanguageService, SemToken, ServiceDiagnostic, ServiceSeverity,
    SymbolNode,
};

use crate::asm::pm1_syntax;

use super::{ConfigResolver, actions_from_findings, parse_ide_allow};

pub(crate) struct PmaLanguageService {
    docs: HashMap<String, PmaDocState>,
    /// IDE-settings allow-list: `None` = never configured; `Ok` = valid
    /// codes; `Err` = human-readable reason (surfaces as invalid-config).
    ide_allow: Option<Result<Vec<String>, String>>,
    /// `pmt.json` parse cache keyed by winner path; (mtime, outcome).
    config_cache: HashMap<PathBuf, (SystemTime, Result<Vec<String>, String>)>,
}

impl PmaLanguageService {
    #[allow(dead_code)] // consumer: cli/lsp.rs (pma plan 3, Task 5)
    pub(crate) fn new() -> Self {
        PmaLanguageService {
            docs: HashMap::new(),
            ide_allow: None,
            config_cache: HashMap::new(),
        }
    }
}

/// Per-document staged state, mirroring `.pmc`'s `DocState` for the
/// simpler `.pma` pipeline: no separate compile-warning channel, so one
/// `lint::lint` call settles either the fatal (lower/assemble failure)
/// or the lint findings — never both.
struct PmaDocState {
    /// The document's current text, verbatim from the framework.
    text: String,
    /// Total: every text parses into a CST (docs/formats.md (assembly
    /// text)) — Raw items mark the lines that are not assembly-shaped.
    #[allow(dead_code)] // consumer: document_symbols()/semantic_tokens() (pma plan 3, Task 4)
    cst: AsmCst,
    /// The one fatal, when `lower`/`assemble` refused the file.
    fatal: Option<AsmError>,
    /// Lint findings, retained fixes included (the quickfix source);
    /// `Some` exactly when `fatal` is `None`.
    lint: Option<Vec<Diagnostic>>,
    /// invalid-config messages that applied to this analysis (0..=2
    /// entries: project file first, then IDE settings).
    config_errors: Vec<String>,
}

/// The merged diagnostic set for one document (docs/lsp.md
/// (diagnostics)): invalid-config warnings first, then either the one
/// fatal or the lint findings. Unlike `.pmc`'s `merged_diagnostics`,
/// there is no second channel to span-sort against — `.pma` has no
/// compile-warning channel, and `lint::lint` already returns its
/// findings span-sorted.
fn merged_diagnostics(state: &PmaDocState) -> Vec<ServiceDiagnostic> {
    let mut out: Vec<ServiceDiagnostic> = state
        .config_errors
        .iter()
        .map(|message| ServiceDiagnostic {
            span: Span::point(1, 1),
            severity: ServiceSeverity::Warning,
            source: "pmt",
            code: Some("invalid-config"),
            message: message.clone(),
        })
        .collect();

    if let Some(fatal) = &state.fatal {
        // Exactly one Error, never a cascade — mirrors `.pmc`'s fatal
        // rendering: the message is the KIND's own Display (no `line
        // N:M:` prefix, no bracketed code suffix — both are CLI
        // renderings; the LSP client places the span and shows the
        // code itself).
        out.push(ServiceDiagnostic {
            span: fatal.span,
            severity: ServiceSeverity::Error,
            source: "pmt",
            code: Some(fatal.kind.code()),
            message: fatal.kind.to_string(),
        });
        return out;
    }

    out.extend(state.lint.iter().flatten().map(|d| ServiceDiagnostic {
        span: d.span,
        severity: ServiceSeverity::Warning,
        source: "pmt lint",
        code: Some(d.code),
        message: d.message.clone(),
    }));
    out
}

impl LanguageService for PmaLanguageService {
    fn language_id(&self) -> &'static str {
        "pma"
    }

    fn extensions(&self) -> &'static [&'static str] {
        &[".pma"]
    }

    fn trigger_characters(&self) -> &[char] {
        &['@', '.']
    }

    fn token_legend(&self) -> (&'static [&'static str], &'static [&'static str]) {
        // Labels ride "variable" with "declaration" on definitions
        // (docs/lsp.md (semantic tokens)); mnemonics stay TextMate's job.
        (
            &["function", "variable", "number"],
            &["declaration", "defaultLibrary"],
        )
    }

    fn watched_globs(&self) -> &'static [&'static str] {
        &["**/pmt.json"]
    }

    fn did_update(&mut self, uri: &str, text: &str) -> Vec<ServiceDiagnostic> {
        // 1. Resolve config — shared machinery (docs/lsp.md (config
        //    channels)), identical union semantics to `.pmc`.
        let (effective_allow, config_errors) = ConfigResolver {
            ide_allow: &self.ide_allow,
            config_cache: &mut self.config_cache,
        }
        .resolve(uri);

        // 2. Total CST, always. One `lint::lint` call gives the fatal
        //    gate (lower/assemble failure) AND the lint findings in one
        //    shot — `.pma` has no separate compile-warning channel.
        let cst = parse_asm_cst(text);
        let syntax = pm1_syntax();
        let (fatal, lint_findings) = match lint::lint(&syntax, text, &effective_allow) {
            Ok(findings) => (None, Some(findings)),
            Err(e) => (Some(e), None),
        };

        let state = PmaDocState {
            text: text.to_string(),
            cst,
            fatal,
            lint: lint_findings,
            config_errors,
        };
        let diagnostics = merged_diagnostics(&state);
        self.docs.insert(uri.to_string(), state);
        diagnostics
    }

    fn did_close(&mut self, uri: &str) {
        // Drop everything; the framework publishes the empty diagnostic
        // set.
        self.docs.remove(uri);
    }

    fn did_change_config(&mut self, settings: serde_json::Value) {
        // Mirrors `.pmc`'s channel: clients that forward whole
        // configuration sections wrap the service's settings under a
        // "pmt" key; unwrap when present. Only `lint.allow` is ours —
        // every other key is client-owned and ignored. Missing entirely
        // means the channel is unconfigured, not invalid.
        let section = settings.get("pmt").unwrap_or(&settings);
        self.ide_allow = section
            .get("lint")
            .and_then(|lint| lint.get("allow"))
            .map(parse_ide_allow);
    }

    fn completion(&mut self, _uri: &str, _pos: Pos) -> Vec<Candidate> {
        Vec::new()
    }

    fn definition(&mut self, _uri: &str, _pos: Pos) -> Option<DefTarget> {
        None
    }

    fn code_actions(&mut self, uri: &str, span: Span) -> Vec<Action> {
        // Lint-tier: `lint` is `Some` exactly when the file assembled
        // cleanly (empty `Vec` otherwise — no lint findings to offer
        // quickfixes for, whether the document is unknown or fataled).
        let Some(lint) = self.docs.get(uri).and_then(|state| state.lint.as_ref()) else {
            return Vec::new();
        };
        actions_from_findings(lint, span)
    }

    fn document_symbols(&mut self, _uri: &str) -> Option<Vec<SymbolNode>> {
        None
    }

    fn semantic_tokens(&mut self, _uri: &str) -> Option<Vec<SemToken>> {
        None
    }

    fn format(&mut self, uri: &str) -> Option<String> {
        // The DOCSTORE's text (docs/lsp.md (format seam)): same
        // single-source contract as `.pmc`.
        let state = self.docs.get(uri)?;
        format_asm(&state.text).ok()
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;
    use std::sync::atomic::{AtomicU64, Ordering};

    use serde_json::json;

    use mtc_core::diagnostics::Edit;

    use super::*;

    /// A fresh scratch directory under `std::env::temp_dir()`, unique per
    /// call. This crate has no shared test-support module (each file
    /// defines its own local helpers) — mirrors `PmcLanguageService`'s
    /// own `unique_tmp_dir`.
    fn unique_tmp_dir(label: &str) -> PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "pmt-lsp-pma-test-{label}-{}-{n}",
            std::process::id()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn file_uri(path: &Path) -> String {
        format!("file://{}", path.display())
    }

    /// A clean, well-formed program: no findings on any channel.
    const CLEAN_FIXTURE: &str = ".func f\n        stp\n";

    /// One `unused-label` finding (label `UNUSED`, never referenced) and
    /// nothing else.
    const UNUSED_LABEL_FIXTURE: &str = ".func f\nUNUSED: nop\n        stp\n";

    /// A listing-shaped line (not assembly text) — `lower`'s Raw check
    /// fires unconditionally, before any function-open check, so this is
    /// a fatal on its own with no `.func` needed.
    const LISTING_FIXTURE: &str = "<stray>\n";

    /// Valid but not canonically gridded — a spaced colon and ragged
    /// indentation `format_asm` normalizes.
    const SCRAMBLED_FIXTURE: &str = ".func f\nL1 :  rgt\n stp\n";

    #[test]
    fn advertises_the_pma_language_surface() {
        let service = PmaLanguageService::new();
        assert_eq!(service.language_id(), "pma");
        assert_eq!(service.extensions(), &[".pma"]);
        assert_eq!(service.trigger_characters(), &['@', '.']);
        assert_eq!(
            service.token_legend(),
            (
                &["function", "variable", "number"][..],
                &["declaration", "defaultLibrary"][..]
            )
        );
        assert_eq!(service.watched_globs(), &["**/pmt.json"]);
    }

    #[test]
    fn clean_program_yields_no_diagnostics() {
        let mut service = PmaLanguageService::new();
        let diags = service.did_update("untitled:Untitled-1", CLEAN_FIXTURE);
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn unknown_mnemonic_yields_one_error_with_its_word_span() {
        let mut service = PmaLanguageService::new();
        let diags = service.did_update("untitled:Untitled-1", ".func f\n        bogus\n");

        assert_eq!(diags.len(), 1, "{diags:?}");
        let d = &diags[0];
        assert_eq!(d.severity, ServiceSeverity::Error);
        assert_eq!(d.source, "pmt");
        assert_eq!(d.code, Some("unknown-mnemonic"));
        assert_eq!(d.message, "unknown mnemonic `bogus`");
        assert_eq!(d.span, Span::new(2, 9, 2, 14)); // the `bogus` word
    }

    #[test]
    fn listing_shaped_line_is_a_raw_line_error() {
        let mut service = PmaLanguageService::new();
        let diags = service.did_update("untitled:Untitled-1", LISTING_FIXTURE);

        assert_eq!(diags.len(), 1, "{diags:?}");
        let d = &diags[0];
        assert_eq!(d.severity, ServiceSeverity::Error);
        assert_eq!(d.source, "pmt");
        assert_eq!(d.code, Some("raw-line"));
        assert_eq!(d.span, Span::new(1, 1, 1, 8)); // `<stray>` is 7 chars
    }

    #[test]
    fn unused_label_is_a_pmt_lint_warning_with_a_quickfix() {
        let mut service = PmaLanguageService::new();
        let uri = "untitled:Untitled-1";
        let diags = service.did_update(uri, UNUSED_LABEL_FIXTURE);

        assert_eq!(diags.len(), 1, "{diags:?}");
        let d = &diags[0];
        assert_eq!(d.severity, ServiceSeverity::Warning);
        assert_eq!(d.source, "pmt lint");
        assert_eq!(d.code, Some("unused-label"));
        assert_eq!(d.span, Span::new(2, 1, 2, 7));
        assert_eq!(
            d.message,
            "label `UNUSED` is never referenced (function `f`)"
        );

        // The fix surfaces via code_actions at that finding's span.
        let actions = service.code_actions(uri, d.span);
        assert_eq!(actions.len(), 1, "{actions:?}");
        let action = &actions[0];
        assert_eq!(action.title, "remove the unused label");
        assert!(action.preferred, "the fix is MachineApplicable");
        assert_eq!(
            action.edits,
            vec![Edit {
                span: Span::new(2, 1, 2, 9),
                replacement: String::new(),
            }]
        );
    }

    #[test]
    fn ide_config_allow_suppresses_the_finding() {
        let mut service = PmaLanguageService::new();
        service.did_change_config(json!({"lint": {"allow": ["unused-label"]}}));

        let diags = service.did_update("untitled:Untitled-1", UNUSED_LABEL_FIXTURE);
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn format_grids_a_scrambled_doc_and_is_none_on_a_listing_doc() {
        let mut service = PmaLanguageService::new();

        service.did_update("untitled:scrambled", SCRAMBLED_FIXTURE);
        let via_service = service
            .format("untitled:scrambled")
            .expect("valid source formats");
        let direct = format_asm(SCRAMBLED_FIXTURE).expect("valid source formats");
        assert_eq!(via_service, direct, "the single-source contract");
        assert_ne!(via_service, SCRAMBLED_FIXTURE, "sanity: really scrambled");

        service.did_update("untitled:listing", LISTING_FIXTURE);
        assert_eq!(service.format("untitled:listing"), None);
    }

    #[test]
    fn invalid_pmt_json_surfaces_invalid_config_first() {
        let dir = unique_tmp_dir("invalid-config");
        let config_path = dir.join("pmt.json");
        fs::write(&config_path, r#"{"lints":{}}"#).unwrap();

        let mut service = PmaLanguageService::new();
        let uri = file_uri(&dir.join("prog.pma"));
        let diags = service.did_update(&uri, UNUSED_LABEL_FIXTURE);

        assert_eq!(diags.len(), 2, "{diags:?}");
        assert_eq!(diags[0].code, Some("invalid-config"));
        assert_eq!(diags[0].severity, ServiceSeverity::Warning);
        assert_eq!(diags[0].source, "pmt");
        assert_eq!(diags[0].span, Span::point(1, 1));
        assert!(
            diags[0]
                .message
                .contains(&config_path.display().to_string()),
            "names the pmt.json at fault: {}",
            diags[0].message
        );
        assert!(
            diags[0].message.contains("unknown key `lints`"),
            "carries the reason: {}",
            diags[0].message
        );
        // The finding is back: lint ran with the remaining sources.
        assert_eq!(diags[1].code, Some("unused-label"));
        assert_eq!(diags[1].source, "pmt lint");
    }
}
