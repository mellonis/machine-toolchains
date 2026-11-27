# .pma/.pmc parity: CST, lint, fmt, LSP — design

Date: 2026-07-12
Status: approved 2026-07-12 (brainstorm walked section-by-section)
Tracker: [machine-toolchains#15](https://github.com/mellonis/machine-toolchains/issues/15)

## Context

The `.pmc` side has span-precise diagnostics (char-level `Span`s from
`mtc_core::diagnostics`), a unified lossless CST (built for fmt, shared
with the LSP), `pmt lint`, `pmt fmt`, and a `PmcLanguageService` behind
the core LSP framework. The `.pma` side has none of it: `AsmError` is
`{ line, kind }` with a Debug-formatted `Display` and no stable codes;
assembly parsing is a `source.lines()` string-splitter that discards
comments and tracks no columns; `pmt lint`/`pmt fmt` route every explicit
file through the `.pmc` front-end regardless of extension (a `.pma` file
"fails confusingly" rather than being rejected); and the LSP serves
`.pmc` only.

The groundwork was laid deliberately: the lint-layer design homed
`Span`/`Pos`/`Diagnostic`/`Fix` in `mtc-core` "for the `.pma` follow-up
and future `tmt`", and the LSP design names this issue as the parity
charter. This spec closes the gap. The split follows the standing
pattern: span/CST machinery belongs to core's asm framework
(arch-agnostic — the future TM-1/`tmt` assembler inherits all of it);
PM-1 specifics ride `pm1_syntax()`.

## Decisions (settled during brainstorming, 2026-07-12)

| Decision | Choice |
|---|---|
| Scope | full parity in one spec: spanned errors, `.pma` CST, lint, fmt, LSP service, editor registration. Implementation may stage into multiple plans (LSP-#4 precedent: three plans off one spec) |
| Architecture | C1 mirror inside core's asm framework: one lossless `AsmCst`; `assemble` = `lower ∘ parse_asm_cst`, acceptance-parity-guarded; the legacy line-splitter is replaced, not duplicated (a CST-alongside variant was rejected as two drifting grammar authorities; folding `.pma` into the `.pmc` front-end was rejected on the core/arch boundary) |
| CST totality | `parse_asm_cst` never fails. A line that cannot shape as `label* [word operands] [; comment]` becomes a lossless `Raw` node (text + span) instead of being force-fitted. All validity checking happens at lowering/assembly |
| Structural vs semantic gates | fmt refuses a file containing `Raw` nodes (per-file error; nothing written) but formats semantically broken drafts (unknown mnemonic still grids). lint refuses on ANY fatal `AsmError` — `Raw` lines included — mirroring `.pmc` lint's compile-fatal gate. The LSP renders `Raw` lines as spanned syntax diagnostics while the rest of the tree keeps serving symbols/completions. Rationale: `pmt dis --listing` output is documented as not reassembleable; neither tool may pretend otherwise, and fmt's in-place default must never rewrite non-assembly text |
| Label grammar | docs win over code: labels are `[A-Za-z_][A-Za-z0-9_]*` — no dots, no `::` (the parser's `is_ident` accepted dots; `docs/formats.md` already documented dot-free). First acceptance change to the PM-1 `.pma` dialect → **0.1 → 0.2**, introducing its version constant per the versioning rule from the lint spec. Codegen emits only `L{n}` labels, so nothing the toolchain generates is affected |
| Error type | `AsmError { span: Span, kind }` + `kind.code()` stable kebab codes + human `Display` (`{line}:{col}: {message} [{code}]`) — the `CompileError` pattern. Assembler-phase errors (`duplicate-label`, `unknown-label`, `short-offset-out-of-range`) carry real spans too (`Slot` carries the operand/label span, not a bare line) |
| Lint home | rules in core (`core/src/asm/lint/`), same function-pointer registry shape as `.pmc`'s; `pmt` drives with `pm1_syntax()`. `unreachable-code` is arch-agnostic via `Flow`; `leftover-debugger` reads a new `ArchSyntax.break_opcode: Option<u8>` (PM-1 sets `brk`; rule silent when `None`) |
| Allow namespace | one namespace across both languages: shared defect names (`line-too-long`, `unused-label`, `redundant-jump-to-next`, `leftover-debugger`) deliberately reuse the `.pmc` codes so one `pmt.json` `lint.allow` entry suppresses the defect everywhere; `validate_allow` accepts the union of both registries |
| fmt canon | the documented column grid, extended: label col 0, mnemonic col 8, operand col 16, trailing comment col 32; single space when content overflows the mnemonic/operand/comment stops. Labels never overflow inline: a field (name + colon) of 8+ chars — too wide to sit inside the 8-column stop with a separating space — goes on its own line, as do all-but-the-last of multiple labels. `grid_line` adopts the same long-label rule so plain `dis` output stays self-canonical (`fmt(dis x) == dis x`) |
| Zero-token-changes | fmt's contract from day one (the `.pmc` leading-zeros lesson): only whitespace/newlines change; token spelling — including leading-zero numbers, which are lint's business — is untouchable, pinned by a token-stream-equality test |
| CLI routing | directory walks collect `*.pmc` AND `*.pma`; explicit files route by extension; an explicit file that is neither is a per-file error (batch continues) — replacing the accidental "any extension goes through the `.pmc` front-end". `pmt fmt -` gains `--lang pmc|pma` (default `pmc`) |
| LSP topology | one server, mux in core: `server::run` widens to a set of services; documents route by `didOpen`'s `languageId` (extension fallback); capabilities merge (trigger chars + watched globs union; semantic-token legends concatenate with per-service index remap); `did_change_config` broadcasts. Two-process and multi-language-trait alternatives rejected (editor plumbing ×2 / trait muddied) |
| `.pma` LSP features | full parity: diagnostics, completion, definition, document symbols, semantic tokens, code actions (lint fixes), formatting — single-document, like the `.pmc` service v1 |
| Editors | shared single-source `.pma` TextMate grammar with a drift guard against `pm1_syntax()` (the `.pmc` reserved-word-guard pattern); VS Code registers the `pma` language; JetBrains adds a PMA file type mapped to the same `pmtLsp` server; both manual checklists gain a `.pma` walk |

## Core: lexer, CST, lowering, errors (`crates/core/src/asm/`)

### Lexer (`asm/lexer.rs`, new)

Char-level scanner producing spanned tokens over `mtc_core::diagnostics`
`Pos`/`Span` (1-based, char-counted columns — Unicode-safe like the
`.pmc` lexer). Token kinds: `Word` (mnemonics, label names, directives
`.func`/`.byte`, operand names — undifferentiated text at this stage),
`Number` (raw spelling retained), `Colon`, `Comma`, `At`, `Comment`
(`;` to end of line, text + column), plus a catch-all for characters
outside the assembly alphabet (they land in `Raw` lines at CST level).
The lexer is total: any byte sequence tokenizes.

### CST (`asm/cst.rs`, new)

Lossless and line-oriented, mirroring the `.pmc` CST's conventions
(`blank_before: bool` per item, `TrailingComment { comment, col }`):

```rust
pub struct AsmCst { pub items: Vec<AsmItem> }

pub struct AsmItem { pub blank_before: bool, pub kind: AsmItemKind }

pub enum AsmItemKind {
    Comment(AsmComment),      // own-line comment
    Func(FuncCst),            // `.func name [local]`
    Line(LineCst),            // labels + optional instruction
    Raw(RawCst),              // lossless fallback: not assembly-shaped
}

pub struct FuncCst {
    pub name: String, pub name_span: Span, pub local: bool,
    pub span: Span, pub trailing: Option<TrailingComment>,
}

pub struct LineCst {
    pub labels: Vec<LabelCst>,        // { name, span }
    pub instr: Option<InstrCst>,      // None → label-only line
    pub span: Span, pub trailing: Option<TrailingComment>,
}

pub struct InstrCst {
    pub word: String, pub word_span: Span,   // mnemonic or `.byte`
    pub operands: Vec<OperandToken>,         // raw spelling + span each
}

pub struct RawCst { pub text: String, pub span: Span }
```

(`AsmComment` is `{ text: String }`; `TrailingComment` is
`{ text: String, col: u32 }` — the `.pma` comment form is `;`-line only,
so no kind discriminant is needed.)

`parse_asm_cst(tokens) -> AsmCst` is **total** — the line grammar has no
unclosed constructs, so every input gets a tree. Losslessness contract
matches the `.pmc` CST: trivia-complete (comments with columns,
blank-line presence, raw text of unshapeable lines), not byte-identical
reprint of arbitrary source; canonical input round-trips byte-identical.

`Raw` is the honesty valve: `pmt dis --listing` output (address/hex
columns, `<name>` annotations), or any other non-assembly text, parses
into `Raw` nodes rather than being force-fitted into fake statements.
Consumers gate on it per the decisions table.

### Lowering (`asm/lower.rs`, replaces the `parser.rs` line-splitter)

`lower(cst: &AsmCst, syntax: &ArchSyntax) -> Result<Vec<SourceFunction>, AsmError>`
produces the exact `SourceFunction`/`SourceItem` shapes the two-pass
assembler consumes today — the assembler and relaxation logic are
untouched. `SourceItem` swaps its bare `line: usize` for the item's
`Span`. All current diagnostics move here and gain the offending token's
span: `unknown-mnemonic` points at the word, `bad-operand` at the operand
token, `duplicate-function` at the name, `outside-function` at the word,
label-at-end at the label. A `Raw` node lowers to the fatal `raw-line`.

`assemble()` becomes `lower ∘ parse_asm_cst` behind the same public
signature (plus the `AsmError` reshape below). The swap is guarded by an
acceptance-parity sweep (Testing).

### `AsmError` (reshaped once; core API break, pre-1.0)

```rust
pub struct AsmError { pub span: Span, pub kind: AsmErrorKind }
```

`AsmErrorKind` gains `RawLine` and a `code() -> &'static str` method with
stable kebab codes: `syntax`, `unknown-mnemonic`, `outside-function`,
`duplicate-function`, `duplicate-label`, `unknown-label`, `bad-operand`,
`short-offset-out-of-range`, `encode-error`, `raw-line`. `Display`
renders `{line}:{col}: {message} [{code}]` with a human message per kind
(no more `{:?}`). The CLI's `pmt asm` error line becomes
`{file}:{line}:{col}: error: {message} [{code}]` — the `.pmc` rendering.

## Lint (`core/src/asm/lint/` + `pmt lint`)

Same architecture as the `.pmc` layer: a `(code, fn)` function-pointer
registry; each rule one file exposing
`fn check(&AsmLintContext, &mut Vec<Diagnostic>)`; findings are
`mtc_core::diagnostics::Diagnostic` (no severity — presentation stays
downstream); results sorted by `span.start`.

```rust
pub struct AsmLintContext<'a> {
    pub source: &'a str,
    pub cst: &'a AsmCst,
    pub functions: &'a [SourceFunction],  // lowered, spans intact
    pub syntax: &'a ArchSyntax,
}
```

Entry point mirrors `.pmc`'s split for LSP reuse:
`lint(syntax, source, allow) -> Result<Vec<Diagnostic>, AsmError>` —
any fatal (structural or semantic, gated by a full assemble) refuses the
file. Core does NOT validate allow codes: allow-list validation moves to
the one place that knows the cross-language union (the `pmt` lint
layer's `validate_allow`, extended over both registries) — otherwise a
shared `pmt.json` allowing a `.pmc`-only code would falsely error on
`.pma` files.

Starter catalog (five rules):

| code | finding | fix |
|---|---|---|
| `unreachable-code` | instruction following a `Flow::Stop`/`Flow::Jump` instruction, with no label on it (per function; `Flow` makes this arch-agnostic) | none |
| `unused-label` | label referenced by no operand | remove label (MachineApplicable) |
| `redundant-jump-to-next` | unconditional jump to the immediately following instruction | remove instruction (MachineApplicable) |
| `line-too-long` | line exceeds 80 chars | none |
| `leftover-debugger` | instruction whose opcode equals `ArchSyntax.break_opcode` | remove instruction (MaybeIncorrect — deletion of a user-written construct, per the `.pmc` applicability policy) |

`ArchSyntax` gains `break_opcode: Option<u8>`; `pm1_syntax()` sets
`brk`'s opcode; the core test fixture leaves it `None` (rule silent),
with one fixture variant setting it to prove the rule.

Duplicate/unknown labels stay fatals (assembler errors), not lints —
channel discipline unchanged.

## Fmt (`core/src/asm/fmt.rs` + `pmt fmt`)

A CST printer targeting the canonical grid, which the `docs/formats.md`
example already exhibits precisely:

- label col 0, mnemonic col 8, operand col 16, trailing comment col 32;
  a single space where content overflows a stop; trailing whitespace
  trimmed.
- `.func` lines at col 0; their trailing comments also align to col 32.
- Labels never overflow inline: a field (name + colon) of 8 or more
  chars — too wide to sit inside the 8-column stop with a separating
  space — goes on its own line; with multiple labels, all but the last
  go own-line (the last stays inline when it fits). Label-only lines
  are already legal and bind forward, so this is layout, not meaning.
  `grid_line` adopts the same rule (today it glues an 8+-char label
  field straight into the mnemonic), keeping plain `dis` output
  self-canonical.
- Own-line comments align to the mnemonic column (8) inside a function,
  col 0 at top level.
- Operand lists print `, `-separated; spaced label colons (`L1 :`)
  normalize to `L1:` (whitespace-only change).
- Blank-line runs collapse to one; leading file blanks drop; exactly one
  final `\n`; output line endings are `\n`.
- No wrapping: `.pma` is one instruction per line by grammar; a line
  pushed past 80 by a long comment is `line-too-long`'s business, not
  fmt's.

Contracts: **zero token changes** (whitespace/newlines only — spelling,
including leading-zero numbers, never rewritten), **idempotence**
(`format ∘ format == format`), **self-canonical emitters**
(`fmt(dis x) == dis x`; `fmt(compile -S x)` is the identity), and the
**structural gate** (`Raw` node present → per-file `raw-line` error,
nothing written; semantically broken drafts still format).

## CLI (`crates/post-machine/src/cli/`)

- `collect_pmc` generalizes to collect `*.pmc` and `*.pma` on directory
  walks for `lint` and `fmt`; explicit files route by extension; an
  explicit file with any other extension is a per-file error (batch
  continues). Zero-match PATH errors, `--exclude`, sorted order, and
  per-file independence all unchanged.
- `pmt lint` on a `.pma` file: fatal `AsmError`s render
  `{file}:{line}:{col}: error: {message} [{code}]` on stderr (per-file,
  batch survives); findings render `{file}:{line}:{col}: lint: {message}`
  with the same fix-hint lines. `--fix`/`--force` reuse the span-based
  edit applier.
- `pmt fmt -` gains `--lang pmc|pma` (default `pmc`); file arguments
  never need it.
- `completions/registry` updates: lint/fmt positional extension filters
  gain `.pma`; fmt gains `--lang` (equals-or-space value, choices
  `pmc|pma`). The zsh renderer follows from the registry; the registry
  drift-guard test covers the new entries.

## LSP

### Mux (core framework)

`server::run` widens from one `&mut impl LanguageService` to a set of
services (existing single-service callers and the fake-service tests
wrap in a one-element set). Routing and merging:

- `didOpen`'s `textDocument.languageId` binds the URI to a service in
  the docstore; all later requests on that URI route by the binding. An
  unexpected languageId falls back to an extension map — the
  `LanguageService` trait gains an `extensions() -> &'static [&'static str]`
  method (additive; `.pmc` service returns `[".pmc"]`, `.pma` returns
  `[".pma"]`) so the mux can build the map without language knowledge.
- Capabilities: trigger characters and watched globs union; the
  semantic-token legend is the concatenation of the services' legends,
  and the mux remaps each service's local token-type/modifier indices
  into the merged legend (services stay self-contained).
- `did_change_config` broadcasts to all services; `did_close` routes.

No protocol-visible change for `.pmc`-only clients.

### `PmaLanguageService` (`crates/post-machine/src/lsp/`)

`language_id = "pma"`, trigger characters `['@', '.']`, single-document
scope (like the `.pmc` service v1):

- **Diagnostics** (`did_update`): `parse_asm_cst` (total — never blocks
  the document) → `lower` + `assemble` with `pm1_syntax()`; the one
  fatal `AsmError` renders as an Error diagnostic (`source: "pmt"`,
  kebab code) — `Raw` lines surface here as `raw-line` syntax errors;
  on success, lint findings (`source: "pmt lint"`). `pmt.json` + IDE
  `lint.allow` union, invalid-config warnings, and `**/pmt.json`
  watching all reuse the `.pmc` plumbing.
- **Completion**: mnemonics from `ArchSyntax` at the instruction-word
  position; the enclosing function's labels at jump/branch operands;
  function symbols after `@`; directives after `.`.
- **Definition**: label reference → label definition; `call name` /
  `jmp @name` → the `.func` line.
- **Document symbols**: functions with their labels as children.
- **Semantic tokens**: function names, label definitions/references,
  numbers; exact legend types picked at implementation from the LSP
  standard set (the mux owns merging).
- **Code actions**: lint fixes. **Formatting**: asm fmt; answers `None`
  only on the structural gate.

`pmt lsp` constructs both services and hands them to the multi-service
`run`. No new CLI flags.

## Editors (`editors/`)

- Shared single-source `.pma` TextMate grammar (comments, labels,
  PM-1 mnemonics, `.func`/`.byte` directives, numbers, `@`-symbols)
  with a drift-guard test against `pm1_syntax()`'s mnemonic list.
- VS Code: `languages` gains `{ id: "pma", extensions: [".pma"] }` +
  grammar; `documentSelector` gains `{ language: 'pma' }`; activation
  `onLanguage:pma`; the task-provider language gate widens.
- JetBrains: second file type (PMA, extension `pma`) + TextMate bundle
  addition + `fileTypeMapping` to the same `pmtLsp` server with
  `languageId="pma"`.
- Both READMEs' manual checklists gain a `.pma` walk (diagnostics,
  completion, definition, symbols, tokens, format, lint fix).

## Testing

- **Acceptance-parity sweep** (the make-or-break milestone, mirroring
  the `.pmc` C1 cut): every `.pma` the toolchain can produce
  (`compile -S` over the test programs, `dis` over objects) assembles
  byte-identically through `lower ∘ parse_asm_cst`; every
  currently-rejected input stays rejected with the same error kind (now
  spanned) — except where the label ruling makes the diagnosis more
  precise (dotted/namespaced text before `:` reports a bad label name
  instead of an unknown mnemonic), and except lines that are not
  assembly-shaped at all (a leading non-word token, stray characters),
  which now report `raw-line` instead of an unknown-mnemonic on garbage. The label-dots tightening is the one
  sanctioned acceptance delta, pinned by a dedicated test (`foo.bar:`
  rejected).
- **Losslessness**: trivia-complete CST contract; byte-identical
  round-trip pinned for canonical fixtures.
- **fmt**: idempotence corpus harness (a `fmt_programs.rs` analog);
  `fmt(dis x) == dis x` and `compile -S` identity tests;
  zero-token-changes pinned by comparing comment-stripped token streams
  before/after.
- **Never-panics** property tests on lexer + `parse_asm_cst` over noise
  (proptest, core's codec convention) — doubling as the totality proof.
- **Lint**: per-rule unit tests; allow-union validation; mixed-language
  batch routing; `break_opcode: None` silence.
- **LSP**: mux cases in the core fake-service suite (two fake services:
  languageId routing, extension fallback, legend remap, config
  broadcast, close routing); a `PmaLanguageService` suite mirroring the
  `.pmc` one.
- **Drift guards**: TextMate mnemonics vs `pm1_syntax()`; completions
  registry probe (existing harness picks up the new entries).

## Version spaces and docs

Release-notes version block for the round this ships in:

- PM-1 `.pma` dialect: **0.1 → 0.2** (labels tightened to dot-free).
  The constant is born: `PM1_PMA_DIALECT_VERSION` in
  `crates/post-machine/src/asm/mod.rs`, surfaced in the
  `docs/formats.md` assembly section header and as an additional
  `pmt --version` line (the `PMC_LANG_VERSION` pattern).
- `.pmc` language version, `IR_VERSION`, MO/MX/MT containers:
  **unchanged** — stated explicitly.
- Crate API: `mtc-core` breaks (`AsmError` shape, `server::run`
  signature, `ArchSyntax.break_opcode`, parser module replaced) —
  workspace-internal, pre-1.0.

Docs impact (all published pages ref-free per policy): `docs/formats.md`
(grid rules incl. comment col 32 + long-label own-line; label grammar
paragraph now matches code; dialect version header), `docs/lint.md`
(`.pma` rules, shared allow namespace), `docs/cli.md` (lint/fmt accept
`.pma`, `fmt --lang`, unknown-extension error, `pmt asm` error format),
`docs/lsp.md` (second language + mux), editors' READMEs/checklists,
crate-level rustdoc for the new core modules.

## Out of scope

Cross-file definition/rename (needs the project-manifest work tracked
separately as issue #16), `.pma` hover docs, DAP, bash/fish completions,
multi-error recovery in lowering (one fatal at a time, like `.pmc`),
`.pma`-side attributes/doc-lines (issue #17 is `.pmc`-side), any TM-1
work (the framework pieces land arch-agnostic; `tmt` itself stays
issue #8), and JSON lint output.
