//! Go-to-definition and hover (docs/lsp.md (navigation)).
//!
//! # Two sides, two sources
//!
//! A navigation request has a REFERENCE side (what does the cursor sit on?)
//! and a TARGET side (where is that declared, and what should it say?).
//!
//! The reference side is answered against the flat program, because that is
//! where every reference span lives: a qualified call target's own span, a
//! `goto`'s span, a tape declaration's alphabet span, a signature
//! parameter's alphabet span, a binding argument's parameter and value
//! spans. The program also survives a resolve-stage fatal, so navigation
//! keeps working on a document whose semantics do not yet check out.
//!
//! The target side is answered against the resolved module, whose per-world
//! `calls` / `grafts` / `binds` vectors already carry the resolution the
//! reference needs — bind targets resolved to mangled names, graft
//! arguments resolved to their bound values. That shape is per-world rather
//! than one flat span→resolution list, so the walk here carries the
//! enclosing world's mangled name with it and indexes into the world's own
//! vectors, instead of looking a span up in a global table.
//!
//! Both requests funnel through one [`reference_at`] walk that names WHAT
//! the cursor is on; `definition` then asks where that is declared and
//! `hover` asks what it says, so the two can never disagree about what the
//! cursor meant.

use mtc_core::diagnostics::{Pos, Span};
use mtc_core::lsp::{DefTarget, HoverContent};

use super::{DocState, render_doc, span_touches, std_enabled};
use crate::compiler::{Resolved, WorldKind, full_name};
use crate::parser::{
    Alphabet, Bind, BindingArg, BindingValue, Continuation, Doc, Graft, Program, SigParamKind,
    Signature, State, Transition,
};

/// What the cursor is on, in resolved terms.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Target {
    /// A mangled alphabet name.
    Alphabet(String),
    /// A mangled routine/graph name.
    World(String),
    /// A state of a world (both mangled/world-local names).
    State { world: String, name: String },
    /// A bind instance of a world.
    Bind { world: String, name: String },
    /// A tape (machine declaration or signature parameter) of a world.
    Tape { world: String, name: String },
    /// A graft instance; navigation goes to the GRAPH it splices, which is
    /// where the states it contributes are actually written.
    Graft { world: String, instance: String },
    /// A signature parameter of another world, named on a binding
    /// argument's left-hand side.
    Param { world: String, name: String },
    /// A `call`/`bind` target this document does not declare: a
    /// `::`-qualified name, or the full path a `use` binding names. `path`
    /// is exactly what a cross-unit reference must be — never re-resolved
    /// against this document's own tables, since the whole point is a name
    /// they do not contain. Resolved against the cross-file overlay first,
    /// the standard library's materialized on-disk copy last
    /// (`external_declaration`, docs/lsp.md (materialized standard
    /// library)); a `graft` never produces this target — grafting splices
    /// a graph's source, which a link boundary does not carry.
    External { path: String },
}

/// A world seen uniformly, whatever its carrier: the machine block's tape
/// declarations and a routine/graph signature's tape parameters are the
/// same thing to every walk here.
struct WorldView<'a> {
    mangled: String,
    ns: &'a [String],
    kind: WorldKind,
    name_span: Span,
    /// `(name, name span, alphabet as written, alphabet span)`.
    tapes: Vec<(&'a str, Span, &'a str, Span)>,
    sig: Option<&'a Signature>,
    states: &'a [State],
    grafts: &'a [Graft],
    binds: &'a [Bind],
}

const NO_NS: &[String] = &[];

fn world_views(program: &Program) -> Vec<WorldView<'_>> {
    let mut out: Vec<WorldView<'_>> = Vec::new();
    if let Some(m) = &program.machine {
        out.push(WorldView {
            mangled: "main".to_string(),
            ns: NO_NS,
            kind: WorldKind::Machine,
            name_span: Span::point(m.line, m.col),
            tapes: m
                .tapes
                .iter()
                .map(|t| {
                    (
                        t.name.as_str(),
                        t.name_span,
                        t.alphabet.as_str(),
                        t.alphabet_span,
                    )
                })
                .collect(),
            sig: None,
            states: &m.states,
            grafts: &m.grafts,
            binds: &m.binds,
        });
    }
    for r in &program.routines {
        out.push(WorldView {
            mangled: full_name(&r.ns, &r.name),
            ns: &r.ns,
            kind: WorldKind::Routine,
            name_span: r.name_span,
            tapes: sig_tapes(&r.sig),
            sig: Some(&r.sig),
            states: &r.states,
            grafts: &r.grafts,
            binds: &r.binds,
        });
    }
    for g in &program.graphs {
        out.push(WorldView {
            mangled: full_name(&g.ns, &g.name),
            ns: &g.ns,
            kind: WorldKind::Graph,
            name_span: g.name_span,
            tapes: sig_tapes(&g.sig),
            sig: Some(&g.sig),
            states: &g.states,
            grafts: &g.grafts,
            binds: &g.binds,
        });
    }
    out
}

fn sig_tapes(sig: &Signature) -> Vec<(&str, Span, &str, Span)> {
    sig.params
        .iter()
        .filter_map(|p| match &p.kind {
            SigParamKind::Tape {
                alphabet,
                alphabet_span,
                ..
            } => Some((
                p.name.as_str(),
                p.name_span,
                alphabet.as_str(),
                *alphabet_span,
            )),
            SigParamKind::State => None,
        })
        .collect()
}

/// Resolves a top-level name AS WRITTEN to its mangled form: an exact hit,
/// then a `use`-bound spelling, then a same-namespace sibling. `known`
/// decides which table the name must land in, so an alphabet reference can
/// never resolve to a routine of the same name.
fn resolve_written(
    program: &Program,
    written: &str,
    scope: &[String],
    known: impl Fn(&str) -> bool,
) -> Option<String> {
    if known(written) {
        return Some(written.to_string());
    }
    for import in &program.imports {
        if import.binding() == written {
            let full = import.full_path();
            if known(&full) {
                return Some(full);
            }
        }
    }
    for depth in (0..scope.len()).rev() {
        let qualified = format!("{}::{written}", scope[..=depth].join("::"));
        if known(&qualified) {
            return Some(qualified);
        }
    }
    None
}

/// Resolves a `call`/`bind` target AS WRITTEN to a full path OUTSIDE this
/// document, for when [`resolve_written`] has already found nothing local:
/// a `::`-qualified name is used as-is, else a bare name is looked up
/// against the document's own `use` bindings (alias → full path). Unlike
/// `resolve_written`, the result is never checked against a local table —
/// that is exactly what the caller already ruled out.
fn external_path(program: &Program, written: &str) -> Option<String> {
    if written.contains("::") {
        return Some(written.to_string());
    }
    program
        .imports
        .iter()
        .find(|import| import.binding() == written)
        .map(|import| import.full_path())
}

fn alphabet_exists(program: &Program) -> impl Fn(&str) -> bool + '_ {
    move |name: &str| {
        program
            .alphabets
            .iter()
            .any(|a| full_name(&a.ns, &a.name) == name)
    }
}

fn world_exists(program: &Program) -> impl Fn(&str) -> bool + '_ {
    move |name: &str| {
        program
            .routines
            .iter()
            .any(|r| full_name(&r.ns, &r.name) == name)
            || program
                .graphs
                .iter()
                .any(|g| full_name(&g.ns, &g.name) == name)
    }
}

/// What `pos` names, and the exact span of the reference it names it by.
fn reference_at(program: &Program, pos: Pos) -> Option<(Target, Span)> {
    // A `use` path: the imported declaration itself.
    for import in &program.imports {
        if span_touches(import.span, pos) {
            let full = import.full_path();
            if alphabet_exists(program)(&full) {
                return Some((Target::Alphabet(full), import.span));
            }
            if world_exists(program)(&full) {
                return Some((Target::World(full), import.span));
            }
            return None;
        }
    }
    // An alphabet's own declaration name.
    for a in &program.alphabets {
        if span_touches(a.name_span, pos) {
            return Some((Target::Alphabet(full_name(&a.ns, &a.name)), a.name_span));
        }
    }
    for world in world_views(program) {
        if let Some(hit) = reference_in_world(program, &world, pos) {
            return Some(hit);
        }
    }
    None
}

fn reference_in_world(
    program: &Program,
    world: &WorldView<'_>,
    pos: Pos,
) -> Option<(Target, Span)> {
    // The world's own declaration name.
    if world.kind != WorldKind::Machine && span_touches(world.name_span, pos) {
        return Some((Target::World(world.mangled.clone()), world.name_span));
    }
    // Tape declarations and signature tape parameters: the name declares a
    // tape, the alphabet references one.
    for (name, name_span, alphabet, alphabet_span) in &world.tapes {
        if span_touches(*name_span, pos) {
            return Some((
                Target::Tape {
                    world: world.mangled.clone(),
                    name: (*name).to_string(),
                },
                *name_span,
            ));
        }
        if span_touches(*alphabet_span, pos) {
            let mangled = resolve_written(program, alphabet, world.ns, alphabet_exists(program))?;
            return Some((Target::Alphabet(mangled), *alphabet_span));
        }
    }
    for graft in world.grafts {
        if span_touches(graft.target.span, pos) {
            let mangled = resolve_written(
                program,
                &graft.target.joined(),
                world.ns,
                world_exists(program),
            )?;
            return Some((Target::World(mangled), graft.target.span));
        }
        if let Some(as_name) = &graft.as_name
            && span_touches(as_name.span, pos)
        {
            return Some((
                Target::Graft {
                    world: world.mangled.clone(),
                    instance: as_name.name.clone(),
                },
                as_name.span,
            ));
        }
        if let Some(hit) = binding_args_reference(program, world, &graft.args, pos) {
            return Some(hit);
        }
    }
    for bind in world.binds {
        if span_touches(bind.target.span, pos) {
            let written = bind.target.joined();
            let hit = match resolve_written(program, &written, world.ns, world_exists(program)) {
                Some(mangled) => Target::World(mangled),
                None => Target::External {
                    path: external_path(program, &written)?,
                },
            };
            return Some((hit, bind.target.span));
        }
        if span_touches(bind.as_name.span, pos) {
            return Some((
                Target::Bind {
                    world: world.mangled.clone(),
                    name: bind.as_name.name.clone(),
                },
                bind.as_name.span,
            ));
        }
        if let Some(hit) = binding_args_reference(program, world, &bind.args, pos) {
            return Some(hit);
        }
    }
    for state in world.states {
        if span_touches(state.name_span, pos) {
            return Some((
                Target::State {
                    world: world.mangled.clone(),
                    name: state.name.clone(),
                },
                state.name_span,
            ));
        }
        for rule in &state.rules {
            match &rule.transition {
                Transition::Goto { name, span, .. } => {
                    let at = name_span(*span, name);
                    if span_touches(at, pos) {
                        return Some((world_local(world, name), at));
                    }
                }
                Transition::Call {
                    target, args, then, ..
                } => {
                    if span_touches(target.span, pos) {
                        let written = target.joined();
                        // A call on a bind instance names the bind, not a
                        // routine — the bind carries the binding.
                        if world.binds.iter().any(|b| b.as_name.name == written) {
                            return Some((
                                Target::Bind {
                                    world: world.mangled.clone(),
                                    name: written,
                                },
                                target.span,
                            ));
                        }
                        let hit = match resolve_written(
                            program,
                            &written,
                            world.ns,
                            world_exists(program),
                        ) {
                            Some(mangled) => Target::World(mangled),
                            None => Target::External {
                                path: external_path(program, &written)?,
                            },
                        };
                        return Some((hit, target.span));
                    }
                    if let Continuation::State { name, span } = then {
                        let at = name_span(*span, name);
                        if span_touches(at, pos) {
                            return Some((world_local(world, name), at));
                        }
                    }
                    if let Some(hit) = binding_args_reference(program, world, args, pos) {
                        return Some(hit);
                    }
                }
                _ => {}
            }
        }
    }
    None
}

/// The span of the NAME inside a transition slot's span. A `goto NAME`
/// carries the keyword in its span, which would underline `goto done`
/// rather than `done`; anchoring on the end and stepping back the name's
/// own length narrows it without assuming how much whitespace the author
/// put between the two. Slots whose span is already just the name come
/// through unchanged.
fn name_span(span: Span, name: &str) -> Span {
    let len = name.chars().count() as u32;
    if span.end.line != span.start.line || span.end.col < span.start.col + len {
        return span;
    }
    Span {
        start: Pos {
            line: span.end.line,
            col: span.end.col - len,
        },
        end: span.end,
    }
}

/// A world-local name in a transition slot: a graft instance if one is
/// declared under that name, else a state (its own or a parameter's).
fn world_local(world: &WorldView<'_>, name: &str) -> Target {
    if world
        .grafts
        .iter()
        .any(|g| g.as_name.as_ref().is_some_and(|a| a.name == name))
    {
        return Target::Graft {
            world: world.mangled.clone(),
            instance: name.to_string(),
        };
    }
    Target::State {
        world: world.mangled.clone(),
        name: name.to_string(),
    }
}

/// A binding argument list: the left-hand side names a parameter of the
/// TARGET world, the right-hand side a tape or state of THIS world.
fn binding_args_reference(
    program: &Program,
    world: &WorldView<'_>,
    args: &[BindingArg],
    pos: Pos,
) -> Option<(Target, Span)> {
    let _ = program;
    for arg in args {
        if span_touches(arg.name_span, pos) {
            return Some((
                Target::Param {
                    world: world.mangled.clone(),
                    name: arg.name.clone(),
                },
                arg.name_span,
            ));
        }
        if let BindingValue::Named {
            target,
            target_span,
            ..
        } = &arg.value
            && span_touches(*target_span, pos)
        {
            if world.tapes.iter().any(|(name, ..)| name == target) {
                return Some((
                    Target::Tape {
                        world: world.mangled.clone(),
                        name: target.clone(),
                    },
                    *target_span,
                ));
            }
            return Some((world_local(world, target), *target_span));
        }
    }
    None
}

/// Where a target is declared, in this document.
fn declaration_span(state: &DocState, program: &Program, target: &Target) -> Option<Span> {
    match target {
        Target::Alphabet(mangled) => program
            .alphabets
            .iter()
            .find(|a| full_name(&a.ns, &a.name) == *mangled)
            .map(|a| a.name_span),
        Target::World(mangled) => world_views(program)
            .into_iter()
            .find(|w| w.mangled == *mangled)
            .map(|w| w.name_span),
        Target::State { world, name } => world_views(program)
            .into_iter()
            .find(|w| w.mangled == *world)
            .and_then(|w| {
                w.states
                    .iter()
                    .find(|s| s.name == *name)
                    .map(|s| s.name_span)
                    .or_else(|| {
                        // A state PARAMETER is a declaration too: it is
                        // where a routine/graph names its exit.
                        w.sig?
                            .params
                            .iter()
                            .find(|p| p.kind == SigParamKind::State && p.name == *name)
                            .map(|p| p.name_span)
                    })
            }),
        Target::Bind { world, name } => world_views(program)
            .into_iter()
            .find(|w| w.mangled == *world)
            .and_then(|w| {
                w.binds
                    .iter()
                    .find(|b| b.as_name.name == *name)
                    .map(|b| b.as_name.span)
            }),
        Target::Tape { world, name } => world_views(program)
            .into_iter()
            .find(|w| w.mangled == *world)
            .and_then(|w| {
                w.tapes
                    .iter()
                    .find(|(tape, ..)| tape == name)
                    .map(|(_, span, ..)| *span)
            }),
        // A graft instance's states are written in the GRAPH it splices,
        // so that graph's declaration is the useful destination.
        Target::Graft { world, instance } => {
            let graft = graft_of(program, world, instance)?;
            let views = world_views(program);
            let scope = views.iter().find(|w| w.mangled == *world)?.ns;
            let mangled = resolve_written(
                program,
                &graft.target.joined(),
                scope,
                world_exists(program),
            )?;
            views
                .iter()
                .find(|w| w.mangled == mangled)
                .map(|w| w.name_span)
        }
        Target::Param { world, name } => {
            // The parameter belongs to whatever world the argument list
            // targets; the enclosing world is where the reference was
            // written, so look the parameter up on the target found by
            // name across every world that declares one.
            let _ = world;
            world_views(program).into_iter().find_map(|w| {
                w.sig?
                    .params
                    .iter()
                    .find(|p| p.name == *name)
                    .map(|p| p.name_span)
            })
        }
        // Declared outside this document; `definition` special-cases the
        // URI, but still drives the SPAN through here, off the overlay or
        // the stdlib roster.
        Target::External { path } => external_declaration(state, path).map(|(_, span)| span),
    }
}

/// Where an `External` target is declared, as `(uri, span)`: the
/// cross-file overlay (docs/lsp.md (project overlay)) is consulted FIRST —
/// a sibling that OWNS `path` answers here even when it carries no
/// source location of its own (a `.tma`/`.tmo` sibling, or a linked
/// library), since ownership is exactly what makes the embedded stdlib's
/// own, possibly same-named, entry the WRONG place to jump to. This
/// mirrors the linker's own user-sources-beat-libraries precedence
/// (docs/tmt/project.md (schema reference)): a project that writes its
/// own `namespace std { export routine goToNumber … }` shadows the
/// embedded routine of the same mangled name, and the overlay already
/// resolves that the same way the linker does. Only a genuine overlay
/// MISS falls through to the standard library's materialized on-disk
/// copy, matched against its roster by full path (docs/lsp.md
/// (materialized standard library)) and itself gated on [`std_enabled`].
fn external_declaration(state: &DocState, path: &str) -> Option<(String, Span)> {
    if let Some(overlay) = state.overlay.as_ref()
        && let Some(sym) = overlay.symbols.get(path)
    {
        return sym.target.clone();
    }
    if !std_enabled(state) {
        return None;
    }
    let entry = crate::stdlib::roster()
        .iter()
        .find(|e| e.full_path == *path)?;
    let uri = crate::stdlib::materialized_std_uri()?;
    Some((uri.to_string(), entry.name_span))
}

/// An `External` target's own documentation, resolved by the SAME
/// overlay-first, stdlib-last precedence [`external_declaration`] uses
/// for its location: the overlay's contributing sibling's own `Doc` when
/// the overlay owns `path` at all — a `.tma`/`.tmo`-backed sibling owns
/// the path but carries no `Doc`, and that still short-circuits here,
/// since falling through would render an unrelated stdlib entry's doc
/// for a name the overlay itself defines. Only a genuine overlay miss
/// falls through to the standard library's own doc map, gated on
/// [`std_enabled`].
fn external_doc<'a>(state: &'a DocState, path: &str) -> Option<&'a Doc> {
    if let Some(overlay) = state.overlay.as_ref()
        && let Some(sym) = overlay.symbols.get(path)
    {
        return sym.doc.as_ref();
    }
    if !std_enabled(state) {
        return None;
    }
    crate::stdlib::docs().get(path)
}

fn graft_of<'a>(program: &'a Program, world: &str, instance: &str) -> Option<&'a Graft> {
    for view in world_views(program) {
        if view.mangled != world {
            continue;
        }
        return view
            .grafts
            .iter()
            .find(|g| g.as_name.as_ref().is_some_and(|a| a.name == instance));
    }
    None
}

pub(super) fn definition(state: &DocState, uri: &str, pos: Pos) -> Option<DefTarget> {
    let program = state.program.as_ref()?;
    let (target, origin) = reference_at(program, pos)?;
    // Every target but `External` is declared IN this document; `External`
    // is declared wherever `external_declaration` found it — a sibling
    // through the overlay, or the stdlib's materialized copy — so it takes
    // a different URI than `uri`.
    let (doc_uri, span) = match &target {
        Target::External { path } => external_declaration(state, path)?,
        _ => (uri.to_string(), declaration_span(state, program, &target)?),
    };
    Some(DefTarget {
        uri: doc_uri,
        span,
        origin: Some(origin),
    })
}

pub(super) fn hover(state: &DocState, pos: Pos) -> Option<HoverContent> {
    let program = state.program.as_ref()?;
    let (target, origin) = reference_at(program, pos)?;
    let text = render(program, state, &target)?;
    Some(HoverContent { text, span: origin })
}

/// The hover body for a target: a signature line first, then the
/// declaration's doc and deprecation callouts under it.
fn render(program: &Program, state: &DocState, target: &Target) -> Option<String> {
    let resolved = state.resolved.as_ref();
    let (head, doc_key) = match target {
        Target::Alphabet(mangled) => {
            let alphabet = program
                .alphabets
                .iter()
                .find(|a| full_name(&a.ns, &a.name) == *mangled)?;
            (alphabet_head(mangled, alphabet, resolved), Some(mangled))
        }
        Target::World(mangled) => {
            let view = world_views(program)
                .into_iter()
                .find(|w| w.mangled == *mangled)?;
            (world_head(&view), Some(mangled))
        }
        Target::State { world, name } => (format!("state {name} (in {world})"), None),
        Target::Tape { world, name } => {
            let view = world_views(program)
                .into_iter()
                .find(|w| w.mangled == *world)?;
            let (_, _, alphabet, _) = view.tapes.iter().find(|(tape, ..)| tape == name)?;
            (format!("tape {name}: {alphabet}"), None)
        }
        Target::Bind { world, name } => (bind_head(resolved, world, name)?, None),
        Target::Graft { world, instance } => {
            let graft = graft_of(program, world, instance)?;
            (
                format!("graft {} as {instance}", graft.target.joined()),
                None,
            )
        }
        Target::Param { name, .. } => (format!("binding argument {name}"), None),
        // The qualified path IS the head — a requesting document's own
        // analysis never holds a std entry, so there is no local signature
        // to render one from (unlike `World`, above).
        Target::External { path } => (path.clone(), Some(path)),
    };
    let doc = doc_key
        .and_then(|key| match target {
            // `external_doc` carries its OWN overlay-first/stdlib-last
            // precedence, matching `external_declaration`'s — a plain
            // `resolved.docs` lookup would never hit for an External path
            // anyway (that map is keyed by this document's OWN mangled
            // names), and unconditionally falling through to
            // `crate::stdlib::docs()` here would skip both the overlay leg
            // and the `std_enabled` gate.
            Target::External { .. } => external_doc(state, key),
            _ => resolved
                .and_then(|r| r.docs.get(key))
                .or_else(|| crate::stdlib::docs().get(key)),
        })
        .and_then(render_doc);
    // Every other head carries real information the source text alone does
    // not (a signature, a resolved binding, a world/alphabet name) — worth
    // showing even undocumented. `External`'s head is just the reference's
    // own qualified text: with no doc body it would be a hover that only
    // echoes what the cursor is already sitting on, which the emptiness
    // rule forbids (docs/lsp.md (hover)).
    if doc.is_none() && matches!(target, Target::External { .. }) {
        return None;
    }
    Some(match doc {
        Some(body) => format!("{head}\n\n{body}"),
        None => head,
    })
}

fn alphabet_head(mangled: &str, alphabet: &Alphabet, resolved: Option<&Resolved>) -> String {
    match resolved.and_then(|r| r.alphabets.get(mangled)) {
        Some(a) => format!(
            "alphabet {mangled} ({} symbols: {})",
            a.glyphs.len(),
            a.glyphs.join(", ")
        ),
        // Unresolved: the source element count is still worth showing.
        None => format!("alphabet {mangled} ({} elements)", alphabet.elems.len()),
    }
}

/// A world's signature as written, tape parameters with their alphabets
/// included — the pmc hover's "signature" line, in TM terms.
fn world_head(view: &WorldView<'_>) -> String {
    let carrier = match view.kind {
        WorldKind::Routine => "routine",
        WorldKind::Graph => "graph",
        WorldKind::Machine => "machine",
    };
    let Some(sig) = view.sig else {
        return format!("{carrier} {}", view.mangled);
    };
    let params: Vec<String> = sig
        .params
        .iter()
        .map(|p| match &p.kind {
            SigParamKind::Tape {
                alphabet, volatile, ..
            } => {
                let prefix = if *volatile { "volatile " } else { "" };
                format!("{prefix}tape {}: {alphabet}", p.name)
            }
            SigParamKind::State => format!("state {}", p.name),
        })
        .collect();
    format!("{carrier} {}({})", view.mangled, params.join(", "))
}

/// A bind instance's RESOLVED binding: the mangled routine it targets and
/// each argument's bound value, which is what the resolved module knows
/// and the source text alone does not.
fn bind_head(resolved: Option<&Resolved>, world: &str, name: &str) -> Option<String> {
    let bind = resolved?
        .worlds
        .iter()
        .find(|w| w.name == world)?
        .binds
        .iter()
        .find(|b| b.name == name)?;
    let args: Vec<String> = bind
        .args
        .iter()
        .map(|arg| match &arg.value {
            BindingValue::Named { target, map, .. } => {
                let mapped = if map.is_some() { " with map" } else { "" };
                format!("{} = {target}{mapped}", arg.name)
            }
            BindingValue::Terminator { kind, .. } => {
                format!("{} = {}", arg.name, terminator_word(*kind))
            }
        })
        .collect();
    let external = if bind.external { " (external)" } else { "" };
    Some(format!(
        "bind {}({}) as {name}{external}",
        bind.target,
        args.join(", ")
    ))
}

fn terminator_word(kind: crate::parser::TermKind) -> &'static str {
    match kind {
        crate::parser::TermKind::Return => "return",
        crate::parser::TermKind::Stop => "stop",
        crate::parser::TermKind::Halt => "halt",
    }
}
