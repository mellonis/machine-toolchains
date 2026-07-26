//! Cross-file project view for the LSP: given an open document, finds its
//! nearest manifest-bearing `pmt.json`, decides whether the document is a
//! member of any build target, and produces the union of sibling source
//! files it links with plus the resolved on-disk paths of its declared
//! libraries (docs/pmt/project.md (discovery), (the declared source set),
//! (schema reference)). The second half of the module turns that file set
//! into an indexed [`Overlay`]: each sibling's exported symbols, extracted
//! per source kind (`.pmc` exports, `.pma` non-local funcs, `.pmo` defined
//! symbols) and merged first-wins (sources before libraries, each in
//! `ProjectView`'s own order) — the table `did_update` stores on every
//! open document for cross-file completion, navigation, hover, and
//! diagnostic refinement (docs/lsp.md (configuration) for the shared
//! mtime-cache discipline both halves of this module use).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Component, Path, PathBuf};
use std::time::SystemTime;

use mtc_core::asm::cst::{AsmCst, AsmItemKind, parse_asm_cst_with};
use mtc_core::diagnostics::Span;
use mtc_core::formats::object::{ObjectFile, SymbolDef};

use crate::compiler::{Analysis, analyze_staged};
use crate::parser::FnDoc;
use crate::project::{Libraries, Manifest, load_file, normalize_rel};
use crate::stdlib::path_to_file_uri;

use super::DocState;

/// One open document's project membership view: the manifest directory,
/// the `stdlib` flag, the sibling source files it links with (self
/// excluded, union across every target the document is a member of, in
/// target order), and the resolved paths of its declared libraries.
#[derive(Debug, PartialEq)]
pub(super) struct ProjectView {
    pub root: PathBuf,
    pub stdlib: bool,
    pub siblings: Vec<PathBuf>,
    pub library_paths: Vec<PathBuf>,
}

/// `pmt.json` parse cache keyed by candidate path: `(mtime, outcome)`.
/// Mirrors `ConfigResolver::project_allow`'s discipline (docs/lsp.md
/// (configuration)) applied to the project-section manifest instead of
/// the lint allow-list: stat first; no stat means no cache entry at all;
/// eviction happens only when inserting a new key at capacity, and is
/// arbitrary (not LRU) — a miss only costs a re-parse, never a wrong
/// answer.
pub(super) type ManifestCache = HashMap<PathBuf, (SystemTime, Result<Option<Manifest>, String>)>;

/// Bounds `ManifestCache`'s growth for the same reason as
/// `CONFIG_CACHE_LIMIT` (docs/lsp.md (configuration)): a long-running server
/// visiting many project roots over its lifetime must not grow this map
/// forever, and an arbitrary eviction at capacity can only turn a hit
/// into a miss.
const MANIFEST_CACHE_LIMIT: usize = 32;

/// The project-file channel for one `pmt.json` candidate: the parsed
/// outcome through the mtime cache, reused only while the file's mtime is
/// unchanged, else re-loaded and re-cached. `Ok(None)` means the file is
/// valid but lint-only (no `project` section) — transparent to the
/// caller's walk; `Err` carries the load error's display string.
fn cached_manifest(path: &Path, cache: &mut ManifestCache) -> Result<Option<Manifest>, String> {
    let mtime = std::fs::metadata(path).and_then(|m| m.modified()).ok();
    if let Some(mtime) = mtime
        && let Some((cached, outcome)) = cache.get(path)
        && *cached == mtime
    {
        return outcome.clone();
    }
    let outcome = load_file(path)
        .map(|file| file.manifest)
        .map_err(|e| e.to_string());
    if let Some(mtime) = mtime {
        // No stat (file racing in and out of existence) → no cache entry:
        // there is no mtime to key staleness on.
        if !cache.contains_key(path)
            && cache.len() >= MANIFEST_CACHE_LIMIT
            && let Some(evict) = cache.keys().next().cloned()
        {
            cache.remove(&evict);
        }
        cache.insert(path.to_path_buf(), (mtime, outcome.clone()));
    }
    outcome
}

/// Resolves a manifest-relative path against the manifest's own directory
/// into a genuinely lexical absolute path. `normalize_rel` only folds
/// `..` interior to the raw STRING and deliberately keeps a leading `..`
/// (the validator doesn't yet know what sits above the manifest —
/// docs/pmt/project.md (path rules)); once the manifest's directory is
/// known here, a leading `..` must be folded against it too, or a source
/// declared as `../shared.pmc` would resolve to `<root>/../shared.pmc`
/// instead of `<root>`'s own sibling `shared.pmc` — the two are NOT the
/// same `PathBuf` even though they name the same file, and this module's
/// whole membership test is a `PathBuf` comparison. A leading `..` that
/// would pop past the root's own last component is left in place rather
/// than eating into `root`'s ancestry (mirrors `/..` staying `/`).
fn resolve(root: &Path, raw: &str) -> Option<PathBuf> {
    let rel = normalize_rel(raw).ok()?;
    let mut parts: Vec<Component> = root.components().collect();
    for comp in rel.components() {
        match comp {
            Component::ParentDir => {
                if matches!(parts.last(), Some(Component::Normal(_))) {
                    parts.pop();
                }
            }
            Component::Normal(_) => parts.push(comp),
            _ => {} // `normalize_rel` already stripped `.`; `rel` is relative.
        }
    }
    Some(parts.into_iter().collect())
}

/// Computes one document's [`ProjectView`], or `None` when it degrades to
/// single-file behavior: no manifest found on the ancestor walk, the
/// document is listed in no target, or a candidate on the walk is
/// malformed.
///
/// The ancestor walk mirrors `project::discover_manifest` exactly: the
/// nearest `pmt.json` WITH a `project` section wins; a lint-only
/// candidate is transparent and the walk continues past it; a malformed
/// candidate ENDS the walk with `None` rather than being skipped — we
/// cannot know whether it would have had a `project` section, and its
/// parse error already reaches the user through the existing
/// invalid-config diagnostic channel (docs/pmt/project.md (discovery)).
pub(super) fn project_view(doc_path: &Path, cache: &mut ManifestCache) -> Option<ProjectView> {
    let start = doc_path.parent()?;
    let abs = std::path::absolute(start).ok()?;
    let mut dir = Some(abs.as_path());
    let (root, manifest) = loop {
        let d = dir?;
        let candidate = d.join("pmt.json");
        if candidate.is_file() {
            match cached_manifest(&candidate, cache) {
                Err(_) => return None,
                Ok(Some(m)) => break (d.to_path_buf(), m),
                Ok(None) => {} // lint-only: transparent to this walk.
            }
        }
        dir = d.parent();
    };

    // Membership + union: BTreeMap target order, effective order within a
    // target, first-seen dedup, self excluded.
    let doc_abs = std::path::absolute(doc_path).ok()?;
    let mut siblings: Vec<PathBuf> = Vec::new();
    let mut lib = Libraries::default();
    let mut member = false;
    for target in manifest.targets.values() {
        let sources: Vec<PathBuf> = manifest
            .effective_sources(target)
            .iter()
            .filter_map(|raw| resolve(&root, raw))
            .collect();
        if !sources.contains(&doc_abs) {
            continue;
        }
        member = true;
        for p in sources {
            if p != doc_abs && !siblings.contains(&p) {
                siblings.push(p);
            }
        }
        let l = manifest.effective_libraries(target);
        for d in &l.dirs {
            if !lib.dirs.contains(d) {
                lib.dirs.push(d.clone());
            }
        }
        for n in &l.link {
            if !lib.link.contains(n) {
                lib.link.push(n.clone());
            }
        }
    }
    if !member {
        return None;
    }

    // Declared library resolution: first dir wins (mirrors
    // `cli::build::find_library`'s own search order — docs/pmt/project.md
    // (schema reference)); a library missing from every dir contributes
    // nothing.
    let dirs: Vec<PathBuf> = lib.dirs.iter().filter_map(|d| resolve(&root, d)).collect();
    let library_paths: Vec<PathBuf> = lib
        .link
        .iter()
        .filter_map(|name| {
            dirs.iter()
                .map(|d| d.join(format!("{name}.pmo")))
                .find(|p| p.is_file())
        })
        .collect();

    Some(ProjectView {
        root,
        stdlib: manifest.stdlib,
        siblings,
        library_paths,
    })
}

/// One sibling's exported symbol: its full (mangled `.pmc`, or bare
/// `.pma`/`.pmo`) name, the span of its own declaration name (`None` for
/// `.pmo`, which carries no source location at all), and its doc comment
/// (`.pmc` only — `.pma`/`.pmo` carry no doc surface).
#[derive(Debug, Clone, PartialEq)]
pub(super) struct ExportedSym {
    pub name: String,
    pub span: Option<Span>,
    pub doc: Option<FnDoc>,
}

/// One sibling file's export list, keyed by its resolved path:
/// `(mtime, exports)`. Mirrors `ManifestCache`'s discipline: stat first;
/// no stat means no cache entry at all; eviction happens only when
/// inserting a new key at capacity, and is arbitrary (not LRU) — a miss
/// only costs a re-scan, never a wrong answer. Never consulted for a
/// `.pmc` sibling that is currently OPEN in this service — that answer
/// comes from the live `DocState` instead (docs/lsp.md (configuration)).
pub(super) type SiblingCache = HashMap<PathBuf, (SystemTime, Vec<ExportedSym>)>;

/// Bounds `SiblingCache`'s growth for the same reason as
/// `MANIFEST_CACHE_LIMIT`: a long-running server whose open documents
/// collectively touch many sibling files over its lifetime must not grow
/// this map forever, and an arbitrary eviction at capacity can only turn
/// a hit into a miss.
const SIBLING_CACHE_LIMIT: usize = 64;

/// A `.pmc` sibling's exported functions only (docs/pmt/language.md
/// (visibility)): `Function.exported` is already `false` for every nested
/// function by the time `flatten` is done with it (a nested function can
/// never itself carry `export`, and flatten additionally clears the flag
/// on anything it emits from inside a parent — `compiler.rs`'s `emit`),
/// so filtering `analysis.ast.functions` by `exported` alone is exactly
/// the exported set, with no separate nested-function exclusion needed.
/// An un-namespaced top-level `main` is auto-exported by the parser, so
/// it is naturally included here too. `name` is already the fully
/// mangled/`::`-qualified form `Analysis.docs` is keyed by.
fn exports_from_pmc(analysis: &Analysis) -> Vec<ExportedSym> {
    analysis
        .ast
        .functions
        .iter()
        .filter(|f| f.exported)
        .map(|f| ExportedSym {
            name: f.name.clone(),
            span: Some(f.name_span),
            doc: analysis.docs.get(&f.name).cloned(),
        })
        .collect()
}

/// A `.pma` sibling's non-`local` `.func` declarations (docs/formats.md
/// (assembly text) — "Visibility and names": `.func name local` is
/// unexported, plain `.func name` exports): the CST is total (every
/// text parses — `lower.rs`'s validity checking is a separate, later
/// stage this never runs), so there is no fatal case to handle here at
/// all. `.pma` carries no doc surface, so `doc` is always `None`.
fn exports_from_pma(cst: &AsmCst) -> Vec<ExportedSym> {
    cst.items
        .iter()
        .filter_map(|i| match &i.kind {
            AsmItemKind::Func(f) if !f.local => Some(ExportedSym {
                name: f.name.clone(),
                span: Some(f.name_span),
                doc: None,
            }),
            _ => None,
        })
        .collect()
}

/// A `.pmo` object's exported symbols (docs/formats.md (.pmo)):
/// `SymbolDef::Defined` only — `Local` is bound within its own object and
/// never enters the linker's namespace (docs/core.md (linking)), so
/// including it here would let the editor resolve names the linker
/// itself could never reach. Neither a span nor a doc exists at the
/// object-file tier. A malformed object contributes nothing rather than
/// aborting the whole overlay build.
fn exports_from_object(bytes: &[u8]) -> Vec<ExportedSym> {
    let Ok(obj) = ObjectFile::from_bytes(bytes) else {
        return Vec::new();
    };
    obj.symbols
        .iter()
        .filter(|s| matches!(s.def, SymbolDef::Defined { .. }))
        .map(|s| ExportedSym {
            name: s.name.clone(),
            span: None,
            doc: None,
        })
        .collect()
}

/// One sibling path's export list, dispatched by extension, through the
/// mtime cache (same discipline as `cached_manifest`). A `.pmc` sibling
/// that is OPEN in this service (its `file:` URI — `crate::stdlib::
/// path_to_file_uri` — is a key of `open_docs`) is read from its live
/// `DocState` instead, bypassing disk and this cache entirely: that
/// document's `analysis` is already current for the text the editor
/// actually holds, so a disk re-read would both waste work and risk
/// missing unsaved edits. Every other path (an unopened `.pmc`, any
/// `.pma`, any `.pmo`) is read from disk and cached by mtime. A sibling
/// that fails to read, fails to parse, or (`.pmc`) fatals during analysis
/// contributes an EMPTY export list rather than aborting the whole
/// build — the entire reason this design runs a lightweight per-file
/// scan instead of a real link.
fn cached_sibling_exports(
    path: &Path,
    open_docs: &HashMap<String, DocState>,
    cache: &mut SiblingCache,
) -> Vec<ExportedSym> {
    let ext = path.extension().and_then(|e| e.to_str());

    if ext == Some("pmc") {
        let uri = path_to_file_uri(path);
        if let Some(doc) = open_docs.get(&uri) {
            return match &doc.analysis {
                Some(analysis) => exports_from_pmc(analysis),
                None => Vec::new(),
            };
        }
    }

    let mtime = std::fs::metadata(path).and_then(|m| m.modified()).ok();
    if let Some(mtime) = mtime
        && let Some((cached, exports)) = cache.get(path)
        && *cached == mtime
    {
        return exports.clone();
    }

    let exports = match ext {
        Some("pmc") => std::fs::read_to_string(path)
            .ok()
            .and_then(|text| analyze_staged(&text).analysis)
            .map(|analysis| exports_from_pmc(&analysis))
            .unwrap_or_default(),
        Some("pma") => std::fs::read_to_string(path)
            .ok()
            .map(|text| exports_from_pma(&parse_asm_cst_with(&text, crate::asm::pm1_syntax().caps)))
            .unwrap_or_default(),
        Some("pmo") => std::fs::read(path)
            .ok()
            .map(|bytes| exports_from_object(&bytes))
            .unwrap_or_default(),
        _ => Vec::new(),
    };

    if let Some(mtime) = mtime {
        // No stat (file racing in and out of existence) → no cache entry:
        // there is no mtime to key staleness on.
        if !cache.contains_key(path)
            && cache.len() >= SIBLING_CACHE_LIMIT
            && let Some(evict) = cache.keys().next().cloned()
        {
            cache.remove(&evict);
        }
        cache.insert(path.to_path_buf(), (mtime, exports.clone()));
    }
    exports
}

/// One resolved overlay entry: `target` is `(uri, name_span)` pointing at
/// the contributing sibling's own declaration; `None` means a name-only
/// answer (`.pmo`, direct source or via a linked library) — there is
/// nowhere to navigate to, only a name to resolve calls against. `doc`
/// carries the contributing `.pmc` sibling's own `FnDoc`, when there is
/// one.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct OverlaySym {
    pub target: Option<(String, Span)>,
    pub doc: Option<FnDoc>,
}

/// One open document's cross-file symbol table: every name its siblings
/// and libraries export, first-wins (`R13` — sources before libraries,
/// each in `ProjectView`'s own order, mirroring the linker's own
/// user-objects-beat-libraries / first-dir-wins precedent), plus a
/// `members` index for namespace-qualified lookups (a bare name under
/// its namespace path; top-level exports live under the empty path) and
/// the project's own `stdlib` flag.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Overlay {
    pub stdlib: bool,
    pub symbols: HashMap<String, OverlaySym>,
    pub members: HashMap<Vec<String>, BTreeMap<String, String>>,
}

impl Overlay {
    /// Every name this overlay defines for cross-file call resolution —
    /// the exact mirror of the driver's own `defined_names`
    /// (`cli/driver.rs`) over one build's declared set: this document's
    /// siblings' and libraries' exported names, unioned with the
    /// embedded stdlib's roster of full `std::<name>` paths when the
    /// project's `stdlib` flag is set (a bare undeclared call can never
    /// resolve to a `std::` name, so the roster contributes only its
    /// full, namespaced paths — exactly what the driver's own union
    /// does).
    pub(super) fn defined_names(&self) -> HashSet<String> {
        let mut names: HashSet<String> = self.symbols.keys().cloned().collect();
        if self.stdlib {
            names.extend(crate::stdlib::roster().iter().map(|e| e.full_path.clone()));
        }
        names
    }
}

/// Inserts one contributed export into `symbols` (first wins) and
/// registers it into `members` regardless of whether it won: the
/// (namespace path, bare name) pair `members` keys on is a pure,
/// reversible function of the full name alone (never of WHICH sibling
/// contributed it), so a losing contributor's registration is always
/// identical to the winner's, never a conflicting overwrite. `uri` is
/// `None` for a library contribution (never a "document" with a URI of
/// its own) and for any `.pmo` sibling — `sym.span` is already `None` in
/// that case, so `target` comes out `None` either way.
fn insert_export(
    symbols: &mut HashMap<String, OverlaySym>,
    members: &mut HashMap<Vec<String>, BTreeMap<String, String>>,
    sym: ExportedSym,
    uri: Option<&str>,
) {
    let target = match (uri, sym.span) {
        (Some(uri), Some(span)) => Some((uri.to_string(), span)),
        _ => None,
    };
    symbols.entry(sym.name.clone()).or_insert(OverlaySym {
        target,
        doc: sym.doc,
    });

    let mut segments: Vec<&str> = sym.name.split("::").collect();
    let bare = segments.pop().unwrap_or(sym.name.as_str());
    let ns_path: Vec<String> = segments.iter().map(|s| s.to_string()).collect();
    members
        .entry(ns_path)
        .or_default()
        .insert(bare.to_string(), sym.name.clone());
}

/// Builds one open document's [`Overlay`]: `view.siblings` first (each in
/// `ProjectView`'s own order — union across every target the document is
/// a member of), then `view.library_paths`, first-wins throughout. `doc_path`
/// is the document the overlay is being built FOR; `view.siblings` never
/// contains it (`project_view`'s own self-exclusion), and the guard below
/// is a genuine second line of defense: it re-derives the SAME
/// `std::path::absolute` comparison `project_view` itself used to exclude
/// `doc_path` from `siblings` (each already an absolute path, built by
/// `resolve`), rather than comparing `doc_path` in whatever raw form the
/// caller happened to pass in — a raw-path comparison would silently stop
/// catching anything the moment `doc_path`'s form ever diverged from what
/// `siblings` compare against.
pub(super) fn build_overlay(
    view: &ProjectView,
    doc_path: &Path,
    open_docs: &HashMap<String, DocState>,
    cache: &mut SiblingCache,
) -> Overlay {
    let mut symbols: HashMap<String, OverlaySym> = HashMap::new();
    let mut members: HashMap<Vec<String>, BTreeMap<String, String>> = HashMap::new();
    let doc_abs = std::path::absolute(doc_path).ok();

    for sibling in &view.siblings {
        if doc_abs.as_deref() == Some(sibling.as_path()) {
            continue;
        }
        let uri = path_to_file_uri(sibling);
        for sym in cached_sibling_exports(sibling, open_docs, cache) {
            insert_export(&mut symbols, &mut members, sym, Some(&uri));
        }
    }

    for lib in &view.library_paths {
        for sym in cached_sibling_exports(lib, open_docs, cache) {
            insert_export(&mut symbols, &mut members, sym, None);
        }
    }

    Overlay {
        stdlib: view.stdlib,
        symbols,
        members,
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::time::Duration;

    use mtc_core::lsp::LanguageService;

    use super::*;

    /// A fresh scratch directory under `std::env::temp_dir()`, unique per
    /// call (process id + an atomic counter — this crate has no tempfile
    /// dependency, matching the zero-new-deps constraint; house
    /// convention has no shared test-support module, so each file defines
    /// its own local helper).
    fn temp_tree() -> PathBuf {
        static N: AtomicU32 = AtomicU32::new(0);
        let d = std::env::temp_dir().join(format!(
            "pmt-overlay-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn member_of_one_target_gets_that_targets_files() {
        let root = temp_tree();
        fs::write(
            root.join("pmt.json"),
            r#"{"project":{"sources":["shared.pmc"],"targets":{"app":{"sources":["app.pmc"]}}}}"#,
        )
        .unwrap();

        let mut cache = ManifestCache::new();
        let view =
            project_view(&root.join("app.pmc"), &mut cache).expect("app.pmc is a target member");

        assert_eq!(view.root, root);
        assert!(view.stdlib);
        assert_eq!(view.siblings, vec![root.join("shared.pmc")]);
    }

    #[test]
    fn member_of_two_targets_gets_the_union_in_target_order() {
        let root = temp_tree();
        fs::write(
            root.join("pmt.json"),
            r#"{"project":{"targets":{
                "a":{"sources":["x.pmc","common.pmc"]},
                "b":{"sources":["x.pmc","extra.pmc"]}
            }}}"#,
        )
        .unwrap();

        let mut cache = ManifestCache::new();
        let view =
            project_view(&root.join("x.pmc"), &mut cache).expect("x.pmc is a member of both");

        assert_eq!(
            view.siblings,
            vec![root.join("common.pmc"), root.join("extra.pmc")]
        );
    }

    #[test]
    fn non_member_and_no_manifest_yield_none() {
        let root = temp_tree();
        fs::write(
            root.join("pmt.json"),
            r#"{"project":{"targets":{"app":{"sources":["app.pmc"]}}}}"#,
        )
        .unwrap();

        let mut cache = ManifestCache::new();
        assert!(
            project_view(&root.join("other.pmc"), &mut cache).is_none(),
            "listed in no target"
        );

        let bare = temp_tree();
        assert!(
            project_view(&bare.join("x.pmc"), &mut cache).is_none(),
            "no pmt.json anywhere on the walk"
        );
    }

    #[test]
    fn lint_only_pmt_json_is_transparent_to_the_walk() {
        let root = temp_tree();
        let sub = root.join("sub");
        fs::create_dir_all(&sub).unwrap();
        fs::write(
            root.join("pmt.json"),
            r#"{"project":{"targets":{"app":{"sources":["sub/deep.pmc"]}}}}"#,
        )
        .unwrap();
        fs::write(
            sub.join("pmt.json"),
            r#"{"lint":{"allow":["unused-label"]}}"#,
        )
        .unwrap();

        let mut cache = ManifestCache::new();
        let view = project_view(&sub.join("deep.pmc"), &mut cache)
            .expect("the lint-only sub/pmt.json is transparent; the root manifest is found");
        assert_eq!(view.root, root);
    }

    #[test]
    fn malformed_manifest_on_the_walk_yields_none_not_a_nearer_hit() {
        let root = temp_tree();
        let sub = root.join("sub");
        fs::create_dir_all(&sub).unwrap();
        fs::write(
            root.join("pmt.json"),
            r#"{"project":{"targets":{"app":{"sources":["sub/x.pmc"]}}}}"#,
        )
        .unwrap();
        fs::write(sub.join("pmt.json"), "{").unwrap();

        let mut cache = ManifestCache::new();
        assert!(
            project_view(&sub.join("x.pmc"), &mut cache).is_none(),
            "the malformed candidate ends the walk instead of being skipped for the valid root"
        );
    }

    #[test]
    fn dotdot_membership_resolves_lexically() {
        let root = temp_tree();
        let proj = root.join("proj");
        fs::create_dir_all(&proj).unwrap();
        fs::write(
            proj.join("pmt.json"),
            r#"{"project":{"sources":["../shared.pmc"],"targets":{"app":{"sources":["app.pmc"]}}}}"#,
        )
        .unwrap();

        let mut cache = ManifestCache::new();
        assert!(
            project_view(&root.join("shared.pmc"), &mut cache).is_none(),
            "discovery starts at root's own directory; proj/pmt.json is not an ancestor of root"
        );

        let view = project_view(&proj.join("app.pmc"), &mut cache)
            .expect("app.pmc is a member via proj/pmt.json");
        assert_eq!(view.siblings, vec![root.join("shared.pmc")]);
    }

    #[test]
    fn manifest_cache_is_mtime_keyed_and_bounded() {
        let mut cache = ManifestCache::new();
        // More distinct manifest roots than the eviction bound, so an
        // unbounded cache would visibly outgrow it.
        for i in 0..(MANIFEST_CACHE_LIMIT + 8) {
            let dir = temp_tree();
            fs::write(
                dir.join("pmt.json"),
                format!(r#"{{"project":{{"targets":{{"app":{{"sources":["p{i}.pmc"]}}}}}}}}"#),
            )
            .unwrap();
            project_view(&dir.join(format!("p{i}.pmc")), &mut cache);
        }
        assert!(
            cache.len() <= MANIFEST_CACHE_LIMIT,
            "cache grew past its bound: {} entries",
            cache.len()
        );

        // A rewritten manifest with a bumped mtime must change the next
        // answer — observed through ProjectView itself (the document
        // falls out of its only target), not through cache internals.
        let dir = temp_tree();
        let manifest_path = dir.join("pmt.json");
        fs::write(
            &manifest_path,
            r#"{"project":{"targets":{"app":{"sources":["x.pmc"]}}}}"#,
        )
        .unwrap();
        let mut fresh_cache = ManifestCache::new();
        let doc = dir.join("x.pmc");
        assert!(
            project_view(&doc, &mut fresh_cache).is_some(),
            "x.pmc starts as a member"
        );

        // A guaranteed-newer mtime — the filesystem's own timestamp
        // granularity is not to be trusted in a fast test (mirrors
        // mod.rs's `rewritten_broken_config_surfaces_invalid_config…`).
        let old_mtime = fs::metadata(&manifest_path).unwrap().modified().unwrap();
        fs::write(
            &manifest_path,
            r#"{"project":{"targets":{"app":{"sources":["other.pmc"]}}}}"#,
        )
        .unwrap();
        fs::File::options()
            .write(true)
            .open(&manifest_path)
            .unwrap()
            .set_modified(old_mtime + Duration::from_secs(2))
            .unwrap();

        assert!(
            project_view(&doc, &mut fresh_cache).is_none(),
            "the bumped mtime must be re-read, not served from the stale cache"
        );
    }

    #[test]
    fn stdlib_false_is_carried() {
        let root = temp_tree();
        fs::write(
            root.join("pmt.json"),
            r#"{"project":{"stdlib":false,"targets":{"app":{"sources":["app.pmc"]}}}}"#,
        )
        .unwrap();

        let mut cache = ManifestCache::new();
        let view = project_view(&root.join("app.pmc"), &mut cache).unwrap();
        assert!(!view.stdlib);
    }

    #[test]
    fn declared_library_paths_resolve_first_wins_and_missing_are_skipped() {
        let root = temp_tree();
        fs::create_dir_all(root.join("libs")).unwrap();
        fs::create_dir_all(root.join("more")).unwrap();
        // A COMPETING `bit.pmo` in the second dir too — proves first-dir
        // precedence (`cli::build::find_library`'s own search order),
        // rather than merely "found in the only dir that has it": a
        // reversed dir-search order would resolve to `more/bit.pmo`
        // instead, which this asserts against.
        fs::write(root.join("libs").join("bit.pmo"), "libs").unwrap();
        fs::write(root.join("more").join("bit.pmo"), "more").unwrap();
        fs::write(
            root.join("pmt.json"),
            r#"{"project":{
                "libraries":{"dirs":["libs","more"],"link":["bit","ghost"]},
                "targets":{"app":{"sources":["app.pmc"]}}
            }}"#,
        )
        .unwrap();

        let mut cache = ManifestCache::new();
        let view = project_view(&root.join("app.pmc"), &mut cache).unwrap();
        assert_eq!(view.library_paths, vec![root.join("libs").join("bit.pmo")]);
    }

    // --- Task 4: sibling export extraction + the `Overlay` table ---

    #[test]
    fn pmc_sibling_contributes_exported_functions_only() {
        let root = temp_tree();
        let sibling = root.join("sibling.pmc");
        fs::write(
            &sibling,
            "?Top doc.\n\
             export top() { right; }\n\
             hidden() { right; }\n\
             namespace ns {\n\
             ?Inner doc.\n\
             export inner() { right; }\n\
             }\n\
             main() { right; }\n",
        )
        .unwrap();

        let view = ProjectView {
            root: root.clone(),
            stdlib: true,
            siblings: vec![sibling.clone()],
            library_paths: vec![],
        };
        let open_docs: HashMap<String, DocState> = HashMap::new();
        let mut cache = SiblingCache::new();
        let overlay = build_overlay(&view, &root.join("app.pmc"), &open_docs, &mut cache);

        let mut names: Vec<&String> = overlay.symbols.keys().collect();
        names.sort();
        assert_eq!(names, vec!["main", "ns::inner", "top"], "{names:?}");

        let top = &overlay.symbols["top"];
        assert!(top.doc.is_some(), "top's doc line is carried");
        let (uri, span) = top.target.as_ref().expect("top's span is carried");
        assert_eq!(*uri, path_to_file_uri(&sibling));
        assert_eq!(span.start.line, 2, "the `export top()` declaration line");

        let inner = &overlay.symbols["ns::inner"];
        assert!(inner.doc.is_some(), "ns::inner's doc line is carried");
        assert_eq!(
            inner.target.as_ref().unwrap().1.start.line,
            6,
            "the `export inner()` declaration line"
        );

        let main = &overlay.symbols["main"];
        assert!(
            main.doc.is_none(),
            "main carries no doc line in this fixture"
        );
    }

    #[test]
    fn pma_sibling_contributes_non_local_funcs() {
        let root = temp_tree();
        let sibling = root.join("sibling.pma");
        fs::write(&sibling, ".func pub_one\nstp\n.func priv_one local\nstp\n").unwrap();

        let view = ProjectView {
            root: root.clone(),
            stdlib: false,
            siblings: vec![sibling.clone()],
            library_paths: vec![],
        };
        let open_docs: HashMap<String, DocState> = HashMap::new();
        let mut cache = SiblingCache::new();
        let overlay = build_overlay(&view, &root.join("app.pmc"), &open_docs, &mut cache);

        assert_eq!(
            overlay.symbols.keys().collect::<Vec<_>>(),
            vec!["pub_one"],
            "{:?}",
            overlay.symbols
        );
        let pub_one = &overlay.symbols["pub_one"];
        assert!(pub_one.doc.is_none(), "`.pma` carries no doc surface");
        let (uri, _span) = pub_one.target.as_ref().expect("pub_one's span is carried");
        assert_eq!(*uri, path_to_file_uri(&sibling));
    }

    #[test]
    fn pmo_sibling_and_libraries_contribute_names_only() {
        // Two DIFFERENT objects, one per leg (`tiny` only as a declared
        // source, `libonly` only via a linked library) — so each leg's
        // contribution is independently load-bearing; the same object
        // reused for both would leave the library loop provably
        // untested (deleting it would still pass).
        let root = temp_tree();
        let source_bytes = crate::compiler::compile(
            "export tiny() { right; }\n",
            crate::compiler::CompileOptions::default(),
        )
        .expect("tiny.pmc compiles")
        .object
        .to_bytes();
        let library_bytes = crate::compiler::compile(
            "export libonly() { right; }\n",
            crate::compiler::CompileOptions::default(),
        )
        .expect("libonly.pmc compiles")
        .object
        .to_bytes();

        let as_source = root.join("as_source.pmo");
        fs::write(&as_source, &source_bytes).unwrap();
        let as_library = root.join("as_library.pmo");
        fs::write(&as_library, &library_bytes).unwrap();

        let view = ProjectView {
            root: root.clone(),
            stdlib: false,
            siblings: vec![as_source],
            library_paths: vec![as_library],
        };
        let open_docs: HashMap<String, DocState> = HashMap::new();
        let mut cache = SiblingCache::new();
        let overlay = build_overlay(&view, &root.join("app.pmc"), &open_docs, &mut cache);

        let tiny = overlay
            .symbols
            .get("tiny")
            .expect("tiny exported via the object listed as a source");
        assert!(tiny.target.is_none(), "a `.pmo` has no source location");
        assert!(tiny.doc.is_none());

        let libonly = overlay
            .symbols
            .get("libonly")
            .expect("libonly exported via the object resolved as a library");
        assert!(libonly.target.is_none(), "a `.pmo` has no source location");
        assert!(libonly.doc.is_none());
    }

    #[test]
    fn resolution_order_is_sources_then_libraries_first_wins() {
        let root = temp_tree();
        let sibling = root.join("sibling.pmc");
        fs::write(&sibling, "namespace ns {\nexport dup() { right; }\n}\n").unwrap();

        let lib_object = crate::compiler::compile(
            "namespace ns {\nexport dup() { left; }\n}\n",
            crate::compiler::CompileOptions::default(),
        )
        .expect("the library object compiles")
        .object;
        let library = root.join("lib.pmo");
        fs::write(&library, lib_object.to_bytes()).unwrap();

        let open_docs: HashMap<String, DocState> = HashMap::new();

        // Positive control FIRST: the library alone (no competing
        // source) really does define `ns::dup` — proving the main
        // assertion below means something, rather than the sibling
        // source supplying `ns::dup` regardless of whether the library
        // leg contributed anything at all.
        let library_only_view = ProjectView {
            root: root.clone(),
            stdlib: false,
            siblings: vec![],
            library_paths: vec![library.clone()],
        };
        let mut control_cache = SiblingCache::new();
        let control = build_overlay(
            &library_only_view,
            &root.join("app.pmc"),
            &open_docs,
            &mut control_cache,
        );
        assert!(
            control.symbols["ns::dup"].target.is_none(),
            "sanity: the library alone defines ns::dup, name-only"
        );

        let view = ProjectView {
            root: root.clone(),
            stdlib: false,
            siblings: vec![sibling.clone()],
            library_paths: vec![library],
        };
        let mut cache = SiblingCache::new();
        let overlay = build_overlay(&view, &root.join("app.pmc"), &open_docs, &mut cache);

        let sym = &overlay.symbols["ns::dup"];
        let (uri, _span) = sym
            .target
            .as_ref()
            .expect("the source's definition wins, and it carries a span");
        assert_eq!(
            *uri,
            path_to_file_uri(&sibling),
            "sources are resolved before libraries — the sibling source wins, not the library"
        );
    }

    #[test]
    fn broken_sibling_contributes_nothing_others_still_do() {
        let root = temp_tree();
        let broken = root.join("broken.pmc");
        fs::write(&broken, "main( {").unwrap();
        let fine = root.join("fine.pmc");
        fs::write(&fine, "export good() { right; }\n").unwrap();

        let view = ProjectView {
            root: root.clone(),
            stdlib: false,
            siblings: vec![broken, fine],
            library_paths: vec![],
        };
        let open_docs: HashMap<String, DocState> = HashMap::new();
        let mut cache = SiblingCache::new();
        let overlay = build_overlay(&view, &root.join("app.pmc"), &open_docs, &mut cache);

        assert_eq!(
            overlay.symbols.keys().collect::<Vec<_>>(),
            vec!["good"],
            "{:?}",
            overlay.symbols
        );
    }

    #[test]
    fn open_sibling_is_read_from_its_doc_state_not_disk() {
        // A space in the fixture path makes the `path_to_file_uri`
        // round-trip load-bearing: without one, a bug that stopped
        // percent-encoding (or decoding) would go uncaught, since every
        // character in an unescaped path is already its own encoding.
        let root = temp_tree().join("has space");
        fs::create_dir_all(&root).unwrap();
        let sibling = root.join("sibling.pmc");
        fs::write(&sibling, "export old() { right; }\n").unwrap();

        let uri = path_to_file_uri(&sibling);
        let mut svc = crate::lsp::PmcLanguageService::new();
        svc.did_update(&uri, "export new() { right; }\n");
        let live = svc.docs.remove(&uri).expect("did_update just inserted it");
        let mut open_docs: HashMap<String, DocState> = HashMap::new();
        open_docs.insert(uri, live);

        let view = ProjectView {
            root: root.clone(),
            stdlib: false,
            siblings: vec![sibling],
            library_paths: vec![],
        };
        let mut cache = SiblingCache::new();
        let overlay = build_overlay(&view, &root.join("app.pmc"), &open_docs, &mut cache);

        assert!(
            overlay.symbols.contains_key("new"),
            "the live DocState's export, not disk's: {:?}",
            overlay.symbols.keys().collect::<Vec<_>>()
        );
        assert!(!overlay.symbols.contains_key("old"));
    }

    #[test]
    fn sibling_cache_is_mtime_keyed_and_bounded() {
        let open_docs: HashMap<String, DocState> = HashMap::new();
        let root = temp_tree();
        let mut cache = SiblingCache::new();
        // More distinct sibling paths than the eviction bound, so an
        // unbounded cache would visibly outgrow it.
        for i in 0..(SIBLING_CACHE_LIMIT + 8) {
            let path = root.join(format!("s{i}.pmc"));
            fs::write(&path, format!("export f{i}() {{ right; }}\n")).unwrap();
            let view = ProjectView {
                root: root.clone(),
                stdlib: false,
                siblings: vec![path],
                library_paths: vec![],
            };
            build_overlay(&view, &root.join("app.pmc"), &open_docs, &mut cache);
        }
        assert!(
            cache.len() <= SIBLING_CACHE_LIMIT,
            "cache grew past its bound: {} entries",
            cache.len()
        );

        // A rewritten sibling with a bumped mtime must change the next
        // answer — observed through the overlay itself, not cache
        // internals.
        let path = root.join("changing.pmc");
        fs::write(&path, "export old() { right; }\n").unwrap();
        let view = ProjectView {
            root: root.clone(),
            stdlib: false,
            siblings: vec![path.clone()],
            library_paths: vec![],
        };
        let mut fresh_cache = SiblingCache::new();
        let overlay = build_overlay(&view, &root.join("app.pmc"), &open_docs, &mut fresh_cache);
        assert!(overlay.symbols.contains_key("old"), "sanity: starts as old");

        // A guaranteed-newer mtime — the filesystem's own timestamp
        // granularity is not to be trusted in a fast test (mirrors
        // `manifest_cache_is_mtime_keyed_and_bounded`).
        let old_mtime = fs::metadata(&path).unwrap().modified().unwrap();
        fs::write(&path, "export new() { right; }\n").unwrap();
        fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(old_mtime + Duration::from_secs(2))
            .unwrap();

        let overlay = build_overlay(&view, &root.join("app.pmc"), &open_docs, &mut fresh_cache);
        assert!(
            overlay.symbols.contains_key("new"),
            "the bumped mtime must be re-read, not served from the stale cache"
        );
        assert!(!overlay.symbols.contains_key("old"));
    }
}

/// The faithfulness contract: the overlay is a per-file APPROXIMATION of
/// what the real linker resolves (docs/pmt/project.md (the declared
/// source set); the module-level doc comment above). This is the one
/// place that checks the approximation is actually right, by building
/// BOTH sides of one fixture — the overlay through the real service
/// (`did_update` + `DocState.overlay`), the linker side through the same
/// effective-source-order dispatch `cli::driver::build_one_target` uses
/// (`.pmc` compiles, `.pma` assembles, `.pmo` loads) — and comparing them
/// by PROVENANCE (`mtc_core::linker::SymbolOrigin`, which object or
/// library index won), not just by name: two candidates can share a
/// name (the shadowing case below), so only provenance tells them apart.
/// Scope, precisely: restricted to call sites reachable from the
/// fixture's `main`, every name the overlay resolves must point at the
/// same definition `resolve_names` picks, and every reachable name the
/// overlay leaves unresolved must be one `resolve_names` also reports
/// unresolved — `resolve_names` only ever errors on a REACHABLE
/// unresolved name, and a dropped (unreached) function may reference
/// anything, even names that don't exist, so the fixture keeps every
/// call reachable from `main` to stay inside the comparable region.
#[cfg(test)]
mod faithfulness {
    use std::fs;
    use std::sync::atomic::{AtomicU32, Ordering};

    use mtc_core::linker::{LinkError, SymbolOrigin, resolve_names};
    use mtc_core::lsp::LanguageService;

    use super::*;

    /// A fresh scratch directory under `std::env::temp_dir()`, unique per
    /// call (process id + an atomic counter — this crate has no tempfile
    /// dependency, matching the zero-new-deps constraint; house
    /// convention has no shared test-support module, so each file
    /// defines its own local helper — mirrors `overlay::tests`' own
    /// copy).
    fn temp_tree() -> PathBuf {
        static N: AtomicU32 = AtomicU32::new(0);
        let d = std::env::temp_dir().join(format!(
            "pmt-faithfulness-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&d).unwrap();
        d
    }

    /// Loads one already-resolved source path per its extension, exactly
    /// mirroring `cli::driver::load_one_source`'s own three-way dispatch
    /// (`.pmc` compiles, `.pma` assembles, anything else loads as a
    /// `.pmo` object) — the SAME dispatch a real `pmt build` runs over a
    /// target's effective sources, so the objects handed to
    /// `resolve_names` below are the objects the real linker would see,
    /// not a shape invented for this test.
    fn load_as_object(path: &Path) -> ObjectFile {
        match path.extension().and_then(|e| e.to_str()) {
            Some("pmc") => {
                let source = fs::read_to_string(path).unwrap();
                crate::compiler::compile(&source, crate::compiler::CompileOptions::default())
                    .unwrap_or_else(|e| panic!("{}: failed to compile: {e}", path.display()))
                    .object
            }
            Some("pma") => {
                let source = fs::read_to_string(path).unwrap();
                crate::asm::assemble(&source, false)
                    .unwrap_or_else(|e| panic!("{}: failed to assemble: {e:?}", path.display()))
            }
            _ => {
                let bytes = fs::read(path).unwrap();
                ObjectFile::from_bytes(&bytes).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
            }
        }
    }

    #[test]
    fn overlay_resolution_matches_linker_resolution_with_provenance() {
        // One target, everything reachable from `main` (see this module's
        // own doc comment): `shared.pmc` a manifest-level source (bare
        // top-level `helper`, `ns::inner`, and `ns::dup` — the LAST one
        // deliberately re-declared by the library below too); `app.pmc`
        // the target's own entry, calling every kind of cross-file name
        // this task's design distinguishes: a bare `.pmc` sibling export,
        // a qualified `.pmc` sibling export, the SHADOWED qualified name,
        // a bare `.pma` sibling export, a bare `.pmo`-backed source
        // export, and a `std::` call routed through the separate stdlib
        // channel; `helpers.pma` a `.pma` sibling; `pre.pmo` a `.pmo`
        // declared directly as a target source; `libs/bitops.pmo` a
        // declared LIBRARY that also defines `ns::dup` (the shadowing
        // case: the linker's rule is user objects beat libraries) plus an
        // unrelated `bit_only` export that APP calls too — the ONLY
        // library-provenance leg in this fixture: every other name has a
        // `.pmc`/`.pma`/`.pmo` SOURCE contributor, so `bit_only` is what
        // proves a `.pmo`-shaped overlay answer can also mean "the
        // declared library", not just "some source `.pmo`".
        let root = temp_tree();
        fs::create_dir_all(root.join("libs")).unwrap();
        fs::write(
            root.join("pmt.json"),
            r#"{"project":{
                "sources":["shared.pmc"],
                "libraries":{"dirs":["libs"],"link":["bitops"]},
                "targets":{"app":{"sources":["app.pmc","helpers.pma","pre.pmo"]}}
            }}"#,
        )
        .unwrap();

        const SHARED: &str = "export helper() { right; }\nnamespace ns {\nexport inner() { right; }\n}\nnamespace ns {\nexport dup() { right; }\n}\n";
        fs::write(root.join("shared.pmc"), SHARED).unwrap();

        const APP: &str = "export main() {\n    @helper();\n    @ns::inner();\n    @ns::dup();\n    @asm_fn();\n    @pre_fn();\n    @bit_only();\n    @std::goToEnd();\n}\n";
        fs::write(root.join("app.pmc"), APP).unwrap();

        const HELPERS: &str = ".func asm_fn\nstp\n";
        fs::write(root.join("helpers.pma"), HELPERS).unwrap();

        let pre_bytes = crate::compiler::compile(
            "export pre_fn() { right; }\n",
            crate::compiler::CompileOptions::default(),
        )
        .expect("pre_fn's source compiles")
        .object
        .to_bytes();
        fs::write(root.join("pre.pmo"), &pre_bytes).unwrap();

        let bitops_bytes = crate::compiler::compile(
            "namespace ns {\nexport dup() { left; }\n}\nexport bit_only() { right; }\n",
            crate::compiler::CompileOptions::default(),
        )
        .expect("bitops's source compiles")
        .object
        .to_bytes();
        fs::write(root.join("libs").join("bitops.pmo"), &bitops_bytes).unwrap();

        // --- Overlay side: the real service, driven exactly as an
        //     editor would (`did_update` over the on-disk text). ---
        let app_uri = path_to_file_uri(&root.join("app.pmc"));
        let mut svc = crate::lsp::PmcLanguageService::new();
        let diags = svc.did_update(&app_uri, APP);
        let state = svc.docs.get(&app_uri).expect("did_update just inserted it");
        let overlay = state
            .overlay
            .as_ref()
            .expect("app.pmc is a member of target `app`");

        let shared_uri = path_to_file_uri(&root.join("shared.pmc"));
        let helpers_uri = path_to_file_uri(&root.join("helpers.pma"));

        // Every source-backed name: the overlay's own pick, by file.
        for (name, expect_uri) in [
            ("helper", &shared_uri),
            ("ns::inner", &shared_uri),
            ("ns::dup", &shared_uri),
            ("asm_fn", &helpers_uri),
        ] {
            let sym = overlay
                .symbols
                .get(name)
                .unwrap_or_else(|| panic!("overlay resolves {name}"));
            let (uri, _span) = sym
                .target
                .as_ref()
                .unwrap_or_else(|| panic!("{name} is source-backed, carries a span"));
            assert_eq!(uri, expect_uri, "{name}: overlay's own pick");
        }
        // `pre_fn` and `bit_only` are both `.pmo`-backed: name-only
        // answers, no location — one from a source `.pmo`, the other
        // from the declared library.
        for name in ["pre_fn", "bit_only"] {
            assert!(
                overlay
                    .symbols
                    .get(name)
                    .unwrap_or_else(|| panic!("overlay resolves {name}"))
                    .target
                    .is_none(),
                "{name} comes from a .pmo — no source location to carry"
            );
        }
        // `std::goToEnd` is never inserted into `overlay.symbols` at all
        // — it resolves through the SEPARATE stdlib channel
        // (`Overlay.stdlib` + `crate::stdlib::roster()`), not the
        // sibling/library scan this struct otherwise represents.
        assert!(!overlay.symbols.contains_key("std::goToEnd"));
        assert!(overlay.stdlib, "the manifest's default stdlib flag is on");
        assert!(
            crate::stdlib::roster()
                .iter()
                .any(|e| e.full_path == "std::goToEnd"),
            "std::goToEnd is a real embedded-stdlib routine"
        );

        // Every bare call in APP (`helper`, `asm_fn`, `pre_fn`,
        // `bit_only`) is covered by a sibling/library export, so the
        // cross-file refinement (docs/pmt/cli.md (undeclared-external))
        // must have silenced every one of them — proving this fixture's
        // calls are the SAME calls just inspected above, not a
        // coincidence of two disconnected checks.
        assert!(
            diags.iter().all(|d| d.code != Some("undeclared-external")),
            "every bare call in APP resolves through the overlay: {diags:?}"
        );

        // --- Linker side: the same effective source order + declared
        //     libraries `pmt build` would use for target `app`
        //     (`cli::driver::build_one_target`). ---
        let object_files = ["shared.pmc", "app.pmc", "helpers.pma", "pre.pmo"];
        let objects: Vec<ObjectFile> = object_files
            .iter()
            .map(|f| load_as_object(&root.join(f)))
            .collect();

        let libs_dir = root.join("libs").to_string_lossy().into_owned();
        let mut libraries = vec![
            crate::cli::build::find_library("bitops", &[libs_dir])
                .expect("bitops.pmo resolves via the declared library dir"),
        ];
        libraries.push(crate::stdlib::object().clone());

        let resolved = resolve_names(&objects, &libraries, "main")
            .expect("every reachable call in this fixture resolves");
        let origin_of = |name: &str| -> SymbolOrigin {
            resolved
                .reached
                .iter()
                .find(|r| r.name == name)
                .unwrap_or_else(|| panic!("{name} is reached from main"))
                .origin
        };

        assert_eq!(origin_of("helper"), SymbolOrigin::Object(0), "shared.pmc");
        assert_eq!(
            origin_of("ns::inner"),
            SymbolOrigin::Object(0),
            "shared.pmc"
        );
        // THE discriminating assertion: the linker's own, independently
        // implemented shadowing rule (`resolve::resolve` — user objects
        // beat libraries) also picks shared.pmc over libs/bitops.pmo. If
        // the overlay's first-wins ordering (`insert_export`, above) ever
        // diverged from the linker's — say, libraries were merged before
        // sources, or a `.pmo` library name were preferred by
        // registration order rather than by kind — the two independent
        // answers being compared here (this one, and the overlay's own
        // `ns::dup` pick asserted above) would disagree, and only THIS
        // fixture's duplicate-defined `ns::dup` can catch that: every
        // other name in this fixture has exactly one definer, so picking
        // the wrong one would be invisible to a name-only comparison.
        assert_eq!(
            origin_of("ns::dup"),
            SymbolOrigin::Object(0),
            "shared.pmc must win — NOT libs/bitops.pmo"
        );
        assert_eq!(origin_of("asm_fn"), SymbolOrigin::Object(2), "helpers.pma");
        assert_eq!(origin_of("pre_fn"), SymbolOrigin::Object(3), "pre.pmo");
        assert_eq!(
            origin_of("bit_only"),
            SymbolOrigin::Library(0),
            "libs/bitops.pmo, the first (only non-stdlib) declared library"
        );
        assert_eq!(
            origin_of("std::goToEnd"),
            SymbolOrigin::Library(1),
            "the embedded stdlib, the second declared library"
        );

        // Map every Object-origin name back to the SAME file the overlay
        // named, by provenance (`SymbolOrigin::Object(i)` ->
        // `object_files[i]`), for every name that carries a navigable
        // overlay location.
        for name in ["helper", "ns::inner", "ns::dup", "asm_fn"] {
            let SymbolOrigin::Object(i) = origin_of(name) else {
                panic!("{name} must come from a user object, not a library");
            };
            let expect_uri = path_to_file_uri(&root.join(object_files[i]));
            let overlay_uri = &overlay.symbols[name].target.as_ref().unwrap().0;
            assert_eq!(
                *overlay_uri, expect_uri,
                "{name}: overlay pick and linker provenance name the same file"
            );
        }

        // `pre_fn` and `bit_only`: each overlay answer is unlocated
        // ("some .pmo"); the linker names a specific object or library
        // index for each. The two agree by PROVENANCE, not coincidence:
        // each name has exactly ONE object-or-library contributor in
        // this whole fixture (the combined `objects` then `libraries`
        // index space `SymbolOrigin` itself counts through), so there is
        // no OTHER candidate the overlay's name-only answer could have
        // silently meant instead.
        let sole_definer = |name: &str| -> usize {
            let definers: Vec<usize> = objects
                .iter()
                .chain(libraries.iter())
                .enumerate()
                .filter(|(_, o)| {
                    o.symbols
                        .iter()
                        .any(|s| s.name == name && matches!(s.def, SymbolDef::Defined { .. }))
                })
                .map(|(i, _)| i)
                .collect();
            assert_eq!(
                definers.len(),
                1,
                "{name} must have exactly one object-or-library definer: {definers:?}"
            );
            definers[0]
        };
        assert_eq!(
            sole_definer("pre_fn"),
            3,
            "pre_fn: pre.pmo (object index 3) alone"
        );
        assert_eq!(
            sole_definer("bit_only"),
            objects.len(),
            "bit_only: libs/bitops.pmo (library index 0 = combined index objects.len())"
        );
    }

    #[test]
    fn overlay_resolution_matches_linker_resolution_for_a_shadowed_std_name() {
        // The `ns::dup` shape (the sibling-vs-library shadowing case
        // above), one level over: a sibling's own `namespace std {
        // export goToEnd() {...} }` mangles to the SAME `std::goToEnd`
        // key the embedded stdlib roster answers under, creating a
        // genuine two-definer collision — the embedded stdlib object
        // really does export a symbol named `std::goToEnd`
        // (`overlay_resolution_matches_linker_resolution_with_provenance`'s
        // own `origin_of("std::goToEnd") == Library(1)` assertion, for
        // the UNSHADOWED case, already proves this). Both sides must
        // pick the sibling: the overlay because a name it owns always
        // wins over the materialized roster, the linker because the
        // embedded stdlib links as an ordinary library, appended LAST,
        // behind every declared source — so its own sources-before-
        // libraries rule already prefers the sibling. This is exactly
        // the case an overlay that special-cased the `std::` PREFIX
        // (routing it straight to the materialized roster without ever
        // consulting the overlay) would get wrong, and it is what
        // caught that defect originally.
        let root = temp_tree();
        fs::write(
            root.join("pmt.json"),
            r#"{"project":{"targets":{"app":{"sources":["app.pmc","shared.pmc"]}}}}"#,
        )
        .unwrap();
        const SHARED: &str = "namespace std {\nexport goToEnd() { right; }\n}\n";
        fs::write(root.join("shared.pmc"), SHARED).unwrap();

        const APP: &str = "export main() {\n    @std::goToEnd();\n}\n";
        fs::write(root.join("app.pmc"), APP).unwrap();

        // --- Overlay side. ---
        let app_uri = path_to_file_uri(&root.join("app.pmc"));
        let mut svc = crate::lsp::PmcLanguageService::new();
        let diags = svc.did_update(&app_uri, APP);
        let state = svc.docs.get(&app_uri).expect("did_update just inserted it");
        let overlay = state
            .overlay
            .as_ref()
            .expect("app.pmc is a member of target `app`");

        let shared_uri = path_to_file_uri(&root.join("shared.pmc"));
        let sym = overlay.symbols.get("std::goToEnd").unwrap_or_else(|| {
            panic!("the sibling's own namespace-std export registers under the same mangled key the embedded roster answers under")
        });
        let (uri, _span) = sym
            .target
            .as_ref()
            .expect("std::goToEnd is source-backed here, carries a span");
        assert_eq!(
            uri, &shared_uri,
            "the overlay must pick the sibling, not the embedded stdlib"
        );

        assert!(
            diags.iter().all(|d| d.code != Some("undeclared-external")),
            "std::goToEnd resolves through the overlay: {diags:?}"
        );

        // --- Linker side: the same effective source order `pmt build`
        //     would use for target `app` — no declared libraries, so the
        //     embedded stdlib is the sole entry in `libraries`. ---
        let object_files = ["shared.pmc", "app.pmc"];
        let objects: Vec<ObjectFile> = object_files
            .iter()
            .map(|f| load_as_object(&root.join(f)))
            .collect();
        let libraries = vec![crate::stdlib::object().clone()];

        let resolved = resolve_names(&objects, &libraries, "main")
            .expect("std::goToEnd resolves — the sibling, not a genuine unresolved miss");
        let origin = resolved
            .reached
            .iter()
            .find(|r| r.name == "std::goToEnd")
            .expect("std::goToEnd is reached from main")
            .origin;
        assert_eq!(
            origin,
            SymbolOrigin::Object(0),
            "shared.pmc must win — NOT the embedded stdlib library"
        );
    }

    #[test]
    fn overlay_unresolved_matches_linker_unresolved() {
        // A bare call to a name defined NOWHERE — no sibling, no library,
        // no stdlib routine — the negative half of the contract: the
        // overlay must leave it unresolved (its warning stays
        // unsuppressed, docs/pmt/cli.md (undeclared-external)) exactly
        // when `resolve_names` also reports it as reachable-unresolved.
        let root = temp_tree();
        fs::write(
            root.join("pmt.json"),
            r#"{"project":{"targets":{"app":{"sources":["app.pmc"]}}}}"#,
        )
        .unwrap();
        const APP: &str = "export main() {\n    @ghost();\n}\n";
        fs::write(root.join("app.pmc"), APP).unwrap();

        // --- Overlay side. ---
        let app_uri = path_to_file_uri(&root.join("app.pmc"));
        let mut svc = crate::lsp::PmcLanguageService::new();
        let diags = svc.did_update(&app_uri, APP);
        let state = svc.docs.get(&app_uri).expect("did_update just inserted it");
        let overlay = state
            .overlay
            .as_ref()
            .expect("app.pmc is a member of target `app`");

        assert!(
            !overlay.symbols.contains_key("ghost"),
            "ghost is defined nowhere in this fixture"
        );
        assert!(
            diags
                .iter()
                .any(|d| d.code == Some("undeclared-external") && d.message.contains("ghost")),
            "nothing defines ghost, so its warning must stay unsuppressed: {diags:?}"
        );

        // --- Linker side: the same one-source, stdlib-linked build
        //     `pmt build` would run for this target. ---
        let object = crate::compiler::compile(APP, crate::compiler::CompileOptions::default())
            .expect("app.pmc compiles despite the undeclared call")
            .object;
        let libraries = vec![crate::stdlib::object().clone()];
        let err = resolve_names(std::slice::from_ref(&object), &libraries, "main")
            .expect_err("ghost is reachable from main and defined nowhere");
        assert_eq!(err, LinkError::Unresolved(vec!["ghost".to_string()]));
    }
}
