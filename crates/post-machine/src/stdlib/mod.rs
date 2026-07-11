//! The standard library: `.pmc` source embedded in the toolchain and
//! compiled once per process. `docs/stdlib.md` covers the prebuilt
//! `std.pmo` the linker adds implicitly; the SOURCE lives here as an
//! embedded `.pmc` string (rather than a file in a data directory)
//! because a cargo-installed binary has no data directory. Built with
//! the release preset; see `docs/stdlib.md (interposition vs
//! optimization)` for the semantic-binding caveat this implies for
//! overriding std routines.
//!
//! The [`roster`] and [`materialized_std_uri`] below serve the LSP's
//! go-to-definition on `std::` calls (docs/lsp.md (navigation)): the
//! roster locates each exported routine's name token in `SOURCE`, and
//! the materializer writes `SOURCE` to a real file on disk once per
//! toolchain version so an editor has something to open.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use mtc_core::diagnostics::Span;
use mtc_core::formats::object::ObjectFile;

use crate::compiler::{CompileOptions, compile};
use crate::cst::TopKind;
use crate::lexer::lex;
use crate::optimizer::OptLevel;
use crate::parser::parse_cst;

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

/// One exported std routine, as declared in `SOURCE` (docs/lsp.md
/// (navigation)) — the go-to-definition target for a `std::<name>` call
/// site.
#[allow(dead_code)] // consumer: PmcLanguageService::navigate (LSP plan 2, Task 9)
pub(crate) struct RosterEntry {
    pub full_path: String,
    /// Span of the routine name token alone, in `SOURCE`.
    pub name_span: Span,
    pub decl_line: u32,
}

/// Parses `SOURCE` once (lex → `parse_cst`, no hand parsing) into the
/// roster of exported routines in the `std` namespace block.
#[allow(dead_code)] // consumer: PmcLanguageService::navigate (LSP plan 2, Task 9)
pub(crate) fn roster() -> &'static [RosterEntry] {
    static ROSTER: OnceLock<Vec<RosterEntry>> = OnceLock::new();
    ROSTER.get_or_init(|| {
        let tokens = lex(SOURCE).expect("the embedded stdlib lexes");
        let cst = parse_cst(&tokens).expect("the embedded stdlib parses");
        let mut entries = Vec::new();
        for top in &cst.items {
            let TopKind::Namespace(ns) = &top.kind else {
                continue;
            };
            if ns.name != "std" {
                continue;
            }
            for body in &ns.items {
                let TopKind::Function(f) = &body.kind else {
                    continue;
                };
                if !f.exported {
                    continue;
                }
                entries.push(RosterEntry {
                    full_path: format!("std::{}", f.name),
                    name_span: f.name_span,
                    decl_line: f.line,
                });
            }
        }
        entries
    })
}

/// The cache directory root: `$XDG_CACHE_HOME` falling back to
/// `~/.cache` on unix, `%LOCALAPPDATA%` on windows. `None` if the
/// relevant environment variable(s) are unset — the materializer
/// degrades to `None` rather than guessing a location.
#[allow(dead_code)] // consumer: materialized_std_uri (below); also LSP plan 2, Task 9
fn cache_root() -> Option<PathBuf> {
    if cfg!(windows) {
        std::env::var_os("LOCALAPPDATA").map(PathBuf::from)
    } else {
        std::env::var_os("XDG_CACHE_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache")))
    }
}

/// True for the bytes RFC 3986 leaves unescaped in a URI path:
/// `unreserved` plus `/` (the segment separator) and `:` (legal
/// unescaped in a `pchar` per RFC 3986 §3.3, sub-delims/`:`/`@` — and
/// needed literal for a windows drive letter, `file:///C:/...`).
fn is_uri_literal(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~' | b'/' | b':')
}

/// Builds a `file:` URI for an absolute path: forward slashes,
/// percent-encoding every byte outside [`is_uri_literal`]. On windows,
/// prefixes `file:///C:/...` (the extra `/` before the drive letter).
#[allow(dead_code)] // consumer: materialize_into (below); also LSP plan 2, Task 9
fn path_to_file_uri(path: &Path) -> String {
    let raw = path.to_string_lossy();
    let normalized = if cfg!(windows) {
        raw.replace('\\', "/")
    } else {
        raw.into_owned()
    };
    let mut uri = String::from("file://");
    if cfg!(windows) {
        uri.push('/');
    }
    for byte in normalized.as_bytes() {
        if is_uri_literal(*byte) {
            uri.push(*byte as char);
        } else {
            uri.push_str(&format!("%{:02X}", byte));
        }
    }
    uri
}

/// Writes `SOURCE` to `<root>/pmt/<CARGO_PKG_VERSION>/std.pmc` if the
/// file is absent or its bytes differ from `SOURCE` (self-heals a
/// corrupted or stale cache file), then returns its `file:` URI. Any IO
/// failure degrades to `None` (docs/lsp.md (materialized stdlib)).
#[allow(dead_code)] // consumer: materialized_std_uri (below); also LSP plan 2, Task 9
fn materialize_into(root: &Path) -> Option<String> {
    let dir = root.join("pmt").join(env!("CARGO_PKG_VERSION"));
    fs::create_dir_all(&dir).ok()?;
    let file = dir.join("std.pmc");
    let needs_write = match fs::read(&file) {
        Ok(existing) => existing != SOURCE.as_bytes(),
        Err(_) => true,
    };
    if needs_write {
        fs::write(&file, SOURCE).ok()?;
    }
    Some(path_to_file_uri(&file))
}

/// The embedded `std.pmc`, written once per toolchain version to
/// `<cache>/pmt/<version>/std.pmc`, as a `file:` URI (docs/lsp.md
/// (materialized stdlib)). `None` if the cache root can't be located or
/// any IO step fails — go-to-definition on `std::` calls then degrades
/// to null rather than pointing at a file that doesn't exist.
#[allow(dead_code)] // consumer: PmcLanguageService::navigate (LSP plan 2, Task 9)
pub(crate) fn materialized_std_uri() -> Option<&'static str> {
    static URI: OnceLock<Option<String>> = OnceLock::new();
    URI.get_or_init(|| cache_root().and_then(|root| materialize_into(&root)))
        .as_deref()
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use mtc_core::formats::object::SymbolDef;

    use super::*;

    /// A fresh scratch directory under `std::env::temp_dir()`, unique per
    /// call (process id + an atomic counter — this crate has no tempfile
    /// dependency, matching the zero-new-deps constraint).
    fn unique_tmp_dir(label: &str) -> PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "pmt-stdlib-roster-test-{label}-{}-{n}",
            std::process::id()
        ))
    }

    /// Drift guard: the roster's full paths are exactly the SET of
    /// exported (`SymbolDef::Defined`) symbol names on the compiled
    /// `object()` — the two are derived from the same `SOURCE` through
    /// independent paths (CST walk vs compile-and-link), so a divergence
    /// here means one of them drifted from the other.
    #[test]
    fn roster_matches_the_compiled_objects_exported_symbols() {
        let mut roster_names: Vec<&str> = roster().iter().map(|e| e.full_path.as_str()).collect();
        roster_names.sort_unstable();
        assert_eq!(roster_names.len(), 11);

        let mut object_names: Vec<&str> = object()
            .symbols
            .iter()
            .filter(|s| matches!(s.def, SymbolDef::Defined { .. }))
            .map(|s| s.name.as_str())
            .collect();
        object_names.sort_unstable();

        assert_eq!(roster_names, object_names);
    }

    /// Each entry's `name_span` lands exactly on the routine name text in
    /// `SOURCE` — slice the declaration line by the span (char-counted,
    /// per `Span`'s "columns count characters" contract) and compare to
    /// the last `::`-segment of `full_path`.
    #[test]
    fn each_name_span_slices_out_the_routine_name() {
        assert_eq!(roster().len(), 11, "would be vacuous over an empty roster");
        for entry in roster() {
            let line_ix = (entry.name_span.start.line - 1) as usize;
            let line = SOURCE.lines().nth(line_ix).expect("span line exists");
            let chars: Vec<char> = line.chars().collect();
            let start = (entry.name_span.start.col - 1) as usize;
            let end = (entry.name_span.end.col - 1) as usize;
            let sliced: String = chars[start..end].iter().collect();
            let expected = entry
                .full_path
                .rsplit("::")
                .next()
                .expect("full_path has a segment");
            assert_eq!(sliced, expected, "entry {:?}", entry.full_path);
            assert_eq!(entry.name_span.start.line, entry.decl_line);
        }
    }

    /// ASCII guard: every declaration line a `name_span` sits on is pure
    /// ASCII. This is load-bearing for navigation, not cosmetic: the LSP
    /// framework converts an external `DefTarget`'s span (no open
    /// document to convert against) via the char==UTF-16 identity — exact
    /// only when the target's line is ASCII up to the span
    /// (`mtc_core::lsp::DefTarget`'s documented contract). This test is
    /// what makes that fallback conversion exact for every std
    /// go-to-definition target.
    #[test]
    fn every_roster_declaration_line_is_ascii() {
        assert_eq!(roster().len(), 11, "would be vacuous over an empty roster");
        for entry in roster() {
            let line_ix = (entry.name_span.start.line - 1) as usize;
            let line = SOURCE.lines().nth(line_ix).expect("span line exists");
            assert!(line.is_ascii(), "non-ASCII stdlib decl line: {line:?}");
        }
    }

    /// Materializer round-trip: `materialize_into` on a fresh tempdir
    /// creates `<root>/pmt/<version>/std.pmc` with `SOURCE`'s exact bytes
    /// and returns a `file:` URI.
    #[test]
    fn materialize_into_writes_source_and_returns_a_file_uri() {
        let root = unique_tmp_dir("write");
        let uri = materialize_into(&root).expect("materializes");
        assert!(uri.starts_with("file://"), "uri: {uri}");

        let file = root
            .join("pmt")
            .join(env!("CARGO_PKG_VERSION"))
            .join("std.pmc");
        assert!(file.exists());
        assert_eq!(fs::read(&file).unwrap(), SOURCE.as_bytes());

        let _ = fs::remove_dir_all(&root);
    }

    /// Self-heal: a corrupted (or stale) existing cache file is
    /// overwritten with `SOURCE`'s exact bytes on the next materialize.
    #[test]
    fn materialize_into_rewrites_a_corrupted_cache_file() {
        let root = unique_tmp_dir("heal");
        let dir = root.join("pmt").join(env!("CARGO_PKG_VERSION"));
        fs::create_dir_all(&dir).unwrap();
        let file = dir.join("std.pmc");
        fs::write(&file, b"not the stdlib").unwrap();

        let uri = materialize_into(&root).expect("materializes");
        assert!(uri.starts_with("file://"), "uri: {uri}");
        assert_eq!(fs::read(&file).unwrap(), SOURCE.as_bytes());

        let _ = fs::remove_dir_all(&root);
    }
}
