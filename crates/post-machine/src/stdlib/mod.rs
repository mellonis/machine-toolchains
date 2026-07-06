//! The standard library: `.pmc` source embedded in the toolchain and
//! compiled once per process. `docs/stdlib.md` covers the prebuilt
//! `std.pmo` the linker adds implicitly; the SOURCE lives here as an
//! embedded `.pmc` string (rather than a file in a data directory)
//! because a cargo-installed binary has no data directory. Built with
//! the release preset; see `docs/stdlib.md (interposition vs
//! optimization)` for the semantic-binding caveat this implies for
//! overriding std routines.

use std::sync::OnceLock;

use mtc_core::formats::object::ObjectFile;

use crate::compiler::{CompileOptions, compile};
use crate::optimizer::OptLevel;

pub const SOURCE: &str = include_str!("std.pmc");

pub fn object() -> &'static ObjectFile {
    static OBJECT: OnceLock<ObjectFile> = OnceLock::new();
    OBJECT.get_or_init(|| {
        compile(
            SOURCE,
            CompileOptions {
                opt_level: OptLevel::O1,
                strip_debugger: true,
                ..Default::default()
            },
        )
        .expect("the embedded stdlib compiles")
        .object
    })
}
