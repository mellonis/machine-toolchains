# `.tmc` Range Expressions and `.rept` Emission — Design

**Status:** approved 2026-07-21.
**Driving issue:** [#31](https://github.com/mellonis/machine-toolchains/issues/31). Folded in by maintainer ruling: [#43](https://github.com/mellonis/machine-toolchains/issues/43) (closure as by-design + opt-in lint), [#44](https://github.com/mellonis/machine-toolchains/issues/44) + [#48](https://github.com/mellonis/machine-toolchains/issues/48) (the never-fires family), [#45](https://github.com/mellonis/machine-toolchains/issues/45) (optional-transition sugar, accepted minimal), the complete unused-family sweep — [#47](https://github.com/mellonis/machine-toolchains/issues/47) (unused-alphabet / unused-tape), [#46](https://github.com/mellonis/machine-toolchains/issues/46) (unused-graft-name), [#33](https://github.com/mellonis/machine-toolchains/issues/33) (unused-exit) — plus [#35](https://github.com/mellonis/machine-toolchains/issues/35) (duplicate-map-source `.tma` lint) and [#38](https://github.com/mellonis/machine-toolchains/issues/38) scoped to the new rules (every rule added here ships a quickfix where a safe textual fix exists).
**Explicitly out of scope:** [#34](https://github.com/mellonis/machine-toolchains/issues/34) (core-side table-aware `unused-label` — ruled out to keep this branch out of `crates/core`; it lands as its own small PR right after); [#32](https://github.com/mellonis/machine-toolchains/issues/32) (dispatch-trampoline threading — optimizer-pass work with its own equivalence-contract review, standalone); [#40](https://github.com/mellonis/machine-toolchains/issues/40) (directive drift-guard bidirectionality — its own test-infra job); the #38 retrofit of pre-existing rules; stdlib text edits, including the twelve `unused-graft-name` findings the new rule will surface in `std.tmc` (they ride the pre-release stdlib pass with #49, which can also adopt the #45 sugar in the same text edit); ring alphabets (a possible future sugar lowering to `%` — nothing here forecloses it).

This is the last gate before the arc release: the maintainer ruled the CHANGELOG version block and GH release wait for this work so `.tmc`'s language version is declared once at a settled grammar.

## 1. Motivation

Codegen stamps every range-expanded rule into individual rows; the assembler's `.rept` expresses the same thing in a handful of lines, and the two forms assemble to **byte-identical objects**. On the brainfuck UTM: 212 hand-written lines vs 1659 generated at `-O1` — the bulk being 127 stamped copies each of the `+`, `-` and `.` handlers. This is a text problem (inspectability of `-S` output, diff churn), not a machine problem — the 391 table rows are irreducible in the machine.

Root cause: the two ends are the same feature, and the lower level is currently more expressive. The assembler's substitution grammar has `+ - * %`; `.tmc`'s range folding is `base ± k` only, `%` does not lex, and a fold that leaves the alphabet is a hard error with no wrap. So the UTM's `.tmc` port needs a boundary-rule workaround, and codegen has no source-level form it could emit compactly.

Two ends, one feature:

1. **`.tmc` write-fold expressions** — extend the fold to the assembler's exact grammar (§3).
2. **codegen `.rept` emission** — re-detect stamped arithmetic families at emission and compress (§5).

## 2. Decisions record

All ruled by the maintainer during the 2026-07-21 design round:

| Question | Ruling |
|---|---|
| Wrap syntax: `%` expressions vs ring alphabets | **`%` expressions** — the assembler's exact grammar; explicit at the use site; ring alphabets remain possible later sugar |
| Negative remainder | **Error at both ends** (mirrors the assembler); diagnostic teaches the `{(v+N-1)%N}` idiom |
| Emission architecture | **Re-detect at emission** (approach B) — no IR change, no optimizer contact, self-check + fallback |
| #43 omitted-map index identity | **Keep as intended semantics**, document it, add opt-in warn-tier lint `index-identity-map` |
| #45 stay-in-state sugar | **Accept minimal**: omitted transition = stay; `call … then` stays mandatory |
| #44 empty expansion | **Warning** (`empty-expansion`) + zero-row-sound codegen |
| #48 dead catch-all ICE | **Folded**: warning (`unreachable-rule`) + drop from emission — same never-fires principle as #44 |
| #47 unused alphabet/tape | **Folded**: two default-tier lint rules |
| #46 unused entry-graft name | **Folded**: `unused-graft-name`, default tier; stdlib's twelve findings are expected output, cleaned up later in the stdlib pass |
| #33 unused graph exit | **Folded**: `unused-exit`, default tier — completes the unused-family sweep |
| #34 table-aware `unused-label` | **Kept out** — core-crate surgery; its own PR right after this branch |
| #35 duplicate `.map` source | **Folded**: `duplicate-map-source`, `.tma` lint, turing-side, no dialect change |
| #38 lint quickfixes | **Folded, scoped**: new rules ship a Fix where a safe textual fix exists; retrofit of pre-existing rules stays out |
| #32 dispatch trampolines, #40 directive guard | **Kept out** — standalone follow-ups |
| `TMC_LANG_VERSION` | **Stays 0.1** — nothing published yet, so the acceptance contract has not activated; the grammar settles under 0.1 and the release declares it once |

## 3. `.tmc` write-fold expressions

### Grammar

The write-cell substitution `{…}` grows from `{name}` / `{name±int}` to exactly the assembler's substitution grammar (`crates/core/src/asm/subst.rs`):

```
expr := mul (('+' | '-') mul)*
mul  := atom (('*' | '%') atom)*
atom := var | integer | '(' expr ')'
```

- `var` is any in-scope numeric pattern binding. Several may appear in one expression (`{v+w}`) — this falls out of the grammar; each expanded row binds all of them.
- The lexer gains a `%` token. Parens already lex.
- A substitution that is a **single bare name** keeps today's passthrough semantics (works for glyph and numeric bindings alike). Any expression containing an operator or parens requires numeric bindings only; a glyph binding in arithmetic context stays the existing `CharArithmetic` error.

### Semantics

Folded at **compile time, per expanded row**, over `i64` — a resolved constant, never a runtime computation (unchanged principle from `docs/tmt/language.md`). Operators are left-associative; `*`/`%` bind tighter than `+`/`-` — identical to the assembler.

Spanned errors, at the substitution's span:

| Condition | Error |
|---|---|
| modulus is zero | zero-modulus (matches the assembler) |
| any intermediate overflows `i64` | overflow (matches the assembler) |
| any intermediate remainder is negative | **new** `negative-remainder` — message suggests the `{(v+N-1)%N}` idiom for a wrapping decrement |
| fold result names no symbol in the tape's alphabet | existing `FoldOutOfAlphabet`, unchanged |

The negative-remainder rule mirrors the assembler **exactly** (which errors rather than wrapping): one `%` semantics across `.tmc` and `.tma`, so expression text moves between the two languages without surprises. This was ruled over the euclidean alternative.

### Version

`TMC_LANG_VERSION` stays `0.1` (maintainer ruling — see §2). The language reference's substitution section is rewritten around the expression grammar; the increment example gains the wrapping form.

## 4. Optional transition (the #45 sugar)

A rule's transition becomes optional **when the rule has at least one of `write`, `move`, or `debugger`**; an omitted transition means "stay in the current state" — the same footing as the write vector's implicit keep and the move vector's implicit stay.

- `['a'] -> ;` (nothing after the arrow) stays a **parse error** — a rule that matches, changes nothing, moves nothing, and stays put is a guaranteed livelock and may not be written by omission.
- `call TARGET(args) then CONT` is unchanged — `then` stays mandatory. "Return to the call site's own state" is not a concept the language acquires.
- **Resolution timing:** the parser records the omission (no transition node); resolution to a concrete self-`goto` happens **after graft splicing**, so a rule inside a graph resolves to its own spliced instance, not to the graph-source state.
- **Formatter:** the omission is preserved — fmt never inserts a `goto`. The whitespace-only guarantee holds.
- **CST:** the rule node's transition child is optional; lossless round-trip includes the omission.
- **LSP/completions:** the transition-position context now also accepts end-of-rule; the completion set at that position is unchanged (offering transitions remains correct — they are merely no longer required).

## 5. `.rept` re-detection emitter

### Placement

A compression stage in codegen operating on the **emitted structured lines before final text join** — after the optimizer, downstream of everything semantic. Ranges are flattened before the IR and both optimizer passes are per-row, so the stage re-detects arithmetic families rather than consuming provenance (ruled over bumping `TM_IR_VERSION` and giving every per-row pass a preserve-or-invalidate obligation). A consequence worth having: it also compresses arithmetic families that never came from a source range.

### Detection

- A **block** is a label-delimited group of emitted lines; **continuation runs** of same-label `.row` / `.targets` lines inside table sections are detected the same way.
- A compressible **run** is a maximal sequence of ≥ **4** consecutive blocks that are identical token-for-token except integers at fixed token positions.
- Each varying position's progression is inferred and verified across the whole run: constant, `v + k`, or `(v + k) % N` (`N` inferred from the wrap point). Labels with embedded indices parameterize the same way (`plus__88` → `plus__{v}`).
- The run is rewritten as `.rept v, lo, hi` … `.endr` with reconstructed `{expr}`s in the assembler's grammar. Emitted expressions are always assembler-legal (in particular, decrements emit the `+N-1` form).
- Runs shorter than 4, and runs where any position fails progression inference, stay stamped. Gaps (e.g. a row deleted by `dead_rows`) split runs naturally.

### Safety — two layers

1. **Always-on self-check with fallback.** Before a compressed run is accepted, the emitter re-expands its own `.rept` text through the real substitution rules and byte-compares against the stamped lines it replaces. Any mismatch silently falls back to stamped for that run. The pass can therefore never change what assembles — only how the text reads.
2. **Byte-identity test gates.** Tests assemble the compressed and stamped forms and assert identical object bytes — on unit fixtures and on the flagship (see §9).

### Interaction with existing contracts

- Compression is emission, not optimization: it applies at every `-O` level, and the `-O0` floor (optimizer artifacts must not leak) is untouched — `-O0` **object bytes** are unchanged by construction; `-O0` `-S` *text* changes, which the floor does not govern.
- `.tma` dialect unchanged — the emitter targets the existing `.rept` / `{expr}` surface at dialect 0.3.
- `tmt compile --stamped-asm` (new flag, default off) disables compression — for diffing against older output and debugging the emitter. Registered in the completions registry with its drift guards; documented in `docs/tmt/cli.md`.

## 6. The never-fires family (#44 + #48)

One principle, two shapes: **a rule that can never fire warns and contributes no rows.** Both are compile warnings (the `expansion-threshold` precedent — surfaced through `CompileReport`, not lint findings).

- **`empty-expansion`** (#44): a rule whose expansion drops to zero rows — every range alternative falls outside the tape's alphabet, or a single concrete symbol is absent. Spanned at the rule. The rule contributes nothing; compilation proceeds.
- **`unreachable-rule`** (#48): a rule following a same-state catch-all (all-wildcard pattern). Spanned at the rule. Dropped from emission, so codegen never produces a table violating the all-wildcard-row-last discipline.

Codegen becomes **sound for zero-row rules**: no dispatch-target label, no `.row`, no dangling table reference. A state left with zero rows is valid and traps at runtime on entry — consistent with existing no-match semantics (and with the opt-in `state-may-trap` lint's story). The implementation sweeps the neighbouring paths for the same shape: anywhere expansion can produce zero rows and codegen assumes at least one.

Relationship to lint: `dead-rule` (lint tier) continues to catch shadowing more generally at `tmt lint` time; the compile warnings exist because compilation must be total and honest even when lint was never run.

## 7. #43 closure — documented index identity + opt-in lint

`docs/tmt/language.md` gains a paragraph documenting that a `call`/`bind` with an omitted map applies **index identity**, including across differently-glyphed same-size alphabets, with the layer rationale: graft is a source-level splice (compile-time, where glyphs are the author's mental model — identity there means glyph identity, hence `identity-glyph-mismatch`); call is a machine-level boundary (resolved at link time, where glyphs do not exist — identity there means index identity). The processor never sees glyphs; a glyph-agnostic routine called index-wise is intended behavior. An error was ruled out: it could only ever cover same-compilation calls, and a check that vanishes when the callee moves to another object is a false promise of a closed set.

New lint rule **`index-identity-map`**, **warn tier** (off by default, enabled by `--warn`, the `state-may-trap` precedent; allow-suppressible in the shared namespace): fires on an omitted-map `call`/`bind` where both alphabets are visible to the compiler and are not glyph-for-glyph equal, naming the first differing index. This makes the one genuine trap — same glyphs, different order, silently swapped meanings — findable the day output is mysteriously wrong, without taxing the legitimate glyph-agnostic idiom.

#43 closes as by-design when this lands.

## 8. The lint wave (#47 + #46 + #33 + #35, with #38-scoped quickfixes)

Four new **default-tier** `.tmc` rules completing the unused-* family, plus one `.tma` rule:

| Rule | Surface | Fires when | Quickfix (#38 policy) |
|---|---|---|---|
| `unused-alphabet` | `.tmc` | an alphabet declaration no tape draws on | delete the declaration |
| `unused-tape` | `.tmc` | a tape no rule ever touches — `*` in every pattern cell, `-`/omitted in every write cell, `.`/omitted in every move cell, never a binding argument | none (deleting a tape changes every vector's arity — not a safe textual fix) |
| `unused-graft-name` | `.tmc` | a graft's `as NAME` that nothing references (the reachable-but-unnamed gap `unused-graft-instance` structurally misses — an entry graft is reachable by being the entry) | remove ` as NAME` |
| `unused-exit` | `.tmc` | a graph declares a `state` exit parameter its body never targets — while every caller is still obliged to bind it | none (removing the parameter is an API change across call sites) |
| `duplicate-map-source` | `.tma` | a `.map` clause names the same source symbol twice (last-wins today, byte-proven); lives turing-side in `lint/tma/`, no core change, no dialect change | drop the earlier, shadowed clause |
| `index-identity-map` (§7) | `.tmc`, **warn tier** | omitted-map `call`/`bind` across visibly differently-glyphed alphabets | none (writing the intended map requires intent the tool cannot guess) |

`unused-tape` is worth flagging beyond tidiness: an untouched tape costs bytes in every row of the program.

**#38 scope ruling:** every rule added in this round ships a `Fix` where a safe, purely textual fix exists (the table's last column); rules where no such fix exists ship `fix: None` with the reason recorded in the rule's doc comment. This ends the zero-`.tmc`-quickfix regression against the PM-1 pair. Retrofitting the fourteen pre-existing rules stays out of scope; #38 is re-triaged (close or narrow) once this lands.

All rules get fixtures, docs rows in `docs/tmt/lint.md`, and entries in the shared allow namespace.

## 9. Flagship, docs, and tests

### Flagship

`docs/examples/brainfuck-utm.tmc` drops the wrap workaround (non-wrapping range + explicit boundary rule) for `{(v+1)%127}` / `{(v+126)%127}`. Goldens stay derivation-first; the `.tma`/`.tmc` equivalence test is unchanged. A new assertion pins that the three 127-way families emit as `.rept` in `-S` output (generated text should land near the hand-written 212 lines).

### Docs

- `docs/tmt/language.md`: substitution section rewritten (expression grammar, error table, wrapping increment example), optional-transition rule, index-identity paragraph.
- `docs/tmt/cli.md`: `--stamped-asm`.
- `docs/tmt/lint.md`: the full lint wave — `index-identity-map` (warn tier), `unused-alphabet`, `unused-tape`, `unused-graft-name`, `unused-exit`, `duplicate-map-source` (`.tma` table) — plus a note relating `dead-rule` to the new compile warnings and the fix-availability column.
- `editors/grammars/`: `%` in the `.tmc` substitution context; bidirectional drift guards stay green.
- CHANGELOG: deferred to the release cut per the standing ruling — this work is what un-defers it.

### Tests

- **Fold expressions:** parser + eval units mirroring the assembler's `subst.rs` cases (`(v+1)%127`, precedence, parens, zero modulus, overflow), the negative-remainder diagnostic with its hint, multi-var folds, `CharArithmetic` still rejected, `FoldOutOfAlphabet` unchanged.
- **Optional transition:** parse, empty-body parse error, self-resolution after graft splicing (rule in a graph self-loops to the spliced instance), fmt idempotence + omission preservation, CST losslessness.
- **Emitter:** detection units (affine, modular, constant columns, run splits on gaps, sub-4 runs stay stamped, inference-failure fallback, self-check fallback on a forced mismatch), assemble-both byte-identity property over generated programs, flagship end-to-end, the `-O0`/`-O1` × mono/frames/hybrid matrix unchanged.
- **Never-fires:** `empty-expansion` and `unreachable-rule` fixtures (warn + assemble + run-to-trap), zero-row state, graft-instantiated generic drop case compiles with warning.
- **Lints:** `index-identity-map` (fires under `--warn` only; silent when glyph-equal; names the differing index), `unused-alphabet`, `unused-tape` (incl. the all-`*`/`-`/`.`-but-bound-as-argument negative case), `unused-graft-name` (entry graft with referenced name stays silent; the stdlib's twelve findings pinned as a fixture expectation), `unused-exit` (an exit targeted only through a nested scope still counts as targeted), `duplicate-map-source` (`rmap=(1->2, 1->3)` flagged; distinct sources silent). Quickfix application tests for each rule that ships a Fix (apply → re-lint clean → still compiles).
- **CLI:** `--stamped-asm` behavior + completions registry drift guards.
- PM-1 byte-identity remains a standing gate (core is untouched except possibly exposing the substitution helper for the self-check — no PM-1-visible change).

## 10. Version spaces

| Space | After this work |
|---|---|
| `TMC_LANG_VERSION` | **0.1** — unchanged by ruling; grammar settles pre-declaration |
| `TM1_TMA_DIALECT_VERSION` | 0.3 — unchanged (emission targets the existing dialect) |
| `TM_IR_VERSION` | 2 — unchanged (approach B touches no IR) |
| `IR_VERSION` (PM) | unchanged |
| Containers MO/MX/MT | unchanged |
| Crates | unchanged until the release cut |
