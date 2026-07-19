//! The TM-1 standard library: `.tmc` source embedded in the toolchain and
//! compiled once per process. The SOURCE lives here as an embedded string
//! (rather than a file in a data directory) because a cargo-installed binary
//! has no data directory. It is built with the release preset (`-O1`, `brk`
//! stripped) and linked lazily by the linker's reachability pass, so a
//! program that calls no `std::` routine pays nothing for it; `tmt link
//! --nostdlib` opts out entirely.
//!
//! The library ports the two binary-number libraries from the
//! turing-machine-js project: `std::binaryNumbers` (a 5-symbol delimited
//! representation) and `std::binaryNumbersBare` (a 3-symbol bare one). See
//! `std.tmc`'s own comment block for the representation trade-off and the
//! facade convention the source is organized around.

use std::sync::OnceLock;

use mtc_core::formats::object::ObjectFile;

use crate::compiler::{CompileOptions, compile};
use crate::optimizer::OptLevel;

/// The embedded standard-library source, in the `.tmc` 0.1 grammar.
pub const SOURCE: &str = include_str!("std.tmc");

/// The compiled standard-library object, built once per process.
///
/// Compiles [`SOURCE`] at `-O1` with `brk` stripped — the release preset —
/// which also makes this the optimizer's first live workload. The object
/// carries no `main` (a library), so nothing is dropped at compile; the
/// linker's reachability pass keeps only the routines a program actually
/// calls.
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

#[cfg(test)]
mod tests {
    use mtc_core::formats::object::SymbolDef;

    use super::*;
    use crate::cst::{ReuseCarrier, TopItem, TopKind};
    use crate::lexer::lex;
    use crate::parser::parse_cst;

    /// The fully-qualified names of every exported `routine` in `SOURCE`,
    /// walking the nested `namespace` blocks to build each `::`-joined path.
    /// Exported `graph`s are deliberately excluded: a graph is spliced into
    /// whoever grafts it and contributes no linkable symbol, so only the
    /// routine facades (and the composition routines) appear in the object.
    fn exported_routine_paths() -> Vec<String> {
        fn walk(items: &[TopItem], prefix: &str, out: &mut Vec<String>) {
            for item in items {
                match &item.kind {
                    TopKind::Namespace(ns) => {
                        let inner = if prefix.is_empty() {
                            ns.name.clone()
                        } else {
                            format!("{prefix}::{}", ns.name)
                        };
                        walk(&ns.items, &inner, out);
                    }
                    TopKind::Reuse(r) if r.exported && r.carrier == ReuseCarrier::Routine => {
                        out.push(format!("{prefix}::{}", r.name));
                    }
                    _ => {}
                }
            }
        }
        let tokens = lex(SOURCE).expect("the embedded stdlib lexes");
        let cst = parse_cst(&tokens).expect("the embedded stdlib parses");
        let mut out = Vec::new();
        walk(&cst.items, "", &mut out);
        out
    }

    /// Drift guard: the set of exported-routine paths the module declares in
    /// `SOURCE` is exactly the set of exported (`Defined`) symbol names on the
    /// compiled `object()`. The two are derived from the same `SOURCE` through
    /// independent paths (a CST walk vs a full compile), so a divergence here
    /// means one drifted from the other — a routine added, removed, renamed,
    /// or its `export` toggled without the other side following.
    #[test]
    fn roster_matches_the_compiled_objects_exported_symbols() {
        let mut roster = exported_routine_paths();
        roster.sort_unstable();
        assert_eq!(roster.len(), 14, "the fourteen exported stdlib routines");

        let mut object_names: Vec<&str> = object()
            .symbols
            .iter()
            .filter(|s| matches!(s.def, SymbolDef::Defined { .. }))
            .map(|s| s.name.as_str())
            .collect();
        object_names.sort_unstable();

        assert_eq!(roster, object_names);
    }
}
