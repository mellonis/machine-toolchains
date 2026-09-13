# Probe 3 — named glyph sets (issue #121, line 3)

Decimal arithmetic over `alphabet dec { '_', digits, '+', '=' }`, written against
master `2c36ebd` with `target/release/tmt`. Everything here is untracked probe
material; the verdicts live in the #121 report.

## Files

| File | What it is |
|---|---|
| `dec-glyph.tmc` | Variant A — the issue's spelling: glyph literals, `['0'..'8' as d] -> write [{d+1}]`. **Does not compile**: `dec-glyph.tmc:12:34: error: arithmetic on a glyph binding is not supported … [char-arithmetic]`. |
| `dec.tmc` | Variant S — numeric digit literals `0..9`; the increment spelled as nine literal rules so that the exact contract `writes { 0..9 }` survives the checker. Builds. |
| `dec-subst.tmc` | Variant F — numeric literals, the increment as one fold rule `[0..8 as d] -> write [{d+1}]`; **no contract on `plusOne`**, because with it the build fails: `dec.tmc:11:26: error: \`decimal::plusOne\` may write '_', '+', '=' on tape \`num\`, which its contract forbids [writes-outside-contract]` (the documented substitution over-approximation, `docs/tmt/language.md` "Contract clauses"). Builds. |
| `in-*.tmt`, `out-*.tmt`, `run-*.log` | Seven cases, both variants. |

Routines: `decimal::plusOne` (walk right to the end, increment with carry,
grow one cell left on overflow, head rests on the last digit) and
`decimal::stripLeadingZeros` (blank leading zeros, keep a lone zero, head
rests on the first surviving digit). The machine runs both in sequence.

## Runs (both variants identical, 91 steps on `999`)

| case | in (head) | out | rc |
|---|---|---|---|
| n129 | `129` (0) | `130` | 0 |
| n999 | `999` (1) | `1000` (origin −1) | 0 |
| n009 | `009` (2) | `10` | 0 |
| n0 | `0` (0) | `1` | 0 |
| n099 | `099` (0) | `100` | 0 |
| expr | `12+3=09` (5) | `12+3=10` | 0 |
| blank | `_` (0) | `1` | 0 |

`tmt lint --warn state-may-trap` flags `blank` in both variants (`dec.tmc:52:11`,
`dec-subst.tmc:44:11`): a state that is total only under a path invariant
(`check` enters it after reading a `0` to the left). Evidence for line 4.

## Respell counts

| spelling | dec.tmc (S) | dec-subst.tmc (F) | where |
|---|---|---|---|
| `0..9` | 5 | 4 | 1 alphabet body, 1 `writes` clause (S only), 3 pattern cells (`toEnd`, `toLast`, `toStart`) |
| `0..8` | 0 | 1 | the fold's binding range |
| `1..9` | 1 | 1 | `check` |
| single-digit literal rules | 13 | 4 | S spells the increment as `[0] … [8]` |

In 55 lines the digit set is written 4–5 times, and two further spellings are
*subsets* (`0..8`, `1..9`) a bare name would not cover. Corpus: `rpnhex.tmc`
writes `0..15` **22** times (2 alphabet bodies + 20 pattern cells; `std.tmc` has
no ranges at all — binary alphabets).

## Two findings that are not about counting

1. **Foldability is decided by the literal's spelling, not by the symbol.**
   `'0'` and `0` are one symbol (`docs/tmt/language.md` "Alphabets"), but
   `check_char_arithmetic` (`parser.rs:1896–1934`, the decision at `:1909`
   `PatternCellKind::Range { lo, .. } => lo.is_glyph()`) rejects
   `['0'..'8' as d] -> {d+1}`. A decimal program must therefore declare and
   match digits as numerics everywhere. A `set` declaration would carry the
   element kind once.
2. **A fold write voids an exact `writes` contract.** Variant F cannot keep
   `writes { 0..9 }`; variant S keeps it at the price of nine literal rules.
   Not a line-3 issue, but it is the same over-approximation that limits the
   `writes` inference a header will export.

## Grammar sites (Part B), `crates/turing-machine/src/parser.rs`

- Alphabet body: `parse_alphabet` `:1388–1395` → `alphabet_elems` `:1406–1420`
  (the one owner of the `,`/`}` loop) → `alphabet_elem` `:1421–1430` →
  `sym_or_range` `:2011–2027` → `sym_lit` `:2028–2051`. AST `AlphabetElem`
  `:128–134`.
- Contract clauses: `contract_clause` `:1588–1616` — a second copy of the
  element loop calling `alphabet_elem` (`:1600`); body is `Vec<AlphabetElem>`.
- Pattern cell: `pattern_cell` `:1965–2010` — `*` → `Wildcard`; `Glyph|Number`
  → `sym_or_range` → `Single`/`Range` (`:1972–1984`); optional `as NAME`
  (`:1994–1999`); wildcard+binding rejected (`:2002–2004`). AST
  `PatternCellKind::Range` `:339`.
- Write cell: `write_cell` `:2078–2110` — `-` | `sym_lit` | `{ fold_expr }`.
  No range or set can appear: a write is one symbol.

Downstream `Range` consumers (grep `PatternCellKind::Range|AlphabetElem::Range`):
`compiler.rs` (2; `resolve_alphabet_glyphs` `:749`), `expand.rs` (1),
`fmt/print.rs` (2), `lint/patterns.rs` (1; `cell_labels` `:50–56`),
`lint/rules/binding_product_threshold.rs` (1),
`lint/rules/contract_clause_overlap.rs` (2), `lsp/roster.rs` (1),
`parser.rs` (3), `parser/tests.rs` (3) — 9 files, 16 sites.
