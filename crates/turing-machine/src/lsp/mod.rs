//! The `.tmc` language service: implements `mtc_core::lsp::LanguageService`
//! over the real TM-1 front end (docs/lsp.md). Owns per-document staged
//! state, the diagnostic merge (fatal / compile warnings / lint findings),
//! and both configuration channels (`tmt.json` project files and IDE
//! settings). Library-only — rendering and stdio belong to the CLI
//! (docs/core.md (thin-renderer rule)).
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
use std::rc::Rc;
use std::time::SystemTime;

use mtc_core::diagnostics::{Applicability, Diagnostic, Pos, Span};
use mtc_core::lsp::{
    Action, Candidate, DefTarget, HoverContent, LanguageService, SemToken, ServiceDiagnostic,
    ServiceSeverity, SymbolNode, SymbolNodeKind,
};
use mtc_core::syntax::{AstNode, GreenNode, SyntaxElement, SyntaxNode, TextLineIndex, TextRange};

use crate::compiler::{CompileError, Resolved, analyze_staged};
use crate::config;
use crate::lexer::Token;
use crate::lint::{LintContext, LintError, run_rules, validate_allow};
use crate::parser::{Doc, Program, significant_tokens};
use crate::syntax::{
    BindView, GraftView, MachineView, NamespaceView, ReuseView, RootView, StateView, TmcKind,
    TopView,
};

mod complete;
mod context;
#[cfg(test)]
mod e2e;
mod navigate;
mod overlay;
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
    /// `tmt.json` project-section discovery cache (docs/lsp.md
    /// (configuration)), keyed the same way as `config_cache` but over
    /// the manifest's `project` section instead of `lint.allow` — the
    /// cross-file overlay's own manifest lookup (`overlay::project_view`).
    manifest_cache: overlay::ManifestCache,
    /// One open document's sibling-export scan cache (docs/lsp.md
    /// (configuration)), shared across every document this service
    /// builds an overlay for (`overlay::build_overlay`).
    sibling_cache: overlay::SiblingCache,
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
            manifest_cache: overlay::ManifestCache::new(),
            sibling_cache: overlay::SiblingCache::new(),
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

/// Bounds `config_cache`'s growth (docs/lsp.md (configuration)). Each key
/// is a discovered `tmt.json` winner path, so a single open workspace keys
/// in at most a handful of entries — but a server process is long-running
/// and nearest-ancestor discovery re-runs per document, so a session that
/// visits many project roots (a monorepo, or several workspace folders over
/// one LSP process's lifetime) would otherwise grow the map forever. A
/// cache miss just re-parses `tmt.json` from disk — the same cost already
/// paid on a first visit or an mtime-stale hit — so evicting an arbitrary
/// entry once at capacity is safe: it can only turn a hit into a miss,
/// never produce a wrong allow-list.
const CONFIG_CACHE_LIMIT: usize = 32;

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
            if !self.config_cache.contains_key(winner)
                && self.config_cache.len() >= CONFIG_CACHE_LIMIT
                && let Some(evict) = self.config_cache.keys().next().cloned()
            {
                self.config_cache.remove(&evict);
            }
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
    /// Line index over `text`, built once per document version in
    /// `did_update` — the text is immutable per version, so every
    /// position-mapping consumer (symbols, quickfixes, formatting)
    /// borrows this instead of rescanning the text per request.
    pub(crate) line_index: TextLineIndex,
    /// WithComments token stream of the current text; `None` only when
    /// lexing itself failed.
    pub(crate) tokens: Option<Vec<Token>>,
    /// Green syntax tree of the current text (docs/core.md (syntax
    /// trees)); `None` when lexing or parsing failed. `Rc`, not `Arc`:
    /// the language server is single-threaded by construction, and this
    /// is the one field that would need to change for `DocState` to
    /// become `Send` — every other field is (pinned below). Read by
    /// `document_symbols` and by `quickfix.rs`'s `state_stub`, both
    /// indexing into the SAME tree by byte range rather than reparsing.
    pub(crate) green: Option<Rc<GreenNode>>,
    /// The resilient parse's tree (docs/core.md (syntax trees), error
    /// recovery), `Some` exactly when a parse-stage fatal left `green`
    /// `None`: lossless over the CURRENT text, broken regions wrapped
    /// in ERROR nodes. Symbols fall back to it; formatting,
    /// `state_stub`, and every clean-parse-keyed feature read `green`
    /// alone.
    pub(crate) recovered_green: Option<Rc<GreenNode>>,
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
    /// The cross-file symbol table for this document (docs/lsp.md
    /// (configuration)): `None` when the document degrades to
    /// single-file behavior (no manifest found, the document is a
    /// member of no target, or it has no `file:` path at all). Consumed
    /// by `did_update`'s own diagnostics refinement; completion,
    /// navigation, and hover each wire in their own read of it
    /// separately, in a later round. Deliberately narrower than this
    /// struct's other fields (`pub(crate)`): `Overlay` itself is
    /// `pub(super)` (visible within `lsp` and its descendants only,
    /// overlay.rs's own reach), and no consumer of this field lives
    /// outside that tree, so a wider modifier here would just be a
    /// `private_interfaces` mismatch waiting to happen.
    overlay: Option<overlay::Overlay>,
}

// Pins the `green` field doc comment above, in two halves.
//
// Half one: `DocState: !Send` fails to compile the moment that stops
// holding, by any route. `AmbiguousIfSend<()>` is a blanket impl every
// type gets; `AmbiguousIfSend<Invalid>` only lands on a `Send` type.
// `DocState: Send` would make both apply, so the trailing type parameter
// on `_marker` cannot be inferred and the crate stops compiling — there
// is no positive way to assert a negative trait bound in stable Rust, so
// this ambiguity is the mechanism, not a workaround for lacking one.
#[allow(dead_code)]
const _: fn() = || {
    trait AmbiguousIfSend<A> {
        fn _marker() {}
    }
    impl<T: ?Sized> AmbiguousIfSend<()> for T {}
    struct Invalid;
    impl<T: ?Sized + Send> AmbiguousIfSend<Invalid> for T {}
    let _ = <DocState as AmbiguousIfSend<_>>::_marker;
};

// Half two: every OTHER field is independently `Send`, which is what
// makes `green` "the one field" rather than merely "a field" — the half
// the assertion above cannot check, since adding a second `!Send` field
// leaves `DocState: !Send` (and that assertion) untouched. Destructured
// exhaustively (no `..`) and by value on purpose: a field added to
// `DocState` tomorrow fails to compile HERE until it is added below and
// classified, rather than silently escaping the check the way a list
// keyed by type alone would. A positive bound needs no ambiguity trick —
// `assert_send(v)` type-checks each binding against `T: Send` whether or
// not this function ever runs.
#[allow(dead_code)]
fn assert_every_other_docstate_field_is_send(state: DocState) {
    fn assert_send<T: Send>(_: T) {}
    let DocState {
        text,
        line_index,
        tokens,
        green: _,
        recovered_green: _,
        program,
        resolved,
        warnings,
        lint,
        fatal,
        roster,
        config_errors,
        overlay,
    } = state;
    assert_send(text);
    assert_send(line_index);
    assert_send(tokens);
    assert_send(program);
    assert_send(resolved);
    assert_send(warnings);
    assert_send(lint);
    assert_send(fatal);
    assert_send(roster);
    assert_send(config_errors);
    assert_send(overlay);
}

/// Whether the embedded stdlib's `std::` surface should be offered at all
/// (docs/tmt/project.md (schema reference)): a document with NO overlay —
/// no manifest found on the ancestor walk, a member of no target, or an
/// untitled/non-`file:` buffer — keeps today's unconditional stdlib
/// surface; only an actual project manifest declaring `"stdlib": false`
/// turns it off. Consumed by every `std::`-surfacing feature this service
/// offers — completion (`complete.rs`), and go-to-definition/hover's name
/// resolution and doc lookup (`navigate.rs`) — each gating its own
/// `std::` call site.
pub(super) fn std_enabled(state: &DocState) -> bool {
    state.overlay.as_ref().is_none_or(|o| o.stdlib)
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

/// Half-open span containment: true when `pos` sits ON a character of the
/// token. Core `Span.end` is exclusive — one past the last character — and
/// that half-open reading is the one span convention every position lookup
/// in this crate starts from. The `.tma` navigation path uses bare
/// containment: a definition answers only with the cursor on the reference
/// itself.
pub(crate) fn span_contains(span: Span, pos: Pos) -> bool {
    span.start <= pos && pos < span.end
}

/// [`span_contains`] widened by exactly the end position — the
/// cursor-touches-token rule for edit-shaped lookups: a cursor that just
/// typed a token's last character sits exactly at `span.end`, and completion
/// must still claim the token being finished. This is a deliberate policy on
/// top of the half-open convention, not an end-inclusive reading of the
/// span — the same split the PM-1 services hold (`.pmc`'s prefix anchor,
/// `.pma`'s whole-token touch). Every `.tmc` position lookup in this module
/// and the `.tma` completion path share it.
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

/// The first child of `node` — token or node — that is neither its own
/// bound doc run nor trivia (docs/core.md (syntax trees)): the keyword a
/// documented declaration's symbol extent opens at — the green tree
/// retro-wraps a bound doc run in front of the keyword, so the node's
/// own start is a line or more earlier than that. Shared by
/// `symbol_extent`, which reads only this element's start, and
/// `machine_symbol`, which reads the whole element because it IS the
/// answer to "where is this machine's name".
fn first_significant_child(node: &SyntaxNode) -> Option<SyntaxElement> {
    node.children_with_tokens().find(|e| {
        e.kind() != TmcKind::DocRun.into()
            && e.kind() != TmcKind::Whitespace.into()
            && e.kind() != TmcKind::LineComment.into()
            && e.kind() != TmcKind::BlockComment.into()
    })
}

/// A declaration's C2 extent — its keyword (or, when one precedes the
/// keyword, its leading modifier — `export`/`entry`) through the node's
/// own end, EXCLUDING a bound doc run. `.tmc` retro-wraps a doc run as
/// the declaration's own first child at seven symbol-level kinds — the
/// file/namespace-level NAMESPACE, ALPHABET, REUSE, MACHINE, and the
/// world-level STATE, GRAFT, BIND
/// (`crates/turing-machine/src/syntax/mod.rs`'s module doc) — so
/// `syntax().text_range()` on any of them starts at the `?`/`!` line
/// rather than the keyword; taking the node's range whole would widen
/// every doc-commented declaration's outline entry into its own leading
/// comment. Ports the sibling's `function_extent`
/// (`crates/post-machine/src/lsp/mod.rs`), generalized from the one kind
/// PM retro-wraps at symbol level to `.tmc`'s seven — a helper trusted
/// across kinds without checking each is exactly how a wrong assumption
/// ships here (the module doc above names all seven explicitly for the
/// same reason). `tree_symbols` and `world_symbols` are pinned
/// separately, one fixture per kind, at both levels — see
/// `a_documented_declarations_symbol_starts_at_its_keyword` and
/// `a_documented_world_members_symbol_starts_at_its_keyword`
/// (`crates/turing-machine/src/lsp/tests.rs`).
fn symbol_extent(node: &SyntaxNode) -> TextRange {
    let full = node.text_range();
    let start = first_significant_child(node).map_or(full.start, |e| e.text_range().start);
    TextRange::new(start, full.end)
}

/// Walks one item list — the file level or a namespace's own `items` —
/// into document symbols. `use` declarations are skipped. A reopened
/// namespace (`.tmc` permits it: the duplicate-binding check
/// `crate::compiler::check_duplicate_bindings` only walks `Program`'s
/// imports, never namespace occurrences) stays a separate sibling: each
/// source occurrence is its own NAMESPACE node, and this walk builds one
/// `SymbolNode` per node it visits with no merge step anywhere in it —
/// unlike `Program`, which has no namespace-block type to merge in the
/// first place, only a flat `ns: Vec<String>` path field per
/// declaration (`crate::parser::Program`).
fn tree_symbols(items: impl Iterator<Item = TopView>, index: &TextLineIndex) -> Vec<SymbolNode> {
    items
        .filter_map(|item| match item {
            TopView::Use(_) => None,
            TopView::Alphabet(a) => Some(SymbolNode {
                name: a.name_token().text().to_string(),
                kind: SymbolNodeKind::Function,
                span: index.span(symbol_extent(a.syntax())),
                selection_span: index.span(a.name_token().text_range()),
                children: Vec::new(),
            }),
            TopView::Namespace(ns) => Some(namespace_symbol(&ns, index)),
            TopView::Reuse(r) => Some(reuse_symbol(&r, index)),
            TopView::Machine(m) => Some(machine_symbol(&m, index)),
        })
        .collect()
}

fn namespace_symbol(ns: &NamespaceView, index: &TextLineIndex) -> SymbolNode {
    SymbolNode {
        name: ns.name(),
        kind: SymbolNodeKind::Namespace,
        span: index.span(symbol_extent(ns.syntax())),
        selection_span: index.span(ns.name_token().text_range()),
        children: tree_symbols(ns.items(), index),
    }
}

fn reuse_symbol(r: &ReuseView, index: &TextLineIndex) -> SymbolNode {
    SymbolNode {
        name: r.name_token().text().to_string(),
        kind: SymbolNodeKind::Function,
        span: index.span(symbol_extent(r.syntax())),
        selection_span: index.span(r.name_token().text_range()),
        children: r
            .world()
            .map(|w| world_symbols(w.syntax(), index))
            .unwrap_or_default(),
    }
}

fn machine_symbol(m: &MachineView, index: &TextLineIndex) -> SymbolNode {
    let node = m.syntax();
    let extent = symbol_extent(node);
    // A machine block has no name token of its own — unlike every other
    // kind `tree_symbols`/`world_symbols` name — so this selection span
    // is SYNTHESIZED rather than read off one: it is the `machine`
    // keyword's own token, which is exactly `first_significant_child`'s
    // answer here (the same element `symbol_extent`'s start above
    // re-bases onto), never a name the tree could supply directly.
    let selection = first_significant_child(node)
        .map_or(TextRange::new(extent.start, extent.start), |e| {
            e.text_range()
        });
    SymbolNode {
        name: "machine".to_string(),
        kind: SymbolNodeKind::Function,
        span: index.span(extent),
        selection_span: index.span(selection),
        children: m
            .world()
            .map(|w| world_symbols(w.syntax(), index))
            .unwrap_or_default(),
    }
}

/// A world body's addressable children, in document order: states,
/// named graft instances, and binds. A WORLD's own direct children are a
/// known-mixed set that also includes TAPE
/// (`crates/turing-machine/src/syntax/mod.rs`'s module doc) — an
/// expected kind that is simply never a symbol, not a wrong-shape tree —
/// and a comment is trivia, never a node at all, so both are excluded
/// by construction rather than by an explicit filter.
fn world_symbols(world: &SyntaxNode, index: &TextLineIndex) -> Vec<SymbolNode> {
    world
        .children()
        .filter_map(|child| {
            if let Some(s) = StateView::cast(child.clone()) {
                return Some(SymbolNode {
                    name: s.name_token().text().to_string(),
                    kind: SymbolNodeKind::Function,
                    span: index.span(symbol_extent(s.syntax())),
                    selection_span: index.span(s.name_token().text_range()),
                    children: Vec::new(),
                });
            }
            if let Some(g) = GraftView::cast(child.clone()) {
                let name = g.as_name()?;
                return Some(SymbolNode {
                    name: name.text().to_string(),
                    kind: SymbolNodeKind::Function,
                    span: index.span(symbol_extent(g.syntax())),
                    selection_span: index.span(name.text_range()),
                    children: Vec::new(),
                });
            }
            if let Some(b) = BindView::cast(child.clone()) {
                let name = b.as_name();
                return Some(SymbolNode {
                    name: name.text().to_string(),
                    kind: SymbolNodeKind::Function,
                    span: index.span(symbol_extent(b.syntax())),
                    selection_span: index.span(name.text_range()),
                    children: Vec::new(),
                });
            }
            None
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

        // 2. Cross-file overlay (docs/lsp.md (configuration) for the
        //    shared mtime-cache discipline; `overlay.rs` for the rest):
        //    the manifest-discovery view for this document, then its
        //    sibling/library export table, built from `self.docs` BEFORE
        //    step 5 below replaces this uri's own entry — so a sibling
        //    read never has to reason about whether it might be looking
        //    at a stale copy of the document currently being updated.
        //    Untitled / non-`file:` URIs degrade to `None` (single-file
        //    view) for free via `uri_to_path`.
        let overlay = uri_to_path(uri).and_then(|p| {
            overlay::project_view(&p, &mut self.manifest_cache)
                .map(|view| overlay::build_overlay(&view, &p, &self.docs, &mut self.sibling_cache))
        });

        // 3. Staged analysis, then — only over a clean resolve — the
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

        // 4. Lint over the resolved module when there is one. The rules also
        //    read the AST and a COMMENT-FREE token stream; the editor lexes
        //    with comment trivia, so filter to `significant_tokens` to match
        //    the batch path's stream (identical findings either way — the
        //    batch `lint()` filters the very same way).
        //    The editor already has the comment-INCLUSIVE stream too
        //    (`raw_tokens`, pre-filter) — handed over as-is, at no extra cost.
        let lint = match (
            staged.resolved.as_ref(),
            staged.program.as_ref(),
            staged.tokens.as_deref(),
        ) {
            (Some(resolved), Some(program), Some(raw_tokens)) => {
                let tokens = significant_tokens(raw_tokens);
                let ctx = LintContext {
                    resolved,
                    diagnostics: &staged.diagnostics,
                    program,
                    tokens: &tokens,
                    comment_tokens: raw_tokens,
                };
                Some(run_rules(&ctx, &effective_allow, &effective_warn))
            }
            _ => None,
        };

        // 5. Store the doc state; a failed re-analysis keeps the previous
        //    last-good roster (the names-only staleness exception).
        let prev = self.docs.remove(uri);
        let roster = match &staged.resolved {
            Some(resolved) => Some(Roster::build(resolved, staged.program.as_ref())),
            None => prev.and_then(|d| d.roster),
        };
        let mut state = DocState {
            text: text.to_string(),
            line_index: TextLineIndex::new(text),
            tokens: staged.tokens,
            green: staged.green,
            recovered_green: staged.recovered_green,
            program: staged.program,
            resolved: staged.resolved,
            warnings: staged.diagnostics,
            lint,
            fatal,
            roster,
            config_errors,
            overlay,
        };

        // 6. Cross-file diagnostics refinement (docs/tmt/cli.md
        //    (undeclared-external)): the same retain predicate `tmt build`
        //    runs over its declared link set, applied here over this
        //    document's own overlay — a bare reference the overlay
        //    defines stops being a defect of THIS document. A document
        //    with no overlay (no manifest found, member of no target, or
        //    an untitled buffer) keeps every warning untouched — the
        //    single-file honesty rule stays exact. Runs on `state.warnings`
        //    (through the disjoint `state.overlay` borrow) rather than on
        //    `staged.diagnostics` before assembly, so the stored DocState
        //    carries the REFINED set — a later consumer reading
        //    `state.warnings` must never see a warning the user never saw.
        if let Some(overlay) = state.overlay.as_ref() {
            crate::compiler::refine_undeclared(&mut state.warnings, &overlay.defined_names());
        }

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
        // Green-tier: answered as long as parsing succeeded, even if the
        // resolve or expansion stage then fatals — and, one tier further
        // down, from the RESILIENT tree on a parse-stage fatal
        // (docs/core.md (syntax trees), error recovery): the
        // declarations around a broken region keep their symbols, from
        // the CURRENT text. `None` only when lexing itself failed.
        let state = self.docs.get(uri)?;
        let green = state.green.as_ref().or(state.recovered_green.as_ref())?;
        let root = RootView::cast(SyntaxNode::new_root(Rc::clone(green)))?;
        let index = &state.line_index;
        Some(tree_symbols(root.items(), index))
    }

    fn semantic_tokens(&mut self, uri: &str) -> Option<Vec<SemToken>> {
        let state = self.docs.get(uri)?;
        tokens::semantic_tokens(state)
    }

    fn format(&mut self, uri: &str) -> Option<String> {
        // Whole-document formatting: reads the DOCSTORE's text — the
        // framework diffs the returned text against exactly what
        // `did_update` last received, never a re-read from disk. Prints
        // from the state's own green tree and line index
        // (`fmt::format_tree`) instead of re-lexing and re-parsing the
        // text they were built from; `green` is `None` exactly when a
        // lex/parse fatal exists, the same inputs on which the full
        // `format` would return `Err`.
        let state = self.docs.get(uri)?;
        let green = state.green.as_ref()?;
        Some(crate::fmt::format_tree(
            &state.text,
            green,
            &state.line_index,
        ))
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

#[cfg(test)]
mod tests;
