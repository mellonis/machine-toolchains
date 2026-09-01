//! Go-to-definition (docs/lsp.md (go-to-definition)): resolves a document
//! position to a [`DefTarget`] through a four-step resolution order —
//! the resolution table (a call's name), a label reference (`goto` /
//! `check` / a labeled successor), a `use std::…` path, else `None`. A
//! resolution-table hit or `use`-path segment naming something this
//! document does NOT itself define is tried against the document's
//! cross-file [`super::overlay::Overlay`] before falling back to today's
//! single-file behavior (docs/lsp.md (project overlay)): a sibling's own
//! declaration wins over an `ImportBinding`'s bare `use`-span jump, and
//! over `QualifiedExternal`'s/`Unresolved`'s plain `None`. A `std::` path
//! is no exception to that overlay-first order (docs/pmt/project.md
//! (libraries)): the overlay is consulted FIRST regardless of the
//! `std::` prefix — a sibling's own `namespace std { export … }` shadows
//! the embedded copy exactly the way a user's own object shadows a
//! library at link time — and only a genuine overlay miss falls through
//! to the materialized stdlib roster, itself still gated on
//! [`super::std_enabled`]. An overlay hit with no source location (a
//! `.pmo`-backed symbol) yields `None` rather than a bogus jump — even
//! when the missing location is for a shadowed `std::` name, since the
//! overlay already OWNS that name and the materialized stdlib must not
//! be consulted behind its back. Analysis-tier: every query degrades to
//! `None` when `DocState::analysis` is `None` (a post-parse fatal
//! anywhere in the document), not just the part that failed.

use std::rc::Rc;

use mtc_core::diagnostics::{Pos, Span};
use mtc_core::lsp::DefTarget;
use mtc_core::syntax::{AstNode, SyntaxNode, TextLineIndex};

use crate::compiler::{Analysis, Resolution};
use crate::stdlib::{materialized_std_uri, roster};
use crate::syntax::{FileView, FunctionView, TopView, extract_statement};

use super::DocState;
use super::walk::{enclosing_function_chain, function_labels, label_refs, span_contains};
use super::{overlay_owns, std_enabled};

/// Step 1's shared scan — the ONE place a position is hit-tested
/// against the resolution table: the entry whose call-site span
/// contains `pos`, as `(origin span, resolution)`. Both [`definition`]
/// and [`hover_target`] start here; only what they DO with the hit
/// differs (a `DefTarget` location vs a qualified name).
fn resolve_at(analysis: &Analysis, pos: Pos) -> Option<(Span, &Resolution)> {
    analysis
        .resolutions
        .iter()
        .find(|(span, _)| span_contains(*span, pos))
        .map(|(span, resolution)| (*span, resolution))
}

/// The definition target for `pos` in `uri`'s current document
/// (docs/lsp.md (go-to-definition)):
///
/// 1. a resolution-table entry whose span contains `pos` (the call name
///    under the cursor) — resolved per its [`Resolution`] variant, every
///    path (std-prefixed or not) consulting the document's cross-file
///    overlay before falling back to its own single-file behavior — a
///    `std::` path's fallback is the materialized roster (gated on
///    [`std_enabled`]), see [`std_path_target`];
/// 2. failing that, a label reference (`goto` target, a `check` arm, or
///    a labeled successor) hit-tested against the innermost enclosing
///    function's own labels;
/// 3. failing that, a `use …` path segment — `std::` through
///    [`std_path_target`] (overlay first, materialized roster on a
///    miss), any other path through the overlay alone;
/// 4. otherwise `None`.
pub(super) fn definition(state: &DocState, uri: &str, pos: Pos) -> Option<DefTarget> {
    let analysis = state.analysis.as_ref()?;

    if let Some((origin, resolution)) = resolve_at(analysis, pos) {
        return resolve_call(state, uri, resolution, origin);
    }

    let green = state.green.as_ref()?;
    let root = SyntaxNode::new_root(Rc::clone(green));
    let file = FileView::cast(root).expect("root is FILE");
    let index = &state.line_index;
    let offset = index.offset(pos);

    if let Some(function) = enclosing_function_chain(&file, offset).pop()
        && let Some((value, origin)) = label_reference_at(&function, index, pos)
    {
        return label_span(&function, index, value).map(|span| DefTarget {
            uri: uri.to_string(),
            span,
            origin: Some(origin),
        });
    }

    if let Some((full_path, origin)) = use_path_at(&file, index, offset) {
        return if full_path.starts_with("std::") {
            std_path_target(state, &full_path, origin)
        } else {
            overlay_target(state, &full_path, origin)
        };
    }

    None
}

/// Step 1's per-variant resolution. `origin` is the call-site name span
/// that `resolution` was keyed by (the reference under the cursor) —
/// carried through to every arm's `DefTarget`. `state` supplies both the
/// cross-file overlay (`Resolution::Local` never needs it — a local
/// definition always wins on its own) and, for `Unresolved`, the raw
/// source text a bare call's written name is sliced from.
fn resolve_call(
    state: &DocState,
    uri: &str,
    resolution: &Resolution,
    origin: Span,
) -> Option<DefTarget> {
    match resolution {
        Resolution::Local { def_name_span } => Some(DefTarget {
            uri: uri.to_string(),
            span: *def_name_span,
            origin: Some(origin),
        }),
        Resolution::ImportBinding {
            use_span,
            full_path,
        } => {
            if full_path.starts_with("std::") {
                // `std_path_target` already tries the overlay before the
                // roster — no separate overlay attempt here, and (like
                // the sibling `QualifiedExternal` arm below) no
                // `use_span` fallback on a miss: that fallback is this
                // arm's own single-file behavior for a name nothing
                // cross-file defines, not std's.
                std_path_target(state, full_path, origin)
            } else if let Some(target) = overlay_target(state, full_path, origin) {
                Some(target)
            } else {
                // No overlay hit (no project, no matching sibling/library
                // export, or a name-only `.pmo` symbol) — today's
                // single-file behavior: jump to this document's own `use`
                // statement.
                Some(DefTarget {
                    uri: uri.to_string(),
                    span: *use_span,
                    origin: Some(origin),
                })
            }
        }
        Resolution::QualifiedExternal { full_path } => {
            if full_path.starts_with("std::") {
                std_path_target(state, full_path, origin)
            } else {
                overlay_target(state, full_path, origin)
            }
        }
        Resolution::Unresolved => {
            // No name is carried on this variant at all — recover the
            // written token straight from source before consulting the
            // overlay under it (bare top-level exports are keyed by
            // their bare name).
            let written = text_at_span(&state.text, origin)?;
            overlay_target(state, written, origin)
        }
    }
}

/// One `full_path` (or, for `Resolution::Unresolved`, the call's own
/// written bare name) resolved through `state`'s cross-file
/// [`super::overlay::Overlay`] (docs/lsp.md (project overlay)): a hit whose
/// `OverlaySym.target` carries a source location becomes a `DefTarget`
/// keyed by `origin`; no overlay at all, a name miss, or a name-only hit
/// (a `.pmo`-backed symbol, which has no location to jump to) all
/// degrade to `None`, leaving the caller free to fall through to its own
/// next step.
fn overlay_target(state: &DocState, full_path: &str, origin: Span) -> Option<DefTarget> {
    let overlay = state.overlay.as_ref()?;
    let (target_uri, span) = overlay.symbols.get(full_path)?.target.as_ref()?;
    Some(DefTarget {
        uri: target_uri.clone(),
        span: *span,
        origin: Some(origin),
    })
}

/// The one seam every `std::`-prefixed full path funnels through
/// (docs/pmt/project.md (libraries)): the embedded stdlib links as an
/// ordinary library, LAST, behind every declared source and library — a
/// sibling or library exporting the same mangled name under `std::`
/// shadows it, the identical user-object-beats-library precedent the
/// linker itself follows. So this checks OWNERSHIP first —
/// [`overlay_owns`] — not [`overlay_target`]'s `Option`: the overlay can
/// OWN a name yet still answer no location (a `.pma`/`.pmo` shadow), and
/// that must still short-circuit here rather than falling through to the
/// materialized stdlib behind the owner's back. Only a genuine miss — no
/// project, or no sibling/library defines this name at all — reaches the
/// materialized roster, itself still gated on [`std_enabled`] (a project
/// declaring `"stdlib": false` gets no materialized jump, exactly as
/// before). Every one of `navigate.rs`'s three `std::`-branch sites
/// reduces to a call here, so a future arch twin only needs to copy this
/// one function's shape, not each call site's.
fn std_path_target(state: &DocState, full_path: &str, origin: Span) -> Option<DefTarget> {
    if overlay_owns(state, full_path) {
        overlay_target(state, full_path, origin)
    } else if std_enabled(state) {
        std_target(full_path, origin)
    } else {
        None
    }
}

/// Slices the literal source text `span` denotes straight out of `text` —
/// the one place [`Resolution::Unresolved`] (which carries no name of its
/// own) recovers the written call name for an overlay lookup. Spans are
/// 1-based, half-open (docs/lsp.md (position encoding)); this walks lines
/// via `text.split('\n')` and each line's `chars()`/`char_indices()`
/// exactly the way `mtc_core::lsp`'s position mapper does — column
/// offsets are character counts, never byte counts, so a multi-byte UTF-8
/// character anywhere on the line before `span` cannot corrupt the slice
/// (`char_indices` only ever yields valid char-boundary byte offsets).
/// `None` for a multi-line span (no name-carrying resolution ever
/// produces one), a line past end-of-file, or an end column before the
/// start column.
pub(super) fn text_at_span(text: &str, span: Span) -> Option<&str> {
    if span.start.line != span.end.line {
        return None;
    }
    let line_ix = span.start.line.checked_sub(1)?;
    let line = text.split('\n').nth(line_ix as usize)?;
    let line = line.strip_suffix('\r').unwrap_or(line);

    let start_char = span.start.col.checked_sub(1)?;
    let end_char = span.end.col.checked_sub(1)?;
    if end_char < start_char {
        return None;
    }

    let mut start_byte = None;
    let mut end_byte = None;
    let mut count = 0u32;
    for (byte_ix, _) in line.char_indices() {
        if count == start_char {
            start_byte = Some(byte_ix);
        }
        if count == end_char {
            end_byte = Some(byte_ix);
        }
        count += 1;
    }
    if count == start_char {
        start_byte = Some(line.len());
    }
    if count == end_char {
        end_byte = Some(line.len());
    }

    line.get(start_byte?..end_byte?)
}

/// A `std::…` full path through the materialized roster: a non-`std`
/// path, a roster miss, or a materializer IO failure all degrade to
/// `None` (docs/lsp.md (materialized standard library)). `origin` is the
/// reference span in the requesting document, carried through
/// unconditionally. The `std::` guard matters now that [`use_path_at`]
/// returns EVERY path, not just std-prefixed ones (hover's own use of
/// it, below) — without it, a local (non-std) path would still miss
/// (the roster has no non-std entries) but only after paying for a
/// materialization attempt first.
fn std_target(full_path: &str, origin: Span) -> Option<DefTarget> {
    if !full_path.starts_with("std::") {
        return None;
    }
    let uri = materialized_std_uri()?;
    let entry = roster().iter().find(|e| e.full_path == full_path)?;
    Some(DefTarget {
        uri: uri.to_string(),
        span: entry.name_span,
        origin: Some(origin),
    })
}

/// The label value referenced at `pos`, plus the reference's own span
/// (the origin), if `pos` sits on one of `function`'s own reference
/// spans — `walk::label_refs`' shared enumeration over each comma-group
/// item, first hit wins. Only `function`'s OWN statements are examined —
/// its nested children are a separate label scope, reached only by
/// `walk::enclosing_function_chain` descending into them for a `pos`
/// that lands there.
///
/// The items come from `crate::syntax::extract_statement`, the parser's
/// own production, so `label_refs` sees exactly the `Item` the compiler
/// sees.
fn label_reference_at(
    function: &FunctionView,
    index: &TextLineIndex,
    pos: Pos,
) -> Option<(u32, Span)> {
    for stmt in function.statements() {
        for item in extract_statement(&stmt, index).items {
            for (value, span) in label_refs(&item).into_iter().flatten() {
                if span_contains(span, pos) {
                    return Some((value, span));
                }
            }
        }
    }
    None
}

/// `value`'s label declaration span within `function`'s OWN statements
/// (labels are function-scoped — never searched in nested children or
/// enclosing scopes), via `walk::function_labels`' shared scan.
fn label_span(function: &FunctionView, index: &TextLineIndex, value: u32) -> Option<Span> {
    function_labels(function, index)
        .into_iter()
        .find(|label| label.value == value)
        .map(|label| label.span)
}

/// Step 3: `pos` inside a `use …` path's span → its joined full path
/// (`"std::goToEnd"`, `"ns::helper"`) plus the path's own span
/// (`UsePath.span`), the origin. Searched recursively through namespace
/// blocks — imports are legal at any nesting level. Every path is
/// returned, not just `std::…` ones — each caller does its OWN
/// `std::`-branching at its own seam instead: [`definition`]'s step 3
/// routes a `std::` path through [`std_path_target`] (overlay first, the
/// materialized roster only on a miss, itself still gated on
/// [`super::std_enabled`]) and everything else through [`overlay_target`]
/// alone, which can genuinely SUCCEED for a sibling's own `use`-imported
/// path, not just miss; hover's caller (`mod.rs`) looks up whatever
/// qualified name comes back against this document's own
/// `Analysis.docs`, the overlay's doc map, or the stdlib's — local,
/// sibling, and `std::` names alike. Filtering by `std` here would only
/// duplicate work every caller already does on its own.
fn use_path_at(file: &FileView, index: &TextLineIndex, offset: u32) -> Option<(String, Span)> {
    fn descend(
        items: impl Iterator<Item = TopView>,
        index: &TextLineIndex,
        offset: u32,
    ) -> Option<(String, Span)> {
        for item in items {
            match item {
                TopView::Namespace(ns) => {
                    if ns.syntax().text_range().contains(offset)
                        && let Some(result) = descend(ns.items(), index, offset)
                    {
                        return Some(result);
                    }
                }
                TopView::Use(use_decl) => {
                    for path in use_decl.paths() {
                        // Alias-exclusive on purpose: `UsePath.span`
                        // (docs/core.md (syntax tree)) never covers an
                        // `as` alias, so both the containment test and
                        // the returned span are built from the segment
                        // tokens the way `extract.rs::extract_import`
                        // does — the node's own `text_range()` would
                        // include the alias and let a cursor sitting on
                        // it resolve to the path.
                        let segments = path.segments();
                        let first = segments
                            .first()
                            .expect("USE_PATH always carries at least one segment");
                        let last = segments
                            .last()
                            .expect("USE_PATH always carries at least one segment");
                        let range = mtc_core::syntax::TextRange::new(
                            first.text_range().start,
                            last.text_range().end,
                        );
                        if range.contains(offset) {
                            let joined = segments
                                .iter()
                                .map(|t| t.text().to_string())
                                .collect::<Vec<_>>()
                                .join("::");
                            return Some((joined, index.span(range)));
                        }
                    }
                }
                TopView::Function(_) => {}
            }
        }
        None
    }
    descend(file.items(), index, offset)
}

/// Hover's own position→target resolution (docs/lsp.md (hover)): the
/// documented target's fully-qualified name — `Analysis.docs`' own key
/// form, or a cross-file overlay symbol's own key — plus the origin span
/// of the reference under the cursor. Shares every WALK [`definition`]
/// uses (the resolution table, and [`use_path_at`] above) instead of
/// re-walking the tree a second time; only the OUTPUT shape differs (a
/// name here, a `DefTarget` location there). Step order:
///
/// 1. a resolution-table entry whose span contains `pos` (a call site)
///    — the shared [`resolve_at`] scan — resolved to a name via
///    [`resolution_qualified_name`]; when that comes back empty (only
///    `Resolution::Unresolved` ever does — it carries no name of its
///    own), the written token is recovered via [`text_at_span`] and
///    tried against the overlay directly, since a bare call's overlay
///    key IS its written name;
/// 2. failing that, a function's OWN declaration name — hover-only
///    (`definition` never needs to resolve a position sitting ON a
///    definition: the location IS the definition already). Every
///    flattened function's `name` IS the qualified form `Analysis.docs`
///    is keyed by, and its `name_span` survives flatten unchanged;
/// 3. failing that, a `use …` path segment, via [`use_path_at`] —
///    generalized beyond std, since a qualified name is enough for a
///    doc lookup even when there's no on-disk location to jump to;
/// 4. otherwise `None`.
///
/// Analysis-tier, same as `definition`: every query degrades to `None`
/// once `DocState::analysis` is `None`. Doc-map lookup (local, stdlib, or
/// overlay), the content-emptiness gate, and rendering are `mod.rs`'s
/// job — this function only ever answers a NAME, never doc content.
pub(super) fn hover_target(state: &DocState, pos: Pos) -> Option<(String, Span)> {
    let analysis = state.analysis.as_ref()?;

    if let Some((origin, resolution)) = resolve_at(analysis, pos) {
        if let Some(name) = resolution_qualified_name(analysis, resolution) {
            return Some((name, origin));
        }
        let written = text_at_span(&state.text, origin)?;
        let overlay = state.overlay.as_ref()?;
        return overlay
            .symbols
            .contains_key(written)
            .then(|| (written.to_string(), origin));
    }

    if let Some(f) = analysis
        .ast
        .functions
        .iter()
        .find(|f| span_contains(f.name_span, pos))
    {
        return Some((f.name.clone(), f.name_span));
    }

    let green = state.green.as_ref()?;
    let root = SyntaxNode::new_root(Rc::clone(green));
    let file = FileView::cast(root).expect("root is FILE");
    let index = &state.line_index;
    let offset = index.offset(pos);
    use_path_at(&file, index, offset)
}

/// The fully-qualified name a step-1 [`Resolution`] ultimately names —
/// as opposed to [`resolve_call`]'s go-to-definition SHAPE (a
/// `DefTarget` location). `Resolution::Local` only carries
/// `def_name_span` (a name would be redundant with the `DefTarget.span`
/// a go-to-definition query needs — see `compiler.rs::flatten`'s
/// post-pass comment); hover needs the NAME instead, recovered by a
/// plain linear scan over this document's own flattened functions —
/// small, no caching warranted, and exactly mirrors the CONTAINS scan
/// `hover_target`'s step 2 already does, just with span EQUALITY
/// instead (the target IS some function's own `name_span`, exactly).
/// `ImportBinding`/`QualifiedExternal` already carry the qualified
/// string verbatim (mangling never touches an external path);
/// `Unresolved` carries no name at all here — [`hover_target`] recovers
/// one itself, via [`text_at_span`] plus the overlay, when this function
/// comes back empty.
fn resolution_qualified_name(analysis: &Analysis, resolution: &Resolution) -> Option<String> {
    match resolution {
        Resolution::Local { def_name_span } => analysis
            .ast
            .functions
            .iter()
            .find(|f| f.name_span == *def_name_span)
            .map(|f| f.name.clone()),
        Resolution::ImportBinding { full_path, .. } => Some(full_path.clone()),
        Resolution::QualifiedExternal { full_path } => Some(full_path.clone()),
        Resolution::Unresolved => None,
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU32, Ordering};

    use mtc_core::diagnostics::Pos;
    use mtc_core::lsp::LanguageService;

    use super::super::PmcLanguageService;
    use super::*;
    use crate::lsp::uri_to_path;

    const URI: &str = "untitled:Nav-1";

    /// A fresh scratch directory under `std::env::temp_dir()`, unique per
    /// call (process id + an atomic counter — this crate has no tempfile
    /// dependency, matching the zero-new-deps constraint; house
    /// convention has no shared test-support module, so each file defines
    /// its own local helper — mirrors `overlay.rs`'s and `complete.rs`'s
    /// own copies).
    fn unique_tmp_dir(label: &str) -> PathBuf {
        static N: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "pmt-navigate-{label}-{}-{}",
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

    /// `pos_after(src, anchor, skip)` plus a `len_chars`-character span
    /// from there — the origin span of a reference token embedded inside
    /// a longer, uniquely-identifying anchor (e.g. the `"sib"` in
    /// `"@sib()"`, skipping the `@`).
    fn span_after(src: &str, anchor: &str, skip: usize, len_chars: usize) -> Span {
        let start = pos_after(src, anchor, skip);
        Span::new(
            start.line,
            start.col,
            start.line,
            start.col + len_chars as u32,
        )
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
    fn span_contains_excludes_a_position_exactly_at_the_end() {
        // Half-open contract (this module's `span_contains` doc comment):
        // `end` is one past the last contained position, so a cursor
        // sitting exactly there belongs to whatever comes NEXT, not this
        // span — the off-by-one this test pins.
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

    /// Go-to-definition on a label reference still finds the label's own
    /// declaration in the same function — the walk that moved from the
    /// hand-written CST to green-tree views. `007` also pins that the
    /// label VALUE survives extraction: a token-text re-derivation
    /// would either hand back
    /// `007` (unparseable as a value) or lose the written form.
    #[test]
    fn label_reference_resolves_to_its_declaration_after_the_view_migration() {
        const SRC: &str = "main() {\n    007: right;\n    goto 007;\n}\n";
        let mut service = PmcLanguageService::new();
        service.did_update(URI, SRC);

        // The `007` inside `goto 007;` — skip past `"goto "`.
        let pos = pos_after(SRC, "goto 007", 5);
        let target = service
            .definition(URI, pos)
            .expect("resolves to the label declaration");
        assert_eq!(target.uri, URI);
        assert_eq!(target.span.start, Pos { line: 2, col: 5 });
    }

    #[test]
    fn local_call_resolves_to_the_top_level_definitions_name_span() {
        let mut service = PmcLanguageService::new();
        service.did_update(URI, NAV_FIXTURE);

        let pos = pos_after(NAV_FIXTURE, "@sib()", 1);
        let target = service.definition(URI, pos).expect("sib is local");
        assert_eq!(target.uri, URI);
        assert_eq!(target.span, span_of(NAV_FIXTURE, "sib"));
        assert_eq!(target.origin, Some(span_after(NAV_FIXTURE, "@sib()", 1, 3)));
    }

    #[test]
    fn nested_call_resolves_to_the_nested_definitions_name_span() {
        let mut service = PmcLanguageService::new();
        service.did_update(URI, NAV_FIXTURE);

        let pos = pos_after(NAV_FIXTURE, "@helper()", 1);
        let target = service.definition(URI, pos).expect("helper is local");
        assert_eq!(target.uri, URI);
        assert_eq!(target.span, span_of(NAV_FIXTURE, "helper"));
        assert_eq!(
            target.origin,
            Some(span_after(NAV_FIXTURE, "@helper()", 1, 6))
        );
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
        assert_eq!(
            target.origin,
            Some(span_after(NAV_FIXTURE, "@ns::inner()", 1, 9))
        );
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
        assert_eq!(target.origin, Some(span_after(NAV_FIXTURE, "@ext()", 1, 3)));
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
        assert_eq!(target.origin, Some(span_after(NAV_FIXTURE, "@ge()", 1, 2)));
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
        assert_eq!(
            helper_target.origin,
            Some(span_after(NAV_FIXTURE, "goto 1; }", 5, 1))
        );

        // main's own `goto 1;` — must resolve to MAIN's label, never
        // helper's same-valued one (no cross-function leak).
        let main_goto = pos_after(NAV_FIXTURE, "    goto 1;\n}", 9);
        let main_target = service
            .definition(URI, main_goto)
            .expect("goto 1 inside main");
        assert_eq!(main_target.span, main_label);
        assert_ne!(main_target.span, helper_label);
        assert_eq!(
            main_target.origin,
            Some(span_after(NAV_FIXTURE, "    goto 1;\n}", 9, 1))
        );
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
        assert_eq!(
            target.origin,
            Some(span_after(NAV_FIXTURE, "right(2)", 6, 1))
        );
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
        assert_eq!(
            target.origin,
            Some(span_after(NAV_FIXTURE, "check(1, !);", 6, 1))
        );
    }

    #[test]
    fn check_arm_reference_resolves_when_the_blank_arm_is_the_label() {
        // `check(1, !)` above only exercises the `marked` arm's own
        // `CheckArm::Label` branch; this pins the `blank` arm's sibling
        // branch (docs/pmt/language.md (check(A1, A2))) by swapping which
        // side carries the label — `!` marked, `3` blank.
        const FIXTURE: &str = "main() {\n    3: right;\n    check(!, 3);\n}\n";
        let mut service = PmcLanguageService::new();
        service.did_update(URI, FIXTURE);

        let label = span_at(FIXTURE, "3: right;", 2);
        let pos = pos_after(FIXTURE, "check(!, 3);", 9);
        let target = service
            .definition(URI, pos)
            .expect("check's blank arm references label 3 in main");
        assert_eq!(target.uri, URI);
        assert_eq!(target.span, label);
        assert_eq!(
            target.origin,
            Some(span_after(FIXTURE, "check(!, 3);", 9, 1))
        );
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
        assert_eq!(target.origin, Some(span_of(NAV_FIXTURE, "std::goToEnd")));
    }

    #[test]
    fn pos_inside_a_namespaced_use_path_resolves_to_the_materialized_roster() {
        // `use_path_at`'s `ns…contains(offset)` guard is a pure
        // narrowing over the recursion into a namespace's own items, but
        // nothing had exercised a `use` declaration NESTED inside a
        // namespace through go-to-definition before this fixture — every
        // other `use` test here sits at top level.
        const SRC: &str =
            "namespace ns {\n    use std::goToEnd;\n    export inner() { @goToEnd(); }\n}\n";
        let mut service = PmcLanguageService::new();
        service.did_update(URI, SRC);

        let pos = pos_at(SRC, "goToEnd");
        let target = service
            .definition(URI, pos)
            .expect("pos sits inside the namespaced use std::goToEnd's path");

        assert!(target.uri.starts_with("file://"), "uri: {}", target.uri);
        let entry = roster()
            .iter()
            .find(|e| e.full_path == "std::goToEnd")
            .expect("goToEnd is in the roster");
        assert_eq!(target.span, entry.name_span);
        assert_eq!(target.origin, Some(span_of(SRC, "std::goToEnd")));
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

    // --- Task 7: cross-file navigation + hover through the overlay ---

    #[test]
    fn text_at_span_slices_by_characters_not_bytes() {
        // The line ABOVE the target carries a multi-byte char — proves
        // `text.split('\n')`'s own line-splitting is untroubled by it
        // (a plain ASCII delimiter search, byte-safe regardless of what
        // sits on other lines).
        const ACROSS_LINES: &str = "?é\nhelper();\n";
        assert_eq!(
            text_at_span(ACROSS_LINES, Span::new(2, 1, 2, 7)),
            Some("helper")
        );

        // The load-bearing case: a multi-byte char on the SAME line,
        // BEFORE the target — `é` is 2 bytes/1 char, so a byte-indexed
        // slice would land two bytes short and read `" helpe"` instead.
        const SAME_LINE: &str = "@é helper";
        assert_eq!(
            text_at_span(SAME_LINE, Span::new(1, 4, 1, 10)),
            Some("helper")
        );

        // A line past end-of-file degrades to `None` rather than
        // panicking (`ACROSS_LINES` has 3 lines after the trailing
        // `split('\n')` empty tail — line 9 doesn't exist).
        assert_eq!(text_at_span(ACROSS_LINES, Span::new(9, 1, 9, 2)), None);
    }

    #[test]
    fn definition_jumps_into_a_pmc_sibling() {
        // `ns::inner` is not defined anywhere in app.pmc itself, so the
        // resolution table types the call `QualifiedExternal` — the
        // overlay leg on THAT arm is what this pins.
        let dir = unique_tmp_dir("nav-pmc-sibling");
        fs::write(
            dir.join("pmt.json"),
            r#"{"project":{"targets":{"app":{"sources":["app.pmc","shared.pmc"]}}}}"#,
        )
        .unwrap();
        const SHARED: &str = "namespace ns {\nexport inner() { right; }\n}\n";
        fs::write(dir.join("shared.pmc"), SHARED).unwrap();

        let mut service = PmcLanguageService::new();
        let app_uri = file_uri(&dir.join("app.pmc"));
        const SRC: &str = "export main() {\n    @ns::inner();\n}\n";
        service.did_update(&app_uri, SRC);

        let pos = pos_after(SRC, "@ns::inner()", 1);
        let target = service
            .definition(&app_uri, pos)
            .expect("ns::inner is defined in the shared.pmc sibling");

        assert_eq!(target.uri, file_uri(&dir.join("shared.pmc")));
        assert_eq!(target.span, span_of(SHARED, "inner"));
        assert_eq!(target.origin, Some(span_after(SRC, "@ns::inner()", 1, 9)));
    }

    #[test]
    fn definition_jumps_into_a_pma_sibling() {
        // `greet` is bare, undeclared anywhere in app.pmc (no local def,
        // no `use`) — the resolution table types the call `Unresolved`.
        // The sibling is `.pma`, exercising `exports_from_pma`'s own
        // `FuncCst.name_span` as the overlay target, through the SAME
        // `Unresolved` arm `unresolved_bare_call_resolves_through_the_overlay`
        // exercises with a `.pmc` sibling below.
        let dir = unique_tmp_dir("nav-pma-sibling");
        fs::write(
            dir.join("pmt.json"),
            r#"{"project":{"targets":{"app":{"sources":["app.pmc","shared.pma"]}}}}"#,
        )
        .unwrap();
        const SHARED: &str = ".func greet\nstp\n";
        fs::write(dir.join("shared.pma"), SHARED).unwrap();

        let mut service = PmcLanguageService::new();
        let app_uri = file_uri(&dir.join("app.pmc"));
        const SRC: &str = "export main() {\n    @greet();\n}\n";
        service.did_update(&app_uri, SRC);

        let pos = pos_after(SRC, "@greet()", 1);
        let target = service
            .definition(&app_uri, pos)
            .expect("greet is defined in the shared.pma sibling");

        assert_eq!(target.uri, file_uri(&dir.join("shared.pma")));
        assert_eq!(target.span, span_of(SHARED, "greet"));
    }

    #[test]
    fn import_binding_prefers_the_sibling_definition_over_the_use_span() {
        // Half A: `ext` is bound by `use ext;` AND exported by the
        // sibling — the sibling's own definition must win over the
        // `use`-span fallback `import_binding_call_resolves_to_the_use_span`
        // (above, no-overlay `NAV_FIXTURE`) pins as the single-file
        // baseline.
        let dir = unique_tmp_dir("nav-import-binding-overlay");
        fs::write(
            dir.join("pmt.json"),
            r#"{"project":{"targets":{"app":{"sources":["app.pmc","shared.pmc"]}}}}"#,
        )
        .unwrap();
        const SHARED: &str = "export ext() { right; }\n";
        fs::write(dir.join("shared.pmc"), SHARED).unwrap();

        let mut service = PmcLanguageService::new();
        let app_uri = file_uri(&dir.join("app.pmc"));
        const SRC: &str = "use ext;\nexport main() {\n    @ext();\n}\n";
        service.did_update(&app_uri, SRC);

        let pos = pos_after(SRC, "@ext()", 1);
        let target = service
            .definition(&app_uri, pos)
            .expect("ext is bound by use AND exported by the sibling");
        assert_eq!(target.uri, file_uri(&dir.join("shared.pmc")));
        assert_eq!(target.span, span_of(SHARED, "ext"));

        // Half B: same project (an overlay DOES exist), but `ghost` is
        // bound by `use` with NO sibling exporting it — the overlay leg
        // must miss and fall through to today's `use`-span behavior,
        // not to `None`. This is the case the brief's own report
        // contract asks to be pinned distinctly from the no-overlay-at-
        // all baseline above.
        const GHOST_SRC: &str = "use ghost;\nexport main() {\n    @ghost();\n}\n";
        service.did_update(&app_uri, GHOST_SRC);
        let ghost_pos = pos_after(GHOST_SRC, "@ghost()", 1);
        let ghost_target = service
            .definition(&app_uri, ghost_pos)
            .expect("ghost falls back to its own use span when the overlay misses");
        assert_eq!(ghost_target.uri, app_uri);
        assert_eq!(ghost_target.span, span_of(GHOST_SRC, "ghost"));
    }

    #[test]
    fn unresolved_bare_call_resolves_through_the_overlay() {
        let dir = unique_tmp_dir("nav-unresolved-bare");
        fs::write(
            dir.join("pmt.json"),
            r#"{"project":{"targets":{"app":{"sources":["app.pmc","shared.pmc"]}}}}"#,
        )
        .unwrap();
        const SHARED: &str = "?Frobs the tape.\nexport helper() { right; }\n";
        fs::write(dir.join("shared.pmc"), SHARED).unwrap();

        let mut service = PmcLanguageService::new();
        let app_uri = file_uri(&dir.join("app.pmc"));
        const SRC: &str = "export main() {\n    @helper();\n}\n";
        service.did_update(&app_uri, SRC);

        let pos = pos_after(SRC, "@helper()", 1);
        let target = service
            .definition(&app_uri, pos)
            .expect("the bare call resolves through the overlay to the sibling's export");
        assert_eq!(target.uri, file_uri(&dir.join("shared.pmc")));
        assert_eq!(target.span, span_of(SHARED, "helper"));

        // Same position's hover: the sibling's doc line surfaces too
        // (the `Unresolved`-arm overlay leg `hover_target` gains in this
        // same task).
        let hover = service
            .hover(&app_uri, pos)
            .expect("hover on the same bare call carries the sibling's doc line");
        assert!(hover.text.contains("Frobs the tape."), "{hover:?}");
    }

    #[test]
    fn pmo_backed_names_navigate_null() {
        // Positive control FIRST: `known` is a `.pmc` sibling's bare
        // export, carrying a real span — proves the fixture's overall
        // wiring resolves through the overlay at all, so the `.pmo`
        // assertion below means "no location", not "overlay never
        // fired".
        let dir = unique_tmp_dir("nav-pmo-null");
        const KNOWN: &str = "export known() { right; }\n";
        fs::write(dir.join("known.pmc"), KNOWN).unwrap();

        let ghost_bytes = crate::compiler::compile(
            "export ghostlib() { right; }\n",
            crate::compiler::CompileOptions::default(),
        )
        .expect("ghostlib.pmc compiles")
        .object
        .to_bytes();
        fs::write(dir.join("ghostlib.pmo"), ghost_bytes).unwrap();

        fs::write(
            dir.join("pmt.json"),
            r#"{"project":{"targets":{"app":{"sources":["app.pmc","known.pmc","ghostlib.pmo"]}}}}"#,
        )
        .unwrap();

        let mut service = PmcLanguageService::new();
        let app_uri = file_uri(&dir.join("app.pmc"));
        const SRC: &str = "export main() {\n    @known();\n    @ghostlib();\n}\n";
        service.did_update(&app_uri, SRC);

        let known_pos = pos_after(SRC, "@known()", 1);
        let known_target = service
            .definition(&app_uri, known_pos)
            .expect("positive control: known is source-backed with a real span");
        assert_eq!(known_target.uri, file_uri(&dir.join("known.pmc")));
        assert_eq!(known_target.span, span_of(KNOWN, "known"));

        let ghost_pos = pos_after(SRC, "@ghostlib()", 1);
        assert_eq!(
            service.definition(&app_uri, ghost_pos),
            None,
            "a .pmo-backed overlay symbol carries no source location to jump to"
        );

        // The SAME `.pmo`-backed miss through `ImportBinding` instead of
        // `Unresolved` is a DIFFERENT, deliberate outcome (the comment on
        // `resolve_call`'s `ImportBinding` fallback documents this): the
        // overlay hit still carries no location, but that arm falls back
        // to the `use` statement's own span rather than degrading to
        // `None`.
        const IMPORT_SRC: &str = "use ghostlib;\nexport main() {\n    @ghostlib();\n}\n";
        service.did_update(&app_uri, IMPORT_SRC);
        let import_pos = pos_after(IMPORT_SRC, "@ghostlib()", 1);
        let import_target = service
            .definition(&app_uri, import_pos)
            .expect("a .pmo-backed ImportBinding falls back to its own use span, not None");
        assert_eq!(import_target.uri, app_uri);
        assert_eq!(import_target.span, span_of(IMPORT_SRC, "ghostlib"));
    }

    #[test]
    fn hover_carries_the_siblings_doc_lines() {
        // `ns::inner` is `QualifiedExternal` (not defined in app.pmc) —
        // `resolution_qualified_name` already answers its qualified name
        // today; this pins that `mod.rs`'s hover doc chain then finds
        // that name in the OVERLAY's doc map, not this document's own
        // (which never flattened a sibling's function at all).
        let dir = unique_tmp_dir("nav-hover-overlay-doc");
        fs::write(
            dir.join("pmt.json"),
            r#"{"project":{"targets":{"app":{"sources":["app.pmc","shared.pmc"]}}}}"#,
        )
        .unwrap();
        fs::write(
            dir.join("shared.pmc"),
            "namespace ns {\n?Frobs the tape.\nexport inner() { right; }\n}\n",
        )
        .unwrap();

        let mut service = PmcLanguageService::new();
        let app_uri = file_uri(&dir.join("app.pmc"));
        const SRC: &str = "export main() {\n    @ns::inner();\n}\n";
        service.did_update(&app_uri, SRC);

        let pos = pos_after(SRC, "@ns::inner()", 1);
        let hover = service
            .hover(&app_uri, pos)
            .expect("the sibling's doc line surfaces through the overlay");
        assert!(hover.text.contains("Frobs the tape."), "{hover:?}");
    }

    #[test]
    fn stdlib_false_kills_std_hover_and_the_materialized_jump() {
        let dir = unique_tmp_dir("nav-stdlib-false");
        fs::write(
            dir.join("pmt.json"),
            r#"{"project":{"stdlib":false,"targets":{"app":{"sources":["app.pmc"]}}}}"#,
        )
        .unwrap();

        let mut service = PmcLanguageService::new();
        let app_uri = file_uri(&dir.join("app.pmc"));

        // Non-std, LOCAL half: proves the gate silences only std, not
        // hover/navigation wholesale, within the very same stdlib:false
        // project.
        const SRC: &str = "?Local doc.\nhelper() { right; }\nexport main() {\n    @helper();\n    @std::goToEnd();\n}\n";
        service.did_update(&app_uri, SRC);

        let helper_pos = pos_after(SRC, "@helper()", 1);
        let helper_hover = service
            .hover(&app_uri, helper_pos)
            .expect("a local, non-std call still hovers under stdlib:false");
        assert!(helper_hover.text.contains("Local doc."), "{helper_hover:?}");
        let helper_target = service
            .definition(&app_uri, helper_pos)
            .expect("a local, non-std call still navigates under stdlib:false");
        assert_eq!(helper_target.uri, app_uri);
        assert_eq!(helper_target.span, span_of(SRC, "helper"));

        // std half: both hover and the materialized jump are gone.
        let std_pos = pos_after(SRC, "std::goToEnd", 6);
        assert_eq!(
            service.hover(&app_uri, std_pos),
            None,
            "stdlib:false kills the std hover"
        );
        assert_eq!(
            service.definition(&app_uri, std_pos),
            None,
            "stdlib:false kills the materialized jump"
        );

        // The aliased `ImportBinding` shape (`use std::goToEnd as ge;`)
        // goes through a different `resolve_call` arm than the bare
        // qualified call above — gate it too, not just
        // `QualifiedExternal`.
        const ALIAS_SRC: &str = "use std::goToEnd as ge;\nexport main() {\n    @ge();\n}\n";
        service.did_update(&app_uri, ALIAS_SRC);
        let alias_pos = pos_after(ALIAS_SRC, "@ge()", 1);
        assert_eq!(
            service.definition(&app_uri, alias_pos),
            None,
            "stdlib:false kills the materialized jump for an aliased std import too"
        );

        // The `use std::goToEnd;` path segment itself (step 3, `use_path_at`)
        // is the third and last `std_path_target` call site — gate it too.
        const USE_SRC: &str = "use std::goToEnd;\nexport main() { right; }\n";
        service.did_update(&app_uri, USE_SRC);
        let use_pos = pos_at(USE_SRC, "goToEnd");
        assert_eq!(
            service.definition(&app_uri, use_pos),
            None,
            "stdlib:false kills the use-path jump onto std itself"
        );

        // Single-file doc (no manifest at all): both keep working — the
        // gate is manifest-driven, not global.
        let mut single = PmcLanguageService::new();
        const SINGLE_SRC: &str = "export main() {\n    @std::goToEnd();\n}\n";
        single.did_update(URI, SINGLE_SRC);
        let single_pos = pos_after(SINGLE_SRC, "std::goToEnd", 6);

        let single_hover = single
            .hover(URI, single_pos)
            .expect("single-file doc keeps the std hover");
        assert!(!single_hover.text.is_empty());

        let single_target = single
            .definition(URI, single_pos)
            .expect("single-file doc keeps the materialized jump");
        assert!(
            single_target.uri.starts_with("file://"),
            "uri: {}",
            single_target.uri
        );
        let entry = roster()
            .iter()
            .find(|e| e.full_path == "std::goToEnd")
            .expect("goToEnd is in the roster");
        assert_eq!(single_target.span, entry.name_span);
    }

    // --- `std::` names shadowed by a sibling's own `namespace std {}` ---

    #[test]
    fn std_import_binding_still_reaches_the_materialized_stdlib_when_unshadowed() {
        // Regression guard, in a REAL project (an overlay exists, unlike
        // `NAV_FIXTURE`'s no-manifest baseline above): a sibling exists
        // but defines something else entirely, so the overlay genuinely
        // MISSES `std::goToEnd` and the fall-through to the materialized
        // roster must still fire, exactly as it did before this fix.
        let dir = unique_tmp_dir("nav-std-unshadowed");
        fs::write(
            dir.join("pmt.json"),
            r#"{"project":{"targets":{"app":{"sources":["app.pmc","shared.pmc"]}}}}"#,
        )
        .unwrap();
        fs::write(dir.join("shared.pmc"), "export unrelated() { right; }\n").unwrap();

        let mut service = PmcLanguageService::new();
        let app_uri = file_uri(&dir.join("app.pmc"));
        const SRC: &str = "use std::goToEnd as ge;\nexport main() {\n    @ge();\n}\n";
        service.did_update(&app_uri, SRC);

        let pos = pos_after(SRC, "@ge()", 1);
        let target = service
            .definition(&app_uri, pos)
            .expect("std::goToEnd is not shadowed by any sibling in this project");

        let std_uri = materialized_std_uri().expect("materialization succeeds in this env");
        assert_eq!(target.uri, std_uri);
        let entry = roster()
            .iter()
            .find(|e| e.full_path == "std::goToEnd")
            .expect("goToEnd is in the roster");
        assert_eq!(target.span, entry.name_span);
    }

    #[test]
    fn std_qualified_call_jumps_into_the_shadowing_sibling_not_the_materialized_stdlib() {
        // THE defect this task fixes: a sibling's own `namespace std {
        // export goToEnd() {...} }` shadows the embedded routine of the
        // same name — the linker's own user-object-beats-library rule —
        // so go-to-definition on `@std::goToEnd()` must land in the
        // sibling, never the materialized stdlib URI.
        let dir = unique_tmp_dir("nav-std-shadow-qualified");
        fs::write(
            dir.join("pmt.json"),
            r#"{"project":{"targets":{"app":{"sources":["app.pmc","shared.pmc"]}}}}"#,
        )
        .unwrap();
        const SHARED: &str = "namespace std {\nexport goToEnd() { right; }\n}\n";
        fs::write(dir.join("shared.pmc"), SHARED).unwrap();

        let mut service = PmcLanguageService::new();
        let app_uri = file_uri(&dir.join("app.pmc"));
        const SRC: &str = "export main() {\n    @std::goToEnd();\n}\n";
        service.did_update(&app_uri, SRC);

        let pos = pos_after(SRC, "@std::goToEnd()", 1);
        let target = service
            .definition(&app_uri, pos)
            .expect("std::goToEnd resolves to the shadowing sibling");

        assert_eq!(target.uri, file_uri(&dir.join("shared.pmc")));
        assert_eq!(target.span, span_of(SHARED, "goToEnd"));
        let std_uri = materialized_std_uri().expect("materialization succeeds in this env");
        assert_ne!(
            target.uri, std_uri,
            "must NOT land in the embedded stdlib copy"
        );
    }

    #[test]
    fn std_import_binding_alias_jumps_into_the_shadowing_sibling() {
        // Same shadowing proof, through the `ImportBinding` arm instead
        // of `QualifiedExternal` (`use std::goToEnd as ge;` — the alias
        // this repo's own `stdlib_false_kills_std_hover_and_the_materialized_jump`
        // test also exercises separately for the OTHER (disabled) half
        // of this same gate).
        let dir = unique_tmp_dir("nav-std-shadow-import-binding");
        fs::write(
            dir.join("pmt.json"),
            r#"{"project":{"targets":{"app":{"sources":["app.pmc","shared.pmc"]}}}}"#,
        )
        .unwrap();
        const SHARED: &str = "namespace std {\nexport goToEnd() { right; }\n}\n";
        fs::write(dir.join("shared.pmc"), SHARED).unwrap();

        let mut service = PmcLanguageService::new();
        let app_uri = file_uri(&dir.join("app.pmc"));
        const SRC: &str = "use std::goToEnd as ge;\nexport main() {\n    @ge();\n}\n";
        service.did_update(&app_uri, SRC);

        let pos = pos_after(SRC, "@ge()", 1);
        let target = service
            .definition(&app_uri, pos)
            .expect("the aliased std import resolves to the shadowing sibling");

        assert_eq!(target.uri, file_uri(&dir.join("shared.pmc")));
        assert_eq!(target.span, span_of(SHARED, "goToEnd"));
    }

    #[test]
    fn std_use_path_itself_jumps_into_the_shadowing_sibling() {
        // Step 3's own `use std::…` path segment (the third
        // `std_path_target` call site, `use_path_at`'s caller in
        // `definition`) — same shadowing proof, one seam over.
        let dir = unique_tmp_dir("nav-std-shadow-use-path");
        fs::write(
            dir.join("pmt.json"),
            r#"{"project":{"targets":{"app":{"sources":["app.pmc","shared.pmc"]}}}}"#,
        )
        .unwrap();
        const SHARED: &str = "namespace std {\nexport goToEnd() { right; }\n}\n";
        fs::write(dir.join("shared.pmc"), SHARED).unwrap();

        let mut service = PmcLanguageService::new();
        let app_uri = file_uri(&dir.join("app.pmc"));
        const SRC: &str = "use std::goToEnd;\nexport main() { right; }\n";
        service.did_update(&app_uri, SRC);

        let pos = pos_at(SRC, "goToEnd");
        let target = service
            .definition(&app_uri, pos)
            .expect("the use-path segment itself resolves to the shadowing sibling");

        assert_eq!(target.uri, file_uri(&dir.join("shared.pmc")));
        assert_eq!(target.span, span_of(SHARED, "goToEnd"));
    }

    #[test]
    fn stdlib_false_still_jumps_into_a_shadowing_siblings_std_export() {
        // The `"stdlib": false` gate silences only the EMBEDDED roster
        // (`std_path_target`'s own `std_enabled` check); a sibling's own
        // `namespace std { export … }` is ordinary linked code, owned by
        // the overlay outright, and must still navigate — completing the
        // picture `stdlib_false_kills_std_hover_and_the_materialized_jump`
        // draws for the (correctly) suppressed unshadowed case.
        let dir = unique_tmp_dir("nav-std-shadow-stdlib-false");
        fs::write(
            dir.join("pmt.json"),
            r#"{"project":{"stdlib":false,"targets":{"app":{"sources":["app.pmc","shared.pmc"]}}}}"#,
        )
        .unwrap();
        const SHARED: &str = "namespace std {\nexport goToEnd() { right; }\n}\n";
        fs::write(dir.join("shared.pmc"), SHARED).unwrap();

        let mut service = PmcLanguageService::new();
        let app_uri = file_uri(&dir.join("app.pmc"));
        const SRC: &str = "export main() {\n    @std::goToEnd();\n}\n";
        service.did_update(&app_uri, SRC);

        let pos = pos_after(SRC, "@std::goToEnd()", 1);
        let target = service
            .definition(&app_uri, pos)
            .expect("the shadowing sibling still resolves under stdlib:false");

        assert_eq!(target.uri, file_uri(&dir.join("shared.pmc")));
        assert_eq!(target.span, span_of(SHARED, "goToEnd"));
    }
}
