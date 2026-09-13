# Probe 1 — head position as a checkable contract (issue 121, line 1)

Date: 2026-09-13. Toolchain: `tmt 0.5.0` (`target/release/tmt`, master at
`2c36ebd`). Everything here is untracked; nothing under `crates/` or `docs/`
proper was modified.

## Files

| File | Role |
|---|---|
| `headlib.tmc` | the library: two representations (`delSym` = `{'_','^','$','0','1'}`, `bareSym` = `{'_','0','1'}`), five routines whose head pre/postconditions live only in `?` lines |
| `good.tmc` | a caller that meets every precondition — expected `^0100$` from `^1010$` |
| `bad1.tmc` | violation 1: `del::plusOne` entered on the number's last digit instead of its `$` (tape block head = 4) |
| `bad2.tmc` | violation 2: `del::goToStart` entered on the blank after `$` (`move [>]` in the calling row) |
| `inunit.tmc` | the library + the bad2 caller in ONE compilation unit (bound calls), for the IR/footprint view |
| `run.sh` | the transcript driver; `transcript.txt` is its captured output |

Library head contracts (prose today, in `headlib.tmc`):

| routine | entry (head on) | exit (head on) | body on a violated entry |
|---|---|---|---|
| `del::goToEnd` | `'^' '0' '1' '$'` | `'$'` | walks on (also accepts `'_'`) |
| `del::goToStart` | `'^' '0' '1' '$'` | `'^'` | strict: no rule for `'_'` → NoTransition |
| `del::plusOne` | `'$'` | `'$'` | trusting: `[*] -> move [<]` steps left blindly |
| `del::invert` | `'^'` | `'$'` | strict on entry, then a cross-representation call |
| `bare::invert` | `'0' '1'` | `'_'` | `'_'` is accepted (returns at once) |

The cross-unit callers re-declare `delSym` and use transparent calls
(`call del::plusOne()`), the only legal form across a link boundary today
(`docs/tmt/stdlib.md (calling a library routine)`) — a first attempt with
`call del::plusOne(num = num)` and no local alphabet failed with
`unresolved-alphabet`, and naming the alphabets `del`/`bare` collided with
the namespaces (`duplicate-name`).

## Transcript (`./run.sh`)

```

$ ../../../../../target/release/tmt --version
tmt 0.5.0
tmc language 0.1
tma dialect (tm-1) 0.3
[exit 0]

$ ../../../../../target/release/tmt build headlib.tmc good.tmc -o good.tmx
[exit 0]

$ ../../../../../target/release/tmt build headlib.tmc bad1.tmc -o bad1.tmx
[exit 0]

$ ../../../../../target/release/tmt build headlib.tmc bad2.tmc -o bad2.tmx
[exit 0]

$ ../../../../../target/release/tmt tape-block new --from good.tmc -o in-head0.tmt --cells num='^','1','0','1','0','$' --head num=0
[exit 0]

$ ../../../../../target/release/tmt tape-block new --from bad1.tmc -o in-head4.tmt --cells num='^','1','0','1','0','$' --head num=4
[exit 0]

$ ../../../../../target/release/tmt tape-block show in-head0.tmt
tape 0: origin 0, head 0 reads '^', alphabet ["_", "^", "$", "0", "1"]
|^1010$|
[exit 0]

$ ../../../../../target/release/tmt tape-block show in-head4.tmt
tape 0: origin 0, head 4 reads '0', alphabet ["_", "^", "$", "0", "1"]
|^1010$|
[exit 0]

$ ../../../../../target/release/tmt run good.tmx --tape-block in-head0.tmt --save-tape-block good.out.tmt
outcome: Stopped
steps 108, core tacts 501, stall tacts 297 (total 798)
tape 0: origin 0, head 5 reads '$'
|^0100$|
[exit 0]

$ ../../../../../target/release/tmt tape-block show good.out.tmt
tape 0: origin 0, head 5 reads '$', alphabet ["_", "^", "$", "0", "1"]
|^0100$|
[exit 0]

$ ../../../../../target/release/tmt run bad1.tmx --tape-block in-head4.tmt --save-tape-block bad1.out.tmt
outcome: Stopped
steps 92, core tacts 426, stall tacts 260 (total 686)
tape 0: origin 0, head 5 reads '$'
|^0011$|
[exit 0]

$ ../../../../../target/release/tmt tape-block show bad1.out.tmt
tape 0: origin 0, head 5 reads '$', alphabet ["_", "^", "$", "0", "1"]
|^0011$|
[exit 0]

$ ../../../../../target/release/tmt run bad2.tmx --tape-block in-head0.tmt --save-tape-block bad2.out.tmt
outcome: Trapped(NoTransition { at: 120 })
steps 49, core tacts 222, stall tacts 111 (total 333)
tape 0: origin 0, head 6 reads '_'
|^1011$_|
[exit 3]

$ ../../../../../target/release/tmt tape-block show bad2.out.tmt
tape 0: origin 0, head 6 reads '_', alphabet ["_", "^", "$", "0", "1"]
|^1011$_|
[exit 0]

$ ../../../../../target/release/tmt build inunit.tmc -o inunit.tmx
[exit 0]

$ ../../../../../target/release/tmt run inunit.tmx --tape-block in-head0.tmt
outcome: Trapped(NoTransition { at: 142 })
steps 55, core tacts 250, stall tacts 133 (total 383)
tape 0: origin 0, head 6 reads '_'
|^1011$_|
[exit 3]

$ ../../../../../target/release/tmt compile inunit.tmc -o inunit.tmo --emit-ir
[exit 0]

$ ../../../../../target/release/tmt ir footprints inunit.ir.json
world del::goToEnd
  tape 0 (num): writes {} of 5

world del::goToStart
  tape 0 (num): writes {} of 5

world del::plusOne
  tape 0 (num): writes {1, 3, 4} of 5

world del::invert
  tape 0 (num): writes {3, 4} of 5

world bare::invert
  tape 0 (num): writes {1, 2} of 3

world main
  tape 0 (num): writes {1, 3, 4} of 5
[exit 0]

$ ../../../../../target/release/tmt lint inunit.tmc --warn state-may-trap
inunit.tmc:16:17: lint: state `walk` may trap — its rules do not cover every input and there is no catch-all
inunit.tmc:25:11: lint: state `carry` may trap — its rules do not cover every input and there is no catch-all
inunit.tmc:30:11: lint: state `grow` may trap — its rules do not cover every input and there is no catch-all
inunit.tmc:31:11: lint: state `toEnd` may trap — its rules do not cover every input and there is no catch-all
inunit.tmc:38:17: lint: state `step` may trap — its rules do not cover every input and there is no catch-all
inunit.tmc:60:9: lint: state `s2` may trap — its rules do not cover every input and there is no catch-all
inunit.tmc:61:9: lint: state `s3` may trap — its rules do not cover every input and there is no catch-all
[exit 1]

$ ../../../../../target/release/tmt lint headlib.tmc --warn state-may-trap
headlib.tmc:25:17: lint: state `walk` may trap — its rules do not cover every input and there is no catch-all
headlib.tmc:38:11: lint: state `carry` may trap — its rules do not cover every input and there is no catch-all
headlib.tmc:43:11: lint: state `grow` may trap — its rules do not cover every input and there is no catch-all
headlib.tmc:44:11: lint: state `toEnd` may trap — its rules do not cover every input and there is no catch-all
headlib.tmc:55:17: lint: state `step` may trap — its rules do not cover every input and there is no catch-all
[exit 1]
```

## What each run shows

- **good**: `^1010$` → `^0100$`, head on `$`, `Stopped`, exit 0. 108 steps.
- **bad1 — silent wrong result.** `plusOne` entered one cell left of `$`
  (on the last digit `0`): its blind `move [<]` lands on the `1` above it,
  the carry runs from the wrong bit, the number becomes `^1100$` (12, not
  11), and the rest of the pipeline inverts it to `^0011$`. The run
  `Stopped` with exit code 0 and the head on `$` exactly as the exit
  contract promises. Nothing distinguishes it from a correct run.
- **bad2 — a trap, reported as a bare address.** `goToStart` entered on
  the blank after `$`: `Trapped(NoTransition { at: 120 })`, exit code 3,
  head on cell 6 reading `_`. The diagnostic names a code address in the
  callee, not the call site, not the violated convention, and not the
  routine (with `-g` and the `.map` sidecar the debugger would resolve the
  address to `goToStart`'s `walk` — still the symptom, one frame below the
  cause).
- **`move [>] call …` in one row is legal** (bad2's `s3`, and the IR row
  for `main.s3` shows `moves: ['right']` with a `call_then` transition), so
  a call site's head glyph is NOT always the calling row's pattern cell.
- **`state-may-trap`** (opt-in) reports every partial state in both files
  — five in the library, seven in the in-unit file — and cannot tell the
  deliberate `['^'] -> return` guard in `invert.step` from a genuine gap;
  it says nothing about which call site can reach the gap.
- **`tmt ir footprints`** confirms the write inference is per-world and
  per-tape (`del::plusOne` writes `{1, 3, 4}` = `'^','0','1'`;
  `del::invert` writes `{3, 4}` through the map; `main` unions them). There
  is no head fact anywhere in the IR or the footprint table.
