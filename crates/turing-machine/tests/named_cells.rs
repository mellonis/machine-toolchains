//! Named, sparse rule-vector cells (`[king: 'x']`) and their resolution into
//! the positional form (docs/tmt/language.md (named vector cells)): the three
//! refusals with their codes and spans — a name that is no tape of this
//! world, one tape named twice in a vector, and named mixed with positional
//! cells — and the fold itself, named spelling running identically to its
//! positional twin.

use mtc_core::diagnostics::{Pos, Span};
use mtc_turing_machine::compiler::{CompileError, CompileErrorKind, CompileOptions, compile};

fn compile_err(src: &str) -> CompileError {
    match compile(src, CompileOptions::default()) {
        Ok(_) => panic!("expected a compile error, got a successful compile"),
        Err(e) => e,
    }
}

/// The 1-based position of the first occurrence of `needle` in `src`.
fn at(src: &str, needle: &str) -> Pos {
    for (i, line) in src.lines().enumerate() {
        if let Some(col) = line.find(needle) {
            return Pos {
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

const UNKNOWN_NAME: &str = "\
alphabet ab { '_', 'a', 'b' }

machine {
  tape t: ab;
  tape u: ab;
  entry state s {
    [t: 'a', qween: 'b'] -> stop;
  }
}
";

#[test]
fn a_cell_name_that_is_no_tape_of_this_world_is_refused() {
    let e = compile_err(UNKNOWN_NAME);
    assert_eq!(e.kind.code(), "unresolved-tape-target");
    assert_eq!(e.span, cover(UNKNOWN_NAME, "qween"));
    assert!(matches!(&e.kind, CompileErrorKind::UnresolvedTapeTarget(n) if n == "qween"));
}

const STATE_PARAM_NAME: &str = "\
alphabet ab { '_', 'a', 'b' }

graph g(tape t: ab, state done) {
  entry state s {
    ['a' as v] -> write [done: {v}] done;
  }
}
";

#[test]
fn a_cell_naming_a_state_parameter_is_refused() {
    // `done` is the graph's exit parameter, not a tape.
    let e = compile_err(STATE_PARAM_NAME);
    assert_eq!(e.kind.code(), "unresolved-tape-target");
    // The span is the NAME's own.
    let p = at(STATE_PARAM_NAME, "done: {v}");
    assert_eq!(e.span, Span::new(p.line, p.col, p.line, p.col + 4));
    assert!(matches!(&e.kind, CompileErrorKind::UnresolvedTapeTarget(n) if n == "done"));
}

const TWIN_NAME: &str = "\
alphabet ab { '_', 'a', 'b' }

machine {
  tape t: ab;
  tape u: ab;
  entry state s {
    [t: 'a', t: 'b'] -> stop;
  }
}
";

#[test]
fn one_tape_named_twice_in_one_vector_is_refused() {
    let e = compile_err(TWIN_NAME);
    assert_eq!(e.kind.code(), "duplicate-named-cell");
    // The SECOND name is the offender — the first committed the tape. The
    // span is the name's own.
    assert_eq!(
        e.span,
        Span::point(at(TWIN_NAME, "t: 'b'").line, at(TWIN_NAME, "t: 'b'").col)
    );
    assert!(matches!(&e.kind, CompileErrorKind::DuplicateNamedCell { name } if name == "t"));
}

const MIXED_AFTER: &str = "\
alphabet ab { '_', 'a', 'b' }

machine {
  tape t: ab;
  tape u: ab;
  entry state s {
    [t: 'a', 'b'] -> stop;
  }
}
";

const MIXED_BEFORE: &str = "\
alphabet ab { '_', 'a', 'b' }

machine {
  tape t: ab;
  tape u: ab;
  entry state s {
    ['a', *] -> move [., t: >] stop;
  }
}
";

#[test]
fn named_and_positional_cells_in_one_vector_are_refused() {
    // The first cell sets the vector's form; the first cell that breaks it is
    // the offender — whichever side it is on.
    let e = compile_err(MIXED_AFTER);
    assert_eq!(e.kind.code(), "mixed-cell-forms");
    let p = at(MIXED_AFTER, "'b'] -> stop");
    assert_eq!(e.span, Span::new(p.line, p.col, p.line, p.col + 3));
    assert!(matches!(e.kind, CompileErrorKind::MixedCellForms));

    let e = compile_err(MIXED_BEFORE);
    assert_eq!(e.kind.code(), "mixed-cell-forms");
    assert_eq!(e.span, cover(MIXED_BEFORE, "t: >"));
}

// ---------------------------------------------------------------------------
// The fold: a named spelling and its positional twin are one program.
// ---------------------------------------------------------------------------

use mtc_core::formats::executable::Executable;
use mtc_core::formats::tapeblock::TapeSnapshot;
use mtc_core::linker::LinkOptions;
use mtc_core::vm::{ArchRegistry, Machine, Outcome, RunLimits, RunOptions, Tape, WideTape};
use mtc_turing_machine::arch::Tm1;
use mtc_turing_machine::asm::link;

fn snap(origin: i64, cells: &[u8], head: i64) -> TapeSnapshot {
    TapeSnapshot {
        origin,
        cells: cells.to_vec(),
        head,
        alphabet: None,
    }
}

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

const FOLD_NAMED: &str = "\
alphabet ab { '_', 'a', 'b' }

machine {
  tape x: ab;
  tape y: ab;
  entry state s {
    [x: 'a'] -> write [x: 'b'] stop;
  }
}
";

const FOLD_POSITIONAL: &str = "\
alphabet ab { '_', 'a', 'b' }

machine {
  tape x: ab;
  tape y: ab;
  entry state s {
    ['a', *] -> write ['b', -] stop;
  }
}
";

#[test]
fn a_named_spelling_and_its_positional_twin_are_one_program() {
    // x holds 'a' under its head, y holds 'b' under its: the rule rewrites
    // x's cell to 'b' and stops. Both spellings must derive the same final
    // tapes — x: 'a' becomes 'b', y: 'b' untouched.
    let seeds = [(snap(0, &[1], 0), 3), (snap(0, &[2], 0), 3)];
    let named = compile_link_run(FOLD_NAMED, &seeds);
    let positional = compile_link_run(FOLD_POSITIONAL, &seeds);
    assert_eq!(named, positional);
    assert_eq!(named.0, Outcome::Stopped);
    assert_eq!(named.1, vec![snap(0, &[2], 0), snap(0, &[2], 0)]);
}

// ---------------------------------------------------------------------------
// named ≡ positional, byte for byte: the two spellings of one program yield
// IDENTICAL match rows in the image and identical runs. Fixtures cover one
// world per surface: a machine's own rules, a routine body, and a grafted
// graph body. The pin is discriminating against a WRONG name mapping (two
// names swapped is legal and yields different rows) — recorded as the round's
// swap mutation.
// ---------------------------------------------------------------------------

use mtc_core::formats::object::ObjectFile;
use mtc_turing_machine::asm::disassemble_executable_with_map;

fn object(src: &str) -> ObjectFile {
    compile(src, CompileOptions::default())
        .unwrap_or_else(|e| panic!("the fixture must compile: {e}"))
        .object
}

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

const WIDE_NAMED: &str = "\
alphabet ab { '_', 'a', 'b' }

machine {
  tape x: ab;
  tape y: ab;
  tape z: ab;
  entry state s {
    [y: 'a', z: 'b'] -> write [y: 'b', z: 'a'] move [y: >, z: <] goto s;
    [x: 'a' as v] -> write [x: {v}] goto s;
    [*, *, '_'] -> stop;
  }
}
";

const WIDE_POSITIONAL: &str = "\
alphabet ab { '_', 'a', 'b' }

machine {
  tape x: ab;
  tape y: ab;
  tape z: ab;
  entry state s {
    [*, 'a', 'b'] -> write [-, 'b', 'a'] move [., >, <] goto s;
    ['a' as v, *, *] -> write [{v}, -, -] goto s;
    [*, *, '_'] -> stop;
  }
}
";

#[test]
fn machine_rules_fold_to_the_same_rows() {
    assert_eq!(rows_of(WIDE_NAMED), rows_of(WIDE_POSITIONAL));
}

const ROUTINE_NAMED: &str = "\
alphabet ab { '_', 'a', 'b' }

routine step(tape n: ab) {
  entry state s {
    [n: 'a'] -> write [n: 'b'] move [n: >] goto s;
    [n: '_'] -> return;
  }
}

machine {
  tape t: ab;
  tape u: ab;
  entry state go {
    [t: 'a'] -> call step(n = t) then fin;
    [u: 'b'] -> call step(n = u) then fin;
  }
  state fin { [*, *] -> stop; }
}
";

const ROUTINE_POSITIONAL: &str = "\
alphabet ab { '_', 'a', 'b' }

routine step(tape n: ab) {
  entry state s {
    ['a'] -> write ['b'] move [>] goto s;
    ['_'] -> return;
  }
}

machine {
  tape t: ab;
  tape u: ab;
  entry state go {
    ['a', *] -> call step(n = t) then fin;
    [*, 'b'] -> call step(n = u) then fin;
  }
  state fin { [*, *] -> stop; }
}
";

#[test]
fn a_routine_body_folds_to_the_same_rows() {
    assert_eq!(rows_of(ROUTINE_NAMED), rows_of(ROUTINE_POSITIONAL));
}

const GRAFT_NAMED: &str = "\
alphabet ab { '_', 'a', 'b' }

graph g(tape n: ab, state done) {
  entry state scan {
    [n: 'a'] -> write [n: 'b'] move [n: >] goto scan;
    [n: '_'] -> done;
  }
}

machine {
  tape t: ab;
  entry graft g(n = t, done = fin) as inst;
  state fin { [*] -> stop; }
}
";

const GRAFT_POSITIONAL: &str = "\
alphabet ab { '_', 'a', 'b' }

graph g(tape n: ab, state done) {
  entry state scan {
    ['a'] -> write ['b'] move [>] goto scan;
    ['_'] -> done;
  }
}

machine {
  tape t: ab;
  entry graft g(n = t, done = fin) as inst;
  state fin { [*] -> stop; }
}
";

#[test]
fn a_grafted_graph_body_folds_to_the_same_rows() {
    assert_eq!(rows_of(GRAFT_NAMED), rows_of(GRAFT_POSITIONAL));
}

// ---------------------------------------------------------------------------
// Lint fidelity: the findings over a named sparse spelling are exactly the
// findings over its positional twin — the fill's defaults neither fabricate
// activity nor hide it (the `unused-*` family sees untouched tapes
// identically).
// ---------------------------------------------------------------------------

use mtc_turing_machine::lint::{LintOptions, lint};

const FOUR_TAPE_NAMED: &str = "\
alphabet ab { '_', 'a', 'b' }

machine {
  tape a1: ab;
  tape a2: ab;
  tape a3: ab;
  tape a4: ab;
  entry state s {
    [a2: 'a'] -> write [a2: 'b'] goto s;
    [a1: '_'] -> stop;
  }
}
";

const FOUR_TAPE_POSITIONAL: &str = "\
alphabet ab { '_', 'a', 'b' }

machine {
  tape a1: ab;
  tape a2: ab;
  tape a3: ab;
  tape a4: ab;
  entry state s {
    [*, 'a', *, *] -> write [-, 'b', -, -] goto s;
    ['_', *, *, *] -> stop;
  }
}
";

#[test]
fn lint_findings_are_the_twin_s_exact() {
    let findings = |src: &str| -> Vec<(String, String)> {
        lint(src, LintOptions::default())
            .expect("lints")
            .diagnostics
            .into_iter()
            .map(|d| (d.code.to_string(), d.message))
            .collect()
    };
    let named = findings(FOUR_TAPE_NAMED);
    assert_eq!(
        named,
        findings(FOUR_TAPE_POSITIONAL),
        "the named spelling must earn exactly its twin's findings"
    );
    // The fill must not hide the untouched tapes: a3 and a4 stay reported.
    let unused: Vec<&str> = named
        .iter()
        .filter(|(code, _)| code == "unused-tape")
        .map(|(_, m)| m.as_str())
        .collect();
    assert_eq!(unused.len(), 2, "a3 and a4 are untouched, named: {unused:?}");
}
