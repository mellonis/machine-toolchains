//! The drift guard for `crate::completions::registry`: the completion
//! registry is hand-authored (the CLI is hand-rolled, no clap — nothing
//! generates the registry FROM the parser), so nothing in the type
//! system stops it from silently drifting as `cli/build.rs` /
//! `cli/inspect.rs` / `cli/run.rs` / `optimizer/mod.rs` change. This file
//! is the mechanical check against that.
//!
//! What IS checked, exactly and automatically:
//!  - the registry's `--fno-<pass>` and `--emit-ir=after:<pass>` choices
//!    match `optimizer::pass_names()` exactly — the single most likely
//!    thing to drift, since the CLI's own docs (the issue this shipped
//!    against) got this wrong (assumed underscore names; the real pass
//!    names are hyphenated, straight from `optimizer::mod::PIPELINE`);
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
//! mechanical check — same limitation the issue's own text acknowledges
//! for a hand-rolled CLI with no framework to generate from.

use mtc_post_machine::cli::execute;
use mtc_post_machine::completions::registry::{
    CommandSpec, FlagKind, Registry, ValueHint, expand, registry,
};
use mtc_post_machine::optimizer::pass_names;

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

const EXPECTED_TOP_LEVEL: &[&str] = &[
    "compile",
    "asm",
    "link",
    "lint",
    "dis",
    "run",
    "tape",
    "ir",
    "completions",
];

#[test]
fn optimizer_pass_names_match_the_registry_exactly() {
    let expected = pass_names(); // ordered: inline, then the 7-pass pipeline
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
            "`pmt --help` no longer mentions `{name}`: {help}"
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
