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

/// Per-dialect syntax capabilities. Defaults = the classic surface
/// (everything off) — the .pma dialect's acceptance is unchanged.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AsmCaps {
    /// `.section` regions, `.row`/`.targets`/`.target` directives.
    pub tables: bool,
    /// `.rept v, lo, hi` … `.endr` with `{expr}` substitution.
    pub rept: bool,
    /// `[a, *, -, <, >, .]` vector operand tokens.
    pub vectors: bool,
}

pub struct ArchSyntax {
    pub entries: Vec<SyntaxEntry>,
    pub relax_pairs: Vec<RelaxPair>,
    pub entry_opcode: u8,
    /// The debugger-break opcode, when the arch has one (drives the
    /// leftover-debugger lint; None = rule silent).
    pub break_opcode: Option<u8>,
    /// The unmapped-symbol trap opcode, when the arch has one — the
    /// `trap #kind` instruction the mono-stamping composition engine
    /// synthesizes for a crossed map hole (kind `0` = unmapped read,
    /// `1` = unmapped write). It cannot be inferred from the syntax table
    /// (an `Imm8`-operand entry is ambiguous — a multi-exit return carries
    /// the same operand kind), so each dialect declares it explicitly.
    /// `None` when the dialect has no trap instruction, which is an error
    /// only if a reachable mono binding needs a trap synthesized
    /// (docs/core.md (the composition engine)).
    pub trap_opcode: Option<u8>,
    /// Opt-in lexer/parser surface for this dialect. Default (all off)
    /// keeps the classic assembly grammar byte-for-byte.
    pub caps: AsmCaps,
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

    /// The opcode of this dialect's framed call (`call.m`-shape), if it has
    /// one: the single `FramedCall`-operand entry. The composition engine
    /// needs it to lower a declarative bound call into a framed call
    /// without naming any architecture's mnemonic (core is arch-agnostic —
    /// README (workspace layout)). `None` when the dialect has no framed
    /// call, which is an error only if a reachable binding needs lowering.
    pub fn framed_call_opcode(&self) -> Option<u8> {
        self.entries
            .iter()
            .find(|e| e.operand == OperandKind::FramedCall)
            .map(|e| e.opcode)
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
    /// vwrmv 0x19 (WriteMoveVec, two `[..]` groups) |
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
                    opcode: 0x19,
                    mnemonic: "vwrmv",
                    operand: OperandKind::WriteMoveVec,
                    flow: FT,
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
            break_opcode: None,
            trap_opcode: None,
            caps: AsmCaps::default(),
        }
    }
}
