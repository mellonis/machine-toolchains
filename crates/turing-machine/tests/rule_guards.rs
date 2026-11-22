//! Rule guards (#133's `where { … }`): the typing and refusal surface —
//! cone-local contexts, one common sequence per order comparison, and the
//! four refusals, each pinned by kind AND span
//! (docs/tmt/language.md (rule guards), docs/internal/specs/…design.md).

use mtc_core::diagnostics::Span;
use mtc_turing_machine::compiler::{CompileError, CompileErrorKind, CompileOptions, compile};

fn parse_ok(src: &str) {
    compile(src, CompileOptions::default()).expect("the fixture must compile");
}

fn refusal(src: &str) -> CompileError {
    compile(src, CompileOptions::default())
        .expect_err("the fixture must be refused")
}

/// The 1-based `Pos` of a needle's first character — the parser battery's
/// own helper, copied for one column basis across test files.
fn at(src: &str, needle: &str) -> mtc_core::diagnostics::Pos {
    for (i, line) in src.lines().enumerate() {
        if let Some(col) = line.find(needle) {
            return mtc_core::diagnostics::Pos {
                line: i as u32 + 1,
                col: col as u32 + 1,
            };
        }
    }
    panic!("`{needle}` not in fixture");
}

/// The half-open span covering the first occurrence of `needle`.
fn cover(src: &str, needle: &str) -> Span {
    let p = at(src, needle);
    Span::new(p.line, p.col, p.line, p.col + needle.len() as u32)
}

const HEAD: &str = "\
alphabet ab { 'a', 'b', 'c' }
set pair { 'a', 'b' }
set trio { 'a', 'b', 'c' }
set foreign { 'q', 'r' }

machine {
  tape t: ab;
  tape u: ab;
  entry state s {
";

#[test]
fn one_sided_narrowing_names_both_contexts() {
    let src = format!("{HEAD}    ['a'..'c' as x, 'a'..'c' as y] where {{ x in pair && x < y }} -> stop;\n  }}\n}}\n");
    let e = refusal(&src);
    assert_eq!(
        e.kind,
        CompileErrorKind::GuardOrderContext {
            left: "pair".into(),
            right: "ab".into(),
        }
    );
    // The refusal spans the COMPARISON, not the whole guard.
    assert_eq!(e.span, cover(&src, "x < y"));
    // …and its message names both contexts.
    let msg = e.kind.to_string();
    assert!(msg.contains("pair") && msg.contains("ab"), "message: {msg}");
}

#[test]
fn a_second_narrowing_agrees_only_on_identity() {
    // `x` narrowed to two different sets: identity needs no sequence and
    // stays legal…
    parse_ok(&format!(
        "{HEAD}    ['a'..'c' as x, 'a'..'c' as y] where {{ x in pair && x in trio && x == y }} -> stop;\n  }}\n}}\n"
    ));
    // …but a position comparison has no single sequence to live in.
    let src = format!(
        "{HEAD}    ['a'..'c' as x, 'a'..'c' as y] where {{ x in pair && x in trio && x < y }} -> stop;\n  }}\n}}\n"
    );
    let e = refusal(&src);
    assert_eq!(
        e.kind,
        CompileErrorKind::GuardOrderContext {
            left: "pair".into(),
            right: "trio".into(),
        }
    );
}

#[test]
fn an_unknown_guard_name_is_refused() {
    let src = format!(
        "{HEAD}    ['a'..'c' as x, 'a'..'c' as y] where {{ zz == y }} -> stop;\n  }}\n}}\n"
    );
    let e = refusal(&src);
    assert_eq!(
        e.kind,
        CompileErrorKind::GuardUnknownBinding { name: "zz".into() }
    );
    // Span at the name itself.
    assert_eq!(e.span, cover(&src, "zz"));
}

#[test]
fn an_in_off_alphabet_is_refused() {
    let src = format!(
        "{HEAD}    ['a'..'c' as x, 'a'..'c' as y] where {{ x in foreign && x == y }} -> stop;\n  }}\n}}\n"
    );
    let e = refusal(&src);
    assert_eq!(
        e.kind,
        CompileErrorKind::GuardSetOutsideAlphabet {
            set: "foreign".into(),
            alphabet: "ab".into(),
        }
    );
    // The span is the set name AS USED in the guard (HEAD itself declares
    // the set, so the needle anchors past `x in `).
    let p = at(&src, "x in foreign");
    assert_eq!(
        e.span,
        Span::new(p.line, p.col + 5, p.line, p.col + 5 + "foreign".len() as u32)
    );
}

#[test]
fn a_comparison_with_no_binding_is_refused() {
    let src = format!(
        "{HEAD}    ['a'..'c' as x, 'a'..'c' as y] where {{ 'q' == 'r' }} -> stop;\n  }}\n}}\n"
    );
    let e = refusal(&src);
    assert_eq!(e.kind, CompileErrorKind::GuardConstantComparison);
    assert_eq!(e.span, cover(&src, "'q' == 'r'"));
}

#[test]
fn a_comparison_types_by_its_own_cone() {
    // The narrowed leg compares in `pair`'s order; the bare leg (a sibling
    // `||` branch, outside the narrowing's cone) is licensed by the shared
    // alphabet instead. Both legs legal, one span of one guard.
    parse_ok(&format!(
        "{HEAD}    ['a'..'c' as x, 'a'..'c' as y] where {{ (x in pair && y in pair && x < y) || x < y }} -> stop;\n  }}\n}}\n"
    ));
}

// ---------------------------------------------------------------------------
// Evaluation and the row filter: a guard drops rows at expansion, and what
// survives is the hand-writable product (docs/tmt/language.md
// (rule guards)). Derivation-first: the guarded fixture's rows equal a
// twin written out rule by rule in the test code itself.
// ---------------------------------------------------------------------------

use std::collections::HashMap;

use mtc_core::formats::executable::Executable;
use mtc_core::formats::tapeblock::TapeSnapshot;
use mtc_core::linker::LinkOptions;
use mtc_core::vm::{ArchRegistry, Machine, Outcome, RunLimits, RunOptions, Tape, WideTape};
use mtc_turing_machine::arch::Tm1;
use mtc_turing_machine::asm::{disassemble_executable_with_map, link};
use mtc_turing_machine::lint::{LintOptions, lint};

fn object(src: &str) -> mtc_core::formats::object::ObjectFile {
    compile(src, CompileOptions::default())
        .unwrap_or_else(|e| panic!("the fixture must compile: {e}"))
        .object
}

/// The linked image's match rows, order-preserving.
fn rows_of(src: &str) -> Vec<String> {
    let out = link(
        std::slice::from_ref(&object(src)),
        &[],
        LinkOptions::default(),
    )
    .unwrap_or_else(|e| panic!("link failed: {e}"));
    let dis = disassemble_executable_with_map(&out.executable, &out.map);
    dis.lines()
        .filter_map(|l| {
            l.split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
                .split_once(".row ")
                .map(|(_, op)| op.to_string())
        })
        .collect()
}

fn snap(origin: i64, cells: &[u8], head: i64) -> TapeSnapshot {
    TapeSnapshot {
        origin,
        cells: cells.to_vec(),
        head,
        alphabet: None,
    }
}

/// Compile, link and run `src` on one seed band per tape.
fn run_on(src: &str, seeds: &[(TapeSnapshot, u32)]) -> Outcome {
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
    result.outcome
}

/// The issue's headline over a 27-label alphabet (`_` then `a`..`z`):
/// `x == y` keeps the 26-cell diagonal, `x == y - 1` the 25-cell
/// subdiagonal — 51 of the 2×26×26 product's 676 rows.
const HEADLINE: &str = "\
alphabet ab { '_', 'a'..'z' }

machine {
  tape t: ab;
  tape u: ab;
  entry state s {
    ['a'..'z' as x, 'a'..'z' as y] where { x == y || x == y - 1 } -> stop;
  }
}
";

/// The same rows spelled rule by rule — the derivation the guard must
/// equal. Generated here so the expected rows are never read off a run.
const HEADLINE_WRITTEN_RULES: &str = "\
alphabet ab { '_', 'a'..'z' }

machine {
  tape t: ab;
  tape u: ab;
  entry state s {
%s  }
}
";

fn headline_written() -> String {
    let glyphs: Vec<char> = ('a'..='z').collect();
    let mut rules = String::new();
    for g in &glyphs {
        rules.push_str(&format!("    ['{g}', '{g}'] -> stop;\n"));
    }
    for pair in glyphs.windows(2) {
        rules.push_str(&format!(
            "    ['{}', '{}'] -> stop;\n",
            pair[0], pair[1]
        ));
    }
    HEADLINE_WRITTEN_RULES.replace("%s", &rules)
}

#[test]
fn a_guard_keeps_exactly_the_hand_written_product() {
    let rows = rows_of(HEADLINE);
    assert_eq!(
        rows,
        rows_of(&headline_written()),
        "the guard's rows are the hand-written diagonal + subdiagonal"
    );
    assert_eq!(rows.len(), 51, "26 diagonal + 25 subdiagonal, out of 676");
}

/// A pair fixture whose set order CONTRADICTS the alphabet's: `qq`'s
/// order is `z, a`, the alphabet's `a, z`. The narrowed leg of the guard
/// compares in `qq`'s order, the bare leg on the alphabet; runs witness
/// both orders distinguishably.
const CONTRAST: &str = "\
alphabet ab { '_', 'a', 'z' }
set qq { 'z', 'a' }

machine {
  tape t: ab;
  tape u: ab;
  entry state s {
    ['a'..'z' as x, 'a'..'z' as y] where { x in qq && y in qq && x < y || x == y } -> stop;
  }
}
";

#[test]
fn a_runs_witness_the_set_order_versus_the_alphabet_s() {
    // Tape t carries the first cell, tape u the second; the alphabet has
    // 3 symbols (`_`=0, `a`=1, `z`=2) and the set `qq` orders them z < a.
    let pair = |x: u8, y: u8| vec![(snap(0, &[x], 0), 3), (snap(0, &[y], 0), 3)];
    // The narrowed leg: `x < y` is SET order, so (z, a) — the pair the
    // alphabet would REVERSE — stops…
    assert_eq!(
        run_on(CONTRAST, &pair(2, 1)),
        Outcome::Stopped,
        "the set's order keeps (z, a)"
    );
    // …and the identity leg keeps equals, never (a, z).
    assert_eq!(
        run_on(CONTRAST, &pair(1, 1)),
        Outcome::Stopped,
        "the identity leg keeps (a, a)"
    );
    assert_eq!(
        run_on(CONTRAST, &pair(2, 2)),
        Outcome::Stopped,
        "the identity leg keeps (z, z)"
    );
}

#[test]
fn a_mismatched_pair_is_rejected_by_the_guard() {
    // t='a', u='z': neither leg (identity false; set-order `x < y` false
    // because set order puts z first) — no row, so the state has no
    // transition and the machine traps.
    let outcome = run_on(CONTRAST, &[(snap(0, &[1], 0), 3), (snap(0, &[2], 0), 3)]);
    assert!(
        matches!(outcome, Outcome::Trapped(_)),
        "no leg keeps (a, z): {outcome:?}"
    );
}

/// A routine body's guarded rule folds to the rows of its hand-written
/// twin, exactly as the machine's does.
const ROUTINE_GUARDED: &str = "\
alphabet ab { '_', 'a'..'z' }

routine one(tape n: ab) {
  entry state s {
    ['a'..'z' as x] where { x == 'q' } -> return;
    ['a'..'z' as x] where { x == 'q' - 1 } -> return;
  }
}

machine {
  tape t: ab;
  entry state go {
    ['q'] -> call one(n = t) then fin;
    [*] -> stop;
  }
  state fin { [*] -> stop; }
}
";

const ROUTINE_WRITTEN: &str = "\
alphabet ab { '_', 'a'..'z' }

routine one(tape n: ab) {
  entry state s {
    ['q'] -> return;
    ['p'] -> return;
  }
}

machine {
  tape t: ab;
  entry state go {
    ['q'] -> call one(n = t) then fin;
    [*] -> stop;
  }
  state fin { [*] -> stop; }
}
";

#[test]
fn a_routine_body_folds_to_the_same_rows() {
    assert_eq!(rows_of(ROUTINE_GUARDED), rows_of(ROUTINE_WRITTEN));
}

#[test]
fn a_guard_that_keeps_no_rows_warns_empty_expansion() {
    let out = compile(
        "\
alphabet ab { '_', 'a'..'z' }

machine {
  tape t: ab;
  tape u: ab;
  entry state s {
    ['a'..'z' as x, 'a'..'z' as y] where { x == y && x != y } -> stop;
  }
}
",
        CompileOptions::default(),
    )
    .expect("compiles");
    let codes: Vec<&str> = out
        .report
        .diagnostics
        .iter()
        .map(|d| d.code)
        .collect();
    assert!(
        codes.contains(&"empty-expansion"),
        "a guard keeping nothing warns: {codes:?}"
    );
}

#[test]
fn the_product_threshold_counts_before_the_filter() {
    // 25×25 = 625 rows of product; the guard keeps 25. The cost warning
    // still fires — a guard does not make expansion cheaper.
    let src = "\
alphabet big { 0 .. 24 }

machine {
  tape a: big;
  tape b: big;
  entry state s {
    [0..24 as x, 0..24 as y] where { x == y } -> stop;
    [*, *] -> stop;
  }
}
";
    let out = compile(src, CompileOptions::default()).expect("compiles");
    let codes: Vec<&str> = out.report.diagnostics.iter().map(|d| d.code).collect();
    assert!(
        codes.contains(&"expansion-threshold"),
        "pre-filter product is the cost: {codes:?}"
    );
    let products = lint(src, LintOptions::default())
        .expect("lints")
        .diagnostics
        .into_iter()
        .filter(|d| d.code == "binding-product-threshold")
        .count();
    assert_eq!(products, 1, "the lint recounts the same pre-filter product");
}
