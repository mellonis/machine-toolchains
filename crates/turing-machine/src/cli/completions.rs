//! `tmt completions`: prints a shell completion script. Thin renderer
//! over `crate::completions` — all the actual script text is built by
//! library code; this file only parses args and hands the string back
//! for `bin/tmt.rs` to print. Mirrors the PM-1 `pmt completions` shape
//! (`crates/post-machine/src/cli/completions.rs`).

use crate::completions::{parse_shell, render};

use super::{Args, CliOutput};

const COMPLETIONS_USAGE: &str = "\
USAGE: tmt completions <SHELL>

Emits a shell completion script to stdout for the given SHELL (zsh; bash
and fish are recognized but not yet implemented).

  tmt completions zsh > ~/.zfunc/_tmt
";

pub(super) fn completions(raw: &[String]) -> Result<CliOutput, String> {
    let mut args = Args::new(raw);
    if args.flag("--help") {
        return Ok(CliOutput::ok(COMPLETIONS_USAGE.into(), String::new()));
    }
    let inputs = args.positionals()?;
    let [shell_name] = inputs.as_slice() else {
        return Err(format!(
            "completions takes exactly one shell name\n\n{COMPLETIONS_USAGE}"
        ));
    };
    let shell = parse_shell(shell_name)?;
    let script = render(shell)?;
    Ok(CliOutput::ok(script, String::new()))
}
