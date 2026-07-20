//! The `.tmc` language service: implements `mtc_core::lsp::LanguageService`
//! over the real TM-1 front end. Owns per-document staged
//! state, the diagnostic merge (fatal / compile warnings / lint findings),
//! and both configuration channels (`tmt.json` project files and IDE
//! settings). Library-only — rendering and stdio belong to the CLI
//! (docs/cli.md (thin-renderer rule)).
//!
//! # Which stages the diagnostics come from
//!
//! [`crate::compiler::analyze_staged`] runs lex → parse → resolve, keeping
//! each stage's partial result. The service adds ONE stage beyond it: when
//! resolution completed cleanly it also runs range/graft expansion, purely
//! for its fatal. Expansion is where the binding-map legality rules live
//! (the identity/blank-pin/injectivity family), so without that step a
//! whole class of errors a `tmt compile` would report stays invisible in
//! the editor — and the map quickfix would have no trigger. Expansion is a
//! pure function of the resolved module, so running it here costs one extra
//! traversal and cannot change what the batch pipeline does.
//!
//! # Staged-seam limitation, stated honestly
//!
//! The resolve stage stops at its first offending span rather than
//! accumulating, and it raises its non-fatal findings (unused-import) only
//! at the very end. A document that fatals partway through resolution
//! therefore surfaces exactly one diagnostic — the fatal — and none of the
//! warnings the earlier, unaffected declarations would have produced. This
//! is a property of the analysis seam, not of the service; the service
//! keeps a last-good name roster so completions stay useful across such an
//! edit, which is the part it can do something about.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use mtc_core::diagnostics::{Applicability, Diagnostic, Pos, Span};
use mtc_core::lsp::{
    Action, Candidate, DefTarget, HoverContent, LanguageService, SemToken, ServiceDiagnostic,
    ServiceSeverity, SymbolNode, SymbolNodeKind,
};

use crate::compiler::{CompileError, Resolved, analyze_staged};
use crate::config;
use crate::cst::{Cst, MachineCst, NamespaceCst, ReuseCst, TopItem, TopKind, WorldItem, WorldKind};
use crate::lexer::{Token, TokenKind};
use crate::lint::{LintContext, LintError, run_rules, validate_allow};
use crate::parser::{Doc, Program};

mod complete;
mod context;
#[cfg(test)]
mod e2e;
mod navigate;
mod quickfix;
mod roster;
mod tma;
mod tokens;

pub(crate) use roster::Roster;
pub(crate) use tma::TmaLanguageService;

pub(crate) struct TmcLanguageService {
    docs: HashMap<String, DocState>,
    /// IDE-settings allow-list: `None` = never configured; `Ok` = valid
    /// codes; `Err` = human-readable reason (surfaces as invalid-config).
    ide_allow: Option<Result<Vec<String>, String>>,
    /// IDE-settings opt-in list, same three states. `tmt.json` has no
    /// `warn` key, so this is the only channel that can turn a default-off
    /// rule on for the editor.
    ide_warn: Option<Result<Vec<String>, String>>,
    /// `tmt.json` parse cache keyed by winner path; (mtime, outcome).
    config_cache: HashMap<PathBuf, (SystemTime, Result<Vec<String>, String>)>,
}

impl Default for TmcLanguageService {
    fn default() -> Self {
        TmcLanguageService::new()
    }
}

impl TmcLanguageService {
    pub(crate) fn new() -> Self {
        TmcLanguageService {
            docs: HashMap::new(),
            ide_allow: None,
            ide_warn: None,
            config_cache: HashMap::new(),
        }
    }
}

/// Config resolution for one document: the mtime-cached `tmt.json` lookup
/// plus the two-channel union (project file first, then IDE settings) into
/// one effective allow-list and its invalid-config messages. Borrows its
/// channels for the span of one call rather than owning them, so the
/// service keeps its own fields.
struct ConfigResolver<'a> {
    ide_allow: &'a Option<Result<Vec<String>, String>>,
    config_cache: &'a mut HashMap<PathBuf, (SystemTime, Result<Vec<String>, String>)>,
}

impl ConfigResolver<'_> {
    /// The project-file channel: the parsed outcome of the discovered
    /// `tmt.json`, through the mtime cache — reused only while the file's
    /// mtime is unchanged, else re-loaded and re-cached. Errors come back
    /// as the full display string (path + reason), ready to be an
    /// `invalid-config` message.
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
            // No stat (a file racing in and out of existence) → no cache
            // entry: there is no mtime to key staleness on.
            self.config_cache
                .insert(winner.to_path_buf(), (mtime, outcome.clone()));
        }
        outcome
    }

    /// `(effective_allow, config_errors)` for one document — union, never
    /// a cascade: the nearest `tmt.json` and the IDE channel both
    /// contribute, project file first.
    fn resolve(&mut self, uri: &str) -> (Vec<String>, Vec<String>) {
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
        match self.ide_allow {
            None => {}
            Some(Ok(codes)) => union_into(&mut effective_allow, codes),
            Some(Err(reason)) => config_errors.push(format!("IDE settings: {reason}")),
        }
        (effective_allow, config_errors)
    }
}

/// Per-document staged state: each stage's outcome for the CURRENT text,
/// plus the one sanctioned piece of staleness (`roster`).
pub(crate) struct DocState {
    /// The document's current text, verbatim from the framework.
    pub(crate) text: String,
    /// WithComments token stream of the current text; `None` only when
    /// lexing itself failed.
    pub(crate) tokens: Option<Vec<Token>>,
    /// The lossless CST (`None` when lexing or parsing failed).
    pub(crate) cst: Option<Cst>,
    /// The flat program — survives a resolve-stage fatal.
    pub(crate) program: Option<Program>,
    /// The resolved module (`None` when any stage up to resolve failed).
    pub(crate) resolved: Option<Resolved>,
    /// Compile-channel warnings of the current text.
    pub(crate) warnings: Vec<Diagnostic>,
    /// Lint findings, retained fixes included; `Some` exactly when
    /// `resolved` is — the rules read the resolved module.
    pub(crate) lint: Option<Vec<Diagnostic>>,
    /// The first (only) fatal, at whichever stage produced it —
    /// expansion's included.
    pub(crate) fatal: Option<CompileError>,
    /// Names-only staleness exception: the last-good roster survives a
    /// failed re-analysis so completion candidates stay useful mid-edit.
    /// Positions ALWAYS come from the current token stream; only names and
    /// glyph rosters may be one edit old.
    pub(crate) roster: Option<Roster>,
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
/// contains-check `tmt lint` uses when folding project config into
/// `--allow`).
fn union_into(dst: &mut Vec<String>, src: &[String]) {
    for code in src {
        if !dst.contains(code) {
            dst.push(code.clone());
        }
    }
}

/// Half-open span overlap: `a.start < b.end && b.start < a.end`.
pub(crate) fn spans_overlap(a: Span, b: Span) -> bool {
    a.start < b.end && b.start < a.end
}

/// True when `pos` sits inside `span`, or exactly at its end — the
/// cursor-touches-token rule every position lookup in this module shares.
pub(crate) fn span_touches(span: Span, pos: Pos) -> bool {
    span.start <= pos && pos <= span.end
}

/// Parses one IDE-channel rule-code list (`lint.allow` / `lint.warn`):
/// an array of known rule codes, or a human-readable reason why not.
fn parse_ide_codes(key: &str, value: &serde_json::Value) -> Result<Vec<String>, String> {
    let shape = format!("`lint.{key}` must be an array of strings");
    let arr = value.as_array().ok_or_else(|| shape.clone())?;
    let mut codes = Vec::with_capacity(arr.len());
    for item in arr {
        codes.push(item.as_str().ok_or_else(|| shape.clone())?.to_string());
    }
    match validate_allow(&codes) {
        Ok(()) => Ok(codes),
        Err(LintError::UnknownAllowCode(code)) => {
            Err(format!("unknown lint rule `{code}` in lint.{key}"))
        }
        // `validate_allow` only ever produces `UnknownAllowCode` today, but
        // this runs on the editor's request thread: a total arm keeps a
        // future variant an invalid-config message rather than a panic that
        // takes the whole language server down.
        Err(other) => Err(other.to_string()),
    }
}

/// The merged diagnostic set for one document: invalid-config warnings
/// first, then the one fatal (if any), then the span-ordered merge of
/// compile warnings (source `"tmt"`) and lint findings (source
/// `"tmt lint"`).
///
/// Unlike a fatal from lex/parse/resolve, an EXPANSION fatal arrives with
/// a complete resolved module behind it, so the resolve-stage warnings and
/// lint findings are still valid and still shown. The rule that produces
/// that behavior needs no special case: warnings and lint are emitted
/// whenever the resolved module exists, and the resolve-or-earlier fatals
/// are exactly the ones for which it does not.
fn merged_diagnostics(state: &DocState) -> Vec<ServiceDiagnostic> {
    let mut out: Vec<ServiceDiagnostic> = state
        .config_errors
        .iter()
        .map(|message| ServiceDiagnostic {
            span: Span::point(1, 1),
            severity: ServiceSeverity::Warning,
            source: "tmt",
            code: Some("invalid-config"),
            message: message.clone(),
            deprecated: false,
        })
        .collect();

    if let Some(fatal) = &state.fatal {
        // Exactly one Error, never a cascade. The message is the KIND's
        // Display — the `line N:M:` prefix and bracketed code suffix are
        // CLI renderings; the client places the span and shows the code.
        out.push(ServiceDiagnostic {
            span: fatal.span,
            severity: ServiceSeverity::Error,
            source: "tmt",
            code: Some(fatal.kind.code()),
            message: fatal.kind.to_string(),
            deprecated: false,
        });
    }

    let mut findings: Vec<ServiceDiagnostic> = Vec::new();
    findings.extend(state.warnings.iter().map(|d| ServiceDiagnostic {
        span: d.span,
        severity: ServiceSeverity::Warning,
        source: "tmt",
        code: Some(d.code),
        message: d.message.clone(),
        deprecated: false,
    }));
    if let Some(lint) = &state.lint {
        findings.extend(lint.iter().map(|d| ServiceDiagnostic {
            span: d.span,
            severity: ServiceSeverity::Warning,
            source: "tmt lint",
            code: Some(d.code),
            message: d.message.clone(),
            // `deprecated-call` is the one tagged code; every other lint
            // finding stays untagged.
            deprecated: d.code == "deprecated-call",
        }));
    }
    // Stable sort: equal starts keep the warnings-then-lint channel order.
    findings.sort_by_key(|d| d.span.start);
    out.extend(findings);
    out
}

/// One documented declaration's hover body: plain text, paragraphs
/// blank-line separated, then a `deprecated[: MSG]` line, then each
/// attention line as its own `note: ` line — a blank line between the
/// three GROUPS, never markdown. `None` when the doc has nothing to show
/// at all: a lone blank `?` line reduces to a `Doc` with every field
/// empty, and that must never surface as a blank popup.
pub(crate) fn render_doc(doc: &Doc) -> Option<String> {
    if doc.paragraphs.is_empty() && doc.attention.is_empty() && doc.deprecated.is_none() {
        return None;
    }
    let mut sections: Vec<String> = Vec::new();
    if !doc.paragraphs.is_empty() {
        sections.push(doc.paragraphs.join("\n\n"));
    }
    if let Some(message) = &doc.deprecated {
        sections.push(if message.is_empty() {
            "deprecated".to_string()
        } else {
            format!("deprecated: {message}")
        });
    }
    if !doc.attention.is_empty() {
        sections.push(
            doc.attention
                .iter()
                .map(|line| format!("note: {line}"))
                .collect::<Vec<_>>()
                .join("\n"),
        );
    }
    Some(sections.join("\n\n"))
}

/// Walks one CST item list — the file level or a namespace block's own
/// items — into document symbols. Comments and imports are skipped; a
/// reopened namespace stays a separate sibling because the CST already
/// keeps it apart.
fn cst_symbols(items: &[TopItem]) -> Vec<SymbolNode> {
    items
        .iter()
        .filter_map(|item| match &item.kind {
            TopKind::Comment(_) | TopKind::Import(_) => None,
            TopKind::Alphabet(a) => Some(SymbolNode {
                name: a.name.clone(),
                kind: SymbolNodeKind::Function,
                span: a.span,
                selection_span: a.name_span,
                children: Vec::new(),
            }),
            TopKind::Namespace(ns) => Some(namespace_symbol(ns)),
            TopKind::Reuse(r) => Some(reuse_symbol(r)),
            TopKind::Machine(m) => Some(machine_symbol(m)),
        })
        .collect()
}

fn namespace_symbol(ns: &NamespaceCst) -> SymbolNode {
    SymbolNode {
        name: ns.name.clone(),
        kind: SymbolNodeKind::Namespace,
        span: ns.span,
        selection_span: ns.name_span,
        children: cst_symbols(&ns.items),
    }
}

fn reuse_symbol(r: &ReuseCst) -> SymbolNode {
    SymbolNode {
        name: r.name.clone(),
        kind: SymbolNodeKind::Function,
        span: r.span,
        selection_span: r.name_span,
        children: world_symbols(&r.items),
    }
}

fn machine_symbol(m: &MachineCst) -> SymbolNode {
    // The machine block has no name token of its own, so its selection
    // span is the keyword's own position (a one-character point the
    // client can still reveal).
    SymbolNode {
        name: "machine".to_string(),
        kind: SymbolNodeKind::Function,
        span: m.span,
        selection_span: Span::point(m.line, m.col),
        children: world_symbols(&m.items),
    }
}

/// A world body's addressable children: states, graft instances, binds.
/// Tape declarations and comments are not symbols.
fn world_symbols(items: &[WorldItem]) -> Vec<SymbolNode> {
    items
        .iter()
        .filter_map(|item| match &item.kind {
            WorldKind::Comment(_) | WorldKind::Tape(_) => None,
            WorldKind::State(s) => Some(SymbolNode {
                name: s.name.clone(),
                kind: SymbolNodeKind::Function,
                span: s.span,
                selection_span: s.name_span,
                children: Vec::new(),
            }),
            WorldKind::Graft(g) => g.as_name.as_ref().map(|(name, name_span)| SymbolNode {
                name: name.clone(),
                kind: SymbolNodeKind::Function,
                span: g.span,
                selection_span: *name_span,
                children: Vec::new(),
            }),
            WorldKind::Bind(b) => Some(SymbolNode {
                name: b.as_name.0.clone(),
                kind: SymbolNodeKind::Function,
                span: b.span,
                selection_span: b.as_name.1,
                children: Vec::new(),
            }),
        })
        .collect()
}

/// Semantic-token legend indices/bits — the ONLY spellings the emitter
/// uses for legend positions; kept in lockstep with `token_legend()`'s
/// arrays by a drift-guard test in `tokens.rs`.
pub(crate) const TOKEN_TYPE_NAMESPACE: u32 = 0;
pub(crate) const TOKEN_TYPE_TYPE: u32 = 1;
pub(crate) const TOKEN_TYPE_FUNCTION: u32 = 2;
pub(crate) const TOKEN_TYPE_VARIABLE: u32 = 3;
pub(crate) const TOKEN_TYPE_STRING: u32 = 4;
pub(crate) const TOKEN_TYPE_NUMBER: u32 = 5;
pub(crate) const MODIFIER_DECLARATION: u32 = 1 << 0;

impl LanguageService for TmcLanguageService {
    fn language_id(&self) -> &'static str {
        "tmc"
    }

    fn extensions(&self) -> &'static [&'static str] {
        &[".tmc"]
    }

    fn trigger_characters(&self) -> &[char] {
        // `:` opens a `::` path and a `tape t:` alphabet slot; `[` opens a
        // pattern/write/move vector; `,` steps to the next cell; `=` opens
        // a binding value; `>` completes the `->` transition arrow.
        &[':', '[', ',', '=', '>']
    }

    fn token_legend(&self) -> (&'static [&'static str], &'static [&'static str]) {
        (
            &[
                "namespace",
                "type",
                "function",
                "variable",
                "string",
                "number",
            ],
            &["declaration"],
        )
    }

    fn watched_globs(&self) -> &'static [&'static str] {
        &["**/tmt.json"]
    }

    fn did_update(&mut self, uri: &str, text: &str) -> Vec<ServiceDiagnostic> {
        // 1. Resolve config. Discovery re-runs on EVERY analysis (a few
        //    stats) — a newly created nearer tmt.json must win; only the
        //    parse of the winner is cached, by mtime.
        let (effective_allow, mut config_errors) = ConfigResolver {
            ide_allow: &self.ide_allow,
            config_cache: &mut self.config_cache,
        }
        .resolve(uri);
        let effective_warn = match &self.ide_warn {
            None => Vec::new(),
            Some(Ok(codes)) => codes.clone(),
            Some(Err(reason)) => {
                config_errors.push(format!("IDE settings: {reason}"));
                Vec::new()
            }
        };

        // 2. Staged analysis, then — only over a clean resolve — the
        //    expansion stage, for its fatal alone (the binding-map
        //    legality family lives there).
        let staged = analyze_staged(text);
        let mut fatal = staged.fatal;
        if let Some(resolved) = &staged.resolved
            && fatal.is_none()
            && let Err(e) = crate::expand::expand(resolved)
        {
            fatal = Some(e);
        }

        // 3. Lint over the resolved module when there is one.
        let lint = staged.resolved.as_ref().map(|resolved| {
            let ctx = LintContext {
                resolved,
                diagnostics: &staged.diagnostics,
            };
            run_rules(&ctx, &effective_allow, &effective_warn)
        });

        // 4. Store the doc state; a failed re-analysis keeps the previous
        //    last-good roster (the names-only staleness exception).
        let prev = self.docs.remove(uri);
        let roster = match &staged.resolved {
            Some(resolved) => Some(Roster::build(resolved, staged.program.as_ref())),
            None => prev.and_then(|d| d.roster),
        };
        let state = DocState {
            text: text.to_string(),
            tokens: staged.tokens,
            cst: staged.cst,
            program: staged.program,
            resolved: staged.resolved,
            warnings: staged.diagnostics,
            lint,
            fatal,
            roster,
            config_errors,
        };
        let diagnostics = merged_diagnostics(&state);
        self.docs.insert(uri.to_string(), state);
        diagnostics
    }

    fn did_close(&mut self, uri: &str) {
        // Drop everything, staleness included; the framework publishes the
        // empty diagnostic set.
        self.docs.remove(uri);
    }

    fn did_change_config(&mut self, settings: serde_json::Value) {
        // Clients that forward whole configuration sections wrap the
        // service's settings under a "tmt" key; unwrap when present.
        let section = settings.get("tmt").unwrap_or(&settings);
        // Only `lint.allow` / `lint.warn` are ours. Every other key is
        // client-owned (binary path, trace switches, …) and deliberately
        // ignored — strictness belongs to tmt.json. Missing entirely = the
        // channel is unconfigured, not invalid. No republish from here:
        // the framework re-runs did_update on every open doc after this.
        let lint = section.get("lint");
        self.ide_allow = lint
            .and_then(|lint| lint.get("allow"))
            .map(|v| parse_ide_codes("allow", v));
        self.ide_warn = lint
            .and_then(|lint| lint.get("warn"))
            .map(|v| parse_ide_codes("warn", v));
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

    fn hover(&mut self, uri: &str, pos: Pos) -> Option<HoverContent> {
        let state = self.docs.get(uri)?;
        navigate::hover(state, pos)
    }

    fn code_actions(&mut self, uri: &str, span: Span) -> Vec<Action> {
        let Some(state) = self.docs.get(uri) else {
            return Vec::new();
        };
        let mut actions = quickfix::fatal_actions(state, span);
        if let Some(lint) = state.lint.as_ref() {
            actions.extend(actions_from_findings(lint, span));
        }
        actions
    }

    fn document_symbols(&mut self, uri: &str) -> Option<Vec<SymbolNode>> {
        // CST-tier: answered as long as parsing succeeded, even if the
        // resolve or expansion stage then fatals.
        let state = self.docs.get(uri)?;
        let cst = state.cst.as_ref()?;
        Some(cst_symbols(&cst.items))
    }

    fn semantic_tokens(&mut self, uri: &str) -> Option<Vec<SemToken>> {
        let state = self.docs.get(uri)?;
        tokens::semantic_tokens(state)
    }

    fn format(&mut self, uri: &str) -> Option<String> {
        // Whole-document formatting: reads the DOCSTORE's text — the
        // framework diffs the returned text against exactly what
        // `did_update` last received, never a re-read from disk.
        let state = self.docs.get(uri)?;
        crate::fmt::format(&state.text).ok()
    }
}

/// Lint findings whose span overlaps `span`, each turned into a quickfix
/// `Action`: only findings carrying a `Fix` contribute; `preferred`
/// mirrors `Applicability::MachineApplicable`.
fn actions_from_findings(findings: &[Diagnostic], span: Span) -> Vec<Action> {
    findings
        .iter()
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

/// The significant token stream of a document: the WithComments stream
/// minus comment trivia. Every position-classification walk in this module
/// works over this, so a comment never shifts a context decision.
pub(crate) fn significant(tokens: &[Token]) -> Vec<Token> {
    tokens
        .iter()
        .filter(|t| !matches!(t.kind, TokenKind::Comment(_)))
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests;
