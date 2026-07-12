//! `pmt lsp`: runs the `.pmc` language server on stdio until the client
//! exits. The only place in the CLI that hands real stdio to library
//! code — everything else stays a thin renderer over an in-memory
//! result (docs/cli.md (thin-renderer rule)); the server loop itself
//! writes protocol frames straight to the writer it is given.

use super::{Args, CliOutput};

const LSP_USAGE: &str = "USAGE: pmt lsp\n\nRun the LSP server for .pmc on stdio until the client exits.\nExit code: 0 after shutdown/exit, 1 on exit without shutdown.\n";

pub(super) fn lsp(raw: &[String]) -> Result<CliOutput, String> {
    let mut args = Args::new(raw);
    if args.flag("--help") {
        return Ok(CliOutput::ok(LSP_USAGE.into(), String::new()));
    }
    let rest = args.positionals()?;
    if !rest.is_empty() {
        return Err(format!("lsp takes no arguments\n\n{LSP_USAGE}"));
    }
    let mut service = crate::lsp::PmcLanguageService::new();
    let mut stdin = std::io::stdin().lock();
    let mut stdout = std::io::stdout();
    // Single service today; the framework's `run` takes a service slice so
    // a second language can plug in later (full dual wiring is a later
    // task). One service means identity capability-merge maps, so this is
    // byte-for-byte a dedicated `.pmc` server.
    let mut services: [&mut dyn mtc_core::lsp::LanguageService; 1] = [&mut service];
    let code = mtc_core::lsp::server::run(
        &mut stdin,
        &mut stdout,
        &mut services,
        mtc_core::lsp::server::ServerIdentity {
            name: "pmt lsp",
            version: env!("CARGO_PKG_VERSION"),
        },
    );
    Ok(CliOutput {
        stdout: String::new(),
        stderr: String::new(),
        code: code as u8,
    })
}
