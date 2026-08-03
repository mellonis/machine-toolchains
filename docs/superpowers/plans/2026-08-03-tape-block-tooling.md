# Tape-Block Tooling Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make tape authoring usable — rename the subcommand to `tape-block`, author a whole block in one invocation, pin real glyphs onto a block minted from an image, render unambiguously, and fix a silent wrong-glyph defect the round would otherwise activate.

**Architecture:** A new glyph-list parser in `mtc-core` gives both CLIs one notation, reusing `.tmc` alphabet syntax and pinned to it by a drift guard. Both `cli/inspect.rs` files grow keyed, repeatable edit flags (`--alphabet KEY=GLYPHS`, `--cells`, `--head`, `--origin`) so one invocation authors a whole block. No container change: MT stays at v2 and tape names are never persisted.

**Tech Stack:** Rust (2024 edition), cargo workspace, `proptest` dev-dep only. No clap — CLI parsing is hand-rolled through `cli::Args`.

**Spec:** `docs/superpowers/specs/2026-08-03-tape-block-tooling-design.md` (rulings R1–R11). **Tracker:** #61.

## Global Constraints

- **MT container stays at version 2.** No new format version, no tape names in the container. `crates/core/src/formats/tapeblock.rs` byte layout is untouched.
- **PM-1 byte-identity is a standing regression gate.** PM blocks keep emitting MT v1; `pm1_syntax()` never opts into `AsmCaps`.
- **Every committed golden stays byte-identical** — 3 `.pmt`, 10 `.tmt`. Never regenerate a golden to make a test pass; goldens are derivation-first.
- **Thin-renderer rule:** library code never prints. Every byte of terminal output originates in `cli/`; stages return structured reports.
- **`crates/core` carries zero PM-1/TM-1 knowledge.** The glyph parser is arch-agnostic — no opcode, mnemonic, or architecture reference.
- **No dependency additions.** `serde`/`serde_json` only, `proptest` as dev-dep.
- **Docs are forge-agnostic:** no issue/PR numbers or hosting URLs in `README.md`, `docs/`, or code comments. Code comments cite durable pages as `docs/<page>.md (keyword)` — never a `docs/superpowers/` path.
- **Commit style:** conventional commits with scope — `feat(cli):`, `fix(core):`, `test(post-machine):`, `docs(tmt):`.
- **Quality gates, run before every commit:** `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo fmt --check`.

## File Structure

| File | Responsibility |
|---|---|
| `crates/core/src/formats/glyphs.rs` | **new** — glyph-list notation parser (`parse_glyph_list`) and its error type |
| `crates/core/src/formats/mod.rs` | re-export the glyph module |
| `crates/turing-machine/src/compiler.rs` | **new public accessor** `machine_tape_layout` — tape names + glyphs from `.tmc` source |
| `crates/turing-machine/tests/glyph_notation_parity.rs` | **new** — bidirectional drift guard: core's parser vs `.tmc`'s resolver |
| `crates/{post-machine,turing-machine}/src/cli/mod.rs` | `render_tape` delimiting policy; `parse_keyed` edit-flag helper |
| `crates/{post-machine,turing-machine}/src/cli/inspect.rs` | the `tape-block` subcommand |
| `crates/{post-machine,turing-machine}/src/cli/run.rs` | flag realignment; `tmt --save-tape-block`; TM cardinality check; PM defect fix |
| `crates/{post-machine,turing-machine}/src/completions/registry.rs` | renamed paths, new flags |
| `crates/post-machine/tests/fixtures/per_tape_alphabet.pmt` | **new** — regression fixture whose override disagrees with the fallback |
| `docs/formats.md`, `docs/pmt/cli.md`, `docs/tmt/cli.md` | durable references |

---

### Task 1: Core glyph-list parser

**Files:**
- Create: `crates/core/src/formats/glyphs.rs`
- Modify: `crates/core/src/formats/mod.rs`

**Interfaces:**
- Consumes: nothing.
- Produces: `pub fn parse_glyph_list`, `pub fn parse_glyph_sequence`, and `pub enum GlyphListError`, re-exported from `mtc_core::formats`.

**Two entry points, not one** (found in Task 7, where `--cells "0='1','1','1'"`
was rejected as a duplicate glyph): the notation is shared but the *semantics*
are not. An **alphabet** is a set — glyphs unique, at most 127. A run of
**cells** is a sequence — repeats are ordinary, and the tape may be far longer
than 127. `parse_glyph_list` is the alphabet form and layers uniqueness and the
cap over `parse_glyph_sequence`, which is the bare notation. `--alphabet` uses
the former, `--cells` the latter. The drift guard stays on `parse_glyph_list`,
since it is `.tmc`'s `alphabet { … }` that must agree.

The notation mirrors `.tmc` alphabet elements exactly (spec R4). Elements are comma-separated; each is a glyph literal `'x'` (with `\'` and `\\` escapes, no others), a bare decimal number, or an inclusive `lo..hi` range whose endpoints are the same kind. A number's label is the decimal string of its **value**, so `05` and `5` both label `"5"`. Glyph ranges require single-scalar endpoints and walk code points. Duplicates are rejected; so is an empty list or one exceeding 127 glyphs.

- [ ] **Step 1: Write the failing tests**

Create `crates/core/src/formats/glyphs.rs` with only the test module for now:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_quoted_glyph_list() {
        assert_eq!(
            parse_glyph_list("' ','s','b','k','1'").unwrap(),
            vec![" ", "s", "b", "k", "1"]
        );
    }

    #[test]
    fn tolerates_whitespace_around_elements() {
        assert_eq!(parse_glyph_list(" ' ' , 's' ").unwrap(), vec![" ", "s"]);
    }

    #[test]
    fn expands_a_glyph_range_by_scalar_succession() {
        assert_eq!(
            parse_glyph_list("'0'..'4'").unwrap(),
            vec!["0", "1", "2", "3", "4"]
        );
    }

    #[test]
    fn expands_a_numeric_range_into_decimal_labels() {
        assert_eq!(parse_glyph_list("0..3").unwrap(), vec!["0", "1", "2", "3"]);
    }

    #[test]
    fn a_numeric_label_is_its_value_not_its_spelling() {
        assert_eq!(parse_glyph_list("05").unwrap(), vec!["5"]);
    }

    #[test]
    fn decodes_the_two_legal_escapes() {
        assert_eq!(parse_glyph_list(r"'\'','\\'").unwrap(), vec!["'", r"\"]);
    }

    #[test]
    fn accepts_a_multi_character_glyph() {
        assert_eq!(parse_glyph_list("'ab','c'").unwrap(), vec!["ab", "c"]);
    }

    #[test]
    fn rejects_an_empty_list() {
        assert!(matches!(parse_glyph_list(""), Err(GlyphListError::Empty)));
        assert!(matches!(parse_glyph_list("   "), Err(GlyphListError::Empty)));
    }

    #[test]
    fn rejects_a_duplicate_glyph() {
        assert!(matches!(
            parse_glyph_list("'a','a'"),
            Err(GlyphListError::Duplicate(g)) if g == "a"
        ));
    }

    #[test]
    fn rejects_an_unterminated_literal() {
        assert!(matches!(
            parse_glyph_list("'a"),
            Err(GlyphListError::UnterminatedLiteral)
        ));
    }

    #[test]
    fn rejects_an_invalid_escape() {
        assert!(matches!(
            parse_glyph_list(r"'\n'"),
            Err(GlyphListError::InvalidEscape('n'))
        ));
    }

    #[test]
    fn rejects_a_descending_range() {
        assert!(matches!(
            parse_glyph_list("'9'..'0'"),
            Err(GlyphListError::RangeDescending)
        ));
        assert!(matches!(
            parse_glyph_list("9..0"),
            Err(GlyphListError::RangeDescending)
        ));
    }

    #[test]
    fn rejects_mixed_kind_range_endpoints() {
        assert!(matches!(
            parse_glyph_list("'a'..3"),
            Err(GlyphListError::RangeKindMismatch)
        ));
    }

    #[test]
    fn rejects_a_multi_scalar_glyph_range_endpoint() {
        assert!(matches!(
            parse_glyph_list("'ab'..'z'"),
            Err(GlyphListError::RangeEndpointNotScalar)
        ));
    }

    #[test]
    fn rejects_more_than_127_glyphs() {
        assert!(matches!(
            parse_glyph_list("0..127"),
            Err(GlyphListError::TooMany(128))
        ));
    }

    #[test]
    fn rejects_a_trailing_or_empty_element() {
        assert!(matches!(
            parse_glyph_list("'a',"),
            Err(GlyphListError::ExpectedElement)
        ));
        assert!(matches!(
            parse_glyph_list("'a',,'b'"),
            Err(GlyphListError::ExpectedElement)
        ));
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p mtc-core --lib formats::glyphs`
Expected: FAIL to compile — `parse_glyph_list` and `GlyphListError` are not defined.

- [ ] **Step 3: Write the implementation**

Prepend to `crates/core/src/formats/glyphs.rs`, above the test module:

```rust
//! Glyph-list notation — the surface both tape-block CLIs use to name a
//! tape's symbols. Deliberately identical to the alphabet-element syntax of
//! the architecture source languages, so a list copy-pastes out of a program
//! and inclusive ranges come along for free (docs/formats.md (glyph tables)).
//!
//! Arch-agnostic by contract: a glyph list is presentation data attached to a
//! tape block, and this module knows nothing about any architecture.

use std::collections::HashSet;
use std::fmt;

/// The most glyphs one tape may distinguish. The compact symbol family caps
/// an alphabet at 127 (docs/formats.md (the compact symbol family)).
const MAX_GLYPHS: usize = 127;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GlyphListError {
    Empty,
    ExpectedElement,
    UnterminatedLiteral,
    InvalidEscape(char),
    BadNumber(String),
    RangeKindMismatch,
    RangeDescending,
    RangeEndpointNotScalar,
    Duplicate(String),
    TooMany(usize),
}

impl fmt::Display for GlyphListError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => write!(f, "empty glyph list"),
            Self::ExpectedElement => write!(f, "expected a glyph or number"),
            Self::UnterminatedLiteral => write!(f, "unterminated glyph literal"),
            Self::InvalidEscape(c) => write!(
                f,
                "invalid escape `\\{c}` in glyph literal — only `\\'` and `\\\\` are allowed"
            ),
            Self::BadNumber(t) => write!(f, "bad number `{t}`"),
            Self::RangeKindMismatch => write!(f, "range endpoints must be the same kind"),
            Self::RangeDescending => write!(f, "range endpoints must ascend"),
            Self::RangeEndpointNotScalar => {
                write!(f, "a glyph range endpoint must be a single character")
            }
            Self::Duplicate(g) => write!(f, "duplicate glyph `{g}`"),
            Self::TooMany(n) => write!(f, "{n} glyphs: at most {MAX_GLYPHS} are allowed"),
        }
    }
}

/// One parsed element, before range expansion.
enum Lit {
    Glyph(String),
    Number(u32),
}

impl Lit {
    /// The label this literal contributes. A number's identity is its VALUE,
    /// so `05` and `5` both label `"5"`.
    fn label(&self) -> String {
        match self {
            Self::Glyph(v) => v.clone(),
            Self::Number(v) => v.to_string(),
        }
    }
}

/// Parse a comma-separated glyph list into its glyphs in position order.
/// Index 0 is the blank by convention; this parser imposes no meaning on it.
pub fn parse_glyph_list(text: &str) -> Result<Vec<String>, GlyphListError> {
    let chars: Vec<char> = text.chars().collect();
    let mut at = 0usize;
    let mut glyphs: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();

    skip_spaces(&chars, &mut at);
    if at == chars.len() {
        return Err(GlyphListError::Empty);
    }

    loop {
        let lo = parse_lit(&chars, &mut at)?;
        skip_spaces(&chars, &mut at);

        let labels = if chars.get(at) == Some(&'.') && chars.get(at + 1) == Some(&'.') {
            at += 2;
            skip_spaces(&chars, &mut at);
            let hi = parse_lit(&chars, &mut at)?;
            expand_range(&lo, &hi)?
        } else {
            vec![lo.label()]
        };

        for label in labels {
            if !seen.insert(label.clone()) {
                return Err(GlyphListError::Duplicate(label));
            }
            glyphs.push(label);
        }

        skip_spaces(&chars, &mut at);
        match chars.get(at) {
            Some(',') => {
                at += 1;
                skip_spaces(&chars, &mut at);
            }
            Some(_) => return Err(GlyphListError::ExpectedElement),
            None => break,
        }
    }

    if glyphs.len() > MAX_GLYPHS {
        return Err(GlyphListError::TooMany(glyphs.len()));
    }
    Ok(glyphs)
}

fn skip_spaces(chars: &[char], at: &mut usize) {
    while matches!(chars.get(*at), Some(c) if c.is_whitespace()) {
        *at += 1;
    }
}

fn parse_lit(chars: &[char], at: &mut usize) -> Result<Lit, GlyphListError> {
    match chars.get(*at) {
        Some('\'') => {
            *at += 1;
            let mut value = String::new();
            loop {
                match chars.get(*at) {
                    None => return Err(GlyphListError::UnterminatedLiteral),
                    Some('\'') => {
                        *at += 1;
                        return Ok(Lit::Glyph(value));
                    }
                    Some('\\') => {
                        *at += 1;
                        match chars.get(*at) {
                            Some('\'') => value.push('\''),
                            Some('\\') => value.push('\\'),
                            None => return Err(GlyphListError::UnterminatedLiteral),
                            Some(bad) => return Err(GlyphListError::InvalidEscape(*bad)),
                        }
                        *at += 1;
                    }
                    Some(c) => {
                        value.push(*c);
                        *at += 1;
                    }
                }
            }
        }
        Some(c) if c.is_ascii_digit() => {
            let start = *at;
            while matches!(chars.get(*at), Some(c) if c.is_ascii_digit()) {
                *at += 1;
            }
            let text: String = chars[start..*at].iter().collect();
            text.parse::<u32>()
                .map(Lit::Number)
                .map_err(|_| GlyphListError::BadNumber(text))
        }
        _ => Err(GlyphListError::ExpectedElement),
    }
}

/// Inclusive, ascending, same-kind. Glyph ranges walk Unicode scalar
/// succession, skipping the surrogate gap (never a valid `char`); numeric
/// ranges mint each value's decimal string.
fn expand_range(lo: &Lit, hi: &Lit) -> Result<Vec<String>, GlyphListError> {
    match (lo, hi) {
        (Lit::Number(l), Lit::Number(h)) => {
            if l > h {
                return Err(GlyphListError::RangeDescending);
            }
            Ok((*l..=*h).map(|v| v.to_string()).collect())
        }
        (Lit::Glyph(l), Lit::Glyph(h)) => {
            let (Some(lc), Some(hc)) = (single_scalar(l), single_scalar(h)) else {
                return Err(GlyphListError::RangeEndpointNotScalar);
            };
            if lc as u32 > hc as u32 {
                return Err(GlyphListError::RangeDescending);
            }
            Ok((lc as u32..=hc as u32)
                .filter_map(char::from_u32)
                .map(|c| c.to_string())
                .collect())
        }
        _ => Err(GlyphListError::RangeKindMismatch),
    }
}

fn single_scalar(s: &str) -> Option<char> {
    let mut it = s.chars();
    match (it.next(), it.next()) {
        (Some(c), None) => Some(c),
        _ => None,
    }
}
```

Then register the module in `crates/core/src/formats/mod.rs` — add alongside the existing `mod` declarations and re-exports:

```rust
pub mod glyphs;
pub use glyphs::{GlyphListError, parse_glyph_list};
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p mtc-core --lib formats::glyphs`
Expected: PASS, 16 tests.

- [ ] **Step 5: Run the quality gates**

```bash
cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --check
```
Expected: clean.

- [ ] **Step 6: Commit**

```bash
git add crates/core/src/formats/glyphs.rs crates/core/src/formats/mod.rs
git commit -m "feat(core): glyph-list notation parser for tape-block authoring"
```

---

### Task 2: Pin the notation to `.tmc` with a drift guard

**Files:**
- Modify: `crates/turing-machine/src/compiler.rs`
- Create: `crates/turing-machine/tests/glyph_notation_parity.rs`

**Interfaces:**
- Consumes: `mtc_core::formats::parse_glyph_list` (Task 1).
- Produces: `pub struct TapeLayout { pub name: String, pub glyphs: Vec<String> }` and `pub fn machine_tape_layout(source: &str) -> Result<Option<Vec<TapeLayout>>, CompileError>` in `mtc_turing_machine::compiler`. Tasks 8 and 9 consume both.

**Why `Option` and not a new error kind** (decided during execution): "this source has no `machine` block" is not a compile error — a library compiles fine, and only the tape-block CLI needs a band. Adding a `CompileErrorKind` variant would require a `code_registry!` entry *and* a row in `docs/tmt/cli.md`'s **published** compile-error catalog, which `tests/error_code_docs.rs` set-compares. Publishing a user-facing compile code for a tooling precondition is the wrong trade. `Ok(None)` says it, and the CLI renders its own message.

Two jobs in one task because they share the accessor: expose per-tape glyphs from `.tmc` source, and use that same surface to prove core's parser agrees with the language.

- [ ] **Step 1: Write the failing tests**

Create `crates/turing-machine/tests/glyph_notation_parity.rs`:

```rust
//! Core's glyph-list parser and the `.tmc` alphabet resolver must accept the
//! same notation and produce the same glyphs. The CLI's `--alphabet` uses the
//! former; a program's `alphabet { … }` uses the latter. They are separate
//! implementations, so this pins them together.

use mtc_core::formats::parse_glyph_list;
use mtc_turing_machine::compiler::machine_tape_layout;

/// Every case is a body legal inside `alphabet name { … }`.
const CORPUS: &[&str] = &[
    "' ','s','b','k','1'",
    "' ','1'",
    "'0'..'9'",
    "0..7",
    "' ','a'..'e','z'",
    "'ab','c'",
    r"' ','\'','\\'",
    "05,'x'",
    "' ','0'..'3',9",
];

fn glyphs_via_tmc(body: &str) -> Vec<String> {
    let source = format!(
        "alphabet probe {{ {body} }}\nmachine {{ tape t: probe;\n  entry state s {{ [*] -> stop; }}\n}}\n"
    );
    let layout = machine_tape_layout(&source)
        .unwrap_or_else(|e| panic!("`{body}` did not compile through .tmc: {e:?}"));
    layout
        .into_iter()
        .next()
        .expect("the machine declares one tape")
        .glyphs
}

#[test]
fn core_parser_agrees_with_the_tmc_resolver_on_every_corpus_entry() {
    for body in CORPUS {
        let via_core = parse_glyph_list(body)
            .unwrap_or_else(|e| panic!("`{body}` rejected by core's parser: {e}"));
        let via_tmc = glyphs_via_tmc(body);
        assert_eq!(
            via_core, via_tmc,
            "glyph notation drifted between core and .tmc for `{body}`"
        );
    }
}

/// The inverse direction: what one rejects, the other must reject too.
#[test]
fn both_reject_the_same_malformed_lists() {
    const BAD: &[&str] = &["'a','a'", "'9'..'0'", "'ab'..'z'", "'a'..3", "'a"];
    for body in BAD {
        assert!(
            parse_glyph_list(body).is_err(),
            "core's parser accepted malformed `{body}`"
        );
        let source = format!(
            "alphabet probe {{ {body} }}\nmachine {{ tape t: probe;\n  entry state s {{ [*] -> stop; }}\n}}\n"
        );
        assert!(
            machine_tape_layout(&source).is_err(),
            ".tmc accepted malformed `{body}`"
        );
    }
}

#[test]
fn tape_layout_reports_names_and_glyphs_in_declaration_order() {
    let source = "\
alphabet mainAlpha { ' ', 's', 'b', 'k', '1' }
alphabet workAlpha { ' ', '1' }

machine {
  tape main: mainAlpha;
  tape cnt:  workAlpha;

  entry state s { [*, *] -> stop; }
}
";
    let layout = machine_tape_layout(source).expect("compiles");
    let names: Vec<&str> = layout.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(names, vec!["main", "cnt"]);
    assert_eq!(layout[0].glyphs, vec![" ", "s", "b", "k", "1"]);
    assert_eq!(layout[1].glyphs, vec![" ", "1"]);
}

#[test]
fn a_source_with_no_machine_block_is_an_error() {
    let source = "alphabet a { ' ', '1' }\nroutine r(tape t: a) { entry state s { [*] -> stop; } }\n";
    assert!(machine_tape_layout(source).is_err());
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p mtc-turing-machine --test glyph_notation_parity`
Expected: FAIL to compile — `machine_tape_layout` does not exist.

- [ ] **Step 3: Add the accessor**

In `crates/turing-machine/src/compiler.rs`, add near `CompileOutput`:

```rust
/// One tape of the `machine` block: its source name and the glyphs of the
/// alphabet it draws from, in position order. The tape-block CLI reads this
/// to mint a block whose bands carry a program's real glyphs rather than
/// index labels (docs/tmt/cli.md (tape-block provenance)).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TapeLayout {
    pub name: String,
    pub glyphs: Vec<String>,
}

/// Resolve `source` far enough to report the `machine` block's tape table,
/// in vector-position order. Analysis only — no expansion, lowering,
/// optimization, or codegen runs, so a program that fully compiles is not
/// required, only one that resolves.
///
/// `Ok(None)` means the source declares no `machine` block: a library takes
/// its tapes from each routine's signature, so there is no single band to
/// describe. That is a legitimate source, not a compile error, so the caller
/// decides whether it can proceed (docs/tmt/cli.md (tape-block provenance)).
pub fn machine_tape_layout(source: &str) -> Result<Option<Vec<TapeLayout>>, CompileError> {
    let analysis = analyze(source)?;
    let resolved = &analysis.resolved;
    let Some(index) = resolved.entry_world else {
        return Ok(None);
    };
    let layout = resolved.worlds[index]
        .tapes
        .iter()
        .map(|tape| {
            let alphabet = resolved
                .alphabets
                .get(&tape.alphabet)
                .expect("resolution guarantees every tape's alphabet exists");
            TapeLayout {
                name: tape.name.clone(),
                glyphs: alphabet.glyphs.clone(),
            }
        })
        .collect();
    Ok(Some(layout))
}
```

No `CompileErrorKind` variant is added, so neither `code_registry!` nor
`docs/tmt/cli.md`'s published error-code inventory moves.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p mtc-turing-machine --test glyph_notation_parity`
Expected: PASS, 4 tests. If a corpus entry disagrees, **fix core's parser to match `.tmc`** — the language is the reference, not the other way round.

- [ ] **Step 5: Run the full suite and the gates**

```bash
cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --check
```
Expected: clean.

- [ ] **Step 6: Commit**

```bash
git add crates/turing-machine/src/compiler.rs crates/turing-machine/tests/glyph_notation_parity.rs
git commit -m "feat(turing-machine): expose machine tape layout; pin glyph notation to .tmc"
```

---

### Task 3: Fix PM's per-tape alphabet defect (both sites)

**Files:**
- Create: `crates/post-machine/tests/fixtures/per_tape_alphabet.pmt`
- Modify: `crates/post-machine/src/cli/inspect.rs:303-306`, `crates/post-machine/src/cli/run.rs:270-278`
- Test: `crates/post-machine/tests/cli_programs.rs`

**Interfaces:**
- Consumes: nothing.
- Produces: nothing new — a behaviour fix.

PM ignores per-tape glyph overrides in two places and renders through the block fallback instead (spec §3). The `run` site is the dangerous one: paired with `--save-tape-block` it *persists* the wrong glyphs. Both TM twins are already correct, as is PM's own `tape_set`.

This lands before the rename so it is reviewable as a pure bug fix.

- [ ] **Step 1: Build the fixture**

The fixture must be an MT v2 block whose per-tape override **disagrees** with the block fallback — no existing tool emits that shape, which is exactly why the defect went unseen.

Both shapes were probed against the shipped binary before this plan was written: `InfiniteTape::from_snapshot` loads the fixture without complaint (its two-glyph width matches PM-1's), `pmt tape show` renders `|B|` where `tmt tape show` renders `|y|`, and `pmt run --tape-block` renders through the fallback too. The fixture is known-good; a load failure here means it was rebuilt wrong, not that the shape is unsupported.

```bash
mkdir -p crates/post-machine/tests/fixtures
python3 - <<'PY'
import struct, zlib
def glyphs(gs):
    b = bytes([len(gs)])
    for g in gs:
        e = g.encode(); b += struct.pack('<H', len(e)) + e
    return b
out  = b'MT\x01' + struct.pack('<H', 2) + b'\x00' + struct.pack('<I', 0)
out += glyphs(['A', 'B'])                    # block fallback
out += b'\x01'                               # one tape
out += struct.pack('<q', 0)                  # origin
out += struct.pack('<I', 1) + bytes([1])     # one cell holding index 1
out += struct.pack('<q', 0)                  # head
out += glyphs(['x', 'y'])                    # per-tape override
out  = bytearray(out); out[6:10] = b'\x00' * 4
out[6:10] = struct.pack('<I', zlib.crc32(bytes(out)) & 0xffffffff)
open('crates/post-machine/tests/fixtures/per_tape_alphabet.pmt', 'wb').write(bytes(out))
PY
```

- [ ] **Step 2: Write the failing tests**

Append to `crates/post-machine/tests/cli_programs.rs` (follow the file's existing helper style for invoking `cli::execute`):

```rust
/// A block whose per-tape override disagrees with the block fallback. Cell 0
/// holds index 1, which the override labels `y` and the fallback labels `B`.
/// Both PM sites must resolve the override, as `tape_set` and both TM twins
/// already do.
/// NOTE: the subcommand is still spelled `tape` — Task 5 renames it to
/// `tape-block` and updates these two tests along with every other call site.
const PER_TAPE_FIXTURE: &str = "tests/fixtures/per_tape_alphabet.pmt";

#[test]
fn tape_show_resolves_a_per_tape_alphabet_override() {
    let out = run_cli(&["tape", "show", PER_TAPE_FIXTURE]).expect("show succeeds");
    assert!(
        out.stdout.contains("|y|"),
        "expected the override glyph `y`, got:\n{}",
        out.stdout
    );
    assert!(
        !out.stdout.contains("|B|"),
        "rendered through the block fallback instead of the override:\n{}",
        out.stdout
    );
}

#[test]
fn run_resolves_a_per_tape_alphabet_override_and_save_preserves_it() {
    let dir = tempdir_for_test("per_tape_override");
    let exe = compile_and_link_to(&dir, "stop.pmc", "stop { stop; }");
    let saved = dir.join("saved.pmt");

    let out = run_cli(&[
        "run",
        exe.to_str().unwrap(),
        "--tape-block",
        PER_TAPE_FIXTURE,
        "--save-tape-block",
        saved.to_str().unwrap(),
    ])
    .expect("run succeeds");
    assert!(
        out.stdout.contains("|y|"),
        "run rendered through the block fallback:\n{}",
        out.stdout
    );

    let shown = run_cli(&["tape", "show", saved.to_str().unwrap()]).expect("show succeeds");
    assert!(
        shown.stdout.contains("|y|"),
        "--save-tape-block persisted the wrong glyphs:\n{}",
        shown.stdout
    );
}
```

Use whatever helpers the file already defines for temp dirs and for compiling a
trivial program; if it has none matching `compile_and_link_to`, build the
smallest `.pmx` the file's existing tests build and reuse that path.

- [ ] **Step 3: Run the tests to verify they fail**

Run: `cargo test -p mtc-post-machine --test cli_programs per_tape`
Expected: FAIL — both assertions on `|y|` fail because both sites render `B`.

- [ ] **Step 4: Fix site 1 — `tape_show`**

In `crates/post-machine/src/cli/inspect.rs`, replace the loop body at 303-306:

```rust
    let mut out = format!("alphabet: {:?}\n", block.alphabet);
    for (i, tape) in block.tapes.iter().enumerate() {
        // Each band renders through its own effective alphabet (its override
        // if present, else the block fallback) — a block authored elsewhere
        // may carry per-tape tables even though PM-1's own tools never write
        // them (docs/formats.md (per-tape glyph tables)).
        let effective: &[String] = tape.alphabet.as_deref().unwrap_or(&block.alphabet);
        out.push_str(&format!("tape {i}: {}", render_tape(tape, effective)));
    }
```

- [ ] **Step 5: Fix site 2 — `initial_tape`**

In `crates/post-machine/src/cli/run.rs`, replace the block-loading arm at 270-278:

```rust
    if let Some(path) = block {
        let bytes = fs::read(path).map_err(|e| format!("cannot read {path}: {e}"))?;
        let file = TapeBlockFile::from_bytes(&bytes).map_err(|e| format!("{path}: {e}"))?;
        let [snapshot] = file.tapes.as_slice() else {
            return Err(format!("{path}: PM-1 blocks hold exactly one tape"));
        };
        let tape = InfiniteTape::from_snapshot(snapshot).map_err(|e| format!("{path}: {e:?}"))?;
        // The band's own glyph table wins over the block fallback; falling
        // back unconditionally would render — and, through --save-tape-block,
        // rewrite — a foreign block with the wrong glyphs.
        let effective = snapshot
            .alphabet
            .clone()
            .unwrap_or_else(|| file.alphabet.clone());
        return Ok((tape, effective));
    }
```

- [ ] **Step 6: Run the tests to verify they pass**

Run: `cargo test -p mtc-post-machine --test cli_programs per_tape`
Expected: PASS.

- [ ] **Step 7: Verify no golden moved**

```bash
cargo test --workspace && git status --short crates/post-machine/tests/golden crates/turing-machine/tests/golden
```
Expected: all tests pass; `git status` prints nothing for either golden directory.

- [ ] **Step 8: Commit**

```bash
git add crates/post-machine/src/cli/inspect.rs crates/post-machine/src/cli/run.rs \
        crates/post-machine/tests/fixtures/per_tape_alphabet.pmt \
        crates/post-machine/tests/cli_programs.rs
git commit -m "fix(post-machine): resolve per-tape glyph overrides in tape show and run"
```

---

### Task 4: Adaptive cell delimiting in `render_tape`

**Files:**
- Modify: `crates/turing-machine/src/cli/mod.rs:104-132`, `crates/post-machine/src/cli/mod.rs:94-122`
- Test: the `render_tape_draws_a_single_bordered_span_with_a_caret` test in each file's `mod tests`

**Interfaces:**
- Consumes: nothing.
- Produces: `pub(crate) enum Delimit { Auto, Dense, Separated }` and `pub(crate) fn render_tape(snapshot: &TapeSnapshot, alphabet: &[String], delimit: Delimit) -> String` in **both** `cli/mod.rs`. Tasks 9, 10, 11 and 12 pass `Delimit`.

Spec R8: dense when every glyph in the effective alphabet is a single character (ambiguity is impossible), separated when any is longer. The two crates keep separate copies, as they already do.

**Scope moved here during execution:** the `--dense` / `--separated` flags on
`tape show` were originally Task 9's and Task 10's. Splitting the enum from its
only non-test consumer made `Dense` and `Separated` dead code, which
`-D warnings` rejects — and a temporary `#[allow(dead_code)]` would have been a
lie that outlived its reason. Wiring the flags here makes the variants honestly
live in the same commit that introduces them. Tasks 9 and 10 therefore only add
the per-band alphabet line to `show`.

- [ ] **Step 1: Write the failing tests**

In **each** crate's `cli/mod.rs` test module, keep the existing test (updating its call to pass `Delimit::Auto`) and add:

```rust
    #[test]
    fn render_tape_stays_dense_for_single_character_alphabets() {
        let snapshot = TapeSnapshot {
            origin: 0,
            cells: vec![1, 0, 1],
            head: 1,
            alphabet: None,
        };
        let alphabet = vec!["_".to_string(), "*".to_string()];
        let text = render_tape(&snapshot, &alphabet, Delimit::Auto);
        assert!(text.contains("|*_*|"), "got:\n{text}");
    }

    #[test]
    fn render_tape_separates_when_a_glyph_is_multi_character() {
        let snapshot = TapeSnapshot {
            origin: 0,
            cells: vec![0, 1, 1],
            head: 0,
            alphabet: None,
        };
        let alphabet = vec!["0".to_string(), "11".to_string()];
        let text = render_tape(&snapshot, &alphabet, Delimit::Auto);
        assert!(text.contains("|0|11|11|"), "got:\n{text}");
    }

    #[test]
    fn render_tape_honours_forced_modes() {
        let snapshot = TapeSnapshot {
            origin: 0,
            cells: vec![0, 1],
            head: 0,
            alphabet: None,
        };
        let single = vec!["a".to_string(), "b".to_string()];
        assert!(render_tape(&snapshot, &single, Delimit::Separated).contains("|a|b|"));

        let multi = vec!["0".to_string(), "11".to_string()];
        assert!(render_tape(&snapshot, &multi, Delimit::Dense).contains("|011|"));
    }

    #[test]
    fn the_caret_tracks_the_head_through_separators() {
        let snapshot = TapeSnapshot {
            origin: 0,
            cells: vec![0, 1],
            head: 1,
            alphabet: None,
        };
        let alphabet = vec!["0".to_string(), "11".to_string()];
        let text = render_tape(&snapshot, &alphabet, Delimit::Auto);
        let mut lines = text.lines().skip(1); // past the "origin …, head …" line
        let cells = lines.next().unwrap();
        let caret = lines.next().unwrap();
        // Carets sit under the head cell's glyph, not under a separator.
        let start = cells.find("11").unwrap();
        assert_eq!(caret.trim_end().len(), start + 2, "cells: {cells}\ncaret: {caret}");
        assert!(caret.trim_start().chars().all(|c| c == '^'));
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p mtc-turing-machine --lib cli::tests::render_tape && cargo test -p mtc-post-machine --lib cli::tests::render_tape`
Expected: FAIL to compile — `Delimit` does not exist and `render_tape` takes two arguments.

- [ ] **Step 3: Implement in both crates**

Replace `render_tape` in **each** `cli/mod.rs` (the bodies are identical; only the surrounding module differs):

```rust
/// Cell-delimiting policy for [`render_tape`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Delimit {
    /// Dense when every glyph is one character, separated otherwise.
    Auto,
    /// Never separate. Ambiguous with multi-character glyphs, by request.
    Dense,
    /// Always separate.
    Separated,
}

impl Delimit {
    /// Resolve `Auto` against the alphabet actually in play. A single-character
    /// alphabet can never be ambiguous, so it stays dense and readable
    /// (docs/tmt/cli.md (tape-block show)).
    fn separates(self, alphabet: &[String]) -> bool {
        match self {
            Self::Dense => false,
            Self::Separated => true,
            Self::Auto => alphabet.iter().any(|g| g.chars().count() != 1),
        }
    }
}

/// Render one tape with its glyphs: the dense span line plus a caret line
/// under the head. Glyph 0 is blank by convention.
pub(crate) fn render_tape(
    snapshot: &TapeSnapshot,
    alphabet: &[String],
    delimit: Delimit,
) -> String {
    let separated = delimit.separates(alphabet);
    let glyph = |index: u8| -> &str {
        alphabet
            .get(usize::from(index))
            .map(String::as_str)
            .unwrap_or("?")
    };
    let mut cells_line = String::new();
    let mut caret_line = String::new();
    for (i, &cell) in snapshot.cells.iter().enumerate() {
        if separated && i > 0 {
            cells_line.push('|');
            caret_line.push(' ');
        }
        let g = glyph(cell);
        let here = snapshot.origin + i as i64 == snapshot.head;
        cells_line.push_str(g);
        let width = g.chars().count().max(1);
        caret_line.push_str(&if here { "^" } else { " " }.repeat(width));
    }
    format!(
        "origin {}, head {}\n|{}|\n {}\n",
        snapshot.origin,
        snapshot.head,
        cells_line,
        caret_line.trim_end()
    )
}
```

Then update every existing call site to pass `Delimit::Auto`:
`turing-machine/src/cli/inspect.rs` (`tape_show`), `turing-machine/src/cli/run.rs:205`,
`post-machine/src/cli/inspect.rs` (`tape_show`), `post-machine/src/cli/run.rs:204`.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test --workspace`
Expected: PASS. If a `cli_docs` or golden test fails, a quoted transcript in
`docs/` needs its rendering updated — do that in Task 13, and for now confirm
the failure is only a doc-transcript mismatch, not a golden byte change.

- [ ] **Step 5: Commit**

```bash
git add crates/turing-machine/src/cli crates/post-machine/src/cli
git commit -m "feat(cli): adaptive cell delimiting in tape rendering"
```

---

### Task 5: Rename `tape` to `tape-block` and realign the run flags

**Files:**
- Modify: `crates/{post-machine,turing-machine}/src/cli/mod.rs` (dispatch + `USAGE`)
- Modify: `crates/{post-machine,turing-machine}/src/cli/inspect.rs` (`TAPE_USAGE`, fn names)
- Modify: `crates/{post-machine,turing-machine}/src/cli/run.rs` (flag names, `RUN_USAGE`)
- Modify: `crates/{post-machine,turing-machine}/src/completions/registry.rs`, `completions/zsh.rs`
- Modify: `crates/post-machine/tests/completions_registry.rs` (`EXPECTED_TOP_LEVEL`)
- Modify: `docs/pmt/cli.md`, `docs/tmt/cli.md` (the `--help` quotes the `cli_docs` guards pin)

**Interfaces:**
- Consumes: nothing.
- Produces: the subcommand `tape-block` on both CLIs; `pmt run --tape-cells`; `tmt run --tape-block`.

Purely mechanical, done as its own task so the tree stays green and every later task builds on the final names. Spec R1: hard rename, **no aliases**.

- [ ] **Step 1: Rename the subcommand**

In both `cli/mod.rs`, change the dispatch arm:

```rust
        Some("tape-block") => inspect::tape_block(&args[1..]),
```

and update the subcommand line in each `USAGE` constant:

```
  tape-block   new/set/show .tmt tape-block snapshots
```

(PM's reads `build/new/set/show .pmt tape-block snapshots`.)

In both `cli/inspect.rs`, rename `pub(super) fn tape` to `pub(super) fn tape_block`, and update every `USAGE:` line and error message inside `TAPE_USAGE` from `tape ` to `tape-block ` — including the `tape new takes no positional arguments`, `tape set takes exactly one file`, `tape show takes exactly one file`, `tape set: -o and --in-place are mutually exclusive`, and `tape set needs -o …` strings.

- [ ] **Step 2: Realign the run flags**

In `crates/post-machine/src/cli/run.rs`, rename the inline-literal flag from `--tape` to `--tape-cells` at its `args.value(...)` call and in `RUN_USAGE`. Update the mutual-exclusion message:

```rust
        return Err("--tape-block and --tape-cells are mutually exclusive".into());
```

In `crates/turing-machine/src/cli/run.rs`, rename `--tape` to `--tape-block` at its `args.value(...)` call, in `RUN_USAGE`, and in the missing-flag error:

```rust
        return Err(format!("run needs --tape-block TAPES.tmt\n\n{RUN_USAGE}"));
```

- [ ] **Step 3: Update the completions registries**

In both `completions/registry.rs`: change every `strings(&["tape", …])` to `strings(&["tape-block", …])`, and the two description match arms from `"tape"` / `(Some("tape"), Some(…))` to `"tape-block"` / `(Some("tape-block"), Some(…))`. In TM's `run_spec`, rename the `--tape` flag to `--tape-block`. In PM's `run_spec`, rename the inline `--tape` flag to `--tape-cells`.

In both `completions/zsh.rs`, update the nested-path special-cases (PM at `:168` and `:399`) from `"tape"` to `"tape-block"`.

In `crates/post-machine/tests/completions_registry.rs`, change `"tape"` to `"tape-block"` in `EXPECTED_TOP_LEVEL`.

In `crates/post-machine/tests/cli_programs.rs`, update Task 3's two tests
(`tape_show_resolves_a_per_tape_alphabet_override` and
`run_resolves_a_per_tape_alphabet_override_and_save_preserves_it`) from `"tape"`
to `"tape-block"`. Grep the whole tree for stragglers:

```bash
rg --fixed-strings '"tape"' crates/ && rg --fixed-strings '"--tape"' crates/
```
Expected: no hits.

- [ ] **Step 4: Update the `--help` quotes the doc guards pin**

Run the tools and paste their real output into the docs:

```bash
cargo build --release
./target/release/tmt --help
./target/release/pmt --help
./target/release/tmt tape-block
./target/release/pmt tape-block
./target/release/tmt run --help
./target/release/pmt run --help
```

Update the corresponding fenced blocks in `docs/tmt/cli.md` and `docs/pmt/cli.md` so each quote matches verbatim.

- [ ] **Step 5: Run the full suite**

Run: `cargo test --workspace`
Expected: PASS. The completions drift guards and `cli_docs` guards are the ones that catch a missed rename — read any failure as a site still spelled `tape`.

- [ ] **Step 6: Commit**

```bash
git add crates docs
git commit -m "feat(cli)!: rename tape to tape-block and realign the run tape flags"
```

---

### Task 6: Keyed edit-flag parsing

**Files:**
- Modify: `crates/{post-machine,turing-machine}/src/cli/mod.rs`

**Interfaces:**
- Consumes: nothing.
- Produces: `pub(crate) fn parse_keyed(flag: &str, values: &[String]) -> Result<Vec<(String, String)>, String>` in **both** `cli/mod.rs`. Tasks 7–11 consume it.

Splits `KEY=VALUE` and rejects a repeated key for the same flag (spec §4.1: repeating a flag for one tape is an error, not last-wins).

**This task does not commit on its own** (established during execution, same
cause as Task 4's): a `pub(crate)` helper whose only callers are `#[cfg(test)]`
is dead code under `-D warnings`. `parse_keyed` therefore ships **with its first
real consumer in each crate** — the TM copy lands in Task 7, the PM copy in
Task 10. Write both copies from this task's code; just do not add PM's until
Task 10, or the workspace gate fails for the intervening tasks.

- [ ] **Step 1: Write the failing tests**

Add to **each** crate's `cli/mod.rs` test module:

```rust
    fn owned(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn parse_keyed_splits_at_the_first_equals() {
        let got = parse_keyed("--cells", &owned(&["0='a','b'", "main='c'"])).unwrap();
        assert_eq!(
            got,
            vec![
                ("0".to_string(), "'a','b'".to_string()),
                ("main".to_string(), "'c'".to_string()),
            ]
        );
    }

    #[test]
    fn parse_keyed_allows_equals_inside_the_value() {
        let got = parse_keyed("--cells", &owned(&["0='='"])).unwrap();
        assert_eq!(got, vec![("0".to_string(), "'='".to_string())]);
    }

    #[test]
    fn parse_keyed_allows_an_empty_value() {
        let got = parse_keyed("--cells", &owned(&["1="])).unwrap();
        assert_eq!(got, vec![("1".to_string(), String::new())]);
    }

    #[test]
    fn parse_keyed_rejects_a_missing_equals() {
        let err = parse_keyed("--cells", &owned(&["0"])).unwrap_err();
        assert!(err.contains("--cells"), "got: {err}");
        assert!(err.contains("KEY=") , "got: {err}");
    }

    #[test]
    fn parse_keyed_rejects_an_empty_key() {
        assert!(parse_keyed("--cells", &owned(&["='a'"])).is_err());
    }

    #[test]
    fn parse_keyed_rejects_a_repeated_key() {
        let err = parse_keyed("--cells", &owned(&["0='a'", "0='b'"])).unwrap_err();
        assert!(err.contains("twice") || err.contains("repeated"), "got: {err}");
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p mtc-turing-machine --lib cli::tests::parse_keyed && cargo test -p mtc-post-machine --lib cli::tests::parse_keyed`
Expected: FAIL to compile — `parse_keyed` is not defined.

- [ ] **Step 3: Implement in both crates**

Add to **each** `cli/mod.rs`:

```rust
/// Split repeatable `KEY=VALUE` edit flags into pairs, preserving order.
/// The key is everything before the FIRST `=`, so a value may contain `=`.
/// A key repeated within one flag is an error rather than last-wins: silently
/// dropping an edit the author wrote is worse than making them look
/// (docs/tmt/cli.md (tape-block edit flags)).
pub(crate) fn parse_keyed(flag: &str, values: &[String]) -> Result<Vec<(String, String)>, String> {
    let mut out: Vec<(String, String)> = Vec::new();
    for raw in values {
        let Some((key, value)) = raw.split_once('=') else {
            return Err(format!("{flag} `{raw}`: expected KEY=VALUE"));
        };
        if key.is_empty() {
            return Err(format!("{flag} `{raw}`: empty tape key"));
        }
        if out.iter().any(|(k, _)| k == key) {
            return Err(format!("{flag}: tape `{key}` given twice"));
        }
        out.push((key.to_string(), value.to_string()));
    }
    Ok(out)
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test --workspace`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/turing-machine/src/cli/mod.rs crates/post-machine/src/cli/mod.rs
git commit -m "feat(cli): keyed edit-flag parsing for tape-block authoring"
```

---

> **Tasks 7–9 land as one commit** (decided during execution). All three
> rewrite functions in the same file — `tape_new`, `from_source_or_image`,
> `tape_set`, `tape_show` — and Task 8 is a single `match` arm inside a
> function Task 7 introduces. Splitting them would mean staging hunks of one
> file rather than whole files, which is worse to review, not better. Each
> task's targeted tests were run green in sequence; the full suite gates the
> combined state.

### Task 7: TM `tape-block new` — keyed edits, image and freehand provenance

**Files:**
- Modify: `crates/turing-machine/src/cli/inspect.rs`
- Test: `crates/turing-machine/tests/cli_programs.rs`

**Interfaces:**
- Consumes: `parse_keyed` (Task 6), `parse_glyph_list` (Task 1), `Delimit` (Task 4).
- Produces: `struct Edits { alphabets, cells, heads, origins }` and `fn collect_edits(args: &mut Args) -> Result<Edits, String>`, plus `fn apply_edits(block: &mut TapeBlockFile, edits: &Edits, names: &[String], pm_block_alphabet: bool) -> Result<(), String>` — both used by Task 8, 9 and 11.

Spec R3 (image + freehand paths), R5 (index keys; names in Task 8), R6 (relabel, same effective cardinality), R7 (one invocation authors a block), §4.1 (`--alphabet` applies before `--cells`).

- [ ] **Step 1: Write the failing tests**

Add to `crates/turing-machine/tests/cli_programs.rs`:

```rust
#[test]
fn tape_block_new_from_an_image_pins_glyphs_and_cells_in_one_call() {
    let dir = tempdir_for_test("tb_new_image");
    let exe = compile_and_link_pow2(&dir); // 3 tapes, cardinalities 5/2/2
    let out_path = dir.join("in.tmt");

    run_cli(&[
        "tape-block", "new",
        "--from", exe.to_str().unwrap(),
        "--alphabet", "0=' ','s','b','k','1'",
        "--alphabet", "1=' ','1'",
        "--alphabet", "2=' ','1'",
        "--cells", "0='s','b','1','1','1','k'",
        "-o", out_path.to_str().unwrap(),
    ])
    .expect("new succeeds");

    let shown = run_cli(&["tape-block", "show", out_path.to_str().unwrap()]).unwrap();
    assert!(shown.stdout.contains("|sb111k|"), "got:\n{}", shown.stdout);
}

#[test]
fn tape_block_new_without_from_sizes_the_block_from_the_alphabet_flags() {
    let dir = tempdir_for_test("tb_new_freehand");
    let out_path = dir.join("in.tmt");

    run_cli(&[
        "tape-block", "new",
        "--alphabet", "0=' ','a'",
        "--alphabet", "1=' ','1'",
        "--cells", "0='a','a'",
        "-o", out_path.to_str().unwrap(),
    ])
    .expect("new succeeds");

    let shown = run_cli(&["tape-block", "show", out_path.to_str().unwrap()]).unwrap();
    assert!(shown.stdout.contains("tape 0"), "got:\n{}", shown.stdout);
    assert!(shown.stdout.contains("tape 1"), "got:\n{}", shown.stdout);
    assert!(!shown.stdout.contains("tape 2"), "got:\n{}", shown.stdout);
    assert!(shown.stdout.contains("|aa|"), "got:\n{}", shown.stdout);
}

#[test]
fn tape_block_new_freehand_rejects_non_contiguous_keys() {
    let dir = tempdir_for_test("tb_new_gap");
    let err = run_cli(&[
        "tape-block", "new",
        "--alphabet", "0=' ','a'",
        "--alphabet", "5=' ','b'",
        "-o", dir.join("x.tmt").to_str().unwrap(),
    ])
    .unwrap_err();
    assert!(err.contains("contiguous"), "got: {err}");
}

#[test]
fn tape_block_new_rejects_an_alphabet_of_the_wrong_cardinality() {
    let dir = tempdir_for_test("tb_new_card");
    let exe = compile_and_link_pow2(&dir);
    let err = run_cli(&[
        "tape-block", "new",
        "--from", exe.to_str().unwrap(),
        "--alphabet", "0=' ','x'", // tape 0 is 5 wide
        "-o", dir.join("x.tmt").to_str().unwrap(),
    ])
    .unwrap_err();
    assert!(err.contains("cardinality 5"), "got: {err}");
    assert!(err.contains("2 glyphs"), "got: {err}");
}

#[test]
fn tape_block_cells_resolve_against_the_alphabet_pinned_in_the_same_call() {
    // `s` exists only in the NEW alphabet, never in the image's decimal labels.
    let dir = tempdir_for_test("tb_new_order");
    let exe = compile_and_link_pow2(&dir);
    run_cli(&[
        "tape-block", "new",
        "--from", exe.to_str().unwrap(),
        "--alphabet", "0=' ','s','b','k','1'",
        "--cells", "0='s'",
        "-o", dir.join("ok.tmt").to_str().unwrap(),
    ])
    .expect("--alphabet must apply before --cells");
}

#[test]
fn tape_block_new_rejects_a_tape_index_past_the_block() {
    let dir = tempdir_for_test("tb_new_range");
    let exe = compile_and_link_pow2(&dir);
    let err = run_cli(&[
        "tape-block", "new",
        "--from", exe.to_str().unwrap(),
        "--cells", "7='1'",
        "-o", dir.join("x.tmt").to_str().unwrap(),
    ])
    .unwrap_err();
    assert!(err.contains("out of range"), "got: {err}");
    assert!(err.contains("3 tape(s)"), "got: {err}");
}
```

Define `compile_and_link_pow2` in the same file if no equivalent exists, using the file's established compile-and-link helper style and this source:

```rust
const POW2_SRC: &str = "\
alphabet mainAlpha { ' ', 's', 'b', 'k', '1' }
alphabet workAlpha { ' ', '1' }

machine {
  tape main: mainAlpha;
  tape cnt:  workAlpha;
  tape tmp:  workAlpha;

  entry state s { ['b', *, *] -> stop; }
}
";
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p mtc-turing-machine --test cli_programs tape_block`
Expected: FAIL — the edit flags are not recognized, so `positionals()` reports `unknown flag`.

- [ ] **Step 3: Implement the edit machinery**

Add to `crates/turing-machine/src/cli/inspect.rs`:

```rust
/// The keyed edits one invocation carries, in flag order.
pub(super) struct Edits {
    pub alphabets: Vec<(String, String)>,
    pub cells: Vec<(String, String)>,
    pub heads: Vec<(String, String)>,
    pub origins: Vec<(String, String)>,
}

impl Edits {
    /// Every tape key mentioned by any edit flag.
    fn keys(&self) -> Vec<&str> {
        let mut out: Vec<&str> = Vec::new();
        for group in [&self.alphabets, &self.cells, &self.heads, &self.origins] {
            for (k, _) in group {
                if !out.contains(&k.as_str()) {
                    out.push(k);
                }
            }
        }
        out
    }
}

pub(super) fn collect_edits(args: &mut Args) -> Result<Edits, String> {
    Ok(Edits {
        alphabets: parse_keyed("--alphabet", &args.values("--alphabet")?)?,
        cells: parse_keyed("--cells", &args.values("--cells")?)?,
        heads: parse_keyed("--head", &args.values("--head")?)?,
        origins: parse_keyed("--origin", &args.values("--origin")?)?,
    })
}

/// Resolve a tape key to its band index. A key is an index, or — when `names`
/// is non-empty, i.e. a source supplied them — a declared tape name
/// (docs/tmt/cli.md (tape-block edit flags)).
fn resolve_key(key: &str, names: &[String], tape_count: usize) -> Result<usize, String> {
    if let Some(i) = names.iter().position(|n| n == key) {
        return Ok(i);
    }
    let Ok(index) = key.parse::<usize>() else {
        return if names.is_empty() {
            Err(format!(
                "tape key `{key}`: expected an index — tape names need `--from` a .tmc source"
            ))
        } else {
            Err(format!(
                "tape key `{key}`: no such tape (declared: {})",
                names.join(", ")
            ))
        };
    };
    if index >= tape_count {
        return Err(format!(
            "tape {index}: out of range (block has {tape_count} tape(s))"
        ));
    }
    Ok(index)
}

/// Apply every edit to `block`. `--alphabet` runs first for each tape so
/// `--cells` in the same invocation resolves against the newly pinned glyphs.
///
/// `pm_block_alphabet` writes a repin to the BLOCK alphabet instead of a
/// per-tape override — PM-1 blocks are single-tape and single-alphabet, and
/// keeping the override unset keeps them at MT v1
/// (docs/formats.md (tape-block snapshot)).
pub(super) fn apply_edits(
    block: &mut TapeBlockFile,
    edits: &Edits,
    names: &[String],
    pm_block_alphabet: bool,
) -> Result<(), String> {
    let tape_count = block.tapes.len();

    for (key, text) in &edits.alphabets {
        let index = resolve_key(key, names, tape_count)?;
        let glyphs = parse_glyph_list(text).map_err(|e| format!("--alphabet `{key}`: {e}"))?;
        let current = block.tapes[index]
            .alphabet
            .as_deref()
            .unwrap_or(&block.alphabet)
            .len();
        // A repin relabels; it never resizes. Measured against the TAPE's
        // effective width, which on a multi-band block differs from the block
        // fallback's (docs/formats.md (per-tape glyph tables)).
        if glyphs.len() != current {
            return Err(format!(
                "--alphabet `{key}`: tape {index} has cardinality {current}, \
                 the given alphabet has {} glyphs",
                glyphs.len()
            ));
        }
        if pm_block_alphabet {
            block.alphabet = glyphs;
            block.tapes[index].alphabet = None;
        } else {
            block.tapes[index].alphabet = Some(glyphs);
        }
    }

    for (key, text) in &edits.cells {
        let index = resolve_key(key, names, tape_count)?;
        let effective: Vec<String> = block.tapes[index]
            .alphabet
            .clone()
            .unwrap_or_else(|| block.alphabet.clone());
        let cells = if text.trim().is_empty() {
            Vec::new()
        } else {
            let glyphs = parse_glyph_list(text).map_err(|e| format!("--cells `{key}`: {e}"))?;
            glyphs
                .iter()
                .map(|g| {
                    effective
                        .iter()
                        .position(|e| e == g)
                        .map(|i| i as u8)
                        .ok_or_else(|| {
                            format!("--cells `{key}`: glyph `{g}` is not in {effective:?}")
                        })
                })
                .collect::<Result<Vec<u8>, String>>()?
        };
        block.tapes[index].cells = cells;
    }

    for (key, text) in &edits.heads {
        let index = resolve_key(key, names, tape_count)?;
        block.tapes[index].head = text
            .parse()
            .map_err(|_| format!("--head `{key}`: bad value `{text}`"))?;
    }

    for (key, text) in &edits.origins {
        let index = resolve_key(key, names, tape_count)?;
        block.tapes[index].origin = text
            .parse()
            .map_err(|_| format!("--origin `{key}`: bad value `{text}`"))?;
    }

    Ok(())
}
```

Import `parse_glyph_list` from `mtc_core::formats` and `parse_keyed` from `super` at the top of the file.

- [ ] **Step 4: Rewrite `tape_new`**

Replace the body of `tape_new` in `crates/turing-machine/src/cli/inspect.rs`:

```rust
/// `tmt tape-block new [--from APP.tmx] [-o OUT.tmt] [EDITS]` — mint a block
/// and apply this invocation's edits to it in one call.
///
/// With `--from` an executable, the band count and each band's cardinality
/// come from the image header, and glyphs default to the decimal labels
/// `0..card-1`. Without `--from`, the `--alphabet` flags define the block:
/// their keys must be contiguous from 0 (docs/tmt/cli.md (tape-block new)).
fn tape_new(raw: &[String]) -> Result<CliOutput, String> {
    let mut args = Args::new(raw);
    let from = args.value("--from")?;
    let out = args.value("-o")?.unwrap_or_else(|| "blank.tmt".into());
    let edits = collect_edits(&mut args)?;
    let extra = args.positionals()?;
    if !extra.is_empty() {
        return Err(format!(
            "tape-block new takes no positional arguments\n\n{TAPE_USAGE}"
        ));
    }

    let (band_glyphs, names): (Vec<Vec<String>>, Vec<String>) = match from.as_deref() {
        Some(path) => from_source_or_image(path)?,
        None => (freehand_bands(&edits)?, Vec::new()),
    };

    let widest = band_glyphs.iter().map(Vec::len).max().unwrap_or(2);
    let mut block = TapeBlockFile {
        // The block-level alphabet is a fallback only (every band overrides
        // it); size it to the widest band so `tape-block show` renders sanely
        // if a band ever drops its override.
        alphabet: (0..widest).map(|i| i.to_string()).collect(),
        tapes: band_glyphs
            .iter()
            .map(|glyphs| TapeSnapshot {
                origin: 0,
                cells: Vec::new(),
                head: 0,
                alphabet: Some(glyphs.clone()),
            })
            .collect(),
    };

    apply_edits(&mut block, &edits, &names, false)?;

    let bytes = block.to_bytes().map_err(|e| format!("{out}: {e}"))?;
    fs::write(&out, bytes).map_err(|e| format!("cannot write {out}: {e}"))?;
    Ok(CliOutput::ok(String::new(), String::new()))
}

/// Without `--from`, the `--alphabet` flags define the block. Their keys must
/// be indices contiguous from 0, so a mistyped key cannot silently inflate the
/// band count (docs/tmt/cli.md (tape-block new)).
fn freehand_bands(edits: &Edits) -> Result<Vec<Vec<String>>, String> {
    let mut bands: Vec<(usize, Vec<String>)> = Vec::new();
    for (key, text) in &edits.alphabets {
        let index = key.parse::<usize>().map_err(|_| {
            format!("--alphabet `{key}`: expected an index — tape names need `--from` a .tmc source")
        })?;
        let glyphs = parse_glyph_list(text).map_err(|e| format!("--alphabet `{key}`: {e}"))?;
        bands.push((index, glyphs));
    }
    bands.sort_by_key(|(index, _)| *index);
    if bands.is_empty() || bands.iter().enumerate().any(|(i, (index, _))| i != *index) {
        return Err(format!(
            "tape-block new without --from needs one --alphabet per tape, \
             keyed contiguously from 0\n\n{TAPE_USAGE}"
        ));
    }
    Ok(bands.into_iter().map(|(_, glyphs)| glyphs).collect())
}

/// `--from` dispatches on the container magic, never on the extension
/// (docs/formats.md (shared conventions)). Returns each band's glyphs and,
/// when the source supplied them, the tape names. Task 8 fills in the
/// non-container arm; until then a `.tmc` path is rejected here.
fn from_source_or_image(path: &str) -> Result<(Vec<Vec<String>>, Vec<String>), String> {
    let bytes = fs::read(path).map_err(|e| format!("cannot read {path}: {e}"))?;
    match sniff(&bytes) {
        Some(ContainerKind::Executable) => {
            let exe = Executable::from_bytes(&bytes).map_err(|e| format!("{path}: {e}"))?;
            let cards: Vec<u32> = if exe.alphabet_cardinalities.is_empty() {
                vec![2; usize::from(exe.tape_count).max(1)]
            } else {
                exe.alphabet_cardinalities.clone()
            };
            // An image carries cardinalities and nothing else, so each band is
            // labelled with the decimal strings `0..card-1` — the author repins
            // them with `--alphabet` (docs/formats.md (glyph tables)).
            let glyphs = cards
                .iter()
                .map(|&c| (0..c).map(|i| i.to_string()).collect())
                .collect();
            Ok((glyphs, Vec::new()))
        }
        Some(_) => Err(format!("{path}: not an executable image (.tmx)")),
        None => Err(format!("{path}: not an executable image (.tmx)")),
    }
}
```

- [ ] **Step 5: Update the usage text**

Replace `TAPE_USAGE` in the same file:

```rust
const TAPE_USAGE: &str = "\
USAGE: tmt tape-block new [--from APP.tmx | --from APP.tmc] [-o OUT.tmt] [EDITS]
       tmt tape-block set IN.tmt (-o OUT.tmt | --in-place) [--from APP.tmc] [EDITS]
       tmt tape-block show FILE.tmt [--dense | --separated]

EDITS (repeatable; KEY is a tape index, or a tape name with --from a .tmc):
  --alphabet KEY=GLYPHS   repin tape KEY's glyphs (relabels; same cardinality)
  --cells    KEY=GLYPHS   set tape KEY's cells
  --head     KEY=N        set tape KEY's head
  --origin   KEY=N        set tape KEY's origin

GLYPHS is alphabet notation: ' ','s','1' or '0'..'9'. --alphabet applies
before --cells, so cells resolve against the glyphs just pinned.
";
```

- [ ] **Step 6: Run the tests to verify they pass**

Run: `cargo test -p mtc-turing-machine --test cli_programs tape_block`
Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add crates/turing-machine/src/cli/inspect.rs crates/turing-machine/tests/cli_programs.rs
git commit -m "feat(turing-machine): whole-block authoring in tmt tape-block new"
```

---

### Task 8: TM `--from APP.tmc` provenance

**Files:**
- Modify: `crates/turing-machine/src/cli/inspect.rs`
- Test: `crates/turing-machine/tests/cli_programs.rs`

**Interfaces:**
- Consumes: `machine_tape_layout` / `TapeLayout` (Task 2), `from_source_or_image` (Task 7).
- Produces: nothing new.

Spec R3: a `.tmc` supplies glyphs *and* names, so no `--alphabet` is needed for the common case and edit keys may be names.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn tape_block_new_from_source_takes_glyphs_and_names_from_the_program() {
    let dir = tempdir_for_test("tb_new_src");
    let src = dir.join("pow2.tmc");
    std::fs::write(&src, POW2_SRC).unwrap();
    let out_path = dir.join("in.tmt");

    run_cli(&[
        "tape-block", "new",
        "--from", src.to_str().unwrap(),
        "--cells", "main='s','b','1','1','1','k'",
        "-o", out_path.to_str().unwrap(),
    ])
    .expect("new from source succeeds");

    let shown = run_cli(&["tape-block", "show", out_path.to_str().unwrap()]).unwrap();
    assert!(shown.stdout.contains("|sb111k|"), "got:\n{}", shown.stdout);
    // Glyphs came from the program, so the band renders real glyphs with no
    // --alphabet flag given at all.
    assert!(shown.stdout.contains("\"s\""), "got:\n{}", shown.stdout);
}

#[test]
fn tape_block_new_from_source_accepts_an_index_key_too() {
    let dir = tempdir_for_test("tb_new_src_ix");
    let src = dir.join("pow2.tmc");
    std::fs::write(&src, POW2_SRC).unwrap();
    run_cli(&[
        "tape-block", "new",
        "--from", src.to_str().unwrap(),
        "--cells", "0='s'",
        "-o", dir.join("in.tmt").to_str().unwrap(),
    ])
    .expect("index keys stay legal on the source path");
}

#[test]
fn tape_block_new_from_source_rejects_an_unknown_tape_name() {
    let dir = tempdir_for_test("tb_new_src_bad");
    let src = dir.join("pow2.tmc");
    std::fs::write(&src, POW2_SRC).unwrap();
    let err = run_cli(&[
        "tape-block", "new",
        "--from", src.to_str().unwrap(),
        "--cells", "nope='s'",
        "-o", dir.join("in.tmt").to_str().unwrap(),
    ])
    .unwrap_err();
    assert!(err.contains("no such tape"), "got: {err}");
    assert!(err.contains("main"), "got: {err}");
}

#[test]
fn a_tape_name_without_a_source_is_a_clear_error() {
    let dir = tempdir_for_test("tb_name_no_src");
    let exe = compile_and_link_pow2(&dir);
    let err = run_cli(&[
        "tape-block", "new",
        "--from", exe.to_str().unwrap(),
        "--cells", "main='s'",
        "-o", dir.join("in.tmt").to_str().unwrap(),
    ])
    .unwrap_err();
    assert!(err.contains(".tmc"), "got: {err}");
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p mtc-turing-machine --test cli_programs from_source`
Expected: FAIL — a `.tmc` path does not sniff as a container, so `from_source_or_image` errors.

- [ ] **Step 3: Add the source arm**

Task 7 left `from_source_or_image` rejecting non-container paths. Replace **only**
its `None` arm — the signature and the executable arm stay exactly as they are:

```rust
        // Not a container: treat it as source text. A `.tmc` supplies both the
        // glyphs and the tape names, so the common case needs no --alphabet
        // at all (docs/tmt/cli.md (tape-block provenance)).
        None => {
            let text = String::from_utf8(bytes)
                .map_err(|_| format!("{path}: not an executable image and not UTF-8 source"))?;
            let layout = crate::compiler::machine_tape_layout(&text)
                .map_err(|e| format!("{path}: {e:?}"))?;
            let glyphs = layout.iter().map(|t| t.glyphs.clone()).collect();
            let names = layout.iter().map(|t| t.name.clone()).collect();
            Ok((glyphs, names))
        }
```

Nothing in `tape_new` changes: it already destructures `(band_glyphs, names)`
and builds each band's override from `band_glyphs`, so the source path lights
up as soon as this arm returns real glyphs and names.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p mtc-turing-machine --test cli_programs`
Expected: PASS, including Task 7's tests — the image path still produces decimal labels.

- [ ] **Step 5: Commit**

```bash
git add crates/turing-machine/src/cli/inspect.rs crates/turing-machine/tests/cli_programs.rs
git commit -m "feat(turing-machine): mint a tape block from .tmc source glyphs and names"
```

---

### Task 9: TM `tape-block set` and `show`

**Files:**
- Modify: `crates/turing-machine/src/cli/inspect.rs`
- Test: `crates/turing-machine/tests/cli_programs.rs`

**Interfaces:**
- Consumes: `collect_edits`, `apply_edits` (Task 7), `from_source_or_image` (Task 8), `Delimit` (Task 4).
- Produces: nothing new.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn tape_block_set_repins_glyphs_without_moving_a_cell() {
    let dir = tempdir_for_test("tb_set_repin");
    let exe = compile_and_link_pow2(&dir);
    let path = dir.join("in.tmt");
    run_cli(&[
        "tape-block", "new",
        "--from", exe.to_str().unwrap(),
        "--cells", "0='1','2','4','4','4','3'", // decimal labels
        "-o", path.to_str().unwrap(),
    ])
    .unwrap();
    let before = std::fs::read(&path).unwrap();

    run_cli(&[
        "tape-block", "set", path.to_str().unwrap(), "--in-place",
        "--alphabet", "0=' ','s','b','k','1'",
    ])
    .expect("repin succeeds");

    let shown = run_cli(&["tape-block", "show", path.to_str().unwrap()]).unwrap();
    assert!(shown.stdout.contains("|sb111k|"), "got:\n{}", shown.stdout);
    // Relabel, never re-map: the cell INDICES are untouched.
    let after = std::fs::read(&path).unwrap();
    assert_ne!(before, after, "the glyph table should have changed");
    assert!(
        cells_of(&after, 0) == cells_of(&before, 0),
        "a repin must not move cell indices"
    );
}

#[test]
fn tape_block_set_rejects_a_repin_of_the_wrong_cardinality() {
    let dir = tempdir_for_test("tb_set_card");
    let exe = compile_and_link_pow2(&dir);
    let path = dir.join("in.tmt");
    run_cli(&["tape-block", "new", "--from", exe.to_str().unwrap(), "-o", path.to_str().unwrap()])
        .unwrap();
    let err = run_cli(&[
        "tape-block", "set", path.to_str().unwrap(), "--in-place",
        "--alphabet", "1=' ','a','b'", // tape 1 is 2 wide
    ])
    .unwrap_err();
    assert!(err.contains("cardinality 2"), "got: {err}");
}

#[test]
fn tape_block_set_takes_names_from_a_source() {
    let dir = tempdir_for_test("tb_set_names");
    let src = dir.join("pow2.tmc");
    std::fs::write(&src, POW2_SRC).unwrap();
    let path = dir.join("in.tmt");
    run_cli(&["tape-block", "new", "--from", src.to_str().unwrap(), "-o", path.to_str().unwrap()])
        .unwrap();
    run_cli(&[
        "tape-block", "set", path.to_str().unwrap(), "--in-place",
        "--from", src.to_str().unwrap(),
        "--cells", "cnt='1'",
    ])
    .expect("name keys resolve on set");
}

#[test]
fn tape_block_show_prints_each_bands_effective_alphabet() {
    let dir = tempdir_for_test("tb_show_alpha");
    let src = dir.join("pow2.tmc");
    std::fs::write(&src, POW2_SRC).unwrap();
    let path = dir.join("in.tmt");
    run_cli(&["tape-block", "new", "--from", src.to_str().unwrap(), "-o", path.to_str().unwrap()])
        .unwrap();
    let shown = run_cli(&["tape-block", "show", path.to_str().unwrap()]).unwrap();
    assert!(shown.stdout.contains("tape 0: origin 0, head 0, alphabet"), "got:\n{}", shown.stdout);
    assert!(shown.stdout.contains("\"k\""), "got:\n{}", shown.stdout);
}

#[test]
fn tape_block_show_honours_the_delimit_flags() {
    let dir = tempdir_for_test("tb_show_delim");
    let path = dir.join("in.tmt");
    run_cli(&[
        "tape-block", "new",
        "--alphabet", "0=' ','a','b'",
        "--cells", "0='a','b'",
        "-o", path.to_str().unwrap(),
    ])
    .unwrap();
    let dense = run_cli(&["tape-block", "show", path.to_str().unwrap()]).unwrap();
    assert!(dense.stdout.contains("|ab|"), "got:\n{}", dense.stdout);
    let sep = run_cli(&["tape-block", "show", path.to_str().unwrap(), "--separated"]).unwrap();
    assert!(sep.stdout.contains("|a|b|"), "got:\n{}", sep.stdout);
}

#[test]
fn tape_block_show_rejects_both_delimit_flags_at_once() {
    let dir = tempdir_for_test("tb_show_both");
    let path = dir.join("in.tmt");
    run_cli(&["tape-block", "new", "--alphabet", "0=' ','a'", "-o", path.to_str().unwrap()])
        .unwrap();
    let err = run_cli(&[
        "tape-block", "show", path.to_str().unwrap(), "--dense", "--separated",
    ])
    .unwrap_err();
    assert!(err.contains("mutually exclusive"), "got: {err}");
}
```

Add a `cells_of(bytes: &[u8], tape: usize) -> Vec<u8>` helper to the test file that decodes an MT block and returns that band's cells, using `mtc_core::formats::TapeBlockFile::from_bytes`.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p mtc-turing-machine --test cli_programs tape_block_s`
Expected: FAIL — `set` still takes `--tape N`/`--cells PATTERN`, and `show` has no delimit flags or per-band alphabet line.

- [ ] **Step 3: Rewrite `tape_set`**

Replace the edit-collecting and application part of `tape_set` (keep the existing `-o`/`--in-place` destination logic verbatim — it is correct and tested):

```rust
    let mut args = Args::new(raw);
    let out = args.value("-o")?;
    let in_place = args.flag("--in-place");
    let from = args.value("--from")?;
    let edits = collect_edits(&mut args)?;
    let inputs = args.positionals()?;
    let [input] = inputs.as_slice() else {
        return Err(format!(
            "tape-block set takes exactly one file\n\n{TAPE_USAGE}"
        ));
    };

    // …destination logic unchanged…

    let bytes = fs::read(input).map_err(|e| format!("cannot read {input}: {e}"))?;
    let mut block = TapeBlockFile::from_bytes(&bytes).map_err(|e| format!("{input}: {e}"))?;

    // `--from` on `set` supplies tape NAMES only; it never reshapes the block
    // (docs/tmt/cli.md (tape-block set)).
    let names: Vec<String> = match from.as_deref() {
        Some(path) => from_source_or_image(path)?.1,
        None => Vec::new(),
    };

    apply_edits(&mut block, &edits, &names, false)?;

    let bytes = block.to_bytes().map_err(|e| format!("{dest}: {e}"))?;
    fs::write(&dest, bytes).map_err(|e| format!("cannot write {dest}: {e}"))?;
    Ok(CliOutput::ok(String::new(), String::new()))
```

- [ ] **Step 4: Rewrite `tape_show`**

```rust
fn tape_show(raw: &[String]) -> Result<CliOutput, String> {
    let mut args = Args::new(raw);
    let dense = args.flag("--dense");
    let separated = args.flag("--separated");
    let inputs = args.positionals()?;
    let [input] = inputs.as_slice() else {
        return Err(format!(
            "tape-block show takes exactly one file\n\n{TAPE_USAGE}"
        ));
    };
    let delimit = match (dense, separated) {
        (true, true) => {
            return Err("tape-block show: --dense and --separated are mutually exclusive".into());
        }
        (true, false) => Delimit::Dense,
        (false, true) => Delimit::Separated,
        (false, false) => Delimit::Auto,
    };
    let bytes = fs::read(input).map_err(|e| format!("cannot read {input}: {e}"))?;
    let block = TapeBlockFile::from_bytes(&bytes).map_err(|e| format!("{input}: {e}"))?;
    let mut out = String::new();
    for (i, tape) in block.tapes.iter().enumerate() {
        // Each band renders through its own effective alphabet, and prints it:
        // which glyphs a band actually uses is not derivable from the block
        // fallback (docs/formats.md (per-tape glyph tables)).
        let effective: &[String] = tape.alphabet.as_deref().unwrap_or(&block.alphabet);
        let rendered = render_tape(tape, effective, delimit);
        let (head_line, rest) = rendered.split_once('\n').expect("render_tape emits 3 lines");
        out.push_str(&format!(
            "tape {i}: {head_line}, alphabet {effective:?}\n{rest}"
        ));
    }
    Ok(CliOutput::ok(out, String::new()))
}
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p mtc-turing-machine --test cli_programs`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/turing-machine/src/cli/inspect.rs crates/turing-machine/tests/cli_programs.rs
git commit -m "feat(turing-machine): keyed edits on tape-block set, per-band alphabets in show"
```

---

### Task 10: PM `tape-block` — keyed edits, block-alphabet repin

**Files:**
- Modify: `crates/post-machine/src/cli/inspect.rs`
- Test: `crates/post-machine/tests/cli_programs.rs`

**Interfaces:**
- Consumes: `parse_keyed` (Task 6), `parse_glyph_list` (Task 1), `Delimit` (Task 4).
- Produces: PM copies of `Edits`, `collect_edits`, `apply_edits` — the same shapes as Task 7, minus the `.tmc` provenance arm.

PM gets the rename, keyed edits, adaptive `show`, and repinning. It does **not** get `--from *.pmc` (spec R3: PM-1's alphabet is fixed at two glyphs, so source has nothing to add). `tape build` is untouched.

The block-alphabet rule matters here: `apply_edits` is called with `pm_block_alphabet = true`, so a repin writes `block.alphabet` and leaves the per-tape override `None`, keeping the file at MT v1 and PM's goldens byte-identical.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn pm_tape_block_repins_the_block_alphabet_and_stays_v1() {
    let dir = tempdir_for_test("pm_repin");
    let path = dir.join("t.pmt");
    run_cli(&["tape-block", "build", " ** ", "-o", path.to_str().unwrap()]).unwrap();

    run_cli(&[
        "tape-block", "set", path.to_str().unwrap(), "--in-place",
        "--alphabet", "0='0','1'",
    ])
    .expect("repin succeeds");

    let shown = run_cli(&["tape-block", "show", path.to_str().unwrap()]).unwrap();
    assert!(shown.stdout.contains("|0110|"), "got:\n{}", shown.stdout);

    // MT v1 is the version field at offset 3..5 (docs/formats.md).
    let bytes = std::fs::read(&path).unwrap();
    assert_eq!(u16::from_le_bytes([bytes[3], bytes[4]]), 1, "PM must stay MT v1");
}

#[test]
fn pm_tape_block_rejects_a_repin_of_the_wrong_cardinality() {
    let dir = tempdir_for_test("pm_repin_card");
    let path = dir.join("t.pmt");
    run_cli(&["tape-block", "build", " * ", "-o", path.to_str().unwrap()]).unwrap();
    let err = run_cli(&[
        "tape-block", "set", path.to_str().unwrap(), "--in-place",
        "--alphabet", "0='a','b','c'",
    ])
    .unwrap_err();
    assert!(err.contains("cardinality 2"), "got: {err}");
}

#[test]
fn pm_tape_block_new_applies_cells_in_the_same_call() {
    let dir = tempdir_for_test("pm_new_cells");
    let path = dir.join("t.pmt");
    run_cli(&[
        "tape-block", "new",
        "--alphabet", "0=' ','*'",
        "--cells", "0='*','*',' '",
        "-o", path.to_str().unwrap(),
    ])
    .expect("new succeeds");
    let shown = run_cli(&["tape-block", "show", path.to_str().unwrap()]).unwrap();
    assert!(shown.stdout.contains("|** |"), "got:\n{}", shown.stdout);
}

#[test]
fn pm_tape_block_rejects_a_tape_name_key() {
    let dir = tempdir_for_test("pm_name_key");
    let path = dir.join("t.pmt");
    run_cli(&["tape-block", "build", " * ", "-o", path.to_str().unwrap()]).unwrap();
    let err = run_cli(&[
        "tape-block", "set", path.to_str().unwrap(), "--in-place", "--cells", "main='*'",
    ])
    .unwrap_err();
    assert!(err.contains("index"), "got: {err}");
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p mtc-post-machine --test cli_programs pm_tape_block`
Expected: FAIL — the edit flags are unknown.

- [ ] **Step 3: Port the edit machinery**

Copy `Edits`, `collect_edits`, `resolve_key`, and `apply_edits` from Task 7 into `crates/post-machine/src/cli/inspect.rs` verbatim, with two changes: `resolve_key`'s name-lookup branch is unreachable on PM (call sites always pass an empty `names`), and the doc-comment citations point at `docs/pmt/cli.md` instead of `docs/tmt/cli.md`.

- [ ] **Step 4: Rewrite `tape_new`**

PM diverges from TM's in three ways: no source arm, `DEFAULT_GLYPHS` instead of
decimal labels, and a bare invocation is legal (PM-1's alphabet is fixed, so
there is nothing an author must supply).

```rust
/// `pmt tape-block new [--from APP.pmx] [-o OUT.pmt] [EDITS]` — mint a block
/// and apply this invocation's edits in one call.
///
/// With `--from`, the band count comes from the image header. Without it, the
/// `--alphabet` keys size the block, or — given none — a single empty band.
/// PM-1's alphabet is fixed at two glyphs, so bands default to the arch pair
/// rather than to index labels (docs/pmt/cli.md (tape-block new)).
fn tape_new(raw: &[String]) -> Result<CliOutput, String> {
    let mut args = Args::new(raw);
    let from = args.value("--from")?;
    let out = args.value("-o")?.unwrap_or_else(|| "blank.pmt".into());
    let edits = collect_edits(&mut args)?;
    let extra = args.positionals()?;
    if !extra.is_empty() {
        return Err(format!(
            "tape-block new takes no positional arguments\n\n{TAPE_USAGE}"
        ));
    }

    let defaults: Vec<String> = DEFAULT_GLYPHS.iter().map(|g| g.to_string()).collect();

    let band_glyphs: Vec<Vec<String>> = match from.as_deref() {
        Some(path) => {
            let bytes = fs::read(path).map_err(|e| format!("cannot read {path}: {e}"))?;
            match sniff(&bytes) {
                Some(ContainerKind::Executable) => {}
                _ => return Err(format!("{path}: not an executable image (.pmx)")),
            }
            let exe = Executable::from_bytes(&bytes).map_err(|e| format!("{path}: {e}"))?;
            vec![defaults.clone(); usize::from(exe.tape_count).max(1)]
        }
        None if edits.alphabets.is_empty() => vec![defaults.clone()],
        None => freehand_bands(&edits)?,
    };

    // Single-alphabet by construction: the block table holds the glyphs and no
    // band overrides it, which is what keeps the file at MT v1
    // (docs/formats.md (tape-block snapshot)).
    let mut block = TapeBlockFile {
        alphabet: band_glyphs[0].clone(),
        tapes: band_glyphs
            .iter()
            .map(|_| TapeSnapshot {
                origin: 0,
                cells: Vec::new(),
                head: 0,
                alphabet: None,
            })
            .collect(),
    };

    apply_edits(&mut block, &edits, &[], true)?;

    let bytes = block.to_bytes().map_err(|e| format!("{out}: {e}"))?;
    fs::write(&out, bytes).map_err(|e| format!("cannot write {out}: {e}"))?;
    Ok(CliOutput::ok(String::new(), String::new()))
}
```

Port `freehand_bands` from Task 7 verbatim, with its citation pointing at
`docs/pmt/cli.md`.

- [ ] **Step 5: Rewrite `tape_set`**

Keep the existing `-o` / `--in-place` destination logic verbatim — it is correct
and already tested — and replace the edit handling:

```rust
    let mut args = Args::new(raw);
    let out = args.value("-o")?;
    let in_place = args.flag("--in-place");
    let edits = collect_edits(&mut args)?;
    let inputs = args.positionals()?;
    let [input] = inputs.as_slice() else {
        return Err(format!(
            "tape-block set takes exactly one file\n\n{TAPE_USAGE}"
        ));
    };

    // …destination logic unchanged…

    let bytes = fs::read(input).map_err(|e| format!("cannot read {input}: {e}"))?;
    let mut block = TapeBlockFile::from_bytes(&bytes).map_err(|e| format!("{input}: {e}"))?;

    // No names on PM: a tape key is always an index, so `resolve_key` reports
    // the "names need --from a .tmc source" error for anything else.
    apply_edits(&mut block, &edits, &[], true)?;

    let bytes = block.to_bytes().map_err(|e| format!("{dest}: {e}"))?;
    fs::write(&dest, bytes).map_err(|e| format!("cannot write {dest}: {e}"))?;
    Ok(CliOutput::ok(String::new(), String::new()))
```

- [ ] **Step 6: Rewrite `tape_show`**

Use Task 9's `tape_show` body verbatim, changing only the usage constant's
wording (`tape-block show takes exactly one file`) and the error prefix
(`tape-block show: --dense and --separated are mutually exclusive`). It keeps
Task 3's effective-alphabet resolution, which the per-band alphabet line now
also prints.

- [ ] **Step 7: Update the usage text**

Replace `TAPE_USAGE`, keeping the `build` line:

```rust
const TAPE_USAGE: &str = "\
USAGE: pmt tape-block build \" * * *\" [--head N] [-o OUT.pmt]
       pmt tape-block new [--from APP.pmx] [-o OUT.pmt] [EDITS]
       pmt tape-block set IN.pmt (-o OUT.pmt | --in-place) [EDITS]
       pmt tape-block show FILE.pmt [--dense | --separated]

EDITS (repeatable; KEY is a tape index):
  --alphabet KEY=GLYPHS   repin the block's glyphs (relabels; same cardinality)
  --cells    KEY=GLYPHS   set tape KEY's cells
  --head     KEY=N        set tape KEY's head
  --origin   KEY=N        set tape KEY's origin

build: cell characters are the PM-1 glyphs (space = blank, * = mark); the
leftmost character is cell 0. GLYPHS is alphabet notation: ' ','*'.
";
```

- [ ] **Step 8: Run the tests and verify PM goldens are untouched**

```bash
cargo test --workspace && git status --short crates/post-machine/tests/golden
```
Expected: PASS; `git status` prints nothing.

- [ ] **Step 9: Commit**

```bash
git add crates/post-machine/src/cli/inspect.rs crates/post-machine/tests/cli_programs.rs
git commit -m "feat(post-machine): keyed edits and glyph repinning in pmt tape-block"
```

---

### Task 11: `tmt run --save-tape-block` and the cardinality check

**Files:**
- Modify: `crates/turing-machine/src/cli/run.rs`
- Test: `crates/turing-machine/tests/cli_programs.rs`

**Interfaces:**
- Consumes: nothing.
- Produces: `RunSettings.save: Option<String>` on the TM side.

Spec R10 and R11. The save must carry **each band's own glyph table** through — cloning one block alphabet, as PM does, would round-trip a repinned three-tape block into a single-alphabet file and silently discard the pinned glyphs.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn run_save_tape_block_preserves_every_bands_glyphs() {
    let dir = tempdir_for_test("tm_save");
    let src = dir.join("pow2.tmc");
    std::fs::write(&src, POW2_SRC).unwrap();
    let exe = compile_and_link_pow2(&dir);
    let input = dir.join("in.tmt");
    let saved = dir.join("out.tmt");

    run_cli(&["tape-block", "new", "--from", src.to_str().unwrap(), "-o", input.to_str().unwrap()])
        .unwrap();
    run_cli(&[
        "run", exe.to_str().unwrap(),
        "--tape-block", input.to_str().unwrap(),
        "--save-tape-block", saved.to_str().unwrap(),
    ])
    .expect("run succeeds");

    let shown = run_cli(&["tape-block", "show", saved.to_str().unwrap()]).unwrap();
    // Band 0's glyphs are mainAlpha's, band 1's are workAlpha's — a single
    // block alphabet cannot express both.
    assert!(shown.stdout.contains("\"k\""), "band 0 lost its glyphs:\n{}", shown.stdout);
    let band1 = shown.stdout.lines().find(|l| l.starts_with("tape 1:")).unwrap();
    assert!(band1.contains("[\" \", \"1\"]"), "band 1 lost its glyphs: {band1}");
}

#[test]
fn run_rejects_a_block_whose_cardinality_disagrees_with_the_image() {
    let dir = tempdir_for_test("tm_card");
    let exe = compile_and_link_pow2(&dir); // tape 0 is 5 wide
    let bad = dir.join("bad.tmt");
    run_cli(&[
        "tape-block", "new",
        "--alphabet", "0=' ','x'",   // 2 wide
        "--alphabet", "1=' ','1'",
        "--alphabet", "2=' ','1'",
        "-o", bad.to_str().unwrap(),
    ])
    .unwrap();

    let err = run_cli(&["run", exe.to_str().unwrap(), "--tape-block", bad.to_str().unwrap()])
        .unwrap_err();
    assert!(err.contains("tape 0"), "got: {err}");
    assert!(err.contains("2"), "got: {err}");
    assert!(err.contains("5"), "got: {err}");
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p mtc-turing-machine --test cli_programs save_tape_block`
Expected: FAIL — `--save-tape-block` is an unknown flag, and the mismatched block runs and renders `?`.

- [ ] **Step 3: Add the cardinality check**

In `execute_run`, immediately after the existing band-count check:

```rust
    // Band count is not enough: a band whose alphabet is narrower than the
    // program's would load, then render `?` for every out-of-range index.
    // The image's per-tape cardinalities are the authority
    // (docs/formats.md (executable header)).
    for (i, snap) in block.tapes.iter().enumerate() {
        // `.get(i)` rather than indexing: the band-count check above pins
        // `block.tapes.len()` to `exe.tape_count`, but nothing pins
        // `alphabet_cardinalities.len()` to it — a v1 code-only image carries
        // none at all. A missing entry means "no declared width", so skip.
        let Some(&declared) = exe.alphabet_cardinalities.get(i) else {
            continue;
        };
        let width = snap
            .alphabet
            .as_ref()
            .map_or(block.alphabet.len(), Vec::len);
        let expected_width = declared as usize;
        if width != expected_width {
            return Err(format!(
                "{tape_path}: tape {i} has {width} glyph(s), but {} expects {expected_width}",
                exe_path.display(),
            ));
        }
    }
```

- [ ] **Step 4: Add `--save-tape-block`**

Add `save: Option<String>` to `RunSettings`, parse it in the argv path alongside the other flags (`args.value("--save-tape-block")?`), add it to `RUN_USAGE`, and write the block after the run, next to where the final tapes are rendered:

```rust
    if let Some(out_path) = &settings.save {
        // Each band keeps its OWN glyph table. Collapsing to one block
        // alphabet would silently relabel a block whose bands differ
        // (docs/formats.md (per-tape glyph tables)).
        //
        // `WideTape::to_snapshot` leaves `alphabet: None` (it is a device and
        // holds no glyphs), so this assignment supplies the table rather than
        // replacing one.
        let saved = TapeBlockFile {
            alphabet: block.alphabet.clone(),
            tapes: tapes
                .iter()
                .enumerate()
                .map(|(i, t)| {
                    let mut snap = t.to_snapshot();
                    snap.alphabet = Some(alphabets[i].clone());
                    snap
                })
                .collect(),
        };
        let bytes = saved.to_bytes().map_err(|e| e.to_string())?;
        fs::write(out_path, bytes).map_err(|e| format!("cannot write {out_path}: {e}"))?;
    }
```

Place this after `drop(devices)` and before building the exit code, so the tapes are no longer mutably borrowed.

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test --workspace`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/turing-machine/src/cli/run.rs crates/turing-machine/tests/cli_programs.rs
git commit -m "feat(turing-machine): tmt run --save-tape-block and a per-tape cardinality check"
```

---

### Task 12: Completions registries

**Files:**
- Modify: `crates/{post-machine,turing-machine}/src/completions/registry.rs`
- Test: `crates/{post-machine,turing-machine}/tests/completions_registry.rs`, `completions_zsh.rs`

**Interfaces:**
- Consumes: the final flag surface from Tasks 7–11.
- Produces: nothing new.

The registry is the single in-crate description of the CLI surface, and its drift guard probes the real parser with every entry. Task 5 renamed the paths; this task adds the new flags.

**Pre-existing defect to fix here** (found during Task 5, not caused by it): a
registry description containing an apostrophe is emitted **unescaped** inside a
zsh single-quoted string. Today `"glyph pattern for the tape's cells"` renders
as `'--cells[glyph pattern for the tape's cells]:value:'`, which zsh parses as
quote-concatenation and mangles the description. `zsh -n` and `compinit` both
accept it, so the existing integration tests cannot catch it. Escape `'` as
`'\''` in the renderer's description path, and add a unit test asserting a
description containing an apostrophe round-trips.

- [ ] **Step 1: Update TM's specs**

In `crates/turing-machine/src/completions/registry.rs`, replace `tape_new_spec`, `tape_set_spec`, `tape_show_spec` flag lists. The four edit flags are repeatable `KEY=GLYPHS` values with `ValueHint::Text`:

```rust
/// The repeatable keyed edit flags, shared by `tape-block new` and `set`.
fn edit_flags() -> Vec<FlagSpec> {
    vec![
        FlagSpec::value("--alphabet", "repin tape KEY's glyphs (KEY=GLYPHS)", ValueHint::Text),
        FlagSpec::value("--cells", "set tape KEY's cells (KEY=GLYPHS)", ValueHint::Text),
        FlagSpec::value("--head", "set tape KEY's head (KEY=N)", ValueHint::Text),
        FlagSpec::value("--origin", "set tape KEY's origin (KEY=N)", ValueHint::Text),
    ]
}
```

`tape_new_spec` keeps `--from` (widen its hint to `ext(&["tmx", "tmc"])`) and `-o`, plus `edit_flags()`. `tape_set_spec` keeps `-o`/`--in-place`/`--from` and drops `--tape`, plus `edit_flags()`. `tape_show_spec` gains two exclusive booleans:

```rust
        flags: vec![
            FlagSpec::boolean("--dense", "never separate cells").exclusive("show-delimit"),
            FlagSpec::boolean("--separated", "always separate cells").exclusive("show-delimit"),
        ],
```

In `run_spec`, add:

```rust
            FlagSpec::value(
                "--save-tape-block",
                "write the final tape band as an MT snapshot",
                ValueHint::File(ext(&["tmt"])),
            ),
```

- [ ] **Step 2: Update PM's specs**

Mirror the same changes in `crates/post-machine/src/completions/registry.rs`, except: `tape_new_spec`'s `--from` hint stays `ext(&["pmx"])` (no source path on PM), and `tape_set_spec` has no `--from` at all.

- [ ] **Step 3: Run the drift guards**

```bash
cargo test -p mtc-turing-machine --test completions_registry --test completions_zsh
cargo test -p mtc-post-machine --test completions_registry --test completions_zsh
```
Expected: PASS. The registry guard probes the real parser with every entry, so a flag the parser does not accept fails here with `unknown flag`.

- [ ] **Step 4: Regenerate and sanity-check a script**

```bash
cargo run --release -p mtc-turing-machine --bin tmt -- completions zsh | head -40
```
Expected: a `#compdef tmt` script naming `tape-block`.

- [ ] **Step 5: Commit**

```bash
git add crates/turing-machine/src/completions crates/post-machine/src/completions
git commit -m "feat(cli): register the tape-block edit flags for shell completion"
```

---

### Task 13: Documentation

**Files:**
- Modify: `docs/formats.md`, `docs/pmt/cli.md`, `docs/tmt/cli.md`

**Interfaces:**
- Consumes: the shipped CLI.
- Produces: nothing.

Per the repo's docs-audit convention, **every quoted transcript is re-run and pasted, never hand-edited**. Published docs are forge-agnostic: no issue numbers, no hosting URLs.

- [ ] **Step 1: Rewrite the `docs/formats.md` tape-block CLI paragraph**

Replace the CLI example lines (currently around 326-330) with re-run output, and revise the glyph-provenance paragraph (currently around 312-324): its claim that decimal labels are what "the author then edits or replaces" now has a named mechanism. State that `tmt tape-block new --from` accepts either an executable (cardinalities, decimal labels) or `.tmc` source (real glyphs and tape names), that `--alphabet` repins an existing block by relabelling, and that **tape names are never stored** — the container addresses bands by index.

- [ ] **Step 2: Rewrite the `tape-block` sections of both CLI pages**

For each of `docs/pmt/cli.md` and `docs/tmt/cli.md`, replace the `tape` section with a `tape-block` section covering: the three (PM: four) subcommands, the edit-flag table, the glyph notation with a range example, the repin semantics (relabel, same cardinality), and the `show` delimiting rule. Update the run-flag reference for `--tape-cells` (PM), `--tape-block` and `--save-tape-block` (TM).

- [ ] **Step 3: Re-run every transcript**

```bash
cargo build --release
./target/release/tmt tape-block
./target/release/pmt tape-block
./target/release/tmt run --help
./target/release/pmt run --help
```

Build the `pow2` example end to end and paste its real output as the worked example:

```bash
./target/release/tmt tape-block new --from pow2.tmc \
  --cells "main='s','b','1','1','1','k'" -o in.tmt
./target/release/tmt tape-block show in.tmt
```

- [ ] **Step 4: Run the doc drift guards**

```bash
cargo test -p mtc-turing-machine --test cli_docs && cargo test -p mtc-post-machine --test cli_docs
```
Expected: PASS. TM's guard pins the `tmt --help` quote verbatim.

- [ ] **Step 5: Full gates**

```bash
cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --check
git status --short crates/post-machine/tests/golden crates/turing-machine/tests/golden
```
Expected: all pass; no golden moved.

- [ ] **Step 6: Commit**

```bash
git add docs
git commit -m "docs: tape-block authoring, glyph repinning, and rendering rules"
```

---

## Verification Checklist

Run at the end of the round, before opening the PR:

- [ ] `cargo test --workspace` passes (~2,226 tests plus this round's additions).
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` is clean.
- [ ] `cargo fmt --check` is clean.
- [ ] `git status --short crates/*/tests/golden` prints nothing — no golden regenerated.
- [ ] `crates/core/src/formats/tapeblock.rs` is unchanged: `git diff --stat master -- crates/core/src/formats/tapeblock.rs` is empty. MT stays v2.
- [ ] No `docs/superpowers/` path appears in any new code comment: `rg "docs/superpowers" crates/` returns nothing.
- [ ] No issue or PR number in published docs: `rg "#[0-9]+" docs/*.md docs/pmt docs/tmt README.md CHANGELOG.md` returns nothing new.
- [ ] The spec's version-space claim holds: no `*_VERSION` constant changed.

## Deferred to the release cut

`CHANGELOG.md` gets its version block at the cut, not in this round. It must state: crates bumped; **MT unchanged at v2**; `.pmc`/`.tmc` languages, both `.pma`/`.tma` dialects, both IR versions, MO, MX, and both project-manifest schemas all unchanged.
