//! Project configuration: `pmt.json`, the toolchain's first (deliberately
//! tiny) project file (docs/lint.md (project file)).
//!
//! Validation is a manual [`serde_json::Value`] walk rather than
//! `#[serde(deny_unknown_fields)]`: a derive-based reject gives one
//! generic "unknown field" error for the whole document, while a typo in
//! a hand-authored config file deserves a precise "unknown key `X` at
//! `lint`" pointing at exactly the offending key. The schema itself is
//! intentionally tiny — today, `lint.allow` and nothing else — so the
//! manual walk stays a handful of match arms, not a maintenance burden.

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::lint::{self, LintError};

/// The parsed, validated contents of a `pmt.json`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProjectConfig {
    /// `lint.allow`: rule codes to suppress, already validated against
    /// the rule table (`lint::validate_allow`).
    pub allow: Vec<String>,
}

/// A `pmt.json` failed to load. Every variant carries the path of the
/// file at fault, so a caller juggling several discovered configs (one
/// per input file, `discover`'s nearest-ancestor-per-file contract) can
/// always say which one.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ConfigError {
    /// The file could not be read (permissions, a race with `discover`
    /// finding it and it disappearing before `load` runs, ...).
    Io { path: PathBuf, message: String },
    /// The file's bytes are not valid JSON, or do not resolve to the
    /// documented shape (an object at the root, an object at `lint`, an
    /// array of strings at `lint.allow`) — shape errors are folded in
    /// here rather than given their own variant, since both are "this
    /// document does not parse into a `pmt.json`". `message` carries
    /// its own prefix at the construction site (`"invalid JSON: "` only
    /// for a genuine `serde_json::from_str` syntax failure; a shape
    /// complaint stands alone) — [`ConfigError::detail`] does not
    /// impose one uniformly, since the two causes aren't the same kind
    /// of wrong.
    Parse { path: PathBuf, message: String },
    /// A key outside the documented schema — at the root, or inside
    /// `lint`. A typo (`"lints"`, `"allowed"`) must not silently do
    /// nothing, so this is an error, not an ignored field.
    UnknownKey { path: PathBuf, key: String },
    /// A `lint.allow` entry names no rule in the catalog
    /// (`lint::validate_allow`'s check, re-homed onto this error type so
    /// the CLI's per-file config posture, docs/lint.md (project file),
    /// has one error type to render for every `pmt.json` problem).
    UnknownAllowCode { path: PathBuf, code: String },
}

impl ConfigError {
    /// The file this error is about.
    pub(crate) fn path(&self) -> &Path {
        match self {
            ConfigError::Io { path, .. }
            | ConfigError::Parse { path, .. }
            | ConfigError::UnknownKey { path, .. }
            | ConfigError::UnknownAllowCode { path, .. } => path,
        }
    }

    /// The kind-specific text, without the path — shared by [`Display`]
    /// (below, which prefixes it with the path) and the CLI's per-file
    /// rendering (`cli/lint.rs`, which prefixes path + `error:`), the
    /// same split `CompileError`/`CompileErrorKind` use for the same
    /// reason: one location prefix, reused by two different callers.
    pub(crate) fn detail(&self) -> String {
        match self {
            ConfigError::Io { message, .. } => format!("cannot read: {message}"),
            // `message` already carries whatever prefix fits its cause —
            // `"invalid JSON: {serde error}"` for a genuine parse
            // failure (baked in at the `serde_json::from_str` call
            // site), or a bare shape complaint ("top-level value must
            // be a JSON object", ...) for well-formed-but-wrong-shape
            // documents. Prefixing here unconditionally would mislabel
            // the shape case as a JSON syntax error it isn't.
            ConfigError::Parse { message, .. } => message.clone(),
            ConfigError::UnknownKey { key, .. } => format!("unknown key `{key}`"),
            ConfigError::UnknownAllowCode { code, .. } => {
                format!("unknown lint rule `{code}` in lint.allow")
            }
        }
    }
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.path().display(), self.detail())
    }
}

impl std::error::Error for ConfigError {}

/// Nearest-ancestor walk from `start` (the source file's directory) to
/// the filesystem root; the first `pmt.json` found wins — never a
/// cascade. A `pmt.json` further up the tree is not read at all, let
/// alone merged: two input files under different nearest configs in the
/// same `pmt lint` run may end up with entirely different effective
/// allow-lists, and that is by design (a subtree opts into its own
/// config by having its own file).
///
/// `start` is absolutized first (`std::path::absolute` — a lexical
/// operation, no filesystem access, no symlink resolution, and unlike
/// `canonicalize` it does not require `start` to exist) before the walk
/// begins. The CLI feeds paths as spelled, so `start` may be relative;
/// walking `Path::parent()` on a relative path bottoms out at `Some("")`
/// then `None` — the invocation directory, not the filesystem root —
/// which would silently stop the search short of any `pmt.json` that
/// lives above the process's cwd. An empty `start` (the caller's usual
/// `file.parent()`, when `file` is a bare filename with no directory
/// component — `Path::new("foo.pmc").parent()` is `Some("")`, not
/// `None`) is treated as `.`/cwd rather than passed to
/// `std::path::absolute` as-is, since `std::path::absolute("")` errors:
/// bare-filename callers must still discover cwd's own `pmt.json`, the
/// one case the pre-absolutization code got right.
pub(crate) fn discover(start: &Path) -> Option<PathBuf> {
    let start = if start.as_os_str().is_empty() {
        Path::new(".")
    } else {
        start
    };
    let abs_start = std::path::absolute(start).ok()?;
    discover_from(&abs_start)
}

/// The nearest-ancestor walk itself, over an already-absolute `start`.
/// Split out of [`discover`] so the absolutization step is unit-testable
/// on its own (see `discover_relative_start_matches_absolutized_delegate`
/// below) without needing a real filesystem tree above cwd.
fn discover_from(start: &Path) -> Option<PathBuf> {
    let mut dir = Some(start);
    while let Some(d) = dir {
        let candidate = d.join("pmt.json");
        if candidate.is_file() {
            return Some(candidate);
        }
        dir = d.parent();
    }
    None
}

/// Loads and validates the `pmt.json` at `path`. An empty object `{}` is
/// valid (an empty allow-list) — a `pmt.json` need not set anything to
/// be worth discovering, e.g. one that exists purely to mark a subtree
/// root.
pub(crate) fn load(path: &Path) -> Result<ProjectConfig, ConfigError> {
    let text = fs::read_to_string(path).map_err(|e| ConfigError::Io {
        path: path.to_path_buf(),
        message: e.to_string(),
    })?;
    let value: Value = serde_json::from_str(&text).map_err(|e| ConfigError::Parse {
        path: path.to_path_buf(),
        message: format!("invalid JSON: {e}"),
    })?;
    let root = value.as_object().ok_or_else(|| ConfigError::Parse {
        path: path.to_path_buf(),
        message: "top-level value must be a JSON object".to_string(),
    })?;

    let mut allow: Vec<String> = Vec::new();
    for (key, val) in root {
        if key != "lint" {
            return Err(ConfigError::UnknownKey {
                path: path.to_path_buf(),
                key: key.clone(),
            });
        }
        let lint_obj = val.as_object().ok_or_else(|| ConfigError::Parse {
            path: path.to_path_buf(),
            message: "`lint` must be a JSON object".to_string(),
        })?;
        for (lkey, lval) in lint_obj {
            if lkey != "allow" {
                return Err(ConfigError::UnknownKey {
                    path: path.to_path_buf(),
                    key: lkey.clone(),
                });
            }
            let arr = lval.as_array().ok_or_else(|| ConfigError::Parse {
                path: path.to_path_buf(),
                message: "`lint.allow` must be an array of strings".to_string(),
            })?;
            for item in arr {
                let s = item.as_str().ok_or_else(|| ConfigError::Parse {
                    path: path.to_path_buf(),
                    message: "`lint.allow` must be an array of strings".to_string(),
                })?;
                allow.push(s.to_string());
            }
        }
    }

    match lint::validate_allow(&allow) {
        Ok(()) => {}
        Err(LintError::UnknownAllowCode(code)) => {
            return Err(ConfigError::UnknownAllowCode {
                path: path.to_path_buf(),
                code,
            });
        }
        Err(other) => unreachable!("validate_allow only ever returns UnknownAllowCode: {other}"),
    }

    Ok(ProjectConfig { allow })
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;

    /// A fresh scratch directory under `std::env::temp_dir()`, unique per
    /// call (process id + an atomic counter — this crate has no tempfile
    /// dependency, matching the zero-new-deps constraint). Mirrors
    /// `stdlib::tests::unique_tmp_dir`.
    fn unique_tmp_dir(label: &str) -> PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "pmt-config-test-{label}-{}-{n}",
            std::process::id()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn discover_nearest_ancestor_wins_never_cascades() {
        let root = unique_tmp_dir("nearest");
        let a = root.join("a");
        let ab = a.join("b");
        let abc = ab.join("c");
        fs::create_dir_all(&abc).unwrap();
        fs::write(a.join("pmt.json"), r#"{"lint":{"allow":["unused-label"]}}"#).unwrap();
        fs::write(
            ab.join("pmt.json"),
            r#"{"lint":{"allow":["leading-zeros"]}}"#,
        )
        .unwrap();

        let found = discover(&abc).expect("a/b/pmt.json is discoverable from a/b/c");
        assert_eq!(found, ab.join("pmt.json"), "the nearer ancestor wins");

        // Never a cascade: the config loaded from the discovered path is
        // ONLY a/b's — a's `unused-label` entry is not merged in.
        let config = load(&found).unwrap();
        assert_eq!(config.allow, vec!["leading-zeros".to_string()]);
    }

    #[test]
    fn discover_returns_none_when_no_ancestor_has_one() {
        let dir = unique_tmp_dir("absent");
        assert_eq!(discover(&dir), None);
    }

    /// Pins the mechanism the bug report was about: a RELATIVE `start`
    /// must not be handled any differently from its already-absolute
    /// form. `discover` absolutizes internally then delegates to
    /// `discover_from`; this asserts that delegation actually happens by
    /// comparing `discover` on a relative path against `discover_from`
    /// fed the same path pre-absolutized by hand — if `discover` ever
    /// regressed to walking the relative path's own (short) `parent()`
    /// chain instead, this would no longer hold in general (a relative
    /// path's ancestor chain is a strict, shorter prefix of its
    /// absolutized form's chain whenever cwd is not `/`).
    #[test]
    fn discover_relative_start_matches_absolutized_delegate() {
        let relative = Path::new("some-nonexistent-relative-subdir");
        let absolutized =
            std::path::absolute(relative).expect("a plain relative path always absolutizes");
        assert_eq!(discover(relative), discover_from(&absolutized));
    }

    /// The doc-comment claim in prose: `Path::parent()` on a relative
    /// path bottoms out well short of the filesystem root (`Some("")`
    /// then `None`), while the absolutized form keeps climbing. This is
    /// the actual defect `discover_relative_start_matches_absolutized_delegate`
    /// guards against — without absolutization, a relative `start` could
    /// never reach a `pmt.json` living above cwd.
    #[test]
    fn relative_path_parent_chain_is_shorter_than_absolutized_chain() {
        let relative = Path::new("some-nonexistent-relative-subdir");
        assert_eq!(relative.parent(), Some(Path::new("")));
        assert_eq!(relative.parent().unwrap().parent(), None);

        let absolutized = std::path::absolute(relative).unwrap();
        // The absolutized form has at least one more ancestor than the
        // bare relative path did (cwd itself, at minimum) — this repo's
        // test run is never rooted at `/`.
        assert!(absolutized.parent().is_some());
    }

    /// The `cli/lint.rs` call site is `file.parent().and_then(discover)`,
    /// and for a bare filename (no directory component, e.g. the PATH
    /// argument `foo.pmc`) `Path::parent()` yields `Some("")`, not
    /// `None` — so `discover` is routinely handed an EMPTY path, not
    /// just a relative one. `std::path::absolute("")` errors
    /// (`InvalidInput`), so absolutizing naively would make `discover`
    /// return `None` even when cwd itself has a `pmt.json` — a
    /// regression from the pre-absolutization behavior, which found
    /// cwd's own `pmt.json` (just never climbed above it). This pins
    /// that an empty `start` is treated exactly like an explicit cwd,
    /// chdir-free: both sides absolutize to the identical path, so the
    /// two calls must agree regardless of what cwd's ancestry contains.
    #[test]
    fn discover_treats_empty_start_as_cwd() {
        assert_eq!(
            discover(Path::new("")),
            discover(&std::env::current_dir().unwrap())
        );
    }

    #[test]
    fn load_rejects_unparseable_json() {
        let dir = unique_tmp_dir("badjson");
        let path = dir.join("pmt.json");
        fs::write(&path, "{").unwrap();
        let err = load(&path).unwrap_err();
        assert!(matches!(err, ConfigError::Parse { .. }), "{err:?}");
        // Reserved for genuine JSON syntax failures (Finding 2): the
        // detail text is prefixed.
        assert!(
            err.detail().starts_with("invalid JSON: "),
            "{}",
            err.detail()
        );
    }

    #[test]
    fn load_rejects_wrong_shape_without_the_json_syntax_prefix() {
        // Well-formed JSON, wrong shape: must NOT read as an "invalid
        // JSON" complaint — the document parsed fine, it just isn't a
        // `pmt.json`.
        let dir = unique_tmp_dir("badshape");
        let path = dir.join("pmt.json");
        fs::write(&path, "[]").unwrap();
        let err = load(&path).unwrap_err();
        assert_eq!(
            err.detail(),
            "top-level value must be a JSON object",
            "shape errors stand alone, no `invalid JSON:` prefix"
        );
    }

    #[test]
    fn load_rejects_unknown_top_level_key() {
        let dir = unique_tmp_dir("topkey");
        let path = dir.join("pmt.json");
        fs::write(&path, r#"{"lints":{}}"#).unwrap();
        let err = load(&path).unwrap_err();
        assert_eq!(
            err,
            ConfigError::UnknownKey {
                path: path.clone(),
                key: "lints".to_string(),
            }
        );
    }

    #[test]
    fn load_rejects_unknown_lint_key() {
        let dir = unique_tmp_dir("lintkey");
        let path = dir.join("pmt.json");
        fs::write(&path, r#"{"lint":{"allowed":[]}}"#).unwrap();
        let err = load(&path).unwrap_err();
        assert_eq!(
            err,
            ConfigError::UnknownKey {
                path: path.clone(),
                key: "allowed".to_string(),
            }
        );
    }

    #[test]
    fn load_rejects_unknown_allow_code() {
        let dir = unique_tmp_dir("badcode");
        let path = dir.join("pmt.json");
        fs::write(&path, r#"{"lint":{"allow":["no-such"]}}"#).unwrap();
        let err = load(&path).unwrap_err();
        assert_eq!(
            err,
            ConfigError::UnknownAllowCode {
                path: path.clone(),
                code: "no-such".to_string(),
            }
        );
    }

    #[test]
    fn load_accepts_a_known_allow_code() {
        let dir = unique_tmp_dir("goodcode");
        let path = dir.join("pmt.json");
        fs::write(&path, r#"{"lint":{"allow":["unused-label"]}}"#).unwrap();
        let config = load(&path).unwrap();
        assert_eq!(config.allow, vec!["unused-label".to_string()]);
    }

    #[test]
    fn load_accepts_an_empty_object() {
        let dir = unique_tmp_dir("empty");
        let path = dir.join("pmt.json");
        fs::write(&path, "{}").unwrap();
        let config = load(&path).unwrap();
        assert!(config.allow.is_empty());
    }

    #[test]
    fn display_names_the_path_and_the_detail() {
        let dir = unique_tmp_dir("display");
        let path = dir.join("pmt.json");
        fs::write(&path, r#"{"lints":{}}"#).unwrap();
        let err = load(&path).unwrap_err();
        let rendered = err.to_string();
        assert!(rendered.contains(&path.display().to_string()));
        assert!(rendered.contains("unknown key `lints`"));
    }
}
