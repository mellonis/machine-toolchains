# rpnwide — 16-bit values spread over four tapes, one digit position each, so
# a stack entry is one cell on each and the head index is the depth. The value
# is therefore not a band at all: it is what the four heads read, most
# significant first, which the run report already prints.
EX_SOURCE=rpnwide/rpnwide.tmc

mk_input() {
  # The empty expression is a real case — it must reach the sentinel and be
  # reported as an empty stack — so the cell list has to survive an empty
  # label rather than emitting a leading comma.
  local c; c=$(cells_hex "$1"); c=${c:+$c,}
  "$TMT" tape-block new --from "$EX_SOURCE" -o "$2" \
    --cells "expr=${c}'_','#'" --head expr=0 >/dev/null 2>&1
}

read_result() {
  local t g out=
  for t in 1 2 3 4; do
    g=$(reads_of "$1" "$t")
    [[ -z $g || $g == '_' ]] && { printf '<none>'; return; }
    out+=$(hex_glyph "$g")
  done
  printf '%s' "$out"
}
