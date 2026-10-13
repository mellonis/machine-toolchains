# Pre-cut round: tape-block shape edits, optimizer references, semantic token colors

Date: 2026-08-07
Driving issues: [#62](https://github.com/mellonis/machine-toolchains/issues/62)
(tape-block add/remove/reorder),
[#67](https://github.com/mellonis/machine-toolchains/issues/67)
(optimizer reference docs),
[#51](https://github.com/mellonis/machine-toolchains/issues/51) prong 2
(LSP4IJ `SemanticTokensColorsProvider`).
Status: approved in conversation 2026-08-07; this document records the rulings.

This is the last engineering round before the release cut
(milestone "Release cut — closes the TM-1 arc"). One branch carries all
three work items — `feat/pre-cut-round-62-67-51` off current master
(`5b729a8`) — merged once, with per-item commits keeping review
boundaries. Execution order inside the branch: #62 → #67 → #51, so the
optimizer/CLI docs land on a CLI surface that is no longer moving.

All three decisions below marked **(ruled)** were made by the maintainer
during the 2026-08-07 brainstorm; the rest is design detail consistent
with those rulings.

---

## Part 1 — #62: shape edits for `tmt tape-block set`

### Scope

**TM-only (ruled).** `pmt tape` gains nothing: PM-1 is architecturally
single-tape, so add/remove produce a block `pmt run` rejects anyway and
reorder of one band is the identity. The PM docs get one sentence
stating this and pointing multi-band editing at `tmt tape-block`.
No new flags appear in the pmt completions registry.

Shape edits live on **`set` only**. `new` already fixes shape at
creation (`--from` header / `--alphabet` keys) and `show` is untouched.

### The three flags

```
--add-tape [KEY=]ALPHABET   insert a band at position KEY, or append
--remove-tape KEY           drop a band
--reorder K1,K2,...         permute bands into the given order
```

- `--add-tape` — ALPHABET is mandatory and uses the existing glyph-list
  notation (`parse_glyph_list`, its 127-glyph cap and duplicate checks
  apply as-is). The new band starts empty: no cells, head 0, origin 0.
  Its alphabet is stored as the band's per-tape override
  (`TapeSnapshot.alphabet = Some(...)`) — never merged into the
  block-level fallback. `KEY` is a **numeric position only** (0..=len;
  len = append, same as the bare form). A tape *name* names a band, not
  a gap, so a name in an add key is an error saying exactly that.
  Repeatable; multiple adds apply in flag order, each seeing the block
  as previous adds left it.
- `--remove-tape` — `KEY` is an index or (with `--from`) a name.
  Repeatable; **all removals resolve against the input block
  simultaneously** (`--remove-tape 0 --remove-tape 1` drops input bands
  0 and 1, not input-0 then post-removal-0). A duplicate key is an
  error. The band's cells, head, origin, and alphabet override go with
  it; the block-level fallback alphabet stays even if nothing uses it.
- `--reorder` — a **complete permutation** of the post-add block: every
  band exactly once, entries are indices or surviving names. Missing or
  repeated entries are an error naming them. At most one `--reorder`
  per invocation (two permutations have no composition order a reader
  can see; error).

A result with zero bands is an error (`tape-block would have no
tapes`). An empty edit set is not an error (today's behavior:
`set` with no edits is a copy).

### Phase pipeline (ruled)

Flag position on the command line never matters. Edits apply in a fixed
documented phase order, extending the existing "`--alphabet` applies
before `--cells`" contract:

```
remove  → keys name bands of the INPUT block
add     → positions in the block AFTER removals (then earlier adds)
reorder → permutation of the post-add block
content → --alphabet / --cells / --head / --origin address the FINAL shape
```

Worked example (goes into `docs/tmt/cli.md` verbatim, verified against
the built binary):

```
tmt tape-block set in.tmt --remove-tape 1 --add-tape 0='0','1' \
    --reorder 2,0,1 --cells 0="'1','1'" -o out.tmt

input : [A B C]
remove 1   -> [A C]      key 1 = B in the INPUT
add 0=...  -> [N A C]    insert at position 0 AFTER removals
reorder    -> [C N A]    permutation of the result
cells 0    -> edits C    FINAL index 0
```

### Names ride the pipeline (ruled)

`--from APP.tmc` on `set` binds source tape names to the bands of the
**input** block (source tape count must equal the block's — the
existing rule) and the names **follow their bands** through the
pipeline: a removed band's name goes with it (using it later in
`--reorder` or a content edit is an error saying the band was removed
by `--remove-tape`), an added band has no name (index-only), reorder
moves the name with the band. So
`--remove-tape scratch --cells main="..."` works in one invocation even
when the removal or a reorder shifted `main`'s index. Names still exist
only for the duration of the invocation — `.tmt` never stores them.

### The reorder foot-gun: nothing new needed

The issue asked whether `run` can detect a block reordered against the
image's expectations. It already detects everything detectable:
`tmt run` checks band count against the MX header's tape count AND each
band's glyph count against the image's per-tape cardinality
(`crates/turing-machine/src/cli/run.rs`). A reorder among
equal-cardinality bands is indistinguishable from intent by
construction. No new mechanism; `docs/tmt/cli.md`'s tape-block section
gets an explicit paragraph stating both halves.

### Implementation shape

`crates/turing-machine/src/cli/inspect.rs`: extend the parsed edit
struct with `adds: Vec<(Option<usize>, Vec<String>)>` (order
preserved), `removes: Vec<String>` (raw keys), `reorder:
Option<Vec<String>>`; apply as a new `reshape` step before the existing
content-edit application, with name binding resolved once against the
input block and threaded through as an index map. No format change —
MT v2 already carries per-tape alphabets; `.tmt` written by the new
path must round-trip through the existing codec untouched.

Surface bookkeeping the drift guards force: the tmt completions
registry gains the three flags on `tape-block` (set shape); the
`tape-block` usage string in `inspect.rs` and the `docs/tmt/cli.md`
section update together.

### Tests

Integration tests beside the existing tape-block ones: each flag alone;
the worked example above end-to-end (byte-compare the resulting block
against one authored directly); names-through-pipeline (remove by name
+ content-edit by name in one invocation, reorder by name); every error
class — bad key, name in add position, duplicate remove, incomplete /
repeated reorder entry, two `--reorder` flags, zero-band result,
removed-name reuse, `--from` count mismatch (existing), glyph-list
errors surfacing with the flag's prefix. Plus: a reshaped block loads
under `tmt run` and the cardinality mismatch path still fires.

---

## Part 2 — #67: optimizer reference pages

Two new pages in the per-toolchain domain split:

- `docs/pmt/optimizer.md` — nine passes: `inline` (program-level,
  first), then per-function `check_fold`, `jump_threading`,
  `cell_state`, `branch_fold`, `tail_call`, `tail_merge`, `dce`,
  `fuse_tape_ops`.
- `docs/tmt/optimizer.md` — eight passes: the ported motion passes
  (`inline`, `jump_threading`, `tail_call`, `tail_merge`, `dce`), the
  TM-native `dead_rows` and `dispatch_select`, and the default-off
  `outline` behind `--foutline`.

Rosters, pipeline split (program-level vs per-function/world), and
actual run order are taken from `optimizer/mod.rs` of each crate at
writing time, not from this spec.

### Page structure

Per page: an intro (where the optimizer sits in the pipeline, `-O0` vs
`-O1`, fixpoint loop with round cap), a **shared contracts** section,
one section per pass, and a flags section (`--fno-<pass>`, `--foutline`
on the TM page, `--emit-ir[=STAGE]`).

The shared contracts are **stated in full on each page, not
cross-referenced** (the issue's own ruling):

- `-O0` bit-identity — a locked floor; no optimizer artifact may leak
  into plain codegen output.
- The equivalence contract — passes preserve final tape, termination
  kind, and match-flag-dependent branches; step counts and
  resource-limit outcomes may change — except across an un-stripped
  `brk`, an observability barrier no motion crosses.
- Pass ordering as contract — `tail_call` before `tail_merge` (return
  chaining destroys tail-call's precondition), stated on both pages.
- PM page only: MF-coupling soundness (after ≥1 tape op the match flag
  equals the cell at head; before any tape op it is the decoupled reset
  value; check-edge refinement applies only on provably coupled paths).
- TM page only: why `dead_rows` and `dispatch_select` exist only there
  (band dispatch / table lowering have no PM counterpart), and why
  `outline` defaults off.

`--foutline` finally gets its decision-making paragraph: what outlining
does, what it trades, why off by default — the issue's sharpest
instance.

### Worked examples (ruled: one per pass)

Every one of the seventeen passes gets a minimal source program plus
real before/after IR fragments obtained through the built binary
(`--emit-ir=after:<pass>` and the neighbouring stage), trimmed to the
lines that show the transformation but never fabricated. The example is
a by-product of the verification standard, which applies unchanged:
every claim checked against source or binary, every quoted transcript
real. Where a pass is easier to show in emitted assembly than IR
(e.g. `fuse_tape_ops`, `dispatch_select`), the page may show `-S`
output instead — same realness rule.

### Cross-links and collateral

Both `docs/{pmt,tmt}/cli.md` `--fno-<pass>` / `--emit-ir` sections gain
a pointer to the new page (`docs/pmt/optimizer.md (passes)` style).
README's docs table (front door) gains the two pages. No change to
`docs/core.md` — core has no optimizer, and the contracts above are
per-toolchain facts. No code changes; `cli_docs` drift guard unaffected
(the `tmt --help` quote does not change).

---

## Part 3 — #51 prong 2: LSP4IJ `SemanticTokensColorsProvider`

### Verified API (probed 2026-08-07 against the real dependency)

From `lsp4ij-0.20.1.jar` in the gradle cache (the version both plugins
pin), via `javap`:

```
public interface com.redhat.devtools.lsp4ij.features.semanticTokens.SemanticTokensColorsProvider {
  TextAttributesKey getTextAttributesKey(String tokenType,
                                         List<String> tokenModifiers,
                                         PsiFile file);
}
```

Extension point (from the jar's `plugin.xml`): name
`semanticTokensColorsProvider` in the
`com.redhat.devtools.lsp4ij` namespace, bean carries `serverId` +
`class`. The jar also ships
`DefaultSemanticTokensColorsProvider` — the fallback delegate below.

### What each server emits (verified from source)

| service | token types (legend order) | modifiers |
|---|---|---|
| tmc | `namespace`, `type`, `function`, `variable`, `string`, `number` | `declaration` |
| tma | `function`, `variable`, `type`, `number` | `declaration` |
| pmc | `namespace`, `function`, `number` | `declaration`, `defaultLibrary` |
| pma | `function`, `variable`, `number` | `declaration`, `defaultLibrary` |

The provider receives token type **names**, so one mapping per plugin
covers both of its languages.

### The mapping

One Kotlin class per plugin — `TmtSemanticTokensColorsProvider`
(`ru.mellonis.tmc`) and `PmtSemanticTokensColorsProvider`
(`ru.mellonis.pmc`) — structurally identical copies, like every other
pair file. Mapping to `DefaultLanguageHighlighterColors`, which every
theme colors:

| tokenType (+ modifier) | TextAttributesKey |
|---|---|
| `function` + `declaration` | `FUNCTION_DECLARATION` |
| `function` | `FUNCTION_CALL` |
| `namespace` | `CLASS_NAME` |
| `type` | `CLASS_REFERENCE` |
| `variable` | `LOCAL_VARIABLE` |
| `string` | `STRING` |
| `number` | `NUMBER` |

`defaultLibrary` does not change the key (a stdlib call reads as a
call) — ruled during design. Any token type outside the table delegates
to `DefaultSemanticTokensColorsProvider`, so a future server-side
legend addition degrades to LSP4IJ's default rendering instead of
disappearing.

Registration in each `plugin.xml`, inside the existing
`defaultExtensionNs="com.redhat.devtools.lsp4ij"` block:

```xml
<semanticTokensColorsProvider serverId="tmtLsp"
    class="ru.mellonis.tmc.TmtSemanticTokensColorsProvider"/>
```

(`pmtLsp` / `ru.mellonis.pmc...` on the PM side.)

### Versions

- `editors/jetbrains-pm`: **0.1.2 → 0.1.3**. 0.1.2 shipped on the
  v0.2.0 release; this changes its code, and the standing note already
  says the next build's bytecode differs — the bump resolves it.
- `editors/jetbrains-tm`: **stays 0.1.0** — never released, same logic
  that kept `TM1_TMA_DIALECT_VERSION` at 0.3.
- `MIN_TESTED_PMT` / `MIN_TESTED_TMT` floors: unchanged — the provider
  consumes only the servers' existing legends.
- VS Code extensions: untouched.

Both plugin READMEs' sideload checklists gain a semantic-colors row;
checklists ship unticked — live sideload verification remains the
maintainer's post-merge step (standing convention). This closes #51
(prong 1 shipped 2026-07-21 in `2c55eab`).

### Verification

`./gradlew buildPlugin` green for both pairs (JBR per the READMEs;
`jvmToolchain(17)` provisions the compile JDK). There is no automated
rendering test — the checkable surface is the build, the EP wiring
(a bad `class` attribute fails plugin load, which sideload surfaces),
and the mapping table itself, which this spec pins.

---

## Process

- Branch: `feat/pre-cut-round-62-67-51` off master `5b729a8`; one
  merge at the end; per-item conventional commits
  (`feat(turing-machine):` / `docs(pmt):` `docs(tmt):` /
  `feat(editors):`), fix rounds as NEW commits, never `--amend`.
- Execution: subagent-driven; the controller runs the slow gates
  (`cargo test --workspace`, workspace clippy, `cargo fmt --check`,
  both `buildPlugin`s) — one cargo invocation at a time; agents get
  scoped commands only.
- Standing gates: PM-1 byte-identity, derivation-first goldens (never
  regenerated from run output), `-O0` bit-identity, `.tma` corpus
  fmt-cleanliness, completions/grammar drift guards.
- Version spaces: **nothing moves** except the jetbrains-pm plugin
  0.1.2 → 0.1.3. `.tmc` 0.1 / `.tma` dialect 0.3 / TM IR 2 / container
  versions / both manifest schemas 0.2 — all untouched by this round.
- CHANGELOG: no entry this round — the feature rides the release cut's
  version block, per every TM-arc round before it.
- Published docs stay forge-agnostic (no issue numbers, no `spec §N`,
  no `D<n>`-style internal labels in code comments or docs).
- Closes at merge: #62, #67, #51.
