//! Lossless assembly CST → per-function source items (docs/formats.md
//! (assembly text) grammar). Shaping is total and lives in `cst.rs`;
//! this pass validates + classifies, attaching a precise [`Span`] to
//! every diagnostic. Replaces the old line-oriented parser.

use super::cst::{AsmCst, AsmItemKind, FuncCst, InstrCst, LabelCst, LineCst};
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

pub(crate) fn lower(cst: &AsmCst, syntax: &ArchSyntax) -> Result<Vec<SourceFunction>, AsmError> {
    let mut functions: Vec<SourceFunction> = Vec::new();
    let mut pending: Vec<SpannedName> = Vec::new();

    for item in &cst.items {
        match &item.kind {
            AsmItemKind::Comment(_) => {}
            AsmItemKind::Raw(raw) => return Err(err(raw.span, AsmErrorKind::RawLine)),
            AsmItemKind::Func(func) => lower_func(func, &mut functions, &pending)?,
            AsmItemKind::Line(line) => lower_line(line, syntax, &mut functions, &mut pending)?,
            // Sections, table directives, and `.rept` blocks are shaped
            // only under the opt-in caps; PM-1's caps are off, so these
            // are unreachable through the real arch and reachable only by
            // a fake-caps direct lower call. Lowering them lands in a
            // later task — until then, a clear error rather than a silent
            // drop.
            AsmItemKind::Section(s) => {
                return Err(err(
                    s.span,
                    AsmErrorKind::Syntax("sections lower in a later task"),
                ));
            }
            AsmItemKind::TableDirective(d) => {
                return Err(err(
                    d.span,
                    AsmErrorKind::Syntax("table directives lower in a later task"),
                ));
            }
            AsmItemKind::Rept(r) => {
                return Err(err(
                    r.span,
                    AsmErrorKind::Syntax("rept blocks lower in a later task"),
                ));
            }
        }
    }

    // A label with no instruction after it, at end of input.
    if let Some(first) = pending.first() {
        return Err(err(
            first.span,
            AsmErrorKind::Syntax("label at end of function"),
        ));
    }
    Ok(functions)
}

fn lower_func(
    func: &FuncCst,
    functions: &mut Vec<SourceFunction>,
    pending: &[SpannedName],
) -> Result<(), AsmError> {
    // A label immediately before a `.func` binds to nothing (legacy: the
    // first check in the `.func` branch, before the name is parsed).
    if let Some(first) = pending.first() {
        return Err(err(
            first.span,
            AsmErrorKind::Syntax("label at end of function"),
        ));
    }
    if !is_symbol_name(&func.name) {
        return Err(err(
            func.name_span,
            AsmErrorKind::Syntax("bad function name"),
        ));
    }
    if functions.iter().any(|f| f.name == func.name) {
        return Err(err(
            func.name_span,
            AsmErrorKind::DuplicateFunction(func.name.clone()),
        ));
    }
    functions.push(SourceFunction {
        name: func.name.clone(),
        name_span: func.name_span,
        local: func.local,
        items: Vec::new(),
    });
    Ok(())
}

fn lower_line(
    line: &LineCst,
    syntax: &ArchSyntax,
    functions: &mut Vec<SourceFunction>,
    pending: &mut Vec<SpannedName>,
) -> Result<(), AsmError> {
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

    let Some(instr) = &line.instr else {
        // Label-only line. Outside any function it is stray code;
        // otherwise the labels wait for the next instruction.
        if functions.is_empty() {
            // A label-only line always carries at least one label.
            return Err(err(line.labels[0].span, AsmErrorKind::OutsideFunction));
        }
        pending.extend(line.labels.iter().map(spanned));
        return Ok(());
    };

    // A malformed `.func` directive — the CST keeps it a Line with word
    // ".func" when the directive is not structurally exact. Only when
    // ".func" is the instruction word with no labels before it;
    // `L1: .func …` is a plain unknown mnemonic. This fires before the
    // open-function check, matching the legacy `.func`-branch precedence.
    if instr.word == ".func" && line.labels.is_empty() {
        return lower_malformed_func(instr, functions, pending);
    }

    // Outside any function an instruction is stray code — reported
    // before mnemonic lookup (matches the pinned `.function f` case).
    if functions.is_empty() {
        return Err(err(instr.word_span, AsmErrorKind::OutsideFunction));
    }

    // Labels bound to this instruction: those pending from prior
    // label-only lines, then this line's own.
    let mut labels: Vec<SpannedName> = std::mem::take(pending);
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
    functions
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
fn lower_malformed_func(
    instr: &InstrCst,
    functions: &mut Vec<SourceFunction>,
    pending: &[SpannedName],
) -> Result<(), AsmError> {
    // Same first check as the exact-`.func` path: a label immediately
    // before any `.func` (well-formed or not) binds to nothing.
    if let Some(first) = pending.first() {
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
    if functions.iter().any(|f| f.name == name) {
        return Err(err(
            word_span,
            AsmErrorKind::DuplicateFunction(name.to_string()),
        ));
    }
    functions.push(SourceFunction {
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
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::asm::cst::parse_asm_cst;
    use crate::asm::syntax::fixture::test_syntax;

    fn lower_src(src: &str) -> Result<Vec<SourceFunction>, AsmError> {
        lower(&parse_asm_cst(src), &test_syntax())
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
}
