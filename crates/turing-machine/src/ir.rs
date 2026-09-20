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
//! name spans). Codegen consumes the IR; `tmt ir graph` renders
//! [`IrWorld::to_mermaid`].
//!
//! # The versioned contract ([`TM_IR_VERSION`])
//!
//! Rows and action vectors are **index-only**: a pattern or write cell carries
//! a symbol INDEX, never a glyph — the processor never sees glyphs, and the
//! spec's match rows are index-resolved. Per-tape *alphabet names* and
//! cardinalities ride along for readability (`tmt ir`) and index-bound
//! validation; since v4 each [`IrTape`] also carries its own glyph table
//! ([`IrTape::glyphs`]) and, for a contracted signature tape, its declared
//! effective write set as glyphs ([`IrTape::writes`]) — both presentation
//! data resolved against THIS tape's alphabet, not a second source of truth
//! for indices already fixed elsewhere in the document.
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
//! `.tma` binding-call operand carries (codegen renders it), with
//! `caller_tape` the host physical tape index and each pair's `src` the
//! AUTHORED symbol resolved to the caller's own alphabet index. An in-unit
//! callee's `dst` resolves the same way, against the callee's alphabet; an
//! out-of-unit callee's `dst` travels as the authored glyph's LABEL instead —
//! its index space belongs to the linker, which resolves it against the
//! callee's real interface at link time (docs/formats.md (bound calls)). No
//! blank pin or closure is applied here — the composition engine does that
//! at link.
//!
//! `compile()` wires the lowering + `validate_world` into the pipeline and
//! codegen consumes the output; the JSON round-trip (`to_json`/`from_json`)
//! and `to_mermaid` render surfaces are wired to the `tmt ir` / `--emit-ir`
//! CLI (`cli::inspect` / `cli::build`).

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

use mtc_core::diagnostics::{Diagnostic, Span};

use crate::compiler::{CompileError, CompileErrorKind, Resolved, ResolvedWorld, WorldKind};
use crate::declarations::Declarations;
use crate::expand::{
    Cell, Expanded, ExpandedRule, ExpandedTape, ExpandedWorld, Transition2, WriteOut,
};
use crate::footprint::FootprintTable;
use crate::parser::{BindingArg, BindingValue, Continuation, MapArrow, MoveDir, SymLit};

/// The TM IR encoding version. Bumps on any change to the serialized shape
/// (field names, serde tags). Embedded in every [`IrProgram`] and pinned by a
/// round-trip test, the `.pmc` `IR_VERSION` discipline.
///
/// Version 2 adds the two optimizer-shape fields: the [`IrTransition::TailCall`]
/// terminal (the `tail_call` pass's output) and the [`IrState::dispatch`] hint
/// (the `dispatch_select` pass's output). Both are internal — no released
/// artifact carries them — but the version contract moves with ANY serialized
/// shape change regardless.
///
/// Version 3 adds the [`IrRule::direct`] lowering hint (the `jump_threading`
/// pass's output). Internal — no released artifact carries it.
///
/// Version 4 adds the binding-arc vocabulary: [`IrTape::glyphs`] and
/// [`IrTape::writes`] (a contracted signature tape's effective write set),
/// [`IrWorld::exits`] and [`IrWorld::returns`] (a routine's declared exit
/// count and whether it can resume normally), the
/// [`IrTransition::ReturnExit`] terminal, [`IrTransition::CallThen`]'s
/// `exits` field, [`IrTapeBinding::param`] (a symbolic binding entry), and
/// [`IrMapPair::dst`]'s widening from a bare index to [`IrMapDst`] (a
/// glyph-labelled pair against an out-of-unit callee). A call/bind into a
/// routine outside this compilation unit is the first (and, at this
/// version, the only) producer of a `param`-bearing entry and a `Label`
/// dst (`ir::resolve_binding`); every in-unit entry still lowers to the
/// positional, index-only shape, so a plain `-O0` document with no
/// cross-unit bound call has no visible change but `glyphs` and the
/// version digit.
///
/// Version 5 adds [`IrTapeBinding::map_written`] — the wire's own
/// `TapeBinding.map_written` distinction (`docs/formats.md` (bound calls)),
/// carried by neither v4 field: an OMITTED map (`pairs` empty,
/// `map_written` false) is index identity and leaves the linker's
/// `glyph-mismatch` guard live; a WRITTEN empty map (`with map { }`,
/// `pairs` empty, `map_written` true) silences it on purpose. Applies to
/// both a positional (in-unit) and a symbolic (out-of-unit) entry alike —
/// the distinction predates `param` and was simply never expressible
/// before this version.
pub const TM_IR_VERSION: u32 = 5;

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
    /// The `.routine`'s declared exit count (its `exits=` clause) — `0` when
    /// none is declared. No pass produces a nonzero value yet.
    #[serde(default, skip_serializing_if = "is_zero_u8")]
    pub exits: u8,
    /// Whether the world can resume normally at a call site's `then` —
    /// `false` only for a `noreturn` routine. `true` (the only state a v3
    /// document ever meant) is the fill/deserialization default, so absence
    /// on the wire reads as "returns", never as "noreturn".
    #[serde(default = "default_true", skip_serializing_if = "is_true")]
    pub returns: bool,
}

fn default_true() -> bool {
    true
}

fn is_true(b: &bool) -> bool {
    *b
}

fn is_zero_u8(n: &u8) -> bool {
    *n == 0
}

/// A world kind that survives to the IR (graphs are gone).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IrWorldKind {
    Machine,
    Routine,
}

/// A tape's position, name, and the index bound its symbols must respect. The
/// `alphabet` name is presentation only (readability of `tmt ir`); match rows
/// and action vectors stay index-only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IrTape {
    pub name: String,
    pub alphabet: String,
    pub cardinality: u32,
    /// `true` for a `volatile tape` — the band is a device, not addressable
    /// memory (docs/tmt/language.md (volatile tapes)).
    #[serde(default, skip_serializing_if = "is_false")]
    pub volatile: bool,
    /// This tape's glyph table, one entry per index — `glyphs.len() ==
    /// cardinality` on any document lowering itself produces. A document
    /// deserialized from a pre-v4 wire form carries this field empty (the
    /// glyph tables lived only in the presentation layers before v4), which
    /// is a legitimate reading of old data, not a violated invariant.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub glyphs: Vec<String>,
    /// A ROUTINE tape's published write set, as glyphs
    /// (`compiler::published_writes`, the one function both this lowering
    /// and the source arm of `tmt interface` call): the declared EFFECTIVE
    /// set (`writes` minus `preserves`) for a contracted signature tape,
    /// or — when the parameter declares NEITHER clause — the tape's
    /// INFERRED write set (`footprint::infer_resolved_with`, the same
    /// sound-upper-bound analysis `check_contracts` runs to validate a
    /// declared contract). The wire has no spelling for "no restriction
    /// declared" (an absent `writes=` decodes as "writes nothing" —
    /// docs/formats.md (routine interfaces)), so an uncontracted routine
    /// tape must still publish what it actually writes rather than an
    /// empty set that would understate it. `preserves` itself has no IR
    /// representation either way: it is source-level sugar the effective
    /// set already absorbs.
    ///
    /// Always `None` on a MACHINE tape — the one case that reaches this
    /// field today: `main` is never a callable, composable routine another
    /// unit binds against, its own interface entry is never read (the
    /// object arm's printer skips the entry world outright), and computing
    /// an inferred set nothing looks at would only cost every
    /// `machine`-bearing program a new `.param writes=` line and move its
    /// compiled bytes for no observable gain.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub writes: Option<Vec<String>>,
}

/// One state: an id, its source name (synthetic for graft-instance internals),
/// its rules in priority (row) order, and the codegen dispatch hint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IrState {
    pub id: u32,
    pub name: String,
    /// Source line of the state's declaration; `0` if unknown.
    pub line: u32,
    pub rules: Vec<IrRule>,
    /// How codegen should lower this state's match: the [`IrDispatch::Table`]
    /// canon, or the [`IrDispatch::Branch`] two-row form the `dispatch_select`
    /// pass selects. Defaults to `Table` (so pre-hint IR and the `-O0` canon
    /// deserialize unchanged); `validate_world` ignores it beyond its shape.
    #[serde(default)]
    pub dispatch: IrDispatch,
}

/// The codegen lowering hint for a state's match. `Table` is the canonical
/// `-O0` form (`rd; mtc T<n>; djmp D<n>` over a match/dispatch table pair);
/// `Branch` is the two-row form (`dispatch_select`'s output). Branch semantics
/// — a one-row table plus a `jm`/fall-through — land with the `dispatch_select`
/// pass and its codegen arm; here the enum only reserves the shape so the
/// single IR version bump carries it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IrDispatch {
    #[default]
    Table,
    Branch,
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
    /// Lowering hint (the `jump_threading` pass's output — never set by
    /// lowering): this bare rule's dispatch entry names its `Goto`
    /// destination state directly and no stub block is emitted
    /// (docs/tmt/optimizer.md (dispatch-target threading)).
    #[serde(default, skip_serializing_if = "is_false")]
    pub direct: bool,
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
        /// The `exits=(…)` operand: same-world state ids the callee's
        /// declared exits (`IrWorld::exits`) resume at, in exit order —
        /// `ReturnExit { exit: k }` in the callee resumes at `exits[k]`
        /// instead of at `then`. Empty for a call whose callee declares no
        /// exits (the only shape any pass produces today).
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        exits: Vec<u32>,
        then: IrThen,
    },
    Return,
    /// A return through one of the callee's declared exits (`retx #k`) —
    /// resumes at the call site's `exits[k]` instead of at `then`. Never
    /// produced by lowering yet: no routine declares `exits=` today.
    ReturnExit {
        exit: u32,
    },
    Stop,
    Halt,
    /// A tail call to a routine — `jmp @<target>` (the `tail_call` pass's
    /// output; codegen emits the relocated external jump). Control transfers
    /// with no return trip, so there is no `then`: the callee's own `return`
    /// pops the frame the ORIGINAL caller pushed. Never produced by lowering —
    /// only by the optimizer — and only for a BINDLESS call (a bound call's
    /// frame discipline forbids it; see the `tail_call` module doc).
    TailCall {
        /// Mangled callee routine name.
        target: String,
    },
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
/// the authored symbol map between their alphabets. An in-unit callee's
/// entry is POSITIONAL (`param: None`, its position in the vector IS the
/// callee's tape index) and every pair's `dst` resolves to a concrete
/// callee-alphabet index; an out-of-unit callee's entry is NAMED (`param:
/// Some`, the callee's parameter name) and every pair's `dst` travels as a
/// glyph LABEL, because the callee's own tape order and index space belong
/// to the LINKER to resolve, not this unit (docs/formats.md (bound calls)).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IrTapeBinding {
    /// Host physical tape index (< 16) — always numeric: it names the
    /// CALLER's own band, which this unit declares either way.
    pub caller_tape: u32,
    /// `(src, dst, one_way)` per authored pair, in source order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pairs: Vec<IrMapPair>,
    /// The callee's parameter NAME, for a symbolic (out-of-unit) entry;
    /// `None` for a positional (in-unit) one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub param: Option<String>,
    /// Whether a `with map` was authored at all — the wire's own
    /// distinction between an OMITTED map (`pairs` empty, this `false`:
    /// index identity, the linker's `glyph-mismatch` guard stays live) and
    /// a WRITTEN empty map (`with map { }`; `pairs` empty, this `true`:
    /// "bind by index, deliberately," which silences that guard on
    /// purpose). A map that carries pairs is written by definition, so
    /// this is only load-bearing when `pairs` is empty
    /// (docs/formats.md (bound calls)).
    #[serde(default, skip_serializing_if = "is_false")]
    pub map_written: bool,
}

/// One `src -> dst` (or `src => dst`, `one_way`) symbol-map pair: `src` a
/// caller-alphabet index, `dst` the callee-alphabet destination.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IrMapPair {
    pub src: u32,
    pub dst: IrMapDst,
    #[serde(default, skip_serializing_if = "is_false")]
    pub one_way: bool,
}

/// A map pair's destination: a callee symbol INDEX when the callee is in this
/// compilation unit, or its glyph LABEL when it is not and the linker must
/// resolve it against the callee's interface (docs/formats.md (bound calls)).
/// `untagged`: a numeric `dst` reads as `Index`, a string `dst` as `Label` —
/// the two JSON shapes never overlap, so this stays unambiguous both ways
/// (pinned by the v3-shaped round-trip test, which feeds a bare `"dst": 1`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum IrMapDst {
    Index(u32),
    Label(String),
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
        // `ReturnExit` terminals are per-exit, so they cannot share the
        // `&'static str` pool above; one shared node per exit number instead.
        let mut ret_exit_terms: HashSet<u32> = HashSet::new();
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
                    IrTransition::TailCall { target } => {
                        // A tail call transfers out of this world for good (no
                        // return trip), so it routes to a shared terminal node,
                        // its label naming the callee.
                        declare(&mut out, "T_tail", "tail", &mut terms);
                        let _ = writeln!(
                            edges,
                            "    S{} -->|\"{label} tail {}\"| T_tail",
                            st.id,
                            escape(target)
                        );
                    }
                    IrTransition::Return => {
                        declare(&mut out, "T_ret", "ret", &mut terms);
                        let _ = writeln!(edges, "    S{} -->|\"{label}\"| T_ret", st.id);
                    }
                    IrTransition::ReturnExit { exit } => {
                        if ret_exit_terms.insert(*exit) {
                            let _ = writeln!(out, "    T_ret{exit}((\"ret #{exit}\"))");
                        }
                        let _ = writeln!(edges, "    S{} -->|\"{label}\"| T_ret{exit}", st.id);
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
///
/// `externals` feeds the write-footprint inference an uncontracted routine
/// tape falls back to (see [`IrTape::writes`]) — the same declarations a
/// compile's own `check_contracts` believes, so an uncontracted call into
/// the standard library is credited its declared effective set rather than
/// the whole alphabet. It is also what a cross-unit `call`/`bind` site's own
/// binding resolution consults (`resolve_binding`): a callee whose
/// declarations are here gets its tape order and parameter names checked at
/// compile time, exactly as a local signature's are.
pub(crate) fn lower(
    expanded: &Expanded,
    resolved: &Resolved,
    externals: &Declarations,
) -> Result<(IrProgram, Vec<Diagnostic>), CompileError> {
    let mut warnings = Vec::new();

    // Correlate emitted worlds with their resolved originals by mangled name.
    let by_name: HashMap<&str, &ResolvedWorld> = resolved
        .worlds
        .iter()
        .map(|w| (w.name.as_str(), w))
        .collect();

    // Computed unconditionally (unlike `check_contracts`'s own "an
    // uncontracted module pays nothing" gate): a routine's write set has to
    // be published whether or not ANY world in the module is contracted,
    // because the interface section is all-or-none per compiled object —
    // one record per blob, not one only where a contract happens to exist.
    let footprint = crate::footprint::infer_resolved_with(resolved, &externals.modules());

    let mut worlds = Vec::with_capacity(expanded.worlds.len());
    for ew in &expanded.worlds {
        let rw = by_name.get(ew.name.as_str()).copied();
        worlds.push(lower_world(
            ew,
            rw,
            expanded,
            resolved,
            externals,
            &footprint,
            &mut warnings,
        )?);
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
    externals: &Declarations,
    footprint: &FootprintTable,
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
            rules.push(lower_rule(
                r,
                ew,
                &name_to_id,
                expanded,
                resolved,
                externals,
            )?);
        }
        states.push(IrState {
            id: i as u32,
            name: s.name.clone(),
            line: s.name_span.start.line,
            rules,
            // Lowering always emits the canonical table form; `dispatch_select`
            // may switch a state to `Branch` later in the pipeline.
            dispatch: IrDispatch::Table,
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
            .enumerate()
            .map(|(i, t)| {
                let glyphs = expanded.alphabets[&t.alphabet].glyphs.clone();
                // `rw.tapes` and `ew.tapes` are both vector-position order
                // over the same signature (a machine's tape decls, or a
                // routine's tape params), so index `i` names the same tape
                // in both.
                let writes = rw.and_then(|w| {
                    let rt = w.tapes.get(i)?;
                    // Machine worlds are excluded — nothing reads `main`'s
                    // own interface entry (the object arm's printer skips
                    // the entry world outright — docs/tmt/cli.md
                    // (interface)) — so this deliberately withholds an
                    // inferred set rather than computing and discarding
                    // one, leaving that path's codegen and every
                    // `machine`-bearing golden alone.
                    if w.kind != WorldKind::Routine {
                        return None;
                    }
                    let inferred = footprint
                        .worlds
                        .get(&w.name)
                        .and_then(|wf| wf.tapes.get(i).copied());
                    let indices = crate::compiler::published_writes(rt, inferred);
                    Some(
                        indices
                            .iter()
                            .filter_map(|index| glyphs.get(index as usize).cloned())
                            .collect(),
                    )
                });
                IrTape {
                    name: t.name.clone(),
                    alphabet: t.alphabet.clone(),
                    cardinality: t.cardinality as u32,
                    volatile: t.volatile,
                    glyphs,
                    writes,
                }
            })
            .collect(),
        entry,
        states,
        local: rw.map(|w| w.local).unwrap_or(false),
        line: rw.map(|w| w.name_span.start.line).unwrap_or(0),
        // No routine declares `exits=`/`noreturn` yet — every world lowers
        // with no exits and normal-return semantics.
        exits: 0,
        returns: true,
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
    externals: &Declarations,
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
            let binding =
                resolve_binding(ew, target, args, *external, expanded, externals, r.span)?;
            (
                IrTransition::CallThen {
                    target: target.clone(),
                    binding,
                    // No `.routine` declares `exits=` yet, so no call site
                    // ever resolves a multi-exit resume table.
                    exits: Vec::new(),
                    then: then_of(then)?,
                },
                false,
            )
        }
        Transition2::BindCall { name, then } => {
            // A bind is pure sugar: look up its routine + args in the world's
            // resolved bind table and lower to the same CallThen a direct call
            // would produce (dedup keys on (routine, binding) regardless).
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
                externals,
                r.span,
            )?;
            (
                IrTransition::CallThen {
                    target: bind.target.clone(),
                    binding,
                    exits: Vec::new(),
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
        // Never set by lowering — only the `jump_threading` optimizer pass
        // sets this hint.
        direct: false,
        line: r.span.start.line,
    })
}

/// The host physical tape a binding arg's target names — the CALLER-side
/// lookup shared by the in-unit and out-of-unit paths below, since the
/// caller's own tapes are this unit's to know either way.
fn host_tape_of<'a>(
    host: &'a ExpandedWorld,
    host_name: &str,
    name_span: Span,
) -> Result<(usize, &'a ExpandedTape), CompileError> {
    host.tapes
        .iter()
        .enumerate()
        .find(|(_, t)| t.name == host_name)
        .ok_or(CompileError {
            span: name_span,
            kind: CompileErrorKind::UnresolvedTapeTarget(host_name.to_string()),
        })
}

/// Order a binding call's named args by the callee's own tape-parameter
/// order — `callee_tape_names` in signature order — erroring on any
/// parameter left unbound. The in-unit loop's own walk, generalized so an
/// out-of-unit callee's DECLARED order (when its declarations are known)
/// drives the identical check (docs/formats.md (bound calls)).
fn order_by_callee_tapes<'a>(
    callee_tape_names: impl Iterator<Item = &'a str>,
    named_args: &[&'a BindingArg],
    site: Span,
) -> Result<Vec<&'a BindingArg>, CompileError> {
    let mut order = Vec::new();
    for name in callee_tape_names {
        let Some(arg) = named_args.iter().find(|a| a.name == name) else {
            // Every callee tape must be bound (T4's arity check locally;
            // the declared signature's own arity for a known external one).
            return Err(CompileError {
                span: site,
                kind: CompileErrorKind::MissingArg(name.to_string()),
            });
        };
        order.push(*arg);
    }
    Ok(order)
}

/// Resolve a call/bind site's source-form binding args to the per-callee-tape
/// binding-call record.
///
/// An IN-UNIT callee's record is POSITIONAL: `binding[k]` binds the callee's
/// tape `k` (`expanded.worlds` carries its signature), `src` resolves
/// against the host (caller) tape alphabet, and `dst` resolves against the
/// callee tape alphabet — the same direction the graft splice uses.
///
/// An OUT-OF-UNIT callee's tape order, parameter names and glyph indices are
/// the LINKER's to resolve, not this unit's — so its record is SYMBOLIC:
/// each entry carries the callee's parameter NAME (`param`) in place of a
/// position, and each pair's `dst` carries the authored glyph's LABEL in
/// place of an index. `caller_tape` and `src` stay numeric in both forms:
/// both name THIS unit's own bands and alphabet. When the callee's
/// declarations are known (`externals`), its tape order and parameter names
/// are checked here, exactly as a local signature's are; when they are not,
/// the site keeps its source order and every entry is named, which is
/// exactly what the linker's own `reorder_named` exists to fix up once the
/// callee's real signature is known.
fn resolve_binding(
    host: &ExpandedWorld,
    target: &str,
    args: &[BindingArg],
    external: bool,
    expanded: &Expanded,
    externals: &Declarations,
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

    if external {
        let callee_decl = externals.routine(target);
        let order: Vec<&BindingArg> = match callee_decl {
            Some(sig) => {
                // Every named arg must name a real declared parameter of
                // the callee — tape or state — and no name may repeat,
                // checked here because `order_by_callee_tapes` below only
                // walks the FORWARD direction (a tape param with no
                // matching arg): an arg naming nothing the callee declares,
                // or naming the same parameter twice, would otherwise be
                // silently dropped or silently overwritten rather than
                // reported — the silent-failure shape this arc exists to
                // raise. A legitimate state-param arg is excluded from the
                // unknown-name check (it never matches a tape name and is
                // not this branch's to bind), not flagged.
                let mut seen: HashSet<&str> = HashSet::new();
                for a in &named_args {
                    if !seen.insert(a.name.as_str()) {
                        return Err(CompileError {
                            span: a.name_span,
                            kind: CompileErrorKind::DuplicateArg(a.name.clone()),
                        });
                    }
                    if !sig.tapes.iter().any(|t| t.name == a.name)
                        && !sig.state_params.iter().any(|p| p == &a.name)
                    {
                        return Err(CompileError {
                            span: a.name_span,
                            kind: CompileErrorKind::UnknownArg(a.name.clone()),
                        });
                    }
                }
                order_by_callee_tapes(sig.tapes.iter().map(|t| t.name.as_str()), &named_args, site)?
            }
            // No declarations for this callee: keep the site's own source
            // order and name every entry — the linker's `reorder_named`
            // fixes the order up once it has the callee's real signature.
            None => named_args.clone(),
        };

        let mut binding = Vec::with_capacity(order.len());
        for arg in order {
            let BindingValue::Named {
                target: host_name,
                map,
                ..
            } = &arg.value
            else {
                unreachable!("named_args are Named by construction");
            };
            let (phys, host_tape) = host_tape_of(host, host_name, arg.name_span)?;
            let host_glyphs = &expanded.alphabets[&host_tape.alphabet].glyphs;

            // An OMITTED map (`map: None`) emits NO pairs and clears
            // `map_written` — index identity, and the linker's
            // glyph-mismatch warning stands guard. Expanding it into
            // identity pairs here would silence that warning, which is
            // exactly the silent-failure shape the arc exists to raise. A
            // WRITTEN empty map (`with map { }`, `map: Some(SymMap{pairs:
            // vec![], ..})`) sets `map_written` even though `pairs` stays
            // empty — the wire's own distinction (`IrTapeBinding::
            // map_written`), which is what silences the warning on
            // purpose: an omitted and a written-empty map would otherwise
            // be bit-for-bit identical here.
            let mut pairs = Vec::new();
            if let Some(m) = map {
                for p in &m.pairs {
                    // `src` resolves against the CALLER's alphabet, which is
                    // this unit's; `dst` travels as a label — the callee's
                    // own index space is the linker's to resolve.
                    let src = glyph_index(host_glyphs, &p.src).ok_or(CompileError {
                        span: p.src.span(),
                        kind: CompileErrorKind::MapSymbolNotInAlphabet(glyph_label(&p.src)),
                    })?;
                    pairs.push(IrMapPair {
                        src: src as u32,
                        dst: IrMapDst::Label(glyph_label(&p.dst)),
                        one_way: p.arrow == MapArrow::ReadOnly,
                    });
                }
            }
            binding.push(IrTapeBinding {
                param: Some(arg.name.clone()),
                caller_tape: phys as u32,
                pairs,
                map_written: map.is_some(),
            });
        }
        // Entries are named or positional, never mixed in one list (a
        // partially-resolved callee is exactly the bug shape) — every entry
        // this branch builds carries `param`, by construction of the loop
        // above; assert it rather than trust the construction silently.
        debug_assert!(
            binding.iter().all(|b| b.param.is_some()),
            "an out-of-unit binding entry is always named"
        );
        return Ok(binding);
    }

    // In-unit ⇒ the callee is one of the module's emitted worlds (`expand`
    // emits the machine and every routine, reachable or not).
    let callee = expanded
        .worlds
        .iter()
        .find(|w| w.name == target)
        .expect("a non-external callee is one of the module's emitted worlds");

    let order = order_by_callee_tapes(
        callee.tapes.iter().map(|t| t.name.as_str()),
        &named_args,
        site,
    )?;

    let mut binding = Vec::with_capacity(order.len());
    for arg in order {
        let BindingValue::Named {
            target: host_name,
            map,
            ..
        } = &arg.value
        else {
            unreachable!("named_args are Named by construction");
        };
        let ct = callee
            .tapes
            .iter()
            .find(|t| t.name == arg.name)
            .expect("order_by_callee_tapes only returns args matching a callee tape name");

        // The host physical tape this callee tape draws from.
        let (phys, host_tape) = host_tape_of(host, host_name, arg.name_span)?;
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
                    // The callee is always in this compilation unit here
                    // (the `external` case returns above), so `dst` always
                    // resolves to a concrete index — never a label.
                    dst: IrMapDst::Index(dst as u32),
                    one_way: p.arrow == MapArrow::ReadOnly,
                });
            }
        }
        binding.push(IrTapeBinding {
            caller_tape: phys as u32,
            pairs,
            // A positional (in-unit) entry never carries a parameter name.
            param: None,
            // A WRITTEN empty map (`with map { }`) silences the linker's
            // glyph-mismatch guard on purpose, exactly as it does for an
            // out-of-unit entry — the wire's `map_written` distinction is
            // not symbolic-only (docs/formats.md (bound calls)).
            map_written: map.is_some(),
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
                // `TailCall`/`ReturnExit` leave the world (no in-world
                // successor), like the terminators. Lowering never produces
                // either, but the walk stays exhaustive so a later
                // intra-world variant must be considered.
                IrTransition::TailCall { .. }
                | IrTransition::Return
                | IrTransition::ReturnExit { .. }
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
                match &r.transition {
                    IrTransition::CallThen { target, .. } | IrTransition::TailCall { target } => {
                        referenced.insert(target.as_str());
                    }
                    _ => {}
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
        for (k, r) in st.rules.iter().enumerate() {
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
            if r.direct
                && !(r.write.is_none()
                    && r.moves.is_none()
                    && !r.debugger
                    && matches!(r.transition, IrTransition::Goto { .. }))
            {
                return Err(format!(
                    "{}: state {} rule {}: `direct` on a non-bare rule",
                    w.name, st.id, k
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
                // A `TailCall` names another WORLD (like `CallThen.target`), so
                // there is no in-world state target to bounds-check — legal
                // wherever a `CallThen` is, which is anywhere a terminal is.
                // `ReturnExit`'s `exit` is bounds-checked against the
                // declaring world's `exits` count once a pass produces it —
                // not here, and not yet (no pass does).
                IrTransition::TailCall { .. }
                | IrTransition::Return
                | IrTransition::ReturnExit { .. }
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
        lower(&ex, &a.resolved, &Declarations::stdlib())
            .unwrap_or_else(|e| panic!("lower failed: {e}"))
    }

    /// analyze → expand → lower, expecting the front end to pass and lowering
    /// to fail; returns the lowering `CompileError`.
    fn lower_err_of(src: &str) -> CompileError {
        let a = analyze(src).unwrap_or_else(|e| panic!("analyze failed: {e}"));
        let ex = expand(&a.resolved).unwrap_or_else(|e| panic!("expand failed: {e}"));
        lower(&ex, &a.resolved, &Declarations::stdlib()).expect_err("expected lowering to fail")
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
        assert!(json.contains("\"version\": 5"), "{json}");
    }

    /// The bare version literal names the acceptance contract, not a hint —
    /// bumping it is what marks the vocabulary grown in this round as part of
    /// v5. Mutation: leaving `TM_IR_VERSION` at 4.
    #[test]
    fn the_version_literal_is_five() {
        assert_eq!(TM_IR_VERSION, 5);
    }

    /// A document exercising every v4/v5 field — glyphs and an effective
    /// write set, a two-exit `CallThen` alongside a `ReturnExit`, a
    /// `noreturn` world, a named (WRITTEN) binding-call param, and a
    /// glyph-labelled map pair — round-trips unchanged. Mutation:
    /// `#[serde(skip_serializing)]` on `IrTapeBinding.param` (or on
    /// `map_written`) drops it from the wire form, so the compare goes red.
    #[test]
    fn v5_documents_round_trip() {
        let ir = IrProgram {
            version: TM_IR_VERSION,
            worlds: vec![
                IrWorld {
                    name: "main".into(),
                    kind: IrWorldKind::Machine,
                    arity: 1,
                    tapes: vec![IrTape {
                        name: "a".into(),
                        alphabet: "al".into(),
                        cardinality: 3,
                        volatile: false,
                        glyphs: vec!["_".into(), "x".into(), "y".into()],
                        writes: Some(vec!["x".into(), "y".into()]),
                    }],
                    entry: 0,
                    states: vec![IrState {
                        id: 0,
                        name: "s".into(),
                        line: 1,
                        rules: vec![
                            IrRule {
                                pattern: vec![IrCell::Wildcard],
                                write: None,
                                moves: None,
                                debugger: false,
                                transition: IrTransition::CallThen {
                                    target: "r".into(),
                                    binding: vec![IrTapeBinding {
                                        caller_tape: 0,
                                        pairs: vec![
                                            IrMapPair {
                                                src: 1,
                                                dst: IrMapDst::Index(1),
                                                one_way: false,
                                            },
                                            IrMapPair {
                                                src: 2,
                                                dst: IrMapDst::Label("y".into()),
                                                one_way: true,
                                            },
                                        ],
                                        param: Some("k".into()),
                                        map_written: true,
                                    }],
                                    // A two-exit call: the exits= operand
                                    // names the resume states.
                                    exits: vec![1, 2],
                                    then: IrThen::Goto { state: 1 },
                                },
                                synthesized: false,
                                direct: false,
                                line: 1,
                            },
                            IrRule {
                                pattern: vec![IrCell::Wildcard],
                                write: None,
                                moves: None,
                                debugger: false,
                                transition: IrTransition::ReturnExit { exit: 1 },
                                synthesized: false,
                                direct: false,
                                line: 2,
                            },
                            IrRule {
                                pattern: vec![IrCell::Wildcard],
                                write: None,
                                moves: None,
                                debugger: false,
                                transition: IrTransition::Stop,
                                synthesized: false,
                                direct: false,
                                line: 3,
                            },
                        ],
                        dispatch: IrDispatch::Table,
                    }],
                    local: false,
                    line: 1,
                    exits: 2,
                    returns: true,
                },
                IrWorld {
                    name: "r".into(),
                    kind: IrWorldKind::Routine,
                    arity: 1,
                    tapes: vec![IrTape {
                        name: "t".into(),
                        alphabet: "al".into(),
                        cardinality: 3,
                        volatile: false,
                        glyphs: vec!["_".into(), "x".into(), "y".into()],
                        writes: None,
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
                            direct: false,
                            line: 1,
                        }],
                        dispatch: IrDispatch::Table,
                    }],
                    local: true,
                    line: 1,
                    // A routine declared `noreturn` — it never resumes at an
                    // in-caller `then`.
                    exits: 0,
                    returns: false,
                },
            ],
            entry_world: Some(0),
        };
        let json = ir.to_json();
        assert_eq!(IrProgram::from_json(&json).unwrap(), ir);
    }

    /// A v3 document (no `glyphs`, no `writes`, no `exits`/`returns`, no
    /// `param`, a bare numeric `dst`) still deserializes into the v4 struct,
    /// every new field landing at its empty value. Mutation: removing
    /// `#[serde(default)]` from `IrTapeBinding.param` (or from any other new
    /// field) turns a missing key into a hard deserialization error instead
    /// of a fill.
    #[test]
    fn a_v3_shaped_document_deserializes_with_empty_new_fields() {
        let v3 = r#"{
            "version": 3,
            "worlds": [
                {
                    "name": "main",
                    "kind": "machine",
                    "arity": 1,
                    "tapes": [{ "name": "a", "alphabet": "al", "cardinality": 3 }],
                    "entry": 0,
                    "states": [
                        {
                            "id": 0,
                            "name": "s",
                            "line": 1,
                            "rules": [
                                {
                                    "pattern": [{ "kind": "wildcard" }],
                                    "transition": {
                                        "kind": "call_then",
                                        "target": "r",
                                        "binding": [
                                            {
                                                "caller_tape": 0,
                                                "pairs": [{ "src": 1, "dst": 1 }]
                                            }
                                        ],
                                        "then": { "kind": "stop" }
                                    },
                                    "line": 1
                                }
                            ]
                        }
                    ],
                    "local": false,
                    "line": 1
                }
            ],
            "entry_world": 0
        }"#;
        let ir = IrProgram::from_json(v3).unwrap();
        let w = &ir.worlds[0];
        assert_eq!(w.exits, 0);
        assert!(w.returns, "absent means returns — v3 knew no noreturn");
        let tape = &w.tapes[0];
        assert!(tape.glyphs.is_empty());
        assert_eq!(tape.writes, None);
        let IrTransition::CallThen { binding, .. } = &w.states[0].rules[0].transition else {
            panic!("expected a call_then");
        };
        assert_eq!(binding[0].param, None);
        assert_eq!(binding[0].pairs[0].dst, IrMapDst::Index(1));
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
                            volatile: false,
                            glyphs: vec!["_".into(), "x".into(), "y".into()],
                            writes: None,
                        },
                        IrTape {
                            name: "b".into(),
                            alphabet: "al".into(),
                            cardinality: 3,
                            volatile: false,
                            glyphs: vec!["_".into(), "x".into(), "y".into()],
                            writes: None,
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
                                            dst: IrMapDst::Index(1),
                                            one_way: true,
                                        }],
                                        param: None,
                                        map_written: true,
                                    }],
                                    exits: Vec::new(),
                                    then: IrThen::Goto { state: 0 },
                                },
                                synthesized: false,
                                direct: false,
                                line: 1,
                            },
                            IrRule {
                                pattern: vec![IrCell::Wildcard, IrCell::Wildcard],
                                write: None,
                                moves: Some(vec![IrMove::Stay, IrMove::Stay]),
                                debugger: false,
                                transition: IrTransition::TrapRead,
                                synthesized: true,
                                direct: false,
                                line: 2,
                            },
                        ],
                        // The canonical hint (emits `"dispatch": "table"`).
                        dispatch: IrDispatch::Table,
                    }],
                    local: false,
                    line: 1,
                    exits: 0,
                    returns: true,
                },
                IrWorld {
                    name: "r".into(),
                    kind: IrWorldKind::Routine,
                    arity: 1,
                    tapes: vec![IrTape {
                        name: "t".into(),
                        alphabet: "al".into(),
                        cardinality: 3,
                        volatile: false,
                        glyphs: vec!["_".into(), "x".into(), "y".into()],
                        writes: None,
                    }],
                    entry: 0,
                    states: vec![IrState {
                        id: 0,
                        name: "s".into(),
                        line: 1,
                        rules: vec![
                            IrRule {
                                pattern: vec![IrCell::Index { index: 1 }],
                                write: None,
                                moves: None,
                                debugger: false,
                                // The optimizer-only terminal (emits the
                                // `"tail_call"` tag); target names another world.
                                transition: IrTransition::TailCall {
                                    target: "r2".into(),
                                },
                                synthesized: false,
                                direct: false,
                                line: 1,
                            },
                            IrRule {
                                pattern: vec![IrCell::Wildcard],
                                write: None,
                                moves: None,
                                debugger: false,
                                transition: IrTransition::Return,
                                synthesized: false,
                                direct: false,
                                line: 2,
                            },
                        ],
                        // The `dispatch_select` hint (emits `"dispatch": "branch"`).
                        dispatch: IrDispatch::Branch,
                    }],
                    local: true,
                    line: 1,
                    exits: 0,
                    returns: true,
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
            "\"kind\": \"tail_call\"",
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
        // The dispatch hint is a bare snake_case string too — both variants
        // present (the machine state is `table`, the routine state `branch`).
        assert!(json.contains("\"dispatch\": \"table\""), "{json}");
        assert!(json.contains("\"dispatch\": \"branch\""), "{json}");
    }

    #[test]
    fn volatile_tape_serializes_only_when_set() {
        let mut tape = IrTape {
            name: "t".into(),
            alphabet: "al".into(),
            cardinality: 3,
            volatile: false,
            glyphs: Vec::new(),
            writes: None,
        };
        let json = serde_json::to_string(&tape).unwrap();
        assert!(!json.contains("volatile"), "false is omitted: {json}");
        tape.volatile = true;
        let json = serde_json::to_string(&tape).unwrap();
        assert!(json.contains("\"volatile\":true"), "{json}");
        // absent field deserializes to false
        let back: IrTape =
            serde_json::from_str(r#"{"name":"t","alphabet":"al","cardinality":3}"#).unwrap();
        assert!(!back.volatile);
    }

    #[test]
    fn machine_tape_volatility_reaches_the_ir() {
        // machine with one volatile and one plain tape; the routine takes a
        // volatile param → its world's tape is flagged.
        let src = "\
alphabet bits { '_', '1' }
export routine probe(volatile tape s: bits) {
  entry state p { [*] -> return; }
}
machine {
  volatile tape sensor: bits;
  tape scratch: bits;
  entry state go { [*, *] -> call probe(s = sensor) then stop; }
}";
        let (ir, _) = lower_of(src);
        let main = world(&ir, "main");
        assert!(main.tapes[0].volatile && !main.tapes[1].volatile);
        let probe = ir
            .worlds
            .iter()
            .find(|w| w.name.ends_with("probe"))
            .expect("the probe world");
        assert!(probe.tapes[0].volatile);
    }

    /// `IrTape.glyphs` carries every tape's glyph table, and `IrTape.writes`
    /// carries the EFFECTIVE set (`compiler::declared_effective`) — never the
    /// raw `writes` clause. The `preserves`-only routine mirrors
    /// `std::…::invertNumber` (`preserves { '_' }`, no `writes` clause): the
    /// controller's ruling is that this must still yield `Some` (the
    /// alphabet minus the preserved glyph), not the `None` a raw reading of
    /// "no `writes` clause" would produce. Mutation: reading `tape.writes`
    /// directly instead of calling `declared_effective` makes the
    /// `preserves`-only assertion fail (it would see `None`).
    #[test]
    fn writes_is_the_effective_set_not_the_raw_clause() {
        let src = "\
alphabet bits { '_', '1' }
export routine byWrites(tape a: bits writes { '1' }) {
  entry state s { [*] -> return; }
}
export routine byPreserves(tape a: bits preserves { '_' }) {
  entry state s { [*] -> return; }
}
export routine byNeither(tape a: bits) {
  entry state s { [*] -> return; }
}
machine {
  tape t: bits;
  entry state go { [*] -> stop; }
}";
        let (ir, _) = lower_of(src);
        let main = world(&ir, "main");
        assert_eq!(main.tapes[0].glyphs, vec!["_".to_string(), "1".to_string()]);
        assert_eq!(
            main.tapes[0].writes, None,
            "a machine tape takes no contract"
        );

        let by_writes = ir
            .worlds
            .iter()
            .find(|w| w.name.ends_with("byWrites"))
            .expect("the byWrites world");
        assert_eq!(by_writes.tapes[0].writes, Some(vec!["1".to_string()]));

        // The invertNumber shape: `preserves` only, no `writes` clause. A raw
        // reading of "no `writes` clause" would answer `None`; the effective
        // set is the alphabet minus the preserved blank.
        let by_preserves = ir
            .worlds
            .iter()
            .find(|w| w.name.ends_with("byPreserves"))
            .expect("the byPreserves world");
        assert_eq!(by_preserves.tapes[0].writes, Some(vec!["1".to_string()]));

        // Neither clause: the routine's body (a bare `return`) writes
        // nothing, so the INFERRED set is empty — `Some(vec![])`, never
        // `None`, since a routine tape always lowers to `Some` (the wire
        // has no spelling for "no restriction declared", so "no clause"
        // and "declared to write nothing" would be indistinguishable on
        // an object read back if this were `None` — docs/formats.md
        // (routine interfaces)). `Some([])` and `None` decode identically
        // at codegen (`emit_params` suppresses `writes=` for either), so
        // this is a distinction only the IR itself still makes.
        let by_neither = ir
            .worlds
            .iter()
            .find(|w| w.name.ends_with("byNeither"))
            .expect("the byNeither world");
        assert_eq!(by_neither.tapes[0].writes, Some(Vec::<String>::new()));
    }

    #[test]
    fn graft_drops_the_graph_params_volatility_host_governs() {
        // A graph declares its param volatile; grafted onto a PLAIN host tape,
        // the host world's tape stays non-volatile (grafts dissolve pre-IR;
        // the host's declaration describes the real band).
        let src = "\
alphabet bits { '_', '1' }
graph g(volatile tape t: bits, state done) {
  entry state w { [*] -> done; }
}
machine {
  tape plain: bits;
  entry graft g(t = plain, done = stop);
}";
        let (ir, _) = lower_of(src);
        let main = world(&ir, "main");
        assert!(!main.tapes[0].volatile);
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
                    ..
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
                    dst: IrMapDst::Index(1),
                    one_way: false
                },
                IrMapPair {
                    src: 4,
                    dst: IrMapDst::Index(2),
                    one_way: false
                },
            ]
        );
        assert_eq!(call.1[0].param, None);
        validate_world(m).unwrap();
        validate_world(plus).unwrap();
    }

    /// A call that binds tapes into an EXTERNAL routine (imported, no local
    /// definition, and no declarations given for `mylib` — `lower_of` reads
    /// with `Declarations::stdlib()` only) lowers to a SYMBOLIC binding
    /// entry: the parameter's authored name (source order, since nothing
    /// declares `mylib`'s real tape order) and, for the with-map form, a
    /// glyph-LABELLED pair rather than an index. Both the with-map and the
    /// bindless (`num = t`) forms of the tape binding lower cleanly — the
    /// binding operand no longer needs the callee's signature to exist.
    #[test]
    fn external_call_binding_tapes_lowers_to_a_symbolic_entry() {
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
        let (ir, _) = lower_of(with_map);
        let m = world(&ir, "main");
        let main = state(m, "main");
        let binding = main
            .rules
            .iter()
            .find_map(|r| match &r.transition {
                IrTransition::CallThen { binding, .. } => Some(binding.clone()),
                _ => None,
            })
            .expect("a call row");
        assert_eq!(binding.len(), 1);
        assert_eq!(binding[0].param, Some("num".to_string()));
        assert_eq!(binding[0].caller_tape, 0);
        assert_eq!(
            binding[0].pairs,
            vec![IrMapPair {
                src: 1, // 'a' at index 1 of ab { '_', 'a', 'b' }
                dst: IrMapDst::Label("b".to_string()),
                one_way: false,
            }]
        );

        // Bindless (`num = t`, no map) lowers too — still a tape binding,
        // this time with NO pairs at all (an omitted map, not an empty one).
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
        let (ir2, _) = lower_of(bindless);
        let m2 = world(&ir2, "main");
        let main2 = state(m2, "main");
        let binding2 = main2
            .rules
            .iter()
            .find_map(|r| match &r.transition {
                IrTransition::CallThen { binding, .. } => Some(binding.clone()),
                _ => None,
            })
            .expect("a call row");
        assert_eq!(binding2.len(), 1);
        assert_eq!(binding2[0].param, Some("num".to_string()));
        assert!(
            binding2[0].pairs.is_empty(),
            "an omitted map emits no pairs"
        );
    }

    /// The bind-sugar path reaches the same lowering as a direct call, so an
    /// external bind that binds tapes lowers to the same symbolic shape —
    /// with-map and bindless alike.
    #[test]
    fn external_bind_sugar_binding_tapes_lowers_to_a_symbolic_entry() {
        let with_map = "\
alphabet ab { '_', 'a', 'b' }
use mylib::plusOne;
machine {
  tape t: ab;
  bind plusOne(num = t with map { 'a'->'b' }) as h;
  entry state main { [*] -> call h() then done; }
  state done { [*] -> stop; }
}";
        let (ir, _) = lower_of(with_map);
        let m = world(&ir, "main");
        let main = state(m, "main");
        let binding = main
            .rules
            .iter()
            .find_map(|r| match &r.transition {
                IrTransition::CallThen { binding, .. } => Some(binding.clone()),
                _ => None,
            })
            .expect("a call row");
        assert_eq!(binding.len(), 1);
        assert_eq!(binding[0].param, Some("num".to_string()));
        assert_eq!(
            binding[0].pairs,
            vec![IrMapPair {
                src: 1,
                dst: IrMapDst::Label("b".to_string()),
                one_way: false,
            }]
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
        let (ir2, _) = lower_of(bindless);
        let m2 = world(&ir2, "main");
        let main2 = state(m2, "main");
        let binding2 = main2
            .rules
            .iter()
            .find_map(|r| match &r.transition {
                IrTransition::CallThen { binding, .. } => Some(binding.clone()),
                _ => None,
            })
            .expect("a call row");
        assert_eq!(binding2.len(), 1);
        assert_eq!(binding2[0].param, Some("num".to_string()));
        assert!(
            binding2[0].pairs.is_empty(),
            "an omitted map emits no pairs"
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

    #[test]
    fn direct_is_rejected_on_a_non_bare_rule() {
        let (ir, _) = lower_of(A1);
        let mut m = world(&ir, "main").clone();
        // Rule 0 (`['b'] -> write ['a'] move [>] goto scan;`) carries a
        // write and a move — not a bare rule.
        m.states[0].rules[0].direct = true;
        let err = validate_world(&m).unwrap_err();
        assert!(err.contains("direct"), "{err}");
    }

    #[test]
    fn direct_round_trips_and_defaults_false() {
        let (ir, _) = lower_of(A1);
        // Lowering never sets `direct` — absent from the wire form, and a
        // program deserialized without it comes back false.
        let json = ir.to_json();
        assert!(!json.contains("\"direct\""), "{json}");
        let back = IrProgram::from_json(&json).unwrap();
        assert_eq!(back, ir);
        assert!(
            back.worlds
                .iter()
                .all(|w| w.states.iter().all(|s| s.rules.iter().all(|r| !r.direct))),
            "{back:?}"
        );

        // A bare rule (no write, no moves, no debugger, a `Goto`) with
        // `direct` set round-trips to_json/from_json unchanged.
        let mut with_direct = ir.clone();
        let rule = &mut with_direct.worlds[0].states[0].rules[0];
        rule.write = None;
        rule.moves = None;
        rule.debugger = false;
        rule.transition = IrTransition::Goto { state: 0 };
        rule.direct = true;
        validate_world(&with_direct.worlds[0]).unwrap();

        let json2 = with_direct.to_json();
        assert!(json2.contains("\"direct\": true"), "{json2}");
        let back2 = IrProgram::from_json(&json2).unwrap();
        assert_eq!(back2, with_direct);
    }
}
