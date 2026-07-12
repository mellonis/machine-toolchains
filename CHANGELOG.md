# Changelog

Release notes for the Post-machine toolchain. Every entry opens with a
version block listing all of the project's version spaces — the
toolchain crates, the `.pmc` language, the per-architecture `.pma`
dialects, the IR encoding, and the container formats — stating
`unchanged` where nothing moved, so the blocks double as a
compatibility matrix across releases.

## [0.2.0] - 2026-07-12

| Version space | This release | Previous |
|---|---|---|
| Toolchain crates (`mtc-core`, `mtc-post-machine`) | **0.2.0** | 0.1.0 |
| `.pmc` language | **0.3** | 0.1 (0.2 and 0.3 both land in this release) |
| PM-1 `.pma` dialect | **0.2** | 0.1 (implicit) |
| IR encoding (JSON) | 3 — unchanged | 3 |
| Container formats (MO / MX / MT) | unchanged | — |

### `.pmc` language

- **Doc lines and attention lines** (language 0.3): a `?` line documents
  the following function; a `!` line carries attention prose or a
  machine-readable attribute — `[deprecated]`, with the rest of the
  line as its message. Runs are docs-then-attention, bind to the next
  function declaration (nested included, at its own indent), and
  dangling runs, out-of-order blocks, unknown attributes, and duplicate
  attributes are compile errors with stable codes. One acceptance
  change rides along: a successor `!` may no longer start a line.
- **Grammar tightenings** (language 0.2): sigil adjacency (`@ name` is
  a syntax error), reserved words barred in all `::` path segments, and
  a pack of clearer parse errors.
- The language version is surfaced as a constant, in the language
  reference's header, and in `pmt --version`.

### `.pma` assembly

- **Dialect 0.2**: labels are letters, digits, and underscores only —
  dots and `::` are rejected (Unicode letters remain valid), which is
  what lets labels and namespaced symbol references coexist without
  ambiguity. The dialect version is surfaced alongside the language
  version.
- **Spanned, coded assembler errors**: `line:col`-precise spans out of
  a total, lossless assembly CST; every error carries a stable
  kebab-case code. Listing output and other non-assembly text is
  refused with a dedicated `raw-line` error instead of being
  misparsed.

### Lint

- `pmt lint` covers both languages: eleven `.pmc` rules (including
  `deprecated-call`, which flags calls to `[deprecated]` functions)
  and five `.pma` rules (unreachable code, unused labels, redundant
  jumps to the next instruction, overlong lines, leftover debugger
  breaks). One allow namespace spans both languages — a single
  `lint.allow` entry in `pmt.json`, on the command line, or in IDE
  settings suppresses a code everywhere. Machine-applicable fixes
  apply with `--fix`; deletion-shaped fixes gate behind `--force`.

### Formatting

- `pmt fmt` formats both languages: the `.pmc` formatter (4-space
  indent, 80-column comma-group wrapping, comment placement, doc-run
  printing) and the `.pma` canonical grid (labels at column 0,
  mnemonics at 8, operands at 16, trailing comments at 32, long labels
  on their own line). Both obey a zero-token-changes contract — only
  whitespace moves; number spellings such as leading zeros are
  preserved exactly (this release fixes a violation where leading-zero
  numbers were rewritten). Disassembler and `-S` output are already
  canonical; formatting them is the identity.

### Language server and editors

- **`pmt lsp`**: one server process serves `.pmc` and `.pma` over
  stdio — diagnostics with stable codes, completions (with operand
  hints for assembly mnemonics and qualified names across namespaces),
  go-to-definition (into a materialized copy of the standard library
  for `std::` calls), document symbols, semantic tokens, formatting,
  lint quickfixes, and — new in this release — **hover documentation**
  sourced from doc lines, with deprecation callouts and strikethrough
  tags on deprecated calls and completions.
- The embedded standard library documents itself: all eleven routines
  carry doc lines, so hover works out of the box on `std::` calls.
- Sideloadable editor integrations for **VS Code** and **JetBrains
  IDEs** (via LSP4IJ), each with a shared TextMate grammar, per-editor
  settings for the binary path and lint allow-list, run/task
  integration, and a manual acceptance checklist. Both plugins are at
  0.1.2, tested against this release.

### CLI

- New subcommands since 0.1.0: `lint`, `fmt` (in-place by default,
  `--check` for CI, stdin via `-` with `--lang`), `lsp`, and
  `completions` (zsh; generated from the same registry that drives the
  argument parser, so completions cannot drift from the flags).
- `pmt --version` reports all three moving version spaces.

### Tooling and docs

- Dependency vulnerability auditing: a `cargo audit` CI gate on
  lockfile changes plus a weekly schedule; the current lockfile is
  clean against the RustSec advisory database.
- A pre-release documentation audit verified every published page's
  claims against the shipped code — the reference pages, the README,
  and both editor guides describe this release accurately.

## [0.1.0] - 2026-07-06

The baseline release: the complete PM-1 pipeline — the C-like `.pmc`
language with namespaces and imports, an eight-pass `-O1` optimizing
compiler with a documented soundness model, `.pmo` objects, a
relaxing linker with lazy standard-library resolution, pure `.pmx`
executables with debug sidecars, `.pmt` tape snapshots, and a
bus-accurate sans-I/O virtual machine with typed traps and a stepping
debug session — driven by `pmt` (compile, asm, link, dis, run, tape,
ir), with the embedded standard library and the durable documentation
set (language, ISA, formats, CLI, stdlib, history).
