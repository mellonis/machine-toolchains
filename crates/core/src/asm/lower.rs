//! Lossless assembly CST → per-function source items (docs/formats.md
//! (assembly text) grammar). Shaping is total and lives in `cst.rs`;
//! this pass validates + classifies, attaching a precise [`Span`] to
//! every diagnostic. Replaces the old line-oriented parser.

use super::cst::{
    AsmCst, AsmItem, AsmItemKind, FrameDirectiveCst, FrameHeaderCst, FrameMapCst, FramePairCst,
    FuncCst, InstrCst, LabelCst, LineCst, OperandToken, ReptCst, RoutineDirectiveCst, SectionCst,
    TableDirectiveCst, TableDirectiveKind, parse_asm_cst_with, parse_binding,
};
use super::subst::substitute;
use super::syntax::{ArchSyntax, Flow, SyntaxEntry};
use super::{AsmError, AsmErrorKind};
use crate::diagnostics::Span;
use crate::formats::object::RoutineSig;
use crate::vm::OperandKind;

/// A name paired with the source span it occupies.
///
/// `pub`, not `pub(crate)`: the lint layer's [`super::lint::AsmLintContext`]
/// carries `&[SourceFunction]` on a `pub` field, and a public field's type
/// must be at least as visible as the field itself (`private_interfaces`)
/// — even though the defining `lower` module itself stays private to
/// `asm` and its descendants, which is where every actual constructor
/// and consumer of these types lives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpannedName {
    pub name: String,
    pub span: Span,
}

#[derive(Debug)]
pub struct SourceFunction {
    pub name: String,
    /// The `.func` name's own span — what the all-or-none signature
    /// diagnostic points at when this function is left unsigned in a
    /// file that signs any.
    pub name_span: Span,
    pub local: bool,
    pub items: Vec<SourceItem>,
}

#[derive(Debug)]
pub enum SourceItem {
    Instr {
        span: Span,
        labels: Vec<SpannedName>,
        opcode: u8,
        operand: SourceOperand,
    },
    RawByte {
        span: Span,
        labels: Vec<SpannedName>,
        value: u8,
    },
}

#[derive(Debug)]
pub enum SourceOperand {
    None,
    Ints(Vec<i64>),
    Name(SpannedName),
    /// `@name` — a function-symbol reference, not a local label.
    SymbolName(SpannedName),
    /// A `[..]` vector operand, parsed per element, carrying the bracket
    /// region's span for emit-time diagnostics. Which elements are legal
    /// depends on the consuming context (match rows: payload and
    /// wildcard; write vectors: payload and keep; move vectors: the
    /// three moves) — that legality is enforced per OperandKind at the
    /// assembler's emit arms; this layer only parses.
    Vector(Vec<VecElem>, Span),
    /// An `#<int>` immediate (Imm8), already range-checked to 0..=255.
    Imm(u8),
    /// A framed call operand: the call `target` (a symbol name, like a
    /// plain call's) and the `frame` table label (like a TableRef).
    FramedCall {
        target: SpannedName,
        frame: SpannedName,
    },
    /// A declarative binding call operand (`call name [binding]`): the
    /// call `target` (a symbol name, like a plain call's) and the tape
    /// binding — one entry per callee virtual tape, in list order. The
    /// assembler emits a plain far-call opcode with a zeroed hole (no
    /// relocation) and records the binding as an MO bound-call for the
    /// composition engine to lower (docs/formats.md (bound calls)).
    BoundCallOp {
        target: SpannedName,
        binding: Vec<SourceTapeBinding>,
    },
    /// A `[w...], [m...]` two-vector operand ([`OperandKind::WriteMoveVec`]):
    /// the write elements then the move elements, carrying the operand
    /// region's span for emit-time diagnostics. Element legality per group
    /// (write vocabulary / move vocabulary) is enforced at the assembler's
    /// emit arm, like the single-vector kinds; this layer only parses the
    /// two groups.
    WriteMoveVectors(Vec<VecElem>, Vec<VecElem>, Span),
}

/// One virtual-tape binding at a declarative call site: which caller
/// physical tape feeds this callee tape (`caller_tape`), and the symbol
/// map between their alphabets. `one_way` (the `=>` spelling) marks a
/// read-only pair, excluded from write-back. Mapping legality (blank
/// rules, bijection, completion) is the composition engine's, checked at
/// link time (docs/formats.md (bound calls)); this layer records the
/// authored pairs verbatim after structural validation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceTapeBinding {
    pub caller_tape: u8,
    /// `(src, dst, one_way)` per authored pair, in source order.
    pub pairs: Vec<(u32, u32, bool)>,
}

/// One element of a `[..]` vector operand.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VecElem {
    Payload(u32),
    /// `*` — any symbol (match rows). Encodes as `0x7F`.
    Wildcard,
    /// `-` — keep, no write on that tape. Encodes as `0x7F`.
    Keep,
    /// `<` — encodes as 1.
    MoveLeft,
    /// `>` — encodes as 2.
    MoveRight,
    /// `.` — encodes as 0.
    Stay,
}

/// A match-table row: parsed elements plus the directive's span (the
/// span every discipline diagnostic for this row points at).
#[derive(Debug)]
pub struct SourceRow {
    pub elems: Vec<VecElem>,
    pub span: Span,
}

/// One file-scoped table lowered from a labeled run of directives in
/// `.section tables`. Byte emission and discipline validation live in
/// the assembler; this is the parsed, spanned source form.
#[derive(Debug)]
pub enum SourceTable {
    /// A labeled run of `.row [..]` directives.
    Match {
        name: SpannedName,
        rows: Vec<SourceRow>,
    },
    /// A labeled run of `.targets`/`.target` directives; entries are
    /// CODE labels, resolved after function layout.
    Dispatch {
        name: SpannedName,
        targets: Vec<SpannedName>,
    },
    /// A `.frame` group: the projection (`tapes`), per-virtual-tape symbol
    /// maps (materialized dense — index 0 forced to identity for
    /// blank↔blank), and the multi-exit return labels (CODE labels,
    /// resolved after function layout, like dispatch entries). Referenced
    /// by a `call.m` frame operand, not by `mtc`/`djmp`.
    Frame {
        name: SpannedName,
        /// Physical tape per virtual tape; arity = `tapes.len()`.
        tapes: Vec<u8>,
        maps: Vec<FrameTapeMap>,
        exits: Vec<SpannedName>,
    },
}

/// One virtual tape's dense symbol maps in a frame descriptor. `rmap`
/// (PHYSICAL->VIRTUAL, read) and `wmap` (VIRTUAL->PHYSICAL, write) are
/// materialized to `max index + 1` entries, index 0 forced to identity,
/// `0xFFFF` = hole; empty = the identity map (`*_len == 0`).
#[derive(Debug)]
pub struct FrameTapeMap {
    pub k: u8,
    pub rmap: Vec<u16>,
    pub wmap: Vec<u16>,
}

impl SourceTable {
    pub fn name(&self) -> &SpannedName {
        match self {
            SourceTable::Match { name, .. }
            | SourceTable::Dispatch { name, .. }
            | SourceTable::Frame { name, .. } => name,
        }
    }
}

/// Everything one lowered source file carries: the functions (code
/// section), the tables (`.section tables`), and the `.routine`
/// signatures. Cap-off dialects never produce tables or signatures —
/// the CST never shapes the directives.
#[derive(Debug)]
pub struct LoweredSource {
    pub functions: Vec<SourceFunction>,
    pub tables: Vec<SourceTable>,
    /// Per-function signatures, parallel to `functions` when present.
    /// `Some` iff the file declares any `.routine` — and then every
    /// function carries one (all or none: the MO signature section is
    /// parallel to the blobs, docs/formats.md (MO)).
    pub signatures: Option<Vec<RoutineSig>>,
}

/// Which source section the lowering cursor is in. The default is code,
/// so dialects without the tables cap never notice sections exist.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Section {
    Code,
    Tables,
}

fn err(span: Span, kind: AsmErrorKind) -> AsmError {
    AsmError { span, kind }
}

/// Label grammar: a letter or `_`, then letters, digits, `_`. Letters
/// follow the Unicode reading (`char::is_alphabetic`), consistent with
/// function names; the tightening over symbol names is dots and `::`
/// only (docs/formats.md (assembly text)).
fn is_label_name(s: &str) -> bool {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if c.is_alphabetic() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_alphanumeric() || c == '_')
}

/// Symbol names: `::`-separated namespace segments, then a dotted
/// function path (`std::api.helper`). Labels do NOT use this rule.
fn is_symbol_name(s: &str) -> bool {
    !s.is_empty()
        && s.split("::").all(|segment| {
            let mut chars = segment.chars();
            match chars.next() {
                Some(c) if c.is_alphabetic() || c == '_' => {}
                _ => return false,
            }
            chars.all(|c| c.is_alphanumeric() || c == '_' || c == '.')
        })
}

fn spanned(label: &LabelCst) -> SpannedName {
    SpannedName {
        name: label.name.clone(),
        span: label.span,
    }
}

/// The functions-only view of [`lower_source`], for consumers that never
/// see tables (the lint layer; every existing cap-off dialect). All
/// validation still runs — only the successfully lowered tables are
/// dropped from the result.
pub(crate) fn lower(
    cst: &AsmCst,
    syntax: &ArchSyntax,
    source: &str,
) -> Result<Vec<SourceFunction>, AsmError> {
    lower_source(cst, syntax, source).map(|lowered| lowered.functions)
}

/// The lowering state threaded through every item: the accumulating
/// output plus the two cursors — pending labels awaiting their
/// instruction, and the section/table-run position.
struct LowerCtx {
    functions: Vec<SourceFunction>,
    pending: Vec<SpannedName>,
    section: Section,
    tables: Vec<SourceTable>,
    /// `tables.last()` still accepts unlabeled continuation directives.
    /// Closed by a `.section` switch; table directives are the only
    /// items legal in the tables section, so nothing else can intervene
    /// (comments are trivia and do not close a run).
    run_open: bool,
    /// `.routine` declarations not yet matched by their `.func`, in
    /// source order. A directive attaches when its function is defined
    /// (the must-precede rule); one still pending at end of input
    /// precedes no `.func` of its name — an error.
    pending_sigs: Vec<(SpannedName, RoutineSig)>,
    /// Per-function signature slots, parallel to `functions`.
    func_sigs: Vec<Option<RoutineSig>>,
}

pub(crate) fn lower_source(
    cst: &AsmCst,
    syntax: &ArchSyntax,
    source: &str,
) -> Result<LoweredSource, AsmError> {
    let mut ctx = LowerCtx {
        functions: Vec::new(),
        pending: Vec::new(),
        section: Section::Code,
        tables: Vec::new(),
        run_open: false,
        pending_sigs: Vec::new(),
        func_sigs: Vec::new(),
    };

    for item in &cst.items {
        lower_item(item, syntax, source, &mut ctx)?;
    }

    // A label with no instruction after it, at end of input.
    if let Some(first) = ctx.pending.first() {
        return Err(err(
            first.span,
            AsmErrorKind::Syntax("label at end of function"),
        ));
    }
    // A `.routine` still pending precedes no `.func` of its name —
    // either the function does not exist or it was defined BEFORE the
    // directive (the must-precede rule).
    if let Some((name, _)) = ctx.pending_sigs.first() {
        return Err(err(
            name.span,
            AsmErrorKind::BadSignature(format!(
                "`.routine` precedes no `.func` named `{}`",
                name.name
            )),
        ));
    }
    // All or none: the MO signature section parallels the blobs
    // (docs/formats.md (MO)), so a file that signs any function must
    // sign every function.
    let signatures = if ctx.func_sigs.iter().any(Option::is_some) {
        let mut sigs = Vec::with_capacity(ctx.func_sigs.len());
        for (function, sig) in ctx.functions.iter().zip(ctx.func_sigs) {
            match sig {
                Some(sig) => sigs.push(sig),
                None => {
                    return Err(err(
                        function.name_span,
                        AsmErrorKind::BadSignature(format!(
                            "function `{}` lacks a `.routine` signature",
                            function.name
                        )),
                    ));
                }
            }
        }
        Some(sigs)
    } else {
        None
    };
    Ok(LoweredSource {
        functions: ctx.functions,
        tables: ctx.tables,
        signatures,
    })
}

/// Lowers one CST item. Shared by the top-level pass and — via
/// [`lower_rept`] — by each expanded `.rept` body item, so the two go
/// through exactly the same classification and error paths. `source` is
/// threaded only so `.rept` can recover its body lines verbatim for
/// substitution; every other arm ignores it.
fn lower_item(
    item: &AsmItem,
    syntax: &ArchSyntax,
    source: &str,
    ctx: &mut LowerCtx,
) -> Result<(), AsmError> {
    match &item.kind {
        AsmItemKind::Comment(_) => {}
        AsmItemKind::Raw(raw) => return Err(err(raw.span, AsmErrorKind::RawLine)),
        AsmItemKind::Func(func) => lower_func(func, ctx)?,
        AsmItemKind::Line(line) => lower_line(line, syntax, ctx)?,
        // Sections, table directives, and `.routine` shape only under
        // the opt-in caps; cap-off dialects (PM-1) never reach these arms.
        AsmItemKind::Section(s) => lower_section(s, ctx)?,
        AsmItemKind::TableDirective(d) => lower_table_directive(d, ctx)?,
        AsmItemKind::Rept(r) => lower_rept(r, syntax, source, ctx)?,
        AsmItemKind::RoutineDirective(d) => lower_routine_directive(d, ctx)?,
        AsmItemKind::FrameDirective(d) => lower_frame_directive(d, ctx)?,
    }
    Ok(())
}

/// `.frame`/`.map`/`.exits`: builds a frame descriptor's source form
/// (docs/formats.md (frame descriptors)). Legal only inside `.section
/// tables`. `.frame <name>` (labeled) opens a group; `.map`/`.exits`
/// (unlabeled) continue the open group. Descriptor bytes are laid out in
/// the assembler once the owner is known; this pass validates structure
/// and materializes the dense symbol maps.
fn lower_frame_directive(d: &FrameDirectiveCst, ctx: &mut LowerCtx) -> Result<(), AsmError> {
    if let Some(first) = ctx.pending.first() {
        return Err(err(
            first.span,
            AsmErrorKind::Syntax("label at end of function"),
        ));
    }
    if ctx.section != Section::Tables {
        return Err(err(
            d.span(),
            AsmErrorKind::BadTable("frame directives live in the tables section"),
        ));
    }
    match d {
        FrameDirectiveCst::Header(h) => lower_frame_header(h, ctx),
        FrameDirectiveCst::Map(m) => lower_frame_map(m, ctx),
        FrameDirectiveCst::Exits(e) => lower_frame_exits(e, ctx),
    }
}

/// `Fname: .frame tapes=(<int>, …)` — opens a descriptor group.
fn lower_frame_header(h: &FrameHeaderCst, ctx: &mut LowerCtx) -> Result<(), AsmError> {
    if !is_label_name(&h.label.name) {
        return Err(err(
            h.label.span,
            AsmErrorKind::Syntax("label names use letters, digits, underscore"),
        ));
    }
    if h.tapes.is_empty() || h.tapes.len() > 16 {
        return Err(err(
            h.tapes_span,
            AsmErrorKind::BadFrame("frame `tapes` list must have 1..=16 entries".to_string()),
        ));
    }
    let mut tapes = Vec::with_capacity(h.tapes.len());
    for &phys in &h.tapes {
        let phys = u8::try_from(phys).map_err(|_| {
            err(
                h.tapes_span,
                AsmErrorKind::BadFrame("physical tape index exceeds 255".to_string()),
            )
        })?;
        tapes.push(phys);
    }
    if ctx.tables.iter().any(|t| t.name().name == h.label.name) {
        return Err(err(
            h.label.span,
            AsmErrorKind::DuplicateLabel(h.label.name.clone()),
        ));
    }
    ctx.tables.push(SourceTable::Frame {
        name: spanned(&h.label),
        tapes,
        maps: Vec::new(),
        exits: Vec::new(),
    });
    ctx.run_open = true;
    Ok(())
}

/// `.map <k>[, rmap=(…)][, wmap=(…)]` — continues the open frame group.
fn lower_frame_map(m: &FrameMapCst, ctx: &mut LowerCtx) -> Result<(), AsmError> {
    let Some((tapes_len, maps)) = open_frame_mut(ctx) else {
        return Err(err(
            m.span,
            AsmErrorKind::BadFrame("`.map` has no preceding `.frame`".to_string()),
        ));
    };
    if usize::try_from(m.k).is_err() || m.k as usize >= tapes_len {
        return Err(err(
            m.k_span,
            AsmErrorKind::BadFrame(format!(
                "`.map` tape {} is >= the frame arity {tapes_len}",
                m.k
            )),
        ));
    }
    let k = m.k as u8;
    if maps.iter().any(|fm| fm.k == k) {
        return Err(err(
            m.k_span,
            AsmErrorKind::BadFrame(format!("duplicate `.map {k}`")),
        ));
    }
    let rmap = match &m.rmap {
        Some(pairs) => build_dense_map(pairs, m.rmap_span.unwrap_or(m.span))?,
        None => Vec::new(),
    };
    let wmap = match &m.wmap {
        Some(pairs) => {
            // The one-way (`=>`) spelling is read-direction only — such a
            // pair is read-only and excluded from write-back
            // (docs/formats.md (bound calls)) — so it is legal in `rmap`
            // but not in `wmap`, the write direction. The shared pair
            // parser accepts `=>` in either clause and keeps the CST
            // lossless; the wmap-scoped rejection lives here.
            if pairs.iter().any(|p| p.one_way) {
                return Err(err(
                    m.wmap_span.unwrap_or(m.span),
                    AsmErrorKind::BadFrame(
                        "one-way pairs (`=>`) are read-direction only; wmap pairs use `->`"
                            .to_string(),
                    ),
                ));
            }
            build_dense_map(pairs, m.wmap_span.unwrap_or(m.span))?
        }
        None => Vec::new(),
    };
    maps.push(FrameTapeMap { k, rmap, wmap });
    Ok(())
}

/// `.exits <label>, …` — sets the open frame's return targets (once).
fn lower_frame_exits(e: &super::cst::FrameExitsCst, ctx: &mut LowerCtx) -> Result<(), AsmError> {
    // Validate the labels first (independent of the open-frame check), so a
    // bad label name reports precisely.
    let mut targets = Vec::with_capacity(e.targets.len());
    for operand in &e.targets {
        if !is_label_name(&operand.text) {
            return Err(err(
                operand.span,
                AsmErrorKind::BadFrame("exit targets are label names".to_string()),
            ));
        }
        targets.push(SpannedName {
            name: operand.text.clone(),
            span: operand.span,
        });
    }
    let Some(SourceTable::Frame { exits, .. }) = open_frame_table_mut(ctx) else {
        return Err(err(
            e.span,
            AsmErrorKind::BadFrame("`.exits` has no preceding `.frame`".to_string()),
        ));
    };
    if !exits.is_empty() {
        return Err(err(
            e.span,
            AsmErrorKind::BadFrame("`.exits` may appear at most once per frame".to_string()),
        ));
    }
    *exits = targets;
    Ok(())
}

/// The open frame's `(tapes.len(), &mut maps)` when the last table is a
/// frame still accepting continuations; `None` otherwise (orphan `.map`).
fn open_frame_mut(ctx: &mut LowerCtx) -> Option<(usize, &mut Vec<FrameTapeMap>)> {
    if !ctx.run_open {
        return None;
    }
    match ctx.tables.last_mut() {
        Some(SourceTable::Frame { tapes, maps, .. }) => Some((tapes.len(), maps)),
        _ => None,
    }
}

/// The open frame table (mutable) when the last table is a frame still
/// accepting continuations; `None` otherwise (orphan `.exits`).
fn open_frame_table_mut(ctx: &mut LowerCtx) -> Option<&mut SourceTable> {
    if !ctx.run_open {
        return None;
    }
    match ctx.tables.last_mut() {
        table @ Some(SourceTable::Frame { .. }) => table,
        _ => None,
    }
}

/// Materializes a `.map` pair list into a dense `max index + 1` u16 table:
/// index 0 is forced to identity (0->0) so the blank symbol always reads
/// and writes as blank, unset indices are holes (`0xFFFF`), and
/// index/value past `0xFFFE` is rejected. Blank pinning is one-directional:
/// index 0 is pinned to 0 — a `0->X` pair with X != 0 is rejected in BOTH
/// maps — but a non-blank index MAY map onto 0. Collapsing a symbol onto
/// blank is ordinary tape behaviour: reading a foreign boundary marker AS
/// the callee's blank (`Y->0` in rmap) is the flagship one-way pattern, and
/// writing a virtual symbol back as the physical blank (`Y->0` in wmap) is
/// an erase. So only index 0 itself is a fixed point; whether a given fold
/// is sound (a non-injective map) is the composition engine's binding
/// check, not this raw descriptor-authoring surface. The one-way (`=>`) bit
/// does not affect the descriptor bytes (the wire form has no one-way
/// flag). An empty pair list is the identity map (`len 0`).
fn build_dense_map(pairs: &[FramePairCst], span: Span) -> Result<Vec<u16>, AsmError> {
    // Validate every pair against the blank-pinning rule and the
    // index/value ceiling, collecting the effective (index != 0) entries.
    // An explicit `0->0` pair is the forced identity itself and contributes
    // nothing — dropping it keeps the dense form canonical (the
    // disassembler never re-emits index 0), so asm∘dis∘asm stays
    // byte-identical.
    let mut max_idx = 0u32;
    let mut effective: Vec<(u32, u32)> = Vec::new();
    for p in pairs {
        if p.from > 0xFFFE || p.to > 0xFFFE {
            return Err(err(
                span,
                AsmErrorKind::BadFrame("frame map index/value exceeds 0xFFFE".to_string()),
            ));
        }
        if p.from == 0 {
            // Index 0 is pinned to identity: blank reads and writes as
            // blank. A `0->X` with X != 0 is the only blank-rule rejection.
            if p.to != 0 {
                return Err(err(
                    span,
                    AsmErrorKind::BadFrame("frame map unpins blank: 0 must map to 0".to_string()),
                ));
            }
            continue; // 0->0 is the forced identity; no dense entry
        }
        // A non-blank index MAY map onto 0 — folding a symbol onto blank (a
        // marker read as blank in rmap, an erase in wmap). Only index 0
        // itself is a fixed point; fold soundness is the composition
        // engine's binding check, not this authoring surface.
        max_idx = max_idx.max(p.from);
        effective.push((p.from, p.to));
    }
    if effective.is_empty() {
        return Ok(Vec::new()); // identity map (empty or all 0->0)
    }
    let mut table = vec![0xFFFFu16; max_idx as usize + 1];
    table[0] = 0; // index 0 pinned: the blank symbol maps to itself
    for (from, to) in effective {
        table[from as usize] = to as u16;
    }
    Ok(table)
}

/// `.routine <name>, tapes=<int>, alpha=(<int>, …)`: declares the named
/// function's generic-routine signature (docs/formats.md (MO)). Rules:
/// code section only; the directive must PRECEDE its `.func` in the
/// same file, any distance — it attaches when the function is defined,
/// and one still pending at end of input is reported there; one
/// directive per function; tapes in 1..=16; the alpha list's length
/// equals tapes; every cardinality at least 1.
fn lower_routine_directive(d: &RoutineDirectiveCst, ctx: &mut LowerCtx) -> Result<(), AsmError> {
    // A pending label cannot bind across a directive (same rule as the
    // `.func` and `.section` boundaries).
    if let Some(first) = ctx.pending.first() {
        return Err(err(
            first.span,
            AsmErrorKind::Syntax("label at end of function"),
        ));
    }
    if ctx.section == Section::Tables {
        return Err(err(
            d.span,
            AsmErrorKind::BadTable("only table directives are allowed in the tables section"),
        ));
    }
    if !(1..=16).contains(&d.tapes) {
        return Err(err(
            d.tapes_span,
            AsmErrorKind::BadSignature("tapes must be 1..=16".to_string()),
        ));
    }
    if d.alpha.len() != d.tapes as usize {
        return Err(err(
            d.alpha_span,
            AsmErrorKind::BadSignature(format!(
                "alpha lists {} cardinalities for tapes={}",
                d.alpha.len(),
                d.tapes
            )),
        ));
    }
    if d.alpha.contains(&0) {
        return Err(err(
            d.alpha_span,
            AsmErrorKind::BadSignature("alphabet cardinalities are at least 1".to_string()),
        ));
    }
    let already_pending = ctx.pending_sigs.iter().any(|(n, _)| n.name == d.name);
    let already_attached = ctx
        .functions
        .iter()
        .position(|f| f.name == d.name)
        .is_some_and(|i| ctx.func_sigs[i].is_some());
    if already_pending || already_attached {
        return Err(err(
            d.name_span,
            AsmErrorKind::BadSignature(format!("duplicate `.routine` for `{}`", d.name)),
        ));
    }
    ctx.pending_sigs.push((
        SpannedName {
            name: d.name.clone(),
            span: d.name_span,
        },
        RoutineSig {
            arity: d.tapes as u8,
            cardinalities: d.alpha.clone(),
        },
    ));
    Ok(())
}

/// Detaches the pending `.routine` signature for a function being
/// defined, if one was declared. Called by BOTH `.func` lowering paths
/// so the parallel `func_sigs` vector never falls out of step.
fn take_pending_sig(ctx: &mut LowerCtx, name: &str) -> Option<RoutineSig> {
    ctx.pending_sigs
        .iter()
        .position(|(n, _)| n.name == name)
        .map(|i| ctx.pending_sigs.remove(i).1)
}

/// `.section NAME`: switches the section cursor and closes any open
/// table run. Only `code` and `tables` exist.
fn lower_section(section: &SectionCst, ctx: &mut LowerCtx) -> Result<(), AsmError> {
    // A pending label cannot bind across a section boundary — same rule
    // as a label immediately before `.func`.
    if let Some(first) = ctx.pending.first() {
        return Err(err(
            first.span,
            AsmErrorKind::Syntax("label at end of function"),
        ));
    }
    ctx.run_open = false;
    ctx.section = match section.name.as_str() {
        "code" => Section::Code,
        "tables" => Section::Tables,
        _ => {
            return Err(err(
                section.span,
                AsmErrorKind::BadTable("unknown section (expected `code` or `tables`)"),
            ));
        }
    };
    Ok(())
}

/// A table directive's parsed payload, before run attachment.
enum ParsedDirective {
    Row(SourceRow),
    Targets(Vec<SpannedName>),
}

/// `.row [..]` / `.targets L1, ..` / `.target L`: legal only inside
/// `.section tables`. A LABELED directive opens a table; unlabeled
/// directives continue the open run. A labeled directive naming the OPEN
/// run of the same kind continues it instead — that is what a `.rept`
/// around `T: .row [..{v}..]` expands to, one same-labeled row per
/// iteration.
fn lower_table_directive(d: &TableDirectiveCst, ctx: &mut LowerCtx) -> Result<(), AsmError> {
    if ctx.section != Section::Tables {
        return Err(err(
            d.span,
            AsmErrorKind::BadTable("table directives live in the tables section"),
        ));
    }
    for label in &d.labels {
        if !is_label_name(&label.name) {
            return Err(err(
                label.span,
                AsmErrorKind::Syntax("label names use letters, digits, underscore"),
            ));
        }
    }
    if d.labels.len() > 1 {
        return Err(err(
            d.labels[1].span,
            AsmErrorKind::BadTable("one label per table directive"),
        ));
    }

    let parsed = match d.kind {
        TableDirectiveKind::Row => {
            // The CST shapes `.row` only around a single bracketed
            // vector; the guard is defensive.
            let [token] = d.operands.as_slice() else {
                return Err(err(
                    d.span,
                    AsmErrorKind::BadVector("`.row` takes one bracketed vector"),
                ));
            };
            let elems = parse_vector(token)?;
            for elem in &elems {
                match elem {
                    // 0x7F is the wildcard byte, so exact payloads stop at 0x7E.
                    VecElem::Payload(p) if *p > 0x7E => {
                        return Err(err(
                            token.span,
                            AsmErrorKind::BadVector("match payloads are at most 126"),
                        ));
                    }
                    VecElem::Payload(_) | VecElem::Wildcard => {}
                    _ => {
                        return Err(err(
                            token.span,
                            AsmErrorKind::BadVector("match rows allow payloads and `*` only"),
                        ));
                    }
                }
            }
            ParsedDirective::Row(SourceRow {
                elems,
                span: d.span,
            })
        }
        TableDirectiveKind::Targets | TableDirectiveKind::Target => {
            if d.operands.is_empty() {
                return Err(err(
                    d.span,
                    AsmErrorKind::BadTable("a dispatch table needs at least one target"),
                ));
            }
            if matches!(d.kind, TableDirectiveKind::Target) && d.operands.len() != 1 {
                return Err(err(
                    d.operands[1].span,
                    AsmErrorKind::BadTable("`.target` takes one label"),
                ));
            }
            let mut targets = Vec::with_capacity(d.operands.len());
            for operand in &d.operands {
                if !is_label_name(&operand.text) {
                    return Err(err(
                        operand.span,
                        AsmErrorKind::BadTable("dispatch targets are label names"),
                    ));
                }
                targets.push(SpannedName {
                    name: operand.text.clone(),
                    span: operand.span,
                });
            }
            ParsedDirective::Targets(targets)
        }
    };

    match d.labels.first() {
        Some(label) => {
            // A labeled directive continuing the open run of the same
            // name AND kind appends; anything else opens a fresh table
            // under a fresh (file-scoped) name.
            let continues = ctx.run_open
                && match (ctx.tables.last(), &parsed) {
                    (Some(SourceTable::Match { name, .. }), ParsedDirective::Row(_)) => {
                        name.name == label.name
                    }
                    (Some(SourceTable::Dispatch { name, .. }), ParsedDirective::Targets(_)) => {
                        name.name == label.name
                    }
                    _ => false,
                };
            if continues {
                append_to_run(ctx.tables.last_mut().expect("run open"), parsed);
            } else {
                if ctx.tables.iter().any(|t| t.name().name == label.name) {
                    return Err(err(
                        label.span,
                        AsmErrorKind::DuplicateLabel(label.name.clone()),
                    ));
                }
                let name = spanned(label);
                ctx.tables.push(match parsed {
                    ParsedDirective::Row(row) => SourceTable::Match {
                        name,
                        rows: vec![row],
                    },
                    ParsedDirective::Targets(targets) => SourceTable::Dispatch { name, targets },
                });
                ctx.run_open = true;
            }
        }
        None => {
            if !ctx.run_open {
                return Err(err(
                    d.span,
                    AsmErrorKind::BadTable("a table starts with a labeled directive"),
                ));
            }
            let table = ctx.tables.last_mut().expect("run open");
            match (&*table, &parsed) {
                (SourceTable::Match { .. }, ParsedDirective::Row(_))
                | (SourceTable::Dispatch { .. }, ParsedDirective::Targets(_)) => {
                    append_to_run(table, parsed);
                }
                _ => {
                    return Err(err(
                        d.span,
                        AsmErrorKind::BadTable("cannot mix rows and targets in one table"),
                    ));
                }
            }
        }
    }
    Ok(())
}

/// Appends a parsed directive to a run whose kind is already known to
/// match (both callers check).
fn append_to_run(table: &mut SourceTable, parsed: ParsedDirective) {
    match (table, parsed) {
        (SourceTable::Match { rows, .. }, ParsedDirective::Row(row)) => rows.push(row),
        (SourceTable::Dispatch { targets, .. }, ParsedDirective::Targets(mut more)) => {
            targets.append(&mut more);
        }
        _ => unreachable!("caller checked the run kind"),
    }
}

/// Parses a verbatim `[..]` operand token into vector elements. Element
/// LEGALITY per context is the caller's (ultimately the dialect's) call;
/// this accepts the full element vocabulary.
fn parse_vector(token: &OperandToken) -> Result<Vec<VecElem>, AsmError> {
    parse_vector_text(&token.text, token.span)
}

/// Splits a two-bracket-group operand `[w...], [m...]` — the verbatim text
/// the CST captures for a `wrmv`-shaped instruction line (first `[` to
/// last `]`, one operand token) — into its two group slices. Requires
/// exactly two `[..]` groups separated by ONE bracket-depth-0 comma;
/// `None` on any other shape (one group, three groups, unbalanced).
fn split_two_bracket_groups(text: &str) -> Option<(&str, &str)> {
    let mut depth = 0i32;
    let mut at = None;
    for (i, c) in text.char_indices() {
        match c {
            '[' => depth += 1,
            ']' => depth -= 1,
            ',' if depth == 0 => {
                if at.is_some() {
                    return None; // a third group / extra top-level comma
                }
                at = Some(i);
            }
            _ => {}
        }
    }
    let at = at?;
    let first = text[..at].trim();
    let second = text[at + 1..].trim();
    (first.starts_with('[')
        && first.ends_with(']')
        && second.starts_with('[')
        && second.ends_with(']'))
    .then_some((first, second))
}

/// Parses a verbatim `[..]` vector's text into elements at `span`. The
/// text-and-span form of [`parse_vector`], shared with the two-vector
/// `wrmv` classification where each group is a slice of one operand token.
fn parse_vector_text(text: &str, span: Span) -> Result<Vec<VecElem>, AsmError> {
    let inner = text
        .strip_prefix('[')
        .and_then(|t| t.strip_suffix(']'))
        .ok_or_else(|| err(span, AsmErrorKind::BadVector("expected a `[..]` vector")))?;
    let mut elems = Vec::new();
    for part in inner.split(',') {
        let elem = match part.trim() {
            "*" => VecElem::Wildcard,
            "-" => VecElem::Keep,
            "<" => VecElem::MoveLeft,
            ">" => VecElem::MoveRight,
            "." => VecElem::Stay,
            // `[]` also lands here: its one split part is empty.
            "" => {
                return Err(err(span, AsmErrorKind::BadVector("empty vector element")));
            }
            payload => VecElem::Payload(payload.parse::<u32>().map_err(|_| {
                err(
                    span,
                    AsmErrorKind::BadVector(
                        "vector elements are integers or `*`, `-`, `<`, `>`, `.`",
                    ),
                )
            })?),
        };
        elems.push(elem);
    }
    Ok(elems)
}

/// Expands a `.rept v, lo, hi` … `.endr` block textually (the GNU-as
/// model): for each `value` in `lo..=hi`, every body line is recovered
/// verbatim from `source`, its `{expr}` markers substituted, and the
/// result re-parsed and lowered through [`lower_item`] as if written
/// inline. Diagnostics point at the original body line's span — both the
/// substitution error and any error lowering the expanded line.
fn lower_rept(
    rept: &ReptCst,
    syntax: &ArchSyntax,
    source: &str,
    ctx: &mut LowerCtx,
) -> Result<(), AsmError> {
    if rept.lo > rept.hi {
        return Err(err(rept.span, AsmErrorKind::BadRept));
    }
    for value in rept.lo..=rept.hi {
        for body_item in &rept.body {
            // Comment body items carry no line number and lower to
            // nothing regardless of substitution, so skipping is
            // equivalent to recover + re-parse + no-op lower.
            let Some(span) = body_item_span(body_item) else {
                continue;
            };
            // Recover the WHOLE physical line (leading indent and any
            // trailing comment ride along — the column-span slice would
            // drop them). Every body item is exactly one physical line.
            let line_text = source
                .lines()
                .nth(span.start.line as usize - 1)
                .unwrap_or_default();
            let expanded = substitute(line_text, &rept.var, value)
                .map_err(|m| err(span, AsmErrorKind::BadSubstitution(m)))?;
            // Re-parse under the same dialect caps. A single line yields
            // at most one item; a nested `.rept` cannot re-open here (a
            // block needs its own `.endr`, absent from one line).
            let cst = parse_asm_cst_with(&expanded, syntax.caps);
            for expanded_item in &cst.items {
                lower_item(expanded_item, syntax, source, ctx).map_err(|e| err(span, e.kind))?;
            }
        }
    }
    Ok(())
}

/// The source span of a CST item, or `None` for a [`AsmItemKind::Comment`]
/// (which carries only a column). Used by [`lower_rept`] to find each
/// body line's physical line for verbatim recovery.
fn body_item_span(item: &AsmItem) -> Option<Span> {
    match &item.kind {
        AsmItemKind::Comment(_) => None,
        AsmItemKind::Func(f) => Some(f.span),
        AsmItemKind::Line(l) => Some(l.span),
        AsmItemKind::Raw(r) => Some(r.span),
        AsmItemKind::Section(s) => Some(s.span),
        AsmItemKind::TableDirective(d) => Some(d.span),
        AsmItemKind::Rept(r) => Some(r.span),
        AsmItemKind::RoutineDirective(d) => Some(d.span),
        AsmItemKind::FrameDirective(d) => Some(d.span()),
    }
}

fn lower_func(func: &FuncCst, ctx: &mut LowerCtx) -> Result<(), AsmError> {
    // A label immediately before a `.func` binds to nothing (legacy: the
    // first check in the `.func` branch, before the name is parsed).
    if let Some(first) = ctx.pending.first() {
        return Err(err(
            first.span,
            AsmErrorKind::Syntax("label at end of function"),
        ));
    }
    if ctx.section == Section::Tables {
        return Err(err(
            func.name_span,
            AsmErrorKind::BadTable("functions are not allowed in the tables section"),
        ));
    }
    if !is_symbol_name(&func.name) {
        return Err(err(
            func.name_span,
            AsmErrorKind::Syntax("bad function name"),
        ));
    }
    if ctx.functions.iter().any(|f| f.name == func.name) {
        return Err(err(
            func.name_span,
            AsmErrorKind::DuplicateFunction(func.name.clone()),
        ));
    }
    let sig = take_pending_sig(ctx, &func.name);
    ctx.functions.push(SourceFunction {
        name: func.name.clone(),
        name_span: func.name_span,
        local: func.local,
        items: Vec::new(),
    });
    ctx.func_sigs.push(sig);
    Ok(())
}

fn lower_line(line: &LineCst, syntax: &ArchSyntax, ctx: &mut LowerCtx) -> Result<(), AsmError> {
    // Every label name must be a bare identifier. This is where
    // `foo.bar:` and `std::x:` are rejected — the CST shapes them as
    // label candidates; the tightening lives here.
    for label in &line.labels {
        if !is_label_name(&label.name) {
            return Err(err(
                label.span,
                AsmErrorKind::Syntax("label names use letters, digits, underscore"),
            ));
        }
    }

    // A malformed frame directive — the CST keeps it a Line when the
    // directive is not structurally exact (mirror `.routine`/`.func`) —
    // gets a precise complaint instead of UnknownMnemonic, in either
    // section. Only for dialects whose tables cap could shape one at all.
    if let Some(instr) = &line.instr
        && matches!(instr.word.as_str(), ".frame" | ".map" | ".exits")
        && syntax.caps.tables
    {
        return Err(err(
            instr.word_span,
            AsmErrorKind::BadFrame(format!("malformed `{}` directive", instr.word)),
        ));
    }

    // Inside the tables section only table directives are legal. A
    // `.row` whose operand region was not one bracketed vector degrades
    // to a Line (CST rule) — give it the precise vector complaint rather
    // than the generic section one.
    if ctx.section == Section::Tables {
        if let Some(instr) = &line.instr
            && instr.word == ".row"
        {
            return Err(err(
                instr.word_span,
                AsmErrorKind::BadVector("`.row` takes one bracketed vector"),
            ));
        }
        return Err(err(
            line.span,
            AsmErrorKind::BadTable("only table directives are allowed in the tables section"),
        ));
    }

    let Some(instr) = &line.instr else {
        // Label-only line. Outside any function it is stray code;
        // otherwise the labels wait for the next instruction.
        if ctx.functions.is_empty() {
            // A label-only line always carries at least one label.
            return Err(err(line.labels[0].span, AsmErrorKind::OutsideFunction));
        }
        ctx.pending.extend(line.labels.iter().map(spanned));
        return Ok(());
    };

    // A malformed `.func` directive — the CST keeps it a Line with word
    // ".func" when the directive is not structurally exact. Only when
    // ".func" is the instruction word with no labels before it;
    // `L1: .func …` is a plain unknown mnemonic. This fires before the
    // open-function check, matching the legacy `.func`-branch precedence.
    if instr.word == ".func" && line.labels.is_empty() {
        return lower_malformed_func(instr, ctx);
    }

    // A malformed `.routine` — the CST keeps it a Line when the
    // directive is not structurally exact — gets a precise complaint
    // instead of UnknownMnemonic. Only for dialects whose caps could
    // shape one at all: with tables off the word is as unknown as any
    // other, exactly as before the directive existed.
    if instr.word == ".routine" && line.labels.is_empty() && syntax.caps.tables {
        return Err(err(
            instr.word_span,
            AsmErrorKind::Syntax("`.routine` takes `<name>, tapes=<int>, alpha=(<int>, …)`"),
        ));
    }

    // Outside any function an instruction is stray code — reported
    // before mnemonic lookup (matches the pinned `.function f` case).
    if ctx.functions.is_empty() {
        return Err(err(instr.word_span, AsmErrorKind::OutsideFunction));
    }

    // Labels bound to this instruction: those pending from prior
    // label-only lines, then this line's own.
    let mut labels: Vec<SpannedName> = std::mem::take(&mut ctx.pending);
    labels.extend(line.labels.iter().map(spanned));

    let item = if instr.word == ".byte" {
        SourceItem::RawByte {
            span: line.span,
            labels,
            value: lower_byte(instr)?,
        }
    } else {
        let entry = syntax.by_mnemonic(&instr.word).ok_or_else(|| {
            err(
                instr.word_span,
                AsmErrorKind::UnknownMnemonic(instr.word.clone()),
            )
        })?;
        SourceItem::Instr {
            span: line.span,
            labels,
            opcode: entry.opcode,
            operand: classify_operand(entry, instr)?,
        }
    };
    ctx.functions
        .last_mut()
        .expect("function open")
        .items
        .push(item);
    Ok(())
}

/// Replicates the legacy `.func`-branch checks for a directive that did
/// not shape as a [`FuncCst`]. `rest` is reconstructed from the operand
/// region (comma-joined so the legacy whitespace tokenization is
/// preserved); spans point at the `.func` word, except the
/// pending-label check which points at the label.
fn lower_malformed_func(instr: &InstrCst, ctx: &mut LowerCtx) -> Result<(), AsmError> {
    // Same first check as the exact-`.func` path: a label immediately
    // before any `.func` (well-formed or not) binds to nothing.
    if let Some(first) = ctx.pending.first() {
        return Err(err(
            first.span,
            AsmErrorKind::Syntax("label at end of function"),
        ));
    }
    let word_span = instr.word_span;
    let rest = instr
        .operands
        .iter()
        .map(|o| o.text.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    let mut words = rest.split_whitespace();
    let name = words.next().unwrap_or("");
    let local = match words.next() {
        None => false,
        Some("local") => {
            if words.next().is_some() {
                return Err(err(word_span, AsmErrorKind::Syntax("junk after `local`")));
            }
            true
        }
        Some(_) => {
            return Err(err(
                word_span,
                AsmErrorKind::Syntax("expected `local` or end of line after the name"),
            ));
        }
    };
    if !is_symbol_name(name) {
        return Err(err(word_span, AsmErrorKind::Syntax("bad function name")));
    }
    if ctx.functions.iter().any(|f| f.name == name) {
        return Err(err(
            word_span,
            AsmErrorKind::DuplicateFunction(name.to_string()),
        ));
    }
    let sig = take_pending_sig(ctx, name);
    ctx.functions.push(SourceFunction {
        name: name.to_string(),
        name_span: word_span,
        local,
        items: Vec::new(),
    });
    ctx.func_sigs.push(sig);
    Ok(())
}

/// `.byte N` — a single 0..=255 operand. Span on the operand, or on the
/// `.byte` word when the operand is missing.
fn lower_byte(instr: &InstrCst) -> Result<u8, AsmError> {
    let [operand] = instr.operands.as_slice() else {
        let span = instr.operands.first().map_or(instr.word_span, |o| o.span);
        return Err(err(span, AsmErrorKind::BadOperand(".byte needs 0..=255")));
    };
    operand.text.parse::<u8>().map_err(|_| {
        err(
            operand.span,
            AsmErrorKind::BadOperand(".byte needs 0..=255"),
        )
    })
}

fn classify_operand(entry: &SyntaxEntry, instr: &InstrCst) -> Result<SourceOperand, AsmError> {
    let operands = &instr.operands;
    match entry.operand {
        OperandKind::None => {
            if let Some(first) = operands.first() {
                return Err(err(
                    first.span,
                    AsmErrorKind::BadOperand("takes no operand"),
                ));
            }
            Ok(SourceOperand::None)
        }
        OperandKind::RelI8 | OperandKind::RelI32 => {
            // Declarative binding-call form: `call <name> [<binding>]` —
            // a call target then a trailing bracket group. The bracket is
            // captured as one verbatim operand by the CST (docs/formats.md
            // (bound calls)); only a call takes a binding.
            if let [target, bracket] = operands.as_slice()
                && bracket.text.starts_with('[')
            {
                return classify_bound_call(entry, target, bracket);
            }
            let [one] = operands.as_slice() else {
                return Err(err(
                    instr.word_span,
                    AsmErrorKind::BadOperand("takes one name"),
                ));
            };
            if let Some(sym) = one.text.strip_prefix('@') {
                if !is_symbol_name(sym) {
                    return Err(err(
                        one.span,
                        AsmErrorKind::BadOperand("bad symbol name after `@`"),
                    ));
                }
                Ok(SourceOperand::SymbolName(SpannedName {
                    name: sym.to_string(),
                    span: one.span,
                }))
            } else {
                if !is_symbol_name(&one.text) {
                    return Err(err(
                        one.span,
                        AsmErrorKind::BadOperand("jump/call operands are names, not numbers"),
                    ));
                }
                Ok(SourceOperand::Name(SpannedName {
                    name: one.text.clone(),
                    span: one.span,
                }))
            }
        }
        OperandKind::SymbolVec => {
            // A bracketed `[..]` region reaches here as ONE verbatim
            // token (caps.vectors CST rule) and classifies as a vector;
            // per-mnemonic encoding of vectors is the dialect's job.
            if let [one] = operands.as_slice()
                && one.text.starts_with('[')
            {
                return Ok(SourceOperand::Vector(parse_vector(one)?, one.span));
            }
            if operands.is_empty() {
                return Err(err(
                    instr.word_span,
                    AsmErrorKind::BadOperand("takes symbol indices"),
                ));
            }
            let mut ints = Vec::with_capacity(operands.len());
            for o in operands {
                ints.push(o.text.parse::<i64>().map_err(|_| {
                    err(
                        o.span,
                        AsmErrorKind::BadOperand("symbol indices are integers"),
                    )
                })?);
            }
            Ok(SourceOperand::Ints(ints))
        }
        OperandKind::MoveVec => {
            // A move vector is written in bracket form only (`[<, ., >]`),
            // routed exactly like SymbolVec's bracketed spelling; unlike
            // SymbolVec there is no legacy spelled-out-ints form to keep.
            if let [one] = operands.as_slice()
                && one.text.starts_with('[')
            {
                return Ok(SourceOperand::Vector(parse_vector(one)?, one.span));
            }
            Err(err(
                instr.word_span,
                AsmErrorKind::BadOperand("takes a `[..]` move vector"),
            ))
        }
        OperandKind::WriteMoveVec => {
            // `wrmv [w...], [m...]`: the CST captures a bracketed region as
            // ONE verbatim `[..]` token from the first `[` to the last `]`,
            // so both groups arrive in a single operand's text. Split at the
            // depth-0 comma between them into the write and move groups; the
            // per-group element vocabulary is enforced at emit.
            let [one] = operands.as_slice() else {
                return Err(err(
                    instr.word_span,
                    AsmErrorKind::BadOperand(
                        "takes a write vector then a move vector: `[w…], [m…]`",
                    ),
                ));
            };
            let Some((w_text, m_text)) = split_two_bracket_groups(&one.text) else {
                return Err(err(
                    one.span,
                    AsmErrorKind::BadVector("expected two `[..]` vectors: a write and a move"),
                ));
            };
            let writes = parse_vector_text(w_text, one.span)?;
            let moves = parse_vector_text(m_text, one.span)?;
            Ok(SourceOperand::WriteMoveVectors(writes, moves, one.span))
        }
        OperandKind::TableRef => {
            // A table reference is a file-scoped table LABEL (label
            // grammar, not the dotted/namespaced symbol grammar).
            let [one] = operands.as_slice() else {
                return Err(err(
                    instr.word_span,
                    AsmErrorKind::BadOperand("takes one table label"),
                ));
            };
            if !is_label_name(&one.text) {
                return Err(err(
                    one.span,
                    AsmErrorKind::BadOperand("table references are labels"),
                ));
            }
            Ok(SourceOperand::Name(SpannedName {
                name: one.text.clone(),
                span: one.span,
            }))
        }
        OperandKind::Imm8 => {
            // Exactly one `#<int>` operand, range 0..=255.
            let [one] = operands.as_slice() else {
                return Err(err(
                    instr.word_span,
                    AsmErrorKind::BadOperand("takes one `#<n>` immediate"),
                ));
            };
            let digits = one.text.strip_prefix('#').ok_or_else(|| {
                err(
                    one.span,
                    AsmErrorKind::BadOperand("immediates are written `#<n>`"),
                )
            })?;
            let value = digits.parse::<u8>().map_err(|_| {
                err(
                    one.span,
                    AsmErrorKind::BadOperand("immediate must be 0..=255"),
                )
            })?;
            Ok(SourceOperand::Imm(value))
        }
        OperandKind::FramedCall => {
            // `<target>, <frame>`: a call target (symbol name, like a
            // plain call's) and a frame table LABEL (like a TableRef).
            let [target, frame] = operands.as_slice() else {
                return Err(err(
                    instr.word_span,
                    AsmErrorKind::BadOperand("takes a call target and a frame table label"),
                ));
            };
            // Target half — same grammar as a plain call target; `@name`
            // is rejected exactly as a call rejects it (already a symbol).
            if target.text.starts_with('@') {
                return Err(err(
                    target.span,
                    AsmErrorKind::BadOperand(
                        "framed-call targets are already symbols; drop the `@`",
                    ),
                ));
            }
            if !is_symbol_name(&target.text) {
                return Err(err(
                    target.span,
                    AsmErrorKind::BadOperand("framed-call targets are names, not numbers"),
                ));
            }
            // Frame half — a file-scoped table LABEL.
            if !is_label_name(&frame.text) {
                return Err(err(
                    frame.span,
                    AsmErrorKind::BadOperand("frame references are table labels"),
                ));
            }
            Ok(SourceOperand::FramedCall {
                target: SpannedName {
                    name: target.text.clone(),
                    span: target.span,
                },
                frame: SpannedName {
                    name: frame.text.clone(),
                    span: frame.span,
                },
            })
        }
    }
}

/// Classifies a declarative binding call (`call <name> [<binding>]`). The
/// `target` is a plain call target and `bracket` the verbatim `[..]`
/// operand. Only a `Flow::Call` mnemonic takes a binding; jumps/branches
/// with a trailing bracket are rejected. Structural validation lives here
/// (physical index `< 16`, canonical `u32` src/dst, no duplicate source
/// in one entry, non-empty binding); mapping legality — the blank↔blank
/// rule, bijection, write-back consistency — is the composition engine's,
/// checked at link time (docs/formats.md (bound calls)).
fn classify_bound_call(
    entry: &SyntaxEntry,
    target: &OperandToken,
    bracket: &OperandToken,
) -> Result<SourceOperand, AsmError> {
    if entry.flow != Flow::Call {
        return Err(err(
            bracket.span,
            AsmErrorKind::BadOperand("only a call takes a tape binding"),
        ));
    }
    // Target half — same grammar as a plain call target.
    if target.text.starts_with('@') {
        return Err(err(
            target.span,
            AsmErrorKind::BadOperand("call targets are already symbols; drop the `@`"),
        ));
    }
    if !is_symbol_name(&target.text) {
        return Err(err(
            target.span,
            AsmErrorKind::BadOperand("call targets are names, not numbers"),
        ));
    }
    let inner = bracket
        .text
        .strip_prefix('[')
        .and_then(|t| t.strip_suffix(']'))
        .ok_or_else(|| {
            err(
                bracket.span,
                AsmErrorKind::BadFrame("malformed tape binding".into()),
            )
        })?;
    let entries = parse_binding(inner, bracket.span.start.line).ok_or_else(|| {
        err(
            bracket.span,
            AsmErrorKind::BadFrame("malformed tape binding".into()),
        )
    })?;
    if entries.is_empty() {
        return Err(err(
            bracket.span,
            AsmErrorKind::BadFrame(
                "a binding call needs at least one tape entry; use a plain `call` for none".into(),
            ),
        ));
    }
    let mut binding = Vec::with_capacity(entries.len());
    for (phys, pairs) in entries {
        let caller_tape = u8::try_from(phys).ok().filter(|&p| p < 16).ok_or_else(|| {
            err(
                bracket.span,
                AsmErrorKind::BadFrame("binding physical tape index must be < 16".into()),
            )
        })?;
        // A source symbol may bind at most once per tape — a repeated src
        // is an ambiguous map, rejected regardless of composition rules.
        let mut seen = Vec::with_capacity(pairs.len());
        for p in &pairs {
            if seen.contains(&p.from) {
                return Err(err(
                    bracket.span,
                    AsmErrorKind::BadFrame("duplicate source symbol in a tape binding".into()),
                ));
            }
            seen.push(p.from);
        }
        binding.push(SourceTapeBinding {
            caller_tape,
            pairs: pairs.iter().map(|p| (p.from, p.to, p.one_way)).collect(),
        });
    }
    Ok(SourceOperand::BoundCallOp {
        target: SpannedName {
            name: target.text.clone(),
            span: target.span,
        },
        binding,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::asm::cst::{parse_asm_cst, parse_asm_cst_with};
    use crate::asm::syntax::AsmCaps;
    use crate::asm::syntax::fixture::test_syntax;

    fn lower_src(src: &str) -> Result<Vec<SourceFunction>, AsmError> {
        lower(&parse_asm_cst(src), &test_syntax(), src)
    }

    /// `test_syntax()` with the `.rept` cap on, so `.rept … .endr` blocks
    /// shape (and expand) instead of degrading. Everything else is the
    /// classic fixture.
    fn rept_syntax() -> ArchSyntax {
        let mut syntax = test_syntax();
        syntax.caps = AsmCaps {
            rept: true,
            ..Default::default()
        };
        syntax
    }

    /// Lower under [`rept_syntax`], parsing the CST with the matching caps
    /// so `.rept` blocks are shaped before lowering expands them.
    fn lower_rept_src(src: &str) -> Result<Vec<SourceFunction>, AsmError> {
        let syntax = rept_syntax();
        lower(&parse_asm_cst_with(src, syntax.caps), &syntax, src)
    }

    fn label_names(labels: &[SpannedName]) -> Vec<&str> {
        labels.iter().map(|l| l.name.as_str()).collect()
    }

    #[test]
    fn parses_functions_labels_and_operands() {
        let src = "\
; a comment line
.func f
L1:     nop
        jmp     L1      ; loop
        wr      1, 2
        call    g
        ret
.func g
        stop
";
        let funcs = lower_src(src).unwrap();
        assert_eq!(funcs.len(), 2);
        assert_eq!(funcs[0].name, "f");
        assert_eq!(funcs[0].name_span, Span::new(2, 7, 2, 8));
        let items = &funcs[0].items;
        assert_eq!(items.len(), 5);
        match &items[0] {
            SourceItem::Instr {
                labels,
                opcode,
                operand,
                ..
            } => {
                assert_eq!(label_names(labels), vec!["L1"]);
                assert_eq!(*opcode, 0x01);
                assert!(matches!(operand, SourceOperand::None));
            }
            other => panic!("unexpected {other:?}"),
        }
        match &items[1] {
            SourceItem::Instr {
                opcode, operand, ..
            } => {
                assert_eq!(*opcode, 0x20);
                assert!(matches!(operand, SourceOperand::Name(n) if n.name == "L1"));
            }
            other => panic!("unexpected {other:?}"),
        }
        match &items[2] {
            SourceItem::Instr { operand, .. } => {
                assert!(matches!(operand, SourceOperand::Ints(v) if v == &vec![1, 2]));
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn label_only_line_binds_to_next_instruction() {
        let src = ".func f\nL1:\nL2:\n        nop\n";
        let funcs = lower_src(src).unwrap();
        match &funcs[0].items[0] {
            SourceItem::Instr { labels, .. } => {
                assert_eq!(label_names(labels), vec!["L1", "L2"]);
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn byte_directive_parses() {
        let src = ".func f\n        .byte 255\n";
        let funcs = lower_src(src).unwrap();
        assert!(matches!(
            funcs[0].items[0],
            SourceItem::RawByte { value: 255, .. }
        ));
    }

    #[test]
    fn func_directive_requires_exact_token() {
        // `.function` must never be silently accepted as `.func`. With no
        // function open, the open-function check fires first, so the
        // error is OutsideFunction. Inside a function, the word reaches
        // mnemonic lookup and reports UnknownMnemonic.
        let e = lower_src(".function f\n").unwrap_err();
        assert_eq!(e.kind, AsmErrorKind::OutsideFunction);
        assert_eq!(e.span, Span::new(1, 1, 1, 10));

        let e = lower_src(".func f\n.function g\n").unwrap_err();
        assert!(matches!(e.kind, AsmErrorKind::UnknownMnemonic(ref m) if m == ".function"));
        assert_eq!(e.span, Span::new(2, 1, 2, 10)); // `.function` is 9 chars
    }

    #[test]
    fn error_cases_carry_spans() {
        let e = lower_src("        nop\n").unwrap_err();
        assert_eq!(e.kind, AsmErrorKind::OutsideFunction);
        assert_eq!(e.span, Span::new(1, 9, 1, 12)); // the `nop` word

        let e = lower_src(".func f\n        bogus\n").unwrap_err();
        assert!(matches!(e.kind, AsmErrorKind::UnknownMnemonic(ref m) if m == "bogus"));
        assert_eq!(e.span, Span::new(2, 9, 2, 14));

        let e = lower_src(".func f\n.func f\n        nop\n").unwrap_err();
        assert!(matches!(e.kind, AsmErrorKind::DuplicateFunction(ref n) if n == "f"));
        assert_eq!(e.span, Span::new(2, 7, 2, 8)); // the second `f`

        let e = lower_src(".func f\n        jmp 5\n").unwrap_err();
        assert!(matches!(e.kind, AsmErrorKind::BadOperand(_))); // jumps take labels
        assert_eq!(e.span, Span::new(2, 13, 2, 14)); // the `5`

        let e = lower_src(".func f\n        wr\n").unwrap_err();
        assert!(matches!(e.kind, AsmErrorKind::BadOperand(_)));
        assert_eq!(e.span, Span::new(2, 9, 2, 11)); // the `wr` word

        let e = lower_src(".func f\nL1:\n").unwrap_err();
        assert!(matches!(e.kind, AsmErrorKind::Syntax(_))); // dangling label
        assert_eq!(e.span, Span::new(2, 1, 2, 3)); // the `L1` label
    }

    #[test]
    fn func_local_modifier_parses() {
        let funcs = lower_src(".func f local\n        ret\n").unwrap();
        assert_eq!(funcs.len(), 1);
        assert_eq!(funcs[0].name, "f");
        assert!(funcs[0].local);
    }

    #[test]
    fn func_without_local_modifier_defaults_to_false() {
        let funcs = lower_src(".func f\n        ret\n").unwrap();
        assert_eq!(funcs.len(), 1);
        assert!(!funcs[0].local);
    }

    #[test]
    fn pending_label_before_a_malformed_func_reports_the_dangling_label_first() {
        // Legacy precedence: the pending-label check is the FIRST thing in
        // the `.func` branch, ahead of name/modifier parsing — so a bad
        // `.func` after a dangling label still reports the label, not the
        // malformed directive. Same KIND either way, but this keeps the
        // exact-`.func` and malformed-`.func` paths symmetric.
        let e = lower_src(".func f\nL1:\n.func g loco\n").unwrap_err();
        assert!(matches!(e.kind, AsmErrorKind::Syntax(_)));
        assert_eq!(e.span, Span::new(2, 1, 2, 3)); // the `L1` label
    }

    #[test]
    fn func_local_modifier_requires_exact_keyword() {
        let e = lower_src(".func f loco\n").unwrap_err();
        assert!(matches!(e.kind, AsmErrorKind::Syntax(_)));
        assert_eq!(e.span, Span::new(1, 1, 1, 6)); // the `.func` word

        let e = lower_src(".func f local extra\n").unwrap_err();
        assert!(matches!(e.kind, AsmErrorKind::Syntax(_)));
        assert_eq!(e.span, Span::new(1, 1, 1, 6));
    }

    #[test]
    fn dotted_function_names_accepted() {
        let funcs = lower_src(".func outer.inner local\n        ret\n").unwrap();
        assert_eq!(funcs[0].name, "outer.inner");
        assert!(funcs[0].local);
    }

    #[test]
    fn namespaced_function_names_accepted() {
        let funcs = lower_src(".func std::api.helper local\n        ret\n").unwrap();
        assert_eq!(funcs[0].name, "std::api.helper");
        assert!(funcs[0].local);
    }

    #[test]
    fn unicode_function_names_are_accepted() {
        // Legacy acceptance: `is_symbol_name` uses Unicode letter classes,
        // and the lexer now tokenizes non-ASCII identifiers as one Word.
        let funcs = lower_src(".func идиВКонец\n        ret\n").unwrap();
        assert_eq!(funcs[0].name, "идиВКонец");
        assert!(!funcs[0].local);
    }

    #[test]
    fn call_operands_accept_dotted_names() {
        let funcs = lower_src(".func f\n        call outer.inner\n").unwrap();
        assert_eq!(funcs[0].items.len(), 1);
        match &funcs[0].items[0] {
            SourceItem::Instr { operand, .. } => {
                assert!(matches!(operand, SourceOperand::Name(n) if n.name == "outer.inner"));
            }
            _ => panic!("expected Instr"),
        }
    }

    #[test]
    fn call_operands_accept_namespaced_names() {
        let funcs = lower_src(".func f\n        call std::api\n").unwrap();
        assert_eq!(funcs[0].items.len(), 1);
        match &funcs[0].items[0] {
            SourceItem::Instr { operand, .. } => {
                assert!(matches!(operand, SourceOperand::Name(n) if n.name == "std::api"));
            }
            _ => panic!("expected Instr"),
        }
    }

    #[test]
    fn label_with_namespace_colons_is_rejected() {
        // Sanctioned delta: legacy misparsed this as UnknownMnemonic(`:x:`);
        // the CST shapes `std::x` as a label candidate and lowering rejects
        // the bad label name with a precise span.
        let e = lower_src(".func f\nstd::x:  nop\n").unwrap_err();
        assert!(matches!(e.kind, AsmErrorKind::Syntax(_)));
        assert_eq!(e.span, Span::new(2, 1, 2, 7)); // `std::x`
    }

    #[test]
    fn labels_with_dots_are_rejected() {
        // Sanctioned delta: dotted label names are no longer accepted.
        let e = lower_src(".func f\nfoo.bar:  nop\n").unwrap_err();
        assert!(matches!(e.kind, AsmErrorKind::Syntax(_)));
        assert_eq!(e.span, Span::new(2, 1, 2, 8)); // `foo.bar`
    }

    #[test]
    fn unicode_labels_still_accepted() {
        // The label tightening is dots and `::` ONLY — letters keep the
        // legacy Unicode reading (`is_alphabetic`), consistent with
        // function names.
        let src = ".func f\nметка:  nop\n        jmp метка\n";
        let funcs = lower_src(src).unwrap();
        match &funcs[0].items[0] {
            SourceItem::Instr { labels, .. } => {
                assert_eq!(label_names(labels), vec!["метка"]);
            }
            other => panic!("unexpected {other:?}"),
        }
        // And the jump target resolves end-to-end through the assembler.
        crate::asm::assemble(&test_syntax(), 0x7E, src, false).unwrap();
    }

    #[test]
    fn raw_line_is_rejected_with_its_span() {
        // A disassembly-listing-shaped line is not assembly text.
        let e = lower_src("<goToEnd>\n").unwrap_err();
        assert_eq!(e.kind, AsmErrorKind::RawLine);
        assert_eq!(e.span, Span::new(1, 1, 1, 10));

        let listing = "  0004:  21 05 00 00 00  call    0x0005 <goToEnd>\n";
        let e = lower_src(listing).unwrap_err();
        assert_eq!(e.kind, AsmErrorKind::RawLine);
        assert_eq!(e.span.start.col, 3); // trimmed extent
    }

    // -- `.rept` macro expansion (docs/formats.md (assembly text)) -------

    #[test]
    fn rept_expands_a_plain_body_line_once_per_iteration() {
        // `.rept v, 0, 2` around a `nop` yields three inlined instructions.
        let src = ".func f\n.rept v, 0, 2\n        nop\n.endr\n";
        let funcs = lower_rept_src(src).unwrap();
        assert_eq!(funcs.len(), 1);
        assert_eq!(funcs[0].items.len(), 3);
        for item in &funcs[0].items {
            assert!(matches!(item, SourceItem::Instr { opcode: 0x01, .. }));
        }
    }

    #[test]
    fn rept_substitutes_the_loop_variable_into_labels() {
        // The re-lex/re-shape model is what makes the label survive: the
        // body item `L{v}: nop` never shapes as a label (the `{` breaks
        // the word), but substituting the physical line to `L0: nop` and
        // re-parsing detects the label — three DISTINCT labels result.
        let src = ".func f\n.rept v, 0, 2\nL{v}: nop\n.endr\n";
        let funcs = lower_rept_src(src).unwrap();
        let names: Vec<&str> = funcs[0]
            .items
            .iter()
            .map(|item| match item {
                SourceItem::Instr { labels, .. } => {
                    assert_eq!(labels.len(), 1);
                    labels[0].name.as_str()
                }
                other => panic!("unexpected {other:?}"),
            })
            .collect();
        assert_eq!(names, vec!["L0", "L1", "L2"]);
    }

    #[test]
    fn rept_with_empty_range_is_bad_rept() {
        // `lo > hi` describes no iterations — a `BadRept`, pointed at the
        // `.rept` header line.
        let src = ".func f\n.rept v, 2, 0\n        nop\n.endr\n";
        let e = lower_rept_src(src).unwrap_err();
        assert_eq!(e.kind, AsmErrorKind::BadRept);
        assert_eq!(e.span.start.line, 2); // the `.rept` header
    }

    #[test]
    fn rept_substitution_failure_carries_the_body_line_span() {
        // `{v+}` is a malformed expression; the error is a
        // `BadSubstitution` at the original body line, not at the
        // re-parsed single line.
        let src = ".func f\n.rept v, 0, 0\n        wr {v+}\n.endr\n";
        let e = lower_rept_src(src).unwrap_err();
        assert!(matches!(e.kind, AsmErrorKind::BadSubstitution(_)));
        assert_eq!(e.span.start.line, 3); // the `wr {v+}` body line
    }

    // -- Vector operands (caps.vectors) ----------------------------------

    /// `test_syntax()` with the vectors cap on, so `[..]` operand tokens
    /// exist for the classic `wr` (SymbolVec) mnemonic to classify.
    fn vectors_syntax() -> ArchSyntax {
        let mut syntax = test_syntax();
        syntax.caps = AsmCaps {
            vectors: true,
            ..Default::default()
        };
        syntax
    }

    fn lower_vectors_src(src: &str) -> Result<Vec<SourceFunction>, AsmError> {
        let syntax = vectors_syntax();
        lower(&parse_asm_cst_with(src, syntax.caps), &syntax, src)
    }

    #[test]
    fn vector_operands_parse_per_element() {
        // The full element vocabulary in one vector; legality per context
        // is the consumer's call — this layer only parses.
        let src = ".func f\n        wr [1, *, -, <, >, .]\n";
        let funcs = lower_vectors_src(src).unwrap();
        match &funcs[0].items[0] {
            SourceItem::Instr {
                operand: SourceOperand::Vector(elems, _),
                ..
            } => {
                assert_eq!(
                    elems,
                    &vec![
                        VecElem::Payload(1),
                        VecElem::Wildcard,
                        VecElem::Keep,
                        VecElem::MoveLeft,
                        VecElem::MoveRight,
                        VecElem::Stay,
                    ]
                );
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn bad_vector_elements_are_rejected() {
        let e = lower_vectors_src(".func f\n        wr [1, x]\n").unwrap_err();
        assert!(matches!(e.kind, AsmErrorKind::BadVector(_)), "{e}");

        let e = lower_vectors_src(".func f\n        wr []\n").unwrap_err();
        assert!(matches!(e.kind, AsmErrorKind::BadVector(_)), "{e}");

        let e = lower_vectors_src(".func f\n        wr [1,,2]\n").unwrap_err();
        assert!(matches!(e.kind, AsmErrorKind::BadVector(_)), "{e}");
    }

    #[test]
    fn write_move_vectors_classify_into_two_groups() {
        // `wrmv [w...], [m...]` arrives from the CST as ONE bracket token
        // (`[1, -], [<, .]`); classify splits it into the write group and
        // the move group, each parsed with the full element vocabulary.
        let funcs = lower_vectors_src(".func f\n        vwrmv [1, -], [<, .]\n").unwrap();
        match &funcs[0].items[0] {
            SourceItem::Instr {
                operand: SourceOperand::WriteMoveVectors(writes, moves, _),
                ..
            } => {
                assert_eq!(writes, &vec![VecElem::Payload(1), VecElem::Keep]);
                assert_eq!(moves, &vec![VecElem::MoveLeft, VecElem::Stay]);
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn write_move_bad_group_shapes_are_rejected() {
        // One bracket group only — the move vector is missing.
        let e = lower_vectors_src(".func f\n        vwrmv [1, -]\n").unwrap_err();
        assert!(matches!(e.kind, AsmErrorKind::BadVector(_)), "{e}");
        // Three groups — an extra top-level comma.
        let e = lower_vectors_src(".func f\n        vwrmv [1], [<], [>]\n").unwrap_err();
        assert!(matches!(e.kind, AsmErrorKind::BadVector(_)), "{e}");
        // No brackets at all — the region comma-splits into two plain
        // operands, not one bracket token.
        let e = lower_vectors_src(".func f\n        vwrmv 1, 2\n").unwrap_err();
        assert!(matches!(e.kind, AsmErrorKind::BadOperand(_)), "{e}");
    }

    #[test]
    fn plain_int_operands_still_classify_as_ints_under_vector_caps() {
        // The vectors cap must not disturb the classic spelled-out form.
        let funcs = lower_vectors_src(".func f\n        wr 1, 2\n").unwrap();
        match &funcs[0].items[0] {
            SourceItem::Instr { operand, .. } => {
                assert!(matches!(operand, SourceOperand::Ints(v) if v == &vec![1, 2]));
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn rept_lowering_error_is_remapped_to_the_body_line_span() {
        // A body line that substitutes cleanly but lowers to an error
        // (unknown mnemonic) reports at the original body line, not the
        // re-parsed line 1.
        let src = ".func f\n.rept v, 0, 0\n        bogus{v}\n.endr\n";
        let e = lower_rept_src(src).unwrap_err();
        assert!(matches!(e.kind, AsmErrorKind::UnknownMnemonic(ref m) if m == "bogus0"));
        assert_eq!(e.span.start.line, 3);
    }
}
