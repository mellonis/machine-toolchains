# C2 Plan 12 — the arc closer: delete both oracles and both C1 CSTs

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Remove the C1 CST from both `mtc-post-machine` and
`mtc-turing-machine` — the differential oracles that held the green tree equal
to it, the `parse_cst`/`lower_cst` path, and the CST construction still running
inside the shared parser on every green parse — leaving one parse path per
crate, with no coverage lost.

**Architecture:** Two phases. Phase A retires the *oracles*: `parser::parse` is
redefined over the green tree so the ~31 `#[cfg(test)]` call sites keep working,
the coverage the oracles carried is measured rather than assumed (deliberate
breaks re-injected, recording which suite catches which), the measured holes are
closed with targeted tests, and only then are the differential tests deleted.
Phase B retires the *CST*: three leaf types rehome to `parser.rs`, the container
productions stop building `*Cst` values, `Option<GreenSink>` collapses to an
unconditional sink, and both `cst.rs` files are deleted.

**Tech Stack:** Rust (workspace toolchain pinned by `rust-toolchain.toml`),
`crates/{post-machine,turing-machine}/src/{parser.rs,cst.rs,syntax/}`,
`mtc-core`'s `syntax` framework. `proptest` as dev-dep. No new dependencies.

**Spec:** `docs/superpowers/specs/2026-08-17-c2-green-tree-syntax-design.md`
(approved; the arc's design doc — plan 12 is its last plan). This plan adds no
new design; it executes the spec's cutover.

**Tracker:** https://github.com/mellonis/machine-toolchains/issues/14 — the C2
migration. This plan closes it.

---

## Global Constraints

Copied verbatim from the arc's standing rules. Every task's requirements
implicitly include this section.

- **`crates/core` neutrality — zero diff.** Plans 8, 9 and 10 each held
  `git diff --stat <base>..HEAD -- crates/core` empty for the whole plan. This
  plan does the same. Core's `asm/cst.rs` is a DIFFERENT CST and is **out of
  scope by the spec**; do not touch it, and do not let a grep for `cst` pull it
  in.
- **PM-1 byte-identity.** Nothing here may change a byte of PM-1 output.
  Enforced by PM's derivation-first goldens and `asm_volatile.rs`.
- **Gates on every commit:** `cargo fmt --check`,
  `cargo clippy --workspace --all-targets -- -D warnings`,
  `cargo test --workspace`. The pinned toolchain means local clippy IS the gate
  CI runs.
- **No CHANGELOG entry.** Prior ruling: the whole C2 arc lands in the v0.5.0
  bump with an all-unchanged version block. Do not add one here.
- **No merge.** The arc merges ONCE, after this plan, and the merge is the
  maintainer's call. This plan ends at merge-prep.
- **Published-docs policy.** `README.md`, `CHANGELOG.md` and `docs/` carry no
  issue/PR numbers and no hosting URLs. Code comments cite durable pages as
  `docs/<page>.md (topic keyword)` — never a `docs/superpowers/` path, frozen or
  active.
- **Two shell names used throughout.** `$SCRATCH` is this session's scratchpad
  directory — never a path inside the repo (`[[probes-belong-in-scratchpad]]`);
  export it once at the start of a task. `<task-base>` is the SHA the current
  task started from: capture it with `BASE=$(git rev-parse HEAD)` before the
  task's first edit and substitute it where the plan writes `<task-base>`.
- **Write the invariant and its enforcement, never the current neighbour's
  reason for satisfying it.** Plan 9 shipped a sentence that was true the day it
  was written and false one task later. This plan deletes a lot of prose; every
  replacement sentence must survive its neighbour changing.

---

## Measured facts this plan rests on

All measured on `b6b698e`, not inferred. An implementer who doubts one should
re-measure it, not reason about it.

**1. `parse` cannot keep its signature.** `parse_green_from_tokens(source,
tokens)` needs `&str` source AND a `LexMode::WithComments` stream — its own doc
comment says a comment-free stream "would lose every comment's own text and
break the `text() == source` law". So `pub fn parse(tokens: &[Token])` cannot be
reimplemented in place; it becomes `parse(source: &str)`.

**2. The C1 CST is built on the green path too.** `parse_green_from_tokens`
calls the same `Parser::file()` and writes `let (_items, sink) = …` — the
container CST is constructed and dropped on every production parse in both
crates. Deleting `parse_cst` alone would leave that intact.

**3. The excision boundary is clean, and it is the container/leaf line.**
`cst.rs` defines ONLY container types plus three leaves:

| crate | container types (die) | leaf types (rehome to `parser.rs`) |
|---|---|---|
| PM | `Cst`, `TopItem`, `TopKind`, `TrailingComment`, `UsePath`, `UseCst`, `NamespaceCst`, `FunctionCst`, `BodyItem`, `BodyKind`, `CommaItem`, `StatementCst` | `DocRunItem`, `DocRunKind`, `AttrCst` |
| TM | `Cst`, `TopItem`, `TopKind`, `UsePath`, `UseCst`, `AlphabetCst`, `ReuseCarrier`, `ReuseCst`, `MachineCst`, `NamespaceCst`, `WorldItem`, `WorldKind`, `TapeCst`, `StateCst`, `RuleItem`, `RuleKind`, `RuleCst`, `GraftCst`, `BindCst` | `DocRunItem`, `DocRunKind`, `AttrCst` |

Every value type the reparse shims return already lives in `parser.rs`:
TM's ten shims return `Transition`, `BindingArg`, `Vec<AlphabetElem>`,
`Pattern`, `WriteVec`, `MoveVec`, `QualName`, `SymMap`, `SigParam`,
`Vec<DocRunItem>`; PM's two return `Item` and `Vec<DocRunItem>`. `AttrCst` is
reachable only through `DocRunKind::Attention { attr: Option<AttrCst>, .. }`,
which is why it rehomes with the other two rather than dying.

**4. Blanket `Program` goldens do not fit, measured not guessed.** `{:#?}` of
the extracted `Program` per corpus file:

| | files | golden lines | bytes |
|---|---|---|---|
| TM (`tests/golden`, `src/stdlib`, `docs/examples`) | 9 | 28,603 | ~1.25 MB |
| — `std.tmc` alone | 1 | 16,574 | ~702 KB |
| — `brainfuck-utm.tmc` alone | 1 | 6,573 | ~317 KB |
| PM (whole `.pmc` corpus) | 13 | 7,588 | ~270 KB |

~36k lines that can only be captured, never hand-derived, and that churn wholly
on any `Program` field addition. **Ruling: no blanket goldens.** The coverage is
replaced by measurement (Task 3) plus targeted tests (Task 4).

**5. Two of the three known breaks probably do not need a new test at all.**
Plan 8's break table (`.superpowers/sdd/2026-08-24-c2-plan8-tmc-views/progress.md`):

| stage | mutation | corpus oracle | generated oracle | round-trip law |
|---|---|---|---|---|
| view accessor | `StateView::is_entry` inverted | RED (`a1_replace_b.tmc`) | RED | green |
| reparse shim | `reparse_pattern` reverses cells | RED (`a3_two_tape_copy.tmc`) | RED | green |
| assembly | a routine's `doc` dropped | RED (`std.tmc`) | RED | green |

The first two change what the machine EXECUTES, so the derivation-first `.tmt`
goldens plausibly already catch them. That is a prediction, and Task 3 measures
it instead of believing it.

**6. Three in-`src` tests are oracles, not helpers**, and Task 2 changes what
each one proves:

- `post-machine/src/compiler.rs::analyze_matches_the_c1_front_end` — compares
  `analyze`'s AST/diagnostics/scopes against `flatten(parse(…))`. Once `parse`
  runs the green tree, both sides run the SAME green parse and the same
  `flatten` + `lower_and_merge`: **this becomes a tautology**, except its
  `a.tokens == lex(src)` provenance assertion.
- `post-machine/src/cli/driver.rs::scan_source_matches_the_c1_front_end` —
  `scan_source` has its own scanning logic, so this stays a real cross-check
  between two code paths. Keep it; restate its doc.
- `turing-machine/src/compiler.rs` staged-vs-batch (`assert_eq!(program,
  &parse(&batch_tokens).unwrap())`) — still crosses `analyze_staged`'s
  degradation tiers against a third party. Keep it; restate its doc.

**6b. `analyze`'s result differs between the crates, and the difference is
load-bearing.** Neither crate's batch `analyze` keeps the green tree — PM's
`AnalysisOutput` (`compiler.rs:389-399`) and TM's `Analysis`
(`compiler.rs:1016-1032`) both lack a tree field; the tree lives on
`StagedAnalysis.green` / `TmcStagedAnalysis.green`, and BOTH language services
call `analyze_staged`, never `analyze`. The `tokens` field diverges too:
**PM's is comment-FREE** (`compiler.rs:432` runs `significant_tokens`), **TM's
is comment-INCLUSIVE** (its own field doc says so, and lint filters at the
`LintContext` boundary). A test written against one crate's shape is wrong
about the other — measured after a first draft of Task 1 Step 6 asserted PM's
tokens needed filtering, which is a no-op there.

**7. The reparse shims' fidelity is pinned by the oracles and almost nothing
else.** `turing-machine/src/syntax/extract.rs`'s test module holds seven
`reparsed_X_equals_the_c1_X` tests plus an `agrees(src)` helper over 38 sources,
all comparing a shim against `lower_cst`. This is the largest single body of
coverage the deletion removes, and Task 4 is mostly about it.

**8. Call sites to migrate: 33, and the count was wrong once already.** PM 26
(`ir.rs` ×4, `codegen.rs` ×1, `compiler.rs` ×1, `cli/driver.rs` ×1, `parser.rs`
×1, `optimizer/*` ×18) — all in `src/`, none in `tests/`. TM 7: five in `src/`
(`compiler.rs`, `expand.rs`, `parser/tests.rs` ×2, `syntax/extract.rs`) **plus
two in `tests/`** — `tests/syntax_green.rs` and `tests/tmc_green_analyze.rs`.
The uniform shape is `lower(&parse(&lex(src).unwrap()).unwrap())`.

**The trap that hid the last two, worth more than the count:** TM's
`tests/syntax_green.rs` imported the function as
`use mtc_turing_machine::parser::parse as parse_ast;` and called it
`parse_ast(...)`. An inventory grep for `parser::parse(` or `::parse(&` matches
neither. **Grep for aliased imports (`parse as `) before trusting any call-site
census in this repo.** Both sites were C1-path ORACLES — they compare the CST
path against the green path — so redefining `parse` would have made them
compare the green path with itself: still passing, proving nothing, silently.
They were rewritten to compose `parse_cst`/`lower_cst` directly, which keeps
the oracle alive until Task 5 retires it deliberately.

---

## Out of scope — stated so an implementer does not sweep them in

- `crates/core`'s `asm/cst.rs` and everything under `crates/*/src/lsp/pma/` and
  `lsp/tma/` that references it. Different CST. Zero core diff.
- The `.pmc`/`.tmc` fmt comment-relocation behaviours. Those are #98's plan
  (`docs/superpowers/plans/2026-08-30-tmc-comments-never-move.md`), which runs
  AFTER this one.
- Two of plan 4's three deferred follow-ups: `views.rs`'s top-level assert
  printing a raw `SyntaxKind(u16)`, and `NamespaceView::name_token`'s
  unconditional `Vec` allocation. **Only the double token split is in scope**
  (prior ruling: "left for the cutover plan which touches them anyway").
- Error-resilient parsing, incremental reparse, the asm CST onto the framework.
  These are the arc's follow-ups, filed in Task 10, not built here.

---

## File structure

| file | change |
|---|---|
| `crates/post-machine/src/parser.rs` | `parse` redefined; `parse_cst`/`lower_cst`/`lower_items` deleted; `DocRunItem`/`DocRunKind`/`AttrCst` rehomed in; container productions stop returning `*Cst`; `sink` unconditional; CST-shape tests deleted or converted |
| `crates/post-machine/src/cst.rs` | **deleted** |
| `crates/turing-machine/src/parser.rs` | same as PM's |
| `crates/turing-machine/src/cst.rs` | **deleted** |
| `crates/post-machine/src/syntax/extract.rs` | C1-side test comparisons converted to pinned values; line-number citations to `parser.rs:395`/`:402` replaced |
| `crates/turing-machine/src/syntax/extract.rs` | same; the seven `reparsed_X_equals_the_c1_X` tests converted |
| `crates/turing-machine/src/fmt/print.rs` | import moves from `crate::cst` to `crate::parser` |
| `crates/post-machine/tests/syntax_green.rs` | four C1 tests removed/converted |
| `crates/turing-machine/tests/syntax_parity.rs` | **deleted** |
| `crates/turing-machine/tests/syntax_green.rs` | two C1-oracle tests retired (Task 5) |
| `crates/turing-machine/tests/tmc_property.rs` | `both_paths` loses its C1 half; `REQUIRED_CONSTRUCTS` re-derived from generator branches |
| `crates/turing-machine/tests/syntax_views.rs`, `tmc_green_analyze.rs` | single C1 call sites converted |
| `crates/post-machine/src/{compiler,cli/driver}.rs`, `crates/turing-machine/src/compiler.rs` | the three oracle tests re-based per fact 6 |
| `CLAUDE.md` | the `### Pipeline and key types` and `### The .tmc front end` paragraphs |

---
# PHASE A — retire the oracles

## Task 1: PM — one parse entry point

**Files:**
- Modify: `crates/post-machine/src/parser.rs:295-306` (the `parse` fn), `:2204-2215` (the test-module helper)
- Modify: `crates/post-machine/src/ir.rs:465-469,584,588,592`, `codegen.rs:175-176`, `compiler.rs:2244-2274`, `cli/driver.rs:1247-1253`
- Modify: `crates/post-machine/src/optimizer/{branch_fold,cell_state,check_fold,dataflow,dce,inline,jump_threading,move_elim,tail_call,tail_merge,tail_sink}.rs`
- Test: `crates/post-machine/src/parser.rs` (test module), `crates/post-machine/tests/syntax_green.rs`

**Interfaces:**
- Produces: `pub fn parse(source: &str) -> Result<Program, CompileError>` in
  `mtc_post_machine::parser` — replaces `parse(tokens: &[Token])`. Tasks 2, 6
  and 8 depend on this signature.
- Consumes: `parse_green_from_tokens(source: &str, tokens: &[Token]) ->
  Result<Rc<GreenNode>, CompileError>` and `crate::syntax::extract_program(&SyntaxNode, &str) -> Program`, both already public.

- [ ] **Step 1: Write the failing test**

In `crates/post-machine/src/parser.rs`'s test module, replace the `parse_src`
helper (currently `parse(&lex(src).unwrap())`) with a test that pins the new
entry point's contract — that it lexes `WithComments` itself, and that its error
on an unlexable source is the lexer's own:

```rust
    /// The crate's one parse entry point: source in, `Program` out, with
    /// the `WithComments` lex it needs done internally. Pinned here
    /// because every other test module in the crate calls it and none of
    /// them assert its contract.
    #[test]
    fn parse_takes_source_and_lexes_with_comments_itself() {
        // The comment sits BETWEEN two significant tokens and on a line
        // it does not lengthen, so every span below it is unmoved: the
        // two programs must be equal field for field. A comment on its
        // own line would shift every following line and the comparison
        // would fail for a reason that has nothing to do with trivia —
        // measured, not assumed.
        let src = "main() { /* c */\n    right;\n}\n";
        let bare = "main() {\n    right;\n}\n";
        let p = parse(src).expect("parses");
        assert_eq!(p.functions.len(), 1);
        assert_eq!(p.functions[0].name, "main");
        assert_eq!(p.functions, parse(bare).expect("parses").functions);
    }

    /// A lex failure surfaces as the lexer's own error, unchanged by the
    /// mode `parse` picks internally.
    #[test]
    fn parse_reports_the_lexers_own_error() {
        let src = "/* never closed\nmain() { right; }\n";
        assert_eq!(
            parse(src).map(|_| ()).unwrap_err(),
            crate::lexer::lex(src).map(|_| ()).unwrap_err()
        );
    }
```

- [ ] **Step 2: Run it and watch it fail**

```
cargo test -p mtc-post-machine --lib parser::tests::parse_takes_source 2>&1 | tail -20
```

Expected: a COMPILE error — `expected &[Token], found &str`. That is the
correct failure; the signature does not exist yet.

- [ ] **Step 3: Redefine `parse`**

In `crates/post-machine/src/parser.rs`, replace the body and doc of `parse`
(currently `parse_cst(tokens).map(|cst| lower_cst(&cst))`):

```rust
/// Source → AST, through the one parse path this crate has: a
/// `WithComments` lex, the green syntax tree, then extraction
/// (docs/core.md (syntax trees)).
///
/// The convenience wrapper for callers that want only the `Program` —
/// it keeps nothing else, and it is the only parse function here that
/// yields one. A caller needing the token stream alongside it uses
/// `compiler::analyze`; a caller needing the green tree uses
/// `compiler::analyze_staged`, which is the one that retains it.
pub fn parse(source: &str) -> Result<Program, CompileError> {
    let tokens = lex_with(source, LexMode::WithComments)?;
    let green = parse_green_from_tokens(source, &tokens)?;
    Ok(crate::syntax::extract_program(
        &SyntaxNode::new_root(green),
        source,
    ))
}
```

Add `use mtc_core::syntax::SyntaxNode;` to the file's imports if absent.
`lex_with`/`LexMode` are already imported — `parse_green` two functions below
uses both.

Note the module cycle this creates (`parser` → `syntax` → `parser`). Rust
permits it and the crate already has the `syntax` → `parser` edge for the
reparse shims; do not restructure to avoid it.

- [ ] **Step 4: Run the two new tests**

```
cargo test -p mtc-post-machine --lib parser::tests::parse_ 2>&1 | tail -10
```

Expected: 2 passed. The rest of the crate will NOT compile yet — that is Step 5.

- [ ] **Step 5: Migrate the 26 call sites**

The uniform shape is `parse(&lex(src).unwrap())`. Run the mechanical pass, then
fix what it cannot reach:

```bash
cd crates/post-machine/src
sed -i '' -E 's/parse\(&lex\((&?[A-Za-z_][A-Za-z0-9_]*)\)\.unwrap\(\)\)/parse(\1)/g' \
  ir.rs codegen.rs optimizer/*.rs
cargo check -p mtc-post-machine --all-targets 2>&1 | grep -E "^error" | head -20
```

The sed does not reach string-literal arguments or the two-line forms. Fix these
five by hand:

- `ir.rs:584,588,592` — `lower(&parse(&lex("f() { goto 9; }").unwrap()).unwrap())`
  becomes `lower(&parse("f() { goto 9; }").unwrap())` (same for the `left(7)`
  and `check(7, !)` sources).
- `optimizer/dce.rs:47` — same shape, source `"f() { goto 1; right; 1: left; }"`.
- `optimizer/check_fold.rs:31,38` — same shape, sources
  `"f() { 1: check(1, 1); }"` and `"f() { 1: check(1, !); }"`.
- `codegen.rs:176` — `crate::parser::parse(&crate::lexer::lex(src).unwrap())`
  becomes `crate::parser::parse(src)`.
- `parser.rs`'s own test module — delete the now-redundant `parse_src` helper and
  call `parse(src)` directly at its call sites.

Then remove every `use crate::lexer::lex;` that has become unused. `cargo check`
names them as warnings; clippy with `-D warnings` will fail on them, so this is
not optional.

- [ ] **Step 6: Re-base the two PM oracle tests (measured fact 6)**

`compiler.rs::analyze_matches_the_c1_front_end` is now a tautology on four of
its five assertions — both sides run the same green parse and the same
`flatten`. Replace it with a test that pins what is still real. Nothing here
needs a value captured from C1: the surviving claim is the token-provenance
law, which compares `analyze`'s own two lex modes against each other.

```rust
    /// `analyze`'s own contract, pinned by value rather than against a
    /// second implementation of it. The C1 front end it used to be
    /// compared against is gone; what survives is the part that was
    /// never a tautology — token provenance, which is a claim about
    /// `analyze`'s lex MODE, not about its parse.
    ///
    /// The AST/diagnostics/scopes halves are covered by this module's
    /// other tests and by `tests/compile_programs.rs`; re-asserting them
    /// against `flatten(parse(src))` would now compare `analyze` with
    /// itself.
    #[test]
    fn analyze_keeps_token_provenance_against_a_plain_lex() {
        let src = "// lead\nuse std::goToEnd as end;\nnamespace ns { export inner() { right; } }\n? documented\nexport main() {\n    helper() { left; }\n    007: @helper();\n    @ns::inner();\n    @end();\n    goto 007;\n}\n";
        let a = analyze(src).expect("analyzes");
        let plain = crate::lexer::lex(src).expect("lexes");
        let significant: Vec<_> = a
            .tokens
            .iter()
            .filter(|t| !matches!(t.kind, crate::lexer::TokenKind::Comment(_)))
            .cloned()
            .collect();
        assert_eq!(significant, plain, "token provenance law");
        assert!(
            a.tokens.len() > plain.len(),
            "fixture sanity: the source must carry a comment, or this proves nothing"
        );
    }
```

`cli/driver.rs::scan_source_matches_the_c1_front_end` STAYS — `scan_source` has
its own scanning logic, so it is a real cross-check. Rename it to
`scan_source_agrees_with_the_parse_entry_point` and rewrite its doc to say what
it checks (two independent readers of the same source agreeing) rather than
naming C1. Its body needs only the Step-5 mechanical change.

- [ ] **Step 7: Run the gates**

```
cargo fmt --check && cargo clippy -p mtc-post-machine --all-targets -- -D warnings && cargo test -p mtc-post-machine
```

Expected: all green. If `tests/syntax_green.rs` fails, it is because one of its
four C1 tests calls `parse`; those are Task 5's subject — leave them compiling by
using `parse_cst`/`lower_cst` directly, which still exist.

- [ ] **Step 8: Commit**

```bash
git add -A crates/post-machine
git commit -m "refactor(post-machine): parse takes source and runs the green tree"
```

---

## Task 2: TM — one parse entry point

**Files:**
- Modify: `crates/turing-machine/src/parser.rs:554-560` (the `parse` fn)
- Modify: `crates/turing-machine/src/compiler.rs:4434`, `expand.rs:1690-1694`, `syntax/extract.rs:2539`, `parser/tests.rs:11-13,1069-1074`
- Test: `crates/turing-machine/src/parser/tests.rs`

**Interfaces:**
- Produces: `pub fn parse(source: &str) -> Result<Program, CompileError>` in
  `mtc_turing_machine::parser`. Tasks 5, 6 and 7 depend on it.
- Consumes: the same two already-public functions Task 1 consumed, TM's copies.

- [ ] **Step 1: Write the failing test**

In `crates/turing-machine/src/parser/tests.rs`, alongside the existing
`parse_src` helper:

```rust
/// The crate's one parse entry point: source in, `Program` out, with the
/// `WithComments` lex it needs done internally.
#[test]
fn parse_takes_source_and_lexes_with_comments_itself() {
    let src = "alphabet ab { '0', '1' }\nmachine {\n  tape t: ab;\n  entry state s {\n    ['0'] -> stop;\n  }\n}\n";
    let commented = "alphabet ab { '0', '1' }\n// lead\nmachine {\n  tape t: ab;\n  entry state s {\n    ['0'] -> stop;\n  }\n}\n";
    let bare = parse(src).expect("parses");
    let with_comment = parse(commented).expect("parses");
    // The comment sits on line 2, AFTER the alphabet, so the alphabet's
    // spans are unmoved and the two must compare equal. Anything the
    // comment precedes would shift by a line — this is why the
    // comparison is scoped to `alphabets` and not to the whole program.
    assert_eq!(bare.alphabets, with_comment.alphabets);
    assert!(bare.machine.is_some());
}
```

Both sources were run through the real TM parser at authoring time and both
parse; the `alphabets`-equal and `machine.is_some()` assertions were both
measured true, not assumed (`[[plan-fixtures-must-parse]]`). If a grammar change
has landed since, fix the fixture rather than the assertion.

- [ ] **Step 2: Run it and watch it fail**

```
cargo test -p mtc-turing-machine --lib parser::tests::parse_takes_source 2>&1 | tail -20
```

Expected: COMPILE error, `expected &[Token], found &str`.

- [ ] **Step 3: Redefine `parse`**

Same shape as Task 1 Step 3, in `crates/turing-machine/src/parser.rs`:

```rust
/// Source → AST, through the one parse path this crate has: a
/// `WithComments` lex, the green syntax tree, then extraction
/// (docs/core.md (syntax trees)).
///
/// The convenience wrapper for callers that want only the `Program` —
/// it keeps nothing else, and it is the only parse function here that
/// yields one. A caller needing the token stream alongside it uses
/// `compiler::analyze`; a caller needing the green tree uses
/// `compiler::analyze_staged`, which is the one that retains it.
pub fn parse(source: &str) -> Result<Program, CompileError> {
    let tokens = lex_with(source, LexMode::WithComments)?;
    let green = parse_green_from_tokens(source, &tokens)?;
    Ok(crate::syntax::extract_program(
        &SyntaxNode::new_root(green),
        source,
    ))
}
```

- [ ] **Step 4: Run the new test**

```
cargo test -p mtc-turing-machine --lib parser::tests::parse_takes_source 2>&1 | tail -10
```

Expected: 1 passed.

- [ ] **Step 5: Migrate the five call sites**

- `parser/tests.rs:11-13` — `parse_src` becomes `fn parse_src(src: &str) -> Result<Program, CompileError> { parse(src) }`; consider deleting it and calling `parse` directly, but only if the diff stays small.
- `parser/tests.rs:1069-1074` — `parse_equals_lower_cst_after_parse_cst` is a
  C1 seam test. LEAVE IT for now; Task 6 deletes it with the seam. It still
  compiles because `parse_cst`/`lower_cst` still exist, but its
  `parse(&tokens)` call must become `parse(A5)`.
- `expand.rs:1690-1694` — the `machine_rules` helper drops its `lex` line:
  `let prog = parse(src).expect("parse");`
- `syntax/extract.rs:2539` — `crate::parser::parse(&lex(src).unwrap())` becomes
  `crate::parser::parse(src)`.
- `compiler.rs:4434` — `parse(&batch_tokens)` becomes `parse(&src)`. Rewrite the
  surrounding comment: it currently calls this a cross-front oracle resting on
  C1 struct-equality. It is now a check that `analyze_staged`'s tiered path and
  the plain entry point agree on the same source — state THAT, and state that
  the enforcement is the assertion itself, not the two paths happening to share
  a parser today.

Remove newly-unused `use crate::lexer::lex;` imports.

- [ ] **Step 6: Re-verify the one-parse-per-request guard**

The new `parse` calls `parse_green_from_tokens`, which **increments
`PARSE_GREEN_FROM_TOKENS_CALLS`** (`parser.rs:671`) — the counter behind plan
10's pinned property that a language-service request costs exactly one parse.

Measured at `b6b698e`: the counter is read in exactly one place,
`lsp/tests.rs:1908 one_parse_per_language_service_request_is_measured_not_assumed`,
which resets it to `0` and never calls `parse`; the counter is thread-local, so
no other test can perturb it. **This task therefore does not move the count** —
but confirm it rather than trusting this sentence, because the sentence is about
a neighbour:

```
grep -rn "PARSE_GREEN_FROM_TOKENS_CALLS" crates/turing-machine/src
cargo test -p mtc-turing-machine --lib lsp::tests::one_parse_per_language_service_request
```

Expected: the same four sites, and the test green. If a later task adds a
`parse` call inside that test's thread, the count changes and the guard is
measuring something else.

- [ ] **Step 7: Run the gates**

```
cargo fmt --check && cargo clippy -p mtc-turing-machine --all-targets -- -D warnings && cargo test -p mtc-turing-machine
```

- [ ] **Step 8: Commit**

```bash
git add -A crates/turing-machine
git commit -m "refactor(turing-machine): parse takes source and runs the green tree"
```

---

## Task 3: MEASURE what the oracles were actually covering

This task changes no committed source. Its deliverable is a table in the
ledger, and every later task's scope depends on it. **Do not skip it and do not
substitute reasoning for it** — the whole point is that plan 6 shipped two
Criticals past eight green reviews because nobody measured what the tests
reached.

**Files:**
- Create: `.superpowers/sdd/2026-08-30-c2-plan12-arc-closer/break-table.md`
- Modify (temporarily, never committed): the mutation points named below

**Interfaces:**
- Produces: `break-table.md`, a per-mutation record of which test files go RED
  with the oracles removed. Task 4 implements exactly the rows that come back
  "nothing catches it".

- [ ] **Step 1: Put the tree in the post-deletion state, uncommitted**

Move the file out of `tests/` rather than deleting it — cargo stops compiling
it, and `git checkout` at Step 4 is not needed to get it back. Use the session
scratchpad, never a path inside the repo (`[[probes-belong-in-scratchpad]]`).

```bash
mv crates/turing-machine/tests/syntax_parity.rs "$SCRATCH/syntax_parity.rs.keep"
cargo test --workspace 2>&1 | tail -5
```

Expected: green (one fewer test file). Do the same for the four C1 tests in
`crates/post-machine/tests/syntax_green.rs`
(`error_parity_with_parse_cst`, `corpus_acceptance_parity`,
`corpus_extraction_parity`, `nested_main_stays_unexported`), the
`extraction_parity` proptest in `tmc_property.rs`, and the seven
`reparsed_X_equals_the_c1_X` tests plus `agrees` in
`turing-machine/src/syntax/extract.rs` — comment them out rather than deleting,
so the tree restores with one `git checkout`.

- [ ] **Step 2: Run each mutation and record what goes red**

For each row: apply the mutation, run `cargo test --workspace 2>&1 | grep -E
"^(test result|failures:|    )" `, record every failing test file, revert.

| # | crate | mutation point | mutation | prediction |
|---|---|---|---|---|
| 1 | TM | `syntax/views.rs:592` `StateView::is_entry` | invert the returned bool | `tmc_golden.rs` RED (entry state moves) |
| 2 | TM | `parser.rs:3107` `reparse_pattern` | reverse the cell vector before returning | `tmc_golden.rs` RED (rules change) |
| 3 | TM | `syntax/extract.rs:671` | stamp `doc: None` instead of `extract_doc(…)` | lint/LSP RED |
| 4 | TM | `syntax/extract.rs` | shift an alphabet declaration's `span.start` by +1 | **nothing** — the hole |
| 5 | TM | `syntax/extract.rs` | stamp a state's `line` as `0` | **nothing** — the hole |
| 6 | PM | `syntax/extract.rs:279` | return `None` instead of `reduce_doc_run(&reparse_doc_items(&tokens))` | `deprecated-call` lint / hover RED |
| 7 | PM | `syntax/extract.rs:203` | pass `in_group: !in_group` to `reparse_item` | comma-group tests RED |
| 8 | PM | `syntax/extract.rs` | shift a function's `span.start` by +1 | **nothing** — the hole |

Rows 4, 5 and 8 are the ones this plan exists to find out about: the differential
compared every `line`/`col`/`span` field-for-field, and most downstream tests
never assert one. **The predictions in the last column are predictions.** Record
what actually happened, including where a prediction was wrong — a wrong
prediction here is the most valuable output this task can produce.

- [ ] **Step 3: Widen the mutation set to whatever the shims still own**

Fact 7: TM's seven `reparsed_X_equals_the_c1_X` tests plus `agrees` over 38
sources are the largest body of coverage being deleted. For each of the ten
`reparse_*` shims in `turing-machine/src/parser.rs:3054-3214`, add a row: a
value-changing mutation inside the shim, and what catches it with the oracles
gone. Ten more rows. This is the bulk of the task and it is the reason the task
exists.

- [ ] **Step 4: Restore the tree and write the table**

```bash
git checkout -- crates/
mv "$SCRATCH/syntax_parity.rs.keep" crates/turing-machine/tests/syntax_parity.rs
git status --short   # expect: clean
cargo test --workspace 2>&1 | tail -3
```

Write `break-table.md` with one row per mutation: point, mutation, files that
went RED, and a verdict of `covered` / `HOLE`. Then commit the ledger only.

- [ ] **Step 5: Commit**

```bash
git add .superpowers/sdd/2026-08-30-c2-plan12-arc-closer/break-table.md
git commit -m "docs(plan): measure what the C1 oracles were the only cover for"
```

---

## Task 4: Close the measured holes

**Files:**
- Modify: `crates/turing-machine/src/syntax/extract.rs` (test module),
  `crates/post-machine/src/syntax/extract.rs` (test module) — plus whatever
  Task 3's table names
- Test: the same files

**Interfaces:**
- Consumes: `break-table.md` from Task 3. Every row marked `HOLE` gets a test
  here; every row marked `covered` gets NOTHING, and the table is the record of
  why.

### MEASURED RESULT — this task is now one test and one decision

Task 3 measured 19 rows. **All three "nothing catches it" predictions in its
table FAILED**: rows 4, 5 and 8 are each caught by an independent non-oracle
test. Seventeen of nineteen rows are `covered`, with content mutations reddening
between 3 and 170 tests apiece. Do not write tests for those.

**The one genuine hole — write this test:** `Transition`'s own `span` field is
asserted NOWHERE in the crate. A uniform +1 on every variant's
`span.start.col` inside `reparse_transition`
(`turing-machine/src/parser.rs:3067`) produces zero failures, `--lib` and full
single-crate alike, while the CONTENT dimension of the same shim is covered by
18 tests. Close it with a test asserting a `Transition`'s exact span for at
least one variant, following the proven pattern of
`extraction_agrees_when_a_declarations_header_spans_lines` — a hard-coded
`assert_eq!` on the position triple, which is exactly the shape that already
covers `State`, `Alphabet` and `Tape`.

**The one decision, and it is NOT a test:** `reparse_sym_map`
(`parser.rs:3182`) is dead code — its only caller is `extract.rs:1480` inside
an oracle test Task 5 deletes, and it already carries `#[allow(dead_code)]`.
Ruled: it is DELETED in Task 7, not tested here. Writing a test to keep
unreachable code alive inverts the arc's purpose.

- [ ] **Step 1: For each HOLE row, write the test while C1 still exists**

This ordering is the point. C1 is still callable, so each new test's expected
value can be *validated* against `lower_cst(&parse_cst(&tokens))` at authoring
time and then written into the test as a literal — the same move plan 6 made for
its 75 converted fmt tests. The test that ships asserts a literal; the reference
that produced the literal was the C1 path, not the code under test.

The shape, for a span hole:

```rust
    /// Spans on extracted declarations, pinned by value. The differential
    /// oracle compared every `line`/`col`/`span` field-for-field against
    /// the C1 lowering; with that gone, nothing downstream reads most of
    /// them — a diagnostic points at a span, but no test asserts the span
    /// of a declaration that never errors.
    ///
    /// Enforcement is this assertion. The values were validated against
    /// the C1 lowering before it was deleted; they are literals now
    /// because there is no second implementation left to ask.
    #[test]
    fn declaration_spans_are_pinned_by_value() {
        let src = "alphabet ab { '0', '1' }\n";
        let p = crate::parser::parse(src).expect("parses");
        let a = &p.alphabets[0];
        // `Alphabet` carries `name_span`, `line` and `col` — there is no
        // whole-declaration `span` field on it. Values measured against
        // the C1 lowering of this exact source before C1 was deleted.
        // `Pos` comes from `mtc_core::diagnostics` — `parser.rs:23`
        // already imports it as `use mtc_core::diagnostics::{Pos, Span};`,
        // so a test module with `use super::*;` needs no new import.
        assert_eq!(a.name_span.start, Pos { line: 1, col: 10 });
        assert_eq!(a.name_span.end, Pos { line: 1, col: 12 });
        assert_eq!((a.line, a.col), (1, 1));
    }
```

**Every literal in a test you write here must be validated, not guessed.** Print
it from the C1 path first:

```
cargo test -p mtc-turing-machine --lib <your_test> -- --nocapture
```

with a temporary `dbg!` on the C1 side, THEN write the literal.

- [ ] **Step 2: Prove each new test discriminates**

For each test written in Step 1, re-apply the mutation from its `break-table.md`
row and confirm the new test goes RED by name. A test that passes under its own
mutation is not coverage — `[[fixtures-must-discriminate]]`. Record the proving
mutation in the test's own doc comment.

- [ ] **Step 3: Run the gates**

```
cargo fmt --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace
```

- [ ] **Step 4: Commit**

```bash
git add -A crates/
git commit -m "test: pin what only the C1 differential was covering"
```

---

## Task 5: Delete both differential oracles

**Files:**
- Delete: `crates/turing-machine/tests/syntax_parity.rs`
- Modify: **`crates/turing-machine/tests/syntax_green.rs`** — added after Task 2
  discovered it (see measured fact 8): `errors_agree_with_the_cst_path` and its
  acceptance sibling are C1 oracles that Task 2 rewrote onto explicit
  `parse_cst`/`lower_cst`. They are C1 oracles, so retiring them is THIS task's
  job. Convert each to assert its expected error/acceptance by value, captured
  from the C1 path before it is deleted — same protocol as the rest of Step 4.
- Modify: `crates/post-machine/tests/syntax_green.rs` (four tests),
  `crates/turing-machine/tests/tmc_property.rs` (`both_paths`, the
  `extraction_parity` proptest, `REQUIRED_CONSTRUCTS`, `stamp_*`),
  `crates/turing-machine/tests/syntax_views.rs:795-813`,
  `crates/turing-machine/tests/tmc_green_analyze.rs` (doc lines only)
- Modify: `crates/turing-machine/src/syntax/extract.rs`,
  `crates/post-machine/src/syntax/extract.rs` (the C1-comparison test bodies)

**Interfaces:**
- Produces: a `tmc_property.rs` with no C1 half, whose `REQUIRED_CONSTRUCTS`
  labels are derived from GENERATOR BRANCHES rather than stamped from the
  extracted `Program`.

- [ ] **Step 0: Partition `REQUIRED_CONSTRUCTS` into chosen and derived**

Do this BEFORE threading anything. `stamp_program` reads the extracted
`Program`; the generator emits TEXT. Some labels are *chosen* — the generator
takes a branch and the label follows — and some are *derived*, a consequence of
a shape rather than a decision. `sym.number.leading-zero` fires on
`written.len() > 1 && written.starts_with('0')`; `doc.paragraphs.one` fires when
a fold happens to produce exactly one paragraph. **The generator cannot record
a derived label without duplicating the derivation**, and a two-directional
set-compare over them would fail on precisely the interesting cases.

Read every `stamp_*` arm (`tmc_property.rs:1529-1946`) and write the partition
into the ledger: label, arm, `chosen` or `derived`. Then:

- **chosen** labels get the two-directional set-compare of Step 1;
- **derived** labels stay floor-only — `REQUIRED_CONSTRUCTS` still requires them
  to be observed across the sweep, which is what they can honestly carry.

Say which is which in the test's doc comment. An implementer who discovers this
mid-threading either fudges the assertion or stalls — plan 8's task 6 stalled at
exactly this kind of step.

- [ ] **Step 1: Re-derive the coverage floor from the generator**

Today `the_generator_reaches_every_construct_extraction_rebuilds` calls
`stamp_program(&actual, …)` — it stamps labels off the extracted `Program` and
compares that set against `REQUIRED_CONSTRUCTS` in both directions. That was a
real oracle while C1 was the other side of `both_paths`. With C1 gone it is
extraction agreeing with itself.

Make the generator the other side: `generate_program` records, per program, the
set of construct labels it CHOSE to emit; the test compares the generator's
recorded set against `stamp_program`'s set over the same program.

```rust
/// What the generator DECIDED to emit, recorded as it decides — the
/// other side of the coverage oracle now that the C1 lowering is gone.
///
/// The invariant: for every generated program, the labels the generator
/// recorded and the labels `stamp_program` reads back off the extracted
/// `Program` are the SAME SET. Enforcement is the set-compare below,
/// in both directions: a construct the generator emits and extraction
/// drops fails, and so does a label extraction invents.
#[derive(Default)]
struct EmittedConstructs(BTreeSet<&'static str>);
```

Thread an `&mut EmittedConstructs` through `gen_top_item` and its callees,
inserting the same label strings `stamp_program` uses at the point the branch is
taken — **for the CHOSEN labels only**, per Step 0. `REQUIRED_CONSTRUCTS` stays
as the floor over the whole sweep and remains the only guard on the derived
ones.

This is the largest single piece of work in Task 5 (~90 labels to partition,
~10 generator functions to thread). Budget accordingly.

- [ ] **Step 2: Prove the re-derived floor bites**

Re-apply mutation 3 from `break-table.md` (a routine's `doc` dropped in
extraction) and confirm the new set-compare fails by name — the generator
recorded `routine.documented`, extraction no longer stamps it.

```
cargo test -p mtc-turing-machine --test tmc_property 2>&1 | tail -20
```

Expected: RED, naming the missing label. Revert the mutation; expected GREEN.

- [ ] **Step 3: Strip the C1 half out of `both_paths`**

`both_paths` becomes a single-path helper (rename it `extracted`), and the
`extraction_parity` proptest is deleted — its lossless sibling
`generated_programs_round_trip` stays. Update the module doc: it currently
explains the corpus/generator division of the oracle, and that sentence dies
with the oracle.

- [ ] **Step 3b: Retire the FIVE oracle sites the plan never named**

Task 3 found these empirically — 12 test functions the plan's file-level list
missed. My inventory was by FILE and by unaliased grep; these needed a
test-function-by-test-function read. All are live C1-vs-green comparisons and
all must go with the rest:

| site | shape |
|---|---|
| `turing-machine/src/parser/tests.rs::parse_equals_lower_cst_after_parse_cst` | direct `assert_eq!(lower_cst(&parse_cst(…)), parse(A5))` — live because `parse` now runs the green path |
| `turing-machine/tests/syntax_views.rs::each_new_nodes_extent_equals_the_ast_span_it_carries` | green `text_range()` extents vs the C1 lowering's AST spans |
| `post-machine/src/stdlib/mod.rs::roster_matches_the_c1_cst_walk` | green roster vs an inline C1 CST walk |
| **`post-machine/src/syntax/extract.rs` — SIX tests in its own `mod tests`**: `reparsed_item_equals_the_c1_item`, `reparsed_doc_items_equal_the_c1_doc_run`, `reparsed_doc_items_reduce_to_the_same_fndoc_when_comments_interleave`, `extracted_function_equals_lowered`, `extracted_program_equals_lowered`, `extracted_program_pins_namespace_scoped_import_and_nested_function_ns` | PM's exact equivalent of TM's seven `reparsed_X_equals_the_c1_X` + `agrees` family; the plan named TM's and never PM's |
| `turing-machine/tests/tmc_property.rs::the_generator_reaches_every_construct_extraction_rebuilds` | piggy-backs the same `both_paths()`-derived `assert_eq!` as the proptest — Step 1 rebuilds this one rather than deleting it |

Two functions Task 3 confirmed are NOT oracles, checked rather than inherited
from `CLAUDE.md`: `expand.rs`'s `machine_rules` helper and `extract.rs`'s
fixture smoke check. Both read fields off a fixture with no green comparison.
Leave them.

- [ ] **Step 4: Delete the remaining oracle sites**

```bash
git rm crates/turing-machine/tests/syntax_parity.rs
```

In `crates/post-machine/tests/syntax_green.rs`: delete
`error_parity_with_parse_cst`, `corpus_acceptance_parity` and
`corpus_extraction_parity`. Keep `corpus_lossless_law` (green-only, still real)
and `corpus()`. Convert `nested_main_stays_unexported` to assert its facts
directly off `extract_program` — the C1 half was only supplying the expected
value, and the test's actual claim (a nested `main` never auto-exports) is a
one-line assertion.

In `crates/turing-machine/tests/syntax_views.rs:795-813` and
`crates/turing-machine/src/syntax/extract.rs`'s seven
`reparsed_X_equals_the_c1_X` tests: convert each expected value to a literal
validated against C1 before removal (same protocol as Task 4 Step 1). **Do not
delete these tests** — fact 7 says they are the shims' only fidelity pin.

**The `agrees()` trap — measured, and it would silently undo Task 3's work.**
This task owns the `agrees()` helper (`extract.rs:1696`, 38 sources). Two of
its callers —
`extraction_agrees_when_a_declarations_header_spans_lines` and
`extraction_anchors_positions_on_header_tokens_not_on_node_starts` — carry
their OWN hard-coded `assert_eq!` after the `agrees()` call, and Task 3
measured that those hard-coded assertions are **the only cover** for a
declaration's `line`/`col`/`name_span` (its rows 4 and 5). Delete the
`agrees()` CALL inside each and keep everything after it. A mechanical pass
that deletes every test function touching `agrees()` recreates exactly the two
holes Task 3 proved were closed. Check every other `agrees()` caller the same
way — the list runs from `extract.rs:1901` onward.

In `crates/turing-machine/tests/tmc_green_analyze.rs`: doc-comment references
only, lines 4 and 88. Rewrite them to name the invariant, not C1.

- [ ] **Step 5: Run the gates**

```
cargo fmt --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace
```

Expected: green, with the workspace test count down by the deleted oracles and
up by Task 4's additions. Record both numbers in the ledger.

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "test: retire the C1 differential oracles"
```

---
# PHASE B — retire the CST

## Task 6: PM — retire `parse_cst`/`lower_cst` and rehome the leaf types

**Files:**
- Modify: `crates/post-machine/src/parser.rs` — delete `parse_cst` (`:359`),
  `lower_cst` (`:428`) and `lower_items`; move `DocRunItem`, `DocRunKind`,
  `AttrCst` in from `cst.rs`
- Modify: `crates/post-machine/src/cst.rs` — remove the three moved types and
  the file's own `#[cfg(test)] mod tests`
- Modify: `crates/post-machine/src/stdlib/mod.rs:333-340`
- Modify: `crates/post-machine/src/syntax/extract.rs` (test module),
  `crates/post-machine/src/fmt/print.rs` (doc comments)
- Test: `crates/post-machine/src/parser.rs` test module

**Interfaces:**
- Produces: `DocRunItem`, `DocRunKind`, `AttrCst` at `crate::parser` (were
  `crate::cst`). `reduce_doc_run` and `reparse_doc_items` keep their signatures
  exactly. Tasks 8 and 10 depend on the types being at `parser`.
- Removes: `parser::parse_cst`, `parser::lower_cst`. Nothing outside tests
  consumed them (measured: zero production callers since plan 6).

- [ ] **Step 1: Move the three leaf types, mechanically, in their own commit**

Cut `DocRunItem` (`cst.rs:291`), `DocRunKind` (`:303`) and `AttrCst` (`:332`)
verbatim into `parser.rs`, next to `reduce_doc_run` at `:528`. Keep every doc
comment; fix only the intra-doc links that now resolve differently
(`[`crate::parser::lower_cst`]` inside them is about to die — Step 3 handles
those, do not pre-empt it here).

Delete `use crate::cst::{… DocRunItem, DocRunKind …}` entries from
`parser.rs:10-12` and add nothing — they are local now.

```
cargo check -p mtc-post-machine --all-targets 2>&1 | grep -E "^error" | head
```

Expected: errors only where `cst::DocRunItem`-style paths are still written.
Fix each to `parser::` and re-run until clean, then commit:

```bash
git add -A crates/post-machine
git commit -m "refactor(post-machine): doc-run leaf types live with the parser"
```

- [ ] **Step 2: Convert the CST-shape tests before deleting their input**

`parser.rs`'s test module holds tests that read `Cst` fields directly and will
not compile once `parse_cst` goes. They are NOT all disposable; sort them:

- **Layout/trivia tests** — `parse_cst_captures_comment_trivia_and_layout`
  (`:2217`), `parse_cst_records_comma_group_newline_before` (`:2305`). These
  assert facts the FORMATTER now owns. `tests/fmt_programs.rs` and
  `tests/fmt_expected/` cover them by value. **Delete**, and record in the
  ledger which fmt fixture covers each — if none does, that is a HOLE and it
  goes back to Task 4's protocol.
- **Extent/span tests** — `function_and_namespace_extent_spans` (`:2768`),
  `volatile_extent_and_has_volatile_are_recorded_on_the_cst` (`:2812`).
  **Convert** to read the green tree: `parse_green(src)` then the typed view's
  `text_range()`. Remember the measured retro-wrap rule — a green FUNCTION node
  starts a line or more before the C1 `FunctionCst.span` did, so the expected
  START differs; the END reproduces exactly.
- **Doc-run tests** — the sixteen `doc_run_*` and `fn_doc_*` tests
  (`:2934-3360`). These read `f.doc_run`, which survives: rewrite each to build
  its run through `reparse_doc_items(&tokens)` (the extraction path's own
  producer) rather than through `parse_cst`. **Convert, do not delete** — they
  are the only per-shape pins on doc-run reduction.
- Everything else in the module already goes through `parse_src`/`parse` and
  needs no change.

- [ ] **Step 3: Delete `parse_cst`, `lower_cst`, `lower_items`**

```bash
cd crates/post-machine/src
# delete the three functions by hand — they are contiguous blocks at
# parser.rs:359 (parse_cst), :428 (lower_cst) and the lower_items family below it
cargo check -p mtc-post-machine --all-targets 2>&1 | grep -E "^error" | head -30
```

Every remaining error names a caller. The expected set, all in tests:
`stdlib/mod.rs:333-340`, `syntax/extract.rs`'s test module,
`tests/syntax_green.rs`. Fix each by routing through `parse` or `parse_green`.

`stdlib/mod.rs:335`'s test parses the embedded stdlib through `parse_cst` — it
becomes `crate::parser::parse(SOURCE)`. Check what it asserts before rewriting;
if it asserts a CST-only fact, it belongs in the Step-2 sort, not here.

- [ ] **Step 4: Run the gates**

```
cargo fmt --check && cargo clippy -p mtc-post-machine --all-targets -- -D warnings && cargo test -p mtc-post-machine
```

**Also run the compiled-stdlib byte-identity gate** at BOTH opt levels — this
IS a compiler-path change, so unlike a formatter change the gate is a real
check here, not a negative control:

`pmt compile` is the command — it emits the `.pmo` object the gate compares.
`pmt build` links, and `std.pmc` has no `main` to link. Verified against
`pmt compile --help`.

```bash
# AT THE TASK'S BASE COMMIT, before any edit:
cargo build --release --bin pmt
for O in 0 1; do
  ./target/release/pmt compile crates/post-machine/src/stdlib/std.pmc \
    -O$O -o "$SCRATCH/std-O$O-before.pmo"
done
# after the edit, rebuild and repeat into -after.pmo, then:
cmp "$SCRATCH/std-O0-before.pmo" "$SCRATCH/std-O0-after.pmo"
cmp "$SCRATCH/std-O1-before.pmo" "$SCRATCH/std-O1-after.pmo"
```

Reference sizes at `b6b698e`: 544 bytes at `-O0`, 547 at `-O1`. TM's twin is
`tmt compile crates/turing-machine/src/stdlib/std.tmc -O$O -o …tmo` (6103 bytes
at `-O0`).

**The assumption this makes, stated so it cannot rot silently:** the in-process
gate compiles `stdlib::SOURCE`, which is `include_str!("std.pmc")`; this command
compiles the FILE. They are the same bytes only while the `include_str!` points
at that path. If the embedding ever changes, this command stops being the gate.

- [ ] **Step 5: Commit**

```bash
git add -A crates/post-machine
git commit -m "refactor(post-machine): retire the C1 parse and lowering"
```

---

## Task 7: TM — retire `parse_cst`/`lower_cst` and rehome the leaf types

**Files:**
- Modify: `crates/turing-machine/src/parser.rs` — delete `parse_cst` (`:617`),
  `lower_cst` (`:695`) and its lowering family; move `DocRunItem` (`cst.rs:385`),
  `DocRunKind` (`:392`), `AttrCst` (`:410`) in
- Modify: `crates/turing-machine/src/cst.rs`, `src/parser/tests.rs`,
  `src/syntax/extract.rs`, `src/fmt/print.rs:180`, `src/syntax/mod.rs`,
  `src/lexer.rs` (one doc reference)
- Test: `crates/turing-machine/src/parser/tests.rs`

**Interfaces:**
- Produces: `DocRunItem`, `DocRunKind`, `AttrCst` at `crate::parser`. The two
  production imports that move are `fmt/print.rs:180` and
  `syntax/extract.rs:63`, both currently `use crate::cst::{DocRunItem, DocRunKind};`.
- Removes: `parser::parse_cst`, `parser::lower_cst`.

- [ ] **Step 1: Move the three leaf types**

Same mechanical move as Task 6 Step 1, into `parser.rs` next to
`reduce_doc_run` at `:885`. Then repoint the two production imports:

```rust
// crates/turing-machine/src/fmt/print.rs:180
use crate::parser::{DocRunItem, DocRunKind};
// crates/turing-machine/src/syntax/extract.rs:63
use crate::parser::{DocRunItem, DocRunKind};
```

```
cargo check -p mtc-turing-machine --all-targets 2>&1 | grep -E "^error" | head
git add -A crates/turing-machine && git commit -m "refactor(turing-machine): doc-run leaf types live with the parser"
```

- [ ] **Step 2: Convert the CST-shape tests in `parser/tests.rs`**

Twelve C1 references, in three groups:

- `volatile_tape_parses_in_a_machine_block` (`:456`) reads `TapeCst` fields off
  `parse_cst`. **Convert** to `parse(src)` and read `Program`'s tapes — the
  facts asserted (a `volatile` tape parses, and the flag lands) are AST facts,
  not CST facts. Verify that claim by reading the assertions before converting.
- `parse_equals_lower_cst_after_parse_cst` (`:1068`) is the seam contract
  itself. **Delete** — the seam is gone; there is no second path to equal.
- The rest reference `DocRunKind`/`ReuseCarrier`/`RuleKind`/`TopKind`/
  `WorldKind` in `let … else` patterns over `parse_cst` output. Sort them the
  way Task 6 Step 2 sorts PM's: extents and doc runs convert, layout/trivia
  facts delete only with a named fmt fixture that covers them.

- [ ] **Step 3: Delete `parse_cst`, `lower_cst` and the lowering family**

```
cargo check -p mtc-turing-machine --all-targets 2>&1 | grep -E "^error" | head -30
```

Expected callers to fix: `syntax/extract.rs`'s test module (`:1096-1700`,
including the `agrees` helper Task 5 already converted), `compiler.rs`'s staged
test, `tests/syntax_views.rs`, `tests/tmc_property.rs`. If Tasks 2 and 5 were
done correctly, all four are already off C1 and this step is a no-op for them —
if one is not, that is a Task 5 miss, and it goes back rather than being patched
here.

- [ ] **Step 4: Run the gates, including TM's standing ones**

```
cargo fmt --check && cargo clippy -p mtc-turing-machine --all-targets -- -D warnings && cargo test -p mtc-turing-machine
```

Plus the everything-matrix and the TM stdlib byte-identity pair, same protocol
as Task 6 Step 4 with `tmt`:

```
cargo test -p mtc-turing-machine --test opt_equivalence everything_matrix_is_green
```

- [ ] **Step 5: Commit**

```bash
git add -A crates/turing-machine
git commit -m "refactor(turing-machine): retire the C1 parse and lowering"
```

---

## Task 8: TM — strip CST construction out of the parser

After Task 7, TM's `cst.rs` container types are reachable ONLY from the
parser's own productions, which build them and drop them on every green parse
(measured fact 2). This task removes that.

**Files:**
- Modify: `crates/turing-machine/src/parser.rs` (the production bodies, the
  `Parser` struct, `file()`, `parse_green_from_tokens`)

**Interfaces:**
- Produces: `Parser` with a non-`Option` `sink: GreenSink` and no `comments`,
  `cpos` or `prev_end_line` fields; `file()` returning
  `Result<GreenSink, CompileError>`; container productions returning
  `Result<(), CompileError>`.
- Preserves exactly: `parse_green`, `parse_green_from_tokens` signatures and
  behaviour; all ten `reparse_*` shims; `doc_run()`'s token consumption.

- [ ] **Step 1: Name the trap before touching anything**

`parser.rs:1811` (PM's copy; TM has the analogue) carries a comment about the
significant-token walk and `self.peek()`. **Before stripping, establish whether
the comment walk (`self.comments`, `self.cpos`, `capture_open_trailing`,
`capture_close_trailing`, `interior_comments`) can affect which SIGNIFICANT
token the parser consumes next.** Measure it: make `split_comments` return an
empty comment vector for the parser's own use, run the whole suite, and see
whether any acceptance test changes.

If it can, this task stops and the finding goes back to the maintainer. If it
cannot — the expected answer, since the green tree takes its trivia from
`syntax::layout` and not from this walk — record the measurement in the ledger
and continue.

- [ ] **Step 2: Make the sink unconditional**

63 `self.g_*` call sites in this file guard on `Option`. With C1 gone there is
no sink-less parse:

```rust
    fn g_flush_start(&mut self, kind: TmcKind) {
        self.sink.flush_start(kind);
    }

    fn g_finish(&mut self) {
        self.sink.finish();
    }

    fn g_checkpoint(&mut self) -> Checkpoint {
        self.sink.checkpoint()
    }

    fn g_start_at(&mut self, cp: Checkpoint, kind: TmcKind) {
        self.sink.start_at(cp, kind);
    }
```

`bump()` loses its `if let Some(sink)`. `parse_green_from_tokens` stops
threading `Some(sink)` and stops unwrapping with the
`"parse_green_from_tokens always seeds a sink before calling file()"` expect —
that expect exists only because the field is optional, and it goes with it.

Run the full suite after this step alone, before touching production return
types. A regression here is a green-tree regression and it must not be
entangled with the next step's diff.

- [ ] **Step 3: Drop the container return values, one production family at a time**

Order: leaves first, containers last, so each intermediate state compiles.
`parse_alphabet`, `parse_namespace`, `parse_machine`, `parse_bind`,
`parse_graft`, the world/state/rule families, then `file()`.

For each: change `Result<XCst, CompileError>` to `Result<(), CompileError>`,
delete the struct literal at the end, and delete the local bindings that fed it
ONLY — never a `self.expect(…)` or `self.bump()` call, which drive the walk.
`capture_open_trailing`/`capture_close_trailing`/`interior_comments` calls go
with their fields; `self.name(…)` calls STAY (they consume tokens and produce
errors) with their return values dropped.

`doc_run()` keeps consuming its tokens and stops returning
`(Vec<DocRunItem>, Span)` — but **`reparse_doc_items` keeps building
`Vec<DocRunItem>`**, because extraction retokenizes and re-runs it. That
asymmetry is the point of the leaf-type rehome; state it in `doc_run`'s doc.

After each family: `cargo test -p mtc-turing-machine`. Commit per family, not
at the end — this is the highest-risk edit in the plan and a bisectable history
is worth the extra commits.

- [ ] **Step 4: Delete the now-dead `Parser` fields**

`comments`, `cpos`, `prev_end_line` and the helpers that read them. Clippy names
them; `-D warnings` makes it mandatory.

- [ ] **Step 5: Run every standing gate**

```
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo test -p mtc-turing-machine --test opt_equivalence everything_matrix_is_green
cargo build -p mtc-core --no-default-features
git diff --stat <task-base>..HEAD -- crates/core crates/post-machine
```

The last one must be EMPTY. A TM-only task showing a PM or core diff is a design
smell, not a convenience.

- [ ] **Step 6: Commit**

```bash
git commit -m "refactor(turing-machine): the parser builds only the green tree"
```

---

## Task 9: PM — strip CST construction out of the parser

Identical in shape to Task 8, against `crates/post-machine/src/parser.rs`:
27 `self.g_*` sites, container types `Cst`, `TopItem`, `TrailingComment`,
`UsePath`, `UseCst`, `NamespaceCst`, `FunctionCst`, `BodyItem`, `CommaItem`,
`StatementCst`.

**Files:**
- Modify: `crates/post-machine/src/parser.rs`

**Interfaces:**
- Produces: the same shape Task 8 produced, PM's copy. `reparse_item` and
  `reparse_doc_items` keep their signatures.

- [ ] **Step 1: Repeat Task 8 Step 1's trap measurement for PM**

Do not carry TM's answer over. The two parsers were written independently and
PM's comment walk has its own `label_break` handling, which TM has no analogue
of.

- [ ] **Step 2: Make the sink unconditional** — same edit as Task 8 Step 2, 27
  sites, `PmcKind` instead of `TmcKind`.

- [ ] **Step 3: Drop the container return values** — leaves first: `statement`,
  `comma_item`, `body_item`, `function`, `namespace`, `use_decl`, then `file()`.
  `doc_run()` keeps consuming and stops returning, `reparse_doc_items` keeps
  building. Commit per family.

- [ ] **Step 4: Delete the dead `Parser` fields** — PM's are `comments`, `cpos`,
  `prev_end_line`, plus whatever `label_break` bookkeeping fed only the CST.

- [ ] **Step 5: Run the gates, PM-1 byte-identity included**

```
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo test -p mtc-post-machine --test golden_programs
cargo test -p mtc-post-machine --test asm_volatile
git diff --stat <task-base>..HEAD -- crates/core crates/turing-machine
```

The goldens and `asm_volatile` ARE the PM-1 byte-identity gate. The last command
must be empty.

- [ ] **Step 6: Re-confirm every `HOLE` row against the post-excision tree**

Task 3 measured the coverage holes in a tree that **no longer exists**: the
`Parser` has since lost `comments`, `cpos` and `prev_end_line`, the productions
return `()`, and the sink is unconditional. A mutation that nothing caught then
may be caught now, and — the case that matters — a Task 4 test may have been
pinned against a shape the excision changed.

For every row marked `HOLE` in `break-table.md`, re-apply its mutation here and
confirm the Task 4 test written for it still fails BY NAME:

```
# per row: apply the mutation, then
cargo test --workspace 2>&1 | grep -E "^(failures:|    )" | head
# revert, then confirm green
```

Append the re-confirmation to `break-table.md` as a second column. A row that
no longer bites is a finding, not a formality — it means the excision moved the
behaviour the test was pinning, and it goes back to the maintainer.

- [ ] **Step 7: Commit**

```bash
git commit -m "refactor(post-machine): the parser builds only the green tree"
```

---

## Task 10: Delete both `cst.rs`, and sweep the prose they leave behind

**Files:**
- Delete: `crates/post-machine/src/cst.rs`, `crates/turing-machine/src/cst.rs`
- Modify: `crates/{post-machine,turing-machine}/src/lib.rs` (the `mod cst;`
  declarations), plus every file the grep gate below still names
- Modify: `CLAUDE.md`

**Interfaces:**
- Produces: two crates with no `cst` module. Nothing else changes.

- [ ] **Step 1: Delete the modules**

```bash
git rm crates/post-machine/src/cst.rs crates/turing-machine/src/cst.rs
# remove `mod cst;` / `pub mod cst;` from both lib.rs files
cargo check --workspace --all-targets 2>&1 | grep -E "^error" | head
```

Expected: clean. If anything still references the module, Task 8 or 9 left
construction behind — go back, do not patch here.

- [ ] **Step 2: The prose sweep, with a completeness gate as its exit criterion**

This is where this arc bleeds. Plan 8 shipped eleven false doc sentences; plan 7
shipped three. **Every doc comment naming `parse_cst`, `lower_cst`, `Cst` or
"C1" became false the moment those functions went.** Measured before this plan
started: 65 references in PM's `parser.rs`, 49 in TM's, 22 in PM's
`syntax/extract.rs`, 33 in TM's, 7 in PM's `fmt/print.rs`, 2 in TM's.

The gate:

```bash
grep -rn "parse_cst\|lower_cst\|crate::cst\|\bC1\b" \
  crates/post-machine/src crates/turing-machine/src
```

Exit criterion for THIS gate: **zero hits**, other than a deliberate historical
mention you can name. It is scoped to the two arch crates' `src/` on purpose —
`crates/core/src` legitimately owns the asm CST, and `docs/core.md` legitimately
describes syntax trees, so including either makes the criterion unsatisfiable
and it gets waived instead of met.

`docs/` gets a SEPARATE, judgment-based pass in the same step: read
`docs/core.md (syntax trees)`, `docs/lsp.md` and `docs/tmt/fmt.md` for sentences
that describe a two-path world. Plan 4 already searched published `docs/` for
stale C1-path claims and found nothing needing correction — so this is a read,
not an expected edit, and finding nothing is the likely outcome.

Two specific debts this task clears for free:

- `syntax/extract.rs`'s doc comments cite `parser.rs:395` and `parser.rs:402`
  **by line number**, and those lines died with the functions. Replace with the
  invariant, not with a new line number.
- Six of the 21 non-resolving `docs/tmt/fmt.md (interior comments)` citations
  live in `cst.rs` and die with the file. The other 15 (9 in `parser.rs`, 5 in
  `fmt.rs`, 1 in `parser/tests.rs`) are pre-existing debt — the page's heading
  is `## Comments`, not `interior comments`. Fix the ones in files this plan
  already touches; leave the rest. No retroactive sweep.

**Write each replacement sentence FROM the code, not from the sentence it
replaces.** State the invariant and its enforcement — never the current
neighbour's reason for satisfying it.

- [ ] **Step 3: Update `CLAUDE.md`**

Two paragraphs are now false end to end:

- `### Pipeline and key types` — the whole "`parse` = `lower_cst ∘ parse_cst`
  over the C1 CST survives only as the differential oracle" clause, and the
  paragraph beginning "**The compiler front end, the `.pmc` language service,
  and `fmt` all run the green tree.**" Its last sentence ("Removing the C1 CST
  itself — in this crate and in the TM one — is later work") is what this plan
  just did.
- `### The `.tmc` front end` — the "**Oracle**: `parser::parse` =
  `lower_cst ∘ parse_cst`" sentence and the THIRD-oracle-site sentence about
  `compiler.rs`'s staged-vs-batch test, whose meaning Task 2 Step 5 changed.

Rewrite both to standing state: one parse path per crate, source → green tree →
extraction, with `fmt` printing from the raw tree. Keep the file at standing
state — the arc's story goes to `docs/superpowers/build-history.md`, not back
into the preamble.

- [ ] **Step 4: Run every gate in the repo**

```
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo build -p mtc-core --no-default-features
```

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "refactor: delete the C1 CSTs"
```

---

## Task 11: Close the arc

**Files:**
- Modify: `docs/superpowers/build-history.md`
- Create: `.superpowers/sdd/2026-08-30-c2-plan12-arc-closer/progress.md` (kept —
  `[[sdd-artifacts-keep]]`)

- [ ] **Step 1: Verify #98's plan survives unedited**

`docs/superpowers/plans/2026-08-30-tmc-comments-never-move.md` runs next and
touches `fmt/print.rs`, which this plan changed (one import line, several doc
comments). Read its task list against the current file and confirm every line
number and quoted snippet it names still resolves. **If any does not, fix the
#98 plan in this commit** — a stale plan is a defect this plan introduced.

- [ ] **Step 2: Rebase onto master and verify the contribution**

```bash
git fetch origin master
git rebase origin/master
cargo test --workspace
```

Per the arc's history `CLAUDE.md` is the file both sides keep touching; expect
to resolve it and expect it to merge cleanly otherwise. Verify the branch's own
contribution is byte-identical outside `CLAUDE.md` after the rebase.

- [ ] **Step 3: Measure and record the arc's totals**

```bash
git diff --stat origin/master..HEAD -- crates/ | tail -1
cargo test --workspace 2>&1 | grep -E "^test result" | \
  awk '{s+=$4} END {print "workspace tests:", s}'
```

Record: total workspace test count, lines deleted, and the per-parse allocation
that measured fact 2 identified as now gone.

- [ ] **Step 4: File the arc's follow-ups**

As issues on `mellonis/machine-toolchains`, not as code:

- error-resilient parsing (deferred by ruling at the arc's start)
- the asm CST onto the `syntax/` framework
- incremental reparse
- the two `.pmc` fmt behaviours plan 11 deliberately preserved — the
  keyword/name relocation and its two-pass settle — routed to #98's systemic
  comment-preservation work
- `views.rs`'s raw-`SyntaxKind(u16)` assert and `NamespaceView::name_token`'s
  unconditional `Vec` (plan 4's two remaining deferred follow-ups)
- the 15 surviving non-resolving `docs/tmt/fmt.md (interior comments)`
  citations

- [ ] **Step 5: Write the ledger and STOP**

Write the progress ledger. **Do not merge** — the arc merges once, and that is
the maintainer's call. Do not delete `.superpowers/sdd/` history.

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "docs(plan): close the C2 green-tree arc"
```

---

## Self-review

**Spec coverage.** The arc spec's cutover requirements map to tasks as: one
parse path per crate → Tasks 1–2; oracles retired without coverage loss →
Tasks 3–5; CST deleted → Tasks 6–10; owned compiler vocabulary surviving as
per-crate `hir.rs` → already satisfied, the leaf value types stay in `parser.rs`
and Task 6/7's rehome extends that rather than changing it.

**Known gaps, stated rather than hidden.**

1. **Task 3's sweep is a sample.** Eighteen mutations over two crates cannot
   prove the absence of a hole. Mitigation: the mutation points are chosen from
   the oracle's OWN field list (it compared every `line`/`col`/`span`), not from
   intuition. A field no mutation touches stays unpinned, and that is a stated
   residual risk, not an oversight.
2. **Task 8 Step 1's trap could stop the plan.** If the comment walk turns out
   to affect significant-token consumption, Phase B's shape changes and the task
   goes back to the maintainer. That is deliberate: discovering it mid-strip
   would be far worse.
3. **Task 6/7 Step 2's sort is a judgment call per test.** The rule is stated
   (layout facts delete only with a NAMED fmt fixture covering them; extents and
   doc runs convert) but applying it needs reading each test. Budget it.
4. **The residual risk plan 11 named still applies**: a bug present in both the
   green path and C1 at capture time is enshrined rather than exposed. Deleting
   C1 does not create that risk; it removes the last chance to find it.
