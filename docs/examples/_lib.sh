#!/usr/bin/env bash
#
# Shared by every script under examples/. Sourced, never run.
#
# An example is a directory holding three things: the program (or, for
# brainfuck-utm, a pointer to where the repo already keeps it), a `cases` table,
# and an `example.sh` adapter. The adapter is what makes one runner serve
# programs whose tapes have nothing in common — it turns a case label into an
# input tape block, and a finished run into one comparable string.
#
# Adapter contract. `example.sh` sets EX_SOURCE (path to the .tmc, relative to
# this directory) and defines:
#
#   mk_input LABEL OUTFILE   write the input tape block; non-zero to skip
#   read_result RUNOUTPUT    echo the value to compare, or <none>
#
# and may set EX_RUN_FLAGS as an array of extra `tmt run` flags.
#
# Cases table. One case per line, `label;expected[;tags]`; blank lines and
# lines starting with # are ignored. `expected` is either the value
# `read_result` should produce, or one of the two abnormal endings, `halt` and
# `trap`. Those are distinct on purpose: halting is a program reporting a
# fault it detected, trapping is a state entered with no matching rule, which
# is a bug in the program rather than in its input. Tags are space-separated;
# `slow` skips the case unless --slow is passed, and anything beginning with
# `--` is handed to `tmt run`.

EXAMPLES=(rpn rpnhex rpnreg rpnwide pow2 brainfuck-utm)

resolve_tmt() {
  if [[ -z ${TMT:-} ]]; then
    for candidate in ../../target/release/tmt ../../../toolchains/target/release/tmt; do
      [[ -x $candidate ]] && { TMT=$candidate; break; }
    done
  fi
  TMT=${TMT:-tmt}
  if [[ ! -x $TMT ]] && ! command -v "$TMT" >/dev/null 2>&1; then
    echo "no tmt binary; build it or set TMT=..." >&2
    return 1
  fi
  TMT=$(cd "$(dirname "$TMT")" && pwd)/$(basename "$TMT")
}

# Every character is its own glyph. A space becomes the blank, whose spelling
# differs per alphabet — '_' by convention, but brainfuck-utm's `ops` really
# does use a space — so the caller names it.
cells_chars() {
  local s=$1 blank=${2:-_} out= i c
  for (( i = 0; i < ${#s}; i++ )); do
    c=${s:i:1}; [[ $c == ' ' ]] && c=$blank
    out+="'$c',"
  done
  printf '%s' "${out%,}"
}

# A hex digit's glyph label is its VALUE, so 'F' is the glyph named 15.
hex_label() {
  case $1 in
    [aA]) printf 10 ;; [bB]) printf 11 ;; [cC]) printf 12 ;;
    [dD]) printf 13 ;; [eE]) printf 14 ;; [fF]) printf 15 ;;
    ' ')  printf '_' ;;
    *)    printf '%s' "$1" ;;
  esac
}
hex_glyph() {
  case $1 in
    10) printf A ;; 11) printf B ;; 12) printf C ;;
    13) printf D ;; 14) printf E ;; 15) printf F ;;
    *)  printf '%s' "$1" ;;
  esac
}
cells_hex() {
  local s=$1 out= i
  for (( i = 0; i < ${#s}; i++ )); do out+="'$(hex_label "${s:i:1}")',"; done
  printf '%s' "${out%,}"
}

# A short, stable stand-in for a value too long to write into a cases table.
# Used where a program's output runs to hundreds of bytes: the digest is exact,
# so it still pins every byte, and it stays one line.
digest_of() {
  local n=$1 h
  if command -v shasum >/dev/null 2>&1; then h=$(printf '%s' "$2" | shasum -a 256)
  else h=$(printf '%s' "$2" | sha256sum); fi
  printf '%s bytes sha256:%s' "$n" "${h:0:12}"
}

# The band of tape N as the run report prints it, without the outer bars.
band_of() {
  local out=$1 n=$2 b
  b=$(printf '%s\n' "$out" | awk -v t="tape $n:" '$0 ~ "^"t { getline; print; exit }')
  b=${b#|}; printf '%s' "${b%|}"
}
# What the head of tape N reads, straight out of the run report.
reads_of() {
  printf '%s\n' "$1" | sed -n "s/^tape $2:.*reads '\(.*\)'.*/\1/p"
}

load_example() {
  local name=$1
  [[ -f $name/example.sh ]] || { echo "no such example: $name" >&2; return 1; }
  unset -f mk_input read_result 2>/dev/null
  EX_RUN_FLAGS=()
  # shellcheck disable=SC1090
  source "$name/example.sh"
}

read_cases() {
  grep -v '^[[:space:]]*#' "$1/cases" | grep -v '^[[:space:]]*$'
}

# each_input NAME WORKDIR -> one `label<TAB>tapefile` line per runnable case.
# The differentials use this so they and run.sh drive the examples over exactly
# the same inputs, built by exactly the same adapter.
each_input() {
  local name=$1 out=$2 i=0 label want tags
  while IFS=';' read -r label want tags; do
    tags=${tags:-}
    if [[ " $tags " == *" slow "* ]] && (( ! ${SLOW:-0} )); then continue; fi
    i=$(( i + 1 ))
    mk_input "$label" "$out/in.$i.tmt" || continue
    printf '%s\t%s\n' "${label:-<empty>}" "$out/in.$i.tmt"
  done < <(read_cases "$name")
}
