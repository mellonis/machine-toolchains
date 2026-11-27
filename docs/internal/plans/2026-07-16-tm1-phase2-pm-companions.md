# TM-1 Phase 2: PM companions — wrl/wrr fusion + tape authoring

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The two PM-side companion features of the TM-1 arc (spec §3.6, §13):
(A) fused write+move opcodes `wrl` (0x07) / `wrr` (0x0F) surfaced as a `-O1`
peephole pass `fuse_tape_ops`, bumping the `.pma` dialect 0.2→0.3 and
`IR_VERSION`; (B) the tape-authoring CLI surface `pmt tape new --from` /
`pmt tape set` (clone semantics) beside the existing `tape build`/`show`.

**Architecture:** Everything lives in `crates/post-machine` except the two
grammar files under `editors/` and docs. Fusion design: two new `IrOp`
variants (`WrLft`/`WrRgt`) produced ONLY by the optimizer pass — codegen
gains emit arms but performs no fusion itself, so `-O0` output stays
bit-identical to plain codegen (the standing contract). Every optimizer pass
that matches on `IrOp` handles the fused variants conservatively (they are
simultaneously a write and a move for every analysis). The pass runs LAST in
the per-function pipeline. Authoring commands reuse the MT codec + existing
CLI conventions (thin renderer, `Args` scanner, `sniff`).

**Tech Stack:** Rust; no new dependencies.

## Global Constraints

- **`-O0` bit-identity**: `-O0` output must stay byte-identical to plain
  codegen — fused ops are never produced at `-O0` (`fuse_tape_ops` is a
  registered `-O1` pass, skippable via `--fno-fuse-tape-ops`).
- **Equivalence contract** (`tests/opt_equivalence.rs` is the gate): passes
  preserve final tape, termination kind, and MF-dependent branches; step
  counts and resource-limit outcomes may change; **no motion across an
  un-stripped `brk`** — the fusion pass must never fuse a pair separated by
  `Brk` (adjacency in one block's `ops` already implies nothing sits
  between; do not relax that).
- Golden `.pmt` files pin final tapes of `-O1` runs: fusion must not change
  any final tape (`golden_programs` gate). `cargo test --workspace` green at
  the end of every task.
- `cargo clippy --workspace --all-targets -- -D warnings` and
  `cargo fmt --check` clean before every commit.
- MF semantics: PM-1 tape ops end with `LatchMatch(MARK)`; the fused ops
  lower to `[Write{dev:0,index}, Move*{dev:0}, LatchMatch(MARK)]` — final MF
  = cell at head AFTER the move, exactly as the unfused `wr x; lft` pair.
- Version spaces this phase moves: `.pma` dialect `PM1_PMA_DIALECT_VERSION`
  "0.2"→"0.3" (new mnemonics = acceptance change); `IR_VERSION` +1 (new op
  variants in the JSON artifact). The toolchain version is NOT the carrier.
- Published docs (docs/*.md) stay forge-agnostic; no spec-§ refs in code
  comments (CLAUDE.md documentation-authority rule).
- Commit style: conventional with scope. NEVER add any Claude/AI attribution
  footer. Commits require the maintainer's explicit go-ahead in the
  executing session.

---

### Task 1: The wrl/wrr opcodes in the arch module

**Files:**
- Modify: `crates/post-machine/src/arch/mod.rs` (consts ~6-25, `operand_kind` ~43-51, `lower` ~53-104, tests incl. `operand_kind_table_matches_spec` ~140)
- Test: inline `mod tests`

**Interfaces:**
- Produces: `pub const WRL: u8 = 0x07; pub const WRR: u8 = 0x0F;` — opcodes
  with `OperandKind::SymbolVec` (single symbol, like `WR`), lowering:
  - `WRL` → `vec![MicroOp::Write { dev: 0, index }, MicroOp::MoveLeft { dev: 0 }, MicroOp::LatchMatch(MARK)]`
  - `WRR` → same with `MoveRight`
  (semantically ≡ `wr x; lft`/`wr x; rgt`: one fetch instead of two and one
  fewer intermediate latch read; final MF identical.) Bad operand (not a
  1-element symbol vec) → same error path as `WR`.

- [ ] **Step 1: Write the failing tests**

```rust
    #[test]
    fn wrl_wrr_lower_to_write_move_latch() {
        let arch = Pm1Arch;
        let one = Operand::Symbols(vec![1]);
        assert_eq!(
            arch.lower(WRL, &one).unwrap(),
            vec![
                MicroOp::Write { dev: 0, index: 1 },
                MicroOp::MoveLeft { dev: 0 },
                MicroOp::LatchMatch(MARK),
            ]
        );
        assert_eq!(
            arch.lower(WRR, &one).unwrap(),
            vec![
                MicroOp::Write { dev: 0, index: 1 },
                MicroOp::MoveRight { dev: 0 },
                MicroOp::LatchMatch(MARK),
            ]
        );
    }

    /// The fused pair must be observably equivalent to the unfused pair:
    /// run `wr 1; lft; stp` and `wrl 1; stp` on identical tapes and compare
    /// final tape + head + MF.
    #[test]
    fn fused_equals_unfused_pair() {
        // Build two raw code images (ENT-prefixed as the arch requires if
        // executed via call, else straight code) and drive them through the
        // driver as the existing arch tests do; assert equal final
        // snapshots and equal core.mf().
        // (Mirror the driving style of the existing lowerings test in this
        // module / pm1_programs.rs — hand-assembled byte vecs.)
        todo_replace_with_real_driver_harness();
    }
```

For the second test, copy the harness shape used by
`crates/post-machine/tests/pm1_programs.rs` (hand-assembled `vec![...]` +
`Machine`/driver run + final-tape compare). If module-level tests lack a
convenient harness, put this test in `tests/pm1_programs.rs` instead — it is
the natural home; then the module keeps only the lowering-shape test.

- [ ] **Step 2: Verify they fail** (`WRL` unresolved).

- [ ] **Step 3: Implement** — add the consts, `operand_kind` arm
(`WRL | WRR => Some(OperandKind::SymbolVec)`), `lower` arms mirroring `WR`'s
operand validation, and UPDATE `operand_kind_table_matches_spec` (it
currently asserts 0x07/0x0F invalid — flip those to the new expectations).

- [ ] **Step 4:** `cargo test --workspace` — all green.

- [ ] **Step 5: Commit**

```bash
git add crates/post-machine
git commit -m "feat(post-machine): wrl/wrr fused write+move opcodes"
```

---

### Task 2: Assembler syntax, dialect 0.3, editor grammars, isa docs

**Files:**
- Modify: `crates/post-machine/src/asm/mod.rs` (`pm1_syntax()` entries; `PM1_PMA_DIALECT_VERSION` "0.2"→"0.3" at ~line 14)
- Modify: `crates/post-machine/tests/cli_programs.rs` (~line 52 pins the dialect string — update to "0.3")
- Modify: `editors/grammars/pma.tmLanguage.json` (~line 35 mnemonic alternation) and `editors/vscode/syntaxes/pma.tmLanguage.json` (the duplicated pattern) — add `wrl|wrr` (do NOT touch the jetbrains build/ artifact — it's generated)
- Modify: `docs/isa.md` (opcode table: 0x07 row `reserved` → `wrl`, add 0x0F `wrr` row; fix the "17 real entries" count to 19 and the 0x07-in-invalid-list prose ~163-164 and ~214; add a sentence in the timing section: fused ops cost one fetch and skip the intermediate latch read)
- Test: `tests/editor_grammar.rs` (auto-derives the required mnemonic set from `pm1_syntax()` — goes green once the grammar files list wrl/wrr)

**Interfaces:**
- Produces: `SyntaxEntry { opcode: WRL, mnemonic: "wrl", operand: SymbolVec, flow: FallThrough }` (+ wrr). No relaxation pairs (no short forms). Disassembler/LSP/lint/fmt all derive from `pm1_syntax()` — no other tables exist (verified by exploration).

- [ ] **Step 1: Write the failing test** (asm→dis round-trip; place beside existing asm round-trip tests — grep `pmt asm` round-trip in tests/)

```rust
    /// wrl/wrr assemble and disassemble by name.
    #[test]
    fn wrl_wrr_round_trip_through_asm_and_dis() {
        // .pma source with wrl 1 / wrr 0 inside a .func, assembled via the
        // core assembler with pm1_syntax(), then disassembled; assert the
        // listing contains "wrl 1" and "wrr 0".
    }
```

(Concrete harness: mirror whichever existing test assembles a `.pma` string
and disassembles it — e.g. in `cli_programs.rs` or core asm tests with
`pm1_syntax()`. Write real code, matching that file's helpers.)

- [ ] **Step 2: Verify it fails** (unknown mnemonic `wrl`).

- [ ] **Step 3: Implement** — the two `SyntaxEntry`s, the dialect bump, the
grammar-file alternations, the cli_programs pin update, the isa.md edits.

- [ ] **Step 4:** `cargo test --workspace` (editor_grammar + cli_programs
green) + clippy + fmt.

- [ ] **Step 5: Commit**

```bash
git add crates/post-machine editors/grammars editors/vscode docs/isa.md
git commit -m "feat(post-machine): pma dialect 0.3 — wrl/wrr mnemonics across the toolchain surface"
```

---

### Task 3: IR variants + codegen emit + IR_VERSION bump

**Files:**
- Modify: `crates/post-machine/src/ir.rs` (`IrOp` enum ~50-58; `IR_VERSION` const; the IR JSON encode/decode of ops)
- Modify: `crates/post-machine/src/codegen.rs` (emit arms ~119-131)
- Modify: every optimizer pass with an exhaustive `IrOp` match — the compiler
  enumerates them; expected: `cell_state.rs` (~52), `dataflow.rs` (~57),
  `inline.rs`, `tail_merge.rs` (~15), possibly check_fold/branch_fold/dce
- Modify: `docs/language.md` (the "IR artifact" section — note the new ops + version bump)
- Test: inline ir tests + `pmt ir` snapshot if one exists

**Interfaces:**
- Produces: `IrOp::WrLft { index: u32, line: u32 }` and
  `IrOp::WrRgt { index: u32, line: u32 }` (field shapes mirroring `Wr`);
  produced ONLY by the fusion pass (task 4) — codegen emits `wrl <index>` /
  `wrr <index>` grid lines for them; `-O0` never sees them. `IR_VERSION`
  incremented by 1 (new variants change the JSON artifact's op vocabulary).

**Conservative semantics rule for every pass arm** (this is the correctness
core — apply uniformly): a fused op IS simultaneously a write of `index` to
the pre-move cell AND a head move AND an MF latch. Wherever a pass treats
`Wr` specially (dead-write windows, known-cell facts) and wherever it treats
`Lft`/`Rgt` as a barrier/window-ender, the fused ops must take the UNION of
both behaviors — when in doubt, the barrier/window-ender treatment (fully
conservative) is always sound. `tail_merge`'s pairwise op equality works
structurally (derived PartialEq) — no special arm needed beyond compilation.

- [ ] **Step 1: Write the failing test**

```rust
    /// Fused IR ops encode into the JSON artifact and codegen emits the
    /// fused mnemonics.
    #[test]
    fn fused_ops_encode_and_emit() {
        // Build a tiny IrFunction whose block ops include
        // IrOp::WrLft { index: 1, line: 1 } and IrOp::WrRgt { index: 0, line: 2 };
        // assert the IR JSON round-trips (if ir.rs has encode/decode tests,
        // mirror them) and that codegen's listing contains "wrl 1" and "wrr 0".
    }
```

- [ ] **Step 2: Verify it fails** (no variants).

- [ ] **Step 3: Implement** — variants, IR JSON encode/decode arms,
`IR_VERSION` +1, codegen arms
(`IrOp::WrLft { index, .. } => grid(None, "wrl", &index.to_string())`, same
for wrr), then `cargo build --workspace` and add the conservative arms in
every pass the compiler names. Update docs/language.md's IR section.

- [ ] **Step 4:** `cargo test --workspace` + gates. (Note: `pmt ir`
snapshots or IR-shape tests may pin the old IR_VERSION — update them.)

- [ ] **Step 5: Commit**

```bash
git add crates/post-machine docs/language.md
git commit -m "feat(post-machine): fused tape ops in the IR — WrLft/WrRgt, IR version bump"
```

---

### Task 4: The fuse_tape_ops pass

**Files:**
- Create: `crates/post-machine/src/optimizer/fuse_tape_ops.rs`
- Modify: `crates/post-machine/src/optimizer/mod.rs` (add `pub mod fuse_tape_ops;` ~34; append `("fuse-tape-ops", fuse_tape_ops::run)` LAST in `PIPELINE` ~80-88)
- Test: inline tests in the new pass file + the drift guards (completions registry picks the name up automatically via `pass_names()`)

**Interfaces:**
- Produces: `pub fn run(f: &mut IrFunction) -> u32` (the `PassFn` shape,
  like `dce::run`): for each block, scan `ops` left to right; replace each
  adjacent pair `[Wr { index, line }, Lft { .. }]` with
  `[WrLft { index, line }]` (and `Rgt` → `WrRgt`), keeping the WRITE's
  `line` (the fused instruction maps to the source line that wrote). Return
  the number of fusions. Never fuses across anything — only immediately
  adjacent pairs in one block's `ops` (a `Brk` between them breaks
  adjacency by construction; do not add lookahead past other ops).
  Idempotent (fused ops never re-match).

- [ ] **Step 1: Write the failing tests** (in the pass file)

```rust
    #[test]
    fn fuses_adjacent_write_move_pairs() { /* [Wr, Lft, Wr, Rgt] -> [WrLft, WrRgt]; returns 2 */ }

    #[test]
    fn brk_between_blocks_fusion() { /* [Wr, Brk, Lft] unchanged; returns 0 */ }

    #[test]
    fn lone_ops_untouched() { /* [Wr], [Lft], [Rgt, Wr] (move BEFORE write) unchanged */ }
```

(Write real bodies — construct minimal `IrFunction`/`IrBlock` values the way
sibling pass tests do; copy their builder helpers.)

- [ ] **Step 2: Verify they fail** (module missing).

- [ ] **Step 3: Implement** the scan-and-splice (a simple index walk
building a new `Vec<IrOp>` is fine), register the pass LAST in `PIPELINE`.

- [ ] **Step 4:** `cargo test --workspace` — the key gates:
`opt_equivalence` (O0 vs O1 observables — fusion changes step counts, which
the contract allows, but must preserve final tape/termination/MF branches),
`golden_programs` (final tapes byte-identical), `completions_registry`
(`--fno-fuse-tape-ops` appears and matches `pass_names()` exactly — its
drift test goes green automatically; if the registry hardcodes a pass list
anywhere, the drift test names it). Plus clippy/fmt.

- [ ] **Step 5: Commit**

```bash
git add crates/post-machine
git commit -m "feat(post-machine): fuse_tape_ops -O1 peephole — adjacent wr+move pairs become wrl/wrr"
```

---

### Task 5: pmt tape new --from / tape set

**Files:**
- Modify: `crates/post-machine/src/cli/inspect.rs` (extend `tape()` dispatch ~101-107 with `new` and `set`; new fns `tape_new`, `tape_set`; extend `TAPE_USAGE`)
- Modify: `crates/post-machine/src/cli/mod.rs` (top-level `USAGE` tape line ~44)
- Modify: `crates/post-machine/src/completions/registry.rs` (add `tape_new_spec()` / `tape_set_spec()` modeled on `tape_build_spec()` ~373-390; register in `registry()` ~541; glosses in `group_child_help` ~497)
- Test: `crates/post-machine/tests/cli_programs.rs` (integration, mirroring existing tape build/show tests)

**Interfaces / behavior (spec §13 adapted to the existing pmt surface):**
- `pmt tape new --from prog.pmx [-o blank.pmt]` — loads the executable
  (`fs::read` + `sniff` + `Executable::from_bytes`, the run.rs/inspect.rs
  pattern; reject non-Executable containers), builds a template
  `TapeBlockFile`: PM-1 default glyph alphabet (the same `DEFAULT_GLYPHS`
  the `tape build` path uses), ONE tape (`origin 0, cells: vec![], head 0,
  alphabet: None`), writes to `-o` (default `blank.pmt`). For a v2
  executable input (tape_count > 1), emit that many tapes. Output goes
  through `CliOutput` (thin renderer).
- `pmt tape set in.pmt -o out.pmt [--tape N] [--cells "PATTERN"] [--origin N] [--head N]`
  — CLONE semantics: reads `in.pmt`, applies the edits to tape index N
  (default 0; `Malformed`-style error if out of range), writes to `-o`
  (REQUIRED — refusing to overwrite the input unless `--in-place` is given
  instead of `-o`). `--cells` maps each character of PATTERN through the
  tape's EFFECTIVE alphabet (own if `Some`, else block) by glyph — works for
  both v1 and v2 tape blocks; unknown glyph → error listing the alphabet.
  `--origin`/`--head` parse as i64. Edits compose (any subset may be given;
  no edits = a plain copy).
- Errors are `CliOutput` failures with usage strings, matching the existing
  `tape build` error style.

- [ ] **Step 1: Write the failing integration tests** (in cli_programs.rs,
mirroring its existing tape build/show tests — use its tempdir/run helpers)

```rust
    // tape_new_from_pmx_creates_blank_template: compile+link a trivial
    //   program (existing helper), run `tape new --from prog.pmx -o t.pmt`,
    //   then `tape show t.pmt` — expect one empty tape, head 0.
    // tape_set_clones_with_edits: `tape build "*.*" --head 1 -o a.pmt`,
    //   then `tape set a.pmt -o b.pmt --cells "**" --head 0`,
    //   `tape show b.pmt` reflects the edits AND `tape show a.pmt` is
    //   unchanged (clone semantics).
    // tape_set_requires_output: `tape set a.pmt --head 2` (no -o, no
    //   --in-place) → error exit code + usage mention.
```

- [ ] **Step 2: Verify they fail** (unknown tape subcommand `new`).

- [ ] **Step 3: Implement** the two functions + dispatch + usage + registry
specs (+ glosses). Registry drift tests auto-probe the new flags against the
real parser.

- [ ] **Step 4:** `cargo test --workspace` + gates.

- [ ] **Step 5: Commit**

```bash
git add crates/post-machine
git commit -m "feat(cli): pmt tape new --from and tape set with clone semantics"
```

---

### Task 6: Docs + phase-2 gate

**Files:**
- Modify: `docs/cli.md` (the `pmt tape` section: document `new`/`set` beside `build`/`show`)
- Modify: `docs/formats.md` only if it cross-references tape tooling (check; likely not)
- Modify: `CLAUDE.md` (internal): pma dialect now 0.3; the new pass in the optimizer list; tape authoring in the CLI section — one-line touches
- No code.

- [ ] **Step 1:** Write the cli.md additions (usage lines, flag semantics,
clone-vs-in-place rule, the glyph-pattern mapping through the effective
alphabet), matching the page's existing style; forge-agnostic.

- [ ] **Step 2: Phase gate** — run and report verbatim:
`cargo test --workspace` · `cargo clippy --workspace --all-targets -- -D warnings` ·
`cargo fmt --check` · `git status --short crates/post-machine/tests/golden/` (empty).

- [ ] **Step 3: Commit**

```bash
git add docs/cli.md CLAUDE.md
git commit -m "docs(cli): tape new/set; note pma 0.3 and the fuse-tape-ops pass"
```

---

## Self-review notes (spec → plan coverage)

- Spec §3.6 in full: opcodes 0x07/0x0F (task 1), `≡ wr;lft / wr;rgt incl. MF
  latch` (task 1's equivalence test), `-O1 peephole, codegen untouched by
  fusion` (tasks 3-4 split: codegen only EMITS, the pass FUSES), `-O0
  bit-identity` (fused ops unreachable at O0), `no fusion across brk`
  (adjacency rule + test), `pma 0.2→0.3` + syntax/lint/fmt/LSP/completions/
  grammar/isa.md tail (task 2; lint/fmt/LSP derive from pm1_syntax so no
  separate edits), additive-ISA-revision policy note lives in isa.md prose
  (task 2).
- Spec §13 tape authoring: `new --from` + `set` with clone semantics,
  `--in-place` opt-in, glyph validation against the effective alphabet,
  works for MT v1 and v2 (task 5). Deviation from spec noted: pmt already
  had `tape build/show`; `build` stays (established surface), `new`/`set`
  are added beside it. Multiple `--tape` groups per call: deferred (one
  tape per invocation in v1 of the surface) — noted as a doc line.
- Version spaces: pma dialect 0.3 (task 2), IR_VERSION +1 (task 3); the
  release-notes version block happens at the next release cut, not here.
- The IrOp-variant ripple (exhaustive matches across passes) is called out
  with a uniform conservative rule (task 3) — the compiler enumerates the
  sites, the rule decides each arm.
