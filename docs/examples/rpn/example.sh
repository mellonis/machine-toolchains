# rpn — variable-length binary numbers in the standard library's delimited
# form. A case is an RPN expression; the value is the stack band, so a result
# reads as its own representation, '^' digits '$'.
EX_SOURCE=rpn/rpn.tmc

mk_input() {
  # The empty expression is a real case — it must reach the sentinel and be
  # reported as an empty stack — so the cell list has to survive an empty
  # label rather than emitting a leading comma.
  local c; c=$(cells_chars "$1"); c=${c:+$c,}
  "$TMT" tape-block new --from "$EX_SOURCE" -o "$2" \
    --cells "expr=${c}'_','#'" --head expr=0 >/dev/null 2>&1
}

read_result() {
  local b; b=$(band_of "$1" 1)
  [[ -z $b ]] && { printf '<none>'; return; }
  printf '%s' "$b"
}
