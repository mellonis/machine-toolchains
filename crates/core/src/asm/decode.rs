//! Shared single-instruction decode machinery for the disassembler (and,
//! later, the linker). All instruction knowledge arrives via [`ArchSyntax`].

use super::syntax::ArchSyntax;
use crate::vm::OperandKind;

/// One decoded instruction (or undecodable byte) at a code offset.
pub(crate) struct Decoded {
    pub(crate) addr: u32,
    pub(crate) len: u32,
    pub(crate) body: Body,
}

pub(crate) enum Body {
    Instr {
        mnemonic: &'static str,
        operand: DecodedOperand,
    },
    Raw(u8),
}

pub(crate) enum DecodedOperand {
    None,
    Ints(Vec<u32>),
    RelTarget(u32), // absolute target address (same space as `addr`)
    /// Absolute TABLE-space offset (TableRef) — NOT a code address; the
    /// traversal must never treat it as a successor.
    TableAddr(u32),
    /// A plain 8-bit immediate (Imm8), rendered `#<n>`.
    Imm(u8),
    /// A framed call (FramedCall): the resolved absolute code target (like
    /// [`DecodedOperand::RelTarget`]) and the absolute table-space offset
    /// of its frame descriptor.
    FramedCall {
        target: u32,
        table: u32,
    },
}

/// Decode ONE instruction at `addr` within `[addr, end)`. `None` means an
/// unknown opcode or a truncated operand; the caller decides how to fall
/// back (`.byte` in streams, path-stop in traversal).
pub(crate) fn decode_at(syntax: &ArchSyntax, code: &[u8], addr: u32, end: u32) -> Option<Decoded> {
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
        OperandKind::TableRef => {
            if addr + 5 > end {
                return None;
            }
            let bytes: [u8; 4] = code[(addr + 1) as usize..(addr + 5) as usize]
                .try_into()
                .unwrap();
            (5, DecodedOperand::TableAddr(u32::from_le_bytes(bytes)))
        }
        OperandKind::Imm8 => {
            if addr + 2 > end {
                return None;
            }
            (2, DecodedOperand::Imm(code[(addr + 1) as usize]))
        }
        OperandKind::FramedCall => {
            if addr + 9 > end {
                return None;
            }
            let rel_bytes: [u8; 4] = code[(addr + 1) as usize..(addr + 5) as usize]
                .try_into()
                .unwrap();
            let table_bytes: [u8; 4] = code[(addr + 5) as usize..(addr + 9) as usize]
                .try_into()
                .unwrap();
            let off = i32::from_le_bytes(rel_bytes);
            // The displacement resolves like RelI32: absolute target =
            // instruction end + off.
            let target = (i64::from(addr) + 9 + i64::from(off)) as u32;
            (
                9,
                DecodedOperand::FramedCall {
                    target,
                    table: u32::from_le_bytes(table_bytes),
                },
            )
        }
        // MoveVec shares SymbolVec's compact wire walk and, like the
        // fetch path, decodes to plain ints — the renderer distinguishes
        // the two kinds via the syntax entry, not the decoded shape.
        OperandKind::SymbolVec | OperandKind::MoveVec => {
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

pub(crate) fn decode_stream(
    syntax: &ArchSyntax,
    code: &[u8],
    start: u32,
    end: u32,
) -> Vec<Decoded> {
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
