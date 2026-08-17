//! Debug Adapter Protocol (DAP) support. `protocol` is the typed,
//! `seq`/`type`-tagged wire envelope shared by every DAP request,
//! response, and event; `server` is the blocking run loop over it
//! (mirroring `lsp::server` for DAP's own message shape and lifecycle).
//! The user-facing protocol surface this crate's two consumers
//! (`pmt dap`/`tmt dap`) build on this framework is documented at
//! docs/dap.md.

pub mod protocol;
pub mod server;
