# Documentation audit — 2026-09-13, master `2c36ebd`

Five parallel sweeps (`.tmc`, `.pmc`, `.tma`/`.pma`, CLI + man + manifests, lint/fmt/LSP/DAP), each against the parser or registry as the source of truth, each simplification and each `$` transcript re-run through fresh `target/release/{pmt,tmt}`. Items an agent could not run are listed as UNVERIFIED, not as findings.

## A. Claims the tools contradict (fix first — wrong today)

| Page | Claim | Reality | Proof |
|---|---|---|---|
| `docs/formats.md:603` | `.frame <name> tapes=(…)` takes a name operand | The assembler takes a **label** (`Fh: .frame tapes=(2, 0)`); the name form is `bad-frame`. The example at `:597` is already right; the bullet contradicts it. | `tmt asm` on both forms |
| `docs/tmt/asm.md:93-94` | "Jump and call targets are labels; `call` additionally accepts a routine symbol" | The split is the opposite: `jmp @name` assembles, `call @name` is refused ("call operands are already symbols; drop the `@`"), `jm`/`jnm @name` refused. `@` is documented only on the PM-1 page, nowhere in the shared "Assembly text" section. | `tmt asm` on each |
| `docs/pmt/isa.md:97` | `wr` operand "in PM-1 always one element" | Half right, half silent: `wr 1, 0`, `wrl 1, 0`, `wrr 0, 1` assemble and round-trip through `pmt dis` (the assembler is arch-agnostic), and at run time PM-1's lowering rejects any length but one with `Trap::BadOperand` (`arch/mod.rs:73-84`; verified, exit 3). The page now says both halves. Candidate for a later PM-1 assembler-side width check. | `pmt asm` + `pmt dis` + `pmt run` |
| `docs/tmt/cli.md:124-128` | the `unknown IR stage` error, hand-wrapped over three lines inside a fence | The binary prints one line; the framing sentence tells the reader to run it and compare. Only `$` transcript on any page that differs. | `tmt compile --emit-ir=after:bogus` |

Everything else that could be run reproduced byte-for-byte: every other `$` block on README and both `cli.md` pages, all 27 fmt fences on both fmt pages, every lint diagnostic string (constants resolved: `PRODUCT_THRESHOLD = 256`, `LIMIT = 80`), the full LSP capability matrix via a real `initialize` handshake against both servers, and the DAP command/capability/event sets.

## B. Constructs documented as a rule but never shown in an example

`.tmc` (`docs/tmt/language.md` unless noted): omitted transition — the only occurrence is the *error* case `['a'] -> ;`; `debugger` in a well-formed rule; `stop`/`halt` as a graft exit binding (verified: `hit = stop, miss = halt` compiles and links); bare-prose `!` line; reopened namespace; fold `*`; glyph escapes `\'`/`\\` inside a literal; `with map` on a `graft` (the glyph-identity subsection has only `call` examples); **empty map `with map { }`** — legal (verified) and entirely undocumented; `writes {}`/`preserves {}` in situ; a range inside a contract clause; a binding on a single-symbol cell (`['a' as c]`); `volatile tape` on a graph parameter; `bind` with a `with map`.

`.pmc` (`docs/pmt/language.md`): `halt`; `debugger`; a live `goto N;` (appears only in comments and commented-out error lines); `use PATH as ALIAS`; `use` inside a namespace; reopened and nested namespaces; a positive `@ns::name()` (only the error `@std::goto()` exists, inline); `@name(5)` call with a label successor; `export` on an ordinary top-level function (the only fenced one is the `volatile export main()` the page calls a no-op); bare-prose `!` line; empty `?` line as paragraph separator.

Assembly: `.byte` — recognized by both dialects (`cst.rs:53`, `lower.rs:1389`), documented nowhere as source; trailing-comma continuation of `.targets`/`.exits`/`.map` — stated twice, zero examples; `wrl`/`wrr` never in a `.pma` example on the ISA or asm page; `wrmv` only as a placeholder; `call.m` never shown together with the `.frame` it references; `.rept` operators `*`, `-` never used.

CLI (`docs/*/cli.md`): no runnable example for `--emit-ir`, `--lang`, stdin `-`, `--trace`, `--fno-<pass>` on either page, `--call-mech` (tmt: only the rejection), `--tact-profile` (pmt). Flags present only inside the quoted `--help` block with no prose of their own: `pmt compile -S`, `pmt asm -o`, `pmt link --no-relax`, `pmt run --head/--save-tape-block/--strict-cells`, `tmt asm -o`, `tmt run --save-tape-block`. Manifests: every loader key is in a table, but `profiles.debug`, `opt`, `debug-info`, `strip-debugger`, target-level `libraries`, `run.max-steps`/`run.tact-profile` (pmt) and `run.max-tacts` (tmt) appear in no JSON example.

Lint: **neither lint page carries a single triggering source example** — `docs/pmt/lint.md` has one fence (the `pmt.json`), `docs/tmt/lint.md` only diagnostic transcripts. Nearest-ancestor `lint.allow`, the `--allow` union, a successful `--allow`, `--fix`/`--force` (pmt), and the comment guard are prose-only. Three `.tma` rules (`shadowed-wildcard-rows`, `retx-exit-bounds`, `rept-var-unused`) state no quickfix availability; the five core rules' fix availability is stated neither on the lint pages nor on `docs/core.md` they delegate to. `docs/pmt/lint.md` omits `--exclude` and the directory walk.

Fmt: `.tmc` fmt aligns a run of adjacent `tape` declarations into one alphabet column (`print.rs:2507`) — the page never states it. `.pmc` fmt's four comment slots, the greedy rewrap and the blank-line rules have inline prose but no before/after fence (tool matches the prose exactly). The two recorded `.tmc` residuals — the only comment behaviour that moves a comment — have no example.

DAP: `disassemble` accepts an `offset` argument on both adapters (`dap/mod.rs:1543`) that shifts the "strictly positional" window; the page never mentions it.

## C. Examples longer than the rule they show (each shorter form proved by the tool)

| Where | Written | Shorter | Proof |
|---|---|---|---|
| `docs/tmt/language.md:593` | `entry graft goToNumberGraph(num = num, done = return) as body;` | drop `as body` — the section is about `done = return`; `as NAME` is introduced two sentences later; `stdlib.md:191` shows the same line unnamed; `unused-graft-name` would flag it | `tmt compile std.tmc` |
| `docs/tmt/language.md:614-615` | `then done;` + `state done { [*] -> stop; }` | `then stop;` — the section demonstrates `bind` | `tmt compile` on a `then stop` fixture |
| `docs/tmt/stdlib.md:65-66` | same forwarding state | `then stop;` | same |
| `docs/tmt/language.md:122` (lower confidence) | `… as seek;` in the overview program, nothing `goto`s it | drop `as seek` | same as row 1 |
| `crates/post-machine/src/stdlib/std.pmc:71,79,88,97` | `mark(!);` / `left(!);` / `right(!);` as the last command | `mark;` etc. — language.md:123 says the last `(!)` may always be omitted | codegen byte-identical at `-O0` and `-O1` |
| `README.md:62-79` | thirteen `N:` labels nothing references, `13: @goToBegin(!);` | drop the labels and the `(!)` — `pmt lint` on the identical golden fixture reports 13 `unused-label` | `pmt compile`; **caveat**: the block is byte-identical to `tests/golden/sum.pmc`, a port of the historic `Sum.pms` whose labels are line numbers — change both or neither |
| `docs/pmt/cli.md:196,197,218` | `-o hand.pmo`, `-o hand.pmx`, `-o app.pmx` | omit — each names the default output | `out_path` default |
| `docs/pmt/cli.md:602` | `--alphabet "0=' ','*'"` | the value is PM-1's default pair; `--cells` alone yields a byte-identical block | `cmp` |
| `docs/tmt/lint.md:530-531` | `'$' -> '_'` line duplicating `'^' -> '_'` | one pair | — |
| `docs/pmt/fmt.md:42-52` | `export goToBegin()` mirroring `goToEnd()` | one routine | — |
| `docs/tmt/fmt.md:313-320` | an `alphabet bits` line the fence's text does not use | drop | — |

Assembly examples: **no redundancy found** — every candidate (`.section code`, catch-all `.row [*]`s, `.routine` on the flagship) was tested and proved load-bearing.

## D. Guards that do not exist

- **No test set-compares any lint registry against any lint table.** The `error_code_docs` guards pin compile/asm error codes only. All four lint registries currently match the pages by hand-check; nothing keeps them so.
- `recognized_directives` pins the recognizer against the *editor grammars*, not against `docs/formats.md`; every asm-doc finding above is unpinned.
- `-h` is consumed by every subcommand of both CLIs but is in neither registry (so neither completion script offers it) and on no page; documented only for `tmt tape-block`/`ir`'s children.

## E. Premises the audit disproved (so they are not re-asked)

No `::`-absolute paths in `.tmc`; no one-target `check` and no `stop` statement in `.pmc`; PM-1 has 19 opcodes, not 20; `-Werror` is a compile/build flag, not a lint knob; `pmt lint` has no `--warn` (opt-in rules exist only on tmt); lint has no error/warn severity axis, only default-on vs opt-in; `goto NAME` vs bare-name sugar is documented as equal spellings and `fmt` preserves either, so neither is redundant.

## Side observation (not from the sweeps)

On a machine whose only rule is `[*] -> call std::…::plusOne() then stop;`, `tmt lint` reports "tape `num` is never read, written, or moved" although the transparent call processes the whole tape. Same root as issue #95 item 4: the lint does not know what an external callee does. Candidate for the hygiene list.

## Keyword hover (ruled 2026-09-13, not from the sweeps)

Hover today answers only on documented symbols — declarations, call sites, `use` paths — and `docs/lsp.md:395` states it never surfaces without text to render; keywords get nothing. Add a static token → one-sentence table to each source-language service, hit-tested on the token under the cursor, no overlay involved, for the words whose behaviour is not implied by their name:

- `.tmc` `stop` / `halt` — normal termination (exit 0) versus abnormal (exit 2); `return`; `debugger`.
- `.pmc` `(!)` — the last-command successor: end of `main` stops, end of a function returns; omissible on the last command (the rule the four `std.pmc` redundancies in §C forgot).
- `!` and `?` line starts — attention line and doc line, and what each attaches to (the bare `!` form has no example anywhere, §B).
- `=>` versus `->` in a map — one-way read collapse versus two-way pair.
- `-` and `.` in vectors, `*` in a pattern — keep, stay, wildcard.

`hoverProvider` is already advertised, so the handshake is unchanged; `docs/lsp.md`'s "no hover without documentation" sentence is reworded, and the LSP audit check (a real `initialize` plus hover probes) re-verifies it. Lives in the docs round or in the arc's LSP phase, whichever comes first.

## UNVERIFIED (not findings)

`ir footprints` transcripts over unshipped fixtures; `--warn state-may-trap`'s `4:15` position; the `.tmc` fmt residuals and the side-by-side block/line diagram; everything past `initialize` in LSP and everything past the handshake in DAP; `Tdec`/`Tdot` in the flagship being byte-identical copies of `Tinc` (a program-level refactor, not a notation, and pinned by five goldens).

## Disposition (ruled 2026-09-13)

- Section A (claims the tools contradict) and the wrapped transcript: a standalone docs commit before the arc — they are wrong today.
- Section B items on `docs/tmt/language.md` (reuse, symbol maps), `docs/tmt/stdlib.md` and both lint pages: inside the arc's docs phase, which rewrites those sections anyway.
- Everything else in B, C and D (`.pmc`, assembly, fmt, CLI, DAP, the two missing guards): a docs round **before the arc or inside it, never after**.
