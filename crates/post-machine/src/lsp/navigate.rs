//! Go-to-definition (docs/lsp.md (navigation)): resolves a document
//! position to a [`DefTarget`] through a four-step resolution order —
//! the resolution table (a call's name), a label reference (`goto` /
//! `check` / a labeled successor), a `use std::…` path, else `None`.
//! Analysis-tier: every query degrades to `None` when `DocState::analysis`
//! is `None` (a post-parse fatal anywhere in the document), not just the
//! part that failed.

use mtc_core::diagnostics::{Pos, Span};
use mtc_core::lsp::DefTarget;

use crate::compiler::Resolution;
use crate::cst::{BodyKind, FunctionCst, TopItem, TopKind};
use crate::parser::{CheckArm, Item, Successor};
use crate::stdlib::{materialized_std_uri, roster};

use super::DocState;

/// Half-open span containment, 1-based. `Pos`'s derived `Ord` compares
/// `line` then `col` (its field order) — exactly a lexicographic
/// position comparison — so this is correct for a multi-line span with
/// no special-casing.
fn span_contains(span: Span, pos: Pos) -> bool {
    pos >= span.start && pos < span.end
}

/// The definition target for `pos` in `uri`'s current document
/// (docs/lsp.md (navigation)):
///
/// 1. a resolution-table entry whose span contains `pos` (the call name
///    under the cursor) — resolved per its [`Resolution`] variant,
///    `std::` paths routed through the materialized roster;
/// 2. failing that, a label reference (`goto` target, a `check` arm, or
///    a labeled successor) hit-tested against the innermost enclosing
///    function's own labels;
/// 3. failing that, a `use std::…` path segment, through the
///    materialized roster;
/// 4. otherwise `None`.
pub(super) fn definition(state: &DocState, uri: &str, pos: Pos) -> Option<DefTarget> {
    let analysis = state.analysis.as_ref()?;

    if let Some((_, resolution)) = analysis
        .resolutions
        .iter()
        .find(|(span, _)| span_contains(*span, pos))
    {
        return resolve_call(uri, resolution);
    }

    let cst = state.cst.as_ref()?;

    if let Some(function) = innermost_function(&cst.items, pos)
        && let Some(value) = label_reference_at(function, pos)
    {
        return label_span(function, value).map(|span| DefTarget {
            uri: uri.to_string(),
            span,
        });
    }

    if let Some(full_path) = std_use_path_at(&cst.items, pos) {
        return std_target(&full_path);
    }

    None
}

/// Step 1's per-variant resolution.
fn resolve_call(uri: &str, resolution: &Resolution) -> Option<DefTarget> {
    match resolution {
        Resolution::Local { def_name_span } => Some(DefTarget {
            uri: uri.to_string(),
            span: *def_name_span,
        }),
        Resolution::ImportBinding {
            use_span,
            full_path,
        } => {
            if full_path.starts_with("std::") {
                std_target(full_path)
            } else {
                Some(DefTarget {
                    uri: uri.to_string(),
                    span: *use_span,
                })
            }
        }
        Resolution::QualifiedExternal { full_path } => {
            if full_path.starts_with("std::") {
                std_target(full_path)
            } else {
                None
            }
        }
        Resolution::Unresolved => None,
    }
}

/// A `std::…` full path through the materialized roster: a roster miss
/// or a materializer IO failure both degrade to `None` (docs/lsp.md
/// (materialized stdlib)).
fn std_target(full_path: &str) -> Option<DefTarget> {
    let uri = materialized_std_uri()?;
    let entry = roster().iter().find(|e| e.full_path == full_path)?;
    Some(DefTarget {
        uri: uri.to_string(),
        span: entry.name_span,
    })
}

/// The innermost `FunctionCst` whose extent span contains `pos` — walks
/// namespace blocks, then descends into `BodyKind::Nested` functions as
/// deep as `pos` still lands inside. Labels are function-scoped, so only
/// the deepest enclosing function's own labels are ever relevant.
fn innermost_function(items: &[TopItem], pos: Pos) -> Option<&FunctionCst> {
    for item in items {
        match &item.kind {
            TopKind::Namespace(ns) => {
                if let Some(f) = innermost_function(&ns.items, pos) {
                    return Some(f);
                }
            }
            TopKind::Function(f) => {
                if span_contains(f.span, pos) {
                    return Some(deepest_nested(f, pos));
                }
            }
            TopKind::Comment(_) | TopKind::Import(_) => {}
        }
    }
    None
}

/// Descends into `f`'s own `BodyKind::Nested` children as long as `pos`
/// stays inside one of them; returns the deepest match (`f` itself if
/// none of its nested children contain `pos`).
fn deepest_nested(f: &FunctionCst, pos: Pos) -> &FunctionCst {
    for item in &f.body {
        if let BodyKind::Nested(nested) = &item.kind
            && span_contains(nested.span, pos)
        {
            return deepest_nested(nested, pos);
        }
    }
    f
}

/// The label value referenced at `pos`, if `pos` sits on one of
/// `function`'s own Task 2 reference spans: `Item::Goto.label_span`,
/// `Item::Check`'s `marked_span`/`blank_span` when that arm is a
/// `CheckArm::Label`, or a `Successor::Label`'s `succ_label_span` on a
/// builtin or a call. Only `function`'s OWN statements are examined —
/// its nested children are a separate label scope, reached only by
/// `innermost_function` descending into them for a `pos` that lands
/// there.
fn label_reference_at(function: &FunctionCst, pos: Pos) -> Option<u32> {
    for item in &function.body {
        let BodyKind::Statement(stmt) = &item.kind else {
            continue;
        };
        for comma in &stmt.items {
            match &comma.item {
                Item::Goto {
                    label, label_span, ..
                } => {
                    if span_contains(*label_span, pos) {
                        return Some(*label);
                    }
                }
                Item::Check {
                    marked,
                    blank,
                    marked_span,
                    blank_span,
                    ..
                } => {
                    if let CheckArm::Label(value) = marked
                        && span_contains(*marked_span, pos)
                    {
                        return Some(*value);
                    }
                    if let CheckArm::Label(value) = blank
                        && span_contains(*blank_span, pos)
                    {
                        return Some(*value);
                    }
                }
                Item::Builtin {
                    succ,
                    succ_label_span: Some(span),
                    ..
                }
                | Item::Call {
                    succ,
                    succ_label_span: Some(span),
                    ..
                } => {
                    if let Successor::Label(value) = succ
                        && span_contains(*span, pos)
                    {
                        return Some(*value);
                    }
                }
                _ => {}
            }
        }
    }
    None
}

/// `value`'s label declaration span within `function`'s OWN statements
/// (labels are function-scoped — never searched in nested children or
/// enclosing scopes).
fn label_span(function: &FunctionCst, value: u32) -> Option<Span> {
    for item in &function.body {
        let BodyKind::Statement(stmt) = &item.kind else {
            continue;
        };
        for label in &stmt.labels {
            if label.value == value {
                return Some(label.span);
            }
        }
    }
    None
}

/// Step 3: `pos` inside a `use std::…` path's span → its joined full
/// path (`"std::goToEnd"`). Searched recursively through namespace
/// blocks — imports are legal at any nesting level.
fn std_use_path_at(items: &[TopItem], pos: Pos) -> Option<String> {
    for item in items {
        match &item.kind {
            TopKind::Namespace(ns) => {
                if let Some(path) = std_use_path_at(&ns.items, pos) {
                    return Some(path);
                }
            }
            TopKind::Import(use_cst) => {
                for path in &use_cst.paths {
                    if span_contains(path.span, pos)
                        && path.path.first().map(String::as_str) == Some("std")
                    {
                        return Some(path.path.join("::"));
                    }
                }
            }
            TopKind::Comment(_) | TopKind::Function(_) => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use mtc_core::diagnostics::Pos;
    use mtc_core::lsp::LanguageService;

    use super::super::PmcLanguageService;
    use super::*;
    use crate::lsp::uri_to_path;

    const URI: &str = "untitled:Nav-1";

    /// Task-3-shaped fixture, extended for navigation coverage:
    /// - `sib()` / `@sib()` — a plain top-level local call (as opposed
    ///   to `helper`, which is nested).
    /// - `succ()` — a self-contained `2: left; right(2);`, covering the
    ///   labeled-successor arm of step 2 (`succ_label_span` on a
    ///   builtin/call), which `goto`/`check` alone don't exercise.
    /// - `helper()` nested in `main`, with its OWN `1: left; goto 1;` —
    ///   proves label scoping is per-function: `main` also declares a
    ///   `1:` label and references it with `goto 1;` / `check(1, !);`,
    ///   and the two must never cross.
    /// - the rest (`ns::inner`, `ext`, `ge`/`std::goToEnd`,
    ///   `other::thing`, `mystery`) mirrors Task 3's resolution-table
    ///   fixture verbatim.
    const NAV_FIXTURE: &str = "use ext;\nuse std::goToEnd as ge;\nnamespace ns { export inner() { right; } }\nsib() { left; }\nsucc() { 2: left; right(2); }\nexport main() {\n    helper() { 1: left; goto 1; }\n    @sib();\n    @succ();\n    @helper();\n    @ns::inner();\n    @inner();\n    @ext();\n    @ge();\n    @other::thing();\n    @mystery();\n    1: right;\n    check(1, !);\n    goto 1;\n}\n";

    /// Same fixture with a trailing `goto 99;` appended inside `main` —
    /// an undefined label, post-parse fatal (`ir::lower`) — for the
    /// degradation test.
    const NAV_FIXTURE_BROKEN: &str = "use ext;\nuse std::goToEnd as ge;\nnamespace ns { export inner() { right; } }\nsib() { left; }\nsucc() { 2: left; right(2); }\nexport main() {\n    helper() { 1: left; goto 1; }\n    @sib();\n    @succ();\n    @helper();\n    @ns::inner();\n    @inner();\n    @ext();\n    @ge();\n    @other::thing();\n    @mystery();\n    1: right;\n    check(1, !);\n    goto 1;\n    goto 99;\n}\n";

    /// 1-based (line, col) of the first byte of `anchor`'s Nth (0-based)
    /// occurrence in `src` — the fixture is pure ASCII, so byte offsets
    /// double as char offsets (`Span`'s "columns count characters"
    /// contract).
    fn pos_at_nth(src: &str, anchor: &str, n: usize) -> Pos {
        let mut search_from = 0;
        let mut found = None;
        for i in 0..=n {
            let idx = src[search_from..].find(anchor).unwrap_or_else(|| {
                panic!("occurrence {i} of {anchor:?} not found in fixture (search from byte {search_from})")
            });
            let abs = search_from + idx;
            found = Some(abs);
            search_from = abs + anchor.len();
        }
        pos_at_byte(src, found.unwrap())
    }

    /// `pos_at_nth(src, anchor, 0)` plus a `skip` char offset into the
    /// anchor — e.g. `pos_after(src, "@sib()", 1)` lands on the `s` of
    /// `sib`, skipping the `@`.
    fn pos_after(src: &str, anchor: &str, skip: usize) -> Pos {
        let start = src
            .find(anchor)
            .unwrap_or_else(|| panic!("{anchor:?} not found in fixture"));
        pos_at_byte(src, start + skip)
    }

    fn pos_at(src: &str, anchor: &str) -> Pos {
        pos_at_nth(src, anchor, 0)
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

    /// The `len_chars`-character span starting at `anchor`'s first
    /// occurrence — for a label's `"N:"` prefix (2 chars) sliced out of
    /// a longer, uniquely-identifying anchor like `"1: right;"`.
    fn span_at(src: &str, anchor: &str, len_chars: usize) -> Span {
        let start = pos_at(src, anchor);
        Span::new(
            start.line,
            start.col,
            start.line,
            start.col + len_chars as u32,
        )
    }

    /// The full anchor's own span (its character length).
    fn span_of(src: &str, anchor: &str) -> Span {
        span_at(src, anchor, anchor.chars().count())
    }

    #[test]
    fn local_call_resolves_to_the_top_level_definitions_name_span() {
        let mut service = PmcLanguageService::new();
        service.did_update(URI, NAV_FIXTURE);

        let pos = pos_after(NAV_FIXTURE, "@sib()", 1);
        let target = service.definition(URI, pos).expect("sib is local");
        assert_eq!(target.uri, URI);
        assert_eq!(target.span, span_of(NAV_FIXTURE, "sib"));
    }

    #[test]
    fn nested_call_resolves_to_the_nested_definitions_name_span() {
        let mut service = PmcLanguageService::new();
        service.did_update(URI, NAV_FIXTURE);

        let pos = pos_after(NAV_FIXTURE, "@helper()", 1);
        let target = service.definition(URI, pos).expect("helper is local");
        assert_eq!(target.uri, URI);
        assert_eq!(target.span, span_of(NAV_FIXTURE, "helper"));
    }

    #[test]
    fn qualified_internal_call_resolves_in_file() {
        let mut service = PmcLanguageService::new();
        service.did_update(URI, NAV_FIXTURE);

        let pos = pos_after(NAV_FIXTURE, "@ns::inner()", 1);
        let target = service
            .definition(URI, pos)
            .expect("ns::inner is defined in this module");
        assert_eq!(target.uri, URI);
        assert_eq!(target.span, span_of(NAV_FIXTURE, "inner"));
    }

    #[test]
    fn import_binding_call_resolves_to_the_use_span() {
        let mut service = PmcLanguageService::new();
        service.did_update(URI, NAV_FIXTURE);

        let pos = pos_after(NAV_FIXTURE, "@ext()", 1);
        let target = service
            .definition(URI, pos)
            .expect("ext is bound by a use import");
        assert_eq!(target.uri, URI);
        assert_eq!(target.span, span_of(NAV_FIXTURE, "ext"));
    }

    #[test]
    fn std_import_binding_call_resolves_to_the_materialized_roster_entry() {
        let mut service = PmcLanguageService::new();
        service.did_update(URI, NAV_FIXTURE);

        let pos = pos_after(NAV_FIXTURE, "@ge()", 1);
        let target = service
            .definition(URI, pos)
            .expect("ge is bound to std::goToEnd, and materialization succeeds in this env");

        assert!(target.uri.starts_with("file://"), "uri: {}", target.uri);
        let path = uri_to_path(&target.uri).expect("a file: uri decodes to a path");
        assert!(path.exists(), "materialized std.pmc must exist on disk");

        let entry = roster()
            .iter()
            .find(|e| e.full_path == "std::goToEnd")
            .expect("goToEnd is in the roster");
        assert_eq!(target.span, entry.name_span);
    }

    #[test]
    fn qualified_external_call_resolves_to_none() {
        let mut service = PmcLanguageService::new();
        service.did_update(URI, NAV_FIXTURE);

        let pos = pos_after(NAV_FIXTURE, "@other::thing()", 1);
        assert_eq!(service.definition(URI, pos), None);
    }

    #[test]
    fn unresolved_call_resolves_to_none() {
        let mut service = PmcLanguageService::new();
        service.did_update(URI, NAV_FIXTURE);

        let pos = pos_after(NAV_FIXTURE, "@mystery()", 1);
        assert_eq!(service.definition(URI, pos), None);
    }

    #[test]
    fn goto_reference_resolves_within_its_own_function_only() {
        let mut service = PmcLanguageService::new();
        service.did_update(URI, NAV_FIXTURE);

        let helper_label = span_at(NAV_FIXTURE, "1: left;", 2);
        let main_label = span_at(NAV_FIXTURE, "1: right;", 2);
        assert_ne!(
            helper_label, main_label,
            "sanity: the two labels really are at different positions"
        );

        // helper's own `goto 1;` (its statement ends inline `; }`,
        // distinguishing it from main's, which ends the line before a
        // bare `}`).
        let helper_goto = pos_after(NAV_FIXTURE, "goto 1; }", 5);
        let helper_target = service
            .definition(URI, helper_goto)
            .expect("goto 1 inside helper");
        assert_eq!(helper_target.span, helper_label);

        // main's own `goto 1;` — must resolve to MAIN's label, never
        // helper's same-valued one (no cross-function leak).
        let main_goto = pos_after(NAV_FIXTURE, "    goto 1;\n}", 9);
        let main_target = service
            .definition(URI, main_goto)
            .expect("goto 1 inside main");
        assert_eq!(main_target.span, main_label);
        assert_ne!(main_target.span, helper_label);
    }

    #[test]
    fn labeled_successor_reference_resolves_within_its_own_function() {
        let mut service = PmcLanguageService::new();
        service.did_update(URI, NAV_FIXTURE);

        let succ_label = span_at(NAV_FIXTURE, "2: left;", 2);
        let pos = pos_after(NAV_FIXTURE, "right(2)", 6);
        let target = service
            .definition(URI, pos)
            .expect("right(2)'s successor references label 2, inside succ itself");
        assert_eq!(target.uri, URI);
        assert_eq!(target.span, succ_label);
    }

    #[test]
    fn check_arm_reference_resolves_within_its_own_function() {
        let mut service = PmcLanguageService::new();
        service.did_update(URI, NAV_FIXTURE);

        let main_label = span_at(NAV_FIXTURE, "1: right;", 2);
        let pos = pos_after(NAV_FIXTURE, "check(1, !);", 6);
        let target = service
            .definition(URI, pos)
            .expect("check's marked arm references label 1 in main");
        assert_eq!(target.uri, URI);
        assert_eq!(target.span, main_label);
    }

    #[test]
    fn pos_inside_a_std_use_path_resolves_to_the_materialized_roster() {
        let mut service = PmcLanguageService::new();
        service.did_update(URI, NAV_FIXTURE);

        let pos = pos_at(NAV_FIXTURE, "goToEnd");
        let target = service
            .definition(URI, pos)
            .expect("pos sits inside use std::goToEnd's path");

        assert!(target.uri.starts_with("file://"), "uri: {}", target.uri);
        let entry = roster()
            .iter()
            .find(|e| e.full_path == "std::goToEnd")
            .expect("goToEnd is in the roster");
        assert_eq!(target.span, entry.name_span);
    }

    #[test]
    fn a_post_parse_fatal_degrades_every_definition_query_to_none() {
        let mut service = PmcLanguageService::new();
        let diags = service.did_update(URI, NAV_FIXTURE_BROKEN);
        assert!(
            diags.iter().any(|d| d.code == Some("undefined-label")),
            "sanity: goto 99 really does fatal, {diags:?}"
        );

        let positions = [
            pos_after(NAV_FIXTURE_BROKEN, "@sib()", 1),
            pos_after(NAV_FIXTURE_BROKEN, "@succ()", 1),
            pos_after(NAV_FIXTURE_BROKEN, "@helper()", 1),
            pos_after(NAV_FIXTURE_BROKEN, "@ns::inner()", 1),
            pos_after(NAV_FIXTURE_BROKEN, "@ext()", 1),
            pos_after(NAV_FIXTURE_BROKEN, "@ge()", 1),
            pos_after(NAV_FIXTURE_BROKEN, "@other::thing()", 1),
            pos_after(NAV_FIXTURE_BROKEN, "@mystery()", 1),
            pos_after(NAV_FIXTURE_BROKEN, "goto 1; }", 5),
            pos_after(NAV_FIXTURE_BROKEN, "right(2)", 6),
            pos_after(NAV_FIXTURE_BROKEN, "check(1, !);", 6),
            pos_at(NAV_FIXTURE_BROKEN, "goToEnd"),
        ];
        for pos in positions {
            assert_eq!(
                service.definition(URI, pos),
                None,
                "pos {pos:?} must degrade to None once analysis fails"
            );
        }
    }
}
