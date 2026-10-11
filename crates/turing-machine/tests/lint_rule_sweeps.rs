//! Six lint rules over every `.tmc` the repository ships: `unused-tape`,
//! `unused-tape-parameter`, `unused-fold-variable`, `inert-never-writes`,
//! `unused-alphabet` and `unused-map` must stay silent on the corpus (docs/tmt/lint.md (the `.tmc` rules)).
//!
//! REGRESSION guards, not evidence that the rules work — each rule's
//! behavioural fixtures live with the rule itself under
//! `crates/turing-machine/src/lint/rules/`. What these hold is the other
//! direction: a rule must not start reporting a construct the repository
//! already ships as used.
//!
//! The corpus roots mirror `tests/unused_set_sweep.rs`'s own.

use std::fs;
use std::path::{Path, PathBuf};

use mtc_turing_machine::lint::{LintOptions, lint};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf()
}

/// Every `.tmc` under the three roots that hold real programs — the golden
/// corpus, the shipped examples, and the embedded standard library's own
/// source.
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
    out
}

fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
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

fn offenders(code: &str) -> Vec<String> {
    let corpus = corpus();
    assert!(
        corpus.len() >= 5,
        "the corpus roots resolved to almost nothing: {corpus:?}"
    );
    let mut out = Vec::new();
    for path in &corpus {
        let source = fs::read_to_string(path).expect("a readable source");
        // A source this lint layer cannot analyze at all (a declarations-
        // only header, say) reports nothing either way.
        let Ok(report) = lint(&source, LintOptions::default()) else {
            continue;
        };
        for d in report.diagnostics.iter().filter(|d| d.code == code) {
            out.push(format!(
                "{}:{}:{}: {}",
                path.display(),
                d.span.start.line,
                d.span.start.col,
                d.message
            ));
        }
    }
    out
}

/// A regression guard with no single discriminating mutation on today's
/// corpus: no shipped machine is reported with or without the transparent
/// early return. The transparent direction is carried by the rule's own
/// `a_transparent_call_into_another_unit_touches_every_tape`.
#[test]
fn no_shipped_tmc_reports_unused_tape() {
    let found = offenders("unused-tape");
    assert!(found.is_empty(), "{}", found.join("\n"));
}

/// Mutation: drop transparent handling — `docs/examples/rpn/rpn.tmc`'s
/// `incr`, `decr`, `seekStart` and `normalize` (each body only a transparent
/// `call std::…()`) fire.
#[test]
fn no_shipped_tmc_reports_unused_tape_parameter() {
    let found = offenders("unused-tape-parameter");
    assert!(found.is_empty(), "{}", found.join("\n"));
}

/// Mutation: report every binding regardless of reads — any shipped rule
/// binding a name its write vector reads (the rpn sources do) fires.
#[test]
fn no_shipped_tmc_reports_unused_fold_variable() {
    let found = offenders("unused-fold-variable");
    assert!(found.is_empty(), "{}", found.join("\n"));
}

/// Vacuous today: the four `never writes` clauses the corpus carries sit on
/// parameters with no `writes` clause, so none can be inert. A regression
/// guard only; the rule's own tests carry the firing direction.
#[test]
fn no_shipped_tmc_reports_inert_never_writes() {
    let found = offenders("inert-never-writes");
    assert!(found.is_empty(), "{}", found.join("\n"));
}

/// A regression guard with no single discriminating mutation on today's
/// corpus: no shipped alphabet is exported yet undrawn on. The exported
/// direction is carried by the rule's own `an_exported_alphabet_is_not_reported`.
#[test]
fn no_shipped_tmc_reports_unused_alphabet() {
    let found = offenders("unused-alphabet");
    assert!(found.is_empty(), "{}", found.join("\n"));
}

/// A regression guard with no single discriminating mutation on today's
/// corpus; the exported direction is carried by the rule's own
/// `an_exported_map_is_not_reported`.
#[test]
fn no_shipped_tmc_reports_unused_map() {
    let found = offenders("unused-map");
    assert!(found.is_empty(), "{}", found.join("\n"));
}
