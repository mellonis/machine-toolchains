//! Lossless assembly CST → per-function source items (docs/formats.md
//! (assembly text) grammar). Shaping is total and lives in `cst.rs`;
//! this pass validates + classifies, attaching a precise [`Span`] to
//! every diagnostic. Replaces the old line-oriented parser.

use super::cst::{
    AsmCst, AsmItem, AsmItemKind, FuncCst, InstrCst, LabelCst, LineCst, OperandToken, ReptCst,
    SectionCst, TableDirectiveCst, TableDirectiveKind, parse_asm_cst_with,
};
use super::subst::substitute;
use super::syntax::ArchSyntax;
use super::{AsmError, AsmErrorKind};
use crate::diagnostics::Span;
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
    /// Pinned by this module's interface. Every function-name diagnostic
    /// (`bad function name`, `duplicate function`) is raised here at
    /// lowering from the CST, so the assembler reads only `name`; the
    /// stored span has no downstream consumer this task.
    #[allow(dead_code)]
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
}

impl SourceTable {
    pub fn name(&self) -> &SpannedName {
        match self {
            SourceTable::Match { name, .. } | SourceTable::Dispatch { name, .. } => name,
        }
    }
}

/// Everything one lowered source file carries: the functions (code
/// section) and the tables (`.section tables`). Cap-off dialects never
/// produce tables — the CST never shapes the directives.
#[derive(Debug)]
pub struct LoweredSource {
    pub functions: Vec<SourceFunction>,
    pub tables: Vec<SourceTable>,
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
    Ok(LoweredSource {
        functions: ctx.functions,
        tables: ctx.tables,
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
        // Sections and table directives shape only under the opt-in caps;
        // cap-off dialects (PM-1) never reach these arms.
        AsmItemKind::Section(s) => lower_section(s, ctx)?,
        AsmItemKind::TableDirective(d) => lower_table_directive(d, ctx)?,
        AsmItemKind::Rept(r) => lower_rept(r, syntax, source, ctx)?,
    }
    Ok(())
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
    let inner = token
        .text
        .strip_prefix('[')
        .and_then(|t| t.strip_suffix(']'))
        .ok_or_else(|| {
            err(
                token.span,
                AsmErrorKind::BadVector("expected a `[..]` vector"),
            )
        })?;
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
                return Err(err(
                    token.span,
                    AsmErrorKind::BadVector("empty vector element"),
                ));
            }
            payload => VecElem::Payload(payload.parse::<u32>().map_err(|_| {
                err(
                    token.span,
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
    ctx.functions.push(SourceFunction {
        name: func.name.clone(),
        name_span: func.name_span,
        local: func.local,
        items: Vec::new(),
    });
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
            operand: classify_operand(entry.operand, instr)?,
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
    ctx.functions.push(SourceFunction {
        name: name.to_string(),
        name_span: word_span,
        local,
        items: Vec::new(),
    });
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

fn classify_operand(kind: OperandKind, instr: &InstrCst) -> Result<SourceOperand, AsmError> {
    let operands = &instr.operands;
    match kind {
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
    }
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
