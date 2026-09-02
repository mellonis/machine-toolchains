# `mtc-wasm` Binding Crate Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A fourth crate, `mtc-wasm`, that exposes compile → link → run, lint, fmt and the disassembly of both toolchains to JavaScript through wasm-bindgen, shipped as a checksummed bundle attached to each GitHub release and smoke-tested under Node in CI.

**Architecture:** `crates/wasm/src/inner/` is plain Rust over the three crates' public APIs (positions, diagnostics, program, listing, session, registry) and is tested natively with nextest. `crates/wasm/src/lib.rs` plus `js.rs` is the thin wasm-bindgen layer: three classes (`Toolchain`, `Program`, `Session`), plain JS objects for every data type, a TypeScript custom section for their types. `scripts/build-wasm-bundle.sh` builds the bundle; `scripts/wasm-smoke.mjs` proves it under Node; `release.yml` attaches it to the tagged release.

**Tech Stack:** Rust 1.98.0 (pinned), wasm-bindgen 0.2.127 + js-sys 0.3.104 (pinned exactly), wasm-bindgen-cli at the same version, binaryen `wasm-opt`, Node 24, GitHub Actions.

**Spec:** `docs/superpowers/specs/2026-09-02-wasm-binding-design.md`

## Global Constraints

- **No commit without the maintainer's explicit go-ahead.** Every task ends with a drafted commit message; the commit itself waits for the word. Conventional commits with scope: `feat(wasm):`, `test(wasm):`, `ci:`, `docs(wasm):`. No Claude attribution anywhere.
- The three existing crates keep their dependency rule: `serde`/`serde_json` only. `wasm-bindgen` and `js-sys` appear in `crates/wasm/Cargo.toml` and nowhere else.
- **No change to `crates/core`.** PM-1 byte-identity and core neutrality are not touched by this arc. The one library edit outside `crates/wasm` is the release-build warning fix in `crates/post-machine/src/fmt/print.rs` (Task 1).
- `crates/wasm/src/inner/` never names `wasm_bindgen` or `js_sys` (Task 1's boundary test enforces it).
- Positions cross as **UTF-16 half-open offsets**. Core `Span`/`Pos` are 1-based, `col` counts characters.
- The channel split: `check` = lint findings + a compile fatal as one error; `build` = compile channel warnings alongside the program.
- `debug_info: true` on every browser compile.
- Published content (`docs/wasm.md`, README, code comments) is forge-agnostic: no issue numbers, no URLs. Code comments cite `docs/wasm.md (<topic>)` only after Task 11 lands the page; until then, prose.
- Test temp paths, if any, use PID plus an atomic counter. This plan's tests need none.
- Gate before declaring any task done: `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo nextest run -p mtc-wasm` (and `--workspace` at the end of Tasks 7, 9, 11).

---

## File map

| path | responsibility |
|---|---|
| `Cargo.toml` (workspace) | add `crates/wasm` member; add `[profile.wasm]` |
| `crates/wasm/Cargo.toml` | the crate: `cdylib` + `rlib`, pinned wasm-bindgen |
| `crates/wasm/src/lib.rs` | wasm-bindgen classes `Toolchain`, `Program`, `Session`; TS custom section |
| `crates/wasm/src/js.rs` | Rust → `JsValue` converters and JS-object readers (the only other file naming `js_sys`) |
| `crates/wasm/src/inner/mod.rs` | `Lang`, re-exports |
| `crates/wasm/src/inner/registry.rs` | per-tape-count cache of leaked `ArchRegistry`s |
| `crates/wasm/src/inner/positions.rs` | `Utf16Index`: core `Pos`/`Span` → UTF-16 offsets |
| `crates/wasm/src/inner/diagnostics.rs` | `Diag`, `check`, `format`, fatal/lint conversion |
| `crates/wasm/src/inner/program.rs` | `build`, `Program` (exe + map + line index + layouts) |
| `crates/wasm/src/inner/listing.rs` | structured listing rows over `listing_parts` |
| `crates/wasm/src/inner/session.rs` | `Session` over `AsyncSession` with owned `WideTape`s |
| `crates/wasm/tests/{boundary,positions,diagnostics,program,listing,session}.rs` | native tests, one file per module |
| `scripts/build-wasm-bundle.sh` | cargo → wasm-bindgen → wasm-opt → manifest → tarball |
| `scripts/wasm-smoke.mjs` | Node end-to-end over the built bundle, size ceiling |
| `.github/workflows/test.yml` | bundle build + smoke steps |
| `.github/workflows/release.yml` | tag-triggered bundle upload |
| `docs/wasm.md`, `README.md`, `CLAUDE.md` | the durable page, the front door, the router |
| `crates/post-machine/src/fmt/print.rs` | release-build warning fix |

Shared fixtures (used verbatim by several tasks — copy them, do not import across test files; there is no shared test-support module in this repo):

```rust
/// Unary increment (historic ex000001). Seed [1,1,1] head 0 → [1,1,1,1] head 0.
pub const PMC_INC: &str = "main() {\n    1: right(2);\n    2: check(1, 3);\n    3: mark(4);\n    4: left(5);\n    5: check(4, 6);\n    6: right(!);\n}\n";

/// A `.pmc` with one `unused-label` finding (label 5 is never referenced).
pub const PMC_UNUSED_LABEL: &str = "namespace api {\nhelper() {\n5: right;\n}\n}\nmain() { @api::helper(); }\n";

/// Replace every 'b' by 'a' walking right; stop on the first blank.
/// Alphabet indices: '_'=0, 'a'=1, 'b'=2. Seed [1,2,2] head 0 → [1,1,1] head 3.
pub const TMC_REPLACE_B: &str = "alphabet ab { '_', 'a', 'b' }\n\nmachine {\n  tape main: ab;\n\n  entry state scan {\n    ['b'] -> write ['a'] move [>] goto scan;\n    ['a'] ->             move [>] goto scan;\n    ['_'] -> stop;\n  }\n}\n";

/// TMC_REPLACE_B plus an alphabet nothing uses → one `unused-alphabet` finding.
pub const TMC_UNUSED_ALPHABET: &str = "alphabet ab { '_', 'a', 'b' }\nalphabet spare { '_', 'x' }\n\nmachine {\n  tape main: ab;\n\n  entry state scan {\n    ['b'] -> write ['a'] move [>] goto scan;\n    ['a'] ->             move [>] goto scan;\n    ['_'] -> stop;\n  }\n}\n";
```

---

### Task 1: Crate scaffold, registry, boundary test, release-warning fix

**Files:**
- Modify: `Cargo.toml` (workspace root)
- Create: `crates/wasm/Cargo.toml`, `crates/wasm/src/lib.rs`, `crates/wasm/src/inner/mod.rs`, `crates/wasm/src/inner/registry.rs`
- Create: `crates/wasm/tests/boundary.rs`
- Modify: `crates/post-machine/src/fmt/print.rs:142-146`

**Interfaces:**
- Produces: `mtc_wasm::inner::Lang { Pmc, Tmc }` with `Lang::parse(&str) -> Option<Lang>` and `Lang::as_str(&self) -> &'static str`; `mtc_wasm::inner::registry::registry_for(tape_count: u8) -> &'static mtc_core::vm::ArchRegistry`.

- [ ] **Step 1: Add the member and the wasm profile to the workspace manifest**

Append to `Cargo.toml` (workspace root) — the `members` line changes, the profile is new:

```toml
members = ["crates/core", "crates/post-machine", "crates/turing-machine", "crates/wasm"]
```

```toml
# The browser bundle's profile. Scoped as its own profile rather than
# overrides on `release`: `lto`, `panic` and `strip` are profile-wide
# settings that per-package overrides cannot set, and the two CLIs keep
# their ordinary release profile. Measured 2026-09-02: `opt-level = "z"`
# came out 25% smaller than `"s"` for this module with no observable cost.
[profile.wasm]
inherits = "release"
opt-level = "z"
lto = "fat"
codegen-units = 1
panic = "abort"
strip = true
```

- [ ] **Step 2: Write the crate manifest**

`crates/wasm/Cargo.toml`:

```toml
[package]
name = "mtc-wasm"
version = "0.4.0"
edition.workspace = true
license.workspace = true
repository.workspace = true
description = "Browser binding for the machine toolchains: compile, lint, format, disassemble and run PM-1 and TM-1 programs from JavaScript"
publish = false

[lib]
crate-type = ["cdylib", "rlib"]

[dependencies]
mtc-core = { path = "../core" }
mtc-post-machine = { path = "../post-machine" }
mtc-turing-machine = { path = "../turing-machine" }
# Pinned exactly: the wasm-bindgen CLI that post-processes the module must
# be the same version, and scripts/build-wasm-bundle.sh reads this line.
wasm-bindgen = "=0.2.127"
js-sys = "0.3.104"
```

- [ ] **Step 3: Write the boundary test (fails: no crate yet)**

`crates/wasm/tests/boundary.rs`:

```rust
//! The layer under wasm-bindgen is plain Rust: nothing in `src/inner/`
//! may name `wasm_bindgen` or `js_sys`, so it stays testable natively and
//! the JS boundary stays in `lib.rs` + `js.rs`, where a reader expects it.

use std::fs;
use std::path::Path;

#[test]
fn inner_module_never_names_the_js_boundary() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/inner");
    let mut checked = 0;
    for entry in fs::read_dir(&dir).expect("src/inner exists") {
        let path = entry.expect("entry").path();
        if path.extension().is_some_and(|e| e == "rs") {
            let text = fs::read_to_string(&path).expect("readable");
            for needle in ["wasm_bindgen", "js_sys"] {
                assert!(
                    !text.contains(needle),
                    "{} names `{needle}`; the JS boundary belongs in lib.rs/js.rs",
                    path.display()
                );
            }
            checked += 1;
        }
    }
    assert!(checked >= 2, "expected the inner modules under {}", dir.display());
}

#[test]
fn registry_serves_both_arches_per_tape_count() {
    use mtc_wasm::inner::registry::registry_for;
    let r1 = registry_for(1);
    assert!(r1.get(0x01).is_some(), "PM-1 registered");
    assert!(r1.get(0x02).is_some(), "TM-1 registered");
    assert!(r1.get(0x7F).is_none(), "the fake test arch is not");
    let again = registry_for(1);
    assert!(std::ptr::eq(r1, again), "one registry per tape count, cached");
    let r4 = registry_for(4);
    assert!(!std::ptr::eq(r1, r4), "a different tape count is a different registry");
}
```

- [ ] **Step 4: Run it to see it fail**

Run: `cargo test -p mtc-wasm --test boundary`
Expected: FAIL — `error: package ID specification 'mtc-wasm' did not match any packages` (or the crate has no `inner` module).

- [ ] **Step 5: Write the crate skeleton**

`crates/wasm/src/lib.rs`:

```rust
//! Browser binding for the machine toolchains. `inner/` is plain Rust over
//! the three crates' public APIs and is what the native tests exercise;
//! this file and `js.rs` are the wasm-bindgen layer over it (filled in by
//! the later tasks of this arc).

#[doc(hidden)]
pub mod inner;
```

`crates/wasm/src/inner/mod.rs`:

```rust
//! The layer under the JS boundary: plain Rust, natively testable.

pub mod registry;

/// Which source language a call is about. The public library APIs of the
/// two toolchains are symmetric for everything the binding exposes, so one
/// class family with a `lang` parameter serves both.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    Pmc,
    Tmc,
}

impl Lang {
    pub fn parse(s: &str) -> Option<Lang> {
        match s {
            "pmc" => Some(Lang::Pmc),
            "tmc" => Some(Lang::Tmc),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Lang::Pmc => "pmc",
            Lang::Tmc => "tmc",
        }
    }
}
```

`crates/wasm/src/inner/registry.rs`:

```rust
//! Arch registries with a `'static` lifetime.
//!
//! `Machine<'a>` and `AsyncSession<'a>` borrow the registry they were
//! loaded from, and a wasm-bindgen class cannot carry a lifetime — so the
//! registry has to outlive everything, i.e. be `'static`. A single global
//! does not work: `Tm1::new(tape_count)` is per program. Instead one
//! registry per tape count is leaked on first use and cached; the cache is
//! bounded by the `u8` tape count, so the leak is at most 255 small boxes
//! per process, and a browser page is one process.

use std::cell::RefCell;
use std::collections::HashMap;

use mtc_core::vm::ArchRegistry;
use mtc_post_machine::arch::Pm1;
use mtc_turing_machine::arch::Tm1;

thread_local! {
    static REGISTRIES: RefCell<HashMap<u8, &'static ArchRegistry>> = RefCell::new(HashMap::new());
}

/// The registry holding PM-1 and TM-1 (the latter sized for `tape_count`
/// tapes). The same reference comes back for the same count.
pub fn registry_for(tape_count: u8) -> &'static ArchRegistry {
    REGISTRIES.with(|cache| {
        *cache.borrow_mut().entry(tape_count).or_insert_with(|| {
            let mut registry = ArchRegistry::new();
            registry.register(Box::new(Pm1));
            registry.register(Box::new(Tm1::new(tape_count)));
            Box::leak(Box::new(registry))
        })
    })
}
```

- [ ] **Step 6: Run the test to see it pass**

Run: `cargo test -p mtc-wasm --test boundary`
Expected: PASS, 2 tests. (If `Tm1::new` takes a different integer type than `u8`, convert with `.into()`/`as` at the call and keep the `u8` cache key — `Executable::tape_count` is a `u8`.)

- [ ] **Step 7: Fix the release-build warning in the PM formatter**

In `crates/post-machine/src/fmt/print.rs`, `format_tree` reads `source` only under `cfg(debug_assertions)` (the conservation gate), so every `--release` build warns. Add, as the first line of the function body:

```rust
    // `source` feeds the conservation gate, which is compiled only with
    // debug assertions; without them the parameter would be unused.
    #[cfg(not(debug_assertions))]
    let _ = source;
```

- [ ] **Step 8: Verify the warning is gone and the gates hold**

Run: `cargo build --release -p mtc-post-machine 2>&1 | grep -c 'unused variable'`
Expected: `0`

Run: `cargo fmt --check && cargo clippy --workspace --all-targets -- -D warnings && cargo build --workspace --lib --target wasm32-unknown-unknown`
Expected: all clean; the wasm32 gate now also builds `mtc-wasm`.

- [ ] **Step 9: Drafted commit (wait for the go-ahead)**

```
feat(wasm): scaffold the mtc-wasm crate with a per-tape-count arch registry

Fourth workspace member, cdylib + rlib, the only crate carrying
wasm-bindgen (pinned exactly). The inner module is plain Rust and a
boundary test keeps it that way. Registries are leaked once per tape
count so machines and sessions get the 'static lifetime a wasm-bindgen
class needs. Also silences the release-only unused-variable warning in
the PM formatter, since the bundle build is the first release-profile
build CI will run.
```

---

### Task 2: `Utf16Index` — core positions to UTF-16 offsets

**Files:**
- Create: `crates/wasm/src/inner/positions.rs`
- Modify: `crates/wasm/src/inner/mod.rs` (add `pub mod positions;`)
- Test: `crates/wasm/tests/positions.rs`

**Interfaces:**
- Consumes: `mtc_core::diagnostics::{Pos, Span}` (`Pos { line: u32, col: u32 }`, 1-based, `col` counts characters; `Span { start: Pos, end: Pos }`, half-open).
- Produces: `Utf16Index::new(text: &str) -> Utf16Index`; `offset(&self, pos: Pos) -> u32`; `span(&self, span: &Span) -> (u32, u32)`; `len(&self) -> u32` (text length in UTF-16 units).

- [ ] **Step 1: Write the failing tests**

`crates/wasm/tests/positions.rs`:

```rust
//! Core spans are 1-based (line, character column); the browser editor
//! wants half-open UTF-16 string offsets. These pin the conversion where
//! the two disagree: astral glyphs, CRLF, and positions past the end.

use mtc_core::diagnostics::{Pos, Span};
use mtc_wasm::inner::positions::Utf16Index;

fn pos(line: u32, col: u32) -> Pos {
    Pos { line, col }
}

#[test]
fn ascii_lines_are_plain_arithmetic() {
    let idx = Utf16Index::new("ab\ncde\n");
    assert_eq!(idx.offset(pos(1, 1)), 0);
    assert_eq!(idx.offset(pos(1, 3)), 2); // one past 'b' — a span end
    assert_eq!(idx.offset(pos(2, 1)), 3);
    assert_eq!(idx.offset(pos(2, 4)), 6);
    assert_eq!(idx.len(), 7);
}

#[test]
fn astral_glyph_counts_two_units() {
    // '𝔞' (U+1D51E) is one character and two UTF-16 code units.
    let idx = Utf16Index::new("x𝔞y\nz");
    assert_eq!(idx.offset(pos(1, 2)), 1); // before the glyph
    assert_eq!(idx.offset(pos(1, 3)), 3); // after the glyph: 1 + 2
    assert_eq!(idx.offset(pos(1, 4)), 4);
    assert_eq!(idx.offset(pos(2, 1)), 5); // the newline is one unit
}

#[test]
fn crlf_keeps_the_carriage_return_in_the_line() {
    let idx = Utf16Index::new("ab\r\ncd");
    assert_eq!(idx.offset(pos(2, 1)), 4);
    assert_eq!(idx.offset(pos(1, 3)), 2); // the '\r' is column 3, still line 1
}

#[test]
fn positions_past_the_end_clamp() {
    let idx = Utf16Index::new("ab\ncd");
    assert_eq!(idx.offset(pos(2, 99)), 5);
    assert_eq!(idx.offset(pos(99, 1)), 5);
    assert_eq!(idx.offset(pos(0, 0)), 0, "a zero position clamps to the start");
}

#[test]
fn span_is_half_open_and_ordered() {
    let idx = Utf16Index::new("hello\nworld\n");
    let (from, to) = idx.span(&Span { start: pos(2, 1), end: pos(2, 6) });
    assert_eq!((from, to), (6, 11));
    let (from, to) = idx.span(&Span { start: pos(2, 6), end: pos(2, 1) });
    assert_eq!((from, to), (6, 11), "a reversed span is normalised, never negative");
}
```

- [ ] **Step 2: Run to see them fail**

Run: `cargo test -p mtc-wasm --test positions`
Expected: FAIL — `unresolved import mtc_wasm::inner::positions`.

- [ ] **Step 3: Implement**

`crates/wasm/src/inner/positions.rs`:

```rust
//! Core `Pos`/`Span` (1-based line and character column) → UTF-16 offsets,
//! the coordinate system a browser editor indexes strings by.

use mtc_core::diagnostics::{Pos, Span};

/// Built once per source text; `offset` is then O(line length).
pub struct Utf16Index {
    text: String,
    /// Byte offset where each line starts (line i is `line_bytes[i]..`).
    line_bytes: Vec<usize>,
    /// UTF-16 offset where each line starts.
    line_units: Vec<u32>,
    /// Total length in UTF-16 units.
    len_units: u32,
}

impl Utf16Index {
    pub fn new(text: &str) -> Utf16Index {
        let mut line_bytes = vec![0];
        let mut line_units = vec![0];
        let mut units: u32 = 0;
        for (byte, ch) in text.char_indices() {
            units += ch.len_utf16() as u32;
            if ch == '\n' {
                line_bytes.push(byte + 1);
                line_units.push(units);
            }
        }
        Utf16Index {
            text: text.to_owned(),
            line_bytes,
            line_units,
            len_units: units,
        }
    }

    pub fn len(&self) -> u32 {
        self.len_units
    }

    /// Total: a line past the end lands at the text end; a column past the
    /// line end lands at the line end (the newline excluded, so a span
    /// ending at end-of-line never swallows it).
    pub fn offset(&self, pos: Pos) -> u32 {
        if pos.line == 0 {
            return 0;
        }
        let line = (pos.line - 1) as usize;
        if line >= self.line_bytes.len() {
            return self.len_units;
        }
        let start = self.line_bytes[line];
        let end = self
            .line_bytes
            .get(line + 1)
            .map(|next| next - 1) // exclude the '\n'
            .unwrap_or(self.text.len());
        let chars_before = pos.col.saturating_sub(1) as usize;
        let units: u32 = self.text[start..end]
            .chars()
            .take(chars_before)
            .map(|c| c.len_utf16() as u32)
            .sum();
        self.line_units[line] + units
    }

    /// Half-open `(from, to)`, normalised so `from <= to`.
    pub fn span(&self, span: &Span) -> (u32, u32) {
        let a = self.offset(span.start);
        let b = self.offset(span.end);
        if a <= b { (a, b) } else { (b, a) }
    }
}
```

Add `pub mod positions;` to `crates/wasm/src/inner/mod.rs`.

- [ ] **Step 4: Run to see them pass**

Run: `cargo test -p mtc-wasm --test positions`
Expected: PASS, 5 tests.

- [ ] **Step 5: Gates, then drafted commit**

Run: `cargo fmt --check && cargo clippy -p mtc-wasm --all-targets -- -D warnings`

```
feat(wasm): map core spans to UTF-16 offsets

Core positions are 1-based line and character column; the browser
editor indexes strings by UTF-16 unit. One index per source text,
clamping past-the-end positions the way the LSP mapping does.
```

---

### Task 3: Diagnostics, `check`, and `format`

**Files:**
- Create: `crates/wasm/src/inner/diagnostics.rs`
- Modify: `crates/wasm/src/inner/mod.rs` (add `pub mod diagnostics;`)
- Test: `crates/wasm/tests/diagnostics.rs`

**Interfaces:**
- Consumes: Task 2's `Utf16Index`; `mtc_core::diagnostics::{Diagnostic, Fix, Edit, Applicability}`; `mtc_post_machine::lint::{lint, LintOptions, LintError}`, `mtc_turing_machine::lint::{lint, LintOptions, LintError}`; `mtc_post_machine::fmt::format`, `mtc_turing_machine::fmt::format`; both crates' `compiler::CompileError { span, kind }` with `kind.code() -> &'static str` and `Display`.
- Produces:
  ```rust
  pub enum Severity { Error, Warning }
  pub struct Edit { pub from: u32, pub to: u32, pub replacement: String }
  pub struct Fix { pub description: String, pub machine_applicable: bool, pub edits: Vec<Edit> }
  pub struct Diag { pub code: String, pub severity: Severity, pub from: u32, pub to: u32, pub message: String, pub fix: Option<Fix> }
  pub struct CheckOptions { pub allow: Vec<String>, pub warn: Vec<String> }
  pub enum CheckError { UnknownAllowCode(String) }
  pub fn check(lang: Lang, source: &str, opts: &CheckOptions) -> Result<Vec<Diag>, CheckError>
  pub fn format(lang: Lang, source: &str) -> Result<String, Diag>
  pub fn from_core(idx: &Utf16Index, d: &mtc_core::diagnostics::Diagnostic, severity: Severity) -> Diag
  pub fn pm_fatal(idx: &Utf16Index, e: &mtc_post_machine::compiler::CompileError) -> Diag
  pub fn tm_fatal(idx: &Utf16Index, e: &mtc_turing_machine::compiler::CompileError) -> Diag
  ```

- [ ] **Step 1: Write the failing tests**

`crates/wasm/tests/diagnostics.rs`:

```rust
//! `check` is the lint channel (findings, plus a compile fatal rendered as
//! one error); compile warnings travel with `build`, never here — the same
//! split the CLI keeps between `lint` and `compile`.

use mtc_wasm::inner::Lang;
use mtc_wasm::inner::diagnostics::{CheckError, CheckOptions, Severity, check, format};

const PMC_UNUSED_LABEL: &str = "namespace api {\nhelper() {\n5: right;\n}\n}\nmain() { @api::helper(); }\n";
const TMC_UNUSED_ALPHABET: &str = "alphabet ab { '_', 'a', 'b' }\nalphabet spare { '_', 'x' }\n\nmachine {\n  tape main: ab;\n\n  entry state scan {\n    ['b'] -> write ['a'] move [>] goto scan;\n    ['a'] ->             move [>] goto scan;\n    ['_'] -> stop;\n  }\n}\n";

fn opts() -> CheckOptions {
    CheckOptions { allow: vec![], warn: vec![] }
}

#[test]
fn pmc_lint_finding_crosses_with_utf16_span() {
    let diags = check(Lang::Pmc, PMC_UNUSED_LABEL, &opts()).unwrap();
    let d = diags.iter().find(|d| d.code == "unused-label").expect("the finding");
    assert_eq!(d.severity, Severity::Warning);
    // "namespace api {\nhelper() {\n" is 27 units; "5" sits at offset 27.
    assert_eq!(d.from, 27, "span starts at the label");
    assert!(d.to > d.from, "half-open, non-empty");
    assert!(d.message.contains("5"), "names the label: {}", d.message);
}

#[test]
fn tmc_lint_finding_crosses() {
    let diags = check(Lang::Tmc, TMC_UNUSED_ALPHABET, &opts()).unwrap();
    let d = diags.iter().find(|d| d.code == "unused-alphabet").expect("the finding");
    assert_eq!(d.severity, Severity::Warning);
    let line2 = "alphabet ab { '_', 'a', 'b' }\n".encode_utf16().count() as u32;
    assert!(d.from >= line2 && d.from < line2 + 40, "on the second line: {}", d.from);
    let fix = d.fix.as_ref().expect("unused-alphabet carries a deletion fix");
    assert!(fix.machine_applicable);
    assert_eq!(fix.edits.len(), 1);
    assert_eq!(fix.edits[0].replacement, "", "a deletion");
}

#[test]
fn allow_suppresses_and_unknown_allow_is_a_caller_error() {
    let allowed = CheckOptions { allow: vec!["unused-alphabet".into()], warn: vec![] };
    let diags = check(Lang::Tmc, TMC_UNUSED_ALPHABET, &allowed).unwrap();
    assert!(diags.iter().all(|d| d.code != "unused-alphabet"));
    let bogus = CheckOptions { allow: vec!["no-such-rule".into()], warn: vec![] };
    assert!(matches!(
        check(Lang::Tmc, TMC_UNUSED_ALPHABET, &bogus),
        Err(CheckError::UnknownAllowCode(c)) if c == "no-such-rule"
    ));
}

#[test]
fn compile_fatal_is_one_error_diagnostic() {
    let diags = check(Lang::Pmc, "main() { this is not pmc", &opts()).unwrap();
    assert_eq!(diags.len(), 1, "exactly the fatal: {diags:?}");
    assert_eq!(diags[0].severity, Severity::Error);
    assert!(!diags[0].code.is_empty(), "carries the compiler's error code");
    let diags = check(Lang::Tmc, "machine {", &opts()).unwrap();
    assert_eq!(diags.len(), 1);
    assert_eq!(diags[0].severity, Severity::Error);
}

#[test]
fn format_is_idempotent_and_reports_a_fatal_as_a_diagnostic() {
    for (lang, src) in [(Lang::Pmc, PMC_UNUSED_LABEL), (Lang::Tmc, TMC_UNUSED_ALPHABET)] {
        let once = format(lang, src).unwrap();
        let twice = format(lang, &once).unwrap();
        assert_eq!(once, twice, "{lang:?} fmt is idempotent");
        let tokens = |s: &str| s.split_whitespace().collect::<String>();
        assert_eq!(tokens(&once), tokens(src), "{lang:?} fmt is whitespace-only");
    }
    let err = format(Lang::Pmc, "main( {").unwrap_err();
    assert_eq!(err.severity, Severity::Error);
}
```

- [ ] **Step 2: Run to see them fail**

Run: `cargo test -p mtc-wasm --test diagnostics`
Expected: FAIL — unresolved import.

- [ ] **Step 3: Implement**

`crates/wasm/src/inner/diagnostics.rs`:

```rust
//! Findings and fatals in the shape the editor consumes, and the two
//! source-text services that produce them without building: `check`
//! (the lint channel) and `format`.

use mtc_core::diagnostics::{Applicability, Diagnostic};

use super::Lang;
use super::positions::Utf16Index;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edit {
    pub from: u32,
    pub to: u32,
    pub replacement: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fix {
    pub description: String,
    pub machine_applicable: bool,
    pub edits: Vec<Edit>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diag {
    pub code: String,
    pub severity: Severity,
    pub from: u32,
    pub to: u32,
    pub message: String,
    pub fix: Option<Fix>,
}

#[derive(Debug, Clone, Default)]
pub struct CheckOptions {
    pub allow: Vec<String>,
    /// TM-1's opt-in warn tier (`state-may-trap`, `index-identity-map`);
    /// ignored for `.pmc`, which has no such tier.
    pub warn: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckError {
    /// A rule name in `allow`/`warn` the lint layer does not know — a
    /// caller bug, thrown rather than reported as a finding.
    UnknownAllowCode(String),
}

pub fn from_core(idx: &Utf16Index, d: &Diagnostic, severity: Severity) -> Diag {
    let (from, to) = idx.span(&d.span);
    Diag {
        code: d.code.to_string(),
        severity,
        from,
        to,
        message: d.message.clone(),
        fix: d.fix.as_ref().map(|f| Fix {
            description: f.description.clone(),
            machine_applicable: matches!(f.applicability, Applicability::MachineApplicable),
            edits: f
                .edits
                .iter()
                .map(|e| {
                    let (from, to) = idx.span(&e.span);
                    Edit { from, to, replacement: e.replacement.clone() }
                })
                .collect(),
        }),
    }
}

pub fn pm_fatal(idx: &Utf16Index, e: &mtc_post_machine::compiler::CompileError) -> Diag {
    let (from, to) = idx.span(&e.span);
    Diag {
        code: e.kind.code().to_string(),
        severity: Severity::Error,
        from,
        to,
        message: e.to_string(),
        fix: None,
    }
}

pub fn tm_fatal(idx: &Utf16Index, e: &mtc_turing_machine::compiler::CompileError) -> Diag {
    let (from, to) = idx.span(&e.span);
    Diag {
        code: e.kind.code().to_string(),
        severity: Severity::Error,
        from,
        to,
        message: e.to_string(),
        fix: None,
    }
}

/// The lint channel: findings as warnings, a compile fatal as one error.
pub fn check(lang: Lang, source: &str, opts: &CheckOptions) -> Result<Vec<Diag>, CheckError> {
    let idx = Utf16Index::new(source);
    match lang {
        Lang::Pmc => {
            use mtc_post_machine::lint::{LintError, LintOptions, lint};
            let options = LintOptions { allow: opts.allow.clone(), ..Default::default() };
            match lint(source, options) {
                Ok(report) => Ok(report
                    .diagnostics
                    .iter()
                    .map(|d| from_core(&idx, d, Severity::Warning))
                    .collect()),
                Err(LintError::Compile(e)) => Ok(vec![pm_fatal(&idx, &e)]),
                Err(LintError::UnknownAllowCode(c)) => Err(CheckError::UnknownAllowCode(c)),
            }
        }
        Lang::Tmc => {
            use mtc_turing_machine::lint::{LintError, LintOptions, lint};
            let options = LintOptions {
                allow: opts.allow.clone(),
                warn: opts.warn.clone(),
                ..Default::default()
            };
            match lint(source, options) {
                Ok(report) => Ok(report
                    .diagnostics
                    .iter()
                    .map(|d| from_core(&idx, d, Severity::Warning))
                    .collect()),
                Err(LintError::Compile(e)) => Ok(vec![tm_fatal(&idx, &e)]),
                Err(LintError::UnknownAllowCode(c)) => Err(CheckError::UnknownAllowCode(c)),
            }
        }
    }
}

/// Canonical, whitespace-only formatting of the whole text; the fatal a
/// broken source raises comes back as the diagnostic it would be in `check`.
pub fn format(lang: Lang, source: &str) -> Result<String, Diag> {
    let idx = Utf16Index::new(source);
    match lang {
        Lang::Pmc => mtc_post_machine::fmt::format(source).map_err(|e| pm_fatal(&idx, &e)),
        Lang::Tmc => mtc_turing_machine::fmt::format(source).map_err(|e| tm_fatal(&idx, &e)),
    }
}
```

If either `LintOptions` has no `Default` derive, replace `..Default::default()` with the explicit remaining fields (PM has only `allow`; TM has `allow` and `warn`). If `LintError` has variants beyond `Compile` and `UnknownAllowCode`, add a match arm rendering them as one `Severity::Error` diagnostic with code `"lint"` at `(0, 0)` and the error's `Display` text.

Add `pub mod diagnostics;` to `inner/mod.rs`.

- [ ] **Step 4: Run to see them pass**

Run: `cargo test -p mtc-wasm --test diagnostics`
Expected: PASS, 5 tests. If `pmc_lint_finding_crosses_with_utf16_span` fails only on the exact `27`, print `d.from` and check whether the rule spans the whole statement (`5: right;`) rather than the label — the label starts at 27 either way; adjust the assertion to `d.from == 27` only if the printed value confirms it, otherwise pin the printed value with a comment explaining what the span covers.

- [ ] **Step 5: Gates, then drafted commit**

```
feat(wasm): the lint channel and the formatter behind the JS boundary

check() returns lint findings as warnings and a compile fatal as one
error, in UTF-16 offsets with the library's own fixes attached; format()
returns the canonical text or that same fatal. Compile warnings stay on
the build channel, the split the CLI already keeps.
```

---

### Task 4: `build` and `Program`

**Files:**
- Create: `crates/wasm/src/inner/program.rs`
- Modify: `crates/wasm/src/inner/mod.rs` (add `pub mod program;`)
- Test: `crates/wasm/tests/program.rs`

**Interfaces:**
- Consumes: Task 3's `Diag`, `from_core`, `pm_fatal`, `tm_fatal`, `Severity`; `mtc_post_machine::compiler::{compile, CompileOptions}` and `optimizer::OptLevel`; the same names in `mtc_turing_machine` plus `compiler::machine_tape_layout`; `mtc_post_machine::asm::{link, disassemble_executable_with_map}` and the TM twins; `mtc_post_machine::stdlib::object()` and the TM twin; `mtc_post_machine::arch::DEFAULT_GLYPHS`; `mtc_core::linker::{LinkOptions, MapFile}`; `mtc_core::linemap::LineIndex`; `mtc_core::formats::executable::Executable`.
- Produces:
  ```rust
  pub struct TapeLayout { pub name: String, pub glyphs: Vec<String> }
  pub struct SourceLoc { pub function: String, pub line: Option<u32> }
  pub struct Program { pub lang: Lang, pub exe: Executable, pub map: MapFile, /* private */ }
  pub fn build(lang: Lang, source: &str, opt_level: u8) -> Result<(Program, Vec<Diag>), Diag>
  impl Program {
      pub fn tapes(&self) -> &[TapeLayout];
      pub fn line_of(&self, addr: u32) -> Option<SourceLoc>;
      pub fn address_for_line(&self, line: u32) -> Option<u32>;
      pub fn disassembly(&self) -> String;
      pub fn bytes(&self) -> Vec<u8>;
      pub fn map_json(&self) -> String;
  }
  ```

- [ ] **Step 1: Write the failing tests**

`crates/wasm/tests/program.rs`:

```rust
//! build() is the compile channel plus the link: warnings ride along, a
//! fatal is the one error. The Program carries the map, so the line
//! table and tape layouts the browser needs are queries, not re-parses.

use mtc_wasm::inner::Lang;
use mtc_wasm::inner::diagnostics::Severity;
use mtc_wasm::inner::program::build;

const PMC_INC: &str = "main() {\n    1: right(2);\n    2: check(1, 3);\n    3: mark(4);\n    4: left(5);\n    5: check(4, 6);\n    6: right(!);\n}\n";
const TMC_REPLACE_B: &str = "alphabet ab { '_', 'a', 'b' }\n\nmachine {\n  tape main: ab;\n\n  entry state scan {\n    ['b'] -> write ['a'] move [>] goto scan;\n    ['a'] ->             move [>] goto scan;\n    ['_'] -> stop;\n  }\n}\n";

#[test]
fn pmc_builds_with_a_single_binary_band() {
    let (program, warnings) = build(Lang::Pmc, PMC_INC, 1).unwrap();
    assert!(warnings.iter().all(|d| d.severity == Severity::Warning));
    assert_eq!(program.exe.arch, 0x01);
    let tapes = program.tapes();
    assert_eq!(tapes.len(), 1);
    assert_eq!(tapes[0].glyphs, vec![" ".to_string(), "*".to_string()], "the CLI's PM-1 glyphs");
    assert!(!program.bytes().is_empty());
    assert!(program.map_json().contains("\"functions\""));
    assert!(program.disassembly().contains("main"), "reassembleable text names main");
}

#[test]
fn tmc_builds_with_named_glyph_bands() {
    let (program, _) = build(Lang::Tmc, TMC_REPLACE_B, 1).unwrap();
    assert_eq!(program.exe.arch, 0x02);
    assert_eq!(program.exe.tape_count, 1);
    let tapes = program.tapes();
    assert_eq!(tapes.len(), 1);
    assert_eq!(tapes[0].name, "main");
    assert_eq!(tapes[0].glyphs, vec!["_", "a", "b"]);
}

#[test]
fn line_table_round_trips_through_the_map() {
    for (lang, src, some_line) in [(Lang::Pmc, PMC_INC, 3u32), (Lang::Tmc, TMC_REPLACE_B, 8u32)] {
        let (program, _) = build(lang, src, 0).unwrap();
        let addr = program
            .address_for_line(some_line)
            .unwrap_or_else(|| panic!("{lang:?}: line {some_line} has an address under -g"));
        let loc = program.line_of(addr).expect("resolves");
        assert_eq!(loc.line, Some(some_line), "{lang:?}");
        assert!(!loc.function.is_empty());
        assert!(program.line_of(0xFFFF_FFF0).is_none(), "outside the image");
    }
}

#[test]
fn a_fatal_is_one_error_and_no_program() {
    let err = build(Lang::Pmc, "main() { nope", 1).unwrap_err();
    assert_eq!(err.severity, Severity::Error);
    let err = build(Lang::Tmc, "alphabet a { '_' }\nmachine {", 1).unwrap_err();
    assert_eq!(err.severity, Severity::Error);
}

#[test]
fn opt_levels_both_build_and_o0_is_not_smaller() {
    let (o0, _) = build(Lang::Tmc, TMC_REPLACE_B, 0).unwrap();
    let (o1, _) = build(Lang::Tmc, TMC_REPLACE_B, 1).unwrap();
    assert!(o0.exe.code.len() >= o1.exe.code.len());
}
```

- [ ] **Step 2: Run to see them fail**

Run: `cargo test -p mtc-wasm --test program`
Expected: FAIL — unresolved import.

- [ ] **Step 3: Implement**

`crates/wasm/src/inner/program.rs`:

```rust
//! The build channel (compile → link against the embedded stdlib) and the
//! Program it yields: the executable, its map, the line index over the
//! map, and the per-band tape layouts the renderer needs.

use mtc_core::formats::executable::Executable;
use mtc_core::linemap::LineIndex;
use mtc_core::linker::{LinkOptions, MapFile};

use super::Lang;
use super::diagnostics::{Diag, Severity, from_core, pm_fatal, tm_fatal};
use super::positions::Utf16Index;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TapeLayout {
    pub name: String,
    pub glyphs: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceLoc {
    pub function: String,
    pub line: Option<u32>,
}

pub struct Program {
    pub lang: Lang,
    pub exe: Executable,
    pub map: MapFile,
    line_index: LineIndex,
    layouts: Vec<TapeLayout>,
}

/// A link failure has no source span; it is reported at the text start.
fn link_diag(message: String) -> Diag {
    Diag { code: "link".to_string(), severity: Severity::Error, from: 0, to: 0, message, fix: None }
}

/// Compile with debug info (the browser always wants the line table),
/// link against the embedded stdlib, and gather the compile channel's
/// warnings. `opt_level` is 0 or 1; anything else is treated as 1.
pub fn build(lang: Lang, source: &str, opt_level: u8) -> Result<(Program, Vec<Diag>), Diag> {
    let idx = Utf16Index::new(source);
    match lang {
        Lang::Pmc => {
            use mtc_post_machine::compiler::{CompileOptions, compile};
            use mtc_post_machine::optimizer::OptLevel;
            let options = CompileOptions {
                opt_level: if opt_level == 0 { OptLevel::O0 } else { OptLevel::O1 },
                debug_info: true,
                ..Default::default()
            };
            let out = compile(source, options).map_err(|e| pm_fatal(&idx, &e))?;
            let warnings = out
                .report
                .diagnostics
                .iter()
                .map(|d| from_core(&idx, d, Severity::Warning))
                .collect();
            let linked = mtc_post_machine::asm::link(
                &[out.object],
                &[mtc_post_machine::stdlib::object().clone()],
                LinkOptions::default(),
            )
            .map_err(|e| link_diag(e.to_string()))?;
            let layouts = vec![TapeLayout {
                name: "tape".to_string(),
                glyphs: mtc_post_machine::arch::DEFAULT_GLYPHS.iter().map(|g| g.to_string()).collect(),
            }];
            Ok((Program::new(lang, linked.executable, linked.map, layouts), warnings))
        }
        Lang::Tmc => {
            use mtc_turing_machine::compiler::{CompileOptions, compile, machine_tape_layout};
            use mtc_turing_machine::optimizer::OptLevel;
            let options = CompileOptions {
                opt_level: if opt_level == 0 { OptLevel::O0 } else { OptLevel::O1 },
                debug_info: true,
                ..Default::default()
            };
            let out = compile(source, options).map_err(|e| tm_fatal(&idx, &e))?;
            let warnings = out
                .report
                .diagnostics
                .iter()
                .map(|d| from_core(&idx, d, Severity::Warning))
                .collect();
            let linked = mtc_turing_machine::asm::link(
                &[out.object],
                &[mtc_turing_machine::stdlib::object().clone()],
                LinkOptions::default(),
            )
            .map_err(|e| link_diag(e.to_string()))?;
            let layouts = machine_tape_layout(source)
                .map_err(|e| tm_fatal(&idx, &e))?
                .unwrap_or_default()
                .into_iter()
                .map(|t| TapeLayout { name: t.name, glyphs: t.glyphs })
                .collect();
            Ok((Program::new(lang, linked.executable, linked.map, layouts), warnings))
        }
    }
}

impl Program {
    fn new(lang: Lang, exe: Executable, map: MapFile, layouts: Vec<TapeLayout>) -> Program {
        let line_index = LineIndex::new(&map);
        Program { lang, exe, map, line_index, layouts }
    }

    pub fn tapes(&self) -> &[TapeLayout] {
        &self.layouts
    }

    pub fn line_of(&self, addr: u32) -> Option<SourceLoc> {
        self.line_index.resolve(addr).map(|loc| SourceLoc {
            function: loc.function.to_string(),
            line: loc.line,
        })
    }

    /// Where a breakpoint on `line` lands. A single source in the browser,
    /// so the per-file filter is not used.
    pub fn address_for_line(&self, line: u32) -> Option<u32> {
        self.line_index.address_for_line(line, None)
    }

    /// The reassembleable `.pma`/`.tma` text, names from the map.
    pub fn disassembly(&self) -> String {
        match self.lang {
            Lang::Pmc => mtc_post_machine::asm::disassemble_executable_with_map(&self.exe, &self.map),
            Lang::Tmc => mtc_turing_machine::asm::disassemble_executable_with_map(&self.exe, &self.map),
        }
    }

    /// The MX image, as `pmt build -o` would write it.
    pub fn bytes(&self) -> Vec<u8> {
        self.exe.to_bytes()
    }

    /// The `.map` sidecar text.
    pub fn map_json(&self) -> String {
        self.map.to_json()
    }
}
```

Add `pub mod program;` to `inner/mod.rs`. If `machine_tape_layout`'s item type is not named `TapeLayout` with `name`/`glyphs`, adapt the field names at that one `map`. If `LinkError` lacks `Display`, use `format!("{e:?}")`.

- [ ] **Step 4: Run to see them pass**

Run: `cargo test -p mtc-wasm --test program`
Expected: PASS, 5 tests. If `line_table_round_trips_through_the_map` fails on line 3 or 8 having no address, run `cargo run -p mtc-turing-machine --bin tmt -- build -g` on the fixture and read the sidecar's `lines` to pick a line that is mapped; update the constant and say which line in a comment.

- [ ] **Step 5: Gates, then drafted commit**

```
feat(wasm): build() — compile with the line table, link the stdlib, keep the map

The Program owns the executable, the map, a LineIndex over it, and the
per-band glyph layouts (the CLI's PM-1 blank/mark pair; the .tmc
machine block's own alphabets). Warnings ride along; a fatal or a link
failure is the one error.
```

---

### Task 5: Structured listing rows

**Files:**
- Create: `crates/wasm/src/inner/listing.rs`
- Modify: `crates/wasm/src/inner/mod.rs` (add `pub mod listing;`)
- Test: `crates/wasm/tests/listing.rs`

**Interfaces:**
- Consumes: Task 4's `Program`; `mtc_core::asm::disassembler::listing_parts(syntax: &ArchSyntax, code: &[u8], addr: u32, resolve: &dyn Fn(u32) -> Option<String>) -> ListingParts { len, bytes_hex, mnemonic, operand }`; `mtc_post_machine::asm::pm1_syntax()`, `mtc_turing_machine::asm::tm1_syntax()`; `MapFile.functions: Vec<MapFunction { name, start, end, labels: Vec<(String, u32)>, .. }>`; `mtc_post_machine::asm::listing_executable(exe, Some(&map)) -> String` and the TM twin (for the agreement test).
- Produces:
  ```rust
  pub struct Row { pub addr: u32, pub bytes: String, pub mnemonic: String, pub operand: String, pub function: Option<String>, pub label: Option<String> }
  pub fn rows(program: &Program) -> Vec<Row>
  ```

- [ ] **Step 1: Write the failing tests**

`crates/wasm/tests/listing.rs`:

```rust
//! The structured listing is the ip view's data: one row per instruction,
//! covering the image exactly once, agreeing with the text listing.

use mtc_wasm::inner::Lang;
use mtc_wasm::inner::listing::rows;
use mtc_wasm::inner::program::build;

const PMC_INC: &str = "main() {\n    1: right(2);\n    2: check(1, 3);\n    3: mark(4);\n    4: left(5);\n    5: check(4, 6);\n    6: right(!);\n}\n";
const TMC_REPLACE_B: &str = "alphabet ab { '_', 'a', 'b' }\n\nmachine {\n  tape main: ab;\n\n  entry state scan {\n    ['b'] -> write ['a'] move [>] goto scan;\n    ['a'] ->             move [>] goto scan;\n    ['_'] -> stop;\n  }\n}\n";

#[test]
fn rows_tile_the_code_image_exactly() {
    for (lang, src) in [(Lang::Pmc, PMC_INC), (Lang::Tmc, TMC_REPLACE_B)] {
        let (program, _) = build(lang, src, 1).unwrap();
        let rows = rows(&program);
        assert!(!rows.is_empty(), "{lang:?}");
        assert_eq!(rows[0].addr, 0);
        let mut expected_next = 0u32;
        for row in &rows {
            assert_eq!(row.addr, expected_next, "{lang:?}: rows are contiguous");
            assert!(!row.mnemonic.is_empty());
            assert!(!row.bytes.is_empty());
            expected_next = row.addr + row.bytes.split_whitespace().count() as u32;
        }
        assert_eq!(expected_next as usize, program.exe.code.len(), "{lang:?}: ends at the image end");
    }
}

#[test]
fn function_starts_are_labelled_with_their_names() {
    let (program, _) = build(Lang::Pmc, PMC_INC, 1).unwrap();
    let rows = rows(&program);
    let main_start = program.map.functions.iter().find(|f| f.name == "main").unwrap().start;
    let row = rows.iter().find(|r| r.addr == main_start).expect("a row starts main");
    assert_eq!(row.function.as_deref(), Some("main"));
    assert!(rows.iter().all(|r| r.function.is_some()), "every row knows its function");
}

#[test]
fn every_mnemonic_appears_in_the_text_listing() {
    let (program, _) = build(Lang::Tmc, TMC_REPLACE_B, 1).unwrap();
    let text = mtc_turing_machine::asm::listing_executable(&program.exe, Some(&program.map));
    for row in rows(&program) {
        assert!(text.contains(&row.mnemonic), "{} missing from the text listing", row.mnemonic);
    }
}
```

- [ ] **Step 2: Run to see them fail**

Run: `cargo test -p mtc-wasm --test listing`
Expected: FAIL — unresolved import.

- [ ] **Step 3: Implement**

`crates/wasm/src/inner/listing.rs`:

```rust
//! The debugger code view as data: one row per instruction, function and
//! label names from the map, jump targets resolved to `function` or
//! `function.label` the way the text listing does.

use mtc_core::asm::disassembler::listing_parts;
use mtc_core::linker::MapFile;

use super::Lang;
use super::program::Program;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub addr: u32,
    /// Space-separated hex bytes, one pair per byte.
    pub bytes: String,
    pub mnemonic: String,
    pub operand: String,
    /// The function whose range contains `addr`.
    pub function: Option<String>,
    /// A label sitting exactly at `addr` (a function start is its name).
    pub label: Option<String>,
}

fn function_at(map: &MapFile, addr: u32) -> Option<&str> {
    map.functions
        .iter()
        .find(|f| f.start <= addr && addr < f.end)
        .map(|f| f.name.as_str())
}

fn label_at(map: &MapFile, addr: u32) -> Option<String> {
    for f in &map.functions {
        if f.start == addr {
            return Some(f.name.clone());
        }
        if let Some((label, _)) = f.labels.iter().find(|(_, a)| *a == addr) {
            return Some(format!("{}.{label}", f.name));
        }
    }
    None
}

pub fn rows(program: &Program) -> Vec<Row> {
    let syntax = match program.lang {
        Lang::Pmc => mtc_post_machine::asm::pm1_syntax(),
        Lang::Tmc => mtc_turing_machine::asm::tm1_syntax(),
    };
    let map = &program.map;
    let code = &program.exe.code;
    let resolve = |target: u32| label_at(map, target);
    let mut out = Vec::new();
    let mut addr = 0u32;
    while (addr as usize) < code.len() {
        let parts = listing_parts(&syntax, code, addr, &resolve);
        // A decoder that cannot advance (an undecodable trailing byte) still
        // gets a row, and the walk moves by one so it always terminates.
        let len = parts.len.max(1);
        out.push(Row {
            addr,
            bytes: parts.bytes_hex,
            mnemonic: parts.mnemonic,
            operand: parts.operand,
            function: function_at(map, addr).map(str::to_string),
            label: label_at(map, addr),
        });
        addr += len;
    }
    out
}
```

Add `pub mod listing;` to `inner/mod.rs`. The test derives an instruction's length from `bytes` by counting hex pairs; if `bytes_hex` turns out not to be space-separated pairs, change the test to carry `len` on the `Row` instead (add `pub len: u32` from `parts.len`) and compare against that.

- [ ] **Step 4: Run to see them pass**

Run: `cargo test -p mtc-wasm --test listing`
Expected: PASS, 3 tests.

- [ ] **Step 5: Gates, then drafted commit**

```
feat(wasm): the debugger listing as rows

One row per instruction over listing_parts, tiling the image exactly,
with the containing function and any label at the address from the map.
```

---

### Task 6: `Session` over `AsyncSession`

**Files:**
- Create: `crates/wasm/src/inner/session.rs`
- Modify: `crates/wasm/src/inner/mod.rs` (add `pub mod session;`)
- Test: `crates/wasm/tests/session.rs`

**Interfaces:**
- Consumes: Task 1's `registry_for`; Task 4's `Program`, `TapeLayout`; `mtc_core::vm::{Machine, AsyncSession, PumpEvent, PauseCause, RunOptions, RunLimits, RunResult, RunStats, Outcome, Trap, WideTape, SyncAsAsync, AsyncTapeDevice}`; `mtc_core::formats::tapeblock::TapeSnapshot`. `AsyncSession::pump(&mut self, devices: &mut [&mut dyn AsyncTapeDevice], budget: Option<u64>) -> PumpEvent`; `Machine::async_session(&self, RunOptions)` / `async_session_tapes`; `WideTape::new(width)`, `WideTape::from_snapshot(&TapeSnapshot, width) -> Result<_, DeviceFault>`, `WideTape::to_snapshot()`; `SyncAsAsync::new(t)`, `get_ref()`.
- Produces:
  ```rust
  pub struct Seed { pub cells: Vec<u8>, pub head: i64, pub origin: i64 }
  pub struct Limits { pub max_steps: Option<u64>, pub max_tacts: Option<u64> }
  pub struct TrapInfo { pub kind: &'static str, pub at: Option<u32>, pub detail: String }
  pub enum OutcomeInfo { Stopped, Halted, Trapped(TrapInfo) }
  pub struct Stats { pub steps: u64, pub core_tacts: u64, pub stall_tacts: u64, pub total_tacts: u64 }
  pub struct Finished { pub outcome: OutcomeInfo, pub stats: Stats, pub ip: u32, pub stack: Vec<u32> }
  pub enum Cause { Step, Brk, Manual, Breakpoint(u32), Trap(TrapInfo) }
  pub enum Event { DeviceWait, BudgetSpent, Paused(Cause), Finished(Finished) }
  pub struct Snapshot { pub band: u32, pub name: String, pub glyphs: Vec<String>, pub origin: i64, pub cells: Vec<u8>, pub head: i64 }
  pub enum SessionError { Stopped, TooManySeeds { given: usize, bands: usize }, BadSeed { band: u32, index: u8, width: u32 }, NoSuchBand(u32), Load(String) }
  pub struct Session { /* private */ }
  impl Session {
      pub fn new(program: &Program, seeds: &[Seed], limits: Limits) -> Result<Session, SessionError>;
      pub fn pump(&mut self, budget: Option<u64>) -> Result<Event, SessionError>;
      pub fn pause(&mut self) -> Result<(), SessionError>;
      pub fn add_breakpoint(&mut self, addr: u32) -> Result<(), SessionError>;
      pub fn remove_breakpoint(&mut self, addr: u32) -> Result<(), SessionError>;
      pub fn snapshot(&self, band: u32) -> Result<Snapshot, SessionError>;
      pub fn snapshots(&self) -> Result<Vec<Snapshot>, SessionError>;
      pub fn ip(&self) -> Result<u32, SessionError>;  pub fn mf(&self) -> Result<bool, SessionError>;
      pub fn fr(&self) -> Result<u32, SessionError>;  pub fn depth(&self) -> Result<usize, SessionError>;
      pub fn stack(&self) -> Result<Vec<u32>, SessionError>;  pub fn stats(&self) -> Result<Stats, SessionError>;
      pub fn finished(&self) -> Result<Option<Finished>, SessionError>;
      pub fn stop(&mut self) -> Result<Stats, SessionError>;   // consumes the inner session; every later call is Err(Stopped)
      pub fn bands(&self) -> usize;
  }
  pub fn trap_kind(t: &Trap) -> &'static str
  ```

- [ ] **Step 1: Write the failing tests**

`crates/wasm/tests/session.rs`:

```rust
//! The Session owns the tapes; JS pumps it. These pin the pump events, the
//! final tapes against the goldens' derivations, seed validation, and the
//! after-stop contract.

use mtc_wasm::inner::Lang;
use mtc_wasm::inner::program::build;
use mtc_wasm::inner::session::{Cause, Event, Limits, OutcomeInfo, Seed, Session, SessionError};

const PMC_INC: &str = "main() {\n    1: right(2);\n    2: check(1, 3);\n    3: mark(4);\n    4: left(5);\n    5: check(4, 6);\n    6: right(!);\n}\n";
const TMC_REPLACE_B: &str = "alphabet ab { '_', 'a', 'b' }\n\nmachine {\n  tape main: ab;\n\n  entry state scan {\n    ['b'] -> write ['a'] move [>] goto scan;\n    ['a'] ->             move [>] goto scan;\n    ['_'] -> stop;\n  }\n}\n";

fn no_limits() -> Limits {
    Limits { max_steps: None, max_tacts: None }
}

fn seed(cells: &[u8]) -> Seed {
    Seed { cells: cells.to_vec(), head: 0, origin: 0 }
}

fn run_to_end(s: &mut Session) -> mtc_wasm::inner::session::Finished {
    loop {
        match s.pump(None).unwrap() {
            Event::Finished(f) => return f,
            Event::Paused(c) => panic!("unexpected pause {c:?}"),
            Event::BudgetSpent => panic!("no budget was given"),
            Event::DeviceWait => panic!("owned devices are always ready"),
        }
    }
}

#[test]
fn pmc_increment_runs_to_stopped_with_the_golden_tape() {
    let (program, _) = build(Lang::Pmc, PMC_INC, 1).unwrap();
    let mut s = Session::new(&program, &[seed(&[1, 1, 1])], no_limits()).unwrap();
    let fin = run_to_end(&mut s);
    assert!(matches!(fin.outcome, OutcomeInfo::Stopped));
    let snap = s.snapshot(0).unwrap();
    assert_eq!(snap.head, 0);
    assert_eq!(&snap.cells[..4], &[1, 1, 1, 1], "two becomes three; head back on the first mark");
    assert!(snap.cells[4..].iter().all(|&c| c == 0));
    assert_eq!(snap.glyphs, vec![" ", "*"]);
    assert_eq!(snap.name, "tape");
    let stats = s.stop().unwrap();
    assert_eq!(stats.steps, fin.stats.steps);
    assert!(matches!(s.pump(None), Err(SessionError::Stopped)));
    assert!(matches!(s.snapshot(0), Err(SessionError::Stopped)));
}

#[test]
fn tmc_replace_b_runs_to_stopped_with_the_expected_tape() {
    let (program, _) = build(Lang::Tmc, TMC_REPLACE_B, 1).unwrap();
    let mut s = Session::new(&program, &[seed(&[1, 2, 2])], no_limits()).unwrap();
    let fin = run_to_end(&mut s);
    assert!(matches!(fin.outcome, OutcomeInfo::Stopped));
    let snap = s.snapshot(0).unwrap();
    assert_eq!(snap.head, 3, "stopped on the first blank");
    assert_eq!(&snap.cells[..3], &[1, 1, 1], "every b became a");
    assert_eq!(snap.glyphs, vec!["_", "a", "b"]);
    assert_eq!(snap.name, "main");
}

#[test]
fn budget_pauses_without_losing_progress() {
    let (program, _) = build(Lang::Tmc, TMC_REPLACE_B, 0).unwrap();
    let mut s = Session::new(&program, &[seed(&[2, 2, 2, 2, 2, 2])], no_limits()).unwrap();
    let mut spent = 0;
    let fin = loop {
        match s.pump(Some(1)).unwrap() {
            Event::BudgetSpent => spent += 1,
            Event::Finished(f) => break f,
            other => panic!("{other:?}"),
        }
    };
    assert!(spent >= 6, "at least one instruction per cell: {spent}");
    assert_eq!(fin.stats.steps, s.stats().unwrap().steps);
    assert_eq!(&s.snapshot(0).unwrap().cells[..6], &[1, 1, 1, 1, 1, 1]);
}

#[test]
fn manual_pause_and_breakpoint_report_their_causes() {
    let (program, _) = build(Lang::Tmc, TMC_REPLACE_B, 0).unwrap();
    let mut s = Session::new(&program, &[seed(&[2, 2, 2])], no_limits()).unwrap();
    s.pause().unwrap();
    assert!(matches!(s.pump(None).unwrap(), Event::Paused(Cause::Manual)));
    // A breakpoint at the current ip is not re-hit on resume; plant one at
    // the next instruction instead.
    let ip = s.ip().unwrap();
    let rows = mtc_wasm::inner::listing::rows(&program);
    let next = rows.iter().find(|r| r.addr > ip).expect("a later instruction").addr;
    s.add_breakpoint(next).unwrap();
    match s.pump(None).unwrap() {
        Event::Paused(Cause::Breakpoint(at)) => assert_eq!(at, next),
        other => panic!("{other:?}"),
    }
    s.remove_breakpoint(next).unwrap();
    assert!(matches!(run_to_end(&mut s).outcome, OutcomeInfo::Stopped));
}

#[test]
fn step_limit_is_a_trap_with_its_kind() {
    let (program, _) = build(Lang::Tmc, TMC_REPLACE_B, 0).unwrap();
    let limits = Limits { max_steps: Some(2), max_tacts: None };
    let mut s = Session::new(&program, &[seed(&[2, 2, 2, 2, 2, 2, 2, 2])], limits).unwrap();
    let fin = run_to_end(&mut s);
    match fin.outcome {
        OutcomeInfo::Trapped(t) => assert_eq!(t.kind, "step-limit"),
        other => panic!("{other:?}"),
    }
    assert!(s.finished().unwrap().is_some(), "the result is repeatable after finishing");
}

#[test]
fn seeds_are_validated_against_the_band() {
    let (program, _) = build(Lang::Tmc, TMC_REPLACE_B, 1).unwrap();
    assert!(matches!(
        Session::new(&program, &[seed(&[1, 7])], no_limits()),
        Err(SessionError::BadSeed { band: 0, index: 7, width: 3 })
    ));
    assert!(matches!(
        Session::new(&program, &[seed(&[1]), seed(&[1])], no_limits()),
        Err(SessionError::TooManySeeds { given: 2, bands: 1 })
    ));
    let s = Session::new(&program, &[], no_limits()).unwrap();
    assert_eq!(s.bands(), 1, "missing seeds are blank bands");
    assert!(matches!(s.snapshot(5), Err(SessionError::NoSuchBand(5))));
}
```

- [ ] **Step 2: Run to see them fail**

Run: `cargo test -p mtc-wasm --test session`
Expected: FAIL — unresolved import.

- [ ] **Step 3: Implement**

`crates/wasm/src/inner/session.rs`:

```rust
//! A pumped run over owned tapes. The embedder (the JS worker) drives it
//! by calling `pump`; the pause priority and budget semantics are core's
//! (`docs/core.md (AsyncSession)`) and are not restated here.

use mtc_core::formats::tapeblock::TapeSnapshot;
use mtc_core::vm::{
    AsyncSession, AsyncTapeDevice, Machine, Outcome, PauseCause, PumpEvent, RunLimits,
    RunOptions, RunResult, RunStats, SyncAsAsync, Trap, WideTape,
};

use super::Lang;
use super::program::{Program, TapeLayout};
use super::registry::registry_for;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Seed {
    pub cells: Vec<u8>,
    pub head: i64,
    pub origin: i64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Limits {
    pub max_steps: Option<u64>,
    pub max_tacts: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrapInfo {
    pub kind: &'static str,
    pub at: Option<u32>,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OutcomeInfo {
    Stopped,
    Halted,
    Trapped(TrapInfo),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stats {
    pub steps: u64,
    pub core_tacts: u64,
    pub stall_tacts: u64,
    pub total_tacts: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finished {
    pub outcome: OutcomeInfo,
    pub stats: Stats,
    pub ip: u32,
    pub stack: Vec<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Cause {
    Step,
    Brk,
    Manual,
    Breakpoint(u32),
    Trap(TrapInfo),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    DeviceWait,
    BudgetSpent,
    Paused(Cause),
    Finished(Finished),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    pub band: u32,
    pub name: String,
    pub glyphs: Vec<String>,
    pub origin: i64,
    pub cells: Vec<u8>,
    pub head: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionError {
    /// `stop()` already consumed the session.
    Stopped,
    TooManySeeds { given: usize, bands: usize },
    BadSeed { band: u32, index: u8, width: u32 },
    NoSuchBand(u32),
    Load(String),
}

/// The trap's kind, spelled as the equivalence harnesses spell it. Exhaustive
/// on purpose: a new `Trap` variant must be named here.
pub fn trap_kind(t: &Trap) -> &'static str {
    match t {
        Trap::InvalidOpcode { .. } => "invalid-opcode",
        Trap::CodeOutOfBounds { .. } => "code-out-of-bounds",
        Trap::BadOperand { .. } => "bad-operand",
        Trap::CallTargetNotEntry { .. } => "call-target-not-entry",
        Trap::StackOverflow => "stack-overflow",
        Trap::StackUnderflow => "stack-underflow",
        Trap::StepLimit => "step-limit",
        Trap::TactLimit => "tact-limit",
        Trap::Device { .. } => "device",
        Trap::NoTransition { .. } => "no-transition",
        Trap::TableOutOfBounds { .. } => "table-out-of-bounds",
        Trap::DispatchOutOfRange { .. } => "dispatch-out-of-range",
        Trap::UnmappedRead { .. } => "unmapped-read",
        Trap::UnmappedWrite { .. } => "unmapped-write",
        Trap::ExitOutOfRange { .. } => "exit-out-of-range",
        Trap::ProfileViolation { .. } => "profile-violation",
    }
}

fn trap_info(t: &Trap) -> TrapInfo {
    let at = match t {
        Trap::InvalidOpcode { at, .. }
        | Trap::CodeOutOfBounds { at }
        | Trap::BadOperand { at }
        | Trap::NoTransition { at }
        | Trap::TableOutOfBounds { at }
        | Trap::DispatchOutOfRange { at }
        | Trap::UnmappedRead { at }
        | Trap::UnmappedWrite { at }
        | Trap::ExitOutOfRange { at }
        | Trap::ProfileViolation { at } => Some(*at),
        Trap::CallTargetNotEntry { target } => Some(*target),
        Trap::StackOverflow
        | Trap::StackUnderflow
        | Trap::StepLimit
        | Trap::TactLimit
        | Trap::Device { .. } => None,
    };
    TrapInfo { kind: trap_kind(t), at, detail: t.to_string() }
}

fn stats(s: RunStats) -> Stats {
    Stats {
        steps: s.steps,
        core_tacts: s.core_tacts,
        stall_tacts: s.stall_tacts,
        total_tacts: s.total_tacts(),
    }
}

fn finished(r: &RunResult) -> Finished {
    Finished {
        outcome: match &r.outcome {
            Outcome::Stopped => OutcomeInfo::Stopped,
            Outcome::Halted => OutcomeInfo::Halted,
            Outcome::Trapped(t) => OutcomeInfo::Trapped(trap_info(t)),
        },
        stats: stats(r.stats),
        ip: r.ip,
        stack: r.stack.clone(),
    }
}

fn cause(c: PauseCause) -> Cause {
    match c {
        PauseCause::Step => Cause::Step,
        PauseCause::Brk => Cause::Brk,
        PauseCause::Manual => Cause::Manual,
        PauseCause::Breakpoint(a) => Cause::Breakpoint(a),
        PauseCause::Trap(t) => Cause::Trap(trap_info(&t)),
    }
}

/// The device slot. One variant today; a JS-implemented `AsyncTapeDevice`
/// on one band is a later variant, not a redesign.
enum Device {
    Owned(SyncAsAsync<WideTape>),
}

impl Device {
    fn as_async(&mut self) -> &mut dyn AsyncTapeDevice {
        match self {
            Device::Owned(d) => d,
        }
    }

    fn snapshot(&self) -> TapeSnapshot {
        match self {
            Device::Owned(d) => d.get_ref().to_snapshot(),
        }
    }
}

pub struct Session {
    inner: Option<AsyncSession<'static>>,
    devices: Vec<Device>,
    layouts: Vec<TapeLayout>,
}

impl Session {
    pub fn new(program: &Program, seeds: &[Seed], limits: Limits) -> Result<Session, SessionError> {
        let exe = &program.exe;
        let bands = exe.tape_count.max(1) as usize;
        if seeds.len() > bands {
            return Err(SessionError::TooManySeeds { given: seeds.len(), bands });
        }
        // PM-1 images carry no cardinalities: one binary band.
        let widths: Vec<u32> = if exe.alphabet_cardinalities.is_empty() {
            vec![2; bands]
        } else {
            exe.alphabet_cardinalities.clone()
        };
        let mut devices = Vec::with_capacity(bands);
        for band in 0..bands {
            let width = widths.get(band).copied().unwrap_or(2);
            let tape = match seeds.get(band) {
                None => WideTape::new(width),
                Some(seed) => {
                    if let Some(&bad) = seed.cells.iter().find(|&&c| c as u32 >= width) {
                        return Err(SessionError::BadSeed { band: band as u32, index: bad, width });
                    }
                    let snap = TapeSnapshot {
                        origin: seed.origin,
                        cells: seed.cells.clone(),
                        head: seed.head,
                        alphabet: None,
                    };
                    WideTape::from_snapshot(&snap, width)
                        .map_err(|e| SessionError::Load(format!("band {band}: {e:?}")))?
                }
            };
            devices.push(Device::Owned(SyncAsAsync::new(tape)));
        }
        let registry = registry_for(exe.tape_count.max(1));
        let machine = Machine::from_executable(exe, registry)
            .map_err(|e| SessionError::Load(format!("{e:?}")))?;
        let opts = RunOptions {
            limits: RunLimits { max_steps: limits.max_steps, max_tacts: limits.max_tacts },
            ..Default::default()
        };
        // PM-1 latches the initial mark through device 0 on the first pump;
        // TM-1 never latches and carries its table ROM.
        let inner = match program.lang {
            Lang::Pmc => machine.async_session(opts),
            Lang::Tmc => machine.async_session_tapes(opts),
        };
        let layouts = program.tapes().to_vec();
        Ok(Session { inner: Some(inner), devices, layouts })
    }

    fn live(&self) -> Result<&AsyncSession<'static>, SessionError> {
        self.inner.as_ref().ok_or(SessionError::Stopped)
    }

    fn live_mut(&mut self) -> Result<&mut AsyncSession<'static>, SessionError> {
        self.inner.as_mut().ok_or(SessionError::Stopped)
    }

    pub fn bands(&self) -> usize {
        self.devices.len()
    }

    pub fn pump(&mut self, budget: Option<u64>) -> Result<Event, SessionError> {
        let session = self.inner.as_mut().ok_or(SessionError::Stopped)?;
        let mut refs: Vec<&mut dyn AsyncTapeDevice> =
            self.devices.iter_mut().map(Device::as_async).collect();
        Ok(match session.pump(&mut refs, budget) {
            PumpEvent::DeviceWait => Event::DeviceWait,
            PumpEvent::BudgetSpent => Event::BudgetSpent,
            PumpEvent::Paused(c) => Event::Paused(cause(c)),
            PumpEvent::Finished(r) => Event::Finished(finished(&r)),
        })
    }

    pub fn pause(&mut self) -> Result<(), SessionError> {
        self.live_mut()?.pause();
        Ok(())
    }

    pub fn add_breakpoint(&mut self, addr: u32) -> Result<(), SessionError> {
        self.live_mut()?.add_breakpoint(addr);
        Ok(())
    }

    pub fn remove_breakpoint(&mut self, addr: u32) -> Result<(), SessionError> {
        self.live_mut()?.remove_breakpoint(addr);
        Ok(())
    }

    pub fn snapshot(&self, band: u32) -> Result<Snapshot, SessionError> {
        self.live()?;
        let device = self.devices.get(band as usize).ok_or(SessionError::NoSuchBand(band))?;
        let snap = device.snapshot();
        let layout = self.layouts.get(band as usize);
        Ok(Snapshot {
            band,
            name: layout.map(|l| l.name.clone()).unwrap_or_else(|| format!("tape{band}")),
            glyphs: layout.map(|l| l.glyphs.clone()).unwrap_or_default(),
            origin: snap.origin,
            cells: snap.cells,
            head: snap.head,
        })
    }

    pub fn snapshots(&self) -> Result<Vec<Snapshot>, SessionError> {
        (0..self.devices.len() as u32).map(|b| self.snapshot(b)).collect()
    }

    pub fn ip(&self) -> Result<u32, SessionError> {
        Ok(self.live()?.ip())
    }

    pub fn mf(&self) -> Result<bool, SessionError> {
        Ok(self.live()?.mf())
    }

    pub fn fr(&self) -> Result<u32, SessionError> {
        Ok(self.live()?.fr())
    }

    pub fn depth(&self) -> Result<usize, SessionError> {
        Ok(self.live()?.depth())
    }

    pub fn stack(&self) -> Result<Vec<u32>, SessionError> {
        Ok(self.live()?.stack().to_vec())
    }

    pub fn stats(&self) -> Result<Stats, SessionError> {
        Ok(stats(self.live()?.stats()))
    }

    pub fn finished(&self) -> Result<Option<Finished>, SessionError> {
        Ok(self.live()?.finished().map(finished))
    }

    /// Consumes the inner session; every later call reports `Stopped`.
    pub fn stop(&mut self) -> Result<Stats, SessionError> {
        let session = self.inner.take().ok_or(SessionError::Stopped)?;
        Ok(stats(session.stop()))
    }
}
```

Add `pub mod session;` to `inner/mod.rs`. Compile notes: if `AsyncSession::finished()` returns `Option<&RunResult>` the `map(finished)` works as written; if it returns by value, add `.as_ref()`. If `RunStats` is not `Copy`, pass by reference (`fn stats(s: &RunStats)`) and adjust the three call sites. If `SyncAsAsync<WideTape>` does not coerce to `&mut dyn AsyncTapeDevice` (the trait is implemented for `SyncAsAsync<T: Tape>` per `docs/core.md (async devices)`), the `as_async` body reads `d as &mut dyn AsyncTapeDevice`.

- [ ] **Step 4: Run to see them pass**

Run: `cargo test -p mtc-wasm --test session`
Expected: PASS, 6 tests. If `manual_pause_and_breakpoint_report_their_causes` sees `Paused(Breakpoint)` not fire, the "next instruction" chosen may be unreachable on this input; pick instead the address `program.address_for_line(8)` (the `['a']` rule's line) — it is executed on the second cell — and keep the comment explaining the choice.

- [ ] **Step 5: Gates, then drafted commit**

```
feat(wasm): Session — owned tapes, pumped by the embedder

Wraps AsyncSession with one WideTape per band (a binary band for PM-1),
seeds validated against each band's alphabet, pump events mirrored one
to one, traps spelled as the equivalence harnesses spell them, and a
stop() after which every call reports the session as stopped. The
device slot is a one-variant enum so a JS-implemented device can join
later.
```

---

### Task 7: The wasm-bindgen layer

**Files:**
- Create: `crates/wasm/src/js.rs`
- Modify: `crates/wasm/src/lib.rs` (replace the skeleton)

**Interfaces:**
- Consumes: everything `inner::*` produces (Tasks 1–6).
- Produces: JS classes `Toolchain` (static `check`, `format`, `build`), `Program` (`tapes`, `listing`, `lineOf`, `addressForLine`, `disassembly`, `bytes`, `mapJson`, `session`), `Session` (`pump`, `pause`, `addBreakpoint`, `removeBreakpoint`, `snapshot`, `snapshots`, getters `ip`/`mf`/`fr`/`depth`, `stack`, `stats`, `finished`, `stop`), and the TypeScript interfaces in the spec §4, verbatim.

This layer cannot be unit-tested natively (`js_sys` calls abort off-wasm); its test is the wasm32 build here and the Node smoke test in Task 9.

- [ ] **Step 1: Write the converters**

`crates/wasm/src/js.rs`:

```rust
//! Rust ↔ JS value plumbing for the boundary: plain objects out, options
//! objects in. The only file besides `lib.rs` that names `js_sys`.

use js_sys::{Array, Object, Reflect, Uint8Array};
use wasm_bindgen::JsValue;

use crate::inner::diagnostics::{Diag, Severity};
use crate::inner::listing::Row;
use crate::inner::program::{SourceLoc, TapeLayout};
use crate::inner::session::{Cause, Event, Finished, Limits, OutcomeInfo, Seed, Snapshot, Stats};

pub fn obj() -> Object {
    Object::new()
}

pub fn set(o: &Object, key: &str, value: impl Into<JsValue>) {
    // Reflect::set fails only on a frozen or non-object target; ours are fresh.
    Reflect::set(o, &JsValue::from_str(key), &value.into()).expect("fresh object is writable");
}

pub fn strings(items: &[String]) -> Array {
    items.iter().map(|s| JsValue::from_str(s)).collect()
}

pub fn u32s(items: &[u32]) -> Array {
    items.iter().map(|&n| JsValue::from_f64(n as f64)).collect()
}

pub fn diag(d: &Diag) -> JsValue {
    let o = obj();
    set(&o, "code", d.code.as_str());
    set(&o, "severity", match d.severity { Severity::Error => "error", Severity::Warning => "warning" });
    set(&o, "from", d.from);
    set(&o, "to", d.to);
    set(&o, "message", d.message.as_str());
    if let Some(f) = &d.fix {
        let fix = obj();
        set(&fix, "description", f.description.as_str());
        set(&fix, "applicability", if f.machine_applicable { "machineApplicable" } else { "maybeIncorrect" });
        let edits: Array = f
            .edits
            .iter()
            .map(|e| {
                let eo = obj();
                set(&eo, "from", e.from);
                set(&eo, "to", e.to);
                set(&eo, "replacement", e.replacement.as_str());
                JsValue::from(eo)
            })
            .collect();
        set(&fix, "edits", edits);
        set(&o, "fix", fix);
    }
    o.into()
}

pub fn diags(ds: &[Diag]) -> JsValue {
    ds.iter().map(diag).collect::<Array>().into()
}

pub fn layout(t: &TapeLayout) -> JsValue {
    let o = obj();
    set(&o, "name", t.name.as_str());
    set(&o, "glyphs", strings(&t.glyphs));
    o.into()
}

pub fn row(r: &Row) -> JsValue {
    let o = obj();
    set(&o, "addr", r.addr);
    set(&o, "bytes", r.bytes.as_str());
    set(&o, "mnemonic", r.mnemonic.as_str());
    set(&o, "operand", r.operand.as_str());
    set(&o, "function", r.function.as_deref().map(JsValue::from_str).unwrap_or(JsValue::NULL));
    set(&o, "label", r.label.as_deref().map(JsValue::from_str).unwrap_or(JsValue::NULL));
    o.into()
}

pub fn source_loc(l: &SourceLoc) -> JsValue {
    let o = obj();
    set(&o, "function", l.function.as_str());
    set(&o, "line", l.line.map(JsValue::from).unwrap_or(JsValue::NULL));
    o.into()
}

pub fn stats(s: &Stats) -> JsValue {
    let o = obj();
    set(&o, "steps", s.steps as f64);
    set(&o, "coreTacts", s.core_tacts as f64);
    set(&o, "stallTacts", s.stall_tacts as f64);
    set(&o, "totalTacts", s.total_tacts as f64);
    o.into()
}

pub fn finished(f: &Finished) -> JsValue {
    let o = obj();
    let outcome = obj();
    match &f.outcome {
        OutcomeInfo::Stopped => set(&outcome, "kind", "stopped"),
        OutcomeInfo::Halted => set(&outcome, "kind", "halted"),
        OutcomeInfo::Trapped(t) => {
            set(&outcome, "kind", "trapped");
            let trap = obj();
            set(&trap, "kind", t.kind);
            set(&trap, "at", t.at.map(JsValue::from).unwrap_or(JsValue::UNDEFINED));
            set(&trap, "detail", t.detail.as_str());
            set(&outcome, "trap", trap);
        }
    }
    set(&o, "outcome", outcome);
    set(&o, "stats", stats(&f.stats));
    set(&o, "ip", f.ip);
    set(&o, "stack", u32s(&f.stack));
    o.into()
}

pub fn event(e: &Event) -> JsValue {
    let o = obj();
    match e {
        Event::DeviceWait => set(&o, "kind", "deviceWait"),
        Event::BudgetSpent => set(&o, "kind", "budgetSpent"),
        Event::Paused(c) => {
            set(&o, "kind", "paused");
            match c {
                Cause::Step => set(&o, "cause", "step"),
                Cause::Brk => set(&o, "cause", "brk"),
                Cause::Manual => set(&o, "cause", "manual"),
                Cause::Breakpoint(a) => {
                    let bp = obj();
                    set(&bp, "breakpoint", *a);
                    set(&o, "cause", bp);
                }
                Cause::Trap(t) => {
                    let tr = obj();
                    set(&tr, "trap", t.kind);
                    set(&o, "cause", tr);
                }
            }
        }
        Event::Finished(f) => {
            set(&o, "kind", "finished");
            set(&o, "result", finished(f));
        }
    }
    o.into()
}

pub fn snapshot(s: &Snapshot) -> JsValue {
    let o = obj();
    set(&o, "band", s.band);
    set(&o, "name", s.name.as_str());
    set(&o, "glyphs", strings(&s.glyphs));
    set(&o, "origin", s.origin as f64);
    set(&o, "cells", Uint8Array::from(s.cells.as_slice()));
    set(&o, "head", s.head as f64);
    o.into()
}

// ---- readers -----------------------------------------------------------

fn field(v: &JsValue, key: &str) -> Option<JsValue> {
    if v.is_undefined() || v.is_null() {
        return None;
    }
    Reflect::get(v, &JsValue::from_str(key)).ok().filter(|x| !x.is_undefined() && !x.is_null())
}

pub fn string_list(v: &JsValue, key: &str) -> Vec<String> {
    field(v, key)
        .map(|arr| Array::from(&arr).iter().filter_map(|x| x.as_string()).collect())
        .unwrap_or_default()
}

pub fn number(v: &JsValue, key: &str) -> Option<f64> {
    field(v, key).and_then(|x| x.as_f64())
}

pub fn limits(v: &JsValue) -> Limits {
    Limits {
        max_steps: number(v, "maxSteps").map(|n| n as u64),
        max_tacts: number(v, "maxTacts").map(|n| n as u64),
    }
}

/// `seeds` is `undefined`, or an array of `{ cells, head?, origin? }` where
/// `cells` is a `Uint8Array` or a number array.
pub fn seeds(v: &JsValue) -> Result<Vec<Seed>, String> {
    if v.is_undefined() || v.is_null() {
        return Ok(Vec::new());
    }
    Array::from(v)
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let cells_val = field(&s, "cells").ok_or_else(|| format!("seed {i}: missing `cells`"))?;
            let cells: Vec<u8> = if cells_val.is_instance_of::<Uint8Array>() {
                Uint8Array::new(&cells_val).to_vec()
            } else {
                Array::from(&cells_val)
                    .iter()
                    .map(|x| x.as_f64().map(|n| n as u8).ok_or_else(|| format!("seed {i}: non-numeric cell")))
                    .collect::<Result<_, _>>()?
            };
            Ok(Seed {
                cells,
                head: number(&s, "head").map(|n| n as i64).unwrap_or(0),
                origin: number(&s, "origin").map(|n| n as i64).unwrap_or(0),
            })
        })
        .collect()
}
```

- [ ] **Step 2: Write the classes**

Replace `crates/wasm/src/lib.rs` with:

```rust
//! Browser binding for the machine toolchains. `inner/` is plain Rust over
//! the three crates' public APIs and is what the native tests exercise;
//! this file and `js.rs` are the wasm-bindgen layer over it: three classes,
//! plain JS objects for every data type, and the TypeScript declarations of
//! those objects.

#[doc(hidden)]
pub mod inner;
mod js;

use wasm_bindgen::prelude::*;

use inner::Lang;
use inner::diagnostics::{CheckError, CheckOptions};
use inner::session::SessionError;

#[wasm_bindgen(typescript_custom_section)]
const TYPES: &str = r#"
export type Lang = "pmc" | "tmc";
export interface CheckOptions { allow?: string[]; warn?: string[] }
export interface BuildOptions { optLevel?: 0 | 1 }
export type FormatResult = { ok: true; text: string } | { ok: false; error: Diagnostic };
export type BuildResult =
  | { ok: true; program: Program; diagnostics: Diagnostic[] }
  | { ok: false; diagnostics: Diagnostic[] };
export interface TapeLayout { name: string; glyphs: string[] }
export interface ListingRow { addr: number; bytes: string; mnemonic: string; operand: string;
                              function: string | null; label: string | null }
export interface SourceLoc { function: string; line: number | null }
export interface Seed { cells: Uint8Array | number[]; head?: number; origin?: number }
export interface Limits { maxSteps?: number; maxTacts?: number }
export type PumpEvent =
  | { kind: "deviceWait" }
  | { kind: "budgetSpent" }
  | { kind: "paused"; cause: "step" | "brk" | "manual" | { breakpoint: number } | { trap: string } }
  | { kind: "finished"; result: RunResult };
export interface RunResult { outcome: Outcome; stats: RunStats; ip: number; stack: number[] }
export type Outcome = { kind: "stopped" } | { kind: "halted" } | { kind: "trapped"; trap: TrapInfo };
export interface TrapInfo { kind: string; at?: number; detail: string }
export interface RunStats { steps: number; coreTacts: number; stallTacts: number; totalTacts: number }
export interface TapeSnapshot { band: number; name: string; glyphs: string[];
                                origin: number; cells: Uint8Array; head: number }
export interface Diagnostic { code: string; severity: "error" | "warning";
                              from: number; to: number; message: string; fix?: Fix }
export interface Fix { description: string; applicability: "machineApplicable" | "maybeIncorrect";
                       edits: Edit[] }
export interface Edit { from: number; to: number; replacement: string }
"#;

fn lang(s: &str) -> Result<Lang, JsError> {
    Lang::parse(s).ok_or_else(|| JsError::new(&format!("unknown lang `{s}`; expected \"pmc\" or \"tmc\"")))
}

fn session_err(e: SessionError) -> JsError {
    JsError::new(&match e {
        SessionError::Stopped => "session already stopped".to_string(),
        SessionError::TooManySeeds { given, bands } => format!("{given} seeds for {bands} band(s)"),
        SessionError::BadSeed { band, index, width } => {
            format!("band {band}: cell index {index} outside its alphabet of {width}")
        }
        SessionError::NoSuchBand(b) => format!("no band {b}"),
        SessionError::Load(m) => format!("load failed: {m}"),
    })
}

/// Stateless entry points: the lint channel, the formatter, the build.
#[wasm_bindgen]
pub struct Toolchain;

#[wasm_bindgen]
impl Toolchain {
    #[wasm_bindgen(unchecked_return_type = "Diagnostic[]")]
    pub fn check(
        lang_name: &str,
        source: &str,
        #[wasm_bindgen(unchecked_param_type = "CheckOptions | undefined")] opts: JsValue,
    ) -> Result<JsValue, JsError> {
        let options = CheckOptions {
            allow: js::string_list(&opts, "allow"),
            warn: js::string_list(&opts, "warn"),
        };
        match inner::diagnostics::check(lang(lang_name)?, source, &options) {
            Ok(ds) => Ok(js::diags(&ds)),
            Err(CheckError::UnknownAllowCode(c)) => Err(JsError::new(&format!("unknown lint rule `{c}`"))),
        }
    }

    #[wasm_bindgen(unchecked_return_type = "FormatResult")]
    pub fn format(lang_name: &str, source: &str) -> Result<JsValue, JsError> {
        let o = js::obj();
        match inner::diagnostics::format(lang(lang_name)?, source) {
            Ok(text) => {
                js::set(&o, "ok", true);
                js::set(&o, "text", text.as_str());
            }
            Err(d) => {
                js::set(&o, "ok", false);
                js::set(&o, "error", js::diag(&d));
            }
        }
        Ok(o.into())
    }

    #[wasm_bindgen(unchecked_return_type = "BuildResult")]
    pub fn build(
        lang_name: &str,
        source: &str,
        #[wasm_bindgen(unchecked_param_type = "BuildOptions | undefined")] opts: JsValue,
    ) -> Result<JsValue, JsError> {
        let opt_level = js::number(&opts, "optLevel").map(|n| n as u8).unwrap_or(1);
        let o = js::obj();
        match inner::program::build(lang(lang_name)?, source, opt_level) {
            Ok((program, warnings)) => {
                js::set(&o, "ok", true);
                js::set(&o, "program", Program { inner: program });
                js::set(&o, "diagnostics", js::diags(&warnings));
            }
            Err(d) => {
                js::set(&o, "ok", false);
                js::set(&o, "diagnostics", js::diags(&[d]));
            }
        }
        Ok(o.into())
    }
}

/// A linked program: the executable, its map, and everything the browser
/// asks of them.
#[wasm_bindgen]
pub struct Program {
    inner: inner::program::Program,
}

#[wasm_bindgen]
impl Program {
    #[wasm_bindgen(unchecked_return_type = "TapeLayout[]")]
    pub fn tapes(&self) -> JsValue {
        self.inner.tapes().iter().map(js::layout).collect::<js_sys::Array>().into()
    }

    #[wasm_bindgen(unchecked_return_type = "ListingRow[]")]
    pub fn listing(&self) -> JsValue {
        inner::listing::rows(&self.inner).iter().map(js::row).collect::<js_sys::Array>().into()
    }

    #[wasm_bindgen(js_name = lineOf, unchecked_return_type = "SourceLoc | null")]
    pub fn line_of(&self, addr: u32) -> JsValue {
        self.inner.line_of(addr).map(|l| js::source_loc(&l)).unwrap_or(JsValue::NULL)
    }

    #[wasm_bindgen(js_name = addressForLine)]
    pub fn address_for_line(&self, line: u32) -> Option<u32> {
        self.inner.address_for_line(line)
    }

    pub fn disassembly(&self) -> String {
        self.inner.disassembly()
    }

    pub fn bytes(&self) -> Vec<u8> {
        self.inner.bytes()
    }

    #[wasm_bindgen(js_name = mapJson)]
    pub fn map_json(&self) -> String {
        self.inner.map_json()
    }

    pub fn session(
        &self,
        #[wasm_bindgen(unchecked_param_type = "Seed[] | undefined")] seeds: JsValue,
        #[wasm_bindgen(unchecked_param_type = "Limits | undefined")] limits: JsValue,
    ) -> Result<Session, JsError> {
        let seeds = js::seeds(&seeds).map_err(|m| JsError::new(&m))?;
        let inner = inner::session::Session::new(&self.inner, &seeds, js::limits(&limits))
            .map_err(session_err)?;
        Ok(Session { inner })
    }
}

/// A pumped run. The embedder owns the loop: every call to `pump` retires
/// instructions until a budget runs out, a pause fires, or the program ends.
#[wasm_bindgen]
pub struct Session {
    inner: inner::session::Session,
}

#[wasm_bindgen]
impl Session {
    #[wasm_bindgen(unchecked_return_type = "PumpEvent")]
    pub fn pump(&mut self, budget: Option<u32>) -> Result<JsValue, JsError> {
        self.inner.pump(budget.map(u64::from)).map(|e| js::event(&e)).map_err(session_err)
    }

    pub fn pause(&mut self) -> Result<(), JsError> {
        self.inner.pause().map_err(session_err)
    }

    #[wasm_bindgen(js_name = addBreakpoint)]
    pub fn add_breakpoint(&mut self, addr: u32) -> Result<(), JsError> {
        self.inner.add_breakpoint(addr).map_err(session_err)
    }

    #[wasm_bindgen(js_name = removeBreakpoint)]
    pub fn remove_breakpoint(&mut self, addr: u32) -> Result<(), JsError> {
        self.inner.remove_breakpoint(addr).map_err(session_err)
    }

    #[wasm_bindgen(unchecked_return_type = "TapeSnapshot")]
    pub fn snapshot(&self, band: u32) -> Result<JsValue, JsError> {
        self.inner.snapshot(band).map(|s| js::snapshot(&s)).map_err(session_err)
    }

    #[wasm_bindgen(unchecked_return_type = "TapeSnapshot[]")]
    pub fn snapshots(&self) -> Result<JsValue, JsError> {
        self.inner
            .snapshots()
            .map(|v| v.iter().map(js::snapshot).collect::<js_sys::Array>().into())
            .map_err(session_err)
    }

    #[wasm_bindgen(getter)]
    pub fn ip(&self) -> Result<u32, JsError> {
        self.inner.ip().map_err(session_err)
    }

    #[wasm_bindgen(getter)]
    pub fn mf(&self) -> Result<bool, JsError> {
        self.inner.mf().map_err(session_err)
    }

    #[wasm_bindgen(getter)]
    pub fn fr(&self) -> Result<u32, JsError> {
        self.inner.fr().map_err(session_err)
    }

    #[wasm_bindgen(getter)]
    pub fn depth(&self) -> Result<u32, JsError> {
        self.inner.depth().map(|d| d as u32).map_err(session_err)
    }

    #[wasm_bindgen(unchecked_return_type = "number[]")]
    pub fn stack(&self) -> Result<JsValue, JsError> {
        self.inner.stack().map(|s| js::u32s(&s).into()).map_err(session_err)
    }

    #[wasm_bindgen(unchecked_return_type = "RunStats")]
    pub fn stats(&self) -> Result<JsValue, JsError> {
        self.inner.stats().map(|s| js::stats(&s)).map_err(session_err)
    }

    #[wasm_bindgen(unchecked_return_type = "RunResult | null")]
    pub fn finished(&self) -> Result<JsValue, JsError> {
        self.inner
            .finished()
            .map(|f| f.map(|f| js::finished(&f)).unwrap_or(JsValue::NULL))
            .map_err(session_err)
    }

    /// Ends the run and returns its statistics; every later call throws.
    #[wasm_bindgen(unchecked_return_type = "RunStats")]
    pub fn stop(&mut self) -> Result<JsValue, JsError> {
        self.inner.stop().map(|s| js::stats(&s)).map_err(session_err)
    }
}
```

Notes for the implementer: `#[wasm_bindgen(unchecked_return_type)]` and `unchecked_param_type` exist from wasm-bindgen 0.2.96 on. `js::set(&o, "program", Program { .. })` moves the class instance into the object — wasm-bindgen structs implement `Into<JsValue>`. If `JsValue::from(u32)` is ambiguous for `Option<u32>` in `source_loc`/`finished`, write `JsValue::from_f64(n as f64)`.

- [ ] **Step 3: Build for wasm32 and on the host**

Run: `cargo build -p mtc-wasm --target wasm32-unknown-unknown && cargo build -p mtc-wasm && cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --check`
Expected: all clean. Fix any signature mismatch against `inner/` here rather than in `inner/` (the native tests pin `inner/`).

- [ ] **Step 4: Run the whole suite**

Run: `cargo nextest run --workspace`
Expected: PASS; the boundary test still passes (this task added `js_sys` only to `js.rs` and `lib.rs`).

- [ ] **Step 5: Drafted commit**

```
feat(wasm): the wasm-bindgen layer — Toolchain, Program, Session

Three classes over inner/, plain JS objects for every data type, and a
TypeScript custom section declaring them so the generated .d.ts is the
API reference. Errors that are caller bugs throw; findings and fatals
are values.
```

---

### Task 8: The bundle build script

**Files:**
- Create: `scripts/build-wasm-bundle.sh` (executable)
- Modify: `.gitignore` (add `/target/` if not already ignored — check first; it is in a Rust repo)

**Interfaces:**
- Consumes: the `[profile.wasm]` from Task 1; `crates/wasm/Cargo.toml`'s `wasm-bindgen = "=0.2.127"` line.
- Produces: `target/wasm-bundle/dist/{mtc_wasm_bg.wasm,mtc_wasm.js,mtc_wasm.d.ts,manifest.json}` and `target/wasm-bundle/machine-toolchains-wasm-v<version>.tar.gz`. Exit non-zero on any failure, including a wasm-bindgen CLI version mismatch.

- [ ] **Step 1: Install the two tools locally, pinned**

Run: `cargo install wasm-bindgen-cli --version 0.2.127 --locked && which wasm-opt || brew install binaryen`
Expected: `wasm-bindgen --version` prints `wasm-bindgen 0.2.127`; `wasm-opt --version` prints a version.

- [ ] **Step 2: Write the script**

`scripts/build-wasm-bundle.sh`:

```bash
#!/usr/bin/env bash
# Build the browser bundle: cargo (profile `wasm`) → wasm-bindgen (target
# web) → wasm-opt → manifest.json → tarball. Same command locally and in
# CI; the release workflow attaches the tarball to the tagged release.
#
# Output: target/wasm-bundle/dist/ and target/wasm-bundle/<name>.tar.gz
# Requires: the pinned toolchain (rust-toolchain.toml, wasm32 target
# included), wasm-bindgen CLI at EXACTLY the crate's pinned version, and
# binaryen's wasm-opt.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"

crate_toml="crates/wasm/Cargo.toml"
version="$(sed -n 's/^version = "\(.*\)"/\1/p' "$crate_toml" | head -1)"
pin="$(sed -n 's/^wasm-bindgen = "=\(.*\)"/\1/p' "$crate_toml" | head -1)"
[ -n "$version" ] || { echo "cannot read version from $crate_toml" >&2; exit 1; }
[ -n "$pin" ] || { echo "cannot read the wasm-bindgen pin from $crate_toml" >&2; exit 1; }

have="$(wasm-bindgen --version | awk '{print $2}')"
if [ "$have" != "$pin" ]; then
  echo "wasm-bindgen CLI is $have but the crate pins $pin; install the matching CLI:" >&2
  echo "  cargo install wasm-bindgen-cli --version $pin --locked" >&2
  exit 1
fi
command -v wasm-opt >/dev/null || { echo "wasm-opt (binaryen) is required" >&2; exit 1; }

out="target/wasm-bundle"
dist="$out/dist"
rm -rf "$out"
mkdir -p "$dist"

cargo build -p mtc-wasm --profile wasm --target wasm32-unknown-unknown
wasm-bindgen --target web --out-dir "$dist" --out-name mtc_wasm \
  target/wasm32-unknown-unknown/wasm/mtc_wasm.wasm
wasm-opt -Oz --enable-bulk-memory --enable-nontrapping-float-to-int \
  -o "$dist/mtc_wasm_bg.wasm" "$dist/mtc_wasm_bg.wasm"

# wasm-bindgen's web target may also emit a *_bg.wasm.d.ts; keep only the
# four files the manifest names.
find "$dist" -type f ! -name 'mtc_wasm_bg.wasm' ! -name 'mtc_wasm.js' ! -name 'mtc_wasm.d.ts' -delete

sha256() {
  if command -v sha256sum >/dev/null; then sha256sum "$1" | awk '{print $1}'
  else shasum -a 256 "$1" | awk '{print $1}'; fi
}
commit="$(git rev-parse HEAD 2>/dev/null || echo unknown)"
{
  echo "{"
  echo "  \"toolchains_version\": \"$version\","
  echo "  \"crate_version\": \"$version\","
  echo "  \"wasm_bindgen_version\": \"$pin\","
  echo "  \"built_from\": \"$commit\","
  echo "  \"files\": {"
  echo "    \"mtc_wasm_bg.wasm\": \"$(sha256 "$dist/mtc_wasm_bg.wasm")\","
  echo "    \"mtc_wasm.js\": \"$(sha256 "$dist/mtc_wasm.js")\","
  echo "    \"mtc_wasm.d.ts\": \"$(sha256 "$dist/mtc_wasm.d.ts")\""
  echo "  }"
  echo "}"
} > "$dist/manifest.json"

name="machine-toolchains-wasm-v$version"
tar -czf "$out/$name.tar.gz" -C "$out" --transform "s,^dist,$name," dist 2>/dev/null \
  || tar -czf "$out/$name.tar.gz" -C "$out" -s ",^dist,$name," dist   # BSD tar (macOS)

raw=$(wc -c < "$dist/mtc_wasm_bg.wasm")
gz=$(gzip -9 -c "$dist/mtc_wasm_bg.wasm" | wc -c)
echo "bundle: $out/$name.tar.gz"
echo "wasm:   $raw bytes, $gz bytes gzipped"
```

Run: `chmod +x scripts/build-wasm-bundle.sh`

- [ ] **Step 3: Run it**

Run: `scripts/build-wasm-bundle.sh`
Expected: ends with the two `bundle:`/`wasm:` lines; `ls target/wasm-bundle/dist` shows exactly four files; `tar -tzf target/wasm-bundle/*.tar.gz` lists them under `machine-toolchains-wasm-v0.4.0/`. Record the gzipped size in the drafted commit message. If `tar --transform` and `-s` both fail on your platform, fall back to `cp -r dist "$out/$name" && tar -czf "$out/$name.tar.gz" -C "$out" "$name"`.

- [ ] **Step 4: Prove the version guard**

Run: `sed -i.bak 's/=0.2.127/=0.2.126/' crates/wasm/Cargo.toml && (scripts/build-wasm-bundle.sh; echo "exit $?") ; mv crates/wasm/Cargo.toml.bak crates/wasm/Cargo.toml`
Expected: the script prints the mismatch message and `exit 1` before building anything. The manifest is restored afterwards; run `git diff --stat crates/wasm/Cargo.toml` and expect no change.

- [ ] **Step 5: Drafted commit**

```
feat(wasm): the bundle build script

cargo (profile wasm) → wasm-bindgen --target web → wasm-opt -Oz →
manifest.json with per-file SHA-256 → tarball. Refuses to run when the
wasm-bindgen CLI differs from the crate's pin. First measured bundle:
<N> bytes gzipped.
```

---

### Task 9: Node smoke test and the CI wiring

**Files:**
- Create: `scripts/wasm-smoke.mjs`
- Modify: `.github/workflows/test.yml`

**Interfaces:**
- Consumes: Task 8's `target/wasm-bundle/dist/`; the JS API of Task 7.
- Produces: a Node script that exits non-zero on any mismatch and prints the bundle sizes; CI steps that build the bundle and run it.

- [ ] **Step 1: Write the smoke test**

`scripts/wasm-smoke.mjs`:

```js
#!/usr/bin/env node
// End-to-end over the BUILT bundle (not the Rust crate): load the web-target
// glue from bytes, then for both languages check → format → build → run,
// verifying manifest checksums, the line table, and a size ceiling.
//
//   node scripts/wasm-smoke.mjs target/wasm-bundle/dist
import { createHash } from "node:crypto";
import { readFileSync, statSync } from "node:fs";
import { gzipSync } from "node:zlib";
import { join } from "node:path";
import { pathToFileURL } from "node:url";

const dist = process.argv[2];
if (!dist) { console.error("usage: wasm-smoke.mjs <dist dir>"); process.exit(2); }

let failures = 0;
function check(cond, msg) {
  if (cond) { console.log(`ok   ${msg}`); } else { failures++; console.log(`FAIL ${msg}`); }
}
function eq(a, b, msg) { check(JSON.stringify(a) === JSON.stringify(b), `${msg} (${JSON.stringify(a)} vs ${JSON.stringify(b)})`); }

// --- manifest and checksums --------------------------------------------
const manifest = JSON.parse(readFileSync(join(dist, "manifest.json"), "utf8"));
for (const [file, sha] of Object.entries(manifest.files)) {
  const actual = createHash("sha256").update(readFileSync(join(dist, file))).digest("hex");
  eq(actual, sha, `checksum ${file}`);
}
check(/^\d+\.\d+\.\d+$/.test(manifest.crate_version), `crate_version ${manifest.crate_version}`);

// --- load --------------------------------------------------------------
const glue = await import(pathToFileURL(join(dist, "mtc_wasm.js")).href);
const wasmBytes = readFileSync(join(dist, "mtc_wasm_bg.wasm"));
await glue.default({ module_or_path: wasmBytes });
const { Toolchain } = glue;

const PMC_INC = "main() {\n    1: right(2);\n    2: check(1, 3);\n    3: mark(4);\n    4: left(5);\n    5: check(4, 6);\n    6: right(!);\n}\n";
const PMC_UNUSED_LABEL = "namespace api {\nhelper() {\n5: right;\n}\n}\nmain() { @api::helper(); }\n";
const TMC_REPLACE_B = "alphabet ab { '_', 'a', 'b' }\n\nmachine {\n  tape main: ab;\n\n  entry state scan {\n    ['b'] -> write ['a'] move [>] goto scan;\n    ['a'] ->             move [>] goto scan;\n    ['_'] -> stop;\n  }\n}\n";
const TMC_UNUSED_ALPHABET = "alphabet ab { '_', 'a', 'b' }\nalphabet spare { '_', 'x' }\n\nmachine {\n  tape main: ab;\n\n  entry state scan {\n    ['b'] -> write ['a'] move [>] goto scan;\n    ['a'] ->             move [>] goto scan;\n    ['_'] -> stop;\n  }\n}\n";

function runToEnd(session) {
  for (;;) {
    const ev = session.pump();
    if (ev.kind === "finished") return ev.result;
    if (ev.kind !== "budgetSpent") throw new Error(`unexpected ${JSON.stringify(ev)}`);
  }
}

// --- check / format ----------------------------------------------------
check(Toolchain.check("pmc", PMC_UNUSED_LABEL).some(d => d.code === "unused-label"), "pmc check finds unused-label");
check(Toolchain.check("tmc", TMC_UNUSED_ALPHABET).some(d => d.code === "unused-alphabet"), "tmc check finds unused-alphabet");
const fatal = Toolchain.check("pmc", "main() { nope");
eq(fatal.length, 1, "pmc fatal is one diagnostic"); eq(fatal[0]?.severity, "error", "…of severity error");
for (const [lang, src] of [["pmc", PMC_UNUSED_LABEL], ["tmc", TMC_UNUSED_ALPHABET]]) {
  const once = Toolchain.format(lang, src);
  check(once.ok, `${lang} format ok`);
  const twice = Toolchain.format(lang, once.text);
  eq(twice.text, once.text, `${lang} format idempotent`);
}
let threw = false;
try { Toolchain.check("cobol", "x"); } catch { threw = true; }
check(threw, "unknown lang throws");

// --- build / run: pmc --------------------------------------------------
{
  const r = Toolchain.build("pmc", PMC_INC, { optLevel: 1 });
  check(r.ok, "pmc builds");
  const p = r.program;
  eq(p.tapes(), [{ name: "tape", glyphs: [" ", "*"] }], "pmc tape layout");
  check(p.listing().length > 0 && p.listing()[0].addr === 0, "pmc listing starts at 0");
  check(p.disassembly().includes("main"), "pmc disassembly names main");
  check(p.bytes().length > 0, "pmc MX bytes");
  check(JSON.parse(p.mapJson()).functions.length > 0, "pmc map json");
  const s = p.session([{ cells: [1, 1, 1], head: 0 }]);
  const result = runToEnd(s);
  eq(result.outcome.kind, "stopped", "pmc stopped");
  const snap = s.snapshot(0);
  eq(Array.from(snap.cells.slice(0, 4)), [1, 1, 1, 1], "pmc final tape");
  eq(snap.head, 0, "pmc head back on the first mark");
  s.stop();
  let stopped = false; try { s.pump(); } catch { stopped = true; } check(stopped, "pmc use after stop throws");
  p.free();
}

// --- build / run: tmc --------------------------------------------------
{
  const r = Toolchain.build("tmc", TMC_REPLACE_B, { optLevel: 0 });
  check(r.ok, "tmc builds");
  const p = r.program;
  eq(p.tapes(), [{ name: "main", glyphs: ["_", "a", "b"] }], "tmc tape layout");
  const line = 8; // the ['a'] rule
  const addr = p.addressForLine(line);
  check(addr !== undefined && addr !== null, `tmc line ${line} has an address`);
  eq(p.lineOf(addr)?.line, line, "tmc lineOf(addressForLine(n)) is n");
  const s = p.session([{ cells: new Uint8Array([2, 2, 2]) }], { maxSteps: 1000 });
  s.pause();
  eq(s.pump().kind, "paused", "tmc manual pause fires");
  const result = runToEnd(s);
  eq(result.outcome.kind, "stopped", "tmc stopped");
  const snap = s.snapshot(0);
  eq(Array.from(snap.cells.slice(0, 3)), [1, 1, 1], "tmc final tape");
  eq(snap.head, 3, "tmc head on the first blank");
  const stats = s.stop();
  check(stats.steps > 0, "tmc stats carry steps");
  p.free();
}
{
  const r = Toolchain.build("tmc", TMC_REPLACE_B);
  const s = r.program.session([{ cells: [2, 2, 2, 2, 2, 2, 2, 2] }], { maxSteps: 2 });
  const result = runToEnd(s);
  eq(result.outcome.kind, "trapped", "tmc step limit traps");
  eq(result.outcome.trap?.kind, "step-limit", "…as step-limit");
  r.program.free();
}
{
  const r = Toolchain.build("tmc", "alphabet a { '_' }\nmachine {");
  eq(r.ok, false, "tmc fatal is not ok");
  eq(r.diagnostics.length, 1, "…with one diagnostic");
}

// --- size ceiling ------------------------------------------------------
const raw = statSync(join(dist, "mtc_wasm_bg.wasm")).size;
const gz = gzipSync(wasmBytes, { level: 9 }).length;
console.log(`size raw=${raw} gzip=${gz}`);
check(gz < 1_000_000, "gzipped wasm under the 1 MB ceiling");

if (failures) { console.error(`${failures} check(s) failed`); process.exit(1); }
console.log("smoke: all checks passed");
```

- [ ] **Step 2: Run it against the local bundle**

Run: `scripts/build-wasm-bundle.sh && node scripts/wasm-smoke.mjs target/wasm-bundle/dist`
Expected: every line starts with `ok`, then `smoke: all checks passed`, exit 0. If the `web` glue's `init` rejects the `{ module_or_path }` object on this wasm-bindgen version, call `await glue.default(wasmBytes)` instead and note the version dependency in a comment. If line 8 has no address, use the line the Task 4 test settled on.

- [ ] **Step 3: Prove the smoke test discriminates**

Run: `node -e 'const fs=require("fs");const p="target/wasm-bundle/dist/manifest.json";const m=JSON.parse(fs.readFileSync(p));m.files["mtc_wasm.js"]="0".repeat(64);fs.writeFileSync(p,JSON.stringify(m))' && node scripts/wasm-smoke.mjs target/wasm-bundle/dist; echo "exit $?"`
Expected: `FAIL checksum mtc_wasm.js` and `exit 1`. Rebuild afterwards: `scripts/build-wasm-bundle.sh`.

- [ ] **Step 4: Wire CI**

In `.github/workflows/test.yml`, after the `Build the libraries for wasm32` step and before `taiki-e/install-action@nextest`, add:

```yaml
      - uses: taiki-e/install-action@v2
        with:
          tool: wasm-bindgen@0.2.127
      - name: Install binaryen
        run: sudo apt-get update -q && sudo apt-get install -y -q binaryen
      - uses: actions/setup-node@v4
        with:
          node-version: 24
      - name: Build the wasm bundle
        run: scripts/build-wasm-bundle.sh
      - name: Smoke-test the wasm bundle under Node
        run: node scripts/wasm-smoke.mjs target/wasm-bundle/dist
```

Update the header comment's command count from "five" to "seven" and add "the browser bundle and its Node smoke test" to the list. The `wasm-bindgen@0.2.127` here must equal the crate's pin; the script refuses otherwise, so a drift fails loudly rather than silently.

- [ ] **Step 5: Validate the workflow and run the full gate**

Run: `python3 -c "import yaml; yaml.safe_load(open('.github/workflows/test.yml')); print('yaml ok')" && cargo fmt --check && cargo clippy --workspace --all-targets -- -D warnings && cargo nextest run --workspace`
Expected: all green.

- [ ] **Step 6: Drafted commit**

```
ci: build the wasm bundle and smoke-test it under Node

The smoke test loads the web-target glue from bytes and runs check,
format, build, a pumped session, the line table, and a trap for both
languages, verifies the manifest checksums, and holds the gzipped wasm
under a 1 MB ceiling. CI installs the pinned wasm-bindgen CLI and
binaryen and runs it after the wasm32 library gate.
```

---

### Task 10: The release workflow

**Files:**
- Create: `.github/workflows/release.yml`

**Interfaces:**
- Consumes: Task 8's script.
- Produces: on a `v*` tag push, the tarball attached to that tag's GitHub release.

- [ ] **Step 1: Write the workflow**

`.github/workflows/release.yml`:

```yaml
name: release

# Attaches the browser bundle to the GitHub release for a pushed `v*` tag.
# The release itself is still created by the maintainer (`gh release
# create`, with the editor plugin artifacts); this job only adds the wasm
# bundle, built on CI from the tagged commit with the pinned toolchain and
# the pinned wasm-bindgen CLI, so the artifact is reproducible and never
# depends on a laptop's tool versions.
#
# Ordering: `gh release create vX.Y.Z` both creates the tag and the
# release, so the release normally exists before this job starts. If the
# tag was pushed first, the upload step waits for the release for up to
# 30 minutes, then fails; re-run the job after creating the release.

on:
  push:
    tags:
      - "v*"

permissions:
  contents: write

jobs:
  bundle:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - uses: Swatinem/rust-cache@v2
      - uses: taiki-e/install-action@v2
        with:
          tool: wasm-bindgen@0.2.127
      - name: Install binaryen
        run: sudo apt-get update -q && sudo apt-get install -y -q binaryen
      - name: Build the wasm bundle
        run: scripts/build-wasm-bundle.sh
      - name: Wait for the release, then attach the bundle
        env:
          GH_TOKEN: ${{ github.token }}
        run: |
          tag="${GITHUB_REF_NAME}"
          for i in $(seq 1 30); do
            if gh release view "$tag" >/dev/null 2>&1; then
              gh release upload "$tag" target/wasm-bundle/*.tar.gz --clobber
              echo "attached to $tag"
              exit 0
            fi
            echo "release $tag not found yet (attempt $i/30); waiting 60s"
            sleep 60
          done
          echo "no release for $tag after 30 minutes" >&2
          exit 1
```

- [ ] **Step 2: Validate**

Run: `python3 -c "import yaml; d=yaml.safe_load(open('.github/workflows/release.yml')); print([s.get('name') or s.get('uses') for s in d['jobs']['bundle']['steps']])"`
Expected: the seven steps in order. The job itself is exercised by the next release; note that in the drafted commit.

- [ ] **Step 3: Drafted commit**

```
ci: attach the wasm bundle to tagged releases

Tag-triggered; builds the bundle on CI with the pinned toolchain and
wasm-bindgen CLI and uploads it to the tag's release, waiting up to 30
minutes for the maintainer's `gh release create` if the tag landed
first. First exercised by the next release cut.
```

---

### Task 11: Documentation

**Files:**
- Create: `docs/wasm.md`
- Modify: `README.md` (front door paragraph), `CLAUDE.md` (Architecture, Commands, version table, release flow)
- Modify: `crates/wasm/src/inner/session.rs:1-3` and `crates/wasm/src/lib.rs:1-5` (add the `docs/wasm.md (...)` citations now that the page exists)

- [ ] **Step 1: Write the durable page**

`docs/wasm.md`:

````markdown
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
fat LTO, `panic = "abort"`) and `wasm-opt -Oz`. Measured at 0.4.0 without
the JS glue: 795 KB, 321 KB gzipped, for both toolchains' full chains —
smaller than a typical diagram-rendering library. A ceiling of 1 MB
gzipped is enforced by the smoke test.

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
error naming the band. The session is the pumped `AsyncSession`
`docs/core.md (AsyncSession)` describes; the pause priority (a retired
break, then a pending pause, then a breakpoint, then the budget) and the
budget semantics are that contract, unchanged. A trap is not a pause: it
ends the run with `outcome.kind === "trapped"` and the trap's kind
spelled as the CLI's exit code 3 family — `step-limit`, `no-transition`,
and so on. `stopped` and `halted` match exit codes 0 and 2.

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
````

- [ ] **Step 2: The README paragraph**

In `README.md`, after the paragraph introducing the two toolchains (find the first `##` section that lists `pmt` and `tmt`), add:

```markdown
Both toolchains also run in a browser: `mtc-wasm` exposes compile, lint,
format, disassembly and a pumped run session to JavaScript, and every
release ships the resulting bundle. See `docs/wasm.md`.
```

- [ ] **Step 3: CLAUDE.md edits**

Apply these exact edits to `CLAUDE.md`:

1. Version table: add a row `| \`mtc-wasm\` crate / JS API | 0.4.0 |` under the crates row.
2. Architecture, after the `crates/turing-machine` bullet, add:

```markdown
- **`crates/wasm` (`mtc-wasm`)** — the browser binding: three wasm-bindgen classes (`Toolchain`, `Program`, `Session`) over a plain-Rust `inner/` layer (positions → UTF-16, the lint/format/build channels, listing rows, a pumped `Session` over `AsyncSession` with owned `WideTape`s, a per-tape-count cache of leaked `ArchRegistry`s for the `'static` lifetimes a class needs). The only crate carrying `wasm-bindgen`/`js-sys`, both pinned; `inner/` is held free of them by a test. Reference: `docs/wasm.md`.
```

3. Commands block: add

```
scripts/build-wasm-bundle.sh                              # the browser bundle → target/wasm-bundle/ (needs wasm-bindgen CLI at the crate's pin + binaryen)
node scripts/wasm-smoke.mjs target/wasm-bundle/dist       # end-to-end over the built bundle
```

4. The CI sentence: "runs fmt → clippy → the no_std build → the wasm32 library build → the bundle build and its Node smoke test → `cargo nextest run --workspace` on ubuntu."
5. Release flow paragraph (under "Version spaces and release notes"): append "The tag push also triggers `release.yml`, which builds the wasm bundle on CI and attaches it to the release; `gh release create` should run first (it creates the tag), and the job waits up to 30 minutes for the release otherwise."
6. Open work: replace the `#6` clause with "#6 the browser arc — the `mtc-wasm` crate and bundle have landed; what remains is the demo-side round in `machines-demo`".

- [ ] **Step 4: Citations in code**

`crates/wasm/src/inner/session.rs` line 3 already says `docs/core.md (AsyncSession)`; add to the module doc: `//! The JS-facing contract is docs/wasm.md (sessions).` In `crates/wasm/src/lib.rs` module doc add: `//! Reference: docs/wasm.md (the object model).`

- [ ] **Step 5: Verify every claim on the page**

Run: `scripts/build-wasm-bundle.sh && node scripts/wasm-smoke.mjs target/wasm-bundle/dist && tar -tzf target/wasm-bundle/*.tar.gz && grep -c 'class Toolchain\|class Program\|class Session' target/wasm-bundle/dist/mtc_wasm.d.ts`
Expected: smoke passes; the tarball lists exactly the four files under the versioned directory; the grep prints `3`. Replace the page's measured size sentence with the numbers the script printed if they differ from the 0.4.0 measurement by more than 10%.

Run: `cargo fmt --check && cargo clippy --workspace --all-targets -- -D warnings && cargo nextest run --workspace`
Expected: all green.

- [ ] **Step 6: Drafted commit**

```
docs(wasm): the browser bundle reference, front door, and router entries

docs/wasm.md is the durable page: bundle contents and verification, the
object model, positions, the session contract by reference to core's,
and what is deliberately absent. README gains the paragraph, CLAUDE.md
the crate, commands, version row and release step.
```

---

## Self-review against the spec

- **§1 measured facts** → recorded in `docs/wasm.md` (Task 11) and the plan header.
- **§2.1 release artifact, crate here** → Tasks 8, 10. **§2.2 v1 surface** → Tasks 3 (check, format), 4 (build), 5 (dis), 6 (run). **§2.3 classes + plain objects, no serde_json** → Task 7 (`js.rs` builds objects; `mapJson` uses core's `to_json`, which does pull serde_json's serializer — the size ceiling in Task 9 is the guard, and the measured number in Task 8's commit records the actual cost). **§2.4 Rust-held tapes, one-variant device slot** → Task 6 `enum Device { Owned }`. **§2.5 UTF-16** → Task 2. **§2.6 native + Node smoke** → Tasks 1–6 native, Task 9 smoke. **§2.7 one class family** → Task 7. **§2.8 debug_info always** → Task 4. **§2.9 channel split** → Tasks 3 and 4.
- **§3 crate/bundle** → Tasks 1, 8. Deviation from the spec: a process-wide static registry is impossible because `Tm1::new(tape_count)` is per program; Task 1 uses a per-tape-count cache of leaked registries (bounded at 255). Recorded in the spec by an amendment note (do this in Task 1: add one sentence to spec §3 "Layout" saying so).
- **§4 object model** → Task 7, signatures match §4 with two additions the TS section declares: `cause` may also be `{ trap: string }` (exhaustive over `PauseCause`), and `TrapInfo.detail` is required rather than optional.
- **§5 positions/diagnostics** → Tasks 2, 3. **§6 session semantics** → Task 6 (seeds, blanks, validation, pump, traps as finished, snapshots after finish, stop). **§7 build/release** → Tasks 8, 9 (test.yml), 10. **§8 testing** → Tasks 1–6, 9; size ceiling in Task 9. **§9 docs/versions** → Task 11; CHANGELOG deferred to the cut per the standing ruling. **§10 out of scope** → nothing in the plan builds them. **§11 open items** → wasm-bindgen pin fixed at 0.2.127 (Task 1); CLI install via `taiki-e/install-action` (Task 9); release wait bounded at 30 minutes (Task 10); the `print.rs` warning fixed (Task 1).
- **Type consistency**: `Diag` fields `code, severity, from, to, message, fix` used identically in Tasks 3, 4, 7; `Session::stop(&mut self)` in Task 6 matches Task 7's `stop(&mut self)`; `Event`/`Cause`/`Finished`/`Stats`/`Snapshot` names match between Tasks 6 and 7; `Row` fields match between Tasks 5 and 7; `TapeLayout` between 4, 6, 7; `Seed`/`Limits` between 6 and 7.
