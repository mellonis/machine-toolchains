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
//!  - the registry's top-level subcommand-name set is an EXACT match,
//!    both directions, against the `SUBCOMMANDS:` block parsed out of the
//!    CLI's own top-level `--help` text, and against the real parser — a
//!    subcommand wired into the dispatcher and documented in `--help` but
//!    never added to the registry (or the reverse: a stale registry entry
//!    for a subcommand `--help` no longer lists) fails loudly here
//!    instead of passing silently.
//!
//! What is NOT (and structurally cannot be) checked here: that the real
//! parser doesn't accept a FLAG the registry is MISSING. The hand-rolled
//! `Args` scanner offers no reflection over `cli/build.rs`'s match arms,
//! so that direction relies on careful authorship at review time, not a
//! mechanical check — the same limitation the PM-1 guard records. (The
//! analogous gap at the subcommand level is the one closed above.)
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

/// Parses the ordered subcommand-name list out of the `SUBCOMMANDS:`
/// block in `tmt --help`'s rendered `USAGE` text. The block is an
/// indented list under a `SUBCOMMANDS:` header; parsing stops at the
/// first blank line or the first non-indented line, whichever comes
/// first, and each kept line contributes its first whitespace-separated
/// word as the subcommand name.
fn parse_usage_subcommands(help: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut in_block = false;
    for line in help.lines() {
        if !in_block {
            if line.trim() == "SUBCOMMANDS:" {
                in_block = true;
            }
            continue;
        }
        if line.trim().is_empty() || !line.starts_with(char::is_whitespace) {
            break;
        }
        if let Some(name) = line.split_whitespace().next() {
            names.push(name.to_string());
        }
    }
    names
}

/// The maintained expectation of `tmt`'s dispatched top-level surface.
/// The set is checked in both directions against the registry AND against
/// the `SUBCOMMANDS:` block of the CLI's own help text, so a subcommand
/// can never be wired into the dispatcher without also being registered
/// here and documented there.
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
    "lsp",
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
    // list EXACTLY this set — not merely mention each name somewhere in
    // the text, which would pass even if `--help` documented a
    // subcommand this list (and the registry) never heard of. A future
    // subcommand wired into the dispatcher and documented in `SUBCOMMANDS:`
    // but never added to the registry (the `tmt lsp` scenario) now fails
    // loudly here instead of passing silently.
    let help = execute(&[]).unwrap().stdout;
    let mut usage_names = parse_usage_subcommands(&help);
    usage_names.sort();
    let missing_from_usage: Vec<&String> = expected
        .iter()
        .filter(|n| !usage_names.contains(n))
        .collect();
    let missing_from_expected: Vec<&String> = usage_names
        .iter()
        .filter(|n| !expected.contains(n))
        .collect();
    assert!(
        missing_from_usage.is_empty() && missing_from_expected.is_empty(),
        "the `SUBCOMMANDS:` block in `tmt --help` and `EXPECTED_TOP_LEVEL` disagree: \
         `SUBCOMMANDS:` is missing {missing_from_usage:?}, `EXPECTED_TOP_LEVEL` is missing \
         {missing_from_expected:?} (help block parsed as {usage_names:?})"
    );

    // Parser probe: the real dispatcher must accept each one — i.e. NOT
    // the `execute_with` catch-all's "unknown subcommand" error. `lsp` is
    // special-cased to `--help`: a bare `tmt lsp` hands real stdio to the
    // server loop and would block this test process forever waiting for a
    // client that will never connect. `tmt lsp --help` returns before any
    // stdio is touched and still proves the dispatcher knows the name.
    for name in &expected {
        let probe: Vec<String> = if name == "lsp" {
            args(&["lsp", "--help"])
        } else {
            args(&[name])
        };
        let out = execute(&probe);
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

/// `fmt --lang` is the other closed value set (`cli/fmt.rs::parse_lang`).
/// The language word is validated on the stdin (`-`) route only, so
/// driving the real accepting path in-process would read this test
/// process's own stdin to EOF — hanging an interactive `cargo test` run.
/// Spawning the real `tmt` binary with stdin wired to `Stdio::null()`
/// sidesteps that: `fmt_stdin` reads stdin to EOF before touching the
/// language choice, and a null stdin hits EOF immediately, so the
/// process returns right away rather than blocking (verified directly:
/// `tmt fmt --check --lang tmc - < /dev/null` returns immediately).
/// Precedent for spawning the binary this way: `cli_programs.rs` in the
/// PM-1 crate, and `completions_zsh.rs` alongside this file.
#[test]
fn fmt_lang_choices_match_the_documented_set_and_are_accepted_by_the_real_binary() {
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

    // Real accepting-path probe: each advertised word must not be
    // rejected by the actual binary as an unknown language.
    for language in langs {
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_tmt"))
            .args(["fmt", "--check", "--lang", language, "-"])
            .stdin(std::process::Stdio::null())
            .output()
            .expect("failed to spawn tmt");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            !stderr.contains("`--lang` takes"),
            "`tmt fmt --check --lang {language} -` was rejected as an unknown language: {stderr}"
        );
    }

    // Rejection path, still driven in-process: `parse_lang` runs before
    // stdin is ever touched, so a bad word errors immediately without
    // needing a subprocess.
    let bogus = execute(&args(&["fmt", "--lang", "not-a-language", "-"]));
    let Err(message) = &bogus else {
        panic!("a bad --lang should be rejected: {bogus:?}");
    };
    assert!(message.contains("`--lang` takes"), "{message}");
}
