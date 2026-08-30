//! The worked examples under `docs/examples/`, run against their own case
//! tables.
//!
//! Those examples are documentation that makes claims — measured step counts,
//! stated outputs, a described failure for every way an input can be wrong —
//! and nothing else in this suite checks the five that are not the flagship.
//! This does, from the same `cases` files the directory's own `run.sh` reads,
//! so the two can never assert different things.
//!
//! It does NOT replace `run.sh`. That harness drives the CLI, takes ad-hoc
//! expressions on argv, and carries the three differentials (`-O0` against
//! `-O1`, the three bound-call lowerings, a disassemble/reassemble round
//! trip). This is the subset worth having in CI: no built binary, no shell,
//! one process per test under nextest.
//!
//! Alphabets are never hardcoded here. `machine_tape_layout` reports each
//! `machine` block's tapes and their glyphs in position order — the same
//! source the `tape-block` CLI reads — so reordering an alphabet in a `.tmc`
//! cannot leave this file mapping to stale indices.

use std::fs;
use std::path::{Path, PathBuf};

use mtc_core::formats::executable::Executable;
use mtc_core::formats::tapeblock::TapeSnapshot;
use mtc_core::linker::LinkOptions;
use mtc_core::vm::{ArchRegistry, Machine, Outcome, RunLimits, RunOptions, Tape, Trap, WideTape};
use mtc_turing_machine::arch::Tm1;
use mtc_turing_machine::asm::link;
use mtc_turing_machine::compiler::{CompileOptions, TapeLayout, compile, machine_tape_layout};
use mtc_turing_machine::stdlib;

fn examples_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/examples")
}

/// A case's expected ending. The three are kept apart deliberately: stopping
/// is success, halting is a program reporting a fault it detected, and
/// trapping is a state entered with no matching rule — a bug in the program
/// rather than in its input. A step-limit trap is NOT accepted for `trap`,
/// so a case that merely ran out of budget can never pass as a diagnosis.
enum Expect {
    Value(String),
    Halt,
    Trap,
}

/// One tape to seed before a run: which tape, what to write from its origin,
/// and where its head starts.
struct Seed {
    tape: usize,
    cells: Vec<u8>,
    head: i64,
}

struct Case {
    label: String,
    expect: Expect,
    slow: bool,
}

/// Read an example's `cases` table. Blank lines and `#` comments are skipped;
/// a value beginning `@` names a sidecar file holding an expected value too
/// long for the table.
fn cases(name: &str) -> Vec<Case> {
    let dir = examples_dir().join(name);
    let text = fs::read_to_string(dir.join("cases")).expect("cases table present");
    let mut out = Vec::new();
    for line in text.lines() {
        let t = line.trim_start();
        if t.is_empty() || t.starts_with('#') {
            continue;
        }
        let mut parts = line.splitn(3, ';');
        let label = parts.next().unwrap_or("").to_string();
        let want = parts.next().unwrap_or("").trim().to_string();
        let tags = parts.next().unwrap_or("");
        let expect = match want.as_str() {
            "halt" => Expect::Halt,
            "trap" => Expect::Trap,
            _ if want.starts_with('@') => Expect::Value(
                fs::read_to_string(dir.join(&want[1..]))
                    .expect("sidecar expected file present")
                    .trim_end()
                    .to_string(),
            ),
            _ => Expect::Value(want),
        };
        out.push(Case {
            label,
            expect,
            slow: tags.split_whitespace().any(|t| t == "slow"),
        });
    }
    out
}

/// Compile and link one example. `rpn` is the only one that reaches outside
/// its own unit — it calls `std::binaryNumbers` — so the embedded standard
/// library is offered to every link and the reachability pass drops it where
/// nothing calls it.
fn build(source: &str) -> Executable {
    let path = examples_dir().join(source);
    let src = fs::read_to_string(&path).unwrap_or_else(|_| panic!("{source} present"));
    let out = compile(&src, CompileOptions::default())
        .unwrap_or_else(|e| panic!("{source} compiles: {e:?}"));
    link(
        &[out.object],
        &[stdlib::object().clone()],
        LinkOptions::default(),
    )
    .unwrap_or_else(|e| panic!("{source} links: {e:?}"))
    .executable
}

fn layout(source: &str) -> Vec<TapeLayout> {
    let src = fs::read_to_string(examples_dir().join(source)).expect("source present");
    machine_tape_layout(&src)
        .expect("source resolves")
        .expect("the example declares a machine block")
}

fn tape_index(tapes: &[TapeLayout], name: &str) -> usize {
    tapes
        .iter()
        .position(|t| t.name == name)
        .unwrap_or_else(|| panic!("no tape named {name}"))
}

/// A glyph's index in its tape's alphabet, which is what the machine stores.
fn idx(tapes: &[TapeLayout], tape: usize, glyph: &str) -> u8 {
    tapes[tape]
        .glyphs
        .iter()
        .position(|g| g == glyph)
        .unwrap_or_else(|| panic!("glyph {glyph:?} is not in tape {tape}'s alphabet")) as u8
}

/// The glyph a cell holds, for rendering a band back into a comparable value.
fn glyph(tapes: &[TapeLayout], tape: usize, cell: u8) -> &str {
    &tapes[tape].glyphs[cell as usize]
}

/// The cell under a snapshot's head, or `None` past the recorded band.
fn at_head(s: &TapeSnapshot) -> Option<u8> {
    usize::try_from(s.head - s.origin)
        .ok()
        .and_then(|i| s.cells.get(i).copied())
}

/// Seed the band and run. Tapes no seed names start blank.
fn run(exe: &Executable, seeds: &[Seed], max_steps: u64) -> (Outcome, Vec<TapeSnapshot>) {
    let n = exe.tape_count as usize;
    let mut tapes: Vec<WideTape> = (0..n)
        .map(|i| {
            let width = exe.alphabet_cardinalities[i];
            match seeds.iter().find(|s| s.tape == i) {
                Some(seed) => WideTape::from_snapshot(
                    &TapeSnapshot {
                        origin: 0,
                        cells: seed.cells.clone(),
                        head: seed.head,
                        alphabet: None,
                    },
                    width,
                )
                .expect("seed fits the tape's width"),
                None => WideTape::new(width),
            }
        })
        .collect();

    let mut registry = ArchRegistry::new();
    registry.register(Box::new(Tm1::new(exe.tape_count)));
    let machine = Machine::from_executable(exe, &registry).expect("loads");
    let mut devices: Vec<&mut dyn Tape> = tapes.iter_mut().map(|t| t as &mut dyn Tape).collect();
    let result = machine
        .run_tapes(
            &mut devices,
            RunOptions {
                limits: RunLimits {
                    max_steps: Some(max_steps),
                    ..Default::default()
                },
                ..Default::default()
            },
        )
        .expect("run set-up ok");
    drop(devices);
    (
        result.outcome,
        tapes.iter().map(|t| t.to_snapshot()).collect(),
    )
}

// ---------------------------------------------------------------------------
// Per-example encoding and decoding
//
// This is the only place an example's own shape is spelled out: which tape
// takes the input, where its head starts, and how a case label becomes cells.
// Everything else — tape count, widths, glyph indices — is read from the
// program itself.
// ---------------------------------------------------------------------------

/// An RPN expression onto the `expr` tape: one glyph per character, a space
/// written as the blank, then the blank-and-sentinel the scanner ends on.
fn seed_expr(tapes: &[TapeLayout], label: &str, blank: &str) -> Vec<Seed> {
    let t = tape_index(tapes, "expr");
    let mut cells: Vec<u8> = label
        .chars()
        .map(|c| {
            let g = if c == ' ' {
                blank.to_string()
            } else {
                c.to_string()
            };
            idx(tapes, t, &g)
        })
        .collect();
    cells.push(idx(tapes, t, blank));
    cells.push(idx(tapes, t, "#"));
    vec![Seed {
        tape: t,
        cells,
        head: 0,
    }]
}

/// The same, for the machines whose digits are hex: a digit's glyph label is
/// its VALUE, so `F` is the glyph named `15`, not the character.
fn seed_expr_hex(tapes: &[TapeLayout], label: &str) -> Vec<Seed> {
    let t = tape_index(tapes, "expr");
    let mut cells: Vec<u8> = label
        .chars()
        .map(|c| {
            let g = match c {
                ' ' => "_".to_string(),
                'a'..='f' => (c as u8 - b'a' + 10).to_string(),
                'A'..='F' => (c as u8 - b'A' + 10).to_string(),
                _ => c.to_string(),
            };
            idx(tapes, t, &g)
        })
        .collect();
    cells.push(idx(tapes, t, "_"));
    cells.push(idx(tapes, t, "#"));
    vec![Seed {
        tape: t,
        cells,
        head: 0,
    }]
}

/// A whole band as one string, blanks included — which is what the delimited
/// representation's own value looks like: `^` digits `$`.
fn band(tapes: &[TapeLayout], t: usize, s: &TapeSnapshot) -> String {
    s.cells.iter().map(|&c| glyph(tapes, t, c)).collect()
}

/// The non-blank cells of a band, hex digits rendered as characters.
fn hex_band(tapes: &[TapeLayout], t: usize, s: &TapeSnapshot) -> String {
    s.cells
        .iter()
        .map(|&c| glyph(tapes, t, c))
        .filter(|g| *g != "_")
        .map(|g| match g.parse::<u8>() {
            Ok(v @ 10..=15) => char::from(b'A' + v - 10).to_string(),
            _ => g.to_string(),
        })
        .collect()
}

struct Example {
    name: &'static str,
    source: &'static str,
    seed: fn(&[TapeLayout], &str) -> Vec<Seed>,
    read: fn(&[TapeLayout], &[TapeSnapshot]) -> String,
    max_steps: u64,
}

fn examples() -> Vec<Example> {
    vec![
        Example {
            name: "rpn",
            source: "rpn/rpn.tmc",
            seed: |t, l| seed_expr(t, l, "_"),
            read: |t, s| {
                let i = tape_index(t, "stack");
                band(t, i, &s[i])
            },
            max_steps: 1_000_000,
        },
        Example {
            name: "rpnhex",
            source: "rpnhex/rpnhex.tmc",
            seed: seed_expr_hex,
            read: |t, s| {
                let i = tape_index(t, "stack");
                let v = hex_band(t, i, &s[i]);
                if v.is_empty() { "<empty>".into() } else { v }
            },
            max_steps: 2_000_000,
        },
        Example {
            name: "rpnreg",
            source: "rpnreg/rpnreg.tmc",
            seed: seed_expr_hex,
            read: |t, s| {
                let i = tape_index(t, "stack");
                let v = hex_band(t, i, &s[i]);
                if v.is_empty() { "<empty>".into() } else { v }
            },
            max_steps: 2_000_000,
        },
        Example {
            // The one example whose value is not a band: a digit per tape, so
            // it is what the four heads read, most significant first.
            name: "rpnwide",
            source: "rpnwide/rpnwide.tmc",
            seed: seed_expr_hex,
            read: |t, s| {
                let mut out = String::new();
                for n in ["sA", "sB", "sC", "sD"] {
                    let i = tape_index(t, n);
                    match at_head(&s[i]).map(|c| glyph(t, i, c)) {
                        Some("_") | None => return "<none>".into(),
                        Some(g) => out.push_str(&match g.parse::<u8>() {
                            Ok(v @ 10..=15) => char::from(b'A' + v - 10).to_string(),
                            _ => g.to_string(),
                        }),
                    }
                }
                out
            },
            max_steps: 2_000_000,
        },
        Example {
            // The label is the exponent; the band is `s b 1xN k` with the head
            // on the 'b'. The value is a shape-and-count summary, because the
            // band at N=24 is sixteen million cells.
            name: "pow2",
            source: "pow2/pow2.tmc",
            seed: |t, l| {
                let i = tape_index(t, "main");
                let n: usize = l.trim().parse().expect("the label is an exponent");
                let mut cells = vec![idx(t, i, "s"), idx(t, i, "b")];
                cells.extend(std::iter::repeat_n(idx(t, i, "1"), n));
                cells.push(idx(t, i, "k"));
                vec![Seed {
                    tape: i,
                    cells,
                    head: 1,
                }]
            },
            read: |t, s| {
                let i = tape_index(t, "main");
                let b = band(t, i, &s[i]);
                match b.strip_prefix("sb").and_then(|r| r.strip_suffix('k')) {
                    Some(ones) => format!("sb{}k", ones.len()),
                    None => "<none>".into(),
                }
            },
            max_steps: 600_000_000,
        },
        Example {
            // The label is brainfuck source; the value is the bytes '.' emitted.
            // A `bytes` tape's index 0 IS its blank, so a genuine zero byte
            // would be dropped — none of the cases emit one.
            name: "brainfuck-utm",
            source: "brainfuck-utm/brainfuck-utm.tmc",
            seed: |t, l| {
                let i = tape_index(t, "prog");
                let mut cells: Vec<u8> = l.chars().map(|c| idx(t, i, &c.to_string())).collect();
                cells.push(idx(t, i, "H"));
                vec![Seed {
                    tape: i,
                    cells,
                    head: 0,
                }]
            },
            read: |t, s| {
                let i = tape_index(t, "out");
                let v: Vec<&str> = s[i]
                    .cells
                    .iter()
                    .map(|&c| glyph(t, i, c))
                    .filter(|g| *g != "0")
                    .collect();
                if v.is_empty() {
                    "<empty>".into()
                } else {
                    v.join(",")
                }
            },
            max_steps: 10_000_000,
        },
    ]
}

/// Long-running cases are opt-in, the same way `run.sh --slow` gates them:
/// pow2 at N=24 is 419 million steps.
fn slow_enabled() -> bool {
    std::env::var_os("MTC_EXAMPLES_SLOW").is_some()
}

/// Run one example's whole table. One test per example rather than one for
/// all six: under nextest that is a process each, so the wall clock is the
/// slowest example rather than their sum, and a failure names its example
/// without taking the other five down with it.
fn check(name: &str, least: usize) {
    let ex = examples()
        .into_iter()
        .find(|e| e.name == name)
        .expect("known example");
    let tapes = layout(ex.source);
    let exe = build(ex.source);
    let mut ran = 0usize;
    for case in cases(ex.name) {
        if case.slow && !slow_enabled() {
            continue;
        }
        let seeds = (ex.seed)(&tapes, &case.label);
        let (outcome, snaps) = run(&exe, &seeds, ex.max_steps);
        let where_ = format!("{} case {:?}", ex.name, case.label);
        match case.expect {
            Expect::Halt => assert_eq!(outcome, Outcome::Halted, "{where_} must halt"),
            Expect::Trap => assert!(
                matches!(outcome, Outcome::Trapped(Trap::NoTransition { .. })),
                "{where_} must trap on a missing transition, got {outcome:?}"
            ),
            Expect::Value(want) => {
                assert_eq!(outcome, Outcome::Stopped, "{where_} must stop");
                assert_eq!((ex.read)(&tapes, &snaps), want, "{where_} value");
            }
        }
        ran += 1;
    }
    assert!(
        ran >= least,
        "{name}'s case table shrank: {ran} ran, expected at least {least}"
    );
}

#[test]
fn rpn_cases_hold() {
    check("rpn", 18);
}

#[test]
fn rpnhex_cases_hold() {
    check("rpnhex", 27);
}

#[test]
fn rpnreg_cases_hold() {
    check("rpnreg", 27);
}

#[test]
fn rpnwide_cases_hold() {
    check("rpnwide", 27);
}

#[test]
fn pow2_cases_hold() {
    check("pow2", 9);
}

#[test]
fn brainfuck_utm_cases_hold() {
    check("brainfuck-utm", 10);
}
