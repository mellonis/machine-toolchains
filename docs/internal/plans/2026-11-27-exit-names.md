# State-Parameter Names in the Object — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The object's routine interface records one name per state parameter, the `.routine` line spells them, and the name-less-reading stopgap is deleted.

**Architecture:** `RoutineInterface::exits` changes from a count to a list of names in `mtc-core` (wire, assembler, disassembler); the TM-1 IR carries the names to codegen; with every reading named, the TM-1 declarations table drops provenance and compares readings by equality.

**Tech Stack:** Rust, the pinned toolchain; `cargo nextest`; no new dependencies.

**Spec:** `docs/internal/specs/2026-11-27-exit-names-design.md`

## Global Constraints

- No version constant moves: MO 4, MX 2, MT 2, `.tma` 0.5, `.tmc` 0.3, `.pmc` 0.4, PM-1 `.pma` 0.3, PM IR 4, TM IR 4, manifests 0.2 / 0.3.
- PM-1 output byte-identical; `crates/core` carries no TM-1 knowledge.
- The numeric `.routine` tail `exits=<K>` is removed, not kept beside the named form.
- New code comments cite durable pages only (`docs/formats.md (routine interfaces)`), never this plan or its spec; no tracker numbers in published pages.
- Gate cost: focused test binaries while working; one `cargo nextest run --workspace -j 4` at the end.
- No commits without the maintainer's word.

## Review Focus

1. A hand-written `.tma` still spelling `exits=2` — expected: a diagnostic at that line, never a silently exit-less routine. (Task 2)
2. Two exits with one name, or an exit named like one of the routine's `.param` tapes — expected: `BadSignature`, since the rendered header could not re-parse. (Task 2)
3. An object whose exit-name pool index is out of range — expected: `FormatError`, no panic (the never-panics-on-noise property). (Task 1)
4. A routine synthesized by the optimizer or the linker that takes exits — expected: it still carries distinct names. (Task 3)
5. Two `--extern` readings of one routine, object and text, in both orders, with a caller naming an exit — expected: same result, no order dependence. (Task 4)

---

### Task 1: The interface record carries names (core, wire)

**Files:**
- Modify: `crates/core/src/formats/object/mod.rs` (`RoutineInterface`), `read.rs`, `write.rs`, `tests.rs`
- Test: `crates/core/tests/format_roundtrips.rs`

**Interfaces:**
- Produces: `RoutineInterface { …, pub exits: Vec<String>, … }` — names in declaration order, `len() <= 255`.

- [ ] Change the round-trip generator in `format_roundtrips.rs` to produce 0..=3 named exits; add `interface_exit_names_round_trip` to `object/tests.rs` asserting `exits == ["done", "fail"]` after write → read.
- [ ] Wire: count byte as today, then one `u32` pool index per exit. Read resolves each through the same `name_of` the params use.
- [ ] Run `cargo test -p mtc-core --lib formats::` and `--test format_roundtrips`: green.

### Task 2: `.routine … exits=(a, b)` (core, assembly)

**Files:**
- Modify: `crates/core/src/asm/cst.rs` (`RoutineDirectiveCst::exits: Option<(Vec<SpannedName-like>, Span)>`, `routine_tail`), `lower.rs` (pending signature, `.param` requirement, distinctness), `disassembler.rs` (print), `assembler.rs`/`fmt.rs`/`views.rs` wherever the count is read
- Modify: `docs/formats.md` (sections and the routine signature; routine interfaces)
- Test: the unit tests beside each, `crates/core/tests/asm_interface.rs`, `link_exits.rs`, `link_checks.rs`, `link_tables.rs`

**Interfaces:**
- Consumes: `RoutineInterface::exits: Vec<String>`.
- Produces: the tail grammar `[, exits=(<name>, …)][, noreturn]`.

- [ ] Tests first: `exits=(done, fail), noreturn` lowers to `["done","fail"]`; `exits=()` alone is accepted without `.param` and is not printed back; `exits=(a)` without `.param` is `BadSignature("`exits=`/`noreturn` need `.param` lines")`; `exits=(a, a)` and an exit named like a `.param` are `BadSignature`; `exits=2` no longer assembles; 256 names is `BadSignature("exits must be 0..=255")` — keep the message; dis → asm byte-identity for a two-exit routine.
- [ ] Implement; rewrite every fixture that spells the numeric form.
- [ ] Run `cargo test -p mtc-core`: green.

### Task 3: The TM-1 pipeline emits and reads the names

**Files:**
- Modify: `crates/turing-machine/src/ir.rs` (`IrWorld::exits: Vec<String>`, serialized as a list, skipped when empty), `codegen.rs` (`emit_func`), `optimizer/inline.rs` and any pass that builds a world, `lint/tma/rules/retx_exit_bounds.rs`, `lsp/tma/descriptors.rs`, the `.tma` TextMate grammar under `editors/grammars/` if it knows the tail
- Modify: the IR page under `docs/tmt/` that lists `exits`
- Test: `crates/turing-machine/tests/state_params.rs`, `tma_dialect.rs`, `interface_emission.rs`

**Interfaces:**
- Consumes: Task 2's grammar.
- Produces: an object whose interface names its exits as the `.tmc` signature does.

- [ ] Test first, in `state_params.rs`: compiling `routine r(tape t: ab, state won, state lost)` yields an interface with `exits == ["won", "lost"]`.
- [ ] Implement; a synthesized world that takes exits copies or mints distinct names.
- [ ] Run the three test binaries plus `cargo test -p mtc-turing-machine --lib ir::`: green.

### Task 4: Delete the stopgap

**Files:**
- Modify: `crates/turing-machine/src/header.rs` (`from_object` prints the recorded names; delete `fresh_param_name`; drop the provenance parameter), `published.rs` (`ParamForm::State { name: String }`; delete both `agrees`; `own_forms`/`check_own_against_table` lose the parameter), `compiler.rs` (delete `Provenance`, `Resolved::from_object`, `analyze_with_provenance`), `declarations.rs` (delete `first_reading`; lookups take the first reading in table order), `head_flow.rs` and the three `name: None` sites in `compiler.rs`
- Modify: `docs/tmt/language.md` (declarations), `docs/tmt/cli.md` or wherever `tmt interface` on an object documents minted names
- Test: the pins in `published.rs`, `crates/turing-machine/tests/` that mention `exit0`

- [ ] Test first: `tmt interface` on a compiled object prints `state won, state lost`; a caller naming `won` compiles against the object reading alone, and against object + text in both `--extern` orders.
- [ ] Delete; convert each first-reading pin to an order-independence pin or remove it with the rule it pinned.
- [ ] `git grep -n -E 'Provenance::|from_object:|first_reading|fresh_param_name|fn agrees'` in `crates/turing-machine/src` returns nothing (`GraftProvenance` and `header::from_object` the function aside).
- [ ] Run `cargo test -p mtc-turing-machine`: green.

### Task 5: Release hygiene and the gate

- [ ] `CHANGELOG.md` `[Unreleased]`: a section for the change with the version block (every space `unchanged`), ref-free prose.
- [ ] `CLAUDE.md`: nothing in the versions table moves; correct any sentence that describes the numeric tail.
- [ ] Version audit: `git diff origin/next -- crates editors | grep -E '^[-+].*VERSION'` is empty; then the same audit of `origin/master..HEAD` against the CHANGELOG's version blocks, reporting any space that moved more than once per cycle or without a recorded reason.
- [ ] `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo build -p mtc-core --no-default-features`, `cargo build --workspace --lib --target wasm32-unknown-unknown`, `cargo nextest run --workspace -j 4`.
