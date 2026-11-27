//! `unused-set` over every `.tmc` the repository ships: the shipped corpus
//! must stay free of the finding (docs/tmt/lint.md (unused-set)).
//!
//! A REGRESSION guard, not evidence that the rule works — its behavioural
//! fixtures live with the rule itself (`crates/turing-machine/src/lint/
//! rules/unused_set.rs`) and its harness pins live in
//! `tests/lint_fix_comment_guard.rs` / `tests/lint_quickfix_comments.rs`.
//! What this holds is the other direction: the rule must not start
//! reporting a set the repository already ships as dead.
//!
//! Unlike `tests/enters_unmet_sweep.rs`'s own caveat, this sweep is not
//! purely vacuous: `tests/golden/glyph_sets.tmc` declares two sets (`low`,
//! built into `digits`; `digits` itself named from both an alphabet body
//! and an `enters` clause), so this sweep does exercise the rule against a
//! real declaration today — it would fail if either recording site
//! regressed on that file. The embedded standard library and
//! `docs/examples` ship no `set` declaration yet, so the promise stays
//! weak there until one is added.
//!
//! The corpus roots mirror `tests/enters_unmet_sweep.rs`'s own.

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

/// Mutation: a `set_refs` lookup that never misses (the rule itself), or a
/// recording site that stops recording — either would turn `digits` or
/// `low` in `tests/golden/glyph_sets.tmc` into a false `unused-set`
/// finding, since those are the only sets any shipped `.tmc` today
/// declares.
#[test]
fn no_shipped_tmc_reports_unused_set() {
    let corpus = corpus();
    assert!(
        corpus.len() >= 5,
        "the corpus roots resolved to almost nothing: {corpus:?}"
    );
    let mut offenders: Vec<String> = Vec::new();
    for path in &corpus {
        let source = fs::read_to_string(path).expect("a readable source");
        // A source this lint layer cannot analyze at all (a declarations-
        // only header, say) is not this sweep's business — it reports
        // nothing either way.
        let Ok(report) = lint(&source, LintOptions::default()) else {
            continue;
        };
        for d in report.diagnostics.iter().filter(|d| d.code == "unused-set") {
            offenders.push(format!(
                "{}:{}:{}: {}",
                path.display(),
                d.span.start.line,
                d.span.start.col,
                d.message
            ));
        }
    }
    assert!(offenders.is_empty(), "{}", offenders.join("\n"));
}
