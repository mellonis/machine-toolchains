# The browser bundle

The toolchains run in a browser. `mtc-wasm` is the crate that exposes them
to JavaScript: compile, lint, format and disassemble `.pmc` and `.tmc`
sources, assemble `.pma` and `.tma` text, read and write tape-block
snapshots, and run the linked program in a session the page drives. The
bundle it builds to is attached to every release.

## What the bundle contains

`machine-toolchains-wasm-vX.Y.Z.tar.gz` unpacks to one directory holding
`mtc_wasm_bg.wasm` (the module), `mtc_wasm.js` (the wasm-bindgen glue,
`web` target — an ES module whose default export initialises the module
from a URL, a `Response`, or raw bytes), `mtc_wasm.d.ts` (the API
reference, generated), and `manifest.json` (the toolchains and crate
versions, the wasm-bindgen version, the commit it was built from, and a
SHA-256 per file). Verify the checksums before loading; the manifest is
the contract a consumer pins.

The module is built with the `wasm` cargo profile (`opt-level = "z"`,
fat LTO, `panic = "abort"`) and `wasm-opt -Oz`. Measured at 0.5.0 with the
JavaScript boundary included: 1.18 MB raw and 466 KB gzipped, for both
toolchains' full chains — of the same order as a diagram-rendering
library. The VM-and-compilers core alone measured 321 KB gzipped before
the boundary was added, so the boundary — glue, JS object construction,
and whatever the linker could not strip — costs about 130 KB gzipped; a
size audit attributing that delta to its individual contributors is a
follow-up. A ceiling of 1 MB gzipped is enforced by the smoke test.

## The object model

Three classes; every other type is a plain JavaScript object, declared in
`mtc_wasm.d.ts`.

- **`Toolchain`** (static methods, `lang` is one of `"pmc"`, `"tmc"`,
  `"pma"`, `"tma"` — a language is an architecture, PM-1 or TM-1,
  crossed with a kind, source or assembly).
  `check(lang, source, { allow?, warn? })` returns the lint channel:
  findings as warnings, plus a compile fatal as one error. Compile
  *warnings* are not here; they come with `build`, the same split the CLI
  keeps between `lint` and `compile`. `format(lang, source)` returns the
  canonical whitespace-only text, or that fatal. `build(lang, source,
  { optLevel? })` compiles with the line table on, links against the
  embedded stdlib, and returns a `Program` plus the compile channel's
  warnings — or the fatal as one error. `stdlibSource(lang)` returns the
  standard library's text (below). `decodeTapeBlock(bytes)` and
  `encodeTapeBlock(block)` are the tape-block codec (below).
- **`Program`**: `tapes()` (one `{ name, glyphs }` per band — the
  machine block's alphabets for `.tmc`; blank and mark for `.pmc`),
  `listing()` (one row per instruction with address, bytes, mnemonic,
  operand, and the function and label from the map), `lineOf(addr)` and
  `addressForLine(line, file?)` (the line table both ways — `lineOf`
  returns `null` for an unmapped address, `addressForLine` returns
  `undefined` for an unmapped line, a wasm-bindgen `Option` convention
  rather than a deliberate distinction; compare either with `!= null`;
  `file` is a `SourceFile`, below), `disassembly()` (reassembleable
  assembly text), `bytes()` (the executable image the CLI would write)
  and `mapJson()` (its map sidecar), `seedsFromTapeBlock(block)` (below),
  and `session(seeds, limits)`. Every class has `free()` and
  `[Symbol.dispose]`, so `using` works; call one when done. (The glue
  also exports `initSync` beside the default async `init`.) Freeing a
  `Program` while one of its sessions is still running is safe — the
  session borrows only the process-wide arch registry, not the program —
  so a page may rebuild a program while an earlier run is in flight.
- **Trailing options arguments are required-but-nullable, not optional.**
  `check`'s and `build`'s options argument and `session`'s `seeds` and
  `limits` are typed `T | undefined` in `mtc_wasm.d.ts`, without the `?`
  a genuinely optional parameter would carry — a generator limitation on
  the version this bundle pins, not a design choice. Pass `undefined`
  explicitly for a default rather than omitting the argument (`p.session(
  undefined, undefined)`, not `p.session()`). `pump(budget?)` and
  `addressForLine(line, file?)` are the exceptions: their native optional
  types come through as real optional parameters.
- **`Session`**: the run. `pump(budget?)` retires instructions until the
  budget runs out, a pause fires, or the program ends, and reports which
  as `{ kind }`. `pause()`, `addBreakpoint(addr)`, `removeBreakpoint(addr)`;
  `snapshot(band)` and `snapshots()` return `{ origin, cells, head }` in
  alphabet indices plus the band's name and glyphs; `ip`, `mf`, `fr`,
  `depth`, `stack()`, `stats()`, `finished()`; `stop()` returns the
  statistics and ends the session — every later call throws.

## Assembly

`build`, `check` and `format` take `"pma"` and `"tma"` beside the two
source languages. On an assembly language `build` runs the assembler
with its line table on — each instruction records the physical line it
was written on, so `lineOf` and `addressForLine` answer against assembly
lines exactly as they do against source lines — and links the one unit
against the embedded stdlib, so a hand-written `call std::goToEnd`
resolves. The result is the same `Program`; `optLevel` is accepted and
ignored, the assembler having no optimizer. An assembler refusal (an
unknown mnemonic, a duplicate label, a line that is not assembly text)
is the one error, carrying the assembler's own code — `unknown-mnemonic`,
`raw-line`, and the rest of `docs/core.md (error codes)`.

`check` on assembly runs the assembly lint (`docs/core.md (assembly
lint)`, plus TM-1's `.tma` rules) behind the same assemble gate the CLI
uses, findings as warnings; `warn` is ignored, there being no opt-in tier.
`format` prints the canonical column grid (`docs/formats.md (assembly
text)`) and is whitespace-only: an unknown mnemonic passes through
untouched, only a line that is not assembly-shaped is refused.

The band layout of an assembled program follows what the image can say.
PM-1 is blank and mark, as always. A TM-1 image carries per-tape
cardinalities and nothing else, so `tapes()` names the bands `tape0`,
`tape1`, … and labels each band's symbols with the decimal strings
`0`…`card-1` — the convention `tmt tape-block new --from app.tmx` uses
(`docs/formats.md (glyph tables)`).

`Program.disassembly()` of a source-built program is `build`-able as
that architecture's assembly language and yields the same code bytes:
the text-expressibility gate, through the browser.

## Tape blocks

`.pmt`/`.tmt` snapshots — the MT container `docs/formats.md (tape-block
snapshot)` describes — travel through two stateless calls:

```ts
Toolchain.decodeTapeBlock(bytes: Uint8Array): TapeBlock
Toolchain.encodeTapeBlock(block: TapeBlockInput): Uint8Array
```

A decoded `TapeBlock` is `{ alphabet, tapes }`: the block-level alphabet
and, per tape, `{ origin, cells, head, glyphs }` where `glyphs` is the
table the tape's cells actually index — its own if the file gave it one,
the block's otherwise — the same table `tmt run` renders and validates
against. A magic, CRC, version or bounds failure throws, with the codec's
message.

`encodeTapeBlock` is the inverse, and its input is the decoded shape with
the redundancies made optional: a tape without `glyphs` inherits the
block `alphabet`; a block without `alphabet` takes the first tape's
`glyphs`; `head` and `origin` default to 0; `cells` may be a `Uint8Array`
or a number array. The per-tape field is called `glyphs` on purpose — a
`Session` snapshot is a valid tape as it is, so
`encodeTapeBlock({ tapes: session.snapshots() })` saves a run's final
bands. Every cell is validated against its tape's effective table before
anything is written, and a shape the container cannot hold — no tapes,
more than 255 tapes or symbols, a glyph over 65535 bytes — throws rather
than aborting.

The container version is the format's own rule, not a parameter: a tape
whose `glyphs` equal the block alphabet is written as inheriting, and a
block in which every tape inherits is version 1 — what `pmt` writes —
while any tape with its own table makes it version 2, what `tmt` writes
for bands that differ. A block is decoded and re-encoded to the same
block; the bytes are identical unless a version-2 tape carried an own
table equal to the block's, which re-encodes as inheriting.

`Program.seedsFromTapeBlock(block)` maps a block onto the program's
bands by glyph: block tape `i` seeds band `i`, and each cell's glyph is
looked up in that band's glyph list (`tapes()[i].glyphs`), so a block
that spells the alphabet in another order still lands on the right
symbols. It returns the `Seed[]` `session` takes, with `head` and
`origin` carried, so a page can render the loaded bands before running.
More tapes than bands, or a glyph the band does not know, throws naming
the tape, the glyph and the band — a block authored for another program
never silently relabels. It accepts the decoded shape and the encode
input shape alike.

## The standard library

`Toolchain.stdlibSource(lang)` returns the embedded `.pmc` or `.tmc` text
of that language's architecture — for `"pma"`/`"tma"` too, an assembled
program linking the same library. It is, by construction, exactly the
text every `build` links: the module carries one copy and both the link
and this call read it, so a page showing the library beside the user's
source needs no second fetch to keep in step.

The library the browser links carries its line table (the CLI's copy
does not; the CLI opens the materialized source through the language
server instead — `docs/lsp.md (materialized standard library)`). Debug
info is a side table, so the code bytes are the CLI's: an image built
here is the image `pmt build`/`tmt build` would write. What changes is
what the map knows. Every link stamps provenance on both inputs, and
`SourceLoc` carries it:

```ts
export type SourceFile = "user" | "std";
export interface SourceLoc { file: SourceFile; function: string; line: number | null }
```

`lineOf(addr)` on a stdlib address now answers `{ file: "std", line }`
with `line` counting into `stdlibSource(lang)`, and
`addressForLine(line, "std")` plants a breakpoint there; `file` omitted
is `"user"`. The two files never capture each other's requests — a
stdlib line and a user line with the same number resolve independently.
A function is `"std"` exactly when the linker recorded the library as
its origin, so a user routine shadowing a `std::` name is `"user"`, and
a linker-synthesized copy takes the provenance of the input it was
stamped from. `mapJson()` shows the same two strings in each function's
`source` field (`docs/formats.md (source provenance)`). A `.tmc` routine
grafts a graph, so its rows map to the graph's lines rather than the
routine header's; and the entry byte before a function's first mapped
row is the function's with `line: null`, as everywhere.

## Positions

Diagnostics and fix edits carry `from`/`to` as half-open UTF-16 string
offsets, the coordinate a browser editor indexes by. A position past the
end of the text clamps to the text length.

## Sessions

Tapes live inside the session: one band per machine tape, seeded in
alphabet indices (`tapes()` gives the glyph for each index), blank where
no seed is given. A seed cell outside its band's alphabet is a thrown
error naming the band. A TM-1 program's tape count must fall in `1..=16`;
outside that range `session()` throws before any tape is built. The
session is the pumped `AsyncSession` `docs/core.md (async session)`
describes; the pause priority (a retired break, then a pending pause,
then a breakpoint, then the budget) and the budget semantics are that
contract, unchanged. A trap is not a pause: it ends the run with
`outcome.kind === "trapped"` and the trap's kind spelled as the CLI's
exit code 3 family — `step-limit`, `no-transition`, and so on. `stopped`
and `halted` match exit codes 0 and 2.

`deviceWait` never fires today: the session's own tapes are always
ready. It is the event a device-backed tape would raise, kept so a page
written against this API needs no change when one exists.

The `paused` cause union lists every core pause cause, for exhaustiveness
against `docs/core.md (async session)`; a pumped session raises only
`brk`, `manual`, and `{ breakpoint }` today. `step` and `{ trap }` never
fire — single-stepping and trap-as-pause are not exposed through `pump`.

## Building and verifying

`scripts/build-wasm-bundle.sh` builds the bundle into
`target/wasm-bundle/`; it needs the wasm-bindgen CLI at exactly the
version the crate pins and refuses to run otherwise. `node
scripts/wasm-smoke.mjs target/wasm-bundle/dist` loads the result and runs
both toolchains end to end; CI runs both on every push to the default
branch and on every pull request, and the release workflow attaches the
tarball to the tagged release, smoke-tested under Node first.

**Failure modes.** The module is built with `panic = "abort"`: a Rust
panic — a bug, never an expected error, which is always a thrown
`JsError` or `TypeError` instead — aborts the module for good, with no
message reaching JavaScript. `Toolchain`, `Program` and `Session` all
become unusable after one; an embedder that sees an unexpected throw
(not one of the documented `JsError`s) should discard the module and
recreate its worker rather than keep calling into it.

## What is not here

Project manifests and user libraries, the composition of several
assembly units (`build` takes one, linked against the stdlib), the
language-server surface (hover, completion, navigation), and a
JavaScript-implemented tape device. Each is a possible later addition;
none is needed to compile, inspect and run a program in a page.
