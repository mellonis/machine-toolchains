//! Semantic tokens (docs/lsp.md (semantic tokens)): a deliberately
//! MINIMAL, resolution-aware legend — `namespace` / `function` / `number`
//! types, `declaration` / `defaultLibrary` modifiers — walked straight
//! off the CST, with call names cross-referenced against the
//! analysis-tier resolution table. Analysis-tier: `state.analysis` gates
//! the whole answer (a post-parse fatal anywhere in the document yields
//! `None`, never a resolution-free subset that would need its own
//! legend).
//!
//! A `use`-path or call-name's per-segment spans are never re-tokenized
//! by the lexer here — they're computed ARITHMETICALLY from the WRITTEN
//! text (`UsePath.span`/`Item::Call.name_span`'s start, plus each
//! segment's own character length, +2 per `::`). Sound only because
//! `.pmc` identifiers are single-line ASCII: no multi-byte segment-length
//! surprises, no line-spanning path.
//!
//! Every emitted span MUST be single-line — the core packer
//! `debug_assert`s it and silently misbehaves in release on violation
//! (docs/lsp.md). All spans here are identifier/number token spans by
//! construction, so this holds without special-casing.

use std::collections::BTreeMap;

use mtc_core::diagnostics::{Pos, Span};
use mtc_core::lsp::SemToken;

use crate::compiler::Resolution;
use crate::cst::{BodyKind, FunctionCst, StatementCst, TopItem, TopKind, UsePath};
use crate::parser::Item;

use super::walk::label_refs;
use super::{
    DocState, MODIFIER_DECLARATION, MODIFIER_DEFAULT_LIBRARY, TOKEN_TYPE_FUNCTION,
    TOKEN_TYPE_NAMESPACE, TOKEN_TYPE_NUMBER, overlay_owns,
};

/// The document's semantic token stream, or `None` on any failed
/// post-parse stage (docs/lsp.md (staged analysis)) — one tier, never a
/// resolution-free subset.
pub(super) fn semantic_tokens(state: &DocState) -> Option<Vec<SemToken>> {
    let analysis = state.analysis.as_ref()?;
    let cst = state.cst.as_ref()?;

    // `Span` has no `Hash` (only `Ord`) — a `BTreeMap` is the map-keyed
    // lookup this table needs without adding a derive to the shared
    // core type. Keyed by the call's own `name_span`: flatten mutates
    // only the AST's `Item::Call::name` string in place, never its
    // span, so the CST's untouched `name_span` matches exactly.
    let resolutions: BTreeMap<Span, &Resolution> = analysis
        .resolutions
        .iter()
        .map(|(span, resolution)| (*span, resolution))
        .collect();

    let mut out = Vec::new();
    walk_items(&cst.items, &resolutions, state, &mut out);
    out.sort_by_key(|token| token.span.start);
    debug_assert!(
        out.windows(2)
            .all(|pair| pair[0].span.end <= pair[1].span.start),
        "semantic tokens must not overlap: {out:?}"
    );
    Some(out)
}

/// One file/namespace-level item list, recursively (namespace blocks
/// nest their own `items`). `state` is threaded down for its `text`
/// (used by `label_def_span`, which derives the declaration span from
/// the written digits rather than the parser's span) and its `overlay`
/// (consulted by `emit_call_name` for an `Unresolved` call's name).
fn walk_items(
    items: &[TopItem],
    resolutions: &BTreeMap<Span, &Resolution>,
    state: &DocState,
    out: &mut Vec<SemToken>,
) {
    for item in items {
        match &item.kind {
            TopKind::Comment(_) => {}
            TopKind::Import(use_cst) => {
                for path in &use_cst.paths {
                    emit_use_path(path, state, out);
                }
            }
            TopKind::Namespace(ns) => {
                out.push(SemToken {
                    span: ns.name_span,
                    token_type: TOKEN_TYPE_NAMESPACE,
                    modifiers: MODIFIER_DECLARATION,
                });
                walk_items(&ns.items, resolutions, state, out);
            }
            TopKind::Function(f) => walk_function(f, resolutions, state, out),
        }
    }
}

/// One function definition — top-level or nested (`BodyKind::Nested`) —
/// plus its own body: statements and further nested definitions,
/// interleaved exactly as written.
fn walk_function(
    f: &FunctionCst,
    resolutions: &BTreeMap<Span, &Resolution>,
    state: &DocState,
    out: &mut Vec<SemToken>,
) {
    out.push(SemToken {
        span: f.name_span,
        token_type: TOKEN_TYPE_FUNCTION,
        modifiers: MODIFIER_DECLARATION,
    });
    for item in &f.body {
        match &item.kind {
            BodyKind::Comment(_) => {}
            BodyKind::Statement(stmt) => walk_statement(stmt, resolutions, state, out),
            BodyKind::Nested(nested) => walk_function(nested, resolutions, state, out),
        }
    }
}

/// One statement: its own labels (definitions), then each comma-group
/// item's label references and (for a resolved call) its name segments.
fn walk_statement(
    stmt: &StatementCst,
    resolutions: &BTreeMap<Span, &Resolution>,
    state: &DocState,
    out: &mut Vec<SemToken>,
) {
    for label in &stmt.labels {
        out.push(SemToken {
            span: label_def_span(label.span, &state.text),
            token_type: TOKEN_TYPE_NUMBER,
            modifiers: MODIFIER_DECLARATION,
        });
    }
    for comma in &stmt.items {
        walk_item(&comma.item, resolutions, state, out);
    }
}

/// Every reference span `item` carries — `walk::label_refs`' shared
/// enumeration (`Goto.label_span`, a `Check` arm's own span only when
/// that arm is `CheckArm::Label`, a builtin/call's `succ_label_span`) —
/// as bare, no-modifier `number` tokens, plus a call's resolved name
/// segments. An `Unresolved` call emits nothing for its name UNLESS the
/// document's cross-file overlay (docs/lsp.md (configuration)) resolves
/// the written name to a sibling export — that quiet-cue default is
/// `emit_call_name`'s own fallback, complementing `undeclared-external`.
fn walk_item(
    item: &Item,
    resolutions: &BTreeMap<Span, &Resolution>,
    state: &DocState,
    out: &mut Vec<SemToken>,
) {
    for (_, span) in label_refs(item).into_iter().flatten() {
        out.push(number_reference(span));
    }
    if let Item::Call {
        name, name_span, ..
    } = item
    {
        emit_call_name(name, *name_span, resolutions, state, out);
    }
}

fn number_reference(span: Span) -> SemToken {
    SemToken {
        span,
        token_type: TOKEN_TYPE_NUMBER,
        modifiers: 0,
    }
}

/// The label declaration's own token span: just the digits, never the
/// parser's `Label.span`. `Label.span` runs from the number's start to
/// the colon's END and (per `crate::parser::Label`) spans any interior
/// whitespace before that colon — `1 :` is legal `.pmc`, and there the
/// span covers `"1 "`, one column past the digit. Trimming a fixed one
/// column off the end (the old approach) only undoes the colon itself;
/// it leaves any interior space in when the colon is spaced, and
/// `Label.value` can't be used to reconstruct the digit count either
/// (leading zeros like `007` collapse to the `u32` value `7`). So the
/// declaration span is derived straight from the source text: start at
/// `span.start`, and walk forward while the text is an ASCII digit.
fn label_def_span(span: Span, text: &str) -> Span {
    let len = digit_run_len(text, span.start);
    Span::new(
        span.start.line,
        span.start.col,
        span.start.line,
        span.start.col + len,
    )
}

/// The number of consecutive ASCII-digit characters in `text` starting
/// at `pos` (1-based line/col, char-indexed to match the lexer's own
/// counting). Lines are split on `'\n'`; `pos` is always a label's
/// number start, so the line exists and the digit run is non-empty by
/// construction, but the walk is written generally regardless.
fn digit_run_len(text: &str, pos: Pos) -> u32 {
    let Some(line) = text.lines().nth((pos.line - 1) as usize) else {
        return 0;
    };
    line.chars()
        .skip((pos.col - 1) as usize)
        .take_while(|c| c.is_ascii_digit())
        .count() as u32
}

/// A `use` path's per-segment tokens: every segment but the last is
/// `namespace`; the last is `function`, plus `defaultLibrary` when the
/// path's own first segment is literally `std` AND the overlay does NOT
/// own the full joined path — [`overlay_owns`]'s own doc comment. A
/// sibling's own `use std::goToEnd;` importing ITS shadowing definition
/// (not the embedded one) therefore tokenizes as a plain `function`.
fn emit_use_path(path: &UsePath, state: &DocState, out: &mut Vec<SemToken>) {
    let full_path = path.path.join("::");
    let default_library =
        path.path.first().map(String::as_str) == Some("std") && !overlay_owns(state, &full_path);
    let segments: Vec<&str> = path.path.iter().map(String::as_str).collect();
    emit_path_segments(&segments, path.span.start, default_library, out);
}

/// A resolved call name's per-segment tokens. Looked up in the
/// resolution table by the CST's own `name_span` — identical to the
/// AST's (flatten mutates only the `name` string, never its span).
/// `defaultLibrary` applies to the final segment when the resolution is
/// `ImportBinding`/`QualifiedExternal` with a `std::`-prefixed full path
/// AND the overlay does not own that full path ([`overlay_owns`]) — a
/// sibling's own `namespace std { export … }` shadows the embedded
/// routine of the same name (the identical overlay-first order
/// `navigate.rs::std_path_target` resolves definitions in), so its call
/// sites read as plain `function`, exactly like any other sibling-
/// resolved call. An `Unresolved` call falls back to the document's
/// cross-file overlay (docs/lsp.md (configuration)) directly: a hit
/// tokenizes as a plain `function` (never `defaultLibrary` — a sibling
/// in the user's own project is not the standard library) regardless of
/// whether the symbol carries a navigable `target` (a `.pmo`-backed
/// sibling has none; tokenizing only asserts the name exists, navigation
/// is a separate concern). No hit keeps today's quiet cue: no token at
/// all.
fn emit_call_name(
    name: &str,
    name_span: Span,
    resolutions: &BTreeMap<Span, &Resolution>,
    state: &DocState,
    out: &mut Vec<SemToken>,
) {
    let Some(&resolution) = resolutions.get(&name_span) else {
        return;
    };
    if matches!(resolution, Resolution::Unresolved) {
        let Some(written) = super::navigate::text_at_span(&state.text, name_span) else {
            return;
        };
        if overlay_owns(state, written) {
            let segments: Vec<&str> = written.split("::").collect();
            emit_path_segments(&segments, name_span.start, false, out);
        }
        return;
    }
    let default_library = match resolution {
        Resolution::ImportBinding { full_path, .. }
        | Resolution::QualifiedExternal { full_path } => {
            full_path.starts_with("std::") && !overlay_owns(state, full_path)
        }
        Resolution::Local { .. } | Resolution::Unresolved => false,
    };
    let segments: Vec<&str> = name.split("::").collect();
    emit_path_segments(&segments, name_span.start, default_library, out);
}

/// The shared arithmetic segmenter (module doc): non-final segments
/// become `namespace`; the final segment becomes `function`, plus
/// `defaultLibrary` when `default_library_on_last` is set. Columns
/// advance by each written segment's own character count, +2 for the
/// `::` separator between segments.
fn emit_path_segments(
    segments: &[&str],
    start: Pos,
    default_library_on_last: bool,
    out: &mut Vec<SemToken>,
) {
    let mut col = start.col;
    let last = segments.len() - 1;
    for (i, segment) in segments.iter().enumerate() {
        let len = segment.chars().count() as u32;
        let span = Span::new(start.line, col, start.line, col + len);
        if i == last {
            let modifiers = if default_library_on_last {
                MODIFIER_DEFAULT_LIBRARY
            } else {
                0
            };
            out.push(SemToken {
                span,
                token_type: TOKEN_TYPE_FUNCTION,
                modifiers,
            });
        } else {
            out.push(SemToken {
                span,
                token_type: TOKEN_TYPE_NAMESPACE,
                modifiers: 0,
            });
        }
        col += len + 2;
    }
}

#[cfg(test)]
mod tests {
    use mtc_core::lsp::LanguageService;

    use super::super::{PmcLanguageService, overlay};
    use super::*;

    const URI: &str = "untitled:Tokens-1";

    fn tok(span: Span, token_type: u32, modifiers: u32) -> SemToken {
        SemToken {
            span,
            token_type,
            modifiers,
        }
    }

    /// A namespace holding an exported function (covers namespace +
    /// exported-fn declarations in one shot), a `use std::goToEnd as ge;`
    /// import, a plain top-level function (`local`, both declared AND
    /// called), and a `main` with a NESTED function (`helper`) — the
    /// nested one carries its own self-contained label/goto pair so
    /// nested-fn label scoping is exercised too. `main`'s own body then
    /// exercises a label def + a `check` reference + a labeled-successor
    /// (`right(2)`) reference, and the four call shapes: `@ge()`
    /// (import binding to a `std::` name, defaultLibrary on the lone
    /// segment), `@std::goToEnd()` (qualified external, `std` namespace
    /// segment + defaultLibrary function segment), `@local()` (plain
    /// local resolution, no modifiers), `@mystery()` (unresolved — must
    /// be ABSENT from the stream).
    ///
    /// Every span below was computed arithmetically from this exact text
    /// (1-based columns, half-open ends) and cross-checked with a
    /// throwaway script before being hand-transcribed into the expected
    /// vector — see the task's TDD evidence.
    const RICH_FIXTURE: &str = "namespace ns {\n    export inner() {\n        right;\n    }\n}\nuse std::goToEnd as ge;\nlocal() {\n    right;\n}\nmain() {\n    helper() {\n        1: right;\n        goto 1;\n    }\n    2: left;\n    check(2, !);\n    right(2);\n    @ge();\n    @std::goToEnd();\n    @local();\n    @mystery();\n}\n";

    /// `goto 99` — a well-formed statement that references a label the
    /// function never declares; `ir::lower` fatals well past the CST
    /// stage, so `analysis` (and therefore `semantic_tokens`) is `None`.
    const UNDEFINED_LABEL_FIXTURE: &str = "main() {\nright;\ngoto 99;\n}\n";

    #[test]
    fn rich_fixture_yields_the_exact_absolute_token_stream() {
        let mut service = PmcLanguageService::new();
        let diags = service.did_update(URI, RICH_FIXTURE);
        assert!(
            diags
                .iter()
                .all(|d| d.severity != mtc_core::lsp::ServiceSeverity::Error),
            "sanity: the fixture must parse and analyze cleanly, {diags:?}"
        );

        let tokens = service
            .semantic_tokens(URI)
            .expect("analysis-tier answer on a clean parse");

        assert_eq!(
            tokens,
            vec![
                // `namespace ns {` — the namespace declaration.
                tok(
                    Span::new(1, 11, 1, 13),
                    TOKEN_TYPE_NAMESPACE,
                    MODIFIER_DECLARATION
                ),
                // `export inner() {` — an exported function declaration.
                tok(
                    Span::new(2, 12, 2, 17),
                    TOKEN_TYPE_FUNCTION,
                    MODIFIER_DECLARATION
                ),
                // `use std::goToEnd as ge;` — the `std` segment (namespace,
                // bare) and the `goToEnd` segment (function, defaultLibrary
                // because path[0] == "std"); the `as ge` alias contributes
                // no token of its own.
                tok(Span::new(6, 5, 6, 8), TOKEN_TYPE_NAMESPACE, 0),
                tok(
                    Span::new(6, 10, 6, 17),
                    TOKEN_TYPE_FUNCTION,
                    MODIFIER_DEFAULT_LIBRARY
                ),
                // `local() {` — a plain top-level function declaration.
                tok(
                    Span::new(7, 1, 7, 6),
                    TOKEN_TYPE_FUNCTION,
                    MODIFIER_DECLARATION
                ),
                // `main() {` — the entry function declaration.
                tok(
                    Span::new(10, 1, 10, 5),
                    TOKEN_TYPE_FUNCTION,
                    MODIFIER_DECLARATION
                ),
                // `helper() {` — a NESTED function declaration.
                tok(
                    Span::new(11, 5, 11, 11),
                    TOKEN_TYPE_FUNCTION,
                    MODIFIER_DECLARATION
                ),
                // `1: right;` inside helper — the label definition, colon
                // excluded (just the "1").
                tok(
                    Span::new(12, 9, 12, 10),
                    TOKEN_TYPE_NUMBER,
                    MODIFIER_DECLARATION
                ),
                // `goto 1;` inside helper — the label reference.
                tok(Span::new(13, 14, 13, 15), TOKEN_TYPE_NUMBER, 0),
                // `2: left;` inside main — the label definition.
                tok(
                    Span::new(15, 5, 15, 6),
                    TOKEN_TYPE_NUMBER,
                    MODIFIER_DECLARATION
                ),
                // `check(2, !);` inside main — only the `marked` arm (a
                // Label) references; the `blank` arm (`!`, Return) emits
                // nothing.
                tok(Span::new(16, 11, 16, 12), TOKEN_TYPE_NUMBER, 0),
                // `right(2);` inside main — a labeled-successor reference.
                tok(Span::new(17, 11, 17, 12), TOKEN_TYPE_NUMBER, 0),
                // `@ge();` — import-bound to `std::goToEnd`: one segment,
                // function + defaultLibrary.
                tok(
                    Span::new(18, 6, 18, 8),
                    TOKEN_TYPE_FUNCTION,
                    MODIFIER_DEFAULT_LIBRARY
                ),
                // `@std::goToEnd();` — qualified external to `std::goToEnd`:
                // `std` namespace segment (bare), `goToEnd` function segment
                // (defaultLibrary).
                tok(Span::new(19, 6, 19, 9), TOKEN_TYPE_NAMESPACE, 0),
                tok(
                    Span::new(19, 11, 19, 18),
                    TOKEN_TYPE_FUNCTION,
                    MODIFIER_DEFAULT_LIBRARY
                ),
                // `@local();` — resolves to the local `local` function:
                // plain function, no modifiers.
                tok(Span::new(20, 6, 20, 11), TOKEN_TYPE_FUNCTION, 0),
                // `@mystery();` is UNRESOLVED — no token at all; note its
                // absence from this vector.
            ]
        );
    }

    /// `Resolution::ImportBinding` with a NON-`std` full path — the rich
    /// fixture above only exercises the std-aliased import (`@ge()`,
    /// `defaultLibrary` set) and a `Resolution::Local` call (`@local()`,
    /// no modifiers); this pins the third combination `emit_call_name`'s
    /// `default_library` guard distinguishes: a user's own `use` import,
    /// which must render as a plain function token with NO
    /// `defaultLibrary` modifier, same as a local resolution but through
    /// the other match arm.
    #[test]
    fn non_std_import_binding_call_carries_no_default_library_modifier() {
        const FIXTURE: &str = "use ext;\nmain() {\n    @ext();\n}\n";
        let mut service = PmcLanguageService::new();
        let diags = service.did_update(URI, FIXTURE);
        assert!(diags.is_empty(), "sanity: clean fixture, {diags:?}");

        let tokens = service
            .semantic_tokens(URI)
            .expect("analysis-tier answer on a clean parse");

        assert_eq!(
            tokens,
            vec![
                // `use ext;` — the single path segment, function (the
                // last/only segment), no defaultLibrary (path[0] != "std").
                tok(Span::new(1, 5, 1, 8), TOKEN_TYPE_FUNCTION, 0),
                // `main() {` — the entry function declaration.
                tok(
                    Span::new(2, 1, 2, 5),
                    TOKEN_TYPE_FUNCTION,
                    MODIFIER_DECLARATION
                ),
                // `@ext();` — import-bound to a NON-std name: plain
                // function, no modifiers (the branch `@ge()` above
                // doesn't reach, since `ge`'s full path IS std-prefixed).
                tok(Span::new(3, 6, 3, 9), TOKEN_TYPE_FUNCTION, 0),
            ]
        );
    }

    #[test]
    fn label_definition_token_excludes_the_trailing_colon() {
        // `5: right;` — the label span covers "5:" (cols 1..3); the
        // definition token must stop one column short, covering only the
        // "5" (cols 1..2).
        let fixture = "main() {\n5: right;\n}\n";
        let mut service = PmcLanguageService::new();
        service.did_update(URI, fixture);

        let tokens = service.semantic_tokens(URI).expect("clean parse");
        let label_tokens: Vec<&SemToken> = tokens
            .iter()
            .filter(|t| t.token_type == TOKEN_TYPE_NUMBER && t.modifiers == MODIFIER_DECLARATION)
            .collect();
        assert_eq!(label_tokens.len(), 1, "{tokens:?}");
        assert_eq!(label_tokens[0].span, Span::new(2, 1, 2, 2));
    }

    #[test]
    fn label_definition_token_covers_only_the_digit_before_a_spaced_colon() {
        // `1 : right;` — `Label.span` covers "1 :" (number start through
        // the colon's end, INCLUDING the interior space: cols 1..4). The
        // declaration token must cover only the digit run "1" (cols
        // 1..2), never the space before the colon.
        let fixture = "main() {\n1 : right;\n}\n";
        let mut service = PmcLanguageService::new();
        service.did_update(URI, fixture);

        let tokens = service.semantic_tokens(URI).expect("clean parse");
        let label_tokens: Vec<&SemToken> = tokens
            .iter()
            .filter(|t| t.token_type == TOKEN_TYPE_NUMBER && t.modifiers == MODIFIER_DECLARATION)
            .collect();
        assert_eq!(label_tokens.len(), 1, "{tokens:?}");
        assert_eq!(label_tokens[0].span, Span::new(2, 1, 2, 2));
    }

    #[test]
    fn label_definition_token_covers_the_full_leading_zero_digit_run() {
        // Two cases in one fixture, covering multi-digit runs on both
        // sides of the spaced/unspaced split: `10 :` (spaced, no leading
        // zero — cols 1..3 are the digits, the token must stop at col 3
        // and NOT include the space or colon) and `007:` (unspaced,
        // leading zeros — `Label.value` collapses `007` to `7`, so the
        // span must come from the source text's digit run, cols 1..4,
        // not from reconstructing digits out of the value).
        let fixture = "main() {\n10 : right;\n007: left;\n}\n";
        let mut service = PmcLanguageService::new();
        service.did_update(URI, fixture);

        let tokens = service.semantic_tokens(URI).expect("clean parse");
        let label_tokens: Vec<&SemToken> = tokens
            .iter()
            .filter(|t| t.token_type == TOKEN_TYPE_NUMBER && t.modifiers == MODIFIER_DECLARATION)
            .collect();
        assert_eq!(label_tokens.len(), 2, "{tokens:?}");
        assert_eq!(label_tokens[0].span, Span::new(2, 1, 2, 3), "`10 :`");
        assert_eq!(label_tokens[1].span, Span::new(3, 1, 3, 4), "`007:`");
    }

    #[test]
    fn null_while_a_post_parse_stage_fails() {
        let mut service = PmcLanguageService::new();
        let diags = service.did_update(URI, UNDEFINED_LABEL_FIXTURE);
        assert!(
            diags.iter().any(|d| d.code == Some("undefined-label")),
            "sanity: goto 99 really does fatal, {diags:?}"
        );

        assert_eq!(service.semantic_tokens(URI), None);
    }

    /// The overlay fallback (module doc, `emit_call_name`'s doc): a bare
    /// call unresolved in-file but present in the document's cross-file
    /// overlay tokenizes as a plain `function`, no `defaultLibrary` (a
    /// sibling export is not the standard library) — and the OverlaySym
    /// is built with `target: None`, the exact shape a `.pmo`-backed
    /// symbol carries (`overlay.rs::insert_export`), proving tokenizing
    /// asserts existence only, never navigability. The negative half
    /// re-runs the identical source with NO overlay at all, confirming
    /// the site still emits nothing — a positive-only test would also
    /// pass if tokens were emitted unconditionally, which would destroy
    /// the quiet no-token cue this whole arm exists to preserve.
    #[test]
    fn overlay_resolved_bare_call_tokenizes_as_function_without_default_library() {
        const FIXTURE: &str = "main() {\n    @sibling();\n}\n";
        // "    @sibling();" — cols 1..4 are the indent, 5 is '@', 6..13 is
        // the 7-char name.
        let call_name_span = Span::new(2, 6, 2, 13);

        let mut service = PmcLanguageService::new();
        let diags = service.did_update(URI, FIXTURE);
        assert!(
            diags.iter().any(|d| d.code == Some("undeclared-external")),
            "sanity: unresolved with no overlay wired up yet, {diags:?}"
        );

        service.docs.get_mut(URI).unwrap().overlay = Some(overlay::Overlay {
            stdlib: false,
            symbols: std::collections::HashMap::from([(
                "sibling".to_string(),
                overlay::OverlaySym {
                    target: None,
                    doc: None,
                },
            )]),
            members: std::collections::HashMap::new(),
        });

        let tokens = service
            .semantic_tokens(URI)
            .expect("analysis-tier answer on a clean parse");
        let hit = tokens
            .iter()
            .find(|t| t.span == call_name_span)
            .unwrap_or_else(|| panic!("no token at the call name span: {tokens:?}"));
        assert_eq!(hit.token_type, TOKEN_TYPE_FUNCTION);
        assert_eq!(
            hit.modifiers & MODIFIER_DEFAULT_LIBRARY,
            0,
            "a sibling export must never read as defaultLibrary: {hit:?}"
        );

        let mut plain_service = PmcLanguageService::new();
        plain_service.did_update(URI, FIXTURE);
        let plain_tokens = plain_service
            .semantic_tokens(URI)
            .expect("analysis-tier answer on a clean parse");
        assert!(
            !plain_tokens.iter().any(|t| t.span == call_name_span),
            "no overlay: the quiet no-token cue must hold, {plain_tokens:?}"
        );
    }

    /// The `defaultLibrary` modifier must track WHERE `std::goToEnd`
    /// actually resolves, not the bare `std::` prefix
    /// (`overlay_owns`'s own doc comment): a sibling's own `namespace
    /// std { export goToEnd() {...} }` shadows the embedded routine —
    /// same overlay-first order as go-to-definition and hover — so its
    /// call site must read as a plain `function`, no modifier. Both
    /// halves are pinned on the SAME fixture/span so a fix that simply
    /// stopped emitting the modifier unconditionally (rather than
    /// gating it on overlay ownership) would fail the unshadowed half.
    #[test]
    fn std_call_default_library_modifier_follows_overlay_ownership() {
        const FIXTURE: &str = "main() {\n    @std::goToEnd();\n}\n";
        // "    @std::goToEnd();" — cols 1..4 indent, 5 '@', 6..9 "std",
        // 9..11 "::", 11..18 the 7-char "goToEnd" segment.
        let function_span = Span::new(2, 11, 2, 18);

        let mut service = PmcLanguageService::new();
        let diags = service.did_update(URI, FIXTURE);
        assert!(
            diags
                .iter()
                .all(|d| d.severity != mtc_core::lsp::ServiceSeverity::Error),
            "sanity: the fixture must parse and analyze cleanly, {diags:?}"
        );

        // Unshadowed half: no overlay defines `std::goToEnd` (no overlay
        // wired up at all here) — this genuinely resolves to the
        // embedded stdlib, so `defaultLibrary` must still be set. A
        // regression guard: this is the SAME assertion `RICH_FIXTURE`'s
        // own `@std::goToEnd()` case already pins, restated here on a
        // minimal fixture so it sits right next to the shadowed half
        // below.
        let plain_tokens = service
            .semantic_tokens(URI)
            .expect("analysis-tier answer on a clean parse");
        let plain_hit = plain_tokens
            .iter()
            .find(|t| t.span == function_span)
            .unwrap_or_else(|| panic!("no token at the function span: {plain_tokens:?}"));
        assert_eq!(plain_hit.token_type, TOKEN_TYPE_FUNCTION);
        assert_eq!(
            plain_hit.modifiers & MODIFIER_DEFAULT_LIBRARY,
            MODIFIER_DEFAULT_LIBRARY,
            "unshadowed std::goToEnd must still read as defaultLibrary: {plain_hit:?}"
        );

        // Shadowed half: a sibling's own `namespace std { export
        // goToEnd() {...} }`, wired directly into the overlay (mirroring
        // `overlay_resolved_bare_call_tokenizes_as_function_without_default_library`
        // above) rather than standing up a whole on-disk project tree —
        // only `symbols`' ownership matters to `overlay_owns`.
        service.docs.get_mut(URI).unwrap().overlay = Some(overlay::Overlay {
            stdlib: true,
            symbols: std::collections::HashMap::from([(
                "std::goToEnd".to_string(),
                overlay::OverlaySym {
                    target: None,
                    doc: None,
                },
            )]),
            members: std::collections::HashMap::new(),
        });

        let shadowed_tokens = service
            .semantic_tokens(URI)
            .expect("analysis-tier answer on a clean parse");
        let shadowed_hit = shadowed_tokens
            .iter()
            .find(|t| t.span == function_span)
            .unwrap_or_else(|| panic!("no token at the function span: {shadowed_tokens:?}"));
        assert_eq!(shadowed_hit.token_type, TOKEN_TYPE_FUNCTION);
        assert_eq!(
            shadowed_hit.modifiers & MODIFIER_DEFAULT_LIBRARY,
            0,
            "a sibling's own std::goToEnd must never read as defaultLibrary: {shadowed_hit:?}"
        );
    }

    #[test]
    fn drift_guard_every_emitted_token_fits_the_legend() {
        let mut service = PmcLanguageService::new();
        service.did_update(URI, RICH_FIXTURE);
        let tokens = service
            .semantic_tokens(URI)
            .expect("clean parse over the maximal fixture");
        assert!(!tokens.is_empty(), "sanity: the fixture emits tokens");

        let (types, modifiers) = service.token_legend();
        // The maximal fixture must actually exercise every legend type
        // and every modifier bit at least once — else the drift guard
        // below would pass vacuously.
        for expected_type in 0..types.len() as u32 {
            assert!(
                tokens.iter().any(|t| t.token_type == expected_type),
                "legend type index {expected_type} ({}) never emitted",
                types[expected_type as usize]
            );
        }
        for (bit_ix, name) in modifiers.iter().enumerate() {
            let bit = 1u32 << bit_ix;
            assert!(
                tokens.iter().any(|t| t.modifiers & bit != 0),
                "legend modifier bit {bit} ({name}) never emitted"
            );
        }

        for t in &tokens {
            assert!(
                (t.token_type as usize) < types.len(),
                "token_type {} has no legend entry in {types:?}",
                t.token_type
            );
            let mut bits = t.modifiers;
            while bits != 0 {
                let bit_ix = bits.trailing_zeros() as usize;
                assert!(
                    bit_ix < modifiers.len(),
                    "modifier bit {} has no legend entry in {modifiers:?}",
                    1u32 << bit_ix
                );
                bits &= bits - 1; // clear the lowest set bit
            }
        }
    }
}
