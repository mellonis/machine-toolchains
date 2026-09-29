//! `tmt interface --check FILE INPUT` (docs/tmt/cli.md (tmt interface)):
//! a CI/pre-commit guard comparing a committed header, byte for byte,
//! against what `tmt interface INPUT` renders today, without writing
//! anything. Helpers are local to this file, per the crate's
//! no-shared-test-support convention.

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use mtc_turing_machine::cli::execute;

fn args(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| s.to_string()).collect()
}

/// A private scratch directory, named uniquely by process id plus a
/// per-call counter so two tests running in parallel — in the same
/// process or in two — never share a path, and removed on drop so the
/// tree does not accumulate across runs.
struct Scratch {
    dir: PathBuf,
}

impl Scratch {
    fn new(name: &str) -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
            .join(format!("interface-check-{name}-{}-{n}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("scratch dir");
        Scratch { dir }
    }

    fn path(&self, name: &str) -> PathBuf {
        self.dir.join(name)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

/// Two exported alphabets and nothing else reachable from `machine` —
/// enough for a two-line canonical header, so a fixture can mutate the
/// SECOND line and prove the reported line number is not merely "line
/// 1" by coincidence.
const TWO_ALPHABETS_TMC: &str = "\
export alphabet ab { '_', 'a' }
export alphabet cd { '_', 'c' }
machine {
  tape t: ab;
  entry state s { [*] -> stop; }
}
";

/// An exported routine over a PRIVATE alphabet: the source arm prints the
/// alphabet by its own name (`ab`), the object arm — which never sees a
/// private declaration's own name — synthesizes one (`util__t`), so the
/// two arms' headers genuinely disagree (docs/tmt/cli.md (tmt
/// interface)). Used to prove `--check` renders the SAME arm `tmt
/// interface INPUT` would, not always the source arm.
const PRIVATE_ALPHABET_ROUTINE_TMC: &str = "\
alphabet ab { '_', 'a' }
export routine util(tape t: ab) {
  entry state s { [*] -> return; }
}
";

/// Runs `tmt interface INPUT -o OUT` and panics on failure — the fixture
/// setup step every test below shares, not itself part of what any test
/// verifies.
fn write_header(input: &std::path::Path, out: &std::path::Path) {
    let result = execute(&args(&[
        "interface",
        input.to_str().unwrap(),
        "-o",
        out.to_str().unwrap(),
    ]))
    .unwrap_or_else(|e| panic!("tmt interface -o should succeed: {e}"));
    assert_eq!(result.code, 0, "stderr: {}", result.stderr);
}

// ── 1: a fresh header matches ───────────────────────────────────────────

/// Mutation this fixture kills: the comparison inverted (`--check`
/// reporting a match as a mismatch, or vice versa).
#[test]
fn check_matches_a_freshly_written_header() {
    let scratch = Scratch::new("fresh");
    let src = scratch.path("unit.tmc");
    fs::write(&src, TWO_ALPHABETS_TMC).unwrap();
    let header = scratch.path("unit.tmh");
    write_header(&src, &header);

    let result = execute(&args(&[
        "interface",
        src.to_str().unwrap(),
        "--check",
        header.to_str().unwrap(),
    ]))
    .unwrap_or_else(|e| panic!("--check on a matching header should still succeed: {e}"));

    assert_eq!(result.code, 0);
    assert!(result.stdout.is_empty(), "stdout: {}", result.stdout);
    assert!(result.stderr.is_empty(), "stderr: {}", result.stderr);
}

// ── 2: a stale header names the first differing line ───────────────────

/// Mutation this fixture kills: the line number computed from the wrong
/// side of the comparison, or off by one. The extra line is spliced in
/// AFTER the header's real first line, so the expected first-difference
/// line is 2, not the boundary value 1 an off-by-one bug could satisfy
/// by accident.
#[test]
fn check_reports_the_first_differing_line_on_a_stale_header() {
    let scratch = Scratch::new("stale");
    let src = scratch.path("unit.tmc");
    fs::write(&src, TWO_ALPHABETS_TMC).unwrap();
    let header = scratch.path("unit.tmh");
    write_header(&src, &header);

    let fresh = fs::read_to_string(&header).unwrap();
    let mut lines: Vec<&str> = fresh.lines().collect();
    assert!(
        lines.len() >= 2,
        "fixture header should carry both exported alphabets: {fresh:?}"
    );
    lines.insert(1, "export alphabet stale { '_', 'z' }");
    // `Vec::join` carries no trailing newline — the fixture's second
    // required mutation (docs/tmt/cli.md (tmt interface): a missing
    // trailing newline is itself a reportable difference), not exercised
    // by ITS OWN line number here since the inserted line is found first.
    let stale = lines.join("\n");
    assert!(!stale.ends_with('\n'));
    fs::write(&header, &stale).unwrap();

    let result = execute(&args(&[
        "interface",
        src.to_str().unwrap(),
        "--check",
        header.to_str().unwrap(),
    ]))
    .unwrap_or_else(|e| panic!("a mismatch is reported, not a usage error: {e}"));

    assert_eq!(result.code, 1);
    assert!(result.stdout.is_empty(), "stdout: {}", result.stdout);
    let expected = format!(
        "{}: differs from the header of {} (first difference at line 2)\n",
        header.display(),
        src.display()
    );
    assert_eq!(result.stderr, expected);
}

// ── 3: a missing FILE is a distinct failure ─────────────────────────────

/// Mutation this fixture kills: an unreadable FILE folded into the
/// "differs" report instead of being reported as its own failure.
#[test]
fn check_fails_when_file_is_missing() {
    let scratch = Scratch::new("missing");
    let src = scratch.path("unit.tmc");
    fs::write(&src, TWO_ALPHABETS_TMC).unwrap();
    let missing = scratch.path("nonexistent.tmh");

    let result = execute(&args(&[
        "interface",
        src.to_str().unwrap(),
        "--check",
        missing.to_str().unwrap(),
    ]));

    let Err(message) = result else {
        panic!("a missing FILE should be a usage-level error, got {result:?}");
    };
    assert!(
        message.starts_with(&format!("{}: cannot read: ", missing.display())),
        "message: {message}"
    );
}

// ── 4: --check and -o refuse each other by name ─────────────────────────

/// Mutation this fixture kills: `-o`/`--check` silently accepted together
/// (one winning over the other) instead of refused.
#[test]
fn check_and_dash_o_are_mutually_exclusive() {
    let scratch = Scratch::new("exclusive");
    let src = scratch.path("unit.tmc");
    fs::write(&src, TWO_ALPHABETS_TMC).unwrap();
    let out = scratch.path("out.tmh");

    let result = execute(&args(&[
        "interface",
        src.to_str().unwrap(),
        "-o",
        out.to_str().unwrap(),
        "--check",
        out.to_str().unwrap(),
    ]));

    let Err(message) = result else {
        panic!("-o and --check together should be a usage error, got {result:?}");
    };
    assert!(message.contains("-o"), "message: {message}");
    assert!(message.contains("--check"), "message: {message}");
    assert!(!out.exists(), "-o must not have written anything");
}

// ── 5: --check follows the same arm as a bare render ────────────────────

/// Mutation this fixture kills: `--check` always rendering the source
/// arm (or always the object arm) instead of the arm `sniff` actually
/// selects for INPUT. `routine_obj.tmh` (the OBJECT arm's own render) is
/// checked against the object and must match; `routine_src.tmh` (the
/// SOURCE arm's render of the identical unit) is checked against the
/// SAME object and must NOT match, since a private alphabet's name
/// (`ab` vs the object arm's synthesized `util__t`) makes the two arms'
/// headers genuinely disagree.
#[test]
fn check_follows_the_same_arm_selection_as_a_bare_render() {
    let scratch = Scratch::new("object-arm");
    let src = scratch.path("routine.tmc");
    fs::write(&src, PRIVATE_ALPHABET_ROUTINE_TMC).unwrap();
    let object = scratch.path("routine.tmo");
    let compiled = execute(&args(&[
        "compile",
        src.to_str().unwrap(),
        "-o",
        object.to_str().unwrap(),
    ]))
    .unwrap_or_else(|e| panic!("tmt compile should succeed: {e}"));
    assert_eq!(compiled.code, 0, "stderr: {}", compiled.stderr);

    let object_header = scratch.path("routine_obj.tmh");
    write_header(&object, &object_header);
    let source_header = scratch.path("routine_src.tmh");
    write_header(&src, &source_header);
    assert_ne!(
        fs::read_to_string(&object_header).unwrap(),
        fs::read_to_string(&source_header).unwrap(),
        "the fixture needs the two arms to disagree for this test to prove anything"
    );

    let matches_own_arm = execute(&args(&[
        "interface",
        object.to_str().unwrap(),
        "--check",
        object_header.to_str().unwrap(),
    ]))
    .unwrap_or_else(|e| panic!("--check on the object's own header should succeed: {e}"));
    assert_eq!(
        matches_own_arm.code, 0,
        "stderr: {}",
        matches_own_arm.stderr
    );

    let rejects_other_arm = execute(&args(&[
        "interface",
        object.to_str().unwrap(),
        "--check",
        source_header.to_str().unwrap(),
    ]))
    .unwrap_or_else(|e| panic!("a mismatch is reported, not a usage error: {e}"));
    assert_eq!(
        rejects_other_arm.code, 1,
        "checking the object against the SOURCE arm's header should differ"
    );
}

// ── 6: -v lists the differing lines ─────────────────────────────────────

/// Mutation this fixture kills: `-v` printing nothing, or not in the
/// `-`/`+` shape. The fixture changes exactly one line's content (not an
/// insertion), so the listing carries exactly one `-` line and one `+`
/// line — a naive positional diff over a one-line edit has nothing else
/// to report.
#[test]
fn check_verbose_lists_the_differing_lines() {
    let scratch = Scratch::new("verbose");
    let src = scratch.path("unit.tmc");
    fs::write(&src, TWO_ALPHABETS_TMC).unwrap();
    let header = scratch.path("unit.tmh");
    write_header(&src, &header);

    let fresh = fs::read_to_string(&header).unwrap();
    let mutated = fresh.replace("cd", "zz");
    assert_ne!(fresh, mutated, "the fixture should actually change a line");
    fs::write(&header, &mutated).unwrap();

    let result = execute(&args(&[
        "interface",
        src.to_str().unwrap(),
        "--check",
        header.to_str().unwrap(),
        "-v",
    ]))
    .unwrap_or_else(|e| panic!("a mismatch is reported, not a usage error: {e}"));

    assert_eq!(result.code, 1);
    let minus_lines: Vec<&str> = result
        .stderr
        .lines()
        .filter(|l| l.starts_with('-'))
        .collect();
    let plus_lines: Vec<&str> = result
        .stderr
        .lines()
        .filter(|l| l.starts_with('+'))
        .collect();
    assert_eq!(minus_lines, vec!["-export alphabet cd { '_', 'c' }"]);
    assert_eq!(plus_lines, vec!["+export alphabet zz { '_', 'c' }"]);
}

// ── helpers for the line-ending and notice fixtures ─────────────────────

/// Writes `TWO_ALPHABETS_TMC` and its fresh header into `scratch`,
/// returning `(source path, header path, fresh header text)`.
fn fresh_header(scratch: &Scratch) -> (PathBuf, PathBuf, String) {
    let src = scratch.path("unit.tmc");
    fs::write(&src, TWO_ALPHABETS_TMC).unwrap();
    let header = scratch.path("unit.tmh");
    write_header(&src, &header);
    let fresh = fs::read_to_string(&header).unwrap();
    (src, header, fresh)
}

fn check(src: &std::path::Path, header: &std::path::Path) -> mtc_turing_machine::cli::CliOutput {
    execute(&args(&[
        "interface",
        src.to_str().unwrap(),
        "--check",
        header.to_str().unwrap(),
    ]))
    .unwrap_or_else(|e| panic!("--check should report, not fail: {e}"))
}

fn differs_at(
    result: &mtc_turing_machine::cli::CliOutput,
    header: &std::path::Path,
    src: &std::path::Path,
    line: usize,
) {
    assert_eq!(result.code, 1, "stderr: {}", result.stderr);
    assert_eq!(
        result.stderr,
        format!(
            "{}: differs from the header of {} (first difference at line {line})\n",
            header.display(),
            src.display()
        )
    );
}

// ── 7: a missing final newline is a difference ──────────────────────────

/// Mutation this fixture kills: `lines()` in place of `split_inclusive`
/// in the line comparison, which folds an unterminated last line into the
/// terminated one and reports no difference line.
#[test]
fn check_reports_a_missing_final_newline() {
    let scratch = Scratch::new("no-final-newline");
    let (src, header, fresh) = fresh_header(&scratch);
    let stripped = fresh.strip_suffix('\n').expect("header ends in a newline");
    fs::write(&header, stripped).unwrap();

    let result = check(&src, &header);
    differs_at(&result, &header, &src, fresh.lines().count());
}

// ── 8: CRLF is a difference ─────────────────────────────────────────────

/// Mutation this fixture kills: `lines()` in place of `split_inclusive`,
/// which strips `\r\n` and `\n` alike and so sees the two as equal lines.
#[test]
fn check_reports_crlf_line_endings() {
    let scratch = Scratch::new("crlf");
    let (src, header, fresh) = fresh_header(&scratch);
    fs::write(&header, fresh.replace('\n', "\r\n")).unwrap();

    let result = check(&src, &header);
    differs_at(&result, &header, &src, 1);
}

// ── 9: the shipped header checks clean ──────────────────────────────────

/// Mutation this fixture kills: the leading-notice skip removed, which
/// makes the shipped header (prefixed by its generated-file notice) differ
/// at line 1.
#[test]
fn check_accepts_the_shipped_std_header() {
    let stdlib = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/stdlib");
    let result = execute(&args(&[
        "interface",
        stdlib.join("std.tmc").to_str().unwrap(),
        "--check",
        stdlib.join("std.tmh").to_str().unwrap(),
    ]))
    .unwrap_or_else(|e| panic!("--check of the shipped header should succeed: {e}"));
    assert_eq!(result.code, 0, "stderr: {}", result.stderr);
    assert!(result.stderr.is_empty(), "stderr: {}", result.stderr);
}

const NOTICE: &str = "// GENERATED - do not edit by hand.\n// Regenerate with tmt interface.\n\n";

// ── 10: a leading notice is skipped ─────────────────────────────────────

/// Mutation this fixture kills: the leading-notice skip removed.
#[test]
fn check_skips_a_leading_notice() {
    let scratch = Scratch::new("notice");
    let (src, header, fresh) = fresh_header(&scratch);
    fs::write(&header, format!("{NOTICE}{fresh}")).unwrap();

    let result = check(&src, &header);
    assert_eq!(result.code, 0, "stderr: {}", result.stderr);
    assert!(result.stderr.is_empty(), "stderr: {}", result.stderr);
}

// ── 11: line numbers count the notice ───────────────────────────────────

/// Mutation this fixture kills: the reported line taken relative to the
/// text after the notice instead of the line in FILE. The notice is three
/// lines (two comments and a blank), the changed header line is its
/// second, so the answer is 5, not 2.
#[test]
fn check_line_numbers_count_the_notice() {
    let scratch = Scratch::new("notice-line");
    let (src, header, fresh) = fresh_header(&scratch);
    let mutated = fresh.replace("cd", "zz");
    assert_ne!(fresh, mutated);
    fs::write(&header, format!("{NOTICE}{mutated}")).unwrap();

    let result = check(&src, &header);
    differs_at(&result, &header, &src, 5);
}

// ── 12: a comment elsewhere is a difference ─────────────────────────────

/// Mutation this fixture kills: comment lines skipped everywhere rather
/// than only at the very top.
#[test]
fn check_rejects_a_comment_in_the_middle() {
    let scratch = Scratch::new("middle-comment");
    let (src, header, fresh) = fresh_header(&scratch);
    let mut lines: Vec<&str> = fresh.lines().collect();
    lines.insert(1, "// stray");
    fs::write(&header, format!("{}\n", lines.join("\n"))).unwrap();

    let result = check(&src, &header);
    differs_at(&result, &header, &src, 2);
}
