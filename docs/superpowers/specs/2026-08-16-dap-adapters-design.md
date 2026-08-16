# DAP debug adapters (`pmt dap` / `tmt dap`) — design

Date: 2026-08-16. Status: approved design.
Driving issue: [#5](https://github.com/mellonis/machine-toolchains/issues/5)
(description refreshed 2026-08-16). Adjacent: [#29](https://github.com/mellonis/machine-toolchains/issues/29)
(JetBrains packaging — out of scope here, pointed at from §9).

## 1. Context and goals

A Debug Adapter Protocol server over the core `vm::DebugSession`, serving
both toolchains: `pmt dap` and `tmt dap` on stdio. The flagship demo:
F5 on a `.pmc` or `.tmc` project in VS Code, stepping at source level
with the tapes rendered in the variables view.

The enabling facts, all shipped: `DebugSession` already has address
breakpoints, depth-aware `step_over`/`step_out`, `run_steps(budget)`,
and a `PauseCause` enum that maps 1:1 onto DAP stopped reasons; the
`-g` line maps already live in the map sidecars
(`MapFunction.lines: Vec<(code offset, source line)>`); the project
manifest gives `launch` a build-and-run contract; the VS Code extension
pair is the packaging vehicle.

## 2. Scope

**One arc, both toolchains** (ruled): the core framework, `pmt dap`,
`tmt dap`, and both VS Code packagings, phased inside one implementation
plan (framework → PM adapter → TM adapter → packaging).

**v1 exclusions, capability-gated off in `initialize`:** `evaluate`,
watch expressions, conditional/hit-count/logpoint breakpoints, data and
function breakpoints, `attach` (the VM has no external process),
variable paging beyond the fixed window, `restart`, `IP`/`FR` writes
(§6). A stated v1 simplification: TM stack-frame names are the
containing function's — enriching composite/binding-call frames with the
`dis` frames-legend naming is deferred.

## 3. Architecture (ruled: the sibling-framework approach)

A new `core/src/dap/` module beside `core/src/lsp/`, following the same
discipline: core owns the framework, arch crates plug in, zero PM-1/TM-1
knowledge in core, fake-adapter tested.

- **Shared transport:** the `Content-Length` framing codec is extracted
  from the LSP transport into one shared module both frameworks consume
  (exact home chosen at implementation for the smallest LSP diff). The
  extraction is behavior-preserving; the existing LSP transport tests —
  including the 64 MiB pre-allocation cap — move with it.
- **Envelope:** typed serde structs for the DAP base protocol
  (`seq`-numbered request/response/event) with typed bodies for exactly
  the v1 commands (§5); everything else answers a uniform DAP-conformant
  "unsupported command" error.
- **`DebugAdapter` trait:** mirrors `LanguageService` — core's server
  loop owns framing, seq bookkeeping, and dispatch; the per-arch
  implementor owns launch, session state, and request semantics.
- **Additive-core-API principle:** capabilities the adapter needs that
  the VM should own land as additive core API (`Tape::poke`, the
  `_tapes` stepping siblings, flag setters — §6), never as adapter-side
  emulation.

**Why not a unified `rpc` layer (the reasoning, on record).** Between
LSP and DAP the genuinely identical code is the framing and the
framed-JSON read/write plumbing — and this design already shares exactly
that. Everything above it diverges in kind: different envelopes
(JSON-RPC vs `seq`/`type`/`command`), different loop shapes (synchronous
request→response with per-URI multi-service routing vs a single stateful
session emitting unsolicited events around a sliced run loop with a
reader thread), different lifecycles. A unified server-loop abstraction
would serve two consumers with divergent needs, bought by refactoring
the shipped, editor-verified LSP loop — regression risk spent
deduplicating boilerplate-shaped, not logic-shaped, code. The rule of
three is not met; no third framed protocol is on any roadmap. If the
loops turn out closer than designed, the implementer is licensed to
hoist more into the shared module — an additive, core-internal move
needing no design change. The accepted cost of the sibling approach:
DAP hardens its own lifecycle state machine (its analog of the LSP
hardening round), listed in §10's tests.

## 4. Launch contract (ruled: both modes, target-primary)

One internal launch struct, two config shapes:

- **Target mode** — `"target": "<name>"` (+ optional `"project"` path
  override): the adapter runs the equivalent of `build` for the target
  **in-process** (never shelling out), **always injecting `-g`**
  regardless of profile — a debug session without line maps is crippled.
  Build diagnostics stream as `output` events (§5); a failed build fails
  the `launch` request. The tape block comes from the target's run
  settings.
- **Program mode** — `"program": "path/to/app.pmx"` (+ `"tape"`):
  prebuilt artifacts used as-is. A sidecar without line info degrades
  honestly: breakpoints answer `verified: false` with a
  "build with -g" message; stepping falls back to instruction
  granularity. A hand-assembled program's `-g` line table carries the
  assembly's own lines, so breakpoints and stepping in the `.pma`/
  `.tma` file itself work for free — the compiled path's source remap
  (`remap_debug_lines`) is what redirects them to `.pmc`/`.tmc` lines.
- **Tape resolution** reuses the `run`/`build --run` rules, shared, not
  reimplemented: TM requires a tape block (no empty-tape default — a TM
  launch without one is a clean launch-time error); PM defaults to the
  empty tape.
- **Common options:** `"stopOnEntry": bool` (default false — pause at
  the entry instruction before the first step), `"trace": bool`
  (default false — §5's opt-in stream).

## 5. Protocol surface and the run loop

**v1 commands:** `initialize`, `launch`, `setBreakpoints`,
`setInstructionBreakpoints`, `configurationDone`, `threads`,
`stackTrace`, `scopes`, `variables`, `setVariable`, `continue`, `next`,
`stepIn`, `stepOut`, `pause`, `disassemble`, `disconnect`.
**v1 events:** `initialized`, `stopped`, `output`, `terminated`,
`exited`. **v1 capabilities declared:** `supportsSetVariable`,
`supportsSteppingGranularity`, `supportsDisassembleRequest`,
`supportsInstructionBreakpoints` (everything else false).

**Stepping granularity.** `DebugSession` steps instructions; DAP's
default `next`/`stepIn` means "advance one source line". Line
granularity therefore repeats the underlying session step until the
address's mapped source line changes — honestly interrupted by any
breakpoint, `brk`, or trap encountered on the way (that pause wins and
reports its own reason). `granularity: "instruction"` — which VS Code
sends automatically when its Disassembly view is focused — maps to a
single session step. `stepOut` is depth-based either way. A `-g`-less
build is instruction-granularity throughout.

**The Disassembly view.** `disassemble` renders instruction ranges via
the same core `listing_line` renderer `dis` and `run --trace` already
use, with sidecar label resolution; stack frames carry
`instructionPointerReference`, so the client's Disassembly view
highlights the current instruction and tracks every step.
`setInstructionBreakpoints` maps the view's breakpoint gutter directly
onto the session's address breakpoints.

**The run loop — one extra thread, total.** A reader thread owns stdin,
decoding framed messages onto an `std::sync::mpsc` channel (no new
dependencies). The main loop owns dispatch and stdout. While running,
it alternates `session.run_steps_tapes(devices, BUDGET)` with
non-blocking `try_recv` polls; `BUDGET` is a documented constant
(~10,000 steps — sub-millisecond slices, instant-feeling `pause` and
`setBreakpoints`). A budget-exhaustion `PauseCause::Manual` is invisible
to the client unless a request arrived. The CLI's 10M step cap
deliberately does not apply — an interactive session has a human with a
pause button.

**Pause mapping:** `Breakpoint(addr)` → `stopped("breakpoint")`;
`Brk` → `stopped("breakpoint")` with a `description` naming the
source-authored `debugger` statement; `Step` → `stopped("step")`;
client `pause` → `stopped("pause")`; the `stopOnEntry` pause →
`stopped("entry")`; `Trap(kind)` → `stopped("exception")` carrying the
trap-kind text.
`Finished(outcome)` → `terminated` + `exited` with the CLI's exit-code
mapping (0 stopped / 2 halted / 3 trapped).

**Output events — the closed list.** Exactly three kinds, nothing else
(no `stdout` category ever — the machine has no output channel; the
tape is the result; no `telemetry`; pause annotations ride
`stopped.description`):

1. Build diagnostics during a target-mode launch (`stderr` category),
   one event per diagnostic, the CLI's rendered text.
2. One termination summary (`console`) before `exited`: outcome, steps,
   core/stall tacts — the numbers `run` prints.
3. Opt-in trace lines (`console`) under `"trace": true` — one event per
   retired instruction, byte-identical to `run --trace`'s lines (the
   `drive_traced` renderer reused, not reinvented).

Any future feature that wants to print must argue its way into this
list explicitly.

## 6. State mapping: stack, scopes, variables

- **Threads:** exactly one, `{id: 1, name: "machine"}`.
- **Stack:** top frame = `ip()`; older frames from `session.stack()`.
  Addresses resolve through the sidecar: containing `MapFunction` →
  frame name; its `lines` table (largest mapped offset ≤ address) →
  source line, with `source` + `line` populated so the editor
  highlights. No sidecar/`-g`: hex-address frame names, no source —
  still steppable at instruction granularity. The line↔address helper
  is a small arch-agnostic utility over `MapFile`, living beside it in
  core.
- **Scopes** (identical for any selected frame — machine state is
  global): **Registers** — PM `IP` (hex), `MF`; TM `IP`, `MR`, and `FR`
  only on a frames-profile image; both sides append read-only `steps`,
  `core tacts`, `stall tacts`. **Tapes** — one child per tape
  (`tape 0`, `tape 1`, …; names are deliberately absent from images),
  each a fixed window of head±8 cells rendered `[pos] glyph` (glyphs
  from the launch tape block's alphabet where known, raw indices
  otherwise), head cell marked (`» [3] '1'`).
- **Writable state** (ruled): `supportsSetVariable: true`, sets only
  while stopped.
  - **Tape cells** via additive core `Tape::poke(pos, symbol)` — a
    trait method with a default implementation (save head, walk to
    `pos`, `write`, walk back; device-level moves, no tact accounting),
    overridable by devices with cheap random access. Stated edge
    behaviors: a `StrictTape` fault on the poke surfaces as a *failed*
    `setVariable` carrying the fault text; poking the cell under the
    head does **not** re-latch MF (the flag stays as the program
    latched it — a hardware probe on the tape). The client sends a
    glyph; an unknown glyph is a failed set naming the legal glyphs.
  - **Flag registers**: `MF` (PM) and `MR` (TM) via additive
    `DebugSession` setters. **`IP` and `FR` stay read-only** —
    overwriting them can desynchronize the return stack and the frames
    discipline; deferred until wanted.
- **Additive stepping API:** `continue_`/`run_steps`/`step_over`/
  `step_out` gain `_tapes` siblings (mirroring
  `step_in`/`step_in_tapes`) so TM's multi-device sessions get the full
  control set.

## 7. Error handling

- Malformed frames / oversize payloads: the shared codec's existing
  behavior (bounded rejection), session survives where the LSP
  transport would survive.
- Requests violating the lifecycle (e.g. `stackTrace` before `launch`,
  anything after `disconnect`): uniform DAP error responses — the DAP
  analog of the LSP post-shutdown guard, tested as such.
- Launch failures (bad target name, missing tape, build errors): failed
  `launch` response with a message, diagnostics as §5 output events,
  followed by `terminated`.
- Trap during run: never an adapter error — it is the
  `stopped("exception")` path with state inspectable, then
  `Finished(Trapped)` on further stepping.

## 8. CLI and registries

`dap` joins both CLIs as a subcommand (`cli/dap.rs`, the only place
real stdio is handed to the core server loop — the `lsp` precedent
verbatim). Both completions registries gain the entry;
`EXPECTED_TOP_LEVEL` mirrors update; `docs/pmt/cli.md` /
`docs/tmt/cli.md` gain the subcommand line, and a new shared
`docs/dap.md` (sibling of `docs/lsp.md`) carries the reference:
launch-config schema, the closed output list, the writable-state
contract, and the degradation rules.

## 9. VS Code packaging

Both extensions gain a `debuggers` contribution — types `"pmt"` and
`"tmt"` — with `configurationAttributes` for exactly §4's surface,
sample `launch.json` snippets in each README, and a
`DebugAdapterDescriptorFactory` resolving the tool binary the same way
each extension's LSP client already does (path setting, else `PATH`)
and launching `pmt dap` / `tmt dap` on stdio. Extension versions and
`MIN_TESTED_*` floors move at the arc's release cut; the sideload
checklists gain a debug-session walkthrough that ships unticked (live
verification stays the maintainer's step). The JetBrains pair is
untouched — its DAP client is #29's round, and lives or dies with
LSP4IJ/platform support there.

## 10. Testing

- **Core framework:** fake-adapter tests mirroring the LSP fake-service
  suite — framing round-trips over in-memory pipes (no real stdio),
  seq bookkeeping, unsupported-command responses, event ordering,
  reader-thread channel draining, the lifecycle-guard errors (§7). The
  moved framing codec keeps the existing LSP transport tests, cap
  included.
- **Per-arch adapters:** in-process scripted-conversation tests over
  tiny prebuilt `.pmx`/`.tmx` + tape fixtures: every `stopped` reason;
  stack/scopes/variables payload shapes; `setVariable` round-trips
  (poke visible on re-read, the `StrictTape` failed-set path,
  unknown-glyph rejection, MF/MR set + `IP` rejected);
  line-breakpoint verification incl. `-g`-less degradation;
  step in/over/out against a known call shape; line-vs-instruction
  granularity (a line step collapses several instructions; an
  instruction step advances exactly one; a breakpoint mid-line-step
  wins and reports its own reason); `disassemble` payloads matching
  `listing_line` output with the frame's
  `instructionPointerReference` resolvable in them;
  `setInstructionBreakpoints` round-trip; `stopOnEntry`; the
  trace stream; termination summary + exit codes across
  `stp`/`hlt`/trap programs.
- **Target-mode launch:** a manifest-fixture test proving the injected
  `-g`, diagnostics-as-output, and run-settings tape resolution —
  scratch dirs per the pid+counter isolation convention.
- **`Tape::poke`:** core unit tests — head restored, default impl vs an
  override, strict-fault surfacing, MF-not-re-latched pinned.
- **No external DAP conformance harness** — the deps-minimal rule
  stands; scripted conversations are the conformance evidence. Live
  editor verification is the maintainer's post-merge checklist step.

## 11. Delivery

The arc runs on `feat/dap-adapters` in its own worktree
(`../toolchains-dap`), in parallel with the optimizer round; the shared
surface between the two branches is limited to registration tables and
top-level docs, so whichever lands second rebases trivially. Ships on
master; CHANGELOG and version bumps ride the next release cut, whose
version block declares the extension floors' moves. #5 closes at the
arc's merge; the deferred items (JetBrains client → #29, TM composite
frame naming, `IP`/`FR` writes, conditional breakpoints) are recorded
there.
