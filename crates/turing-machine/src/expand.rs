//! `.tmc` front-end stage 2 — graft splicing + range expansion (spec §10.4 /
//! §10.3), the compiler-side analog of the linker's mono stamping.
//!
//! [`expand`] runs after resolution (`compiler::analyze`) and before IR
//! lowering, matching the pipeline order flatten → GRAFT EXPANSION → RANGE
//! EXPANSION → `ir::lower`. It turns the [`crate::compiler::Resolved`] module
//! (rules still in SOURCE form: ranges, pattern bindings, `{v±k}`/`{c}`
//! substitutions, graft declarations) into an [`Expanded`] module whose states
//! carry only CONCRETE, index-resolved rules — no ranges, no bindings, no
//! grafts. Trap rows synthesized for a graft's holey binding survive as
//! [`Transition::TrapRead`] / [`Transition::TrapWrite`] markers the IR lowers
//! to `trap #0` / `trap #1`; every rule keeps a provenance span for
//! diagnostics.
//!
//! The graft splice mirrors `crates/core/src/linker/stamp.rs` (mono stamping)
//! at the source level: a graph's states are copied into the host world, the
//! signature tape order projects onto host tape indices, per-tape symbol maps
//! rewrite pattern cells by READ-direction preimage (multi-preimage expands
//! rows, zero-preimage drops them) and write cells by write direction (a write
//! hole becomes a synthesized `trap #1` row), and each bound tape's read holes
//! synthesize `trap #0` rows prepended first-match to every conditional state.
//! The map legality (blank pinning, one-way `=>` collapse, equal-size
//! identity-completion injectivity, unequal hole-based projection) is the same
//! contract the 5b composition engine enforces at link
//! (`crates/core/src/linker/{compose,engine}.rs`); the compiler-side checks are
//! earlier and carry source spans.
//!
//! Unused until Task 7 wires `compile()` over it (Task 6 lowers its output);
//! the in-module tests exercise it meanwhile.
#![allow(dead_code)]

use std::collections::{BTreeMap, BTreeSet, HashMap};

use mtc_core::diagnostics::{Diagnostic, Span};

use crate::compiler::{CompileError, CompileErrorKind, ResolvedAlphabet, WorldKind};
use crate::parser::{
    BindingArg, Continuation, MoveCell, MoveDir, MoveVec, PatternCell, PatternCellKind, Rule,
    SymLit, Transition, WriteCell, WriteCellKind, WriteVec,
};

// ---------------------------------------------------------------------------
// Output — the concrete, index-resolved module Task 6 (IR lowering) consumes.
// ---------------------------------------------------------------------------

/// The fully-expanded module: every world's states carry concrete rules only.
/// Graph worlds are gone (their bodies are spliced into their graft hosts);
/// only the machine (a program's entry) and routines remain as EMITTED worlds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Expanded {
    /// Resolved alphabets, passed through from [`Resolved`] (index → glyph).
    pub alphabets: HashMap<String, ResolvedAlphabet>,
    /// Emitted worlds: the machine and every routine, in source order.
    pub worlds: Vec<ExpandedWorld>,
    /// Index into `worlds` of the machine block, or `None` for a library.
    pub entry_world: Option<usize>,
    /// Non-fatal findings (shadowed rules, product-threshold warnings).
    pub diagnostics: Vec<Diagnostic>,
}

/// One emitted world — a machine block or a routine, after expansion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ExpandedWorld {
    pub kind: WorldKind,
    pub name: String,
    /// Tapes in vector-position order (name, mangled alphabet, cardinality).
    pub tapes: Vec<ExpandedTape>,
    /// Routine state-parameter names, in signature order (empty for a
    /// machine). A routine's continuations are resolved at its call sites.
    pub state_params: Vec<String>,
    /// The concrete entry-state name.
    pub entry: Option<String>,
    /// States in emission order (own states, then spliced graft instances).
    pub states: Vec<ExpandedState>,
}

/// A tape's position, name, alphabet, and cardinality.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ExpandedTape {
    pub name: String,
    pub alphabet: String,
    pub cardinality: usize,
}

/// A concrete state: a name plus its concrete rules, in priority (row) order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ExpandedState {
    pub name: String,
    pub name_span: Span,
    pub rules: Vec<ExpandedRule>,
}

/// A concrete rule (one machine step). The pattern/write/move vectors are
/// world-width (one cell per tape), index-resolved against the tape alphabets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ExpandedRule {
    /// Match cell per tape: a concrete symbol index or a wildcard.
    pub pattern: Vec<Cell>,
    /// `debugger` — pause at this rule's code head (`brk`).
    pub debugger: bool,
    /// Write cell per tape (`Keep` = leave the current symbol). Ignored for a
    /// trap transition (a trap stops before any write).
    pub write: Vec<WriteOut>,
    /// Move per tape (`Stay` default).
    pub moves: Vec<MoveDir>,
    pub transition: Transition2,
    /// Provenance: the source rule this concrete row derives from.
    pub span: Span,
}

/// A match cell: a concrete symbol index, or a wildcard (`*`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Cell {
    Wild,
    Sym(u16),
}

/// A write cell: keep the current symbol, or write a concrete symbol index.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WriteOut {
    Keep,
    Sym(u16),
}

/// A concrete rule's control transfer. Mirrors [`Transition`] but with graft
/// continuations already substituted, plus the two synthesized trap markers a
/// graft's holey binding produces (the IR lowers them to `trap #0`/`#1`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Transition2 {
    /// `goto` a concrete same-world state.
    Goto(String),
    /// A routine call surviving to IR (a binding-call operand). `args` are the
    /// source-form binding args the IR lowers to a bound-call record.
    Call {
        target: String,
        external: bool,
        args: Vec<BindingArg>,
        then: Continuation,
    },
    /// A call on a world-local bind name (the bind carries the binding).
    BindCall {
        name: String,
        then: Continuation,
    },
    Return,
    Stop,
    Halt,
    /// A synthesized unmapped-read trap row (`trap #0`) — prepended first-match
    /// for a bound tape's read hole.
    TrapRead,
    /// A synthesized unmapped-write trap (`trap #1`) — a rule whose graft write
    /// maps a symbol with no host image.
    TrapWrite,
}

// ---------------------------------------------------------------------------
// The symbol-map algebra — reimplemented compiler-side (the linker's
// `compose.rs` is core-private). Identity-default reads with cardinality
// holes, one-way `=>` collapse excluded from write-back, blank pinned, and
// the equal-size identity-completion injectivity check.
// ---------------------------------------------------------------------------

/// A partial symbol map, identity for unlisted symbols; `holes` trap. Mirrors
/// the linker's `SparseMap` (docs/formats.md (frames profile)).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct SymMap {
    pairs: BTreeMap<u16, u16>,
    holes: BTreeSet<u16>,
}

impl SymMap {
    fn identity() -> Self {
        Self::default()
    }

    /// `None` marks a hole; `Some(d)` the image (identity default for unlisted).
    fn apply(&self, s: u16) -> Option<u16> {
        if self.holes.contains(&s) {
            None
        } else {
            Some(self.pairs.get(&s).copied().unwrap_or(s))
        }
    }

    fn is_identity(&self) -> bool {
        self.holes.is_empty() && self.pairs.iter().all(|(s, d)| s == d)
    }

    /// A deterministic byte serialization for the dedup key.
    fn write_key(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&(self.pairs.len() as u32).to_le_bytes());
        for (s, d) in &self.pairs {
            out.extend_from_slice(&s.to_le_bytes());
            out.extend_from_slice(&d.to_le_bytes());
        }
        out.extend_from_slice(&(self.holes.len() as u32).to_le_bytes());
        for h in &self.holes {
            out.extend_from_slice(&h.to_le_bytes());
        }
    }
}

/// Compose two maps: `second ∘ first` (apply `first`, then `second`), holes
/// propagating (the linker's `compose_map` verbatim).
fn compose_map(first: &SymMap, second: &SymMap) -> SymMap {
    let mut out = SymMap::identity();
    let candidates: BTreeSet<u16> = first
        .pairs
        .keys()
        .chain(first.holes.iter())
        .chain(second.pairs.keys())
        .chain(second.holes.iter())
        .copied()
        .collect();
    for s in candidates {
        match first.apply(s).and_then(|m| second.apply(m)) {
            None => {
                out.holes.insert(s);
            }
            Some(d) if d != s => {
                out.pairs.insert(s, d);
            }
            Some(_) => {}
        }
    }
    out
}

/// One bound graph tape's absolute placement onto a host tape, plus its
/// read map (host → graph) and write map (graph → host) and the two
/// cardinalities the hole checks need.
#[derive(Debug, Clone, PartialEq, Eq)]
struct TapeMap {
    /// Host tape index this graph tape projects onto.
    phys: usize,
    host_card: usize,
    graph_card: usize,
    /// host symbol → graph symbol (the binding's read direction).
    rmap: SymMap,
    /// graph symbol → host symbol (bidirectional pairs' inverses).
    wmap: SymMap,
}

impl TapeMap {
    /// The graph symbol host symbol `p` reads as, or `None` (a read hole) when
    /// its image falls outside the graph tape's alphabet.
    fn read_image(&self, p: u16) -> Option<u16> {
        match self.rmap.apply(p) {
            Some(v) if usize::from(v) < self.graph_card => Some(v),
            _ => None,
        }
    }

    /// The host symbol graph symbol `v` writes as, or `None` (a write hole).
    fn write_image(&self, v: u16) -> Option<u16> {
        match self.wmap.apply(v) {
            Some(p) if usize::from(p) < self.host_card => Some(p),
            _ => None,
        }
    }

    /// The ascending host preimage of graph symbol `v` (the host symbols that
    /// read as `v`) — the row-rewriting preimage. Empty ⇒ the cell is dead.
    fn preimage(&self, v: u16) -> Vec<u16> {
        (0..self.host_card as u16)
            .filter(|&p| self.read_image(p) == Some(v))
            .collect()
    }

    /// The ascending host symbols that read as no graph symbol (read holes).
    fn holes(&self) -> Vec<u16> {
        (0..self.host_card as u16)
            .filter(|&p| self.read_image(p).is_none())
            .collect()
    }
}

/// A composed graft frame: one [`TapeMap`] per graph tape (in signature tape
/// order). The continuation substitution rides alongside in the splice.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Composite {
    tapes: Vec<TapeMap>,
}

impl Composite {
    /// A deterministic dedup key over the projection and both maps per tape.
    fn key(&self) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&(self.tapes.len() as u32).to_le_bytes());
        for t in &self.tapes {
            out.extend_from_slice(&(t.phys as u32).to_le_bytes());
            out.extend_from_slice(&(t.host_card as u32).to_le_bytes());
            out.extend_from_slice(&(t.graph_card as u32).to_le_bytes());
            t.rmap.write_key(&mut out);
            t.wmap.write_key(&mut out);
        }
        out
    }
}

/// Compose an outer graft frame (a graph running under a host) with a nested
/// graft's binding (a deeper graph running under the outer graph): the nested
/// binding's caller tapes index the OUTER graph's tapes, resolved through
/// `outer` to host tapes. Mirrors the linker's `compose_composites`.
fn compose(outer: &Composite, inner: &Composite) -> Composite {
    let tapes = inner
        .tapes
        .iter()
        .map(|it| {
            let ot = &outer.tapes[it.phys];
            TapeMap {
                phys: ot.phys,
                host_card: ot.host_card,
                graph_card: it.graph_card,
                rmap: compose_map(&ot.rmap, &it.rmap),
                wmap: compose_map(&it.wmap, &ot.wmap),
            }
        })
        .collect();
    Composite { tapes }
}

// ---------------------------------------------------------------------------
// The splice — map a graph's (already range-expanded, graph-space) states to
// host width through a [`Composite`], the compiler-side analog of
// `crates/core/src/linker/stamp.rs::build_stamp`. Pattern cells map by the
// read PREIMAGE (multi-preimage expands rows, zero-preimage drops), write cells
// by the write image (a hole turns the whole rule into a `trap #1` row), and
// each bound tape's read holes prepend `trap #0` rows to every conditional
// state (a straight-line state does no `rd`, so a hole under it never traps —
// spec §5.3).
// ---------------------------------------------------------------------------

/// True when every cell is a wildcard.
fn is_all_wild(pattern: &[Cell]) -> bool {
    pattern.iter().all(|c| matches!(c, Cell::Wild))
}

/// True when the state performs a read (`rd` + match): anything but a single
/// all-wildcard rule, which codegen lowers to straight-line code (GC3). Only a
/// reading state gains synthesized read-trap rows.
fn state_reads(rules: &[ExpandedRule]) -> bool {
    !(rules.len() == 1 && is_all_wild(&rules[0].pattern))
}

/// The cartesian product of per-position option lists, position order
/// preserved and each list's order kept (mirrors the linker's `cartesian`:
/// the last position varies fastest).
fn cartesian(opts: &[(usize, Vec<Cell>)]) -> Vec<Vec<(usize, Cell)>> {
    let mut result: Vec<Vec<(usize, Cell)>> = vec![Vec::new()];
    for (pos, vals) in opts {
        let mut next = Vec::with_capacity(result.len() * vals.len());
        for combo in &result {
            for &v in vals {
                let mut c = combo.clone();
                c.push((*pos, v));
                next.push(c);
            }
        }
        result = next;
    }
    result
}

/// Synthesized read-trap rows, first-match: one per bound-tape read hole, in
/// tape order then ascending hole symbol (matching the linker's prepend order).
fn trap_read_rows(comp: &Composite, host_arity: usize, span: Span) -> Vec<ExpandedRule> {
    let mut rows = Vec::new();
    for t in &comp.tapes {
        for u in t.holes() {
            let mut pattern = vec![Cell::Wild; host_arity];
            pattern[t.phys] = Cell::Sym(u);
            rows.push(ExpandedRule {
                pattern,
                debugger: false,
                write: vec![WriteOut::Keep; host_arity],
                moves: vec![MoveDir::Stay; host_arity],
                transition: Transition2::TrapRead,
                span,
            });
        }
    }
    rows
}

/// Map one graph-space rule to host width. Returns the host rows (several under
/// one-way preimage collapse; empty when a read cell has zero preimage — the
/// rule is dead). A write with no host image makes every row a `trap #1`.
fn map_rule(
    rule: &ExpandedRule,
    comp: &Composite,
    host_arity: usize,
    remap_tr: &impl Fn(&Transition2) -> Transition2,
) -> Vec<ExpandedRule> {
    // Write + move projection (independent of which read preimage matched).
    let mut write_hole = false;
    let mut host_write = vec![WriteOut::Keep; host_arity];
    let mut host_moves = vec![MoveDir::Stay; host_arity];
    for (k, t) in comp.tapes.iter().enumerate() {
        if let WriteOut::Sym(gv) = rule.write[k] {
            match t.write_image(gv) {
                Some(p) => host_write[t.phys] = WriteOut::Sym(p),
                None => write_hole = true,
            }
        }
        host_moves[t.phys] = rule.moves[k];
    }

    // Read projection: per bound tape, the preimage (or wildcard).
    let mut opts: Vec<(usize, Vec<Cell>)> = Vec::with_capacity(comp.tapes.len());
    for (k, t) in comp.tapes.iter().enumerate() {
        match rule.pattern[k] {
            Cell::Wild => opts.push((t.phys, vec![Cell::Wild])),
            Cell::Sym(gv) => {
                let pre = t.preimage(gv);
                if pre.is_empty() {
                    return Vec::new(); // no host symbol reads as this cell — dead
                }
                opts.push((t.phys, pre.into_iter().map(Cell::Sym).collect()));
            }
        }
    }

    let transition = if write_hole {
        Transition2::TrapWrite
    } else {
        remap_tr(&rule.transition)
    };
    cartesian(&opts)
        .into_iter()
        .map(|combo| {
            let mut pattern = vec![Cell::Wild; host_arity];
            for (pos, c) in combo {
                pattern[pos] = c;
            }
            ExpandedRule {
                pattern,
                debugger: rule.debugger,
                write: if write_hole {
                    vec![WriteOut::Keep; host_arity]
                } else {
                    host_write.clone()
                },
                moves: if write_hole {
                    vec![MoveDir::Stay; host_arity]
                } else {
                    host_moves.clone()
                },
                transition: transition.clone(),
                span: rule.span,
            }
        })
        .collect()
}

/// Splice one graph-space state into host width under `comp`: prepend read-trap
/// rows to a reading state, then map every rule. `remap_tr` renames the
/// state's transitions (own states → synthetic/instance names, state-params →
/// the graft's continuation substitution).
fn splice_state(
    gstate: &ExpandedState,
    comp: &Composite,
    host_arity: usize,
    host_name: &str,
    remap_tr: &impl Fn(&Transition2) -> Transition2,
) -> ExpandedState {
    let mut rules = Vec::new();
    if state_reads(&gstate.rules) {
        rules.extend(trap_read_rows(comp, host_arity, gstate.name_span));
    }
    for r in &gstate.rules {
        rules.extend(map_rule(r, comp, host_arity, remap_tr));
    }
    ExpandedState {
        name: host_name.to_string(),
        name_span: gstate.name_span,
        rules,
    }
}

// ---------------------------------------------------------------------------
// Range expansion (spec §10.3) — one source rule → concrete index-resolved
// rows. Pattern ranges / single-with-binding expand cartesian (leftmost tape
// varies slowest, rightmost fastest — matching the linker's preimage
// cartesian); `{v±k}` folds per row (numeric, bounds-checked), `{c}` passes the
// bound glyph through; a range value with no glyph on the tape drops that
// alternative. Product over 256 warns.
// ---------------------------------------------------------------------------

/// The product-count above which a rule's expansion warns (spec §10.3 / GC7).
const PRODUCT_THRESHOLD: usize = 256;

/// One tape's resolution context: its glyph vector (index → glyph) and the
/// inverse lookup (glyph → index).
struct TapeInfo {
    glyphs: Vec<String>,
    index: HashMap<String, u16>,
}

impl TapeInfo {
    fn new(glyphs: &[String]) -> Self {
        let index = glyphs
            .iter()
            .enumerate()
            .map(|(i, g)| (g.clone(), i as u16))
            .collect();
        Self {
            glyphs: glyphs.to_vec(),
            index,
        }
    }

    fn card(&self) -> usize {
        self.glyphs.len()
    }

    fn idx(&self, glyph: &str) -> Option<u16> {
        self.index.get(glyph).copied()
    }
}

/// A symbol bound by a pattern cell's `as v`. `glyph` is what `{v}` writes;
/// `value` is the number `{v±k}` arithmetic folds (`None` for glyph bindings —
/// the parser already forbids arithmetic on those).
#[derive(Debug, Clone)]
struct BoundVal {
    glyph: String,
    value: Option<i64>,
}

/// The glyph label a symbol literal contributes (numeric → its decimal value).
fn glyph_label(s: &SymLit) -> String {
    match s {
        SymLit::Glyph { value, .. } => value.clone(),
        SymLit::Number { value, .. } => value.to_string(),
    }
}

/// A symbol literal's numeric value, if it is a number literal.
fn numeric_value(s: &SymLit) -> Option<i64> {
    match s {
        SymLit::Number { value, .. } => Some(i64::from(*value)),
        SymLit::Glyph { .. } => None,
    }
}

/// Enumerate a pattern range's `(glyph, value)` members, inclusive/ascending.
/// Numeric ranges mint decimal glyphs with their value; glyph ranges walk
/// scalar succession with no value. Descending or non-scalar endpoints error
/// at `span`.
fn enumerate_range(
    lo: &SymLit,
    hi: &SymLit,
    span: Span,
) -> Result<Vec<(String, Option<i64>)>, CompileError> {
    match (lo, hi) {
        (SymLit::Number { value: l, .. }, SymLit::Number { value: h, .. }) => {
            if l > h {
                return Err(CompileError {
                    span,
                    kind: CompileErrorKind::RangeDescending,
                });
            }
            Ok((*l..=*h)
                .map(|v| (v.to_string(), Some(i64::from(v))))
                .collect())
        }
        (SymLit::Glyph { value: l, .. }, SymLit::Glyph { value: h, .. }) => {
            let (Some(lc), Some(hc)) = (single_scalar(l), single_scalar(h)) else {
                return Err(CompileError {
                    span,
                    kind: CompileErrorKind::RangeEndpointNotScalar,
                });
            };
            if lc as u32 > hc as u32 {
                return Err(CompileError {
                    span,
                    kind: CompileErrorKind::RangeDescending,
                });
            }
            Ok((lc as u32..=hc as u32)
                .filter_map(char::from_u32)
                .map(|c| (c.to_string(), None))
                .collect())
        }
        _ => Err(CompileError {
            span,
            kind: CompileErrorKind::RangeEndpointNotScalar,
        }),
    }
}

/// The single Unicode scalar of a glyph, or `None` if it is not exactly one.
fn single_scalar(g: &str) -> Option<char> {
    let mut chars = g.chars();
    let first = chars.next()?;
    chars.next().is_none().then_some(first)
}

/// One pattern cell's expansion alternatives: `(concrete cell, optional
/// binding)`. A wildcard, single symbol, or one row per range member; a range
/// value with no glyph on the tape drops silently.
type CellOpt = (Cell, Option<(String, BoundVal)>);

fn cell_options(cell: &PatternCell, ti: &TapeInfo) -> Result<Vec<CellOpt>, CompileError> {
    let binding = cell.binding.as_ref().map(|b| b.name.clone());
    match &cell.kind {
        PatternCellKind::Wildcard => Ok(vec![(Cell::Wild, None)]),
        PatternCellKind::Single(s) => {
            let Some(i) = ti.idx(&glyph_label(s)) else {
                // A single concrete symbol not on this tape can never match —
                // the rule is dead; drop it (no valid index to lower).
                return Ok(Vec::new());
            };
            let bv = binding.map(|n| {
                (
                    n,
                    BoundVal {
                        glyph: glyph_label(s),
                        value: numeric_value(s),
                    },
                )
            });
            Ok(vec![(Cell::Sym(i), bv)])
        }
        PatternCellKind::Range { lo, hi } => {
            let mut opts = Vec::new();
            for (glyph, value) in enumerate_range(lo, hi, cell.span)? {
                if let Some(i) = ti.idx(&glyph) {
                    let bv = binding.clone().map(|n| (n, BoundVal { glyph, value }));
                    opts.push((Cell::Sym(i), bv));
                }
            }
            Ok(opts)
        }
    }
}

/// Range-expand one source rule against its world's tape contexts. Rows come
/// out in expansion order; a product over [`PRODUCT_THRESHOLD`] pushes a
/// warning to `warn`. The transition is lowered with `lower_tr` (own states
/// pass state names through; a graft body remaps them at splice).
fn expand_rule(
    rule: &Rule,
    tapes: &[TapeInfo],
    warn: &mut Vec<Diagnostic>,
    lower_tr: &impl Fn(&Transition) -> Transition2,
) -> Result<Vec<ExpandedRule>, CompileError> {
    let arity = tapes.len();
    check_width(rule.pattern.cells.len(), arity, rule.span)?;
    if let Some(w) = &rule.write {
        check_width(w.cells.len(), arity, w.span)?;
    }
    if let Some(m) = &rule.mov {
        check_width(m.cells.len(), arity, m.span)?;
    }

    let mut per_cell: Vec<Vec<CellOpt>> = Vec::with_capacity(arity);
    for (i, cell) in rule.pattern.cells.iter().enumerate() {
        per_cell.push(cell_options(cell, &tapes[i])?);
    }

    // Cartesian product, leftmost tape varying slowest (rightmost fastest).
    let mut combos: Vec<Vec<CellOpt>> = vec![Vec::new()];
    for opts in &per_cell {
        let mut next = Vec::with_capacity(combos.len() * opts.len());
        for combo in &combos {
            for opt in opts {
                let mut c = combo.clone();
                c.push(opt.clone());
                next.push(c);
            }
        }
        combos = next;
    }

    if combos.len() > PRODUCT_THRESHOLD {
        warn.push(Diagnostic {
            code: "expansion-threshold",
            span: rule.span,
            message: format!(
                "rule expands to {} rows (over {PRODUCT_THRESHOLD}) — the cost is large",
                combos.len()
            ),
            fix: None,
        });
    }

    let transition = lower_tr(&rule.transition);
    let mut out = Vec::with_capacity(combos.len());
    for combo in combos {
        let pattern: Vec<Cell> = combo.iter().map(|(c, _)| *c).collect();
        let env: HashMap<&str, &BoundVal> = combo
            .iter()
            .filter_map(|(_, b)| b.as_ref().map(|(n, v)| (n.as_str(), v)))
            .collect();
        let write = resolve_write(rule.write.as_ref(), tapes, &env)?;
        let moves = resolve_moves(rule.mov.as_ref(), arity);
        out.push(ExpandedRule {
            pattern,
            debugger: rule.debugger,
            write,
            moves,
            transition: transition.clone(),
            span: rule.span,
        });
    }
    Ok(out)
}

fn check_width(got: usize, expected: usize, span: Span) -> Result<(), CompileError> {
    if got == expected {
        Ok(())
    } else {
        Err(CompileError {
            span,
            kind: CompileErrorKind::RowWidth { expected, got },
        })
    }
}

/// Resolve a write vector to per-tape [`WriteOut`], folding `{v±k}` / `{c}`.
fn resolve_write(
    write: Option<&WriteVec>,
    tapes: &[TapeInfo],
    env: &HashMap<&str, &BoundVal>,
) -> Result<Vec<WriteOut>, CompileError> {
    let arity = tapes.len();
    let Some(wv) = write else {
        return Ok(vec![WriteOut::Keep; arity]);
    };
    let mut out = Vec::with_capacity(arity);
    for (i, cell) in wv.cells.iter().enumerate() {
        out.push(resolve_write_cell(cell, &tapes[i], env)?);
    }
    Ok(out)
}

fn resolve_write_cell(
    cell: &WriteCell,
    ti: &TapeInfo,
    env: &HashMap<&str, &BoundVal>,
) -> Result<WriteOut, CompileError> {
    match &cell.kind {
        WriteCellKind::Keep => Ok(WriteOut::Keep),
        WriteCellKind::Lit(s) => {
            let glyph = glyph_label(s);
            ti.idx(&glyph).map(WriteOut::Sym).ok_or(CompileError {
                span: cell.span,
                kind: CompileErrorKind::MapSymbolNotInAlphabet(glyph),
            })
        }
        WriteCellKind::Subst {
            name,
            name_span,
            delta,
        } => {
            let bv = env.get(name.as_str()).ok_or_else(|| CompileError {
                span: *name_span,
                kind: CompileErrorKind::FoldOutOfAlphabet(format!(
                    "`{{{name}}}` refers to no pattern binding in this rule"
                )),
            })?;
            let glyph = if *delta == 0 {
                bv.glyph.clone()
            } else {
                // Arithmetic is numeric-only (the parser rejects `{c±k}`).
                let base = bv.value.ok_or_else(|| CompileError {
                    span: *name_span,
                    kind: CompileErrorKind::FoldOutOfAlphabet(format!(
                        "`{{{name}}}` binds a glyph, which cannot take arithmetic"
                    )),
                })?;
                let folded = base + delta;
                if folded < 0 {
                    return Err(CompileError {
                        span: *name_span,
                        kind: CompileErrorKind::FoldOutOfAlphabet(format!(
                            "`{{{name}{delta:+}}}` folds to {folded}, below the alphabet"
                        )),
                    });
                }
                folded.to_string()
            };
            ti.idx(&glyph).map(WriteOut::Sym).ok_or(CompileError {
                span: *name_span,
                kind: CompileErrorKind::FoldOutOfAlphabet(format!(
                    "`{{{name}{delta:+}}}` folds to `{glyph}`, not in the tape's alphabet"
                )),
            })
        }
    }
}

/// Resolve a move vector to per-tape [`MoveDir`] (`Stay` default when omitted).
fn resolve_moves(mov: Option<&MoveVec>, arity: usize) -> Vec<MoveDir> {
    match mov {
        None => vec![MoveDir::Stay; arity],
        Some(mv) => mv.cells.iter().map(|c: &MoveCell| c.dir).collect(),
    }
}

#[cfg(test)]
mod map_tests {
    use super::*;

    fn m(pairs: &[(u16, u16)]) -> SymMap {
        let mut s = SymMap::identity();
        for &(a, b) in pairs {
            s.pairs.insert(a, b);
        }
        s
    }

    #[test]
    fn identity_read_and_write_with_cardinality_holes() {
        // host wider (5) than graph (3), remap the two out-of-range symbols
        // (3→1, 4→2). In-range unlisted symbols keep identity (1→1, 2→2).
        let t = TapeMap {
            phys: 0,
            host_card: 5,
            graph_card: 3,
            rmap: m(&[(3, 1), (4, 2)]),
            wmap: m(&[(1, 3), (2, 4)]),
        };
        assert_eq!(t.read_image(0), Some(0));
        assert_eq!(t.read_image(1), Some(1)); // identity default, in range
        assert_eq!(t.read_image(3), Some(1)); // remapped
        // no host symbol is a hole here (every image < 3).
        assert!(t.holes().is_empty());
        // graph 1 has preimage {1 (identity), 3 (remap)} ascending.
        assert_eq!(t.preimage(1), vec![1, 3]);
        assert_eq!(t.preimage(2), vec![2, 4]);
        assert_eq!(t.write_image(1), Some(3));
    }

    #[test]
    fn out_of_range_symbol_is_a_read_hole() {
        // host (4) → graph (3), symbol 3 unremapped: identity image 3 ≥ 3 ⇒ hole.
        let t = TapeMap {
            phys: 0,
            host_card: 4,
            graph_card: 3,
            rmap: SymMap::identity(),
            wmap: SymMap::identity(),
        };
        assert_eq!(t.read_image(3), None);
        assert_eq!(t.holes(), vec![3]);
    }

    #[test]
    fn compose_threads_reads_and_writes() {
        // outer: host 4 → graph-A 1 (and back). inner (graph-B under A): A 1 → B 3.
        let outer = Composite {
            tapes: vec![TapeMap {
                phys: 2,
                host_card: 6,
                graph_card: 6,
                rmap: m(&[(4, 1)]),
                wmap: m(&[(1, 4)]),
            }],
        };
        let inner = Composite {
            tapes: vec![TapeMap {
                phys: 0,
                host_card: 6,
                graph_card: 6,
                rmap: m(&[(1, 3)]),
                wmap: m(&[(3, 1)]),
            }],
        };
        let c = compose(&outer, &inner);
        assert_eq!(c.tapes[0].phys, 2);
        assert_eq!(c.tapes[0].rmap.apply(4), Some(3)); // host 4 → A 1 → B 3
        assert_eq!(c.tapes[0].wmap.apply(3), Some(4)); // B 3 → A 1 → host 4
    }
}

#[cfg(test)]
mod range_tests {
    use super::*;
    use crate::lexer::lex;
    use crate::parser::parse;

    /// Parse a program and return the machine's `state_idx`-th state's rules.
    fn machine_rules(src: &str, state_idx: usize) -> Vec<Rule> {
        let toks = lex(src).expect("lex");
        let prog = parse(&toks).expect("parse");
        prog.machine.expect("machine").states[state_idx]
            .rules
            .clone()
    }

    fn ti(glyphs: &[&str]) -> TapeInfo {
        TapeInfo::new(&glyphs.iter().map(|g| g.to_string()).collect::<Vec<_>>())
    }

    /// A transition lowerer for own states (goto passes the name through).
    fn own_tr(t: &Transition) -> Transition2 {
        match t {
            Transition::Goto { name, .. } => Transition2::Goto(name.clone()),
            Transition::Return { .. } => Transition2::Return,
            Transition::Stop { .. } => Transition2::Stop,
            Transition::Halt { .. } => Transition2::Halt,
            Transition::Call { .. } => panic!("no call in these fixtures"),
        }
    }

    #[test]
    fn glyph_range_binding_passes_through_on_the_other_tape() {
        // A.3's copy rule: `['0'..'1' as c, *] -> write [-, {c}] move [>, >] goto copy`.
        let src = "\
alphabet bits { '_', '0', '1' }
machine {
  tape src: bits;
  tape dst: bits;
  entry state copy {
    ['0'..'1' as c, *] -> write [-, {c}] move [>, >] goto copy;
    ['_', *]           -> stop;
  }
}
";
        let rules = machine_rules(src, 0);
        let tapes = vec![ti(&["_", "0", "1"]), ti(&["_", "0", "1"])];
        let mut warn = Vec::new();
        let rows = expand_rule(&rules[0], &tapes, &mut warn, &own_tr).unwrap();
        assert_eq!(rows.len(), 2);
        // '0' is index 1, '1' is index 2; `{c}` writes the same glyph on dst.
        assert_eq!(rows[0].pattern, vec![Cell::Sym(1), Cell::Wild]);
        assert_eq!(rows[0].write, vec![WriteOut::Keep, WriteOut::Sym(1)]);
        assert_eq!(rows[1].pattern, vec![Cell::Sym(2), Cell::Wild]);
        assert_eq!(rows[1].write, vec![WriteOut::Keep, WriteOut::Sym(2)]);
        assert_eq!(rows[0].moves, vec![MoveDir::Right, MoveDir::Right]);
        assert_eq!(rows[0].transition, Transition2::Goto("copy".into()));
        assert!(warn.is_empty());
    }

    #[test]
    fn numeric_range_folds_arithmetic_per_row() {
        // A.4's `[1..125 as v] -> write [{v+1}] stop` on `bytes = 0..126`.
        let src = "\
alphabet bytes { 0..126 }
machine {
  tape cell: bytes;
  entry state inc {
    [1..125 as v] -> write [{v+1}] stop;
    [126]         -> halt;
    [0]           -> write [1] stop;
  }
}
";
        let rules = machine_rules(src, 0);
        let glyphs: Vec<String> = (0..=126).map(|v| v.to_string()).collect();
        let tapes = vec![TapeInfo::new(&glyphs)];
        let mut warn = Vec::new();
        let rows = expand_rule(&rules[0], &tapes, &mut warn, &own_tr).unwrap();
        assert_eq!(rows.len(), 125);
        // value == index for this alphabet: v reads index v, writes v+1.
        assert_eq!(rows[0].pattern, vec![Cell::Sym(1)]);
        assert_eq!(rows[0].write, vec![WriteOut::Sym(2)]);
        assert_eq!(rows[124].pattern, vec![Cell::Sym(125)]);
        assert_eq!(rows[124].write, vec![WriteOut::Sym(126)]);
        // The `[126] -> halt` and `[0] -> write [1]` rows are singletons.
        let halt = expand_rule(&rules[1], &tapes, &mut warn, &own_tr).unwrap();
        assert_eq!(halt.len(), 1);
        assert_eq!(halt[0].transition, Transition2::Halt);
    }

    #[test]
    fn fold_below_or_above_the_alphabet_errors() {
        let src = "\
alphabet three { 0..2 }
machine {
  tape cell: three;
  entry state s { [1 as v] -> write [{v+2}] stop; }
}
";
        let rules = machine_rules(src, 0);
        let tapes = vec![TapeInfo::new(&["0".into(), "1".into(), "2".into()])];
        let mut warn = Vec::new();
        let err = expand_rule(&rules[0], &tapes, &mut warn, &own_tr).unwrap_err();
        assert_eq!(err.kind.code(), "fold-out-of-alphabet");
    }

    #[test]
    fn a_row_width_mismatch_is_caught() {
        // Two tapes but a one-wide pattern.
        let src = "\
alphabet bits { '_', '0', '1' }
machine {
  tape a: bits;
  tape b: bits;
  entry state s { ['0'] -> stop; }
}
";
        let rules = machine_rules(src, 0);
        let tapes = vec![ti(&["_", "0", "1"]), ti(&["_", "0", "1"])];
        let mut warn = Vec::new();
        let err = expand_rule(&rules[0], &tapes, &mut warn, &own_tr).unwrap_err();
        assert_eq!(err.kind.code(), "row-width");
    }
}

#[cfg(test)]
mod oracle_tests {
    //! The load-bearing guard (the plan's oracle): a graft instance's spliced
    //! rows, run first-match over every concrete host tuple, agree with walking
    //! the ORIGINAL graph rules through the symbol maps per symbol (read
    //! host→graph, hole ⇒ trap-read; match the graph rules; write graph→host,
    //! hole ⇒ trap-write). The same map model both sides — this proves the
    //! splice's preimage expansion, first-match ordering, and trap synthesis
    //! preserve the graph's semantics under any (holey, one-way, collapsing)
    //! binding.
    use super::*;
    use proptest::prelude::*;

    /// The observable of one host tuple: a trap, a fired source rule (with its
    /// projected write/move), or no match.
    #[derive(Debug, PartialEq, Eq)]
    enum Outcome {
        TrapRead,
        TrapWrite,
        Fired(usize, Vec<WriteOut>, Vec<MoveDir>),
        NoMatch,
    }

    fn host_matches(pattern: &[Cell], tuple: &[u16]) -> bool {
        pattern.iter().zip(tuple).all(|(c, &t)| match c {
            Cell::Wild => true,
            Cell::Sym(s) => *s == t,
        })
    }

    fn graph_matches(pattern: &[Cell], g: &[u16]) -> bool {
        pattern.iter().zip(g).all(|(c, &gv)| match c {
            Cell::Wild => true,
            Cell::Sym(s) => *s == gv,
        })
    }

    /// (a) — first-match over the spliced rows.
    fn actual(spliced: &[ExpandedRule], tuple: &[u16]) -> Outcome {
        for r in spliced {
            if host_matches(&r.pattern, tuple) {
                return match &r.transition {
                    Transition2::TrapRead => Outcome::TrapRead,
                    Transition2::TrapWrite => Outcome::TrapWrite,
                    Transition2::Goto(n) => {
                        Outcome::Fired(n.parse().unwrap(), r.write.clone(), r.moves.clone())
                    }
                    other => panic!("unexpected transition {other:?}"),
                };
            }
        }
        Outcome::NoMatch
    }

    /// Project a fired graph rule's write/move to host width; a write hole makes
    /// it a trap-write (the same formula [`map_rule`] uses).
    fn classify(idx: usize, r: &ExpandedRule, comp: &Composite, host_arity: usize) -> Outcome {
        let mut write_hole = false;
        let mut hw = vec![WriteOut::Keep; host_arity];
        let mut hm = vec![MoveDir::Stay; host_arity];
        for (k, t) in comp.tapes.iter().enumerate() {
            if let WriteOut::Sym(gv) = r.write[k] {
                match t.write_image(gv) {
                    Some(p) => hw[t.phys] = WriteOut::Sym(p),
                    None => write_hole = true,
                }
            }
            hm[t.phys] = r.moves[k];
        }
        if write_hole {
            Outcome::TrapWrite
        } else {
            Outcome::Fired(idx, hw, hm)
        }
    }

    /// (b) — walk the original graph rules through the maps per symbol.
    fn reference(state: &ExpandedState, comp: &Composite, tuple: &[u16], cond: bool) -> Outcome {
        let host_arity = tuple.len();
        if !cond {
            // Straight-line: no `rd`, the single all-wildcard rule fires.
            return classify(0, &state.rules[0], comp, host_arity);
        }
        let mut g = vec![0u16; comp.tapes.len()];
        for (k, t) in comp.tapes.iter().enumerate() {
            match t.read_image(tuple[t.phys]) {
                None => return Outcome::TrapRead, // first hole in tape order
                Some(v) => g[k] = v,
            }
        }
        for (idx, r) in state.rules.iter().enumerate() {
            if graph_matches(&r.pattern, &g) {
                return classify(idx, r, comp, host_arity);
            }
        }
        Outcome::NoMatch
    }

    /// A per-tape map: identity default, some remaps, some explicit holes.
    /// `dst`/`whole` range beyond the graph alphabet to also mint cardinality
    /// holes. Blank (0) stays identity, non-hole (a realistic pinned blank).
    fn tape(
        host_card: usize,
        graph_card: usize,
        rdst: &[u16],
        rhole: &[bool],
        wdst: &[u16],
        whole: &[bool],
        phys: usize,
    ) -> TapeMap {
        let mut rmap = SymMap::identity();
        for s in 1..host_card {
            if rhole[s] {
                rmap.holes.insert(s as u16);
            } else if usize::from(rdst[s]) != s {
                rmap.pairs.insert(s as u16, rdst[s]);
            }
        }
        let mut wmap = SymMap::identity();
        for v in 1..graph_card {
            if whole[v] {
                wmap.holes.insert(v as u16);
            } else if usize::from(wdst[v]) != v {
                wmap.pairs.insert(v as u16, wdst[v]);
            }
        }
        TapeMap {
            phys,
            host_card,
            graph_card,
            rmap,
            wmap,
        }
    }

    /// One tape's random spec: cards plus remap/hole vectors sized to the cards.
    fn tape_spec() -> impl Strategy<Value = (usize, usize, Vec<u16>, Vec<bool>, Vec<u16>, Vec<bool>)>
    {
        (2usize..=4, 2usize..=4).prop_flat_map(|(hc, gc)| {
            (
                Just(hc),
                Just(gc),
                proptest::collection::vec(0u16..6, hc),
                proptest::collection::vec(any::<bool>(), hc),
                proptest::collection::vec(0u16..6, gc),
                proptest::collection::vec(any::<bool>(), gc),
            )
        })
    }

    /// A graph rule: per tape a wildcard or a concrete graph symbol; a write
    /// keep or a concrete graph symbol; a move. Symbols stay in each tape's
    /// graph alphabet. The transition encodes the source rule index.
    fn rule_spec(
        gcards: Vec<usize>,
    ) -> impl Strategy<Value = (Vec<Option<u16>>, Vec<Option<u16>>, Vec<u8>)> {
        let pat: Vec<_> = gcards
            .iter()
            .map(|&gc| proptest::option::of(0u16..gc as u16))
            .collect();
        let wr: Vec<_> = gcards
            .iter()
            .map(|&gc| proptest::option::of(0u16..gc as u16))
            .collect();
        let mv: Vec<_> = gcards.iter().map(|_| 0u8..3).collect();
        (pat, wr, mv)
    }

    fn mv_of(n: u8) -> MoveDir {
        match n {
            0 => MoveDir::Left,
            1 => MoveDir::Right,
            _ => MoveDir::Stay,
        }
    }

    fn build_rule(idx: usize, pat: &[Option<u16>], wr: &[Option<u16>], mv: &[u8]) -> ExpandedRule {
        ExpandedRule {
            pattern: pat
                .iter()
                .map(|o| o.map_or(Cell::Wild, Cell::Sym))
                .collect(),
            debugger: false,
            write: wr
                .iter()
                .map(|o| o.map_or(WriteOut::Keep, WriteOut::Sym))
                .collect(),
            moves: mv.iter().map(|&n| mv_of(n)).collect(),
            transition: Transition2::Goto(idx.to_string()),
            span: Span::point(1, 1),
        }
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(400))]
        #[test]
        fn graft_splice_matches_per_symbol_walk(
            specs in proptest::collection::vec(tape_spec(), 1..=2),
            n_rules in 1usize..=4,
            seed in any::<u64>(),
        ) {
            let arity = specs.len();
            let comp = Composite {
                tapes: specs
                    .iter()
                    .enumerate()
                    .map(|(k, (hc, gc, rdst, rhole, wdst, whole))| {
                        tape(*hc, *gc, rdst, rhole, wdst, whole, k)
                    })
                    .collect(),
            };
            let host_cards: Vec<usize> = specs.iter().map(|(hc, ..)| *hc).collect();
            let graph_cards: Vec<usize> = specs.iter().map(|(_, gc, ..)| *gc).collect();

            // Deterministic pseudo-random graph rules from `seed` (a fresh
            // proptest sub-generation would need nested runners; a splitmix
            // walk over the seed keeps it flat and reproducible).
            let mut st = seed;
            let mut next = || {
                st = st.wrapping_add(0x9E3779B97F4A7C15);
                let mut z = st;
                z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
                z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB);
                z ^ (z >> 31)
            };
            let mut rules = Vec::new();
            for idx in 0..n_rules {
                let mut pat = Vec::new();
                let mut wr = Vec::new();
                let mut mv = Vec::new();
                for &gc in &graph_cards {
                    // wildcard 1-in-3, else a concrete symbol.
                    pat.push(if next() % 3 == 0 { None } else { Some((next() % gc as u64) as u16) });
                    wr.push(if next() % 2 == 0 { None } else { Some((next() % gc as u64) as u16) });
                    mv.push((next() % 3) as u8);
                }
                rules.push(build_rule(idx, &pat, &wr, &mv));
            }
            let state = ExpandedState {
                name: "s".into(),
                name_span: Span::point(1, 1),
                rules,
            };
            let cond = state_reads(&state.rules);
            let spliced = splice_state(&state, &comp, arity, "s", &|t| t.clone());

            // Enumerate every host tuple and compare.
            let total: usize = host_cards.iter().product();
            for n in 0..total {
                let mut tuple = vec![0u16; arity];
                let mut rem = n;
                for k in 0..arity {
                    tuple[k] = (rem % host_cards[k]) as u16;
                    rem /= host_cards[k];
                }
                let a = actual(&spliced.rules, &tuple);
                let b = reference(&state, &comp, &tuple, cond);
                prop_assert_eq!(&a, &b, "tuple {:?}: splice {:?} vs walk {:?}", tuple, a, b);
            }
        }
    }
}
