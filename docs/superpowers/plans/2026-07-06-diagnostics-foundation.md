# Diagnostics Foundation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the stringly `Warning { line, message }` / point-based `CompileError` diagnostics with span-carrying, code-bearing `Diagnostic`s living in `mtc-core`, split `compile()` into a codegen-free `analyze()` front half, and land the compiler riders (sigil adjacency, reserved-word path guard, six-fix error-message pack).

**Architecture:** A new arch-agnostic `diagnostics` module in `mtc-core` defines `Pos`/`Span`/`Diagnostic`/`Fix`/`Edit`/`Applicability`. The `.pmc` front end (lexer → parser → flatten → `ir::lower`) is migrated to produce real spans everywhere; the four existing warnings become coded `Diagnostic`s; `compile()` is recomposed as `analyze()` + optimize/codegen/assemble back half. This plan is plan 1 of 2 — the lint layer plan (`2026-07-06-pmc-lint-layer.md`) consumes every interface produced here.

**Tech Stack:** Rust (cargo workspace: `mtc-core` at `crates/core`, `mtc-post-machine` at `crates/post-machine`), serde/serde_json only, hand-rolled CLI (no clap).

**Spec:** `docs/superpowers/specs/2026-07-06-pmc-lint-layer-design.md` (approved + audited). This plan covers the spec's §1 (diagnostic primitives), the migrations, §2's `analyze()` split, the "Compiler riders" section, and the foundation share of §6 (docs).

## Global Constraints

- Dependencies stay `serde`/`serde_json` only (plus `proptest` as an existing dev-dep). **No new crates.**
- **Thin-renderer rule:** library code never prints; every byte of terminal output originates in `crates/post-machine/src/cli/`.
- **`-O0` bit-identity:** `pmt compile -O0` output must remain byte-identical to pre-change output. The golden suite (`cargo test -p mtc-post-machine --test golden_programs`) enforces this — it must pass untouched at every commit.
- Quality gates at **every** commit: `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo fmt --check` (run `cargo fmt` first).
- Published docs (`README.md`, `docs/*.md`) are forge-agnostic: no issue/PR numbers, no hosting URLs.
- Commit messages: conventional commits with scope (`feat(core):`, `refactor(post-machine):`, `docs(language):`). **No AI/Claude attribution footers.**
- All positions are 1-based and character-counted (not bytes — identifiers are Unicode). Spans are end-exclusive.

**Interfaces produced for plan 2 (the lint layer consumes these exactly):**
`mtc_core::diagnostics::{Pos, Span, Diagnostic, Fix, Edit, Applicability}`; `Token { kind, line, col, len }` + `Token::span()`; parser types `Label { value, span }`, `Statement { labels: Vec<Label>, items, line, span }`, `Function::name_span`, `Import::span`, `Item::Call { name, name_span, succ, succ_span, line }`, `Item::Builtin { which, succ, succ_span, line }`, `Item::Check { marked, blank, span, line }`; `CompileReport { diagnostics: Vec<Diagnostic>, opt }`; `pub(crate) analyze(source) -> Result<AnalysisOutput, CompileError>` with `AnalysisOutput { tokens, ast, ir, diagnostics, scopes: ScopeSummary }`; warning codes `undeclared-external`, `unused-import`, `unused-function`, `unreachable-code`.

---

### Task 1: Core diagnostics module

**Files:**
- Create: `crates/core/src/diagnostics.rs`
- Modify: `crates/core/src/lib.rs`
- Test: unit tests inside `crates/core/src/diagnostics.rs`

**Interfaces:**
- Consumes: nothing (pure new data types).
- Produces: `mtc_core::diagnostics::{Pos, Span, Applicability, Edit, Fix, Diagnostic}` and constructors `Span::new(start_line, start_col, end_line, end_col)`, `Span::point(line, col)`. Every later task and all of plan 2 import from here.

- [ ] **Step 1: Write the failing tests**

Create `crates/core/src/diagnostics.rs` with the test module only (types come in Step 3):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn positions_and_spans_order_by_start() {
        let a = Span::new(1, 5, 1, 8);
        let b = Span::new(2, 1, 2, 2);
        let c = Span::new(1, 9, 1, 10);
        let mut spans = vec![b, c, a];
        spans.sort();
        assert_eq!(spans, vec![a, c, b]);
        assert!(Pos { line: 1, col: 9 } < Pos { line: 2, col: 1 });
    }

    #[test]
    fn point_spans_are_one_column_wide() {
        let p = Span::point(3, 7);
        assert_eq!(p.start, Pos { line: 3, col: 7 });
        assert_eq!(p.end, Pos { line: 3, col: 8 });
    }

    #[test]
    fn a_diagnostic_carries_its_optional_fix() {
        let d = Diagnostic {
            code: "unused-label",
            span: Span::new(12, 3, 12, 5),
            message: "label 5 is never referenced (function 'f')".into(),
            fix: Some(Fix {
                description: "remove the label prefix `5:`".into(),
                applicability: Applicability::MaybeIncorrect,
                edits: vec![Edit {
                    span: Span::new(12, 3, 12, 5),
                    replacement: String::new(),
                }],
            }),
        };
        assert_eq!(d.fix.as_ref().unwrap().edits[0].replacement, "");
        assert_eq!(
            d.fix.unwrap().applicability,
            Applicability::MaybeIncorrect
        );
    }
}
```

Register the module in `crates/core/src/lib.rs` (alphabetical order):

```rust
pub mod asm;
pub mod diagnostics;
pub mod formats;
pub mod linker;
pub mod vm;
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p mtc-core diagnostics`
Expected: COMPILE ERROR — `Span`, `Pos`, `Diagnostic` etc. not found.

- [ ] **Step 3: Write the implementation**

Prepend to `crates/core/src/diagnostics.rs` (above the test module):

```rust
//! Shared diagnostic primitives: positions, spans, and structured
//! findings with optional machine-applicable fixes. Arch-agnostic by
//! contract — no architecture may leak in. Producers live in the arch
//! crates (the `.pmc` compiler and lint layer today); renderers live in
//! their CLIs (docs/cli.md (thin-renderer rule)).

/// 1-based line and column; columns count characters, not bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Pos {
    pub line: u32,
    pub col: u32,
}

/// Half-open range: `start` inclusive, `end` exclusive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Span {
    pub start: Pos,
    pub end: Pos,
}

impl Span {
    pub fn new(start_line: u32, start_col: u32, end_line: u32, end_col: u32) -> Span {
        Span {
            start: Pos {
                line: start_line,
                col: start_col,
            },
            end: Pos {
                line: end_line,
                col: end_col,
            },
        }
    }

    /// A single-column span at one position.
    pub fn point(line: u32, col: u32) -> Span {
        Span::new(line, col, line, col + 1)
    }
}

/// Confidence tier of a fix (the rustc suggestion model): plain `--fix`
/// applies only `MachineApplicable`; `MaybeIncorrect` needs `--force`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Applicability {
    MachineApplicable,
    MaybeIncorrect,
}

/// One text edit; an empty `replacement` deletes the span.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edit {
    pub span: Span,
    pub replacement: String,
}

/// A machine-applicable remedy; `edits` apply atomically.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fix {
    pub description: String,
    pub applicability: Applicability,
    pub edits: Vec<Edit>,
}

/// One structured finding. The code is a stable kebab-case rule id
/// (`"unused-label"`); rendering prefixes (`warning:` / `lint:`) are a
/// property of the producing channel, not a field here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub code: &'static str,
    pub span: Span,
    pub message: String,
    pub fix: Option<Fix>,
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p mtc-core diagnostics`
Expected: `3 passed`.

- [ ] **Step 5: Quality gates**

Run: `cargo fmt && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace`
Expected: all green (nothing else references the new module yet).

- [ ] **Step 6: Commit**

```bash
git add crates/core/src/diagnostics.rs crates/core/src/lib.rs
git commit -m "feat(core): diagnostics primitives — Pos/Span/Diagnostic/Fix with applicability tiers"
```

---

### Task 2: Lexer token lengths + sigil adjacency

**Files:**
- Modify: `crates/post-machine/src/lexer.rs`
- Test: unit tests inside `crates/post-machine/src/lexer.rs`

**Interfaces:**
- Consumes: `mtc_core::diagnostics::Span` (Task 1).
- Produces: `Token { kind, line, col, len }` and `Token::span(&self) -> Span`. New lex-time syntax error: whitespace/non-ident after `@`. Plan 2's `leading-zeros` rule slices source by `Token::span()`.

- [ ] **Step 1: Write the failing tests**

Append inside `mod tests` in `crates/post-machine/src/lexer.rs`:

```rust
    #[test]
    fn tokens_carry_char_lengths_and_spans() {
        let tokens = lex("std::api 12 идиВКонец").unwrap();
        // std (len 3) :: (len 2) api (len 3) 12 (len 2) идиВКонец (len 9, chars)
        let lens: Vec<u32> = tokens.iter().map(|t| t.len).collect();
        assert_eq!(lens, vec![3, 2, 3, 2, 9, 0]); // trailing 0 = Eof
        let colon_colon = &tokens[1];
        let s = colon_colon.span();
        assert_eq!((s.start.line, s.start.col, s.end.col), (1, 4, 6));
    }

    #[test]
    fn sigil_must_touch_the_callee_name() {
        for src in [
            "f() { @ qq(); }", // space after @
            "f() { @5(); }",   // digit after @
            "f() { @(); }",    // punctuation after @
            "@",               // trailing @
        ] {
            let e = lex(src).unwrap_err();
            assert!(
                matches!(e.kind, CompileErrorKind::Lex(ref m)
                    if m.contains("immediately after")),
                "{src} should be a lex error about sigil adjacency, got {e:?}"
            );
        }
    }

    #[test]
    fn tight_sigil_still_lexes() {
        let tokens = lex("@qq()").unwrap();
        assert_eq!(tokens[0].kind, TokenKind::At);
        assert_eq!(tokens[1].kind, TokenKind::Ident("qq".into()));
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p mtc-post-machine lexer`
Expected: COMPILE ERROR (`t.len`, `t.span()` don't exist). After stubbing nothing — proceed to Step 3; the sigil test would also FAIL today because `@ qq` lexes fine.

- [ ] **Step 3: Write the implementation**

In `crates/post-machine/src/lexer.rs`:

1. Extend `Token` and add `span()`:

```rust
use mtc_core::diagnostics::Span;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Token {
    pub kind: TokenKind,
    pub line: u32,
    pub col: u32,
    /// Length in characters. Every token is single-line; 0 only for Eof.
    pub len: u32,
}

impl Token {
    /// End-exclusive span of this token's source text.
    pub fn span(&self) -> Span {
        Span::new(self.line, self.col, self.line, self.col + self.len)
    }
}
```

2. In `lex()`, update every push site with a length, and pull `@` out of the `single` match into its own arm placed **before** it (the adjacency check needs lookahead):

```rust
        if c == ':' {
            cur.bump();
            let (kind, len) = if cur.peek() == Some(':') {
                cur.bump();
                (TokenKind::ColonColon, 2)
            } else {
                (TokenKind::Colon, 1)
            };
            tokens.push(Token { kind, line, col, len });
            continue;
        }
        if c == '@' {
            cur.bump();
            // Sigil adjacency (docs/language.md): `@` is part of the
            // callee name's spelling — whitespace, digits, punctuation,
            // comments, or end of input after it are lex errors.
            if !cur.peek().is_some_and(is_ident_start) {
                return Err(err(
                    line,
                    col,
                    "expected a function name immediately after `@`".into(),
                ));
            }
            tokens.push(Token {
                kind: TokenKind::At,
                line,
                col,
                len: 1,
            });
            continue;
        }
        let single = match c {
            '!' => Some(TokenKind::Bang),
            ',' => Some(TokenKind::Comma),
            ';' => Some(TokenKind::Semi),
            '(' => Some(TokenKind::LParen),
            ')' => Some(TokenKind::RParen),
            '{' => Some(TokenKind::LBrace),
            '}' => Some(TokenKind::RBrace),
            _ => None,
        };
        if let Some(kind) = single {
            cur.bump();
            tokens.push(Token {
                kind,
                line,
                col,
                len: 1,
            });
            continue;
        }
```

(Note `'@' => Some(TokenKind::At)` is **removed** from `single`.)

3. Number and identifier sites:

```rust
            tokens.push(Token {
                kind: TokenKind::Number(value),
                line,
                col,
                len: digits.len() as u32, // ASCII digits: bytes == chars
            });
```

```rust
            tokens.push(Token {
                kind: TokenKind::Ident(name.clone()),
                line,
                col,
                len: name.chars().count() as u32, // identifiers are Unicode
            });
```

(For the ident site, compute the length before moving `name` into the token — either clone as above or `let len = name.chars().count() as u32;` first and use `TokenKind::Ident(name)`; prefer the latter, no clone:)

```rust
            let len = name.chars().count() as u32;
            tokens.push(Token {
                kind: TokenKind::Ident(name),
                line,
                col,
                len,
            });
```

4. Eof site:

```rust
    tokens.push(Token {
        kind: TokenKind::Eof,
        line: cur.line,
        col: cur.col,
        len: 0,
    });
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p mtc-post-machine lexer`
Expected: all lexer tests pass, including the three new ones and all pre-existing ones (`lexes_the_shape_of_a_function`, `tracks_line_and_column`, `unicode_identifiers`, `comments_are_skipped`, `colon_colon_is_greedy_and_labels_keep_single_colons`, `error_positions_and_kinds`).

- [ ] **Step 5: Quality gates**

Run: `cargo fmt && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace`
Expected: all green. (Parser tests exercising `@`-calls still pass — `@name` lexes identically when tight. If any parser test intentionally used a spaced sigil, it would fail here; there are none in the current suite.)

- [ ] **Step 6: Commit**

```bash
git add crates/post-machine/src/lexer.rs
git commit -m "feat(post-machine): token lengths + spans; sigil adjacency enforced at lex time"
```

---

### Task 3: CompileError carries a Span

**Files:**
- Modify: `crates/post-machine/src/compiler.rs` (struct, `Display`, `check_duplicate_bindings`, the `Internal` site in `compile()`)
- Modify: `crates/post-machine/src/lexer.rs` (the `err()` helper)
- Modify: `crates/post-machine/src/parser.rs` (`err_at`, inline constructions)
- Modify: `crates/post-machine/src/ir.rs` (`UndefinedLabel` site)
- Modify: `crates/post-machine/src/cli/build.rs` (error formatting)
- Test: existing tests migrate; one new construction-shape test in `compiler.rs`

**Interfaces:**
- Consumes: `Span`/`Span::point` (Task 1), `Token::span()` (Task 2).
- Produces: `CompileError { pub span: Span, pub kind: CompileErrorKind }` — the `line`/`col` fields and the `col == 0` whole-line convention are GONE. Plan 2 and Tasks 4–7 construct errors with this shape only.

- [ ] **Step 1: Change the struct and Display**

In `crates/post-machine/src/compiler.rs` replace the struct and its `Display`:

```rust
use mtc_core::diagnostics::Span;

/// Fatal compile error at a real source span (1-based, char-counted,
/// end-exclusive; see mtc_core::diagnostics).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompileError {
    pub span: Span,
    pub kind: CompileErrorKind,
}
```

```rust
impl std::fmt::Display for CompileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "line {}:{}: {}",
            self.span.start.line, self.span.start.col, self.kind
        )
    }
}
```

- [ ] **Step 2: Fix every construction site**

Run: `cargo build -p mtc-post-machine 2>&1 | head -50` and fix each error. The complete site list:

1. `crates/post-machine/src/lexer.rs` — the `err()` helper (keep its signature so no lexer call site changes):

```rust
fn err(line: u32, col: u32, message: String) -> CompileError {
    CompileError {
        span: mtc_core::diagnostics::Span::point(line, col),
        kind: CompileErrorKind::Lex(message),
    }
}
```

2. `crates/post-machine/src/parser.rs` — `err_at` uses the token's real span:

```rust
    fn err_at(t: &Token, kind: CompileErrorKind) -> CompileError {
        CompileError {
            span: t.span(),
            kind,
        }
    }
```

3. `crates/post-machine/src/parser.rs` — the five remaining inline `CompileError { line: …, col: …, … }` constructions become `err_at` calls or point spans:

   - Duplicate function after `function()` returns (two sites in `top_items`, around the current lines 361–378): the `Function` still exposes `line`/`col` until Task 4, so:

```rust
                return Err(CompileError {
                    span: mtc_core::diagnostics::Span::point(f.line, f.col),
                    kind: CompileErrorKind::DuplicateFunction(f.name),
                });
```

   (same shape for the namespace-pool clash right below it)

   - Nested duplicate in `function()` (current lines 421–427):

```rust
                    return Err(CompileError {
                        span: mtc_core::diagnostics::Span::point(child.line, child.col),
                        kind: CompileErrorKind::DuplicateFunction(child.name),
                    });
```

   - `NestedExport` (current lines 438–443) and `DanglingLabel` (current lines 460–466) both have the offending token at hand — replace the inline construction with `err_at`:

```rust
                let t = self.peek().clone();
                return Err(Self::err_at(&t, CompileErrorKind::NestedExport));
```

```rust
                if let Some(&label) = labels.first() {
                    let t = self.peek().clone();
                    return Err(Self::err_at(
                        &t,
                        CompileErrorKind::DanglingLabel(label),
                    ));
                }
```

4. `crates/post-machine/src/compiler.rs` — `check_duplicate_bindings` (interim span; Task 4 swaps it for the real `import.span`):

```rust
                    return Err(CompileError {
                        span: Span::point(import.line, 1),
                        kind: CompileErrorKind::DuplicateBinding(import.binding().to_string()),
                    });
```

5. `crates/post-machine/src/compiler.rs` — the `Internal` site in `compile()` (no source location exists for a compiler bug):

```rust
        crate::asm::assemble(&pma.text, options.debug_info).map_err(|e| CompileError {
            span: Span::point(0, 0),
            kind: CompileErrorKind::Internal(format!("generated .pma failed to assemble: {e}")),
        })?;
```

6. `crates/post-machine/src/ir.rs` — the `resolve` closure (interim point span; the label token span is not yet threaded):

```rust
    let resolve = |label: u32, line: u32| -> Result<u32, CompileError> {
        label_block.get(&label).copied().ok_or(CompileError {
            span: mtc_core::diagnostics::Span::point(line, 1),
            kind: CompileErrorKind::UndefinedLabel(label),
        })
    };
```

7. `crates/post-machine/src/cli/build.rs` — the compile error rendering:

```rust
    let out = compile_source(&source, options).map_err(|e| {
        format!(
            "{}:{}:{}: error: {}",
            input.display(),
            e.span.start.line,
            e.span.start.col,
            e.kind
        )
    })?;
```

- [ ] **Step 3: Migrate test assertions**

Find every test touching the old fields:

Run: `grep -rn "e\.line\|e\.col\|\.line, e\.\|err\.line\|err\.col" crates/post-machine/src crates/post-machine/tests`

Apply one mechanical rule everywhere: `(e.line, e.col)` → `(e.span.start.line, e.span.start.col)`; a bare `e.line` → `e.span.start.line`. Known sites include `lexer.rs::tests::error_positions_and_kinds` (shown fully migrated here — apply the same shape to each hit):

```rust
    #[test]
    fn error_positions_and_kinds() {
        let e = lex("f() { $ }").unwrap_err();
        assert_eq!((e.span.start.line, e.span.start.col), (1, 7));
        assert!(matches!(e.kind, CompileErrorKind::Lex(ref m) if m.contains('$')));

        let e = lex("/* never closed").unwrap_err();
        assert!(matches!(e.kind, CompileErrorKind::Lex(ref m) if m.contains("unterminated")));

        let e = lex("12abc").unwrap_err();
        assert!(matches!(e.kind, CompileErrorKind::Lex(ref m) if m.contains("digit")));

        let e = lex("99999999999").unwrap_err();
        assert!(matches!(e.kind, CompileErrorKind::Lex(ref m) if m.contains("too large")));
    }
```

Tests that assert a `col == 0` whole-line error (the old convention) now see the interim column `1` — update those expected values to `1` and leave a `// real span lands in Task 4/5` comment ONLY if the value changes again later per this plan; otherwise just fix the number.

- [ ] **Step 4: Run the full suite**

Run: `cargo test --workspace`
Expected: all green — behavior is unchanged, only the error carrier's shape moved.

- [ ] **Step 5: Quality gates**

Run: `cargo fmt && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace`
Expected: all green.

- [ ] **Step 6: Commit**

```bash
git add crates/post-machine crates/core
git commit -m "refactor(post-machine): CompileError carries a Span; col==0 convention removed"
```

---

### Task 4: Parser span retention

**Files:**
- Modify: `crates/post-machine/src/parser.rs` (AST types + parsing)
- Modify: `crates/post-machine/src/ir.rs` (label consumption)
- Modify: `crates/post-machine/src/compiler.rs` (`check_duplicate_bindings` real span; `flatten` pattern-match updates)
- Test: parser unit tests; wide mechanical fixes in `crates/post-machine/tests/compile_programs.rs`, `crates/post-machine/tests/visibility_programs.rs`, ir unit tests

**Interfaces:**
- Consumes: `Token::span()` (Task 2), `CompileError { span, kind }` (Task 3).
- Produces (plan 2 consumes all of these):
  - `pub struct Label { pub value: u32, pub span: Span }` — span = number start → colon END (spans interior whitespace; `1 :` is legal).
  - `Statement { pub labels: Vec<Label>, pub items: Vec<Item>, pub line: u32, pub span: Span }` — span = first label/item token start → `;` end.
  - `Function` gains `pub name_span: Span`.
  - `Import` gains `pub span: Span` (path start → last segment end, alias excluded).
  - `Item::Call { name, name_span: Span, succ, succ_span: Option<Span>, line }` (`succ_span` = the `(`…`)` range incl. parens; always `Some` for calls).
  - `Item::Builtin { which, succ, succ_span: Option<Span>, line }` (`None` when parens absent).
  - `Item::Check { marked, blank, span: Span, line }` (`check` keyword start → `)` end).

- [ ] **Step 1: Write the failing test**

Append inside `mod tests` in `crates/post-machine/src/parser.rs`:

```rust
    #[test]
    fn spans_are_retained_for_labels_names_and_items() {
        let p = parse_src("f() {\n  5 : right(7);\n7:  left;\n}").unwrap();
        let f = &p.functions[0];
        assert_eq!(
            (f.name_span.start.col, f.name_span.end.col),
            (1, 2) // "f" at 1:1, end-exclusive
        );
        let s0 = &f.body[0];
        let label = &s0.labels[0];
        assert_eq!(label.value, 5);
        // "5 : …": number at col 3, colon at col 5 → span 3..6 (spans the gap)
        assert_eq!(
            (label.span.start.col, label.span.end.col),
            (3, 6)
        );
        // statement span: from the label through the `;`
        assert_eq!(s0.span.start.col, 3);
        assert_eq!(s0.span.end.col, 16); // after `;` of "right(7);"
        let Item::Builtin { succ_span, .. } = &s0.items[0] else {
            panic!("expected builtin");
        };
        let ss = succ_span.expect("right(7) has parens");
        assert_eq!((ss.start.col, ss.end.col), (12, 15)); // "(7)"
    }

    #[test]
    fn call_and_check_spans() {
        let p = parse_src("f() { @a::b(); check(1, !); 1: left; }").unwrap();
        let f = &p.functions[0];
        let Item::Call { name, name_span, succ_span, .. } = &f.body[0].items[0] else {
            panic!("expected call");
        };
        assert_eq!(name, "a::b");
        assert_eq!((name_span.start.col, name_span.end.col), (8, 12)); // "a::b"
        assert!(succ_span.is_some()); // "()" always parenthesised
        let Item::Check { span, .. } = &f.body[1].items[0] else {
            panic!("expected check");
        };
        assert_eq!((span.start.col, span.end.col), (16, 27)); // "check(1, !)"
    }

    #[test]
    fn import_spans_exclude_the_alias() {
        let p = parse_src("use std::go as g;\nmain() { @g(); }").unwrap();
        let imp = &p.imports[0];
        assert_eq!((imp.span.start.col, imp.span.end.col), (5, 12)); // "std::go"
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p mtc-post-machine parser`
Expected: COMPILE ERROR — `name_span`, `Label`, `succ_span` etc. don't exist.

- [ ] **Step 3: Change the AST types**

In `crates/post-machine/src/parser.rs`:

```rust
use mtc_core::diagnostics::Span;

/// A label prefix `N:` — the span runs from the number's start to the
/// colon's END, spanning any interior whitespace (spaced `1 :` is legal).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Label {
    pub value: u32,
    pub span: Span,
}
```

```rust
pub struct Statement {
    pub labels: Vec<Label>,
    pub items: Vec<Item>,
    pub line: u32,
    /// First token of the statement (label or item) through the `;` end.
    pub span: Span,
}
```

`Function` gains (keep `line`/`col` — other code and Task-3 spans still read them):

```rust
    pub name_span: Span,
```

`Import` gains:

```rust
    /// Path start → last segment end; an `as` alias is NOT included.
    pub span: Span,
```

`Item` variants:

```rust
pub enum Item {
    Builtin {
        which: Builtin,
        succ: Successor,
        /// The `(`…`)` range including both parens; None without parens.
        succ_span: Option<Span>,
        line: u32,
    },
    Debugger {
        line: u32,
    },
    Call {
        name: String,
        /// Name start → last `::` segment end.
        name_span: Span,
        succ: Successor,
        /// The `(`…`)` range; calls always have parens, so always Some.
        succ_span: Option<Span>,
        line: u32,
    },
    Check {
        marked: CheckArm,
        blank: CheckArm,
        /// `check` keyword start → `)` end.
        span: Span,
        line: u32,
    },
    Halt {
        line: u32,
    },
    Goto {
        label: u32,
        line: u32,
    },
}
```

- [ ] **Step 4: Update the parsing code**

1. `function()` — capture the name span and the label/colon tokens:

```rust
        Ok(Function {
            name,
            line: name_tok.line,
            col: name_tok.col,
            name_span: name_tok.span(),
            body,
            exported: false,
            local: false,
            nested,
            ns: Vec::new(),
        })
```

Label loop (inside `function()`):

```rust
            let mut labels = Vec::new();
            loop {
                let tok = self.peek().clone();
                let TokenKind::Number(n) = tok.kind else {
                    break;
                };
                self.bump();
                let colon = self.peek().clone();
                self.expect(&TokenKind::Colon, "`:` after a label number")?;
                if !seen_labels.insert(n) {
                    return Err(Self::err_at(&tok, CompileErrorKind::DuplicateLabel(n)));
                }
                labels.push(Label {
                    value: n,
                    span: Span {
                        start: tok.span().start,
                        end: colon.span().end,
                    },
                });
            }
```

Dangling-label check right below: `labels.first()` is now a `Label`:

```rust
                if let Some(label) = labels.first() {
                    let t = self.peek().clone();
                    return Err(Self::err_at(
                        &t,
                        CompileErrorKind::DanglingLabel(label.value),
                    ));
                }
```

2. `statement()` — compute the statement span:

```rust
    fn statement(&mut self, labels: Vec<Label>) -> Result<Statement, CompileError> {
        let start = labels
            .first()
            .map(|l| l.span.start)
            .unwrap_or_else(|| self.peek().span().start);
        let line = self.peek().line;
        let mut items = vec![self.item(false)?];
        while matches!(self.peek().kind, TokenKind::Comma) {
            // …the existing group-position match stays byte-identical…
            self.bump();
            items.push(self.item(true)?);
        }
        let semi = self.peek().clone();
        self.expect(&TokenKind::Semi, "`;`")?;
        Ok(Statement {
            labels,
            items,
            line,
            span: Span {
                start,
                end: semi.span().end,
            },
        })
    }
```

3. `item()` — thread spans through calls, checks, builtins. The `@` arm:

```rust
            TokenKind::At => {
                self.bump();
                let name_tok = self.peek().clone();
                let TokenKind::Ident(name) = &name_tok.kind else {
                    return Err(Self::expected(&name_tok, "a function name after `@`"));
                };
                let mut name = name.clone();
                if RESERVED.contains(&name.as_str()) {
                    return Err(Self::err_at(
                        &name_tok,
                        CompileErrorKind::BuiltinCalled(name),
                    ));
                }
                let mut name_end = name_tok.span().end;
                self.bump();
                while matches!(self.peek().kind, TokenKind::ColonColon) {
                    self.bump();
                    let t = self.peek().clone();
                    let TokenKind::Ident(seg) = &t.kind else {
                        return Err(Self::expected(&t, "a name after `::`"));
                    };
                    name.push_str("::");
                    name.push_str(seg);
                    name_end = t.span().end;
                    self.bump();
                }
                let lparen = self.peek().clone();
                self.expect(&TokenKind::LParen, "`(` (user calls are written `@name()`)")?;
                let succ = self.successor()?;
                let rparen = self.peek().clone();
                self.expect(&TokenKind::RParen, "`)`")?;
                Ok(Item::Call {
                    name,
                    name_span: Span {
                        start: name_tok.span().start,
                        end: name_end,
                    },
                    succ,
                    succ_span: Some(Span {
                        start: lparen.span().start,
                        end: rparen.span().end,
                    }),
                    line: tok.line,
                })
            }
```

The `check` arm:

```rust
                "check" => {
                    self.bump();
                    self.expect(&TokenKind::LParen, "`(` after `check`")?;
                    let marked = self.check_arm()?;
                    self.expect(&TokenKind::Comma, "`,` between check arms")?;
                    let blank = self.check_arm()?;
                    let rparen = self.peek().clone();
                    self.expect(&TokenKind::RParen, "`)`")?;
                    Ok(Item::Check {
                        marked,
                        blank,
                        span: Span {
                            start: tok.span().start,
                            end: rparen.span().end,
                        },
                        line: tok.line,
                    })
                }
```

The builtin arm:

```rust
                "left" | "right" | "mark" | "unmark" => {
                    let which = match word.as_str() {
                        "left" => Builtin::Left,
                        "right" => Builtin::Right,
                        "mark" => Builtin::Mark,
                        _ => Builtin::Unmark,
                    };
                    self.bump();
                    let (succ, succ_span) = if matches!(self.peek().kind, TokenKind::LParen) {
                        let lparen = self.peek().clone();
                        self.bump();
                        let succ = self.successor()?;
                        let rparen = self.peek().clone();
                        self.expect(&TokenKind::RParen, "`)`")?;
                        (
                            succ,
                            Some(Span {
                                start: lparen.span().start,
                                end: rparen.span().end,
                            }),
                        )
                    } else {
                        (Successor::FallThrough, None)
                    };
                    Ok(Item::Builtin {
                        which,
                        succ,
                        succ_span,
                        line: tok.line,
                    })
                }
```

4. The `use` loop in `top_items` — track the import span:

```rust
                    let mut path = vec![name.clone()];
                    let path_start = t.span().start;
                    let mut path_end = t.span().end;
                    self.bump();
                    while matches!(self.peek().kind, TokenKind::ColonColon) {
                        self.bump();
                        let t = self.peek().clone();
                        let TokenKind::Ident(seg) = &t.kind else {
                            return Err(Self::expected(&t, "a name after `::`"));
                        };
                        path.push(seg.clone());
                        path_end = t.span().end;
                        self.bump();
                    }
```

and the push:

```rust
                    imports.push(Import {
                        path,
                        alias,
                        line: t.line,
                        ns: ns.to_vec(),
                        span: Span {
                            start: path_start,
                            end: path_end,
                        },
                    });
```

- [ ] **Step 5: Update consumers**

1. `crates/post-machine/src/ir.rs` — labels are `Vec<Label>` now; `IrBlock.labels` stays `Vec<u32>` (the serialized IR JSON must NOT change shape):

```rust
            current = Some(IrBlock {
                id: block_of_stmt[i],
                labels: stmt.labels.iter().map(|l| l.value).collect(),
                line: stmt.line,
                ops: vec![],
                term: IrTerm::Return, // placeholder, always overwritten
                term_line: 0,
            });
```

```rust
    let mut label_block: HashMap<u32, u32> = HashMap::new();
    for (i, stmt) in f.body.iter().enumerate() {
        for l in &stmt.labels {
            label_block.insert(l.value, block_of_stmt[i]);
        }
    }
```

2. `crates/post-machine/src/compiler.rs` — `check_duplicate_bindings` gets the real span (replacing the Task-3 interim):

```rust
                    return Err(CompileError {
                        span: import.span,
                        kind: CompileErrorKind::DuplicateBinding(import.binding().to_string()),
                    });
```

`flatten` pattern matches use `..` and keep compiling: `Item::Call { name, line, .. }` still matches. Verify with the build; no code change expected in `flatten` for this task.

3. Test fixes — run `cargo test --workspace 2>&1 | grep "error\[" | head -30` and fix mechanically with these exact rules:
   - a test comparing `stmt.labels` to `vec![1, 2]` becomes `stmt.labels.iter().map(|l| l.value).collect::<Vec<_>>() == vec![1, 2]`;
   - a test pattern-matching `Item::Call { name, succ, .. }` / `Item::Builtin { which, succ, .. }` / `Item::Check { marked, blank, .. }` is untouched (the `..` absorbs new fields); patterns WITHOUT `..` gain it;
   - no test outside `parser.rs` constructs these types literally (the parser is the only producer), so construction fixes stay inside `parser.rs` tests if any exist.

  Affected files to sweep: `crates/post-machine/src/parser.rs` (tests), `crates/post-machine/src/ir.rs` (tests), `crates/post-machine/tests/compile_programs.rs`, `crates/post-machine/tests/visibility_programs.rs`.

- [ ] **Step 6: Run the full suite**

Run: `cargo test --workspace`
Expected: all green, including the three new span tests from Step 1 and the untouched golden suite.

- [ ] **Step 7: Quality gates + commit**

```bash
cargo fmt && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace
git add crates/post-machine
git commit -m "refactor(post-machine): parser retains label/name/import/item spans"
```

---

### Task 5: Warning → Diagnostic

**Files:**
- Modify: `crates/post-machine/src/compiler.rs` (delete `Warning`, migrate `CompileReport` + `flatten`)
- Modify: `crates/post-machine/src/ir.rs` (`lower` signature + unreachable-code diagnostic)
- Modify: `crates/post-machine/src/lib.rs` (drop the `Warning` re-export)
- Modify: `crates/post-machine/src/cli/build.rs` (`render_warnings`, `-Werror`)
- Test: migrate warning tests in `compiler.rs`, `ir.rs`, `tests/compile_programs.rs`, `tests/visibility_programs.rs`, `tests/cli_programs.rs`, `tests/golden_programs.rs`, `tests/stdlib_programs.rs`

**Interfaces:**
- Consumes: `Diagnostic` (Task 1), parser spans (`Item::Call::name_span`, `Import::span`, `Function::name_span`, `Statement::span`) (Task 4).
- Produces: `CompileReport { pub diagnostics: Vec<Diagnostic>, pub opt: OptReport }`; `Warning` no longer exists anywhere; warning codes `undeclared-external`, `unused-import`, `unused-function`, `unreachable-code`; `flatten(program) -> (Program, Vec<Diagnostic>)` (Task 6 widens this); `lower(&Program) -> Result<(IrProgram, Vec<Diagnostic>), CompileError>`.

- [ ] **Step 1: Write the failing test**

In `crates/post-machine/src/compiler.rs` tests, add:

```rust
    #[test]
    fn compile_warnings_carry_codes_and_spans() {
        let src = "use std::go;\nmain() { right; }\nhelper() { left; }\n";
        let out = compile(src, CompileOptions::default()).unwrap();
        let codes: Vec<&str> = out.report.diagnostics.iter().map(|d| d.code).collect();
        assert!(codes.contains(&"unused-import"));
        assert!(codes.contains(&"unused-function"));
        let unused_import = out
            .report
            .diagnostics
            .iter()
            .find(|d| d.code == "unused-import")
            .unwrap();
        // "std::go" on line 1, cols 5..12
        assert_eq!(
            (unused_import.span.start.line, unused_import.span.start.col),
            (1, 5)
        );
        assert!(out.report.diagnostics.iter().all(|d| d.fix.is_none()));
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p mtc-post-machine compile_warnings_carry_codes_and_spans`
Expected: COMPILE ERROR — `report.diagnostics` doesn't exist yet.

- [ ] **Step 3: Migrate the library**

1. `crates/post-machine/src/compiler.rs`:
   - **Delete** the `Warning` struct entirely.
   - Import `use mtc_core::diagnostics::{Diagnostic, Span};`
   - `CompileReport`:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompileReport {
    pub diagnostics: Vec<Diagnostic>,
    pub opt: OptReport,
}
```

   - `flatten` signature and `Ctx`:

```rust
fn flatten(program: crate::parser::Program) -> (crate::parser::Program, Vec<Diagnostic>) {
```

```rust
    struct Ctx {
        defs: HashMap<Vec<String>, HashMap<String, String>>,
        bindings: HashMap<Vec<String>, HashMap<String, (usize, String)>>,
        imports_used: Vec<bool>,
        warned_undeclared: HashSet<String>,
        warnings: Vec<Diagnostic>,
    }
```

   - `resolve` takes the call's name span instead of a line:

```rust
    fn resolve(
        name: &mut String,
        span: mtc_core::diagnostics::Span,
        nested: &[HashMap<String, String>],
        ns: &[String],
        ctx: &mut Ctx,
    ) {
        // …unchanged resolution logic…
        if ctx.warned_undeclared.insert(name.clone()) {
            ctx.warnings.push(Diagnostic {
                code: "undeclared-external",
                span,
                message: format!(
                    "call to undeclared external `{name}` — declare it with `use {name};`"
                ),
                fix: None,
            });
        }
    }
```

   and the call site inside `emit`:

```rust
        for stmt in &mut f.body {
            for item in &mut stmt.items {
                if let Item::Call { name, name_span, .. } = item {
                    resolve(name, *name_span, &inner, ns, ctx);
                }
            }
        }
```

   - Unused imports and functions:

```rust
    for (i, imp) in imports.iter().enumerate() {
        if !ctx.imports_used[i] {
            warnings.push(Diagnostic {
                code: "unused-import",
                span: imp.span,
                message: format!("unused import `{}`", imp.full_path()),
                fix: None,
            });
        }
    }
```

```rust
    for f in &out {
        if !reached.contains(f.name.as_str()) {
            warnings.push(Diagnostic {
                code: "unused-function",
                span: f.name_span,
                message: format!("unused function `{}` (not exported, never called)", f.name),
                fix: None,
            });
        }
    }
```

   - `compile()`:

```rust
    let (program, vis) = flatten(parsed);
    let (mut ir, mut diagnostics) = crate::ir::lower(&program)?;
    diagnostics.extend(vis);
```

   and the report construction: `report: CompileReport { diagnostics, opt }`.

2. `crates/post-machine/src/ir.rs`:
   - Replace `use crate::compiler::{CompileError, CompileErrorKind, Warning};` with `use crate::compiler::{CompileError, CompileErrorKind};` and add `use mtc_core::diagnostics::{Diagnostic, Span};`
   - `lower` and `lower_function` signatures speak `Diagnostic`:

```rust
pub fn lower(program: &Program) -> Result<(IrProgram, Vec<Diagnostic>), CompileError> {
```

```rust
fn lower_function(
    f: &crate::parser::Function,
    warnings: &mut Vec<Diagnostic>,
) -> Result<IrFunction, CompileError> {
```

   - The unreachable-code diagnostic needs the block's first-statement SPAN, but `IrBlock` must not change shape (the IR JSON is a versioned artifact). Keep a side map while building blocks:

```rust
    let mut blocks: Vec<IrBlock> = Vec::new();
    let mut block_spans: HashMap<u32, Span> = HashMap::new();
    let mut current: Option<IrBlock> = None;

    for (i, stmt) in f.body.iter().enumerate() {
        if starts[i] {
            debug_assert!(current.is_none(), "predecessor closed the block");
            block_spans.insert(block_of_stmt[i], stmt.span);
            current = Some(IrBlock {
                // …unchanged from Task 4…
```

   and the warning loop:

```rust
    for b in &blocks {
        if !seen.contains(&b.id) && b.line != 0 {
            warnings.push(Diagnostic {
                code: "unreachable-code",
                span: block_spans[&b.id],
                message: format!("unreachable code in `{}`", f.name),
                fix: None,
            });
        }
    }
```

3. `crates/post-machine/src/lib.rs` — drop `Warning` from the re-export:

```rust
pub use compiler::{
    CompileError, CompileErrorKind, CompileOptions, CompileOutput, CompileReport, compile,
};
```

4. `crates/post-machine/src/cli/build.rs`:

```rust
fn render_warnings(stderr: &mut String, input: &Path, report: &CompileReport) {
    for d in &report.diagnostics {
        let _ = writeln!(
            stderr,
            "{}:{}:{}: warning: {}",
            input.display(),
            d.span.start.line,
            d.span.start.col,
            d.message
        );
    }
}
```

```rust
    if werror && !out.report.diagnostics.is_empty() {
        return Err(format!(
            "{stderr}-Werror: {} warning(s) treated as errors",
            out.report.diagnostics.len()
        ));
    }
```

- [ ] **Step 4: Migrate the tests**

Mechanical rules (apply to every hit of `grep -rn "\.warnings\|Warning" crates/post-machine`):
- `out.report.warnings` → `out.report.diagnostics`;
- assertions by message substring move to codes: `w.message.contains("unused import")` → `d.code == "unused-import"`; `contains("unused function")` → `"unused-function"`; `contains("undeclared external")` → `"undeclared-external"`; `contains("unreachable")` → `"unreachable-code"` (keep one message-content assertion per code somewhere so message wording stays covered — the existing texts are unchanged);
- `w.line == N` → `d.span.start.line == N`;
- `assert!(out.report.warnings.is_empty())` in `tests/golden_programs.rs` and `tests/stdlib_programs.rs` → `assert!(out.report.diagnostics.is_empty())`;
- the `-Werror` CLI test in `tests/cli_programs.rs` (`werror_fails_on_warnings`) keeps asserting stderr contains `"warning"` and the error contains `"-Werror"` — both survive unchanged.

Known unit-test sites in `compiler.rs`: `warnings_flow_into_the_report`, `undeclared_external_warns_once…`, `unused_imports_and_unused_functions_warn`, `uncalled_nested_functions_warn…`; in `ir.rs`: `unreachable_code_warns_with_its_line` (its line assertion becomes `d.span.start.line`).

- [ ] **Step 5: Run the full suite**

Run: `cargo test --workspace`
Expected: all green, including the Step-1 test and the goldens (compile OUTPUT is unchanged; only the report shape moved).

- [ ] **Step 6: Quality gates + commit**

```bash
cargo fmt && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace
git add crates/post-machine
git commit -m "refactor(post-machine): Warning replaced by span-carrying coded Diagnostics"
```

---

### Task 6: The analyze() split

**Files:**
- Modify: `crates/post-machine/src/compiler.rs`
- Test: unit tests in `compiler.rs`; the golden suite guards `-O0` bit-identity

**Interfaces:**
- Consumes: everything above.
- Produces (plan 2's `lint()` consumes these — same crate, `pub(crate)`):

```rust
pub(crate) struct ScopeSummary {
    pub defs: HashMap<Vec<String>, HashMap<String, String>>,
    pub bindings: HashMap<Vec<String>, HashMap<String, (usize, String)>>,
}
pub(crate) struct AnalysisOutput {
    pub tokens: Vec<Token>,
    pub ast: Program,      // the FLATTENED program (mangled names)
    pub ir: IrProgram,     // unoptimized
    pub diagnostics: Vec<Diagnostic>,
    pub scopes: ScopeSummary,
}
pub(crate) fn analyze(source: &str) -> Result<AnalysisOutput, CompileError>
```

- [ ] **Step 1: Write the failing test**

In `crates/post-machine/src/compiler.rs` tests:

```rust
    #[test]
    fn analyze_stops_before_the_optimizer_and_keeps_the_raw_material() {
        let src = "use std::go;\nmain() { right; }\n";
        let a = analyze(src).unwrap();
        assert!(!a.tokens.is_empty());
        assert_eq!(a.ir.functions.len(), 1);
        assert!(a.diagnostics.iter().any(|d| d.code == "unused-import"));
        // flatten's scope summary is retained, not discarded:
        assert!(a.scopes.defs.contains_key(&Vec::<String>::new()));
        assert!(a.scopes.bindings.contains_key(&Vec::<String>::new()));
        // compile() reports exactly what analyze() found:
        let out = compile(src, CompileOptions::default()).unwrap();
        assert_eq!(out.report.diagnostics, a.diagnostics);
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p mtc-post-machine analyze_stops_before`
Expected: COMPILE ERROR — `analyze` not defined.

- [ ] **Step 3: Implement the split**

In `crates/post-machine/src/compiler.rs`:

```rust
use std::collections::HashMap;

use crate::lexer::Token;
use crate::parser::Program;

/// flatten's per-scope name maps, retained for scope-aware lint rules
/// instead of being discarded.
pub(crate) struct ScopeSummary {
    /// ns path -> (bare name -> full mangled name)
    pub defs: HashMap<Vec<String>, HashMap<String, String>>,
    /// ns path -> (bare name -> (import index, full `::` path))
    pub bindings: HashMap<Vec<String>, HashMap<String, (usize, String)>>,
}

/// The codegen-free front half of the pipeline: everything the lint
/// layer (and a future LSP) needs, nothing it doesn't.
pub(crate) struct AnalysisOutput {
    pub tokens: Vec<Token>,
    pub ast: Program,
    pub ir: IrProgram,
    pub diagnostics: Vec<Diagnostic>,
    pub scopes: ScopeSummary,
}

/// lex → parse → duplicate-binding check → flatten → lower. Stops before
/// the optimizer; `compile()` composes this with the back half.
pub(crate) fn analyze(source: &str) -> Result<AnalysisOutput, CompileError> {
    let tokens = crate::lexer::lex(source)?;
    let parsed = crate::parser::parse(&tokens)?;
    check_duplicate_bindings(&parsed)?;
    let (program, scopes, vis) = flatten(parsed);
    let (ir, mut diagnostics) = crate::ir::lower(&program)?;
    diagnostics.extend(vis);
    Ok(AnalysisOutput {
        tokens,
        ast: program,
        ir,
        diagnostics,
        scopes,
    })
}
```

`flatten` returns the summary — change its signature and tail:

```rust
fn flatten(
    program: crate::parser::Program,
) -> (crate::parser::Program, ScopeSummary, Vec<Diagnostic>) {
```

At the end of `flatten`, destructure `Ctx` instead of field-reading it (the maps move out):

```rust
    let Ctx {
        defs,
        bindings,
        imports_used,
        warnings: mut warnings_out,
        ..
    } = ctx;
    let mut warnings = warnings_out;

    // Unused imports: none of the import's bindings resolved any call.
    for (i, imp) in imports.iter().enumerate() {
        if !imports_used[i] {
            warnings.push(Diagnostic {
                code: "unused-import",
                span: imp.span,
                message: format!("unused import `{}`", imp.full_path()),
                fix: None,
            });
        }
    }

    // …the unused-function reachability block stays as in Task 5…

    (
        Program {
            functions: out,
            imports,
        },
        ScopeSummary { defs, bindings },
        warnings,
    )
}
```

(Collapse `let mut warnings = warnings_out;` into the destructure — `warnings: mut warnings` — when writing the real code; clippy will flag the redundant rebinding.)

`compile()` becomes a composition:

```rust
pub fn compile(source: &str, options: CompileOptions) -> Result<CompileOutput, CompileError> {
    let analysis = analyze(source)?;
    let AnalysisOutput {
        mut ir,
        diagnostics,
        ..
    } = analysis;
    let mut ir_snapshots = Vec::new();
    if options.capture_ir {
        ir_snapshots.push(("lowered".to_string(), ir.clone()));
    }
    let opt = optimize(
        &mut ir,
        &OptOptions {
            level: options.opt_level,
            disabled: options.disabled_passes.iter().cloned().collect(),
            capture: options.capture_ir,
        },
        &mut ir_snapshots,
    );
    if options.capture_ir {
        ir_snapshots.push(("final".to_string(), ir.clone()));
    }
    let pma = emit_program(
        &ir,
        CodegenOptions {
            strip_debugger: options.strip_debugger,
        },
    );
    let mut object =
        crate::asm::assemble(&pma.text, options.debug_info).map_err(|e| CompileError {
            span: Span::point(0, 0),
            kind: CompileErrorKind::Internal(format!("generated .pma failed to assemble: {e}")),
        })?;
    if options.debug_info {
        remap_debug_lines(&mut object, &pma.line_map);
    }
    Ok(CompileOutput {
        object,
        pma: pma.text,
        ir,
        ir_snapshots,
        report: CompileReport { diagnostics, opt },
    })
}
```

- [ ] **Step 4: Run the full suite (bit-identity is the point)**

Run: `cargo test --workspace`
Expected: all green. Pay special attention to:
`cargo test -p mtc-post-machine --test golden_programs`
Expected: PASS — proof that recomposing `compile()` changed no output byte.

- [ ] **Step 5: Quality gates + commit**

```bash
cargo fmt && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace
git add crates/post-machine
git commit -m "refactor(post-machine): compile() splits into a codegen-free analyze() front half"
```

---

### Task 7: Reserved-word path guard + error-message pack

**Files:**
- Modify: `crates/post-machine/src/parser.rs`
- Modify: `crates/post-machine/src/compiler.rs` (`CompileErrorKind` variants + `Display`)
- Test: parser unit tests (one failing-test-first cycle per item)

**Interfaces:**
- Consumes: Tasks 2–4 shapes.
- Produces: new/reshaped `CompileErrorKind` variants — `ReservedName { name: String, what: &'static str }` (replaces `ReservedFunctionName`), `DuplicateName { name: String, what: &'static str }` (replaces `DuplicateFunction`), `KeywordNeedsName(&'static str)`, `KeywordInBody(&'static str)`, `SingleColonInPath`, `TopLevelStatement(String)`. Grammar change: reserved words rejected in EVERY `::` path segment (calls and `use`). Also `pub const PMC_LANG_VERSION: &str = "0.2";` in `parser.rs` (re-exported from `lib.rs`) — the language acceptance-contract version (pre-1.0: 0.N bumps on any grammar change; major/minor axes activate at a declared 1.0; no patch digit; this round's tightenings ARE the 0.1 → 0.2 change), surfaced as the second line of `pmt --version`.

- [ ] **Step 1: Write the failing tests (whole pack + guard + stays-legal)**

Append inside `mod tests` in `crates/post-machine/src/parser.rs`:

```rust
    fn err_msg(src: &str) -> String {
        parse_src(src).unwrap_err().to_string()
    }

    #[test]
    fn reserved_words_are_barred_in_every_path_segment() {
        let m = err_msg("main() { @std::goto(); }");
        assert!(m.contains("reserved word"), "got: {m}");
        let m = err_msg("use std::goto;\nmain() { right; }");
        assert!(m.contains("reserved word"), "got: {m}");
    }

    #[test]
    fn keyword_followed_by_brace_gets_a_hint() {
        let m = err_msg("namespace {\n}");
        assert!(m.contains("did you mean `namespace <name> { … }`"), "got: {m}");
        let m = err_msg("use {}");
        assert!(m.contains("did you mean `use <name>;`"), "got: {m}");
        let m = err_msg("export {}");
        assert!(m.contains("did you mean `export <name>() { … }`"), "got: {m}");
    }

    #[test]
    fn use_and_namespace_inside_a_body_say_the_real_rule() {
        let m = err_msg("main() { use go; }");
        assert!(m.contains("not allowed inside a function body"), "got: {m}");
        let m = err_msg("main() { namespace x; }");
        assert!(m.contains("not allowed inside a function body"), "got: {m}");
    }

    #[test]
    fn single_colon_in_a_path_hints_double_colon() {
        let m = err_msg("use std:b;\nmain() { right; }");
        assert!(m.contains("did you mean `::`"), "got: {m}");
        let m = err_msg("main() { @f:g(); }");
        assert!(m.contains("did you mean `::`"), "got: {m}");
    }

    #[test]
    fn namespace_naming_errors_say_namespace() {
        let m = err_msg("namespace goto { }");
        assert!(m.contains("namespace"), "got: {m}");
        let m = err_msg("namespace a { } a() { right; }");
        assert!(m.contains("namespace"), "got: {m}");
    }

    #[test]
    fn unclosed_function_body_mentions_the_brace() {
        let m = err_msg("f() { left;");
        assert!(m.contains("`}` to close the function body"), "got: {m}");
    }

    #[test]
    fn top_level_statements_state_the_rule() {
        for src in ["left;\nmain() { right; }", "goto 1;", "@foo();"] {
            let m = err_msg(src);
            assert!(
                m.contains("not allowed at top level"),
                "{src} got: {m}"
            );
        }
    }

    #[test]
    fn spaced_label_colons_and_paths_stay_legal() {
        assert!(parse_src("main() { 1 : right; }").is_ok());
        assert!(parse_src("main() { 1: 2: right; }").is_ok());
        assert!(parse_src("use std :: goToEnd;\nmain() { @goToEnd(); }").is_ok());
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p mtc-post-machine parser`
Expected: the eight new tests FAIL (wrong messages / missing errors); `spaced_label_colons_and_paths_stay_legal` PASSES already (it pins the reverted rulings).

- [ ] **Step 3: Add and reshape the error kinds**

In `crates/post-machine/src/compiler.rs`, replace `ReservedFunctionName(String)` and `DuplicateFunction(String)` and add the pack variants:

```rust
    /// A reserved word used to name something (`what`: "function",
    /// "namespace", "path segment").
    ReservedName {
        name: String,
        what: &'static str,
    },
    /// A name already taken in this scope (`what` names the EXISTING
    /// entity: "function" or "namespace").
    DuplicateName {
        name: String,
        what: &'static str,
    },
    /// `namespace {` / `use {` / `export {` — keyword with no name.
    KeywordNeedsName(&'static str),
    /// `use` / `namespace` inside a function body.
    KeywordInBody(&'static str),
    /// A single `:` where a `::` path separator was meant.
    SingleColonInPath,
    /// A command or call at top level (outside any function body).
    TopLevelStatement(String),
```

and the `Display` arms:

```rust
            CompileErrorKind::ReservedName { name, what } => {
                write!(f, "`{name}` is a reserved word and cannot name a {what}")
            }
            CompileErrorKind::DuplicateName { name, what } => {
                write!(
                    f,
                    "duplicate name `{name}` — already used by a {what} in this scope"
                )
            }
            CompileErrorKind::KeywordNeedsName(kw) => match *kw {
                "use" => write!(f, "`use` needs a name — did you mean `use <name>;`?"),
                "export" => write!(
                    f,
                    "`export` needs a name — did you mean `export <name>() {{ … }}`?"
                ),
                _ => write!(
                    f,
                    "`namespace` needs a name — did you mean `namespace <name> {{ … }}`?"
                ),
            },
            CompileErrorKind::KeywordInBody(kw) => {
                write!(
                    f,
                    "`{kw}` is not allowed inside a function body — imports and namespaces live at file or namespace level"
                )
            }
            CompileErrorKind::SingleColonInPath => {
                write!(f, "single `:` in a name path — did you mean `::`?")
            }
            CompileErrorKind::TopLevelStatement(found) => {
                write!(
                    f,
                    "statements are not allowed at top level — commands and calls live inside function bodies (found {found})"
                )
            }
```

Delete the old `ReservedFunctionName` / `DuplicateFunction` variants and their `Display` arms.

- [ ] **Step 4: Wire the parser**

In `crates/post-machine/src/parser.rs`:

1. **Reshape call sites.** Every `ReservedFunctionName(name)` becomes `ReservedName { name, what: "function" }` — EXCEPT the namespace-name check in `top_items` (currently around line 322), which becomes `what: "namespace"`... note the reserved word here names a namespace:

```rust
                if RESERVED.contains(&name.as_str()) {
                    return Err(Self::err_at(
                        &name_tok,
                        CompileErrorKind::ReservedName {
                            name,
                            what: "namespace",
                        },
                    ));
                }
```

   Every `DuplicateFunction(name)` becomes `DuplicateName { name, what: … }` where `what` names the EXISTING entity:
   - function-vs-function duplicate (both in `top_items` after `function()` and the nested duplicate in `function()`): `what: "function"`;
   - namespace reusing a function's name (in the namespace branch): `what: "function"`;
   - function reusing a namespace's name (the `as_ns` check): `what: "namespace"`.

2. **Path guard** — in the `@`-call `::` loop (Task 4's version):

```rust
                while matches!(self.peek().kind, TokenKind::ColonColon) {
                    self.bump();
                    let t = self.peek().clone();
                    let TokenKind::Ident(seg) = &t.kind else {
                        return Err(Self::expected(&t, "a name after `::`"));
                    };
                    if RESERVED.contains(&seg.as_str()) {
                        return Err(Self::err_at(
                            &t,
                            CompileErrorKind::ReservedName {
                                name: seg.clone(),
                                what: "path segment",
                            },
                        ));
                    }
                    name.push_str("::");
                    name.push_str(seg);
                    name_end = t.span().end;
                    self.bump();
                }
```

   and identically in the `use`-path `::` loop in `top_items` (after reading `seg`, before `path.push`).

3. **KeywordNeedsName** — at the top of the `top_items` loop body, after the terminator match, BEFORE the `use` branch:

```rust
            // `namespace {` / `use {` / `export {`: the contextual keyword
            // has no name; without this check it parses as a function
            // named `namespace` and the error blames the `{`.
            if let TokenKind::Ident(w) = &t.kind
                && matches!(w.as_str(), "namespace" | "use" | "export")
                && matches!(
                    self.tokens.get(self.pos + 1).map(|t| &t.kind),
                    Some(TokenKind::LBrace)
                )
            {
                let kw: &'static str = match w.as_str() {
                    "use" => "use",
                    "export" => "export",
                    _ => "namespace",
                };
                return Err(Self::err_at(&t, CompileErrorKind::KeywordNeedsName(kw)));
            }
```

4. **TopLevelStatement** — immediately after the KeywordNeedsName check (still before the `use` branch):

```rust
            // A command or call at top level: `left;`, `goto 1;`, `@f();`.
            // Without this, reserved words blame naming rules and `@`
            // blames a missing function name.
            let top_level_stmt = match &t.kind {
                TokenKind::At => true,
                TokenKind::Ident(w) => RESERVED.contains(&w.as_str()),
                _ => false,
            };
            if top_level_stmt {
                return Err(Self::err_at(
                    &t,
                    CompileErrorKind::TopLevelStatement(describe(&t.kind)),
                ));
            }
```

   (Note: this makes the `function()`-level `ReservedName` check unreachable from top level, but it still fires for nested definitions inside bodies — keep it.)

5. **KeywordInBody** — in `item()`'s `Ident` match, add arms before the `other` fallback:

```rust
                "use" => Err(Self::err_at(&tok, CompileErrorKind::KeywordInBody("use"))),
                "namespace" => Err(Self::err_at(
                    &tok,
                    CompileErrorKind::KeywordInBody("namespace"),
                )),
```

6. **SingleColonInPath** — two hooks. In the `use` loop, extend the separator match:

```rust
                    let sep = self.peek().clone();
                    match sep.kind {
                        TokenKind::Comma => {
                            self.bump();
                        }
                        TokenKind::Semi => {
                            self.bump();
                            break;
                        }
                        TokenKind::Colon => {
                            return Err(Self::err_at(&sep, CompileErrorKind::SingleColonInPath));
                        }
                        _ => return Err(Self::expected(&sep, "`,` or `;`")),
                    }
```

   In the `@`-call arm, after the `::` loop and before `expect(LParen…)`:

```rust
                if matches!(self.peek().kind, TokenKind::Colon) {
                    let t = self.peek().clone();
                    return Err(Self::err_at(&t, CompileErrorKind::SingleColonInPath));
                }
```

7. **Unclosed function body** — at the very top of `function()`'s body loop:

```rust
        loop {
            if matches!(self.peek().kind, TokenKind::Eof) {
                return Err(Self::expected(
                    self.peek(),
                    "`}` to close the function body",
                ));
            }
```

- [ ] **Step 5: Fix knock-on test assertions**

The variant reshape changes some existing messages. Sweep with:
`grep -rn "ReservedFunctionName\|DuplicateFunction\|reserved word and cannot name a function\|duplicate function" crates/post-machine`
- pattern matches on the old variants → new variants (`CompileErrorKind::ReservedName { .. }`, `CompileErrorKind::DuplicateName { .. }`);
- message assertions `"duplicate function"` → `"duplicate name"`;
- the top-level `left;`-style inputs in older tests (if any asserted `ReservedFunctionName`) now yield `TopLevelStatement` — update those expectations.

- [ ] **Step 6: Surface the language version (failing test first)**

The tightenings change the set of accepted programs — per the spec's grammar-versioning ruling that makes the `.pmc` language **0.2** (the v1 grammar is retroactively 0.1). The version is surfaced, not enforced: a constant, a `pmt --version` line, a docs header (Task 8). Scheme: pre-1.0, the version is 0.N and N bumps on ANY grammar change; major/minor axes (breaking/additive) activate at a future declared 1.0; NO patch digit.

Add to `crates/post-machine/tests/cli_programs.rs`:

```rust
#[test]
fn version_reports_the_language_version() {
    let out = execute(&args(&["--version"])).unwrap();
    assert!(out.stdout.contains(&format!(
        "pmc language {}",
        mtc_post_machine::PMC_LANG_VERSION
    )));
    assert_eq!(mtc_post_machine::PMC_LANG_VERSION, "0.2");
}
```

Run: `cargo test -p mtc-post-machine --test cli_programs version_reports`
Expected: COMPILE ERROR (`PMC_LANG_VERSION` not defined).

Implement — in `crates/post-machine/src/parser.rs`, next to `RESERVED` (the grammar's home; the constant versions the ACCEPTANCE CONTRACT, not a serialization format — contrast `IR_VERSION`):

```rust
/// The `.pmc` language acceptance-contract version (docs/language.md):
/// pre-1.0 the version is 0.N and N bumps on ANY grammar change; at a
/// declared 1.0 the axes activate (major = breaking, minor = additive).
/// No patch digit — spec-text corrections are errata;
/// implementation-conformance fixes live in the crate changelog. The
/// sigil-adjacency and reserved-path tightenings made this 0.2 (the v1
/// grammar is retroactively 0.1).
pub const PMC_LANG_VERSION: &str = "0.2";
```

Re-export in `crates/post-machine/src/lib.rs`:

```rust
pub use parser::PMC_LANG_VERSION;
```

And in `crates/post-machine/src/cli/mod.rs`, the `--version` arm:

```rust
        Some("--version") => Ok(CliOutput::ok(
            format!(
                "pmt {}\npmc language {}\n",
                env!("CARGO_PKG_VERSION"),
                crate::parser::PMC_LANG_VERSION
            ),
            String::new(),
        )),
```

Run: `cargo test -p mtc-post-machine --test cli_programs version_reports`
Expected: PASS. (If another test asserts the exact full `--version` output, update it to expect the two-line form.)

- [ ] **Step 7: Run the full suite**

Run: `cargo test --workspace`
Expected: all green — the eight new tests pass, stays-legal pins hold, goldens untouched.

- [ ] **Step 8: Quality gates + commit**

```bash
cargo fmt && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace
git add crates/post-machine
git commit -m "feat(post-machine): reserved-word path guard, error-message pack, language version 0.2"
```

---

### Task 8: Foundation docs

**Files:**
- Modify: `docs/language.md`
- Modify: `docs/cli.md`

**Interfaces:**
- Consumes: the shipped behavior of Tasks 2 and 7.
- Produces: user-facing documentation; no code.

- [ ] **Step 1: docs/language.md — language version header + grammar tightenings**

At the top of the document, add the language-version line per the grammar-versioning ruling (e.g. directly under the title): `The .pmc language version is 0.2 (pre-1.0: the version is 0.N and N bumps on any grammar change; at a declared 1.0 the axes activate — major = breaking acceptance change, minor = additive syntax; no patch digit — spec-text corrections are errata, implementation fixes live in the crate changelog). The v1 toolchain's grammar is retroactively 0.1; the adjacency and path-segment tightenings below are the 0.2 change.`

In the grammar/notes section (near the reserved-words bullet):

- Document sigil adjacency: `@` must be immediately followed by the function name — `@ qq()` is a syntax error; the sigil is part of the name's spelling. State explicitly that spaced label colons (`1 : right;`) and spaced paths (`std :: goToEnd`) remain legal.
- Document the path guard: reserved words cannot appear in ANY `::` path segment (they were already barred as head names); `@std::goto()` is a syntax error because such a symbol is undefinable from `.pmc` source.

- [ ] **Step 2: docs/language.md — accuracy batch**

Apply the seven corrections (forge-agnostic prose, no tracker refs):

1. Duplicate `use`: soften "two imports binding the same bare name in the same scope are an error" — an EXACTLY identical `use` line (same path and alias) is tolerated and surfaces as an unused-import warning instead.
2. Fix the "every command takes an optional successor in parentheses" overstatement: only the four tape builtins and `@`-calls take one; `goto`, `check`, `halt`, `debugger` do not (the statement table is already accurate — align the prose to it).
3. Document that a function body may be empty (`f() { }` compiles to an immediate return).
4. Document stacked labels (`1: 2: left;` — several labels may name one statement).
5. Document comment edge cases: block comments do not nest; a lone `/` is a lex error; an unterminated block comment is a lex error.
6. State `goto`'s total exclusion from comma groups (not just "last position" — it may not appear in a group at all).
7. State that `export main()` at top level is a redundant no-op (`main` always exports), and that a `main` inside a namespace is NOT the program entry and is not auto-exported.

- [ ] **Step 3: docs/cli.md — warning format + --version**

Update the compile-warnings note: warnings now render as `FILE:LINE:COL: warning: MESSAGE` (the column is new). `-Werror` semantics unchanged. Document that `pmt --version` now prints a second line, `pmc language <VERSION>` — the language acceptance-contract version, independent of the crate version.

- [ ] **Step 4: Verify and commit**

Run: `cargo test --workspace` (docs only — sanity) and re-read both diffs for forge-agnostic wording (no issue numbers, no URLs).

```bash
git add docs/language.md docs/cli.md
git commit -m "docs(language): sigil adjacency + path guard; grammar accuracy batch; warning column in cli.md"
```

---

## Plan self-review (done at authoring time)

- **Spec coverage (foundation scope):** §1 primitives → Task 1; token ends + spans → Task 2; `CompileError` span migration (col==0 dies) → Task 3; parser span retention incl. label span over interior whitespace → Task 4; `Warning` deletion, 4 coded warnings, renderer/-Werror → Task 5; `analyze()` split + `ScopeSummary` + token retention + `-O0` bit-identity → Task 6; two grammar tightenings + six-fix pack + stays-legal pins + `PMC_LANG_VERSION` 0.2 surface (constant, `--version` line) → Task 7 (sigil tightening itself in Task 2); §6 language.md/cli.md foundation share + language-version header → Task 8. Acceptance criteria 2, 3, 5 are covered here; criteria 1 and 4 belong to plan 2.
- **Placeholders:** none — every step carries code, commands, or an exact mechanical rule with a grep.
- **Type consistency:** `Label { value, span }`, `name_span`, `succ_span`, `Item::Check::span`, `CompileReport::diagnostics`, `AnalysisOutput { tokens, ast, ir, diagnostics, scopes }`, and the four warning codes are spelled identically in every task and in the header interface list plan 2 consumes.
