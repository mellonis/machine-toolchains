# pow2 — unary exponentiation. A case label is the exponent N; the input band
# is `s b 1xN k` with the head on the 'b'. Generating it here reproduces, byte
# for byte, the tape blocks this experiment used to carry as committed
# binaries, so nothing on disk has to be kept in step with the program.
#
# The value is a shape-and-count summary rather than the raw band: the band at
# N=24 is sixteen million cells, and `sb<count>k` is what the original harness
# checked anyway.
EX_SOURCE=pow2/pow2.tmc

mk_input() {
  local n=$1 cells="'s','b'" i
  [[ $n =~ ^[0-9]+$ ]] || return 1
  for (( i = 0; i < n; i++ )); do cells+=",'1'"; done
  "$TMT" tape-block new --from "$EX_SOURCE" -o "$2" \
    --cells "main=$cells,'k'" --head main=1 >/dev/null 2>&1
}

read_result() {
  local b ones; b=$(band_of "$1" 0)
  [[ $b == sb*k ]] || { printf '<none>'; return; }
  ones=${b#sb}; ones=${ones%k}
  printf 'sb%sk' "${#ones}"
}
