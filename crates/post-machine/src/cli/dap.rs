//! `pmt dap`: runs the Debug Adapter Protocol server for a `.pmx` program
//! on stdio until the client disconnects. Mirrors `cli/lsp.rs`'s shape —
//! the only other place real stdio is handed to library code
//! (docs/pmt/cli.md (thin-renderer rule)); the server loop itself writes
//! protocol frames straight to the writer it is given. The launch schema
//! and protocol surface `PmDapAdapter` answers are documented at
//! docs/dap.md.

use super::{Args, CliOutput};

const DAP_USAGE: &str = "USAGE: pmt dap\n\nRun the DAP debug-adapter server for a .pmx program on stdio until the client disconnects.\nExit code: 0 after a clean disconnect, 1 on transport EOF before one.\n";

pub(super) fn dap(raw: &[String]) -> Result<CliOutput, String> {
    let mut args = Args::new(raw);
    if args.help() {
        return Ok(CliOutput::ok(DAP_USAGE.into(), String::new()));
    }
    let rest = args.positionals()?;
    if !rest.is_empty() {
        return Err(format!("dap takes no arguments\n\n{DAP_USAGE}"));
    }
    let mut adapter = crate::dap::PmDapAdapter::new();
    // Owned (not locked): `mtc_core::dap::server::run` moves `reader` into
    // its background thread, so it must be `Send + 'static` — `StdinLock`
    // is not `Send`, but plain `Stdin` (which locks internally per read)
    // is, mirroring why `cli/lsp.rs` locks stdin/stdout but this cannot.
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    let code = mtc_core::dap::server::run(stdin, &mut stdout, &mut adapter);
    Ok(CliOutput {
        stdout: String::new(),
        stderr: String::new(),
        code: code as u8,
    })
}
