//! Lint integration: multi-rule ordering, --allow filtering, fix
//! round-trips, the acceptance fixture, and the stdlib dogfood.

use mtc_post_machine::{LintOptions, apply_fixes, lint};

const FIXTURE: &str = include_str!("lint/unused_labels.pmc");

#[test]
fn fixture_yields_exactly_the_two_showcase_findings() {
    let report = lint(FIXTURE, LintOptions::default()).unwrap();
    let codes: Vec<_> = report
        .diagnostics
        .iter()
        .map(|d| (d.code, d.span.start.line))
        .collect();
    assert_eq!(codes, vec![("unused-label", 4), ("unused-label", 12)]);
}

#[test]
fn fixture_fixes_apply_cleanly_and_idempotently() {
    let report = lint(FIXTURE, LintOptions::default()).unwrap();
    let outcome = apply_fixes(FIXTURE, &report.diagnostics);
    assert_eq!((outcome.applied, outcome.skipped), (2, 0));
    assert!(!outcome.fixed_source.contains("1:  check"));
    assert!(!outcome.fixed_source.contains("5:  right"));
    // Idempotence: the fixed source re-lints clean.
    let rerun = lint(&outcome.fixed_source, LintOptions::default()).unwrap();
    assert!(rerun.diagnostics.is_empty());
}

#[test]
fn findings_are_source_ordered_across_rules() {
    let src = "\
main() {
007: right;
5:   left;
     goto 007;
     debugger;
}
";
    let report = lint(src, LintOptions::default()).unwrap();
    let lines: Vec<u32> = report
        .diagnostics
        .iter()
        .map(|d| d.span.start.line)
        .collect();
    let mut sorted = lines.clone();
    sorted.sort();
    assert_eq!(lines, sorted);
    let codes: Vec<_> = report.diagnostics.iter().map(|d| d.code).collect();
    // leading-zeros twice (definition + goto), unused-label (5), debugger.
    assert!(codes.contains(&"leading-zeros"));
    assert!(codes.contains(&"unused-label"));
    assert!(codes.contains(&"leftover-debugger"));
}

#[test]
fn allow_filters_a_rule_out() {
    let report = lint(
        FIXTURE,
        LintOptions {
            allow: vec!["unused-label".into()],
        },
    )
    .unwrap();
    assert!(report.diagnostics.is_empty());
}

#[test]
fn stdlib_dogfoods_clean() {
    let std_pmc = include_str!("../src/stdlib/std.pmc");
    let report = lint(std_pmc, LintOptions::default()).unwrap();
    assert!(
        report.diagnostics.is_empty(),
        "stdlib must lint clean, found: {:?}",
        report
            .diagnostics
            .iter()
            .map(|d| (d.code, d.span.start.line))
            .collect::<Vec<_>>()
    );
}
