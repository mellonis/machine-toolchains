# Binding Arc, Phase 2 — Linker Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Teach the link stage to RESOLVE the symbolic binding forms phase 1 taught the object format and the assembler to carry — parameter names, glyph labels, open maps and exit vectors — plus the checks that ride with them (graft drift, plain-site sizing, the omitted-map glyph warning) and the link-diagnostics infrastructure those warnings need, with every existing program's image byte-identical.

**Architecture:** A resolution **pre-pass** runs in `link()` between `resolve` and `engine::lower`, turning each reachable bound call's symbolic record into a numeric one stored in an **arena** the `FuncRef`s are re-pointed at; nothing below the pre-pass ever sees `param`, `dst_label` or an unresolved label again. Exits thread through the existing raw-descriptor exit path under frames and through a new per-site `jmp`-entered copy under mono. Everything lands in `crates/core` and is proven on the crate-private fake dialects; the TM crate gains only the CLI surface for link warnings and a `.tma`-driven three-mechanism run matrix.

**Tech Stack:** Rust (pinned toolchain in `rust-toolchain.toml`), `proptest` for the format round trips, `cargo nextest run --workspace` as the final gate, no new dependencies.

**Spec:** `docs/superpowers/specs/2026-09-13-issue-95-binding-analysis.md` — "Item 4", "### Predicates" (P1–P4, P6, P8, P9), the "**Linker.**" paragraph of "### By layer", the "Open bindings — `*` in a map, the `opaque` bit" paragraph folded in from #121, and phase 2's line under "### Versions, sequencing, gates".

**Predecessor:** `docs/superpowers/plans/2026-09-13-binding-arc-phase-1-format-and-assembler.md` (landed; the code is the authority now).

**Reference map gathered for this plan:** `.superpowers/linker-map-2026-09-14.md` (file:line for everything below; verify before editing — the working tree carries an uncommitted split of `crates/core/src/formats/object.rs` into `crates/core/src/formats/object/{mod,read,write,tests}.rs` and every path here uses the split form).

## Rulings carried in (do not re-open)

Ratified 2026-09-13 with the arc, and again for this phase:

- Decimal digests; one space after a directive word; `writes=()` never printed; a digest line obliges a full interface; only a written-empty map promotes to v4; **a labelled pair carries `dst` 0** (the wire gives that field to the label, so writing a resolved index back into the `ObjectFile` would break `from_bytes(to_bytes(x)) == x`); **`open ⇒ map_written`**; a binding names every entry or none; **exits are blob-relative caller offsets**; dis names exit labels by its synthesized scheme.
- `refuse_symbolic_binding` (`crates/core/src/linker/engine.rs:641-671`) is this phase's deletion target, **form by form** — the last task of the resolution arc removes it, and every intermediate commit stays green.
- The imported-alphabet record (Task 15) is a **proposed wire addition**; MO 4 is unreleased so nothing bumps. It is marked PULLABLE — the controller may defer it without touching any other task.
- No `--hybrid-min-saving` knob in this phase.
- Nothing from phase 3 (compiler, `.tmh` headers, `state` parameters, `.tmc` changes) or phase 4 (`outline`, LSP, stdlib rewrite, CHANGELOG) enters this plan.

## Global Constraints

**Every task's requirements implicitly include this section. Re-read it before each task.**

- **PM-1 byte identity.** No byte of any `pmt` output may change. PM-1 objects carry no bound calls, no interface section and no grafts, so every new linker path must be unreachable for them; the PM CLI text is unchanged. Re-check deliberately on any change to `crates/core`: `cargo test -p mtc-post-machine --test golden_programs` and `cargo test -p mtc-post-machine --test asm_volatile`.
- **Core neutrality.** `crates/core` learns nothing about TM-1 or PM-1. Core tests use the crate-private fake arch (`vm/arch.rs::test_arch`, arch id `0x7F`) and the per-file fake dialects the link tests already define (`crates/core/tests/link_interface.rs::fake_syntax`, `crates/core/tests/link_tables.rs`). A new `ArchSyntax` field is a dialect TABLE entry, never architecture knowledge.
- **`-O0` bit identity and the `brk` barrier are untouched** — no optimizer change anywhere in this phase.
- **Drift guards are set-compares in BOTH directions**: the error/warning code registries against the `docs/tmt/cli.md` tables (`crates/turing-machine/tests/error_code_docs.rs`, `crates/core/tests/error_code_docs.rs`); the completions registry against the real parser (`crates/turing-machine/src/completions/registry.rs`, `crates/turing-machine/tests/completions_registry.rs`); `cli_docs` quoting `--help` verbatim (`crates/turing-machine/tests/cli_docs.rs:52`, which pins `docs/tmt/cli.md:277-291` byte-for-byte against `LINK_USAGE`); the man page (rendered from `cli::usage_text`, so an edited `LINK_USAGE` propagates automatically).
- **The composition algebra's law tests stay green** (`crates/core/src/linker/compose.rs:1211-1284`: `compose_matches_step_by_step_simulation`, `composition_is_associative`, `identity_laws_hold`, `canonicalize_stable_under_repetition`). **The everything-matrix stays green** (`crates/turing-machine/tests/opt_equivalence.rs::everything_matrix_is_green`). **The three mechanisms stay equivalent on every existing program** (`crates/turing-machine/tests/mode_equivalence.rs`).
- **Docs policy.** Published pages (`docs/core.md`, `docs/formats.md`, `docs/tmt/*.md`) and code comments cite `docs/<page>.md (keyword)` only — no `spec §N`, no `Task N`, no issue/PR numbers, no hosting URLs. This plan is an internal artifact and may cite freely.
- **Forward citations are accepted within this phase** (the same ruling phase 1 ran under): a code comment may cite `docs/core.md (symbolic resolution)`, `(link warnings)`, `(graft drift)` or `(call mechanisms)` before Tasks 10, 13 and 17 land those sections. **The final whole-branch review checks that every citation resolves to a real page-plus-keyword** — a citation still dangling when the branch is reviewed is a defect, not a deferred nicety.
- **Commits.** Conventional commits with scope (`feat(core):`, `fix(core):`, `test(core):`, `docs(core):`, `feat(turing-machine):`). Implementer subagents MAY commit their own task on branch `binding-arc-2`; merging and pushing stay the owner's. **Commit messages carry no Claude attribution and no `Claude-Session:` line** — the harness appends one, so every commit step ends with: run `git log -1 --format=%B`, and if a `Claude-Session:` or `Generated with Claude Code` line is present, `git commit --amend` with the message stripped back to the intended text.
- **`git add` explicit paths, never `-A` or `.`** — a task commit carries its own files only; anything else in the working tree (the SDD workspace, scratch, another task's leftovers) must never be swept into it.
- **Temp paths in tests**: PID plus a per-call atomic counter, never a fixed name. Copy `crates/turing-machine/tests/mode_equivalence.rs:902-917` (`fn scratch`) verbatim into any new TM test file that writes to disk.
- **Every fixture must assemble before it is trusted.** TM-side `.tma` fixtures in this plan were run through `cargo run -q -p mtc-turing-machine --bin tmt -- asm` and are marked **[tool-verified]**. Core-side fake-dialect fixtures are not reachable from a CLI; they are marked **[shape-copied]** and were built by copying the shapes of `crates/core/tests/link_interface.rs`, `crates/core/tests/asm_interface.rs` and `crates/core/tests/link_tables.rs`, which do assemble in CI. A fixture that does not assemble is a plan defect — report it, do not paper over it.
- **Every test states, in one line, the mutation it catches.** A fixture that merely parses and passes proves nothing.
- **Cargo invocations** are prefixed `CARGO_TARGET_DIR=/Users/mellonis/Developer/mellonis-workspace/machines/toolchains/target` and run from the worktree root.

---

## Established facts this plan builds on

Verified against the working tree on 2026-09-14. An implementer may rely on these without re-deriving them.

1. **The arena design compiles.** `FuncRef<'a>` is covariant in `'a`, so a `Vec<FuncRef<'obj>>` passed by value into a function taking `Vec<FuncRef<'arena>>` coerces when `'obj: 'arena`. A throwaway edit adding `resolve_bindings`/`rebind` to `engine.rs` and calling them from `link()` compiled clean under `cargo check -p mtc-core`. Do **not** convert `FuncRef.bound` or `SiteKind` to owned data — the arena is smaller and keeps every existing signature.
2. **The hybrid re-scan is the reason the arena must rewrite `FuncRef.bound`, not a side table.** `stamp.rs:351` calls `scan_sites` a second time over `new_order`, and `prune_unreachable` (`stamp.rs:442-498`) reindexes the order — a side table keyed by `(func_idx, addr)` would silently go stale and the frames leg would see symbolic records again.
3. **No compiled `.tmc` object carries an `Interface` in this phase.** The TM compiler emits no `.param` (grep of `crates/turing-machine/src/codegen.rs` for `.param`: zero hits); only the assembler writes interface sections. So the glyph-level checks of Task 12 are structurally dormant on the shipped corpus until phase 3, and the only live plain-site comparison is arity/cardinality from `RoutineSig`.
4. **The four shipped plain `call std::` sites are cardinality-equal.** `docs/examples/rpn/rpn.tmc:75,82,90,99` call `std::binaryNumbers::*` from routines typed `tape n: bin` where `alphabet bin { '_', '^', '$', '0', '1' }` (5 glyphs) matches `std::binaryNumbers::symbols` (5 glyphs). Task 1 still runs the sweep over the whole corpus — this is a prior, not a substitute.
5. **A cross-object bound call already links today in its NUMERIC form.** Verified end to end: `tmt asm app_num.tma`, `tmt asm mylib.tma`, `tmt link app_num.tmo mylib.tmo --nostdlib --call-mech frames -v` → `1 composite(s), 0 stamp(s), 4 B compose table`. The symbolic form is refused with `bad binding to 'mylib::plusOne': the call site uses a named entry, ...`. That pair is Task 14's reference image.
6. **`ArchSyntax` has no return opcode and it cannot be inferred** — `ret`, `stp` and `hlt` are all `OperandKind::None` + `Flow::Stop`. `trap_opcode`'s own doc comment (`crates/core/src/asm/syntax.rs:80-89`) states the precedent for declaring such an opcode explicitly per dialect. There are **36 `ArchSyntax { … }` literals** across `crates/core/src/asm/{syntax,cst,lower,assembler,disassembler,fmt}.rs`, `crates/core/src/asm/lint/{mod,rules/unused_label,rules/leftover_debugger}.rs`, `crates/core/src/linker/layout.rs`, `crates/core/tests/{asm_tables,asm_interface,link_interface,link_variants,link_tables}.rs`, `crates/post-machine/src/asm/mod.rs` and `crates/turing-machine/src/asm/mod.rs`.
7. **`emit_planned_region` runs after layout's per-function code loop** (`crates/core/src/linker/layout.rs:791-802` calling `:876`), where the per-iteration `orig_to_new` map is already out of scope. Exit rebasing needs that map retained.
8. **The exits of a `BoundCall` are relative to the ORIGINAL blob** (`crates/core/src/formats/object/mod.rs:224-226`), and `rewrite_blob` widens framed bound sites 5 → 9 bytes, shifting every later offset by `+4` per preceding widened site (`crates/core/src/linker/engine.rs:858-861`).

---

## File structure

| File | Responsibility in this phase |
|---|---|
| `crates/core/src/linker/interface.rs` | **NEW.** The resolution pre-pass (symbolic → numeric bindings, the arena, the `opaque` and exit-count checks), graft-drift and imported-alphabet checks. Everything that reads `Interface`/`RoutineInterface`. |
| `crates/core/src/linker/mod.rs` | `LinkError` gains `OpenBindingUnsupported`, `GraftDrift`, `CalleeWider`, `AlphabetDrift`; `LinkDiagnostic` + `DIAGNOSTIC_CODES` + `LinkReport.diagnostics`/`.folds`; `link()` calls the pre-pass and the object-level checks |
| `crates/core/src/linker/resolve.rs` | `FuncRef` gains `interface: Option<&'a RoutineInterface>` and `site_fixups` |
| `crates/core/src/linker/engine.rs` | `LoweredOrder` → `struct Lowered`; exits threaded into the frames closure, the intern key and `materialize`; `refuse_symbolic_binding` deleted; `check_sites` (plain-site + omitted-map diagnostics); `is_full_passthrough` conjoined with `exits.is_empty()` |
| `crates/core/src/linker/stamp.rs` | Mono's exit-bearing per-site copy (`jmp` entry, `ret`/`retx` rewrite), the stamp key `(routine, composite, exits, then)`, hybrid's exit-bearing byte-saving fold |
| `crates/core/src/linker/compose.rs` | The open-binding rule: an `open` tape maps unlisted caller symbols one-way onto the opaque index `callee_card` instead of holing them |
| `crates/core/src/linker/layout.rs` | Retained per-function offset maps; engine-descriptor exit rebasing; `site_fixups` patching |
| `crates/core/src/asm/syntax.rs` | `ArchSyntax.return_opcode: Option<u8>`, `ArchSyntax::jump_opcode()` |
| `crates/core/src/formats/object/{mod,write,read,tests}.rs` | `Interface.imports` + `ImportedAlphabet` (Task 15, PULLABLE); the v4 reader's `map_written` normalization (Task 14) |
| `crates/core/tests/link_interface.rs` | The four refusal tests flip one at a time into resolution tests; the reachability test is re-pinned |
| `crates/core/tests/link_resolution.rs` | **NEW.** Resolution unit coverage: reorder, glyph labels, every `BadBinding` case |
| `crates/core/tests/link_exits.rs` | **NEW.** Exit vectors under frames, mono and hybrid on the fake dialect |
| `crates/core/tests/link_open.rs` | **NEW.** Open bindings under all three mechanisms; the `opaque` refusal |
| `crates/core/tests/link_checks.rs` | **NEW.** Plain-site sizing, the omitted-map warnings, graft drift, imported-alphabet drift |
| `crates/core/tests/link_cross_object.rs` | **NEW.** A caller object and a callee object, numeric and symbolic, three mechanisms |
| `crates/core/tests/format_roundtrips.rs` | proptest coverage for `Interface.imports` (Task 15) |
| `crates/core/tests/error_code_docs.rs` | The link-warning code table ↔ `docs/core.md` set-compare |
| `crates/turing-machine/src/asm/mod.rs` | `tm1_syntax()` gains `return_opcode: Some(RET)` |
| `crates/turing-machine/src/cli/build.rs` | `tmt link` gains `--allow`/`-Werror`; `render_link_report` + `render_link_diagnostics` factored out |
| `crates/turing-machine/src/cli/driver.rs` | `tmt build`'s `-Werror` covers the link stage; manifest `lint.allow` unions into the link allow list |
| `crates/turing-machine/src/lint/mod.rs` | `known_code` gains the fifth surface |
| `crates/turing-machine/src/completions/registry.rs` | `link_spec()` gains `--allow` (repeatable) and `-Werror` |
| `crates/turing-machine/tests/link_matrix.rs` | **CREATED by Task 8b** (its harness — the `mono_run.rs` `build`/`run`/`cell_at` helpers, `MECHS`, and the closure-fold run test); **appended to by Task 16** with the exits/open/cross-object/mixed fixtures. Task 16 keeps 8b's fixtures. |
| `crates/turing-machine/tests/mode_equivalence.rs` | The relink byte-identity sweep gains the three new programs |
| `crates/turing-machine/tests/plain_site_sweep.rs` | **NEW, `#[ignore]`d.** The corpus sweep instrument (Task 1) |
| `docs/core.md` | The linker sections: symbolic resolution, graft drift, link diagnostics, the hybrid fold rule, the link-warning code table |
| `docs/formats.md` | "What resolves them, and when." becomes present tense; the open-binding and exit semantics |
| `docs/tmt/cli.md` | `tmt link`'s new flags and the `### Link warnings` table |
| `docs/tmt/lint.md` | The fifth allow surface |

---

### Task 1: The plain-site corpus sweep (a gated finding, no behaviour change)

**Why first:** Task 12 turns "a callee wider in tape count or alphabet" into a hard link error. Today plain call sites are checked for *nothing* (`crates/core/src/linker/engine.rs:583-588` pushes `SiteKind::Plain` and moves on), so this is the one genuine behaviour change in the phase. This task measures it before anybody implements it.

**`SiteKind::Plain` is wider than "a call".** `scan_sites` pushes it for a relocated plain call AND for a relocated tail jump or conditional branch into another function (`crates/core/src/linker/engine.rs:595-609`) — the tail-call optimizer turns calls into jumps, and the hazard is identical either way: control reaches the callee's body over the caller's bands. So Task 12 grades those edges too, and this sweep must cover them. It does, and not by accident: it walks `obj.relocations`, which is every symbol reference in the blob regardless of the instruction that consumes it — calls, tail jumps and branches alike. Do not narrow it to call sites.

**Files:**
- Create: `crates/turing-machine/tests/plain_site_sweep.rs`

**Interfaces:**
- Consumes: nothing.
- Produces: a recorded finding pasted into this task's checkbox notes, and a STOP decision. Nothing in `src/`.

- [x] **Step 1: Write the sweep instrument**

Create `crates/turing-machine/tests/plain_site_sweep.rs`:

```rust
//! An `#[ignore]`d measurement instrument, not a correctness test: it links
//! every shipped program and reports what the planned plain-site size check
//! WOULD say, so the check's blast radius is known before it becomes an
//! error. Run it with
//! `cargo test -p mtc-turing-machine --test plain_site_sweep -- --ignored --nocapture`.

use std::fs;
use std::path::{Path, PathBuf};

use mtc_core::formats::object::{ObjectFile, RoutineSig};
use mtc_turing_machine::asm::assemble;
use mtc_turing_machine::compiler::{CompileOptions, compile};

/// Every `.tmc` and `.tma` the repository ships, recursively, under the
/// three roots that hold real programs.
fn corpus() -> Vec<PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap();
    let mut out = Vec::new();
    for dir in [
        root.join("docs/examples"),
        root.join("crates/turing-machine/tests/golden"),
        root.join("crates/turing-machine/src/stdlib"),
    ] {
        collect(&dir, &mut out);
    }
    out.sort();
    out
}

fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            collect(&p, out);
        } else if matches!(
            p.extension().and_then(|s| s.to_str()),
            Some("tmc") | Some("tma")
        ) {
            out.push(p);
        }
    }
}

/// Every plain (relocation) call site's caller and callee signatures.
///
/// **The callee is looked up ACROSS the unit and the stdlib**, not just
/// inside the object: every `call std::…` in the shipped corpus is an
/// EXTERNAL symbol, so a lookup confined to one object would report
/// nothing about the exact sites this sweep exists to measure. `extra`
/// is the stdlib object (and, for a multi-unit target, its siblings),
/// searched after the object itself — the linker's own first-wins order.
/// Reported, never asserted.
fn report(path: &Path, obj: &ObjectFile, extra: &[&ObjectFile]) {
    let Some(sigs) = obj.signatures.as_ref() else {
        println!("{}: no signatures (nothing to compare)", path.display());
        return;
    };
    let mut resolved = 0usize;
    let mut unresolved = 0usize;
    for reloc in &obj.relocations {
        let caller: &RoutineSig = &sigs[reloc.blob as usize];
        let name = &obj.symbols[reloc.symbol as usize].name;
        let find = |o: &ObjectFile| -> Option<RoutineSig> {
            let blob = o.symbols.iter().find_map(|s| match s.def {
                mtc_core::formats::object::SymbolDef::Defined { blob }
                | mtc_core::formats::object::SymbolDef::Local { blob }
                    if s.name == *name =>
                {
                    Some(blob)
                }
                _ => None,
            })?;
            o.signatures.as_ref()?.get(blob as usize).cloned()
        };
        let Some(callee) = find(obj).or_else(|| extra.iter().find_map(|o| find(o))) else {
            unresolved += 1;
            println!(
                "UNRESOLVED       {}: blob {} -> `{name}` (no signature in the unit or \
                 the stdlib)",
                path.display(),
                reloc.blob
            );
            continue;
        };
        resolved += 1;
        let callee = &callee;
        let wider_tapes = callee.arity > caller.arity;
        let wider_alpha = callee
            .cardinalities
            .iter()
            .zip(&caller.cardinalities)
            .any(|(c, k)| c > k);
        let narrower_alpha = callee
            .cardinalities
            .iter()
            .zip(&caller.cardinalities)
            .any(|(c, k)| c < k);
        if wider_tapes || wider_alpha {
            println!(
                "ERROR-WOULD-FIRE {}: blob {} -> `{name}` caller {:?} callee {:?}",
                path.display(),
                reloc.blob,
                caller.cardinalities,
                callee.cardinalities
            );
        } else if narrower_alpha {
            println!(
                "WARN-WOULD-FIRE  {}: blob {} -> `{name}` caller {:?} callee {:?}",
                path.display(),
                reloc.blob,
                caller.cardinalities,
                callee.cardinalities
            );
        }
    }
    println!(
        "{}: {resolved} call site(s) compared, {unresolved} unresolved",
        path.display()
    );
}

#[test]
#[ignore = "measurement instrument; run explicitly with --ignored --nocapture"]
fn sweep_the_shipped_corpus() {
    // The embedded stdlib is where every `call std::…` in the corpus
    // resolves, so it is part of the comparison, not a separate concern.
    let stdlib = mtc_turing_machine::stdlib::object().clone();
    let mut compiled: Vec<(PathBuf, ObjectFile)> = Vec::new();
    for path in corpus() {
        let src = fs::read_to_string(&path).expect("readable");
        let obj = if path.extension().and_then(|s| s.to_str()) == Some("tma") {
            match assemble(&src, false) {
                Ok(o) => o,
                Err(e) => {
                    println!("{}: does not assemble standalone ({e})", path.display());
                    continue;
                }
            }
        } else {
            match compile(&src, CompileOptions::default()) {
                Ok(out) => out.object,
                Err(e) => {
                    println!("{}: does not compile standalone ({e:?})", path.display());
                    continue;
                }
            }
        };
        compiled.push((path, obj));
    }
    // A multi-unit target's sibling sources resolve each other, so every
    // compiled unit is a candidate callee holder for every other one —
    // over-broad by design, since a false RESOLUTION here only widens what
    // the sweep reports, and a missed one hides a finding.
    for (path, obj) in &compiled {
        let mut extra: Vec<&ObjectFile> = compiled
            .iter()
            .filter(|(p, _)| p != path)
            .map(|(_, o)| o)
            .collect();
        extra.push(&stdlib);
        report(path, obj, &extra);
    }
    println!("--- sweep complete ---");
}
```

- [x] **Step 2: Run it and record the output**

Run: `CARGO_TARGET_DIR=/Users/mellonis/Developer/mellonis-workspace/machines/toolchains/target cargo test -p mtc-turing-machine --test plain_site_sweep -- --ignored --nocapture`

Paste the full `ERROR-WOULD-FIRE` and `WARN-WOULD-FIRE` line count and the lines themselves into this task's notes in the plan file, under a `**Sweep result (YYYY-MM-DD):**` heading.

**A sweep that resolved no external callee is a FAILED sweep, not a clean one.** The per-file tail line reports how many sites were compared and how many were unresolved; if `docs/examples/rpn/rpn.tmc` shows zero resolved sites, the stdlib lookup is not working and the result says nothing about the four `call std::binaryNumbers::*` sites this measurement exists for. Fix the lookup and re-run before recording anything. Record the total compared/unresolved counts alongside the findings.

If `mtc_turing_machine::stdlib::object()` is not public, use whatever the crate exposes (`crates/turing-machine/src/stdlib/mod.rs:57` holds the `OnceLock`); if nothing is, make the sweep compile `crates/turing-machine/src/stdlib/std.tmc` itself and say so in the notes.

- [x] **Step 3: Audit core's own link fixtures by hand**

Grep the fixtures for plain sites whose callee is wider:

Run: `grep -n "^\.routine" crates/core/tests/link_tables.rs crates/core/tests/link_variants.rs crates/turing-machine/tests/mode_equivalence.rs crates/turing-machine/tests/mono_run.rs crates/turing-machine/tests/composition_engine.rs`

Record in the notes every fixture where a `.routine` declared `alpha=(…)` is WIDER than the `.routine` that plainly calls it. Note explicitly that a *bound* site with an explicit map is not a candidate — the omitted-map check only fires on `map_written == false`.

- [x] **Step 4: The stop gate**

**If any `ERROR-WOULD-FIRE` line appears in either sweep, STOP.** Do not start Task 12. Report to the controller: the offending file, the two signatures, and which of the two fallbacks the finding argues for —

- **(a) fix the program** — when the finding is a real latent bug (the callee genuinely addresses a band or symbol the caller lacks);
- **(b) narrow the rule** — restrict the wider-callee ERROR to plain sites only and leave a bound site with an omitted map on its existing hole-and-trap behaviour, since `binding_to_composite` already gives that case a defined runtime meaning.

If no `ERROR-WOULD-FIRE` line appears, record "clean" and continue. Record the `WARN-WOULD-FIRE` count either way: a corpus that would emit hundreds of new warnings is itself a controller decision (default-on versus shipped `--allow`), even though nothing breaks.

**Sweep result (2026-09-14):**

Ran `CARGO_TARGET_DIR=/Users/mellonis/Developer/mellonis-workspace/machines/toolchains/target cargo test -p mtc-turing-machine --test plain_site_sweep -- --ignored --nocapture`. Full per-file output:

```
crates/turing-machine/src/stdlib/std.tmc: 0 call site(s) compared, 0 unresolved
crates/turing-machine/tests/golden/a1_replace_b.tmc: 0 call site(s) compared, 0 unresolved
crates/turing-machine/tests/golden/a2_binary_plus_one.tmc: 0 call site(s) compared, 0 unresolved
crates/turing-machine/tests/golden/a3_two_tape_copy.tmc: 0 call site(s) compared, 0 unresolved
crates/turing-machine/tests/golden/a4_byte_increment.tmc: 0 call site(s) compared, 0 unresolved
crates/turing-machine/tests/golden/a5_call_across_alphabets.tmc: 0 call site(s) compared, 0 unresolved
crates/turing-machine/tests/golden/a6_graph_graft_multi_exit.tmc: 0 call site(s) compared, 0 unresolved
crates/turing-machine/tests/golden/nested_graft.tmc: 0 call site(s) compared, 0 unresolved
docs/examples/brainfuck-utm/brainfuck-utm-handwritten.tma: 0 call site(s) compared, 0 unresolved
docs/examples/brainfuck-utm/brainfuck-utm.tmc: 0 call site(s) compared, 0 unresolved
docs/examples/pow2/pow2.tmc: 0 call site(s) compared, 0 unresolved
docs/examples/rpn/rpn.tmc: 4 call site(s) compared, 0 unresolved
docs/examples/rpnhex/rpnhex.tmc: 0 call site(s) compared, 0 unresolved
docs/examples/rpnreg/rpnreg.tmc: 0 call site(s) compared, 0 unresolved
docs/examples/rpnwide/rpnwide.tmc: 0 call site(s) compared, 0 unresolved
--- sweep complete ---
test sweep_the_shipped_corpus ... ok
```

No `ERROR-WOULD-FIRE` line and no `WARN-WOULD-FIRE` line anywhere — zero of each. Totals: **4 sites compared, 0 unresolved** across the whole corpus, all four on `docs/examples/rpn/rpn.tmc` (the four `call std::binaryNumbers::*` sites the stdlib lookup exists to resolve — confirming the lookup works, per the stop-gate check above). Confirms the prediction already on record above (item 4 in this file's own prior notes): the four shipped `call std::` sites are cardinality-equal to their callers.

**Framed calls are excluded from the count by opcode, not just by the bound-call carve-out.** A framed call's displacement half is emitted as a relocation shaped exactly like a plain call's (`crates/core/src/asm/assembler.rs:900-905`: `Slot::FramedCall` pushes the opcode byte, then a relocation at the next offset, before the frame-half table-ref hole) — so a hand-authored `call.m` in a future corpus would be graded as a plain site if the sweep only walked `obj.relocations` blindly. The instrument now decodes the opcode byte at `reloc.offset - 1` for every relocation and skips it when it equals `tm1_syntax().framed_call_opcode()`, the same exclusion the brief already gives an explicit map on a bound site. Re-ran the sweep after the fix: **the counts did not change** — there is no `call.m` anywhere under the three swept roots (`docs/examples`, `crates/turing-machine/tests/golden`, `crates/turing-machine/src/stdlib`), so this exclusion is currently inert but guards the instrument against silently mis-grading one the day a hand-written `.tma` example uses `call.m`.

**Why every other file shows zero, and the caveats on this "clean":**

- Measured under `CompileOptions::default()` (`opt_level: OptLevel::O0`, the `#[default]` variant). This is not merely "inline didn't run": at `-O1` the optimizer can only **inline** a callee (removing its site from `obj.relocations` entirely) or **turn a call into a tail jump** to the same callee (still the same caller/callee pair, still graded on the same signature comparison, since `SiteKind::Plain` already covers tail jumps and branches, not only calls). Neither transformation can introduce a new caller/callee pair or change a comparison's outcome — it can only remove a site or re-shape how an existing one is reached. So the `-O0` site set this sweep measures is a **superset** of the `-O1` one, and the "zero wider, zero unresolved-surprise" finding holds at `-O1` too; this run does not, however, exercise the tail-jump *mechanism* itself (no shipped example both calls another function AND gets tail-call-optimized under `-O1` within the swept roots), which is fine precisely because that mechanism adds no new pairs to grade, only a different instruction shape for a pair already covered.
- The instrument walks `obj.relocations` only. `rpnhex.tmc`, `rpnreg.tmc`, `rpnwide.tmc` and `a5_call_across_alphabets.tmc` all call other routines in source (`call pushToken(...)`, `call plusOne(...)`, …) — those are NOT absent from the object, they compile to declarative **bound calls** (explicit parameter/tape maps) and land in `obj.bound_calls`, a field this sweep never reads. The step-3 hand audit shows the same split independently: the great majority of `.tma`-fixture call sites carry an explicit `[…]` binding map and are excluded from the relocation-based check by the brief's own carve-out. So "zero plain-relocation sites" in most of the corpus reflects the corpus being bound-call-heavy, not an absence of cross-function control flow — and it is a real, not an assumed, zero for what this instrument measures (`SiteKind::Plain`: transparent calls, and any relocated tail jump/branch into another function — framed calls now excluded by opcode, per above).
- **The `WARN-WOULD-FIRE` zero covers only the narrow-alphabet-on-plain-sites rule.** It says nothing about Task 12's other warn-tier rule, the omitted-map warning on a *bound* site with `map_written == false` — this instrument never inspects `obj.bound_calls` or `map_written`, so that rule's blast radius is unmeasured here. The "0, not hundreds" default-on-vs-`--allow` question in Step 4 is answered only for the plain-site narrow-alphabet warning, not for the omitted-map one.

**Step 3 hand audit:** ran the `grep -n "^\.routine" …` command over all five files (`link_variants.rs` has no `.routine` declarations to audit — 0 matches). Parsed every `.func`/`call` pair across the five files programmatically to separate plain calls (no `[…]` binding brackets) from bound calls with an explicit map. Of 92 total `call`/`call.*` sites across the fixtures, 82 carry an explicit binding map (`[…]`) and are excluded per the brief's own note — not candidates, since the omitted-map check only fires on `map_written == false`. Of the remaining 10, **one is a framed call, not a plain site**: `composition_engine.rs:148` is a hand-authored `call.m  leaf, Fr` (no `[…]` map, so the earlier grep-based classification miscounted it as plain) — excluded on the same opcode grounds as the sweep fix above, since a `call.m` site is graded by the composition algebra, not this check. That leaves **9 plain sites**, caller → callee cardinalities:

| site | caller | callee |
|---|---|---|
| `link_tables.rs:205` | `main[2]` | `helper[2]` |
| `link_tables.rs:247` | `main[2]` | `helper[2]` |
| `link_tables.rs:446` | `main[2]` | `helper[2]` |
| `link_tables.rs:484` | `main[2]` | `helper[2]` |
| `link_tables.rs:1477` | `main[4, 4]` | `apiB[4, 4]` (cross-object, `link_tables.rs:1482`) |
| `link_tables.rs:1561` | `sub[4, 4]` | `{stamp}[4, 4]` |
| `link_tables.rs:2664` | `main[4]` | `A[4]` |
| `link_tables.rs:2665` | `main[4]` | `B[4]` |
| `mode_equivalence.rs:857` | `M[4]` | `P[4]` |

Excluded, not a plain-site candidate: `composition_engine.rs:148` (`r[4]` calls `leaf[4]` via `call.m … Fr` — a framed call under the composition algebra).

None of the 9 is wider — every plain callee is exactly as wide, in both arity and every cardinality, as its caller. The one apparent wider-callee pair in the whole grep output, `link_tables.rs:1183-1184` (`main, alpha=(4,4)` / `sub, alpha=(8,8)`, in `an_out_of_range_caller_symbol_is_a_link_error`), is a **bound** site with an explicit map (`call sub [0{5->1}, 1]`) — excluded per the brief's own carve-out, and in fact is itself a test that a too-narrow caller-side binding index is already a link error today, unrelated to Task 12's callee-width check.

**Stop-gate verdict: clean.** Zero `ERROR-WOULD-FIRE` lines in the corpus sweep, zero widening plain sites in the hand audit. No `WARN-WOULD-FIRE` lines either (0, not "hundreds" — the shipped corpus has almost no plain-relocation sites at all outside the four `std::binaryNumbers` calls). Task 12 is clear to proceed once the controller reviews this record.

- [ ] **Step 5: Commit**

```bash
git add crates/turing-machine/tests/plain_site_sweep.rs docs/superpowers/plans/2026-09-14-binding-arc-phase-2-linker.md
git commit -m "test(turing-machine): the plain-site sweep instrument and its recorded finding"
```

Then run `git log -1 --format=%B` and `git commit --amend` if a `Claude-Session:` line was appended.

---

### Task 2: The resolution arena, `FuncRef.interface`, and `struct Lowered`

**No behaviour change.** The pre-pass is wired in and copies every record verbatim; `refuse_symbolic_binding` still stands in front of it, so every existing test — including the four refusals — stays exactly as green as it is now. This task exists so that Tasks 3–8 each add one resolution rule to a structure that already compiles.

**Files:**
- Create: `crates/core/src/linker/interface.rs`
- Modify: `crates/core/src/linker/mod.rs:4-9` (module list), `:398-442` (`link`)
- Modify: `crates/core/src/linker/resolve.rs:169-200` (`FuncRef`), `:450` area (the `FuncRef` construction in the `order` map)
- Modify: `crates/core/src/linker/engine.rs:119-124` (`LoweredOrder`), `:152-185` (`lower`), `:192-197` + `:402-415` (`lower_frames` returns)
- Modify: `crates/core/src/linker/stamp.rs:124-243`, `:253-363` (the four `Ok((…))` returns)

**Interfaces:**
- Produces, for every later task:
  - `pub(crate) struct FuncRef<'a> { …, pub interface: Option<&'a RoutineInterface>, … }` — the callee's per-blob interface record, indexed exactly like `signature` is.
  - `pub(super) fn interface::resolve_bindings(order: &[FuncRef]) -> Result<Vec<Vec<BoundCall>>, LinkError>` — one resolved `BoundCall` per entry of each `FuncRef::bound`, in the same order.
  - `pub(super) fn interface::rebind<'a>(order: Vec<FuncRef<'a>>, arena: &'a [Vec<BoundCall>]) -> Vec<FuncRef<'a>>` — re-points every `FuncRef::bound` entry at its arena record. Takes `order` **by value** so the covariance coercion applies; `&mut Vec<FuncRef<'a>>` would be invariant and would not compile.
  - ```rust
    pub(super) struct Lowered<'a> {
        pub order: Vec<FuncRef<'a>>,
        pub plan: Option<FramesPlan>,
        pub stats: EngineStats,
        pub orphaned: Vec<String>,
        pub diagnostics: Vec<LinkDiagnostic>,
        pub folds: Vec<FoldDecision>,
    }
    ```
    replacing the `LoweredOrder` 4-tuple. `LinkDiagnostic` and `FoldDecision` are introduced empty here (Tasks 10 and 8 fill them).

- [ ] **Step 1: Write the failing test**

The regression floor for this task is **the existing `cargo test -p mtc-core` suite** — its link tests already assert report numbers and image properties over the whole bound-call surface, so a pre-pass that corrupted a record would break them. Linking one source twice and comparing proves nothing extra, so this task's own new test targets the one thing the suite cannot see: that the records the engine reads are *the arena's*, not the objects'.

Add a `#[cfg(test)] mod tests` at the end of `crates/core/src/linker/interface.rs` (a unit test — it needs `FuncRef` and the arena, both private to the crate):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    /// The pre-pass is the identity on a purely numeric binding, and
    /// `rebind` really re-points the `FuncRef`s at the arena: after it,
    /// every `bound` entry's record is the arena's own allocation, not
    /// the object's.
    ///
    /// Mutation it catches: make `rebind` a no-op (or have it point at
    /// anything other than the arena entry the pre-pass produced) and the
    /// `ptr::eq` assertion fails — which is precisely the failure the
    /// hybrid re-scan would otherwise hit silently, since the object's
    /// record and the arena's are EQUAL for a numeric binding and no
    /// value comparison can tell them apart.
    #[test]
    fn rebind_points_every_site_at_its_arena_record() {
        let (objects, order) = numeric_fixture();
        let arena = resolve_bindings(&order).expect("a numeric binding resolves");
        // The identity half: nothing symbolic, so nothing changed.
        for (f, resolved) in order.iter().zip(&arena) {
            for (&(_, _, original), r) in f.bound.iter().zip(resolved) {
                assert_eq!(original, r, "the pre-pass altered a numeric record");
            }
        }
        // The identity half again, sharper: equal but NOT the same object.
        for (f, resolved) in order.iter().zip(&arena) {
            for (&(_, _, original), r) in f.bound.iter().zip(resolved) {
                assert!(
                    !std::ptr::eq(original, r),
                    "the arena must own its records, not alias the object's"
                );
            }
        }
        let order = rebind(order, &arena);
        for (f, resolved) in order.iter().zip(&arena) {
            for (&(_, _, record), r) in f.bound.iter().zip(resolved) {
                assert!(
                    std::ptr::eq(record, r),
                    "`{}` still reads the object's record, not the arena's",
                    f.name
                );
            }
        }
        drop(objects);
    }
}
```

`numeric_fixture()` builds an `ObjectFile` carrying one numeric bound call and runs `resolve::resolve` over it to get the order. Write it in the same module; the shape to copy is `crates/core/tests/link_tables.rs`'s single-object link fixtures plus `crates/core/src/asm::assemble` with the crate's own `asm::syntax::fixture::test_syntax()`. Keep the objects alive (hence the binding and the `drop` at the end) — `order` borrows from them.

- [ ] **Step 2: Run it to verify it fails**

Run: `CARGO_TARGET_DIR=/Users/mellonis/Developer/mellonis-workspace/machines/toolchains/target cargo test -p mtc-core linker::interface`
Expected: FAIL — `resolve_bindings`/`rebind` do not exist yet (compile error). After Step 4 it must pass; if the `ptr::eq` assertion passes *before* `rebind` is called, the arena is aliasing the object and Step 4 is wrong.

- [ ] **Step 3: Add `interface` to `FuncRef`**

In `crates/core/src/linker/resolve.rs`, add to the `use` list at the top: `RoutineInterface` alongside the other `crate::formats::object` imports. Then, in the `FuncRef` struct (after the `signature` field):

```rust
    /// The function's interface record, when its object carries an
    /// interface section: parameter names, per-tape glyphs, `writes`,
    /// `enters`/`leaves`, the `opaque` bits, the exit count and the
    /// `returns` bit (docs/formats.md (routine interfaces)). Indexed by
    /// blob exactly like `signature`. `None` for a v2/v3 object, for a
    /// PM-1 object, and for any object whose assembly declared no
    /// `.param` lines — all three read the same way, which is the point
    /// of the typed absence.
    pub interface: Option<&'a RoutineInterface>,
```

and in the `FuncRef { … }` literal inside the `order` map (immediately after the `signature:` field):

```rust
                interface: object
                    .interface
                    .as_ref()
                    .and_then(|i| i.routines.get(site.1 as usize)),
```

- [ ] **Step 4: Create the pre-pass module**

Create `crates/core/src/linker/interface.rs`:

```rust
//! Interface-aware link-stage resolution (docs/core.md (symbolic
//! resolution)). The object format carries a bound call in a SYMBOLIC
//! form — a parameter name instead of a list position, a glyph label
//! instead of a callee symbol index — because a compiler that has never
//! seen the callee cannot spell either as a number
//! (docs/formats.md (bound calls)). Turning them into numbers is the
//! link stage's own step, and it happens here, once, before the
//! composition engine reads a single binding.
//!
//! The resolved records live in an ARENA the caller owns: an
//! `ObjectFile` is never mutated, because a labelled pair carries `dst`
//! 0 by the writer's invariant and writing a resolved index into it
//! would break `from_bytes(to_bytes(x)) == x`
//! (docs/formats.md (bound calls)).
//!
//! Placed here rather than in `resolve` for the reason the old refusal
//! guard documented: `resolve` is shared with `resolve_names`, the
//! standalone name-resolution query the editor overlays run against,
//! which has no business failing over a binding.

use super::LinkError;
use super::resolve::FuncRef;
use crate::formats::object::BoundCall;

/// Resolve every reached bound call's binding against its callee's
/// interface. Returns one `Vec<BoundCall>` per function, parallel to
/// `FuncRef::bound`, holding the numeric records the engine consumes.
///
/// The resolved record preserves everything the symbolic one carried
/// EXCEPT the two symbolic fields: `caller_tape`, `map_written`, `open`,
/// `one_way`, `src` and `exits` all survive verbatim, and only `param`
/// and `dst_label` are cleared (with `dst` filled in). `open` in
/// particular is SEMANTIC, not symbolic — the composition algebra reads
/// it (docs/formats.md (bound calls)) — so a pre-pass that normalized it
/// away would silently close every open binding.
pub(super) fn resolve_bindings(order: &[FuncRef]) -> Result<Vec<Vec<BoundCall>>, LinkError> {
    order
        .iter()
        .map(|f| {
            f.bound
                .iter()
                .map(|&(_, callee, record)| resolve_one(&order[callee], record))
                .collect::<Result<Vec<_>, _>>()
        })
        .collect()
}

/// One site. Task 2 is the identity; Tasks 3-8 add one rule each.
fn resolve_one(_callee: &FuncRef, record: &BoundCall) -> Result<BoundCall, LinkError> {
    Ok(record.clone())
}

/// Re-point every `FuncRef::bound` entry at its arena record, so the
/// composition engine — and hybrid's second `scan_sites` pass over the
/// mono-rewritten order — can only ever see the resolved form. Takes
/// `order` BY VALUE: `FuncRef<'a>` is covariant in `'a`, so a
/// `Vec<FuncRef<'obj>>` coerces to `Vec<FuncRef<'arena>>` at the call;
/// `&mut Vec<FuncRef<'a>>` would be invariant and would not compile.
pub(super) fn rebind<'a>(
    mut order: Vec<FuncRef<'a>>,
    arena: &'a [Vec<BoundCall>],
) -> Vec<FuncRef<'a>> {
    for (f, resolved) in order.iter_mut().zip(arena) {
        debug_assert_eq!(
            f.bound.len(),
            resolved.len(),
            "the arena is parallel to FuncRef::bound"
        );
        for (slot, rec) in f.bound.iter_mut().zip(resolved) {
            slot.2 = rec;
        }
    }
    order
}
```

Register it in `crates/core/src/linker/mod.rs`, in the module list at `:4-9` (alphabetical, after `engine`):

```rust
mod interface;
```

- [ ] **Step 5: Wire the pre-pass into `link`**

In `crates/core/src/linker/mod.rs`, replace the `engine::lower` dispatch block (`:434-442`) with:

```rust
    // Symbolic bound-call records resolve here, once, into an arena the
    // engine reads instead of the objects (docs/core.md (symbolic
    // resolution)). Nothing below this point can see a parameter name or
    // a glyph label.
    let arena = interface::resolve_bindings(&resolved.order)?;
    let resolved_order = interface::rebind(resolved.order, &arena);
    let lowered = match entry_sig {
        Some(sig) => engine::lower(syntax, resolved_order, sig, options.call_mech)?,
        None => engine::Lowered {
            order: resolved_order,
            plan: None,
            stats: engine::EngineStats::default(),
            orphaned: Vec::new(),
            diagnostics: Vec::new(),
            folds: Vec::new(),
        },
    };
    let engine::Lowered {
        order,
        plan: frames_plan,
        stats,
        orphaned,
        diagnostics: _diagnostics,
        folds: _folds,
    } = lowered;
```

(`_diagnostics` and `_folds` become real in Tasks 10 and 8; the leading underscore keeps clippy quiet until then.)

- [ ] **Step 6: Replace `LoweredOrder` with `Lowered`**

In `crates/core/src/linker/engine.rs`, replace the `LoweredOrder` alias (`:114-124`) with:

```rust
/// The composition engine's lowering result, shared by every entry point
/// (`lower`, `lower_mono`, `lower_hybrid`): the (possibly rewritten)
/// order, an optional `FramesPlan`, engine counters, the sorted names any
/// stamping pass pruned as newly-orphaned generics, the link warnings the
/// site checks raised, and the hybrid fold decisions
/// (docs/core.md (the link report)).
pub(super) struct Lowered<'a> {
    pub order: Vec<FuncRef<'a>>,
    pub plan: Option<FramesPlan>,
    pub stats: EngineStats,
    pub orphaned: Vec<String>,
    pub diagnostics: Vec<super::LinkDiagnostic>,
    pub folds: Vec<super::FoldDecision>,
}
```

and add the two types `Lowered` refers to in `crates/core/src/linker/mod.rs`, right after the `CallMech` definition:

```rust
/// A link-time WARNING: a finding that does not stop the link
/// (docs/core.md (link warnings)). Task 10 gives it its fields and
/// its code registry; it exists from this task so every lowering entry
/// point has one shape.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkDiagnostic {
    /// The stable kebab-case code; joins the shared allow namespace
    /// (docs/tmt/lint.md (the allow namespace)).
    pub code: &'static str,
    /// The rendered finding, one sentence, no trailing period.
    pub message: String,
    /// The function the site sits in.
    pub function: String,
    /// The site's blob offset inside that function.
    pub offset: u32,
    /// The source line, when the object carried `-g` debug lines.
    pub line: Option<u32>,
}

/// One hybrid exit-bearing fold decision, for the link report
/// (docs/core.md (call mechanisms)). Task 8 fills it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FoldDecision {
    /// The callee routine the group reaches.
    pub routine: String,
    /// The number of exit-bearing bijection sites in the group.
    pub sites: u32,
    /// The callee body's size in bytes (code blob plus table blob).
    pub body_bytes: u32,
    /// The bytes the would-be frames descriptors cost, summed.
    pub descriptor_bytes: u32,
    /// True when the group was shared under frames; false when each site
    /// became a mono seed.
    pub shared: bool,
}
```

Then change every producer:

- `engine::lower` (`:152`) returns `Result<Lowered<'a>, LinkError>`; its early return at `:172` becomes
  ```rust
        return Ok(Lowered {
            order,
            plan: None,
            stats: EngineStats::default(),
            orphaned: Vec::new(),
            diagnostics: Vec::new(),
            folds: Vec::new(),
        });
  ```
  and the `CallMech::Frames` arm (`:180-183`) becomes
  ```rust
        CallMech::Frames => {
            let (order, plan, stats) = lower_frames(syntax, order, &sites, machine_sig)?;
            Ok(Lowered {
                order,
                plan,
                stats,
                orphaned: Vec::new(),
                diagnostics: Vec::new(),
                folds: Vec::new(),
            })
        }
  ```
- `stamp::lower_mono` (`:124`) and `stamp::lower_hybrid` (`:253`) return `Result<Lowered<'a>, LinkError>`; their `Ok((out, None, engine_stats, orphaned))` (`:242`) and `Ok((order, plan, stats, orphaned))` (`:362`) become the struct form with `diagnostics: Vec::new(), folds: Vec::new()`. The two fast-path returns in `lower_hybrid` (`:293-295` and `:297`) forward the struct unchanged.
- `use super::engine::{…, Lowered, …}` replaces `LoweredOrder` in `stamp.rs:50-52`.

- [ ] **Step 7: Run the full core suite and the PM-1 gates**

Run: `CARGO_TARGET_DIR=/Users/mellonis/Developer/mellonis-workspace/machines/toolchains/target cargo test -p mtc-core`
Expected: PASS — including all six `link_interface` tests, the four refusals among them (the guard is untouched).

Run: `CARGO_TARGET_DIR=/Users/mellonis/Developer/mellonis-workspace/machines/toolchains/target cargo test -p mtc-post-machine --test golden_programs && CARGO_TARGET_DIR=/Users/mellonis/Developer/mellonis-workspace/machines/toolchains/target cargo test -p mtc-post-machine --test asm_volatile`
Expected: PASS — PM-1 byte identity.

Run: `CARGO_TARGET_DIR=/Users/mellonis/Developer/mellonis-workspace/machines/toolchains/target cargo test -p mtc-turing-machine --test mode_equivalence`
Expected: PASS — every existing image is byte-identical.

- [ ] **Step 8: Commit**

```bash
git add crates/core/src/linker/interface.rs crates/core/src/linker/mod.rs crates/core/src/linker/resolve.rs crates/core/src/linker/engine.rs crates/core/src/linker/stamp.rs
git commit -m "feat(core): the link stage's resolution arena, FuncRef::interface and a named lowering result"
```

Then `git log -1 --format=%B`; amend if a `Claude-Session:` line was appended.

---

### Task 3: Named parameters resolve

**Files:**
- Modify: `crates/core/src/linker/interface.rs` (`resolve_one`)
- Modify: `crates/core/src/linker/engine.rs:641-671` (`refuse_symbolic_binding` — drop the `param` arm only)
- Modify: `crates/core/tests/link_interface.rs:170-173` (`a_named_entry_is_refused_under_every_mechanism` flips)
- Create: `crates/core/tests/link_resolution.rs`

**Interfaces:**
- Consumes: `FuncRef::interface`, `interface::resolve_one` from Task 2.
- Produces: `resolve_one` now reorders a fully-named binding into callee tape order and clears `param`. Every later task extends the same function.

- [ ] **Step 1: Flip the refusal test into a resolution test**

In `crates/core/tests/link_interface.rs`, the callee `sub` in `fn program` carries no `.param` lines, so it describes no interface. Give it one — replace `fn program` (`:133-146`) with:

```rust
/// A two-function program whose `main` bound-calls `sub` once, with
/// `binding` as the call's operand text. The two tapes have equal
/// cardinalities, so a swap `[1, 0]` is a legal non-identity binding: it
/// does not collapse to a plain call, and it needs no hole. Both
/// routines declare their interface, so a symbolic form has something to
/// resolve against; `sub`'s parameters are `p` and `q`, in that order.
fn program(binding: &str) -> String {
    format!(
        "\
.routine main, tapes=2, alpha=(4, 4)
.param a, ('_', 'x', 'y', 'z')
.param b, ('_', 'x', 'y', 'z')
.routine sub, tapes=2, alpha=(4, 4)
.param p, ('_', '0', '1', '2')
.param q, ('_', '0', '1', '2')
.section code
.func main
        call    sub {binding}
L:      stp
.func sub
        ret
"
    )
}
```

**[shape-copied]** from `crates/core/tests/asm_interface.rs:89-95` (the `.routine`/`.param` pair) and `crates/core/tests/link_interface.rs:133-146` (the two-function skeleton).

Then replace `a_named_entry_is_refused_under_every_mechanism` (`:170-173`) with:

```rust
/// `p: 1, q: 0` binds by callee PARAMETER. The linker looks each name up
/// in the callee's interface, reorders the entries into the callee's own
/// tape order, and hands the engine the very binding the positional
/// spelling would have produced — so the image is byte-identical to the
/// numeric form.
///
/// Mutation it catches: make the resolver take a named entry positionally
/// (ignore `param`) and `[p: 1, q: 0]` links as `[1, 0]` while
/// `[q: 0, p: 1]` links as `[0, 1]` — the second assertion below fails.
#[test]
fn a_named_entry_resolves_to_the_numeric_form_under_every_mechanism() {
    let numeric = program("[1, 0]");
    for named in ["[p: 1, q: 0]", "[q: 0, p: 1]"] {
        let src = program(named);
        for mech in MECHS {
            let a = link(&fake_syntax(), &[asm(&src)], &[], opts(mech))
                .unwrap_or_else(|e| panic!("`{named}` must link under {mech}: {e}"));
            let b = link(&fake_syntax(), &[asm(&numeric)], &[], opts(mech))
                .unwrap_or_else(|e| panic!("`[1, 0]` must link under {mech}: {e}"));
            assert_eq!(
                a.executable.to_bytes(),
                b.executable.to_bytes(),
                "`{named}` must link exactly like `[1, 0]` under {mech}"
            );
        }
    }
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `CARGO_TARGET_DIR=/Users/mellonis/Developer/mellonis-workspace/machines/toolchains/target cargo test -p mtc-core --test link_interface a_named_entry_resolves`
Expected: FAIL — `` `[p: 1, q: 0]` must link under mono: bad binding to `sub`: the call site uses a named entry, a symbolic form the object carries but the link stage does not resolve yet ``.

- [ ] **Step 3: Resolve names in the pre-pass**

In `crates/core/src/linker/interface.rs`, replace `resolve_one` and add the helpers:

```rust
/// One site's binding, resolved against the callee's interface.
fn resolve_one(callee: &FuncRef, record: &BoundCall) -> Result<BoundCall, LinkError> {
    let named = record.binding.iter().filter(|tb| tb.param.is_some()).count();
    if named == 0 {
        return Ok(record.clone());
    }
    if named != record.binding.len() {
        return Err(bad(
            callee,
            "the binding mixes named and positional entries; write every entry \
             one way or the other"
                .to_string(),
        ));
    }
    let iface = require_interface(callee, "a named entry")?;
    let binding = reorder_named(callee, iface, &record.binding)?;
    Ok(BoundCall {
        binding,
        ..record.clone()
    })
}

/// The callee's interface, or the refusal that replaces
/// `external-binding-unsupported` for a callee that describes none
/// (docs/core.md (symbolic resolution)).
fn require_interface<'a>(
    callee: &FuncRef<'a>,
    form: &str,
) -> Result<&'a RoutineInterface, LinkError> {
    callee.interface.ok_or_else(|| {
        bad(
            callee,
            format!(
                "the call site uses {form}, but `{}` describes no interface; \
                 only a transparent call can reach it",
                callee.name
            ),
        )
    })
}

/// Reorder a fully-named binding into the callee's own tape order,
/// clearing `param` as it goes. Every parameter must be bound exactly
/// once: a binding names every entry or none
/// (docs/tmt/language.md (symbol maps)).
fn reorder_named(
    callee: &FuncRef,
    iface: &RoutineInterface,
    binding: &[TapeBinding],
) -> Result<Vec<TapeBinding>, LinkError> {
    let mut slots: Vec<Option<TapeBinding>> = vec![None; iface.params.len()];
    for tb in binding {
        let name = tb.param.as_deref().expect("checked fully named");
        let Some(k) = iface.params.iter().position(|p| p == name) else {
            return Err(bad(
                callee,
                format!(
                    "the binding names parameter `{name}`, which `{}` does not declare",
                    callee.name
                ),
            ));
        };
        if slots[k].is_some() {
            return Err(bad(
                callee,
                format!("the binding names parameter `{name}` twice"),
            ));
        }
        slots[k] = Some(TapeBinding {
            param: None,
            ..tb.clone()
        });
    }
    slots
        .into_iter()
        .enumerate()
        .map(|(k, slot)| {
            slot.ok_or_else(|| {
                bad(
                    callee,
                    format!(
                        "the binding does not bind parameter `{}`; an argument list \
                         is complete",
                        iface.params[k]
                    ),
                )
            })
        })
        .collect()
}

fn bad(callee: &FuncRef, message: String) -> LinkError {
    LinkError::BadBinding {
        callee: callee.name.to_string(),
        message,
    }
}
```

Extend the module's `use` list to `use crate::formats::object::{BoundCall, RoutineInterface, TapeBinding};`.

- [ ] **Step 4: Drop the `param` arm from the refusal guard**

In `crates/core/src/linker/engine.rs`, inside `refuse_symbolic_binding` (`:641-671`), delete the first `if`/`else if` branch so the chain begins at `open`:

```rust
                .find_map(|tb| {
                    if tb.open {
                        Some("an open map")
                    } else if tb.pairs.iter().any(|p| p.dst_label.is_some()) {
                        Some("a glyph-labelled destination")
                    } else {
                        None
                    }
                })
```

and strike "a named entry," from the function's doc comment's opening list.

- [ ] **Step 5: Run the test to verify it passes**

Run: `CARGO_TARGET_DIR=/Users/mellonis/Developer/mellonis-workspace/machines/toolchains/target cargo test -p mtc-core --test link_interface`
Expected: PASS — five tests now, with the three remaining refusals still red-lining their own forms.

- [ ] **Step 6: Add the failure-mode unit coverage**

Create `crates/core/tests/link_resolution.rs`. It needs its own fake dialect; copy `fake_syntax`, `asm`, `MECHS` and `opts` verbatim from `crates/core/tests/link_interface.rs:22-127` (the per-file-helper convention — there is no shared test-support module).

```rust
//! Every way a symbolic binding can fail to resolve against the callee's
//! interface (docs/core.md (symbolic resolution)), on a neutral fake
//! dialect.

// … fake_syntax / asm / MECHS / opts copied from link_interface.rs …

/// A caller/callee pair whose callee declares two parameters `p`, `q`
/// over a 4-symbol alphabet each.
fn program(binding: &str) -> String {
    format!(
        "\
.routine main, tapes=2, alpha=(4, 4)
.param a, ('_', 'x', 'y', 'z')
.param b, ('_', 'x', 'y', 'z')
.routine sub, tapes=2, alpha=(4, 4)
.param p, ('_', '0', '1', '2')
.param q, ('_', '0', '1', '2')
.section code
.func main
        call    sub {binding}
L:      stp
.func sub
        ret
"
    )
}

/// The same pair with NO `.param` lines on the callee: it describes no
/// interface, so nothing symbolic can reach it.
fn interfaceless(binding: &str) -> String {
    format!(
        "\
.routine main, tapes=2, alpha=(4, 4)
.routine sub, tapes=2, alpha=(4, 4)
.section code
.func main
        call    sub {binding}
L:      stp
.func sub
        ret
"
    )
}

#[track_caller]
fn refused(src: &str, needle: &str) {
    let err = link(&fake_syntax(), &[asm(src)], &[], opts(CallMech::Frames))
        .expect_err("the binding must be refused");
    let LinkError::BadBinding { callee, message } = &err else {
        panic!("expected a BadBinding, got {err:?}");
    };
    assert_eq!(callee, "sub");
    assert!(message.contains(needle), "{message}");
}

/// Mutation it catches: drop the `position` lookup's `None` arm and an
/// unknown name silently binds nothing.
#[test]
fn an_unknown_parameter_is_refused() {
    refused(&program("[p: 1, zz: 0]"), "parameter `zz`, which `sub` does not declare");
}

/// Mutation it catches: drop the `slots[k].is_some()` guard and the
/// second `p` silently overwrites the first, leaving `q` unbound.
#[test]
fn a_parameter_bound_twice_is_refused() {
    refused(&program("[p: 1, p: 0]"), "names parameter `p` twice");
}

/// A one-entry named list against a two-parameter callee. Mutation it
/// catches: let `reorder_named` fill a missing slot with a default and
/// the call binds tape 1 to caller tape 0 by accident.
#[test]
fn a_missing_parameter_is_refused() {
    refused(&program("[p: 1]"), "does not bind parameter `q`");
}

/// Mutation it catches: make `require_interface` fall back to positional
/// resolution and a symbolic call into an interfaceless callee links
/// silently — the exact hazard the refusal existed for.
#[test]
fn a_named_entry_into_an_interfaceless_callee_is_refused() {
    refused(
        &interfaceless("[p: 1, q: 0]"),
        "describes no interface; only a transparent call can reach it",
    );
}

/// Mutation it catches: delete the mixed-form guard and the positional
/// entry is taken at its LIST position while the named one is taken at
/// its parameter position — two different orderings in one list.
#[test]
fn a_mixed_named_and_positional_list_is_refused() {
    // The assembler rejects a mixed list at parse time, so this fixture
    // is built by hand rather than assembled: it is the hand-crafted
    // object a third-party producer could emit.
    use mtc_core::formats::object::TapeBinding;
    let mut obj = asm(&program("[1, 0]"));
    obj.bound_calls[0].binding[0] = TapeBinding {
        param: Some("p".to_string()),
        ..obj.bound_calls[0].binding[0].clone()
    };
    let err = link(&fake_syntax(), &[obj], &[], opts(CallMech::Frames))
        .expect_err("a mixed list must be refused");
    assert!(
        matches!(&err, LinkError::BadBinding { message, .. }
            if message.contains("mixes named and positional entries")),
        "{err:?}"
    );
}
```

- [ ] **Step 7: Run the new file**

Run: `CARGO_TARGET_DIR=/Users/mellonis/Developer/mellonis-workspace/machines/toolchains/target cargo test -p mtc-core --test link_resolution`
Expected: PASS, five tests.

If `a_mixed_named_and_positional_list_is_refused` fails at the `asm` step because the assembler accepts the mixed spelling after all, drop the hand-construction and write the mixed list directly in the fixture text instead — but record which way it went in the commit body.

- [ ] **Step 8: Run the gates and commit**

Run: `CARGO_TARGET_DIR=/Users/mellonis/Developer/mellonis-workspace/machines/toolchains/target cargo test -p mtc-core && CARGO_TARGET_DIR=/Users/mellonis/Developer/mellonis-workspace/machines/toolchains/target cargo test -p mtc-post-machine --test golden_programs`

```bash
git add crates/core/src/linker/interface.rs crates/core/src/linker/engine.rs crates/core/tests/link_interface.rs crates/core/tests/link_resolution.rs
git commit -m "feat(core): a named binding entry resolves against the callee's parameter list"
```

Then `git log -1 --format=%B`; amend if a `Claude-Session:` line was appended.

---

### Task 4: Glyph labels resolve

**Files:**
- Modify: `crates/core/src/linker/interface.rs` (`resolve_one`)
- Modify: `crates/core/src/linker/engine.rs` (`refuse_symbolic_binding` — drop the `dst_label` arm)
- Modify: `crates/core/tests/link_interface.rs:178-181`
- Modify: `crates/core/tests/link_resolution.rs`

**Interfaces:**
- Consumes: `resolve_one`, `require_interface`, `bad` from Task 3.
- Produces: a resolved `MapPair` whose `dst_label` is `None` and whose `dst` is the label's position in `iface.glyphs[k]`. Label resolution runs AFTER the named reorder, so `k` is always the callee's own tape index.

- [ ] **Step 1: Flip the refusal test**

In `crates/core/tests/link_interface.rs`, replace `a_glyph_labelled_destination_is_refused_under_every_mechanism` (`:178-181`) with:

```rust
/// `3=>'1'` names the callee symbol by GLYPH. The linker looks the label
/// up in the callee's declared glyph list for that tape and fills in the
/// index, so the image is byte-identical to the spelling that wrote the
/// index directly. `sub`'s tapes are `('_', '0', '1', '2')`, so `'1'` is
/// index 2 and `'2'` is index 3.
///
/// Mutation it catches: leave `dst_label` unresolved and the pair reads
/// the `dst: 0` a labelled pair is written with — `'1'` would bind blank.
/// The two spellings below then diverge.
#[test]
fn a_glyph_labelled_destination_resolves_under_every_mechanism() {
    let labelled = program("[1{3=>'1'}, 0]");
    let indexed = program("[1{3=>2}, 0]");
    for mech in MECHS {
        let a = link(&fake_syntax(), &[asm(&labelled)], &[], opts(mech))
            .unwrap_or_else(|e| panic!("the labelled form must link under {mech}: {e}"));
        let b = link(&fake_syntax(), &[asm(&indexed)], &[], opts(mech))
            .unwrap_or_else(|e| panic!("the indexed form must link under {mech}: {e}"));
        assert_eq!(
            a.executable.to_bytes(),
            b.executable.to_bytes(),
            "`3=>'1'` must link exactly like `3=>2` under {mech}"
        );
    }
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `CARGO_TARGET_DIR=/Users/mellonis/Developer/mellonis-workspace/machines/toolchains/target cargo test -p mtc-core --test link_interface a_glyph_labelled`
Expected: FAIL with `the call site uses a glyph-labelled destination, a symbolic form … does not resolve yet`.

- [ ] **Step 3: Resolve labels in the pre-pass**

In `crates/core/src/linker/interface.rs`, rewrite `resolve_one` so the two rules compose:

```rust
fn resolve_one(callee: &FuncRef, record: &BoundCall) -> Result<BoundCall, LinkError> {
    let named = record.binding.iter().filter(|tb| tb.param.is_some()).count();
    if named != 0 && named != record.binding.len() {
        return Err(bad(
            callee,
            "the binding mixes named and positional entries; write every entry \
             one way or the other"
                .to_string(),
        ));
    }
    let labelled = record
        .binding
        .iter()
        .any(|tb| tb.pairs.iter().any(|p| p.dst_label.is_some()));
    if named == 0 && !labelled {
        return Ok(record.clone());
    }

    // Names first: after the reorder, entry `k` IS callee tape `k`, which
    // is what makes the per-tape glyph list the right one to look a label
    // up in.
    let mut binding = if named == 0 {
        record.binding.clone()
    } else {
        let iface = require_interface(callee, "a named entry")?;
        reorder_named(callee, iface, &record.binding)?
    };
    if labelled {
        let iface = require_interface(callee, "a glyph-labelled destination")?;
        resolve_labels(callee, iface, &mut binding)?;
    }
    Ok(BoundCall {
        binding,
        ..record.clone()
    })
}

/// Turn every `dst_label` into the glyph's position in the callee's
/// declared alphabet for that tape (docs/formats.md (bound calls)). The
/// label is cleared as it is consumed, so nothing downstream can read a
/// stale one.
fn resolve_labels(
    callee: &FuncRef,
    iface: &RoutineInterface,
    binding: &mut [TapeBinding],
) -> Result<(), LinkError> {
    for (k, tb) in binding.iter_mut().enumerate() {
        let Some(glyphs) = iface.glyphs.get(k) else {
            return Err(bad(
                callee,
                format!(
                    "binding tape {k} is outside `{}`'s declared interface \
                     ({} parameter(s))",
                    callee.name,
                    iface.glyphs.len()
                ),
            ));
        };
        for pair in tb.pairs.iter_mut() {
            let Some(label) = pair.dst_label.take() else {
                continue;
            };
            let Some(idx) = glyphs.iter().position(|g| *g == label) else {
                return Err(bad(
                    callee,
                    format!(
                        "binding tape {k} names glyph `{label}`, which is not in \
                         `{}`'s alphabet for parameter `{}`",
                        callee.name,
                        iface
                            .params
                            .get(k)
                            .map_or_else(|| k.to_string(), String::clone)
                    ),
                ));
            };
            pair.dst = u32::try_from(idx).expect("a glyph index fits u32");
        }
    }
    Ok(())
}
```

- [ ] **Step 4: Drop the `dst_label` arm from the refusal guard**

In `crates/core/src/linker/engine.rs`, `refuse_symbolic_binding`'s chain becomes:

```rust
                .find_map(|tb| tb.open.then_some("an open map"))
```

and the doc comment's list drops "a glyph-labelled destination".

- [ ] **Step 5: Run to verify it passes**

Run: `CARGO_TARGET_DIR=/Users/mellonis/Developer/mellonis-workspace/machines/toolchains/target cargo test -p mtc-core --test link_interface`
Expected: PASS.

- [ ] **Step 6: Add the failure-mode coverage**

Append to `crates/core/tests/link_resolution.rs`:

```rust
/// Mutation it catches: make the unknown-glyph arm fall back to `dst: 0`
/// and a typo'd glyph silently binds blank.
#[test]
fn an_unknown_glyph_is_refused() {
    refused(
        &program("[1{3=>'9'}, 0]"),
        "names glyph `9`, which is not in `sub`'s alphabet for parameter `p`",
    );
}

/// Mutation it catches: make `require_interface` optional for labels and
/// an interfaceless callee links with every labelled pair reading 0.
#[test]
fn a_glyph_label_into_an_interfaceless_callee_is_refused() {
    refused(
        &interfaceless("[1{3=>'1'}, 0]"),
        "describes no interface; only a transparent call can reach it",
    );
}

/// Names and labels in ONE binding: the reorder must run first, or the
/// label is looked up in the wrong tape's glyph list. Mutation it
/// catches: resolve labels before reordering and `q: 1{3=>'1'}` resolves
/// `'1'` against parameter `p`'s glyphs instead of `q`'s.
#[test]
fn a_named_entry_carrying_a_glyph_label_resolves_in_callee_tape_order() {
    let mixed = program("[q: 1{3=>'1'}, p: 0]");
    let indexed = program("[0, 1{3=>2}]");
    for mech in MECHS {
        let a = link(&fake_syntax(), &[asm(&mixed)], &[], opts(mech))
            .unwrap_or_else(|e| panic!("under {mech}: {e}"));
        let b = link(&fake_syntax(), &[asm(&indexed)], &[], opts(mech))
            .unwrap_or_else(|e| panic!("under {mech}: {e}"));
        assert_eq!(a.executable.to_bytes(), b.executable.to_bytes(), "under {mech}");
    }
}
```

Note for the implementer: `[q: 1{3=>'1'}, p: 0]` binds callee tape `q` (index 1) to caller tape 1 and callee tape `p` (index 0) to caller tape 0 — which is the positional list `[0, 1{3=>2}]` with the map on tape 1. Check the assembled `bound_calls[0].binding` in a scratch assertion if the equality surprises you; the fixture is the discriminator, not the arithmetic.

- [ ] **Step 7: Run the gates and commit**

Run: `CARGO_TARGET_DIR=/Users/mellonis/Developer/mellonis-workspace/machines/toolchains/target cargo test -p mtc-core && CARGO_TARGET_DIR=/Users/mellonis/Developer/mellonis-workspace/machines/toolchains/target cargo test -p mtc-post-machine --test golden_programs`

```bash
git add crates/core/src/linker/interface.rs crates/core/src/linker/engine.rs crates/core/tests/link_interface.rs crates/core/tests/link_resolution.rs
git commit -m "feat(core): a glyph-labelled binding destination resolves against the callee's alphabet"
```

Then `git log -1 --format=%B`; amend if a `Claude-Session:` line was appended.

---

### Task 5: Open bindings

`with map { 'a'->'a', * }` means "every unlisted caller symbol is opaque": it maps ONE-WAY onto the index equal to the callee's cardinality — an index no callee row names, so only a `*` row matches it and only `-` keeps it. The machine already runs this under mono and frames with a hand-authored descriptor; the probe `docs/superpowers/probes/2026-09-13-tmc-completeness/probe2-generic/hand_open_abcd.tma` is the evidence, and its `rmap=(1->1, 2->2, 3->3, 4->3)` over a 3-symbol callee is exactly what the closed rule must stop overwriting with holes.

**Files:**
- Modify: `crates/core/src/linker/compose.rs:284-395` (`binding_to_composite`), `:423-438` (`close_unlisted`)
- Modify: `crates/core/src/linker/interface.rs` (the `opaque` check)
- Modify: `crates/core/src/linker/mod.rs` (`LinkError::OpenBindingUnsupported`)
- Modify: `crates/core/src/linker/engine.rs` (shrink `refuse_symbolic_binding` to the ONE form still unresolved — the exit vector; its call stays, and Task 9 deletes the function)
- Modify: `crates/core/tests/link_interface.rs:185-188`
- Create: `crates/core/tests/link_open.rs`

**Interfaces:**
- Consumes: `resolve_one` (Tasks 3–4).
- Produces:
  - `LinkError::OpenBindingUnsupported { callee: String, tape: usize, param: Option<String> }`.
  - `compose::binding_to_composite` treats an `open` tape's unlisted non-blank caller symbols as one-way images of the OPAQUE index `card` (the callee cardinality), never as holes. `MAX_SYMBOL` still bounds it, so `card <= MAX_SYMBOL` is required.
  - `stamp.rs` and `engine.rs` need **no change**: the opaque index is a normal `rmap` image, so `dense_map`'s `Some(v) if v < codomain_card` guard is what must widen — see Step 4.

- [ ] **Step 1: Flip the refusal test**

Replace `an_open_map_is_refused_under_every_mechanism` (`crates/core/tests/link_interface.rs:185-188`) with:

```rust
/// `{*}` says the listed pairs are not the whole map: every unlisted
/// caller symbol reads as the OPAQUE index — the callee's cardinality,
/// an index no callee row names. `sub`'s tapes are 4 wide, so the opaque
/// index is 4 and the binding is legal only because `sub` declares that
/// tape `opaque`.
///
/// `{*}` no longer refuses. What an open map MEANS is pinned in
/// `link_open.rs` (Step 7), against its closed counterpart on the
/// UNEQUAL alphabets where the two genuinely differ — on the equal
/// cardinalities of this fixture the closed rule completes by identity
/// and holes nothing, so an open/closed comparison here would prove
/// nothing.
///
/// Mutation it catches: restore the `open` arm of the refusal guard and
/// this link fails under every mechanism with a `BadBinding`.
#[test]
fn an_open_map_no_longer_refuses() {
    let src = open_program("[1{*}, 0]");
    for mech in MECHS {
        link(&fake_syntax(), &[asm(&src)], &[], opts(mech))
            .unwrap_or_else(|e| panic!("an open map must link under {mech}: {e}"));
    }
}
```

and add, next to `fn program`:

```rust
/// `program`, but `sub`'s first parameter is declared `opaque` — the
/// precondition an open binding requires.
fn open_program(binding: &str) -> String {
    format!(
        "\
.routine main, tapes=2, alpha=(4, 4)
.param a, ('_', 'x', 'y', 'z')
.param b, ('_', 'x', 'y', 'z')
.routine sub, tapes=2, alpha=(4, 4)
.param p, ('_', '0', '1', '2'), opaque
.param q, ('_', '0', '1', '2')
.section code
.func main
        call    sub {binding}
L:      stp
.func sub
        ret
"
    )
}
```

**[shape-copied]** — `opaque` as the last `.param` suffix is `crates/core/tests/asm_interface.rs:94` and `crates/turing-machine/tests/tma_dialect.rs:106`.

- [ ] **Step 2: Run it to verify it fails**

Run: `CARGO_TARGET_DIR=/Users/mellonis/Developer/mellonis-workspace/machines/toolchains/target cargo test -p mtc-core --test link_interface an_open_map`
Expected: FAIL with `the call site uses an open map, a symbolic form …`.

- [ ] **Step 3: Add the error variant and the `opaque` check**

In `crates/core/src/linker/mod.rs`, after `BadBinding`:

```rust
    /// An open binding (`{…, *}`) names a callee tape the callee does not
    /// declare `opaque`. An open map sends every unlisted caller symbol
    /// onto one index no callee row names, which is sound only where
    /// every state that reads the tape has a `*` row — the fact the
    /// `opaque` bit records (docs/formats.md (routine interfaces)). A
    /// callee that describes no interface cannot accept one at all.
    /// `param` is the parameter's declared name when the interface names
    /// it, `None` when there is no interface to ask.
    OpenBindingUnsupported {
        callee: String,
        tape: usize,
        param: Option<String>,
    },
```

and its `Display` arm:

```rust
            Self::OpenBindingUnsupported {
                callee,
                tape,
                param,
            } => {
                let which = match param {
                    Some(p) => format!("parameter `{p}`"),
                    None => format!("tape {tape}"),
                };
                write!(
                    f,
                    "an open binding into `{callee}`'s {which}, which is not declared \
                     opaque; every state that reads it must have a `*` row"
                )
            }
```

In `crates/core/src/linker/interface.rs`, extend `resolve_one`: after the `named`/`labelled` early-out and before the reorder, add

```rust
    let open = record.binding.iter().any(|tb| tb.open);
    if named == 0 && !labelled && !open {
        return Ok(record.clone());
    }
```

(replacing the old two-condition early-out), and after the label pass:

```rust
    if open {
        check_opaque(callee, &binding)?;
    }
```

with

```rust
/// Refuse an open binding into a tape the callee does not declare
/// opaque, and into a callee that describes no interface at all
/// (docs/core.md (symbolic resolution)). Runs after the reorder, so
/// entry `k` is callee tape `k`.
fn check_opaque(callee: &FuncRef, binding: &[TapeBinding]) -> Result<(), LinkError> {
    for (k, tb) in binding.iter().enumerate() {
        if !tb.open {
            continue;
        }
        let opaque = callee
            .interface
            .and_then(|i| i.opaque.get(k).copied())
            .unwrap_or(false);
        if !opaque {
            return Err(LinkError::OpenBindingUnsupported {
                callee: callee.name.to_string(),
                tape: k,
                param: callee
                    .interface
                    .and_then(|i| i.params.get(k))
                    .cloned(),
            });
        }
    }
    Ok(())
}
```

- [ ] **Step 4: Open the closed rule in the composition algebra**

In `crates/core/src/linker/compose.rs`, replace the closed-on-unequal block inside `binding_to_composite` (`:368-380`) with:

```rust
        // Closed-on-unequal (docs/formats.md (bound calls)): identity
        // completion is only for equal-size alphabets. Across differently
        // sized alphabets the map is closed — a non-blank source symbol
        // the binding does not name is a hole. Computed from the explicit
        // srcs still present in `pairs` (identity pairs are stored during
        // ingestion and only dropped by `canonicalize` below), so an
        // explicit `k->k` keeps `k` mapped while a truly absent `k`
        // traps. Read holes are caller symbols with no pair; write holes
        // are callee symbols with no bidirectional pair writing back.
        //
        // An OPEN binding replaces the read half of that rule: the
        // unlisted caller symbols are not absent, they are OPAQUE — they
        // read as the index `card`, one past the callee's alphabet, which
        // no callee row can name, so only a `*` cell matches them and
        // only a keep preserves them (docs/formats.md (bound calls)). The
        // WRITE half stays closed either way: an opaque symbol is
        // read-only by construction, exactly like a one-way `=>` pair.
        if tb.open {
            if card > MAX_SYMBOL {
                return Err(ComposeError::SymbolRange {
                    tape: k,
                    symbol: card,
                    cardinality: MAX_SYMBOL + 1,
                });
            }
            open_unlisted(&mut rmap, caller_card, card as u16);
            close_unlisted(&mut wmap, card);
        } else if caller_card != card {
            close_unlisted(&mut rmap, caller_card);
            close_unlisted(&mut wmap, card);
        }
```

and add, next to `close_unlisted`:

```rust
/// Open a read map over its `domain_card` source symbols: every non-blank
/// source index below `domain_card` that no explicit pair names maps
/// ONE-WAY onto `opaque` — the index equal to the callee's cardinality,
/// which no callee row names (docs/formats.md (bound calls) — the open
/// rule). Blank stays pinned. Called before [`SparseMap::canonicalize`],
/// like `close_unlisted`, so an explicit identity pair survives as
/// identity and only a genuinely unnamed symbol becomes opaque.
fn open_unlisted(map: &mut SparseMap, domain_card: u32, opaque: u16) {
    let listed: BTreeSet<u16> = map.pairs.keys().copied().collect();
    let upper = domain_card.min(MAX_SYMBOL + 1);
    for s in 1..upper {
        let s16 = s as u16;
        if !listed.contains(&s16) {
            map.pairs.insert(s16, opaque);
        }
    }
}
```

In `crates/core/src/linker/engine.rs`, `dense_map` (`:796-816`) currently writes `0xFFFF` for any image at or past `codomain_card`. The opaque index IS `codomain_card`, so widen the guard by one:

```rust
    (0..domain_card)
        .map(|s| {
            let Ok(s16) = u16::try_from(s) else {
                return 0xFFFF;
            };
            match apply(s16) {
                // `codomain_card` itself is the OPAQUE index an open
                // binding sends unlisted symbols to: a real image the
                // callee can only match with `*`, not a hole
                // (docs/formats.md (bound calls)). Anything beyond it is
                // still a hole.
                Some(v) if u32::from(v) <= codomain_card => v,
                _ => 0xFFFF,
            }
        })
        .collect()
```

In `crates/core/src/linker/stamp.rs`, `read_image` (`:722-727`) is the mono-side twin and needs the same widening:

```rust
fn read_image(t: &CompositeTape, p: u16, callee_card: u32) -> Option<u16> {
    match t.rmap.apply(p) {
        // `callee_card` is the opaque index (docs/formats.md (bound
        // calls)); it is an image, so the stamp synthesizes no trap row
        // for it and its preimage expands the callee's `*` row.
        Some(v) if u32::from(v) <= callee_card => Some(v),
        _ => None,
    }
}
```

`write_image` is NOT widened: an opaque symbol is read-only.

- [ ] **Step 5: Reduce the refusal guard to its LAST unresolved form**

**Do not remove the guard's call here.** Exits are still unresolved until Task 6, and a link that silently DROPPED an exit vector would produce a green test suite over an unsound binary — the exact failure the guard exists to prevent. Keep the call and shrink the body to the one form that is still unresolved.

In `crates/core/src/linker/engine.rs`, `refuse_symbolic_binding`'s body becomes:

```rust
fn refuse_symbolic_binding(order: &[FuncRef]) -> Result<(), LinkError> {
    for f in order {
        for &(_, callee, record) in &f.bound {
            if !record.exits.is_empty() {
                return Err(LinkError::BadBinding {
                    callee: order[callee].name.to_string(),
                    message: "the call site uses an exit vector, a symbolic form the \
                              object carries but the link stage does not resolve yet; \
                              write the binding without one"
                        .to_string(),
                });
            }
        }
    }
    Ok(())
}
```

and its doc comment shrinks to describe only the exit vector: names, glyph labels and open maps all resolve now. Task 6 removes the call; Task 9 deletes the function and re-pins the reachability test, which keeps each diff reviewable.

- [ ] **Step 6: Run the flipped test**

Run: `CARGO_TARGET_DIR=/Users/mellonis/Developer/mellonis-workspace/machines/toolchains/target cargo test -p mtc-core --test link_interface`
Expected: PASS, all six tests — `an_exit_vector_is_refused_under_every_mechanism` still refuses, because the guard still runs for that one form. No `#[ignore]` is needed anywhere.

- [ ] **Step 7: Write the open-binding coverage**

Create `crates/core/tests/link_open.rs`. Copy `fake_syntax`/`asm`/`MECHS`/`opts` from `link_interface.rs`, then:

```rust
//! Open bindings (`{…, *}`): every unlisted caller symbol reads as the
//! OPAQUE index — the callee's cardinality — instead of becoming a hole
//! (docs/formats.md (bound calls)). Proven on a neutral fake dialect.

// … fake_syntax / asm / MECHS / opts copied from link_interface.rs …

/// A 5-symbol caller calling a 3-symbol callee that declares its tape
/// opaque and carries a `*` row. `1->1, 2->2` are named; caller symbols
/// 3 and 4 are opaque and read as index 3.
const OPEN: &str = "\
.routine main, tapes=1, alpha=(5)
.param t, ('_', 'a', 'b', 'c', 'd')
.routine sub, tapes=1, alpha=(3)
.param n, ('_', 'a', 'b'), writes=('a', 'b'), opaque
.section code
.func main
        call    sub [0{1->1, 2->2, *}]
        stp
.func sub
        ret
";

/// The same program with the tape NOT declared opaque.
const OPEN_UNSUPPORTED: &str = "\
.routine main, tapes=1, alpha=(5)
.param t, ('_', 'a', 'b', 'c', 'd')
.routine sub, tapes=1, alpha=(3)
.param n, ('_', 'a', 'b'), writes=('a', 'b')
.section code
.func main
        call    sub [0{1->1, 2->2, *}]
        stp
.func sub
        ret
";

/// The same program whose callee describes no interface at all.
const OPEN_INTERFACELESS: &str = "\
.routine main, tapes=1, alpha=(5)
.routine sub, tapes=1, alpha=(3)
.section code
.func main
        call    sub [0{1->1, 2->2, *}]
        stp
.func sub
        ret
";

/// The CLOSED counterpart: the same pairs without `*`, where the
/// unlisted symbols become holes.
const CLOSED: &str = "\
.routine main, tapes=1, alpha=(5)
.param t, ('_', 'a', 'b', 'c', 'd')
.routine sub, tapes=1, alpha=(3)
.param n, ('_', 'a', 'b'), writes=('a', 'b'), opaque
.section code
.func main
        call    sub [0{1->1, 2->2}]
        stp
.func sub
        ret
";

/// Mutation it catches: keep the closed rule for an open tape and this
/// links to the CLOSED program's bytes — which the first assertion
/// forbids — and, under mono, the stamp synthesizes an unmapped-read
/// trap row for each of the two unlisted symbols, which the second
/// forbids. The two halves fail in different mechanisms, so both are
/// asserted.
#[test]
fn an_open_binding_links_under_every_mechanism_and_differs_from_the_closed_one() {
    for mech in MECHS {
        let open = link(&fake_syntax(), &[asm(OPEN)], &[], opts(mech))
            .unwrap_or_else(|e| panic!("the open form must link under {mech}: {e}"));
        let closed = link(&fake_syntax(), &[asm(CLOSED)], &[], opts(mech))
            .unwrap_or_else(|e| panic!("the closed form must link under {mech}: {e}"));
        assert_ne!(
            open.executable.to_bytes(),
            closed.executable.to_bytes(),
            "an open map must not link like a closed one under {mech}"
        );
        assert_eq!(
            open.report.synthesized_trap_rows, 0,
            "an opaque symbol is an IMAGE, not a hole, so it owes no trap row \
             under {mech}: {:?}",
            open.report
        );
    }
    // And the closed form really does hole — otherwise the contrast above
    // would hold for a reason unrelated to the open rule.
    let closed = link(&fake_syntax(), &[asm(CLOSED)], &[], opts(CallMech::Mono))
        .expect("the closed form links under mono");
    assert!(
        closed.report.synthesized_trap_rows > 0,
        "the closed counterpart must hole: {:?}",
        closed.report
    );
}

/// Mutation it catches: delete `check_opaque` and a routine that
/// discriminates every glyph accepts opaque input it can only misread.
#[test]
fn an_open_binding_into_a_non_opaque_tape_is_refused() {
    for mech in MECHS {
        let err = link(&fake_syntax(), &[asm(OPEN_UNSUPPORTED)], &[], opts(mech))
            .expect_err("a non-opaque tape must refuse an open binding");
        let LinkError::OpenBindingUnsupported {
            callee,
            tape,
            param,
        } = &err
        else {
            panic!("expected OpenBindingUnsupported under {mech}, got {err:?}");
        };
        assert_eq!((callee.as_str(), *tape, param.as_deref()), ("sub", 0, Some("n")));
    }
}

/// Mutation it catches: default `opaque` to `true` when there is no
/// interface and an interfaceless callee silently accepts one.
#[test]
fn an_open_binding_into_an_interfaceless_callee_is_refused() {
    let err = link(
        &fake_syntax(),
        &[asm(OPEN_INTERFACELESS)],
        &[],
        opts(CallMech::Frames),
    )
    .expect_err("an interfaceless callee must refuse an open binding");
    assert!(
        matches!(&err, LinkError::OpenBindingUnsupported { param: None, .. }),
        "{err:?}"
    );
}

/// The descriptor the frames path emits must carry the opaque index, not
/// the hole sentinel. Mutation it catches: leave `dense_map`'s guard at
/// `< codomain_card` and the two opaque symbols come back `0xFFFF`.
#[test]
fn the_frames_descriptor_carries_the_opaque_index_not_a_hole() {
    let out = link(&fake_syntax(), &[asm(OPEN)], &[], opts(CallMech::Frames))
        .expect("links");
    let bytes = out.executable.to_bytes();
    // The dense rmap for a 5-symbol physical band reads
    // [0, 1, 2, 3, 3] — blank pinned, two named, two opaque. Search the
    // image for that little-endian u16 run; `0xFFFF` anywhere in it is
    // the regression.
    let want: Vec<u8> = [0u16, 1, 2, 3, 3]
        .iter()
        .flat_map(|v| v.to_le_bytes())
        .collect();
    assert!(
        bytes.windows(want.len()).any(|w| w == want),
        "the opaque rmap run is not in the image"
    );
}
```

- [ ] **Step 8: Run everything and commit**

Run: `CARGO_TARGET_DIR=/Users/mellonis/Developer/mellonis-workspace/machines/toolchains/target cargo test -p mtc-core`
Expected: PASS, all six `link_interface` tests included — `an_exit_vector_is_refused_under_every_mechanism` still refuses, because the guard still runs for that one form. Nothing is ignored anywhere in this task.

Run: `CARGO_TARGET_DIR=/Users/mellonis/Developer/mellonis-workspace/machines/toolchains/target cargo test -p mtc-post-machine --test golden_programs && CARGO_TARGET_DIR=/Users/mellonis/Developer/mellonis-workspace/machines/toolchains/target cargo test -p mtc-turing-machine --test mode_equivalence`
Expected: PASS — no existing program has an open binding, so no image moves.

```bash
git add crates/core/src/linker/compose.rs crates/core/src/linker/engine.rs crates/core/src/linker/interface.rs crates/core/src/linker/mod.rs crates/core/src/linker/stamp.rs crates/core/tests/link_interface.rs crates/core/tests/link_open.rs
git commit -m "feat(core): an open binding maps unlisted caller symbols onto the opaque index"
```

Then `git log -1 --format=%B`; amend if a `Claude-Session:` line was appended.

---

### Task 6: Exit vectors under frames (P1, P3)

**P1 — exits go through compose.** A site's descriptor under frames is `compose(directory[FR], binding)` plus the site's exit vector. **P3 — an exit-bearing site never collapses to a plain call.**

Three facts shape the implementation, and none of them is optional:

1. `BoundCall.exits` are offsets into the caller's **original** blob; `rewrite_blob` shifts every offset past a widened bound site by `+4` per preceding widened site (`crates/core/src/linker/engine.rs:858-861`).
2. Composites are interned during the closure BFS, **before** the rewrite; descriptors are materialized **after** it (`:393-396`). So exits must travel with the interned entry, and the intern key must distinguish two otherwise-identical composites whose sites have different exits.
3. **Exits must NOT go on `Composite`.** `canonical_key`/`digest` feed the algebra's law proptests and the mono stamp name; widening them would change both. The exits ride alongside, in the engine's own intern key.

**Files:**
- Modify: `crates/core/src/linker/engine.rs:95-112` (`FramesPlan`), `:209-282` (the closure BFS), `:392-415` (materialization + return), `:497-514` (`intern_composite`), `:536-614` (`scan_sites` — the collapse conjunct), `:749-790` (`materialize`)
- Modify: `crates/core/src/linker/stamp.rs:604` (the in-stamp collapse conjunct)
- Modify: `crates/core/src/linker/layout.rs:585-700` (retain per-function offset maps), `:791-802` + `:876-919` (`emit_planned_region`)
- Modify: `crates/core/tests/link_interface.rs` (un-ignore + flip the exit test)
- Create: `crates/core/tests/link_exits.rs`

**Interfaces:**
- Consumes: everything from Tasks 2–5.
- Produces:
  - `FramesPlan.engine_exits: Vec<Option<(usize, Vec<u32>)>>` — parallel to `engine_descriptors`; `Some((func, exits))` names the function whose POST-REWRITE blob the offsets are relative to. `None` for an exit-free descriptor.
  - `engine::widen_shift(sites: &[SiteKind], old: u32) -> u32` — the rewrite's offset map, derived from a function's site list. Public to the module only.
  - `layout::build` retains `abs_of: Vec<HashMap<u32, u32>>` (per function, post-rewrite blob offset → offset within the emitted function) and `bases: Vec<u32>` for `emit_planned_region`, which now returns `Result<u32, LinkError>`.
  - **The collapse conjunct, spelled IDENTICALLY at both sites:** `record.exits.is_empty() && is_full_passthrough(&composite, caller_sig, callee_sig)`. `is_full_passthrough`'s own signature is unchanged — it is pure algebra and has no business knowing about exits.

- [ ] **Step 1: Write the failing test**

In `crates/core/tests/link_interface.rs`, replace `an_exit_vector_is_refused_under_every_mechanism` (`:192-195`) with the frames half only — mono and hybrid land in Tasks 7 and 8. **Also delete the call to `refuse_symbolic_binding` at `crates/core/src/linker/engine.rs:160` and its two preceding comment lines, and put `#[allow(dead_code)]` immediately above `fn refuse_symbolic_binding`** — the last form it guarded is the one this task resolves. Task 9 deletes the function itself.

```rust
/// `exits=(L)` names where the callee's exits land. Under FRAMES the
/// site's descriptor carries the vector, so the linked image differs
/// from the exit-free spelling and the exit target's address appears in
/// the frames region.
///
/// Mutation it catches: drop `record.exits` on the way into
/// `materialize` and the image becomes byte-identical to the exit-free
/// one — which the assertion forbids.
#[test]
fn an_exit_vector_reaches_the_frames_descriptor() {
    let with_exits = exit_program("[1, 0] exits=(L)");
    let without = exit_program("[1, 0]");
    let a = link(&fake_syntax(), &[asm(&with_exits)], &[], opts(CallMech::Frames))
        .expect("an exit-bearing site must link under frames");
    let b = link(&fake_syntax(), &[asm(&without)], &[], opts(CallMech::Frames))
        .expect("the exit-free site links");
    assert_ne!(
        a.executable.to_bytes(),
        b.executable.to_bytes(),
        "an exit vector must reach the image"
    );
}
```

and add next to `fn program`:

```rust
/// `program`, but the callee declares one state parameter (`exits=1`),
/// which is what makes a one-entry exit vector legal at the call site.
fn exit_program(binding: &str) -> String {
    format!(
        "\
.routine main, tapes=2, alpha=(4, 4)
.param a, ('_', 'x', 'y', 'z')
.param b, ('_', 'x', 'y', 'z')
.routine sub, tapes=2, alpha=(4, 4), exits=1
.param p, ('_', '0', '1', '2')
.param q, ('_', '0', '1', '2')
.section code
.func main
        call    sub {binding}
L:      stp
.func sub
        ret
"
    )
}
```

**[shape-copied]** — the `exits=1` routine tail is `crates/core/tests/asm_interface.rs:237`; the `exits=(L)` operand is `crates/core/tests/link_interface.rs:194`.

- [ ] **Step 2: Run it to verify it fails**

Run: `CARGO_TARGET_DIR=/Users/mellonis/Developer/mellonis-workspace/machines/toolchains/target cargo test -p mtc-core --test link_interface an_exit_vector`
Expected: FAIL on the `assert_ne!` — the exits vanish silently today, so the two images are identical.

- [ ] **Step 3: Check the exit count against the callee's interface**

In `crates/core/src/linker/interface.rs`, extend `resolve_one`'s early-out condition to include `!record.exits.is_empty()`, and add after `check_opaque`:

```rust
    if !record.exits.is_empty() {
        let iface = require_interface(callee, "an exit vector")?;
        if record.exits.len() != usize::from(iface.exits) {
            return Err(bad(
                callee,
                format!(
                    "the call site supplies {} exit(s), but `{}` declares {}",
                    record.exits.len(),
                    callee.name,
                    iface.exits
                ),
            ));
        }
    }
```

- [ ] **Step 4: Conjoin `exits.is_empty()` at both collapse sites**

In `crates/core/src/linker/engine.rs`, inside `scan_sites` (`:576`):

```rust
                    // Identity collapse (handoff): a site lowers to a plain call
                    // ONLY when the binding, absolutized at the caller's own
                    // identity, is a genuine full pass-through of the caller's
                    // tapes into the callee — identity placement, identity
                    // maps, AND equal per-tape alphabets. A narrower or wider
                    // callee carries a cardinality hole that must trap, and a
                    // projecting identity (fewer tapes than the caller) fails
                    // the arity check; both stay framed calls. An EXIT-BEARING
                    // site never collapses either, whatever its binding: a
                    // plain call returns through the pushed return address and
                    // has nowhere to put the other exits
                    // (docs/core.md (call mechanisms)).
                    let collapse = record.exits.is_empty()
                        && is_full_passthrough(&composite, caller_sig, callee_sig);
```

In `crates/core/src/linker/stamp.rs`, inside `mono_stamps`' bound arm (`:604`):

```rust
                    let idx = if record.exits.is_empty()
                        && is_full_passthrough(&child, machine_sig, callee_sig)
                    {
```

- [ ] **Step 5: Thread exits through the frames closure**

In `crates/core/src/linker/engine.rs`:

(a) Add the shift helper, next to `routine_sig`:

```rust
/// The blob rewrite's offset map for one function, derived from its site
/// list: `new(old) = old + 4 * (framed bound sites strictly before old)`
/// — the same total `rewrite_blob` applies to every offset it carries
/// forward (docs/core.md (the composition engine)). Exposed so the
/// engine can shift a bound call's exit vector, which lives on the
/// object record rather than inside the blob and so is not carried by
/// the rewrite itself.
pub(super) fn widen_shift(sites: &[SiteKind], old: u32) -> u32 {
    let widened = sites
        .iter()
        .filter(|s| {
            matches!(
                s,
                SiteKind::Bound {
                    addr,
                    collapse: false,
                    ..
                } if *addr < old
            )
        })
        .count();
    old + 4 * u32::try_from(widened).expect("widened-site count fits u32")
}
```

(b) Change `engine_comps` to carry the exits and widen the intern key. Replace the declarations at `:209-215` with:

```rust
    let mut engine_comps: Vec<(Composite, Option<(usize, Vec<u32>)>)> = Vec::new();
    let mut comp_index: HashMap<Vec<u8>, u16> = HashMap::new();
```

and `intern_composite` (`:501-514`) with:

```rust
/// Intern a composite into the engine directory, deduped by canonical
/// key. Returns its 1-based directory index (engine composites occupy
/// 1..=E) and whether it resolved to an ALREADY-interned composite (a
/// descriptor emit the dedup avoided).
///
/// An EXIT-BEARING site widens the key with the owning function and its
/// exit offsets: two sites may compose to the same placement and still
/// need different descriptors, because the exit vector is part of the
/// descriptor (docs/formats.md (frame descriptors)). An exit-FREE site
/// appends nothing, so its key is byte-for-byte what it always was and
/// every existing image keeps its directory.
fn intern_composite(
    comps: &mut Vec<(Composite, Option<(usize, Vec<u32>)>)>,
    index: &mut HashMap<Vec<u8>, u16>,
    c: Composite,
    exits: Option<(usize, Vec<u32>)>,
) -> (u16, bool) {
    let mut key = canonical_key(&c);
    if let Some((func, offsets)) = &exits {
        key.extend_from_slice(&(*func as u64).to_le_bytes());
        for off in offsets {
            key.extend_from_slice(&off.to_le_bytes());
        }
    }
    if let Some(&i) = index.get(&key) {
        return (i, true);
    }
    comps.push((c, exits));
    let i = u16::try_from(comps.len()).expect("composite index fits u16");
    index.insert(key, i);
    (i, false)
}
```

(c) In the BFS's non-collapsing bound arm (`:255-274`), pass the shifted exits:

```rust
                SiteKind::Bound {
                    addr,
                    callee,
                    record,
                    collapse: false,
                } => {
                    let callee_sig = routine_sig(&order, *callee)?;
                    let child = compose(&ctx, caller_cards, *callee, &record.binding, callee_sig)
                        .map_err(|e| bad_binding(&order[*callee].name, &e))?;
                    // The record's exits are ORIGINAL blob offsets; the
                    // rewrite widens every framed site by 4 bytes, so
                    // they are shifted here into the post-rewrite blob
                    // layout layout will map to addresses
                    // (docs/core.md (the composition engine)).
                    let exits = (!record.exits.is_empty()).then(|| {
                        (
                            fi,
                            record
                                .exits
                                .iter()
                                .map(|&e| widen_shift(&sites[fi], e))
                                .collect::<Vec<u32>>(),
                        )
                    });
                    let (idx, deduped) =
                        intern_composite(&mut engine_comps, &mut comp_index, child.clone(), exits);
                    if deduped {
                        dedup_savings += 1;
                    }
                    site_columns
                        .entry((fi, *addr))
                        .or_default()
                        .insert(fr_row, idx);
                    queue.push_back((*callee, child));
                }
```

(d) Fix the two other reads of `engine_comps`: the `routines` map (`:290-293`) becomes `.map(|(c, _)| order[c.routine].name.to_string())`, and the materialization loop (`:393-396`) becomes

```rust
    let mut engine_descriptors = Vec::with_capacity(engine_count);
    let mut engine_exits = Vec::with_capacity(engine_count);
    for (c, exits) in &engine_comps {
        let offsets: &[u32] = exits.as_ref().map_or(&[], |(_, o)| o.as_slice());
        engine_descriptors.push(materialize(c, machine_sig, &new_order, offsets)?);
        engine_exits.push(exits.clone());
    }
```

(e) `materialize` (`:749-790`) gains the parameter and stops hard-coding an empty exit vector:

```rust
/// Materialize a composite into frame-descriptor bytes (docs/formats.md
/// (frame descriptors)): per virtual tape a physical index and dense
/// read/write maps sized to the relevant alphabet — identity where it
/// fits, `0xFFFF` holes where a symbol has no image in the target
/// alphabet (unequal-size, hole-based) — followed by the site's exit
/// vector.
///
/// `exits` are POST-REWRITE blob offsets in the CALLING function, written
/// here as placeholders: layout rebases each to an absolute code address
/// once the function is placed, exactly as it rebases a raw `.frame`
/// descriptor's exits (docs/formats.md (frames region)). An exit-free
/// descriptor is address-independent, as every engine descriptor was
/// before declarative exits existed.
fn materialize(
    c: &Composite,
    machine_sig: &RoutineSig,
    order: &[FuncRef],
    exits: &[u32],
) -> Result<Vec<u8>, LinkError> {
```

with the final line `Ok(descriptor_bytes(&entries, exits))`.

(f) `FramesPlan` (`:95-112`) gains:

```rust
    /// Per engine descriptor, the function whose POST-REWRITE blob its
    /// exit offsets are relative to, and those offsets. `None` for an
    /// exit-free (address-independent) descriptor. Layout rebases each
    /// to an absolute code address, like a raw `.frame` descriptor's
    /// exits (docs/formats.md (frames region)).
    pub engine_exits: Vec<Option<(usize, Vec<u32>)>>,
```

and the `FramesPlan { … }` literal at `:404-409` carries it. The doc comment on `engine_descriptors` loses "No `retx` exits, so byte-content is address-free" — replace with "Exit-free descriptors are address-free; an exit-bearing one carries placeholder offsets `engine_exits` names."

- [ ] **Step 6: Rebase the exits in layout**

In `crates/core/src/linker/layout.rs`:

(a) Before the per-function loop (`:585`), declare

```rust
    // Each function's post-rewrite blob offset -> its offset within the
    // emitted function, retained past the loop so the frames region can
    // rebase an engine descriptor's exit vector (docs/formats.md (frames
    // region)). `bases` is already in scope for the absolute part.
    let mut abs_of: Vec<HashMap<u32, u32>> = Vec::with_capacity(order.len());
```

and inside the loop, immediately after `orig_to_new` is built, `abs_of.push(orig_to_new.clone());`.

(b) The plan arm (`:791-802`) becomes

```rust
        Some(plan) => {
            let frames_offset = emit_planned_region(
                plan,
                &mut code,
                &mut tables,
                &fcall_holes,
                &func_table_bases,
                &bases,
                &abs_of,
                order,
            )?;
```

(c) `emit_planned_region` (`:876`) gains the three parameters, returns `Result<u32, LinkError>`, and rebases each exit-bearing descriptor's trailing exit words before appending it:

```rust
#[allow(clippy::too_many_arguments)]
fn emit_planned_region(
    plan: &super::engine::FramesPlan,
    code: &mut [u8],
    tables: &mut Vec<u8>,
    fcall_holes: &[usize],
    func_table_bases: &[u32],
    bases: &[u32],
    abs_of: &[HashMap<u32, u32>],
    order: &[FuncRef],
) -> Result<u32, LinkError> {
    use super::engine::DirSource;

    // Append the synthesized descriptors, rebasing any exit vector: the
    // engine wrote POST-REWRITE blob offsets in the calling function as
    // placeholders, and the absolute address is that function's base plus
    // its own offset map's image (docs/formats.md (frames region)). An
    // exit off an instruction boundary is malformed blob data no rebase
    // can make sense of.
    let mut engine_offsets: Vec<u32> = Vec::with_capacity(plan.engine_descriptors.len());
    for (desc, exits) in plan.engine_descriptors.iter().zip(&plan.engine_exits) {
        engine_offsets.push(u32::try_from(tables.len()).expect("table offset fits u32"));
        let start = tables.len();
        tables.extend_from_slice(desc);
        let Some((func, offsets)) = exits else {
            continue;
        };
        let tail = start + desc.len() - 4 * offsets.len();
        for (i, &off) in offsets.iter().enumerate() {
            let Some(&within) = abs_of[*func].get(&off) else {
                return Err(LinkError::MalformedBlob {
                    symbol: order[*func].name.to_string(),
                    at: off,
                });
            };
            let abs = bases[*func] + within;
            let at = tail + 4 * i;
            tables[at..at + 4].copy_from_slice(&abs.to_le_bytes());
        }
    }
```

The rest of the body is unchanged except the final `frames_offset` becomes `Ok(frames_offset)`.

- [ ] **Step 7: Run the flipped test**

Run: `CARGO_TARGET_DIR=/Users/mellonis/Developer/mellonis-workspace/machines/toolchains/target cargo test -p mtc-core --test link_interface`
Expected: PASS.

- [ ] **Step 8: Write the frames exit coverage**

Create `crates/core/tests/link_exits.rs` with the `link_interface.rs` dialect helpers copied in, plus a `jmp` entry the later tasks need:

```rust
//! Exit vectors on a declarative bound call (docs/core.md (call
//! mechanisms)), on a neutral fake dialect. Frames carries them in the
//! descriptor; mono splices a per-site copy; hybrid folds.

// … fake_syntax (with the extra entries below) / asm / MECHS / opts …
```

In this file's `fake_syntax`, add to `entries` — the exact literals, so mono's rewrite in Task 7 has an unconditional jump and a multi-exit return to work with:

```rust
            SyntaxEntry {
                opcode: 0x20,
                mnemonic: "jmp",
                operand: OperandKind::RelI32,
                flow: Flow::Jump,
            },
            SyntaxEntry {
                opcode: 0x1A,
                mnemonic: "retx",
                operand: OperandKind::Imm8,
                flow: Flow::Stop,
            },
```

and set `return_opcode: Some(0x0B)` (Task 7 adds the field; until then omit the line and add it in Task 7's step that updates all 36 literals).

Then:

```rust
/// A caller whose `main` bound-calls a two-exit `sub` and a one-exit
/// `solo`. `sub` returns through `retx #0` / `retx #1`; `solo` through
/// `retx #0`. Every routine declares its interface, so the exit counts
/// are checkable.
const TWO_EXITS: &str = "\
.routine main, tapes=1, alpha=(3)
.param t, ('_', '0', '1')
.routine sub, tapes=1, alpha=(3), exits=2
.param n, ('_', '0', '1')
.section code
.func main
        call    sub [0] exits=(won, lost)
        stp
won:    wr      [1]
        stp
lost:   wr      [2]
        stp
.func sub
        retx    #0
";

/// The same program whose site supplies ONE exit against a callee that
/// declares two.
const WRONG_COUNT: &str = "\
.routine main, tapes=1, alpha=(3)
.param t, ('_', '0', '1')
.routine sub, tapes=1, alpha=(3), exits=2
.param n, ('_', '0', '1')
.section code
.func main
        call    sub [0] exits=(won)
        stp
won:    wr      [1]
        stp
.func sub
        retx    #0
";

/// An exit-bearing site whose binding is the full identity — the exact
/// shape that WOULD collapse to a plain call without P3.
const IDENTITY_WITH_EXITS: &str = "\
.routine main, tapes=1, alpha=(3)
.param t, ('_', '0', '1')
.routine sub, tapes=1, alpha=(3), exits=1
.param n, ('_', '0', '1')
.section code
.func main
        call    sub [0] exits=(won)
        stp
won:    wr      [1]
        stp
.func sub
        retx    #0
";

/// Mutation it catches: drop the `exits.is_empty()` conjunct at
/// `scan_sites` and this site collapses to a plain call — the image then
/// carries no frames region at all and `composites` is 0.
#[test]
fn an_identity_binding_with_exits_does_not_collapse() {
    let out = link(
        &fake_syntax(),
        &[asm(IDENTITY_WITH_EXITS)],
        &[],
        opts(CallMech::Frames),
    )
    .expect("links under frames");
    assert!(
        out.report.composites > 0,
        "an exit-bearing identity binding must stay framed: {:?}",
        out.report
    );
}

/// Mutation it catches: skip the exit-count check and a site that
/// supplies too few exits links, leaving `retx #1` to read past the
/// vector at run time.
#[test]
fn a_wrong_exit_count_is_refused() {
    let err = link(&fake_syntax(), &[asm(WRONG_COUNT)], &[], opts(CallMech::Frames))
        .expect_err("a short exit vector must be refused");
    assert!(
        matches!(&err, LinkError::BadBinding { message, .. }
            if message.contains("supplies 1 exit(s), but `sub` declares 2")),
        "{err:?}"
    );
}

/// The descriptor's exit words must be the EXACT absolute addresses of
/// `won` and `lost`, in that order — not the blob-relative placeholders
/// the engine wrote.
///
/// The expected values are DERIVED, not transcribed: assemble with `-g`
/// so the object carries each label's original blob offset, then
/// `absolute = main.start + raw + 4`, the `+ 4` being the one widened
/// bound site (`main`'s only bound call, at offset 1) that precedes both
/// labels. The sidecar's own label addresses must agree with that
/// arithmetic, which pins the shift independently of the descriptor.
///
/// Mutation it catches: skip the rebase in `emit_planned_region` and the
/// image carries the shifted BLOB offsets instead — small numbers, which
/// the final assertion explicitly forbids. A range check would not catch
/// it, since a small offset can also fall inside `main`'s range.
#[test]
fn the_frames_descriptor_exits_are_the_exact_absolute_addresses() {
    let obj = assemble(&fake_syntax(), ARCH, TWO_EXITS, true).expect("assembles with -g");
    let raw_of = |name: &str| -> u32 {
        obj.debug
            .as_ref()
            .expect("-g")
            .iter()
            .flat_map(|b| b.labels.iter())
            .find(|(n, _)| n == name)
            .map(|(_, off)| *off)
            .unwrap_or_else(|| panic!("no label `{name}` in the object"))
    };
    let out = link(&fake_syntax(), &[obj.clone()], &[], opts(CallMech::Frames))
        .expect("links under frames");
    let main = out
        .map
        .functions
        .iter()
        .find(|f| f.name == "main")
        .expect("`main` is in the sidecar");
    let want_addr = |name: &str| main.start + raw_of(name) + 4;
    // The sidecar agrees with the arithmetic: the shift is +4, once.
    for name in ["won", "lost"] {
        let sidecar = main
            .labels
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, a)| *a)
            .unwrap_or_else(|| panic!("no `{name}` in the sidecar"));
        assert_eq!(sidecar, want_addr(name), "`{name}`'s address");
    }
    // The descriptor carries exactly those two words, in vector order.
    let bytes = out.executable.to_bytes();
    let want: Vec<u8> = [want_addr("won"), want_addr("lost")]
        .iter()
        .flat_map(|a| a.to_le_bytes())
        .collect();
    assert!(
        bytes.windows(want.len()).any(|w| w == want),
        "the descriptor does not carry [{}, {}] in order",
        want_addr("won"),
        want_addr("lost")
    );
    // And the UN-rebased placeholders are nowhere in it.
    let bad: Vec<u8> = [raw_of("won") + 4, raw_of("lost") + 4]
        .iter()
        .flat_map(|a| a.to_le_bytes())
        .collect();
    assert!(
        !bytes.windows(bad.len()).any(|w| w == bad),
        "the exit vector was never rebased"
    );
}
```

Notes for the implementer, both load-bearing: `assemble` needs importing into this file with `ARCH` (copy the `const ARCH: u8` and the `use` line from `link_interface.rs:17-22`), and `ObjectFile` must be `Clone` for the `obj.clone()` above — it is (`crates/core/src/formats/object/mod.rs:75`). If `MapFunction`'s field for labels is not `labels`, read `crates/core/src/linker/mod.rs`'s `MapFunction` and adjust; the arithmetic does not change.

- [ ] **Step 9: Run the gates and commit**

Run: `CARGO_TARGET_DIR=/Users/mellonis/Developer/mellonis-workspace/machines/toolchains/target cargo test -p mtc-core`
Expected: PASS. `link_exits`'s mono and hybrid cases are not written yet.

Run: `CARGO_TARGET_DIR=/Users/mellonis/Developer/mellonis-workspace/machines/toolchains/target cargo test -p mtc-turing-machine --test mode_equivalence && CARGO_TARGET_DIR=/Users/mellonis/Developer/mellonis-workspace/machines/toolchains/target cargo test -p mtc-post-machine --test golden_programs`
Expected: PASS — no existing program has an exit vector, so no intern key changes and every image is byte-identical.

```bash
git add crates/core/src/linker/engine.rs crates/core/src/linker/interface.rs crates/core/src/linker/layout.rs crates/core/src/linker/stamp.rs crates/core/tests/link_interface.rs crates/core/tests/link_exits.rs
git commit -m "feat(core): a declarative bound call's exit vector reaches its frames descriptor"
```

Then `git log -1 --format=%B`; amend if a `Claude-Session:` line was appended.

---

### Task 7: Exit vectors under mono (P2)

**P2 — mono enters through `jmp`.** TM-1 has no pop opcode, so `call` into a copy whose `retx` became `jmp` would leak the return address. Under mono an exit-bearing site is lowered as `jmp` into a per-site copy in which `ret → jmp <then>` and `retx #k → jmp <exit_k>`. `then` is the return address the site would have pushed: the instruction after the call. No stack interaction — this is a splice of the stamped body at link time.

Two mechanisms this task introduces, both new:

- **A dialect must name its return opcode.** `ret`, `stp` and `hlt` are all `OperandKind::None` + `Flow::Stop`, so `ret` cannot be inferred. `trap_opcode`'s doc comment (`crates/core/src/asm/syntax.rs:80-89`) states the codebase's own precedent for declaring such an opcode per dialect.
- **A stamp needs code-offset fixups into ANOTHER function.** `FuncRef.calls` names a function index and layout resolves it to `bases[callee]`; a jump to the middle of the caller is not expressible today. `FuncRef.site_fixups` carries `(hole in this blob, target function index, post-rewrite blob offset in that function)` and layout patches each after the final converged layout, exactly as it patches table fixups.

**Files:**
- Modify: `crates/core/src/asm/syntax.rs:73-93` (`ArchSyntax`), plus a `jump_opcode()` helper near `framed_call_opcode()`
- Modify: **every `ArchSyntax { … }` literal in the tree at this point.** Established fact 6 counted 36 across twelve files on the pre-phase tree, and this phase has since ADDED two test files that carry one each — `crates/core/tests/link_resolution.rs` (Task 3) and `crates/core/tests/link_open.rs` (Task 5) — plus `crates/core/tests/link_exits.rs` (Task 6). **Do not work from the fact's list.** Run `grep -rln "ArchSyntax {" crates` first, edit every file it names, and stage exactly that set.
- Modify: `crates/core/src/linker/resolve.rs` (`FuncRef.site_fixups`)
- Modify: `crates/core/src/linker/stamp.rs:124-243` (`lower_mono` seeds), `:505-661` (`mono_stamps` key + targets), `:741-1020` (`build_stamp`)
- Modify: `crates/core/src/linker/layout.rs` (patch `site_fixups`)
- Modify: `crates/core/tests/link_exits.rs`

**Interfaces:**
- Consumes: `FuncRef.interface`, resolved bindings, `widen_shift`, `engine_exits` (Tasks 2–6).
- Produces:
  - `ArchSyntax.return_opcode: Option<u8>` — the dialect's plain return instruction. `None` means the dialect has none, which is an error only if a reachable mono exit-bearing site needs one.
  - `ArchSyntax::jump_opcode(&self) -> Option<u8>` — the single `Flow::Jump` entry whose operand is `OperandKind::RelI32`.
  - `FuncRef.site_fixups: Vec<(u32, usize, u32)>` — `(hole offset in this blob, target function index, post-rewrite blob offset inside that function)`. Layout patches each to the RelI32 displacement reaching that address. Empty for every function the engine did not synthesize.
  - The mono stamp key becomes `(routine, composite, caller, exits, then)` — appended to `canonical_key(&composite)` exactly as Task 6 appends to the frames intern key, so an exit-free stamp's key and therefore its `<routine>.<digest8>` NAME are unchanged. **The digest itself stays `digest(&composite)`** — widening it would rename every existing stamp and move every existing mono image. The **caller index** is in the key alongside the spec's `(routine, composite, exits, then)` because `then` and the exit offsets are CALLER-blob-relative and mean nothing without naming which caller: two sites in different functions can share a `then` value and be entirely different splices. A precision of the spec's tuple, not a departure from it.
  - ```rust
    /// The `SpliceSite` for one exit-bearing site, with its caller
    /// offsets shifted into the post-rewrite blob layout. One spelling,
    /// used by the seed loop, the closure loop, and Task 8b's
    /// shared-aware closure.
    fn site_for(
        caller: usize,
        addr: u32,
        record: &BoundCall,
        widened: &[HashSet<u32>],
    ) -> Option<SpliceSite>;
    ```

- [ ] **Step 1: Write the failing test**

Append to `crates/core/tests/link_exits.rs`:

```rust
/// Under MONO an exit-bearing site is a splice: the caller `jmp`s into a
/// per-site copy whose `retx #k` becomes a jump to exit `k` and whose
/// `ret` becomes a jump to the instruction after the call. The image
/// runs on the base profile — no frames region at all.
///
/// Mutation it catches: leave `build_stamp`'s `MonoRawFrame` refusal on
/// `Imm8 + Flow::Stop` in place and this link fails; drop the `ret`
/// rewrite and the copy returns through a return address nobody pushed.
#[test]
fn an_exit_bearing_site_splices_under_mono() {
    let out = link(&fake_syntax(), &[asm(TWO_EXITS)], &[], opts(CallMech::Mono))
        .expect("an exit-bearing site must splice under mono");
    assert_eq!(
        out.report.composites, 0,
        "a mono image carries no frames region: {:?}",
        out.report
    );
    assert!(
        out.report.instantiations >= 1,
        "the site must produce a copy: {:?}",
        out.report
    );
}

/// Two sites into the same routine with DIFFERENT exits must not share a
/// copy. Mutation it catches: leave the exits out of the stamp key and
/// the second site reuses the first site's copy, jumping to the first
/// site's exits.
#[test]
fn two_sites_with_different_exits_get_different_copies() {
    const TWO_SITES: &str = "\
.routine main, tapes=1, alpha=(3)
.param t, ('_', '0', '1')
.routine sub, tapes=1, alpha=(3), exits=1
.param n, ('_', '0', '1')
.section code
.func main
        call    sub [0] exits=(a)
        call    sub [0] exits=(b)
        stp
a:      wr      [1]
        stp
b:      wr      [2]
        stp
.func sub
        retx    #0
";
    let out = link(&fake_syntax(), &[asm(TWO_SITES)], &[], opts(CallMech::Mono))
        .expect("links under mono");
    assert_eq!(
        out.report.instantiations, 2,
        "two exit vectors, two copies: {:?}",
        out.report
    );
}

/// A splice whose caller ALSO orphans a generic: `sub` is reached only
/// through the exit-bearing site, so retargeting that site to the copy
/// leaves `sub` with no caller and the prune drops it — reindexing every
/// function after it.
///
/// Mutation it catches: leave `site_fixups` out of `prune_unreachable`'s
/// reindex and the splice's `ret`/`retx` jumps point at whatever function
/// slid into the dropped index — a wrong-target jump the run exposes.
#[test]
fn a_splice_survives_a_prune_that_reindexes_its_caller() {
    const ORPHANS: &str = "\
.routine main, tapes=1, alpha=(3)
.param t, ('_', '0', '1')
.routine sub, tapes=1, alpha=(3), exits=1
.param n, ('_', '0', '1')
.routine tail, tapes=1, alpha=(3)
.param u, ('_', '0', '1')
.section code
.func main
        call    sub [0] exits=(won)
        stp
won:    call    tail
        stp
.func sub
        retx    #0
.func tail
        ret
";
    let obj = assemble(&fake_syntax(), ARCH, ORPHANS, true).expect("assembles with -g");
    let out = link(&fake_syntax(), &[obj], &[], opts(CallMech::Mono))
        .expect("links under mono");
    // `sub` lost its only caller when the site was retargeted to the copy.
    assert!(
        out.report.dropped.contains(&"sub".to_string()),
        "the generic must be pruned: {:?}",
        out.report.dropped
    );
    // The copy's `retx #0` became a jump to `won` in `main`. DECODE it:
    // a stale function index would send it somewhere else entirely, and
    // nothing about `dropped` or the function list would show that.
    let main = out
        .map
        .functions
        .iter()
        .find(|f| f.name == "main")
        .expect("`main` is in the sidecar");
    let want = main
        .labels
        .iter()
        .find(|(n, _)| n == "won")
        .map(|(_, a)| *a)
        .expect("`won` is a labelled position in `main`");
    // The copy is the one function whose name starts with `sub.`.
    let copy = out
        .map
        .functions
        .iter()
        .find(|f| f.name.starts_with("sub."))
        .expect("the splice copy is in the sidecar");
    let bytes = out.executable.to_bytes();
    let jmp = fake_syntax()
        .jump_opcode()
        .expect("the fake dialect has a far jump");
    let mut landed = Vec::new();
    let mut at = copy.start as usize;
    while at + 5 <= copy.end as usize {
        if bytes[at] == jmp {
            let disp = i32::from_le_bytes(bytes[at + 1..at + 5].try_into().unwrap());
            landed.push((at as i64 + 5 + i64::from(disp)) as u32);
            at += 5;
        } else {
            at += 1;
        }
    }
    assert!(
        landed.contains(&want),
        "the splice's jump lands at {landed:?}, not at `won` ({want})"
    );
}
```

The scan is a linear sweep, so it can read an operand byte as an opcode and add a spurious entry — harmless, because the assertion is that the wanted address IS among the targets, not that it is the only one.

```rust

/// A caller holding BOTH a spliced exit-bearing site and a framed holey
/// one: under hybrid the frames path widens the second 5 → 9 bytes,
/// shifting every later offset in that blob — including the splice's
/// `then` and its exits.
///
/// Mutation it catches: leave `splice_shift` out (use the raw record
/// offsets) and the fixup's `abs_of` lookup misses or lands on the wrong
/// instruction, so the link fails or the copy returns to the wrong place.
#[test]
fn a_spliced_site_and_a_framed_site_in_one_caller_agree_under_hybrid() {
    const MIXED: &str = "\
.routine main, tapes=1, alpha=(5)
.param t, ('_', 'a', 'b', 'c', 'd')
.routine pick, tapes=1, alpha=(5), exits=1
.param n, ('_', 'a', 'b', 'c', 'd')
.routine holey, tapes=1, alpha=(3)
.param m, ('_', 'a', 'b')
.section code
.func main
        call    pick [0] exits=(won)
        stp
won:    call    holey [0{1->1, 2->2}]
        stp
.func pick
        retx    #0
.func holey
        ret
";
    for mech in MECHS {
        link(&fake_syntax(), &[asm(MIXED)], &[], opts(mech))
            .unwrap_or_else(|e| panic!("the mixed program must link under {mech}: {e}"));
    }
}
```

**Ordering note for the implementer:** `a_spliced_site_and_a_framed_site_in_one_caller_agree_under_hybrid` exercises the hybrid path, whose exit-bearing classification lands in Task 8. In this task it passes because a single exit-bearing site never shares, so hybrid seeds it to mono either way — which is exactly the shape the shift must survive. Re-run it at the end of Task 8.

- [ ] **Step 2: Run it to verify it fails**

Run: `CARGO_TARGET_DIR=/Users/mellonis/Developer/mellonis-workspace/machines/toolchains/target cargo test -p mtc-core --test link_exits an_exit_bearing_site_splices`
Expected: FAIL — `` `sub` uses a raw framed call, which cannot be lowered onto the base profile `` (the `Imm8 + Flow::Stop` refusal at `crates/core/src/linker/stamp.rs:830-831`).

- [ ] **Step 3: Add `return_opcode` and `jump_opcode`**

In `crates/core/src/asm/syntax.rs`, after `trap_opcode`:

```rust
    /// The dialect's plain return instruction, when it has one — the
    /// `ret` a mono-stamped exit-bearing copy rewrites into a jump to the
    /// call site's continuation (docs/core.md (call mechanisms)). It
    /// cannot be inferred from the syntax table: `ret`, `stp` and `hlt`
    /// share `OperandKind::None` and `Flow::Stop`, so each dialect
    /// declares it explicitly, exactly as it declares `trap_opcode`.
    /// `None` when the dialect has no return, which is an error only if a
    /// reachable mono exit-bearing site needs one rewritten.
    pub return_opcode: Option<u8>,
```

and, next to `framed_call_opcode()`:

```rust
    /// The opcode of this dialect's far unconditional jump, if it has one:
    /// the single `Flow::Jump` entry whose operand is a 32-bit relative
    /// target. The composition engine needs it to splice a mono
    /// exit-bearing copy without naming any architecture's mnemonic (core
    /// is arch-agnostic — docs/core.md (call mechanisms)).
    pub fn jump_opcode(&self) -> Option<u8> {
        let mut found = None;
        for e in &self.entries {
            if e.flow == Flow::Jump && e.operand == OperandKind::RelI32 {
                if found.is_some() {
                    return None; // ambiguous: the dialect must have exactly one
                }
                found = Some(e.opcode);
            }
        }
        found
    }
```

Now add `return_opcode: …` to every literal `grep -rln "ArchSyntax {" crates` names. The two production dialects:

- `crates/turing-machine/src/asm/mod.rs`, in `tm1_syntax()`, next to `trap_opcode: Some(TRAP),`:
  ```rust
        // The plain return a mono exit-bearing copy rewrites into a jump
        // to the call site's continuation (docs/core.md (call mechanisms)).
        return_opcode: Some(RET),
  ```
- `crates/post-machine/src/asm/mod.rs`, in `pm1_syntax()`: `return_opcode: Some(<PM-1's ret opcode>),` — read the entry table in that function for the name. **PM-1 cannot reach the mono exit path at all**, and the reason is structural rather than a property of PM programs: PM's compiler builds objects through `ObjectFile::v2` (`crates/core/src/formats/object/mod.rs:292-314`), which sets `signatures: None`, so `link()` takes the `None` arm of its `match entry_sig` (`crates/core/src/linker/mod.rs:417-442`) and never calls `engine::lower`. No lowering, no stamping, no splice. This is a table entry only, and no PM-1 byte moves.

Every other literal is a fake dialect in core's own source or tests: add `return_opcode: None,` unless the dialect has a `ret` entry that a test needs rewritten — in `crates/core/tests/link_exits.rs`, `link_interface.rs`, `link_resolution.rs` and `link_open.rs` set `return_opcode: Some(0x0B)` (their `ret`).

Run `CARGO_TARGET_DIR=/Users/mellonis/Developer/mellonis-workspace/machines/toolchains/target cargo build --workspace --all-targets 2>&1 | grep "missing field"` to enumerate anything missed; the compiler is the checklist, and `--all-targets` is what makes it see the test files.

- [ ] **Step 4: Add `site_fixups` to `FuncRef` and patch them in layout**

In `crates/core/src/linker/resolve.rs`, after `table_fixups`:

```rust
    /// Code-offset fixups that reach INTO another function: `(hole offset
    /// in this blob, target function index, post-rewrite blob offset
    /// inside that function)`. A mono-stamped exit-bearing copy uses them
    /// for its `ret → jmp <then>` and `retx #k → jmp <exit_k>` rewrites,
    /// where the target is a position inside the CALLER rather than a
    /// function start (docs/core.md (call mechanisms)). Layout patches
    /// each to the reaching RelI32 displacement after the final converged
    /// layout, like a table fixup. Empty for every function the engine
    /// did not synthesize.
    pub site_fixups: Vec<(u32, usize, u32)>,
```

and `site_fixups: Vec::new(),` in the `FuncRef { … }` literal in `resolve`'s `order` map, and in every other `FuncRef { … }` construction (`crates/core/src/linker/stamp.rs:647-657` is the one in `mono_stamps`; `rewrite_blob` uses struct-update syntax and needs no change unless it spells the fields out — check).

**`prune_unreachable` MUST remap them.** Its reindex loop (`crates/core/src/linker/stamp.rs:483-496`) rewrites exactly two things today — `calls` and `bound` — and `site_fixups`' middle element is a function index of the same kind. Mono stamping orphans generics as a matter of course (that is what the prune is FOR), so an un-remapped fixup points at the wrong function the first time it fires, under pure mono as much as under hybrid. Add the third loop:

```rust
            for (_, target, _) in &mut f.site_fixups {
                *target = new_index[*target];
            }
```

immediately after the `bound` loop.

In `crates/core/src/linker/layout.rs`, after the per-function code loop and **before** the frames-region block, add:

```rust
    // Cross-function code fixups from mono's exit-bearing copies
    // (docs/core.md (call mechanisms)): each hole is the RelI32 operand
    // of a jump whose target is a position inside another function, so it
    // is patched here, after the final converged layout, exactly as a
    // table fixup is. The engine emitted a displacement of 0 as the
    // placeholder — a jump to the following boundary — so the blob decodes
    // cleanly during layout's own walk.
    for (fi, f) in order.iter().enumerate() {
        for &(hole, target_func, target_off) in &f.site_fixups {
            let Some(&here) = abs_of[fi].get(&hole.saturating_sub(1)) else {
                return Err(LinkError::MalformedBlob {
                    symbol: f.name.to_string(),
                    at: hole,
                });
            };
            let Some(&there) = abs_of[target_func].get(&target_off) else {
                return Err(LinkError::MalformedBlob {
                    symbol: order[target_func].name.to_string(),
                    at: target_off,
                });
            };
            let operand_at = (bases[fi] + here + 1) as usize;
            // The same typed refusal the two lookups above take: a fixup
            // whose operand falls outside the emitted code is malformed
            // blob data, never a slice panic.
            if operand_at + 4 > code.len() {
                return Err(LinkError::MalformedBlob {
                    symbol: f.name.to_string(),
                    at: hole,
                });
            }
            let end = i64::from(bases[fi] + here + 5);
            let off = i64::from(bases[target_func] + there) - end;
            let off32 = i32::try_from(off).expect("a spliced jump reaches within i32");
            code[operand_at..operand_at + 4].copy_from_slice(&off32.to_le_bytes());
        }
    }
```

- [ ] **Step 5: Teach mono to splice an exit-bearing site**

In `crates/core/src/linker/stamp.rs`:

(a) `StampNode` gains the site data:

```rust
/// One (routine, composite) pair to stamp, with its map-visible name. An
/// EXIT-BEARING node also carries the call site it splices: the caller's
/// function index, the post-rewrite offset of the instruction after the
/// call (`then`), and the exit vector (docs/core.md (call mechanisms)).
struct StampNode {
    routine: usize,
    composite: Composite,
    name: String,
    site: Option<SpliceSite>,
}

/// Where a mono exit-bearing copy returns to.
#[derive(Clone)]
struct SpliceSite {
    /// The calling function's index in `order`.
    caller: usize,
    /// The blob offset of the instruction after the call — where `ret`
    /// lands.
    then: u32,
    /// The blob offsets of the site's exits — where `retx #k` lands.
    exits: Vec<u32>,
}
```

(b) `intern` (`:679-706`) takes `site: Option<SpliceSite>` and widens the KEY (never the digest):

```rust
    let mut key = canonical_key(&composite);
    if let Some(s) = &site {
        key.extend_from_slice(&(s.caller as u64).to_le_bytes());
        key.extend_from_slice(&s.then.to_le_bytes());
        for e in &s.exits {
            key.extend_from_slice(&e.to_le_bytes());
        }
    }
```

with the name still `format!("{}.{:08x}", order[routine].name, digest(&composite))` — **plus a disambiguating suffix when the key collided on the name**: after `used_names.insert(name.clone())` fails, an exit-bearing node appends `.{n}` for the smallest `n` that is free rather than raising `StampNameCollision`. Two exit-bearing sites into the same routine under the same composite legitimately share a digest, so the collision is expected there and only there:

```rust
    let mut name = format!("{}.{:08x}", order[routine].name, digest(&composite));
    if site.is_some() {
        // Two exit-bearing splices of one (routine, composite) share a
        // digest by construction — the exits are not in it. Number them
        // rather than refuse (docs/core.md (the composition engine)).
        let base = name.clone();
        let mut n = 1u32;
        while used_names.contains(&name) {
            name = format!("{base}.{n}");
            n += 1;
        }
    }
    if !used_names.insert(name.clone()) {
        return Err(LinkError::StampNameCollision(name));
    }
```

(c) **`lower_mono`'s seed loop (`:150-166`) is unchanged.** It keeps pushing `(fi, *addr, *callee, record)`; every splice-site datum is derived inside `mono_stamps` from that same `record` and `addr` through `site_for` (below), so there is one construction site rather than two that can drift. `lower_mono` passes an all-empty `widened`, because mono rewrites no blob and therefore shifts nothing — which is what makes `site_for` the identity on the pure-mono path while still being correct on hybrid's.

(d) In `mono_stamps`, the seed interning (`:527-550`) passes the site — **with the offsets shifted into the post-rewrite blob layout** — through the one helper both loops use:

```rust
/// The `SpliceSite` for one exit-bearing site, with its caller offsets
/// shifted into the post-rewrite blob layout. `None` for an exit-free
/// site, which splices nothing.
fn site_for(
    caller: usize,
    addr: u32,
    record: &BoundCall,
    widened: &[HashSet<u32>],
) -> Option<SpliceSite> {
    if record.exits.is_empty() {
        return None;
    }
    let w = &widened[caller];
    Some(SpliceSite {
        caller,
        // Under mono the site keeps its 5-byte shape (a `jmp` where the
        // `call` was), so `then` is the very next instruction.
        then: splice_shift(w, addr + 5),
        exits: record.exits.iter().map(|&e| splice_shift(w, e)).collect(),
    })
}
```

called as `site_for(fi, addr, record, widened)` in the seed loop and `site_for(routine, *addr, record, widened)` in the closure's bound arm (`:583-622`).

**Why the shift is not optional, and why it is not `widen_shift`.** A splice fixup names an offset in the CALLER's blob, and layout's `abs_of` map is keyed by POST-rewrite offsets. Under pure mono nothing is rewritten, so the shift is the identity. Under **hybrid** it is not: `lower_hybrid` finishes by calling `lower_frames` over the mono-rewritten order, which widens every bound site that is still framed 5 → 9 bytes — shifting exactly the caller offsets these fixups name. A caller holding one spliced exit-bearing site and one framed holey site is the reachable shape, and an unshifted `then` then lands on the wrong instruction or misses `abs_of` entirely.

`widen_shift` (Task 6) counts the framed sites in a `SiteKind` list; here the widening set is known only after the mono/frames split, so `mono_stamps` takes it explicitly:

```rust
/// The offsets the frames path will shift, per function: the addresses
/// of the bound sites that are still framed once the mono/frames split
/// is decided. Empty for every function under pure mono, which rewrites
/// no blob at all.
fn splice_shift(widened: &HashSet<u32>, old: u32) -> u32 {
    old + 4 * u32::try_from(widened.iter().filter(|&&a| a < old).count())
        .expect("widened-site count fits u32")
}
```

`mono_stamps` gains a parameter `widened: &[HashSet<u32>]`, parallel to `order`. `lower_mono` passes `&vec![HashSet::new(); n]` (it never calls `lower_frames`, so nothing is widened). `lower_hybrid` passes, per function, the non-collapsing bound-site addresses **it did not turn into mono seeds** — which it knows before it calls `mono_stamps`, since `mono_holes` is complete by then:

```rust
    let widened: Vec<HashSet<u32>> = (0..n)
        .map(|fi| {
            sites[fi]
                .iter()
                .filter_map(|s| match s {
                    SiteKind::Bound {
                        addr,
                        collapse: false,
                        ..
                    } if !mono_holes[fi].contains(addr) => Some(*addr),
                    _ => None,
                })
                .collect()
        })
        .collect();
```

(e) `build_stamp` takes the site and does the rewrite. Its signature gains `site: Option<&SpliceSite>`; `StampBody` gains `site_fixups: Vec<(u32, usize, u32)>`. Inside the decode loop:

- Replace the `OperandKind::Imm8` arm (`:827-834`) with:
  ```rust
            OperandKind::Imm8 => {
                if entry.flow == Flow::Stop {
                    // A multi-exit return. Inside an exit-bearing splice
                    // it becomes a jump to exit `k` of the site's vector;
                    // anywhere else it is a frames instruction the base
                    // profile cannot run (docs/core.md (call mechanisms)).
                    let Some(s) = site else {
                        return Err(LinkError::MonoRawFrame(callee.name.to_string()));
                    };
                    let DecodedOperand::Int(k) = operand else {
                        unreachable!("Imm8 decodes to Int")
                    };
                    let Some(&target) = s.exits.get(*k as usize) else {
                        return Err(LinkError::BadBinding {
                            callee: callee.name.to_string(),
                            message: format!(
                                "the body returns through exit {k}, but the call site \
                                 supplies {} exit(s)",
                                s.exits.len()
                            ),
                        });
                    };
                    emit_splice_jump(&mut blob, jmp, s.caller, target, &mut site_fixups);
                    continue;
                }
                blob.extend_from_slice(&blob_bytes[old_addr as usize..(old_addr + d.len) as usize]);
            }
  ```
  (Adjust `DecodedOperand::Int` to whatever the real variant is — read `crates/core/src/asm/decode.rs` for the `Imm8` decode shape and use its exact name.)
- Replace the `OperandKind::None` arm (`:824-826`) with:
  ```rust
            OperandKind::None => {
                // Inside an exit-bearing splice the plain return becomes a
                // jump to the call site's continuation: the copy is
                // entered by `jmp`, so no return address was pushed
                // (docs/core.md (call mechanisms)). `stp`/`hlt` are left
                // alone — only the dialect's declared return is rewritten.
                if let Some(s) = site
                    && Some(entry.opcode) == syntax.return_opcode
                {
                    emit_splice_jump(&mut blob, jmp, s.caller, s.then, &mut site_fixups);
                    continue;
                }
                blob.extend_from_slice(&blob_bytes[old_addr as usize..(old_addr + d.len) as usize]);
            }
  ```
- `jmp` is bound once at the top of `build_stamp`:
  ```rust
      // Only an exit-bearing splice needs a jump opcode; an ordinary
      // stamp never rewrites control flow out of the copy.
      let jmp = match site {
          Some(_) => Some(syntax.jump_opcode().ok_or_else(|| LinkError::BadBinding {
              callee: callee.name.to_string(),
              message: "the dialect has no unconditional far jump to splice an \
                        exit-bearing call site into"
                  .to_string(),
          })?),
          None => None,
      };
  ```
  and `emit_splice_jump` takes `Option<u8>`, `expect`ing it (it is `Some` whenever `site` is):
  ```rust
  /// Emit `jmp <placeholder>` and record the cross-function fixup layout
  /// will patch (docs/core.md (call mechanisms)). The placeholder
  /// displacement is 0 — a jump to the following boundary — so the blob
  /// decodes cleanly during layout's own walk before the patch lands.
  fn emit_splice_jump(
      blob: &mut Vec<u8>,
      jmp: Option<u8>,
      caller: usize,
      target: u32,
      fixups: &mut Vec<(u32, usize, u32)>,
  ) {
      blob.push(jmp.expect("a splice always resolves its jump opcode"));
      let hole = blob.len() as u32;
      blob.extend_from_slice(&0i32.to_le_bytes());
      fixups.push((hole, caller, target));
  }
  ```

(f) The retarget loop in `lower_mono` (`:188-209`) additionally rewrites an exit-bearing site's OPCODE from the call to the far jump. **The `calls` entry it already pushes stays exactly as it is** — layout relocates a jump's displacement through the same `Piece::CallSite` path it relocates a call's, so dropping the entry would leave the displacement unpatched. Only the one opcode byte changes, in the caller's blob, which means `lower_mono` must own that blob (`Cow::to_mut`). Add, before the retarget loop:

```rust
    // An exit-bearing site is ENTERED by `jmp`, not `call` (P2: the copy
    // never returns through a pushed address) — docs/core.md (call
    // mechanisms). The site keeps its 5-byte shape and its relocation
    // hole, so only the opcode byte changes and no offset moves.
    let jmp = syntax.jump_opcode();
```

and inside the loop, AFTER the existing `f.calls.push((*addr + 1, target));`, for a site whose `record.exits` is non-empty and which is in the identity world:

```rust
                if !record.exits.is_empty() {
                    let op = jmp.ok_or_else(|| LinkError::BadBinding {
                        callee: order_names[*callee].clone(),
                        message: "the dialect has no unconditional far jump to enter \
                                  an exit-bearing copy with"
                            .to_string(),
                    })?;
                    f.blob.to_mut()[*addr as usize] = op;
                }
```

(`order_names` is a `Vec<String>` of the callee names captured before `order` is consumed; capture it at the top of `lower_mono` as `let order_names: Vec<String> = order.iter().map(|f| f.name.to_string()).collect();`.)

(g) `mono_stamps`' `FuncRef` construction (`:647-657`) carries `site_fixups: body.site_fixups`.

- [ ] **Step 6: Run the mono tests**

Run: `CARGO_TARGET_DIR=/Users/mellonis/Developer/mellonis-workspace/machines/toolchains/target cargo test -p mtc-core --test link_exits`
Expected: PASS.

- [ ] **Step 7: Run every gate**

Run: `CARGO_TARGET_DIR=/Users/mellonis/Developer/mellonis-workspace/machines/toolchains/target cargo test -p mtc-core && CARGO_TARGET_DIR=/Users/mellonis/Developer/mellonis-workspace/machines/toolchains/target cargo test -p mtc-post-machine && CARGO_TARGET_DIR=/Users/mellonis/Developer/mellonis-workspace/machines/toolchains/target cargo test -p mtc-turing-machine --test mode_equivalence --test mono_run --test opt_equivalence`
Expected: PASS. Mono stamp NAMES must be unchanged for every existing program (the digest did not widen) — if `mode_equivalence`'s byte-identity sweep goes red, the digest was widened by mistake.

- [ ] **Step 8: Commit**

```bash
git add crates/core/src/asm/syntax.rs crates/core/src/linker/layout.rs crates/core/src/linker/resolve.rs crates/core/src/linker/stamp.rs
git add $(grep -rln "ArchSyntax {" crates)
git commit -m "feat(core): mono splices an exit-bearing call site into a per-site copy"
```

The second `git add` stages exactly the set the grep in Step 3 edited — never `-A`, and never a hand-copied list that can go stale as earlier tasks add fake dialects.

Then `git log -1 --format=%B`; amend if a `Claude-Session:` line was appended.

---

### Task 8: Hybrid's exit-bearing fold (P4)

**P4.** Exit-free sites keep today's rule: a bijection site is a mono seed, a holey site goes to frames. Exit-bearing bijection sites reaching one `(routine, composite)` are **grouped across the whole reachable set** — including sites met inside stamped closures — and shared under frames **when sharing pays in bytes**: with `k` sites, a body of `B` bytes and descriptors of `d_i` bytes,

> share iff `k >= 2` and `(k - 1) * B > sum(d_i)`

otherwise each is a mono seed (a per-site splice by P2). Holey exit-bearing sites go to frames as before. `B` is the callee's `blob.len() + table.len()`; `d_i` is `materialize(...).len() + 4 * exits.len()`. Both are pre-layout counts, deterministic, nothing predicted at compile time — which is what the spec asks of any hybrid arm.

**The compose-column term is deliberately dropped.** The spec's formula adds "the compose-column entries", but that count depends on `K`, the directory size, which is not known until the directory is built — a circular dependency, and `mode_equivalence`'s relink byte-identity requires the decision to be deterministic. Dropping a second-order term is recorded in "Decisions for the controller".

**Why the existing matrices stay green:** the grouping rule fires only on exit-bearing sites, and no existing program has one. A reviewer should not have to re-derive that.

**Files:**
- Modify: `crates/core/src/linker/stamp.rs:253-363` (`lower_hybrid`), `:371-390` (`is_bijection` — unchanged, stated for the reader)
- Modify: `crates/core/src/linker/mod.rs` (`LinkReport.folds`)
- Modify: `crates/core/tests/link_exits.rs`

**Interfaces:**
- Consumes: `FoldDecision` (Task 2), `SpliceSite` and the mono splice (Task 7), `engine_exits` (Task 6).
- Produces: `LinkReport.folds: Vec<FoldDecision>`, sorted by `(routine, sites)` so the report is deterministic.

- [ ] **Step 1: Write the failing tests**

Append to `crates/core/tests/link_exits.rs`:

```rust
/// ONE exit-bearing site: the rule refuses on `k >= 2` alone, so the
/// site is seeded to mono and `TWO_EXITS` has no other bound site — which
/// means `any_frames` stays false and hybrid takes its **`!any_frames` →
/// `lower_mono` fast path**. That is precisely why `folds` has to be
/// attached to that return too: the decision was taken before the fast
/// path was chosen, and it is the only place it can be reported from.
///
/// Mutation it catches: leave `folds` off the `lower_mono` return (or
/// take the fast path before the decision loop) and `report.folds` comes
/// back empty, so the `expect` below fires. Separately, drop the
/// `k >= 2` conjunct and a single site shares under frames, producing a
/// frames region for nothing — which `composites == 0` forbids.
#[test]
fn one_exit_bearing_site_splices_under_hybrid() {
    let out = link(&fake_syntax(), &[asm(TWO_EXITS)], &[], opts(CallMech::Hybrid))
        .expect("links under hybrid");
    assert_eq!(out.report.composites, 0, "{:?}", out.report);
    let fold = out
        .report
        .folds
        .iter()
        .find(|f| f.routine == "sub")
        .unwrap_or_else(|| panic!("no fold decision survived the mono fast path: {:?}", out.report));
    assert!(!fold.shared, "{fold:?}");
    assert_eq!(fold.sites, 1, "{fold:?}");
    assert!(out.report.instantiations >= 1, "{:?}", out.report);
}

/// Three exit-bearing sites over a body big enough that two extra copies
/// cost more than three descriptors: hybrid shares them under frames.
///
/// Mutation it catches: invert the inequality and this shares nothing —
/// `composites` drops to 0 and `shared` goes false.
#[test]
fn three_exit_bearing_sites_over_a_large_body_share_under_hybrid() {
    let out = link(&fake_syntax(), &[asm(THREE_SITES_BIG_BODY)], &[], opts(CallMech::Hybrid))
        .expect("links under hybrid");
    let fold = out
        .report
        .folds
        .iter()
        .find(|f| f.routine == "big")
        .expect("a fold decision for `big`");
    assert!(fold.shared, "{fold:?}");
    assert_eq!(fold.sites, 3, "{fold:?}");
    // The arithmetic, pinned: if either number moves, the instruction
    // widths are not what the fixture assumes and N must be re-derived.
    assert_eq!(fold.body_bytes, 23, "1 ent + 20 nop + 2 retx: {fold:?}");
    assert_eq!(fold.descriptor_bytes, 36, "three 12-byte descriptors: {fold:?}");
    assert!(
        out.report.composites > 0,
        "a shared group frames: {:?}",
        out.report
    );
}
```

and the fixture, whose body is padded with exactly the `nop` count the arithmetic calls for — computable, not tuned, now that `descriptor_cost` is exact:

> Each site's binding is `[0]` over a 3-symbol caller into a 3-symbol
> callee, so the composite is the identity on one tape. `dense_map`
> returns EMPTY for an identity map at equal cardinalities
> (`crates/core/src/linker/engine.rs:802-804`), so both maps are
> zero-length and `descriptor_bytes` emits
> `1 (arity) + 2 (exit_count) + [1 (phys) + 2 + 0 + 2 + 0] + 4 (one exit)`
> = **12 bytes** per site. Three sites: `sum(d_i)` = **36**.
> `big`'s blob is the implicit 1-byte `ent` prologue + N `nop`s +
> 2 bytes of `retx #0`, and it carries no table, so `B = N + 3`.
> The rule shares iff `2 * B > 36`, i.e. `B > 18`, i.e. **N ≥ 16**.
> The fixture uses **N = 20** (`B = 23`, `2 * 23 = 46 > 36`) — clear of
> the boundary, so a one-byte drift in any instruction width does not
> silently flip the test's meaning.

Assert the arithmetic in the test rather than trusting it: `fold.body_bytes == 23` and `fold.descriptor_bytes == 36`. If either differs, the instruction widths are not what this note assumes — report the real numbers and re-derive N before touching the rule.

```rust
/// A body large enough that sharing three sites beats three copies:
/// 20 `nop`s, so `B` = 23 against `sum(d_i)` = 36 and `2 * 23 > 36`.
/// The flip point is 16 nops; see the arithmetic in the plan.
const THREE_SITES_BIG_BODY: &str = "\
.routine main, tapes=1, alpha=(3)
.param t, ('_', '0', '1')
.routine big, tapes=1, alpha=(3), exits=1
.param n, ('_', '0', '1')
.section code
.func main
        call    big [0] exits=(x)
        call    big [0] exits=(y)
        call    big [0] exits=(z)
        stp
x:      wr      [1]
        stp
y:      wr      [2]
        stp
z:      wr      [1]
        stp
.func big
        nop
        nop
        nop
        nop
        nop
        nop
        nop
        nop
        nop
        nop
        nop
        nop
        nop
        nop
        nop
        nop
        nop
        nop
        nop
        nop
        retx    #0
";
```

- [ ] **Step 2: Run to verify they fail**

Run: `CARGO_TARGET_DIR=/Users/mellonis/Developer/mellonis-workspace/machines/toolchains/target cargo test -p mtc-core --test link_exits hybrid`
Expected: FAIL — `LinkReport` has no `folds` field yet (compile error), which is the red state.

- [ ] **Step 3: Add `LinkReport.folds`**

In `crates/core/src/linker/mod.rs`, after `expanded_rows`:

```rust
    /// The hybrid exit-bearing fold decisions, sorted by routine then
    /// site count so the report is deterministic (docs/core.md (call
    /// mechanisms)). Empty under `mono` and `frames`, and for any image
    /// with no exit-bearing site.
    pub folds: Vec<FoldDecision>,
```

and `folds: lowered_folds,` in the `LinkReport { … }` literal (`crates/core/src/linker/mod.rs:534`), fed from the `Lowered.folds` Task 2 destructured as `_folds` — rename it to `folds` there.

- [ ] **Step 4: Group and decide in `lower_hybrid`**

In `crates/core/src/linker/stamp.rs`, `lower_hybrid`'s classification loop (`:264-289`) splits exit-bearing bijection sites out of `seeds` into a grouping map, keyed by `(callee, canonical_key(composite))`:

**The whole classify → group → decide sequence runs BEFORE both of `lower_hybrid`'s fast paths** (`seeds.is_empty()` → `lower_frames` at `:292-295`, and `!any_frames` → `lower_mono` at `:296-298`). It has to: the decision loop is what fills `seeds` for a spliced group and what sets `any_frames` for a shared one, so a fast path taken ahead of it would branch on a state the rule has not produced yet. Move both `if`s below the decision loop.

```rust
    // Exit-bearing bijection sites are grouped rather than seeded
    // directly: whether they share one framed body or each splice a copy
    // is a byte count over the WHOLE group (docs/core.md (call
    // mechanisms)).
    let mut groups: HashMap<(usize, Vec<u8>), Vec<(usize, u32, &BoundCall)>> = HashMap::new();
```

Inside the loop, for a non-collapsing bound site whose binding `is_bijection`:

```rust
                    if record.exits.is_empty() {
                        seeds.push((fi, *addr, *callee, record));
                        mono_holes[fi].insert(*addr);
                    } else {
                        let composite =
                            compose(&identity_composite(machine_sig.arity as usize, 0),
                                    caller_sig.cardinalities.as_slice(),
                                    *callee, &record.binding, callee_sig)
                                .map_err(|e| bad_binding(&order[*callee].name, &e))?;
                        groups
                            .entry((*callee, canonical_key(&composite)))
                            .or_default()
                            .push((fi, *addr, record));
                    }
```

Then, after the loop, decide per group and record it:

```rust
    // The byte rule (docs/core.md (call mechanisms)): with `k` sites, a
    // body of `B` bytes and would-be descriptors of `d_i` bytes, share iff
    // `k >= 2 && (k - 1) * B > sum(d_i)`. Both counts are pre-layout, so
    // the decision is deterministic and a relink is byte-identical.
    let mut folds: Vec<super::FoldDecision> = Vec::new();
    let mut keys: Vec<&(usize, Vec<u8>)> = groups.keys().collect();
    keys.sort();
    for key in keys {
        let sites_in_group = &groups[key];
        let callee = key.0;
        let body = u32::try_from(order[callee].blob.len() + order[callee].table.len())
            .expect("a body size fits u32");
        let k = u32::try_from(sites_in_group.len()).expect("a group size fits u32");
        // Each site's would-be descriptor, EXACTLY: the bytes
        // `materialize` will emit for that composite and that exit
        // vector.
        let descriptors: u32 = sites_in_group
            .iter()
            .map(|(_, _, record)| {
                descriptor_cost(&group_composite[key], machine_sig, &order, &record.exits)
            })
            .sum::<Result<u32, LinkError>>()?;
        let shared = k >= 2 && u64::from(k - 1) * u64::from(body) > u64::from(descriptors);
        folds.push(super::FoldDecision {
            routine: order[callee].name.to_string(),
            sites: k,
            body_bytes: body,
            descriptor_bytes: descriptors,
            shared,
        });
        if shared {
            any_frames = true; // the group stays in `f.bound` for the frames path
        } else {
            for &(fi, addr, record) in sites_in_group {
                seeds.push((fi, addr, callee, record));
                mono_holes[fi].insert(addr);
            }
        }
    }
    folds.sort_by(|a, b| (&a.routine, a.sites).cmp(&(&b.routine, b.sites)));
```

with a **per-group composite map** built alongside `groups` in the classification loop (`group_composite.entry(key).or_insert(composite)` — every member of a group shares the key's composite by construction, so the first one is the group's), and:

```rust
/// The bytes a site's frames descriptor costs — EXACT, not an estimate.
/// The composite is already known at decision time (it is what the
/// group key was computed from), and `materialize` needs only the
/// machine signature and the callee's own signature, neither of which
/// the blob rewrite touches. So the size is taken from the bytes
/// themselves rather than re-derived from a second formula that could
/// disagree with the emitter (docs/core.md (call mechanisms)).
fn descriptor_cost(
    composite: &Composite,
    machine_sig: &RoutineSig,
    order: &[FuncRef],
    exits: &[u32],
) -> Result<u32, LinkError> {
    Ok(
        u32::try_from(super::engine::materialize(composite, machine_sig, order, exits)?.len())
            .expect("a descriptor size fits u32"),
    )
}
```

This needs `engine::materialize` to be `pub(super)` — Task 8b's Interfaces record the same change, so whichever task lands first makes it and the other finds it done.

**Thread `folds` into ALL THREE of `lower_hybrid`'s returns**, not two. The two fast paths return somebody else's `Lowered`, so each is wrapped and the field attached:

```rust
    if seeds.is_empty() {
        let (order, plan, stats) = lower_frames(syntax, order, sites, machine_sig)?;
        return Ok(Lowered {
            order,
            plan,
            stats,
            orphaned: Vec::new(),
            diagnostics: Vec::new(),
            folds,
        });
    }
    if !any_frames {
        let mut lowered = lower_mono(syntax, order, sites, machine_sig)?;
        lowered.folds = folds;
        return Ok(lowered);
    }
```

and the mixed path's own `Lowered` carries `folds` directly. `lower_mono` called on its OWN (under `CallMech::Mono`) still returns `folds: Vec::new()` — a mono link takes no fold decisions, because nothing is ever shared.

**The `widened` set Task 7 introduced must be computed AFTER this loop, not before it.** A group the rule SHARES stays in `f.bound` and is widened by the frames path; a group it SPLICES joins `mono_holes` and is not. Build `widened` from the final `mono_holes` immediately before the `mono_stamps` call, exactly as Task 7 spells it — if it is built earlier, every shared group's widening is missing from the splice offsets of any site that follows it in the same blob.

- [ ] **Step 4b: Check `LinkReport`'s consumers before running anything**

`folds` is populated under hybrid and empty under mono and frames **by construction**, so any consumer that compares whole reports across mechanisms, or destructures `LinkReport` exhaustively, breaks the moment this field lands.

Run: `grep -n "LinkReport" crates/turing-machine/tests/mode_equivalence.rs crates/post-machine/tests/*.rs crates/core/tests/*.rs`

For each hit: an exhaustive `let LinkReport { … } = …` without a trailing `..` stops compiling — add the `..`. An `assert_eq!` on two whole reports from different mechanisms goes red — narrow it to the field subset it actually means to pin, and say in a comment that `folds` is mechanism-specific. Report anything you find that is neither of those two shapes before changing it.

- [ ] **Step 5: Leave the closure grouping to Task 8b, and say so in the code**

This task groups the sites visible at the machine's own frame. Task 8b widens the count to the whole reachable set, which is the ratified rule — but it needs a probe pass this task's structure does not have yet, and splitting them keeps each commit green and reviewable.

Write the group loop so 8b extends it rather than rewrites it: build `groups` with the key `(callee, canonical_key(&composite))` and nothing else, so a closure site can land in an existing group by key alone. Add the comment:

```rust
    // The fold count runs over the whole reachable set — an exit-bearing
    // site met inside a stamped copy joins its group by the same
    // (routine, composite) key (docs/core.md (call mechanisms)). This
    // loop sees the machine-frame sites; the closure probe that reports
    // the rest merges into the same map before the decision is taken.
```

**Do not** state anywhere, in code or docs, that the grouping is identity-world-only: it is not the rule, and it would be wrong the moment 8b lands.

- [ ] **Step 6: Run the tests and gates**

Run: `CARGO_TARGET_DIR=/Users/mellonis/Developer/mellonis-workspace/machines/toolchains/target cargo test -p mtc-core --test link_exits`
Expected: PASS, with the `nop` count in `THREE_SITES_BIG_BODY` tuned until `shared` is true.

Run: `CARGO_TARGET_DIR=/Users/mellonis/Developer/mellonis-workspace/machines/toolchains/target cargo test -p mtc-core && CARGO_TARGET_DIR=/Users/mellonis/Developer/mellonis-workspace/machines/toolchains/target cargo test -p mtc-turing-machine --test mode_equivalence --test opt_equivalence && CARGO_TARGET_DIR=/Users/mellonis/Developer/mellonis-workspace/machines/toolchains/target cargo test -p mtc-post-machine`
Expected: PASS. `LinkReport` gained a field but nothing constructs one by literal outside `crates/core/src/linker/mod.rs:534`, so no other crate needs a change.

- [ ] **Step 7: Commit**

```bash
git add crates/core/src/linker/mod.rs crates/core/src/linker/stamp.rs crates/core/tests/link_exits.rs
git commit -m "feat(core): hybrid shares exit-bearing sites only when sharing pays in bytes"
```

Then `git log -1 --format=%B`; amend if a `Claude-Session:` line was appended.

---

### Task 8b: Hybrid grouping inside stamped closures

Task 8 groups over the identity world only. The ratified rule is wider: **the count runs over the whole reachable set, not only the identity world.** An exit-bearing site met while `mono_stamps` copies a routine under composite `C` joins its group, and when the group shares, the stamped copy carries a `call.m` to the shared body through a descriptor `compose(C, binding)` plus the site's exits — sound by P1, and the same mixed image hybrid already produces.

**The ordering problem, and its resolution.** Hybrid cannot decide a group before it knows the group's size, and it cannot know the closure's contents before it decides — a stamp that reaches a SHARED callee emits a framed call instead of recursing into a child stamp, so the closure's shape depends on the decision. The resolution is two passes over one walk:

1. **Probe.** Enumerate the `(routine, composite)` closure from the identity-world mono seeds, building no bodies, and report every exit-bearing bijection site met inside a copy with its binding already composed against the enclosing composite.
2. **Decide.** Merge those into `groups` alongside the identity-world sites and apply Task 8's byte rule unchanged.
3. **Build.** Run `mono_stamps` again, told which `(routine, composite)` pairs are shared. At such a site the stamp emits a framed call rather than interning a child.

The walk is written once and used by both passes, so the probe cannot drift from the build.

**Why the descriptor needs no new mechanism.** A stamp's framed call is a hand-authored-shaped `call.m`: its descriptor goes into the stamp's OWN table blob with the exits as stamp-blob-relative offsets, which is exactly what `layout::append_frame_descriptor` already rebases for a raw `.frame` (`crates/core/src/linker/layout.rs:440-499`). `lower_frames` then sees it as `SiteKind::RawCallM`, gives it a directory entry and a CONSTANT compose column (`crates/core/src/linker/engine.rs:295-329`) — correct, because `compose(C, binding)` is already absolute and does not vary with the active frame. Under pure mono this shape never arises: a shared group sets `any_frames`, so the image is a frames image by construction.

**Files:**
- Modify: `crates/core/src/linker/stamp.rs:253-363` (`lower_hybrid` — the probe, the widened group, the shared set), `:505-661` (`mono_stamps` — the shared-aware closure), `:741-1020` (`build_stamp` — the framed emission), plus the new `mono_closure_probe`, `GroupSite`, `ClosureSite`, `StampTarget`
- Modify: `crates/core/src/linker/engine.rs:749-790` (`materialize` becomes `pub(super)`; Task 8's exact `descriptor_cost` may have made this already — if so, leave it)
- Modify: `crates/core/tests/link_exits.rs`
- Create: `crates/turing-machine/tests/link_matrix.rs` (its harness and the closure-fold run test; Task 16 appends the rest)
- Modify: `crates/turing-machine/tests/mode_equivalence.rs` (add `CLOSURE_FOLD` to the relink byte-identity list)

**Interfaces:**

- **Consumes from Task 8** — by these exact names:
  - `groups: HashMap<(usize, Vec<u8>), Vec<…>>` in `lower_hybrid`, keyed `(callee order index, canonical_key(&composite))`. **Task 8b widens its element type** from `(usize, u32, &BoundCall)` to `GroupSite<'a>` (below); the key is unchanged, which is what lets identity-world and closure sites land in one group.
  - `descriptor_cost(record: &BoundCall) -> u32` — used unchanged for a closure site too. Accepted as an estimate.
  - The byte rule `k >= 2 && (k - 1) * B > sum(d_i)`, with `B = order[callee].blob.len() + order[callee].table.len()`. **The compose-column term stays dropped.**
  - `FoldDecision { routine, sites, body_bytes, descriptor_bytes, shared }` and `LinkReport.folds` — `sites` now counts closure sites too.
  - `any_frames`, `mono_holes`, and the `widened: Vec<HashSet<u32>>` build that Task 8 ordered after the group loop.
- **Consumes from Task 7**: `SpliceSite`, `splice_shift`, `mono_stamps`' `widened: &[HashSet<u32>]` parameter, `FuncRef.site_fixups`.
- **Consumes from Task 6**: `engine::materialize(c, machine_sig, order, exits)` — made `pub(super)` here.
- **Produces:**
  ```rust
  /// One exit-bearing bijection site in a fold group. An identity-world
  /// site names the calling function in `order`; a CLOSURE site names
  /// the routine whose copy it was met inside, and carries the composite
  /// it reaches — already composed against that copy's own composite, so
  /// the descriptor a shared site needs is the composite itself.
  enum GroupSite<'a> {
      Identity {
          caller: usize,
          addr: u32,
          record: &'a BoundCall,
      },
      InClosure {
          routine: usize,
          record: &'a BoundCall,
      },
  }

  /// One exit-bearing bijection site the probe met inside a copy.
  struct ClosureSite<'a> {
      /// The routine whose body the site sits in.
      routine: usize,
      /// The callee the site reaches.
      callee: usize,
      record: &'a BoundCall,
      /// `compose(enclosing, binding)` — absolute, and exactly the
      /// descriptor a shared site is given.
      composite: Composite,
  }

  /// What a stamped body does at one of its own call sites.
  enum StampTarget {
      /// A plain call into a child stamp, or into the original on a
      /// full pass-through: today's behaviour.
      Plain(usize),
      /// A framed call into the ONE generic copy of the callee, through
      /// a descriptor the stamp carries in its own table blob.
      Framed {
          callee: usize,
          composite: Composite,
          /// The site's exits, as offsets in the ENCLOSING routine's
          /// original blob; `build_stamp` remaps them into the stamp's
          /// own blob before writing them into the descriptor.
          exits: Vec<u32>,
      },
  }

  fn mono_closure_probe<'a>(
      order: &[FuncRef<'a>],
      sites: &[Vec<SiteKind<'a>>],
      machine_sig: &RoutineSig,
      seeds: &[(usize, u32, usize, &'a BoundCall)],
  ) -> Result<Vec<ClosureSite<'a>>, LinkError>;
  ```
  and `mono_stamps` gains a final parameter `shared: &HashSet<(usize, Vec<u8>)>` — the `(callee, canonical_key(&composite))` pairs whose group the byte rule shared. `lower_mono` passes an empty set.

- [ ] **Step 1: Write the failing test**

Append to `crates/core/tests/link_exits.rs`. **[shape-copied]** from `link_interface.rs`'s two-function skeleton and `asm_interface.rs`'s `.param` lines; the TM-1 twin in Step 6 is **[tool-verified]** (assembled 2026-09-14).

```rust
/// `big` is reached from ONE exit-bearing site at the identity world
/// (`main`'s, through a swap binding) and from TWO more inside `outer`'s
/// stamped copy (transparent, so they compose to the same composite the
/// swap does). The three are one group only if the closure sites are
/// counted.
///
/// The arithmetic, EXACT and stated so the fixture is auditable rather
/// than tuned. All three sites reach the SAME composite — the swap —
/// because `outer` is stamped under it and its two calls to `big` are
/// transparent, so `compose(C_swap, identity)` is `C_swap` again. That
/// composite's maps are not identity, so `dense_map` emits one `u16`
/// per symbol in each direction over the 4-symbol alphabet and
/// `descriptor_bytes` writes
/// `1 (arity) + 2 (exit_count) + [1 (phys) + 2 + 2*4 + 2 + 2*4] + 4 (one exit)`
/// = **28 bytes** per site — the same 28 for the swap site and for each
/// transparent one, since they share the composite. `sum(d_i)` = **84**.
/// `big`'s blob is the implicit 1-byte `ent` prologue + 50 `nop`s +
/// 2 bytes of `retx #0`, with no table, so `B` = **53**. With all three
/// sites, `(3 - 1) * 53 = 106 > 84` and the group SHARES; the flip point
/// is 40 nops, so the fixture is clear of the boundary. With only the
/// identity-world site, `k` is 1 and the rule refuses on `k >= 2` alone,
/// whatever `B` is.
///
/// Mutation it catches: drop the probe (group over the identity world
/// only) and `fold.sites` is 1 and `fold.shared` is false — the two
/// closure sites each splice a per-site copy instead, which
/// `instantiations` shows.
const CLOSURE_FOLD: &str = "\
.routine main, tapes=1, alpha=(4)
.param t, ('_', 'x', 'y', 'z')
.routine outer, tapes=1, alpha=(4)
.param u, ('_', 'x', 'y', 'z')
.routine big, tapes=1, alpha=(4), exits=1
.param n, ('_', 'x', 'y', 'z')
.section code
.func main
        call    outer [0{1->2, 2->1}]
        call    big [0{1->2, 2->1}] exits=(a)
        stp
a:      wr      [1]
        stp
.func outer
        call    big [0] exits=(p)
        ret
p:      call    big [0] exits=(q)
        ret
q:      ret
.func big
        nop
        nop
        nop
        nop
        nop
        nop
        nop
        nop
        nop
        nop
        nop
        nop
        nop
        nop
        nop
        nop
        nop
        nop
        nop
        nop
        nop
        nop
        nop
        nop
        nop
        nop
        nop
        nop
        nop
        nop
        nop
        nop
        nop
        nop
        nop
        nop
        nop
        nop
        nop
        nop
        nop
        nop
        nop
        nop
        nop
        nop
        nop
        nop
        nop
        nop
        retx    #0
";

#[test]
fn exit_bearing_sites_inside_a_stamped_copy_join_their_group() {
    let out = link(&fake_syntax(), &[asm(CLOSURE_FOLD)], &[], opts(CallMech::Hybrid))
        .expect("links under hybrid");
    let fold = out
        .report
        .folds
        .iter()
        .find(|f| f.routine == "big")
        .unwrap_or_else(|| panic!("no fold decision for `big`: {:?}", out.report.folds));
    assert_eq!(
        fold.sites, 3,
        "one identity-world site plus two inside the copy: {fold:?}"
    );
    assert_eq!(
        fold.body_bytes, 53,
        "1 ent + 50 nop + 2 retx: {fold:?}"
    );
    assert_eq!(
        fold.descriptor_bytes, 84,
        "three 28-byte descriptors over one shared composite: {fold:?}"
    );
    assert!(fold.shared, "106 > 84, so the group shares: {fold:?}");
}

/// A shared closure site becomes a framed call inside the copy, NOT a
/// child stamp: `outer` is stamped once and `big` keeps its single
/// generic copy, so the image carries exactly one stamp.
///
/// Mutation it catches: ignore the `shared` set in `mono_stamps` and the
/// copy interns two child stamps of `big`, so `instantiations` is 3
/// instead of 1.
#[test]
fn a_shared_closure_site_frames_instead_of_stamping_a_child() {
    let out = link(&fake_syntax(), &[asm(CLOSURE_FOLD)], &[], opts(CallMech::Hybrid))
        .expect("links under hybrid");
    assert_eq!(
        out.report.instantiations, 1,
        "only `outer` is stamped; `big` stays generic: {:?}",
        out.report
    );
    assert!(
        out.report.composites >= 2,
        "the shared body needs a directory entry per site: {:?}",
        out.report
    );
    // `big` survives as a generic routine — a shared group's whole point.
    assert!(
        out.map.functions.iter().any(|f| f.name == "big"),
        "the shared body must be in the image"
    );
}

/// The same program under the other two mechanisms: mono splices
/// everything, frames descriptors everything, and both must still link.
/// The three images differ; what must not differ is that each is
/// well-formed and reproducible.
///
/// Mutation it catches: emit the stamp's descriptor with exits in the
/// ENCLOSING routine's offsets (skip the `old_to_new` remap) and layout
/// rejects the raw descriptor as malformed table data, so hybrid stops
/// linking while mono and frames still do.
#[test]
fn the_closure_fold_program_links_and_relinks_under_every_mechanism() {
    for mech in MECHS {
        let a = link(&fake_syntax(), &[asm(CLOSURE_FOLD)], &[], opts(mech))
            .unwrap_or_else(|e| panic!("under {mech}: {e}"));
        let b = link(&fake_syntax(), &[asm(CLOSURE_FOLD)], &[], opts(mech))
            .unwrap_or_else(|e| panic!("under {mech}: {e}"));
        assert_eq!(
            a.executable.to_bytes(),
            b.executable.to_bytes(),
            "the {mech} image is not reproducible"
        );
    }
}
```

- [ ] **Step 2: Run to verify they fail**

Run: `CARGO_TARGET_DIR=/Users/mellonis/Developer/mellonis-workspace/machines/toolchains/target cargo test -p mtc-core --test link_exits closure`
Expected: FAIL — `exit_bearing_sites_inside_a_stamped_copy_join_their_group` reports `sites: 1, shared: false`, and `instantiations` is 3 rather than 1.

- [ ] **Step 3: Write the probe**

In `crates/core/src/linker/stamp.rs`, add the three types from **Interfaces** above, then the probe. It is the closure BFS of `mono_stamps` (`:556-629`) with the body building removed:

```rust
/// Enumerate the `(routine, composite)` closure from the identity-world
/// mono seeds WITHOUT building any body, reporting every exit-bearing
/// bijection site met inside a copy with its binding already composed
/// against the enclosing composite (docs/core.md (call mechanisms)).
///
/// Hybrid needs this before it can size a fold group: a group's members
/// are not all visible at the machine's own frame, and a stamp that
/// reaches a SHARED callee emits a framed call rather than recursing, so
/// the closure's own shape depends on the decision. Probing first, then
/// deciding, then building is what breaks that circle.
fn mono_closure_probe<'a>(
    order: &[FuncRef<'a>],
    sites: &[Vec<SiteKind<'a>>],
    machine_sig: &RoutineSig,
    seeds: &[(usize, u32, usize, &'a BoundCall)],
) -> Result<Vec<ClosureSite<'a>>, LinkError> {
    let ma = machine_sig.arity as usize;
    let id = identity_composite(ma, 0);
    let mut visited: HashSet<(usize, Vec<u8>)> = HashSet::new();
    let mut queue: VecDeque<(usize, Composite)> = VecDeque::new();
    let mut met: Vec<ClosureSite<'a>> = Vec::new();

    let caller_cards = |fi: usize| -> &[u32] {
        order[fi]
            .signature
            .map(|s| s.cardinalities.as_slice())
            .unwrap_or(machine_sig.cardinalities.as_slice())
    };

    for &(fi, _, callee, record) in seeds {
        let callee_sig = routine_sig(order, callee)?;
        let child = compose(&id, caller_cards(fi), callee, &record.binding, callee_sig)
            .map_err(|e| bad_binding(&order[callee].name, &e))?;
        queue.push_back((callee, child));
    }

    while let Some((routine, comp)) = queue.pop_front() {
        if !visited.insert((routine, canonical_key(&comp))) {
            continue;
        }
        for site in &sites[routine] {
            match site {
                // A raw framed call inside a copy is the mono refusal
                // `mono_stamps` raises; leave it to raise it, so the
                // probe never changes which error a link reports.
                SiteKind::RawCallM { .. } => {}
                SiteKind::Plain { callee, .. } => {
                    let mut child = comp.clone();
                    child.routine = *callee;
                    queue.push_back((*callee, child));
                }
                SiteKind::Bound {
                    addr,
                    callee,
                    record,
                    ..
                } => {
                    let callee_sig = routine_sig(order, *callee)?;
                    let child = compose(
                        &comp,
                        caller_cards(routine),
                        *callee,
                        &record.binding,
                        callee_sig,
                    )
                    .map_err(|e| bad_binding(&order[*callee].name, &e))?;
                    // An exit-bearing bijection site is a fold-group
                    // candidate wherever it sits. Everything else keeps
                    // descending exactly as the builder will.
                    if !record.exits.is_empty()
                        && is_bijection(order[routine].signature.unwrap_or(machine_sig), callee_sig, record)
                    {
                        met.push(ClosureSite {
                            routine,
                            callee: *callee,
                            record,
                            composite: child.clone(),
                        });
                    }
                    if !(record.exits.is_empty()
                        && is_full_passthrough(&child, machine_sig, callee_sig))
                    {
                        queue.push_back((*callee, child));
                    }
                }
            }
        }
    }
    Ok(met)
}
```

- [ ] **Step 4: Merge the probe into the group loop**

In `lower_hybrid`, after the identity-world classification loop and **before** the group decision loop Task 8 added:

```rust
    // The fold count runs over the WHOLE reachable set, not only the
    // identity world (docs/core.md (call mechanisms)): an exit-bearing
    // site met while a routine is copied under composite `C` joins its
    // group, and a shared group is reached from inside that copy through
    // a descriptor `compose(C, binding)` plus the site's exits.
    for cs in mono_closure_probe(&order, sites, machine_sig, &seeds)? {
        groups
            .entry((cs.callee, canonical_key(&cs.composite)))
            .or_default()
            .push(GroupSite::InClosure {
                routine: cs.routine,
                record: cs.record,
            });
    }
```

and change the identity-world push to the enum form:

```rust
                        groups
                            .entry((*callee, canonical_key(&composite)))
                            .or_default()
                            .push(GroupSite::Identity {
                                caller: fi,
                                addr: *addr,
                                record,
                            });
```

The decision loop's `descriptors` sum reads the record out of either variant — and stays EXACT, through the group's own composite:

```rust
        let descriptors: u32 = sites_in_group
            .iter()
            .map(|s| {
                let record = match s {
                    GroupSite::Identity { record, .. }
                    | GroupSite::InClosure { record, .. } => *record,
                };
                descriptor_cost(&group_composite[key], machine_sig, &order, &record.exits)
            })
            .sum::<Result<u32, LinkError>>()?;
```

and the `else` branch (splice) seeds only the identity-world members — a closure member is spliced by `mono_stamps` itself, which is what it already does when the pair is not in `shared`:

```rust
        if shared {
            any_frames = true;
            shared_pairs.insert(key.clone());
        } else {
            for s in sites_in_group {
                if let GroupSite::Identity { caller, addr, record } = s {
                    seeds.push((*caller, *addr, callee, *record));
                    mono_holes[*caller].insert(*addr);
                }
            }
        }
```

with `let mut shared_pairs: HashSet<(usize, Vec<u8>)> = HashSet::new();` declared above the loop and passed to `mono_stamps`. **`widened` is still built after this loop**, per Task 8's ordering note — a shared group's identity-world sites stay in `f.bound` and ARE widened.

`lower_mono`'s `mono_stamps` call passes `&HashSet::new()`.

- [ ] **Step 5: Teach the builder to frame a shared site**

In `mono_stamps`, change `stamp_targets` to `Vec<HashMap<u32, StampTarget>>` and, in the closure's Bound arm, branch on the shared set before interning:

```rust
                SiteKind::Bound {
                    addr,
                    callee,
                    record,
                    ..
                } => {
                    let callee_sig = routine_sig(order, *callee)?;
                    let caller_cards = order[routine]
                        .signature
                        .map(|s| s.cardinalities.as_slice())
                        .unwrap_or(machine_sig.cardinalities.as_slice());
                    let child = compose(&comp, caller_cards, *callee, &record.binding, callee_sig)
                        .map_err(|e| bad_binding(&order[*callee].name, &e))?;
                    // A group the byte rule shared is reached through ONE
                    // generic copy: the stamp frames the call instead of
                    // minting a child (docs/core.md (call mechanisms)).
                    if shared.contains(&(*callee, canonical_key(&child))) {
                        targets.insert(
                            *addr,
                            StampTarget::Framed {
                                callee: *callee,
                                composite: child,
                                exits: record.exits.clone(),
                            },
                        );
                        continue;
                    }
                    let idx = if record.exits.is_empty()
                        && is_full_passthrough(&child, machine_sig, callee_sig)
                    {
                        *callee
                    } else {
                        let (idx, dup) = intern(
                            &mut nodes,
                            &mut key_to_slot,
                            &mut worklist,
                            &mut used_names,
                            order,
                            *callee,
                            child,
                            site_for(routine, *addr, record, widened),
                        )?;
                        if dup {
                            stats.dedup_savings += 1;
                        }
                        idx
                    };
                    targets.insert(*addr, StampTarget::Plain(idx));
                }
```

(`site_for` is the `SpliceSite` constructor Task 7's step (d) spells inline; factor it into a helper there or repeat it here — either way one spelling.)

In `build_stamp`, replace the `targets.get(&old_addr)` block (`:815-821`) with:

```rust
        match targets.get(&old_addr) {
            Some(StampTarget::Plain(target)) => {
                blob.push(entry.opcode);
                let hole = blob.len() as u32;
                blob.extend_from_slice(&[0u8; 4]);
                calls.push((hole, *target));
                continue;
            }
            Some(StampTarget::Framed {
                callee: target,
                composite,
                exits,
            }) => {
                // A framed call into the shared generic body: opcode,
                // displacement (relocated to the body like a far call),
                // then the frame half, which names a descriptor in THIS
                // stamp's own table blob — the hand-authored `.frame`
                // shape, which layout already rebases
                // (docs/formats.md (frame descriptors)).
                let fc = syntax
                    .framed_call_opcode()
                    .ok_or_else(|| LinkError::BadBinding {
                        callee: callee.name.to_string(),
                        message: "the dialect has no framed-call opcode to reach a \
                                  shared exit-bearing body"
                            .to_string(),
                    })?;
                blob.push(fc);
                let disp = blob.len() as u32;
                blob.extend_from_slice(&[0u8; 4]);
                calls.push((disp, *target));
                let frame_hole = blob.len() as u32;
                blob.extend_from_slice(&[0u8; 4]);
                // The descriptor, with placeholder exits: they are the
                // ENCLOSING routine's offsets here and are remapped into
                // this stamp's own blob once the body is emitted.
                let desc_off = table.len() as u32;
                let placeholders: Vec<u32> = exits.clone();
                let bytes = crate::linker::engine::materialize(
                    composite,
                    machine_sig,
                    order_for_materialize,
                    &placeholders,
                )?;
                let exits_at = table.len() + bytes.len() - 4 * placeholders.len();
                table.extend_from_slice(&bytes);
                for (i, &old) in placeholders.iter().enumerate() {
                    frame_exit_fixups.push((exits_at + 4 * i, old));
                }
                table_fixups.push((frame_hole, desc_off));
                continue;
            }
            None => {}
        }
```

`build_stamp` gains `machine_sig` (it already has it), a `frame_exit_fixups: Vec<(usize, u32)>` accumulator, and an `order_for_materialize: &[FuncRef]` parameter — `materialize` needs the callee's signature to size its dense maps, and it reads it from the order by `composite.routine`. Pass `mono_stamps`' own `order`.

After the jump patching, before `Ok(StampBody { … })`, resolve them:

```rust
    // A framed call's descriptor names its exits as offsets in THIS
    // stamp's blob; layout rebases those to absolute addresses the way
    // it does for any hand-authored descriptor
    // (docs/formats.md (frames region)).
    for (pos, old) in frame_exit_fixups {
        let new = *old_to_new.get(&old).ok_or(LinkError::MalformedBlob {
            symbol: callee.name.to_string(),
            at: old,
        })?;
        table[pos..pos + 4].copy_from_slice(&new.to_le_bytes());
    }
```

Finally, make `materialize` reachable: change `fn materialize(` to `pub(super) fn materialize(` in `crates/core/src/linker/engine.rs:749` and import it in `stamp.rs`'s `use super::engine::{…}` list.

- [ ] **Step 6: Run the core tests, then add the TM run-equivalence half**

Run: `CARGO_TARGET_DIR=/Users/mellonis/Developer/mellonis-workspace/machines/toolchains/target cargo test -p mtc-core --test link_exits`
Expected: PASS, all of Tasks 6, 7, 8 and 8b.

Then **CREATE** `crates/turing-machine/tests/link_matrix.rs` — this task is the file's first author; Task 16 appends to it. Set up its harness by copying `fn build`/`fn run`/`fn cell_at` verbatim from `crates/turing-machine/tests/mono_run.rs:17-63` (`build(src, mech)` assembles and links under a mechanism; `run(exe, widths)` runs on blank tapes and returns `(Outcome, Vec<TapeSnapshot>)`; `cell_at(snap, pos)` reads one absolute cell). **Keep the `drop(devices);` line before `to_snapshot()`** — it is a required borrow release, not decoration. Add the module header and

```rust
const MECHS: [CallMech; 3] = [CallMech::Mono, CallMech::Frames, CallMech::Hybrid];
```

then the TM-1 twin — **[tool-verified]**, assembled 2026-09-14 — and its equivalence test. This is where a mixed image is proven equivalent by RUNNING it; core has no VM harness of its own.

```rust
/// The closure-fold shape on TM-1: `big` is reached once at the identity
/// world and twice inside `outer`'s stamped copy, so hybrid shares the
/// body and the image carries both a stamp and a frames region. Mono
/// splices all three; frames descriptors all three. All three must leave
/// the same tape. The 50-`nop` body is the core test's arithmetic
/// (`B` = 53 against `sum(d_i)` = 84); this twin only has to RUN.
///
/// Mutation it catches: build the stamp's descriptor from the SITE's
/// binding instead of `compose(C, binding)` and the copy reads `big`
/// through the wrong symbol map, so hybrid's tape stops matching mono's.
const CLOSURE_FOLD: &str = "\
.routine main, tapes=1, alpha=(4)
.param t, ('_', 'x', 'y', 'z')
.routine outer, tapes=1, alpha=(4)
.param u, ('_', 'x', 'y', 'z')
.routine big, tapes=1, alpha=(4), exits=1
.param n, ('_', 'x', 'y', 'z')
.section code
.func main
        call    outer [0{1->2, 2->1}]
        call    big [0{1->2, 2->1}] exits=(a)
        stp
a:      wrmv    [1], [.]
        stp
.func outer
        call    big [0] exits=(p)
        ret
p:      call    big [0] exits=(q)
        ret
q:      ret
.func big
        nop
        nop
        nop
        nop
        nop
        nop
        nop
        nop
        nop
        nop
        nop
        nop
        nop
        nop
        nop
        nop
        nop
        nop
        nop
        nop
        nop
        nop
        nop
        nop
        nop
        nop
        nop
        nop
        nop
        nop
        nop
        nop
        nop
        nop
        nop
        nop
        nop
        nop
        nop
        nop
        nop
        nop
        nop
        nop
        nop
        nop
        nop
        nop
        nop
        nop
        retx    #0
";

#[test]
fn a_closure_fold_program_agrees_across_mechanisms() {
    let results: Vec<_> = MECHS
        .iter()
        .map(|&m| run(&build(CLOSURE_FOLD, m), &[4]))
        .collect();
    for (m, r) in MECHS.iter().zip(&results[1..]) {
        assert_eq!(
            (&results[0].0, &results[0].1),
            (&r.0, &r.1),
            "mono vs {m} diverged on a closure-fold program"
        );
    }
}
```

Add `CLOSURE_FOLD` to `mode_equivalence.rs`'s relink byte-identity list too (copy the const in, as Task 16 Step 3 does for the others — each integration test is its own crate).

Run: `CARGO_TARGET_DIR=/Users/mellonis/Developer/mellonis-workspace/machines/toolchains/target cargo test -p mtc-turing-machine --test link_matrix --test mode_equivalence`
Expected: PASS.

- [ ] **Step 7: Run every gate**

Run: `CARGO_TARGET_DIR=/Users/mellonis/Developer/mellonis-workspace/machines/toolchains/target cargo test -p mtc-core && CARGO_TARGET_DIR=/Users/mellonis/Developer/mellonis-workspace/machines/toolchains/target cargo test -p mtc-turing-machine && CARGO_TARGET_DIR=/Users/mellonis/Developer/mellonis-workspace/machines/toolchains/target cargo test -p mtc-post-machine`
Expected: PASS. No existing program has an exit-bearing site, so the probe finds nothing and every existing image is byte-identical — if `mode_equivalence` moves, the probe is descending differently from the builder, which is the one invariant this task rests on.

- [ ] **Step 8: Commit**

```bash
git add crates/core/src/linker/engine.rs crates/core/src/linker/stamp.rs crates/core/tests/link_exits.rs crates/turing-machine/tests/link_matrix.rs crates/turing-machine/tests/mode_equivalence.rs
git commit -m "feat(core): hybrid folds exit-bearing sites met inside stamped copies"
```

Then `git log -1 --format=%B`; amend if a `Claude-Session:` line was appended.

---

### Task 9: Retire `refuse_symbolic_binding`

Every form it guarded now resolves. This task deletes the function, its `#[allow(dead_code)]`, and its doc comment, and re-pins the one test that was written against it.

**The reachability test is the subtle part.** `the_refusal_is_gated_on_reachability` (`crates/core/tests/link_interface.rs:226-264`) uses `SYMBOLIC = "[num: 1{3=>'0',*}, ctl: 0] exits=(G)"` — all four forms at once — and asserts the reached copy is refused. With the forms resolving, its reached half would still fail: the fixture's `sub` has parameters `p`/`q`, so `num`/`ctl` are unknown parameters. That would keep the test green **by accident**, testing nothing about reachability. Re-pin it against a deliberate resolution error instead.

**Files:**
- Modify: `crates/core/src/linker/engine.rs` (delete `refuse_symbolic_binding`)
- Modify: `crates/core/tests/link_interface.rs:226-264` (re-pin the reachability test)
- Modify: `crates/core/src/linker/interface.rs` (doc comment: absorb the retired guard's reasoning)

**Interfaces:**
- Consumes: Tasks 3–8.
- Produces: nothing new; `crates/core/src/linker/engine.rs` no longer mentions symbolic forms.

- [ ] **Step 1: Re-pin the reachability test**

Replace `the_refusal_is_gated_on_reachability` with:

```rust
/// Resolution is reachability-gated like every other link error: an
/// unreachable function may carry anything, an unresolvable binding
/// included (docs/core.md (linking)). The two programs below differ ONLY
/// in which function holds the bad call — `ghost`, which the BFS from
/// `main` never reaches, or `main` itself — so the contrast pins the
/// gating and nothing else. Both carry a REACHED bound call (`main`'s
/// numeric one), so the pre-pass really walks a binding in each.
///
/// The bad call names a parameter `sub` does not declare, which is a
/// RESOLUTION error rather than the retired blanket refusal: had the
/// fixture kept a form that merely used to be refused, this test would
/// go green for the wrong reason once that form resolved.
///
/// Mutation it catches: move the pre-pass ahead of reachability (run it
/// over every object rather than over `order`) and the first link fails.
#[test]
fn resolution_is_gated_on_reachability() {
    let program = |unreached_body: &str, main_call: &str| {
        format!(
            "\
.routine main, tapes=2, alpha=(4, 4)
.param a, ('_', 'x', 'y', 'z')
.param b, ('_', 'x', 'y', 'z')
.routine sub, tapes=2, alpha=(4, 4)
.param p, ('_', '0', '1', '2')
.param q, ('_', '0', '1', '2')
.routine ghost, tapes=2, alpha=(4, 4)
.param g, ('_', '0', '1', '2')
.param h, ('_', '0', '1', '2')
.section code
.func main
        call    sub {main_call}
M:      stp
.func sub
        ret
.func ghost
        call    sub {unreached_body}
G:      ret
"
        )
    };
    const BAD: &str = "[nosuch: 1, q: 0]";
    let unreached = program(BAD, "[1, 0]");
    let reached = program("[1, 0]", BAD);
    for mech in MECHS {
        let out = link(&fake_syntax(), &[asm(&unreached)], &[], opts(mech))
            .unwrap_or_else(|e| panic!("an unreached binding must not refuse under {mech}: {e}"));
        assert!(
            out.report.dropped.contains(&"ghost".to_string()),
            "`ghost` must be the unreached one under {mech}: {:?}",
            out.report.dropped
        );
        // The very same call, moved into the reached `main`, is refused.
        let err = link(&fake_syntax(), &[asm(&reached)], &[], opts(mech))
            .expect_err("the same binding in a reached function must be refused");
        assert!(
            matches!(&err, LinkError::BadBinding { message, .. }
                if message.contains("parameter `nosuch`")),
            "under {mech}: {err:?}"
        );
    }
}
```

- [ ] **Step 2: Run it to verify it passes with the guard still present**

Run: `CARGO_TARGET_DIR=/Users/mellonis/Developer/mellonis-workspace/machines/toolchains/target cargo test -p mtc-core --test link_interface resolution_is_gated`
Expected: PASS. (The guard is already unreachable; this confirms the new pin holds before the deletion.)

- [ ] **Step 3: Delete the guard**

In `crates/core/src/linker/engine.rs`, delete `fn refuse_symbolic_binding` (`:641-671`) entirely, together with its `#[allow(dead_code)]` and its whole doc comment. Check that `lower`'s doc comment no longer claims a refusal runs first.

- [ ] **Step 4: Move the retired reasoning into the pre-pass**

Append to `crates/core/src/linker/interface.rs`'s module doc:

```rust
//! This module is also where a symbolic form's resolution can FAIL, and
//! it is the only place it can: a parameter the callee does not declare,
//! a glyph outside its alphabet, an open binding into a tape it does not
//! declare opaque, an exit vector whose length disagrees with the
//! declared exit count. Every one of them is refused before the
//! composition engine reads a binding, so no mechanism can mis-lower one
//! — the single gate all three pass through.
```

- [ ] **Step 5: Run everything**

Run: `CARGO_TARGET_DIR=/Users/mellonis/Developer/mellonis-workspace/machines/toolchains/target cargo test -p mtc-core && CARGO_TARGET_DIR=/Users/mellonis/Developer/mellonis-workspace/machines/toolchains/target cargo clippy -p mtc-core --all-targets -- -D warnings`
Expected: PASS, no dead-code warning.

- [ ] **Step 6: Commit**

```bash
git add crates/core/src/linker/engine.rs crates/core/src/linker/interface.rs crates/core/tests/link_interface.rs
git commit -m "refactor(core): retire the symbolic-binding refusal now that every form resolves"
```

Then `git log -1 --format=%B`; amend if a `Claude-Session:` line was appended.

---

### Task 10: Link diagnostics — the core half

The link report has only structural fields today and prints under `-v`. Link WARNINGS need somewhere to live before Task 12 can raise any.

**Files:**
- Modify: `crates/core/src/linker/mod.rs` (`DIAGNOSTIC_CODES`, `LinkReport.diagnostics`)
- Modify: `crates/core/src/linker/engine.rs` (`diag_at` — the site→diagnostic helper)
- Modify: `crates/core/tests/error_code_docs.rs`
- Modify: `docs/core.md`

**Interfaces:**
- Consumes: `LinkDiagnostic` (Task 2).
- Produces:
  - `pub const DIAGNOSTIC_CODES: &[(&str, &str)]` in `crates/core/src/linker` — `(code, one-line meaning)`, the registry the docs set-compare reads and the TM crate's allow namespace joins.
  - `LinkReport.diagnostics: Vec<LinkDiagnostic>`, in site order (function order, then blob offset).
  - `engine::diag_at(order: &[FuncRef], fi: usize, offset: u32, code: &'static str, message: String) -> LinkDiagnostic` — fills `function`, `offset` and `line` (the largest `BlobDebug.lines` offset at or below `offset`).

- [ ] **Step 1: Write the failing test**

Create the code-registry guard by appending to `crates/core/tests/error_code_docs.rs` (core's own, which already parses `docs/core.md` by heading and reads a two-column table — reuse its `doc`, `section` and `table_rows` helpers verbatim):

```rust
/// The published link-warning catalog lists exactly the registry's codes.
/// Mutation it catches: add a code to `DIAGNOSTIC_CODES` without a docs
/// row (or the reverse) and the set-compare fails — which is the intent,
/// not friction.
#[test]
fn the_published_link_warning_catalog_lists_exactly_the_registry_codes() {
    let doc = doc();
    let mut published: Vec<String> = table_rows(&section(&doc, "### Link warnings"))
        .into_iter()
        .map(|(code, _)| code)
        .collect();
    published.sort();
    let mut registry: Vec<String> = mtc_core::linker::DIAGNOSTIC_CODES
        .iter()
        .map(|(c, _)| (*c).to_string())
        .collect();
    registry.sort();
    assert_eq!(published, registry, "docs/core.md (### Link warnings)");
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `CARGO_TARGET_DIR=/Users/mellonis/Developer/mellonis-workspace/machines/toolchains/target cargo test -p mtc-core --test error_code_docs`
Expected: FAIL — `DIAGNOSTIC_CODES` does not exist (compile error).

- [ ] **Step 3: Add the registry and the report field**

In `crates/core/src/linker/mod.rs`, after the `LinkDiagnostic` definition:

```rust
/// Every link-time warning code, with its one-line meaning
/// (docs/core.md (link warnings)). The published catalog is
/// set-compared against this table in both directions, and a
/// consumer's allow namespace joins it here rather than maintaining a
/// copy.
pub const DIAGNOSTIC_CODES: &[(&str, &str)] = &[
    (
        "glyph-mismatch",
        "A call site binds by index into a callee whose alphabet is the same size \
         but spells different glyphs, so the callee reads the caller's symbols as \
         other symbols.",
    ),
    (
        "narrow-alphabet",
        "A call site binds by index into a callee whose alphabet is narrower, so \
         the caller's high symbols have no image in it.",
    ),
];
```

and in `LinkReport`, after `folds`:

```rust
    /// Link-time WARNINGS, in site order (function order, then blob
    /// offset): findings that do not stop the link
    /// (docs/core.md (link warnings)). A consumer suppresses them
    /// through its own allow namespace and promotes them with its own
    /// `-Werror`; the linker itself never prints and never decides.
    pub diagnostics: Vec<LinkDiagnostic>,
```

fed from the `Lowered.diagnostics` Task 2 destructured as `_diagnostics` — rename it to `diagnostics` there and add `diagnostics,` to the `LinkReport { … }` literal.

- [ ] **Step 4: Add the site→diagnostic helper**

In `crates/core/src/linker/engine.rs`, next to `bad_binding`:

```rust
/// Build a link diagnostic for a site inside function `fi`. The source
/// line is the largest `-g` line-table offset at or below the site — the
/// same "innermost preceding line" rule the map sidecar uses
/// (docs/formats.md (map sidecar)); `None` without debug data.
pub(super) fn diag_at(
    order: &[FuncRef],
    fi: usize,
    offset: u32,
    code: &'static str,
    message: String,
) -> super::LinkDiagnostic {
    let line = order[fi].debug.as_ref().and_then(|d| {
        d.lines
            .iter()
            .filter(|(off, _)| *off <= offset)
            .max_by_key(|(off, _)| *off)
            .map(|&(_, line)| line)
    });
    super::LinkDiagnostic {
        code,
        message,
        function: order[fi].name.to_string(),
        offset,
        line,
    }
}
```

- [ ] **Step 5: Add the docs section**

In `docs/core.md`, after `### The link report` (`:752`), add the section — **the heading is `### Link warnings`, byte for byte**, because `section()` in `crates/core/tests/error_code_docs.rs` matches a line equal to its argument after `trim_end`, and Step 1's test passes exactly that string. Do not spell it "Link diagnostics" anywhere it is used as a heading; the prose may call them diagnostics, the heading may not.

```markdown
### Link warnings

A link error stops the link; a link **warning** does not. The report
carries them in `diagnostics`, one per site, each with a stable
kebab-case **code**, the finding, the function and blob offset it was
raised at, and the source line when the objects carried debug data. The
linker never prints and never decides what a warning means: a consumer
renders them, suppresses them through its own allow list, and promotes
them to errors under its own strict-mode flag.

Codes are permanent identifiers: they never change meaning.

| Code | Meaning |
|---|---|
| `glyph-mismatch` | A call site binds by index into a callee whose alphabet is the same size but spells different glyphs, so the callee reads the caller's symbols as other symbols. |
| `narrow-alphabet` | A call site binds by index into a callee whose alphabet is narrower, so the caller's high symbols have no image in it. |

Errors are outside this catalog and cannot be suppressed.
```

- [ ] **Step 6: Run and commit**

Run: `CARGO_TARGET_DIR=/Users/mellonis/Developer/mellonis-workspace/machines/toolchains/target cargo test -p mtc-core && CARGO_TARGET_DIR=/Users/mellonis/Developer/mellonis-workspace/machines/toolchains/target cargo test -p mtc-post-machine`
Expected: PASS — PM reads `LinkReport` by reference only (`crates/post-machine/src/cli/build.rs:66`), so a new field compiles without a PM change and renders as nothing.

```bash
git add crates/core/src/linker/mod.rs crates/core/src/linker/engine.rs crates/core/tests/error_code_docs.rs docs/core.md
git commit -m "feat(core): the link report carries diagnostics, and a code registry the docs mirror"
```

Then `git log -1 --format=%B`; amend if a `Claude-Session:` line was appended.

---

### Task 11: `tmt link` and `tmt build` surface link warnings

The spec rules that warnings **print always**, in the compile-warning format — not only under `-v`. A compile warning renders as `{path}:{line}:{col}: warning: {message}` (`crates/turing-machine/src/cli/build.rs:42-49`); a link diagnostic has no path and no column, so it gets its own `{where}` with the same tail. Rendering is factored out of the three inlined sites the TM crate carries today.

**Files:**
- Modify: `crates/turing-machine/src/cli/build.rs:256-270` (`LINK_USAGE`), `:286-302` (flag parse), `:341-367` (the `-v` block)
- Modify: `crates/turing-machine/src/cli/driver.rs:518-528` + `:751-757` (`link_and_write`, `link_and_write_argv`), `:472-486` + `:719-734` (the `-Werror` blocks)
- Modify: `crates/turing-machine/src/lint/mod.rs:178-188` (`known_code`)
- Modify: `crates/turing-machine/src/completions/registry.rs:291-332` (`link_spec`)
- Modify: `docs/tmt/cli.md:277-291` (the quoted usage block) and the `## `tmt link`` section (adding `### Link warnings` as its last subsection)
- Modify: `docs/tmt/lint.md` (the fifth allow surface)
- Modify: `crates/turing-machine/tests/error_code_docs.rs` (the link-warning catalog set-compare)
- Create: `crates/turing-machine/tests/link_warnings.rs`

**Interfaces:**
- Consumes: `mtc_core::linker::{LinkDiagnostic, DIAGNOSTIC_CODES}` and `LinkReport.diagnostics` (Task 10).
- Produces:
  - `crates/turing-machine/src/cli/build.rs`: `pub(super) fn render_link_report(stderr: &mut String, prefix: &str, report: &LinkReport)` (the `-v` structural lines, mirroring PM's at `crates/post-machine/src/cli/build.rs:66`) and `pub(super) fn render_link_diagnostics(stderr: &mut String, report: &LinkReport, allow: &[String]) -> usize`, which prints every non-allowed diagnostic and returns how many it printed.
  - `tmt link` accepts `--allow CODE` (repeatable) and `-Werror`.
  - `tmt build`'s `-Werror` covers the link stage; manifest mode unions the manifest's `lint.allow` into the link allow list.

**Decision recorded here, not discoverable from the code:** `tmt link` honours `--allow` only, with no manifest walk — it consumes prebuilt objects and has no project context (`crates/turing-machine/src/cli/build.rs:319-331` builds `LinkOptions` with `sources: Vec::new()` for exactly that reason). `tmt build` unions `--allow` with the manifest's already-loaded `TmtFile.allow`. See "Decisions for the controller".

- [ ] **Step 1: Write the failing test**

Create `crates/turing-machine/tests/link_warnings.rs`. Copy `fn args` and `fn scratch` verbatim from `crates/turing-machine/tests/mode_equivalence.rs:894-917`.

```rust
//! Link warnings on the CLI: printed always, suppressed by `--allow`,
//! promoted by `-Werror` (docs/tmt/cli.md (link warnings)).

use mtc_turing_machine::cli::execute;

// … args / scratch copied from mode_equivalence.rs …

/// A caller whose 5-symbol band plainly calls a 3-symbol callee: the
/// `narrow-alphabet` warning's minimal shape.
const NARROW: &str = "\
.routine main, tapes=1, alpha=(5)
.routine sub, tapes=1, alpha=(3)
.section code
.func main
        call    sub
        stp
.func sub
        ret
";

fn build_object(dir: &std::path::Path) -> std::path::PathBuf {
    let src = dir.join("narrow.tma");
    std::fs::write(&src, NARROW).unwrap();
    let obj = dir.join("narrow.tmo");
    execute(&args(&["asm", src.to_str().unwrap(), "-o", obj.to_str().unwrap()]))
        .expect("assembles");
    obj
}

/// Mutation it catches: render diagnostics only under `-v` and this
/// assertion fails, because no `-v` is passed.
#[test]
fn a_link_warning_prints_without_v() {
    let dir = scratch("link_warnings_plain");
    let obj = build_object(&dir);
    let out = execute(&args(&[
        "link",
        obj.to_str().unwrap(),
        "--nostdlib",
        "-o",
        dir.join("a.tmx").to_str().unwrap(),
    ]))
    .expect("links");
    assert_eq!(out.code, 0, "a warning does not fail the link");
    assert!(
        out.stderr.contains("warning:") && out.stderr.contains("[narrow-alphabet]"),
        "{}",
        out.stderr
    );
}

/// Mutation it catches: ignore the allow list and the warning still
/// prints.
#[test]
fn allow_suppresses_a_link_warning() {
    let dir = scratch("link_warnings_allow");
    let obj = build_object(&dir);
    let out = execute(&args(&[
        "link",
        obj.to_str().unwrap(),
        "--nostdlib",
        "--allow",
        "narrow-alphabet",
        "-o",
        dir.join("a.tmx").to_str().unwrap(),
    ]))
    .expect("links");
    assert_eq!(out.code, 0);
    assert!(!out.stderr.contains("narrow-alphabet"), "{}", out.stderr);
}

/// Mutation it catches: leave `-Werror` off the link stage and this link
/// succeeds.
#[test]
fn werror_promotes_a_link_warning() {
    let dir = scratch("link_warnings_werror");
    let obj = build_object(&dir);
    let err = execute(&args(&[
        "link",
        obj.to_str().unwrap(),
        "--nostdlib",
        "-Werror",
        "-o",
        dir.join("a.tmx").to_str().unwrap(),
    ]));
    assert!(err.is_err(), "-Werror must fail the link: {err:?}");
}

/// An unknown code is a typo, caught up front like a lint `--allow`.
/// Mutation it catches: skip validation and a typo'd `--allow` silently
/// suppresses nothing.
#[test]
fn an_unknown_allow_code_is_rejected() {
    let dir = scratch("link_warnings_typo");
    let obj = build_object(&dir);
    let err = execute(&args(&[
        "link",
        obj.to_str().unwrap(),
        "--nostdlib",
        "--allow",
        "narow-alphabet",
        "-o",
        dir.join("a.tmx").to_str().unwrap(),
    ]));
    assert!(err.is_err(), "a typo'd code must be rejected: {err:?}");
}
```

**Note for the implementer:** these four tests depend on Task 12 actually raising `narrow-alphabet`, and **Task 11 runs before Task 12** — the plan executes in order. So write them, mark each `#[ignore = "raised by the plain-site check task"]` with that exact reason string, and remove the four attributes as the first step of Task 12. Do not reorder the tasks.

- [ ] **Step 2: Add the flags**

In `crates/turing-machine/src/cli/build.rs`, `LINK_USAGE` gains two lines after `--nostdlib`:

```
  --allow CODE      suppress a link warning code (repeatable)
  -Werror           treat link warnings as errors
```

and the parse (after `let nostdlib = args.flag("--nostdlib");`):

```rust
    let allow = args.values("--allow")?;
    crate::lint::validate_allow(&allow).map_err(|e| e.to_string())?;
    let werror = args.flag("-Werror");
```

- [ ] **Step 3: Factor the rendering and wire it**

Replace the `-v` block (`:341-367`) with the following, and place it **BEFORE the code that writes `OUT.tmx` and its `.map` sidecar**. A promoted warning is an error, and an error writes nothing — leaving a half-finished artifact on disk after a failed strict build is the one behaviour a `-Werror` user cannot want. Move the write below this block if it currently sits above it.

```rust
    let mut stderr = String::new();
    // A link warning prints always, in the compile-warning format — the
    // report's structural lines stay behind `-v`
    // (docs/tmt/cli.md (link warnings)).
    let warned = render_link_diagnostics(&mut stderr, &linked.report, &allow);
    if verbose {
        render_link_report(&mut stderr, "", &linked.report);
    }
    if werror && warned > 0 {
        return Err(werror_message(&stderr, warned));
    }
```

with the message factored so `link` and both `build` modes cannot drift:

```rust
/// The one spelling of the strict-mode refusal, shared by `tmt link` and
/// both of `tmt build`'s modes (docs/tmt/cli.md (link warnings)).
pub(super) fn werror_message(stderr: &str, warned: usize) -> String {
    format!("{stderr}-Werror: {warned} link warning(s) treated as errors")
}
```

and add the two renderers near `render_warnings` (`:40`):

```rust
/// Every non-allowed link warning, in the compile-warning format
/// (docs/tmt/cli.md (link warnings)). A link diagnostic has no path and
/// no column, so its location is the function and blob offset, or the
/// source line when the objects carried debug data. Returns how many
/// were printed, which is what `-Werror` counts.
pub(super) fn render_link_diagnostics(
    stderr: &mut String,
    report: &LinkReport,
    allow: &[String],
) -> usize {
    let mut n = 0;
    for d in &report.diagnostics {
        if allow.iter().any(|a| a == d.code) {
            continue;
        }
        n += 1;
        match d.line {
            Some(line) => {
                let _ = writeln!(
                    stderr,
                    "{}:{line}: warning: {} [{}]",
                    d.function, d.message, d.code
                );
            }
            None => {
                let _ = writeln!(
                    stderr,
                    "{}+0x{:04x}: warning: {} [{}]",
                    d.function, d.offset, d.message, d.code
                );
            }
        }
    }
    n
}

/// The link report's structural lines, `-v` only. Mirrors PM's renderer
/// of the same name so the two CLIs do not drift
/// (docs/core.md (the link report)).
pub(super) fn render_link_report(stderr: &mut String, prefix: &str, report: &LinkReport) {
    let _ = writeln!(
        stderr,
        "{prefix}link: dropped [{}]; {} site(s) relaxed short, {} far",
        report.dropped.join(", "),
        report.relaxed_calls,
        report.far_calls
    );
    if report.composites > 0 || report.instantiations > 0 {
        let _ = writeln!(
            stderr,
            "{prefix}frames: {} composite(s), {} stamp(s), {} B compose table; \
             {} deduped, {} trap row(s), {} expanded row(s)",
            report.composites,
            report.instantiations,
            report.compose_table_bytes,
            report.dedup_savings,
            report.synthesized_trap_rows,
            report.expanded_rows
        );
    }
    for fold in &report.folds {
        let _ = writeln!(
            stderr,
            "{prefix}fold: `{}` {} site(s), body {} B, descriptors {} B — {}",
            fold.routine,
            fold.sites,
            fold.body_bytes,
            fold.descriptor_bytes,
            if fold.shared { "shared" } else { "spliced" }
        );
    }
}
```

Import `LinkReport` in `crates/turing-machine/src/cli/build.rs`.

- [ ] **Step 4: Cover the two `build` paths**

In `crates/turing-machine/src/cli/driver.rs`, `link_and_write` and `link_and_write_argv` both return `Result<String, String>` (the `-v` chunk). Change both to return `Result<(String, usize), String>` — the rendered text and the warning count — calling `render_link_diagnostics` before `render_link_report`.

**Both functions currently link AND write in one body.** Split the write off so the promotion happens first: render the diagnostics, and if `flags.werror` and the count is non-zero, return `werror_message(...)` **before** the executable and its sidecar are written. A strict build that fails must leave no artifact, exactly as a link error does today.

Then at both call sites (`:495-505` and `:740-742`), after `stderr.push_str(&tail)`, add:

```rust
    if flags.werror && link_warnings > 0 {
        return Err(crate::cli::build::werror_message(&stderr, link_warnings));
    }
```

(The inner refusal already covers the write ordering; this outer one carries the accumulated `stderr` — the compile warnings plus the link ones — into the message the user sees.)

In manifest mode, the allow list is `flags.allow` unioned with the manifest's own `lint.allow` — `crate::project::load_file` already parsed and validated it into `TmtFile.allow`, so read it from the loaded manifest rather than adding a second discovery walk. In argv mode there is no manifest, so the list is `flags.allow` alone. Add `allow: Vec<String>` to `struct Flags` (`:60-81`) parsed as `allow: args.values("--allow")?` with `crate::lint::validate_allow(&flags.allow)` immediately after, and the `--allow CODE` line in `BUILD_USAGE`.

- [ ] **Step 5: Join the allow namespace**

In `crates/turing-machine/src/lint/mod.rs`, `known_code` (`:178-188`) gains a fifth arm:

```rust
pub(crate) fn known_code(code: &str) -> bool {
    RULES.iter().any(|(c, _)| *c == code)
        || OPT_IN_RULES.iter().any(|(c, _)| *c == code)
        || tma::TMA_RULES.iter().any(|(c, _)| *c == code)
        || mtc_core::asm::lint::RULES.iter().any(|(c, _)| *c == code)
        // The fifth surface: link warnings share the one allow namespace,
        // so `lint.allow` in `tmt.json` and `--allow` on `link`/`build`
        // suppress them with no new key (docs/tmt/lint.md (the allow
        // namespace)).
        || mtc_core::linker::DIAGNOSTIC_CODES.iter().any(|(c, _)| *c == code)
}
```

Update its doc comment's list of surfaces from four to five.

- [ ] **Step 6: Register the flags and update the docs**

In `crates/turing-machine/src/completions/registry.rs`, `link_spec()` gains, after the `--nostdlib` entry:

```rust
            FlagSpec::value("--allow", "suppress a link warning code (repeatable)", ValueHint::Text)
                .repeatable(),
            FlagSpec::boolean("-Werror", "treat link warnings as errors"),
```

In `docs/tmt/cli.md`, update the fenced usage block at `:277-291` to the new `LINK_USAGE` **byte for byte** (`crates/turing-machine/tests/cli_docs.rs:68-83` asserts it), and add under the `## `tmt link`` section:

```markdown
### Link warnings

A link warning names a site the linker can see is suspect but will not
refuse — a callee whose alphabet is narrower than the caller's band, or
one whose glyphs differ at the same width. It prints always, in the same
format a compile warning does, and carries a bracketed code:

```
main+0x0000: warning: `sub` reads a 3-symbol alphabet where `main`'s band is 5 wide [narrow-alphabet]
```

The codes share the one allow namespace `tmt lint` uses, so `--allow CODE`
suppresses one here and `lint.allow` in `tmt.json` suppresses it for
`tmt build`. `-Werror` promotes every unsuppressed warning to an error.
Errors — a callee wider than the caller, a graft whose digest drifted —
are outside the namespace and cannot be suppressed.

| Code | Meaning |
|---|---|
| `glyph-mismatch` | A call site binds by index into a callee whose alphabet is the same size but spells different glyphs, so the callee reads the caller's symbols as other symbols. |
| `narrow-alphabet` | A call site binds by index into a callee whose alphabet is narrower, so the caller's high symbols have no image in it. |
```

**Placement matters, and so does the guard.** Before writing the section, read `crates/turing-machine/tests/error_code_docs.rs` end to end: `section()` takes the lines after a heading that equals its argument (after `trim_end`) up to the next line starting with `#`, and `table_codes()` keeps only lines beginning with `` | ` `` and reads the first cell. So put `### Link warnings` as the LAST subsection of `## tmt link`, ending at `## tmt build` — anywhere earlier and it would swallow `### --call-mech`, whose `| \`mono\` |` rows would then read as codes. The fenced example above is transparent to the parser (its lines do not start with `` | ` ``).

Then extend the guard, per the spec's "the codes enter the same registry-versus-docs set-compare as error codes" — **in both directions**, so a code in one place and not the other is red either way:

```rust
/// The published link-warning catalog on the CLI page lists exactly the
/// linker's registry — the same two-way set-compare the compile-error
/// catalog gets.
///
/// Mutation it catches: add a code to `DIAGNOSTIC_CODES` without a row
/// here (or leave a row behind after retiring a code) and the sorted
/// vectors differ.
#[test]
fn the_published_link_warning_catalog_lists_exactly_the_registry_codes() {
    let doc = doc();
    let mut published = table_codes(&section(&doc, "### Link warnings"));
    published.sort();
    assert!(!published.is_empty(), "no `### Link warnings` table in docs/tmt/cli.md");
    let mut registry: Vec<String> = mtc_core::linker::DIAGNOSTIC_CODES
        .iter()
        .map(|(c, _)| (*c).to_string())
        .collect();
    registry.sort();
    assert_eq!(published, registry, "docs/tmt/cli.md (### Link warnings)");
}
```

The `assert!(!published.is_empty(), …)` is load-bearing: without it, a heading typo makes `section()` return nothing and an empty-vs-empty comparison would pass while the table went unguarded. Add `mtc_core` as a dev-dependency of the TM crate if the test file does not already reach it — check `crates/turing-machine/Cargo.toml` first; the crate already depends on `mtc-core` normally, so the test can use it as-is.

In `docs/tmt/lint.md`, in the allow-namespace paragraph, extend the list of surfaces to name link warnings as the fifth and point at `docs/tmt/cli.md (link warnings)`.

- [ ] **Step 7: Run the guards**

Run: `CARGO_TARGET_DIR=/Users/mellonis/Developer/mellonis-workspace/machines/toolchains/target cargo test -p mtc-turing-machine --test cli_docs --test completions_registry --test man_page --test error_code_docs --test link_warnings`
Expected: PASS. If `cli_docs` fails, the usage block and `LINK_USAGE` differ by a byte — fix the doc, not the test.

Run: `CARGO_TARGET_DIR=/Users/mellonis/Developer/mellonis-workspace/machines/toolchains/target cargo test -p mtc-post-machine`
Expected: PASS — `pmt` is untouched.

- [ ] **Step 8: Commit**

```bash
git add crates/turing-machine/src/cli/build.rs crates/turing-machine/src/cli/driver.rs crates/turing-machine/src/lint/mod.rs crates/turing-machine/src/completions/registry.rs crates/turing-machine/tests/link_warnings.rs crates/turing-machine/tests/error_code_docs.rs docs/tmt/cli.md docs/tmt/lint.md
git commit -m "feat(turing-machine): tmt link and tmt build surface link warnings, with --allow and -Werror"
```

Then `git log -1 --format=%B`; amend if a `Claude-Session:` line was appended.

---

### Task 12: Plain-site checks and the omitted-map warning (item 4, P6)

**DO NOT START until Task 1's stop gate is cleared.** If Task 1 recorded any `ERROR-WOULD-FIRE` line, the controller must rule first.

Today a `SiteKind::Plain` call is checked for nothing — not arity, not cardinality (`crates/core/src/linker/engine.rs:583-588`). This task grades it:

| Relation of callee to caller | Verdict |
|---|---|
| wider in tape count | **error** — it would address a band that does not exist |
| wider in alphabet on a shared tape | **error** — it would write an index the tape cannot hold |
| narrower in tape count | silent — a transparent call on the first `k` bands is how it is meant to work |
| narrower in alphabet at equal tape count | warning `narrow-alphabet` |
| same size, both sides carry interfaces, glyphs differ | warning `glyph-mismatch`, naming both alphabets and the first differing position |

The same grading runs on a **bound** site's tape whose map is omitted (`!tb.map_written`). An explicit map — the empty `with map { }` included — is the author's statement that the re-labelling is meant, and silences the glyph comparison for that binding (P6).

**The check runs ONCE.** `lower` computes `sites` a single time (`crates/core/src/linker/engine.rs:164-167`) before dispatching; hybrid's second `scan_sites` (`crates/core/src/linker/stamp.rs:351`) is for the stamps and must not re-raise anything. Putting the check in `lower` next to that one scan is what keeps a hybrid link from double-reporting.

**Two scope facts, both deliberate, both disclosed rather than discovered later.**

- **`SiteKind::Plain` covers more than calls.** `scan_sites` pushes it for a relocated plain call and for a relocated tail jump or conditional branch into another function (`crates/core/src/linker/engine.rs:595-609`). Grading those is intended, not an accident of the match arm: the tail-call pass turns calls into jumps, so restricting the rule to calls would let the same hazard through whenever the optimizer had run. Task 1's sweep covers them too, because it walks `obj.relocations`.
- **An unsigned entry is graded not at all.** `link()` calls `engine::lower` only when the entry function has a signature (`crates/core/src/linker/mod.rs:417-442`); without one there is no machine signature to grade against and the whole engine is skipped. So a hand-assembled `.tma` with no `.routine` lines gets no plain-site check — the same degradation the spec describes for a callee that carries no interface, one level up. This is also exactly why PM-1 is untouched (Step 5).

**Files:**
- Modify: `crates/core/src/linker/engine.rs:152-185` (`lower` — call the new pass), plus `check_sites`
- Modify: `crates/core/src/linker/mod.rs` (`LinkError::CalleeWider`)
- Modify: `crates/core/tests/link_interface.rs` (any fixture the new error catches)
- Create: `crates/core/tests/link_checks.rs`
- Modify: `crates/turing-machine/tests/link_warnings.rs` (remove the `#[ignore]`s)

**Interfaces:**
- Consumes: `diag_at` (Task 10), `FuncRef.interface` (Task 2), `Lowered.diagnostics` (Task 2).
- Produces:
  - `LinkError::CalleeWider { callee: String, caller: String, offset: u32, what: String }` — `what` is `"tape count"` or `"the alphabet of tape N"`.
  - `engine::check_sites(order: &[FuncRef], sites: &[Vec<SiteKind>], machine_sig: &RoutineSig) -> Result<Vec<LinkDiagnostic>, LinkError>`.

- [ ] **Step 1: Write the failing tests**

Create `crates/core/tests/link_checks.rs` with the `link_interface.rs` dialect helpers copied in, plus:

```rust
//! What the linker checks at a call site that binds by INDEX: a plain
//! call, and a bound call whose map is omitted
//! (docs/core.md (link warnings)). An explicit map — `{}` included —
//! is the author's statement and silences the glyph comparison.

// … fake_syntax / asm / MECHS / opts copied from link_interface.rs …

/// A 5-symbol caller plainly calling a 3-symbol callee.
const NARROW: &str = "\
.routine main, tapes=1, alpha=(5)
.routine sub, tapes=1, alpha=(3)
.section code
.func main
        call    sub
        stp
.func sub
        ret
";

/// A 3-symbol caller plainly calling a 5-symbol callee.
const WIDER_ALPHABET: &str = "\
.routine main, tapes=1, alpha=(3)
.routine sub, tapes=1, alpha=(5)
.section code
.func main
        call    sub
        stp
.func sub
        ret
";

/// A one-tape caller plainly calling a two-tape callee.
const WIDER_TAPES: &str = "\
.routine main, tapes=1, alpha=(3)
.routine sub, tapes=2, alpha=(3, 3)
.section code
.func main
        call    sub
        stp
.func sub
        ret
";

/// A two-tape caller plainly calling a one-tape callee: narrower in tape
/// count, which is exactly how a transparent call is meant to work.
const NARROWER_TAPES: &str = "\
.routine main, tapes=2, alpha=(3, 3)
.routine sub, tapes=1, alpha=(3)
.section code
.func main
        call    sub
        stp
.func sub
        ret
";

/// Equal widths, DIFFERENT glyphs, both sides declaring an interface.
const REORDERED: &str = "\
.routine main, tapes=1, alpha=(3)
.param t, ('_', '0', '1')
.routine sub, tapes=1, alpha=(3)
.param n, ('_', '1', '0')
.section code
.func main
        call    sub
        stp
.func sub
        ret
";

/// Mutation it catches: skip the narrower-alphabet arm and this program
/// links with no diagnostic at all.
#[test]
fn a_narrower_callee_alphabet_warns() {
    let out = link(&fake_syntax(), &[asm(NARROW)], &[], opts(CallMech::Frames))
        .expect("a warning does not stop the link");
    assert_eq!(out.report.diagnostics.len(), 1, "{:?}", out.report.diagnostics);
    assert_eq!(out.report.diagnostics[0].code, "narrow-alphabet");
    assert_eq!(out.report.diagnostics[0].function, "main");
}

/// Mutation it catches: grade a wider alphabet as a warning and this
/// link succeeds, letting the callee write an index the band cannot
/// hold.
#[test]
fn a_wider_callee_alphabet_is_an_error() {
    let err = link(&fake_syntax(), &[asm(WIDER_ALPHABET)], &[], opts(CallMech::Frames))
        .expect_err("a wider callee alphabet must stop the link");
    assert!(
        matches!(&err, LinkError::CalleeWider { callee, what, .. }
            if callee == "sub" && what.contains("alphabet")),
        "{err:?}"
    );
}

/// Mutation it catches: compare only the alphabets and a callee that
/// addresses a band the caller does not have links silently.
#[test]
fn a_wider_callee_tape_count_is_an_error() {
    let err = link(&fake_syntax(), &[asm(WIDER_TAPES)], &[], opts(CallMech::Frames))
        .expect_err("a wider callee arity must stop the link");
    assert!(
        matches!(&err, LinkError::CalleeWider { what, .. } if what == "tape count"),
        "{err:?}"
    );
}

/// Mutation it catches: treat ANY arity difference as an error and every
/// transparent call on the first k bands breaks.
#[test]
fn a_narrower_callee_tape_count_is_silent() {
    let out = link(&fake_syntax(), &[asm(NARROWER_TAPES)], &[], opts(CallMech::Frames))
        .expect("a narrower callee arity is how it is meant to work");
    assert!(out.report.diagnostics.is_empty(), "{:?}", out.report.diagnostics);
}

/// Mutation it catches: compare the glyph LISTS by length instead of
/// element-wise and a reordered alphabet — the item-4 hazard, silently
/// wrong output — goes unreported.
#[test]
fn differing_glyphs_at_equal_width_warn_and_name_the_first_difference() {
    let out = link(&fake_syntax(), &[asm(REORDERED)], &[], opts(CallMech::Frames))
        .expect("a warning does not stop the link");
    let d = out
        .report
        .diagnostics
        .iter()
        .find(|d| d.code == "glyph-mismatch")
        .unwrap_or_else(|| panic!("{:?}", out.report.diagnostics));
    assert!(d.message.contains("position 1"), "{}", d.message);
}

/// A hybrid link classifies twice but must report once. Mutation it
/// catches: move the check into `scan_sites` and a hybrid image gets two
/// copies of every diagnostic.
#[test]
fn a_diagnostic_is_reported_once_under_every_mechanism() {
    for mech in MECHS {
        let out = link(&fake_syntax(), &[asm(NARROW)], &[], opts(mech))
            .unwrap_or_else(|e| panic!("under {mech}: {e}"));
        assert_eq!(
            out.report.diagnostics.len(),
            1,
            "under {mech}: {:?}",
            out.report.diagnostics
        );
    }
}
```

Add the omitted-map half, using the `program`/`open_program` shape of `link_interface.rs`:

```rust
/// A bound site whose ONE tape omits its map into a callee with the same
/// width but different glyphs. Mutation it catches: restrict the glyph
/// check to plain sites and the omitted-map case — item 4's other half —
/// goes unreported.
#[test]
fn an_omitted_map_on_a_bound_site_warns_like_a_plain_one() {
    const OMITTED: &str = "\
.routine main, tapes=2, alpha=(3, 3)
.param a, ('_', '0', '1')
.param b, ('_', '0', '1')
.routine sub, tapes=2, alpha=(3, 3)
.param p, ('_', '1', '0')
.param q, ('_', '0', '1')
.section code
.func main
        call    sub [1, 0]
        stp
.func sub
        ret
";
    let out = link(&fake_syntax(), &[asm(OMITTED)], &[], opts(CallMech::Frames))
        .expect("links");
    assert!(
        out.report.diagnostics.iter().any(|d| d.code == "glyph-mismatch"),
        "{:?}",
        out.report.diagnostics
    );
}

/// The empty map `{}` is "bind by index, on purpose" and silences the
/// comparison for that binding. Mutation it catches: ignore
/// `map_written` and `{}` warns exactly like the bare form, so the
/// author has no local opt-out.
#[test]
fn a_written_empty_map_silences_the_glyph_comparison() {
    const WRITTEN: &str = "\
.routine main, tapes=2, alpha=(3, 3)
.param a, ('_', '0', '1')
.param b, ('_', '0', '1')
.routine sub, tapes=2, alpha=(3, 3)
.param p, ('_', '1', '0')
.param q, ('_', '0', '1')
.section code
.func main
        call    sub [1{}, 0{}]
        stp
.func sub
        ret
";
    let out = link(&fake_syntax(), &[asm(WRITTEN)], &[], opts(CallMech::Frames))
        .expect("links");
    assert!(
        !out.report.diagnostics.iter().any(|d| d.code == "glyph-mismatch"),
        "`{{}}` must silence the comparison: {:?}",
        out.report.diagnostics
    );
}
```

**Careful with the omitted-map fixtures:** in `[1, 0]` the entries are POSITIONAL, so entry 0 binds callee tape `p` to caller tape 1, and entry 1 binds callee tape `q` to caller tape 0. The glyph comparison is therefore `main`'s tape-1 glyphs against `sub`'s `p` glyphs. Both are 3 wide; `p` is reordered, so the warning fires. Verify with a scratch `println!` of the diagnostic message before trusting the assertion.

- [ ] **Step 2: Run to verify they fail**

Run: `CARGO_TARGET_DIR=/Users/mellonis/Developer/mellonis-workspace/machines/toolchains/target cargo test -p mtc-core --test link_checks`
Expected: FAIL — `LinkError::CalleeWider` does not exist (compile error), and the diagnostics vector is empty.

- [ ] **Step 3: Add the error variant**

In `crates/core/src/linker/mod.rs`, after `OpenBindingUnsupported`:

```rust
    /// A call site binds by index into a callee that is WIDER than the
    /// caller's bands: more tapes than the caller has, or an alphabet a
    /// caller band cannot hold. Either would let the callee touch what
    /// the caller lacks — address a band that does not exist, or write an
    /// index past the band's width — so it is an error, not a warning
    /// (docs/core.md (link warnings)). `what` names the dimension:
    /// `"tape count"`, or `"the alphabet of tape N"`.
    CalleeWider {
        callee: String,
        caller: String,
        offset: u32,
        what: String,
    },
```

with the `Display` arm:

```rust
            Self::CalleeWider {
                callee,
                caller,
                offset,
                what,
            } => write!(
                f,
                "`{callee}`, called from `{caller}`+0x{offset:04x}, is wider than the \
                 caller in {what}; a call binding by index cannot reach what the \
                 caller does not have"
            ),
```

- [ ] **Step 4: Write the check**

In `crates/core/src/linker/engine.rs`, add after `scan_sites`:

```rust
/// Grade every index-binding call site against the callee's declared
/// shape (docs/core.md (link warnings)): a plain call, and each tape
/// of a bound call whose map is OMITTED. An explicit map — the empty
/// `{}` included — is the author's statement that the re-labelling is
/// meant, and is never graded.
///
/// Runs ONCE, from `lower`, over the single site scan every mechanism
/// shares: hybrid's second scan exists to classify stamps, and a check
/// living there would report every finding twice.
pub(super) fn check_sites(
    order: &[FuncRef],
    sites: &[Vec<SiteKind>],
    machine_sig: &RoutineSig,
) -> Result<Vec<super::LinkDiagnostic>, LinkError> {
    let mut out = Vec::new();
    for (fi, func_sites) in sites.iter().enumerate() {
        let caller_sig = order[fi].signature.unwrap_or(machine_sig);
        for site in func_sites {
            match site {
                SiteKind::Plain { addr, callee } => {
                    let Some(callee_sig) = order[*callee].signature else {
                        continue; // nothing declared, nothing to compare
                    };
                    if callee_sig.arity > caller_sig.arity {
                        return Err(wider(order, fi, *callee, *addr, "tape count".to_string()));
                    }
                    for k in 0..usize::from(callee_sig.arity) {
                        grade_tape(
                            order,
                            fi,
                            *callee,
                            *addr,
                            k,
                            k,
                            caller_sig,
                            callee_sig,
                            &mut out,
                        )?;
                    }
                }
                SiteKind::Bound {
                    addr,
                    callee,
                    record,
                    ..
                } => {
                    let Some(callee_sig) = order[*callee].signature else {
                        continue;
                    };
                    for (k, tb) in record.binding.iter().enumerate() {
                        if tb.map_written {
                            continue; // the author said what they meant
                        }
                        grade_tape(
                            order,
                            fi,
                            *callee,
                            *addr,
                            usize::from(tb.caller_tape),
                            k,
                            caller_sig,
                            callee_sig,
                            &mut out,
                        )?;
                    }
                }
                SiteKind::RawCallM { .. } => {}
            }
        }
    }
    Ok(out)
}

/// One tape of one site: caller band `ct` against callee tape `k`.
#[allow(clippy::too_many_arguments)]
fn grade_tape(
    order: &[FuncRef],
    fi: usize,
    callee: usize,
    addr: u32,
    ct: usize,
    k: usize,
    caller_sig: &RoutineSig,
    callee_sig: &RoutineSig,
    out: &mut Vec<super::LinkDiagnostic>,
) -> Result<(), LinkError> {
    let Some(&caller_card) = caller_sig.cardinalities.get(ct) else {
        return Ok(()); // the caller-tape range is the composition algebra's to police
    };
    let Some(&callee_card) = callee_sig.cardinalities.get(k) else {
        return Ok(());
    };
    if callee_card > caller_card {
        return Err(wider(
            order,
            fi,
            callee,
            addr,
            format!("the alphabet of tape {ct}"),
        ));
    }
    if callee_card < caller_card {
        out.push(diag_at(
            order,
            fi,
            addr,
            "narrow-alphabet",
            format!(
                "`{}` reads a {callee_card}-symbol alphabet where `{}`'s tape {ct} \
                 is {caller_card} wide",
                order[callee].name, order[fi].name
            ),
        ));
        return Ok(());
    }
    // Equal width: compare the glyphs themselves, when both sides declare
    // them. This is the only mechanism that detects a REORDERING
    // (docs/tmt/language.md (symbol maps)).
    let (Some(caller_if), Some(callee_if)) = (order[fi].interface, order[callee].interface) else {
        return Ok(());
    };
    let (Some(cg), Some(eg)) = (caller_if.glyphs.get(ct), callee_if.glyphs.get(k)) else {
        return Ok(());
    };
    if let Some(pos) = cg.iter().zip(eg).position(|(a, b)| a != b) {
        out.push(diag_at(
            order,
            fi,
            addr,
            "glyph-mismatch",
            format!(
                "`{}` declares ({}) where `{}`'s tape {ct} declares ({}); they first \
                 differ at position {pos}",
                order[callee].name,
                eg.join(", "),
                order[fi].name,
                cg.join(", ")
            ),
        ));
    }
    Ok(())
}

fn wider(
    order: &[FuncRef],
    fi: usize,
    callee: usize,
    addr: u32,
    what: String,
) -> LinkError {
    LinkError::CalleeWider {
        callee: order[callee].name.to_string(),
        caller: order[fi].name.to_string(),
        offset: addr,
        what,
    }
}
```

In `lower`, immediately after `sites` is computed and **before** the `has_bound` early-out (so a bindingless link is graded too):

```rust
    // Index-binding sites are graded once, here, over the one scan every
    // mechanism shares (docs/core.md (link warnings)).
    let diagnostics = check_sites(&order, &sites, machine_sig)?;
```

and thread `diagnostics` into all four `Lowered` constructions in `lower` — the early-out, the frames arm, and the two stamping arms, which take it as a new parameter:

```rust
        CallMech::Mono => {
            let mut lowered = super::stamp::lower_mono(syntax, order, &sites, machine_sig)?;
            lowered.diagnostics = diagnostics;
            Ok(lowered)
        }
        CallMech::Hybrid => {
            let mut lowered = super::stamp::lower_hybrid(syntax, order, &sites, machine_sig)?;
            lowered.diagnostics = diagnostics;
            Ok(lowered)
        }
```

- [ ] **Step 5: Run and fix any fixture the new error catches**

Run: `CARGO_TARGET_DIR=/Users/mellonis/Developer/mellonis-workspace/machines/toolchains/target cargo test -p mtc-core`
Expected: PASS. **If an existing core fixture now fails with `CalleeWider`, STOP and report it** — Task 1's sweep was supposed to find it, and a fixture that trips the rule is either a latent bug in the fixture or evidence the rule is too broad.

Run: `CARGO_TARGET_DIR=/Users/mellonis/Developer/mellonis-workspace/machines/toolchains/target cargo test -p mtc-turing-machine && CARGO_TARGET_DIR=/Users/mellonis/Developer/mellonis-workspace/machines/toolchains/target cargo test -p mtc-post-machine`
Expected: PASS, and the reason is structural rather than statistical: **PM-1 objects carry no signatures at all.** PM's compiler builds them through `ObjectFile::v2` (`crates/core/src/formats/object/mod.rs:292-314`), which sets `signatures: None`, so the entry function has none; `link()` then takes the `None` arm of its `match entry_sig` (`crates/core/src/linker/mod.rs:417-442`) and **never calls `engine::lower` at all**. No lowering means no `scan_sites`, no `check_sites`, and no grading — for PM-1 and for any other unsigned program. Confirm with `golden_programs` and `asm_volatile` explicitly.

- [ ] **Step 6: Un-ignore the CLI warning tests**

Remove the four `#[ignore = "raised by the plain-site check task"]` attributes Task 11 left in `crates/turing-machine/tests/link_warnings.rs` and run it.

Run: `CARGO_TARGET_DIR=/Users/mellonis/Developer/mellonis-workspace/machines/toolchains/target cargo test -p mtc-turing-machine --test link_warnings`
Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add crates/core/src/linker/engine.rs crates/core/src/linker/mod.rs crates/core/tests/link_checks.rs crates/turing-machine/tests/link_warnings.rs
git commit -m "feat(core): the link stage grades a call site that binds by index"
```

Then `git log -1 --format=%B`; amend if a `Claude-Session:` line was appended.

---

### Task 13: Graft drift

A unit that grafted a library graph records the graph's qualified name and the CRC-32 of the body it spliced (`ObjectFile.grafts`). The exporting object records the digest of the body it means (`Interface.graphs`). When both are in the link they must agree; a graph whose exporter is not in the link is a header-only library and is not checked — the one place a header is trusted, stated as such.

**Files:**
- Modify: `crates/core/src/linker/interface.rs` (`check_graft_drift`)
- Modify: `crates/core/src/linker/mod.rs` (`LinkError::GraftDrift`, the call in `link`)
- Modify: `crates/core/tests/link_checks.rs`
- Modify: `docs/core.md`

**Interfaces:**
- Consumes: `LinkOptions.sources` (`crates/core/src/linker/mod.rs:208-226`) for readable input names.
- Produces:
  - `LinkError::GraftDrift { graph: String, consumer: String, library: String }`.
  - `pub(super) fn interface::check_graft_drift(objects: &[ObjectFile], libraries: &[ObjectFile], sources: &[Option<String>]) -> Result<(), LinkError>`.

**Why it is object-level, not `FuncRef`-level:** `Interface.graphs` and `ObjectFile.grafts` describe the whole unit, not one blob, so `FuncRef` never carries them. The check runs in `link()` over `objects.iter().chain(libraries)`, re-deriving the "user objects first, then libraries, first-wins" order locally — `resolve` builds that namespace but does not expose it.

- [ ] **Step 1: Write the failing tests**

Append to `crates/core/tests/link_checks.rs`:

```rust
/// An exporter and a consumer that agree, and one that does not. The
/// `.graph` and `.grafted` directives are object-level, before the first
/// `.func` (docs/formats.md (routine interfaces)).
fn exporter(digest: u32) -> String {
    format!(
        "\
.graph lib::findA, {digest}
.routine lib::facade, tapes=1, alpha=(3)
.param t, ('_', '0', '1')
.section code
.func lib::facade
        ret
"
    )
}

fn consumer(digest: u32) -> String {
    format!(
        "\
.grafted lib::findA, {digest}
.routine main, tapes=1, alpha=(3)
.param t, ('_', '0', '1')
.section code
.func main
        call    lib::facade
        stp
"
    )
}

/// Mutation it catches: compare the graph NAMES only and a drifted body
/// links silently — the failure mode the digest exists to prevent.
#[test]
fn a_drifted_graft_digest_is_refused() {
    let err = link(
        &fake_syntax(),
        &[asm(&consumer(42))],
        &[asm(&exporter(7))],
        opts(CallMech::Frames),
    )
    .expect_err("a drifted digest must stop the link");
    assert!(
        matches!(&err, LinkError::GraftDrift { graph, .. } if graph == "lib::findA"),
        "{err:?}"
    );
}

/// Mutation it catches: compare with `!=` inverted and the agreeing pair
/// is refused instead.
#[test]
fn an_agreeing_graft_digest_links() {
    link(
        &fake_syntax(),
        &[asm(&consumer(42))],
        &[asm(&exporter(42))],
        opts(CallMech::Frames),
    )
    .expect("agreeing digests link");
}

/// A header-only library exports nothing to check against. Mutation it
/// catches: treat a missing exporter as a mismatch and every header-only
/// library stops linking.
#[test]
fn a_graft_with_no_exporter_in_the_link_is_unchecked() {
    const ALONE: &str = "\
.grafted lib::findA, 42
.routine main, tapes=1, alpha=(3)
.param t, ('_', '0', '1')
.section code
.func main
        stp
";
    link(&fake_syntax(), &[asm(ALONE)], &[], opts(CallMech::Frames))
        .expect("an unexported graft is not checked");
}
```

**[shape-copied]** — `.graph lib::findA, 3735928559` / `.grafted other::h, 42` are `crates/core/tests/asm_interface.rs:87-88`, and a graft-only file with no `.func` is `:277`.

- [ ] **Step 2: Run to verify they fail**

Run: `CARGO_TARGET_DIR=/Users/mellonis/Developer/mellonis-workspace/machines/toolchains/target cargo test -p mtc-core --test link_checks graft`
Expected: FAIL — `LinkError::GraftDrift` does not exist.

- [ ] **Step 3: Add the error variant**

In `crates/core/src/linker/mod.rs`, after `CalleeWider`:

```rust
    /// A unit spliced a library graph whose body does not match the one
    /// the exporting object describes: the two CRC-32 digests disagree,
    /// so the consumer compiled against a header that has drifted from
    /// its object (docs/core.md (graft drift)). `consumer` and `library`
    /// name the two inputs, using their `LinkOptions::sources`
    /// provenance when the caller supplied it.
    GraftDrift {
        graph: String,
        consumer: String,
        library: String,
    },
```

with the `Display` arm:

```rust
            Self::GraftDrift {
                graph,
                consumer,
                library,
            } => write!(
                f,
                "`{graph}` was grafted into {consumer} from a header that does not \
                 match {library}"
            ),
```

- [ ] **Step 4: Write the check**

In `crates/core/src/linker/interface.rs`:

```rust
/// Compare every recorded library graft against the digest its exporter
/// declares (docs/core.md (graft drift)). The exporter is found in the
/// linker's own namespace order — user objects first, then libraries,
/// first-wins — so a user object that also exports the graph shadows a
/// library exactly as it shadows a symbol. A graph no input exports is
/// NOT checked: that is a header-only library, the one place a header is
/// trusted.
///
/// Object-level, not per-blob: `Interface::graphs` and
/// `ObjectFile::grafts` describe a whole unit, so this runs over the
/// inputs rather than over the reached order, and reachability does not
/// gate it — a unit either spliced that body or it did not.
pub(super) fn check_graft_drift(
    objects: &[ObjectFile],
    libraries: &[ObjectFile],
    sources: &[Option<String>],
) -> Result<(), LinkError> {
    let inputs: Vec<&ObjectFile> = objects.iter().chain(libraries).collect();
    let name_of = |i: usize| -> String {
        sources
            .get(i)
            .and_then(Option::as_ref)
            .cloned()
            .unwrap_or_else(|| format!("input #{i}"))
    };
    // First-wins exporter map, in the namespace's own order.
    let mut exporter: HashMap<&str, (usize, u32)> = HashMap::new();
    for (i, obj) in inputs.iter().enumerate() {
        let Some(iface) = obj.interface.as_ref() else {
            continue;
        };
        for g in &iface.graphs {
            exporter.entry(g.name.as_str()).or_insert((i, g.digest));
        }
    }
    for (i, obj) in inputs.iter().enumerate() {
        for graft in &obj.grafts {
            let Some(&(lib, digest)) = exporter.get(graft.graph.as_str()) else {
                continue; // header-only library: nothing to check against
            };
            if digest != graft.digest {
                return Err(LinkError::GraftDrift {
                    graph: graft.graph.clone(),
                    consumer: name_of(i),
                    library: name_of(lib),
                });
            }
        }
    }
    Ok(())
}
```

Extend the module's `use` list with `use crate::formats::object::ObjectFile;` and `use std::collections::HashMap;`.

In `crates/core/src/linker/mod.rs::link`, immediately after `let resolved = resolve::resolve(objects, libraries, entry)?;`:

```rust
    // Graft provenance is object-level and reachability does not gate it:
    // a unit either spliced that body or it did not
    // (docs/core.md (graft drift)).
    interface::check_graft_drift(objects, libraries, &options.sources)?;
```

- [ ] **Step 5: Document it**

In `docs/core.md`, after the `### Link warnings` section Task 10 added:

```markdown
### Graft drift

A graph is a compile-time template, so it travels as source rather than
inside an object. The link stage verifies the splice after the fact: the
exporting object records, per exported graph, a CRC-32 of the graph's
canonical text, and a unit that spliced one records the graph's qualified
name and the digest of the body it actually spliced. When both objects
are in the link the two must agree, and a mismatch stops it — the
consumer compiled against a header that has drifted from its object.

A graph no input exports is not checked. That is a header-only library,
and it is the one place a header is trusted; the trust is deliberate and
recorded here rather than discovered.
```

- [ ] **Step 6: Run and commit**

Run: `CARGO_TARGET_DIR=/Users/mellonis/Developer/mellonis-workspace/machines/toolchains/target cargo test -p mtc-core && CARGO_TARGET_DIR=/Users/mellonis/Developer/mellonis-workspace/machines/toolchains/target cargo test -p mtc-post-machine && CARGO_TARGET_DIR=/Users/mellonis/Developer/mellonis-workspace/machines/toolchains/target cargo test -p mtc-turing-machine`

```bash
git add crates/core/src/linker/interface.rs crates/core/src/linker/mod.rs crates/core/tests/link_checks.rs docs/core.md
git commit -m "feat(core): the link stage refuses a library graft whose digest has drifted"
```

Then `git log -1 --format=%B`; amend if a `Claude-Session:` line was appended.

---

### Task 14: Cross-object bound calls, and the v4 reader's `map_written` normalization

The spec notes that the cross-object bound-call path "works by inspection" (`crates/core/src/linker/resolve.rs:305-312` resolves a bound callee exactly like a relocation) but that **every bound-call test links one object**. This task closes that, in core and on the TM side, and folds in the one piece of phase-1 review hygiene that belongs to the reader.

**The hygiene:** a hand-crafted v4 stream can decode to a binding with pairs and `map_written: false`, which the writer's own debug assert then forbids on re-encode — `from_bytes` would produce a value `to_bytes` rejects. The reader must normalize: a binding that carries pairs, or is open, has `map_written` set.

**Files:**
- Modify: `crates/core/src/formats/object/read.rs` (the v4 tape-binding decode)
- Modify: `crates/core/src/formats/object/tests.rs`
- Create: `crates/core/tests/link_cross_object.rs`

**Interfaces:**
- Consumes: everything through Task 13.
- Produces: nothing new in `src/` beyond the reader's normalization.

- [ ] **Step 1: Write the failing tests**

Create `crates/core/tests/link_cross_object.rs` with the `link_interface.rs` dialect helpers copied in:

```rust
//! A caller object and a callee object, linked together: numeric and
//! symbolic bindings, under every mechanism. Every other bound-call test
//! in the suite links ONE object, so the cross-object path has until now
//! been correct only by inspection (docs/core.md (linking)).

// … fake_syntax / asm / MECHS / opts copied from link_interface.rs …

/// The callee, alone in its own object, declaring its interface.
const CALLEE: &str = "\
.routine mylib::plusOne, tapes=1, alpha=(3)
.param num, ('_', '0', '1'), writes=('0', '1')
.section code
.func mylib::plusOne
        wr      [2]
        ret
";

/// The caller, in its own object, naming the callee by symbol and
/// binding it SYMBOLICALLY.
const CALLER_SYMBOLIC: &str = "\
.routine main, tapes=1, alpha=(5)
.param t, ('_', 'a', 'b', '0', '1')
.section code
.func main
        call    mylib::plusOne [num: 0{3->'0', 4->'1'}]
        stp
";

/// The same caller with the binding written numerically.
const CALLER_NUMERIC: &str = "\
.routine main, tapes=1, alpha=(5)
.param t, ('_', 'a', 'b', '0', '1')
.section code
.func main
        call    mylib::plusOne [0{3->1, 4->2}]
        stp
";

/// The whole point: a symbolic cross-object binding links to the same
/// image the numeric one does, under every mechanism.
///
/// Mutation it catches: resolve against the CALLER's interface instead of
/// the callee's (a one-index slip in `FuncRef::interface`) and `'0'`
/// resolves to 3 rather than 1 — the two images diverge.
#[test]
fn a_symbolic_cross_object_binding_links_like_the_numeric_one() {
    for mech in MECHS {
        let a = link(
            &fake_syntax(),
            &[asm(CALLER_SYMBOLIC), asm(CALLEE)],
            &[],
            opts(mech),
        )
        .unwrap_or_else(|e| panic!("the symbolic form must link under {mech}: {e}"));
        let b = link(
            &fake_syntax(),
            &[asm(CALLER_NUMERIC), asm(CALLEE)],
            &[],
            opts(mech),
        )
        .unwrap_or_else(|e| panic!("the numeric form must link under {mech}: {e}"));
        assert_eq!(
            a.executable.to_bytes(),
            b.executable.to_bytes(),
            "cross-object symbolic != numeric under {mech}"
        );
    }
}

/// The callee as a LIBRARY rather than a user object: the same
/// resolution, through the first-wins library namespace.
///
/// Mutation it catches: restrict the interface lookup to user objects and
/// a library callee resolves nothing.
#[test]
fn a_symbolic_binding_resolves_against_a_library_callee() {
    for mech in MECHS {
        link(
            &fake_syntax(),
            &[asm(CALLER_SYMBOLIC)],
            &[asm(CALLEE)],
            opts(mech),
        )
        .unwrap_or_else(|e| panic!("a library callee must resolve under {mech}: {e}"));
    }
}
```

**[tool-verified equivalent]** — the same pair was run end to end through the real TM toolchain: `tmt asm app.tma`, `tmt asm mylib.tma`, `tmt link app.tmo mylib.tmo --nostdlib` refuses today with `bad binding to 'mylib::plusOne': the call site uses a named entry`, and the numeric twin links to `1 composite(s), 0 stamp(s), 4 B compose table` under frames.

And the reader normalization test, appended to `crates/core/src/formats/object/tests.rs`:

The writer's debug assert fires on `to_bytes` for an un-normalized value, so the test cannot round-trip one — it must build the BYTES. Phase 1 already established the idiom for exactly this, in `reserved_binding_flags_rejected` (`crates/core/src/formats/object/tests.rs:926-940`) over the `minimal_v4_bound_call()` helper (`:908-924`): serialize, assert the expected flags byte at a known tail offset, patch it, restamp the CRC, read back. Follow it.

```rust
    /// A hand-crafted v4 stream can spell a binding that is OPEN with a
    /// cleared `map_written` flag, or one that carries pairs with the
    /// flag cleared. The writer's own invariant forbids both values
    /// (docs/formats.md (bound calls)), so `from_bytes` would otherwise
    /// hand back something `to_bytes` panics on. The reader normalizes.
    ///
    /// Mutation it catches: drop the normalization and `map_written`
    /// comes back false, so a caller that re-encodes the value trips the
    /// writer's debug assert — a `from_bytes`/`to_bytes` asymmetry no
    /// round-trip proptest can reach, because the proptest only ever
    /// generates legal values.
    #[test]
    fn the_reader_normalizes_map_written_from_open() {
        let mut obj = minimal_v4_bound_call();
        obj.bound_calls[0].binding[0].open = true;
        let mut bytes = obj.to_bytes();
        // Same tail as `reserved_binding_flags_rejected`: binding flags,
        // pair count (u16), exit count, graft count (u32).
        let pos = bytes.len() - 8;
        assert_eq!(
            bytes[pos], 0b11,
            "layout assumption: the binding-flags byte (map written | open)"
        );
        bytes[pos] = 0b10; // open, map_written cleared — the illegal spelling
        crate::formats::crc32::stamp_crc(&mut bytes, CRC_OFFSET);
        let back = ObjectFile::from_bytes(&bytes).expect("reads back");
        assert!(
            back.bound_calls[0].binding[0].map_written,
            "an open map is a written one"
        );
        // And the normalized value survives its own re-encoding.
        assert_eq!(
            ObjectFile::from_bytes(&back.to_bytes()).expect("re-reads"),
            back
        );
    }
```

**Then add the pairs half.** It needs its own helper — `minimal_v4_bound_call()` deliberately carries no pairs so its tail offset is fixed. Write `one_pair_v4_bound_call()` beside it (same object, one `MapPair { src: 1, dst: 1, dst_label: None, one_way: false }` and `map_written: true`), read `crates/core/src/formats/object/write.rs`'s binding encoder to get the pair record's width, and compute the flags byte's offset from the end the same way. **Pin it with the same `assert_eq!(bytes[pos], 0b01, "layout assumption: …")` guard before patching** — that assertion is what turns a wrong offset into a loud failure instead of a silently patched neighbouring byte. Then clear bit 0, restamp, and assert `map_written` comes back true.

- [ ] **Step 2: Run to verify they fail**

Run: `CARGO_TARGET_DIR=/Users/mellonis/Developer/mellonis-workspace/machines/toolchains/target cargo test -p mtc-core --test link_cross_object && CARGO_TARGET_DIR=/Users/mellonis/Developer/mellonis-workspace/machines/toolchains/target cargo test -p mtc-core the_reader_normalizes
Expected: the cross-object tests may already PASS (the path is correct by inspection — that is the point of adding them); the reader test FAILS.

If a cross-object test fails, that is a real finding: report the exact assertion and the mechanism before changing anything.

- [ ] **Step 3: Normalize in the reader**

In `crates/core/src/formats/object/read.rs`, at the end of the v4 tape-binding decode (after the pairs are read), add:

```rust
            // The writer's invariant: a binding with pairs, and an open
            // binding, both have `map_written` set — a map with pairs was
            // written by definition, and an open map is a written one
            // (docs/formats.md (bound calls)). A hand-crafted stream can
            // spell the flag clear anyway, and the value would then be one
            // the writer refuses to re-encode, so normalize on the way in.
            let map_written = map_written || open || !pairs.is_empty();
```

- [ ] **Step 4: Run and commit**

Run: `CARGO_TARGET_DIR=/Users/mellonis/Developer/mellonis-workspace/machines/toolchains/target cargo test -p mtc-core`
Expected: PASS, including `crates/core/tests/format_roundtrips.rs`'s proptests — the normalization must be idempotent on every legal value, or the round trip goes red.

```bash
git add crates/core/src/formats/object/read.rs crates/core/src/formats/object/tests.rs crates/core/tests/link_cross_object.rs
git commit -m "test(core): a cross-object bound call links symbolically, and the v4 reader normalizes map_written"
```

Then `git log -1 --format=%B`; amend if a `Claude-Session:` line was appended.

---

### Task 15: The imported-alphabet record — PULLABLE

**Read this first.** The spec says the linker compares an imported alphabet's glyph list recorded by the compiler with the exporting object's — but MO v4 as landed carries only EXPORTED alphabets, so the compiler has nowhere to put an import. This task adds that place. **It is a wire addition, and the controller may defer the whole task** without touching any other: nothing else in this plan depends on it, and phase 3 is what would fill the list. MO 4 is unreleased, so no version bumps either way.

**Files:**
- Modify: `crates/core/src/formats/object/mod.rs` (`ImportedAlphabet`, `Interface.imports`)
- Modify: `crates/core/src/formats/object/write.rs`, `read.rs`, `tests.rs`
- Modify: `crates/core/tests/format_roundtrips.rs`
- Modify: `crates/core/src/asm/disassembler.rs` (the comment line)
- Modify: `crates/core/src/linker/interface.rs` (`check_imported_alphabets`)
- Modify: `crates/core/src/linker/mod.rs` (`LinkError::AlphabetDrift`)
- Modify: `crates/core/tests/link_checks.rs`
- Modify: `docs/formats.md`

**Interfaces:**
- Produces:
  - `pub struct ImportedAlphabet { pub name: String, pub glyphs: Vec<String> }`.
  - `Interface.imports: Vec<ImportedAlphabet>` — wire-encoded after `graphs`: a `u32` count, then per entry a string index for the name, a `u32` glyph count, and that many string indices.
  - `LinkError::AlphabetDrift { alphabet: String, consumer: String, library: String, position: usize }`.
  - `pub(super) fn interface::check_imported_alphabets(objects, libraries, sources) -> Result<(), LinkError>` — same object-level, first-wins shape as `check_graft_drift`.

**`.tma` has no spelling for an import**, exactly as it has none for an export: both are compiler facts. `tmt dis` prints them as comments, `; import alphabet <name>: (<glyphs>)`, alongside the existing `; alphabet <name>: (<glyphs>)` export comments — so an object carrying either does not round-trip through `dis → asm` byte-identically, which is the same recorded limit exported alphabets already carry and not a new exception to the text-expressibility gate.

- [ ] **Step 1: Write the failing format test**

Append to `crates/core/src/formats/object/tests.rs`:

```rust
    /// An imported alphabet survives its own bytes, next to an exported
    /// one. Mutation it catches: write the list and forget to read it (or
    /// the reverse) and the value does not come home.
    #[test]
    fn imported_alphabets_round_trip() {
        // `v4_sample()` (crates/core/src/formats/object/tests.rs:494) is
        // the module's own v4 fixture: signatures, one `RoutineInterface`,
        // one exported alphabet, one exported graph. Extend it rather
        // than building a second one.
        let mut obj = v4_sample();
        let iface = obj.interface.as_mut().expect("v4_sample carries one");
        iface.imports.push(ImportedAlphabet {
            name: "other::wide".to_string(),
            glyphs: vec!["_".to_string(), "a".to_string()],
        });
        assert_eq!(ObjectFile::from_bytes(&obj.to_bytes()).expect("reads back"), obj);
    }
```

`v4_sample()` already carries an exported alphabet (`bits`), so the test exercises exports and imports side by side without adding one.

- [ ] **Step 2: Run to verify it fails**

Run: `CARGO_TARGET_DIR=/Users/mellonis/Developer/mellonis-workspace/machines/toolchains/target cargo test -p mtc-core imported_alphabets_round_trip`
Expected: FAIL — `ImportedAlphabet` does not exist.

- [ ] **Step 3: Add the type and the wire form**

In `crates/core/src/formats/object/mod.rs`, next to `ExportedAlphabet`:

```rust
/// An alphabet the unit IMPORTED by name from another unit, with the
/// glyph list it compiled against. The linker compares it with the
/// exporting object's own list: a header that drifted from its object
/// would otherwise re-label a tape silently
/// (docs/formats.md (routine interfaces)).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportedAlphabet {
    pub name: String,
    pub glyphs: Vec<String>,
}
```

and in `Interface`, after `graphs`:

```rust
    /// Alphabets this unit imported from another, with the glyphs it
    /// compiled against. Written by the compiler; `.tma` has no spelling
    /// for one, and `tmt dis` prints them as comments like exported
    /// alphabets.
    pub imports: Vec<ImportedAlphabet>,
```

In `write.rs`, after the `graphs` list: a `u32` count, then per entry the name's string-pool index (`u32`), a `u32` glyph count, and that many string indices. In `read.rs`, the mirror — and an empty present list is legal here (unlike an `enters`/`leaves` clause, an import with no glyphs is simply an alphabet of nothing, which the compiler will never write but the reader need not police).

In `crates/core/src/asm/disassembler.rs`, wherever exported alphabets print their `; alphabet …` comment, print imports as `; import alphabet <name>: (<glyphs>)` immediately after.

- [ ] **Step 4: Extend the proptest**

In `crates/core/tests/format_roundtrips.rs`, find the v4 `Interface` generator and add an `imports` strategy mirroring the `alphabets` one (0..3 entries, each with 0..4 glyph strings). Run the file:

Run: `CARGO_TARGET_DIR=/Users/mellonis/Developer/mellonis-workspace/machines/toolchains/target cargo test -p mtc-core --test format_roundtrips`
Expected: PASS, including the never-panics-on-noise case.

- [ ] **Step 5: Add the drift check**

In `crates/core/src/linker/mod.rs`:

```rust
    /// A unit imported an alphabet whose glyph list does not match the
    /// one the exporting object declares: the consumer compiled against a
    /// header that has drifted from its object, so it would read the
    /// exporter's tape through the wrong glyphs
    /// (docs/core.md (graft drift)). `position` is the first index at
    /// which the two lists differ, or the shorter list's length when one
    /// is a prefix of the other.
    AlphabetDrift {
        alphabet: String,
        consumer: String,
        library: String,
        position: usize,
    },
```

with the `Display` arm:

```rust
            Self::AlphabetDrift {
                alphabet,
                consumer,
                library,
                position,
            } => write!(
                f,
                "`{alphabet}` as imported by {consumer} differs from {library}'s own \
                 declaration, first at position {position}"
            ),
```

In `crates/core/src/linker/interface.rs`, `check_imported_alphabets` mirrors `check_graft_drift` exactly: build a first-wins map from `Interface::alphabets` over `objects.iter().chain(libraries)`, then walk every input's `Interface::imports` and compare glyph lists. An import no input exports is NOT checked — the same header-only rule.

**`position` has one definition, and it must be implemented as exactly that:** the index of the first element at which the two lists differ, or — when one list is a prefix of the other — the shorter list's length. In code:

```rust
            let differs = imported
                .glyphs
                .iter()
                .zip(&exported.glyphs)
                .position(|(a, b)| a != b)
                .or_else(|| {
                    (imported.glyphs.len() != exported.glyphs.len())
                        .then(|| imported.glyphs.len().min(exported.glyphs.len()))
                });
```

`None` means the lists agree entirely; `Some(position)` is the error. Note the `.or_else` arm: `zip` stops at the shorter list, so a pure prefix would otherwise report no difference at all and a truncated import would link silently.

Call it in `link()` on the line after `check_graft_drift`.

- [ ] **Step 6: Test the check**

Append to `crates/core/tests/link_checks.rs` a pair of hand-built objects — the assembler has no spelling for either list, so build `ObjectFile` values directly from a `.tma`-assembled base and push the `ExportedAlphabet`/`ImportedAlphabet` entries onto `obj.interface`:

```rust
/// Mutation it catches: compare only the alphabet NAMES and a drifted
/// glyph list links silently — the exact hazard item 4 demonstrated.
#[test]
fn a_drifted_imported_alphabet_is_refused() {
    let mut lib = asm(&exporter(1));
    lib.interface
        .as_mut()
        .expect("the exporter declares an interface")
        .alphabets
        .push(mtc_core::formats::object::ExportedAlphabet {
            name: "lib::bits".to_string(),
            glyphs: vec!["_".into(), "0".into(), "1".into()],
        });
    let mut app = asm(&consumer(1));
    app.interface
        .as_mut()
        .expect("the consumer declares an interface")
        .imports
        .push(mtc_core::formats::object::ImportedAlphabet {
            name: "lib::bits".to_string(),
            glyphs: vec!["_".into(), "1".into(), "0".into()],
        });
    let err = link(&fake_syntax(), &[app], &[lib], opts(CallMech::Frames))
        .expect_err("a drifted import must stop the link");
    assert!(
        matches!(&err, LinkError::AlphabetDrift { position: 1, .. }),
        "{err:?}"
    );
}
```

- [ ] **Step 7: Document and commit**

In `docs/formats.md`, in the interface-section description, add the imports list to the layout table and state that `.tma` has no spelling for it (a compiler fact, printed as a comment), with the same recorded round-trip limit exported alphabets carry. In `docs/core.md`'s `### Graft drift` section, add a sentence: an imported alphabet is verified the same way, against the exporting object's own declaration.

Run: `CARGO_TARGET_DIR=/Users/mellonis/Developer/mellonis-workspace/machines/toolchains/target cargo test -p mtc-core && CARGO_TARGET_DIR=/Users/mellonis/Developer/mellonis-workspace/machines/toolchains/target cargo test -p mtc-post-machine --test golden_programs`

```bash
git add crates/core/src/formats/object/mod.rs crates/core/src/formats/object/write.rs crates/core/src/formats/object/read.rs crates/core/src/formats/object/tests.rs crates/core/src/asm/disassembler.rs crates/core/src/linker/interface.rs crates/core/src/linker/mod.rs crates/core/tests/format_roundtrips.rs crates/core/tests/link_checks.rs docs/core.md docs/formats.md
git commit -m "feat(core): objects record the alphabets they imported, and the link stage checks them"
```

Then `git log -1 --format=%B`; amend if a `Claude-Session:` line was appended.

---

### Task 16: The TM three-mechanism `.tma` matrix, and `mode_equivalence`

`opt_equivalence.rs`'s matrix is driven by `.tmc` programs, and the compiler has no `state` parameters until phase 3 — so it cannot take an exit-bearing program yet. This task adds the `.tma`-driven equivalent on the TM side and extends the relink byte-identity sweep with the three new shapes.

**Files:**
- Modify: `crates/turing-machine/tests/link_matrix.rs` — **created by Task 8b**, which already put the harness (`build`/`run`/`cell_at` copied from `mono_run.rs:17-63`, `MECHS`, the imports) and the `CLOSURE_FOLD` run test there. This task APPENDS the remaining fixtures and keeps 8b's.
- Modify: `crates/turing-machine/tests/mode_equivalence.rs:740-769`

**Interfaces:**
- Consumes: `mtc_turing_machine::asm::{assemble, link}`, `mtc_core::vm::{ArchRegistry, Machine, WideTape, Tape, RunOptions, RunLimits, Outcome}`, `mtc_turing_machine::arch::Tm1`.
- Produces: nothing other crates read.

All three fixtures below are **[tool-verified]**: each was assembled with `cargo run -q -p mtc-turing-machine --bin tmt -- asm` and disassembled back on 2026-09-14.

- [ ] **Step 1: Append to the matrix**

`crates/turing-machine/tests/link_matrix.rs` already exists: Task 8b created it with the module header, the `build`/`run`/`cell_at` harness copied from `crates/turing-machine/tests/mono_run.rs:17-63`, the `MECHS` array, and `CLOSURE_FOLD` with `a_closure_fold_program_agrees_across_mechanisms`. **Keep all of it** and append the fixtures below; the harness is shared, so do not re-declare `build`, `run`, `cell_at` or `MECHS`.

Widen the module header to name the whole set:

```rust
//! The three call mechanisms agree on the shapes phase 2 adds: an
//! exit-bearing call (from one site and from three), a fold inside a
//! stamped copy, an open binding, a mixed splice-and-frame caller, and
//! a cross-object bound call. Driven from `.tma`, because the `.tmc`
//! front end has no `state` parameters yet
//! (docs/core.md (call mechanisms)).
```

then append:

```rust
/// ONE exit-bearing site: hybrid splices it (one site never pays to
/// share), mono splices it, frames descriptors it. All three must leave
/// the same tape.
const ONE_EXIT_SITE: &str = "\
.routine main, tapes=1, alpha=(3)
.param t, ('_', '0', '1')
.routine pick, tapes=1, alpha=(3), exits=2
.param n, ('_', '0', '1')
.section tables
T0:     .row    [0]
        .row    [1]
        .row    [2]
T1:     .targets zero, one, two
.section code
.func main
        call    pick [n: 0] exits=(won, lost)
        stp
won:    wrmv    [1], [.]
        stp
lost:   wrmv    [2], [.]
        stp
.func pick
        rd
        mtc     T0
        djmp    T1
zero:   retx    #0
one:    retx    #1
two:    ret
";

/// THREE exit-bearing sites into one routine: the shape hybrid's byte
/// rule may share. Whatever it decides, the three mechanisms must agree
/// on the tape.
const THREE_EXIT_SITES: &str = "\
.routine main, tapes=1, alpha=(3)
.param t, ('_', '0', '1')
.routine pick, tapes=1, alpha=(3), exits=1
.param n, ('_', '0', '1')
.section tables
T0:     .row    [0]
        .row    [*]
T1:     .targets zero, rest
.section code
.func main
        call    pick [n: 0] exits=(a)
        stp
a:      call    pick [n: 0] exits=(b)
        stp
b:      call    pick [n: 0] exits=(c)
        stp
c:      wrmv    [2], [.]
        stp
.func pick
        rd
        mtc     T0
        djmp    T1
zero:   retx    #0
rest:   ret
";

/// An open binding: a 5-symbol band into a 3-symbol callee that declares
/// its tape opaque and carries a `*` row. Modelled on the probe's
/// hand-authored descriptor, written declaratively.
const OPEN: &str = "\
.routine main, tapes=1, alpha=(5)
.param t, ('_', 'a', 'b', 'c', 'd')
.routine swapABopen, tapes=1, alpha=(3)
.param n, ('_', 'a', 'b'), writes=('a', 'b'), opaque
.section tables
T0:     .row    [0]
        .row    [1]
        .row    [2]
        .row    [*]
T1:     .targets done, swapA, swapB, pass
.section code
.func main
        call    swapABopen [0{1->1, 2->2, *}]
        stp
.func swapABopen
walk:   rd
        mtc     T0
        djmp    T1
swapA:  wrmv    [2], [>]
        jmp     walk
swapB:  wrmv    [1], [>]
        jmp     walk
done:   ret
pass:   wrmv    [-], [>]
        jmp     walk
";

/// The cross-object caller and callee, assembled separately and linked
/// together.
const XO_CALLER: &str = "\
.routine main, tapes=1, alpha=(5)
.param t, ('_', 'a', 'b', '0', '1')
.section code
.func main
        call    mylib::plusOne [num: 0{3->'0', 4->'1'}]
        stp
";

const XO_CALLEE: &str = "\
.routine mylib::plusOne, tapes=1, alpha=(3)
.param num, ('_', '0', '1'), writes=('0', '1')
.section code
.func mylib::plusOne
        rd
        wrmv    [2], [.]
        ret
";

/// Mutation it catches: break any one mechanism's exit lowering — mono's
/// `retx → jmp`, frames' descriptor rebase, hybrid's routing — and the
/// three stop agreeing on the final tape.
#[test]
fn an_exit_bearing_program_agrees_across_mechanisms() {
    for src in [ONE_EXIT_SITE, THREE_EXIT_SITES] {
        let results: Vec<_> = MECHS
            .iter()
            .map(|&m| run(&build(src, m), &[3]))
            .collect();
        for (m, r) in MECHS.iter().zip(&results[1..]) {
            assert_eq!(
                (&results[0].0, &results[0].1),
                (&r.0, &r.1),
                "mono vs {m} diverged on an exit-bearing program"
            );
        }
    }
}

/// Mutation it catches: revert the open rule anywhere — the sparse map,
/// `dense_map`'s guard, or mono's `read_image` — and the opaque symbols
/// trap under at least one mechanism, so the outcomes diverge.
#[test]
fn an_open_binding_program_agrees_across_mechanisms() {
    let results: Vec<_> = MECHS.iter().map(|&m| run(&build(OPEN, m), &[5])).collect();
    for (m, r) in MECHS.iter().zip(&results[1..]) {
        assert_eq!(
            (&results[0].0, &results[0].1),
            (&r.0, &r.1),
            "mono vs {m} diverged on an open binding"
        );
    }
}

/// Mutation it catches: resolve a cross-object binding against the wrong
/// object's interface and the callee writes through the wrong glyph, so
/// at least one mechanism's tape differs.
#[test]
fn a_cross_object_program_agrees_across_mechanisms() {
    let caller = assemble(XO_CALLER, false).expect("assembles");
    let callee = assemble(XO_CALLEE, false).expect("assembles");
    let images: Vec<_> = MECHS
        .iter()
        .map(|&m| {
            link(
                &[caller.clone(), callee.clone()],
                &[],
                LinkOptions {
                    call_mech: m,
                    ..Default::default()
                },
            )
            .unwrap_or_else(|e| panic!("the {m} link failed: {e}"))
            .executable
        })
        .collect();
    let results: Vec<_> = images.iter().map(|e| run(e, &[5])).collect();
    for (m, r) in MECHS.iter().zip(&results[1..]) {
        assert_eq!(
            (&results[0].0, &results[0].1),
            (&r.0, &r.1),
            "mono vs {m} diverged on a cross-object bound call"
        );
    }
}
```

`build` in `mono_run.rs` links one object; the cross-object test calls `link` directly, so import `LinkOptions` and `mtc_turing_machine::asm::{assemble, link}` in this file.

- [ ] **Step 1b: Add the mixed splice-and-frame fixture**

The three fixtures above are all-exit-bearing, so none of them exercises the one shape where a splice's caller offsets MOVE: a caller holding both a spliced exit-bearing site and a framed holey one, where the frames path widens the second 5 → 9 bytes and shifts everything after it in that blob. **[tool-verified]** — assembled on 2026-09-14.

```rust
/// One caller, two sites: an exit-bearing one hybrid splices, and a
/// holey one it frames. The framed site widens, shifting the splice's
/// `then` and its exits — the only shape in this file where the offsets
/// a splice fixup names are not the ones the record carried.
const MIXED_SPLICE_AND_FRAME: &str = "\
.routine main, tapes=1, alpha=(5)
.param t, ('_', 'a', 'b', 'c', 'd')
.routine pick, tapes=1, alpha=(5), exits=1
.param n, ('_', 'a', 'b', 'c', 'd')
.routine holey, tapes=1, alpha=(3)
.param m, ('_', 'a', 'b')
.section tables
T0:     .row    [0]
        .row    [*]
T1:     .targets zero, rest
.section code
.func main
        call    pick [0] exits=(won)
        stp
won:    call    holey [0{1->1, 2->2}]
        wrmv    [3], [.]
        stp
.func pick
        rd
        mtc     T0
        djmp    T1
zero:   retx    #0
rest:   ret
.func holey
        ret
";

/// Mutation it catches: drop `splice_shift` on the hybrid path and the
/// splice's `then` lands on the wrong instruction (or misses the offset
/// map and fails the link), so hybrid stops agreeing with mono.
#[test]
fn a_mixed_splice_and_frame_caller_agrees_across_mechanisms() {
    let results: Vec<_> = MECHS
        .iter()
        .map(|&m| run(&build(MIXED_SPLICE_AND_FRAME, m), &[5]))
        .collect();
    for (m, r) in MECHS.iter().zip(&results[1..]) {
        assert_eq!(
            (&results[0].0, &results[0].1),
            (&r.0, &r.1),
            "mono vs {m} diverged on a mixed splice/frame caller"
        );
    }
}
```

- [ ] **Step 2: Run the matrix**

Run: `CARGO_TARGET_DIR=/Users/mellonis/Developer/mellonis-workspace/machines/toolchains/target cargo test -p mtc-turing-machine --test link_matrix`
Expected: PASS. If a mechanism diverges, that is the finding — report the outcome pair and the diverging tape before touching anything.

- [ ] **Step 3: Extend the relink byte-identity sweep**

In `crates/turing-machine/tests/mode_equivalence.rs`, add the three programs to `every_program_relinks_byte_identically_in_every_mode`'s list. They are defined in `link_matrix.rs`, so copy them into `mode_equivalence.rs` as three new consts rather than cross-importing between integration test binaries (each is its own crate):

```rust
    for src in [
        CROSS_ALPHABET,
        NESTED_TWO_LEVEL,
        EQUAL_SIZE_BIJECTION,
        TRAP_TAXONOMY,
        NARROWER_IDENTITY,
        IN_RANGE_HOLES,
        UNALIASED,
        ONE_EXIT_SITE,
        THREE_EXIT_SITES,
        OPEN,
        MIXED_SPLICE_AND_FRAME,
    ] {
```

`CLOSURE_FOLD` is **already in this list** — Task 8b added it and its const alongside. Do not add it twice; do not drop it. `OPEN` is the const's name in `link_matrix.rs`, so keep it spelled the same on both sides, and `MIXED_SPLICE_AND_FRAME` is the fixture Step 1b adds.

(The cross-object program is not added here: `build_full` links one object, and widening its signature for one case is not worth it — the cross-object determinism is covered by `crates/core/tests/link_cross_object.rs`, whose `a_symbolic_cross_object_binding_links_like_the_numeric_one` byte-compares the two images under every mechanism, in Task 14.)

- [ ] **Step 4: Run the sweep**

Run: `CARGO_TARGET_DIR=/Users/mellonis/Developer/mellonis-workspace/machines/toolchains/target cargo test -p mtc-turing-machine --test mode_equivalence`
Expected: PASS. A failure here means a decision somewhere is not deterministic — the hybrid fold's byte rule and the intern keys are the two candidates.

- [ ] **Step 5: Commit**

```bash
git add crates/turing-machine/tests/link_matrix.rs crates/turing-machine/tests/mode_equivalence.rs
git commit -m "test(turing-machine): a .tma three-mechanism matrix over exits, open bindings and cross-object calls"
```

Then `git log -1 --format=%B`; amend if a `Claude-Session:` line was appended.

---

### Task 17: Documentation and the full gate

**Files:**
- Modify: `docs/formats.md` (the "What resolves them, and when." paragraph)
- Modify: `docs/core.md` (`## The linker`, `### The link report`, `## The composition engine`, `### Call mechanisms`)
- Modify: `docs/superpowers/plans/2026-09-14-binding-arc-phase-2-linker.md` (the behaviour-change statement)

**Interfaces:**
- Consumes: everything.
- Produces: the durable pages the phase's code comments cite.

- [ ] **Step 1: Make `docs/formats.md`'s bound-call paragraph present tense**

Find the paragraph headed "What resolves them, and when." in the bound-calls section. It currently describes the symbolic forms as carried-but-unresolved. Rewrite it to say what happens now, in prose with no tracker references:

```markdown
**What resolves them, and when.** A symbolic bound site — a parameter
name instead of a list position, a glyph label instead of a callee symbol
index — is resolved at LINK time, against the callee's interface section,
before the composition engine reads a binding. The parameter name gives
the callee tape, the entries are reordered into the callee's own tape
order, and each glyph label becomes that glyph's position in the callee's
declared alphabet for that tape. A callee that describes no interface can
be reached only by a transparent call: there is nothing to resolve
against, and the link says so rather than guessing. The resolution
produces a fresh numeric binding; the object is never rewritten, because
a labelled pair carries `dst` 0 on the wire and filling it in would make
the value one the writer refuses to re-encode.

An **open** map (`{…, *}`) is not symbolic and is not resolved away: it
is read by the composition algebra itself, which sends every unlisted
caller symbol one-way onto the index equal to the callee's cardinality —
an index no callee row names, so only a `*` cell matches it and only a
keep preserves it. The link refuses an open binding into a tape the
callee does not declare opaque.

An **exit vector** is likewise carried through rather than resolved away.
Its entries are blob-relative offsets in the CALLING function; under
frames they become absolute code addresses in the site's descriptor,
under mono the call site jumps into a per-site copy whose returns jump to
them instead.
```

- [ ] **Step 2: Extend `docs/core.md`'s linker sections**

Under `## The linker` (`:642`), add a `### Symbolic resolution` subsection stating: the pre-pass runs between name resolution and the composition engine; it is where a parameter name, a glyph label, an open binding's opaque precondition and an exit vector's length are checked; it never mutates an object; and it is placed there rather than in name resolution because name resolution is shared with the standalone name-resolution query editors run, which has no business failing over a binding.

Under `### Call mechanisms` (`:811`), add the exit lowering and the fold rule:

```markdown
An **exit-bearing** site — one whose callee takes state parameters —
lowers differently under each mechanism, and never collapses to a plain
call whatever its binding: a plain call returns through the address it
pushed and has nowhere to put the other exits.

- **Frames** puts the site's exit vector in its descriptor, alongside the
  composed placement. The vector holds absolute code addresses, so an
  exit-bearing descriptor is address-dependent where an exit-free one is
  not, and the link rebases it when the calling function is placed.
- **Mono** splices. The site jumps into a per-site copy of the callee in
  which the plain return becomes a jump to the instruction after the
  call, and each multi-exit return becomes a jump to its exit. Nothing is
  pushed, so nothing has to be popped — which matters because the
  architecture has no pop.
- **Hybrid** groups the exit-bearing sites reaching one routine under one
  composite and decides by byte count: with `k` sites, a body of `B`
  bytes and would-be descriptors of `d` bytes in total, the group is
  shared under frames when `k` is at least two and `(k − 1) · B` exceeds
  `d`; otherwise each site splices. Two sites over a large body share;
  two over a small one splice, because two copies cost less than a body
  plus two descriptor loads. Every input to that comparison is a
  link-time count, so the decision is deterministic and a relink is
  byte-identical. The link report prints each decision.

  The count runs over the whole reachable set, not only the sites
  visible at the machine's own frame. A site met while a routine is
  being copied under some composite joins the group its composed
  binding lands in, and when that group is shared the copy reaches the
  one shared body through a framed call whose descriptor is that
  composed binding plus the site's own exits. So a routine can be
  shared on the strength of sites that only exist inside copies — which
  is the case the rule is worth having for.
```

Under `### The link report` (`:752`), add the `diagnostics` and `folds` fields to whatever field list the section carries.

- [ ] **Step 3: Run the full gate**

Run each, in order, and record the result:

```
CARGO_TARGET_DIR=/Users/mellonis/Developer/mellonis-workspace/machines/toolchains/target cargo fmt --check
CARGO_TARGET_DIR=/Users/mellonis/Developer/mellonis-workspace/machines/toolchains/target cargo clippy --workspace --all-targets -- -D warnings
CARGO_TARGET_DIR=/Users/mellonis/Developer/mellonis-workspace/machines/toolchains/target cargo build -p mtc-core --no-default-features
CARGO_TARGET_DIR=/Users/mellonis/Developer/mellonis-workspace/machines/toolchains/target cargo build --workspace --lib --target wasm32-unknown-unknown
CARGO_TARGET_DIR=/Users/mellonis/Developer/mellonis-workspace/machines/toolchains/target cargo nextest run --workspace
```

**`cargo nextest run --workspace` is the final gate, not `cargo test`** — one process per test is what exposes a shared-state assumption, and it has caught a stdlib `OnceLock` regression before.

- [ ] **Step 4: Re-confirm the standing regression gates explicitly**

```
CARGO_TARGET_DIR=/Users/mellonis/Developer/mellonis-workspace/machines/toolchains/target cargo test -p mtc-post-machine --test golden_programs
CARGO_TARGET_DIR=/Users/mellonis/Developer/mellonis-workspace/machines/toolchains/target cargo test -p mtc-post-machine --test asm_volatile
CARGO_TARGET_DIR=/Users/mellonis/Developer/mellonis-workspace/machines/toolchains/target cargo test -p mtc-turing-machine --test opt_equivalence
CARGO_TARGET_DIR=/Users/mellonis/Developer/mellonis-workspace/machines/toolchains/target cargo test -p mtc-turing-machine --test mode_equivalence
CARGO_TARGET_DIR=/Users/mellonis/Developer/mellonis-workspace/machines/toolchains/target cargo test -p mtc-core --lib linker::compose
```

PM-1 byte identity, the everything-matrix, the three-mechanism equivalence, and the composition algebra's law proptests.

- [ ] **Step 5: State what changed**

Append to this plan file, under a `## Behaviour changes in phase 2` heading:

```markdown
## Behaviour changes in phase 2

One. **An index-binding site into a callee wider than the caller — in
tape count, or in the alphabet of a shared tape — is now a link error.**
It was unchecked: the linker inspected nothing at a plain site, not even
arity.

Two things sharpen that sentence, and both are deliberate:

- **"Site" is wider than "call".** The rule grades every relocated edge
  into another function — a plain call, and also a tail jump or
  conditional branch, which the linker classifies identically because
  the tail-call pass turns one into the other. A call-only rule would
  miss the same hazard on any optimized program.
- **A program whose entry function carries no signature is graded not at
  all.** There is no machine signature to grade against, and the whole
  composition engine is skipped for such a link. Hand-assembled files
  without `.routine` lines are therefore unaffected, and so is every
  `pmt` program.

Everything else the phase adds is either a resolution of a form that was
previously REFUSED (named entries, glyph labels, open maps, exit
vectors), a new WARNING that does not stop a link, or an error on a form
that could not be written before. No existing program's image moves; the
sweep that measured the error's blast radius is Task 1, and its recorded
result is above.
```

Fill in the sweep's actual finding rather than repeating "clean" blindly.

- [ ] **Step 6: Commit**

```bash
git add docs/core.md docs/formats.md docs/superpowers/plans/2026-09-14-binding-arc-phase-2-linker.md
git commit -m "docs(core): symbolic resolution, graft drift, link diagnostics and the exit lowering"
```

Then `git log -1 --format=%B`; amend if a `Claude-Session:` line was appended.

---

## Self-review

### 1. Spec coverage

| Spec requirement | Task | Notes |
|---|---|---|
| Pre-pass before the composition engine; `FuncRef` gains the interface | 2 | Arena, not mutation — the labelled-pair `dst` 0 invariant forbids rewriting the object |
| Parameter name → tape position; entries reordered | 3 | |
| Glyph label → callee symbol index | 4 | Runs after the reorder, so the glyph list is the right tape's |
| Positional entries unchanged | 3 | The `named == 0 && !labelled && !open && exits.is_empty()` early-out clones verbatim |
| `BadBinding`: no interface ("only a transparent call can reach it") | 3 | `require_interface` |
| `BadBinding`: unknown parameter, parameter bound twice, missing parameter | 3 | Three distinct messages, three tests |
| `BadBinding`: unknown glyph | 4 | |
| The four refusal tests flip; `refuse_symbolic_binding` deleted piecewise | 3, 4, 5, 6, 9 | One form per task; the function is emptied in 5 and deleted in 9 |
| P1 — exits go through compose | 6 | The descriptor is `compose(...)` plus the vector |
| P2 — mono enters through `jmp`; `ret → jmp then`, `retx #k → jmp exit_k` | 7 | Plus `ArchSyntax.return_opcode` and `FuncRef.site_fixups` |
| Mono stamp key `(routine, composite, exits, then)` | 7 | Appended to the KEY, never to the digest |
| `build_stamp` accepts `retx` only in an exit-bearing splice | 7 | The `MonoRawFrame` refusal stays for exit-free bodies |
| Exit count vs the callee's declared `exits` is `BadBinding` | 6 | Checked in the pre-pass |
| P3 — `is_full_passthrough` gains `exits.is_empty()` | 6 | Conjoined identically at both call sites; the function's own signature unchanged |
| P4 — hybrid groups, sizes and decides; the decision is in the report | 8 | Compose-column term dropped (below) |
| P4 — the count runs over the WHOLE reachable set, including sites met inside stamped closures | 8b | Probe → decide → build; a shared closure site frames through `compose(C, binding)` plus its exits |
| Open bindings: compose, stamp, engine; mono keeps the `*` row | 5 | `open_unlisted` + the two `<=` guard widenings |
| `OpenBindingUnsupported` for a non-opaque or interfaceless tape | 5 | |
| Graft drift | 13 | Object-level, first-wins, header-only unchecked |
| Plain-site checks and the omitted-map warning; `{}` silences | 12 | Gated on Task 1's sweep; grades relocated tail jumps and branches too (S1), and nothing at all for an unsigned entry (S2) |
| A header's `noreturn` checked against the object's `returns` bit | — | **Deferred to phase 3** (S3): headers arrive there, so there is nothing yet to disagree with |
| Link diagnostics: code, message, function, offset, line under `-g` | 10 | |
| Codes join the shared allow namespace; `lint.allow`, `--allow`, `-Werror` | 11 | Fifth arm of `known_code` |
| Codes enter the registry ↔ docs set-compare | 10, 11 | `docs/core.md (### Link warnings)` + `docs/tmt/cli.md` |
| Completions registry, man page, `cli_docs` | 11 | The man page follows `LINK_USAGE` automatically |
| PM-1's CLI untouched | 10, 11 | `LinkReport` has one construction site; PM reads by reference |
| Cross-object bound-call test | 14 | Core and TM |
| Imported-alphabet check | 15 | **PULLABLE**; the wire addition is stated as such |
| v4 reader normalizes `map_written` | 14 | |
| `docs/formats.md` "What resolves them, and when." present tense | 17 | |
| `docs/core.md` linker sections | 10, 13, 17 | |
| `docs/tmt/cli.md`, `docs/tmt/lint.md` | 11 | |
| Matrices: exits from one site and from three, open, cross-object | 16 | `.tma`-driven; `.tmc` waits for phase 3 |
| `mode_equivalence` relink sweep extended | 16 | |
| Full gate + PM-1 identity + the behaviour-change statement | 17 | |

**Gaps, stated rather than hidden:**

- **The compose-column term is dropped from the hybrid formula.** It depends on the directory size, which is not known when the decision is made, and the relink byte-identity gate requires determinism. Recorded below.
- **The spec's `enters`/`leaves` runtime asserts and the `enters-unmet` lint are phase 3**, not this plan, per the spec's own sequencing.

### 2. Placeholder scan

Searched the plan for "TBD", "TODO", "implement later", "fill in details", "add appropriate error handling", "write tests for the above", "similar to Task N". None present. Four places deliberately require the implementer to **measure** rather than transcribe, and each says exactly what to measure and what to do with it:

- Task 1's sweep output and its stop gate.
- Task 8's `nop` count in `THREE_SITES_BIG_BODY`, tuned until the byte rule flips, with the count to be recorded in a comment.
- Task 14's `the_reader_normalizes_map_written_from_the_pairs`, whose byte surgery depends on the writer's flag-bit position, with a stated fallback.
- Task 15's `imported_alphabets_round_trip`, which reuses the existing v4 fixture builder by name.

Two places name a symbol the implementer must confirm against the code rather than trust: `DecodedOperand`'s `Imm8` variant name (Task 7) and `MapFile`'s function-list field (Task 6). Both say so inline.

### 3. Type consistency

- `interface::resolve_bindings` / `interface::rebind` — defined Task 2, called once in `link` (Task 2), extended in Tasks 3–6 through `resolve_one` only. One name throughout.
- `resolve_one` / `require_interface` / `reorder_named` / `resolve_labels` / `check_opaque` / `bad` — all introduced in Tasks 3–5 and referenced by those exact names afterwards.
- `FuncRef.interface` (Task 2), `FuncRef.site_fixups` (Task 7) — both spelled identically everywhere they appear.
- `Lowered` with fields `order`/`plan`/`stats`/`orphaned`/`diagnostics`/`folds` — introduced Task 2, filled in Tasks 8, 10, 12. Never `LoweredOrder` after Task 2.
- `LinkDiagnostic { code, message, function, offset, line }` — declared Task 2, registered Task 10, produced by `diag_at` (Task 10) and consumed by `render_link_diagnostics` (Task 11). `code` is `&'static str` in all three.
- `FoldDecision { routine, sites, body_bytes, descriptor_bytes, shared }` — declared Task 2, filled Task 8, rendered Task 11. Same five field names.
- `DIAGNOSTIC_CODES` — Task 10, read by Task 11's `known_code` and by Task 10's docs guard.
- `LinkError` additions: `OpenBindingUnsupported { callee, tape, param }` (5), `CalleeWider { callee, caller, offset, what }` (12), `GraftDrift { graph, consumer, library }` (13), `AlphabetDrift { alphabet, consumer, library, position }` (15). Each has its `Display` arm in the same step.
- `FramesPlan.engine_exits: Vec<Option<(usize, Vec<u32>)>>` — Task 6, consumed in Task 6's layout change only.
- `ArchSyntax.return_opcode` / `ArchSyntax::jump_opcode()` — Task 7, used in Task 7's mono splice only.
- `is_full_passthrough`'s signature is **unchanged**; the exits conjunct is spelled `record.exits.is_empty() && is_full_passthrough(...)` at both call sites (`engine.rs` and `stamp.rs`) in Task 6. This was the specific drift risk flagged in review, and it is resolved by not changing the function.
- `widen_shift(sites, old)` — Task 6, one definition, used once (the frames closure).
- `SpliceSite { caller, then, exits }` and `splice_shift(widened, old)` — Task 7, one definition each. `splice_shift` is deliberately NOT `widen_shift`: the frames closure derives its widening set from a `SiteKind` list, while a splice's set is known only after the mono/frames split, so the two take different inputs and neither can stand in for the other. Both are used in Task 7 and constrained by Task 8's ordering note.
- `mono_stamps`' new `widened: &[HashSet<u32>]` parameter — Task 7, passed empty by `lower_mono` and populated by `lower_hybrid` after its group loop (Task 8).
- `descriptor_cost(composite, machine_sig, order, exits) -> Result<u32, LinkError>` — Task 8, one definition, four arguments; used by Task 8's identity-world sites and Task 8b's closure sites through the same `GroupSite` match, and by both through `group_composite[key]`. It wraps `engine::materialize`, so the size can never disagree with the emitter.
- `GroupSite { Identity, InClosure }`, `ClosureSite`, `StampTarget { Plain, Framed }`, `mono_closure_probe` — Task 8b, one definition each. `GroupSite` REPLACES Task 8's `(usize, u32, &BoundCall)` group element; the group KEY `(callee, canonical_key(&composite))` is unchanged in both tasks, which is what puts an identity-world site and a closure site in one group.
- `engine::materialize(c, machine_sig, order, exits)` — Task 6 defines it private, Task 8b makes it `pub(super)`. One signature, four arguments, both call sites.
- `mono_stamps`' `shared: &HashSet<(usize, Vec<u8>)>` parameter — Task 8b; `lower_mono` passes an empty set, `lower_hybrid` passes `shared_pairs`.
- `check_sites` / `grade_tape` / `wider` — Task 12, one definition each.
- `check_graft_drift` / `check_imported_alphabets` — Tasks 13 and 15, same parameter shape `(objects, libraries, sources)`.

---

## Decisions for the controller

Every point where this plan had to choose. None is settled by the spec.

- **(H) The imported-alphabet record is a wire addition, and Task 15 is marked PULLABLE.** The spec says the linker compares an imported alphabet's glyph list with the exporter's, but MO v4 as landed carries only EXPORTED alphabets — there is nowhere for the compiler to put an import. Task 15 adds `Interface.imports: Vec<ImportedAlphabet>` with its encoding, the disassembler comment line, and the `AlphabetDrift` check. MO 4 is unreleased, so nothing bumps either way, and phase 3 is what would fill the list. **Defer the whole task and nothing else in this plan changes.**
- **(E) The wider-callee error's shape.** The plan implements the spec literally: a callee wider in tape count, or in the alphabet of a shared tape, is `LinkError::CalleeWider` on a plain site AND on a bound site whose map is omitted. Task 1's sweep is the gate, and if any shipped program trips it the plan offers the narrowing — restrict the error to plain sites, and leave an omitted-map bound tape on its existing hole-and-trap behaviour, which `binding_to_composite` already gives a defined runtime meaning. **The narrowing is defensible and the plan does not take it unilaterally.**
- **(E) A wider callee on a BOUND site is not a new hazard the way a plain one is.** A bound site already passes through `binding_to_composite`, which holes the gap and traps at run time. The plan says so in Task 12's table but still errors, per the spec; the narrowing above is the alternative.
- **(P4) The compose-column term is dropped from the hybrid byte rule.** `(k − 1)·B > Σ d_i` is implemented; the spec's additional "+ the compose-column entries" depends on `K`, the directory size, which is not known when the decision is made — a circular dependency, and `mode_equivalence`'s relink byte-identity requires determinism. The term is second-order. **Restore it only with a defined proxy for `K` at decision time.**
- **(P4) ~~Hybrid groups over the identity world only.~~ RULED BY THE CONTROLLER, 2026-09-14: not for this plan to narrow.** The ratified rule — "the fold happens wherever the sites are, including inside stamped closures" — is built in full by **Task 8b**: `mono_closure_probe` reports the exit-bearing bijection sites met inside copies with their bindings pre-composed against the enclosing composite, they join their `(routine, composite)` group, and a shared group is reached from inside the copy through a `call.m` whose descriptor is `compose(C, binding)` plus the site's exits. The circular dependency the narrowing had been justified by — a stamp that reaches a shared callee frames instead of recursing, so the closure's shape depends on the decision — is broken by probing before deciding and building after, over one shared walk.
- **(P4) `descriptor_cost` is EXACT.** An earlier draft of this plan estimated it from the binding and argued the estimate was a safe upper bound; **that argument was wrong in both halves** — the formula under-counted the dense maps (it sized them by the number of listed pairs, where `descriptor_bytes` emits one `u16` per symbol of the whole alphabet whenever the map is not identity), and under-counting `d` makes sharing MORE attractive, not less. It is now taken from `engine::materialize(...).len()`, the emitter's own output. That is available at decision time: `materialize` reads only the machine signature and the callee's signature, and the blob rewrite changes neither. One source of truth, no second formula to drift.
- **(S1) `check_sites` grades every `SiteKind::Plain` edge, including relocated tail jumps and conditional branches into another function.** The spec says "plain call site"; `scan_sites` classifies a relocated tail jump or branch the same way (`crates/core/src/linker/engine.rs:595-609`), and grading those is deliberate — the tail-call pass turns calls into jumps, so a call-only rule would let the identical hazard through on any optimized program. Task 1's sweep covers them because it walks `obj.relocations`. Disclosed in Task 1, Task 12 and "Behaviour changes".
- **(S2) A link whose entry function has no signature is graded not at all.** `link()` calls `engine::lower` only for a signed entry (`crates/core/src/linker/mod.rs:417-442`); otherwise there is no machine signature to grade against and the engine is skipped entirely. So a hand-assembled `.tma` with no `.routine` lines gets no plain-site check — the same shape of degradation the spec describes for a callee with no interface, one level up. This is also the stated reason PM-1 is untouched throughout: PM builds objects through `ObjectFile::v2`, which sets `signatures: None`.
- **(S3) The header-`noreturn`-versus-object-`returns` check is DEFERRED to phase 3.** The spec lists it under the linker ("A header's `noreturn` is likewise checked against the object's `returns` bit"), but headers do not exist until phase 3 — there is nothing in this phase for the object's bit to disagree with. The bit is read and round-tripped by the format layer already; only the comparison waits.
- **(S4) The mono stamp key carries the CALLER INDEX alongside `(routine, composite, exits, then)`.** `then` and the exit offsets are caller-blob-relative, so two sites in different functions can share a `then` value and be entirely different splices. A precision of the spec's tuple, not a departure from it.
- **(S5) Task 6 rebases engine-descriptor exits in `emit_planned_region`, not in `append_frame_descriptor`.** The spec points at the path raw-descriptor exits take today, and that path is `append_frame_descriptor` — which works from a function's own table blob. An engine descriptor is not in any function's table blob; it is appended to the table section by `emit_planned_region`, which runs after layout's per-function loop (Established fact 7). So the rebase lives there, and layout retains the per-function offset maps for it. Same arithmetic, different site.
- **(P2) Two new mechanisms the spec does not name.** Mono's exit lowering needs (a) `ArchSyntax.return_opcode`, because `ret`, `stp` and `hlt` are indistinguishable in the syntax table, which touches **36 `ArchSyntax` literals**; and (b) `FuncRef.site_fixups`, a cross-function code-offset fixup layout patches, because `FuncRef.calls` names a function START and a splice jumps into the middle of the caller. Both follow existing precedents (`trap_opcode`; table fixups), but both are structural.
- **(P2) An exit-bearing mono stamp's NAME is numbered, not refused.** Two exit-bearing splices of one `(routine, composite)` share a digest by construction — the exits are deliberately not in it, so existing stamp names do not move — so `intern` appends `.1`, `.2` rather than raising `StampNameCollision`. The alternative, widening the digest, would rename every existing stamp and move every existing mono image.
- **(F) Where a link warning's location comes from.** A compile warning renders `{path}:{line}:{col}: warning: {msg}`; a link diagnostic has neither path nor column. The plan renders `{function}:{line}: warning: {msg} [{code}]` under `-g` and `` {function}+0x{offset:04x}: warning: {msg} [{code}] `` without. **An alternative is to thread the map sidecar's per-function `source` into the render for a real path.**
- **(F) Warnings print always, the report stays behind `-v`.** The spec says "warnings print always, in the compile warning format", which contradicts the existing framing that the link report prints under `-v`. The plan splits them: `render_link_diagnostics` always, `render_link_report` only under `-v`.
- **(F) `tmt link` honours `--allow` only; `tmt build` also unions the manifest's `lint.allow`.** `tmt link` consumes prebuilt objects and has no project context — it passes `sources: Vec::new()` for exactly that reason — and adding a manifest walk to it would be a new discovery path. `tmt build` already has the manifest loaded, `lint.allow` parsed and validated.
- **(F) The TM crate gains `render_link_report`, mirroring PM's.** The report is rendered at three inlined sites with three different formats in the TM crate today (`cli/build.rs:341`, `cli/driver.rs:557`, `:781`). The plan factors it, which is how PM already does it.
- **(F) `pmt` produces link diagnostics it does not render.** The codes are arch-agnostic and live in core's linker, so a PM-1 link could in principle raise one. In practice PM-1 programs are single-arity and single-alphabet, so nothing can be graded — but the field exists in PM's reports, unrendered. **Deliberate, per the spec's "`pmt` is untouched".**
- **(Placement) The pre-pass lives in a new `crates/core/src/linker/interface.rs`, called from `link()` in `mod.rs`, not from `engine::lower`.** The resolved records live in an arena that must outlive the engine call, and `lower` returns `Vec<FuncRef<'a>>` — an arena created inside it would be dropped. The module placement follows the retired guard's own reasoning: not in `resolve`, which is shared with `resolve_names`, the standalone query the editor overlays run.
- **(Ownership) The arena, not owned records.** `FuncRef.bound` keeps `&'a BoundCall` and `SiteKind` keeps its lifetime; the pre-pass re-points them at arena records. This was verified to compile (`cargo check -p mtc-core`) before the plan was written. The alternative — owned `BoundCall` in `FuncRef` and owned `Vec<TapeBinding>` in `SiteKind` — is a larger refactor with no benefit once the arena rewrites `FuncRef.bound`, which is what makes hybrid's second scan correct by construction.
- **(P2) Three consequences of `site_fixups` that the spec does not mention, and that a reviewer should know were chosen rather than missed.** (a) `prune_unreachable` must reindex `site_fixups`' function index alongside `calls` and `bound`, or a splice jump follows a dropped function's old slot — Task 7 adds that loop and a test that forces the prune. (b) A splice's caller offsets are pre-rewrite, and under hybrid the frames path widens the caller's surviving framed sites afterwards, so the offsets are shifted through `splice_shift` before they become fixups — Task 7 spells it, Task 8 orders it, Task 16 has the mixed fixture. (c) The placeholder displacement is 0 (a jump to the following boundary) so layout's own decode accepts the blob before the patch lands; any other placeholder risks layout's non-boundary-jump rejection.
- **(Task 1) The sweep resolves callees across the stdlib and the sibling units, not within one object.** Every `call std::…` in the corpus is an external symbol, so an in-object lookup would report nothing about precisely the sites being measured. The sweep prints a per-file compared/unresolved tally, and the plan states that a sweep resolving nothing is a failed sweep rather than a clean one. The sibling-unit widening is deliberately over-broad: a spurious resolution only widens what is reported, while a missed one hides a finding.
- **(Message texts)** Every `BadBinding` message in this plan is invented, not quoted from the spec, which names the CASES but not the wording. They are listed together so they can be revised in one pass: "the binding mixes named and positional entries; write every entry one way or the other"; "the call site uses {form}, but `{callee}` describes no interface; only a transparent call can reach it"; "the binding names parameter `{name}`, which `{callee}` does not declare"; "the binding names parameter `{name}` twice"; "the binding does not bind parameter `{name}`; an argument list is complete"; "binding tape {k} names glyph `{label}`, which is not in `{callee}`'s alphabet for parameter `{p}`"; "the call site supplies {n} exit(s), but `{callee}` declares {m}"; "the body returns through exit {k}, but the call site supplies {n} exit(s)".
- **(Diagnostics catalog)** Two codes: `glyph-mismatch` and `narrow-alphabet`. The spec writes "(`glyph-mismatch`, `narrow-alphabet`, …)" — the ellipsis is not filled, and the checks in this phase need no third code. Every other finding here is an error and outside the namespace.
- **(Task 1 prior, not a substitute for the sweep)** The TM compiler emits no `.param`, so no compiled object carries an interface in phase 2 and the glyph-level checks are structurally dormant on the shipped corpus until phase 3; the only live plain-site comparison is arity/cardinality from `RoutineSig`. The four shipped plain `call std::` sites (`docs/examples/rpn/rpn.tmc:75,82,90,99`) are cardinality-equal. The sweep still runs.

