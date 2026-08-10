//! The write-footprint OVER-APPROXIMATION property, end to end through the
//! shipped tool.
//!
//! The inference is one-directional by contract (the soundness contract —
//! crates/turing-machine/src/footprint.rs): an inferred write-set is a
//! SUPERSET of what a run can actually write, so a symbol OUTSIDE a set
//! provably never lands on that tape while a symbol inside it merely may.
//! This corpus is the empirical side of that claim. Every program below is
//! compiled, linked and RUN with a recording decorator on each tape band; the
//! symbols the run actually wrote must all be members of the set the tool
//! reports for the entry world.
//!
//! Both `-O0` and `-O1` are checked, and the relation is asserted per level —
//! never BETWEEN them. The two levels legitimately infer different sets: the
//! optimizer changes which calls exist (a tail call is a shape only the
//! optimizer makes, and inlining dissolves call sites outright), so the sets
//! need not agree. What both must do is contain the run. The `-O1` leg is the
//! load-bearing half: a tail call whose contribution went missing would
//! UNDER-approximate, the one direction this analysis may never take.
//!
//! The inferred sets come from `tmt ir footprints` over `tmt compile
//! --emit-ir` output — the shipped public surface, not the inference behind
//! it. The report's line shape is a pinned CLI contract (docs/tmt/cli.md
//! (tmt ir)), which is what makes parsing it stable here; the coupling is
//! deliberate, and it buys the property a path through the real tool rather
//! than an in-crate shortcut around it.
//!
//! Helpers are local to this file, per the crate's no-shared-test-support
//! convention; the seeds for the committed `.tmc` fixtures are a local copy of
//! the roster `opt_equivalence.rs` runs them on.

use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use mtc_core::formats::executable::Executable;
use mtc_core::formats::tapeblock::TapeSnapshot;
use mtc_core::vm::{ArchRegistry, DeviceFault, Machine, RunLimits, RunOptions, Tape, WideTape};
use mtc_turing_machine::arch::Tm1;
use mtc_turing_machine::cli::execute;
use mtc_turing_machine::ir::{IrProgram, IrTransition};

// ── the recorder ────────────────────────────────────────────────────────────

/// A [`Tape`] decorator that records every symbol index a run actually writes
/// to its band — the `StrictTape` shape (docs/core.md (the tape and device
/// bus)): wrap a device, forward every method, observe one of them.
///
/// The recorded indices are already in the band's OWN alphabet frame, the same
/// frame the entry world's report is written in. The VM resolves a framed
/// call's virtual symbol through the active frame's write map BEFORE the write
/// reaches the device (docs/formats.md (frames region)), so what a decorator
/// on the physical band sees is the caller-frame symbol the footprint
/// projection predicts, never the callee's.
///
/// A `-` keep cell never reaches here at all: the arch lowers a keep marker to
/// no device write, and an all-keep vector does no work whatsoever
/// (docs/tmt/isa.md (reading, writing and moving)), so "actually written" needs
/// no filtering of its own. A faulted write is not recorded — nothing landed on
/// the cell.
struct RecordingTape<T: Tape> {
    inner: T,
    writes: BTreeSet<u32>,
}

impl<T: Tape> RecordingTape<T> {
    fn new(inner: T) -> Self {
        Self {
            inner,
            writes: BTreeSet::new(),
        }
    }
}

impl<T: Tape> Tape for RecordingTape<T> {
    fn alphabet_size(&self) -> u32 {
        self.inner.alphabet_size()
    }

    fn left(&mut self) {
        self.inner.left();
    }

    fn right(&mut self) {
        self.inner.right();
    }

    fn read(&self) -> u32 {
        self.inner.read()
    }

    fn write(&mut self, index: u32) -> Result<(), DeviceFault> {
        self.inner.write(index)?;
        self.writes.insert(index);
        Ok(())
    }

    fn head(&self) -> i64 {
        self.inner.head()
    }
}

// ── the tool route ──────────────────────────────────────────────────────────

/// A private scratch directory. The suffix is the process id plus a
/// per-call counter, so two tests running in parallel — in the same process or
/// in two — never share a path.
fn scratch(name: &str) -> PathBuf {
    static COUNTER: AtomicUsize = AtomicUsize::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("footprint-{name}-{}-{n}", std::process::id()));
    fs::create_dir_all(&dir).expect("scratch dir");
    dir
}

/// A committed `.tmc` fixture under `tests/golden/`.
fn golden_src(name: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden")
        .join(name);
    fs::read_to_string(path).unwrap_or_else(|e| panic!("{name}: {e}"))
}

/// Parse a `tmt ir footprints` report into per-world, per-tape index sets.
///
/// The two line shapes are the leaf's pinned output (docs/tmt/cli.md (tmt ir)):
///
/// ```text
/// world main
///   tape 0 (ctl): writes {3, 4} of 5
/// ```
///
/// An empty set renders as `writes {}` — nothing at all between the braces —
/// so the member split drops empty fragments rather than trying to parse one.
/// Each tape line's own index is checked against its position, which pins the
/// report's tape ORDER to the world's tape order (and so to the band order the
/// run's recorders sit on).
fn parse_footprints(report: &str) -> HashMap<String, Vec<BTreeSet<u32>>> {
    let mut worlds: HashMap<String, Vec<BTreeSet<u32>>> = HashMap::new();
    let mut current = String::new();
    for line in report.lines() {
        if let Some(name) = line.strip_prefix("world ") {
            current = name.trim().to_string();
            worlds.insert(current.clone(), Vec::new());
            continue;
        }
        let Some(rest) = line.strip_prefix("  tape ") else {
            continue;
        };
        let (index, rest) = rest
            .split_once(' ')
            .unwrap_or_else(|| panic!("a tape line names its index: {line}"));
        let (members, _) = rest
            .split_once("writes {")
            .and_then(|(_, after)| after.split_once('}'))
            .unwrap_or_else(|| panic!("a tape line carries a write set: {line}"));
        let set: BTreeSet<u32> = members
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| {
                s.parse()
                    .unwrap_or_else(|e| panic!("a set member is an index ({e}): {line}"))
            })
            .collect();
        let tapes = worlds
            .get_mut(&current)
            .unwrap_or_else(|| panic!("a tape line follows a world line: {line}"));
        assert_eq!(
            index.parse::<usize>().ok(),
            Some(tapes.len()),
            "the report's tape order is the world's tape order: {line}"
        );
        tapes.push(set);
    }
    worlds
}

/// Compile every unit at `level` through the real `tmt compile` (emitting unit
/// 0's world IR alongside its object), link them through `tmt link`, and read
/// back both halves: the linked image and the entry world's inferred write
/// sets as `tmt ir footprints` reports them.
///
/// Unit 0 is the one carrying the `machine` block, so `main` is its entry
/// world and its IR is what the report is read from. One compile feeds both
/// halves, so the image that runs and the IR that is analysed are the same
/// build, not two invocations that happen to agree.
///
/// `link_flags` goes to `tmt link` verbatim, for a fixture that pins the
/// bound-call lowering it means to exercise rather than inheriting the default.
fn build(
    dir: &Path,
    level: &str,
    units: &[String],
    link_flags: &[&str],
) -> (Executable, Vec<BTreeSet<u32>>) {
    let dir = dir.join(level.trim_start_matches('-'));
    fs::create_dir_all(&dir).expect("scratch dir");
    let path = |name: String| dir.join(name).to_str().expect("utf-8 path").to_string();

    let mut link_argv = vec!["link".to_string()];
    link_argv.extend(link_flags.iter().map(|f| f.to_string()));
    let mut ir_json = String::new();
    for (k, unit) in units.iter().enumerate() {
        let src = path(format!("u{k}.tmc"));
        fs::write(&src, unit).expect("write the unit");
        let obj = path(format!("u{k}.tmo"));
        let mut argv = vec![
            "compile".to_string(),
            src,
            "-o".to_string(),
            obj.clone(),
            level.to_string(),
        ];
        if k == 0 {
            argv.push("--emit-ir".to_string());
            ir_json = path(format!("u{k}.ir.json"));
        }
        execute(&argv).unwrap_or_else(|e| panic!("compile {level} u{k}: {e}"));
        link_argv.push(obj);
    }
    let exe_path = path("out.tmx".to_string());
    link_argv.push("-o".to_string());
    link_argv.push(exe_path.clone());
    execute(&link_argv).unwrap_or_else(|e| panic!("link {level}: {e}"));

    let report = execute(&["ir".to_string(), "footprints".to_string(), ir_json])
        .unwrap_or_else(|e| panic!("ir footprints {level}: {e}"))
        .stdout;
    let bytes = fs::read(&exe_path).expect("the linked image");
    let exe = Executable::from_bytes(&bytes).expect("the image decodes");

    let entry = parse_footprints(&report)
        .remove("main")
        .unwrap_or_else(|| panic!("the report names the machine world:\n{report}"));
    assert_eq!(
        entry.len(),
        exe.tape_count as usize,
        "the entry world's report lists one line per band:\n{report}"
    );
    (exe, entry)
}

// ── the run ─────────────────────────────────────────────────────────────────

/// One tape's initial contents: `cells` laid at origin 0, head at the given
/// coordinate. A `Case` is one such spec per physical tape, in tape order.
type Case = &'static [(&'static [u8], i64)];

/// Run `exe` on `seeds` with a recorder on every band, returning the outcome
/// (for failure messages) and the set of symbols each band actually received.
fn run_recording(exe: &Executable, seeds: Case) -> (String, Vec<BTreeSet<u32>>) {
    assert_eq!(
        seeds.len(),
        exe.tape_count as usize,
        "a case must seed exactly one tape per machine tape"
    );
    let mut registry = ArchRegistry::new();
    registry.register(Box::new(Tm1::new(exe.tape_count)));
    let machine = Machine::from_executable(exe, &registry).expect("loads");
    let mut tapes: Vec<RecordingTape<WideTape>> = seeds
        .iter()
        .zip(&exe.alphabet_cardinalities)
        .map(|(&(cells, head), &width)| {
            RecordingTape::new(
                WideTape::from_snapshot(
                    &TapeSnapshot {
                        origin: 0,
                        cells: cells.to_vec(),
                        head,
                        alphabet: None,
                    },
                    width,
                )
                .expect("the seed fits the tape width"),
            )
        })
        .collect();
    let result = {
        let mut devices: Vec<&mut dyn Tape> =
            tapes.iter_mut().map(|t| t as &mut dyn Tape).collect();
        machine
            .run_tapes(
                &mut devices,
                RunOptions {
                    // Explicit and generous: a recursive fixture that
                    // overflowed the return stack would stop writing early and
                    // make its own containment check vacuous.
                    stack_depth: 1024,
                    limits: RunLimits {
                        max_steps: Some(1_000_000),
                        ..Default::default()
                    },
                    ..Default::default()
                },
            )
            .expect("run set-up ok")
    };
    let writes = tapes.into_iter().map(|t| t.writes).collect();
    (format!("{:?}", result.outcome), writes)
}

/// What one checked fixture leaves behind for further pinning: its scratch
/// directory, and per opt level the entry world's inferred sets beside the
/// union of what the fixture's cases ACTUALLY wrote, band by band.
struct Checked {
    dir: PathBuf,
    /// One entry per level, in `-O0`, `-O1` order.
    levels: Vec<LevelCheck>,
}

struct LevelCheck {
    level: &'static str,
    /// Per band, the entry world's inferred write set.
    inferred: Vec<BTreeSet<u32>>,
    /// Per band, the union of every case's actual writes at that level.
    actual: Vec<BTreeSet<u32>>,
}

impl Checked {
    /// Pin a fixture whose inferred sets are exactly what its cases write —
    /// per band, per level, union across the cases.
    ///
    /// Containment alone cannot fail on a WIDER inference (a set degraded to
    /// its tape's whole alphabet contains everything), so a fixture that
    /// claims tightness in prose and checks only containment would survive the
    /// analysis collapsing into "anything may be written". Equality is what
    /// makes that collapse fail, which is why the two fixtures whose
    /// derivations predict an exact answer assert it instead of describing it.
    fn assert_tight(&self, label: &str) {
        for lv in &self.levels {
            assert_eq!(
                lv.actual, lv.inferred,
                "{} {}: the fixture's derivation predicts the inferred sets \
                 EXACTLY — its cases' writes and the inference must agree band \
                 for band (actual union vs inferred)",
                label, lv.level
            );
        }
    }
}

/// THE PROPERTY. Build `units` at `-O0` and at `-O1`, run every case on both
/// images with recording bands, and assert each band's actually-written
/// symbols are contained in the entry world's inferred set for that band.
///
/// The two levels are checked independently and never against each other. The
/// non-vacuity floor is per level and per fixture, not per case: a case may
/// legitimately write nothing (a fixture seeded into an immediate trap or
/// halt), but a fixture whose whole case list writes nothing would pass
/// containment while proving nothing at all.
fn assert_over_approximates(label: &str, units: &[String], cases: &[Case]) -> Checked {
    assert_over_approximates_with(label, units, cases, &[])
}

/// [`assert_over_approximates`] with explicit `tmt link` flags.
fn assert_over_approximates_with(
    label: &str,
    units: &[String],
    cases: &[Case],
    link_flags: &[&str],
) -> Checked {
    assert!(!cases.is_empty(), "{label}: the fixture carries seeds");
    let dir = scratch(label);
    let mut levels = Vec::new();
    for level in ["-O0", "-O1"] {
        let (exe, inferred) = build(&dir, level, units, link_flags);
        let mut union: Vec<BTreeSet<u32>> = vec![BTreeSet::new(); inferred.len()];
        for (i, case) in cases.iter().enumerate() {
            let (outcome, actual) = run_recording(&exe, case);
            assert_eq!(
                actual.len(),
                inferred.len(),
                "{label} {level}: one recorder per reported tape"
            );
            for (band, (act, inf)) in actual.iter().zip(&inferred).enumerate() {
                assert!(
                    act.is_subset(inf),
                    "{label} {level} case {i} ({outcome}): tape {band} actually wrote \
                     {act:?}, which escapes the inferred {inf:?} — the inference \
                     UNDER-approximated",
                );
                union[band].extend(act.iter().copied());
            }
        }
        assert!(
            union.iter().any(|b| !b.is_empty()),
            "{label} {level}: no case wrote anything, so containment proved nothing"
        );
        levels.push(LevelCheck {
            level,
            inferred,
            actual: union,
        });
    }
    Checked { dir, levels }
}

/// Unit 0's emitted world IR at `level` — the very file [`build`] handed to
/// `tmt ir footprints`, so a shape pinned here is a shape of the same program
/// the containment above was checked on.
fn emitted_ir(dir: &Path, level: &str) -> IrProgram {
    let path = dir
        .join(level.trim_start_matches('-'))
        .join("u0.ir.json")
        .to_str()
        .expect("utf-8 path")
        .to_string();
    let text = fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
    IrProgram::from_json(&text).expect("the emitted IR decodes")
}

/// Every transition one world's rules carry, in emission order.
fn transitions<'a>(ir: &'a IrProgram, world: &str) -> Vec<&'a IrTransition> {
    ir.worlds
        .iter()
        .find(|w| w.name == world)
        .unwrap_or_else(|| panic!("the IR carries a world `{world}`"))
        .states
        .iter()
        .flat_map(|s| s.rules.iter().map(|r| &r.transition))
        .collect()
}

/// A single-unit fixture.
fn one(src: String) -> Vec<String> {
    vec![src]
}

// ── the committed `.tmc` corpus ─────────────────────────────────────────────
//
// The seven sources under `tests/golden/`, on the same seeds `opt_equivalence.rs`
// runs them on (a local copy of that roster, per the no-shared-helpers
// convention). Each seed comment states what the seed makes the program do.

#[test]
fn a1_replace_b_over_approximates() {
    // Walk right, 'b'→'a', stop at blank. Seed "bab" (cells [2,1,2]).
    assert_over_approximates(
        "a1_replace_b",
        &one(golden_src("a1_replace_b.tmc")),
        &[&[(&[2, 1, 2], 0)]],
    );
}

#[test]
fn a2_binary_plus_one_over_approximates() {
    // Increment "11", head on the LSB (position 1); the carry extends leftward.
    assert_over_approximates(
        "a2_binary_plus_one",
        &one(golden_src("a2_binary_plus_one.tmc")),
        &[&[(&[2, 2], 1)]],
    );
}

#[test]
fn a3_two_tape_copy_over_approximates() {
    // src "10" (cells [2,1]) copied cell-by-cell onto a blank dst tape. The
    // second band is where the writes land — a per-band check, not a merged one.
    assert_over_approximates(
        "a3_two_tape_copy",
        &one(golden_src("a3_two_tape_copy.tmc")),
        &[&[(&[2, 1], 0), (&[], 0)]],
    );
}

#[test]
fn a4_byte_increment_over_approximates() {
    // Normal (5→6, stop), overflow (126→halt), and blank (0→1, stop). The
    // 127-glyph alphabet is the widest the language allows, so this fixture
    // also exercises the set representation at its ceiling.
    assert_over_approximates(
        "a4_byte_increment",
        &one(golden_src("a4_byte_increment.tmc")),
        &[&[(&[5], 0)], &[(&[126], 0)], &[(&[], 0)]],
    );
}

#[test]
fn a5_call_across_alphabets_over_approximates() {
    // Happy path (ctl "1", data "1"→"10") plus the two holey reads ('a'/'b'
    // under the data head → unmapped-read). The trapping seeds write nothing;
    // the happy one carries the fixture's non-vacuity.
    assert_over_approximates(
        "a5_call_across_alphabets",
        &one(golden_src("a5_call_across_alphabets.tmc")),
        &[
            &[(&[2], 0), (&[4], 0)],
            &[(&[2], 0), (&[1], 0)],
            &[(&[2], 0), (&[2], 0)],
        ],
    );
}

#[test]
fn a6_graph_graft_multi_exit_over_approximates() {
    // x-found (seed "zx", celebrate writes the blank, stop) and blank-found
    // (seed "y", halt with nothing written).
    assert_over_approximates(
        "a6_graph_graft_multi_exit",
        &one(golden_src("a6_graph_graft_multi_exit.tmc")),
        &[&[(&[3, 1], 0)], &[(&[2], 0)]],
    );
}

#[test]
fn nested_graft_over_approximates() {
    // happy (seed "xy", win writes the blank, stop) and lose (seed "x" with no
    // following 'y', halt with nothing written).
    assert_over_approximates(
        "nested_graft",
        &one(golden_src("nested_graft.tmc")),
        &[&[(&[1, 2], 0)], &[(&[1], 0)]],
    );
}

#[test]
fn every_committed_tmc_fixture_is_in_the_corpus() {
    // A drift guard: a `.tmc` fixture added to `tests/golden/` must join this
    // corpus or fail here. Without it a new program could sit uncovered while
    // every test above stays green.
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden");
    let mut found: Vec<String> = fs::read_dir(&dir)
        .expect("the golden directory")
        .map(|e| e.expect("a directory entry").file_name())
        .map(|n| n.to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".tmc"))
        .collect();
    found.sort();
    assert_eq!(
        found,
        vec![
            "a1_replace_b.tmc",
            "a2_binary_plus_one.tmc",
            "a3_two_tape_copy.tmc",
            "a4_byte_increment.tmc",
            "a5_call_across_alphabets.tmc",
            "a6_graph_graft_multi_exit.tmc",
            "nested_graft.tmc",
        ],
        "the committed `.tmc` corpus grew or shrank: add the new fixture above"
    );
}

// ── the local fixtures ──────────────────────────────────────────────────────

/// A mutually recursive pair over one tape: `ping` turns a '0' into a '1' and
/// hands the next cell to `pong`, which turns a '1' into a '0' and hands the
/// next cell back. Both calls are BOUND (an in-unit call to a tape-bearing
/// routine always binds), so the recursion rides the call graph the fixpoint
/// has to close over rather than terminate on.
const MUTUAL_RECURSION: &str = "\
alphabet bits { '_', '0', '1' }

routine ping(tape t: bits) {
  entry state s {
    ['0'] -> write ['1'] move [>] call pong(t = t) then return;
    [*]   -> return;
  }
}

routine pong(tape t: bits) {
  entry state s {
    ['1'] -> write ['0'] move [>] call ping(t = t) then return;
    [*]   -> return;
  }
}

machine {
  tape t: bits;
  entry state go { [*] -> call ping(t = t) then stop; }
}
";

#[test]
fn mutual_recursion_over_approximates() {
    // Seed "01" (bits indices [1,2], head at 0): `ping` writes '1' (2) at cell
    // 0, `pong` writes '0' (1) at cell 1, `ping` meets the blank and the stack
    // unwinds. Actual writes {1,2}; the pair's fixpoint infers exactly {1,2} on
    // both routines and so on `main` through the identity binding — the
    // tightest containment in the corpus, asserted rather than described.
    assert_over_approximates(
        "mutual_recursion",
        &one(MUTUAL_RECURSION.to_string()),
        &[&[(&[1, 2], 0)]],
    )
    .assert_tight("mutual_recursion");
}

/// A two-hop call chain where BOTH hops carry an explicit symbol map: the
/// machine's wide `data` tape reaches `relay` through a wide→bits map, and
/// `relay` reaches `flip` through a bits→bits one. An explicit map is exactly
/// what the inliner refuses (even all-identity pairs), so both calls survive
/// `-O1` and the projection is exercised at both levels rather than only at
/// `-O0`.
const MAP_CHAIN: &str = "\
alphabet wide { '_', 'a', 'b', '0', '1' }
alphabet bits { '_', '0', '1' }

routine flip(tape num: bits) {
  entry state s {
    ['0'] -> write ['1'] return;
    ['1'] -> write ['0'] return;
    [*]   -> return;
  }
}

routine relay(tape v: bits) {
  entry state s {
    [*] -> call flip(num = v with map { '0' -> '0', '1' -> '1' }) then return;
  }
}

machine {
  tape ctl:  bits;
  tape data: wide;
  entry state s {
    [*, *] -> call relay(v = data with map { '0' -> '0', '1' -> '1' }) then stop;
  }
}
";

#[test]
fn map_chain_over_approximates() {
    // Two seeds, ctl blank throughout: data "0" (wide index 3) flips to "1",
    // and data "1" (wide 4) flips to "0". Derivation of the inferred set the
    // two seeds are chosen against: `flip` writes bits {1,2}; `relay`'s map
    // pairs both digits across EQUAL cardinalities, so {1,2} survives the hop;
    // the machine's map is CLOSED (5 glyphs against 3) and carries bits 1→wide
    // 3 and bits 2→wide 4, so `data` infers {3,4} and `ctl`, bound nowhere,
    // infers the empty set.
    //
    // Neither SEED is tight on its own — one run writes {4}, the other {3} —
    // but their UNION is exactly {3,4} on `data` and empty on `ctl`, which is
    // the relation `assert_tight` checks and the reason the fixture carries two
    // seeds rather than one.
    //
    // The lowering is pinned to `frames` rather than left to the default:
    // frames is what routes the write through the composite's write map, and
    // that map is the whole reason a symbol recorded at the band is in the
    // CALLER's frame. A future default that stamped this chain mono instead
    // would leave that path unexercised while every assertion here still
    // passed.
    let checked = assert_over_approximates_with(
        "map_chain",
        &one(MAP_CHAIN.to_string()),
        &[&[(&[], 0), (&[3], 0)], &[(&[], 0), (&[4], 0)]],
        &["--call-mech", "frames"],
    );
    checked.assert_tight("map_chain");
    let dir = &checked.dir;

    // The shape the fixture claims: BOTH mapped calls survive `-O1`, so the
    // projection really is exercised at both levels. Believing it without
    // asserting it would let a future inliner swallow the chain and leave the
    // `-O1` leg checking a program with no calls in it at all.
    let ir = emitted_ir(dir, "-O1");
    assert!(
        matches!(
            transitions(&ir, "main").as_slice(),
            [IrTransition::CallThen { target, binding, .. }]
                if target == "relay" && !binding.is_empty()
        ),
        "-O1 keeps the machine's mapped call: {:?}",
        transitions(&ir, "main")
    );
    assert!(
        matches!(
            transitions(&ir, "relay").as_slice(),
            [IrTransition::CallThen { target, binding, .. }]
                if target == "flip" && !binding.is_empty()
        ),
        "-O1 keeps the inner mapped call: {:?}",
        transitions(&ir, "relay")
    );
}

/// The tail-call chain, in TWO compilation units — the fixture the `-O1` leg
/// exists for.
///
/// A tail call is only ever made from a BINDLESS `call … then return`, and the
/// front end never mints a bindless in-unit call to a tape-bearing routine, so
/// the callee has to live outside the unit. `hop` is therefore the middle of a
/// cross-unit chain: `main` calls it BOUND (in-unit), and it calls `ext::leaf`
/// bindlessly in tail position, which `-O1` rewrites to a tail call. `hop`
/// carries a call, so the inliner — which takes leaf callees only — never
/// splices it away and the chain survives to link.
///
/// The entry world's inferred set is therefore reached THROUGH the tail call:
/// `ext::leaf` is outside `hop`'s unit, so the tail call answers with `hop`'s
/// whole alphabet, which the identity binding carries onto `main`'s tape. Drop
/// the tail-call edge and `main` infers nothing while the run still writes —
/// which is what makes this fixture the corpus's mutation detector.
const TAIL_CHAIN_MAIN: &str = "\
alphabet bits { '_', '0', '1' }

use ext::leaf;

routine hop(tape t: bits) {
  entry state s { [*] -> call leaf() then return; }
}

machine {
  tape t: bits;
  entry state go { [*] -> call hop(t = t) then stop; }
}
";

/// The chain's far end, its own compilation unit: the routine `hop` tail-calls.
const TAIL_CHAIN_LEAF: &str = "\
alphabet bits { '_', '0', '1' }

namespace ext {
  export routine leaf(tape t: bits) {
    entry state s { [*] -> write ['1'] return; }
  }
}
";

#[test]
fn cross_unit_tail_call_chain_over_approximates() {
    // A blank tape: `main` calls `hop`, `hop` transfers to `ext::leaf`, which
    // writes '1' (bits index 2) and returns straight to `main`, which stops.
    // Actual {2}; inferred {0,1,2} — the deliberate slack of an out-of-unit
    // callee, and the whole point of the fixture is that the slack is REACHED.
    let checked = assert_over_approximates(
        "cross_unit_tail_call",
        &[TAIL_CHAIN_MAIN.to_string(), TAIL_CHAIN_LEAF.to_string()],
        &[&[(&[], 0)]],
    );

    // The structural pin behind the fixture's whole reason to exist, in its
    // two-sided form — which also spells out the level asymmetry this file's
    // "the levels infer different sets" claim rests on. `hop`'s one rule is a
    // plain bindless call at `-O0` and a TAIL call at `-O1`. If a later
    // optimizer change quietly stopped producing the tail call here, the
    // corpus would lose its only coverage of that edge while staying green,
    // and the containment above would be checked on the `-O0` shape twice.
    let o0 = emitted_ir(&checked.dir, "-O0");
    assert!(
        matches!(
            transitions(&o0, "hop").as_slice(),
            [IrTransition::CallThen { target, binding, .. }]
                if target == "ext::leaf" && binding.is_empty()
        ),
        "-O0 leaves the bindless call in place: {:?}",
        transitions(&o0, "hop")
    );
    let o1 = emitted_ir(&checked.dir, "-O1");
    assert!(
        matches!(
            transitions(&o1, "hop").as_slice(),
            [IrTransition::TailCall { target }] if target == "ext::leaf"
        ),
        "-O1 rewrites it to a tail call — the edge this corpus exists to \
         cover: {:?}",
        transitions(&o1, "hop")
    );
}
