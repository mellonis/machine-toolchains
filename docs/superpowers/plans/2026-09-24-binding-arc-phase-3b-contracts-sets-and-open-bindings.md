# Binding Arc, Phase 3b — Head Contracts, Named Sets and Open Bindings

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Teach the `.tmc` language the three things the completeness analysis
found missing — a head-position contract on a tape parameter, a named glyph
set, and an open binding — and make the toolchain honest about each: statically
where inference is exact, at run time where it is not, and in the header either
way. Phases 1 and 2 already put `enters`, `leaves`, `opaque` and the `open` bit
on the wire and taught the linker to enforce them; **nothing consumes them
yet.** 3b is the producer and the consumer.

**Architecture:** One clause grammar, one expansion path, one printer.
`enters { … }` and `leaves { … }` reuse the `writes`/`preserves` production
verbatim — same element grammar, same green node kind, same reparse shim — and
a named `set` joins that same element grammar, which lights it up in alphabet
bodies, contract clauses and pattern cells in one change. The clauses travel
outward through the single `published_writes`-shaped join point into `.param`
and both `tmt interface` arms; inward they drive a static check on the callee's
entry state (reusing the accepted-glyph primitive `state-may-trap` already
computes) and, in debug builds only, a synthesized check state per tape whose
catch-all raises a new named trap kind. Stripping is a **compiler option**, not
a codegen filter: with `--strip-asserts` the states are never built, so a
source with clauses compiles byte-for-byte to the same object as that source
with the clauses deleted. The strict range/set rule turns a silent per-cell
drop into an error at the one authoring-time site, leaving the graft-time
`empty-expansion` — a different mechanism at a different stage — untouched.

**Tech Stack:** Rust (pinned toolchain in `rust-toolchain.toml`), `proptest`
where a codec is touched, `cargo nextest run --workspace` as the final gate, no
new dependencies.

**Spec:** `docs/superpowers/specs/2026-09-13-issue-95-binding-analysis.md` —
the whole of "### Folded in from #121 (ratified 2026-09-13)" plus the 3b lines
of "### Versions, sequencing, gates".

**Predecessor:**
`docs/superpowers/plans/2026-09-15-binding-arc-phase-3a-compiler-and-headers.md`,
landed on master at `98590e3`. **The code on this branch is the authority**;
where 3a's plan and the tree disagree the tree wins, and this plan says so
under "Established facts".

**Successor:** phase 4 — `outline` multi-exit, the LSP overlay including `.tmh`
routing in both editor pairs, the stdlib rewrite, `docs/lsp.md`, `CHANGELOG.md`
and the 0.6 release. Nothing from it enters 3b.

**Deferred to phase 4, named here so the deferral is explicit rather than
silent** (each is a clause of the #121 section that 3b does NOT cover):

- **`enters-unmet` "runs in the editor through the overlay."** 3b ships the
  rule for `tmt lint` and the compiler; surfacing it in the LSP cross-file
  overlay is the phase-4 overlay round, with every other overlay surface.
- **`.tmh` routing in the editors** — `bind_service`'s `.tmh` arm, the VS Code
  `contributes.languages` entry and the JetBrains file type plus parser
  definition. 3b makes `tmt fmt`/`tmt lint` accept a header from the CLI only.
- **The `std.tmc` rewrite** — the header comment, the named map, the exported
  map that 3a's M4 carry would otherwise want. 3b's single stdlib edit is the
  `enters`/`leaves` annotation at an unchanged line count (R-3b-3).

**Reference map gathered for this plan:** `.superpowers/compiler-map-3b.md` —
file:line for every surface below, with a VERIFICATION PASS block at the top
recording thirteen corrections and confirmations made against this tree.
**Fixture verification log:** `.superpowers/plan3b-fixture-check.md` — every
command, exit code and first output line behind a `[tool-verified]` mark, plus
the corpus sweep.

---

## Rulings carried in (do not re-open)

From the controller's brief, ratified 2026-09-24:

- **R-3b-1 Properties, not mechanisms.** In 3a, three of seven review-fix
  cycles came from plan text that prescribed a mechanism and the mechanism was
  the defect. Every task below opens with (a) the PROPERTY that must hold, in
  one sentence a reviewer can falsify, (b) the test that falsifies it and the
  mutation that test catches, and only then (c) a design **labelled as
  suggested**. Where the spec prescribes a mechanism — the check-state shape,
  the dataflow seeds — it is quoted as the spec's suggestion and the property
  stays primary. **One concept, one implementation:** a task that would add a
  second resolver, converter, printer, guard or import-walk beside an existing
  one reuses the existing one instead, and names it.
- **R-3b-2 Version literals.** `TMC_LANG_VERSION` stays `0.2` (unreleased, and
  3b's grammar additions join it). `TM_IR_VERSION` stays `4` — `IrTape` gains
  `enters`/`leaves`/`opaque` inside the unreleased v4. The `.tma` TM-1 dialect
  stays `0.5` unless 3b changes the `.tma` grammar; phase 1 already added every
  `.tma` form 3b needs (verified — see Established fact 2), and if a change
  proves unavoidable it joins 0.5, which is also unreleased. `tmt.json`
  `project` goes `0.2` → `0.3` in the task that adds `strip-asserts`, and in no
  other. Reserved words go **28 → 31** (`enters`, `leaves`, `set`) — the spec
  says 27 → 30 and is stale by one; the tree wins. Crates stay `0.5.x`; MO
  stays 4.
- **R-3b-3 The embedded stdlib.** 3b annotates `std.tmc`'s tape parameters with
  `enters`/`leaves` derived from their `?` doc lines — the ONE `std.tmc` edit
  of the arc before phase 4 — **keeping the file's line count unchanged**,
  because the browser binding maps stdlib addresses to source lines. Every
  embedded stdlib object (both arch-crate presets and the wasm crate's
  debug-compiled twin) is built with asserts STRIPPED, so no compiled-object
  byte pin moves for the check states; the task states which pins DO move (the
  `std.tmh` regen) and proves the object pins do not. **A stdlib routine whose
  `?` lines and body disagree is reported, not silently annotated.**
- **R-3b-4 Assert stripping is a compiler option, not a codegen filter.** With
  `--strip-asserts` the synthesized states do not exist in the IR at all. So
  `-O0` bit identity for a source without clauses holds **by construction**,
  and `-O0` of a source WITH clauses under `--strip-asserts` equals the same
  source with the clauses deleted — tested byte-for-byte.
- **R-3b-5 Core neutrality.** Trap kind `#2` is named in core only because core
  already names `#0`/`#1` there (`RaisedTrapKind`, verified — Established fact
  4). Every `crates/core` diff is justified by file in its own task. The budget
  is **two core SOURCE diffs** — the trap kind and the exit-return body scan —
  **plus one core TEST addition**, the execution-trap-table drift guard, which
  the controller ruled free because a test that guards a registry against its
  published table adds no behaviour and encodes no architecture knowledge.
  Global Constraints states the budget in those terms and is the operative
  text.
- **R-3b-6 Lint on a `.tmh`.** A header is declarations-only, so `unused-*`,
  `dead-*` and every body rule are meaningless on it. The proposal is under
  "Decisions for the controller"; `fmt` on a header uses the SAME printer as
  `tmt interface`'s canonical form, never a second one.
- **R-3b-7 Docs are a task, not a footnote.** The last task before the full
  gate documents every surface and lists its claims; each is verified against
  the built tool by the implementer. A carries file (`task-16-carries.md`)
  accumulates execution-time rulings for it.
- **R-3b-8 Behaviour-change honesty.** "Behaviour changes in phase 3b" names
  every observable change in prose a release note can quote — above all the
  strict range rule, which turns previously-accepted programs into errors.

Ratified 2026-09-13 with the arc and still binding:

- Head contracts are **optional by construction, never empty**: absence of a
  clause is absence of a promise, exactly as with `writes`. Nothing is
  inferred when a clause is absent, the interface field is `None`, `.param`
  carries no suffix, and `enters-unmet` never fires against a callee without
  `enters`. An *empty* clause is meaningless and is the compile error
  `empty-head-clause`; the object reader rejects an empty present list as
  `Malformed`, so `Some(vec![])` never exists in a `RoutineInterface`.
  (`writes {}` keeps its meaning, "writes nowhere" — a different kind of claim.)
- The runtime check is the authority, which is why the static site check is a
  warning-tier lint and not an error: static inference loses everything after a
  `move` and widens through one-way collapse maps.
- `set` is never a tape type (no index space, no blank) and never legal in a
  write cell (a write is one symbol; `{d}` writes what matched).
- `opaque` is inferred and exported, never spelled in `.tmc`.

Carried from 3a's own rulings and reviews:

- **Ruling 36** — "the continuation is the lone trap" counts as no
  continuation; the trap arm does not require the trap to end the function.
- **Ruling 31** — IR stays 4; additions join the unreleased v4.
- **A labelled pair carries `dst` 0** on the wire, and `open ⇒ map_written`.
- **N-1 (3a final re-review)** — a consumer object is byte-identical whether a
  header bound the right same-named alphabet or the wrong one, because a call
  binds by index. **"Byte-compare the consumer's object" is not an instrument
  for header-binding correctness**; do not reach for it.
- **N-2 (3a final re-review)** — the `use`-line rule's second half ("prints
  nothing unneeded") is unguarded; folding it into the corpus round trip is a
  3b carry.

---

## Global Constraints

**Every task's requirements implicitly include this section. Re-read it before
each task.**

- **PM-1 byte identity.** No byte of any `pmt` output may change. This is a
  TM-language phase; `crates/post-machine` should not appear in any diff at
  all. The two tasks that touch `crates/core` (Task 5, Task 14) name
  `cargo test -p mtc-post-machine --test golden_programs` and
  `cargo test -p mtc-post-machine --test asm_volatile` in their own gate steps.
- **Core neutrality.** `crates/core` learns nothing about TM-1. **A new
  `crates/core` diff in a language phase is a design smell — justify each
  one.** The budget is **two core SOURCE diffs plus one core TEST addition**,
  and no others:
  - **Task 5** — a third member of `RaisedTrapKind`, core's own arch-agnostic
    vocabulary of kinds an architecture MAY raise, where core already names
    two (source).
  - **Task 14** — a link-time scan over fields already on the wire, mirroring
    `stamp.rs::callee_can_return` (source).
  - **Task 5** — the `docs/core.md` execution-trap-table drift guard, a new
    test in `crates/core/tests/` (test).

  **A core test that guards a registry against its published table is free
  under this rule**, because it adds no behaviour and encodes no architecture
  knowledge — it is the repo's standing drift-guard doctrine applied to a
  table that currently has none. Source diffs are what the budget counts. If a
  task finds itself reaching into core **source** outside the two above, stop
  and report.
- **`-O0` bit identity and the `brk` barrier.** A program that declares no
  head contract compiles byte-identically to today at `-O0`, and
  `opt_equivalence.rs::brk_barrier_blocks_elimination` stays green. Under
  R-3b-4 the first half is by construction, not by a filter.
- **Compiled-stdlib byte identity at BOTH opt levels** is a gate on every task
  touching the compiler or codegen: compile `stdlib::SOURCE` before and after
  and byte-compare the object at `-O0` and `-O1`. **Every path in the tree that
  recompiles the embedded stdlib strips asserts — the two shipped presets AND
  the test helpers that execute this gate — and the sites are enumerated with a
  per-site ruling in the `--strip-asserts` task.** The gate is executed by
  helpers that build their own `CompileOptions`, not by the presets, so
  "the presets strip" does not by itself make the gate hold; that is why the
  enumeration exists and why it lands in the task that adds the field, before
  any assert exists and while every edit there is still a provable no-op.
- **The everything-matrix, `mode_equivalence`, the three-mechanism `.tma`
  matrix and the composition law tests stay green**
  (`crates/turing-machine/tests/opt_equivalence.rs::everything_matrix_is_green`,
  `tests/mode_equivalence.rs`, `tests/link_matrix.rs`,
  `crates/core/src/linker/compose.rs`'s law tests,
  `crates/core/tests/link_open.rs`).
- **Drift guards are set-compares in BOTH directions, and every one a task
  trips is named in it**: `crates/core/tests/error_code_docs.rs`,
  `crates/turing-machine/tests/error_code_docs.rs`,
  `crates/turing-machine/tests/cli_docs.rs` (byte-compares usage blocks against
  `docs/tmt/cli.md`), `tests/completions_registry.rs` + `EXPECTED_TOP_LEVEL`,
  `tests/man_page.rs`, `tests/editor_grammar.rs`, the reserved-word count in
  `lexer.rs`, `syntax/kinds.rs`'s discriminant-range guard,
  `src/lint/docs_drift.rs`, `project.rs::the_bundled_schema_matches_the_key_inventories`
  and `every_inventory_is_sorted_and_unique`, `tests/stdlib_header.rs`,
  `crates/wasm/tests/stdlib.rs`, the IR round-trip test, `tests/link_matrix.rs`.
- **Docs policy.** Published pages (`README.md`, `CHANGELOG.md`, `docs/**`) and
  code comments cite `docs/<page>.md (keyword)` only — no `spec §N`, no
  `Task N`, no ruling numbers, no issue/PR numbers, no hosting URLs. **Forward
  citations are accepted within this phase**; the final whole-branch review
  checks that every one resolves to a real page-plus-keyword. Rulings are
  described to implementers in PROSE — in 3a an implementer copied a ruling
  label into code. This plan is an internal artifact and may cite freely.
- **Commits.** Conventional with scope (`feat(turing-machine):`,
  `fix(turing-machine):`, `test(turing-machine):`, `feat(cli):`,
  `docs(turing-machine):`, `feat(core):`). Implementer subagents MAY commit
  their own task on branch `binding-arc-4` **once the user has granted it —
  the controller confirms before Task 1**; merging and pushing stay the
  owner's. **Commit messages carry no Claude attribution and no
  `Claude-Session:` line** — the harness appends one, so every commit step ends
  with `git log -1 --format=%B` and, if a trailer was injected,
  `git commit --amend -S -F <file>` with the message stripped back. Messages
  are written to a file and passed with `-S -F`.
- **`git add` explicit paths, never `-A` or `.`.**
- **Temp paths in tests**: PID plus a per-call atomic counter, never a fixed
  name. Copy `fn scratch` from `crates/turing-machine/tests/mode_equivalence.rs`
  verbatim into any new TM test file that writes to disk, and **locate it by
  symbol name** (`grep -n 'fn scratch'`) — it has moved repeatedly.
- **Every `file:line` in this plan and in the map is a bearing, not an
  address.** Locate by symbol name and treat the number as a hint.
- **Every fixture is run through the real tool before it is trusted.** Fixtures
  marked **[tool-verified]** were run by the planner, with command, exit code
  and first output line in `.superpowers/plan3b-fixture-check.md`. Fixtures
  marked **[shape-derived: …]** name what they were derived from and what the
  implementer runs to confirm them — always as **step 1** of the owning task.
  A fixture that does not compile is a plan defect; report it, do not paper
  over it.
- **Every test states, in one line, the mutation it catches**, and the fixture
  must FAIL under that mutation. **ONE guard per property** — in 3a a test's
  stated mutation could not fail because two guards covered the same property.
  **For each new error or lint code there is a firing fixture and a near-miss
  fixture.** **For each runtime assert there is a program that traps under
  debug and runs clean under `--strip-asserts`, under all three call
  mechanisms, with SEEDED tapes.**
- **Cargo invocations** are prefixed
  `CARGO_TARGET_DIR=/Users/mellonis/Developer/mellonis-workspace/machines/toolchains/target`
  and run from the worktree root, one at a time, in the foreground. The final
  gate is `cargo nextest run --workspace` (never `cargo test` alone — one
  process per test is what exposed the stdlib `OnceLock` parse), plus
  `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`,
  `cargo build -p mtc-core --no-default-features`,
  `cargo build --workspace --lib --target wasm32-unknown-unknown`,
  `scripts/build-wasm-bundle.sh` **without** the target-dir prefix (it cannot
  honour one) followed by `node scripts/wasm-smoke.mjs target/wasm-bundle/dist`,
  and the two PM byte-identity tests.

---

## Established facts this plan builds on

Verified against this working tree at `98590e3` on 2026-09-24, each with its
evidence. An implementer may rely on these without re-deriving them.

1. **The wire is already built; 3b produces into it.** `RoutineInterface`
   (`crates/core/src/formats/object/mod.rs:229-253`) already carries
   `enters: Vec<Option<Vec<String>>>`, `leaves: Vec<Option<Vec<String>>>` and
   `opaque: Vec<bool>` with doc comments stating exactly the semantics this
   plan implements. `TapeBinding.open` (`:194-212`) and `BoundCall.exits`
   (`:226`) likewise. `linker/interface.rs::check_opaque` (`:183-205`) already
   refuses an open binding into a non-opaque tape as
   `LinkError::OpenBindingUnsupported`. **Nothing in `crates/core` or the
   compiler reads `enters`/`leaves` today**; no `.tmc` grammar populates them.
   There is no object-format design decision left to make.

2. **Every `.tma` form 3b needs already exists.** `asm/cst.rs:506-521`
   `BindingEntryCst { param, phys, map_written, open, pairs }`;
   `asm/cst.rs:1922-1934` `parse_binding_entry`; `asm/lower.rs:2182-2227`;
   `asm/disassembler.rs:313-358` `render_binding` prints `{*}` / `{3->'0',*}`.
   The `.tma` dialect therefore stays `0.5` (R-3b-2). If a task finds a gap,
   it stops and reports rather than bumping.

3. **`sym_lit` refuses a bare `*`, and the refusal comes from inside
   `map_pair`.** `parser.rs:2160-2180` has arms for `Glyph` and `Number` only;
   `map_pair` (`:2696-2711`) calls it unconditionally. Live: `tmt compile` on a
   fixture containing `with map { 'a' -> 'a', * }` exits 1 with
   `error: expected a glyph or number, found `*` [unexpected-token]` spanned at
   the `*`. **[tool-verified]** log F2. The open-binding grammar is a lookahead
   in the pair-list loop, not a relaxation of `sym_lit`.

4. **Core already names trap kinds `#0` and `#1`.** `vm/trap.rs:106-112`
   `pub enum RaisedTrapKind { UnmappedRead, UnmappedWrite }`, consumed by
   `vm/core.rs:644-650` and constructed by the crate-private fake arch at
   `vm/arch.rs:292,295`. The number→kind mapping lives once per arch:
   `crates/turing-machine/src/arch/mod.rs:228-239`, whose own unit test
   `trap_lowers_to_the_two_raise_kinds_and_rejects_other_kinds` (`:645-667`)
   **currently pins `Imm(2)` as `BadOperand`** — giving `#2` a meaning flips
   that assertion, it does not merely add one. PM-1 has no trap instruction at
   all (`trap_opcode: None`).

5. **`engine.rs::next_is_trap` matches by opcode alone.** `:932-934`, with a
   doc comment at `:924-931` stating it never inspects "the immediate a
   compiler is free to spend on a different trap kind". Moving the
   continuation-less-call trap from `#0` to `#2` cannot disturb
   `tail-call-no-continuation`.

6. **`docs/core.md`'s Execution trap table has NO drift guard.**
   `crates/core/tests/error_code_docs.rs` reads the page only for
   `(error codes)` and `### Link warnings`. A new `Trap` variant is caught by
   no test if the table is not updated by hand.

7. **The two `empty-expansion` diagnostics are different mechanisms at
   different stages.** `expand.rs::splice_state` (`:428-468`) receives an
   `ExpandedState` — the graph body has ALREADY been range-expanded against the
   graph's own tape alphabet — and warns when `map_rule`'s preimage is empty:
   an instantiation fact the author of a generic graph cannot know.
   `expand.rs::cell_options` (`:588-620`) is the authoring-time fact: an
   alternative outside the cell's OWN declared alphabet.

8. **The shipped corpus has no partial range and no out-of-alphabet single.**
   An instrumented `cell_options` (reverted; tree clean) reported ZERO drops
   over `crates/turing-machine/tests/golden`, `docs/examples`,
   `crates/turing-machine/src/stdlib`, `docs/superpowers/probes`,
   `crates/turing-machine/tests/fmt_adversarial`, and the 940 in-crate lib
   tests. The drops are confined to `crates/turing-machine/tests/tmc_never_fires.rs`.
   **[tool-verified]** log, "The corpus sweep".

9. **A zero-row state is authorable without the drop.** `entry state s { }`
   compiles (exit 0). **[tool-verified]** log F1. CLAUDE.md's "zero-row states
   are valid and trap on entry" contract and `codegen.rs::zero_row`'s
   127-symbol bare-`trap` fallback survive the strict rule.

10. **Foldability is enforced only in the parser.** `parser.rs:2028-2074`
    `check_char_arithmetic` decides it from `SymLit::is_glyph()`
    (`parser.rs:114-116`) — true iff the literal was written with quotes.
    `expand.rs` never re-derives it; `eval_fold` merely `debug_assert!`s that
    `BoundVal.value` is `Some`. The fix is parser-side, with `numeric_value`
    (`expand.rs:522-528`) as the shape the parser's new predicate must agree
    with. Firing fixture live: `probe3-sets/dec-glyph.tmc` exits 1 with
    `[char-arithmetic]`; `dec.tmc` and `dec-subst.tmc` compile clean.
    **[tool-verified]** log F7.

11. **`Resolved` carries no set-reference surface.** `compiler.rs:1083-1099`:
    `alphabets`, `maps`, `worlds`, `entry_world`, `docs`. A `set` expands in
    place before a `Resolved` exists, so `unused-set` cannot mirror
    `unused_alphabet.rs` unless resolution records the references.

12. **The accepted-glyph primitive exists but is private.**
    `lint/patterns.rs::cell_labels` (`:50`) is shared by `dead_rule.rs:76` and
    `state_may_trap.rs:44`; `state_may_trap::rule_sets` (`:27-48`) is the
    private wrapper that builds per-rule, per-tape `HashSet<String>` match
    sets. Their union over a state's rules at tape index `k` IS "every glyph
    this state accepts on tape `k`" — exactly what a static `enters` check
    needs, with no dataflow.

13. **There is no fixpoint/dataflow precedent in the lint layer.**
    `state_may_trap` is a single-state input-product enumeration; `dead_rule`
    is same-state band cover; `unreachable-continuation` is single-hop. The
    closest reusable SHAPE in the crate is `footprint.rs`'s monotone
    `SymSet`-per-tape fixpoint (`:302-403` IR-stage, `:711-855` source-stage,
    `project_write_back` at `:189-219`) which answers a different question and
    is not extensible in place.

14. **`--strip-debugger` is a TWELVE-point plumbing job.** Enumerated by
    `grep -rln "strip_debugger"` and `grep -rln "strip-debugger"` over
    `crates/`, `editors/` and `docs/`, TM side only:

    | # | Site | `--strip-asserts` does what |
    |---|---|---|
    | 1 | `compiler.rs` `CompileOptions.strip_debugger` | gains `strip_asserts` |
    | 2 | `codegen.rs` `CodegenOptions.strip_debugger` | **NOT mirrored** — R-3b-4: stripping is a compiler decision and the flag never reaches codegen |
    | 3 | `cli/build.rs` (`compile`'s argv parse, the `--release` fold) | mirrored |
    | 4 | `cli/driver.rs` (`Flags`, `argv_compile_options`, `build_one_target` where the flag beats the profile) | mirrored |
    | 5 | `project.rs` `ProfileOverrides` / `ResolvedProfile` / both `Profiles::resolve` bases / `PROFILE_KEYS` (**must stay sorted**) / the `parse_profile` arm | mirrored |
    | 6 | `editors/schemas/tmt.schema.json` | mirrored |
    | 7 | `completions/registry.rs` `compile_spec` + `build_spec` | mirrored |
    | 8 | **`completions/fish.rs`** | mirrored |
    | 9 | **`crates/turing-machine/src/stdlib/mod.rs`** — the embedded preset | set to `true` (see below) |
    | 10 | **`crates/wasm/src/inner/stdlib.rs`** — the debug-compiled twin | set to `true` (see below) |
    | 11 | `docs/tmt/cli.md` | mirrored |
    | 12 | `docs/tmt/project.md` | mirrored |

    Sites 8, 9 and 10 are the ones a nine-point reading misses, and 9 and 10
    are load-bearing for R-3b-3. **Both preset literals spell every
    `CompileOptions` field out rather than using `..Default::default()`**
    (verified: `stdlib/mod.rs`'s literal ends `inline_cap: None,` with no
    tail, and its comment explains why the tail is forbidden — it would
    evaluate `Declarations::stdlib()` and read the very cache it is building).
    **So adding a field to `CompileOptions` is a compile error at both sites
    until they are updated** — the implementer cannot silently miss them, but
    must set them to the RIGHT value rather than whatever compiles.

15. **`tmt.json` `project` 0.2 has no code literal.** The version is prose in
    `docs/tmt/project.md:16-19`; no test asserts `"0.2"`. The 0.3 bump is docs
    plus the new key's code spots and the JSON Schema.

16. **`.tmh` is already a first-class file kind everywhere except two CLI
    dispatch sites.** `tmt interface -o OUT.tmh` generates them; `--extern`
    accepts them (case-insensitively) on both `compile` and `interface`;
    `header::resolve_declarations` reads them in a fixpoint; the completions
    registry already lists `tmh` as an extension for `--extern`
    (`registry.rs:271-275`, `:524-528`); `fmt::format` is pure `.tmc`-grammar
    text→text and accepts the bodiless form unchanged. The gap is
    `cli/fmt.rs:161-165` + `cli/lint.rs`'s twin match arm, and
    `cli/lint.rs::collect_sources`'s extension filter (`:244`), plus
    `registry.rs::source_or_dir` (`:428-436`). Live: an explicit `.tmh` is
    REFUSED with `unknown source extension (expected .tmc or .tma)`; a `.tmh`
    in a scanned directory is SILENTLY SKIPPED. **[tool-verified]** log F5, F6.

17. **Three independent "28"s exist and only one moves.**
    `lexer::RESERVED.len()` (the reserved-word count — this one moves),
    `syntax/kinds.rs`'s significant-token census, and
    `tests/editor_grammar.rs`'s TextMate keyword set-compare (which reads
    `RESERVED` directly, so it moves WITH it, not independently). Separately,
    `parser.rs:13` says "The 27 reserved keywords" — a pre-existing off-by-one.
    **The spec's "27 → 30" is stale; the tree's number is 28, so 3b goes to 31.**

18. **`syntax/kinds.rs`'s node-kind guard under-covers an appended
    discriminant** unless its range is widened in the same edit
    (`kind_name_never_falls_through_for_an_occupied_discriminant`, `:406-418`,
    walking `(0..=30).chain(32..=55)` with `31` pinned as the deliberate gap).
    Its own doc comment calls this out as its blind spot.

19. **`tmt lint` has no `--fix` DRIVER, but TM-1 lint rules emit fixes
    routinely, and the deletion fix `unused-set` needs already exists.**
    Twelve rule files under `crates/turing-machine/src/lint/rules/`
    construct a `Fix`; `crates/turing-machine/tests/lint_fix_comment_guard.rs`'s
    module doc states the roster verbatim — *"eleven rules emit a `Fix`. Nine
    are pinned here"* — and holds nine `*_withholds_the_fix_*` pairs plus a
    `.tma` arm, with `contract-clause-overlap` pinned in its own unit tests and
    `dead-map-pair` exempt by mechanism (a single-token edit span can never
    contain a comment). Applied texts are pinned in
    `crates/turing-machine/tests/lint_quickfix_comments.rs`. **The mirror
    `unused-set` should copy is `lint/rules/unused_map.rs`** — a
    whole-declaration delete:

    ```rust
    let fix = decl_span::<MapDeclView>(ctx, map.name_span).map(|span| Fix {
        description: format!("delete the unused map `{name}`"),
        applicability: Applicability::MaybeIncorrect,
        edits: vec![Edit { span, replacement: String::new() }],
    });
    ```

    The only true statement in the neighbourhood is the narrow one in
    `cli/lint.rs`'s module doc — there is no `tmt lint --fix` command — and
    **that comment is itself stale** in claiming "no `.tmc` or `.tma` rule
    emits a machine-applicable fix". Correcting it is a docs-task item.

20. **No corpus identifier collides with the three new keywords.**
    `grep -rnwE "enters|leaves|set"` over every `.tmc`/`.tmh`/`.tma` in
    `crates/` and `docs/` hits only comment text and `?` doc-line prose, both
    free text to the lexer. A wider grep catching inline `#[cfg(test)]` string
    fixtures under `crates/turing-machine/src` and `tests` is also clean.

21. **`AlphabetElem` and `PatternCellKind` are two different enums with two
    different productions, and nothing funnels between them.**
    `alphabet_elem()` has exactly two callers — `alphabet_elems`
    (`parser.rs:1514`) and `contract_clause` (`parser.rs:1727`).
    `pattern_cell` (`parser.rs:2097-2122`) matches the token itself, calls
    `sym_or_range()` directly, builds `PatternCellKind` (`parser.rs:391-395`),
    and its fallback arm **refuses an identifier outright**. Verified live:
    `[digits] -> stop;` fails with
    ``error: expected a pattern element (glyph, number, range, or `*`), found `digits` [unexpected-token]``.
    A `set` therefore needs **two** grammar surfaces, not one.

22. **Every `PatternCellKind` match site, by grep** (the fan-out a new variant
    must service): `parser.rs:2040-2042` (`check_char_arithmetic`),
    `parser.rs:2102-2113` (construction), `expand.rs:591-609` (`cell_options`),
    `lint/patterns.rs:52-54` (`cell_labels`),
    `lint/rules/binding_product_threshold.rs:21-23`, `fmt/print.rs:1050-1052`,
    `header.rs:1733-1737`, plus three `matches!`-on-`Wildcard` sites that are
    not exhaustive and need no arm (`compiler.rs:1819`, `parser.rs:2132`,
    `lint/rules/unused_tape.rs:65`) and the unit tests in
    `parser/tests.rs:196,208`. **`syntax/extract.rs` and `codegen.rs` do NOT
    match on it** — codegen works on the already-expanded `Cell`.

23. **`tests/opt_equivalence.rs::trap_kind` is an exhaustive match over
    `Trap`.** `:84-103`, 16 arms, with a doc comment saying so on purpose:
    *"Exhaustive on purpose — a new `Trap` variant must be named here, not
    folded into a catch-all that could mask a cross-configuration
    divergence."* A new variant is a **compilation break** of that test
    binary, and `outcome_kind` (`:107-112`) renders `Outcome::Trapped(t)` as
    `"trapped:{kind}"`, so the equivalence contract genuinely discriminates a
    contract trap from any other.

24. **`MapFunction.labels` and `.lines` are "empty without `-g` objects"**
    (`crates/core/src/linker/mod.rs:461-464`, verbatim), and `cli/run.rs`'s
    resolver reads `f.labels`. `MapFunction.name`/`start`/`end` are present
    either way, so a function's own name is resolvable without `-g` — but a
    synthesized check state's label, and with it the tape and clause the label
    encodes, is not.

---

## File structure

```
crates/turing-machine/src/
  lexer.rs                     RESERVED 28 → 31                        (T1)
  parser.rs                    enters/leaves arms + order/dup arms,
                               set decl, AlphabetElem::SetRef,
                               PatternCellKind::SetRef + ident arm,
                               site-map `*`, char-arithmetic predicate,
                               stale "27" doc            (T1,2,9,10a,10b,11)
  syntax/kinds.rs              SET_DECL kind, widened guard range,
                               stale module-doc range                   (T10a)
  syntax/extract.rs            TopView::SetDecl arm, Program.sets       (T10a)
  syntax/views.rs              SetDeclView                              (T10a)
  compiler.rs                  clause resolution (the ONE writes/preserves
                               path), static checks, opaque inference,
                               set expansion, Resolved.set_refs
                                                       (T2,3,4,9,10a,10b)
  patterns.rs        NEW       accepted-glyph primitive, moved out of
                               lint/patterns.rs + state_may_trap.rs
                                                            (T4; +T10b,T11)
  expand.rs                    set cell arm, strict cell_options   (T10b,11)
  ir.rs                        IrTape.enters/leaves/opaque + the v4
                               field-list doc (T3); IrTransition::
                               TrapContract + mermaid terminal + the
                               then:None doc                            (T5)
  codegen.rs                   .param suffixes, trap #2, check states
                                                                  (T3,5,7)
  header.rs                    tape_param_text + BOTH arms, set decls,
                               PatternCellKind arm         (T3,9,10a,10b,15)
  contracts/head.rs  NEW       synthesized check-state construction     (T7)
  lint/rules/enters_unmet.rs   NEW                                      (T8)
  lint/rules/unused_set.rs     NEW, with the deletion fix               (T12)
  lint/rules/contract_clause_overlap.rs  elem_span + elem_indices arms
                               (the Option arm that fails SILENTLY)    (T10a)
  lint/rules/binding_product_threshold.rs  PatternCellKind arm         (T10b)
  lsp/roster.rs                the `_` arm that swallows a SetRef      (T10a)
  fmt/print.rs                 AlphabetElem arm + set decl printing (T10a),
                               PatternCellKind arm                     (T10b)
  cli/build.rs, cli/driver.rs  --strip-asserts, --release               (T6)
  cli/fmt.rs, cli/lint.rs      .tmh dispatch + the shared collect_sources
                               filter + the header-rule flag           (T13a)
  cli/run.rs                   contract trap rendering, -g and no-`-g`  (T7)
  dap/mod.rs                   stop reason `contract`                   (T7)
  project.rs                   strip-asserts key                        (T6)
  completions/registry.rs      --strip-asserts, source_or_dir + tmh
                                                                 (T6,T13a)
  completions/fish.rs          --strip-asserts                          (T6)
  stdlib/mod.rs                the preset sets strip_asserts: true      (T6)
  stdlib/std.tmc, std.tmh      annotation + regen                       (T15)

crates/wasm/src/
  inner/stdlib.rs              the debug twin sets strip_asserts: true  (T6)

crates/core/src/                    [core SOURCE diffs: exactly two tasks]
  vm/trap.rs                   RaisedTrapKind::Contract, Trap::Contract (T5)
  vm/core.rs                   the new Raise arm                        (T5)
  vm/arch.rs                   fake test_arch's own arm                 (T5)
  linker/engine.rs (or stamp)  undeclared-exit body scan               (T14)
  linker/mod.rs                DIAGNOSTIC_CODES row                    (T14)

crates/core/tests/                              [core TEST addition: one]
  execution_trap_docs.rs  NEW  Trap ↔ docs/core.md trap table, both
                               directions, modelled on error_code_docs  (T5)

crates/turing-machine/tests/
  opt_equivalence.rs           trap_kind arm (compile break); the
                               stdlib helper strips (T6); roster
                               literal + its prose message         (T5,6,7,9)
  stdlib_golden.rs, state_params.rs   stdlib helpers strip           (T6)
  state_params.rs              the two moving assertions              (T5)
  lint_fix_comment_guard.rs    unused-set pair + roster count         (T12)
  lint_quickfix_comments.rs    the applied text                       (T12)
  comment_positions.rs         new positions                  (T2,9,10a,10b)
  tmc_property.rs              stamp_elems (T10a), stamp_rule (T10b)
  tmc_never_fires.rs           eight dispositioned fixtures           (T11)
  stdlib_header.rs             the std.tmh pin                    (T13b,15)

crates/turing-machine/src/footprint.rs   the stdlib fixture strips    (T6)

editors/
  grammars/tmc.tmLanguage.json three keywords                           (T1)
  schemas/tmt.schema.json      strip-asserts                            (T6)

docs/
  tmt/language.md, cli.md, lint.md, project.md, isa.md, stdlib.md,
  core.md, dap.md, formats.md, CLAUDE.md                                (T16)
```

---

### Task 1: The grammar bump — three reserved words and nothing else

**PULLABLE: no.** Everything downstream depends on it.

**Property.** `enters`, `leaves` and `set` are reserved words of `.tmc`: each
lexes as a plain identifier token the parser may match by keyword, each appears
in the TextMate grammar's `keyword.control.tmc` tier, and no other count in the
tree claims a different number of reserved words.

**Falsifying test.** `lexer.rs`'s `every_reserved_keyword_lexes_as_a_plain_ident`
with `assert_eq!(RESERVED.len(), 31)` and the three new entries, plus
`tests/editor_grammar.rs::tmc_grammar_covers_exactly_the_reserved_keywords`.
**Mutation each catches:** adding a keyword to `RESERVED` without adding it to
`editors/grammars/tmc.tmLanguage.json` — the set-compare goes red in the
grammar direction; adding it to the grammar only — red in the other direction.
These are the same guard read both ways, which is why they are ONE property and
one task.

**Suggested design.** Append the three names to `RESERVED` in the array's
existing order convention, bump the literal `31` in the count assertion and in
the two `lexer.rs` module-doc sentences, fix `parser.rs:13`'s stale "27" to
"31", and add the three to the grammar's keyword alternation. **Do not touch**
`syntax/kinds.rs`'s significant-token census or its node-kind range: those are
different axes with coincidentally equal numbers today (Established fact 17),
and conflating them is the documented trap.

**TDD steps**

- [ ] **Step 1:** `grep -rnwE "enters|leaves|set" --include="*.tmc" --include="*.tmh" --include="*.tma" crates docs` and confirm every hit is comment or `?`-doc prose. The planner ran this; re-run it because the tree may have moved. If a real identifier appears, STOP and report — a corpus rename is a separate decision.
- [ ] **Step 2:** Update the count assertion to `31` and add the three names to `RESERVED`. Run `cargo test -p mtc-turing-machine --lib lexer` — expect the editor-grammar guard to be the thing that is still red, in the grammar direction.
- [ ] **Step 3:** Add the three keywords to `editors/grammars/tmc.tmLanguage.json` in the `keyword.control.tmc` tier. Run `cargo test -p mtc-turing-machine --test editor_grammar`.
- [ ] **Step 4:** Fix the two `lexer.rs` doc sentences and `parser.rs:13`'s "27". State in the commit body that the "27" was already wrong before this change.
- [ ] **Step 5: Gate** — `cargo nextest run -p mtc-turing-machine`, `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`. `TMC_LANG_VERSION` stays `0.2`: confirm by grep that no bump crept in.
- [ ] **Step 6: Commit** — `feat(turing-machine): enters, leaves and set are reserved words`.

---

### Task 2: `enters { … }` / `leaves { … }` on a signature tape parameter

**PULLABLE: no.**

**Property.** A signature tape parameter may carry `enters { … }` and/or
`leaves { … }` after `writes`/`preserves`, in that canonical order, at most one
of each; the clause body takes the same elements an alphabet body does; every
element must be a glyph of the parameter's own alphabet; an empty clause is the
error `empty-head-clause`; and the green tree still satisfies
`text() == source` over a source carrying both clauses.

**Falsifying test.** A new `tests/head_contracts.rs` holding a source with both
clauses to `text() == source` and to `extract_program`'s round trip, plus these
fixtures, each with the one mutation it catches:

| Fixture | Expected | Mutation it catches |
|---|---|---|
| clause glyph outside the parameter's alphabet | `contract-symbol-unknown` | skipping the membership check for the two new clause kinds (the existing `writes`/`preserves` fixtures stay green, so only this one moves) |
| `enters {}` | `empty-head-clause` | accepting an empty clause as `Some(vec![])` |
| `leaves {}` | `empty-head-clause` | same, on the other keyword — one guard each because the two arms are written separately |
| **near miss** `writes {}` | accepted | conflating "empty clause" with "empty set", which would break `writes {}`'s established meaning |
| `writes … leaves … enters` | `contract-clause-order` | ordering the new arms with no order check at all |
| `writes … enters … preserves` | `contract-clause-order` | checking order only among the new pair and not against the old pair |
| `enters … enters` | `duplicate-contract-clause` (`what: "enters"`) | a single `seen` flag shared by both new keywords — this fixture stays green under that mutation but the next one goes red |
| `leaves … leaves` | `duplicate-contract-clause` (`what: "leaves"`) | the shared-flag mutation above, and a `what` hard-coded to one keyword |

**`enters` and `leaves` do NOT join the overlap computation.**
`contract_clause_overlap::check` (`:182-232`) is strictly `writes` × `preserves`
— it reads `tape.writes` / `tape.preserves` and `writes_clause_for`, and it
will not drift by itself. But the temptation to "extend the overlap check
uniformly to all four clauses" is real and would be **wrong**:
`enters { '$' } leaves { '$' }` is not merely legal, it is the normal shape for
a routine that walks to a marker and stops on it, and flagging it would
manufacture a false positive on correct code. `writes` × `preserves` overlap is
a contradiction ("I write it" and "I leave it alone"); `enters` × `leaves`
coinciding is a statement about two moments, not a contradiction at either.
**Add a clean fixture — a parameter declaring the same glyph in both head
clauses — and assert `contract-clause-overlap` stays silent.**
*Mutation it catches:* extending the overlap computation over the new clauses.

**These are not new error codes.** `CompileErrorKind::ContractClauseOrder`
(code `contract-clause-order`) and
`CompileErrorKind::DuplicateContractClause { what: &'static str }` (code
`duplicate-contract-clause`) already exist and are already raised by the very
loop this task edits — `parser.rs:1650-1668`, declared at `compiler.rs:113`
and `:116`, coded at `:439-440`, displayed at `:598-603`, with a registry pin
at `:4080-4081`. Extending them to two more keywords adds **no** row to
`docs/tmt/cli.md` and trips no error-code drift guard. Only
`empty-head-clause` is new.

**Suggested design** (the spec prescribes only the canonical order; the rest is
this plan's suggestion). `SigParamKind::Tape` grows
`enters: Option<ContractClause>` and `leaves: Option<ContractClause>` beside
`writes`/`preserves`. In `parser.rs::sig_param`'s existing clause loop, two new
`else if self.at_kw(...)` arms slot in before the terminal `break`, each calling
the **existing** `contract_clause()` unchanged and each raising the **existing**
`ContractClauseOrder` / `DuplicateContractClause` the way the `writes` and
`preserves` arms already do — R-3b-1's one-concept rule: no second clause
production, no second element grammar, no second order mechanism, and no new
green node kind
(`ContractClause` is already a keyword-decided extent, which is exactly the
shape `kinds.rs`'s own granularity rules prescribe). Extraction is free:
`extract_reuse` hands a `SIG_PARAM` node's tokens back to `Parser::sig_param`
through `reparse_sig_param`, so the clauses arrive with no separate code path.
The alphabet-membership check reuses the existing `ContractSymbolUnknown` path
that already guards `writes`/`preserves`; `empty-head-clause` is a new
`CompileErrorKind` variant with a row in `docs/tmt/cli.md`'s error table and in
`CompileErrorKind::CODES` (both directions of `tests/error_code_docs.rs`).

**Fixtures** — **[shape-derived: `writes`/`preserves` clause grammar,
`parser.rs::sig_param`; confirm with step 1]**

```
alphabet sym { '_', '^', '$', '0', '1' }

routine walk(tape num: sym writes {} enters { '^', '0', '1', '$' } leaves { '$' }) {
  entry state go {
    ['$'] -> return;
    [*]   -> move [>] goto go;
  }
}

machine {
  tape t: sym;
  entry state s { [*] -> call walk(num = t) then done; }
  state done { [*] -> stop; }
}
```

Near miss for `empty-head-clause` — `writes {}` on the same parameter must stay
accepted, as it is in the fixture above.

**`fmt` obligation.** `.tmc` fmt holds the never-move rule unconditionally and
is idempotent (measured over the 39-position `.pmc` audit and the 61-position
`.tmc` audit). A new header clause is a new comment position. Add positions to
`crates/turing-machine/tests/comment_positions.rs`'s
`header_comments_stay_in_their_headers` group for a comment before `enters`,
inside its braces, and between the two clauses, each in both comment flavours.
**Known residuals stay residuals**: the third recorded residual is any signature
parameter before its comma; if a new position lands on it, record it, do not
"fix" it here.

**TDD steps**

- [ ] **Step 1:** Write the fixture above to the scratchpad and run `tmt compile -S -o <scratch>/out.tma` with the clauses REMOVED (it must compile clean today), then with them present. Record both. **Take this before-picture at Task 1's BASE, not after it:** before Task 1 the failure is ``error: expected `,` or `)`, found `enters` [unexpected-token]`` (planner-verified); after `enters` is reserved the token classification changes and that exact text will not reproduce, so a picture taken post-Task-1 will not match what is recorded here.
- [ ] **Step 2: Tests first** — the `text() == source` pin, the extraction round trip, and all eight fixtures in the table above with their mutations written one line each. Run; all red.
- [ ] **Step 3: Implement** the two AST fields and the two parser arms, reusing `contract_clause()`, the existing `ContractSymbolUnknown` check, and the existing `ContractClauseOrder` / `DuplicateContractClause` raises. Add `EmptyHeadClause` to `CompileErrorKind` and its `CODES` row — **it is the only new code in this task.**
- [ ] **Step 4: Drift guards** — `crates/turing-machine/tests/error_code_docs.rs` against `docs/tmt/cli.md`'s compile-error table (add the row in the same commit or the set-compare is red). `src/compiler.rs::analyze_staged_and_analyze_agree_on_the_0_2_declaration_forms` is the designated home for the new declaration form: extend it, and confirm `analyze_staged` and `analyze` still agree field-for-field on the annotated source.
- [ ] **Step 5: Gate** — `cargo nextest run -p mtc-turing-machine`; compiled-stdlib byte identity at both opt levels (nothing should move — `std.tmc` is untouched in this task).
- [ ] **Step 6: Commit** — `feat(turing-machine): a tape parameter can declare where the head enters and leaves`.

---

### Task 3: The clauses reach the IR, `.param` and both header arms

**PULLABLE: no.**

**Property.** A routine's declared `enters`/`leaves` survive intact from source
to object interface and back out through `tmt interface`, on BOTH arms — the
source arm and the object arm — so a header generated from an object declares
the same clauses as a header generated from the source it was built from; and a
parameter with no clause carries no suffix anywhere.

**Falsifying test.** A round trip in
`crates/turing-machine/tests/interface_emission.rs`: compile the Task 2 fixture,
render the header from the source, render the header from the resulting object,
and assert the two are byte-identical AND both name `enters`/`leaves`. A second
test compiles a clause-free routine and asserts neither suffix appears in either
arm. **Mutation caught:** printing the clause on the source arm only — the
byte-comparison of the two arms goes red, while a name-only assertion would
not. **Do not** add a consumer-object byte-compare as a second guard: 3a's
re-review established that a consumer object is byte-identical whether the
header bound the right alphabet or the wrong one, because a call binds by index.

**Suggested design.** `IrTape` grows `enters: Option<Vec<String>>`,
`leaves: Option<Vec<String>>`, `opaque: bool` **inside v4** — `TM_IR_VERSION`
does not move.

**The `ContractClause` → `Vec<String>` conversion is NOT new, and this task
must not write a second one.** A `ContractClause` holds `Vec<AlphabetElem>`;
`writes` and `preserves` already travel that road to glyph labels today —
`compiler.rs` resolves a tape parameter's clause elements against the
parameter's own alphabet (the same walk that raises `ContractSymbolUnknown`),
and `declared_effective` / `published_writes` are the join point every
consumer reads, with `published_writes`' own doc calling itself "the ONE place
both `ir::lower` and `header::render_source` compute a tape's published write
set". **`enters`/`leaves` reuse that resolution path**; the implementer names
it in the task report. R-3b-1's one-concept rule: if the implementer finds
itself writing a fresh elements→labels loop, that is the defect, not the
design.

**Forward note for the named-set task.** Once a `set` may appear inside a
contract clause, this same conversion must expand it. That expansion is owned
by the set task, which lands later and lists `ir.rs`, `codegen.rs` and
`header.rs` among its files for exactly this reason, and carries a test that a
set inside an `enters` clause reaches `IrTape.enters` already expanded. This
task does nothing about sets and does not need to.

`codegen.rs::emit_params` grows the two suffixes after
`writes=(…)`, in canonical order; its doc comment currently says "the IR carries
no head contract or opacity fact yet" and must be rewritten, not deleted.
`header.rs::tape_param_text` grows the two lists in its signature and **both**
call sites — `sig_param_text` (source arm) and `from_object` (object arm) —
change together, which is what makes the byte-compare above meaningful. The
`opaque` field rides along here as a plumbed-but-always-false value; Task 9
computes it. The object reader already rejects an empty present list as
`Malformed` — **verify this, do not assume it**: if it does not, that check is
part of this task, because the "never empty" invariant is what lets `None` mean
"no promise" everywhere downstream.

**TDD steps**

- [ ] **Step 1:** Verify the object reader's empty-list refusal by constructing a `RoutineInterface` with `enters: Some(vec![])`, encoding, and decoding. Record the result. If it round-trips, add the refusal.
- [ ] **Step 2: Tests** — the two-arm byte-identity round trip, the clause-free negative, and the IR guards. **The IR guards live in `crates/turing-machine/src/ir.rs`'s own `#[cfg(test)]` module, not in a `tests/` binary** (there is none): `json_round_trips_with_a_version` (`:2128`), `the_version_literal_is_four` (`:2141`), and the every-v4-field document test (`:2147+`) — **the last must gain the three new `IrTape` fields**, since it is the guard that a field added without a reader arm fails. While in that file, extend the doc at `:2136-2138` enumerating "every field this arc adds — `param`, the `Label` `dst`, and `map_written`" with `enters`, `leaves` and `opaque`. **This task adds three `IrTape` fields and no `IrTransition` variant**; the trap-kind task adds a fourth v4 surface, `IrTransition::TrapContract`, to the same document test. Two surfaces of one unreleased v4, two tasks, no version bump either way.
- [ ] **Step 3: Implement** — `IrTape` fields, `ir::lower` populating them from the resolved tape, `emit_params`, `tape_param_text` and both call sites.
- [ ] **Step 4: Drift guards** — `docs/formats.md`'s `.param` grammar and the `recognized_directives` guard; the dis→asm byte-identity text-expressibility gate (a `.param` suffix a compiler can emit must be hand-writable — phase 1 already made it so; confirm).
- [ ] **Step 5: Gate** — `cargo nextest run -p mtc-turing-machine`; `cargo nextest run -p mtc-core`; compiled-stdlib byte identity at both opt levels. **The stdlib object must not move in this task** (no `std.tmc` clause exists yet), which is the clean proof that the suffix is emitted only when declared.
- [ ] **Step 6: Commit** — `feat(turing-machine): head contracts travel to the object interface and back`.

---

### Task 4: The static callee checks — `enters-not-accepted`, `leaves-outside-contract`

**PULLABLE: no** (Task 8 reuses its primitive).

**Property.** A declared `enters` set that the routine's entry state does not
accept is a compile error naming the state and a missing glyph; a `leaves` set
contradicted by an exit row whose leaving glyph is statically known is a compile
error; and **no check fires where inference is inexact** — a row with a move, or
a `Subst` write, is left to the runtime check, never guessed at.

**Falsifying test.** Four fixtures: (1) `enters { 'x' }` on a routine whose
entry state has no rule matching `'x'` → `enters-not-accepted` naming the state
and `'x'`; (2) the near miss, the same routine with a `[*]` catch-all in its
entry state → clean; (3) `leaves { '$' }` on a routine with a `return` row that
writes a literal `'0'` and does not move → `leaves-outside-contract`; (4) the
near miss, the same `return` row with a `move [>]` → clean, because the leaving
cell is no longer statically known. **Mutation caught by (4):** treating a
moved row's post-glyph as the written glyph — the near miss starts firing,
which is exactly the over-approximation the spec forbids.

**Suggested design.** The spec suggests: "`enters` must be a subset of what the
entry state accepts on that tape (the cell sets `state-may-trap` already
computes; exact on one tape, product-bounded on several)… `leaves` is checked
statically where inference is exact — exit rows without a move on that tape
(matched cell, or the written glyph)". Taken as suggestion, with two
plan-level decisions the implementer must not re-open:

1. **The accepted-glyph primitive moves, it is not copied.** `rule_sets` is
   private to `lint/rules/state_may_trap.rs` and the static check lives in the
   compiler, which must not depend on `lint`. Move `lint/patterns.rs` to a
   crate-level `src/patterns.rs` carrying `cell_labels` unchanged plus one new
   `pub(crate) fn accepted_glyphs(state, tape_glyphs) -> Option<Vec<HashSet<String>>>`
   (the per-tape union over a state's rules). `state_may_trap::rule_sets`
   becomes a call to it; `dead_rule.rs` follows the moved path. **One
   computation, three callers** — that is the whole point, and a second copy in
   the compiler is a plan violation.
2. **The pass is a third sibling of the contract machinery, not an extension of
   the write fixpoint.** It follows the existing `footprint.rs` (inference) /
   `compiler.rs::check_contracts` (compare-and-raise) split and is invoked from
   the same `resolve_program` call site, after `expand_named_maps`. The write
   fixpoint answers "what may be written"; this answers "where may the head
   be". Do not widen `SymSet`.

The three write-cell tiers `footprint.rs`'s source-level seeding already
distinguishes — `Keep` (exact: the matched cell), `Lit` (exact: the literal),
`Subst` (inexact: the whole alphabet) — are exactly the tiers the `leaves`
check needs on a no-move exit row. Reuse that classification rather than
re-deriving it.

**TDD steps**

- [ ] **Step 1:** Write the four fixtures and run each through `tmt compile -S` at HEAD; all four compile clean today. Record. This pins that the task adds two errors and nothing else.
- [ ] **Step 2: Tests first** — the four fixtures plus a fifth pinning that a routine with NO clause is never checked (absence of a promise).
- [ ] **Step 3: Move** `lint/patterns.rs` → `src/patterns.rs` and add `accepted_glyphs`; rewrite `rule_sets` as a call; update `dead_rule.rs`, `state_may_trap.rs` and `lint/mod.rs`'s `mod` line to the new path. **No visibility widening is needed** — `cell_labels` is already `pub(crate)`, which is exactly what makes a compiler-side call legal from a crate-level module; do not make anything `pub`. Run `cargo nextest run -p mtc-turing-machine` — the move alone must be green before any new check exists.
- [ ] **Step 4: Implement** the two checks. Two new `CompileErrorKind` variants and their `CODES` rows.
- [ ] **Step 5: Drift guards** — `tests/error_code_docs.rs` against `docs/tmt/cli.md`.
- [ ] **Step 6: Gate** — `cargo nextest run -p mtc-turing-machine`; compiled-stdlib byte identity (the stdlib declares no head clause yet, so this must not move).
- [ ] **Step 7: Commit** — `feat(turing-machine): a head contract the body cannot keep is a compile error`.

---

### Task 5: Trap kind `#2` is `contract`, and the continuation-less call moves to it

**PULLABLE: no.** Touches `crates/core` — **core diff 1 of 2.**

**Property.** `trap #2` raises a distinct, named trap kind whose `Display` says
"contract"; `trap #3` and above remain malformed operands; and the
continuation-less-call trap the compiler synthesizes is `#2`, because a callee
declared never to return that nonetheless returned IS a contract violation.

**Falsifying test.** `crates/turing-machine/src/arch/mod.rs`'s existing
`trap_lowers_to_the_two_raise_kinds_and_rejects_other_kinds` becomes a
three-kind test that still pins `Imm(3)` as `BadOperand`. An end-to-end test
links a program whose callee is declared `noreturn` but returns, runs it, and
asserts the outcome is the contract trap — not `UnmappedRead`.
**Mutation caught:** mapping `2` to `UnmappedRead` — the end-to-end test goes
red on the trap kind, while the arch unit test would stay green, which is why
both exist and cover different properties.

**The identifiers, written down** — Task 7 matches on them across a task
boundary, so they are pinned here and quoted there, not left to taste:

```rust
// crates/core/src/vm/trap.rs
pub enum RaisedTrapKind { UnmappedRead, UnmappedWrite, Contract }

pub enum Trap {
    // … the 16 existing variants …
    /// A contract the program declared was broken: a head-position
    /// contract's check found an undeclared glyph under the head, or a
    /// callee declared never to return, returned.
    Contract { at: u32 },
}
```

`Display` for `Contract` names the concept — the word "contract" must appear,
because `tmt run` and the DAP description both forward `Trap`'s `Display`.

**And a third IR terminal, because `trap #2` has no IR representation today.**
`IrTransition` (`ir.rs:317-380`) carries `TrapRead` and `TrapWrite` with **no
kind parameter**, so a third kind is a third variant — inside the unreleased
v4, no version bump:

```rust
// crates/turing-machine/src/ir.rs
pub enum IrTransition {
    // … existing variants …
    TrapRead,
    TrapWrite,
    /// The contract trap: a head-position contract's check failed, or a
    /// callee declared never to return, returned.
    TrapContract,
}
```

This task lands the variant and its three in-crate consequences; the
runtime-check task's synthesized states lower to it, and that task's
`--emit-ir` falsifier reads it by this name. The consequences:

| Site | Change |
|---|---|
| `ir.rs`'s every-v4-field document test | a row for the new variant — the IR/`.param` task extends the same test with three `IrTape` fields; **these are different surfaces of one v4 and both must land** |
| `ir.rs`'s JSON round trip (`json_round_trips_with_a_version`) | covers the variant |
| the mermaid renderer (`ir.rs:580-592`) | a third terminal pseudo-node beside `T_trap0` / `T_trap1` |
| `docs/tmt/optimizer.md:276` | reads verbatim "`trap #0` / `trap #1` for the **two** synthesized trap kinds" — becomes three; the page joins the docs task's table |

**Core-diff justification (R-3b-5).** Three source files plus one test file,
each unavoidable and each arch-agnostic:

| File | Kind | Change | Why core |
|---|---|---|---|
| `crates/core/src/vm/trap.rs` | source | `RaisedTrapKind::Contract` and `Trap::Contract { at }` with its `Display` | `RaisedTrapKind` is core's own vocabulary of kinds an architecture MAY raise; core already names `#0` and `#1` there, so naming `#2` elsewhere would be the neutrality violation |
| `crates/core/src/vm/core.rs` | source | one arm in the exhaustive `MicroOp::Raise` match (`:644-650`) | the match is exhaustive over `RaisedTrapKind`; it does not compile otherwise |
| `crates/core/src/vm/arch.rs` | source | the crate-private fake `test_arch`'s own lowering arm (`:292,295`) | the fake arch proves neutrality by exercising the same vocabulary; leaving it out would make the new kind TM-only in practice |
| `crates/core/tests/execution_trap_docs.rs` | **test, NEW** | a bidirectional set-compare of the `Trap` enum against `docs/core.md`'s table | see below |

`crates/core` learns no TM-1 opcode, no `.tmc` concept and no TM numbering: the
number→kind mapping stays in `arch/mod.rs`, exactly where `#0`/`#1` live today.
Global Constraints budget this as **two source diffs plus one test addition**;
a registry-versus-published-table guard adds no behaviour and is the repo's
standing doctrine.

**The drift guard this task adds, and the one way it can be built.**
`docs/core.md`'s `## Execution` "Trap causes" table (`:328-346`) is
variant-for-variant equal to the `Trap` enum — 16 rows today, 17 after this
task — **and nothing enforces it** (Established fact 6).

**The page side transfers from `error_code_docs.rs`; the registry side does
not, and the naive fix would breach the core budget.** That file set-compares
against `CODES`-style consts (`AsmErrorKind::CODES`,
`linker::DIAGNOSTIC_CODES`); **`Trap` has no such const, and Rust cannot
enumerate enum variants without one.** So there are exactly two constructions,
and only one is admissible:

- ~~A new `Trap::VARIANTS`-shaped const in `crates/core/src/`~~ — **a third core
  SOURCE diff, which the budget forbids. Do not build this.**
- **A test-local exhaustive `match` over `Trap`** inside
  `crates/core/tests/execution_trap_docs.rs`, mapping each variant to its
  published name, set-compared against the page's rows in both directions.
  Adding a variant is then **a compile break of the guard itself**, which is
  the same device `opt_equivalence.rs::trap_kind` already uses and relies on
  — its own doc says so.

Page side: reuse `error_code_docs.rs`'s shape — `section(doc, "## Execution")`
to take lines to the next heading, and its `table_rows` filter
(`l.starts_with("| \`")`), which the trap table's rows satisfy.
*Mutation the guard catches:* adding a `Trap` variant without a row, or leaving
a row for a deleted variant — today both pass silently; after this task the
first does not even compile. Task 16 then states the table **is** guarded.

`docs/tmt/isa.md` needs three edits: the opcode-table row, the "### Explicit
traps" prose (which currently explains at length why `#0` was used for the
synthesized stop — that paragraph is **replaced**, not amended), and the
trap-kind rows near the end.

**The exhaustive match this task breaks, and the byte sweep it owes.**

- `crates/turing-machine/tests/opt_equivalence.rs::trap_kind` (`:84-103`) is an
  exhaustive `match` over `Trap` whose own doc says a new variant "must be
  named here, not folded into a catch-all". **The new variant is a compilation
  break of that test binary**, fixed by one arm: `Trap::Contract { .. } =>
  "contract"`. Named here because a compile break discovered at gate time
  reads like a regression; it is the guard working.
- This task **deliberately changes a codegen byte** — `trap #0` → `trap #2` —
  for every program containing a continuation-less call. The sweep has been
  run by the planner and by the pre-flight scan, and it converges: **exactly
  two assertions move, both in `crates/turing-machine/tests/state_params.rs`.**

  | Assertion | What moves |
  |---|---|
  | `an_exit_bearing_tail_call_prints_a_trap` (`:1371-1381`) | `assert_eq!(after_call, "trap    #0", …)` → `"trap    #2"` |
  | `a_lying_noreturn_header_traps_instead_of_falling_through` (`:1479-1529`) | the outcome asserted at `:1528` is `UnmappedRead` under all three mechanisms → the contract trap |

  **Three prose sites state the old rationale and must change with the byte:**
  `src/ir.rs:339-354` (the `then: None` doc, which says codegen "still emits a
  synthesized `trap #0`"), `src/codegen.rs:112`, and `docs/tmt/isa.md:456`'s
  trap-kind row (`| UnmappedRead / UnmappedWrite | … or an explicit trap #0 /
  trap #1 |`).

  **Everything else is must-not-move, and the reasons are recorded so the
  implementer disposes of each rather than re-deriving them:**
  `tests/tma_dialect.rs:86,203` and `tests/fmt_programs.rs:59` carry `trap #0`
  in **hand-written `.tma`**, not codegen output (`fmt_programs.rs` will
  surface in the grep and is a false positive);
  `tests/mode_equivalence.rs`'s `trap_taxonomy_*`/`in_range_holes_*` and
  `tests/tmc_golden.rs:266` trap on **graft/map holes**;
  `tests/golden_programs.rs:411,444` on **no-transition**;
  `tests/link_matrix.rs:1023-1061` is hand-written `.tma` leaving through
  `retx` and `:1001-1015` asserts a **return-stack underflow**;
  `tests/fmt_adversarial/noreturn_clause.tmc` is a **fmt** fixture and `.tmc`
  fmt is whitespace-only text→text that never reaches codegen.
  **`crates/turing-machine/tests/golden/` holds `.expected.tmt` tape snapshots
  only** — there is no committed `.tma` there, and a trap kind is not in a tape
  snapshot. The compiled stdlib is proved unmoved by the measurement below.

  **Step 2a re-runs the sweep anyway**, because the tree may have moved, and
  records the disposition. For any golden that does move, **derivation-first
  discipline holds absolutely**: re-derive the expected snapshot in code from
  the changed semantics and assert the committed file matches the derivation.
  **Never regenerate a golden from run output** — that would launder this
  task's own bug into the pin.

**Suggested design.** Name the `RaisedTrapKind` member and the `Trap` variant
for the concept, not the number. `codegen.rs`'s `Term::Call{then: None}` arm
changes its bare `"#0"` string to `"#2"` and its comment loses the "neither a
literal fit" rationale, which stops being true. `Term::TrapRead`/`TrapWrite`
and `zero_row`'s fallback keep `#0`/`#1` — the `#0` overload the codegen
comment complains about is what this task removes.

**TDD steps**

- [ ] **Step 1:** Build a `noreturn`-lying program (a callee declared `noreturn` whose body returns) and run it at HEAD; record the outcome text, which today names the unmapped-read trap. **[shape-derived: `docs/tmt/isa.md`'s own description of the synthesized stop, corroborated by `codegen.rs:1019-1040`'s literal `trap #0` on `Term::Call { then: None }`; confirm in this step.]**
- [ ] **Step 2a: The byte sweep, before any edit.** Grep the test tree and the golden corpus for programs carrying a continuation-less call (start from `noreturn`, then from the emitted immediate) and write the expected-to-move / must-not-move classification into the task report. Nothing shipped under `docs/examples` or `src/stdlib` is expected to move.
- [ ] **Step 2b: Tests first** — the arch unit test extended to three kinds with `Imm(3)` still rejected; the end-to-end outcome test; and the new `crates/core/tests/execution_trap_docs.rs` bidirectional guard, which must go red against the un-updated page before the page is updated.
- [ ] **Step 3: Implement** the three core source files, then `arch/mod.rs`'s numeric arm, then `IrTransition::TrapContract` with its v4 document-test row, JSON round trip and mermaid terminal, then `codegen.rs`'s one string, then `opt_equivalence.rs::trap_kind`'s new arm (a compile break, expected), then the two `state_params.rs` assertions, the three prose sites and `docs/core.md`'s table row.
- [ ] **Step 4:** Confirm `engine.rs::next_is_trap` needs no change by reading it — it matches by opcode alone (Established fact 5). Add nothing; record the confirmation in the task report.
**The compiled stdlib does NOT move, and this was measured.** This is the one
task in the phase whose purpose is to change a codegen byte, so the global
"compiled-stdlib byte identity at both opt levels" gate deserves an explicit
answer rather than an assumption. **The compiled standard library contains no
`trap` instruction at all** — neither from the continuation-less-call arm nor
from a graft hole — at `-O0` and at the embedded preset (`-O1
--strip-debugger`). Measured: `tmt compile … std.tmc -S -o <scratch>/std.tma`
and the same with `-O1 --strip-debugger`, then a mnemonic count over each,
**0 and 0**, against a positive control of 194 `ret`/`jm` matches in the same
file. So `stdlib_header.rs`'s pins, both arch-crate presets and
`crates/wasm/tests/stdlib.rs`'s twin image pin all stay still. **If the
implementer's own re-measurement disagrees, STOP and report** — a moved stdlib
pin in this task is a finding, not a regeneration.

- [ ] **Step 5: Gate** — `cargo nextest run --workspace`; **compiled-stdlib byte identity at both opt levels, re-measured in this task** (the paragraph above says it must not move and why); every artifact from step 2a's classification confirmed to have moved or not moved exactly as predicted — **a surprise in either direction is a finding, not a regeneration**; **PM-1 byte identity explicitly**: `cargo test -p mtc-post-machine --test golden_programs` then `cargo test -p mtc-post-machine --test asm_volatile`. PM has no trap instruction, so both must be untouched.
- [ ] **Step 6: Commit** — **two commits, each green on its own.** An earlier three-way split was not: `Trap::Contract` breaks `opt_equivalence.rs::trap_kind`'s compilation until its arm lands, and the guard cannot pass until `docs/core.md`'s row exists.
  1. `feat(core): a third raisable trap kind names a broken contract` — the three core source files, `arch/mod.rs`'s arm, **`opt_equivalence.rs::trap_kind`'s arm**, **`docs/core.md`'s table row**, and the new `crates/core/tests/execution_trap_docs.rs` guard. Everything the new variant breaks, and the guard that pins it, in one green commit.
  2. `fix(turing-machine): a call that cannot return traps as a contract violation` — `IrTransition::TrapContract` and its three consequences, `codegen.rs`'s immediate, the two `state_params.rs` assertions, the three prose sites, `docs/tmt/isa.md`.

---

### Task 6: `--strip-asserts`, `--release`, and `tmt.json` `project` 0.3

**PULLABLE: no.** Deliberately lands BEFORE any assert exists, so the option's
own property is testable without the feature confusing it.

**Property.** `--strip-asserts` is accepted by `compile` and `build`,
independently of `--strip-debugger`; `--release` expands to
`-O1 --strip-debugger --strip-asserts`; the manifest profile key
`strip-asserts` resolves with a per-invocation flag winning over the profile,
`false` in the debug base and `true` in the release base; and today, with no
assert to strip, **every output is byte-identical with the flag and without
it.**

**Falsifying test.** Three, and **the first is not a falsifier — say so rather
than dress it up**:

| Test | Mutation it catches |
|---|---|
| a byte-comparison of `tmt compile -S` output with and without `--strip-asserts` over the golden corpus | **None while no assert exists — equality is the correct answer either way.** It is a *no-harm* check, recorded as such. It becomes a real guard only once check states exist, and the task that adds them owns that version of it |
| the manifest pair: flag-beats-profile, **both directions** | `profile.strip_asserts` winning over the flag — the "flag turns it on against a `false` profile" direction goes red. One direction alone would not catch it |
| **the threading test**: `tmt compile --strip-asserts …` and assert the flag reaches `CompileOptions.strip_asserts`, via the same route the `-v` compile report or a unit test over the argv parser already exposes | **the flag parsed and accepted but never threaded into `CompileOptions`** — the failure the byte-comparison cannot see, because with nothing to strip a disconnected flag and a connected one produce identical output. This is the task's real headline guard |

Plus `project.rs`'s own inventory tests (`every_inventory_is_sorted_and_unique`,
`every_inventoried_key_is_reachable_in_its_parse_loop`,
`the_bundled_schema_matches_the_key_inventories`).

**Suggested design.** Mirror `--strip-debugger`'s **twelve** points exactly
(Established fact 14), adding nothing new, with one deliberate omission and two
deliberate settings:

- **Omitted:** `CodegenOptions` does NOT get the field (R-3b-4: stripping is a
  compiler decision, so the option never reaches codegen).
- **`completions/fish.rs`** is the eighth point a nine-point reading misses.
- **Both embedded stdlib presets are set to `strip_asserts: true` IN THIS
  TASK** — `crates/turing-machine/src/stdlib/mod.rs` and
  `crates/wasm/src/inner/stdlib.rs`.

**Why the presets are set here and not in the stdlib task.** R-3b-3 requires
every embedded stdlib object to be built with asserts stripped, so that when
the stdlib is later annotated with `enters`/`leaves` its compiled object bytes
do not move and only its header pin does. If that flag were first set in the
annotation task, the two changes would land together and the object
byte-comparison there would prove nothing — it could not tell "the presets
strip" from "the annotation happened to be inert". **Setting it here, before
any assert exists, makes it a provable no-op** (there is nothing to strip yet,
so every object is byte-identical), and turns the later comparison into a real
falsifier of exactly one thing. Both preset literals spell every
`CompileOptions` field out with no `..Default::default()` tail, so the compiler
forces the edit at both sites — the task's job is to set the right value, not
to find the sites.

### The stdlib-recompile enumeration — this task's second deliverable

**The two presets are not where the compiled-stdlib gate is executed.** That
gate runs in test helpers that build their own `CompileOptions` with
`..Default::default()`, and `CompileOptions::default()` is hand-written
(`compiler.rs:3030-3045`), where a new `strip_asserts` **must** default to
`false` (the debug profile base is `false`). So without this enumeration the
gate would silently start comparing assert-bearing objects the moment the
stdlib is annotated — the pins the annotation task's falsifier depends on.

**The ruling, per site:** a site that **compares object bytes** against a
shipped preset, against another level, or against a pin **strips asserts, with
the field set explicitly and no `..Default::default()` for it**. A site that is
an **instrument deliberately wanting asserts present** says so in a comment
naming what it is measuring. No site is left to the default by accident.

Enumerated by `grep -rn "stdlib::SOURCE" crates/turing-machine crates/wasm`
(re-run it — the list is a bearing):

| Site | Today | Ruling |
|---|---|---|
| `src/stdlib/mod.rs:69` — the shipped preset | explicit literal, every field | **strip** |
| `crates/wasm/src/inner/stdlib.rs:25` — the shipped debug twin | explicit literal, every field | **strip** |
| `tests/opt_equivalence.rs:1091` `stdlib_object_bytes(level)` | `..Default::default()` | **strip** — its own doc calls it "an apples-to-apples byte comparison (the `-O1` side is exactly what `stdlib::object()` caches)", which is false the moment one side strips and the other does not |
| `tests/opt_equivalence.rs:1289` — Part B's `-O1` build behind the `fired == {"jump-threading"}` floor | `..Default::default()` | **strip** — synthesized check states are new states and rows that `dead-rows` or `dispatch-select` could find slack in, turning the floor red for a reason that has nothing to do with the optimizer |
| `tests/stdlib_golden.rs:74` `stdlib_object(O0)` (the `-O1` arm reuses the stripped `stdlib::object()`) | `..Default::default()` | **strip** — otherwise the 132 derivation-first behavioural runs compare an assert-bearing `-O0` build against a stripped `-O1` one, and a golden seed violating a std tape's `enters` would trap at one level and stop clean at the other |
| `tests/state_params.rs:658` via the `assembly(src, level)` helper (`:55-65`) | `..Default::default()` | **strip** — it compares emitted assembly text |
| `src/footprint.rs:1817` | `CompileOptions::default()` | **strip** — the fixture asserts a footprint over the shipped library, and the shipped library ships stripped |
| `tests/header_roundtrip.rs:512`, `tests/sweep.rs:167`, `tests/stdlib_twins.rs:118`, `tests/interface_emission.rs:66,75,118,128` | mixed | **disposition each in step 3** by the ruling above; several are interface/header instruments where asserts are irrelevant, but none may be left to the default silently |
| `tests/stdlib_header.rs` (`:58,:171,:248`) | writes `SOURCE` to a file and drives the real CLI | **not a `CompileOptions` site** — record it as disposed, since `tmt interface` does not lower asserts |
| `src/lint/rules/dead_map_pair.rs:702`, `src/stdlib/mod.rs:165` | lint / `analyze_staged_with` | **not compile sites** — record as disposed |

**A site the grep finds that this table does not list is a plan defect to
report**, not a judgement call to make quietly.

**No "presets agree" test.** An earlier draft asked for one; it is not
constructible as stated — both literals live inside `OnceLock` closures in
different crates and expose no value, and the two deliberately disagree on
`debug_info` (`false` vs `true`). **The real instrument already exists and this
task's gate already names it**: `crates/wasm/tests/stdlib.rs::debug_stdlib_links_to_the_same_code_as_the_release_stdlib`,
a whole-image byte equality between the twin and the release object. If one
preset strips and the other does not, that pin goes red.

`docs/tmt/project.md`'s prose version marker moves `0.2` → `0.3` — there is no
code literal to change (Established fact 15).

**`cli_docs` obligation, narrower than it looks.** Both `compile --help` and
`build --help` are quoted byte-for-byte in `docs/tmt/cli.md`, and both list
`--strip-debugger`, so both gain a `--strip-asserts` line. But **only
`compile --help` spells the preset out**:

```
  --release          preset: -O1 --strip-debugger
```

`build --help`'s line is `  --debug | --release   presets (manifest mode:
profile selection)` — it names no flags, so the `--release` expansion does not
appear there and that line does not move. Verified live. The man page renders
from the registry plus `usage_text`, so it follows automatically — confirm with
`cargo test -p mtc-turing-machine --test man_page`.

**TDD steps**

- [ ] **Step 1:** Record `tmt compile --help` and `tmt build --help` verbatim at HEAD (**[tool-verified]** log F4 and F10) and `tmt compile … --strip-asserts` → ``tmt: unknown flag `--strip-asserts` `` (log F3).
- [ ] **Step 2: Tests first** — the threading test (the real guard), the manifest flag-beats-profile pair in both directions, the no-harm byte-comparison, and the three `project.rs` inventory tests.
- [ ] **Step 3: Implement** the twelve points, including `completions/fish.rs` and `strip_asserts: true` at both preset sites — **then re-run the `stdlib::SOURCE` grep and dispose of every site in the enumeration table above**, setting the field explicitly where the ruling says strip and adding a comment where a site is deliberately an assert-bearing instrument. Record the disposition of every site, including the ones the table marks "not a compile site".
- [ ] **Step 4: Drift guards** — `cli_docs.rs` (both usage blocks gain a flag line; only `compile`'s `--release` line changes), `completions_registry.rs`, `man_page.rs`, the schema-match test, `docs/tmt/project.md`'s two profile tables.
- [ ] **Step 5: Gate** — `cargo nextest run -p mtc-turing-machine`; `cargo nextest run -p mtc-wasm`; compiled-stdlib byte identity at both opt levels **and the wasm twin's whole-image pin** (`crates/wasm/tests/stdlib.rs::debug_stdlib_links_to_the_same_code_as_the_release_stdlib`) — all must be unmoved, which is what makes this task's preset change a provable no-op.
- [ ] **Step 6: Commit** — `feat(cli): contract asserts can be stripped independently of the debugger`.

---

### Task 7: The runtime check — synthesized states, `trap #2`, and the stripping proof

**PULLABLE: no.**

**Property.** In a debug build, a routine whose tape parameter declares `enters`
traps with `Trap::Contract` when entered with the head on an undeclared glyph,
and one declaring `leaves` traps when it is about to leave with the head on an
undeclared glyph — **after any move**, so the check reads the real cell; the
trap is REPORTED at two fidelities depending on `-g` (see the reporting split
below); and under `--strip-asserts` the same source compiles to
**byte-identical output to the same source with the clauses deleted**.

**Falsifying test.** Two halves, and R-3b-4 hands the second one its shape:

1. A seeded-tape program per clause, run under **all three call mechanisms**
   (`mono`, `frames`, `hybrid`) at `-O0` and `-O1`: traps with `Trap::Contract`
   under debug, runs to `Stopped` with the expected final tape under
   `--strip-asserts`. The near miss is the same program seeded so the head IS
   on a declared glyph: clean under both. Plus the two reporting tests — the
   `-g` one naming routine/tape/clause/glyph, and the **no-`-g` degradation
   test** pinning the reduced message.
2. **The stripping identity, in two layers — and the discriminating one is the
   IR, not the object.**
   - *The user-visible property:* compile source A (with clauses) under
     `--strip-asserts` and source B (A with the clause text deleted) plainly,
     and byte-compare the objects at `-O0` and at `-O1`. **Compile both
     WITHOUT `-g`**: A and B differ in source text, so a debug line table would
     differ for reasons that have nothing to do with the property.
   - *The guard that can go red:* `--emit-ir` on A under `--strip-asserts` must
     contain **no check state at all** — and no `TrapContract` transition, the
     IR variant the trap-kind task added and the one these states lower their
     catch-all to. Reading the IR by that variant name is what makes the
     assertion specific rather than a state-name grep.
     **Mutation caught:** synthesizing the states unconditionally and filtering
     them out at codegen. A clean filter would leave the object comparison
     GREEN — which is exactly 3a's failure mode of a test that cannot fail —
     whereas the IR still carries the states and the assertion goes red. The
     object comparison alone is not a guard for R-3b-4; the IR assertion is
     what makes "compiler option, not codegen filter" a fact rather than an
     intention.

**Suggested design** (the spec's, quoted as suggestion): "the compiler
synthesizes one check state per tape at the routine's entry
(`<routine>__enters_<tape>`: rows for the declared set → the real entry;
catch-all → `trap #2`) and one before every way out (`… return` /
`goto <state param>` rewritten to `goto <routine>__leaves_<tape>`, which reads
and returns or traps), so the check reads the real cell after any move. Chained
per tape, `|set| + 1` rows each, no product."

Three plan-level notes the implementer must carry:

- **Synthesis is strictly per-clause opt-in.** A tape without a clause gets no
  state; a routine without any clause is untouched. This is what keeps every
  existing byte pin still.
- **There is no reserved state-name namespace in this crate.**
  `ir.rs::fresh_state_name` freshens by `HashSet` plus a `_1`/`_2` suffix, so a
  user state colliding with a synthesized name is silently renamed, not
  refused. Use `fresh_state_name`; do not invent a sentinel prefix and do not
  add a collision refusal — that would be a second freshening mechanism beside
  the two that already exist (`ir.rs` and `header.rs::fresh_param_name`).
- **The reporting property splits along `-g`, and conflating the two axes is
  the easy mistake.** CLAUDE.md already distinguishes them for the debugger:
  *source provenance* decides whether a frame is openable, `-g` decides only
  whether the line table is populated. The same split governs here, and
  Established fact 24 is the reason: `MapFunction.labels` and `.lines` are
  **"empty without `-g` objects"** (`crates/core/src/linker/mod.rs:461-464`)
  and `cli/run.rs`'s resolver reads `f.labels`. A synthesized check state's
  label — which is where the tape and the clause are encoded — simply does not
  exist in a non-`-g` link. So:

  | Build | `tmt run` must report |
  |---|---|
  | asserts present, **no `-g`** | the trap KIND (the word "contract") and the faulting ADDRESS. The routine's own name is additionally available from `MapFunction.name`/`start`/`end`, which are populated either way — include it if it resolves, but the property does not require it. |
  | asserts present, **`-g`** | the routine, the tape, the clause (`enters` or `leaves`) and the offending glyph, with the trap mapped to the **signature line** |

  **Both arms need a test**, and the no-`-g` one is a degradation test pinning
  the reduced message — without it, a reader would believe the richer message
  is always available, which is the false claim this split exists to remove.
  *Mutation the degradation test catches:* resolving the clause through
  `labels` unconditionally, which yields an empty or panicking render on a
  non-`-g` object.

- **DAP.** The stop reason `contract` depends on the trap kind, not on labels,
  so it holds in both builds; the *description* is `Trap`'s `Display` and the
  *source location* degrades exactly like `tmt run`'s. Today TM's DAP forwards
  the literal `"exception"` with `Trap`'s `Display` (`dap/mod.rs`,
  `nonstep_outcome` and `tick`); the new reason is a small, explicit divergence
  at those two sites, and `crates/core/src/dap/` stays untouched.
  `cli/run.rs` currently prints `outcome: {outcome:?}`, so the contract case
  needs a rendering of its own there, in the CLI layer where every byte of
  output belongs (thin-renderer rule). It matches on
  `Trap::Contract { at }` — the identifier the trap-kind task pins.

- **Synthesized names are visible, and that is fine.** A check state becomes a
  `.tma` label and a `MapFunction.labels` entry in a `-g` build, so it appears
  in `tmt dis`, `--trace` and the DAP stack. No shipped artifact moves (no
  shipped source declares a clause until the stdlib task, whose presets strip),
  but the task report says so and the docs task records it, rather than leaving
  a reader to discover a state nobody wrote.

**Optimizer obligation.** `opt_equivalence.rs`'s Part B asserts that over the
compiled stdlib the only pass that ever fires is `jump-threading` and that the
`-O1` object strictly shrinks. The stdlib is built with asserts stripped
(R-3b-3), so this floor must not move — but a program WITH asserts may expose
new slack. Add the contract-assert program to the everything-matrix roster
(which asserts its own length), at both opt levels and all three mechanisms,
rather than relaxing Part B.

**TDD steps**

- [ ] **Step 1:** Build the seeded program pair in the scratchpad and confirm at HEAD that the debug and stripped runs are identical today (no assert exists). Record the exact `tmt run` output and the final tape snapshots — they are the derivation the test asserts against, derivation-first, never regenerated from output.
- [ ] **Step 2: Tests first** — the six-cell matrix (2 opt × 3 mechanisms) per clause, the seeded near miss, and the stripping byte-identity. All red.
- [ ] **Step 3: Implement** the synthesis in a new `contracts/head.rs`, gated on `CompileOptions.strip_asserts` being false AND the clause being present. The catch-all row lowers to `IrTransition::TrapContract` — **the variant already exists** (the trap-kind task added it with its v4 document-test row, JSON round trip and mermaid terminal); this task consumes it and adds no IR variant of its own.
- [ ] **Step 4:** `tmt run`'s contract rendering at both fidelities, `-g`'s signature-line mapping, the DAP stop reason. One test each; the DAP one goes in `tests/dap_programs.rs`.
- [ ] **Step 5: Drift guards** — **the everything-matrix roster**: `tests/opt_equivalence.rs::everything_matrix_is_green` asserts `roster.len()` is `15` at `:1240`, **and the assert carries an explanatory message enumerating the roster's composition** (`:1242-1246`: "6 Appendix A + nested graft + 8 pass/barrier fixtures (jump-threading+dce, dispatch-select, dead-rows, inline ×2, the brk barrier, the exit-bearing call and the facade that forwards its exits)"). **The message is the part that goes stale, and no compiler catches it** — update the literal AND the enumeration. This task takes the literal to 16 and the open-binding task to 17; whichever lands second reads the current value rather than assuming. `docs/dap.md`'s stop-reason list and the synthesized-name visibility note go into `task-16-carries.md`.
- [ ] **Step 6: Gate** — `cargo nextest run -p mtc-turing-machine`; `cargo nextest run -p mtc-core`; compiled-stdlib byte identity at both opt levels; `-O0` bit identity on the golden corpus.
- [ ] **Step 7: Commit** — `feat(turing-machine): debug builds check a head contract at run time`.

---

### Task 8: `enters-unmet` — PULLABLE

**PULLABLE.** A warning-tier lint with no quickfix; deferring it changes no
other task. It depends on Tasks 2 and 4 and nothing depends on it.

**Property.** At a call or bind site whose callee declares `enters`, if the
caller's own analysis says the head **may** be on a glyph outside that set, the
lint reports it naming both sets and the offending glyphs — and it never fires
against a callee that declares no `enters`, nor against a site the analysis
proves safe.

**Falsifying test.** The commonest mistake, as the spec names it:
`[*] -> call plusOne(…)` in a machine's entry state, where the head is
unconstrained and the callee declares `enters` — fires. The near miss is the
same program whose entry state constrains the cell to the callee's declared set
— silent. **Mutation caught:** seeding the dataflow with the whole alphabet
unconditionally instead of the world's own `enters` — the near miss starts
firing. A second near miss: the same call against a callee with NO `enters` —
silent, which catches the mutation "treat a missing clause as the empty set".

**Suggested design** (the spec's, quoted as suggestion): "A forward dataflow
per world and tape over 'where the head may be': seeded by the world's own
`enters` (whole alphabet without one); after `then s` the callee's declared
`leaves` projected through the map's read preimage (whole alphabet without
one); after `goto s` the row's post-glyph (matched cell without a move, the
written glyph after a write, whole alphabet after a move); union to a fixpoint.
At every `call`/`bind` whose callee declares `enters`, the caller's set at that
row, intersected with the pattern cell and projected forward through the map,
must be a subset of the callee's `enters`."

Two plan-level notes:

- **There is no fixpoint precedent in the lint layer** (Established fact 13).
  The closest reusable SHAPE is `footprint.rs` — monotone per-tape set, uncapped
  loop bounded by cardinality, project-through-binding at call and graft edges —
  and the spec says the new analysis "lives in the compiler beside the footprint
  fixpoint (same shape, no write-back projection)". Put it there, not in `lint/`,
  and have the rule read its result. **Reuse `footprint.rs::project_write_back`'s
  counterpart direction rather than writing a second binding projection**: if the
  forward projection cannot be expressed against the existing pair-list walk,
  say so in the task report instead of quietly adding a parallel one.
- **The per-state accepted-glyph query is the one the static check already
  moved.** Where this analysis needs "what does state `s` accept on tape `k`"
  — seeding, and the `goto` post-glyph of a matched cell — it calls
  `src/patterns.rs::accepted_glyphs`, the primitive the static-check task
  moves out of `lint/rules/state_may_trap.rs`. Its signature there is
  `(state, tape_glyphs: &[&[String]])`, matching today's private `rule_sets`.
  Do not build a second one; if the dataflow needs a shape `accepted_glyphs`
  cannot give, say so in the report rather than forking it.

- **The message says "may be on".** Over-approximation is stated in the text,
  because the assert is the authority. That is why this is warn tier and has
  **no quickfix** — not because fixes are unprecedented here (they are not:
  eleven `.tmc` rules emit one, Established fact 19), but because the spec
  says so and because there is no correct edit for "this call site may be
  wrong": the remedy could be a wider `enters`, a narrower call site, or a
  guard row, and the rule cannot tell which.

**Registration.** `RULES` (default-on), not `OPT_IN_RULES`: it checks an
explicit author-written clause, like `contract-clause-overlap`, rather than
auditing unannotated totality like `state-may-trap`. A `### enters-unmet`
heading joins `docs/tmt/lint.md`'s `## The \`.tmc\` rules` section or
`lint::docs_drift`'s set-compare is red in both directions.

**TDD steps**

- [ ] **Step 1:** Build the firing fixture and its two near misses and run `tmt lint` on all three at HEAD; all silent. Record.
- [ ] **Step 2: Tests first** — firing, near miss ×2, and a corpus sweep test asserting ZERO findings over `tests/golden`, `docs/examples` and `std.tmc`.

  **The stdlib arm of that sweep is a JOINT gate with the stdlib-annotation
  task, not an independent one.** `std.tmc` declares no clause when this task
  lands, so the sweep is trivially green here and only becomes meaningful
  later. It can then only stay green if each stdlib clause is at least as wide
  as what the routine's own call sites may hand it — which is why the
  annotation task derives each clause from the **body**, not from the `?`
  prose, wherever the two disagree. The evidence that they do disagree is
  already visible: `goToEnd`'s doc says "ENTRY: head on the number — any of
  `'^','0','1','$'`" and then, two lines later, "(The body also accepts `'_'`
  without trapping: it just walks on.)". A clause transcribed from the first
  sentence would be narrower than the body and would make this sweep — and the
  runtime `enters` assert — fail on a perfectly correct program. **If this
  sweep goes red after the annotation lands, the defect is the clause, not the
  lint.**
- [ ] **Step 3: Implement** the analysis beside `footprint.rs`, then the rule.
- [ ] **Step 4: Drift guards** — `lint::docs_drift` (the `### ` heading), `lint/mod.rs::known_code`.
- [ ] **Step 5: Gate** — `cargo nextest run -p mtc-turing-machine`; measure the per-pass cost on the widest source (`docs/examples/rpnwide/rpnwide.tmc`) and record it — `expand` already dominates a keystroke there, and a lint that runs a second fixpoint per keystroke is an editor regression.
- [ ] **Step 6: Commit** — `feat(turing-machine): a call that may break a head contract is a lint`.

---

### Task 9: Open bindings in `.tmc`, and the `opaque` inference

**PULLABLE: no.**

**Property.** `with map { …, * }` (and `with map { * }`) at a call, bind or
graft site sets the phase-1 `open` bit on that tape binding and nothing else;
the compiler infers per tape whether every state that reads it reads it as `*`,
exports that as `opaque`, and the linker's existing refusal then accepts
exactly the open bindings whose callee is genuinely opaque and refuses the
rest.

**Falsifying test.** A `.tmc` program that opens a binding into a genuinely
opaque callee, linked and run under all three mechanisms with seeded tapes,
agreeing with a hand-written `.tma` twin — the shape
`tests/link_matrix.rs::an_open_binding_program_agrees_across_mechanisms`
already has in `.tma` only, now reachable from `.tmc`.

**The near miss is a state that READS the tape and has NO `*` cell on it** —
not merely "a state that discriminates glyphs". Discriminating is not the
opposite of opaque: the shipped `OPEN` fixture's callee
(`tests/link_matrix.rs:285-315`) dispatches `[0] [1] [2] [*]` on its tape and
is correctly declared `opaque`, because the `*` row means no glyph is ever
rejected for being unlisted. A near miss written as "one state discriminates"
would therefore LINK, and the test would pass while proving nothing. The
fixture is a callee with a state whose rows name only concrete glyphs — no
`[*]` — which must be refused as `open-binding-unsupported` naming that state.
**Mutation caught:** inferring `opaque` as "no state reads the tape at all", or
as "no state discriminates", instead of "every state that reads it has a `*`
cell there" — under either, the near miss links.

**Suggested design.** Grammar first: `*` is a marker on the map, not a pair.
`SymMap` grows an `open: bool` (with the `*`'s own span for diagnostics), and a
lookahead for a bare `*` runs **before** `map_pair` is called, leaving
`sym_lit` untouched (Established fact 3).

**The lookahead belongs to the call/bind/graft SITE map only — NOT to
`map_pairs_body` as a whole.** `parser.rs:2650`'s own doc says that production
is "shared by `Self::sym_map`'s inline form and `Self::parse_map_decl`'s
declaration body", and `parse_map_decl` (`:2671-2680`) **discards its pairs
entirely**. A lookahead added at the shared level would make
`map m: ab -> cd { 'a' -> 'c', * }` compile, with the marker silently swallowed
and no open binding anywhere — a legal-looking spelling that means nothing.
Put it in `sym_map`'s inline arm, or thread an `allow_open: bool` through
`map_pairs_body` with `parse_map_decl` passing `false`. **A top-level `map`
declaration keeps refusing `*`, and a test pins that refusal** — today it is
``error: expected a glyph or number, found `*` [unexpected-token]`` at the
marker (**[tool-verified]**, log F11). *Mutation that test catches:* exactly
the shared-level lookahead described above.

**Position rule — the plan's decision: `*` is legal only as the LAST entry**,
with or without a trailing comma, because a marker that may appear anywhere has
no reading that is not arbitrary and the `.tma` disassembler already prints it
last (`{3->'0',*}`). A `*` elsewhere is a parse error naming the rule.
`* as v` stays the existing `wildcard-binding` error and must be confirmed
unaffected — this is the guarantee that no substitution can copy the opaque
index.

The inference is per tape over the world's states: a tape is opaque iff every
state that reads it has a `*` cell in that position. `ResolvedWorld.tapes`
already normalizes machine tape declarations and signature tape parameters into
one list, which is the single surface to walk — do not add a second.

**Everything-matrix obligation.** The spec asks for an open-binding program in
the matrix; add the `.tmc` one to `everything_matrix_is_green`'s roster rather
than replacing the `.tma` fixture, which pins the hand-authored form. **The
roster's length is asserted at `tests/opt_equivalence.rs:1240` (`15` today),
and the assert's message at `:1242-1246` enumerates the roster's composition
in prose — update both.** The message is the part no compiler checks. The
runtime-check task also adds a program, so if that lands first this one takes
the literal from 16 to 17; if this lands first, from 15 to 16. Read the current
literal, do not assume it.
`crates/core/tests/link_open.rs` and `compose.rs`'s own tests stay green —
note that `compose.rs`'s `tape()` fixture hardcodes `open: false`, so core's
open path is covered by `link_open.rs`, not by the compose unit tests.

**TDD steps**

- [ ] **Step 1:** Re-run the two refusals — the site-map `*` (log F2) and the named-map-declaration `*` (log F11) — to confirm both texts at HEAD, and read `tests/link_matrix.rs`'s `OPEN` `.tma` fixture (`:285-315`), noting that its `walk` state dispatches `[0][1][2][*]` and is still `opaque`. The `.tmc` program must produce the same wire shape. **[tool-verified]** log F2, F11; **[shape-derived: the `OPEN` fixture]** for the program.
- [ ] **Step 2: Tests first** — the three-mechanism run, the **no-`*`-cell** non-opaque refusal near miss, a `*`-not-last parse error, the named-map-declaration `*` refusal, the `* as v` confirmation, and the `opaque` bit's appearance in both header arms.
- [ ] **Step 3: Implement** the grammar (parser + green tree + the `SYM_MAP` node's new child token — **no new node kind**, per `kinds.rs`'s own granularity rule: a new shape inside an existing bracketed reparse unit needs none), then the inference, then the `IrTape.opaque` population Task 3 plumbed.
- [ ] **Step 4: fmt** — a `*` inside a `with map` is a new list position; add entries to `comment_positions.rs`'s `list_interior_comments_stay_in_their_entries` group. **The `with map` surface already breaks to multi-line on any boundary comment** — a recorded rendering gap, not data loss; do not "fix" it here.
- [ ] **Step 5: Gate** — `cargo nextest run -p mtc-turing-machine`; `cargo nextest run -p mtc-core`; **`crates/core/tests/link_open.rs` named explicitly** — it is the core-side open-binding guard and this is the task that first drives that path from `.tmc`, so a green run there is the evidence that the `.tmc` form lands on the same wire shape the hand-written `.tma` does; plus the everything-matrix and `link_matrix.rs`.
- [ ] **Step 6: Commit** — `feat(turing-machine): a symbol map can be left open`.

---

### Task 10a: Named glyph sets — the declaration, alphabet bodies and clauses

**PULLABLE: no** (10b, 11 and 12 depend on it).

> **Why this task is split in two.** The pre-flight scan disproved the
> single-funnel design this task was first written on. `alphabet_elem()` has
> exactly two callers — `alphabet_elems` (`parser.rs:1514`) and
> `contract_clause` (`parser.rs:1727`). **`pattern_cell` does not use it**: it
> matches the token itself (`parser.rs:2097-2122`), calls `sym_or_range()`
> directly, builds `PatternCellKind` — a *different enum* (`parser.rs:391-395`)
> — and its fallback arm refuses an identifier outright (Established fact 21).
> So a `set` needs **two** grammar surfaces, and the second drags in every
> exhaustive match over `PatternCellKind` (Established fact 22). 10a is the
> declaration and the `AlphabetElem` surface; 10b is the pattern-cell surface.
> Each is independently green and independently reviewable.

**Property.** A `set NAME { … }` declaration names a glyph set built from
literals, ranges and other sets; it is namespaced, `export`able and
`use`-importable; it is carried in a `.tmh` and printed by `tmt interface`; it
expands in place in an **alphabet body** and in **any contract clause**; it is
never a tape type; and a cycle among sets is a compile error rather than a
hang.

**Falsifying test.**

| Fixture | Expected | Mutation it catches |
|---|---|---|
| `set digits { '0'..'9' }` used in an alphabet body and in an `enters` clause, compiled and run against a derivation | clean, and the alphabet's glyph list is the expansion in order | expanding in the body but not in the clause (or vice versa) — one fixture covering both sites is enough here because they share `alphabet_elem` |
| `set a { b }` / `set b { a }` | a cycle error | memoising expansion without a visiting-set check — the fixture hangs or stack-overflows instead of erroring. This crate already has one pre-existing deep-graft stack overflow; do not add a second |
| a set named as a tape type (`tape t: digits`) | refused | treating a set as an alphabet because both resolve to glyph lists |
| a set inside an `enters` clause reaching `IrTape.enters` **already expanded** | the object's `.param` carries the member glyphs, not the set name | the clause→labels conversion passing a set name through as a literal |
| `text() == source` over a source with a `set` declaration, and `extract_program`'s round trip | hold | a green-tree shaping walk that drops the declaration's trivia |
| a `.tmh` carrying an exported set, read back by the strict reader | accepted | printing a set the reader cannot parse |
| **a `writes` clause naming a set whose members overlap a `preserves` clause** | `contract-clause-overlap` **still fires** | **`elem_indices` returning `None` for a `SetRef`** — the arm that compiles, passes everything else, and silently switches the rule off for set-bearing clauses |
| a set's members appear in the numeric-glyph notation roster | they do | `lsp/roster.rs`'s `_ => {}` arm swallowing `SetRef` |

**Suggested design.** `AlphabetElem` gains a `SetRef` alternative. That is
**two grammar productions** — `alphabet_elems` and `contract_clause` — but
**seven consumers**, and one of them fails silently rather than loudly. The
same enumerate-then-confirm-by-grep treatment the pattern-cell task gets
applies here, for the same reason:

| Site | Shape | A missing or wrong arm |
|---|---|---|
| `compiler.rs:966-974` (alphabet bodies) | exhaustive | compile break — loud |
| `compiler.rs:2846-2852` (the clause road, inside `resolve_contract_clause`) | exhaustive | compile break — loud |
| `lint/rules/contract_clause_overlap.rs:68-69` `elem_span` | exhaustive | compile break — loud |
| **`lint/rules/contract_clause_overlap.rs:78-84` `elem_indices`** | exhaustive, **returns `Option`**, and its caller is `let Some(indices) = … else { continue; }` (`:203-206`) | **`SetRef => None` COMPILES, passes every other test this task names, and silently stops reporting `contract-clause-overlap` for any clause holding a set.** This is precisely the clean-filter-leaves-it-green failure mode the runtime-check task congratulates itself on avoiding, and it is the one arm that needs a fixture of its own |
| `fmt/print.rs:1018-1022` `alphabet_elem_text` | exhaustive | compile break — loud |
| `lsp/roster.rs:296-312` | has a `_ => {}` arm | **silent** — a set's members drop out of the numeric-glyph notation roster. No compile break; disposition it deliberately |
| `tests/tmc_property.rs:2018-2033` `stamp_elems` | exhaustive | compile break of that test binary |

`lint/rules/contract_clause_overlap.rs`, `lsp/roster.rs` and
`tests/tmc_property.rs` join this task's file list for this reason.

Expansion happens where each already
expands: `compiler.rs::resolve_alphabet_glyphs`/`expand_range` for alphabet
bodies, and the `writes`/`preserves` clause-resolution path (the one the
IR/header task named as the ONE elements→labels road) for clauses. **The naming
trap the map records:** `compiler.rs::expand_range` (alphabet bodies) and
`expand.rs::enumerate_range` (pattern cells) are two separate
range-enumeration implementations with confusingly similar names; a set must
expand identically through both, and 10b's shared-fixture test is what proves
it.

Surfaces this task owns:

- A `SET_DECL` green node kind **appended after the last discriminant**, a
  `kind_name` arm, **and** the widened range in
  `kind_name_never_falls_through_for_an_occupied_discriminant` — all three in
  one edit, because the test silently under-covers an appended kind otherwise
  (Established fact 18). While in that file, fix the stale module doc at
  `syntax/kinds.rs:148-151`, which still says the guard "walks `0..=30` and
  `32..=53`" while the code walks `32..=55`.
- `set` joins `top_item`'s dispatch, the `export` sub-match,
  `next_is_top_doc_accepting`, and `skip_to_sync`'s top-level recovery word set
  — miss the last and a broken `set` desyncs the resilient parser.
- `Program.sets`, a `TopView::SetDecl` arm in `extract_items`, and **a
  `SetDeclView` in `syntax/views.rs`** — the typed view `decl_span::<SetDeclView>`
  needs, which the `unused-set` task's quickfix is built on.
- `header.rs` prints exported set declarations; `fmt/print.rs` prints the
  declaration. `std.tmh`'s shape is unaffected (no `std.tmc` set exists).
- **Two resolution surfaces, both needed by `unused-set`**, mirroring exactly
  what `unused_alphabet.rs` reads for alphabets (`ctx.resolved.alphabets`, a
  map of mangled name → declaration carrying `name_span`):
  - `Resolved.set_refs: HashSet<String>` — **mangled** names, written at the
    ONE place each expansion consumes a set name (here for both 10a sites, in
    10b for the pattern-cell site). This is what makes `unused-set` a single
    uniform walk instead of three hand-rolled ones (Established fact 11).
  - `Resolved.sets` — **mangled name → the set declaration with its
    `name_span`**, the surface the lint iterates and the surface
    `decl_span::<SetDeclView>(ctx, set.name_span)` anchors its quickfix on.
    **Without it the lint would have to match unmangled `Program.sets` names
    against mangled `set_refs`** — exactly the trap that rules out the
    hand-rolled walks in the first place.

**TDD steps**

- [ ] **Step 1: The before-picture, corrected.** A top-level `set` today fails ``error: expected a top-level declaration, found `set` [unexpected-token]`` at 1:1 — a top-level dispatch refusal, **not** an undefined-name error (**[tool-verified]**, log F12). Re-run and record.
- [ ] **Step 1b: Re-grep the consumer fan-out.** `grep -rn "AlphabetElem::" crates/` and compare against the seven-row table above. **A site the table does not list is a plan defect to report**, not a judgement call. Pay particular attention to any arm returning `Option` or carrying a `_` fallback — those are the ones a compile break will not catch for you.
- [ ] **Step 2: Tests first** — every row of the table above. All red.
- [ ] **Step 3: Implement** the declaration grammar, the node kind and its guard range, the recovery word, extraction, `SetDeclView`, `AlphabetElem::SetRef` **and an arm at each of the seven consumer sites**, expansion at the two grammar sites, the cycle check, `Resolved.set_refs` **and `Resolved.sets`**, and the header/fmt printing.
- [ ] **Step 4: fmt and corpus** — add a `set` declaration to a `tests/golden` fixture so `tmc_green_analyze.rs`'s corpus sweep and `syntax_green.rs` pick it up automatically (this is 3a's M6 carry: a new top-level form absent from the corpus is pinned only indirectly); add `comment_positions.rs` entries for a comment in a `set` header and inside its body.
- [ ] **Step 5: Drift guards** — `editor_grammar.rs` (the keyword landed in the grammar-bump task; confirm no second edit is needed), `error_code_docs.rs` for the cycle code.
- [ ] **Step 6: Gate** — `cargo nextest run -p mtc-turing-machine`; compiled-stdlib byte identity at both opt levels.
- [ ] **Step 7: Commit** — `feat(turing-machine): glyph sets can be named, exported and imported`.

---

### Task 10b: Named sets in a pattern cell

**PULLABLE: no.** Task 11's `set-outside-alphabet` needs it, **and so does
Task 12** — that task's third falsifying fixture is "a set referenced from a
pattern cell only", which cannot parse without this task.

**Property.** A pattern cell may name a set, with or without an `as` binding —
`[digits]` and `[digits as d]` — and the cell expands to exactly the same rows
the equivalent literal alternation would produce; a set is still refused in a
write cell; and the expansion a pattern cell performs agrees glyph-for-glyph
with the expansion an alphabet body performs for the same set.

**Falsifying test.**

| Fixture | Expected | Mutation it catches |
|---|---|---|
| `[digits] -> stop;` on a tape whose alphabet holds every member | compiles; the rows equal those of the literal alternation, derivation-first | expanding a set cell to one row instead of `\|set\|` rows |
| `[digits as d] -> write [{d+1}] …` | compiles and folds per row | dropping the binding on the set arm — the `as` form is the one the task's first draft never mentioned |
| a set in a **write** cell | refused | letting `PatternCellKind::SetRef` leak into `write_cell`'s grammar |
| **the agreement fixture:** one `set`, expanded once through an alphabet body and once through a pattern cell, compared | identical glyph order | `compiler.rs::expand_range` and `expand.rs::enumerate_range` diverging — two implementations, one meaning |

**Suggested design.** `PatternCellKind` gains a `SetRef` alternative and
`pattern_cell`'s match gains an identifier arm (today that token hits the
fallback and is refused). **Every exhaustive match over `PatternCellKind` is a
touched site** — this is the task's real cost and it is enumerated rather than
discovered at step 3 (Established fact 22):

| Site | What the new arm does |
|---|---|
| `parser.rs:2040-2042` `check_char_arithmetic` | decide foldability for a set-bound name — coordinate with the `char-arithmetic` fix in the strictness task |
| `parser.rs:2102-2113` construction | build the variant |
| `expand.rs:591-609` `cell_options` | expand to one option per member — the arm parallel to `Range` |
| `lint/patterns.rs:52-54` (`src/patterns.rs` after the move) `cell_labels` | return the member labels |
| `lint/rules/binding_product_threshold.rs:21-23` | count the members toward the product |
| `fmt/print.rs:1050-1052` | print the set name back verbatim |
| `header.rs:1733-1737` | print the set name in a graph body |
| **`tests/tmc_property.rs:2123-2136` `stamp_rule`** | a compile break of that test binary, plus a generator/coverage stamp the new variant should carry |

Three `matches!`-on-`Wildcard` sites need **no** arm (`compiler.rs:1819`,
`parser.rs:2132`, `lint/rules/unused_tape.rs:65`) — note that
`parser.rs:2132` is the `wildcard-binding` refusal, and a `SetRef` arm must
leave it undisturbed, which is what keeps `[SET as d]` legal while `[* as v]`
stays refused. `syntax/extract.rs` and `codegen.rs` do not match on this enum
at all — codegen works on the already-expanded `Cell`. Confirm by grep before
editing; if the grep finds a site this table does not list, that is a plan
defect to report.

**TDD steps**

- [ ] **Step 1:** Confirm the before-picture: `[digits]` today fails ``error: expected a pattern element (glyph, number, range, or `*`), found `digits` [unexpected-token]`` (**[tool-verified]**, log F13). Re-run the `PatternCellKind::` grep and compare against the table above.
- [ ] **Step 2: Tests first** — the four rows above, plus `comment_positions.rs` entries for a comment beside a set cell.
- [ ] **Step 3: Implement** the variant, the identifier arm, and each site in the table.
- [ ] **Step 4: Gate** — `cargo nextest run -p mtc-turing-machine`; compiled-stdlib byte identity at both opt levels; `-O0` bit identity on the golden corpus (no shipped program names a set, so nothing may move).
- [ ] **Step 5: Commit** — `feat(turing-machine): a pattern cell can name a glyph set`.

---

### Task 11: Strictness — `set-outside-alphabet`, `range-outside-alphabet`, and the `char-arithmetic` fix

**PULLABLE: no.** This is the task with the largest behaviour change; read
"Behaviour changes in phase 3b" before starting it.

**Property.** A set member, a range endpoint's expansion, or a single glyph
outside the alphabet it is written against is a **compile error** naming the
glyph and the alphabet, **in every position where one can be written** — while
a grafted generic rule that maps to no host symbol at its splice remains a
**warning**, because those are two different facts at two different stages.
Separately, whether a bound symbol may be folded arithmetically is decided by
the symbol's **label being numeric**, not by whether the literal was written
with quotes.

**One code per position class, and contract clauses are already strict.** This
task's two new codes are for the positions that are lax today:

| Position | Code | Status |
|---|---|---|
| a pattern cell | `set-outside-alphabet` / `range-outside-alphabet` | **new here** — today the alternative is silently dropped |
| an alphabet body | `set-outside-alphabet` / `range-outside-alphabet` | **new here**, same expansion machinery |
| a **contract clause** | `contract-symbol-unknown` | **already strict — do not add a second code.** `compiler.rs::resolve_contract_clause`'s `take()` closure (`:2830-2852`) raises `ContractSymbolUnknown { glyph, clause, alphabet }` for any label, single or range-expanded, that is not in the parameter's alphabet |

The property is therefore satisfied at three positions by two mechanisms, and
the clause mechanism is pre-existing. **A second code for the clause position
would be the defect**, not the fix: one position class, one code. When the set
tasks make a `SetRef` legal in a clause, its members flow through the same
`take()` and inherit `contract-symbol-unknown` for free — the clause fixture
below is what proves that rather than assuming it.

**Falsifying test.** Three pairs:

1. `['0'..'9']` on a three-glyph alphabet → `range-outside-alphabet`; the near
   miss, the same range on an alphabet that has all ten → clean.
2. A set whose member is off the tape → `set-outside-alphabet`; the near miss,
   the same set on a tape that has every member → clean.
3. `docs/superpowers/probes/2026-09-13-tmc-completeness/probe3-sets/dec-glyph.tmc`
   compiles and runs equivalently to `dec.tmc` — the `char-arithmetic` refusal
   is gone for a numeric-labelled glyph; the near miss is a fold over a
   genuinely non-numeric label (`['a'..'c' as d] -> write [{d+1}]`), which must
   still be `char-arithmetic`.
   **[tool-verified]** log F7 records the current refusal and the two clean
   siblings.

4. **The clause position, under the EXISTING code.** A contract clause naming a
   set whose members are not all in the parameter's alphabet, and one naming an
   out-of-alphabet range, must both fail with **`contract-symbol-unknown`** —
   not with either new code. **[tool-verified]** log F14 records that a clause
   naming an out-of-alphabet glyph already fails with that code today, which is
   the before-picture this fixture extends to sets.
   **Mutation caught:** routing clause elements through this task's new
   pattern-cell strictness and thereby emitting `set-outside-alphabet` at a
   clause — the fixture asserts the code, not merely that it failed.

**Mutation caught by pair 3's near miss:** deciding foldability from "the
alphabet position is an integer index" instead of "the label parses as a
number" — every glyph becomes foldable and the near miss stops firing.

**And the one that guards the split:**
`tests/tmc_never_fires.rs::graft_rule_that_vanishes_only_at_the_splice_warns_and_the_rest_works`
must stay green **unchanged**. **Mutation caught:** removing the splice-time
`empty-expansion` along with the authoring-time drop — that test goes red,
which is what stops the strict rule from silently deleting generic-graph reuse.

**Suggested design.** `expand.rs::cell_options`'s two silent-drop paths become
errors — **three paths after the pattern-cell set task lands**, since a set
member off the tape is the same fact as a range member off the tape and must
raise `set-outside-alphabet` from the same place. `expand_rule`'s aggregate
`empty-expansion` warning at that site goes with them, since it can no longer
fire. `splice_state`'s `empty-expansion`
stays, with its message and code unchanged. **`empty-expansion` therefore
survives with exactly one firing site** — it does NOT retire from the code
registry, and `docs/tmt/language.md` and `docs/tmt/lint.md`, which both
currently describe it as the range-drop warning, must be reworded (Task 16). The
`char-arithmetic` predicate moves from `SymLit::is_glyph()` to a check on the
resolved label, which must agree with `expand.rs::numeric_value` — `eval_fold`
only `debug_assert!`s the invariant, so a disagreement is a debug-build panic in
release-adjacent code paths. Pin the agreement with a test over both.
`check_char_arithmetic` is also a `PatternCellKind` match site
(`parser.rs:2040-2042`) that the pattern-cell set task gives a `SetRef` arm:
**the two tasks touch the same match**, and whichever lands second must leave
the set arm's foldability decided by the same label rule, not by a second
predicate.

**The fixture blast radius is known and named.** `.superpowers/plan3b-fixture-check.md`
lists eight tests in `crates/turing-machine/tests/tmc_never_fires.rs` with a
disposition each. Six become error fixtures or move to an empty state body
(`entry state s { }` compiles — Established fact 9, log F1); one is unaffected;
one (`dropped_unreachable_rule_keeps_the_call_list_aligned`) needs its premise
restated, because the rule it drops must now be dropped for a surviving reason.

**TDD steps**

- [ ] **Step 1 — the definitive sweep, before any test is written.** Flip `cell_options`'s two drops to `todo!()`/an error, build, and run `cargo nextest run --workspace`. Record EVERY red test and give each a disposition in the task report. The planner's instrumented run covered the lib (940 tests, zero drops) and `tmc_never_fires` only; other binaries may surface, and discovering them here rather than at review is the point of this step.
- [ ] **Step 2: Tests first** — the three firing/near-miss pairs, the splice-warning survival pin, and the `numeric_value` agreement test.
- [ ] **Step 3: Implement** the two errors and the `char-arithmetic` predicate.
- [ ] **Step 4: Rewrite** the dispositioned fixtures in `tmc_never_fires.rs`, each keeping the property it was written to guard. A fixture whose property no longer exists is deleted with a one-line note in the commit body, not silently.
- [ ] **Step 5: Drift guards** — `error_code_docs.rs` for the two new codes; confirm `empty-expansion` is still in the registry with its splice-site meaning.
- [ ] **Step 6: Gate** — `cargo nextest run --workspace`; compiled-stdlib byte identity at both opt levels (Established fact 8 says the stdlib has no drop, so this must not move — that is the proof the rule does not touch shipped code).
- [ ] **Step 7: Commit** — two commits: `feat(turing-machine): a match cell outside the tape alphabet is an error` and `fix(turing-machine): foldability follows the symbol's label, not its quotes`.

---

### Task 12: `unused-set` — PULLABLE

**PULLABLE.** A default-on lint beside `unused-alphabet`; nothing depends on it.

**Property.** A declared `set` that no alphabet body, contract clause or pattern
cell references is reported once at its declaration, **and the finding carries
a quickfix that deletes the whole declaration — withheld when the declaration's
span holds a comment** — while a set referenced from any one of those three
sites is not reported at all.

**Falsifying test.**

| Fixture | Expected | Mutation it catches |
|---|---|---|
| a set referenced from an **alphabet body** only | silent | not recording a reference at the alphabet-body expansion site |
| a set referenced from a **contract clause** only | silent | not recording at the clause site |
| a set referenced from a **pattern cell** only | silent | not recording at the pattern-cell site (**needs the pattern-cell task**, which is why this rule is not pullable ahead of it) |
| a set referenced from nowhere | **fires once** | the rule itself — a `set_refs` lookup that never misses |
| the same fixture, the finding's `fix` field | **is `Some`** | the fix never attaching, e.g. `decl_span::<SetDeclView>` returning `None` because the view or the anchor is wrong. Separate row from the one above: a rule that fires with no fix passes "fires once" |
| the same, with a comment inside the declaration | fires, **fix withheld** | an edit span narrower than the declaration, which would slip past the central comment guard and silently delete the comment |
| the applied text of the fix | the declaration is gone and the rest of the file is byte-identical | an edit span that eats the following declaration or leaves a blank line |

Three separate reference near misses is deliberate: they are three *sites*, not
three properties, and one fixture referencing a set from all three would pass
under a recorder that is broken at two of them.

**Suggested design.** Read **both** surfaces the set-declaration task records:
iterate `Resolved.sets` (mangled name → declaration with `name_span`, the
shape `unused_alphabet.rs` iterates for alphabets) and test membership against
`Resolved.set_refs` (mangled names). Using the AST's `Program.sets` instead
would mean matching unmangled names against mangled references — the trap that
rules out the hand-rolled walks in the first place. This is the
`unused_alphabet.rs` shape (one uniform source), not the
`unused_map.rs` shape (three hand-rolled walks) — that recording exists
precisely so this rule does not hand-roll a fourth walk. **Export-independence**:
like `unused-alphabet`/`unused-tape`, an exported set is still reported if
nothing in the unit uses it — the established house rule, stated so it is not
re-litigated.

**The quickfix, and why it is cheap.** The rule this mirrors is
`lint/rules/unused_map.rs:62-69`, a whole-declaration delete:

```rust
let fix = decl_span::<SetDeclView>(ctx, set.name_span).map(|span| Fix {
    description: format!("delete the unused set `{name}`"),
    applicability: Applicability::MaybeIncorrect,
    edits: vec![Edit { span, replacement: String::new() }],
});
```

`SetDeclView` comes from the set-declaration task's `syntax/views.rs` work, and
`set.name_span` from its `Resolved.sets`. `decl_span<V: AstNode>(ctx, anchor)
-> Option<Span>` is the existing helper at `lint/rules/spans.rs:55`.
**This is not new ground** (Established fact 19 corrects the earlier claim that
it would be): eleven `.tmc` rules already emit a `Fix`, the central comment
guard in `lint/mod.rs:248-278` runs unconditionally over every rule's
diagnostics, and the harness is already in place. Two pins must be added and
one count updated:

- `crates/turing-machine/tests/lint_fix_comment_guard.rs` — a
  comment-inside / comment-free pair, in the shape the other nine use (the
  comment-free sibling is what stops the withhold assertion passing
  vacuously). **Update that file's module-doc roster**, which reads verbatim
  *"Roster: eleven rules emit a `Fix`. Nine are pinned here"* → twelve and ten.
- `crates/turing-machine/tests/lint_quickfix_comments.rs` — the applied text.

**TDD steps**

- [ ] **Step 1:** Build the six fixtures and run `tmt lint` at HEAD (the grammar exists once the set tasks have landed); record.
- [ ] **Step 2: Tests first** — the six rows above, the two harness pins, the roster-count update, plus a corpus sweep asserting zero findings.
- [ ] **Step 3: Implement** the rule as a new `lint/rules/unused_set.rs` reading `set_refs` and attaching the fix; one `mod` line; one `RULES` tuple.
- [ ] **Step 4: Drift guards** — a `### unused-set` heading in `docs/tmt/lint.md`'s `.tmc` section (or `lint::docs_drift` is red both directions), and the page's quickfix-availability column.
- [ ] **Step 5: Gate** — `cargo nextest run -p mtc-turing-machine`.
- [ ] **Step 6: Commit** — `feat(turing-machine): an unused named set is a lint with a deletion fix`.

---

### Task 13a: `.tmh` reaches `tmt fmt` and `tmt lint` — PULLABLE

**PULLABLE.** A mechanical CLI gap; nothing depends on it.

> **Why 13a and 13b are separate tasks.** The `.tmh` dispatch is three known
> line edits plus a registry flag — bounded, reviewable in one sitting. Making
> a generated header `fmt`-clean is a printer refactor whose size is not known
> until the divergence is measured. Bundling them would put an open-ended
> refactor behind a mechanical fix and make the pair un-reviewable, which is
> the shape the pre-flight scan flagged.

**Property.** A `.tmh` is visible to `fmt` and `lint` **the same way whether it
is named explicitly or found by a directory scan**, and a `.tmh` is linted with
the rules that mean something on declarations and silent on the rest.

**Falsifying test.**

| Fixture | Expected | Mutation it catches |
|---|---|---|
| `tmt fmt --check` on `std.tmh` | exits 0 | the per-file match arm never gaining a `tmh` case |
| `tmt lint` on a **directory** holding a `.tmh` with a header-rule finding | reports it, identically to the explicit-file form | fixing the per-file arm and leaving `collect_sources`'s filter alone — the explicit form works, the scan form reports nothing. Only a test running BOTH catches it; this is the divergence measured live (log F5 vs F6) |
| a `.tmh` containing a construct a **body** rule would flag | silent | a header flag that is declared but never consulted |
| a `.tmh` containing a construct a **header** rule flags | reports | a flag that suppresses everything |
| **near miss** — a `.tmo` given to `fmt` | still refused | widening the extension match to "anything not `.tmo`" |

**One filter, not two.** `cli/fmt.rs:27` already imports `collect_sources` from
`cli::lint`, so the directory filter (`cli/lint.rs:244`,
`x == "tmc" || x == "tma"`) is **shared** — one edit there serves both
subcommands. The per-file match arms are separate (`cli/fmt.rs:189`,
`cli/lint.rs:187`) and both need the case. So it is three edits, not four, and
the plan says which is which so the implementer does not go looking for a
second filter.

**Which rules run on a `.tmh`.** See "Decisions for the controller": the
strict-mode read's own diagnostics, plus the contract-clause consistency rules,
plus `unused-import`; every `unused-*`/`dead-*` over bodies and every rule that
reads a rule grid is off. The mechanism is a per-rule "applies to headers" flag
in the registry, drift-guarded like the rest, exactly as the spec's `.tmh`
paragraph describes — **and the flag gets its own test**, the two middle rows
of the table above.

**TDD steps**

- [ ] **Step 1:** Re-run log F5 and F6 to confirm both behaviours at HEAD.
- [ ] **Step 2: Tests first** — all five rows, red.
- [ ] **Step 3: Implement** the two per-file match arms, the one shared `collect_sources` filter, `registry.rs::source_or_dir`'s extension list (`.tmh` is already a known extension there for `--extern`), both USAGE strings, and the per-rule header flag.
- [ ] **Step 4: Drift guards** — `cli_docs.rs` byte-compares `fmt --help` and `lint --help` against `docs/tmt/cli.md`; `completions_registry.rs`; `man_page.rs`; `lint::docs_drift` if the header flag is documented per rule.
- [ ] **Step 5: Gate** — `cargo nextest run -p mtc-turing-machine`. **`tests/stdlib_header.rs`'s byte pin must NOT move in this task** — nothing here changes what `tmt interface` prints.
- [ ] **Step 6: Commit** — `feat(cli): fmt and lint accept a header`.

---

### Task 13b: A generated header is `fmt`-clean — PULLABLE

**PULLABLE.** It fixes a cosmetic divergence and nothing depends on it. **If
step 1's measurement shows the divergence is wider than the grid, this task is
the one to defer** — it is the only task in the phase whose size is not known
until it starts.

**Property.** For every shipped source, `tmt interface`'s output passes
`tmt fmt --check` unchanged — and the canonical form is produced by ONE code
path, not by a header printer taught to imitate the formatter.

**Falsifying test.** `tmt interface` on each shipped source, its output written
to a `.tmh` in the scratchpad, then `tmt fmt --check` on that file: exit 0, for
every source. **Mutation caught:** a header printer that hard-codes the grid it
believes `fmt` uses — the moment `fmt`'s grid rule changes, the test goes red,
which is exactly what a second printer would hide.

**The named code path, and the scope bound.** The divergence is in the **rule
grid inside a header's graph bodies**: `header.rs` renders graph-body rules
itself (`header.rs:1733-1737` is its pattern-cell printer), while `.tmc` fmt
prints rules through `fmt/print.rs` from the green tree, with the canonical
grid. **The fix is that `header.rs`'s graph-body rule rendering calls
`fmt/print.rs`'s rule printer rather than formatting rules itself.** That is
the whole intended scope.

**Bounded by measurement, not by hope.** Step 1 measures the divergence before
any edit. **If it is confined to the rule grid, implement the call-through and
stop. If step 1 shows divergences outside graph-body rules** — header wrapping,
doc-line layout, declaration spacing — **stop and report rather than widening
the task**: each of those is a separate property with a separate canonical
form, and this task's property does not license changing them.

**TDD steps**

- [ ] **Step 1: Measure first.** Run `tmt interface` over every shipped source, write each output to the scratchpad, run `tmt fmt --check` on each, and record the exact diffs. Classify each as grid or non-grid. **A non-grid diff is a stop-and-report.**
- [ ] **Step 2: Tests first** — the corpus-wide `interface` → `fmt --check` test, red.
- [ ] **Step 3: Implement** the call-through from `header.rs`'s graph-body rendering to `fmt/print.rs`'s rule printer.
- [ ] **Step 4: Gate** — `cargo nextest run -p mtc-turing-machine`; **`tests/stdlib_header.rs`'s `std_tmh_is_what_tmt_interface_prints` byte pin is expected to MOVE here** if `std.tmh` carries a graph body whose grid changes. Regenerate via the `#[ignore]`d `regen` test, **never by hand**, and state in the report that this task moved it. The stdlib-annotation task moves the same pin for a different reason; whichever runs second re-regenerates.
- [ ] **Step 5: Commit** — `fix(turing-machine): a generated header is fmt-clean`.

---

### Task 14: The undeclared-exit body scan — PULLABLE

**PULLABLE.** A 3a carry, independent of every #121 fold. Touches
`crates/core` — **core source diff 2 of 2.**

**One ordering note, not a dependency.** Its second near miss uses `trap #2`,
which assembles today and only *means* something once the trap-kind task has
landed — the near miss works either way (it is testing that the scan does not
confuse a trap with a `retx`), but if this task runs first the `#2` in that
fixture is a kind the VM would reject at execution. The fixture never executes
it, so nothing breaks; stated so a reviewer does not read it as a hidden
dependency.

**Property.** A callee whose linked body executes `retx #k` for a `k` at or
above its declared exit count is reported at link time, naming the callee and
the index — a diagnostic where today there is only a controlled runtime failure.
The same scan's diagnostic also covers the `duplicate-tape-target` aliasing that
goes unchecked at external sites.

**Falsifying test.** A link of a hand-written `.tma` callee declaring two exits
whose body carries `retx #2` → the diagnostic. Two near misses: `retx #1`
(within range) → silent, and a `trap #2` at the same offset → silent, because
`trap` and `retx` share `OperandKind::Imm8` and are told apart only by
`entry.flow` (`Flow::Stop` vs `Flow::FallThrough`).
**Mutation caught by the second near miss:** discriminating on operand kind
alone — the `trap` near miss starts firing. This is the mutation the map warns
about and it is the reason the second near miss exists.

**Tier — the plan's decision, stated for the controller.** **A warning**, in
the shared allow namespace and promotable by `-Werror` on `link`, not an error.
Reasons: the runtime outcome today is a *controlled* trap (`ProfileViolation` /
`ExitOutOfRange`, exit 3, under all mechanisms — 3a's Task 16 review measured
this), so nothing is unsound; hand-written `.tma` is a supported authoring
surface where a deliberately over-wide body is conceivable; and the arc's own
precedent (`tail-call-no-continuation`, glyph reordering) is that a
truthfulness gap between a declaration and a body is a promotable warning.
The alternative — an error — is named under "Decisions for the controller".

**Core-diff justification (R-3b-5).** `crates/core/src/linker/` only:
a `body_exits(syntax, blob) -> impl Iterator<Item = u8>` sibling to
`stamp.rs::body_has_return`, decoding the blob through `decode::decode_stream`
and filtering by `ArchSyntax` flow — **no new `ArchSyntax` field, no TM
knowledge** — plus one `DIAGNOSTIC_CODES` row in `linker/mod.rs`. It is the
same shape, in the same file family, as the `callee_can_return` predicate the
linker already runs.

**One trap the map records:** stamps set `interface: None`
(`stamp.rs:1636-1641`), so a post-stamp scan over the stamped order sees nothing
to check against. **The scan must run on the original, pre-stamp function
records.**

**TDD steps**

- [ ] **Step 1:** Build the firing `.tma` and its two near misses and link each at HEAD; record that all three link silently today and that the firing one traps at run time with the controlled outcome.
- [ ] **Step 2: Tests first** — the three, in `crates/core/tests/link_checks.rs` against the fake arch (neutrality: the fake arch must be able to express `retx`; if it cannot, extend the fake, not the real one), and a TM-side end-to-end in `crates/turing-machine/tests/link_warnings.rs`.
- [ ] **Step 3: Implement** `body_exits` and the check on pre-stamp records; the `duplicate-tape-target` external-site case rides the same walk.
- [ ] **Step 4: Drift guards** — `crates/core/tests/error_code_docs.rs` against `docs/core.md`'s `### Link warnings` AND `crates/turing-machine/tests/error_code_docs.rs` against `docs/tmt/cli.md`'s `### Link warnings`. Both, in both directions.
- [ ] **Step 5: Gate** — `cargo nextest run --workspace`; **PM-1 byte identity explicitly** (`golden_programs`, `asm_volatile`); a corpus link sweep confirming zero new findings over every shipped program.
- [ ] **Step 6: Commit** — `feat(core): a body that returns through an undeclared exit is a link warning`.

---

### Task 15: The stdlib annotation, and the 3a carries

**PULLABLE: no.**

**Property.** Every `std.tmc` tape parameter whose `?` doc lines state a head
position carries the matching `enters`/`leaves` clause; `std.tmc`'s line count
is unchanged; the compiled stdlib object is byte-identical at both opt levels
before and after; and `std.tmh` is what `tmt interface std.tmc` prints.

**Falsifying test.** The line-count assertion, the object byte-comparison at
`-O0` and `-O1`, `tests/stdlib_header.rs::std_tmh_is_what_tmt_interface_prints`,
`the_header_and_the_source_agree_on_every_declared_contract` extended to the two
new clause kinds, and `crates/wasm/tests/stdlib.rs::debug_stdlib_links_to_the_same_code_as_the_release_stdlib`.
**Mutation caught by the object comparison:** a stdlib preset built with asserts
UNstripped — the object grows the check states and the byte pin goes red.
**That comparison is a real falsifier only because the presets were set to
strip asserts in the earlier option task**, before any assert existed, where the
change was a provable no-op. Here it is the *consequence* being checked, not the
mechanism being introduced: this task adds clauses and proves the object does
not move.

**Which pins move and which must not:**

**Named by instrument, not in the abstract** — these are the helpers the option
task made strip, and they are what actually executes each row:

| Pin | Instrument | This task |
|---|---|---|
| the compiled stdlib object at both levels | `tests/opt_equivalence.rs::stdlib_object_bytes(level)` | **must NOT move** |
| the stdlib optimizer floor | `opt_equivalence.rs` Part B's `fired == {"jump-threading"}` and its strict `-O1` shrink | **must NOT move** — a red floor here means a preset or helper is not stripping, not that the optimizer changed |
| the 132 derivation-first behavioural runs | `tests/stdlib_golden.rs::stdlib_object(level)` | **must NOT move**, and must stay apples-to-apples across levels |
| the wasm debug twin's whole-image equality | `crates/wasm/tests/stdlib.rs::debug_stdlib_links_to_the_same_code_as_the_release_stdlib` | **must NOT move** |
| the emitted stdlib assembly | `tests/state_params.rs`'s `assembly(stdlib::SOURCE, level)` at `:658` | **must NOT move** |
| the header | `tests/stdlib_header.rs::std_tmh_is_what_tmt_interface_prints` | **MOVES** — the header gains the clauses; regenerate via the `#[ignore]`d `regen` test, never by hand |
| `std.tmc`'s line count | a direct assertion in this task | **must NOT move** |

**The annotation is derived from the BODY, and disagreements are reported.**
The `?` lines state the contract in prose — the #121 probe's own `headlib.tmc`
shows the `ENTRY: … EXIT: …` convention, and `std.tmh`'s shipped doc lines
carry the same claims for every routine (log F8). **But the prose is already
known to be narrower than the body in at least one place**, and the file says
so itself: `goToEnd`'s doc reads "ENTRY: head on the number — any of `'^'`,
`'0'`, `'1'`, `'$'`" and then, two lines later, "(The body also accepts `'_'`
without trapping: it just walks on.)".

**Where the `?` prose and the body disagree, the clause is derived from the
BODY, the prose is corrected to match, and the disagreement is reported.** A
clause transcribed from the narrower prose would be a promise the library does
not need to keep and its callers would violate: the runtime `enters` assert
would trap on a correct program, and the `enters-unmet` sweep over `std.tmc` —
a **joint gate this task must leave green**, not an independent one — would
fire. Never silently narrow. Every disagreement goes in the task report
(R-3b-3 requires it), with what the prose said, what the body does, and which
the clause took.

**The line-count invariant is tight and the mechanism is prose reclamation.** A
one-line signature such as
`export routine deleteNumber(tape num: symbols writes { '_' }) {` cannot carry
two more clauses within the line limit; `fmt` would wrap it and the count would
move. The structured clause supersedes the prose that states the same fact, so
the doc run above each routine sheds the line the clause now carries. Where that
does not balance, **stop and report** — see "Decisions for the controller" for
the alternative the controller may take instead.

**The 3a carries this task also closes** (each is small, each is a named parked
item, and they share this task because they share its review):

- **N-2, the `use`-line rule's second half.** Fold "prints nothing unneeded"
  into `every_shipped_source_interface_accepts_round_trips_through_the_strict_reader`:
  for each printed `use` line, delete it, re-read strictly, and expect failure.
  A printer emitting every import unconditionally leaves the suite green today.
- **Parked 4** — a one-line mutation statement on
  `interface_emission.rs::the_writes_suffix_lists_the_effective_set`.
- **Parked 9** — the `inline` exclusion pin cannot fail under a realistic
  mutation. Replace it with an equivalence run over an exit-bearing callee at
  `-O1`, or delete it. **A test that cannot go red is not a guard**; do not
  leave it as is.
- **Parked 10** — `duplicate-tape-target` aliasing at external sites: closed by
  Task 14; confirm and strike it.
- **Parked 12** — correct the `duplicate-graft-instance` rule's comment about
  the "survivor has no name" branch, which IS reachable.
- **Parked 13** — a direct test for the binding-argument branch of
  `other_reference_exists`.
- **Parked 14** — a test for the declarations-missing arm of the trailing
  mutual-dependency note.
- **3a Task 9, M4 and M6.** M6's half is covered by Task 10a's corpus fixture. M4
  needs a unit that exports a named map with a qualified or imported alphabet as
  a pair's source or destination — **as a test fixture, not as stdlib text**,
  because R-3b-3 forbids growing `std.tmc`. Add it under `tests/golden` so the
  corpus sweeps pick it up.

**TDD steps**

- [ ] **Step 1:** `wc -l crates/turing-machine/src/stdlib/std.tmc` and record. Compile the stdlib at `-O0` and `-O1` and save both objects to the scratchpad — these are the before-bytes.
- [ ] **Step 2:** For every routine, read the `?` lines AND the body, and derive the clause **from the body**. **Report every disagreement — prose, body, and which the clause took — before editing anything.** At least one is known to exist (`goToEnd`).
- [ ] **Step 3: Tests first** — the line-count pin, the object byte-comparison at both opt levels, the wasm twin image pin, the extended header/source agreement, and the N-2 minimality fold.
- [ ] **Step 4: Annotate** `std.tmc`; regenerate `std.tmh` via the `#[ignore]`d `regen` test; **never hand-edit the header.**
- [ ] **Step 5:** The seven carries above, each with its own test or its own deletion.
- [ ] **Step 6: Gate** — `cargo nextest run --workspace`; **the `enters-unmet` sweep over `std.tmc` must be green** (the joint gate); the wasm bundle and smoke (`scripts/build-wasm-bundle.sh` **without** the target-dir prefix, then `node scripts/wasm-smoke.mjs target/wasm-bundle/dist`), because the wasm crate links its own debug-compiled stdlib twin and pins it against the release object. **If the sweep is red, the defect is a clause that is too narrow, not the lint.**
- [ ] **Step 7: Commit** — `feat(turing-machine): the standard library declares where its heads enter and leave`, and a separate `test(turing-machine): the parked guards from the previous phase`.

---

### Task 16: Documentation and the full gate

**PULLABLE: no.** Last.

**Property.** Every surface 3b added or changed is described on the durable
page that owns it, every claim on those pages is true of the built tool, and
every `docs/<page>.md (keyword)` citation added anywhere in the phase resolves.

**Falsifying test.** The drift guards, all of them, in both directions — and
the implementer's own claim list, each entry run through the built tool with its
output recorded. A docs task whose claims were not executed is the failure mode
3a's own docs task was written to prevent.

**Pages, and what each owes:**

| Page | Owes |
|---|---|
| `docs/tmt/language.md` | head contracts (both clauses, the optional-never-empty rule, the static/runtime split); named sets (declaration, the three legal positions and the three illegal ones, cycles); open bindings (`*` last, the inferred `opaque`, never spelled); **the strict range/set rule**; the reworded `empty-expansion` (now the graft-splice diagnostic only); the reserved-word list at 31; the version-history entry |
| `docs/tmt/cli.md` | `--strip-asserts` on `compile` and `build`; the new `--release` expansion; every new error code's row; the new link-warning row; the `.tmh` mentions in `fmt`/`lint` usage — **all quoted blocks byte-identical to real `--help`** |
| `docs/tmt/lint.md` | `### enters-unmet`, `### unused-set` **with its quickfix availability**, the per-rule header flag, and the reworded `empty-expansion` cross-reference |
| `docs/tmt/project.md` | schema `0.2` → `0.3`; `strip-asserts` in both profile tables |
| `docs/tmt/isa.md` | `trap #2`; the replaced "### Explicit traps" rationale paragraph; the opcode-table row; **the trap-kind row at `:456`**, which today reads "or an explicit `trap #0` / `trap #1`" |
| `docs/tmt/optimizer.md` | **`:276` reads verbatim "`trap #0` / `trap #1` for the two synthesized trap kinds" — now three**, with the new IR terminal beside them. The page was absent from this table before fix round 2 |
| `docs/core.md` | the Execution taxonomy table's new `Trap` variant — **the table is now guarded in both directions by the new `crates/core/tests/execution_trap_docs.rs`**, so state that, not the old "unguarded" note; plus the new link warning |
| `docs/dap.md` | the `contract` stop reason |
| `docs/formats.md` | the `.param` suffixes, if any wire text moved |
| `docs/tmt/stdlib.md` | the annotated contracts |
| `CLAUDE.md` | the version table (reserved words 31, `tmt.json` 0.3, `.tmc` 0.2, `.tma` 0.5, TM IR 4); the standing-state paragraphs; **the comment-position audit counts**, which four tasks add to (`comment_positions.rs` gains entries for the two clause headers, the open-binding `*` in a list, the `set` header and body, and a set pattern cell) — **this page is the one place those counts live, and no earlier task owns them**; the fmt residual counts if any moved; and the stale "#109 gated on measurements" line, a pre-existing correction this phase may as well carry |
| `crates/turing-machine/src/cli/lint.rs` | its module doc claims "no `.tmc` or `.tma` rule emits a machine-applicable fix" — **stale today** (eleven do) and more so after `unused-set`. Correct it to the true, narrow statement: there is no `tmt lint --fix` driver |

**Carries file.** `task-16-carries.md` accumulates execution-time rulings from
every earlier task. Tasks 5, 7, 11 and 14 each write into it explicitly.

**TDD steps**

- [ ] **Step 1:** Read `task-16-carries.md` in full before writing a word.
- [ ] **Step 2:** Write the pages. Every example runs through the built tool; every transcript is pasted from a real run.
- [ ] **Step 3:** The claim list — one row per claim, with the command and its output.
- [ ] **Step 4:** Citation sweep — every `docs/<page>.md (keyword)` added in this phase resolves to a real heading or anchor. A dangling one is a defect.
- [ ] **Step 5: Full gate** — `cargo fmt --check`; `cargo clippy --workspace --all-targets -- -D warnings`; `cargo build -p mtc-core --no-default-features`; `cargo build --workspace --lib --target wasm32-unknown-unknown`; `scripts/build-wasm-bundle.sh` (no target-dir prefix) then `node scripts/wasm-smoke.mjs target/wasm-bundle/dist`; `cargo test -p mtc-post-machine --test golden_programs`; `cargo test -p mtc-post-machine --test asm_volatile`; `cargo nextest run --workspace`. One command at a time.
- [ ] **Step 6: Commit** — `docs(turing-machine): head contracts, named sets and open bindings`.

---

## Behaviour changes in phase 3b

In prose a release note can quote.

1. **Programs that previously compiled now fail.** A match cell or contract
   clause naming a glyph the tape's alphabet does not have — a single glyph, a
   set member, or any member of a range's expansion — is now the compile error
   `set-outside-alphabet` or `range-outside-alphabet`. Previously such an
   alternative was dropped silently, and a rule that lost *every* alternative
   raised the `empty-expansion` warning. **No shipped program is affected**:
   every program under `docs/examples`, every golden fixture, the embedded
   standard library and the probe corpus expand with every alternative present
   (measured). The change surfaces only in code written against the old,
   permissive reading.

2. **`empty-expansion` keeps one meaning and loses the other.** It no longer
   describes a rule whose alternatives fell outside the alphabet — that is now
   an error. It still describes a *grafted* rule that maps to no host symbol at
   its splice: a generic graph body reused against a host binding that covers
   none of its match cells. Generic graph reuse is unaffected.

3. **A call written without a continuation now traps as a contract violation.**
   Where the compiler previously emitted `trap #0` — the unmapped-read kind,
   chosen because no better one existed — it now emits `trap #2`, a kind named
   for exactly this: a callee the program believed could never return, did.
   Programs that never reach that trap are byte-identical apart from the one
   immediate; a program that does reach it reports a clearer outcome.

4. **`trap #2` is now a valid instruction kind.** Hand-written assembly using
   `trap #2` previously assembled and then failed at execution with a malformed
   operand. It now raises the contract trap. `trap #3` and above are still
   malformed.

5. **`--release` does more.** It now expands to
   `-O1 --strip-debugger --strip-asserts`. For a program with no head contract
   this changes nothing; for one with contracts it removes the runtime checks,
   which is the intent of a release build.

6. **`tmt fmt` and `tmt lint` accept a header.** A `.tmh` named explicitly was
   previously refused with "unknown source extension"; a `.tmh` found by a
   directory scan was silently skipped. Both now behave the same way, and a
   header generated by `tmt interface` is `tmt fmt`-clean.

7. **Arithmetic on a quoted glyph whose label is numeric now folds.**
   `['0'..'8' as d] -> write [{d+1}]` was refused as `char-arithmetic`; it now
   behaves exactly like `[0..8 as d]`, because `'0'` and `0` name one symbol.
   A fold over a genuinely non-numeric label is still refused.

8. **New diagnostics on previously silent programs.** The `enters-unmet`
   warning fires where a call site may hand a callee a head position its
   contract excludes. A link now warns when a body returns through an exit its
   signature does not declare — a controlled runtime trap before, a named
   diagnostic now, promotable by `-Werror` and silenceable through the shared
   allow namespace.

9. **Debug builds of contract-bearing routines are bigger and slower.** The
   synthesized check states add `|set| + 1` rows per clause per tape, chained,
   with no product blow-up. `--strip-asserts` removes them entirely; the
   embedded standard library ships with them stripped.

10. **`tmt.json` grows a profile key.** `strip-asserts` joins `strip-debugger`,
    `opt`, `debug-info` and `werror`; the `project` section's schema is 0.3.
    Existing files are unaffected.

11. **Three words become reserved.** `enters`, `leaves` and `set` are now
    keywords of `.tmc`, so a program using any of them as an identifier — a
    state name, a tape name, an alphabet name, an `as` binding — stops
    compiling. No shipped program, example, golden fixture or stdlib source is
    affected (checked two ways, including inline test fixtures), but the
    change is the kind a release note must carry, because it breaks source
    that was legal in the previous language version.

12. **`unused-set` findings carry a deletion quickfix**, withheld when the
    declaration's span holds a comment, like every other whole-declaration
    delete in the `.tmc` rule set. There is still no `tmt lint --fix` driver;
    editors consume the fix through the language server.

---

## Self-review

### 1. Spec coverage — every clause of "### Folded in from #121" and every carry

| Spec clause / carry | Task |
|---|---|
| `enters`/`leaves` grammar, canonical order, same element grammar | 2 |
| …and "at most one of each", via the existing order/duplicate errors | 2 |
| Two new reserved words (plus `set`) | 1 |
| Static `enters` check against the entry state's accepted cells | 4 |
| Static `leaves` check where inference is exact; moved rows left to runtime | 4 |
| Neither clause inferred when absent | 2, 4 |
| Synthesized check states, chained per tape, `\|set\| + 1` rows | 7 |
| `trap #2 = contract`, a new named kind in core and the ISA | 5 |
| …and its IR terminal `IrTransition::TrapContract` inside the unreleased v4 | 5 (7 lowers to it) |
| `tmt run` names routine, tape, clause, glyph | 7 — **with `-g`**; without it the kind and the address, pinned by a degradation test |
| `-g` maps the trap to the signature line | 7 |
| DAP stop reason `contract` | 7 |
| `--strip-asserts` on `compile`/`build`, independent of `--strip-debugger` | 6 |
| `--release` = `-O1 --strip-debugger --strip-asserts` | 6 |
| Manifest profile key; `tmt.json` `project` 0.2 → 0.3 | 6 |
| `-O0` bit identity without clauses | 6, 7 (by construction, R-3b-4) |
| Optional by construction, never empty; `empty-head-clause` | 2 |
| Object reader refuses an empty present list | 3 |
| `RoutineInterface` carrier, `.param` suffixes, headers, `tmt interface` | 3 |
| Every stdlib tape annotated (from the body where the prose disagrees) | 15 |
| `enters-unmet` lint, forward dataflow, warn tier, no quickfix | 8 |
| …"runs in the editor through the overlay" | **deferred to phase 4**, named in the Deferred paragraph |
| Open bindings: `*` in `with map` at call/bind/graft sites → the `open` bit | 9 |
| …and a top-level `map` declaration still refuses `*` | 9 |
| Compiler-side per-tape `opaque` inference, exported, never spelled | 9 |
| `set NAME { … }`: literals, ranges, other sets, cycle-checked | 10a |
| Namespaced, `export`able, `use`-importable, carried in `.tmh` | 10a |
| Legal in alphabet bodies and contract clauses; never a tape type | 10a |
| Legal in a pattern cell, **with or without `as`**; not in a write cell | 10b |
| Expands before IR; the object never sees it | 10a, 10b |
| `unused-set`, **with the deletion quickfix and its comment-guard pin** | 12 |
| Strictness: `set-outside-alphabet` / `range-outside-alphabet` in a pattern cell and an alphabet body | 11 |
| …and in a contract clause | 11 — **under the pre-existing `contract-symbol-unknown`**, one code per position class, with a fixture pinning which code fires |
| The silent drop and its `empty-expansion` warning go | 11 |
| `char-arithmetic` fixed on the label, not the quotes | 11 |
| Everything-matrix gains an open-binding and a contract-assert program | 7, 9 |
| **Carry:** continuation-less-call trap `#0` → `#2`, with the golden sweep | 5 |
| **Carry:** link-time body scan for undeclared exit returns (tier proposed) | 14 |
| **Carry:** `duplicate-tape-target` unchecked at external sites | 14 |
| **Carry:** `tmt fmt`/`tmt lint` accept `.tmh` | 13a |
| **Carry:** a generated header is fmt-clean; ONE printer | 13b |
| **Carry:** the `use`-line rule's "prints nothing unneeded" half (N-2) | 15 |
| **Carry:** parked minors 4, 9, 10, 12, 13, 14 | 15 (10 via 14) |
| **Carry:** 3a Task 9's M4 and M6 halves | 15 (M4), 10a (M6) |
| Docs on every page; `CLAUDE.md` standing state | 16 |

Nothing from phase 4 appears: no `outline` multi-exit, no LSP overlay, no
`.tmh` editor routing, no stdlib rewrite, no `docs/lsp.md`, no `CHANGELOG.md`,
no 0.6 release. The three clauses of the #121 section that touch phase-4
surfaces are named in the **Deferred** paragraph rather than left silent.

### 2. Placeholder scan

No task contains a `TODO`, a `…`-in-code, or a fixture that was not either run
by the planner (**[tool-verified]**, thirteen entries in the fixture log) or
marked **[shape-derived: …]** with the source it was derived from and the step
that confirms it. The shape-derived fixtures are in Tasks 2, 5 and 9, each as
step 1 of its own task; the set tasks' before-pictures are now tool-verified
(log F12, F13) rather than derived.

### 3. One guard per property

Checked deliberately, because 3a's ledger records a test whose stated mutation
could not fail:

- Task 3's two-arm byte comparison **replaces**, rather than supplements, a
  name-only assertion.
- Task 6's manifest pair tests two *directions* of one property — one guard
  read both ways.
- Task 7's stripping property has two layers by design and they are not
  redundant: the object byte-comparison is the user-visible fact, the IR
  assertion is the only one that can go red under a clean codegen filter.
- Task 12's three reference near misses cover three *sites*, not three
  properties, and the plan says why a combined fixture would not discriminate.
  Its "fires once" and "the fix is `Some`" rows were one row carrying two
  mutations and are now two rows, because a rule that fires with no fix passes
  the first.
- Task 6's headline byte-comparison **states plainly that it is not a
  falsifier** while no assert exists; the task's real guard is the threading
  test, and the plan says so rather than letting a no-op check look like one.
- Task 14's second near miss exists solely to catch the operand-kind confusion.
- Task 10a's `elem_indices` row exists because that arm is the one consumer
  whose wrong answer (`SetRef => None`) compiles and passes everything else
  while silently switching `contract-clause-overlap` off.
- Task 2's two duplicate fixtures (`enters … enters`, `leaves … leaves`) look
  redundant and are not: a single shared `seen` flag passes the first and
  fails the second.
- Task 15 explicitly deletes or replaces a parked pin that cannot go red.

### 4. Version consistency

`TMC_LANG_VERSION` `0.2` (no task moves it), `TM_IR_VERSION` `4` (Task 3 adds
fields inside it), `.tma` TM-1 dialect `0.5` (no task moves it; Task 3 confirms
phase 1 left no gap), `tmt.json` `project` `0.2` → `0.3` (Task 6 only, prose
marker only — there is no code literal), reserved words `28` → `31` (Task 1
only), crates `0.5.x`, MO `4`.

### 5. Core budget

Two source diffs (Task 5's `vm/` trio, Task 14's `linker/` scan) plus one test
addition (Task 5's execution-trap-table drift guard). Global Constraints states
the budget in exactly those terms, so no task is instructed to stop and report
for a change this plan asks it to make.

---

## Decisions for the controller

Each was decided so the plan could proceed; each names the alternative.

1. **The strict rule kills the authoring-time drop and keeps the graft-time
   warning.** `empty-expansion` survives with exactly one firing site and does
   NOT retire from the code registry. **Alternative:** kill both, retiring the
   code entirely. **Rejected** because the splice-time diagnostic runs on an
   already-expanded graph body and reports an *instantiation* fact the author of
   a generic graph cannot know — it is the only graceful signal for generic
   graph reuse, and removing it would make a reusable graph either illegal or
   silent. The two diagnostics share a code today and the plan keeps it; if the
   controller prefers a distinct code for the surviving site, say so before Task
   11.

2. **`unused-set` reads a normalized reference surface that the set tasks
   record.** `Resolved` has none today and a `set` expands before a `Resolved`
   exists, so Task 10a adds `Resolved.set_refs` at the one place each expansion
   consumes a set name (and 10b adds the pattern-cell site to the same set).
   **Alternative:** have the lint hand-roll three walks over the AST, as
   `unused_map.rs` does for its three site kinds. **Rejected** under
   "one concept, one implementation": three walks over unresolved names would
   also mishandle imported and qualified set names, which the resolver already
   knows how to bind.

3. **The accepted-glyph primitive moves out of `lint/`.** `lint/patterns.rs`
   becomes `src/patterns.rs` and gains one `accepted_glyphs` function that
   `state_may_trap`, `dead_rule` and the compiler's static `enters` check all
   call. **Alternative:** leave it in `lint/` and have the compiler call
   `crate::lint::patterns::...`. **Rejected** because a compiler that depends on
   the lint layer inverts the crate's layering; the alternative is cheap to
   switch to if the controller prefers it.

4. **The undeclared-exit body scan is a warning, not an error.** In the shared
   allow namespace, promotable by `-Werror` on `link`. **Alternative:** an
   error. The case for it: a body returning through an undeclared exit is a lie
   in a published signature, and every other *declaration-versus-body* lie in
   this arc is refused at link time. The case against, which the plan takes: the
   runtime outcome is already a controlled trap under all three mechanisms, and
   hand-written `.tma` is a supported surface. **This is the most likely
   decision to be overturned; Task 14 is PULLABLE and the tier is a one-line
   change inside it.**

5. **Which lint rules run on a `.tmh` (R-3b-6).** Proposal: the strict-mode
   read's own diagnostics, plus `contract-clause-overlap` **as it is today —
   `writes` × `preserves` only** — plus the two new head-contract checks, plus
   `unused-import`. Off: every `unused-*` over bodies, every `dead-*`, every
   rule that reads a rule grid. **`enters` and `leaves` are never
   overlap-checked**: two head clauses naming the same glyph is the normal
   shape for a routine that stops on the marker it entered on, and "the
   contract-clause consistency rules" must not be read as licence to extend the
   overlap computation over them. Mechanism: a per-rule "applies to headers" flag
   in the registry, drift-guarded like the rest, **with its own test** (a body
   rule silent on a `.tmh`, a header rule firing). `fmt` on a header uses the
   same printer as `tmt interface`'s canonical form — one printer, which Task
   13b exists to make true rather than to preserve.

6. **The open-binding `*` is legal only as the last entry.** With or without a
   trailing comma. **Alternative:** anywhere in the list. **Rejected** because
   "the rest stays open" has no position-dependent reading, the disassembler
   already prints it last, and a free position gives `fmt` and the never-move
   audit a position with no canonical form.

7. **`std.tmc`'s line-count invariant is met by reclaiming prose.** The `?` doc
   lines already state each head contract in prose; the structured clause
   supersedes that sentence, so the doc run sheds the line the clause now
   carries. **Alternative, if it does not balance:** relax the invariant to "the
   compiled object bytes are unchanged and the wasm debug twin's image pin is
   re-verified", on the ground that the `-g` line table is regenerated from the
   same source in the same build, so a uniform shift is consistent. **Task 15
   stops and reports rather than choosing this itself.**

8. **3a Task 9's M4 lands as a test fixture, not as stdlib text.** The brief
   says M4/M6 are "now possible since 3b touches `std.tmc`", but R-3b-3 confines
   the `std.tmc` edit to `enters`/`leaves` at an unchanged line count, and M4
   needs a unit that *exports a named map*. The fixture goes under
   `tests/golden`, where the corpus sweeps pick it up. **Alternative:** widen
   R-3b-3 to allow a stdlib map export. **Not taken** — that is the phase-4
   stdlib rewrite.

9. **RULED BY THE CONTROLLER — `docs/core.md`'s Execution trap table IS
   guarded.** The plan originally proposed leaving it unguarded on the ground
   that a third core change would breach the two-diff budget. The controller
   overturned that and clarified the budget: **it counts core SOURCE diffs;
   a core test that guards a registry is free.** Task 5 therefore adds
   `crates/core/tests/execution_trap_docs.rs`, a bidirectional set-compare of
   the `Trap` enum against the page's "Trap causes" table modelled on
   `crates/core/tests/error_code_docs.rs`; Global Constraints states the budget
   in those terms; and Task 16 states that the table is guarded. **Closed — no
   decision outstanding.**

10. **RULED BY THE CONTROLLER — `unused-set` carries the deletion quickfix.**
    The plan originally withheld it on the ground that it would be the first
    TM-1 fix exercised end to end. **That ground was factually wrong**: eleven
    `.tmc` rules already emit a `Fix`, nine are comment-guard-pinned, and
    `unused_map.rs` is the exact whole-declaration-delete mirror (Established
    fact 19, corrected). The overturn is therefore cheap and precedented, not
    risky. Task 12 carries the fix, the `SetDeclView` it needs, the
    comment-guard pin, the roster-count update and the applied-text pin.
    `enters-unmet` still has none — by the spec, and because there is no single
    correct edit for "this call site may be wrong". **Closed — no decision
    outstanding.**

11. **The pattern-cell set surface is its own task (10b), and the `.tmh`
    one-printer fix is its own task (13b).** Both splits are the controller's
    ruling, taken because each second half is materially larger or less bounded
    than its first half: 10b drags in seven `PatternCellKind` match sites that
    10a does not touch, and 13b's size is unknown until its step 1 measures the
    divergence. **Alternative:** keep each as one task. **Rejected** — a task
    that cannot be reviewed in one sitting is where 3a's fix rounds came from.
    Both second halves are PULLABLE-adjacent: 10b is required by the strictness
    task, but 13b can be deferred outright without touching anything else.
