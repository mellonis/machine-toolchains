//! Lossless assembly CST → per-function source items (docs/formats.md
//! (assembly text) grammar). Shaping is total and lives in `cst.rs`;
//! this pass validates + classifies, attaching a precise [`Span`] to
//! every diagnostic. Replaces the old line-oriented parser.

use super::cst::{
    AsmCst, AsmItem, AsmItemKind, BYTE_WORD, BindingShapeError, DigestDirectiveCst,
    FRAME_DIRECTIVE_WORDS, FUNC_WORD, FrameDirectiveCst, FrameHeaderCst, FrameMapCst, FramePairCst,
    FuncCst, GRAFTED_WORD, GRAPH_WORD, INTERFACE_DIRECTIVE_WORDS, InstrCst, LabelCst, LineCst,
    OperandToken, PARAM_WORD, PairDst, ParamDirectiveCst, ROUTINE_WORD, ROW_WORD, ReptCst,
    RoutineDirectiveCst, SectionCst, TableDirectiveCst, TableDirectiveKind, VOLATILE_WORD,
    VolatileCst, exit_vector_interior, parse_asm_cst_with, parse_binding,
};
use super::subst::substitute;
use super::syntax::{ArchSyntax, AsmCaps, Flow, SyntaxEntry};
use super::{AsmError, AsmErrorKind};
use crate::diagnostics::Span;
use crate::formats::object::{
    ExportedGraph, GraftProvenance, Interface, RoutineInterface, RoutineSig,
};
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
    /// This block carries a `.volatile` directive, so its blob belongs to
    /// the gated build column (docs/core.md (the assembler framework)).
    /// Absence is the normal column; a name may be defined once per column.
    pub volatile: bool,
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
    /// call `target` (a symbol name, like a plain call's), the tape
    /// binding — one entry per callee virtual tape, in list order unless
    /// the entries name their parameters — and the `exits=(…)` vector,
    /// the local labels the callee's declared exits return to (empty when
    /// the operand is absent). The assembler emits a plain far-call
    /// opcode with a zeroed hole (no relocation) and records the binding
    /// as an MO bound-call for the composition engine to lower
    /// (docs/formats.md (bound calls)).
    BoundCallOp {
        target: SpannedName,
        binding: Vec<SourceTapeBinding>,
        exits: Vec<SpannedName>,
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
    /// The callee parameter this entry binds (`num: 1`); `None` is the
    /// positional form, where the entry's position is the callee tape. A
    /// binding names every entry or none.
    pub param: Option<String>,
    /// The map was written out — `1{}` (the empty map) versus `1` (index
    /// identity).
    pub map_written: bool,
    /// The map ends in `*`: the listed pairs are not the whole map and
    /// the linker completes the rest. Implies `map_written`.
    pub open: bool,
    /// `(src, dst, one_way)` per authored pair, in source order.
    pub pairs: Vec<(u32, SourceDst, bool)>,
}

/// A binding pair's destination as authored: a symbol index, or a glyph
/// label naming a symbol in the callee's alphabet (docs/formats.md (bound
/// calls)). Resolving a label against the callee's declared glyphs is the
/// linker's, at composition time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceDst {
    Index(u32),
    Label(String),
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
    /// The interface section (docs/formats.md (routine interfaces)):
    /// `Some` iff the file declares any `.param` line or any `.graph`
    /// digest, and then `routines` parallels `functions` exactly as
    /// `signatures` does — the wire section repeats one routine record
    /// per signature, so an interface obliges every function to carry
    /// both a `.routine` and its `.param` lines.
    ///
    /// `alphabets` is always empty here: an exported alphabet is a
    /// compiler fact (a source-language declaration), not something a
    /// hand-written assembly file states, so the assembler authors none
    /// and the compiler fills them in after assembly.
    #[allow(dead_code)] // the assembler reads it once interface emission lands
    pub interface: Option<Interface>,
    /// The library graphs this unit spliced, from its `.grafted`
    /// directives — an object-level list of its own, outside the
    /// interface section (docs/formats.md (routine interfaces)).
    #[allow(dead_code)] // the assembler reads it once interface emission lands
    pub grafts: Vec<GraftProvenance>,
    /// The file declares a `.volatile` ahead of its first `.func`: this
    /// source builds a volatile program (docs/core.md (linking)).
    /// Independent of any per-function tag — it is a whole-object header
    /// bit, not a blob record.
    pub program_volatile: bool,
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

/// [`spanned`], but with the span replaced by `override_span` when set —
/// the enclosing `.rept` header, so an expanded label carries a usable
/// source anchor instead of the line-1 span of its one-line re-parse.
/// Reduces to `spanned` (byte-for-byte) when the override is `None`,
/// which is always the case outside a `.rept` expansion.
fn spanned_in(label: &LabelCst, override_span: Option<Span>) -> SpannedName {
    SpannedName {
        name: label.name.clone(),
        span: override_span.unwrap_or(label.span),
    }
}

/// The functions-only view of [`lower_source`], dropping the tables. All
/// validation still runs — only the successfully lowered tables are
/// discarded. A convenience for the asm-lint rule unit tests whose
/// fixtures exercise no table section (they build the context with an
/// empty `tables` slice); production lint calls [`lower_source`] so a
/// rule can read the table-carried label references. Test-only: no
/// non-test caller remains, so it is gated to keep the release build
/// free of dead code.
#[cfg(test)]
pub(crate) fn lower(
    cst: &AsmCst,
    syntax: &ArchSyntax,
    source: &str,
) -> Result<Vec<SourceFunction>, AsmError> {
    lower_source(cst, syntax, source).map(|lowered| lowered.functions)
}

/// The interface fields a `.routine` gathers while it waits for its
/// `.func` (docs/formats.md (routine interfaces)): the directive's own
/// tail, then one record per `.param` line in tape order.
#[derive(Debug)]
struct PendingInterface {
    exits: u8,
    /// A `noreturn` routine sets this false.
    returns: bool,
    /// The tail's span, for the diagnostic that reports `exits=`/
    /// `noreturn` on a routine with no `.param` lines under it.
    tail_span: Option<Span>,
    params: Vec<PendingParam>,
}

/// One `.param` line, validated against its tape's cardinality and
/// alphabet at the moment it was read.
#[derive(Debug)]
struct PendingParam {
    name: String,
    glyphs: Vec<String>,
    writes: Vec<String>,
    enters: Option<Vec<String>>,
    leaves: Option<Vec<String>>,
    opaque: bool,
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
    pending_sigs: Vec<(SpannedName, RoutineSig, PendingInterface)>,
    /// Per-function signature slots, parallel to `functions`.
    func_sigs: Vec<Option<RoutineSig>>,
    /// Per-function interface slots, parallel to `functions`. `Some`
    /// exactly when the function's `.routine` was followed by `.param`
    /// lines (docs/formats.md (routine interfaces)).
    func_ifaces: Vec<Option<RoutineInterface>>,
    /// `.graph` digests, in source order — the interface section's
    /// exported-graph list.
    graphs: Vec<ExportedGraph>,
    /// `.grafted` digests, in source order.
    grafts: Vec<GraftProvenance>,
    /// While expanding a `.rept` block, the header's span. Each body line
    /// is recovered, substituted, and re-parsed as a standalone one-line
    /// source, so its labels come back carrying line-1 spans of that
    /// throwaway parse — useless to point a diagnostic at. Stamping the
    /// block header's span onto those labels gives a finding on an
    /// expanded label (e.g. `unused-label` on a never-referenced
    /// `Linc{v}`) a usable anchor. `None` outside a `.rept` expansion, so
    /// every non-rept label keeps its own span and cap-off dialects (no
    /// `.rept`) are unaffected.
    span_override: Option<Span>,
    /// A `.volatile` seen ahead of the first `.func` — the object's
    /// program bit (docs/core.md (linking)).
    program_volatile: bool,
    /// The open function was tagged by lookahead and its `.volatile` line
    /// has not been consumed yet. The directive is legal exactly where
    /// this is set — the item directly after its own `.func`, own-line
    /// comments being trivia that do not close the slot.
    volatile_pending: bool,
    /// One-item lookahead: the next non-comment item after the one being
    /// lowered is a `.volatile`. A `.func` reads it to learn its own build
    /// column at the moment it is defined, which keeps the duplicate check
    /// exactly where it always fired rather than deferring it to a
    /// post-pass that would reorder diagnostics.
    next_is_volatile: bool,
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
        func_ifaces: Vec::new(),
        graphs: Vec::new(),
        grafts: Vec::new(),
        span_override: None,
        program_volatile: false,
        volatile_pending: false,
        next_is_volatile: false,
    };

    for (i, item) in cst.items.iter().enumerate() {
        ctx.next_is_volatile = leads_with_volatile(&cst.items[i + 1..]);
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
    if let Some((name, _, _)) = ctx.pending_sigs.first() {
        return Err(err(
            name.span,
            AsmErrorKind::BadSignature(format!(
                "`.routine` precedes no `.func` named `{}`",
                name.name
            )),
        ));
    }
    // The interface section parallels the signatures on the wire, which
    // parallel the blobs (docs/formats.md (routine interfaces)): once a
    // file declares any interface content — a `.param` line, a `.graph`
    // or a `.grafted` digest — every function owes both a `.routine` and
    // its `.param` lines, or the object could not be written at all.
    let declares_params = ctx.func_ifaces.iter().any(Option::is_some);
    let declares_digests = !ctx.graphs.is_empty() || !ctx.grafts.is_empty();
    let declares_interface = declares_params || declares_digests;
    // When a digest line is the ONLY reason a function owes an interface,
    // the diagnostic says so: the file it points at otherwise looks
    // perfectly legal, and nothing on the `.func` line hints at what
    // obliged it.
    const DIGEST_CAUSE: &str =
        " — a `.graph`/`.grafted` line obliges an interface for every function";
    let signs_any = ctx.func_sigs.iter().any(Option::is_some);
    // All or none: the MO signature section parallels the blobs
    // (docs/formats.md (MO)), so a file that signs any function must
    // sign every function.
    let signatures = if declares_interface || signs_any {
        let cause = if signs_any || declares_params {
            ""
        } else {
            DIGEST_CAUSE
        };
        let mut sigs = Vec::with_capacity(ctx.func_sigs.len());
        for (function, sig) in ctx.functions.iter().zip(ctx.func_sigs) {
            match sig {
                Some(sig) => sigs.push(sig),
                None => {
                    return Err(err(
                        function.name_span,
                        AsmErrorKind::BadSignature(format!(
                            "function `{}` lacks a `.routine` signature{cause}",
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
    // The same rule for the interface: every function carries one, or
    // none does. `.param` lines can only follow a `.routine`, so the
    // signature loop above has already answered an unsigned function.
    let mut routines = Vec::with_capacity(ctx.func_ifaces.len());
    if declares_interface {
        let cause = if declares_params { "" } else { DIGEST_CAUSE };
        for (function, iface) in ctx.functions.iter().zip(ctx.func_ifaces) {
            match iface {
                Some(iface) => routines.push(iface),
                None => {
                    return Err(err(
                        function.name_span,
                        AsmErrorKind::BadSignature(format!(
                            "function `{}` lacks `.param` lines{cause}",
                            function.name
                        )),
                    ));
                }
            }
        }
    }
    let interface = (!routines.is_empty() || !ctx.graphs.is_empty()).then(|| Interface {
        routines,
        // An exported alphabet has no assembly spelling — it is a
        // source-language declaration the compiler fills in later.
        alphabets: Vec::new(),
        graphs: ctx.graphs,
    });
    Ok(LoweredSource {
        functions: ctx.functions,
        tables: ctx.tables,
        signatures,
        interface,
        grafts: ctx.grafts,
        program_volatile: ctx.program_volatile,
    })
}

/// Does this run of items open with a `.volatile` directive? Own-line
/// comments are trivia and are skipped, so a comment may sit between a
/// `.func` and the directive that tags it.
fn leads_with_volatile(rest: &[AsmItem]) -> bool {
    rest.iter()
        .find(|item| !matches!(item.kind, AsmItemKind::Comment(_)))
        .is_some_and(|item| matches!(item.kind, AsmItemKind::Volatile(_)))
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
        // The interface directives shape only under `caps.interface`.
        AsmItemKind::ParamDirective(p) => lower_param_directive(p, ctx)?,
        AsmItemKind::DigestDirective(d) => lower_digest_directive(d, ctx)?,
        AsmItemKind::FrameDirective(d) => lower_frame_directive(d, ctx)?,
        AsmItemKind::Volatile(v) => lower_volatile(v, ctx)?,
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
    let exits = match d.exits {
        Some((exits, span)) => match u8::try_from(exits) {
            Ok(exits) => exits,
            Err(_) => {
                return Err(err(
                    span,
                    AsmErrorKind::BadSignature("exits must be 0..=255".to_string()),
                ));
            }
        },
        None => 0,
    };
    let already_pending = ctx.pending_sigs.iter().any(|(n, _, _)| n.name == d.name);
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
        PendingInterface {
            exits,
            returns: d.noreturn.is_none(),
            tail_span: d.exits.map(|(_, span)| span).or(d.noreturn),
            params: Vec::new(),
        },
    ));
    Ok(())
}

/// `.param <name>, (<glyphs>)[, writes=(…)][, enters=(…)][, leaves=(…)]
/// [, opaque]`: one tape of the most recent pending `.routine`'s
/// interface, in tape order (docs/formats.md (routine interfaces)).
/// Rules: code section only; a `.param` line follows the `.routine` it
/// describes; its glyph count equals that tape's declared cardinality;
/// every `writes`/`enters`/`leaves` glyph is one of the tape's own; a
/// written `enters=()`/`leaves=()` is rejected — a clause lists at least
/// one glyph, and no clause at all is the way to say nothing.
fn lower_param_directive(p: &ParamDirectiveCst, ctx: &mut LowerCtx) -> Result<(), AsmError> {
    // A pending label cannot bind across a directive (same rule as the
    // `.func`, `.section` and `.routine` boundaries).
    if let Some(first) = ctx.pending.first() {
        return Err(err(
            first.span,
            AsmErrorKind::Syntax("label at end of function"),
        ));
    }
    if ctx.section == Section::Tables {
        return Err(err(
            p.span,
            AsmErrorKind::BadTable("only table directives are allowed in the tables section"),
        ));
    }
    // The tape this line describes is the next one the pending routine
    // has not named yet.
    let Some((_, sig, iface)) = ctx.pending_sigs.last() else {
        return Err(err(
            p.span,
            AsmErrorKind::BadSignature("`.param` precedes no `.routine`".to_string()),
        ));
    };
    // A parameter is named again at every call site that binds by name
    // (`call g [num: 1]`), so its name takes the label grammar — no dots,
    // no `::` — and must be unique within its routine, or a named entry
    // would not say which tape it means.
    if !is_label_name(&p.name) {
        return Err(err(
            p.name_span,
            AsmErrorKind::BadSignature(
                "`.param` names use letters, digits, underscore".to_string(),
            ),
        ));
    }
    if iface.params.iter().any(|prev| prev.name == p.name) {
        return Err(err(
            p.name_span,
            AsmErrorKind::BadSignature(format!("`.param {}` is declared twice", p.name)),
        ));
    }
    let k = iface.params.len();
    let Some(&cardinality) = sig.cardinalities.get(k) else {
        return Err(err(
            p.span,
            AsmErrorKind::BadSignature(format!("{} .param line(s) for tapes={}", k + 1, sig.arity)),
        ));
    };
    if p.glyphs.len() as u64 != u64::from(cardinality) {
        return Err(err(
            p.glyphs_span,
            AsmErrorKind::BadSignature(format!(
                "`.param {}` lists {} glyphs for a cardinality of {}",
                p.name,
                p.glyphs.len(),
                cardinality
            )),
        ));
    }
    // Every named glyph is one of this tape's own: the object's reader
    // enforces the same membership, so a violation here would only
    // surface as an unreadable object later.
    let subset_of_alphabet = |list: &[String], clause: &str, span: Span| {
        for glyph in list {
            if !p.glyphs.contains(glyph) {
                return Err(err(
                    span,
                    AsmErrorKind::BadSignature(format!(
                        "`.param {}` {clause} names `{glyph}`, which is not in its alphabet",
                        p.name
                    )),
                ));
            }
        }
        Ok(())
    };
    let writes = p.writes.clone().unwrap_or_default();
    if let Some(span) = p.writes_span {
        subset_of_alphabet(&writes, "writes", span)?;
    }
    let clause = |list: &Option<Vec<String>>, span: Option<Span>, name: &str| {
        let (Some(list), Some(span)) = (list, span) else {
            return Ok(None);
        };
        if list.is_empty() {
            return Err(err(
                span,
                AsmErrorKind::BadSignature(
                    "`enters=`/`leaves=` list at least one glyph; omit the suffix for no clause"
                        .to_string(),
                ),
            ));
        }
        subset_of_alphabet(list, name, span)?;
        Ok(Some(list.clone()))
    };
    let enters = clause(&p.enters, p.enters_span, "enters")?;
    let leaves = clause(&p.leaves, p.leaves_span, "leaves")?;
    ctx.pending_sigs
        .last_mut()
        .expect("the pending routine was read just above")
        .2
        .params
        .push(PendingParam {
            name: p.name.clone(),
            glyphs: p.glyphs.clone(),
            writes,
            enters,
            leaves,
            opaque: p.opaque.is_some(),
        });
    Ok(())
}

/// `.graph <name>, <digest>` / `.grafted <name>, <digest>`: the
/// object-level graph digests (docs/formats.md (routine interfaces)).
/// Both precede the first `.func` — they describe the unit, not a
/// function — and neither belongs in the tables section.
fn lower_digest_directive(d: &DigestDirectiveCst, ctx: &mut LowerCtx) -> Result<(), AsmError> {
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
    if !ctx.functions.is_empty() {
        return Err(err(
            d.span,
            AsmErrorKind::Syntax("`.graph`/`.grafted` precede the first `.func`"),
        ));
    }
    if d.grafted {
        ctx.grafts.push(GraftProvenance {
            graph: d.name.clone(),
            digest: d.digest,
        });
    } else {
        ctx.graphs.push(ExportedGraph {
            name: d.name.clone(),
            digest: d.digest,
        });
    }
    Ok(())
}

/// Detaches the pending `.routine` signature — and the interface its
/// `.param` lines built — for a function being defined, if one was
/// declared. Called by BOTH `.func` lowering paths so the parallel
/// `func_sigs`/`func_ifaces` vectors never fall out of step.
///
/// This is where an interface is finally checked against its signature:
/// the `.param` lines must cover every tape, and a routine that declares
/// `exits=`/`noreturn` must describe its tapes at all — the wire record
/// carries those fields inside the interface, with nowhere else to live.
///
/// That refusal is deliberately asymmetric: a written `exits=0` on an
/// interface-less routine is accepted and dropped, while `exits=1` is an
/// error. Zero IS the field's default, so the object the assembler would
/// write is the same either way, and nothing is lost — the text itself
/// survives in the CST, which is what `fmt` reprints.
fn take_pending(
    ctx: &mut LowerCtx,
    name: &str,
) -> Result<(Option<RoutineSig>, Option<RoutineInterface>), AsmError> {
    let Some(i) = ctx.pending_sigs.iter().position(|(n, _, _)| n.name == name) else {
        return Ok((None, None));
    };
    let (declared, sig, pending) = ctx.pending_sigs.remove(i);
    if pending.params.is_empty() {
        if pending.exits != 0 || !pending.returns {
            return Err(err(
                pending.tail_span.unwrap_or(declared.span),
                AsmErrorKind::BadSignature("`exits=`/`noreturn` need `.param` lines".to_string()),
            ));
        }
        return Ok((Some(sig), None));
    }
    if pending.params.len() != sig.arity as usize {
        return Err(err(
            declared.span,
            AsmErrorKind::BadSignature(format!(
                "{} .param line(s) for tapes={}",
                pending.params.len(),
                sig.arity
            )),
        ));
    }
    let mut iface = RoutineInterface {
        params: Vec::with_capacity(pending.params.len()),
        glyphs: Vec::with_capacity(pending.params.len()),
        writes: Vec::with_capacity(pending.params.len()),
        enters: Vec::with_capacity(pending.params.len()),
        leaves: Vec::with_capacity(pending.params.len()),
        opaque: Vec::with_capacity(pending.params.len()),
        exits: pending.exits,
        returns: pending.returns,
    };
    for param in pending.params {
        iface.params.push(param.name);
        iface.glyphs.push(param.glyphs);
        iface.writes.push(param.writes);
        iface.enters.push(param.enters);
        iface.leaves.push(param.leaves);
        iface.opaque.push(param.opaque);
    }
    Ok((Some(sig), Some(iface)))
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
    // Anchor every label an expanded body line produces at the block
    // header — its re-parsed line-1 span is a throwaway-parse artifact,
    // not a place a diagnostic can point (module doc on `span_override`).
    // Saved and restored (rather than cleared) so a hypothetical nested
    // expansion would restore the outer header, never leak `None`.
    let saved_override = ctx.span_override.replace(rept.span);
    let result = lower_rept_body(rept, syntax, source, ctx);
    ctx.span_override = saved_override;
    result
}

fn lower_rept_body(
    rept: &ReptCst,
    syntax: &ArchSyntax,
    source: &str,
    ctx: &mut LowerCtx,
) -> Result<(), AsmError> {
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
                // The enclosing item's lookahead says nothing about a
                // one-line re-parse, and a body line is expanded alone —
                // nothing can follow it here — so no expanded `.func`
                // is ever tagged.
                ctx.next_is_volatile = false;
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
        AsmItemKind::ParamDirective(p) => Some(p.span),
        AsmItemKind::DigestDirective(d) => Some(d.span),
        AsmItemKind::FrameDirective(d) => Some(d.span()),
        AsmItemKind::Volatile(v) => Some(v.span),
    }
}

/// `.volatile` (docs/formats.md (assembly text)) in its two placements:
/// ahead of the first `.func` it sets the object's program bit; directly
/// after a `.func` it tags that block's build column — and there the tag
/// was already applied by the lookahead in [`lower_source`], so this arm
/// only consumes the slot the `.func` opened. Anywhere else — after code,
/// after a pending label, or a second time in one block — the slot is
/// closed and the placement is an error.
fn lower_volatile(v: &VolatileCst, ctx: &mut LowerCtx) -> Result<(), AsmError> {
    if ctx.functions.is_empty() {
        if ctx.program_volatile {
            return Err(err(v.span, AsmErrorKind::Syntax("duplicate `.volatile`")));
        }
        ctx.program_volatile = true;
        return Ok(());
    }
    if !std::mem::take(&mut ctx.volatile_pending) {
        return Err(err(
            v.span,
            AsmErrorKind::Syntax("`.volatile` must directly follow its `.func`"),
        ));
    }
    Ok(())
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
    open_function(
        func.name.clone(),
        func.name_span,
        func.local,
        std::mem::take(&mut ctx.next_is_volatile),
        ctx,
    )
}

/// Records one `.func` after its name has been validated, applying the
/// variant-aware duplicate rule (docs/formats.md (assembly text)): a name
/// may be defined once per build column, so a bare/`.volatile` pair is the
/// only same-name pair a file may carry, and the two members must agree on
/// visibility (the linker's namespace pairs a name's columns, and it only
/// pairs exported ones — a `Local`/`Defined` mix would half-vanish there).
fn open_function(
    name: String,
    name_span: Span,
    local: bool,
    volatile: bool,
    ctx: &mut LowerCtx,
) -> Result<(), AsmError> {
    let mut twin = None;
    for prior in ctx.functions.iter().filter(|f| f.name == name) {
        if prior.volatile == volatile {
            return Err(err(name_span, AsmErrorKind::DuplicateFunction(name)));
        }
        twin = Some(prior);
    }
    if let Some(twin) = twin
        && twin.local != local
    {
        return Err(err(
            name_span,
            AsmErrorKind::Syntax("a `.volatile` twin must match its function's visibility"),
        ));
    }
    let (sig, iface) = take_pending(ctx, &name)?;
    ctx.functions.push(SourceFunction {
        name,
        name_span,
        local,
        volatile,
        items: Vec::new(),
    });
    ctx.func_sigs.push(sig);
    ctx.func_ifaces.push(iface);
    // The directive that tagged this block is still ahead of the cursor;
    // its own arm consumes the slot.
    ctx.volatile_pending = volatile;
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
        && FRAME_DIRECTIVE_WORDS.contains(&instr.word.as_str())
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
            && instr.word == ROW_WORD
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
        let override_span = ctx.span_override;
        ctx.pending
            .extend(line.labels.iter().map(|l| spanned_in(l, override_span)));
        return Ok(());
    };

    // A malformed `.func` directive — the CST keeps it a Line with word
    // ".func" when the directive is not structurally exact. Only when
    // ".func" is the instruction word with no labels before it;
    // `L1: .func …` is a plain unknown mnemonic. This fires before the
    // open-function check, matching the legacy `.func`-branch precedence.
    if instr.word == FUNC_WORD && line.labels.is_empty() {
        return lower_malformed_func(instr, ctx);
    }

    // A malformed `.routine` — the CST keeps it a Line when the
    // directive is not structurally exact — gets a precise complaint
    // instead of UnknownMnemonic. Only for dialects whose caps could
    // shape one at all: with tables off the word is as unknown as any
    // other, exactly as before the directive existed.
    if instr.word == ROUTINE_WORD && line.labels.is_empty() && syntax.caps.tables {
        return Err(err(
            instr.word_span,
            AsmErrorKind::Syntax("`.routine` takes `<name>, tapes=<int>, alpha=(<int>, …)`"),
        ));
    }

    // A malformed interface directive — the CST keeps it a Line when the
    // directive is not structurally exact — gets its own grammar back
    // instead of UnknownMnemonic, for dialects whose caps could shape one
    // (mirror `.routine`).
    if let Some(instr) = &line.instr
        && INTERFACE_DIRECTIVE_WORDS.contains(&instr.word.as_str())
        && line.labels.is_empty()
        && syntax.caps.interface
    {
        let grammar = match instr.word.as_str() {
            PARAM_WORD => {
                "`.param` takes `<name>, (<glyphs>)` and the \
                 `writes=`/`enters=`/`leaves=`/`opaque` suffixes in that order"
            }
            GRAPH_WORD => "`.graph` takes `<name>, <digest>`",
            GRAFTED_WORD => "`.grafted` takes `<name>, <digest>`",
            _ => unreachable!("the guard matched an interface directive word"),
        };
        return Err(err(instr.word_span, AsmErrorKind::Syntax(grammar)));
    }

    // A malformed `.volatile` — the CST keeps it a Line when the bare
    // word carries anything at all — gets its own complaint instead of
    // UnknownMnemonic, for dialects whose caps could shape one. Labeled
    // (`L1: .volatile`) it is not a directive at all, exactly as with
    // `.func`, and falls through to mnemonic lookup.
    if instr.word == VOLATILE_WORD && line.labels.is_empty() && syntax.caps.volatile {
        return Err(err(
            instr.word_span,
            AsmErrorKind::Syntax("`.volatile` takes no operands"),
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
    let override_span = ctx.span_override;
    labels.extend(line.labels.iter().map(|l| spanned_in(l, override_span)));

    let item = if instr.word == BYTE_WORD {
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
            operand: classify_operand(entry, instr, syntax.caps)?,
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
    open_function(
        name.to_string(),
        word_span,
        local,
        std::mem::take(&mut ctx.next_is_volatile),
        ctx,
    )
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

fn classify_operand(
    entry: &SyntaxEntry,
    instr: &InstrCst,
    caps: AsmCaps,
) -> Result<SourceOperand, AsmError> {
    // An `exits=(…)` operand rides a binding call and nothing else: it
    // names where the callee's declared exits return to (docs/formats.md
    // (bound calls)). Split it off before the per-kind classification, so
    // every other mnemonic answers with the same complaint rather than
    // its own operand-shape one.
    let (operands, exits) = match caps.interface {
        true => split_exits_operand(&instr.operands),
        false => (instr.operands.as_slice(), None),
    };
    if let Some(exits) = exits
        && !(entry.flow == Flow::Call
            && matches!(entry.operand, OperandKind::RelI8 | OperandKind::RelI32))
    {
        return Err(err(
            exits.span,
            AsmErrorKind::BadOperand("only a call takes an exit vector"),
        ));
    }
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
            if let [target, bracket] = operands
                && bracket.text.starts_with('[')
            {
                return classify_bound_call(entry, target, bracket, exits, caps);
            }
            if let Some(exits) = exits {
                return Err(err(
                    exits.span,
                    AsmErrorKind::BadOperand(
                        "an exit vector needs a binding; write `[…]` (empty is allowed) before it",
                    ),
                ));
            }
            let [one] = operands else {
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
            if let [one] = operands
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
            if let [one] = operands
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
            let [one] = operands else {
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
            let [one] = operands else {
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
            let [one] = operands else {
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
            let [target, frame] = operands else {
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

/// Splits a trailing `exits=(…)` operand off the operand list. The CST
/// captures the run as one token under `caps.interface`, so this is a
/// shape test on that token's text, never a second grammar.
fn split_exits_operand(operands: &[OperandToken]) -> (&[OperandToken], Option<&OperandToken>) {
    match operands.split_last() {
        Some((last, head)) if exit_vector_interior(&last.text).is_some() => (head, Some(last)),
        _ => (operands, None),
    }
}

/// The labels of an `exits=(…)` operand, in source order, each spanned at
/// its own text (docs/formats.md (bound calls)). Exit targets are local
/// labels, like a frame descriptor's `.exits` list.
fn parse_exit_vector(operand: &OperandToken) -> Result<Vec<SpannedName>, AsmError> {
    let interior = exit_vector_interior(&operand.text).expect("the caller matched the shape");
    if interior.trim().is_empty() {
        return Err(err(
            operand.span,
            AsmErrorKind::BadOperand("an exit vector names at least one label"),
        ));
    }
    // The operand's text is a verbatim single-line slice starting at its
    // own span, so a char offset into it IS a column offset.
    let at = operand.text.chars().count() - interior.chars().count() - 1;
    let base = operand.span.start;
    let mut names = Vec::new();
    let mut col = u32::try_from(at).expect("an operand is one line long") + base.col;
    for part in interior.split(',') {
        let lead = part.chars().take_while(|c| c.is_whitespace()).count();
        let name = part.trim();
        let start = col + u32::try_from(lead).expect("an operand is one line long");
        let span = Span::new(
            base.line,
            start,
            base.line,
            start + u32::try_from(name.chars().count()).expect("an operand is one line long"),
        );
        if !is_label_name(name) {
            return Err(err(
                span,
                AsmErrorKind::BadOperand("exit targets are label names"),
            ));
        }
        names.push(SpannedName {
            name: name.to_string(),
            span,
        });
        col += u32::try_from(part.chars().count() + 1).expect("an operand is one line long");
    }
    Ok(names)
}

/// Classifies a declarative binding call (`call <name> [<binding>]
/// [exits=(…)]`). The `target` is a plain call target, `bracket` the
/// verbatim `[..]` operand and `exits` the optional exit vector. Only a
/// `Flow::Call` mnemonic takes a binding; jumps/branches with a trailing
/// bracket are rejected. Structural validation lives here (physical index
/// `< 16`, canonical `u32` sources, no duplicate source in one entry,
/// named-or-positional but never both, the cap behind the symbolic
/// forms); mapping legality — the blank↔blank rule, bijection, write-back
/// consistency, resolving a glyph label against the callee's alphabet —
/// is the composition engine's, checked at link time (docs/formats.md
/// (bound calls)).
fn classify_bound_call(
    entry: &SyntaxEntry,
    target: &OperandToken,
    bracket: &OperandToken,
    exits: Option<&OperandToken>,
    caps: AsmCaps,
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
    let entries = parse_binding(inner, bracket.span.start.line, caps).map_err(|e| {
        let message = match e {
            BindingShapeError::StarNotLast => "`*` closes a map: write it last, once",
            BindingShapeError::Malformed => "malformed tape binding",
        };
        err(bracket.span, AsmErrorKind::BadFrame(message.into()))
    })?;
    // An empty binding is the zero-tape call an exit vector may ride on,
    // and only the interface tier can write one: without the cap the
    // form stays the error it has always been.
    if entries.is_empty() && !caps.interface {
        return Err(err(
            bracket.span,
            AsmErrorKind::BadFrame(
                "a binding call needs at least one tape entry; use a plain `call` for none".into(),
            ),
        ));
    }
    // Named and positional entries answer two different questions — which
    // parameter, versus which position — and a half-named binding answers
    // neither for the entries it omits.
    let named = entries.iter().filter(|e| e.param.is_some()).count();
    if named != 0 && named != entries.len() {
        return Err(err(
            bracket.span,
            AsmErrorKind::BadFrame("a binding names every entry or none".into()),
        ));
    }
    let mut binding = Vec::with_capacity(entries.len());
    for e in entries {
        // Defense in depth for the named form and the open marker: a
        // glyph label cannot lex without the cap, but a parameter name
        // and the `*` marker can (the `rept` cap already emits `Star`
        // inside braces), so a capless dialect must refuse them here.
        if !caps.interface && (e.param.is_some() || e.open) {
            return Err(err(
                bracket.span,
                AsmErrorKind::BadFrame(
                    "named entries and open maps need the interface capability".into(),
                ),
            ));
        }
        let caller_tape = u8::try_from(e.phys)
            .ok()
            .filter(|&p| p < 16)
            .ok_or_else(|| {
                err(
                    bracket.span,
                    AsmErrorKind::BadFrame("binding physical tape index must be < 16".into()),
                )
            })?;
        // A source symbol may bind at most once per tape — a repeated src
        // is an ambiguous map, rejected regardless of composition rules.
        let mut seen = Vec::with_capacity(e.pairs.len());
        for p in &e.pairs {
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
            param: e.param,
            map_written: e.map_written,
            open: e.open,
            pairs: e
                .pairs
                .into_iter()
                .map(|p| {
                    let dst = match p.to {
                        PairDst::Index(n) => SourceDst::Index(n),
                        PairDst::Label(g) => SourceDst::Label(g),
                    };
                    (p.from, dst, p.one_way)
                })
                .collect(),
        });
    }
    Ok(SourceOperand::BoundCallOp {
        target: SpannedName {
            name: target.text.clone(),
            span: target.span,
        },
        binding,
        exits: exits
            .map(parse_exit_vector)
            .transpose()?
            .unwrap_or_default(),
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

    // -- The interface surface (caps.interface) ------------------------

    /// The interface tier's caps. `tables` carries `.routine` and `=`,
    /// `rept` the `(..)` groups every glyph list is written in — the same
    /// pairing `.routine`'s alpha list already needs.
    fn iface_caps() -> AsmCaps {
        AsmCaps {
            tables: true,
            rept: true,
            interface: true,
            ..AsmCaps::default()
        }
    }

    /// Lower to the whole [`LoweredSource`] under `caps`, parsing the CST
    /// with the same caps so the directives are shaped before lowering
    /// reads them.
    fn lower_with(caps: AsmCaps, src: &str) -> Result<LoweredSource, AsmError> {
        let mut syntax = test_syntax();
        syntax.caps = caps;
        lower_source(&parse_asm_cst_with(src, caps), &syntax, src)
    }

    #[test]
    fn param_lines_build_the_routine_interface() {
        let src = "\
.routine f, tapes=2, alpha=(3, 2), exits=1, noreturn
.param  num, ('_', '0', '1'), writes=('0', '1')
.param  flag, ('_', 'x')
.func f
stop
";
        let lowered = lower_with(iface_caps(), src).unwrap();
        let iface = lowered.interface.expect("interface present");
        assert_eq!(iface.routines.len(), 1);
        assert!(iface.alphabets.is_empty(), "the assembler authors none");
        let r = &iface.routines[0];
        assert_eq!(r.params, vec!["num", "flag"]);
        assert_eq!(r.glyphs[0], vec!["_", "0", "1"]);
        assert_eq!(r.writes[0], vec!["0", "1"]);
        assert!(r.writes[1].is_empty());
        assert_eq!(r.enters, vec![None, None]);
        assert_eq!(r.leaves, vec![None, None]);
        assert_eq!(r.opaque, vec![false, false]);
        assert_eq!(r.exits, 1);
        assert!(!r.returns);
    }

    #[test]
    fn param_contract_suffixes_and_opaque() {
        let src = "\
.routine f, tapes=1, alpha=(3)
.param  num, ('_', '0', '1'), writes=('0', '1'), enters=('1'), leaves=('0', '1'), opaque
.func f
stop
";
        let r = &lower_with(iface_caps(), src)
            .unwrap()
            .interface
            .unwrap()
            .routines[0];
        assert_eq!(r.enters[0].as_deref(), Some(&["1".to_string()][..]));
        assert_eq!(
            r.leaves[0].as_deref(),
            Some(&["0".to_string(), "1".to_string()][..])
        );
        assert!(r.opaque[0]);
        // A routine with no tail returns and takes no exits.
        assert_eq!(r.exits, 0);
        assert!(r.returns);
    }

    #[test]
    fn param_count_must_equal_tapes() {
        let src = ".routine f, tapes=2, alpha=(3, 2)\n.param num, ('_', '0', '1')\n.func f\nstop\n";
        let e = lower_with(iface_caps(), src).unwrap_err();
        assert!(
            matches!(e.kind, AsmErrorKind::BadSignature(ref m) if m.contains("1 .param line(s) for tapes=2")),
            "{e}"
        );
        // One too many is reported the same way, at the extra line.
        let src = ".routine f, tapes=1, alpha=(2)\n.param a, ('_', 'x')\n.param b, ('_', 'x')\n.func f\nstop\n";
        let e = lower_with(iface_caps(), src).unwrap_err();
        assert!(
            matches!(e.kind, AsmErrorKind::BadSignature(ref m) if m.contains("2 .param line(s) for tapes=1")),
            "{e}"
        );
    }

    #[test]
    fn param_glyph_count_must_equal_cardinality() {
        let src = ".routine f, tapes=1, alpha=(3)\n.param num, ('_', '0')\n.func f\nstop\n";
        let e = lower_with(iface_caps(), src).unwrap_err();
        assert!(
            matches!(e.kind, AsmErrorKind::BadSignature(ref m) if m.contains("2 glyphs for a cardinality of 3")),
            "{e}"
        );
    }

    #[test]
    fn writes_must_be_a_subset_of_the_alphabet() {
        let src =
            ".routine f, tapes=1, alpha=(2)\n.param num, ('_', 'a'), writes=('b')\n.func f\nstop\n";
        let e = lower_with(iface_caps(), src).unwrap_err();
        assert!(
            matches!(e.kind, AsmErrorKind::BadSignature(ref m) if m.contains("writes names `b`")),
            "{e}"
        );
    }

    #[test]
    fn head_clauses_must_be_a_subset_of_the_alphabet() {
        for (suffix, clause) in [("enters=('b')", "enters"), ("leaves=('b')", "leaves")] {
            let src = format!(
                ".routine f, tapes=1, alpha=(2)\n.param num, ('_', 'a'), {suffix}\n.func f\nstop\n"
            );
            let e = lower_with(iface_caps(), &src).unwrap_err();
            let wanted = format!("{clause} names `b`");
            assert!(
                matches!(e.kind, AsmErrorKind::BadSignature(ref m) if m.contains(&wanted)),
                "{e}"
            );
        }
    }

    #[test]
    fn an_empty_head_clause_is_rejected() {
        for suffix in ["enters=()", "leaves=()"] {
            let src = format!(
                ".routine f, tapes=1, alpha=(2)\n.param n, ('_', 'a'), {suffix}\n.func f\nstop\n"
            );
            let e = lower_with(iface_caps(), &src).unwrap_err();
            assert!(
                matches!(
                    e.kind,
                    AsmErrorKind::BadSignature(ref m)
                        if m == "`enters=`/`leaves=` list at least one glyph; omit the suffix for no clause"
                ),
                "{e}"
            );
        }
        // An empty `writes=()` is not the same thing: a routine that
        // writes nothing is well-formed and says so.
        let src =
            ".routine f, tapes=1, alpha=(2)\n.param n, ('_', 'a'), writes=()\n.func f\nstop\n";
        let r = &lower_with(iface_caps(), src)
            .unwrap()
            .interface
            .unwrap()
            .routines[0];
        assert!(r.writes[0].is_empty());
    }

    #[test]
    fn a_param_needs_a_pending_routine() {
        let src = ".param n, ('_', 'a')\n.func f\nstop\n";
        let e = lower_with(iface_caps(), src).unwrap_err();
        assert!(
            matches!(e.kind, AsmErrorKind::BadSignature(ref m) if m == "`.param` precedes no `.routine`"),
            "{e}"
        );
    }

    #[test]
    fn the_routine_tail_needs_param_lines() {
        for tail in ["exits=1", "noreturn"] {
            let src = format!(".routine f, tapes=1, alpha=(2), {tail}\n.func f\nstop\n");
            let e = lower_with(iface_caps(), &src).unwrap_err();
            assert!(
                matches!(e.kind, AsmErrorKind::BadSignature(ref m)
                    if m == "`exits=`/`noreturn` need `.param` lines"),
                "{e}"
            );
        }
    }

    #[test]
    fn exits_is_a_u8_field() {
        // The wire record carries one byte, so the directive's u32 value
        // is range-checked rather than truncated.
        let src =
            ".routine f, tapes=1, alpha=(2), exits=256\n.param t, ('_', 'a')\n.func f\nstop\n";
        let e = lower_with(iface_caps(), src).unwrap_err();
        assert!(
            matches!(e.kind, AsmErrorKind::BadSignature(ref m) if m == "exits must be 0..=255"),
            "{e}"
        );
        // The last value that fits comes through.
        let src =
            ".routine f, tapes=1, alpha=(2), exits=255\n.param t, ('_', 'a')\n.func f\nstop\n";
        let r = &lower_with(iface_caps(), src)
            .unwrap()
            .interface
            .unwrap()
            .routines[0];
        assert_eq!(r.exits, 255);
    }

    #[test]
    fn interface_directives_are_code_section_only() {
        for line in [".param t, ('_', 'a')", ".graph g, 1"] {
            let src = format!(".section tables\n{line}\n.section code\n.func f\nstop\n");
            let e = lower_with(iface_caps(), &src).unwrap_err();
            assert!(
                matches!(e.kind, AsmErrorKind::BadTable(m)
                    if m == "only table directives are allowed in the tables section"),
                "{line}: {e}"
            );
        }
    }

    #[test]
    fn a_param_after_its_func_precedes_no_routine() {
        // The `.routine` it would describe was consumed when the function
        // was defined, so a `.param` below the `.func` attaches to
        // nothing — the must-precede rule, reported the same way as a
        // `.param` with no `.routine` at all.
        let src = ".routine f, tapes=1, alpha=(2)\n.func f\n.param t, ('_', 'a')\nstop\n";
        let e = lower_with(iface_caps(), src).unwrap_err();
        assert!(
            matches!(e.kind, AsmErrorKind::BadSignature(ref m) if m == "`.param` precedes no `.routine`"),
            "{e}"
        );
    }

    #[test]
    fn interface_is_all_or_none_per_object() {
        let src = "\
.routine f, tapes=1, alpha=(2)
.param t, ('_', 'a')
.func f
stop
.routine g, tapes=1, alpha=(2)
.func g
stop
";
        let e = lower_with(iface_caps(), src).unwrap_err();
        assert!(
            matches!(e.kind, AsmErrorKind::BadSignature(ref m) if m.contains("function `g` lacks `.param` lines")),
            "{e}"
        );
    }

    #[test]
    fn graph_and_grafted_directives_collect_at_object_level() {
        let src = "\
.graph lib::g, 3735928559
.grafted other::h, 42
.routine f, tapes=1, alpha=(2)
.param t, ('_', 'a')
.func f
stop
";
        let lowered = lower_with(iface_caps(), src).unwrap();
        assert_eq!(
            lowered.interface.as_ref().unwrap().graphs,
            vec![ExportedGraph {
                name: "lib::g".into(),
                digest: 3_735_928_559
            }]
        );
        assert_eq!(
            lowered.grafts,
            vec![GraftProvenance {
                graph: "other::h".into(),
                digest: 42
            }]
        );
    }

    #[test]
    fn digest_directives_need_a_fully_described_object() {
        // An interface section parallels the blobs on the wire, so a
        // digest directive obliges every function to be signed … and the
        // message names the cause, since nothing on the `.func` line
        // hints at what obliged it.
        let src = ".graph g, 1\n.func f\nstop\n";
        let e = lower_with(iface_caps(), src).unwrap_err();
        assert!(
            matches!(e.kind, AsmErrorKind::BadSignature(ref m)
                if m == "function `f` lacks a `.routine` signature — a `.graph`/`.grafted` \
                         line obliges an interface for every function"),
            "{e}"
        );
        // … and to carry its parameters.
        let src = ".graph g, 1\n.routine f, tapes=1, alpha=(2)\n.func f\nstop\n";
        let e = lower_with(iface_caps(), src).unwrap_err();
        assert!(
            matches!(e.kind, AsmErrorKind::BadSignature(ref m)
                if m == "function `f` lacks `.param` lines — a `.graph`/`.grafted` \
                         line obliges an interface for every function"),
            "{e}"
        );
        // A file that signs and describes functions on its own gets the
        // plain message: the digest line is not what obliged anything.
        let src = "\
.routine f, tapes=1, alpha=(2)
.param t, ('_', 'a')
.func f
stop
.func g
stop
";
        let e = lower_with(iface_caps(), src).unwrap_err();
        assert!(
            matches!(e.kind, AsmErrorKind::BadSignature(ref m)
                if m == "function `g` lacks a `.routine` signature"),
            "{e}"
        );
    }

    #[test]
    fn digest_directives_must_precede_the_first_func() {
        let src = ".func f\nstop\n.graph g, 1\n";
        let e = lower_with(iface_caps(), src).unwrap_err();
        assert!(
            matches!(
                e.kind,
                AsmErrorKind::Syntax("`.graph`/`.grafted` precede the first `.func`")
            ),
            "{e}"
        );
    }

    #[test]
    fn interface_directives_are_unknown_without_the_cap() {
        let caps = AsmCaps {
            tables: true,
            rept: true,
            ..AsmCaps::default()
        };
        for word in [".param", ".graph", ".grafted"] {
            let src = format!(".func f\n{word}\nstop\n");
            let e = lower_with(caps, &src).unwrap_err();
            assert!(
                matches!(e.kind, AsmErrorKind::UnknownMnemonic(ref w) if w == word),
                "{e}"
            );
        }
    }

    #[test]
    fn malformed_interface_directives_get_their_own_complaint() {
        // Structurally inexact lines degrade to Lines in the CST; lowering
        // answers with the directive's own grammar, never "unknown
        // mnemonic" (mirror the malformed `.routine` path).
        for (src, word) in [
            (".func f\n.param n\nstop\n", ".param"),
            (".graph g\n.func f\nstop\n", ".graph"),
            (".grafted g\n.func f\nstop\n", ".grafted"),
        ] {
            let e = lower_with(iface_caps(), src).unwrap_err();
            assert!(
                matches!(e.kind, AsmErrorKind::Syntax(m) if m.contains(word)),
                "{src:?}: {e}"
            );
        }
    }

    #[test]
    fn an_object_without_interface_content_carries_none() {
        let src = ".routine f, tapes=1, alpha=(2)\n.func f\nstop\n";
        let lowered = lower_with(iface_caps(), src).unwrap();
        assert!(lowered.interface.is_none());
        assert!(lowered.grafts.is_empty());
        assert!(lowered.signatures.is_some());
    }

    // -- Symbolic binding operands and `exits=(…)` (caps.interface) -----

    /// The interface tier plus `vectors`: a binding call's `[..]` operand
    /// is a bracket region, which only that cap lexes.
    fn binding_caps() -> AsmCaps {
        AsmCaps {
            vectors: true,
            ..iface_caps()
        }
    }

    /// The `n`th item's classified operand in function `name`.
    fn operand_of<'a>(lowered: &'a LoweredSource, name: &str, n: usize) -> &'a SourceOperand {
        let f = lowered
            .functions
            .iter()
            .find(|f| f.name == name)
            .expect("function defined");
        match &f.items[n] {
            SourceItem::Instr { operand, .. } => operand,
            other => panic!("not an instruction: {other:?}"),
        }
    }

    #[test]
    fn bound_call_with_names_labels_and_exits_lowers() {
        let src = "\
.func f
        call    g [num: 1{3->'0', 4=>'1'}, ctl: 0{}] exits=(won, lost)
won:    stop
lost:   stop
";
        let lowered = lower_with(binding_caps(), src).unwrap();
        let SourceOperand::BoundCallOp {
            target,
            binding,
            exits,
        } = operand_of(&lowered, "f", 0)
        else {
            panic!("not a bound call")
        };
        assert_eq!(target.name, "g");
        assert_eq!(binding.len(), 2);
        assert_eq!(binding[0].param.as_deref(), Some("num"));
        assert_eq!(binding[0].caller_tape, 1);
        assert!(binding[0].map_written);
        assert!(!binding[0].open);
        assert_eq!(
            binding[0].pairs,
            vec![
                (3, SourceDst::Label("0".into()), false),
                (4, SourceDst::Label("1".into()), true),
            ]
        );
        assert_eq!(binding[1].param.as_deref(), Some("ctl"));
        assert!(binding[1].map_written);
        assert!(binding[1].pairs.is_empty());
        assert_eq!(
            exits.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(),
            vec!["won", "lost"]
        );
        // Each exit label carries its own span inside the operand, so a
        // later resolution failure points at the label, not the vector.
        assert_eq!(exits[0].span, Span::new(2, 61, 2, 64)); // `won`
        assert_eq!(exits[1].span, Span::new(2, 66, 2, 70)); // `lost`
    }

    #[test]
    fn exit_label_spans_survive_irregular_spacing() {
        // The operand's text is verbatim, so the span arithmetic must
        // hold for any spelling the grammar accepts, not just the
        // canonical one space after each comma.
        let src = ".func f\n        call g [0] exits=( won ,  lost )\nwon:    stop\nlost:   stop\n";
        let lowered = lower_with(binding_caps(), src).unwrap();
        let SourceOperand::BoundCallOp { exits, .. } = operand_of(&lowered, "f", 0) else {
            panic!("not a bound call")
        };
        assert_eq!(
            exits.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(),
            vec!["won", "lost"]
        );
        assert_eq!(exits[0].span, Span::new(2, 28, 2, 31)); // `won`
        assert_eq!(exits[1].span, Span::new(2, 35, 2, 39)); // `lost`
    }

    #[test]
    fn positional_entries_and_numeric_destinations_still_lower() {
        let src = ".func f\n        call g [2{1->3, 2=>0}, 0]\n        stop\n";
        let lowered = lower_with(binding_caps(), src).unwrap();
        let SourceOperand::BoundCallOp { binding, exits, .. } = operand_of(&lowered, "f", 0) else {
            panic!("not a bound call")
        };
        assert!(binding.iter().all(|b| b.param.is_none()));
        assert_eq!(
            binding[0].pairs,
            vec![
                (1, SourceDst::Index(3), false),
                (2, SourceDst::Index(0), true)
            ]
        );
        assert!(binding[0].map_written);
        assert!(!binding[1].map_written);
        assert!(exits.is_empty());
    }

    #[test]
    fn an_open_map_marks_the_entry_open() {
        let src = ".func f\n        call g [ctl: 0{1->'a', *}]\n        stop\n";
        let lowered = lower_with(binding_caps(), src).unwrap();
        let SourceOperand::BoundCallOp { binding, .. } = operand_of(&lowered, "f", 0) else {
            panic!("not a bound call")
        };
        assert!(binding[0].open);
        assert!(binding[0].map_written, "an open map is a written one");
        assert_eq!(binding[0].pairs.len(), 1);
    }

    #[test]
    fn a_misplaced_open_marker_names_its_rule() {
        let src = ".func f\n        call g [0{*, 1->2}]\n        stop\n";
        let e = lower_with(binding_caps(), src).unwrap_err();
        assert!(
            matches!(e.kind, AsmErrorKind::BadFrame(ref m)
                if m == "`*` closes a map: write it last, once"),
            "{e}"
        );
    }

    #[test]
    fn mixed_named_and_positional_entries_are_rejected() {
        let src = ".func f\n        call g [num: 1, 0]\n        stop\n";
        let e = lower_with(binding_caps(), src).unwrap_err();
        assert!(
            matches!(e.kind, AsmErrorKind::BadFrame(ref m)
                if m == "a binding names every entry or none"),
            "{e}"
        );
    }

    #[test]
    fn the_symbolic_binding_forms_need_the_interface_cap() {
        // Without the cap a glyph label never lexes, so only the two
        // forms the rept/vectors caps alone can spell reach lowering:
        // a named entry (`num:`) and an open map (`*` is the rept cap's
        // own token). Both are refused.
        let caps = AsmCaps {
            interface: false,
            ..binding_caps()
        };
        for src in [
            ".func f\n        call g [num: 1]\n        stop\n",
            ".func f\n        call g [0{*}]\n        stop\n",
        ] {
            let e = lower_with(caps, src).unwrap_err();
            assert!(
                matches!(e.kind, AsmErrorKind::BadFrame(ref m)
                    if m == "named entries and open maps need the interface capability"),
                "{src:?}: {e}"
            );
        }
    }

    #[test]
    fn an_empty_binding_is_a_binding_only_under_the_interface_cap() {
        // `[]` is the zero-tape binding an exit vector may ride on.
        let lowered =
            lower_with(binding_caps(), ".func f\n        call g []\n        stop\n").unwrap();
        let SourceOperand::BoundCallOp { binding, .. } = operand_of(&lowered, "f", 0) else {
            panic!("not a bound call")
        };
        assert!(binding.is_empty());
        // Without the cap it stays the error it has always been.
        let caps = AsmCaps {
            interface: false,
            ..binding_caps()
        };
        let e = lower_with(caps, ".func f\n        call g []\n        stop\n").unwrap_err();
        assert!(
            matches!(e.kind, AsmErrorKind::BadFrame(ref m) if m.contains("at least one")),
            "{e}"
        );
    }

    #[test]
    fn exits_operand_only_on_call() {
        let src = ".func f\n        jmp L exits=(L)\nL:      stop\n";
        let e = lower_with(binding_caps(), src).unwrap_err();
        assert!(
            matches!(
                e.kind,
                AsmErrorKind::BadOperand("only a call takes an exit vector")
            ),
            "{e}"
        );
        // Not a flow operand at all: the same complaint, not a vector one.
        let src = ".func f\n        wr [1] exits=(L)\nL:      stop\n";
        let e = lower_with(binding_caps(), src).unwrap_err();
        assert!(
            matches!(
                e.kind,
                AsmErrorKind::BadOperand("only a call takes an exit vector")
            ),
            "{e}"
        );
    }

    #[test]
    fn an_exit_vector_needs_a_binding() {
        let src = ".func f\n        call g exits=(L)\nL:      stop\n";
        let e = lower_with(binding_caps(), src).unwrap_err();
        assert!(
            matches!(e.kind, AsmErrorKind::BadOperand(m) if m.contains("needs a binding")),
            "{e}"
        );
    }

    #[test]
    fn exit_vector_entries_are_label_names() {
        for (src, needle) in [
            (
                ".func f\n        call g [0] exits=(1)\n        stop\n",
                "label",
            ),
            (
                ".func f\n        call g [0] exits=(a::b)\n        stop\n",
                "label",
            ),
            (
                ".func f\n        call g [0] exits=()\n        stop\n",
                "at least one",
            ),
            (
                ".func f\n        call g [0] exits=(a,)\n        stop\n",
                "label",
            ),
        ] {
            let e = lower_with(binding_caps(), src).unwrap_err();
            assert!(
                matches!(e.kind, AsmErrorKind::BadOperand(m) if m.contains(needle)),
                "{src:?}: {e}"
            );
        }
    }

    #[test]
    fn param_names_are_identifiers() {
        // A parameter is named at a call site (`num: 1`), so its name
        // takes the label grammar — no dots, no `::`.
        for name in ["a::b", "a.b"] {
            let src = format!(
                ".routine f, tapes=1, alpha=(2)\n.param {name}, ('_', 'a')\n.func f\nstop\n"
            );
            let e = lower_with(iface_caps(), &src).unwrap_err();
            assert!(
                matches!(e.kind, AsmErrorKind::BadSignature(ref m)
                    if m == "`.param` names use letters, digits, underscore"),
                "{name}: {e}"
            );
        }
    }

    #[test]
    fn duplicate_param_names_are_rejected() {
        let src = "\
.routine f, tapes=2, alpha=(2, 2)
.param num, ('_', 'a')
.param num, ('_', 'b')
.func f
stop
";
        let e = lower_with(iface_caps(), src).unwrap_err();
        assert!(
            matches!(e.kind, AsmErrorKind::BadSignature(ref m)
                if m == "`.param num` is declared twice"),
            "{e}"
        );
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
