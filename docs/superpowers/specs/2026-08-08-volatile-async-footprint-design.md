# Volatile tapes, async bus, write footprints — design

**Date:** 2026-08-08
**Status:** approved (maintainer rulings recorded inline)
**Driving issues:** [#74](https://github.com/mellonis/machine-toolchains/issues/74) (volatile / device tapes — the semantic half), [#7](https://github.com/mellonis/machine-toolchains/issues/7) (async bus — the transport half), [#53](https://github.com/mellonis/machine-toolchains/issues/53) (per-graph/routine write footprints).
**Sequencing:** this round lands BEFORE the v0.3.0 cut (#8), per the 2026-08-08 ruling — the unreleased TM version spaces absorb the changes without number churn.

One spec, five plans (§9). The three issues are one story: a tape may be a
device (#74 semantics), a device needs a transport the machine can really
wait on (#7), and "who may clobber what" across a call is the footprint
question (#53). Designing them together produced one shared vocabulary —
observability — and several rulings that cut across all three.

---

## 1. Goals and non-goals

**Goals**

- A non-blocking, embedder-pumped execution surface in `mtc-core` that a
  wasm host, a microcontroller firmware, or a native stand can drive, with
  real WAIT/READY device semantics. The sync surface stays untouched.
- `volatile` as a per-tape property in `.tmc` (machine tapes and signature
  tape parameters) and as a program property in `.pmc` (`volatile main()`),
  gating every optimizer assumption that is unsound for a device band.
- PM object multiversioning: every `.pmo` carries a normal and a volatile
  build of each function; the linker picks the column by `main`'s marker.
- Inferred per-tape **write-sets** for TM worlds and graphs, a
  `dead-map-pair` lint, footprint visibility (hover, `tmt ir`), and
  declared write contracts (`writes` / `preserves` clauses) checked
  against the inference.

**Non-goals (deliberate, with reasons)**

- **No interrupts.** The machine model stays sequential; asynchronous
  external events enter through a volatile band (the external world
  mutates cells; the program observes by reading). Preemptive interrupts
  would be an ISA revision — out of scope, path known. Manual pause/stop
  from outside is session API in software and clock gating (RUN/HALT) in
  hardware — neither touches the ISA.
- **No `BusResponse` extension.** The core's settle arms treat an
  unexpected response variant as a driver-protocol violation (16 catch-all
  arms trap it silently). Waiting is resolved in the session, outside the
  core; the core never sees a "not ready" response.
- **No container volatility bits.** Per the C-style ruling (§4.4) volatile
  is a compilation directive; MX carries no per-tape flag, and `tmt run` /
  embedders learn device placement out of band. Revisit only if a runtime
  consumer appears.
- **No read-sets.** Every consumer this round needs writes only; a read
  notion ("symbols distinguished by patterns") returns together with
  liveness when a dead-write pass wants it. The `unused-tape-symbol` lint
  idea is dropped with it.
- **No footprint/contract export to MO.** `tmt compile` never reads
  objects; the only consumers this round are compile-time. Export becomes
  a follow-up when a link-time consumer exists.
- **No `DeviceTimeout` trap.** "The device never became ready" is the
  embedder's judgement (stop pumping, drop the session); the sans-I/O core
  has no clock to judge it by.
- **No `ReadAll` overlap.** The N per-instruction device reads serialize
  in device order, exactly as the sync driver prices them. Overlapping
  them under WAIT/READY is a possible future optimization, noted here so
  the serialized order is understood to be a choice.

---

## 2. Shared semantic base: observability

The optimizer equivalence contract already has one observability barrier:
an un-stripped `brk`, which no motion crosses. A **volatile band
generalizes that barrier from a point to a standing, per-band rule**:

> Every access to a volatile band is externally observable, and the
> external world may change the band's cells between accesses. No pass may
> assume a value read from or written to a volatile band persists, and no
> pass may change the band's access sequence — no dropping idempotent or
> dead writes, no fusing or splitting write+move shapes, no value
> propagation through its reads. Each read is a fresh observation.

This paragraph (adapted per side) lands in `optimizer/mod.rs` of both
toolchains and in both `docs/{pmt,tmt}/optimizer.md`, next to the existing
`brk` barrier — so passes born later (the #53-adjacent dataflow work)
start life already constrained.

**Volatility vs. footprint.** These are orthogonal axes and the docs must
say so: volatility is about **bus activity** (the machine physically
touches every tape every step; what matters is which accesses are
observable), a footprint is about **value dependence** (which symbols a
world can write). Future passes consult both.

---

## 3. Async transport (#7)

### 3.1 Consumers and shape (ruling)

Target consumers: **wasm embedding (#6), microcontroller firmware hosting
the VM, and FPGA-class hardware** — all three tiers, per the maintainer.
The browser cannot block and bare-metal firmware has no threads, so the
surface is **poll-shaped and non-blocking**; the sync surface stays as-is
(additive, mirroring the turing-machine-js v7.1 ruling). For the hardware
tier the Rust core is the **golden model**: `Core` is already a per-tact
state machine (`BusRequest` → `BusResponse` with an explicit `Pending`
continuation), i.e. an executable spec of the wait-state behavior the
silicon implements; co-simulation (same image in the Rust core and in
hardware, compare traces) is the long-term verification story.

The clock correspondence: in hardware the clock generator pumps the
processor; in software the embedder's `pump()` calls play the role of
clock edges. Same state machine, different clock source.

### 3.2 Device trait

A poll-shaped mirror of the bus protocol's device requests:

```rust
pub enum DeviceCmd   { MoveLeft, MoveRight, Read, Write { index: u32 } }
pub enum DeviceReply { Ok, Symbol(u32), Fault(DeviceFault) }
pub enum DevicePoll  { Pending, Ready { reply: DeviceReply, cost: Option<u32> } }

pub trait AsyncTapeDevice {
    fn alphabet_size(&self) -> u32;
    fn head(&self) -> i64;
    fn issue(&mut self, cmd: DeviceCmd);
    fn poll(&mut self) -> DevicePoll;
}
```

- **One command in flight per device.** `issue` while a command is pending
  is a contract violation (documented; debug-assertable). The WAIT/READY
  reading: `issue` puts the command on the bus; each `poll` samples the
  READY line on a clock edge — `Pending` is READY low, `Ready` is READY
  high with the data latched.
- **Cost reporting.** `cost: None` means "price me at the `TactProfile`
  model cost"; `Some(n)` is the device's own measurement in tacts (a real
  device's wait is real machine time — in hardware the counter ticks
  during a WAIT stall). The whole transaction's cost arrives as one number
  in `Ready`; **`Pending` polls never tick the tact counter** — otherwise
  stats would depend on the embedder's pump cadence and lose determinism.
- **Blanket adapter.** `SyncAsAsync<T: Tape>` wraps any existing tape as
  an always-ready device with `cost: None`. This is what makes the
  sync ≡ async equivalence gate (§3.5) bit-exact for free.
- **`LatencyTape<T: Tape>` — the shipped async device.** A decorator in
  `devices/` (the `StrictTape` pattern applied to time): `issue` latches
  the command, `poll` stays `Pending` for a configurable per-operation
  number of polls, then performs the operation on the inner tape and
  answers `Ready` with a configurable per-operation `cost: Some(tacts)`.
  It is three things at once: the **reference implementation** of the
  device contract for stand authors (one in-flight command, READY
  discipline, cost reporting — shown as working code, not just prose),
  the **test vehicle** for §3.5 (no throwaway fake), and the transport
  counterpart of the `TactProfile` mechanical profile (the profile models
  the price; `LatencyTape` models the wait itself). `no_std`-clean by
  construction — latency is counted in polls, no timers.
- Real I/O transports (serial, GPIO) stay OUT of core — it is sans-I/O;
  such drivers are embedder code implementing this trait. No CLI exposure
  this round (`run` stays sync; a `run --async` demo waits for a real
  consumer, likely the wasm arc).
- Object safety: the trait is `dyn`-usable (no `async fn`), so device
  slices work exactly like `&mut [&mut dyn Tape]` does today.

### 3.3 The session

A fourth resident of `vm/`, beside `driver`/`DebugSession`, touching
neither:

```rust
let mut s = machine.async_session(opts);   // Core + ReturnStack + RunStats + breakpoints
match s.pump(&mut devices, budget) {
    PumpEvent::DeviceWait      => { /* a device is Pending — come back later */ }
    PumpEvent::BudgetSpent     => { /* `budget` instructions retired this call */ }
    PumpEvent::Paused(cause)   => { /* breakpoint / brk / manual pause */ }
    PumpEvent::Finished(result)=> { /* RunResult, same type the sync driver returns */ }
}
```

- One `pump` call retires instructions until a device polls `Pending`, the
  per-call `budget` is spent (wasm frame pacing; `budget = 1` is
  single-step), a pause condition fires, or the program terminates. The
  in-flight instruction lives in the core's existing `Pending`
  continuation — **the core does not change**.
- The session serves code/stack/table requests itself (they are always
  ready); only device requests go through the poll trait.
- Debug controls are built in from birth: breakpoints, external `pause()`
  (takes effect at the next instruction boundary, `Paused(Manual)`),
  `stop()` (finalizes with a result), and the register/stats accessors
  `DebugSession` has. Multi-tape from birth. This is the surface #5 (DAP)
  will later drive for embedded targets. The sync `DebugSession` is not
  modified.
- The initial-MF latch on the async path becomes a real device-0
  transaction (a slow device makes loading genuinely wait) but stays
  unaccounted, per the loading model; whether it happens at all mirrors
  the sync entry points' rule (single-tape v1-shape images latch,
  multi-tape v2 images don't).
- Limits: `RunLimits` enforced as in the sync driver, in model tacts.

### 3.4 `no_std`

Tier 2 (firmware) requires the vm module to build without std:
`mtc-core` gains a default-on `std` feature; `vm/` (core, bus, driver,
devices, debug, session) builds under `no_std` + `alloc` with the feature
off; formats, linker, assembler, LSP remain std-only. A probe confirmed
no serde reaches `vm/` today. CI gains a
`cargo build -p mtc-core --no-default-features` gate.

### 3.5 Tests

- **sync ≡ pump property (the headline gate):** over a corpus of programs
  (reuse the golden/equivalence corpora), `driver::run` and a pumped
  session with `SyncAsAsync` adapters produce bit-identical `RunResult`
  AND `RunStats`.
- **`LatencyTape`** (the shipped device, §3.2): same result, same stats
  (pending polls don't tick; reported costs land in `stall_tacts`), and
  `pump` reports `DeviceWait` while waiting.
- Budget chunking; breakpoint/manual pause through `pump`; `no_std` build.

---

## 4. Volatile tapes (#74)

### 4.1 `.tmc` surface (rulings: declaration sites; reservation)

`volatile` is a **reserved word** (RESERVED grows 24 → 25). Rationale:
every other declaration modifier (`export`, `local`, …) is reserved; the
one contextual word, `deprecated`, lives in the `![attr]` micro-language,
not the declaration grammar. A contextual `volatile` would also force
context-blind tools (TextMate) to lie about valid code. `.tmc` is
unreleased, so there is no compatibility cost. The plan's first step
verifies the word is unused in `std.tmc`, fixtures, and `docs/examples/`
(probe: it is).

Two declaration positions, per the separate-compilation ruling (a routine
compiled into an object cannot know its future caller's tapes — so the
routine's own signature must be able to say it):

```
machine {
  volatile tape sensor: readings;    // this band is a device
  tape scratch: bits;
}
export routine probe(volatile tape s: readings)   // compiled for a volatile band
```

**Semantics** (`docs/tmt/language.md` wording): a volatile tape is a
device band — §2's observability rule verbatim, plus the graft/call
asymmetry:

- **Grafts dissolve before optimization**, so spliced rows live on the
  host's tapes and the **host tape's volatility governs** — which is
  always correct, because the host's declaration describes the band the
  code will actually run on (grafting a "sensor" graph onto a memory tape
  means it runs on memory; optimizing it is sound).
- **Calls/binds are separate-compilation boundaries** and follow the
  C-style ruling (§4.4): no cross-unit checking; the author calls the
  routine variant written for the right kind of band. `language.md`
  carries an honest foot-gun paragraph.

### 4.2 IR and optimizer

- `IrTape` gains `volatile: bool` (serde default, omitted when false). A
  signature parameter's marker becomes the flag on the routine world's
  tape — one field covers both declaration positions. `TM_IR_VERSION`
  **stays 2**: the shape is amended in place, unreleased (§8).
- Today **no TM pass needs gating**: all eight preserve per-band access
  sequences (`dead_rows` removes only rows that never fire, so the dynamic
  sequence is unchanged — argued explicitly on the optimizer page; the
  per-rule `wrmv` fusion is language semantics, not an optimization).
  The real changes are **flag propagation**: `inline` splices onto the
  caller's tapes (flag already the caller's — verified, not assumed);
  `outline` synthesizes worlds that mirror the host's tapes and must copy
  the flag.
- The §2 contract paragraph lands in `optimizer/mod.rs` and
  `docs/tmt/optimizer.md`.

### 4.3 `.pmc` surface: `volatile main()` (rulings: main-only; reservation)

PM-1 is single-tape, so volatility is a **program** property declared in
the one place the program has a name:

```
volatile main() { ... }
volatile export main() { ... }    // fixed order: volatile first
```

On any other function: error `volatile-not-on-main`, naming the rule.
`volatile` is likewise reserved as an identifier (a function or namespace
named `volatile` becomes an error) — `PMC_LANG_VERSION` 0.3 → 0.4 (the
one released space this round moves; accepted 2026-08-08).

A per-function `volatile` was considered twice and rejected twice: first
as a barrier-region model (#74's original note), then as a library-author
marker — the latter made unnecessary by multiversioning (§4.5), which
solves "the library is already optimized and the user can't control it"
mechanically, without grammar. The asymmetry with TM is principled: TM
volatility is per tape parameter (2^n variants would explode; the author
declares), PM has one tape (exactly 2 variants; the toolchain builds
both).

### 4.4 The C-style ruling (link boundary)

Maintainer ruling: **volatile affects compilation; the link boundary is
not checked.** No link-time volatile errors, and objects carry no
compilation *facts* for the linker to verify (the rejected alternative —
a per-world "value-assumptions applied" taint bit checked at link — was
declined as long-haul plumbing of a flag through the link for little gain
over the two-variant scheme). On TM the author writes (or picks) the
routine variant for the band kind; on PM the toolchain's multiversioning
makes the choice automatic. Note the distinction from §4.5: the MO
variant tags there are *selection metadata* (which body is which), not
checked facts — a mismatch degrades to a counted fallback, never an
error.

### 4.5 PM multiversioning (ruling: in this round)

Every `.pmc` compilation emits **two builds of every function**: *normal*
(today's pipeline) and *volatile* (the gated pipeline). Mechanics:

- **Gated pipeline** = the optimizer run with every pass that consumes
  write-read-back identity or reorders the tape access sequence disabled.
  Certain members: `cell-state`, `fuse-tape-ops`. The plan probes whether
  any other pass consumes cell/MF *predictions* (e.g. `branch-fold`
  consuming `cell-state`-derived facts — note MF as a *register* latched
  by a performed access stays sound; predicting MF from a written value
  assumes write-read-back and does not) and pins every gated/not-gated
  decision with a test. Implementation reuses the existing
  `disabled_passes` mechanism — PM IR does not change.
- **Dedup by digest:** functions whose two bodies come out byte-identical
  (any function that doesn't touch the tape; every function at `-O0`)
  collapse to one blob tagged *both*. Otherwise blobs are tagged
  *normal* / *volatile*. MO v3 (unreleased) gains the per-blob variant
  tag; a legacy object without tags reads as *normal-only*. The mechanism
  is name mangling's job done in a typed field: like a mangled overload,
  the two variants are distinct link-time entities sharing one source
  name — but since MO is our own format, the discriminator rides a record
  field instead of a name convention, keeping symbol names clean in
  `dis`/map/`LinkReport` and making the legacy fallback a typed rule
  rather than a string match.
- **Linker choice:** the object defining `main` records the program's
  volatile bit (a free MO v3 flags bit); symbol resolution prefers the
  matching column. A library
  that only has a single body for a routine (legacy object) still links —
  the linker takes it and counts the fallback in `LinkReport` (visible
  under `-v`). Not an error, per §4.4; not silent either.
- **`pmt ir`** gains `--variant normal|volatile` (default `normal`).
- **`pmt build` in-memory mode** compiles only the needed column, in both
  directions (a volatile program skips the normal column, a non-volatile
  one skips the volatile column). The both-columns rule exists for
  artifacts that outlive the invocation — an on-disk `.pmo` cannot know
  its future program; an in-memory object dies inside a link whose bit is
  already known, so the other column is provably dead. The boundary is
  disk: standalone `pmt compile -o` and `pmt build --keep-objects` always
  emit both columns. Gate: the `.pmx` built through the in-memory path is
  byte-identical to the one built through on-disk objects.
- **Byte-identity, re-scoped deliberately:** the `.pmx` of a non-volatile
  program and all `-S` listings stay byte-identical — that is the gate.
  `.pmo` files change shape (they now carry variant records): this is the
  round's one format change, riding unreleased MO v3; older `pmt` cannot
  read the new objects, declared at the cut.

**Text form (ruled 2026-08-09; supersedes the "nothing reaches the
assembler" clause for PM only):** the `.pma` dialect gains a
presence-form **`.volatile`** directive, riding the already-unreleased
dialect 0.3 amended in place (v0.2.0 released 0.2; the `wrl`/`wrr` round
moved master to 0.3). Inside a `.func` block, `.volatile` tags that blob
as the volatile column; absence = normal; `duplicate-function` becomes
variant-aware — a same-name pair is legal iff exactly one member carries
`.volatile`. Before the first `.func`, `.volatile` sets the object's
program bit. `Both` has no directive: dis prints a deduped function
twice (bare + `.volatile`) and the assembler dedups a byte-identical
same-name pair back to one Both-tagged blob — the compiler's dedup
mirrored. Result: `pmt dis` output of any PM object is assemblable and
byte-round-trips, tags and program bit included; hand-written `.pma` can
author a volatile column (a directive-free file stays a normal-only
legacy object). The directive is PM-dialect-only: `.tma` does not
recognize it — `.func` is a core directive shared by both dialects, but
TM volatility is per tape parameter, not per routine, so a routine-level
tag has no TM meaning.

**Why two variants differ materially** (worked probe, kept for the docs):
an 11-op pulse routine at `-O1` compiles to 4 fused instructions
(`wrr/wrl` — idempotent second `mark` dropped, `mark;unmark;mark` dead-store
run collapsed, write+move fused so the intermediate MF latch read never
happens); the gated build keeps all 8 writes and 3 moves as separate bus
transactions (27 vs 54 tacts, same final tape). For memory the 4-op form
is pure win; for a device those "redundant" transactions — keep-alive
pulses, commands, intermediate sensor samples — are the point of the
program. `docs/pmt/optimizer.md` gets this example.

### 4.6 Strict cells

`docs/pmt/isa.md` gains the honest link: a volatile program cannot lose a
strict-cell fault to `cell-state` by construction, so the current
"build with `--fno-cell-state` when strict-cell faults are the point"
advice gets the simpler alternative.

---

## 5. Write footprints (#53, inference half)

### 5.1 What is computed

Per world, per tape: the **write-set** — the set of symbol indices
appearing in any rule's write cells, taken transitively through calls.
Reads are out of scope (ruling; see non-goals). Two computations, one
shared primitive (set type + union + map projection):

- **IR walk** (`worlds → states → rules`), following `CallThen` **and**
  `TailCall` (the latter appears at `-O1` and is easy to lose;
  `unused_routine_warnings` is the existing walk that gets both right).
  Binding projection: callee tape `k` is `binding[k].caller_tape`; symbols
  map through the binding pairs (`one_way` pairs never write back — their
  write half contributes nothing); an empty binding is identity.
  Recursion: monotone fixpoint over union. An externally-unresolvable
  call target contributes conservatively: the full alphabet on every
  bound tape. Conservatism costs findings, never correctness.
- **Pre-splice computation in `expand.rs`** for graft maps: grafts don't
  survive into IR, and the graft-map lint needs the graph's **own**
  (pre-map) alphabets — the write-set must live in the graph's own frame,
  projected through `TapeMap` per graft site (the #53 "alphabet frame"
  hazard, honored).

Footprints are **derived data**: not serialized into the IR artifact, not
exported to MO (see non-goals).

### 5.2 Consumers this round

- **`dead-map-pair` lint (`.tmc`)** — write-half only, which is the
  provable half: a bidirectional pair `a -> b` whose write-back half can
  never fire because the callee never writes `b`. One-way pairs `a => b`
  have no write half and are never reported. The read direction is
  undecidable at compile time (it depends on the caller's writes and the
  initial tape content), and stays out. Works on grafts and on locally
  resolvable calls/binds; silent on unresolvable targets. **Quickfix:
  demote `a -> b` to `a => b`** (behavior-preserving: the write-back
  never fired) — not deletion, which would also kill the live read half.
- **Visibility** — the motivating scenario (marker preservation) in its
  awareness form: LSP hover on a routine/graph shows the inferred
  per-tape write-set (computed on demand); `tmt ir --footprints` prints a
  footprint report (a separate report, NOT part of the IR JSON).
- **The stdlib's own load-bearing example** becomes checkable: the
  delimited `std::binaryNumbers::invertNumber` calls the bare invert with
  markers collapsed one-way onto the callee's blank, and its doc line
  promises "the markers … survive the call because bare invert never
  writes a blank" — a write-set claim, currently held by prose alone.
  Inference verifies it; a `preserves` contract (§6) pins it.

### 5.3 Tests

The **over-approximation property**: run a corpus, record the actually
written symbols per tape, assert containment in the inferred sets. Units:
binding/map projection, mutual-recursion fixpoint, conservative unknown
target, `TailCall` edges.

---

## 6. Declared write contracts (#53, contract half)

### 6.1 Form (rulings: both clauses; the naming)

Two optional clauses on a signature tape parameter, after the alphabet,
fixed order, canonicalized by fmt:

```
export routine copyDigits(tape src: symbols,
                          tape dst: symbols writes {'0','1'} preserves {'#'})
```

- `writes {…}` — upper bound: the body writes at most these symbols.
- `preserves {…}` — forbidden set: the body never writes these.
- Effective allowed set = (`writes` if present, else the full alphabet)
  minus `preserves`. **One check:** the inferred write-set is contained in
  the effective allowed set; violation is error `writes-outside-contract`
  at the declaration, naming the offending symbols.
- A symbol in both clauses is redundancy, not contradiction (`preserves`
  wins): lint `contract-clause-overlap` with a quickfix removing it from
  `writes`. A symbol outside the tape's alphabet: error
  `contract-symbol-unknown`. Slack in `writes` (writing less than
  declared) is deliberately NOT a lint — headroom is the point of a
  contract.
- Legal on both `graph` and `routine` tape parameters. `writes` and
  `preserves` are reserved words (RESERVED 25 → 27; both probed unused —
  `writes` occurs once in `std.tmc`, in doc-line prose, which is free
  text).

### 6.2 Semantics of the declaration

The contract is an assertion **over** the inference, never a replacement
(the #53 principle). Inside a compilation unit the inference is always
available and at least as precise, so lints and passes consume the
inference; the declaration's value is (a) it **breaks the build** when a
body edit violates the promise — API stability for the stdlib and any
library shipped as source — and (b) it is the only thing that could later
cross a separate-compilation boundary (the MO-export follow-up).
PM is untouched: #53 is TM-only (interprocedural cell tracking is blocked
by head-motion unknowability regardless of write-sets).

### 6.3 Stdlib adoption

The plan adds `preserves` clauses where the stdlib's doc lines already
promise preservation (at minimum: bare `invertNumber`'s blank), keeping
doc prose and contract in sync — the compiled-stdlib byte-identity check
proves contracts are zero-cost at codegen.

---

## 7. Documentation matrix

| Page | Change |
|---|---|
| `docs/core.md` | WAIT/READY handshake, device contract (one in-flight command, cost-in-reply), the pump session beside `run`/`DebugSession`, the clock correspondence + golden-model note, `no_std` surface, timing-model amendment (model tacts vs device-reported cost) |
| `docs/tmt/language.md` | `volatile` on machine tapes and signature params, §2 semantics, graft-vs-call asymmetry + foot-gun, `writes`/`preserves` clauses, reserved-word table (+3) |
| `docs/pmt/language.md` | `volatile main()`, program semantics, the two-variant mechanism, library/legacy-object story |
| `docs/pmt/optimizer.md` | volatile barrier beside `brk`; gated-pass list; the worked two-variant example |
| `docs/tmt/optimizer.md` | volatile barrier beside `brk`; `inline`/`outline` flag propagation; volatility-vs-footprint orthogonality |
| `docs/pmt/isa.md` | strict-cells ↔ volatile link |
| `docs/formats.md` | MO v3 variant records, legacy-object reading |
| `docs/tmt/lint.md` | `dead-map-pair`, `contract-clause-overlap` |
| `docs/lsp.md` | footprint hover |
| `docs/pmt/cli.md`, `docs/tmt/cli.md` | `pmt ir --variant`, `tmt ir --footprints` (byte-exact USAGE quotes — the cli_docs guards pin them) |

---

## 8. Version spaces

Moves: **`PMC_LANG_VERSION` 0.3 → 0.4** (released space; `volatile` as a
reserved word + modifier is a grammar change; accepted 2026-08-08).

Everything else stays, per the pre-cut sequencing ruling: `TMC_LANG_VERSION`
0.1 and `TM_IR_VERSION` 2 (unreleased — shapes amended in place),
MX v2 untouched, MO v3 amended in place (variant tags), MT v2 untouched,
`.pma` 0.3 (unreleased — v0.2.0 shipped 0.2) amended in place with the
`.volatile` directive (ruled 2026-08-09, §4.5 text form; the constant
stays "0.3"), `.tma` 0.3 untouched (TM volatility stays a compile-time
notion; the directive is PM-dialect-only), PM IR untouched
(variants are two optimizer runs over one shape), both project-manifest
schemas untouched. Editor plugins: TM pair unreleased (grammar rides
without a bump); the PM pair's grammar addition rides the already-pending
0.1.3. Compatibility note for the cut's CHANGELOG: new `.pmo` objects are
MO v3 with variant records — readable by this toolchain, not by v0.2.0.

---

## 9. Plan split and order

Five plans, each independently mergeable behind the full gate set:

1. **Async transport** (#7): device trait, adapter, session, cost model,
   `no_std`, `docs/core.md`. Constructive constraint: `BusResponse` is
   not extended. First — independent of the rest, and its one risk
   (`no_std`) should surface early.
2. **Volatile TM** (#74 first half): reservation, both declaration sites,
   IR flag, `inline`/`outline` propagation, contract paragraphs, tooling
   sweep (fmt, grammar + drift guard, completions, LSP), docs.
3. **Volatile PM + multiversioning** (#74 second half): reservation,
   `volatile main()` + `volatile-not-on-main`, gated pipeline (probed
   pass set, pinned by tests), digest dedup, MO variant records, linker
   column choice + legacy fallback counter, `pmt ir --variant`, docs,
   the `.pmx` byte-identity gate.
4. **Footprint engine + lint** (#53 first half): IR walk + `expand.rs`
   pre-splice computation, `dead-map-pair` (write-half, demote quickfix),
   hover + `tmt ir --footprints`, the over-approximation property test.
5. **Declared contracts** (#53 second half): `writes`/`preserves`
   grammar (+2 reserved words), the containment check + three
   diagnostics, stdlib `preserves` adoption, docs. Last — it layers on
   plan 4's engine and plan 2's grammar plumbing, and can be dropped
   without touching either.

Order 1 → 2 → 3 → 4 → 5: plans 2, 4, 5 touch the same TM front-end files
(parser, CST, fmt, LSP, grammars, `language.md`) and therefore run
sequentially, not concurrently; plan 3 interleaves as the PM side
(formats + linker — no file overlap with 1/2/4).

**Standing gates for every plan:** PM-1 `.pmx`/`-S` byte-identity,
`crates/core` arch-neutrality, `-O0` bit-identity on both toolchains, the
TM `-O0`/`-O1` × mono/frames/hybrid equivalence matrix, clippy, fmt, and
every touched drift guard (error-code registries + docs inventories,
completions registry, editor grammars, cli_docs USAGE quotes).

---

## 10. Deferred work — recorded, not ticketed

None of the following gets an issue now (ruled 2026-08-08): each has the
shape "build X when trigger Y", and no Y has happened — a ticket is work
someone intends, not a someday-maybe note. The durable record is this
spec plus the closing comments on #74/#7/#53 when the round ships; a
ticket gets filed by whoever hits the trigger, with real context in hand.

- Footprint/contract export to MO records — trigger: a link-time
  consumer (cross-unit precision; the declared contract is what crosses).
- Read-sets + liveness — trigger: someone wants a dead-write elimination
  pass.
- `ReadAll` overlap on the async bus — trigger: real slow multi-tape
  devices where serialization measurably hurts.
- Per-tape volatility bits in containers — trigger: a runtime consumer
  (embedder introspection); note the C-style ruling deliberately rejected
  this once.
- Interrupt-style external events (an ISA revision) — trigger: the
  volatile-band polling model proving insufficient in practice.
