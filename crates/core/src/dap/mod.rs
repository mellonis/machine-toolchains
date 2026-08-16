//! Debug Adapter Protocol (DAP) support. `protocol` is the typed,
//! `seq`/`type`-tagged wire envelope shared by every DAP request,
//! response, and event; a server loop over it (mirroring `lsp::server`
//! for DAP's own message shape) lands in a follow-up module.

pub mod protocol;
