# TM-1 arc — phase 5b: the composition engine

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** declarative binding calls link and run — the linker computes the finite closure of (routine, composite) pairs, lowers every call site per `--call-mech = mono | frames | hybrid`, and the three modes are observably equivalent on the same programs and tapes (the phase-5 milestone: three-mode equivalence green).

**Architecture:** a link-time composition engine running as a pre-pass between `resolve` and `layout` (layout is decode-once/shrink-only and cannot inject code — fact-sheet surprise 4): it produces a synthetic function set. The compose model is the MAINTAINER-RULED runtime compose table via redefined `call.m` semantics (ruling 2026-07-17, recorded in the ledger): `call.m`'s operand half becomes a call-site index S; at runtime `FR' = compose[FR][S]`, `descriptor = directory[FR']`; FR is a composite INDEX (0 = identity), restoring spec §2's register semantics. Every descriptor in the image is absolute (composition happens at link), so hand-authored raw `call.m` sites become constant compose columns — one uniform semantics, no new opcode, `.tma` stays 0.2. Mono mode stamps rewritten copies; hybrid classifies per site.

**Tech stack:** Rust 2024; no new dependencies.

## Global Constraints

1. **PM-1 byte-identity.** Goldens untouched; PM-1 emits v1 images with no frames content; the engine pre-pass is a no-op for objects without `bound_calls`/frames content; frameless/bindingless links stay byte-identical (lock tests).
2. **Core stays arch-agnostic** — the engine is driven by MO record kinds and `ArchSyntax`, tested with the fake dialect; zero TM-1 knowledge in core.
3. **The compose model (ruled):** `call.m` operand table-half = call-site index (u32, dense per-image); `FR' = compose[FR][S]`; `descriptor = directory[FR']`; FR = composite index, 0 = identity, 1..=K = directory entries. Raw sites = constant columns. Identity-composite sites lower to plain `call` (§5.6 — callee inherits the frame; joins the existing call→call.s relaxation).
4. **MX v2 frames region** (pre-release header amendment — v2 ships first at the arc release): header gains `frames_offset: u32` (0 = none; else offset INTO the tables section). At that offset: `composite_count K: u16 LE`, `site_count S: u16 LE`, directory `K × u32 LE` (descriptor offsets in the tables section), compose table `(K+1) × S × u16 LE` (rows = active FR 0..=K, columns = site index; entries = composite indices 1..=K, or `0xFFFF` = site invalid under that context → `Trap::NoTransition`? NO — an invalid context/site pair is a LINK-TIME impossibility (the closure enumerated every reachable pair); entry 0 is reserved-invalid at runtime and traps `Trap::BadOperand{at}`; the linker never emits a reachable 0). The `v2_layout_is_exact` pin test grows the field; `is_v1_shape` requires `frames_offset == 0`; docs/formats.md updated.
5. **Trap taxonomy invariant** preserved across ALL three modes on the same inputs: no-transition vs unmapped-read vs unmapped-write, distinct and mode-independent (equivalence harness asserts trap KIND equality).
6. **Composition algebra laws** (property-tested): associativity of binding composition; identity laws (E∘id = id∘E = E, detected and deduped to the same index); hole composition = outer holes ∪ preimages of inner holes; one-way (`one_way`) pairs participate in the read direction only and are excluded from bijectivity/injectivity checks; blank pinned (0↔0) with collapse-onto-blank legal (5a corrected rule).
7. **Mapping legality (the linker is authoritative):** binding arity == callee `RoutineSig.arity`; equal-size alphabets identity-complete to a bijection with all failures static (missing pairs filled by identity; conflicts = `LinkError`); unequal-size mappings legal with runtime holes; per-tape symbols bounded by `cardinalities`.
8. **Mono stamping:** decode the generic body via `decode_stream`; rewrite `wr`/`mov` vector operands (positions per projection at the CALLER's width — vector padding: match rows get wildcards at unbound positions, write vectors keep-markers 0x7F, move vectors stay 0) and match-table rows (cells re-mapped; 0x7F wildcard preserved; one-way collapses expand rows per collapsed preimage — growth counted in LinkReport); synthesize unmapped-read trap rows (`[*,…,u,…,*] → trap #0` stub) PREPENDED per table, and replace unmapped-write sites with `trap #1` stubs; identical stamped copies dedup by (callee, canonical composite) key.
9. **LinkReport grows:** `instantiations` (mono copies per routine), `composites` (directory size K), `compose_table_bytes`, `dedup_savings`, `synthesized_trap_rows`, `expanded_rows` (one-way growth). LinkOptions gains `entry: Option<String>` (default "main") and `call_mech: CallMech { Mono | Frames | Hybrid }` (default Hybrid).
10. **Map sidecar:** `MapFile` gains `#[serde(default)] bindings: Vec<MapBinding>` — structured records per composite (index, routine, per-tape (phys, pairs incl. one_way)) + the canonical label per the spec's grammar (`name@[entry{pairs}]`, identity omission rules, >8 pairs → `{#xxxxxxxx}` CRC-32 digest of the canonical serialization via `formats::crc32::crc32`, `.2` suffix on display collision). One shared render module (`linker/binding_label.rs`) — the same notation dis's legend uses. Structured truth first; labels derived.
11. **Version spaces:** `.tma` dialect stays 0.2 (authoring surface unchanged); MO v3 unchanged; MT v2 unchanged; MX v2 amended pre-release (GC4). No crate version bumps mid-arc.
12. Thin renderer; conventional commits with scope, NO attribution footers; durable citations only (docs/formats.md sections land in T7 — prose before); goldens derivation-first; fmt/clippy clean; AsmErrorKind/LinkError Display style consistency.

## File Structure

- Create: `crates/core/src/linker/{compose.rs,engine.rs,stamp.rs,binding_label.rs}`; `crates/turing-machine/tests/mode_equivalence.rs`
- Modify: `crates/core/src/linker/{mod,resolve,layout}.rs`, `crates/core/src/formats/executable.rs`, `crates/core/src/vm/{core,frame,machine,debug}.rs`, `crates/core/src/asm/disassembler.rs` (legend + call.m site rendering), `crates/core/tests/link_tables.rs`, `crates/turing-machine/src/cli/build.rs` (`--call-mech`, `--entry`), `crates/turing-machine/tests/frames_programs.rs` (redefined call.m expectations), `docs/formats.md`

---

### Task 1: `LinkOptions.entry` + `CallMech` + resolve reaches bound callees

**Files:** `crates/core/src/linker/{mod,resolve}.rs`, `crates/turing-machine/src/cli/build.rs`, tests in link_tables.rs + cli tests

- `LinkOptions { relax: bool, entry: Option<String>, call_mech: CallMech }` (+ `CallMech` enum, Default = Hybrid); `resolve::resolve(objects, libraries, entry: &str)` — replace the two `"main"` literals (resolve.rs:90-94); `NoEntrySymbol` naming unchanged in meaning (message may name the entry).
- Resolve's BFS additionally traverses `bound_calls` (fact surprise 1): `FuncRef` gains `bound: Vec<(u32, usize, &BoundCall)>` (hole offset, callee index, record) built alongside `calls`; bound callees enter reachability; `dropped` accounts for them.
- The `UnsupportedBindings` guard STAYS for this task (the engine replaces it in T4) — but move it AFTER resolve so it only fires for REACHABLE bound calls (a dropped function's bindings no longer poison the link; behavior change, test it).
- `tmt link --entry NAME --call-mech mono|frames|hybrid` flags parsed (usage text updated); pmt link untouched (PM-1 has no bindings; `entry` optionally exposed — implementer's judgment, lean NO for pmt this phase).
- [ ] TDD → implement → gate. Commit: `feat(core): LinkOptions entry and call-mech; resolve reaches bound callees`

### Task 2: MX v2 frames region + redefined `call.m` VM semantics

**Files:** `crates/core/src/formats/executable.rs`, `crates/core/src/vm/{core,frame,machine,debug}.rs`, `crates/core/src/linker/layout.rs` (emission of directory + compose region for RAW sites), tests across all touched files + frames_programs.rs updates

**This is the ruling's implementation task.**
- Executable: `frames_offset: u32` field per GC4 (codec v2 read/write, `is_v1_shape` extended, layout pin test updated, `sectioned()` signature grows or a builder sets it — pick minimal churn).
- VM: `CallFrame { rel, frame }`'s `frame` half is now the SITE INDEX. Execution: `ProfileViolation` check as today → resolve target/EntCheck → on push success: read `compose[FR][S]` (2 bytes at `frames_base + 4 + K*4 + (FR*(S_count)+S)*2` — the Core receives `frames_base/K/S_count` at construction from the Machine, via `with_frames(meta)`), entry 0 → `Trap::BadOperand{at}`; else `FR' = entry`; read `directory[FR'-1]` (4 bytes) → descriptor offset → FrameWalk as today. `Ret`/`RetX` restore-reload goes through the directory (`FR != 0` → directory[FR-1] → offset → FrameWalk). All reads via `FrameRead` (frame_load_cost). `fr()` now reports the index (docs/comments updated).
- Layout (raw-sites emission): when frames content exists, layout builds the directory (all descriptors in table-section order → indices 1..=K), assigns each `FramedCall` piece a site index (dense, piece order), rewrites the operand's table half to the site index, and emits the frames region (constant column per raw site: `compose[F][S] = index(descriptor)` for all F in 0..=K) appended to the tables section with `frames_offset` in the header. Frameless images: `frames_offset = 0`, byte-identity lock.
- 5a test updates: frames_programs.rs (FR trace values become indices; image bytes gain the region), link_tables.rs FRAMES expectations, machine/debug tests. The 5a milestone program must still run UNCHANGED at the source level (authoring surface untouched) — only byte/FR expectations move.
- [ ] TDD → implement → gate (this task has the widest 5a-test churn — every changed expectation must be re-derived, not captured). Commit: `feat(core): the compose model — frames region, directory, call.m site indices`

### Task 3: The composition algebra (`linker/compose.rs`)

**Files:** create `crates/core/src/linker/compose.rs`; property tests in-module + `crates/core/tests/` if public surface suffices

- Pure data + ops, no I/O: `Composite { routine: usize, tapes: Vec<CompositeTape> }`, `CompositeTape { phys: u8, rmap: SparseMap, wmap: SparseMap }` (sparse canonical form: sorted pairs + one_way flags + hole set; dense u16 tables materialized only at descriptor emission).
- Ops: `compose(outer: &Composite-ish binding, inner: &Binding) -> Composite` (associative; hole composition per GC6); `is_identity`; `canonicalize` (sorted, identity pairs elided per the label grammar's rules) → dedup key; `digest` (CRC-32 of canonical serialization); legality per GC7 with typed errors (`LinkError` additions: `BindingArity { callee, expected, got }`, `BindingConflict {...}`, `BindingRange {...}` — Display in house style).
- Property tests: associativity on random composable chains; identity laws; hole-composition equality vs direct computation; one-way exclusion from bijectivity; canonicalize idempotent; digest stability.
- [ ] TDD → implement → gate. Commit: `feat(core): the composition algebra — compose, canonicalize, holes, digests`

### Task 4: The engine pre-pass — closure + frames lowering (replaces the guard)

**Files:** create `crates/core/src/linker/engine.rs`; modify `linker/mod.rs` (guard removed; engine invoked between resolve and layout), `linker/layout.rs` (consume synthetic set), assembler.rs (`emit_frame_descriptor` lifted to `pub(crate)` for reuse — fact surprise 5)
- Closure BFS over (routine, composite) pairs seeded from the entry at identity; deterministic order (BFS + canonical-key tiebreak) → reproducible builds (re-link byte-identity test).
- **Frames mode:** ONE generic copy per routine (spec §5.1 restored by the ruling); every bound-call site becomes a `call.m` site (synthetic 9-byte FramedCall piece replacing the 5-byte bound hole — the engine rewrites blobs BEFORE layout: emit a new blob with the site widened, remapping the function's internal offsets — jumps/labels/fixups/debug — via a per-function offset map; this is the engine's hardest mechanical piece, spell it: walk `decode_stream`, copy instructions, at each bound site emit CALL_M opcode + 8-byte hole, record new Relocation for the rel half + site registration; all other offsets shift by +4 per preceding widened site); compose columns per site: `compose[F][S] = index(compose(directory[F], site binding))` for every reachable F (unreachable pairs → 0); descriptors synthesized from composites (dense maps; reuse lifted emitter); directory + dedup (identical descriptor bytes → one entry; identity composite → column entry that lowers the SITE to plain call instead — §5.6, no call.m at all, feeding the existing relaxation).
- Raw `.frame`/`call.m` sites in input objects flow through unchanged (constant columns, T2's layout path) and compose with engine composites when a raw-framed routine is reached under a non-identity context (compose(active, raw-descriptor-as-binding) — the algebra handles it; test the nesting).
- `.frame` phys-vs-machine-width static check (5a carry-over): the engine validates every descriptor's phys indices against the entry signature's arity → `LinkError`.
- Mono/Hybrid mode selection stubs: this task implements FRAMES fully; `CallMech::Mono`/`Hybrid` return a clear `LinkError::Unimplemented`-style error ONLY until T5 lands (internal, next task — acceptable inter-task state, gate-tested).
- [ ] TDD (fake dialect: two-level nesting R→Q under two contexts — the compose column differs by row; determinism; guard-replacement tests flip from "refused" to "links and runs" via driver where feasible) → implement → gate. Commit: `feat(core): the composition engine — closure, frames lowering, compose emission`

### Task 5: Mono stamping + hybrid classification

**Files:** create `crates/core/src/linker/stamp.rs`; engine.rs mode dispatch; link_tables.rs tests

- Per GC8. Stamped copies: new synthetic functions named `callee$<digest8>` (map-visible; symbol table untouched — linker-internal naming); dedup by canonical composite; unbound positions padded (rows wildcards / writes keep / moves stay) to the CALLER's machine width; trap synthesis (read rows prepended FIRST-match, write sites replaced); one-way row expansion with growth counters; tables rewritten (rows re-mapped through the composite; dispatch tables copied with entries following the stamped body's offsets — reuse the T4 offset-remap machinery).
- Hybrid: per-site classification — completed bijections (post-identity-completion, no holes, no one-way) → stamp; anything holey/one-way → frames. Both paths already exist; this task is the classifier + tests proving a mixed program uses both in one image.
- [ ] TDD → implement → gate. Commit: `feat(core): mono stamping and hybrid classification`

### Task 6: LinkReport + map sidecar bindings + dis legend

**Files:** `linker/{mod,engine,binding_label}.rs`, `asm/disassembler.rs`, `cli` render paths (tmt link -v report lines), tests

- LinkReport fields per GC9 (rendered under `-v` in tmt link — thin renderer).
- `MapFile.bindings` per GC10 + the canonical label renderer (shared module; spec grammar: identity omission, `=>` one-way, digest fallback, `.2` collision suffix).
- Executable dis: `call.m` renders the SITE index + a `; frames legend:` block listing directory entries with labels (glyph-rendering deferred — numeric); with the map sidecar, labels come from `bindings`; without, digests/index-level maps from the image (spec §6.5 inspectability: structure always recoverable).
- DebugSession: no new surface (FR→label resolution is phase-7/DAP scope — confirmed greenfield; the sidecar now CARRIES the data).
- [ ] TDD → implement → gate. Commit: `feat(core): link report counters, sidecar binding records, the dis frames legend`

### Task 7: Three-mode equivalence + docs + milestone gate

**Files:** create `crates/turing-machine/tests/mode_equivalence.rs`; docs/formats.md; final sweeps

- Harness mirrors opt_equivalence.rs: programs (≥4: the spec's cross-alphabet call shape — a delimited-world caller calling a bare-representation callee through a holey one-way binding; a nested two-level composition; an equal-size bijection; a multi-exit graft-like shape) × tapes × THREE modes: compare outcome (incl. trap KIND), final per-tape snapshots, head positions. Plus: re-link determinism (byte-identical); the §5.2 depth-independence property — tact accounting for a composed call at depth 1 vs depth 3 differs only by the constant per-call costs (assert the composed-call overhead is depth-invariant from RunStats deltas).
- The 5a milestone program still runs (regression); a NEW milestone: the same logical program built via DECLARATIVE bindings links in all three modes and matches the raw-frames 5a version's observables.
- docs/formats.md: the frames region layout (GC4 as prose+table), redefined call.m semantics (site index + compose + directory; FR = composite index), the engine's mode semantics summary, sidecar `bindings` schema, LinkReport fields. Forge-agnostic ref-free. Resolve/refresh the frame.rs "normative here" stale note (5a carry-over) with the citation swap.
- Full phase gate + hand CLI run of a binding program in all three modes pasted into the report.
- [ ] TDD → implement → gate. Commit: `feat(turing-machine): three-mode equivalence green — phase-5b milestone` (+ `docs(formats):` separate).

---

## Self-review notes

- Spec coverage: §5.1 modes ✅ T4/T5; §5.2 static composition + compose table + O(1) ✅ T2/T4/T7; §5.3 mono ✅ T5; §5.4 frames ✅ T2/T4; §5.6 identity relaxation ✅ T4 (site → plain call feeding call→call.s); §6.4 sidecar + labels + digests ✅ T6; §6.5 inspectability ✅ T6; §9 linker phases ✅ T1-T6; §16 equivalence + determinism ✅ T7; LinkOptions.entry (manifest absorption) ✅ T1.
- The ruling's consequences all land in T2 (header/VM) and T4 (emission); the spec text needs NO amendment (§3.3's "one compose-table ROM lookup" is now literal — plus one directory read, both priced at call time; record that nuance in docs).
- Deliberately NOT here: `--foutline`/outline pass (phase 6 optimizer); wrmv (phase 6); writes-through-collapse lint (phase 7); FR→label debugger resolution (phase 7/DAP); glyph-rendered legends (needs tape supplied — phase 7 polish); n-byte symbol family (unchanged trigger).
- Risk, stated: T4's blob-rewrite (5→9 byte site widening with offset remapping) is the mechanically hardest piece — it is spelled as an explicit walk with its own offset map, and T5 reuses it. T2's 5a-test churn is wide but mechanical; every new expectation must be re-derived in-test, never captured from a run.
