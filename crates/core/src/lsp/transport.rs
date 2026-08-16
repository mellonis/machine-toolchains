//! Content-Length message framing for the LSP server (LSP 3.17 base
//! protocol), re-exported from the shared codec at [`crate::framing`];
//! JSON-RPC envelope parsing lives above this layer (docs/lsp.md).

pub use crate::framing::{MAX_CONTENT_LENGTH, TransportError, read_message, write_message};
