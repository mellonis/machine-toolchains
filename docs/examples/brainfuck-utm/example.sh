# brainfuck-utm — the repo's flagship example, a universal Turing machine
# interpreting brainfuck. Both the .tmc and the hand-written .tma it is proved
# equivalent to live here; the repo's own goldens reference them by path, so
# this directory is named in six test files and three docs pages.
#
# A case label is brainfuck source; the input is that source on the `prog`
# tape followed by the 'H' sentinel the interpreter stops on, with the data,
# output and bracket-counter tapes blank.
#
# The value is the bytes emitted by '.', comma-separated — or, once the output
# passes forty bytes, a count and a digest of exactly that list, because a
# Sierpinski triangle does not belong in a cases table. The digest still pins
# every byte.
#
# Either way this is a WEAKER check than the repo's own goldens, which derive
# all four final tapes from an independent reference interpreter and compare
# tape for tape; this only looks at what the program printed.
EX_SOURCE=brainfuck-utm/brainfuck-utm.tmc

mk_input() {
  "$TMT" tape-block new --from "$EX_SOURCE" -o "$2" \
    --cells "prog=$(cells_chars "$1" ' '),'H'" --head prog=0 >/dev/null 2>&1
}

read_result() {
  local b f out= IFS='|'
  b=$(band_of "$1" 2)
  [[ -z $b ]] && { printf '<none>'; return; }
  local n=0
  for f in $b; do
    [[ $f == 0 ]] && continue          # index 0 is the blank on a `bytes` tape
    out+="${out:+,}$f"; n=$(( n + 1 ))
  done
  [[ -z $out ]] && { printf '<empty>'; return; }
  if (( n > 40 )); then digest_of "$n" "$out"; else printf '%s' "$out"; fi
}
