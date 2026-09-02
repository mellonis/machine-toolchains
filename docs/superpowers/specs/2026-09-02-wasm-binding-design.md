# The wasm binding crate (`mtc-wasm`) — design

Date: 2026-09-02. Status: approved design, spec under review.
Driving issue: [#6](https://github.com/mellonis/machine-toolchains/issues/6)
(body rewritten and size-measured the same day; the measurement comment is
[here](https://github.com/mellonis/machine-toolchains/issues/6#issuecomment-5503639603)).
Sequencing: branches from master after the wasm32 CI gate (`24fd2db`).
The demo-side pages are a separate round in `mellonis/machines-demo`,
designed there once this crate ships a bundle.

## 1. Problem and measured facts

The three library crates already build for `wasm32-unknown-unknown`
unchanged, and CI gates that. What does not exist is a JavaScript
boundary: nothing exposes compile → link → run, lint, fmt, or the
disassembly to a browser, and no artifact carries a wasm build anywhere.

Measured 2026-09-02 (throwaway `cdylib`, raw `extern "C"` exports, fat
LTO, `panic = "abort"`, stripped, `wasm-opt -Oz`):

| exported surface | after `wasm-opt` | gzip -9 |
|---|---|---|
| VM only, both arches (decode MX, load, run) | 64 KB | 26 KB |
| + PM-1 compile and link against the stdlib | 489 KB | 200 KB |
| + TM-1 compile and link against the stdlib | 676 KB | 272 KB |
| + both full chains | 795 KB | 321 KB |

The demo's mermaid renderer chunk alone gzips to 439 KB; its whole JS
payload to 1.61 MB. **The compilers ship in the browser.** A VM-only
module is not needed for size and is not built.

Also verified: the in-memory chain (`compile` → `link` →
`Machine::from_executable` → `run`/sessions) touches no filesystem or
clock. `std` on this target is a stub OS layer (filesystem calls fail as
unsupported, stdout is discarded, `Instant::now` panics); the surfaces
that would reach it — the LSP/DAP transports, the manifest loaders, the
TM stdlib's on-disk cache — are host-only by nature and are not wrapped.

## 2. Decisions (ruled, with what was rejected)

1. **Artifact channel: a release artifact built by this repo's CI.** The
   demo downloads a checksummed bundle attached to the GitHub release for
   a pinned tag. Rejected: an npm package published from here (a second
   publishing flow for one consumer); the demo building the wasm itself
   (a Rust toolchain in a Svelte repo, builds on whichever laptop ran
   them). Consequence: the binding crate lives **here**.
2. **v1 surface: build + run + dis + lint + fmt, both toolchains.** The
   browser editor gets the same channels the CLI has. Rejected: build +
   run only (the ip view has nothing to highlight); deferring lint/fmt
   (they are one library call each).
3. **API shape: wasm-bindgen classes, plain JS values for data.** Results
   are built through `js-sys`/wasm-bindgen structs, never JSON text, so
   `serde_json` stays out of the module and the `.d.ts` is generated.
   Rejected: flat functions over JSON strings (+~100 KB, hand-written
   types); `serde-wasm-bindgen` (types degrade to `any`).
4. **Tapes live in Rust; snapshots on demand; room for a JS device
   later.** The demo's own JS engines work this way (the worker owns the
   tapes, the UI reads `TapeSnapshot`s on `built`/`ran`/`progress`), so
   the session slots into the same worker with the same message shapes.
   The device slot is an enum with one variant today so a JS-implemented
   `AsyncTapeDevice` can be added on one band without redesign.
5. **Positions cross as UTF-16 half-open offsets** (`from`/`to` as JS
   string indices). One known consumer, CodeMirror, takes them directly.
   Rejected: LSP line/character (a JS-side conversion per diagnostic);
   byte offsets (wrong for non-ASCII until converted).
6. **Testing: native tests of the layer under wasm-bindgen, plus a Node
   smoke test of the real bundle in CI.** Rejected: `wasm-bindgen-test`
   in headless Chrome (a browser install in CI for what Node can prove).
7. **One class family with a `lang` parameter**, not `Pm*`/`Tm*` pairs:
   the public library API is symmetric for everything v1 needs.
8. **`debug_info` is always on in the browser.** The line table is what
   source-level stepping and the ip→line highlight run on; the cost is
   object size, irrelevant in memory.
9. **Channel split inherited from the CLI.** `check` returns the lint
   channel (findings, plus a compile fatal as one error); `build` returns
   the compile channel's warnings alongside the program. Both lint layers
   state the split as a contract (`crates/*/src/lint/mod.rs`), and it
   avoids compiling twice per keystroke.

## 3. Crate and bundle

- `crates/wasm`, package **`mtc-wasm`**, `crate-type = ["cdylib", "rlib"]`,
  version in lockstep with the other three. Dependencies: the three
  crates, `wasm-bindgen` (pinned `=0.2.x`), `js-sys`. It is the only crate
  carrying either, and the workspace's dependency rule (serde only) keeps
  holding for the other three.
- **Layout.** `src/lib.rs` is the wasm-bindgen layer: thin adapters only.
  `src/inner/` is plain Rust with no wasm-bindgen types — `positions.rs`
  (span → UTF-16), `snapshot.rs` (device ↔ snapshot ↔ layout),
  `listing.rs` (map + `listing_parts` → rows), `program.rs` (compile,
  link, layouts), `session.rs` (device slot, pump, events), `registry.rs`
  (a single process-wide leaked `ArchRegistry` holding `Pm1` and `Tm1`;
  `Tm1` is width-agnostic — its constructor only validates the tape
  count — so one instance serves every program). The registry is
  `'static`, which gives `Machine<'static>` and `AsyncSession<'static>`,
  so a session can own its machine without self-references.
- **Bundle** produced by `scripts/build-wasm-bundle.sh`:
  `cargo build -p mtc-wasm --release --target wasm32-unknown-unknown`
  under a `[profile.release]` that sets `opt-level = "z"`, `lto = "fat"`,
  `codegen-units = 1`, `panic = "abort"`, `strip = true` for this crate
  (profile overrides scoped so the CLIs' release profile is untouched);
  `wasm-bindgen --target web --out-dir`; `wasm-opt -Oz`. Contents:
  `mtc_wasm_bg.wasm`, `mtc_wasm.js`, `mtc_wasm.d.ts`, `manifest.json`
  (`toolchains_version`, `crate_version`, `wasm_bindgen_version`,
  `built_from` commit, `sha256` per file). Packed as
  `machine-toolchains-wasm-vX.Y.Z.tar.gz`. The `web` target runs in a
  Vite Web Worker and loads in Node from bytes, so one bundle serves the
  demo and the smoke test.

## 4. Object model

Signatures in TypeScript as the generated `.d.ts` will present them.
Data types are plain objects; only the four named classes are
wasm-bindgen classes.

```ts
type Lang = "pmc" | "tmc";

class Toolchain {
  static check(lang: Lang, source: string, opts?: CheckOptions): Diagnostic[];
  static format(lang: Lang, source: string): FormatResult;
  static build(lang: Lang, source: string, opts?: BuildOptions): BuildResult;
}
interface CheckOptions { allow?: string[]; warn?: string[] }      // warn: TM opt-in tier
interface BuildOptions { optLevel?: 0 | 1 }                        // debug_info always on
type FormatResult = { ok: true; text: string } | { ok: false; error: Diagnostic };
type BuildResult  = { ok: true; program: Program; diagnostics: Diagnostic[] }
                  | { ok: false; diagnostics: Diagnostic[] };      // fatal as one error

class Program {
  tapes(): TapeLayout[];                       // per band: name + glyphs
  listing(): ListingRow[];                     // debugger code view, structured
  lineOf(addr: number): SourceLoc | null;      // LineIndex::resolve
  addressForLine(line: number): number | null; // LineIndex::address_for_line
  disassembly(): string;                       // reassembleable .pma/.tma text
  bytes(): Uint8Array;                         // the MX image
  mapJson(): string;                           // the .map sidecar
  session(seeds?: Seed[], limits?: Limits): Session;
  free(): void;
}
interface TapeLayout { name: string; glyphs: string[] }
interface ListingRow { addr: number; bytes: string; mnemonic: string; operand: string;
                       function: string | null; label: string | null }
interface SourceLoc  { function: string; line: number | null }
interface Seed       { cells: Uint8Array | number[]; head?: number; origin?: number }
interface Limits     { maxSteps?: number; maxTacts?: number }

class Session {
  pump(budget?: number): PumpEvent;
  pause(): void;
  addBreakpoint(addr: number): void;
  removeBreakpoint(addr: number): void;
  snapshot(band: number): TapeSnapshot;
  snapshots(): TapeSnapshot[];
  readonly ip: number; readonly mf: boolean; readonly fr: number;
  readonly depth: number; stack(): number[]; stats(): RunStats;
  finished(): RunResult | null;
  stop(): RunStats;                            // consumes; later calls throw
  free(): void;
}
type PumpEvent =
  | { kind: "deviceWait" }
  | { kind: "budgetSpent" }
  | { kind: "paused"; cause: "step" | "brk" | "manual" | { breakpoint: number } }
  | { kind: "finished"; result: RunResult };
interface RunResult { outcome: Outcome; stats: RunStats; ip: number; stack: number[] }
type Outcome = { kind: "stopped" } | { kind: "halted" } | { kind: "trapped"; trap: TrapInfo };
interface TrapInfo  { kind: string; at?: number; detail?: string }   // kind spelled as the
                                                                     // equivalence tests spell it
interface RunStats  { steps: number; coreTacts: number; stallTacts: number; totalTacts: number }
interface TapeSnapshot { band: number; name: string; glyphs: string[];
                         origin: number; cells: Uint8Array; head: number }

interface Diagnostic {
  code: string; severity: "error" | "warning";
  from: number; to: number;                    // UTF-16 offsets, half-open
  message: string; fix?: Fix;
}
interface Fix  { description: string; applicability: "machineApplicable" | "maybeIncorrect";
                 edits: Edit[] }
interface Edit { from: number; to: number; replacement: string }
```

Library entries behind each method (all `pub` today; no widening needed):

| method | post-machine | turing-machine | core |
|---|---|---|---|
| `check` | `lint::lint(source, LintOptions{allow})` | `lint::lint(source, LintOptions{allow, warn})` | `diagnostics::{Diagnostic, Fix, Edit}` |
| `format` | `fmt::format` | `fmt::format` | — |
| `build` | `compiler::compile` (+ `CompileReport.diagnostics`), `asm::link`, `stdlib::object()` | same names | `linker::LinkOutput{executable, map}` |
| `tapes` | one band from `arch::DEFAULT_GLYPHS` (space = blank, `*` = mark, the CLI's own convention), name `"tape"` | `compiler::machine_tape_layout` | — |
| `listing` | `asm::listing_executable` for text; rows via core | same | `asm::disassembler::listing_parts`, `linker::MapFile` |
| `lineOf`/`addressForLine` | — | — | `linemap::LineIndex` |
| `disassembly` | `asm::disassemble_executable_with_map` | same | — |
| `session` | `arch::Pm1`, `InfiniteTape` | `arch::Tm1::new(tape_count)`, `WideTape` per band | `Machine::from_executable`, `async_session[_tapes]`, `SyncAsAsync` |

`.pma`/`.tma` (assembly source in, object out) are **not** in v1; the
browser editor is a source-language editor. Listed under §10 for a later cut.

## 5. Positions and diagnostics

- Core `Span` is 1-based `(line, col)` with `col` counting characters,
  half-open. `inner/positions.rs` builds a table of UTF-16 line starts
  once per source and converts `Pos → from/to` as `line_start_utf16 +
  utf16_len(prefix of that line up to col)`. Total: a position past the
  end clamps to the text length, the same policy `pos_to_lsp` applies.
  Tests: ASCII, a non-BMP glyph inside a `.tmc` alphabet (two UTF-16
  units), CRLF line endings, a span ending at end-of-file.
- Severity: lint findings are `warning`; a compile fatal (`LintError::
  Compile`, or `compile`'s `Err`) is `error`; the TM `warn`-tier rules
  are on only when named in `opts.warn`, exactly as `LintOptions` takes
  them. `LintError::UnknownAllowCode` is thrown as a JS error naming the
  code — a caller bug, not a diagnostic.
- Fixes cross only when the library kept them; the comment guard decides
  Rust-side. Applicability passes through unchanged.
- `format`'s error is the compile fatal rendered as a `Diagnostic`; a
  formatted result is the whole text (fmt is canonical and whitespace-
  only; the caller diffs if it wants minimal edits).

## 6. Session semantics

- `program.session(seeds, limits)`: one `Seed` per band in alphabet
  **indices**; JS maps glyphs to indices through `program.tapes()`. A
  missing seed is a blank band with the head at 0. An index outside the
  band's alphabet throws, naming the band. PM-1 seeds are `0|1`.
- Devices: PM-1 one `InfiniteTape` (`from_cells`); TM-1 one `WideTape`
  per band (`from_snapshot(width = alphabet cardinality)`), the tables
  attached via `async_session_tapes`. Each wrapped in `SyncAsAsync`
  behind `enum Device { Owned(..) }` — the one-variant slot §2.4 names.
- `pump(budget)` is the only way execution advances; `undefined` means
  run to the next pause or termination. `PumpEvent` mirrors core's enum
  one to one, `PauseCause` flattened to the `cause` field. The pause
  priority (break → pause → breakpoint → budget) is core's contract
  (`docs/core.md (AsyncSession)`) and is not restated or re-checked.
- A trap folds into `finished` with `outcome.kind = "trapped"`, as on
  the CLI (exit code 3); `stopped`/`halted` match exit codes 0/2.
- `snapshot(band)` returns the trimmed span core emits (`to_snapshot`),
  plus the band's `name` and `glyphs` so the renderer needs no second
  lookup. Snapshots work on a finished session; `stop()` consumes it and
  later calls throw. `free()` releases without the stats.
- Limits pass through to `RunLimits{max_steps, max_tacts}`; the demo's
  settings panel already owns those numbers. `stack_depth` and the tact
  profile stay at core defaults in v1.
- Demo mapping, for the record (the demo round owns the details):
  RUNNING_AUTO = pump a small budget, post `progress`, yield;
  RUNNING_CONTINUOUS = pump a large budget, post `progress`, repeat;
  `paused` ↔ RUNNING_PAUSED; `finished` ↔ HALTED with the outcome kind
  as its flavour; `deviceWait` never fires with owned devices.

## 7. Build and release

- **`scripts/build-wasm-bundle.sh`** (bash, no new tooling beyond
  wasm-bindgen-cli and binaryen): reads the `wasm-bindgen` pin out of
  `crates/wasm/Cargo.toml`, refuses to run if the installed
  `wasm-bindgen` CLI version differs, builds, binds, optimises, writes
  `manifest.json`, packs the tarball into `target/wasm-bundle/`. Runs
  the same way locally and in CI.
- **`.github/workflows/release.yml`**, on `push: tags: ["v*"]`: checkout,
  the pinned toolchain from `rust-toolchain.toml` (wasm32 target included
  by the pin), `cargo install wasm-bindgen-cli --version <pin> --locked`,
  `apt-get install binaryen`, run the script, `gh release upload <tag>
  <tarball> --clobber`. The release itself is still created by the
  maintainer (`gh release create`, as today); the workflow only attaches.
  If the release does not exist yet when the tag lands, the job waits
  for it (retry with backoff for a bounded time), then attaches.
- **`test.yml`** gains the bundle build and the smoke test (§8) after the
  wasm32 library gate. wasm-bindgen-cli is cached by `Swatinem/rust-cache`
  through `~/.cargo/bin`; if that proves flaky, `taiki-e/install-action`
  has a `wasm-bindgen` entry.
- The demo side (its own round): a fetch script keyed by tag + checksum
  verifying `manifest.json`, an environment variable pointing at a local
  bundle path for iteration, cached under a gitignored directory.

## 8. Testing and gates

- **Native** (`crates/wasm/tests/`, nextest like everything else):
  `positions.rs` (§5 cases); `snapshot.rs` (seed → device → snapshot
  round trip, out-of-alphabet rejection, PM `0|1`); `listing.rs` (rows
  cover every address exactly once and agree with `listing_executable`'s
  text line for line); `program.rs` (one `.pmc` and one `.tmc` build,
  warnings channel populated, `lineOf`/`addressForLine` round-trip
  through the map); `session.rs` (each toolchain: run to `finished`
  and compare the final tape to the literal the CLI produces; one program
  per `Trap` kind mapped to its `TrapInfo.kind`; breakpoint → `paused`;
  `pause()` → `paused manual`; budget → `budgetSpent` with no progress
  lost; `stop()` then use → error).
- **Smoke** (`scripts/wasm-smoke.mjs`, Node 24 via `actions/setup-node`):
  loads the `web` glue from bytes (`init({ module_or_path: bytes })`),
  then for both languages: `check` yields one expected finding on a
  fixture with a known lint hit; `format` round-trips a fixture to its
  committed `.fmt`; `build` a fixture, `session`, `pump()` to `finished`,
  compare the final snapshot to a literal; `lineOf(program.
  addressForLine(n))` returns line `n`. Exit non-zero on any mismatch.
- **Size ceiling**: the smoke script asserts the gzipped `.wasm` under
  **1 MB**. Deliberately generous: it exists to catch an accidental
  `serde_json`/`format!`-heavy pull-in, not to enforce the measurement.
  The measured number is recorded in `docs/wasm.md` per release.
- **Standing gates**: no change to `crates/core`, so PM-1 byte-identity
  and core neutrality are not touched; the `--lib` wasm32 gate keeps
  running on the three crates. `mtc-wasm` itself is `wasm32`-only in
  purpose but compiles natively (wasm-bindgen types compile on host),
  which is what the native tests rely on. Nothing in `inner/` names a
  wasm-bindgen type: a test reads the `inner/` sources and fails on any
  `wasm_bindgen` or `js_sys` mention, so the boundary stays greppable.

## 9. Docs and versions

- New durable page **`docs/wasm.md`**: what the bundle contains and how
  to verify it; the object model (§4) as the reference; the session
  contract pointing at `docs/core.md (AsyncSession)`; positions; the
  channel split; the size record per release. Forge-agnostic: "the
  bundle attached to the release" without a URL.
- `README.md` front door: one paragraph and a pointer. `CLAUDE.md`:
  the crate in the Architecture map, the bundle script and release
  workflow under Commands, the version table row.
- Version spaces: `mtc-wasm` in lockstep with the crates; the JS API
  version is the crate version, stated in `manifest.json`; wasm-bindgen
  pin bumps are deliberate commits like the toolchain pin. CHANGELOG
  entry at the next cut, per the standing ruling.
- Code comments cite `docs/wasm.md (<topic>)` once the page lands; until
  then prose only, never this spec.

## 10. Out of scope (named so nobody rediscovers them)

- A JS-implemented device on a band (`Device::Js`), and `deviceWait`
  actually firing — waits for a stand or a WebSocket tape to exist.
- Per-step device command traces for belt animation (the demo's mirror
  replay); a snapshot per pause covers v1.
- Semantic tokens, hover, completion, go-to-definition for the browser
  editor (the LSP surface); `analyze_staged` is `pub(crate)` and stays so.
- `.pma`/`.tma` in the browser (assemble hand-written assembly).
- Project manifests, user libraries, `--nostdlib`; `stack_depth` and
  tact-profile selection; IR emission.
- The demo pages themselves (a `/pmt` + `/tmt` pair or one page with a
  language switch, the disassembly pane, CodeMirror modes ported from
  `editors/grammars/`) — designed in `machines-demo`.

## 11. Open items carried into the plan

- The exact `wasm-bindgen` pin (latest stable 0.2.127 on 2026-08-08;
  pick at implementation time).
- Whether `Swatinem/rust-cache` caches `~/.cargo/bin` reliably enough
  for `wasm-bindgen-cli`, or the install-action route is needed.
- The bounded wait in `release.yml` for a release that does not exist
  yet at tag-push time (or: document "create the release first, then
  push the tag" and fail fast).
- The `unused variable: source` release-build warning in
  `crates/post-machine/src/fmt/print.rs::format_tree` (read only under
  `cfg(debug_assertions)`) — a one-line fix that rides this arc since the
  bundle build is the first release-profile build CI runs.
