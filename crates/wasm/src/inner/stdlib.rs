//! The standard library as the browser links it: the embedded source of
//! each architecture, and an object compiled from it WITH the line table.
//!
//! The arch crates' own `stdlib::object()` is the release preset — `-O1`,
//! `brk` stripped, no debug info — which is right for a CLI that opens
//! the materialized source through the language server when a user asks
//! for it. A page shows the library beside the user's source and steps
//! into it, so it needs stdlib addresses to resolve to lines of that text
//! (`docs/wasm.md (the standard library)`). Debug info is a side table:
//! the code bytes of this object are pinned identical to the release
//! object's (`tests/stdlib.rs`), so a browser-linked image is the image
//! the CLI would write. Built once per process, like its sibling.

use std::sync::OnceLock;

use mtc_core::formats::object::ObjectFile;

use super::Arch;

/// The embedded `.pmc` / `.tmc` text — exactly what [`object`] compiled,
/// so a page showing it and a line table indexing it cannot drift.
pub fn source(arch: Arch) -> &'static str {
    match arch {
        Arch::Pm1 => mtc_post_machine::stdlib::SOURCE,
        Arch::Tm1 => mtc_turing_machine::stdlib::SOURCE,
    }
}

/// The library object with its line table, built once per process per
/// architecture: the release preset plus `debug_info`.
pub fn object(arch: Arch) -> &'static ObjectFile {
    match arch {
        Arch::Pm1 => {
            static OBJECT: OnceLock<ObjectFile> = OnceLock::new();
            OBJECT.get_or_init(|| {
                use mtc_post_machine::compiler::{CompileOptions, VariantColumns, compile};
                use mtc_post_machine::optimizer::OptLevel;
                compile(
                    source(arch),
                    CompileOptions {
                        opt_level: OptLevel::O1,
                        strip_debugger: true,
                        columns: VariantColumns::Both,
                        debug_info: true,
                        ..Default::default()
                    },
                )
                .expect("the embedded PM-1 stdlib compiles")
                .object
            })
        }
        Arch::Tm1 => {
            static OBJECT: OnceLock<ObjectFile> = OnceLock::new();
            OBJECT.get_or_init(|| {
                use mtc_turing_machine::compiler::{CompileOptions, ExternalContracts, compile};
                use mtc_turing_machine::optimizer::OptLevel;
                compile(
                    source(arch),
                    CompileOptions {
                        opt_level: OptLevel::O1,
                        strip_debugger: true,
                        // The library vouches for nobody but itself, and must
                        // not consult its own once-per-process cache while
                        // building — the arch crate's own reasoning.
                        externals: ExternalContracts::None,
                        debug_info: true,
                        ..Default::default()
                    },
                )
                .expect("the embedded TM-1 stdlib compiles")
                .object
            })
        }
    }
}
