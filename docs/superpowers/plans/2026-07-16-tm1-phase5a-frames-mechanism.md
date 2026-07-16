# TM-1 arc — phase 5a: the frames mechanism

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** the frames execution profile works end-to-end on hand-written input — a `.tma` program whose 4-tape machine calls a narrower routine through a hand-authored `.frame` (projection + per-tape symbol maps + multi-exit `retx`) assembles, links, and runs on the frames-profile VM. (Phase 5b adds the automatic composition engine: closure BFS, mono stamping, hybrid, compose table, dedup, `call.m → call` relaxation, map-sidecar binding labels, `LinkOptions.entry`.)

**Architecture:** the VM core gains an `FR` register, a core-local frame cache loaded from table ROM at `call.m` time (through a new `FrameRead` bus request with its own tact price), a parallel FR stack synced with the return stack, and three new micro-ops (`ReadAll`, `CallFrame`, `RetX`); virtual→physical tape and symbol translation happens at the four dev-carrying micro-op arms. The assembler gains the `.frame`/`.map`/`.exits` directive family (frame descriptors as a third table kind riding the existing single-owner/fixup/rebase machinery), `#imm` operands, the `call.m` framed-call operand, and the declarative binding-grammar call operand producing the (still link-rejected) MO v3 `BoundCall` records. The TM-1 dialect moves to 0.2.

**Tech stack:** Rust 2024; no new dependencies.

**Plan-vs-spec adjudications (recorded):**
- The FR half of the uniform `(return address, FR)` stack lives in a Core-internal parallel stack synced with successful bus pushes/pops — observable behavior identical to pair-storage, and it keeps `ReturnStack`/`RunResult.stack`/`BusRequest` types (and their PM-1 consumers) untouched.
- Frame-descriptor loads ride a new `BusRequest::FrameRead { addr }` served from the same table ROM but priced by a new `TactProfile::frame_load_cost` — this is how "bus reads priced at call time" gets its own knob without driver heuristics.
- `retx`/`ret` under frames restore FR and RELOAD the restored frame's descriptor from ROM (priced); a descriptor-stack cache is a later optimization if measured.
- Vector-width validation moves from lower time to two other places (a routine body authored at arity M < machine N must lower): the arch accepts any width `1..=16`; the assembler statically checks width == `RoutineSig.arity` inside signed functions; at run time an out-of-range dev is `NoSuchDevice` (identity) or a frame-arity trap (framed).
- Immediates are numeric in 0.2 (`trap #0`, `retx #1`); named trap kinds can join later without a grammar break.

## Global Constraints

1. **PM-1 byte-identity.** Goldens under `crates/post-machine/tests/golden/` unchanged; `pm1_syntax()` caps stay default-off; PM-1 never sets `frames_enabled`, never emits the new micro-ops, never sees the new bus request; legacy `Machine::run`/`debug` behavior unchanged; tableless/frameless links byte-identical.
2. **Core stays arch-agnostic** — all new core mechanisms tested with `test_arch` (0x7F) / fake dialects; zero TM-1 knowledge (in code AND comments).
3. **Trap taxonomy invariant** (spec-critical): `no-transition` vs `unmapped-read` vs `unmapped-write` stay distinct; holes trap ONLY when crossed (read direction on `rd`-through-rmap, write direction on `wr`-through-wmap); passing over a hole without reading does not trap.
4. **Frame descriptor wire format** (this plan's §"Descriptor format" is normative for every task): arity u8 (1..=16), exit_count u16 LE, then arity × [phys u8, rmap_len u16 LE, rmap_len × u16 LE, wmap_len u16 LE, wmap_len × u16 LE], then exit_count × u32 LE. Map entry `0xFFFF` = hole; `*_len == 0` = identity map; rmap is indexed by PHYSICAL symbol yielding virtual, wmap by VIRTUAL symbol yielding physical; exits are blob-relative code offsets at assembly, absolute after link (same rebase as dispatch entries).
5. **FR semantics**: `FR: u32`; 0 = identity frame (no translation, machine width); non-zero FR = descriptor's final table-section offset + 1.
6. **Profile constants**: `pub const PROFILE_BASE: u8 = 0; pub const PROFILE_FRAMES: u8 = 1;` in `crates/core/src/formats/mod.rs`. The linker emits `PROFILE_FRAMES` iff the image contains a frame descriptor or a framed call; `Machine::from_executable` rejects profiles the VM doesn't implement (> 1) with a new `LoadError::UnsupportedProfile { profile: u8 }`.
7. **Dialect version**: `TM1_TMA_DIALECT_VERSION` 0.1 → "0.2" (grammar change: `call.m`, `retx`, `trap`, `#imm`, `.frame`/`.map`/`.exits`, binding call operand).
8. **AsmErrorKind count test** (`every_kind_has_a_distinct_code`, currently 17) updates with each added variant.
9. Thin renderer; conventional commits with scope, NO attribution footers; durable-page citations only (`docs/formats.md (frame descriptors)` once T7 lands the section — prose before that); no new deps; fmt/clippy clean; goldens derivation-first.
10. The linker's `UnsupportedBindings` guard STAYS — producing `BoundCall` records is 5a scope, linking them is 5b. A binding-call program must assemble and must still be refused at link (gate-tested).

## File Structure

- Modify (core): `vm/{core,bus,driver,trap,arch,machine,debug}.rs`, `formats/mod.rs`, `asm/{lexer,cst,lower,assembler,disassembler,fmt,mod}.rs`, `linker/{mod,layout}.rs`
- Modify (turing-machine): `src/arch/mod.rs`, `src/asm/mod.rs`, `src/cli/run.rs`, tests
- Create: `crates/core/tests/frames_vm.rs` (or extend driver tests in-module — T1 implementer's choice, one home), `crates/turing-machine/tests/frames_programs.rs`
- Modify (docs): `docs/formats.md` (frame descriptors + `.tma` 0.2 additions)

---

### Task 1: Core frames execution — FR, frame cache, translation, three micro-ops

**Files:** `crates/core/src/vm/{arch,core,bus,driver,trap}.rs` (+ chosen test home)

**Interfaces produced (consumed by T2, T4):**
```rust
// arch.rs MicroOp additions:
ReadAll,                              // identity: Read{dev:i,slot:i} for i in 0..device_count; framed: per virtual k in 0..arity, read phys(k) through rmap_k into TR[k]; tr_len := width read
CallFrame { rel: i32, frame: u32 },   // frame = descriptor table-section offset
RetX { k: u8 },
// bus.rs: BusRequest::FrameRead { addr: u32 }  (response Byte/OutOfTable, like TableRead)
// trap.rs: Trap::ExitOutOfRange { at: u32 }, Trap::ProfileViolation { at: u32 }
// core.rs: Core::with_device_count(self, n: u8) -> Self  (builder; default 1)
//          Core::with_frames(self) -> Self               (builder; enables pair discipline)
//          pub fn fr(&self) -> u32
// driver.rs: TactProfile gains pub frame_load_cost: u32 (ELECTRONIC = 1); FrameRead served from `tables` priced at frame_load_cost per byte
```

**Semantics (normative):**
- **Frame cache**: a Core-held decoded descriptor (`arity`, per-tape `(phys, rmap: Vec<u16>, wmap: Vec<u16>)`, `exits: Vec<u32>`). Loaded by an async `FrameWalk` pending state (mirror `MatchWalk`'s request-per-byte shape) issuing `FrameRead`s. Malformed descriptor (arity 0 or > 16, truncated) → `Trap::TableOutOfBounds { at }`.
- **Translation** at the four dev-carrying arms when FR ≠ 0: `Read{dev,slot}`/`Write{dev,index}`/`MoveLeft/Right{dev}` treat `dev` as VIRTUAL — `dev >= arity` → `Trap::BadOperand { at }`; physical dev = `entries[dev].phys`. Read settle (`Pending::ReadSlot`/`Latch`): physical symbol s → virtual via rmap (identity if empty; `s >= len || rmap[s] == 0xFFFF` → `Trap::UnmappedRead { at }`). Write issue: virtual index v → physical via wmap (identity if empty; hole → `Trap::UnmappedWrite { at }`). Moves translate dev only.
- **`ReadAll`**: expands at execution (never at lower). Identity: `device_count` reads, `tr_len = device_count`. Framed: `arity` reads through the per-tape rmaps, `tr_len = arity`.
- **`CallFrame`**: resolve rel target (as `Call`), EntCheck; on success push return address to bus stack AND current FR to the parallel stack; `FR := frame + 1`; load descriptor via FrameWalk; jump. Stack-full → `StackOverflow` with no FR push (sync discipline: FR pushes/pops happen only on successful bus responses).
- **`RetX{k}`**: requires FR ≠ 0 and `k < exits.len()` (else `ExitOutOfRange { at }`); read `exits[k]` from the CURRENT cache first, then pop the pair (bus pop + parallel pop; popped address discarded), restore FR, reload the restored frame's descriptor if non-zero, `ip := exits[k]`.
- **Uniform pair discipline under `with_frames()`**: plain `Call` also pushes FR (unchanged value) to the parallel stack; plain `Ret` pops both, restores FR, reloads if the restored FR ≠ current cache. Without `with_frames()` (PM-1/base): `Call`/`Ret` behave exactly as today; `CallFrame`/`RetX`/frame-translation paths → `Trap::ProfileViolation { at }` (ReadAll works in identity mode regardless — it has no frames dependency).
- **`MatchTable`/`DispatchJump` unchanged** — they consume TR/MR, which are already virtual.

**Steps:**
- [ ] **Step 1:** Extend `test_arch` with fake opcodes for `ReadAll` (0x18), `CallFrame` (0x19, operand: the new FramedCall kind is T3 — for THIS task use a hand-lowered pairing: give the fake opcode `OperandKind::RelI32` and hard-code the frame offset in the fake `lower`… NO — simpler and honest: `test_arch::lower` may return micro-ops directly from hand-chosen operands; add fake opcodes 0x18 `ReadAll`, 0x19 → `CallFrame{rel: op, frame: <const from a second fake opcode table>}`. Cleanest concrete choice: fake 0x19 takes `TableRef` and lowers to `CallFrame { rel: +1 fixed…`) — the implementer picks the least-contrived fake encoding that exercises all three micro-ops and documents it; the REAL encoding arrives with T3/T4. Write failing core tests: descriptor decode via FrameWalk; translation on all four arms (both directions, identity maps, explicit maps, holes → the two Unmapped traps); ReadAll identity vs framed widths; CallFrame/RetX round trip incl. nested plain call inside a framed body (ret restores the frame); ExitOutOfRange; ProfileViolation without `with_frames()`; StackOverflow leaves FR stack synced.
- [ ] **Step 2:** Implement (arch enum + core arms + FrameWalk + bus + driver + traps). Tests pass.
- [ ] **Step 3:** Driver-level end-to-end (mirror `table_program_end_to_end`): hand-assembled fake program — framed call, framed body does ReadAll/Write through maps, retx to a labeled exit — asserts Stopped, final tape cells, and exact stall accounting incl. `frame_load_cost`.
- [ ] **Step 4:** Full gate (workspace tests / clippy -D warnings / fmt / goldens). Commit: `feat(core): the frames execution mechanism — FR, frame cache, ReadAll/CallFrame/RetX`

### Task 2: Profile plumbing — constants, Machine validation, DebugSession FR, tmt trace

**Files:** `crates/core/src/formats/mod.rs`, `crates/core/src/vm/{machine,debug}.rs`, `crates/turing-machine/src/cli/run.rs`

- `PROFILE_BASE`/`PROFILE_FRAMES` constants; migrate `executable.rs` test comments/values to them where trivially applicable.
- `Machine` carries `profile: u8` from the executable; `from_executable` rejects `profile > PROFILE_FRAMES` → `LoadError::UnsupportedProfile { profile }` (+ Display). `run_tapes`/`debug_tapes` construct the Core `with_device_count(tape_count)` always, `with_frames()` iff `profile == PROFILE_FRAMES`.
- `DebugSession::fr() -> u32` accessor; depth-based stepping untouched (retx pops one entry like ret — assert with a test).
- `tmt run --trace`: the per-instruction line gains ` FR=<n>` after `heads=[..]` ONLY when the image profile is frames (base-profile trace output byte-identical — pin).
- [ ] Failing tests (machine/debug with test_arch on a profile-1 sectioned image; UnsupportedProfile on profile 2; base-profile trace pin in tmt cli tests) → implement → gate. Commit: `feat(core): frames profile plumbing — constants, load validation, FR observability`

### Task 3: Core operand kinds — `Imm8` and `FramedCall` (+ `#` lexing)

**Files:** `crates/core/src/vm/{arch,core}.rs`, `crates/core/src/asm/{lexer,lower,assembler,decode,disassembler,mod}.rs`, `crates/core/tests/operand_codec.rs`

- `OperandKind::Imm8` / `Operand::Imm(u8)` — 1 raw byte. `OperandKind::FramedCall` / `Operand::FramedCall { rel: i32, table: u32 }` — 8 bytes: rel i32 LE then table u32 LE. `encode_operand` arms + core fetch arms + `decode.rs` (dis) arms (`DecodedOperand::Imm(u8)` and `DecodedOperand::FramedCall { target: u32, table: u32 }` — target resolved like RelTarget) + property tests (round-trips, never-panic).
- Lexer: `#` becomes a token under `caps.tables` (mirror the 4a/4b gating pattern + byte-compat pin with caps off).
- `classify_operand`: `Imm8` → exactly `#<int>` 0..=255 (else BadOperand with span); `FramedCall` → `<name>, <name>` (target then frame-table label) → new `SourceOperand::FramedCall { target: SpannedName, frame: SpannedName }`.
- Assembler emit: FramedCall = 8-byte hole; `Relocation { blob, offset, symbol }` for the rel half at `offset`, `TableRefHole`-style fixup for the table half at `offset+4` (rides the existing table attribution + `TableFixup` machinery). Imm8 emits the byte.
- Dis: `Imm8` renders `#<n>`; `FramedCall` renders `<label-or-addr>, <T-label>`; caps-off rendering byte-compat pinned.
- Fake-dialect coverage: extend `fake_syntax()` with `fcall` (FramedCall) + `fimm` (Imm8) mnemonics; assemble∘dis fixpoints.
- [ ] TDD → implement → gate. Commit: `feat(core): Imm8 and FramedCall operand kinds with # immediates`

### Task 4: TM-1 arch + dialect — `trap`/`call.m`/`retx`, ReadAll rd, width relax, dialect 0.2

**Files:** `crates/turing-machine/src/arch/mod.rs`, `src/asm/mod.rs`, `crates/core/src/asm/assembler.rs` (signed-function width check), tests

- Opcodes go live: `TRAP = 0x11` (Imm8; kind 0 → `Raise{UnmappedRead}`, 1 → `Raise{UnmappedWrite}`, else lower-time `Trap::BadOperand`), `CALL_M = 0x13` (FramedCall → `[CallFrame{rel, frame}]`), `RETX = 0x14` (Imm8 → `[RetX{k}]`). The reserved-slots comment shrinks to `wrmv 0x12` only.
- `rd` lowering: N × `Read` → `[ReadAll]`; update the T1-era unit tests; TM-1 integration tests must stay green unchanged (behavioral identity on base profile).
- `wr`/`mov` lower-time width check relaxes from `== tape_count` to `1..=16` (payload rules unchanged); the per-function STATIC width check moves to the assembler: inside a `.func` that has a `RoutineSig`, a SymbolVec/MoveVec vector operand whose element count ≠ `arity` → `BadVector` (core, caps-gated, fake-dialect tested; unsigned functions keep the old freedom).
- `tm1_syntax()` entries: `trap` (Imm8, FallThrough), `call.m` (FramedCall, Call), `retx` (Imm8, Stop — mirror `ret`); relax_pairs unchanged (call.m relaxation is 5b). `TM1_TMA_DIALECT_VERSION = "0.2"`; `--version` line updates (pin in cli test).
- [ ] TDD (arch unit tests per lowering + error cases; dialect round-trips; width-check cases) → implement → gate. Commit: `feat(turing-machine): trap, call.m, retx — the frames instructions in the .tma dialect (0.2)`

### Task 5: `.frame` directive family + linker frame emission

**Files:** `crates/core/src/asm/{cst,lower,assembler,disassembler,fmt}.rs`, `crates/core/src/linker/{mod,layout}.rs`, tests (incl. a new/extended link test home)

**Authoring grammar (normative; all tokens already lexable under caps tables+rept+vectors):**
```
Fname:  .frame tapes=(3,0)                      ; arity = list length; virtual k → physical tapes[k]
        .map 0, rmap=(2->1, 4=>0), wmap=(1->2)  ; optional, per virtual tape; -> bidirectional pair split into rmap/wmap entries as written; => one-way (rmap only)
        .exits Lodd, Leven                       ; optional, once, labels in the owning function
```
`.map k` may appear at most once per k; omitted → identity maps. Pairs list PHYSICAL->VIRTUAL for `rmap=`, VIRTUAL->PHYSICAL for `wmap=` (document in the CST comment; the descriptor builder materializes dense u16 tables sized to the max index + holes at 0xFFFF). Blank↔blank (0↔0) is implicit and always present. Errors → new `AsmErrorKind::BadFrame(String)` with spans (duplicate `.map k`, k ≥ arity, tapes list empty or > 16, map index > 0xFFFE, `.exits` twice, orphan `.map`/`.exits` without a preceding `.frame`).

- CST: `FrameDirective(FrameDirectiveCst)` items (header/map/exits shapes), lossless, caps-gated (degrade to Line/Raw like `.routine`); fmt renders on the canonical grid, idempotent.
- Lower/build: a labeled `.frame` group builds a descriptor blob per Global Constraint 4 and enters the SAME single-owner table attribution as match/dispatch tables (`SourceTable::Frame`-style variant); referenced by `call.m`'s frame label (the `offset+4` fixup from T3); exits recorded as blob-relative code offsets.
- Linker (`layout.rs`): the table walker learns `TableKind::Frame` — kind inference: a fixup hole at `offset+4` of a `FramedCall`-kind operand (the walker knows the referencing opcode; a `Call`-flow opcode whose operand kind is FramedCall → Frame; existing Match/Dispatch inference untouched); walks the descriptor by its self-describing header; rebases the exits region through the owning function's `orig_to_new` + code base exactly like dispatch entries; profile emission: `PROFILE_FRAMES` iff any frame descriptor or FramedCall fixup was emitted, else `PROFILE_BASE` (byte-identity lock for frameless links).
- Dis: object + executable render `.frame`/`.map`/`.exits` back (exits via map labels when present); assemble∘dis∘assemble round-trip test (single-function, the strong form mirroring 4b's).
- [ ] TDD (fake dialect end-to-end: asm → link → byte-derived descriptor in the table section with absolute exits → dis round-trip; relaxation-shift case for exits; every BadFrame case) → implement → gate. Commit: `feat(core): frame descriptors — .frame authoring, table-section emission, exit rebasing`

### Task 6: The declarative binding call operand → `BoundCall` records

**Files:** `crates/core/src/asm/{lower,assembler,disassembler}.rs`, linker gate test, tm1 dialect test

**Source grammar (§ spec's canonical entry grammar as source):** `call <name> [<entry>, <entry>, …]` where `entry = <physIdx>` or `<physIdx>{<src>-><dst>, <src>=><dst>, …}` — list position = callee virtual tape. Example: `call plusOne [2{1->3,2=>0}, 0]`.
- classify (RelI32 + trailing `[..]` group, caps-gated): → `SourceOperand::BoundCallOp { target: SpannedName, binding: Vec<SourceTapeBinding> }`.
- Assembler: emits the far-call opcode + 4-byte ZERO hole and a `BoundCall { blob, offset, symbol, binding }` record (NO `Relocation` — the linker's composition engine owns lowering in 5b); pair flags map `->` → `one_way: false`, `=>` → `one_way: true`; validation (`physIdx < 16`, src/dst fit u32, duplicate src in one entry → `BadVector`-style error — new message under `BadFrame` or `BadOperand`, implementer picks and states).
- Object-level dis renders the operand back from `bound_calls` (round-trip test).
- Gate test: a `.tma` program with a binding call ASSEMBLES, and `link` still refuses with `UnsupportedBindings` (Global Constraint 10) — in BOTH the fake dialect (core test) and the tm1 dialect (turing-machine test).
- [ ] TDD → implement → gate. Commit: `feat(core): the declarative binding call operand produces bound-call records`

### Task 7: Milestone — hand-written frames program end-to-end; docs; property tests; phase gate

**Files:** `crates/turing-machine/tests/frames_programs.rs`, `docs/formats.md`, `crates/core/tests/` property additions

- **The milestone program** (hand-written `.tma`, committed as a test fixture string or under `docs/examples/` — implementer judgment, lean toward a test-local string unless it reads as teaching material): a 4-tape machine (`.routine main, tapes=4, alpha=(…)`) with a 2-arity helper routine; `main` calls it via `call.m helper, Fh` through a hand-authored `.frame` binding virtual (0,1) → physical (2,0) with a non-identity rmap containing a one-way `=>` pair and a hole; the helper uses `rd`/`mtc`/`djmp`/`wr`/`mov` at width 2 and returns via `retx #0` / `retx #1` to two distinct exits. Assemble → link (profile must come out `PROFILE_FRAMES`) → run via `run_tapes`:
  - happy path: Stopped, final tapes assert the projection + maps did what the derivation says (derive expected tapes in the test by hand-simulating the two routines);
  - trap taxonomy: a tape seeded with an unmapped symbol under the helper's read → `Trapped(UnmappedRead)`; a helper write of a hole-mapped symbol → `Trapped(UnmappedWrite)`; helper `djmp` with no row → `NoTransition` (distinctness proven);
  - `retx #2` variant → `ExitOutOfRange`;
  - debug: `debug_tapes` stepping shows `fr()` = 0 → non-zero inside the helper → 0 after return; depth behaves like plain call.
- **Property tests** (core): descriptor codec — arbitrary valid descriptors round-trip through build → FrameWalk decode; never-panic on truncated/noise descriptor bytes (Trap, not panic).
- **Docs** (`docs/formats.md`, forge-agnostic ref-free): frame-descriptor layout in the table section (Global Constraint 4 verbatim, as prose+table); `.tma` 0.2 section additions — `call.m`/`retx`/`trap`/`#imm`, the `.frame`/`.map`/`.exits` grammar, the binding call operand, and the note that binding calls await the composition engine. Resolve any `docs/formats.md (frame descriptors)` citations left in prose by earlier tasks.
- **Phase gate** (verbatim in report): `cargo test --workspace` · `cargo clippy --workspace --all-targets -- -D warnings` · `cargo fmt --check` · `git status --short crates/post-machine/tests/golden/` — plus a hand CLI run of the milestone program pasted into the report.
- [ ] TDD → implement → gate. Commit: `feat(turing-machine): hand-written frames program runs — phase-5a milestone` (+ optional separate `docs(formats):` commit)

---

## Self-review notes (run at plan time)

- Spec §7 item 5 coverage: FR ✅ T1, frame cache ✅ T1, `JumpFrameExit` ✅ (named `RetX`; recorded rename — the spec's name described the micro-op's jump effect, ours its instruction), frame-load pricing ✅ T1 (`FrameRead` + `frame_load_cost`), DebugSession FR ✅ T2. §3.3 semantics ✅ T1 (uniform pairs, retx discipline). §5.4 frames-mode translation ✅ T1; static arity check ✅ T4 (assembler, signed functions) + 5b linker check to come. §5.5 exit vectors ✅ T1/T5. §8.1 `.frame` ✅ T5. §8.2 both call forms ✅ T4 (raw) / T6 (declarative). §3.5 trap taxonomy ✅ T1/T7.
- Deliberately NOT here (5b): composition engine, compose table, mono stamping, hybrid, dedup, `call.m → call` relaxation, `--call-mech` flag, map-sidecar binding labels + digests + dis legend, `LinkOptions.entry`, the §5.2 "one lookup at any depth" property test, three-mode equivalence harness.
- Type consistency: `CallFrame{rel,frame}` (T1) ← `Operand::FramedCall{rel,table}` (T3) ← `call.m` lowering (T4) ← `.frame` label fixup (T5). `Imm8` (T3) ← `trap`/`retx` (T4). `RoutineSig.arity` (existing) ← width check (T4) ← milestone (T7).
- Open risk, stated: T1 is the deepest core change since phase 1 (a second async walk + translation on the hot path). Mitigation: translation code paths are all behind `FR ≠ 0` / `with_frames()` checks that PM-1 never enables, and the PM-1 suite + goldens gate every task.
