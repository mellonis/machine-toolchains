# Volatile Tapes in TM (Plan 2 of the volatile/async/footprint round) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `volatile` as a reserved `.tmc` word with two declaration positions (machine tapes and signature tape parameters), flowing through CST → AST → resolve → expand → `IrTape.volatile`, pinned through `inline`/`outline`, stated as the per-band observability-barrier contract beside `brk`, with fmt/grammar/LSP/docs parity.

**Architecture:** The flag is born in the parser (a leading `volatile` before `tape` in both grammars), threads the existing tape chain (`TapeCst`/`SigParamKind::Tape` → AST → `ResolvedTape` → `ExpandedTape` → `IrTape`), and dies before the assembler — no pass gates on it today (all eight TM passes preserve per-band access sequences), so the optimizer work is contract prose plus propagation *pins*, not behavior. Grafts dissolve pre-IR, so a graph parameter's marker is deliberately inert (the host tape's declaration governs).

**Tech Stack:** Rust (edition 2024), `crates/turing-machine` only. Spec: `docs/superpowers/specs/2026-08-08-volatile-async-footprint-design.md` §2 + §4.1-4.2 (internal — never cite in code/docs; cite `docs/tmt/language.md (volatile tapes)` / `docs/tmt/optimizer.md (volatile barrier)`).

## Global Constraints

- **Version spaces do not move:** `TMC_LANG_VERSION` stays `"0.1"` and `TM_IR_VERSION` stays `2` — both unreleased; shapes amend in place per the round's sequencing ruling. The release-notes version block at the cut declares the final shapes.
- **No behavioral optimizer change.** No pass consults the flag this round; `-O0` bit-identity and the mode-equivalence matrix must be untouched.
- **PM-1 is entirely out of scope** (plan 3). `crates/post-machine` and `crates/core` see zero diff. The `.tma` dialect sees zero diff — volatility never reaches the assembler.
- **No new dependencies.**
- Comments and docs cite durable pages only; published docs forge-agnostic (no issue/PR refs, no spec §N).
- Commit style: conventional with scope (`feat(turing-machine):`, `test(turing-machine):`, `docs(tmt):`).
- **Gates for every task**, foreground, direct exit codes, no pipes, no background runs: `cargo test -p mtc-turing-machine`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo fmt --check`. The final task adds `cargo test --workspace` and `cargo build -p mtc-core --no-default-features`.
- **TDD honestly**: run the red stage and quote its output in the report. Where a task pins propagation (Tasks 4-5), ALSO run the stated mutation experiment and quote the failing output — a pin that survives its mutation is not done.

---

### Task 1: Reserve the word

**Files:**
- Modify: `crates/turing-machine/src/lexer.rs:15-48` (the `RESERVED` array + its doc comment + the keyword test battery below it)
- Modify: `editors/grammars/tmc.tmLanguage.json` (the `keywordsModifier` repository rule)
- Modify: `docs/tmt/language.md:767-777` (the "Reserved keywords" section)
- Modify: `crates/turing-machine/src/parser.rs:24-30` (the `TMC_LANG_VERSION` doc comment — one clause)
- Test: existing batteries — `crates/turing-machine/src/lexer.rs` inline tests, `crates/turing-machine/tests/editor_grammar.rs` (drift guard), plus one new parser test

**Interfaces:**
- Produces: `"volatile"` is a member of `mtc_turing_machine::lexer::RESERVED` (`[&str; 25]`); every later task relies on the parser's existing `ReservedName` rejection covering it for free (the parser is the sole enforcer — `parser.rs:935`).

- [ ] **Step 1: Probe for collisions** (expected clean — a prior probe found none):

```
grep -rn "volatile" crates/turing-machine/src/stdlib/std.tmc crates/turing-machine/tests/ docs/examples/ docs/tmt/
```

Expected: no hits where `volatile` is used as an identifier (doc-line prose is free text and fine). If a fixture uses it as a name, STOP and report — that fixture must be renamed first, in this task.

- [ ] **Step 2: Write the failing test** (in the parser's test module, next to the existing reserved-name tests — find them with `grep -n "ReservedName" crates/turing-machine/src/parser.rs` and copy the established test shape):

```rust
#[test]
fn volatile_is_reserved_as_a_name() {
    // A tape may not be NAMED volatile — the word is reserved.
    let err = parse_err("machine { tape volatile: bits; }");
    assert!(matches!(err.kind, CompileErrorKind::ReservedName { .. }));
}
```

(Use the file's existing helper for parse-error tests; if it is named differently than `parse_err`, adapt to the local helper — do not add a new one.)

- [ ] **Step 3: Run to verify failure** — `cargo test -p mtc-turing-machine volatile_is_reserved` → FAIL (the word is not yet reserved, so `volatile` parses as a legal tape name).

- [ ] **Step 4: Reserve it.** In `lexer.rs`: append `"volatile"` to the array, change the type to `[&str; 25]`, and update the doc comment's "The 24 fully-reserved" to 25. Update the lexer's keyword test battery (it pins that keywords lex as `Ident` — add `volatile` wherever the battery enumerates the set; if it iterates `RESERVED`, nothing to do). In `tmc.tmLanguage.json`: add `volatile` to the `keywordsModifier` rule's alternation (the drift guard asserts SET EQUALITY between all `keyword*` rules and `RESERVED`, so the grammar edit is forced). In `docs/tmt/language.md`: "Twenty-four words" → "Twenty-five words", and add `volatile` to the code block (keep the block's 8-per-row layout). In `parser.rs:24-30`: append one clause to the `TMC_LANG_VERSION` doc: `/// (An unreleased version amends in place; the bump discipline binds from the first release.)`

- [ ] **Step 5: Run the batteries** — `cargo test -p mtc-turing-machine --test editor_grammar` → PASS; `cargo test -p mtc-turing-machine volatile_is_reserved` → PASS; full `cargo test -p mtc-turing-machine` → PASS (nothing else regressed).

- [ ] **Step 6: Gates + commit**

```bash
git add crates/turing-machine/src/lexer.rs crates/turing-machine/src/parser.rs editors/grammars/tmc.tmLanguage.json docs/tmt/language.md
git commit -m "feat(turing-machine): reserve volatile — the 25th .tmc keyword"
```

---

### Task 2: Machine-tape grammar — `volatile tape NAME: ALPHABET;`

**Files:**
- Modify: `crates/turing-machine/src/cst.rs:240-252` (`TapeCst`)
- Modify: `crates/turing-machine/src/parser.rs` — the `world_body` dispatch (~:1617, the `at_kw("tape")` arm and the trailing `expected` message) and `parse_tape` (~:1634)
- Test: parser inline tests

**Interfaces:**
- Consumes: `RESERVED` membership from Task 1.
- Produces: `TapeCst` gains `pub volatile: bool`; `parse_tape` takes the leading token so the span covers the modifier:
  ```rust
  fn parse_tape(&mut self, volatile: bool, lead_tok: Token) -> Result<TapeCst, CompileError>
  ```
  Tasks 4 and 6 rely on `TapeCst.volatile` and on `span` starting at `volatile` when present.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn volatile_tape_parses_in_a_machine_block() {
    // Adapt to the file's CST-access helper; assert the flag and that the
    // tape's span starts at the `volatile` token, not at `tape`.
    let cst = parse_ok("machine { volatile tape sensor: bits; tape scratch: bits; }");
    /* find the two TapeCst items */
    assert!(sensor.volatile);
    assert!(!scratch.volatile);
}

#[test]
fn volatile_must_be_followed_by_tape_in_a_world_body() {
    let err = parse_err("machine { volatile state s { [*] -> stop; } }");
    /* expected-style parse error mentioning "`tape` after `volatile`" */
}

#[test]
fn volatile_tape_outside_a_machine_is_tape_not_in_machine() {
    let err = parse_err("routine r(tape t: bits) { volatile tape x: bits; entry state s { [*] -> stop; } }");
    assert!(matches!(err.kind, CompileErrorKind::TapeNotInMachine));
}
```

- [ ] **Step 2: Run to verify failure** — the first fails to compile (`volatile` field missing); run and quote.

- [ ] **Step 3: Implement.** In `cst.rs`, add the field with a doc line:

```rust
pub struct TapeCst {
    pub name: String,
    pub name_span: Span,
    pub alphabet: String,
    pub alphabet_span: Span,
    /// `volatile tape …` — the band is a device (docs/tmt/language.md
    /// (volatile tapes)).
    pub volatile: bool,
    pub line: u32,
    /// First token (`volatile` or `tape`) start → `;` end.
    pub span: Span,
    pub trailing: Option<Comment>,
}
```

In `world_body`'s dispatch chain, add an arm BEFORE the `at_kw("tape")` arm:

```rust
} else if self.at_kw("volatile") {
    let lead = self.peek().clone();
    self.bump(); // `volatile`
    if !self.at_kw("tape") {
        return Err(Self::expected(self.peek(), "`tape` after `volatile`"));
    }
    if in_machine {
        WorldKind::Tape(self.parse_tape(true, lead)?)
    } else {
        return Err(Self::err_at(&t, CompileErrorKind::TapeNotInMachine));
    }
} else if self.at_kw("tape") {
    if in_machine {
        let lead = self.peek().clone();
        WorldKind::Tape(self.parse_tape(false, lead)?)
    } else {
        return Err(Self::err_at(&t, CompileErrorKind::TapeNotInMachine));
    }
}
```

Rework `parse_tape` to the new signature: it no longer clones its own lead token; `line: lead_tok.line`, `span: join(lead_tok.span(), semi.span())`, `volatile` stored. Update the trailing `expected` message in `world_body` to `"a tape declaration, `state`, `graft`, or `bind`"` → include `volatile`? No — a volatile tape IS "a tape declaration"; leave the message as is.

Check the CST↔fmt lossless contract: `cst.rs`'s `WorldKind::Tape` docstring says "grammatical only in a `machine` block" — extend it to mention the optional modifier. If `lower_cst` copies `TapeCst` fields into the AST machine-tape type, the compiler stops building until Task 4's threading — to keep THIS task green, thread the flag only as far as it must go to compile: follow the compiler errors and add `volatile: t.volatile` / `volatile: false` initializers WITHOUT any semantic use (Task 4 owns resolve/expand/IR). List every such mechanical touch in your report.

- [ ] **Step 4: Run tests** — the three new tests PASS; full crate suite PASS.
- [ ] **Step 5: Gates + commit**

```bash
git add crates/turing-machine/src/cst.rs crates/turing-machine/src/parser.rs
git commit -m "feat(turing-machine): volatile modifier on machine tape declarations"
```

(Include any files the mechanical threading touched.)

---

### Task 3: Signature grammar — `(volatile tape T: ALPHA, …)`

**Files:**
- Modify: `crates/turing-machine/src/parser.rs:139-154` (`SigParamKind`), `sig_param` (~:1515)
- Test: parser inline tests

**Interfaces:**
- Consumes: Task 1's reservation.
- Produces: `SigParamKind::Tape { alphabet, alphabet_span, volatile: bool }` — Task 4 matches on the widened variant; Task 6's fmt prints it.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn volatile_tape_parameter_parses() {
    let cst = parse_ok("routine r(volatile tape t: bits, tape u: bits, state done) { entry state s { [*] -> done; } }");
    /* assert params[0] is Tape{volatile: true}, params[1] Tape{volatile: false} */
}

#[test]
fn volatile_state_parameter_is_an_error() {
    let err = parse_err("routine r(volatile state done) { entry state s { [*] -> done; } }");
    /* expected-style error mentioning "`tape` after `volatile` (only tape parameters can be volatile)" */
}
```

- [ ] **Step 2: Run to verify failure** — compile FAIL on the widened variant; quote.
- [ ] **Step 3: Implement.** In `sig_param`, add a leading branch:

```rust
fn sig_param(&mut self) -> Result<SigParam, CompileError> {
    let t = self.peek().clone();
    let volatile = if self.at_kw("volatile") {
        self.bump();
        if !self.at_kw("tape") {
            return Err(Self::expected(
                self.peek(),
                "`tape` after `volatile` (only tape parameters can be volatile)",
            ));
        }
        true
    } else {
        false
    };
    if self.at_kw("tape") {
        /* existing body; SigParamKind::Tape { alphabet, alphabet_span, volatile },
           span: join(t.span(), alphabet_span)  — `t` is the `volatile` token when
           present, so the span covers the modifier */
    } else if self.at_kw("state") {
        /* existing body unchanged */
    } else {
        Err(Self::expected(&t, "a `tape` or `state` signature parameter"))
    }
}
```

Thread the widened variant mechanically wherever it stops compiling (compiler.rs:1519's match gains `volatile, ..` capture but USES nothing yet — Task 4 owns semantics; LSP/fmt sites take `volatile: false`-style adaptations only if the compiler forces them). List every mechanical touch in the report.

- [ ] **Step 4: Run tests**; **Step 5: Gates + commit**

```bash
git commit -m "feat(turing-machine): volatile modifier on signature tape parameters"
```

---

### Task 4: Thread to IR — `IrTape.volatile`, graft inertness, `tmt ir`

**Files:**
- Modify: `crates/turing-machine/src/compiler.rs:842-850` (`ResolvedTape` + both construction sites :1516-1530 sig-params, :1605-1616 machine), and the AST machine-tape link (`lower_cst`'s tape lowering — the struct `m.tapes` elements; trace it from `resolve_machine_world`'s `for t in &m.tapes`)
- Modify: `crates/turing-machine/src/expand.rs:70` (`ExpandedTape` + its construction from `ResolvedTape`)
- Modify: `crates/turing-machine/src/ir.rs:109-113` (`IrTape`), :522-530 (the lowering map)
- Test: ir.rs inline serde tests + one CLI-level test in `crates/turing-machine/tests/cli_programs.rs`

**Interfaces:**
- Consumes: `TapeCst.volatile` (Task 2), `SigParamKind::Tape{volatile}` (Task 3).
- Produces:
  ```rust
  pub struct IrTape {
      pub name: String,
      pub alphabet: String,
      pub cardinality: u32,
      #[serde(default, skip_serializing_if = "is_false")]
      pub volatile: bool,
  }
  fn is_false(b: &bool) -> bool { !*b }   // module-private helper beside IrTape
  ```
  Task 5 reads `IrWorld.tapes[k].volatile`; Task 7's end-to-end test reads the `tmt ir` JSON.

- [ ] **Step 1: Write the failing tests**

In `ir.rs`'s test module (beside `serde_tags_are_frozen`, ir.rs:1125):

```rust
#[test]
fn volatile_tape_serializes_only_when_set() {
    let mut tape = IrTape {
        name: "t".into(), alphabet: "al".into(), cardinality: 3, volatile: false,
    };
    let json = serde_json::to_string(&tape).unwrap();
    assert!(!json.contains("volatile"), "false is omitted: {json}");
    tape.volatile = true;
    let json = serde_json::to_string(&tape).unwrap();
    assert!(json.contains("\"volatile\":true"), "{json}");
    // absent field deserializes to false
    let back: IrTape = serde_json::from_str(r#"{"name":"t","alphabet":"al","cardinality":3}"#).unwrap();
    assert!(!back.volatile);
}
```

End-to-end compile tests (place beside similar compile-to-IR tests — grep `cli_programs.rs` or the compiler test module for an existing "compile source, inspect IR" helper and use its shape):

```rust
#[test]
fn machine_tape_volatility_reaches_the_ir() {
    // machine with one volatile and one plain tape; the routine takes a
    // volatile param → its world's tape is flagged.
    let src = r#"
        alphabet bits { '_', '1' }
        export routine probe(volatile tape s: bits) {
            entry state p { [*] -> return; }
        }
        machine {
            volatile tape sensor: bits;
            tape scratch: bits;
            entry state go { [*, *] -> call probe(s = sensor) then stop; }
        }
    "#;
    let ir = /* compile via the established path, take CompileOutput's IR */;
    let main = /* world "main" */;
    assert!(main.tapes[0].volatile && !main.tapes[1].volatile);
    let probe = /* world "probe" (mangled name — match by suffix) */;
    assert!(probe.tapes[0].volatile);
}

#[test]
fn graft_drops_the_graph_params_volatility_host_governs() {
    // A graph declares its param volatile; grafted onto a PLAIN host tape,
    // the host world's tape stays non-volatile (grafts dissolve pre-IR;
    // the host's declaration describes the real band).
    let src = r#"
        alphabet bits { '_', '1' }
        graph g(volatile tape t: bits, state done) {
            entry state w { [*] -> done; }
        }
        machine {
            tape plain: bits;
            entry graft g(t = plain, done = stop);
        }
    "#;
    let ir = /* compile */;
    assert!(!/* main world */.tapes[0].volatile);
}
```

(Adapt `.tmc` snippets to the real grammar as needed — verify each compiles; if a snippet needs adjustment, keep the scenario and note the change.)

- [ ] **Step 2: Run to verify failure** — compile FAIL (`volatile` fields missing); quote.
- [ ] **Step 3: Implement the chain.** `ResolvedTape` gains `pub volatile: bool`; the sig-param site reads the Task-3 variant field; the machine site reads the AST tape's flag (thread the AST link from Task 2's mechanical pass — now give it semantics). `ExpandedTape` gains the field, copied from `ResolvedTape` wherever expanded worlds are built. The ir.rs:522 map adds `volatile: t.volatile`. Graft splicing (`expand.rs`) must NOT read the graph's own param volatility anywhere — verify by inspection that the splice path only touches host tapes, and say so in the report (that is what makes the second test pass with no extra code).
- [ ] **Step 4: Run tests** — all new PASS; `serde_tags_are_frozen` UNCHANGED and green (false is omitted, so the frozen JSON is untouched — if it fails, you broke the skip attribute).
- [ ] **Step 5: Mutation experiment** (required): temporarily make the machine construction site hardcode `volatile: false`, confirm `machine_tape_volatility_reaches_the_ir` FAILS, restore, re-run green. Quote the failure.
- [ ] **Step 6: Gates + commit**

```bash
git commit -m "feat(turing-machine): IrTape.volatile — the flag reaches the IR, grafts stay host-governed"
```

---

### Task 5: Optimizer — propagation pins and the barrier contract

**Files:**
- Modify: `crates/turing-machine/src/optimizer/mod.rs` (the module-doc equivalence-contract block — locate the `brk` barrier sentence)
- Test: `crates/turing-machine/src/optimizer/outline.rs` + `inline.rs` inline test modules

**Interfaces:**
- Consumes: `IrTape.volatile` (Task 4).
- Produces: the contract text later plans and passes cite; no behavior.

- [ ] **Step 1: Write the failing pins**

In `outline.rs`'s test module (its tests build `IrProgram`s directly — copy the local builder shape; `outline.rs:450` clones the host's tapes into the synthesized world, so this pins that the clone keeps carrying the flag):

```rust
#[test]
fn outlined_worlds_inherit_the_hosts_tape_volatility() {
    let mut program = /* the module's existing outline-firing fixture, with
                         the host world's tapes[0].volatile = true */;
    let changed = run(&mut program);
    assert!(changed > 0, "premise: outline must fire on this fixture");
    let synthesized = program.worlds.iter().find(|w| w.name.contains(".outline")).unwrap();
    assert!(synthesized.tapes[0].volatile);
}
```

In `inline.rs`'s test module (bindless splice — caller tapes are untouched by construction; the pin keeps it that way):

```rust
#[test]
fn inline_keeps_the_callers_tape_volatility() {
    let mut program = /* the module's existing inline-firing fixture, caller
                         world tapes[0].volatile = true */;
    let changed = run(&mut program);
    assert!(changed > 0, "premise: inline must fire on this fixture");
    assert!(program.worlds[/* caller */].tapes[0].volatile);
}
```

- [ ] **Step 2: Run** — both should PASS immediately (propagation is structural). That is expected — these are pins, not red-stage tests; the red stage is Step 3's mutation.
- [ ] **Step 3: Mutation experiments** (required, both): (a) in `outline.rs:450`, replace `tapes: w.tapes.clone()` with a map that rebuilds each `IrTape` with `volatile: false` — the outline pin MUST FAIL; restore. (b) in `inline.rs`, if any code path rebuilds caller tapes, mutate it likewise; if none exists (pure splice, tapes never touched), state that in the report — the pin then guards future refactors and (b) is vacuously strong. Quote failure output for (a).
- [ ] **Step 4: The contract paragraph.** In `optimizer/mod.rs`'s module doc, immediately after the existing `brk`-barrier sentence, add:

```rust
//! A volatile band (docs/tmt/language.md (volatile tapes)) generalizes the
//! `brk` barrier from a point to a standing, per-band rule: every access to
//! a volatile band is externally observable, and the external world may
//! change the band's cells between accesses. No pass may assume a value
//! read from or written to a volatile band persists, and no pass may change
//! the band's access sequence — no dropping idempotent or dead writes, no
//! fusing or splitting write+move shapes, no value propagation through its
//! reads. Today every pass in this pipeline preserves per-band access
//! sequences (`dead-rows` removes only rows that never fire, so the dynamic
//! sequence is unchanged), so nothing gates on the flag; any future pass
//! that reasons about values or motion must consult `IrTape::volatile`
//! (docs/tmt/optimizer.md (volatile barrier)).
```

- [ ] **Step 5: Gates + commit**

```bash
git commit -m "test(turing-machine): pin volatile propagation through inline/outline; state the per-band barrier"
```

---

### Task 6: fmt — canonical printing in both positions

**Files:**
- Modify: `crates/turing-machine/src/fmt.rs:1585-1590` (`render_tape`), :719 (the signature-param rendering)
- Test: `crates/turing-machine/tests/fmt_tmc.rs` (idempotence + fixtures)

**Interfaces:**
- Consumes: `TapeCst.volatile`, `SigParamKind::Tape{volatile}`.
- Produces: canonical text `volatile tape NAME: ALPHA;` / `(volatile tape T: ALPHA)`.

- [ ] **Step 1: Write the failing tests** (follow `fmt_tmc.rs`'s existing fixture shape — source in, canonical text out, idempotence):

```rust
#[test]
fn volatile_tape_declarations_format_canonically() {
    // Mixed run: name padding stays name-based; the modifier simply
    // prefixes its own line (a mixed volatile/plain run does not
    // column-align across the modifier).
    let src = "machine{volatile   tape  sensor:bits;\ntape scratch : bits;\n /* … a minimal complete machine … */}";
    /* assert formatted output contains "volatile tape sensor: bits;" and
       "tape scratch:  bits;" per the run-padding rules; assert
       format(format(src)) == format(src) */
}

#[test]
fn volatile_signature_params_format_canonically() {
    /* routine with (volatile   tape t:bits) → "(volatile tape t: bits)";
       idempotence */
}
```

- [ ] **Step 2: Run to verify failure** — the formatter currently DROPS the modifier (it prints from the CST fields it knows); the assert on `volatile tape` fails. Quote. (If Task 2's mechanical threading already made fmt print it, these pass immediately — then the red stage is a mutation: remove the prefix, watch it fail, restore.)
- [ ] **Step 3: Implement.** `render_tape` (:1585): prefix `volatile ` when `t.volatile` (before the padded-name `format!`; the name-width run logic at :1557 stays name-based — document the alignment choice in a one-line comment). The sig-param line (:719): `format!("{}tape {}: {alphabet}", if volatile { "volatile " } else { "" }, param.name)` adapted to the local code shape.
- [ ] **Step 4: Run** — new tests PASS; the whole `fmt_tmc.rs` + `fmt_programs.rs`/`fmt_interior.rs` suites PASS; **the compiled-stdlib byte-identity check** (locate it — the fmt/stdlib gate test) PASS (trivially: the stdlib contains no `volatile`).
- [ ] **Step 5: Gates + commit**

```bash
git commit -m "feat(turing-machine): fmt prints the volatile modifier in both declaration positions"
```

---

### Task 7: LSP completions, docs, end-to-end, final gates

**Files:**
- Modify: `crates/turing-machine/src/lsp/complete.rs:59` area (world-body keyword words) and the signature-param completion context (grep `complete.rs`/`context.rs` for where `"tape"`/`"state"` are offered inside a signature)
- Modify: `docs/tmt/language.md` (tape declarations §, signatures §, new semantics §), `docs/tmt/optimizer.md` (the contracts section)
- Test: LSP inline tests (`lsp/tests.rs` or the completion tests' home), one end-to-end test in `crates/turing-machine/tests/cli_programs.rs`

**Interfaces:** consumes everything prior; produces the user-facing surface.

- [ ] **Step 1: LSP completions.** At `complete.rs:59` (`words.push("tape")`) — determine from the surrounding context classifier whether this site is machine-body-only; if yes, also `words.push("volatile");` there; if the site serves all world bodies, gate `volatile` on the machine context the classifier exposes. In the signature-param context (where `tape`/`state` are offered), add `volatile`. Write the covering completion tests in the established harness (grep for an existing test asserting `"tape"` appears in completions and mirror it for `volatile` in both contexts, plus a negative: `volatile` NOT offered inside a routine world body).
- [ ] **Step 2: Run LSP tests** — red first if the harness compiles (the completion lists lack the word), then green after the edit; quote.
- [ ] **Step 3: docs/tmt/language.md.** Three edits, in the page's voice, forge-agnostic:
  1. The tape-declaration section (~:195): the modifier's syntax + one paragraph of semantics: a volatile tape is a **device band** — every access to it is externally observable, and the external world may change its cells between accesses; the toolchain preserves the band's exact access sequence, and each read is a fresh observation.
  2. The signatures section: `volatile tape T: ALPHA` — the routine is compiled treating that band as volatile. Then the asymmetry + foot-gun paragraph: **grafts dissolve before optimization**, so spliced rows live on the host's tapes and the host declaration governs (a graph's `volatile` parameter is accepted and inert — correct, because the host's declaration describes the band the code actually runs on); **calls and binds are separate-compilation boundaries and are not checked** — the author calls the routine variant compiled for the right kind of band; binding a volatile machine tape into a routine compiled without `volatile` is not diagnosed, by design.
  3. The reserved table already updated in Task 1 — verify the count and block render.
- [ ] **Step 4: docs/tmt/optimizer.md.** In the contracts section, beside the `brk` barrier: the same per-band barrier paragraph as Task 5's module doc (page prose, not a code quote), plus one sentence that `inline` splices onto the caller's tapes and `outline`'s synthesized worlds mirror the host's tape declarations including volatility — so the flag survives both program-level passes.
- [ ] **Step 5: End-to-end pipeline test** (the round's cross-task checklist item — one test that no single earlier task owns), in `cli_programs.rs`:

```rust
#[test]
fn volatile_survives_the_whole_pipeline() {
    // source → fmt (idempotent, modifier kept) → compile → ir JSON carries
    // "volatile":true → -O1 optimize → still carried (worlds may be
    // renumbered; find the machine world) → .tma output contains NO trace
    // of the word (volatility never reaches the assembler).
    /* drive tmt-level helpers already used by this file: fmt, compile with
       --emit-ir/-S equivalents or the library API */
    assert!(ir_json.contains("\"volatile\":true"));
    assert!(!tma_text.contains("volatile"));
}
```

- [ ] **Step 6: Docs verification.** Replay every syntax snippet added to the two pages through the real toolchain (compile the examples; run `tmt fmt` on them and confirm the canonical form matches what the page shows). The cli_docs guard does not cover these pages, so this manual replay is the gate.
- [ ] **Step 7: Full final gates** — `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo fmt --check`, `cargo build -p mtc-core --no-default-features`. All direct exit codes.
- [ ] **Step 8: Commit**

```bash
git add crates/turing-machine/src/lsp/ docs/tmt/language.md docs/tmt/optimizer.md crates/turing-machine/tests/cli_programs.rs
git commit -m "feat(turing-machine): volatile completions, language/optimizer docs, pipeline proof"
```
