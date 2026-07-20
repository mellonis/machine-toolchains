//! The drift guard for `crate::completions::registry`: the completion
//! registry is hand-authored (the CLI is hand-rolled, no clap — nothing
//! generates the registry FROM the parser), so nothing in the type
//! system stops it from silently drifting as `cli/build.rs` /
//! `cli/inspect.rs` / `cli/run.rs` / `cli/lint.rs` / `cli/fmt.rs` /
//! `optimizer/mod.rs` change. This file is the mechanical check against
//! that.
//!
//! What IS checked, exactly and automatically:
//!  - the registry's `--fno-<pass>` and `--emit-ir=after:<pass>` choices
//!    match `optimizer::pass_names()` exactly — including `outline`,
//!    which is a registered pass like any other even though it is the
//!    one pass that defaults OFF (its `--foutline` enable switch is a
//!    separate flag, checked separately below);
//!  - every registry subcommand path and every registry flag spelling is
//!    accepted by the REAL parser, probed by actually invoking
//!    `cli::execute` — `cli::Args::positionals` rejects any unrecognized
//!    dashed token with an "unknown flag" error, so a registry entry with
//!    a typo or an invented flag would surface that error here;
//!  - the registry's top-level subcommand-name set matches both the
//!    CLI's own top-level `--help` text and the real parser.
//!
//! What is NOT (and structurally cannot be) checked here: that the real
//! parser doesn't accept a flag the registry is MISSING. The hand-rolled
//! `Args` scanner offers no reflection over `cli/build.rs`'s match arms,
//! so that direction relies on careful authorship at review time, not a
//! mechanical check — the same limitation the PM-1 guard records.
//!
//! One more soft spot worth naming: the parser probe is vacuous for the
//! `--fno-` family specifically. `take_disabled_passes` strips ANY token
//! with that prefix without validating the suffix, so `--fno-nonsense`
//! would sail through the probe too. The pass-name cross-check above is
//! the only thing standing behind those spellings.

use mtc_turing_machine::cli::execute;
use mtc_turing_machine::completions::registry::{
    CommandSpec, FlagKind, Registry, ValueHint, expand, registry,
};
use mtc_turing_machine::optimizer::pass_names;

fn args(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| s.to_string()).collect()
}

fn find<'a>(reg: &'a Registry, path: &[&str]) -> &'a CommandSpec {
    let target: Vec<String> = path.iter().map(|s| s.to_string()).collect();
    reg.commands
        .iter()
        .find(|c| c.path == target)
        .unwrap_or_else(|| panic!("no registry entry for {path:?}"))
}

/// The maintained expectation of `tmt`'s dispatched top-level surface.
/// `lsp` is deliberately absent: it is not wired yet, and registering a
/// subcommand the parser rejects would fail the probe below — the change
/// that adds the subcommand adds its registry entry and this row
/// together.
const EXPECTED_TOP_LEVEL: &[&str] = &[
    "compile",
    "asm",
    "link",
    "dis",
    "run",
    "tape",
    "ir",
    "lint",
    "fmt",
    "completions",
];

#[test]
fn optimizer_pass_names_match_the_registry_exactly() {
    let expected: Vec<String> = pass_names().into_iter().map(|p| p.to_string()).collect();
    let reg = registry();

    let compile = find(&reg, &["compile"]);
    let fno = compile
        .flags
        .iter()
        .find(|f| f.name == "--fno-")
        .expect("compile should register --fno-<pass>");
    let FlagKind::SuffixFamily(choices) = &fno.kind else {
        panic!("--fno- should be registered as a SuffixFamily");
    };
    assert_eq!(
        choices, &expected,
        "registry's --fno-<pass> choices drifted from optimizer::pass_names()"
    );

    let emit_ir = compile
        .flags
        .iter()
        .find(|f| f.name == "--emit-ir")
        .expect("compile should register --emit-ir");
    let FlagKind::OptionalEqualsValue(ValueHint::Choices(stages)) = &emit_ir.kind else {
        panic!("--emit-ir should be an OptionalEqualsValue(Choices(..))");
    };
    let mut expected_stages = vec!["lowered".to_string(), "final".to_string()];
    expected_stages.extend(expected.iter().map(|pass| format!("after:{pass}")));
    assert_eq!(
        stages, &expected_stages,
        "registry's --emit-ir stage choices drifted from optimizer::pass_names()"
    );
}

/// `outline` is the one default-OFF pass, and the two halves of its
/// surface live in different places: the pass name rides the `--fno-`
/// family (so `--fno-outline` renders like every other pass), while
/// `--foutline` — the switch that turns it ON — is its own boolean flag
/// that must NOT be derived from, or collapsed into, the pass list.
#[test]
fn outline_is_both_a_registered_pass_and_its_own_enable_flag() {
    assert!(
        pass_names().contains(&"outline"),
        "outline should be a registered optimizer pass"
    );
    let reg = registry();
    let compile = find(&reg, &["compile"]);

    let fno = compile
        .flags
        .iter()
        .find(|f| f.name == "--fno-")
        .expect("compile should register --fno-<pass>");
    assert!(
        expand(fno).contains(&"--fno-outline".to_string()),
        "the --fno- family should carry outline like any other pass"
    );

    let foutline = compile
        .flags
        .iter()
        .find(|f| f.name == "--foutline")
        .expect("compile should register --foutline separately from the --fno- family");
    assert!(matches!(foutline.kind, FlagKind::Boolean));
}

#[test]
fn top_level_subcommands_match_the_maintained_list_cli_help_and_the_real_parser() {
    let reg = registry();

    let mut registry_names: Vec<String> = Vec::new();
    for command in &reg.commands {
        if let Some(first) = command.path.first()
            && !registry_names.contains(first)
        {
            registry_names.push(first.clone());
        }
    }
    let mut expected: Vec<String> = EXPECTED_TOP_LEVEL.iter().map(|s| s.to_string()).collect();
    registry_names.sort();
    expected.sort();
    assert_eq!(
        registry_names, expected,
        "registry's top-level subcommand set no longer matches the maintained list \
         (update EXPECTED_TOP_LEVEL alongside the registry)"
    );

    // The CLI's own top-level `--help` text (`cli/mod.rs`'s `USAGE`) must
    // still mention every one of them.
    let help = execute(&[]).unwrap().stdout;
    for name in &expected {
        assert!(
            help.contains(name.as_str()),
            "`tmt --help` no longer mentions `{name}`: {help}"
        );
    }

    // Parser probe: the real dispatcher must accept each one — i.e. NOT
    // the `execute_with` catch-all's "unknown subcommand" error.
    for name in &expected {
        let out = execute(&args(&[name]));
        let unknown_subcommand =
            matches!(&out, Err(message) if message.contains("unknown subcommand"));
        assert!(
            !unknown_subcommand,
            "`{name}` was rejected by the real dispatcher as an unknown subcommand"
        );
    }

    // Sanity check on the probe itself: a made-up name IS rejected.
    let bogus = execute(&args(&["definitely-not-a-real-subcommand"]));
    assert!(matches!(&bogus, Err(message) if message.contains("unknown subcommand")));
}

#[test]
fn every_registry_flag_is_accepted_by_the_real_parser() {
    let reg = registry();
    for command in &reg.commands {
        if command.path.is_empty() {
            continue; // root's flags are probed separately below
        }
        for flag in &command.flags {
            for spelling in expand(flag) {
                let mut full = command.path.clone();
                full.push(spelling.clone());
                let result = execute(&full);
                if let Err(message) = &result {
                    assert!(
                        !message.contains("unknown flag"),
                        "registry flag `{spelling}` on `{}` was rejected as unknown by the \
                         real parser: {message}",
                        command.path.join(" ")
                    );
                }
            }
        }
    }
}

/// Sanity check on the flag probe itself: an invented flag on a real
/// subcommand IS rejected as unknown, so the loop above would actually
/// fail on a typo'd registry entry rather than passing vacuously.
#[test]
fn the_flag_probe_rejects_a_flag_the_parser_does_not_know() {
    let bogus = execute(&args(&["compile", "--definitely-not-a-real-flag"]));
    assert!(
        matches!(&bogus, Err(message) if message.contains("unknown flag")),
        "{bogus:?}"
    );
}

#[test]
fn root_flags_are_accepted_by_the_real_top_level_dispatch() {
    let reg = registry();
    let root = reg
        .commands
        .iter()
        .find(|c| c.path.is_empty())
        .expect("registry always has a root entry");
    for flag in &root.flags {
        let out = execute(&args(&[&flag.name]));
        assert!(
            out.is_ok(),
            "root flag `{}` was rejected: {out:?}",
            flag.name
        );
    }
}

/// `take_emit_ir` (`cli/build.rs`) validates the stage string itself
/// (`lowered` / `final` / `after:` prefix) independently of the flag
/// scanner — this exercises that predicate directly against every
/// choice the registry advertises, catching a stage name the CLI
/// wouldn't actually accept.
#[test]
fn emit_ir_stage_choices_are_all_accepted_by_the_real_stage_check() {
    let reg = registry();
    let compile = find(&reg, &["compile"]);
    let emit_ir = compile
        .flags
        .iter()
        .find(|f| f.name == "--emit-ir")
        .expect("compile should register --emit-ir");
    let FlagKind::OptionalEqualsValue(ValueHint::Choices(stages)) = &emit_ir.kind else {
        panic!("--emit-ir should be an OptionalEqualsValue(Choices(..))");
    };
    for stage in stages {
        let out = execute(&args(&["compile", &format!("--emit-ir={stage}")]));
        if let Err(message) = &out {
            assert!(
                !message.contains("unknown IR stage"),
                "registry --emit-ir stage `{stage}` was rejected by the real stage check: {message}"
            );
        }
    }
    // Sanity check: a genuinely bad stage IS rejected.
    let bogus = execute(&args(&["compile", "--emit-ir=not-a-real-stage"]));
    assert!(matches!(&bogus, Err(message) if message.contains("unknown IR stage")));
}

/// `--call-mech` is the one TM-1 value flag whose accepted words the
/// parser closes over (`cli/build.rs::parse_call_mech`), so every
/// mechanism the registry advertises is probed against it — the
/// value-set analogue of the `--emit-ir` stage check above.
#[test]
fn call_mech_choices_are_all_accepted_by_the_real_parser() {
    let reg = registry();
    let link = find(&reg, &["link"]);
    let call_mech = link
        .flags
        .iter()
        .find(|f| f.name == "--call-mech")
        .expect("link should register --call-mech");
    let FlagKind::Value(ValueHint::Choices(mechs)) = &call_mech.kind else {
        panic!("--call-mech should be a Value(Choices(..))");
    };
    for mech in mechs {
        let out = execute(&args(&["link", "--call-mech", mech]));
        if let Err(message) = &out {
            assert!(
                !message.contains("unknown --call-mech"),
                "registry --call-mech `{mech}` was rejected by the real parser: {message}"
            );
        }
    }
    // Sanity check: a genuinely bad mechanism IS rejected.
    let bogus = execute(&args(&["link", "--call-mech", "not-a-mechanism"]));
    assert!(matches!(&bogus, Err(message) if message.contains("unknown --call-mech")));
}

/// `fmt --lang` is the other closed value set (`cli/fmt.rs::parse_lang`),
/// and the only one whose accepting path this guard cannot drive: the
/// language word is validated on the stdin (`-`) route ONLY, and a probe
/// that took that route would read this test process's stdin to EOF —
/// hanging an interactive `cargo test` run. So the check is one-sided
/// here: a bad word must be rejected (that error is raised before stdin
/// is ever touched), and the registry must advertise exactly the words
/// the CLI's own usage text documents.
#[test]
fn fmt_lang_choices_match_the_documented_set_and_a_bad_word_is_rejected() {
    let reg = registry();
    let fmt = find(&reg, &["fmt"]);
    let lang = fmt
        .flags
        .iter()
        .find(|f| f.name == "--lang")
        .expect("fmt should register --lang");
    let FlagKind::Value(ValueHint::Choices(langs)) = &lang.kind else {
        panic!("--lang should be a Value(Choices(..))");
    };
    assert_eq!(langs, &args(&["tmc", "tma"]));

    // `parse_lang` runs before the stdin read, so this rejection path
    // never blocks; its message names the accepted words, which is what
    // ties the registry's list to the parser's.
    let bogus = execute(&args(&["fmt", "--lang", "not-a-language", "-"]));
    let Err(message) = &bogus else {
        panic!("a bad --lang should be rejected: {bogus:?}");
    };
    assert!(message.contains("`--lang` takes"), "{message}");
    for language in langs {
        assert!(
            message.contains(language.as_str()),
            "the parser's own --lang error should name `{language}`: {message}"
        );
    }
}
