//! Completions (docs/lsp.md (completions)): four contexts detected from
//! the CURRENT significant token stream (WithComments minus `Comment`)
//! plus the current CST for positioning, resolved against the *names*
//! roster — `analysis`'s scopes when available, else
//! `scopes_for_completion` (the sanctioned staleness exception; names
//! only, positions always come from the current tokens).
//!
//! # The prefix/replace rule
//!
//! [`prefix_anchor`] is the single seam every context flows through: an
//! `Ident`/`Number` token whose span contains the cursor (or ends
//! exactly at it) becomes the whole-token `replace_span`; otherwise the
//! span is zero-width at the cursor. This is what keeps `replace_span`
//! always on the cursor's line and touching the cursor (the plan-1
//! review's sharp edge) — by construction, never by a follow-up check.
//! Every context receives the SAME `replace_span` and stamps it onto
//! every candidate it returns; the server never text-filters by the
//! already-typed prefix (that's the client's job over `replace_span`).
//!
//! # Context detection order
//!
//! 1. **`use` path** — the current top-level statement (walk back to
//!    the nearest `Semi`/`LBrace`/`RBrace`) starts with `Ident("use")`.
//! 2. **Qualified call path** — a `ColonColon` chain immediately left of
//!    the cursor walks back to an `At`, with at least one path segment.
//! 3. **Call position** — an `At` sits immediately left of the cursor
//!    with NO `::` chain (the zero-segment case of the same chain walk
//!    context 2 uses — see [`walk_path_chain`]).
//! 4. **Command position** — the cursor sits at a statement start, after
//!    a label `Colon`, after a `Comma` sitting at PAREN DEPTH ZERO in
//!    the current statement (a comma-group separator — see
//!    [`comma_at_depth_zero`]), or right after `Ident("goto")`. A
//!    `Comma` inside parens (`check(A, ▮`, the grammar's one
//!    comma-in-parens construct) matches none of these and falls
//!    through to no-context-match.
//!
//! No match → empty. Otherwise a matched context's candidates flow from
//! up to three tiers, in order: this file's own scopes first; then, when
//! the document is a member of a project target, the cross-file
//! [`super::overlay::Overlay`]'s sibling/library exports (docs/lsp.md
//! (configuration)) — a name the local roster already offers is never
//! shadowed or duplicated by an overlay candidate, the same
//! definition-beats-library precedent the linker itself follows; then the
//! embedded stdlib roster, unless the project's own manifest opts out
//! (`super::std_enabled`) — a document with no overlay at all (single-file,
//! untitled, or a member of no target) keeps the unconditional stdlib
//! surface.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::rc::Rc;

use mtc_core::diagnostics::{Pos, Span};
use mtc_core::lsp::{Candidate, CandidateKind};
use mtc_core::syntax::{AstNode, SyntaxNode, TextLineIndex};

use crate::compiler::{ScopeSummary, full_name};
use crate::cst::{TopItem, TopKind};
use crate::lexer::{Token, TokenKind};
use crate::parser::{FnDoc, RESERVED};
use crate::stdlib::roster;
use crate::syntax::FileView;

use super::DocState;
use super::overlay::OverlaySym;
use super::std_enabled;
use super::walk::{enclosing_function_chain, function_labels, span_contains};

/// The completion candidates for `pos` in `state`'s current document.
pub(super) fn completion(state: &DocState, pos: Pos) -> Vec<Candidate> {
    let Some(tokens) = &state.tokens else {
        return Vec::new(); // lexing itself failed
    };
    let sig: Vec<Token> = tokens
        .iter()
        .filter(|t| !matches!(t.kind, TokenKind::Comment(_)))
        .cloned()
        .collect();

    let (replace_span, cursor_idx) = prefix_anchor(&sig, pos);

    // Context 1: `use` path — checked first regardless of what sits
    // immediately left of the cursor, since a `use` list's paths can be
    // separated by commas (`use a, ns::`) which would otherwise be
    // mistaken for context 4's comma sub-case.
    if use_statement_start(&sig, cursor_idx) {
        let Some(scopes) = names_roster(state) else {
            return Vec::new();
        };
        let (segments, _) = walk_path_chain(&sig, cursor_idx);
        return if segments.is_empty() {
            use_roots(scopes, state, replace_span)
        } else {
            member_candidates(scopes, &segments, state, replace_span)
        };
    }

    // Contexts 2 and 3 share one chain walk: zero segments with an `At`
    // anchor is context 3 (bare call position); one or more segments
    // with an `At` anchor is context 2 (qualified call path).
    let (segments, chain_start) = walk_path_chain(&sig, cursor_idx);
    if chain_start > 0 && matches!(sig[chain_start - 1].kind, TokenKind::At) {
        return if segments.is_empty() {
            call_candidates(state, pos, replace_span)
        } else {
            let Some(scopes) = names_roster(state) else {
                return Vec::new();
            };
            member_candidates(scopes, &segments, state, replace_span)
        };
    }

    // Context 4: command position.
    if cursor_idx > 0 {
        match &sig[cursor_idx - 1].kind {
            TokenKind::Ident(word) if word == "goto" => {
                return label_candidates(state, pos, replace_span);
            }
            TokenKind::Semi | TokenKind::LBrace | TokenKind::RBrace | TokenKind::Colon => {
                return command_candidates(None, replace_span);
            }
            TokenKind::Comma if comma_at_depth_zero(&sig, cursor_idx - 1) => {
                let final_slot = is_final_slot(&sig, cursor_idx);
                return command_candidates(Some(final_slot), replace_span);
            }
            _ => {}
        }
    }

    Vec::new()
}

/// The names roster (docs/lsp.md (staged analysis)): `analysis`'s own
/// scopes when the current text analyzes cleanly, else the last-good
/// `scopes_for_completion` — the one sanctioned staleness exception.
/// Positions are NEVER taken from this source, only names; every caller
/// pairs it with a `replace_span`/CST computed from the CURRENT tokens.
fn names_roster(state: &DocState) -> Option<&ScopeSummary> {
    state
        .analysis
        .as_ref()
        .map(|a| &a.scopes)
        .or(state.scopes_for_completion.as_ref())
}

/// A candidate with nothing extra to say — Module/Keyword/Value kinds
/// (every Function candidate goes through `mk_function_candidate`
/// below). `detail` is always `None`, `deprecated` always `false`.
fn mk_candidate(label: &str, kind: CandidateKind, replace_span: Span) -> Candidate {
    Candidate {
        label: label.to_string(),
        kind,
        replace_span,
        insert_text: label.to_string(),
        detail: None,
        deprecated: false,
    }
}

/// A `Function` candidate carrying its target's qualified name — a
/// `ScopeSummary` map's VALUE, a roster entry's own `full_path`, or
/// (nested defs) flatten's own mangling formula reapplied over the
/// enclosing chain; never an invented derivation of the caller's own
/// (design-doc rule: "nothing invented"): `detail` is that qualified
/// name when it differs from the bare `label` (a cross-namespace or
/// nested candidate), `None` when they're the same (an unnamespaced
/// top-level candidate — nothing to add). `deprecated` is
/// `Analysis.docs`' own tag, keyed by the same
/// qualified name; `false` when `docs` is unavailable (stale-scopes
/// completion, docs/lsp.md (staged analysis)) or the name isn't
/// documented at all — both fall out of `Option::and_then` naturally,
/// no special-casing (std candidates included: `docs` here is always
/// the REQUESTING document's own map, which never carries a `std::…`
/// key — a `std::` name always misses and reads `false`. This is a
/// deliberate scope stop, not a gap: hover's own `std::` lookup falls
/// back to `crate::stdlib::docs()`, but the embedded stdlib ships
/// nothing deprecated, so wiring that same fallback in here would add
/// an unexercised path with no observable behavior to pin).
fn mk_function_candidate(
    label: &str,
    qualified: &str,
    docs: Option<&HashMap<String, FnDoc>>,
    replace_span: Span,
) -> Candidate {
    let deprecated = docs
        .and_then(|docs| docs.get(qualified))
        .is_some_and(|doc| doc.deprecated.is_some());
    Candidate {
        label: label.to_string(),
        kind: CandidateKind::Function,
        replace_span,
        insert_text: label.to_string(),
        detail: (qualified != label).then(|| qualified.to_string()),
        deprecated,
    }
}

/// An overlay-sourced `Function` candidate: the SAME label/detail shape as
/// [`mk_function_candidate`] (`detail` is the qualified name only when it
/// differs from `label`), but `deprecated` comes from the contributing
/// SIBLING's own `OverlaySym.doc` rather than this document's
/// `Analysis.docs` — the two are different documents' doc maps, and this
/// document's own map never holds a sibling's key (mirrors
/// `mk_function_candidate`'s std-candidate scope stop, but for a REAL
/// answer instead of a deliberately absent one: a sibling's `.pmc` doc
/// comment is exactly as available here as it is to that sibling's own
/// hover). `sym` is `None` only when `full` names something the overlay's
/// `members` index registered but its `symbols` table doesn't carry
/// (never happens in practice — both are populated together by the same
/// `insert_export` call — but the caller has no easy proof of that at the
/// type level).
fn mk_overlay_candidate(
    label: &str,
    qualified: &str,
    sym: Option<&OverlaySym>,
    replace_span: Span,
) -> Candidate {
    let deprecated = sym
        .and_then(|sym| sym.doc.as_ref())
        .is_some_and(|doc| doc.deprecated.is_some());
    Candidate {
        label: label.to_string(),
        kind: CandidateKind::Function,
        replace_span,
        insert_text: label.to_string(),
        detail: (qualified != label).then(|| qualified.to_string()),
        deprecated,
    }
}

/// The prefix/replace rule: an `Ident`/`Number` token whose span
/// contains `pos` (or ends exactly at it) is the whole prefix, and
/// `cursor_idx` is that token's own index. Otherwise `pos` sits between
/// tokens (or at the very start/end of the stream) — the span is
/// zero-width at `pos`, and `cursor_idx` is the index of the first
/// token starting at or after `pos` (`sig.len()` if none), i.e. exactly
/// where a new token would land. Either way, `sig[cursor_idx - 1]` (when
/// `cursor_idx > 0`) is "the token immediately left of the cursor" every
/// context below keys on.
fn prefix_anchor(sig: &[Token], pos: Pos) -> (Span, usize) {
    for (i, t) in sig.iter().enumerate() {
        if matches!(t.kind, TokenKind::Ident(_) | TokenKind::Number(_, _)) {
            let span = t.span();
            if pos >= span.start && pos <= span.end {
                return (span, i);
            }
        }
    }
    for (i, t) in sig.iter().enumerate() {
        if t.span().start >= pos {
            return (
                Span {
                    start: pos,
                    end: pos,
                },
                i,
            );
        }
    }
    (
        Span {
            start: pos,
            end: pos,
        },
        sig.len(),
    )
}

/// Walks strictly backward from `cursor_idx` over a chain of `Ident ::`
/// pairs — the qualified path already typed before the cursor's own
/// segment. Returns the segments in left-to-right order and the index
/// right after the chain's last consumed token (`cursor_idx` itself
/// when there is no chain at all); `sig[result.1 - 1]` is the token
/// immediately before the chain, the "anchor" contexts 1-3 branch on.
fn walk_path_chain(sig: &[Token], cursor_idx: usize) -> (Vec<String>, usize) {
    let mut segments = Vec::new();
    let mut i = cursor_idx;
    while i >= 2 {
        if !matches!(sig[i - 1].kind, TokenKind::ColonColon) {
            break;
        }
        let TokenKind::Ident(name) = &sig[i - 2].kind else {
            break;
        };
        segments.push(name.clone());
        i -= 2;
    }
    segments.reverse();
    (segments, i)
}

/// Whether the current top-level item (walking back from `cursor_idx`
/// to the nearest `Semi`/`LBrace`/`RBrace`, or the start of the stream)
/// begins with `Ident("use")`, with at least the `use` token itself
/// strictly before `cursor_idx` — a bare cursor sitting ON the word
/// "use" being typed does not count yet (nothing marks it as a `use`
/// statement until something follows).
fn use_statement_start(sig: &[Token], cursor_idx: usize) -> bool {
    let mut start = 0;
    let mut i = cursor_idx;
    while i > 0 {
        i -= 1;
        if matches!(
            sig[i].kind,
            TokenKind::Semi | TokenKind::LBrace | TokenKind::RBrace
        ) {
            start = i + 1;
            break;
        }
    }
    start < cursor_idx && matches!(&sig[start].kind, TokenKind::Ident(w) if w == "use")
}

/// Whether the `Comma` at `comma_idx` sits at PAREN DEPTH ZERO within
/// its statement — a genuine command-GROUP separator — as opposed to an
/// internal comma inside a command's own argument list (the only case
/// in this grammar: `check(A, B)`'s arm-separating comma). Walks
/// BACKWARD from `comma_idx` toward the nearest statement boundary
/// (`Semi`/`LBrace`/`RBrace` seen at depth zero) or the stream start,
/// tracking paren balance in reverse — `RParen` closes-early/`+1`,
/// `LParen` opens-early/`-1`. An `LParen` that drives the running
/// balance negative has no matching `RParen` between it and the comma,
/// meaning the comma sits INSIDE that paren, not at the statement's top
/// level. Reaching the boundary (or the stream start) with the balance
/// still at zero means every paren seen along the way was already
/// closed before the comma, so the comma IS a group separator. This is
/// the entry gate `completion`'s `Comma` arm checks before treating the
/// comma as a group slot at all — [`is_final_slot`] below is only ever
/// reached once this has already returned `true`.
fn comma_at_depth_zero(sig: &[Token], comma_idx: usize) -> bool {
    let mut depth: i32 = 0;
    let mut i = comma_idx;
    while i > 0 {
        i -= 1;
        match sig[i].kind {
            TokenKind::RParen => depth += 1,
            TokenKind::LParen => {
                depth -= 1;
                if depth < 0 {
                    return false;
                }
            }
            TokenKind::Semi | TokenKind::LBrace | TokenKind::RBrace if depth == 0 => {
                return true;
            }
            _ => {}
        }
    }
    true // stream start reached with every paren along the way balanced
}

/// Whether the comma slot starting at `scan_from` is the group's FINAL
/// slot: scanning forward, the next `Comma` or `Semi` seen AT PAREN
/// DEPTH ZERO decides it — a `Semi` first means final, a `Comma` first
/// means more items follow. The ENTRY comma itself is already known to
/// sit at depth zero — [`comma_at_depth_zero`] gates every call site —
/// so this function's own forward depth tracking exists for what lies
/// AHEAD in the same group: a later slot's own `check(a, b)` (or any
/// future call's argument comma) must never be mistaken for the next
/// group-continuation comma while scanning past it. Running off the end
/// without finding either (an unterminated statement mid-edit) defaults
/// to final — the permissive choice.
fn is_final_slot(sig: &[Token], scan_from: usize) -> bool {
    let mut depth: i32 = 0;
    for t in &sig[scan_from..] {
        match t.kind {
            TokenKind::LParen => depth += 1,
            TokenKind::RParen => depth -= 1,
            TokenKind::Semi if depth == 0 => return true,
            TokenKind::Comma if depth == 0 => return false,
            _ => {}
        }
    }
    true
}

/// The enclosing namespace path at `pos` — walks only `Namespace`
/// blocks (a function's own extent never changes it; only `namespace {
/// }` blocks add a `::` segment), recursively, innermost match wins.
/// Unlike `walk::enclosing_function_chain`, this walks a DIFFERENT node
/// kind (namespace blocks, never function extents) toward a different
/// result shape (a path of names, not a chain of CST nodes) — its own
/// walk, not a duplicate of the shared one.
fn enclosing_ns_path(items: &[TopItem], pos: Pos) -> Vec<String> {
    for item in items {
        if let TopKind::Namespace(ns) = &item.kind
            && span_contains(ns.span, pos)
        {
            let mut path = vec![ns.name.clone()];
            path.extend(enclosing_ns_path(&ns.items, pos));
            return path;
        }
    }
    Vec::new()
}

/// Context 1/2's shared member lookup for an exact namespace `path`:
/// `path == ["std"]` is special-cased since `std` is magic — it never has
/// a `ScopeSummary` entry of its own, so the generic lookup below (which
/// only ever reads `scopes`/`overlay.members`) would answer empty for it
/// regardless. The special case still consults the overlay FIRST, same
/// as every other overlay leg in this function: a sibling's own
/// `namespace std { export … }` registers under `overlay.members[["std"]]`
/// exactly like any other namespaced export (`overlay.rs::insert_export`
/// mangles it the same way), so it is offered — and, via `seen`, wins
/// first — before the embedded stdlib roster fills in the rest, gated on
/// [`std_enabled`]: a project opting out of the stdlib only drops the
/// ROSTER half, never a sibling's own overlay-registered names (those are
/// ordinary linked code, unaffected by the stdlib toggle). Unlike the
/// generic path below, this doesn't also scan for overlay child
/// namespaces one segment deeper (a sibling's `std::sub::thing` won't
/// offer `sub` here) — `std`'s own members are the only thing this
/// narrow special case is for; a project nesting real namespaces under a
/// literal `std` is exotic enough that the generic path's fuller
/// treatment isn't worth duplicating into this branch.
///
/// Otherwise: `scopes.defs` under the exact path (Function kind) plus
/// child namespaces exactly one segment deeper, derived the same way
/// `use_roots` derives roots (Module kind); the overlay contributes the
/// SAME two shapes at this same seam — its own child namespaces one
/// segment deeper (`overlay.rs::insert_export` registers EVERY
/// intermediate namespace level along an export's path, not only its own
/// leaf level, so a sibling's `outer::inner::f` registers a key at
/// `["outer"]` — mapping `inner` onward — in addition to its own leaf key
/// at `["outer","inner"]`; without this scan, typing `outer::` would
/// offer nothing even though `use_roots` already offered `outer` as a
/// root, and the scan keeps working at any nesting depth since every
/// ancestor level is registered), and its own `members` entry for this
/// EXACT path (Function kind, docs/lsp.md (project overlay); when the exact
/// path is itself an intermediate namespace level rather than a leaf, its
/// bare names were already offered as Module kind by the child-namespace
/// scan above and `seen` skips the duplicate here). Every overlay name
/// whose bare label a local def or child namespace already produced is
/// skipped (`seen`): a local name always wins, the same
/// definition-beats-library precedent the linker itself follows. Sorted
/// by label for a deterministic result — the underlying maps are
/// hash-ordered. `docs` (`None` when analysis itself is stale/absent)
/// backs every LOCAL Function candidate's `detail`/`deprecated` — see
/// `mk_function_candidate`; an overlay candidate's `deprecated` instead
/// comes from that sibling's own `OverlaySym.doc` — see
/// `mk_overlay_candidate`.
fn member_candidates(
    scopes: &ScopeSummary,
    path: &[String],
    state: &DocState,
    replace_span: Span,
) -> Vec<Candidate> {
    let docs = state.analysis.as_ref().map(|a| &a.docs);

    if path.len() == 1 && path[0] == "std" {
        let mut out: Vec<Candidate> = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();

        if let Some(overlay) = state.overlay.as_ref()
            && let Some(members) = overlay.members.get(path)
        {
            for (bare, full) in members {
                if seen.insert(bare.clone()) {
                    out.push(mk_overlay_candidate(
                        bare,
                        full,
                        overlay.symbols.get(full),
                        replace_span,
                    ));
                }
            }
        }

        if std_enabled(state) {
            for entry in roster() {
                let name = entry
                    .full_path
                    .strip_prefix("std::")
                    .unwrap_or(&entry.full_path);
                if seen.insert(name.to_string()) {
                    out.push(mk_function_candidate(
                        name,
                        &entry.full_path,
                        docs,
                        replace_span,
                    ));
                }
            }
        }

        out.sort_by(|a, b| a.label.cmp(&b.label));
        return out;
    }

    let mut out: Vec<Candidate> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    if let Some(defs) = scopes.defs.get(path) {
        for (name, full) in defs {
            seen.insert(name.clone());
            out.push(mk_function_candidate(name, full, docs, replace_span));
        }
    }
    let mut children: BTreeSet<&str> = BTreeSet::new();
    for key in scopes.defs.keys().chain(scopes.bindings.keys()) {
        if key.len() == path.len() + 1 && key.starts_with(path) {
            children.insert(key[path.len()].as_str());
        }
    }
    for name in children {
        seen.insert(name.to_string());
        out.push(mk_candidate(name, CandidateKind::Module, replace_span));
    }

    if let Some(overlay) = state.overlay.as_ref() {
        let mut overlay_children: BTreeSet<&str> = BTreeSet::new();
        for key in overlay.members.keys() {
            if key.len() == path.len() + 1 && key.starts_with(path) {
                overlay_children.insert(key[path.len()].as_str());
            }
        }
        for name in overlay_children {
            if seen.insert(name.to_string()) {
                out.push(mk_candidate(name, CandidateKind::Module, replace_span));
            }
        }
    }

    if let Some(overlay) = state.overlay.as_ref()
        && let Some(members) = overlay.members.get(path)
    {
        for (bare, full) in members {
            if seen.insert(bare.clone()) {
                out.push(mk_overlay_candidate(
                    bare,
                    full,
                    overlay.symbols.get(full),
                    replace_span,
                ));
            }
        }
    }

    out.sort_by(|a, b| a.label.cmp(&b.label));
    out
}

/// Context 1's no-`::` case: the file's namespace roots (the distinct
/// first segments of `scopes.defs`/`scopes.bindings` keys), the first
/// segment of every NAMESPACED cross-file overlay export (a `members` key
/// with a non-empty path — a bare, unnamespaced overlay export has
/// nothing to offer as a `use`-able root), plus `std` when
/// [`std_enabled`] says the project hasn't opted out.
fn use_roots(scopes: &ScopeSummary, state: &DocState, replace_span: Span) -> Vec<Candidate> {
    let mut names: BTreeSet<String> = scopes
        .defs
        .keys()
        .chain(scopes.bindings.keys())
        .filter_map(|k| k.first().cloned())
        .collect();
    if let Some(overlay) = state.overlay.as_ref() {
        for path in overlay.members.keys() {
            if let Some(first) = path.first() {
                names.insert(first.clone());
            }
        }
    }
    if std_enabled(state) {
        names.insert("std".to_string());
    }
    names
        .into_iter()
        .map(|name| mk_candidate(&name, CandidateKind::Module, replace_span))
        .collect()
}

/// Context 3: visible callables with shadowing, assembled in flatten's
/// own resolve order (`compiler.rs::flatten`'s `resolve` — nested scopes
/// innermost-outward, THEN each enclosing namespace prefix longest-first
/// with that level's defs before its bindings) — first-wins per bare
/// name via `seen`, so a definition always outranks a same-named import,
/// and an inner nested def always outranks an outer one. Then (c) the std
/// roster (gated on [`std_enabled`]) and (d) the cross-file overlay's own
/// top-level bare exports ride in after, subject to the same `seen`
/// shadow-check — a local name still always wins; both then contribute
/// their remaining, `::`-qualified entries as fully qualified paths. Bare
/// names and qualified names never collide with EACH OTHER (a bare name
/// never contains `::`), but (c)'s and (d)'s own qualified entries CAN
/// collide with each other — a sibling/library's own `namespace std {
/// export … }` mangles to the same `std::…` label a roster entry already
/// carries (docs/pmt/project.md (libraries): the embedded stdlib links
/// as an ordinary library, so a same-named user object shadows it). (c)
/// resolves that by skipping any roster entry the overlay already owns
/// (`overlay.symbols.contains_key`) — (d) still emits every overlay
/// qualified entry unconditionally, so the shadowing sibling's own entry
/// rides in from there instead of being duplicated or losing to (c)'s.
fn call_candidates(state: &DocState, pos: Pos, replace_span: Span) -> Vec<Candidate> {
    let Some(scopes) = names_roster(state) else {
        return Vec::new();
    };
    let docs = state.analysis.as_ref().map(|a| &a.docs);
    let mut seen: HashSet<String> = HashSet::new();
    let mut out: Vec<Candidate> = Vec::new();

    // The enclosing namespace path, shared by (a)'s qualified-name
    // reconstruction and (b)'s prefix walk. Falls back to the top-level
    // scope ([]) when the CST is unavailable.
    let ns_path: Vec<String> = state
        .cst
        .as_ref()
        .map(|cst| enclosing_ns_path(&cst.items, pos))
        .unwrap_or_default();

    // (a) nested defs of the enclosing function chain, innermost
    // outward, hoisted (a function's OWN direct nested children, via
    // `FunctionView::nested`, regardless of their position relative to
    // `pos`). Unavailable without the green tree — skipped, not
    // substituted, per spec. Each chain level's qualified name is
    // rebuilt with flatten's OWN formula — `compiler::full_name` for the
    // top level, then a `.` segment per nesting level
    // (`compiler.rs::flatten`'s `emit`) — never a re-derivation, so a
    // nested candidate's qualified name matches `Analysis.docs`' key
    // exactly.
    if let Some(green) = &state.green {
        let root = SyntaxNode::new_root(Rc::clone(green));
        let file = FileView::cast(root).expect("root is FILE");
        let index = TextLineIndex::new(&state.text);
        let offset = index.offset(pos);
        let chain = enclosing_function_chain(&file, offset);
        let mut quals: Vec<String> = Vec::with_capacity(chain.len());
        for (i, f) in chain.iter().enumerate() {
            let header = f.header();
            let name = header.name.text();
            quals.push(match i {
                0 => full_name(&ns_path, name),
                _ => format!("{}.{name}", quals[i - 1]),
            });
        }
        for (f, qual) in chain.iter().zip(&quals).rev() {
            for nested in f.nested() {
                let name = nested.header().name.text().to_string();
                if seen.insert(name.clone()) {
                    out.push(mk_function_candidate(
                        &name,
                        &format!("{qual}.{name}"),
                        docs,
                        replace_span,
                    ));
                }
            }
        }
    }

    // (b) per enclosing namespace prefix, longest first: that level's
    // defs, then its bindings.
    for k in (0..=ns_path.len()).rev() {
        let prefix = &ns_path[..k];
        if let Some(defs) = scopes.defs.get(prefix) {
            for (name, full) in defs {
                if seen.insert(name.clone()) {
                    out.push(mk_function_candidate(name, full, docs, replace_span));
                }
            }
        }
        if let Some(bindings) = scopes.bindings.get(prefix) {
            for (name, (_, full)) in bindings {
                if seen.insert(name.clone()) {
                    out.push(mk_function_candidate(name, full, docs, replace_span));
                }
            }
        }
    }

    // (c) the std roster, as qualified paths — label already IS the
    // qualified name here, so `detail` comes back `None` by
    // construction (nothing to add beyond the label itself) — gated on
    // `std_enabled`: a project opting out of the stdlib gets none of
    // these. A roster entry the overlay already owns (a sibling/library's
    // own `namespace std { export … }`, this function's own doc comment)
    // is skipped here — (d) below emits the overlay's own entry for that
    // same qualified label instead, first-wins, the overlay's copy
    // shadowing the embedded one exactly as the linker's own
    // sources-before-libraries order does.
    if std_enabled(state) {
        for entry in roster() {
            if state
                .overlay
                .as_ref()
                .is_some_and(|o| o.symbols.contains_key(&entry.full_path))
            {
                continue;
            }
            out.push(mk_function_candidate(
                &entry.full_path,
                &entry.full_path,
                docs,
                replace_span,
            ));
        }
    }

    // (d) the cross-file overlay (docs/lsp.md (project overlay)): its
    // top-level BARE exports (`members[[]]`, label = bare name) subject
    // to the same `seen` shadow-check as every local name above — a local
    // def/import always wins; then every NAMESPACED overlay symbol (a
    // `::`-qualified key of `overlay.symbols`) as a qualified-label
    // candidate, mirroring (c)'s std-roster shape exactly: label already
    // IS the qualified name, a space that bare local names never occupy (no
    // bare name contains `::`), so it never needs — or competes for —
    // `seen`. This is also where a shadowed `std::…` name's own entry
    // comes from — (c) above already skipped it — so nothing further is
    // needed here to keep the two in sync.
    if let Some(overlay) = state.overlay.as_ref() {
        let empty_path: Vec<String> = Vec::new();
        if let Some(top_level) = overlay.members.get(&empty_path) {
            for (bare, full) in top_level {
                if seen.insert(bare.clone()) {
                    out.push(mk_overlay_candidate(
                        bare,
                        full,
                        overlay.symbols.get(full),
                        replace_span,
                    ));
                }
            }
        }
        let mut qualified: Vec<(&String, &OverlaySym)> = overlay
            .symbols
            .iter()
            .filter(|(name, _)| name.contains("::"))
            .collect();
        qualified.sort_by(|a, b| a.0.cmp(b.0));
        for (full, sym) in qualified {
            out.push(mk_overlay_candidate(full, full, Some(sym), replace_span));
        }
    }

    out
}

/// Context 4's base offer: the eight command words, cited from
/// `parser::RESERVED` (never a hardcoded copy). `final_slot` is `None`
/// at a plain statement start / after a label colon (unfiltered);
/// `Some(final)` after a comma, filtering per the parser's own
/// comma-group rules (`parser.rs`'s `statement`/`item`): `goto` never
/// appears in a group at all, `check`/`halt` only in the final slot.
fn command_candidates(final_slot: Option<bool>, replace_span: Span) -> Vec<Candidate> {
    RESERVED
        .iter()
        .filter(|word| match final_slot {
            None => true,
            Some(true) => **word != "goto",
            Some(false) => !matches!(**word, "goto" | "check" | "halt"),
        })
        .map(|word| mk_candidate(word, CandidateKind::Keyword, replace_span))
        .collect()
}

/// Context 4's `after goto` sub-case: the innermost enclosing function's
/// OWN labels (labels are function-scoped, same as `navigate.rs`'s
/// `label_span`), via `walk::function_labels`' shared scan, as Value
/// candidates whose label is the decimal value. No green tree → no
/// labels (not a hardcoded fallback list).
fn label_candidates(state: &DocState, pos: Pos, replace_span: Span) -> Vec<Candidate> {
    let Some(green) = &state.green else {
        return Vec::new();
    };
    let root = SyntaxNode::new_root(Rc::clone(green));
    let file = FileView::cast(root).expect("root is FILE");
    let index = TextLineIndex::new(&state.text);
    let offset = index.offset(pos);
    let chain = enclosing_function_chain(&file, offset);
    let Some(f) = chain.last() else {
        return Vec::new();
    };
    let mut seen: HashSet<u32> = HashSet::new();
    let mut out = Vec::new();
    for label in function_labels(f, &index) {
        if seen.insert(label.value) {
            out.push(mk_candidate(
                &label.value.to_string(),
                CandidateKind::Value,
                replace_span,
            ));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU32, Ordering};

    use super::super::PmcLanguageService;
    use super::*;
    use mtc_core::lsp::LanguageService;

    const URI: &str = "untitled:Complete-1";

    /// A fresh scratch directory under `std::env::temp_dir()`, unique per
    /// call (process id + an atomic counter — this crate has no tempfile
    /// dependency, matching the zero-new-deps constraint; house
    /// convention has no shared test-support module, so each file defines
    /// its own local helper — mirrors `overlay.rs`'s and `lsp/mod.rs`'s
    /// own copies).
    fn unique_tmp_dir(label: &str) -> PathBuf {
        static N: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "pmt-complete-{label}-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A real client's own `file:` URI construction, byte-for-byte —
    /// `crate::stdlib::path_to_file_uri` percent-encodes.
    fn file_uri(path: &Path) -> String {
        crate::stdlib::path_to_file_uri(path)
    }

    /// 1-based (line, col) of the first byte of `anchor`'s occurrence in
    /// `src`, plus a `skip` char offset into the anchor. Pure ASCII
    /// fixtures throughout, so byte offsets double as char offsets.
    fn pos_after(src: &str, anchor: &str, skip: usize) -> Pos {
        let start = src
            .find(anchor)
            .unwrap_or_else(|| panic!("{anchor:?} not found in fixture"));
        pos_at_byte(src, start + skip)
    }

    fn pos_at(src: &str, anchor: &str) -> Pos {
        pos_after(src, anchor, 0)
    }

    fn pos_at_byte(src: &str, byte_idx: usize) -> Pos {
        let prefix = &src[..byte_idx];
        let line = prefix.matches('\n').count() as u32 + 1;
        let col = match prefix.rfind('\n') {
            Some(nl) => prefix[nl + 1..].chars().count() as u32 + 1,
            None => prefix.chars().count() as u32 + 1,
        };
        Pos { line, col }
    }

    fn span_of(src: &str, anchor: &str) -> Span {
        let start = pos_at(src, anchor);
        Span::new(
            start.line,
            start.col,
            start.line,
            start.col + anchor.chars().count() as u32,
        )
    }

    /// The `len_chars`-character span starting `skip` characters into
    /// `anchor`'s first occurrence — for pulling out a specific
    /// sub-token's span from a longer, uniquely-identifying anchor (e.g.
    /// the `sib` inside `"@sib()"`, distinct from `sib`'s OWN definition
    /// elsewhere in the same fixture).
    fn span_after(src: &str, anchor: &str, skip: usize, len_chars: usize) -> Span {
        let start = pos_after(src, anchor, skip);
        Span::new(
            start.line,
            start.col,
            start.line,
            start.col + len_chars as u32,
        )
    }

    fn labels(candidates: &[Candidate]) -> BTreeSet<String> {
        candidates.iter().map(|c| c.label.clone()).collect()
    }

    #[test]
    fn span_contains_excludes_a_position_exactly_at_the_end() {
        // Half-open contract (this module's `span_contains` doc comment
        // — the CST-extent variant, distinct from `prefix_anchor`'s
        // deliberately wider `<=` touches-the-end rule tested below):
        // `end` is one past the last contained position.
        let span = Span::new(1, 1, 1, 5);
        assert!(
            span_contains(span, Pos { line: 1, col: 1 }),
            "start is inclusive"
        );
        assert!(
            span_contains(span, Pos { line: 1, col: 4 }),
            "last contained column"
        );
        assert!(
            !span_contains(span, Pos { line: 1, col: 5 }),
            "end is exclusive"
        );
    }

    // --- Call position (context 3) ---

    const CALL_TOP_FIXTURE: &str = "use ext;\nsib() { left; }\nexport main() {\n    @sib();\n}\n";

    #[test]
    fn call_position_top_level_offers_defs_imports_and_std_paths() {
        let mut service = PmcLanguageService::new();
        service.did_update(URI, CALL_TOP_FIXTURE);

        let pos = pos_after(CALL_TOP_FIXTURE, "@sib()", 1);
        let candidates = service.completion(URI, pos);

        assert!(
            candidates
                .iter()
                .any(|c| c.label == "sib" && c.kind == CandidateKind::Function)
        );
        assert!(
            candidates
                .iter()
                .any(|c| c.label == "ext" && c.kind == CandidateKind::Function)
        );
        assert!(candidates.iter().any(|c| c.label == "std::goToEnd"));
        let std_count = candidates
            .iter()
            .filter(|c| c.label.starts_with("std::"))
            .count();
        assert_eq!(std_count, 11, "the whole std roster, qualified");
        for c in &candidates {
            assert_eq!(c.insert_text, c.label);
            assert_eq!(c.replace_span, span_after(CALL_TOP_FIXTURE, "@sib()", 1, 3));
        }
    }

    const CALL_SHADOW_FIXTURE: &str =
        "use shadow;\nshadow() { right; }\nexport main() {\n    @shadow();\n}\n";

    #[test]
    fn call_position_def_shadows_same_named_import() {
        let mut service = PmcLanguageService::new();
        service.did_update(URI, CALL_SHADOW_FIXTURE);

        let pos = pos_after(CALL_SHADOW_FIXTURE, "@shadow()", 1);
        let candidates = service.completion(URI, pos);

        assert_eq!(
            candidates.iter().filter(|c| c.label == "shadow").count(),
            1,
            "def and import share a bare name — exactly one candidate: {candidates:?}"
        );
    }

    #[test]
    fn call_position_top_level_candidate_carries_no_detail_when_qualified_equals_label() {
        // `sib` is unnamespaced and top-level: its fully-qualified name
        // (`compiler.rs::full_name` on an empty ns path) IS its bare
        // label — `detail` must be `None`, not `Some("sib")`.
        let mut service = PmcLanguageService::new();
        service.did_update(URI, CALL_TOP_FIXTURE);

        let pos = pos_after(CALL_TOP_FIXTURE, "@sib()", 1);
        let candidates = service.completion(URI, pos);

        let sib = candidates
            .iter()
            .find(|c| c.label == "sib")
            .expect("sib is offered");
        assert_eq!(sib.detail, None, "{sib:?}");
        assert!(!sib.deprecated, "{sib:?}");
    }

    // `old` is a documented, `! [deprecated]` NESTED function — its
    // `Analysis.docs` key is the dot-mangled `main.old`, reachable only
    // if branch (a) rebuilds the qualified name with flatten's formula.
    const CALL_NESTED_DOC_FIXTURE: &str = "\
export main() {
    ?old helper.
    ! [deprecated] use fresh.
    old() { right; }
    fresh() { right; }
    @x();
}
";

    #[test]
    fn call_position_nested_deprecated_candidate_is_tagged() {
        let mut service = PmcLanguageService::new();
        service.did_update(URI, CALL_NESTED_DOC_FIXTURE);

        let pos = pos_after(CALL_NESTED_DOC_FIXTURE, "@x()", 1);
        let candidates = service.completion(URI, pos);

        let old = candidates
            .iter()
            .find(|c| c.label == "old")
            .expect("old is offered");
        assert!(old.deprecated, "{old:?}");
        assert_eq!(old.detail, Some("main.old".to_string()));
    }

    // A nested function inside a NAMESPACED top-level function — the
    // qualified name crosses both mangling axes (`::` then `.`), so the
    // detail proves branch (a) seeds the chain with `full_name` (the ns
    // path), not just the bare top-level name.
    const CALL_NESTED_NS_FIXTURE: &str = "\
namespace ns {
export outer() {
    helper() { right; }
    @x();
}
}
";

    #[test]
    fn call_position_nested_candidate_detail_is_the_dot_mangled_qualified_name() {
        let mut service = PmcLanguageService::new();
        service.did_update(URI, CALL_NESTED_NS_FIXTURE);

        let pos = pos_after(CALL_NESTED_NS_FIXTURE, "@x()", 1);
        let candidates = service.completion(URI, pos);

        let helper = candidates
            .iter()
            .find(|c| c.label == "helper")
            .expect("helper is offered");
        assert_eq!(helper.detail, Some("ns::outer.helper".to_string()));
        assert!(!helper.deprecated, "{helper:?}");
    }

    const CALL_INNER_SHADOWS_FIXTURE: &str =
        "foo() { right; }\nexport main() {\n    foo() { left; }\n    @foo();\n}\n";

    #[test]
    fn call_position_inner_nested_def_shadows_outer_top_level_def() {
        let mut service = PmcLanguageService::new();
        service.did_update(URI, CALL_INNER_SHADOWS_FIXTURE);

        let pos = pos_after(CALL_INNER_SHADOWS_FIXTURE, "@foo()", 1);
        let candidates = service.completion(URI, pos);

        assert_eq!(
            candidates.iter().filter(|c| c.label == "foo").count(),
            1,
            "the nested foo shadows the top-level one — one candidate: {candidates:?}"
        );
    }

    const CALL_HOISTED_FIXTURE: &str = "export main() {\n    @x();\n    helper() { right; }\n}\n";

    #[test]
    fn call_position_nested_defs_are_hoisted_regardless_of_source_position() {
        let mut service = PmcLanguageService::new();
        service.did_update(URI, CALL_HOISTED_FIXTURE);

        let pos = pos_after(CALL_HOISTED_FIXTURE, "@x()", 1);
        let candidates = service.completion(URI, pos);

        assert!(
            candidates
                .iter()
                .any(|c| c.label == "helper" && c.kind == CandidateKind::Function),
            "helper is defined BELOW the cursor's statement but is still hoisted: {candidates:?}"
        );
    }

    // --- `use` path (context 1) ---

    const USE_ROOTS_FIXTURE: &str =
        "namespace ns {\n    helper() { right; }\n}\nuse x;\nexport main() { right; }\n";

    #[test]
    fn use_path_with_no_colon_colon_offers_roots_and_std() {
        let mut service = PmcLanguageService::new();
        service.did_update(URI, USE_ROOTS_FIXTURE);

        let pos = pos_after(USE_ROOTS_FIXTURE, "use x", 4);
        let candidates = service.completion(URI, pos);

        assert_eq!(
            labels(&candidates),
            BTreeSet::from(["ns".to_string(), "std".to_string()])
        );
        assert!(candidates.iter().all(|c| c.kind == CandidateKind::Module));
    }

    const USE_STD_FIXTURE: &str = "use std::x;\nexport main() { right; }\n";

    #[test]
    fn use_path_under_std_offers_the_eleven_routine_names() {
        let mut service = PmcLanguageService::new();
        service.did_update(URI, USE_STD_FIXTURE);

        let pos = pos_after(USE_STD_FIXTURE, "std::", 5);
        let candidates = service.completion(URI, pos);

        assert_eq!(candidates.len(), 11, "{candidates:?}");
        assert!(candidates.iter().all(|c| c.kind == CandidateKind::Function));
        let expected: BTreeSet<String> = roster()
            .iter()
            .map(|e| e.full_path.strip_prefix("std::").unwrap().to_string())
            .collect();
        assert_eq!(labels(&candidates), expected);
    }

    const USE_NS_FIXTURE: &str = "namespace ns {\n    helper() { right; }\n    namespace inner {\n        thing() { right; }\n    }\n}\nuse ns::x;\nexport main() { right; }\n";

    #[test]
    fn use_path_under_a_namespace_offers_its_members() {
        let mut service = PmcLanguageService::new();
        service.did_update(URI, USE_NS_FIXTURE);

        let pos = pos_after(USE_NS_FIXTURE, "use ns::", 8);
        let candidates = service.completion(URI, pos);

        assert_eq!(
            labels(&candidates),
            BTreeSet::from(["helper".to_string(), "inner".to_string()])
        );
        assert!(
            candidates
                .iter()
                .any(|c| c.label == "helper" && c.kind == CandidateKind::Function)
        );
        assert!(
            candidates
                .iter()
                .any(|c| c.label == "inner" && c.kind == CandidateKind::Module)
        );
    }

    // --- Qualified call path (context 2) ---

    const QUALIFIED_STD_FIXTURE: &str = "export main() {\n    @std::x();\n}\n";

    #[test]
    fn qualified_call_under_std_offers_routine_names() {
        let mut service = PmcLanguageService::new();
        service.did_update(URI, QUALIFIED_STD_FIXTURE);

        let pos = pos_after(QUALIFIED_STD_FIXTURE, "std::", 5);
        let candidates = service.completion(URI, pos);

        assert_eq!(candidates.len(), 11, "{candidates:?}");
        assert!(candidates.iter().any(|c| c.label == "goToEnd"));
    }

    #[test]
    fn qualified_call_std_members_carry_their_qualified_detail_and_no_tag() {
        // The std branch of `member_candidates`: label is the bare
        // routine name, `detail` its full `std::` path (they always
        // differ); never deprecated — `docs` here is the requesting
        // document's own map, which never carries a `std::…` key (see
        // `mk_function_candidate`'s doc comment), and the embedded
        // stdlib ships nothing deprecated regardless.
        let mut service = PmcLanguageService::new();
        service.did_update(URI, QUALIFIED_STD_FIXTURE);

        let pos = pos_after(QUALIFIED_STD_FIXTURE, "std::", 5);
        let candidates = service.completion(URI, pos);

        let go_to_end = candidates
            .iter()
            .find(|c| c.label == "goToEnd")
            .expect("goToEnd is offered");
        assert_eq!(go_to_end.detail, Some("std::goToEnd".to_string()));
        assert!(candidates.iter().all(|c| !c.deprecated), "{candidates:?}");
    }

    const QUALIFIED_NS_FIXTURE: &str =
        "namespace ns {\n    helper() { right; }\n}\nexport main() {\n    @ns::x();\n}\n";

    #[test]
    fn qualified_call_under_a_namespace_offers_its_members() {
        let mut service = PmcLanguageService::new();
        service.did_update(URI, QUALIFIED_NS_FIXTURE);

        let pos = pos_after(QUALIFIED_NS_FIXTURE, "ns::", 4);
        let candidates = service.completion(URI, pos);

        assert_eq!(labels(&candidates), BTreeSet::from(["helper".to_string()]));
    }

    // `old` is documented and `! [deprecated]`; `fresh` is documented but
    // not deprecated — one fixture proving both the `deprecated` tag AND
    // the cross-namespace `detail` independently, via two different
    // candidates from the SAME `member_candidates` call.
    const QUALIFIED_NS_DOC_FIXTURE: &str = "\
namespace ns {
?old thing.
! [deprecated] use other.
export old() { right; }
export fresh() { right; }
}
export main() {
    @ns::x();
}
";

    #[test]
    fn qualified_call_candidate_for_a_deprecated_function_is_tagged() {
        let mut service = PmcLanguageService::new();
        service.did_update(URI, QUALIFIED_NS_DOC_FIXTURE);

        let pos = pos_after(QUALIFIED_NS_DOC_FIXTURE, "ns::x()", 4);
        let candidates = service.completion(URI, pos);

        let old = candidates
            .iter()
            .find(|c| c.label == "old")
            .expect("old is offered");
        assert!(old.deprecated, "{old:?}");
    }

    #[test]
    fn qualified_call_candidate_carries_the_cross_namespace_qualified_detail() {
        let mut service = PmcLanguageService::new();
        service.did_update(URI, QUALIFIED_NS_DOC_FIXTURE);

        let pos = pos_after(QUALIFIED_NS_DOC_FIXTURE, "ns::x()", 4);
        let candidates = service.completion(URI, pos);

        let fresh = candidates
            .iter()
            .find(|c| c.label == "fresh")
            .expect("fresh is offered");
        assert_eq!(fresh.detail, Some("ns::fresh".to_string()));
        assert!(!fresh.deprecated, "{fresh:?}");
    }

    // --- Command position (context 4) ---

    const COMMAND_FIXTURE: &str = "export main() {\n    right;\n    1: right;\n    left, right, check(1, !);\n    goto 1;\n}\n";

    fn reserved_set() -> BTreeSet<String> {
        RESERVED.iter().map(|w| w.to_string()).collect()
    }

    #[test]
    fn command_position_at_statement_start_offers_all_eight_reserved_words() {
        let mut service = PmcLanguageService::new();
        service.did_update(URI, COMMAND_FIXTURE);

        let pos = pos_at(COMMAND_FIXTURE, "right;");
        let candidates = service.completion(URI, pos);

        assert_eq!(labels(&candidates), reserved_set());
        assert!(candidates.iter().all(|c| c.kind == CandidateKind::Keyword));
    }

    #[test]
    fn command_position_after_a_label_colon_offers_all_eight_reserved_words() {
        let mut service = PmcLanguageService::new();
        service.did_update(URI, COMMAND_FIXTURE);

        let pos = pos_after(COMMAND_FIXTURE, "1: right;", 3);
        let candidates = service.completion(URI, pos);

        assert_eq!(labels(&candidates), reserved_set());
    }

    #[test]
    fn command_position_after_a_comma_with_more_items_following_drops_goto_check_halt() {
        let mut service = PmcLanguageService::new();
        service.did_update(URI, COMMAND_FIXTURE);

        let pos = pos_after(COMMAND_FIXTURE, "left, right, check(1, !);", 6);
        let candidates = service.completion(URI, pos);

        assert_eq!(
            labels(&candidates),
            BTreeSet::from(["left", "right", "mark", "unmark", "debugger"].map(str::to_string))
        );
    }

    #[test]
    fn command_position_after_a_comma_in_the_final_slot_keeps_check_and_halt_but_not_goto() {
        let mut service = PmcLanguageService::new();
        service.did_update(URI, COMMAND_FIXTURE);

        let pos = pos_after(COMMAND_FIXTURE, "left, right, check(1, !);", 13);
        let candidates = service.completion(URI, pos);

        assert_eq!(
            labels(&candidates),
            BTreeSet::from(
                [
                    "left", "right", "mark", "unmark", "halt", "check", "debugger"
                ]
                .map(str::to_string)
            )
        );
    }

    #[test]
    fn command_position_after_checks_own_internal_comma_offers_nothing() {
        // `check(1, ▮!)` — the comma is `check`'s own arm separator,
        // inside its parens, not a command-group separator. No context
        // matches here (only a label number or `!` can parse), so the
        // result must be EMPTY — never the 7 RESERVED command words.
        let mut service = PmcLanguageService::new();
        service.did_update(URI, COMMAND_FIXTURE);

        let pos = pos_after(COMMAND_FIXTURE, "left, right, check(1, !);", 22);
        let candidates = service.completion(URI, pos);

        assert_eq!(candidates, Vec::new(), "{candidates:?}");
    }

    const COMMAND_UNTERMINATED_CHECK_FIXTURE: &str = "export main() {\n    check(1, ";

    #[test]
    fn command_position_after_checks_internal_comma_at_eof_offers_nothing() {
        // Same shape as above but unterminated mid-edit — `check(1, ▮`
        // at EOF, no closing `)`/`!`/`;` yet. `is_final_slot`'s forward
        // scan would run straight off the end of the token stream if it
        // were ever reached; the paren-depth gate must reject the comma
        // before that scan starts at all.
        let mut service = PmcLanguageService::new();
        service.did_update(URI, COMMAND_UNTERMINATED_CHECK_FIXTURE);

        let pos = pos_at_byte(
            COMMAND_UNTERMINATED_CHECK_FIXTURE,
            COMMAND_UNTERMINATED_CHECK_FIXTURE.len(),
        );
        let candidates = service.completion(URI, pos);

        assert_eq!(candidates, Vec::new(), "{candidates:?}");
    }

    // `mark(5)` taking a successor mid-group is a parser-level
    // GroupPosition error ("only the last command in a comma group may
    // take a successor") — the CST fails to build, but lexing doesn't
    // care, so `state.tokens` still populates (same staleness tier as
    // `analyze_staged_parse_failure_keeps_tokens_but_not_cst`). Exists
    // to exercise `comma_at_depth_zero`'s ACCEPT path through a
    // genuinely balanced paren pair (`RParen` then `LParen` netting
    // back to zero) — every other comma test here rejects via an
    // unmatched `LParen`, which would also pass a cruder "reject if any
    // LParen precedes the comma" implementation that this one must not.
    const COMMAND_BALANCED_PARENS_FIXTURE: &str = "export main() {\n    mark(5), left, right;\n}\n";

    #[test]
    fn command_position_after_a_comma_following_a_balanced_paren_pair_still_offers_the_group() {
        let mut service = PmcLanguageService::new();
        service.did_update(URI, COMMAND_BALANCED_PARENS_FIXTURE);

        let pos = pos_after(COMMAND_BALANCED_PARENS_FIXTURE, "mark(5), left, right;", 9);
        let candidates = service.completion(URI, pos);

        assert_eq!(
            labels(&candidates),
            BTreeSet::from(["left", "right", "mark", "unmark", "debugger"].map(str::to_string)),
            "{candidates:?}"
        );
    }

    #[test]
    fn command_position_after_goto_offers_only_the_enclosing_functions_labels() {
        let mut service = PmcLanguageService::new();
        service.did_update(URI, COMMAND_FIXTURE);

        let pos = pos_after(COMMAND_FIXTURE, "goto 1;", 5);
        let candidates = service.completion(URI, pos);

        assert_eq!(candidates.len(), 1, "{candidates:?}");
        assert_eq!(candidates[0].label, "1");
        assert_eq!(candidates[0].kind, CandidateKind::Value);
    }

    // --- Prefix replacement ---

    const HELP_FIXTURE: &str = "export main() {\n    @help();\n}\n";

    #[test]
    fn prefix_replacement_covers_the_whole_token_when_cursor_sits_mid_word() {
        let mut service = PmcLanguageService::new();
        service.did_update(URI, HELP_FIXTURE);

        let pos = pos_after(HELP_FIXTURE, "@help()", 3); // between "he" and "lp"
        let candidates = service.completion(URI, pos);

        assert!(!candidates.is_empty());
        let expected = span_of(HELP_FIXTURE, "help");
        for c in &candidates {
            assert_eq!(c.replace_span, expected);
        }
    }

    #[test]
    fn prefix_replacement_covers_the_whole_token_when_cursor_sits_at_its_end() {
        // `prefix_anchor`'s span check is `pos <= span.end`, one wider
        // than `span_contains`'s half-open `<` (this module's doc
        // comment, "The prefix/replace rule") — a cursor sitting exactly
        // at the end of a just-typed identifier must still anchor to
        // that identifier, not fall through to a zero-width span one
        // column later.
        let mut service = PmcLanguageService::new();
        service.did_update(URI, HELP_FIXTURE);

        let pos = pos_after(HELP_FIXTURE, "@help()", 5); // right after "help"'s "p"
        let candidates = service.completion(URI, pos);

        assert!(!candidates.is_empty());
        let expected = span_of(HELP_FIXTURE, "help");
        for c in &candidates {
            assert_eq!(c.replace_span, expected);
        }
    }

    const BLANK_LINE_FIXTURE: &str = "export main() {\n\n}\n";

    #[test]
    fn prefix_replacement_is_zero_width_away_from_any_token() {
        let mut service = PmcLanguageService::new();
        service.did_update(URI, BLANK_LINE_FIXTURE);

        let pos = Pos { line: 2, col: 1 };
        let candidates = service.completion(URI, pos);

        assert!(!candidates.is_empty(), "sanity: some context still matches");
        for c in &candidates {
            assert_eq!(
                c.replace_span,
                Span {
                    start: pos,
                    end: pos
                }
            );
        }
    }

    // --- Staleness ---

    const STALE_CLEAN: &str = "sib() { right; }\nexport main() {\n    @sib();\n}\n";
    // The lexer itself rejects a bare `@` with nothing identifier-like
    // after it (sigil adjacency, docs/pmt/language.md) — so a fixture that's
    // broken enough to fail PARSING (an unterminated call, missing `)`)
    // but still lexes clean is needed to exercise the tokens-survive/
    // analysis-doesn't split.
    const STALE_BROKEN: &str = "sib() { right; }\nexport main() {\n    @sib();\n    @x(\n";

    #[test]
    fn call_position_names_survive_a_parse_broken_edit_positions_stay_current() {
        let mut service = PmcLanguageService::new();
        let clean = service.did_update(URI, STALE_CLEAN);
        assert!(clean.is_empty(), "{clean:?}");

        let broken = service.did_update(URI, STALE_BROKEN);
        assert!(!broken.is_empty(), "sanity: the broken edit really fatals");

        let state = service.docs.get(URI).unwrap();
        assert!(state.tokens.is_some(), "lexing still succeeds");
        assert!(state.cst.is_none(), "parsing failed on the broken edit");
        assert!(state.analysis.is_none());
        assert!(
            state.scopes_for_completion.is_some(),
            "last-good scopes retained"
        );

        let pos = pos_after(STALE_BROKEN, "@x(", 1);
        let candidates = service.completion(URI, pos);
        assert!(
            candidates
                .iter()
                .any(|c| c.label == "sib" && c.kind == CandidateKind::Function),
            "names still offered from the stale scopes: {candidates:?}"
        );
    }

    // --- Task 6: cross-file completion through the overlay ---

    #[test]
    fn member_completion_unions_overlay_namespace_members() {
        let dir = unique_tmp_dir("member-union");
        fs::write(
            dir.join("pmt.json"),
            r#"{"project":{"targets":{"app":{"sources":["app.pmc","helper.pmc"]}}}}"#,
        )
        .unwrap();
        fs::write(
            dir.join("helper.pmc"),
            "namespace ns {\nexport inner() { right; }\n}\n",
        )
        .unwrap();

        let mut service = PmcLanguageService::new();
        let app_uri = file_uri(&dir.join("app.pmc"));
        const SRC: &str = "export main() {\n    @ns::x();\n}\n";
        service.did_update(&app_uri, SRC);

        let pos = pos_after(SRC, "ns::", 4);
        let candidates = service.completion(&app_uri, pos);

        let inner = candidates
            .iter()
            .find(|c| c.label == "inner")
            .expect("the sibling's ns::inner export is offered");
        assert_eq!(inner.kind, CandidateKind::Function);
        assert_eq!(inner.detail, Some("ns::inner".to_string()), "{inner:?}");
    }

    #[test]
    fn member_completion_local_definition_shadows_overlay_namespace_member() {
        // The overlay's own `ns::inner` is DEPRECATED; app.pmc defines its
        // OWN (undeprecated) `ns::inner` locally — proves the local
        // definition wins OUTRIGHT (exactly one `inner` candidate, not a
        // duplicate) and that the surviving candidate really is the local
        // one, not the overlay's: `seen`'s skip is exercised, not merely
        // present in the source.
        let dir = unique_tmp_dir("member-local-wins");
        fs::write(
            dir.join("pmt.json"),
            r#"{"project":{"targets":{"app":{"sources":["app.pmc","helper.pmc"]}}}}"#,
        )
        .unwrap();
        fs::write(
            dir.join("helper.pmc"),
            "namespace ns {\n?old.\n! [deprecated] use other.\nexport inner() { right; }\n}\n",
        )
        .unwrap();

        let mut service = PmcLanguageService::new();
        let app_uri = file_uri(&dir.join("app.pmc"));
        const SRC: &str =
            "namespace ns {\nexport inner() { right; }\n}\nexport main() {\n    @ns::x();\n}\n";
        service.did_update(&app_uri, SRC);

        let pos = pos_after(SRC, "ns::", 4);
        let candidates = service.completion(&app_uri, pos);

        let matches: Vec<_> = candidates.iter().filter(|c| c.label == "inner").collect();
        assert_eq!(
            matches.len(),
            1,
            "the local def wins outright, no overlay duplicate: {candidates:?}"
        );
        assert!(
            !matches[0].deprecated,
            "the surviving candidate is the LOCAL inner, not the overlay's deprecated one: {:?}",
            matches[0]
        );
    }

    #[test]
    fn use_path_completion_offers_sibling_roots_and_members() {
        let dir = unique_tmp_dir("use-roots-members");
        fs::write(
            dir.join("pmt.json"),
            r#"{"project":{"targets":{"app":{"sources":["app.pmc","helper.pmc"]}}}}"#,
        )
        .unwrap();
        fs::write(
            dir.join("helper.pmc"),
            "namespace ns {\nexport inner() { right; }\n}\n",
        )
        .unwrap();

        let mut service = PmcLanguageService::new();
        let app_uri = file_uri(&dir.join("app.pmc"));

        const ROOTS_SRC: &str = "use n;\nexport main() { right; }\n";
        service.did_update(&app_uri, ROOTS_SRC);
        let pos = pos_after(ROOTS_SRC, "use n", 4);
        let roots = service.completion(&app_uri, pos);
        assert!(
            roots
                .iter()
                .any(|c| c.label == "ns" && c.kind == CandidateKind::Module),
            "{roots:?}"
        );

        const MEMBERS_SRC: &str = "use ns::x;\nexport main() { right; }\n";
        service.did_update(&app_uri, MEMBERS_SRC);
        let pos = pos_after(MEMBERS_SRC, "use ns::", 8);
        let members = service.completion(&app_uri, pos);
        assert!(
            members
                .iter()
                .any(|c| c.label == "inner" && c.kind == CandidateKind::Function),
            "{members:?}"
        );
    }

    #[test]
    fn use_path_completion_offers_a_second_level_namespace_from_a_deeper_sibling_export() {
        // A genuinely 2-level-deep sibling namespace: `insert_export`
        // registers `outer::inner::f` under the `members` key
        // `["outer","inner"]` alone — there is no separate `["outer"]`
        // entry at all. `use_roots` already offers `outer` as a root (it
        // scans `path.first()` at any depth), but before this fix,
        // `member_candidates`'s overlay leg looked up `["outer"]` by
        // EXACT key only and found nothing, so typing `outer::` offered
        // no `inner` — this pins the fix (the child-namespace scan one
        // segment deeper, mirroring the local-scope leg just above it).
        let dir = unique_tmp_dir("use-roots-nested-ns");
        fs::write(
            dir.join("pmt.json"),
            r#"{"project":{"targets":{"app":{"sources":["app.pmc","helper.pmc"]}}}}"#,
        )
        .unwrap();
        fs::write(
            dir.join("helper.pmc"),
            "namespace outer {\nnamespace inner {\nexport f() { right; }\n}\n}\n",
        )
        .unwrap();

        let mut service = PmcLanguageService::new();
        let app_uri = file_uri(&dir.join("app.pmc"));

        const SRC: &str = "use outer::x;\nexport main() { right; }\n";
        service.did_update(&app_uri, SRC);
        let pos = pos_after(SRC, "use outer::", 11);
        let candidates = service.completion(&app_uri, pos);
        assert!(
            candidates
                .iter()
                .any(|c| c.label == "inner" && c.kind == CandidateKind::Module),
            "{candidates:?}"
        );
    }

    #[test]
    fn use_path_completion_drills_down_three_namespace_levels_deep() {
        // A genuinely THREE-level-deep sibling namespace: the full export
        // path `a::b::c::f` carries three namespace segments before its
        // own leaf. Before `overlay.rs::insert_export` registered every
        // intermediate namespace level, it registered only the leaf's own
        // immediate parent — `members[["a","b","c"]] = {"f": "a::b::c::f"}`
        // — and nothing at `["a"]` or `["a","b"]` at all. `use_roots`
        // still offered `a` as a root (it scans `path.first()` at any
        // depth), and typing `outer::` one level in worked by coincidence
        // whenever the leaf sat exactly two segments down (the sibling
        // test above), but here typing `a::` found no length-2 member key
        // and offered nothing, and `a::b::` likewise found no length-3
        // key — the drill-down died at the second hop. This test pins
        // BOTH hops on a fixture where the old code had nothing to find at
        // either one.
        let dir = unique_tmp_dir("use-roots-three-deep-ns");
        fs::write(
            dir.join("pmt.json"),
            r#"{"project":{"targets":{"app":{"sources":["app.pmc","helper.pmc"]}}}}"#,
        )
        .unwrap();
        fs::write(
            dir.join("helper.pmc"),
            "namespace a {\nnamespace b {\nnamespace c {\nexport f() { right; }\n}\n}\n}\n",
        )
        .unwrap();

        let mut service = PmcLanguageService::new();
        let app_uri = file_uri(&dir.join("app.pmc"));

        const FIRST_HOP_SRC: &str = "use a::x;\nexport main() { right; }\n";
        service.did_update(&app_uri, FIRST_HOP_SRC);
        let pos = pos_after(FIRST_HOP_SRC, "use a::", 7);
        let first_hop = service.completion(&app_uri, pos);
        assert!(
            first_hop
                .iter()
                .any(|c| c.label == "b" && c.kind == CandidateKind::Module),
            "`use a::` must offer `b`: {first_hop:?}"
        );

        const SECOND_HOP_SRC: &str = "use a::b::x;\nexport main() { right; }\n";
        service.did_update(&app_uri, SECOND_HOP_SRC);
        let pos = pos_after(SECOND_HOP_SRC, "use a::b::", 10);
        let second_hop = service.completion(&app_uri, pos);
        assert!(
            second_hop
                .iter()
                .any(|c| c.label == "c" && c.kind == CandidateKind::Module),
            "`use a::b::` must offer `c`: {second_hop:?}"
        );
    }

    #[test]
    fn bare_call_position_offers_top_level_sibling_exports_and_qualified_paths() {
        let dir = unique_tmp_dir("call-position-overlay");
        fs::write(
            dir.join("pmt.json"),
            r#"{"project":{"targets":{"app":{"sources":["app.pmc","helper.pmc"]}}}}"#,
        )
        .unwrap();
        fs::write(
            dir.join("helper.pmc"),
            "export helper() { right; }\nnamespace ns {\nexport inner() { right; }\n}\n",
        )
        .unwrap();

        let mut service = PmcLanguageService::new();
        let app_uri = file_uri(&dir.join("app.pmc"));
        const SRC: &str = "export main() {\n    @x();\n}\n";
        service.did_update(&app_uri, SRC);

        let pos = pos_after(SRC, "@x()", 1);
        let candidates = service.completion(&app_uri, pos);

        let bare = candidates
            .iter()
            .find(|c| c.label == "helper")
            .expect("the sibling's bare top-level export is offered");
        assert_eq!(bare.kind, CandidateKind::Function);
        assert_eq!(bare.detail, None, "{bare:?}");

        let qualified = candidates
            .iter()
            .find(|c| c.label == "ns::inner")
            .expect("the sibling's namespaced export is offered fully qualified");
        assert_eq!(qualified.kind, CandidateKind::Function);
        assert_eq!(
            qualified.detail, None,
            "label already is the qualified name: {qualified:?}"
        );
    }

    #[test]
    fn overlay_deprecated_docs_tag_candidates() {
        let dir = unique_tmp_dir("overlay-deprecated");
        fs::write(
            dir.join("pmt.json"),
            r#"{"project":{"targets":{"app":{"sources":["app.pmc","helper.pmc"]}}}}"#,
        )
        .unwrap();
        fs::write(
            dir.join("helper.pmc"),
            "?deprecated helper.\n! [deprecated] use other.\nexport old() { right; }\n",
        )
        .unwrap();

        let mut service = PmcLanguageService::new();
        let app_uri = file_uri(&dir.join("app.pmc"));
        const SRC: &str = "export main() {\n    @x();\n}\n";
        service.did_update(&app_uri, SRC);

        let pos = pos_after(SRC, "@x()", 1);
        let candidates = service.completion(&app_uri, pos);

        let old = candidates
            .iter()
            .find(|c| c.label == "old")
            .expect("the sibling's deprecated export is offered");
        assert!(old.deprecated, "{old:?}");
    }

    #[test]
    fn stdlib_false_removes_std_candidates_everywhere() {
        let dir = unique_tmp_dir("stdlib-false");
        fs::write(
            dir.join("pmt.json"),
            r#"{"project":{"stdlib":false,"targets":{"app":{"sources":["app.pmc","helper.pmc"]}}}}"#,
        )
        .unwrap();
        fs::write(dir.join("helper.pmc"), "export helper() { right; }\n").unwrap();

        let mut service = PmcLanguageService::new();
        let app_uri = file_uri(&dir.join("app.pmc"));

        // Root list: no "std" root; a genuinely local root ("ns") is
        // unaffected by the gate.
        const ROOTS_SRC: &str =
            "namespace ns {\n    helper() { right; }\n}\nuse x;\nexport main() { right; }\n";
        service.did_update(&app_uri, ROOTS_SRC);
        let pos = pos_after(ROOTS_SRC, "use x", 4);
        let roots = service.completion(&app_uri, pos);
        assert!(!roots.iter().any(|c| c.label == "std"), "{roots:?}");
        assert!(roots.iter().any(|c| c.label == "ns"), "{roots:?}");

        // Member list under `std::`: empty — never the 11-routine roster.
        const STD_MEMBER_SRC: &str = "use std::x;\nexport main() { right; }\n";
        service.did_update(&app_uri, STD_MEMBER_SRC);
        let pos = pos_after(STD_MEMBER_SRC, "std::", 5);
        let std_members = service.completion(&app_uri, pos);
        assert_eq!(std_members, Vec::new(), "{std_members:?}");

        // Bare call position: no `std::…` qualified candidates, but the
        // sibling's own top-level export is still offered — the gate
        // silences only std, not completion wholesale.
        const CALL_SRC: &str = "export main() {\n    @x();\n}\n";
        service.did_update(&app_uri, CALL_SRC);
        let pos = pos_after(CALL_SRC, "@x()", 1);
        let call = service.completion(&app_uri, pos);
        assert!(
            !call.iter().any(|c| c.label.starts_with("std::")),
            "{call:?}"
        );
        assert!(call.iter().any(|c| c.label == "helper"), "{call:?}");

        // A single-file doc (no manifest at all) keeps the unconditional
        // stdlib surface — the gate is manifest-driven, not global.
        let mut single = PmcLanguageService::new();
        single.did_update("untitled:Untitled-1", CALL_SRC);
        let pos = pos_after(CALL_SRC, "@x()", 1);
        let single_candidates = single.completion("untitled:Untitled-1", pos);
        assert!(
            single_candidates.iter().any(|c| c.label == "std::goToEnd"),
            "{single_candidates:?}"
        );
    }

    // --- `std::` names shadowed by a sibling's own `namespace std {}` ---

    #[test]
    fn std_member_list_prefers_a_shadowing_siblings_export() {
        // A sibling redefining `std::goToEnd` must win the member list
        // under `std::` — exactly once, carrying the SIBLING's own
        // deprecation tag (the embedded roster ships nothing deprecated,
        // so a deprecated `goToEnd` proves the overlay's own candidate is
        // what came back, not the roster's, and the single occurrence
        // proves it isn't offered twice).
        let dir = unique_tmp_dir("std-member-shadow");
        fs::write(
            dir.join("pmt.json"),
            r#"{"project":{"targets":{"app":{"sources":["app.pmc","shared.pmc"]}}}}"#,
        )
        .unwrap();
        fs::write(
            dir.join("shared.pmc"),
            "namespace std {\n! [deprecated] shadowed.\nexport goToEnd() { right; }\n}\n",
        )
        .unwrap();

        let mut service = PmcLanguageService::new();
        let app_uri = file_uri(&dir.join("app.pmc"));
        const SRC: &str = "use std::x;\nexport main() { right; }\n";
        service.did_update(&app_uri, SRC);

        let pos = pos_after(SRC, "std::", 5);
        let candidates = service.completion(&app_uri, pos);

        let matches: Vec<&Candidate> = candidates.iter().filter(|c| c.label == "goToEnd").collect();
        assert_eq!(
            matches.len(),
            1,
            "goToEnd must appear exactly once, not duplicated by the roster: {candidates:?}"
        );
        assert!(matches[0].deprecated, "the sibling's own copy: {matches:?}");

        // Every other roster routine is still offered unshadowed — this
        // sibling only redefines `goToEnd`.
        assert!(candidates.iter().any(|c| c.label == "goToBegin"));
    }

    #[test]
    fn qualified_call_std_prefers_a_shadowing_siblings_export() {
        // Same shadowing proof, in the OTHER `std::`-qualified surface —
        // `call_candidates`' (c)/(d) split — where the label is the full
        // `std::goToEnd` path rather than the bare member name.
        let dir = unique_tmp_dir("std-call-shadow");
        fs::write(
            dir.join("pmt.json"),
            r#"{"project":{"targets":{"app":{"sources":["app.pmc","shared.pmc"]}}}}"#,
        )
        .unwrap();
        fs::write(
            dir.join("shared.pmc"),
            "namespace std {\n! [deprecated] shadowed.\nexport goToEnd() { right; }\n}\n",
        )
        .unwrap();

        let mut service = PmcLanguageService::new();
        let app_uri = file_uri(&dir.join("app.pmc"));
        const SRC: &str = "export main() {\n    @x();\n}\n";
        service.did_update(&app_uri, SRC);

        let pos = pos_after(SRC, "@x()", 1);
        let candidates = service.completion(&app_uri, pos);

        let matches: Vec<&Candidate> = candidates
            .iter()
            .filter(|c| c.label == "std::goToEnd")
            .collect();
        assert_eq!(
            matches.len(),
            1,
            "std::goToEnd must appear exactly once, not duplicated by the roster: {candidates:?}"
        );
        assert!(matches[0].deprecated, "the sibling's own copy: {matches:?}");
    }

    #[test]
    fn stdlib_false_still_offers_a_shadowing_siblings_std_export() {
        // The stdlib toggle drops only the EMBEDDED roster; a sibling's
        // own `namespace std { export … }` is ordinary linked code and
        // stays offered regardless — the same distinction
        // `stdlib_false_removes_std_candidates_everywhere` draws for
        // navigation, here for completion's `["std"]` member block.
        let dir = unique_tmp_dir("std-member-shadow-stdlib-false");
        fs::write(
            dir.join("pmt.json"),
            r#"{"project":{"stdlib":false,"targets":{"app":{"sources":["app.pmc","shared.pmc"]}}}}"#,
        )
        .unwrap();
        fs::write(
            dir.join("shared.pmc"),
            "namespace std {\nexport goToEnd() { right; }\n}\n",
        )
        .unwrap();

        let mut service = PmcLanguageService::new();
        let app_uri = file_uri(&dir.join("app.pmc"));
        const SRC: &str = "use std::x;\nexport main() { right; }\n";
        service.did_update(&app_uri, SRC);

        let pos = pos_after(SRC, "std::", 5);
        let candidates = service.completion(&app_uri, pos);

        assert_eq!(
            labels(&candidates),
            BTreeSet::from(["goToEnd".to_string()]),
            "stdlib:false drops the roster but keeps the sibling's own export: {candidates:?}"
        );
    }
}
