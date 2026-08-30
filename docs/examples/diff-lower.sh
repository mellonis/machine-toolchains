#!/usr/bin/env bash
#
# Differential: the three bound-call lowerings must agree.
#
# `tmt link --call-mech mono | frames | hybrid` picks how a bound call is
# lowered — mono stamps a specialized copy per site, frames goes through the
# runtime composite directory, hybrid chooses per site. Same object, three
# links, every input through each; the whole final MT snapshot, the outcome
# and the exit code must match.
#
# Two measured caveats, so a pass is not read as more than it is. On these
# examples `hybrid` produces an image byte-identical to `mono`: none contains
# a raw hand-authored frame, so hybrid's classifier takes its no-frames fast
# path and delegates wholesale. And pow2 has no routines, graphs or calls at
# all, so all three mechanisms give it the same image — for that example this
# check compares a program against itself and proves nothing.
#
# Usage: ./diff-lower.sh [--slow] [EXAMPLE ...]
set -uo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")" || exit 1
source ./_lib.sh
resolve_tmt || exit 1
SLOW=0; [[ ${1:-} == --slow ]] && { SLOW=1; shift; }
NAMES=("$@"); (( ${#NAMES[@]} == 0 )) && NAMES=("${EXAMPLES[@]}")
MECHS=(mono frames hybrid)

tp=0 tf=0
for name in "${NAMES[@]}"; do
  load_example "$name" || continue
  echo "=== $name ==="
  work=$(mktemp -d)
  "$TMT" compile "$EX_SOURCE" -o "$work/a.tmo" 2>/dev/null || { echo "  compile failed"; rm -rf "$work"; continue; }
  skip=0
  for m in "${MECHS[@]}"; do
    "$TMT" link "$work/a.tmo" -o "$work/$m.tmx" --call-mech "$m" >"$work/$m.log" 2>&1 \
      || { echo "  link --call-mech $m REFUSED:"; sed 's/^/    /' "$work/$m.log"; skip=1; }
  done
  (( skip )) && { rm -rf "$work"; continue; }
  printf '  sizes:'; for m in "${MECHS[@]}"; do printf ' %s=%s' "$m" "$(wc -c < "$work/$m.tmx" | tr -d ' ')"; done
  cmp -s "$work/mono.tmx" "$work/frames.tmx" && printf '  (all identical — no bound calls here)' \
    || { cmp -s "$work/mono.tmx" "$work/hybrid.tmx" && printf '  (hybrid == mono, byte-identical)'; }
  printf '\n'

  pass=0 fail=0
  while IFS=$'\t' read -r label tape; do
    why= rr= ro= tt=
    for m in "${MECHS[@]}"; do
      out=$("$TMT" run "$work/$m.tmx" --tape-block "$tape" --save-tape-block "$work/e.$m.tmt" 2>&1); rc=$?
      oc=$(printf '%s\n' "$out" | sed -n 's/^outcome: //p')
      tt+="${tt:+/}$(printf '%s\n' "$out" | sed -n 's/.*(total \([0-9]*\)).*/\1/p')"
      if [[ -z $rr ]]; then rr=$rc; ro=$oc; else
        [[ $rc == "$rr" ]] || why="${why:+$why; }$m exit $rc vs $rr"
        [[ $oc == "$ro" ]] || why="${why:+$why; }$m outcome $oc vs $ro"
        cmp -s "$work/e.${MECHS[0]}.tmt" "$work/e.$m.tmt" || why="${why:+$why; }$m final band differs"
      fi
    done
    if [[ -z $why ]]; then (( pass++ ))
      printf '  %-44s tacts mono/frames/hybrid %s  ok\n' "${label:0:44}" "$tt"
    else (( fail++ )); printf '  %-44s DIVERGED: %s\n' "${label:0:44}" "$why"; fi
  done < <(each_input "$name" "$work")
  echo "  -> $pass agreed across all three, $fail diverged"
  tp=$(( tp + pass )); tf=$(( tf + fail )); rm -rf "$work"
done
echo "==="; echo "$tp agreed, $tf diverged"; (( tf == 0 ))
