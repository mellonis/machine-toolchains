# TM-1 architecture + tmt toolchain — master design

- **Date**: 2026-07-16 (design round 2026-07-15..16)
- **Status**: APPROVED (maintainer-reviewed section by section)
- **Tracker**: [#8](https://github.com/mellonis/machine-toolchains/issues/8)
- **Inputs**: frozen PM-1 design spec Appendix A (TM-1 seed), the two design-seed
  comments on #8 (tape projection, alphabet polymorphism, and the 2026-07-12
  alphabet-mapping rules — cited below as "the ruled mapping rules"),
  `docs/examples/brainfuck-utm.tma` (the speculative UTM and its four findings).

This is the master spec for the second machine family: the TM-1 architecture,
its `.tma` assembly dialect, the `.tmc` language, and the `tmt` toolchain.
It is decision-deep across the whole surface (including lint/fmt/LSP/editors);
implementation happens in phases (§17), each with its own plan.

---

## 0. Version block

Spaces **born** by this arc:

| Space | Initial value |
|---|---|
| `TMC_LANG_VERSION` (`.tmc` acceptance contract) | 0.1 |
| `TM1_TMA_DIALECT_VERSION` (`.tma` dialect) | 0.1 |
| `TM_IR_VERSION` (TM IR JSON encoding) | 1 |
| `tmt.json` schema | born tracking the `pmt.json` schema design level at implementation time |
| crate `mtc-turing-machine` (`crates/turing-machine`, binary `tmt`) | workspace versioning |
| TM editor plugins (VS Code + JetBrains) | 0.1.0, floor `MIN_TESTED_TMT` |

Spaces that **move**:

| Space | Change |
|---|---|
| MX container | v2: sectioned layout (code + table section), new header fields. PM-1 keeps emitting v1; readers accept both |
| MO container | new record kinds (generic-routine signatures, call-site bindings) |
| MT container | per-tape glyph tables (was: one shared alphabet per block); v1 files stay readable |
| PM-1 ISA | first additive minor revision: `wrl`/`wrr` (§3.6) |
| `.pma` dialect | 0.2 → 0.3 (`wrl`/`wrr` mnemonics accepted) |
| workspace crates | normal release versioning |

Spaces explicitly **unchanged**: `PMC_LANG_VERSION`, `pmt.json` schema (owned by
the manifest arc, #16). Release notes for this arc open with the full version
block, `unchanged` rows included (existing CHANGELOG convention).

## 1. Goals, scope, delivery order

**Goal**: a multi-tape, wide-alphabet Turing-machine architecture (`arch = 0x02`)
with a C-like-in-form, state-machine-in-substance source language and a full
toolchain (`tmt`) at feature parity with `pmt`, reusing everything arch-agnostic
in `mtc-core`.

**Sharing contract** (from Appendix A, confirmed): `tmt` = new language front
end + TM-1 arch module + thin CLI; the VM core, buses, stack, traps, debug API,
MO/MX/MT codecs, linker, assembler/disassembler frameworks, LSP server
framework, and tape devices are imported from `mtc-core`. Core stays provably
free of TM-1 knowledge the same way it is free of PM-1 knowledge (§7, item 10).

**File extensions**: `.tmc` / `.tma` / `.tmo` / `.tmx` / `.tmt` — full symmetry
with the PM family (the tape-extension/CLI-name collision is precedented by
`.pmt`/pmt and harmless; containers dispatch on magic, never extension).

**Roadmap placement** (maintainer decision, 2026-07-16): this arc executes
**before** the manifest/build execution round (#16 + #11). The manifest
execution round then covers `pmt build` **and** `tmt build` together. The one
shared prerequisite — core `LinkOptions.entry` (task 1 of the committed
manifest plan 1) — is absorbed into phase 5 of this arc.

## 2. TM-1 architecture

**Arch id `0x02`.** Harvard model like PM-1, plus one memory:

| Memory | Contents |
|---|---|
| code ROM | instructions |
| **table ROM** | match tables, dispatch target tables; frames profile adds frame descriptors and the compose table |
| call stack | base profile: return addresses; frames profile: uniform `(return address, saved FR)` pairs |
| N tape devices | index-based (the processor never sees glyphs; glyphs live only in `.tmt`) |

**Registers**: `IP`; `TR` — tuple register, a bank of 16 latches holding the
last `rd` result; `MR` — match register (`u32`; 0 = no row matched); `SP`;
frames profile adds `FR` — index of the active composite frame (0 = the
identity frame: no translation, full machine width).

**Processor profiles**: `base` (no frame hardware; executes `mono`-mode
output) and `+frames` (translation unit + FR + frame cache; executes
`frames`/`hybrid`-mode output). The `.tmx` header carries the required profile;
the VM refuses an image whose profile it does not implement.

**Tape cap**: 16 tapes per machine (the TR bank size / head-latch realizability
bound). The bus allows 256 devices; the self-delimiting encodings do not limit
tape count — the cap is a hardware statement. The image header declares the
actual tape count.

## 3. Instruction set

Two instruction families share one scheme: the **compact family** (7-bit symbol
payloads) and the **n-byte family** for wider alphabets (same continuation idea
within a symbol; capped at 4 bytes per symbol = 28-bit payloads, the format
ceiling). Short forms follow PM-1: `short = far | 0x10`, selected only by
linker relaxation.

### 3.1 Core set

```
rd                ; read ALL heads of the current world into TR (batch tuple)
mtc  T            ; MR := 1-based index of the first matching row of table T (0 = none)
djmp D            ; IP := D[MR]; trap no-transition when MR = 0
wrmv [w], [m]     ; PRIMARY tape op: batched write-then-move (one formal TM step)
wr   [v]          ; write-only form (saves the move vector's bytes)
mov  [v]          ; move-only form
jm / jnm  rel     ; conditional on MR ≠ 0 / MR = 0 (arch-invariant: MF ≡ MR≠0)
jmp  rel
call / call.s / ret
trap #kind        ; explicit typed trap (mono mode's synthesized stubs need it)
stp / hlt / brk / nop
```

Vector notation: write vectors use the reserved payload for "keep" (do not
write this tape), move payloads are stay/left/right. `wrmv` is what a `.tmc`
rule's action compiles to; `wr`+`mov` adjacent pairs elsewhere are fused by the
`fuse_tape_ops` pass (§11.3).

### 3.2 The reserved payload 0x7F

One reserved payload in the compact family means **"transparent at this
position"** in both contexts: in a match row it is the wildcard
(`ifOtherSymbol` semantics under ordered matching), in a write vector it is
the keep marker. Consequence (UTM finding 1): compact-family alphabets hold at
most 127 symbols (indices 0..126); wider alphabets use the n-byte family.

**N-byte family precision** (closes the mixed-width question): symbols are
self-delimiting per position (1–4 bytes; a narrow tape's symbol naturally
takes one byte), so one row freely mixes per-tape widths. The **single-byte
`0x7F` is the transparent marker at any position in BOTH families**; the real
symbol index 127 on a wide tape is therefore encoded in the two-byte form —
the one sanctioned non-minimal encoding in the codec (every other non-minimal
encoding is invalid; the property tests pin both facts).

### 3.3 Frames-profile instructions

```
call.m R, F       ; call through composite frame F: one compose-table ROM lookup,
                  ; push (return address, current FR), FR := composite index,
                  ; load descriptor into the frame cache
retx #k           ; multi-exit return: read exit[k] of the ACTIVE frame,
                  ; pop (address, saved FR) — the popped address is discarded —
                  ; restore FR, jump to exit[k]
```

Stack discipline in the frames profile is uniform: **every** call form pushes
the `(return address, FR)` pair; `ret` and `retx` both pop one entry. A plain
`call` therefore leaves FR untouched — the callee transparently inherits the
caller's frame (this *is* the "frame that carries only a return address";
no special frame kind exists). Nested `call.m` saves/restores FR, so by the
time `retx` runs, the active frame is the body's own frame — its exit table is
the one consulted.

### 3.4 Reserved opcode space

Reserved explicitly: a mapped-access family (per-access hardware remap beyond
the frame cache — the fully dynamic variant this design rejected for v1), and
headroom in the frames group. Additive extension is a minor arch revision.

### 3.5 Traps

| Trap | Source |
|---|---|
| `no-transition` | `djmp` with MR = 0 — the classical "no applicable transition" |
| `unmapped-read` | holey mapping, read direction (§5.4) |
| `unmapped-write` | holey mapping, write direction (§5.4) |
| `invalid-opcode`, device faults, … | as PM-1 |

The distinction between `no-transition` and the unmapped-symbol traps is a
contract preserved by **every** boundary mechanism (mono, frames, graft).

### 3.6 PM-1 companion revision: `wrl` / `wrr`

The batched write+move idea back-ports to PM-1 as its first additive ISA
revision (free opcode slots `0x07`, `0x0F`):

| Opcode | Mnemonic | Operand | Meaning |
|---|---|---|---|
| `0x07` | `wrl` | symbol vector | write + head left (≡ `wr x; lft`, MF latching identical by construction) |
| `0x0F` | `wrr` | symbol vector | write + head right |

Surfaced as a `-O1` peephole pass `fuse_tape_ops` — codegen is untouched, so
`-O0` bit-identity holds; no fusion across `brk` (observability barrier).
Tail: `pm1_syntax()`, disassembler, `.pma` dialect 0.2→0.3, lint/fmt/LSP/
completions tables, TextMate grammar, `docs/isa.md`. Policy stated by this
spec: additive opcodes = minor ISA revision; an older VM traps invalid-opcode
on them (acceptable — the in-repo VM is the only hardware).

## 4. Match tables

A match table lives in the table ROM as three subsections, in this order:

1. **exact rows** — no wildcards, pairwise disjoint, sorted by symbol vector →
   binary search. The assembler **verifies** sortedness and disjointness; it
   never sorts silently (that would move MR numbering out from under the
   source).
2. **wildcard rows** — scanned in **encoded (source) order**; first match wins.
   Order is semantics: the author/compiler controls priority, which is exactly
   what gives a wildcard position `ifOtherSymbol` meaning ("fires only for
   combinations no earlier row claimed"). No specificity auto-ordering exists —
   wildcard-count does not totally order rows (`[1,*]` vs `[*,2]`), and
   auto-sorting would take priority control away from the author.
3. optional **catch-all** `[*,…,*]`.

`MR :=` the 1-based encoded row number. Dispatch target tables are vectors of
code addresses, patched by the linker like any relocation. Monomorphized copies
of `M < N` routines land entirely in the wildcard subsection (their unbound
positions are wildcards) — expected and fine.

Reconciliation with the language's "rule order = priority" (§10.3): priority
only *means* anything among overlapping rows. Exact rows are pairwise disjoint,
so their relative order is semantics-neutral — the compiler is free to sort
them into the exact subsection (MR numbers and dispatch targets are emitted
together, so behavior is untouched); source order is preserved as priority
exactly where it matters, among the wildcard rows. Hand-written `.tma` authors
sort exact rows themselves (the assembler verifies rather than sorts, §8.1).

## 5. Call boundaries: tape projection + alphabet mapping

A call site binds caller tapes to callee positions (projection) and maps
caller symbols onto the callee's per-tape alphabets. The ruled mapping rules
apply verbatim: equal-size alphabets identity-complete to a bijection with all
failures static; unequal-size mappings are legal with runtime holes; blank maps
to blank always and implicitly; the two hole directions trap distinctly
(`unmapped-read` / `unmapped-write`).

**One-way pairs `=>`** (added during spec review; resolves the seed's open
collapse question constructively): `src => dst` is a **read-only** map entry —
it participates in the read direction (several caller symbols MAY collapse
onto one callee symbol) and is excluded from the write direction and from the
bijectivity/injectivity checks, which apply to the bidirectional `->` part
only (the implicit blank↔blank pair is always bidirectional). If the callee
writes a symbol reached only via one-way entries, the written-back value comes
from the bidirectional part as usual — i.e. the caller accepts that such cells
are overwritable through the normal map. Mechanics: frames mode — extra rmap
entries, free; mono mode — match rows mentioning a collapsed callee symbol
are expanded per collapsed preimage (row growth is reported by `LinkReport`).
Canonical use: reading foreign boundary markers as the callee's blank
(`'^' => '_', '$' => '_'`) so a bare-representation routine runs inside a
delimited region and returns with the markers intact — sound exactly when the
callee never writes the collapsed symbol, which is statically checkable
(§14's writes-through-collapse lint).

### 5.1 Execution modes (maintainer decision: hybrid with a mode flag)

`--call-mech = mono | frames | hybrid` (link-time option; **hybrid is the
default**). The modes are *processor profiles to compile for*, like ISA
extension sets:

- **mono** → runs on `base` hardware. The linker stamps a specialized copy of
  the callee per distinct (routine, projection, mapping) composite.
- **frames** → requires `+frames`. One generic copy of each routine; calls go
  through composite frame descriptors.
- **hybrid** → requires `+frames`. Per-call-site classification: completed
  bijections are stamped (mono), holey mappings go through frames.

All three modes consume **one link-time composition engine** (§5.2) and MUST be
observably equivalent on the same program and tapes — a CI contract on the
`opt_equivalence` harness pattern (§16).

### 5.2 Static composition (the load-bearing decision)

Bindings compose associatively, and the space of (projection, mapping) pairs is
finite — so the set of distinct composites reachable from `main` is finite and
the linker computes **all of them ahead of time** (the same BFS as reachability,
now over (routine, composite) pairs; deterministic order → reproducible
builds). Nothing composes at runtime:

- Composite descriptors live in table ROM. A descriptor is a list of M entries,
  one per callee virtual tape: `(physical tape, rmap, wmap)` — per-tape maps,
  because alphabets are per-tape. Frames-profile descriptors may also carry an
  **exit vector** (§5.5).
- A shared `call.m` instruction cannot name a different composite per calling
  context, so the linker also emits a **compose table**:
  `compose[active frame][call site] → composite index`. At runtime `call.m`
  performs exactly one ROM lookup; every tape access under a frame performs
  exactly one translation through the frame cache. Cost is O(1) at any call
  depth; the frame stack exists only so `ret`/`retx` can restore FR — it is
  never walked for translation.
- Hole sets compose statically (outer holes ∪ preimages of inner holes).

### 5.3 Mono mode: monomorphization

The linker decodes the generic body via `ArchSyntax` (the property the
disassembler already relies on — self-delimiting operands, sectioned tables —
means **no fixup metadata is needed**) and rewrites: tape positions per the
projection, symbol payloads per the mapping. Projection is vector padding:
match rows get wildcards at unbound positions, write vectors keep-markers,
move vectors stay. Identical composites dedup to one copy.

Holey mappings keep the trap taxonomy statically:

- **unmapped-read**: for each unmapped caller symbol on each bound tape, a
  synthesized first-match trap row (`[*,…,u,…,*] → trap stub`) is prepended to
  every match table of the copy — strict on `rd`, exactly "the head reads a
  symbol with no image". Passing over a hole without `rd` does not trap
  (consistent with the "caller prepares the tape" discipline).
- **unmapped-write**: a `wr`/`wrmv` whose immediate payload has no preimage can
  never execute validly; the linker replaces it with a `trap unmapped-write`
  stub outright.

### 5.4 Frames mode

The generic body executes under the active composite: `rd` reads exactly the M
heads listed in the frame (the instruction carries no operand — the frame
defines the read set), each through its own rmap; writes go through wmap;
sentinel entries in either map raise the corresponding trap. Match tables stay
M-wide as authored; "frame arity == table row width == authored vector width"
is a static signature check — a mismatch never survives to runtime.
`call.m` loads the descriptor into the core's frame cache (TLB-fill flavor:
priced at call time; zero extra bus traffic per access afterwards).

### 5.5 Frame exit vectors and `retx` (in 0.1 by maintainer decision)

Frames-profile descriptors may carry an exit vector — per-instance
continuation addresses as *data*. `retx #k` (§3.3) gives shared bodies
**multi-exit returns**. Primary producer: the `outline` pass (§11.4) in
frames/hybrid mode, sharing one body across a graft group at any exit count.
Hand-written `.tma` may use `.frame exits = […]` + `retx` directly.
Composites do not inherit exit vectors (exits belong to their own frame level;
nested `call.m` saves/restores FR, so `retx` always consults the right table).

The language surface does NOT expose multi-exit calls in 0.1; the ISA decision
deliberately de-risks a post-0.1 language feature (callable graphs:
`call g(…) exits { … }`).

### 5.6 Transparent calls and relaxation

A call without a binding clause is a same-world call: plain `call`, callee
inherits the active frame; zero translation cost. When an explicitly written
binding composes to identity, the algebra collapses it (`E ∘ id = E`, dedup
returns the same index) and a relaxation rule narrows `call.m` → `call`
(shrink-only, monotone — joins the existing `call → call.s` fixpoint).

### 5.7 Revisit trigger

Recorded for the future: per-access hardware remap (the rejected fully-dynamic
variant) becomes worth revisiting only on **measured** instantiation bloat that
`--foutline` cannot reclaim, or a real hardware stand's ROM limit. `LinkReport`
carries the counters that would show it (§9).

## 6. Formats

### 6.1 MX v2 (`.tmx`)

Sectioned image: code + table section, with a section map in the header. New
header fields: tape count (≤ 16), required processor profile, per-tape alphabet
**cardinalities** (numbers only — glyphs never enter the image). PM-1 keeps
emitting v1; `sniff()` + version dispatch as always.

### 6.2 MO (`.tmo`)

Generic routines ship un-instantiated: code + own tables + signature (arity M,
per-tape alphabet cardinality) + **call-site records** (relative bindings:
projection + per-tape maps) — the composition engine's input — plus the usual
exports/imports/relocations. New record kinds = MO version bump.

### 6.3 MT v2 (`.tmt`)

Per-tape glyph tables: each tape carries its own `index → UTF-8 string` table
(emoji and multi-scalar grapheme clusters included; numeric alphabets carry
decimal-string labels). Multi-tape support already existed; v1 (single shared
alphabet) stays readable.

### 6.4 The `.tmx.map` sidecar and binding labels

Structured truth first: the sidecar (JSON, like `.pmx.map`) records every
instantiation/composite **structurally** — routine, per-tape `(phys, map)`
entries, exit vectors symbolically. Tools read the structure; the
human-readable label is derived by a canonical grammar:

```
label   = name "@[" entry ("," entry)* "]"
entry   = physIdx [ "{" pairs "}" ]          ; list position = virtual tape
pairs   = pair ("," pair)*                   ; decimal, no leading zeros, sorted by src
pair    = src "->" dst | src "=>" dst        ; => marks read-only (one-way) entries
```

- equal-size (completed bijection): identity pairs omitted, empty `{}` omitted;
- holey: ALL mapped pairs listed; an absent src is a hole (no collision with
  identity omission — identity completion does not exist in the unequal case);
- blank→blank never written;
- more than 8 pairs → digest `{#xxxxxxxx}` = **CRC-32 of the canonical
  serialization of the completed map** (the container checksum algorithm;
  stable across builds — a content address, matched like a short git hash,
  never decoded). Display collisions get a deterministic `.2` suffix;
  semantics always come from the structure.

**Resolution rule**: a digest is never shown without a path to its expansion —
`tmt dis` prints a legend (glyph-rendered when a tape is supplied);
DebugSession surfaces the resolved binding; LSP hover expands it.

The same binding grammar is used in `.tma` source (§8), dis output, map labels,
and the debugger — one notation across the toolchain.

### 6.5 Image-inspectability principle

Generalizing the PM-1 sidecar rule, now a stated contract:

> Everything the machine executes is inspectable from the image alone; the
> sidecar adds names and provenance, never semantics.

Without the map: frames mode still shows full index-level mappings (descriptors
are operational data in table ROM) and even digests (computable from
descriptor content); mono mode is concrete self-contained code (a stripped
binary — only names are missing); glyph rendering is orthogonal (it comes from
the supplied tape's glyph tables, not from the map).

## 7. VM

1. **Micro-ops** gain tape-indexed forms: `Write{dev, index}`,
   `MoveLeft{dev}` / `MoveRight{dev}`, `Read{dev, slot}` (latching into TR).
2. **MR unification**: the core register is `MR: u32`; PM-1's `LatchMatch`
   writes 0/1; `jm`/`jnm` test `MR ≠ 0`. Appendix A's "MF is formally MR ≠ 0"
   becomes the literal device; PM-1 behavior is byte-identical (regression-
   gated, §16).
3. **Table engine** in core as a generic mechanism (like the stack):
   `MatchTable{id}` (walk rows against TR per §4; row encoding is the SymbolVec
   the core already knows from `encode_operand`) and `DispatchJump{id}`.
   PM-1 simply never emits these micro-ops.
4. **Bus**: one new request kind, `TableRead{addr}` (+ response). Tape devices
   are untouched — `InfiniteTape` / `AnnularTape` / `StrictTape` reused as-is,
   N instances.
5. **Frames profile**: FR + a core-local frame cache loaded at `call.m`
   (bus reads priced at call time), `JumpFrameExit{k}` micro-op for `retx`.
6. **Driver**: serves `TableRead` from the image's table section;
   `TactProfile` extended with table-read and frame-load prices. All tact
   accounting stays in the driver.
7. **Loading**: `Machine::from_executable` validates arch id, required profile,
   tape count, and per-tape alphabet cardinalities against the supplied `.tmt`.
8. **Traps**: `Trap` extended with the kinds of §3.5 (additive).
9. **DebugSession**: same loop; frames profile adds FR and the resolved active
   binding to observable state; `retx` decreases depth exactly like `ret`, so
   the depth-based step model (stepIn/stepOver/stepOut) is unchanged.
10. **Agnosticism proof**: the crate-private fake arch (`test_arch`, `0x7F`)
    grows to exercise the table engine, frames, and `retx` — core's own tests
    keep proving zero PM-1/TM-1 knowledge on the new surface.

## 8. Assembler and the `.tma` dialect

### 8.1 Core framework extensions (arch-agnostic, per-dialect opt-in)

Three mechanisms enter the shared framework; each dialect enables them
explicitly (`.pma` enables none of these — it stays at 0.3 = 0.2 + `wrl`/`wrr`):

1. **Sections** — `.section code` / `.section tables`. The assembler enforces
   table discipline (§4): exact-subsection sortedness and disjointness,
   subsection order, catch-all last. The VM trusts the layout.
2. **Table directives** — `.row [vec]`, `.targets L1, L2, …`,
   `.frame tapes/rmap/wmap [exits = [L1, …]]`.
3. **Repetition macros** — `.rept v, lo, hi … {v} … .endr`; substitution in
   operands and label suffixes (`Linc{v}`), constant arithmetic (`{v+1}`)
   folded at expansion. The macro lives in the lossless CST **as written**
   (fmt never expands; expansion happens at lower). Rationale: table-driven
   states are inherently one-row-per-value (UTM finding 3) — macros compress
   the source while the table stays what the machine must pay. They exist for
   hand-written `.tma`; the compiler generates rows from language ranges and
   does not use them. In-dialect (not an external preprocessor) because
   fmt/lint/LSP operate on the CST.

### 8.2 The `.tma` dialect (0.1)

Everything from Appendix A + the UTM findings: sections, tables, macros,
`wrmv`, vector notation (`*` wildcard / `-` keep / `.` stay). Signature
directive for generic routines (shape: `.routine plusOne, tapes=1, alpha=(3)`;
exact grammar at implementation). Two call forms:

- **declarative, mode-independent**: `call plusOne [2{1->3,2->4}]` — the
  canonical binding grammar of §6.4 as source syntax; the assembler encodes it
  into an MO call-site record; the linker lowers it per `--call-mech`;
- **raw frames form**: `call.m plusOne, F7` with a hand-written `.frame`
  (+ `retx #k` for multi-exit bodies).

The disassembler renders the table section back into directives — assemble/dis
round-trips.

## 9. Linker

Phases in order, all consuming one composition engine, all arch-agnostic
(driven by MO record kinds — PM-1 objects have no binding records, so the new
phases are no-ops for them):

1. **Resolve + closure** — the reachability BFS from `main` generalized to
   (routine, composite binding) pairs; deterministic traversal → byte-identical
   re-links.
2. **Signature checks** — arity, alphabet cardinalities, mapping legality per
   the ruled rules. The compiler checks earlier, but the linker is
   authoritative: hand-written `.tma` objects arrive without a compiler.
3. **Mode lowering** (§5.1): stamping / descriptor + compose-table emission /
   per-site hybrid classification.
4. **Table dedup** — match tables, target tables, descriptors, trap stubs;
   after stamping so identical copies unify.
5. **Layout + relaxation** — the existing shrink-only fixpoint: `call → call.s`
   plus `call.m → call` on identity composites.

Libraries stay first-wins with silent shadowing; the stdlib links lazily via
reachability. `LinkReport` grows: instantiations/composites per routine, dedup
savings, synthesized trap-row counts — the evidence base for §5.7's trigger.

## 10. The `.tmc` language (0.1)

C-like in braces and statements only; there are no general-purpose expressions
or variables — the transition table *is* the computation.

### 10.1 Worlds

A **state lives only inside the braces of a world-carrier**; there are exactly
three, all brace-delimited: the `machine` block body, a `routine` body, a
`graph` body. **File top level is uniformly worldless in every file** —
alphabets, namespaces, routines, graphs, imports, and (in a program) the
single `machine` block; states never appear there. `namespace` is a pure
naming construct (as in `.pmc`) and can hold alphabets, routines, graphs, and
nested namespaces — never states. `goto` cannot cross a world boundary; every
cross-world edge is a `call` or a `graft` and carries a binding.

```tmc
alphabet bits { '_', '0', '1' }        // file top level: worldless declarations

machine {
  tape num: bits;          // world data: declaration order = vector positions; max 16

  entry state inc { … }    // world behavior: states live INSIDE the block
}
```

A library file is the same file shape minus the `machine` block; a program has
exactly one. Program-level `graft` and `bind` declarations live inside the
`machine` block too — they reference its tapes. This is deliberate symmetry:
`machine`/`routine`/`graph` all carry world data (tape declarations /
signature) and world behavior (states) inside one pair of braces.

### 10.2 Alphabets and glyphs

```
element = glyph | glyph '..' glyph | number '..' number
```

- Elements enumerate left to right; **index = position; blank = index 0
  always**, whatever its glyph (`'_'` is convention, not magic).
- A **glyph** is any non-empty UTF-8 string, unique within its alphabet
  (single grapheme cluster is a recommendation; emoji and ZWJ sequences are
  legal; numeric ranges mint decimal-string labels — the glyph of value 126 is
  the three-character string `126`).
- Ranges are inclusive at **both** ends in both forms; there is no count form
  (`'a'..3` is a syntax error — rejected for off-by-one ambiguity). Char
  ranges require single-scalar endpoints.
- Compact family ≤ 127 symbols; larger alphabets silently select the n-byte
  family (the compiler warns on absurd sizes — one row per value is the price
  of the model).
- `export alphabet` is one of the three exportable entity kinds.

### 10.3 States and rules — the classical triple

```
rule       = pattern "->" action ";"
action     = [debugger] [write [vec]] [move [vec]] transition
transition = goto Name | Name | call Target(binding) then continuation
           | return | stop | halt
continuation = Name | return | stop | halt
```

At most one `write` and one `move` per rule (omitted = keep-all / stay-all):
**one rule = one machine step**, mapping 1:1 onto `wrmv` + a control transfer,
and onto the formal δ. Multi-step rule bodies were considered and rolled back;
straight-line chains are written as chains of unconditional states, which
codegen collapses to straight-line code anyway (UTM finding 4).

Pattern positions: glyph | number | range (`'a'..'c'`, `0..125`) with optional
binding `as v` | `*` wildcard. Bindings:

- attach to any *enumerable* element — including single values
  (`10 as v` ≡ `10..10 as v`; named constants for experimentation);
- `* as v` is **forbidden** — it would silently expand the cheapest row into
  alphabet-sized rows; write the range explicitly so the cost is visible
  (the `.rept` ethos);
- multiple bindings per pattern are legal; expansion is the cartesian product
  of the ranges (both costs written in the pattern; the compiler warns above a
  product threshold). Swap idiom:
  `['a'..'z' as c, '0'..'9' as d] -> write [{d}, {c}] …`;
- binding scope = its own rule; names unique per pattern; unused binding is a
  lint note;
- quoted elements bind glyph-kind (pass-through `{c}` only); bare numbers bind
  numeric-kind (constant arithmetic `{v±k}`, folded at expansion — this is
  table-expansion sugar, not runtime dataflow: in each expanded row the
  substitution is that row's constant. Char arithmetic (`{c+1}`) is
  deliberately absent in 0.1.)

Rule order = table row order = priority (§4). A state with no catch-all can
trap `no-transition` — legitimate semantics (the UTM's free invalid-opcode
fault), with an opt-in totality lint (§14).

Terminators: `stop` → `stp` (exit 0), `halt` → `hlt` (exit 2), `return` only
inside a routine (a dynamic transition through the stack — an instruction, not
a state). **Terminology note**: js `haltState` ≈ tmc `stop`; the js
`abortState` being designed ≈ tmc `halt`; `.pmc`'s `halt` is also the abnormal
one. The spec's language chapter carries this table — js users will otherwise
misread `halt`. User-typed traps are not exposed in 0.1 (`halt` covers
"give up"; a reasoned `trap` surface is a post-0.1 candidate).

`debugger` (one grammar position: first element of an action) compiles to
`brk` at the rule's code head — pause when *this rule* fires, after the match,
before its write/move. Inside a `graph` it is copied into every graft instance
(stated consequence of splicing). State-entry/exit pauses are debugger-tool
features (by address via the map sidecar), not source syntax. `brk` semantics,
the `leftover-debugger` lint, strip options, and the optimizer barrier carry
over from PM-1.

### 10.4 Reuse: routines, graphs, and the closed vocabulary

Cross-world reuse has a closed vocabulary: `routine` + `call`,
`graph` + `graft`, `export alphabet`. Naked states are never shared — the
boundary (signature + binding) is common to both forms; what distinguishes
them is the return mechanism:

| | boundary | return | body | needs source | exits |
|---|---|---|---|---|---|
| `routine` + `call` | linker-resolved (`--call-mech`) | **dynamic** (stack) | shared | no (`.tmo`) | one (`return`) |
| `graph` + `graft` | compiler-resolved (splice) | **static** (baked continuations) | copied per instance | yes | any number |

```tmc
export routine plusOne(tape num: bits) {
  entry state inc {
    ['1'] -> write ['0'] move [<] goto inc;
    [*]   -> write ['1'] return;
  }
}

export graph findX(tape t: marks, state found, state missing) {
  entry state walk {
    ['x'] -> found;
    ['_'] -> missing;
    [*]   -> move [>] goto walk;
  }
}
```

Signature parameters follow one shape, `kind name [: type]`: tape parameters
carry their alphabet (`tape src: bits` — the type varies and is the boundary
contract); state parameters carry none (their "type" is fixed: a state of the
instantiator's world). Signature order defines the body's vector positions.

**`graft`** is a declaration (not an action): it splices a copy of the graph's
states into the host world with bindings applied and continuations
substituted; `as name` names the instance — a state name usable anywhere
(goto target, another graft's continuation argument); instance internals are
unaddressable. Rules:

- any number of grafts, in any order (hoisting: world-body names resolve
  regardless of declaration order; forward references and **instance-level
  cycles are legal** — they are goto loops);
- the graph *definition* graft-dependency graph must be **acyclic** (self- or
  mutual graft of definitions = infinite expansion; static error — the Tarjan
  precedent from post-machine-js applies);
- each graft is its own copy; identical instances (same fragment + binding +
  continuations) are silently deduplicated; instances differing in exits
  duplicate irreducibly (different reachable behavior) — `tail_merge` and
  linker table-dedup recover identical fragments, and `outline` (§11.4) can
  reclaim the rest by converting to calls;
- continuation arguments accept terminators (`done = return`) — this is how
  facades work;
- `entry graft …;` marks the instance as the world's entry (`as` optional
  there, required otherwise — an unreferenced unnamed instance is dead states);
- graft binding syntax and semantics are IDENTICAL to call's (same algebra,
  same trap synthesis — applied by the compiler at splice time, with
  source-span diagnostics).

**`bind`** — a named bound call target, pure sugar with zero machine footprint.
Like every binding, it speaks its enclosing world's tape names (machine tapes
in a `machine` block, signature params in a routine/graph body), so it lives
inside a world-carrier:

```tmc
machine {
  tape ctl:  bits;
  tape data: wide;

  bind plusOne(num = data with map { '0'->'0', '1'->'1' }) as incData;
  bind plusOne(num = ctl)                                  as incCtl;  // same routine, other tape

  entry state main {
    ['1', *] -> call incData then done;
    …
  }
}
```

Dedup keys on (routine, binding) anyway; `bind` makes sharing syntactically
guaranteed and gives hover one place of truth. `goto` on a bind name is an
error (it is not a state).

**Binding grammar** (one notation with §6.4/§8.2):

```
binding  = "(" arg ("," arg)* ")"
arg      = name "=" tape-target ("with" "map" "{" pairs "}")?   ; tape params
         | name "=" (state | terminator)                        ; continuations
pair     = src "->" dst          ; bidirectional (read + write-back)
         | src "=>" dst          ; read-only: collapse allowed, no write-back (§5)
```

Per-tape maps (each alphabet pair independent, blank→blank per tape); mapping
omitted = identity (glyph sets must match — ruled); projection always explicit
even in the same world. `with` is the qualifier extension point.

**The facade pattern** (the stdlib's anatomy, available to everyone): define
the behavior ONCE as a graph; a one-line routine facade
(`entry graft g(…, done = return);`) provides the shared-body callable form.
Consumers then choose per use site: `call` the facade (zero duplication,
stack) or `graft` the graph (zero runtime, copies). Visibility is an API
lever: exporting only the facade hides the graph (call-only API — meaningful
for `.tmo`-distributed libraries, where `export graph` is useless anyway since
graft needs source).

What js `withOverriddenHaltState` did is covered twice: graft matches it by
continuation (static), call…then matches it by body (shared, with the stack
doing what wrapper chains did — no memoization/collapse workarounds needed).

### 10.5 entry rules

`entry` is a modifier on `state` and `graft` declarations only; **exactly one
per world** (zero or ≥2 = compile error); the program's entry lives inside the
`machine` block (the `.pmc` "un-namespaced main" norm transposed — an entry
can never sit in a namespace because states can't). Entry restricts nothing:
goto onto an entry is legal (PM-1's `ent` precedent), rules are arbitrary,
`debugger` allowed.

Entry does not cross the graft boundary: a graph's `entry` marks the entry of
**its own world** — at graft time it determines which spliced state the
instance name refers to, and nothing more. The host world's entry-multiplicity
check counts only the host's own `entry state` / `entry graft` declarations;
the entries inside grafted definitions never participate.

### 10.6 Family constructs and keywords

Namespaces, visibility, imports, doc lines mirror `.pmc` §Visibility verbatim:
hidden-by-default + `export`; `namespace` blocks open and merging; `use path
[as alias]`; qualified `@ns::path::name`-style references adapted to `.tmc`
call targets; `?` doc lines and `!` attention lines; `[deprecated]`.

**24 fully-reserved keywords** (deliberate deviation from `.pmc`'s contextual
set — one simple rule beats a context table; called out as a family
difference): `alphabet machine tape state entry routine graph namespace export
use graft bind as map with write move goto call then return stop halt
debugger`. `deprecated` stays contextual (attribute position only). Pattern
and vector punctuation (`* - < > . ..`) and doc markers (`? !`) are not
keywords.

### 10.7 Static checks

Duplicate names per scope; exact-row disjointness; arity/alphabet checks at
every `call`/`graft`; mapping legality (completion, bijectivity — static
errors on the equal-size side); graph-definition acyclicity; entry
multiplicity; `return` outside a routine; `goto` to a missing state or across
a world; bind misuse as a state; limits (16 tapes, 127 compact, binding
product threshold, absurd alphabet sizes).

### 10.8 Deliberately absent in 0.1 (each with its trigger recorded)

Imperative sugar over states; multi-step rule bodies; char-binding arithmetic;
value parameters; suite/shared-world blocks (rejected: graph composition
covers the need); user-typed traps; callable graphs / multi-exit calls at the
language level (ISA-ready via `retx`; language door open); single-tape pattern
sugar (bare `'1' ->` without brackets — the brackets carry the tuple
semantics, keep arity visible, survive adding a tape, and give fmt one canon;
sugar would be additive later, removing it would be breaking); `entry state
name;` redirects (one way to mark an entry).

## 11. Compiler pipeline, TM IR, optimizer

### 11.1 Pipeline

```
.tmc → lexer (0.1 grammar; ?/! doc tokens)
     → parser (recursive descent; parse = lower_cst ∘ parse_cst over ONE
       lossless CST shared with fmt/LSP — the parity-round architecture)
     → compile(source, CompileOptions) → CompileOutput:
         world/duplicate checks → flatten (mangling, visibility, Analysis.docs)
         → graft expansion (front-end phase: acyclicity, binding application,
           trap-rule synthesis for holey graft maps, instance dedup)
         → range expansion (one row per value; product bindings)
     → ir::lower (TM IR)
     → optimizer (in-place)
     → codegen (IR → .tma text ONLY)
     → core asm::assemble → .tmo
```

Objects are **mode-independent**; `--call-mech` is a link option. Canonical
flat lowering — conditional state → `rd; mtc; djmp` + tables; unconditional
chains → straight-line `wrmv`/`jmp`; `call … then S` → `call; jmp S` — is the
definition of `-O0`.

### 11.2 TM IR

`TM_IR_VERSION = 1`, a documented versioned JSON artifact (`tmt ir` renders
it): per-world **state graphs** — states with index-resolved match rows,
actions (write/move vectors, transition kind), and call-site binding records.
The form follows the model (not a general CFG).

### 11.3 Optimizer (`-O1`, fixpoint with a round cap)

Ported passes with their contracts: `inline` (small routines → splice;
program-level, first), `jump_threading`, `tail_call` (`call … then return` →
`jmp`), `tail_merge`, `dce`. Order contract carries: `tail_call` before
`tail_merge`; `brk` is a barrier; `-O0` bit-identity. New TM passes:
`dead_rows` (shadowing-aware, per §4 order semantics), `dispatch_select`
(states with ≤ 2 rows lower through `jm`/`jnm` instead of a table — the
"tables primary + special cases" decision). PM-1 additionally gains
`fuse_tape_ops` (§3.6). Deep multi-tape dataflow (analogs of `cell_state` /
`check_fold`) is deliberately out of v1 — prior art (the MF-coupling lattice)
and a trigger (measured missed optimizations on the live stdlib) are recorded.

### 11.4 `outline` (default OFF)

The inverse of `inline`: convert graft groups back to shared bodies.

- **mono / base profile**: call/ret mechanics — applicable to single-exit
  fragments, refined to **exit-free subgraphs with a single junction** back to
  per-instance code (a multi-junction interior would need an exit selector,
  and no channel exists: no general registers, MR is clobbered by any `mtc`,
  and a tape-written selector would change observables).
- **frames / hybrid**: full multi-exit sharing via exit-vector frames + `retx`
  (§5.5) — one body per graft group regardless of exit count.

Contract-legal at `-O1` (step counts and resource-limit outcomes may change —
the PM equivalence wording carries), but **default off**: graft-vs-call is the
language's documented performance lever, and silently rewriting it would make
the docs lie. Enabled by `--foutline`; thresholds disjoint from `inline`'s;
a natural `-Os` profile member if one ever exists. The per-mode capability
asymmetry is an explicit spec point.

`brk` interaction: `outline` **refuses** to fold a graft group whose body
contains an un-stripped `brk` — merging N debug addresses into one changes the
debugging surface, which is exactly what the observability barrier forbids.
(`debugger` lives in the graph source and is copied into every instance, so a
group is always all-or-nothing.) With debugger strips on, the barrier is gone
and outlining proceeds.

### 11.5 Reports

`CompileReport` warnings: unused import/routine/graph/binding/graft-instance,
undeclared external, shadowed rule; `-Werror` escalates; hygiene beyond
warnings belongs to lint (§14). Library code never prints (thin-renderer rule).

## 12. Stdlib

Mechanics are the pmt precedent verbatim: embedded source
(`include_str!("std.tmc")`), compiled once per process via `OnceLock` at `-O1`
with debugger strips, linked lazily via reachability, `--nostdlib` opts out.

Content: the port of the js binary-number pair. The twins differ by **number
representation** — that is what "bare" means (maintainer correction during
spec review; the js READMEs confirm):

- **`std::binaryNumbers`** — the 5-symbol **delimited** representation
  (alphabet `{ '_', '^', '$', '0', '1' }`): numbers written `^…$`, several
  per tape, inter-number navigation (`goToNumber`, `goToNextNumber`,
  `goToPreviousNumber`, `goToNumbersStart`, …) plus arithmetic on delimited
  numbers. The markers cost extra states per algorithm but buy navigation.
- **`std::binaryNumbersBare`** — the 3-symbol **bare** representation
  (`{ '_', '0', '1' }`): one number per region, boundaries detected by
  blanks; `plusOne`, `minusOne`, `invertNumber`, `normalizeNumber`. Smaller
  graphs, single-number contract.

Each namespace **exports its representation alphabet** (`export alphabet`) so
callers can declare compatible tapes or write mappings against it. Exact
routine inventories are fixed at implementation from the js libraries'
`states.md`; the js doc-parity rule transposes: the two namespaces document
the same operations side by side so the representation trade-off stays
visible.

Internal anatomy — orthogonal to the twin split: within **each** namespace,
behavior is defined once as graphs (explicit continuations) with one-line
routine facades (`entry graft …(…, done = return);`) — the facade pattern of
§10.4. Both forms are exported: the stdlib is source-distributed, so
`export graph` is meaningful.

Cross-representation composition: parts of the delimited namespace MAY be
implemented over the bare one through one-way maps (§5) — e.g. delimited
`invertNumber` = navigation + `bare::invert` called with
`'^' => '_', '$' => '_'` (the markers read as the callee's blank and survive,
since invert never writes blanks). Operations that write over boundaries
(`plusOne` extends leftward onto the `^` cell) cannot compose this way and
stay independent — the writes-through-collapse lint (§14) polices the line.
Which delimited operations actually share bare bodies is an implementation
choice; the js pair shares nothing (independent side-by-side implementations),
so sharing is an option, not a parity requirement.

## 13. CLI (`tmt`) and project config

Eleven subcommands mirroring `pmt`: `compile asm link dis run tape ir lint fmt
lsp completions`. Same load-bearing rules: thin renderer, hand-rolled parser
(no clap), `run` exits 0 = `stp`, 2 = `hlt`, 3 = trap; live `--trace`.
Deltas of substance:

- `link`: `--call-mech=mono|frames|hybrid` (default hybrid), profile
  validation, `entry` (core mechanics absorbed from manifest plan 1).
- `compile`: `--foutline` joins the `--f<pass>`/`--fno-<pass>` family;
  `--emit-ir[=STAGE]` over TM IR.
- `dis`: sectioned round-trip output; binding legend with digest expansion;
  `--tape file.tmt` adds glyph rendering.
- **`tape` becomes an authoring surface** (core helper, exposed by BOTH CLIs):

  ```
  tmt tape show  start.tmt
  tmt tape new   --from prog.tmx -o blank.tmt
  tmt tape set   blank.tmt -o start.tmt \
                 --tape data --cells "72,101,108" --origin 0 --head 2
  ```

  `new --from` builds a blank template from the image (arity + cardinalities
  from the header; glyphs and tape names from the map sidecar when present,
  numeric labels otherwise; heads at 0). `set` has **clone semantics** — input
  untouched, `-o` required, `--in-place` opt-in; per-tape addressing by name
  or index; cell contents as comma-separated glyphs (contiguous-string sugar
  for single-grapheme alphabets) + `--origin` + `--head`; glyphs validated
  against the tape's alphabet; several `--tape` groups per call. Sparse-write
  sugar is an implementation detail; dense run + origin + head is the spec
  surface. `pmt` gains `tape new`/`tape set` symmetrically (second PM-side
  companion item). Goldens remain derivation-first — tool output never
  becomes a golden.
- `completions`: zsh, registry-driven with both drift guards (pass-name
  cross-check including `outline`; parser probes for every entry).

**`tmt.json`** — own file, own schema space, same shape as `pmt.json`
(nearest-ancestor discovery, `lint.allow`, union semantics with IDE settings).
It follows the approved manifest spec (#16) — named-targets map, profiles,
per-target `run` blocks, `tmt build` — without reopening it; TM per-target
extras: `callMech`, processor profile. `tmt build` lands in the manifest
execution round (§17).

## 14. Lint and fmt

**Lint** (shared allow namespace, both languages by extension, stdin `--lang`):

- `.tma`: the five arch-agnostic core rules apply as-is (driven by
  `Flow`/`break_opcode`); TM additions: unused-label, shadowed wildcard rows,
  `retx` index out of the frame's exit-vector bounds, macro hygiene (unused
  `{v}`).
- `.tmc` starter set: leftover-debugger; unused import/routine/graph/alphabet/
  binding/graft-instance; dead-rule (order-aware shadowing); deprecated-call
  (via `Analysis.docs`); redundant identity pairs in maps; binding-product
  threshold; **writes-through-collapse** — a call/graft whose map has one-way
  entries targeting a routine/graph that provably writes the collapsed callee
  symbol (its write vectors are static — the marker-destruction hazard of §5's
  one-way pairs). One opt-in rule: **`state-may-trap`** — flags states without
  a catch-all; off by default (trapping is legitimate semantics — the UTM's
  invalid-opcode is built on it), for users who want provable totality.

**Fmt**: `.tma` — the core canonical grid extended over sections/directives;
macros printed as written. `.tmc` — pmc-fmt philosophy (canonical, idempotent,
whitespace-only, trivia preserved) with two substantive choices: a
**state-block grid** (align `->`, `write`/`move`, terminators within a state —
a transition table should read as a table) and one-binding-arg-per-line beyond
a width threshold.

## 15. Editor plugins

A **separate TM plugin pair** (maintainer decision): new VS Code extension and
JetBrains/LSP4IJ plugin for `.tmc`/`.tma`, launching `tmt lsp`; single-source
TextMate grammars for both languages, drift-guarded against the `.tmc` parser
and `tm1_syntax()`; sideload-only with manual-checklist READMEs; artifacts
attached to GH releases; `MIN_TESTED_TMT` floor. Their node/gradle toolchains
live only under `editors/` (sibling directories to the pmt pair; exact names
at implementation).

**LSP** (`tmt lsp`): two `LanguageService`s in one process over core's
multi-service loop. `.tmc`: live compile diagnostics; completions including
**alphabet glyphs in pattern/write/map positions** (the tape's type is known
from the signature or machine block); go-to-definition (states, graft
instances → graph definitions, routines, alphabets, use paths); hover
(signatures with tape params/alphabets, resolved `bind` bindings, doc/
deprecation callouts — pmc 0.3 parity); semantic tokens; quickfixes (state
stub from an unresolved goto, missing map pair); formatting. `.tma`: pma
parity (no hover, operand hints in completion detail) plus table-label
go-to-definition from `mtc`/`djmp`/`call.m` and `.frame` field diagnostics.
Cross-file behavior is single-file for now; the manifest arc's LSP overlay
design is inherited when it lands (dependency noted, not redesigned).

## 16. Testing strategy

- **Core**: `test_arch` (0x7F) exercises the table engine, frames, `retx`,
  and tape-indexed micro-ops — agnosticism stays proved by core's own tests.
  Property tests (proptest) for every new codec: n-byte symbol family, MX v2
  sections, MO record kinds, MT v2, frame descriptors — round-trips and
  never-panics-on-noise. **PM-1 regression gate: byte-identical behavior**
  after the MR unification.
- **Linker**: unit tests for the composition algebra (associativity, hole
  composition, closure finiteness on adversarial graphs); goldens for
  stamping, descriptors, compose tables; dedup; both relaxations; determinism
  (re-link ⇒ identical bytes).
- **Equivalence — two orthogonal axes** on the `opt_equivalence` harness:
  `-O0` ↔ `-O1` and `mono` ↔ `frames` ↔ `hybrid` — a 2×3 matrix over the same
  programs and tapes, comparing final tapes, termination kind, and trap kind.
- **Goldens are derivation-first**, no exceptions (expected snapshots derived
  in test code; `tape set`/`run` output never committed as a golden).
  **Flagship: `docs/examples/brainfuck-utm.tma` is de-speculated** — it must
  assemble and run (syntax adjustments discovered on the way are folded back
  into the example). The six canonical language examples (§Appendix A) are
  golden programs; the holey-mapping trap paths of stdlib calls are mandatory
  cases.
- Fmt idempotency + lossless CST round-trip; completions drift guards;
  plugins via manual checklists (precedent).

## 17. Delivery phasing and risks

Bottom-up; each phase gets its own implementation plan (writing-plans) with
review. DAG: 1 → 3 → 4 → 5 → 6 → 7 → 8; phase 2 is independent and may ship
any time as a pmt minor release.

| # | Phase | Milestone |
|---|---|---|
| 1 | Core groundwork: tape-indexed micro-ops, MR unification, TR, table engine, `TableRead`, trap kinds, tact prices | PM-1 regression green |
| 2 | PM companions: `wrl`/`wrr` + `fuse_tape_ops` + `.pma` 0.3; tape authoring (`pmt tape new/set`) | independent pmt release possible |
| 3 | Formats: MX v2, MO records, MT v2 | codecs + property tests |
| 4 | TM-1 arch module + `.tma` assembler (core framework: sections/tables/`.frame`/`.rept`) + minimal CLI (asm/dis/run/tape) | **the hand-written UTM assembles and runs** |
| 5 | Linker: composition engine, stamping, frames (+ compose table + exit vectors + `retx`), hybrid, dedup, relaxations, map sidecar; frames profile in the VM; `LinkOptions.entry` (absorbed from manifest plan 1) | three-mode equivalence green |
| 6 | Language: lexer/parser/CST, front end (worlds, graft, ranges, bind), TM IR, optimizer, codegen, `tmt compile`/`ir`, stdlib port | examples 1–6 as goldens; opt-equivalence green |
| 7 | Tooling: lint/fmt ×2, LSP services, `tmt lsp`, completions, `tmt.json`; the TM plugin pair | sideload checklists pass |
| 8 | Docs (per-arch pages under `docs/`), CHANGELOG version block, GH release with plugin artifacts | arc release |

After the arc: the manifest execution round (#16 + #11) implements `pmt build`
**and** `tmt build` per the approved manifest spec.

**Main risk**, stated plainly: both call mechanisms plus `retx` in v1 make
phase 5 the widest single phase in the repo's history. Mitigations: the
equivalence harnesses land inside phase 5 (not after), `LinkReport` counters
provide early evidence, and every phase is separately planned and reviewed.

---

## Appendix A — canonical examples

Ordered simple → composed (mirroring the machines-demo showcase convention).
These six are the spec's teaching set and become golden programs (§16).

### A.1 replace-b

```tmc
? Walk right; replace every 'b' with 'a'; stop at the first blank.

alphabet ab { '_', 'a', 'b' }

machine {
  tape main: ab;

  entry state scan {
    ['b'] -> write ['a'] move [>] goto scan;
    ['a'] ->            move [>] goto scan;
    ['_'] -> stop;
  }
}
```

### A.2 binary +1

```tmc
alphabet bits { '_', '0', '1' }

machine {
  tape num: bits;                    // head on the least significant digit

  entry state inc {
    ['1'] -> write ['0'] move [<] goto inc;   // carry
    ['0'] -> write ['1'] stop;
    ['_'] -> write ['1'] stop;
  }
}
```

### A.3 two-tape copy

```tmc
alphabet bits { '_', '0', '1' }

machine {
  tape src: bits;
  tape dst: bits;

  entry state copy {
    ['0'..'1' as c, *] -> write [-, {c}] move [>, >] goto copy;
    ['_', *]           -> stop;
  }
}
```

### A.4 byte increment (range binding + arithmetic)

```tmc
alphabet bytes { 0..126 }            // 127 symbols; blank = index 0, glyph "0"

machine {
  tape cell: bytes;

  entry state inc {
    [1..125 as v] -> write [{v+1}] stop;
    [126]         -> halt;             // overflow
    [0]           -> write [1] stop;   // blank cell = value 0
  }
}
```

### A.5 routine + call across alphabets (holey mapping)

```tmc
alphabet bits { '_', '0', '1' }
alphabet wide { '_', 'a', 'b', '0', '1' }

namespace mylib {
  export routine plusOne(tape num: bits) {
    entry state inc {
      ['1'] -> write ['0'] move [<] goto inc;
      [*]   -> write ['1'] return;
    }
  }
}

use mylib::plusOne;

machine {
  tape ctl:  bits;
  tape data: wide;

  entry state main {
    ['1', *] -> call plusOne(num = data with map { '0'->'0', '1'->'1' }) then done;
    [*, *]   -> move [>, .] goto main;
  }

  state done { [*, *] -> stop; }
}
```

(`'a'`/`'b'` have no image: reading one inside the call traps `unmapped-read` —
the mandatory trap-path golden.)

### A.6 graph + graft (multi-exit) with an entry instance

```tmc
alphabet marks { '_', 'x', 'y', 'z' }

export graph findX(tape t: marks, state found, state missing) {
  entry state walk {
    ['x'] -> found;
    ['_'] -> missing;
    [*]   -> move [>] goto walk;
  }
}

machine {
  tape work: marks;

  entry graft findX(t = work, found = celebrate, missing = giveUp) as seek;

  state celebrate { [*] -> write ['_'] stop; }
  state giveUp    { [*] -> halt; }
}
```

## Appendix B — rejected alternatives (with revisit triggers)

| Alternative | Why rejected | Trigger to revisit |
|---|---|---|
| mono-only call mechanism | maintainer chose the hybrid+flag model: modes as processor profiles, mechanisms cross-checking each other | — (superseded by the chosen design) |
| frames-only call mechanism | pays runtime indirection on the common total-bijection case; heaviest VM addition carrying all traffic | — |
| runtime (eager or chained) frame composition | dissolved by static link-time composition — depth-dependent cost and frame RAM were the objections | — |
| per-access hardware remap (mapped-access family) | monomorphization + cached frames cover v1; ISA space reserved | measured instantiation bloat `--foutline` cannot reclaim, or a hardware stand's ROM limit |
| imperative language (pmc + more tapes) | tuple dispatch is THE multi-tape operation; explicit states chosen | — |
| multi-step rule bodies | broke the classical δ shape; rolled back at maintainer's call | post-0.1 sugar if chains prove painful |
| suite / shared-world blocks | graph composition covers the need with explicit signatures | multi-fragment stdlib graphs proving unwieldy |
| count-form ranges (`'a'..3`) | off-by-one ambiguity vs endpoint form | — |
| `* as v` bindings | silently expands the cheapest row to alphabet size; explicit ranges keep cost visible | — |
| single-tape pattern sugar (no brackets) | brackets carry tuple semantics; one fmt canon; adding a tape widens instead of rewriting | corpus evidence of bracket fatigue (additive change) |
| `entry state name;` redirect | second way to say one thing | — |
| specificity-ordered wildcard rows | does not totally order; steals priority control | — |
| `std::binaryNumbersGraphs` rename | js "bare" vocabulary kept deliberately | — |
| user-typed traps in the language | `halt` covers "give up"; vocabulary growth without demand | real programs needing distinguishable failure kinds |
| callable graphs (`call g(…) exits {…}`) | language scope control; ISA is ready (`retx`) | post-0.1, on demand |
