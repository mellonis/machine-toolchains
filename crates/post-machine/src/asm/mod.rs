//! PM-1 assembly: the docs/isa.md mnemonic table bound to the core framework.

use mtc_core::asm::{ArchSyntax, AsmCaps, AsmError, Flow, RelaxPair, SyntaxEntry};
use mtc_core::formats::ARCH_PM1;
use mtc_core::formats::executable::Executable;
use mtc_core::formats::object::ObjectFile;
use mtc_core::vm::OperandKind;

use crate::arch::opcodes::*;

/// PM-1 `.pma` dialect version — an acceptance contract (docs/formats.md
/// (assembly text)): pre-1.0 it is 0.N and N bumps on ANY grammar
/// change. 0.2: labels tightened to letters/digits/underscore. 0.3: the
/// fused write+move mnemonics `wrl`/`wrr` are accepted.
pub const PM1_PMA_DIALECT_VERSION: &str = "0.3";

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
            // Fused write+move (docs/isa.md): `wr x; lft` / `wr x; rgt` in
            // one fetch. No short forms, so no relax pairs.
            SyntaxEntry {
                opcode: WRL,
                mnemonic: "wrl",
                operand: SymbolVec,
                flow: FT,
            },
            SyntaxEntry {
                opcode: WRR,
                mnemonic: "wrr",
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
            // The call pair feeds LINKER relaxation and disassembly display
            // (short call prints as far `call`) — never assembler behavior:
            // the assembler always emits far `call` and rejects `call.s` by
            // name; only the linker's fixpoint selects the short form.
            RelaxPair {
                far: CALL,
                short: CALL_S,
            },
        ],
        entry_opcode: ENT,
        break_opcode: Some(BRK),
        // PM-1 has no trap instruction — a one-tape two-symbol machine has
        // no symbol maps to cross, so it never needs a synthesized trap.
        trap_opcode: None,
        // PM-1 `.pma` uses the classic assembly grammar — no vector /
        // substitution / table surface.
        caps: AsmCaps::default(),
    }
}

pub fn assemble(source: &str, with_debug: bool) -> Result<ObjectFile, AsmError> {
    mtc_core::asm::assemble(&pm1_syntax(), ARCH_PM1, source, with_debug)
}

pub fn disassemble_object(obj: &ObjectFile) -> String {
    mtc_core::asm::disassemble_object(&pm1_syntax(), obj)
}

pub fn disassemble_executable(exe: &Executable) -> String {
    mtc_core::asm::disassemble_executable(&pm1_syntax(), exe, None)
}

pub fn disassemble_executable_with_map(
    exe: &Executable,
    map: &mtc_core::linker::MapFile,
) -> String {
    mtc_core::asm::disassemble_executable(&pm1_syntax(), exe, Some(map))
}

pub fn listing_executable(exe: &Executable, map: Option<&mtc_core::linker::MapFile>) -> String {
    mtc_core::asm::listing_executable(&pm1_syntax(), exe, map)
}

pub fn link(
    objects: &[ObjectFile],
    libraries: &[ObjectFile],
    options: mtc_core::linker::LinkOptions,
) -> Result<mtc_core::linker::LinkOutput, mtc_core::linker::LinkError> {
    mtc_core::linker::link(&pm1_syntax(), objects, libraries, options)
}
