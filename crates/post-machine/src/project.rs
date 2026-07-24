//! The `project` section of `pmt.json`: the declared project model —
//! schema, validation, discovery (docs/pmt/project.md (schema)). Shared
//! by `pmt build` (cli/driver.rs) and the LSP. One loader validates the
//! WHOLE file (both sections) regardless of consumer, so the lint walk
//! and the project walk can never disagree about well-formedness.

#![allow(dead_code)] // remove in Task 2, once the loader consumes these types.

use std::collections::{BTreeMap, HashSet};
use std::path::{Component, Path, PathBuf};

use serde_json::Value;

use crate::config::ConfigError;
use crate::optimizer::OptLevel;

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Manifest {
    pub stdlib: bool,
    pub sources: Vec<String>,
    pub libraries: Libraries,
    pub profiles: Profiles,
    /// BTreeMap: alphabetical iteration IS the documented cross-target
    /// build order (targets are independent; serde_json has no
    /// preserve_order feature in this tree — zero-new-deps).
    pub targets: BTreeMap<String, Target>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Libraries {
    pub dirs: Vec<String>,
    pub link: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Profiles {
    pub debug: ProfileOverrides,
    pub release: ProfileOverrides,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct ProfileOverrides {
    pub opt: Option<OptLevel>,
    pub debug_info: Option<bool>,
    pub strip_debugger: Option<bool>,
    pub werror: Option<bool>,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Target {
    pub sources: Vec<String>,
    pub libraries: Libraries,
    pub entry: Option<String>,
    pub output: Option<String>,
    pub run: Option<RunSpec>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct RunSpec {
    pub tape: Option<String>,
    pub tape_block: Option<String>,
    pub head: Option<i64>,
    pub strict_cells: bool,
    pub max_steps: Option<u64>,
    pub max_tacts: Option<u64>,
    pub tact_profile: Option<[u32; 3]>,
}

/// The two profile names mirror the CLI presets exactly
/// (docs/pmt/cli.md (compile presets): `--debug` = `-g -O0`,
/// `--release` = `-O1 --strip-debugger`); `resolve` layers the
/// manifest's per-key overrides on the preset base. Flags override the
/// result at the driver (flags win — cli/driver.rs).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct ResolvedProfile {
    pub opt_level: OptLevel,
    pub debug_info: bool,
    pub strip_debugger: bool,
    pub werror: bool,
}

impl Profiles {
    pub(crate) fn resolve(&self, release: bool) -> ResolvedProfile {
        let (base, over) = if release {
            (
                ResolvedProfile {
                    opt_level: OptLevel::O1,
                    debug_info: false,
                    strip_debugger: true,
                    werror: false,
                },
                &self.release,
            )
        } else {
            (
                ResolvedProfile {
                    opt_level: OptLevel::O0,
                    debug_info: true,
                    strip_debugger: false,
                    werror: false,
                },
                &self.debug,
            )
        };
        ResolvedProfile {
            opt_level: over.opt.unwrap_or(base.opt_level),
            debug_info: over.debug_info.unwrap_or(base.debug_info),
            strip_debugger: over.strip_debugger.unwrap_or(base.strip_debugger),
            werror: over.werror.unwrap_or(base.werror),
        }
    }
}

impl Manifest {
    pub(crate) fn effective_sources(&self, target: &Target) -> Vec<String> {
        self.sources
            .iter()
            .chain(target.sources.iter())
            .cloned()
            .collect()
    }

    pub(crate) fn effective_libraries(&self, target: &Target) -> Libraries {
        Libraries {
            dirs: self
                .libraries
                .dirs
                .iter()
                .chain(target.libraries.dirs.iter())
                .cloned()
                .collect(),
            link: self
                .libraries
                .link
                .iter()
                .chain(target.libraries.link.iter())
                .cloned()
                .collect(),
        }
    }

    pub(crate) fn output_of(&self, name: &str, target: &Target) -> String {
        target
            .output
            .clone()
            .unwrap_or_else(|| format!("{name}.pmx"))
    }
}

/// Lexical normalization of a manifest-relative path: rejects absolute
/// paths (portability — a manifest is a committed artifact), folds `.`
/// and interior `..`, KEEPS leading `..` (sources above the manifest
/// directory are allowed — docs/pmt/project.md (path rules)). Lexical
/// only: symlink aliases are not detected, documented not solved.
pub(crate) fn normalize_rel(path_str: &str) -> Result<PathBuf, String> {
    let p = Path::new(path_str);
    let absolute_err = || {
        format!(
            "absolute path `{path_str}` — manifest paths are relative to the manifest's directory"
        )
    };
    if p.is_absolute() {
        return Err(absolute_err());
    }
    let mut parts: Vec<String> = Vec::new();
    for comp in p.components() {
        match comp {
            Component::CurDir => {}
            Component::Normal(c) => parts.push(c.to_string_lossy().into_owned()),
            Component::ParentDir => {
                if parts.last().is_some_and(|last| last != "..") {
                    parts.pop();
                } else {
                    parts.push("..".to_string());
                }
            }
            Component::RootDir | Component::Prefix(_) => return Err(absolute_err()),
        }
    }
    if parts.is_empty() {
        return Err(format!("path `{path_str}` names no file"));
    }
    Ok(parts.iter().collect())
}

fn invalid(path: &Path, message: String) -> ConfigError {
    ConfigError::Invalid {
        path: path.to_path_buf(),
        message,
    }
}

fn parse_err(path: &Path, message: &str) -> ConfigError {
    ConfigError::Parse {
        path: path.to_path_buf(),
        message: message.to_string(),
    }
}

fn unknown_key(path: &Path, key: &str) -> ConfigError {
    ConfigError::UnknownKey {
        path: path.to_path_buf(),
        key: key.to_string(),
    }
}

fn as_obj<'v>(
    path: &Path,
    value: &'v Value,
    what: &str,
) -> Result<&'v serde_json::Map<String, Value>, ConfigError> {
    value
        .as_object()
        .ok_or_else(|| parse_err(path, &format!("`{what}` must be a JSON object")))
}

fn as_str_array(path: &Path, value: &Value, what: &str) -> Result<Vec<String>, ConfigError> {
    let complain = || parse_err(path, &format!("`{what}` must be an array of strings"));
    let arr = value.as_array().ok_or_else(complain)?;
    arr.iter()
        .map(|item| item.as_str().map(str::to_string).ok_or_else(complain))
        .collect()
}

fn as_bool(path: &Path, value: &Value, what: &str) -> Result<bool, ConfigError> {
    value
        .as_bool()
        .ok_or_else(|| parse_err(path, &format!("`{what}` must be a boolean")))
}

fn as_u64(path: &Path, value: &Value, what: &str) -> Result<u64, ConfigError> {
    value
        .as_u64()
        .ok_or_else(|| parse_err(path, &format!("`{what}` must be a non-negative integer")))
}

fn as_str(path: &Path, value: &Value, what: &str) -> Result<String, ConfigError> {
    value
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| parse_err(path, &format!("`{what}` must be a string")))
}

fn valid_target_name(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    first.is_ascii_alphanumeric()
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

fn parse_libraries(path: &Path, value: &Value) -> Result<Libraries, ConfigError> {
    let obj = as_obj(path, value, "libraries")?;
    let mut libs = Libraries::default();
    for (key, val) in obj {
        match key.as_str() {
            "dirs" => libs.dirs = as_str_array(path, val, "libraries.dirs")?,
            "link" => libs.link = as_str_array(path, val, "libraries.link")?,
            other => return Err(unknown_key(path, other)),
        }
    }
    Ok(libs)
}

fn parse_profile(path: &Path, name: &str, value: &Value) -> Result<ProfileOverrides, ConfigError> {
    let obj = as_obj(path, value, &format!("profiles.{name}"))?;
    let mut over = ProfileOverrides::default();
    for (key, val) in obj {
        match key.as_str() {
            "opt" => {
                over.opt = Some(match as_str(path, val, "opt")?.as_str() {
                    "O0" => OptLevel::O0,
                    "O1" => OptLevel::O1,
                    other => {
                        return Err(invalid(
                            path,
                            format!("unknown opt level `{other}` (O0 | O1)"),
                        ));
                    }
                });
            }
            "debug-info" => over.debug_info = Some(as_bool(path, val, "debug-info")?),
            "strip-debugger" => over.strip_debugger = Some(as_bool(path, val, "strip-debugger")?),
            "werror" => over.werror = Some(as_bool(path, val, "werror")?),
            other => return Err(unknown_key(path, other)),
        }
    }
    Ok(over)
}

fn parse_run(path: &Path, value: &Value) -> Result<RunSpec, ConfigError> {
    let obj = as_obj(path, value, "run")?;
    let mut run = RunSpec::default();
    for (key, val) in obj {
        match key.as_str() {
            "tape" => run.tape = Some(as_str(path, val, "tape")?),
            "tape-block" => run.tape_block = Some(as_str(path, val, "tape-block")?),
            "head" => {
                run.head = Some(
                    val.as_i64()
                        .ok_or_else(|| parse_err(path, "`head` must be an integer"))?,
                );
            }
            "strict-cells" => run.strict_cells = as_bool(path, val, "strict-cells")?,
            "max-steps" => run.max_steps = Some(as_u64(path, val, "max-steps")?),
            "max-tacts" => run.max_tacts = Some(as_u64(path, val, "max-tacts")?),
            "tact-profile" => {
                let arr = val
                    .as_array()
                    .ok_or_else(|| parse_err(path, "`tact-profile` must be [move, read, write]"))?;
                let [m, r, w] = arr.as_slice() else {
                    return Err(parse_err(
                        path,
                        "`tact-profile` must be [move, read, write]",
                    ));
                };
                let cost = |v: &Value| -> Result<u32, ConfigError> {
                    v.as_u64()
                        .and_then(|n| u32::try_from(n).ok())
                        .ok_or_else(|| parse_err(path, "`tact-profile` costs must be u32"))
                };
                run.tact_profile = Some([cost(m)?, cost(r)?, cost(w)?]);
            }
            other => return Err(unknown_key(path, other)),
        }
    }
    if run.tape.is_some() && run.tape_block.is_some() {
        return Err(invalid(
            path,
            "`tape` and `tape-block` are mutually exclusive".into(),
        ));
    }
    if run.head.is_some() && run.tape.is_none() {
        return Err(invalid(
            path,
            "`head` is only meaningful alongside `tape`".into(),
        ));
    }
    Ok(run)
}

fn parse_target(path: &Path, name: &str, value: &Value) -> Result<Target, ConfigError> {
    let obj = as_obj(path, value, &format!("targets.{name}"))?;
    let mut target = Target {
        sources: Vec::new(),
        libraries: Libraries::default(),
        entry: None,
        output: None,
        run: None,
    };
    for (key, val) in obj {
        match key.as_str() {
            "sources" => target.sources = as_str_array(path, val, "sources")?,
            "libraries" => target.libraries = parse_libraries(path, val)?,
            "entry" => target.entry = Some(as_str(path, val, "entry")?),
            "output" => target.output = Some(as_str(path, val, "output")?),
            "run" => target.run = Some(parse_run(path, val)?),
            other => return Err(unknown_key(path, other)),
        }
    }
    if let Some(entry) = &target.entry
        && entry.is_empty()
    {
        return Err(invalid(
            path,
            format!("target `{name}`: `entry` must not be empty"),
        ));
    }
    Ok(target)
}

/// Validates a whole `project` section value into a [`Manifest`],
/// including the semantic rules (docs/pmt/project.md (schema rules)):
/// target-name charset, per-target effective-list duplicate rejection,
/// cross-target output collision, path normalization/absolute rejection.
pub(crate) fn validate_manifest(path: &Path, value: &Value) -> Result<Manifest, ConfigError> {
    let obj = as_obj(path, value, "project")?;
    let mut manifest = Manifest {
        stdlib: true,
        sources: Vec::new(),
        libraries: Libraries::default(),
        profiles: Profiles::default(),
        targets: BTreeMap::new(),
    };
    for (key, val) in obj {
        match key.as_str() {
            "stdlib" => manifest.stdlib = as_bool(path, val, "stdlib")?,
            "sources" => manifest.sources = as_str_array(path, val, "sources")?,
            "libraries" => manifest.libraries = parse_libraries(path, val)?,
            "profiles" => {
                let profiles = as_obj(path, val, "profiles")?;
                for (pname, pval) in profiles {
                    match pname.as_str() {
                        "debug" => manifest.profiles.debug = parse_profile(path, pname, pval)?,
                        "release" => manifest.profiles.release = parse_profile(path, pname, pval)?,
                        other => {
                            return Err(invalid(
                                path,
                                format!("unknown profile `{other}` (debug | release)"),
                            ));
                        }
                    }
                }
            }
            "targets" => {
                let targets = as_obj(path, val, "targets")?;
                for (tname, tval) in targets {
                    if !valid_target_name(tname) {
                        return Err(invalid(
                            path,
                            format!("bad target name `{tname}` (want [A-Za-z0-9][A-Za-z0-9_-]*)"),
                        ));
                    }
                    manifest
                        .targets
                        .insert(tname.clone(), parse_target(path, tname, tval)?);
                }
            }
            other => return Err(unknown_key(path, other)),
        }
    }
    if manifest.targets.is_empty() {
        return Err(invalid(
            path,
            "`project` needs at least one entry in `targets`".into(),
        ));
    }

    // Semantic pass: normalize every declared path (rejecting absolute
    // ones), reject duplicate effective sources per target, reject
    // colliding outputs across targets.
    let norm = |raw: &str| normalize_rel(raw).map_err(|message| invalid(path, message));
    for raw in manifest
        .sources
        .iter()
        .chain(manifest.libraries.dirs.iter())
    {
        norm(raw)?;
    }
    let mut outputs: HashSet<PathBuf> = HashSet::new();
    for (name, target) in &manifest.targets {
        let mut seen: HashSet<PathBuf> = HashSet::new();
        for raw in manifest.effective_sources(target) {
            let normalized = norm(&raw)?;
            if !seen.insert(normalized.clone()) {
                return Err(invalid(
                    path,
                    format!(
                        "target `{name}`: source `{}` appears twice in the effective list",
                        normalized.display()
                    ),
                ));
            }
        }
        for raw in &target.libraries.dirs {
            norm(raw)?;
        }
        if let Some(rs) = &target.run
            && let Some(block) = &rs.tape_block
        {
            norm(block)?;
        }
        let output = norm(&manifest.output_of(name, target))?;
        if !outputs.insert(output.clone()) {
            return Err(invalid(
                path,
                format!(
                    "two targets resolve to the same output `{}`",
                    output.display()
                ),
            ));
        }
    }
    Ok(manifest)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::path::Path;

    fn v(path_json: serde_json::Value) -> Result<Manifest, crate::config::ConfigError> {
        validate_manifest(Path::new("/x/pmt.json"), &path_json)
    }

    #[test]
    fn minimal_manifest_one_target_defaults() {
        let m = v(json!({ "targets": { "app": { "sources": ["main.pmc"] } } })).unwrap();
        assert!(m.stdlib);
        let t = &m.targets["app"];
        assert_eq!(m.effective_sources(t), vec!["main.pmc".to_string()]);
        assert_eq!(m.output_of("app", t), "app.pmx");
        assert!(t.entry.is_none() && t.run.is_none());
    }

    #[test]
    fn unknown_keys_error_at_every_level() {
        for bad in [
            json!({ "target": {} }),
            json!({ "targets": { "a": { "sources": [], "outputs": "x" } } }),
            json!({ "targets": { "a": { "sources": ["m.pmc"] } }, "profiles": { "debug": { "opt2": "O1" } } }),
            json!({ "targets": { "a": { "sources": ["m.pmc"], "run": { "tapes": " " } } } }),
            json!({ "targets": { "a": { "sources": ["m.pmc"] } }, "libraries": { "dir": [] } }),
        ] {
            let err = v(bad).unwrap_err();
            assert!(
                matches!(err, crate::config::ConfigError::UnknownKey { .. }),
                "{err:?}"
            );
        }
    }

    #[test]
    fn targets_required_and_nonempty() {
        assert!(v(json!({})).is_err());
        assert!(v(json!({ "targets": {} })).is_err());
    }

    #[test]
    fn target_name_charset_enforced() {
        for bad in ["a.b", "-x", "", "sp ace"] {
            let err = v(json!({ "targets": { bad: { "sources": ["m.pmc"] } } })).unwrap_err();
            assert!(
                matches!(err, crate::config::ConfigError::Invalid { .. }),
                "{bad}: {err:?}"
            );
        }
        assert!(v(json!({ "targets": { "ok-Name_2": { "sources": ["m.pmc"] } } })).is_ok());
    }

    #[test]
    fn duplicate_effective_source_is_an_error_after_normalization() {
        let err = v(json!({
            "sources": ["src/../m.pmc"],
            "targets": { "a": { "sources": ["m.pmc"] } }
        }))
        .unwrap_err();
        assert!(err.detail().contains("m.pmc"), "{}", err.detail());
    }

    #[test]
    fn colliding_target_outputs_error() {
        let err = v(json!({ "targets": {
            "a": { "sources": ["a.pmc"], "output": "out.pmx" },
            "b": { "sources": ["b.pmc"], "output": "./out.pmx" }
        }}))
        .unwrap_err();
        assert!(err.detail().contains("out.pmx"), "{}", err.detail());
    }

    #[test]
    fn absolute_paths_rejected_parent_traversal_allowed() {
        assert!(v(json!({ "targets": { "a": { "sources": ["/abs/m.pmc"] } } })).is_err());
        assert!(v(json!({ "targets": { "a": { "sources": ["../shared/m.pmc"] } } })).is_ok());
        assert_eq!(
            normalize_rel("../shared/../shared/m.pmc").unwrap(),
            PathBuf::from("../shared/m.pmc")
        );
    }

    #[test]
    fn run_block_tape_xor_tape_block_head_requires_tape() {
        let base = |run: serde_json::Value| json!({ "targets": { "a": { "sources": ["m.pmc"], "run": run } } });
        assert!(v(base(json!({ "tape": " *", "tape-block": "t.pmt" }))).is_err());
        assert!(v(base(json!({ "tape-block": "t.pmt", "head": 3 }))).is_err());
        assert!(
            v(base(
                json!({ "tape": " *", "head": 3, "strict-cells": true })
            ))
            .is_ok()
        );
        assert!(v(base(json!({}))).is_ok(), "empty run block = run defaults");
    }

    #[test]
    fn profiles_only_debug_and_release_and_resolve_applies_overrides() {
        assert!(
            v(json!({
                "targets": { "a": { "sources": ["m.pmc"] } },
                "profiles": { "bench": {} }
            }))
            .is_err()
        );
        let m = v(json!({
            "targets": { "a": { "sources": ["m.pmc"] } },
            "profiles": { "release": { "werror": true, "debug-info": true } }
        }))
        .unwrap();
        let r = m.profiles.resolve(true);
        assert!(r.werror && r.debug_info && r.strip_debugger);
        assert_eq!(r.opt_level, crate::optimizer::OptLevel::O1);
        let d = m.profiles.resolve(false);
        assert!(!d.werror && d.debug_info && !d.strip_debugger);
        assert_eq!(d.opt_level, crate::optimizer::OptLevel::O0);
    }

    #[test]
    fn shared_and_per_target_lists_concatenate_in_order() {
        let m = v(json!({
            "sources": ["shared.pmc"],
            "libraries": { "dirs": ["libs"], "link": ["base"] },
            "targets": { "a": {
                "sources": ["a.pmc"],
                "libraries": { "dirs": ["alibs"], "link": ["extra"] }
            } }
        }))
        .unwrap();
        let t = &m.targets["a"];
        assert_eq!(
            m.effective_sources(t),
            vec!["shared.pmc".to_string(), "a.pmc".to_string()]
        );
        let libs = m.effective_libraries(t);
        assert_eq!(libs.dirs, vec!["libs".to_string(), "alibs".to_string()]);
        assert_eq!(libs.link, vec!["base".to_string(), "extra".to_string()]);
    }
}
