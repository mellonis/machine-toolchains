# State-parameter names in the object — design

2026-11-27. Approved by the maintainer the same day. A small change, cut
before 0.6.0 because the two contracts it touches — container MO 4 and the
TM-1 `.tma` dialect 0.5 — are both still unreleased (0.5.0 shipped MO 3 and
`.tma` 0.3), so neither version moves.

## Intent

A routine's state parameters (its exits) are part of its interface: a caller
may name them. The object records only how many there are, so a declaration
read back from an object has no names. The language-tightening round worked
around that with a stopgap, and its final review named the root fix: record
the names. This is that fix.

The stopgap's cost today: a caller written against a header rendered from an
object (`exit0`, `exit1`, …) breaks when a text declaration of the same
routine joins the build; and the conflict comparison between readings is
non-transitive, which forced an all-pairs comparison and a
text-before-object lookup rule.

## Goals

- The interface record carries one name per state parameter.
- The `.routine` line spells them, so the text-expressibility gate holds:
  `.routine main, tapes=2, alpha=(3, 6), exits=(done, fail), noreturn`.
- Every reading of a routine — from source, from a header, from an object —
  carries the authored names, and the stopgap machinery is deleted:
  `ParamForm::State { name: Option<_> }` becomes a plain name,
  `PublishedForm::agrees`/`ParamForm::agrees` collapse to equality,
  `Provenance`, `Resolved::from_object` and `declarations::first_reading`
  go, and `tmt interface` on an object prints real names instead of minting
  `exit0…`.

## Non-goals

- No version movement: MO stays 4, `.tma` stays 0.5, `.tmc` stays 0.3.
- No change to a call site's `exits=(<label>, …)` operand or to a frame
  descriptor's `.exits` list — those name the CALLER's labels and already
  work.
- No PM-1 change: `pm1_syntax()` does not opt into the interface capability;
  PM-1 output stays byte-identical.

## Anti-goals

- Optional names. While a name may be absent, every piece of the stopgap has
  to stay. The numeric form `exits=<K>` is removed, not kept beside the
  named one.

## Approaches

1. **Mandatory names, `exits=(a, b)` on the `.routine` line** — chosen. One
   spelling, mirrors the call-site operand, deletes the stopgap whole. Cost:
   a hand-written `.tma` must name its exits, and the tests pinning the
   numeric form are rewritten.
2. **Optional names beside the count.** Rejected: keeps `Option`, the
   non-transitive comparison and the lookup rule — the fix would fix
   nothing.
3. **One directive per exit, on the `.param` precedent.** Rejected: an exit
   carries a name and nothing else, so a directive line per exit is
   ceremony, and it would open the question of ordering between `.param` and
   the new lines.

## Design

**Wire (MO 4, interface section, per routine).** Where the record holds the
exit count byte today it holds the same count byte followed by one
string-pool index (`u32`) per exit, in declaration order. The count stays a
`u8`, so the 255 ceiling is unchanged.

**In memory.** `RoutineInterface::exits: u8` becomes `exits: Vec<String>`;
every reader of the count takes `exits.len()`.

**Assembly.** The `.routine` tail is `[, exits=(<name>, …)][, noreturn]`,
fixed order, each at most once. Names are word tokens, pairwise distinct
(a repeat is a `BadSignature`). `exits=()` is accepted wherever `exits=0`
was — alone, without `.param` lines — and is never printed back; a
non-empty list needs the `.param` lines, as before. The numeric form no
longer parses as the tail, so the line degrades the way any malformed
`.routine` does. The disassembler prints `, exits=(a, b)` for a non-empty
list. `fmt`, the `.tma` language service descriptors and the TextMate
grammar follow the new spelling where they know the old one.

**Compiler.** Codegen emits the authored names. Reading an object for
declarations yields the same `ParamForm::State { name }` a text reading
does, so the table needs no provenance: lookups take the first reading in
table order, and the conflict check is plain equality.

## Left to the builder

- Whether a state-parameter name may equal a tape-parameter name follows
  whatever `.tmc` already rules; core only enforces distinctness among the
  exits.
- The exact wording of the new diagnostics.
- Which of the existing first-reading pins survive as order-independence
  pins and which are deleted with the rule they pinned.

## Verification

- The object round-trip property test generates named exits.
- dis → asm byte-identity over a routine with exits (the
  text-expressibility gate).
- `tmt interface` over an object and over its source print the same exit
  names.
- A caller naming an exit compiles the same against the object reading and
  the text reading, in either `--extern` order.
- PM-1 goldens untouched; the everything-matrix green; one full
  `cargo nextest run --workspace` at the end.
- Before the cut: confirm no version constant moved (`git diff` of every
  `*VERSION*` literal against `origin/next`).
