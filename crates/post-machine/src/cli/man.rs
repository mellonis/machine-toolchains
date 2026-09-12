//! `pmt man`: prints the `pmt(1)` manual page. Thin renderer over
//! `crate::man` — the roff is built by library code; this file only
//! parses args and hands the string back for `bin/pmt.rs` to print.
//! Mirrors the TM-1 `tmt man` shape
//! (`crates/turing-machine/src/cli/man.rs`).

use super::{Args, CliOutput};

pub(super) const MAN_USAGE: &str = "\
USAGE: pmt man

Emits the pmt(1) manual page as man(7) roff to stdout.

  pmt man > pmt.1
  man ./pmt.1
";

pub(super) fn man(raw: &[String]) -> Result<CliOutput, String> {
    let mut args = Args::new(raw);
    if args.help() {
        return Ok(CliOutput::ok(MAN_USAGE.into(), String::new()));
    }
    let inputs = args.positionals()?;
    if !inputs.is_empty() {
        return Err(format!("man takes no arguments\n\n{MAN_USAGE}"));
    }
    Ok(CliOutput::ok(crate::man::render(), String::new()))
}
