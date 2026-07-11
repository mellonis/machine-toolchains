# .pma parity Plan 1/3 — the core assembly front-end: lexer, CST, lowering, spanned errors

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Replace the line-splitter `.pma` parser with a total, lossless, span-tracking front-end in `crates/core/src/asm/` — `assemble` becomes `lower ∘ parse_asm_cst` — and reshape `AsmError` to `{ span, kind }` with stable kebab codes, guarded by an acceptance-parity sweep. Labels tighten to dot-free (the one sanctioned acceptance change); the PM-1 `.pma` dialect version constant is born at `0.2`.

**Architecture:** The `.pmc` C1 pattern applied to the asm framework: a per-line spanned lexer feeds a **total** CST parser (`Raw` nodes are the lossless fallback for non-assembly lines); all validity checking happens in a new `lower` step that produces the exact `SourceFunction`/`SourceItem` shapes the untouched two-pass assembler consumes, now with spans threaded through every error site. Plans 2 (lint/fmt/CLI) and 3 (LSP/editors) consume this front-end.

**Tech Stack:** Rust edition 2024; zero new deps (`proptest` dev-only, already present). Design authority: `docs/superpowers/specs/2026-07-12-pma-parity-design.md` (Core section, Testing, Version spaces).

## Global Constraints

Every task's requirements implicitly include these.

- **Zero new dependencies.** Gates at every commit: `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo fmt --check`. Commit only on a clean tree.
- **Core carries zero PM-1 knowledge.** All core asm tests run against `syntax::fixture::test_syntax()`; the words "pmt"/"pmc"/"PM-1" never appear in core code.
- **Position types are `mtc_core::diagnostics::Pos`/`Span`** — 1-based, char-counted columns, half-open. Never byte offsets.
- **Losslessness = trivia-complete**, not byte-identical reprint: comments with columns, blank-line presence, raw text of unshapeable lines. Canonical input round-trips byte-identical (pinned in plan 2's fmt).
- **Hand-rolled error idiom** (house style): `#[derive(Debug, PartialEq, Eq)]`, hand-written `Display` with lowercase messages, bare `impl std::error::Error`.
- **Acceptance contract:** every currently-accepted `.pma` program still assembles to byte-identical objects; every rejected input stays rejected with the same error kind — the two sanctioned deltas are (a) dotted labels now rejected, (b) dotted/namespaced text before `:` reports a bad label name instead of an unknown mnemonic.
- Code comments cite durable pages (`docs/formats.md (assembly text)`), never issue/PR numbers.
- **Conventional commits** (`feat(core):`, `test(core):`, `feat(cli):`). **No AI/Claude attribution footers.** Do NOT merge or push; the branch is left for the user's review.

## File Structure

- `crates/core/src/asm/lexer.rs` — per-line spanned tokenizer (Task 1).
- `crates/core/src/asm/cst.rs` — CST types + total `parse_asm_cst` (Task 2).
- `crates/core/src/asm/lower.rs` — CST → `SourceFunction` validation/lowering; `SourceFunction`/`SourceItem`/`SourceOperand`/`SpannedName` move here (Task 3). `parser.rs` is **deleted** in Task 3; its tests port to `lower.rs`.
- `crates/core/src/asm/mod.rs` — `AsmError` reshape + `code()` + new `Display`; module wiring (Task 3).
- `crates/core/src/asm/assembler.rs` — span threading through `Slot` and all error sites (Task 3).
- `crates/post-machine/src/cli/build.rs` — `pmt asm` error rendering (Task 3).
- `crates/post-machine/tests/asm_acceptance.rs` — the parity sweep (Task 4).
- `crates/post-machine/src/asm/mod.rs` + `crates/post-machine/src/cli/mod.rs` + `docs/formats.md` — dialect constant + `--version` line (Task 5).

---

### Task 1: Per-line spanned lexer

**Files:**
- Create: `crates/core/src/asm/lexer.rs`
- Modify: `crates/core/src/asm/mod.rs` (add `pub(crate) mod lexer;` to the module list)

**Interfaces (Produces):**

```rust
//! Per-line spanned tokenizer for assembly text (docs/formats.md
//! (assembly text)). Total: any input tokenizes; unknown characters
//! become Junk tokens. Arch-agnostic — mnemonics are just Words here.

use crate::diagnostics::Span;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AsmTokenKind {
    /// Identifier-ish text: mnemonics, labels, directives, symbol names.
    /// May contain `.` and embedded `::` (maximal munch); never a
    /// trailing single `:`.
    Word(String),
    /// Integer literal, raw spelling retained (`007`, `-3`).
    Number(String),
    Colon,
    Comma,
    At,
    /// `;` to end of line, verbatim including the `;`.
    Comment(String),
    /// Any character no other rule accepts (`<`, `>`, `"`, …).
    Junk(char),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AsmToken {
    pub kind: AsmTokenKind,
    pub line: u32, // 1-based
    pub col: u32,  // 1-based, char-counted
    pub len: u32,  // in chars
}

impl AsmToken {
    pub fn span(&self) -> Span; // Span::new(line, col, line, col + len)
}

/// Tokenizes one line (no `\n` inside). Total — never fails.
pub(crate) fn lex_line(text: &str, line_no: u32) -> Vec<AsmToken>;
```

Lexing rules (implementation notes):
- Whitespace (spaces/tabs) separates tokens and is not emitted.
- `Word`: starts `[A-Za-z_.]`; continues over `[A-Za-z0-9_.]`; **a `::` pair is munched into the word** (two-char lookahead), a single `:` ends the word and lexes as `Colon`. So `L1:` → `Word("L1"), Colon`; `std::x:` → `Word("std::x"), Colon`; `foo.bar:` → `Word("foo.bar"), Colon`; `.func` → `Word(".func")`. A leading `::` with no open word starts a Word too (`::x` → `Word("::x")` — lowering rejects it; never Junk).
- `Number`: `-?[0-9]+`. A `-` not followed by a digit is `Junk('-')`. A digit run followed immediately by word chars (`0abc`) lexes as `Number("0")` then `Word("abc")` — the CST's shape rule turns such lines into `Raw` when the number heads the line, and lowering rejects stray tokens elsewhere.
- `;` starts `Comment` — everything to EOL verbatim (including the `;`), exactly one Comment max per line, always last.
- `@` → `At`; `,` → `Comma`; anything else → `Junk(c)`.
- Columns count `chars()`, not bytes (a `é` in a comment must not shift later spans — no later tokens exist after comments, but Junk before a comment can carry non-ASCII).

**Steps:**
- [ ] Write failing tests in `lexer.rs` (inline `#[cfg(test)]`): (1) `L1:     rgt` → `[Word("L1"), Colon, Word("rgt")]` with exact line/col/len for each; (2) `        jm      L1              ; loop` — comment token col and verbatim text `"; loop"`; (3) `std::x:` → `[Word("std::x"), Colon]`; (4) `foo.bar:` → `[Word("foo.bar"), Colon]`; (5) `.func std::api.helper local` → three Words; (6) `wr      007, -1` → `[Word("wr"), Number("007"), Comma, Number("-1")]` (raw spelling); (7) `call    @std::api` → `[Word("call"), At, Word("std::api")]`; (8) `  0004:  21 05 <goToEnd>` → leading `Number("0004"), Colon, Number("21"), Number("05"), Junk('<'), Word("goToEnd"), Junk('>')`; (9) empty and whitespace-only lines → `[]`; (10) non-ASCII: `wr 1 ; тест` — comment col is char-counted.
- [ ] Run `cargo test -p mtc-core asm::lexer` — all fail; implement `lex_line`; all pass.
- [ ] Add proptest: `lex_line` never panics on arbitrary `String` (no `\n`) × arbitrary `line_no`, and the concatenation of token texts (with kind-appropriate rendering) never exceeds the input char length — cheap sanity, the real totality check is no-panic.
- [ ] Full gates (`cargo test --workspace`, clippy `-D warnings`, `fmt --check`).
- [ ] Commit: `feat(core): spanned per-line lexer for assembly text`

---

### Task 2: CST types + total `parse_asm_cst`

**Files:**
- Create: `crates/core/src/asm/cst.rs`
- Modify: `crates/core/src/asm/mod.rs` (add `pub mod cst;`)

**Interfaces (Produces):**

```rust
//! Lossless assembly CST (docs/formats.md (assembly text)). Total:
//! every text parses — lines that are not assembly-shaped become Raw
//! nodes. Trivia-complete: comments with columns, blank-line presence,
//! raw text. Validity checking lives in lower.rs, not here.

use crate::diagnostics::Span;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AsmCst { pub items: Vec<AsmItem> }

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AsmItem { pub blank_before: bool, pub kind: AsmItemKind }

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AsmItemKind {
    /// Own-line comment: `; text`.
    Comment(AsmComment),
    /// `.func name [local]` — only when structurally exact; otherwise
    /// the line lands in Line (word ".func") and lower.rs reports the
    /// precise legacy error.
    Func(FuncCst),
    /// labels + optional instruction (label-only lines have instr: None).
    Line(LineCst),
    /// Not assembly-shaped (first token isn't a Word, or a Junk token
    /// is present). Lossless: the verbatim line text.
    Raw(RawCst),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AsmComment { pub text: String, pub col: u32 } // text incl. `;`

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrailingComment { pub text: String, pub col: u32 }

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FuncCst {
    pub name: String, pub name_span: Span, pub local: bool,
    pub span: Span, pub trailing: Option<TrailingComment>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LabelCst { pub name: String, pub span: Span }

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineCst {
    pub labels: Vec<LabelCst>,
    pub instr: Option<InstrCst>,
    pub span: Span,
    pub trailing: Option<TrailingComment>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstrCst {
    pub word: String, pub word_span: Span, // mnemonic / `.byte` / junk word
    pub operands: Vec<OperandToken>,
}

/// One comma-separated operand: the raw source slice between
/// delimiters, trimmed; span covers the trimmed slice.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperandToken { pub text: String, pub span: Span }

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawCst { pub text: String, pub span: Span }

/// Total: never fails.
pub fn parse_asm_cst(source: &str) -> AsmCst;
```

Shape rules (implementation notes; per line, over `lex_line` output):
- Empty token list → contributes `blank_before = true` to the NEXT item (runs of blanks fold to one bool, matching the `.pmc` CST). Leading file blanks set nothing.
- `[Comment]` only → `AsmItemKind::Comment` (col = token col).
- Any `Junk` token, or a first non-comment token that is not a `Word` → `Raw` (verbatim line text, span = full trimmed-line span). This is what `pmt dis --listing` output (`  0004:  21 …`, `<goToEnd>`) parses into.
- Labels: leading repeated `Word Colon` pairs (regardless of the word's grammar — `foo.bar:`/`std::x:` become label candidates; **lower.rs** rejects bad names with a precise span). After label pairs: nothing (label-only), or `Word` (instruction word) + operand region.
- `.func` special case: exactly `Word(".func") Word(name)` or `… Word("local")` (plus optional trailing comment) → `Func`. Anything else starting `.func` stays a `Line` so lowering can replicate the legacy errors verbatim (`bad function name`, `junk after \`local\``, `expected \`local\` or end of line after the name`).
- Operand region: split the remaining tokens (up to the comment) at `Comma`; each group's `text` is the original line sliced by char-cols from the group's first to last token, trimmed (this preserves raw spelling and interior anomalies like `std :: api`, which lowering rejects exactly as today). An empty group (`wr 1,,2` / trailing comma) yields an empty-text OperandToken — lowering rejects it as `bad-operand`, matching today's `"".parse::<i64>()` failure.
- A trailing `Comment` token on a Func/Line becomes `trailing: Some(TrailingComment)`.

**Steps:**
- [ ] Write failing tests: (1) the doc example from `docs/formats.md` (assembly text) parses into the expected item sequence (Func with trailing comment, Line with label, instr, trailing comment, …) — assert the full tree with spans on one representative line; (2) label-only line + multi-label line `A: B: nop` → one LineCst with two labels + instr; (3) `foo.bar:  nop` → LineCst with label candidate `foo.bar` (NOT Raw, NOT word); (4) `.func f local` → Func; `.func f loco` and `.func f local extra` and `.func` → Line with word ".func"; (5) `wr 007, -1 ; c` → operands `["007", "-1"]` raw + trailing comment; (6) listing lines `  0004:  21 05 00 00 00  call    0x0005 <goToEnd>` and `<goToEnd>` → Raw with verbatim text; (7) blank-line folding: two blanks between items → single `blank_before: true`; (8) comment-only line at col 9 → Comment { col: 9 }.
- [ ] Run `cargo test -p mtc-core asm::cst` — fail; implement; pass.
- [ ] Add proptest: `parse_asm_cst` never panics on arbitrary `String` (raw bytes via `any::<String>()`), and every input line is accounted for: count of non-blank input lines == count of produced items (totality + losslessness of line coverage).
- [ ] Full gates.
- [ ] Commit: `feat(core): total lossless assembly CST`

---

### Task 3: Spanned `AsmError` + `lower.rs` cutover

The atomic swap. `parser.rs` is deleted; `assemble` runs `lower ∘ parse_asm_cst`; every error site gains a span. This task is the largest — it must land as one commit because the crate has one `AsmError` shape.

**Files:**
- Modify: `crates/core/src/asm/mod.rs` (`AsmError`/`AsmErrorKind` reshape; `mod parser;` → `mod lower;`)
- Create: `crates/core/src/asm/lower.rs` (moves `SourceFunction`/`SourceItem`/`SourceOperand` here, adds `SpannedName`, ports ALL of `parser.rs`'s tests)
- Delete: `crates/core/src/asm/parser.rs`
- Modify: `crates/core/src/asm/assembler.rs` (span threading; `BlobDebug` line derivation)
- Modify: `crates/post-machine/src/cli/build.rs` (`pmt asm` error rendering, currently `.map_err(|e| format!("{}: {e}", input.display()))` at the `asm` subcommand)
- Modify: any other `AsmError` consumer found by `grep -rn "AsmError" crates/` (known: the compiler pipeline's internal assemble call in `crates/post-machine/src/compiler.rs`; core asm tests)

**Interfaces (Produces):**

```rust
// mod.rs — the reshape:
#[derive(Debug, PartialEq, Eq)]
pub struct AsmError { pub span: Span, pub kind: AsmErrorKind }

#[derive(Debug, PartialEq, Eq)]
pub enum AsmErrorKind {
    Syntax(&'static str),
    UnknownMnemonic(String),
    OutsideFunction,
    DuplicateFunction(String),
    DuplicateLabel(String),
    UnknownLabel(String),
    BadOperand(&'static str),
    ShortOffsetOutOfRange { target: String },
    EncodeError(&'static str),
    /// A line that is not assembly-shaped (CST Raw node).
    RawLine,
}

impl AsmErrorKind {
    /// Stable kebab-case code (docs/cli.md (error codes)).
    pub fn code(&self) -> &'static str; // "syntax" | "unknown-mnemonic"
    // | "outside-function" | "duplicate-function" | "duplicate-label"
    // | "unknown-label" | "bad-operand" | "short-offset-out-of-range"
    // | "encode-error" | "raw-line"
}
// Display for AsmErrorKind: lowercase human message, e.g.
//   UnknownMnemonic(m) → `unknown mnemonic \`{m}\``
//   RawLine            → `not assembly text`
//   ShortOffsetOutOfRange{target} → `short jump to \`{target}\` is out of range`
// Display for AsmError: `{span.start.line}:{span.start.col}: {kind} [{code}]`

// lower.rs:
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SpannedName { pub name: String, pub span: Span }

#[derive(Debug)]
pub(crate) struct SourceFunction {
    pub name: String, pub name_span: Span, pub local: bool,
    pub items: Vec<SourceItem>,
}

#[derive(Debug)]
pub(crate) enum SourceItem {
    Instr { span: Span, labels: Vec<SpannedName>, opcode: u8, operand: SourceOperand },
    RawByte { span: Span, labels: Vec<SpannedName>, value: u8 },
}

#[derive(Debug)]
pub(crate) enum SourceOperand {
    None,
    Ints(Vec<i64>),
    Name(SpannedName),
    SymbolName(SpannedName), // `@name`
}

pub(crate) fn lower(cst: &AsmCst, syntax: &ArchSyntax)
    -> Result<Vec<SourceFunction>, AsmError>;

// mod.rs — assemble keeps its signature:
pub fn assemble(syntax: &ArchSyntax, arch_id: u8, source: &str, with_debug: bool)
    -> Result<ObjectFile, AsmError>;
// body: lower(&cst::parse_asm_cst(source), syntax) → assemble passes
```

Lowering rules (must replicate `parser.rs` semantics, checks in the same order so error precedence is preserved):
- `Raw` node → `Err(AsmError { span, kind: RawLine })` immediately.
- **Label grammar check** (the tightening): each `LabelCst.name` must match `[A-Za-z_][A-Za-z0-9_]*`; violation → `Syntax("label names use letters, digits, underscore")` at the label's span. This is where `foo.bar:` and `std::x:` now fail.
- `Func` node: `is_symbol_name` check on the name (port the fn from parser.rs verbatim) → `Syntax("bad function name")`; duplicate name → `DuplicateFunction` at `name_span`; pending labels before a `.func` → `Syntax("label at end of function")` at the first pending label's span.
- `Line` with word ".func" (the malformed-directive fallback): reconstruct `rest` by joining the operand region's raw text; apply the legacy checks verbatim (`bad function name` / `junk after \`local\`` / `expected \`local\` or end of line after the name`), spans on the word.
- Label-only lines accumulate `pending_labels: Vec<SpannedName>`; `OutsideFunction` fires (at the word span, or first label span for label-only) when no `.func` is open — **before** mnemonic lookup, matching the pinned `.function f` precedence test.
- `.byte` word: single operand parsed `u8` → `BadOperand(".byte needs 0..=255")` at the operand span (or word span when no operand).
- Mnemonic lookup via `syntax.by_mnemonic` → `UnknownMnemonic` at the word span.
- Operand classification per `OperandKind` — port each check from parser.rs with spans: `takes no operand` (first operand's span), `takes one name` (word span), `bad symbol name after \`@\`` / `jump/call operands are names, not numbers` (operand span), `symbol indices are integers` (the offending operand's span), `takes symbol indices` (word span). `@` detection: `text.strip_prefix('@')`.
- Trailing pending labels at EOF → `Syntax("label at end of function")` at the label's span (parser.rs used the last line number; the span upgrade is strictly better — update the ported test's expectation).

Assembler span threading (`assembler.rs`, every site from the current code):
- `err(line, kind)` helper → `err(span: Span, kind)`.
- `Slot::Fixed/Jump/Call { line: usize, … }` → `{ span: Span, … }`; `Slot::Jump`/`Call` additionally carry `target_span: Span`/`symbol_span: Span` (from the `SourceOperand`'s `SpannedName.span`) so `UnknownLabel` and `ShortOffsetOutOfRange` point at the operand, and `DuplicateLabel` uses the label's own span (from `SpannedName`), not the item span.
- The `resolve` closure takes `span: Span` instead of `line: usize`.
- `BlobDebug` keeps its `(addr, line: u32)` shape — derive `span.start.line` at the two build sites (the MO format is untouched).

CLI (`cli/build.rs`, `asm` subcommand): render the error as
`{file}:{line}:{col}: error: {kind} [{code}]` (the exact shape `cli/fmt.rs` uses for `CompileError`), replacing the `format!("{}: {e}", …)` mapping.

**Steps:**
- [ ] Port every test from `parser.rs` into `lower.rs` (they run `lower(&parse_asm_cst(src), &test_syntax())`), updating error assertions to check `kind` AND `span` (line + col): the existing cases `parses_functions_labels_and_operands`, `label_only_line_binds_to_next_instruction`, `byte_directive_parses`, `func_directive_requires_exact_token`, `error_cases_carry_line_numbers` (now spans), `func_local_modifier_*`, `dotted_function_names_accepted`, `namespaced_function_names_accepted`, `call_operands_accept_dotted_names`, `call_operands_accept_namespaced_names`. **Two expectation changes** (the sanctioned deltas): `labels_with_dots_still_parse` becomes `labels_with_dots_are_rejected` (`foo.bar:  nop` → `Syntax` at 2:1..2:8); `label_with_namespace_colons_errors_not_misparsed` now expects `Syntax` (bad label name) instead of `UnknownMnemonic`.
- [ ] Add new span-precision tests: `unknown-mnemonic` span covers exactly the word; `bad-operand` (`jmp 5`) covers the `5`; `duplicate-label` covers the second label occurrence; `unknown-label` covers the jump operand; `duplicate-function` covers the name on the second `.func`; `raw-line` on a listing-style line.
- [ ] Reshape `AsmError`/`AsmErrorKind` in `mod.rs` (+ `code()` + both `Display` impls, with unit tests for `Display` format and one `code()` per kind).
- [ ] Write `lower.rs`; delete `parser.rs`; rewire `assemble` in `assembler.rs` (`use super::lower::{…}`, body `lower(&cst::parse_asm_cst(source), syntax)?`).
- [ ] Thread spans through `assembler.rs` (all Slot variants, `err`, `resolve`, duplicate-label at the label span, `BlobDebug` line derivation `span.start.line`).
- [ ] `grep -rn "AsmError" crates/` and fix every remaining consumer: `cli/build.rs` rendering (above); the compiler pipeline's internal assemble mapping in `crates/post-machine/src/compiler.rs` (it treats an asm failure as an internal error — keep that semantic, just adapt to the new fields); all core asm tests.
- [ ] Run the full existing suite: `cargo test --workspace`. Fix regressions until green — the assembler tests, disassembler round-trip tests, `cli_programs`, and `golden_programs` are the de-facto parity net for this step.
- [ ] Full gates.
- [ ] Commit: `feat(core): assembly front-end on the lossless CST — spanned coded AsmError, lower replaces the line parser`

---

### Task 4: Acceptance-parity sweep

**Files:**
- Create: `crates/post-machine/tests/asm_acceptance.rs`

**Interfaces (Consumes):** `mtc_post_machine::compiler::compile` (`-S`-equivalent text lives in `CompileOutput` — mirror how `cli/build.rs` obtains assembly text vs object), `mtc_post_machine::asm::assemble`, `mtc_core::asm::{disassemble_object}`.

The sweep pins three properties end-to-end with PM-1 syntax (integration tests own local helpers — no shared test-support module):

1. **compile → -S → asm ≡ compile → object**: for a battery of `.pmc` programs (write ~6 inline: straight-line, labels+branches, subroutine calls, namespaced `use std::…` routine, `.byte`-producing case if reachable, a program pulling several stdlib routines) plus the embedded stdlib itself (`include_str!` path used by `stdlib/`): compile at `-O0` AND `-O1`, take the emitted assembly text, `assemble` it, and assert the re-assembled `ObjectFile` encodes byte-identically to the directly-compiled object.
2. **dis → asm round-trip**: disassemble each object from (1) with `disassemble_object`, re-assemble, assert byte-identical blobs (documented invariant: "round-tripping through `asm` reproduces the original bytes exactly").
3. **Rejection pinning**: table-driven cases asserting `(kind, span.start)` for each `AsmErrorKind` — including the two deltas (`foo.bar:` label rejected; `std::x:` bad label name) and `raw-line` on a `--listing`-shaped snippet.

**Steps:**
- [ ] Write the sweep as three `#[test]` fns with the program battery as `const` strings; run `cargo test -p mtc-post-machine --test asm_acceptance` — must pass immediately (Task 3 landed the behavior); any failure here is a Task-3 regression to fix BEFORE this commit.
- [ ] Full gates.
- [ ] Commit: `test(post-machine): .pma acceptance-parity sweep over the CST front-end`

---

### Task 5: PM-1 dialect constant + `--version` + docs

**Files:**
- Modify: `crates/post-machine/src/asm/mod.rs` (the constant)
- Modify: `crates/post-machine/src/lib.rs` (re-export, next to `pub use parser::PMC_LANG_VERSION;`)
- Modify: `crates/post-machine/src/cli/mod.rs:62-71` (`--version` arm)
- Modify: `docs/formats.md` (assembly-text section: dialect version note + confirm the label-grammar paragraph matches the tightened code)
- Modify: `docs/cli.md` (the `pmt asm` error format + a line about stable error codes)

**Interfaces (Produces):**

```rust
/// PM-1 `.pma` dialect version — an acceptance contract (docs/formats.md
/// (assembly text)): pre-1.0 it is 0.N and N bumps on ANY grammar
/// change. 0.2: labels tightened to letters/digits/underscore.
pub const PM1_PMA_DIALECT_VERSION: &str = "0.2";
```

`--version` output becomes exactly:

```
pmt {CARGO_PKG_VERSION}
pmc language {PMC_LANG_VERSION}
pma dialect (pm-1) {PM1_PMA_DIALECT_VERSION}
```

**Steps:**
- [ ] Write a failing CLI test (alongside the existing `--version` coverage in `cli/mod.rs`'s tests, or add one) asserting the three-line output.
- [ ] Add the constant + re-export + the third `--version` line.
- [ ] `docs/formats.md`: in the `.pma` section, add the dialect-version sentence (mirroring how `docs/language.md` surfaces `PMC_LANG_VERSION`) and state the 0.2 tightening in ref-free prose; verify the "labels are colon-free" paragraph — it already documents dot-free labels, now true.
- [ ] `docs/cli.md`: `pmt asm` errors render `path:line:col: error: message [code]`; codes are stable.
- [ ] Full gates.
- [ ] Commit: `feat(cli): pma dialect version constant (0.2) surfaced in --version; asm error docs`

---

## Self-Review (run after writing, fixed inline)

- Spec coverage: lexer ✔ (T1), total CST + Raw ✔ (T2), lowering + spanned coded AsmError + assembler spans + BlobDebug ✔ (T3), acceptance sweep + label delta ✔ (T4), dialect constant + version surfacing + docs ✔ (T5). Lint/fmt/CLI-routing and LSP/editors are plans 2 and 3.
- Type consistency: `SpannedName`, `OperandToken`, `parse_asm_cst`, `lower`, `PM1_PMA_DIALECT_VERSION` used consistently across tasks; plan 2 consumes `AsmCst`/`lower`/`AsmError` exactly as defined here.
