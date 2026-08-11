//! The volatile world's standing proof: the two build columns of one
//! source agree on everything a program can observe, and every path that
//! can produce an image produces the SAME image.
//!
//! A `volatile main` (docs/pmt/language.md (volatile programs)) sets the
//! object's program bit; the linker reads that bit off the entry's owner
//! and selects a column per name (docs/core.md (linking)). These tests
//! drive that whole chain — source keyword, program bit, column choice,
//! emitted image — rather than any one stage of it. The pass-level
//! verdicts live in `gated_passes.rs`, the merged object's shape in
//! `variant_columns.rs`, the assembly text form in `asm_volatile.rs`, and
//! the driver's per-direction disk/in-memory rule in `build_driver.rs`;
//! what is proven here is the composition of all of them.

use std::fs;
use std::path::PathBuf;

use mtc_core::formats::executable::Executable;
use mtc_core::formats::object::{BlobVariant, ObjectFile, SymbolDef};
use mtc_core::linker::{LinkOptions, LinkOutput};
use mtc_core::vm::{ArchRegistry, InfiniteTape, Machine, Outcome, RunLimits, RunOptions, Trap};
use mtc_post_machine::arch::Pm1;
use mtc_post_machine::arch::opcodes::*;
use mtc_post_machine::asm::{assemble, link};
use mtc_post_machine::cli::execute;
use mtc_post_machine::compiler::{CompileOptions, VariantColumns, compile};
use mtc_post_machine::optimizer::OptLevel;

// --- Helpers -------------------------------------------------------------

fn args(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| s.to_string()).collect()
}

fn scratch(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    // CARGO_TARGET_TMPDIR outlives a single `cargo test`, so a stale
    // artifact could satisfy an assertion after the code that writes it
    // has broken. Start clean.
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// Each fixture carries the entry's declaration as `{V}main()`, so the
/// same text builds a plain and a gated program with nothing else moved.
fn source(fixture: &str, volatile: bool) -> String {
    fixture.replace("{V}", if volatile { "volatile " } else { "" })
}

/// Compile both columns (the on-disk default) and link against the
/// embedded stdlib — the full path a `.pmo` takes to an image, and the
/// same library set `pmt link` supplies. Unreached `std::` routines are
/// dropped by the reachability pass, so offering the library costs the
/// fixtures that ignore it nothing.
fn build(src: &str, level: OptLevel) -> LinkOutput {
    let out = compile(
        src,
        CompileOptions {
            opt_level: level,
            ..Default::default()
        },
    )
    .expect("compiles");
    link_object(out.object)
}

/// Link one object against the embedded stdlib — the library set
/// `pmt link` supplies.
fn link_object(object: ObjectFile) -> LinkOutput {
    link(
        &[object],
        std::slice::from_ref(mtc_post_machine::stdlib::object()),
        LinkOptions::default(),
    )
    .expect("links")
}

/// What a program can observe about a run: how it ended, and the tape it
/// left behind. Step and tact counts are deliberately NOT part of this —
/// they are allowed to differ between columns.
#[derive(Debug, PartialEq, Eq)]
struct Observables {
    outcome: Outcome,
    marked: Vec<i64>,
    head: i64,
}

fn run_image(exe: &Executable, cells: &[bool], head: i64) -> (Observables, u64) {
    let mut registry = ArchRegistry::new();
    registry.register(Box::new(Pm1));
    let machine = Machine::from_executable(exe, &registry).expect("loads");
    let mut tape = InfiniteTape::from_cells(cells.iter().copied(), 0, head);
    let options = RunOptions {
        limits: RunLimits {
            max_steps: Some(100_000),
            ..Default::default()
        },
        ..Default::default()
    };
    let result = machine.run(&mut tape, options);
    (
        Observables {
            outcome: result.outcome,
            marked: tape.marked_cells(),
            head: tape.head(),
        },
        result.stats.total_tacts(),
    )
}

fn read_object(path: &PathBuf) -> ObjectFile {
    ObjectFile::from_bytes(&fs::read(path).expect("the object was written")).expect("reads back")
}

/// Every column a name ships in an object, in symbol order.
fn columns_of(obj: &ObjectFile, name: &str) -> Vec<BlobVariant> {
    let variants = obj.variants.as_deref().unwrap_or_default();
    obj.symbols
        .iter()
        .filter(|s| s.name == name)
        .filter_map(|s| match s.def {
            SymbolDef::Defined { blob } | SymbolDef::Local { blob } => {
                Some(variants[blob as usize])
            }
            SymbolDef::External => None,
        })
        .collect()
}

/// One linked function's bytes, sliced out of the image by its map entry.
fn function_bytes<'a>(out: &'a LinkOutput, name: &str) -> &'a [u8] {
    let f = out
        .map
        .functions
        .iter()
        .find(|f| f.name == name)
        .unwrap_or_else(|| panic!("`{name}` is not in the map"));
    &out.executable.code[f.start as usize..f.end as usize]
}

// --- The corpus ----------------------------------------------------------

/// Straight-line tape traffic and nothing else: every gated pass has
/// something to chew on here, so this is the fixture where the columns
/// diverge hardest.
const PULSE: &str = "\
{V}main() {
    mark;
    mark;
    right;
    unmark;
    mark;
    left;
    mark;
    unmark;
    mark;
    right;
    unmark;
}
";

/// A branch decided from a written value — the shape `branch-fold` folds
/// in the normal column and must not fold in the gated one.
const BRANCHING: &str = "\
{V}main() {
    mark;
    check(1, 2);
1:  right, unmark(!);
2:  left, mark;
}
";

/// A subroutine chain: `inner` is small enough to be spliced in the
/// normal column and stays a real call under the gate, so the two images
/// have different call structure as well as different tape traffic.
const CALL_CHAIN: &str = "\
{V}main() {
    @outer();
    mark;
}
outer() {
    @inner();
    right;
}
inner() {
    mark;
    right;
    unmark;
}
";

/// A loop bounded by tape content, not by a step budget: it clears cells
/// while walking right over a marked run, then writes one cell at the
/// end. Every tape below leaves it after at most four turns, so no run
/// here can end on the step limit — where a run does, step counts stop
/// being comparable and the whole tact pin below inverts. The clearing
/// write is what makes the body column-sensitive: a bare `right; check`
/// walk compiles to the same bytes in both columns and would prove
/// nothing.
const LOOP: &str = "\
{V}main() {
1:  unmark, right;
    check(1, 2);
2:  mark;
}
";

/// The one fixture that does not end in `stp`: termination KIND is part
/// of the observable contract, and `hlt` is the other kind.
const HALTING: &str = "\
{V}main() {
    mark;
    right;
    @stopper();
}
stopper() {
    halt;
}
";

/// The only fixture that reaches outside its own object. Three of the
/// embedded stdlib's eleven routines ship two genuinely different columns
/// (`eraseSection`, `removeFirstMark`, `removeLastMark`); the rest touch
/// no tape and dedup to one `Both` blob. A gated program calling one of
/// the three must resolve a LIBRARY name to its gated body — a different
/// resolution path from an intra-object call, which binds a blob
/// directly. `removeFirstMark` walks left off the marked run and steps
/// back onto its first cell, so it terminates on every tape below.
const STDLIB: &str = "\
use std::removeFirstMark;

{V}main() {
    mark;
    right;
    @removeFirstMark();
}
";

const CORPUS: &[(&str, &str)] = &[
    ("pulse", PULSE),
    ("branching", BRANCHING),
    ("call-chain", CALL_CHAIN),
    ("loop", LOOP),
    ("halting", HALTING),
    ("stdlib", STDLIB),
];

/// The `opt_equivalence` tape set: blank, marked, runs, and an off-zero
/// head.
const TAPES: &[(&[bool], i64)] = &[
    (&[false], 0),
    (&[true], 0),
    (&[true, true, true], 0),
    (&[false, true, true], 0),
    (&[true, false, true], 1),
];

// --- Equivalence ---------------------------------------------------------

/// The round's central claim: gating the optimizer changes what a program
/// COSTS, never what it computes. Both columns of every corpus program
/// are linked from the same source and run on the same tapes; the
/// outcome, the marked cells and the final head must match, while the
/// gated column is allowed — and on a fusing program required — to spend
/// more tacts getting there.
#[test]
fn normal_and_volatile_columns_agree_on_observables() {
    let mut costlier: Vec<&str> = Vec::new();
    for (name, fixture) in CORPUS {
        let normal = build(&source(fixture, false), OptLevel::O1);
        let volatile = build(&source(fixture, true), OptLevel::O1);
        // The column choice is the linker's, taken from the entry
        // owner's program bit — assert it directly rather than inferring
        // it from behaviour, so a swapped column fails here first.
        assert!(
            !normal.report.program_volatile,
            "{name}: a plain `main` is not a volatile program"
        );
        assert!(
            volatile.report.program_volatile,
            "{name}: `volatile main` must select the gated column"
        );
        assert!(
            normal.report.variant_fallbacks.is_empty()
                && volatile.report.variant_fallbacks.is_empty(),
            "{name}: a compiled object offers every reached name in both columns"
        );
        // Every fixture must actually exercise the gate. A program whose
        // columns compile to the same bytes agrees with itself for free,
        // and would quietly stop testing anything.
        assert_ne!(
            normal.executable.code, volatile.executable.code,
            "{name}: both columns emit the same image — the fixture has stopped discriminating"
        );

        for (cells, head) in TAPES {
            let (normal_run, normal_tacts) = run_image(&normal.executable, cells, *head);
            let (volatile_run, volatile_tacts) = run_image(&volatile.executable, cells, *head);
            for (column, run) in [("normal", &normal_run), ("volatile", &volatile_run)] {
                assert_ne!(
                    run.outcome,
                    Outcome::Trapped(Trap::StepLimit),
                    "{name} on {cells:?}/{head}: the {column} column must terminate, \
                     or its tact count is a budget and not a cost"
                );
            }
            assert_eq!(
                normal_run, volatile_run,
                "{name} on tape {cells:?}/{head}: the columns disagree on an observable"
            );
            assert!(
                volatile_tacts >= normal_tacts,
                "{name} on tape {cells:?}/{head}: the gated column cannot be cheaper \
                 than the optimized one ({volatile_tacts} < {normal_tacts})"
            );
            if volatile_tacts > normal_tacts {
                costlier.push(name);
            }
        }
    }
    // The weak inequality alone is satisfied by two identical images, so
    // pin the direction where it has to bite: the straight-line fixture's
    // transactions are exactly what the gate preserves.
    assert!(
        costlier.contains(&"pulse"),
        "no fixture paid for its transactions — are both links picking the same column? \
         costlier: {costlier:?}"
    );
}

/// The measured cost of the round's worked example: one empty tape, two
/// columns of one source, identical observables — and the gated column
/// paying 55 tacts where the optimized one pays 26. Pinned through the
/// real column-selection path (keyword → program bit → linker), a
/// different route to the same two images than the pass-level measurement
/// behind `gated_passes.rs`, so the figures are a property of the program
/// rather than of one way of building it.
#[test]
fn the_pulse_example_costs_what_the_page_reports() {
    let normal = build(&source(PULSE, false), OptLevel::O1);
    let volatile = build(&source(PULSE, true), OptLevel::O1);
    let (normal_run, normal_tacts) = run_image(&normal.executable, &[false], 0);
    let (volatile_run, volatile_tacts) = run_image(&volatile.executable, &[false], 0);
    assert_eq!(
        (
            normal_run.outcome,
            normal_run.marked.as_slice(),
            normal_run.head
        ),
        (Outcome::Stopped, &[0i64][..], 1)
    );
    assert_eq!(normal_run, volatile_run);
    // Derivation, so a move here is re-derived and not regenerated. The
    // normal column fuses the source's eleven commands into four
    // instructions (`wrr 1 / wrl 1 / wrr 1 / wr 0`) plus `stp`: 5 steps,
    // 15 core + 11 stall = 26. The gated column keeps every transaction —
    // eleven instructions plus `stp`: 12 steps, 33 core + 22 stall = 55.
    assert_eq!(
        (normal_tacts, volatile_tacts),
        (26, 55),
        "the pulse's published cost moved"
    );
}

/// The gate's honest end-to-end consequence: a dropped write is a
/// transaction that never reaches the device. On a strict cell — which
/// faults when a write does not change the cell — the optimized column
/// completes precisely BECAUSE the second `mark` was folded away, and the
/// gated column performs it and traps. Unlike `gated_passes.rs`'s
/// pass-level twin, this one goes through `volatile main` and `pmt run`,
/// by BOTH routes to an image: the driver's in-memory build, which picks
/// the column before the linker ever sees it, and a two-column `.pmo` on
/// disk, where the linker's own choice is what decides. Either route
/// alone would leave half the chain — keyword → program bit → column →
/// exit code — unpinned.
#[test]
fn volatile_keeps_the_strict_cell_fault() {
    const BODY: &str = "{V}main() {\n    mark;\n    mark;\n}\n";
    let dir = scratch("volatile_strict_cells");
    for (kind, volatile, code, outcome) in [
        ("normal", false, 0u8, "outcome: Stopped"),
        ("volatile", true, 3, "StrictCellViolation"),
    ] {
        let src = dir.join(format!("{kind}.pmc"));
        fs::write(&src, source(BODY, volatile)).unwrap();

        // Route 1: the in-memory driver — one column, never on disk.
        let built = dir.join(format!("{kind}-built.pmx"));
        execute(&args(&[
            "build",
            "-O1",
            src.to_str().unwrap(),
            "-o",
            built.to_str().unwrap(),
        ]))
        .unwrap_or_else(|e| panic!("{kind}: build failed: {e}"));

        // Route 2: a two-column object the linker then selects from.
        let object = dir.join(format!("{kind}.pmo"));
        let linked = dir.join(format!("{kind}-linked.pmx"));
        execute(&args(&[
            "compile",
            "-O1",
            src.to_str().unwrap(),
            "-o",
            object.to_str().unwrap(),
        ]))
        .unwrap_or_else(|e| panic!("{kind}: compile failed: {e}"));
        assert_eq!(
            columns_of(&read_object(&object), "main"),
            vec![BlobVariant::Normal, BlobVariant::Volatile],
            "{kind}: the object must offer the linker a real choice"
        );
        execute(&args(&[
            "link",
            object.to_str().unwrap(),
            "-o",
            linked.to_str().unwrap(),
        ]))
        .unwrap_or_else(|e| panic!("{kind}: link failed: {e}"));

        for (route, image) in [("built", &built), ("linked", &linked)] {
            let out = execute(&args(&["run", "--strict-cells", image.to_str().unwrap()]))
                .unwrap_or_else(|e| panic!("{kind}/{route}: run failed: {e}"));
            assert_eq!(out.code, code, "{kind}/{route}: exit code\n{}", out.stdout);
            assert!(
                out.stdout.contains(outcome),
                "{kind}/{route}: wanted `{outcome}` in:\n{}",
                out.stdout
            );
        }
    }
}

// --- One image, every path -----------------------------------------------

/// The driver compiles only the column the program needs, while anything
/// that lands on disk carries both — so a build mixing a pre-compiled
/// two-column `.pmo` with an in-memory source runs BOTH rules at once.
/// The image it produces must equal the one built entirely from disk
/// objects, byte for byte, sidecar included.
#[test]
fn in_memory_and_on_disk_paths_agree() {
    let dir = scratch("volatile_mixed_paths");
    let app = dir.join("app.pmc");
    let util = dir.join("util.pmc");
    // Both routines fuse write+move in the normal column and keep two
    // transactions in the gated one, so the columns genuinely differ.
    fs::write(
        &app,
        "volatile main() {\n    mark;\n    right;\n    @util();\n}\n",
    )
    .unwrap();
    fs::write(&util, "export util() {\n    mark;\n    right;\n}\n").unwrap();

    let util_object = dir.join("util.pmo");
    execute(&args(&[
        "compile",
        "-O1",
        util.to_str().unwrap(),
        "-o",
        util_object.to_str().unwrap(),
    ]))
    .unwrap();
    assert_eq!(
        columns_of(&read_object(&util_object), "util"),
        vec![BlobVariant::Normal, BlobVariant::Volatile],
        "the disk object must carry both columns, or the mixed build proves nothing"
    );

    let mem = dir.join("mem.pmx");
    let built = execute(&args(&[
        "build",
        "-O1",
        "-v",
        app.to_str().unwrap(),
        util_object.to_str().unwrap(),
        "-o",
        mem.to_str().unwrap(),
    ]))
    .unwrap();
    assert!(
        !built.stderr.contains("column"),
        "every reached name offers the gated column:\n{}",
        built.stderr
    );

    let app_object = dir.join("app.pmo");
    execute(&args(&[
        "compile",
        "-O1",
        app.to_str().unwrap(),
        "-o",
        app_object.to_str().unwrap(),
    ]))
    .unwrap();
    let disk = dir.join("disk.pmx");
    execute(&args(&[
        "link",
        app_object.to_str().unwrap(),
        util_object.to_str().unwrap(),
        "-o",
        disk.to_str().unwrap(),
    ]))
    .unwrap();

    assert_eq!(
        fs::read(&mem).unwrap(),
        fs::read(&disk).unwrap(),
        "the in-memory column rule must not change the image"
    );
    assert_eq!(
        fs::read_to_string(dir.join("mem.pmx.map")).unwrap(),
        fs::read_to_string(dir.join("disk.pmx.map")).unwrap(),
        "nor the debug sidecar"
    );

    // And the agreed image really is the gated one: the same build with a
    // plain entry selects the other column and comes out different.
    let plain = dir.join("plain.pmc");
    fs::write(&plain, "main() {\n    mark;\n    right;\n    @util();\n}\n").unwrap();
    let normal = dir.join("normal.pmx");
    execute(&args(&[
        "build",
        "-O1",
        plain.to_str().unwrap(),
        util_object.to_str().unwrap(),
        "-o",
        normal.to_str().unwrap(),
    ]))
    .unwrap();
    assert_ne!(
        fs::read(&mem).unwrap(),
        fs::read(&normal).unwrap(),
        "a volatile program and a plain one over the same object must not agree"
    );
}

/// A `.pmo` from before the columns existed carries no variant records at
/// all. A volatile program may still link it — the linker takes the one
/// column there is and COUNTS the borrow, so the mixed edge is reported
/// rather than hidden. The image runs; the fallback is named once.
#[test]
fn legacy_object_end_to_end() {
    let dir = scratch("volatile_legacy_object");
    let legacy = dir.join("util.pma");
    fs::write(
        &legacy,
        ".func util\n        wr      1\n        rgt\n        ret\n",
    )
    .unwrap();
    let legacy_object = dir.join("util.pmo");
    execute(&args(&[
        "asm",
        legacy.to_str().unwrap(),
        "-o",
        legacy_object.to_str().unwrap(),
    ]))
    .unwrap();
    assert_eq!(
        read_object(&legacy_object).variants,
        None,
        "a directive-free file is exactly what a pre-volatile toolchain emitted"
    );

    let app = dir.join("app.pmc");
    fs::write(&app, "volatile main() {\n    @util();\n}\n").unwrap();
    let image = dir.join("app.pmx");
    let built = execute(&args(&[
        "build",
        "-O1",
        "-v",
        app.to_str().unwrap(),
        legacy_object.to_str().unwrap(),
        "-o",
        image.to_str().unwrap(),
    ]))
    .unwrap();
    assert_eq!(built.code, 0, "{}", built.stderr);
    assert!(
        built
            .stderr
            .contains("link: 1 name(s) with no volatile column linked normal [util]"),
        "the borrowed column must be counted and named:\n{}",
        built.stderr
    );

    // `util` writes the cell under the head and steps right; `main` then
    // stops. Derived, not read back off the run.
    let exe = Executable::from_bytes(&fs::read(&image).unwrap()).expect("the image loads");
    let (run, _) = run_image(&exe, &[false], 0);
    assert_eq!(
        run,
        Observables {
            outcome: Outcome::Stopped,
            marked: vec![0],
            head: 1,
        }
    );
}

/// Hand-written assembly can offer both columns: a same-name pair, one bare
/// and one `.volatile` (docs/pmt/asm.md (the .volatile directive)). A gated
/// program then links the tagged bodies with nothing borrowed — and the
/// choice is visible in the image, not merely in the report. `main` is
/// written as an identical pair, which the assembler would dedup to one
/// `Both` blob if `util` did not split it: the demotion is what keeps the
/// entry itself from pinning a column.
#[test]
fn handwritten_volatile_column_links_without_fallback() {
    const BODY: &str = "\
.func main
        call    util
        stp
.func main
.volatile
        call    util
        stp
.func util
        wrr     1
        ret
.func util
.volatile
        wr      1
        rgt
        ret
";
    let normal = link(
        &[assemble(BODY, false).expect("assembles")],
        &[],
        LinkOptions::default(),
    )
    .expect("links");
    let volatile = link(
        &[assemble(&format!(".volatile\n{BODY}"), false).expect("assembles")],
        &[],
        LinkOptions::default(),
    )
    .expect("links");

    assert!(!normal.report.program_volatile);
    assert!(volatile.report.program_volatile);
    for (kind, out) in [("normal", &normal), ("volatile", &volatile)] {
        assert!(
            out.report.variant_fallbacks.is_empty(),
            "{kind}: every name ships both columns — nothing may be borrowed, got {:?}",
            out.report.variant_fallbacks
        );
    }
    // The bodies differ by exactly the fusion the gate exists to prevent.
    assert_eq!(function_bytes(&normal, "util"), [ENT, WRR, 0x81, RET]);
    assert_eq!(function_bytes(&volatile, "util"), [ENT, WR, 0x81, RGT, RET]);
}

/// The round's text-expressibility gate, end to end through the tools: a
/// merged two-column object disassembles to `.pma` that assembles back to
/// the same bytes — variant tags and program bit included. Anything the
/// compiler can put in an object, an author can write by hand.
///
/// The two levels round-trip DIFFERENT shapes, and each asserts its own
/// before trusting the byte compare. At `-O1` `main` fuses `mark; right`
/// and `helper` does not touch the tape, so the object carries a split
/// `{Normal, Volatile}` pair AND a `Both` blob — the shape that exercises
/// the per-blob `.volatile` tag. At `-O0` nothing is withheld, so every
/// blob is `Both` and only the program bit and the print-twice path are
/// under test. Pinning both keeps the split leg from decaying to green:
/// were `main`'s columns to coincide at `-O1`, the bytes would still
/// round-trip while proving nothing about tags.
#[test]
fn dis_roundtrips_a_two_column_object() {
    const SRC: &str = "\
helper() {
    halt;
}

volatile main() {
    mark;
    right;
    @helper();
}
";
    let dir = scratch("volatile_dis_roundtrip");
    let src = dir.join("app.pmc");
    fs::write(&src, SRC).unwrap();
    for (level, main_columns) in [
        ("-O0", &[BlobVariant::Both][..]),
        ("-O1", &[BlobVariant::Normal, BlobVariant::Volatile][..]),
    ] {
        let object = dir.join(format!("app{level}.pmo"));
        execute(&args(&[
            "compile",
            level,
            src.to_str().unwrap(),
            "-o",
            object.to_str().unwrap(),
        ]))
        .unwrap();
        let compiled = read_object(&object);
        assert!(
            compiled.program_volatile,
            "{level}: the compiled object carries the program bit"
        );
        assert_eq!(
            columns_of(&compiled, "main"),
            main_columns,
            "{level}: the fixture no longer round-trips the shape this leg is for"
        );
        assert_eq!(
            columns_of(&compiled, "helper"),
            vec![BlobVariant::Both],
            "{level}: `helper` touches no tape, so its columns dedup at either level"
        );

        let text = execute(&args(&["dis", object.to_str().unwrap()])).unwrap();
        assert!(
            text.stdout.starts_with(".volatile\n"),
            "{level}: the program bit leads the dump:\n{}",
            text.stdout
        );
        let listing = dir.join(format!("app{level}.pma"));
        fs::write(&listing, &text.stdout).unwrap();
        let back = dir.join(format!("back{level}.pmo"));
        execute(&args(&[
            "asm",
            listing.to_str().unwrap(),
            "-o",
            back.to_str().unwrap(),
        ]))
        .unwrap_or_else(|e| panic!("{level}: the dump must assemble: {e}\n{}", text.stdout));
        assert_eq!(
            fs::read(&back).unwrap(),
            fs::read(&object).unwrap(),
            "{level}: dis -> asm diverged\n{}",
            text.stdout
        );
    }
}

/// The embedded stdlib's column roster, in full. The `stdlib` corpus
/// fixture proves that the ONE routine it calls links its gated body;
/// this pins which routines have a gated body to link at all. Three of
/// the eleven touch the tape in a way the gate withholds work from and
/// ship two columns; the other eight dedup to one `Both` blob. The
/// roster is a published fact about the library, so a stdlib edit that
/// splits a ninth routine — or fuses one of the three — has to come
/// through here rather than silently dating the documentation.
#[test]
fn the_stdlib_ships_exactly_three_split_routines() {
    const SPLIT: [&str; 3] = [
        "std::eraseSection",
        "std::removeFirstMark",
        "std::removeLastMark",
    ];
    const DEDUPED: [&str; 8] = [
        "std::appendMark",
        "std::goToBegin",
        "std::goToBlankLeft",
        "std::goToBlankRight",
        "std::goToEnd",
        "std::goToMarkLeft",
        "std::goToMarkRight",
        "std::prependMark",
    ];
    let object = mtc_post_machine::stdlib::object();
    let mut names: Vec<&str> = object.symbols.iter().map(|s| s.name.as_str()).collect();
    names.sort_unstable();
    names.dedup();

    let mut split: Vec<&str> = Vec::new();
    let mut deduped: Vec<&str> = Vec::new();
    for name in names {
        match columns_of(object, name).as_slice() {
            [BlobVariant::Both] => deduped.push(name),
            [BlobVariant::Normal, BlobVariant::Volatile] => split.push(name),
            other => panic!("`{name}` carries an unexpected column shape: {other:?}"),
        }
    }
    assert_eq!(split, SPLIT, "the split routines moved");
    assert_eq!(deduped, DEDUPED, "the column-invariant routines moved");
}

/// The `-O0` bit-identity floor, extended to the volatile world: with the
/// optimizer off there is nothing for the gate to withhold, so every
/// function the compiler emits dedups to one `Both` blob and all three
/// routes to an image — gated program, plain program, and the
/// single-column pipeline that predates the columns — emit the same
/// bytes.
///
/// The floor is a property of the COMPILER'S OUTPUT for the unit it
/// compiles, not of the whole image. The embedded stdlib is compiled once
/// at `-O1` whatever level the user asks for, so its split routines carry
/// two real columns even into a `-O0` link, and a gated program links the
/// gated body there. The `stdlib` fixture pins that difference rather
/// than being excused from the test: `-O0` narrows what the compiler
/// withholds, it does not switch the linker's column selection off.
#[test]
fn o0_matrix() {
    for (name, fixture) in CORPUS {
        let volatile = compile(
            &source(fixture, true),
            CompileOptions {
                opt_level: OptLevel::O0,
                ..Default::default()
            },
        )
        .expect("compiles");
        let normal = compile(
            &source(fixture, false),
            CompileOptions {
                opt_level: OptLevel::O0,
                ..Default::default()
            },
        )
        .expect("compiles");
        let single_column = compile(
            &source(fixture, false),
            CompileOptions {
                opt_level: OptLevel::O0,
                columns: VariantColumns::NormalOnly,
                ..Default::default()
            },
        )
        .expect("compiles");

        // WHY the compiler's own columns coincide at -O0: they are
        // identical by construction, so nothing is left to choose between.
        for (kind, out) in [("volatile", &volatile), ("normal", &normal)] {
            assert!(
                out.object
                    .variants
                    .as_deref()
                    .expect("a compiled object is tagged")
                    .iter()
                    .all(|v| *v == BlobVariant::Both),
                "{name} ({kind}): -O0 columns must dedup to Both, got {:?}",
                out.object.variants
            );
        }
        assert!(volatile.object.program_volatile);

        let images: Vec<Vec<u8>> = [volatile, normal, single_column]
            .into_iter()
            .map(|out| link_object(out.object).executable.to_bytes())
            .collect();
        if *name == "stdlib" {
            assert_ne!(
                images[0], images[1],
                "the linked library's columns are real at -O0 too — a gated program \
                 must still take the gated stdlib body"
            );
        } else {
            assert_eq!(
                images[0], images[1],
                "{name}: with nothing but -O0 code in the link, the image cannot \
                 depend on the program bit"
            );
        }
        assert_eq!(
            images[1], images[2],
            "{name}: -O0 must stay byte-identical to the single-column build"
        );
    }
}
