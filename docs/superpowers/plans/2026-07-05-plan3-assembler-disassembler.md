# Plan 3/7: Assembler + Disassembler

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `.pma` text ↔ binary, both directions: an arch-generic assembler framework (two-pass, label resolution, short/far jump relaxation, `MO` object emission) and disassembler framework (canonical column grid, synthesized labels) in `mtc-core`, the PM-1 syntax table + public `asm`/`dis` API in `mtc-post-machine`, plus two deferred core items: operand **encoders** property-tested against the live core decoder, and `formats::sniff`.

**Architecture:** Spec §6.2/§6.4 + §5. The framework is data-driven: an `ArchSyntax` table (mnemonic ↔ opcode ↔ operand kind, relax pairs, entry opcode) supplied by the arch crate. `.func` blocks become per-function blobs; local labels resolve intra-blob with start-short-and-grow relaxation; `call NAME` always emits a far 4-byte hole + relocation (linker relaxes calls — Plan 4). Disassembly of an executable object-ifies it (`.func func_XXXX` at every `ent`), so `dis` output is always valid assembler input.

**Tech Stack:** Rust stable, edition 2024, crates `mtc-core` + `mtc-post-machine`. No new dependencies (proptest already a dev-dep of mtc-core).

**Spec:** `docs/superpowers/specs/2026-07-04-post-machine-toolchain-design.md` §5 (ISA/encodings), §6.2 (`MO`), §6.4 (`.pma`, canonical grid, dis behavior).

## Global Constraints

- Canonical grid emitted by the disassembler (spec §6.4): labels at column 0, mnemonics at column 8, operands at column 16 (comments at 32 — the v1 disassembler emits none). The assembler accepts any whitespace.
- `;` starts a comment; one instruction per line; labels `NAME:` bind to the next instruction; `.func NAME` opens a function and implicitly emits the entry opcode; `.byte N` emits a raw byte (needed for round-trip fallback).
- Relaxation (spec §5): bare relaxable mnemonics (`jmp`/`jm`/`jnm` in PM-1) start SHORT and are promoted to far when the offset leaves `i8` range; iterate to fixpoint (start-short-and-grow is monotone → guaranteed to converge). Explicit `.s` mnemonics FORCE short — out-of-range is an error. `call` is never relaxed by the assembler (always far + relocation; linker relaxes, Plan 4).
- Offsets are relative to the END of the instruction (spec §5).
- Symbol-vector operands: 7-bit payloads, high bit on the last element; payload > 0x7F is an encode error (spec Appendix A).
- Every `call NAME` produces a `Relocation { blob, offset-of-hole, symbol }`; names not defined by a `.func` in the same file become `External` symbols (spec §6.2).
- Round-trip law (spec §6.4, tested): `assemble(disassemble_object(obj)) == obj` for assembler-produced objects (same `with_debug` setting).
- Quality gates on every commit: `cargo test --workspace` green, `cargo clippy --workspace --all-targets -- -D warnings` clean, `cargo fmt --check` clean; no attribution footers.
- Commit policy: per-task commits pre-approved in this repo; never push. COMMIT your own work.

## Interfaces Established by This Plan

```rust
// mtc_core::vm::arch (additions)
pub fn encode_operand(operand: &Operand) -> Result<Vec<u8>, &'static str>;
// None → []; I8 → 1 byte; I32 → 4 bytes LE;
// Symbols → 7-bit payloads, high bit on last; errors: empty vector, payload > 0x7F

// mtc_core::formats (addition)
pub enum ContainerKind { Object, Executable, TapeBlock }
pub fn sniff(bytes: &[u8]) -> Option<ContainerKind>;   // 3-byte magic match

// mtc_core::asm (new module: asm/{mod,syntax,parser,assembler,disassembler}.rs)
/// Control-flow class of an instruction — drives both assembly (call =
/// symbol operand + relocation) and recursive-descent disassembly.
pub enum Flow {
    FallThrough,     // nop, tape ops, ent, brk
    Stop,            // stp, hlt, ret — no successors
    Jump,            // unconditional: successor = target only
    Branch,          // conditional: successors = target + fall-through
    Call,            // successors = fall-through; target = NEW FUNCTION root
}
pub struct SyntaxEntry {
    pub opcode: u8,
    pub mnemonic: &'static str,
    pub operand: OperandKind,
    pub flow: Flow,
}
pub struct RelaxPair { pub far: u8, pub short: u8 }
pub struct ArchSyntax {
    pub entries: Vec<SyntaxEntry>,
    pub relax_pairs: Vec<RelaxPair>,
    pub entry_opcode: u8,
}
impl ArchSyntax {
    pub fn by_mnemonic(&self, m: &str) -> Option<&SyntaxEntry>;
    pub fn by_opcode(&self, op: u8) -> Option<&SyntaxEntry>;
    pub fn short_of(&self, far: u8) -> Option<u8>;
    pub fn is_call(&self, op: u8) -> bool;  // flow == Flow::Call
}

#[derive(Debug, PartialEq, Eq)]
pub struct AsmError { pub line: usize, pub kind: AsmErrorKind }
#[derive(Debug, PartialEq, Eq)]
pub enum AsmErrorKind {
    Syntax(&'static str),
    UnknownMnemonic(String),
    OutsideFunction,
    DuplicateFunction(String),
    DuplicateLabel(String),
    UnknownLabel(String),
    BadOperand(&'static str),
    ShortOffsetOutOfRange { target: String },
    EncodeError(&'static str),
}
pub fn assemble(syntax: &ArchSyntax, arch_id: u8, source: &str, with_debug: bool)
    -> Result<ObjectFile, AsmError>;
pub fn disassemble_object(syntax: &ArchSyntax, obj: &ObjectFile) -> String;
pub fn disassemble_executable(syntax: &ArchSyntax, exe: &Executable) -> String;
// exe form uses RECURSIVE-DESCENT discovery, not byte scanning (function
// discovery from raw bytes is heuristic in general; with no indirect
// control flow in v1, traversal is exact): worklist from `entry`,
// following Flow edges; every Call target becomes a function root.
// Regions = sorted roots partition; visited instructions print normally
// (`.func func_XXXX` per root, root's leading ent implied); bytes never
// reached by traversal print as `.byte` lines. This is immune to
// ent-valued bytes inside operands. Calls print their target root's
// synthesized name; a jump whose target lies outside its own region is
// emitted as `.byte` fallback lines (v1 — revisit with Plan 6 tail calls,
// which will also need jump-targets-that-are-roots added to discovery).

// mtc_post_machine::asm (new module)
pub fn pm1_syntax() -> ArchSyntax;   // spec §5 mnemonics; relax pairs jmp/jm/jnm; entry ENT
pub fn assemble(source: &str, with_debug: bool) -> Result<ObjectFile, AsmError>;
pub fn disassemble_object(obj: &ObjectFile) -> String;
pub fn disassemble_executable(exe: &Executable) -> String;
```

Parsing rules (the `.pma` grammar, spec §6.4): per line — strip from first `;`, trim; empty → skip; `.func NAME`; `.byte N`; `[LABEL:] [mnemonic [operand{,operand}*]]`; label-only lines bind the label to the next instruction in the same function. Operands: integers (decimal, optional `-`) or identifiers. By operand kind: `RelI8`/`RelI32` take ONE identifier — a local label when `flow` is `Jump`/`Branch`, a symbol name (→ relocation) when `flow` is `Call`. `SymbolVec` takes one-or-more integers. `None` takes nothing.

---

### Task 1: Operand encoders + `formats::sniff`

**Files:**
- Modify: `crates/core/src/vm/arch.rs` (add `encode_operand` + unit tests)
- Modify: `crates/core/src/formats/mod.rs` (add `ContainerKind`, `sniff`)
- Create: `crates/core/tests/operand_codec.rs` (property test: encode vs live core decode)

**Interfaces:**
- Consumes: `Operand`, `Core`, `TestArch` (test-only), format magics.
- Produces: `encode_operand`, `ContainerKind`, `sniff` per the header block.

- [ ] **Step 1: Write the failing tests**

Unit tests appended to `arch.rs`'s existing test module:

```rust
    #[test]
    fn encode_operand_matches_wire_format() {
        use super::encode_operand;
        assert_eq!(encode_operand(&Operand::None).unwrap(), Vec::<u8>::new());
        assert_eq!(encode_operand(&Operand::I8(-3)).unwrap(), vec![0xFD]);
        assert_eq!(encode_operand(&Operand::I32(-6)).unwrap(), vec![0xFA, 0xFF, 0xFF, 0xFF]);
        assert_eq!(encode_operand(&Operand::Symbols(vec![1])).unwrap(), vec![0x81]);
        assert_eq!(
            encode_operand(&Operand::Symbols(vec![3, 0x7F, 0])).unwrap(),
            vec![0x03, 0x7F, 0x80]
        );
        assert!(encode_operand(&Operand::Symbols(vec![])).is_err());
        assert!(encode_operand(&Operand::Symbols(vec![0x80])).is_err());
    }
```

Wait — check the multi-symbol encoding against Appendix A: high bit marks the LAST element. `[3, 0x7F, 0]` → bytes `0x03` (more follow), `0x7F` (more follow), `0x80` (payload 0, last). Correct as written.

`crates/core/tests/format_roundtrips.rs`-style sniff test — add to `crates/core/src/formats/mod.rs` a test module:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sniff_recognizes_all_containers_and_rejects_noise() {
        assert!(matches!(sniff(b"MO\x01rest"), Some(ContainerKind::Object)));
        assert!(matches!(sniff(b"MX\x01rest"), Some(ContainerKind::Executable)));
        assert!(matches!(sniff(b"MT\x01rest"), Some(ContainerKind::TapeBlock)));
        assert!(sniff(b"MZ\x01").is_none());
        assert!(sniff(b"MO").is_none()); // too short
        assert!(sniff(b"MO\x02xx").is_none()); // wrong epoch
    }
}
```

Property test `crates/core/tests/operand_codec.rs` — encode with the new encoder, decode with the LIVE core fetch machinery (this is the drift-guard the Plan 2b final review demanded):

```rust
//! decode(encode(x)) == x, where decode is the real sans-I/O core fetch.

use mtc_core::vm::{encode_operand, BusRequest, BusResponse, Core, CoreEvent, Operand};
use proptest::prelude::*;

// TestArch is crate-private; use a minimal local arch mirroring the operand
// kinds (the codec property only needs operand_kind + lower to accept).
struct CodecArch;
impl mtc_core::vm::Arch for CodecArch {
    fn arch_id(&self) -> u8 { 0x7E }
    fn operand_kind(&self, opcode: u8) -> Option<mtc_core::vm::OperandKind> {
        match opcode {
            0x01 => Some(mtc_core::vm::OperandKind::RelI8),
            0x02 => Some(mtc_core::vm::OperandKind::RelI32),
            0x03 => Some(mtc_core::vm::OperandKind::SymbolVec),
            _ => None,
        }
    }
    fn lower(
        &self,
        opcode: u8,
        operand: &Operand,
    ) -> Result<Vec<mtc_core::vm::MicroOp>, mtc_core::vm::Trap> {
        // Smuggle the decoded operand out through a Write micro-op stream:
        // encode a fingerprint the test can compare. Simplest: return Stop
        // and let the test capture via a thread_local? No — cleanest is to
        // panic on mismatch here, with the expected operand injected via a
        // cell. See EXPECTED below.
        EXPECTED.with(|e| {
            let expected = e.borrow();
            assert_eq!(
                (opcode, operand),
                (expected.0, &expected.1),
                "core decoded a different operand than was encoded"
            );
        });
        Ok(vec![mtc_core::vm::MicroOp::Stop])
    }
    fn is_entry_marker(&self, _byte: u8) -> bool { false }
}

thread_local! {
    static EXPECTED: std::cell::RefCell<(u8, Operand)> =
        std::cell::RefCell::new((0, Operand::None));
}

fn round_trip(opcode: u8, operand: Operand) {
    let mut code = vec![opcode];
    code.extend(encode_operand(&operand).unwrap());
    EXPECTED.with(|e| *e.borrow_mut() = (opcode, operand));
    let arch = CodecArch;
    let mut core = Core::new(&arch, 0);
    let mut ev = core.start();
    loop {
        match ev {
            CoreEvent::Request(BusRequest::CodeRead { addr }) => {
                let resp = match code.get(addr as usize) {
                    Some(&b) => BusResponse::Byte(b),
                    None => BusResponse::OutOfCode,
                };
                ev = core.resume(resp);
            }
            CoreEvent::Stopped => return, // lower's assert_eq already ran
            other => panic!("unexpected event {other:?}"),
        }
    }
}

proptest! {
    #[test]
    fn rel_i8_round_trips(v in any::<i8>()) {
        round_trip(0x01, Operand::I8(v));
    }

    #[test]
    fn rel_i32_round_trips(v in any::<i32>()) {
        round_trip(0x02, Operand::I32(v));
    }

    #[test]
    fn symbol_vec_round_trips(v in proptest::collection::vec(0u32..0x80, 1..8)) {
        round_trip(0x03, Operand::Symbols(v));
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p mtc-core` — expected: compile errors (`encode_operand`, `sniff`, `ContainerKind` missing; `Arch`/`Core`/bus types not re-exported enough — the test's `use mtc_core::vm::{...}` list defines needed public surface; add re-exports as required).

- [ ] **Step 3: Implement**

In `crates/core/src/vm/arch.rs` (above the test modules):
```rust
/// Encode an operand to its wire form (spec §5 / Appendix A). The inverse
/// of the core's fetch-time decoding — property-tested against it.
pub fn encode_operand(operand: &Operand) -> Result<Vec<u8>, &'static str> {
    Ok(match operand {
        Operand::None => Vec::new(),
        Operand::I8(v) => vec![*v as u8],
        Operand::I32(v) => v.to_le_bytes().to_vec(),
        Operand::Symbols(symbols) => {
            let Some((last, init)) = symbols.split_last() else {
                return Err("symbol vector must not be empty");
            };
            if symbols.iter().any(|&s| s > 0x7F) {
                return Err("symbol payload exceeds 7 bits");
            }
            let mut out: Vec<u8> = init.iter().map(|&s| s as u8).collect();
            out.push(*last as u8 | 0x80);
            out
        }
    })
}
```

In `crates/core/src/vm/mod.rs`: re-export `pub use arch::encode_operand;` (and ensure `Arch`, `Core`, `CoreEvent`, `BusRequest`, `BusResponse`, `MicroOp`, `Operand`, `OperandKind`, `Trap` are all publicly reachable — they are, from Plans 2a/2b; add any missing).

In `crates/core/src/formats/mod.rs`:
```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContainerKind {
    Object,
    Executable,
    TapeBlock,
}

/// Identify a container by magic (tools never dispatch on extensions —
/// spec §6).
pub fn sniff(bytes: &[u8]) -> Option<ContainerKind> {
    match bytes.get(..3)? {
        m if m == executable::MAGIC_EXECUTABLE => Some(ContainerKind::Executable),
        m if m == object::MAGIC_OBJECT => Some(ContainerKind::Object),
        m if m == tapeblock::MAGIC_TAPEBLOCK => Some(ContainerKind::TapeBlock),
        _ => None,
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p mtc-core`
Expected: all pass (incl. 3 new properties × 256 cases).

- [ ] **Step 5: Gates + commit**

Run: `cargo clippy --workspace --all-targets -- -D warnings && cargo fmt`
```bash
git add -A
git commit -m "feat(core): operand encoders property-tested against live decode; formats::sniff"
```

---

### Task 2: `asm` syntax types + `.pma` parser

**Files:**
- Create: `crates/core/src/asm/mod.rs`
- Create: `crates/core/src/asm/syntax.rs`
- Create: `crates/core/src/asm/parser.rs`
- Modify: `crates/core/src/lib.rs` (add `pub mod asm;`)

**Interfaces:**
- Consumes: `OperandKind`.
- Produces: `Flow`, `SyntaxEntry`, `RelaxPair`, `ArchSyntax`, `AsmError`, `AsmErrorKind` (public); crate-internal parser output consumed by Task 3:
  ```rust
  pub(crate) struct SourceFunction { pub name: String, pub line: usize, pub items: Vec<SourceItem> }
  pub(crate) enum SourceItem {
      Instr { line: usize, labels: Vec<String>, opcode: u8, operand: SourceOperand },
      RawByte { line: usize, labels: Vec<String>, value: u8 },
  }
  pub(crate) enum SourceOperand { None, Ints(Vec<i64>), Name(String) }
  pub(crate) fn parse(syntax: &ArchSyntax, source: &str) -> Result<Vec<SourceFunction>, AsmError>
  ```
- Parser validates: mnemonic known; operand arity/type matches the entry's `OperandKind` (`None` → no operands; `RelI8`/`RelI32` → exactly one identifier; `SymbolVec` → ≥1 integers); code before any `.func` → `OutsideFunction`; duplicate `.func` names → `DuplicateFunction`; trailing labels with no following instruction → `Syntax("label at end of function")`.

- [ ] **Step 1: Write the failing tests**

Test module at the bottom of `parser.rs`, using this fixture (shared with Tasks 3–4 — define it in `syntax.rs` under `#[cfg(test)] pub(crate) mod fixture`):

```rust
// syntax.rs fixture:
#[cfg(test)]
pub(crate) mod fixture {
    use super::*;
    use crate::vm::OperandKind;

    /// Standalone syntax for asm framework tests (independent of TestArch —
    /// the assembler never executes, it only needs kinds/sizes).
    /// nop 0x01 | stop 0x02 | wr 0x07 (SymbolVec) | jmp 0x20 far / 0x30 short |
    /// call 0x21 (far, symbol operand) | ret 0x0B | entry marker 0x0E
    pub(crate) fn test_syntax() -> ArchSyntax {
        use Flow::{Call, FallThrough as FT, Jump, Stop};
        ArchSyntax {
            entries: vec![
                SyntaxEntry { opcode: 0x01, mnemonic: "nop", operand: OperandKind::None, flow: FT },
                SyntaxEntry { opcode: 0x02, mnemonic: "stop", operand: OperandKind::None, flow: Stop },
                SyntaxEntry { opcode: 0x07, mnemonic: "wr", operand: OperandKind::SymbolVec, flow: FT },
                SyntaxEntry { opcode: 0x20, mnemonic: "jmp", operand: OperandKind::RelI32, flow: Jump },
                SyntaxEntry { opcode: 0x30, mnemonic: "jmp.s", operand: OperandKind::RelI8, flow: Jump },
                SyntaxEntry { opcode: 0x21, mnemonic: "call", operand: OperandKind::RelI32, flow: Call },
                SyntaxEntry { opcode: 0x0B, mnemonic: "ret", operand: OperandKind::None, flow: Stop },
                SyntaxEntry { opcode: 0x0E, mnemonic: "ent", operand: OperandKind::None, flow: FT },
            ],
            relax_pairs: vec![RelaxPair { far: 0x20, short: 0x30 }],
            entry_opcode: 0x0E,
        }
    }
}
```

Parser tests:

```rust
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
            SourceItem::Instr { labels, opcode, operand, .. } => {
                assert_eq!(labels, &vec!["L1".to_string()]);
                assert_eq!(*opcode, 0x01);
                assert!(matches!(operand, SourceOperand::None));
            }
            other => panic!("unexpected {other:?}"),
        }
        match &items[1] {
            SourceItem::Instr { opcode, operand, .. } => {
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
        assert!(matches!(funcs[0].items[0], SourceItem::RawByte { value: 255, .. }));
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
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p mtc-core asm` — expected: compile error.

- [ ] **Step 3: Implement**

`crates/core/src/asm/mod.rs`:
```rust
//! Arch-generic assembler/disassembler frameworks (spec §6.4). All
//! instruction knowledge arrives via [`ArchSyntax`] tables.

mod parser;
mod syntax;

pub use syntax::{ArchSyntax, Flow, RelaxPair, SyntaxEntry};

#[derive(Debug, PartialEq, Eq)]
pub struct AsmError {
    pub line: usize,
    pub kind: AsmErrorKind,
}

#[derive(Debug, PartialEq, Eq)]
pub enum AsmErrorKind {
    Syntax(&'static str),
    UnknownMnemonic(String),
    OutsideFunction,
    DuplicateFunction(String),
    DuplicateLabel(String),
    UnknownLabel(String),
    BadOperand(&'static str),
    ShortOffsetOutOfRange { target: String },
    EncodeError(&'static str),
}

impl std::fmt::Display for AsmError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "line {}: {:?}", self.line, self.kind)
    }
}

impl std::error::Error for AsmError {}
```

`crates/core/src/asm/syntax.rs`:
```rust
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
        self.relax_pairs.iter().find(|p| p.far == far).map(|p| p.short)
    }

    pub fn is_call(&self, op: u8) -> bool {
        self.by_opcode(op).is_some_and(|e| e.flow == Flow::Call)
    }
}

// [fixture module from Step 1 goes here]
```

`crates/core/src/asm/parser.rs`:
```rust
//! `.pma` text → per-function source items (spec §6.4 grammar).

use super::syntax::ArchSyntax;
use super::{AsmError, AsmErrorKind};
use crate::vm::OperandKind;

#[derive(Debug)]
pub(crate) struct SourceFunction {
    pub name: String,
    #[allow(dead_code)] // consumed by the assembler in Task 3
    pub line: usize,
    pub items: Vec<SourceItem>,
}

#[derive(Debug)]
pub(crate) enum SourceItem {
    Instr { line: usize, labels: Vec<String>, opcode: u8, operand: SourceOperand },
    RawByte { line: usize, labels: Vec<String>, value: u8 },
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

        let first_word = text.split_whitespace().next().unwrap_or("");
        if first_word == ".func" {
            if !pending_labels.is_empty() {
                return Err(err(line_no, AsmErrorKind::Syntax("label at end of function")));
            }
            let name = text[".func".len()..].trim();
            if !is_ident(name) {
                return Err(err(line_no, AsmErrorKind::Syntax("bad function name")));
            }
            if functions.iter().any(|f| f.name == name) {
                return Err(err(line_no, AsmErrorKind::DuplicateFunction(name.to_string())));
            }
            functions.push(SourceFunction { name: name.to_string(), line: line_no, items: Vec::new() });
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

        let current = functions.last_mut().ok_or(err(line_no, AsmErrorKind::OutsideFunction))?;
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
                    return Err(err(line_no, AsmErrorKind::BadOperand("takes symbol indices")));
                }
                let mut ints = Vec::with_capacity(operands.len());
                for o in &operands {
                    ints.push(o.parse::<i64>().map_err(|_| {
                        err(line_no, AsmErrorKind::BadOperand("symbol indices are integers"))
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

// [tests from Step 1]
```

In `crates/core/src/lib.rs` add `pub mod asm;`.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p mtc-core` — all pass (expect temporary `dead_code` allows only where marked).

- [ ] **Step 5: Gates + commit**

Run: `cargo clippy --workspace --all-targets -- -D warnings && cargo fmt`
```bash
git add -A
git commit -m "feat(core): asm syntax tables and .pma parser"
```

---

### Task 3: Assembler — labels, relaxation, `ObjectFile` emission

**Files:**
- Create: `crates/core/src/asm/assembler.rs`
- Modify: `crates/core/src/asm/mod.rs` (add `mod assembler; pub use assembler::assemble;`)

**Interfaces:**
- Consumes: parser output, `ArchSyntax`, `encode_operand`, `ObjectFile`/`Symbol`/`SymbolDef`/`Relocation`/`BlobDebug`.
- Produces: `pub fn assemble(syntax, arch_id, source, with_debug) -> Result<ObjectFile, AsmError>`.
- Semantics: each `.func` → one blob starting with `entry_opcode`; local labels resolve to blob-local addresses; bare relaxable jumps start SHORT and grow to far at fixpoint; explicit short mnemonics force short (out-of-range → `ShortOffsetOutOfRange`); call opcodes emit far form + 4-byte zero hole + `Relocation { blob, offset = hole position, symbol }`; call names resolve to `Defined` symbols (this file) or appended `External` symbols; `with_debug` → per-blob `BlobDebug { labels, lines }` (labels = user labels at their addresses; lines = (instruction start offset, source line)).

- [ ] **Step 1: Write the failing tests**

Test module at the bottom of `assembler.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::asm::syntax::fixture::test_syntax;
    use crate::formats::object::SymbolDef;

    fn asm(src: &str) -> crate::formats::object::ObjectFile {
        assemble(&test_syntax(), 0x7E, src, false).unwrap()
    }

    #[test]
    fn single_function_with_backward_short_jump() {
        // fixture: ent 0x0E auto | nop 0x01 | jmp relaxable 0x20/0x30
        let obj = asm(".func f\nL:      nop\n        jmp L\n        stop\n");
        // blob: [0E] [01] [30 FD] [02]  — jmp relaxed short, off = 1-4 = -3
        assert_eq!(obj.blobs, vec![vec![0x0E, 0x01, 0x30, 0xFD, 0x02]]);
        assert_eq!(obj.symbols.len(), 1);
        assert!(matches!(obj.symbols[0].def, SymbolDef::Defined { blob: 0 }));
        assert!(obj.relocations.is_empty());
    }

    #[test]
    fn forward_jump_and_growth_to_far() {
        // 130 nops between jmp and its target force far form (offset > 127).
        let mut src = String::from(".func f\n        jmp END\n");
        for _ in 0..130 {
            src.push_str("        nop\n");
        }
        src.push_str("END:    stop\n");
        let obj = asm(&src);
        let blob = &obj.blobs[0];
        assert_eq!(blob[1], 0x20); // far jmp
        let off = i32::from_le_bytes(blob[2..6].try_into().unwrap());
        // instr_end = 6; target = 6 + 130 nops = 136; off = 130
        assert_eq!(off, 130);
        assert_eq!(blob.len(), 1 + 5 + 130 + 1);
    }

    #[test]
    fn boundary_offsets_stay_short() {
        // Exactly -128 and +127 must fit i8.
        // Backward: target ent at 0? Use label on first nop (addr 1):
        // [0E][01]...125 nops...[30 xx] → instr_end = 1+125+... compute:
        // simpler: forward jump over 125 nops → off 125 (fits), stays short.
        let mut src = String::from(".func f\n        jmp END\n");
        for _ in 0..125 {
            src.push_str("        nop\n");
        }
        src.push_str("END:    stop\n");
        let obj = asm(&src);
        assert_eq!(obj.blobs[0][1], 0x30); // short
        assert_eq!(obj.blobs[0][2] as i8, 125);
    }

    #[test]
    fn relaxation_cascade_converges() {
        // Two jumps whose shortness depends on each other's size: jmp A over
        // a 124-nop gap that also contains jmp B — if B grows, A must grow.
        // Build: jmp END ; 124 nops ; jmp END2 ; END: stop ; …(130 nops)… ; END2: stop
        let mut src = String::from(".func f\n        jmp END\n");
        for _ in 0..124 {
            src.push_str("        nop\n");
        }
        src.push_str("        jmp END2\n");
        src.push_str("END:    stop\n");
        for _ in 0..130 {
            src.push_str("        nop\n");
        }
        src.push_str("END2:   stop\n");
        let obj = asm(&src);
        let blob = &obj.blobs[0];
        // inner jmp END2 must be far (>127 away); that pushes jmp END's
        // span to 124 + 5 = 129 > 127 → far as well.
        assert_eq!(blob[1], 0x20, "outer jump must have grown far");
    }

    #[test]
    fn forced_short_out_of_range_errors() {
        let mut src = String::from(".func f\n        jmp.s END\n");
        for _ in 0..130 {
            src.push_str("        nop\n");
        }
        src.push_str("END:    stop\n");
        let e = assemble(&test_syntax(), 0x7E, &src, false).unwrap_err();
        assert!(matches!(e.kind, AsmErrorKind::ShortOffsetOutOfRange { ref target } if target == "END"));
    }

    #[test]
    fn calls_emit_holes_relocations_and_externals() {
        let obj = asm(".func f\n        call g\n        call f\n        stop\n.func g\n        ret\n");
        // f blob: [0E][21 00000000][21 00000000][02]
        assert_eq!(obj.blobs[0].len(), 1 + 5 + 5 + 1);
        assert_eq!(&obj.blobs[0][2..6], &[0, 0, 0, 0]);
        assert_eq!(obj.relocations.len(), 2);
        assert_eq!(obj.relocations[0].blob, 0);
        assert_eq!(obj.relocations[0].offset, 2);
        assert_eq!(obj.relocations[1].offset, 7);
        // symbols: f Defined(0), g Defined(1); call g resolves to index 1,
        // call f to index 0 — no externals needed here.
        let g_idx = obj.relocations[0].symbol as usize;
        assert_eq!(obj.symbols[g_idx].name, "g");
        let ext = asm(".func f\n        call missing\n");
        assert!(ext.symbols.iter().any(|s| s.name == "missing" && s.def == SymbolDef::External));
    }

    #[test]
    fn wr_and_byte_and_errors() {
        let obj = asm(".func f\n        wr 1\n        .byte 200\n");
        assert_eq!(obj.blobs[0], vec![0x0E, 0x07, 0x81, 200]);

        let e = assemble(&test_syntax(), 0x7E, ".func f\n        wr 300\n", false).unwrap_err();
        assert!(matches!(e.kind, AsmErrorKind::EncodeError(_)));

        let e = assemble(&test_syntax(), 0x7E, ".func f\nL: nop\nL: nop\n", false).unwrap_err();
        assert!(matches!(e.kind, AsmErrorKind::DuplicateLabel(ref l) if l == "L"));

        let e = assemble(&test_syntax(), 0x7E, ".func f\n        jmp NOWHERE\n", false).unwrap_err();
        assert!(matches!(e.kind, AsmErrorKind::UnknownLabel(ref l) if l == "NOWHERE"));
    }

    #[test]
    fn debug_labels_and_lines_when_requested() {
        let obj = assemble(&test_syntax(), 0x7E, ".func f\nL:      nop\n        stop\n", true).unwrap();
        let dbg = obj.debug.as_ref().unwrap();
        assert_eq!(dbg[0].labels, vec![("L".to_string(), 1)]);
        assert_eq!(dbg[0].lines, vec![(1, 2), (2, 3)]); // (blob offset, source line)
        assert_eq!(obj.arch, 0x7E);
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p mtc-core assembler` — expected: compile error.

- [ ] **Step 3: Implement**

`crates/core/src/asm/assembler.rs`:
```rust
//! Two-pass assembly with short/far relaxation (spec §5, §6.2, §6.4).

use std::collections::HashMap;

use super::parser::{parse, SourceFunction, SourceItem, SourceOperand};
use super::syntax::ArchSyntax;
use super::{AsmError, AsmErrorKind};
use crate::formats::object::{BlobDebug, ObjectFile, Relocation, Symbol, SymbolDef};
use crate::vm::{encode_operand, Operand, OperandKind};

fn err(line: usize, kind: AsmErrorKind) -> AsmError {
    AsmError { line, kind }
}

/// One instruction after operand classification, before layout.
enum Slot {
    Fixed { line: usize, bytes: Vec<u8> },
    Jump { line: usize, far: u8, short: Option<u8>, forced_short: bool, target: String },
    Call { line: usize, opcode: u8, symbol: String },
}

impl Slot {
    fn size(&self, is_far: bool) -> u32 {
        match self {
            Slot::Fixed { bytes, .. } => bytes.len() as u32,
            Slot::Jump { .. } => if is_far { 5 } else { 2 },
            Slot::Call { .. } => 5,
        }
    }
}

pub fn assemble(
    syntax: &ArchSyntax,
    arch_id: u8,
    source: &str,
    with_debug: bool,
) -> Result<ObjectFile, AsmError> {
    let functions = parse(syntax, source)?;

    let mut symbols: Vec<Symbol> = functions
        .iter()
        .enumerate()
        .map(|(i, f)| Symbol { name: f.name.clone(), def: SymbolDef::Defined { blob: i as u32 } })
        .collect();
    let mut symbol_index: HashMap<String, u32> =
        symbols.iter().enumerate().map(|(i, s)| (s.name.clone(), i as u32)).collect();

    let mut blobs = Vec::with_capacity(functions.len());
    let mut relocations = Vec::new();
    let mut debug = with_debug.then(Vec::new);

    for (blob_idx, function) in functions.iter().enumerate() {
        let (blob, relocs, blob_debug) = assemble_function(
            syntax,
            function,
            blob_idx as u32,
            &mut symbols,
            &mut symbol_index,
        )?;
        blobs.push(blob);
        relocations.extend(relocs);
        if let Some(d) = debug.as_mut() {
            d.push(blob_debug);
        }
    }

    Ok(ObjectFile { arch: arch_id, symbols, blobs, relocations, debug })
}

fn assemble_function(
    syntax: &ArchSyntax,
    function: &SourceFunction,
    blob_idx: u32,
    symbols: &mut Vec<Symbol>,
    symbol_index: &mut HashMap<String, u32>,
) -> Result<(Vec<u8>, Vec<Relocation>, BlobDebug), AsmError> {
    // Pass A: classify items into slots; collect label → slot-index.
    let mut slots: Vec<Slot> = Vec::new();
    let mut label_slot: HashMap<String, usize> = HashMap::new();
    for item in &function.items {
        let (line, labels) = match item {
            SourceItem::Instr { line, labels, .. } | SourceItem::RawByte { line, labels, .. } => {
                (*line, labels)
            }
        };
        for label in labels {
            if label_slot.insert(label.clone(), slots.len()).is_some() {
                return Err(err(line, AsmErrorKind::DuplicateLabel(label.clone())));
            }
        }
        match item {
            SourceItem::RawByte { line, value, .. } => {
                slots.push(Slot::Fixed { line: *line, bytes: vec![*value] });
            }
            SourceItem::Instr { line, opcode, operand, .. } => {
                let entry = syntax.by_opcode(*opcode).expect("parser guarantees known opcode");
                match (&entry.operand, operand) {
                    (OperandKind::None, SourceOperand::None) => {
                        slots.push(Slot::Fixed { line: *line, bytes: vec![*opcode] });
                    }
                    (OperandKind::SymbolVec, SourceOperand::Ints(ints)) => {
                        let sym: Result<Vec<u32>, _> = ints
                            .iter()
                            .map(|&i| u32::try_from(i).map_err(|_| "negative symbol index"))
                            .collect();
                        let encoded = sym
                            .and_then(|s| encode_operand(&Operand::Symbols(s)))
                            .map_err(|m| err(*line, AsmErrorKind::EncodeError(m)))?;
                        let mut bytes = vec![*opcode];
                        bytes.extend(encoded);
                        slots.push(Slot::Fixed { line: *line, bytes });
                    }
                    (OperandKind::RelI8 | OperandKind::RelI32, SourceOperand::Name(name)) => {
                        if syntax.is_call(*opcode) {
                            slots.push(Slot::Call { line: *line, opcode: *opcode, symbol: name.clone() });
                        } else {
                            // Is this the short half of a pair (forced) or a
                            // bare far mnemonic (relaxable) or unpaired?
                            let far_of_short =
                                syntax.relax_pairs.iter().find(|p| p.short == *opcode);
                            if let Some(pair) = far_of_short {
                                slots.push(Slot::Jump {
                                    line: *line,
                                    far: pair.far,
                                    short: Some(*opcode),
                                    forced_short: true,
                                    target: name.clone(),
                                });
                            } else {
                                slots.push(Slot::Jump {
                                    line: *line,
                                    far: *opcode,
                                    short: syntax.short_of(*opcode),
                                    forced_short: false,
                                    target: name.clone(),
                                });
                            }
                        }
                    }
                    _ => return Err(err(*line, AsmErrorKind::BadOperand("operand kind mismatch"))),
                }
            }
        }
    }

    // Pass B: relaxation fixpoint. is_far[i] applies to Jump slots only.
    // Start short wherever a short form exists (start-short-and-grow).
    let mut is_far: Vec<bool> = slots
        .iter()
        .map(|s| match s {
            Slot::Jump { short, .. } => short.is_none(),
            _ => true, // unused for non-jumps
        })
        .collect();
    loop {
        // Layout: addresses per slot (blob starts with the implicit ent byte).
        let mut addr = 1u32;
        let mut starts = Vec::with_capacity(slots.len());
        for (i, slot) in slots.iter().enumerate() {
            starts.push(addr);
            addr += slot.size(is_far[i]);
        }
        let resolve = |name: &str, line: usize| -> Result<u32, AsmError> {
            label_slot
                .get(name)
                .map(|&i| starts[i])
                .ok_or_else(|| err(line, AsmErrorKind::UnknownLabel(name.to_string())))
        };

        let mut grew = false;
        for (i, slot) in slots.iter().enumerate() {
            if let Slot::Jump { line, target, forced_short, .. } = slot {
                if is_far[i] {
                    continue;
                }
                let end = starts[i] + slot.size(false);
                let target_addr = resolve(target, *line)?;
                let off = i64::from(target_addr) - i64::from(end);
                if i8::try_from(off).is_err() {
                    if *forced_short {
                        return Err(err(
                            *line,
                            AsmErrorKind::ShortOffsetOutOfRange { target: target.clone() },
                        ));
                    }
                    is_far[i] = true;
                    grew = true;
                }
            }
        }
        if !grew {
            // Final emit.
            let mut blob = vec![syntax.entry_opcode];
            let mut relocs = Vec::new();
            let mut lines = Vec::new();
            for (i, slot) in slots.iter().enumerate() {
                lines.push((starts[i], match slot {
                    Slot::Fixed { line, .. } | Slot::Jump { line, .. } | Slot::Call { line, .. } => {
                        *line as u32
                    }
                }));
                match slot {
                    Slot::Fixed { bytes, .. } => blob.extend_from_slice(bytes),
                    Slot::Jump { line, far, short, target, .. } => {
                        let end = starts[i] + slot.size(is_far[i]);
                        let off = i64::from(resolve(target, *line)?) - i64::from(end);
                        if is_far[i] {
                            blob.push(*far);
                            blob.extend((off as i32).to_le_bytes());
                        } else {
                            blob.push(short.expect("short exists when !is_far"));
                            blob.push((off as i8) as u8);
                        }
                    }
                    Slot::Call { opcode, symbol, .. } => {
                        blob.push(*opcode);
                        let sym_idx = *symbol_index.entry(symbol.clone()).or_insert_with(|| {
                            symbols.push(Symbol { name: symbol.clone(), def: SymbolDef::External });
                            (symbols.len() - 1) as u32
                        });
                        relocs.push(Relocation {
                            blob: blob_idx,
                            offset: (blob.len()) as u32,
                            symbol: sym_idx,
                        });
                        blob.extend([0u8; 4]);
                    }
                }
            }
            let labels = label_slot
                .iter()
                .map(|(name, &i)| (name.clone(), starts[i]))
                .collect::<Vec<_>>();
            let mut labels = labels;
            labels.sort_by_key(|(_, a)| *a);
            return Ok((blob, relocs, BlobDebug { labels, lines }));
        }
    }
}
```

Note on `starts[i]` inside the emit loop: emit recomputes `end` from the frozen layout — `blob.len()` naturally tracks `starts[i]` since sizes are final; the explicit `starts` keeps the arithmetic honest. Debug lines record instruction start offsets. Also note `wr` negative ints are rejected via the `u32::try_from` mapping to `EncodeError`.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p mtc-core` — all pass, including the cascade and boundary tests. If the cascade test fails, verify the layout loop recomputes ALL addresses after every growth round (fixpoint, not single pass).

- [ ] **Step 5: Gates + commit**

Run: `cargo clippy --workspace --all-targets -- -D warnings && cargo fmt`
```bash
git add -A
git commit -m "feat(core): two-pass assembler with jump relaxation and MO emission"
```

---

### Task 4: Disassembler + round-trip

**Files:**
- Create: `crates/core/src/asm/disassembler.rs`
- Modify: `crates/core/src/asm/mod.rs` (add `mod disassembler; pub use disassembler::{disassemble_executable, disassemble_object};`)

**Interfaces:**
- Consumes: `ArchSyntax`, `ObjectFile`, `Executable`, `OperandKind`.
- Produces: `disassemble_object`, `disassemble_executable` per the header block.
- Canonical grid: label field cols 0–7 (`{label}:` padded to 8), mnemonic cols 8–15, operand from col 16. `.func NAME` lines for each function. Object form: local labels `L{addr:04X}` for jump targets; call operands print the relocation's symbol name. Executable form: `.func func_{addr:04X}` at every entry-opcode byte; call target = containing function's synthesized name; jump targets inside the same function → `L{addr:04X}`; a jump whose target lies OUTSIDE its function → emit the whole instruction as `.byte` lines (v1 limitation, documented).
- Round-trip law (test): `assemble(disassemble_object(obj)) == obj` for assembler-produced objects (`with_debug: false` both sides; debug row of the law: `with_debug: true` on both sides compares labels only up to naming — skip, keep law to false).

- [ ] **Step 1: Write the failing tests**

Test module at the bottom of `disassembler.rs`:

```rust
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
        let exe = Executable { arch: 0x7E, entry: 0, code };
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
        let exe = Executable { arch: 0x7E, entry: 0, code };
        let text = disassemble_executable(&syntax, &exe);
        assert!(text.contains(".func func_0000"));
        assert!(text.contains(".func func_0014"));
        assert!(!text.contains("func_0002"), "operand byte must not become a function");
        assert!(text.contains("call    func_0014"));
        assert!(text.contains(".byte   1"), "unreachable padding dumps as bytes");
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p mtc-core disassembler` — expected: compile error. (Task 3 must make `assembler` module visible to this test: change `mod assembler;` to `pub(crate) use` as needed — `pub use assembler::assemble;` in mod.rs already covers it.)

- [ ] **Step 3: Implement**

`crates/core/src/asm/disassembler.rs`:
```rust
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
    Instr { mnemonic: &'static str, operand: DecodedOperand },
    Raw(u8),
}

enum DecodedOperand {
    None,
    Ints(Vec<u32>),
    RelTarget(u32), // absolute target address (same space as `addr`)
}

fn decode_stream(syntax: &ArchSyntax, code: &[u8], start: u32, end: u32) -> Vec<Decoded> {
    let mut out = Vec::new();
    let mut addr = start;
    while addr < end {
        let opcode = code[addr as usize];
        let Some(entry) = syntax.by_opcode(opcode) else {
            out.push(Decoded { addr, len: 1, body: Body::Raw(opcode) });
            addr += 1;
            continue;
        };
        let (len, operand) = match entry.operand {
            OperandKind::None => (1, DecodedOperand::None),
            OperandKind::RelI8 => {
                if addr + 2 > end {
                    out.push(Decoded { addr, len: 1, body: Body::Raw(opcode) });
                    addr += 1;
                    continue;
                }
                let off = code[(addr + 1) as usize] as i8;
                let target = (i64::from(addr) + 2 + i64::from(off)) as u32;
                (2, DecodedOperand::RelTarget(target))
            }
            OperandKind::RelI32 => {
                if addr + 5 > end {
                    out.push(Decoded { addr, len: 1, body: Body::Raw(opcode) });
                    addr += 1;
                    continue;
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
                    out.push(Decoded { addr, len: 1, body: Body::Raw(opcode) });
                    addr += 1;
                    continue;
                }
                (i - addr, DecodedOperand::Ints(symbols))
            }
        };
        out.push(Decoded { addr, len, body: Body::Instr { mnemonic: entry.mnemonic, operand } });
        addr += len;
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
        .map(|r| ((r.blob, r.offset), obj.symbols[r.symbol as usize].name.as_str()))
        .collect();

    for symbol in &obj.symbols {
        let SymbolDef::Defined { blob } = symbol.def else { continue };
        let code = &obj.blobs[blob as usize];
        out.push_str(&format!(".func {}\n", symbol.name));
        // Skip the leading entry byte if present (implied by .func).
        let start = if code.first() == Some(&syntax.entry_opcode) { 1 } else { 0 };
        let decoded = decode_stream(syntax, code, start, code.len() as u32);

        let mut targets = BTreeSet::new();
        for d in &decoded {
            if let Body::Instr { operand: DecodedOperand::RelTarget(t), mnemonic } = &d.body {
                let is_call = syntax
                    .by_mnemonic(mnemonic)
                    .is_some_and(|e| syntax.is_call(e.opcode));
                if !is_call {
                    targets.insert(*t);
                }
            }
        }

        for d in &decoded {
            let label_name = targets.contains(&d.addr).then(|| format!("L{:04X}", d.addr));
            let line = match &d.body {
                Body::Raw(b) => grid_line(label_name.as_deref(), ".byte", &b.to_string()),
                Body::Instr { mnemonic, operand } => {
                    let entry = syntax.by_mnemonic(mnemonic).unwrap();
                    let operand_text = match operand {
                        DecodedOperand::None => String::new(),
                        DecodedOperand::Ints(v) => v
                            .iter()
                            .map(u32::to_string)
                            .collect::<Vec<_>>()
                            .join(", "),
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
    let mut v = decode_stream(syntax, code, addr, code.len() as u32);
    match v.drain(..).next() {
        Some(d @ Decoded { body: Body::Instr { .. }, .. }) => Some(d),
        _ => None,
    }
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
        let Body::Instr { mnemonic, operand } = &d.body else { unreachable!() };
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
            if let Body::Instr { mnemonic, operand: DecodedOperand::RelTarget(t) } = &d.body {
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
                    let Body::Instr { mnemonic, operand } = &d.body else { unreachable!() };
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
                        DecodedOperand::Ints(v) => Some(
                            v.iter().map(u32::to_string).collect::<Vec<_>>().join(", "),
                        ),
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
                            out.push_str(&grid_line(label_name.as_deref(), mnemonic, &operand_text));
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
```

(If the borrow checker objects to the `continue`-inside-match emit shape in `disassemble_executable`, restructure that arm into an early `if` that emits fallback lines and `continue`s the outer loop before the normal-path formatting — behavior as specified in the tests.)

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p mtc-core` — all pass, round-trip law included.

- [ ] **Step 5: Gates + commit**

Run: `cargo clippy --workspace --all-targets -- -D warnings && cargo fmt`
```bash
git add -A
git commit -m "feat(core): disassembler with canonical grid and object round-trip"
```

---

### Task 5: PM-1 syntax + public asm API + spec sample end-to-end

**Files:**
- Create: `crates/post-machine/src/asm/mod.rs`
- Modify: `crates/post-machine/src/lib.rs` (add `pub mod asm;`)
- Create: `crates/post-machine/tests/asm_programs.rs`

**Interfaces:**
- Consumes: everything above + `Pm1`/`opcodes` + `Machine`.
- Produces: `pm1_syntax()`, `assemble`, `disassemble_object`, `disassemble_executable` (thin wrappers, per the header block).

- [ ] **Step 1: Write the implementation and tests together (wrappers are trivial; the tests are the substance)**

`crates/post-machine/src/asm/mod.rs`:
```rust
//! PM-1 assembly: the spec §5 mnemonic table bound to the core framework.

use mtc_core::asm::{ArchSyntax, AsmError, Flow, RelaxPair, SyntaxEntry};
use mtc_core::formats::executable::Executable;
use mtc_core::formats::object::ObjectFile;
use mtc_core::formats::ARCH_PM1;
use mtc_core::vm::OperandKind;

use crate::arch::opcodes::*;

pub fn pm1_syntax() -> ArchSyntax {
    use Flow::{Branch, Call as CallF, FallThrough as FT, Jump, Stop};
    use OperandKind::{None as N, RelI32, RelI8, SymbolVec};
    ArchSyntax {
        entries: vec![
            SyntaxEntry { opcode: NOP, mnemonic: "nop", operand: N, flow: FT },
            SyntaxEntry { opcode: STP, mnemonic: "stp", operand: N, flow: Stop },
            SyntaxEntry { opcode: HLT, mnemonic: "hlt", operand: N, flow: Stop },
            SyntaxEntry { opcode: LFT, mnemonic: "lft", operand: N, flow: FT },
            SyntaxEntry { opcode: RGT, mnemonic: "rgt", operand: N, flow: FT },
            SyntaxEntry { opcode: WR, mnemonic: "wr", operand: SymbolVec, flow: FT },
            SyntaxEntry { opcode: JMP, mnemonic: "jmp", operand: RelI32, flow: Jump },
            SyntaxEntry { opcode: JM, mnemonic: "jm", operand: RelI32, flow: Branch },
            SyntaxEntry { opcode: JNM, mnemonic: "jnm", operand: RelI32, flow: Branch },
            SyntaxEntry { opcode: CALL, mnemonic: "call", operand: RelI32, flow: CallF },
            SyntaxEntry { opcode: RET, mnemonic: "ret", operand: N, flow: Stop },
            SyntaxEntry { opcode: ENT, mnemonic: "ent", operand: N, flow: FT },
            SyntaxEntry { opcode: BRK, mnemonic: "brk", operand: N, flow: FT },
            SyntaxEntry { opcode: JMP_S, mnemonic: "jmp.s", operand: RelI8, flow: Jump },
            SyntaxEntry { opcode: JM_S, mnemonic: "jm.s", operand: RelI8, flow: Branch },
            SyntaxEntry { opcode: JNM_S, mnemonic: "jnm.s", operand: RelI8, flow: Branch },
            SyntaxEntry { opcode: CALL_S, mnemonic: "call.s", operand: RelI8, flow: CallF },
        ],
        relax_pairs: vec![
            RelaxPair { far: JMP, short: JMP_S },
            RelaxPair { far: JM, short: JM_S },
            RelaxPair { far: JNM, short: JNM_S },
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
```

`crates/post-machine/tests/asm_programs.rs`:
```rust
//! PM-1 assembly end-to-end: the spec §6.4 sample, byte-exact, and an
//! assembled program actually running on the Machine.

use mtc_core::formats::object::SymbolDef;
use mtc_core::vm::{ArchRegistry, InfiniteTape, Machine, Outcome, RunOptions};
use mtc_post_machine::arch::opcodes::*;
use mtc_post_machine::arch::Pm1;
use mtc_post_machine::asm::{assemble, disassemble_object};

/// The spec §6.4 sample, verbatim.
const SPEC_SAMPLE: &str = "\
.func goToEnd                   ; emits ent, defines symbol
L1:     rgt
        jm      L1              ; assembler picks jm.s automatically
        lft
        ret

.func main
        call    goToEnd         ; width decided at link time
        rgt
        wr      1               ; mark
        stp
";

#[test]
fn spec_sample_assembles_byte_exact() {
    let obj = assemble(SPEC_SAMPLE, false).unwrap();
    assert_eq!(obj.arch, mtc_core::formats::ARCH_PM1);
    // goToEnd: ent, rgt, jm.s -3, lft, ret
    assert_eq!(obj.blobs[0], vec![ENT, RGT, JM_S, 0xFD, LFT, RET]);
    // main: ent, call <hole>, rgt, wr 1, stp
    assert_eq!(obj.blobs[1], vec![ENT, CALL, 0, 0, 0, 0, RGT, WR, 0x81, STP]);
    assert_eq!(obj.relocations.len(), 1);
    assert_eq!(obj.relocations[0].blob, 1);
    assert_eq!(obj.relocations[0].offset, 2);
    let sym = &obj.symbols[obj.relocations[0].symbol as usize];
    assert_eq!(sym.name, "goToEnd");
    assert!(matches!(sym.def, SymbolDef::Defined { blob: 0 }));
}

#[test]
fn spec_sample_round_trips_through_disassembly() {
    let obj1 = assemble(SPEC_SAMPLE, false).unwrap();
    let text = disassemble_object(&obj1);
    let obj2 = assemble(&text, false).unwrap();
    assert_eq!(obj1, obj2);
}

#[test]
fn assembled_function_runs_on_the_machine() {
    // Self-contained (no calls): goToEnd's body as main.
    let src = "\
.func main
L:      rgt
        jm      L
        stp
";
    let obj = assemble(src, false).unwrap();
    assert!(obj.relocations.is_empty());
    // A single self-contained blob IS runnable code: entry 0 is its ent.
    let arch = Pm1;
    let machine = Machine::with_arch(&arch, obj.blobs[0].clone(), 0).unwrap();
    let mut tape = InfiniteTape::from_cells([true, true, true], 0, 0);
    let mut registry = ArchRegistry::new();
    registry.register(Box::new(Pm1));
    let result = machine.run(&mut tape, RunOptions::default());
    assert_eq!(result.outcome, Outcome::Stopped);
    assert_eq!(tape.head(), 3); // stopped on the first blank — assembled, not hand-built
}

#[test]
fn forced_short_and_explicit_far_forms() {
    // jm.s forced short — fits, so identical bytes to relaxed jm.
    let short = assemble(".func f\nL:      rgt\n        jm.s    L\n", false).unwrap();
    let relaxed = assemble(".func f\nL:      rgt\n        jm      L\n", false).unwrap();
    assert_eq!(short.blobs, relaxed.blobs);
}

#[test]
fn errors_carry_lines() {
    let e = assemble(".func f\n        wr\n", false).unwrap_err();
    assert_eq!(e.line, 2);
}
```

- [ ] **Step 2: Run RED then GREEN**

Run: `cargo test -p mtc-post-machine --test asm_programs`
Expected first: compile errors until the module lands; then all green. If `spec_sample_assembles_byte_exact` fails on the `jm.s` offset, re-derive: blob layout `[0]ent [1]rgt [2..4]jm.s [4]lft [5]ret`, jm.s instr_end 4, target 1 → `-3` = `0xFD`.

- [ ] **Step 3: Full gates**

Run: `cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --check`

- [ ] **Step 4: Commit**

```bash
git add -A
git commit -m "feat(post-machine): PM-1 assembly syntax, public asm API, spec sample end-to-end"
```

---

## Self-Review Notes

- **Spec coverage:** §6.4 grammar + canonical grid + dis-of-both-binaries + round-trip ✓ (Tasks 2–5); §5 encodings via shared `encode_operand` property-tested against the live core ✓ (Task 1, the Plan 2b deferral); §6.2 blob/symbol/reloc/debug emission ✓ (Task 3); relaxation incl. boundary and cascade tests ✓ (Task 3, spec §11); `formats::sniff` ✓ (Plan 1 deferral). NOT here: linker (Plan 4) — `call` holes stay zero and `.pmx` disassembly of *linked* calls prints `L`-style targets or synthesized function names; CLI (`pmt asm/dis`, Plan 7).
- **Type consistency:** every `SyntaxEntry` construction site (fixture, PM-1) carries a `flow`; `ArchSyntax::is_call` derives from `Flow::Call`; `AsmErrorKind` variants match between interface block, parser, and assembler; the fixture's opcodes deliberately differ from both TestArch and PM-1 (framework must not care).
- **Known limitations, on record:** executable function discovery is recursive descent from `entry` + call targets — exact for v1 (no indirect control flow, no data-in-code); unreachable bytes dump as `.byte`; cross-region jumps fall back to `.byte` and jump-targets-that-are-roots joins discovery when Plan 6's tail-call lands; explicit far mnemonics don't exist (bare = relaxable, `.s` = forced short) — matching "assembler picks the width" (spec §6.4).
- **Arithmetic spot-checks:** fixture jmp far=5 bytes/short=2; `L: nop / jmp L` → off `1−4=−3`; forward 130 nops → far off `130`; spec sample `jm.s` off `0xFD`; call hole at blob offset 2. All hand-derived twice.
