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
//! `completion`/`definition`/`document_symbols`/`semantic_tokens` all read
//! straight off the total `AsmCst` (`complete.rs`/`navigate.rs`/`tokens.rs`;
//! `document_symbols` stays inline here, mirroring the `.pmc` service's own
//! placement) — never gated on `fatal`/`lint`, so every one of them still
//! answers over a document that fails to assemble (docs/lsp.md; total CST).
//! `AsmCst` is flat (no per-function nesting the way `.pmc`'s `Cst` has,
//! and `AsmComment` alone carries no line of its own) — [`item_lines`] and
//! [`enclosing_function_range`] below recover the per-line/per-function
//! structure every feature module needs from that flat shape.

use std::collections::{BTreeSet, HashMap};
use std::ops::Range;
use std::path::PathBuf;
use std::time::SystemTime;

use mtc_core::asm::cst::{AsmCst, AsmItem, AsmItemKind, FuncCst, OperandToken, parse_asm_cst};
use mtc_core::asm::{AsmError, Flow, SyntaxEntry, format_asm, lint};
use mtc_core::diagnostics::{Diagnostic, Pos, Span};
use mtc_core::lsp::{
    Action, Candidate, DefTarget, LanguageService, SemToken, ServiceDiagnostic, ServiceSeverity,
    SymbolNode, SymbolNodeKind,
};
use mtc_core::vm::OperandKind;

use crate::asm::pm1_syntax;

use super::{ConfigResolver, actions_from_findings, parse_ide_allow};

mod complete;
mod navigate;
mod tokens;

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
        // Lint-tier: `lint` is `Some` exactly when the file assembled
        // cleanly (empty `Vec` otherwise — no lint findings to offer
        // quickfixes for, whether the document is unknown or fataled).
        let Some(lint) = self.docs.get(uri).and_then(|state| state.lint.as_ref()) else {
            return Vec::new();
        };
        actions_from_findings(lint, span)
    }

    fn document_symbols(&mut self, uri: &str) -> Option<Vec<SymbolNode>> {
        // CST-tier (total): answered for any known document, broken or
        // not — `.pma` has no post-CST analysis stage to gate on.
        let state = self.docs.get(uri)?;
        Some(document_symbols(&state.text, &state.cst))
    }

    fn semantic_tokens(&mut self, uri: &str) -> Option<Vec<SemToken>> {
        let state = self.docs.get(uri)?;
        Some(tokens::semantic_tokens(state))
    }

    fn format(&mut self, uri: &str) -> Option<String> {
        // The DOCSTORE's text (docs/lsp.md (format seam)): same
        // single-source contract as `.pmc`.
        let state = self.docs.get(uri)?;
        format_asm(&state.text).ok()
    }
}

/// Semantic-token legend indices/bits (`tokens.rs`) — the ONLY spellings
/// the emitter uses for legend positions; kept in lockstep with
/// `token_legend()`'s arrays above. Distinct from `.pmc`'s own constants
/// of the same name (that legend orders `function` at index 1; this one
/// at index 0) — each service's constants stay local to its own module,
/// never shared across the two languages.
const TOKEN_TYPE_FUNCTION: u32 = 0;
const TOKEN_TYPE_VARIABLE: u32 = 1;
const TOKEN_TYPE_NUMBER: u32 = 2;
const MODIFIER_DECLARATION: u32 = 1 << 0;
// `defaultLibrary` sits in the legend for cross-service symmetry with
// `.pmc` (docs/lsp.md (semantic tokens)) but `.pma` has no stdlib-call
// notion of its own — `tokens.rs` never emits this bit.
#[allow(dead_code)]
const MODIFIER_DEFAULT_LIBRARY: u32 = 1 << 1;

/// One CST item paired with its 1-based source line. `AsmCst` is flat
/// (docs/formats.md (assembly text)) and every item but `Comment` already
/// carries its own line inside a `Span` — `AsmComment` is the one shape
/// with no line of its own (just `col`). Recovered instead by zipping
/// `cst.items` against the source's own non-blank lines, in order: exactly
/// one item per non-blank line is `parse_asm_cst`'s own invariant
/// (enforced by `cst.rs`'s `total_and_every_nonblank_line_becomes_an_item`
/// proptest), so the two sequences always line up.
fn item_lines(text: &str, cst: &AsmCst) -> Vec<u32> {
    let lines: Vec<u32> = text
        .lines()
        .enumerate()
        .filter(|(_, line)| line.chars().any(|c| c != ' ' && c != '\t'))
        .map(|(i, _)| i as u32 + 1)
        .collect();
    debug_assert_eq!(
        lines.len(),
        cst.items.len(),
        "parse_asm_cst's own invariant: one item per non-blank line"
    );
    lines
}

/// The item at source `line`, if any (`None` on a blank line, a line past
/// the end of the document, or — since `Comment` carries no line of its
/// own and is matched by position here rather than content — a line that
/// turned out to hold nothing else). `lines` is `item_lines`'s parallel
/// per-item line vector.
fn item_at_line<'a>(cst: &'a AsmCst, lines: &[u32], line: u32) -> Option<&'a AsmItem> {
    cst.items
        .iter()
        .zip(lines)
        .find(|&(_, &l)| l == line)
        .map(|(item, _)| item)
}

/// The `.func` item enclosing source `line`, plus the half-open index
/// range of `cst.items` holding its own body — the items between it and
/// the next `Func` (exclusive of both). The flat `AsmCst` carries no
/// per-function grouping of its own; this recovers it by walking to the
/// LAST `Func` item whose own line is `<= line` (items arrive in source
/// order, so once a `Func`'s line exceeds the target, every later one
/// does too). `line` past the last function in the document still
/// resolves to that trailing function — there is no next `.func` to have
/// left it for.
fn enclosing_function_range<'a>(
    cst: &'a AsmCst,
    lines: &[u32],
    line: u32,
) -> Option<(&'a FuncCst, Range<usize>)> {
    let mut found: Option<(&FuncCst, usize)> = None;
    for (i, item) in cst.items.iter().enumerate() {
        if lines[i] > line {
            break;
        }
        if let AsmItemKind::Func(f) = &item.kind {
            found = Some((f, i));
        }
    }
    let (f, idx) = found?;
    let end = cst.items[idx + 1..]
        .iter()
        .position(|it| matches!(it.kind, AsmItemKind::Func(_)))
        .map_or(cst.items.len(), |rel| idx + 1 + rel);
    Some((f, idx + 1..end))
}

/// Every `.func` name declared anywhere in the document (exported and
/// local alike — visibility narrows what a DIFFERENT file may reference,
/// not what this file's own editor tooling should offer/highlight over
/// it), sorted and deduplicated.
fn doc_function_names(cst: &AsmCst) -> BTreeSet<&str> {
    doc_functions(cst).map(|f| f.name.as_str()).collect()
}

/// Every `FuncCst` declared anywhere in the document, in source order.
fn doc_functions(cst: &AsmCst) -> impl Iterator<Item = &FuncCst> {
    cst.items.iter().filter_map(|item| match &item.kind {
        AsmItemKind::Func(f) => Some(f),
        _ => None,
    })
}

/// Which reference kind an operand's raw, trimmed `text` plays for a
/// resolved mnemonic `entry`, per docs/formats.md (assembly text) (symbol
/// jumps): `@name` is always a function-symbol reference (only the
/// RelI8/RelI32 operand kinds `jmp`/`jm`/`jnm`/`call` and their short
/// forms share); a bare (non-`@`) name is a LABEL for `Jump`/`Branch` flow
/// and a FUNCTION for `Call` flow — `call`'s bare operand already IS the
/// symbol, unprefixed ("call operands are already symbols; drop the `@`",
/// docs/formats.md). `None` for any other operand kind (`.byte`'s raw
/// byte, `wr`'s `SymbolVec`) — neither ever references a label or
/// function, and for an unknown mnemonic (no `entry` to classify against
/// at all, the caller never reaches this function).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OperandRole {
    Label,
    Function,
}

fn operand_role(entry: &SyntaxEntry, text: &str) -> Option<OperandRole> {
    if !matches!(entry.operand, OperandKind::RelI8 | OperandKind::RelI32) {
        return None;
    }
    if text.starts_with('@') {
        return Some(OperandRole::Function);
    }
    match entry.flow {
        Flow::Jump | Flow::Branch => Some(OperandRole::Label),
        Flow::Call => Some(OperandRole::Function),
        _ => None,
    }
}

/// An operand's own NAME span — `operand.span` itself for a bare name,
/// or `operand.span` minus its leading `@` for a symbol reference. Every
/// consumer of a [`OperandRole::Function`] operand (completion's replace
/// span, a semantic token, a definition's origin span) points at the
/// name, never the sigil — mirroring `.pmc`'s own call-site spans, which
/// likewise never include its `@` trigger character.
fn name_span(operand: &OperandToken) -> Span {
    if operand.text.starts_with('@') {
        Span::new(
            operand.span.start.line,
            operand.span.start.col + 1,
            operand.span.end.line,
            operand.span.end.col,
        )
    } else {
        operand.span
    }
}

/// Document symbols (docs/lsp.md (document symbols), CST-tier): one
/// [`SymbolNode`] per `.func` item — kind `Function`, span from the
/// `.func` line through the last item before the next `Func` (or the
/// document's end) — with that function's own labels as `Function`
/// children (core's `SymbolNodeKind` has no separate `Label` variant;
/// reusing `Function` for both is the accepted mapping, not a widening of
/// the enum). Total over the CST: answered even when the document does
/// not assemble.
fn document_symbols(text: &str, cst: &AsmCst) -> Vec<SymbolNode> {
    let lines = item_lines(text, cst);
    let mut out = Vec::new();
    let mut i = 0;
    while i < cst.items.len() {
        let AsmItemKind::Func(f) = &cst.items[i].kind else {
            i += 1;
            continue;
        };
        let end = cst.items[i + 1..]
            .iter()
            .position(|it| matches!(it.kind, AsmItemKind::Func(_)))
            .map_or(cst.items.len(), |rel| i + 1 + rel);
        let last_end = if end > i + 1 {
            item_end_pos(&cst.items[end - 1], lines[end - 1])
        } else {
            f.span.end
        };
        let children = cst.items[i + 1..end]
            .iter()
            .filter_map(|it| match &it.kind {
                AsmItemKind::Line(l) => Some(&l.labels),
                _ => None,
            })
            .flatten()
            .map(|label| SymbolNode {
                name: label.name.clone(),
                kind: SymbolNodeKind::Function,
                span: label.span,
                selection_span: label.span,
                children: Vec::new(),
            })
            .collect();
        out.push(SymbolNode {
            name: f.name.clone(),
            kind: SymbolNodeKind::Function,
            span: Span::new(
                f.span.start.line,
                f.span.start.col,
                last_end.line,
                last_end.col,
            ),
            selection_span: f.name_span,
            children,
        });
        i = end;
    }
    out
}

/// An item's own end position — every variant but `Comment` already
/// carries one in its own `Span`; `Comment` is reconstructed from its
/// `col` plus its own character length, paired with the `line` the
/// caller already recovered via [`item_lines`].
fn item_end_pos(item: &AsmItem, line: u32) -> Pos {
    match &item.kind {
        AsmItemKind::Func(f) => f.span.end,
        AsmItemKind::Line(l) => l.span.end,
        AsmItemKind::Raw(r) => r.span.end,
        AsmItemKind::Comment(c) => Pos {
            line,
            col: c.col + c.text.chars().count() as u32,
        },
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

    /// One unknown mnemonic (`bogus`) inside an otherwise open function —
    /// a fatal (Task 4's "total CST" fixture: parsing still succeeds, so
    /// CST-tier features answer regardless).
    const UNKNOWN_MNEMONIC_FIXTURE: &str = ".func f\n        bogus\n";

    /// Two functions, `f` (exported) then `g` (local), each declaring its
    /// own `L1` label — same name in both, proving per-function grouping
    /// rather than a doc-wide label list. `f`'s `L1` is referenced (`jm
    /// L1`); `g`'s is not — irrelevant here (`document_symbols` is
    /// CST-tier, never gated on lint).
    const SYMBOLS_FIXTURE: &str = ".func f\nL1: rgt\njm L1\nret\n.func g local\nL1: nop\nret\n";

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
        let diags = service.did_update("untitled:Untitled-1", UNKNOWN_MNEMONIC_FIXTURE);

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

    #[test]
    fn document_symbols_is_functions_with_their_own_labels_as_children() {
        let mut service = PmaLanguageService::new();
        service.did_update("untitled:Untitled-1", SYMBOLS_FIXTURE);

        let symbols = service
            .document_symbols("untitled:Untitled-1")
            .expect("CST present on any known document");

        assert_eq!(
            symbols,
            vec![
                SymbolNode {
                    name: "f".to_string(),
                    kind: SymbolNodeKind::Function,
                    span: Span::new(1, 1, 4, 4), // `.func f` through `f`'s `ret`
                    selection_span: Span::new(1, 7, 1, 8),
                    children: vec![SymbolNode {
                        name: "L1".to_string(),
                        kind: SymbolNodeKind::Function,
                        span: Span::new(2, 1, 2, 3),
                        selection_span: Span::new(2, 1, 2, 3),
                        children: vec![],
                    }],
                },
                SymbolNode {
                    name: "g".to_string(),
                    kind: SymbolNodeKind::Function,
                    span: Span::new(5, 1, 7, 4), // `.func g local` through `g`'s `ret`
                    selection_span: Span::new(5, 7, 5, 8),
                    children: vec![SymbolNode {
                        name: "L1".to_string(),
                        kind: SymbolNodeKind::Function,
                        span: Span::new(6, 1, 6, 3),
                        selection_span: Span::new(6, 1, 6, 3),
                        children: vec![],
                    }],
                },
            ]
        );
    }

    #[test]
    fn document_symbols_still_answered_on_a_document_that_fails_to_assemble() {
        // `bogus` is an unknown mnemonic — `did_update` publishes a
        // fatal — but `parse_asm_cst` is total: `f` still shows up with
        // its full extent and no children (no labels on the broken line).
        let mut service = PmaLanguageService::new();
        let diags = service.did_update("untitled:Untitled-1", UNKNOWN_MNEMONIC_FIXTURE);
        assert_eq!(diags.len(), 1, "sanity: the fatal published, {diags:?}");
        assert_eq!(diags[0].code, Some("unknown-mnemonic"));

        let symbols = service
            .document_symbols("untitled:Untitled-1")
            .expect("CST-tier symbols survive a fatal");
        assert_eq!(
            symbols,
            vec![SymbolNode {
                name: "f".to_string(),
                kind: SymbolNodeKind::Function,
                span: Span::new(1, 1, 2, 14), // through the `bogus` word's own end
                selection_span: Span::new(1, 7, 1, 8),
                children: vec![],
            }]
        );
    }

    #[test]
    fn document_symbols_none_for_an_unknown_document() {
        let mut service = PmaLanguageService::new();
        assert_eq!(service.document_symbols("untitled:never-opened"), None);
    }
}
