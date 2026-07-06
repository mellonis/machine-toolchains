# .pmc Lint Layer Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship `pmt lint` — a 10-rule hygiene linter for `.pmc` with tiered `--fix`, batch directory walking, and its docs — per the approved spec `docs/superpowers/specs/2026-07-06-pmc-lint-layer-design.md`.

**Architecture:** A `lint` module in `mtc-post-machine` runs the crate-private `analyze()` entrypoint (lexer → parser → flatten → lower, no codegen), then applies a const table of rule functions over a `LintContext { source, tokens, ast, ir, scopes }`. Findings are `mtc_core::diagnostics::Diagnostic` values (code + span + message + optional tiered `Fix`). The CLI subcommand is a thin renderer: it walks paths, calls the library per file, prints, and picks exit codes. Strict channel split: `pmt lint` reports lint findings only, never the compile warnings.

**Tech Stack:** Rust (edition per workspace), std only + `serde`/`serde_json` (already in-tree). **No new dependencies** — no regex, glob, or walkdir crates; the camelCase check and the directory walk are hand-rolled.

**Prerequisite:** the diagnostics-foundation plan (`docs/superpowers/plans/2026-07-06-diagnostics-foundation.md`) is **fully executed**. This plan consumes its interfaces AS GIVEN and must not redefine them:

```rust
// mtc-core::diagnostics (mtc_core::diagnostics)
pub struct Pos  { pub line: u32, pub col: u32 }   // 1-based, character-counted; derives Ord
pub struct Span { pub start: Pos, pub end: Pos }  // end-exclusive
impl Span { pub fn new(l: u32, c: u32, el: u32, ec: u32) -> Span; pub fn point(l: u32, c: u32) -> Span; }
pub enum Applicability { MachineApplicable, MaybeIncorrect }
pub struct Edit { pub span: Span, pub replacement: String }   // "" = delete
pub struct Fix  { pub description: String, pub applicability: Applicability, pub edits: Vec<Edit> }
pub struct Diagnostic { pub code: &'static str, pub span: Span, pub message: String, pub fix: Option<Fix> }

// post-machine lexer
pub struct Token { pub kind: TokenKind, pub line: u32, pub col: u32, pub len: u32 }
impl Token { pub fn span(&self) -> Span }         // [line:col, line:col+len)

// post-machine parser (AST after the span refactor)
pub struct Label { pub value: u32, pub span: Span }          // span: number start → colon end
pub struct Statement { pub labels: Vec<Label>, pub items: Vec<Item>, pub line: u32, pub span: Span }
// Function gains  pub name_span: Span
// Import gains    pub span: Span                            // the path tokens in the use line
// Item::Call gains  name_span: Span
// Item::Check gains span: Span                              // `check` keyword → closing `)`
// Item::Builtin and Item::Call gain succ_span: Option<Span> // the successor TOKEN (`N`/`!`) inside the parens

// post-machine compiler
pub(crate) struct ScopeSummary {
    pub defs: HashMap<Vec<String>, HashMap<String, String>>,             // ns path → bare → full name
    pub bindings: HashMap<Vec<String>, HashMap<String, (usize, String)>>, // ns path → bare → (import idx, full path)
}
pub(crate) struct AnalysisOutput {
    pub tokens: Vec<Token>,
    pub ast: Program,        // FLATTENED: function names fully qualified; statements untouched
    pub ir: IrProgram,       // unoptimized
    pub diagnostics: Vec<Diagnostic>,   // the 4 compile warnings — NOT reported by lint
    pub scopes: ScopeSummary,
}
pub(crate) fn analyze(source: &str) -> Result<AnalysisOutput, CompileError>
// CompileError { pub span: Span, pub kind: CompileErrorKind }
```

## Global Constraints

- Dependencies stay `serde`/`serde_json` only; `proptest` dev-only. No regex/glob/walkdir.
- Thin-renderer rule: library code never prints; every terminal byte originates in `cli/`.
- `pmt lint` reports **lint findings only** (strict channel split); compile warnings stay on `pmt compile`.
- Rule codes are defect-named and exactly: `unused-label`, `shadowed-import`, `redundant-jump-to-next`, `identical-check-arms`, `leftover-debugger`, `namespaced-main`, `line-too-long`, `leading-zeros`, `non-camel-case`, `confusable-names`.
- Fix tiers: plain `--fix` applies `MachineApplicable` only (`leading-zeros`); `--fix --force` adds `MaybeIncorrect` (`unused-label`, `redundant-jump-to-next`, `leftover-debugger` deletions; `identical-check-arms` replacement). Plain `--fix` never deletes user-written code.
- Exit codes: 0 = every file clean; 1 = findings or errors anywhere.
- Quality gates before every commit: `cargo test --workspace` green, `cargo clippy --workspace --all-targets -- -D warnings` clean, `cargo fmt --check` clean.
- Conventional commits with scope (`feat(post-machine):`, `feat(cli):`, `test(post-machine):`, `docs:`). No AI/Claude attribution lines anywhere.
- Published docs (`README.md`, `docs/`) are forge-agnostic: no issue/PR numbers, no host URLs.

---

### Task 1: Lint module skeleton — `lint()`, options, report, empty rule table

**Files:**
- Create: `crates/post-machine/src/lint/mod.rs`
- Create: `crates/post-machine/src/lint/rules/mod.rs`
- Modify: `crates/post-machine/src/lib.rs` (add `pub mod lint;` + re-exports)

**Interfaces:**
- Consumes: `compiler::analyze`, `AnalysisOutput`, `ScopeSummary` (crate-private), `mtc_core::diagnostics::{Diagnostic, Span, Pos}`.
- Produces (all later tasks build on these exact names):
  - `pub fn lint(source: &str, options: LintOptions) -> Result<LintReport, LintError>`
  - `pub struct LintOptions { pub allow: Vec<String> }` (+ `Default`)
  - `pub struct LintReport { pub diagnostics: Vec<Diagnostic> }`
  - `pub enum LintError { Compile(CompileError), UnknownAllowCode(String) }` (+ `Display`, `Error`, `From<CompileError>`)
  - `pub(crate) struct LintContext<'a> { pub source, pub tokens, pub ast, pub ir, pub scopes }`
  - `pub(crate) fn span_text(source: &str, span: Span) -> String`
  - `pub(crate) const RULES: &[(&'static str, fn(&LintContext, &mut Vec<Diagnostic>))]` (in `lint/mod.rs`; rules register here task by task)

- [ ] **Step 1: Write the failing tests**

In `crates/post-machine/src/lint/mod.rs` (file does not exist yet — create it with the tests at the bottom and stub items so intent is clear; the module isn't wired yet, so compilation fails first on the missing `pub mod lint;`):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_program_with_no_rules_yields_empty_report() {
        let report = lint("main() { right; }", LintOptions::default()).unwrap();
        assert!(report.diagnostics.is_empty());
    }

    #[test]
    fn unknown_allow_code_is_an_error() {
        let err = lint(
            "main() { right; }",
            LintOptions {
                allow: vec!["no-such-rule".into()],
            },
        )
        .unwrap_err();
        assert!(matches!(err, LintError::UnknownAllowCode(ref c) if c == "no-such-rule"));
        assert!(err.to_string().contains("no-such-rule"));
    }

    #[test]
    fn fatal_parse_error_propagates() {
        let err = lint("main( {", LintOptions::default()).unwrap_err();
        assert!(matches!(err, LintError::Compile(_)));
    }

    #[test]
    fn span_text_slices_by_char_positions() {
        use mtc_core::diagnostics::Span;
        let src = "ab\ncdef\n";
        assert_eq!(span_text(src, Span::new(2, 2, 2, 4)), "de");
        // Multi-line span crosses the newline.
        assert_eq!(span_text(src, Span::new(1, 2, 2, 2)), "b\nc");
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p mtc-post-machine lint 2>&1 | head -20`
Expected: compile error — `lint` module does not exist / unresolved imports.

- [ ] **Step 3: Implement the skeleton**

`crates/post-machine/src/lint/mod.rs` (above the tests from Step 1):

```rust
//! `.pmc` lint layer (docs/lint.md): hygiene findings over the compiler's
//! analysis. Library-only — the CLI renders (docs/cli.md). Strict channel
//! split: lint reports lint findings ONLY; the compile warnings stay on
//! the compile channel and are never re-reported here.

pub mod rules;

use mtc_core::diagnostics::{Diagnostic, Span};

use crate::compiler::{self, CompileError, ScopeSummary};
use crate::ir::IrProgram;
use crate::lexer::Token;
use crate::parser::Program;

#[derive(Debug, Clone, Default)]
pub struct LintOptions {
    /// Rule codes to suppress. Unknown codes are an error (typo protection).
    pub allow: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct LintReport {
    /// Lint findings only, source-ordered by span start (stable).
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug)]
pub enum LintError {
    /// Lint requires a program that parses and resolves.
    Compile(CompileError),
    /// `--allow` named a code no rule declares.
    UnknownAllowCode(String),
}

impl std::fmt::Display for LintError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LintError::Compile(e) => write!(f, "{e}"),
            LintError::UnknownAllowCode(code) => {
                write!(f, "unknown lint rule `{code}` in --allow")
            }
        }
    }
}

impl std::error::Error for LintError {}

impl From<CompileError> for LintError {
    fn from(e: CompileError) -> Self {
        LintError::Compile(e)
    }
}

/// Everything a rule may read. Rules never mutate the program.
pub(crate) struct LintContext<'a> {
    pub source: &'a str,
    pub tokens: &'a [Token],
    /// FLATTENED program: function names are fully qualified
    /// (`std::api.helper`); statement/item shapes are untouched.
    pub ast: &'a Program,
    /// Unoptimized CFG — rules judge source hygiene, not optimizer output.
    pub ir: &'a IrProgram,
    pub scopes: &'a ScopeSummary,
}

/// The rule table. One entry per rule, keyed by its defect-named code;
/// registration order is irrelevant (findings are sorted by span).
pub(crate) const RULES: &[(&'static str, fn(&LintContext, &mut Vec<Diagnostic>))] = &[];

pub fn lint(source: &str, options: LintOptions) -> Result<LintReport, LintError> {
    for code in &options.allow {
        if !RULES.iter().any(|(c, _)| c == code) {
            return Err(LintError::UnknownAllowCode(code.clone()));
        }
    }
    let analysis = compiler::analyze(source)?;
    let ctx = LintContext {
        source,
        tokens: &analysis.tokens,
        ast: &analysis.ast,
        ir: &analysis.ir,
        scopes: &analysis.scopes,
    };
    let mut diagnostics = Vec::new();
    for (code, rule) in RULES {
        if options.allow.iter().any(|a| a == code) {
            continue;
        }
        rule(&ctx, &mut diagnostics);
    }
    diagnostics.sort_by_key(|d| d.span.start); // stable; Pos is Ord
    Ok(LintReport { diagnostics })
}

/// Slice `source` by a char-counted span (1-based line/col, end-exclusive).
pub(crate) fn span_text(source: &str, span: Span) -> String {
    let mut out = String::new();
    let (mut line, mut col) = (1u32, 1u32);
    for c in source.chars() {
        let pos = mtc_core::diagnostics::Pos { line, col };
        if pos >= span.start && pos < span.end {
            out.push(c);
        }
        if c == '\n' {
            line += 1;
            col = 1;
        } else {
            col += 1;
        }
    }
    out
}
```

`crates/post-machine/src/lint/rules/mod.rs`:

```rust
//! One file per lint rule (docs/lint.md). Each rule exposes
//! `pub(crate) fn check(&LintContext, &mut Vec<Diagnostic>)` and is
//! registered in `super::RULES` under its defect-named code.
```

In `crates/post-machine/src/lib.rs`, add after `pub mod lexer;`:

```rust
pub mod lint;
```

and extend the re-exports:

```rust
pub use lint::{LintError, LintOptions, LintReport, lint};
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p mtc-post-machine lint`
Expected: 4 passed.

- [ ] **Step 5: Quality gates + commit**

```bash
cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --check
git add crates/post-machine/src/lint crates/post-machine/src/lib.rs
git commit -m "feat(post-machine): lint module skeleton — lint(), LintOptions, empty rule table"
```

---

### Task 2: `line-too-long` rule

**Files:**
- Create: `crates/post-machine/src/lint/rules/line_too_long.rs`
- Modify: `crates/post-machine/src/lint/rules/mod.rs` (register module)
- Modify: `crates/post-machine/src/lint/mod.rs` (RULES entry)

**Interfaces:**
- Consumes: `LintContext.source`, `Diagnostic`, `Span::new`.
- Produces: `rules::line_too_long::check(&LintContext, &mut Vec<Diagnostic>)`, registered as code `"line-too-long"`.

- [ ] **Step 1: Write the failing tests**

`crates/post-machine/src/lint/rules/line_too_long.rs`:

```rust
#[cfg(test)]
mod tests {
    use crate::lint::{LintOptions, lint};

    #[test]
    fn fires_past_80_chars_with_excess_span() {
        // A comment line of exactly 90 chars inside a valid program.
        let long = format!("// {}", "x".repeat(87));
        let src = format!("{long}\nmain() {{ right; }}\n");
        let report = lint(&src, LintOptions::default()).unwrap();
        let d: Vec<_> = report
            .diagnostics
            .iter()
            .filter(|d| d.code == "line-too-long")
            .collect();
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].message, "line is 90 characters long (limit 80)");
        assert_eq!((d[0].span.start.line, d[0].span.start.col), (1, 81));
        assert_eq!(d[0].span.end.col, 91); // end-exclusive: col 81..=90
        assert!(d[0].fix.is_none());
    }

    #[test]
    fn exactly_80_chars_is_clean() {
        let edge = format!("// {}", "x".repeat(77)); // 80 chars
        let src = format!("{edge}\nmain() {{ right; }}\n");
        let report = lint(&src, LintOptions::default()).unwrap();
        assert!(report.diagnostics.iter().all(|d| d.code != "line-too-long"));
    }

    #[test]
    fn counts_chars_not_bytes() {
        // 80 Cyrillic chars (160 bytes) — must be clean.
        let edge = format!("// {}", "ж".repeat(77));
        let src = format!("{edge}\nmain() {{ right; }}\n");
        let report = lint(&src, LintOptions::default()).unwrap();
        assert!(report.diagnostics.iter().all(|d| d.code != "line-too-long"));
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p mtc-post-machine line_too_long`
Expected: compile error (module missing), or the filter tests fail — the rule doesn't exist.

- [ ] **Step 3: Implement**

Top of `crates/post-machine/src/lint/rules/line_too_long.rs`:

```rust
//! `line-too-long` (docs/lint.md): a line longer than 80 characters
//! (char count). Report-only — rewrapping is the fmt phase's job.

use mtc_core::diagnostics::{Diagnostic, Span};

use crate::lint::LintContext;

const LIMIT: u32 = 80;

pub(crate) fn check(ctx: &LintContext, out: &mut Vec<Diagnostic>) {
    for (i, text) in ctx.source.lines().enumerate() {
        let line = i as u32 + 1;
        let n = text.chars().count() as u32;
        if n > LIMIT {
            out.push(Diagnostic {
                code: "line-too-long",
                span: Span::new(line, LIMIT + 1, line, n + 1),
                message: format!("line is {n} characters long (limit {LIMIT})"),
                fix: None,
            });
        }
    }
}
```

In `crates/post-machine/src/lint/rules/mod.rs` add:

```rust
pub(crate) mod line_too_long;
```

In `crates/post-machine/src/lint/mod.rs` change the table to:

```rust
pub(crate) const RULES: &[(&'static str, fn(&LintContext, &mut Vec<Diagnostic>))] =
    &[("line-too-long", rules::line_too_long::check)];
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p mtc-post-machine lint`
Expected: all lint tests pass, including Task 1's (empty-table test asserted an empty report on a short program — still true).

- [ ] **Step 5: Quality gates + commit**

```bash
cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --check
git add crates/post-machine/src/lint
git commit -m "feat(post-machine): line-too-long lint rule"
```

---

### Task 3: `leading-zeros` rule (MachineApplicable fix)

**Files:**
- Create: `crates/post-machine/src/lint/rules/leading_zeros.rs`
- Modify: `crates/post-machine/src/lint/rules/mod.rs`, `crates/post-machine/src/lint/mod.rs` (register)

**Interfaces:**
- Consumes: `LintContext.tokens` (`Token { kind, span() }`), `span_text`, `Fix`, `Edit`, `Applicability::MachineApplicable`.
- Produces: `rules::leading_zeros::check`, code `"leading-zeros"`.

- [ ] **Step 1: Write the failing tests**

`crates/post-machine/src/lint/rules/leading_zeros.rs`:

```rust
#[cfg(test)]
mod tests {
    use mtc_core::diagnostics::Applicability;

    use crate::lint::{LintOptions, lint};

    #[test]
    fn fires_on_label_definition_and_goto_target() {
        let src = "main() {\n007: right;\n    goto 007;\n}\n";
        let report = lint(src, LintOptions::default()).unwrap();
        let d: Vec<_> = report
            .diagnostics
            .iter()
            .filter(|d| d.code == "leading-zeros")
            .collect();
        assert_eq!(d.len(), 2);
        assert_eq!(d[0].message, "'007' has leading zeros — write '7'");
        let fix = d[0].fix.as_ref().unwrap();
        assert!(matches!(fix.applicability, Applicability::MachineApplicable));
        assert_eq!(fix.edits[0].replacement, "7");
        // Span covers exactly the three digits of the definition.
        assert_eq!((d[0].span.start.line, d[0].span.start.col), (2, 1));
        assert_eq!(d[0].span.end.col, 4);
    }

    #[test]
    fn plain_numbers_and_comments_are_clean() {
        let src = "main() {\n7: right; // 007 in a comment is fine\n    goto 7;\n}\n";
        let report = lint(src, LintOptions::default()).unwrap();
        assert!(report.diagnostics.iter().all(|d| d.code != "leading-zeros"));
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p mtc-post-machine leading_zeros`
Expected: compile error / missing rule.

- [ ] **Step 3: Implement**

```rust
//! `leading-zeros` (docs/lint.md): a numeric token written with leading
//! zeros. The lexer parses digit runs straight to `u32`, so `007` and `7`
//! denote the same label while looking unrelated. Token-level — fires on
//! definitions, goto targets, check arms, and call successors alike, and
//! never inside comments (comments produce no tokens).

use mtc_core::diagnostics::{Applicability, Diagnostic, Edit, Fix};

use crate::lexer::TokenKind;
use crate::lint::{LintContext, span_text};

pub(crate) fn check(ctx: &LintContext, out: &mut Vec<Diagnostic>) {
    for tok in ctx.tokens {
        let TokenKind::Number(value) = &tok.kind else {
            continue;
        };
        let text = span_text(ctx.source, tok.span());
        if text.len() > 1 && text.starts_with('0') {
            let canonical = value.to_string();
            out.push(Diagnostic {
                code: "leading-zeros",
                span: tok.span(),
                message: format!("'{text}' has leading zeros — write '{canonical}'"),
                fix: Some(Fix {
                    description: format!("rewrite '{text}' as '{canonical}'"),
                    applicability: Applicability::MachineApplicable,
                    edits: vec![Edit {
                        span: tok.span(),
                        replacement: canonical,
                    }],
                }),
            });
        }
    }
}
```

Register in `rules/mod.rs` (`pub(crate) mod leading_zeros;`) and append to RULES:

```rust
pub(crate) const RULES: &[(&'static str, fn(&LintContext, &mut Vec<Diagnostic>))] = &[
    ("line-too-long", rules::line_too_long::check),
    ("leading-zeros", rules::leading_zeros::check),
];
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p mtc-post-machine lint`
Expected: all pass.

- [ ] **Step 5: Quality gates + commit**

```bash
cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --check
git add crates/post-machine/src/lint
git commit -m "feat(post-machine): leading-zeros lint rule with MachineApplicable fix"
```

---

### Task 4: `unused-label` rule (gated delete-fix)

**Files:**
- Create: `crates/post-machine/src/lint/rules/unused_label.rs`
- Modify: `crates/post-machine/src/lint/rules/mod.rs`, `crates/post-machine/src/lint/mod.rs` (register)

**Interfaces:**
- Consumes: `LintContext.ast` (flattened `Program`), `Label { value, span }`, `Item`, `CheckArm`, `Successor`, `Fix`, `Applicability::MaybeIncorrect`.
- Produces: `rules::unused_label::check`, code `"unused-label"`.

- [ ] **Step 1: Write the failing tests**

`crates/post-machine/src/lint/rules/unused_label.rs`:

```rust
#[cfg(test)]
mod tests {
    use mtc_core::diagnostics::Applicability;

    use crate::lint::{LintOptions, lint};

    #[test]
    fn unreferenced_label_fires_with_qualified_function_name() {
        let src = "namespace api {\nhelper() {\n5: right;\n}\n}\nmain() { @api::helper(); }\n";
        let report = lint(src, LintOptions::default()).unwrap();
        let d: Vec<_> = report
            .diagnostics
            .iter()
            .filter(|d| d.code == "unused-label")
            .collect();
        assert_eq!(d.len(), 1);
        assert_eq!(
            d[0].message,
            "label 5 is never referenced (function 'api::helper')"
        );
        let fix = d[0].fix.as_ref().unwrap();
        assert!(matches!(fix.applicability, Applicability::MaybeIncorrect));
        assert_eq!(fix.edits[0].replacement, "");
        // The label span covers `5:` — number start to colon end.
        assert_eq!((d[0].span.start.line, d[0].span.start.col), (3, 1));
        assert_eq!(d[0].span.end.col, 3);
    }

    #[test]
    fn referenced_labels_are_clean() {
        // goto, check arm, and successor references all count.
        let src = "main() {\n1: right(2);\n2: check(1, 3);\n3: goto 1;\n}\n";
        let report = lint(src, LintOptions::default()).unwrap();
        assert!(report.diagnostics.iter().all(|d| d.code != "unused-label"));
    }

    #[test]
    fn self_loop_label_on_single_statement_body_is_used() {
        // The single-statement sibling rule is subsumed: a referenced
        // label on the only statement is a self-loop, not a finding.
        let src = "main() {\n1: check(1, !);\n}\n";
        let report = lint(src, LintOptions::default()).unwrap();
        assert!(report.diagnostics.iter().all(|d| d.code != "unused-label"));
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p mtc-post-machine unused_label`
Expected: compile error / missing rule.

- [ ] **Step 3: Implement**

```rust
//! `unused-label` (docs/lint.md): a label nothing in its function
//! references — no goto, no check arm, no command successor. Function-
//! scoped, the same scope as label resolution. The delete-fix is gated:
//! an unused label may be evidence of a jump the author forgot to write.

use std::collections::HashSet;

use mtc_core::diagnostics::{Applicability, Diagnostic, Edit, Fix};

use crate::lint::LintContext;
use crate::parser::{CheckArm, Item, Successor};

pub(crate) fn check(ctx: &LintContext, out: &mut Vec<Diagnostic>) {
    for f in &ctx.ast.functions {
        let mut referenced: HashSet<u32> = HashSet::new();
        for stmt in &f.body {
            for item in &stmt.items {
                match item {
                    Item::Goto { label, .. } => {
                        referenced.insert(*label);
                    }
                    Item::Check { marked, blank, .. } => {
                        for arm in [marked, blank] {
                            if let CheckArm::Label(n) = arm {
                                referenced.insert(*n);
                            }
                        }
                    }
                    Item::Builtin { succ, .. } | Item::Call { succ, .. } => {
                        if let Successor::Label(n) = succ {
                            referenced.insert(*n);
                        }
                    }
                    Item::Halt { .. } | Item::Debugger { .. } => {}
                }
            }
        }
        for stmt in &f.body {
            for label in &stmt.labels {
                if !referenced.contains(&label.value) {
                    out.push(Diagnostic {
                        code: "unused-label",
                        span: label.span,
                        message: format!(
                            "label {} is never referenced (function '{}')",
                            label.value, f.name
                        ),
                        fix: Some(Fix {
                            description: format!("remove the label prefix '{}:'", label.value),
                            applicability: Applicability::MaybeIncorrect,
                            edits: vec![Edit {
                                span: label.span,
                                replacement: String::new(),
                            }],
                        }),
                    });
                }
            }
        }
    }
}
```

Register (`pub(crate) mod unused_label;`; RULES gains `("unused-label", rules::unused_label::check)`).

- [ ] **Step 4: Run tests**

Run: `cargo test -p mtc-post-machine lint`
Expected: all pass. Note the Task 1 empty-report test uses `main() { right; }` — no labels, still clean.

- [ ] **Step 5: Quality gates + commit**

```bash
cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --check
git add crates/post-machine/src/lint
git commit -m "feat(post-machine): unused-label lint rule with gated delete-fix"
```

---

### Task 5: `redundant-jump-to-next` rule

**Files:**
- Create: `crates/post-machine/src/lint/rules/redundant_jump.rs`
- Modify: `crates/post-machine/src/lint/rules/mod.rs`, `crates/post-machine/src/lint/mod.rs` (register)

**Interfaces:**
- Consumes: `Statement { labels, items, span }`, `Item::Goto`, `Item::Builtin/Call { succ, succ_span }`, `Fix`, `Applicability::MaybeIncorrect`.
- Produces: `rules::redundant_jump::check`, code `"redundant-jump-to-next"`.

- [ ] **Step 1: Write the failing tests**

`crates/post-machine/src/lint/rules/redundant_jump.rs`:

```rust
#[cfg(test)]
mod tests {
    use crate::lint::{LintOptions, lint};

    fn findings(src: &str) -> Vec<(String, bool)> {
        lint(src, LintOptions::default())
            .unwrap()
            .diagnostics
            .into_iter()
            .filter(|d| d.code == "redundant-jump-to-next")
            .map(|d| (d.message, d.fix.is_some()))
            .collect()
    }

    #[test]
    fn goto_to_lexically_next_statement_fires_with_fix() {
        let src = "main() {\n    goto 5;\n5:  right;\n}\n";
        let f = findings(src);
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].0, "goto 5 targets the next statement — fall-through is identical");
        assert!(f[0].1, "unlabeled goto statement gets the delete-fix");
    }

    #[test]
    fn labeled_goto_statement_is_report_only() {
        // Deleting `3: goto 5;` would orphan the reference to 3.
        let src = "main() {\n    check(3, 5);\n3:  goto 5;\n5:  right;\n}\n";
        let f = findings(src);
        assert_eq!(f.len(), 1);
        assert!(!f[0].1, "labeled statement must not carry a fix");
    }

    #[test]
    fn successor_to_next_statement_fires_and_deletes_only_the_successor() {
        let src = "main() {\n    right(5);\n5:  left;\n}\n";
        let report = lint(src, LintOptions::default()).unwrap();
        let d: Vec<_> = report
            .diagnostics
            .iter()
            .filter(|d| d.code == "redundant-jump-to-next")
            .collect();
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].message, "successor (5) targets the next statement — drop it");
        let fix = d[0].fix.as_ref().unwrap();
        // The edit deletes exactly the `5` inside the parens (leaves `right()`).
        assert_eq!(fix.edits[0].replacement, "");
        assert_eq!((fix.edits[0].span.start.line, fix.edits[0].span.start.col), (2, 11));
    }

    #[test]
    fn jump_past_the_next_statement_is_clean() {
        let src = "main() {\n    goto 6;\n5:  right;\n6:  left, check(5, !);\n}\n";
        assert!(findings(src).is_empty());
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p mtc-post-machine redundant_jump`
Expected: compile error / missing rule.

- [ ] **Step 3: Implement**

```rust
//! `redundant-jump-to-next` (docs/lint.md): a `goto N;` statement or a
//! `(N)` successor whose target labels the lexically next statement —
//! fall-through is identical (codegen's layout even elides such jumps).

use mtc_core::diagnostics::{Applicability, Diagnostic, Edit, Fix};

use crate::lint::LintContext;
use crate::parser::{Item, Successor};

pub(crate) fn check(ctx: &LintContext, out: &mut Vec<Diagnostic>) {
    for f in &ctx.ast.functions {
        for window in f.body.windows(2) {
            let (stmt, next) = (&window[0], &window[1]);
            let next_has = |n: u32| next.labels.iter().any(|l| l.value == n);
            // `goto` is never grouped (parser rule), so it is the only item.
            let last = stmt.items.last().expect("parser: statements have items");
            match last {
                Item::Goto { label, .. } if next_has(*label) => {
                    let fix = stmt.labels.is_empty().then(|| Fix {
                        description: format!("remove the redundant 'goto {label};'"),
                        applicability: Applicability::MaybeIncorrect,
                        edits: vec![Edit {
                            span: stmt.span,
                            replacement: String::new(),
                        }],
                    });
                    out.push(Diagnostic {
                        code: "redundant-jump-to-next",
                        span: stmt.span,
                        message: format!(
                            "goto {label} targets the next statement — fall-through is identical"
                        ),
                        fix,
                    });
                }
                Item::Builtin { succ, succ_span, .. } | Item::Call { succ, succ_span, .. } => {
                    if let (Successor::Label(n), Some(sspan)) = (succ, succ_span)
                        && next_has(*n)
                    {
                        out.push(Diagnostic {
                            code: "redundant-jump-to-next",
                            span: *sspan,
                            message: format!(
                                "successor ({n}) targets the next statement — drop it"
                            ),
                            fix: Some(Fix {
                                description: format!("remove the redundant successor ({n})"),
                                applicability: Applicability::MaybeIncorrect,
                                edits: vec![Edit {
                                    span: *sspan,
                                    replacement: String::new(),
                                }],
                            }),
                        });
                    }
                }
                _ => {}
            }
        }
    }
}
```

Register (`pub(crate) mod redundant_jump;`; RULES gains `("redundant-jump-to-next", rules::redundant_jump::check)`).

- [ ] **Step 4: Run tests**

Run: `cargo test -p mtc-post-machine lint`
Expected: all pass.

- [ ] **Step 5: Quality gates + commit**

```bash
cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --check
git add crates/post-machine/src/lint
git commit -m "feat(post-machine): redundant-jump-to-next lint rule"
```

---

### Task 6: `identical-check-arms` rule

**Files:**
- Create: `crates/post-machine/src/lint/rules/identical_check_arms.rs`
- Modify: `crates/post-machine/src/lint/rules/mod.rs`, `crates/post-machine/src/lint/mod.rs` (register)

**Interfaces:**
- Consumes: `Item::Check { marked, blank, span }`, `CheckArm`, `Fix`, `Applicability::MaybeIncorrect`.
- Produces: `rules::identical_check_arms::check`, code `"identical-check-arms"`.

- [ ] **Step 1: Write the failing tests**

`crates/post-machine/src/lint/rules/identical_check_arms.rs`:

```rust
#[cfg(test)]
mod tests {
    use crate::lint::{LintOptions, lint};

    #[test]
    fn identical_label_arms_fire_with_goto_replacement() {
        let src = "main() {\n5:  check(5, 5);\n}\n";
        let report = lint(src, LintOptions::default()).unwrap();
        let d: Vec<_> = report
            .diagnostics
            .iter()
            .filter(|d| d.code == "identical-check-arms")
            .collect();
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].message, "both check arms target 5 — replace with 'goto 5'");
        let fix = d[0].fix.as_ref().unwrap();
        // Replacement, not deletion — statement labels stay attached.
        assert_eq!(fix.edits[0].replacement, "goto 5");
    }

    #[test]
    fn group_final_check_is_report_only() {
        // `goto` is barred from comma groups — no legal substitution.
        let src = "main() {\n5:  right, check(5, 5);\n}\n";
        let report = lint(src, LintOptions::default()).unwrap();
        let d: Vec<_> = report
            .diagnostics
            .iter()
            .filter(|d| d.code == "identical-check-arms")
            .collect();
        assert_eq!(d.len(), 1);
        assert!(d[0].fix.is_none());
    }

    #[test]
    fn identical_bang_arms_are_exempt() {
        // check(!, !) is the language's only pure mid-function return —
        // there is no `return` keyword; legitimate, nothing to suggest.
        let src = "main() {\n1:  check(!, !);\n    goto 1;\n}\n";
        let report = lint(src, LintOptions::default()).unwrap();
        assert!(report.diagnostics.iter().all(|d| d.code != "identical-check-arms"));
    }

    #[test]
    fn different_arms_are_clean() {
        let src = "main() {\n1: right;\n2: check(1, !);\n}\n";
        let report = lint(src, LintOptions::default()).unwrap();
        assert!(report.diagnostics.iter().all(|d| d.code != "identical-check-arms"));
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p mtc-post-machine identical_check_arms`
Expected: compile error / missing rule.

- [ ] **Step 3: Implement**

```rust
//! `identical-check-arms` (docs/lint.md): `check(N, N)` — both arms land
//! in the same place, so the branch is unconditional; `goto N` was meant
//! or one arm is a typo. `check(!, !)` is EXEMPT: it is the language's
//! only pure mid-function return (there is no `return` keyword and `(!)`
//! successors need a carrier action).

use mtc_core::diagnostics::{Applicability, Diagnostic, Edit, Fix};

use crate::lint::LintContext;
use crate::parser::{CheckArm, Item};

pub(crate) fn check(ctx: &LintContext, out: &mut Vec<Diagnostic>) {
    for f in &ctx.ast.functions {
        for stmt in &f.body {
            for item in &stmt.items {
                let Item::Check {
                    marked: CheckArm::Label(a),
                    blank: CheckArm::Label(b),
                    span,
                    ..
                } = item
                else {
                    continue; // different arms, or `!` arms (exempt)
                };
                if a != b {
                    continue;
                }
                // Standalone statement → replace with `goto N` (labels stay
                // attached — this is a replacement). Group-final → report
                // only: `goto` is barred from comma groups.
                let fix = (stmt.items.len() == 1).then(|| Fix {
                    description: format!("replace 'check({a}, {a})' with 'goto {a}'"),
                    applicability: Applicability::MaybeIncorrect,
                    edits: vec![Edit {
                        span: *span,
                        replacement: format!("goto {a}"),
                    }],
                });
                out.push(Diagnostic {
                    code: "identical-check-arms",
                    span: *span,
                    message: format!("both check arms target {a} — replace with 'goto {a}'"),
                    fix,
                });
            }
        }
    }
}
```

Register (`pub(crate) mod identical_check_arms;`; RULES gains `("identical-check-arms", rules::identical_check_arms::check)`).

- [ ] **Step 4: Run tests**

Run: `cargo test -p mtc-post-machine lint`
Expected: all pass. (The exempt-arms test also exercises `unused-label` cleanliness: label 1 is referenced by `goto 1`.)

- [ ] **Step 5: Quality gates + commit**

```bash
cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --check
git add crates/post-machine/src/lint
git commit -m "feat(post-machine): identical-check-arms lint rule"
```

---

### Task 7: `leftover-debugger` rule

**Files:**
- Create: `crates/post-machine/src/lint/rules/leftover_debugger.rs`
- Modify: `crates/post-machine/src/lint/rules/mod.rs`, `crates/post-machine/src/lint/mod.rs` (register)

**Interfaces:**
- Consumes: `Item::Debugger`, `Statement { labels, items, span }`, `Fix`, `Applicability::MaybeIncorrect`.
- Produces: `rules::leftover_debugger::check`, code `"leftover-debugger"`.

- [ ] **Step 1: Write the failing tests**

`crates/post-machine/src/lint/rules/leftover_debugger.rs`:

```rust
#[cfg(test)]
mod tests {
    use crate::lint::{LintOptions, lint};

    #[test]
    fn lone_unlabeled_debugger_fires_with_delete_fix() {
        let src = "main() {\n    debugger;\n    right;\n}\n";
        let report = lint(src, LintOptions::default()).unwrap();
        let d: Vec<_> = report
            .diagnostics
            .iter()
            .filter(|d| d.code == "leftover-debugger")
            .collect();
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].message, "leftover 'debugger' statement");
        assert!(d[0].fix.is_some());
    }

    #[test]
    fn labeled_or_grouped_debugger_is_report_only() {
        // Labeled: deleting would orphan the `goto 5` reference.
        let labeled = "main() {\n    goto 5;\n5:  debugger;\n    right;\n}\n";
        let report = lint(labeled, LintOptions::default()).unwrap();
        let d: Vec<_> = report
            .diagnostics
            .iter()
            .filter(|d| d.code == "leftover-debugger")
            .collect();
        assert_eq!(d.len(), 1);
        assert!(d[0].fix.is_none());

        // Grouped: the statement carries more than the debugger.
        let grouped = "main() {\n    debugger, right;\n}\n";
        let report = lint(grouped, LintOptions::default()).unwrap();
        let d: Vec<_> = report
            .diagnostics
            .iter()
            .filter(|d| d.code == "leftover-debugger")
            .collect();
        assert_eq!(d.len(), 1);
        assert!(d[0].fix.is_none());
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p mtc-post-machine leftover_debugger`
Expected: compile error / missing rule.

- [ ] **Step 3: Implement**

```rust
//! `leftover-debugger` (docs/lint.md): a `debugger` statement in source.
//! Builds strip breakpoints with `--strip-debugger`, and an un-stripped
//! `brk` is an optimizer observability barrier — shipping one also
//! pessimizes `-O1` output. Delete-fix only for a lone, unlabeled
//! `debugger;` statement (anything else risks orphaning labels or
//! mangling a comma group).

use mtc_core::diagnostics::{Applicability, Diagnostic, Edit, Fix};

use crate::lint::LintContext;
use crate::parser::Item;

pub(crate) fn check(ctx: &LintContext, out: &mut Vec<Diagnostic>) {
    for f in &ctx.ast.functions {
        for stmt in &f.body {
            for item in &stmt.items {
                if !matches!(item, Item::Debugger { .. }) {
                    continue;
                }
                let deletable = stmt.labels.is_empty() && stmt.items.len() == 1;
                let fix = deletable.then(|| Fix {
                    description: "remove the 'debugger;' statement".to_string(),
                    applicability: Applicability::MaybeIncorrect,
                    edits: vec![Edit {
                        span: stmt.span,
                        replacement: String::new(),
                    }],
                });
                out.push(Diagnostic {
                    code: "leftover-debugger",
                    span: stmt.span,
                    message: "leftover 'debugger' statement".to_string(),
                    fix,
                });
            }
        }
    }
}
```

Register (`pub(crate) mod leftover_debugger;`; RULES gains `("leftover-debugger", rules::leftover_debugger::check)`).

- [ ] **Step 4: Run tests**

Run: `cargo test -p mtc-post-machine lint`
Expected: all pass.

- [ ] **Step 5: Quality gates + commit**

```bash
cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --check
git add crates/post-machine/src/lint
git commit -m "feat(post-machine): leftover-debugger lint rule"
```

---

### Task 8: `namespaced-main` rule

**Files:**
- Create: `crates/post-machine/src/lint/rules/namespaced_main.rs`
- Modify: `crates/post-machine/src/lint/rules/mod.rs`, `crates/post-machine/src/lint/mod.rs` (register)

**Interfaces:**
- Consumes: flattened `Function { name, ns, name_span }`.
- Produces: `rules::namespaced_main::check`, code `"namespaced-main"`.

- [ ] **Step 1: Write the failing tests**

`crates/post-machine/src/lint/rules/namespaced_main.rs`:

```rust
#[cfg(test)]
mod tests {
    use crate::lint::{LintOptions, lint};

    #[test]
    fn main_inside_a_namespace_fires() {
        let src = "namespace app {\nmain() { right; }\n}\nmain() { @app::main(); }\n";
        let report = lint(src, LintOptions::default()).unwrap();
        let d: Vec<_> = report
            .diagnostics
            .iter()
            .filter(|d| d.code == "namespaced-main")
            .collect();
        assert_eq!(d.len(), 1);
        assert_eq!(
            d[0].message,
            "'app::main' is not the program entry (only top-level 'main' is)"
        );
        assert!(d[0].fix.is_none());
    }

    #[test]
    fn top_level_main_and_nested_main_are_clean() {
        // Top-level main IS the entry; a NESTED function named main
        // (dot-mangled `outer.main`) is not the namespaced footgun.
        let src = "main() {\n    @helper();\nhelper() {\n    right;\n}\n}\n";
        let report = lint(src, LintOptions::default()).unwrap();
        assert!(report.diagnostics.iter().all(|d| d.code != "namespaced-main"));
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p mtc-post-machine namespaced_main`
Expected: compile error / missing rule.

- [ ] **Step 3: Implement**

```rust
//! `namespaced-main` (docs/lint.md): a function named `main` inside a
//! namespace. Only the un-namespaced top-level `main` is the program
//! entry, and a namespaced `main` is not auto-exported either — it
//! silently becomes an ordinary local function. Almost always a
//! misunderstanding.

use mtc_core::diagnostics::Diagnostic;

use crate::lint::LintContext;

pub(crate) fn check(ctx: &LintContext, out: &mut Vec<Diagnostic>) {
    for f in &ctx.ast.functions {
        // Flattened names: `ns::main` (namespaced, no `.` nesting part).
        let is_namespaced_main = !f.ns.is_empty()
            && !f.name.contains('.')
            && f.name.rsplit("::").next() == Some("main");
        if is_namespaced_main {
            out.push(Diagnostic {
                code: "namespaced-main",
                span: f.name_span,
                message: format!(
                    "'{}' is not the program entry (only top-level 'main' is)",
                    f.name
                ),
                fix: None,
            });
        }
    }
}
```

Register (`pub(crate) mod namespaced_main;`; RULES gains `("namespaced-main", rules::namespaced_main::check)`).

- [ ] **Step 4: Run tests**

Run: `cargo test -p mtc-post-machine lint`
Expected: all pass.

- [ ] **Step 5: Quality gates + commit**

```bash
cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --check
git add crates/post-machine/src/lint
git commit -m "feat(post-machine): namespaced-main lint rule"
```

---

### Task 9: `shadowed-import` rule

**Files:**
- Create: `crates/post-machine/src/lint/rules/shadowed_import.rs`
- Modify: `crates/post-machine/src/lint/rules/mod.rs`, `crates/post-machine/src/lint/mod.rs` (register)

**Interfaces:**
- Consumes: `LintContext.scopes` (`ScopeSummary { defs, bindings }`), `ast.functions[].{name, name_span}`.
- Produces: `rules::shadowed_import::check`, code `"shadowed-import"`.

- [ ] **Step 1: Write the failing tests**

`crates/post-machine/src/lint/rules/shadowed_import.rs`:

```rust
#[cfg(test)]
mod tests {
    use crate::lint::{LintOptions, lint};

    #[test]
    fn definition_outranking_same_scope_import_fires() {
        let src = "use std::goToEnd;\ngoToEnd() { right; }\nmain() { @goToEnd(); }\n";
        let report = lint(src, LintOptions::default()).unwrap();
        let d: Vec<_> = report
            .diagnostics
            .iter()
            .filter(|d| d.code == "shadowed-import")
            .collect();
        assert_eq!(d.len(), 1);
        assert_eq!(
            d[0].message,
            "function 'goToEnd' shadows the import of 'std::goToEnd' — bare calls resolve to the local definition"
        );
        assert!(d[0].fix.is_none());
        // Anchored at the shadowing definition, line 2.
        assert_eq!(d[0].span.start.line, 2);
    }

    #[test]
    fn cross_scope_shadowing_is_legal_layering() {
        // File-level import, namespace-level definition: inner shadows
        // outer by design — not flagged (same-scope only).
        let src = "use std::goToEnd;\nnamespace inner {\ngoToEnd() { right; }\n}\nmain() { @goToEnd(); @inner::goToEnd(); }\n";
        let report = lint(src, LintOptions::default()).unwrap();
        assert!(report.diagnostics.iter().all(|d| d.code != "shadowed-import"));
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p mtc-post-machine shadowed_import`
Expected: compile error / missing rule.

- [ ] **Step 3: Implement**

```rust
//! `shadowed-import` (docs/lint.md): a function definition whose name
//! outranks an import binding of the same bare name in the SAME scope —
//! legal (definitions always win), but a bare `@name()` call silently
//! hits the local function while the `use` line suggests the external.
//! Cross-scope shadowing (inner over outer) is legal layering: not flagged.

use mtc_core::diagnostics::Diagnostic;

use crate::lint::LintContext;

pub(crate) fn check(ctx: &LintContext, out: &mut Vec<Diagnostic>) {
    for (scope, bindings) in &ctx.scopes.bindings {
        let Some(defs) = ctx.scopes.defs.get(scope) else {
            continue;
        };
        for (bare, (_idx, full_path)) in bindings {
            let Some(full_def) = defs.get(bare) else {
                continue;
            };
            // Anchor at the shadowing definition's name token.
            let Some(f) = ctx.ast.functions.iter().find(|f| &f.name == full_def) else {
                continue;
            };
            out.push(Diagnostic {
                code: "shadowed-import",
                span: f.name_span,
                message: format!(
                    "function '{bare}' shadows the import of '{full_path}' — bare calls resolve to the local definition"
                ),
                fix: None,
            });
        }
    }
}
```

Register (`pub(crate) mod shadowed_import;`; RULES gains `("shadowed-import", rules::shadowed_import::check)`).

- [ ] **Step 4: Run tests**

Run: `cargo test -p mtc-post-machine lint`
Expected: all pass. (HashMap iteration order does not matter: `lint()` sorts findings by span.)

- [ ] **Step 5: Quality gates + commit**

```bash
cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --check
git add crates/post-machine/src/lint
git commit -m "feat(post-machine): shadowed-import lint rule"
```

---

### Task 10: `non-camel-case` rule

**Files:**
- Create: `crates/post-machine/src/lint/rules/non_camel_case.rs`
- Modify: `crates/post-machine/src/lint/rules/mod.rs`, `crates/post-machine/src/lint/mod.rs` (register)

**Interfaces:**
- Consumes: flattened `Function { name, ns, name_span }`, `Import { span, alias, path }` + `Import::binding()/full_path()`.
- Produces: `rules::non_camel_case::check`, code `"non-camel-case"`; helpers `is_lower_camel(&str) -> bool`, `to_camel(&str) -> String` (rule-private).

- [ ] **Step 1: Write the failing tests**

`crates/post-machine/src/lint/rules/non_camel_case.rs`:

```rust
#[cfg(test)]
mod tests {
    use crate::lint::{LintOptions, lint};

    fn messages(src: &str) -> Vec<String> {
        lint(src, LintOptions::default())
            .unwrap()
            .diagnostics
            .into_iter()
            .filter(|d| d.code == "non-camel-case")
            .map(|d| d.message)
            .collect()
    }

    #[test]
    fn snake_case_function_fires_with_suggestion() {
        let m = messages("export sum_bits() { right; }\nmain() { @sum_bits(); }\n");
        assert_eq!(
            m,
            vec!["function 'sum_bits' is not camelCase — rename to 'sumBits'"]
        );
    }

    #[test]
    fn violating_import_binding_suggests_an_alias() {
        let m = messages("use their::do_thing;\nmain() { @do_thing(); }\n");
        assert_eq!(
            m,
            vec![
                "import binding 'do_thing' is not camelCase — alias it: 'use their::do_thing as doThing'"
            ]
        );
    }

    #[test]
    fn violating_namespace_segment_fires_once() {
        let src = "namespace my_ns {\nexport a() { right; }\nexport b() { right; }\n}\nmain() { @my_ns::a(); @my_ns::b(); }\n";
        let m = messages(src);
        assert_eq!(
            m,
            vec!["namespace 'my_ns' is not camelCase — rename to 'myNs'"]
        );
    }

    #[test]
    fn camel_case_names_are_clean() {
        let m = messages("main() { @goToEnd(); }\ngoToEnd() { right; }\n");
        assert!(m.is_empty());
    }

    #[test]
    fn to_camel_derivations() {
        use super::to_camel;
        assert_eq!(to_camel("sum_bits"), "sumBits");
        assert_eq!(to_camel("Foo"), "foo");
        assert_eq!(to_camel("do_thing_2"), "doThing2");
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p mtc-post-machine non_camel_case`
Expected: compile error / missing rule.

- [ ] **Step 3: Implement**

```rust
//! `non-camel-case` (docs/lint.md): user-owned definition names —
//! functions, namespaces, import bindings — must be lowerCamelCase
//! (`^[a-z][a-zA-Z0-9]*$`, checked by hand: no regex dependency). The
//! project's de-facto house style; the stdlib is uniformly camelCase.
//! Report-only: a rename is a multi-site edit and, for exports, changes
//! the mangled symbol name (link-time ABI). The message carries a
//! mechanically derived suggestion instead.

use std::collections::HashSet;

use mtc_core::diagnostics::Diagnostic;

use crate::lint::LintContext;

/// `^[a-z][a-zA-Z0-9]*$` by hand.
fn is_lower_camel(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if c.is_ascii_lowercase() => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric())
}

/// Mechanical camelCase derivation: drop `_`, capitalize the char after
/// each dropped `_`, lowercase the first char.
pub(super) fn to_camel(name: &str) -> String {
    let mut out = String::new();
    let mut upper_next = false;
    for c in name.chars() {
        if c == '_' {
            upper_next = true;
            continue;
        }
        if out.is_empty() {
            out.extend(c.to_lowercase());
        } else if upper_next {
            out.extend(c.to_uppercase());
        } else {
            out.push(c);
        }
        upper_next = false;
    }
    out
}

pub(crate) fn check(ctx: &LintContext, out: &mut Vec<Diagnostic>) {
    // Functions: judge the user-authored final segment of the flattened
    // name (`std::api.helper` → `helper`; plain `api` → `api`).
    for f in &ctx.ast.functions {
        let last = f
            .name
            .rsplit("::")
            .next()
            .and_then(|s| s.rsplit('.').next())
            .expect("names are non-empty");
        if !is_lower_camel(last) {
            out.push(Diagnostic {
                code: "non-camel-case",
                span: f.name_span,
                message: format!(
                    "function '{last}' is not camelCase — rename to '{}'",
                    to_camel(last)
                ),
                fix: None,
            });
        }
    }
    // Namespace segments, once per unique path prefix. The flattened AST
    // retains no namespace-name spans, so the finding anchors at the
    // first function defined under that namespace.
    let mut seen_ns: HashSet<Vec<String>> = HashSet::new();
    for f in &ctx.ast.functions {
        for depth in 1..=f.ns.len() {
            let prefix = f.ns[..depth].to_vec();
            let segment = prefix.last().expect("depth >= 1").clone();
            if !seen_ns.insert(prefix) {
                continue;
            }
            if !is_lower_camel(&segment) {
                out.push(Diagnostic {
                    code: "non-camel-case",
                    span: f.name_span,
                    message: format!(
                        "namespace '{segment}' is not camelCase — rename to '{}'",
                        to_camel(&segment)
                    ),
                    fix: None,
                });
            }
        }
    }
    // Import bindings: the binding is the user's to rename via `as`.
    for imp in &ctx.ast.imports {
        let binding = imp.binding();
        if !is_lower_camel(binding) {
            out.push(Diagnostic {
                code: "non-camel-case",
                span: imp.span,
                message: format!(
                    "import binding '{binding}' is not camelCase — alias it: 'use {} as {}'",
                    imp.full_path(),
                    to_camel(binding)
                ),
                fix: None,
            });
        }
    }
}
```

Register (`pub(crate) mod non_camel_case;`; RULES gains `("non-camel-case", rules::non_camel_case::check)`).

- [ ] **Step 4: Run tests**

Run: `cargo test -p mtc-post-machine lint`
Expected: all pass. Note: Unicode identifiers (legal in `.pmc`) fail the ASCII check by spec (`^[a-z][a-zA-Z0-9]*$`); `--allow non-camel-case` is the escape hatch — documented in Task 17.

- [ ] **Step 5: Quality gates + commit**

```bash
cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --check
git add crates/post-machine/src/lint
git commit -m "feat(post-machine): non-camel-case lint rule"
```

---

### Task 11: `confusable-names` rule

**Files:**
- Create: `crates/post-machine/src/lint/rules/confusable_names.rs`
- Modify: `crates/post-machine/src/lint/rules/mod.rs`, `crates/post-machine/src/lint/mod.rs` (register)

**Interfaces:**
- Consumes: `ScopeSummary { defs, bindings }`, `ast.functions[].name_span`, `ast.imports[].span`.
- Produces: `rules::confusable_names::check`, code `"confusable-names"`.

- [ ] **Step 1: Write the failing tests**

`crates/post-machine/src/lint/rules/confusable_names.rs`:

```rust
#[cfg(test)]
mod tests {
    use crate::lint::{LintOptions, lint};

    #[test]
    fn confusable_pair_reports_at_the_later_definition() {
        let src = "sumBits() { right; }\nsum_bits() { left; }\nmain() { @sumBits(); @sum_bits(); }\n";
        let report = lint(src, LintOptions::default()).unwrap();
        let d: Vec<_> = report
            .diagnostics
            .iter()
            .filter(|d| d.code == "confusable-names")
            .collect();
        assert_eq!(d.len(), 1);
        assert_eq!(
            d[0].message,
            "'sum_bits' is confusable with 'sumBits' (defined at line 1)"
        );
        assert_eq!(d[0].span.start.line, 2);
        assert!(d[0].fix.is_none());
    }

    #[test]
    fn digit_letter_confusables_fire() {
        // fool vs foo1: '1' normalizes to 'l'.
        let src = "fool() { right; }\nfoo1() { left; }\nmain() { @fool(); @foo1(); }\n";
        let report = lint(src, LintOptions::default()).unwrap();
        assert_eq!(
            report
                .diagnostics
                .iter()
                .filter(|d| d.code == "confusable-names")
                .count(),
            1
        );
    }

    #[test]
    fn distinct_names_and_cross_scope_pairs_are_clean() {
        let src = "namespace a {\nexport doIt() { right; }\n}\ndoIt() { left; }\nmain() { @doIt(); @a::doIt(); }\n";
        let report = lint(src, LintOptions::default()).unwrap();
        assert!(report.diagnostics.iter().all(|d| d.code != "confusable-names"));
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p mtc-post-machine confusable_names`
Expected: compile error / missing rule.

- [ ] **Step 3: Implement**

```rust
//! `confusable-names` (docs/lint.md): two definitions or bindings in the
//! SAME scope whose names differ only under a confusability
//! normalization — lowercase, strip `_`, map `1→l`, `i→l`, `0→o`.
//! Deterministic; one finding per pair, reported at the later
//! definition, naming the earlier one.

use std::collections::HashMap;

use mtc_core::diagnostics::{Diagnostic, Span};

use crate::lint::LintContext;

fn normalize(name: &str) -> String {
    name.to_lowercase()
        .chars()
        .filter(|&c| c != '_')
        .map(|c| match c {
            '1' | 'i' => 'l',
            '0' => 'o',
            other => other,
        })
        .collect()
}

pub(crate) fn check(ctx: &LintContext, out: &mut Vec<Diagnostic>) {
    // Scope → the names visible in it: definitions and import bindings.
    let mut scopes: HashMap<&[String], Vec<(&str, Span)>> = HashMap::new();
    for (scope, defs) in &ctx.scopes.defs {
        for (bare, full) in defs {
            if let Some(f) = ctx.ast.functions.iter().find(|f| &f.name == full) {
                scopes.entry(scope).or_default().push((bare, f.name_span));
            }
        }
    }
    for (scope, bindings) in &ctx.scopes.bindings {
        for (bare, (idx, _path)) in bindings {
            if let Some(imp) = ctx.ast.imports.get(*idx) {
                scopes.entry(scope).or_default().push((bare, imp.span));
            }
        }
    }
    for names in scopes.values_mut() {
        names.sort_by_key(|(_, span)| span.start); // source order
        let mut by_norm: HashMap<String, (&str, Span)> = HashMap::new();
        for &(raw, span) in names.iter() {
            let norm = normalize(raw);
            match by_norm.get(&norm) {
                Some(&(first_raw, first_span)) if first_raw != raw => {
                    out.push(Diagnostic {
                        code: "confusable-names",
                        span,
                        message: format!(
                            "'{raw}' is confusable with '{first_raw}' (defined at line {})",
                            first_span.start.line
                        ),
                        fix: None,
                    });
                }
                Some(_) => {} // same raw name (e.g. def + its own re-listing)
                None => {
                    by_norm.insert(norm, (raw, span));
                }
            }
        }
    }
}
```

Register (`pub(crate) mod confusable_names;`; RULES gains `("confusable-names", rules::confusable_names::check)`). The full table is now:

```rust
pub(crate) const RULES: &[(&'static str, fn(&LintContext, &mut Vec<Diagnostic>))] = &[
    ("line-too-long", rules::line_too_long::check),
    ("leading-zeros", rules::leading_zeros::check),
    ("unused-label", rules::unused_label::check),
    ("redundant-jump-to-next", rules::redundant_jump::check),
    ("identical-check-arms", rules::identical_check_arms::check),
    ("leftover-debugger", rules::leftover_debugger::check),
    ("namespaced-main", rules::namespaced_main::check),
    ("shadowed-import", rules::shadowed_import::check),
    ("non-camel-case", rules::non_camel_case::check),
    ("confusable-names", rules::confusable_names::check),
];
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p mtc-post-machine lint`
Expected: all pass — all 10 rules registered.

- [ ] **Step 5: Quality gates + commit**

```bash
cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --check
git add crates/post-machine/src/lint
git commit -m "feat(post-machine): confusable-names lint rule — catalog complete"
```

---

### Task 12: `apply_fixes` — the batch edit applier

**Files:**
- Create: `crates/post-machine/src/lint/fixes.rs`
- Modify: `crates/post-machine/src/lint/mod.rs` (`pub mod fixes;` + re-export), `crates/post-machine/src/lib.rs` (re-export `apply_fixes`, `FixOutcome`)

**Interfaces:**
- Consumes: `Diagnostic { fix: Option<Fix> }`, `Fix { edits }`, `Edit { span, replacement }`, `Pos`.
- Produces: `pub fn apply_fixes(source: &str, diagnostics: &[Diagnostic]) -> FixOutcome`; `pub struct FixOutcome { pub fixed_source: String, pub applied: usize, pub skipped: usize }` (Task 15's CLI consumes both).

- [ ] **Step 1: Write the failing tests**

`crates/post-machine/src/lint/fixes.rs`:

```rust
#[cfg(test)]
mod tests {
    use mtc_core::diagnostics::{Applicability, Diagnostic, Edit, Fix, Span};

    use super::*;

    fn fix_diag(span: Span, replacement: &str) -> Diagnostic {
        Diagnostic {
            code: "test",
            span,
            message: "test".into(),
            fix: Some(Fix {
                description: "test".into(),
                applicability: Applicability::MachineApplicable,
                edits: vec![Edit {
                    span,
                    replacement: replacement.into(),
                }],
            }),
        }
    }

    #[test]
    fn deletes_and_replaces() {
        let src = "5: right;\n";
        // Delete the `5:` prefix (cols 1..3).
        let out = apply_fixes(src, &[fix_diag(Span::new(1, 1, 1, 3), "")]);
        assert_eq!(out.fixed_source, " right;\n");
        assert_eq!((out.applied, out.skipped), (1, 0));

        // Replace `right` with `left` (cols 4..9).
        let out = apply_fixes(src, &[fix_diag(Span::new(1, 4, 1, 9), "left")]);
        assert_eq!(out.fixed_source, "5: left;\n");
    }

    #[test]
    fn two_disjoint_fixes_apply_bottom_up() {
        let src = "007: right;\ngoto 007;\n";
        let fixes = [
            fix_diag(Span::new(1, 1, 1, 4), "7"),
            fix_diag(Span::new(2, 6, 2, 9), "7"),
        ];
        let out = apply_fixes(src, &fixes);
        assert_eq!(out.fixed_source, "7: right;\ngoto 7;\n");
        assert_eq!((out.applied, out.skipped), (2, 0));
    }

    #[test]
    fn overlapping_fix_is_skipped_whole() {
        let src = "abcdef\n";
        let fixes = [
            fix_diag(Span::new(1, 1, 1, 4), "X"), // abc -> X
            fix_diag(Span::new(1, 3, 1, 6), "Y"), // cde overlaps -> skipped
        ];
        let out = apply_fixes(src, &fixes);
        assert_eq!(out.fixed_source, "Xdef\n");
        assert_eq!((out.applied, out.skipped), (1, 1));
    }

    #[test]
    fn diagnostics_without_fixes_are_ignored() {
        let src = "x\n";
        let d = Diagnostic {
            code: "test",
            span: Span::new(1, 1, 1, 2),
            message: "no fix".into(),
            fix: None,
        };
        let out = apply_fixes(src, &[d]);
        assert_eq!(out.fixed_source, src);
        assert_eq!((out.applied, out.skipped), (0, 0));
    }

    #[test]
    fn char_positions_survive_unicode() {
        // Cyrillic chars are 2 bytes each; spans are char-counted.
        let src = "жж 007;\n";
        let out = apply_fixes(src, &[fix_diag(Span::new(1, 4, 1, 7), "7")]);
        assert_eq!(out.fixed_source, "жж 7;\n");
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p mtc-post-machine fixes`
Expected: compile error — `apply_fixes` does not exist.

- [ ] **Step 3: Implement**

Top of `crates/post-machine/src/lint/fixes.rs`:

```rust
//! Fix application (docs/lint.md, docs/cli.md): one batch pass against
//! original-source coordinates — no re-analysis between edits. A `Fix`'s
//! edits apply atomically; a fix overlapping an already-kept fix is
//! skipped whole and counted. The CLI re-lints the fixed source and
//! reports from the re-run (cascades are reported, not looped).

use mtc_core::diagnostics::{Diagnostic, Pos};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FixOutcome {
    pub fixed_source: String,
    pub applied: usize,
    pub skipped: usize,
}

/// Char-counted (line, col) → byte offset; end-of-input if past the end.
fn byte_offset(source: &str, pos: Pos) -> usize {
    let (mut line, mut col) = (1u32, 1u32);
    for (i, c) in source.char_indices() {
        if line == pos.line && col == pos.col {
            return i;
        }
        if c == '\n' {
            line += 1;
            col = 1;
        } else {
            col += 1;
        }
    }
    source.len()
}

pub fn apply_fixes(source: &str, diagnostics: &[Diagnostic]) -> FixOutcome {
    // Phase 1 — plan: keep each fix whose edits overlap no kept edit.
    let mut kept_edits: Vec<(usize, usize, String)> = Vec::new();
    let mut kept_ranges: Vec<(usize, usize)> = Vec::new();
    let (mut applied, mut skipped) = (0usize, 0usize);
    for d in diagnostics {
        let Some(fix) = &d.fix else { continue };
        let ranges: Vec<(usize, usize)> = fix
            .edits
            .iter()
            .map(|e| {
                (
                    byte_offset(source, e.span.start),
                    byte_offset(source, e.span.end),
                )
            })
            .collect();
        let overlaps = ranges
            .iter()
            .any(|&(s, e)| kept_ranges.iter().any(|&(ks, ke)| s < ke && ks < e));
        if overlaps {
            skipped += 1;
            continue;
        }
        for (&(s, e), edit) in ranges.iter().zip(&fix.edits) {
            kept_edits.push((s, e, edit.replacement.clone()));
        }
        kept_ranges.extend(ranges);
        applied += 1;
    }
    // Phase 2 — apply bottom-up: descending start keeps every pending
    // (lower) offset valid; kept edits are pairwise disjoint by phase 1.
    kept_edits.sort_by_key(|&(s, _, _)| std::cmp::Reverse(s));
    let mut fixed_source = source.to_string();
    for (s, e, rep) in kept_edits {
        fixed_source.replace_range(s..e, &rep);
    }
    FixOutcome {
        fixed_source,
        applied,
        skipped,
    }
}
```

In `crates/post-machine/src/lint/mod.rs` add:

```rust
pub mod fixes;

pub use fixes::{FixOutcome, apply_fixes};
```

In `crates/post-machine/src/lib.rs` extend the lint re-export:

```rust
pub use lint::{FixOutcome, LintError, LintOptions, LintReport, apply_fixes, lint};
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p mtc-post-machine fixes`
Expected: 5 passed.

- [ ] **Step 5: Quality gates + commit**

```bash
cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --check
git add crates/post-machine/src/lint crates/post-machine/src/lib.rs
git commit -m "feat(post-machine): apply_fixes batch edit applier"
```

---

### Task 13: CLI `pmt lint` — single file, `--allow`, rendering, exit codes

**Files:**
- Create: `crates/post-machine/src/cli/lint.rs`
- Modify: `crates/post-machine/src/cli/mod.rs` (`mod lint;`, dispatch arm, USAGE line)
- Test: `crates/post-machine/tests/cli_programs.rs` (append)

**Interfaces:**
- Consumes: `crate::lint::{lint, LintOptions, LintError}`, `Diagnostic`, `Applicability`, `Args` helpers (`cli/mod.rs`), `CliOutput`.
- Produces: `pub(super) fn lint(raw: &[String]) -> Result<CliOutput, String>` dispatched from `Some("lint")`; render helper `render_findings(out: &mut String, path: &Path, diags: &[Diagnostic])` (Tasks 14–15 reuse it).

- [ ] **Step 1: Write the failing tests**

Append to `crates/post-machine/tests/cli_programs.rs` (reuse the existing `args()` / `scratch()` helpers at the top of that file):

```rust
#[test]
fn lint_reports_findings_with_exit_1_and_fix_hints() {
    let dir = scratch("lint_single");
    let src = dir.join("prog.pmc");
    std::fs::write(&src, "main() {\n5: right;\n007: left;\n    goto 007;\n}\n").unwrap();
    let out = execute(&args(&["lint", src.to_str().unwrap()])).unwrap();
    assert_eq!(out.code, 1);
    // unused-label on `5:` — gated fix hint.
    assert!(out.stdout.contains("lint: label 5 is never referenced"));
    assert!(out.stdout.contains("fix (requires --force): remove the label prefix '5:'"));
    // leading-zeros — safe-tier fix hint.
    assert!(out.stdout.contains("has leading zeros"));
    assert!(out.stdout.contains("  fix: rewrite '007' as '7'"));
}

#[test]
fn lint_clean_file_exits_0_and_allow_suppresses() {
    let dir = scratch("lint_clean");
    let src = dir.join("prog.pmc");
    std::fs::write(&src, "main() { right; }\n").unwrap();
    let out = execute(&args(&["lint", src.to_str().unwrap()])).unwrap();
    assert_eq!(out.code, 0);
    assert!(out.stdout.is_empty());

    let dirty = dir.join("dirty.pmc");
    std::fs::write(&dirty, "main() {\n5: right;\n}\n").unwrap();
    let out = execute(&args(&[
        "lint",
        dirty.to_str().unwrap(),
        "--allow",
        "unused-label",
    ]))
    .unwrap();
    assert_eq!(out.code, 0);
}

#[test]
fn lint_unknown_allow_code_is_a_tool_error() {
    let dir = scratch("lint_badallow");
    let src = dir.join("prog.pmc");
    std::fs::write(&src, "main() { right; }\n").unwrap();
    let err = execute(&args(&[
        "lint",
        src.to_str().unwrap(),
        "--allow",
        "no-such-rule",
    ]))
    .unwrap_err();
    assert!(err.contains("no-such-rule"));
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p mtc-post-machine --test cli_programs lint`
Expected: FAIL — `unknown subcommand \`lint\``.

- [ ] **Step 3: Implement**

`crates/post-machine/src/cli/lint.rs`:

```rust
//! `pmt lint` (docs/cli.md, docs/lint.md): thin renderer over the lint
//! library. Findings go to stdout; exit 0 = clean, 1 = findings or
//! errors anywhere.

use std::fmt::Write as _;
use std::fs;
use std::path::Path;

use mtc_core::diagnostics::{Applicability, Diagnostic};

use crate::lint::{LintOptions, lint as lint_source};

use super::{Args, CliOutput};

const LINT_USAGE: &str = "\
USAGE: pmt lint PATH [--allow CODE]...

FLAGS:
  --allow CODE   suppress a lint rule by code (repeatable;
                 unknown codes are an error)
";

pub(super) fn lint(raw: &[String]) -> Result<CliOutput, String> {
    let mut args = Args::new(raw);
    if args.flag("--help") {
        return Ok(CliOutput::ok(LINT_USAGE.into(), String::new()));
    }
    let allow = args.values("--allow")?;
    let paths = args.positionals()?;
    let [path] = paths.as_slice() else {
        return Err(format!("lint takes exactly one input\n\n{LINT_USAGE}"));
    };
    let path = Path::new(path);

    let source =
        fs::read_to_string(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let report = lint_source(&source, LintOptions { allow }).map_err(|e| e.to_string())?;

    let mut stdout = String::new();
    render_findings(&mut stdout, path, &report.diagnostics);
    let code = u8::from(!report.diagnostics.is_empty());
    Ok(CliOutput {
        stdout,
        stderr: String::new(),
        code,
    })
}

/// `{file}:{line}:{col}: lint: {message}` plus an indented fix-hint line;
/// a gated fix names its gate so plain `--fix` runs explain themselves.
pub(super) fn render_findings(out: &mut String, path: &Path, diags: &[Diagnostic]) {
    for d in diags {
        let _ = writeln!(
            out,
            "{}:{}:{}: lint: {}",
            path.display(),
            d.span.start.line,
            d.span.start.col,
            d.message
        );
        if let Some(fix) = &d.fix {
            let _ = match fix.applicability {
                Applicability::MachineApplicable => {
                    writeln!(out, "  fix: {}", fix.description)
                }
                Applicability::MaybeIncorrect => {
                    writeln!(out, "  fix (requires --force): {}", fix.description)
                }
            };
        }
    }
}
```

In `crates/post-machine/src/cli/mod.rs`:

```rust
mod build;
mod inspect;
mod lint;
mod run;
```

Add the dispatch arm after `Some("link")`:

```rust
Some("lint") => lint::lint(&args[1..]),
```

Add to the USAGE subcommand list (after `link`):

```
  lint     lint .pmc sources (hygiene findings; docs/lint.md)
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p mtc-post-machine --test cli_programs lint`
Expected: 3 passed.

- [ ] **Step 5: Quality gates + commit**

```bash
cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --check
git add crates/post-machine/src/cli crates/post-machine/tests/cli_programs.rs
git commit -m "feat(cli): pmt lint subcommand — single file, --allow, fix hints"
```

---

### Task 14: Batch model — `PATH...`, directory walk, `--exclude`

**Files:**
- Modify: `crates/post-machine/src/cli/lint.rs` (walk + batch loop; usage text)
- Test: `crates/post-machine/tests/cli_programs.rs` (append)

**Interfaces:**
- Consumes: Task 13's `render_findings`; `LintError`.
- Produces: `collect_pmc(path: &Path, excludes: &[PathBuf], out: &mut Vec<PathBuf>) -> Result<usize, String>` (returns files discovered for this PATH, pre-exclusion — zero is the caller's typo error); batch loop Task 15 extends.

- [ ] **Step 1: Write the failing tests**

Append to `crates/post-machine/tests/cli_programs.rs`:

```rust
#[test]
fn lint_walks_directories_sorted_skips_dot_dirs_and_excludes() {
    let dir = scratch("lint_walk");
    std::fs::create_dir_all(dir.join("src/nested")).unwrap();
    std::fs::create_dir_all(dir.join(".hidden")).unwrap();
    std::fs::create_dir_all(dir.join("vendor")).unwrap();
    // b before a alphabetically reversed on disk creation order.
    std::fs::write(dir.join("src/b.pmc"), "main() {\n5: right;\n}\n").unwrap();
    std::fs::write(dir.join("src/a.pmc"), "a() {\n6: right;\n}\nmain() { @a(); }\n").unwrap();
    std::fs::write(dir.join("src/nested/c.pmc"), "c() {\n7: right;\n}\nmain() { @c(); }\n").unwrap();
    std::fs::write(dir.join(".hidden/d.pmc"), "main() {\n8: right;\n}\n").unwrap();
    std::fs::write(dir.join("vendor/e.pmc"), "main() {\n9: right;\n}\n").unwrap();

    let out = execute(&args(&[
        "lint",
        dir.to_str().unwrap(),
        "--exclude",
        dir.join("vendor").to_str().unwrap(),
    ]))
    .unwrap();
    assert_eq!(out.code, 1);
    // Sorted walk: a.pmc findings before b.pmc, nested/c.pmc last.
    let a = out.stdout.find("a.pmc").unwrap();
    let b = out.stdout.find("b.pmc").unwrap();
    let c = out.stdout.find("c.pmc").unwrap();
    assert!(a < b && b < c);
    // Dot-dir and excluded subtree never appear.
    assert!(!out.stdout.contains(".hidden"));
    assert!(!out.stdout.contains("vendor"));
}

#[test]
fn lint_zero_match_path_is_an_error() {
    let dir = scratch("lint_zero");
    std::fs::create_dir_all(dir.join("empty")).unwrap();
    let err = execute(&args(&["lint", dir.join("empty").to_str().unwrap()])).unwrap_err();
    assert!(err.contains("no .pmc files"));
}

#[test]
fn lint_batch_survives_a_fatal_file_and_still_fails() {
    let dir = scratch("lint_fatal");
    std::fs::write(dir.join("bad.pmc"), "main( {\n").unwrap();
    std::fs::write(dir.join("good.pmc"), "main() { right; }\n").unwrap();
    let out = execute(&args(&[
        "lint",
        dir.join("bad.pmc").to_str().unwrap(),
        dir.join("good.pmc").to_str().unwrap(),
    ]))
    .unwrap();
    assert_eq!(out.code, 1);
    assert!(out.stderr.contains("error:"));
    assert!(out.stderr.contains("bad.pmc"));
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p mtc-post-machine --test cli_programs lint_walks`
Expected: FAIL — `lint takes exactly one input`.

- [ ] **Step 3: Implement**

Rework `crates/post-machine/src/cli/lint.rs` — usage grows, and the single-path body becomes a walk + batch loop:

```rust
const LINT_USAGE: &str = "\
USAGE: pmt lint PATH... [--exclude PATH]... [--allow CODE]...

PATH is a .pmc file or a directory; directories are walked recursively
for *.pmc (sorted order, symlinks not followed, dot-entries skipped).

FLAGS:
  --exclude PATH  skip a file or prune a directory subtree (repeatable;
                  plain paths compared as spelled — no globs); exclusion
                  wins even over explicitly listed files
  --allow CODE    suppress a lint rule by code (repeatable;
                  unknown codes are an error)
";

pub(super) fn lint(raw: &[String]) -> Result<CliOutput, String> {
    let mut args = Args::new(raw);
    if args.flag("--help") {
        return Ok(CliOutput::ok(LINT_USAGE.into(), String::new()));
    }
    let allow = args.values("--allow")?;
    let excludes: Vec<PathBuf> = args.values("--exclude")?.into_iter().map(PathBuf::from).collect();
    let paths = args.positionals()?;
    if paths.is_empty() {
        return Err(format!("lint takes at least one PATH\n\n{LINT_USAGE}"));
    }

    let mut files: Vec<PathBuf> = Vec::new();
    for p in &paths {
        let path = Path::new(p);
        let found = collect_pmc(path, &excludes, &mut files)?;
        if found == 0 {
            return Err(format!("{p}: no .pmc files found"));
        }
    }

    let mut stdout = String::new();
    let mut stderr = String::new();
    let mut any = false;
    for file in &files {
        let source = fs::read_to_string(file)
            .map_err(|e| format!("cannot read {}: {e}", file.display()))?;
        match lint_source(&source, LintOptions { allow: allow.clone() }) {
            Ok(report) => {
                if !report.diagnostics.is_empty() {
                    any = true;
                }
                render_findings(&mut stdout, file, &report.diagnostics);
            }
            Err(LintError::Compile(e)) => {
                // Per-file fatal: report, keep going (batch model).
                any = true;
                let _ = writeln!(
                    stderr,
                    "{}:{}:{}: error: {}",
                    file.display(),
                    e.span.start.line,
                    e.span.start.col,
                    e.kind
                );
            }
            Err(e @ LintError::UnknownAllowCode(_)) => return Err(e.to_string()),
        }
    }
    Ok(CliOutput {
        stdout,
        stderr,
        code: u8::from(any),
    })
}

/// Walk one PATH argument. Returns how many `.pmc` files the PATH
/// yielded BEFORE exclusion (zero = the caller's typo error); excluded
/// files are counted but not collected — an excluded PATH is not a typo.
fn collect_pmc(
    path: &Path,
    excludes: &[PathBuf],
    out: &mut Vec<PathBuf>,
) -> Result<usize, String> {
    let excluded = |p: &Path| excludes.iter().any(|e| p.starts_with(e));
    let meta = fs::symlink_metadata(path)
        .map_err(|e| format!("cannot stat {}: {e}", path.display()))?;
    if meta.is_symlink() {
        return Ok(0); // never followed
    }
    if meta.is_file() {
        // An explicit file is linted as given (any extension) unless excluded.
        if !excluded(path) {
            out.push(path.to_path_buf());
        }
        return Ok(1);
    }
    if excluded(path) {
        return Ok(1); // pruned subtree still "matched" — not a typo
    }
    let mut entries: Vec<_> = fs::read_dir(path)
        .map_err(|e| format!("cannot read {}: {e}", path.display()))?
        .collect::<Result<_, _>>()
        .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    entries.sort_by_key(|e| e.file_name());
    let mut found = 0usize;
    for entry in entries {
        let name = entry.file_name();
        if name.to_string_lossy().starts_with('.') {
            continue; // dot-entries: .git, scratch dirs
        }
        let child = entry.path();
        let meta = fs::symlink_metadata(&child)
            .map_err(|e| format!("cannot stat {}: {e}", child.display()))?;
        if meta.is_symlink() {
            continue;
        }
        if meta.is_dir() {
            found += collect_pmc(&child, excludes, out)?;
        } else if child.extension().is_some_and(|x| x == "pmc") {
            found += 1;
            if !excluded(&child) {
                out.push(child);
            }
        }
    }
    Ok(found)
}
```

Add the needed imports at the top: `use std::path::PathBuf;` and `use crate::lint::LintError;`.

- [ ] **Step 4: Run tests**

Run: `cargo test -p mtc-post-machine --test cli_programs lint`
Expected: all lint CLI tests pass (Task 13's single-file tests still pass — one file is a one-element batch).

- [ ] **Step 5: Quality gates + commit**

```bash
cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --check
git add crates/post-machine/src/cli crates/post-machine/tests/cli_programs.rs
git commit -m "feat(cli): lint batch model — directory walk and --exclude"
```

---

### Task 15: `--fix` / `--force`

**Files:**
- Modify: `crates/post-machine/src/cli/lint.rs` (flags, tier filter, write + re-lint; usage text)
- Test: `crates/post-machine/tests/cli_programs.rs` (append)

**Interfaces:**
- Consumes: `apply_fixes`, `FixOutcome`, `Applicability`; `Diagnostic` is `Clone` (provided by the foundation plan's derives).
- Produces: final CLI behavior per spec §3; nothing downstream.

- [ ] **Step 1: Write the failing tests**

Append to `crates/post-machine/tests/cli_programs.rs`:

```rust
#[test]
fn fix_applies_safe_tier_only_and_force_unlocks_deletions() {
    let dir = scratch("lint_fix");
    let src = dir.join("prog.pmc");
    let original = "main() {\n5: right;\n    goto 007;\n7: left;\n}\n";
    std::fs::write(&src, original).unwrap();

    // Plain --fix: leading-zeros applied, unused-label deletion left.
    let out = execute(&args(&["lint", src.to_str().unwrap(), "--fix"])).unwrap();
    let fixed = std::fs::read_to_string(&src).unwrap();
    assert!(fixed.contains("goto 7;"), "safe tier applied");
    assert!(fixed.contains("5: right;"), "gated deletion NOT applied");
    assert_eq!(out.code, 1, "the unused label remains a finding");

    // --fix --force: the unused-label prefix goes too; re-run is clean.
    let out = execute(&args(&["lint", src.to_str().unwrap(), "--fix", "--force"])).unwrap();
    assert_eq!(out.code, 0);
    let fixed = std::fs::read_to_string(&src).unwrap();
    assert!(!fixed.contains("5:"));

    // Idempotence: a second forced run changes nothing and stays clean.
    let before = std::fs::read_to_string(&src).unwrap();
    let out = execute(&args(&["lint", src.to_str().unwrap(), "--fix", "--force"])).unwrap();
    assert_eq!(out.code, 0);
    assert_eq!(std::fs::read_to_string(&src).unwrap(), before);
}

#[test]
fn force_without_fix_errors_and_fatal_files_are_never_written() {
    let dir = scratch("lint_force");
    let src = dir.join("prog.pmc");
    std::fs::write(&src, "main() { right; }\n").unwrap();
    let err = execute(&args(&["lint", src.to_str().unwrap(), "--force"])).unwrap_err();
    assert!(err.contains("--force requires --fix"));

    let bad = dir.join("bad.pmc");
    let broken = "main( {\n";
    std::fs::write(&bad, broken).unwrap();
    let out = execute(&args(&["lint", bad.to_str().unwrap(), "--fix", "--force"])).unwrap();
    assert_eq!(out.code, 1);
    assert_eq!(std::fs::read_to_string(&bad).unwrap(), broken, "never written");
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p mtc-post-machine --test cli_programs fix_applies`
Expected: FAIL — `unknown flag \`--fix\``.

- [ ] **Step 3: Implement**

In `crates/post-machine/src/cli/lint.rs`: extend the usage —

```rust
const LINT_USAGE: &str = "\
USAGE: pmt lint PATH... [--exclude PATH]... [--allow CODE]... [--fix [--force]]

PATH is a .pmc file or a directory; directories are walked recursively
for *.pmc (sorted order, symlinks not followed, dot-entries skipped).

FLAGS:
  --exclude PATH  skip a file or prune a directory subtree (repeatable;
                  plain paths compared as spelled — no globs); exclusion
                  wins even over explicitly listed files
  --allow CODE    suppress a lint rule by code (repeatable;
                  unknown codes are an error)
  --fix           apply machine-applicable fixes in place, then re-lint;
                  the report and exit code reflect what REMAINS
  --force         with --fix: also apply the gated fixes (deletions and
                  rewrites whose diagnosis may have another reading)
";
```

parse the flags after `--allow`:

```rust
    let fix = args.flag("--fix");
    let force = args.flag("--force");
    if force && !fix {
        return Err(format!("--force requires --fix\n\n{LINT_USAGE}"));
    }
```

and replace the batch loop's `Ok(report)` arm:

```rust
            Ok(report) => {
                let diags = if fix {
                    // Mask fixes outside the allowed tier, apply, rewrite,
                    // then re-lint: the report reflects what REMAINS.
                    let masked: Vec<Diagnostic> = report
                        .diagnostics
                        .iter()
                        .cloned()
                        .map(|mut d| {
                            let gated = matches!(
                                d.fix.as_ref().map(|f| &f.applicability),
                                Some(Applicability::MaybeIncorrect)
                            );
                            if gated && !force {
                                d.fix = None;
                            }
                            d
                        })
                        .collect();
                    let outcome = apply_fixes(&source, &masked);
                    if outcome.applied > 0 {
                        fs::write(file, &outcome.fixed_source)
                            .map_err(|e| format!("cannot write {}: {e}", file.display()))?;
                        match lint_source(
                            &outcome.fixed_source,
                            LintOptions { allow: allow.clone() },
                        ) {
                            Ok(rerun) => rerun.diagnostics,
                            Err(e) => return Err(e.to_string()),
                        }
                    } else {
                        report.diagnostics
                    }
                } else {
                    report.diagnostics
                };
                if !diags.is_empty() {
                    any = true;
                }
                render_findings(&mut stdout, file, &diags);
            }
```

Add imports: `use mtc_core::diagnostics::Diagnostic;` and `use crate::lint::apply_fixes;`.

Note the masking preserves the policy line exactly: with plain `--fix`, gated fixes are stripped BEFORE `apply_fixes`, so the hint still renders (`requires --force`) from the re-lint, and nothing gated is ever applied.

- [ ] **Step 4: Run tests**

Run: `cargo test -p mtc-post-machine --test cli_programs lint && cargo test -p mtc-post-machine --test cli_programs fix`
Expected: all pass.

- [ ] **Step 5: Quality gates + commit**

```bash
cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --check
git add crates/post-machine/src/cli crates/post-machine/tests/cli_programs.rs
git commit -m "feat(cli): lint --fix/--force — tiered application with re-lint reporting"
```

---

### Task 16: Integration suite, acceptance fixture, stdlib dogfood

**Files:**
- Create: `crates/post-machine/tests/lint_programs.rs`
- Create: `crates/post-machine/tests/lint/unused_labels.pmc` (committed fixture)

**Interfaces:**
- Consumes: `mtc_post_machine::{lint, LintOptions, apply_fixes}` (public API only — this is an integration test).
- Produces: the acceptance-criteria evidence (spec criteria 1 and 4).

- [ ] **Step 1: Commit the fixture**

`crates/post-machine/tests/lint/unused_labels.pmc` — reproduces the two unused-label shapes from the 8-bit summing showcase (a label on a single-statement helper body; a label on a fall-through-only statement after a call). The showcase itself is a local untracked artifact; this fixture is the committed stand-in:

```
// Lint fixture: unused-label shapes from the 8-bit summing showcase.
main() {
    @helper();
1:  check(2, 3);
2:  right;
    goto 4;
3:  left;
4:  halt;
}

helper() {
5:  right;
}
```

Expected findings: `unused-label` on `1:` (line 4 — fall-through-only statement after a call) and on `5:` (line 12 — single-statement helper body). Nothing else fires: all other labels are referenced, no jumps target the lexically next statement, arms differ, names are camelCase, lines are short.

- [ ] **Step 2: Write the failing integration tests**

`crates/post-machine/tests/lint_programs.rs`:

```rust
//! Lint integration: multi-rule ordering, --allow filtering, fix
//! round-trips, the acceptance fixture, and the stdlib dogfood.

use mtc_post_machine::{LintOptions, apply_fixes, lint};

const FIXTURE: &str = include_str!("lint/unused_labels.pmc");

#[test]
fn fixture_yields_exactly_the_two_showcase_findings() {
    let report = lint(FIXTURE, LintOptions::default()).unwrap();
    let codes: Vec<_> = report
        .diagnostics
        .iter()
        .map(|d| (d.code, d.span.start.line))
        .collect();
    assert_eq!(codes, vec![("unused-label", 4), ("unused-label", 12)]);
}

#[test]
fn fixture_fixes_apply_cleanly_and_idempotently() {
    let report = lint(FIXTURE, LintOptions::default()).unwrap();
    let outcome = apply_fixes(FIXTURE, &report.diagnostics);
    assert_eq!((outcome.applied, outcome.skipped), (2, 0));
    assert!(!outcome.fixed_source.contains("1:  check"));
    assert!(!outcome.fixed_source.contains("5:  right"));
    // Idempotence: the fixed source re-lints clean.
    let rerun = lint(&outcome.fixed_source, LintOptions::default()).unwrap();
    assert!(rerun.diagnostics.is_empty());
}

#[test]
fn findings_are_source_ordered_across_rules() {
    let src = "\
main() {
007: right;
5:   left;
     goto 007;
     debugger;
}
";
    let report = lint(src, LintOptions::default()).unwrap();
    let lines: Vec<u32> = report.diagnostics.iter().map(|d| d.span.start.line).collect();
    let mut sorted = lines.clone();
    sorted.sort();
    assert_eq!(lines, sorted);
    let codes: Vec<_> = report.diagnostics.iter().map(|d| d.code).collect();
    // leading-zeros twice (definition + goto), unused-label (5), debugger.
    assert!(codes.contains(&"leading-zeros"));
    assert!(codes.contains(&"unused-label"));
    assert!(codes.contains(&"leftover-debugger"));
}

#[test]
fn allow_filters_a_rule_out() {
    let report = lint(
        FIXTURE,
        LintOptions {
            allow: vec!["unused-label".into()],
        },
    )
    .unwrap();
    assert!(report.diagnostics.is_empty());
}

#[test]
fn stdlib_dogfoods_clean() {
    let std_pmc = include_str!("../src/stdlib/std.pmc");
    let report = lint(std_pmc, LintOptions::default()).unwrap();
    assert!(
        report.diagnostics.is_empty(),
        "stdlib must lint clean, found: {:?}",
        report
            .diagnostics
            .iter()
            .map(|d| (d.code, d.span.start.line))
            .collect::<Vec<_>>()
    );
}
```

- [ ] **Step 3: Run — fix whatever the dogfood surfaces**

Run: `cargo test -p mtc-post-machine --test lint_programs`
Expected: the fixture and ordering tests pass immediately. If `stdlib_dogfoods_clean` fails, **fix `crates/post-machine/src/stdlib/std.pmc`** (remove unused labels / rename as findings dictate) — then re-run the FULL suite: the stdlib golden and e2e tests must stay green (`cargo test -p mtc-post-machine`), proving the cleanup changed no behavior. If the fixture line numbers drifted from the file as committed, fix the test constants, not the fixture.

- [ ] **Step 4: Run the whole workspace**

Run: `cargo test --workspace`
Expected: everything green, including golden and stdlib suites.

- [ ] **Step 5: Quality gates + commit**

```bash
cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --check
git add crates/post-machine/tests/lint_programs.rs crates/post-machine/tests/lint crates/post-machine/src/stdlib/std.pmc
git commit -m "test(post-machine): lint integration suite, acceptance fixture, stdlib dogfood"
```

(Drop `src/stdlib/std.pmc` from the `git add` if the dogfood needed no fix.)

---

### Task 17: Documentation

**Files:**
- Create: `docs/lint.md`
- Modify: `docs/cli.md` (new `pmt lint` section), `README.md` (CLI overview line), `docs/language.md` (warnings paragraph pointer)

**Interfaces:**
- Consumes: the shipped behavior of Tasks 1–16.
- Produces: the durable reference pages (spec §6). Published docs are forge-agnostic — no issue/PR numbers, no host URLs.

- [ ] **Step 1: Write `docs/lint.md`**

Full page (rule catalog — code, semantics, example, fix behavior, `--allow`):

~~~markdown
# Linting `.pmc` — `pmt lint`

`pmt lint` reports hygiene findings the compiler deliberately does not
warn about. It runs the compiler's analysis (through lowering, no code
generation), applies the rule catalog below, and prints one finding per
line as `FILE:LINE:COL: lint: MESSAGE`. Exit code 0 means every file is
clean; 1 means findings (or errors) somewhere. Lint reports lint
findings only — compile warnings stay on `pmt compile`.

Suppress a rule with `--allow CODE` (repeatable). Unknown codes are an
error, so a typo cannot silently disable linting.

## Fixes

Findings may carry a machine-applicable fix, shown as an indented hint.
`--fix` applies the safe tier and rewrites the file in place; fixes that
delete or rewrite constructs on an ambiguous diagnosis are gated behind
`--fix --force` and their hints say so. After fixing, the file is linted
again — the report and the exit code describe what REMAINS. Applying a
fix can expose a new finding (deleting a redundant goto can leave its
target label unused); the re-run reports it, and repeating `--fix
--force` converges.

## Rules

### unused-label

A label is unused iff nothing in its function references it: no `goto`,
no check arm, no command successor. Labels cost zero bytes in the
binary, so this is pure source hygiene. A label on a single-statement
body is an instance of this rule, not a special case: unreferenced means
caught here; referenced means a self-loop that cannot be removed.

Fix (requires `--force`): delete the `N:` prefix. Review findings before
forcing — an unused label sometimes marks a jump you forgot to write,
and the fix removes the label, not the underlying omission.

### shadowed-import

A function definition outranks an import binding of the same bare name
in the same scope. Legal — definitions always win — but a bare `@name()`
call silently resolves to the local function while the `use` line
suggests the external. Cross-scope shadowing (inner over outer) is legal
layering and is not flagged. No fix: renaming either side is plausible.

### redundant-jump-to-next

A `goto N;` statement, or a `(N)` successor, whose target labels the
lexically next statement — fall-through is identical. Fix (requires
`--force`): delete the jump. The statement form is fixable only when the
`goto` statement carries no labels of its own (deleting a labeled
statement would orphan references to it).

### identical-check-arms

`check(N, N)`: both arms land in the same place, so the branch is
unconditional — `goto N` was meant, or one arm is a typo. `check(!, !)`
is exempt: the language has no `return` keyword, and identical-`!` arms
are its only pure mid-function return. Fix (requires `--force`,
standalone statements only): replace with `goto N`. A group-final check
is report-only — `goto` cannot appear in comma groups.

### leftover-debugger

A `debugger` statement in source. Builds strip breakpoints with
`--strip-debugger`, and an un-stripped `brk` is an optimizer
observability barrier — shipping one pessimizes `-O1` output. Fix
(requires `--force`): delete a lone, unlabeled `debugger;` statement;
labeled or comma-grouped occurrences are report-only.

### namespaced-main

A function named `main` inside a namespace is not the program entry
(only the un-namespaced top-level `main` is) and is not auto-exported —
it silently becomes an ordinary local function. No fix: rename it or
move it out.

### line-too-long

A line longer than 80 characters (character count). Report-only: where
to break a statement is layout policy, a formatter's job. The limit is
fixed at 80.

### leading-zeros

A numeric token written with leading zeros: `007:`, `goto 007`, check
arms, call successors. Digit runs parse straight to a number, so `007`
and `7` denote the same label while looking unrelated — and `07:` next
to `7:` is a puzzling duplicate-label error. Fix (safe tier): rewrite
the token to its canonical decimal form.

### non-camel-case

Definition names the user owns — functions, namespaces, import
bindings — should be lowerCamelCase, the project's house style. The
message carries a mechanically derived rename suggestion; an import
binding's suggestion is an `as` alias. Report-only: a rename is a
multi-site edit, and renaming an exported function changes its symbol
name. The most opinionated rule in the set — `--allow non-camel-case`
is the escape hatch (note that non-ASCII identifiers, which the
language permits, do not satisfy the ASCII convention).

### confusable-names

Two definitions or bindings in the same scope whose names differ only
under a confusability normalization (case, underscores, `1`/`l`,
`i`/`l`, `0`/`o`): `sum_bits` vs `sumBits`, `fool` vs `foo1`. Reported
at the later definition, naming the earlier one. No fix.
~~~

- [ ] **Step 2: Extend `docs/cli.md`**

Append a `## pmt lint` section (mirroring the existing per-subcommand structure — verbatim usage block plus prose):

~~~markdown
## pmt lint

```
USAGE: pmt lint PATH... [--exclude PATH]... [--allow CODE]... [--fix [--force]]
```

PATH is a `.pmc` file or a directory. Directories are walked
recursively for `*.pmc` in sorted order; symlinks are never followed
and dot-entries (`.git`, editor scratch) are skipped. A PATH that
yields no `.pmc` files is an error. `--exclude PATH` (repeatable)
skips a file or prunes a directory subtree; paths are compared as
spelled (no globs — the shell covers the include side), and exclusion
wins even over explicitly listed files.

Files lint independently: a file that fails to parse is reported on
stderr and the batch continues. Exit codes: 0 = every file clean,
1 = findings or errors anywhere (tool errors are also 1).

`--fix` applies safe fixes in place and lints the result again — the
report and exit code reflect what remains. `--fix --force` also
applies the gated fixes (deletions and rewrites whose diagnosis may
have another reading). `--force` without `--fix` is an error. A file
with a fatal error is never written. The rule catalog and per-rule fix
behavior live in `docs/lint.md`.
~~~

- [ ] **Step 3: Touch `README.md` and `docs/language.md`**

README, in the CLI overview where the subcommands are listed, add one line matching the surrounding style:

```
- `pmt lint` — hygiene findings over `.pmc` sources, with `--fix` (docs/lint.md)
```

`docs/language.md`, at the end of the warnings paragraph (the one describing the three visibility warnings), append:

```
Hygiene findings beyond these warnings — unused labels, shadowed
imports, naming style — are the lint layer's job: `pmt lint`
(docs/lint.md), a separate channel that never runs during compilation.
```

- [ ] **Step 4: Verify docs build nothing is broken**

Run: `cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --check`
Expected: green (docs changes cannot break the build; this is the pre-commit gate).

Also proofread: no issue/PR numbers, no host URLs in any touched published page.

- [ ] **Step 5: Commit**

```bash
git add docs/lint.md docs/cli.md README.md docs/language.md
git commit -m "docs: lint reference page, cli lint section, README and language pointers"
```
