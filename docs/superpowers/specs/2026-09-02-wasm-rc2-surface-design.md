# mtc-wasm: the rc.2 surface — assembly, tape blocks, the standard library

**Status:** active. Drives the `v0.5.0-rc.2` cut. Folds
[#113](https://github.com/mellonis/machine-toolchains/issues/113),
[#114](https://github.com/mellonis/machine-toolchains/issues/114) and
[#115](https://github.com/mellonis/machine-toolchains/issues/115) — the
three asks the demo-side design
([machines-demo#136](https://github.com/mellonis/machines-demo/issues/136))
surfaced against the rc.1 binding — into one additive round over the
surface `docs/superpowers/specs/2026-09-02-wasm-binding-design.md`
shipped. Nothing that exists changes shape; every rc.1 call keeps its
meaning.

## 1. Assembly (#113)

`Lang` grows from two values to four: `"pmc" | "tmc" | "pma" | "tma"`.
A language is an architecture (PM-1 or TM-1) crossed with a kind
(source or assembly); `Lang::arch()` and `Lang::is_asm()` are the two
projections, and everything downstream of the build — `Program`,
`Session`, the listing, the registry — keys on the architecture alone.

The issue offered `Toolchain.assemble(...)` or `build` accepting the
assembly languages; the second is taken. One entry point, one result
shape, and `lang` already discriminates every other call. On `pma`/`tma`,
`build` assembles **with the line table on** (the assembler records each
instruction's physical source line, so `lineOf`/`addressForLine` work
against assembly lines as the issue asks), then links against the
stdlib exactly as a source build does. `optLevel` is accepted and
ignored — the assembler has no optimizer — and documented so.

`check` and `format` follow for the assembly languages: `check` runs
core's arch-agnostic `.pma` lint (PM-1) or `lint_tma` (TM-1) with the
assemble gate as the fatal, `format` runs the canonical-grid printer.
Not in the issue, but a four-valued `Lang` on which two values throw from
`check`/`format` is a worse contract than one on which all four answer,
and the demo's assembly editor wants both. `warn` is ignored on assembly
(no opt-in tier). `allow` codes are validated through each arch crate's
`validate_allow`, made `pub` for this — the one change outside
`crates/wasm`.

An `AsmError` becomes one `Diagnostic` the way a compile fatal does:
`code` from `AsmErrorKind::code()`, `message` from its `Display`, span →
UTF-16.

**Tape layouts of an assembled program.** PM-1: blank and mark, as
always. TM-1: the image carries cardinalities and nothing else, so each
band is `tape<i>` with the decimal glyphs `0..card-1` — the convention
`tmt tape-block new --from app.tmx` already uses (`docs/formats.md
(glyph tables)`).

## 2. Tape blocks (#114)

Two stateless calls on `Toolchain`, both architectures, plain values:

```ts
export interface TapeBlock { alphabet: string[]; tapes: TapeBlockTape[] }
export interface TapeBlockTape { origin: number; cells: Uint8Array; head: number; glyphs: string[] }
export interface TapeBlockInput { alphabet?: string[]; tapes: TapeBlockTapeInput[] }
export interface TapeBlockTapeInput { cells: Uint8Array | number[]; head?: number; origin?: number; glyphs?: string[] }
decodeTapeBlock(bytes: Uint8Array): TapeBlock
encodeTapeBlock(block: TapeBlockInput): Uint8Array
```

`decodeTapeBlock` returns the block alphabet and, per tape, its
**effective** glyph table (its own if it carries one, the block's
otherwise) — the table `tmt run` renders and validates against. A
magic, CRC, version or bounds failure is a thrown error naming the
`FormatError`.

`encodeTapeBlock` is the inverse. The per-tape field is called `glyphs`
so a `Session` snapshot (`{ glyphs, origin, cells, head, … }`) is a
valid input as it is — `encodeTapeBlock({ tapes: session.snapshots() })`
saves a run's final bands. A tape without `glyphs` inherits the block
alphabet; a block without `alphabet` takes the first tape's `glyphs`;
neither present is a thrown error. A tape whose `glyphs` equal the block
alphabet is written as inheriting. The MT version then follows the
format's own rule — every tape inheriting is version 1, any own table
is version 2 (`docs/formats.md (tape-block snapshot)`) — which is why no
`lang` parameter exists: a PM-1 block comes out as the version-1 file
`pmt` writes, a TM-1 block with differing bands as version 2. Cells are
validated against the effective table before encoding; more than 255
tapes or glyphs, or a glyph over 65535 bytes, throws instead of
panicking (the core codec asserts those).

`decode(encode(x))` is semantically `x`; `encode(decode(bytes))` is
semantically `bytes` but not necessarily byte-identical (a version-2
tape whose own table equals the block's re-encodes as inheriting).

**`Program.seedsFromTapeBlock(block)`** is the convenience the issue
called optional, taken because the mapping is where an embedder would
go wrong: block tape `i` → band `i`, each cell's glyph looked up in the
band's own glyph list (`tapes()[i].glyphs`), `origin`/`head` carried.
More tapes than bands, or a glyph the band does not know, throws naming
the tape, the glyph and the band. The result is a `Seed[]` for
`session(...)`, so the page can render the loaded bands before running.
It is a mapping, not a session constructor, on purpose.

## 3. The standard library (#115)

`Toolchain.stdlibSource(lang)` returns the embedded `.pmc`/`.tmc` text
of that architecture's library — for `pma`/`tma` too, since an assembled
program links the same library. The tarball does not grow a copy: the
API returns, by construction, the exact text the module linked, which is
the issue's point.

The library the browser links is compiled **with the line table** — a
second `OnceLock` in `crates/wasm`, not a change to the arch crates'
release-preset `stdlib::object()`, because a debug-compiled object
differs only in side tables: the code bytes are pinned identical
(`tests/stdlib.rs`), so a browser-linked image is the image the CLI
would write. Every link passes `sources: ["user", "std"]` to the linker,
so the map sidecar carries provenance, and `SourceLoc` gains the
discriminator:

```ts
export type SourceFile = "user" | "std";
export interface SourceLoc { file: SourceFile; function: string; line: number | null }
addressForLine(line: number, file?: SourceFile): number | undefined   // file defaults to "user"
```

A function's `file` is `"std"` exactly when the linker recorded the
library as its origin — so a user routine shadowing a `std::` name is
`"user"`, and a linker-synthesized function (a mono-stamped copy) takes
the provenance of the input it was stamped from. `lineOf` on a stdlib
address now returns the line in `stdlibSource(lang)`, and
`addressForLine(n, "std")` plants a breakpoint there. `mapJson()` shows
the same two strings in each function's `source` field.

## 4. Out of scope, still

Project manifests and user libraries, the language-server surface, a
JavaScript tape device, and the composition of several assembly units
(`build` takes one unit, linked against the stdlib — what the issue
asked for).

## 5. Verification

- Native tests in `crates/wasm/tests/`: `assembly.rs` (both
  dialects build, run, carry the line table on physical lines, report an
  `AsmError` as one coded diagnostic, link `std::` calls; `check`/`format`
  on both), `tapeblock.rs` (decode of core-written v1/v2, encode of every
  shape, the version rule, every error, `seedsFromTapeBlock`), and
  `stdlib.rs` (the source is the embedded text, the debug object's code
  bytes equal the release object's, a stdlib address resolves to a line
  under `file: "std"`, `addressForLine` per file, shadowing).
- `scripts/wasm-smoke.mjs` gains the JS-boundary checks for each: an
  assembled `.pma` runs, a `.tma` builds and its line table answers, a
  snapshot round-trips through encode → decode → `seedsFromTapeBlock`,
  and a step into the stdlib resolves to a line of `stdlibSource`.
- Standing gates: PM-1 byte-identity (the debug stdlib's code equals
  the release stdlib's), `crates/core` untouched, no `wasm_bindgen`/
  `js_sys` in `inner/`.
