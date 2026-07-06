//! The standard library: `.pmc` source embedded in the toolchain and
//! compiled once per process (ruling R1: "prebuilt std.pmo ships with
//! the toolchain" realized as an embedded object — a cargo-installed
//! binary has no data directory). Built with the release preset; -O1
//! may inline std-internal calls, so overriding a std routine rebinds
//! direct user calls, not std's internal uses (semantic binding).

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
