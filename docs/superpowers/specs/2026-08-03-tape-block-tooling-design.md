# Tape-block tooling — design

**Date:** 2026-08-03
**Status:** awaiting maintainer review of this document
**Tracked by:** [#61](https://github.com/mellonis/machine-toolchains/issues/61)
**Scope:** `pmt` / `tmt` tape authoring and inspection surface. No engine,
compiler, linker, or assembly change.

## 1. Problem

Authoring a tape is step one of running anything, and it is the worst surface
in the toolchain. Four complaints, each reproduced against a real three-tape
`.tmc` program (`pow2.tmc`: `tape main: mainAlpha`, `tape cnt: workAlpha`,
`tape tmp: workAlpha`, with `alphabet mainAlpha { ' ', 's', 'b', 'k', '1' }`).

### 1.1 The command names the wrong object

The subcommand is `tape`, but the thing it reads and writes is a *block* —
a whole band of tapes. The vocabulary is already right everywhere else: the
completions registries describe these files as "tape-block snapshots"
(`post-machine/src/completions/registry.rs:591`,
`turing-machine/src/completions/registry.rs:616`) and `docs/formats.md` calls
the format a tape-block snapshot throughout.

The confusion is not cosmetic — it has produced a live flag collision across
the twin CLIs:

| CLI | flag | means |
|---|---|---|
| `pmt run` | `--tape " * *"` | an inline glyph literal for one tape |
| `tmt run` | `--tape TAPES.tmt` | a whole multi-tape block |
| `pmt run` | `--tape-block IN.pmt` | a whole block |

Same flag name, two different concepts, in sibling tools.

### 1.2 A block must be built one band at a time

`tape set` edits the single band named by `--tape N` (default `0`). A
three-tape block therefore takes three chained clone-and-edit invocations.

### 1.3 A block minted from an image has index labels, not glyphs

`tmt tape new --from pow2.tmx` produces:

```
alphabet: ["0", "1", "2", "3", "4"]
```

This is correct by contract, not an oversight. `docs/formats.md` (312–324)
rules that glyphs live only on the tape side; code-side artifacts — objects,
executables, and the map sidecar — carry symbol indices only, matching the
hardware-realizability rule that the processor never sees glyphs. `.tmx`
carries `alphabet_cardinalities` and nothing else, so decimal labels are all
`--from *.tmx` can possibly know.

The consequence is that the author hand-encodes glyphs as indices. The
observed tape `sb111k` had to be written `124443`, and there is no way to
attach the real glyphs to the block afterwards.

### 1.4 Rendering is ambiguous

`render_tape` concatenates glyphs with no delimiter
(`turing-machine/src/cli/mod.rs:106`, `post-machine/src/cli/mod.rs:96`), so
a block renders `|124443|`. With multi-character glyphs this is genuinely
ambiguous: `|011|` could be the three cells `0`,`1`,`1` or the two cells
`0`,`11`. `show` also prints only the block's fallback alphabet, so which
alphabet a given band actually uses is not visible at all.

## 2. Rulings

Each ruling records the decision and why it beat the alternatives.

**R1 — `tape` is renamed `tape-block` on both CLIs, and the run flags realign
with it. Hard rename, no aliases.** `--tape-block` always names the whole
block; `--tape*` never names two different things. Aliases were rejected:
they need an exemption in the completion drift guards (which probe the real
parser) and something has to delete them before 1.0. The project is pre-1.0
and the rename is CHANGELOG-visible.

**R2 — MT stays at v2. Tape names are never stored in the container; glyphs
are pinned by tape index.** Tapes are addressed on the bus by number; names
are a `.tmc` construct that exists so `call` / `graft` can map tapes by
position, and they have no referent once a program is linked. The container
never needed to answer "what is tape 1 called", only "what glyphs does tape 1
use" — which v2 already answers.

This also keeps a version space off the pre-release path. A "names present"
bit in the header's reserved `flags` byte was rejected outright: both reader
paths *discard* flags (`core/src/formats/tapeblock.rs:114`, `:176`) rather
than validating them, so an older build would read a name's `u16` length
prefix as the next tape's `i64` origin and silently misparse. The version
field, by contrast, is validated (`_ => Err(FormatError::UnsupportedVersion)`),
so a hypothetical v3 would at least refuse cleanly — but v3 is moot under R2.

**R3 — `tape-block new` has two provenance paths, and `--from` is optional.**

- `--from APP.tmx` / `APP.pmx` — tape count and per-tape cardinality from the
  image header; glyph labels default to today's decimal strings.
- `--from APP.tmc` (TM only) — tape count, cardinality, **glyphs, and tape
  names** from the `machine` block's tape declarations (the entry world; a
  source with no `machine` block is a library and is an error here).
- no `--from` — the block is defined entirely by the edit flags.

`--from` dispatches on `sniff()`, never on the file extension, per the
standing container rule in `docs/formats.md`. A file that sniffs as a
container is an image; one that does not is source text.

PM gets no `--from *.pmc`: PM-1's alphabet is fixed at two glyphs, so there
is nothing to extract from source.

**R4 — glyph lists reuse `.tmc` alphabet-element notation.** Quoted symbols,
comma-separated, with `lo..hi` ranges. This is byte-identical to what an
author already writes inside `alphabet { … }`, so it copy-pastes; it handles
space, comma, and quote glyphs without inventing escaping rules; and ranges
come free, because `AlphabetElem::{Single, Range}`
(`turing-machine/src/parser.rs:124`) is already expanded by
`resolve_alphabet_glyphs`.

```
--alphabet "0=' ','s','b','k','1'"
--alphabet "data='0'..'9'"
```

**R5 — edit-flag keys are tape indices; a tape name is additionally accepted
when the invocation has a `.tmc` in hand.** The name is resolved to an index
at parse time and never stored, so R2 holds. Without a source in scope, a
name key is an error naming the flag that would supply one.

**R6 — `--alphabet` works on `set` as well as `new`, and it relabels rather
than re-maps.** Cell *indices* are untouched; only the glyph table is
replaced. This is the only coherent reading when the old labels are `0`…`4`
and share no glyph with the new table, and it is what makes an
already-authored block repinnable in place.

Where a cardinality is **already established** — on `set`, and on `new
--from` where the image header supplied it — the new table must have exactly
that many glyphs.

**The check measures the tape's *effective* cardinality**, never the
cardinality of whichever table physically gets written. This matters because
the two differ: a TM block minted by `--from *.tmx` has a block fallback
sized to `max(cardinalities)` with per-band overrides at each band's own
cardinality (`turing-machine/src/cli/inspect.rs:164–172`), so for `pow2`
the fallback is 5 wide while tapes 1 and 2 are 2 wide. Measuring the tape
keeps R6 and the PM block-alphabet rule of §4.7 composable. Cells are validated against the effective alphabet on read
(`docs/formats.md`, MT v2), so a shrinking repin would strand any cell
holding a now-out-of-range index and make the block unloadable. Because
`--from APP.tmx` takes cardinality from the image header, a pure relabel is
all that path ever needs.

On `new` with no `--from` there is no established cardinality: `--alphabet`
*defines* it, and the rule does not apply.

**R7 — edits are keyed and repeatable, so one invocation authors a whole
block.** This is the direct answer to §1.2.

**R8 — `show` delimits adaptively, with explicit overrides.** Dense when
every glyph in the effective alphabet is a single character (`chars().count()
== 1`, the measure `render_tape` already uses for its caret line), where
ambiguity is impossible; separated when any glyph is longer. The choice is
made per band, from that band's effective alphabet. `--dense` / `--separated`
force either form for stable output. A blanket separator was rejected: it
would turn PM-1's readable `| *** |` into `| | |*|*|*| | |`, a regression for
the machine whose entire alphabet is two glyphs.

**R9 — `show` prints the effective alphabet per band**, not one header line
for the block fallback.

**R10 — `tmt run` gains `--save-tape-block`**, the twin of PM's. Left as an
asymmetry the flag realignment would have made look arbitrary.

The saved block must carry **each band's own glyph table** through, not a
single block alphabet. PM's save clones one alphabet
(`post-machine/src/cli/run.rs:208`) because a PM block has exactly one; a TM
save that did the same would round-trip a repinned three-tape block into a
single-alphabet file and silently discard the glyphs just pinned — the same
class of failure as §3. Covered by a round-trip test: `new` → `set
--alphabet` → `run --save-tape-block` → `show` renders the pinned glyphs.

**R11 — `tmt run` gains a per-tape cardinality check.** It currently
validates band count only (`turing-machine/src/cli/run.rs:123`), while
`render_tape` falls back to `"?"` for an out-of-range index
(`cli/mod.rs:111`). A block whose alphabet is too small for the program
therefore loads and silently renders `?`. `exe.alphabet_cardinalities` is
already in hand one line away. This is the safety net that makes the
no-`--from` authoring path of R3 sound.

## 3. Defect found during design

PM ignores per-tape glyph overrides in **two** places, rendering through the
block fallback instead:

1. `pmt tape show` renders every band through `&block.alphabet`
   (`post-machine/src/cli/inspect.rs:305`).
2. `initial_tape` returns `file.alphabet` for a loaded block
   (`post-machine/src/cli/run.rs:277`), so `pmt run --tape-block` renders —
   and, via `--save-tape-block`, *rewrites* — through the fallback too.

Both are wrong, although `tape_set` resolves overrides correctly
(`inspect.rs:258–260`), and both TM sites do: `tape_show`
(`turing-machine/src/cli/inspect.rs:329`) and `execute_run`
(`turing-machine/src/cli/run.rs:134–138`).

Reproduced with a hand-built MT v2 block: one tape, block fallback
`["A","B"]`, per-tape override `["x","y"]`, one cell holding index `1`. The
same 46 bytes render `|y|` under `tmt tape show` (correct) and `|B|` under
`pmt tape show` (wrong).

The defect is unreachable today, by luck in both directions: PM's own tools
never write per-tape overrides, and TM's decimal labels can never disagree
with the block fallback. The second reason is worth pinning precisely,
because it is what makes the fixture necessary rather than redundant:
`labels(card)` is `(0..card).map(|i| i.to_string())`, so a label depends
**only on the index**, never on the cardinality — `labels(n)[k] ==
labels(m)[k]` for every `k` valid in both. Change the default labeller and
this "unreachable" property expires silently. **This round activates the defect** — repinning is exactly
what makes an override diverge — and `.pmt` / `.tmt` are the same container,
so `pmt tape show` will read a TM-authored block and render it wrong.

Fix: resolve the effective alphabet at **both** sites, matching `tape_set`
and the TM twins. The `run` site is the more dangerous of the two — paired
with `--save-tape-block` it does not merely display the wrong glyphs, it
persists them.

## 4. CLI surface

### 4.1 Edit flags (shared by `new` and `set`, repeatable)

```
--alphabet KEY=GLYPHS    replace tape KEY's glyph table (relabel; R6)
--cells    KEY=GLYPHS    replace tape KEY's cells
--head     KEY=N         set tape KEY's head
--origin   KEY=N         set tape KEY's origin
```

`KEY` is a tape index, or a tape name when a `.tmc` is in scope (R5).
`GLYPHS` is `.tmc` alphabet-element notation (R4). In `--cells`, each glyph
is looked up in the tape's effective alphabet **as of after this
invocation's `--alphabet` edit** — the newly pinned table, not the one
loaded from disk — and stored as its index; a glyph absent from that
alphabet is an error. Today's `tape_set` resolves against the loaded block
(`post-machine/src/cli/inspect.rs:258–260`), so this is a deliberate change,
and it is what makes `--alphabet` and `--cells` usable together in one call. An empty value (`--cells 0=`)
means a zero-length tape. A range in `--cells` expands to one cell per
glyph, so `--cells "0='0'..'9'"` writes ten cells.

**Application order is fixed:** for a given tape, `--alphabet` applies before
`--cells`, so cells in the same invocation resolve against the newly pinned
glyphs. Repeating the same flag for the same tape is an error rather than
last-wins.

On PM, `--alphabet` writes the block alphabet rather than a per-tape
override — see §4.7.

### 4.2 `tape-block new`

```
pmt tape-block new [--from APP.pmx] [-o OUT.pmt] [EDITS]
tmt tape-block new [--from APP.tmx | --from APP.tmc] [-o OUT.tmt] [EDITS]
```

Without `--from`, tape count is the number of `--alphabet` flags, whose keys
must be contiguous from `0`. No `--tapes` flag: contiguity is self-checking,
and a mistyped key produces a precise error rather than a silently oversized
block. TM requires an `--alphabet` per tape in this mode, having no fixed
alphabet; PM has a fixed pair, so a bare `pmt tape-block new` is a one-tape
empty block.

```
tmt tape-block new --from pow2.tmx \
  --alphabet "0=' ','s','b','k','1'" \
  --alphabet "1=' ','1'" \
  --alphabet "2=' ','1'" \
  --cells    "0='s','b','1','1','1','k'" -o in.tmt

tmt tape-block new --from pow2.tmc \
  --cells "main='s','b','1','1','1','k'" -o in.tmt
```

### 4.3 `tape-block set`

```
{pmt|tmt} tape-block set IN (-o OUT | --in-place) [--from APP.tmc] [EDITS]
```

Clone semantics as today. `--from APP.tmc` exists solely to make name keys
resolvable; it never reshapes the block.

```
tmt tape-block set pow2^3.tmt --in-place --alphabet "0=' ','s','b','k','1'"
```

### 4.4 `tape-block show`

```
{pmt|tmt} tape-block show FILE [--dense | --separated]
```

```
tape 0: origin 0, head 0, alphabet [' ','s','b','k','1']
|sb111k|
 ^
tape 1: origin 0, head 0, alphabet [' ','1']
||
```

### 4.5 `tape-block build` (PM only)

Unchanged. It hard-codes the PM-1 pair (`'*' => 1`,
`post-machine/src/cli/inspect.rs:139`) and stamps `DEFAULT_GLYPHS` — genuine
fixed-alphabet sugar that only ever creates blocks, never edits them.

### 4.6 Run flags

| CLI | today | after |
|---|---|---|
| `pmt run` | `--tape-block IN.pmt` | unchanged |
| `pmt run` | `--tape " * *"` | `--tape-cells " * *"` |
| `pmt run` | `--save-tape-block OUT.pmt` | unchanged |
| `tmt run` | `--tape TAPES.tmt` | `--tape-block TAPES.tmt` |
| `tmt run` | — | `--save-tape-block OUT.tmt` (R10) |

### 4.7 PM specifics

PM gets the rename, keyed edits, adaptive `show`, and the §3 fix. Repinning
works and is verified end to end: `initial_tape` returns the loaded block's
alphabet (`post-machine/src/cli/run.rs:277`), falling back to
`DEFAULT_GLYPHS` only when no block is supplied (`:288`, `:290`), and
`--save-tape-block` writes that alphabet back (`:208`).

Because a PM block is single-tape and single-alphabet, `--alphabet 0=…` on PM
writes the **block** alphabet and leaves the per-tape override `None`, so the
file stays MT v1 (`to_bytes()` selects v1 via `is_v1_shape()`,
`core/src/formats/tapeblock.rs:42`) and PM's byte-compared goldens are
untouched. TM continues to write per-tape tables.

## 5. Implementation

### 5.1 Glyph-list notation needs one owner

Both CLIs must parse R4 notation, but the `.tmc` alphabet parser lives in the
TM crate and PM cannot reach it. A glyph list is arch-agnostic, so it belongs
in `mtc-core` beside the MT codec, used by both CLIs, and pinned with a
**bidirectional drift guard** asserting it agrees with TM's
`resolve_alphabet_glyphs` over a shared corpus — matching how this repo
already guards grammars, directives, and error codes.

Honest cost: roughly 100 lines of symbol-literal lexing duplicated in core.
The alternative — each crate rolling its own — drifts with no guard at all.

### 5.2 Touch list

| file | change |
|---|---|
| `core/src/formats/` (new module) | glyph-list parser + drift guard |
| `{post-machine,turing-machine}/src/cli/inspect.rs` | the `tape-block` subcommand; PM `show` fix (§3) |
| `{post-machine,turing-machine}/src/cli/run.rs` | flag renames; `tmt --save-tape-block`; `tmt` cardinality check |
| `{post-machine,turing-machine}/src/cli/mod.rs` | dispatch, `USAGE`, `render_tape` + its two unit tests |
| `{post-machine,turing-machine}/src/completions/registry.rs` | renamed paths and descriptions |
| `{post-machine,turing-machine}/src/completions/zsh.rs` | nested-path special-cases (`zsh.rs:168`, `:399`) |
| PM `tests/completions_registry.rs` | `EXPECTED_TOP_LEVEL` mirror |

### 5.3 Errors

Every case below is a typed, spanned CLI error, not a panic:

- repin cardinality mismatch — *tape 0 has cardinality 5, the given alphabet has 3 glyphs*
- non-contiguous `--alphabet` keys in the no-`--from` path
- a TM tape with no `--alphabet` in the no-`--from` path
- a name key with no `.tmc` in scope, and an unknown tape name
- a tape index past the block's tape count
- a `--cells` glyph absent from the tape's effective alphabet
- a block whose per-tape cardinality disagrees with the image, at `run` (R11)
- `-o` together with `--in-place` (existing behaviour, retained)

## 6. Testing

The rename is guarded for free: the completion drift guards probe the real
parser, and the `cli_docs` guard quotes `tmt --help` verbatim, so both fail
until every surface is updated.

New coverage:

- whole-block authoring in a single `new` invocation, both provenance paths
  and the no-`--from` path
- **the repin invariant** — cell bytes byte-identical across an `--alphabet`
  edit, which is the relabel-not-remap contract stated directly
- same-cardinality rejection; contiguous-key rejection; name-key-without-source
  rejection
- `run` cardinality mismatch
- adaptive vs `--dense` vs `--separated` rendering
- **§3 regression fixture** — a committed MT v2 block whose per-tape override
  disagrees with the block fallback. That disagreement is the only shape that
  can catch the defect, and no existing tool emits it.

Standing gate: **every committed golden stays byte-identical** — 3 `.pmt` at
MT v1, 10 `.tmt` at MT v2, none regenerated. Goldens are derivation-first
here, so a changed golden means a changed contract, not a changed renderer.

## 7. Documentation

- `docs/formats.md` — the `.pmt`/`.tmt` CLI examples (326–330), and the
  paragraph at 312–324 stating that decimal labels are what "the author then
  edits or replaces": there is now a mechanism, and it should be named.
- `docs/pmt/cli.md`, `docs/tmt/cli.md` — the `tape-block` section and the run
  flag reference.

Per the published-docs policy these pages stay forge-agnostic and carry the
substance in prose.

## 8. Version spaces

Crates bump. **Every other version space is unchanged** — MT stays v2 (R2),
and `.pmc` / `.tmc` languages, both `.pma` / `.tma` dialects, both IR
versions, MO, MX, and both project-manifest schemas are untouched. Keeping
this round off the version-space critical path is the direct payoff of R2.

## 9. Non-goals

- A textual tape-block source format. Considered and deferred: it would kill
  all four complaints at once, but a fifth text format implies a new version
  space plus eventual fmt / lint / LSP parity pressure, which is this repo's
  established precedent for every text format it owns.
- Tape names in the container (settled by R2).
- Any assembly-, compiler-, linker-, or engine-side change.

## 10. Follow-ups

- The textual tape-block source format, if the authoring surface still feels
  thin after this round.
- `render_tape` is duplicated across the two CLIs and is now growing a
  delimiting policy. If it grows further it is a candidate for core — but
  only as a **string-returning** helper. Core's thin-renderer rule is that
  library code never prints, so a renderer may compute text and must never
  emit it.
