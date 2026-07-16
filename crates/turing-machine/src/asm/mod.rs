//! TM-1 assembly: the TM-1 mnemonic table bound to the core framework.
//! The sibling of `crates/post-machine/src/asm/mod.rs` — where PM-1 drives
//! one two-symbol tape, TM-1 drives up to sixteen tapes through the shared
//! match/dispatch table engine, so its dialect turns on the table / rept /
//! vector capabilities the classic PM-1 grammar leaves off.

use mtc_core::asm::{ArchSyntax, AsmCaps, AsmError, Flow, RelaxPair, SyntaxEntry};
use mtc_core::formats::ARCH_TM1;
use mtc_core::formats::executable::Executable;
use mtc_core::formats::object::ObjectFile;
use mtc_core::vm::OperandKind;

use crate::arch::opcodes::*;

/// TM-1 `.tma` dialect version — an acceptance contract (same kind of
/// contract as the `.pmc` language version and PM-1's `.pma` dialect):
/// pre-1.0 it is 0.N and N bumps on ANY grammar change. 0.1: the initial
/// TM-1 assembly surface — the sixteen mnemonics below plus the sectioned
/// `.routine` / `.section` / `.row` / `.targets` / `.rept` directives and
/// the `[..]` write- and move-vector operand forms. 0.2: the frames
/// instructions — `trap #kind`, the framed call `call.m target, F`, and the
/// multi-exit return `retx #k` — with the `#imm` immediate operand form.
pub const TM1_TMA_DIALECT_VERSION: &str = "0.2";

/// The TM-1 mnemonic table (the `.tma` dialect). Opcode/operand shapes
/// mirror the TM-1 arch module (`crate::arch`); flows follow the same
/// conventions PM-1 uses for each shape (jump / branch / call / stop /
/// fall-through) so the arch-agnostic reachability walk and lint rules
/// treat both dialects uniformly.
pub fn tm1_syntax() -> ArchSyntax {
    use Flow::{Branch, Call as CallF, FallThrough as FT, Jump, Stop};
    use OperandKind::{FramedCall, Imm8, MoveVec, None as N, RelI8, RelI32, SymbolVec, TableRef};
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
            // Vector read: latch every tape head into its TR slot in one
            // fetch. No operand — the tape count fixes the width.
            SyntaxEntry {
                opcode: RD,
                mnemonic: "rd",
                operand: N,
                flow: FT,
            },
            // Match-table walk: a pure lookup that sets MR and falls
            // through (mirrors the neutral `tmatch` flow in the core
            // table-assembly tests).
            SyntaxEntry {
                opcode: MTC,
                mnemonic: "mtc",
                operand: TableRef,
                flow: FT,
            },
            // Dispatch jump: transfers control through the table indexed by
            // MR (an unconditional transfer, like `jmp`).
            SyntaxEntry {
                opcode: DJMP,
                mnemonic: "djmp",
                operand: TableRef,
                flow: Jump,
            },
            // Vector write: one symbol per tape, `-` keeps a cell untouched.
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
            // Vector move: one step per tape (`.` stay, `<` left, `>` right).
            SyntaxEntry {
                opcode: MOV,
                mnemonic: "mov",
                operand: MoveVec,
                flow: FT,
            },
            // Raise a typed trap explicitly (`trap #kind`): a plain
            // fall-through, like `nop` for the reachability walk.
            SyntaxEntry {
                opcode: TRAP,
                mnemonic: "trap",
                operand: Imm8,
                flow: FT,
            },
            // Framed call (`call.m target, F`): the call flow (a static
            // successor plus the fall-through) matches plain `call`. Its
            // relaxation to a short form is a later phase — `relax_pairs`
            // stays call-only.
            SyntaxEntry {
                opcode: CALL_M,
                mnemonic: "call.m",
                operand: FramedCall,
                flow: CallF,
            },
            // Multi-exit frame return (`retx #k`): a transfer with no
            // static successor, so `Stop` for the walk — mirrors `ret`.
            SyntaxEntry {
                opcode: RETX,
                mnemonic: "retx",
                operand: Imm8,
                flow: Stop,
            },
            SyntaxEntry {
                opcode: CALL_S,
                mnemonic: "call.s",
                operand: RelI8,
                flow: CallF,
            },
        ],
        // TM-1's only short form is the call. The pair feeds LINKER
        // relaxation and disassembly display (short call prints as far
        // `call`) — never assembler behavior: the assembler always emits
        // far `call` and rejects `call.s <target>` by name; only the
        // linker's fixpoint selects the short form.
        relax_pairs: vec![RelaxPair {
            far: CALL,
            short: CALL_S,
        }],
        entry_opcode: ENT,
        break_opcode: Some(BRK),
        // TM-1's multi-tape dispatch surface uses the whole sectioned
        // grammar: match/dispatch tables, `.rept` blocks, and `[..]`
        // vector operands.
        caps: AsmCaps {
            tables: true,
            rept: true,
            vectors: true,
        },
    }
}

pub fn assemble(source: &str, with_debug: bool) -> Result<ObjectFile, AsmError> {
    mtc_core::asm::assemble(&tm1_syntax(), ARCH_TM1, source, with_debug)
}

pub fn disassemble_object(obj: &ObjectFile) -> String {
    mtc_core::asm::disassemble_object(&tm1_syntax(), obj)
}

pub fn disassemble_executable(exe: &Executable) -> String {
    mtc_core::asm::disassemble_executable(&tm1_syntax(), exe, None)
}

pub fn disassemble_executable_with_map(
    exe: &Executable,
    map: &mtc_core::linker::MapFile,
) -> String {
    mtc_core::asm::disassemble_executable(&tm1_syntax(), exe, Some(map))
}

pub fn listing_executable(exe: &Executable, map: Option<&mtc_core::linker::MapFile>) -> String {
    mtc_core::asm::listing_executable(&tm1_syntax(), exe, map)
}

pub fn link(
    objects: &[ObjectFile],
    libraries: &[ObjectFile],
    options: mtc_core::linker::LinkOptions,
) -> Result<mtc_core::linker::LinkOutput, mtc_core::linker::LinkError> {
    mtc_core::linker::link(&tm1_syntax(), objects, libraries, options)
}
