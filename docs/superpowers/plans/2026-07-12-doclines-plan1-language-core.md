# .pmc doc/attention lines Plan 1/2 — language core: lexer, CST, doc-map, 0.3

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax.

**Goal:** `?` doc lines and `!` attention lines become real `.pmc` grammar — lexed positionally, held losslessly in the CST, carried as one additive AST field, qualified into `Analysis.docs` by flatten — with `PMC_LANG_VERSION` at 0.3 and the grammar documented.

**Architecture:** Positional lexing (a line whose first non-whitespace char is `?`/`!` lexes as one text-to-EOL token; `!` elsewhere stays the successor `Bang`). `parse_cst` collects a run (docs block then attention block, grammar-fixed order) onto the following `FunctionCst`; all four new error shapes are parse-time `CompileError`s with stable codes. `lower_cst` reduces the run to `FnDoc { paragraphs, deprecated }` on the AST function; every compiler pass ignores it; `flatten` keys it by qualified name into `Analysis.docs`. Plan 2 consumes.

**Tech Stack:** Rust edition 2024, zero new deps. Design authority: `docs/superpowers/specs/2026-07-12-pmc-doc-lines-attributes-design.md`. Prerequisite: the round-a branch (the #19 fmt spelling fix) merged — the CST/fmt files this touches must not race it.

## Global Constraints

- Zero new dependencies. Gates at every commit: `cargo fmt`, `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo fmt --check`.
- Grammar-fixed run order: optional `?` block then optional `!` block; interleave or `!`-before-`?` = parse error. Dangling run = parse error. Unknown `[attr]` = parse error. Duplicate `[deprecated]` = parse error.
- Sigils recognized at the FIRST NON-WHITESPACE column of an item-position line only — `!` inside expressions (successor) and `?` anywhere else keep today's behavior byte-for-byte.
- Doc text: verbatim after the sigil (one canonical leading space, not required on input); paragraphs join lines with a single space; empty `?` line = paragraph break.
- `-O0` bit-identity untouched; `IR_VERSION` and containers untouched.
- Conventional commits (`feat(post-machine):`), no AI/Claude attribution footers, no issue/PR refs in code/docs (cite `docs/language.md (doc lines)`).
- Do NOT merge or push; branch left for review.

## File Structure

- `crates/post-machine/src/lexer.rs` — `TokenKind::DocLine(String)` / `TokenKind::AttentionLine(String)` + positional rule (Task 1).
- `crates/post-machine/src/cst.rs` — `DocRunItem` + `FunctionCst.doc_run` (Task 2).
- `crates/post-machine/src/parser.rs` — `parse_cst` run collection + checks; `FnDoc` on `parser::Function`; `lower_cst` reduction; new `CompileErrorKind` variants live with the others in `compiler.rs` (Task 2–3).
- `crates/post-machine/src/compiler.rs` — error kinds + codes; `flatten` builds `Analysis.docs` (Task 3).
- `crates/post-machine/src/parser.rs` `PMC_LANG_VERSION` + `docs/language.md` + the `--version` test expectation (Task 4).

---

### Task 1: Lexer — positional `?`/`!` line tokens

**Files:**
- Modify: `crates/post-machine/src/lexer.rs`

**Interfaces (Produces):**

```rust
// TokenKind gains (payload = the raw text AFTER the sigil, verbatim,
// excluding the line terminator; may be empty):
DocLine(String),
AttentionLine(String),
```

Lexing rule: when the scanner is at the first non-whitespace character of a line (same line-start tracking the existing lexer uses for columns) and that character is `?` → consume to EOL as `DocLine`; `!` in that position → `AttentionLine`. `!` anywhere else lexes as `Bang` exactly as today; `?` anywhere else stays the existing lex error. Token `col` = the sigil's column; `len` = chars from sigil through last text char. Both modes (`WithComments`/`WithoutComments`) emit them (they are semantic, not trivia).

**Steps:**
- [ ] Failing tests in `lexer.rs`: (1) `"? doc text"` → `[DocLine("doc text")]` with exact col/len (canonical space stripped: payload excludes ONE leading space if present); (2) `"    ! [deprecated] msg"` at indent → `AttentionLine("[deprecated] msg")`, col 5; (3) `"?"` alone → `DocLine("")`; (4) `"right(!);"` still lexes `Bang` inside parens (unchanged assertion vs existing tests); (5) `"x ? y"` — `?` mid-line still errors exactly as today (pin the current error); (6) both lex modes emit the new tokens.
- [ ] Run `cargo test -p mtc-post-machine --lib lexer` — fail; implement; pass.
- [ ] Full gates.
- [ ] Commit: `feat(post-machine): lexer — positional doc (?) and attention (!) line tokens`

---

### Task 2: CST + parse_cst — runs, order, attachment, attribute checks

**Files:**
- Modify: `crates/post-machine/src/cst.rs`, `crates/post-machine/src/parser.rs`, `crates/post-machine/src/compiler.rs` (error kinds)

**Interfaces (Produces):**

```rust
// cst.rs
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocRunItem { pub blank_before: bool, pub kind: DocRunKind }

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DocRunKind {
    Doc { text: String, span: Span },              // "?" line
    Attention { attr: Option<AttrCst>, text: String, span: Span }, // "!" line
    Comment(Comment),                              // ordinary comment inside the run
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttrCst { pub name: String, pub span: Span } // v1: "deprecated"

// FunctionCst gains:
pub doc_run: Vec<DocRunItem>,   // empty = undocumented

// compiler.rs — CompileErrorKind gains (codes in parens):
DanglingDocRun,          // "dangling-doc-run"
DocLineOrder,            // "doc-line-order"  (? after !, i.e. interleave/wrong order)
UnknownAttribute(String),// "unknown-attribute"
DuplicateAttribute,      // "duplicate-attribute"
```

Rules for `parse_cst` (binding, from the spec): a run accumulates `DocLine`/`AttentionLine` tokens (plus interleaved comments/blank markers) at item position, top level or body; order check on the Doc/Attention subsequence only (comments/blanks don't participate); the run binds to the next `FunctionCst` at that scope — anything else next (statement, `use`, namespace, close brace, EOF) → `DanglingDocRun` at the run's first line. Attention parsing: optional leading `[ident]` (exact token shape: `[`, ident, `]` at the start of the payload — parse from the payload string; `deprecated` is the only accepted name, anything else → `UnknownAttribute` at the attr span; a second `[deprecated]` in one run → `DuplicateAttribute`). `lower_cst` reduction comes in Task 3 — in THIS task `lower_cst` simply ignores `doc_run` (compiles as before); the C1 parity guard (`parse == lower_cst ∘ parse_cst`) must stay green.

**Steps:**
- [ ] Failing tests (parser.rs test module, acceptance/rejection table): legal — docs only; attention only; both in order; nested at indent; blanks + `//` comments interleaved within/after the run; run before a nested function inside a body. Rejects — `?!?` interleave (`DocLineOrder`), `!` block then `?` block (`DocLineOrder`), dangling at top level before `use`/namespace/EOF and in-body before a statement/close (`DanglingDocRun`, span = first run line), `[depercated]` (`UnknownAttribute`), two `[deprecated]` lines (`DuplicateAttribute`). Each error asserts kind + code + span.start.
- [ ] CST losslessness: a documented function's `doc_run` round-trips `clone() == self`; text payloads verbatim.
- [ ] Run focused; implement; pass. Verify the C1 parity guard test still green.
- [ ] Full gates.
- [ ] Commit: `feat(post-machine): pmc grammar 0.3 — doc/attention runs in the CST with order, attachment, and attribute checks`

---

### Task 3: AST carry + flatten doc-map

**Files:**
- Modify: `crates/post-machine/src/parser.rs` (AST `Function` + `lower_cst`), `crates/post-machine/src/compiler.rs` (`flatten`, `Analysis`)

**Interfaces (Produces):**

```rust
// parser.rs (AST side)
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FnDoc {
    /// Paragraphs: lines joined by single spaces; empty ? line splits.
    pub paragraphs: Vec<String>,
    /// Some(message) when [deprecated] present; message may be empty.
    pub deprecated: Option<String>,
}
// parser::Function gains: pub doc: Option<FnDoc>,   (None = undocumented)

// compiler.rs
// Analysis gains: pub docs: HashMap<String, FnDoc>,  // key = fully-qualified flattened name
```

`lower_cst` reduces `doc_run` → `FnDoc` (paragraph assembly per the spec: single-space joins, empty-`?` splits; attention prose without an attr contributes NOTHING to `FnDoc` in v1 — wait, no: bare-prose `!` lines are warnings shown in hover; they join `paragraphs`? NO — decision: bare-prose attention lines become their own trailing paragraphs prefixed nothing at the data level; hover rendering decides presentation. Concretely: `FnDoc` gains `pub attention: Vec<String>` — one entry per bare-prose `!` line, verbatim). Every existing compiler pass ignores `doc` (compile-only tests unchanged). `flatten` copies each function's `FnDoc` into `Analysis.docs` under the same fully-qualified name it already computes (nested dot-mangling, namespace `::` paths included).

CORRECTED interface (use this, not the sketch above):

```rust
pub struct FnDoc {
    pub paragraphs: Vec<String>,      // from ? lines
    pub attention: Vec<String>,       // bare-prose ! lines, verbatim, in order
    pub deprecated: Option<String>,   // [deprecated]'s message ("" if none)
}
```

**Steps:**
- [ ] Failing tests: paragraph assembly (multi-line join with single space; empty `?` splits into two paragraphs); attention prose captured in order; deprecated message captured (`! [deprecated] use goToStart instead` → `Some("use goToStart instead")`; bare `! [deprecated]` → `Some("")`); `Analysis.docs` keys for a top-level fn, a nested fn (dot-mangled), a namespaced fn (`ns::f`), and a namespaced nested one; undocumented functions absent from the map.
- [ ] `-O0` bit-identity + behavior: run the existing compile/e2e suites unchanged (documented fixtures added only to NEW tests).
- [ ] Full gates.
- [ ] Commit: `feat(post-machine): lower doc runs onto the AST and qualify them into Analysis.docs`

---

### Task 4: `PMC_LANG_VERSION` 0.3 + `docs/language.md`

**Files:**
- Modify: `crates/post-machine/src/parser.rs` (the constant), `docs/language.md`, the `--version` CLI test in `crates/post-machine/tests/cli_programs.rs` (its exact-string expectation carries `pmc language 0.2`)

**Steps:**
- [ ] Flip `PMC_LANG_VERSION` to `"0.3"`; update the pinned three-line `--version` test expectation.
- [ ] `docs/language.md`: header version sentence → 0.3; new grammar section "Doc lines and attention lines" (sigil position rule, run shape with the fixed `?`-then-`!` order, attachment + dangling error, `[deprecated]` + unknown-attribute error, paragraph semantics, plain-prose contract); version-history entry for 0.3 (ref-free prose).
- [ ] Full gates.
- [ ] Commit: `feat(post-machine): pmc language version 0.3 — doc and attention lines documented`

---

## Self-Review (run after writing, fixed inline)

- Spec coverage: grammar rules incl. all four errors ✔ (T1–T2), data flow to `Analysis.docs` ✔ (T3), version + language docs ✔ (T4). fmt/lint/LSP/stdlib are plan 2.
- Type consistency: `FnDoc { paragraphs, attention, deprecated }` (the CORRECTED block is authoritative), `DocRunItem`/`DocRunKind`/`AttrCst`, `Analysis.docs: HashMap<String, FnDoc>` — plan 2 consumes these names exactly.
- Note for plan 2: hover presentation of `attention` lines is a consumer decision; the data layer stores them verbatim.
