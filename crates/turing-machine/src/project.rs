//! The `project` section of `tmt.json`: the declared project model —
//! schema, validation, discovery (docs/tmt/project.md (schema)). The
//! strict twin of PM-1's `project.rs`, one crate over, because core
//! stays arch-agnostic and holds no manifest knowledge. Shared by
//! `tmt build` (cli/driver.rs) and the LSP. One loader validates the
//! WHOLE file (both sections) regardless of consumer.

use std::collections::{BTreeMap, HashSet};
use std::path::{Component, Path, PathBuf};

use mtc_core::linker::CallMech;
use serde_json::Value;

use crate::config::ConfigError;
use crate::optimizer::OptLevel;

/// The manifest-driven `tmt build` driver constructs and reads every
/// field on this type (docs/tmt/project.md (schema)); the one-loader
/// `tmt.json` walk (`load_file` -> `validate_manifest`) constructs one
/// too.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Manifest {
    pub stdlib: bool,
    pub sources: Vec<String>,
    pub libraries: Libraries,
    pub profiles: Profiles,
    /// Project-level default lowering for bound calls; a target's own
    /// key overrides it, and `--call-mech` overrides both
    /// (docs/tmt/project.md (call-mech)). `None` = the linker default.
    pub call_mech: Option<CallMech>,
    pub targets: BTreeMap<String, Target>,
}

/// Consumed by the manifest-driven `tmt build` driver's library search
/// (`-L`/`-l` resolution).
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Libraries {
    pub dirs: Vec<String>,
    pub link: Vec<String>,
}

/// Consumed by `Profiles::resolve`, itself consumed by the manifest
/// driver's `--debug`/`--release` preset selection.
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

/// Consumed by the manifest-driven `tmt build` driver's per-target build
/// loop.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Target {
    pub sources: Vec<String>,
    pub libraries: Libraries,
    pub entry: Option<String>,
    pub output: Option<String>,
    pub call_mech: Option<CallMech>,
    pub run: Option<RunSpec>,
}

/// A TM-1 run block. `tmt run` always drives a whole multi-tape band
/// loaded from a `.tmt` snapshot: there is no inline-glyph form, no
/// head, no strict-cells decorator, no tact-profile knob
/// (docs/tmt/cli.md (run)). `tape` is therefore required for the block
/// to be runnable at all.
///
/// Consumed by `tmt build --run`'s manifest-mode settings split.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct RunSpec {
    pub tape: Option<String>,
    pub max_steps: Option<u64>,
    pub no_step_limit: bool,
    pub max_tacts: Option<u64>,
}

/// The two profile names mirror the CLI presets exactly
/// (docs/tmt/cli.md (compile presets): `--debug` = `-g -O0`,
/// `--release` = `-O1 --strip-debugger`); `resolve` layers the
/// manifest's per-key overrides on the preset base. Flags override the
/// result at the driver (flags win — cli/driver.rs).
///
/// Consumed by the manifest-driven `tmt build` driver.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct ResolvedProfile {
    pub opt_level: OptLevel,
    pub debug_info: bool,
    pub strip_debugger: bool,
    pub werror: bool,
}

impl Profiles {
    /// Consumed by the manifest-driven `tmt build` driver's profile
    /// selection.
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
    /// Also used by `validate_manifest`'s own semantic pass (duplicate
    /// effective-source detection); the manifest-driven `tmt build`
    /// driver is the other consumer.
    pub(crate) fn effective_sources(&self, target: &Target) -> Vec<String> {
        self.sources
            .iter()
            .chain(target.sources.iter())
            .cloned()
            .collect()
    }

    /// Consumed by the manifest-driven `tmt build` driver's library
    /// search resolution.
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

    /// Also used by `validate_manifest`'s own semantic pass (cross-target
    /// output-collision detection); the manifest-driven `tmt build`
    /// driver is the other consumer.
    pub(crate) fn output_of(&self, name: &str, target: &Target) -> String {
        target
            .output
            .clone()
            .unwrap_or_else(|| format!("{name}.tmx"))
    }

    /// The union of every target's effective sources, first-seen order,
    /// deduped after lexical normalization, with `.tmo` entries dropped —
    /// the file set a bare `tmt lint` / `tmt fmt` operates on
    /// (docs/tmt/project.md (the declared source set)). Objects carry no
    /// text, so there is nothing in them to lint or format. `.tma` stays:
    /// hand-written assembly is a first-class TM-1 source with both a lint
    /// layer and a formatter.
    pub(crate) fn all_sources(&self) -> Vec<String> {
        let mut seen: HashSet<PathBuf> = HashSet::new();
        let mut out: Vec<String> = Vec::new();
        for target in self.targets.values() {
            for raw in self.effective_sources(target) {
                if raw.ends_with(".tmo") {
                    continue;
                }
                let Ok(normalized) = normalize_rel(&raw) else {
                    continue; // validate_manifest already rejected these
                };
                if seen.insert(normalized) {
                    out.push(raw);
                }
            }
        }
        out
    }

    /// Effective lowering for one target: its own key, else the
    /// project default, else `None` (the linker's own default). The
    /// `--call-mech` flag overrides the result at the driver — flags
    /// win, as the profile keys do (docs/tmt/project.md (call-mech)).
    ///
    /// Consumed by the manifest-driven `tmt build` driver's link-step
    /// lowering selection.
    pub(crate) fn effective_call_mech(&self, target: &Target) -> Option<CallMech> {
        target.call_mech.or(self.call_mech)
    }
}

/// Lexical normalization of a manifest-relative path: rejects absolute
/// paths (portability — a manifest is a committed artifact), folds `.`
/// and interior `..`, KEEPS leading `..` (sources above the manifest
/// directory are allowed — docs/tmt/project.md (path rules)). Lexical
/// only: symlink aliases are not detected, documented not solved.
///
/// Used by `validate_manifest`'s semantic pass, and by the manifest
/// driver and bare `tmt lint`/`tmt fmt` to resolve declared paths
/// against the manifest's directory.
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

/// `call-mech` accepts exactly the three lowercase names `tmt link
/// --call-mech` accepts; the CLI's own parser is the reference for the
/// spelling and the error wording (cli/build.rs::parse_call_mech).
fn parse_call_mech_value(path: &Path, value: &Value) -> Result<CallMech, ConfigError> {
    match as_str(path, value, "call-mech")?.as_str() {
        "mono" => Ok(CallMech::Mono),
        "frames" => Ok(CallMech::Frames),
        "hybrid" => Ok(CallMech::Hybrid),
        other => Err(invalid(
            path,
            format!("unknown call-mech `{other}` (expected one of: mono, frames, hybrid)"),
        )),
    }
}

fn parse_run(path: &Path, value: &Value) -> Result<RunSpec, ConfigError> {
    let obj = as_obj(path, value, "run")?;
    let mut run = RunSpec::default();
    for (key, val) in obj {
        match key.as_str() {
            "tape" => run.tape = Some(as_str(path, val, "tape")?),
            "max-steps" => run.max_steps = Some(as_u64(path, val, "max-steps")?),
            "no-step-limit" => run.no_step_limit = as_bool(path, val, "no-step-limit")?,
            "max-tacts" => run.max_tacts = Some(as_u64(path, val, "max-tacts")?),
            other => return Err(unknown_key(path, other)),
        }
    }
    if run.max_steps.is_some() && run.no_step_limit {
        return Err(invalid(
            path,
            "`max-steps` and `no-step-limit` are mutually exclusive".into(),
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
        call_mech: None,
        run: None,
    };
    for (key, val) in obj {
        match key.as_str() {
            "sources" => target.sources = as_str_array(path, val, "sources")?,
            "libraries" => target.libraries = parse_libraries(path, val)?,
            "entry" => target.entry = Some(as_str(path, val, "entry")?),
            "output" => target.output = Some(as_str(path, val, "output")?),
            "call-mech" => target.call_mech = Some(parse_call_mech_value(path, val)?),
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
/// including the semantic rules (docs/tmt/project.md (schema rules)):
/// target-name charset, per-target effective-list duplicate rejection,
/// cross-target output collision, path normalization/absolute rejection.
///
/// Consumed by the one-loader `tmt.json` walk (`load_file`,
/// docs/tmt/project.md (one loader)) and by the manifest-driven
/// `tmt build` driver.
pub(crate) fn validate_manifest(path: &Path, value: &Value) -> Result<Manifest, ConfigError> {
    let obj = as_obj(path, value, "project")?;
    let mut manifest = Manifest {
        stdlib: true,
        sources: Vec::new(),
        libraries: Libraries::default(),
        profiles: Profiles::default(),
        call_mech: None,
        targets: BTreeMap::new(),
    };
    for (key, val) in obj {
        match key.as_str() {
            "stdlib" => manifest.stdlib = as_bool(path, val, "stdlib")?,
            "sources" => manifest.sources = as_str_array(path, val, "sources")?,
            "libraries" => manifest.libraries = parse_libraries(path, val)?,
            "call-mech" => manifest.call_mech = Some(parse_call_mech_value(path, val)?),
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

/// A whole validated `tmt.json`: the lint allow-list plus the optional
/// project manifest. THE one loader — both consumers (lint config, the
/// project model) validate everything so a typo in either section
/// surfaces no matter who reads the file first.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct TmtFile {
    pub allow: Vec<String>,
    /// Read by `discover_manifest`, the manifest-driven `tmt build`
    /// driver, and bare `tmt lint`/`tmt fmt`'s per-section source-set
    /// discovery.
    pub manifest: Option<Manifest>,
}

pub(crate) fn load_file(path: &Path) -> Result<TmtFile, ConfigError> {
    let text = std::fs::read_to_string(path).map_err(|e| ConfigError::Io {
        path: path.to_path_buf(),
        message: e.to_string(),
    })?;
    let value: Value = serde_json::from_str(&text).map_err(|e| ConfigError::Parse {
        path: path.to_path_buf(),
        message: format!("invalid JSON: {e}"),
    })?;
    let root = value
        .as_object()
        .ok_or_else(|| parse_err(path, "top-level value must be a JSON object"))?;

    let mut file = TmtFile {
        allow: Vec::new(),
        manifest: None,
    };
    for (key, val) in root {
        match key.as_str() {
            "lint" => file.allow = parse_lint(path, val)?,
            "project" => file.manifest = Some(validate_manifest(path, val)?),
            other => return Err(unknown_key(path, other)),
        }
    }
    Ok(file)
}

/// The lint section walk, moved verbatim from `config::load` (which now
/// delegates here): `lint.allow` only, entries validated against the
/// rule catalog.
fn parse_lint(path: &Path, value: &Value) -> Result<Vec<String>, ConfigError> {
    let lint_obj = as_obj(path, value, "lint")?;
    let mut allow: Vec<String> = Vec::new();
    for (lkey, lval) in lint_obj {
        if lkey != "allow" {
            return Err(unknown_key(path, lkey));
        }
        allow = as_str_array(path, lval, "lint.allow")?;
    }
    match crate::lint::validate_allow(&allow) {
        Ok(()) => {}
        Err(crate::lint::LintError::UnknownAllowCode(code)) => {
            return Err(ConfigError::UnknownAllowCode {
                path: path.to_path_buf(),
                code,
            });
        }
        Err(other) => unreachable!("validate_allow only ever returns UnknownAllowCode: {other}"),
    }
    Ok(allow)
}

/// Nearest ancestor `tmt.json` that HAS a `project` section — the
/// per-section discovery rule (docs/tmt/project.md (discovery)): a
/// lint-only file on the walk is transparent to THIS walk (while
/// `config::discover` still stops at it for lint). A malformed
/// candidate is an error, not a skip: we cannot know whether it had a
/// project section.
///
/// Not yet called outside this module's own tests — the manifest-driven
/// `tmt build` driver and `tmt lint`/`tmt fmt`'s per-section source-set
/// discovery are the future production consumers.
pub(crate) fn discover_manifest(start: &Path) -> Result<Option<(PathBuf, Manifest)>, ConfigError> {
    let start = if start.as_os_str().is_empty() {
        Path::new(".")
    } else {
        start
    };
    let Ok(abs) = std::path::absolute(start) else {
        return Ok(None);
    };
    let mut dir = Some(abs.as_path());
    while let Some(d) = dir {
        let candidate = d.join("tmt.json");
        if candidate.is_file() {
            let file = load_file(&candidate)?;
            if let Some(manifest) = file.manifest {
                return Ok(Some((candidate, manifest)));
            }
        }
        dir = d.parent();
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::path::Path;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn v(path_json: serde_json::Value) -> Result<Manifest, crate::config::ConfigError> {
        validate_manifest(Path::new("/x/tmt.json"), &path_json)
    }

    /// A fresh scratch directory under `std::env::temp_dir()`, unique per
    /// call (process id + an atomic counter — this crate has no tempfile
    /// dependency, matching the zero-new-deps constraint). Mirrors
    /// `config::tests::unique_tmp_dir`, local to this file per this
    /// crate's no-shared-test-support convention.
    fn unique_tmp_dir(label: &str) -> PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "tmt-project-test-{label}-{}-{n}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn discover_manifest_skips_lint_only_files_but_lint_walk_stops_at_them() {
        let root = unique_tmp_dir("per-section");
        let sub = root.join("sub");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(
            root.join("tmt.json"),
            r#"{ "project": { "targets": { "app": { "sources": ["m.tmc"] } } } }"#,
        )
        .unwrap();
        std::fs::write(
            sub.join("tmt.json"),
            r#"{ "lint": { "allow": ["leftover-debugger"] } }"#,
        )
        .unwrap();

        // Project walk: the nested lint-only file is transparent.
        let (found, manifest) = discover_manifest(&sub).unwrap().expect("project above");
        assert_eq!(found, root.join("tmt.json"));
        assert!(manifest.targets.contains_key("app"));

        // Lint walk: unchanged — nearest file wins, even lint-only.
        assert_eq!(crate::config::discover(&sub), Some(sub.join("tmt.json")));
    }

    #[test]
    fn one_loader_a_broken_project_section_fails_the_lint_load_too() {
        let dir = unique_tmp_dir("one-loader");
        let path = dir.join("tmt.json");
        std::fs::write(
            &path,
            r#"{ "lint": { "allow": [] }, "project": { "targets": {} } }"#,
        )
        .unwrap();
        assert!(
            crate::config::load(&path).is_err(),
            "empty targets must fail even for lint"
        );
        assert!(load_file(&path).is_err());
    }

    #[test]
    fn load_file_reads_both_sections() {
        let dir = unique_tmp_dir("both");
        let path = dir.join("tmt.json");
        std::fs::write(
            &path,
            r#"{ "lint": { "allow": ["leftover-debugger"] },
                "project": { "targets": { "app": { "sources": ["m.tmc"] } } } }"#,
        )
        .unwrap();
        let file = load_file(&path).unwrap();
        assert_eq!(file.allow, vec!["leftover-debugger".to_string()]);
        assert!(file.manifest.is_some());
    }

    #[test]
    fn discover_manifest_errors_on_a_malformed_candidate() {
        let dir = unique_tmp_dir("malformed-walk");
        std::fs::write(dir.join("tmt.json"), "{").unwrap();
        assert!(discover_manifest(&dir).is_err());
    }

    #[test]
    fn minimal_manifest_one_target_defaults() {
        let m = v(json!({ "targets": { "app": { "sources": ["main.tmc"] } } })).unwrap();
        assert!(m.stdlib);
        let t = &m.targets["app"];
        assert_eq!(m.effective_sources(t), vec!["main.tmc".to_string()]);
        assert_eq!(m.output_of("app", t), "app.tmx");
        assert!(t.entry.is_none() && t.run.is_none());
    }

    #[test]
    fn unknown_keys_error_at_every_level() {
        for bad in [
            json!({ "target": {} }),
            json!({ "targets": { "a": { "sources": [], "outputs": "x" } } }),
            json!({ "targets": { "a": { "sources": ["m.tmc"] } }, "profiles": { "debug": { "opt2": "O1" } } }),
            json!({ "targets": { "a": { "sources": ["m.tmc"], "run": { "tapes": " " } } } }),
            json!({ "targets": { "a": { "sources": ["m.tmc"] } }, "libraries": { "dir": [] } }),
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
            let err = v(json!({ "targets": { bad: { "sources": ["m.tmc"] } } })).unwrap_err();
            assert!(
                matches!(err, crate::config::ConfigError::Invalid { .. }),
                "{bad}: {err:?}"
            );
        }
        assert!(v(json!({ "targets": { "ok-Name_2": { "sources": ["m.tmc"] } } })).is_ok());
    }

    #[test]
    fn duplicate_effective_source_is_an_error_after_normalization() {
        let err = v(json!({
            "sources": ["src/../m.tmc"],
            "targets": { "a": { "sources": ["m.tmc"] } }
        }))
        .unwrap_err();
        assert!(err.detail().contains("m.tmc"), "{}", err.detail());
    }

    #[test]
    fn colliding_target_outputs_error() {
        let err = v(json!({ "targets": {
            "a": { "sources": ["a.tmc"], "output": "out.tmx" },
            "b": { "sources": ["b.tmc"], "output": "./out.tmx" }
        }}))
        .unwrap_err();
        assert!(err.detail().contains("out.tmx"), "{}", err.detail());
    }

    #[test]
    fn absolute_paths_rejected_parent_traversal_allowed() {
        assert!(v(json!({ "targets": { "a": { "sources": ["/abs/m.tmc"] } } })).is_err());
        assert!(v(json!({ "targets": { "a": { "sources": ["../shared/m.tmc"] } } })).is_ok());
        assert_eq!(
            normalize_rel("../shared/../shared/m.tmc").unwrap(),
            PathBuf::from("../shared/m.tmc")
        );
    }

    #[test]
    fn run_block_accepts_only_the_tmt_keys() {
        let base = |run: serde_json::Value| json!({ "targets": { "a": { "sources": ["m.tmc"], "run": run } } });
        // PM-1-only keys are unknown here — the run block mirrors `tmt run`.
        for bad in ["head", "strict-cells", "tape-block", "tact-profile"] {
            let err = v(base(json!({ bad: 1 }))).unwrap_err();
            assert!(
                matches!(err, crate::config::ConfigError::UnknownKey { .. }),
                "{bad}: {err:?}"
            );
        }
        assert!(v(base(json!({ "tape": "tapes/in.tmt", "max-tacts": 500000 }))).is_ok());
        assert!(v(base(json!({ "tape": "t.tmt", "no-step-limit": true }))).is_ok());
        assert!(
            v(base(
                json!({ "tape": "t.tmt", "no-step-limit": true, "max-steps": 10 })
            ))
            .is_err(),
            "max-steps and no-step-limit contradict"
        );
        assert!(
            v(base(json!({}))).is_ok(),
            "an empty run block parses (but cannot --run)"
        );
    }

    #[test]
    fn call_mech_parses_at_both_levels_and_target_wins() {
        let m = v(json!({
            "call-mech": "frames",
            "targets": {
                "a": { "sources": ["a.tmc"] },
                "b": { "sources": ["b.tmc"], "call-mech": "mono" }
            }
        }))
        .unwrap();
        assert_eq!(
            m.effective_call_mech(&m.targets["a"]),
            Some(CallMech::Frames)
        );
        assert_eq!(m.effective_call_mech(&m.targets["b"]), Some(CallMech::Mono));

        let err = v(json!({
            "call-mech": "monolithic",
            "targets": { "a": { "sources": ["a.tmc"] } }
        }))
        .unwrap_err();
        assert!(
            err.detail().contains("mono, frames, hybrid"),
            "{}",
            err.detail()
        );
    }

    #[test]
    fn default_output_is_tmx() {
        let m = v(json!({ "targets": { "utm": { "sources": ["utm.tmc"] } } })).unwrap();
        assert_eq!(m.output_of("utm", &m.targets["utm"]), "utm.tmx");
    }

    #[test]
    fn profiles_only_debug_and_release_and_resolve_applies_overrides() {
        assert!(
            v(json!({
                "targets": { "a": { "sources": ["m.tmc"] } },
                "profiles": { "bench": {} }
            }))
            .is_err()
        );
        let m = v(json!({
            "targets": { "a": { "sources": ["m.tmc"] } },
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
            "sources": ["shared.tmc"],
            "libraries": { "dirs": ["libs"], "link": ["base"] },
            "targets": { "a": {
                "sources": ["a.tmc"],
                "libraries": { "dirs": ["alibs"], "link": ["extra"] }
            } }
        }))
        .unwrap();
        let t = &m.targets["a"];
        assert_eq!(
            m.effective_sources(t),
            vec!["shared.tmc".to_string(), "a.tmc".to_string()]
        );
        let libs = m.effective_libraries(t);
        assert_eq!(libs.dirs, vec!["libs".to_string(), "alibs".to_string()]);
        assert_eq!(libs.link, vec!["base".to_string(), "extra".to_string()]);
    }

    #[test]
    fn all_sources_dedupes_across_targets_and_drops_objects() {
        let m = v(json!({
            "sources": ["shared.tmc"],
            "targets": {
                "a": { "sources": ["a.tmc", "vendor.tmo"] },
                "b": { "sources": ["b.tmc"] }
            }
        }))
        .unwrap();
        assert_eq!(
            m.all_sources(),
            vec![
                "shared.tmc".to_string(),
                "a.tmc".to_string(),
                "b.tmc".to_string()
            ],
            "shared.tmc appears once, the .tmo is dropped"
        );
    }

    /// The one divergence from PM's `all_sources`: `.tma` is a first-class
    /// hand-written source on the TM side, with both a lint layer and a
    /// formatter, so it stays in the set. Only `.tmo` — an object, carrying
    /// no text — is dropped.
    #[test]
    fn all_sources_keeps_tma_and_drops_tmo() {
        let m = v(json!({
            "targets": { "a": { "sources": ["a.tmc", "tables.tma", "vendor.tmo"] } }
        }))
        .unwrap();
        assert_eq!(
            m.all_sources(),
            vec!["a.tmc".to_string(), "tables.tma".to_string()],
            ".tma is a lintable/formattable source; .tmo is not"
        );
    }
}
