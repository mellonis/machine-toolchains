//! Project configuration: `tmt.json`, the TM toolchain's project file — the
//! twin of PM-1's `pmt.json` (docs/tmt/lint.md (project file)). Same tiny
//! schema (`lint.allow` and nothing else), same nearest-ancestor discovery,
//! same UNION-with-the-flag merge (never a cascade).
//!
//! Validation is a manual [`serde_json::Value`] walk rather than
//! `#[serde(deny_unknown_fields)]`: a typo in a hand-authored config deserves
//! a precise "unknown key `X`" pointing at the offending key, not one generic
//! deserialize error for the whole document. The schema is intentionally tiny,
//! so the walk stays a handful of match arms.

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::lint::{self, LintError};

/// The parsed, validated contents of a `tmt.json`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProjectConfig {
    /// `lint.allow`: rule codes to suppress, already validated against the
    /// shared rule namespace (`lint::validate_allow`).
    pub allow: Vec<String>,
}

/// A `tmt.json` failed to load. Every variant carries the path at fault, so a
/// caller juggling one discovered config per input file can always say which.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ConfigError {
    /// The file could not be read.
    Io { path: PathBuf, message: String },
    /// The bytes are not valid JSON, or do not resolve to the documented shape
    /// (an object at the root, an object at `lint`, an array of strings at
    /// `lint.allow`). `message` carries its own prefix at the construction site
    /// (`"invalid JSON: "` only for a genuine syntax failure; a shape complaint
    /// stands alone).
    Parse { path: PathBuf, message: String },
    /// A key outside the documented schema — at the root, or inside `lint`.
    UnknownKey { path: PathBuf, key: String },
    /// A `lint.allow` entry names no rule in the shared namespace.
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

    /// The kind-specific text, without the path — shared by [`Display`] (which
    /// prefixes the path) and the CLI's per-file rendering (which prefixes
    /// `path + error:`).
    pub(crate) fn detail(&self) -> String {
        match self {
            ConfigError::Io { message, .. } => format!("cannot read: {message}"),
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

/// Nearest-ancestor walk from `start` (the source file's directory) to the
/// filesystem root; the first `tmt.json` found wins — never a cascade. A
/// `tmt.json` further up the tree is not read at all: two input files under
/// different nearest configs in one `tmt lint` run may end up with entirely
/// different allow-lists, by design (a subtree opts into its own config by
/// having its own file).
///
/// `start` is absolutized first (`std::path::absolute` — a lexical operation,
/// no filesystem access) before the walk. The CLI feeds paths as spelled, so
/// `start` may be relative; walking `Path::parent()` on a relative path bottoms
/// out at `Some("")` then `None` — the invocation directory, not the filesystem
/// root — which would stop the search short of a `tmt.json` above cwd. An empty
/// `start` (a bare filename's `parent()`) is treated as `.`/cwd rather than
/// passed to `std::path::absolute` (which errors on `""`), so bare-filename
/// callers still discover cwd's own `tmt.json`.
pub(crate) fn discover(start: &Path) -> Option<PathBuf> {
    let start = if start.as_os_str().is_empty() {
        Path::new(".")
    } else {
        start
    };
    let abs_start = std::path::absolute(start).ok()?;
    discover_from(&abs_start)
}

/// The nearest-ancestor walk itself, over an already-absolute `start`. Split
/// out of [`discover`] so absolutization is unit-testable on its own.
fn discover_from(start: &Path) -> Option<PathBuf> {
    let mut dir = Some(start);
    while let Some(d) = dir {
        let candidate = d.join("tmt.json");
        if candidate.is_file() {
            return Some(candidate);
        }
        dir = d.parent();
    }
    None
}

/// Loads and validates the `tmt.json` at `path`. An empty object `{}` is valid
/// (an empty allow-list) — a `tmt.json` need not set anything to be worth
/// discovering (one may exist purely to mark a subtree root).
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

    /// A fresh scratch directory, unique per call (process id + an atomic
    /// counter — this crate has no tempfile dependency).
    fn unique_tmp_dir(label: &str) -> PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "tmt-config-test-{label}-{}-{n}",
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
        fs::write(a.join("tmt.json"), r#"{"lint":{"allow":["dead-rule"]}}"#).unwrap();
        fs::write(
            ab.join("tmt.json"),
            r#"{"lint":{"allow":["leftover-debugger"]}}"#,
        )
        .unwrap();

        let found = discover(&abc).expect("a/b/tmt.json is discoverable from a/b/c");
        assert_eq!(found, ab.join("tmt.json"), "the nearer ancestor wins");

        // Never a cascade: only a/b's config is loaded.
        let config = load(&found).unwrap();
        assert_eq!(config.allow, vec!["leftover-debugger".to_string()]);
    }

    #[test]
    fn discover_returns_none_when_no_ancestor_has_one() {
        let dir = unique_tmp_dir("absent");
        assert_eq!(discover(&dir), None);
    }

    #[test]
    fn discover_relative_start_matches_absolutized_delegate() {
        let relative = Path::new("some-nonexistent-relative-subdir");
        let absolutized =
            std::path::absolute(relative).expect("a plain relative path always absolutizes");
        assert_eq!(discover(relative), discover_from(&absolutized));
    }

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
        let path = dir.join("tmt.json");
        fs::write(&path, "{").unwrap();
        let err = load(&path).unwrap_err();
        assert!(matches!(err, ConfigError::Parse { .. }), "{err:?}");
        assert!(
            err.detail().starts_with("invalid JSON: "),
            "{}",
            err.detail()
        );
    }

    #[test]
    fn load_rejects_wrong_shape_without_the_json_syntax_prefix() {
        let dir = unique_tmp_dir("badshape");
        let path = dir.join("tmt.json");
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
        let path = dir.join("tmt.json");
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
        let path = dir.join("tmt.json");
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
        let path = dir.join("tmt.json");
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
        let path = dir.join("tmt.json");
        fs::write(&path, r#"{"lint":{"allow":["dead-rule"]}}"#).unwrap();
        let config = load(&path).unwrap();
        assert_eq!(config.allow, vec!["dead-rule".to_string()]);
    }

    #[test]
    fn load_accepts_an_asm_only_code_in_the_shared_namespace() {
        // A shared `tmt.json` may carry a `.tma`-only code; validating for the
        // `.tmc` half must not reject it (the union namespace).
        let dir = unique_tmp_dir("asmcode");
        let path = dir.join("tmt.json");
        fs::write(&path, r#"{"lint":{"allow":["unreachable-code"]}}"#).unwrap();
        let config = load(&path).unwrap();
        assert_eq!(config.allow, vec!["unreachable-code".to_string()]);
    }

    #[test]
    fn load_accepts_an_empty_object() {
        let dir = unique_tmp_dir("empty");
        let path = dir.join("tmt.json");
        fs::write(&path, "{}").unwrap();
        let config = load(&path).unwrap();
        assert!(config.allow.is_empty());
    }

    #[test]
    fn display_names_the_path_and_the_detail() {
        let dir = unique_tmp_dir("display");
        let path = dir.join("tmt.json");
        fs::write(&path, r#"{"lints":{}}"#).unwrap();
        let err = load(&path).unwrap_err();
        let rendered = err.to_string();
        assert!(rendered.contains(&path.display().to_string()));
        assert!(rendered.contains("unknown key `lints`"));
    }
}
