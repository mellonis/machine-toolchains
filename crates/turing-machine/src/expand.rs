//! `.tmc` front-end stage 2 — graft splicing + range expansion, the
//! compiler-side analog of the linker's mono stamping.
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
//! `compile()` wires `expand` into the pipeline (IR lowering consumes its
//! output).

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use mtc_core::diagnostics::{Diagnostic, Span};

use crate::compiler::{
    CompileError, CompileErrorKind, Resolved, ResolvedAlphabet, ResolvedCallTarget, ResolvedGraft,
    ResolvedWorld, WorldKind,
};
use crate::parser::{
    BindingArg, BindingValue, Continuation, FoldExprKind, FoldExprNode, FoldOp, MapArrow, MoveCell,
    MoveDir, MoveVec, PatternCell, PatternCellKind, Rule, SymLit, SymMap as SrcSymMap, TermKind,
    Transition, WriteCell, WriteCellKind, WriteVec,
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
// `compose.rs` is core-private). Equal-size alphabets identity-complete
// unlisted symbols (with the injectivity check); across differently-sized
// alphabets the map is CLOSED — every unlisted non-blank source is a hole (no
// identity completion). One-way `=>` collapse excluded from write-back, blank
// pinned. The holes live in the SymMap (minted by `close_unlisted` at build
// time), so the read/write image methods stay pure lookups.
// ---------------------------------------------------------------------------

/// A partial symbol map, identity for unlisted symbols; `holes` trap. Mirrors
/// the linker's `SparseMap` (docs/core.md (the composition engine)).
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

// ---------------------------------------------------------------------------
// The splice — map a graph's (already range-expanded, graph-space) states to
// host width through a [`Composite`], the compiler-side analog of
// `crates/core/src/linker/stamp.rs::build_stamp`. Pattern cells map by the
// read PREIMAGE (multi-preimage expands rows, zero-preimage drops), write cells
// by the write image (a hole turns the whole rule into a `trap #1` row), and
// each bound tape's read holes prepend `trap #0` rows to every conditional
// state (a straight-line state does no `rd`, so a hole under it never traps).
// ---------------------------------------------------------------------------

/// True when every cell is a wildcard.
fn is_all_wild(pattern: &[Cell]) -> bool {
    pattern.iter().all(|c| matches!(c, Cell::Wild))
}

/// True when the state performs a read (`rd` + match): anything but a single
/// all-wildcard rule, which codegen lowers to straight-line code. Only a
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
// Range expansion — one source rule → concrete index-resolved
// rows. Pattern ranges / single-with-binding expand cartesian (leftmost tape
// varies slowest, rightmost fastest — matching the linker's preimage
// cartesian); `{v±k}` folds per row (numeric, bounds-checked), `{c}` passes the
// bound glyph through; a range value with no glyph on the tape drops that
// alternative. Product over 256 warns.
// ---------------------------------------------------------------------------

/// The product-count above which a rule's expansion warns. Shared with the
/// `.tmc` lint's `binding-product-threshold` rule, the source-level mirror of
/// this same warning, so the two agree on the cutoff.
pub(crate) const PRODUCT_THRESHOLD: usize = 256;

/// One tape's resolution context: the inverse lookup (glyph → index).
struct TapeInfo {
    index: HashMap<String, u16>,
}

impl TapeInfo {
    fn new(glyphs: &[String]) -> Self {
        let index = glyphs
            .iter()
            .enumerate()
            .map(|(i, g)| (g.clone(), i as u16))
            .collect();
        Self { index }
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
    lower_tr: &mut impl FnMut(&Transition) -> Transition2,
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
        WriteCellKind::Subst { expr } => {
            // Transitional bridge while fold-expression evaluation is under
            // construction: only a bare name and `{name±int}` fold here; any
            // richer expression is a clear error, never silently wrong output.
            let (name, name_span, delta) = simple_fold(expr)?;
            let bv = env.get(name).ok_or_else(|| CompileError {
                span: name_span,
                kind: CompileErrorKind::FoldOutOfAlphabet(format!(
                    "`{{{name}}}` refers to no pattern binding in this rule"
                )),
            })?;
            let glyph = if delta == 0 {
                bv.glyph.clone()
            } else {
                // Arithmetic is numeric-only (the parser rejects `{c±k}`).
                let base = bv.value.ok_or_else(|| CompileError {
                    span: name_span,
                    kind: CompileErrorKind::FoldOutOfAlphabet(format!(
                        "`{{{name}}}` binds a glyph, which cannot take arithmetic"
                    )),
                })?;
                let folded = base + delta;
                if folded < 0 {
                    return Err(CompileError {
                        span: name_span,
                        kind: CompileErrorKind::FoldOutOfAlphabet(format!(
                            "`{{{name}{delta:+}}}` folds to {folded}, below the alphabet"
                        )),
                    });
                }
                folded.to_string()
            };
            ti.idx(&glyph).map(WriteOut::Sym).ok_or(CompileError {
                span: name_span,
                kind: CompileErrorKind::FoldOutOfAlphabet(format!(
                    "`{{{name}{delta:+}}}` folds to `{glyph}`, not in the tape's alphabet"
                )),
            })
        }
    }
}

/// Extract `(name, name_span, delta)` from the two fold shapes this
/// transitional bridge evaluates — a bare `{name}` (delta 0) and `{name±int}`
/// — rejecting anything richer with a clear, spanned error. Replaced by full
/// fold-expression evaluation once that lands.
fn simple_fold(expr: &FoldExprNode) -> Result<(&str, Span, i64), CompileError> {
    let unsupported = || CompileError {
        span: expr.span,
        kind: CompileErrorKind::FoldExprUnsupported(
            "this fold expression is not yet evaluated — only a bare name or `{name±int}` folds \
             for now"
                .to_string(),
        ),
    };
    match &expr.kind {
        FoldExprKind::Var(name) => Ok((name.as_str(), expr.span, 0)),
        FoldExprKind::Bin { op, lhs, rhs }
            if matches!(op, FoldOp::Add | FoldOp::Sub)
                && matches!(lhs.kind, FoldExprKind::Var(_))
                && matches!(rhs.kind, FoldExprKind::Int(_)) =>
        {
            let FoldExprKind::Var(name) = &lhs.kind else {
                unreachable!("guarded by the match arm")
            };
            let FoldExprKind::Int(n) = rhs.kind else {
                unreachable!("guarded by the match arm")
            };
            let delta = if *op == FoldOp::Sub { -n } else { n };
            Ok((name.as_str(), lhs.span, delta))
        }
        _ => Err(unsupported()),
    }
}

/// Resolve a move vector to per-tape [`MoveDir`] (`Stay` default when omitted).
fn resolve_moves(mov: Option<&MoveVec>, arity: usize) -> Vec<MoveDir> {
    match mov {
        None => vec![MoveDir::Stay; arity],
        Some(mv) => mv.cells.iter().map(|c: &MoveCell| c.dir).collect(),
    }
}

// ---------------------------------------------------------------------------
// Building a graft composite from source binding args (the map legality
// contract, spanned: blank pinning, per-direction conflicts, equal-size
// identity-completion injectivity, omitted-map identity requiring equal
// glyphs). Mirrors `crates/core/src/linker/{compose,engine}.rs`.
// ---------------------------------------------------------------------------

/// Insert one read/write map pair, rejecting a repeat with a different image.
fn insert_map_pair(
    map: &mut SymMap,
    src: u16,
    dst: u16,
    glyph: &str,
    span: Span,
) -> Result<(), CompileError> {
    match map.pairs.get(&src) {
        Some(&e) if e != dst => Err(CompileError {
            span,
            kind: CompileErrorKind::MapConflict {
                symbol: glyph.to_string(),
            },
        }),
        _ => {
            map.pairs.insert(src, dst);
            Ok(())
        }
    }
}

/// Close a map over its `domain_card` source symbols: every non-blank source
/// index below `domain_card` that no explicit pair names becomes a hole
/// (docs/formats.md (bound calls) — the closed-on-unequal rule; identity
/// completion exists only for equal-size alphabets). Blank stays pinned.
/// Called before the identity-pair `retain` in [`build_tapemap`], so an
/// explicit `k->k` (still in `pairs`) keeps `k` out of the hole set and
/// survives as identity, while a symbol the binding never named traps.
fn close_unlisted(map: &mut SymMap, domain_card: usize) {
    let listed: BTreeSet<u16> = map.pairs.keys().copied().collect();
    let upper = domain_card.min(usize::from(u16::MAX));
    for s in 1..upper {
        let s16 = s as u16;
        if !listed.contains(&s16) {
            map.holes.insert(s16);
        }
    }
}

/// Build one bound tape's [`TapeMap`] from its (optional) source symbol map,
/// resolving `src` glyphs against the host alphabet and `dst` glyphs against
/// the graph alphabet.
fn build_tapemap(
    map: Option<&SrcSymMap>,
    phys: usize,
    host_glyphs: &[String],
    graph_glyphs: &[String],
    span: Span,
) -> Result<TapeMap, CompileError> {
    let host_card = host_glyphs.len();
    let graph_card = graph_glyphs.len();
    let Some(m) = map else {
        // Omitted map = identity, which requires glyph-for-glyph equal tapes.
        if host_glyphs != graph_glyphs {
            return Err(CompileError {
                span,
                kind: CompileErrorKind::IdentityGlyphMismatch,
            });
        }
        return Ok(TapeMap {
            phys,
            host_card,
            graph_card,
            rmap: SymMap::identity(),
            wmap: SymMap::identity(),
        });
    };

    let host_idx: HashMap<&str, u16> = host_glyphs
        .iter()
        .enumerate()
        .map(|(i, g)| (g.as_str(), i as u16))
        .collect();
    let graph_idx: HashMap<&str, u16> = graph_glyphs
        .iter()
        .enumerate()
        .map(|(i, g)| (g.as_str(), i as u16))
        .collect();

    let mut rmap = SymMap::identity();
    let mut wmap = SymMap::identity();
    let mut bidir: Vec<(u16, u16)> = Vec::new();
    for pair in &m.pairs {
        let src_g = glyph_label(&pair.src);
        let dst_g = glyph_label(&pair.dst);
        let src = *host_idx.get(src_g.as_str()).ok_or(CompileError {
            span: pair.src.span(),
            kind: CompileErrorKind::MapSymbolNotInAlphabet(src_g.clone()),
        })?;
        let dst = *graph_idx.get(dst_g.as_str()).ok_or(CompileError {
            span: pair.dst.span(),
            kind: CompileErrorKind::MapSymbolNotInAlphabet(dst_g.clone()),
        })?;
        // Blank reads as blank.
        if src == 0 && dst != 0 {
            return Err(CompileError {
                span: pair.span,
                kind: CompileErrorKind::MapBlankPin,
            });
        }
        insert_map_pair(&mut rmap, src, dst, &src_g, pair.span)?;
        if pair.arrow == MapArrow::Bidirectional {
            // A two-way fold onto blank would write blank back as non-blank.
            if dst == 0 && src != 0 {
                return Err(CompileError {
                    span: pair.span,
                    kind: CompileErrorKind::MapBlankPin,
                });
            }
            insert_map_pair(&mut wmap, dst, src, &dst_g, pair.span)?;
            bidir.push((src, dst));
        }
    }
    // Closed-on-unequal (docs/formats.md (bound calls)): identity completion
    // exists only for equal-size alphabets. Across differently-sized tapes
    // every non-blank source the map does not name is a hole — computed from
    // the explicit srcs still in `pairs`, before the identity-pair retain
    // below, so an explicit `k->k` survives as identity while a truly absent
    // symbol traps.
    if host_card != graph_card {
        close_unlisted(&mut rmap, host_card);
        close_unlisted(&mut wmap, graph_card);
    }
    // Canonical: drop identity pairs (stable dedup keys).
    rmap.pairs.retain(|s, d| s != d);
    wmap.pairs.retain(|s, d| s != d);

    // Equal-size alphabets must identity-complete to a bijection: the
    // BIDIRECTIONAL read map, filled with identity, must be injective.
    if host_card == graph_card {
        let bmap: HashMap<u16, u16> = bidir.into_iter().collect();
        let mut seen: HashSet<u16> = HashSet::new();
        for s in 0..host_card as u16 {
            let v = bmap.get(&s).copied().unwrap_or(s);
            if !seen.insert(v) {
                let g = graph_glyphs
                    .get(usize::from(v))
                    .cloned()
                    .unwrap_or_else(|| v.to_string());
                return Err(CompileError {
                    span,
                    kind: CompileErrorKind::MapNotInjective { symbol: g },
                });
            }
        }
    }
    Ok(TapeMap {
        phys,
        host_card,
        graph_card,
        rmap,
        wmap,
    })
}

/// Build a graft's [`Composite`] (per graph tape) plus its continuation
/// substitution (graph state-param → the host continuation the graft binds).
/// The T4 world checks guarantee every parameter is bound with a
/// kind-correct argument, so the "impossible" branches are invariants.
fn build_composite(
    graft: &ResolvedGraft,
    host: &ResolvedWorld,
    graph: &ResolvedWorld,
    alphabets: &HashMap<String, ResolvedAlphabet>,
) -> Result<(Composite, HashMap<String, Transition2>), CompileError> {
    let args: HashMap<&str, &BindingArg> =
        graft.args.iter().map(|a| (a.name.as_str(), a)).collect();

    let mut tapes = Vec::with_capacity(graph.tapes.len());
    for gt in &graph.tapes {
        let arg = args
            .get(gt.name.as_str())
            .expect("T4 binds every tape parameter");
        let BindingValue::Named { target, map, .. } = &arg.value else {
            unreachable!("T4 gives a tape parameter a named tape target");
        };
        let phys = host
            .tapes
            .iter()
            .position(|t| &t.name == target)
            .expect("T4 resolves the tape target to a host tape");
        let host_glyphs = &alphabets[&host.tapes[phys].alphabet].glyphs;
        let graph_glyphs = &alphabets[&gt.alphabet].glyphs;
        tapes.push(build_tapemap(
            map.as_ref(),
            phys,
            host_glyphs,
            graph_glyphs,
            arg.span,
        )?);
    }

    let mut cont: HashMap<String, Transition2> = HashMap::new();
    for sp in &graph.state_params {
        let arg = args
            .get(sp.as_str())
            .expect("T4 binds every state parameter");
        let t2 = match &arg.value {
            BindingValue::Named { target, .. } => Transition2::Goto(target.clone()),
            BindingValue::Terminator { kind, .. } => match kind {
                TermKind::Return => Transition2::Return,
                TermKind::Stop => Transition2::Stop,
                TermKind::Halt => Transition2::Halt,
            },
        };
        cont.insert(sp.clone(), t2);
    }
    Ok((Composite { tapes }, cont))
}

/// A deterministic key for a continuation substitution (dedup input).
fn cont_key(cont: &HashMap<String, Transition2>) -> Vec<u8> {
    let mut entries: Vec<(&String, String)> = cont
        .iter()
        .map(|(k, v)| {
            let r = match v {
                Transition2::Goto(n) => format!("g{n}"),
                Transition2::Return => "r".into(),
                Transition2::Stop => "s".into(),
                Transition2::Halt => "h".into(),
                _ => "?".into(),
            };
            (k, r)
        })
        .collect();
    entries.sort();
    let mut out = Vec::new();
    for (k, r) in entries {
        out.extend_from_slice(k.as_bytes());
        out.push(b'=');
        out.extend_from_slice(r.as_bytes());
        out.push(0);
    }
    out
}

// ---------------------------------------------------------------------------
// Graph-definition acyclicity: the graft-dependency graph of
// graph DEFINITIONS must be acyclic (a self- or mutual graft is infinite
// expansion). Instance-level cycles (continuation loops) stay legal.
// ---------------------------------------------------------------------------

fn check_graph_acyclicity(graphs: &HashMap<&str, &ResolvedWorld>) -> Result<(), CompileError> {
    // 0 = unvisited, 1 = on the current path, 2 = done.
    let mut color: HashMap<&str, u8> = HashMap::new();
    for &name in graphs.keys() {
        if color.get(name).copied().unwrap_or(0) == 0 {
            acyclicity_dfs(name, graphs, &mut color)?;
        }
    }
    Ok(())
}

fn acyclicity_dfs<'a>(
    name: &'a str,
    graphs: &HashMap<&'a str, &'a ResolvedWorld>,
    color: &mut HashMap<&'a str, u8>,
) -> Result<(), CompileError> {
    color.insert(name, 1);
    let world = graphs[name];
    for graft in &world.grafts {
        // A graft target is always a locally-defined graph (T4's
        // `undefined-graph`); look up its canonical key in `graphs`.
        let Some((&target, _)) = graphs.get_key_value(graft.target.as_str()) else {
            continue;
        };
        match color.get(target).copied().unwrap_or(0) {
            1 => {
                return Err(CompileError {
                    span: graft.target_span,
                    kind: CompileErrorKind::GraftCycle(graft.target.clone()),
                });
            }
            0 => acyclicity_dfs(target, graphs, color)?,
            _ => {}
        }
    }
    color.insert(name, 2);
    Ok(())
}

// ---------------------------------------------------------------------------
// World expansion — own states (range-expanded) + spliced graft instances,
// with instance dedup and alias resolution. Graphs expand to a memoized
// graph-space form; the machine and routines emit as `ExpandedWorld`s.
// ---------------------------------------------------------------------------

/// A graph's expanded graph-space form: its concrete states (own + nested
/// grafts spliced, still over the graph's own tapes) and its resolved entry
/// state name. Cached across graft instances.
#[derive(Debug, Clone)]
struct GraphExpansion {
    states: Vec<ExpandedState>,
    entry: String,
}

/// A collision-proof world-local name minter: seeded with the world's user
/// names, each `fresh` returns a name absent from every prior user or minted
/// name (bumping a numeric suffix), so synthetic instance internals never
/// clash with a user state or another instance.
struct NameGen {
    used: HashSet<String>,
}

impl NameGen {
    fn new<I: IntoIterator<Item = String>>(reserved: I) -> Self {
        Self {
            used: reserved.into_iter().collect(),
        }
    }

    fn fresh(&mut self, base: &str) -> String {
        if self.used.insert(base.to_string()) {
            return base.to_string();
        }
        let mut i = 1;
        loop {
            let cand = format!("{base}_{i}");
            if self.used.insert(cand.clone()) {
                return cand;
            }
            i += 1;
        }
    }
}

fn tape_infos(
    world: &ResolvedWorld,
    alphabets: &HashMap<String, ResolvedAlphabet>,
) -> Vec<TapeInfo> {
    world
        .tapes
        .iter()
        .map(|t| TapeInfo::new(&alphabets[&t.alphabet].glyphs))
        .collect()
}

/// A world's user-visible state-name space (own states, graft instances,
/// state params) — the reserved set synthetic names must avoid.
fn reserved_names(world: &ResolvedWorld) -> Vec<String> {
    let mut names: Vec<String> = world.states.iter().map(|s| s.name.clone()).collect();
    names.extend(world.grafts.iter().filter_map(|g| g.as_name.clone()));
    names.extend(world.state_params.iter().cloned());
    names
}

/// Range-expand a world's OWN states (not graft instances), lowering each
/// rule's transition (calls resolved through the world's call records, in
/// source order).
fn expand_own_states(
    world: &ResolvedWorld,
    tapes: &[TapeInfo],
    warn: &mut Vec<Diagnostic>,
    out: &mut Vec<ExpandedState>,
) -> Result<(), CompileError> {
    let calls = &world.calls;
    let mut cursor = 0usize;
    for s in &world.states {
        let mut lower = |t: &Transition| -> Transition2 {
            match t {
                Transition::Goto { name, .. } => Transition2::Goto(name.clone()),
                Transition::Return { .. } => Transition2::Return,
                Transition::Stop { .. } => Transition2::Stop,
                Transition::Halt { .. } => Transition2::Halt,
                Transition::Call { .. } => {
                    let rc = &calls[cursor];
                    cursor += 1;
                    match &rc.target {
                        ResolvedCallTarget::Routine {
                            name,
                            external,
                            args,
                        } => Transition2::Call {
                            target: name.clone(),
                            external: *external,
                            args: args.clone(),
                            then: rc.then.clone(),
                        },
                        ResolvedCallTarget::Bind { name } => Transition2::BindCall {
                            name: name.clone(),
                            then: rc.then.clone(),
                        },
                    }
                }
            }
        };
        let mut rules = Vec::new();
        for r in &s.rules {
            rules.extend(expand_rule(r, tapes, warn, &mut lower)?);
        }
        out.push(ExpandedState {
            name: s.name.clone(),
            name_span: s.name_span,
            rules,
        });
    }
    Ok(())
}

/// Splice every graft instance of `host` into `out`, deduping identical
/// (graph, binding, continuations) instances (their names alias one entry).
/// Returns the entry-graft instance's entry-state host name, if any.
#[allow(clippy::too_many_arguments)]
fn expand_grafts_into<'a>(
    host: &ResolvedWorld,
    resolved: &'a Resolved,
    graphs: &HashMap<&'a str, &'a ResolvedWorld>,
    memo: &mut HashMap<String, GraphExpansion>,
    warn: &mut Vec<Diagnostic>,
    namegen: &mut NameGen,
    out: &mut Vec<ExpandedState>,
    alias: &mut HashMap<String, String>,
) -> Result<Option<String>, CompileError> {
    let mut dedup: HashMap<Vec<u8>, String> = HashMap::new();
    let mut graft_entry: Option<String> = None;

    for graft in &host.grafts {
        let graph = graphs[graft.target.as_str()];
        let gx = expand_graph(graft.target.as_str(), graphs, resolved, memo, warn)?;
        // A grafted graph body must not contain a call yet — splicing it into
        // host space needs binding composition (not implemented). Detect
        // before emitting anything; the graft-site span names the
        // instantiation that can't be done, the message names the call.
        if let Some(call) = first_grafted_call(&gx.states) {
            return Err(CompileError {
                span: graft.target_span,
                kind: CompileErrorKind::GraftCallUnsupported(call.to_string()),
            });
        }
        let (comp, cont) = build_composite(graft, host, graph, &resolved.alphabets)?;

        let mut key = graft.target.clone().into_bytes();
        key.push(0);
        key.extend(comp.key());
        key.push(0);
        key.extend(cont_key(&cont));

        let instance = match &graft.as_name {
            Some(n) => n.clone(),
            None => namegen.fresh(&format!("{}__entry", sanitize(&graft.target))),
        };

        if let Some(canonical) = dedup.get(&key) {
            // Identical instance: alias this name to the existing entry state.
            alias.insert(instance.clone(), canonical.clone());
            if graft.entry {
                graft_entry = Some(canonical.clone());
            }
            continue;
        }

        // A fresh splice: map every graph-space state name to a host name
        // (the entry → the instance name; internals → collision-proof synth).
        let mut name_map: HashMap<String, String> = HashMap::new();
        for st in &gx.states {
            let host_name = if st.name == gx.entry {
                instance.clone()
            } else {
                namegen.fresh(&format!("{instance}__{}", st.name))
            };
            name_map.insert(st.name.clone(), host_name);
        }
        let params: HashSet<&str> = graph.state_params.iter().map(String::as_str).collect();
        let host_arity = host.tapes.len();

        let remap = |t2: &Transition2| -> Transition2 {
            match t2 {
                Transition2::Goto(n) => {
                    if let Some(c) = cont.get(n) {
                        c.clone()
                    } else if params.contains(n.as_str()) {
                        // A state param with no binding arg — T4 forbids this;
                        // pass through defensively.
                        t2.clone()
                    } else {
                        Transition2::Goto(name_map.get(n).cloned().unwrap_or_else(|| n.clone()))
                    }
                }
                // Return / Stop / Halt / the trap markers carry no name to
                // rewrite. Call / BindCall would need their binding args and
                // continuation rewritten into host space, but the guard above
                // rejects a call-bearing graph before any splice, so they
                // cannot reach here.
                other => other.clone(),
            }
        };

        for st in &gx.states {
            let host_name = &name_map[&st.name];
            out.push(splice_state(st, &comp, host_arity, host_name, &remap));
        }
        dedup.insert(key, instance.clone());
        if graft.entry {
            graft_entry = Some(instance);
        }
    }
    Ok(graft_entry)
}

/// A synthetic-name base from a mangled graph name (bare-label safe).
fn sanitize(name: &str) -> String {
    name.replace("::", "_").replace([':', '.'], "_")
}

/// The target of the first `call` in a graph's expanded body (routine call or
/// bind call), in state-then-rule order, or `None`. A grafted graph body
/// carrying a call cannot be spliced yet: its binding args still name the
/// graph's signature tapes and its continuation is a graph-space state, and
/// the binding composition that would rewrite both into the host is not
/// implemented. The scan runs at SPLICE time only, so a graph that carries a
/// call but is never grafted stays legal and dead (it is never expanded).
fn first_grafted_call(states: &[ExpandedState]) -> Option<&str> {
    states
        .iter()
        .flat_map(|st| &st.rules)
        .find_map(|r| match &r.transition {
            Transition2::Call { target, .. } => Some(target.as_str()),
            Transition2::BindCall { name, .. } => Some(name.as_str()),
            _ => None,
        })
}

/// Expand a graph to its memoized graph-space form: own states range-expanded,
/// nested grafts spliced, aliases resolved. The graft-dependency DAG being
/// acyclic, the recursion terminates.
fn expand_graph<'a>(
    name: &str,
    graphs: &HashMap<&'a str, &'a ResolvedWorld>,
    resolved: &'a Resolved,
    memo: &mut HashMap<String, GraphExpansion>,
    warn: &mut Vec<Diagnostic>,
) -> Result<GraphExpansion, CompileError> {
    if let Some(gx) = memo.get(name) {
        return Ok(gx.clone());
    }
    let world = graphs[name];
    let tapes = tape_infos(world, &resolved.alphabets);
    let mut states = Vec::new();
    expand_own_states(world, &tapes, warn, &mut states)?;
    let mut namegen = NameGen::new(reserved_names(world));
    let mut alias = HashMap::new();
    let graft_entry = expand_grafts_into(
        world,
        resolved,
        graphs,
        memo,
        warn,
        &mut namegen,
        &mut states,
        &mut alias,
    )?;
    resolve_aliases(&mut states, &alias);
    let entry = world_entry(world, graft_entry, &alias);
    let gx = GraphExpansion { states, entry };
    memo.insert(name.to_string(), gx.clone());
    Ok(gx)
}

/// A world's entry-state name: its own `entry state`, or its `entry graft`'s
/// spliced entry (resolved through the dedup alias).
fn world_entry(
    world: &ResolvedWorld,
    graft_entry: Option<String>,
    alias: &HashMap<String, String>,
) -> String {
    let own = world
        .states
        .iter()
        .find(|s| s.entry)
        .map(|s| s.name.clone());
    let e = own
        .or(graft_entry)
        .expect("T4 gives every world exactly one entry");
    resolve_alias(&e, alias)
}

fn resolve_alias(name: &str, alias: &HashMap<String, String>) -> String {
    alias.get(name).cloned().unwrap_or_else(|| name.to_string())
}

/// Rewrite every transition target (goto / call continuation) through the
/// dedup alias, so a goto onto a deduplicated instance reaches the survivor.
fn resolve_aliases(states: &mut [ExpandedState], alias: &HashMap<String, String>) {
    if alias.is_empty() {
        return;
    }
    for st in states {
        for r in &mut st.rules {
            match &mut r.transition {
                Transition2::Goto(n) => *n = resolve_alias(n, alias),
                Transition2::Call { then, .. } | Transition2::BindCall { then, .. } => {
                    if let Continuation::State { name, span } = then {
                        *then = Continuation::State {
                            name: resolve_alias(name, alias),
                            span: *span,
                        };
                    }
                }
                _ => {}
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Post-expansion checks: exact-row disjointness (docs/tmt/isa.md (match and
// dispatch); a spanned error naming both rules) and cheap shadowed-rule
// warnings (an identical earlier pattern makes a later rule unreachable).
// ---------------------------------------------------------------------------

fn render_pattern(pattern: &[Cell], glyphs: &[Vec<String>]) -> String {
    let cells: Vec<String> = pattern
        .iter()
        .enumerate()
        .map(|(k, c)| match c {
            Cell::Wild => "*".to_string(),
            Cell::Sym(i) => glyphs
                .get(k)
                .and_then(|g| g.get(usize::from(*i)))
                .cloned()
                .unwrap_or_else(|| i.to_string()),
        })
        .collect();
    format!("[{}]", cells.join(", "))
}

fn check_state_rows(
    state: &ExpandedState,
    glyphs: &[Vec<String>],
    warn: &mut Vec<Diagnostic>,
) -> Result<(), CompileError> {
    for j in 0..state.rules.len() {
        let pj = &state.rules[j].pattern;
        for i in 0..j {
            if state.rules[i].pattern == *pj {
                if is_all_wild(pj) || pj.iter().any(|c| matches!(c, Cell::Wild)) {
                    // A partial/full wildcard duplicate is a shadow (the later
                    // row is unreachable); warn, don't fail.
                    warn.push(Diagnostic {
                        code: "shadowed-rule",
                        span: state.rules[j].span,
                        message: format!(
                            "this rule is unreachable — an earlier rule has the same pattern {}",
                            render_pattern(pj, glyphs)
                        ),
                        fix: None,
                    });
                } else {
                    // Two exact (wildcard-free) rows matching the same tuple.
                    return Err(CompileError {
                        span: state.rules[j].span,
                        kind: CompileErrorKind::ExactRowConflict {
                            first: render_pattern(&state.rules[i].pattern, glyphs),
                            second: render_pattern(pj, glyphs),
                        },
                    });
                }
                break; // first earlier match is enough
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// The stage entry point.
// ---------------------------------------------------------------------------

/// Expand a resolved module: graft splicing then range expansion, producing
/// worlds whose states carry only concrete, index-resolved rules (Task 6 input).
pub(crate) fn expand(resolved: &Resolved) -> Result<Expanded, CompileError> {
    let graphs: HashMap<&str, &ResolvedWorld> = resolved
        .worlds
        .iter()
        .filter(|w| w.kind == WorldKind::Graph)
        .map(|w| (w.name.as_str(), w))
        .collect();
    check_graph_acyclicity(&graphs)?;

    let mut memo: HashMap<String, GraphExpansion> = HashMap::new();
    let mut diagnostics: Vec<Diagnostic> = Vec::new();
    let mut worlds: Vec<ExpandedWorld> = Vec::new();
    let mut entry_world = None;

    for (idx, world) in resolved.worlds.iter().enumerate() {
        if world.kind == WorldKind::Graph {
            continue; // graphs are consumed by grafting, never emitted
        }
        if Some(idx) == resolved.entry_world {
            entry_world = Some(worlds.len());
        }
        let tapes = tape_infos(world, &resolved.alphabets);
        let mut states = Vec::new();
        expand_own_states(world, &tapes, &mut diagnostics, &mut states)?;
        let mut namegen = NameGen::new(reserved_names(world));
        let mut alias = HashMap::new();
        let graft_entry = expand_grafts_into(
            world,
            resolved,
            &graphs,
            &mut memo,
            &mut diagnostics,
            &mut namegen,
            &mut states,
            &mut alias,
        )?;
        resolve_aliases(&mut states, &alias);
        let entry = Some(world_entry(world, graft_entry, &alias));

        let per_tape_glyphs: Vec<Vec<String>> = world
            .tapes
            .iter()
            .map(|t| resolved.alphabets[&t.alphabet].glyphs.clone())
            .collect();
        for st in &states {
            check_state_rows(st, &per_tape_glyphs, &mut diagnostics)?;
        }

        worlds.push(ExpandedWorld {
            kind: world.kind,
            name: world.name.clone(),
            tapes: world
                .tapes
                .iter()
                .map(|t| ExpandedTape {
                    name: t.name.clone(),
                    alphabet: t.alphabet.clone(),
                    cardinality: t.cardinality,
                })
                .collect(),
            state_params: world.state_params.clone(),
            entry,
            states,
        });
    }

    Ok(Expanded {
        alphabets: resolved.alphabets.clone(),
        worlds,
        entry_world,
        diagnostics,
    })
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
    fn unequal_tape_closes_unlisted_reads_to_holes() {
        // host wider (5) than graph (3), remap two symbols (3→1, 4→2). Across
        // the unequal alphabets there is no identity completion, so every
        // OTHER non-blank host symbol (1, 2) is a read hole — even though its
        // index is within the graph alphabet. This is what build_tapemap
        // mints; construct the closed maps the same way and assert the methods.
        let mut rmap = m(&[(3, 1), (4, 2)]);
        close_unlisted(&mut rmap, 5); // host_card
        rmap.pairs.retain(|s, d| s != d);
        let t = TapeMap {
            phys: 0,
            host_card: 5,
            graph_card: 3,
            rmap,
            wmap: m(&[(1, 3), (2, 4)]),
        };
        assert_eq!(t.read_image(0), Some(0)); // blank pinned
        assert_eq!(t.read_image(1), None); // in-range unlisted ⇒ hole
        assert_eq!(t.read_image(3), Some(1)); // explicit remap
        assert_eq!(t.holes(), vec![1, 2]);
        // graph 1's only preimage is the explicit remap (3); the identity 1 is
        // now a hole, not a preimage.
        assert_eq!(t.preimage(1), vec![3]);
        assert_eq!(t.preimage(2), vec![4]);
        assert_eq!(t.write_image(1), Some(3));
    }

    #[test]
    fn empty_unequal_tape_holes_every_nonblank_read() {
        // host (4) → graph (3), empty map: no silent identity across unequal
        // alphabets, so every non-blank host symbol (1, 2, 3) is a read hole.
        let mut rmap = SymMap::identity();
        close_unlisted(&mut rmap, 4); // host_card
        let t = TapeMap {
            phys: 0,
            host_card: 4,
            graph_card: 3,
            rmap,
            wmap: SymMap::identity(),
        };
        assert_eq!(t.read_image(0), Some(0));
        assert_eq!(t.read_image(1), None);
        assert_eq!(t.holes(), vec![1, 2, 3]);
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
        let rows = expand_rule(&rules[0], &tapes, &mut warn, &mut own_tr).unwrap();
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
        let rows = expand_rule(&rules[0], &tapes, &mut warn, &mut own_tr).unwrap();
        assert_eq!(rows.len(), 125);
        // value == index for this alphabet: v reads index v, writes v+1.
        assert_eq!(rows[0].pattern, vec![Cell::Sym(1)]);
        assert_eq!(rows[0].write, vec![WriteOut::Sym(2)]);
        assert_eq!(rows[124].pattern, vec![Cell::Sym(125)]);
        assert_eq!(rows[124].write, vec![WriteOut::Sym(126)]);
        // The `[126] -> halt` and `[0] -> write [1]` rows are singletons.
        let halt = expand_rule(&rules[1], &tapes, &mut warn, &mut own_tr).unwrap();
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
        let err = expand_rule(&rules[0], &tapes, &mut warn, &mut own_tr).unwrap_err();
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
        let err = expand_rule(&rules[0], &tapes, &mut warn, &mut own_tr).unwrap_err();
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

    /// A per-tape map matching what build_tapemap mints: explicit remaps,
    /// explicit holes, and — on UNEQUAL alphabets — the closed-on-unequal
    /// completion (every non-blank source the map does not name becomes a
    /// hole). `dst`/`whole` range beyond the target alphabet to also mint
    /// out-of-range holes. Blank (0) stays pinned.
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
        // Closed-on-unequal, exactly as build_tapemap: unlisted non-blank
        // source symbols become holes across differently-sized alphabets.
        if host_card != graph_card {
            close_unlisted(&mut rmap, host_card);
            close_unlisted(&mut wmap, graph_card);
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

#[cfg(test)]
mod expand_tests {
    //! End-to-end: `analyze` (T4) then [`expand`], over the spec's graft
    //! example A.6, instance dedup, a holey graft (trap synthesis), and the
    //! graph-definition acyclicity guard.
    use super::*;
    use crate::compiler::analyze;

    fn expand_ok(src: &str) -> Expanded {
        let a = analyze(src).unwrap_or_else(|e| panic!("analyze failed: {e}"));
        expand(&a.resolved).unwrap_or_else(|e| panic!("expand failed: {e}"))
    }

    fn machine(ex: &Expanded) -> &ExpandedWorld {
        ex.worlds
            .iter()
            .find(|w| w.kind == WorldKind::Machine)
            .expect("a machine world")
    }

    fn state<'a>(w: &'a ExpandedWorld, name: &str) -> &'a ExpandedState {
        w.states.iter().find(|s| s.name == name).unwrap_or_else(|| {
            panic!(
                "state {name} not found in {:?}",
                w.states.iter().map(|s| &s.name).collect::<Vec<_>>()
            )
        })
    }

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
}
";

    #[test]
    fn a6_graft_splices_the_entry_and_substitutes_continuations() {
        let ex = expand_ok(A6);
        // Graphs are consumed — only the machine remains.
        assert!(ex.worlds.iter().all(|w| w.kind != WorldKind::Graph));
        let m = machine(&ex);
        assert_eq!(m.entry.as_deref(), Some("seek"));
        assert!(ex.diagnostics.is_empty(), "{:?}", ex.diagnostics);

        // `seek` is findX's `walk` spliced: marks is identity (equal glyphs),
        // so no trap rows; 'x' → celebrate, '_' → giveUp, else move right.
        let seek = state(m, "seek");
        assert_eq!(seek.rules.len(), 3);
        assert_eq!(seek.rules[0].pattern, vec![Cell::Sym(1)]); // 'x' at index 1
        assert_eq!(
            seek.rules[0].transition,
            Transition2::Goto("celebrate".into())
        );
        assert_eq!(seek.rules[1].pattern, vec![Cell::Sym(0)]); // '_' at index 0
        assert_eq!(seek.rules[1].transition, Transition2::Goto("giveUp".into()));
        assert_eq!(seek.rules[2].pattern, vec![Cell::Wild]);
        assert_eq!(seek.rules[2].moves, vec![MoveDir::Right]);
        assert_eq!(seek.rules[2].transition, Transition2::Goto("seek".into()));

        // The own states survived unchanged.
        assert_eq!(state(m, "celebrate").rules[0].write, vec![WriteOut::Sym(0)]);
        assert_eq!(state(m, "giveUp").rules[0].transition, Transition2::Halt);
    }

    #[test]
    fn two_identical_grafts_dedup_to_one_fragment() {
        // A graph grafted twice with identical bindings + continuations: one
        // set of spliced states; the second instance name aliases the first.
        let src = "\
alphabet marks { '_', 'x' }
graph g(tape t: marks, state done) {
  entry state walk { ['x'] -> done; [*] -> move [>] goto walk; }
}
machine {
  tape work: marks;
  entry graft g(t = work, done = fin) as a;
  graft g(t = work, done = fin) as b;
  state fin { [*] -> stop; }
  state kick { [*] -> goto b; }
}
";
        let ex = expand_ok(src);
        let m = machine(&ex);
        // Only ONE spliced walk fragment (named `a`); `b` did not add states.
        assert!(m.states.iter().any(|s| s.name == "a"));
        assert!(!m.states.iter().any(|s| s.name == "b"));
        // `goto b` was rewritten to the surviving instance `a`.
        let kick = state(m, "kick");
        assert_eq!(kick.rules[0].transition, Transition2::Goto("a".into()));
        assert_eq!(m.entry.as_deref(), Some("a"));
    }

    #[test]
    fn a_holey_graft_synthesizes_trap_read_rows() {
        // Host tape `quad` (4 symbols) grafts a graph over `bits` (3 symbols).
        // Across the unequal alphabets there is no identity completion, so the
        // in-range data symbols the graph uses must be listed explicitly
        // (`'0'->'0', '1'->'1'`); the extra host symbol `q` (index 3) is
        // unlisted — a read hole ⇒ a trap-read row prepended to the reading
        // state. (An empty map here would hole `'0'` and `'1'` too — see
        // `a_holey_graft_holes_an_unlisted_in_range_symbol` for that rule.)
        let src = "\
alphabet bits { '_', '0', '1' }
alphabet quad { '_', '0', '1', 'q' }
graph flip(tape t: bits, state done) {
  entry state go { ['0'] -> write ['1'] done; ['1'] -> write ['0'] done; [*] -> done; }
}
machine {
  tape data: quad;
  entry graft flip(t = data with map { '0'->'0', '1'->'1' }, done = fin) as f;
  state fin { [*] -> stop; }
}
";
        let ex = expand_ok(src);
        let m = machine(&ex);
        let f = state(m, "f");
        // One read hole (`q` = index 3) prepends one trap-read row, first.
        let traps: Vec<&ExpandedRule> = f
            .rules
            .iter()
            .filter(|r| r.transition == Transition2::TrapRead)
            .collect();
        assert_eq!(traps.len(), 1, "{:?}", f.rules);
        assert_eq!(f.rules[0].transition, Transition2::TrapRead);
        assert_eq!(f.rules[0].pattern, vec![Cell::Sym(3)]); // 'q'
        // The real rows read/write the explicitly-mapped bits symbols.
        let real: Vec<&ExpandedRule> = f
            .rules
            .iter()
            .filter(|r| matches!(r.transition, Transition2::Goto(_)))
            .collect();
        // '0'(idx1) → write '1'(idx2); '1'(idx2) → write '0'(idx1).
        assert!(
            real.iter()
                .any(|r| r.pattern == vec![Cell::Sym(1)] && r.write == vec![WriteOut::Sym(2)])
        );
        assert!(
            real.iter()
                .any(|r| r.pattern == vec![Cell::Sym(2)] && r.write == vec![WriteOut::Sym(1)])
        );
    }

    #[test]
    fn a_holey_graft_holes_an_unlisted_in_range_symbol() {
        // The closed-on-unequal conformance case at the graft level: host `h3`
        // (3 symbols) grafts a graph over `g4` (4 symbols) mapping only
        // `'a'->'a'`. Host `'b'` (index 2) is UNLISTED — and its index is
        // within the graph's 4-symbol alphabet, so pre-fix it read through by
        // identity. Under the closed-on-unequal rule it is a read hole and
        // synthesizes a trap-read row. The reading graph state is what makes
        // the hole observable (a straight-line state does no `rd`).
        let src = "\
alphabet h3 { '_', 'a', 'b' }
alphabet g4 { '_', 'a', 'b', 'c' }
graph g(tape t: g4, state done) {
  entry state s { ['a'] -> done; [*] -> done; }
}
machine {
  tape w: h3;
  entry graft g(t = w with map { 'a'->'a' }, done = fin) as x;
  state fin { [*] -> stop; }
}
";
        let ex = expand_ok(src);
        let m = machine(&ex);
        let x = state(m, "x");
        // Host 'b' (index 2, in range of the 4-symbol graph) is a read hole.
        let traps: Vec<&ExpandedRule> = x
            .rules
            .iter()
            .filter(|r| r.transition == Transition2::TrapRead)
            .collect();
        assert_eq!(
            traps.len(),
            1,
            "one in-range read hole ('b'): {:?}",
            x.rules
        );
        assert_eq!(x.rules[0].transition, Transition2::TrapRead);
        assert_eq!(x.rules[0].pattern, vec![Cell::Sym(2)]); // 'b'
    }

    #[test]
    fn a_write_hole_becomes_a_trap_write_row() {
        // Graph over `bits` writes '1'; host tape has only '_' (1 symbol), so
        // the graph symbol '1' has no host write image ⇒ trap-write.
        // (The graph tape is wider than the host — a write hole.)
        let src = "\
alphabet bits { '_', '0', '1' }
alphabet one  { '_' }
graph w(tape t: bits, state done) {
  entry state s { [*] -> write ['1'] done; }
}
machine {
  tape z: one;
  entry graft w(t = z with map { }, done = fin) as g;
  state fin { [*] -> stop; }
}
";
        let ex = expand_ok(src);
        let m = machine(&ex);
        let g = state(m, "g");
        // The single all-wildcard rule writes graph '1' — no host image.
        assert!(
            g.rules
                .iter()
                .any(|r| r.transition == Transition2::TrapWrite)
        );
    }

    #[test]
    fn a_self_grafting_graph_is_a_cycle_error() {
        let src = "\
alphabet marks { '_', 'x' }
graph loop(tape t: marks) {
  entry graft loop(t = t) as inner;
}
machine { tape w: marks; entry state s { [*] -> stop; } }
";
        let a = analyze(src).expect("analyze");
        let err = expand(&a.resolved).unwrap_err();
        assert_eq!(err.kind.code(), "graft-cycle");
    }

    #[test]
    fn grafting_a_call_bearing_graph_is_a_clear_error() {
        // A graph whose body calls a routine is legal source (T4 permits it),
        // but splicing it into a host needs binding composition (not
        // implemented). Grafting it is a spanned error at the graft site.
        let src = "\
alphabet marks { '_', 'x' }
routine helper(tape t: marks) { entry state h { [*] -> return; } }
graph g(tape t: marks, state done) {
  entry state s { [*] -> call helper(t = t) then done; }
}
machine {
  tape w: marks;
  entry graft g(t = w, done = fin) as x;
  state fin { [*] -> stop; }
}
";
        let a = analyze(src).expect("analyze");
        let graft_span = a
            .resolved
            .worlds
            .iter()
            .find(|w| w.kind == WorldKind::Machine)
            .expect("machine")
            .grafts[0]
            .target_span;
        let err = expand(&a.resolved).unwrap_err();
        assert_eq!(err.kind.code(), "graft-call-unsupported");
        // The span names the graft instantiation, not the call site inside g.
        assert_eq!(err.span, graft_span);
    }

    #[test]
    fn a_call_bearing_graph_left_ungrafted_compiles_clean() {
        // The SAME graph, defined but never grafted: unreachable source that
        // is never expanded, so the call-in-graph guard never fires.
        let src = "\
alphabet marks { '_', 'x' }
routine helper(tape t: marks) { entry state h { [*] -> return; } }
graph g(tape t: marks, state done) {
  entry state s { [*] -> call helper(t = t) then done; }
}
machine {
  tape w: marks;
  entry state go { [*] -> stop; }
}
";
        let a = analyze(src).expect("analyze");
        expand(&a.resolved).expect("ungrafted call-bearing graph expands clean");
    }

    #[test]
    fn an_exact_row_conflict_after_expansion_is_a_static_error() {
        // Overlapping numeric ranges expand to a shared concrete row `[2]`.
        let src = "\
alphabet bytes { 0..4 }
machine {
  tape c: bytes;
  entry state s {
    [1..2] -> write [0] stop;
    [2..3] -> write [1] stop;
  }
}
";
        let a = analyze(src).expect("analyze");
        let err = expand(&a.resolved).unwrap_err();
        assert_eq!(err.kind.code(), "exact-row-conflict");
    }

    /// The graft map-legality contract (spanned), mirroring the linker.
    fn graft_err(map: &str, host_alpha: &str, graph_alpha: &str) -> &'static str {
        let src = format!(
            "\
alphabet ga {{ {graph_alpha} }}
alphabet ha {{ {host_alpha} }}
graph g(tape t: ga, state done) {{ entry state s {{ [*] -> done; }} }}
machine {{ tape w: ha; entry graft g(t = w{map}, done = fin) as x; state fin {{ [*] -> stop; }} }}
"
        );
        let a = analyze(&src).expect("analyze");
        expand(&a.resolved).unwrap_err().kind.code()
    }

    #[test]
    fn graft_map_legality_errors() {
        // Omitted map on unequal glyph sets — identity needs equal alphabets.
        assert_eq!(
            graft_err("", "'_', '0'", "'_', '0', '1'"),
            "identity-glyph-mismatch"
        );
        // Blank pinned: `'_'->'0'` moves blank off itself.
        assert_eq!(
            graft_err(" with map { '_'->'0' }", "'_', '0', '1'", "'_', '0', '1'"),
            "map-blank-pin"
        );
        // A map symbol not in the tape's alphabet (`z` absent on the host).
        assert_eq!(
            graft_err(" with map { 'z'->'0' }", "'_', '0', '1'", "'_', '0', '1'"),
            "map-symbol-not-in-alphabet"
        );
        // A read conflict: `'0'` sent to two different images.
        assert_eq!(
            graft_err(
                " with map { '0'->'0', '0'->'1' }",
                "'_', '0', '1'",
                "'_', '0', '1'"
            ),
            "map-conflict"
        );
        // Equal-size non-injective: bidir `'0'->'1'` collides with identity 1.
        assert_eq!(
            graft_err(" with map { '0'->'1' }", "'_', '0', '1'", "'_', '0', '1'"),
            "map-not-injective"
        );
    }

    #[test]
    fn a_duplicate_wildcard_rule_warns_shadowed() {
        let src = "\
alphabet b { '_', '0' }
machine {
  tape t: b;
  entry state s { [*] -> stop; [*] -> halt; }
}
";
        let ex = expand_ok(src);
        assert!(
            ex.diagnostics.iter().any(|d| d.code == "shadowed-rule"),
            "{:?}",
            ex.diagnostics
        );
    }
}
