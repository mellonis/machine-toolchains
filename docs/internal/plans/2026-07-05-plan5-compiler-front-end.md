# Plan 5 — Compiler Front End Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The `.pmc` language becomes real: `compile(source)` turns spec §3 programs into linkable `ObjectFile`s, with the lowered CFG available as a versioned JSON artifact and `-g` debug lines that speak `.pmc`, not generated assembly.

**Architecture:** Classic staged pipeline (spec §7): lexer → recursive-descent parser → AST → CFG lowering (with label resolution and unreachable-code warnings) → codegen. Codegen emits `.pma` text and drives the existing core assembler — the cc→as pipeline of spec §2 — so encoding, intra-function jump relaxation, and the `ent` prologue are inherited, not re-implemented, and `-S` output can never disagree with the object. Task 1 first lands the hardening items deferred from Plan 4's final review.

**Tech Stack:** Rust edition 2024. New modules all in `crates/post-machine`. `serde`/`serde_json` added to `mtc-post-machine` for the IR JSON artifact (spec §10 sanctions serde for exactly this).

## Spec deltas (controller applies to the spec doc on plan approval, before Task 1)

1. **§3.1 identifier rule concretized:** first char `char::is_alphabetic()` or `_`, then `char::is_alphanumeric()` or `_` — a conservative subset of JS `ID_Start`/`ID_Continue`, and exactly the `.pma` symbol rule, so every `.pmc` name survives the trip through generated assembly with zero new dependencies.
2. **§7 lowering:** IR terminator set is `{fallthrough, goto, check, return, halt}` — `halt` lowers as a terminator, not a block op (a block after `halt` can never execute; a false fall-through edge would poison Plan 6's dataflow). `!` check arms target a shared synthetic return block per function.

Clarifications that need no spec text change: `halt` and `debugger` take no parentheses (the spec table shows them bare); a label at the end of a function body is an error ("dangling label", mirroring the `.pma` assembler); statement labels may stack (`1: 2: left;`), each a distinct name for the same block.

## Global Constraints

- Gates on every task: `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo fmt --check` (run `cargo fmt` before committing).
- Library code never prints (spec §10): diagnostics are returned values — `CompileError` for fatals, `Warning`s in `CompileReport` for the rest.
- Reserved words (spec §3.3): `goto check left right mark unmark halt debugger`. `@` on a builtin is an error; a bare non-builtin identifier is an error; `goto !` is an error; comma-group rules: `check`/`halt` only last, successor only on the last item, `goto` never grouped.
- PM-1 facts: mark = symbol index 1, blank = 0 (`mark` → `wr 1`, `unmark` → `wr 0`); `debugger` → `brk`; returning from `main` → `stp`, from anything else → `ret`.
- Layout invariant (spec §7, active at `-O0`): never emit an unconditional transfer to the physically next instruction.
- Only dependency addition in this plan: `serde` (derive) + `serde_json` to `mtc-post-machine`.
- Commits are per-task and path-scoped (`git commit <paths>`), never pushed, no attribution footers.
- If the plan's code contradicts the spec or does not compile as written, report BLOCKED with the discrepancy — do not silently improvise.

## File Structure

- **Task 1 (core hardening):** modify `crates/core/src/formats/object.rs` (invariants doc), `crates/core/src/linker/layout.rs` (ent-prologue check, boundary-checked jump/debug remap, `try_from` upgrade), `crates/core/src/linker/mod.rs` (doc), `crates/core/src/asm/disassembler.rs` (`.byte` fallback helper).
- **Tasks 2–6:** create `crates/post-machine/src/{compiler.rs, lexer.rs, parser.rs, ir.rs, codegen.rs}`; modify `crates/post-machine/src/lib.rs`, `crates/post-machine/Cargo.toml`, and (Task 5 only) export `grid_line` from `crates/core/src/asm/`.
- **Task 7:** create `crates/post-machine/tests/compile_programs.rs`.

---

### Task 1: Core hardening (Plan 4 final-review deferrals)

**Files:**
- Modify: `crates/core/src/formats/object.rs` (doc block only)
- Modify: `crates/core/src/linker/layout.rs`
- Modify: `crates/core/src/linker/mod.rs` (doc comment only)
- Modify: `crates/core/src/asm/disassembler.rs`

**Interfaces:**
- Consumes: existing `LinkError::MalformedBlob { symbol, at }`, `ArchSyntax::entry_opcode`, `grid_line`.
- Produces: no signature changes. Three new `MalformedBlob` causes (blob without `ent` prologue; jump target off instruction boundaries; debug label/line offset off boundaries), and a private `push_byte_lines` helper in the disassembler.

- [ ] **Step 1: Document `ObjectFile` invariants.** In `crates/core/src/formats/object.rs`, replace the one-line comment above `pub struct ObjectFile` with:

```rust
/// In-memory object: symbols + code blobs + call relocations (+ optional
/// per-blob debug info).
///
/// Invariants — enforced by `from_bytes`, and REQUIRED of any
/// hand-constructed value handed to the linker:
/// - every `SymbolDef::Defined { blob }` indexes into `blobs`;
/// - every relocation's `blob` indexes into `blobs`, its `symbol` into
///   `symbols`, and `offset..offset + 4` lies inside that blob;
/// - each relocation hole is the operand of a far-call instruction at
///   `offset - 1` (the linker re-decodes blobs and rejects holes that
///   land anywhere else);
/// - each blob's first byte is the arch's entry opcode — function bodies
///   begin with their `ent` prologue;
/// - `debug`, when present, parallels `blobs` one-to-one, with label and
///   line offsets on instruction boundaries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObjectFile {
```

- [ ] **Step 2: Write the three failing adversarial tests.** Append to the `tests` module of `crates/core/src/linker/layout.rs`:

```rust
    #[test]
    fn blob_without_ent_prologue_is_malformed() {
        use crate::formats::object::{ObjectFile, Symbol, SymbolDef};
        let syntax = syntax_with_short_call();
        let obj = ObjectFile {
            arch: 0x7E,
            symbols: vec![Symbol {
                name: "main".into(),
                def: SymbolDef::Defined { blob: 0 },
            }],
            blobs: vec![vec![0x01, 0x02]], // nop, stop — no leading ent
            relocations: vec![],
            debug: None,
        };
        let e = link(&syntax, &[obj], &[], LinkOptions::default()).unwrap_err();
        assert_eq!(
            e,
            crate::linker::LinkError::MalformedBlob {
                symbol: "main".into(),
                at: 0
            }
        );
    }

    #[test]
    fn jump_to_mid_instruction_is_malformed() {
        use crate::formats::object::{ObjectFile, Symbol, SymbolDef};
        let syntax = syntax_with_short_call();
        // [0E][30 FF][02]: jmp.s at 1 ends at 3, offset −1 → target 2 = the
        // middle of the jmp.s itself; boundaries are 0, 1, 3.
        let obj = ObjectFile {
            arch: 0x7E,
            symbols: vec![Symbol {
                name: "main".into(),
                def: SymbolDef::Defined { blob: 0 },
            }],
            blobs: vec![vec![0x0E, 0x30, 0xFF, 0x02]],
            relocations: vec![],
            debug: None,
        };
        let e = link(&syntax, &[obj], &[], LinkOptions::default()).unwrap_err();
        assert_eq!(
            e,
            crate::linker::LinkError::MalformedBlob {
                symbol: "main".into(),
                at: 1
            }
        );
    }

    #[test]
    fn debug_label_off_instruction_boundary_is_malformed() {
        use crate::formats::object::{BlobDebug, ObjectFile, Symbol, SymbolDef};
        let syntax = syntax_with_short_call();
        // [0E][30 00][02]: a VALID jump (target 3 = the stop) so layout
        // succeeds — but the debug label at 2 points into the jmp.s.
        let obj = ObjectFile {
            arch: 0x7E,
            symbols: vec![Symbol {
                name: "main".into(),
                def: SymbolDef::Defined { blob: 0 },
            }],
            blobs: vec![vec![0x0E, 0x30, 0x00, 0x02]],
            relocations: vec![],
            debug: Some(vec![BlobDebug {
                labels: vec![("X".into(), 2)],
                lines: vec![],
            }]),
        };
        let e = link(&syntax, &[obj], &[], LinkOptions::default()).unwrap_err();
        assert_eq!(
            e,
            crate::linker::LinkError::MalformedBlob {
                symbol: "main".into(),
                at: 2
            }
        );
    }
```

- [ ] **Step 3: Run them to verify they fail.** `cargo test -p mtc-core linker::layout` — the first two must FAIL (the current code panics or emits silently); the third must FAIL (panic on `orig_to_new[off]`). A test failing by *panic* is expected here.

- [ ] **Step 4: Implement in `layout.rs`.** Three edits:

(a) At the top of `classify()`, before building `call_holes`:

```rust
    // Every linked function must begin with its `ent` prologue (the ABI
    // `.func` guarantees). A blob that doesn't would trap at its first
    // call landing anyway — fail at link time instead.
    if f.blob.first() != Some(&syntax.entry_opcode) {
        return Err(LinkError::MalformedBlob {
            symbol: f.name.to_string(),
            at: 0,
        });
    }
```

Also extend `classify()`'s doc comment: add "a blob whose first byte is not the entry opcode" to the listed `MalformedBlob` causes.

(b) Replace the `Piece::Jump { .. }` emission arm (currently indexes `orig_to_new[orig_target]` and uses `debug_assert!` + `as i8`):

```rust
                Piece::Jump {
                    orig,
                    opcode,
                    width,
                    orig_target,
                } => {
                    let Some(&target_new) = orig_to_new.get(orig_target) else {
                        // Not an instruction boundary of this function —
                        // a malformed blob, not a layout bug.
                        return Err(LinkError::MalformedBlob {
                            symbol: f.name.to_string(),
                            at: *orig,
                        });
                    };
                    let new_target = base + target_new;
                    let new_end = base + piece_offsets[pi] + 1 + u32::from(*width);
                    let off = i64::from(new_target) - i64::from(new_end);
                    code.push(*opcode);
                    match *width {
                        1 => {
                            let off8 = i8::try_from(off).expect(
                                "shrink-only invariant: jump still fits its original width",
                            );
                            code.push(off8 as u8);
                        }
                        4 => {
                            let off32 = i32::try_from(off).expect("jump offset fits i32");
                            code.extend(off32.to_le_bytes());
                        }
                        _ => unreachable!("jump operand width is always 1 or 4"),
                    }
                }
```

(c) Replace the debug-remap `match f.debug` block (currently unwraps via `orig_to_new[...]` indexing inside `map()`):

```rust
        let (labels, lines) = match f.debug {
            Some(debug) => {
                let mut labels = Vec::with_capacity(debug.labels.len());
                for (name, off) in &debug.labels {
                    let Some(&new) = orig_to_new.get(off) else {
                        return Err(LinkError::MalformedBlob {
                            symbol: f.name.to_string(),
                            at: *off,
                        });
                    };
                    labels.push((name.clone(), base + new));
                }
                let mut lines = Vec::with_capacity(debug.lines.len());
                for (off, line) in &debug.lines {
                    let Some(&new) = orig_to_new.get(off) else {
                        return Err(LinkError::MalformedBlob {
                            symbol: f.name.to_string(),
                            at: *off,
                        });
                    };
                    lines.push((base + new, *line));
                }
                (labels, lines)
            }
            None => (Vec::new(), Vec::new()),
        };
```

In `crates/core/src/linker/mod.rs`, extend the `MalformedBlob` variant's doc comment (keep the existing text, append): "Also raised when a blob lacks its entry-opcode prologue, when a jump targets a non-boundary offset, or when a debug label/line offset falls off instruction boundaries."

- [ ] **Step 5: Dedupe the `.byte` fallback in `disassembler.rs`.** Add below `grid_line`:

```rust
/// `.byte` fallback: one directive per byte, the label (if any) attached
/// to the first line.
fn push_byte_lines(out: &mut String, label: Option<&str>, bytes: &[u8]) {
    for (k, b) in bytes.iter().enumerate() {
        out.push_str(&grid_line(
            if k == 0 { label } else { None },
            ".byte",
            &b.to_string(),
        ));
        out.push('\n');
    }
}
```

Replace all four emission sites with calls to it:
- object `Body::Raw(b)` arm → `push_byte_lines(&mut out, label_name.as_deref(), &[*b]);`
- object reloc-less-call `None` arm (the `for k in 0..d.len` loop) → `push_byte_lines(&mut out, label_name.as_deref(), &code[d.addr as usize..(d.addr + d.len) as usize]);`
- executable gap (`None` from `instrs.get`) → `push_byte_lines(&mut out, label_name.as_deref(), &code[addr as usize..addr as usize + 1]); addr += 1;`
- executable cross-region `None` arm → `push_byte_lines(&mut out, label_name.as_deref(), &code[addr as usize..(addr + d.len) as usize]);`

No behavior change: the existing disassembler tests pin the output.

- [ ] **Step 6: Run the full gates.** `cargo test --workspace` (all green, including the three new tests), `cargo clippy --workspace --all-targets -- -D warnings`, `cargo fmt --check`.

- [ ] **Step 7: Commit.**

```bash
git add crates/core/src/formats/object.rs crates/core/src/linker/layout.rs crates/core/src/linker/mod.rs crates/core/src/asm/disassembler.rs
git commit -m "fix(core): plan-4 hardening — ent-prologue link check, boundary-checked jump/debug remap, .byte fallback helper"
```

---

### Task 2: Shared diagnostics + `.pmc` lexer

**Files:**
- Create: `crates/post-machine/src/compiler.rs` (diagnostics only in this task; the `compile()` driver arrives in Task 6)
- Create: `crates/post-machine/src/lexer.rs`
- Modify: `crates/post-machine/src/lib.rs`

**Interfaces:**
- Produces: `compiler::{CompileError, CompileErrorKind, Warning}`; `lexer::{Token, TokenKind, lex}`. Every later task reports through `CompileError`; the whole `CompileErrorKind` enum is defined NOW (later tasks construct variants, never add them).

- [ ] **Step 1: Write `crates/post-machine/src/compiler.rs`:**

```rust
//! `.pmc` compiler driver and shared diagnostics (spec §7).
//!
//! Every pipeline stage (lexer → parser → lowering → codegen) reports
//! fatals through [`CompileError`]; non-fatal findings accumulate as
//! [`Warning`]s — library code never prints (spec §10).

/// 1-based `line`; 1-based `col` counted in characters, or 0 when the
/// error is attributed to a whole line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompileError {
    pub line: u32,
    pub col: u32,
    pub kind: CompileErrorKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompileErrorKind {
    /// Lexical error (unexpected character, unterminated comment, …).
    Lex(String),
    /// The parser needed one thing and saw another.
    Expected { what: &'static str, found: String },
    /// A reserved word used as a function name.
    ReservedFunctionName(String),
    /// A bare identifier statement that is not a builtin (spec §3.3).
    UnknownCommand(String),
    /// `@` applied to a builtin name (`@left()`).
    BuiltinCalled(String),
    DuplicateFunction(String),
    DuplicateLabel(u32),
    /// `goto`/`check`/successor names a label the function never declares.
    UndefinedLabel(u32),
    /// `goto !` — spec §3.2: put `(!)` on the preceding command instead.
    GotoReturn,
    /// A comma-group position rule violated (spec §3.2, last table row).
    GroupPosition(&'static str),
    /// A label at the end of a function body binds to nothing.
    DanglingLabel(u32),
    /// The generated `.pma` failed to assemble — a compiler bug, not a
    /// user error; the message carries the assembler diagnostic.
    Internal(String),
}

impl std::fmt::Display for CompileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.col > 0 {
            write!(f, "line {}:{}: ", self.line, self.col)?;
        } else {
            write!(f, "line {}: ", self.line)?;
        }
        match &self.kind {
            CompileErrorKind::Lex(m) => write!(f, "{m}"),
            CompileErrorKind::Expected { what, found } => {
                write!(f, "expected {what}, found {found}")
            }
            CompileErrorKind::ReservedFunctionName(n) => {
                write!(f, "`{n}` is a reserved word and cannot name a function")
            }
            CompileErrorKind::UnknownCommand(n) => {
                write!(f, "unknown command `{n}` (user functions are called `@{n}()`)")
            }
            CompileErrorKind::BuiltinCalled(n) => {
                write!(f, "`{n}` is a builtin — write it without `@`")
            }
            CompileErrorKind::DuplicateFunction(n) => write!(f, "duplicate function `{n}`"),
            CompileErrorKind::DuplicateLabel(l) => write!(f, "duplicate label `{l}`"),
            CompileErrorKind::UndefinedLabel(l) => write!(f, "undefined label `{l}`"),
            CompileErrorKind::GotoReturn => {
                write!(f, "`goto !` is not allowed — put `(!)` on the preceding command")
            }
            CompileErrorKind::GroupPosition(m) => write!(f, "{m}"),
            CompileErrorKind::DanglingLabel(l) => {
                write!(f, "label `{l}` at end of function binds to nothing")
            }
            CompileErrorKind::Internal(m) => write!(f, "internal compiler error: {m}"),
        }
    }
}

impl std::error::Error for CompileError {}

/// A non-fatal finding, reported (never printed) via `CompileReport`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Warning {
    pub line: u32,
    pub message: String,
}
```

- [ ] **Step 2: Write `crates/post-machine/src/lexer.rs`:**

```rust
//! `.pmc` lexer (spec §3): source text → tokens with line:col.

use crate::compiler::{CompileError, CompileErrorKind};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TokenKind {
    Ident(String),
    Number(u32),
    At,
    Bang,
    Comma,
    Semi,
    Colon,
    LParen,
    RParen,
    LBrace,
    RBrace,
    Eof,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Token {
    pub kind: TokenKind,
    pub line: u32,
    pub col: u32,
}

/// Identifier rule (spec §3.1): Unicode; first char alphabetic or `_`,
/// then alphanumeric or `_` — the same classes as the `.pma` symbol
/// grammar, so every `.pmc` name survives the trip through generated
/// assembly.
fn is_ident_start(c: char) -> bool {
    c.is_alphabetic() || c == '_'
}

fn is_ident_continue(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

struct Cursor<'a> {
    chars: std::iter::Peekable<std::str::Chars<'a>>,
    line: u32,
    col: u32,
}

impl Cursor<'_> {
    fn peek(&mut self) -> Option<char> {
        self.chars.peek().copied()
    }

    fn bump(&mut self) -> Option<char> {
        let c = self.chars.next()?;
        if c == '\n' {
            self.line += 1;
            self.col = 1;
        } else {
            self.col += 1;
        }
        Some(c)
    }
}

fn err(line: u32, col: u32, message: String) -> CompileError {
    CompileError {
        line,
        col,
        kind: CompileErrorKind::Lex(message),
    }
}

pub fn lex(source: &str) -> Result<Vec<Token>, CompileError> {
    let mut cur = Cursor {
        chars: source.chars().peekable(),
        line: 1,
        col: 1,
    };
    let mut tokens = Vec::new();

    while let Some(c) = cur.peek() {
        let (line, col) = (cur.line, cur.col);
        if c.is_whitespace() {
            cur.bump();
            continue;
        }
        if c == '/' {
            cur.bump();
            match cur.peek() {
                Some('/') => {
                    while let Some(c) = cur.bump() {
                        if c == '\n' {
                            break;
                        }
                    }
                }
                Some('*') => {
                    cur.bump();
                    let mut prev = '\0';
                    let mut closed = false;
                    while let Some(c) = cur.bump() {
                        if prev == '*' && c == '/' {
                            closed = true;
                            break;
                        }
                        prev = c;
                    }
                    if !closed {
                        return Err(err(line, col, "unterminated block comment".into()));
                    }
                }
                _ => return Err(err(line, col, "unexpected character `/`".into())),
            }
            continue;
        }
        let single = match c {
            '@' => Some(TokenKind::At),
            '!' => Some(TokenKind::Bang),
            ',' => Some(TokenKind::Comma),
            ';' => Some(TokenKind::Semi),
            ':' => Some(TokenKind::Colon),
            '(' => Some(TokenKind::LParen),
            ')' => Some(TokenKind::RParen),
            '{' => Some(TokenKind::LBrace),
            '}' => Some(TokenKind::RBrace),
            _ => None,
        };
        if let Some(kind) = single {
            cur.bump();
            tokens.push(Token { kind, line, col });
            continue;
        }
        if c.is_ascii_digit() {
            let mut digits = String::new();
            while let Some(c) = cur.peek() {
                if c.is_ascii_digit() {
                    digits.push(c);
                    cur.bump();
                } else {
                    break;
                }
            }
            if cur.peek().is_some_and(is_ident_start) {
                return Err(err(line, col, "identifier cannot start with a digit".into()));
            }
            let value: u32 = digits
                .parse()
                .map_err(|_| err(line, col, format!("number `{digits}` is too large")))?;
            tokens.push(Token {
                kind: TokenKind::Number(value),
                line,
                col,
            });
            continue;
        }
        if is_ident_start(c) {
            let mut name = String::new();
            while let Some(c) = cur.peek() {
                if is_ident_continue(c) {
                    name.push(c);
                    cur.bump();
                } else {
                    break;
                }
            }
            tokens.push(Token {
                kind: TokenKind::Ident(name),
                line,
                col,
            });
            continue;
        }
        return Err(err(line, col, format!("unexpected character `{c}`")));
    }

    tokens.push(Token {
        kind: TokenKind::Eof,
        line: cur.line,
        col: cur.col,
    });
    Ok(tokens)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compiler::CompileErrorKind;

    fn kinds(src: &str) -> Vec<TokenKind> {
        lex(src).unwrap().into_iter().map(|t| t.kind).collect()
    }

    #[test]
    fn lexes_the_shape_of_a_function() {
        use TokenKind::*;
        assert_eq!(
            kinds("f() { 1: right(!); }"),
            vec![
                Ident("f".into()),
                LParen,
                RParen,
                LBrace,
                Number(1),
                Colon,
                Ident("right".into()),
                LParen,
                Bang,
                RParen,
                Semi,
                RBrace,
                Eof,
            ]
        );
    }

    #[test]
    fn tracks_line_and_column() {
        let tokens = lex("f()\n{\n  goto 7;\n}").unwrap();
        let goto = tokens
            .iter()
            .find(|t| t.kind == TokenKind::Ident("goto".into()))
            .unwrap();
        assert_eq!((goto.line, goto.col), (3, 3));
        let seven = tokens
            .iter()
            .find(|t| t.kind == TokenKind::Number(7))
            .unwrap();
        assert_eq!((seven.line, seven.col), (3, 8));
    }

    #[test]
    fn unicode_identifiers() {
        assert_eq!(
            kinds("идиВКонец()"),
            vec![
                TokenKind::Ident("идиВКонец".into()),
                TokenKind::LParen,
                TokenKind::RParen,
                TokenKind::Eof
            ]
        );
    }

    #[test]
    fn comments_are_skipped() {
        assert_eq!(
            kinds("// line\nleft /* block\n over lines */ ;"),
            vec![
                TokenKind::Ident("left".into()),
                TokenKind::Semi,
                TokenKind::Eof
            ]
        );
    }

    #[test]
    fn error_positions_and_kinds() {
        let e = lex("f() { $ }").unwrap_err();
        assert_eq!((e.line, e.col), (1, 7));
        assert!(matches!(e.kind, CompileErrorKind::Lex(ref m) if m.contains('$')));

        let e = lex("/* never closed").unwrap_err();
        assert!(matches!(e.kind, CompileErrorKind::Lex(ref m) if m.contains("unterminated")));

        let e = lex("12abc").unwrap_err();
        assert!(matches!(e.kind, CompileErrorKind::Lex(ref m) if m.contains("digit")));

        let e = lex("99999999999").unwrap_err();
        assert!(matches!(e.kind, CompileErrorKind::Lex(ref m) if m.contains("too large")));
    }
}
```

- [ ] **Step 3: Register the modules.** In `crates/post-machine/src/lib.rs` add:

```rust
pub mod compiler;
pub mod lexer;
```

- [ ] **Step 4: Run.** `cargo test -p mtc-post-machine lexer` — all pass. Then the three workspace gates.

- [ ] **Step 5: Commit.**

```bash
git add crates/post-machine/src/compiler.rs crates/post-machine/src/lexer.rs crates/post-machine/src/lib.rs
git commit -m "feat(post-machine): .pmc shared diagnostics and lexer"
```

---

### Task 3: Parser and AST

**Files:**
- Create: `crates/post-machine/src/parser.rs`
- Modify: `crates/post-machine/src/lib.rs`

**Interfaces:**
- Consumes: `lexer::{Token, TokenKind}`, `compiler::{CompileError, CompileErrorKind}`.
- Produces: `parser::{parse, Program, Function, Statement, Item, Builtin, Successor, CheckArm, RESERVED}` with exactly the shapes below — Task 4's lowering pattern-matches them verbatim.

Parse-time semantic checks live here (the syntactic half of spec §7 module 4): reserved words, duplicate functions, duplicate labels, comma-group position rules, `goto !`, dangling labels. Label *resolution* (undefined targets) waits for lowering, where the per-function label map is built — declaration order is free (spec §3.3).

- [ ] **Step 1: Write `crates/post-machine/src/parser.rs`:**

```rust
//! `.pmc` recursive-descent parser (spec §3): tokens → AST.

use std::collections::HashSet;

use crate::compiler::{CompileError, CompileErrorKind};
use crate::lexer::{Token, TokenKind};

/// Spec §3.3: words that cannot name a function.
pub const RESERVED: [&str; 8] = [
    "goto", "check", "left", "right", "mark", "unmark", "halt", "debugger",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Program {
    pub functions: Vec<Function>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Function {
    pub name: String,
    pub line: u32,
    pub col: u32,
    pub body: Vec<Statement>,
}

/// One `;`-terminated statement: an optional run of labels, then one or
/// more comma-separated items. `items.len() > 1` only for comma groups,
/// whose position rules the parser has enforced: `check`/`halt` only
/// last, a successor only on the last item, `goto` never grouped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Statement {
    pub labels: Vec<u32>,
    pub items: Vec<Item>,
    pub line: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Builtin {
    Left,
    Right,
    Mark,
    Unmark,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Successor {
    FallThrough,
    Label(u32),
    Return,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckArm {
    Label(u32),
    Return,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Item {
    Builtin {
        which: Builtin,
        succ: Successor,
        line: u32,
    },
    Debugger {
        line: u32,
    },
    Call {
        name: String,
        succ: Successor,
        line: u32,
    },
    Check {
        marked: CheckArm,
        blank: CheckArm,
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

fn describe(kind: &TokenKind) -> String {
    match kind {
        TokenKind::Ident(n) => format!("`{n}`"),
        TokenKind::Number(v) => format!("`{v}`"),
        TokenKind::At => "`@`".into(),
        TokenKind::Bang => "`!`".into(),
        TokenKind::Comma => "`,`".into(),
        TokenKind::Semi => "`;`".into(),
        TokenKind::Colon => "`:`".into(),
        TokenKind::LParen => "`(`".into(),
        TokenKind::RParen => "`)`".into(),
        TokenKind::LBrace => "`{`".into(),
        TokenKind::RBrace => "`}`".into(),
        TokenKind::Eof => "end of file".into(),
    }
}

pub fn parse(tokens: &[Token]) -> Result<Program, CompileError> {
    Parser { tokens, pos: 0 }.program()
}

struct Parser<'a> {
    tokens: &'a [Token],
    pos: usize,
}

impl Parser<'_> {
    fn peek(&self) -> &Token {
        // Safe: the lexer always appends Eof and bump() never passes it.
        &self.tokens[self.pos]
    }

    fn bump(&mut self) {
        if !matches!(self.tokens[self.pos].kind, TokenKind::Eof) {
            self.pos += 1;
        }
    }

    fn err_at(t: &Token, kind: CompileErrorKind) -> CompileError {
        CompileError {
            line: t.line,
            col: t.col,
            kind,
        }
    }

    fn expected(t: &Token, what: &'static str) -> CompileError {
        Self::err_at(
            t,
            CompileErrorKind::Expected {
                what,
                found: describe(&t.kind),
            },
        )
    }

    fn expect(&mut self, kind: &TokenKind, what: &'static str) -> Result<(), CompileError> {
        if &self.peek().kind == kind {
            self.bump();
            Ok(())
        } else {
            Err(Self::expected(self.peek(), what))
        }
    }

    fn program(mut self) -> Result<Program, CompileError> {
        let mut functions: Vec<Function> = Vec::new();
        while !matches!(self.peek().kind, TokenKind::Eof) {
            let f = self.function()?;
            if functions.iter().any(|g| g.name == f.name) {
                return Err(CompileError {
                    line: f.line,
                    col: f.col,
                    kind: CompileErrorKind::DuplicateFunction(f.name),
                });
            }
            functions.push(f);
        }
        Ok(Program { functions })
    }

    fn function(&mut self) -> Result<Function, CompileError> {
        let name_tok = self.peek().clone();
        let TokenKind::Ident(name) = &name_tok.kind else {
            return Err(Self::expected(&name_tok, "a function name"));
        };
        let name = name.clone();
        if RESERVED.contains(&name.as_str()) {
            return Err(Self::err_at(
                &name_tok,
                CompileErrorKind::ReservedFunctionName(name),
            ));
        }
        self.bump();
        self.expect(&TokenKind::LParen, "`(` after the function name")?;
        self.expect(&TokenKind::RParen, "`)` (functions take no parameters)")?;
        self.expect(&TokenKind::LBrace, "`{`")?;

        let mut body = Vec::new();
        let mut seen_labels: HashSet<u32> = HashSet::new();
        loop {
            // Labels announced before the next statement (possibly stacked).
            let mut labels = Vec::new();
            loop {
                let tok = self.peek().clone();
                let TokenKind::Number(n) = tok.kind else { break };
                self.bump();
                self.expect(&TokenKind::Colon, "`:` after a label number")?;
                if !seen_labels.insert(n) {
                    return Err(Self::err_at(&tok, CompileErrorKind::DuplicateLabel(n)));
                }
                labels.push(n);
            }
            if matches!(self.peek().kind, TokenKind::RBrace) {
                if let Some(&label) = labels.first() {
                    let t = self.peek();
                    return Err(CompileError {
                        line: t.line,
                        col: t.col,
                        kind: CompileErrorKind::DanglingLabel(label),
                    });
                }
                self.bump();
                break;
            }
            body.push(self.statement(labels)?);
        }
        Ok(Function {
            name,
            line: name_tok.line,
            col: name_tok.col,
            body,
        })
    }

    fn statement(&mut self, labels: Vec<u32>) -> Result<Statement, CompileError> {
        let line = self.peek().line;
        let mut items = vec![self.item(false)?];
        while matches!(self.peek().kind, TokenKind::Comma) {
            let comma = self.peek().clone();
            // Whatever precedes a `,` must be bare (spec §3.2).
            match items.last().expect("items is never empty") {
                Item::Check { .. } => {
                    return Err(Self::err_at(
                        &comma,
                        CompileErrorKind::GroupPosition(
                            "check must be the last command in a comma group",
                        ),
                    ));
                }
                Item::Halt { .. } => {
                    return Err(Self::err_at(
                        &comma,
                        CompileErrorKind::GroupPosition(
                            "halt must be the last command in a comma group",
                        ),
                    ));
                }
                Item::Goto { .. } => {
                    return Err(Self::err_at(
                        &comma,
                        CompileErrorKind::GroupPosition("goto cannot appear in a comma group"),
                    ));
                }
                Item::Builtin { succ, .. } | Item::Call { succ, .. }
                    if *succ != Successor::FallThrough =>
                {
                    return Err(Self::err_at(
                        &comma,
                        CompileErrorKind::GroupPosition(
                            "only the last command in a comma group may take a successor",
                        ),
                    ));
                }
                _ => {}
            }
            self.bump();
            items.push(self.item(true)?);
        }
        self.expect(&TokenKind::Semi, "`;`")?;
        Ok(Statement { labels, items, line })
    }

    fn item(&mut self, in_group: bool) -> Result<Item, CompileError> {
        let tok = self.peek().clone();
        match &tok.kind {
            TokenKind::At => {
                self.bump();
                let name_tok = self.peek().clone();
                let TokenKind::Ident(name) = &name_tok.kind else {
                    return Err(Self::expected(&name_tok, "a function name after `@`"));
                };
                let name = name.clone();
                if RESERVED.contains(&name.as_str()) {
                    return Err(Self::err_at(
                        &name_tok,
                        CompileErrorKind::BuiltinCalled(name),
                    ));
                }
                self.bump();
                self.expect(&TokenKind::LParen, "`(` (user calls are written `@name()`)")?;
                let succ = self.successor()?;
                self.expect(&TokenKind::RParen, "`)`")?;
                Ok(Item::Call {
                    name,
                    succ,
                    line: tok.line,
                })
            }
            TokenKind::Ident(word) => match word.as_str() {
                "goto" => {
                    if in_group {
                        return Err(Self::err_at(
                            &tok,
                            CompileErrorKind::GroupPosition("goto cannot appear in a comma group"),
                        ));
                    }
                    self.bump();
                    let target = self.peek().clone();
                    match target.kind {
                        TokenKind::Number(n) => {
                            self.bump();
                            Ok(Item::Goto {
                                label: n,
                                line: tok.line,
                            })
                        }
                        TokenKind::Bang => Err(Self::err_at(&target, CompileErrorKind::GotoReturn)),
                        _ => Err(Self::expected(&target, "a numeric label after `goto`")),
                    }
                }
                "check" => {
                    self.bump();
                    self.expect(&TokenKind::LParen, "`(` after `check`")?;
                    let marked = self.check_arm()?;
                    self.expect(&TokenKind::Comma, "`,` between check arms")?;
                    let blank = self.check_arm()?;
                    self.expect(&TokenKind::RParen, "`)`")?;
                    Ok(Item::Check {
                        marked,
                        blank,
                        line: tok.line,
                    })
                }
                "halt" => {
                    self.bump();
                    Ok(Item::Halt { line: tok.line })
                }
                "debugger" => {
                    self.bump();
                    Ok(Item::Debugger { line: tok.line })
                }
                "left" | "right" | "mark" | "unmark" => {
                    let which = match word.as_str() {
                        "left" => Builtin::Left,
                        "right" => Builtin::Right,
                        "mark" => Builtin::Mark,
                        _ => Builtin::Unmark,
                    };
                    self.bump();
                    let succ = if matches!(self.peek().kind, TokenKind::LParen) {
                        self.bump();
                        let succ = self.successor()?;
                        self.expect(&TokenKind::RParen, "`)`")?;
                        succ
                    } else {
                        Successor::FallThrough
                    };
                    Ok(Item::Builtin {
                        which,
                        succ,
                        line: tok.line,
                    })
                }
                other => Err(Self::err_at(
                    &tok,
                    CompileErrorKind::UnknownCommand(other.to_string()),
                )),
            },
            _ => Err(Self::expected(&tok, "a command")),
        }
    }

    /// Inside `( … )`: empty → fall through, `N` → label, `!` → return.
    fn successor(&mut self) -> Result<Successor, CompileError> {
        let t = self.peek().clone();
        match t.kind {
            TokenKind::Number(n) => {
                self.bump();
                Ok(Successor::Label(n))
            }
            TokenKind::Bang => {
                self.bump();
                Ok(Successor::Return)
            }
            _ => Ok(Successor::FallThrough), // the caller checks the `)`
        }
    }

    fn check_arm(&mut self) -> Result<CheckArm, CompileError> {
        let t = self.peek().clone();
        match t.kind {
            TokenKind::Number(n) => {
                self.bump();
                Ok(CheckArm::Label(n))
            }
            TokenKind::Bang => {
                self.bump();
                Ok(CheckArm::Return)
            }
            _ => Err(Self::expected(&t, "a label number or `!`")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compiler::CompileErrorKind;
    use crate::lexer::lex;

    fn parse_src(src: &str) -> Result<Program, CompileError> {
        parse(&lex(src).unwrap())
    }

    #[test]
    fn parses_the_spec_sample() {
        let src = r#"
// Move right until the first blank cell.
goToEnd() {
1:  right;
    check(1, 2);      // cell marked -> goto 1, blank -> goto 2
2:  left;             // last command - implicit return
}

goToBegin() {
1:  left(2);
2:  check(1, 3);
3:  right(!);
}

main() {
    @goToEnd();
    right;
    check(3, 4);
3:  unmark(!);
4:  mark;
}
"#;
        let p = parse_src(src).unwrap();
        assert_eq!(
            p.functions.iter().map(|f| f.name.as_str()).collect::<Vec<_>>(),
            vec!["goToEnd", "goToBegin", "main"]
        );
        let main = &p.functions[2];
        assert_eq!(main.body.len(), 5);
        assert_eq!(
            main.body[0].items,
            vec![Item::Call {
                name: "goToEnd".into(),
                succ: Successor::FallThrough,
                line: main.body[0].line
            }]
        );
        assert_eq!(main.body[3].labels, vec![3]);
        assert_eq!(
            main.body[3].items,
            vec![Item::Builtin {
                which: Builtin::Unmark,
                succ: Successor::Return,
                line: main.body[3].line
            }]
        );
        match &main.body[2].items[0] {
            Item::Check {
                marked: CheckArm::Label(3),
                blank: CheckArm::Label(4),
                ..
            } => {}
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn comma_groups_parse_and_enforce_positions() {
        let p = parse_src("f() { 1: right, right, mark(5); 5: left, check(1, !); }").unwrap();
        assert_eq!(p.functions[0].body[0].items.len(), 3);
        assert_eq!(p.functions[0].body[1].items.len(), 2);

        let e = parse_src("f() { left(1), left(2); 1: mark; 2: mark; }").unwrap_err();
        assert!(matches!(e.kind, CompileErrorKind::GroupPosition(m) if m.contains("successor")));

        let e = parse_src("f() { check(1, 2), left; 1: mark; 2: mark; }").unwrap_err();
        assert!(matches!(e.kind, CompileErrorKind::GroupPosition(m) if m.contains("check")));

        let e = parse_src("f() { halt, left; }").unwrap_err();
        assert!(matches!(e.kind, CompileErrorKind::GroupPosition(m) if m.contains("halt")));

        let e = parse_src("f() { goto 1, left; 1: mark; }").unwrap_err();
        assert!(matches!(e.kind, CompileErrorKind::GroupPosition(m) if m.contains("goto")));
        let e = parse_src("f() { left, goto 1; 1: mark; }").unwrap_err();
        assert!(matches!(e.kind, CompileErrorKind::GroupPosition(m) if m.contains("goto")));
    }

    #[test]
    fn reserved_and_at_rules() {
        let e = parse_src("check() { }").unwrap_err();
        assert!(matches!(e.kind, CompileErrorKind::ReservedFunctionName(n) if n == "check"));

        let e = parse_src("f() { @left(); }").unwrap_err();
        assert!(matches!(e.kind, CompileErrorKind::BuiltinCalled(n) if n == "left"));

        let e = parse_src("f() { flip; }").unwrap_err();
        assert!(matches!(e.kind, CompileErrorKind::UnknownCommand(n) if n == "flip"));

        // A user function called without `@` is the same error (spec §3.3).
        let e = parse_src("f() { goToEnd(); }").unwrap_err();
        assert!(matches!(e.kind, CompileErrorKind::UnknownCommand(n) if n == "goToEnd"));
    }

    #[test]
    fn goto_bang_is_a_dedicated_error() {
        let e = parse_src("f() { goto !; }").unwrap_err();
        assert!(matches!(e.kind, CompileErrorKind::GotoReturn));
    }

    #[test]
    fn duplicate_and_dangling_diagnostics() {
        let e = parse_src("f() { } f() { }").unwrap_err();
        assert!(matches!(e.kind, CompileErrorKind::DuplicateFunction(n) if n == "f"));

        let e = parse_src("f() { 1: left; 1: right; }").unwrap_err();
        assert!(matches!(e.kind, CompileErrorKind::DuplicateLabel(1)));

        let e = parse_src("f() { left; 2: }").unwrap_err();
        assert!(matches!(e.kind, CompileErrorKind::DanglingLabel(2)));
    }

    #[test]
    fn empty_function_and_stacked_labels() {
        let p = parse_src("f() { }").unwrap();
        assert!(p.functions[0].body.is_empty());

        let p = parse_src("f() { 1: 2: left; }").unwrap();
        assert_eq!(p.functions[0].body[0].labels, vec![1, 2]);
    }

    #[test]
    fn unicode_function_names_and_calls() {
        let p = parse_src("идиВКонец() { right(!); } main() { @идиВКонец(); }").unwrap();
        assert_eq!(p.functions[0].name, "идиВКонец");
        match &p.functions[1].body[0].items[0] {
            Item::Call { name, .. } => assert_eq!(name, "идиВКонец"),
            other => panic!("unexpected {other:?}"),
        }
    }
}
```

- [ ] **Step 2: Register the module.** In `crates/post-machine/src/lib.rs` add `pub mod parser;`.

- [ ] **Step 3: Run.** `cargo test -p mtc-post-machine parser` — all pass. Then the three workspace gates.

- [ ] **Step 4: Commit.**

```bash
git add crates/post-machine/src/parser.rs crates/post-machine/src/lib.rs
git commit -m "feat(post-machine): .pmc recursive-descent parser and AST"
```

---

### Task 4: CFG IR and lowering

**Files:**
- Create: `crates/post-machine/src/ir.rs`
- Modify: `crates/post-machine/src/lib.rs`, `crates/post-machine/Cargo.toml`

**Interfaces:**
- Consumes: `parser::{Program, Function, Statement, Item, Builtin, Successor, CheckArm}`, `compiler::{CompileError, CompileErrorKind, Warning}`.
- Produces: `ir::{IR_VERSION, IrProgram, IrFunction, IrBlock, IrOp, IrTerm, lower}` with `IrProgram::{to_json, from_json}`. Task 5's codegen consumes exactly these shapes; Plan 6's passes will rewrite them.

Lowering completes semantic checking (undefined labels) and computes unreachable-code warnings (spec §7 module 4 / §8 pass 3 will later delete them). Block edges make every successor form explicit — the old IR's `-1` stop / `-2` auto-link, spelled out (spec §7).

- [ ] **Step 1: Add the serde dependencies.** In `crates/post-machine/Cargo.toml` under `[dependencies]`:

```toml
serde = { version = "1", features = ["derive"] }
serde_json = "1"
```

- [ ] **Step 2: Write `crates/post-machine/src/ir.rs`:**

```rust
//! Per-function CFG IR (spec §7, §7.1): a versioned, documented JSON
//! artifact, not an internal detail. Lowering makes every statement
//! successor an explicit block edge.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::compiler::{CompileError, CompileErrorKind, Warning};
use crate::parser::{Builtin, CheckArm, Item, Program, Successor};

pub const IR_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IrProgram {
    pub version: u32,
    pub functions: Vec<IrFunction>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IrFunction {
    pub name: String,
    /// Source line of the definition.
    pub line: u32,
    /// Entry is `blocks[0]`. Ids are unique within the function but need
    /// not stay dense once optimizer passes (Plan 6) delete blocks.
    pub blocks: Vec<IrBlock>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IrBlock {
    pub id: u32,
    /// Source labels naming this block (empty for synthetic blocks).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub labels: Vec<u32>,
    /// Source line of the block's first statement; 0 = synthetic.
    pub line: u32,
    pub ops: Vec<IrOp>,
    pub term: IrTerm,
    /// Source line of the statement that produced the terminator; 0 =
    /// synthetic (implicit return, shared exit block).
    pub term_line: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum IrOp {
    Lft { line: u32 },
    Rgt { line: u32 },
    Wr { index: u32, line: u32 },
    Brk { line: u32 },
    Call { name: String, line: u32 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum IrTerm {
    FallThrough { to: u32 },
    Goto { to: u32 },
    Check { marked: u32, blank: u32 },
    Return,
    Halt,
}

impl IrProgram {
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).expect("IR serializes")
    }

    pub fn from_json(s: &str) -> Result<IrProgram, String> {
        serde_json::from_str(s).map_err(|e| e.to_string())
    }
}

/// Does this statement end its basic block?
fn terminates(stmt: &crate::parser::Statement) -> bool {
    match stmt.items.last().expect("parser: statements have items") {
        Item::Check { .. } | Item::Halt { .. } | Item::Goto { .. } => true,
        Item::Builtin { succ, .. } | Item::Call { succ, .. } => *succ != Successor::FallThrough,
        Item::Debugger { .. } => false,
    }
}

/// AST → CFG, plus the label-resolution half of semantic checking and
/// unreachable-code warnings.
pub fn lower(program: &Program) -> Result<(IrProgram, Vec<Warning>), CompileError> {
    let mut functions = Vec::with_capacity(program.functions.len());
    let mut warnings = Vec::new();
    for f in &program.functions {
        functions.push(lower_function(f, &mut warnings)?);
    }
    Ok((
        IrProgram {
            version: IR_VERSION,
            functions,
        },
        warnings,
    ))
}

fn lower_function(
    f: &crate::parser::Function,
    warnings: &mut Vec<Warning>,
) -> Result<IrFunction, CompileError> {
    if f.body.is_empty() {
        // `f() {}` — a single empty block: ent; ret.
        return Ok(IrFunction {
            name: f.name.clone(),
            line: f.line,
            blocks: vec![IrBlock {
                id: 0,
                labels: vec![],
                line: 0,
                ops: vec![],
                term: IrTerm::Return,
                term_line: 0,
            }],
        });
    }

    // Pass A: block boundaries. A statement starts a new block when it is
    // labeled or its predecessor terminated one.
    let mut starts = vec![false; f.body.len()];
    for (i, stmt) in f.body.iter().enumerate() {
        starts[i] = i == 0 || !stmt.labels.is_empty() || terminates(&f.body[i - 1]);
    }
    let mut block_of_stmt = vec![0u32; f.body.len()];
    let mut n_blocks = 0u32;
    for (i, &s) in starts.iter().enumerate() {
        if s {
            n_blocks += 1;
        }
        block_of_stmt[i] = n_blocks - 1;
    }

    // The shared synthetic return block: target of `!` check arms.
    let exit_id = n_blocks;
    let mut exit_used = false;

    let mut label_block: HashMap<u32, u32> = HashMap::new();
    for (i, stmt) in f.body.iter().enumerate() {
        for &l in &stmt.labels {
            label_block.insert(l, block_of_stmt[i]);
        }
    }
    let resolve = |label: u32, line: u32| -> Result<u32, CompileError> {
        label_block.get(&label).copied().ok_or_else(|| CompileError {
            line,
            col: 0,
            kind: CompileErrorKind::UndefinedLabel(label),
        })
    };

    enum Close {
        None,
        Term(IrTerm),
    }

    let mut blocks: Vec<IrBlock> = Vec::new();
    let mut current: Option<IrBlock> = None;

    for (i, stmt) in f.body.iter().enumerate() {
        if starts[i] {
            debug_assert!(current.is_none(), "predecessor closed the block");
            current = Some(IrBlock {
                id: block_of_stmt[i],
                labels: stmt.labels.clone(),
                line: stmt.line,
                ops: vec![],
                term: IrTerm::Return, // placeholder, always overwritten
                term_line: 0,
            });
        }
        let block = current.as_mut().expect("a block is always open here");

        for item in &stmt.items {
            match item {
                Item::Builtin { which, line, .. } => block.ops.push(match which {
                    Builtin::Left => IrOp::Lft { line: *line },
                    Builtin::Right => IrOp::Rgt { line: *line },
                    Builtin::Mark => IrOp::Wr { index: 1, line: *line },
                    Builtin::Unmark => IrOp::Wr { index: 0, line: *line },
                }),
                Item::Debugger { line } => block.ops.push(IrOp::Brk { line: *line }),
                Item::Call { name, line, .. } => block.ops.push(IrOp::Call {
                    name: name.clone(),
                    line: *line,
                }),
                Item::Check { .. } | Item::Halt { .. } | Item::Goto { .. } => {}
            }
        }

        let last = stmt.items.last().expect("parser: statements have items");
        let close = match last {
            Item::Goto { label, line } => Close::Term(IrTerm::Goto {
                to: resolve(*label, *line)?,
            }),
            Item::Halt { .. } => Close::Term(IrTerm::Halt),
            Item::Check { marked, blank, line } => {
                let mut arm = |a: &CheckArm| -> Result<u32, CompileError> {
                    Ok(match a {
                        CheckArm::Label(l) => resolve(*l, *line)?,
                        CheckArm::Return => {
                            exit_used = true;
                            exit_id
                        }
                    })
                };
                Close::Term(IrTerm::Check {
                    marked: arm(marked)?,
                    blank: arm(blank)?,
                })
            }
            Item::Builtin { succ, line, .. } | Item::Call { succ, line, .. } => match succ {
                Successor::Label(l) => Close::Term(IrTerm::Goto {
                    to: resolve(*l, *line)?,
                }),
                Successor::Return => Close::Term(IrTerm::Return),
                Successor::FallThrough => Close::None,
            },
            Item::Debugger { .. } => Close::None,
        };

        let is_last_stmt = i + 1 == f.body.len();
        match close {
            Close::Term(term) => {
                let mut b = current.take().expect("block open");
                b.term = term;
                b.term_line = stmt.line;
                blocks.push(b);
            }
            Close::None => {
                if is_last_stmt {
                    // Falling off the end — implicit return (spec §3.2).
                    let mut b = current.take().expect("block open");
                    b.term = IrTerm::Return;
                    b.term_line = stmt.line;
                    blocks.push(b);
                } else if starts[i + 1] {
                    let mut b = current.take().expect("block open");
                    b.term = IrTerm::FallThrough {
                        to: block_of_stmt[i + 1],
                    };
                    b.term_line = stmt.line;
                    blocks.push(b);
                }
                // else: the same block continues into the next statement.
            }
        }
    }

    if exit_used {
        blocks.push(IrBlock {
            id: exit_id,
            labels: vec![],
            line: 0,
            ops: vec![],
            term: IrTerm::Return,
            term_line: 0,
        });
    }

    // Unreachable-code warnings: DFS over terminator edges from the entry.
    let index_of: HashMap<u32, usize> =
        blocks.iter().enumerate().map(|(i, b)| (b.id, i)).collect();
    let mut seen: HashSet<u32> = HashSet::new();
    let mut work = vec![blocks[0].id];
    while let Some(id) = work.pop() {
        if !seen.insert(id) {
            continue;
        }
        match blocks[index_of[&id]].term {
            IrTerm::FallThrough { to } | IrTerm::Goto { to } => work.push(to),
            IrTerm::Check { marked, blank } => {
                work.push(marked);
                work.push(blank);
            }
            IrTerm::Return | IrTerm::Halt => {}
        }
    }
    for b in &blocks {
        if !seen.contains(&b.id) && b.line != 0 {
            warnings.push(Warning {
                line: b.line,
                message: format!("unreachable code in `{}`", f.name),
            });
        }
    }

    Ok(IrFunction {
        name: f.name.clone(),
        line: f.line,
        blocks,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::lex;
    use crate::parser::parse;

    fn ir_of(src: &str) -> (IrProgram, Vec<Warning>) {
        lower(&parse(&lex(src).unwrap()).unwrap()).unwrap()
    }

    #[test]
    fn lowers_go_to_end() {
        // 1: right; check(1,2); 2: left;  →  b0 {rgt | check b0,b1}, b1 {lft | ret}
        let (ir, warnings) = ir_of("goToEnd() { 1: right; check(1, 2); 2: left; }");
        assert!(warnings.is_empty());
        let f = &ir.functions[0];
        assert_eq!(f.blocks.len(), 2);
        assert_eq!(f.blocks[0].labels, vec![1]);
        assert_eq!(f.blocks[0].ops, vec![IrOp::Rgt { line: 1 }]);
        assert_eq!(f.blocks[0].term, IrTerm::Check { marked: 0, blank: 1 });
        assert_eq!(f.blocks[1].labels, vec![2]);
        assert_eq!(f.blocks[1].term, IrTerm::Return);
    }

    #[test]
    fn explicit_successors_become_gotos() {
        let (ir, _) = ir_of("goToBegin() { 1: left(2); 2: check(1, 3); 3: right(!); }");
        let f = &ir.functions[0];
        assert_eq!(f.blocks.len(), 3);
        assert_eq!(f.blocks[0].term, IrTerm::Goto { to: 1 });
        assert_eq!(f.blocks[1].term, IrTerm::Check { marked: 0, blank: 2 });
        assert_eq!(f.blocks[2].term, IrTerm::Return);
    }

    #[test]
    fn comma_groups_flatten_and_the_exit_block_is_shared() {
        let (ir, _) = ir_of("f() { 1: right, right, mark(5); 5: left, check(1, !); }");
        let f = &ir.functions[0];
        // b0: rgt rgt wr1 | goto b1; b1: lft | check(b0, exit); exit: ret
        assert_eq!(f.blocks.len(), 3);
        assert_eq!(
            f.blocks[0].ops,
            vec![
                IrOp::Rgt { line: 1 },
                IrOp::Rgt { line: 1 },
                IrOp::Wr { index: 1, line: 1 }
            ]
        );
        assert_eq!(f.blocks[0].term, IrTerm::Goto { to: 1 });
        assert_eq!(f.blocks[1].term, IrTerm::Check { marked: 0, blank: 2 });
        assert_eq!(f.blocks[2].id, 2);
        assert!(f.blocks[2].labels.is_empty());
        assert_eq!(f.blocks[2].line, 0); // synthetic
        assert_eq!(f.blocks[2].term, IrTerm::Return);
    }

    #[test]
    fn unlabeled_statements_merge_and_the_end_returns_implicitly() {
        let (ir, _) = ir_of("main() { @goToEnd(); right; check(3, 4); 3: unmark(!); 4: mark; }");
        let f = &ir.functions[0];
        assert_eq!(f.blocks.len(), 3);
        assert_eq!(
            f.blocks[0].ops,
            vec![
                IrOp::Call { name: "goToEnd".into(), line: 1 },
                IrOp::Rgt { line: 1 }
            ]
        );
        assert_eq!(f.blocks[0].term, IrTerm::Check { marked: 1, blank: 2 });
        assert_eq!(f.blocks[1].ops, vec![IrOp::Wr { index: 0, line: 1 }]);
        assert_eq!(f.blocks[1].term, IrTerm::Return); // unmark(!)
        assert_eq!(f.blocks[2].term, IrTerm::Return); // implicit
    }

    #[test]
    fn halt_is_a_terminator_and_debugger_an_op() {
        let (ir, _) = ir_of("f() { debugger; halt; }");
        let f = &ir.functions[0];
        assert_eq!(f.blocks.len(), 1);
        assert_eq!(f.blocks[0].ops, vec![IrOp::Brk { line: 1 }]);
        assert_eq!(f.blocks[0].term, IrTerm::Halt);
    }

    #[test]
    fn empty_function_is_one_returning_block() {
        let (ir, _) = ir_of("f() { }");
        assert_eq!(ir.functions[0].blocks.len(), 1);
        assert_eq!(ir.functions[0].blocks[0].term, IrTerm::Return);
    }

    #[test]
    fn undefined_labels_error_wherever_they_are_referenced() {
        let e = lower(&parse(&lex("f() { goto 9; }").unwrap()).unwrap()).unwrap_err();
        assert!(matches!(e.kind, CompileErrorKind::UndefinedLabel(9)));
        let e = lower(&parse(&lex("f() { left(7); }").unwrap()).unwrap()).unwrap_err();
        assert!(matches!(e.kind, CompileErrorKind::UndefinedLabel(7)));
        let e = lower(&parse(&lex("f() { check(1, 2); 1: mark; }").unwrap()).unwrap()).unwrap_err();
        assert!(matches!(e.kind, CompileErrorKind::UndefinedLabel(2)));
    }

    #[test]
    fn unreachable_code_warns_with_its_line() {
        let (ir, warnings) = ir_of("f() {\n    goto 1;\n    right;\n1:  left;\n}");
        assert_eq!(ir.functions[0].blocks.len(), 3);
        assert_eq!(warnings.len(), 1);
        assert_eq!(warnings[0].line, 3);
        assert!(warnings[0].message.contains("unreachable"));
    }

    #[test]
    fn json_round_trips_with_a_version() {
        let (ir, _) = ir_of("main() { @go(); check(1, !); 1: mark(!); }");
        let json = ir.to_json();
        assert_eq!(IrProgram::from_json(&json).unwrap(), ir);
        assert!(json.contains("\"version\": 1"));
    }
}
```

- [ ] **Step 3: Register the module.** In `crates/post-machine/src/lib.rs` add `pub mod ir;`.

- [ ] **Step 4: Run.** `cargo test -p mtc-post-machine ir` — all pass. Then the three workspace gates (clippy will now also lint the serde derives).

- [ ] **Step 5: Commit.**

```bash
git add crates/post-machine/src/ir.rs crates/post-machine/src/lib.rs crates/post-machine/Cargo.toml Cargo.lock
git commit -m "feat(post-machine): CFG IR with versioned JSON form; lowering with label resolution and unreachable warnings"
```

---

### Task 5: Codegen — CFG → `.pma` text

**Files:**
- Modify: `crates/core/src/asm/disassembler.rs` (make `grid_line` `pub`), `crates/core/src/asm/mod.rs` (re-export it)
- Create: `crates/post-machine/src/codegen.rs`
- Modify: `crates/post-machine/src/lib.rs`

**Interfaces:**
- Consumes: `ir::{IrProgram, IrFunction, IrBlock, IrOp, IrTerm}`; `mtc_core::asm::grid_line` (newly exported).
- Produces: `codegen::{CodegenOptions, PmaOutput, emit_program}`. `PmaOutput.line_map` is `(pma_line, pmc_line)` pairs, 1-based both sides — Task 6 composes it with the assembler's debug lines.

- [ ] **Step 1: Export the canonical grid.** In `crates/core/src/asm/disassembler.rs` change `fn grid_line` to `pub fn grid_line` and give it a doc comment: `/// Canonical .pma grid (spec §6.4): label col 0, mnemonic col 8, operand col 16; trailing spaces trimmed.` In `crates/core/src/asm/mod.rs` add `pub use disassembler::grid_line;`.

- [ ] **Step 2: Write `crates/post-machine/src/codegen.rs`:**

```rust
//! CFG → `.pma` text (spec §7 module 6). The generated text is fed to
//! the core assembler (spec §2's cc → as pipeline), which supplies
//! encoding, intra-function jump relaxation, and the `ent` prologue via
//! `.func` — codegen never touches bytes.
//!
//! Layout invariant (spec §7, active even at `-O0`): an unconditional
//! transfer to the physically next instruction is never emitted — blocks
//! are laid out in order and fall-through is selected instead.

use std::collections::{HashMap, HashSet};

use mtc_core::asm::grid_line as grid;

use crate::ir::{IrBlock, IrFunction, IrOp, IrProgram, IrTerm};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CodegenOptions {
    /// Drop `brk` ops (`--strip-debugger`).
    pub strip_debugger: bool,
}

/// Generated assembly plus the pma→pmc line correspondence that lets the
/// driver remap assembler debug lines back to `.pmc` sources.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PmaOutput {
    pub text: String,
    /// `(pma_line, pmc_line)`, 1-based on both sides, for every generated
    /// line that carries an instruction or `.func` directive.
    pub line_map: Vec<(u32, u32)>,
}

struct Emitter {
    lines: Vec<String>,
    line_map: Vec<(u32, u32)>,
}

impl Emitter {
    /// `pmc_line == 0` → no source correspondence (label lines, synthetic
    /// return blocks).
    fn push(&mut self, text: String, pmc_line: u32) {
        self.lines.push(text);
        if pmc_line != 0 {
            self.line_map.push((self.lines.len() as u32, pmc_line));
        }
    }
}

pub fn emit_program(ir: &IrProgram, options: CodegenOptions) -> PmaOutput {
    let mut e = Emitter {
        lines: Vec::new(),
        line_map: Vec::new(),
    };
    for f in &ir.functions {
        emit_function(f, options, &mut e);
    }
    let mut text = e.lines.join("\n");
    text.push('\n');
    PmaOutput {
        text,
        line_map: e.line_map,
    }
}

/// Canonical `.pma` name for a block: its first source label (`L5`), or
/// the block id (`B3`) for synthetic blocks. The prefixes cannot collide.
fn block_name(b: &IrBlock) -> String {
    match b.labels.first() {
        Some(l) => format!("L{l}"),
        None => format!("B{}", b.id),
    }
}

fn emit_function(f: &IrFunction, options: CodegenOptions, e: &mut Emitter) {
    let name_of: HashMap<u32, String> =
        f.blocks.iter().map(|b| (b.id, block_name(b))).collect();
    let next_id = |i: usize| f.blocks.get(i + 1).map(|b| b.id);

    // Pass 1: which blocks need a label line — exactly those that some
    // emitted jump will reference (fall-through references nothing).
    let mut referenced: HashSet<u32> = HashSet::new();
    for (i, b) in f.blocks.iter().enumerate() {
        let next = next_id(i);
        match b.term {
            IrTerm::FallThrough { to } | IrTerm::Goto { to } => {
                if next != Some(to) {
                    referenced.insert(to);
                }
            }
            IrTerm::Check { marked, blank } => {
                if next == Some(blank) {
                    referenced.insert(marked);
                } else if next == Some(marked) {
                    referenced.insert(blank);
                } else {
                    referenced.insert(marked);
                    referenced.insert(blank);
                }
            }
            IrTerm::Return | IrTerm::Halt => {}
        }
    }

    e.push(format!(".func {}", f.name), f.line);
    for (i, b) in f.blocks.iter().enumerate() {
        if referenced.contains(&b.id) {
            if b.labels.is_empty() {
                e.push(format!("B{}:", b.id), 0);
            } else {
                // Every source label names the block; jumps use the first.
                for l in &b.labels {
                    e.push(format!("L{l}:"), 0);
                }
            }
        }

        for op in &b.ops {
            match op {
                IrOp::Lft { line } => e.push(grid(None, "lft", ""), *line),
                IrOp::Rgt { line } => e.push(grid(None, "rgt", ""), *line),
                IrOp::Wr { index, line } => e.push(grid(None, "wr", &index.to_string()), *line),
                IrOp::Brk { line } => {
                    if !options.strip_debugger {
                        e.push(grid(None, "brk", ""), *line);
                    }
                }
                IrOp::Call { name, line } => e.push(grid(None, "call", name), *line),
            }
        }

        let next = next_id(i);
        let target = |id: u32| {
            name_of
                .get(&id)
                .expect("terminator targets an existing block")
                .clone()
        };
        match b.term {
            IrTerm::FallThrough { to } | IrTerm::Goto { to } => {
                if next != Some(to) {
                    e.push(grid(None, "jmp", &target(to)), b.term_line);
                }
            }
            IrTerm::Check { marked, blank } => {
                if next == Some(blank) {
                    e.push(grid(None, "jm", &target(marked)), b.term_line);
                } else if next == Some(marked) {
                    e.push(grid(None, "jnm", &target(blank)), b.term_line);
                } else {
                    e.push(grid(None, "jm", &target(marked)), b.term_line);
                    e.push(grid(None, "jmp", &target(blank)), b.term_line);
                }
            }
            IrTerm::Return => {
                // Returning from main stops the machine (spec §3.2).
                let mnemonic = if f.name == "main" { "stp" } else { "ret" };
                e.push(grid(None, mnemonic, ""), b.term_line);
            }
            IrTerm::Halt => e.push(grid(None, "hlt", ""), b.term_line),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn emit_src(src: &str, strip: bool) -> PmaOutput {
        let program = crate::parser::parse(&crate::lexer::lex(src).unwrap()).unwrap();
        let (ir, _) = crate::ir::lower(&program).unwrap();
        emit_program(
            &ir,
            CodegenOptions {
                strip_debugger: strip,
            },
        )
    }

    #[test]
    fn go_to_end_emits_the_plan3_shape() {
        let out = emit_src("goToEnd() { 1: right; check(1, 2); 2: left; }", false);
        assert_eq!(
            out.text,
            "\
.func goToEnd
L1:
        rgt
        jm      L1
        lft
        ret
"
        );
    }

    #[test]
    fn goto_to_next_vanishes_and_unreferenced_labels_drop() {
        let out = emit_src("goToBegin() { 1: left(2); 2: check(1, 3); 3: right(!); }", false);
        assert_eq!(
            out.text,
            "\
.func goToBegin
L1:
        lft
        jm      L1
        rgt
        ret
"
        );
    }

    #[test]
    fn check_with_neither_arm_adjacent_emits_branch_plus_jump() {
        let out = emit_src("f() { 1: check(2, 3); mark; 2: left(!); 3: right(!); }", false);
        assert_eq!(
            out.text,
            "\
.func f
        jm      L2
        jmp     L3
        wr      1
L2:
        lft
        ret
L3:
        rgt
        ret
"
        );
    }

    #[test]
    fn main_returns_as_stp_and_the_synthetic_exit_gets_a_b_label() {
        let out = emit_src("main() { 1: check(!, 2); mark; 2: left; }", false);
        assert_eq!(
            out.text,
            "\
.func main
        jm      B3
        jmp     L2
        wr      1
L2:
        lft
        stp
B3:
        stp
"
        );
    }

    #[test]
    fn strip_debugger_drops_brk() {
        let kept = emit_src("f() { debugger; left; }", false);
        assert!(kept.text.contains("brk"));
        let stripped = emit_src("f() { debugger; left; }", true);
        assert!(!stripped.text.contains("brk"));
    }

    #[test]
    fn calls_and_halt_emit() {
        let out = emit_src("f() { @helper(); halt; }", false);
        assert_eq!(out.text, ".func f\n        call    helper\n        hlt\n");
    }

    #[test]
    fn line_map_points_instructions_at_their_pmc_lines() {
        let out = emit_src("f() {\n    left;\n    right(!);\n}", false);
        assert_eq!(out.text, ".func f\n        lft\n        rgt\n        ret\n");
        // .func ← line 1, lft ← 2, rgt ← 3, ret ← 3 (the `(!)` successor).
        assert_eq!(out.line_map, vec![(1, 1), (2, 2), (3, 3), (4, 3)]);
    }
}
```

- [ ] **Step 3: Register the module.** In `crates/post-machine/src/lib.rs` add `pub mod codegen;`.

- [ ] **Step 4: Run.** `cargo test -p mtc-core` (disassembler untouched behaviorally) and `cargo test -p mtc-post-machine codegen` — all pass. Then the three workspace gates.

- [ ] **Step 5: Commit.**

```bash
git add crates/core/src/asm/disassembler.rs crates/core/src/asm/mod.rs crates/post-machine/src/codegen.rs crates/post-machine/src/lib.rs
git commit -m "feat(post-machine): CFG -> .pma codegen with fall-through layout; export the canonical grid"
```

---

### Task 6: The `compile()` driver

**Files:**
- Modify: `crates/post-machine/src/compiler.rs` (append the driver below the diagnostics)
- Modify: `crates/post-machine/src/lib.rs`

**Interfaces:**
- Consumes: everything from Tasks 2–5, plus `crate::asm::assemble` and `mtc_core::formats::object::ObjectFile`.
- Produces: `compiler::{CompileOptions, CompileReport, CompileOutput, compile}`; crate-root re-exports. This is the public API Plan 7's `pmt compile` wraps.

- [ ] **Step 1: Append to `crates/post-machine/src/compiler.rs`:**

```rust
use mtc_core::formats::object::ObjectFile;

use crate::codegen::{CodegenOptions, emit_program};
use crate::ir::IrProgram;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CompileOptions {
    /// `-g`: record label/line debug info in the object, with lines
    /// remapped to `.pmc` sources.
    pub debug_info: bool,
    /// `--strip-debugger`: drop `brk` at codegen (spec §10).
    pub strip_debugger: bool,
}

/// Structured stage report — `pmt -v` renders it; the library never
/// prints (spec §10, the LinkReport pattern).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompileReport {
    pub warnings: Vec<Warning>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompileOutput {
    pub object: ObjectFile,
    /// The generated assembly (`-S` output). The object IS its assembly,
    /// so the two can never disagree.
    pub pma: String,
    /// The lowered CFG (`--emit-ir`). At `-O0` this is also the final IR;
    /// Plan 6 adds per-pass snapshots.
    pub ir: IrProgram,
    pub report: CompileReport,
}

/// `.pmc` source → object file (spec §7): lex → parse → lower → emit
/// `.pma` → assemble. Assembly failure of GENERATED text is a compiler
/// bug and reports as `CompileErrorKind::Internal`.
pub fn compile(source: &str, options: CompileOptions) -> Result<CompileOutput, CompileError> {
    let tokens = crate::lexer::lex(source)?;
    let program = crate::parser::parse(&tokens)?;
    let (ir, warnings) = crate::ir::lower(&program)?;
    let pma = emit_program(
        &ir,
        CodegenOptions {
            strip_debugger: options.strip_debugger,
        },
    );
    let mut object = crate::asm::assemble(&pma.text, options.debug_info).map_err(|e| {
        CompileError {
            line: 0,
            col: 0,
            kind: CompileErrorKind::Internal(format!("generated .pma failed to assemble: {e}")),
        }
    })?;
    if options.debug_info {
        remap_debug_lines(&mut object, &pma.line_map);
    }
    Ok(CompileOutput {
        object,
        pma: pma.text,
        ir,
        report: CompileReport { warnings },
    })
}

/// The assembler recorded `(code_offset, pma_line)`; compose with the
/// codegen's `(pma_line, pmc_line)` map so debug info speaks `.pmc`.
/// Offsets with no source correspondence (synthetic returns) are dropped.
fn remap_debug_lines(object: &mut ObjectFile, line_map: &[(u32, u32)]) {
    let to_pmc: std::collections::HashMap<u32, u32> = line_map.iter().copied().collect();
    if let Some(per_blob) = &mut object.debug {
        for d in per_blob {
            d.lines = d
                .lines
                .iter()
                .filter_map(|&(off, pma_line)| to_pmc.get(&pma_line).map(|&l| (off, l)))
                .collect();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mtc_core::formats::object::SymbolDef;

    #[test]
    fn compiles_to_an_object_with_symbols_and_relocations() {
        let out = compile("main() { @goToEnd(); mark; }", CompileOptions::default()).unwrap();
        assert!(out.object.symbols.iter().any(
            |s| s.name == "main" && matches!(s.def, SymbolDef::Defined { .. })
        ));
        assert!(out.object.symbols.iter().any(
            |s| s.name == "goToEnd" && matches!(s.def, SymbolDef::External)
        ));
        assert_eq!(out.object.relocations.len(), 1);
        assert!(out.report.warnings.is_empty());
    }

    #[test]
    fn object_equals_assembly_of_the_emitted_pma() {
        let out = compile("f() { 1: right; check(1, !); }", CompileOptions::default()).unwrap();
        let direct = crate::asm::assemble(&out.pma, false).unwrap();
        assert_eq!(out.object, direct);
    }

    #[test]
    fn debug_lines_speak_pmc_not_pma() {
        let src = "main() {\n    right;\n    mark;\n}";
        let out = compile(
            src,
            CompileOptions {
                debug_info: true,
                strip_debugger: false,
            },
        )
        .unwrap();
        let debug = out.object.debug.as_ref().unwrap();
        let lines = &debug[0].lines;
        // Blob: ent@0, rgt@1, wr@2..3, stp@4. Sources: right; = pmc line 2,
        // mark; = line 3, implicit stp ← the line-3 statement.
        assert!(lines.contains(&(1, 2)), "{lines:?}");
        assert!(lines.contains(&(2, 3)), "{lines:?}");
        assert!(lines.contains(&(4, 3)), "{lines:?}");
    }

    #[test]
    fn warnings_flow_into_the_report() {
        let out = compile("f() { goto 1; right; 1: left; }", CompileOptions::default()).unwrap();
        assert_eq!(out.report.warnings.len(), 1);
    }

    #[test]
    fn strip_debugger_reaches_the_bytes() {
        let src = "main() { debugger; mark; }";
        let kept = compile(src, CompileOptions::default()).unwrap();
        assert!(kept.object.blobs[0].contains(&crate::arch::opcodes::BRK));
        let stripped = compile(
            src,
            CompileOptions {
                debug_info: false,
                strip_debugger: true,
            },
        )
        .unwrap();
        assert!(!stripped.object.blobs[0].contains(&crate::arch::opcodes::BRK));
    }
}
```

- [ ] **Step 2: Re-export at the crate root.** In `crates/post-machine/src/lib.rs`:

```rust
pub use compiler::{
    CompileError, CompileErrorKind, CompileOptions, CompileOutput, CompileReport, Warning, compile,
};
```

- [ ] **Step 3: Run.** `cargo test -p mtc-post-machine compiler` — all pass. Then the three workspace gates.

- [ ] **Step 4: Commit.**

```bash
git add crates/post-machine/src/compiler.rs crates/post-machine/src/lib.rs
git commit -m "feat(post-machine): compile() driver — .pmc to object with pma/ir artifacts and pmc-line debug remap"
```

---

### Task 7: Golden end-to-end tests

**Files:**
- Create: `crates/post-machine/tests/compile_programs.rs`

**Interfaces:**
- Consumes: the full public pipeline — `compile`, `mtc_post_machine::asm::link`, `Machine`/`InfiniteTape`/`RunOptions` — nothing new is produced.

The byte expectations below were hand-derived; they are the golden contract. If any assertion fails, first re-derive by hand — if the plan's arithmetic is wrong, report BLOCKED with your derivation (Task-5-of-Plan-4 precedent: controller ratifies). Note: `.pma` labels are function-local (both `goToEnd` and `goToBegin` use `L1`) — if the assembler rejects that, it is a core bug; BLOCK.

- [ ] **Step 1: Write `crates/post-machine/tests/compile_programs.rs`:**

```rust
//! The first COMPILED Post-machine programs (spec §11's golden path):
//! `.pmc` → compile → link → run, pinning the spec §3 sample end to end.

use mtc_core::linker::LinkOptions;
use mtc_core::vm::{ArchRegistry, InfiniteTape, Machine, Outcome, RunOptions};
use mtc_post_machine::arch::Pm1;
use mtc_post_machine::asm::link;
use mtc_post_machine::compiler::{CompileOptions, compile};
use mtc_post_machine::ir::IrProgram;

/// Spec §3's source sample, verbatim modulo comments.
const SPEC_PMC: &str = "\
goToEnd() {
1:  right;
    check(1, 2);
2:  left;
}

goToBegin() {
1:  left(2);
2:  check(1, 3);
3:  right(!);
}

main() {
    @goToEnd();
    right;
    check(3, 4);
3:  unmark(!);
4:  mark;
}
";

const EXPECTED_PMA: &str = "\
.func goToEnd
L1:
        rgt
        jm      L1
        lft
        ret
.func goToBegin
L1:
        lft
        jm      L1
        rgt
        ret
.func main
        call    goToEnd
        rgt
        jnm     L4
        wr      0
        stp
L4:
        wr      1
        stp
";

fn registry() -> ArchRegistry {
    let mut r = ArchRegistry::new();
    r.register(Box::new(Pm1));
    r
}

#[test]
fn spec_sample_compiles_to_the_expected_assembly() {
    let out = compile(SPEC_PMC, CompileOptions::default()).unwrap();
    assert_eq!(out.pma, EXPECTED_PMA);
    assert!(out.report.warnings.is_empty());
}

#[test]
fn spec_sample_links_byte_exact() {
    use mtc_post_machine::arch::opcodes::*;
    let out = compile(SPEC_PMC, CompileOptions::default()).unwrap();
    let linked = link(&[out.object], &[], LinkOptions::default()).unwrap();
    // main: ent@0, call.s@1 (end 3, goToEnd at 12 → +9), rgt@3,
    // jnm.s@4 (end 6, L4 at 9 → +3), wr 0 @6..7, stp@8, wr 1 @9..10,
    // stp@11; goToEnd at 12: ent, rgt, jm.s −3, lft, ret.
    assert_eq!(
        linked.executable.code,
        vec![
            ENT, CALL_S, 0x09, RGT, JNM_S, 0x03, WR, 0x80, STP, WR, 0x81, STP, // main
            ENT, RGT, JM_S, 0xFD, LFT, RET, // goToEnd
        ]
    );
    assert_eq!(linked.executable.entry, 0);
}

#[test]
fn spec_sample_runs_and_drops_the_dead_function() {
    let out = compile(SPEC_PMC, CompileOptions::default()).unwrap();
    let linked = link(&[out.object], &[], LinkOptions::default()).unwrap();
    assert_eq!(linked.report.dropped, vec!["goToBegin".to_string()]);

    // Marks at 0..=2, head on the first mark (the Plan 4 scenario).
    let reg = registry();
    let machine = Machine::from_executable(&linked.executable, &reg).unwrap();
    let mut tape = InfiniteTape::from_cells([true, true, true], 0, 0);
    let result = machine.run(&mut tape, RunOptions::default());
    assert_eq!(result.outcome, Outcome::Stopped);
    // goToEnd: right to the first blank (3), check → left (2), return;
    // main: right (3), check → blank arm → mark cell 3, stop.
    assert_eq!(tape.head(), 3);
    assert_eq!(tape.marked_cells(), vec![0, 1, 2, 3]);
}

#[test]
fn debug_build_maps_executable_offsets_to_pmc_lines() {
    let out = compile(
        SPEC_PMC,
        CompileOptions {
            debug_info: true,
            strip_debugger: false,
        },
    )
    .unwrap();
    let linked = link(&[out.object], &[], LinkOptions::default()).unwrap();
    let main = linked.map.functions.iter().find(|f| f.name == "main").unwrap();
    // rgt at absolute 3 ← `right;` on SPEC_PMC line 15.
    assert!(main.lines.contains(&(3, 15)), "{:?}", main.lines);
    assert!(main.labels.contains(&("L4".to_string(), 9)), "{:?}", main.labels);
    let go = linked.map.functions.iter().find(|f| f.name == "goToEnd").unwrap();
    // goToEnd's rgt at absolute 13 ← `right;` on line 2.
    assert!(go.lines.contains(&(13, 2)), "{:?}", go.lines);
    assert!(go.labels.contains(&("L1".to_string(), 13)), "{:?}", go.labels);
}

#[test]
fn emitted_ir_is_a_versioned_json_artifact() {
    let out = compile(SPEC_PMC, CompileOptions::default()).unwrap();
    let json = out.ir.to_json();
    let back = IrProgram::from_json(&json).unwrap();
    assert_eq!(back, out.ir);
    assert_eq!(back.version, 1);
    assert_eq!(back.functions.len(), 3);
}

#[test]
fn unicode_identifiers_survive_compile_and_link() {
    let src = "\
идиВКонец() {
1:  right;
    check(1, 2);
2:  left;
}

main() {
    @идиВКонец();
    mark;
}
";
    let out = compile(src, CompileOptions::default()).unwrap();
    let linked = link(&[out.object], &[], LinkOptions::default()).unwrap();
    let names: Vec<&str> = linked.map.functions.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(names, vec!["main", "идиВКонец"]);

    let reg = registry();
    let machine = Machine::from_executable(&linked.executable, &reg).unwrap();
    let mut tape = InfiniteTape::from_cells([true, true], 0, 0);
    let result = machine.run(&mut tape, RunOptions::default());
    assert_eq!(result.outcome, Outcome::Stopped);
    assert_eq!(tape.marked_cells(), vec![0, 1]);
}

#[test]
fn a_pmc_compiled_library_links_lazily() {
    let lib = compile(
        "goToEnd() { 1: right; check(1, 2); 2: left; } unusedHelper() { halt; }",
        CompileOptions::default(),
    )
    .unwrap();
    let main = compile(
        "main() { @goToEnd(); right; mark; }",
        CompileOptions::default(),
    )
    .unwrap();
    let linked = link(&[main.object], &[lib.object], LinkOptions::default()).unwrap();
    let names: Vec<&str> = linked.map.functions.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(names, vec!["main", "goToEnd"]);
}

#[test]
fn halt_program_halts() {
    let out = compile("main() { right; halt; }", CompileOptions::default()).unwrap();
    let linked = link(&[out.object], &[], LinkOptions::default()).unwrap();
    let reg = registry();
    let machine = Machine::from_executable(&linked.executable, &reg).unwrap();
    let mut tape = InfiniteTape::from_cells([true], 0, 0);
    let result = machine.run(&mut tape, RunOptions::default());
    assert_eq!(result.outcome, Outcome::Halted);
}
```

- [ ] **Step 2: Run the suite.** `cargo test -p mtc-post-machine --test compile_programs` — all pass.

- [ ] **Step 3: Run the full gates.** `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo fmt --check`.

- [ ] **Step 4: Commit.**

```bash
git add crates/post-machine/tests/compile_programs.rs
git commit -m "test(post-machine): golden compile -> link -> run for the spec sample"
```

---

## Plan Self-Review Notes

- **Spec coverage.** §3 language: lexer (Task 2), parser with all statement forms and rules (Task 3). §7 modules 1–4 and 6: Tasks 2–5 (module 5, the optimizer, is Plan 6 by design). §7.1: versioned JSON IR (Task 4), surfaced via `CompileOutput.ir` (Task 6); `pmt ir graph`/`--emit-ir` CLI plumbing is Plan 7. §10 build modes: `-g` and `--strip-debugger` land here as `CompileOptions`; `-O0/-O1` switches arrive with Plan 6. §11: golden end-to-end for the spec sample (Task 7); Sum/Ty historical ports ride with Plan 7's CLI goldens. Plan 4 deferrals: all five in Task 1 (the `.byte`-dedup lives in Task 1, `grid_line` export in Task 5).
- **Type consistency hand-check.** `CompileErrorKind` is closed in Task 2 and only constructed afterwards. `parser::Item` shapes match `ir.rs`'s `match` arms field-for-field. `PmaOutput.line_map` (Task 5) is exactly what `remap_debug_lines` (Task 6) consumes. `IrTerm` carries no lines; `IrBlock.term_line` covers terminator-emitted instructions.
- **Derived-byte audit.** Task 7's 18-byte executable and the `(3, 15)`/`(13, 2)` line pairs were derived twice (blob layout, then post-relaxation layout). goToEnd's blob `[0D 05 19 FD 04 0C]` matches Plan 3's assembled sample exactly — an independent cross-check.
- **Known asymmetry, accepted:** `compile()` always produces all three artifacts (object, pma, ir). At this scale the extra cost is a string and a small struct; option-gating output selection is CLI (Plan 7) territory.


