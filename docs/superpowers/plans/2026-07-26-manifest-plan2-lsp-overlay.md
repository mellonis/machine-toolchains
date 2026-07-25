# Project Manifest — Plan 2: LSP Cross-file Overlay Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make both language servers manifest-aware: an open `.pmc`/`.tmc`
document that belongs to a `pmt.json`/`tmt.json` project target gains
cross-file completion, go-to-definition, hover, (PM-only) semantic tokens,
and the undeclared-external diagnostic refinement — all faithful to what
`linker::resolve` does on the same declared set — plus the `.tmc` stdlib
bridge ([#37](https://github.com/mellonis/machine-toolchains/issues/37))
that the overlay's stdlib resolution leg requires. Executes the LSP section
of `docs/superpowers/specs/2026-07-12-project-manifest-and-build-design.md`
(§459–541). Tracker: [#56](https://github.com/mellonis/machine-toolchains/issues/56).
Milestone: *Plan 2 — LSP cross-file overlay* (#56 + #37). One docs
fold-in by maintainer request (2026-07-26): `docs/pmt/fmt.md` gains `.pma`
coverage for parity with `docs/tmt/fmt.md` (Task 18).

**Architecture:** Approach A per the spec — every file keeps today's
per-file pipeline; sibling **exports** merge into an overlay consulted
after local resolution. Each arch crate gets a new `lsp/overlay.rs` twin
(project-view discovery + sibling-export extraction + the `Overlay` symbol
table), mirroring the `config.rs`/`project.rs` per-crate twinning. The one
core change is additive and arch-agnostic: a narrow public
`linker::resolve_names` query API that the faithfulness tests (and any
future tooling) use to ask "which definition does the linker pick" with
provenance. No `LanguageService` trait change, no protocol change, no
schema change.

**Tech Stack:** Rust, `serde_json` only (zero new deps), existing LSP
framework (`crates/core/src/lsp/`), existing per-file analyses
(`analyze_staged` in both crates), fixture tests via per-test temp dirs
(PID + atomic counter, the post-hygiene-sweep pattern).

**Task order:** Core first (Task 1), then PM-1 (Tasks 2–10, a translation
of the spec), then TM-1 (Tasks 11–17, executing the design pass in the
next section — #37 first, it is the stdlib leg the TM overlay resolves
through), then docs (Task 18). PM before TM mirrors how every twinned
subsystem landed.

## Global Constraints

- **Zero new dependencies** — `serde/serde_json` runtime, `proptest` dev-only. No tempfile, no clap.
- **Thin-renderer rule** — library code never prints. (LSP services return values through the framework; nothing here touches `cli/` rendering except the behavior-neutral refactor in Tasks 2/14.)
- **No `LanguageService` trait change, no capability change** — every feature lands inside the existing 15 trait methods; `watched_globs` already covers `**/pmt.json` / `**/tmt.json`.
- **Core stays arch-agnostic** — Task 1's `resolve_names` knows no opcodes and no manifest; core gains zero manifest knowledge.
- **PM-1 byte-identity** — nothing here touches codegen, the assembler, or the optimizer. `-O0` bit-identity is untouched.
- **CLI behavior is untouched** — Tasks 2/14 refactor `refine_reports` internals with pinned tests proving byte-identical driver output.
- **Path identity is lexical** — mirror the CLI: `normalize_rel` + join, no `canonicalize`, symlink aliases not detected, documented not solved (spec §58 and the ruling below).
- **Published docs** (README, `docs/`) are forge-agnostic and ref-free. Code comments cite durable pages by page + parenthetical keyword (`docs/lsp.md (project overlay)`); never a `docs/superpowers/` path or an issue number.
- **Version spaces**: NOTHING moves — no language, dialect, IR, container, or manifest-schema bump. LSP behavior is additive; crates bump at the release cut.
- **Quality gates**: `cargo clippy --workspace --all-targets -- -D warnings` and `cargo fmt --check` at every commit.
- **Commit style**: conventional commits with scope (`feat(core):`, `feat(post-machine):`, `feat(turing-machine):`, `test(...)`, `docs:`).
- **Commit permission**: the user's standing rule forbids commits without explicit permission. At execution start, ask for blanket per-task commit permission; if not granted, skip every commit step and stop after each task for review.

## Verified starting state (master `40c2f89`, recon 2026-07-26)

Facts every task depends on; re-verify only if master moves.

**Project loader (both crates, all `pub(crate)`, twins):**
- PM `crates/post-machine/src/project.rs`: `Manifest` (:16, fields `stdlib: bool`, `sources`, `libraries`, `profiles`, `targets: BTreeMap<String, Target>`), `Target` (:48), `Libraries { dirs, link }` (:28), `Manifest::effective_sources` (:113, raw strings, project ++ target, order preserved), `effective_libraries` (:121), `normalize_rel` (:177, lexical only; leading `..` kept; absolute rejected), `load_file(path) -> Result<PmtFile, ConfigError>` (:510, the ONE loader — validates both sections), `PmtFile { allow, manifest: Option<Manifest> }` (:505), `discover_manifest(start)` (:568 — nearest ancestor `pmt.json` **with** a `project` key; a malformed candidate on the walk is an **error, not a skip**, pinned at :619).
- TM `crates/turing-machine/src/project.rs`: same names — `Manifest` :22 (adds `call_mech`), `Target` :61, `normalize_rel` :224, `load_file` :548, `TmtFile` :540, `discover_manifest` :610. `call-mech` is link-lowering only — **irrelevant to name resolution and to this plan**.
- **No "which targets contain file X" query exists anywhere** — this plan adds it (LSP-side, not in `project.rs`).

**PM LSP (`crates/post-machine/src/lsp/`):**
- `PmcLanguageService { docs: HashMap<String, DocState>, ide_allow, config_cache }` mod.rs:33-40; `DocState { text, tokens, cst, analysis, lint, fatal, scopes_for_completion, config_errors }` mod.rs:140-163; `uri_to_path` mod.rs:169 (percent-decode only, no canonicalize; non-`file:` → None).
- The config cache pattern to mirror: `config_cache: HashMap<PathBuf, (SystemTime, Result<Vec<String>, String>)>`, `CONFIG_CACHE_LIMIT = 32` (mod.rs:76), mtime hit test + arbitrary-entry eviction + **no stat → no cache entry**, in `ConfigResolver::project_allow` mod.rs:84-109. Documented invariant (mod.rs:70-75): a miss may only cost a re-parse, never change an answer. Discovery itself is deliberately NOT cached (mod.rs:445-448).
- `invalid-config` channel: `merged_diagnostics` mod.rs:268-282 — code `Some("invalid-config")`, `Span::point(1, 1)`, emitted first, sourced from `DocState.config_errors`. A malformed `project` section **already** surfaces here (config::load delegates to `project::load_file`, which validates both sections).
- Watch plumbing: `watched_globs ["**/pmt.json"]` mod.rs:439; core's `didChangeWatchedFiles` handler discards the event payload and calls `republish_all` (`crates/core/src/lsp/server.rs:582-587`, :693) — **the service never learns which file changed**; invalidation must be mtime-self-checking inside `did_update`.
- `analyze_staged(source) -> StagedAnalysis { tokens, cst, analysis, fatal }` compiler.rs:438; `Analysis { ast: Program, scopes, warnings, resolutions: Vec<(Span, Resolution)>, docs: HashMap<String, FnDoc> }` compiler.rs:397-412; `Resolution::{Local{def_name_span}, ImportBinding{use_span, full_path}, QualifiedExternal{full_path}, Unresolved}` compiler.rs:328-337.
- Flattened AST: `Program { functions: Vec<Function>, imports }` parser.rs:33; `Function.exported: bool` parser.rs:80 (un-namespaced `main` auto-exported, parser.rs:1133; **nested functions are never exported**, parser.rs:366-368).
- Feature delegates: `complete::completion` complete.rs:56 (contexts: use-path → `use_roots` :405 / `member_candidates` :362 with the `["std"]` special case :368-381; qualified path → `member_candidates`; bare call → `call_candidates` :427 with std block (c) at :498-508; command position). `navigate::definition` navigate.rs:44 (`resolve_at` :24 → `resolve_call` :73; `std_target` :114; `use_path_at` :169). `navigate::hover_target` :215 + doc lookup `analysis.docs.get(&name).or_else(|| stdlib::docs().get(&name))` mod.rs:549-554. `tokens::semantic_tokens` tokens.rs:40; `emit_call_name` :216 emits **nothing** for `Resolution::Unresolved`; `defaultLibrary` iff path starts with `std::`.
- `DefTarget` conversion contract (`crates/core/src/lsp/mod.rs:68-86`): a target URI not open in the DocStore converts spans by char==UTF-16 identity — exact only for ASCII lines. The stdlib guards its own decl lines ASCII (`every_roster_declaration_line_is_ascii`, stdlib/mod.rs:266); sibling user files cannot be guarded — documented caveat, Task 18.
- `.pma` service: `PmaDocState.cst: AsmCst` is total (every text parses); `FuncCst { name, name_span, local, .. }` `crates/core/src/asm/cst.rs:148-154`; `parse_asm_cst_with(source, caps) -> AsmCst` cst.rs:355. `local` is read nowhere in the LSP today (pma/mod.rs:349 rationale comment).

**Undeclared-external + build refinement (both crates, twins):**
- PM warning: compiler.rs:745-754, code `"undeclared-external"`, message ``call to undeclared external `{name}` — declare it with `use {name};` ``, bare calls only, deduped per name program-wide. TM: compiler.rs:2487-2503, message says "reference to", fires for bare `call` AND bare `bind` targets.
- Driver post-filter (PM driver.rs:536/550/560; TM driver.rs:573/587/597): `defined_names` = `SymbolDef::Defined` over objects ++ libraries ("exactly the set `linker::resolve` builds its namespace from"); `undeclared_name` = first-backtick-pair extraction; `refine_reports` retains diagnostics failing `code == "undeclared-external" && defined.contains(name)`. Runs before `-Werror`. Per **target**, name-level, reachability-blind.
- The LSP publishes `undeclared-external` unconditionally today (PM via `Analysis.warnings` → `merged_diagnostics`; TM via `TmcStagedAnalysis.diagnostics` → `DocState.warnings`). **No TM LSP test pins that publication** — Task 14 adds one.

**Linker (`crates/core/src/linker/`):**
- `resolve.rs` is `pub(crate)`: `resolve(objects, libraries, entry) -> Result<Resolved, LinkError>` :62; namespace = `SymbolDef::Defined` only (user dup = error :84-89; libraries first-wins :91-99); `Local` binds intra-object only (:127-134); BFS follows relocations AND bound_calls; **unresolved names error only if reachable** (:197-199); `Resolved.dropped` :201-210. `FuncRef` :16 (no origin index today). No re-exports.
- `link(syntax, objects, libraries, options)` mod.rs:319 runs resolve at :326. Arch wrappers: `post-machine/src/asm/mod.rs:195`, `turing-machine/src/asm/mod.rs:231`.

**Stdlib:**
- PM `stdlib/mod.rs`: `SOURCE` :34, `object()` :36, `RosterEntry { full_path, name_span, decl_line }` :55, `roster()` :67 (11 entries, single-level CST walk of exported `std` functions), `docs()` :105 (`analyze_staged(SOURCE)` docs), `materialized_std_uri()` :185 (writes `<cache_root>/pmt/<CARGO_PKG_VERSION>/std.pmc`, self-healing; `cache_root` = `$XDG_CACHE_HOME` → `$HOME/.cache` unix / `%LOCALAPPDATA%` windows; IO failure → None), private `path_to_file_uri` :140. **Four independent `OnceLock`s** — accessors are lazy and separate, not fused.
- TM `stdlib/mod.rs`: `SOURCE` :23 + `object()` :32 **only**. `std.tmc` = one file, both twins as nested namespaces; **14 exported routines** (10 delimited + 4 bare) = the linkable symbols (drift-guard :88-93); graphs/alphabets export no symbol. 108 `?` doc lines. The `#[cfg(test)]` recursive walk helper `exported_routine_paths()`/`walk()` at :62-86 is the natural promotion seed.
- TM docs analog: `Resolved.docs: HashMap<String, Doc>` compiler.rs:798, keys = `full_name(ns, name)` (`ns.join("::") + "::" + name`, compiler.rs:1152) for **alphabets, routines, and graphs** (28 documented entities); `Doc { paragraphs, attention, deprecated }` parser.rs:508-515, field-identical to PM's `FnDoc`.

**TM LSP (`crates/turing-machine/src/lsp/`):**
- `TmcLanguageService` mod.rs:61; `DocState { text, tokens, cst, program, resolved, warnings, lint, fatal, roster, config_errors }` mod.rs:173; `analyze_staged -> TmcStagedAnalysis` compiler.rs:1077 (stages lex → parse_cst → lower_cst (infallible) → resolve_program; service adds an expand stage for its fatal, mod.rs:560-565).
- Hover/def funnel: `reference_at(program, pos)` navigate.rs:203 → `reference_in_world` :231; call-target arm :326-341 and bind-target arm :284-289 both gate on `resolve_written` (:154-178: exact mangled hit → `use`-bound spelling → same-namespace sibling — **document-local by design**), whose `None` propagates to a null hover/def. Doc lookup in `render` :588-590 reads only the open document's `Resolved.docs`. `Target` enum :40 has **no file dimension**.
- Completion: `classify` context.rs:407 → `candidates` complete.rs:34-118; `importable` :323 (UsePath — document-only by design), `target_names` :205 (routines for call/bind, graphs for graft), `binding_names`/`binding_value` :244/:279, `vector_cell` :129 (per-cell alphabet — resolves through a **local** alphabet by definition).
- Semantic tokens are **purely lexical** (tokens.rs:45) — no resolution tier exists.
- `ResolvedWorld { kind, name, name_span, exported, local, tapes, state_params, states, grafts, binds, entry, calls }` compiler.rs:803-829; `WorldKind::{Machine, Routine, Graph}` :832.

**TM cross-file model (the design-pass facts):**
- **Only exported `routine` worlds and the machine's `main` become `SymbolDef::Defined` MO symbols.** Non-exported routine → `.func … local`. `export graph`/`export alphabet` emit **no symbol** (stdlib/mod.rs:57-61, drift-guarded).
- Cross-object references are **fatal** for grafts (`undefined-graph`, compiler.rs:1690-1697 — "a graft needs the graph's source") and tape alphabets (`UnresolvedAlphabet`, :1637-1657 — "external alphabets are unsupported in 0.1").
- A cross-object `call`/`bind` target **must be argless**: `external-binding-unsupported` (`CompileErrorKind::ExternalBindingUnsupported`, compiler.rs:239, raised in ir.rs:716-723). An argless external call lowers to a plain `Relocation` + `SymbolDef::External` keyed by the flat `::`-joined qualified name. **Every `BoundCall` record is intra-object by construction; the composition engine plays no part in cross-object name resolution.**
- TM resolution is literally the same `resolve::resolve` as PM (reachability additionally follows bound_calls — a reachability delta, not a naming one).

**Misc:** `find_library(name, dirs) -> Result<ObjectFile, String>` is `pub(super)` in each crate's `cli/build.rs` (PM :301, TM :379). `ObjectFile::from_bytes(bytes) -> Result<Self, FormatError>` formats/object.rs:491. `Manifest` and friends may lack `Clone` derives (verify in Task 3 step 0). `docs/lsp.md:207` under-reports the `.tmc` warning channels (omits `undeclared-external`) and :530-533 records the TM no-materialization gap this plan closes.

---

## Design rulings (the TM-1 pass the spec deferred, plus gaps the spec left open)

The spec (§461–471, amended 2026-07-21) explicitly defers the TM-1 overlay
design "to when plan 2 is written". This section IS that design pass, plus
rulings on under-specified corners. **These rulings govern the tasks below;
do not re-derive them mid-execution.**

**R1 — What the TM overlay resolves: exactly one name kind.** Routine
symbols — a sibling `.tmc`'s exported routines plus its machine's `main`;
a sibling `.tma`'s non-`local` `.func` names; a `.tmo`'s `SymbolDef::Defined`
symbols name-only; declared libraries first-wins; the stdlib's 14 routines.
Graphs, alphabets, and a sibling's tape/state parameters are **out** —
offering them would complete code the compiler rejects (`undefined-graph`,
`UnresolvedAlphabet`, `external-binding-unsupported`). This is narrower
than PM by the language's own rules, not by implementation laziness.

**R2 — What lights up TM-side.** `use`-path completion (roots + namespace
members), call/bind **target** completion (argless positions), go-to-def
into sibling `.tmc`/`.tma`, hover docs from the sibling's `Resolved.docs`,
and the undeclared-external refinement. **Not** semantic tokens: TM's
token layer is purely lexical, and adding a resolution tier to it is a
separate feature, out of this round (documented divergence from PM, where
the token win falls out of the existing resolution table for free). **Not**
binding-name/binding-value/map contexts, graft targets, or vector cells
(R1). TM navigation/hover overlay coverage is call/bind target spans (the
#37 evidence case is a call target); `use`-path hover/def stays local
TM-side this round — PM keeps it because `use_path_at` already exists there.

**R3 — External-target plumbing TM-side.** `navigate::Target` gains an
`External { path: String }` variant. In `reference_in_world`'s call-target
and bind-target arms, when `resolve_written` returns `None`, form the full
path (written name if it contains `::`; else through the document's `use`
bindings) and yield `Target::External`. `definition`/`hover` route
`External` through the overlay, then the stdlib. #37 is exactly the
stdlib leg of this path with an empty overlay.

**R4 — The faithfulness contract, corrected.** The spec's phrasing ("every
call the overlay leaves unresolved must be one the linker also fails to
resolve") is unsatisfiable as stated: `linker::resolve` errors only on
**reachable** unresolved names, and dropped functions may reference
anything. Contract as pinned by the tests (both toolchains): *restricted
to call sites reachable from the fixture entry*, every name the overlay
resolves points at the same definition (same file/object, same symbol,
provenance-compared) that `resolve` picks, and every reachable name the
overlay leaves unresolved is one `resolve` reports unresolved. Fixtures
keep everything reachable from `main` so the reachability blind spot never
discriminates. Deliberately outside the contract: unreachable code,
`MissingSignature`, mapping legality, everything the composition engine
does (none of it is name resolution), and the driver's name-level
refinement (which is reachability-blind by design and stays so).

**R5 — Provenance needs a narrow core API.** `linker::resolve` and its
types are `pub(crate)`; `link()`'s outputs carry names but not which
object won (shadowing cases are indistinguishable by name). Task 1 adds
`pub fn resolve_names(objects, libraries, entry) -> Result<ResolvedNames, LinkError>`
returning owned names + `SymbolOrigin` provenance + the dropped list.
Additive, arch-agnostic, documented in `docs/core.md` (one paragraph).

**R6 — Assembly services participate as export *sources* only.** The
`.pma`/`.tma` services' own query surfaces stay single-file this round —
the spec's "what lights up" list is the compiled-language services'. File
a follow-up issue after merge for assembly-side overlay features
(callable-operand cross-file navigation/completion); do not build them here.

**R7 — Path identity is lexical, mirroring the CLI.** Membership compares
`uri_to_path(uri)` (percent-decoded, not canonicalized) against
`manifest_dir.join(normalize_rel(raw))`. Symlink aliases and non-canonical
spellings silently miss — same posture as the CLI ("documented not
solved"), written down in `docs/lsp.md` (Task 18).

**R8 — Sibling text sourcing.** A sibling open in the **same service** is
read from its live `DocState` (no recompute — its analysis is already
current). A sibling opened in the *other* service (a `.pma` open while a
`.pmc` builds its overlay) or unopened is read from disk, mtime-cached.
Cross-service text sharing would be a structural change out of proportion;
the staleness window (until save) is documented. Also documented: a
sibling's *unsaved* edits propagate into an open document's overlay only
on that document's next `did_update` (keystroke, config event, or watch
event) — core republishes on watch/config events only.

**R9 — Refinement helper unification.** The name-extraction + retain logic
moves from each `cli/driver.rs` into a `pub(crate) fn refine_undeclared`
next to the warning's producer in each `compiler.rs`; the driver delegates.
Behavior-neutral (pinned by the existing driver tests); gives the LSP the
same single source of truth the CLI uses.

**R10 — `"stdlib": false` gates the editor too (both services).** The
project view carries the manifest's `stdlib` flag; when an open document
is a project member and the flag is false, `std::` completion, hover,
navigation (the materialized jump), and the stdlib contribution to the
refinement set all switch off. Documents in no project keep today's
unconditional stdlib surface. (This touches PM paths that predate this
plan — it is spec §509-510, not scope creep.)

**R11 — Project-walk errors degrade, single file view stays.** A manifest
that fails to load during the overlay's project walk yields no project
view (single-file behavior) — the parse error already reaches the user
through the existing `invalid-config` channel, because `config::load`
validates the whole file. A declared library that fails to load
contributes nothing (robustness over completeness — `build` errors there,
the editor must keep working); documented.

**R12 — TM stdlib bridge shape (#37).** One `OnceLock` holding
`(Vec<RosterEntry>, HashMap<String, Doc>)` built from a single
`analyze_staged(SOURCE)` pass over `Resolved.worlds` + `Resolved.docs`,
behind two accessors `roster()` / `docs()`. This deviates from PM's four
independent `OnceLock`s deliberately: TM's roster source IS the resolved
module (PM's roster deliberately avoids the compile pipeline; TM has no
cheaper source with spans). Roster = the 14 exported routines
(navigation/completion surface); docs = all 28 documented entities
(hover surface — a `std::` graph or alphabet name in a `use` line can
hover meaningfully even though it can never link). Materialization
mirrors PM under `<cache_root>/tmt/<CARGO_PKG_VERSION>/std.tmc`, with the
ASCII-line guard test replicated.

**R13 — Overlay resolution order (both toolchains, = spec §507-510).**
Local file first (unchanged, unfiltered — a file's own non-exported names
stay callable locally), then declared sources of the union of containing
targets (exported symbols only, target iteration in `BTreeMap` order, each
target's effective source order, first insertion wins, self excluded),
then declared libraries (first-wins), then the stdlib unless gated by R10.
Duplicate definitions across user sources are a *link* error the overlay
does not diagnose this round; its first-wins lookup is only reached in
fixtures that avoid the duplicate case.

---

### Task 1: Core `linker::resolve_names` query API

**Files:**
- Modify: `crates/core/src/linker/resolve.rs` (add `origin` to `FuncRef`, populate it)
- Modify: `crates/core/src/linker/mod.rs` (public API + types)
- Modify: `docs/core.md` (one paragraph, linker section)
- Test: `mod tests` in `crates/core/src/linker/resolve.rs` (or the existing linker test module — follow the file's current test placement)

**Interfaces:**
- Consumes: `resolve::resolve` (pub(crate)), `ObjectFile`, `LinkError`.
- Produces (all `pub`, in `linker/mod.rs`):
  - `enum SymbolOrigin { Object(usize), Library(usize) }` (derive `Debug, Clone, Copy, PartialEq, Eq`)
  - `struct ResolvedName { pub name: String, pub origin: SymbolOrigin }` (derive `Debug, Clone, PartialEq, Eq`)
  - `struct ResolvedNames { pub reached: Vec<ResolvedName>, pub dropped: Vec<String> }` (derive `Debug, Clone, PartialEq, Eq`)
  - `pub fn resolve_names(objects: &[ObjectFile], libraries: &[ObjectFile], entry: &str) -> Result<ResolvedNames, LinkError>`

- [ ] **Step 1: Record origin on `FuncRef`**

In `resolve.rs`, add a field to `FuncRef` and populate it wherever a
`FuncRef` is constructed from a `Site` (the `(object_index, blob_index)`
pair — object index counts through the objects++libraries concatenation):

```rust
    /// Index of the input that supplied this definition, counting through
    /// the user objects then the libraries — provenance for the
    /// name-resolution query surface (docs/core.md (name resolution)).
    pub(crate) origin: usize,
```

Every construction site sets `origin: site.0`. Run `cargo build -p mtc-core`
until the compiler has walked you to every construction site.

- [ ] **Step 2: Write the failing tests**

In the linker test module, using the same fake-arch object helpers the
existing resolve tests use (crate-private `test_arch`). Cover:

```rust
#[test]
fn resolve_names_reports_reached_with_provenance_and_dropped() {
    // main (object 0) calls lib_fn (library 0); helper in object 0 unreached.
    // reached == [ {main, Object(0)}, {lib_fn, Library(0)} ] (BFS order)
    // dropped == ["helper"]
}

#[test]
fn resolve_names_user_definition_shadows_library() {
    // "dup" defined in object 0 AND library 0; main calls dup.
    // The dup entry's origin must be SymbolOrigin::Object(0).
}

#[test]
fn resolve_names_reachable_unresolved_is_an_error() {
    // main references "ghost" defined nowhere → Err(LinkError::Unresolved-…)
    // (assert on the same LinkError variant resolve/link produce today).
}

#[test]
fn resolve_names_dead_code_may_be_broken() {
    // unreached fn references "ghost" → Ok, ghost never mentioned,
    // the broken fn appears in dropped.
}
```

- [ ] **Step 3: Run tests to verify they fail**

Run: `cargo test -p mtc-core resolve_names`
Expected: FAIL — `resolve_names` not found.

- [ ] **Step 4: Implement `resolve_names`**

In `linker/mod.rs`:

```rust
/// Which input supplied a winning definition: index into the user-object
/// list or the library list as passed to [`resolve_names`] / [`link`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SymbolOrigin {
    Object(usize),
    Library(usize),
}

/// One reached function: its linked symbol name and where its winning
/// definition came from (docs/core.md (name resolution)).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedName {
    pub name: String,
    pub origin: SymbolOrigin,
}

/// The linker's name-resolution answer, without layout: which symbols the
/// reachability BFS reaches (in BFS order, with provenance) and which
/// winning definitions it drops. This is the query surface editor tooling
/// compares itself against.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedNames {
    pub reached: Vec<ResolvedName>,
    pub dropped: Vec<String>,
}

pub fn resolve_names(
    objects: &[ObjectFile],
    libraries: &[ObjectFile],
    entry: &str,
) -> Result<ResolvedNames, LinkError> {
    let resolved = resolve::resolve(objects, libraries, entry)?;
    let n = objects.len();
    Ok(ResolvedNames {
        reached: resolved
            .order
            .iter()
            .map(|f| ResolvedName {
                name: f.name.clone().into_owned(),
                origin: if f.origin < n {
                    SymbolOrigin::Object(f.origin)
                } else {
                    SymbolOrigin::Library(f.origin - n)
                },
            })
            .collect(),
        dropped: resolved.dropped.clone(),
    })
}
```

(Adjust the `Cow` conversion to the real field type; `Resolved.order` /
`.dropped` names are from resolve.rs:51.)

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p mtc-core resolve_names && cargo test -p mtc-core --lib`
Expected: PASS, and every pre-existing linker test still green.

- [ ] **Step 6: Document in `docs/core.md`**

Add one ref-free paragraph to the linker section: the name-resolution
query surface — what `resolve_names` answers (post-shadowing namespace,
BFS reachability, provenance), that it shares the exact code path `link`
runs, and that layout/relaxation are not involved.

- [ ] **Step 7: Quality gates + commit**

Run: `cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --check`

```bash
git add crates/core/src/linker/ docs/core.md
git commit -m "feat(core): resolve_names — public name-resolution query with provenance"
```

---

### Task 2: PM refinement helper moves next to its producer

**Files:**
- Modify: `crates/post-machine/src/compiler.rs` (add `undeclared_name` + `refine_undeclared`)
- Modify: `crates/post-machine/src/cli/driver.rs` (delegate; delete the local copies)
- Test: existing driver tests must stay green unchanged; add one unit test in `compiler.rs`

**Interfaces:**
- Produces (`pub(crate)`, in `compiler.rs`, next to the warning producer at :745):
  - `fn undeclared_name(message: &str) -> Option<&str>` (moved verbatim from driver.rs:550-554)
  - `fn refine_undeclared(diags: &mut Vec<Diagnostic>, defined: &HashSet<String>)`

- [ ] **Step 1: Move + wrap**

In `compiler.rs`, immediately after the warning-producing code:

```rust
/// The build driver and the language server refine this warning the same
/// way wherever a full link set is declared: a bare call the declared set
/// defines stops warning (docs/pmt/cli.md (undeclared-external)).
pub(crate) fn undeclared_name(message: &str) -> Option<&str> {
    // …verbatim body from cli/driver.rs…
}

pub(crate) fn refine_undeclared(
    diags: &mut Vec<Diagnostic>,
    defined: &std::collections::HashSet<String>,
) {
    diags.retain(|d| {
        !(d.code == "undeclared-external"
            && undeclared_name(&d.message).is_some_and(|n| defined.contains(n)))
    });
}
```

In `driver.rs`: delete the local `undeclared_name`; `refine_reports` body
becomes a loop calling `crate::compiler::refine_undeclared(&mut report.diagnostics, defined)`.
Keep `refine_reports` and `defined_names` in the driver (they are
report/object-shaped). The pinned test
`refinement_name_extraction_matches_the_compiler_format` (driver.rs:577)
moves to `compiler.rs`'s test module (it pins the compiler's own format now).

- [ ] **Step 2: Add the unit test**

```rust
#[test]
fn refine_undeclared_drops_only_defined_undeclared_externals() {
    // three diagnostics: undeclared-external `a` (defined), undeclared-external
    // `b` (not defined), some other code mentioning `a` — only the first is dropped.
}
```

- [ ] **Step 3: Verify behavior-neutral**

Run: `cargo test -p mtc-post-machine`
Expected: PASS — including every existing `build_driver`/`cli_programs` test, unchanged.

- [ ] **Step 4: Commit**

```bash
git commit -m "polish(post-machine): refine_undeclared lives beside its producer"
```

---

### Task 3: PM project view — discovery, membership, caches

**Files:**
- Create: `crates/post-machine/src/lsp/overlay.rs` (declare `mod overlay;` in `lsp/mod.rs`)
- Modify: `crates/post-machine/src/project.rs` (derive `Clone` where missing)
- Modify: `crates/post-machine/src/cli/build.rs` (`find_library` → `pub(crate)`)
- Test: `mod tests` in `overlay.rs`

**Interfaces:**
- Consumes: `crate::project::{load_file, PmtFile, Manifest, Target, normalize_rel}`, `super::uri_to_path`.
- Produces (`pub(super)` in `lsp/overlay.rs`):
  - `struct ProjectView { root: PathBuf, stdlib: bool, siblings: Vec<PathBuf>, library_paths: Vec<PathBuf> }`
  - `type ManifestCache = HashMap<PathBuf, (SystemTime, Result<Option<Manifest>, String>)>`
  - `const MANIFEST_CACHE_LIMIT: usize = 32;`
  - `fn project_view(doc_path: &Path, cache: &mut ManifestCache) -> Option<ProjectView>`

- [ ] **Step 0: Verify `Clone` derives on the manifest types**

Run: `grep -n "derive" crates/post-machine/src/project.rs | head -20`
If `Manifest` / `Target` / `Libraries` / `Profiles` / `ProfileOverrides` /
`RunSpec` lack `Clone`, add it (data-only structs; nothing else changes).
Same check rides Task 13 for the TM crate.

- [ ] **Step 1: Write the failing tests**

Fixture helper local to the test module (per testing conventions — no
shared test-support module), the PID + atomic-counter temp-dir pattern the
driver tests use:

```rust
fn temp_tree() -> PathBuf {
    static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let d = std::env::temp_dir().join(format!(
        "pmt-overlay-{}-{}",
        std::process::id(),
        N.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&d).unwrap();
    d
}
```

Tests:

```rust
#[test]
fn member_of_one_target_gets_that_targets_files() {
    // pmt.json: project { sources: ["shared.pmc"], targets: { app: { sources: ["app.pmc"] } } }
    // view for app.pmc: siblings == [<root>/shared.pmc]  (self excluded)
    // stdlib == true, root == manifest dir
}

#[test]
fn member_of_two_targets_gets_the_union_in_target_order() {
    // targets a { sources: [x.pmc, common.pmc] }, b { sources: [x.pmc, extra.pmc] }
    // view for x.pmc: siblings == [common.pmc, extra.pmc] (BTreeMap order a,b; deduped)
}

#[test]
fn non_member_and_no_manifest_yield_none() {
    // a file listed in no target → None; a dir with no pmt.json anywhere → None
}

#[test]
fn lint_only_pmt_json_is_transparent_to_the_walk() {
    // root/pmt.json has project; root/sub/pmt.json is lint-only;
    // root/sub/deep.pmc listed in a target via "sub/deep.pmc" → view found at root.
}

#[test]
fn malformed_manifest_on_the_walk_yields_none_not_a_nearer_hit() {
    // root/pmt.json valid; root/sub/pmt.json is invalid JSON;
    // view for root/sub/x.pmc → None (mirror discover_manifest: error, not skip;
    // the parse error reaches the user via the existing invalid-config channel).
}

#[test]
fn dotdot_membership_resolves_lexically() {
    // manifest at root/proj/pmt.json listing "../shared.pmc";
    // view for root/shared.pmc: NOT found when discovery starts at root
    // (no ancestor manifest names it from there — the walk starts at the
    // document's own directory); found for root/proj/app.pmc with
    // siblings == [root/shared.pmc].
}

#[test]
fn manifest_cache_is_mtime_keyed_and_bounded() {
    // same shape as config_cache_stays_bounded_across_many_distinct_project_roots
    // (lsp/mod.rs:1154): >32 distinct manifest paths → len() stays <= 32;
    // rewriting a manifest with a bumped mtime changes the next answer.
}

#[test]
fn stdlib_false_is_carried() { /* "stdlib": false → view.stdlib == false */ }

#[test]
fn declared_library_paths_resolve_first_wins_and_missing_are_skipped() {
    // libraries { dirs: ["libs", "more"], link: ["bit", "ghost"] } with
    // libs/bit.pmo present, ghost nowhere → library_paths == [<root>/libs/bit.pmo]
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p mtc-post-machine overlay::`
Expected: FAIL — module/type not found.

- [ ] **Step 3: Implement**

`project_view` shape (the walk mirrors `project::discover_manifest`
semantics exactly, with the per-candidate parse going through the bounded
mtime cache — the same discipline as `ConfigResolver::project_allow`:
stat first, no stat → no cache entry, arbitrary eviction only when
inserting a new key at capacity):

```rust
pub(super) fn project_view(doc_path: &Path, cache: &mut ManifestCache) -> Option<ProjectView> {
    let start = doc_path.parent()?;
    let abs = std::path::absolute(start).ok()?;
    // Ancestor walk — nearest pmt.json WITH a project section wins; a
    // malformed candidate ends the walk (its error already reaches the
    // user through the invalid-config channel; docs/lsp.md (project overlay)).
    let mut dir = Some(abs.as_path());
    let (root, manifest) = loop {
        let d = dir?;
        let candidate = d.join("pmt.json");
        if candidate.is_file() {
            match cached_manifest(&candidate, cache) {
                Err(_) => return None,           // R11: degrade to single-file
                Ok(Some(m)) => break (d.to_path_buf(), m),
                Ok(None) => {}                   // lint-only: transparent
            }
        }
        dir = d.parent();
    };
    // Membership + union (R13): BTreeMap target order, effective order
    // within a target, first-seen dedup, self excluded.
    let doc_abs = std::path::absolute(doc_path).ok()?;
    let resolve = |raw: &str| crate::project::normalize_rel(raw).ok().map(|p| root.join(p));
    let mut siblings = Vec::new();
    let mut lib = crate::project::Libraries::default();
    let mut member = false;
    for target in manifest.targets.values() {
        let sources: Vec<_> = manifest
            .effective_sources(target)
            .iter()
            .filter_map(|raw| resolve(raw))
            .collect();
        if !sources.iter().any(|p| *p == doc_abs) {
            continue;
        }
        member = true;
        for p in sources {
            if p != doc_abs && !siblings.contains(&p) {
                siblings.push(p);
            }
        }
        let l = manifest.effective_libraries(target);
        for d in &l.dirs {
            if !lib.dirs.contains(d) {
                lib.dirs.push(d.clone());
            }
        }
        for n in &l.link {
            if !lib.link.contains(n) {
                lib.link.push(n.clone());
            }
        }
    }
    if !member {
        return None;
    }
    let dirs: Vec<String> = lib.dirs.iter()
        .filter_map(|d| resolve(d).map(|p| p.to_string_lossy().into_owned()))
        .collect();
    let library_paths = lib.link.iter()
        .filter_map(|name| {
            // First-wins over dirs; a missing library contributes nothing (R11).
            dirs.iter().map(|d| Path::new(d).join(format!("{name}.pmo")))
                .find(|p| p.is_file())
        })
        .collect();
    Some(ProjectView { root, stdlib: manifest.stdlib, siblings, library_paths })
}
```

`cached_manifest(path, cache) -> Result<Option<Manifest>, String>` is the
mtime-cache read/insert around `crate::project::load_file(path)` mapping
`PmtFile.manifest`. Note the walk-error arm returns the *whole view* as
`None` — single-file behavior — because the same load error already lands
on the document as `invalid-config` through the existing config channel.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p mtc-post-machine overlay::`
Expected: PASS.

- [ ] **Step 5: Make `find_library` reachable later (one word)**

`crates/post-machine/src/cli/build.rs:301`: `pub(super)` → `pub(crate)`.
(Task 4 does not call it — the overlay resolves library paths itself in
`project_view` — but the faithfulness test in Task 9 reuses it to load
libraries the way the driver does. If Task 9 ends up not needing it,
revert this step there.)

- [ ] **Step 6: Quality gates + commit**

```bash
git commit -m "feat(post-machine): LSP project view — manifest discovery, membership, caches"
```

---

### Task 4: PM sibling exports + the `Overlay` table

**Files:**
- Modify: `crates/post-machine/src/lsp/overlay.rs` (the export extraction + `Overlay`)
- Modify: `crates/post-machine/src/lsp/mod.rs` (`DocState.overlay`, service cache fields, build in `did_update`)
- Modify: `crates/post-machine/src/stdlib/mod.rs` (`path_to_file_uri` → `pub(crate)`)
- Test: `mod tests` in `overlay.rs` + one `did_update` test in `mod.rs`

**Interfaces:**
- Consumes: Task 3's `ProjectView`; `crate::compiler::{analyze_staged, Analysis}`; `mtc_core::asm::{parse_asm_cst_with}` + `crate::asm::pm1_syntax` (caps for `.pma`); `mtc_core::formats::object::ObjectFile::from_bytes`; `crate::stdlib::{roster, path_to_file_uri}`.
- Produces (`pub(super)` unless noted):
  - `struct ExportedSym { name: String, span: Option<Span>, doc: Option<FnDoc> }`
  - `type SiblingCache = HashMap<PathBuf, (SystemTime, Vec<ExportedSym>)>`
  - `const SIBLING_CACHE_LIMIT: usize = 64;`
  - `struct OverlaySym { target: Option<(String, Span)>, doc: Option<FnDoc> }` — `target` is `(uri, name_span)`; `None` = name-only (`.pmo`)
  - `struct Overlay { stdlib: bool, symbols: HashMap<String, OverlaySym>, members: HashMap<Vec<String>, BTreeMap<String, String>> }` — `members`: namespace path → bare name → full name; top-level exports live under the empty path
  - `impl Overlay { fn defined_names(&self) -> HashSet<String> }` — symbol keys ∪ (stdlib roster full paths when `self.stdlib`) — the exact mirror of the driver's `defined_names` per declared set
  - `fn build_overlay(view: &ProjectView, doc_path: &Path, open_docs: &HashMap<String, DocState>, cache: &mut SiblingCache) -> Overlay`
- `DocState` gains `pub overlay: Option<Overlay>`; `PmcLanguageService` gains `manifest_cache: ManifestCache, sibling_cache: SiblingCache`.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn pmc_sibling_contributes_exported_functions_only() {
    // sibling: export fn top {…}  fn hidden {…}  namespace ns { export fn inner {…} }
    // plus an un-namespaced main (auto-exported).
    // exports: "top", "main", "ns::inner" — never "hidden"; spans + docs carried.
}

#[test]
fn pma_sibling_contributes_non_local_funcs() {
    // .func pub_one / .func priv_one local → only pub_one, span carried, doc None.
}

#[test]
fn pmo_sibling_and_libraries_contribute_names_only() {
    // compile+assemble a tiny object in-code, write it, list it as a source /
    // resolve as library → OverlaySym.target == None.
}

#[test]
fn resolution_order_is_sources_then_libraries_first_wins() {
    // "ns::dup" exported by sibling source AND defined in a linked library →
    // overlay.symbols["ns::dup"].target points at the sibling source URI.
}

#[test]
fn broken_sibling_contributes_nothing_others_still_do() {
    // sibling A has a parse error, sibling B is fine → B's exports present, no A.
}

#[test]
fn open_sibling_is_read_from_its_doc_state_not_disk() {
    // disk copy exports `old`; open_docs carries the same path with `new` →
    // overlay has "new", not "old".
}

#[test]
fn sibling_cache_is_mtime_keyed_and_bounded() { /* same shape as Task 3's */ }
```

- [ ] **Step 2: Run tests to verify they fail** (`cargo test -p mtc-post-machine overlay::`)

- [ ] **Step 3: Implement extraction + build**

```rust
fn exports_from_pmc(analysis: &Analysis) -> Vec<ExportedSym> {
    analysis.ast.functions.iter()
        .filter(|f| f.exported)
        .map(|f| ExportedSym {
            name: f.name.clone(),                 // mangled full name
            span: Some(f.name_span),
            doc: analysis.docs.get(&f.name).cloned(),
        })
        .collect()
}

fn exports_from_pma(cst: &mtc_core::asm::AsmCst) -> Vec<ExportedSym> {
    cst.items.iter()
        .filter_map(|i| match &i.kind {
            mtc_core::asm::AsmItemKind::Func(f) if !f.local => Some(ExportedSym {
                name: f.name.clone(), span: Some(f.name_span), doc: None,
            }),
            _ => None,
        })
        .collect()
}

fn exports_from_object(bytes: &[u8]) -> Vec<ExportedSym> {
    let Ok(obj) = ObjectFile::from_bytes(bytes) else { return Vec::new() };
    obj.symbols.iter()
        .filter(|s| matches!(s.def, SymbolDef::Defined { .. }))
        .map(|s| ExportedSym { name: s.name.clone(), span: None, doc: None })
        .collect()
}
```

`build_overlay` walks `view.siblings` in order, then `view.library_paths`:
per sibling, dispatch on extension (`.pmc` → open-doc `DocState.analysis`
if `open_docs` holds its `file:` URI (build the URI with
`crate::stdlib::path_to_file_uri`), else disk text + `analyze_staged`,
mtime-cached; `.pma` → `parse_asm_cst_with(text, crate::asm::pm1_syntax().caps)`;
`.pmo` → `exports_from_object`); insert with `entry(...).or_insert(...)`
(first wins, R13) into `symbols`, and register into `members` by splitting
the name on `"::"` (last segment = bare name, prefix = namespace path;
a name with no `::` lands under the empty path). `Overlay.stdlib = view.stdlib`.

A fatal in a sibling's `analyze_staged` (no `analysis`) → empty export
list, cached under its mtime like any other answer.

- [ ] **Step 4: Wire into `did_update`**

In `PmcLanguageService::did_update`, after the config resolve and BEFORE
inserting the new `DocState` (so `open_docs` reads see the *other*
documents):

```rust
let overlay = uri_to_path(uri).and_then(|p| {
    overlay::project_view(&p, &mut self.manifest_cache)
        .map(|view| overlay::build_overlay(&view, &p, &self.docs, &mut self.sibling_cache))
});
```

Store `overlay` on the new `DocState`. Untitled / non-`file:` URIs get
`None` (single-file view) for free via `uri_to_path`.

- [ ] **Step 5: Run tests + full crate** (`cargo test -p mtc-post-machine`)

- [ ] **Step 6: Commit**

```bash
git commit -m "feat(post-machine): sibling-export extraction and the LSP overlay table"
```

---

### Task 5: PM diagnostics refinement through the overlay

**Files:**
- Modify: `crates/post-machine/src/lsp/mod.rs` (`did_update`)
- Test: `mod tests` in `lsp/mod.rs`

**Interfaces:**
- Consumes: `Overlay::defined_names` (Task 4), `crate::compiler::refine_undeclared` (Task 2).

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn undeclared_external_is_dropped_when_the_overlay_defines_the_name() {
    // project: app.pmc bare-calls `helper`; sibling exports top-level `helper`.
    // did_update(app) diagnostics contain NO "undeclared-external".
}

#[test]
fn undeclared_external_stays_for_names_the_overlay_lacks() { /* bare call `ghost` keeps warning */ }

#[test]
fn single_file_documents_keep_todays_warning() {
    // no manifest anywhere: bare call `helper` still warns — per-file honest.
}

#[test]
fn stdlib_resolved_bare_name_is_not_suppressed() {
    // bare call `goToEnd` in a project: stays warned — the stdlib defines
    // `std::goToEnd`, not `goToEnd`; exactly what the driver does.
}
```

- [ ] **Step 2: Run to verify they fail** — the first test publishes the warning today.

- [ ] **Step 3: Implement**

In `did_update`, after `analyze_staged` and after the overlay is built
(Task 4 wiring), before `DocState` assembly:

```rust
if let (Some(overlay), Some(analysis)) = (overlay.as_ref(), staged.analysis.as_mut()) {
    crate::compiler::refine_undeclared(&mut analysis.warnings, &overlay.defined_names());
}
```

- [ ] **Step 4: Run tests** (`cargo test -p mtc-post-machine lsp::`)

- [ ] **Step 5: Commit**

```bash
git commit -m "feat(post-machine): LSP mirrors build's undeclared-external refinement"
```

---

### Task 6: PM completion overlay legs + stdlib gating

**Files:**
- Modify: `crates/post-machine/src/lsp/complete.rs`
- Modify: `crates/post-machine/src/lsp/mod.rs` (a `std_enabled` helper)
- Test: `mod tests` in `complete.rs` (existing test-module style)

**Interfaces:**
- Consumes: `DocState.overlay` (`symbols`, `members`, `stdlib`).
- Produces: `pub(super) fn std_enabled(state: &DocState) -> bool` in `lsp/mod.rs`:
  `state.overlay.as_ref().map_or(true, |o| o.stdlib)` — single-file docs keep the unconditional stdlib surface (R10).

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn member_completion_unions_overlay_namespace_members() {
    // sibling exports ns::inner; typing `ns::` in app.pmc offers `inner`
    // with detail "ns::inner".
}

#[test]
fn use_path_completion_offers_sibling_roots_and_members() {
    // `use n▮` offers `ns`; `use ns::▮` offers `inner`.
}

#[test]
fn bare_call_position_offers_top_level_sibling_exports_and_qualified_paths() {
    // sibling exports top-level `helper` and ns::inner:
    // call position offers `helper` (bare) and `ns::inner` (qualified label),
    // mirroring the std block's shape.
}

#[test]
fn overlay_deprecated_docs_tag_candidates() {
    // sibling export carries `! [deprecated] use other` → candidate.deprecated == true.
}

#[test]
fn stdlib_false_removes_std_candidates_everywhere() {
    // project with "stdlib": false → no `std` use-root, no std members,
    // no std qualified call candidates. Single-file doc still gets them.
}
```

- [ ] **Step 2: Run to verify they fail**

- [ ] **Step 3: Implement**

- `member_candidates` (complete.rs:362): gate the existing `["std"]` block
  on `std_enabled(state)`; after the local `ScopeSummary` lookup, union
  `overlay.members.get(&path)` (skip names the local map already offered).
- `use_roots` (complete.rs:405): gate the literal `"std"` insertion on
  `std_enabled(state)`; add the first segment of every overlay member path
  (the keys of `members` whose path is non-empty → first component).
- `call_candidates` (complete.rs:427): gate block (c) (std roster) on
  `std_enabled(state)`; add block (d): overlay `members[&[]]` bare names
  (labels = bare name), and every overlay symbol with a `::` path as a
  qualified-label candidate (the std block's shape). `deprecated` from
  `OverlaySym.doc.as_ref().and_then(|d| d.deprecated.as_ref()).is_some()`.
  Skip any label the local roster already produced.

- [ ] **Step 4: Run tests** (`cargo test -p mtc-post-machine complete`)

- [ ] **Step 5: Commit**

```bash
git commit -m "feat(post-machine): cross-file completion through the overlay"
```

---

### Task 7: PM navigation + hover overlay legs

**Files:**
- Modify: `crates/post-machine/src/lsp/navigate.rs`
- Modify: `crates/post-machine/src/lsp/mod.rs` (hover doc chain)
- Test: `mod tests` in `navigate.rs` / `mod.rs`

**Interfaces:**
- Consumes: `DocState.overlay`, `std_enabled` (Task 6).
- Produces: `pub(super) fn text_at_span(text: &str, span: Span) -> Option<&str>` in `navigate.rs` (slices the written name for `Resolution::Unresolved` sites; spans are 1-based, half-open — mirror the span math `merged_diagnostics` and the position mapper already use).

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn definition_jumps_into_a_pmc_sibling() {
    // QualifiedExternal ns::inner → DefTarget { uri: file://…/shared.pmc, span: inner's name_span }.
}

#[test]
fn definition_jumps_into_a_pma_sibling() { /* .pma FuncCst.name_span target */ }

#[test]
fn import_binding_prefers_the_sibling_definition_over_the_use_span() {
    // `use ns::inner;` + call site: definition lands in the sibling, not on the use.
    // With no overlay hit, today's use_span behavior is preserved.
}

#[test]
fn unresolved_bare_call_resolves_through_the_overlay() {
    // bare call `helper`, sibling exports top-level `helper` → sibling DefTarget.
}

#[test]
fn pmo_backed_names_navigate_null() { /* OverlaySym.target == None → definition None */ }

#[test]
fn hover_carries_the_siblings_doc_lines() {
    // sibling's `? Frobs the tape.` on ns::inner → hover text contains "Frobs the tape."
}

#[test]
fn stdlib_false_kills_std_hover_and_the_materialized_jump() {
    // project with "stdlib": false: hover on std::goToEnd → None; definition → None.
    // Single-file doc: both still work.
}
```

- [ ] **Step 2: Run to verify they fail**

- [ ] **Step 3: Implement**

`navigate.rs` — thread `state` (it already has it) and read
`state.overlay`:

- `resolve_call` (:73): in the `ImportBinding` arm, before the `use_span`
  fallback, try `overlay.symbols.get(full_path)` → `Some(target)` →
  `DefTarget { uri, span, origin }`; keep `std_target` for `std::` paths
  but gate it on `std_enabled(state)`. Same overlay leg in
  `QualifiedExternal`. In the `Unresolved` arm (today `None`):
  `text_at_span(&state.text, name_span)` → `overlay.symbols.get(written)`
  (bare top-level exports have their bare name as the symbol key) → target.
- `use_path_at` step (:63-65 in `definition`): non-`std::` joined paths
  also consult the overlay; `std::` stays gated.
- `hover_target` (:215): add a step — when `resolve_at` yields
  `Resolution::Unresolved`, recover the written name via `text_at_span`;
  return `(name, span)` when the overlay resolves it.
- `mod.rs` hover chain (:549-554) becomes:

```rust
let overlay_doc = state.overlay.as_ref().and_then(|o| o.symbols.get(&name)).and_then(|s| s.doc.as_ref());
let std_doc = if std_enabled(state) { crate::stdlib::docs().get(&name) } else { None };
let doc = state.analysis.as_ref()?.docs.get(&name).or(std_doc).or(overlay_doc)?;
```

(Order: local → stdlib → overlay for doc *content* is irrelevant in
practice — the key spaces are disjoint; keep local first, then the two
external legs.)

- [ ] **Step 4: Run tests** (`cargo test -p mtc-post-machine navigate lsp::`)

- [ ] **Step 5: Commit**

```bash
git commit -m "feat(post-machine): cross-file navigation and hover through the overlay"
```

---

### Task 8: PM semantic tokens for overlay-resolved call sites

**Files:**
- Modify: `crates/post-machine/src/lsp/tokens.rs`
- Test: `mod tests` in `tokens.rs`

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn overlay_resolved_bare_call_tokenizes_as_function_without_default_library() {
    // bare call `helper` + sibling top-level export: a `function` token appears
    // at the call span, MODIFIER_DEFAULT_LIBRARY clear. Without the overlay
    // (single file), the site still emits nothing.
}
```

- [ ] **Step 2: Run to verify it fails**

- [ ] **Step 3: Implement**

`emit_call_name` (tokens.rs:216): the `Resolution::Unresolved` arm — today
`return` — first checks
`state.overlay.as_ref().and_then(|o| o.symbols.get(text_at_span(&state.text, span)?))`;
a hit emits `TOKEN_TYPE_FUNCTION` with no modifiers (`defaultLibrary`
stays std-only, spec §522). `tokens::semantic_tokens` already receives
`state`. `QualifiedExternal` sites already tokenize today; no change there.

- [ ] **Step 4: Run tests** (`cargo test -p mtc-post-machine tokens`)

- [ ] **Step 5: Commit**

```bash
git commit -m "feat(post-machine): overlay-resolved call sites emit function tokens"
```

---

### Task 9: PM faithfulness — overlay ≡ `resolve_names`

**Files:**
- Modify: `crates/post-machine/src/lsp/overlay.rs` (`#[cfg(test)] mod faithfulness`)

**Interfaces:**
- Consumes: `mtc_core::linker::{resolve_names, SymbolOrigin}` (Task 1), the service (`did_update` + `DocState`), `crate::compiler::compile`, `crate::asm::{assemble-path via cli helpers or direct}`, `crate::cli::build::find_library` (Task 3 step 5).

- [ ] **Step 1: Build the fixture**

One temp tree, everything reachable from `main` (R4):

```
pmt.json      project { sources: ["shared.pmc"],
                        libraries: { dirs: ["libs"], link: ["bitops"] },
                        targets: { app: { sources: ["app.pmc", "helpers.pma", "pre.pmo"] } } }
shared.pmc    export fn helper …           // bare top-level export
              namespace ns { export fn inner … }
              namespace ns { export fn dup … }   // shadows the library's ns::dup
app.pmc       fn main { call helper; call ns::inner; call ns::dup;
                        call asm_fn; call pre_fn; call std::goToEnd; } // via use std::goToEnd
helpers.pma   .func asm_fn …
pre.pmo       (compiled in-code from a snippet exporting pre_fn)
libs/bitops.pmo  (compiled in-code: exports ns::dup and bit_only)
```

- [ ] **Step 2: Write the failing equivalence test**

```rust
#[test]
fn overlay_resolution_matches_linker_resolution_with_provenance() {
    // 1. Overlay side: did_update(app.pmc from disk); collect, per call site,
    //    the overlay/local pick: Local → "app.pmc"; overlay target uri → its
    //    file; OverlaySym.target == None → the .pmo path by symbol name.
    // 2. Linker side: compile/assemble/load the target's effective sources in
    //    effective order → objects; find_library over the declared dirs +
    //    stdlib::object() → libraries; resolve_names(&objects, &libs, "main").
    // 3. Map SymbolOrigin::Object(i) → the i-th effective source file,
    //    Library(i) → the i-th library. For every reached name that app.pmc
    //    calls, assert the overlay's pick names the SAME file (provenance),
    //    incl. ns::dup landing on shared.pmc, NOT the library.
}

#[test]
fn overlay_unresolved_matches_linker_unresolved() {
    // Second mini-fixture: app bare-calls `ghost` (defined nowhere).
    // Overlay: symbol absent, warning NOT suppressed (Task 5 behavior).
    // Linker: resolve_names errors naming `ghost` (reachable-unresolved).
}
```

- [ ] **Step 3: Run to verify the harness fails** (missing plumbing, then make it pass)

- [ ] **Step 4: Run the whole crate** (`cargo test -p mtc-post-machine`)

- [ ] **Step 5: Commit**

```bash
git commit -m "test(post-machine): overlay-vs-linker faithfulness fixture"
```

---

### Task 10: PM LSP integration fixtures (the spec's matrix)

**Files:**
- Modify: `crates/post-machine/src/lsp/mod.rs` (test module) or `overlay.rs` tests — follow where Task 5's tests landed

- [ ] **Step 1: Write the matrix tests** (spec §617-620; several already
  landed piecemeal in Tasks 3–8 — this task closes the remaining cells,
  each a real `PmcLanguageService` driven through `did_update` +
  feature calls over a temp tree):

```rust
#[test]
fn manifest_edit_changes_the_next_answer() {
    // did_update → sibling completion present; rewrite pmt.json REMOVING the
    // sibling (bump mtime); did_update again → candidate gone. (Core's
    // republish_all delivers the re-update in production; here we call it.)
}

#[test]
fn broken_sibling_degrades_only_itself() { /* if not already pinned in Task 4 */ }

#[test]
fn untitled_documents_keep_the_single_file_view() {
    // uri "untitled:Untitled-1": no overlay, no refinement, std intact.
}

#[test]
fn dotdot_declared_source_gets_exact_features_when_opened_from_the_tree() {
    // the Task 3 dotdot fixture, driven through the service end to end.
}

#[test]
fn stdlib_false_matrix() {
    // one project, one did_update, assert all four gates at once:
    // no std completion root, no std hover, no std definition, and an
    // undeclared-external on a bare std-shaped call stays (nothing new suppressed).
}
```

- [ ] **Step 2: Run** (`cargo test -p mtc-post-machine`)

- [ ] **Step 3: Commit**

```bash
git commit -m "test(post-machine): overlay integration matrix"
```

---

### Task 11: TM stdlib accessors (#37, part 1)

**Files:**
- Modify: `crates/turing-machine/src/stdlib/mod.rs`
- Test: extend the existing `#[cfg(test)]` module there

**Interfaces:**
- Consumes: `crate::compiler::{analyze_staged, Resolved, WorldKind}`, `crate::parser::Doc`.
- Produces (all `pub(crate)`):
  - `struct RosterEntry { full_path: String, name_span: Span }`
  - `fn roster() -> &'static [RosterEntry]` — the 14 exported routines
  - `fn docs() -> &'static HashMap<String, Doc>` — all 28 documented entities
  - `fn materialized_std_uri() -> Option<&'static str>`
  - `fn path_to_file_uri(path: &Path) -> Option<String>` (ported from PM stdlib/mod.rs:140)

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn roster_is_the_fourteen_exported_routines() {
    // same set the existing exported_routine_paths() drift guard pins;
    // spot-check "std::binaryNumbers::goToNumber" has a name_span on its decl line.
}

#[test]
fn docs_cover_routines_graphs_and_alphabets() {
    // docs().get("std::binaryNumbers::goToNumber") has paragraphs;
    // docs().get("std::binaryNumbers::symbols") (alphabet) present;
    // docs().get("std::binaryNumbers::plusOneGraph") (graph) present.
}

#[test]
fn every_roster_declaration_line_is_ascii() { /* ported guard — load-bearing (DefTarget identity conversion) */ }

#[test]
fn materialized_std_uri_points_at_a_byte_identical_std_tmc() {
    // parse the file:// URI back to a path, read it, assert == SOURCE.
}
```

- [ ] **Step 2: Run to verify they fail**

- [ ] **Step 3: Implement**

One `OnceLock` (R12 — deviation from PM's four justified there):

```rust
static ANALYSIS: OnceLock<(Vec<RosterEntry>, HashMap<String, Doc>)> = OnceLock::new();

fn analysis() -> &'static (Vec<RosterEntry>, HashMap<String, Doc>) {
    ANALYSIS.get_or_init(|| {
        let resolved = crate::compiler::analyze_staged(SOURCE)
            .resolved
            .expect("the embedded stdlib always resolves");
        let roster = resolved.worlds.iter()
            .filter(|w| matches!(w.kind, WorldKind::Routine) && w.exported)
            .map(|w| RosterEntry { full_path: w.name.clone(), name_span: w.name_span })
            .collect();
        (roster, resolved.docs)
    })
}
```

`roster()` / `docs()` project the tuple. Retire the `#[cfg(test)]`
`exported_routine_paths` walk in favor of asserting the existing expected
set against `roster()` (the drift guard keeps its teeth). Port
`cache_root` / `path_to_file_uri` / `materialize_into` from PM with the
`"pmt"` path segment → `"tmt"` and `std.pmc` → `std.tmc`.

- [ ] **Step 4: Run tests** (`cargo test -p mtc-turing-machine stdlib`)

- [ ] **Step 5: Commit**

```bash
git commit -m "feat(turing-machine): stdlib roster, docs, and materialization accessors"
```

---

### Task 12: `.tmc` service consumes the stdlib bridge (#37, part 2)

**Files:**
- Modify: `crates/turing-machine/src/lsp/navigate.rs` (`Target::External`, arm fallbacks, render doc fallback)
- Modify: `crates/turing-machine/src/lsp/complete.rs` (`importable`, `target_names`)
- Test: `crates/turing-machine/src/lsp/tests.rs` (house test file)

**Interfaces:**
- Consumes: Task 11's accessors.
- Produces: `Target::External { path: String }` variant (navigate.rs:40 enum); `fn external_path(program: &Program, written: &str) -> Option<String>` — written contains `::` → as-is; else through the document's `use` bindings (alias → full path); else `None`.

- [ ] **Step 1: Write the failing tests** (the #37 evidence case first)

```rust
#[test]
fn hover_on_a_std_call_target_returns_its_doc() {
    // machine calling std::binaryNumbers::goToNumber() (argless, after use or qualified):
    // hover over the target span → HoverContent containing the routine's first paragraph.
    // This is issue #37's exact reproduction, inverted.
}

#[test]
fn definition_on_a_std_call_target_jumps_into_materialized_std_tmc() { /* DefTarget uri + roster span */ }

#[test]
fn use_path_completion_offers_std_and_its_members() {
    // `use s▮` offers `std`; `use std::binaryNumbers::▮` offers the ten routines
    // (routines only — graphs/alphabets are not offered in use paths, R1).
}

#[test]
fn call_target_completion_offers_qualified_std_routines() { /* target_names, Call and Bind kinds */ }

#[test]
fn graft_target_completion_never_offers_std_names() { /* R1: graft targets stay local-only */ }

#[test]
fn undeclared_external_still_published_for_bare_names() {
    // pin the channel Task 14 refines: a bare `call helper()` publishes
    // "undeclared-external" through did_update. (The recon found no test pins this.)
}
```

- [ ] **Step 2: Run to verify they fail**

- [ ] **Step 3: Implement**

- `navigate.rs`: add `External { path: String }` to `Target`. In
  `reference_in_world`'s call-target arm (:326-341) and bind-target arm
  (:284-289): when `resolve_written` yields `None`, try
  `external_path(program, &written)` → `Target::External`. In
  `declaration_span`-driven `definition` (:535): route `External` →
  stdlib roster lookup → `DefTarget { uri: materialized_std_uri()?, span }`
  (overlay leg arrives in Task 15). In `hover`/`render`: `External`
  renders the qualified path as the head line + the doc; the shared doc
  lookup at :588-590 gains `.or_else(|| crate::stdlib::docs().get(key))`.
- `complete.rs`: `importable` (:323) — add the `std` root and, for a
  `std::…` prefix path, the roster members under it (routines only);
  `target_names` (:205) — for `CallKind::Call` / `CallKind::Bind` add the
  14 qualified roster paths; `CallKind::Graft` untouched.

- [ ] **Step 4: Run tests** (`cargo test -p mtc-turing-machine lsp`)

- [ ] **Step 5: Commit** — closes #37's user-visible gap.

```bash
git commit -m "feat(turing-machine): .tmc stdlib bridge — std:: hover, navigation, completion"
```

---

### Task 13: TM project view + overlay table

**Files:**
- Create: `crates/turing-machine/src/lsp/overlay.rs` (declare in `lsp/mod.rs`)
- Modify: `crates/turing-machine/src/project.rs` (Clone derives if missing — Task 3 step 0's check)
- Modify: `crates/turing-machine/src/cli/build.rs` (`find_library` → `pub(crate)`, for Task 16)
- Test: `mod tests` in `overlay.rs`

**Interfaces:** the Task 3 + Task 4 twins with TM divergences:
- `ProjectView` identical (no `call_mech` — link lowering is name-irrelevant, R1 preamble).
- `ExportedSym.doc: Option<Doc>` (TM's doc type).
- `exports_from_tmc(resolved: &Resolved) -> Vec<ExportedSym>`:

```rust
resolved.worlds.iter()
    .filter(|w| match w.kind {
        WorldKind::Machine => true,                    // `.func main`, Defined
        WorldKind::Routine => w.exported,              // Defined iff exported
        WorldKind::Graph => false,                     // no symbol, ever
    })
    .map(|w| ExportedSym {
        name: w.name.clone(),
        span: Some(w.name_span),
        doc: resolved.docs.get(&w.name).cloned(),
    })
    .collect()
```

- `exports_from_tma` walks `AsmCst` items for non-`local` `Func` (same as
  PM's — build the CST with `parse_asm_cst_with(text, crate::asm::tm1_syntax().caps)`).
  A `.routine` signature directive with no `.func` is a *declaration* of an
  external, not a definition — it contributes nothing.
- `.tmo` → `exports_from_object` (same body as PM's).
- Sibling dispatch covers `.tmc` / `.tma` / `.tmo`; manifest walk targets `tmt.json`.

- [ ] **Step 1: Write the failing tests** — port Task 3's matrix (member/union/
  transparent-lint-only/malformed/dotdot/bounded caches/stdlib flag/libraries)
  plus TM-specific:

```rust
#[test]
fn tmc_sibling_contributes_exported_routines_and_main_never_graphs() {
    // sibling: export routine ns::r; routine ns::hidden; export graph ns::g; machine {…}
    // exports == {"ns::r", "main"} — hidden (local) and g (graph) absent.
}

#[test]
fn tma_sibling_contributes_non_local_funcs_only() { /* .func a / .func b local → a */ }
```

- [ ] **Step 2: Run to verify they fail**
- [ ] **Step 3: Implement** (port `overlay.rs` from PM; the file-level doc
  comment states the twin relationship and the divergence list, citing
  `docs/lsp.md (project overlay)`)
- [ ] **Step 4: Wire into `TmcLanguageService::did_update`** exactly as Task 4 step 4 (fields `manifest_cache` + `sibling_cache` + `DocState.overlay`)
- [ ] **Step 5: Run** (`cargo test -p mtc-turing-machine overlay::`)
- [ ] **Step 6: Commit**

```bash
git commit -m "feat(turing-machine): LSP project view and overlay table"
```

---

### Task 14: TM refinement — shared helper + LSP mirror

**Files:**
- Modify: `crates/turing-machine/src/compiler.rs` (`undeclared_name` + `refine_undeclared`, Task 2's twin)
- Modify: `crates/turing-machine/src/cli/driver.rs` (delegate)
- Modify: `crates/turing-machine/src/lsp/mod.rs` (`did_update` refinement)
- Test: `compiler.rs` unit + `lsp/tests.rs`

- [ ] **Step 1: Port Task 2 verbatim** (the pinned-format test moves to
  `compiler.rs`; TM's message says "reference to" — the extractor is
  wording-agnostic, keep the pin against the real emitted message).
- [ ] **Step 2: Write the failing LSP tests** — Task 5's four cases with TM
  spellings (`call helper()` bare; sibling `export routine helper`; the
  stdlib case: bare `call plusOne()` stays warned — the stdlib defines
  `std::binaryNumbersBare::plusOne`, not `plusOne`). Also the **bind**
  variant: `use lib::r; bind r() as h;` with a sibling exporting `lib::r`
  — no warning (qualified/imported never warned anyway); bare `bind ghost() as h;`
  keeps its warning.
- [ ] **Step 3: Implement** — `did_update` calls
  `crate::compiler::refine_undeclared(&mut warnings, &overlay.defined_names())`
  when the overlay exists (TM `defined_names` = symbols ∪ stdlib roster
  paths when `stdlib`, identical to PM's).
- [ ] **Step 4: Run** (`cargo test -p mtc-turing-machine`)
- [ ] **Step 5: Commit**

```bash
git commit -m "feat(turing-machine): LSP mirrors build's undeclared-external refinement"
```

---

### Task 15: TM completion + navigation + hover overlay legs

**Files:**
- Modify: `crates/turing-machine/src/lsp/complete.rs`, `navigate.rs`, `mod.rs` (`std_enabled` twin)
- Test: `lsp/tests.rs`

**Interfaces:**
- Consumes: `DocState.overlay`, `Target::External` (Task 12), Task 13's table.
- Produces: `pub(super) fn std_enabled(state: &DocState) -> bool` (Task 6's twin).

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn use_path_completion_offers_sibling_namespaces_and_routines() { /* importable + overlay members */ }

#[test]
fn call_and_bind_target_completion_offers_sibling_exported_routines() {
    // qualified labels; graft targets and binding-name/value contexts get NOTHING
    // from the overlay (R1/R2) — assert the negative explicitly.
}

#[test]
fn definition_and_hover_reach_a_tmc_sibling() {
    // call target ns::r → DefTarget into the sibling; hover carries its ? lines.
}

#[test]
fn definition_reaches_a_tma_sibling_and_tmo_names_navigate_null() { }

#[test]
fn stdlib_false_gates_std_surfaces_tm_side() { /* Task 6/7's matrix, TM spellings */ }

#[test]
fn semantic_tokens_are_unchanged_by_the_overlay() {
    // same document, overlay present vs absent → identical token stream (R2).
}
```

- [ ] **Step 2: Run to verify they fail**
- [ ] **Step 3: Implement**

- `navigate.rs`: `Target::External` resolution order becomes overlay →
  stdlib (both `definition` and the hover doc lookup): overlay hit with
  `target: Some((uri, span))` → sibling `DefTarget`; `None` target →
  navigation null, doc still served from `OverlaySym.doc`. Stdlib legs
  gain the `std_enabled` gate.
- `complete.rs`: `importable` unions overlay member roots/members
  (routines only by construction — the table never holds anything else);
  `target_names` (Call/Bind) adds overlay qualified paths; deprecation
  tags from `OverlaySym.doc.deprecated`.

- [ ] **Step 4: Run** (`cargo test -p mtc-turing-machine lsp`)
- [ ] **Step 5: Commit**

```bash
git commit -m "feat(turing-machine): cross-file completion, navigation, and hover"
```

---

### Task 16: TM faithfulness — overlay ≡ `resolve_names`

**Files:**
- Modify: `crates/turing-machine/src/lsp/overlay.rs` (`#[cfg(test)] mod faithfulness`)

- [ ] **Step 1: Build the fixture** (Task 9's twin, TM spellings; everything
  reachable from `main`; all cross-object calls **argless**):

```
tmt.json      project { sources: ["shared.tmc"],
                        libraries: { dirs: ["libs"], link: ["bit"] },
                        targets: { app: { sources: ["app.tmc", "helpers.tma", "pre.tmo"] } } }
shared.tmc    export routine helper(t: a) …; namespace ns { export routine dup(…) …; }
app.tmc       machine { … call helper() …; call ns::dup() …; call asm_fn() …;
                        call pre_fn() …; call std::binaryNumbersBare::plusOne() … }
helpers.tma   .routine asm_fn, tapes=1, alpha=(2)  +  .func asm_fn …
libs/bit.tmo  (compiled in-code: exports ns::dup and bit_only)
```

- [ ] **Step 2: Write the two equivalence tests** (same assertions as Task 9:
  provenance match per reachable call incl. the `ns::dup` shadow landing on
  `shared.tmc`; the `ghost` mini-fixture erroring in `resolve_names` while
  the overlay leaves it unresolved and unsuppressed).
- [ ] **Step 3: Make them pass**; run `cargo test -p mtc-turing-machine`
- [ ] **Step 4: Commit**

```bash
git commit -m "test(turing-machine): overlay-vs-linker faithfulness fixture"
```

---

### Task 17: TM LSP integration matrix

**Files:**
- Modify: `crates/turing-machine/src/lsp/tests.rs`

- [ ] **Step 1: Port Task 10's matrix** (manifest-edit re-answer,
  broken-sibling degradation, untitled fallback, `../` membership,
  `stdlib: false` all-gates) with TM spellings, plus the TM-only negative:

```rust
#[test]
fn overlay_never_leaks_into_binding_or_vector_contexts() {
    // a sibling's routine/tape names absent from BindingName / BindingValue /
    // MapSrc / MapDst / VectorCell candidates — completing them would write
    // external-binding-unsupported / unresolved-alphabet code (R1).
}
```

- [ ] **Step 2: Run** (`cargo test -p mtc-turing-machine`)
- [ ] **Step 3: Commit**

```bash
git commit -m "test(turing-machine): overlay integration matrix"
```

---

### Task 18: Docs — `docs/lsp.md` cross-file section, corrections, fmt-page parity

**Files:**
- Modify: `docs/lsp.md`
- Modify: `docs/pmt/fmt.md` (`.pma` parity with `docs/tmt/fmt.md` — maintainer fold-in, 2026-07-26)
- Modify: `CLAUDE.md` (current-state paragraph; internal, links allowed)

- [ ] **Step 1: Write the cross-file section** (ref-free, both toolchains):

Structure — new top-level section "Cross-file resolution (the project
overlay)":
- Membership: the open document's directory walks to the nearest project
  file *with* a `project` section; membership = the normalized path is in
  a target's effective sources; the overlay set is the union across
  containing targets. No project / untitled / non-member → the single-file
  view, unchanged.
- Resolution order: local → declared sources (exported symbols only) →
  declared libraries first-wins → stdlib; `"stdlib": false` removes the
  `std::` surface (completion, hover, the materialized jump, and the
  warning refinement's stdlib contribution).
- What lights up per toolchain — PM: completion / definition / hover /
  semantic tokens / the undeclared-external refinement. TM: the same
  minus semantic tokens (the `.tmc` token layer is lexical), with the
  overlay surface deliberately narrower: routine names only, argless
  call/bind targets — grafts, alphabets, and binding arguments cannot
  cross a compilation unit, so the editor never offers them.
- Faithfulness: what the overlay resolves is what the linker resolves for
  the same declared set (stated as the design contract, in prose).
- Caveats, honestly: lexical path identity (symlinks/aliases not
  detected); assembly siblings open in the editor read from disk until
  saved; a sibling's unsaved edits reach a document's overlay on that
  document's next update; jumps into unopened files with non-ASCII target
  lines may mis-place the cursor within the line (UTF-16 identity
  conversion).
- **Corrections while here**: the `.tmc` diagnostics-channel list gains
  `undeclared-external` (today's text under-reports it); the "no
  materialized stdlib TM-side" note is replaced by the real behavior
  (both services materialize their stdlib for navigation).

- [ ] **Step 2: Verify every claim against the built tool** (the house docs-audit
  posture): run `cargo build --release`, then spot-drive `pmt lsp` /
  `tmt lsp` through an initialize handshake for any claim the section
  states about capabilities; every quoted behavior must trace to a test
  added in this plan.

- [ ] **Step 3: `docs/pmt/fmt.md` — `.pma` parity with the TM page** (folded
  into this milestone by maintainer request, 2026-07-26; docs-only, no
  code change — `pmt fmt` has formatted `.pma` since the pma-parity round,
  the page just never caught up to the shape `docs/tmt/fmt.md` was born with):

  - Title: `# Formatting \`.pmc\` — \`pmt fmt\`` → `# Formatting \`.pmc\`/\`.pma\` — \`pmt fmt\``.
  - Intro paragraph gains the extension-dispatch sentence, mirroring the
    TM page's opening: each input's extension picks its formatter — a
    `.pmc` file goes through the language's own printer (this page), a
    `.pma` file through the canonical assembly grid shared with the rest
    of the toolchain (`docs/formats.md`).
  - New `## \`.pma\` formatting` section between "Doc and attention runs"
    /"Spacing" and the `--check` section, mirroring `docs/tmt/fmt.md:221-237`:
    the canonical-grid statement, a short fenced example, and the
    whitespace-only/idempotence sentence. **Take the example from a real
    formatted `.pma`** (a golden fixture or a `docs/pmt/isa.md` snippet
    run through `pmt fmt -  --lang pma`) — never hand-type mnemonics.
    Before copying the TM page's closing `line-too-long` sentence, verify
    the claim PM-side (run `pmt lint` on an overlong formatted `.pma`
    line); state only what the tool does.
  - The stdin/`--lang pma` sentence in the `--check` section stays — it
    now has a body section to point at.
  - Ref-free prose throughout (published-docs policy).

- [ ] **Step 4: Update `CLAUDE.md`** — the current-state paragraph gains the
  plan-2 round (overlay shipped both toolchains, #37 closed, the
  follow-up issue for assembly-side overlay features filed per R6, the
  fmt-page parity fold-in).

- [ ] **Step 5: Run the full workspace one last time**

Run: `cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --check`
Expected: all green; test count grows from the 2,329 baseline.

- [ ] **Step 6: Commit**

```bash
git commit -m "docs: cross-file overlay in docs/lsp.md; pmt fmt page covers .pma"
```

---

## Post-merge follow-ups (file these, do not build them)

- Assembly-side overlay features (`.pma`/`.tma` callable-operand
  cross-file navigation + completion) — R6's follow-up issue.
- A TM semantic-token resolution tier (would give TM the PM token win) —
  note on the same issue or its own.
- Rename / find-references stay parked (the spec's v2 ledger).
