#!/usr/bin/env bash
# Probe 1 transcript driver. Run from this directory; prints every command
# before its output so NOTES.md can quote the transcript verbatim.
set -u
TMT=${TMT:-../../../../../target/release/tmt}
run() { printf '\n$ %s\n' "$*"; "$@"; printf '[exit %s]\n' "$?"; }
cells="num='^','1','0','1','0','\$'"

run $TMT --version
for f in good bad1 bad2; do
  run $TMT build headlib.tmc $f.tmc -o $f.tmx
done
# good / bad2: head on '^' (cell 0). bad1: head on the last digit (cell 4).
run $TMT tape-block new --from good.tmc -o in-head0.tmt --cells "$cells" --head num=0
run $TMT tape-block new --from bad1.tmc -o in-head4.tmt --cells "$cells" --head num=4
run $TMT tape-block show in-head0.tmt
run $TMT tape-block show in-head4.tmt

run $TMT run good.tmx --tape-block in-head0.tmt --save-tape-block good.out.tmt
run $TMT tape-block show good.out.tmt
run $TMT run bad1.tmx --tape-block in-head4.tmt --save-tape-block bad1.out.tmt
run $TMT tape-block show bad1.out.tmt
run $TMT run bad2.tmx --tape-block in-head0.tmt --save-tape-block bad2.out.tmt
run $TMT tape-block show bad2.out.tmt

# The in-unit variant: bound calls, IR and the footprint view.
run $TMT build inunit.tmc -o inunit.tmx
run $TMT run inunit.tmx --tape-block in-head0.tmt
run $TMT compile inunit.tmc -o inunit.tmo --emit-ir
run $TMT ir footprints inunit.ir.json
run $TMT lint inunit.tmc --warn state-may-trap
run $TMT lint headlib.tmc --warn state-may-trap
