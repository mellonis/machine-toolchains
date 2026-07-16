# TM-1 arc — phase 4b: TM-1 crate, `.tma` dialect, minimal `tmt` CLI, UTM milestone

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `docs/examples/brainfuck-utm.tma` assembles, links, and runs through a new `tmt` CLI — the phase-4 milestone of the TM-1/tmt arc (spec `docs/superpowers/specs/2026-07-16-tm1-and-tmt-design.md` §17, phase 4).

**Architecture:** A new cargo-workspace crate `crates/turing-machine` (`mtc-turing-machine`, binary `tmt`) plugs the TM-1 architecture (arch id `0x02`) into the arch-agnostic core exactly the way PM-1 does: an `Arch` impl (opcode → micro-op lowering) plus an `ArchSyntax` (`tm1_syntax()`, all `AsmCaps` on). Core grows four arch-agnostic pieces the fact-finding pass proved missing: a `MoveVec` operand kind + instruction-level vector emit arms, a `.routine` signature directive, the linker's table-section emission (concat + dispatch rebase + `TableRef` patching + `sectioned()` emit), and multi-device/table-aware `Machine` loading. Nothing TM-1-specific enters core — fake dialects prove every core addition.

**Tech stack:** Rust 2024, serde/serde_json, proptest (dev). No new dependencies.

**Plan-vs-spec deviations (controller-adjudicated, recorded):**
- The spec's phase-4 row lists a minimal CLI "asm/dis/run/tape"; this plan adds **`link`** — the milestone (UTM runs) is unreachable without it, and `tmt asm` alone produces only a `.tmo`.
- `wrmv` (spec §3.1's primary op) and `trap #kind` are **deferred to their producer phases** (§11's compiler codegen for `wrmv`; §5.3's mono-stub synthesis for `trap`). The UTM uses `wr`+`mov` separately, and TM-1's ISA is unreleased until the arc release, so late addition is not a public revision. Their opcode slots are reserved in Task 1's table so no renumbering happens later. Encoding decision recorded for `wrmv`: a future `OperandKind::SymbolVecPair` — two self-delimited compact groups back-to-back (writes then moves).
- Only `call` relaxes in TM-1 0.1 (`call.s = call | 0x10`), following spec §3.1's core set exactly; jump short forms remain headroom.

## Global Constraints

Copied from the spec / repo contracts; every task's requirements include these.

1. **PM-1 byte-identity.** No golden under `crates/post-machine/tests/golden/` changes. `pm1_syntax()` keeps `caps: AsmCaps::default()`. The linker's `code_only` emit path stays byte-identical for objects with no v3 table content. `Machine::run(&self, device: &mut dyn Tape, …)`'s existing behavior (including its `set_mf(device.read() == 1)` PM-1 mark preload) is unchanged.
2. **Core stays arch-agnostic.** Zero TM-1 knowledge in `crates/core`; every new core mechanism is exercised by fake/neutral dialects (`fake_syntax()` in assembler.rs/disassembler.rs, `test_arch` 0x7F) in core's own tests.
3. **Constants:** `ARCH_TM1: u8 = 0x02` (in `crates/core/src/formats/mod.rs` next to `ARCH_PM1`); `TM1_TMA_DIALECT_VERSION: &str = "0.1"` (in the TM-1 crate's `asm` module, re-exported from `lib.rs`, printed by `tmt --version`).
4. **Encoding contracts** (already pinned by 4a tests — do not disturb): match blob = `width u8, count u16 LE, rows` (width bytes each, `0x7F` wildcard); dispatch blob = `count u16 LE` + `u32 LE` blob-relative code offsets; `Operand::Table(u32)` = 4-byte LE absolute table-section offset; compact symbol elements are single-byte 7-bit with high-bit-on-last terminator, payload ≤ `0x7E`, `0x7F` = transparent (keep/wildcard).
5. **Vector element values** (from `VecElem`, `crates/core/src/asm/lower.rs:74-87`): `Payload(v)` = v, `Keep`(`-`) = `0x7F`, `Stay`(`.`) = 0, `MoveLeft`(`<`) = 1, `MoveRight`(`>`) = 2.
6. **Thin-renderer rule:** library code never prints; every terminal byte originates in the crate's `cli/`; errors flow as typed values / `Result<_, String>`.
7. **Exit codes for `run`:** 0 = stopped (`stp`), 2 = halted (`hlt`), 3 = trapped, 1 = tool error.
8. **Goldens are derivation-first** — expected snapshots derived in test code; tool output never committed as a golden. Mirror `crates/post-machine/tests/golden_programs.rs`'s exact discipline.
9. **No new dependencies.** `[dependencies] mtc-core, serde, serde_json`, `[dev-dependencies] proptest` only.
10. **Docs policy:** code comments cite durable pages as `docs/<page>.md (keyword)`; never cite `docs/superpowers/` anything; published content (docs/, README, the UTM example file) is forge-agnostic — no issue/PR numbers, no hosting URLs. TM-1-specific doc PAGES wait for phase 8's domain split; the `.tma` dialect section lands in the shared root `docs/formats.md` (spec §17's shared-root rule); where no durable page exists yet, comments carry substance in prose.
11. **No per-file license headers** (repo convention: `license.workspace = true` only); files open with `//!` module docs.
12. Conventional commits with scope: `feat(turing-machine):`, `feat(core):`, `test(core):`, etc. **No attribution footers.**
13. CLI tests are **in-process** (`use mtc_turing_machine::cli::execute;` + `CARGO_TARGET_TMPDIR` scratch) — no subprocess spawning.

## File Structure

- Create: `crates/turing-machine/` — `Cargo.toml`, `src/lib.rs`, `src/arch/mod.rs` (opcodes + `Arch` impl), `src/asm/mod.rs` (`tm1_syntax()`, dialect version, assemble/dis/link wrappers), `src/cli/{mod.rs,build.rs,inspect.rs,run.rs}`, `src/bin/tmt.rs`, `tests/{tm1_arch.rs,tma_dialect.rs,cli_programs.rs,golden_programs.rs}`, `tests/golden/`.
- Modify (core, all arch-agnostic): `formats/mod.rs` (ARCH_TM1), `vm/arch.rs` (`OperandKind::MoveVec` + encode), `vm/core.rs` (MoveVec fetch decode), `asm/decode.rs` (MoveVec dis decode), `asm/lexer.rs` (`Eq` token under tables cap), `asm/cst.rs` (`.routine` node), `asm/lower.rs` (MoveVec classify, `.routine` lowering, vector emit routing), `asm/assembler.rs` (vector emit arms, signature emission), `asm/disassembler.rs` (vector rendering under `caps.vectors`; executable table-section rendering), `vm/machine.rs` (v2 metadata + `run_tapes`), `vm/debug.rs` (tables + multi-device stepping), `linker/{mod.rs,layout.rs}` (table emission), workspace `Cargo.toml` (members), `docs/formats.md` (`.tma` dialect section), `docs/examples/brainfuck-utm.tma` (de-speculation).

---

### Task 1: Crate scaffold + `ARCH_TM1` + the TM-1 `Arch` implementation

**Files:**
- Modify: root `Cargo.toml` (`members = ["crates/core", "crates/post-machine", "crates/turing-machine"]`)
- Modify: `crates/core/src/formats/mod.rs` (add `pub const ARCH_TM1: u8 = 0x02;` beside `ARCH_PM1`, doc comment "TM-1, the multi-tape Turing architecture")
- Create: `crates/turing-machine/Cargo.toml`, `src/lib.rs`, `src/arch/mod.rs`
- Test: `crates/turing-machine/tests/tm1_arch.rs` + unit tests in `arch/mod.rs`

**Interfaces:**
- Produces: `Tm1::new(tape_count: u8) -> Tm1` (asserts `1..=16`); `impl Arch for Tm1` (`arch_id() == ARCH_TM1`); `pub mod opcodes` with the table below. Consumed by Tasks 2/4/5/6.

**The TM-1 opcode table (0.1)** — mirrors PM-1's numbering style; `Flow` for the syntax table in Task 2 shown here for one-place truth:

| Opcode | Mnemonic | OperandKind | Flow | Lowering (N = tape_count) |
|---|---|---|---|---|
| 0x01 | `nop` | None | FallThrough | `[Nop]` |
| 0x02 | `stp` | None | Stop | `[Stop]` |
| 0x03 | `hlt` | None | Stop | `[Halt]` |
| 0x04 | `rd`  | None | FallThrough | `[Read{dev:0,slot:0}, …, Read{dev:N-1,slot:N-1}]` |
| 0x05 | `mtc` | TableRef | FallThrough | `[MatchTable{table}]` |
| 0x06 | `djmp`| TableRef | Jump | `[DispatchJump{table}]` |
| 0x07 | `wr`  | SymbolVec | FallThrough | per element i: `0x7F` → skip (keep), else `Write{dev:i, index:v}` |
| 0x08 | `jmp` | RelI32 | Jump | `[JumpRel(off)]` |
| 0x09 | `jm`  | RelI32 | Branch | `[JumpRelIf{off, when_match:true}]` |
| 0x0A | `jnm` | RelI32 | Branch | `[JumpRelIf{off, when_match:false}]` |
| 0x0B | `call`| RelI32 | Call | `[Call(off)]` |
| 0x0C | `ret` | None | (mirror PM-1's RET flow) | `[Ret]` |
| 0x0D | `ent` | None | (mirror PM-1's ENT) | entry marker |
| 0x0E | `brk` | None | FallThrough | `[Brk]` |
| 0x0F | `mov` | MoveVec | FallThrough | per element i: 0 → skip (stay), 1 → `MoveLeft{dev:i}`, 2 → `MoveRight{dev:i}` |
| 0x1B | `call.s` | RelI8 | Call | `[Call(off)]` — `call \| 0x10`, linker-relaxation only |

Reserved, documented in a comment block, NOT implemented: `trap` 0x11, `wrmv` 0x12, `call.m` 0x13, `retx` 0x14 (producers: phase-5 linker stubs, phase-6 codegen, phase-5 frames).

Semantics that differ from PM-1 and must hold in `lower`:
- **No `LatchMatch` anywhere.** TM-1's MR is written only by `mtc` (`MatchTable`); `wr`/`mov`/`rd` do not touch it (PM-1's per-op mark latching is a PM-1-ism).
- `wr`/`mov` validate operand length == `tape_count` and (for `wr`) payload ≤ `0x7E` — violations return the `Trap` variant PM-1 uses for malformed operands (mirror its style; look at `Pm1::lower`'s error arms).
- `Read{dev,slot}` uses slot == dev (TR bank is 16 wide; tape cap 16 makes this total).

**Steps:**

- [ ] **Step 1:** Write `crates/turing-machine/Cargo.toml` mirroring `crates/post-machine/Cargo.toml` exactly (name `mtc-turing-machine`, version `0.2.0`, `edition.workspace`/`license.workspace`/`repository.workspace`, deps `mtc-core = { path = "../core" }`, serde/serde_json, dev-dep proptest). Add the member to the root `Cargo.toml`. `src/lib.rs` starts with `//!` module doc ("TM-1: everything specific to the multi-tape Turing architecture…") and `pub mod arch;`.
- [ ] **Step 2:** Add `ARCH_TM1` to `crates/core/src/formats/mod.rs`. Migrate the two hard-coded `0x02` literals in `crates/core/src/formats/executable.rs` tests (lines ~288, ~316) to the constant.
- [ ] **Step 3:** Write failing unit tests in `src/arch/mod.rs` (mirror `crates/post-machine/src/arch/mod.rs`'s test module): `operand_kind` totality over the table + `None` for unknown opcodes; each lowering row above (e.g. `rd` on a 4-tape Tm1 → exactly 4 `Read`s with dev==slot; `wr` `[5, 0x7F, 0]` on 3 tapes → `[Write{0,5}, Write{2,0}]`; `mov` `[1,0,2]` → `[MoveLeft{0}, MoveRight{2}]`); error cases: operand length ≠ tape_count, and payload > `0x7F` from a hand-built `Operand::Symbols` (`0x7F` itself is legal in `wr` — it is the keep marker); `mtc`/`djmp` lower to the table micro-ops carrying the u32; no lowering emits `LatchMatch` (assert over every opcode).
- [ ] **Step 4:** Implement `opcodes` module + `Tm1 { tape_count: u8 }` + `impl Arch for Tm1`. Run: `cargo test -p mtc-turing-machine` → PASS.
- [ ] **Step 5:** Integration smoke test `tests/tm1_arch.rs`: drive a hand-assembled byte program (raw opcode bytes + `encode_operand`) through `mtc_core::vm` driver with 2 `InfiniteTape` devices and a hand-built table blob — a 2-tape "read, match, dispatch, write, stop" loop — proving Tm1 works end-to-end against core's driver with N devices and a table ROM (core's `test_arch` already proves the core side; this proves Tm1's lowering composes with it).
- [ ] **Step 6:** `cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --check`. Commit: `feat(turing-machine): crate scaffold, ARCH_TM1, and the TM-1 arch module`

---

### Task 2: Core — `MoveVec` operand kind, instruction vector emit arms, `.routine` signature directive

All arch-agnostic; fake-dialect tested; every new lexer/CST/emit path caps-gated so PM-1 (caps off) is untouched.

**Files:**
- Modify: `crates/core/src/vm/arch.rs` (`OperandKind::MoveVec`; `encode_operand` treats `MoveVec` payloads like `Symbols` — same compact self-delimiting encoding; move payloads are 0/1/2 so ≤ 0x7E holds)
- Modify: `crates/core/src/vm/core.rs` (fetch decode: `MoveVec` walks the same compact loop as `SymbolVec`)
- Modify: `crates/core/src/asm/decode.rs` (dis decode: `MoveVec` → `DecodedOperand::Ints`, same walk)
- Modify: `crates/core/src/asm/lexer.rs` (new `Eq` token for `=`, gated under `caps.tables`, mirroring 4a's gating pattern)
- Modify: `crates/core/src/asm/cst.rs` (new `AsmItemKind::RoutineDirective(RoutineDirectiveCst)`; `RoutineDirectiveCst { name: SpannedName, tapes: u32, alpha: Vec<u32>, span: Span, trailing: … }` parsed from `.routine <name>, tapes=<int>, alpha=(<int>,…)` — note `(`/`)` lex under `caps.rept`, so the directive requires both caps in practice; document that in the parser comment)
- Modify: `crates/core/src/asm/lower.rs` (classify `MoveVec` leading-`[` operands as `SourceOperand::Vector` exactly like `SymbolVec`; lower `.routine` into a per-blob `RoutineSig`)
- Modify: `crates/core/src/asm/assembler.rs` (two new emit arms + signature plumbing into `ObjectFile.signatures`)
- Modify: `crates/core/src/asm/disassembler.rs` + `fmt.rs` if needed (render vectors under `caps.vectors`)
- Modify: `crates/core/src/asm/mod.rs` (`AsmErrorKind::BadSignature` — duplicate `.routine` for one func, unknown func name, arity outside `1..=16`, cardinality 0, alpha len ≠ tapes)
- Test: extend the existing fake-dialect test modules in `assembler.rs` / `disassembler.rs` / `cst.rs`, plus `crates/core/tests/asm_tables.rs` if a round-trip fits there; extend `crates/core/tests/operand_codec.rs` property tests over `MoveVec`

**Interfaces:**
- Produces: `OperandKind::MoveVec`; assembler emit arms `(SymbolVec, SourceOperand::Vector)` and `(MoveVec, SourceOperand::Vector)`; `ObjectFile.signatures` populated from `.routine`. Consumed by Tasks 3/5/6.

**Emit-arm vocabulary rules** (the assembler is arch-agnostic, so vocabulary is enforced per OperandKind, not per mnemonic):
- `SymbolVec` × `Vector`: `Payload(v ≤ 0x7E)` → v; `Keep` → `0x7F`; `Wildcard`/`MoveLeft`/`MoveRight`/`Stay` → `AsmErrorKind::BadVector` ("move/wildcard element in a write vector" — wildcards belong to `.row`).
- `MoveVec` × `Vector`: `Stay` → 0, `MoveLeft` → 1, `MoveRight` → 2; anything else → `BadVector`.
- Existing `(SymbolVec, Ints)` arm (PM-1's `wr 1`) is untouched.

**Dis rendering rule** (byte-identity lever): when `syntax.caps.vectors` is set, a `SymbolVec` operand renders as `[e0,e1,…]` with `0x7F` → `-`, and a `MoveVec` renders as `[m0,…]` with 0 → `.`, 1 → `<`, 2 → `>`; with caps off the legacy plain-ints rendering is byte-identical to today (regression-pin with a test).

**Steps:**

- [ ] **Step 1:** Failing property test in `crates/core/tests/operand_codec.rs`: `MoveVec` vectors of 1..=16 elements in {0,1,2} encode/fetch-decode/dis-decode round-trip; encode rejects empty.
- [ ] **Step 2:** Implement `OperandKind::MoveVec` through arch.rs / core.rs / decode.rs. Tests pass.
- [ ] **Step 3:** Failing assembler tests (fake dialect — extend `fake_syntax()` with `vmove` = opcode 0x18, `OperandKind::MoveVec`; `vwrite` 0x07 `SymbolVec` already exists): `vwrite [1,-,2]` assembles to `0x07` + encoded `[1,0x7F,2]`; `vmove [<,.,>]` → `0x18` + `[1,0,2]`; vocabulary violations produce `BadVector` with the operand's span; `vwrite [1,*]` rejected.
- [ ] **Step 4:** Implement the two emit arms in `assembler.rs` (route `SourceOperand::Vector` by OperandKind) + `classify_operand` MoveVec branch in lower.rs. Tests pass.
- [ ] **Step 5:** Failing dis tests: assemble the Step-3 programs, `disassemble_object` renders `vwrite [1,-,2]` / `vmove [<,.,>]` back byte-identically (assemble∘dis fixpoint); a caps-off dialect's `SymbolVec` rendering is unchanged vs. today (pin with a PM-1-shaped fake, caps default).
- [ ] **Step 6:** Implement rendering. Tests pass.
- [ ] **Step 7:** Failing `.routine` tests: lexer `Eq` gating (caps off → `=` stays Junk; pin byte-compat); CST shape (`.routine main, tapes=2, alpha=(3,5)` → RoutineDirective node, trivia preserved, `format_asm_with` idempotent and verbatim); lower/assemble → `obj.signatures == Some(vec![…])` with `RoutineSig { arity: 2, cardinalities: vec![3,5] }` parallel to the named func's blob, `is_v2_shape()` false (v3 emit engages — reuse 4a's flag plumbing); each `BadSignature` case errors with span.
- [ ] **Step 8:** Implement lexer token, CST node, lowering, signature emission. Tests pass.
- [ ] **Step 9:** Full gate: `cargo test --workspace`, clippy, fmt, `git status --short crates/post-machine/tests/golden/` empty. Commit: `feat(core): MoveVec operands, instruction vector emission, and the .routine signature directive`

---

### Task 3: Core linker — table-section emission + executable-level table disassembly

**Files:**
- Modify: `crates/core/src/linker/layout.rs` (table gathering, dispatch rebase, TableRef patching; `Built` gains `tables: Vec<u8>` + per-function table bases)
- Modify: `crates/core/src/linker/mod.rs` (`LinkError::{UnsupportedBindings(String), MissingSignature(String), MalformedTable { symbol: String, at: u32 }}`; sectioned-vs-code_only emit decision; header fields from the entry function's `RoutineSig`)
- Modify: `crates/core/src/asm/disassembler.rs` (`disassemble_executable` renders `exe.tables` as a `.section tables` block — synthesized `T<n>` labels, `.row`/`.targets` directives, dispatch entries resolved to code labels via the `MapFile` when present, raw `0x…` addresses otherwise; kind inference reuses 4a's Flow-driven approach, now walking `TableRef` operands found in the linked code)
- Test: extend `crates/core/src/linker/` unit tests + `crates/core/tests/asm_tables.rs` (or a new `crates/core/tests/link_tables.rs` — implementer's choice, one home)

**Interfaces:**
- Consumes: `ObjectFile.{table_blobs, table_fixups, signatures, bound_calls}` (Task 2 emits signatures; 4a emits tables/fixups); `Executable::sectioned(...)` (exact signature in `executable.rs:43`).
- Produces: `link(...)` emits a v2 sectioned image when table content or signatures are present; byte-identical `code_only` behavior otherwise. Consumed by Tasks 5/6.

**The algorithm (spell it in code comments citing `docs/formats.md (executable image)`):**
1. **Guard:** any object with non-empty `bound_calls` → `Err(LinkError::UnsupportedBindings(symbol))` — bindings need the phase-5 composition engine.
2. **Gather:** for each laid-out function (in `resolve.order`) with a table blob, assign a table base = running length of the concatenated section; record `(blob index → table base)`.
3. **Rebase dispatch entries:** walk each function's table blob using the fixup-derived kind inference (each `TableFixup.table_offset` names a table start; the kind comes from the referencing instruction's mnemonic Flow, exactly as `disassemble_object`'s `render_tables_section` infers it; match tables are `3 + width*count` bytes, dispatch `2 + 4*count`). For every dispatch entry `v` (u32 LE, blob-relative code offset within the owning function): new value = the function's final code base combined with the intra-function offset map that relaxation maintains (`orig_to_new` in layout.rs:264-338 — the same map jump re-encoding uses). Table bytes not covered by any fixup-attributed table → `LinkError::MalformedTable`.
4. **Patch TableRef holes:** each `TableFixup { blob, offset, table_offset }` → write `u32 LE (section base of blob + table_offset)` at the final code position of `offset` (mapped through `orig_to_new`).
5. **Header:** if any table content or signature exists, the entry function's `RoutineSig` is REQUIRED (`MissingSignature` otherwise) and supplies `tape_count` = arity, `alphabet_cardinalities` = cardinalities, `profile` = 0 (base; frames = phase 5); emit `Executable::sectioned(...)`. Otherwise `Executable::code_only(...)` exactly as today.

**Steps:**

- [ ] **Step 1:** Failing linker tests (fake dialect from `fake_syntax()` — assemble sources with `tmatch`/`tdispatch`/`.routine`): (a) single function with one match + one dispatch table links to a sectioned image whose `tables` bytes equal an independently derived section (mirror 4a's derivation style) and whose dispatch entries equal the final absolute addresses of the target labels; (b) TWO functions with tables (main calls the second) — per-function bases correct, `TableRef` operands point at the right section offsets; (c) relaxation shift: a `call`→`call.s` narrowing BEFORE a dispatch-target label moves the label — dispatch entries follow (this is the 4a forward-note's "phase-5 rebasing dependency", landing here); (d) `bound_calls` present → `UnsupportedBindings`; (e) tables without an entry-function signature → `MissingSignature`; (f) a tableless PM-1-shaped link is byte-identical to today's output (lock test).
- [ ] **Step 2:** Implement layout/mod changes. Tests pass.
- [ ] **Step 3:** Failing dis test: link the Step-1(a) program, `disassemble_executable` with the map renders a `.section tables` block whose `.targets` name the map's labels; without the map, raw addresses; the rendered text re-assembles + re-links to a byte-identical executable (the strong round-trip — single-function case).
- [ ] **Step 4:** Implement executable-level table rendering. Tests pass.
- [ ] **Step 5:** Full gate incl. goldens check. Commit: `feat(core): linker emits the table section — dispatch rebasing, TableRef patching, sectioned images`

---

### Task 4: Core — `Machine` carries v2 metadata; multi-device run + debug

**Files:**
- Modify: `crates/core/src/vm/machine.rs`, `crates/core/src/vm/debug.rs`
- Test: machine.rs/debug.rs unit tests with `test_arch` (0x7F) + multi-device programs

**Interfaces:**
- Produces:
  ```rust
  // Machine gains: tables: Vec<u8>, tape_count: u8, alphabet_cardinalities: Vec<u32> (from exe)
  pub fn run_tapes(&self, devices: &mut [&mut dyn Tape], opts: RunOptions) -> Result<RunResult, RunSetupError>
  pub fn debug_tapes(&self, opts: RunOptions) -> DebugSession  // session carries self.tables
  // DebugSession gains: pub fn step_in_tapes(&mut self, devices: &mut [&mut dyn Tape]) -> DebugEvent
  pub enum RunSetupError { DeviceCount { expected: u8, got: usize }, AlphabetMismatch { tape: u8, expected: u32, got: u32 } }
  ```
  Consumed by Task 5's `tmt run`.

**Rules:**
- `from_executable` copies `exe.tables` / `exe.tape_count` / `exe.alphabet_cardinalities` into the struct (it currently drops them — machine.rs:97-105); existing validation unchanged.
- `run_tapes` validates `devices.len() == tape_count` and, when `alphabet_cardinalities` is non-empty, each `devices[i].alphabet_size() == cardinalities[i]`; passes `&self.tables` to the driver (not `&[]`); does **NOT** preload MF (no `set_mf` — MR starts 0; PM-1's mark preload stays only in the legacy single-tape `run`, which is reimplemented as a thin wrapper over the same internals preserving its exact current behavior).
- `DebugSession` gains a `tables: Vec<u8>` field (constructor addition or a `with_tables` builder — keep the existing `new` signature compiling for pmt by defaulting to empty); `step_in_tapes` mirrors `step_in`; `step_in(&mut dyn Tape)` becomes a one-device wrapper.

**Steps:**

- [ ] **Step 1:** Failing tests: `test_arch` program using dev-0 and dev-1 micro-ops + a match/dispatch table runs through `Machine::from_executable(sectioned image)` + `run_tapes` with 2 tapes end-to-end; `DeviceCount` and `AlphabetMismatch` cases; the same program steps through `debug_tapes`/`step_in_tapes` with a breakpoint; legacy `run`'s mark preload pinned unchanged (existing tests should already cover — verify, add if not).
- [ ] **Step 2:** Implement. Tests pass.
- [ ] **Step 3:** Full gate incl. goldens. Commit: `feat(core): Machine loads v2 images whole — table ROM, multi-device run and debug`

---

### Task 5: `tm1_syntax()`, the `.tma` dialect, and the minimal `tmt` CLI

**Files:**
- Create: `crates/turing-machine/src/asm/mod.rs` (`TM1_TMA_DIALECT_VERSION = "0.1"`, `tm1_syntax()`, wrappers `assemble`/`disassemble_object`/`disassemble_executable`/`listing_executable`/`link` mirroring `crates/post-machine/src/asm/mod.rs:169-198`)
- Create: `crates/turing-machine/src/cli/{mod.rs,build.rs,inspect.rs,run.rs}`, `src/bin/tmt.rs`
- Modify: `crates/turing-machine/src/lib.rs` (re-exports mirroring post-machine's lib.rs)
- Test: `crates/turing-machine/tests/{tma_dialect.rs,cli_programs.rs}`

**Interfaces:**
- Consumes: everything from Tasks 1–4.
- Produces: `mtc_turing_machine::cli::execute(&[String]) -> Result<CliOutput, String>` (own `CliOutput`/`Args` copies — mirror `crates/post-machine/src/cli/mod.rs:14-29,120-184` verbatim-adapted; hoisting to core is a phase-7 decision, note it in a comment carried in prose).

**`tm1_syntax()`:** entries per Task 1's table (mnemonic strings lowercase as shown; `call.s` mnemonic exactly as PM-1 spells its short forms — check `pm1_syntax()`), `relax_pairs: vec![RelaxPair { far: CALL, short: CALL_S }]`, `entry_opcode: ENT`, `break_opcode: Some(BRK)`, `caps: AsmCaps { tables: true, rept: true, vectors: true }`.

**CLI surface (5 subcommands + `--version`):**
- `tmt asm IN.tma [-o OUT.tmo]` — mirror `pmt asm`'s flags/default-output-naming exactly (read `crates/post-machine/src/cli/build.rs` and copy the shape; extension `.tmo`).
- `tmt link IN.tmo… [-o OUT.tmx] [--no-relax]` — mirror `pmt link` (map sidecar `OUT.tmx.map` via `MapFile::to_json`, same as pmt).
- `tmt dis FILE` — sniff-dispatch object vs executable (mirror pmt `dis`; `--map` / auto-sidecar behavior copied).
- `tmt run PROG.tmx --tape TAPES.tmt [--trace] [--max-steps N] [--max-tacts N]` — loads the MT block, builds one `InfiniteTape::from_snapshot` per tape (count must match; glyph alphabets sized per tape), fresh `ArchRegistry` with `Tm1::new(exe.tape_count)` registered, `run_tapes`, exit codes 0/2/3; `--trace` steps via `debug_tapes` + `listing_line` like `pmt run --trace` (run.rs:274). On finish, print each final tape via a `render_tape` copy.
- `tmt tape show|new|set` — mirror `crates/post-machine/src/cli/inspect.rs:105-298` with the v2 upgrade: `tape new --from PROG.tmx` emits an MT whose per-tape alphabets are sized from the image's `alphabet_cardinalities` (tape i gets glyphs `["0", "1", …, "card-1"]` as decimal-string labels — MT v2 per-tape alphabets; the 4a carry-over item). `tape set` clone semantics (`-o` XOR `--in-place`) exactly as pmt.
- `tmt --version` → two lines: `tmt {CARGO_PKG_VERSION}` and `tma dialect (tm-1) {TM1_TMA_DIALECT_VERSION}` (the `.tmc` language line joins in phase 6).
- `bin/tmt.rs` = byte-for-byte `bin/pmt.rs` with names swapped.

**Steps:**

- [ ] **Step 1:** Failing dialect tests (`tests/tma_dialect.rs`): every Task-1 mnemonic assembles and dis-round-trips; a small sectioned program (2 tapes: table + `rd/mtc/djmp/wr [·,-]/mov [>,.]/jmp/stp`) assembles → links → dis renders both sections; `format_asm_with(src, tm1_syntax().caps)` idempotent on it; `.routine` + vectors + `.rept` accepted together.
- [ ] **Step 2:** Implement `asm/mod.rs`. Tests pass.
- [ ] **Step 3:** Failing CLI tests (`tests/cli_programs.rs`, in-process pattern with `args()`/`scratch()` helpers copied from pmt's cli_programs.rs:8-16): `--version` output exact; asm→link→tape new→tape set→run pipeline on the Step-1 program with exit code 0; a `hlt` variant exits 2; an MR=0 `djmp` variant exits 3 (trap); `run` with wrong tape count fails as a tool error (exit 1, message names the mismatch); `tape new --from` writes per-tape alphabets sized by the header's cardinalities (open the MT and assert).
- [ ] **Step 4:** Implement the CLI. Tests pass.
- [ ] **Step 5:** Full gate. Commit: `feat(turing-machine): the .tma dialect (tm1_syntax) and the minimal tmt CLI — asm, link, dis, run, tape`

---

### Task 5b (execution-time amendment): `WideTape` — a wide-alphabet core tape device

**Why (discovered during Task 5):** `InfiniteTape` is physically two-symbol — its storage is a packed bit array (`HashMap<i64, u64>` pages). The spec's §7 assumption that tape devices are "reused as-is" fails for TM-1's wide alphabets; the UTM's 9- and 127-symbol tapes cannot exist at runtime. Additive fix; `InfiniteTape` untouched (PM-1's hot path stays byte-identical).

**Files:**
- Create: `crates/core/src/vm/devices/wide_tape.rs` (+ export from `devices/mod.rs`, re-export at `vm::` level beside `InfiniteTape`)
- Modify: `crates/turing-machine/src/cli/run.rs` (build `WideTape` universally — width = each tape's effective-alphabet length; a 2-symbol tape is just width 2)
- Test: unit tests in `wide_tape.rs`; a `StrictTape<WideTape>` sanity test; a tmt CLI test running a 3-symbol program end-to-end (1 tape, `alpha=(3)`, write symbol 2, `stp`, assert exit 0 and the final tape)

**API:**
```rust
pub struct WideTape { cells: HashMap<i64, u32>, width: u32, head: i64 }
impl WideTape {
    pub fn new(width: u32) -> Self                       // width >= 1; 0 is a caller bug (assert)
    pub fn from_snapshot(s: &TapeSnapshot, width: u32) -> Result<Self, DeviceFault>  // cells >= width → IndexOutsideAlphabet
    pub fn to_snapshot(&self) -> TapeSnapshot            // mirror InfiniteTape::to_snapshot's trim/origin policy exactly
    pub fn head(&self) -> i64
}
impl Tape for WideTape  // read: 0 for unset cells; write: index >= width → IndexOutsideAlphabet; alphabet_size() = width
```

**Steps:** failing unit tests (round-trip, default-blank reads, out-of-width write fault, from_snapshot rejection, StrictTape decoration) → implement → failing tmt 3-symbol CLI test → switch run.rs to WideTape → full gate (workspace tests, clippy `-D warnings`, fmt, goldens clean) → commit `feat(core): WideTape — wide-alphabet tape device` + `feat(turing-machine): run builds WideTape devices` (or one commit).

---

### Task 6: UTM de-speculation — the phase-4 milestone — plus docs and the phase gate

**Files:**
- Modify: `docs/examples/brainfuck-utm.tma`
- Modify: `docs/formats.md` (new `.tma` dialect subsection in the assembly-text area, sibling to the `.pma` one: mnemonic table, vector notation `*`/`-`/`<`/`>`/`.`, sections/`.row`/`.targets`/`.target`/`.rept`/`.routine`, the compact-family `0x7F` transparency rule, dialect version 0.1)
- Create: `crates/turing-machine/tests/golden_programs.rs`, `tests/golden/*.tmt`
- Test: this task IS tests + docs + gate

**The example must actually assemble — known required edits (fold back into the file, spec-sanctioned):**
1. **Labeled `.rept` headers don't shape as labels** (4a carry-over). Respell the three dispatch-table rept blocks moving the label onto the body line, where 4a's same-label continuation rule merges expanded lines into one table:
   ```
   ; before                          ; after
   Dinc:   .rept v, 0, 126           .rept v, 0, 126
           .target Linc{v}           Dinc:   .target Linc{v}
           .endr                     .endr
   ```
   (Same for `Ddec:`/`Ddot:`. The continuation rule was tested for `.row` in 4a — if `.target` continuation is not yet covered, extend the core test in `assembler.rs` first, as a fake-dialect test.)
2. **Add the signature directive** before `.func main`: `.routine main, tapes=4, alpha=(9,127,127,2)`.
3. **Header comment rewrite:** drop "SPECULATIVE (arch 0x02 is not implemented…)" — state it is the working flagship example, assembled and run by the test suite. Remove the forge reference ("mellonis/machines-demo#64") per the published-docs policy — describe in prose: "Source algorithm: a ~20-state universal Turing machine interpreting brainfuck, from the machines-demo project."
4. Any FURTHER syntax adjustment the assembler forces — fold it into the file AND record it in the task report (the spec's de-speculation contract: "syntax adjustments discovered on the way are folded back into the example").

**Golden tests (derivation-first — mirror `crates/post-machine/tests/golden_programs.rs`'s discipline exactly, including its snapshot-normalization approach):**
- A tiny brainfuck reference interpreter INSIDE the test (independent derivation; cells wrap mod 127 to mirror the UTM's `%127` arithmetic; unbounded tape both directions).
- Two programs: `+++.` (straight-line: expects out tape `[3]`) and `++[>+++<-]>.` (loops: expects out `[6]`), each: build the 4-tape MT programmatically (prog tape = bf symbol indices + `8` sentinel per the file's alphabet comment, others blank), assemble+link the UTM via library calls, `run_tapes`, assert `Outcome::Stopped`, assert the out/data tapes match the reference interpreter's derivation, assert prog tape unchanged, cnt tape blank (normalized); then assert the committed `.tmt` golden is byte-identical to the derived snapshot.
- One trap case: a prog tape containing an index outside the alphabet's dispatch rows (e.g. `0` under the head at fetch — no row, no catch-all) → `Outcome::Trapped(Trap::NoTransition { .. })` — the UTM's "free invalid-opcode fault" comment, now proven.

**Steps:**

- [ ] **Step 1:** Verify/extend the `.target` same-label continuation core test (fake dialect, in `assembler.rs`'s test module).
- [ ] **Step 2:** Apply the example edits; loop `tmt asm docs/examples/brainfuck-utm.tma` (via a scratch test or the CLI) until it assembles; fold every forced adjustment back into the file; record each in the report.
- [ ] **Step 3:** Write the failing golden tests; run to see the real outcome; debug the UTM/toolchain until the derivations hold (report any toolchain bug found — fix it in the owning module with its own test).
- [ ] **Step 4:** Regenerate-and-commit the goldens FROM THE DERIVATION (write the derived bytes in the test's regen path, mirroring pmt's `#[ignore]` regen convention if one exists in golden_programs.rs — check and mirror).
- [ ] **Step 5:** Write the `docs/formats.md` `.tma` section (forge-agnostic, no spec refs).
- [ ] **Step 6:** Full phase gate, report each verbatim: `cargo test --workspace` · `cargo clippy --workspace --all-targets -- -D warnings` · `cargo fmt --check` · `git status --short crates/post-machine/tests/golden/` (empty). CLI round-trip sanity by hand: `cargo run -p mtc-turing-machine --bin tmt -- asm docs/examples/brainfuck-utm.tma -o /tmp/utm.tmo` then link/dis — paste outputs into the report.
- [ ] **Step 7:** Commit: `feat(turing-machine): the brainfuck UTM assembles, links, and runs — phase-4 milestone` (example edits + goldens + tests; docs/formats.md may ride the same commit or its own `docs(formats):` — implementer's choice, both conventional).

---

## Self-review notes (run at plan time)

- Spec coverage for phase 4 (spec §17 row 4): arch module ✅ T1; `.tma` assembler over the 4a framework ✅ T2+T5; minimal CLI ✅ T5 (+link, recorded deviation); UTM milestone ✅ T6. Deferred WITH recorded producers: `wrmv`, `trap`, n-byte symbol family (>127 alphabets have no consumer until phase 6 language ranges — first alphabet wider than 127 symbols triggers it), `.frame`/`call.m`/`retx` (phase 5), map-sidecar binding labels (phase 5).
- Type-consistency: `Tm1::new(tape_count)` (T1) is what T5's `run` constructs; `run_tapes`/`step_in_tapes` (T4) are what T5 calls; `RoutineSig` field names per `object.rs:98` (`arity`, `cardinalities`); `Executable::sectioned` argument order per `executable.rs:43`.
- 4a carry-over disposition: labeled-`.rept` → T6 respelling (+continuation test); `tape new` v2 sizing → T5; dispatch dis round-trip on linked images → T3; `.rept` iteration cap + `{expr}` depth cap → NOT taken (recorded as optional hardening, no untrusted-input consumer yet); 3a leftovers (v2 inherit-path cell rejection, CRC-gated proptests) → NOT taken here, remain on the ledger for phase 5's format-touching round.
