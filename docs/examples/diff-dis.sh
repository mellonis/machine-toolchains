#!/usr/bin/env bash
#
# Round-trip: compile -> dis -> asm must reproduce the program.
#
# The object is disassembled back to .tma text, reassembled and relinked.
# Three things are checked: the reassembled object behaves identically on
# every input (whole final MT snapshot, outcome, exit code); disassembling the
# REASSEMBLED object reproduces the same text, i.e. the text is a fixpoint;
# and the two objects are compared byte for byte.
#
# Usage: ./diff-dis.sh [--slow] [EXAMPLE ...]     Env: KEEP=1 keeps the .tma
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
  work=$(mktemp -d)
  "$TMT" compile "$EX_SOURCE" -o "$work/a.tmo" 2>/dev/null || { echo "  compile failed"; rm -rf "$work"; continue; }
  "$TMT" dis "$work/a.tmo" > "$work/a.tma" 2>"$work/e" || { echo "  dis failed"; sed 's/^/    /' "$work/e" | head -3; rm -rf "$work"; continue; }
  "$TMT" asm "$work/a.tma" -o "$work/b.tmo" 2>"$work/e" || { echo "  asm failed"; sed 's/^/    /' "$work/e" | head -3; rm -rf "$work"; continue; }
  "$TMT" dis "$work/b.tmo" > "$work/b.tma" 2>/dev/null
  printf '  .tma %s lines; ' "$(wc -l < "$work/a.tma" | tr -d ' ')"
  cmp -s "$work/a.tma" "$work/b.tma" && printf 'text is a fixpoint; ' || printf 'TEXT NOT A FIXPOINT; '
  cmp -s "$work/a.tmo" "$work/b.tmo" && printf 'object byte-identical\n' \
    || printf 'object differs (%s vs %s bytes)\n' "$(wc -c < "$work/a.tmo")" "$(wc -c < "$work/b.tmo")"
  [[ ${KEEP:-0} == 1 ]] && cp "$work/a.tma" "$name/$name.dis.tma"
  "$TMT" link "$work/a.tmo" -o "$work/a.tmx" >/dev/null || { rm -rf "$work"; continue; }
  "$TMT" link "$work/b.tmo" -o "$work/b.tmx" >/dev/null || { rm -rf "$work"; continue; }

  pass=0 fail=0
  while IFS=$'\t' read -r label tape; do
    A=$("$TMT" run "$work/a.tmx" --tape-block "$tape" --save-tape-block "$work/eA.tmt" 2>&1); ra=$?
    B=$("$TMT" run "$work/b.tmx" --tape-block "$tape" --save-tape-block "$work/eB.tmt" 2>&1); rb=$?
    oa=$(printf '%s\n' "$A" | sed -n 's/^outcome: //p'); ob=$(printf '%s\n' "$B" | sed -n 's/^outcome: //p')
    why=
    [[ $ra == "$rb" ]] || why="exit $ra vs $rb"
    [[ $oa == "$ob" ]] || why="${why:+$why; }outcome $oa vs $ob"
    cmp -s "$work/eA.tmt" "$work/eB.tmt" || why="${why:+$why; }final band differs"
    if [[ -z $why ]]; then (( pass++ )); else (( fail++ )); printf '  %-44s DIVERGED: %s\n' "${label:0:44}" "$why"; fi
  done < <(each_input "$name" "$work")
  echo "  -> $pass round-tripped, $fail diverged"
  tp=$(( tp + pass )); tf=$(( tf + fail )); rm -rf "$work"
done
echo "==="; echo "$tp round-tripped, $tf diverged"; (( tf == 0 ))
