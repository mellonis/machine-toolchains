#!/bin/bash
# probe2-generic: every spelling the language offers for reusing an
# alphabet-typed routine over other alphabets. Output goes to run.log.
set -u
cd "$(dirname "$0")"
T=../../../../../target/release/tmt
mkdir -p gen
exec > run.log 2>&1

decl() { # alphabet declaration for a tape name
  case "$1" in
    ab)   echo "alphabet ab   { '_', 'a', 'b' }";;
    ab2)  echo "alphabet ab2  { '_', 'a', 'b' }";;
    abcd) echo "alphabet abcd { '_', 'a', 'b', 'c', 'd' }";;
    xyab) echo "alphabet xyab { '_', 'x', 'y', 'a', 'b' }";;
    xy3)  echo "alphabet xy3  { '_', 'x', 'y' }";;
  esac
}
cells() {
  case "$1" in
    ab)   echo "'a','b','b','a'";;
    ab2)  echo "'a','b','b','a'";;
    abcd) echo "'a','c','b','d'";;
    xyab) echo "'x','a','y','b'";;
    xy3)  echo "'x','y','y','x'";;
  esac
}

# transparent(NAME, ALPHA, ROUTINE): a separate unit calling lib.tmo by symbol
transparent() {
  local name=$1 alpha=$2 routine=$3
  { echo "use lib::$routine;"; decl $alpha; cat <<E2
machine {
  tape t: $alpha;
  entry state s { [*] -> call $routine() then done; }
  state done { [*] -> stop; }
}
E2
  } > gen/$name.tmc
}
# bound(NAME, ALPHA, LIB, CALL-EXPR): the library source pasted into the unit
bound() {
  local name=$1 alpha=$2 lib=$3 call=$4
  { cat $lib; decl $alpha; cat <<E2
machine {
  tape t: $alpha;
  entry state s { [*] -> call $call then done; }
  state done { [*] -> stop; }
}
E2
  } > gen/$name.tmc
}
runcase() { # NAME ALPHA [MECH...]
  local name=$1 alpha=$2; shift 2
  local mechs=${*:-hybrid}
  echo "=================================================================="
  echo "### $name  (tape $alpha = |$(cells $alpha | tr -d "',")|)"
  $T tape-block new --from gen/$name.tmc --cells "t=$(cells $alpha)" -o gen/$name.tmt || return
  for m in $mechs; do
    echo "--- --call-mech $m"
    if [ -f gen/$name.link ]; then
      $T link gen/$name.tmo gen/lib.tmo -o gen/$name.$m.tmx --call-mech $m -v 2>&1 | grep -v "dropped" | sed "s/^/  link: /"
    else
      $T link gen/$name.tmo -o gen/$name.$m.tmx --call-mech $m -v 2>&1 | grep -v "dropped" | sed "s/^/  link: /"
    fi
    $T run gen/$name.$m.tmx --tape-block gen/$name.tmt --save-tape-block gen/$name.$m.out.tmt
    echo "  exit=$?"
  done
}

echo "### toolchain"; $T --version
echo "### compile lib.tmc (the library object other units link against)"
$T compile lib.tmc -o gen/lib.tmo && echo "ok"

echo
echo "################ (i) transparent argless call, cross-unit, binds by INDEX, no map, no cardinality check"
for a in ab abcd xyab; do
  for r in skipRight swapAB swapABopen; do
    n=i_${r}_$a
    transparent $n $a $r
    touch gen/$n.link
    $T compile gen/$n.tmc -o gen/$n.tmo || continue
    runcase $n $a hybrid
  done
done

echo
echo "################ (ii) bound call, omitted map, EQUAL cardinality (identity completion by index)"
bound ii_swapAB_ab2  ab2 lib.tmc "lib::swapAB(t = t)"
bound ii_swapAB_xy3  xy3 lib.tmc "lib::swapAB(t = t)"
for n in ii_swapAB_ab2 ii_swapAB_xy3; do
  $T compile gen/$n.tmc -o gen/$n.tmo || continue
  echo "--- lint --warn index-identity-map"; $T lint gen/$n.tmc --warn index-identity-map | grep -v "^$" | sed 's/^/  /'
  runcase $n ${n##*_} mono frames
done

echo
echo "################ (ii') bound call, omitted map, UNEQUAL cardinality (closed: everything but blank is a hole)"
bound ii_skipRight_abcd_omitted abcd lib.tmc "lib::skipRight(t = t)"
$T compile gen/ii_skipRight_abcd_omitted.tmc -o gen/ii_skipRight_abcd_omitted.tmo && runcase ii_skipRight_abcd_omitted abcd mono frames

echo
echo "################ (iii) bound call, explicit map naming ONLY the needed symbols, UNEQUAL cardinality -> holes"
M="with map { 'a' -> 'a', 'b' -> 'b' }"
bound iii_skipRight_abcd   abcd lib.tmc "lib::skipRight(t = t $M)"
bound iii_swapAB_abcd      abcd lib.tmc "lib::swapAB(t = t $M)"
bound iii_swapABopen_abcd  abcd lib.tmc "lib::swapABopen(t = t $M)"
bound iii_skipRight_xyab   xyab lib.tmc "lib::skipRight(t = t $M)"
bound iii_swapABopen_xyab  xyab lib.tmc "lib::swapABopen(t = t $M)"
for n in iii_skipRight_abcd iii_swapAB_abcd iii_swapABopen_abcd iii_skipRight_xyab iii_swapABopen_xyab; do
  $T compile gen/$n.tmc -o gen/$n.tmo || continue
  runcase $n ${n##*_} mono frames hybrid
done
echo "--- stamped copy of skipRight under the closed map (mono): the synthesized trap rows"
$T dis gen/iii_skipRight_abcd.mono.tmx | sed -n '/^\.section tables/,/^\.section code/p' | head -30
echo "--- .tmx.map bindings record (frames)"
python3 -c "import json,sys; m=json.load(open('gen/iii_skipRight_abcd.frames.tmx.map')); print(json.dumps(m.get('bindings'), indent=1))" 2>/dev/null || grep -o '"bindings":.*' gen/iii_skipRight_abcd.frames.tmx.map | head -c 600

echo
echo "################ (B.3) one-way collapse of every unlisted symbol onto ONE callee symbol"
echo "## B.3a read-only callee: collapse onto an existing symbol ('a'); skipRight never writes"
C="with map { 'a' -> 'a', 'b' -> 'b', 'c' => 'a', 'd' => 'a' }"
bound b3_skipRight_abcd_collapse abcd lib.tmc "lib::skipRight(t = t $C)"
$T compile gen/b3_skipRight_abcd_collapse.tmc -o gen/b3_skipRight_abcd_collapse.tmo && runcase b3_skipRight_abcd_collapse abcd mono frames
echo "## B.3b writing callee, collapse onto a symbol it DISTINGUISHES: no trap, wrong answer (c,d read as 'a', get swapped to 'b')"
bound b3_swapAB_abcd_collapse abcd lib.tmc "lib::swapAB(t = t $C)"
$T compile gen/b3_swapAB_abcd_collapse.tmc -o gen/b3_swapAB_abcd_collapse.tmo && runcase b3_swapAB_abcd_collapse abcd mono frames
echo "## B.3c writing callee with a SPARE opaque symbol 'o'; keep ('-') on 'o' cells"
CO="with map { 'a' -> 'a', 'b' -> 'b', 'c' => 'o', 'd' => 'o' }"
bound b3_swapABo_abcd abcd lib_o.tmc "libo::swapABo(t = t $CO)"
$T compile gen/b3_swapABo_abcd.tmc -o gen/b3_swapABo_abcd.tmo && runcase b3_swapABo_abcd abcd mono frames hybrid
CX="with map { 'a' -> 'a', 'b' -> 'b', 'x' => 'o', 'y' => 'o' }"
bound b3_swapABo_xyab xyab lib_o.tmc "libo::swapABo(t = t $CX)"
$T compile gen/b3_swapABo_xyab.tmc -o gen/b3_swapABo_xyab.tmo && runcase b3_swapABo_xyab xyab mono frames
echo "## B.3d the same collapse, but the callee WRITES 'o' back: a write hole"
bound b3_stampO_abcd abcd lib_o.tmc "libo::stampO(t = t $CO)"
$T compile gen/b3_stampO_abcd.tmc -o gen/b3_stampO_abcd.tmo && runcase b3_stampO_abcd abcd mono frames
echo "--- stamped stampO (mono): the write of 'o' became a trap stub"
$T dis gen/b3_stampO_abcd.mono.tmx | grep -n "trap\|wrmv\|\.func" | head -20
echo
echo "### done"
