//! Binary → canonical `.pma` text (spec §6.4). Output is valid assembler
//! input; object round-trips are exact.

use std::collections::{BTreeMap, BTreeSet};

use super::syntax::ArchSyntax;
use crate::formats::executable::Executable;
use crate::formats::object::{ObjectFile, SymbolDef};
use crate::vm::OperandKind;

/// One decoded instruction (or undecodable byte) at a code offset.
struct Decoded {
    addr: u32,
    len: u32,
    body: Body,
}

enum Body {
    Instr {
        mnemonic: &'static str,
        operand: DecodedOperand,
    },
    Raw(u8),
}

enum DecodedOperand {
    None,
    Ints(Vec<u32>),
    RelTarget(u32), // absolute target address (same space as `addr`)
}

/// Decode ONE instruction at `addr` within `[addr, end)`. `None` means an
/// unknown opcode or a truncated operand; the caller decides how to fall
/// back (`.byte` in streams, path-stop in traversal).
fn decode_at(syntax: &ArchSyntax, code: &[u8], addr: u32, end: u32) -> Option<Decoded> {
    let opcode = code[addr as usize];
    let entry = syntax.by_opcode(opcode)?;
    let (len, operand) = match entry.operand {
        OperandKind::None => (1, DecodedOperand::None),
        OperandKind::RelI8 => {
            if addr + 2 > end {
                return None;
            }
            let off = code[(addr + 1) as usize] as i8;
            let target = (i64::from(addr) + 2 + i64::from(off)) as u32;
            (2, DecodedOperand::RelTarget(target))
        }
        OperandKind::RelI32 => {
            if addr + 5 > end {
                return None;
            }
            let bytes: [u8; 4] = code[(addr + 1) as usize..(addr + 5) as usize]
                .try_into()
                .unwrap();
            let off = i32::from_le_bytes(bytes);
            let target = (i64::from(addr) + 5 + i64::from(off)) as u32;
            (5, DecodedOperand::RelTarget(target))
        }
        OperandKind::SymbolVec => {
            let mut i = addr + 1;
            let mut symbols = Vec::new();
            let mut ok = false;
            while i < end {
                let b = code[i as usize];
                symbols.push(u32::from(b & 0x7F));
                i += 1;
                if b & 0x80 != 0 {
                    ok = true;
                    break;
                }
            }
            if !ok {
                return None;
            }
            (i - addr, DecodedOperand::Ints(symbols))
        }
    };
    Some(Decoded {
        addr,
        len,
        body: Body::Instr {
            mnemonic: entry.mnemonic,
            operand,
        },
    })
}

fn decode_stream(syntax: &ArchSyntax, code: &[u8], start: u32, end: u32) -> Vec<Decoded> {
    let mut out = Vec::new();
    let mut addr = start;
    while addr < end {
        match decode_at(syntax, code, addr, end) {
            Some(d) => {
                addr += d.len;
                out.push(d);
            }
            None => {
                out.push(Decoded {
                    addr,
                    len: 1,
                    body: Body::Raw(code[addr as usize]),
                });
                addr += 1;
            }
        }
    }
    out
}

fn grid_line(label: Option<&str>, mnemonic: &str, operand: &str) -> String {
    let label_field = match label {
        Some(l) => format!("{l}:"),
        None => String::new(),
    };
    let mut line = format!("{label_field:<8}{mnemonic:<8}{operand}");
    while line.ends_with(' ') {
        line.pop();
    }
    line
}

pub fn disassemble_object(syntax: &ArchSyntax, obj: &ObjectFile) -> String {
    let mut out = String::new();
    // reloc lookup: (blob, hole offset) -> symbol name
    let reloc_at: BTreeMap<(u32, u32), &str> = obj
        .relocations
        .iter()
        .map(|r| {
            (
                (r.blob, r.offset),
                obj.symbols[r.symbol as usize].name.as_str(),
            )
        })
        .collect();

    for symbol in &obj.symbols {
        let SymbolDef::Defined { blob } = symbol.def else {
            continue;
        };
        let code = &obj.blobs[blob as usize];
        out.push_str(&format!(".func {}\n", symbol.name));
        // Skip the leading entry byte if present (implied by .func).
        let start = if code.first() == Some(&syntax.entry_opcode) {
            1
        } else {
            0
        };
        let decoded = decode_stream(syntax, code, start, code.len() as u32);

        let mut targets = BTreeSet::new();
        for d in &decoded {
            if let Body::Instr {
                operand: DecodedOperand::RelTarget(t),
                mnemonic,
            } = &d.body
            {
                let is_call = syntax
                    .by_mnemonic(mnemonic)
                    .is_some_and(|e| syntax.is_call(e.opcode));
                if !is_call {
                    targets.insert(*t);
                }
            }
        }

        for d in &decoded {
            let label_name = targets
                .contains(&d.addr)
                .then(|| format!("L{:04X}", d.addr));
            let line = match &d.body {
                Body::Raw(b) => grid_line(label_name.as_deref(), ".byte", &b.to_string()),
                Body::Instr { mnemonic, operand } => {
                    let entry = syntax.by_mnemonic(mnemonic).unwrap();
                    let operand_text = match operand {
                        DecodedOperand::None => String::new(),
                        DecodedOperand::Ints(v) => {
                            v.iter().map(u32::to_string).collect::<Vec<_>>().join(", ")
                        }
                        DecodedOperand::RelTarget(t) => {
                            if syntax.is_call(entry.opcode) {
                                // The hole starts one byte after the opcode.
                                match reloc_at.get(&(blob, d.addr + 1)) {
                                    Some(name) => (*name).to_string(),
                                    None => format!("L{t:04X}"), // resolved call (linker output)
                                }
                            } else {
                                format!("L{t:04X}")
                            }
                        }
                    };
                    grid_line(label_name.as_deref(), mnemonic, &operand_text)
                }
            };
            out.push_str(&line);
            out.push('\n');
        }
    }
    out
}

/// Decode ONE instruction at `addr` (None = unknown opcode / truncated).
fn decode_one(syntax: &ArchSyntax, code: &[u8], addr: u32) -> Option<Decoded> {
    decode_at(syntax, code, addr, code.len() as u32)
}

pub fn disassemble_executable(syntax: &ArchSyntax, exe: &Executable) -> String {
    use crate::asm::syntax::Flow;
    let code = &exe.code;
    let len = code.len() as u32;

    // Recursive-descent discovery (exact in v1: no indirect control flow).
    // instrs: every reachable instruction; roots: entry + all call targets.
    let mut instrs: BTreeMap<u32, Decoded> = BTreeMap::new();
    let mut roots: BTreeSet<u32> = BTreeSet::from([exe.entry]);
    let mut work: Vec<u32> = vec![exe.entry];
    while let Some(addr) = work.pop() {
        if addr >= len || instrs.contains_key(&addr) {
            continue;
        }
        let Some(d) = decode_one(syntax, code, addr) else {
            continue; // unknown byte ends this path; gap pass will .byte it
        };
        let Body::Instr { mnemonic, operand } = &d.body else {
            unreachable!()
        };
        let entry = syntax.by_mnemonic(mnemonic).unwrap();
        let next = addr + d.len;
        match (entry.flow, operand) {
            (Flow::FallThrough, _) => work.push(next),
            (Flow::Stop, _) => {}
            (Flow::Jump, DecodedOperand::RelTarget(t)) => work.push(*t),
            (Flow::Branch, DecodedOperand::RelTarget(t)) => {
                work.push(*t);
                work.push(next);
            }
            (Flow::Call, DecodedOperand::RelTarget(t)) => {
                roots.insert(*t);
                work.push(*t);
                work.push(next);
            }
            _ => work.push(next), // malformed flow/operand combo: keep walking
        }
        instrs.insert(addr, d);
    }

    let roots: Vec<u32> = roots.into_iter().filter(|&r| r < len).collect();
    let func_name = |addr: u32| format!("func_{addr:04X}");
    let region_end = |i: usize| roots.get(i + 1).copied().unwrap_or(len);

    let mut out = String::new();
    for (i, &root) in roots.iter().enumerate() {
        let end = region_end(i);
        out.push_str(&format!(".func {}\n", func_name(root)));

        // Jump-target labels within this region.
        let mut targets = BTreeSet::new();
        for (_, d) in instrs.range(root..end) {
            if let Body::Instr {
                mnemonic,
                operand: DecodedOperand::RelTarget(t),
            } = &d.body
            {
                let e = syntax.by_mnemonic(mnemonic).unwrap();
                if e.flow != Flow::Call && *t > root && *t < end {
                    targets.insert(*t);
                }
            }
        }

        let mut addr = root;
        let mut first = true;
        while addr < end {
            let label_name = targets.contains(&addr).then(|| format!("L{addr:04X}"));
            match instrs.get(&addr) {
                None => {
                    out.push_str(&grid_line(
                        label_name.as_deref(),
                        ".byte",
                        &code[addr as usize].to_string(),
                    ));
                    out.push('\n');
                    addr += 1;
                }
                Some(d) => {
                    let Body::Instr { mnemonic, operand } = &d.body else {
                        unreachable!()
                    };
                    let entry = syntax.by_mnemonic(mnemonic).unwrap();
                    // The root's leading entry instruction is implied by .func.
                    if first && entry.opcode == syntax.entry_opcode {
                        first = false;
                        addr += d.len;
                        continue;
                    }
                    first = false;
                    let text = match operand {
                        DecodedOperand::None => Some(String::new()),
                        DecodedOperand::Ints(v) => {
                            Some(v.iter().map(u32::to_string).collect::<Vec<_>>().join(", "))
                        }
                        DecodedOperand::RelTarget(t) => {
                            if entry.flow == Flow::Call && roots.contains(t) {
                                Some(func_name(*t))
                            } else if entry.flow != Flow::Call && *t > root && *t < end {
                                Some(format!("L{t:04X}"))
                            } else {
                                None // cross-region jump: .byte fallback
                            }
                        }
                    };
                    match text {
                        Some(operand_text) => {
                            out.push_str(&grid_line(
                                label_name.as_deref(),
                                mnemonic,
                                &operand_text,
                            ));
                            out.push('\n');
                        }
                        None => {
                            for k in 0..d.len {
                                out.push_str(&grid_line(
                                    if k == 0 { label_name.as_deref() } else { None },
                                    ".byte",
                                    &code[(addr + k) as usize].to_string(),
                                ));
                                out.push('\n');
                            }
                        }
                    }
                    addr += d.len;
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::asm::assembler::assemble;
    use crate::asm::syntax::fixture::test_syntax;
    use crate::formats::executable::Executable;

    #[test]
    fn object_disassembly_uses_canonical_grid() {
        let syntax = test_syntax();
        let src = ".func f\nL0001:  nop\n        jmp.s   L0001\n        wr      1\n        call    g\n        stop\n";
        let obj = assemble(&syntax, 0x7E, src, false).unwrap();
        let text = disassemble_object(&syntax, &obj);
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines[0], ".func f");
        assert_eq!(lines[1], "L0001:  nop");
        assert_eq!(lines[2], "        jmp.s   L0001");
        assert_eq!(lines[3], "        wr      1");
        assert_eq!(lines[4], "        call    g");
        assert_eq!(lines[5], "        stop");
    }

    #[test]
    fn round_trip_law() {
        let syntax = test_syntax();
        let src = "\
.func f
START:  nop
        jmp     START
        wr      1, 2
        call    g
        call    missing
        stop
.func g
        wr      0
        ret
";
        let obj1 = assemble(&syntax, 0x7E, src, false).unwrap();
        let text = disassemble_object(&syntax, &obj1);
        let obj2 = assemble(&syntax, 0x7E, &text, false).unwrap();
        assert_eq!(obj1, obj2);
    }

    #[test]
    fn unknown_byte_falls_back_to_byte_directive_and_round_trips() {
        let syntax = test_syntax();
        // Hand-build an object with an undecodable byte (0x55 not in table).
        let obj = crate::formats::object::ObjectFile {
            arch: 0x7E,
            symbols: vec![crate::formats::object::Symbol {
                name: "f".into(),
                def: crate::formats::object::SymbolDef::Defined { blob: 0 },
            }],
            blobs: vec![vec![0x0E, 0x55, 0x02]],
            relocations: vec![],
            debug: None,
        };
        let text = disassemble_object(&syntax, &obj);
        assert!(text.contains(".byte   85"));
        let back = assemble(&syntax, 0x7E, &text, false).unwrap();
        assert_eq!(back.blobs, obj.blobs);
    }

    #[test]
    fn executable_disassembly_discovers_functions_by_traversal() {
        let syntax = test_syntax();
        // f at 0 calls g at 7: f = [0E][21 off=+1][02] (call end 6; 7-6=1),
        // g = [0E][0B].
        let code = vec![0x0E, 0x21, 0x01, 0x00, 0x00, 0x00, 0x02, 0x0E, 0x0B];
        let exe = Executable {
            arch: 0x7E,
            entry: 0,
            code,
        };
        let text = disassemble_executable(&syntax, &exe);
        assert!(text.contains(".func func_0000"));
        assert!(text.contains(".func func_0007"));
        assert!(text.contains("call    func_0007"));
        assert!(text.contains("ret"));
    }

    #[test]
    fn entry_valued_operand_byte_does_not_split_functions() {
        let syntax = test_syntax();
        // f calls g at 20 (0x14): call offset = 20 - 6 = 14 = 0x0E — the
        // operand's first LE byte EQUALS the entry opcode. A byte-scanning
        // discoverer would invent a bogus function at addr 2; traversal
        // must not. Bytes 7..20 are unreachable padding → .byte lines.
        let mut code = vec![0x0E, 0x21, 0x0E, 0x00, 0x00, 0x00, 0x02];
        code.extend(std::iter::repeat_n(0x01, 13)); // unreachable nops
        code.extend([0x0E, 0x0B]); // g at 20
        let exe = Executable {
            arch: 0x7E,
            entry: 0,
            code,
        };
        let text = disassemble_executable(&syntax, &exe);
        assert!(text.contains(".func func_0000"));
        assert!(text.contains(".func func_0014"));
        assert!(
            !text.contains("func_0002"),
            "operand byte must not become a function"
        );
        assert!(text.contains("call    func_0014"));
        assert!(
            text.contains(".byte   1"),
            "unreachable padding dumps as bytes"
        );
    }

    #[test]
    fn branch_traversal_discovers_fall_through() {
        let syntax = test_syntax();
        // 0: ent | 1: br +1 -> 4 | 3: stop (fall-through, must be discovered) | 4: ret
        let code = vec![0x0E, 0x22, 0x01, 0x02, 0x0B];
        let exe = Executable {
            arch: 0x7E,
            entry: 0,
            code,
        };
        let text = disassemble_executable(&syntax, &exe);
        assert!(
            text.contains("stop"),
            "fall-through path must be discovered"
        );
        assert!(text.contains("ret"));
        assert!(text.contains("br      L0004"));
        assert!(!text.contains(".byte"), "everything reachable, no gaps");
    }

    #[test]
    fn cross_region_jump_falls_back_to_bytes() {
        let syntax = test_syntax();
        // f calls g (so g is a root) AND jumps into g's BODY (addr 13):
        // 0: ent | 1: call +6 -> 12 | 6: jmp +2 -> 13 | 11: stop | 12: ent | 13: ret
        let code = vec![
            0x0E, 0x21, 0x06, 0x00, 0x00, 0x00, 0x20, 0x02, 0x00, 0x00, 0x00, 0x02, 0x0E, 0x0B,
        ];
        let exe = Executable {
            arch: 0x7E,
            entry: 0,
            code,
        };
        let text = disassemble_executable(&syntax, &exe);
        assert!(text.contains(".func func_000C"));
        assert!(text.contains("call    func_000C"));
        // the jmp into g's body cannot be a local label -> whole instruction as bytes
        assert!(text.contains(".byte   32")); // 0x20 opcode byte
        assert!(
            !text.contains("jmp"),
            "cross-region jmp must not print as jmp"
        );
        assert!(text.contains("ret"));
    }
}
