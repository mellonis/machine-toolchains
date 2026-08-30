# Examples

Worked `.tmc` programs, each with a table of cases and a shared runner. Nothing
here is written to the working tree: every script compiles and links into a
temporary directory and throws it away.

```
./run.sh                    # every example's cases
./run.sh rpnhex pow2        # just these
./run.sh --slow             # include the cases tagged slow
```

## The examples

| | what it shows |
|---|---|
| `brainfuck-utm` | a universal Turing machine interpreting brainfuck, over four tapes — the flagship. Its cases run Hello World, Cristofani's Sierpinski triangle, and a golfed Hello World that produces garbage here because it assumes 8-bit wraparound and these cells are a 0..126 ring. Ships twice: `brainfuck-utm.tmc` in the language, and `brainfuck-utm-handwritten.tma` in TM-1 assembly, proved equivalent by derivation-first goldens in the crate's own tests |
| `pow2` | unary exponentiation, `2^N` — one flat machine, three tapes, no reuse constructs at all |
| `rpn` | an RPN calculator over variable-length binary numbers in the standard library's delimited form. The only example here that links anything: it reaches `std::binaryNumbers` through one-tape adapter routines |
| `rpnhex` | the same calculator over fixed-width 16-bit values, four hex digits packed into consecutive cells of one tape. Digit arithmetic becomes a compile-time fold instead of a counting loop |
| `rpnreg` | the same values again, with the packed stack kept as memory and four tapes added as a register — a load-store machine, where arithmetic happens only in registers |
| `rpnwide` | the four digits spread over four tapes, which are also the stack: an entry is one cell on each, so the head index IS the depth and push is a single move |

The four calculators are the same program under four representations, which is
the point of having all four. On the same sum:

| | `rpn` | `rpnhex` | `rpnreg` | `rpnwide` |
|---|---|---|---|---|
| steps | 788,375 | 279 | 195 | 164 |
| tapes | 6 | 6 | 11 | 14 |

`rpn` counts its right operand down one unit at a time, so its cost scales with
the operands' VALUES; the other three are flat. What the last three trade is
tape count against walking: `rpnwide` never walks and pays fourteen tapes,
which every rule of its machine world carries as a fourteen-cell vector.

## How an example is put together

Four files, the same four every time:

```
<name>/
  <name>.tmc     the program
  example.sh     the adapter
  cases          the case table
```

The six programs have nothing in common on the tape — one takes an expression
string, one an exponent, one brainfuck source — so the adapter is what lets one
runner drive them all. `example.sh` sets `EX_SOURCE` and defines two functions:

- `mk_input LABEL OUTFILE` — write the input tape block; non-zero to skip
- `read_result RUNOUTPUT` — echo the one value to compare

`_lib.sh` holds the contract in full, plus the glyph helpers the adapters share.

A `cases` line is `label;expected[;tags]`. `expected` is either the value
`read_result` should produce or one of the two abnormal endings, `halt` and
`trap` — kept distinct because they mean different things: halting is a program
reporting a fault it detected, trapping is a state entered with no matching
rule, which is a bug in the program rather than in its input. The runner
asserts the outcome alongside the exit code, so one can never pass for the
other. Tags are space-separated; `slow` skips the case unless `--slow` is
given, and anything beginning with `--` is handed to `tmt run`.

Adding an example is those four files plus a name in `EXAMPLES` in `_lib.sh`
and a target in `tmt.json`.

## The differential checks

Where `run.sh` compares one value per run, these build the same program several
ways and compare the runs against each other — the WHOLE final tape snapshot,
byte for byte, so a scratch tape left in a different state or a head parked one
cell off counts as a divergence. They drive the examples through the same
`mk_input` the case runner uses.

```
./diff-opt.sh      # -O0 against -O1
./diff-lower.sh    # --call-mech mono against frames against hybrid
./diff-dis.sh      # compile -> dis -> asm -> relink
```

`diff-opt` probes the optimizer — and only the optimizer. Both builds share one
front end, so a bug in the lexer, parser, expander, IR lowering or codegen
produces the same wrong answer at both levels and is invisible to it. The
crate's own differential oracle and property tests are what cover that layer.

`diff-dis` checks more than behaviour: it also holds the `.tma` text to being a
fixpoint under a second disassembly, and reports whether the reassembled object
is byte-identical. On these six it is, every time.

`diff-lower` carries two caveats that keep a pass from reading as more than it
is. `hybrid` produces an image byte-identical to `mono` on every example here —
none contains a raw hand-authored frame, so hybrid's classifier takes its
no-frames fast path and delegates wholesale, which makes the real comparison
mono against frames. And `pow2` and `brainfuck-utm` have no routines, graphs or
calls at all, so all three mechanisms give them the same image: for those two
the check compares a program against itself. The script says so on the line
where it happens.

Incidentally measured, not asserted: linking `frames` gives an image 44–72% the
size of `mono` AND fewer tacts — up to 57% fewer on a multiply — at identical
step counts, because the two mechanisms swap six `ent` for six `call.m` and
differ in what each surviving instruction costs.
