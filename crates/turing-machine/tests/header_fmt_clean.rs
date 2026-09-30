//! A generated header is canonical under `tmt fmt`: whatever `tmt
//! interface` prints — from a source or from a compiled object — passes
//! `tmt fmt --check` unchanged (docs/tmt/cli.md (interface)). The printer
//! gets there by running its assembled text through the formatter itself,
//! never by imitating the formatter's layout, so these tests drive the two
//! real CLI subcommands against each other over every shipped source.
//!
//! The graft digest is NOT the printed text: it is computed over the
//! header module's own canonical rendering of a graph's signature and body
//! (docs/formats.md (routine interfaces)), and reflowing the printed header
//! must never move it — a moved digest would turn every graft recorded
//! against an older object into a false drift finding. The second half of
//! this file pins the embedded stdlib's digests to their values from
//! before the printed header was made canonical, on both the exporting and
//! the grafting side.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use mtc_turing_machine::asm::assemble;
use mtc_turing_machine::cli::execute;
use mtc_turing_machine::compiler::{CompileOptions, Declarations, compile};
use mtc_turing_machine::stdlib;

fn args(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| s.to_string()).collect()
}

/// A fresh, per-call fixture directory under `CARGO_TARGET_TMPDIR`, named
/// uniquely by process id + an atomic counter (the repo's collision-free
/// temp-path rule for parallel test runs).
fn scratch(name: &str) -> ScratchDir {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("{name}-{}-{n}", std::process::id()));
    ScratchDir::fresh(dir)
}

/// A scratch directory this test owns. Its name can repeat across
/// processes — a later test process may reuse a pid, restarting the
/// counter — so `fresh` removes whatever an earlier run left under the
/// name before creating it, and `Drop` removes it again once the test
/// ends, passing or panicking.
struct ScratchDir(std::path::PathBuf);

impl ScratchDir {
    fn fresh(dir: std::path::PathBuf) -> Self {
        match std::fs::remove_dir_all(&dir) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => panic!("clearing stale scratch dir {}: {e}", dir.display()),
        }
        std::fs::create_dir_all(&dir).unwrap();
        ScratchDir(dir)
    }
}

impl std::ops::Deref for ScratchDir {
    type Target = std::path::Path;
    fn deref(&self) -> &std::path::Path {
        &self.0
    }
}

impl AsRef<std::path::Path> for ScratchDir {
    fn as_ref(&self) -> &std::path::Path {
        &self.0
    }
}

impl Drop for ScratchDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf()
}

/// Every shipped `.tmc`: the examples, the program goldens, and the
/// embedded stdlib source — the same corpus the repo's other sweeps use.
fn corpus() -> Vec<PathBuf> {
    let root = repo_root();
    let mut out = Vec::new();
    for dir in [
        root.join("docs/examples"),
        root.join("crates/turing-machine/tests/golden"),
        root.join("crates/turing-machine/src/stdlib"),
    ] {
        collect(&dir, &mut out);
    }
    out.sort();
    assert!(
        out.iter().any(|p| p.ends_with("std.tmc")),
        "the corpus lost the stdlib source: {out:?}"
    );
    out
}

fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            collect(&p, out);
        } else if p.extension().and_then(|s| s.to_str()) == Some("tmc") {
            out.push(p);
        }
    }
}

/// The stdlib source declares the `std` namespace itself, so it compiles
/// (and headers) without the embedded declarations of that same namespace.
fn nostdlib_for(src: &Path) -> Option<&'static str> {
    src.ends_with("std.tmc").then_some("--nostdlib")
}

/// `tmt fmt --check` over `header` (written as a `.tmh`), returning the
/// failure text or `None` when it is already canonical.
fn fmt_check(header: &Path) -> Option<String> {
    let out = execute(&args(&["fmt", "--check", header.to_str().unwrap()]))
        .unwrap_or_else(|e| panic!("fmt --check {}: {e}", header.display()));
    (out.code != 0).then(|| {
        let text = std::fs::read_to_string(header).unwrap();
        format!(
            "{} (exit {}):\n{}{}\n--- header ---\n{text}",
            header.display(),
            out.code,
            out.stdout,
            out.stderr
        )
    })
}

/// `tmt interface INPUT -o OUT`, asserting success.
fn interface_to(input: &Path, extra: Option<&str>, out: &Path) {
    let mut list = vec!["interface", input.to_str().unwrap()];
    list.extend(extra);
    list.extend(["-o", out.to_str().unwrap()]);
    let result = execute(&args(&list)).unwrap_or_else(|e| panic!("interface {input:?}: {e}"));
    assert_eq!(result.code, 0, "interface {input:?}: {}", result.stderr);
}

/// The source arm: `tmt interface` over every shipped `.tmc`, its output
/// written to a `.tmh`, then `tmt fmt --check` on that file — exit 0 for
/// every source. Mutation: a header printer that lays out its own text
/// (hard-coding the grid or the wrap width it believes `fmt` uses) instead
/// of handing it to the formatter — the moment the two disagree, as they
/// did before the printer ran the formatter (a graph's catch-all rule off
/// the state-block grid, a signature past the width limit on one line, an
/// exportless unit printing nothing where `fmt` prints one newline), this
/// goes red.
#[test]
fn every_shipped_source_header_is_fmt_clean() {
    let dir = scratch("header_fmt_clean_source");
    let mut failures = Vec::new();
    for (i, src) in corpus().iter().enumerate() {
        let header = dir.join(format!("h{i}.tmh"));
        interface_to(src, nostdlib_for(src), &header);
        failures.extend(fmt_check(&header).map(|f| format!("{}: {f}", src.display())));
    }
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}

/// The object arm: every shipped `.tmc` compiled to a `.tmo`, `tmt
/// interface` over the object, then `tmt fmt --check` on the printed
/// header. The object arm prints no graphs, so the grid never arises here,
/// but its signatures wrap at the same width limit (the stdlib's
/// four-glyph `writes` clauses cross it). Mutation: running the formatter
/// on the source arm only — the object arm's over-width signatures and its
/// empty exportless output then fail the check.
#[test]
fn every_shipped_object_header_is_fmt_clean() {
    let dir = scratch("header_fmt_clean_object");
    let mut failures = Vec::new();
    for (i, src) in corpus().iter().enumerate() {
        let object = dir.join(format!("o{i}.tmo"));
        let mut list = vec!["compile", src.to_str().unwrap()];
        list.extend(nostdlib_for(src));
        list.extend(["-o", object.to_str().unwrap()]);
        let result = execute(&args(&list)).unwrap_or_else(|e| panic!("compile {src:?}: {e}"));
        assert_eq!(result.code, 0, "compile {src:?}: {}", result.stderr);
        let header = dir.join(format!("o{i}.tmh"));
        interface_to(&object, None, &header);
        failures.extend(fmt_check(&header).map(|f| format!("{}: {f}", src.display())));
    }
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}

/// Every graph the embedded stdlib exports, with its graft digest. The
/// digest is the CRC-32 of the header module's own graph rendering, never
/// of the printed header, so reflowing the printed text must leave every
/// value here untouched. The rendering does cover the graph's signature,
/// so a declared clause is part of it: the values were re-taken when each
/// graph's tape parameter gained its `leaves` clause (docs/formats.md
/// (routine interfaces)), and a reflow still moves none of them.
const STD_GRAPH_DIGESTS: &[(&str, u32)] = &[
    ("std::binaryNumbers::goToNumberGraph", 2800156793),
    ("std::binaryNumbers::goToNumbersStartGraph", 553620967),
    ("std::binaryNumbers::goToNextNumberGraph", 4114293331),
    ("std::binaryNumbers::goToPreviousNumberGraph", 2573355874),
    ("std::binaryNumbers::deleteNumberGraph", 1750554584),
    ("std::binaryNumbers::normalizeNumberGraph", 2111348676),
    ("std::binaryNumbers::plusOneGraph", 1369402796),
    ("std::binaryNumbers::minusOneFastGraph", 2460668743),
    ("std::binaryNumbersBare::plusOneGraph", 81519859),
    ("std::binaryNumbersBare::minusOneGraph", 565278662),
    ("std::binaryNumbersBare::invertNumberGraph", 579301745),
    ("std::binaryNumbersBare::normalizeNumberGraph", 609241632),
];

/// The exporting side: compiling the stdlib source records exactly the
/// pinned digest for every exported graph, and no other graph. Mutation:
/// running the formatter inside the digest's own rendering (the tempting
/// wrong placement for making the printed header canonical) — every
/// digest whose graph body carries a rule off the grid, or whose signature
/// crosses the width limit, moves.
#[test]
fn the_stdlib_exporter_digests_are_unmoved() {
    let object = compile(
        stdlib::SOURCE,
        CompileOptions {
            externals: Declarations::none(),
            ..Default::default()
        },
    )
    .unwrap_or_else(|e| panic!("compile std.tmc: {e}"))
    .object;
    let got: Vec<(String, u32)> = object
        .interface
        .as_ref()
        .expect("the stdlib object carries an interface section")
        .graphs
        .iter()
        .map(|g| (g.name.clone(), g.digest))
        .collect();
    let want: Vec<(String, u32)> = STD_GRAPH_DIGESTS
        .iter()
        .map(|(n, d)| (n.to_string(), *d))
        .collect();
    assert_eq!(got, want);
}

/// The grafting side: a consumer that grafts each stdlib graph through the
/// embedded declarations — which are read from the committed, reflowed
/// `std.tmh` — records the same pinned digest for the body it spliced.
/// Mutation: a digest computed over the printed header text (or anything
/// else whitespace in the committed header reaches) — the reflowed
/// `std.tmh` then yields a digest that no longer matches the exporter's,
/// every graft of a stdlib graph a false drift finding.
#[test]
fn a_graft_through_the_committed_stdlib_header_records_the_unmoved_digest() {
    for (name, want) in STD_GRAPH_DIGESTS {
        let ns = name.rsplit_once("::").unwrap().0;
        let consumer = format!(
            "use {ns}::symbols;\n\
             machine {{\n\
             \x20 tape num: symbols;\n\
             \x20 entry graft {name}(num = num, done = fin) as g;\n\
             \x20 state fin {{ [*] -> stop; }}\n\
             }}\n"
        );
        let object = compile(&consumer, CompileOptions::default())
            .unwrap_or_else(|e| panic!("compile a consumer of {name}: {e}"))
            .object;
        let got = object
            .grafts
            .iter()
            .find(|g| g.graph == *name)
            .unwrap_or_else(|| panic!("no graft record for {name} in {:?}", object.grafts))
            .digest;
        assert_eq!(got, *want, "{name}");
    }
}

/// A hand-assembled object whose routine names its tape parameter after a
/// `.tmc` keyword. The `.tma` dialect accepts the name (a parameter name
/// is letters, digits and underscore, docs/formats.md (routine
/// interfaces)), so the object arm prints a signature the `.tmc` grammar
/// then refuses.
const KEYWORD_PARAM_TMA: &str = "\
.routine helper, tapes=1, alpha=(2)
.param state, ('_', '1')
.func helper
        rd
        ret
";

/// The one input class that reaches the formatter's error on renderer
/// output: the object arm prints a header whose tape parameter is named
/// `state`, and the formatter's parse refuses it. `tmt interface` must
/// fail with that refusal rather than print text no reader accepts.
/// Mutation: the printer falling back to the unformatted text when the
/// formatter errors (`unwrap_or` over `crate::fmt::format`) — the command
/// then succeeds with an unparseable header, and every corpus sweep above
/// stays green, since no shipped source names a parameter after a
/// keyword.
#[test]
fn a_header_the_formatter_refuses_is_an_interface_error() {
    let dir = scratch("header_fmt_refused");
    let object = assemble(KEYWORD_PARAM_TMA, false).expect("the `.tma` assembles");
    let path = dir.join("keyword.tmo");
    std::fs::write(&path, object.to_bytes()).unwrap();
    let out = dir.join("keyword.tmh");
    match execute(&args(&[
        "interface",
        path.to_str().unwrap(),
        "-o",
        out.to_str().unwrap(),
    ])) {
        Err(message) => assert!(message.contains("does not re-read"), "{message}"),
        Ok(o) => panic!(
            "got Ok: code={} stdout={} stderr={}",
            o.code, o.stdout, o.stderr
        ),
    }
}
