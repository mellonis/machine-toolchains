//! Drift guard for `docs/tmt/cli.md`: the page quotes every subcommand's
//! `--help` text VERBATIM, and a verbatim quote of a help string is a
//! drift bomb without a check — the doc and the binary can diverge with
//! nothing failing. This file is that check.
//!
//! What is checked:
//!  - every usage block the page quotes is byte-identical to what the
//!    real CLI renders for that invocation, compared against
//!    `cli::execute`'s own output rather than a retyped copy;
//!  - every top-level subcommand the completion registry knows about has
//!    such a block on the page, so a NEW subcommand fails here instead of
//!    silently going undocumented.
//!
//! Two invocation shapes, both dictated by the real parser:
//!  - `lsp` is probed as `lsp --help` and NEVER bare — a bare `tmt lsp`
//!    hands real stdio to the server loop and would block this test
//!    process forever waiting for a client that never connects;
//!  - `tape` and `ir` are probed BARE, because their children consume no
//!    `--help` (`tmt tape new --help` is an unknown-flag error); the
//!    group's own usage comes from invoking the group with no arguments.
//!
//! What is NOT checked: the page's prose. Only the fenced usage blocks
//! are mechanically pinned; the surrounding explanation is reviewed by
//! hand, as prose has to be.

use mtc_turing_machine::cli::execute;
use mtc_turing_machine::completions::registry::registry;

fn doc() -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/tmt/cli.md");
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()))
}

/// The rendered help text for one invocation, as the binary produces it.
fn help(argv: &[&str]) -> String {
    let owned: Vec<String> = argv.iter().map(|s| s.to_string()).collect();
    execute(&owned)
        .unwrap_or_else(|e| panic!("`tmt {}` should render help, got: {e}", argv.join(" ")))
        .stdout
}

/// Every invocation whose help text the page quotes, paired with the
/// top-level subcommand it documents (`None` for the root usage).
fn quoted_blocks() -> Vec<(Option<&'static str>, Vec<&'static str>)> {
    vec![
        (None, vec![]),
        (Some("compile"), vec!["compile", "--help"]),
        (Some("asm"), vec!["asm", "--help"]),
        (Some("link"), vec!["link", "--help"]),
        (Some("dis"), vec!["dis", "--help"]),
        (Some("run"), vec!["run", "--help"]),
        // Group commands: bare, not `--help` (see the module note).
        (Some("tape"), vec!["tape"]),
        (Some("ir"), vec!["ir"]),
        (Some("lint"), vec!["lint", "--help"]),
        (Some("fmt"), vec!["fmt", "--help"]),
        (Some("lsp"), vec!["lsp", "--help"]),
        (Some("completions"), vec!["completions", "--help"]),
    ]
}

#[test]
fn every_quoted_usage_block_is_byte_identical_to_the_real_help_output() {
    let doc = doc();
    for (_, argv) in quoted_blocks() {
        let rendered = help(&argv);
        // The page fences the block without its trailing blank line.
        let expected = rendered.trim_end_matches('\n');
        assert!(
            doc.contains(expected),
            "docs/tmt/cli.md no longer quotes `tmt {}`'s help text verbatim.\n\
             The binary renders:\n{expected}\n\
             Update the page's fenced block to match (or drop the verbatim quote).",
            argv.join(" ")
        );
    }
}

#[test]
fn every_top_level_subcommand_has_a_quoted_block_on_the_page() {
    let reg = registry();
    let mut subcommands: Vec<String> = Vec::new();
    for command in &reg.commands {
        if let Some(first) = command.path.first()
            && !subcommands.contains(first)
        {
            subcommands.push(first.clone());
        }
    }
    assert!(
        !subcommands.is_empty(),
        "the registry should list top-level subcommands"
    );

    let documented: Vec<&str> = quoted_blocks().into_iter().filter_map(|(n, _)| n).collect();
    for name in &subcommands {
        assert!(
            documented.contains(&name.as_str()),
            "`{name}` is a real subcommand but docs/tmt/cli.md quotes no usage block \
             for it — add a section to the page AND an entry to `quoted_blocks`"
        );
    }
}

/// The page names each subcommand in a heading, so a citation of the form
/// `docs/tmt/cli.md (tmt fmt)` resolves to a real heading rather than to an
/// incidental substring somewhere in the prose.
#[test]
fn every_subcommand_has_its_own_heading() {
    let doc = doc();
    let headings: Vec<&str> = doc
        .lines()
        .filter(|l| l.starts_with('#'))
        .map(str::trim_end)
        .collect();
    for (name, _) in quoted_blocks() {
        let Some(name) = name else { continue };
        let wanted = format!("## `tmt {name}`");
        assert!(
            headings.contains(&wanted.as_str()),
            "docs/tmt/cli.md should carry the heading `{wanted}` so citations \
             naming `tmt {name}` resolve to a heading; headings are: {headings:?}"
        );
    }
}
