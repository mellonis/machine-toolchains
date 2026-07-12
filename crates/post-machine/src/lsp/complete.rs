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
//! No match → empty (cross-file namespaces are invisible by design —
//! only this file's scopes and the embedded stdlib roster ever
//! contribute a candidate).

use std::collections::{BTreeSet, HashSet};

use mtc_core::diagnostics::{Pos, Span};
use mtc_core::lsp::{Candidate, CandidateKind};

use crate::compiler::ScopeSummary;
use crate::cst::{BodyKind, FunctionCst, TopItem, TopKind};
use crate::lexer::{Token, TokenKind};
use crate::parser::RESERVED;
use crate::stdlib::roster;

use super::DocState;

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
            use_roots(scopes, replace_span)
        } else {
            member_candidates(scopes, &segments, replace_span)
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
            member_candidates(scopes, &segments, replace_span)
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

fn mk_candidate(label: &str, kind: CandidateKind, replace_span: Span) -> Candidate {
    Candidate {
        label: label.to_string(),
        kind,
        replace_span,
        insert_text: label.to_string(),
        // `detail`/`deprecated` wiring is Task 4 (`Analysis.docs`-backed);
        // this task only adds the fields mechanically.
        detail: None,
        deprecated: false,
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

/// Half-open span containment, 1-based (mirrors `navigate.rs`'s own
/// helper of the same name — CST extent hit-testing, not the prefix
/// rule's touches-the-end variant above).
fn span_contains(span: Span, pos: Pos) -> bool {
    pos >= span.start && pos < span.end
}

/// The enclosing namespace path at `pos` — walks only `Namespace`
/// blocks (a function's own extent never changes it; only `namespace {
/// }` blocks add a `::` segment), recursively, innermost match wins.
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

/// The enclosing function CHAIN at `pos`, outermost first: the
/// top-level function containing `pos`, then its `BodyKind::Nested`
/// descendant containing `pos`, as deep as `pos` still lands inside one.
/// Mirrors the shape of `navigate.rs`'s `innermost_function` +
/// `deepest_nested`, but accumulates the whole chain instead of
/// returning only the innermost — context 3's assembly needs every
/// enclosing level, innermost-outward.
fn enclosing_function_chain(items: &[TopItem], pos: Pos) -> Vec<&FunctionCst> {
    for item in items {
        match &item.kind {
            TopKind::Namespace(ns) => {
                let chain = enclosing_function_chain(&ns.items, pos);
                if !chain.is_empty() {
                    return chain;
                }
            }
            TopKind::Function(f) => {
                if span_contains(f.span, pos) {
                    let mut chain = vec![f];
                    push_deepest_nested(f, pos, &mut chain);
                    return chain;
                }
            }
            TopKind::Comment(_) | TopKind::Import(_) => {}
        }
    }
    Vec::new()
}

fn push_deepest_nested<'a>(f: &'a FunctionCst, pos: Pos, chain: &mut Vec<&'a FunctionCst>) {
    for item in &f.body {
        if let BodyKind::Nested(nested) = &item.kind
            && span_contains(nested.span, pos)
        {
            chain.push(nested);
            push_deepest_nested(nested, pos, chain);
            return;
        }
    }
}

/// Context 1/2's shared member lookup for an exact namespace `path`:
/// `path == ["std"]` is special-cased to the embedded stdlib roster
/// (bare routine names, Function kind) since `std` is magic — it never
/// has a `ScopeSummary` entry of its own. Otherwise: `scopes.defs` under
/// the exact path (Function kind) plus child namespaces exactly one
/// segment deeper, derived the same way `use_roots` derives roots
/// (Module kind). Sorted by label for a deterministic result — the
/// underlying maps are hash-ordered.
fn member_candidates(scopes: &ScopeSummary, path: &[String], replace_span: Span) -> Vec<Candidate> {
    if path.len() == 1 && path[0] == "std" {
        let mut out: Vec<Candidate> = roster()
            .iter()
            .map(|entry| {
                let name = entry
                    .full_path
                    .strip_prefix("std::")
                    .unwrap_or(&entry.full_path);
                mk_candidate(name, CandidateKind::Function, replace_span)
            })
            .collect();
        out.sort_by(|a, b| a.label.cmp(&b.label));
        return out;
    }

    let mut out: Vec<Candidate> = Vec::new();
    if let Some(defs) = scopes.defs.get(path) {
        for name in defs.keys() {
            out.push(mk_candidate(name, CandidateKind::Function, replace_span));
        }
    }
    let mut children: BTreeSet<&str> = BTreeSet::new();
    for key in scopes.defs.keys().chain(scopes.bindings.keys()) {
        if key.len() == path.len() + 1 && key.starts_with(path) {
            children.insert(key[path.len()].as_str());
        }
    }
    for name in children {
        out.push(mk_candidate(name, CandidateKind::Module, replace_span));
    }
    out.sort_by(|a, b| a.label.cmp(&b.label));
    out
}

/// Context 1's no-`::` case: the file's namespace roots (the distinct
/// first segments of `scopes.defs`/`scopes.bindings` keys) plus `std` —
/// always offered even though `std` never appears in either map.
fn use_roots(scopes: &ScopeSummary, replace_span: Span) -> Vec<Candidate> {
    let mut names: BTreeSet<&str> = scopes
        .defs
        .keys()
        .chain(scopes.bindings.keys())
        .filter_map(|k| k.first().map(String::as_str))
        .collect();
    names.insert("std");
    names
        .into_iter()
        .map(|name| mk_candidate(name, CandidateKind::Module, replace_span))
        .collect()
}

/// Context 3: visible callables with shadowing, assembled in flatten's
/// own resolve order (`compiler.rs::flatten`'s `resolve` — nested scopes
/// innermost-outward, THEN each enclosing namespace prefix longest-first
/// with that level's defs before its bindings) — first-wins per bare
/// name via `seen`, so a definition always outranks a same-named import,
/// and an inner nested def always outranks an outer one. The std roster
/// rides in last, as fully qualified paths, in a disjoint label space
/// (bare names never contain `::`) so it never competes for `seen`.
fn call_candidates(state: &DocState, pos: Pos, replace_span: Span) -> Vec<Candidate> {
    let Some(scopes) = names_roster(state) else {
        return Vec::new();
    };
    let mut seen: HashSet<String> = HashSet::new();
    let mut out: Vec<Candidate> = Vec::new();

    // (a) nested defs of the enclosing function chain, innermost
    // outward, hoisted (a function's OWN direct BodyKind::Nested
    // children, regardless of their position relative to `pos`).
    // Unavailable without a CST — skipped, not substituted, per spec.
    if let Some(cst) = &state.cst {
        let chain = enclosing_function_chain(&cst.items, pos);
        for f in chain.iter().rev() {
            for item in &f.body {
                if let BodyKind::Nested(nested) = &item.kind
                    && seen.insert(nested.name.clone())
                {
                    out.push(mk_candidate(
                        &nested.name,
                        CandidateKind::Function,
                        replace_span,
                    ));
                }
            }
        }
    }

    // (b) per enclosing namespace prefix, longest first: that level's
    // defs, then its bindings. Falls back to the top-level scope ([])
    // when the CST is unavailable.
    let ns_path: Vec<String> = state
        .cst
        .as_ref()
        .map(|cst| enclosing_ns_path(&cst.items, pos))
        .unwrap_or_default();
    for k in (0..=ns_path.len()).rev() {
        let prefix = &ns_path[..k];
        if let Some(defs) = scopes.defs.get(prefix) {
            for name in defs.keys() {
                if seen.insert(name.clone()) {
                    out.push(mk_candidate(name, CandidateKind::Function, replace_span));
                }
            }
        }
        if let Some(bindings) = scopes.bindings.get(prefix) {
            for name in bindings.keys() {
                if seen.insert(name.clone()) {
                    out.push(mk_candidate(name, CandidateKind::Function, replace_span));
                }
            }
        }
    }

    // (c) the std roster, as qualified paths.
    for entry in roster() {
        out.push(mk_candidate(
            &entry.full_path,
            CandidateKind::Function,
            replace_span,
        ));
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
/// `label_span`), as Value candidates whose label is the decimal value.
/// No CST → no labels (not a hardcoded fallback list).
fn label_candidates(state: &DocState, pos: Pos, replace_span: Span) -> Vec<Candidate> {
    let Some(cst) = &state.cst else {
        return Vec::new();
    };
    let chain = enclosing_function_chain(&cst.items, pos);
    let Some(f) = chain.last() else {
        return Vec::new();
    };
    let mut seen: HashSet<u32> = HashSet::new();
    let mut out = Vec::new();
    for item in &f.body {
        if let BodyKind::Statement(stmt) = &item.kind {
            for label in &stmt.labels {
                if seen.insert(label.value) {
                    out.push(mk_candidate(
                        &label.value.to_string(),
                        CandidateKind::Value,
                        replace_span,
                    ));
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::super::PmcLanguageService;
    use super::*;
    use mtc_core::lsp::LanguageService;

    const URI: &str = "untitled:Complete-1";

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
    // after it (sigil adjacency, docs/language.md) — so a fixture that's
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
}
