//! Completion candidates (docs/lsp.md (completions)): one classified
//! cursor in, one context-appropriate list out.
//!
//! Positions come from [`super::context::classify`] over the CURRENT token
//! stream; names and symbols come from the roster, which may be one edit
//! old. Everything a candidate needs is already on the classified cursor —
//! nothing is re-derived here.
//!
//! Every candidate stamps the cursor's own `replace_span`, so the client
//! replaces exactly the token being typed. The server never filters by the
//! typed prefix; that is the client's job over `replace_span`.

use std::collections::HashSet;

use mtc_core::diagnostics::{Pos, Span};
use mtc_core::lsp::{Candidate, CandidateKind};

use super::context::{CallKind, Context, Cursor, VectorKind, classify};
use super::overlay::OverlaySym;
use super::roster::{ParamKind, Roster};
use super::{DocState, significant, std_enabled};
use crate::compiler::WorldKind;

/// The completion candidates for `pos` in `state`'s current document.
pub(super) fn completion(state: &DocState, pos: Pos) -> Vec<Candidate> {
    let Some(tokens) = &state.tokens else {
        return Vec::new(); // lexing itself failed
    };
    let sig = significant(tokens);
    let Some(cursor) = classify(&sig, pos) else {
        return Vec::new();
    };
    let roster = state.roster.as_ref();
    candidates(&cursor, roster, state)
}

fn candidates(cursor: &Cursor, roster: Option<&Roster>, state: &DocState) -> Vec<Candidate> {
    let span = cursor.replace_span;
    match &cursor.context {
        Context::UsePath => match roster {
            Some(roster) => importable(roster, state, span),
            None => Vec::new(),
        },
        Context::TopLevelItem => keywords(
            &[
                "alphabet",
                "export",
                "graph",
                "machine",
                "namespace",
                "routine",
                "use",
            ],
            span,
        ),
        Context::WorldItem { machine } => {
            let mut words = vec!["bind", "entry", "graft", "state"];
            if *machine {
                words.push("tape");
            }
            words.sort_unstable();
            keywords(&words, span)
        }
        Context::AlphabetRef => match roster {
            Some(roster) => named_decls(
                roster,
                roster.alphabet_names(),
                CandidateKind::Module,
                "alphabet",
                span,
            ),
            None => Vec::new(),
        },
        Context::VectorCell { kind, index } => vector_cell(*kind, *index, cursor, roster, span),
        Context::ActionStart => {
            let mut out = keywords(
                &[
                    "call", "debugger", "goto", "halt", "move", "return", "stop", "write",
                ],
                span,
            );
            out.extend(transition_targets(cursor, roster, span));
            out
        }
        Context::GotoTarget => transition_targets(cursor, roster, span),
        Context::Continuation => {
            let mut out = transition_targets(cursor, roster, span);
            out.extend(keywords(&["halt", "return", "stop"], span));
            out
        }
        Context::Target(kind) => match roster {
            Some(roster) => target_names(*kind, cursor, roster, state, span),
            None => Vec::new(),
        },
        Context::BindingName { target } => match (roster, target) {
            (Some(roster), Some(target)) => binding_names(roster, cursor, target, span),
            _ => Vec::new(),
        },
        Context::BindingValue { target, param } => {
            binding_value(cursor, roster, target.as_deref(), param.as_deref(), span)
        }
        Context::MapSrc { host_tape } => match (roster, host_tape) {
            (Some(roster), Some(tape)) => {
                let alphabet = cursor
                    .world
                    .as_deref()
                    .and_then(|w| roster.worlds.get(w))
                    .and_then(|world| world.alphabet_of_param(tape));
                glyph_candidates(roster, alphabet, span)
            }
            _ => Vec::new(),
        },
        Context::MapDst { target, param } => match (roster, target, param) {
            (Some(roster), Some(target), Some(param)) => {
                let alphabet = roster
                    .resolve_world(target, &cursor.namespaces)
                    .and_then(|world| world.alphabet_of_param(param));
                glyph_candidates(roster, alphabet, span)
            }
            _ => Vec::new(),
        },
    }
}

/// The cell contexts, where the tape's alphabet is the whole point.
///
/// The resolution chain is: enclosing world (from the frame stack) → its
/// tape table → the tape at THIS vector position → that tape's alphabet →
/// its symbols. A world the roster does not know, or a position past the
/// world's arity, contributes no symbols — the vector's own literal
/// vocabulary is still offered, since that part needs no tape at all.
fn vector_cell(
    kind: VectorKind,
    index: usize,
    cursor: &Cursor,
    roster: Option<&Roster>,
    span: Span,
) -> Vec<Candidate> {
    if kind == VectorKind::Move {
        // A move cell's vocabulary is closed and tape-independent.
        return vec![
            literal("<", "move the head left", span),
            literal(">", "move the head right", span),
            literal(".", "keep the head where it is", span),
        ];
    }
    let mut out = match kind {
        VectorKind::Pattern => vec![literal("*", "match any symbol", span)],
        VectorKind::Write => vec![literal("-", "keep the cell's current symbol", span)],
        VectorKind::Move => Vec::new(),
    };
    let Some(roster) = roster else {
        return out;
    };
    let alphabet = cursor
        .world
        .as_deref()
        .and_then(|w| roster.worlds.get(w))
        .and_then(|world| world.alphabet_at(index));
    out.extend(glyph_candidates(roster, alphabet, span));
    out
}

/// The symbols of an alphabet, spelled the way source spells them.
fn glyph_candidates(roster: &Roster, alphabet: Option<&str>, span: Span) -> Vec<Candidate> {
    let Some(glyphs) = alphabet.and_then(|a| roster.glyphs(a)) else {
        return Vec::new();
    };
    let detail = alphabet.map(|a| format!("alphabet {a}"));
    glyphs
        .iter()
        .map(|entry| {
            let text = entry.spelling();
            Candidate {
                label: text.clone(),
                kind: CandidateKind::Value,
                replace_span: span,
                insert_text: text,
                detail: detail.clone(),
                deprecated: false,
            }
        })
        .collect()
}

/// Everything a `goto` or a `then` continuation may address in the
/// enclosing world: its states, its state parameters, and its graft
/// instance names.
fn transition_targets(cursor: &Cursor, roster: Option<&Roster>, span: Span) -> Vec<Candidate> {
    let Some(world) = roster
        .zip(cursor.world.as_deref())
        .and_then(|(roster, name)| roster.worlds.get(name))
    else {
        return Vec::new();
    };
    named(
        world.transition_targets(),
        CandidateKind::Function,
        "state",
        span,
    )
}

/// The names legal in a `call` / `graft` / `bind` target slot: routines
/// for a call or a bind, graphs for a graft, plus — for a call only — the
/// enclosing world's bind instances, which are call targets in their own
/// right. The cross-file overlay's own exported routines join a call or a
/// bind (`overlay_target_candidates`) for the identical reason the stdlib
/// roster does — never a graft, which a link boundary cannot carry
/// (docs/tmt/stdlib.md (transparent call)).
fn target_names(
    kind: CallKind,
    cursor: &Cursor,
    roster: &Roster,
    state: &DocState,
    span: Span,
) -> Vec<Candidate> {
    match kind {
        // A graft splices a graph's SOURCE, which a link boundary does not
        // carry — the stdlib exposes no name here (docs/tmt/stdlib.md
        // (transparent call)).
        CallKind::Graft => named_decls(
            roster,
            roster.graph_names(),
            CandidateKind::Function,
            "graph",
            span,
        ),
        CallKind::Bind => {
            let mut out = named_decls(
                roster,
                roster.routine_names(),
                CandidateKind::Function,
                "routine",
                span,
            );
            out.extend(std_routine_candidates(state, span));
            out.extend(overlay_target_candidates(state, span));
            out
        }
        CallKind::Call => {
            let mut out = named_decls(
                roster,
                roster.routine_names(),
                CandidateKind::Function,
                "routine",
                span,
            );
            if let Some(world) = cursor.world.as_deref().and_then(|w| roster.worlds.get(w)) {
                out.extend(named(
                    world.binds.clone(),
                    CandidateKind::Function,
                    "bind",
                    span,
                ));
            }
            out.extend(std_routine_candidates(state, span));
            out.extend(overlay_target_candidates(state, span));
            out
        }
    }
}

/// Whether the cross-file overlay defines `full_path` OUTRIGHT — the
/// check every stdlib-candidate site shares before offering its own
/// entry, so a sibling's shadowing declaration (docs/tmt/project.md
/// (schema reference): the embedded stdlib links as an ordinary library,
/// first-wins) suppresses the embedded copy instead of duplicating it.
fn overlay_owns(state: &DocState, full_path: &str) -> bool {
    state
        .overlay
        .as_ref()
        .is_some_and(|overlay| overlay.symbols.contains_key(full_path))
}

/// The stdlib roster's qualified routine names, as `call`/`bind` target
/// candidates — a transparent, argless `call` (or a `bind` that skips
/// binding-arg validation) is the one call shape that works against the
/// linked object (docs/tmt/stdlib.md (transparent call)). Gated on
/// [`std_enabled`]: a project opting out of the stdlib offers none of
/// these. An entry the cross-file overlay already OWNS (a sibling's own
/// `namespace std { export … }`, mangling to the same qualified name a
/// roster entry carries) is skipped here — `overlay_target_candidates`
/// emits the overlay's own entry for that same label instead, so the
/// sibling's copy shadows the embedded one exactly as the linker's own
/// sources-before-libraries order does, rather than the two surfacing as
/// a visible duplicate.
fn std_routine_candidates(state: &DocState, span: Span) -> Vec<Candidate> {
    if !std_enabled(state) {
        return Vec::new();
    }
    crate::stdlib::roster()
        .iter()
        .filter(|entry| !overlay_owns(state, &entry.full_path))
        .map(|entry| Candidate {
            label: entry.full_path.clone(),
            kind: CandidateKind::Function,
            replace_span: span,
            insert_text: entry.full_path.clone(),
            detail: Some("routine".to_string()),
            deprecated: false,
        })
        .collect()
}

/// The cross-file overlay's own `call`/`bind` target candidates: every
/// name it defines (docs/lsp.md (project overlay)), as a qualified label —
/// the SAME transparent-call shape the stdlib roster offers, since both
/// cross a compiled-object boundary the same way. `deprecated` comes
/// from the contributing SIBLING's own `OverlaySym.doc` rather than this
/// document's roster — a sibling's doc comment lives in a different
/// document's analysis entirely, never this one's.
///
/// `main` is excluded: a `machine` world always contributes its linker
/// symbol to the table (the diagnostics refinement this table also
/// serves treats it as a legitimately defined external name), but a
/// machine is a program's own entry, never something another unit
/// `call`s or `bind`s — the local loop just above skips it for the
/// identical reason (`WorldKind::Machine => continue`), and the overlay
/// table carries no per-entry kind to filter on more precisely than by
/// this one reserved name.
fn overlay_target_candidates(state: &DocState, span: Span) -> Vec<Candidate> {
    let Some(overlay) = state.overlay.as_ref() else {
        return Vec::new();
    };
    overlay
        .symbols
        .iter()
        .filter(|(full, _)| full.as_str() != "main")
        .map(|(full, sym)| overlay_routine_candidate(full.clone(), Some(sym), span))
        .collect()
}

/// One overlay-sourced routine candidate, in the same `Function`/"routine"
/// shape a local declaration's own candidate takes (`named_decls`), but
/// `deprecated` sourced from `sym`'s own `Doc` instead of this document's
/// roster.
fn overlay_routine_candidate(label: String, sym: Option<&OverlaySym>, span: Span) -> Candidate {
    Candidate {
        deprecated: sym
            .and_then(|sym| sym.doc.as_ref())
            .is_some_and(|doc| doc.deprecated.is_some()),
        insert_text: label.clone(),
        label,
        kind: CandidateKind::Function,
        replace_span: span,
        detail: Some("routine".to_string()),
    }
}

/// A binding argument's parameter names: the target world's signature,
/// tapes and state parameters alike.
fn binding_names(roster: &Roster, cursor: &Cursor, target: &str, span: Span) -> Vec<Candidate> {
    let Some(world) = roster.resolve_world(target, &cursor.namespaces) else {
        return Vec::new();
    };
    let mut out: Vec<Candidate> = world
        .tapes
        .iter()
        .map(|(name, alphabet)| Candidate {
            label: name.clone(),
            kind: CandidateKind::Value,
            replace_span: span,
            insert_text: name.clone(),
            detail: Some(format!("tape param: {alphabet}")),
            deprecated: false,
        })
        .collect();
    out.extend(named(
        world.state_params.clone(),
        CandidateKind::Value,
        "state param",
        span,
    ));
    out
}

/// A binding argument's value, filtered by which half of the CALLEE's
/// signature the parameter names: a tape parameter takes a tape of the
/// enclosing world, a state parameter takes one of that world's transition
/// targets or a continuation terminator. The two vocabularies are disjoint,
/// so offering both is offering a wrong answer half the time.
///
/// When the parameter cannot be classified — an unresolvable callee, a
/// parameter name not in its signature, a roster one edit stale — the union
/// is offered instead. An editor degrading to MORE candidates is a
/// nuisance; degrading to none is a dead completion list.
fn binding_value(
    cursor: &Cursor,
    roster: Option<&Roster>,
    target: Option<&str>,
    param: Option<&str>,
    span: Span,
) -> Vec<Candidate> {
    let Some(roster) = roster else {
        return Vec::new();
    };
    let Some(world) = cursor.world.as_deref().and_then(|w| roster.worlds.get(w)) else {
        return Vec::new();
    };
    let kind = target.zip(param).and_then(|(target, param)| {
        roster
            .resolve_world(target, &cursor.namespaces)
            .and_then(|callee| callee.param_kind(param))
    });
    let mut out: Vec<Candidate> = Vec::new();
    if kind != Some(ParamKind::State) {
        out.extend(world.tapes.iter().map(|(name, alphabet)| Candidate {
            label: name.clone(),
            kind: CandidateKind::Value,
            replace_span: span,
            insert_text: name.clone(),
            detail: Some(format!("tape: {alphabet}")),
            deprecated: false,
        }));
    }
    if kind != Some(ParamKind::Tape) {
        out.extend(named(
            world.transition_targets(),
            CandidateKind::Function,
            "state",
            span,
        ));
        out.extend(keywords(&["halt", "return", "stop"], span));
    }
    out
}

/// The names a `use` path may reach: every top-level world and alphabet
/// the file defines, by mangled name, plus the standard library — the one
/// well-known external always available without configuration — and the
/// cross-file overlay's own sibling/library routines (docs/lsp.md
/// (configuration)). Any OTHER cross-file namespace is invisible by
/// design — only this document, the stdlib, and the overlay ever
/// contribute a candidate.
fn importable(roster: &Roster, state: &DocState, span: Span) -> Vec<Candidate> {
    let mut out: Vec<Candidate> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for name in roster.alphabet_names() {
        // Not the tautology it looks like: `alphabet_names` appends the
        // bare spellings existing `use` statements bound, and those are
        // exactly the names the alphabet table does NOT key. The guard
        // therefore drops them — which is what a `use` path wants, since
        // it names full paths and re-importing an alias is not one.
        if roster.has_alphabet(&name) {
            seen.insert(name.clone());
            out.push(decl(roster, name, CandidateKind::Module, "alphabet", span));
        }
    }
    for (name, world) in &roster.worlds {
        let detail = match world.kind {
            WorldKind::Routine => "routine",
            WorldKind::Graph => "graph",
            WorldKind::Machine => continue,
        };
        seen.insert(name.clone());
        out.push(decl(
            roster,
            name.clone(),
            CandidateKind::Function,
            detail,
            span,
        ));
    }
    if std_enabled(state) {
        seen.insert("std".to_string());
        out.push(Candidate {
            label: "std".to_string(),
            kind: CandidateKind::Module,
            replace_span: span,
            insert_text: "std".to_string(),
            detail: Some("standard library".to_string()),
            deprecated: false,
        });
        // Routines only — the stdlib's graphs and alphabets contribute no
        // linkable symbol a `use` path could ever bind (docs/tmt/stdlib.md
        // (transparent call)). An entry the overlay already OWNS is
        // skipped — `overlay_importable_candidates` below emits the
        // overlay's own entry for that label instead, the sibling's copy
        // shadowing the embedded one exactly as the linker itself would
        // (docs/tmt/project.md (schema reference)).
        for entry in crate::stdlib::roster() {
            if overlay_owns(state, &entry.full_path) {
                continue;
            }
            seen.insert(entry.full_path.clone());
            out.push(Candidate {
                label: entry.full_path.clone(),
                kind: CandidateKind::Function,
                replace_span: span,
                insert_text: entry.full_path.clone(),
                detail: Some("routine".to_string()),
                deprecated: false,
            });
        }
    }
    out.extend(overlay_importable_candidates(state, &mut seen, span));
    out.sort_by(|a, b| a.label.cmp(&b.label));
    out
}

/// The cross-file overlay's own `use`-path candidates (docs/lsp.md
/// (configuration)): every top-level member — a bare leaf routine, or a
/// namespace ROOT the user can keep typing past, the identical
/// convenience the hardcoded `"std"` literal above offers for the
/// embedded library — plus every namespaced full path the overlay
/// defines, so a routine can be picked whole without walking its
/// namespace one segment at a time (mirrors the stdlib's own full-path
/// loop above). `seen` carries every label already offered — by a local
/// declaration, or by the (possibly shadowed) embedded stdlib — so a name
/// already visible never surfaces twice. `main` is excluded from the
/// top-level leaf/root split for the same reason `overlay_target_
/// candidates` excludes it: a machine's linker symbol is not a `use`-
/// able unit.
fn overlay_importable_candidates(
    state: &DocState,
    seen: &mut HashSet<String>,
    span: Span,
) -> Vec<Candidate> {
    let Some(overlay) = state.overlay.as_ref() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let top_level: Vec<String> = Vec::new();
    if let Some(members) = overlay.members.get(&top_level) {
        for (bare, full) in members {
            if bare == "main" {
                continue;
            }
            if !seen.insert(bare.clone()) {
                continue;
            }
            out.push(match overlay.symbols.get(full) {
                // A genuine leaf: this bare name IS itself an exported
                // routine, not merely a namespace prefix.
                Some(sym) => overlay_routine_candidate(bare.clone(), Some(sym), span),
                // A pure namespace root — nothing to jump to or document,
                // only a prefix worth completing past.
                None => Candidate {
                    label: bare.clone(),
                    kind: CandidateKind::Module,
                    replace_span: span,
                    insert_text: bare.clone(),
                    detail: Some("namespace".to_string()),
                    deprecated: false,
                },
            });
        }
    }
    let mut quals: Vec<&String> = overlay
        .symbols
        .keys()
        .filter(|full| full.contains("::"))
        .collect();
    quals.sort();
    for full in quals {
        if seen.insert(full.clone()) {
            out.push(overlay_routine_candidate(
                full.clone(),
                overlay.symbols.get(full),
                span,
            ));
        }
    }
    out
}

fn keywords(words: &[&str], span: Span) -> Vec<Candidate> {
    words
        .iter()
        .map(|word| Candidate {
            label: (*word).to_string(),
            kind: CandidateKind::Keyword,
            replace_span: span,
            insert_text: (*word).to_string(),
            detail: None,
            deprecated: false,
        })
        .collect()
}

fn literal(text: &str, detail: &str, span: Span) -> Candidate {
    Candidate {
        label: text.to_string(),
        kind: CandidateKind::Keyword,
        replace_span: span,
        insert_text: text.to_string(),
        detail: Some(detail.to_string()),
        deprecated: false,
    }
}

/// Declaration-name candidates: [`named`], plus each name's own deprecation
/// tag. Only DECLARATIONS can be deprecated — routines, graphs, alphabets —
/// so the lookup lives at those call sites rather than inside [`one`], which
/// also serves states, graft instances and bind instances (names that carry no
/// doc of their own and would silently borrow a same-spelled declaration's).
fn named_decls(
    roster: &Roster,
    names: Vec<String>,
    kind: CandidateKind,
    detail: &str,
    span: Span,
) -> Vec<Candidate> {
    names
        .into_iter()
        .map(|name| decl(roster, name, kind, detail, span))
        .collect()
}

/// One declaration-name candidate, tagged from the roster.
fn decl(roster: &Roster, name: String, kind: CandidateKind, detail: &str, span: Span) -> Candidate {
    Candidate {
        deprecated: roster.is_deprecated(&name),
        ..one(name, kind, detail, span)
    }
}

fn named(names: Vec<String>, kind: CandidateKind, detail: &str, span: Span) -> Vec<Candidate> {
    names
        .into_iter()
        .map(|name| one(name, kind, detail, span))
        .collect()
}

fn one(name: String, kind: CandidateKind, detail: &str, span: Span) -> Candidate {
    Candidate {
        label: name.clone(),
        kind,
        replace_span: span,
        insert_text: name,
        detail: Some(detail.to_string()),
        deprecated: false,
    }
}
