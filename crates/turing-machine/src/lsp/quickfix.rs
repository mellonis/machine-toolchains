//! Quickfixes derived from a fatal (docs/lsp.md (code actions)).
//!
//! The lint layer's own findings already carry `Fix`es, and those convert
//! mechanically. The two fixes here are different in kind: they are built
//! from the compiler's FATAL, which carries no fix of its own because the
//! batch pipeline has nowhere to apply one. Both reconstruct the missing
//! source from what the analysis already knows — the world's tape arity for
//! a state stub, the two alphabets for a binding map — so neither invents a
//! shape the language would then reject.

use std::rc::Rc;

use mtc_core::diagnostics::{Edit, Pos, Span};
use mtc_core::lsp::Action;
use mtc_core::syntax::{AstNode, SyntaxNode, TextLineIndex};

use super::{DocState, spans_overlap};
use crate::compiler::{CompileErrorKind, Resolved, full_name};
use crate::parser::{BindingArg, BindingValue, Program, SigParamKind, Signature};
use crate::syntax::{MachineView, ReuseView, RootView, TopView};

/// Quickfixes for the document's fatal, when it overlaps `span`.
pub(super) fn fatal_actions(state: &DocState, span: Span) -> Vec<Action> {
    let Some(fatal) = &state.fatal else {
        return Vec::new();
    };
    if !spans_overlap(fatal.span, span) {
        return Vec::new();
    }
    match &fatal.kind {
        CompileErrorKind::UndefinedState(name) => {
            state_stub(state, name, fatal.span).into_iter().collect()
        }
        CompileErrorKind::IdentityGlyphMismatch => state
            .program
            .as_ref()
            .zip(state.resolved.as_ref())
            .and_then(|(program, resolved)| map_pairs(program, resolved, fatal.span))
            .into_iter()
            .collect(),
        _ => Vec::new(),
    }
}

/// A world body as the stub inserter needs it: where its closing brace is,
/// and how wide a rule's vectors must be there.
struct BodyExtent {
    /// The closing `}` of the world block.
    close: Pos,
    /// The world's tape count — a stub rule's pattern arity.
    arity: usize,
}

/// "Declare the missing state": a stub state appended to the world whose
/// body the unresolved `goto` sits in, with a catch-all rule of the right
/// arity that stops. The arity matters — a stub with the wrong vector
/// width would trade one error for another.
///
/// Green-tier: indexes `state.green` (docs/core.md (syntax trees))
/// directly — the same tree `document_symbols`
/// (`crate::lsp::document_symbols`) reads, an `Rc` clone of the one
/// `analyze_staged` built rather than a reparse. Availability is
/// `state.green.is_some()`, the parse-tier gate `analyze_staged` already
/// applied.
fn state_stub(state: &DocState, name: &str, at: Span) -> Option<Action> {
    let green = state.green.as_ref()?;
    let root = RootView::cast(SyntaxNode::new_root(Rc::clone(green)))?;
    let index = &state.line_index;
    let extent = enclosing_body(&root, state.program.as_ref(), at, index)?;
    let cells = vec!["*"; extent.arity.max(1)].join(", ");
    // Insert on its own line just before the closing brace, one level in
    // from it. The block's own depth is read off that brace rather than
    // assumed: a world nested in a namespace sits deeper than a top-level
    // `machine`, and a stub indented for the wrong depth would leave the
    // file the fix just produced failing `tmt fmt --check`. The brace's
    // span end is exclusive, so its column IS the one-level-in width.
    let indent = " ".repeat((extent.close.col as usize).max(2));
    let insert = Pos {
        line: extent.close.line,
        col: 1,
    };
    Some(Action {
        title: format!("declare state `{name}`"),
        preferred: true,
        edits: vec![Edit {
            span: Span {
                start: insert,
                end: insert,
            },
            replacement: format!("{indent}state {name} {{ [{cells}] -> stop; }}\n"),
        }],
    })
}

/// The innermost world block containing `at`, with its tape count.
///
/// A mechanical port off the green tree: MACHINE and REUSE never nest
/// inside one another (a world's own direct children are TAPE/STATE/
/// GRAFT/BIND, never another MACHINE or REUSE — `crates/turing-machine/
/// src/syntax/mod.rs`'s module doc), so the only nesting a real program
/// can put between the root and a candidate is NAMESPACE, and recursing
/// into every namespace's own `items()` is what makes the walk find the
/// one that actually contains `at`.
///
/// Reads only each candidate's range END (`span.end`, below) — never its
/// start, so a bound doc run retro-wrapped at the front of a MACHINE or
/// REUSE node (`crates/turing-machine/src/syntax/mod.rs`'s module doc)
/// never moves where the stub lands; the containment check reads the
/// full node range (doc run included), which only makes it MORE
/// inclusive of `at`, never less — an unreachable divergence in
/// practice, since `at` is always a reference inside the world's own
/// body, never inside the header the doc run sits in front of.
fn enclosing_body(
    root: &RootView,
    program: Option<&Program>,
    at: Span,
    index: &TextLineIndex,
) -> Option<BodyExtent> {
    fn walk(
        items: impl Iterator<Item = TopView>,
        at: Span,
        index: &TextLineIndex,
        out: &mut Option<(Span, Option<String>)>,
    ) {
        for item in items {
            match item {
                TopView::Namespace(ns) => walk(ns.items(), at, index, out),
                TopView::Machine(m) => consider_machine(&m, at, index, out),
                TopView::Reuse(r) => consider_reuse(&r, at, index, out),
                _ => {}
            }
        }
    }
    fn consider_machine(
        m: &MachineView,
        at: Span,
        index: &TextLineIndex,
        out: &mut Option<(Span, Option<String>)>,
    ) {
        let span = index.span(m.syntax().text_range());
        if contains(span, at) {
            *out = Some((span, None));
        }
    }
    fn consider_reuse(
        r: &ReuseView,
        at: Span,
        index: &TextLineIndex,
        out: &mut Option<(Span, Option<String>)>,
    ) {
        let span = index.span(r.syntax().text_range());
        if contains(span, at) {
            *out = Some((span, Some(r.name_token().text().to_string())));
        }
    }
    let mut found: Option<(Span, Option<String>)> = None;
    walk(root.items(), at, index, &mut found);
    let (span, reuse_name) = found?;
    let arity = match (&reuse_name, program) {
        (None, Some(program)) => program.machine.as_ref().map_or(1, |m| m.tapes.len()),
        (Some(name), Some(program)) => program
            .routines
            .iter()
            .map(|r| (&r.name, &r.sig))
            .chain(program.graphs.iter().map(|g| (&g.name, &g.sig)))
            .find(|(n, _)| *n == name)
            .map_or(1, |(_, sig)| tape_arity(sig)),
        _ => 1,
    };
    Some(BodyExtent {
        close: span.end,
        arity,
    })
}

fn tape_arity(sig: &Signature) -> usize {
    sig.params
        .iter()
        .filter(|p| matches!(p.kind, SigParamKind::Tape { .. }))
        .count()
}

fn contains(outer: Span, inner: Span) -> bool {
    outer.start <= inner.start && inner.end <= outer.end
}

/// "Supply the missing map pairs": the omitted binding map is only legal
/// when the two tapes carry glyph-for-glyph equal alphabets, so the fix
/// writes the map the arguments actually need. Pairs are emitted
/// position-by-position, skipping index 0 (blank always reads as blank)
/// and any position the two alphabets already agree on — exactly the pairs
/// identity cannot supply.
fn map_pairs(program: &Program, resolved: &Resolved, at: Span) -> Option<Action> {
    let (arg, world_name, target) = binding_argument_at(program, at)?;
    let BindingValue::Named {
        target: tape,
        target_span,
        map: None,
    } = &arg.value
    else {
        return None;
    };
    let host = resolved
        .worlds
        .iter()
        .find(|w| w.name == world_name)?
        .tapes
        .iter()
        .find(|t| t.name == *tape)?;
    let callee = resolved
        .worlds
        .iter()
        .find(|w| w.name == target)?
        .tapes
        .iter()
        .find(|t| t.name == arg.name)?;
    let host_glyphs = &resolved.alphabets.get(&host.alphabet)?.glyphs;
    let callee_glyphs = &resolved.alphabets.get(&callee.alphabet)?.glyphs;

    let pairs: Vec<String> = host_glyphs
        .iter()
        .enumerate()
        .skip(1)
        .filter_map(|(i, h)| {
            let c = callee_glyphs.get(i)?;
            (h != c).then(|| format!("{} -> {}", quoted(h), quoted(c)))
        })
        .collect();
    if pairs.is_empty() {
        return None;
    }
    let end = target_span.end;
    Some(Action {
        title: "add the missing `with map` pairs".to_string(),
        // The pairing is positional, which is a guess about intent even
        // though every pair it writes is individually legal.
        preferred: false,
        edits: vec![Edit {
            span: Span { start: end, end },
            replacement: format!(" with map {{ {} }}", pairs.join(", ")),
        }],
    })
}

fn quoted(label: &str) -> String {
    format!("'{}'", label.replace('\\', "\\\\").replace('\'', "\\'"))
}

/// The binding argument whose span covers `at`, with the mangled names of
/// the world it is written in and the world it targets.
fn binding_argument_at(program: &Program, at: Span) -> Option<(&BindingArg, String, String)> {
    let mut sites: Vec<(&[BindingArg], String, String, &[String])> = Vec::new();
    if let Some(m) = &program.machine {
        for graft in &m.grafts {
            sites.push((&graft.args, "main".to_string(), graft.target.joined(), &[]));
        }
        for bind in &m.binds {
            sites.push((&bind.args, "main".to_string(), bind.target.joined(), &[]));
        }
    }
    for (name, ns, grafts, binds) in program
        .routines
        .iter()
        .map(|r| (&r.name, &r.ns, &r.grafts, &r.binds))
        .chain(
            program
                .graphs
                .iter()
                .map(|g| (&g.name, &g.ns, &g.grafts, &g.binds)),
        )
    {
        let mangled = full_name(ns, name);
        for graft in grafts {
            sites.push((&graft.args, mangled.clone(), graft.target.joined(), ns));
        }
        for bind in binds {
            sites.push((&bind.args, mangled.clone(), bind.target.joined(), ns));
        }
    }
    for (args, world, target, ns) in sites {
        for arg in args {
            if contains(arg.span, at) || contains(at, arg.span) {
                let resolved_target = resolve_target(program, &target, ns);
                return Some((arg, world, resolved_target));
            }
        }
    }
    None
}

/// The mangled name a graft/bind target spells, resolved the way the
/// compiler resolves it: exact, then `use`-bound, then same-namespace.
fn resolve_target(program: &Program, written: &str, scope: &[String]) -> String {
    let known = |name: &str| {
        program
            .routines
            .iter()
            .any(|r| full_name(&r.ns, &r.name) == name)
            || program
                .graphs
                .iter()
                .any(|g| full_name(&g.ns, &g.name) == name)
    };
    if known(written) {
        return written.to_string();
    }
    for import in &program.imports {
        if import.binding() == written {
            let full = import.full_path();
            if known(&full) {
                return full;
            }
        }
    }
    for depth in (0..scope.len()).rev() {
        let qualified = format!("{}::{written}", scope[..=depth].join("::"));
        if known(&qualified) {
            return qualified;
        }
    }
    written.to_string()
}
