//! The `tmt(1)` manual page, rendered as `man(7)` roff (docs/tmt/cli.md
//! (tmt man)). Library code: `cli::man` hands the string to `bin/tmt.rs`
//! to print, per the thin-renderer rule.
//!
//! The page is built from two sources the binary already carries, and
//! from nothing else:
//!
//! * the completion registry's root entry — the subcommand list with its
//!   one-line glosses — for NAME, SYNOPSIS and the SUBCOMMANDS list;
//! * every subcommand's `--help` text (`cli::usage_text`), reproduced
//!   verbatim in a literal block under a section of its own.
//!
//! So the page cannot say anything `--help` does not, and a subcommand
//! that is registered but has no usage text fails this module's tests
//! rather than silently going undocumented. EXIT STATUS and SEE ALSO are
//! the only prose authored here.
//!
//! Escaping follows roff's rules for text lines: a backslash becomes
//! `\e`, a hyphen `\-` (so flags render as ASCII minus and copy-paste
//! back into a shell), and a line opening with `.` or `'` gets `\&` in
//! front so roff does not read it as a request.

use crate::cli::usage_text;
use crate::completions::registry::{Positional, PositionalHint, registry};

/// The date `.TH` carries: the day this crate version was released. A
/// binary has no clock it could trust for this and no release metadata
/// beyond its version, so the date is a literal — bumped with the version
/// at each release, and held against the CHANGELOG's heading for that
/// version by this module's tests so it cannot go stale unnoticed.
pub const RELEASE_DATE: &str = "2026-09-02";

pub fn render() -> String {
    let reg = registry();
    let root = reg
        .commands
        .iter()
        .find(|c| c.path.is_empty())
        .expect("registry always has a root entry");
    let Positional::One(PositionalHint::Choices(subcommands)) = &root.positional else {
        panic!("root positional must be a subcommand-name choice list");
    };
    let root_usage = usage_text(&[]).expect("the root usage text");
    let tagline = root_usage.lines().next().unwrap_or_default();
    let (name, gloss) = tagline
        .split_once(" — ")
        .expect("the root usage opens with `<name> — <gloss>`");
    let version = env!("CARGO_PKG_VERSION");

    let mut out = String::new();
    out.push_str(&format!(
        ".TH {} 1 \"{RELEASE_DATE}\" \"{name} {version}\" \"User Commands\"\n",
        name.to_uppercase()
    ));
    out.push_str(&format!(".SH NAME\n{name} \\- {}\n", escape(gloss)));
    out.push_str(&format!(
        ".SH SYNOPSIS\n\
         .B {name}\n\
         .I SUBCOMMAND\n\
         .RI [ ARGS ]\n\
         .br\n\
         .B {name}\n\
         .RB [ \\-\\-help \" | \" \\-\\-version ]\n"
    ));
    // The gloss sits on a line of its own: roff joins the lines, and
    // mandoc's style lint caps an input line at 80 bytes.
    out.push_str(&format!(
        ".SH DESCRIPTION\n\
         .B {name}\n\
         is the command\\-line front to the\n\
         {}:\n\
         one binary carrying the compiler, assembler, linker, disassembler,\n\
         virtual machine, linter, formatter, language server and debug adapter,\n\
         each as a subcommand.\n\
         The usage of every subcommand, exactly as\n\
         .B \"{name} <SUBCOMMAND> \\-\\-help\"\n\
         prints it, follows in a section of its own.\n",
        escape(gloss)
    ));

    out.push_str(".SH SUBCOMMANDS\n");
    for sub in subcommands {
        out.push_str(&format!(".TP\n.B {}\n{}\n", sub.value, escape(&sub.help)));
    }
    for sub in subcommands {
        let usage = usage_text(&[sub.value.as_str()])
            .unwrap_or_else(|| panic!("`{}` is registered but has no usage text", sub.value));
        out.push_str(&format!(
            ".SS \"{name} {}\"\n.nf\n{}\n.fi\n",
            sub.value,
            escape(usage.trim_end_matches('\n'))
        ));
    }

    out.push_str(
        ".SH EXIT STATUS\n\
         Every subcommand exits 0 on success and 1 on a tool error: a bad flag, an\n\
         unreadable input, a compile or link failure.\n\
         .B run\n\
         reports the program's own outcome instead: 0 when it stopped, 2 when it\n\
         halted, 3 when it trapped, and 1 still for a tool error.\n\
         .B lint\n\
         exits 1 when any finding or error is reported, and\n\
         .B \"fmt \\-\\-check\"\n\
         exits 1 when any file would change.\n\
         .B lsp\n\
         and\n\
         .B dap\n\
         exit 0 after an orderly shutdown and 1 when the client went away without\n\
         one.\n",
    );
    out.push_str(
        ".SH SEE ALSO\n\
         .BR pmt (1),\n\
         the Post\\-machine toolchain of the same family.\n\
         .PP\n\
         The reference pages shipped with the toolchain's sources under\n\
         .IR docs/tmt/ :\n\
         language, isa, asm, cli, optimizer, lint, fmt, stdlib, project.\n",
    );
    out
}

/// Escape a block of text for roff text lines: `\` → `\e`, `-` → `\-`,
/// and a line opening with `.` or `'` is prefixed with `\&` so it cannot
/// be read as a request. Backslashes go first, so the ones this
/// function introduces are not escaped again.
fn escape(text: &str) -> String {
    text.lines()
        .map(|line| {
            let line = line.replace('\\', "\\e").replace('-', "\\-");
            if line.starts_with('.') || line.starts_with('\'') {
                format!("\\&{line}")
            } else {
                line
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::usage_text;
    use crate::completions::registry::registry;
    use crate::completions::registry::{Positional, PositionalHint};

    fn top_level_names() -> Vec<String> {
        let reg = registry();
        let root = reg.commands.iter().find(|c| c.path.is_empty()).unwrap();
        let Positional::One(PositionalHint::Choices(choices)) = &root.positional else {
            panic!("root positional is the subcommand list");
        };
        choices.iter().map(|c| c.value.clone()).collect()
    }

    #[test]
    fn usage_text_covers_the_root_and_every_registered_subcommand() {
        assert!(
            usage_text(&[]).is_some_and(|u| u.starts_with("tmt — ")),
            "the root usage opens with the tagline"
        );
        for name in top_level_names() {
            let text = usage_text(&[name.as_str()])
                .unwrap_or_else(|| panic!("`{name}` is registered but has no usage text"));
            assert!(
                text.starts_with(&format!("USAGE: tmt {name}")),
                "`{name}`'s usage text opens with its own USAGE line:\n{text}"
            );
        }
        assert!(usage_text(&["nonesuch"]).is_none());
    }

    #[test]
    fn opens_with_a_th_line_carrying_the_release_date_and_version() {
        let page = render();
        let version = env!("CARGO_PKG_VERSION");
        assert!(
            page.starts_with(&format!(
                ".TH TMT 1 \"{RELEASE_DATE}\" \"tmt {version}\" \"User Commands\"\n"
            )),
            "{page}"
        );
    }

    /// The release date is a literal, so this is what keeps it honest: the
    /// CHANGELOG's heading for this crate's version — `## [<version>] -
    /// <date>` — must carry the same date. A version bump without the
    /// matching literal bump fails here. A release candidate has no
    /// heading of its own; it takes the date of the version it precedes.
    #[test]
    fn release_date_matches_the_changelog_heading_for_this_version() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../CHANGELOG.md");
        let changelog = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
        let version = env!("CARGO_PKG_VERSION");
        let base = version.split('-').next().unwrap();
        let heading = changelog
            .lines()
            .find(|l| {
                l.starts_with(&format!("## [{version}] "))
                    || l.starts_with(&format!("## [{base}] "))
            })
            .unwrap_or_else(|| panic!("CHANGELOG.md has no heading for version {version}"));
        assert_eq!(
            heading.rsplit(" - ").next().unwrap().trim(),
            RELEASE_DATE,
            "RELEASE_DATE must match the CHANGELOG heading {heading:?}"
        );
    }

    #[test]
    fn name_section_takes_the_tagline_from_the_root_usage() {
        let page = render();
        assert!(
            page.contains(".SH NAME\ntmt \\- Turing\\-machine toolchain (TM\\-1)\n"),
            "{page}"
        );
    }

    #[test]
    fn every_subcommand_has_a_section_quoting_its_usage_verbatim() {
        let page = render();
        for name in top_level_names() {
            let heading = format!(".SS \"tmt {name}\"\n.nf\n");
            let start = page
                .find(&heading)
                .unwrap_or_else(|| panic!("no section for `{name}`:\n{page}"));
            let body = &page[start + heading.len()..];
            let body = &body[..body.find("\n.fi\n").expect("the literal block closes")];
            let expected = escape(usage_text(&[name.as_str()]).unwrap().trim_end_matches('\n'));
            assert_eq!(
                body, expected,
                "`{name}`'s section is its usage text, escaped"
            );
        }
    }

    #[test]
    fn escaping_protects_leading_dots_hyphens_and_backslashes() {
        assert_eq!(
            escape(".tmc source -> .tmo object"),
            "\\&.tmc source \\-> .tmo object"
        );
        assert_eq!(escape("  -o OUT.tmo"), "  \\-o OUT.tmo");
        assert_eq!(escape("a\\b"), "a\\eb");
        assert_eq!(escape("'quoted' start"), "\\&'quoted' start");
        assert_eq!(escape("one\n.two"), "one\n\\&.two");
    }

    #[test]
    fn exit_status_and_see_also_close_the_page() {
        let page = render();
        let exit = page.find(".SH EXIT STATUS\n").expect("EXIT STATUS section");
        let see = page.find(".SH SEE ALSO\n").expect("SEE ALSO section");
        assert!(exit < see, "EXIT STATUS precedes SEE ALSO");
        assert!(page[exit..see].contains("3 when it trapped"), "{page}");
        assert!(page[see..].contains(".BR pmt (1)"), "{page}");
        assert!(page.ends_with('\n'), "the page ends with a newline");
    }
}
