//! The `.pmc` language service: implements `mtc_core::lsp::LanguageService`
//! over the real compiler pipeline (docs/lsp.md). Owns per-document staged
//! state, the three-channel diagnostic merge (fatal / compile warnings /
//! lint findings), and both configuration channels (`pmt.json` project
//! files and IDE settings). Library-only — rendering and stdio belong to
//! the CLI (docs/cli.md (thin-renderer rule)).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use mtc_core::diagnostics::{Applicability, Diagnostic, Pos, Span};
use mtc_core::lsp::{
    Action, Candidate, DefTarget, LanguageService, SemToken, ServiceDiagnostic, ServiceSeverity,
    SymbolNode, SymbolNodeKind,
};

use crate::compiler::{Analysis, CompileError, ScopeSummary, analyze_staged};
use crate::config;
use crate::cst::{BodyKind, Cst, FunctionCst, TopItem, TopKind};
use crate::lexer::{Token, TokenKind};
use crate::lint::{LintContext, LintError, run_rules, validate_allow};

mod complete;
mod navigate;
mod tokens;

pub(crate) struct PmcLanguageService {
    docs: HashMap<String, DocState>,
    /// IDE-settings allow-list: `None` = never configured; `Ok` = valid
    /// codes; `Err` = human-readable reason (surfaces as invalid-config).
    ide_allow: Option<Result<Vec<String>, String>>,
    /// `pmt.json` parse cache keyed by winner path; (mtime, outcome).
    config_cache: HashMap<PathBuf, (SystemTime, Result<Vec<String>, String>)>,
}

impl PmcLanguageService {
    pub(crate) fn new() -> Self {
        PmcLanguageService {
            docs: HashMap::new(),
            ide_allow: None,
            config_cache: HashMap::new(),
        }
    }

    /// The project-file channel for one analysis: the parsed outcome of
    /// the discovered `pmt.json`, through the mtime cache — reused only
    /// while the file's mtime is unchanged, else re-loaded and re-cached.
    /// Errors come back as the full display string (path + reason), ready
    /// to be an `invalid-config` message.
    fn project_allow(&mut self, winner: &Path) -> Result<Vec<String>, String> {
        let mtime = std::fs::metadata(winner).and_then(|m| m.modified()).ok();
        if let Some(mtime) = mtime
            && let Some((cached, outcome)) = self.config_cache.get(winner)
            && *cached == mtime
        {
            return outcome.clone();
        }
        let outcome = match config::load(winner) {
            Ok(project) => Ok(project.allow),
            Err(e) => Err(e.to_string()),
        };
        if let Some(mtime) = mtime {
            // No stat (file racing in and out of existence) → no cache
            // entry: there is no mtime to key staleness on.
            self.config_cache
                .insert(winner.to_path_buf(), (mtime, outcome.clone()));
        }
        outcome
    }
}

/// Per-document staged state (docs/lsp.md (staged analysis)): each
/// pipeline stage's outcome for the CURRENT text, plus the one sanctioned
/// piece of staleness (`scopes_for_completion`).
struct DocState {
    /// The document's current text, verbatim from the framework.
    text: String,
    /// WithComments token stream of the current text; `None` only when
    /// lexing itself failed.
    tokens: Option<Vec<Token>>,
    /// CST of the current text (`None` when lexing or parsing failed).
    cst: Option<Cst>,
    /// Post-parse analysis of the current text (`None` when any stage
    /// failed).
    analysis: Option<Analysis>,
    /// Lint findings, retained fixes included (the quickfix source);
    /// `Some` exactly when `analysis` is — lint only runs over a
    /// successful analysis.
    lint: Option<Vec<Diagnostic>>,
    /// The first (only) fatal, at whichever stage produced it.
    fatal: Option<CompileError>,
    /// Names-only staleness exception: last-good scopes survive a failed
    /// re-analysis so completion candidates stay useful mid-edit.
    scopes_for_completion: Option<ScopeSummary>,
    /// invalid-config messages that applied to this analysis (0..=2
    /// entries: project file first, then IDE settings).
    config_errors: Vec<String>,
}

/// `file:` URIs → percent-decoded filesystem path; any other scheme
/// (`untitled:` buffers, …) → `None`. An authority component
/// (`file://localhost/x`) is skipped — editors emit the empty-authority
/// `file:///x` form, but the spelled-out host is legal URI syntax.
fn uri_to_path(uri: &str) -> Option<PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    let path = &rest[rest.find('/')?..];
    Some(PathBuf::from(percent_decode(path)?))
}

/// Hand-rolled percent-decoding: `%XX` hex pairs become bytes; malformed
/// escapes pass through literally. `None` only when the decoded bytes are
/// not UTF-8 (no `PathBuf` to build portably).
fn percent_decode(s: &str) -> Option<String> {
    fn hex(b: u8) -> Option<u8> {
        match b {
            b'0'..=b'9' => Some(b - b'0'),
            b'a'..=b'f' => Some(b - b'a' + 10),
            b'A'..=b'F' => Some(b - b'A' + 10),
            _ => None,
        }
    }
    let bytes = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && let (Some(hi), Some(lo)) = (
                bytes.get(i + 1).copied().and_then(hex),
                bytes.get(i + 2).copied().and_then(hex),
            )
        {
            out.push(hi * 16 + lo);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// Set-union append: codes already present are not duplicated (the same
/// contains-check `pmt lint` uses when folding project config into
/// `--allow`).
fn union_into(dst: &mut Vec<String>, src: &[String]) {
    for code in src {
        if !dst.contains(code) {
            dst.push(code.clone());
        }
    }
}

/// Half-open span overlap: `a.start < b.end && b.start < a.end` (`Pos` is
/// `Ord`). Mirrors the fake service's helper in `mtc_core::lsp` — the
/// contract is language-agnostic, only the caller differs.
fn spans_overlap(a: Span, b: Span) -> bool {
    a.start < b.end && b.start < a.end
}

/// Parses the IDE channel's `lint.allow` value: an array of known rule
/// codes, or a human-readable reason why not.
fn parse_ide_allow(value: &serde_json::Value) -> Result<Vec<String>, String> {
    const SHAPE: &str = "`lint.allow` must be an array of strings";
    let arr = value.as_array().ok_or_else(|| SHAPE.to_string())?;
    let mut codes = Vec::with_capacity(arr.len());
    for item in arr {
        codes.push(item.as_str().ok_or_else(|| SHAPE.to_string())?.to_string());
    }
    match validate_allow(&codes) {
        Ok(()) => Ok(codes),
        Err(LintError::UnknownAllowCode(code)) => {
            Err(format!("unknown lint rule `{code}` in lint.allow"))
        }
        Err(other) => unreachable!("validate_allow only returns UnknownAllowCode: {other}"),
    }
}

/// The merged diagnostic set for one document (docs/lsp.md
/// (diagnostics)): invalid-config warnings first, then either the one
/// fatal or the span-ordered merge of compile warnings (source `"pmt"`)
/// and lint findings (source `"pmt lint"`).
fn merged_diagnostics(state: &DocState) -> Vec<ServiceDiagnostic> {
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
        // Exactly one Error, never a cascade. The message is the KIND's
        // Display — the `line N:M:` prefix and bracketed code suffix are
        // CLI renderings; the LSP client places the span and shows the
        // code itself.
        out.push(ServiceDiagnostic {
            span: fatal.span,
            severity: ServiceSeverity::Error,
            source: "pmt",
            code: Some(fatal.kind.code()),
            message: fatal.kind.to_string(),
        });
        return out;
    }

    let mut findings: Vec<ServiceDiagnostic> = Vec::new();
    if let Some(analysis) = &state.analysis {
        findings.extend(analysis.warnings.iter().map(|d| ServiceDiagnostic {
            span: d.span,
            severity: ServiceSeverity::Warning,
            source: "pmt",
            code: Some(d.code),
            message: d.message.clone(),
        }));
    }
    if let Some(lint) = &state.lint {
        findings.extend(lint.iter().map(|d| ServiceDiagnostic {
            span: d.span,
            severity: ServiceSeverity::Warning,
            source: "pmt lint",
            code: Some(d.code),
            message: d.message.clone(),
        }));
    }
    // Stable sort: equal starts keep the warnings-then-lint channel order.
    findings.sort_by_key(|d| d.span.start);
    out.extend(findings);
    out
}

/// Walks one CST item list — the file level or a [`crate::cst::NamespaceCst`]'s
/// own `items` — into document symbols (docs/lsp.md (document symbols),
/// CST-tier). Comments and imports are skipped; a reopened namespace
/// block stays a separate sibling because the CST already keeps it apart
/// (it never merges same-name blocks the way the AST does). Each node's
/// `span`/`selection_span` are the CST's own extent/name spans, copied
/// verbatim.
fn cst_symbols(items: &[TopItem]) -> Vec<SymbolNode> {
    items
        .iter()
        .filter_map(|item| match &item.kind {
            TopKind::Comment(_) | TopKind::Import(_) => None,
            TopKind::Namespace(ns) => Some(SymbolNode {
                name: ns.name.clone(),
                kind: SymbolNodeKind::Namespace,
                span: ns.span,
                selection_span: ns.name_span,
                children: cst_symbols(&ns.items),
            }),
            TopKind::Function(f) => Some(function_symbol(f)),
        })
        .collect()
}

/// One function's symbol (top-level or nested). Children are its
/// `BodyKind::Nested` functions, recursively; labels and statements are
/// never emitted as symbols.
fn function_symbol(f: &FunctionCst) -> SymbolNode {
    SymbolNode {
        name: f.name.clone(),
        kind: SymbolNodeKind::Function,
        span: f.span,
        selection_span: f.name_span,
        children: f
            .body
            .iter()
            .filter_map(|item| match &item.kind {
                BodyKind::Nested(nested) => Some(function_symbol(nested)),
                _ => None,
            })
            .collect(),
    }
}

/// Semantic-token legend indices/bits (Task 12, `tokens.rs`) — the ONLY
/// spellings the emitter uses for legend positions; kept in lockstep
/// with `token_legend()`'s arrays just below by the drift-guard test in
/// `tokens.rs`.
const TOKEN_TYPE_NAMESPACE: u32 = 0;
const TOKEN_TYPE_FUNCTION: u32 = 1;
const TOKEN_TYPE_NUMBER: u32 = 2;
const MODIFIER_DECLARATION: u32 = 1 << 0;
const MODIFIER_DEFAULT_LIBRARY: u32 = 1 << 1;

impl LanguageService for PmcLanguageService {
    fn language_id(&self) -> &'static str {
        "pmc"
    }

    fn trigger_characters(&self) -> &[char] {
        &['@', ':']
    }

    fn token_legend(&self) -> (&'static [&'static str], &'static [&'static str]) {
        (
            &["namespace", "function", "number"],
            &["declaration", "defaultLibrary"],
        )
    }

    fn watched_globs(&self) -> &'static [&'static str] {
        &["**/pmt.json"]
    }

    fn did_update(&mut self, uri: &str, text: &str) -> Vec<ServiceDiagnostic> {
        // 1. Resolve config. The discovery re-runs EVERY analysis (a few
        //    stats) — a newly created nearer pmt.json must win; only the
        //    parse of the winner is cached (by mtime). Idempotent for the
        //    same (uri, text): the framework's config/watched-file
        //    republish sweeps re-invoke this freely.
        let mut config_errors: Vec<String> = Vec::new();
        let mut effective_allow: Vec<String> = Vec::new();

        if let Some(path) = uri_to_path(uri)
            && let Some(winner) = path.parent().and_then(config::discover)
        {
            match self.project_allow(&winner) {
                Ok(codes) => union_into(&mut effective_allow, &codes),
                // ConfigError's Display already names the path.
                Err(message) => config_errors.push(message),
            }
        }
        match &self.ide_allow {
            None => {}
            Some(Ok(codes)) => union_into(&mut effective_allow, codes),
            Some(Err(reason)) => config_errors.push(format!("IDE settings: {reason}")),
        }

        // 2. Staged analysis; lint over a successful analysis only, with
        //    the effective allow union of the valid config sources.
        let staged = analyze_staged(text);
        let lint = match (&staged.tokens, &staged.analysis) {
            (Some(tokens), Some(analysis)) => {
                // Comment trivia filtered out: exactly the
                // WithoutComments stream the lint rules were written
                // against.
                let significant: Vec<Token> = tokens
                    .iter()
                    .filter(|t| !matches!(t.kind, TokenKind::Comment(_)))
                    .cloned()
                    .collect();
                let ctx = LintContext {
                    source: text,
                    tokens: &significant,
                    ast: &analysis.ast,
                    scopes: &analysis.scopes,
                };
                Some(run_rules(&ctx, &effective_allow))
            }
            _ => None,
        };

        // 3. Store the doc state; a failed re-analysis keeps the
        //    previous last-good scopes (the names-only staleness
        //    exception for completion).
        let prev = self.docs.remove(uri);
        let scopes_for_completion = match &staged.analysis {
            Some(analysis) => Some(analysis.scopes.clone()),
            None => prev.and_then(|d| d.scopes_for_completion),
        };
        let state = DocState {
            text: text.to_string(),
            tokens: staged.tokens,
            cst: staged.cst,
            analysis: staged.analysis,
            lint,
            fatal: staged.fatal,
            scopes_for_completion,
            config_errors,
        };
        let diagnostics = merged_diagnostics(&state);
        self.docs.insert(uri.to_string(), state);
        diagnostics
    }

    fn did_close(&mut self, uri: &str) {
        // Drop everything, staleness included; the framework publishes
        // the empty diagnostic set.
        self.docs.remove(uri);
    }

    fn did_change_config(&mut self, settings: serde_json::Value) {
        // Clients that forward whole configuration sections wrap the
        // service's settings under a "pmt" key; unwrap when present.
        let section = settings.get("pmt").unwrap_or(&settings);
        // Only `lint.allow` is ours. Every other key is client-owned
        // (binary path, trace switches, …) and deliberately ignored —
        // strictness belongs to pmt.json. Missing entirely = the channel
        // is unconfigured, not invalid. No republish from here: the
        // framework re-runs did_update on every open doc after this call.
        self.ide_allow = section
            .get("lint")
            .and_then(|lint| lint.get("allow"))
            .map(parse_ide_allow);
    }

    fn completion(&mut self, uri: &str, pos: Pos) -> Vec<Candidate> {
        match self.docs.get(uri) {
            Some(state) => complete::completion(state, pos),
            None => Vec::new(),
        }
    }

    fn definition(&mut self, uri: &str, pos: Pos) -> Option<DefTarget> {
        let state = self.docs.get(uri)?;
        navigate::definition(state, uri, pos)
    }

    fn code_actions(&mut self, uri: &str, span: Span) -> Vec<Action> {
        // Analysis-tier: `lint` is `Some` exactly when analysis succeeded
        // (empty `Vec` otherwise — no lint findings to offer quickfixes
        // for, whether the document is unknown or the analysis fataled).
        let Some(lint) = self.docs.get(uri).and_then(|state| state.lint.as_ref()) else {
            return Vec::new();
        };
        lint.iter()
            .filter_map(|d| {
                let fix = d.fix.as_ref()?;
                if !spans_overlap(d.span, span) {
                    return None;
                }
                Some(Action {
                    title: fix.description.clone(),
                    preferred: matches!(fix.applicability, Applicability::MachineApplicable),
                    edits: fix.edits.clone(),
                })
            })
            .collect()
    }

    fn document_symbols(&mut self, uri: &str) -> Option<Vec<SymbolNode>> {
        // CST-tier: answered as long as parsing succeeded, even if a
        // later stage (duplicate-binding check, `ir::lower`) fatals.
        let state = self.docs.get(uri)?;
        let cst = state.cst.as_ref()?;
        Some(cst_symbols(&cst.items))
    }

    fn semantic_tokens(&mut self, uri: &str) -> Option<Vec<SemToken>> {
        let state = self.docs.get(uri)?;
        tokens::semantic_tokens(state)
    }

    fn format(&mut self, uri: &str) -> Option<String> {
        // The DOCSTORE's text (docs/lsp.md (format seam)): the framework
        // diffs the returned text against exactly what `did_update` last
        // received, never a re-read from disk or a stale revision.
        let state = self.docs.get(uri)?;
        crate::fmt::format(&state.text).ok()
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::Duration;

    use serde_json::json;

    use super::*;

    /// A fresh scratch directory under `std::env::temp_dir()`, unique per
    /// call (process id + an atomic counter — this crate has no tempfile
    /// dependency, matching the zero-new-deps constraint). Mirrors
    /// `config::tests::unique_tmp_dir`.
    fn unique_tmp_dir(label: &str) -> PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("pmt-lsp-test-{label}-{}-{n}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn file_uri(path: &Path) -> String {
        format!("file://{}", path.display())
    }

    /// One `unused-label` lint finding (label 5, never referenced) and
    /// nothing else.
    const UNUSED_LABEL_FIXTURE: &str = "main() {\n5: right;\n}\n";

    /// `unused-label` (line 2, lint channel) AND `unused-import` (line 4,
    /// compile-warning channel) — the channels interleave only if the
    /// merge really sorts by span.
    const WARNING_AND_LINT_FIXTURE: &str = "main() {\n5: right;\n}\nuse unused;\n";

    /// Both `leading-zeros` and `unused-label` fire on the `007:` label.
    const TWO_FINDINGS_FIXTURE: &str = "main() {\n007: right;\n}\n";

    /// Two `namespace a { … }` blocks (a REOPENED namespace, not merged —
    /// the CST keeps reopened blocks as separate siblings) plus a
    /// top-level `main` with a nested `helper` and a LABELED statement
    /// (`5: right;`) — proves labels never become symbols, rather than
    /// vacuously passing because none are present.
    const SYMBOLS_FIXTURE: &str = "namespace a {\n    f() {\n        right;\n    }\n}\n\nnamespace a {\n    g() {\n        right;\n    }\n}\n\nmain() {\n    helper() {\n        right;\n    }\n    5: right;\n}\n";

    /// Valid but not canonically formatted (`right;` isn't indented) — a
    /// clean parse whose `fmt::format` output differs from the input, so
    /// equality with a direct `fmt::format` call isn't vacuous.
    const UNFORMATTED_FIXTURE: &str = "main() {\nright;\n}\n";

    #[test]
    fn advertises_the_pmc_language_surface() {
        let service = PmcLanguageService::new();
        assert_eq!(service.language_id(), "pmc");
        assert_eq!(service.trigger_characters(), &['@', ':']);
        assert_eq!(
            service.token_legend(),
            (
                &["namespace", "function", "number"][..],
                &["declaration", "defaultLibrary"][..]
            )
        );
        assert_eq!(service.watched_globs(), &["**/pmt.json"]);
    }

    #[test]
    fn parse_failure_yields_exactly_one_error_with_its_fatal_code() {
        let mut service = PmcLanguageService::new();
        let diags = service.did_update("untitled:Untitled-1", "main( {");

        assert_eq!(diags.len(), 1, "one honest fatal, never a cascade");
        let d = &diags[0];
        assert_eq!(d.severity, ServiceSeverity::Error);
        assert_eq!(d.source, "pmt");
        assert_eq!(d.code, Some("unexpected-token"));
        // The message is the KIND's own Display — no `line N:M:` prefix,
        // no bracketed code suffix (both are CLI renderings).
        assert_eq!(
            d.message,
            "expected `)` (functions take no parameters), found `{`"
        );
    }

    #[test]
    fn compile_warnings_and_lint_findings_merge_span_ordered() {
        let mut service = PmcLanguageService::new();
        let diags = service.did_update("untitled:Untitled-1", WARNING_AND_LINT_FIXTURE);

        assert_eq!(diags.len(), 2, "{diags:?}");
        // The lint finding (line 2) precedes the compile warning (line 4):
        // the two channels are merged by span, not concatenated.
        assert_eq!(diags[0].code, Some("unused-label"));
        assert_eq!(diags[0].source, "pmt lint");
        assert_eq!(diags[0].severity, ServiceSeverity::Warning);
        assert_eq!(diags[0].span, Span::new(2, 1, 2, 3));
        assert_eq!(
            diags[0].message,
            "label 5 is never referenced (function 'main')"
        );

        assert_eq!(diags[1].code, Some("unused-import"));
        assert_eq!(diags[1].source, "pmt");
        assert_eq!(diags[1].severity, ServiceSeverity::Warning);
        assert_eq!(diags[1].span.start.line, 4);
        assert_eq!(diags[1].message, "unused import `unused`");
    }

    #[test]
    fn undefined_label_is_a_single_char_precise_error() {
        let mut service = PmcLanguageService::new();
        let diags = service.did_update("untitled:Untitled-1", "main() {\nright;\ngoto 99;\n}\n");

        assert_eq!(diags.len(), 1);
        let d = &diags[0];
        assert_eq!(d.severity, ServiceSeverity::Error);
        assert_eq!(d.source, "pmt");
        assert_eq!(d.code, Some("undefined-label"));
        assert_eq!(d.message, "undefined label `99`");
        // Char-precise: exactly the `99` target token, not the whole line.
        assert_eq!(d.span, Span::new(3, 6, 3, 8));
    }

    #[test]
    fn lint_fix_is_retained_in_doc_state() {
        let mut service = PmcLanguageService::new();
        service.did_update("untitled:Untitled-1", UNUSED_LABEL_FIXTURE);

        let state = service.docs.get("untitled:Untitled-1").unwrap();
        let lint = state.lint.as_ref().expect("lint ran (analysis succeeded)");
        assert_eq!(lint.len(), 1);
        assert_eq!(lint[0].code, "unused-label");
        let fix = lint[0]
            .fix
            .as_ref()
            .expect("the finding's Fix is retained for code actions");
        assert_eq!(fix.description, "remove the label prefix '5:'");
    }

    #[test]
    fn code_actions_maps_applicability_to_preferred_and_carries_the_fix() {
        // Both `leading-zeros` (MachineApplicable) and `unused-label`
        // (MaybeIncorrect) fire on the same `007:` label — one request
        // span overlapping both proves the applicability → `preferred`
        // mapping for each tier.
        let mut service = PmcLanguageService::new();
        let uri = "untitled:Untitled-1";
        service.did_update(uri, TWO_FINDINGS_FIXTURE);

        // Overlaps both findings' spans (both start at line 2, col 1).
        let actions = service.code_actions(uri, Span::new(2, 1, 2, 2));
        assert_eq!(actions.len(), 2, "{actions:?}");

        // RULES-table order (leading-zeros before unused-label), stable
        // sort on equal span starts preserves it.
        assert_eq!(actions[0].title, "rewrite '007' as '7'");
        assert!(actions[0].preferred, "leading-zeros is MachineApplicable");
        assert_eq!(
            actions[0].edits,
            vec![mtc_core::diagnostics::Edit {
                span: Span::new(2, 1, 2, 4),
                replacement: "7".to_string(),
            }]
        );

        assert_eq!(actions[1].title, "remove the label prefix '7:'");
        assert!(!actions[1].preferred, "unused-label is MaybeIncorrect");
        assert_eq!(
            actions[1].edits,
            vec![mtc_core::diagnostics::Edit {
                span: Span::new(2, 1, 2, 5),
                replacement: String::new(),
            }]
        );
    }

    #[test]
    fn code_actions_empty_when_the_request_span_does_not_overlap_the_finding() {
        let mut service = PmcLanguageService::new();
        let uri = "untitled:Untitled-1";
        service.did_update(uri, UNUSED_LABEL_FIXTURE);

        // The finding's span is (2,1)-(2,3); this span sits entirely on
        // line 1, so the half-open overlap check fails on both ends.
        let actions = service.code_actions(uri, Span::new(1, 1, 1, 2));
        assert!(actions.is_empty(), "{actions:?}");
    }

    #[test]
    fn code_actions_edits_round_trip_makes_the_finding_disappear() {
        use mtc_core::diagnostics::{Edit, Fix};

        let mut service = PmcLanguageService::new();
        let uri = "untitled:Untitled-1";
        service.did_update(uri, UNUSED_LABEL_FIXTURE);

        let finding_span = Span::new(2, 1, 2, 3);
        let actions = service.code_actions(uri, finding_span);
        assert_eq!(actions.len(), 1, "{actions:?}");
        let action = &actions[0];
        assert_eq!(action.title, "remove the label prefix '5:'");
        assert!(!action.preferred);
        assert_eq!(
            action.edits,
            vec![Edit {
                span: finding_span,
                replacement: String::new(),
            }]
        );

        // Byte-apply the returned edits via the CLI's own fix-application
        // helper (a synthetic Diagnostic wrapping the action's edits —
        // `apply_fixes` only cares that a `Fix` is present).
        let synthetic = Diagnostic {
            code: "unused-label",
            span: finding_span,
            message: String::new(),
            fix: Some(Fix {
                description: action.title.clone(),
                applicability: Applicability::MaybeIncorrect,
                edits: action.edits.clone(),
            }),
        };
        let outcome = crate::lint::apply_fixes(UNUSED_LABEL_FIXTURE, &[synthetic]);
        assert_eq!((outcome.applied, outcome.skipped), (1, 0));
        assert_eq!(outcome.fixed_source, "main() {\n right;\n}\n");

        let diags = service.did_update(uri, &outcome.fixed_source);
        assert!(
            diags.iter().all(|d| d.code != Some("unused-label")),
            "the finding is gone from the re-analyzed source: {diags:?}"
        );
    }

    #[test]
    fn code_actions_empty_when_analysis_failed() {
        // `goto 99` parses fine; it's the undefined-label check well past
        // the CST stage (`ir::lower`) that fatals — a post-parse fatal,
        // so `analysis` (and therefore `lint`) is `None`.
        let mut service = PmcLanguageService::new();
        let uri = "untitled:Untitled-1";
        let diags = service.did_update(uri, "main() {\nright;\ngoto 99;\n}\n");
        assert_eq!(diags.len(), 1, "sanity: the fatal published, {diags:?}");
        assert_eq!(diags[0].code, Some("undefined-label"));

        // A wide span covering the whole document — would overlap a
        // finding if `lint` had run at all.
        let actions = service.code_actions(uri, Span::new(1, 1, 4, 2));
        assert!(actions.is_empty(), "{actions:?}");
    }

    #[test]
    fn scopes_for_completion_survive_a_parse_broken_edit() {
        let mut service = PmcLanguageService::new();
        let clean = service.did_update("untitled:Untitled-1", "main() {\n1: right;\ngoto 1;\n}\n");
        assert!(clean.is_empty(), "{clean:?}");

        let broken = service.did_update("untitled:Untitled-1", "main( {");
        assert_eq!(broken.len(), 1);

        let state = service.docs.get("untitled:Untitled-1").unwrap();
        assert!(
            state.tokens.is_some(),
            "lexing succeeded on the broken text"
        );
        assert!(state.cst.is_none());
        assert!(state.analysis.is_none());
        assert!(state.lint.is_none());
        assert!(state.fatal.is_some());
        let scopes = state
            .scopes_for_completion
            .as_ref()
            .expect("last-good scopes survive the failed re-analysis");
        assert_eq!(scopes.defs[&Vec::<String>::new()]["main"], "main");
    }

    #[test]
    fn did_close_drops_the_state_and_its_staleness() {
        let mut service = PmcLanguageService::new();
        service.did_update("untitled:Untitled-1", UNUSED_LABEL_FIXTURE);
        service.did_close("untitled:Untitled-1");
        assert!(!service.docs.contains_key("untitled:Untitled-1"));

        // Re-opening broken starts from scratch: no scopes leak across
        // the close.
        service.did_update("untitled:Untitled-1", "main( {");
        let state = service.docs.get("untitled:Untitled-1").unwrap();
        assert!(state.scopes_for_completion.is_none());
    }

    #[test]
    fn project_config_suppresses_an_allowed_finding() {
        let dir = unique_tmp_dir("suppress");
        fs::write(
            dir.join("pmt.json"),
            r#"{"lint":{"allow":["unused-label"]}}"#,
        )
        .unwrap();

        let mut service = PmcLanguageService::new();
        let uri = file_uri(&dir.join("prog.pmc"));
        let diags = service.did_update(&uri, UNUSED_LABEL_FIXTURE);
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn rewritten_broken_config_surfaces_invalid_config_and_restores_findings() {
        let dir = unique_tmp_dir("rewrite");
        let config_path = dir.join("pmt.json");
        fs::write(&config_path, r#"{"lint":{"allow":["unused-label"]}}"#).unwrap();

        let mut service = PmcLanguageService::new();
        let uri = file_uri(&dir.join("prog.pmc"));
        assert!(service.did_update(&uri, UNUSED_LABEL_FIXTURE).is_empty());

        // Rewrite with a broken schema and a guaranteed-newer mtime (the
        // filesystem's own timestamp granularity is not to be trusted in
        // a fast test).
        let old_mtime = fs::metadata(&config_path).unwrap().modified().unwrap();
        fs::write(&config_path, r#"{"lints":{}}"#).unwrap();
        fs::File::options()
            .write(true)
            .open(&config_path)
            .unwrap()
            .set_modified(old_mtime + Duration::from_secs(2))
            .unwrap();

        let diags = service.did_update(&uri, UNUSED_LABEL_FIXTURE);
        assert_eq!(diags.len(), 2, "{diags:?}");
        // invalid-config first, at the document anchor position.
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

    #[test]
    fn ide_settings_suppress_findings_bare_or_wrapped() {
        // Bare section, as a client that sends the settings directly.
        let mut bare = PmcLanguageService::new();
        bare.did_change_config(json!({"lint": {"allow": ["unused-label"]}}));
        assert!(
            bare.did_update("untitled:Untitled-1", UNUSED_LABEL_FIXTURE)
                .is_empty()
        );

        // Wrapped under "pmt", as a client that forwards whole sections.
        let mut wrapped = PmcLanguageService::new();
        wrapped.did_change_config(json!({"pmt": {"lint": {"allow": ["unused-label"]}}}));
        assert!(
            wrapped
                .did_update("untitled:Untitled-1", UNUSED_LABEL_FIXTURE)
                .is_empty()
        );
    }

    #[test]
    fn ide_unknown_code_yields_invalid_config_naming_ide_settings() {
        let mut service = PmcLanguageService::new();
        service.did_change_config(json!({"lint": {"allow": ["no-such"]}}));

        let diags = service.did_update("untitled:Untitled-1", UNUSED_LABEL_FIXTURE);
        assert_eq!(diags.len(), 2, "{diags:?}");
        assert_eq!(diags[0].code, Some("invalid-config"));
        assert_eq!(diags[0].severity, ServiceSeverity::Warning);
        assert_eq!(diags[0].span, Span::point(1, 1));
        assert!(
            diags[0].message.starts_with("IDE settings: "),
            "names the IDE channel: {}",
            diags[0].message
        );
        assert!(diags[0].message.contains("no-such"), "{}", diags[0].message);
        // The Err source contributed nothing to the allow union — the
        // finding stays.
        assert_eq!(diags[1].code, Some("unused-label"));
    }

    #[test]
    fn missing_lint_allow_leaves_the_ide_channel_unconfigured() {
        let mut service = PmcLanguageService::new();
        // Other keys are client-owned and ignored; no lint.allow at all
        // means UNCONFIGURED, not invalid.
        service.did_change_config(json!({"pmt": {"path": "/usr/local/bin/pmt"}}));
        assert!(service.ide_allow.is_none());

        let diags = service.did_update("untitled:Untitled-1", UNUSED_LABEL_FIXTURE);
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert_eq!(diags[0].code, Some("unused-label"));
    }

    #[test]
    fn file_and_ide_allow_lists_union() {
        let dir = unique_tmp_dir("union");
        fs::write(
            dir.join("pmt.json"),
            r#"{"lint":{"allow":["unused-label"]}}"#,
        )
        .unwrap();
        let uri = file_uri(&dir.join("prog.pmc"));

        // Control: the file alone suppresses only unused-label.
        let mut file_only = PmcLanguageService::new();
        let diags = file_only.did_update(&uri, TWO_FINDINGS_FIXTURE);
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert_eq!(diags[0].code, Some("leading-zeros"));

        // Union: file allows unused-label, IDE allows leading-zeros.
        let mut union = PmcLanguageService::new();
        union.did_change_config(json!({"lint": {"allow": ["leading-zeros"]}}));
        let diags = union.did_update(&uri, TWO_FINDINGS_FIXTURE);
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn untitled_uri_gets_no_project_config_and_no_error() {
        // No path → no discovery, no invalid-config noise; the finding
        // itself is untouched.
        let mut service = PmcLanguageService::new();
        let diags = service.did_update("untitled:Untitled-1", UNUSED_LABEL_FIXTURE);
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert_eq!(diags[0].code, Some("unused-label"));
        assert!(diags.iter().all(|d| d.code != Some("invalid-config")));
    }

    #[test]
    fn did_update_is_idempotent_for_identical_input() {
        // The framework's config/watched-file republish sweeps re-invoke
        // did_update freely; same (uri, text) must give the same answer.
        let dir = unique_tmp_dir("idempotent");
        fs::write(
            dir.join("pmt.json"),
            r#"{"lint":{"allow":["unused-label"]}}"#,
        )
        .unwrap();
        let uri = file_uri(&dir.join("prog.pmc"));

        let mut service = PmcLanguageService::new();
        let first = service.did_update(&uri, TWO_FINDINGS_FIXTURE);
        let second = service.did_update(&uri, TWO_FINDINGS_FIXTURE);
        assert_eq!(first, second);
    }

    #[test]
    fn uri_to_path_decodes_file_uris_and_rejects_other_schemes() {
        assert_eq!(
            uri_to_path("file:///a%20dir/prog.pmc"),
            Some(PathBuf::from("/a dir/prog.pmc"))
        );
        assert_eq!(
            uri_to_path("file:///caf%C3%A9.pmc"),
            Some(PathBuf::from("/café.pmc"))
        );
        // An authority (host) is skipped, not folded into the path.
        assert_eq!(
            uri_to_path("file://localhost/x.pmc"),
            Some(PathBuf::from("/x.pmc"))
        );
        assert_eq!(uri_to_path("untitled:Untitled-1"), None);
        assert_eq!(uri_to_path("https://example.com/x.pmc"), None);
    }

    #[test]
    fn document_symbols_walks_reopened_namespaces_and_nested_functions() {
        let mut service = PmcLanguageService::new();
        service.did_update("untitled:Untitled-1", SYMBOLS_FIXTURE);

        let symbols = service
            .document_symbols("untitled:Untitled-1")
            .expect("CST present on a clean parse");

        assert_eq!(
            symbols,
            vec![
                SymbolNode {
                    name: "a".to_string(),
                    kind: SymbolNodeKind::Namespace,
                    span: Span::new(1, 1, 5, 2),
                    selection_span: Span::new(1, 11, 1, 12),
                    children: vec![SymbolNode {
                        name: "f".to_string(),
                        kind: SymbolNodeKind::Function,
                        span: Span::new(2, 5, 4, 6),
                        selection_span: Span::new(2, 5, 2, 6),
                        children: vec![],
                    }],
                },
                // The reopened `a` is a SEPARATE sibling, not merged into
                // the first.
                SymbolNode {
                    name: "a".to_string(),
                    kind: SymbolNodeKind::Namespace,
                    span: Span::new(7, 1, 11, 2),
                    selection_span: Span::new(7, 11, 7, 12),
                    children: vec![SymbolNode {
                        name: "g".to_string(),
                        kind: SymbolNodeKind::Function,
                        span: Span::new(8, 5, 10, 6),
                        selection_span: Span::new(8, 5, 8, 6),
                        children: vec![],
                    }],
                },
                SymbolNode {
                    name: "main".to_string(),
                    kind: SymbolNodeKind::Function,
                    span: Span::new(13, 1, 18, 2),
                    selection_span: Span::new(13, 1, 13, 5),
                    // `helper` is a child of `main`, not a sibling; its
                    // own trailing `right;` statement contributes no
                    // symbol (labels/statements are never emitted).
                    children: vec![SymbolNode {
                        name: "helper".to_string(),
                        kind: SymbolNodeKind::Function,
                        span: Span::new(14, 5, 16, 6),
                        selection_span: Span::new(14, 5, 14, 11),
                        children: vec![],
                    }],
                },
            ]
        );
    }

    #[test]
    fn document_symbols_still_answered_on_a_post_parse_fatal() {
        // `goto 99` parses fine (a well-formed statement); it's the
        // undefined-label check in `ir::lower` that fails, well past the
        // CST stage — document_symbols is CST-tier and must not care.
        let mut service = PmcLanguageService::new();
        let diags = service.did_update("untitled:Untitled-1", "main() {\nright;\ngoto 99;\n}\n");
        assert_eq!(diags.len(), 1, "sanity: the fatal published, {diags:?}");
        assert_eq!(diags[0].code, Some("undefined-label"));

        let symbols = service
            .document_symbols("untitled:Untitled-1")
            .expect("CST-tier symbols survive a post-parse fatal");
        assert_eq!(symbols.len(), 1);
        assert_eq!(symbols[0].name, "main");
        assert_eq!(symbols[0].kind, SymbolNodeKind::Function);
        assert!(symbols[0].children.is_empty());
    }

    #[test]
    fn document_symbols_none_when_parsing_failed() {
        let mut service = PmcLanguageService::new();
        service.did_update("untitled:Untitled-1", "main( {");
        assert_eq!(service.document_symbols("untitled:Untitled-1"), None);
    }

    #[test]
    fn format_matches_a_direct_fmt_format_call() {
        let mut service = PmcLanguageService::new();
        service.did_update("untitled:Untitled-1", UNFORMATTED_FIXTURE);

        let via_service = service
            .format("untitled:Untitled-1")
            .expect("valid source formats");
        let direct = crate::fmt::format(UNFORMATTED_FIXTURE).expect("valid source formats");
        assert_eq!(via_service, direct, "the single-source contract");
        // The fixture really was unformatted, so this isn't a vacuous
        // equality check.
        assert_ne!(via_service, UNFORMATTED_FIXTURE);
    }

    #[test]
    fn format_returns_none_on_a_parse_error() {
        let mut service = PmcLanguageService::new();
        service.did_update("untitled:Untitled-1", "main( {");
        assert_eq!(service.format("untitled:Untitled-1"), None);
    }

    #[test]
    fn format_of_already_formatted_source_is_byte_identical() {
        // The framework's empty-edit path: formatting a document already
        // in canonical form must round-trip to the exact same bytes.
        let canonical = crate::fmt::format(UNFORMATTED_FIXTURE).expect("valid source formats");

        let mut service = PmcLanguageService::new();
        service.did_update("untitled:Untitled-1", &canonical);
        let reformatted = service
            .format("untitled:Untitled-1")
            .expect("already-canonical source formats");
        assert_eq!(reformatted, canonical);
    }

    // --- End-to-end scripted session (docs/lsp.md's "Testing" section,
    // service bullet 8): the REAL service driven through core's blocking
    // server loop over in-memory pipes, exactly as `pmt lsp` drives it
    // over stdio — the CLI subcommand (`cli/lsp.rs`) differs only in
    // which reader/writer it hands to `mtc_core::lsp::server::run`. ----

    /// Frames each of `client_messages` (`mtc_core::lsp::transport::
    /// write_message`) into an in-memory buffer, drives the real server
    /// loop against `service`, and decodes every framed response back to
    /// JSON. Mirrors plan 1's `run_session` test helper in
    /// `crates/core/src/lsp/server.rs`, reimplemented locally because
    /// that helper is `#[cfg(test)]`-private to core.
    fn run_session(
        client_messages: &[serde_json::Value],
        service: &mut PmcLanguageService,
    ) -> (Vec<serde_json::Value>, i32) {
        use mtc_core::lsp::transport::{read_message, write_message};

        let mut input = Vec::new();
        for msg in client_messages {
            write_message(&mut input, &msg.to_string()).expect("write into a Vec cannot fail");
        }

        let mut output = Vec::new();
        let mut reader = &input[..];
        let exit_code = mtc_core::lsp::server::run(
            &mut reader,
            &mut output,
            service,
            mtc_core::lsp::server::ServerIdentity {
                name: "pmt lsp",
                version: "0.0.0-test",
            },
        );

        let mut out_reader = &output[..];
        let mut outputs = Vec::new();
        while let Some(payload) = read_message(&mut out_reader).expect("recorded output frames") {
            outputs.push(serde_json::from_str(&payload).expect("recorded output is valid json"));
        }
        (outputs, exit_code)
    }

    #[test]
    fn e2e_scripted_session_through_the_real_server_loop() {
        use mtc_core::diagnostics::{Edit as CoreEdit, Fix};

        // `goto 99` is the first (and only) fatal — `undefined-label` —
        // parse-clean; label `5` and `use unused;` sit dormant behind
        // it, invisible until the fatal is fixed (post-parse analysis
        // never runs while it stands).
        const BAD: &str = "main() {\n5: right;\ngoto 99;\n}\nuse unused;\n";
        // The fatal fixed (the `goto 99;` statement removed): label `5`
        // is now an `unused-label` lint finding and `use unused;` an
        // `unused-import` compile warning — byte-identical to
        // `WARNING_AND_LINT_FIXTURE` by construction.
        const FIXED: &str = WARNING_AND_LINT_FIXTURE;

        let uri = "file:///e2e.pmc";
        let mut service = PmcLanguageService::new();

        fn init(id: i64) -> serde_json::Value {
            json!({"jsonrpc": "2.0", "id": id, "method": "initialize", "params": {}})
        }
        fn open(uri: &str, text: &str) -> serde_json::Value {
            json!({
                "jsonrpc": "2.0",
                "method": "textDocument/didOpen",
                "params": {"textDocument": {"uri": uri, "languageId": "pmc", "version": 1, "text": text}},
            })
        }
        fn change(uri: &str, version: i32, text: &str) -> serde_json::Value {
            json!({
                "jsonrpc": "2.0",
                "method": "textDocument/didChange",
                "params": {"textDocument": {"uri": uri, "version": version}, "contentChanges": [{"text": text}]},
            })
        }

        // initialize -> didOpen the bad file: exactly one fatal,
        // undefined-label, at the `99` token.
        let (outputs, _) = run_session(&[init(1), open(uri, BAD)], &mut service);
        assert_eq!(outputs.len(), 2, "{outputs:?}");
        assert!(outputs[0]["result"]["capabilities"].is_object());
        assert_eq!(outputs[1]["method"], "textDocument/publishDiagnostics");
        let diags = outputs[1]["params"]["diagnostics"].as_array().unwrap();
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert_eq!(diags[0]["code"], json!("undefined-label"));
        assert_eq!(
            diags[0]["range"],
            json!({
                "start": {"line": 2, "character": 5},
                "end": {"line": 2, "character": 7},
            }),
            "char-precise span on the `99` token"
        );

        // didChange fixing the fatal, then a codeAction request on the
        // lint finding's span: warnings + lint both publish, and the
        // one quickfix comes back.
        let (outputs, _) = run_session(
            &[
                init(1),
                open(uri, BAD),
                change(uri, 2, FIXED),
                json!({
                    "jsonrpc": "2.0",
                    "id": 2,
                    "method": "textDocument/codeAction",
                    "params": {
                        "textDocument": {"uri": uri},
                        "range": {
                            "start": {"line": 1, "character": 0},
                            "end": {"line": 1, "character": 2},
                        },
                    },
                }),
            ],
            &mut service,
        );
        assert_eq!(outputs.len(), 4, "{outputs:?}");
        let diags = outputs[2]["params"]["diagnostics"].as_array().unwrap();
        assert_eq!(diags.len(), 2, "{diags:?}");
        assert_eq!(diags[0]["code"], json!("unused-label"));
        assert_eq!(diags[0]["source"], json!("pmt lint"));
        assert_eq!(diags[1]["code"], json!("unused-import"));
        assert_eq!(diags[1]["source"], json!("pmt"));

        // Apply the returned quickfix client-side (byte-for-byte, the
        // way a real client applies a `WorkspaceEdit`).
        let actions: Vec<mtc_core::lsp::types::CodeAction> =
            serde_json::from_value(outputs[3]["result"].clone()).unwrap();
        assert_eq!(actions.len(), 1, "{actions:?}");
        assert_eq!(actions[0].title, "remove the label prefix '5:'");
        let edit = actions[0].edit.changes.get(uri).unwrap()[0].clone();
        let span = mtc_core::lsp::position::range_to_span(FIXED, edit.range.clone());
        let synthetic = Diagnostic {
            code: "unused-label",
            span,
            message: String::new(),
            fix: Some(Fix {
                description: actions[0].title.clone(),
                applicability: Applicability::MaybeIncorrect,
                edits: vec![CoreEdit {
                    span,
                    replacement: edit.new_text.clone(),
                }],
            }),
        };
        let outcome = crate::lint::apply_fixes(FIXED, &[synthetic]);
        assert_eq!((outcome.applied, outcome.skipped), (1, 0));
        let after_quickfix = outcome.fixed_source;
        assert_eq!(after_quickfix, "main() {\n right;\n}\nuse unused;\n");

        // The remaining script in one continuous session: replay up to
        // the quickfix, apply it via didChange, confirm the diagnostics
        // shrink, round-trip formatting, then shut down cleanly.
        let (outputs, exit_code) = run_session(
            &[
                init(1),
                open(uri, BAD),
                change(uri, 2, FIXED),
                change(uri, 3, &after_quickfix),
                json!({
                    "jsonrpc": "2.0",
                    "id": 3,
                    "method": "textDocument/formatting",
                    "params": {"textDocument": {"uri": uri}},
                }),
                json!({"jsonrpc": "2.0", "id": 4, "method": "shutdown", "params": null}),
                json!({"jsonrpc": "2.0", "method": "exit"}),
            ],
            &mut service,
        );

        // init, publish(open), publish(change#1), publish(change#2),
        // formatting response, shutdown response — exit produces none.
        assert_eq!(outputs.len(), 6, "{outputs:?}");

        // Diagnostics shrink: the unused-label finding is gone, only the
        // unused-import compile warning remains.
        let diags = outputs[3]["params"]["diagnostics"].as_array().unwrap();
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert_eq!(diags[0]["code"], json!("unused-import"));

        // Formatting round-trip: one whole-document edit, matching
        // `fmt::format` directly.
        let edits: Vec<mtc_core::lsp::types::TextEdit> =
            serde_json::from_value(outputs[4]["result"].clone()).unwrap();
        assert_eq!(edits.len(), 1, "{edits:?}");
        let canonical = crate::fmt::format(&after_quickfix).expect("valid source formats");
        assert_ne!(
            after_quickfix, canonical,
            "sanity: the fixture really was unformatted"
        );
        assert_eq!(edits[0].new_text, canonical);

        // shutdown -> exit: clean lifecycle exit code.
        assert_eq!(outputs[5]["id"], json!(4));
        assert_eq!(outputs[5]["result"], serde_json::Value::Null);
        assert_eq!(exit_code, 0);
    }

    /// Spec acceptance: opening the embedded stdlib under the server
    /// yields zero diagnostics, a full semantic-token stream, and a
    /// format-no-op — the same fmt-clean/lint-clean dogfood lock
    /// `pmt fmt`/`pmt lint`'s own test suites hold this file to
    /// (`tests/fmt_programs.rs`, `tests/lint_programs.rs`), now proven
    /// through the LSP surface as well.
    #[test]
    fn dogfood_the_embedded_stdlib_is_clean_tokenized_and_format_stable() {
        let mut service = PmcLanguageService::new();
        let uri = "untitled:stdlib-dogfood";

        let diags = service.did_update(uri, crate::stdlib::SOURCE);
        assert!(diags.is_empty(), "{diags:?}");

        let tokens = service.semantic_tokens(uri);
        assert!(matches!(&tokens, Some(t) if !t.is_empty()), "{tokens:?}");

        let formatted = service
            .format(uri)
            .expect("the embedded stdlib parses cleanly");
        assert_eq!(
            formatted,
            crate::stdlib::SOURCE,
            "the embedded stdlib is fmt-clean by construction"
        );
    }
}
