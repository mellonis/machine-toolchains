//! `.pma` text → per-function source items (spec §6.4 grammar).

use super::syntax::ArchSyntax;
use super::{AsmError, AsmErrorKind};
use crate::vm::OperandKind;

#[derive(Debug)]
pub(crate) struct SourceFunction {
    pub name: String,
    pub items: Vec<SourceItem>,
}

#[derive(Debug)]
pub(crate) enum SourceItem {
    Instr {
        line: usize,
        labels: Vec<String>,
        opcode: u8,
        operand: SourceOperand,
    },
    RawByte {
        line: usize,
        labels: Vec<String>,
        value: u8,
    },
}

#[derive(Debug)]
pub(crate) enum SourceOperand {
    None,
    Ints(Vec<i64>),
    Name(String),
}

fn err(line: usize, kind: AsmErrorKind) -> AsmError {
    AsmError { line, kind }
}

fn is_ident(s: &str) -> bool {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if c.is_alphabetic() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_alphanumeric() || c == '_')
}

pub(crate) fn parse(syntax: &ArchSyntax, source: &str) -> Result<Vec<SourceFunction>, AsmError> {
    let mut functions: Vec<SourceFunction> = Vec::new();
    let mut pending_labels: Vec<String> = Vec::new();

    for (idx, raw) in source.lines().enumerate() {
        let line_no = idx + 1;
        let text = raw.split(';').next().unwrap_or("").trim();
        if text.is_empty() {
            continue;
        }

        // `.func` must match as an exact token: `.function x` is NOT a
        // function directive — it falls through to mnemonic handling below
        // (and errors there; never silently accepted).
        let mut directive = text.splitn(2, char::is_whitespace);
        if directive.next() == Some(".func") {
            if !pending_labels.is_empty() {
                return Err(err(
                    line_no,
                    AsmErrorKind::Syntax("label at end of function"),
                ));
            }
            let name = directive.next().unwrap_or("").trim();
            if !is_ident(name) {
                return Err(err(line_no, AsmErrorKind::Syntax("bad function name")));
            }
            if functions.iter().any(|f| f.name == name) {
                return Err(err(
                    line_no,
                    AsmErrorKind::DuplicateFunction(name.to_string()),
                ));
            }
            functions.push(SourceFunction {
                name: name.to_string(),
                items: Vec::new(),
            });
            continue;
        }

        let mut rest = text;
        // Labels: leading `NAME:` prefixes, possibly several on one line.
        while let Some(colon) = rest.find(':') {
            let (head, tail) = rest.split_at(colon);
            let head = head.trim();
            if !is_ident(head) {
                break; // not a label — let mnemonic handling report it
            }
            pending_labels.push(head.to_string());
            rest = tail[1..].trim_start();
        }
        if rest.is_empty() {
            if functions.is_empty() && !pending_labels.is_empty() {
                return Err(err(line_no, AsmErrorKind::OutsideFunction));
            }
            continue; // label-only line
        }

        let current = functions
            .last_mut()
            .ok_or(err(line_no, AsmErrorKind::OutsideFunction))?;
        let mut parts = rest.splitn(2, char::is_whitespace);
        let word = parts.next().unwrap();
        let operand_text = parts.next().unwrap_or("").trim();

        if word == ".byte" {
            let value: u8 = operand_text
                .parse()
                .map_err(|_| err(line_no, AsmErrorKind::BadOperand(".byte needs 0..=255")))?;
            current.items.push(SourceItem::RawByte {
                line: line_no,
                labels: std::mem::take(&mut pending_labels),
                value,
            });
            continue;
        }

        let entry = syntax
            .by_mnemonic(word)
            .ok_or_else(|| err(line_no, AsmErrorKind::UnknownMnemonic(word.to_string())))?;
        let operands: Vec<&str> = if operand_text.is_empty() {
            Vec::new()
        } else {
            operand_text.split(',').map(str::trim).collect()
        };

        let operand = match entry.operand {
            OperandKind::None => {
                if !operands.is_empty() {
                    return Err(err(line_no, AsmErrorKind::BadOperand("takes no operand")));
                }
                SourceOperand::None
            }
            OperandKind::RelI8 | OperandKind::RelI32 => {
                let [one] = operands.as_slice() else {
                    return Err(err(line_no, AsmErrorKind::BadOperand("takes one name")));
                };
                if !is_ident(one) {
                    return Err(err(
                        line_no,
                        AsmErrorKind::BadOperand("jump/call operands are names, not numbers"),
                    ));
                }
                SourceOperand::Name((*one).to_string())
            }
            OperandKind::SymbolVec => {
                if operands.is_empty() {
                    return Err(err(
                        line_no,
                        AsmErrorKind::BadOperand("takes symbol indices"),
                    ));
                }
                let mut ints = Vec::with_capacity(operands.len());
                for o in &operands {
                    ints.push(o.parse::<i64>().map_err(|_| {
                        err(
                            line_no,
                            AsmErrorKind::BadOperand("symbol indices are integers"),
                        )
                    })?);
                }
                SourceOperand::Ints(ints)
            }
        };

        current.items.push(SourceItem::Instr {
            line: line_no,
            labels: std::mem::take(&mut pending_labels),
            opcode: entry.opcode,
            operand,
        });
    }

    if !pending_labels.is_empty() {
        let line = source.lines().count();
        return Err(err(line, AsmErrorKind::Syntax("label at end of function")));
    }
    Ok(functions)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::asm::syntax::fixture::test_syntax;

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
        let funcs = parse(&test_syntax(), src).unwrap();
        assert_eq!(funcs.len(), 2);
        assert_eq!(funcs[0].name, "f");
        let items = &funcs[0].items;
        assert_eq!(items.len(), 5);
        match &items[0] {
            SourceItem::Instr {
                labels,
                opcode,
                operand,
                ..
            } => {
                assert_eq!(labels, &vec!["L1".to_string()]);
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
                assert!(matches!(operand, SourceOperand::Name(n) if n == "L1"));
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
        let funcs = parse(&test_syntax(), src).unwrap();
        match &funcs[0].items[0] {
            SourceItem::Instr { labels, .. } => {
                assert_eq!(labels, &vec!["L1".to_string(), "L2".to_string()]);
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn byte_directive_parses() {
        let src = ".func f\n        .byte 255\n";
        let funcs = parse(&test_syntax(), src).unwrap();
        assert!(matches!(
            funcs[0].items[0],
            SourceItem::RawByte { value: 255, .. }
        ));
    }

    #[test]
    fn func_directive_requires_exact_token() {
        // `.function` must never be silently accepted as `.func`. With no
        // function open, the open-function check fires first, so the error
        // is OutsideFunction (still line-accurate). Inside a function, the
        // word reaches mnemonic lookup and reports UnknownMnemonic.
        let e = parse(&test_syntax(), ".function f\n").unwrap_err();
        assert_eq!((e.line, &e.kind), (1, &AsmErrorKind::OutsideFunction));

        let e = parse(&test_syntax(), ".func f\n.function g\n").unwrap_err();
        assert_eq!(e.line, 2);
        assert!(matches!(e.kind, AsmErrorKind::UnknownMnemonic(ref m) if m == ".function"));
    }

    #[test]
    fn error_cases_carry_line_numbers() {
        let syntax = test_syntax();
        let e = parse(&syntax, "        nop\n").unwrap_err();
        assert_eq!((e.line, &e.kind), (1, &AsmErrorKind::OutsideFunction));

        let e = parse(&syntax, ".func f\n        bogus\n").unwrap_err();
        assert_eq!(e.line, 2);
        assert!(matches!(e.kind, AsmErrorKind::UnknownMnemonic(ref m) if m == "bogus"));

        let e = parse(&syntax, ".func f\n.func f\n        nop\n").unwrap_err();
        assert!(matches!(e.kind, AsmErrorKind::DuplicateFunction(ref n) if n == "f"));

        let e = parse(&syntax, ".func f\n        jmp 5\n").unwrap_err();
        assert!(matches!(e.kind, AsmErrorKind::BadOperand(_))); // jumps take labels, not ints

        let e = parse(&syntax, ".func f\n        wr\n").unwrap_err();
        assert!(matches!(e.kind, AsmErrorKind::BadOperand(_)));

        let e = parse(&syntax, ".func f\nL1:\n").unwrap_err();
        assert!(matches!(e.kind, AsmErrorKind::Syntax(_))); // dangling label
    }
}
