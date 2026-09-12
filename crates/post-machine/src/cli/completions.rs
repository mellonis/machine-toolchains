//! `pmt completions`: prints a shell completion script. Thin renderer
//! over `crate::completions` — all the actual script text is built by
//! library code; this file only parses args and hands the string back
//! for `bin/pmt.rs` to print.

use crate::completions::{parse_shell, render};

use super::{Args, CliOutput};

pub(super) const COMPLETIONS_USAGE: &str = "\
USAGE: pmt completions <SHELL>

Emits a shell completion script to stdout for the given SHELL: zsh, bash,
or fish.

  pmt completions zsh  > ~/.zfunc/_pmt
  pmt completions bash > ~/.local/share/bash-completion/completions/pmt
  pmt completions fish > ~/.config/fish/completions/pmt.fish
";

pub(super) fn completions(raw: &[String]) -> Result<CliOutput, String> {
    let mut args = Args::new(raw);
    if args.help() {
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
