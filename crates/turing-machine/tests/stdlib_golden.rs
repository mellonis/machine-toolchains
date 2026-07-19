//! Derivation-first goldens for the embedded standard library
//! (`std::binaryNumbers` / `std::binaryNumbersBare`).
//!
//! Discipline, mirroring `tmc_golden.rs` and the PM-1 `golden_programs.rs`:
//! every expected final tape is DERIVED BY HAND here from the routine's
//! contract (the derivation spelled out in the comment above each case),
//! never captured from a run. The JS libraries these routines port supply
//! the cross-check for the trimmed number content; the exact snapshot
//! (origin, cells, head) is derived from the head-position contracts. There
//! are no committed `.tmt` sidecars — the derivation lives in the comments,
//! which is the durable record, and dozens of trivial one-tape blocks would
//! only add noise.
//!
//! Each golden is a tiny consumer machine that transparently calls one
//! stdlib routine and stops; the routine's behavior comes from the embedded
//! stdlib object. `assert_stdlib_golden` runs the FULL equivalence matrix —
//! the stdlib compiled at `-O0` and at `-O1` (the latter via the `OnceLock`
//! `stdlib::object()`, the optimizer's first live workload) × the three
//! `--call-mech` lowerings — and asserts every combination reproduces the
//! one hand-derived observable. So these goldens double as the stdlib's
//! 2×3 opt/mode-equivalence coverage; the T7 milestone re-runs the same
//! matrix over the wider program set.
//!
//! Alphabets (index = position):
//!   delimited `std::binaryNumbers::symbols`     `_`=0 `^`=1 `$`=2 `0`=3 `1`=4
//!   bare      `std::binaryNumbersBare::symbols` `_`=0 `0`=1 `1`=2

use mtc_core::formats::executable::Executable;
use mtc_core::formats::tapeblock::TapeSnapshot;
use mtc_core::linker::{CallMech, LinkOptions};
use mtc_core::vm::{ArchRegistry, Machine, Outcome, RunLimits, RunOptions, Tape, WideTape};
use mtc_turing_machine::arch::Tm1;
use mtc_turing_machine::asm::link;
use mtc_turing_machine::compiler::{CompileOptions, compile};
use mtc_turing_machine::optimizer::OptLevel;
use mtc_turing_machine::stdlib;

fn snap(origin: i64, cells: &[u8], head: i64) -> TapeSnapshot {
    TapeSnapshot {
        origin,
        cells: cells.to_vec(),
        head,
        alphabet: None,
    }
}

/// A consumer machine over `alphabet` (a full `alphabet a { … }` line) that
/// transparently calls `qualified` (a fully-qualified stdlib routine) on its
/// one tape and stops. The transparent (bindingless) call is the compiled
/// stdlib's consumption path — the routine runs on the caller's tape with
/// the head where the seed left it.
fn consumer(alphabet: &str, qualified: &str) -> String {
    format!(
        "{alphabet}\n\
         machine {{\n\
           tape num: a;\n\
           entry state s {{ [*] -> call {qualified}() then done; }}\n\
           state done {{ [*] -> stop; }}\n\
         }}\n"
    )
}

const DELIM: &str = "alphabet a { '_', '^', '$', '0', '1' }";
const BARE: &str = "alphabet a { '_', '0', '1' }";

/// The stdlib object at `level`: `-O1` reuses the `OnceLock` `stdlib::object()`
/// (the release-preset build); `-O0` compiles `SOURCE` fresh (both `brk`-strip,
/// though the stdlib carries no `brk`).
fn stdlib_object(level: OptLevel) -> mtc_core::formats::object::ObjectFile {
    match level {
        OptLevel::O1 => stdlib::object().clone(),
        OptLevel::O0 => {
            compile(
                stdlib::SOURCE,
                CompileOptions {
                    opt_level: OptLevel::O0,
                    strip_debugger: true,
                    ..Default::default()
                },
            )
            .expect("the stdlib compiles at -O0")
            .object
        }
    }
}

/// Compile the consumer at `level`, link it against the stdlib (also at
/// `level`) under `mech`.
fn build(src: &str, level: OptLevel, mech: CallMech) -> Executable {
    let consumer = compile(
        src,
        CompileOptions {
            opt_level: level,
            ..Default::default()
        },
    )
    .expect("the consumer compiles")
    .object;
    link(
        &[consumer],
        &[stdlib_object(level)],
        LinkOptions {
            call_mech: mech,
            ..Default::default()
        },
    )
    .expect("the consumer links against the stdlib")
    .executable
}

/// Run `exe` on one tape band, returning outcome and the final snapshot.
fn run_one(exe: &Executable, seed: &TapeSnapshot, width: u32) -> (Outcome, TapeSnapshot) {
    let mut tape = WideTape::from_snapshot(seed, width).expect("seed fits width");
    let mut registry = ArchRegistry::new();
    registry.register(Box::new(Tm1::new(exe.tape_count)));
    let machine = Machine::from_executable(exe, &registry).expect("loads");
    let mut devices: Vec<&mut dyn Tape> = vec![&mut tape as &mut dyn Tape];
    let result = machine
        .run_tapes(
            &mut devices,
            RunOptions {
                limits: RunLimits {
                    max_steps: Some(1_000_000),
                    ..Default::default()
                },
                ..Default::default()
            },
        )
        .expect("run set-up ok");
    drop(devices);
    (result.outcome, tape.to_snapshot())
}

/// Assert `src` on `seed` produces `expected` (stopping), across the full
/// 2×3 matrix — stdlib at {O0, O1} × link mech {Mono, Frames, Hybrid}. Every
/// combination must reproduce the single hand-derived observable.
fn assert_stdlib_golden(src: &str, width: u32, seed: TapeSnapshot, expected: TapeSnapshot) {
    for level in [OptLevel::O0, OptLevel::O1] {
        for mech in [CallMech::Mono, CallMech::Frames, CallMech::Hybrid] {
            let exe = build(src, level, mech);
            let (outcome, got) = run_one(&exe, &seed, width);
            assert_eq!(outcome, Outcome::Stopped, "{level:?}/{mech:?}: {src}");
            assert_eq!(got, expected, "{level:?}/{mech:?}: {src}");
        }
    }
}

// ── std::binaryNumbers (delimited) — the ten routines ───────────────────────

#[test]
fn delim_go_to_number() {
    // goToNumber walks right to the current number's '$'. Seed '^11$'
    // [1,4,4,2] head 0 (on '^'):
    //   ^(0)→> 1(1)→> 1(2)→> $(3) done. Head 3, tape unchanged.
    let src = consumer(DELIM, "std::binaryNumbers::goToNumber");
    assert_stdlib_golden(
        &src,
        5,
        snap(0, &[1, 4, 4, 2], 0),
        snap(0, &[1, 4, 4, 2], 3),
    );
}

#[test]
fn delim_go_to_numbers_start() {
    // goToNumbersStart walks left to '^'. Seed '^11$' [1,4,4,2] head 3
    // (on '$', walking back): $(3)→< 1(2)→< 1(1)→< ^(0) done. Head 0.
    let src = consumer(DELIM, "std::binaryNumbers::goToNumbersStart");
    assert_stdlib_golden(
        &src,
        5,
        snap(0, &[1, 4, 4, 2], 3),
        snap(0, &[1, 4, 4, 2], 0),
    );
}

#[test]
fn delim_go_to_next_number() {
    // Multi-number tape '^1$_^10$' = [1,4,2,0,1,4,3,2], head 2 (first '$').
    // goToNextNumber steps right to the blank gap (3) then walks to the next
    // '$': _(3)→ ^(4)→ 1(5)→ 0(6)→ $(7) done. Head 7, tape unchanged.
    let src = consumer(DELIM, "std::binaryNumbers::goToNextNumber");
    assert_stdlib_golden(
        &src,
        5,
        snap(0, &[1, 4, 2, 0, 1, 4, 3, 2], 2),
        snap(0, &[1, 4, 2, 0, 1, 4, 3, 2], 7),
    );
}

#[test]
fn delim_go_to_previous_number() {
    // Same tape, head 7 (second '$'). goToPreviousNumber steps left then
    // walks left to the previous '$': step→0(6), 0(6)→< 1(5)→< ^(4)→< _(3)→<
    // $(2) done. Head 2.
    let src = consumer(DELIM, "std::binaryNumbers::goToPreviousNumber");
    assert_stdlib_golden(
        &src,
        5,
        snap(0, &[1, 4, 2, 0, 1, 4, 3, 2], 7),
        snap(0, &[1, 4, 2, 0, 1, 4, 3, 2], 2),
    );
}

#[test]
fn delim_go_to_numbers_start_multi_number() {
    // The navigation trio's third leg on the multi-number tape: from the
    // second number's '$' (head 7), goToNumbersStart stops at THIS number's
    // '^' (4), not the previous one: $(7)→< 0(6)→< 1(5)→< ^(4) done. Head 4.
    let src = consumer(DELIM, "std::binaryNumbers::goToNumbersStart");
    assert_stdlib_golden(
        &src,
        5,
        snap(0, &[1, 4, 2, 0, 1, 4, 3, 2], 7),
        snap(0, &[1, 4, 2, 0, 1, 4, 3, 2], 4),
    );
}

#[test]
fn delim_delete_number() {
    // deleteNumber erases '^'…'$'. Seed '^1$' [1,4,2] head 0: go to '^'
    // (already there), erase ^→_ >, 1→_ >, $→_ done. All blank; head on the
    // erased '$' cell (2). Snapshot of an all-blank tape = one blank at head.
    let src = consumer(DELIM, "std::binaryNumbers::deleteNumber");
    assert_stdlib_golden(&src, 5, snap(0, &[1, 4, 2], 0), snap(2, &[0], 2));
}

#[test]
fn delim_normalize_number() {
    // normalizeNumber strips leading zeros. Seed '^0101$' [1,3,4,3,4,2]
    // head 0: erase '^' and the leading '0' (cells 0,1 → blank), back up onto
    // the freed cell 1, plant '^', walk right to '$'. Result '^101$' at
    // origin 1: [1,4,3,4,2], head on '$' (5).
    let src = consumer(DELIM, "std::binaryNumbers::normalizeNumber");
    assert_stdlib_golden(
        &src,
        5,
        snap(0, &[1, 3, 4, 3, 4, 2], 0),
        snap(1, &[1, 4, 3, 4, 2], 5),
    );
}

#[test]
fn delim_normalize_number_all_zero_preserves_zero() {
    // Edge: an all-zero number normalizes to '^$' (value zero keeps its
    // representation). Seed '^00$' [1,3,3,2] head 0: erase '^',0,0 (cells
    // 0,1,2), back up onto cell 2, plant '^', walk right to '$'. Result
    // '^$' at origin 2: [1,2], head 3.
    let src = consumer(DELIM, "std::binaryNumbers::normalizeNumber");
    assert_stdlib_golden(&src, 5, snap(0, &[1, 3, 3, 2], 0), snap(2, &[1, 2], 3));
}

#[test]
fn delim_invert_number_cross_representation() {
    // invertNumber is implemented ACROSS representations (over
    // binaryNumbersBare::invertNumber with '^'/'$' collapsed one-way onto the
    // callee blank). Seed '^101$' [1,4,3,4,2] head 0: walk to '^', step onto
    // the first digit, bare-invert 1→0 0→1 1→0 sweeping right, stop when '$'
    // reads as blank. Result '^010$' [1,3,4,3,2], head on '$' (4).
    let src = consumer(DELIM, "std::binaryNumbers::invertNumber");
    assert_stdlib_golden(
        &src,
        5,
        snap(0, &[1, 4, 3, 4, 2], 0),
        snap(0, &[1, 3, 4, 3, 2], 4),
    );
}

#[test]
fn delim_invert_number_preserves_markers() {
    // The marker-preservation contract of the cross-representation call: the
    // '^' and '$' collapse to the callee's blank one-way and SURVIVE, because
    // bare invert never writes a blank. Seed '^10$' [1,4,3,2] head 0 →
    // '^01$' [1,3,4,2], head 3. The '^' (index 1) at 0 and the '$' (index 2)
    // at 3 are untouched; only the digits flipped.
    let src = consumer(DELIM, "std::binaryNumbers::invertNumber");
    let got = snap(0, &[1, 3, 4, 2], 3);
    assert_stdlib_golden(&src, 5, snap(0, &[1, 4, 3, 2], 0), got.clone());
    assert_eq!(got.cells[0], 1, "'^' marker survived the collapse");
    assert_eq!(got.cells[3], 2, "'$' marker survived the collapse");
}

#[test]
fn delim_plus_one() {
    // plusOne on '^1$' [1,4,2] head 0 (=1) → '^10$' (=2). Walk to '$', step
    // to the LSB '1', carry: 1→< ^ overflow: ^→'1' <, plant a fresh '^' one
    // cell left, fill the old leading 1→0, stop at '$'. Result at origin -1:
    // [1,4,3,2], head 2.
    let src = consumer(DELIM, "std::binaryNumbers::plusOne");
    assert_stdlib_golden(&src, 5, snap(0, &[1, 4, 2], 0), snap(-1, &[1, 4, 3, 2], 2));
}

#[test]
fn delim_plus_one_overflow_relocates_start_marker() {
    // Edge: overflow grows the number leftward, relocating '^'. '^111$'
    // [1,4,4,4,2] head 0 (=7) → '^1000$' (=8). The '^' is planted at the new
    // origin -1 and the old leading '1's fill to '0'; the '$' never moves, so
    // it stays at its original position 4 where the fill sweep stops. Result
    // at origin -1: [1,4,3,3,3,2], head 4 (on the '$').
    let src = consumer(DELIM, "std::binaryNumbers::plusOne");
    assert_stdlib_golden(
        &src,
        5,
        snap(0, &[1, 4, 4, 4, 2], 0),
        snap(-1, &[1, 4, 3, 3, 3, 2], 4),
    );
}

#[test]
fn delim_minus_one_fast() {
    // minusOneFast on '^10$' [1,4,3,2] head 0 (=2) → '^1$' (=1). Direct
    // borrow from the LSB then auto-normalize: 0→1 borrow, 1→0 stop → '^01$',
    // normalize strips the leading 0 → '^1$' at origin 1: [1,4,2], head 3.
    let src = consumer(DELIM, "std::binaryNumbers::minusOneFast");
    assert_stdlib_golden(&src, 5, snap(0, &[1, 4, 3, 2], 0), snap(1, &[1, 4, 2], 3));
}

#[test]
fn delim_minus_one_fast_zero_stays_zero() {
    // Edge: '^$' − 1 stays '^$' (minusOneFast normalizes the underflow back
    // to the zero representation). Seed [1,2] head 0: seek to '$', step to
    // '^' (underflow), normalize keeps '^$' at origin 0: [1,2], head 1.
    let src = consumer(DELIM, "std::binaryNumbers::minusOneFast");
    assert_stdlib_golden(&src, 5, snap(0, &[1, 2], 0), snap(0, &[1, 2], 1));
}

#[test]
fn delim_minus_one_composition() {
    // minusOne is the ~(~x + 1) composition (invert, plusOne, invert,
    // normalize). On '^10$' [1,4,3,2] head 0 (=2): ~x '^01$', +1 '^10$', ~
    // '^01$', normalize '^1$' (=1) at origin 1: [1,4,2], head 3 — the same
    // result minusOneFast reaches by a different route.
    let src = consumer(DELIM, "std::binaryNumbers::minusOne");
    assert_stdlib_golden(&src, 5, snap(0, &[1, 4, 3, 2], 0), snap(1, &[1, 4, 2], 3));
}

// ── std::binaryNumbersBare (bare) — the four routines ────────────────────────

#[test]
fn bare_plus_one() {
    // plusOne on '1' [2] head 0 (=1) → '10' (=2). Walk to the trailing blank,
    // step to the LSB '1', carry: 1→0 < , blank→'1' done (overflow extends
    // left). Result at origin -1: [2,1], head -1.
    let src = consumer(BARE, "std::binaryNumbersBare::plusOne");
    assert_stdlib_golden(&src, 3, snap(0, &[2], 0), snap(-1, &[2, 1], -1));
}

#[test]
fn bare_plus_one_overflow_extends_left() {
    // Edge: '111' [2,2,2] head 0 (=7) → '1000' (=8). The carry runs off the
    // MSB, writing a new leading '1'. Result at origin -1: [2,1,1,1], head -1.
    let src = consumer(BARE, "std::binaryNumbersBare::plusOne");
    assert_stdlib_golden(&src, 3, snap(0, &[2, 2, 2], 0), snap(-1, &[2, 1, 1, 1], -1));
}

#[test]
fn bare_minus_one_keeps_leading_zero() {
    // minusOne on '10' [2,1] head 0 (=2) → '01' (=1). Walk to the trailing
    // blank, step to the LSB '0', borrow: 0→1 < , 1→0 done. Result '01' at
    // origin 0: [1,2], head 0 — the leading zero is NOT normalized away.
    let src = consumer(BARE, "std::binaryNumbersBare::minusOne");
    assert_stdlib_golden(&src, 3, snap(0, &[2, 1], 0), snap(0, &[1, 2], 0));
}

#[test]
fn bare_minus_one_underflow_halts_unchanged() {
    // Edge: an empty region (underflow) halts unchanged and does NOT
    // normalize. Seed the blank tape (no cells) head 0: seek steps left into
    // the blank, borrow sees blank → done. Tape stays all blank; head -1.
    let src = consumer(BARE, "std::binaryNumbersBare::minusOne");
    assert_stdlib_golden(&src, 3, snap(0, &[], 0), snap(-1, &[0], -1));
}

#[test]
fn bare_invert_number() {
    // invertNumber on '01' [1,2] head 0 → '10'. Sweep right flipping each
    // bit; halt at the trailing blank. Result [2,1] with the head one past on
    // the trailing blank: origin 0, [2,1,0], head 2.
    let src = consumer(BARE, "std::binaryNumbersBare::invertNumber");
    assert_stdlib_golden(&src, 3, snap(0, &[1, 2], 0), snap(0, &[2, 1, 0], 2));
}

#[test]
fn bare_normalize_number() {
    // normalizeNumber on '01' [1,2] head 0 → '1'. Erase the leading '0'
    // moving right, stop on the first '1'. Result at origin 1: [2], head 1.
    let src = consumer(BARE, "std::binaryNumbersBare::normalizeNumber");
    assert_stdlib_golden(&src, 3, snap(0, &[1, 2], 0), snap(1, &[2], 1));
}

#[test]
fn bare_normalize_number_all_zero_preserves_zero() {
    // Edge: an all-zero number keeps a single '0'. '00' [1,1] head 0: erase
    // both zeros moving right, then blank → write '0' done. Result at
    // origin 2: [1], head 2.
    let src = consumer(BARE, "std::binaryNumbersBare::normalizeNumber");
    assert_stdlib_golden(&src, 3, snap(0, &[1, 1], 0), snap(2, &[1], 2));
}
