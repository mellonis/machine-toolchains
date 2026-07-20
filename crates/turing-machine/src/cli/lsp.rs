//! `tmt lsp`: runs the `.tmc`/`.tma` language server on stdio until the client
//! exits. The only place in this CLI that hands real stdio to library code —
//! everything else stays a thin renderer over an in-memory result
//! (docs/cli.md (thin-renderer rule)); the server loop itself writes protocol
//! frames straight to the writer it is given.

use super::{Args, CliOutput};

const LSP_USAGE: &str = "USAGE: tmt lsp\n\nRun the LSP server for .tmc and .tma on stdio until the client exits.\nExit code: 0 after shutdown/exit, 1 on exit without shutdown.\n";

pub(super) fn lsp(raw: &[String]) -> Result<CliOutput, String> {
    let mut args = Args::new(raw);
    if args.flag("--help") {
        return Ok(CliOutput::ok(LSP_USAGE.into(), String::new()));
    }
    let rest = args.positionals()?;
    if !rest.is_empty() {
        return Err(format!("lsp takes no arguments\n\n{LSP_USAGE}"));
    }
    let mut tmc = crate::lsp::TmcLanguageService::new();
    let mut tma = crate::lsp::TmaLanguageService::new();
    let mut stdin = std::io::stdin().lock();
    let mut stdout = std::io::stdout();
    // `.tmc` listed first: it is the service-0 fallback an unmatched document
    // binds to, so a didOpen whose languageId and URI extension both miss
    // every service still lands somewhere instead of going unbound.
    let mut services: [&mut dyn mtc_core::lsp::LanguageService; 2] = [&mut tmc, &mut tma];
    let code = mtc_core::lsp::server::run(
        &mut stdin,
        &mut stdout,
        &mut services,
        mtc_core::lsp::server::ServerIdentity {
            name: "tmt lsp",
            version: env!("CARGO_PKG_VERSION"),
        },
    );
    Ok(CliOutput {
        stdout: String::new(),
        stderr: String::new(),
        code: code as u8,
    })
}
