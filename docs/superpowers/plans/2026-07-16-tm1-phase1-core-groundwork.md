# TM-1 Phase 1: Core Groundwork Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Extend the arch-agnostic VM core with everything TM-1 needs from the
core layer — tape-indexed micro-ops, the MR match register (unifying PM-1's
MF), the TR tuple bank, the table-match engine (`MatchTable`/`DispatchJump`
over table ROM via a new `TableRead` bus request), new trap kinds, a
multi-device driver, and tact prices — while PM-1 behavior stays byte-identical.

**Architecture:** Spec: `docs/superpowers/specs/2026-07-16-tm1-and-tmt-design.md`
§7 (VM), §4 (match tables), §3.5 (traps), §17 phase 1. All changes live in
`crates/core`; the only non-core edits are mechanical variant-shape updates in
PM-1's arch module and driver call sites. The table walk is a pure state
machine in a new `vm/table.rs`, driven by the core through `TableRead`
requests, so it is unit-testable without the core. Frames/`retx`, MX v2, and
the n-byte row codec are NOT in this phase (phases 5, 3, 3 respectively).

**Tech Stack:** Rust, cargo workspace; no new dependencies (serde/proptest
stay as they are); tests via `cargo test`.

## Global Constraints

- `cargo test --workspace` must be green at the end of every task — the PM-1
  suite doubles as the byte-identity regression gate (spec §16: "PM-1
  regression gate: byte-identical behavior after the MR unification").
- `cargo clippy --workspace --all-targets -- -D warnings` and
  `cargo fmt --check` must pass before every commit.
- `crates/core` must contain zero PM-1/TM-1 knowledge: new mechanisms are
  exercised by the crate-private fake arch (`test_arch`, id `0x7F`) only.
- No new dependencies.
- Commit style: conventional commits with scope (`feat(core):`,
  `test(core):`, `fix(post-machine):`).
- Commits require the maintainer's explicit go-ahead in the executing
  session (repo rule); if not yet granted, stop at the commit step and ask.
- Table encodings implemented here are the compact family only (one byte per
  row position, payload `0x7F` = wildcard). N-byte rows arrive with the wider
  codec work in phase 3 (spec §3.2) — do not implement them here.

---

### Task 1: MR register unification (MF becomes the 1-bit view of MR)

Spec §7 item 2: the core register becomes `mr: u32`; PM-1's `LatchMatch`
writes 0/1; `jm`/`jnm` test `MR ≠ 0`. Public `mf()`/`set_mf()` keep their
exact semantics so the driver and PM-1 stay untouched.

**Files:**
- Modify: `crates/core/src/vm/core.rs` (struct `Core` ~line 35–74, `Pending::Latch` settle ~line 196–200, `JumpRelIf` ~line 258–266)
- Test: `crates/core/src/vm/core.rs` (inline `mod tests`)

**Interfaces:**
- Consumes: existing `Core` API.
- Produces: `Core::mr(&self) -> u32`, `Core::set_mr(&mut self, u32)`;
  `mf()`/`set_mf(bool)` unchanged in signature and meaning (`mf() == (mr != 0)`,
  `set_mf(b)` sets `mr` to `1`/`0`). Task 8 relies on `mr`/`set_mr`.

- [ ] **Step 1: Write the failing test** (append to `mod tests` in `core.rs`)

```rust
    /// MR is the general register; MF is its boolean view (spec: MF ≡ MR≠0).
    #[test]
    fn mr_generalizes_mf() {
        let arch = TestArch;
        let mut core = Core::new(&arch, 0);
        assert_eq!(core.mr(), 0);
        assert!(!core.mf());
        core.set_mr(5);
        assert!(core.mf());
        core.set_mf(true);
        assert_eq!(core.mr(), 1);
        core.set_mf(false);
        assert_eq!(core.mr(), 0);
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p mtc-core mr_generalizes_mf`
Expected: compile error — `no method named mr found for struct Core`.

- [ ] **Step 3: Implement**

In `struct Core`, replace the field `mf: bool` with `mr: u32` (and the
`Core::new` initializer `mf: false` with `mr: 0`). Replace the accessor block:

```rust
    pub fn mf(&self) -> bool {
        self.mr != 0
    }

    /// The driver latches initial MF from the tape before the first resume.
    pub fn set_mf(&mut self, mf: bool) {
        self.mr = u32::from(mf);
    }

    /// The match register (docs/isa.md (registers)): 0 = no row matched.
    /// MF is formally `MR != 0`; PM-1 only ever sees 0/1 here.
    pub fn mr(&self) -> u32 {
        self.mr
    }

    pub fn set_mr(&mut self, mr: u32) {
        self.mr = mr;
    }
```

Update the two register uses:
- `Pending::Latch` settle arm: `self.mf = s == match_index;` →
  `self.mr = u32::from(s == match_index);`
- `MicroOp::JumpRelIf` arm: `if self.mf == when_match {` →
  `if (self.mr != 0) == when_match {`

- [ ] **Step 4: Run tests**

Run: `cargo test -p mtc-core` then `cargo test --workspace`
Expected: all PASS (PM-1 behavior unchanged — this is the regression claim).

- [ ] **Step 5: Commit**

```bash
git add crates/core/src/vm/core.rs
git commit -m "feat(core): unify MF into a u32 match register MR (MF = MR != 0)"
```

---

### Task 2: Tape-indexed micro-ops

Spec §7 item 1. The tape micro-ops gain a `dev` field; `dev: 0` is no longer
hardcoded in the core. PM-1's arch module and the fake arch emit `dev: 0`
explicitly.

**Files:**
- Modify: `crates/core/src/vm/arch.rs` (enum `MicroOp`, `test_arch::lower`)
- Modify: `crates/core/src/vm/core.rs` (`step_execute` micro-op dispatch ~line 242–250)
- Modify: `crates/post-machine/src/arch/mod.rs` (every `MicroOp::MoveLeft` / `MoveRight` / `Write(..)` construction)
- Test: `crates/core/src/vm/core.rs` (inline `mod tests`)

**Interfaces:**
- Produces (relied on by tasks 4, 8 and phase 4):
  `MicroOp::MoveLeft { dev: u8 }`, `MicroOp::MoveRight { dev: u8 }`,
  `MicroOp::Write { dev: u8, index: u32 }`. `LatchMatch(u32)` keeps reading
  dev 0 (PM-1-shaped; a dev-indexed latch is not needed — TM-1 uses `Read`).

- [ ] **Step 1: Write the failing test** (append to `core.rs` `mod tests`;
  it uses the new fake-arch opcode added in step 3)

```rust
    /// Tape micro-ops carry their device index through to the bus.
    #[test]
    fn tape_micro_ops_are_device_indexed() {
        // 0x14 = test-arch "move left on dev 1"
        let (ev, _) = run_fetch(&[0x14], 0);
        assert_eq!(ev, Ev::Request(Rq::DeviceMoveLeft { dev: 1 }));
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p mtc-core tape_micro_ops_are_device_indexed`
Expected: FAIL (opcode 0x14 unknown → the event is `Trapped(InvalidOpcode …)`).

- [ ] **Step 3: Implement**

In `arch.rs`, change the three `MicroOp` variants:

```rust
    MoveLeft { dev: u8 },
    MoveRight { dev: u8 },
    Write { dev: u8, index: u32 },
```

In `core.rs` `step_execute`:

```rust
                MicroOp::MoveLeft { dev } => (BusRequest::DeviceMoveLeft { dev }, Pending::Move),
                MicroOp::MoveRight { dev } => (BusRequest::DeviceMoveRight { dev }, Pending::Move),
                MicroOp::Write { dev, index } => {
                    (BusRequest::DeviceWrite { dev, index }, Pending::Write)
                }
```

In `test_arch::lower`, update existing arms to `dev: 0` and add the probe
opcode (also add `0x14` to `operand_kind`'s `None` group):

```rust
                (0x05, _) => vec![MicroOp::MoveLeft { dev: 0 }, MicroOp::LatchMatch(1)],
                (0x06, _) => vec![MicroOp::MoveRight { dev: 0 }, MicroOp::LatchMatch(1)],
                (0x07, Operand::Symbols(s)) if s.len() == 1 => {
                    vec![MicroOp::Write { dev: 0, index: s[0] }, MicroOp::LatchMatch(1)]
                }
                // …
                (0x14, _) => vec![MicroOp::MoveLeft { dev: 1 }],
```

In `crates/post-machine/src/arch/mod.rs`, mechanically update every
construction: `MicroOp::MoveLeft` → `MicroOp::MoveLeft { dev: 0 }`,
`MicroOp::MoveRight` → `MicroOp::MoveRight { dev: 0 }`,
`MicroOp::Write(x)` → `MicroOp::Write { dev: 0, index: x }`.
(`grep -n "MicroOp::" crates/post-machine/src/arch/mod.rs` finds them all.)

- [ ] **Step 4: Run tests**

Run: `cargo test --workspace`
Expected: all PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/core/src/vm/arch.rs crates/core/src/vm/core.rs crates/post-machine/src/arch/mod.rs
git commit -m "feat(core): tape micro-ops carry a device index"
```

---

### Task 3: Multi-device driver

Spec §7 item 4 (N tape instances; devices untouched). The driver routes
`Device*` requests by `dev` over a device slice; a request for a missing
device is a device fault. PM-1 passes a one-element slice.

**Files:**
- Modify: `crates/core/src/vm/trap.rs` (add `DeviceFault::NoSuchDevice`)
- Modify: `crates/core/src/vm/driver.rs` (`step_instruction`, `run` signatures + device arms; local tests' `drive` helper)
- Modify: every `run`/`step_instruction` call site — find with
  `grep -rn "vm::driver::run\|driver::run(\|step_instruction(" crates/` —
  expected: `crates/core/src/vm/machine.rs`, `crates/core/src/vm/debug.rs`
  (and their tests). Each passes `&mut [device]` where it passed `device`.
- Test: `crates/core/src/vm/driver.rs` (inline `mod tests`)

**Interfaces:**
- Produces: `pub fn run(core, code, stack, devices: &mut [&mut dyn Tape], profile, limits) -> RunResult`
  and the same slice parameter on `step_instruction`. Task 9's end-to-end
  test and phase 4's machine loader rely on this shape.
  `DeviceFault::NoSuchDevice { dev: u8 }`.

- [ ] **Step 1: Write the failing test** (append to `driver.rs` `mod tests`)

```rust
    /// Device requests route by index; a missing device is a device fault.
    #[test]
    fn routes_devices_by_index() {
        // 0x14 = test-arch "move left on dev 1"; only one device supplied.
        let arch = TestArch;
        let mut core = Core::new(&arch, 0);
        let mut stack = ReturnStack::new(4);
        let mut tape = InfiniteTape::new(2);
        let mut devices: Vec<&mut dyn crate::vm::devices::Tape> = vec![&mut tape];
        let r = run(
            &mut core,
            &[0x14, 0x02],
            &mut stack,
            &mut devices,
            TactProfile::ELECTRONIC,
            RunLimits::default(),
        );
        assert_eq!(
            r.outcome,
            Outcome::Trapped(Trap::Device {
                fault: crate::vm::trap::DeviceFault::NoSuchDevice { dev: 1 }
            })
        );
    }
```

(If `InfiniteTape::new` has a different constructor signature, mirror the one
used by the existing `drive` helper in this test module.)

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p mtc-core routes_devices_by_index`
Expected: compile error (`run` takes `&mut dyn Tape`, not a slice).

- [ ] **Step 3: Implement**

`trap.rs`: add to `DeviceFault`:

```rust
    NoSuchDevice { dev: u8 },
```

`driver.rs`: change both signatures to `devices: &mut [&mut dyn Tape]` and
replace the four device arms with routed versions; the pattern for each:

```rust
                    BusRequest::DeviceMoveLeft { dev } => match devices.get_mut(dev as usize) {
                        Some(device) => {
                            device.left();
                            stats.stall_tacts += u64::from(profile.move_cost);
                            BusResponse::Ok
                        }
                        None => BusResponse::Fault(DeviceFault::NoSuchDevice { dev }),
                    },
```

(same shape for `DeviceMoveRight`, `DeviceRead` — responding
`BusResponse::Symbol(device.read())` — and `DeviceWrite` which keeps its
existing `Ok`/`Fault` mapping; import `DeviceFault` from `super::trap`).
Update `run` to forward the slice, the local `drive` helper
(`let mut devices: Vec<&mut dyn Tape> = vec![&mut tape];`), and the call
sites found by the grep (`machine.rs`, `debug.rs`): wrap their single device
in a one-element slice the same way.

- [ ] **Step 4: Run tests**

Run: `cargo test --workspace`
Expected: all PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/core/src/vm/trap.rs crates/core/src/vm/driver.rs crates/core/src/vm/machine.rs crates/core/src/vm/debug.rs
git commit -m "feat(core): driver routes device requests over a device slice"
```

---

### Task 4: TR tuple bank and the Read micro-op

Spec §2 (TR: bank of 16 latches) + §7 item 1 (`Read{dev, slot}`).

**Files:**
- Modify: `crates/core/src/vm/arch.rs` (`MicroOp::Read`, test-arch opcodes 0x10, 0x13)
- Modify: `crates/core/src/vm/core.rs` (TR fields, `Pending::ReadSlot`, dispatch arm)
- Test: `crates/core/src/vm/core.rs` (inline `mod tests`)

**Interfaces:**
- Produces: `MicroOp::Read { dev: u8, slot: u8 }`;
  `Core::tr(&self) -> &[u32]` (the latched prefix, length = highest written
  slot + 1, reset at each `rd`-style sequence start is NOT done — slots are
  overwritten; `tr_len` grows monotonically per instruction… see step 3 note);
  task 6's `MatchWalk` consumes `tr()`.

- [ ] **Step 1: Write the failing test**

```rust
    /// Read{dev, slot} latches device symbols into the TR bank.
    #[test]
    fn read_latches_into_tr() {
        // 0x10 = test-arch "read dev0→slot0, dev1→slot1".
        let arch = TestArch;
        let mut core = Core::new(&arch, 0);
        let mut ev = core.start();
        // Serve CodeRead(0) → 0x10, then two DeviceReads with symbols 7 and 9.
        ev = match ev {
            Ev::Request(Rq::CodeRead { addr: 0 }) => core.resume(Rs::Byte(0x10)),
            other => panic!("unexpected: {other:?}"),
        };
        ev = match ev {
            Ev::Request(Rq::DeviceRead { dev: 0 }) => core.resume(Rs::Symbol(7)),
            other => panic!("unexpected: {other:?}"),
        };
        ev = match ev {
            Ev::Request(Rq::DeviceRead { dev: 1 }) => core.resume(Rs::Symbol(9)),
            other => panic!("unexpected: {other:?}"),
        };
        assert_eq!(ev, Ev::Step);
        assert_eq!(core.tr(), &[7, 9]);
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p mtc-core read_latches_into_tr`
Expected: compile error (`no variant Read`, `no method tr`).

- [ ] **Step 3: Implement**

`arch.rs`: add variant and fake-arch entries (opcode map: `0x10` → `None`
kind; `0x13` → `SymbolVec`):

```rust
    /// Latch the symbol under `dev`'s head into TR slot `slot`.
    Read { dev: u8, slot: u8 },
```

```rust
                (0x10, _) => vec![
                    MicroOp::Read { dev: 0, slot: 0 },
                    MicroOp::Read { dev: 1, slot: 1 },
                ],
                (0x13, Operand::Symbols(s)) if s.len() == 1 => {
                    vec![MicroOp::Write { dev: 1, index: s[0] }]
                }
```

`core.rs`: add fields to `Core` (`tr: [u32; 16]`, `tr_len: u8`, both zeroed
in `new`), the accessor, the pending kind, and the arms:

```rust
    /// The tuple register: symbols latched by `Read` micro-ops this
    /// instruction sequence. `MatchTable` compares rows against this prefix.
    pub fn tr(&self) -> &[u32] {
        &self.tr[..usize::from(self.tr_len)]
    }
```

`Pending` gains `ReadSlot { slot: u8 }`; its settle arm:

```rust
            Pending::ReadSlot { slot } => match resp {
                BusResponse::Symbol(s) => {
                    self.tr[usize::from(slot)] = s;
                    self.tr_len = self.tr_len.max(slot + 1);
                }
                BusResponse::Fault(fault) => return self.trap(Trap::Device { fault }),
                _ => return self.trap(Trap::CodeOutOfBounds { at: self.ip }),
            },
```

Dispatch arm (slot ≥ 16 is an arch-module bug — trap `BadOperand`):

```rust
                MicroOp::Read { dev, slot } => {
                    if slot >= 16 {
                        return self.trap(Trap::BadOperand { at: self.instr_start });
                    }
                    (BusRequest::DeviceRead { dev }, Pending::ReadSlot { slot })
                }
```

- [ ] **Step 4: Run tests**

Run: `cargo test --workspace`
Expected: all PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/core/src/vm/arch.rs crates/core/src/vm/core.rs
git commit -m "feat(core): TR tuple bank and the device-indexed Read micro-op"
```

---

### Task 5: New trap kinds, Raise micro-op, TableRead bus surface

Spec §3.5 (trap taxonomy), §7 items 4 and 8. This lays the vocabulary tasks
6–9 build on; nothing walks tables yet.

**Files:**
- Modify: `crates/core/src/vm/trap.rs` (five `Trap` variants + `RaisedTrapKind` + Display arms)
- Modify: `crates/core/src/vm/bus.rs` (`TableRead`, `OutOfTable`)
- Modify: `crates/core/src/vm/arch.rs` (`MicroOp::Raise`, test-arch 0x15/0x16)
- Modify: `crates/core/src/vm/core.rs` (`Raise` dispatch arm)
- Test: `crates/core/src/vm/core.rs` (inline `mod tests`)

**Interfaces:**
- Produces: `Trap::{NoTransition { at }, TableOutOfBounds { at },
  DispatchOutOfRange { at }, UnmappedRead { at }, UnmappedWrite { at }}` (all
  `at: u32`); `RaisedTrapKind::{UnmappedRead, UnmappedWrite}` (in `trap.rs`,
  `Copy`); `MicroOp::Raise { kind: RaisedTrapKind }`;
  `BusRequest::TableRead { addr: u32 }`; `BusResponse::OutOfTable`.
  Tasks 6–9 and phases 4–5 (the `trap #kind` instruction, mono trap stubs)
  rely on these exact names.

- [ ] **Step 1: Write the failing test**

```rust
    /// The Raise micro-op traps with the instruction's own address.
    #[test]
    fn raise_micro_op_traps_typed() {
        // 0x15 = test-arch "raise unmapped-read".
        let (ev, _) = run_fetch(&[0x15], 0);
        assert_eq!(ev, Ev::Trapped(Trap::UnmappedRead { at: 0 }));
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p mtc-core raise_micro_op_traps_typed`
Expected: compile error (no `UnmappedRead` variant).

- [ ] **Step 3: Implement**

`trap.rs` — variants on `Trap`:

```rust
    /// Dispatch with MR = 0: no applicable transition (spec §3.5).
    NoTransition { at: u32 },
    /// A TableRead left the table section.
    TableOutOfBounds { at: u32 },
    /// Dispatch with MR beyond the target table's entry count.
    DispatchOutOfRange { at: u32 },
    /// Holey-mapping read hole (spec §5.4).
    UnmappedRead { at: u32 },
    /// Holey-mapping write hole (spec §5.4).
    UnmappedWrite { at: u32 },
```

plus Display arms (same style as neighbors, e.g.
`"no applicable transition at {at:#010x}"`,
`"table read out of bounds at {at:#010x}"`,
`"dispatch index out of range at {at:#010x}"`,
`"unmapped symbol read at {at:#010x}"`,
`"unmapped symbol write at {at:#010x}"`), and:

```rust
/// Trap kinds an architecture may raise explicitly via `MicroOp::Raise`
/// (the `trap #kind` instruction family; spec §3.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RaisedTrapKind {
    UnmappedRead,
    UnmappedWrite,
}
```

`bus.rs`: add `TableRead { addr: u32 }` to `BusRequest` and `OutOfTable` to
`BusResponse`.

`arch.rs`: add `Raise { kind: RaisedTrapKind }` to `MicroOp` (import the
kind from `super::trap`); test-arch opcodes (kind `None`):

```rust
                (0x15, _) => vec![MicroOp::Raise { kind: RaisedTrapKind::UnmappedRead }],
                (0x16, _) => vec![MicroOp::Raise { kind: RaisedTrapKind::UnmappedWrite }],
```

`core.rs` dispatch arm (before the request-issuing pairs, alongside
`Stop`/`Halt`):

```rust
                MicroOp::Raise { kind } => {
                    let at = self.instr_start;
                    return self.trap(match kind {
                        RaisedTrapKind::UnmappedRead => Trap::UnmappedRead { at },
                        RaisedTrapKind::UnmappedWrite => Trap::UnmappedWrite { at },
                    });
                }
```

- [ ] **Step 4: Run tests**

Run: `cargo test --workspace`
Expected: all PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/core/src/vm/trap.rs crates/core/src/vm/bus.rs crates/core/src/vm/arch.rs crates/core/src/vm/core.rs
git commit -m "feat(core): typed boundary traps, Raise micro-op, TableRead bus surface"
```

---

### Task 6: Pure match-table walk (`vm/table.rs`)

Spec §4 semantics as a pure, core-independent state machine. Compact-family
encoding (Global Constraints).

**Match table byte layout** (defined here, emitted by phases 3/4, trusted by
the VM):

```
offset 0:  width      u8   — positions per row (1..=16)
offset 1:  row_count  u16  LE
offset 3:  rows       row_count × width bytes; each byte is a 7-bit symbol
                      payload; 0x7F = wildcard ("transparent", spec §3.2)
```

**Files:**
- Create: `crates/core/src/vm/table.rs`
- Modify: `crates/core/src/vm/mod.rs` (add `pub(crate) mod table;` next to the sibling module declarations)
- Test: inline `mod tests` in `table.rs`

**Interfaces:**
- Produces (consumed by task 8's core integration):

```rust
pub(crate) enum WalkStep {
    NeedByte(u32),   // issue TableRead{addr} and feed the byte back
    Done(u32),       // match result: MR value (0 = no row matched)
    Malformed,       // width 0 or > 16, or width > tr.len()
}

pub(crate) struct MatchWalk { /* private */ }
impl MatchWalk {
    pub(crate) fn new(table_addr: u32) -> Self;
    /// Drive the walk: `feed(None, tr)` to get the first request,
    /// then `feed(Some(byte), tr)` with each TableRead response.
    pub(crate) fn feed(&mut self, byte: Option<u8>, tr: &[u32]) -> WalkStep;
}
```

- [ ] **Step 1: Write the failing tests** (in `table.rs`, written together
  with the skeleton so the file compiles; a helper runs a walk against an
  in-memory table)

```rust
#[cfg(test)]
mod tests {
    use super::*;

    /// Run a MatchWalk to completion against an in-memory table blob.
    fn walk(table: &[u8], tr: &[u32]) -> Result<u32, &'static str> {
        let mut w = MatchWalk::new(0);
        let mut input = None;
        loop {
            match w.feed(input, tr) {
                WalkStep::NeedByte(addr) => {
                    input = Some(*table.get(addr as usize).ok_or("out of table")?);
                }
                WalkStep::Done(mr) => return Ok(mr),
                WalkStep::Malformed => return Err("malformed"),
            }
        }
    }

    /// width=2, three rows: [1,2] [1,0x7F] [0x7F,0x7F]
    fn sample() -> Vec<u8> {
        vec![2, 3, 0, 1, 2, 1, 0x7F, 0x7F, 0x7F]
    }

    #[test]
    fn first_match_wins_exact() {
        assert_eq!(walk(&sample(), &[1, 2]), Ok(1));
    }

    #[test]
    fn wildcard_matches_any_symbol() {
        assert_eq!(walk(&sample(), &[1, 9]), Ok(2)); // row 2: [1, *]
        assert_eq!(walk(&sample(), &[8, 8]), Ok(3)); // catch-all
    }

    #[test]
    fn no_match_yields_zero() {
        // table without catch-all: width=1, one row [3]
        assert_eq!(walk(&[1, 1, 0, 3], &[4]), Ok(0));
    }

    #[test]
    fn short_circuits_failed_row() {
        // A row failing at position 0 must not read its remaining bytes:
        // truncate row 1's second byte — walk must still reach row 2.
        // width=2, 2 rows: [5,?][0x7F,0x7F]; tr=[1,1] fails row 1 at pos 0.
        let table = vec![2, 2, 0, 5, 0, 0x7F, 0x7F];
        assert_eq!(walk(&table, &[1, 1]), Ok(2));
    }

    #[test]
    fn malformed_widths_rejected() {
        assert_eq!(walk(&[0, 1, 0], &[1]), Err("malformed")); // width 0
        assert_eq!(walk(&[17, 1, 0], &[1; 16]), Err("malformed")); // width 17
        assert_eq!(walk(&[3, 1, 0, 1, 1, 1], &[1, 1]), Err("malformed")); // width > tr
    }
}
```

Note on `short_circuits_failed_row`: even a failed row's bytes may be read by
a simple implementation — this test pins the cheaper contract (skip the rest
of a row once a position mismatches, jumping to the next row's base address),
which also keeps tact costs proportional to work. Implement the skip.

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p mtc-core table::`
Expected: compile error (module skeleton missing) — write the skeleton with
`todo!()` bodies first if needed, then FAIL.

- [ ] **Step 3: Implement**

```rust
//! Match-table walk (spec: docs/isa.md (match tables) once phase 8 lands;
//! until then the layout comment below is normative). Pure state machine:
//! the core owns the bus, this module owns the table semantics.

pub(crate) enum WalkStep {
    NeedByte(u32),
    Done(u32),
    Malformed,
}

enum Stage {
    Width,
    CountLo { width: u8 },
    CountHi { width: u8, lo: u8 },
    Row { width: u8, rows: u16, row: u16, pos: u8 },
}

pub(crate) struct MatchWalk {
    base: u32,
    stage: Stage,
}

impl MatchWalk {
    pub(crate) fn new(table_addr: u32) -> Self {
        Self { base: table_addr, stage: Stage::Width }
    }

    fn row_byte_addr(&self, width: u8, row: u16, pos: u8) -> u32 {
        self.base + 3 + u32::from(row) * u32::from(width) + u32::from(pos)
    }

    pub(crate) fn feed(&mut self, byte: Option<u8>, tr: &[u32]) -> WalkStep {
        match (&self.stage, byte) {
            (Stage::Width, None) => WalkStep::NeedByte(self.base),
            (Stage::Width, Some(w)) => {
                if w == 0 || w > 16 || usize::from(w) > tr.len() {
                    return WalkStep::Malformed;
                }
                self.stage = Stage::CountLo { width: w };
                WalkStep::NeedByte(self.base + 1)
            }
            (Stage::CountLo { width }, Some(lo)) => {
                let width = *width; // copy before assigning to self.stage (borrowck)
                self.stage = Stage::CountHi { width, lo };
                WalkStep::NeedByte(self.base + 2)
            }
            (Stage::CountHi { width, lo }, Some(hi)) => {
                let (width, lo) = (*width, *lo); // copy before assigning (borrowck)
                let rows = u16::from_le_bytes([lo, hi]);
                if rows == 0 {
                    return WalkStep::Done(0);
                }
                self.stage = Stage::Row { width, rows, row: 0, pos: 0 };
                WalkStep::NeedByte(self.row_byte_addr(width, 0, 0))
            }
            (Stage::Row { width, rows, row, pos }, Some(b)) => {
                let (width, rows, row, pos) = (*width, *rows, *row, *pos);
                let matches = b == 0x7F || u32::from(b) == tr[usize::from(pos)];
                if matches && pos + 1 == width {
                    return WalkStep::Done(u32::from(row) + 1); // 1-based MR
                }
                let (next_row, next_pos) = if matches {
                    (row, pos + 1) // same row, next position
                } else {
                    (row + 1, 0) // row failed: skip to the next row's base
                };
                if next_row == rows {
                    return WalkStep::Done(0);
                }
                self.stage = Stage::Row { width, rows, row: next_row, pos: next_pos };
                WalkStep::NeedByte(self.row_byte_addr(width, next_row, next_pos))
            }
            // feed(None) is only legal on a fresh walk; anything else is a
            // core-side driver bug.
            _ => WalkStep::Malformed,
        }
    }
}
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p mtc-core table::`
Expected: all PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/core/src/vm/table.rs crates/core/src/vm/mod.rs
git commit -m "feat(core): pure match-table walk with wildcard and first-match semantics"
```

---

### Task 7: Pure dispatch-table walk (`vm/table.rs`)

**Dispatch table byte layout**:

```
offset 0:  entry_count u16 LE
offset 2:  entries     entry_count × u32 LE — absolute code addresses
```

**Files:**
- Modify: `crates/core/src/vm/table.rs`
- Test: inline `mod tests` in `table.rs`

**Interfaces:**
- Produces (consumed by task 8):

```rust
pub(crate) enum DispatchStep {
    NeedByte(u32),
    Done(u32),      // jump target
    OutOfRange,     // mr > entry_count  → Trap::DispatchOutOfRange
}

pub(crate) struct DispatchWalk { /* private */ }
impl DispatchWalk {
    /// `mr` must be ≥ 1 (the caller handles MR = 0 as NoTransition).
    pub(crate) fn new(table_addr: u32, mr: u32) -> Self;
    pub(crate) fn feed(&mut self, byte: Option<u8>) -> DispatchStep;
}
```

- [ ] **Step 1: Write the failing tests**

```rust
    fn dispatch(table: &[u8], mr: u32) -> Result<u32, &'static str> {
        let mut w = DispatchWalk::new(0, mr);
        let mut input = None;
        loop {
            match w.feed(input) {
                DispatchStep::NeedByte(addr) => {
                    input = Some(*table.get(addr as usize).ok_or("out of table")?);
                }
                DispatchStep::Done(t) => return Ok(t),
                DispatchStep::OutOfRange => return Err("out of range"),
            }
        }
    }

    #[test]
    fn dispatch_selects_by_mr() {
        // 2 entries: 0x11111111, 0x22222222
        let t = vec![2, 0, 0x11, 0x11, 0x11, 0x11, 0x22, 0x22, 0x22, 0x22];
        assert_eq!(dispatch(&t, 1), Ok(0x1111_1111));
        assert_eq!(dispatch(&t, 2), Ok(0x2222_2222));
        assert_eq!(dispatch(&t, 3), Err("out of range"));
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p mtc-core table::dispatch`
Expected: compile error, then FAIL after skeleton.

- [ ] **Step 3: Implement** (append to `table.rs`)

```rust
pub(crate) enum DispatchStep {
    NeedByte(u32),
    Done(u32),
    OutOfRange,
}

enum DStage {
    CountLo,
    CountHi { lo: u8 },
    Entry { pos: u8, acc: [u8; 4] },
}

pub(crate) struct DispatchWalk {
    base: u32,
    mr: u32,
    stage: DStage,
}

impl DispatchWalk {
    /// `mr` must be ≥ 1 (the caller handles MR = 0 as NoTransition).
    pub(crate) fn new(table_addr: u32, mr: u32) -> Self {
        Self { base: table_addr, mr, stage: DStage::CountLo }
    }

    fn entry_addr(&self, pos: u8) -> u32 {
        self.base + 2 + (self.mr - 1) * 4 + u32::from(pos)
    }

    pub(crate) fn feed(&mut self, byte: Option<u8>) -> DispatchStep {
        match (&self.stage, byte) {
            (DStage::CountLo, None) => DispatchStep::NeedByte(self.base),
            (DStage::CountLo, Some(lo)) => {
                self.stage = DStage::CountHi { lo };
                DispatchStep::NeedByte(self.base + 1)
            }
            (DStage::CountHi { lo }, Some(hi)) => {
                let lo = *lo; // copy before assigning to self.stage (borrowck)
                let count = u16::from_le_bytes([lo, hi]);
                if self.mr > u32::from(count) {
                    return DispatchStep::OutOfRange;
                }
                self.stage = DStage::Entry { pos: 0, acc: [0; 4] };
                DispatchStep::NeedByte(self.entry_addr(0))
            }
            (DStage::Entry { pos, acc }, Some(b)) => {
                let (pos, mut acc) = (*pos, *acc);
                acc[usize::from(pos)] = b;
                if pos == 3 {
                    return DispatchStep::Done(u32::from_le_bytes(acc));
                }
                self.stage = DStage::Entry { pos: pos + 1, acc };
                DispatchStep::NeedByte(self.entry_addr(pos + 1))
            }
            // feed(None) mid-walk is a core-side protocol bug.
            _ => DispatchStep::OutOfRange,
        }
    }
}
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p mtc-core table::`
Expected: all PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/core/src/vm/table.rs
git commit -m "feat(core): pure dispatch-table walk indexed by MR"
```

---

### Task 8: Core integration — MatchTable and DispatchJump micro-ops

Wires the two walks into the core's bus loop.

**Files:**
- Modify: `crates/core/src/vm/arch.rs` (`MicroOp::{MatchTable, DispatchJump}`, test-arch 0x11/0x12)
- Modify: `crates/core/src/vm/core.rs` (two `Pending` kinds, settle + dispatch arms)
- Test: `crates/core/src/vm/core.rs` (inline `mod tests`; extend the local
  scripted-driver helper to serve `TableRead` from a table blob)

**Interfaces:**
- Consumes: `MatchWalk`/`DispatchWalk` (tasks 6–7), `tr()` (task 4),
  `mr`/`set_mr` (task 1), traps (task 5).
- Produces: `MicroOp::MatchTable { table: u32 }` (table = byte offset into
  table ROM; the pre-link symbolic index is resolved by the linker, spec §4)
  and `MicroOp::DispatchJump { table: u32 }`. Phase 4's TM-1 arch lowers
  `mtc`/`djmp` to exactly these.

- [ ] **Step 1: Write the failing tests** (append to `core.rs` `mod tests`;
  the operand of both new fake-arch opcodes is an ABSOLUTE table-ROM byte
  offset — `OperandKind::RelI32` reused, payload cast to u32)

```rust
    /// Serve CodeRead from `code`, TableRead from `tables`, DeviceRead from
    /// a symbol queue; return the first non-request event.
    fn run_with_tables(code: &[u8], tables: &[u8], symbols: &[u32]) -> Ev {
        let arch = TestArch;
        let mut core = Core::new(&arch, 0);
        let mut reads = symbols.iter().copied();
        let mut ev = core.start();
        loop {
            ev = match ev {
                Ev::Request(Rq::CodeRead { addr }) => core.resume(match code.get(addr as usize) {
                    Some(&b) => Rs::Byte(b),
                    None => Rs::OutOfCode,
                }),
                Ev::Request(Rq::TableRead { addr }) => core.resume(match tables.get(addr as usize) {
                    Some(&b) => Rs::Byte(b),
                    None => Rs::OutOfTable,
                }),
                Ev::Request(Rq::DeviceRead { .. }) => {
                    core.resume(Rs::Symbol(reads.next().expect("device script exhausted")))
                }
                other => return other,
            };
        }
    }

    /// Match table at 0 (width 2, rows [1,2] and [1,*]), dispatch table at 7
    /// (2 entries: code addresses 11 and 12).
    fn table_blob() -> Vec<u8> {
        let mut t = vec![2, 2, 0, 1, 2, 1, 0x7F];
        t.extend([2u8, 0]);
        t.extend(11u32.to_le_bytes()); // MR=1 → stp at code addr 11
        t.extend(12u32.to_le_bytes()); // MR=2 → hlt at code addr 12
        t
    }

    /// 0x10 rd(dev0→tr0, dev1→tr1); 0x11 mtc @0; 0x12 djmp @7; stp; hlt.
    fn table_code() -> Vec<u8> {
        let mut c = vec![0x10, 0x11];
        c.extend(0i32.to_le_bytes());
        c.push(0x12);
        c.extend(7i32.to_le_bytes());
        c.push(0x02); // stp — code addr 11
        c.push(0x03); // hlt — code addr 12
        c
    }

    /// rd; mtc; djmp — the canonical conditional-state shape end to end.
    #[test]
    fn match_then_dispatch_selects_target() {
        // [1,2] matches row 1 → MR=1 → dispatch to stp.
        assert_eq!(run_with_tables(&table_code(), &table_blob(), &[1, 2]), Ev::Stopped);
        // [1,9] falls to the wildcard row → MR=2 → dispatch to hlt.
        assert_eq!(run_with_tables(&table_code(), &table_blob(), &[1, 9]), Ev::Halted);
    }

    #[test]
    fn dispatch_on_no_match_traps_no_transition() {
        // [5,5] matches nothing (no catch-all) → MR=0 → djmp (at addr 6) traps.
        assert_eq!(
            run_with_tables(&table_code(), &table_blob(), &[5, 5]),
            Ev::Trapped(Trap::NoTransition { at: 6 })
        );
    }

    #[test]
    fn table_read_past_section_traps() {
        // Truncated blob: header parses (width 2, 2 rows), first row byte
        // (addr 3) is out of table → the mtc at addr 1 faults.
        assert_eq!(
            run_with_tables(&table_code(), &table_blob()[..3], &[1, 2]),
            Ev::Trapped(Trap::TableOutOfBounds { at: 1 })
        );
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p mtc-core match_then_dispatch`
Expected: compile error (no `MatchTable` variant).

- [ ] **Step 3: Implement**

`arch.rs`:

```rust
    /// Walk the match table at byte offset `table` in table ROM against TR;
    /// set MR (0 = no row matched). Spec §4.
    MatchTable { table: u32 },
    /// Jump through the dispatch table at `table` by MR;
    /// MR = 0 traps NoTransition. Spec §4.
    DispatchJump { table: u32 },
```

test-arch: `0x11`/`0x12` with `OperandKind::RelI32`, lowering
`(0x11, Operand::I32(o)) => vec![MicroOp::MatchTable { table: *o as u32 }]`
(same shape for 0x12).

`core.rs`: `Pending::Match(MatchWalk)` and `Pending::Dispatch(DispatchWalk)`
(import from `super::table`). Dispatch arms:

```rust
                MicroOp::MatchTable { table } => {
                    let mut walk = crate::vm::table::MatchWalk::new(table);
                    match walk.feed(None, self.tr()) {
                        crate::vm::table::WalkStep::NeedByte(addr) => {
                            (BusRequest::TableRead { addr }, Pending::Match(walk))
                        }
                        _ => return self.trap(Trap::BadOperand { at: self.instr_start }),
                    }
                }
                MicroOp::DispatchJump { table } => {
                    if self.mr == 0 {
                        return self.trap(Trap::NoTransition { at: self.instr_start });
                    }
                    let mut walk = crate::vm::table::DispatchWalk::new(table, self.mr);
                    match walk.feed(None) {
                        crate::vm::table::DispatchStep::NeedByte(addr) => {
                            (BusRequest::TableRead { addr }, Pending::Dispatch(walk))
                        }
                        _ => return self.trap(Trap::BadOperand { at: self.instr_start }),
                    }
                }
```

Settle arms (the `Pending::Match`/`Pending::Dispatch` cases must re-issue
`TableRead` while the walk needs bytes — mirror the `EntCheck` early-return
pattern):

```rust
            Pending::Match(mut walk) => match resp {
                BusResponse::Byte(b) => match walk.feed(Some(b), &self.tr[..usize::from(self.tr_len)]) {
                    crate::vm::table::WalkStep::NeedByte(addr) => {
                        self.phase = Phase::Execute { ops, pending: Pending::Match(walk) };
                        return CoreEvent::Request(BusRequest::TableRead { addr });
                    }
                    crate::vm::table::WalkStep::Done(mr) => self.mr = mr,
                    crate::vm::table::WalkStep::Malformed => {
                        return self.trap(Trap::BadOperand { at: self.instr_start });
                    }
                },
                BusResponse::OutOfTable => {
                    return self.trap(Trap::TableOutOfBounds { at: self.instr_start });
                }
                _ => return self.trap(Trap::CodeOutOfBounds { at: self.ip }),
            },
            Pending::Dispatch(mut walk) => match resp {
                BusResponse::Byte(b) => match walk.feed(Some(b)) {
                    crate::vm::table::DispatchStep::NeedByte(addr) => {
                        self.phase = Phase::Execute { ops, pending: Pending::Dispatch(walk) };
                        return CoreEvent::Request(BusRequest::TableRead { addr });
                    }
                    crate::vm::table::DispatchStep::Done(target) => self.ip = target,
                    crate::vm::table::DispatchStep::OutOfRange => {
                        return self.trap(Trap::DispatchOutOfRange { at: self.instr_start });
                    }
                },
                BusResponse::OutOfTable => {
                    return self.trap(Trap::TableOutOfBounds { at: self.instr_start });
                }
                _ => return self.trap(Trap::CodeOutOfBounds { at: self.ip }),
            },
```

(Borrow note: the `Pending::Match` settle arm needs `self.tr` while `walk`
is owned by the match — pass the TR slice via
`&self.tr[..usize::from(self.tr_len)]` as shown, not via `self.tr()`, to
avoid a simultaneous immutable borrow issue if the compiler objects;
adjust mechanically if it doesn't.)

- [ ] **Step 4: Run tests**

Run: `cargo test --workspace`
Expected: all PASS (all four new cases + full suite).

- [ ] **Step 5: Commit**

```bash
git add crates/core/src/vm/arch.rs crates/core/src/vm/core.rs
git commit -m "feat(core): MatchTable/DispatchJump micro-ops drive the table engine"
```

---

### Task 9: Driver serves table ROM; tact price; end-to-end fake-arch program

Spec §7 items 5–6: the driver answers `TableRead` from a table blob and
prices it. PM-1 passes an empty blob.

**Files:**
- Modify: `crates/core/src/vm/driver.rs` (`step_instruction`/`run` gain `tables: &[u8]`; `TableRead` arm; `TactProfile.table_read_cost`)
- Modify: the same call sites as task 3 (`machine.rs`, `debug.rs`) — pass `&[]`
- Modify: every `TactProfile` struct literal (grep `TactProfile {` across `crates/`) — add the field
- Test: `crates/core/src/vm/driver.rs` (inline `mod tests`)

**Interfaces:**
- Produces: `run(core, code, stack, devices, tables: &[u8], profile, limits)`
  (same on `step_instruction`); `TactProfile.table_read_cost: u32`
  (`ELECTRONIC` value: 1). Phase 3 will feed the blob from the MX v2 table
  section; phase 5 prices frame loads on top.

- [ ] **Step 1: Write the failing test**

```rust
    /// A conditional-state program runs end to end through the driver and
    /// table reads are priced as stall tacts.
    #[test]
    fn table_program_end_to_end() {
        // match table (width 1): row [2] → MR=1 ; dispatch: 1 entry → addr of stp
        // code: 0x10-analog for 1 tape is not in the fake arch; use dev0 read
        // via 0x17 (added below): [Read{0,0}]; then mtc(0), djmp(4), then 0x03 halt
        // (unreached), stp at the dispatch target.
        let mut tables = vec![1, 1, 0, 2]; // match table at 0, 4 bytes
        let stp_addr: u32 = 11; // code: 0x17, 0x11+4, 0x12+4 → 1+5+5 = 11
        tables.extend([1, 0]);
        tables.extend(stp_addr.to_le_bytes());
        let mut code = vec![0x17, 0x11];
        code.extend(0i32.to_le_bytes());
        code.push(0x12);
        code.extend(4i32.to_le_bytes());
        code.push(0x02); // stp at 11
        let arch = TestArch;
        let mut core = Core::new(&arch, 0);
        let mut stack = ReturnStack::new(4);
        let mut tape = InfiniteTape::new(3);
        tape.write(2).unwrap(); // head cell = symbol 2 → row matches
        let mut devices: Vec<&mut dyn crate::vm::devices::Tape> = vec![&mut tape];
        let r = run(
            &mut core, &code, &mut stack, &mut devices, &tables,
            TactProfile::ELECTRONIC, RunLimits::default(),
        );
        assert_eq!(r.outcome, Outcome::Stopped);
        // 4 match-table bytes + 2 count bytes + 4 entry bytes = 10 table reads
        // priced at table_read_cost=1, plus 1 device read.
        assert_eq!(r.stats.stall_tacts, 10 + 1);
    }
```

(Add fake-arch opcode `0x17` (kind `None`) → `vec![MicroOp::Read { dev: 0, slot: 0 }]`
in this task. If `InfiniteTape`'s constructor/write differ, mirror the
existing `drive` helper's usage. If the byte counting in the comment turns
out off by the actual walk's read pattern, fix the expected number to the
observed one ONLY after hand-verifying the walk's request sequence —
the test documents the pricing contract, not a magic number.)

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p mtc-core table_program_end_to_end`
Expected: compile error (`run` has no `tables` parameter).

- [ ] **Step 3: Implement**

`TactProfile`: add `pub table_read_cost: u32`, and to `ELECTRONIC`:
`table_read_cost: 1`. Update every other `TactProfile { … }` literal found by
`grep -rn "TactProfile {" crates/` (add `table_read_cost: 1` unless the test
clearly wants another value).

`step_instruction`/`run`: add `tables: &[u8]` after `devices`; new arm:

```rust
                    BusRequest::TableRead { addr } => match tables.get(addr as usize) {
                        Some(&byte) => {
                            stats.stall_tacts += u64::from(profile.table_read_cost);
                            BusResponse::Byte(byte)
                        }
                        None => BusResponse::OutOfTable,
                    },
```

Call sites (`machine.rs`, `debug.rs`, driver-local tests): pass `&[]` where
no table section exists yet.

- [ ] **Step 4: Run tests**

Run: `cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --check`
Expected: all PASS / clean.

- [ ] **Step 5: Commit**

```bash
git add crates/core/src/vm/driver.rs crates/core/src/vm/machine.rs crates/core/src/vm/debug.rs
git commit -m "feat(core): driver serves table ROM with priced TableRead"
```

---

### Task 10: Phase gate — full regression + quality sweep

The phase milestone (spec §17): "PM-1 regression green".

**Files:**
- No new code; fixes only if the sweep finds anything.

- [ ] **Step 1: Full suite**

Run: `cargo test --workspace`
Expected: everything green, including `golden_programs` (byte-identical
committed goldens — the literal byte-identity check), `opt_equivalence`,
`cli_programs`.

- [ ] **Step 2: Quality gates**

Run: `cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --check`
Expected: clean.

- [ ] **Step 3: Grep audit — no arch leakage**

Run: `grep -rn "pm1\|PM-1\|tm1\|TM-1" crates/core/src/vm/table.rs crates/core/src/vm/core.rs | grep -v "docs/isa"`
Expected: no matches in code identifiers (comments citing docs pages are fine).

- [ ] **Step 4: Commit (only if fixes were needed)**

```bash
git add -A crates/
git commit -m "polish(core): phase-1 regression sweep fixes"
```

---

## Self-review notes (spec → plan coverage)

- §7 item 1 (tape-indexed micro-ops) → tasks 2, 4. item 2 (MR) → task 1.
  item 3 (table engine, SymbolVec-encoded rows — compact subset) → tasks 6–8.
  item 4 (TableRead; devices untouched, N instances) → tasks 3, 5, 9.
  item 6 (driver + TactProfile) → task 9. item 8 (trap kinds) → task 5.
  item 10 (test_arch proves agnosticism) → every task extends `test_arch`,
  task 10 greps for leakage.
- §7 items 5, 9 (frames, DebugSession additions) and item 7
  (`from_executable` validation) are phases 5 and 3/4 — deliberately absent.
- §4's binary-search subsection layout is a layout DISCIPLINE (assembler
  verifies, phase 4); the runtime walk is linear first-match — semantically
  identical, noted in task 6.
- The `at` payloads on the new traps use `instr_start` (the faulting
  instruction), matching the existing `BadOperand` convention.
