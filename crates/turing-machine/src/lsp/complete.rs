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

use mtc_core::diagnostics::{Pos, Span};
use mtc_core::lsp::{Candidate, CandidateKind};

use super::context::{CallKind, Context, Cursor, VectorKind, classify};
use super::roster::{ParamKind, Roster};
use super::{DocState, significant};
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
    candidates(&cursor, roster)
}

fn candidates(cursor: &Cursor, roster: Option<&Roster>) -> Vec<Candidate> {
    let span = cursor.replace_span;
    match &cursor.context {
        Context::UsePath => match roster {
            Some(roster) => importable(roster, span),
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
            Some(roster) => target_names(*kind, cursor, roster, span),
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
/// right.
fn target_names(kind: CallKind, cursor: &Cursor, roster: &Roster, span: Span) -> Vec<Candidate> {
    match kind {
        CallKind::Graft => named_decls(
            roster,
            roster.graph_names(),
            CandidateKind::Function,
            "graph",
            span,
        ),
        CallKind::Bind => named_decls(
            roster,
            roster.routine_names(),
            CandidateKind::Function,
            "routine",
            span,
        ),
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
            out
        }
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
/// the file defines, by mangled name. Cross-file namespaces are invisible
/// by design — only this document ever contributes a candidate.
fn importable(roster: &Roster, span: Span) -> Vec<Candidate> {
    let mut out: Vec<Candidate> = Vec::new();
    for name in roster.alphabet_names() {
        // Not the tautology it looks like: `alphabet_names` appends the
        // bare spellings existing `use` statements bound, and those are
        // exactly the names the alphabet table does NOT key. The guard
        // therefore drops them — which is what a `use` path wants, since
        // it names full paths and re-importing an alias is not one.
        if roster.has_alphabet(&name) {
            out.push(decl(roster, name, CandidateKind::Module, "alphabet", span));
        }
    }
    for (name, world) in &roster.worlds {
        let detail = match world.kind {
            WorldKind::Routine => "routine",
            WorldKind::Graph => "graph",
            WorldKind::Machine => continue,
        };
        out.push(decl(
            roster,
            name.clone(),
            CandidateKind::Function,
            detail,
            span,
        ));
    }
    out.sort_by(|a, b| a.label.cmp(&b.label));
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
