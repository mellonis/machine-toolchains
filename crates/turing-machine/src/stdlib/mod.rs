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
//!
//! [`roster`] and [`materialized_std_uri`] below serve the LSP's
//! go-to-definition on `std::` calls (docs/lsp.md (materialized standard
//! library)): the roster locates each exported routine's name token in
//! `SOURCE`, and the materializer writes `SOURCE` to a real file on disk
//! once per toolchain version so an editor has something to open. [`docs`]
//! serves hover (docs/lsp.md (hover)): the embedded stdlib's own resolved
//! doc map, keyed the same fully-qualified way `roster`'s `full_path` is
//! and covering every declaration `SOURCE` documents — routines, graphs,
//! and alphabets, three of the kinds a doc run may attach to
//! (docs/tmt/language.md (doc lines and attention lines)) — not just
//! `roster`'s linkable routines, since a requesting document's own
//! analysis never contains std entries (`std::` references are external to
//! it).

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use mtc_core::diagnostics::Span;
use mtc_core::formats::object::ObjectFile;

use crate::compiler::{CompileOptions, WorldKind, analyze_staged, compile};
use crate::optimizer::OptLevel;
use crate::parser::Doc;

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

/// One exported std routine, as declared in `SOURCE` (docs/lsp.md
/// (materialized standard library)) — the go-to-definition target for a
/// `std::<name>` call site.
// consumer: the .tmc language service's navigation and completion surfaces,
// wired in separately from this module.
pub(crate) struct RosterEntry {
    /// The fully-qualified `ns::name` form (`crate::compiler::full_name`),
    /// e.g. `std::binaryNumbers::goToNumber`.
    pub full_path: String,
    /// Span of the routine name token alone, in `SOURCE`.
    pub name_span: Span,
}

/// Runs the embedded stdlib through [`analyze_staged`] — the SAME staged
/// resolve path every requesting document goes through — once per process,
/// and keeps both projections the language service needs: the navigable
/// routine roster and the full hover doc map.
///
/// One `OnceLock` holding both, unlike the `.pmc` sibling's four
/// independent ones: there, the roster is derived from a cheap CST walk
/// that sidesteps the compile pipeline entirely (a function's `name_span`
/// is available straight off the parse tree), so roster and docs are two
/// unrelated cheap computations worth caching separately. Here a world's
/// `name_span` only exists on the *resolved* module (`ResolvedWorld`,
/// namespaces flattened into `ns::name`), which is also where `docs` comes
/// from — one pass already produces both, so a second `OnceLock` would
/// just be caching a field projection.
// consumer: roster() and docs() below.
fn analysis() -> &'static (Vec<RosterEntry>, HashMap<String, Doc>) {
    static ANALYSIS: OnceLock<(Vec<RosterEntry>, HashMap<String, Doc>)> = OnceLock::new();
    ANALYSIS.get_or_init(|| {
        let resolved = analyze_staged(SOURCE)
            .resolved
            .expect("the embedded stdlib always resolves");
        let roster = resolved
            .worlds
            .iter()
            .filter(|world| matches!(world.kind, WorldKind::Routine) && world.exported)
            .map(|world| RosterEntry {
                full_path: world.name.clone(),
                name_span: world.name_span,
            })
            .collect();
        (roster, resolved.docs)
    })
}

/// The stdlib's exported routines — the linkable `std::` symbols. Graphs
/// and alphabets are documented (see [`docs`]) but contribute no linkable
/// symbol, so they are not roster entries: a graph is spliced into whoever
/// grafts it, and a cross-unit graft is a compile error.
// consumer: the .tmc language service's navigation and completion surfaces,
// wired in separately from this module.
pub(crate) fn roster() -> &'static [RosterEntry] {
    &analysis().0
}

/// The embedded stdlib's resolved doc map (docs/lsp.md (hover)), keyed by
/// the same fully-qualified `ns::name` form [`roster`] uses. Covers every
/// documented top-level entity — routines, graphs, and alphabets — not
/// just the [`roster`]'s 14 linkable routines, so hovering a `std::` graph
/// or alphabet reference still has something to say even though neither
/// can ever be a go-to-definition target of its own.
// consumer: the .tmc language service's hover surface, wired in separately
// from this module.
pub(crate) fn docs() -> &'static HashMap<String, Doc> {
    &analysis().1
}

/// The cache directory root: `$XDG_CACHE_HOME` falling back to
/// `~/.cache` on unix, `%LOCALAPPDATA%` on windows. `None` if the
/// relevant environment variable(s) are unset — the materializer
/// degrades to `None` rather than guessing a location.
// consumer: materialized_std_uri() below.
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
// consumer: path_to_file_uri() below.
fn is_uri_literal(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~' | b'/' | b':')
}

/// Builds a `file:` URI for an absolute path: forward slashes,
/// percent-encoding every byte outside [`is_uri_literal`]. On windows,
/// prefixes `file:///C:/...` (the extra `/` before the drive letter).
/// Infallible — built entirely on `Path::to_string_lossy`, which never
/// fails. `pub(crate)` rather than private: a general path-to-URI encoder,
/// not stdlib-specific, so a future cross-file consumer (docs/lsp.md
/// (configuration)) can reuse it instead of hand-rolling a second one.
// consumer: materialize_into() below, plus any future caller needing the
// same path->URI encoding.
pub(crate) fn path_to_file_uri(path: &Path) -> String {
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

/// Writes `SOURCE` to `<root>/tmt/<CARGO_PKG_VERSION>/std.tmc` if the
/// file is absent or its bytes differ from `SOURCE` (self-heals a
/// corrupted or stale cache file), then returns its `file:` URI. Any IO
/// failure degrades to `None` (docs/lsp.md (materialized standard
/// library)).
// consumer: materialized_std_uri() below.
fn materialize_into(root: &Path) -> Option<String> {
    let dir = root.join("tmt").join(env!("CARGO_PKG_VERSION"));
    fs::create_dir_all(&dir).ok()?;
    let file = dir.join("std.tmc");
    let needs_write = match fs::read(&file) {
        Ok(existing) => existing != SOURCE.as_bytes(),
        Err(_) => true,
    };
    if needs_write {
        fs::write(&file, SOURCE).ok()?;
    }
    Some(path_to_file_uri(&file))
}

/// The embedded `std.tmc`, written once per toolchain version to
/// `<cache>/tmt/<version>/std.tmc`, as a `file:` URI (docs/lsp.md
/// (materialized standard library)). `None` if the cache root can't be
/// located or any IO step fails — go-to-definition on `std::` calls then
/// degrades to null rather than pointing at a file that doesn't exist.
// consumer: the .tmc language service's go-to-definition surface, wired in
// separately from this module.
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
            "tmt-stdlib-roster-test-{label}-{}-{n}",
            std::process::id()
        ))
    }

    /// Hand-rolled percent-decoding, mirroring [`path_to_file_uri`]'s own
    /// encoding scheme (there is no `pub(crate)` decoder to reuse — the
    /// LSP module owns one but it is private and out of scope for this
    /// task). `%XX` hex pairs become bytes; anything else passes through.
    fn decode_percent(s: &str) -> String {
        let bytes = s.as_bytes();
        let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i] == b'%' && i + 2 < bytes.len() {
                let hi = (bytes[i + 1] as char).to_digit(16);
                let lo = (bytes[i + 2] as char).to_digit(16);
                if let (Some(hi), Some(lo)) = (hi, lo) {
                    out.push((hi * 16 + lo) as u8);
                    i += 3;
                    continue;
                }
            }
            out.push(bytes[i]);
            i += 1;
        }
        String::from_utf8(out).expect("uri path is valid utf8")
    }

    /// Parses a `file:` URI (as produced by [`path_to_file_uri`]) back to a
    /// filesystem path.
    fn uri_to_path(uri: &str) -> PathBuf {
        let rest = uri.strip_prefix("file://").expect("a file: uri");
        PathBuf::from(decode_percent(rest))
    }

    /// Drift guard: the roster's full paths are exactly the fourteen
    /// exported routines the stdlib declares — ten in `binaryNumbers`, four
    /// in `binaryNumbersBare`. Also spot-checks that one entry's
    /// `name_span` slices out exactly its routine name in `SOURCE`.
    #[test]
    fn roster_is_the_fourteen_exported_routines() {
        let mut roster_paths: Vec<&str> = roster().iter().map(|e| e.full_path.as_str()).collect();
        roster_paths.sort_unstable();

        let mut expected = vec![
            "std::binaryNumbers::goToNumber",
            "std::binaryNumbers::goToNumbersStart",
            "std::binaryNumbers::goToNextNumber",
            "std::binaryNumbers::goToPreviousNumber",
            "std::binaryNumbers::deleteNumber",
            "std::binaryNumbers::normalizeNumber",
            "std::binaryNumbers::plusOne",
            "std::binaryNumbers::minusOneFast",
            "std::binaryNumbers::invertNumber",
            "std::binaryNumbers::minusOne",
            "std::binaryNumbersBare::plusOne",
            "std::binaryNumbersBare::minusOne",
            "std::binaryNumbersBare::invertNumber",
            "std::binaryNumbersBare::normalizeNumber",
        ];
        expected.sort_unstable();
        assert_eq!(roster_paths, expected);

        // Also matches the object's exported symbol set — same drift-guard
        // teeth the old CST-walk helper had, now checked against roster().
        let mut object_names: Vec<&str> = object()
            .symbols
            .iter()
            .filter(|s| matches!(s.def, SymbolDef::Defined { .. }))
            .map(|s| s.name.as_str())
            .collect();
        object_names.sort_unstable();
        assert_eq!(roster_paths, object_names);

        let entry = roster()
            .iter()
            .find(|e| e.full_path == "std::binaryNumbers::goToNumber")
            .expect("goToNumber is in the roster");
        let line_ix = (entry.name_span.start.line - 1) as usize;
        let line = SOURCE.lines().nth(line_ix).expect("span line exists");
        let chars: Vec<char> = line.chars().collect();
        let start = (entry.name_span.start.col - 1) as usize;
        let end = (entry.name_span.end.col - 1) as usize;
        let sliced: String = chars[start..end].iter().collect();
        assert_eq!(sliced, "goToNumber");
    }

    /// The doc map is the hover surface, not the roster's linkable-symbol
    /// view: it must cover all three documented entity kinds — a routine,
    /// a graph, and an alphabet — not just the routines the roster links.
    /// Asserting only a routine here would still pass if `docs()` were
    /// truncated to the roster's 14 entries, so all three are checked.
    #[test]
    fn docs_cover_routines_graphs_and_alphabets() {
        let map = docs();

        let routine = map
            .get("std::binaryNumbers::goToNumber")
            .expect("routine doc present");
        assert!(!routine.paragraphs.is_empty(), "routine doc has paragraphs");

        assert!(
            map.contains_key("std::binaryNumbers::symbols"),
            "alphabet doc present"
        );
        assert!(
            map.contains_key("std::binaryNumbers::plusOneGraph"),
            "graph doc present"
        );

        // Every documented entity in std.tmc: 10 routines + 8 graphs + 1
        // alphabet in binaryNumbers, 4 routines + 4 graphs + 1 alphabet in
        // binaryNumbersBare.
        assert_eq!(map.len(), 28, "all documented std.tmc entities");
    }

    /// ASCII guard: every declaration line a `name_span` sits on is pure
    /// ASCII. This is load-bearing for navigation, not cosmetic: the LSP
    /// framework converts an external `DefTarget`'s span (no open document
    /// to convert against) via the char==UTF-16 identity — exact only when
    /// the target's line is ASCII up to the span
    /// (`mtc_core::lsp::DefTarget`'s documented contract). This test is
    /// what makes that fallback conversion exact for every std
    /// go-to-definition target.
    #[test]
    fn every_roster_declaration_line_is_ascii() {
        assert_eq!(roster().len(), 14, "would be vacuous over an empty roster");
        for entry in roster() {
            let line_ix = (entry.name_span.start.line - 1) as usize;
            let line = SOURCE.lines().nth(line_ix).expect("span line exists");
            assert!(line.is_ascii(), "non-ASCII stdlib decl line: {line:?}");
        }
    }

    /// Materialization round-trip: `materialized_std_uri()` (the real
    /// cache-root path, not a scratch `materialize_into` call) parses back
    /// to a path whose bytes are exactly `SOURCE`.
    #[test]
    fn materialized_std_uri_points_at_a_byte_identical_std_tmc() {
        let uri = materialized_std_uri().expect("materialization succeeds in this env");
        assert!(uri.starts_with("file://"), "uri: {uri}");

        let path = uri_to_path(uri);
        assert_eq!(fs::read(&path).unwrap(), SOURCE.as_bytes());
    }

    /// Materializer round-trip on a scratch root: `materialize_into`
    /// creates `<root>/tmt/<version>/std.tmc` with `SOURCE`'s exact bytes
    /// and returns a `file:` URI.
    #[test]
    fn materialize_into_writes_source_and_returns_a_file_uri() {
        let root = unique_tmp_dir("write");
        let uri = materialize_into(&root).expect("materializes");
        assert!(uri.starts_with("file://"), "uri: {uri}");

        let file = root
            .join("tmt")
            .join(env!("CARGO_PKG_VERSION"))
            .join("std.tmc");
        assert!(file.exists());
        assert_eq!(fs::read(&file).unwrap(), SOURCE.as_bytes());

        let _ = fs::remove_dir_all(&root);
    }

    /// Self-heal: a corrupted (or stale) existing cache file is
    /// overwritten with `SOURCE`'s exact bytes on the next materialize.
    #[test]
    fn materialize_into_rewrites_a_corrupted_cache_file() {
        let root = unique_tmp_dir("heal");
        let dir = root.join("tmt").join(env!("CARGO_PKG_VERSION"));
        fs::create_dir_all(&dir).unwrap();
        let file = dir.join("std.tmc");
        fs::write(&file, b"not the stdlib").unwrap();

        let uri = materialize_into(&root).expect("materializes");
        assert!(uri.starts_with("file://"), "uri: {uri}");
        assert_eq!(fs::read(&file).unwrap(), SOURCE.as_bytes());

        let _ = fs::remove_dir_all(&root);
    }
}
