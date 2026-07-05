//! PM-1 assembly: the spec §5 mnemonic table bound to the core framework.

use mtc_core::asm::{ArchSyntax, AsmError, Flow, RelaxPair, SyntaxEntry};
use mtc_core::formats::ARCH_PM1;
use mtc_core::formats::executable::Executable;
use mtc_core::formats::object::ObjectFile;
use mtc_core::vm::OperandKind;

use crate::arch::opcodes::*;

pub fn pm1_syntax() -> ArchSyntax {
    use Flow::{Branch, Call as CallF, FallThrough as FT, Jump, Stop};
    use OperandKind::{None as N, RelI8, RelI32, SymbolVec};
    ArchSyntax {
        entries: vec![
            SyntaxEntry {
                opcode: NOP,
                mnemonic: "nop",
                operand: N,
                flow: FT,
            },
            SyntaxEntry {
                opcode: STP,
                mnemonic: "stp",
                operand: N,
                flow: Stop,
            },
            SyntaxEntry {
                opcode: HLT,
                mnemonic: "hlt",
                operand: N,
                flow: Stop,
            },
            SyntaxEntry {
                opcode: LFT,
                mnemonic: "lft",
                operand: N,
                flow: FT,
            },
            SyntaxEntry {
                opcode: RGT,
                mnemonic: "rgt",
                operand: N,
                flow: FT,
            },
            SyntaxEntry {
                opcode: WR,
                mnemonic: "wr",
                operand: SymbolVec,
                flow: FT,
            },
            SyntaxEntry {
                opcode: JMP,
                mnemonic: "jmp",
                operand: RelI32,
                flow: Jump,
            },
            SyntaxEntry {
                opcode: JM,
                mnemonic: "jm",
                operand: RelI32,
                flow: Branch,
            },
            SyntaxEntry {
                opcode: JNM,
                mnemonic: "jnm",
                operand: RelI32,
                flow: Branch,
            },
            SyntaxEntry {
                opcode: CALL,
                mnemonic: "call",
                operand: RelI32,
                flow: CallF,
            },
            SyntaxEntry {
                opcode: RET,
                mnemonic: "ret",
                operand: N,
                flow: Stop,
            },
            SyntaxEntry {
                opcode: ENT,
                mnemonic: "ent",
                operand: N,
                flow: FT,
            },
            SyntaxEntry {
                opcode: BRK,
                mnemonic: "brk",
                operand: N,
                flow: FT,
            },
            SyntaxEntry {
                opcode: JMP_S,
                mnemonic: "jmp.s",
                operand: RelI8,
                flow: Jump,
            },
            SyntaxEntry {
                opcode: JM_S,
                mnemonic: "jm.s",
                operand: RelI8,
                flow: Branch,
            },
            SyntaxEntry {
                opcode: JNM_S,
                mnemonic: "jnm.s",
                operand: RelI8,
                flow: Branch,
            },
            SyntaxEntry {
                opcode: CALL_S,
                mnemonic: "call.s",
                operand: RelI8,
                flow: CallF,
            },
        ],
        relax_pairs: vec![
            RelaxPair {
                far: JMP,
                short: JMP_S,
            },
            RelaxPair {
                far: JM,
                short: JM_S,
            },
            RelaxPair {
                far: JNM,
                short: JNM_S,
            },
        ],
        entry_opcode: ENT,
    }
}

pub fn assemble(source: &str, with_debug: bool) -> Result<ObjectFile, AsmError> {
    mtc_core::asm::assemble(&pm1_syntax(), ARCH_PM1, source, with_debug)
}

pub fn disassemble_object(obj: &ObjectFile) -> String {
    mtc_core::asm::disassemble_object(&pm1_syntax(), obj)
}

pub fn disassemble_executable(exe: &Executable) -> String {
    mtc_core::asm::disassemble_executable(&pm1_syntax(), exe)
}
