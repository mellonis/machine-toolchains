# TM-1 arc — phase 8 (docs half): the domain split

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** restructure `docs/` into per-toolchain domains so the TM-1 toolchain is documented to the same standard as PM-1, with every code citation resolving to a page that actually covers its keyword.

**Scope — the docs half ONLY.** The CHANGELOG version block and the GH release are **deliberately excluded** and land after the range-expression work, per the maintainer's ruling: `.tmc`'s language version has never appeared in a release (v0.2.0 predates the language), so waiting makes the first declaration a settled one rather than a revision. Do not add a version block, do not bump any version, do not tag or release in this phase.

**Architecture:** `docs/pmt/` and `docs/tmt/` each carry `language` / `isa` / `cli` / `stdlib` / `lint` / `fmt`. Genuinely shared pages stay at the root: `formats.md`, `history.md`, `lsp.md`, and — new, ruled during planning — **`docs/core.md`**.

**Tech stack:** Markdown only. No crate code changes except comment citations and three emitted-output strings.

## Maintainer rulings that shaped this plan

1. **`docs/core.md` is a fourth shared root page.** `crates/core` carries 47 citations of PM-domain pages for arch-agnostic substance (`docs/isa.md` ×28 alone — the VM, bus, devices, `DebugSession`, timing, loading, execution). Core is arch-agnostic by contract and tests against a fake arch; pointing it into `docs/pmt/` would be wrong, and worse once `docs/tmt/isa.md` documents the same VM. Core's five assembly lint rules make this concrete — they run on BOTH dialects but each comment can cite only one page. `docs/core.md` documents what `mtc-core` provides and mirrors the crate boundary the repo already treats as a hard contract. This EXTENDS the spec's own "genuinely shared pages stay at root" principle to a case it did not foresee.
2. **TM execution semantics RELOCATE out of `formats.md`.** ~372 lines of frames profile / composition engine / dispatch tables / framed calls currently live there only because earlier phases were told to keep landing TM material in shared root pages until phase 8. Execution semantics move to `docs/tmt/isa.md`; **wire layouts stay** in `formats.md` (frame descriptor bytes, the MX v2 frames region, container encodings). Duplicating would create two sources of truth that drift.
3. **The sweep rewrites only what is resolvable now.** Eight citations carry explicit "once it lands" hedges that cannot resolve until their target page exists. The first commit leaves those alone; each resolves in the commit that creates its page. No stub pages — a stub satisfies an ordering rule by lying through omission.

## Global Constraints

1. **Every citation keyword must RESOLVE.** A citation reads `docs/<page>.md (keyword)` and the keyword must correspond to real content on that page — ideally a heading. The v0.2.0 release precedent ran a citation-keyword resolution pass; this phase is held to it. A citation whose page lacks the keyword is a defect, not a stylistic nit. By the end of T8 there must be ZERO unresolvable citations and ZERO surviving "once it lands" hedges.
2. **Published-docs policy.** `README.md`, `CHANGELOG.md`, and everything under `docs/` (except `docs/superpowers/`) is published: forge-agnostic, no issue or PR numbers, no hosting-provider URLs. The canonical repository URL lives only in package manifests. Never `spec §N`, `GC{n}`, or `docs/superpowers/` citations in published content or code comments.
3. **Documentation is verified, not asserted.** Every claim a page makes about behaviour must be checked against the code or by running the tool. A page that describes an intended design rather than the shipped one is worse than no page. Where something is not implemented, say so plainly.
4. **No prose that treats a moving contract as settled.** `docs/tmt/language.md` describes range folding as it is (`{v±k}`, integer offsets, no modulus) WITHOUT implying that shape is final or complete — a language change is expected before the version is ever published. The same caution applies anywhere a known-open design question is described.
5. **The emitted-output refs move together.** Three `docs/` references live in emitted output, not comments — the `pmt --help` text, the completions registry, and the generated zsh script header — and the help text is quoted verbatim in `docs/cli.md`. They are drift-guarded against each other, so all four move in ONE commit or `tests/completions_registry.rs` fails. The same applies to any `tmt` equivalents.
6. **Gates every task:** `cargo test --workspace` · `cargo clippy --workspace --all-targets -- -D warnings` · `cargo fmt --check` · `git status --short crates/post-machine/tests/golden/` empty. Docs-only tasks still run them — citation edits touch source comments, and the registry drift guard tests emitted strings.
7. **No version movement.** No CHANGELOG entry, no version block, no crate/plugin/language/dialect version bump, no tag, no release. Out of scope by ruling.
8. **Cross-language differences get documented where a user of both would look.** The `.pmc` formatter aligns trailing comments only if the author had already aligned them in source — its output is therefore NOT a pure function of the token stream — while `.tmc` aligns unconditionally and never reads source columns. This belongs in BOTH fmt pages, not one.
9. Conventional commits with scope; NO attribution footers of any kind. Core crate behaviour untouched (comments only). No new dependencies.

## File Structure

- **Create:** `docs/core.md`; `docs/tmt/{language,isa,cli,stdlib,lint,fmt}.md`
- **Move:** `docs/{language,isa,cli,stdlib,lint,fmt}.md` → `docs/pmt/` (with `isa.md` SPLIT — shared VM to `docs/core.md`, PM-1 arch to `docs/pmt/isa.md`)
- **Stay, but rewritten:** `docs/lsp.md` (restructure to a shared framework page with per-language subsections), `docs/history.md` (gains TM-1 lineage), `docs/formats.md` (loses execution semantics, gains `.tmo`/`.tmx`/`.tmt`), `README.md` (front door)
- **Modify:** citation comments across `crates/`; `CLAUDE.md` links

---

### Task 1: the split, the sweep, and `docs/core.md`
The spec-mandated first move. Split `docs/isa.md` (266 lines, currently doing double duty) into `docs/core.md` (the arch-agnostic VM, bus, devices, `DebugSession`, timing model, registers, loading, execution — plus the assembler/linker frameworks, the shared asm lint rules, and the thin-renderer rule that core's other citations reach for) and `docs/pmt/isa.md` (PM-1's own architecture). `git mv` the remaining five PM pages into `docs/pmt/`. Rewrite every resolvable citation: core's 47 → `docs/core.md`, PM-domain → `docs/pmt/…`. LEAVE the eight hedged TM citations untouched. Move the emitted-output refs and `docs/cli.md`'s verbatim quote in this same commit (GC5). Update `CLAUDE.md` and `README.md` links (README's prose rewrite is T8; links only here).
- [ ] Verify: every rewritten citation's keyword resolves on its new page; the registry drift guard passes; gates green. Commit: `docs: the domain split — docs/core.md, docs/pmt/, and the citation sweep`

### Task 2: `docs/tmt/isa.md`
The hardest page, and the PM template actively misleads — `docs/pmt/isa.md` has one tape, no tables, no frames, no vectors, no FR register. Relocate the execution semantics out of `docs/formats.md` (frames profile, composition engine, match/dispatch tables, framed calls, traps, multi-exit returns) and write the TM-1 architecture around them: 20 opcodes, multi-tape vectors, `mtc`/`djmp` dispatch, `call.m`/`retx`/`trap`, the FR register and frame cache, the three call mechanisms. Wire layouts STAY in `formats.md` — move semantics, not byte tables, and leave `formats.md` coherent where content departs. Update the ~20 `formats.md` citations in TM crate + tests to their new target in the same commit.
- [ ] Verify: `formats.md` still reads coherently and its remaining citations resolve; no duplicated substance across the two pages. Commit: `docs(tmt): the TM-1 ISA page`

### Task 3: `docs/tmt/language.md`
The `.tmc` language: worlds (machine/routine/graph), the rule triple, alphabets, tapes, `call`/`graft`/`bind`, `with map` bindings incl. one-way `=>`, range expansion, doc and attention lines, `[deprecated]`, the reserved keywords, `TMC_LANG_VERSION` and what a pre-1.0 `0.N` acceptance contract means. Per GC4, describe range folding as it currently is without implying finality. Draw substance from the language's own source of truth (lexer/parser/compiler) and the Appendix A examples; verify every claim by compiling something.
- [ ] Verify: every construct documented has a compiling example. Commit: `docs(tmt): the .tmc language page`

### Task 4: `docs/tmt/cli.md` and the `tmt.json` schema
Every `tmt` subcommand with its real flags — compile/asm/link/dis/run/tape/ir/lint/fmt/lsp/completions — exit codes (0 stopped, 2 halted, 3 trapped), `--call-mech`, `--entry`, `--foutline`, `--emit-ir`, the `--fno-<pass>` family, `--nostdlib`. Document `tmt.json` (nearest-ancestor discovery, `lint.allow`, union semantics with IDE settings — never a cascade), noting it is a strict twin of `pmt.json`. Derive flags from the parser and the completions registry, not from memory; if the page and the registry disagree, the parser wins and the discrepancy is a finding.
- [ ] Verify: every documented flag is accepted by the real parser; every parser flag is documented. Commit: `docs(tmt): the tmt CLI and tmt.json reference`

### Task 5: `docs/tmt/lint.md` and `docs/tmt/fmt.md`
All 15 rules (12 `.tmc` + 3 `.tma` additions) with code, meaning, and why each matters; the shared allow namespace across both languages; `--warn` as the opt-in tier and that allow beats warn; that core's `unused-label` is suppressed on the `.tma` path and why (its lint context cannot see labels reached through lowered table sections) — stated as a current limitation, not a design. The fmt page: the grid rules, the width threshold, idempotence, whitespace-only, trivia preservation AND its documented exception (comments inside a binding list, signature parameter list, or alphabet body relocate). Per GC8, document the `.pmc`/`.tmc` trailing-comment divergence in BOTH fmt pages.
- [ ] Verify: rule list matches the code exactly, in both directions. Commit: `docs(tmt): the lint and fmt pages`

### Task 6: `docs/tmt/stdlib.md`
The twins — `std::binaryNumbers` (delimited, ten routines) and `std::binaryNumbersBare` (bare, four) — their alphabets and number representations, the graph+facade anatomy, how the delimited `invertNumber` is composed over the bare one through one-way marker collapses, embedding via `include_str!` + `OnceLock` at `-O1`-stripped, and `--nostdlib`. Mirror `docs/pmt/stdlib.md`'s shape where it transfers.
- [ ] Verify: every routine's documented signature matches `std.tmc`. Commit: `docs(tmt): the stdlib page`

### Task 7: the shared pages
`lsp.md`: restructure from "The `pmt` lsp language server" into a shared framework page with per-language service subsections (pmc, pma, tmc, tma) — and give it real headings for `completions`, `navigation`, `code actions`, and `semantic tokens`, because the seven citations phase 7 removed from `crates/turing-machine/src/lsp/*` come BACK here (NOT to a tmt page — the spec keeps `lsp.md` shared) and cite exactly those keywords. `history.md`: add the TM-1 lineage; the spec says the lineage covers both families and today the page has no TM content. `formats.md`: cover `.tmo`/`.tmx`/`.tmt` (currently zero mentions though `tmt` emits them), scope the sniff-not-extension rule to both toolchains, and fix the "five containers" intro.
- [ ] Verify: all seven restored citations resolve to real headings. Commit: `docs: the shared pages cover both toolchains`

### Task 8: README, `CLAUDE.md`, and the closing audit
`README.md` is a front-door rewrite, not a link fix — it still opens "A Rust toolchain for a Post machine" and says the build produces the `pmt` binary. Present both toolchains. Refresh `CLAUDE.md`'s documentation-authority section for the new layout. Then the closing audit, mirroring the v0.2.0 precedent: per-page claim verification and full citation-keyword resolution across the whole repo. GC1's exit condition is checked here — ZERO unresolvable citations, ZERO surviving "once it lands" hedges, zero `docs/superpowers/` or forge references in published content.
- [ ] Verify: the audit's greps come back empty and are shown in the report. Commit: `docs: the front door covers both toolchains` (+ a separate audit-fix commit if the audit finds anything)

---

## Self-review notes
- Spec coverage: the domain split ✅ T1; per-toolchain pages ✅ T2–T6; shared pages ✅ T7; the sweep-first ordering ✅ T1 (with ruling 3's carve-out). The version block and release are deliberately absent per the maintainer ruling — that is a scope exclusion, not a gap.
- Risks: T2 is the largest single write and the one where the PM template misleads most; T1's citation sweep is wide and its correctness is only visible through the keyword-resolution check, which is why T8 re-runs it repo-wide; T7's `lsp.md` restructure is the load-bearing prerequisite for the seven restored citations, so it must not be deferred.
- Carry-in debts closed here: the eight hedged citations (T2–T6, each in its page's commit), the seven removed `lsp.md` citations (T7), the `.pmc`/`.tmc` formatter divergence (T5), and the stale plan text about reusing `unused-label` as-is (T5 documents the real behaviour).
