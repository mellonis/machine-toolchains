# Design: interior comments in comma-separated lists (both toolchains)

Closes [#57](https://github.com/mellonis/machine-toolchains/issues/57).
Milestone: [Release cut — closes the TM-1 arc](https://github.com/mellonis/machine-toolchains/milestone/3).

## 1. The defect

A comment written *inside* a comma-separated list re-attaches to whatever
follows the enclosing item, because those list nodes hold entries rather
than entries-with-trivia. There is nowhere in the tree to hang the
comment, so the parser's pending-comment cursor drains it at the next
item boundary and the printer emits it there.

```
alphabet bit { '_', // the blank
 '0', '1' }
```

becomes

```
alphabet bit { '_', '0', '1' }
// the blank

machine {
```

This is not data loss and not an idempotence break — every comment
reprints exactly once, and a second pass is a no-op on the moved output.
It is a **semantic misattachment**: `// the blank` now reads as a comment
on `machine`. That is worse than losing it would be, because it is
silently wrong rather than visibly absent.

### 1.1 Surfaces, measured

The issue reports four surfaces. Probing the shipped binaries found more,
including one the issue misses entirely (`.tmc`'s own `use` list). They
split on whether the CST node owns the entry `Vec` or reaches it through
`RuleCst`'s verbatim `parser::Rule` embed:

| Surface | Node / field | Side of the seam |
|---|---|---|
| TM `alphabet` body | `AlphabetCst.elems` | CST-owned |
| TM `routine`/`graph` signature params | `ReuseCst.sig.params` | CST-owned |
| TM `graft` args | `GraftCst.args` | CST-owned |
| TM `bind` args | `BindCst.args` | CST-owned |
| TM `use` paths | `UseCst.paths` | CST-owned |
| PM `use` paths | `UseCst.paths` | CST-owned |
| TM `call` binding args | `Transition::Call.args`, inside `RuleCst.rule` | behind the embed |
| TM `with map { }` pairs | `SymMap.pairs`, inside `RuleCst.rule` | behind the embed |
| TM pattern cells / write vectors / move vectors | `Pattern.cells`, `WriteVec`, `MoveVec`, inside `RuleCst.rule` | behind the embed |

Note the issue's phrase "call/graft binding lists" spans the seam:
graft/bind args are CST-owned, call args are not.

### 1.2 Positions, measured

Three distinct positions misattach, not just "between entries":

| Position | Example | Slot needed |
|---|---|---|
| before entry 0, on its own line | `{` ⏎ `// note` ⏎ `'_', '0' }` | precedes entry 0 |
| between entries | `'_', // the blank` | precedes entry 1 |
| after the last entry, before the closer | `'_', '0' // last` ⏎ `}` | precedes the closer |

The third has no following entry, so a per-entry wrapper alone cannot
hold it. The existing `close_trailing` does not cover it — that field
captures a comment on the *same physical line as the closer*, which is a
different position.

## 2. Scope

**In scope** — the named-argument, alphabet, and import lists:

- TM `alphabet` body elements
- TM `routine`/`graph` signature parameters
- TM `graft` / `bind` argument lists
- TM `call` binding argument lists
- TM `with map { }` pair lists
- TM and PM `use` path lists

**Out of scope**, deferred to a follow-up issue: TM pattern cells, write
vectors, and move vectors. These are positional glyph vectors walked
per-row by codegen and the optimizer; a mid-vector comment is vanishingly
rare, and per-cell trivia would put a wrapper on the compiler's hottest
structures. They keep today's relocation behaviour, and the docs keep
saying so for them alone.

**Also out of scope**: PM's existing statement comma-group behaviour
(§4.2 below). It is shipped, documented, and correct for every input
shape #57 reports.

## 3. Why not wrap the entries

The obvious fix — wrap every entry in a `{ value, leading }` struct,
following PM's existing `CommaItem` — is the wrong trade here, for two
measured reasons.

### 3.1 Arbiter: the AST is contractually comment-independent

`crates/turing-machine/src/compiler.rs:3577` asserts:

```rust
assert_eq!(program, &parse(&batch_tokens).unwrap());
```

The AST parsed from a comment-carrying token stream must equal the AST
parsed from a comment-free one. Any trivia reaching an AST-facing type
has to be stripped in `lower_cst`, or this breaks.

`Transition::Call.args`, `SymMap.pairs`, and `Signature.params` are all
AST-facing: `RuleCst` embeds `parser::Rule` verbatim, and `lower_cst`
hands `r.sig.clone()` straight to the AST `Routine`.

### 3.2 Arbiter: blast radius

Changing those two seam types in place touches:

| | `Transition::Call` sites | `SymMap.pairs` sites |
|---|---|---|
| optimizer (inline 9, outline 3, tail_call 3, +5) | **20** | 1 |
| `ir.rs` | 10 | 3 |
| `compiler.rs` | 5 | — |
| `expand.rs` | 2 | **13** |
| `codegen.rs` | 1 | 2 |
| lint / lsp / fmt / parser | 3 | 3 |
| **total** | **41** | **22** |

A comment-printing fix would drag 20 optimizer sites — code governed by
the `-O0` bit-identity and equivalence contracts — into its diff. That is
not a trade worth making for comment placement.

## 4. Design

### 4.1 Representation: sparse, index-keyed interior comments

Every affected list node gains one field:

```rust
/// Comments written inside the list, in source order, each keyed by the
/// index of the entry it precedes. An index equal to the entry count
/// means "after the last entry, before the closer".
pub interior: Vec<(usize, Comment)>,
```

This covers all three positions of §1.2 with one concept, and works
identically on both sides of the seam. Consequences:

- **No element type changes anywhere.** `AlphabetCst.elems` stays
  `Vec<AlphabetElem>`, `GraftCst.args` stays `Vec<BindingArg>`,
  `UseCst.paths` stays `Vec<UsePath>`. Every existing reader compiles
  untouched.
- **`lower_cst` needs no change.** The new fields live only on CST
  nodes; no AST type is touched, so §3.1's arbiter holds *by
  construction* — there is no strip pass to get wrong.
- **`RuleCst`'s verbatim embed is preserved literally.** The CST module
  doc's "Rule internals are reused, not redefined" stands as written.
- **Zero change to `crates/core`**, the optimizer, IR, codegen, or
  expand.

The cost is a range invariant on the index in place of structural
pairing. It is produced by one helper and consumed by one bucketer;
§4.4 covers the guard.

Field placement:

| Node | Crate | New field(s) | Closer |
|---|---|---|---|
| `AlphabetCst` | TM | `interior` | `}` |
| `ReuseCst` (signature params) | TM | `sig_interior` | `)` |
| `GraftCst` | TM | `interior` | `)` |
| `BindCst` | TM | `interior` | `)` |
| `UseCst` | TM | `interior` | `;` |
| `UseCst` | PM | `interior` | `;` |
| `RuleCst` | TM | `call_args: Vec<(usize, Comment)>`, `map_pairs: Vec<(usize, usize, Comment)>` | `)` / `}` |

`RuleCst` needs two fields because map pairs nest one level inside call
args; the triple is (arg index, pair index, comment).

PM's existing `CommaItem` is **not** touched. It carries
`newline_before` for author-grouping preservation, which these surfaces
do not need — it solves a genuinely different problem.

### 4.2 Placement rule

Both crates' `lexer::Comment` carries `own_line: bool` ("only whitespace
preceded the comment on its physical line"). Using it lets each comment
reprint in the position its author chose:

- **`own_line: false`** — a trailing comment such as `'_', // the blank`
  — rides the end of the *preceding* entry's line, after the separator.
- **`own_line: true`** — prints on its own line, at the entry indent,
  before the entry it precedes.
- Either way a **LINE** comment forces the list onto its multi-line
  path: nothing can follow `//` on its physical line. This is the same
  forcing `open_trailing` already applies to an `alphabet` body.
- A **BLOCK** comment with no LINE comment in its run stays inline and
  forces nothing — `alphabet bits { '_', /* x */ '0', '1' }` stays on
  one line.

This is deliberately **stricter than PM's §D**
(`post-machine/src/fmt/mod.rs::layout_leading`), which resolves both
cases to "ride the preceding line" — so an own-line comment gets pulled
*up* and re-attached to the previous entry. Honouring `own_line` keeps
that normalisation out of the new surfaces. PM's `use` list will
therefore place own-line comments more faithfully than PM's statement
comma-groups do; one surface right beats both consistently wrong, and §D
stays untouched per §2.

### 4.3 Rendered output

The existing multi-line shapes are reused verbatim — the `alphabet` body
loop (entries at indent + `INDENT_UNIT`, closer at indent) and
`paren_list` (entries at col + `INDENT_UNIT`, closer at col):

```
alphabet bits { '_', // the blank        alphabet bits {
  '0', '1' }                      ──▶      '_', // the blank
                                           '0',
                                           '1'
                                         }

routine walk(tape t: bits, // note      routine walk(
             state done) {       ──▶      tape t: bits, // note
                                          state done
                                        ) {

[*] -> call walk(t = m, // note         [*] -> call walk(
                 done = stop)    ──▶             t = m, // note
       then stop;                                done = stop
                                               ) then stop;

use lib::p, // the first                use lib::p, // the first
    lib::q;                      ──▶        lib::q;
```

Two shapes need new code rather than reuse:

- `fmt.rs::sym_map_text` currently returns a flat `String`, so a nested
  `with map { … }` cannot break. It gains a `col` parameter and the
  breaking behaviour of `paren_list`.
- `use` continuation lines indent to align under the first path — 4
  columns past the statement indent, clearing `use `. Both crates.

A rule whose call argument list breaks across lines already occurs today
(the >80-column path), and the state-block grid already handles it, so
comments inside a call ride an existing code path.

### 4.4 Components and data flow

**Parser** — one shared helper per crate, joining the existing
`drain_pending` / `capture_open_trailing` / `capture_close_trailing` /
`take_trailing` family:

```rust
/// Drain every pending comment written before entry `index` of the list
/// being parsed, tagging each with that index.
fn interior_comments(&mut self, index: usize, out: &mut Vec<(usize, Comment)>)
```

Called at the top of each list-loop iteration, then once more before
consuming the closer with `index = entries.len()`. It runs *after* the
existing `capture_open_trailing` call, so an `alphabet`'s brace-line
comment is claimed before the loop starts and the two never contend for
the same comment.

Six call sites: the `alphabet` body loop, `parser.rs::signature`,
`parser.rs::binding_args` (which serves graft, bind, *and* call),
`parser.rs::sym_map`, and each crate's `use` loop.

**`lower_cst`** — unchanged, per §4.1.

**Printer** — one bucketing helper per crate turning
`Vec<(usize, Comment)>` into per-slot buckets plus a `forces_break`
flag:

```rust
struct Interior<'a> { slots: Vec<Vec<&'a Comment>>, forces_break: bool }
fn bucket(interior: &[(usize, Comment)], entry_count: usize) -> Interior<'_>
```

Consumed by four renderers: `render_alphabet`, `paren_list`,
`render_use`, and `sym_map_text`.

### 4.5 Error handling

There is no new failure mode — comments are trivia and cannot fail to
parse. The one new invariant is the index range:

- `debug_assert!(index <= entry_count)` in the bucketer.
- In release, an out-of-range index **clamps to the tail slot** rather
  than being dropped. A misplaced comment is a bug; a lost one is data
  loss, and the whole point of this round is that silently-wrong
  placement is worse than absence.

## 5. Testing

### 5.1 The gate that already encodes the bug

`crates/turing-machine/tests/fmt_tmc.rs` already runs the two hard gates
over every `.tmc` in the repository, and its token signature compares
`own_line` alongside comment text and stream position:

```rust
Sig::Comment { text: …, own_line: *own_line }
```

`formatting_never_changes_a_token` therefore **already encodes this
bug** — it has simply never been exercised, because no corpus fixture
contains an interior list comment. Relocation flips `own_line` from
false to true *and* moves the comment's index within the stream; either
difference fails the assertion.

The TDD entry point is thus to add interior list comments to an existing
golden. `crates/turing-machine/tests/golden/a5_call_across_alphabets.tmc`
already carries a `call` with a `with map { … }`, so a single fixture
edit reaches both seam surfaces plus the alphabet and `use` lists.

### 5.2 Gate matrix

| Gate | Where | What it proves |
|---|---|---|
| whitespace-only | `fmt_tmc.rs::formatting_never_changes_a_token` | fails today, passes after |
| idempotence | `fmt_tmc.rs::every_tmc_source_formats_idempotently` | second pass reproduces the first |
| stdlib byte-identical at `-O0` and `-O1` | `stdlib_golden.rs` | no AST leak |
| goldens still byte-identical | `golden_programs.rs` | the fixture edit is provably text-only |
| PM-1 byte-identity, `crates/core` zero-diff | `cargo test --workspace` | round floor; core is not touched at all |
| PM fmt gates | `post-machine/tests/fmt_programs.rs`, `fmt_property.rs` | PM `use` list, and §D unchanged |

### 5.3 Per-surface matrix

Every in-scope list × 3 positions (before-first / between / after-last)
× LINE vs BLOCK × `own_line` true/false where applicable. Fixture-driven,
one assertion per cell, so a regression names the exact surface and
position.

Plus a unit test on the bucketer's clamp path (§4.5), which no
fixture-driven test can reach.

## 5.4 Re-verification (2026-08-05)

Re-probed against a binary built from current master before planning,
because a sibling round established that this repo's published pages can
be wrong about behaviour and must not be taken as the reference.

Still true, unchanged: all three positions of §1.2 misattach; the
AST-purity assertion holds at `crates/turing-machine/src/compiler.rs:3577`;
`fmt_tmc.rs`'s `Sig::Comment` still compares `own_line`, so
`formatting_never_changes_a_token` genuinely already encodes this bug;
and `tests/golden/a5_call_across_alphabets.tmc` still carries a `call`
with a `with map`, so one fixture edit reaches both seam surfaces.

**Found wrong — the durable pages understate the defect**, which changes
§6 from a rewrite into a correction:

- `docs/tmt/fmt.md` § "The trivia exception" names three list kinds and
  says "those three lists". At least **six** TM surfaces misattach: it
  omits `bind` argument lists, `use` path lists, and `with map` pair
  lists, and does not mention the glyph vectors this design defers.
- `docs/pmt/fmt.md` does not mention the behaviour **at all**. PM's `use`
  path list misattaches exactly as TM's does, and a PM reader currently
  has no warning of it.

Neither page is *false* about what it does say; both are incomplete, and
the TM one's "those three" phrasing actively implies a closed set.

## 6. Documentation

- `docs/tmt/fmt.md` § "The trivia exception" — rewritten from a
  known-limitation note into a statement of the §4.2 placement rule.
  The exception paragraph survives only for the out-of-scope glyph
  vectors.
- `docs/pmt/fmt.md` — gains the `use`-list half; the § "Comma groups"
  description of §D behaviour is unchanged.
- Both pages stay forge-agnostic per the workspace's published-docs
  policy: substance in prose, no issue or PR numbers, no hosting URLs.
- New code comments cite `docs/tmt/fmt.md` / `docs/pmt/fmt.md` by page
  plus parenthetical keyword; no `docs/superpowers/` citation, per the
  repo's documentation-authority rule.

## 7. Follow-up

One new issue for the deferred glyph vectors — TM pattern cells, write
vectors, and move vectors — recording that the mechanism designed here
extends to them unchanged, and that the reason for deferral is per-cell
wrapper cost on structures the optimizer and codegen walk per row, not
any obstacle in the design.
