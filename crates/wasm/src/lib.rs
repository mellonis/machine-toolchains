//! Browser binding for the machine toolchains. `inner/` is plain Rust over
//! the three crates' public APIs and is what the native tests exercise;
//! this file and `js.rs` are the wasm-bindgen layer over it (filled in by
//! the later tasks of this arc).

#[doc(hidden)]
pub mod inner;
