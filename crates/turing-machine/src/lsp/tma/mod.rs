//! The `.tma` language service: implements `mtc_core::lsp::LanguageService`
//! over the TM-1 assembly front end. The sibling of the PM-1 crate's `.pma`
//! service — same feature set (diagnostics, completions with operand hints,
//! go-to-definition, quickfixes, semantic tokens, formatting; no hover, since
//! assembly text has no doc-line grammar), reusing this crate's own
//! config-resolution and code-action machinery from `lsp/mod.rs` rather than
//! duplicating it. Library-only — rendering and stdio belong to the CLI
//! (docs/core.md (thin-renderer rule)).
//!
//! # Where the diagnostics come from
//!
//! One call to [`crate::lint::tma::lint_tma`] settles both the fatal gate (a
//! lower or assemble failure) and the lint findings — `.tma` has no separate
//! compile-warning channel the way `.tmc` does. Routing every diagnostic
//! through that one entry is deliberate: it is the same function `tmt lint`
//! calls, so the editor and the command line agree on every finding, the
//! suppression of core's `unused-label` on this path included (that rule
//! cannot see label references living in lowered table sections, so on a
//! dispatch-table program it would false-flag every reachable dispatch and
//! exit target; the reasoning lives at the suppression site).
//!
//! On top of that the service adds one channel of its own: the frame-descriptor
//! field checks in `descriptors.rs`. Those are not new findings — every one of
//! them mirrors a rule the assembler itself enforces, and each is published
//! under the assembler's own `bad-frame` code with the assembler's own wording.
//! What the CST tier buys is that they surface ALL AT ONCE and independently of
//! the fatal gate, where lowering stops at the first offending descriptor and
//! an unrelated fatal elsewhere in the file hides the descriptor problems
//! entirely. The finding that duplicates the published fatal is dropped, so no
//! defect is ever reported twice.
//!
//! # The total CST tier
//!
//! `completion`/`definition`/`document_symbols`/`semantic_tokens` all read
//! straight off the total `AsmCst` and are never gated on `fatal`/`lint`, so
//! each still answers over a document that fails to assemble — mid-edit is
//! exactly when they matter most.
//!
//! # Recovering line structure from a `.tma` CST
//!
//! `AsmCst` is flat, and `AsmComment` alone carries no line of its own. The
//! `.pma` service recovers the missing lines by zipping items against the
//! source's non-blank lines, one item per line. **That invariant does not hold
//! for `.tma`**: `tm1_syntax()` turns the `.rept` capability on, and a `.rept`
//! … `.endr` block collapses many source lines into ONE item whose body items
//! nest inside it. [`flatten`] therefore walks the tree instead, taking each
//! item's line from its own span and consuming a non-blank line only for the
//! one shape that has no span to read — a comment. Nested `.rept` bodies are
//! flattened in place, so a cursor inside a macro body classifies against the
//! body line it is really on.

use std::collections::{BTreeSet, HashMap};
use std::ops::Range;
use std::path::PathBuf;
use std::time::SystemTime;

use mtc_core::asm::cst::{
    AsmCst, AsmItem, AsmItemKind, FrameDirectiveCst, FrameHeaderCst, FuncCst, OperandToken,
    RoutineDirectiveCst, TableDirectiveCst, TableDirectiveKind, parse_asm_cst_with,
};
use mtc_core::asm::{AsmError, Flow, SyntaxEntry, format_asm_with};
use mtc_core::diagnostics::{Diagnostic, Pos, Span};
use mtc_core::lsp::{
    Action, Candidate, DefTarget, HoverContent, LanguageService, SemToken, ServiceDiagnostic,
    ServiceSeverity, SymbolNode, SymbolNodeKind,
};
use mtc_core::vm::OperandKind;

use crate::asm::tm1_syntax;
use crate::lint::tma::lint_tma;

use super::{ConfigResolver, actions_from_findings, parse_ide_codes};

mod complete;
mod descriptors;
mod navigate;
mod tokens;

pub(crate) struct TmaLanguageService {
    docs: HashMap<String, TmaDocState>,
    /// IDE-settings allow-list: `None` = never configured; `Ok` = valid
    /// codes; `Err` = human-readable reason (surfaces as invalid-config).
    ide_allow: Option<Result<Vec<String>, String>>,
    /// `tmt.json` parse cache keyed by winner path; (mtime, outcome).
    config_cache: HashMap<PathBuf, (SystemTime, Result<Vec<String>, String>)>,
}

impl TmaLanguageService {
    pub(crate) fn new() -> Self {
        TmaLanguageService {
            docs: HashMap::new(),
            ide_allow: None,
            config_cache: HashMap::new(),
        }
    }
}

/// Per-document staged state. Simpler than `.tmc`'s: no separate
/// compile-warning channel, so one `lint_tma` call settles either the fatal
/// (a lower/assemble failure) or the lint findings — never both.
pub(crate) struct TmaDocState {
    /// The document's current text, verbatim from the framework.
    pub(crate) text: String,
    /// The document's items in source order, each with its own source line
    /// recovered by [`flatten`]. Total: every text parses, and `Raw` items
    /// mark the lines that are not assembly-shaped.
    pub(crate) flat: Vec<FlatItem>,
    /// The one fatal, when `lower`/`assemble` refused the file.
    fatal: Option<AsmError>,
    /// Lint findings, retained fixes included (the quickfix source);
    /// `Some` exactly when `fatal` is `None`.
    lint: Option<Vec<Diagnostic>>,
    /// Frame-descriptor field findings, CST-tier — always computed, never
    /// gated on the fatal.
    descriptor_findings: Vec<Diagnostic>,
    /// invalid-config messages that applied to this analysis (0..=2 entries:
    /// project file first, then IDE settings).
    config_errors: Vec<String>,
}

/// One CST item paired with the 1-based source line it starts on. Produced by
/// [`flatten`] in source order, `.rept` bodies spliced in place.
pub(crate) struct FlatItem {
    pub(crate) line: u32,
    pub(crate) item: AsmItem,
}

/// The document's items in source order with their lines recovered.
///
/// Every shape but `Comment` carries its own span, so its line is read
/// directly. A comment has only a column, so it consumes the next unclaimed
/// non-blank source line — which lands correctly because the item before it
/// has already advanced the cursor past its own last line. Total: a malformed
/// document simply yields whatever items parsed, and running out of non-blank
/// lines leaves a comment on the last line seen rather than panicking.
fn flatten(text: &str, cst: &AsmCst) -> Vec<FlatItem> {
    let nonblank: Vec<u32> = text
        .lines()
        .enumerate()
        .filter(|(_, line)| line.chars().any(|c| c != ' ' && c != '\t'))
        .map(|(i, _)| i as u32 + 1)
        .collect();
    let mut out = Vec::new();
    let mut cursor = 0usize;
    walk(&cst.items, &nonblank, &mut cursor, &mut out);
    out
}

/// [`flatten`]'s recursive worker: `cursor` indexes `nonblank` and only ever
/// moves forward, so the two sequences stay aligned across nesting.
fn walk(items: &[AsmItem], nonblank: &[u32], cursor: &mut usize, out: &mut Vec<FlatItem>) {
    for item in items {
        match &item.kind {
            AsmItemKind::Comment(_) => {
                let line = nonblank
                    .get(*cursor)
                    .copied()
                    .or_else(|| out.last().map(|f| f.line))
                    .unwrap_or(1);
                *cursor += 1;
                out.push(FlatItem {
                    line,
                    item: item.clone(),
                });
            }
            AsmItemKind::Rept(rept) => {
                let header = rept.span.start.line;
                advance_past(nonblank, cursor, header);
                out.push(FlatItem {
                    line: header,
                    item: item.clone(),
                });
                walk(&rept.body, nonblank, cursor, out);
                // The closing `.endr` shapes no item of its own; skip its line
                // so a comment after the block is not mis-seated on it.
                advance_past(nonblank, cursor, rept.endr_span.end.line);
            }
            _ => {
                let Some(span) = item_span(item) else {
                    continue;
                };
                advance_past(nonblank, cursor, span.end.line);
                out.push(FlatItem {
                    line: span.start.line,
                    item: item.clone(),
                });
            }
        }
    }
}

/// Move `cursor` to the first non-blank line strictly after `line`.
fn advance_past(nonblank: &[u32], cursor: &mut usize, line: u32) {
    while *cursor < nonblank.len() && nonblank[*cursor] <= line {
        *cursor += 1;
    }
}

/// An item's own span — `None` only for a comment, the one shape with no span
/// of its own (its line is recovered by the cursor walk instead).
fn item_span(item: &AsmItem) -> Option<Span> {
    match &item.kind {
        AsmItemKind::Func(f) => Some(f.span),
        AsmItemKind::Line(l) => Some(l.span),
        AsmItemKind::Raw(r) => Some(r.span),
        AsmItemKind::Section(s) => Some(s.span),
        AsmItemKind::TableDirective(d) => Some(d.span),
        AsmItemKind::Rept(r) => Some(r.span),
        AsmItemKind::RoutineDirective(r) => Some(r.span),
        AsmItemKind::FrameDirective(d) => Some(d.span()),
        AsmItemKind::Comment(_) => None,
    }
}

/// The flat item starting on source `line`, if any. `None` on a blank line, a
/// `.endr` line, or a line past the document's end.
pub(crate) fn item_at_line(flat: &[FlatItem], line: u32) -> Option<&AsmItem> {
    flat.iter().find(|f| f.line == line).map(|f| &f.item)
}

/// Every item in the document, flattened — the walk order every doc-wide
/// lookup below shares.
pub(crate) fn flat_items(flat: &[FlatItem]) -> impl Iterator<Item = &AsmItem> {
    flat.iter().map(|f| &f.item)
}

/// The `.func` enclosing source `line`, plus the half-open index range of the
/// FLAT list holding its own body — the items between it and the next `.func`.
/// Mirrors the `.pma` service's own recovery of per-function grouping from a
/// flat item list; `.rept` body lines belong to the function they are written
/// in, so they are included.
pub(crate) fn enclosing_function_range(
    flat: &[FlatItem],
    line: u32,
) -> Option<(&FuncCst, Range<usize>)> {
    let mut found: Option<(&FuncCst, usize)> = None;
    for (i, f) in flat.iter().enumerate() {
        if f.line > line {
            break;
        }
        if let AsmItemKind::Func(func) = &f.item.kind {
            found = Some((func, i));
        }
    }
    let (func, idx) = found?;
    let end = flat[idx + 1..]
        .iter()
        .position(|f| matches!(f.item.kind, AsmItemKind::Func(_)))
        .map_or(flat.len(), |rel| idx + 1 + rel);
    Some((func, idx + 1..end))
}

/// Every `.func` declared anywhere in the document, in source order.
pub(crate) fn doc_functions(flat: &[FlatItem]) -> impl Iterator<Item = &FuncCst> {
    flat_items(flat).filter_map(|item| match &item.kind {
        AsmItemKind::Func(f) => Some(f),
        _ => None,
    })
}

/// Every `.routine` signature declared anywhere in the document. A routine
/// whose body lives in another translation unit has a signature here and no
/// `.func`, so this is the fallback target for a call whose definition is not
/// in this file.
pub(crate) fn doc_routines(flat: &[FlatItem]) -> impl Iterator<Item = &RoutineDirectiveCst> {
    flat_items(flat).filter_map(|item| match &item.kind {
        AsmItemKind::RoutineDirective(r) => Some(r),
        _ => None,
    })
}

/// Every name a `call`/`call.m` target may resolve to: the `.func` definitions
/// plus the `.routine` signatures, sorted and deduplicated.
pub(crate) fn doc_callable_names(flat: &[FlatItem]) -> BTreeSet<&str> {
    doc_functions(flat)
        .map(|f| f.name.as_str())
        .chain(doc_routines(flat).map(|r| r.name.as_str()))
        .collect()
}

/// Every labeled table directive (`.row`/`.targets`/`.target`) in the
/// document, paired with the label that names it — what an `mtc`/`djmp`
/// operand refers to. A table's label sits on its FIRST row, so only labeled
/// directives are entries.
pub(crate) fn doc_tables(
    flat: &[FlatItem],
) -> impl Iterator<Item = (&str, Span, &TableDirectiveCst)> {
    flat_items(flat).filter_map(|item| match &item.kind {
        AsmItemKind::TableDirective(d) => d
            .labels
            .first()
            .map(|label| (label.name.as_str(), label.span, d)),
        _ => None,
    })
}

/// Every `.frame` descriptor header in the document — what a `call.m`'s second
/// operand refers to.
pub(crate) fn doc_frames(flat: &[FlatItem]) -> impl Iterator<Item = &FrameHeaderCst> {
    flat_items(flat).filter_map(|item| match &item.kind {
        AsmItemKind::FrameDirective(FrameDirectiveCst::Header(h)) => Some(h),
        _ => None,
    })
}

/// Which reference kind an operand plays, given its mnemonic's syntax entry
/// and its own position in the operand list.
///
/// `mtc`/`djmp` name a TABLE; `call.m` names a callable in slot 0 and a FRAME
/// descriptor in slot 1; the relative-branch family behaves exactly as it does
/// in PM-1 assembly — `@name` is always a callable reference, a bare name is a
/// LABEL under jump/branch flow and a callable under call flow. Immediates
/// (`trap #k`, `retx #k`) and the vector operands (`wr`, `mov`, `wrmv`) name
/// nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OperandRole {
    Label,
    Callable,
    Table,
    Frame,
}

pub(crate) fn operand_role(entry: &SyntaxEntry, index: usize, text: &str) -> Option<OperandRole> {
    match entry.operand {
        OperandKind::TableRef => (index == 0).then_some(OperandRole::Table),
        OperandKind::FramedCall => match index {
            0 => Some(OperandRole::Callable),
            1 => Some(OperandRole::Frame),
            _ => None,
        },
        OperandKind::RelI8 | OperandKind::RelI32 => {
            if index != 0 {
                return None;
            }
            if text.starts_with('@') {
                return Some(OperandRole::Callable);
            }
            match entry.flow {
                Flow::Jump | Flow::Branch => Some(OperandRole::Label),
                Flow::Call => Some(OperandRole::Callable),
                Flow::FallThrough | Flow::Stop => None,
            }
        }
        OperandKind::None
        | OperandKind::Imm8
        | OperandKind::SymbolVec
        | OperandKind::MoveVec
        | OperandKind::WriteMoveVec => None,
    }
}

/// An operand's own NAME span — the span itself for a bare name, or the span
/// minus its leading `@` for a sigil-prefixed callable reference. Every
/// consumer of a name operand (a completion's replace span, a semantic token,
/// a definition's origin) points at the name, never the sigil.
pub(crate) fn name_span(operand: &OperandToken) -> Span {
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

/// An operand's referenced name, sigil stripped.
pub(crate) fn operand_name(operand: &OperandToken) -> &str {
    operand.text.strip_prefix('@').unwrap_or(&operand.text)
}

/// True when a name carries an unexpanded `.rept` substitution marker. Such a
/// name is a template, not an identifier: it resolves to nothing in the CST
/// (the expanded spellings exist only after lowering), so navigation and
/// highlighting stay quiet on it rather than reporting a false miss.
pub(crate) fn is_templated(name: &str) -> bool {
    name.contains('{')
}

/// The merged diagnostic set for one document: invalid-config warnings first,
/// then either the one fatal or the lint findings, with the CST-tier
/// descriptor findings folded in on either branch.
fn merged_diagnostics(state: &TmaDocState) -> Vec<ServiceDiagnostic> {
    let mut out: Vec<ServiceDiagnostic> = state
        .config_errors
        .iter()
        .map(|message| ServiceDiagnostic {
            span: Span::point(1, 1),
            severity: ServiceSeverity::Warning,
            source: "tmt",
            code: Some("invalid-config"),
            message: message.clone(),
            // `.tma` has no attribute grammar, so no channel here is ever
            // deprecation-tagged.
            deprecated: false,
        })
        .collect();

    let mut findings: Vec<ServiceDiagnostic> = Vec::new();
    if let Some(fatal) = &state.fatal {
        // Exactly one Error from the gate, never a cascade. The message is
        // the KIND's own Display: the `line N:M:` prefix and the bracketed
        // code suffix are CLI renderings, and the client places the span and
        // shows the code itself.
        findings.push(ServiceDiagnostic {
            span: fatal.span,
            severity: ServiceSeverity::Error,
            source: "tmt",
            code: Some(fatal.kind.code()),
            message: fatal.kind.to_string(),
            deprecated: false,
        });
    } else {
        findings.extend(state.lint.iter().flatten().map(|d| ServiceDiagnostic {
            span: d.span,
            severity: ServiceSeverity::Warning,
            source: "tmt lint",
            code: Some(d.code),
            message: d.message.clone(),
            deprecated: false,
        }));
    }

    // The descriptor channel. A finding at the published fatal's own span IS
    // that fatal seen one tier earlier — dropping it keeps every defect
    // reported exactly once.
    let fatal_span = state.fatal.as_ref().map(|f| f.span);
    findings.extend(
        state
            .descriptor_findings
            .iter()
            .filter(|d| Some(d.span) != fatal_span)
            .map(|d| ServiceDiagnostic {
                span: d.span,
                severity: ServiceSeverity::Error,
                source: "tmt",
                code: Some(d.code),
                message: d.message.clone(),
                deprecated: false,
            }),
    );

    findings.sort_by_key(|d| d.span.start);
    out.extend(findings);
    out
}

/// Semantic-token legend indices/bits (`tokens.rs`) — the ONLY spellings the
/// emitter uses for legend positions; kept in lockstep with `token_legend()`'s
/// arrays by a drift-guard test in `tokens.rs`. Distinct from the `.tmc`
/// service's own constants of the same name: each service's legend is its own.
pub(crate) const TOKEN_TYPE_FUNCTION: u32 = 0;
pub(crate) const TOKEN_TYPE_VARIABLE: u32 = 1;
pub(crate) const TOKEN_TYPE_TYPE: u32 = 2;
pub(crate) const TOKEN_TYPE_NUMBER: u32 = 3;
pub(crate) const MODIFIER_DECLARATION: u32 = 1 << 0;

impl LanguageService for TmaLanguageService {
    fn language_id(&self) -> &'static str {
        "tma"
    }

    fn extensions(&self) -> &'static [&'static str] {
        &[".tma"]
    }

    fn trigger_characters(&self) -> &[char] {
        // `@` opens a symbol reference, `.` a directive, `,` the next operand
        // slot (a `call.m`'s frame, a `.targets` entry), `[` a vector operand.
        &['@', '.', ',', '[']
    }

    fn token_legend(&self) -> (&'static [&'static str], &'static [&'static str]) {
        // Code labels ride "variable" (with "declaration" on definitions);
        // table and frame labels ride "type", the one distinction `.tma` has
        // that PM-1 assembly does not — a dispatch table is a data structure,
        // not a jump target. Mnemonics stay TextMate's job.
        (
            &["function", "variable", "type", "number"],
            &["declaration"],
        )
    }

    fn watched_globs(&self) -> &'static [&'static str] {
        &["**/tmt.json"]
    }

    fn did_update(&mut self, uri: &str, text: &str) -> Vec<ServiceDiagnostic> {
        // 1. Resolve config — the same machinery, and the same union
        //    semantics, the `.tmc` service uses.
        let (effective_allow, config_errors) = ConfigResolver {
            ide_allow: &self.ide_allow,
            config_cache: &mut self.config_cache,
        }
        .resolve(uri);

        // 2. Total CST, always. One `lint_tma` call gives the fatal gate AND
        //    the lint findings — the same entry `tmt lint` uses, so both
        //    surfaces report the same set.
        let cst = parse_asm_cst_with(text, tm1_syntax().caps);
        let (fatal, lint_findings) = match lint_tma(text, &effective_allow) {
            Ok(findings) => (None, Some(findings)),
            Err(e) => (Some(e), None),
        };

        // 3. The CST-tier descriptor channel, independent of the gate.
        let flat = flatten(text, &cst);
        let descriptor_findings = descriptors::check(&flat);

        let state = TmaDocState {
            text: text.to_string(),
            flat,
            fatal,
            lint: lint_findings,
            descriptor_findings,
            config_errors,
        };
        let diagnostics = merged_diagnostics(&state);
        self.docs.insert(uri.to_string(), state);
        diagnostics
    }

    fn did_close(&mut self, uri: &str) {
        // Drop everything; the framework publishes the empty diagnostic set.
        self.docs.remove(uri);
    }

    fn did_change_config(&mut self, settings: serde_json::Value) {
        // Clients that forward whole configuration sections wrap the
        // service's settings under a "tmt" key; unwrap when present. Only
        // `lint.allow` is ours — `.tma` has no opt-in rule tier, so unlike
        // the `.tmc` service there is no `lint.warn` channel to read. Every
        // other key is client-owned and ignored; missing entirely means the
        // channel is unconfigured, not invalid.
        let section = settings.get("tmt").unwrap_or(&settings);
        self.ide_allow = section
            .get("lint")
            .and_then(|lint| lint.get("allow"))
            .map(|v| parse_ide_codes("allow", v));
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

    // Assembly text has no doc/attention-line grammar of its own, so there is
    // nothing for a hover to render. Permanent, not a this-round gap — the
    // same call the `.pma` service makes.
    fn hover(&mut self, _uri: &str, _pos: Pos) -> Option<HoverContent> {
        None
    }

    fn code_actions(&mut self, uri: &str, span: Span) -> Vec<Action> {
        // Lint-tier: `lint` is `Some` exactly when the file assembled cleanly
        // (an empty `Vec` otherwise — no findings to offer quickfixes for,
        // whether the document is unknown or fataled).
        let Some(lint) = self.docs.get(uri).and_then(|state| state.lint.as_ref()) else {
            return Vec::new();
        };
        actions_from_findings(lint, span)
    }

    fn document_symbols(&mut self, uri: &str) -> Option<Vec<SymbolNode>> {
        // CST-tier (total): answered for any known document, broken or not.
        let state = self.docs.get(uri)?;
        Some(document_symbols(&state.flat))
    }

    fn semantic_tokens(&mut self, uri: &str) -> Option<Vec<SemToken>> {
        let state = self.docs.get(uri)?;
        Some(tokens::semantic_tokens(state))
    }

    fn format(&mut self, uri: &str) -> Option<String> {
        // Whole-document formatting: reads the DOCSTORE's text — the same
        // single-source contract the `.tmc` service holds — through the
        // canonical grid printer under `tm1_syntax()`'s caps, so sections,
        // table directives, `.rept` blocks and frame descriptors all print in
        // their own canonical form.
        let state = self.docs.get(uri)?;
        format_asm_with(&state.text, tm1_syntax().caps).ok()
    }
}

/// Document symbols, CST-tier: one node per `.func` (its own code labels as
/// children), one per `.routine` signature, and one per labeled table or frame
/// descriptor. `SymbolNodeKind` has no label variant, so code labels reuse
/// `Function` (the accepted mapping the `.pma` service also makes) while
/// tables and frames — data, not code — ride `Namespace` to read distinctly in
/// an outline.
fn document_symbols(flat: &[FlatItem]) -> Vec<SymbolNode> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < flat.len() {
        match &flat[i].item.kind {
            AsmItemKind::RoutineDirective(r) => out.push(SymbolNode {
                name: r.name.clone(),
                kind: SymbolNodeKind::Function,
                span: r.span,
                selection_span: r.name_span,
                children: Vec::new(),
            }),
            AsmItemKind::TableDirective(d) => {
                if let Some(label) = d.labels.first() {
                    out.push(SymbolNode {
                        name: label.name.clone(),
                        kind: SymbolNodeKind::Namespace,
                        span: d.span,
                        selection_span: label.span,
                        children: Vec::new(),
                    });
                }
            }
            AsmItemKind::FrameDirective(FrameDirectiveCst::Header(h)) => out.push(SymbolNode {
                name: h.label.name.clone(),
                kind: SymbolNodeKind::Namespace,
                span: h.span,
                selection_span: h.label.span,
                children: Vec::new(),
            }),
            AsmItemKind::Func(f) => {
                let end = flat[i + 1..]
                    .iter()
                    .position(|it| matches!(it.item.kind, AsmItemKind::Func(_)))
                    .map_or(flat.len(), |rel| i + 1 + rel);
                let last_end = flat[i + 1..end]
                    .iter()
                    .filter_map(|it| item_span(&it.item))
                    .map(|s| s.end)
                    .max()
                    .unwrap_or(f.span.end);
                let children = flat[i + 1..end]
                    .iter()
                    .filter_map(|it| match &it.item.kind {
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
                continue;
            }
            _ => {}
        }
        i += 1;
    }
    out
}

/// The table-directive kind's own keyword, for a completion hint or a symbol
/// detail.
pub(crate) fn table_kind_word(kind: TableDirectiveKind) -> &'static str {
    match kind {
        TableDirectiveKind::Row => ".row",
        TableDirectiveKind::Targets => ".targets",
        TableDirectiveKind::Target => ".target",
    }
}

#[cfg(test)]
mod tests;
