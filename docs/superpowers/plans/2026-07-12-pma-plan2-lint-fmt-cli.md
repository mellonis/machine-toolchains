# .pma parity Plan 2/3 — asm lint + fmt in core, CLI routing in pmt

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax.

**Goal:** An arch-agnostic lint layer (5 rules) and a canonical-grid formatter for assembly text in `crates/core/src/asm/`, plus `pmt lint`/`pmt fmt` accepting `.pma` (extension routing, shared allow namespace, `--lang` for stdin) with completions-registry and docs updates.

**Architecture:** Rules and printer read the plan-1 front-end (`AsmCst` + lowered `SourceFunction`s) and are parameterized by `ArchSyntax` (`Flow` drives `unreachable-code`; a new `break_opcode` field drives `leftover-debugger`). `pmt` drives both with `pm1_syntax()`; allow-list validation is centralized in the pmt lint layer over the union of both registries. fmt gates on structure only (`Raw` nodes); lint gates on any fatal `AsmError`.

**Tech Stack:** Rust edition 2024; zero new deps. Design authority: `docs/superpowers/specs/2026-07-12-pma-parity-design.md` (Lint, Fmt, CLI sections). Prerequisite: plan 1 merged (`parse_asm_cst`, `lower`, spanned `AsmError` exist).

## Global Constraints

- **Zero new dependencies.** Gates at every commit: `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo fmt --check`.
- **Core carries zero PM-1 knowledge** — core lint/fmt tests use `test_syntax()`; `break_opcode` is `None` in the base fixture.
- **Thin renderer:** core lint/fmt never print; every terminal byte originates in `cli/`.
- **Zero-token-changes (fmt):** only whitespace/newlines change; token spelling (incl. leading-zero numbers) is untouchable.
- **Structural vs semantic gates:** fmt refuses only `Raw`-containing files; lint refuses any fatal `AsmError`.
- **Channel discipline:** fatals are errors, never lint findings; duplicate/unknown labels stay fatal.
- **Diagnostics carry no severity**; `Diagnostic { code, span, message, fix }` from `mtc_core::diagnostics`.
- Conventional commits (`feat(core):`, `feat(cli):`, `test(post-machine):`). **No AI/Claude attribution footers.** Do NOT merge or push.

## File Structure

- `crates/core/src/asm/syntax.rs` — `break_opcode` field (Task 1).
- `crates/post-machine/src/asm/mod.rs` — `pm1_syntax()` sets `break_opcode: Some(BRK)` (Task 1).
- `crates/core/src/asm/lint/mod.rs` — context, registry, entry (Task 2).
- `crates/core/src/asm/lint/rules/{unreachable_code,unused_label,redundant_jump,line_too_long,leftover_debugger}.rs` — one file per rule (Tasks 2–3).
- `crates/core/src/asm/fmt.rs` — the printer (Task 4).
- `crates/core/src/asm/disassembler.rs` — `grid_line` long-label rule (Task 4).
- `crates/post-machine/src/lint/mod.rs` — `validate_allow` over the union (Task 5).
- `crates/post-machine/src/cli/{lint.rs,fmt.rs}` — routing + `.pma` paths + `--lang` (Tasks 5–6).
- `crates/post-machine/src/completions/registry.rs` — `.pma` extensions + `--lang` (Task 6).
- `docs/{lint.md,cli.md,formats.md}` — reference updates (Task 6).

---

### Task 1: `ArchSyntax.break_opcode`

**Files:**
- Modify: `crates/core/src/asm/syntax.rs` (field + fixture)
- Modify: `crates/post-machine/src/asm/mod.rs` (`pm1_syntax()`)

**Interfaces (Produces):**

```rust
pub struct ArchSyntax {
    pub entries: Vec<SyntaxEntry>,
    pub relax_pairs: Vec<RelaxPair>,
    pub entry_opcode: u8,
    /// The debugger-break opcode, when the arch has one (drives the
    /// leftover-debugger lint; None = rule silent).
    pub break_opcode: Option<u8>,
}
```

`test_syntax()` sets `break_opcode: None`. `pm1_syntax()` sets `break_opcode: Some(BRK)` (BRK = 0x0E from `crate::arch::opcodes`).

**Steps:**
- [ ] Add the field; fix both constructors (compiler drives the tour — any other `ArchSyntax` literal in tests fails to compile until given the field).
- [ ] Add a `pm1_syntax()` assertion in the existing post-machine asm tests: `assert_eq!(pm1_syntax().break_opcode, Some(0x0E));`
- [ ] Full gates.
- [ ] Commit: `feat(core): ArchSyntax.break_opcode — arch-declared debugger break`

---

### Task 2: asm lint framework + `unreachable-code` + `unused-label`

**Files:**
- Create: `crates/core/src/asm/lint/mod.rs`, `crates/core/src/asm/lint/rules/mod.rs`, `rules/unreachable_code.rs`, `rules/unused_label.rs`
- Modify: `crates/core/src/asm/mod.rs` (add `pub mod lint;` — note: module path becomes `mtc_core::asm::lint`)

**Interfaces (Produces):**

```rust
//! Assembly lint layer (docs/lint.md). Arch-agnostic: control flow via
//! ArchSyntax::Flow, the break opcode via ArchSyntax::break_opcode.

use crate::diagnostics::Diagnostic;

pub struct AsmLintContext<'a> {
    pub source: &'a str,
    pub cst: &'a AsmCst,
    pub functions: &'a [SourceFunction],
    pub syntax: &'a ArchSyntax,
}

pub(crate) type Rule = fn(&AsmLintContext, &mut Vec<Diagnostic>);

/// Public so the pmt lint layer can validate allow codes over the
/// cross-language union. (code, rule) pairs, defect-named kebab codes.
pub const RULES: &[(&str, Rule)] = &[
    ("unreachable-code", rules::unreachable_code::check),
    ("unused-label", rules::unused_label::check),
    // Task 3 appends: redundant-jump-to-next, line-too-long, leftover-debugger
];

/// Fatal gate = a full assemble (structural Raw lines AND semantic
/// errors both refuse the file). Does NOT validate `allow` — the
/// driver owns that (it knows the cross-language union).
pub fn lint(syntax: &ArchSyntax, source: &str, allow: &[String])
    -> Result<Vec<Diagnostic>, AsmError>;
```

Implementation notes:
- `lint` body: `parse_asm_cst` → `lower` → run `assemble(syntax, 0, source, false)?` for the full fatal gate (arch id is irrelevant to the gate; discard the object) → build context → run non-allowed rules → sort by `span.start` (Pos is Ord).
- `unreachable-code`: walk each function's `items` in order; after an item whose opcode's `Flow` is `Stop` or `Jump` (unconditional — `Branch` does NOT arm it), the next item is a finding **unless it carries at least one label**; the armed state also resets after a labeled item. Span = the unreachable item's span. Message: `unreachable code: no label between here and the preceding unconditional jump/stop`. No fix. RawByte items: `.byte` data after a terminator with no label is legitimately flagged too (same rule, no special case).
- `unused-label`: collect every `SpannedName` in `labels` per function; subtract targets referenced by `SourceOperand::Name` within the same function (label references never cross functions). Finding span = the label's span; fix = `Fix { description: "remove the unused label", applicability: MachineApplicable, edits: [Edit { span: label span extended through the `:` and any following spaces, replacement: "" }] }` — compute the edit span from the CST's `LabelCst` (label span + 1 for the colon; consume trailing spaces up to the next token so the line stays grid-clean). NOTE: `SourceOperand::SymbolName` (`@name`) targets function symbols, never labels — excluded from reference counting.

**Steps:**
- [ ] Write failing tests in each rule file + `lint/mod.rs` (test_syntax programs as `const` strs): unreachable after `jmp`/`stop`/`ret`; NOT after `br` (Branch); labeled successor not flagged; reset semantics; unused label flagged with correct edit span; label referenced by `jmp` not flagged; `@`-symbol operand does not count as a label reference; allowed code suppressed; fatal input (`foo garbage` unknown mnemonic / a Raw line) → `Err(AsmError)`; findings sorted by span.
- [ ] Run `cargo test -p mtc-core asm::lint` — fail; implement; pass.
- [ ] Full gates.
- [ ] Commit: `feat(core): asm lint layer — unreachable-code, unused-label`

---

### Task 3: `redundant-jump-to-next` + `line-too-long` + `leftover-debugger`

**Files:**
- Create: `crates/core/src/asm/lint/rules/redundant_jump.rs`, `rules/line_too_long.rs`, `rules/leftover_debugger.rs`
- Modify: `crates/core/src/asm/lint/mod.rs` (append the three RULES entries in the order shown in Task 2's comment)

**Rule specs:**
- `redundant-jump-to-next`: an item with `Flow::Jump` + `SourceOperand::Name(t)` where `t` labels the **immediately following item in the same function**. Span = the whole instruction item's span. Fix: remove the instruction line (MachineApplicable; edit span covers the item's line including the newline — but NOT its labels: if the jump line carries labels, replace only the instruction portion and keep the labels binding forward; simplest correct edit when labels are present is replacing from the word's start col to end-of-line).
- `line-too-long`: source lines (raw, `source.lines()`) longer than 80 **chars**. Span = `Span::new(line, 81, line, len+1)`. Message mirrors the `.pmc` rule's. No fix.
- `leftover-debugger`: any `SourceItem::Instr` whose opcode equals `ctx.syntax.break_opcode` (rule returns immediately when `None`). Span = item span. Fix: remove the instruction line, `MaybeIncorrect` (deletion of a user-written construct — the `.pmc` applicability policy), same label-preserving edit shape as `redundant-jump-to-next`.

**Steps:**
- [ ] Failing tests per rule: jump-to-next flagged, jump-over-one not, forced-short `jmp.s`-style mnemonics also flagged (Flow is what matters), labels-on-jump-line edit keeps the labels; an 81-char line flagged at col 81 (append trailing spaces? no — trailing content); `leftover-debugger` silent under `test_syntax()` (break_opcode None) and firing under a fixture variant `ArchSyntax { break_opcode: Some(0x0F), … }` with an extra `SyntaxEntry { opcode: 0x0F, mnemonic: "dbg", … }` added locally in the test.
- [ ] Implement; pass; full gates.
- [ ] Commit: `feat(core): asm lint rules — redundant-jump-to-next, line-too-long, leftover-debugger`

---

### Task 4: asm fmt + `grid_line` long-label rule

**Files:**
- Create: `crates/core/src/asm/fmt.rs`
- Modify: `crates/core/src/asm/mod.rs` (add `pub mod fmt;` and re-export `pub use fmt::format_asm;`)
- Modify: `crates/core/src/asm/disassembler.rs` (`grid_line`)

**Interfaces (Produces):**

```rust
//! Canonical-grid printer for assembly text (docs/formats.md (assembly
//! text)): label col 0, mnemonic col 8, operand col 16, trailing
//! comment col 32. Zero token changes — whitespace/newlines only.

/// Err = the structural gate: the file contains a Raw (non-assembly)
/// line; nothing else refuses (unknown mnemonics still format).
pub fn format_asm(source: &str) -> Result<String, AsmError>;
```

Printing rules (each is a test):
- Grid stops 0/8/16/32; when a field overflows its stop, one space separates it from the next field. Trailing whitespace trimmed.
- Label field (name + `:`) of **8+ chars** goes on its own line (col 0); with multiple labels, all but the last go own-line; the last stays inline when its field is ≤ 7 chars + colon... precisely: inline iff `name.chars().count() + 1 <= 7`? No — inline iff field ≤ 7 chars **is wrong**; the rule is: inline iff field (name+colon) fits cols 0..8 leaving ≥1 space before col 8, i.e. `field_len <= 7`. An 8-char field would touch the mnemonic → own line.
- `.func name [local]` at col 0; label-only lines at col 0; own-line comments at col 8 inside a function, col 0 at top level (before the first `.func` or between functions); trailing comments at col 32 (one space if content reaches past 31).
- Operands joined `, `; spaced colons (`L1 :`) normalize to `L1:`; word spelling verbatim from the CST tokens (raw operand text is re-tokenized only for separator normalization — split on the CST's `OperandToken` boundaries, never inside one).
- Blank-line runs collapse to one (`blank_before`); no leading file blanks; exactly one trailing `\n`; `\n` line endings throughout (CRLF input normalizes — whitespace-only change).
- No wrapping ever.

`grid_line` change (same file as today's `format!("{label_field:<8}{mnemonic:<8}{operand}")`): when the label field is 8+ chars, emit the label on its own line followed by the instruction line without a label. Signature stays `fn grid_line(label: Option<&str>, mnemonic: &str, operand: &str) -> String` — it may now return a two-line string; all existing callers `push_str` the result and a `\n`, so change it to always return line content WITHOUT trailing newline but possibly containing an interior `\n`. Update the disassembler tests accordingly.

**Steps:**
- [ ] Failing tests: (1) the `docs/formats.md` example reprints byte-identically (it is canonical — trailing comments at 32); (2) a scrambled-whitespace version of the same program formats TO the canonical text; (3) long label `verylongname:` → own line, instruction on the next; (4) `A: B: nop` → `A:` own-line, `B:      nop` inline; (5) idempotence over every fixture (`format_asm(format_asm(x)) == format_asm(x)`); (6) zero-token-changes: for each fixture, `lex` every line of input and output, drop Comment tokens' columns, assert equal token-kind/text sequences (comments compare by text too — they are tokens); (7) leading-zero operand `wr 007` survives verbatim; (8) structural gate: a listing-shaped line → `Err` with `kind == RawLine`, span on that line; (9) unknown mnemonic still formats; (10) blank-run collapse + final-newline; (11) `grid_line(Some("verylongname"), "nop", "")` returns `"verylongname:\n        nop"` and short labels are unchanged vs today.
- [ ] `fmt(dis x) == dis x`: disassemble a couple of objects built via `test_syntax()` assembly (reuse an assembler-test program), assert `format_asm(&dis) == Ok(dis)`.
- [ ] Implement `format_asm` + the `grid_line` change; pass; full gates.
- [ ] Commit: `feat(core): asm fmt — canonical grid printer with structural gate; grid_line long-label rule`

---

### Task 5: `pmt lint` routes `.pma`

**Files:**
- Modify: `crates/post-machine/src/lint/mod.rs` (`validate_allow` widens to the union)
- Modify: `crates/post-machine/src/cli/lint.rs` (`collect_pmc` → `collect_sources`; per-file routing)

**Interfaces (Produces / changes):**

```rust
// lint/mod.rs — the ONE allow validator, now over both registries:
pub(crate) fn validate_allow(codes: &[String]) -> Result<(), LintError> {
    for code in codes {
        let known = RULES.iter().any(|(c, _)| c == code)
            || mtc_core::asm::lint::RULES.iter().any(|(c, _)| c == code);
        if !known { return Err(LintError::UnknownAllowCode(code.clone())); }
    }
    Ok(())
}
// pmc lint() keeps calling validate_allow — a .pma-only code in a
// pmt.json shared by both languages must NOT error on .pmc files.
```

`collect_sources` (renamed from `collect_pmc`, still in `cli/lint.rs`, still shared with fmt): the directory-walk extension check widens to `x == "pmc" || x == "pma"`; the explicit-file branch keeps pushing any extension (routing decides later); the doc comment updates.

Per-file routing in the lint loop (`cli/lint.rs`), by extension:
- `.pmc` → existing path unchanged.
- `.pma` → `mtc_core::asm::lint::lint(&crate::asm::pm1_syntax(), &source, &effective_allow)`; BUT validate first: call `crate::lint::validate_allow(&effective_allow)` per file (cheap; the `--allow` set was already validated once up front — keep that too so a bad `--allow` still aborts the run before any file work). `Ok(diags)` → the `--fix` path reuses `apply_fixes` + re-lint exactly like `.pmc` (the edit applier is language-blind — spans + replacements); `Err(e)` → stderr `{file}:{line}:{col}: error: {kind} [{code}]`, `any = true`, continue.
- anything else (explicit file) → stderr `{file}: error: unknown source extension (expected .pmc or .pma)`, `any = true`, continue.

USAGE string: `pmt lint PATH...` help text mentions both extensions.

**Steps:**
- [ ] Failing integration tests (extend `crates/post-machine/tests/` lint CLI coverage following its existing harness style): (1) `pmt lint dir/` with a `.pmc` and a `.pma` file lints both (one finding each, both rendered); (2) explicit `foo.txt` → per-file unknown-extension error, batch continues, exit 1; (3) `.pma` with unused label + `--fix` removes it and exits clean on re-run; (4) `--allow unreachable-code` accepted (union) and suppresses on `.pma`; (5) `--allow non-camel-case` on a `.pma`-only run accepted (union — no false unknown-code); (6) `--allow nonsense` still aborts the whole run; (7) `.pma` with a listing line → per-file `raw-line` error on stderr; (8) `pmt.json` `lint.allow` merges for `.pma` files identically to `.pmc`.
- [ ] Implement; pass; full gates.
- [ ] Commit: `feat(cli): pmt lint accepts .pma — extension routing, union allow namespace`

---

### Task 6: `pmt fmt` routes `.pma`; `--lang`; registry + docs

**Files:**
- Modify: `crates/post-machine/src/cli/fmt.rs`
- Modify: `crates/post-machine/src/completions/registry.rs`
- Modify: `docs/cli.md`, `docs/lint.md`, `docs/formats.md`

**Changes:**
- `cli/fmt.rs` per-file loop: route by extension — `.pmc` → `format_source`, `.pma` → `mtc_core::asm::format_asm`, other → the same per-file unknown-extension error as lint. Both formatters return `Result<String, E>` with `{span, kind, code()}`-shaped errors — render identically (the existing stderr line works for both; factor a tiny local closure if needed).
- Stdin: `fmt_stdin(check)` gains the language: `--lang pmc|pma` (default `pmc`); parse via `args.value("--lang")?` before positionals; reject values other than `pmc`/`pma` with `` `--lang` takes pmc or pma ``; `--lang` with file paths is an error (`--lang applies to stdin (-) only`).
- USAGE: `pmt fmt PATH... [--exclude PATH]... [--check]` / `pmt fmt - [--check] [--lang pmc|pma]`.
- Registry (`completions/registry.rs`): `pmc_or_dir()` → rename `source_or_dir()` with `extensions: strings(&["pmc", "pma"])` (used by both lint and fmt specs — positional AND `--exclude` hints); `fmt_spec()` gains `FlagSpec::value("--lang", "stdin language (pmc or pma)", ValueHint::Choices(strings(&["pmc", "pma"])))`. The registry drift-guard (`tests/completions_registry.rs`) probes the real parser with every entry — it will catch a mismatch; extend its expectations where it pins lint/fmt hints.
- Docs: `docs/cli.md` — lint/fmt accept `.pma`, per-file unknown-extension error, `--lang`, `pmt asm` coded errors (if not already from plan 1 Task 5); `docs/lint.md` — the five `.pma` rules with one-line descriptions + the shared-allow-namespace paragraph + `break_opcode` note in ref-free prose; `docs/formats.md` — the canonical grid gains the trailing-comment-col-32 and long-label-own-line sentences (with `pmt fmt` named as the enforcing tool).
- **fmt self-canonical emitters, PM-1 edition**: add a test asserting `format_asm` over `pmt compile -S` output and over `pmt dis` output of a real PM-1 program is the identity (extends plan 1's `asm_acceptance.rs` or a new `fmt_pma.rs` integration test — new file preferred, mirroring `fmt_programs.rs`).

**Steps:**
- [ ] Failing tests: fmt batch over a mixed dir formats both languages; `.pma` in-place write + `--check` exit codes; stdin `--lang pma` grids a scrambled snippet; `--lang` with paths rejected; unknown-extension per-file error; registry drift-guard green after the registry edit; `compile -S`/`dis` identity test.
- [ ] Implement; pass; full gates. Run `pmt completions zsh | zsh -n` via the existing `completions_zsh.rs` harness (it runs in-tree).
- [ ] Commit: `feat(cli): pmt fmt accepts .pma + stdin --lang; completions registry + docs`

---

## Self-Review (run after writing, fixed inline)

- Spec coverage: break_opcode ✔ (T1), 5 rules + fatal gate + no-core-allow-validation ✔ (T2/T3/T5), fmt canon + zero-token-changes + structural gate + grid_line + self-canonical ✔ (T4/T6), extension routing + per-file unknown-ext error + `--lang` + union allow ✔ (T5/T6), registry + docs ✔ (T6).
- Type consistency with plan 1: consumes `parse_asm_cst`/`lower`/`AsmCst`/`SourceFunction`/`SourceItem`/`SourceOperand::Name(SpannedName)`/`AsmError{span,kind}`/`RawLine` as defined there; `format_asm` and `mtc_core::asm::lint::{RULES, lint, AsmLintContext}` are what plan 3's LSP service consumes.
