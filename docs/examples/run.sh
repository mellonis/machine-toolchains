#!/usr/bin/env bash
#
# Run every example's case table.
#
# One runner for programs that have nothing in common on the tape: each
# example directory carries an `example.sh` adapter turning a case label into
# an input tape block and a finished run into one comparable string. See
# _lib.sh for that contract and the cases-table format.
#
# Nothing is written to the working tree — each example is compiled and linked
# into a temporary directory and thrown away.
#
# Usage: ./run.sh [--slow] [EXAMPLE ...]     (default: every example)
# Env:   TMT=path/to/tmt

set -uo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")" || exit 1
source ./_lib.sh
resolve_tmt || exit 1

slow=0
while [[ $# -gt 0 && $1 == -* ]]; do
  case $1 in
    --slow) slow=1; shift ;;
    -h|--help) sed -n '2,/^$/p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) echo "run: unknown flag $1" >&2; exit 1 ;;
  esac
done

NAMES=("$@")
(( ${#NAMES[@]} == 0 )) && NAMES=("${EXAMPLES[@]}")

total_pass=0 total_fail=0 total_skip=0
for name in "${NAMES[@]}"; do
  load_example "$name" || { total_fail=$(( total_fail + 1 )); continue; }
  echo "=== $name ($EX_SOURCE) ==="
  work=$(mktemp -d)
  if ! "$TMT" compile "$EX_SOURCE" -o "$work/a.tmo" 2>"$work/err" \
     || ! "$TMT" link "$work/a.tmo" -o "$work/a.tmx" >/dev/null 2>>"$work/err"; then
    echo "  build failed:"; sed 's/^/    /' "$work/err" | head -5
    total_fail=$(( total_fail + 1 )); rm -rf "$work"; continue
  fi

  pass=0 fail=0 skip=0
  while IFS=';' read -r label want tags; do
    tags=${tags:-}
    if [[ " $tags " == *" slow "* ]] && (( ! slow )); then
      printf '  %-28s %s\n' "${label:-<empty>}" 'SKIPPED (--slow)'
      (( skip++ )); continue
    fi
    flags=(); for tk in $tags; do [[ $tk == --* ]] && flags+=("$tk"); done

    if ! mk_input "$label" "$work/in.tmt"; then
      printf '  %-28s %s\n' "${label:-<empty>}" 'SKIPPED (no input)'
      (( skip++ )); continue
    fi

    out=$("$TMT" run "$work/a.tmx" --tape-block "$work/in.tmt" \
            ${EX_RUN_FLAGS[@]+"${EX_RUN_FLAGS[@]}"} ${flags[@]+"${flags[@]}"} 2>&1)
    rc=$?
    outcome=$(printf '%s\n' "$out" | sed -n 's/^outcome: //p')
    steps=$(printf '%s\n' "$out" | sed -n 's/^steps \([0-9]*\).*/\1/p')
    got=$(read_result "$out")

    # The outcome is asserted alongside the exit code, and the three endings
    # are kept distinct: stopping is success, halting is the program reporting
    # a fault it detected, trapping is a state entered with no matching rule.
    want=$(resolve_expected "$name" "$want")
    case $want in
      halt) [[ $rc == 2 && $outcome == Halted ]]   && verdict=ok || verdict=FAIL ;;
      trap) [[ $rc == 3 && $outcome == Trapped* ]] && verdict=ok || verdict=FAIL ;;
      *)    [[ $rc == 0 && $outcome == Stopped && $got == "$want" ]] \
              && verdict=ok || verdict=FAIL ;;
    esac

    if [[ $verdict == ok ]]; then (( pass++ )); else (( fail++ )); fi
    printf '  %-28s %-18s steps %-9s %-5s %s\n' \
      "${label:0:28}" "${outcome:-<none>}" "${steps:-?}" "$verdict" "${got:0:56}"
    [[ $verdict == ok ]] || printf '  %-28s expected %s\n' '' "$want"
  done < <(read_cases "$name")

  printf '  -> %s passed, %s failed%s\n' "$pass" "$fail" \
    "$( (( skip )) && printf ', %s skipped' "$skip" )"
  total_pass=$(( total_pass + pass )); total_fail=$(( total_fail + fail ))
  total_skip=$(( total_skip + skip ))
  rm -rf "$work"
done

echo "==="
printf '%s passed, %s failed%s\n' "$total_pass" "$total_fail" \
  "$( (( total_skip )) && printf ', %s skipped' "$total_skip" )"
(( total_fail == 0 ))
