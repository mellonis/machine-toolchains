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

use super::{DocState, render_doc, span_touches};
use crate::compiler::{Resolved, WorldKind, full_name};
use crate::parser::{
    Alphabet, Bind, BindingArg, BindingValue, Continuation, Graft, Program, Signature,
    SigParamKind, State, Transition,
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
            let mangled =
                resolve_written(program, alphabet, world.ns, alphabet_exists(program))?;
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
            let mangled = resolve_written(
                program,
                &bind.target.joined(),
                world.ns,
                world_exists(program),
            )?;
            return Some((Target::World(mangled), bind.target.span));
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
                Transition::Goto { name, span, .. } if span_touches(*span, pos) => {
                    return Some((world_local(world, name), *span));
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
                        let mangled =
                            resolve_written(program, &written, world.ns, world_exists(program))?;
                        return Some((Target::World(mangled), target.span));
                    }
                    if let Continuation::State { name, span } = then
                        && span_touches(*span, pos)
                    {
                        return Some((world_local(world, name), *span));
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
fn declaration_span(program: &Program, target: &Target) -> Option<Span> {
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
    }
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
    let span = declaration_span(program, &target)?;
    Some(DefTarget {
        uri: uri.to_string(),
        span,
        origin: Some(origin),
    })
}

pub(super) fn hover(state: &DocState, pos: Pos) -> Option<HoverContent> {
    let program = state.program.as_ref()?;
    let (target, origin) = reference_at(program, pos)?;
    let text = render(program, state.resolved.as_ref(), &target)?;
    Some(HoverContent { text, span: origin })
}

/// The hover body for a target: a signature line first, then the
/// declaration's doc and deprecation callouts under it.
fn render(program: &Program, resolved: Option<&Resolved>, target: &Target) -> Option<String> {
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
    };
    let doc = doc_key
        .and_then(|key| resolved.and_then(|r| r.docs.get(key)))
        .and_then(render_doc);
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
            SigParamKind::Tape { alphabet, .. } => format!("tape {}: {alphabet}", p.name),
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
