//! The data an architecture supplies to drive assembly/disassembly.

use crate::vm::OperandKind;

/// Control-flow class — drives assembly operand rules and
/// recursive-descent disassembly (successor edges).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flow {
    FallThrough,
    Stop,
    Jump,
    Branch,
    Call,
}

pub struct SyntaxEntry {
    pub opcode: u8,
    pub mnemonic: &'static str,
    pub operand: OperandKind,
    pub flow: Flow,
}

pub struct RelaxPair {
    pub far: u8,
    pub short: u8,
}

pub struct ArchSyntax {
    pub entries: Vec<SyntaxEntry>,
    pub relax_pairs: Vec<RelaxPair>,
    pub entry_opcode: u8,
}

impl ArchSyntax {
    pub fn by_mnemonic(&self, m: &str) -> Option<&SyntaxEntry> {
        self.entries.iter().find(|e| e.mnemonic == m)
    }

    pub fn by_opcode(&self, op: u8) -> Option<&SyntaxEntry> {
        self.entries.iter().find(|e| e.opcode == op)
    }

    pub fn short_of(&self, far: u8) -> Option<u8> {
        self.relax_pairs
            .iter()
            .find(|p| p.far == far)
            .map(|p| p.short)
    }

    pub fn is_call(&self, op: u8) -> bool {
        self.by_opcode(op).is_some_and(|e| e.flow == Flow::Call)
    }
}

#[cfg(test)]
pub(crate) mod fixture {
    use super::*;
    use crate::vm::OperandKind;

    /// Standalone syntax for asm framework tests (independent of TestArch —
    /// the assembler never executes, it only needs kinds/sizes).
    /// nop 0x01 | stop 0x02 | wr 0x07 (SymbolVec) | jmp 0x20 far / 0x30 short |
    /// call 0x21 (far, symbol operand) | ret 0x0B | entry marker 0x0E |
    /// br 0x22 (RelI8, unpaired — disassembler traversal coverage only; per
    /// framework invariant a RelI8 Jump/Branch not in a relax pair takes the
    /// far path in the assembler, so `br` must not appear in assembler tests)
    pub(crate) fn test_syntax() -> ArchSyntax {
        use Flow::{Branch, Call, FallThrough as FT, Jump, Stop};
        ArchSyntax {
            entries: vec![
                SyntaxEntry {
                    opcode: 0x01,
                    mnemonic: "nop",
                    operand: OperandKind::None,
                    flow: FT,
                },
                SyntaxEntry {
                    opcode: 0x02,
                    mnemonic: "stop",
                    operand: OperandKind::None,
                    flow: Stop,
                },
                SyntaxEntry {
                    opcode: 0x07,
                    mnemonic: "wr",
                    operand: OperandKind::SymbolVec,
                    flow: FT,
                },
                SyntaxEntry {
                    opcode: 0x20,
                    mnemonic: "jmp",
                    operand: OperandKind::RelI32,
                    flow: Jump,
                },
                SyntaxEntry {
                    opcode: 0x30,
                    mnemonic: "jmp.s",
                    operand: OperandKind::RelI8,
                    flow: Jump,
                },
                SyntaxEntry {
                    opcode: 0x21,
                    mnemonic: "call",
                    operand: OperandKind::RelI32,
                    flow: Call,
                },
                SyntaxEntry {
                    opcode: 0x22,
                    mnemonic: "br",
                    operand: OperandKind::RelI8,
                    flow: Branch,
                },
                SyntaxEntry {
                    opcode: 0x0B,
                    mnemonic: "ret",
                    operand: OperandKind::None,
                    flow: Stop,
                },
                SyntaxEntry {
                    opcode: 0x0E,
                    mnemonic: "ent",
                    operand: OperandKind::None,
                    flow: FT,
                },
            ],
            relax_pairs: vec![RelaxPair {
                far: 0x20,
                short: 0x30,
            }],
            entry_opcode: 0x0E,
        }
    }
}
