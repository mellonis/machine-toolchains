//! Language-agnostic LSP server framework (LSP 3.17 subset): framing,
//! JSON-RPC, protocol structs, position mapping, document store, and the
//! blocking server loop behind the `LanguageService` seam. Carries zero
//! architecture or language knowledge by contract — exercised against a
//! crate-private fake service (docs/lsp.md; docs/cli.md (thin-renderer rule)).

pub mod jsonrpc;
pub mod position;
pub mod transport;
pub mod types;
