# Plan 2a/7: VM Devices + Sans-I/O Processor Core

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The arch-agnostic half of the VM in `mtc-core`: tape devices (`InfiniteBelt`, `AnnularBelt`, `StrictBelt`) and the pure sans-I/O processor core (fetch/decode/execute automaton speaking a bus request/response protocol), fully unit-tested with a fake test arch. No PM-1 knowledge anywhere in this plan — that's Plan 2b (`mtc-post-machine`), along with the sync driver, loader, and end-to-end runs.

**Architecture:** Spec §4 — the core is a pure transition function: `resume(BusResponse) -> CoreEvent` emitting `BusRequest`s; it owns only registers (IP, MF) and the in-flight instruction. Devices are index-based `Tape` implementors behind the (future) driver. The `Arch` trait supplies operand shapes and lowers instructions to `MicroOp`s; core tests script bus responses and assert emitted requests — no devices, no I/O.

**Tech Stack:** Rust stable, edition 2024, existing `mtc-core` crate. No new dependencies.

**Spec:** `docs/superpowers/specs/2026-07-04-post-machine-toolchain-design.md` §4 (processor architecture, buses, sans-I/O, belts, timing roles), §5 (operand encodings the fetcher must handle).

## Global Constraints

- **Core contains no opcode**: all instruction knowledge enters through the `Arch` trait; core tests use only the fake `TestArch` defined here (spec §10 "arch-agnostic by contract").
- Jump/call operands are **IP-relative to the end of the instruction** (spec §5).
- Symbol-vector operands are **self-delimiting**: each byte = 7-bit payload, high bit set on the LAST element (spec Appendix A / §5 `wr`).
- MF is the **match flag**: `LatchMatch(idx)` micro-op sets `MF := (device0.read() == idx)` (spec §4.1); jumps test MF, never the tape.
- `call` semantics: verify target byte is the arch's entry marker BEFORE pushing/jumping; failure = `Trap::CallTargetNotEntry` (spec §5).
- `InfiniteBelt` guarantees (spec §4.2): paged `u64` bitmask storage in a `HashMap`, current-page cache, reads never allocate, zeroed pages are freed — memory `O(pages containing non-blank cells)`.
- `StrictBelt`: writing the value a cell already holds is a `DeviceFault` (2006/2007 semantics); default belts are idempotent.
- Quality gates on every commit: `cargo test --workspace` green, `cargo clippy --workspace --all-targets -- -D warnings` clean, `cargo fmt --check` clean; no attribution footers.
- Commit policy: per-task commits pre-approved in this repo; never push.

## Interfaces Established by This Plan (later plans depend on these exact names)

```rust
// mtc_core::vm (module tree: vm/{mod,trap,bus,arch,devices,core}.rs; devices/ is a dir module)
pub enum Trap {
    InvalidOpcode { opcode: u8, at: u32 },
    CodeOutOfBounds { at: u32 },
    BadOperand { at: u32 },
    CallTargetNotEntry { target: u32 },
    StackOverflow,
    StackUnderflow,
    StepLimit,
    TactLimit,
    Device { fault: DeviceFault },
}
pub enum DeviceFault { IndexOutsideAlphabet { index: u32 }, StrictCellViolation }

pub enum BusRequest {
    CodeRead { addr: u32 },
    StackPush { value: u32 },
    StackPop,
    DeviceMoveLeft { dev: u8 },
    DeviceMoveRight { dev: u8 },
    DeviceRead { dev: u8 },
    DeviceWrite { dev: u8, index: u32 },
}
pub enum BusResponse {
    Byte(u8), OutOfCode,           // CodeRead results
    Ok,                            // StackPush / moves / writes / Step acknowledgement
    StackFull,
    Value(u32), StackEmpty,        // StackPop results
    Symbol(u32),                   // DeviceRead result
    Fault(DeviceFault),            // device rejected the command
}
pub enum CoreEvent {
    Request(BusRequest),
    Step,                          // one instruction retired; driver adds exec tact, checks limits, resumes with Ok
    Stopped, Halted,
    Trapped(Trap),
}

pub enum OperandKind { None, RelI8, RelI32, SymbolVec }
pub enum Operand { None, I8(i8), I32(i32), Symbols(Vec<u32>) }
pub enum MicroOp {
    MoveLeft, MoveRight,           // device 0 (v1 single-device)
    Write(u32),
    LatchMatch(u32),               // MF := (device0.read() == idx)
    JumpRel(i32),
    JumpRelIf { off: i32, when_match: bool },
    Call(i32), Ret,
    Stop, Halt, Brk, Nop,
}
pub trait Arch {
    fn arch_id(&self) -> u8;
    fn operand_kind(&self, opcode: u8) -> Option<OperandKind>;   // None = invalid opcode
    fn lower(&self, opcode: u8, operand: &Operand) -> Result<Vec<MicroOp>, Trap>;
    fn is_entry_marker(&self, byte: u8) -> bool;
}

// mtc_core::vm::devices
pub trait Tape {
    fn alphabet_size(&self) -> u32;
    fn left(&mut self);
    fn right(&mut self);
    fn read(&self) -> u32;
    fn write(&mut self, index: u32) -> Result<(), DeviceFault>;
}
pub struct InfiniteBelt; // ::new(), ::from_cells(iter of bool, head: i64), .head() -> i64, .page_count() -> usize, .marked_cells() -> Vec<i64> (sorted)
pub struct AnnularBelt;  // ::new(size: u32), .head() -> u32; wraps
pub struct StrictBelt<T: Tape>; // ::new(inner: T), Deref-free wrapper implementing Tape

// mtc_core::vm::core
pub struct Core<'a> { /* arch: &'a dyn Arch, ip, mf, phase */ }
// Core::new(arch: &dyn Arch, entry: u32) -> Core   (initial MF is latched by the DRIVER before first resume — Plan 2b; core starts at FetchOpcode)
// Core::start(&mut self) -> CoreEvent               (first CodeRead request)
// Core::resume(&mut self, resp: BusResponse) -> CoreEvent
// Core::ip(&self) -> u32; Core::mf(&self) -> bool; Core::set_mf(&mut self, bool)
```

Protocol rules (the contract Plan 2b's driver will obey):
- Core issues one `Request` at a time; the driver answers with exactly one `BusResponse` via `resume`.
- After an instruction's last micro-op, core returns `CoreEvent::Step`; the driver resumes with `BusResponse::Ok` to begin the next fetch.
- `OutOfCode` in response to any `CodeRead` → `Trapped(CodeOutOfBounds)`. `StackFull`/`StackEmpty` → the stack traps. `Fault(f)` → `Trapped(Device { fault: f })`.
- A jump/call whose computed target is negative or exceeds `u32::MAX` traps `CodeOutOfBounds` immediately (core-side arithmetic); in-range-but-past-the-image targets surface on the next `CodeRead` as `OutOfCode`.

---

### Task 1: `vm` type foundations + `TestArch` fixture

**Files:**
- Create: `crates/core/src/vm/mod.rs`
- Create: `crates/core/src/vm/trap.rs`
- Create: `crates/core/src/vm/bus.rs`
- Create: `crates/core/src/vm/arch.rs`
- Modify: `crates/core/src/lib.rs` (add `pub mod vm;`)

**Interfaces:**
- Consumes: nothing new.
- Produces: every type in the "Interfaces Established" block above except `Tape`/belts/`Core`; plus the test-only `TestArch` (in `arch.rs`'s test module, `pub(crate)` re-exported for sibling test modules via `#[cfg(test)] pub(crate) mod test_arch`).

- [ ] **Step 1: Write the failing test**

`crates/core/src/vm/arch.rs` will end with (write this first):

```rust
/// Fake architecture for core tests — proves core is arch-agnostic.
/// 0x01 nop | 0x02 stop | 0x03 halt | 0x04 brk | 0x05 left+latch |
/// 0x06 right+latch | 0x07 wr(vec)+latch | 0x08 jmp rel8 | 0x09 jm rel32 |
/// 0x0A call rel32 | 0x0B ret | 0x0E entry marker (lowers to nothing)
#[cfg(test)]
pub(crate) mod test_arch {
    use super::*;

    pub(crate) struct TestArch;

    impl Arch for TestArch {
        fn arch_id(&self) -> u8 {
            0x7F
        }

        fn operand_kind(&self, opcode: u8) -> Option<OperandKind> {
            match opcode {
                0x01..=0x06 | 0x0B | 0x0E => Some(OperandKind::None),
                0x07 => Some(OperandKind::SymbolVec),
                0x08 => Some(OperandKind::RelI8),
                0x09 | 0x0A => Some(OperandKind::RelI32),
                _ => None,
            }
        }

        fn lower(&self, opcode: u8, operand: &Operand) -> Result<Vec<MicroOp>, Trap> {
            Ok(match (opcode, operand) {
                (0x01, _) | (0x0E, _) => vec![MicroOp::Nop],
                (0x02, _) => vec![MicroOp::Stop],
                (0x03, _) => vec![MicroOp::Halt],
                (0x04, _) => vec![MicroOp::Brk],
                (0x05, _) => vec![MicroOp::MoveLeft, MicroOp::LatchMatch(1)],
                (0x06, _) => vec![MicroOp::MoveRight, MicroOp::LatchMatch(1)],
                (0x07, Operand::Symbols(s)) if s.len() == 1 => {
                    vec![MicroOp::Write(s[0]), MicroOp::LatchMatch(1)]
                }
                (0x07, _) => return Err(Trap::BadOperand { at: 0 }),
                (0x08, Operand::I8(o)) => vec![MicroOp::JumpRel(i32::from(*o))],
                (0x09, Operand::I32(o)) => {
                    vec![MicroOp::JumpRelIf { off: *o, when_match: true }]
                }
                (0x0A, Operand::I32(o)) => vec![MicroOp::Call(*o)],
                (0x0B, _) => vec![MicroOp::Ret],
                _ => return Err(Trap::BadOperand { at: 0 }),
            })
        }

        fn is_entry_marker(&self, byte: u8) -> bool {
            byte == 0x0E
        }
    }
}

#[cfg(test)]
mod tests {
    use super::test_arch::TestArch;
    use super::*;

    #[test]
    fn operand_kinds_cover_the_table() {
        let a = TestArch;
        assert!(matches!(a.operand_kind(0x01), Some(OperandKind::None)));
        assert!(matches!(a.operand_kind(0x07), Some(OperandKind::SymbolVec)));
        assert!(matches!(a.operand_kind(0x08), Some(OperandKind::RelI8)));
        assert!(matches!(a.operand_kind(0x09), Some(OperandKind::RelI32)));
        assert!(a.operand_kind(0x55).is_none());
    }

    #[test]
    fn lower_write_requires_exactly_one_symbol() {
        let a = TestArch;
        assert!(a.lower(0x07, &Operand::Symbols(vec![1])).is_ok());
        assert!(a.lower(0x07, &Operand::Symbols(vec![1, 2])).is_err());
        assert!(a.lower(0x07, &Operand::None).is_err());
    }

    #[test]
    fn entry_marker_is_recognized() {
        let a = TestArch;
        assert!(a.is_entry_marker(0x0E));
        assert!(!a.is_entry_marker(0x01));
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p mtc-core vm::`
Expected: compile error — module/types don't exist.

- [ ] **Step 3: Implement the types**

`crates/core/src/vm/mod.rs`:
```rust
//! The processor VM: sans-I/O core, bus protocol, devices (spec §4).

pub mod arch;
pub mod bus;
pub mod trap;

pub use arch::{Arch, MicroOp, Operand, OperandKind};
pub use bus::{BusRequest, BusResponse, CoreEvent};
pub use trap::{DeviceFault, Trap};
```

`crates/core/src/vm/trap.rs`:
```rust
//! Traps: the processor's controlled stop on an execution error (spec §4.5).

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceFault {
    IndexOutsideAlphabet { index: u32 },
    StrictCellViolation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trap {
    InvalidOpcode { opcode: u8, at: u32 },
    CodeOutOfBounds { at: u32 },
    BadOperand { at: u32 },
    CallTargetNotEntry { target: u32 },
    StackOverflow,
    StackUnderflow,
    StepLimit,
    TactLimit,
    Device { fault: DeviceFault },
}

impl std::fmt::Display for Trap {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidOpcode { opcode, at } => {
                write!(f, "invalid opcode {opcode:#04x} at {at:#010x}")
            }
            Self::CodeOutOfBounds { at } => write!(f, "execution left the code image at {at:#010x}"),
            Self::BadOperand { at } => write!(f, "malformed operand at {at:#010x}"),
            Self::CallTargetNotEntry { target } => {
                write!(f, "call target {target:#010x} is not an entry marker")
            }
            Self::StackOverflow => write!(f, "return-stack overflow"),
            Self::StackUnderflow => write!(f, "return-stack underflow"),
            Self::StepLimit => write!(f, "step limit exceeded"),
            Self::TactLimit => write!(f, "tact limit exceeded"),
            Self::Device { fault } => write!(f, "device fault: {fault:?}"),
        }
    }
}
```

`crates/core/src/vm/bus.rs`:
```rust
//! Bus protocol between the sans-I/O core and its driver (spec §4).

use super::trap::{DeviceFault, Trap};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BusRequest {
    CodeRead { addr: u32 },
    StackPush { value: u32 },
    StackPop,
    DeviceMoveLeft { dev: u8 },
    DeviceMoveRight { dev: u8 },
    DeviceRead { dev: u8 },
    DeviceWrite { dev: u8, index: u32 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BusResponse {
    Byte(u8),
    OutOfCode,
    Ok,
    StackFull,
    Value(u32),
    StackEmpty,
    Symbol(u32),
    Fault(DeviceFault),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoreEvent {
    Request(BusRequest),
    Step,
    Stopped,
    Halted,
    Trapped(Trap),
}
```

`crates/core/src/vm/arch.rs` (above the fixture/tests from Step 1):
```rust
//! The architecture interface: all instruction knowledge enters here (spec §10).

use super::trap::Trap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperandKind {
    None,
    RelI8,
    RelI32,
    /// Self-delimiting symbol vector: 7-bit payloads, high bit on the last.
    SymbolVec,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Operand {
    None,
    I8(i8),
    I32(i32),
    Symbols(Vec<u32>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MicroOp {
    MoveLeft,
    MoveRight,
    Write(u32),
    LatchMatch(u32),
    JumpRel(i32),
    JumpRelIf { off: i32, when_match: bool },
    Call(i32),
    Ret,
    Stop,
    Halt,
    Brk,
    Nop,
}

pub trait Arch {
    fn arch_id(&self) -> u8;
    /// `None` means: not an opcode of this architecture (trap on fetch).
    fn operand_kind(&self, opcode: u8) -> Option<OperandKind>;
    fn lower(&self, opcode: u8, operand: &Operand) -> Result<Vec<MicroOp>, Trap>;
    fn is_entry_marker(&self, byte: u8) -> bool;
}
```

In `crates/core/src/lib.rs` add:
```rust
pub mod vm;
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p mtc-core`
Expected: previous 29 + 3 new pass. (Expect `dead_code` on some enum variants until Tasks 4–5 consume them — if clippy objects, add a single `#![allow(dead_code)]` at the top of `vm/mod.rs` with the comment `// TODO(plan2a): remove when core.rs lands (Task 5)`, and remove it in Task 5.)

- [ ] **Step 5: Gates + commit**

Run: `cargo clippy --workspace --all-targets -- -D warnings && cargo fmt`
```bash
git add -A
git commit -m "feat(core): vm bus protocol, trap taxonomy, Arch trait with test arch"
```

---

### Task 2: `InfiniteBelt` — paged sparse tape

**Files:**
- Create: `crates/core/src/vm/devices/mod.rs`
- Create: `crates/core/src/vm/devices/infinite_belt.rs`
- Modify: `crates/core/src/vm/mod.rs` (add `pub mod devices;`)

**Interfaces:**
- Consumes: `DeviceFault` from Task 1.
- Produces:
  ```rust
  pub trait Tape {
      fn alphabet_size(&self) -> u32;
      fn left(&mut self);
      fn right(&mut self);
      fn read(&self) -> u32;
      fn write(&mut self, index: u32) -> Result<(), DeviceFault>;
  }
  pub struct InfiniteBelt { /* pages: HashMap<i64, u64>, head: i64, cached page key+value */ }
  impl InfiniteBelt {
      pub fn new() -> Self;                                          // blank tape, head 0
      pub fn from_cells(cells: impl IntoIterator<Item = bool>, first_cell_at: i64, head: i64) -> Self;
      pub fn head(&self) -> i64;
      pub fn page_count(&self) -> usize;                             // observability for the sparse guarantees
      pub fn marked_cells(&self) -> Vec<i64>;                        // sorted, for tests/snapshots
  }
  ```
- Page geometry: 64 cells per page (`u64` bitmask); page key = `coord.div_euclid(64)`, bit = `coord.rem_euclid(64)`.

- [ ] **Step 1: Write the failing tests**

Test module at the bottom of `infinite_belt.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blank_tape_reads_zero_everywhere_without_allocating() {
        let mut belt = InfiniteBelt::new();
        for _ in 0..10_000 {
            belt.right();
            assert_eq!(belt.read(), 0);
        }
        for _ in 0..20_000 {
            belt.left();
            assert_eq!(belt.read(), 0);
        }
        assert_eq!(belt.page_count(), 0); // reads never allocate
        assert_eq!(belt.head(), -10_000);
    }

    #[test]
    fn write_read_round_trip_across_page_boundaries() {
        let mut belt = InfiniteBelt::new();
        // mark cells -1, 0, 63, 64 (spans three pages: -1, 0, 1)
        for target in [-1i64, 0, 63, 64] {
            while belt.head() < target {
                belt.right();
            }
            while belt.head() > target {
                belt.left();
            }
            belt.write(1).unwrap();
        }
        assert_eq!(belt.marked_cells(), vec![-1, 0, 63, 64]);
        assert_eq!(belt.page_count(), 3);
    }

    #[test]
    fn erasing_last_mark_frees_the_page() {
        let mut belt = InfiniteBelt::new();
        belt.write(1).unwrap();
        assert_eq!(belt.page_count(), 1);
        belt.write(0).unwrap();
        assert_eq!(belt.page_count(), 0);
    }

    #[test]
    fn idempotent_writes_are_ok() {
        let mut belt = InfiniteBelt::new();
        belt.write(1).unwrap();
        belt.write(1).unwrap(); // marking a marked cell: fine on a default belt
        assert_eq!(belt.read(), 1);
        belt.write(0).unwrap();
        belt.write(0).unwrap();
        assert_eq!(belt.read(), 0);
    }

    #[test]
    fn out_of_alphabet_write_faults() {
        let mut belt = InfiniteBelt::new();
        assert_eq!(
            belt.write(2),
            Err(DeviceFault::IndexOutsideAlphabet { index: 2 })
        );
    }

    #[test]
    fn from_cells_places_data_and_head() {
        let belt = InfiniteBelt::from_cells([false, true, true, false, true], 0, 2);
        assert_eq!(belt.marked_cells(), vec![1, 2, 4]);
        assert_eq!(belt.head(), 2);
        assert_eq!(belt.read(), 1);
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p mtc-core infinite_belt`
Expected: compile error.

- [ ] **Step 3: Implement**

`crates/core/src/vm/devices/mod.rs`:
```rust
//! Tape devices behind the device bus (spec §4.2). Index-based; the
//! processor never sees glyphs and never knows the head position.

mod infinite_belt;

pub use infinite_belt::InfiniteBelt;

use super::trap::DeviceFault;

pub trait Tape {
    fn alphabet_size(&self) -> u32;
    fn left(&mut self);
    fn right(&mut self);
    fn read(&self) -> u32;
    fn write(&mut self, index: u32) -> Result<(), DeviceFault>;
}
```

`crates/core/src/vm/devices/infinite_belt.rs`:
```rust
//! Unbounded two-symbol belt with paged sparse storage (spec §4.2):
//! `TBelt`'s packed bit array, generalized to an infinite tape.

use std::collections::HashMap;

use super::Tape;
use crate::vm::trap::DeviceFault;

const PAGE_BITS: i64 = 64;

#[derive(Debug, Default)]
pub struct InfiniteBelt {
    pages: HashMap<i64, u64>,
    head: i64,
}

impl InfiniteBelt {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_cells(
        cells: impl IntoIterator<Item = bool>,
        first_cell_at: i64,
        head: i64,
    ) -> Self {
        let mut belt = Self { pages: HashMap::new(), head };
        for (i, marked) in cells.into_iter().enumerate() {
            if marked {
                belt.set(first_cell_at + i as i64, true);
            }
        }
        belt
    }

    pub fn head(&self) -> i64 {
        self.head
    }

    pub fn page_count(&self) -> usize {
        self.pages.len()
    }

    pub fn marked_cells(&self) -> Vec<i64> {
        let mut out = Vec::new();
        for (&page, &bits) in &self.pages {
            for bit in 0..PAGE_BITS {
                if bits & (1u64 << bit) != 0 {
                    out.push(page * PAGE_BITS + bit);
                }
            }
        }
        out.sort_unstable();
        out
    }

    fn get(&self, coord: i64) -> bool {
        let page = coord.div_euclid(PAGE_BITS);
        let bit = coord.rem_euclid(PAGE_BITS);
        self.pages
            .get(&page)
            .is_some_and(|bits| bits & (1u64 << bit) != 0)
    }

    fn set(&mut self, coord: i64, marked: bool) {
        let page = coord.div_euclid(PAGE_BITS);
        let bit = coord.rem_euclid(PAGE_BITS);
        if marked {
            *self.pages.entry(page).or_insert(0) |= 1u64 << bit;
        } else if let Some(bits) = self.pages.get_mut(&page) {
            *bits &= !(1u64 << bit);
            if *bits == 0 {
                self.pages.remove(&page); // freed: memory stays O(non-blank pages)
            }
        }
    }
}

impl Tape for InfiniteBelt {
    fn alphabet_size(&self) -> u32 {
        2
    }

    fn left(&mut self) {
        self.head -= 1;
    }

    fn right(&mut self) {
        self.head += 1;
    }

    fn read(&self) -> u32 {
        u32::from(self.get(self.head))
    }

    fn write(&mut self, index: u32) -> Result<(), DeviceFault> {
        if index >= self.alphabet_size() {
            return Err(DeviceFault::IndexOutsideAlphabet { index });
        }
        self.set(self.head, index == 1);
        Ok(())
    }
}
```

In `crates/core/src/vm/mod.rs` add `pub mod devices;` (and re-export `pub use devices::Tape;`).

Note: the spec's current-page cache is a performance refinement; land the correct `HashMap` version now and only add the cache if a later plan's profiling wants it (the sparse *guarantees* — no-alloc reads, page freeing — are the contract; the cache is not observable). Record this as a deviation-by-simplification in your report.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p mtc-core`
Expected: all pass.

- [ ] **Step 5: Gates + commit**

Run: `cargo clippy --workspace --all-targets -- -D warnings && cargo fmt`
```bash
git add -A
git commit -m "feat(core): Tape trait and paged-sparse InfiniteBelt"
```

---

### Task 3: `AnnularBelt` + `StrictBelt`

**Files:**
- Create: `crates/core/src/vm/devices/annular_belt.rs`
- Create: `crates/core/src/vm/devices/strict_belt.rs`
- Modify: `crates/core/src/vm/devices/mod.rs`

**Interfaces:**
- Consumes: `Tape`, `DeviceFault`.
- Produces:
  ```rust
  pub struct AnnularBelt { /* words: Vec<u64>, size: u32, head: u32 */ }
  impl AnnularBelt { pub fn new(size: u32) -> Self /* panics if size == 0 */; pub fn head(&self) -> u32; }
  pub struct StrictBelt<T: Tape>(/* inner */);
  impl<T: Tape> StrictBelt<T> { pub fn new(inner: T) -> Self; pub fn into_inner(self) -> T; }
  ```
- `AnnularBelt` is the historical ring (`TBelt`, default construction size 2048 is the caller's choice — no default constant here); head wraps: `left` from 0 → `size-1`, `right` from `size-1` → 0.
- `StrictBelt` implements 2006/2007 semantics: `write(i)` when the cell already holds `i` → `Err(DeviceFault::StrictCellViolation)`; everything else delegates.

- [ ] **Step 1: Write the failing tests**

In `annular_belt.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::vm::devices::Tape;
    use crate::vm::trap::DeviceFault;

    #[test]
    fn wraps_both_directions() {
        let mut belt = AnnularBelt::new(4);
        assert_eq!(belt.head(), 0);
        belt.left();
        assert_eq!(belt.head(), 3);
        belt.right();
        belt.right();
        belt.right();
        belt.right();
        belt.right();
        assert_eq!(belt.head(), 1);
    }

    #[test]
    fn a_full_lap_returns_to_written_cell() {
        let mut belt = AnnularBelt::new(100);
        belt.write(1).unwrap();
        for _ in 0..100 {
            belt.right();
        }
        assert_eq!(belt.read(), 1); // the wrap detector's mark, found again
    }

    #[test]
    fn spans_multiple_words() {
        let mut belt = AnnularBelt::new(130); // 3 u64 words
        for _ in 0..129 {
            belt.right();
        }
        belt.write(1).unwrap();
        assert_eq!(belt.head(), 129);
        assert_eq!(belt.read(), 1);
        belt.right(); // wraps to 0
        assert_eq!(belt.read(), 0);
    }

    #[test]
    fn out_of_alphabet_faults() {
        let mut belt = AnnularBelt::new(8);
        assert_eq!(belt.write(7), Err(DeviceFault::IndexOutsideAlphabet { index: 7 }));
    }
}
```

In `strict_belt.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::vm::devices::{InfiniteBelt, Tape};
    use crate::vm::trap::DeviceFault;

    #[test]
    fn double_mark_and_double_erase_fault() {
        let mut belt = StrictBelt::new(InfiniteBelt::new());
        assert_eq!(belt.write(0), Err(DeviceFault::StrictCellViolation)); // erase blank
        belt.write(1).unwrap();
        assert_eq!(belt.write(1), Err(DeviceFault::StrictCellViolation)); // mark marked
        belt.write(0).unwrap();
    }

    #[test]
    fn moves_and_reads_delegate() {
        let mut belt = StrictBelt::new(InfiniteBelt::new());
        belt.write(1).unwrap();
        belt.right();
        assert_eq!(belt.read(), 0);
        belt.left();
        assert_eq!(belt.read(), 1);
        assert_eq!(belt.alphabet_size(), 2);
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p mtc-core annular` — expected: compile error.

- [ ] **Step 3: Implement**

`crates/core/src/vm/devices/annular_belt.rs`:
```rust
//! Ring-shaped bounded belt — the historical `TBelt` (spec §4.2).

use super::Tape;
use crate::vm::trap::DeviceFault;

#[derive(Debug)]
pub struct AnnularBelt {
    words: Vec<u64>,
    size: u32,
    head: u32,
}

impl AnnularBelt {
    pub fn new(size: u32) -> Self {
        assert!(size > 0, "annular belt needs at least one cell");
        Self {
            words: vec![0; size.div_ceil(64) as usize],
            size,
            head: 0,
        }
    }

    pub fn head(&self) -> u32 {
        self.head
    }

    fn get(&self, at: u32) -> bool {
        self.words[(at / 64) as usize] & (1u64 << (at % 64)) != 0
    }
}

impl Tape for AnnularBelt {
    fn alphabet_size(&self) -> u32 {
        2
    }

    fn left(&mut self) {
        self.head = if self.head == 0 { self.size - 1 } else { self.head - 1 };
    }

    fn right(&mut self) {
        self.head = if self.head == self.size - 1 { 0 } else { self.head + 1 };
    }

    fn read(&self) -> u32 {
        u32::from(self.get(self.head))
    }

    fn write(&mut self, index: u32) -> Result<(), DeviceFault> {
        if index >= self.alphabet_size() {
            return Err(DeviceFault::IndexOutsideAlphabet { index });
        }
        let word = &mut self.words[(self.head / 64) as usize];
        let bit = 1u64 << (self.head % 64);
        if index == 1 {
            *word |= bit;
        } else {
            *word &= !bit;
        }
        Ok(())
    }
}
```

`crates/core/src/vm/devices/strict_belt.rs`:
```rust
//! Strict-cells decorator: 2006/2007 semantics — writing the value a
//! cell already holds is an error (spec §4.2).

use super::Tape;
use crate::vm::trap::DeviceFault;

#[derive(Debug)]
pub struct StrictBelt<T: Tape> {
    inner: T,
}

impl<T: Tape> StrictBelt<T> {
    pub fn new(inner: T) -> Self {
        Self { inner }
    }

    pub fn into_inner(self) -> T {
        self.inner
    }
}

impl<T: Tape> Tape for StrictBelt<T> {
    fn alphabet_size(&self) -> u32 {
        self.inner.alphabet_size()
    }

    fn left(&mut self) {
        self.inner.left();
    }

    fn right(&mut self) {
        self.inner.right();
    }

    fn read(&self) -> u32 {
        self.inner.read()
    }

    fn write(&mut self, index: u32) -> Result<(), DeviceFault> {
        if index < self.alphabet_size() && self.inner.read() == index {
            return Err(DeviceFault::StrictCellViolation);
        }
        self.inner.write(index)
    }
}
```

In `devices/mod.rs` add the modules and re-exports:
```rust
mod annular_belt;
mod strict_belt;

pub use annular_belt::AnnularBelt;
pub use strict_belt::StrictBelt;
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p mtc-core`
Expected: all pass.

- [ ] **Step 5: Gates + commit**

Run: `cargo clippy --workspace --all-targets -- -D warnings && cargo fmt`
```bash
git add -A
git commit -m "feat(core): AnnularBelt ring tape and StrictBelt decorator"
```

---

### Task 4: Core fetch — opcode + operand collection

**Files:**
- Create: `crates/core/src/vm/core.rs`
- Modify: `crates/core/src/vm/mod.rs` (add `pub mod core;` and `pub use core::Core;`)

**Interfaces:**
- Consumes: everything from Task 1.
- Produces: `Core` with `new(arch, entry)`, `start()`, `resume(BusResponse) -> CoreEvent`, `ip()`, `mf()`, `set_mf()`. After this task the core can fetch a full instruction (opcode + operand of any `OperandKind`) and traps on invalid opcodes / `OutOfCode` / bad self-delimiting vectors; execution of micro-ops is Task 5 (until then, a fetched instruction immediately yields `CoreEvent::Step` with lowering stored but unexecuted — the Task 5 diff replaces that stub).

- [ ] **Step 1: Write the failing tests**

Test module at the bottom of `core.rs`. The scripted-driver helper is the heart of all core testing — transcribe it exactly:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::vm::arch::test_arch::TestArch;
    use crate::vm::bus::{BusRequest as Rq, BusResponse as Rs, CoreEvent as Ev};
    use crate::vm::trap::Trap;

    /// Drive the core with a scripted byte image; panics if the core asks
    /// for anything but code during fetch. Returns the first non-Request
    /// event and the addresses the core fetched.
    fn run_fetch(code: &[u8], entry: u32) -> (Ev, Vec<u32>) {
        let arch = TestArch;
        let mut core = Core::new(&arch, entry);
        let mut fetched = Vec::new();
        let mut ev = core.start();
        loop {
            match ev {
                Ev::Request(Rq::CodeRead { addr }) => {
                    fetched.push(addr);
                    let resp = match code.get(addr as usize) {
                        Some(&b) => Rs::Byte(b),
                        None => Rs::OutOfCode,
                    };
                    ev = core.resume(resp);
                }
                other => return (other, fetched),
            }
        }
    }

    #[test]
    fn fetches_single_byte_instruction() {
        // 0x01 = nop (no operand) — Task 4 stub yields Step after fetch
        let (ev, fetched) = run_fetch(&[0x01], 0);
        assert_eq!(ev, Ev::Step);
        assert_eq!(fetched, vec![0]);
    }

    #[test]
    fn fetches_rel8_operand() {
        // 0x08 = jmp rel8; operand byte follows
        let (ev, fetched) = run_fetch(&[0x08, 0x05], 0);
        assert_eq!(ev, Ev::Step);
        assert_eq!(fetched, vec![0, 1]);
    }

    #[test]
    fn fetches_rel32_operand_little_endian() {
        let (ev, fetched) = run_fetch(&[0x09, 0x01, 0x00, 0x00, 0x00], 0);
        assert_eq!(ev, Ev::Step);
        assert_eq!(fetched, vec![0, 1, 2, 3, 4]);
    }

    #[test]
    fn fetches_symbol_vec_until_high_bit() {
        // 0x07 = wr(vec); TestArch requires exactly one element;
        // 0x81 = payload 1 with high bit (last element)
        let (ev, fetched) = run_fetch(&[0x07, 0x81], 0);
        assert_eq!(ev, Ev::Step);
        assert_eq!(fetched, vec![0, 1]);
    }

    #[test]
    fn multi_element_symbol_vec_is_collected_then_arch_rejects() {
        // 0x01 (no high bit, more follow) then 0x82 (last) → 2 elements;
        // TestArch's wr wants exactly 1 → BadOperand
        let (ev, fetched) = run_fetch(&[0x07, 0x01, 0x82], 0);
        assert!(matches!(ev, Ev::Trapped(Trap::BadOperand { .. })));
        assert_eq!(fetched, vec![0, 1, 2]);
    }

    #[test]
    fn invalid_opcode_traps_with_location() {
        let (ev, _) = run_fetch(&[0x55], 0);
        assert_eq!(ev, Ev::Trapped(Trap::InvalidOpcode { opcode: 0x55, at: 0 }));
    }

    #[test]
    fn out_of_code_during_fetch_traps() {
        let (ev, _) = run_fetch(&[], 0);
        assert!(matches!(ev, Ev::Trapped(Trap::CodeOutOfBounds { at: 0 })));
        // operand runs off the end:
        let (ev2, _) = run_fetch(&[0x09, 0x01], 0);
        assert!(matches!(ev2, Ev::Trapped(Trap::CodeOutOfBounds { .. })));
    }

    #[test]
    fn entry_offset_is_respected() {
        let (_, fetched) = run_fetch(&[0x00, 0x00, 0x01], 2);
        assert_eq!(fetched, vec![2]);
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p mtc-core vm::core` — expected: compile error.

- [ ] **Step 3: Implement the fetch machine**

`crates/core/src/vm/core.rs`:
```rust
//! The sans-I/O processor core (spec §4): a pure transition function
//! from bus responses to bus requests. Owns registers and the in-flight
//! instruction; performs no I/O; knows no opcodes (that's the Arch).

use super::arch::{Arch, MicroOp, Operand, OperandKind};
use super::bus::{BusRequest, BusResponse, CoreEvent};
use super::trap::Trap;

enum Phase {
    FetchOpcode,
    FetchOperand {
        opcode: u8,
        kind: OperandKind,
        buf: Vec<u8>,
    },
    /// Task 4 stub: lowering is stored, then Step is emitted without
    /// executing. Task 5 replaces this with real micro-op execution.
    Retire {
        #[allow(dead_code)] // consumed starting Task 5
        ops: Vec<MicroOp>,
    },
    Done,
}

pub struct Core<'a> {
    arch: &'a dyn Arch,
    ip: u32,
    instr_start: u32,
    mf: bool,
    phase: Phase,
}

impl<'a> Core<'a> {
    pub fn new(arch: &'a dyn Arch, entry: u32) -> Self {
        Self {
            arch,
            ip: entry,
            instr_start: entry,
            mf: false,
            phase: Phase::FetchOpcode,
        }
    }

    pub fn ip(&self) -> u32 {
        self.ip
    }

    pub fn mf(&self) -> bool {
        self.mf
    }

    /// The driver latches initial MF from the belt before the first resume.
    pub fn set_mf(&mut self, mf: bool) {
        self.mf = mf;
    }

    pub fn start(&mut self) -> CoreEvent {
        self.instr_start = self.ip;
        self.phase = Phase::FetchOpcode;
        self.code_read()
    }

    fn code_read(&self) -> CoreEvent {
        CoreEvent::Request(BusRequest::CodeRead { addr: self.ip })
    }

    fn trap(&mut self, trap: Trap) -> CoreEvent {
        self.phase = Phase::Done;
        CoreEvent::Trapped(trap)
    }

    pub fn resume(&mut self, resp: BusResponse) -> CoreEvent {
        match std::mem::replace(&mut self.phase, Phase::Done) {
            Phase::FetchOpcode => self.on_opcode(resp),
            Phase::FetchOperand { opcode, kind, buf } => {
                self.on_operand_byte(opcode, kind, buf, resp)
            }
            Phase::Retire { .. } => {
                // Task 4 stub: driver acknowledged Step with Ok — fetch next.
                self.start()
            }
            Phase::Done => self.trap(Trap::CodeOutOfBounds { at: self.ip }),
        }
    }

    fn on_opcode(&mut self, resp: BusResponse) -> CoreEvent {
        let byte = match resp {
            BusResponse::Byte(b) => b,
            BusResponse::OutOfCode => {
                return self.trap(Trap::CodeOutOfBounds { at: self.ip })
            }
            _ => return self.trap(Trap::CodeOutOfBounds { at: self.ip }),
        };
        let Some(kind) = self.arch.operand_kind(byte) else {
            return self.trap(Trap::InvalidOpcode { opcode: byte, at: self.ip });
        };
        self.ip += 1;
        match kind {
            OperandKind::None => self.finish_fetch(byte, Operand::None),
            _ => {
                self.phase = Phase::FetchOperand { opcode: byte, kind, buf: Vec::new() };
                self.code_read()
            }
        }
    }

    fn on_operand_byte(
        &mut self,
        opcode: u8,
        kind: OperandKind,
        mut buf: Vec<u8>,
        resp: BusResponse,
    ) -> CoreEvent {
        let byte = match resp {
            BusResponse::Byte(b) => b,
            _ => return self.trap(Trap::CodeOutOfBounds { at: self.ip }),
        };
        buf.push(byte);
        self.ip += 1;
        let complete = match kind {
            OperandKind::None => true, // unreachable by construction
            OperandKind::RelI8 => buf.len() == 1,
            OperandKind::RelI32 => buf.len() == 4,
            OperandKind::SymbolVec => byte & 0x80 != 0,
        };
        if !complete {
            self.phase = Phase::FetchOperand { opcode, kind, buf };
            return self.code_read();
        }
        let operand = match kind {
            OperandKind::None => Operand::None,
            OperandKind::RelI8 => Operand::I8(buf[0] as i8),
            OperandKind::RelI32 => {
                Operand::I32(i32::from_le_bytes(buf[..4].try_into().unwrap()))
            }
            OperandKind::SymbolVec => {
                Operand::Symbols(buf.iter().map(|b| u32::from(b & 0x7F)).collect())
            }
        };
        self.finish_fetch(opcode, operand)
    }

    fn finish_fetch(&mut self, opcode: u8, operand: Operand) -> CoreEvent {
        match self.arch.lower(opcode, &operand) {
            Ok(ops) => {
                // Task 4 stub: store lowering, retire immediately.
                self.phase = Phase::Retire { ops };
                CoreEvent::Step
            }
            Err(_) => self.trap(Trap::BadOperand { at: self.instr_start }),
        }
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p mtc-core`
Expected: all pass (Task 1's `#![allow(dead_code)]` in `vm/mod.rs`, if added, can likely be narrowed now — gates decide).

- [ ] **Step 5: Gates + commit**

Run: `cargo clippy --workspace --all-targets -- -D warnings && cargo fmt`
```bash
git add -A
git commit -m "feat(core): sans-io core fetch phase (opcode, rel8/rel32, symbol vectors)"
```

---

### Task 5: Core execute — micro-ops, MF, jumps, call/ret

**Files:**
- Modify: `crates/core/src/vm/core.rs` (replace the `Retire` stub with real execution)

**Interfaces:**
- Consumes: Tasks 1 and 4.
- Produces: the full core protocol of the plan header. Semantics implemented here, exactly:
  - `MoveLeft`/`MoveRight` → `DeviceMoveLeft/Right { dev: 0 }`, expect `Ok` (a `Fault(f)` response traps `Device { fault: f }` — applies to every device request).
  - `Write(i)` → `DeviceWrite { dev: 0, index: i }`, expect `Ok`.
  - `LatchMatch(idx)` → `DeviceRead { dev: 0 }`; on `Symbol(s)`: `mf = (s == idx)`.
  - `JumpRel(off)` / taken `JumpRelIf` → `ip = instr_end + off` where `instr_end` is the address after the instruction's last byte (spec §5: relative to end); computed in `i64`; negative or `> u32::MAX` → `CodeOutOfBounds` trap. Untaken conditional falls through (ip already = instr_end).
  - `Call(off)` → compute target as above; `CodeRead { addr: target }`; response byte must satisfy `arch.is_entry_marker` else `Trapped(CallTargetNotEntry { target })`; then `StackPush { value: instr_end }` (`StackFull` → `StackOverflow` trap); then `ip = target`.
  - `Ret` → `StackPop`; `Value(v)` → `ip = v`; `StackEmpty` → `StackUnderflow` trap.
  - `Stop` → `CoreEvent::Stopped`; `Halt` → `CoreEvent::Halted`; `Brk`/`Nop` → nothing (Brk pauses only under a debugger — later plan).
  - After the last micro-op: `CoreEvent::Step`; driver resumes `Ok` → next fetch.

- [ ] **Step 1: Write the failing tests**

Append to `core.rs`'s test module (keep the Task 4 tests; update the two that asserted the stub — `fetches_single_byte_instruction` and friends still expect `Ev::Step` for nop, which remains correct; only `multi_element_symbol_vec…` and trap tests are unaffected). New scripted driver with devices and stack:

```rust
    /// Full scripted driver: code image + tiny stack + fake device log.
    /// Returns (final event, request log, mf).
    fn run_full(
        code: &[u8],
        entry: u32,
        stack_cap: usize,
        device_symbols: &[u32], // successive DeviceRead answers
        max_steps: usize,
    ) -> (Ev, Vec<Rq>, bool) {
        let arch = TestArch;
        let mut core = Core::new(&arch, entry);
        let mut log = Vec::new();
        let mut stack: Vec<u32> = Vec::new();
        let mut reads = device_symbols.iter().copied();
        let mut steps = 0;
        let mut ev = core.start();
        loop {
            match ev {
                Ev::Request(rq) => {
                    log.push(rq);
                    let resp = match rq {
                        Rq::CodeRead { addr } => match code.get(addr as usize) {
                            Some(&b) => Rs::Byte(b),
                            None => Rs::OutOfCode,
                        },
                        Rq::StackPush { value } => {
                            if stack.len() == stack_cap {
                                Rs::StackFull
                            } else {
                                stack.push(value);
                                Rs::Ok
                            }
                        }
                        Rq::StackPop => match stack.pop() {
                            Some(v) => Rs::Value(v),
                            None => Rs::StackEmpty,
                        },
                        Rq::DeviceRead { .. } => Rs::Symbol(reads.next().unwrap_or(0)),
                        Rq::DeviceMoveLeft { .. }
                        | Rq::DeviceMoveRight { .. }
                        | Rq::DeviceWrite { .. } => Rs::Ok,
                    };
                    ev = core.resume(resp);
                }
                Ev::Step => {
                    steps += 1;
                    if steps >= max_steps {
                        return (Ev::Step, log, core.mf());
                    }
                    ev = core.resume(Rs::Ok);
                }
                terminal => return (terminal, log, core.mf()),
            }
        }
    }

    #[test]
    fn move_write_latch_sequence_and_mf() {
        // right (move+latch reads 1 → mf=true), wr 0 (+latch reads 0 → mf=false), stop
        let code = [0x06, 0x07, 0x80, 0x02];
        let (ev, log, mf) = run_full(&code, 0, 4, &[1, 0], 100);
        assert_eq!(ev, Ev::Stopped);
        assert!(!mf);
        assert_eq!(
            log,
            vec![
                Rq::CodeRead { addr: 0 },
                Rq::DeviceMoveRight { dev: 0 },
                Rq::DeviceRead { dev: 0 },
                Rq::CodeRead { addr: 1 },
                Rq::CodeRead { addr: 2 },
                Rq::DeviceWrite { dev: 0, index: 0 },
                Rq::DeviceRead { dev: 0 },
                Rq::CodeRead { addr: 3 },
            ]
        );
    }

    #[test]
    fn conditional_jump_taken_and_untaken() {
        // 0x09 jm rel32: at entry mf=false → falls through to stop at 5;
        // then with initial mf=true → jumps back? Simpler: two programs.
        let fall = [0x09, 0x01, 0x00, 0x00, 0x00, 0x02, 0x02];
        let (ev, log, _) = run_full(&fall, 0, 4, &[], 100);
        assert_eq!(ev, Ev::Stopped);
        assert_eq!(*log.last().unwrap(), Rq::CodeRead { addr: 5 }); // fell through

        // taken: set mf via a latch first — right reads 1 → mf=true, then jm +1
        // layout: [0]=0x06 right, [1..6]=jm +1, [6]=halt (skipped), [7]=stop
        let taken = [0x06, 0x09, 0x01, 0x00, 0x00, 0x00, 0x03, 0x02];
        let (ev2, log2, _) = run_full(&taken, 0, 4, &[1], 100);
        assert_eq!(ev2, Ev::Stopped); // jumped over the halt at 6 to stop at 7
        assert!(log2.contains(&Rq::CodeRead { addr: 7 }));
        assert!(!log2.contains(&Rq::CodeRead { addr: 6 }) || matches!(ev2, Ev::Stopped));
    }

    #[test]
    fn unconditional_jump_targets_end_relative() {
        // jmp rel8 at 0: instr_end = 2; off = +1 → target 3 (skip halt at 2)
        let code = [0x08, 0x01, 0x03, 0x02];
        let (ev, log, _) = run_full(&code, 0, 4, &[], 100);
        assert_eq!(ev, Ev::Stopped);
        assert!(log.contains(&Rq::CodeRead { addr: 3 }));
    }

    #[test]
    fn negative_jump_target_traps() {
        let code = [0x08, 0x80]; // jmp rel8 -128 from instr_end 2 → -126
        let (ev, _, _) = run_full(&code, 0, 4, &[], 100);
        assert!(matches!(ev, Ev::Trapped(Trap::CodeOutOfBounds { .. })));
    }

    #[test]
    fn call_checks_entry_pushes_and_jumps_ret_returns() {
        // [0..5] call +1 → target 6 must hold 0x0E (entry) — instr_end 5, off 1
        // [5] stop  [6] 0x0E entry  [7] ret
        let code = [0x0A, 0x01, 0x00, 0x00, 0x00, 0x02, 0x0E, 0x0B];
        let (ev, log, _) = run_full(&code, 0, 4, &[], 100);
        assert_eq!(ev, Ev::Stopped);
        let call_check = Rq::CodeRead { addr: 6 }; // ent verification read
        let push = Rq::StackPush { value: 5 };
        let pos_check = log.iter().position(|r| *r == call_check).unwrap();
        let pos_push = log.iter().position(|r| *r == push).unwrap();
        assert!(pos_check < pos_push, "ent verified before push");
        assert!(log.contains(&Rq::StackPop));
        assert!(log.contains(&Rq::CodeRead { addr: 5 })); // returned to stop
    }

    #[test]
    fn call_to_non_entry_traps() {
        let code = [0x0A, 0x01, 0x00, 0x00, 0x00, 0x02, 0x01]; // target 6 = nop, not entry
        let (ev, _, _) = run_full(&code, 0, 4, &[], 100);
        assert_eq!(ev, Ev::Trapped(Trap::CallTargetNotEntry { target: 6 }));
    }

    #[test]
    fn stack_overflow_and_underflow_trap() {
        // capacity 0 stack: call overflows
        let code = [0x0A, 0x01, 0x00, 0x00, 0x00, 0x02, 0x0E, 0x0B];
        let (ev, _, _) = run_full(&code, 0, 0, &[], 100);
        assert_eq!(ev, Ev::Trapped(Trap::StackOverflow));
        // bare ret underflows
        let (ev2, _, _) = run_full(&[0x0B], 0, 4, &[], 100);
        assert_eq!(ev2, Ev::Trapped(Trap::StackUnderflow));
    }

    #[test]
    fn device_fault_becomes_trap() {
        let arch = TestArch;
        let mut core = Core::new(&arch, 0);
        let mut ev = core.start();
        // feed: opcode 0x07 (wr), operand 0x82 → Write(2) request → Fault
        ev = core.resume(Rs::Byte(0x07));
        ev = core.resume(Rs::Byte(0x82));
        let Ev::Request(Rq::DeviceWrite { index: 2, .. }) = ev else {
            panic!("expected write request, got {ev:?}");
        };
        let ev = core.resume(Rs::Fault(
            crate::vm::trap::DeviceFault::IndexOutsideAlphabet { index: 2 },
        ));
        assert!(matches!(
            ev,
            Ev::Trapped(Trap::Device {
                fault: crate::vm::trap::DeviceFault::IndexOutsideAlphabet { index: 2 }
            })
        ));
    }

    #[test]
    fn halt_and_brk_nop() {
        let (ev, _, _) = run_full(&[0x03], 0, 4, &[], 100);
        assert_eq!(ev, Ev::Halted);
        let (ev2, _, _) = run_full(&[0x04, 0x01, 0x02], 0, 4, &[], 100);
        assert_eq!(ev2, Ev::Stopped); // brk and nop are no-ops without a debugger
    }
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p mtc-core vm::core`
Expected: new tests fail (stub retires without executing — `Stopped` never emitted, request logs too short).

- [ ] **Step 3: Implement execution**

Replace the `Retire` stub in `core.rs`. The phase enum becomes:

```rust
enum Phase {
    FetchOpcode,
    FetchOperand { opcode: u8, kind: OperandKind, buf: Vec<u8> },
    Execute { ops: std::collections::VecDeque<MicroOp>, pending: Pending },
    StepAck,
    Done,
}

/// What the in-flight bus request was for.
enum Pending {
    None,
    Move,
    Write,
    Latch { match_index: u32 },
    EntCheck { target: u32 },
    Push { target: u32 },
    Pop,
}
```

`finish_fetch` becomes:
```rust
    fn finish_fetch(&mut self, opcode: u8, operand: Operand) -> CoreEvent {
        match self.arch.lower(opcode, &operand) {
            Ok(ops) => {
                self.phase = Phase::Execute { ops: ops.into(), pending: Pending::None };
                self.step_execute(BusResponse::Ok)
            }
            Err(_) => self.trap(Trap::BadOperand { at: self.instr_start }),
        }
    }
```

And the executor (new methods on `Core`; `resume` dispatches `Phase::Execute` to `step_execute(resp)` and `Phase::StepAck` to `self.start()`):

```rust
    fn step_execute(&mut self, resp: BusResponse) -> CoreEvent {
        let Phase::Execute { mut ops, pending } = std::mem::replace(&mut self.phase, Phase::Done)
        else {
            unreachable!("step_execute outside Execute phase");
        };

        // 1. Settle the in-flight request, if any.
        match pending {
            Pending::None => {}
            Pending::Move | Pending::Write => match resp {
                BusResponse::Ok => {}
                BusResponse::Fault(fault) => return self.trap(Trap::Device { fault }),
                _ => return self.trap(Trap::CodeOutOfBounds { at: self.ip }),
            },
            Pending::Latch { match_index } => match resp {
                BusResponse::Symbol(s) => self.mf = s == match_index,
                BusResponse::Fault(fault) => return self.trap(Trap::Device { fault }),
                _ => return self.trap(Trap::CodeOutOfBounds { at: self.ip }),
            },
            Pending::EntCheck { target } => match resp {
                BusResponse::Byte(b) if self.arch.is_entry_marker(b) => {
                    self.phase = Phase::Execute {
                        ops,
                        pending: Pending::Push { target },
                    };
                    return CoreEvent::Request(BusRequest::StackPush { value: self.ip });
                }
                BusResponse::Byte(_) | BusResponse::OutOfCode => {
                    return self.trap(Trap::CallTargetNotEntry { target })
                }
                _ => return self.trap(Trap::CodeOutOfBounds { at: self.ip }),
            },
            Pending::Push { target } => match resp {
                BusResponse::Ok => self.ip = target,
                BusResponse::StackFull => return self.trap(Trap::StackOverflow),
                _ => return self.trap(Trap::CodeOutOfBounds { at: self.ip }),
            },
            Pending::Pop => match resp {
                BusResponse::Value(v) => self.ip = v,
                BusResponse::StackEmpty => return self.trap(Trap::StackUnderflow),
                _ => return self.trap(Trap::CodeOutOfBounds { at: self.ip }),
            },
        }

        // 2. Issue the next micro-op.
        while let Some(op) = ops.pop_front() {
            let (request, pending) = match op {
                MicroOp::Nop | MicroOp::Brk => continue,
                MicroOp::Stop => {
                    self.phase = Phase::Done;
                    return CoreEvent::Stopped;
                }
                MicroOp::Halt => {
                    self.phase = Phase::Done;
                    return CoreEvent::Halted;
                }
                MicroOp::MoveLeft => (BusRequest::DeviceMoveLeft { dev: 0 }, Pending::Move),
                MicroOp::MoveRight => (BusRequest::DeviceMoveRight { dev: 0 }, Pending::Move),
                MicroOp::Write(index) => {
                    (BusRequest::DeviceWrite { dev: 0, index }, Pending::Write)
                }
                MicroOp::LatchMatch(match_index) => {
                    (BusRequest::DeviceRead { dev: 0 }, Pending::Latch { match_index })
                }
                MicroOp::JumpRel(off) => {
                    match self.jump_target(off) {
                        Ok(t) => self.ip = t,
                        Err(trap) => return self.trap(trap),
                    }
                    continue;
                }
                MicroOp::JumpRelIf { off, when_match } => {
                    if self.mf == when_match {
                        match self.jump_target(off) {
                            Ok(t) => self.ip = t,
                            Err(trap) => return self.trap(trap),
                        }
                    }
                    continue;
                }
                MicroOp::Call(off) => match self.jump_target(off) {
                    Ok(target) => (
                        BusRequest::CodeRead { addr: target },
                        Pending::EntCheck { target },
                    ),
                    Err(trap) => return self.trap(trap),
                },
                MicroOp::Ret => (BusRequest::StackPop, Pending::Pop),
            };
            self.phase = Phase::Execute { ops, pending };
            return CoreEvent::Request(request);
        }

        // 3. Instruction retired.
        self.phase = Phase::StepAck;
        CoreEvent::Step
    }

    /// Operands are relative to the END of the instruction (spec §5);
    /// at execute time `self.ip` == instr_end (fetch advanced it).
    fn jump_target(&self, off: i32) -> Result<u32, Trap> {
        let target = i64::from(self.ip) + i64::from(off);
        u32::try_from(target).map_err(|_| Trap::CodeOutOfBounds { at: self.instr_start })
    }
```

(`resume`'s `Phase::Retire` arm and the `Retire` variant are deleted; `Phase::StepAck` resumes into `self.start()`. Remove any `dead_code` allow left from Task 1/4 — everything is consumed now.)

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p mtc-core`
Expected: all pass — Task 4's fetch tests still green (nop path unchanged), all new execute tests green.

- [ ] **Step 5: Gates + commit**

Run: `cargo clippy --workspace --all-targets -- -D warnings && cargo fmt`
```bash
git add -A
git commit -m "feat(core): sans-io core execute phase (micro-ops, MF, jumps, call/ret)"
```

---

## Self-Review Notes

- **Spec coverage (this plan's slice):** §4 registers (IP, MF here; SP lives with the driver's stack — Plan 2b), §4 buses + sans-I/O contract ✓, §4.1 MF-as-match + LatchMatch ✓, §4.2 Tape trait (index-based, write faults) + InfiniteBelt guarantees + AnnularBelt + StrictBelt ✓, §5 operand encodings (rel-to-end i8/i32, self-delimiting vectors) + ent verification ✓. Deliberately Plan 2b: PM-1 opcode table, sync driver + tact/wait-state accounting + `Step` bookkeeping, loader/registry/Machine, FLAGS-beyond-MF presentation, `.pmb` integration, end-to-end programs.
- **Type consistency:** `Trap`/`DeviceFault`/`BusRequest`/`BusResponse`/`CoreEvent`/`OperandKind`/`Operand`/`MicroOp`/`Arch` names match between the header block, Task 1 code, and Tasks 4–5 usage. `TestArch` opcodes used in Tasks 4–5 tests match its Task 1 table.
- **Known simplifications, on record:** no current-page cache in `InfiniteBelt` (guarantees hold without it; noted in Task 2); `dev` is always 0 in v1 micro-ops (multi-device is TM-1); initial-MF latch is the driver's job (Plan 2b) — core starts `mf = false` and exposes `set_mf`.
