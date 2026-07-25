//! Cross-file project view for the LSP: given an open document, finds its
//! nearest manifest-bearing `pmt.json`, decides whether the document is a
//! member of any build target, and produces the union of sibling source
//! files it links with plus the resolved on-disk paths of its declared
//! libraries (docs/pmt/project.md (discovery), (the declared source set),
//! (schema reference)). This module only computes the VIEW — a later
//! stage turns the file set into an indexed symbol overlay.

use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};
use std::time::SystemTime;

use crate::project::{Libraries, Manifest, load_file, normalize_rel};

/// One open document's project membership view: the manifest directory,
/// the `stdlib` flag, the sibling source files it links with (self
/// excluded, union across every target the document is a member of, in
/// target order), and the resolved paths of its declared libraries.
#[derive(Debug, PartialEq)]
#[allow(dead_code)] // constructed only by `project_view`, wired into `did_update` by the next task.
pub(super) struct ProjectView {
    pub root: PathBuf,
    pub stdlib: bool,
    pub siblings: Vec<PathBuf>,
    pub library_paths: Vec<PathBuf>,
}

/// `pmt.json` parse cache keyed by candidate path: `(mtime, outcome)`.
/// Mirrors `ConfigResolver::project_allow`'s discipline (mod.rs
/// (configuration)) applied to the project-section manifest instead of
/// the lint allow-list: stat first; no stat means no cache entry at all;
/// eviction happens only when inserting a new key at capacity, and is
/// arbitrary (not LRU) — a miss only costs a re-parse, never a wrong
/// answer.
pub(super) type ManifestCache = HashMap<PathBuf, (SystemTime, Result<Option<Manifest>, String>)>;

/// Bounds `ManifestCache`'s growth for the same reason as
/// `CONFIG_CACHE_LIMIT` (mod.rs (configuration)): a long-running server
/// visiting many project roots over its lifetime must not grow this map
/// forever, and an arbitrary eviction at capacity can only turn a hit
/// into a miss.
#[allow(dead_code)] // read only by `cached_manifest`, wired into `did_update` by the next task.
const MANIFEST_CACHE_LIMIT: usize = 32;

/// The project-file channel for one `pmt.json` candidate: the parsed
/// outcome through the mtime cache, reused only while the file's mtime is
/// unchanged, else re-loaded and re-cached. `Ok(None)` means the file is
/// valid but lint-only (no `project` section) — transparent to the
/// caller's walk; `Err` carries the load error's display string.
#[allow(dead_code)] // called only by `project_view`, wired into `did_update` by the next task.
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
#[allow(dead_code)] // called only by `project_view`, wired into `did_update` by the next task.
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
#[allow(dead_code)] // wired into `did_update` by the next task.
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

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::time::Duration;

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
        fs::write(root.join("libs").join("bit.pmo"), "").unwrap();
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
}
