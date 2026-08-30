# rpnhex — 16-bit values as four hex digits packed into consecutive cells of one
# stack tape. A case is an RPN expression written in hex; the value is the
# four digits of the single entry left on the stack.
EX_SOURCE=rpnhex/rpnhex.tmc

mk_input() {
  # The empty expression is a real case — it must reach the sentinel and be
  # reported as an empty stack — so the cell list has to survive an empty
  # label rather than emitting a leading comma.
  local c; c=$(cells_hex "$1"); c=${c:+$c,}
  "$TMT" tape-block new --from "$EX_SOURCE" -o "$2" \
    --cells "expr=${c}'_','#'" --head expr=0 >/dev/null 2>&1
}

read_result() {
  local b f out= IFS='|'
  b=$(band_of "$1" 1)
  [[ -z $b ]] && { printf '<none>'; return; }
  for f in $b; do
    [[ $f == '_' ]] && continue
    out+=$(hex_glyph "$f")
  done
  printf '%s' "${out:-<empty>}"
}
