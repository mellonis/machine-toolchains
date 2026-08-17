# DAP Adapters Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship `pmt dap` and `tmt dap` — Debug Adapter Protocol servers
over `vm::DebugSession` — plus VS Code packaging for both extensions,
per the approved spec.

**Architecture:** A `core/src/dap/` sibling framework beside
`core/src/lsp/` (shared `Content-Length` framing extracted; typed DAP
envelope; a `DebugAdapter` trait mirroring `LanguageService`; a server
loop with one stdin reader thread and a run-tick seam). Additive VM API
(`Tape::poke`, `_tapes` stepping siblings, MF/MR access). Per-arch
adapters in the arch crates, wired as `cli/dap.rs` on the `cli/lsp.rs`
template. Phases: core framework → PM adapter → TM adapter → packaging.

**Tech Stack:** Rust, `serde`/`serde_json`, `std::thread` +
`std::sync::mpsc` only — no new dependencies. TypeScript only inside
`editors/vscode-{pm,tm}`.

**Spec:** `docs/superpowers/specs/2026-08-16-dap-adapters-design.md`
(binding; read it first — §5's closed output list, §6's writable-state
contract, and the granularity semantics are requirements).

## Global Constraints

- No new crate dependencies anywhere in the workspace.
- `crates/core` stays arch-agnostic: zero PM-1/TM-1 knowledge; the DAP
  framework is fake-adapter tested.
- LSP behavior is byte-stable: the framing extraction (Task 1) must
  leave every existing LSP test passing unchanged.
- The thin-renderer rule: `cli/dap.rs` is the ONLY new place real stdio
  reaches library code (the `cli/lsp.rs` precedent verbatim).
- Output events are the spec §5 closed list — nothing else, ever.
- Gates on every commit: `cargo clippy --workspace --all-targets -- -D warnings`,
  `cargo fmt --check`, the touched crate's suite green.
- Doc comments cite durable pages only (`docs/dap.md (…)` once Task 13
  lands it; carry substance in prose meanwhile). No issue/spec refs in
  code or published docs. Conventional commits with scope; no AI
  attribution footers.
- Branch: `feat/dap-adapters` in the `../toolchains-dap` worktree.

---

### Task 1: extract the shared framing codec

**Files:**
- Create: `crates/core/src/framing.rs`
- Modify: `crates/core/src/lsp/transport.rs`, `crates/core/src/lib.rs`

**Interfaces:**
- Produces: `pub fn read_message(reader: &mut dyn BufRead) -> Result<Option<String>, TransportError>`
  and `pub fn write_message(writer: &mut dyn Write, payload: &str) -> Result<(), TransportError>`
  plus `TransportError`, moved verbatim from `lsp/transport.rs:57,126`
  into `core::framing`; `lsp::transport` re-exports them so every
  existing import path keeps compiling. Tasks 2–3 consume
  `core::framing`.

- [ ] **Step 1:** Move the two functions, `TransportError`, and the
  64 MiB cap logic verbatim to `framing.rs`; make `lsp/transport.rs`
  `pub use crate::framing::{read_message, write_message, TransportError};`
  keeping any LSP-only helpers in place. Move the transport unit tests
  that test framing (not LSP semantics) with them.
- [ ] **Step 2:** Run: `cargo test -p mtc-core` → all green, zero test
  edits beyond the file move. Run both arch crates' LSP-touching suites:
  `cargo test -p mtc-post-machine --test '*lsp*'` equivalent (find the
  LSP test files by `ls crates/post-machine/tests | grep -i lsp`) — green.
- [ ] **Step 3:** Gates + commit:
  `refactor(core): extract the Content-Length framing codec shared by lsp and dap`

---

### Task 2: the DAP envelope (`core/src/dap/protocol.rs`)

**Files:**
- Create: `crates/core/src/dap/mod.rs`, `crates/core/src/dap/protocol.rs`

**Interfaces:**
- Produces (Task 3 consumes): serde types
  `ProtocolMessage { seq: u64, kind: MessageKind }` with
  `MessageKind::Request { command: String, arguments: serde_json::Value }`,
  `MessageKind::Response { request_seq: u64, success: bool, command: String, message: Option<String>, body: serde_json::Value }`,
  `MessageKind::Event { event: String, body: serde_json::Value }` —
  serialized to the DAP wire shape (`"type": "request" | "response" | "event"`,
  flat fields, `#[serde(tag/rename)]` as needed to match DAP JSON
  exactly). Helpers: `ProtocolMessage::response_to(req_seq, seq, command, Result<serde_json::Value, String>)`,
  `ProtocolMessage::event(seq, name, body)`.

- [ ] **Step 1 (RED):** unit tests in `protocol.rs`: decode a literal
  DAP `initialize` request JSON string (copy a real one from the DAP
  spec) into the typed shape; encode a response/event and compare
  against expected JSON `Value`s; round-trip an unknown command
  (arguments preserved as raw `Value`).
- [ ] **Step 2:** implement; `cargo test -p mtc-core dap` green.
- [ ] **Step 3:** gates + commit:
  `feat(core): dap protocol envelope types`

---

### Task 3: the `DebugAdapter` trait and server loop

**Files:**
- Create: `crates/core/src/dap/server.rs`
- Modify: `crates/core/src/dap/mod.rs`

**Interfaces (Tasks 6–11 consume — exact shapes):**

```rust
pub enum AdapterEvent { Stopped { reason: &'static str, description: Option<String> },
                        Output { category: &'static str, output: String },
                        Terminated, Exited { code: i32 } }
pub enum RunState { Stopped, Running, Done }
pub trait DebugAdapter {
    /// Handle one request; push events via `out`. Returns the response
    /// body or a DAP error message.
    fn handle(&mut self, command: &str, arguments: &serde_json::Value,
              out: &mut Vec<AdapterEvent>) -> Result<serde_json::Value, String>;
    /// Called by the loop when no request is pending and state is
    /// Running: advance one bounded slice (the adapter calls
    /// run_steps_tapes(BUDGET) internally), pushing events on pause /
    /// completion.
    fn tick(&mut self, out: &mut Vec<AdapterEvent>) -> RunState;
    fn run_state(&self) -> RunState;
}
pub fn run(reader: impl std::io::Read + Send + 'static,
           writer: &mut dyn std::io::Write,
           adapter: &mut dyn DebugAdapter) -> i32;
```

The loop: spawn the reader thread (`std::thread` + `mpsc::channel`)
decoding frames via `core::framing`; main loop — when `run_state()` is
`Running`, `try_recv` then `tick()`; otherwise block on `recv`.
Dispatch requests to `handle`, own all seq numbering, translate
`AdapterEvent`s to protocol events, answer unknown commands with the
uniform unsupported-command error, enforce the lifecycle guard
(`initialize` first; everything after `disconnect` → error; loop exits
after `disconnect` response, joining the reader by closing).

- [ ] **Step 1 (RED):** fake-adapter tests over in-memory pipes
  (`std::io::Cursor` for input assembled from framed messages; a
  `Vec<u8>` writer): initialize/disconnect lifecycle; seq monotonicity
  and request_seq echo; unknown command error; a scripted fake whose
  `tick` emits `Stopped` after N calls proving the pump processes a
  `pause` request queued mid-run; post-disconnect request rejected.
- [ ] **Step 2:** implement; `cargo test -p mtc-core dap` green.
- [ ] **Step 3:** gates + commit:
  `feat(core): dap server loop with reader thread and run-tick seam`

---

### Task 4: additive VM API

**Files:**
- Modify: `crates/core/src/vm/devices/mod.rs` (`Tape::poke` default
  method), `crates/core/src/vm/debug.rs` (`mr()`, `set_mf`, `set_mr`,
  and `continue_tapes` / `run_steps_tapes` / `step_over_tapes` /
  `step_out_tapes` mirroring the existing single-device fns via
  `step_in_tapes`'s pattern)

**Interfaces:**
- `fn poke(&mut self, pos: i64, index: u32) -> Result<(), DeviceFault>`
  default impl on `trait Tape`: walk head to `pos` with
  `left()`/`right()`, `write(index)?`, walk back — restoring the head
  even when the write faults (walk back before returning the error).
- `DebugSession::{mr, set_mf, set_mr}` over the core register access
  (`vm/core.rs:206,211` shows the existing mf/mr storage).

- [ ] **Step 1 (RED):** core unit tests — poke restores head on success
  AND on a `StrictTape` fault (fault surfaces as `Err(DeviceFault)`);
  poking the head cell leaves `mf()` unchanged mid-session; `_tapes`
  stepping drives a two-device fake exactly as the single-device twin
  drives one; `set_mf` flips the flag a subsequent check-shaped
  instruction reads (use the crate-private fake arch, `vm/arch.rs::test_arch`).
- [ ] **Step 2:** implement; `cargo test -p mtc-core` green.
- [ ] **Step 3:** gates + commit:
  `feat(core): Tape::poke, flag setters, and _tapes stepping siblings for dap`

---

### Task 5: the line-map helper

**Files:**
- Create: `crates/core/src/linemap.rs` (exported from `lib.rs`)

**Interfaces (Tasks 6–11 consume):**

```rust
pub struct LineIndex { /* built once from &MapFile */ }
impl LineIndex {
    pub fn new(map: &MapFile) -> LineIndex;
    /// Containing function + its mapped source line (largest mapped
    /// offset <= addr), if any.
    pub fn resolve(&self, addr: u32) -> Option<(&str, Option<u32>)>;
    /// First address mapped at-or-after `line` inside any function —
    /// the breakpoint planting rule. None = unverifiable line.
    pub fn address_for_line(&self, line: u32) -> Option<u32>;
}
```

- [ ] **Step 1 (RED):** unit tests over a hand-built `MapFile` (two
  functions, `lines` tables incl. gaps): resolve at exact offset,
  between offsets, outside any function; address_for_line exact, gap
  (next mapped line wins), past-the-end (None).
- [ ] **Step 2:** implement (sorted vectors + binary search; no maps
  needed); green; gates + commit:
  `feat(core): LineIndex - address/line resolution over the map sidecar`

---

### Task 6: PM adapter skeleton — program-mode launch, run control, termination

**Files:**
- Create: `crates/post-machine/src/dap/mod.rs`,
  `crates/post-machine/src/cli/dap.rs`
- Modify: `crates/post-machine/src/cli/mod.rs` (subcommand dispatch +
  help table), `crates/post-machine/src/completions/registry.rs`
  (`dap_spec()` on the `lsp_spec()` template at registry.rs:597, added
  to the registry list), `crates/post-machine/tests/completions_registry.rs`
  (`EXPECTED_TOP_LEVEL` += "dap")
- Test: `crates/post-machine/tests/dap_programs.rs`

**Interfaces:**
- Produces `PmDapAdapter` implementing `mtc_core::dap::DebugAdapter`;
  handles v1 lifecycle + `launch` (program mode: `program` path +
  optional `tape`, PM empty-tape default), `configurationDone`,
  `threads`, `continue`, `pause`, `disconnect`; `stopOnEntry`; the
  termination path (summary output event + `terminated` + `exited`
  with 0/2/3). `cli/dap.rs` mirrors `cli/lsp.rs` byte-for-byte in
  shape (usage text, no-args check, stdio handoff to
  `mtc_core::dap::server::run`).
- Tasks 7–9 extend `PmDapAdapter` — keep launch/session state in
  clearly named fields (`session`, `tape`, `line_index`, `launch_opts`).

- [ ] **Step 1 (RED):** scripted-conversation tests: build a tiny
  fixture `.pmx` + `.pmt` in the test (compile+link in-process, write
  to a pid+counter scratch dir — the `lint_programs.rs` isolation
  pattern), then drive `PmDapAdapter` DIRECTLY (call `handle`/`tick`
  — no stdio): launch → configurationDone → continue → tick-to-
  completion asserts the summary output event, `terminated`, `exited 0`
  for a `stp` program; a `hlt` program exits 2; a trapping program
  stops with `reason: "exception"` first; `stopOnEntry: true` yields
  `stopped("entry")` before any step; `pause` mid-run yields
  `stopped("pause")`.
- [ ] **Step 2:** implement; `cargo test -p mtc-post-machine --test dap_programs` green.
- [ ] **Step 3:** run the completions drift guard
  (`cargo test -p mtc-post-machine --test completions_registry`) —
  green with the new entry.
- [ ] **Step 4:** gates + commit:
  `feat(post-machine): pmt dap skeleton - launch, run control, termination`

---

### Task 7: PM stepping and breakpoints

**Files:**
- Modify: `crates/post-machine/src/dap/mod.rs`
- Test: extend `crates/post-machine/tests/dap_programs.rs`

**Interfaces:** adds `setBreakpoints`, `setInstructionBreakpoints`,
`next`/`stepIn`/`stepOut` with granularity semantics (spec §5): line
granularity repeats session steps until `LineIndex::resolve(ip).line`
changes (breakpoint/brk/trap interrupts win and report their own
reason); `granularity: "instruction"` = one session step; `stepOut` →
`step_out_tapes`. Source breakpoints verify via
`LineIndex::address_for_line` (`verified: false` + "build with -g"
message when unmapped); instruction breakpoints take the
`instructionReference` hex address directly to `add_breakpoint`.

- [ ] **Step 1 (RED):** tests against a fixture with a call and a
  multi-instruction source line: a line `next` collapses several
  instructions (assert one `stopped("step")` and the line changed); an
  instruction step advances IP by exactly one instruction; a breakpoint
  planted mid-line interrupts a line step with
  `stopped("breakpoint")`; `stepIn`/`stepOut` around the call behave
  depth-wise; a source breakpoint on an unmapped line answers
  `verified: false`; an instruction breakpoint round-trips.
- [ ] **Step 2:** implement; green; gates + commit:
  `feat(post-machine): pmt dap stepping granularities and breakpoints`

---

### Task 8: PM state — stack, variables, setVariable, disassemble, trace

**Files:**
- Modify: `crates/post-machine/src/dap/mod.rs`
- Test: extend `crates/post-machine/tests/dap_programs.rs`

**Interfaces:** `stackTrace` (frames from `ip()` + `session.stack()`
resolved via `LineIndex`, each with `instructionPointerReference` hex),
`scopes` (Registers: IP hex, MF, read-only steps/tacts; Tapes: head±8
window, `» [pos] glyph` head marker, glyphs via the tape block's
alphabet else raw indices), `variables`, `setVariable` (cells via
`Tape::poke` — strict fault → failed set with fault text; unknown glyph
→ failed set naming legal glyphs; MF via `set_mf`; IP/steps rejected),
`disassemble` (render via the same `listing_line` path `dis` uses,
sidecar label resolution), and the `"trace": true` per-instruction
output events reusing the run/trace renderer.

- [ ] **Step 1 (RED):** payload-shape tests: stack frame names/lines
  against the fixture's known map; the tape window marks the head and
  renders glyphs; setVariable on a cell is visible on re-read and on
  the tape after termination; strict-tape launch (StrictTape fixture)
  fails a same-value poke with the fault text; setVariable on IP fails;
  MF set flips a following `check`'s arm; disassemble returns the
  `listing_line` text for the requested range and the top frame's
  `instructionPointerReference` resolves within it; with trace on, the
  output-event count equals the step count.
- [ ] **Step 2:** implement; green; gates + commit:
  `feat(post-machine): pmt dap state surface - stack, tapes, setVariable, disassemble, trace`

---

### Task 9: PM target-mode launch

**Files:**
- Modify: `crates/post-machine/src/cli/driver.rs` (expose a
  `pub(crate)` build seam callable with (project path override,
  target name, force `-g`) returning the built executable + map +
  rendered diagnostics — carve it out of the existing manifest-mode
  path without changing `pmt build`'s behavior),
  `crates/post-machine/src/dap/mod.rs` (target-mode `launch`)
- Test: extend `crates/post-machine/tests/dap_programs.rs`

- [ ] **Step 1 (RED):** manifest-fixture tests (pid+counter scratch): a
  target launch builds with `-g` (breakpoints verify even though the
  manifest profile didn't ask for `-g`), warnings surface as `stderr`
  output events, the target's run-settings tape is loaded; a bad
  target name fails the launch with the driver's error text; existing
  `build_driver.rs` suite still green (the seam refactor changed no
  behavior).
- [ ] **Step 2:** implement; green; gates + commit:
  `feat(post-machine): pmt dap target-mode launch through the build driver`

---

### Task 10: TM adapter — full program-mode surface

**Files:**
- Create: `crates/turing-machine/src/dap/mod.rs`
- Test: `crates/turing-machine/tests/dap_programs.rs`

**Interfaces:** `TmDapAdapter` — the whole Task 6+7+8 surface with the
TM specifics: multi-tape windows (`tape 0..n`), Registers = IP, MR
(writable via `set_mr`), FR read-only and present only on a
frames-profile image, launch REQUIRES a tape block (no empty default —
launch-time error), `.tmc` source breakpoints via the remapped line
table, `.tma` fixtures debug at assembly-line level (the spec §4
bonus), disassemble via the TM syntax's `listing_line`.

- [ ] **Step 1 (RED):** mirror the PM test structure over two fixtures:
  a compiled `.tmc` (source-line stepping, `.tmc` breakpoints, MR
  set/read, multi-tape windows + poke on tape 1) and a hand-written
  `.tma` (assembly-line breakpoints); a frames-profile fixture shows
  FR and rejects writing it; a tape-less launch errors cleanly.
- [ ] **Step 2:** implement; green; gates + commit:
  `feat(turing-machine): tmt dap adapter - full program-mode surface`

---

### Task 11: TM cli wiring + target-mode launch

**Files:**
- Create: `crates/turing-machine/src/cli/dap.rs`
- Modify: `crates/turing-machine/src/cli/mod.rs`,
  `crates/turing-machine/src/completions/` registry (+ its
  `EXPECTED_TOP_LEVEL`-equivalent drift-guard mirror),
  `crates/turing-machine/src/cli/driver.rs` (the same `pub(crate)`
  build seam as Task 9), `crates/turing-machine/src/dap/mod.rs`
- Test: extend `crates/turing-machine/tests/dap_programs.rs`; the TM
  completions drift guard

- [ ] **Step 1 (RED):** target-mode tests mirroring Task 9 (forced
  `-g`, diagnostics as output, tmt.json run-settings tape — incl. the
  tape-only run-block rule: a target without one cannot launch);
  completions guard green with the `dap` entry; `tmt dis`-parity: the
  cli help table lists dap.
- [ ] **Step 2:** implement; green; gates + commit:
  `feat(turing-machine): tmt dap cli wiring and target-mode launch`

---

### Task 12: VS Code packaging (both extensions)

**Files:**
- Modify: `editors/vscode-pm/package.json`, `editors/vscode-pm/src/extension.ts`,
  `editors/vscode-pm/README.md`, and the `-tm` triplet

- [ ] **Step 1:** add the `debuggers` contribution (types `"pmt"` /
  `"tmt"`; `configurationAttributes` for `target` | `program`+`tape`,
  `trace`, `stopOnEntry`; `initialConfigurations` with a target-mode
  sample) and a `DebugAdapterDescriptorFactory` in each `extension.ts`
  launching the resolved binary with `["dap"]` — reuse each
  extension's existing binary-resolution code path (read
  `extension.ts` first; the LSP client already resolves it).
- [ ] **Step 2:** `cd editors/vscode-pm && npm run package` builds; the
  `-tm` twin builds; README gains a "Debugging" section with the
  launch.json sample and the sideload checklist gains a debug
  walkthrough entry (unticked — live verification is the maintainer's
  step).
- [ ] **Step 3:** commit:
  `feat(editors): debugger contributions launching pmt dap / tmt dap`

---

### Task 13: docs and final gates

**Files:**
- Create: `docs/dap.md`
- Modify: `docs/pmt/cli.md`, `docs/tmt/cli.md` (the `dap` subcommand
  line each), `README.md` (one line in the surface list if the CLI
  subcommand lists are enumerated there — check first)

- [ ] **Step 1:** write `docs/dap.md` as `docs/lsp.md`'s sibling:
  launch-config schema (both modes, both toolchains), the closed
  output-events list, the writable-state contract (poke edge behaviors
  verbatim from the spec), granularity semantics, degradation rules,
  the Disassembly view, exit codes. Forge-agnostic prose, no tracker
  refs. Verify every claim against the shipped behavior by running the
  scripted tests' fixtures where a claim is quotable.
- [ ] **Step 2:** sweep the round's code comments for
  `docs/dap.md (…)` citations now resolvable — add where Task 6–11
  carried prose-only.
- [ ] **Step 3:** final gates: `cargo test --workspace`,
  `cargo clippy --workspace --all-targets -- -D warnings`,
  `cargo fmt --check`, both extension `npm run package` builds.
- [ ] **Step 4:** commit: `docs: dap reference page and cli entries`

---

## Self-Review Notes

- Spec coverage: §3→T1–3; §5 envelope/loop/outputs→T2–3/T6/T8; §6
  state+writes→T4/T5/T8/T10; §4 launch→T6/T9/T10/T11; §7 errors→T3
  (lifecycle) + T6/T9–11 (launch); §8→T6/T11/T13; §9→T12; §10 tests
  distributed per task; §11 delivery = this plan's branch/worktree.
- Names consistent: `DebugAdapter`/`AdapterEvent`/`RunState`/`tick`
  (T3, consumed T6/T10), `LineIndex` (T5 → T7/T8/T10), `poke`/`set_mf`/
  `set_mr`/`_tapes` siblings (T4 → T7/T8/T10), `dap_spec` (T6/T11).
- Executor look-ups (anchored): the exact LSP test file names (T1), the
  PM/TM trace renderer seams (T8/T10 — `cli/run.rs`'s traced path each
  side), each extension's binary-resolution function (T12), whether
  README enumerates subcommands (T13).
