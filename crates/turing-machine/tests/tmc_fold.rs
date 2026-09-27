//! Fold-expression evaluation in `.tmc` write cells (docs/tmt/language.md
//! (substitution)): the modulo wrap, multi-variable folds, and the four fold
//! diagnostics — out-of-alphabet, zero-modulus, negative-remainder (with the
//! wrapping-decrement idiom hint), and i64 overflow.
//!
//! Result cases are derivation-first: the expected final tape is derived by
//! hand here and the run must reproduce it. Diagnostic cases assert the
//! rendered error CODE (and, for the hint, its message text).

use mtc_core::formats::executable::Executable;
use mtc_core::formats::tapeblock::TapeSnapshot;
use mtc_core::linker::LinkOptions;
use mtc_core::vm::{ArchRegistry, Machine, Outcome, RunLimits, RunOptions, Tape, WideTape};
use mtc_turing_machine::arch::Tm1;
use mtc_turing_machine::asm::link;
use mtc_turing_machine::compiler::{CompileError, CompileOptions, compile};

/// Compile `src`, expecting a compile error (the diagnostic cases).
fn compile_err(src: &str) -> CompileError {
    match compile(src, CompileOptions::default()) {
        Ok(_) => panic!("expected a compile error, got a successful compile"),
        Err(e) => e,
    }
}

fn snap(origin: i64, cells: &[u8], head: i64) -> TapeSnapshot {
    TapeSnapshot {
        origin,
        cells: cells.to_vec(),
        head,
        alphabet: None,
    }
}

/// Compile, link, and run `src` on the given seed `(snapshot, width)` bands
/// (no CLI round-trip, matching `tmc_golden`), returning the outcome and the
/// final per-tape snapshots.
fn compile_link_run(src: &str, seeds: &[(TapeSnapshot, u32)]) -> (Outcome, Vec<TapeSnapshot>) {
    let out =
        compile(src, CompileOptions::default()).unwrap_or_else(|e| panic!("must compile: {e}"));
    assert!(
        out.report.diagnostics.is_empty(),
        "expected no diagnostics, got {:?}",
        out.report.diagnostics
    );
    let exe: Executable = link(
        std::slice::from_ref(&out.object),
        &[],
        LinkOptions::default(),
    )
    .unwrap_or_else(|e| panic!("link failed: {e}"))
    .executable;
    let mut tapes: Vec<WideTape> = seeds
        .iter()
        .map(|(s, w)| WideTape::from_snapshot(s, *w).expect("seed fits its width"))
        .collect();
    let mut registry = ArchRegistry::new();
    registry.register(Box::new(Tm1::new(exe.tape_count)));
    let machine = Machine::from_executable(&exe, &registry).expect("loads");
    let mut devices: Vec<&mut dyn Tape> = tapes.iter_mut().map(|t| t as &mut dyn Tape).collect();
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
    let snaps = tapes.iter().map(WideTape::to_snapshot).collect();
    (result.outcome, snaps)
}

#[test]
fn fold_modulo_wraps_increment() {
    // a6 = {0,1,2,3,4,5} (glyph == index). The rule increments mod 6, so
    // reading 5 WRAPS to (5+1)%6 = 0 (the blank). Seed the single cell with 5,
    // head 0: read 5 → write 0 → stop (no move, head stays at 0). An all-blank
    // WideTape snapshots as one blank cell at the head, so the derived final
    // tape is a lone 0 at origin 0.
    let src = "\
alphabet a6 { 0..5 }
machine {
  tape t: a6;
  entry state inc {
    [0..5 as v] -> write [{(v+1)%6}] stop;
  }
}
";
    let (outcome, snaps) = compile_link_run(src, &[(snap(0, &[5], 0), 6)]);
    assert_eq!(outcome, Outcome::Stopped);
    assert_eq!(snaps, vec![snap(0, &[0], 0)]);
}

#[test]
fn fold_multi_var() {
    // Two bindings in one row: a on tape x (0..2), b on tape y (0..1); the
    // fold writes a+b onto x. Run the (a=1, b=1) row: x=1, y=1 → x becomes
    // a+b = 2 (non-blank), y unchanged. (The four-row derivation is pinned at
    // the expansion level in `expand::range_tests::fold_multi_var_expands_four_rows`.)
    let src = "\
alphabet a3 { 0..2 }
alphabet a2 { 0..1 }
machine {
  tape x: a3;
  tape y: a2;
  entry state sum {
    [0..1 as a, 0..1 as b] -> write [{a+b}, -] stop;
  }
}
";
    let (outcome, snaps) = compile_link_run(src, &[(snap(0, &[1], 0), 3), (snap(0, &[1], 0), 2)]);
    assert_eq!(outcome, Outcome::Stopped);
    assert_eq!(snaps, vec![snap(0, &[2], 0), snap(0, &[1], 0)]);
}

#[test]
fn fold_negative_remainder_errors_with_hint() {
    // {(v-1)%6} goes negative at v=0 (in Rust, -1 % 6 = -1). The modulus is
    // the positive literal 6, so the diagnostic teaches the wrapping-decrement
    // idiom {(v+5)%6} (5 = 6 - 1).
    let src = "\
alphabet a6 { 0..5 }
machine {
  tape t: a6;
  entry state s { [0..5 as v] -> write [{(v-1)%6}] stop; }
}
";
    let e = compile_err(src);
    assert_eq!(e.kind.code(), "negative-remainder");
    assert!(e.to_string().contains("{(v+5)%6}"), "message: {e}");
}

#[test]
fn fold_zero_modulus_errors() {
    let src = "\
alphabet a6 { 0..5 }
machine {
  tape t: a6;
  entry state s { [0..5 as v] -> write [{v%0}] stop; }
}
";
    assert_eq!(compile_err(src).kind.code(), "zero-modulus");
}

#[test]
fn fold_overflow_errors() {
    // The `.tmc` number literal is a u32, so i64::MAX is not writable as a
    // literal (it lexes as "too large"). Force the i64 overflow through
    // arithmetic on u32::MAX-valued literals instead: 4294967295 * 4294967295
    // (~1.8e19) exceeds i64::MAX (~9.2e18), so `checked_mul` reports overflow.
    let src = "\
alphabet a6 { 0..5 }
machine {
  tape t: a6;
  entry state s { [1..5 as v] -> write [{v*4294967295*4294967295}] stop; }
}
";
    assert_eq!(compile_err(src).kind.code(), "fold-overflow");
}

#[test]
fn fold_out_of_alphabet_unchanged() {
    // {v+10} over a 6-symbol alphabet folds above the alphabet — the existing
    // out-of-alphabet diagnostic, unchanged by full fold evaluation.
    let src = "\
alphabet a6 { 0..5 }
machine {
  tape t: a6;
  entry state s { [0..5 as v] -> write [{v+10}] stop; }
}
";
    assert_eq!(compile_err(src).kind.code(), "fold-out-of-alphabet");
}

/// One of the decimal-increment probe programs, read from the repository.
fn probe(name: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../docs/superpowers/probes/2026-09-13-tmc-completeness/probe3-sets")
        .join(name);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// Whether a bound symbol folds is decided by its LABEL naming a number,
/// never by how its literal was quoted (docs/tmt/language.md
/// (substitution)): `'7'` and `7` are one symbol, so a quoted digit range
/// folds exactly as a numeric one does. The glyph-spelled increment
/// compiles and computes what the number-spelled one does — the two
/// alphabets are the same symbols at the same indices ('_' 0, '0'..'9'
/// 1..10). The number-spelled program also strips leading zeros, a no-op
/// on these seeds, so only the cells are compared, not the head.
/// Mutation: deciding foldability by quotes again — the glyph-spelled
/// program is refused (`char-arithmetic`).
#[test]
fn a_glyph_spelled_digit_range_folds_like_a_numeric_one() {
    let glyphs = probe("dec-glyph.tmc");
    let numbers = probe("dec.tmc");
    // (seed digits as indices, derived result): 199 + 1 = 200, 9 + 1 = 10
    // (the carry grows the number one cell left), 41 + 1 = 42.
    for (seed, expected) in [
        (vec![2u8, 10, 10], vec![3u8, 1, 1]),
        (vec![10], vec![2, 1]),
        (vec![5, 2], vec![5, 3]),
    ] {
        let tape = snap(0, &seed, 0);
        let (g_outcome, g_snaps) = compile_link_run(&glyphs, &[(tape.clone(), 13)]);
        let (n_outcome, n_snaps) = compile_link_run(&numbers, &[(tape, 13)]);
        assert_eq!(g_outcome, Outcome::Stopped);
        assert_eq!(n_outcome, Outcome::Stopped);
        // The number, left to right: every non-blank cell (a result has no
        // interior blank).
        let digits =
            |s: &TapeSnapshot| -> Vec<u8> { s.cells.iter().copied().filter(|&c| c != 0).collect() };
        assert_eq!(digits(&g_snaps[0]), expected, "glyph-spelled on {seed:?}");
        assert_eq!(digits(&n_snaps[0]), expected, "number-spelled on {seed:?}");
    }
}

/// The near miss: a label that names no number does not fold, however the
/// range is spelled. Mutation: deciding foldability from the symbol's
/// alphabet POSITION (every glyph has one) — this fold would then compile.
#[test]
fn a_fold_over_non_numeric_labels_is_still_char_arithmetic() {
    let src = "\
alphabet abc { '_', 'a'..'d' }
machine {
  tape t: abc;
  entry state s { ['a'..'c' as d] -> write [{d+1}] stop; [*] -> stop; }
}
";
    assert_eq!(compile_err(src).kind.code(), "char-arithmetic");
}
