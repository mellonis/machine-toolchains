# Probe 2 — a routine generic over its alphabet (issue #121, line 2)

Question: can a routine say "I need only the blank and these symbols; pass
everything else through", and be used over three different alphabets from
one machine? Toolchain: `tmt 0.5.0`, `.tmc` 0.1, `.tma` 0.3 (`target/release/tmt`).

Files here: `lib.tmc` (the library: `skipRight`, `swapAB`, `swapABopen` over
`ab`), `lib_o.tmc` (the spare-symbol variant for B.3), `run.sh` (the whole
matrix; writes `run.log` and `gen/`), `hand_open_*.tma` (the Part B.1
hand-authored descriptors). Re-run with `./run.sh`.

The three alphabets, one tape per program (a transparent call can only
reach tape 0, see A.i):

| name | glyphs | tape used |
|---|---|---|
| `ab` | `'_','a','b'` | `abba` |
| `abcd` (superset) | `'_','a','b','c','d'` | `acbd` |
| `xyab` (same symbols, other indices) | `'_','x','y','a','b'` | `xayb` |

## Part A — every spelling the language offers

### A.i — transparent argless call, cross-unit (`lib.tmo` linked by symbol)

```
tmt compile lib.tmc -o gen/lib.tmo
tmt compile gen/i_<routine>_<alpha>.tmc -o …   # `use lib::skipRight;` + `call skipRight()`
tmt link gen/i_….tmo gen/lib.tmo -o … ; tmt run … --tape-block …
```

| routine | ab | abcd | xyab |
|---|---|---|---|
| `skipRight` | `abba_` head 4, exit 0 | `acbd_` head 4, exit 0 | `xayb_` head 4, exit 0 |
| `swapAB` (rows a,b,_) | `baab_`, exit 0 | **`Trapped(NoTransition)`** at 'c', exit 3 | **`Trapped(NoTransition)`** at 'a' (after x→y by index), exit 3 |
| `swapABopen` (rows a,b,_,`*`) | `baab_`, exit 0 | **`bcad_`, exit 0** — c,d passed through | `yaxb_`, exit 0 — **x↔y swapped, a,b untouched** (index binding) |

Findings:

- A transparent call is already "open": there is no frame, no map and
  no cardinality check on a plain call site (the #95 analysis, item 4,
  confirmed here — `lib::skipRight` with `alpha=(3)` runs over a 5-symbol
  band without a word from the linker). The callee's `*` row matches the
  physical index directly (`crates/core/src/vm/table.rs:92`: a `0x7F` cell
  matches any `tr[pos]`), a `-` keep is not a write, so `swapABopen`
  IS alphabet-generic under this spelling — but only by accident of index
  layout: over `xyab` it swaps `x`↔`y`, because callee index 1/2 are the
  caller's `x`/`y`. This is the item-4 hazard of #95; the interface
  section's glyph check would turn it into a link warning.
- A transparent call reaches only tape 0 (callee virtual `k` = physical
  `k`), so "three alphabets from one machine" is impossible under this
  spelling — one program per alphabet.
- The "pass everything else through" row `[*]` in `swapABopen` is NOT
  removed by `-O1` (`dead-rows` reasons by same-band cover, not alphabet
  exhaustion — `docs/tmt/lint.md` (dead-rule)) and `tmt lint` is silent
  on it, so a library author can write the row today and it lands in
  the object. Whether that silence is right is a separate lint question
  (over `ab` the row is provably dead).

### A.ii — bound call, omitted map, equal cardinality (in-unit callee)

Binding needs the callee's signature, so `lib.tmc` is pasted into the
unit (`bound()` in `run.sh`; the stdlib header describes the same
"library ships as source" path).

```
call lib::swapAB(t = t)     // t: ab2 {'_','a','b'}  → baab_, exit 0 (mono, frames)
call lib::swapAB(t = t)     // t: xy3 {'_','x','y'}  → yxxy_, exit 0 (index binding)
tmt lint … --warn index-identity-map
  gen/ii_swapAB_xy3.tmc:38:43: lint: call maps by index across differently-glyphed alphabets ('x' vs 'a' at index 1); glyphs change meaning here
```

Equal cardinality only — over `abcd` or `xyab` an omitted map is the
closed empty binding:

```
call lib::skipRight(t = t)  // t: abcd
  mono:   link: … 1 stamp(s) … 4 trap row(s)   → Trapped(UnmappedRead { at: 25 }), head 0 reads 'a', exit 3
  frames: link: … 1 composite(s) …            → Trapped(UnmappedRead { at: 12 }), head 0 reads 'a', exit 3
```

### A.iii — bound call, explicit map naming only the needed symbols, unequal cardinality

`with map { 'a' -> 'a', 'b' -> 'b' }` — the unlisted non-blank symbols are
holes (`docs/tmt/language.md` (unequal alphabets: closed maps and holes)).

**The trap** — `skipRight` (needs only the blank) over `abcd`:

```
$ tmt compile gen/iii_skipRight_abcd.tmc -o gen/iii_skipRight_abcd.tmo
$ tmt link gen/iii_skipRight_abcd.tmo -o gen/iii_skipRight_abcd.mono.tmx --call-mech mono -v
  link: frames: 0 composite(s), 1 stamp(s), 0 B compose table; 0 deduped, 2 trap row(s), 0 expanded row(s)
$ tmt run gen/iii_skipRight_abcd.mono.tmx --tape-block gen/iii_skipRight_abcd.tmt
outcome: Trapped(UnmappedRead { at: 25 })
tape 0: origin 0, head 1 reads 'c'
|acbd|
exit=3
$ tmt link … --call-mech frames   → Trapped(UnmappedRead { at: 12 }), head 1 reads 'c', exit 3
$ tmt link … --call-mech hybrid   → Trapped(UnmappedRead { at: 12 }), head 1 reads 'c', exit 3
```

The stamped copy (mono) shows why: the two unlisted physical symbols got
synthesized exact trap rows, sorted ahead of the callee's `*` row:

```
T0:     .row    [0]
        .row    [3]      ; 'c' → trap stub
        .row    [4]      ; 'd' → trap stub
        .row    [*]
T1:     .targets L0010, L0019, L0019, L0011
```

and the frames sidecar records them as read holes:
`"read_holes": [3, 4]`, label `lib::skipRight@[0{1->1,2->2}]`.

Same trap for every other combination — `swapAB` over `abcd` (at 'c'),
`swapABopen` over `abcd` (at 'c': **the `*` row does not help, the trap
row wins**), `skipRight` and `swapABopen` over `xyab` (at 'x'); all three
mechanisms, always `UnmappedRead`, exit 3. That trap is the missing
feature: there is no spelling for "unlisted symbols are opaque and pass
through".

## Part B — feasibility

### B.1 What "open" means at the composite level

**Not identity — opaque.** The closed rule is one block in
`crates/core/src/linker/compose.rs:377-380` (`if caller_card != card {
close_unlisted(&mut rmap, caller_card); close_unlisted(&mut wmap, card) }`,
`close_unlisted` at `:429-438` inserting every unlisted `1..card` into the
hole set). Replacing "hole" by "identity" is unsound: over `xyab` the
unlisted `x`=1, `y`=2 would read as the callee's `a`=1, `b`=2. The sound
reading of "open" is: an unlisted caller symbol maps to a virtual index
the callee never names — any index `>= callee_card`; one shared index
(`callee_card` itself) suffices, since the callee cannot distinguish
opaque symbols from one another anyway.

**Mono.** `build_stamp` (`crates/core/src/linker/stamp.rs:765-781`) splits
physical symbols into `preimage` (has an image `< callee_card`,
`read_image` at `:722-729`) and `holes`; `rewrite_match_table`
synthesizes one exact trap row per hole (`:1179-1187`) and keeps a
callee `*` cell as `*` (`:1197-1199`, "wildcard stays wildcard"). So a
`*` row DOES survive and WOULD match the unlisted physical symbol — it
loses today only because the trap row is exact and sorts ahead. Open
mono = put the unlisted symbols in neither set: no trap row, no
preimage, the `*` row catches them, an exact row never does. Writes:
`project_writes` (`:1033-1045`) skips `0x7F` keep before consulting
`write_image`, so a keep never crosses `wmap`; a named write goes
through its two-way pair as today; the callee has no name for an opaque
symbol so it can never write one. Sound with no callee-side promise.

**Frames.** `virt_symbol` (`crates/core/src/vm/core.rs:855-869`) returns
`Ok(v)` for any dense entry `!= 0xFFFF` — it does not range-check `v`
against the callee cardinality; `MatchWalk` compares `b == 0x7F ||
b == tr[pos]` (`table.rs:92`), so an exact row never matches an
out-of-range `v` and a `*` row always does; a `-` keep emits no `Write`
micro-op at all (`crates/turing-machine/src/arch/mod.rs:165`), so
`phys_symbol` (`core.rs:874-888`) is never reached for it. **There is
room in the descriptor format as it stands** — `rmap` is `rmap_len × u16`
indexed by physical symbol (`docs/formats.md` (frame descriptors)); the
only thing that turns an unlisted symbol into `0xFFFF` is the linker's
`dense_map` (`crates/core/src/linker/engine.rs:731-753`, `Some(v) if v <
codomain_card`) and the compose-side hole. The assembler validates only
blank pinning and the `0xFFFE` ceiling (`crates/core/src/asm/lower.rs:635-672`).
No new descriptor flag.

**Proof by hand-authored descriptor, today's binary, no code change:**

```
; hand_open_abcd.tma — swapABopen alpha=(3) over an alpha=(5) band
F0:     .frame  tapes=(0)
        .map    0, rmap=(1->1, 2->2, 3->3, 4->3), wmap=(1->1, 2->2)
$ tmt asm hand_open_abcd.tma -o gen/hand_open_abcd.tmo
$ tmt link gen/hand_open_abcd.tmo --nostdlib --call-mech frames -o gen/hand_open_abcd.frames.tmx
$ tmt run gen/hand_open_abcd.frames.tmx --tape-block gen/iii_swapABopen_abcd.tmt --trace
  … 000c: rd  ; heads=[1] FR=1 → mtc → djmp → 0028: wrmv [-], [>]   ; 'c' read as virtual 3, took the `*` row, kept
outcome: Stopped
|bcad_|      exit=0

; hand_open_xyab.tma — x,y opaque (→3), a→1, b→2, write-back 1→3, 2→4
        .map    0, rmap=(1->3, 2->3, 3->1, 4->2), wmap=(1->3, 2->4)
outcome: Stopped
|xbya_|      exit=0
```

(Under `--call-mech mono` the linker left the raw `call.m` as a framed
call and the image ran identically — nothing to stamp.)

### B.2 The decomposition, sized

(a) **Callee-side declaration** ("generic over the rest" on a tape
parameter). What it would check: every row's cell on that tape is a
named glyph or `*`, every write a named glyph or `-` — but that is
already true of every `.tmc` routine by construction (a row can only
name glyphs of its alphabet; there is nothing else to write). The one
checkable fact with content is "every state has a `*` row on this tape"
(never `NoTransition` on an opaque symbol); the other useful fact,
"writes over a `*`-matched cell", is `writes` already. So (a) is a
*contract*, not a soundness requirement: a compile-time check that the
routine is total over unnamed symbols, plus a per-tape flag in the
interface section so the linker can refuse an open binding against a
callee that never asked to be generic. Size: small (one clause keyword
in the signature grammar, one totality walk over expanded rows, one bit
per tape in the `.param` line of the #95 interface section). Risk: low;
optional for the mechanism.

(b) **Binding-side spelling** for "open": e.g. `with map { 'a' -> 'a',
'b' -> 'b', * }` or `with open map { … }`. Compiler: one flag on the
tape binding, emitted into the bound-site record (one flag bit in the
`TapeBinding` wire form — a MO change, which the #95 arc is already
making when the record becomes a bound *site*, so it rides that bump for
free; standalone it is an MO 3 → 4 bump on its own). `.tma` operand
grows the same marker (`[0{1->1, 2->2, *}]`) for the text-expressibility
gate. Size: small. Risk: low.

(c) **Linker.** `binding_to_composite`: under the flag, instead of
`close_unlisted(&mut rmap, caller_card)`, map every unlisted non-blank
source symbol one-way to the opaque index `callee_card` (explicit pairs,
so `SparseMap` keeps its identity default and the composition laws are
untouched — an opaque symbol reaching a further *closed* binding becomes
a hole there, correctly; reaching a further *open* one becomes opaque
again). `close_unlisted(&mut wmap, card)` stays (write holes for callee
symbols with no write-back are unchanged). `read_image` and `dense_map`
learn one extra legal value (`== callee_card`): mono puts it in neither
`preimage` nor `holes`; frames emits it in the dense rmap. The canonical
label grammar (`binding_label.rs`) needs one token for it. Hybrid's
classifier already routes anything non-bijective to frames. Size: ~4
touch points in core, all small; the everything-matrix gains open
programs. Risk: low — the runtime semantics were exercised above with
the current VM. **(c)-frames is sound**: the physical index never reaches
the callee's table; the descriptor's `u16` carries an out-of-range
virtual index, which only `*` matches (`table.rs:92`), and the only
"write" of an opaque cell a callee can express is `-`, which is not a
write (`arch/mod.rs:165`). A callee with no `*` row in some state traps
`NoTransition` on an opaque symbol — its own semantics, and exactly
what (a) can check away.

### B.3 The alternative without new machinery: one-way collapse

```
; read-only callee: collapse onto ANY existing symbol
call lib::skipRight(t = t with map { 'a' -> 'a', 'b' -> 'b', 'c' => 'a', 'd' => 'a' })
  mono:   0 trap row(s) → Stopped, |acbd_| head 4, exit 0
  frames:                 Stopped, |acbd_| head 4, exit 0

; writing callee, collapsed onto a symbol it DISTINGUISHES: no trap, wrong answer
call lib::swapAB(t = t with map { … 'c' => 'a', 'd' => 'a' })
  mono/frames: Stopped, |bbab_|, exit 0        ; c,d read as 'a' and were "swapped" to 'b'

; writing callee with a SPARE opaque symbol (lib_o.tmc: abo {'_','a','b','o'}, ['o'] -> move [>])
call libo::swapABo(t = t with map { 'a' -> 'a', 'b' -> 'b', 'c' => 'o', 'd' => 'o' })
  mono (1 expanded row), frames, hybrid: Stopped, |bcad_|, exit 0      ; abcd
call libo::swapABo(t = t with map { 'a' -> 'a', 'b' -> 'b', 'x' => 'o', 'y' => 'o' })
  mono, frames:                          Stopped, |xbya_|, exit 0      ; xyab

; the same collapse, callee WRITES 'o' back (stampO): a write hole
call libo::stampO(t = t with map { … 'c' => 'o', 'd' => 'o' })
  mono:   Trapped(UnmappedWrite { at: 32 }), head 1 reads 'c', exit 3   ; stamped: L0020: trap #1
  frames: Trapped(UnmappedWrite { at: 39 }), head 1 reads 'c', exit 3
```

Plainly: **keep bypasses the write map entirely** (it is not a write —
`arch/mod.rs:165` emits no `Write` micro-op; `stamp.rs:1044` skips it),
so a one-way collapse already gives "generic over the rest" TODAY for a
routine that only keeps the symbols it does not own. A read-only routine
needs no spare symbol (collapse onto anything non-blank the routine
treats the same way — for `skipRight` any non-blank). A routine that
distinguishes its own symbols needs the library author to have reserved
a spare "opaque" symbol in its alphabet, and the caller to collapse
every foreign symbol onto it — which is exactly what an open binding
would automate: the opaque index is the spare symbol, synthesized by the
linker at `callee_card` instead of declared by the author at
`callee_card - 1`. Two costs of the manual form: the caller must
enumerate every foreign symbol (a map that grows with the caller's
alphabet, not the callee's needs), and the spare pollutes the callee's
alphabet (its `writes` set, its rows, its glyph budget).

## Verdict

**Worth adding, small, and it rides the #95/#118 arc.** The machine-level
mechanism exists today under both call mechanisms — proven by the
hand-authored descriptors and by the one-way-collapse runs — and the
language lacks only the word. The smallest design:

1. Binding side: `*` as the last entry of a `with map { … }` (or a
   keyword), meaning "every unlisted non-blank caller symbol is opaque:
   readable only through `*`, keepable, never writable". One flag bit on
   the tape-binding record and the same marker in the `.tma` operand —
   both ride the bound-site record rewrite in the #95 arc (MO 3 → 4).
2. Linker: in `binding_to_composite`, open ⇒ unlisted read symbols
   collapse one-way onto the opaque index `callee_card`; `read_image`
   and `dense_map` accept that index; mono synthesizes no trap row for
   it, frames emits it in the dense rmap. Label grammar gains one token.
3. Callee side (optional contract, adds checking not soundness): a
   per-tape `generic` clause in the signature — the compiler verifies
   every state carries a `*` row on that tape (total over unnamed
   symbols); it is exported through the interface section's `.param`
   line so the linker may refuse `*` against a non-generic callee, and
   `tmt interface` prints it. Depends on the interface section (#95
   item 1).

Without (3) the feature is still complete and sound; (3) is what turns
"generic" from a caller's assertion into a callee's checked promise —
the same shape as `writes`, which is why it belongs in the interface
section rather than in prose. Index binding plus the item-4 warning is
NOT the honest limit: it covers only same-cardinality reuse and only by
luck of index layout (`swapABopen` over `xyab` swapped `x`↔`y`).
