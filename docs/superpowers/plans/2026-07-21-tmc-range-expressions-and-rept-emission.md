# `.tmc` Range Expressions and `.rept` Emission Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement the approved spec `docs/superpowers/specs/2026-07-21-tmc-range-expressions-and-rept-emission-design.md` — `%` fold expressions, the optional-transition sugar, the `.rept` re-detection emitter, the never-fires compile warnings, and the six-rule lint wave — closing issues #31, #33, #35, #43, #44, #45, #46, #47, #48 (#38 scoped).

**Architecture:** All work lives in `crates/turing-machine` (plus `docs/` and `editors/grammars/`). The write-fold substitution grows to the assembler's expression grammar and is evaluated at expansion time; the emitter re-compresses stamped assembly text after codegen with an assemble-both byte-identity self-check; warnings ride `CompileReport` (the `expansion-threshold` precedent); lint rules join the existing `.tmc` and `.tma` layers.

**Tech Stack:** Rust, cargo workspace, proptest (dev-dep), hand-rolled CLI. No new dependencies.

## Global Constraints

Copied from the spec and standing repo rules — every task implicitly includes these:

- **`crates/core` is untouched** — `git diff master -- crates/core` must be empty at every commit. The emitter's self-check assembles both texts through core's existing public `assemble` instead of needing new core surface.
- **PM-1 byte-identity**: `git status --short crates/post-machine/tests/golden/` stays empty; PM-1 tests all green.
- **`-O0` floor**: `-O0` **object bytes** unchanged by compression (proven by the self-check); `-S` text may change at all `-O` levels.
- **`TMC_LANG_VERSION` stays `"0.1"`** (maintainer ruling — unpublished contract). `TM1_TMA_DIALECT_VERSION` stays `"0.3"`. `TM_IR_VERSION` stays `2`.
- **Negative remainder, zero modulus, and `i64` overflow in a fold are errors** — mirroring core's `asm/subst.rs` exactly; the negative-remainder diagnostic must suggest the `{(v+N-1)%N}` idiom with the actual `N`.
- Exact new diagnostic codes: `negative-remainder`, `zero-modulus`, `fold-overflow` (errors); `empty-expansion`, `unreachable-rule` (warnings). Exact new lint rule ids: `index-identity-map` (warn tier), `unused-alphabet`, `unused-tape`, `unused-graft-name`, `unused-exit` (`.tmc`, default tier), `duplicate-map-source` (`.tma`).
- New code comments cite durable pages only (`docs/tmt/language.md (substitution)` style). **No `spec §N`, no issue/PR numbers, no `docs/superpowers/` paths in code comments or published docs.**
- Conventional commits with scope. **No Claude/AI attribution anywhere.**
- **Verify before editing:** file:line citations below were verified 2026-07-21 but the tree moves — re-locate cited items before editing; if reality contradicts a plan claim, trust reality and say so in your report.
- Branch: `tmc-range-expressions` off updated master.
- Per-task gate: `cargo test -p mtc-turing-machine` green + the task's named tests; per-task finish: `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo fmt --check` all green, then commit.

---

### Task 1: Fold-expression grammar — lexer `%`, parser, CST, formatter

**Files:**
- Modify: `crates/turing-machine/src/lexer.rs` (token enum near the `Plus`/`Star`/`Minus` variants, ~:478-486)
- Modify: `crates/turing-machine/src/parser.rs` (`WriteCellKind::Subst` ~:341-347; `check_char_arithmetic` ~:1675-1712; the substitution-parsing code they serve)
- Modify: `crates/turing-machine/src/cst.rs` and `crates/turing-machine/src/fmt.rs` (write-cell substitution node; only if the CST stores substitutions structurally — if it stores raw tokens, verify fmt round-trips `%` untouched and add tests only)
- Modify: `editors/grammars/` `.tmc` grammar **only if** a drift guard fails after the lexer change (run the guard tests; if green, grammar work stays in Task 9)
- Test: existing parser/fmt test files in `crates/turing-machine/` (follow each file's local-helper conventions)

**Interfaces (produces — later tasks rely on these exact shapes):**

```rust
// parser.rs — replaces the delta-based Subst payload
pub enum FoldOp { Add, Sub, Mul, Rem }

pub struct FoldExprNode {
    pub kind: FoldExprKind,
    pub span: Span,            // use the parser's existing span type
}
pub enum FoldExprKind {
    Var(String),               // an in-scope pattern binding
    Int(i64),
    Bin { op: FoldOp, lhs: Box<FoldExprNode>, rhs: Box<FoldExprNode> },
}

// WriteCellKind::Subst becomes:
Subst { expr: FoldExprNode }
```

**Requirements:**

1. Lexer: add a `%` token (`Percent`). Parens already lex.
2. Parser: inside `{…}` write-cell substitutions, parse the assembler's exact grammar (`crates/core/src/asm/subst.rs:6-17` documents it — read it, do not import it):
   ```
   expr := mul (('+' | '-') mul)*
   mul  := atom (('*' | '%') atom)*
   atom := var | integer | '(' expr ')'
   ```
   Left-associative; `*`/`%` bind tighter. Multiple distinct vars in one expression are legal.
3. **Passthrough rule:** a substitution whose entire content is a single bare name (`FoldExprKind::Var` at top level, no operators, no parens) keeps today's passthrough semantics — legal for glyph and numeric bindings alike. Anything else is arithmetic.
4. `check_char_arithmetic` generalizes: any expression that is *not* a top-level bare Var and references a binding bound by a **glyph** pattern cell → the existing `CharArithmetic` error (same code, same span behavior as today). Verify how the current check identifies glyph bindings and reuse that mechanism.
5. Old `{v+3}` / `{v-3}` sources must parse to the same semantics as before (now as `Bin{Add/Sub, Var, Int}`) — no behavior change for existing programs; the whole existing test suite is the regression net.
6. Formatter: a substitution prints **tight** — no spaces inside `{…}`: `{(v+1)%127}`. If the CST keeps raw tokens, confirm this happens for free and pin it with a test; if fmt normalizes spacing, implement tight printing. fmt stays idempotent and whitespace-only.

**Steps:**

- [ ] **Step 1: Failing parser tests.** In the parser's existing test module/file, add (adapting helper names to local convention):

```rust
#[test]
fn fold_expr_modulo_parses() {
    // a machine with a numeric range binding and a % fold — must parse
    let src = wrap_rule("[0..5 as v] -> write [{(v+1)%6}] move [>] goto s;");
    assert!(parse_ok(&src));
}

#[test]
fn fold_expr_precedence_and_multi_var() {
    let src = wrap_rule("[0..2 as a, 0..2 as b] -> write [{a+b*2}, -] move [>, .] goto s;");
    assert!(parse_ok(&src)); // b*2 binds tighter; two vars legal
}

#[test]
fn fold_expr_char_arithmetic_still_rejected() {
    let src = wrap_rule("['a'..'c' as c] -> write [{(c+1)%3}] move [>] goto s;");
    assert_error_code(&src, "char-arithmetic"); // verify the existing code's exact spelling first
}

#[test]
fn fold_expr_bare_var_stays_passthrough_for_glyphs() {
    let src = wrap_rule("['a'..'c' as c] -> write [{c}] move [>] goto s;");
    assert!(parse_ok(&src));
}
```

- [ ] **Step 2:** `cargo test -p mtc-turing-machine <new test names>` — confirm the `%` cases fail with today's lex error.
- [ ] **Step 3:** Implement lexer token + parser grammar + `FoldExprNode` + generalized char-arithmetic check. Migrate every consumer of the old `Subst { name, delta }` shape (compiler/expand read it — leave `expand.rs` compiling by mapping the old delta fold onto the new tree *temporarily*: evaluate only `Var` and `Var±Int` shapes there, returning a clear `unimplemented`-style compile error for deeper trees; Task 2 replaces this).
- [ ] **Step 4:** Run the new tests → pass; run `cargo test -p mtc-turing-machine` → everything else still green (existing `{v±k}` programs unaffected).
- [ ] **Step 5:** fmt tests: reformat a fixture containing `{ ( v + 1 ) % 6 }` with sprinkled spaces → expect `{(v+1)%6}`; idempotence holds. Run the grammar drift guards; update `editors/grammars/` only if one fails.
- [ ] **Step 6:** Commit: `feat(turing-machine): fold expressions parse — the assembler's substitution grammar in .tmc write cells`

---

### Task 2: Fold evaluation in expand — `%` semantics, new error codes

**Files:**
- Modify: `crates/turing-machine/src/expand.rs` (`resolve_write_cell` ~:685-739; the `FoldOutOfAlphabet` error path ~:721-736)
- Modify: the compiler's error-rendering site so the three new codes print (find where `FoldOutOfAlphabet` renders and add siblings)
- Test: `expand.rs` test module + an integration compile test file

**Interfaces:**
- Consumes: `FoldExprNode`/`FoldExprKind`/`FoldOp` from Task 1.
- Produces: full fold evaluation inside `resolve_write_cell`; the Task 1 temporary deep-tree error is gone.

**Requirements:**

1. Evaluate the expression per expanded row over `i64` with the row's binding environment (all bound numeric values in scope). Semantics **identical to** `crates/core/src/asm/subst.rs` (read its `mul` handling at ~:101-132 for the exact error conditions):
   - modulus zero → error `zero-modulus`
   - any intermediate negative remainder → error `negative-remainder`, message MUST include the idiom hint with the actual modulus, e.g. `negative remainder in fold; for a wrapping decrement write {(v+126)%127}`
   - any intermediate `i64` overflow (use `checked_add`/`checked_sub`/`checked_mul`/`checked_rem`) → error `fold-overflow`
2. The fold result then passes through the **existing** out-of-alphabet check unchanged (`FoldOutOfAlphabet`).
3. Errors span the substitution (`FoldExprNode.span` or the write cell's existing span — match the current `FoldOutOfAlphabet` span behavior).
4. A glyph binding consumed by a top-level bare `Var` still passes through (Task 1 rule); numeric-only enforcement for arithmetic was done at parse time — expansion may `debug_assert!` it, not re-error.

**Steps:**

- [ ] **Step 1: Failing tests** (integration-style, through `compile`):

```rust
#[test]
fn fold_modulo_wraps_increment() {
    // alphabet of 6 numeric symbols 0..5; rule [0..5] writes {(v+1)%6}
    // derive expected rows: v=5 writes 0 — assert the compiled program,
    // run on a tape holding 5, writes 0 (derivation-first, not snapshot)
}

#[test]
fn fold_negative_remainder_errors_with_hint() {
    // {(v-1)%6} → error code negative-remainder,
    // message contains "{(v+5)%6}"
}

#[test]
fn fold_zero_modulus_errors() { /* {v%0} → zero-modulus */ }

#[test]
fn fold_overflow_errors() { /* {v*9223372036854775807} → fold-overflow */ }

#[test]
fn fold_out_of_alphabet_unchanged() {
    // {v+10} over a 6-symbol alphabet still hits the existing error
}

#[test]
fn fold_multi_var() {
    // [0..1 as a, 0..1 as b] write [{a+b}, -] — 4 rows, derive each
}
```

- [ ] **Step 2:** Run → the modulo/multi-var cases fail against Task 1's temporary error.
- [ ] **Step 3:** Implement `eval_fold(expr: &FoldExprNode, env: &…) -> Result<i64, FoldEvalError>` in `expand.rs` (private), wire into `resolve_write_cell`, add the three error variants + rendering.
- [ ] **Step 4:** All new tests pass; full `cargo test -p mtc-turing-machine` green.
- [ ] **Step 5:** Commit: `feat(turing-machine): fold evaluation — modulo wrap, zero-modulus/negative-remainder/overflow diagnostics`

---

### Task 3: Optional transition — parser, resolution after graft splicing, fmt

**Files:**
- Modify: `crates/turing-machine/src/parser.rs` (`rule()` — the unconditional `self.transition()` call; the CST rule node)
- Modify: `crates/turing-machine/src/expand.rs` (`expand_rule` ~:604-621 lowers the transition once — resolution point for the omitted case)
- Modify: `crates/turing-machine/src/fmt.rs` (omission preserved)
- Modify: `crates/turing-machine/src/lsp/` completion context **only if** a transition-position test fails (the completion set is unchanged; position may now also be rule-end)
- Test: parser + expand + fmt test files

**Requirements:**

1. Grammar: the transition is optional **iff** the rule has at least one of `write`, `move`, `debugger`. `['a'] -> ;` (arrow, then nothing) stays a parse error with the existing "expected a transition …" message family.
2. `call … then` unchanged — `then` mandatory (existing error already covers it; keep its test).
3. Representation: the rule's transition becomes `Option<…>` (or an explicit `Stay` variant if `Option` ripples too far — pick whichever touches fewer sites, state the choice in your report). Every downstream exhaustive match must be found and handled (`grep` for the transition enum's name).
4. **Resolution timing:** an omitted transition resolves to a self-`goto` at expansion time, where the *current post-splicing state identity* is in hand — a graph rule self-loops to its own **spliced instance**, never to the graph-source state. Verify where `expand_rule` learns the owning state and use exactly that.
5. fmt: prints the rule without a transition; idempotent; whitespace-only guarantee intact.
6. CST losslessness: round-trip of a file using the sugar is byte-preserving through the lossless path.

**Steps:**

- [ ] **Step 1: Failing tests:**

```rust
#[test]
fn omitted_transition_stays_in_state() {
    // state scan { ['a'] -> write ['b'] move [>]; ['_'] -> stop; }
    // run on "aa_": both a's become b, head walks right, stops on '_'
    // (derivation-first: expected tape "bb_")
}

#[test]
fn empty_rule_body_still_errors() {
    // ['a'] -> ;  → parse error (transition family message)
}

#[test]
fn omitted_transition_in_graph_self_loops_to_instance() {
    // a graph whose rule omits the transition; grafted twice under two
    // names; each instance loops to ITSELF (run both paths, derive tapes)
}

#[test]
fn fmt_preserves_omitted_transition() { /* reformat, assert no `goto` inserted, idempotent */ }
```

- [ ] **Step 2:** Run → parse failures on the sugar.
- [ ] **Step 3:** Implement parser optionality + downstream match sites + expansion resolution + fmt.
- [ ] **Step 4:** All green including the full crate suite; run the LSP/completion tests — fix the context only if one fails.
- [ ] **Step 5:** Commit: `feat(turing-machine): optional transition — omitted means stay in the current state`

---

### Task 4: The never-fires family — `empty-expansion`, `unreachable-rule`, zero-row-sound codegen

**Files:**
- Modify: `crates/turing-machine/src/expand.rs` (zero-row detection; the `expansion-threshold` warning at ~:623-633 is the plumbing precedent — new warnings ride the same channel)
- Modify: `crates/turing-machine/src/compiler.rs` (catch-all shadowing check on flattened rules, pre-expansion)
- Modify: `crates/turing-machine/src/codegen.rs` (`conditional` ~:284-341 — zero-row rules mint nothing; zero-row states)
- Test: integration compile tests (the #44 and #48 reproductions from the issues become fixtures)

**Requirements:**

1. **`empty-expansion`** (warning): a rule whose expansion produced zero rows — every range alternative dropped (`cell_options` filter ~:576-578) or a single absent glyph (~:556-558). Spanned at the rule. Compilation proceeds; the rule contributes nothing.
2. **`unreachable-rule`** (warning): a rule strictly after an **all-wildcard** rule (every pattern cell `Wildcard`) in the same state. Spanned at the later rule. The rule is **dropped before codegen** so the all-wildcard-row-must-last discipline can never be violated. Detection on flattened (post-graft, pre-expansion) rules so both hand-written and spliced shapes are covered.
3. **Zero-row state**: a state whose rules all vanished is valid. Codegen emits the state's label followed by the same outcome a runtime no-match produces. First **verify what a no-match does today** (run a program whose state lacks a matching row — establish the trap kind), then make the zero-row state produce the *identical* observable (same termination kind, same trap payload). Do not guess the encoding — read `codegen.rs`'s dispatch emission and the arch's opcode table.
4. **Neighbouring sweep**: probe every other shape where expansion can produce zero of something codegen assumes ≥1 of (e.g., a `.targets` list, a synthesized dispatch label, a group with all rules dropped). For each: add a test if broken, or a one-line note in your report if sound. The two issue reproductions are the floor, not the ceiling.
5. Both warnings render through `CompileReport` exactly like `expansion-threshold` (verify how the CLI shows it under `-v` and match).

**Steps:**

- [ ] **Step 1: Failing tests** — the two issue reproductions:

```rust
#[test]
fn empty_expansion_warns_and_compiles() {
    // alphabet small { '_', 'a', 'b' }; rule [0..5] -> move [>] goto s;
    // → compiles; warnings contain code "empty-expansion"; running a tape
    //   never fires the rule (derive: input unchanged until another rule acts)
}

#[test]
fn unreachable_after_catch_all_warns_and_compiles() {
    // entry state s { [*] -> move [>] goto s; [*] -> stop; }
    // → compiles (no internal error); warnings contain "unreachable-rule";
    //   behavior identical to the single-rule program (derive both, compare)
}

#[test]
fn zero_row_state_traps_like_no_match() {
    // a state whose only rule empty-expands; entering it traps with the
    // same kind as a genuine no-match (assert on RunResult / trap kind)
}

#[test]
fn graft_instantiated_empty_expansion_warns_not_errors() {
    // a graph with a [0..9] rule, grafted onto a tape whose alphabet has
    // no numeric labels: compiles with "empty-expansion", the instance's
    // other rules still work (derive and run) — the generic-code case the
    // warning ruling exists to protect
}
```

- [ ] **Step 2:** Run → today the second is the `internal-error` from the issue; first likewise.
- [ ] **Step 3:** Implement detection + drop + zero-row codegen; sweep per requirement 4.
- [ ] **Step 4:** Full crate suite green.
- [ ] **Step 5:** Commit: `fix(turing-machine): never-fires rules warn and vanish — empty-expansion, unreachable-rule, zero-row-sound codegen`

---

### Task 5: The `.rept` re-detection emitter + `--stamped-asm`

**Files:**
- Create: `crates/turing-machine/src/rept_emit.rs`
- Modify: `crates/turing-machine/src/lib.rs` (module decl), the compile pipeline where codegen text meets `asm::assemble` (find it in `compiler.rs`/`cli/build.rs` — verify), `CompileOptions` (+ `stamped_asm: bool`, default `false`), `crates/turing-machine/src/completions/` registry (`--stamped-asm`, boolean flag on `compile`)
- Test: `crates/turing-machine/tests/` new file `rept_emit.rs` + the completions drift-guard suite

**Interfaces (produces):**

```rust
// rept_emit.rs
pub struct ReptEmitReport {
    pub runs_compressed: usize,
    pub lines_before: usize,
    pub lines_after: usize,
    pub fell_back: bool,       // self-check mismatch → stamped text returned
}

/// Compress stamped assembly text by rewriting arithmetic families as .rept.
/// `syntax` is the TM-1 dialect used for the self-check assembly.
pub fn compress_asm(text: &str, syntax: &ArchSyntax) -> (String, ReptEmitReport)
```

**Requirements:**

1. **Blocks:** split the text into label-delimited blocks (a block starts at a labeled line, extends to the next labeled line). Same-label `.row`/`.targets` continuation lines inside table sections form their own run type over consecutive lines.
2. **Run detection:** a compressible run is a maximal sequence of ≥ **4** consecutive blocks (or table lines) that tokenize identically except integers at fixed token positions. Integers include those embedded in label identifiers (`plus__88` → prefix `plus__` + `88`; parameterize as `plus__{…}`).
3. **Progression inference per varying position** across the run of `n` members, with `v` running `0..n-1`:
   - constant `c` at every member → emit the literal `c` (position wasn't actually varying — exclude it up front)
   - `value_i == first + i` → emit `{v+first}` (or `{v}` when `first == 0`)
   - `value_i == (first + i) % N` where `N = max(value) + 1` and at least one wrap occurred → emit `{(v+first)%N}` (when `first == 0`: `{v%N}`)
   - anything else → the run is not compressible at that boundary; split/shrink; sub-4 remainders stay stamped.
   Emit header `.rept v, 0, <n-1>` … `.endr`. Every emitted expression must be assembler-legal (never a form that can produce a negative remainder).
4. **Self-check (always on):** assemble the compressed text and the stamped text through core's existing public assemble entry with `tm1_syntax()`; compare the resulting object **bytes**. Mismatch or any assemble error on the compressed side → return the stamped text with `fell_back: true`. This is the safety property: the pass can never change what assembles.
5. **Wiring:** in the compile pipeline, when `!options.stamped_asm`, run `compress_asm` on codegen's text before the normal assemble; use the compressed text as the `-S` artifact and its (already produced) assembly as the object. When `stamped_asm` or `fell_back`, behavior is byte-for-byte today's.
6. **CLI:** `tmt compile --stamped-asm` sets the option; unknown-flag behavior untouched; completions registry entry + the drift guards updated. Choose a var name that keeps the parser's local conventions.
7. If the loop var name `v` collides with an existing label/identifier semantics in `.rept` (it cannot — substitution only replaces `{…}` occurrences — verify against `lower_rept`, `crates/core/src/asm/lower.rs:959-1002`), note it; no escaping needed.

**Steps:**

- [ ] **Step 1: Failing unit tests** in `tests/rept_emit.rs` (synthetic stamped text, no compiler needed):

```rust
#[test]
fn affine_family_compresses() {
    // 5 blocks: L{i}: wr [{i+3}] / jmp done, i = 0..4  (write literal operand i+3)
    // → output contains ".rept v, 0, 4", "L{v}:", "{v+3}"
    // → assemble both with tm1_syntax(), bytes identical, fell_back == false
}

#[test]
fn modular_family_compresses() {
    // 6 blocks writing (i+1)%6 → output contains "{(v+1)%6}"
}

#[test]
fn sub_four_run_stays_stamped() { /* 3 blocks → unchanged text, 0 runs */ }

#[test]
fn gap_splits_run() {
    // 9 blocks with block 4 shaped differently → two stamped-or-compressed
    // segments; whole-text assemble-both identity still holds
}

#[test]
fn non_integer_variance_stays_stamped() { /* mnemonic differs mid-run */ }

#[test]
fn table_rows_compress() {
    // one label + 8 continuation .row lines with one varying index
    // → ".rept" wrapping same-label rows; assemble-both identical
}
```

- [ ] **Step 2:** Run → module doesn't exist.
- [ ] **Step 3:** Implement `rept_emit.rs`; wire pipeline + flag + registry.
- [ ] **Step 4:** Unit tests green; completions drift guards green; whole crate green. Add one **property test** (proptest): generate small programs of randomized stamped families (affine/modular/mixed) → `compress_asm` output always assembles to identical bytes.
- [ ] **Step 5:** Commit: `feat(turing-machine): rept re-detection emitter — compressed -S output, assemble-both self-check, --stamped-asm`

---

### Task 6: Flagship rewrite — wrap workaround out, `.rept` families asserted

**Files:**
- Modify: `docs/examples/brainfuck-utm.tmc` (the increment/decrement boundary workaround → `{(v+1)%127}` / `{(v+126)%127}`)
- Modify: the flagship's test file in `crates/turing-machine/tests/` (find the existing `.tma`/`.tmc` equivalence + golden tests and extend)
- Test: same file

**Requirements:**

1. Replace the non-wrapping-range-plus-boundary-rule workaround with single modular fold rules. Read the file's own comments first — keep its teaching-oriented comment style; update any comment describing the workaround.
2. Semantics identical: the existing derivation-first goldens and the `.tma`/`.tmc` equivalence test (final tapes + outcome) must pass **unchanged** — do not regenerate goldens.
3. New assertion: `tmt compile -S` (via the library API) of the flagship contains **at least 3** `.rept` headers, and total emitted line count is **< 400** (hand-written is 212; generated was 1659).
4. Record before/after line counts in your report (they feed the issue-close comment).

**Steps:**

- [ ] **Step 1:** Add the failing assertion test (compile current file, assert `.rept` count ≥ 3 → fails only because the source still stamps via workaround… verify: if Task 5 already compresses the workaround's stamped rows, the test may pass early — in that case tighten it to also assert the source contains `%` folds, i.e. this task is source cleanup + measurement).
- [ ] **Step 2:** Rewrite the `.tmc`; run equivalence + goldens → green, untouched.
- [ ] **Step 3:** Full crate suite green.
- [ ] **Step 4:** Commit: `feat(turing-machine): flagship .tmc uses modular folds — compact generated assembly proven end to end`

---

### Task 7: Lint wave A — the unused family (`unused-alphabet`, `unused-tape`, `unused-graft-name`, `unused-exit`)

**Files:**
- Create: four rule files under `crates/turing-machine/src/lint/` following the existing per-rule file pattern (read two existing rules first — one simple, one using resolution data — and mirror their structure, registration, and fixture style)
- Modify: the lint registry/module that enumerates `.tmc` rules; the shared allow namespace list
- Test: the lint layer's existing fixture-based test files

**Requirements (firing conditions are exact):**

1. **`unused-alphabet`** (default tier): an `alphabet` declaration referenced by no tape declaration and no signature parameter type. **Fix:** delete the declaration (the whole line/span incl. its doc run — check how existing fixes handle attached trivia).
2. **`unused-tape`** (default tier): a machine-level tape declaration where, across every rule of the machine world: its pattern cell is `*` (or the pattern doesn't constrain it), its write cell is `-`/omitted, its move cell is `.`/omitted, and the tape is never a binding argument (`call`/`graft`/`bind`). **No fix** (`fix: None`; doc comment states why: removing a tape changes every vector's arity).
3. **`unused-graft-name`** (default tier): a graft carrying `as NAME` whose instance is reachable but `NAME` is referenced by no rule transition and no binding. (For unreachable instances `unused-graft-instance` already fires — do not double-report; check reachability the same way that rule does.) **Fix:** remove the ` as NAME` text.
4. **`unused-exit`** (default tier): a `graph` declaring a `state` exit parameter that its body never targets — no rule transition, no bare-name goto, no `call … then`, and no binding argument inside the body references it. A reference **through a nested construct counts as a use**. **No fix** (removing a parameter is an API change at every call site).
5. All four: docs row content is written in Task 9, but each rule's doc comment carries the substance now (durable prose, no tracker refs). Allow-namespace entries registered. **Expected stdlib fallout:** `unused-graft-name` will find ~12 sites in `std.tmc` — the stdlib is NOT edited in this round; if any repo test lints the stdlib and fails, report it as a concern instead of editing `std.tmc`.

**Steps:**

- [ ] **Step 1:** Failing fixture tests — per rule, one firing case + one silent case (the silent cases: alphabet used by a signature param only; tape all-wildcard but passed as a binding argument; entry graft whose name IS referenced; exit targeted only inside a nested scope).
- [ ] **Step 2:** Run → rules unknown.
- [ ] **Step 3:** Implement + register (rules, allow names, fixes for 1 and 3).
- [ ] **Step 4:** Quickfix application tests for the two Fixes: apply → re-lint clean → still compiles.
- [ ] **Step 5:** Full crate suite; commit: `feat(turing-machine): the unused-family lint sweep — alphabet, tape, graft name, graph exit`

---

### Task 8: Lint wave B — `index-identity-map` (warn tier) + `duplicate-map-source` (`.tma`)

**Files:**
- Create: one rule file under `crates/turing-machine/src/lint/` (`.tmc`, warn tier) and one under `crates/turing-machine/src/lint/tma/rules/` (mirror `rept_var_unused.rs`'s structure)
- Modify: both registries; the allow namespace; the `lint/tma/mod.rs` rule-inventory note that records the duplicate-`.map` gap (it is now closed — update the note)
- Test: both layers' fixture files

**Requirements:**

1. **`index-identity-map`** — **warn tier** (verify how `state-may-trap` implements opt-in under `--warn` and use the identical mechanism; allow-suppression must beat `--warn`, the existing precedent): fires on a `call` or `bind` with an **omitted** map where both tapes' alphabets are resolvable in-compilation and are not glyph-for-glyph equal. Message names the first differing index and both glyphs, e.g. `call maps by index across differently-glyphed alphabets ('a' vs 'x' at index 1); glyphs change meaning here`. Silent when: a map is written, alphabets are glyph-equal, or the callee's alphabet is not visible. **No fix.**
2. **`duplicate-map-source`** (`.tma`, default tier): a `.map` directive whose `rmap=(…)` clause list names the same source symbol twice. Span the **later** clause; message `source symbol N mapped twice; the last mapping wins`. **Fix:** remove the earlier, shadowed clause (it is the dead one — last wins, byte-proven in the tracker; state "last write wins" in the rule doc as observed assembler behavior). Turing-side only — core's `lower.rs` map building is NOT touched.
3. Fixture floor: `rmap=(1->2, 1->3)` fires; `rmap=(1->2, 2->3)` silent; the `.tmc` rule's four silent cases above; `--warn` on/off behavior; allow-beats-warn.

**Steps:**

- [ ] **Step 1:** Failing fixtures (both rules, firing + silent + tier behavior).
- [ ] **Step 2:** Run → fail.
- [ ] **Step 3:** Implement + register + update the inventory note.
- [ ] **Step 4:** Quickfix application test for `duplicate-map-source` (apply → re-lint clean → assembles to the same bytes as hand-removing the clause).
- [ ] **Step 5:** Full crate suite; commit: `feat(turing-machine): index-identity-map warn-tier rule and duplicate-map-source tma rule`

---

### Task 9: Docs, grammar, and closure text

**Files:**
- Modify: `docs/tmt/language.md` (substitution section: expression grammar, error table, wrapping increment example replacing the boundary special-case; the optional-transition rule; the omitted-map index-identity paragraph with the graft-vs-call layer rationale)
- Modify: `docs/tmt/cli.md` (`--stamped-asm` under compile — match the page's existing flag-description style; do NOT touch the verbatim `tmt --help` quote unless the help text changed, in which case `cli_docs.rs` will tell you)
- Modify: `docs/tmt/lint.md` (six new rule rows: tier, fires-when, fix availability; a note relating `dead-rule` to the `unreachable-rule`/`empty-expansion` compile warnings)
- Modify: `docs/tmt/fmt.md` **only if** substitution printing or the optional transition needs a sentence (check the existing "whitespace-only" claims still hold verbatim)
- Modify: `editors/grammars/` `.tmc` grammar for `%` in the substitution context (if not already forced in Task 1), drift guards green
- Test: `cargo test --workspace` (the doc drift guards + everything)

**Requirements:**

1. **Verify every claim against the built binary** before writing it (the phase-8 discipline): run `tmt compile` / `tmt lint --warn` / `tmt fmt` on the documented examples; each documented error message is copied from real output.
2. Published-docs policy: ref-free prose, forge-agnostic, no `spec §N`.
3. The index-identity paragraph documents: omitted `call`/`bind` map = index identity, including across differently-glyphed same-size alphabets; graft's omitted map = glyph identity because graft is a source-level splice; the machine level has no glyphs. Mention the `--warn` rule by name as the audit tool.
4. The language reference's substitution error table lists exactly: `zero-modulus`, `negative-remainder` (with the idiom), `fold-overflow`, and the pre-existing out-of-alphabet error under its real rendered code.
5. `TMC_LANG_VERSION` is **not** edited by this task (stays `"0.1"`).

**Steps:**

- [ ] **Step 1:** Write all doc edits; run every example through the real binary (`cargo build --release` first).
- [ ] **Step 2:** `cargo test --workspace` + clippy + fmt-check green (drift guards prove grammar and cli_docs coherence).
- [ ] **Step 3:** Commit: `docs(tmt): fold expressions, optional transition, index identity, the lint wave — language/cli/lint pages`

---

## Final gates (whole branch, run by the controller before merge)

- `cargo test --workspace` / `cargo clippy --workspace --all-targets -- -D warnings` / `cargo fmt --check`
- `git diff master -- crates/core` → empty
- `git status --short crates/post-machine/tests/golden/` → empty
- Flagship: `.tma`/`.tmc` equivalence green; `-S` line count recorded (target < 400, expectation ≈ 250)
- Issue closes on merge: #31, #33, #35, #44, #45, #46, #47, #48; #43 closes as by-design citing the language.md paragraph; #38 re-triaged (comment scope shipped, retrofit remains)
