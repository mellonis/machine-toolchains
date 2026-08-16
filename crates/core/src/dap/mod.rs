//! Debug Adapter Protocol (DAP) support. `protocol` is the typed,
//! `seq`/`type`-tagged wire envelope shared by every DAP request,
//! response, and event; `server` is the blocking run loop over it
//! (mirroring `lsp::server` for DAP's own message shape and lifecycle).

pub mod protocol;
pub mod server;
