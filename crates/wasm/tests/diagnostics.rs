//! `check` is the lint channel (findings, plus a compile fatal rendered as
//! one error); compile warnings travel with `build`, never here — the same
//! split the CLI keeps between `lint` and `compile`.

use mtc_wasm::inner::Lang;
use mtc_wasm::inner::diagnostics::{CheckError, CheckOptions, Severity, check, format};

const PMC_UNUSED_LABEL: &str =
    "namespace api {\nhelper() {\n5: right;\n}\n}\nmain() { @api::helper(); }\n";
const TMC_UNUSED_ALPHABET: &str = "alphabet ab { '_', 'a', 'b' }\nalphabet spare { '_', 'x' }\n\nmachine {\n  tape main: ab;\n\n  entry state scan {\n    ['b'] -> write ['a'] move [>] goto scan;\n    ['a'] ->             move [>] goto scan;\n    ['_'] -> stop;\n  }\n}\n";

fn opts() -> CheckOptions {
    CheckOptions {
        allow: vec![],
        warn: vec![],
    }
}

#[test]
fn pmc_lint_finding_crosses_with_utf16_span() {
    let diags = check(Lang::Pmc, PMC_UNUSED_LABEL, &opts()).unwrap();
    let d = diags
        .iter()
        .find(|d| d.code == "unused-label")
        .expect("the finding");
    assert_eq!(d.severity, Severity::Warning);
    // "namespace api {\nhelper() {\n" is 27 units; "5" sits at offset 27.
    assert_eq!(d.from, 27, "span starts at the label");
    assert!(d.to > d.from, "half-open, non-empty");
    assert!(d.message.contains("5"), "names the label: {}", d.message);
}

#[test]
fn tmc_lint_finding_crosses() {
    let diags = check(Lang::Tmc, TMC_UNUSED_ALPHABET, &opts()).unwrap();
    let d = diags
        .iter()
        .find(|d| d.code == "unused-alphabet")
        .expect("the finding");
    assert_eq!(d.severity, Severity::Warning);
    let line2 = "alphabet ab { '_', 'a', 'b' }\n".encode_utf16().count() as u32;
    assert!(
        d.from >= line2 && d.from < line2 + 40,
        "on the second line: {}",
        d.from
    );
    let fix = d
        .fix
        .as_ref()
        .expect("unused-alphabet carries a deletion fix");
    // The rule marks its whole-declaration deletion `MaybeIncorrect`
    // (crates/turing-machine/src/lint/rules/unused_alphabet.rs), so the
    // binding must report it as not machine-applicable: an editor must
    // not auto-apply it.
    assert!(
        !fix.machine_applicable,
        "MaybeIncorrect, not auto-applicable"
    );
    assert_eq!(fix.edits.len(), 1);
    assert_eq!(fix.edits[0].replacement, "", "a deletion");
}

#[test]
fn a_machine_applicable_fix_maps_true() {
    // `leading-zeros` (crates/post-machine/src/lint/rules/leading_zeros.rs)
    // is `Applicability::MachineApplicable`; without a fixture exercising
    // that arm, `from_core`'s `matches!` could be inverted and no test here
    // would notice.
    let diags = check(Lang::Pmc, "main() {\n007: right;\n}\n", &opts()).unwrap();
    let d = diags
        .iter()
        .find(|d| d.code == "leading-zeros")
        .expect("the finding");
    let fix = d.fix.as_ref().expect("leading-zeros carries a rewrite fix");
    assert!(fix.machine_applicable);
}

#[test]
fn allow_suppresses_and_unknown_allow_is_a_caller_error() {
    let allowed = CheckOptions {
        allow: vec!["unused-alphabet".into()],
        warn: vec![],
    };
    let diags = check(Lang::Tmc, TMC_UNUSED_ALPHABET, &allowed).unwrap();
    assert!(diags.iter().all(|d| d.code != "unused-alphabet"));
    let bogus = CheckOptions {
        allow: vec!["no-such-rule".into()],
        warn: vec![],
    };
    assert!(matches!(
        check(Lang::Tmc, TMC_UNUSED_ALPHABET, &bogus),
        Err(CheckError::UnknownAllowCode(c)) if c == "no-such-rule"
    ));
}

#[test]
fn compile_fatal_is_one_error_diagnostic() {
    let diags = check(Lang::Pmc, "main() { this is not pmc", &opts()).unwrap();
    assert_eq!(diags.len(), 1, "exactly the fatal: {diags:?}");
    assert_eq!(diags[0].severity, Severity::Error);
    assert!(
        !diags[0].code.is_empty(),
        "carries the compiler's error code"
    );
    let diags = check(Lang::Tmc, "machine {", &opts()).unwrap();
    assert_eq!(diags.len(), 1);
    assert_eq!(diags[0].severity, Severity::Error);
}

#[test]
fn format_is_idempotent_and_reports_a_fatal_as_a_diagnostic() {
    for (lang, src) in [
        (Lang::Pmc, PMC_UNUSED_LABEL),
        (Lang::Tmc, TMC_UNUSED_ALPHABET),
    ] {
        let once = format(lang, src).unwrap();
        let twice = format(lang, &once).unwrap();
        assert_eq!(once, twice, "{lang:?} fmt is idempotent");
        let tokens = |s: &str| s.split_whitespace().collect::<String>();
        assert_eq!(
            tokens(&once),
            tokens(src),
            "{lang:?} fmt is whitespace-only"
        );
    }
    let err = format(Lang::Pmc, "main( {").unwrap_err();
    assert_eq!(err.severity, Severity::Error);
}
