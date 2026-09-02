# The browser bundle

The toolchains run in a browser. `mtc-wasm` is the crate that exposes them
to JavaScript: compile, lint, format and disassemble `.pmc` and `.tmc`
sources, and run the linked program in a session the page drives. The
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
fat LTO, `panic = "abort"`) and `wasm-opt -Oz`. Measured at 0.4.0 with the
JavaScript boundary included: 1.12 MB raw and 450 KB gzipped, for both
toolchains' full chains — of the same order as a diagram-rendering
library. The VM-and-compilers core alone measured 321 KB gzipped before
the boundary was added, so the boundary (glue, JS object construction,
and the map's JSON serialiser) costs about 130 KB gzipped. A ceiling of
1 MB gzipped is enforced by the smoke test.

## The object model

Three classes; every other type is a plain JavaScript object, declared in
`mtc_wasm.d.ts`.

- **`Toolchain`** (static methods, `lang` is `"pmc"` or `"tmc"`).
  `check(lang, source, { allow?, warn? })` returns the lint channel:
  findings as warnings, plus a compile fatal as one error. Compile
  *warnings* are not here; they come with `build`, the same split the CLI
  keeps between `lint` and `compile`. `format(lang, source)` returns the
  canonical whitespace-only text, or that fatal. `build(lang, source,
  { optLevel? })` compiles with the line table on, links against the
  embedded stdlib, and returns a `Program` plus the compile channel's
  warnings — or the fatal as one error.
- **`Program`**: `tapes()` (one `{ name, glyphs }` per band — the machine
  block's alphabets for `.tmc`; blank and mark for `.pmc`), `listing()`
  (one row per instruction with address, bytes, mnemonic, operand, and
  the function and label from the map), `lineOf(addr)` and
  `addressForLine(line)` (the line table both ways), `disassembly()`
  (reassembleable assembly text), `bytes()` (the executable image the
  CLI would write) and `mapJson()` (its map sidecar), and `session(seeds,
  limits)`. Call `free()` when done.
- **`Session`**: the run. `pump(budget?)` retires instructions until the
  budget runs out, a pause fires, or the program ends, and reports which
  as `{ kind }`. `pause()`, `addBreakpoint(addr)`, `removeBreakpoint(addr)`;
  `snapshot(band)` and `snapshots()` return `{ origin, cells, head }` in
  alphabet indices plus the band's name and glyphs; `ip`, `mf`, `fr`,
  `depth`, `stack()`, `stats()`, `finished()`; `stop()` returns the
  statistics and ends the session — every later call throws.

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

## Building and verifying

`scripts/build-wasm-bundle.sh` builds the bundle into
`target/wasm-bundle/`; it needs the wasm-bindgen CLI at exactly the
version the crate pins and refuses to run otherwise. `node
scripts/wasm-smoke.mjs target/wasm-bundle/dist` loads the result and runs
both toolchains end to end; CI runs both on every push, and the release
workflow attaches the tarball to the tagged release.

## What is not here

Assembly sources (`.pma`/`.tma`), project manifests and user libraries,
the language-server surface (hover, completion, navigation), and a
JavaScript-implemented tape device. Each is a possible later addition;
none is needed to compile, inspect and run a program in a page.
