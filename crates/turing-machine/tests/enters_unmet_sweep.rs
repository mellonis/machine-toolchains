//! `enters-unmet` over every `.tmc` the repository ships: the shipped
//! corpus must stay free of the finding (docs/tmt/lint.md (enters-unmet)).
//!
//! A REGRESSION guard, not evidence that the rule works — its behavioural
//! fixtures live with the rule itself. What it holds is the other
//! direction: the rule must not start reporting the programs the
//! repository already ships. Today that is a weak promise, because a
//! source with no `enters` clause anywhere gives the rule nothing to check
//! and this sweep is green by construction. It becomes a real gate the
//! moment a shipped source — the embedded standard library first — carries
//! a clause: a clause narrower than what the routine's own call sites may
//! hand it turns this sweep red, and the defect is then the clause, not
//! the rule.
//!
//! The corpus roots mirror `tests/plain_site_sweep.rs`'s.

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

/// What it catches TODAY: nothing — no shipped source declares a clause,
/// so the analysis does not even run over this corpus and no mutation of
/// the rule can turn this red. Stated rather than dressed up: the rule's
/// own discriminating fixtures live beside it. What this guard is FOR is
/// the day a shipped clause exists, when a clause narrower than what its
/// routine's own call sites hand it turns the sweep red.
#[test]
fn no_shipped_tmc_reports_enters_unmet() {
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
        for d in report
            .diagnostics
            .iter()
            .filter(|d| d.code == "enters-unmet")
        {
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
