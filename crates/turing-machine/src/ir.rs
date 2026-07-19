//! The TM-1 intermediate representation — per-world STATE GRAPHS, a versioned,
//! documented JSON artifact (not an internal detail). "The form follows the
//! model": a Turing world is a set of states, each a priority-ordered list of
//! classical match rows (δ), so the IR is a graph of states rather than the
//! basic-block CFG the imperative `.pmc` front end lowers to.
//!
//! [`lower`] consumes the fully-expanded module ([`crate::expand::Expanded`] —
//! concrete, index-resolved rules only: no ranges, no pattern bindings, no
//! grafts; graft holes already survive as [`crate::expand::Transition2::TrapRead`]
//! / [`TrapWrite`](crate::expand::Transition2::TrapWrite) markers) together with
//! the [`crate::compiler::Resolved`] context (visibility, `bind` records, world
//! name spans). Codegen (Task 7) consumes the IR; `tmt ir graph` (Task 8) renders
//! [`IrWorld::to_mermaid`].
//!
//! # The versioned contract ([`TM_IR_VERSION`])
//!
//! The IR is **index-only**: match rows and action vectors carry symbol
//! INDICES, never glyphs — the processor never sees glyphs, and the spec's
//! match rows are index-resolved. Per-tape *alphabet names* and cardinalities
//! ride along for readability (`tmt ir`) and index-bound validation, but the
//! glyph tables themselves stay in the presentation layers (the `.pmx`/`.tmx`
//! map sidecar, MT snapshots), never here.
//!
//! State ids are dense (`0..states.len()`) in the module's EMISSION order (a
//! world's own states in source order, then its spliced graft instances). The
//! entry state is named by [`IrWorld::entry`] (its id), not moved to position
//! zero: a graft-entry instance is emitted after the host's own states, and
//! reordering would sever the source-to-IR provenance every rule carries as a
//! line number. Reachability walks from [`IrWorld::entry`].
//!
//! A cross-world `call` carries the declarative binding-call record
//! ([`IrTransition::CallThen`]'s `binding`): the SAME per-callee-tape data the
//! `.tma` binding-call operand carries (Task 7 renders it), with `caller_tape`
//! the host physical tape index and each pair's `src`/`dst` the AUTHORED
//! symbols resolved to caller/callee alphabet indices. No blank pin or closure
//! is applied here — the composition engine does that at link.
//!
//! Unused until Task 7 wires `compile()` over it (codegen consumes the output);
//! the in-module tests exercise the lowering meanwhile.
#![allow(dead_code)]

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

use mtc_core::diagnostics::{Diagnostic, Span};

use crate::compiler::{CompileError, CompileErrorKind, Resolved, ResolvedWorld, WorldKind};
use crate::expand::{Cell, Expanded, ExpandedRule, ExpandedWorld, Transition2, WriteOut};
use crate::parser::{BindingArg, BindingValue, Continuation, MapArrow, MoveDir, SymLit};

/// The TM IR encoding version. Bumps on any change to the serialized shape
/// (field names, serde tags). Embedded in every [`IrProgram`] and pinned by a
/// round-trip test, the `.pmc` `IR_VERSION` discipline.
pub const TM_IR_VERSION: u32 = 1;

/// A whole compiled module: its emitted worlds plus the index (into `worlds`)
/// of the `machine` block — the program entry — or `None` for a library.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IrProgram {
    pub version: u32,
    pub worlds: Vec<IrWorld>,
    /// Index into `worlds` of the machine block; `None` for a library.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entry_world: Option<usize>,
}

/// One emitted world — the `machine` block or a `routine`. Graphs never appear
/// (they are spliced into their graft hosts before lowering).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IrWorld {
    /// Mangled name (`main` for the machine, `ns::name` for a routine).
    pub name: String,
    pub kind: IrWorldKind,
    /// Tape count — the width of every match/action vector in this world.
    pub arity: u32,
    /// Tapes in vector-position order.
    pub tapes: Vec<IrTape>,
    /// The entry state's id. Every world has exactly one entry.
    pub entry: u32,
    /// States in emission order; ids are dense `0..states.len()`.
    pub states: Vec<IrState>,
    /// Hidden-by-default visibility: `true` unless the source `export`ed the
    /// world. The machine is always `local == false`.
    pub local: bool,
    /// Source line of the world's definition; `0` if unknown.
    pub line: u32,
}

/// A world kind that survives to the IR (graphs are gone).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IrWorldKind {
    Machine,
    Routine,
}

/// A tape's position, name, and the index bound its symbols must respect. The
/// `alphabet` name is presentation only (readability of `tmt ir`); the IR
/// itself is index-only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IrTape {
    pub name: String,
    pub alphabet: String,
    pub cardinality: u32,
}

/// One state: an id, its source name (synthetic for graft-instance internals),
/// and its rules in priority (row) order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IrState {
    pub id: u32,
    pub name: String,
    /// Source line of the state's declaration; `0` if unknown.
    pub line: u32,
    pub rules: Vec<IrRule>,
}

/// One classical match row (δ): a per-tape pattern, an optional write/move
/// action (elided when it is the identity — all-keep / all-stay), a
/// `debugger` head-break flag, a control transition, and provenance.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IrRule {
    /// Match cell per tape (`arity`-wide).
    pub pattern: Vec<IrCell>,
    /// Write cell per tape, or `None` when the whole write vector is `keep`
    /// (codegen elides the action). `arity`-wide when present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub write: Option<Vec<IrWrite>>,
    /// Move per tape, or `None` when every tape stays. `arity`-wide when
    /// present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub moves: Option<Vec<IrMove>>,
    /// `debugger` — pause at this row's code head (`brk`).
    #[serde(default, skip_serializing_if = "is_false")]
    pub debugger: bool,
    pub transition: IrTransition,
    /// `true` for a compiler-synthesized row (a graft hole's trap row). A
    /// trap transition may appear ONLY on a synthesized row (validated).
    #[serde(default, skip_serializing_if = "is_false")]
    pub synthesized: bool,
    /// Source line the row derives from; `0` if unknown.
    pub line: u32,
}

fn is_false(b: &bool) -> bool {
    !*b
}

/// A match cell: a concrete symbol index, or `*` (any symbol).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum IrCell {
    Wildcard,
    Index { index: u32 },
}

/// A write cell: keep the current symbol, or write a concrete symbol index.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum IrWrite {
    Keep,
    Index { index: u32 },
}

/// A head move for one tape.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IrMove {
    Left,
    Right,
    Stay,
}

/// A row's control transfer. `Goto` stays in-world; `CallThen` crosses to a
/// routine and resumes at `then`; the terminators end the run; the two traps
/// are the graft-hole failure kinds (`trap #0` / `trap #1`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum IrTransition {
    /// `goto` a same-world state (its id).
    Goto {
        state: u32,
    },
    /// `call target(binding) then cont` — cross-world with a resume point.
    CallThen {
        /// Mangled callee routine name.
        target: String,
        /// The binding-call record: `binding[k]` binds the callee's virtual
        /// tape `k`. Empty for a bindless `call`.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        binding: Vec<IrTapeBinding>,
        then: IrThen,
    },
    Return,
    Stop,
    Halt,
    /// A synthesized unmapped-read trap (`trap #0`).
    TrapRead,
    /// A synthesized unmapped-write trap (`trap #1`).
    TrapWrite,
}

/// A `call … then` resume point: a same-world state (its id) or a terminator.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum IrThen {
    Goto { state: u32 },
    Return,
    Stop,
    Halt,
}

/// One virtual-tape binding at a call site — the SAME shape as the `.tma`
/// binding-call operand: which host physical tape feeds this callee tape, and
/// the authored symbol map between their alphabets (resolved to indices).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IrTapeBinding {
    /// Host physical tape index (< 16).
    pub caller_tape: u32,
    /// `(src, dst, one_way)` per authored pair, in source order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pairs: Vec<IrMapPair>,
}

/// One `src -> dst` (or `src => dst`, `one_way`) symbol-map pair: `src` a
/// caller-alphabet index, `dst` a callee-alphabet index.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct IrMapPair {
    pub src: u32,
    pub dst: u32,
    #[serde(default, skip_serializing_if = "is_false")]
    pub one_way: bool,
}

impl IrProgram {
    /// Pretty JSON — the documented on-disk artifact.
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).expect("IR serializes")
    }

    pub fn from_json(s: &str) -> Result<IrProgram, String> {
        serde_json::from_str(s).map_err(|e| e.to_string())
    }
}

impl IrWorld {
    /// A Mermaid flowchart of this world's state graph (`tmt ir graph`):
    /// nodes are states, edges are rows labelled by a compact pattern/action
    /// summary. Terminators (and call `then`-terminators) route to shared
    /// round terminal nodes so the whole control flow is visible.
    pub fn to_mermaid(&self) -> String {
        use std::fmt::Write as _;
        let mut out = String::from("flowchart TD\n");
        for st in &self.states {
            let _ = writeln!(out, "    S{}[\"{}\"]", st.id, escape(&st.name));
        }
        // Terminal pseudo-nodes, declared once each on first use.
        let mut terms: HashSet<&'static str> = HashSet::new();
        let declare =
            |out: &mut String, id: &'static str, text: &str, terms: &mut HashSet<&'static str>| {
                if terms.insert(id) {
                    let _ = writeln!(out, "    {id}((\"{text}\"))");
                }
            };
        // Two passes so all node declarations precede the edges.
        let mut edges = String::new();
        for st in &self.states {
            for r in &st.rules {
                let label = row_label(r);
                match &r.transition {
                    IrTransition::Goto { state } => {
                        let _ = writeln!(edges, "    S{} -->|\"{label}\"| S{state}", st.id);
                    }
                    IrTransition::CallThen { target, then, .. } => {
                        let call = format!("{label} call {}", escape(target));
                        match then {
                            IrThen::Goto { state } => {
                                let _ = writeln!(edges, "    S{} -->|\"{call}\"| S{state}", st.id);
                            }
                            IrThen::Return => {
                                declare(&mut out, "T_ret", "ret", &mut terms);
                                let _ = writeln!(edges, "    S{} -->|\"{call}\"| T_ret", st.id);
                            }
                            IrThen::Stop => {
                                declare(&mut out, "T_stp", "stp", &mut terms);
                                let _ = writeln!(edges, "    S{} -->|\"{call}\"| T_stp", st.id);
                            }
                            IrThen::Halt => {
                                declare(&mut out, "T_hlt", "hlt", &mut terms);
                                let _ = writeln!(edges, "    S{} -->|\"{call}\"| T_hlt", st.id);
                            }
                        }
                    }
                    IrTransition::Return => {
                        declare(&mut out, "T_ret", "ret", &mut terms);
                        let _ = writeln!(edges, "    S{} -->|\"{label}\"| T_ret", st.id);
                    }
                    IrTransition::Stop => {
                        declare(&mut out, "T_stp", "stp", &mut terms);
                        let _ = writeln!(edges, "    S{} -->|\"{label}\"| T_stp", st.id);
                    }
                    IrTransition::Halt => {
                        declare(&mut out, "T_hlt", "hlt", &mut terms);
                        let _ = writeln!(edges, "    S{} -->|\"{label}\"| T_hlt", st.id);
                    }
                    IrTransition::TrapRead => {
                        declare(&mut out, "T_trap0", "trap #0", &mut terms);
                        let _ = writeln!(edges, "    S{} -->|\"{label}\"| T_trap0", st.id);
                    }
                    IrTransition::TrapWrite => {
                        declare(&mut out, "T_trap1", "trap #1", &mut terms);
                        let _ = writeln!(edges, "    S{} -->|\"{label}\"| T_trap1", st.id);
                    }
                }
            }
        }
        out.push_str(&edges);
        out
    }
}

/// A compact row summary for a Mermaid edge label: the pattern, then the
/// write/move action when present, then a `brk` marker. ASCII only; `"` and
/// `|` are stripped since Mermaid edge labels are pipe-delimited.
fn row_label(r: &IrRule) -> String {
    let mut s = String::new();
    if r.debugger {
        s.push_str("brk ");
    }
    s.push('[');
    for (i, c) in r.pattern.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        match c {
            IrCell::Wildcard => s.push('*'),
            IrCell::Index { index } => {
                let _ = std::fmt::Write::write_fmt(&mut s, format_args!("{index}"));
            }
        }
    }
    s.push(']');
    if let Some(w) = &r.write {
        s.push_str(" w[");
        for (i, c) in w.iter().enumerate() {
            if i > 0 {
                s.push(',');
            }
            match c {
                IrWrite::Keep => s.push('-'),
                IrWrite::Index { index } => {
                    let _ = std::fmt::Write::write_fmt(&mut s, format_args!("{index}"));
                }
            }
        }
        s.push(']');
    }
    if let Some(m) = &r.moves {
        s.push_str(" m[");
        for (i, d) in m.iter().enumerate() {
            if i > 0 {
                s.push(',');
            }
            s.push(match d {
                IrMove::Left => '<',
                IrMove::Right => '>',
                IrMove::Stay => '.',
            });
        }
        s.push(']');
    }
    s
}

/// Strip the two characters Mermaid quoted labels cannot carry.
fn escape(s: &str) -> String {
    s.replace(['"', '|'], "")
}

// ---------------------------------------------------------------------------
// Lowering: Expanded (+ Resolved context) → IrProgram + diagnostics.
// ---------------------------------------------------------------------------

/// Lower a fully-expanded module to the TM IR. Index resolution is already
/// done by [`crate::expand`] for match/action cells; this stage assigns dense
/// per-world state ids, resolves goto / call-`then` / call targets to those
/// ids, resolves each `call`/`bind` site's binding record to alphabet indices,
/// and emits reachability warnings (unreachable state, unused routine). The
/// `resolved` context supplies visibility, world spans, and `bind` records.
pub(crate) fn lower(
    expanded: &Expanded,
    resolved: &Resolved,
) -> Result<(IrProgram, Vec<Diagnostic>), CompileError> {
    let mut warnings = Vec::new();

    // Correlate emitted worlds with their resolved originals by mangled name.
    let by_name: HashMap<&str, &ResolvedWorld> = resolved
        .worlds
        .iter()
        .map(|w| (w.name.as_str(), w))
        .collect();

    let mut worlds = Vec::with_capacity(expanded.worlds.len());
    for ew in &expanded.worlds {
        let rw = by_name.get(ew.name.as_str()).copied();
        worlds.push(lower_world(ew, rw, expanded, resolved, &mut warnings)?);
    }

    let program = IrProgram {
        version: TM_IR_VERSION,
        worlds,
        entry_world: expanded.entry_world,
    };

    // Unused-routine warnings: a non-exported routine referenced by no call
    // (spec's report list; the cheap "referenced by any call/graft" form).
    unused_routine_warnings(&program, &by_name, &mut warnings);

    Ok((program, warnings))
}

fn lower_world(
    ew: &ExpandedWorld,
    rw: Option<&ResolvedWorld>,
    expanded: &Expanded,
    resolved: &Resolved,
    warnings: &mut Vec<Diagnostic>,
) -> Result<IrWorld, CompileError> {
    let arity = ew.tapes.len();

    // Dense ids in emission order; the name→id map resolves gotos and thens.
    let name_to_id: HashMap<&str, u32> = ew
        .states
        .iter()
        .enumerate()
        .map(|(i, s)| (s.name.as_str(), i as u32))
        .collect();

    let entry_name = ew
        .entry
        .as_deref()
        .expect("an emitted world always has a concrete entry state");
    let entry = *name_to_id
        .get(entry_name)
        .expect("the entry names one of the world's states");

    let mut states = Vec::with_capacity(ew.states.len());
    for (i, s) in ew.states.iter().enumerate() {
        let mut rules = Vec::with_capacity(s.rules.len());
        for r in &s.rules {
            rules.push(lower_rule(r, ew, &name_to_id, expanded, resolved)?);
        }
        states.push(IrState {
            id: i as u32,
            name: s.name.clone(),
            line: s.name_span.start.line,
            rules,
        });
    }

    let world = IrWorld {
        name: ew.name.clone(),
        kind: match ew.kind {
            WorldKind::Machine => IrWorldKind::Machine,
            WorldKind::Routine => IrWorldKind::Routine,
            WorldKind::Graph => unreachable!("graphs are spliced away before lowering"),
        },
        arity: arity as u32,
        tapes: ew
            .tapes
            .iter()
            .map(|t| IrTape {
                name: t.name.clone(),
                alphabet: t.alphabet.clone(),
                cardinality: t.cardinality as u32,
            })
            .collect(),
        entry,
        states,
        local: rw.map(|w| w.local).unwrap_or(false),
        line: rw.map(|w| w.name_span.start.line).unwrap_or(0),
    };

    unreachable_state_warnings(&world, ew, warnings);
    Ok(world)
}

fn lower_rule(
    r: &ExpandedRule,
    ew: &ExpandedWorld,
    name_to_id: &HashMap<&str, u32>,
    expanded: &Expanded,
    resolved: &Resolved,
) -> Result<IrRule, CompileError> {
    let pattern: Vec<IrCell> = r
        .pattern
        .iter()
        .map(|c| match c {
            Cell::Wild => IrCell::Wildcard,
            Cell::Sym(s) => IrCell::Index { index: *s as u32 },
        })
        .collect();

    // Elide an all-keep write and an all-stay move — the codegen action
    // elision (an all-keep + all-stay row emits no `wrmv`).
    let write = if r.write.iter().all(|w| matches!(w, WriteOut::Keep)) {
        None
    } else {
        Some(
            r.write
                .iter()
                .map(|w| match w {
                    WriteOut::Keep => IrWrite::Keep,
                    WriteOut::Sym(s) => IrWrite::Index { index: *s as u32 },
                })
                .collect(),
        )
    };
    let moves = if r.moves.iter().all(|m| *m == MoveDir::Stay) {
        None
    } else {
        Some(
            r.moves
                .iter()
                .map(|m| match m {
                    MoveDir::Left => IrMove::Left,
                    MoveDir::Right => IrMove::Right,
                    MoveDir::Stay => IrMove::Stay,
                })
                .collect(),
        )
    };

    let resolve_state = |name: &str| -> Result<u32, CompileError> {
        if let Some(id) = name_to_id.get(name).copied() {
            return Ok(id);
        }
        // A goto/continuation target that names no concrete state. For a
        // T4-validated module this is either the routine's own STATE PARAMETER
        // (a continuation the call site supplies — the composition engine's
        // work, out of scope here) or a genuine dangling reference. Report each
        // honestly rather than folding a not-yet-supported construct into
        // "undefined state".
        let kind = if ew.state_params.iter().any(|p| p == name) {
            CompileErrorKind::StateParamContinuationUnsupported(name.to_string())
        } else {
            CompileErrorKind::UndefinedState(name.to_string())
        };
        Err(CompileError { span: r.span, kind })
    };
    let then_of = |cont: &Continuation| -> Result<IrThen, CompileError> {
        Ok(match cont {
            Continuation::State { name, .. } => IrThen::Goto {
                state: resolve_state(name)?,
            },
            Continuation::Return { .. } => IrThen::Return,
            Continuation::Stop { .. } => IrThen::Stop,
            Continuation::Halt { .. } => IrThen::Halt,
        })
    };

    let (transition, synthesized) = match &r.transition {
        Transition2::Goto(name) => (
            IrTransition::Goto {
                state: resolve_state(name)?,
            },
            false,
        ),
        Transition2::Return => (IrTransition::Return, false),
        Transition2::Stop => (IrTransition::Stop, false),
        Transition2::Halt => (IrTransition::Halt, false),
        Transition2::TrapRead => (IrTransition::TrapRead, true),
        Transition2::TrapWrite => (IrTransition::TrapWrite, true),
        Transition2::Call {
            target,
            external,
            args,
            then,
        } => {
            let binding = resolve_binding(ew, target, args, *external, expanded, r.span)?;
            (
                IrTransition::CallThen {
                    target: target.clone(),
                    binding,
                    then: then_of(then)?,
                },
                false,
            )
        }
        Transition2::BindCall { name, then } => {
            // A bind is pure sugar: look up its routine + args in the world's
            // resolved bind table and lower to the same CallThen a direct call
            // would produce (GC9 — dedup keys on (routine, binding) regardless).
            let rw = resolved
                .worlds
                .iter()
                .find(|w| w.name == ew.name)
                .expect("the emitted world has a resolved original");
            let bind = rw
                .binds
                .iter()
                .find(|b| b.name == *name)
                .expect("a bind-call names a declared bind");
            let binding = resolve_binding(
                ew,
                &bind.target,
                &bind.args,
                bind.external,
                expanded,
                r.span,
            )?;
            (
                IrTransition::CallThen {
                    target: bind.target.clone(),
                    binding,
                    then: then_of(then)?,
                },
                false,
            )
        }
    };

    Ok(IrRule {
        pattern,
        write,
        moves,
        debugger: r.debugger,
        transition,
        synthesized,
        line: r.span.start.line,
    })
}

/// Resolve a call/bind site's source-form binding args to the per-callee-tape
/// binding-call record. `binding[k]` binds the callee's tape `k`, so the
/// records are emitted in the callee's signature tape order; `src` glyphs
/// resolve against the host (caller) tape alphabet, `dst` glyphs against the
/// callee tape alphabet — the same direction the graft splice uses.
fn resolve_binding(
    host: &ExpandedWorld,
    target: &str,
    args: &[BindingArg],
    external: bool,
    expanded: &Expanded,
    site: Span,
) -> Result<Vec<IrTapeBinding>, CompileError> {
    // A named binding arg (`name = target`) is a tape-target binding OR a
    // state-param continuation — a bare name is either, resolution decides.
    // A call with no named args carries no binding at all (a plain call the
    // linker resolves), so it needs no callee signature. State-param args do
    // not match any callee tape and drop out of the loop below; the composition
    // engine threads them (out of scope here).
    let named_args: Vec<&BindingArg> = args
        .iter()
        .filter(|a| matches!(&a.value, BindingValue::Named { .. }))
        .collect();
    if named_args.is_empty() {
        return Ok(Vec::new());
    }

    // Binding args need the callee's tape signature to rewrite its rows. That
    // signature is unknown for a routine defined outside this compilation unit
    // (imported-to-external / `::`-absolute) — a cross-object concern for the
    // composition engine, not this lowering. Refuse with a clear error rather
    // than the earlier panic on the missing world.
    if external {
        return Err(CompileError {
            span: site,
            kind: CompileErrorKind::ExternalBindingUnsupported(target.to_string()),
        });
    }

    // Non-external ⇒ the callee is one of the module's emitted worlds (`expand`
    // emits the machine and every routine, reachable or not).
    let callee = expanded
        .worlds
        .iter()
        .find(|w| w.name == target)
        .expect("a non-external callee is one of the module's emitted worlds");

    let mut binding = Vec::with_capacity(callee.tapes.len());
    for ct in &callee.tapes {
        let Some(arg) = named_args.iter().find(|a| a.name == ct.name) else {
            // Every callee tape is bound (T4's arity check); defensive.
            return Err(CompileError {
                span: site,
                kind: CompileErrorKind::MissingArg(ct.name.clone()),
            });
        };
        let BindingValue::Named {
            target: host_name,
            map,
            ..
        } = &arg.value
        else {
            unreachable!("named_args are Named by construction");
        };

        // The host physical tape this callee tape draws from.
        let (phys, host_tape) = host
            .tapes
            .iter()
            .enumerate()
            .find(|(_, t)| t.name == *host_name)
            .ok_or(CompileError {
                span: arg.name_span,
                kind: CompileErrorKind::UnresolvedTapeTarget(host_name.clone()),
            })?;
        let host_glyphs = &expanded.alphabets[&host_tape.alphabet].glyphs;
        let callee_glyphs = &expanded.alphabets[&ct.alphabet].glyphs;

        let mut pairs = Vec::new();
        if let Some(m) = map {
            for p in &m.pairs {
                let src = glyph_index(host_glyphs, &p.src).ok_or(CompileError {
                    span: p.src.span(),
                    kind: CompileErrorKind::MapSymbolNotInAlphabet(glyph_label(&p.src)),
                })?;
                let dst = glyph_index(callee_glyphs, &p.dst).ok_or(CompileError {
                    span: p.dst.span(),
                    kind: CompileErrorKind::MapSymbolNotInAlphabet(glyph_label(&p.dst)),
                })?;
                pairs.push(IrMapPair {
                    src: src as u32,
                    dst: dst as u32,
                    one_way: p.arrow == MapArrow::ReadOnly,
                });
            }
        }
        binding.push(IrTapeBinding {
            caller_tape: phys as u32,
            pairs,
        });
    }
    Ok(binding)
}

/// The glyph label a symbol literal contributes (numeric literals label their
/// value's decimal string — a numeric glyph's identity is its value).
fn glyph_label(s: &SymLit) -> String {
    match s {
        SymLit::Glyph { value, .. } => value.clone(),
        SymLit::Number { value, .. } => value.to_string(),
    }
}

/// The index of a symbol literal's glyph within an alphabet, or `None`.
fn glyph_index(glyphs: &[String], s: &SymLit) -> Option<u16> {
    let label = glyph_label(s);
    glyphs.iter().position(|g| *g == label).map(|i| i as u16)
}

/// Warn on states unreachable from the world's entry, walking goto / call-
/// `then` / bind-`then` continuation edges (the state-graph analog of the
/// `.pmc` unreachable-code walk). The synthesized graft-hole trap rows carry
/// no outgoing edge; a state reached only to trap is still reached.
fn unreachable_state_warnings(world: &IrWorld, ew: &ExpandedWorld, warnings: &mut Vec<Diagnostic>) {
    let mut seen: HashSet<u32> = HashSet::new();
    let mut work = vec![world.entry];
    while let Some(id) = work.pop() {
        if !seen.insert(id) {
            continue;
        }
        for r in &world.states[id as usize].rules {
            match &r.transition {
                IrTransition::Goto { state } => work.push(*state),
                IrTransition::CallThen { then, .. } => {
                    if let IrThen::Goto { state } = then {
                        work.push(*state);
                    }
                }
                IrTransition::Return
                | IrTransition::Stop
                | IrTransition::Halt
                | IrTransition::TrapRead
                | IrTransition::TrapWrite => {}
            }
        }
    }
    for st in &world.states {
        if !seen.contains(&st.id) {
            warnings.push(Diagnostic {
                code: "unreachable-state",
                span: ew.states[st.id as usize].name_span,
                message: format!("state `{}` is unreachable in `{}`", st.name, world.name),
                fix: None,
            });
        }
    }
}

/// Warn on non-exported routines that no `call`/`bind` targets — the cheap
/// "referenced by any call" form of the spec's unused-routine warning (a bind
/// site lowers to a `CallThen`, so scanning IR call targets covers both).
fn unused_routine_warnings(
    program: &IrProgram,
    by_name: &HashMap<&str, &ResolvedWorld>,
    warnings: &mut Vec<Diagnostic>,
) {
    let mut referenced: HashSet<&str> = HashSet::new();
    for w in &program.worlds {
        for st in &w.states {
            for r in &st.rules {
                if let IrTransition::CallThen { target, .. } = &r.transition {
                    referenced.insert(target.as_str());
                }
            }
        }
    }
    for w in &program.worlds {
        if w.kind != IrWorldKind::Routine || !w.local {
            continue;
        }
        if referenced.contains(w.name.as_str()) {
            continue;
        }
        if let Some(rw) = by_name.get(w.name.as_str()) {
            warnings.push(Diagnostic {
                code: "unused-routine",
                span: rw.name_span,
                message: format!("routine `{}` is never called", w.name),
                fix: None,
            });
        }
    }
}

// ---------------------------------------------------------------------------
// Structural validation — the invariants every later stage may assume, the
// `.pmc` `validate_function` analog. Scoped to one world (per-world graphs).
// ---------------------------------------------------------------------------

/// Every world invariant codegen relies on: dense unique ids, an existing
/// entry, `arity`-wide rows, in-bounds indices, resolvable transition targets,
/// and traps only on synthesized rows. `dst` binding indices are checked at
/// lowering (they need the callee alphabet); here the caller side is checked.
pub fn validate_world(w: &IrWorld) -> Result<(), String> {
    let arity = w.arity as usize;
    if w.states.is_empty() {
        return Err(format!("{}: world has no states", w.name));
    }
    let mut ids = HashSet::new();
    for (i, st) in w.states.iter().enumerate() {
        if st.id as usize != i {
            return Err(format!(
                "{}: state ids are not dense in emission order (id {} at position {})",
                w.name, st.id, i
            ));
        }
        if !ids.insert(st.id) {
            return Err(format!("{}: duplicate state id {}", w.name, st.id));
        }
    }
    if w.entry as usize >= w.states.len() {
        return Err(format!(
            "{}: entry state {} is out of range",
            w.name, w.entry
        ));
    }
    let in_state = |t: u32| -> Result<(), String> {
        if (t as usize) < w.states.len() {
            Ok(())
        } else {
            Err(format!(
                "{}: transition targets missing state {}",
                w.name, t
            ))
        }
    };
    for st in &w.states {
        for r in &st.rules {
            if r.pattern.len() != arity {
                return Err(format!(
                    "{}: state {} has a width-{} pattern (arity {})",
                    w.name,
                    st.id,
                    r.pattern.len(),
                    arity
                ));
            }
            // Pattern is arity-wide (checked), so cell `i` matches tape `i`.
            for (i, c) in r.pattern.iter().enumerate() {
                if let IrCell::Index { index } = c {
                    check_index(w, i, *index, "pattern", st.id)?;
                }
            }
            if let Some(v) = &r.write {
                if v.len() != arity {
                    return Err(format!(
                        "{}: state {} has a width-{} write (arity {})",
                        w.name,
                        st.id,
                        v.len(),
                        arity
                    ));
                }
                for (i, c) in v.iter().enumerate() {
                    if let IrWrite::Index { index } = c {
                        check_index(w, i, *index, "write", st.id)?;
                    }
                }
            }
            if let Some(v) = &r.moves
                && v.len() != arity
            {
                return Err(format!(
                    "{}: state {} has a width-{} move (arity {})",
                    w.name,
                    st.id,
                    v.len(),
                    arity
                ));
            }
            let is_trap = matches!(
                r.transition,
                IrTransition::TrapRead | IrTransition::TrapWrite
            );
            if is_trap && !r.synthesized {
                return Err(format!(
                    "{}: state {} carries a trap on a non-synthesized row",
                    w.name, st.id
                ));
            }
            match &r.transition {
                IrTransition::Goto { state } => in_state(*state)?,
                IrTransition::CallThen { binding, then, .. } => {
                    if let IrThen::Goto { state } = then {
                        in_state(*state)?;
                    }
                    for tb in binding {
                        if tb.caller_tape as usize >= arity {
                            return Err(format!(
                                "{}: state {} binds caller tape {} (arity {})",
                                w.name, st.id, tb.caller_tape, arity
                            ));
                        }
                        let card = w.tapes[tb.caller_tape as usize].cardinality;
                        for p in &tb.pairs {
                            if p.src >= card {
                                return Err(format!(
                                    "{}: state {} binds src {} on tape {} (cardinality {})",
                                    w.name, st.id, p.src, tb.caller_tape, card
                                ));
                            }
                        }
                    }
                }
                IrTransition::Return
                | IrTransition::Stop
                | IrTransition::Halt
                | IrTransition::TrapRead
                | IrTransition::TrapWrite => {}
            }
        }
    }
    Ok(())
}

/// A cell's symbol index must fall inside its tape's alphabet (cell `col`
/// matches tape `col` — the width check guarantees the alignment).
fn check_index(w: &IrWorld, col: usize, index: u32, what: &str, state: u32) -> Result<(), String> {
    let card = w.tapes[col].cardinality;
    if index >= card {
        return Err(format!(
            "{}: state {} has a {what} index {} on tape {} (cardinality {})",
            w.name, state, index, col, card
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compiler::analyze;
    use crate::expand::expand;

    /// analyze → expand → lower, panicking on any front-end failure.
    fn lower_of(src: &str) -> (IrProgram, Vec<Diagnostic>) {
        let a = analyze(src).unwrap_or_else(|e| panic!("analyze failed: {e}"));
        let ex = expand(&a.resolved).unwrap_or_else(|e| panic!("expand failed: {e}"));
        lower(&ex, &a.resolved).unwrap_or_else(|e| panic!("lower failed: {e}"))
    }

    /// analyze → expand → lower, expecting the front end to pass and lowering
    /// to fail; returns the lowering `CompileError`.
    fn lower_err_of(src: &str) -> CompileError {
        let a = analyze(src).unwrap_or_else(|e| panic!("analyze failed: {e}"));
        let ex = expand(&a.resolved).unwrap_or_else(|e| panic!("expand failed: {e}"));
        lower(&ex, &a.resolved).expect_err("expected lowering to fail")
    }

    const A1: &str = "\
alphabet ab { '_', 'a', 'b' }
machine {
  tape main: ab;
  entry state scan {
    ['b'] -> write ['a'] move [>] goto scan;
    ['a'] ->            move [>] goto scan;
    ['_'] -> stop;
  }
}";

    const A5: &str = "\
alphabet bits { '_', '0', '1' }
alphabet wide { '_', 'a', 'b', '0', '1' }
namespace mylib {
  export routine plusOne(tape num: bits) {
    entry state inc {
      ['1'] -> write ['0'] move [<] goto inc;
      [*]   -> write ['1'] return;
    }
  }
}
use mylib::plusOne;
machine {
  tape ctl:  bits;
  tape data: wide;
  entry state main {
    ['1', *] -> call plusOne(num = data with map { '0'->'0', '1'->'1' }) then done;
    [*, *]   -> move [>, .] goto main;
  }
  state done { [*, *] -> stop; }
}";

    const A6: &str = "\
alphabet marks { '_', 'x', 'y', 'z' }
export graph findX(tape t: marks, state found, state missing) {
  entry state walk {
    ['x'] -> found;
    ['_'] -> missing;
    [*]   -> move [>] goto walk;
  }
}
machine {
  tape work: marks;
  entry graft findX(t = work, found = celebrate, missing = giveUp) as seek;
  state celebrate { [*] -> write ['_'] stop; }
  state giveUp    { [*] -> halt; }
}";

    fn world<'a>(ir: &'a IrProgram, name: &str) -> &'a IrWorld {
        ir.worlds.iter().find(|w| w.name == name).expect("world")
    }
    fn state<'a>(w: &'a IrWorld, name: &str) -> &'a IrState {
        w.states.iter().find(|s| s.name == name).expect("state")
    }

    #[test]
    fn json_round_trips_with_a_version() {
        let (ir, _) = lower_of(A1);
        let json = ir.to_json();
        assert_eq!(IrProgram::from_json(&json).unwrap(), ir);
        assert!(json.contains("\"version\": 1"), "{json}");
    }

    /// The serde tags are the frozen wire contract. Build one program that
    /// carries EVERY variant so a rename or retag is a visible break, not a
    /// silent format bump — and round-trip it.
    #[test]
    fn serde_tags_are_frozen() {
        let ir = IrProgram {
            version: TM_IR_VERSION,
            worlds: vec![
                IrWorld {
                    name: "main".into(),
                    kind: IrWorldKind::Machine,
                    arity: 2,
                    tapes: vec![
                        IrTape {
                            name: "a".into(),
                            alphabet: "al".into(),
                            cardinality: 3,
                        },
                        IrTape {
                            name: "b".into(),
                            alphabet: "al".into(),
                            cardinality: 3,
                        },
                    ],
                    entry: 0,
                    states: vec![IrState {
                        id: 0,
                        name: "s".into(),
                        line: 1,
                        rules: vec![
                            IrRule {
                                pattern: vec![IrCell::Index { index: 1 }, IrCell::Wildcard],
                                write: Some(vec![IrWrite::Keep, IrWrite::Index { index: 2 }]),
                                moves: Some(vec![IrMove::Left, IrMove::Right]),
                                debugger: true,
                                transition: IrTransition::CallThen {
                                    target: "r".into(),
                                    binding: vec![IrTapeBinding {
                                        caller_tape: 0,
                                        pairs: vec![IrMapPair {
                                            src: 1,
                                            dst: 1,
                                            one_way: true,
                                        }],
                                    }],
                                    then: IrThen::Goto { state: 0 },
                                },
                                synthesized: false,
                                line: 1,
                            },
                            IrRule {
                                pattern: vec![IrCell::Wildcard, IrCell::Wildcard],
                                write: None,
                                moves: Some(vec![IrMove::Stay, IrMove::Stay]),
                                debugger: false,
                                transition: IrTransition::TrapRead,
                                synthesized: true,
                                line: 2,
                            },
                        ],
                    }],
                    local: false,
                    line: 1,
                },
                IrWorld {
                    name: "r".into(),
                    kind: IrWorldKind::Routine,
                    arity: 1,
                    tapes: vec![IrTape {
                        name: "t".into(),
                        alphabet: "al".into(),
                        cardinality: 3,
                    }],
                    entry: 0,
                    states: vec![IrState {
                        id: 0,
                        name: "s".into(),
                        line: 1,
                        rules: vec![IrRule {
                            pattern: vec![IrCell::Wildcard],
                            write: None,
                            moves: None,
                            debugger: false,
                            transition: IrTransition::Return,
                            synthesized: false,
                            line: 1,
                        }],
                    }],
                    local: true,
                    line: 1,
                },
            ],
            entry_world: Some(0),
        };

        let json = ir.to_json();
        assert_eq!(IrProgram::from_json(&json).unwrap(), ir);
        for tag in [
            "\"kind\": \"machine\"",
            "\"kind\": \"routine\"",
            "\"kind\": \"index\"",
            "\"kind\": \"wildcard\"",
            "\"kind\": \"keep\"",
            "\"kind\": \"goto\"",
            "\"kind\": \"call_then\"",
            "\"kind\": \"return\"",
            "\"kind\": \"trap_read\"",
            "\"caller_tape\"",
            "\"one_way\": true",
            "\"synthesized\": true",
            "\"debugger\": true",
        ] {
            assert!(json.contains(tag), "missing tag {tag} in\n{json}");
        }
        // Move variants serialize as bare snake_case strings.
        assert!(json.contains("\"left\""), "{json}");
        assert!(json.contains("\"right\""), "{json}");
        assert!(json.contains("\"stay\""), "{json}");
    }

    #[test]
    fn a1_lowers_to_a_single_scanning_state() {
        let (ir, warnings) = lower_of(A1);
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(ir.entry_world, Some(0));
        let m = world(&ir, "main");
        assert_eq!(m.kind, IrWorldKind::Machine);
        assert_eq!(m.arity, 1);
        assert_eq!(m.states.len(), 1);
        let scan = &m.states[0];
        assert_eq!(m.entry, scan.id);
        assert_eq!(scan.name, "scan");
        assert_eq!(scan.rules.len(), 3);
        // ['b'] -> write ['a'] move [>] goto scan   (b=2, a=1 in ab)
        assert_eq!(scan.rules[0].pattern, vec![IrCell::Index { index: 2 }]);
        assert_eq!(scan.rules[0].write, Some(vec![IrWrite::Index { index: 1 }]));
        assert_eq!(scan.rules[0].moves, Some(vec![IrMove::Right]));
        assert_eq!(
            scan.rules[0].transition,
            IrTransition::Goto { state: scan.id }
        );
        // ['a'] -> move [>] goto scan   (no write → elided)
        assert_eq!(scan.rules[1].write, None);
        assert_eq!(scan.rules[1].moves, Some(vec![IrMove::Right]));
        // ['_'] -> stop   (no write, no move → both elided)
        assert_eq!(scan.rules[2].pattern, vec![IrCell::Index { index: 0 }]);
        assert_eq!(scan.rules[2].write, None);
        assert_eq!(scan.rules[2].moves, None);
        assert_eq!(scan.rules[2].transition, IrTransition::Stop);
        validate_world(m).unwrap();
    }

    #[test]
    fn a5_call_site_carries_the_resolved_binding_record() {
        let (ir, _) = lower_of(A5);
        // The routine lowers as its own world.
        let plus = world(&ir, "mylib::plusOne");
        assert_eq!(plus.kind, IrWorldKind::Routine);
        assert!(!plus.local, "plusOne is exported");
        assert_eq!(plus.arity, 1);
        // Its `[*] -> write ['1'] return` row returns.
        let inc = state(plus, "inc");
        assert!(
            inc.rules
                .iter()
                .any(|r| r.transition == IrTransition::Return)
        );

        let m = world(&ir, "main");
        let main = state(m, "main");
        // The call row: call plusOne(num = data with map {'0'->'0','1'->'1'}) then done
        let call = main
            .rules
            .iter()
            .find_map(|r| match &r.transition {
                IrTransition::CallThen {
                    target,
                    binding,
                    then,
                } => Some((target.clone(), binding.clone(), *then)),
                _ => None,
            })
            .expect("a call row");
        assert_eq!(call.0, "mylib::plusOne");
        // done is a state in main; the then resumes there.
        let done = state(m, "done");
        assert_eq!(call.2, IrThen::Goto { state: done.id });
        // binding[0] binds callee tape 0 (num) to host tape 1 (data, wide).
        // wide = _,a,b,0,1 → '0'=3,'1'=4 ; bits = _,0,1 → '0'=1,'1'=2.
        assert_eq!(call.1.len(), 1);
        assert_eq!(call.1[0].caller_tape, 1);
        assert_eq!(
            call.1[0].pairs,
            vec![
                IrMapPair {
                    src: 3,
                    dst: 1,
                    one_way: false
                },
                IrMapPair {
                    src: 4,
                    dst: 2,
                    one_way: false
                },
            ]
        );
        validate_world(m).unwrap();
        validate_world(plus).unwrap();
    }

    /// A call that binds tapes into an EXTERNAL routine (imported, no local
    /// definition) cannot be lowered — the binding rewrite needs the callee's
    /// tape signature, which lives in another compilation unit. The reviewer's
    /// exact repro; the compiler reports a clear error, never panicking. Both
    /// the with-map and the bindless (`num = t`) forms of the tape binding
    /// trigger it — the binding operand needs the signature either way.
    #[test]
    fn external_call_binding_tapes_is_a_clear_error_not_a_panic() {
        // With a `with map { … }`.
        let with_map = "\
alphabet ab { '_', 'a', 'b' }
use mylib::plusOne;
machine {
  tape t: ab;
  entry state main {
    ['a'] -> call plusOne(num = t with map { 'a'->'b' }) then done;
    [*]   -> stop;
  }
  state done { [*] -> stop; }
}";
        let e = lower_err_of(with_map);
        assert_eq!(e.kind.code(), "external-binding-unsupported");
        assert!(
            matches!(&e.kind, CompileErrorKind::ExternalBindingUnsupported(n) if n == "mylib::plusOne"),
            "{:?}",
            e.kind
        );

        // Bindless (`num = t`, no map) triggers it too — still a tape binding.
        let bindless = "\
alphabet ab { '_', 'a', 'b' }
use mylib::plusOne;
machine {
  tape t: ab;
  entry state main {
    ['a'] -> call plusOne(num = t) then done;
    [*]   -> stop;
  }
  state done { [*] -> stop; }
}";
        assert_eq!(
            lower_err_of(bindless).kind.code(),
            "external-binding-unsupported"
        );
    }

    /// The bind-sugar path reaches the same lowering as a direct call, so an
    /// external bind that binds tapes is the same clear error — with-map and
    /// bindless alike.
    #[test]
    fn external_bind_sugar_binding_tapes_is_a_clear_error() {
        let with_map = "\
alphabet ab { '_', 'a', 'b' }
use mylib::plusOne;
machine {
  tape t: ab;
  bind plusOne(num = t with map { 'a'->'b' }) as h;
  entry state main { [*] -> call h() then done; }
  state done { [*] -> stop; }
}";
        let e = lower_err_of(with_map);
        assert_eq!(e.kind.code(), "external-binding-unsupported");
        assert!(
            matches!(&e.kind, CompileErrorKind::ExternalBindingUnsupported(n) if n == "mylib::plusOne"),
            "{:?}",
            e.kind
        );

        let bindless = "\
alphabet ab { '_', 'a', 'b' }
use mylib::plusOne;
machine {
  tape t: ab;
  bind plusOne(num = t) as h;
  entry state main { [*] -> call h() then done; }
  state done { [*] -> stop; }
}";
        assert_eq!(
            lower_err_of(bindless).kind.code(),
            "external-binding-unsupported"
        );
    }

    /// A PLAIN external call — no binding args — still lowers: it becomes a
    /// `CallThen` with an empty binding the LINKER resolves across objects.
    #[test]
    fn plain_external_call_still_lowers() {
        let src = "\
alphabet ab { '_', 'a' }
use lib::ext;
machine {
  tape t: ab;
  entry state go { [*] -> call ext() then done; }
  state done { [*] -> stop; }
}";
        let (ir, _) = lower_of(src);
        let m = world(&ir, "main");
        let go = state(m, "go");
        let call = go
            .rules
            .iter()
            .find_map(|r| match &r.transition {
                IrTransition::CallThen {
                    target, binding, ..
                } => Some((target.clone(), binding.clone())),
                _ => None,
            })
            .expect("a call row");
        assert_eq!(call.0, "lib::ext");
        assert!(call.1.is_empty(), "a plain call carries no binding");
    }

    /// A routine that hands control to one of its own `state` parameters
    /// (`goto <state-param>`) is a T4-valid definition, but lowering it on its
    /// own needs the composition engine to thread the continuation from the
    /// call site. It reports the honest not-yet-supported error, not the
    /// misleading `undefined-state` (`k` IS a declared parameter).
    #[test]
    fn routine_goto_state_param_is_a_clear_error() {
        let src = "\
alphabet ab { '_', 'a' }
routine r(tape t: ab, state k) {
  entry state s { [*] -> goto k; }
}
machine {
  tape t: ab;
  entry state go { [*] -> stop; }
}";
        let e = lower_err_of(src);
        assert_eq!(e.kind.code(), "state-param-continuation-unsupported");
        assert!(
            matches!(&e.kind, CompileErrorKind::StateParamContinuationUnsupported(n) if n == "k"),
            "{:?}",
            e.kind
        );
    }

    #[test]
    fn a6_graft_splices_states_with_the_instance_entry() {
        let (ir, _) = lower_of(A6);
        // Only the machine is emitted (the graph is spliced away).
        assert_eq!(ir.worlds.len(), 1);
        let m = world(&ir, "main");
        // Own states plus the spliced graft instance `seek` (findX::walk).
        let names: Vec<&str> = m.states.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"celebrate"), "{names:?}");
        assert!(names.contains(&"giveUp"), "{names:?}");
        assert!(names.contains(&"seek"), "{names:?}");
        // The entry graft names the world entry — the instance's entry state.
        assert_eq!(m.states[m.entry as usize].name, "seek");
        // The spliced entry walks: ['x'] -> celebrate, ['_'] -> giveUp, [*] -> >.
        let seek = &m.states[m.entry as usize];
        let cel = state(m, "celebrate");
        let give = state(m, "giveUp");
        assert!(
            seek.rules
                .iter()
                .any(|r| r.transition == IrTransition::Goto { state: cel.id })
        );
        assert!(
            seek.rules
                .iter()
                .any(|r| r.transition == IrTransition::Goto { state: give.id })
        );
        validate_world(m).unwrap();
    }

    #[test]
    fn unreachable_state_warns() {
        // `orphan` is reachable from nothing.
        let src = "\
alphabet ab { '_', 'a' }
machine {
  tape t: ab;
  entry state go { [*] -> stop; }
  state orphan { [*] -> halt; }
}";
        let (_, warnings) = lower_of(src);
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert_eq!(warnings[0].code, "unreachable-state");
        assert!(warnings[0].message.contains("orphan"));
    }

    #[test]
    fn unused_routine_warns_only_when_unexported_and_uncalled() {
        // A local routine nobody calls warns; an exported one does not.
        let src = "\
alphabet ab { '_', 'a' }
routine helper(tape t: ab) { entry state s { [*] -> return; } }
export routine api(tape t: ab) { entry state s { [*] -> return; } }
machine {
  tape t: ab;
  entry state go { [*] -> stop; }
}";
        let (_, warnings) = lower_of(src);
        let unused: Vec<&str> = warnings
            .iter()
            .filter(|d| d.code == "unused-routine")
            .map(|d| d.message.as_str())
            .collect();
        assert_eq!(unused.len(), 1, "{warnings:?}");
        assert!(unused[0].contains("helper"), "{unused:?}");
    }

    #[test]
    fn to_mermaid_renders_a_state_graph() {
        let (ir, _) = lower_of(A1);
        let mer = world(&ir, "main").to_mermaid();
        assert!(mer.starts_with("flowchart TD\n"), "{mer}");
        assert!(mer.contains("S0[\"scan\"]"), "{mer}");
        assert!(mer.contains("-->|"), "{mer}");
        // The `stop` row routes to the shared terminal node.
        assert!(mer.contains("T_stp"), "{mer}");
    }

    #[test]
    fn validate_world_rejects_dangling_and_bad_width() {
        let (ir, _) = lower_of(A1);
        let m = world(&ir, "main");
        validate_world(m).unwrap();

        let mut dangling = m.clone();
        dangling.states[0].rules[0].transition = IrTransition::Goto { state: 99 };
        assert!(validate_world(&dangling).is_err());

        let mut wide = m.clone();
        wide.states[0].rules[0].pattern.push(IrCell::Wildcard);
        assert!(validate_world(&wide).is_err());

        // ab has cardinality 3 — index 3 is out of its tape's alphabet.
        let mut oob = m.clone();
        oob.states[0].rules[0].pattern[0] = IrCell::Index { index: 3 };
        assert!(validate_world(&oob).is_err());
    }

    #[test]
    fn validate_world_rejects_trap_on_non_synthesized_row() {
        let (ir, _) = lower_of(A1);
        let mut m = world(&ir, "main").clone();
        m.states[0].rules[0].transition = IrTransition::TrapRead;
        m.states[0].rules[0].synthesized = false;
        assert!(validate_world(&m).is_err());
    }
}
