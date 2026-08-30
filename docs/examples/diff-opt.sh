#!/usr/bin/env bash
#
# Differential: -O0 and -O1 must agree on the WHOLE final tape band.
#
# The case tables only compare one value per run. This compares the entire
# saved MT snapshot byte for byte, so a scratch tape left in a different state,
# a head parked one cell off, or a differing origin all count as a divergence.
# Step counts are expected to differ and are reported, not compared.
#
# This tests the OPTIMIZER, not the compiler: both builds share one front end,
# so a bug in the lexer, parser, expander, IR lowering or codegen produces the
# same wrong answer at both levels and is invisible here.
#
# Usage: ./diff-opt.sh [--slow] [EXAMPLE ...]
set -uo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")" || exit 1
source ./_lib.sh
resolve_tmt || exit 1
SLOW=0; [[ ${1:-} == --slow ]] && { SLOW=1; shift; }
NAMES=("$@"); (( ${#NAMES[@]} == 0 )) && NAMES=("${EXAMPLES[@]}")

tp=0 tf=0
for name in "${NAMES[@]}"; do
  load_example "$name" || continue
  echo "=== $name ==="
  work=$(mktemp -d); ok=1
  for o in 0 1; do
    "$TMT" compile "-O$o" "$EX_SOURCE" -o "$work/o$o.tmo" 2>/dev/null \
      && "$TMT" link "$work/o$o.tmo" -o "$work/o$o.tmx" >/dev/null || { echo "  build -O$o failed"; ok=0; }
  done
  (( ok )) || { rm -rf "$work"; continue; }
  pass=0 fail=0
  while IFS=$'\t' read -r label tape; do
    a=$("$TMT" run "$work/o0.tmx" --tape-block "$tape" --save-tape-block "$work/e0.tmt" 2>&1); ra=$?
    b=$("$TMT" run "$work/o1.tmx" --tape-block "$tape" --save-tape-block "$work/e1.tmt" 2>&1); rb=$?
    oa=$(printf '%s\n' "$a" | sed -n 's/^outcome: //p'); ob=$(printf '%s\n' "$b" | sed -n 's/^outcome: //p')
    sa=$(printf '%s\n' "$a" | sed -n 's/^steps \([0-9]*\).*/\1/p'); sb=$(printf '%s\n' "$b" | sed -n 's/^steps \([0-9]*\).*/\1/p')
    why=
    [[ $ra == "$rb" ]] || why="exit $ra vs $rb"
    [[ $oa == "$ob" ]] || why="${why:+$why; }outcome $oa vs $ob"
    cmp -s "$work/e0.tmt" "$work/e1.tmt" || why="${why:+$why; }final band differs"
    if [[ -z $why ]]; then (( pass++ ))
      printf '  %-44s steps %9s -> %-9s ok\n' "${label:0:44}" "$sa" "$sb"
    else (( fail++ )); printf '  %-44s DIVERGED: %s\n' "${label:0:44}" "$why"; fi
  done < <(each_input "$name" "$work")
  echo "  -> $pass agreed, $fail diverged"
  tp=$(( tp + pass )); tf=$(( tf + fail )); rm -rf "$work"
done
echo "==="; echo "$tp agreed, $tf diverged"; (( tf == 0 ))
