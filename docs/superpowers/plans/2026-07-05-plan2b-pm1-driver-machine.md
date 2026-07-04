# Plan 2b/7: PM-1 Arch Module, Sync Driver, Loader/Machine, First Programs

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The VM becomes a whole machine: the real PM-1 instruction set (`mtc-post-machine`), the synchronous driver with tact/wait-state accounting (`mtc-core`), the loader + `Machine` facade, the `InfiniteTape` ↔ `TapeSnapshot` bridge — ending with hand-assembled PM-1 programs executing end-to-end and the spec's tact arithmetic pinned by tests.

**Architecture:** Spec §4.3–§4.5 + §5. The driver is the sans-I/O core's counterpart: it owns code image, return stack, and the device, answers `BusRequest`s, and does all tact bookkeeping (core tacts vs stall tacts, wait states). `Machine` wires `Executable` → arch lookup → entry validation → per-run `Core` + `ReturnStack`. PM-1 is pure table: opcode → operand kind → micro-ops.

**Tech Stack:** Rust stable, edition 2024, crates `mtc-core` + `mtc-post-machine`. No new dependencies.

**Spec:** `docs/superpowers/specs/2026-07-04-post-machine-toolchain-design.md` §4.3 (loading), §4.4 (tacts + wait states), §4.5 (traps), §5 (ISA). Plan 2a's "Interfaces Established" + amended "Protocol rules" are the base contract.

## Global Constraints

- PM-1 opcode table EXACTLY per spec §5: `0x01 nop, 0x02 stp, 0x03 hlt, 0x04 lft, 0x05 rgt, 0x06 wr(SymbolVec), 0x08 jmp, 0x09 jm, 0x0A jnm, 0x0B call (RelI32), 0x0C ret, 0x0D ent, 0x0E brk`, short forms `short = far | 0x10` (`0x18 jmp.s, 0x19 jm.s, 0x1A jnm.s, 0x1B call.s`, RelI8); `0x00`, `0x07`, and everything else invalid. Entry marker byte = `0x0D`.
- PM-1 match index is `1` (`LatchMatch(1)` after every tape micro-op); `wr` takes exactly one symbol element (more → `BadOperand`).
- Tact rules (spec §4.4): every code-bus byte answered = 1 core tact (fetch bytes AND the call's ent-verification read); each successful stack push/pop = 1 core tact; execute base = 1 core tact, charged at `CoreEvent::Step` (terminal `stp`/`hlt` never reach Step — they cost only their fetch); device commands cost the **tape profile** as stall tacts (electronic default `move/read/write = 1`). Failed responses (`OutOfCode`, `StackFull`, `StackEmpty`, `Fault`) charge nothing.
- Worked numbers that MUST hold as tests: `rgt` = 2 core + 2 stall; `jm.s` = 3 core; `call` far = 8 core; `call.s` = 5 core; `wr` = 3 core + 2 stall (electronic).
- Initial MF is latched by `Machine::run` from the device BEFORE the first instruction (spec §4.3 step 4), tact-free (loading, not execution).
- Loader validation (spec §4.3): unknown arch byte → `LoadError::UnknownArch`; `code[entry]` not an entry marker → `LoadError::EntryNotEntryMarker` (Executable::from_bytes already guarantees `entry < code.len()`).
- `ARCH_PM1` (= `0x01`) comes from `mtc_core::formats` — do not redefine it.
- Quality gates on every commit: `cargo test --workspace` green, `cargo clippy --workspace --all-targets -- -D warnings` clean, `cargo fmt --check` clean; no attribution footers.
- Commit policy: per-task commits pre-approved in this repo; never push.

## Interfaces Established by This Plan

```rust
// mtc_core::vm::driver
pub struct ReturnStack { /* entries: Vec<u32>, capacity: usize */ }
impl ReturnStack {
    pub fn new(capacity: usize) -> Self;
    pub fn depth(&self) -> usize;
    pub fn entries(&self) -> &[u32];
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TactProfile { pub move_cost: u32, pub read_cost: u32, pub write_cost: u32 }
impl TactProfile { pub const ELECTRONIC: TactProfile = TactProfile { move_cost: 1, read_cost: 1, write_cost: 1 }; }
#[derive(Debug, Clone, Copy, Default)]
pub struct RunLimits { pub max_steps: Option<u64>, pub max_tacts: Option<u64> }
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RunStats { pub steps: u64, pub core_tacts: u64, pub stall_tacts: u64 }
impl RunStats { pub fn total_tacts(&self) -> u64; }
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome { Stopped, Halted, Trapped(Trap) }
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RunResult { pub outcome: Outcome, pub stats: RunStats }
pub fn run(core: &mut Core, code: &[u8], stack: &mut ReturnStack,
           device: &mut dyn Tape, profile: TactProfile, limits: RunLimits) -> RunResult;

// mtc_core::vm::machine
pub struct ArchRegistry { /* Vec<Box<dyn Arch>> */ }
impl ArchRegistry { pub fn new() -> Self; pub fn register(&mut self, arch: Box<dyn Arch>); pub fn get(&self, id: u8) -> Option<&dyn Arch>; }
#[derive(Debug, PartialEq, Eq)]
pub enum LoadError { UnknownArch(u8), EntryNotEntryMarker { at: u32 } }
#[derive(Debug, Clone, Copy)]
pub struct RunOptions { pub stack_depth: usize, pub profile: TactProfile, pub limits: RunLimits }
impl Default for RunOptions { /* 1024, ELECTRONIC, RunLimits::default() */ }
pub struct Machine<'a> { /* arch: &'a dyn Arch, code: Vec<u8>, entry: u32 */ }
impl<'a> Machine<'a> {
    pub fn with_arch(arch: &'a dyn Arch, code: Vec<u8>, entry: u32) -> Result<Machine<'a>, LoadError>;
    pub fn from_executable(exe: &Executable, registry: &'a ArchRegistry) -> Result<Machine<'a>, LoadError>;
    pub fn run(&self, device: &mut dyn Tape, opts: RunOptions) -> RunResult; // latches MF tact-free, then drives
    pub fn entry(&self) -> u32; pub fn code(&self) -> &[u8];
}

// mtc_core::vm::devices (additions on InfiniteTape)
impl InfiniteTape {
    pub fn from_snapshot(s: &TapeSnapshot) -> Result<InfiniteTape, DeviceFault>; // cells must be 0/1
    pub fn to_snapshot(&self) -> TapeSnapshot; // span covers marks ∪ head; blank tape → single cell at head
}

// mtc_post_machine::arch
pub struct Pm1;
impl Arch for Pm1 { /* arch_id() == mtc_core::formats::ARCH_PM1 */ }
pub mod opcodes {
    pub const NOP: u8 = 0x01;  pub const STP: u8 = 0x02;  pub const HLT: u8 = 0x03;
    pub const LFT: u8 = 0x04;  pub const RGT: u8 = 0x05;  pub const WR: u8 = 0x06;
    pub const JMP: u8 = 0x08;  pub const JM: u8 = 0x09;   pub const JNM: u8 = 0x0A;
    pub const CALL: u8 = 0x0B; pub const RET: u8 = 0x0C;  pub const ENT: u8 = 0x0D;
    pub const BRK: u8 = 0x0E;
    pub const JMP_S: u8 = 0x18; pub const JM_S: u8 = 0x19;
    pub const JNM_S: u8 = 0x1A; pub const CALL_S: u8 = 0x1B;
}
```

---

### Task 1: PM-1 architecture module

**Files:**
- Create: `crates/post-machine/src/arch/mod.rs`
- Modify: `crates/post-machine/src/lib.rs` (add `pub mod arch;`)

**Interfaces:**
- Consumes: `mtc_core::vm::{Arch, MicroOp, Operand, OperandKind, Trap}`, `mtc_core::formats::ARCH_PM1`.
- Produces: `Pm1`, `opcodes` (exact constants above). Plan 3's assembler will import `opcodes` — the constants are a public contract.

- [ ] **Step 1: Write the failing tests**

Test module at the bottom of `arch/mod.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::opcodes::*;
    use super::*;
    use mtc_core::vm::{MicroOp, Operand, OperandKind};

    #[test]
    fn operand_kind_table_matches_spec() {
        let a = Pm1;
        for op in [NOP, STP, HLT, LFT, RGT, RET, ENT, BRK] {
            assert!(matches!(a.operand_kind(op), Some(OperandKind::None)), "opcode {op:#04x}");
        }
        assert!(matches!(a.operand_kind(WR), Some(OperandKind::SymbolVec)));
        for op in [JMP, JM, JNM, CALL] {
            assert!(matches!(a.operand_kind(op), Some(OperandKind::RelI32)), "opcode {op:#04x}");
        }
        for op in [JMP_S, JM_S, JNM_S, CALL_S] {
            assert!(matches!(a.operand_kind(op), Some(OperandKind::RelI8)), "opcode {op:#04x}");
            assert_eq!(op, (op - 0x10) | 0x10); // short = far | 0x10 (self-check of constants)
        }
        for invalid in [0x00u8, 0x07, 0x0F, 0x10, 0x17, 0x1C, 0x80, 0xFF] {
            assert!(a.operand_kind(invalid).is_none(), "opcode {invalid:#04x} must be invalid");
        }
    }

    #[test]
    fn short_form_rule_holds_for_constants() {
        assert_eq!(JMP_S, JMP | 0x10);
        assert_eq!(JM_S, JM | 0x10);
        assert_eq!(JNM_S, JNM | 0x10);
        assert_eq!(CALL_S, CALL | 0x10);
    }

    #[test]
    fn lowerings_match_semantics() {
        let a = Pm1;
        assert_eq!(a.lower(LFT, &Operand::None).unwrap(),
                   vec![MicroOp::MoveLeft, MicroOp::LatchMatch(1)]);
        assert_eq!(a.lower(RGT, &Operand::None).unwrap(),
                   vec![MicroOp::MoveRight, MicroOp::LatchMatch(1)]);
        assert_eq!(a.lower(WR, &Operand::Symbols(vec![1])).unwrap(),
                   vec![MicroOp::Write(1), MicroOp::LatchMatch(1)]);
        assert_eq!(a.lower(JMP, &Operand::I32(-6)).unwrap(), vec![MicroOp::JumpRel(-6)]);
        assert_eq!(a.lower(JM_S, &Operand::I8(-3)).unwrap(),
                   vec![MicroOp::JumpRelIf { off: -3, when_match: true }]);
        assert_eq!(a.lower(JNM, &Operand::I32(9)).unwrap(),
                   vec![MicroOp::JumpRelIf { off: 9, when_match: false }]);
        assert_eq!(a.lower(CALL_S, &Operand::I8(1)).unwrap(), vec![MicroOp::Call(1)]);
        assert_eq!(a.lower(RET, &Operand::None).unwrap(), vec![MicroOp::Ret]);
        assert_eq!(a.lower(STP, &Operand::None).unwrap(), vec![MicroOp::Stop]);
        assert_eq!(a.lower(HLT, &Operand::None).unwrap(), vec![MicroOp::Halt]);
        assert_eq!(a.lower(ENT, &Operand::None).unwrap(), vec![MicroOp::Nop]);
        assert_eq!(a.lower(BRK, &Operand::None).unwrap(), vec![MicroOp::Brk]);
        assert_eq!(a.lower(NOP, &Operand::None).unwrap(), vec![MicroOp::Nop]);
    }

    #[test]
    fn wr_requires_exactly_one_symbol() {
        let a = Pm1;
        assert!(a.lower(WR, &Operand::Symbols(vec![0])).is_ok());
        assert!(a.lower(WR, &Operand::Symbols(vec![1, 2])).is_err());
        assert!(a.lower(WR, &Operand::Symbols(vec![])).is_err());
    }

    #[test]
    fn identity() {
        let a = Pm1;
        assert_eq!(a.arch_id(), mtc_core::formats::ARCH_PM1);
        assert!(a.is_entry_marker(ENT));
        assert!(!a.is_entry_marker(NOP));
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p mtc-post-machine`
Expected: compile error — module doesn't exist.

- [ ] **Step 3: Implement**

`crates/post-machine/src/arch/mod.rs`:
```rust
//! PM-1: the Post-machine instruction set (spec §5), as an arch module
//! for the mtc-core VM. Pure table — no state.

use mtc_core::vm::{Arch, MicroOp, Operand, OperandKind, Trap};

pub mod opcodes {
    pub const NOP: u8 = 0x01;
    pub const STP: u8 = 0x02;
    pub const HLT: u8 = 0x03;
    pub const LFT: u8 = 0x04;
    pub const RGT: u8 = 0x05;
    pub const WR: u8 = 0x06;
    pub const JMP: u8 = 0x08;
    pub const JM: u8 = 0x09;
    pub const JNM: u8 = 0x0A;
    pub const CALL: u8 = 0x0B;
    pub const RET: u8 = 0x0C;
    pub const ENT: u8 = 0x0D;
    pub const BRK: u8 = 0x0E;
    // Short forms: far | 0x10 (spec §5).
    pub const JMP_S: u8 = 0x18;
    pub const JM_S: u8 = 0x19;
    pub const JNM_S: u8 = 0x1A;
    pub const CALL_S: u8 = 0x1B;
}

use opcodes::*;

/// PM-1 matches against the mark index (spec §4.1).
const MARK: u32 = 1;

pub struct Pm1;

impl Arch for Pm1 {
    fn arch_id(&self) -> u8 {
        mtc_core::formats::ARCH_PM1
    }

    fn operand_kind(&self, opcode: u8) -> Option<OperandKind> {
        match opcode {
            NOP | STP | HLT | LFT | RGT | RET | ENT | BRK => Some(OperandKind::None),
            WR => Some(OperandKind::SymbolVec),
            JMP | JM | JNM | CALL => Some(OperandKind::RelI32),
            JMP_S | JM_S | JNM_S | CALL_S => Some(OperandKind::RelI8),
            _ => None,
        }
    }

    fn lower(&self, opcode: u8, operand: &Operand) -> Result<Vec<MicroOp>, Trap> {
        let off32 = |o: &Operand| match o {
            Operand::I32(v) => Ok(*v),
            _ => Err(Trap::BadOperand { at: 0 }),
        };
        let off8 = |o: &Operand| match o {
            Operand::I8(v) => Ok(i32::from(*v)),
            _ => Err(Trap::BadOperand { at: 0 }),
        };
        Ok(match opcode {
            NOP | ENT => vec![MicroOp::Nop],
            STP => vec![MicroOp::Stop],
            HLT => vec![MicroOp::Halt],
            BRK => vec![MicroOp::Brk],
            LFT => vec![MicroOp::MoveLeft, MicroOp::LatchMatch(MARK)],
            RGT => vec![MicroOp::MoveRight, MicroOp::LatchMatch(MARK)],
            WR => match operand {
                Operand::Symbols(s) if s.len() == 1 => {
                    vec![MicroOp::Write(s[0]), MicroOp::LatchMatch(MARK)]
                }
                _ => return Err(Trap::BadOperand { at: 0 }),
            },
            JMP => vec![MicroOp::JumpRel(off32(operand)?)],
            JMP_S => vec![MicroOp::JumpRel(off8(operand)?)],
            JM => vec![MicroOp::JumpRelIf { off: off32(operand)?, when_match: true }],
            JM_S => vec![MicroOp::JumpRelIf { off: off8(operand)?, when_match: true }],
            JNM => vec![MicroOp::JumpRelIf { off: off32(operand)?, when_match: false }],
            JNM_S => vec![MicroOp::JumpRelIf { off: off8(operand)?, when_match: false }],
            CALL => vec![MicroOp::Call(off32(operand)?)],
            CALL_S => vec![MicroOp::Call(off8(operand)?)],
            RET => vec![MicroOp::Ret],
            _ => return Err(Trap::InvalidOpcode { opcode, at: 0 }),
        })
    }

    fn is_entry_marker(&self, byte: u8) -> bool {
        byte == ENT
    }
}
```

In `crates/post-machine/src/lib.rs`:
```rust
//! Post-machine toolchain: PM-1 arch module, `.pmc` compiler, stdlib, `pmt`.

pub mod arch;
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --workspace`
Expected: all pass (5 new).

- [ ] **Step 5: Gates + commit**

Run: `cargo clippy --workspace --all-targets -- -D warnings && cargo fmt`
```bash
git add -A
git commit -m "feat(post-machine): PM-1 arch module with spec §5 opcode table"
```

---

### Task 2: Sync driver with tact accounting

**Files:**
- Create: `crates/core/src/vm/driver.rs`
- Modify: `crates/core/src/vm/mod.rs` (add `pub mod driver;` + re-exports `pub use driver::{run, Outcome, ReturnStack, RunLimits, RunResult, RunStats, TactProfile};`)

**Interfaces:**
- Consumes: `Core`, bus types, `Tape`, `Trap` (Plan 2a); `TestArch` for tests.
- Produces: the `driver` block from the header, exactly. Accounting rules (Global Constraints) are the semantics; limits: after each accounting event, if `max_tacts` exceeded → `Outcome::Trapped(Trap::TactLimit)`; at each `Step`, `steps += 1`, `core_tacts += 1`, then if `steps` exceeds `max_steps` → `Trapped(Trap::StepLimit)`.

- [ ] **Step 1: Write the failing tests**

Test module at the bottom of `driver.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::vm::arch::test_arch::TestArch;
    use crate::vm::devices::InfiniteTape;
    use crate::vm::trap::Trap;
    use crate::vm::Core;

    // TestArch: 0x01 nop | 0x02 stop | 0x03 halt | 0x05 left+latch |
    // 0x06 right+latch | 0x07 wr(vec)+latch | 0x08 jmp rel8 |
    // 0x0A call rel32 | 0x0B ret | 0x0E entry(Nop)

    fn drive(code: &[u8], limits: RunLimits, profile: TactProfile) -> (RunResult, InfiniteTape) {
        let arch = TestArch;
        let mut core = Core::new(&arch, 0);
        let mut stack = ReturnStack::new(4);
        let mut tape = InfiniteTape::new();
        let result = run(&mut core, code, &mut stack, &mut tape, profile, limits);
        (result, tape)
    }

    #[test]
    fn nop_stop_costs_fetch_and_exec_only() {
        // nop: fetch 1 + exec 1; stop: fetch 1 (terminal, no Step)
        let (r, _) = drive(&[0x01, 0x02], RunLimits::default(), TactProfile::ELECTRONIC);
        assert_eq!(r.outcome, Outcome::Stopped);
        assert_eq!(r.stats, RunStats { steps: 1, core_tacts: 3, stall_tacts: 0 });
    }

    #[test]
    fn tape_instruction_splits_core_and_stall() {
        // right: fetch 1 + exec 1 core; move 1 + latch-read 1 stall; then stop 1
        let (r, tape) = drive(&[0x06, 0x02], RunLimits::default(), TactProfile::ELECTRONIC);
        assert_eq!(r.outcome, Outcome::Stopped);
        assert_eq!(r.stats, RunStats { steps: 1, core_tacts: 3, stall_tacts: 2 });
        assert_eq!(tape.head(), 1);
    }

    #[test]
    fn mechanical_profile_inflates_stall_only() {
        let mech = TactProfile { move_cost: 50, read_cost: 5, write_cost: 10 };
        let (r, _) = drive(&[0x06, 0x02], RunLimits::default(), mech);
        assert_eq!(r.stats, RunStats { steps: 1, core_tacts: 3, stall_tacts: 55 });
    }

    #[test]
    fn write_pays_write_then_latch_read() {
        // wr(1): fetch 2 + exec 1 core; write 1 + read 1 stall; stop 1
        let (r, tape) = drive(&[0x07, 0x81, 0x02], RunLimits::default(), TactProfile::ELECTRONIC);
        assert_eq!(r.stats, RunStats { steps: 1, core_tacts: 4, stall_tacts: 2 });
        assert_eq!(tape.marked_cells(), vec![0]);
    }

    #[test]
    fn call_costs_eight_with_rel32() {
        // [0]=call +1 (target 6 = entry), [5]=stop, [6]=entry, [7]=ret
        // call: fetch 5 + ent-read 1 + push 1 + exec 1 = 8 core (spec §4.4)
        // entry(Nop): 2; ret: fetch 1 + pop 1 + exec 1 = 3; stop: 1
        let code = [0x0A, 0x01, 0x00, 0x00, 0x00, 0x02, 0x0E, 0x0B];
        let (r, _) = drive(&code, RunLimits::default(), TactProfile::ELECTRONIC);
        assert_eq!(r.outcome, Outcome::Stopped);
        assert_eq!(r.stats, RunStats { steps: 3, core_tacts: 14, stall_tacts: 0 });
    }

    #[test]
    fn step_limit_traps() {
        // jmp rel8 -2: instr_end 2, target 0 → infinite loop
        let code = [0x08, 0xFE];
        let limits = RunLimits { max_steps: Some(10), max_tacts: None };
        let (r, _) = drive(&code, limits, TactProfile::ELECTRONIC);
        assert_eq!(r.outcome, Outcome::Trapped(Trap::StepLimit));
        assert_eq!(r.stats.steps, 10);
    }

    #[test]
    fn tact_limit_traps() {
        let code = [0x08, 0xFE];
        let limits = RunLimits { max_steps: None, max_tacts: Some(25) };
        let (r, _) = drive(&code, limits, TactProfile::ELECTRONIC);
        assert_eq!(r.outcome, Outcome::Trapped(Trap::TactLimit));
        assert!(r.stats.total_tacts() >= 25);
    }

    #[test]
    fn stack_overflow_surfaces_as_trap() {
        // call rel32 -6 → target 0 (this instruction) = infinite recursion
        let code = [0x0E, 0x0B, 0xFA, 0xFF, 0xFF, 0xFF, 0x02];
        // entry at 0 is 0x0E (TestArch entry marker), call at 1, instr_end 6, off -6 → 0
        let (r, _) = drive(&code, RunLimits::default(), TactProfile::ELECTRONIC);
        assert_eq!(r.outcome, Outcome::Trapped(Trap::StackOverflow)); // capacity 4
    }

    #[test]
    fn halt_and_device_state_reported() {
        let (r, _) = drive(&[0x03], RunLimits::default(), TactProfile::ELECTRONIC);
        assert_eq!(r.outcome, Outcome::Halted);
        assert_eq!(r.stats, RunStats { steps: 0, core_tacts: 1, stall_tacts: 0 });
    }

    #[test]
    fn return_stack_reports_depth_and_entries() {
        let mut s = ReturnStack::new(2);
        assert_eq!(s.depth(), 0);
        assert!(s.push(7));
        assert!(s.push(9));
        assert!(!s.push(11)); // full
        assert_eq!(s.entries(), &[7, 9]);
        assert_eq!(s.pop(), Some(9));
        assert_eq!(s.depth(), 1);
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p mtc-core driver` — expected: compile error.

- [ ] **Step 3: Implement**

`crates/core/src/vm/driver.rs`:
```rust
//! Synchronous driver: answers the sans-I/O core's bus requests against
//! in-memory components and does all tact accounting (spec §4.4).

use super::bus::{BusRequest, BusResponse, CoreEvent};
use super::core::Core;
use super::devices::Tape;
use super::trap::Trap;

#[derive(Debug)]
pub struct ReturnStack {
    entries: Vec<u32>,
    capacity: usize,
}

impl ReturnStack {
    pub fn new(capacity: usize) -> Self {
        Self { entries: Vec::new(), capacity }
    }

    pub fn depth(&self) -> usize {
        self.entries.len()
    }

    pub fn entries(&self) -> &[u32] {
        &self.entries
    }

    pub(crate) fn push(&mut self, value: u32) -> bool {
        if self.entries.len() == self.capacity {
            return false;
        }
        self.entries.push(value);
        true
    }

    pub(crate) fn pop(&mut self) -> Option<u32> {
        self.entries.pop()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TactProfile {
    pub move_cost: u32,
    pub read_cost: u32,
    pub write_cost: u32,
}

impl TactProfile {
    pub const ELECTRONIC: TactProfile =
        TactProfile { move_cost: 1, read_cost: 1, write_cost: 1 };
}

#[derive(Debug, Clone, Copy, Default)]
pub struct RunLimits {
    pub max_steps: Option<u64>,
    pub max_tacts: Option<u64>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RunStats {
    pub steps: u64,
    pub core_tacts: u64,
    pub stall_tacts: u64,
}

impl RunStats {
    pub fn total_tacts(&self) -> u64 {
        self.core_tacts + self.stall_tacts
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Stopped,
    Halted,
    Trapped(Trap),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RunResult {
    pub outcome: Outcome,
    pub stats: RunStats,
}

pub fn run(
    core: &mut Core,
    code: &[u8],
    stack: &mut ReturnStack,
    device: &mut dyn Tape,
    profile: TactProfile,
    limits: RunLimits,
) -> RunResult {
    let mut stats = RunStats::default();
    let over_tacts = |stats: &RunStats| {
        limits.max_tacts.is_some_and(|max| stats.total_tacts() >= max)
    };

    let mut event = core.start();
    loop {
        match event {
            CoreEvent::Request(request) => {
                let response = match request {
                    BusRequest::CodeRead { addr } => match code.get(addr as usize) {
                        Some(&byte) => {
                            stats.core_tacts += 1;
                            BusResponse::Byte(byte)
                        }
                        None => BusResponse::OutOfCode,
                    },
                    BusRequest::StackPush { value } => {
                        if stack.push(value) {
                            stats.core_tacts += 1;
                            BusResponse::Ok
                        } else {
                            BusResponse::StackFull
                        }
                    }
                    BusRequest::StackPop => match stack.pop() {
                        Some(value) => {
                            stats.core_tacts += 1;
                            BusResponse::Value(value)
                        }
                        None => BusResponse::StackEmpty,
                    },
                    BusRequest::DeviceMoveLeft { .. } => {
                        device.left();
                        stats.stall_tacts += u64::from(profile.move_cost);
                        BusResponse::Ok
                    }
                    BusRequest::DeviceMoveRight { .. } => {
                        device.right();
                        stats.stall_tacts += u64::from(profile.move_cost);
                        BusResponse::Ok
                    }
                    BusRequest::DeviceRead { .. } => {
                        stats.stall_tacts += u64::from(profile.read_cost);
                        BusResponse::Symbol(device.read())
                    }
                    BusRequest::DeviceWrite { index, .. } => match device.write(index) {
                        Ok(()) => {
                            stats.stall_tacts += u64::from(profile.write_cost);
                            BusResponse::Ok
                        }
                        Err(fault) => BusResponse::Fault(fault),
                    },
                };
                if over_tacts(&stats) {
                    return RunResult { outcome: Outcome::Trapped(Trap::TactLimit), stats };
                }
                event = core.resume(response);
            }
            CoreEvent::Step => {
                stats.steps += 1;
                stats.core_tacts += 1; // execute base (spec §4.4)
                if limits.max_steps.is_some_and(|max| stats.steps >= max) {
                    return RunResult { outcome: Outcome::Trapped(Trap::StepLimit), stats };
                }
                if over_tacts(&stats) {
                    return RunResult { outcome: Outcome::Trapped(Trap::TactLimit), stats };
                }
                event = core.resume(BusResponse::Ok);
            }
            CoreEvent::Stopped => return RunResult { outcome: Outcome::Stopped, stats },
            CoreEvent::Halted => return RunResult { outcome: Outcome::Halted, stats },
            CoreEvent::Trapped(trap) => {
                return RunResult { outcome: Outcome::Trapped(trap), stats }
            }
        }
    }
}
```

Note for the test `return_stack_reports_depth_and_entries`: it calls `push`/`pop` directly — they are `pub(crate)`, which the same-crate test module can reach. Keep them `pub(crate)` (drivers outside the crate go through `run`).

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p mtc-core`
Expected: all pass. Verify the exact-stats tests especially — if one fails, the accounting order deviates from spec §4.4; fix the driver, not the numbers.

- [ ] **Step 5: Gates + commit**

Run: `cargo clippy --workspace --all-targets -- -D warnings && cargo fmt`
```bash
git add -A
git commit -m "feat(core): sync driver with tact accounting, wait states, limits"
```

---

### Task 3: `ArchRegistry`, `LoadError`, `Machine` facade

**Files:**
- Create: `crates/core/src/vm/machine.rs`
- Modify: `crates/core/src/vm/mod.rs` (add `pub mod machine;` + `pub use machine::{ArchRegistry, LoadError, Machine, RunOptions};`)

**Interfaces:**
- Consumes: `Executable` (formats), `Arch`, `Core`, driver items, `Tape`.
- Produces: the `machine` block from the header. Loading rules: `from_executable` = registry lookup (`UnknownArch`) then `with_arch`; `with_arch` validates `code[entry]` via `arch.is_entry_marker` (`EntryNotEntryMarker { at: entry }`). `run` latches `MF := (device.read() == 1)` tact-free before `Core::start` — spec §4.3 step 4. (PM-1's mark index is 1; a future arch with different initial-latch semantics will need an `Arch` hook — out of scope, note only.)

- [ ] **Step 1: Write the failing tests**

Test module at the bottom of `machine.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::formats::executable::Executable;
    use crate::vm::arch::test_arch::TestArch;
    use crate::vm::devices::{InfiniteTape, Tape};
    use crate::vm::driver::Outcome;

    // TestArch entry marker: 0x0E

    #[test]
    fn with_arch_rejects_bad_entry() {
        let arch = TestArch;
        let err = Machine::with_arch(&arch, vec![0x01, 0x02], 0).unwrap_err();
        assert_eq!(err, LoadError::EntryNotEntryMarker { at: 0 });
        assert!(Machine::with_arch(&arch, vec![0x0E, 0x02], 0).is_ok());
    }

    #[test]
    fn registry_resolves_arch_or_errors() {
        let mut registry = ArchRegistry::new();
        registry.register(Box::new(TestArch));
        let exe = Executable { arch: 0x7F, entry: 0, code: vec![0x0E, 0x02] };
        assert!(Machine::from_executable(&exe, &registry).is_ok());
        let alien = Executable { arch: 0x09, entry: 0, code: vec![0x0E, 0x02] };
        assert_eq!(
            Machine::from_executable(&alien, &registry).unwrap_err(),
            LoadError::UnknownArch(0x09)
        );
    }

    #[test]
    fn run_executes_and_reports() {
        let arch = TestArch;
        // entry, right (move+latch), stop
        let machine = Machine::with_arch(&arch, vec![0x0E, 0x06, 0x02], 0).unwrap();
        let mut tape = InfiniteTape::new();
        let result = machine.run(&mut tape, RunOptions::default());
        assert_eq!(result.outcome, Outcome::Stopped);
        assert_eq!(tape.head(), 1);
    }

    #[test]
    fn initial_mf_is_latched_from_device_tact_free() {
        let arch = TestArch;
        // jm rel32 +1 (instr_end 5, target 6): taken only if MF was latched true
        // [0]=0x0E entry? — entry must be entry marker; but we start at entry 1
        // layout: [0]=0x0E, [1..6]=jm +1, [6]=halt (skipped if taken), [7]=stop
        // Wait: taken jump target = 6+1=7? instr_end of jm at 1 is 6; off +1 → 7.
        let code = vec![0x0E, 0x09, 0x01, 0x00, 0x00, 0x00, 0x03, 0x02];
        let machine = Machine::with_arch(&arch, code, 0).unwrap();

        // Marked start cell → MF true → jump skips the halt, reaches stop.
        let mut marked = InfiniteTape::from_cells([true], 0, 0);
        let r1 = machine.run(&mut marked, RunOptions::default());
        assert_eq!(r1.outcome, Outcome::Stopped);

        // Blank start cell → MF false → falls into halt.
        let mut blank = InfiniteTape::new();
        let r2 = machine.run(&mut blank, RunOptions::default());
        assert_eq!(r2.outcome, Outcome::Halted);

        // The latch read is tact-free: identical stats except the outcome path.
        assert_eq!(r1.stats.stall_tacts, 0); // no device commands executed at all
    }

    #[test]
    fn accessors_expose_code_and_entry() {
        let arch = TestArch;
        let machine = Machine::with_arch(&arch, vec![0x02, 0x0E, 0x02], 1).unwrap();
        assert_eq!(machine.entry(), 1);
        assert_eq!(machine.code(), &[0x02, 0x0E, 0x02]);
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p mtc-core machine` — expected: compile error.

- [ ] **Step 3: Implement**

`crates/core/src/vm/machine.rs`:
```rust
//! Loader + facade: Executable → validated Machine → runs (spec §4.3).

use crate::formats::executable::Executable;

use super::arch::Arch;
use super::core::Core;
use super::devices::Tape;
use super::driver::{run, ReturnStack, RunLimits, RunResult, TactProfile};

#[derive(Default)]
pub struct ArchRegistry {
    archs: Vec<Box<dyn Arch>>,
}

impl ArchRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, arch: Box<dyn Arch>) {
        self.archs.push(arch);
    }

    pub fn get(&self, id: u8) -> Option<&dyn Arch> {
        self.archs.iter().find(|a| a.arch_id() == id).map(|a| a.as_ref())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoadError {
    UnknownArch(u8),
    EntryNotEntryMarker { at: u32 },
}

impl std::fmt::Display for LoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownArch(id) => write!(f, "unknown architecture {id:#04x}"),
            Self::EntryNotEntryMarker { at } => {
                write!(f, "entry point {at:#010x} is not an entry marker")
            }
        }
    }
}

impl std::error::Error for LoadError {}

#[derive(Debug, Clone, Copy)]
pub struct RunOptions {
    pub stack_depth: usize,
    pub profile: TactProfile,
    pub limits: RunLimits,
}

impl Default for RunOptions {
    fn default() -> Self {
        Self {
            stack_depth: 1024,
            profile: TactProfile::ELECTRONIC,
            limits: RunLimits::default(),
        }
    }
}

pub struct Machine<'a> {
    arch: &'a dyn Arch,
    code: Vec<u8>,
    entry: u32,
}

impl<'a> Machine<'a> {
    pub fn with_arch(
        arch: &'a dyn Arch,
        code: Vec<u8>,
        entry: u32,
    ) -> Result<Machine<'a>, LoadError> {
        match code.get(entry as usize) {
            Some(&byte) if arch.is_entry_marker(byte) => Ok(Machine { arch, code, entry }),
            _ => Err(LoadError::EntryNotEntryMarker { at: entry }),
        }
    }

    pub fn from_executable(
        exe: &Executable,
        registry: &'a ArchRegistry,
    ) -> Result<Machine<'a>, LoadError> {
        let arch = registry.get(exe.arch).ok_or(LoadError::UnknownArch(exe.arch))?;
        Machine::with_arch(arch, exe.code.clone(), exe.entry)
    }

    pub fn entry(&self) -> u32 {
        self.entry
    }

    pub fn code(&self) -> &[u8] {
        &self.code
    }

    pub fn run(&self, device: &mut dyn Tape, opts: RunOptions) -> RunResult {
        let mut core = Core::new(self.arch, self.entry);
        // Spec §4.3 step 4: latch initial MF from the device, tact-free
        // (loading, not execution). PM-1 matches against the mark index 1.
        core.set_mf(device.read() == 1);
        let mut stack = ReturnStack::new(opts.stack_depth);
        run(&mut core, &self.code, &mut stack, device, opts.profile, opts.limits)
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p mtc-core`
Expected: all pass.

- [ ] **Step 5: Gates + commit**

Run: `cargo clippy --workspace --all-targets -- -D warnings && cargo fmt`
```bash
git add -A
git commit -m "feat(core): ArchRegistry, LoadError, Machine facade with tact-free MF latch"
```

---

### Task 4: `InfiniteTape` ↔ `TapeSnapshot` bridge

**Files:**
- Modify: `crates/core/src/vm/devices/infinite_tape.rs`

**Interfaces:**
- Consumes: `TapeSnapshot` (formats::tapeblock), `DeviceFault`.
- Produces:
  ```rust
  impl InfiniteTape {
      pub fn from_snapshot(s: &TapeSnapshot) -> Result<InfiniteTape, DeviceFault>; // any cell > 1 → IndexOutsideAlphabet
      pub fn to_snapshot(&self) -> TapeSnapshot; // span = min..=max of (marked cells ∪ {head}); blank tape → 1-cell span at head
  }
  ```
  Round-trip law: `from_snapshot(&t.to_snapshot())` reproduces marked cells + head.

- [ ] **Step 1: Write the failing tests**

Append to `infinite_tape.rs`'s test module:

```rust
    use crate::formats::tapeblock::TapeSnapshot;

    #[test]
    fn from_snapshot_places_cells_and_head() {
        let snap = TapeSnapshot { origin: -2, cells: vec![1, 0, 1, 1, 0], head: 1 };
        let tape = InfiniteTape::from_snapshot(&snap).unwrap();
        assert_eq!(tape.marked_cells(), vec![-2, 0, 1]);
        assert_eq!(tape.head(), 1);
        assert_eq!(tape.read(), 1);
    }

    #[test]
    fn from_snapshot_rejects_wide_alphabet_cells() {
        let snap = TapeSnapshot { origin: 0, cells: vec![0, 2], head: 0 };
        assert_eq!(
            InfiniteTape::from_snapshot(&snap),
            Err(DeviceFault::IndexOutsideAlphabet { index: 2 })
        );
    }

    #[test]
    fn to_snapshot_covers_marks_and_head() {
        let mut tape = InfiniteTape::from_cells([true, false, true], 0, 0);
        for _ in 0..5 {
            tape.right(); // head 5, past the data
        }
        let snap = tape.to_snapshot();
        assert_eq!(snap.origin, 0);
        assert_eq!(snap.cells, vec![1, 0, 1, 0, 0, 0]); // span 0..=5 (marks ∪ head)
        assert_eq!(snap.head, 5);
    }

    #[test]
    fn blank_tape_snapshot_is_single_cell_at_head() {
        let mut tape = InfiniteTape::new();
        tape.left();
        tape.left();
        let snap = tape.to_snapshot();
        assert_eq!(snap.origin, -2);
        assert_eq!(snap.cells, vec![0]);
        assert_eq!(snap.head, -2);
    }

    #[test]
    fn snapshot_round_trip_law() {
        let mut tape = InfiniteTape::from_cells([true, true, false, true], -3, 2);
        tape.write(1).unwrap();
        let back = InfiniteTape::from_snapshot(&tape.to_snapshot()).unwrap();
        assert_eq!(back.marked_cells(), tape.marked_cells());
        assert_eq!(back.head(), tape.head());
    }
```

Requires `PartialEq` on the `from_snapshot` error path — `DeviceFault` already derives it; add `#[derive(Debug, PartialEq)]`-compatible comparison for `InfiniteTape` only via its accessors (do NOT derive PartialEq on the tape).

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p mtc-core infinite_tape` — expected: compile error.

- [ ] **Step 3: Implement**

Append to the `impl InfiniteTape` block:

```rust
    /// Build from a `TapeSnapshot` (spec §6.3). Cells must be 0/1 —
    /// a wider index is the snapshot's problem, not this tape's.
    pub fn from_snapshot(
        s: &crate::formats::tapeblock::TapeSnapshot,
    ) -> Result<Self, DeviceFault> {
        if let Some(&bad) = s.cells.iter().find(|&&c| c > 1) {
            return Err(DeviceFault::IndexOutsideAlphabet { index: u32::from(bad) });
        }
        Ok(Self::from_cells(
            s.cells.iter().map(|&c| c == 1),
            s.origin,
            s.head,
        ))
    }

    /// Dense snapshot spanning marked cells ∪ head (blank tape → one
    /// blank cell at the head).
    pub fn to_snapshot(&self) -> crate::formats::tapeblock::TapeSnapshot {
        let marks = self.marked_cells();
        let lo = marks.first().copied().unwrap_or(self.head).min(self.head);
        let hi = marks.last().copied().unwrap_or(self.head).max(self.head);
        let cells = (lo..=hi).map(|c| u8::from(self.get(c))).collect();
        crate::formats::tapeblock::TapeSnapshot { origin: lo, cells, head: self.head }
    }
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p mtc-core`
Expected: all pass.

- [ ] **Step 5: Gates + commit**

Run: `cargo clippy --workspace --all-targets -- -D warnings && cargo fmt`
```bash
git add -A
git commit -m "feat(core): InfiniteTape <-> TapeSnapshot bridge"
```

---

### Task 5: PM-1 end-to-end programs

**Files:**
- Create: `crates/post-machine/tests/pm1_programs.rs`

**Interfaces:**
- Consumes: everything this plan built. This is the plan's deliverable: real PM-1 bytecode running on the real machine, tact numbers matching spec §4.4, `.pmb` round trip through an actual run.

- [ ] **Step 1: Write the tests (these ARE the deliverable — no stub phase)**

`crates/post-machine/tests/pm1_programs.rs`:

```rust
//! First real Post-machine programs: hand-assembled PM-1 bytecode,
//! end-to-end through Executable → Machine → tape, with spec §4.4
//! tact arithmetic pinned exactly.

use mtc_core::formats::executable::Executable;
use mtc_core::formats::tapeblock::{TapeBlockFile, TapeSnapshot};
use mtc_core::formats::ARCH_PM1;
use mtc_core::vm::{
    ArchRegistry, InfiniteTape, LoadError, Machine, Outcome, RunLimits, RunOptions,
    RunStats, TactProfile, Tape, Trap,
};
use mtc_post_machine::arch::opcodes::*;
use mtc_post_machine::arch::Pm1;

fn registry() -> ArchRegistry {
    let mut r = ArchRegistry::new();
    r.register(Box::new(Pm1));
    r
}

fn machine_for(code: Vec<u8>) -> Executable {
    Executable { arch: ARCH_PM1, entry: 0, code }
}

#[test]
fn go_to_end_walks_to_first_blank() {
    // ent; L: rgt; jm.s L; stp        (the 2012 goToEnd, hand-assembled)
    // jm.s at 2..4, instr_end 4, target 1 → off -3
    let code = vec![ENT, RGT, JM_S, 0xFD, STP];
    let reg = registry();
    let machine = Machine::from_executable(&machine_for(code), &reg).unwrap();

    // marks at 0,1,2 — head starts on 0
    let mut tape = InfiniteTape::from_cells([true, true, true], 0, 0);
    let result = machine.run(&mut tape, RunOptions::default());

    assert_eq!(result.outcome, Outcome::Stopped);
    assert_eq!(tape.head(), 3); // first blank after the section
    assert_eq!(tape.marked_cells(), vec![0, 1, 2]); // tape unchanged
    // ent 2 | 3 × rgt (2 core + 2 stall) | 3 × jm.s 3 | stp 1
    assert_eq!(
        result.stats,
        RunStats { steps: 7, core_tacts: 18, stall_tacts: 6 }
    );
}

#[test]
fn spec_tact_numbers_hold() {
    let reg = registry();

    // rgt: 2 core + 2 stall (program total: ent 2 + rgt 4 + stp 1)
    let m = Machine::from_executable(&machine_for(vec![ENT, RGT, STP]), &reg).unwrap();
    let mut t = InfiniteTape::new();
    let r = m.run(&mut t, RunOptions::default());
    assert_eq!(r.stats, RunStats { steps: 2, core_tacts: 5, stall_tacts: 2 });

    // wr: 3 core + 2 stall (spec: wr = 5 total, electronic)
    let m = Machine::from_executable(&machine_for(vec![ENT, WR, 0x81, STP]), &reg).unwrap();
    let mut t = InfiniteTape::new();
    let r = m.run(&mut t, RunOptions::default());
    assert_eq!(r.stats, RunStats { steps: 2, core_tacts: 6, stall_tacts: 2 });
    assert_eq!(t.marked_cells(), vec![0]);

    // call far = 8 core (spec §4.4): ent 2 + call 8 + ent 2 + ret 3 + stp 1
    let code = vec![ENT, CALL, 0x01, 0x00, 0x00, 0x00, STP, ENT, RET];
    let m = Machine::from_executable(&machine_for(code), &reg).unwrap();
    let mut t = InfiniteTape::new();
    let r = m.run(&mut t, RunOptions::default());
    assert_eq!(r.outcome, Outcome::Stopped);
    assert_eq!(r.stats, RunStats { steps: 4, core_tacts: 16, stall_tacts: 0 });

    // call.s = 5 core: ent 2 + call.s 5 + ent 2 + ret 3 + stp 1
    let code = vec![ENT, CALL_S, 0x01, STP, ENT, RET];
    let m = Machine::from_executable(&machine_for(code), &reg).unwrap();
    let mut t = InfiniteTape::new();
    let r = m.run(&mut t, RunOptions::default());
    assert_eq!(r.stats, RunStats { steps: 4, core_tacts: 13, stall_tacts: 0 });
}

#[test]
fn mechanical_profile_shows_the_stall_economy() {
    let reg = registry();
    let m = Machine::from_executable(&machine_for(vec![ENT, RGT, STP]), &reg).unwrap();
    let mut t = InfiniteTape::new();
    let mech = TactProfile { move_cost: 50, read_cost: 5, write_cost: 10 };
    let r = m.run(&mut t, RunOptions { profile: mech, ..RunOptions::default() });
    assert_eq!(r.stats.core_tacts, 5);
    assert_eq!(r.stats.stall_tacts, 55); // one move + one latch read
}

#[test]
fn call_to_non_entry_traps() {
    // call targets the stp byte (not ent)
    let code = vec![ENT, CALL, 0x01, 0x00, 0x00, 0x00, STP, STP];
    let reg = registry();
    let m = Machine::from_executable(&machine_for(code), &reg).unwrap();
    let mut t = InfiniteTape::new();
    let r = m.run(&mut t, RunOptions::default());
    assert_eq!(r.outcome, Outcome::Trapped(Trap::CallTargetNotEntry { target: 7 }));
}

#[test]
fn runaway_recursion_overflows_the_stack() {
    // ent; call -6 (targets its own ent → infinite recursion)
    let code = vec![ENT, CALL, 0xFA, 0xFF, 0xFF, 0xFF, STP];
    let reg = registry();
    let m = Machine::from_executable(&machine_for(code), &reg).unwrap();
    let mut t = InfiniteTape::new();
    let opts = RunOptions { stack_depth: 8, ..RunOptions::default() };
    let r = m.run(&mut t, opts);
    assert_eq!(r.outcome, Outcome::Trapped(Trap::StackOverflow));
}

#[test]
fn step_limit_stops_the_infinite_loop() {
    // ent; L: jmp.s L    (jmp.s at 1..3, instr_end 3, target 1 → off -2)
    let code = vec![ENT, JMP_S, 0xFE];
    let reg = registry();
    let m = Machine::from_executable(&machine_for(code), &reg).unwrap();
    let mut t = InfiniteTape::new();
    let opts = RunOptions {
        limits: RunLimits { max_steps: Some(1000), max_tacts: None },
        ..RunOptions::default()
    };
    let r = m.run(&mut t, opts);
    assert_eq!(r.outcome, Outcome::Trapped(Trap::StepLimit));
}

#[test]
fn loader_rejects_bad_entry_and_unknown_arch() {
    let reg = registry();
    let bad_entry = Executable { arch: ARCH_PM1, entry: 0, code: vec![RGT, STP] };
    assert_eq!(
        Machine::from_executable(&bad_entry, &reg).unwrap_err(),
        LoadError::EntryNotEntryMarker { at: 0 }
    );
    let alien = Executable { arch: 0x42, entry: 0, code: vec![ENT, STP] };
    assert_eq!(
        Machine::from_executable(&alien, &reg).unwrap_err(),
        LoadError::UnknownArch(0x42)
    );
}

#[test]
fn pmb_in_run_pmb_out() {
    // Input tape-block file: marks at 0,1,2 and 4, head 0. Run goToEnd.
    let input = TapeBlockFile {
        alphabet: vec![" ".into(), "*".into()],
        tapes: vec![TapeSnapshot { origin: 0, cells: vec![1, 1, 1, 0, 1], head: 0 }],
    };
    let bytes = input.to_bytes();
    let parsed = TapeBlockFile::from_bytes(&bytes).unwrap();
    let mut tape = InfiniteTape::from_snapshot(&parsed.tapes[0]).unwrap();

    let code = vec![ENT, RGT, JM_S, 0xFD, STP];
    let reg = registry();
    let m = Machine::from_executable(&machine_for(code), &reg).unwrap();
    let r = m.run(&mut tape, RunOptions::default());
    assert_eq!(r.outcome, Outcome::Stopped);
    assert_eq!(tape.head(), 3);

    // Snapshot the result back into a .pmb and round-trip it.
    let output = TapeBlockFile {
        alphabet: parsed.alphabet.clone(),
        tapes: vec![tape.to_snapshot()],
    };
    let out_bytes = output.to_bytes();
    let reparsed = TapeBlockFile::from_bytes(&out_bytes).unwrap();
    assert_eq!(reparsed.tapes[0].head, 3);
    assert_eq!(reparsed.tapes[0].cells, vec![1, 1, 1, 0, 1]); // data intact
}
```

Also extend `crates/core/src/vm/mod.rs` re-exports if the imports above need them (`pub use devices::{AnnularTape, InfiniteTape, StrictTape, Tape};` etc.) — the test file's `use mtc_core::vm::{...}` list is the required public surface.

- [ ] **Step 2: Run to verify RED, then GREEN**

Run: `cargo test -p mtc-post-machine --test pm1_programs`
Expected first run: compile errors for any missing re-export; fix `vm/mod.rs` re-exports only (no logic changes). Then: if any tact assertion fails, re-derive the arithmetic by hand against spec §4.4 BEFORE touching anything — the numbers in these tests were derived from the accounting rules; a mismatch means either the driver deviates from spec (fix driver) or the derivation is wrong (report it, don't silently adjust).

- [ ] **Step 3: Full gates**

Run: `cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --check`
Expected: everything green.

- [ ] **Step 4: Commit**

```bash
git add -A
git commit -m "test(post-machine): first end-to-end PM-1 programs with spec tact numbers"
```

---

## Self-Review Notes

- **Spec coverage:** §5 opcode table incl. short-form rule + reserved gaps (Task 1); §4.4 tact rules, wait states, tape profiles, limits (Task 2, numbers re-asserted end-to-end in Task 5); §4.3 loading sequence — arch select, entry validation, tact-free MF latch (Task 3); §4.5 trap surfacing through `Outcome` (Tasks 2/5); §6.3 `.pmb` as VM input/output (Tasks 4/5). Deliberately NOT here: `DebugSession` (Plan 7), assembler/disassembler (Plan 3), `pmt run` CLI (Plan 7), the `.pmx` sniff helper (Plan 3+, noted from Plan 1 review).
- **Type consistency:** driver/machine/bridge names match the header block; PM-1 constants match spec §5 and the `short = far | 0x10` rule is self-checked in tests; `ARCH_PM1` imported from formats, not redefined.
- **Tact arithmetic cross-check (hand-derived):** `go_to_end` = ent(2) + 3×rgt(2c+2s) + 3×jm.s(3) + stp(1) = 18 core + 6 stall, 7 steps ✓; `call` far chain = 2+8+2+3+1 = 16 core, 4 steps ✓; `call.s` chain = 2+5+2+3+1 = 13 ✓. Terminal `stp` costs fetch only (never reaches `Step`) — stated in Global Constraints.
- **Known simplifications, on record:** initial-MF match index 1 is hardcoded in `Machine::run` (correct for PM-1 and TestArch; a future arch hook is noted in Task 3); `ReturnStack::push/pop` stay `pub(crate)`; registry is a linear scan (n ≤ 2 for years).
