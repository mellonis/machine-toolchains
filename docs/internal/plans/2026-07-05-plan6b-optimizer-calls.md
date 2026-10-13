# Plan 6b — Optimizer Call Passes Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The remaining spec §8 passes — **inline** (5), **tail-merge** (7), **tail-call** (8) — complete `-O1`, on top of new core support for symbol-operand jumps (`jmp @name`) through the assembler, linker, and both disassemblers.

**Architecture:** Tail calls need a relocated jump: the assembler's existing `Slot::Call` machinery (far opcode + 4-byte hole + relocation) is reused verbatim for `jmp @name`; the linker's `classify()` learns to treat a holed jump as a relaxable symbol site; disassemblers print cross-function jumps as `jmp @name`, preserving the round-trip law. In the optimizer, `IrTerm` gains a `TailCall { name }` terminator (IR JSON version 2), the driver gains a program-level pass stage (inline needs cross-function access), and the pipeline becomes: **inline** (program-level) then per-function **check-fold, jump-threading, cell-state, branch-fold, tail-call, tail-merge, dce** (Task-6 ruling: tail-call runs BEFORE tail-merge — return-chaining would otherwise rewrite a tail-position `Return` to `FallThrough` and permanently destroy tail-call's precondition; ordering also prefers the larger saving when both apply).

**Tech Stack:** Rust edition 2024, no new dependencies. Baseline: 239 workspace tests green at master/52cf8af.

## Spec deltas (controller applies on plan approval, before Task 1)

1. **§8 equivalence contract, resource exception:** resource-limit traps (step/tact limits, stack overflow/underflow) are quality-of-implementation outcomes, not semantic observables — passes may change resource consumption. Inline and tail-call change stack depth; a self-recursive tail call becomes an in-place loop (StackOverflow at `-O0` → StepLimit at `-O1` is legal and pinned by test).
2. **§6.4 symbol jumps:** `jmp @name` assembles as far `jmp` + relocation to the function symbol (the tail-call form). `jmp.s @name` is an error (width is linker-selected, like `call.s`); conditional `jm`/`jnm @name` are errors (v1: branches take labels only). Disassemblers print relocated/cross-function jumps as `jmp @name`.
3. **§7 lowering/IR:** the terminator set gains `tailcall(name)` — produced only by the optimizer, never by lowering; IR JSON version bumps to 2 (additive).

## Global Constraints

- Equivalence contract (spec §8, as amended above): final tape, termination kind (modulo the resource exception), every MF-dependent branch decision. `brk` remains an observability barrier — **a function containing `brk` is never inlined** (inlining would erase the call frame a debugger shows).
- Inline candidate rule, exactly: callee is defined in this module, is a **leaf** (no `Call` ops, no `TailCall` terminators anywhere in its blocks), contains no `Brk`, is not the caller itself, and (total op count ≤ `INLINE_MAX_OPS = 6` **or** is called from exactly one site module-wide). Candidate set is computed once per pass invocation, from the pre-pass program state.
- Tail-call rule, exactly: a block whose last op is `Call` and whose terminator is `Return`, in any function **except `main`** (main's return is `stp`; the callee's `ret` would underflow).
- Tail-merge v1 scope, exactly: (a) whole-block dedup — identical ops (modulo line numbers) + identical terminator → retarget all references to the first (keeper) and delete the duplicate; (b) return-chaining — a `Return` block physically followed by an empty `Return` block gets `FallThrough` to it (adjacent = free; the spec §8 example's shared `stp`).
- Pass names for `--fno-<name>` and reports: `inline`, `tail-merge`, `tail-call` (joining the five 6a names). Pipeline per round: program-level `inline` first, then per-function `check-fold, jump-threading, cell-state, branch-fold, tail-call, tail-merge, dce` (Task-6 ruling — see Architecture note).
- `IrTerm` loses `Copy` (TailCall carries a `String`): every existing `match`/`if let` on a term is updated mechanically (the compiler enumerates the sites; bind by reference and deref the `u32`s). No wildcard arms — exhaustiveness stays the safety net.
- `-O0` and all Plan 5/6a `-O0` goldens stay bit-identical. ONE 6a `-O1` golden changes **intentionally**: `spec_sample_is_already_optimal` becomes wrong once inline exists (main's call to goToEnd inlines) and is rewritten in Task 4 — this is the only sanctioned golden change.
- Gates per task: `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo fmt --check`. Commits per task, path-scoped, never push, no attribution footers. BLOCK with derivation on any golden mismatch; never adjust numbers to observed output.

## File Structure

- **Task 1 (core):** modify `crates/core/src/asm/parser.rs` (`@name` operand), `crates/core/src/asm/assembler.rs` (symbol-jump slots + restrictions), `crates/core/src/linker/layout.rs` (hole-first classify), `crates/core/src/asm/disassembler.rs` (object `jmp @sym`).
- **Task 2 (core):** modify `crates/core/src/asm/disassembler.rs` (executable jump-to-root symbol form); add PM-level round-trip tests in `crates/post-machine/tests/link_programs.rs`.
- **Task 3:** modify `crates/post-machine/src/ir.rs` (TailCall, version 2, match sites), `optimizer/{mod,dataflow,check_fold,jump_threading,dce,branch_fold,cell_state}.rs` (match sites), `codegen.rs`; create `optimizer/tail_call.rs`; tests + `tests/opt_equivalence.rs`.
- **Task 4:** create `optimizer/inline.rs`; modify `optimizer/mod.rs` (program-pass stage); rewrite the one sanctioned golden in `tests/opt_equivalence.rs`.
- **Task 5:** create `optimizer/tail_merge.rs`; tests + spec-example golden.
- **Task 6:** combined goldens in `tests/opt_equivalence.rs`.

---

### Task 1: Core symbol-jump support (assembler + linker + object disassembler)

**Files:**
- Modify: `crates/core/src/asm/parser.rs`, `crates/core/src/asm/assembler.rs`, `crates/core/src/linker/layout.rs`, `crates/core/src/asm/disassembler.rs`

**Interfaces:**
- Produces: `.pma` accepts `jmp @name` (far jump + relocation); linker relaxes holed jumps exactly like calls; object disassembly prints them as `jmp @name` and round-trips. Errors: `jmp.s @name`, `jm/jnm @name`, `call @name` all rejected with `BadOperand`.
- Consumes: existing `Slot::Call`, `Piece::CallSite`, relax-pair tables (PM-1 and the test fixture both already pair `jmp`/`jmp.s`).

- [ ] **Step 1: Parser — the `@name` operand.** In `crates/core/src/asm/parser.rs`, add a variant to `SourceOperand`:

```rust
#[derive(Debug)]
pub(crate) enum SourceOperand {
    None,
    Ints(Vec<i64>),
    Name(String),
    /// `@name` — a function-symbol reference, not a local label.
    SymbolName(String),
}
```

and in the `OperandKind::RelI8 | OperandKind::RelI32` arm of `parse`, replace the operand construction with:

```rust
            OperandKind::RelI8 | OperandKind::RelI32 => {
                let [one] = operands.as_slice() else {
                    return Err(err(line_no, AsmErrorKind::BadOperand("takes one name")));
                };
                if let Some(sym) = one.strip_prefix('@') {
                    if !is_ident(sym) {
                        return Err(err(
                            line_no,
                            AsmErrorKind::BadOperand("bad symbol name after `@`"),
                        ));
                    }
                    SourceOperand::SymbolName(sym.to_string())
                } else {
                    if !is_ident(one) {
                        return Err(err(
                            line_no,
                            AsmErrorKind::BadOperand("jump/call operands are names, not numbers"),
                        ));
                    }
                    SourceOperand::Name((*one).to_string())
                }
            }
```

- [ ] **Step 2: Assembler — classify symbol jumps into the existing `Slot::Call`.** In `crates/core/src/asm/assembler.rs`: update `Slot::Call`'s comment to `/// A symbol site — call or `jmp @name`: far opcode + 4-byte hole + relocation.`, add `Flow` to the `use super::syntax::…` import, and add this arm to the `match (&entry.operand, operand)` in `assemble_function` (before the existing `Name` arm):

```rust
                    (
                        OperandKind::RelI8 | OperandKind::RelI32,
                        SourceOperand::SymbolName(name),
                    ) => match entry.flow {
                        Flow::Call => {
                            return Err(err(
                                *line,
                                AsmErrorKind::BadOperand(
                                    "call operands are already symbols; drop the `@`",
                                ),
                            ));
                        }
                        Flow::Jump => {
                            if entry.operand == OperandKind::RelI8 {
                                return Err(err(
                                    *line,
                                    AsmErrorKind::BadOperand(
                                        "jmp.s width is linker-selected; write jmp @name",
                                    ),
                                ));
                            }
                            slots.push(Slot::Call {
                                line: *line,
                                opcode: *opcode,
                                symbol: name.clone(),
                            });
                        }
                        _ => {
                            return Err(err(
                                *line,
                                AsmErrorKind::BadOperand(
                                    "conditional jumps take labels, not symbols",
                                ),
                            ));
                        }
                    },
```

(`entry.flow` is the `SyntaxEntry` field added in Plan 3.)

- [ ] **Step 3: Linker — holed jumps are symbol sites.** In `crates/core/src/linker/layout.rs`, `classify()`: replace the `(Flow::Jump | Flow::Branch, DecodedOperand::RelTarget(orig_target))` arm with a hole-first version:

```rust
                    (Flow::Jump | Flow::Branch, DecodedOperand::RelTarget(orig_target)) => {
                        let hole = addr + 1;
                        if let Some(&callee) = call_holes.get(&hole) {
                            // A relocated symbol jump (tail call). Branches
                            // are labels-only in v1 — a holed branch is a
                            // malformed object, not a feature.
                            if entry.flow == Flow::Branch {
                                return Err(LinkError::MalformedBlob {
                                    symbol: f.name.to_string(),
                                    at: hole,
                                });
                            }
                            consumed_holes.insert(hole);
                            pieces.push(Piece::CallSite { orig: addr, callee });
                        } else {
                            pieces.push(Piece::Jump {
                                orig: addr,
                                opcode: entry.opcode,
                                width: (len - 1) as u8,
                                orig_target,
                            });
                        }
                    }
```

Update `Piece::CallSite`'s doc comment to say "symbol site (call or relocated tail jump)"; extend `LinkReport`'s `relaxed_calls`/`far_calls` doc comments to "symbol sites (calls and tail jumps)" — field names unchanged. The relaxation fixpoint and emission need NO changes: `short_of(far_opcode)` already resolves `jmp` → `jmp.s` through the existing relax pairs.

- [ ] **Step 4: Object disassembler — print and round-trip `jmp @name`.** In `crates/core/src/asm/disassembler.rs`, `disassemble_object`:

(a) In the jump-target collection loop, do not invent labels for relocated jumps — replace the `if !is_call { targets.insert(*t); }` logic with:

```rust
                if !is_call && !reloc_at.contains_key(&(blob, d.addr + 1)) {
                    targets.insert(*t);
                }
```

(b) In the operand-rendering `DecodedOperand::RelTarget(t)` arm, extend the non-call path:

```rust
                        DecodedOperand::RelTarget(t) => {
                            if syntax.is_call(entry.opcode) {
                                reloc_at
                                    .get(&(blob, d.addr + 1))
                                    .map(|name| (*name).to_string())
                            } else if let Some(name) = reloc_at.get(&(blob, d.addr + 1)) {
                                // Relocated symbol jump — always far in objects.
                                Some(format!("@{name}"))
                            } else {
                                Some(format!("L{t:04X}"))
                            }
                        }
```

- [ ] **Step 5: Core tests.** Append to the `tests` module of `crates/core/src/asm/assembler.rs`:

```rust
    #[test]
    fn symbol_jump_emits_hole_and_relocation() {
        // fixture: jmp far = 0x20; g defined → blob 1.
        let obj = asm(".func f\n        jmp @g\n.func g\n        ret\n");
        assert_eq!(obj.blobs[0], vec![0x0E, 0x20, 0, 0, 0, 0]);
        assert_eq!(obj.relocations.len(), 1);
        assert_eq!(obj.relocations[0].offset, 2);
        assert_eq!(obj.symbols[obj.relocations[0].symbol as usize].name, "g");
        // External symbol jump works the same way:
        let ext = asm(".func f\n        jmp @missing\n");
        assert!(ext.symbols.iter().any(|s| s.name == "missing"
            && s.def == SymbolDef::External));
    }

    #[test]
    fn symbol_operand_restrictions() {
        let e = assemble(&test_syntax(), 0x7E, ".func f\n        jmp.s @g\n", false).unwrap_err();
        assert!(matches!(e.kind, AsmErrorKind::BadOperand(m) if m.contains("linker-selected")));
        let e = assemble(&test_syntax(), 0x7E, ".func f\n        br @g\n", false).unwrap_err();
        assert!(matches!(e.kind, AsmErrorKind::BadOperand(m) if m.contains("labels")));
        let e = assemble(&test_syntax(), 0x7E, ".func f\n        call @g\n", false).unwrap_err();
        assert!(matches!(e.kind, AsmErrorKind::BadOperand(m) if m.contains("drop the `@`")));
    }
```

(the fixture's `br` at 0x22 is its Flow::Branch instruction). Append to `crates/core/src/linker/layout.rs` tests:

```rust
    #[test]
    fn tail_jump_relaxes_like_a_call() {
        let syntax = syntax_with_short_call();
        // main tail-jumps g: [ent][jmp @g] → linked short: [0E][30 off][0E][0B].
        let src = ".func main\n        jmp @g\n.func g\n        ret\n";
        let obj = assemble(&syntax, 0x7E, src, false).unwrap();
        let out = link(&syntax, &[obj], &[], LinkOptions::default()).unwrap();
        // jmp.s at 1, end 3, g at 3 → off 0.
        assert_eq!(out.executable.code, vec![0x0E, 0x30, 0x00, 0x0E, 0x0B]);
        assert_eq!(out.report.relaxed_calls, 1);
    }

    #[test]
    fn holed_branch_is_malformed() {
        use crate::formats::object::{ObjectFile, Relocation, Symbol, SymbolDef};
        let syntax = syntax_with_short_call();
        // [0E][22 xx][02]: br (Flow::Branch, RelI8) with a reloc hole at 2.
        let obj = ObjectFile {
            arch: 0x7E,
            symbols: vec![
                Symbol { name: "main".into(), def: SymbolDef::Defined { blob: 0 } },
                Symbol { name: "g".into(), def: SymbolDef::External },
            ],
            blobs: vec![vec![0x0E, 0x22, 0x00, 0x02]],
            relocations: vec![Relocation { blob: 0, offset: 2, symbol: 1 }],
            debug: None,
        };
        let lib = assemble(&syntax, 0x7E, ".func g\n        ret\n", false).unwrap();
        let e = link(&syntax, &[obj], &[lib], LinkOptions::default()).unwrap_err();
        assert_eq!(
            e,
            crate::linker::LinkError::MalformedBlob { symbol: "main".into(), at: 2 }
        );
    }
```

NOTE for the implementer on `holed_branch_is_malformed`: the relocation points at offset 2 but `br`'s operand is 1 byte (RelI8) — if `ObjectFile::from_bytes`-level validation (offset+4 ≤ blob len) makes this exact shape unrepresentable, adjust the blob to keep 4 bytes after the hole (e.g. pad with nops: `[0x0E, 0x22, 0x00, 0x01, 0x01, 0x01, 0x02]` with the hole at 2) — the POINT is a hole coinciding with a Branch operand; keep that and adapt the padding. This object is hand-built in memory, so from_bytes checks don't run — but decode of `br` consumes 2 bytes and the hole at 2 falls inside... verify the hole lands exactly at the branch's operand byte (addr+1 = 2 for the br at 1). If the arithmetic needs adjusting, BLOCK with your derivation.

Append to `crates/core/src/asm/disassembler.rs` tests:

```rust
    #[test]
    fn object_symbol_jump_prints_at_form_and_round_trips() {
        let syntax = test_syntax();
        let src = ".func f\n        jmp @g\n        stop\n.func g\n        ret\n";
        let obj1 = assemble(&syntax, 0x7E, src, false).unwrap();
        let text = disassemble_object(&syntax, &obj1);
        assert!(text.contains("jmp     @g"), "{text}");
        assert!(!text.contains("L0"), "no phantom label for the reloc'd jump: {text}");
        let obj2 = assemble(&syntax, 0x7E, &text, false).unwrap();
        assert_eq!(obj1, obj2);
    }
```

- [ ] **Step 6: Gates, then commit.**

```bash
git add crates/core/src/asm/parser.rs crates/core/src/asm/assembler.rs crates/core/src/linker/layout.rs crates/core/src/asm/disassembler.rs
git commit -m "feat(core): symbol-operand jumps — jmp @name assembles, relaxes, and round-trips as a relocated symbol site"
```

---

### Task 2: Executable disassembler — cross-function jumps as `jmp @name`

**Files:**
- Modify: `crates/core/src/asm/disassembler.rs` (`disassemble_executable`)
- Modify: `crates/post-machine/tests/link_programs.rs` (round-trip tests)

**Interfaces:**
- Produces: a jump whose target is a discovered root prints as far-mnemonic `jmp @<funcname>`; the round-trip law (dis → asm → link byte-identical) holds for tail-call layouts. A jump into another region NOT at a root keeps the `.byte` fallback. A tail-called-only function (never `call`ed) is not a root and disassembles as a local label inside its caller's region — bytes still round-trip; only the map knows it was a function.

- [ ] **Step 1: Implement.** In `disassemble_executable`:

(a) Generalize the far-mnemonic helper — rename `display_mnemonic` to `far_mnemonic` and drop its Call-only gate:

```rust
    // A short opcode displays as its far partner when the operand is
    // printed in symbol form (the two are interchangeable at source
    // level; only far is canonical for symbol sites).
    let far_mnemonic = |entry: &SyntaxEntry| -> &'static str {
        if let Some(pair) = syntax.relax_pairs.iter().find(|p| p.short == entry.opcode)
            && let Some(far) = syntax.by_opcode(pair.far)
        {
            return far.mnemonic;
        }
        entry.mnemonic
    };
```

(b) In the region emission loop, restructure the `DecodedOperand::RelTarget(t)` arm to yield `(mnemonic, operand)` pairs:

```rust
                        DecodedOperand::RelTarget(t) => {
                            if entry.flow == Flow::Call && roots.contains(t) {
                                Some((far_mnemonic(entry), func_name(*t)))
                            } else if entry.flow == Flow::Jump && roots.contains(t) {
                                // Tail jump to a function: symbol form.
                                Some((far_mnemonic(entry), format!("@{}", func_name(*t))))
                            } else if entry.flow != Flow::Call && *t > root && *t < end {
                                Some((entry.mnemonic, format!("L{t:04X}")))
                            } else {
                                None // cross-region non-root: .byte fallback
                            }
                        }
```

and adjust the other arms (`None` → `Some((entry.mnemonic, String::new()))`, `Ints` likewise) plus the grid call to use the pair. Also update the jump-target LABEL collection loop: skip targets that are roots (they print as symbols now):

```rust
                if e.flow != Flow::Call && *t > root && *t < end && !roots.contains(t) {
                    targets.insert(*t);
                }
```

(`roots` here is the `Vec<u32>` — use `roots.binary_search(t).is_ok()` or keep a `BTreeSet` copy; implementer's choice, note it.)

- [ ] **Step 2: Core-level test** (append to disassembler tests):

```rust
    #[test]
    fn executable_tail_jump_prints_symbol_form_and_reassembles() {
        let syntax = syntax_with_pairs(); // helper below
        // main calls f (root), f tail-jumps main: infinite loop program.
        // main at 0: [0E][21 +1 → 7... derive], f: [0E][30/20 …].
        let src = "\
.func main
        call    f
        stop
.func f
        jmp     @main
";
        let obj = assemble(&syntax, 0x7E, src, false).unwrap();
        let out = crate::linker::link(&syntax, &[obj], &[], crate::linker::LinkOptions::default())
            .unwrap();
        let text = disassemble_executable(&syntax, &out.executable);
        assert!(text.contains("jmp     @main"), "{text}");
        assert!(!text.contains(".byte"), "{text}");
        let obj2 = assemble(&syntax, 0x7E, &text, false).unwrap();
        let out2 =
            crate::linker::link(&syntax, &[obj2], &[], crate::linker::LinkOptions::default())
                .unwrap();
        assert_eq!(out2.executable.code, out.executable.code);
    }
```

with a local helper `fn syntax_with_pairs()` = `test_syntax()` + the 0x21/0x31 call pair exactly as `layout.rs`'s `syntax_with_short_call()` builds it (copy that shape; disassembler tests currently build it inline in `short_call_in_executable_prints_far_mnemonic` — reuse that pattern).

- [ ] **Step 3: PM-level round-trip** (append to `crates/post-machine/tests/link_programs.rs`):

```rust
#[test]
fn tail_call_layout_round_trips_through_disassembly() {
    // g is called (a root) AND tail-jumped: both forms must survive.
    let src = "\
.func main
        call    g
        rgt
        call    f
        stp
.func f
        lft
        jmp     @g
.func g
        ret
";
    let obj = assemble(src, false).unwrap();
    let out = link(&[obj], &[], LinkOptions::default()).unwrap();
    let text = disassemble_executable(&out.executable);
    assert!(text.contains("jmp     @"), "{text}");
    let obj2 = assemble(&text, false).unwrap();
    let out2 = link(&[obj2], &[], LinkOptions::default()).unwrap();
    assert_eq!(out2.executable.code, out.executable.code);
}
```

- [ ] **Step 4: Gates, then commit.**

```bash
git add crates/core/src/asm/disassembler.rs crates/post-machine/tests/link_programs.rs
git commit -m "feat(core): executable disassembly prints jumps-to-roots as jmp @name; tail-call layouts round-trip"
```

---

### Task 3: `IrTerm::TailCall` + the tail-call pass

**Files:**
- Modify: `crates/post-machine/src/ir.rs`, `crates/post-machine/src/codegen.rs`, `crates/post-machine/src/optimizer/{mod,dataflow,check_fold,jump_threading,dce,cell_state,branch_fold}.rs` (match-site fallout), `crates/post-machine/tests/opt_equivalence.rs`
- Create: `crates/post-machine/src/optimizer/tail_call.rs`

**Interfaces:**
- Produces: `IrTerm::TailCall { name: String }` (IR_VERSION = 2, serde tag `tail_call`), codegen emits `jmp @name`, `tail_call::run` converts `[…, Call{name}] + Return` (non-main) into `[…] + TailCall{name}`. PIPELINE per-function order gains `tail-call` LAST.
- **`IrTerm` loses `Copy`.** The compiler will enumerate every match site; the mechanical migration is: match on `&b.term` (or rely on default binding modes), deref the `u32` bindings, and `.clone()` where a term value is stored (codegen has none; inline in Task 4 clones).

- [ ] **Step 1: IR changes** in `ir.rs`:

```rust
pub const IR_VERSION: u32 = 2;
```

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum IrTerm {
    FallThrough { to: u32 },
    Goto { to: u32 },
    Check { marked: u32, blank: u32 },
    Return,
    Halt,
    /// Optimizer-produced (spec §8 pass 8): jump to the callee's `ent`
    /// instead of `call` + `ret`. Never emitted by lowering.
    TailCall { name: String },
}
```

(remove `Copy` from the derive). `validate_function` gains `IrTerm::TailCall { .. } => {}` alongside Return/Halt. Fix every match site the compiler now flags across ir.rs (lowering DFS, validate), optimizer modules, and codegen — semantics identical, plus a `TailCall` arm everywhere, always in the "no successors / no targets / terminal" group:
  - `dataflow.rs` `block_entry_facts`: `IrTerm::Return | IrTerm::Halt | IrTerm::TailCall { .. } => {}`
  - `dce.rs` DFS: same grouping (a tail call leaves the function — no intra-function successors).
  - `jump_threading.rs`: `forwards_to` unchanged in meaning (TailCall is not a forwarder); the retarget match gains the empty arm.
  - `check_fold.rs`/`branch_fold.rs`/`cell_state.rs`: compile-driven arm additions only.
  - `optimizer/mod.rs`: no logic change (passes don't match terms there).

- [ ] **Step 2: Codegen** in `codegen.rs` — two arms:

```rust
            // referenced-labels pass:
            IrTerm::Return | IrTerm::Halt | IrTerm::TailCall { .. } => {}
```

```rust
            // emission:
            IrTerm::TailCall { name } => {
                e.push(grid(None, "jmp", &format!("@{name}")), b.term_line)
            }
```

- [ ] **Step 3: The pass** — `crates/post-machine/src/optimizer/tail_call.rs`:

```rust
//! tail-call (spec §8 pass 8): a call in tail position emits `jmp` to
//! the callee's `ent` (legal for jumps) instead of `call` + `ret` —
//! saves a stack slot and the return trip. Never applied in `main`,
//! whose return is `stp`: the callee's `ret` would underflow.

use crate::ir::{IrFunction, IrOp, IrTerm};

pub fn run(f: &mut IrFunction) -> u32 {
    if f.name == "main" {
        return 0;
    }
    let mut changes = 0;
    for b in &mut f.blocks {
        if matches!(b.term, IrTerm::Return)
            && matches!(b.ops.last(), Some(IrOp::Call { .. }))
        {
            let Some(IrOp::Call { name, .. }) = b.ops.pop() else {
                unreachable!("just matched a trailing call")
            };
            b.term = IrTerm::TailCall { name };
            changes += 1;
        }
    }
    changes
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{IrTerm, lower};
    use crate::lexer::lex;
    use crate::parser::parse;

    fn tc(src: &str) -> crate::ir::IrProgram {
        let mut ir = lower(&parse(&lex(src).unwrap()).unwrap()).unwrap().0;
        for f in &mut ir.functions {
            run(f);
            crate::ir::validate_function(f).unwrap();
        }
        ir
    }

    #[test]
    fn trailing_call_becomes_a_tail_jump() {
        let ir = tc("g() { left; @f(!); }");
        let b = &ir.functions[0].blocks[0];
        assert_eq!(b.ops.len(), 1); // the call op is gone
        assert_eq!(b.term, IrTerm::TailCall { name: "f".into() });
    }

    #[test]
    fn implicit_return_after_call_also_converts() {
        let ir = tc("g() { @f(); }"); // falls off the end
        assert_eq!(ir.functions[0].blocks[0].term, IrTerm::TailCall { name: "f".into() });
    }

    #[test]
    fn main_is_exempt_and_non_tail_calls_survive() {
        let ir = tc("main() { @f(!); }");
        assert!(matches!(ir.functions[0].blocks[0].term, IrTerm::Return));
        let ir = tc("g() { @f(); left; }"); // call not in tail position
        assert_eq!(ir.functions[0].blocks[0].ops.len(), 2);
    }
}
```

Wire `("tail-call", tail_call::run)` into the per-function `PIPELINE` and declare the module. [Task-6 ruling: its position is AFTER `branch-fold` and BEFORE `tail-merge`/`dce` — originally authored as last, which let return-chaining destroy the tail-call precondition; see Architecture note.]

- [ ] **Step 4: Equivalence + semantics pins** (append to `tests/opt_equivalence.rs`):

```rust
#[test]
fn tail_call_preserves_behavior_and_shrinks() {
    // g tail-calls f; inline would dissolve the call first, so pin the
    // tail-call transform in isolation via --fno-inline (Task 4 adds
    // inline; this test is written to be correct both before and after).
    let src = "f() { right(!); } g() { left, @f(!); } main() { @g(); mark; }";
    let o0 = build(src, OptLevel::O0);
    let out1 = compile(
        src,
        CompileOptions {
            opt_level: OptLevel::O1,
            disabled_passes: vec!["inline".to_string()],
            ..Default::default()
        },
    )
    .unwrap();
    let o1 = link(&[out1.object], &[], LinkOptions::default()).unwrap().executable;
    for (cells, head) in TAPES {
        let r0 = run_tape(&o0, cells, *head);
        let r1 = run_tape(&o1, cells, *head);
        assert_eq!(r0, r1, "tape {cells:?}/{head}");
    }
    assert!(o1.code.len() < o0.code.len(), "{} -> {}", o0.code.len(), o1.code.len());
}

#[test]
fn self_recursive_tail_call_becomes_an_in_place_loop() {
    // THE documented resource exception (spec §8 as amended): at -O0 the
    // recursion overflows the return stack; at -O1 the tail call is a
    // self-jump — an infinite loop that hits the step limit instead.
    // Termination KIND changes; that is sanctioned for resource traps.
    let src = "spin() { @spin(!); } main() { @spin(); }";
    let o0 = build(src, OptLevel::O0);
    let o1 = build(src, OptLevel::O1);
    let (outcome0, _, _) = run_tape(&o0, &[true], 0);
    let (outcome1, _, _) = run_tape(&o1, &[true], 0);
    assert!(
        matches!(outcome0, mtc_core::vm::Outcome::Trapped(mtc_core::vm::Trap::StackOverflow)),
        "{outcome0:?}"
    );
    assert!(
        matches!(outcome1, mtc_core::vm::Outcome::Trapped(mtc_core::vm::Trap::StepLimit)),
        "{outcome1:?}"
    );
}
```

(`spin` calls itself → not a leaf → inline skips it with no flag needed; verify that reasoning holds when Task 4 lands.) Also add the byte-shape pin:

```rust
#[test]
fn tail_call_emits_a_relaxed_jump() {
    use mtc_post_machine::arch::opcodes::*;
    let src = "f() { right(!); } g() { left, @f(!); } main() { @g(); mark; }";
    let out = compile(
        src,
        CompileOptions {
            opt_level: OptLevel::O1,
            disabled_passes: vec!["inline".to_string()],
            ..Default::default()
        },
    )
    .unwrap();
    let linked = link(&[out.object], &[], LinkOptions::default()).unwrap();
    // Layout (BFS from main): main, g, f.
    // main: ent0, call.s g +? , wr 1, stp | g: ent, lft, jmp.s @f | f: ent, rgt, ret.
    // main = [0D][1B off][06 81][02] = 6 bytes → g at 6: [0D][04][18 off] = 4 → f at 10: [0D][05][0C].
    // call.s: end 3, g at 6 → +3. jmp.s: at 8, end 10, f at 10 → 0.
    assert_eq!(
        linked.executable.code,
        vec![
            ENT, CALL_S, 0x03, WR, 0x81, STP, // main
            ENT, LFT, JMP_S, 0x00, // g
            ENT, RGT, RET, // f
        ]
    );
}
```

- [ ] **Step 5: IR JSON version test** (in `ir.rs` tests): update `json_round_trips_with_a_version` to assert `version, 2`, and add:

```rust
    #[test]
    fn tail_call_serializes_with_its_own_tag() {
        let term = IrTerm::TailCall { name: "f".into() };
        let json = serde_json::to_string(&term).unwrap();
        assert!(json.contains("\"kind\":\"tail_call\""), "{json}");
        assert_eq!(serde_json::from_str::<IrTerm>(&json).unwrap(), term);
    }
```

- [ ] **Step 6: Gates, then commit.**

```bash
git add crates/post-machine/src/ir.rs crates/post-machine/src/codegen.rs crates/post-machine/src/optimizer crates/post-machine/tests/opt_equivalence.rs
git commit -m "feat(post-machine): TailCall terminator (IR v2) and the tail-call pass"
```

---

### Task 4: The inline pass (program-level driver stage)

**Files:**
- Create: `crates/post-machine/src/optimizer/inline.rs`
- Modify: `crates/post-machine/src/optimizer/mod.rs` (program-pass stage), `crates/post-machine/tests/opt_equivalence.rs` (the ONE sanctioned golden rewrite)

**Interfaces:**
- Produces: `inline::run(&mut IrProgram) -> u32`; the driver gains `PROGRAM_PIPELINE` executed at the START of each round, before the per-function pipeline. `PassChange.function` for program passes is `"(module)"`.

- [ ] **Step 1: Driver stage** in `optimizer/mod.rs`:

```rust
type ProgramPassFn = fn(&mut IrProgram) -> u32;

/// Program-level passes (cross-function), run at round start.
const PROGRAM_PIPELINE: &[(&str, ProgramPassFn)] = &[("inline", inline::run)];
```

and inside the round loop, BEFORE the per-function pipeline:

```rust
        for (name, pass) in PROGRAM_PIPELINE {
            if options.disabled.contains(*name) {
                continue;
            }
            let n = pass(ir);
            #[cfg(debug_assertions)]
            for f in &ir.functions {
                if let Err(e) = crate::ir::validate_function(f) {
                    panic!("pass `{name}` broke IR invariants: {e}");
                }
            }
            if n > 0 {
                report.changes.push(PassChange {
                    pass: name,
                    function: "(module)".to_string(),
                    changes: n,
                });
                if options.capture {
                    snapshots.push((format!("after:{name}"), ir.clone()));
                }
            }
            round_changes += n;
        }
```

- [ ] **Step 2: `inline.rs`:**

```rust
//! inline (spec §8 pass 5): splice small leaf callees into their call
//! sites, intra-module. Dissolving the call barrier is what unlocks the
//! dataflow across it — the other passes then see through the old
//! boundary. Candidates never contain `brk` (inlining would erase the
//! call frame a debugger shows) and never contain calls of their own.

use std::collections::HashMap;

use crate::ir::{IrBlock, IrFunction, IrOp, IrProgram, IrTerm};

const INLINE_MAX_OPS: usize = 6;

fn is_leaf_without_brk(f: &IrFunction) -> bool {
    f.blocks.iter().all(|b| {
        !matches!(b.term, IrTerm::TailCall { .. })
            && b.ops
                .iter()
                .all(|op| !matches!(op, IrOp::Call { .. } | IrOp::Brk { .. }))
    })
}

fn op_count(f: &IrFunction) -> usize {
    f.blocks.iter().map(|b| b.ops.len()).sum()
}

pub fn run(ir: &mut IrProgram) -> u32 {
    // Candidate set is fixed from the pre-pass program state.
    let mut call_counts: HashMap<&str, u32> = HashMap::new();
    for f in &ir.functions {
        for b in &f.blocks {
            for op in &b.ops {
                if let IrOp::Call { name, .. } = op {
                    *call_counts.entry(name.as_str()).or_insert(0) += 1;
                }
            }
            if let IrTerm::TailCall { name } = &b.term {
                *call_counts.entry(name.as_str()).or_insert(0) += 1;
            }
        }
    }
    let candidates: HashMap<String, IrFunction> = ir
        .functions
        .iter()
        .filter(|f| {
            is_leaf_without_brk(f)
                && (op_count(f) <= INLINE_MAX_OPS
                    || call_counts.get(f.name.as_str()).copied().unwrap_or(0) == 1)
        })
        .map(|f| (f.name.clone(), f.clone()))
        .collect();

    let mut changes = 0;
    for f in &mut ir.functions {
        while let Some((bi, oi)) = find_site(f, &candidates) {
            splice(f, bi, oi, &candidates);
            changes += 1;
        }
    }
    changes
}

fn find_site(
    f: &IrFunction,
    candidates: &HashMap<String, IrFunction>,
) -> Option<(usize, usize)> {
    for (bi, b) in f.blocks.iter().enumerate() {
        for (oi, op) in b.ops.iter().enumerate() {
            if let IrOp::Call { name, .. } = op
                && name != &f.name
                && candidates.contains_key(name)
            {
                return Some((bi, oi));
            }
        }
    }
    None
}

fn splice(f: &mut IrFunction, bi: usize, oi: usize, candidates: &HashMap<String, IrFunction>) {
    let next_id = f.blocks.iter().map(|b| b.id).max().unwrap_or(0) + 1;
    let IrOp::Call { name, line } = f.blocks[bi].ops[oi].clone() else {
        unreachable!("find_site returned a call site")
    };
    let callee = &candidates[&name];

    // Split the site block: ops after the call + the original terminator
    // move to a fresh continuation block.
    let tail_ops = f.blocks[bi].ops.split_off(oi + 1);
    f.blocks[bi].ops.pop(); // the call itself
    let cont_id = next_id;
    let mut id_map: HashMap<u32, u32> = HashMap::new();
    for (k, cb) in callee.blocks.iter().enumerate() {
        id_map.insert(cb.id, next_id + 1 + k as u32);
    }
    let cont = IrBlock {
        id: cont_id,
        labels: vec![],
        line,
        ops: tail_ops,
        term: f.blocks[bi].term.clone(),
        term_line: f.blocks[bi].term_line,
    };
    f.blocks[bi].term = IrTerm::Goto {
        to: id_map[&callee.blocks[0].id],
    };
    f.blocks[bi].term_line = line;

    let clones: Vec<IrBlock> = callee
        .blocks
        .iter()
        .map(|cb| {
            let mut nb = cb.clone();
            nb.id = id_map[&cb.id];
            nb.labels = vec![]; // callee label names are meaningless here
            nb.term = match &cb.term {
                IrTerm::FallThrough { to } => IrTerm::FallThrough { to: id_map[to] },
                IrTerm::Goto { to } => IrTerm::Goto { to: id_map[to] },
                IrTerm::Check { marked, blank } => IrTerm::Check {
                    marked: id_map[marked],
                    blank: id_map[blank],
                },
                // The callee's return continues after the call site.
                IrTerm::Return => IrTerm::Goto { to: cont_id },
                IrTerm::Halt => IrTerm::Halt,
                IrTerm::TailCall { .. } => unreachable!("candidates are leaves"),
            };
            nb
        })
        .collect();

    // Insertion order: callee body, then continuation, right after the
    // site block — preserves fall-through layout quality.
    let mut at = bi + 1;
    for c in clones {
        f.blocks.insert(at, c);
        at += 1;
    }
    f.blocks.insert(at, cont);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::lower;
    use crate::lexer::lex;
    use crate::parser::parse;

    fn inlined(src: &str) -> IrProgram {
        let mut ir = lower(&parse(&lex(src).unwrap()).unwrap()).unwrap().0;
        run(&mut ir);
        for f in &ir.functions {
            crate::ir::validate_function(f).unwrap();
        }
        ir
    }

    #[test]
    fn small_leaf_is_spliced_and_the_call_disappears() {
        let ir = inlined("f() { right; } main() { @f(); mark; }");
        let main = ir.functions.iter().find(|f| f.name == "main").unwrap();
        assert!(main.blocks.iter().all(|b| b
            .ops
            .iter()
            .all(|op| !matches!(op, IrOp::Call { .. }))));
        // site block + callee clone + continuation = 3 blocks.
        assert_eq!(main.blocks.len(), 3);
    }

    #[test]
    fn brk_and_non_leaf_callees_are_never_inlined() {
        let ir = inlined("f() { debugger; right; } main() { @f(); }");
        let main = ir.functions.iter().find(|f| f.name == "main").unwrap();
        assert!(main.blocks[0].ops.iter().any(|op| matches!(op, IrOp::Call { .. })));

        let ir = inlined("f() { @g(); } g() { right; } main() { @f(); }");
        let main = ir.functions.iter().find(|f| f.name == "main").unwrap();
        // f calls g → f is no leaf; the call to f survives. (g gets
        // inlined INTO f, which is fine — candidates come from the
        // pre-pass state where f was not a leaf.)
        assert!(main.blocks[0].ops.iter().any(
            |op| matches!(op, IrOp::Call { name, .. } if name == "f")
        ));
    }

    #[test]
    fn recursion_is_never_inlined() {
        let ir = inlined("f() { @f(); } main() { @f(); }");
        // f is not a leaf (calls itself) → nothing inlines anywhere.
        for f in &ir.functions {
            let calls: usize = f
                .blocks
                .iter()
                .map(|b| b.ops.iter().filter(|op| matches!(op, IrOp::Call { .. })).count())
                .sum();
            assert_eq!(calls, 1, "{}", f.name);
        }
    }

    #[test]
    fn single_call_site_admits_a_large_callee() {
        // 8 ops > INLINE_MAX_OPS, but exactly one call site module-wide.
        let ir = inlined(
            "big() { right; right; right; right; left; left; left; left; } main() { @big(); }",
        );
        let main = ir.functions.iter().find(|f| f.name == "main").unwrap();
        assert!(main.blocks.iter().all(|b| b
            .ops
            .iter()
            .all(|op| !matches!(op, IrOp::Call { .. }))));
    }

    #[test]
    fn check_arms_inside_the_callee_are_remapped() {
        let ir = inlined("f() { 1: right; check(1, 2); 2: left; } main() { @f(); mark; }");
        let main = ir.functions.iter().find(|f| f.name == "main").unwrap();
        crate::ir::validate_function(main).unwrap(); // remapped targets resolve
        assert!(main.blocks.iter().any(|b| matches!(b.term, IrTerm::Check { .. })));
    }
}
```

- [ ] **Step 3: The sanctioned golden rewrite.** In `tests/opt_equivalence.rs`, REPLACE `spec_sample_is_already_optimal` entirely with:

```rust
#[test]
fn spec_sample_inlines_at_o1() {
    // 6a's "already optimal" golden is obsolete BY DESIGN: with inline,
    // main absorbs goToEnd (leaf, 2 ops) and the linker then drops the
    // now-uncalled goToEnd. Derivation of the 14-byte -O1 executable:
    // main after splice: B[](goto g0'), g0'[rgt](check{g0',g1'}),
    // g1'[lft](goto C), C[rgt](check{b1,b2}), b1[wr0](ret), b2[wr1](ret)
    // → ent, rgt, jm.s -3, lft, rgt, jnm.s +3, wr 0, stp, wr 1, stp
    // = 1+1+2+1+1+2+2+1+2+1 = 14. -O0 linked = 18 (Plan 5 golden).
    let src = "\
goToEnd() {
1:  right;
    check(1, 2);
2:  left;
}

main() {
    @goToEnd();
    right;
    check(3, 4);
3:  unmark(!);
4:  mark;
}
";
    let (o0, o1) = assert_equivalent(src, TAPES);
    assert_eq!((o0, o1), (18, 14));

    // And the linker confirms the callee died:
    let out = compile(
        src,
        CompileOptions {
            opt_level: OptLevel::O1,
            ..Default::default()
        },
    )
    .unwrap();
    let linked = link(&[out.object], &[], LinkOptions::default()).unwrap();
    assert_eq!(linked.report.dropped, vec!["goToEnd".to_string()]);
}

#[test]
fn fno_inline_restores_the_do_no_harm_floor() {
    // With inline off, nothing in the 6b pipeline fires on the spec
    // sample (no tail position, no duplicate blocks, no empty-return
    // adjacency) — the old 6a byte-stability golden, behind the flag.
    let src = "\
goToEnd() {
1:  right;
    check(1, 2);
2:  left;
}

main() {
    @goToEnd();
    right;
    check(3, 4);
3:  unmark(!);
4:  mark;
}
";
    let o0 = compile(src, CompileOptions::default()).unwrap();
    let o1 = compile(
        src,
        CompileOptions {
            opt_level: OptLevel::O1,
            disabled_passes: vec!["inline".to_string()],
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(o0.object, o1.object);
}
```

- [ ] **Step 4: Gates, then commit.**

```bash
git add crates/post-machine/src/optimizer crates/post-machine/tests/opt_equivalence.rs
git commit -m "feat(post-machine): inline pass — program-level driver stage; spec-sample golden rewritten (18 -> 14 by design)"
```

---

### Task 5: The tail-merge pass

**Files:**
- Create: `crates/post-machine/src/optimizer/tail_merge.rs`
- Modify: `crates/post-machine/src/optimizer/mod.rs` (PIPELINE gains `("tail-merge", tail_merge::run)` between `tail-call` and `dce` — Task-6 ruling), `crates/post-machine/tests/opt_equivalence.rs`

- [ ] **Step 1: `tail_merge.rs`:**

```rust
//! tail-merge (spec §8 pass 7), v1 scope: (a) whole-block dedup —
//! semantically identical blocks (same ops modulo line numbers, same
//! terminator) collapse to one, references retargeted; (b) return-
//! chaining — a Return block physically followed by an EMPTY Return
//! block falls through to share the terminal instruction (the spec's
//! `jm Lstp; wr 1; Lstp: stp` example: one stp serves both paths).
//! Suffix-level merging (partial tails) is a future refinement.

use crate::ir::{IrFunction, IrOp, IrTerm};

fn same_op(a: &IrOp, b: &IrOp) -> bool {
    match (a, b) {
        (IrOp::Lft { .. }, IrOp::Lft { .. })
        | (IrOp::Rgt { .. }, IrOp::Rgt { .. })
        | (IrOp::Brk { .. }, IrOp::Brk { .. }) => true,
        (IrOp::Wr { index: x, .. }, IrOp::Wr { index: y, .. }) => x == y,
        (IrOp::Call { name: x, .. }, IrOp::Call { name: y, .. }) => x == y,
        _ => false,
    }
}

fn same_block(a: &crate::ir::IrBlock, b: &crate::ir::IrBlock) -> bool {
    a.ops.len() == b.ops.len()
        && a.ops.iter().zip(&b.ops).all(|(x, y)| same_op(x, y))
        && a.term == b.term
}

pub fn run(f: &mut IrFunction) -> u32 {
    let mut changes = 0;

    // (a) dedup to the earliest identical block; the duplicate is
    // deleted immediately (all references just moved), which also keeps
    // this loop terminating.
    loop {
        let mut found: Option<(u32, u32)> = None; // (dup id, keeper id)
        'outer: for i in 0..f.blocks.len() {
            for j in (i + 1)..f.blocks.len() {
                if same_block(&f.blocks[i], &f.blocks[j]) {
                    found = Some((f.blocks[j].id, f.blocks[i].id));
                    break 'outer;
                }
            }
        }
        let Some((dup, keeper)) = found else { break };
        for b in &mut f.blocks {
            let mut r = |t: &mut u32| {
                if *t == dup {
                    *t = keeper;
                }
            };
            match &mut b.term {
                IrTerm::FallThrough { to } | IrTerm::Goto { to } => r(to),
                IrTerm::Check { marked, blank } => {
                    r(marked);
                    r(blank);
                }
                IrTerm::Return | IrTerm::Halt | IrTerm::TailCall { .. } => {}
            }
        }
        f.blocks.retain(|b| b.id != dup); // j > i ≥ 0, so never the entry
        changes += 1;
    }

    // (b) return-chaining: share the physically-next terminal.
    for i in 0..f.blocks.len().saturating_sub(1) {
        if matches!(f.blocks[i].term, IrTerm::Return)
            && f.blocks[i + 1].ops.is_empty()
            && matches!(f.blocks[i + 1].term, IrTerm::Return)
        {
            f.blocks[i].term = IrTerm::FallThrough {
                to: f.blocks[i + 1].id,
            };
            changes += 1;
        }
    }
    changes
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::lower;
    use crate::lexer::lex;
    use crate::parser::parse;

    fn merged(src: &str) -> crate::ir::IrFunction {
        let mut ir = lower(&parse(&lex(src).unwrap()).unwrap()).unwrap().0;
        run(&mut ir.functions[0]);
        crate::ir::validate_function(&ir.functions[0]).unwrap();
        ir.functions.remove(0)
    }

    #[test]
    fn identical_blocks_dedup_and_check_arms_converge() {
        // Arms 2 and 3 are the same code — dedup makes check(2,3) a
        // check(k,k), which check-fold will collapse next.
        let f = merged("f() { 1: check(2, 3); 2: mark, right(!); 3: mark, right(!); }");
        assert_eq!(f.blocks.len(), 2);
        let (m, b) = match &f.blocks[0].term {
            crate::ir::IrTerm::Check { marked, blank } => (*marked, *blank),
            other => panic!("unexpected {other:?}"),
        };
        assert_eq!(m, b);
    }

    #[test]
    fn return_chaining_shares_the_adjacent_terminal() {
        // The spec §8 example: 1: check(!, 2); 2: mark(!);
        // blocks: b0 Check{exit, b1}, b1 [wr1] Return, exit [] Return.
        let f = merged("f() { 1: check(!, 2); 2: mark(!); }");
        assert!(matches!(
            f.blocks[1].term,
            crate::ir::IrTerm::FallThrough { .. }
        ));
    }

    #[test]
    fn non_adjacent_and_non_empty_returns_stay() {
        let f = merged("f() { 1: check(2, 3); 2: mark(!); 3: unmark(!); }");
        // b1 [wr1] Ret, b2 [wr0] Ret: different ops, not empty — no merge.
        assert!(matches!(f.blocks[1].term, crate::ir::IrTerm::Return));
    }
}
```

- [ ] **Step 2: The spec-example byte golden** (append to `tests/opt_equivalence.rs`):

```rust
#[test]
fn tail_merge_shares_the_stp_exactly_as_the_spec_promises() {
    use mtc_post_machine::arch::opcodes::*;
    // Spec §8 pass 7's own example. -O0: jm B2; wr 1; stp; B2: stp = 7
    // bytes (two stp). -O1: return-chaining drops the first stp — 6.
    let src = "main() { 1: check(!, 2); 2: mark(!); }";
    let (o0, o1) = assert_equivalent(src, TAPES);
    assert_eq!((o0, o1), (7, 6));
    let out = compile(
        src,
        CompileOptions {
            opt_level: OptLevel::O1,
            ..Default::default()
        },
    )
    .unwrap();
    let linked = link(&[out.object], &[], LinkOptions::default()).unwrap();
    // ent, jm.s +2, wr 1, stp — one stp serves both paths.
    assert_eq!(linked.executable.code, vec![ENT, JM_S, 0x02, WR, 0x81, STP]);
}
```

Derivation for the -O1 bytes: blocks b0 `[] Check{exit, b1}`, b1 `[wr1] FallThrough{exit}` (chained), exit `[] Return`. Codegen: b0's blank arm (b1) is next → `jm` to marked arm (exit, name B2); b1: `wr 1`, fall through; exit: label B2, `stp`. Layout: ent@0, jm.s@1..2 (end 3), wr@3..4, stp@5; jm target 5, end 3 → **+2** (at -O0 the target sits one `stp` later, at 6 → +3; the moved target IS the merge's saving). Total 6.

- [ ] **Step 3: Gates, then commit.**

```bash
git add crates/post-machine/src/optimizer crates/post-machine/tests/opt_equivalence.rs
git commit -m "feat(post-machine): tail-merge pass — block dedup and shared adjacent returns"
```

---

### Task 6: Combined `-O1` goldens

**Files:**
- Modify: `crates/post-machine/tests/opt_equivalence.rs`, `crates/core/src/asm/disassembler.rs` (one test — Task-2 review follow-up, controller-ratified)

- [ ] **Step 0 (Task-2 review follow-up): pin the self-recursive tail jump.** Append to `crates/core/src/asm/disassembler.rs` tests:

```rust
    #[test]
    fn self_recursive_tail_jump_round_trips() {
        // A jump to one's OWN root prints in symbol form and survives
        // the round trip (Task-2 behavior expansion, empirically pinned).
        let syntax = test_syntax();
        let src = ".func main\n        jmp @main\n";
        let obj = assemble(&syntax, 0x7E, src, false).unwrap();
        let out = crate::linker::link(&syntax, &[obj], &[], crate::linker::LinkOptions::default())
            .unwrap();
        let text = disassemble_executable(&syntax, &out.executable);
        assert!(text.contains("jmp     @main"), "{text}");
        let obj2 = assemble(&syntax, 0x7E, &text, false).unwrap();
        let out2 = crate::linker::link(&syntax, &[obj2], &[], crate::linker::LinkOptions::default())
            .unwrap();
        assert_eq!(out2.executable.code, out.executable.code);
    }
```

- [ ] **Step 1: Append:**

```rust
#[test]
fn flagship_is_untouched_by_the_6b_passes() {
    // The 6a crown jewel must not move: no calls, no duplicate blocks,
    // no empty-return adjacency (b0 ends Goto, not Return).
    let out = compile(
        FLAGSHIP,
        CompileOptions {
            opt_level: OptLevel::O1,
            ..Default::default()
        },
    )
    .unwrap();
    let linked = link(&[out.object], &[], LinkOptions::default()).unwrap();
    use mtc_post_machine::arch::opcodes::*;
    assert_eq!(
        linked.executable.code,
        vec![ENT, WR, 0x81, RGT, WR, 0x80, STP]
    );
}

#[test]
fn inline_then_tail_call_compose() {
    // step() is inlined into walk(); walk()'s own trailing call to
    // itself is NOT inlined (recursion) but IS tail-converted — the
    // classic loop-from-recursion, verified terminating identically.
    let src = "\
step() { right; }
walk() { @step(); check(1, !); 1: @walk(!); }
main() { @walk(); mark; }
";
    let (o0, o1) = assert_equivalent(src, TAPES);
    assert!(o1 < o0, "{o0} -> {o1}");
    let out = compile(
        src,
        CompileOptions {
            opt_level: OptLevel::O1,
            capture_ir: true,
            ..Default::default()
        },
    )
    .unwrap();
    let stages: Vec<&str> = out.ir_snapshots.iter().map(|(s, _)| s.as_str()).collect();
    assert!(stages.contains(&"after:inline"), "{stages:?}");
    assert!(stages.contains(&"after:tail-call"), "{stages:?}");
    // The recursive call became a tail jump:
    let walk = out.ir.functions.iter().find(|f| f.name == "walk").unwrap();
    assert!(walk.blocks.iter().any(|b| matches!(
        &b.term,
        mtc_post_machine::ir::IrTerm::TailCall { name } if name == "walk"
    )));
}

#[test]
fn fno_tail_call_keeps_calls() {
    let src = "f() { right(!); } g() { left, @f(!); } main() { @g(); }";
    let out = compile(
        src,
        CompileOptions {
            opt_level: OptLevel::O1,
            disabled_passes: vec!["inline".to_string(), "tail-call".to_string()],
            ..Default::default()
        },
    )
    .unwrap();
    assert!(!out.report.opt.changes.iter().any(|c| c.pass == "tail-call"));
    let g = out.ir.functions.iter().find(|f| f.name == "g").unwrap();
    assert!(g.blocks.iter().all(|b| !matches!(
        b.term,
        mtc_post_machine::ir::IrTerm::TailCall { .. }
    )));
}
```

Note on `inline_then_tail_call_compose`: `walk`'s equivalence run — on tapes where `walk` recurses many times, `-O0` may hit StackOverflow while `-O1` loops to StepLimit; the TAPES matrix runs at head positions where the walk terminates quickly (blank within a few cells), so outcomes agree. If a tape/head combination diverges by resource trap, that is the documented exception — if it happens, assert outcomes individually per tape rather than weakening: split the offending tape into its own `#[test]` documenting the trap difference, and BLOCK to report which tape it was so the plan records it.

- [ ] **Step 2: Full gates, then commit.**

```bash
git add crates/post-machine/tests/opt_equivalence.rs crates/core/src/asm/disassembler.rs
git commit -m "test: 6b combined goldens — flagship untouched, inline+tail-call composition, opt-outs, self-jump pin"
```

---

## Plan Self-Review Notes

- **Spec coverage:** §8 passes 5/7/8 complete `-O1`; §6.4 gains the `jmp @name` form end-to-end (assemble → relax → disassemble → round-trip); §7.1 IR v2. Cross-module (link-time) inlining stays a designed extension per spec — not here.
- **Order-of-operations audit:** inline runs first (dissolves barriers), tail-call last (a lowering decision); tail-merge sits before dce so retargeted duplicates are collected the same round. A function converted to TailCall in round N is invisible to inline in round N+1 (candidates must be leaves; TailCall disqualifies) — a missed optimization, never an error; documented.
- **Derived bytes triple-checked:** tail-call golden 13 bytes (`6 + 4 + 3`; call.s +3, jmp.s 0); spec-sample inline golden 14 vs 18; tail-merge spec example 6 vs 7 (jm.s +3). Rounds deliberately NOT pinned anywhere in 6b (lesson from both 6a BLOCKED escalations).
- **`IrTerm` Copy-loss** is called out as a constraint with the mechanical migration recipe; the compiler enumerates every site, so nothing can be silently missed.
- **Known accepted risks:** inline changes stack headroom and tail-call changes resource-trap kinds — both covered by the §8 amendment and pinned by `self_recursive_tail_call_becomes_an_in_place_loop`. Dedup drops the duplicate's source labels from debug info (optimized `-g` is approximate by spec). `relaxed_calls`/`far_calls` field names now also count tail jumps — doc-noted, rename deferred to a future breaking pass.


