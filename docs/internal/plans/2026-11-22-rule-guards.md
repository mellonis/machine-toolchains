# Rule Guards Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `.tmc` rule guards — `where { … }`, a compile-time predicate over a
rule's bindings that filters the pattern's cartesian expansion — with typed
comparison domains, cone-local narrowing, the printing/declared policies, and
one warn-tier lint.

**Architecture:** the guard is a green-tree sub-AST on `Rule`, resolved and
typed at the world-resolution stage (after `resolve_pattern_sets`, so
pre-existing errors keep priority), and evaluated per candidate row inside
`expand.rs`'s existing `env` loop — the rows that survive are ordinary rows,
so nothing past `expand` changes. Graft splicing already expands a graph in
its own world before `map_rule`, so the filter is graph-space by
construction.

**Tech Stack:** Rust (the `mtc-turing-machine` crate), plain cargo tests;
TextMate JSON grammar; Markdown docs.

**Spec:** `docs/internal/specs/2026-11-22-rule-guards-design.md` (binding —
the plan argues from it; executors read both). Issue-driven; see the spec's
Intent for the chess-engine motive.

## Global Constraints

- Diff boundary: `crates/turing-machine/`, `docs/`, `CLAUDE.md`,
  `CHANGELOG.md`, `editors/grammars/tmc.tmLanguage.json` ONLY. `crates/core`,
  `crates/post-machine`, `crates/wasm` untouched (core neutrality; PM-1
  byte-identity).
- `.tmc` language version does NOT move: 0.3, "no movement" — the acceptance
  addition folds into the standing once-per-`next`-cycle bump; the CHANGELOG
  entry carries a "0.3, no movement" version block in `## [Unreleased]`.
- Strictness priority: `guard` checks run AFTER `resolve_pattern_sets` for the
  same world (the named-cells ruling) — set errors win over guard errors.
- Every new error code lands with its registry entry, witness (order-asserted
  vs `CODES`), `Display`, AND its `docs/tmt/cli.md` row, one commit — the
  drift guard is bidirectional.
- Formatter policy: a guard is printed verbatim (whitespace-only
  normalization); the never-move comment rule covers every new slot; the
  comment-positions templates put `@C@` at line END (a `//` flavour eats the
  rest of a physical line).
- Cross-unit tests go through `mtc_turing_machine::cli::execute` with PID +
  atomic-counter scratch dirs under `CARGO_TARGET_TMPDIR` (idioms:
  `tests/library_grafts.rs`, `tests/named_cells.rs::ScratchDir`).
- Equivalence pins compare what the round claims: full linked disassembly /
  run outcomes where writes and moves ride (`wrmv`), not match rows alone —
  the row-only blind spot found by the named-cells mutation sweep.
- TDD with a mutation check per task (record in the ledger which assertion
  turned red); one fix-pass after the final review; merge into `next` only on
  the partner's explicit say-so.

## Review Focus

Guards are a filter that REMOVES rows; each line below is a case where a
reasonable person expects truth and the feature could silently give none.

1. **A guard that filters every row** warns the existing `empty-expansion`
   (never silent, never an error; a zero-row state keeps its trap-on-entry
   meaning). → Task 3 `a_guard_that_keeps_no_rows_warns_empty_expansion`.
2. **A guarded graph body crossing units**: rows are filtered in GRAPH space
   and then mapped (so an order-non-preserving `with map` cannot change which
   rows exist), and `graph_digest`'s named-content appendix includes guard
   `in S` sets so `GraftDrift` fires on set-member drift. → Task 4
   `a_guarded_graph_body_splices_graph_space` and
   `guard_set_drift_is_still_graft_drift`.
3. **fmt's clean reprint never drops a guard** (the named-cells hazard) and
   comments inside a guard stay in their written slots, both flavours, all
   three mechanical gates. → Task 5 `a_guard_survives_fmt_verbatim` + the new
   comment-position group.
4. **Cone-local typing**: identical text `x < y` in two cones types by its own
   cone (narrowed leg over the set, bare leg over the alphabet — both pinned),
   and one-sided narrowing `x in S && x < y` is refused naming BOTH contexts,
   span at the comparison. → Task 2 `a_comparison_types_by_its_own_cone` and
   `one_sided_narrowing_names_both_contexts`.
5. **`binding-product-threshold` counts the PRE-filter product**: a 25×25
   product keeping 26 rows still trips the audit, compile and lint agreeing.
   → Task 3 `the_product_threshold_counts_before_the_filter`.

---

## File map

- `crates/turing-machine/src/lexer.rs` — the reserved-word table gains `where`.
- `crates/turing-machine/src/parser.rs` (+ `parser/tests.rs`) — `Rule.guard`,
  the `Guard` AST, `fn guard()` in the rule walk.
- `crates/turing-machine/src/syntax/{kinds,views,extract}.rs` — green node,
  typed view, flattening into `Rule.guard`.
- `crates/turing-machine/src/guard.rs` (NEW) — `guard_text` renderer, cone
  typing/resolution, `row_matches` evaluator. One module, pure functions.
- `crates/turing-machine/src/compiler.rs` — the resolution hook and the new
  `CompileErrorKind` kinds.
- `crates/turing-machine/src/expand.rs` — the filter in `expand_rule`'s combo
  loop (~line 788) and the `empty-expansion` warning site (~line 512).
- `crates/turing-machine/src/header.rs` — `rule_text` prints guards; the
  `graph_digest` named-content appendix (~lines 1704/1777/1941) gains guard
  sets.
- `crates/turing-machine/src/fmt/print.rs` (+ `fmt/trivia.rs`) — guard
  printing slots.
- `crates/turing-machine/src/lint/rules/redundant_guard_narrowing.rs` (NEW) +
  `lint/mod.rs` registry + `docs/tmt/lint.md` inventory row.
- Tests: `tests/rule_guards.rs` (NEW, the round's pins), `comment_positions.rs`
  (new slot group), `fmt_tmc.rs` + `fmt_adversarial/guards.{tmc,fmt}`.
- Docs: `docs/tmt/language.md` (new "Rule guards" section + version history),
  `docs/tmt/fmt.md`, `docs/tmt/cli.md` (error rows), `CHANGELOG.md`,
  `CLAUDE.md` (reserved-word count 31 → 32; comment-audit count), 
  `editors/grammars/tmc.tmLanguage.json` (keyword tier).

**Interfaces (the cross-task contract):**

```rust
// parser.rs — the guard as written
pub struct Guard { pub expr: GuardExpr, pub span: Span }
pub enum GuardExpr {
    Cmp(GuardCmp),
    In(GuardIn),
    And(Box<GuardExpr>, Box<GuardExpr>),
    Or(Box<GuardExpr>, Box<GuardExpr>),
}
pub struct GuardCmp { pub left: GuardTerm, pub op: CmpOp, pub right: GuardTerm, pub span: Span }
pub struct GuardIn  { pub binding: CellName, pub set: CellName, pub span: Span }
pub struct GuardTerm { pub base: GuardBase, pub offset: i64, pub span: Span }
pub enum GuardBase { Binding(CellName), Lit(SymLit) }   // SymLit = Glyph | Number
pub enum CmpOp { Eq, Ne, Lt, Le, Gt, Ge }
// Rule gains: pub guard: Option<Guard>
```

`guard.rs` public surface (module-internal hosting of contexts is the
builder's; the names below are what later tasks reference):

```rust
pub(crate) fn guard_text(g: &Guard) -> String         // canonical render; header.rs + tests
pub(crate) fn check_rule_guard(rule: &mut Rule, world: &GuardContext) -> Result<(), CompileError>
pub(crate) fn row_matches(g: &Guard, env: &HashMap<&str, &BoundVal>, world: &GuardContext) -> bool
```

`GuardContext` abstracts what typing/evaluation need: per-tape alphabet
label order, and `in S` sets resolved to member sequences (resolve like
`PatternCellKind::SetRef`'s `resolved` — "nothing past the front end sees a
set name"; the rendered `guard_text` still spells names for headers).

`BoundVal` is expand's existing row-binding value type (`expand.rs:788`).

---

### Task 1: Grammar — `where`, the guard AST, the keyword tier

**Files:**
- Modify: `crates/turing-machine/src/lexer.rs` (the 31-keyword reserved table)
- Modify: `crates/turing-machine/src/parser.rs` (`Rule`, `fn rule()` — parse
  `where { … }` between the pattern vector and `->`; a lookahead of ZERO
  needed: `where` is reserved, `] where` is unambiguous)
- Modify: `crates/turing-machine/src/syntax/kinds.rs`,
  `syntax/views.rs`, `syntax/extract.rs` (node kind, typed view, flattening)
- Modify: `editors/grammars/tmc.tmLanguage.json` (keyword tier — the
  bidirectional TextMate↔parser drift guard moves in the same commit)
- Test: `crates/turing-machine/src/parser/tests.rs`

**Interfaces:**
- Consumes: `Span`, `SymLit`, `CellName` (the spanned-name carrier from the
  named-cells round: `struct CellName { name: String, span: Span }`).
- Produces: the `Guard` AST above, `Rule.guard: Option<Guard>`, `where` as
  reserved word #32.

- [ ] **Step 1: Write the failing tests** (parser/tests.rs): a guard with
  `==`/offsets/`in`/`&&`/`||`/parens parses to the exact AST shape asserted
  field-by-field; `y - 1` is `GuardTerm { base: Binding("y"), offset: -1 }`;
  a parse error inside the braces is a typed `CompileError` with span inside
  the guard; and `parse → text()` over a guarded fixture equals the source.

```rust
const G: &str = "… ['a'..'z' as x, 'a'..'z' as y] where { x == y || x == y - 1 } -> stop; …";
// assert: guard.expr == Or(Cmp(x==y), Cmp(x == y-1)), spans cover each node
```

- [ ] **Step 2: Run to verify they fail**
Run: `cargo test -p mtc-turing-machine parser::tests guard`
Expected: FAIL (no `guard` field / tests absent).

- [ ] **Step 3: Implement.** Reserved word in the lexer table; `fn guard()` in
  the rule walk; green node + view + extraction. Precedence in the parser:
  `Or` ⊃ `And` ⊃ `cmp | in` (no `!`); terms are `base [± integer]` only — no
  `*`/`%`. A guard on a rule whose pattern is absent is unreachable
  (`->`-less rules carry no pattern to guard).

- [ ] **Step 4: Verify green + drift guard**
Run: `cargo test -p mtc-turing-machine 2>&1 | grep -cE 'test result: ok'`
Expected: the whole crate green, incl. the TextMate drift-guard test after
`where` joins `tmc.tmLanguage.json`'s `keyword.control.tmc` tier.

- [ ] **Step 5: Mutation check + commit.** Mutate precedence (`And`/`Or`
  swapped): exactly the mixed-operator test reddens; revert. Commit
  `feat(turing-machine): a rule may carry a where guard`.

### Task 2: Typing and refusals — cone contexts, one common sequence

**Files:**
- Create: `crates/turing-machine/src/guard.rs`
- Modify: `crates/turing-machine/src/compiler.rs` (new `CompileErrorKind`s +
  `code()` + witness + `Display`; the hook in `resolve_world` /
  `resolve_machine_world`, after `resolve_pattern_sets`)
- Modify: `docs/tmt/cli.md` (four rows)
- Test: `crates/turing-machine/tests/rule_guards.rs` (NEW) +
  `compiler.rs` unit tests for the witness order

**Interfaces:**
- Consumes: the Task 1 AST.
- Produces: `guard::check_rule_guard`, `guard::guard_text`, `GuardContext` +
  resolution of `in S` to member sequences; error kinds
  `GuardOrderContext { left: String, right: String }` (`guard-order-context`),
  `GuardUnknownBinding { name: String }` (`guard-unknown-binding`),
  `GuardSetOutsideAlphabet { set: String, alphabet: String }`
  (`guard-set-outside-alphabet`), `GuardConstantComparison`
  (`guard-constant-comparison`).

- [ ] **Step 1: The failing refusal tests** (tests/rule_guards.rs), each
  asserting kind right AND span (the named-cells span discipline: the refusal
  names the offending operand/comparison, not the whole guard):
  - `one_sided_narrowing_names_both_contexts`: `x in S && x < y` —
    `guard-order-context`, span at `x < y`, message names `S` and the
    alphabet.
  - `a_second_narrowing_needs_no_position_is_fine`: `x in S && x in T &&
    x == y` compiles; `x in S && x in T && x < y` is `guard-order-context`.
  - `an_unknown_guard_name_is_refused`: `where { z == y }` —
    `guard-unknown-binding` at `z`.
  - `an_in_off_alphabet_is_refused`: `t in big` where `big ⊄ t`'s alphabet —
    `guard-set-outside-alphabet`, names both.
  - `a_comparison_with_no_binding_is_refused`: `where { 'a' == 'b' }` —
    `guard-constant-comparison`.
- [ ] **Step 2: Verify FAIL** — `cargo test -p mtc-turing-machine --test rule_guards`.
- [ ] **Step 3: Implement** `check_rule_guard` per the spec's typing table:
  occurrences typed against their own cone (walk up `&&` from each `In` for
  the cone root, mark bindings within that subtree); an order/offset operand
  pair is licensed iff both are `Binding`s of one alphabet unnarrowed **in
  that occurrence's cone**, or both narrowed **there** to one set name; a
  `Lit` is licensed through its label being in the other side's sequence;
  equality/`!=` offset-free needs no license. New kinds + registry + docs
  rows in the same commit; witness order asserted against `CODES`.
- [ ] **Step 4: Verify green** — whole crate; `error_code_docs` 4/4-style
  guard passes; then the mutation check: swap the cone walk to "whole
  guard" — `one_sided_narrowing_names_both_contexts` must redden (it is
  wrongly licensed), revert.
- [ ] **Step 5: Commit** `feat(turing-machine): guard typing and refusals`.

### Task 3: Evaluation and the row filter

**Files:**
- Modify: `crates/turing-machine/src/guard.rs` (`row_matches`)
- Modify: `crates/turing-machine/src/expand.rs` (`expand_rule` combo loop
  ~788: skip a combo whose guard fails; `empty-expansion` warning at the
  ~512 precedent when a rule's product is non-empty but the filter leaves
  none)
- Test: `tests/rule_guards.rs`

**Interfaces:**
- Consumes: `check_rule_guard`'s resolved `Guard`s, expand's `env:
  HashMap<&str, &BoundVal>`.
- Produces: filtered rows (observable only through compilation output).

- [ ] **Step 1: The failing equivalence tests** — derivation-first, guard
  fixture vs its handwritten product (rows from linked disassembly, and full
  image equality vs the handwritten twin's image):
  - the issue's headline: `['a'..'z' as x, 'a'..'z' as y] where { x == y ||
    x == y - 1 }` keeps exactly the 51 rows the hand-written diagonal +
    subdiagonal states (derive both row lists in code, assert equal);
  - `where { x == y }` over `'a'..'c'` × `'a'..'c'` = the 3 diagonal rows;
  - cone locality at RUN level: `where { (x in qq && y in qq && x < y) ||
    x == y }` run on tapes witnesses both legs per the set's order vs the
    alphabet's (fixtures where the two disagree, e.g. `set qq { 'z', 'a' }`);
  - a routine body and the same rule written unguarded (handwritten rows)
    fold to identical rows (the spec's acceptance spans machine/routine/graft
    bodies — the graft half is Task 4's extern pin);
  - `a_guard_that_keeps_no_rows_warns_empty_expansion` (the `empty-expansion`
    diagnostic, message naming the rule);
  - `the_product_threshold_counts_before_the_filter`: 25×25 keeping few rows
    still trips `binding-product-threshold` on lint AND the compile-side
    cutoff behaves as before.
- [ ] **Step 2: Verify FAIL.**
- [ ] **Step 3: Implement** `row_matches` (identity when both sides are
  offset-free terms; else gather each operand's index in its cone context
  sequence — a label not on that sequence is off-line and matches nothing;
  integer offsets; `In` is membership in the resolved set) and the filter in
  `expand_rule`'s combo loop, applied to the filled cells' env (env keys are
  binding names).
- [ ] **Step 4: Verify green + mutation check:** flip a comparison operator
  (`<`→`>` inside `row_matches`) — the headline derivations redden; revert.
- [ ] **Step 5: Commit** `feat(turing-machine): a guard filters the expansion`.

### Task 4: Cross-unit truth — graph space, headers, digests

**Files:**
- Modify: `crates/turing-machine/src/header.rs` (`rule_text` prints
  `guard_text(...)` after the pattern; the `graph_digest` named-content
  appendix scans guard `In` set names too — the three `SetRef` scan sites at
  ~1704/~1777/~1941 are the pattern to extend)
- Test: `tests/rule_guards.rs` (CLI-`--extern` harness, `ScratchDir`)

**Interfaces:**
- Consumes: `guard::guard_text`, the Task 3 filter (already graph-space via
  `GraphExpansion`'s memoized graph-world expansion + `map_rule`).
- Produces: headers carrying guards; digests covering them.

- [ ] **Step 1: The failing pins:**
  - `a_guarded_graph_body_splices_graph_space`: a library graph with
    `x < y` guard over its own alphabets, grafted through `--extern` with an
    order-NON-preserving `with map` — emitted assembly equals the same host
    grafting the unguarded handwritten rows (filter-then-map, never
    host-space);
  - `a_twin_header_is_still_one_header`: named/positional twin sources with
    the same guard print byte-identical headers and the header text contains
    the guard;
  - `guard_set_drift_is_still_graft_drift`: two units' set declarations with
    the same name but different members → `GraftDrift` at link
    (the digest appendix must see the guard's `in S`);
  - `an_unfiltered_guard_never_reaches_a_header` (anti-two-truths): `tmt
    interface` over a guarded source prints the guard, and the `.tmh`
    round-trips through `tmt fmt` unchanged.
- [ ] **Step 2: Verify FAIL** (the header-twin one is expected to fail
  exactly as the named-cells round's was NOT — assert an anti-vacuity
  `contains("where")` needle, and check it cannot pass with empty bodies).
- [ ] **Step 3: Implement** the two printer/appendix changes.
- [ ] **Step 4: Verify green + mutation check:** drop the appendix scan —
  `guard_set_drift_is_still_graft_drift` redden (drift goes quiet = the
  bug), revert. Full crate green incl. `std.tmh` pin (stdlib has no guards —
  its bytes must not move).
- [ ] **Step 5: Commit** `polish(turing-machine): guards travel through headers and digests`.

### Task 5: fmt and the comment audit

**Files:**
- Modify: `crates/turing-machine/src/fmt/print.rs` (`guard_text`-style
  printing from the raw tree + trivia slots; names of bindings verbatim)
- Modify: `crates/turing-machine/tests/comment_positions.rs` (new slot
  group; update the count in its doc header)
- Modify: `crates/turing-machine/tests/fmt_tmc.rs` (`ADVERSARIAL` array) +
  `tests/fmt_adversarial/guards.tmc` + `guards.fmt`
- Test: above + `tests/rule_guards.rs` fmt pins

**Interfaces:**
- Consumes: the Task 1 tree/trivia; the never-move machinery
  (`fmt/trivia.rs`'s comment slot handling).

- [ ] **Step 1: The failing pins:** `a_guard_survives_fmt_verbatim` (canonical
  guarded fixture == its own fmt output, names and all), an idempotence pin
  over a scraggly guarded fixture, and the new comment group:
  `]
  `where`-`{`, `{`-operand, operator-operand slots around `==`/`&&`/`in`,
  `}`-`->` — each `@C@` at line END, both flavours (`/* c */`, `// c`),
  the three mechanical gates (neighbour invariance, token preservation,
  fixed point).
- [ ] **Step 2: Verify FAIL** — the named-cells-style RED is expected on the
  clean-reprint pin (guard dropped) if the printer is naive.
- [ ] **Step 3: Implement** the guard slot in the rule printer (between
  pattern and `->`), the grid interaction (a BLOCK comment keeps the rule on
  the grid; a LINE comment takes it off — the established rule applies
  unchanged), and the adversarial pair via `tmt fmt - --lang tmc` (NOT
  `fmt PATH`, which writes in place).
- [ ] **Step 4: Verify green:** `cargo test -p mtc-turing-machine --test
  comment_positions --test fmt_tmc`, and update the audit count in the test
  doc header (and leave the CLAUDE.md count to Task 7's doc sweep if
  preferred — one place writes both, note it in the ledger).
- [ ] **Step 5: Mutation check + commit:** clean-reprint without the guard
  arm — `a_guard_survives_fmt_verbatim` must redden; revert; commit
  `feat(turing-machine): fmt keeps the guard, the audit grows its slots`.

### Task 6: `redundant-guard-narrowing` (warn-tier audit)

**Files:**
- Create: `crates/turing-machine/src/lint/rules/redundant_guard_narrowing.rs`
- Modify: `crates/turing-machine/src/lint/mod.rs` (registry entry in the
  shared allow namespace; mirror `index-identity-map`'s warn-tier plumbing)
- Modify: `docs/tmt/lint.md` (inventory row — drift-guarded)
- Test: the new rule's `#[cfg(test)]` block + `tests/rule_guards.rs`

**Interfaces:**
- Consumes: resolved guards where the cell's produced-glyph set is
  computable (range/set/single/wildcard cells — same facts
  `binding_product_threshold::factor` reads).
- Produces: `check(ctx: &LintContext, out: &mut Vec<Diagnostic>)`;
  code `redundant-guard-narrowing`.

- [ ] **Step 1: The failing tests, both directions** (spec's Lint section):
  warns on `['a'..'z' as x, 'a'..'z' as y] where { x in abc && y in abc &&
    x < y }` (`set abc` = the whole alphabet) — message says the conjunct
  narrows nothing here; SILENT on a conjunct whose cell lies outside `S`, on
  a cross-alphabet pair leaning on it as license, and on `x in S
  && x == y` where the cell can leave `S` (restriction still cuts rows).
- [ ] **Step 2: Verify FAIL.**
- [ ] **Step 3: Implement** the two-part test from the spec: (1) the
  binding's cell glyph set ⊆ `S` members, AND (2) every order/offset
  comparison of the binding keeps its license with the conjunct removed
  (re-run the cone check minus this `In`). Warn-tier, `fix: None`.
- [ ] **Step 4: Verify green + mutation check:** drop condition (2) — a
  cross-alphabet pin reddens (false positive), revert.
- [ ] **Step 5: Commit** `feat(turing-machine): a guard narrowing that does
  nothing says so`.

### Task 7: Docs and version record

**Files:**
- Modify: `docs/tmt/language.md` — "Rule guards" section (syntax, typing
  table, the `&&`-cone rule with the three branch shapes, the two-domain
  contrast with write-folds, the empty-guard warning) + version history
  entry (`where`, reserved word #32)
- Modify: `docs/tmt/fmt.md` — the guard's never-move slots
- Modify: `CHANGELOG.md` — `## [Unreleased]` entry with the "0.3, no
  movement" version block
- Modify: `CLAUDE.md` — reserved-word count 31 → 32 and the comment-audit
  count wherever it stands
- Test: the docs drift guards (`error_code_docs`, `docs_drift`, `man`) —
  green, no new rows expected beyond Task 2/6's

- [ ] **Step 1: Write the section** with the spec's examples quoted from RUNS
  (each fenced example either compiles to the rows stated or is a stated
  refusal — verify with `tmt` before pasting).
- [ ] **Step 2: Run the doc gates**
Run: `cargo test -p mtc-turing-machine --test error_code_docs && cargo test
-p mtc-turing-machine --lib docs_drift && cargo test -p mtc-turing-machine
--lib man`
Expected: all green.
- [ ] **Step 3: Commit** `docs(turing-machine): rule guards`.

### Task 8: Workspace gates, final review, mutation checklist

- [ ] **Step 1: Full gates**
Run: `cargo test --workspace`; `cargo clippy --workspace --all-targets --
-D warnings`; `cargo fmt --check`; `cargo build -p mtc-core
--no-default-features` (core untouched — sanity).
Expected: green; the pass/failed/ignored sums recorded in the ledger.
- [ ] **Step 2: Diff audit** — `git diff --name-only master-next-range`
  shows only the Global Constraints boundary (plus `editors/grammars/`).
- [ ] **Step 3: Fresh whole-branch review** (subagent; the plan's Review Focus
  verbatim) + ONE fix-pass over Critical/Important.
- [ ] **Step 4: Mutation checklist** — one row per failure route, verified or
  reasoned in the ledger:

| Mutation | Repaired by | Verification |
|---|---|---|
| comparison operator flipped in `row_matches` | Task 3 derivations | red |
| cone walk widened to the whole guard | Task 2 refusal + cone-local pins | red |
| equality typed via positions | Task 2/3 identity pins (cross-alphabet `==`) | red |
| off-line value wraps to the first member | Task 3 boundary rows | red |
| header prints the guard's rows unfiltered (two truths) | Task 4 twin/`interface` pins | red |
| digest appendix skips guard sets | Task 4 `GraftDrift` pin | red |
| fmt clean-reprint drops the guard | Task 5 verbatim pin | red |
| `redundant-guard-narrowing` drops its license condition | Task 6 cross-alphabet pin | red (false positive) |

- [ ] **Step 5: Report** — ledger summary; merge into `next` + push + worktree
  cleanup ONLY on the partner's explicit confirmation (the established
  precedent).
