# Project Manifest + `pmt build` (Plan 1 of 3) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship the `pmt.json` `project` section (schema 0.2) and the
`pmt build` driver — both modes, `--run`, `--list-targets`, shell
completion — per the spec
`docs/superpowers/specs/2026-07-12-project-manifest-and-build-design.md`.
Plan 2 (LSP overlay) and Plan 3 (editors) follow off the same spec.

**Architecture:** Manifest types + validation + discovery live in a new
`crates/post-machine/src/project.rs`, which becomes the ONE loader that
validates a whole `pmt.json` (both sections) regardless of consumer;
`config.rs` keeps its public shape but delegates. The driver is a new
`cli/driver.rs` that composes the existing compile/asm/link/run
internals in memory. One small core change: `LinkOptions.entry`.

**Tech Stack:** Rust, `serde_json` only (zero new deps), hand-rolled
CLI (`cli::Args`), in-process integration tests via
`mtc_post_machine::cli::execute` + spawned-binary tests via
`env!("CARGO_BIN_EXE_pmt")` for cwd-dependent discovery.

## Global Constraints

- **Zero new dependencies** — `serde/serde_json` runtime, `proptest` dev-only. No tempfile, no clap, no glob crates.
- **Thin-renderer rule** — library code never prints; every terminal byte originates in `cli/` behind structured reports.
- **Strict unknown-key errors** in `pmt.json` at every level, in `config.rs`'s precise style ("unknown key `X`").
- **Manifest paths**: relative to the manifest's directory; `../` allowed; absolute rejected; lexical normalization only.
- **`pmt.json` schema version becomes 0.2** (0.1 = retroactive lint-only shape); document in `docs/project.md`, not in the file itself.
- **Published docs** (README, `docs/`) are forge-agnostic and ref-free: no issue/PR numbers, no URLs.
- **Quality gates**: `cargo clippy --workspace --all-targets -- -D warnings` and `cargo fmt --check` must pass at every commit.
- **Commit style**: conventional commits with scope (`feat(post-machine):`, `feat(core):`, `test(post-machine):`, `docs:`).
- **Commit permission**: the user's standing rule forbids commits without explicit permission. At execution start, ask the user for blanket per-task commit permission for this plan; if not granted, skip every commit step and stop after each task for review.

---

### Task 1: `LinkOptions.entry` in core

**Files:**
- Modify: `crates/core/src/linker/mod.rs` (LinkOptions ~line 54, Display ~line 37, `link()` ~line 121)
- Modify: `crates/core/src/linker/resolve.rs` (`resolve()` signature; entry lookup ~line 80; its `mod tests`)
- Modify: `crates/post-machine/src/cli/build.rs:267` (`LinkOptions` literal)
- Modify: `crates/post-machine/tests/visibility_programs.rs:363,371`

**Interfaces:**
- Produces: `LinkOptions { relax: bool, entry: Option<String> }` (`None` = `"main"`); `LinkError::NoEntrySymbol(String)` carrying the configured entry name. Task 5 passes `entry: Some(target.entry.clone())`.

- [ ] **Step 1: Write the failing test** (append to `crates/core/src/linker/resolve.rs` `mod tests`, matching the existing `obj(0x7E, …)` helper style there)

```rust
#[test]
fn custom_entry_starts_the_bfs_and_missing_entry_names_it() {
    let a = obj(0x7E, &[("start", &["helper"]), ("helper", &[]), ("main", &[])]);
    let r = resolve(&[a.clone()], &[], "start").unwrap();
    assert_eq!(r.order[0].name, "start");
    let names: Vec<&str> = r.order.iter().map(|s| s.name.as_str()).collect();
    assert!(!names.contains(&"main"), "unreached default entry must drop");

    let err = resolve(&[a], &[], "absent").unwrap_err();
    assert_eq!(err, LinkError::NoEntrySymbol("absent".to_string()));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p mtc-core custom_entry_starts_the_bfs 2>&1 | tail -20`
Expected: COMPILE ERROR (`resolve` takes 2 args; `NoEntrySymbol` is a unit variant)

- [ ] **Step 3: Implement**

In `crates/core/src/linker/mod.rs`:

```rust
pub struct LinkOptions {
    pub relax: bool,
    /// Entry symbol the reachability BFS starts from; `None` = `"main"`.
    pub entry: Option<String>,
}

impl Default for LinkOptions {
    fn default() -> Self {
        Self { relax: true, entry: None }
    }
}
```

Variant: `NoEntrySymbol(String)`. Display arm:

```rust
Self::NoEntrySymbol(entry) => write!(f, "no `{entry}` entry symbol"),
```

In `link()`: `let resolved = resolve::resolve(objects, libraries, options.entry.as_deref().unwrap_or("main"))?;`

In `resolve.rs`, add the parameter and use it at the lookup:

```rust
pub(super) fn resolve<'a>(
    objects: &'a [ObjectFile],
    libraries: &'a [ObjectFile],
    entry: &str,
) -> Result<Resolved<'a>, LinkError> {
```

(keep the existing signature's exact generics/return — only add `entry: &str`) and:

```rust
    let Some(&main_site) = namespace.get(entry) else {
        return Err(LinkError::NoEntrySymbol(entry.to_string()));
    };
```

Fix every existing `resolve(…, …)` call in `resolve.rs` tests to pass `"main"`. Fix every `LinkOptions { relax }` struct literal — `crates/post-machine/src/cli/build.rs:267` becomes `LinkOptions { relax, entry: None }`; let the compiler find any others (`cargo build --workspace`). Update `crates/post-machine/tests/visibility_programs.rs:363` and `:371` to `LinkError::NoEntrySymbol("main".to_string())`.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p mtc-core && cargo test -p mtc-post-machine --test visibility_programs`
Expected: PASS (all)

- [ ] **Step 5: Quality gates + commit**

```bash
cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --check
git add -A && git commit -m "feat(core): LinkOptions.entry — configurable reachability root, error names it"
```

---

### Task 2: `project.rs` — manifest schema types + validation walk

**Files:**
- Create: `crates/post-machine/src/project.rs`
- Modify: `crates/post-machine/src/lib.rs` (add `mod project;` next to `mod config;`)
- Modify: `crates/post-machine/src/config.rs` (add `Invalid` variant to `ConfigError` + `detail()` arm)
- Test: unit tests inside `project.rs`

**Interfaces:**
- Consumes: `crate::config::ConfigError`, `crate::optimizer::OptLevel`, `crate::lint::validate_allow`.
- Produces (all `pub(crate)`):
  - `struct Manifest { stdlib: bool, sources: Vec<String>, libraries: Libraries, profiles: Profiles, targets: BTreeMap<String, Target> }`
  - `struct Libraries { dirs: Vec<String>, link: Vec<String> }` (Default)
  - `struct Profiles { debug: ProfileOverrides, release: ProfileOverrides }` (Default); `struct ProfileOverrides { opt: Option<OptLevel>, debug_info: Option<bool>, strip_debugger: Option<bool>, werror: Option<bool> }` (Default)
  - `struct Target { sources: Vec<String>, libraries: Libraries, entry: Option<String>, output: Option<String>, run: Option<RunSpec> }`
  - `struct RunSpec { tape: Option<String>, tape_block: Option<String>, head: Option<i64>, strict_cells: bool, max_steps: Option<u64>, max_tacts: Option<u64>, tact_profile: Option<[u32; 3]> }` (Default)
  - `struct ResolvedProfile { opt_level: OptLevel, debug_info: bool, strip_debugger: bool, werror: bool }`; `Profiles::resolve(&self, release: bool) -> ResolvedProfile`
  - `fn validate_manifest(path: &Path, value: &Value) -> Result<Manifest, ConfigError>`
  - `fn normalize_rel(path_str: &str) -> Result<PathBuf, String>`
  - `Manifest::effective_sources(&self, target: &Target) -> Vec<String>`; `Manifest::effective_libraries(&self, target: &Target) -> Libraries`; `Manifest::output_of(&self, name: &str, target: &Target) -> String`

- [ ] **Step 1: Add the `ConfigError::Invalid` variant** (semantic manifest rule violations — distinct from `Parse` shape complaints)

In `config.rs`, add to the enum, to `path()`, and to `detail()`:

```rust
    /// A semantically invalid `project` section: duplicate effective
    /// path, colliding target outputs, bad target name, `tape` and
    /// `tape-block` together, an unknown profile name, ... The message
    /// is complete on its own.
    Invalid { path: PathBuf, message: String },
```

`path()` arm joins the existing `|` chain; `detail()` arm: `ConfigError::Invalid { message, .. } => message.clone(),`

- [ ] **Step 2: Write the failing validation-matrix tests** (in `project.rs` `mod tests`; reuse `config.rs`'s `unique_tmp_dir` pattern locally — copy the helper, tests here validate over `serde_json::Value` directly so most need no filesystem)

```rust
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
            assert!(matches!(err, crate::config::ConfigError::UnknownKey { .. }), "{err:?}");
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
            assert!(matches!(err, crate::config::ConfigError::Invalid { .. }), "{bad}: {err:?}");
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
        let base = |run: serde_json::Value| {
            json!({ "targets": { "a": { "sources": ["m.pmc"], "run": run } } })
        };
        assert!(v(base(json!({ "tape": " *", "tape-block": "t.pmt" }))).is_err());
        assert!(v(base(json!({ "tape-block": "t.pmt", "head": 3 }))).is_err());
        assert!(v(base(json!({ "tape": " *", "head": 3, "strict-cells": true }))).is_ok());
        assert!(v(base(json!({}))).is_ok(), "empty run block = run defaults");
    }

    #[test]
    fn profiles_only_debug_and_release_and_resolve_applies_overrides() {
        assert!(v(json!({
            "targets": { "a": { "sources": ["m.pmc"] } },
            "profiles": { "bench": {} }
        }))
        .is_err());
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
        assert_eq!(m.effective_sources(t), vec!["shared.pmc".to_string(), "a.pmc".to_string()]);
        let libs = m.effective_libraries(t);
        assert_eq!(libs.dirs, vec!["libs".to_string(), "alibs".to_string()]);
        assert_eq!(libs.link, vec!["base".to_string(), "extra".to_string()]);
    }
}
```

- [ ] **Step 3: Run tests to verify they fail**

Run: `cargo test -p mtc-post-machine project:: 2>&1 | tail -5`
Expected: COMPILE ERROR (module doesn't exist)

- [ ] **Step 4: Implement `project.rs`**

```rust
//! The `project` section of `pmt.json`: the declared project model —
//! schema, validation, discovery (docs/project.md). Shared by
//! `pmt build` (cli/driver.rs) and the LSP. One loader validates the
//! WHOLE file (both sections) regardless of consumer, so the lint walk
//! and the project walk can never disagree about well-formedness.

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
/// (docs/cli.md: `--debug` = `-g -O0`, `--release` = `-O1
/// --strip-debugger`); `resolve` layers the manifest's per-key
/// overrides on the preset base. Flags override the result at the
/// driver (flags win — cli/driver.rs).
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
        self.sources.iter().chain(target.sources.iter()).cloned().collect()
    }

    pub(crate) fn effective_libraries(&self, target: &Target) -> Libraries {
        Libraries {
            dirs: self.libraries.dirs.iter().chain(target.libraries.dirs.iter()).cloned().collect(),
            link: self.libraries.link.iter().chain(target.libraries.link.iter()).cloned().collect(),
        }
    }

    pub(crate) fn output_of(&self, name: &str, target: &Target) -> String {
        target.output.clone().unwrap_or_else(|| format!("{name}.pmx"))
    }
}

/// Lexical normalization of a manifest-relative path: rejects absolute
/// paths (portability — a manifest is a committed artifact), folds `.`
/// and interior `..`, KEEPS leading `..` (sources above the manifest
/// directory are allowed — docs/project.md (path rules)). Lexical only:
/// symlink aliases are not detected, documented not solved.
pub(crate) fn normalize_rel(path_str: &str) -> Result<PathBuf, String> {
    let p = Path::new(path_str);
    let absolute_err = || {
        format!("absolute path `{path_str}` — manifest paths are relative to the manifest's directory")
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
    ConfigError::Invalid { path: path.to_path_buf(), message }
}

fn parse_err(path: &Path, message: &str) -> ConfigError {
    ConfigError::Parse { path: path.to_path_buf(), message: message.to_string() }
}

fn unknown_key(path: &Path, key: &str) -> ConfigError {
    ConfigError::UnknownKey { path: path.to_path_buf(), key: key.to_string() }
}

fn as_obj<'v>(
    path: &Path,
    value: &'v Value,
    what: &str,
) -> Result<&'v serde_json::Map<String, Value>, ConfigError> {
    value.as_object().ok_or_else(|| parse_err(path, &format!("`{what}` must be a JSON object")))
}

fn as_str_array(path: &Path, value: &Value, what: &str) -> Result<Vec<String>, ConfigError> {
    let complain = || parse_err(path, &format!("`{what}` must be an array of strings"));
    let arr = value.as_array().ok_or_else(complain)?;
    arr.iter()
        .map(|item| item.as_str().map(str::to_string).ok_or_else(complain))
        .collect()
}

fn as_bool(path: &Path, value: &Value, what: &str) -> Result<bool, ConfigError> {
    value.as_bool().ok_or_else(|| parse_err(path, &format!("`{what}` must be a boolean")))
}

fn as_u64(path: &Path, value: &Value, what: &str) -> Result<u64, ConfigError> {
    value.as_u64().ok_or_else(|| parse_err(path, &format!("`{what}` must be a non-negative integer")))
}

fn as_str(path: &Path, value: &Value, what: &str) -> Result<String, ConfigError> {
    value.as_str().map(str::to_string).ok_or_else(|| parse_err(path, &format!("`{what}` must be a string")))
}

fn valid_target_name(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else { return false };
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
                        return Err(invalid(path, format!("unknown opt level `{other}` (O0 | O1)")));
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
                run.head = Some(val.as_i64().ok_or_else(|| {
                    parse_err(path, "`head` must be an integer")
                })?);
            }
            "strict-cells" => run.strict_cells = as_bool(path, val, "strict-cells")?,
            "max-steps" => run.max_steps = Some(as_u64(path, val, "max-steps")?),
            "max-tacts" => run.max_tacts = Some(as_u64(path, val, "max-tacts")?),
            "tact-profile" => {
                let arr = val.as_array().ok_or_else(|| {
                    parse_err(path, "`tact-profile` must be [move, read, write]")
                })?;
                let [m, r, w] = arr.as_slice() else {
                    return Err(parse_err(path, "`tact-profile` must be [move, read, write]"));
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
        return Err(invalid(path, "`tape` and `tape-block` are mutually exclusive".into()));
    }
    if run.head.is_some() && run.tape.is_none() {
        return Err(invalid(path, "`head` is only meaningful alongside `tape`".into()));
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
        return Err(invalid(path, format!("target `{name}`: `entry` must not be empty")));
    }
    Ok(target)
}

/// Validates a whole `project` section value into a [`Manifest`],
/// including the semantic rules (docs/project.md): target-name charset,
/// per-target effective-list duplicate rejection, cross-target output
/// collision, path normalization/absolute rejection.
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
                            format!(
                                "bad target name `{tname}` (want [A-Za-z0-9][A-Za-z0-9_-]*)"
                            ),
                        ));
                    }
                    manifest.targets.insert(tname.clone(), parse_target(path, tname, tval)?);
                }
            }
            other => return Err(unknown_key(path, other)),
        }
    }
    if manifest.targets.is_empty() {
        return Err(invalid(path, "`project` needs at least one entry in `targets`".into()));
    }

    // Semantic pass: normalize every declared path (rejecting absolute
    // ones), reject duplicate effective sources per target, reject
    // colliding outputs across targets.
    let norm = |raw: &str| normalize_rel(raw).map_err(|message| invalid(path, message));
    for raw in manifest.sources.iter().chain(manifest.libraries.dirs.iter()) {
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
                format!("two targets resolve to the same output `{}`", output.display()),
            ));
        }
    }
    Ok(manifest)
}
```

Add `mod project;` to `lib.rs` next to the existing `mod config;`.

- [ ] **Step 5: Run tests, gates, commit**

Run: `cargo test -p mtc-post-machine project::`
Expected: PASS (all Step-2 tests)

```bash
cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --check
git add -A && git commit -m "feat(post-machine): pmt.json project section — schema types + validation walk (schema 0.2)"
```

---

### Task 3: One loader for the whole file + per-section discovery

**Files:**
- Modify: `crates/post-machine/src/project.rs` (add `PmtFile`, `load_file`, `discover_manifest`)
- Modify: `crates/post-machine/src/config.rs` (`load` delegates; move the lint walk into `project.rs`)
- Test: unit tests in both files

**Interfaces:**
- Consumes: Task 2's `validate_manifest`; `config::discover` (unchanged).
- Produces (`pub(crate)`): `struct PmtFile { allow: Vec<String>, manifest: Option<Manifest> }`; `fn load_file(path: &Path) -> Result<PmtFile, ConfigError>`; `fn discover_manifest(start: &Path) -> Result<Option<(PathBuf, Manifest)>, ConfigError>` (nearest ancestor `pmt.json` WITH a `project` key; a lint-only file on the walk is transparent; a malformed file on the walk is an error). `config::load` keeps its exact signature `(path) -> Result<ProjectConfig, ConfigError>`.

- [ ] **Step 1: Write the failing tests** (in `project.rs` `mod tests`; copy `config.rs`'s `unique_tmp_dir` helper with label prefix `pmt-project-test`)

```rust
    #[test]
    fn discover_manifest_skips_lint_only_files_but_lint_walk_stops_at_them() {
        let root = unique_tmp_dir("per-section");
        let sub = root.join("sub");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(
            root.join("pmt.json"),
            r#"{ "project": { "targets": { "app": { "sources": ["m.pmc"] } } } }"#,
        )
        .unwrap();
        std::fs::write(sub.join("pmt.json"), r#"{ "lint": { "allow": ["unused-label"] } }"#).unwrap();

        // Project walk: the nested lint-only file is transparent.
        let (found, manifest) = discover_manifest(&sub).unwrap().expect("project above");
        assert_eq!(found, root.join("pmt.json"));
        assert!(manifest.targets.contains_key("app"));

        // Lint walk: unchanged — nearest file wins, even lint-only.
        assert_eq!(crate::config::discover(&sub), Some(sub.join("pmt.json")));
    }

    #[test]
    fn one_loader_a_broken_project_section_fails_the_lint_load_too() {
        let dir = unique_tmp_dir("one-loader");
        let path = dir.join("pmt.json");
        std::fs::write(&path, r#"{ "lint": { "allow": [] }, "project": { "targets": {} } }"#).unwrap();
        assert!(crate::config::load(&path).is_err(), "empty targets must fail even for lint");
        assert!(load_file(&path).is_err());
    }

    #[test]
    fn load_file_reads_both_sections() {
        let dir = unique_tmp_dir("both");
        let path = dir.join("pmt.json");
        std::fs::write(
            &path,
            r#"{ "lint": { "allow": ["unused-label"] },
                "project": { "targets": { "app": { "sources": ["m.pmc"] } } } }"#,
        )
        .unwrap();
        let file = load_file(&path).unwrap();
        assert_eq!(file.allow, vec!["unused-label".to_string()]);
        assert!(file.manifest.is_some());
    }

    #[test]
    fn discover_manifest_errors_on_a_malformed_candidate() {
        let dir = unique_tmp_dir("malformed-walk");
        std::fs::write(dir.join("pmt.json"), "{").unwrap();
        assert!(discover_manifest(&dir).is_err());
    }
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p mtc-post-machine project:: 2>&1 | tail -5`
Expected: COMPILE ERROR (`load_file`/`discover_manifest` undefined)

- [ ] **Step 3: Implement**

In `project.rs`:

```rust
/// A whole validated `pmt.json`: the lint allow-list plus the optional
/// project manifest. THE one loader — both consumers (lint config, the
/// project model) validate everything so a typo in either section
/// surfaces no matter who reads the file first.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct PmtFile {
    pub allow: Vec<String>,
    pub manifest: Option<Manifest>,
}

pub(crate) fn load_file(path: &Path) -> Result<PmtFile, ConfigError> {
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

    let mut file = PmtFile { allow: Vec::new(), manifest: None };
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
            return Err(ConfigError::UnknownAllowCode { path: path.to_path_buf(), code });
        }
        Err(other) => unreachable!("validate_allow only ever returns UnknownAllowCode: {other}"),
    }
    Ok(allow)
}

/// Nearest ancestor `pmt.json` that HAS a `project` section — the
/// per-section discovery rule (docs/project.md): a lint-only file on
/// the walk is transparent to THIS walk (while `config::discover`
/// still stops at it for lint). A malformed candidate is an error, not
/// a skip: we cannot know whether it had a project section.
pub(crate) fn discover_manifest(
    start: &Path,
) -> Result<Option<(PathBuf, Manifest)>, ConfigError> {
    let start = if start.as_os_str().is_empty() { Path::new(".") } else { start };
    let Ok(abs) = std::path::absolute(start) else {
        return Ok(None);
    };
    let mut dir = Some(abs.as_path());
    while let Some(d) = dir {
        let candidate = d.join("pmt.json");
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
```

Note: `parse_lint`'s array handling differs slightly from the original (`as_str_array` gives one uniform message); update `config.rs`'s two message-shape tests if their expected strings change — the CONTRACT (shape errors without the `invalid JSON:` prefix) stays.

In `config.rs`, replace `load`'s body with delegation (keep signature and doc comment, adjusting the comment to name the one-loader rule):

```rust
pub(crate) fn load(path: &Path) -> Result<ProjectConfig, ConfigError> {
    crate::project::load_file(path).map(|file| ProjectConfig { allow: file.allow })
}
```

Delete the now-unused direct-walk code from `config.rs` (the `serde_json` imports it no longer needs); keep `discover`/`discover_from` untouched.

- [ ] **Step 4: Run the full crate test suite** (config tests must keep passing through the delegation)

Run: `cargo test -p mtc-post-machine`
Expected: PASS. If `load_rejects_wrong_shape_without_the_json_syntax_prefix` or the lint-array message tests fail on message wording, align the expected strings with `as_str_array`'s message — the no-prefix contract itself must hold.

- [ ] **Step 5: Gates + commit**

```bash
cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --check
git add -A && git commit -m "feat(post-machine): one pmt.json loader — full-file validation, per-section manifest discovery"
```

---

### Task 4: `cli/driver.rs` — argv mode (the cc driver)

**Files:**
- Create: `crates/post-machine/src/cli/driver.rs`
- Modify: `crates/post-machine/src/cli/mod.rs` (add `mod driver;`, dispatch `Some("build")`, add the `build` line to `USAGE`)
- Modify: `crates/post-machine/src/cli/build.rs` (make `out_path`, `render_warnings`, `render_opt_report`, `read_object`, `find_library`, `sidecar_path`, `take_disabled_passes` `pub(super)`)
- Test: create `crates/post-machine/tests/build_driver.rs`

**Interfaces:**
- Consumes: `compile_source` (=`compiler::compile`), `crate::asm::assemble`, `crate::asm::link`, `stdlib::object()`, Task 1's `LinkOptions { relax, entry }`, build.rs helpers above.
- Produces: `pub(super) fn build(raw: &[String]) -> Result<CliOutput, String>`; internal `struct Flags`; `fn undeclared_name(&str) -> Option<&str>`; `fn defined_names(&[ObjectFile], &[ObjectFile]) -> HashSet<String>`; `fn refine_reports(&mut [(PathBuf, CompileReport)], &HashSet<String>)`. Task 5 extends this file with manifest mode; the dispatch in `build()` (files vs targets) is written HERE with manifest mode stubbed as an error string, replaced in Task 5.

- [ ] **Step 1: Write the failing E2E tests** (`tests/build_driver.rs`, in-process style copied from `cli_programs.rs`)

```rust
use std::fs;
use std::path::PathBuf;

use mtc_post_machine::cli::execute;

fn args(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| s.to_string()).collect()
}

fn scratch(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    fs::create_dir_all(&dir).unwrap();
    dir
}

const MAIN_CALLS_UTIL: &str = "main() { @util(); }";
const UTIL_EXPORTED: &str = "export util() { mark; }";

#[test]
fn argv_mode_compiles_and_links_multiple_pmc_inputs_in_memory() {
    let dir = scratch("argv_two_pmc");
    let main = dir.join("main.pmc");
    let util = dir.join("util.pmc");
    fs::write(&main, MAIN_CALLS_UTIL).unwrap();
    fs::write(&util, UTIL_EXPORTED).unwrap();

    let out = execute(&args(&["build", main.to_str().unwrap(), util.to_str().unwrap()])).unwrap();
    assert_eq!(out.code, 0);
    assert!(dir.join("main.pmx").is_file(), "default output = first input's stem + .pmx");
    assert!(dir.join("main.pmx.map").is_file(), "sidecar rides along");
    assert!(!dir.join("main.pmo").exists(), "no disk intermediates by default");
}

#[test]
fn argv_mode_keep_objects_writes_pmo_next_to_each_source() {
    let dir = scratch("argv_keep_objects");
    let main = dir.join("main.pmc");
    let util = dir.join("util.pmc");
    fs::write(&main, MAIN_CALLS_UTIL).unwrap();
    fs::write(&util, UTIL_EXPORTED).unwrap();

    execute(&args(&[
        "build", "--keep-objects", main.to_str().unwrap(), util.to_str().unwrap(),
    ]))
    .unwrap();
    assert!(dir.join("main.pmo").is_file());
    assert!(dir.join("util.pmo").is_file());
}

#[test]
fn argv_mode_accepts_mixed_pmc_and_pmo_inputs() {
    let dir = scratch("argv_mixed");
    let util = dir.join("util.pmc");
    fs::write(&util, UTIL_EXPORTED).unwrap();
    execute(&args(&["compile", util.to_str().unwrap()])).unwrap();
    let main = dir.join("main.pmc");
    fs::write(&main, MAIN_CALLS_UTIL).unwrap();

    let out = execute(&args(&[
        "build", main.to_str().unwrap(), dir.join("util.pmo").to_str().unwrap(),
    ]))
    .unwrap();
    assert_eq!(out.code, 0);
    assert!(dir.join("main.pmx").is_file());
}

#[test]
fn argv_mode_refines_undeclared_external_resolved_by_a_sibling() {
    let dir = scratch("argv_refine");
    let main = dir.join("main.pmc");
    let util = dir.join("util.pmc");
    fs::write(&main, MAIN_CALLS_UTIL).unwrap();
    fs::write(&util, UTIL_EXPORTED).unwrap();

    // `@util()` in main.pmc is a bare undeclared external per-file, but
    // the declared set (both files) resolves it — no warning survives,
    // so -Werror over the POST-filter set succeeds.
    let out = execute(&args(&[
        "build", "-Werror", main.to_str().unwrap(), util.to_str().unwrap(),
    ]))
    .unwrap();
    assert_eq!(out.code, 0);
    assert!(!out.stderr.contains("undeclared"), "{}", out.stderr);

    // A genuinely unresolvable bare external still warns and -Werror fails.
    let lone = dir.join("lone.pmc");
    fs::write(&lone, "main() { @missing(); }").unwrap();
    let err = execute(&args(&["build", "-Werror", lone.to_str().unwrap()])).unwrap_err();
    assert!(err.contains("treated as errors"), "{err}");
}

#[test]
fn mixing_files_and_target_names_is_an_error() {
    let dir = scratch("argv_mixing");
    let main = dir.join("main.pmc");
    fs::write(&main, "main() { mark; }").unwrap();
    let err = execute(&args(&["build", main.to_str().unwrap(), "sometarget"])).unwrap_err();
    assert!(err.contains("not both"), "{err}");
}

#[test]
fn argv_mode_rejects_s_and_emit_ir() {
    let dir = scratch("argv_no_inspect_flags");
    let main = dir.join("main.pmc");
    fs::write(&main, "main() { mark; }").unwrap();
    for flag in ["-S", "--emit-ir"] {
        let err = execute(&args(&["build", flag, main.to_str().unwrap()])).unwrap_err();
        assert!(err.contains("unknown flag"), "{flag}: {err}");
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p mtc-post-machine --test build_driver 2>&1 | tail -5`
Expected: FAIL — `execute(["build", …])` returns `unknown subcommand \`build\``

- [ ] **Step 3: Implement `cli/driver.rs`** (argv mode + dispatch; manifest mode stubbed)

```rust
//! `pmt build`: the cc-style driver (docs/cli.md (pmt build)). Two
//! modes by positional shape — file inputs (argv mode, manifest never
//! read) or target names/none (manifest mode, docs/project.md). Both
//! compose the same internals `compile`/`asm`/`link` expose; objects
//! stay in memory unless --keep-objects.

use std::collections::HashSet;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use mtc_core::formats::object::ObjectFile;
use mtc_core::formats::object::SymbolDef;
use mtc_core::linker::LinkOptions;

use crate::compiler::{CompileOptions, CompileReport, compile as compile_source};
use crate::optimizer::OptLevel;
use crate::stdlib;

use super::build::{find_library, out_path, read_object, render_opt_report, render_warnings, sidecar_path, take_disabled_passes};
use super::{Args, CliOutput};

const BUILD_USAGE: &str = "\
USAGE: pmt build [INPUT.pmc|.pma|.pmo ...] [-o OUT.pmx] [FLAGS]   (argv mode)
       pmt build [TARGET ...] [FLAGS]                             (manifest mode)

Argv mode compiles/assembles/loads every input in memory, links with
the stdlib, and writes OUT.pmx (+ .pmx.map). Manifest mode discovers
the nearest pmt.json with a `project` section from the current
directory and builds its targets (all of them when none is named).

COMPILE FLAGS (argv mode; manifest mode: override the profile):
  --debug | --release   presets (manifest mode: profile selection)
  -O0 | -O1             optimization level
  -g                    record debug info
  --strip-debugger      drop `brk` at codegen
  --fno-<pass>          disable one optimizer pass (repeatable)
  -Werror               treat (post-refinement) warnings as errors

LINK FLAGS (argv mode only; the manifest declares these):
  --nostdlib            do not link the built-in std
  -L DIR / -l NAME      library search dir / library (repeatable)
  -o OUT.pmx            output path

COMMON:
  --no-relax            keep every symbol site in far form
  --keep-objects        write each intermediate .pmo next to its source
  --run [TARGET]        manifest mode: build, then run the target's run block
  --list-targets        manifest mode: print `NAME[\\trun]` per target
  -v                    render the build report
";

struct Flags {
    debug_preset: bool,
    release_preset: bool,
    o0: bool,
    o1: bool,
    debug_info: bool,
    strip_debugger: bool,
    werror: bool,
    disabled_passes: Vec<String>,
    no_relax: bool,
    nostdlib: bool,
    keep_objects: bool,
    search_dirs: Vec<String>,
    lib_names: Vec<String>,
    out: Option<String>,
    run: bool,
    list_targets: bool,
    verbose: bool,
}

pub(super) fn build(raw: &[String]) -> Result<CliOutput, String> {
    let mut args = Args::new(raw);
    if args.flag("--help") {
        return Ok(CliOutput::ok(BUILD_USAGE.into(), String::new()));
    }
    let mut disabled_passes = Vec::new();
    take_disabled_passes(&mut args, &mut disabled_passes);
    let flags = Flags {
        debug_preset: args.flag("--debug"),
        release_preset: args.flag("--release"),
        o0: args.flag("-O0"),
        o1: args.flag("-O1"),
        debug_info: args.flag("-g"),
        strip_debugger: args.flag("--strip-debugger"),
        werror: args.flag("-Werror"),
        disabled_passes,
        no_relax: args.flag("--no-relax"),
        nostdlib: args.flag("--nostdlib"),
        keep_objects: args.flag("--keep-objects"),
        search_dirs: args.values("-L")?,
        lib_names: args.values("-l")?,
        out: args.value("-o")?,
        run: args.flag("--run"),
        list_targets: args.flag("--list-targets"),
        verbose: args.flag("-v"),
    };
    let positionals = args.positionals()?;

    let is_file = |s: &str| {
        s.ends_with(".pmc") || s.ends_with(".pma") || s.ends_with(".pmo")
    };
    let (files, targets): (Vec<String>, Vec<String>) =
        positionals.into_iter().partition(|p| is_file(p));
    if !files.is_empty() && !targets.is_empty() {
        return Err(format!(
            "pmt build takes file inputs or target names, not both\n\n{BUILD_USAGE}"
        ));
    }
    if files.is_empty() {
        manifest_mode(&targets, &flags)
    } else {
        argv_mode(&files, &flags)
    }
}

fn manifest_mode(_targets: &[String], _flags: &Flags) -> Result<CliOutput, String> {
    Err("manifest mode lands in the next task".to_string()) // Task 5 replaces this
}

/// Compile options for argv mode: exactly `pmt compile`'s preset/flag
/// logic (cli/build.rs::compile), minus -S/--emit-ir which stay
/// compile-only inspection artifacts.
fn argv_compile_options(flags: &Flags) -> CompileOptions {
    let mut options = CompileOptions {
        debug_info: flags.debug_preset || flags.debug_info,
        strip_debugger: flags.release_preset || flags.strip_debugger,
        opt_level: if flags.release_preset { OptLevel::O1 } else { OptLevel::O0 },
        disabled_passes: flags.disabled_passes.clone(),
        capture_ir: false,
    };
    if flags.o0 {
        options.opt_level = OptLevel::O0;
    }
    if flags.o1 {
        options.opt_level = OptLevel::O1;
    }
    options
}

fn argv_mode(files: &[String], flags: &Flags) -> Result<CliOutput, String> {
    if flags.run || flags.list_targets {
        return Err(format!(
            "--run and --list-targets are manifest-mode flags\n\n{BUILD_USAGE}"
        ));
    }
    let options = argv_compile_options(flags);

    let mut objects: Vec<ObjectFile> = Vec::new();
    let mut reports: Vec<(PathBuf, CompileReport)> = Vec::new();
    for file in files {
        let path = Path::new(file);
        match path.extension().and_then(|e| e.to_str()) {
            Some("pmc") => {
                let source = fs::read_to_string(path)
                    .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
                let out = compile_source(&source, options.clone()).map_err(|e| {
                    format!(
                        "{}:{}:{}: error: {} [{}]",
                        path.display(), e.span.start.line, e.span.start.col, e.kind, e.kind.code()
                    )
                })?;
                if flags.keep_objects {
                    let pmo = path.with_extension("pmo");
                    fs::write(&pmo, out.object.to_bytes())
                        .map_err(|e| format!("cannot write {}: {e}", pmo.display()))?;
                }
                reports.push((path.to_path_buf(), out.report));
                objects.push(out.object);
            }
            Some("pma") => {
                let source = fs::read_to_string(path)
                    .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
                let object = crate::asm::assemble(&source, options.debug_info).map_err(|e| {
                    format!(
                        "{}:{}:{}: error: {} [{}]",
                        path.display(), e.span.start.line, e.span.start.col, e.kind, e.kind.code()
                    )
                })?;
                if flags.keep_objects {
                    let pmo = path.with_extension("pmo");
                    fs::write(&pmo, object.to_bytes())
                        .map_err(|e| format!("cannot write {}: {e}", pmo.display()))?;
                }
                objects.push(object);
            }
            _ => objects.push(read_object(path)?),
        }
    }

    let mut libraries = Vec::new();
    for name in &flags.lib_names {
        libraries.push(find_library(name, &flags.search_dirs)?);
    }
    if !flags.nostdlib {
        libraries.push(stdlib::object().clone());
    }

    refine_reports(&mut reports, &defined_names(&objects, &libraries));

    let mut stderr = String::new();
    let mut warning_count = 0usize;
    for (path, report) in &reports {
        warning_count += report.diagnostics.len();
        render_warnings(&mut stderr, path, report);
        if flags.verbose {
            render_opt_report(&mut stderr, report);
        }
    }
    if flags.werror && warning_count > 0 {
        return Err(format!("{stderr}-Werror: {warning_count} warning(s) treated as errors"));
    }

    let linked = crate::asm::link(
        &objects,
        &libraries,
        LinkOptions { relax: !flags.no_relax, entry: None },
    )
    .map_err(|e| e.to_string())?;

    let target = out_path(Path::new(&files[0]), flags.out.clone(), "pmx");
    fs::write(&target, linked.executable.to_bytes())
        .map_err(|e| format!("cannot write {}: {e}", target.display()))?;
    let map_path = sidecar_path(&target);
    fs::write(&map_path, linked.map.to_json())
        .map_err(|e| format!("cannot write {}: {e}", map_path.display()))?;

    if flags.verbose {
        let r = &linked.report;
        let _ = writeln!(
            stderr,
            "link: dropped [{}]; {} site(s) relaxed short, {} far",
            r.dropped.join(", "), r.relaxed_calls, r.far_calls
        );
    }
    Ok(CliOutput::ok(String::new(), stderr))
}

/// Every symbol name the declared set defines FOR CROSS-OBJECT
/// resolution — `SymbolDef::Defined` only, exactly the set
/// `linker::resolve` builds its namespace from (`Local` is invisible
/// there too).
fn defined_names(objects: &[ObjectFile], libraries: &[ObjectFile]) -> HashSet<String> {
    objects
        .iter()
        .chain(libraries.iter())
        .flat_map(|o| &o.symbols)
        .filter(|s| matches!(s.def, SymbolDef::Defined { .. }))
        .map(|s| s.name.clone())
        .collect()
}

/// The name inside the first backtick pair of an `undeclared-external`
/// message — the compiler's fixed format
/// ("call to undeclared external `NAME` — ..."), pinned by
/// `refinement_name_extraction_matches_the_compiler_format` below.
fn undeclared_name(message: &str) -> Option<&str> {
    let start = message.find('`')? + 1;
    let rest = &message[start..];
    Some(&rest[..rest.find('`')?])
}

/// The undeclared-external refinement (docs/cli.md (pmt build)): a bare
/// call that is undeclared per-file but resolved by the declared set is
/// not a defect of the BUILD — drop its warning. Runs before -Werror
/// counting so -Werror judges the post-filter set.
fn refine_reports(reports: &mut [(PathBuf, CompileReport)], defined: &HashSet<String>) {
    for (_, report) in reports.iter_mut() {
        report.diagnostics.retain(|d| {
            !(d.code == "undeclared-external"
                && undeclared_name(&d.message).is_some_and(|n| defined.contains(n)))
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Pins the extraction against the compiler's REAL warning format —
    /// if the message ever changes shape, this fails here rather than
    /// silently breaking the refinement.
    #[test]
    fn refinement_name_extraction_matches_the_compiler_format() {
        let out = compile_source("main() { @go(); }", CompileOptions::default()).unwrap();
        let diag = out
            .report
            .diagnostics
            .iter()
            .find(|d| d.code == "undeclared-external")
            .expect("bare @go() warns");
        assert_eq!(undeclared_name(&diag.message), Some("go"));
    }
}
```

In `cli/mod.rs`: add `mod driver;`, dispatch line `Some("build") => driver::build(&args[1..]),` (place after `Some("link")`), and add to `USAGE` after the `link` line:

```
  build        compile+link driver: .pmc/.pma/.pmo inputs or manifest targets
```

In `cli/build.rs`: change the visibility of `out_path`, `render_warnings`, `render_opt_report`, `read_object`, `find_library`, `sidecar_path`, `take_disabled_passes` from private to `pub(super)`.

The driver compiles several files from one options value: if `CompileOptions` does not already derive `Clone`, add `Clone` to its derive list in `compiler.rs` (additive, no behavior change).

- [ ] **Step 4: Run tests**

Run: `cargo test -p mtc-post-machine --test build_driver && cargo test -p mtc-post-machine driver::`
Expected: PASS (all Step-1 tests + the extraction pin)

- [ ] **Step 5: Gates + commit**

```bash
cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --check
git add -A && git commit -m "feat(post-machine): pmt build argv mode — in-memory cc driver with undeclared-external refinement"
```

---

### Task 5: `cli/driver.rs` — manifest mode + `--list-targets`

**Files:**
- Modify: `crates/post-machine/src/cli/driver.rs` (replace the `manifest_mode` stub)
- Test: extend `crates/post-machine/tests/build_driver.rs` (spawned-binary tests — manifest discovery starts at the process cwd)

**Interfaces:**
- Consumes: Task 3's `project::discover_manifest`, Task 2's `Manifest`/`Target`/`Profiles::resolve`/`normalize_rel`, Task 1's `LinkOptions.entry`.
- Produces: working `fn manifest_mode(targets: &[String], flags: &Flags) -> Result<CliOutput, String>`; `fn build_one_target(root: &Path, manifest: &Manifest, name: &str, target: &Target, flags: &Flags) -> Result<(PathBuf, String), String>` returning `(output_path, stderr_chunk)` — Task 6's `--run` consumes the output path.

- [ ] **Step 1: Write the failing E2E tests** (spawned binary; append to `build_driver.rs`)

```rust
use std::process::Command;

fn pmt() -> Command {
    Command::new(env!("CARGO_BIN_EXE_pmt"))
}

fn write_project(dir: &PathBuf) {
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::write(dir.join("src/shared.pmc"), "export util() { mark; }").unwrap();
    fs::write(dir.join("src/app.pmc"), "main() { @util(); }").unwrap();
    fs::write(dir.join("src/bench.pmc"), "export start() { @util(); halt; }").unwrap();
    fs::write(
        dir.join("pmt.json"),
        r#"{ "project": {
            "sources": ["src/shared.pmc"],
            "targets": {
                "app":   { "sources": ["src/app.pmc"] },
                "bench": { "sources": ["src/bench.pmc"], "entry": "start",
                           "run": { "tape": " *" } }
            }
        } }"#,
    )
    .unwrap();
}

#[test]
fn manifest_mode_bare_build_builds_all_targets_alphabetically() {
    let dir = scratch("manifest_all");
    write_project(&dir);
    let out = pmt().arg("build").current_dir(&dir).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(dir.join("app.pmx").is_file(), "default output <name>.pmx next to manifest");
    assert!(dir.join("app.pmx.map").is_file());
    assert!(dir.join("bench.pmx").is_file());
}

#[test]
fn manifest_mode_named_target_builds_only_it() {
    let dir = scratch("manifest_named");
    write_project(&dir);
    let out = pmt().args(["build", "app"]).current_dir(&dir).output().unwrap();
    assert!(out.status.success());
    assert!(dir.join("app.pmx").is_file());
    assert!(!dir.join("bench.pmx").exists());
}

#[test]
fn manifest_mode_discovery_walks_up_from_a_subdirectory() {
    let dir = scratch("manifest_walkup");
    write_project(&dir);
    let out = pmt().args(["build", "app"]).current_dir(dir.join("src")).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(dir.join("app.pmx").is_file(), "outputs resolve against the MANIFEST dir, not cwd");
}

#[test]
fn manifest_mode_rejects_declared_model_flags() {
    let dir = scratch("manifest_reject_flags");
    write_project(&dir);
    for flagset in [vec!["-o", "x.pmx"], vec!["-L", "libs"], vec!["-l", "x"], vec!["--nostdlib"]] {
        let mut cmd = pmt();
        cmd.arg("build").args(&flagset).arg("app").current_dir(&dir);
        let out = cmd.output().unwrap();
        assert!(!out.status.success(), "{flagset:?} must be rejected in manifest mode");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(stderr.contains("manifest"), "{flagset:?}: {stderr}");
    }
}

#[test]
fn manifest_mode_unknown_target_and_missing_manifest_error() {
    let dir = scratch("manifest_unknown");
    write_project(&dir);
    let out = pmt().args(["build", "nosuch"]).current_dir(&dir).output().unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("nosuch"));

    let empty = scratch("manifest_absent");
    let out = pmt().arg("build").current_dir(&empty).output().unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("project"));
}

#[test]
fn list_targets_prints_name_and_run_marker() {
    let dir = scratch("manifest_list");
    write_project(&dir);
    let out = pmt().args(["build", "--list-targets"]).current_dir(&dir).output().unwrap();
    assert!(out.status.success());
    assert_eq!(String::from_utf8_lossy(&out.stdout), "app\nbench\trun\n");
}

#[test]
fn release_flag_selects_the_release_profile() {
    let dir = scratch("manifest_release");
    write_project(&dir);
    let out = pmt().args(["build", "--release", "app"]).current_dir(&dir).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(dir.join("app.pmx").is_file());
}
```

Note: `scratch` dirs persist across runs under `CARGO_TARGET_TMPDIR` — each test writes its full fixture, so reruns are self-overwriting; tests asserting absence (`!bench.pmx exists`) must use their own scratch name (they do).

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p mtc-post-machine --test build_driver manifest 2>&1 | tail -5`
Expected: FAIL — stub error "manifest mode lands in the next task"

- [ ] **Step 3: Implement manifest mode** (replace the stub; `--run` gate lands here, execution in Task 6)

```rust
fn manifest_mode(requested: &[String], flags: &Flags) -> Result<CliOutput, String> {
    if flags.out.is_some() || !flags.search_dirs.is_empty() || !flags.lib_names.is_empty() || flags.nostdlib {
        return Err(format!(
            "-o/-L/-l/--nostdlib contradict the manifest — it declares outputs and libraries\n\n{BUILD_USAGE}"
        ));
    }
    let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
    let Some((manifest_path, manifest)) =
        crate::project::discover_manifest(&cwd).map_err(|e| e.to_string())?
    else {
        return Err(
            "no pmt.json with a `project` section found from the current directory upward".into(),
        );
    };
    let root = manifest_path.parent().expect("pmt.json has a parent").to_path_buf();

    if flags.list_targets {
        let mut stdout = String::new();
        for (name, target) in &manifest.targets {
            stdout.push_str(name);
            if target.run.is_some() {
                stdout.push_str("\trun");
            }
            stdout.push('\n');
        }
        return Ok(CliOutput::ok(stdout, String::new()));
    }

    for name in requested {
        if !manifest.targets.contains_key(name) {
            return Err(format!(
                "no target `{name}` in {} (targets: {})",
                manifest_path.display(),
                manifest.targets.keys().cloned().collect::<Vec<_>>().join(", ")
            ));
        }
    }
    let selected: Vec<&str> = if requested.is_empty() {
        manifest.targets.keys().map(String::as_str).collect() // BTreeMap: alphabetical
    } else {
        requested.iter().map(String::as_str).collect()
    };

    if flags.run && selected.len() != 1 {
        return Err(format!(
            "--run needs exactly one target (have {}): name it\n\n{BUILD_USAGE}",
            selected.len()
        ));
    }

    let mut stderr = String::new();
    let mut built: Vec<(String, PathBuf)> = Vec::new();
    for name in &selected {
        let target = &manifest.targets[*name];
        let (output, chunk) = build_one_target(&root, &manifest, name, target, flags)?;
        stderr.push_str(&chunk);
        built.push((name.to_string(), output));
    }

    if flags.run {
        let (name, output) = &built[0];
        let target = &manifest.targets[name.as_str()];
        return run_target(&root, output, target.run.as_ref(), stderr); // Task 6
    }
    Ok(CliOutput::ok(String::new(), stderr))
}

/// Builds one target: compile/assemble/load its effective sources with
/// the resolved profile (+ flag overrides), refine warnings against the
/// declared set, link with the declared libraries + entry, write the
/// output (+ sidecar) relative to the manifest dir. Returns the
/// absolute output path and the stderr chunk.
fn build_one_target(
    root: &Path,
    manifest: &crate::project::Manifest,
    name: &str,
    target: &crate::project::Target,
    flags: &Flags,
) -> Result<(PathBuf, String), String> {
    // In manifest mode --debug/--release are PURE profile selectors
    // (docs/cli.md (pmt build)): only the individual flags (-g, -O*,
    // --strip-debugger, -Werror) override the resolved profile's keys.
    let profile = manifest.profiles.resolve(flags.release_preset);
    let mut options = CompileOptions {
        debug_info: if flags.debug_info { true } else { profile.debug_info },
        strip_debugger: if flags.strip_debugger { true } else { profile.strip_debugger },
        opt_level: profile.opt_level,
        disabled_passes: flags.disabled_passes.clone(),
        capture_ir: false,
    };
    if flags.o0 {
        options.opt_level = OptLevel::O0;
    }
    if flags.o1 {
        options.opt_level = OptLevel::O1;
    }
    let werror = profile.werror || flags.werror;

    let resolve = |raw: &str| -> Result<PathBuf, String> {
        Ok(root.join(crate::project::normalize_rel(raw)?))
    };

    let mut objects: Vec<ObjectFile> = Vec::new();
    let mut reports: Vec<(PathBuf, CompileReport)> = Vec::new();
    for raw in manifest.effective_sources(target) {
        let path = resolve(&raw)?;
        match path.extension().and_then(|e| e.to_str()) {
            Some("pmc") => {
                let source = fs::read_to_string(&path)
                    .map_err(|e| format!("target `{name}`: cannot read {}: {e}", path.display()))?;
                let out = compile_source(&source, options.clone()).map_err(|e| {
                    format!(
                        "{}:{}:{}: error: {} [{}]",
                        path.display(), e.span.start.line, e.span.start.col, e.kind, e.kind.code()
                    )
                })?;
                if flags.keep_objects {
                    let pmo = path.with_extension("pmo");
                    fs::write(&pmo, out.object.to_bytes())
                        .map_err(|e| format!("cannot write {}: {e}", pmo.display()))?;
                }
                reports.push((path.clone(), out.report));
                objects.push(out.object);
            }
            Some("pma") => {
                let source = fs::read_to_string(&path)
                    .map_err(|e| format!("target `{name}`: cannot read {}: {e}", path.display()))?;
                let object = crate::asm::assemble(&source, options.debug_info).map_err(|e| {
                    format!(
                        "{}:{}:{}: error: {} [{}]",
                        path.display(), e.span.start.line, e.span.start.col, e.kind, e.kind.code()
                    )
                })?;
                objects.push(object);
            }
            _ => objects.push(read_object(&path)?),
        }
    }

    let libs = manifest.effective_libraries(target);
    let dirs: Vec<String> = libs
        .dirs
        .iter()
        .map(|d| resolve(d).map(|p| p.to_string_lossy().into_owned()))
        .collect::<Result<_, _>>()?;
    let mut libraries = Vec::new();
    for lib in &libs.link {
        libraries.push(find_library(lib, &dirs)?);
    }
    if manifest.stdlib {
        libraries.push(stdlib::object().clone());
    }

    refine_reports(&mut reports, &defined_names(&objects, &libraries));
    let mut stderr = String::new();
    let mut warning_count = 0usize;
    for (path, report) in &reports {
        warning_count += report.diagnostics.len();
        render_warnings(&mut stderr, path, report);
        if flags.verbose {
            render_opt_report(&mut stderr, report);
        }
    }
    if werror && warning_count > 0 {
        return Err(format!("{stderr}-Werror: {warning_count} warning(s) treated as errors"));
    }

    let linked = crate::asm::link(
        &objects,
        &libraries,
        LinkOptions { relax: !flags.no_relax, entry: target.entry.clone() },
    )
    .map_err(|e| format!("target `{name}`: {e}"))?;

    let output = resolve(&manifest.output_of(name, target))?;
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
    }
    fs::write(&output, linked.executable.to_bytes())
        .map_err(|e| format!("cannot write {}: {e}", output.display()))?;
    let map_path = sidecar_path(&output);
    fs::write(&map_path, linked.map.to_json())
        .map_err(|e| format!("cannot write {}: {e}", map_path.display()))?;

    if flags.verbose {
        let r = &linked.report;
        let _ = writeln!(
            stderr,
            "{name}: link: dropped [{}]; {} site(s) relaxed short, {} far",
            r.dropped.join(", "), r.relaxed_calls, r.far_calls
        );
    }
    Ok((output, stderr))
}

fn run_target(
    _root: &Path,
    _output: &Path,
    _run: Option<&crate::project::RunSpec>,
    _stderr: String,
) -> Result<CliOutput, String> {
    Err("--run lands in the next task".to_string()) // Task 6 replaces this
}
```

Note: `argv_mode` also needs the manifest-flags guard mirrored (`--run`/`--list-targets` rejected there — already done in Task 4). `project::Manifest`, `Target`, `RunSpec`, `normalize_rel` must be visible from `cli/` — they are `pub(crate)`.

- [ ] **Step 4: Run tests**

Run: `cargo test -p mtc-post-machine --test build_driver`
Expected: PASS except any `--run` test (none yet)

- [ ] **Step 5: Gates + commit**

```bash
cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --check
git add -A && git commit -m "feat(post-machine): pmt build manifest mode — targets, profiles, list-targets, declared-model flag rejection"
```

---

### Task 6: `--run` + `run.rs` settings/execution split

**Files:**
- Modify: `crates/post-machine/src/cli/run.rs` (extract `RunSettings` + `execute_run`; `run()` becomes parse→delegate)
- Modify: `crates/post-machine/src/cli/driver.rs` (replace `run_target` stub)
- Test: extend `crates/post-machine/tests/build_driver.rs`

**Interfaces:**
- Consumes: Task 5's `build_one_target` output path; `RunSpec` from the manifest.
- Produces in `run.rs` (`pub(super)`): `struct RunSettings { tape_block: Option<String>, tape_inline: Option<String>, head: i64, save: Option<String>, strict: bool, no_step_limit: bool, max_steps: Option<u64>, max_tacts: Option<u64>, profile: TactProfile, trace: bool }` with `Default` (head 0, profile `TactProfile::ELECTRONIC`, all else off/None); `fn execute_run(exe_path: &Path, settings: &RunSettings, trace_out: &mut dyn std::io::Write) -> Result<CliOutput, String>`.

- [ ] **Step 1: Write the failing tests** (append to `build_driver.rs`)

```rust
#[test]
fn build_run_adopts_the_machine_exit_code() {
    let dir = scratch("manifest_run");
    write_project(&dir);
    // bench: `entry: start`, `run: { tape: " *" }`, ends in `halt` -> exit 2.
    let out = pmt().args(["build", "--run", "bench"]).current_dir(&dir).output().unwrap();
    assert_eq!(out.status.code(), Some(2), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(String::from_utf8_lossy(&out.stdout).contains("outcome"));

    // app: no run block -> pmt run defaults; program stops -> exit 0.
    let out = pmt().args(["build", "--run", "app"]).current_dir(&dir).output().unwrap();
    assert_eq!(out.status.code(), Some(0), "{}", String::from_utf8_lossy(&out.stderr));
}

#[test]
fn build_run_without_a_named_target_needs_exactly_one() {
    let dir = scratch("manifest_run_ambiguous");
    write_project(&dir);
    let out = pmt().args(["build", "--run"]).current_dir(&dir).output().unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("exactly one"));
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p mtc-post-machine --test build_driver build_run 2>&1 | tail -5`
Expected: FAIL — stub error "--run lands in the next task"

- [ ] **Step 3: Refactor `run.rs`** — move everything in `run()` AFTER the `args.positionals()` block into:

```rust
pub(super) struct RunSettings {
    pub tape_block: Option<String>,
    pub tape_inline: Option<String>,
    pub head: i64,
    pub save: Option<String>,
    pub strict: bool,
    pub no_step_limit: bool,
    pub max_steps: Option<u64>,
    pub max_tacts: Option<u64>,
    pub profile: TactProfile,
    pub trace: bool,
}

impl Default for RunSettings {
    fn default() -> Self {
        Self {
            tape_block: None,
            tape_inline: None,
            head: 0,
            save: None,
            strict: false,
            no_step_limit: false,
            max_steps: None,
            max_tacts: None,
            profile: TactProfile::ELECTRONIC,
            trace: false,
        }
    }
}

pub(super) fn execute_run(
    exe_path: &Path,
    settings: &RunSettings,
    trace_out: &mut dyn std::io::Write,
) -> Result<CliOutput, String> {
    // body: run()'s current lines from `let bytes = fs::read(exe_path)…`
    // to the final Ok(CliOutput { … }), with every local flag variable
    // replaced by the corresponding settings.* field
    // (trace -> settings.trace, strict -> settings.strict, …).
}
```

`run()` keeps its exact current argument parsing, then builds a `RunSettings` from the parsed values and returns `execute_run(exe_path, &settings, trace_out)`. Behavior must be byte-identical — the existing `cli_programs.rs` run tests are the guard.

- [ ] **Step 4: Implement `run_target` in `driver.rs`**

```rust
fn run_target(
    root: &Path,
    output: &Path,
    run: Option<&crate::project::RunSpec>,
    build_stderr: String,
) -> Result<CliOutput, String> {
    use mtc_core::vm::TactProfile;
    let spec = run.cloned().unwrap_or_default();
    let settings = super::run::RunSettings {
        tape_block: spec
            .tape_block
            .map(|raw| -> Result<String, String> {
                Ok(root
                    .join(crate::project::normalize_rel(&raw)?)
                    .to_string_lossy()
                    .into_owned())
            })
            .transpose()?,
        tape_inline: spec.tape,
        head: spec.head.unwrap_or(0),
        save: None,
        strict: spec.strict_cells,
        no_step_limit: false,
        max_steps: spec.max_steps,
        max_tacts: spec.max_tacts,
        profile: spec.tact_profile.map_or(TactProfile::ELECTRONIC, |[m, r, w]| TactProfile {
            move_cost: m,
            read_cost: r,
            write_cost: w,
        }),
        trace: false,
    };
    let mut run_out = super::run::execute_run(output, &settings, &mut std::io::sink())?;
    run_out.stderr = format!("{build_stderr}{}", run_out.stderr);
    Ok(run_out)
}
```

(`run` module visibility: `mod run;` in `cli/mod.rs` is already crate-internal; `RunSettings`/`execute_run` are `pub(super)` = visible throughout `cli/`.)

- [ ] **Step 5: Run the full suite** (run.rs refactor must not disturb existing run tests)

Run: `cargo test -p mtc-post-machine`
Expected: PASS — including `cli_programs.rs`'s run/trace tests and the two new `build_run` tests

- [ ] **Step 6: Gates + commit**

```bash
cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --check
git add -A && git commit -m "feat(post-machine): pmt build --run — manifest run blocks through a settings-split pmt run"
```

---

### Task 7: Shell completion — `build` registry entry + dynamic target names

**Files:**
- Modify: `crates/post-machine/src/completions/registry.rs` (new `PositionalHint::FilesOrTargets`, `build_spec()`, register it, `top_level_help`, doc-comment count)
- Modify: `crates/post-machine/src/completions/zsh.rs` (render the new hint + emit the `__pmt_build_targets` helper)
- Test: existing `tests/completions_registry.rs` (drift guard runs as-is), `tests/completions_zsh.rs`, unit tests in both source files

**Interfaces:**
- Consumes: Task 5's `--list-targets` output format (`NAME[\trun]` per line).
- Produces: `PositionalHint::FilesOrTargets(FileHint)` — files by extension OR dynamic target names.

- [ ] **Step 1: Write the failing tests**

In `registry.rs` `mod tests`, update the root-choices test's expected vec to include `"build"` after `"link"`, and add:

```rust
    #[test]
    fn build_positional_offers_files_and_dynamic_targets() {
        let reg = registry();
        let build = reg
            .commands
            .iter()
            .find(|c| c.path == vec!["build".to_string()])
            .expect("build should be registered");
        let Positional::OneOrMore(PositionalHint::FilesOrTargets(hint)) = &build.positional else {
            panic!("build positional should be files-or-targets");
        };
        assert_eq!(hint.extensions, vec!["pmc", "pma", "pmo"]);
        assert!(build.flags.iter().any(|f| f.name == "--list-targets"));
        assert!(build.flags.iter().any(|f| f.name == "--run"));
        assert!(build.flags.iter().any(|f| f.name == "--keep-objects"));
    }
```

In `zsh.rs` `mod tests`:

```rust
    #[test]
    fn build_renders_dynamic_target_alternative_and_helper() {
        let script = render(&registry());
        assert!(script.contains("__pmt_build_targets"), "helper function emitted");
        assert!(
            script.contains("targets:target:__pmt_build_targets"),
            "positional _alternative wires the helper: {script}"
        );
        assert!(script.contains("pmt build --list-targets"), "helper shells out to pmt");
    }
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p mtc-post-machine completions 2>&1 | tail -5`
Expected: COMPILE ERROR (`FilesOrTargets` doesn't exist)

- [ ] **Step 3: Implement**

`registry.rs` — add the variant and the spec:

```rust
    /// Files by extension OR a target name from the nearest manifest —
    /// `pmt build`'s positional. Rendered dynamically: the zsh script's
    /// `__pmt_build_targets` helper shells out to
    /// `pmt build --list-targets` at completion time (the `_git`
    /// pattern), so target names track the manifest with zero drift.
    FilesOrTargets(FileHint),
```

```rust
fn build_spec() -> CommandSpec {
    CommandSpec {
        path: strings(&["build"]),
        positional: Positional::OneOrMore(PositionalHint::FilesOrTargets(ext(&[
            "pmc", "pma", "pmo",
        ]))),
        flags: vec![
            FlagSpec::boolean("--debug", "preset/profile: -g -O0").exclusive("profile"),
            FlagSpec::boolean("--release", "preset/profile: -O1 --strip-debugger")
                .exclusive("profile"),
            FlagSpec::boolean("-O0", "optimization level O0").exclusive("opt-level"),
            FlagSpec::boolean("-O1", "optimization level O1").exclusive("opt-level"),
            FlagSpec::boolean("-g", "record debug info"),
            FlagSpec::boolean("--strip-debugger", "drop `brk` at codegen"),
            FlagSpec::suffix_family(
                "--fno-",
                "disable one optimizer pass (repeatable)",
                crate::optimizer::pass_names().iter().map(|p| p.to_string()).collect(),
            ),
            FlagSpec::boolean("-Werror", "treat post-refinement warnings as errors"),
            FlagSpec::boolean("--no-relax", "keep every symbol site in far form"),
            FlagSpec::boolean("--nostdlib", "argv mode: do not link the built-in std"),
            FlagSpec::value("-L", "argv mode: library search directory", ValueHint::Directory)
                .repeatable(),
            FlagSpec::value("-l", "argv mode: link NAME.pmo from the search path", ValueHint::Text)
                .repeatable(),
            FlagSpec::value("-o", "argv mode: output path", ValueHint::File(any_file())),
            FlagSpec::boolean("--keep-objects", "write each intermediate .pmo next to its source"),
            FlagSpec::boolean("--run", "manifest mode: build then run the target"),
            FlagSpec::boolean("--list-targets", "manifest mode: print NAME[\\trun] per target"),
            FlagSpec::boolean("-v", "render the build report"),
            FlagSpec::boolean("--help", "show subcommand help"),
        ],
    }
}
```

Register `build_spec()` in `registry()` after `link_spec()`; add `"build" => "compile+link driver: .pmc/.pma/.pmo inputs or manifest targets",` to `top_level_help`; update the `registry()` doc comment (12 top-level subcommands, `build` no longer "deliberately absent").

`zsh.rs` — extend the two positional matches:

```rust
// in positional_message:
        PositionalHint::FilesOrTargets(_) => "file or target",
// in positional_action:
        PositionalHint::FilesOrTargets(file_hint) => {
            let escaped_glob = glob_action(&file_hint.extensions).replace('"', "\\\"");
            format!(
                "_alternative \"files:file:{escaped_glob}\" \"targets:target:__pmt_build_targets\""
            )
        }
```

and emit the helper into the script preamble (next to wherever `render` writes its function definitions, before the `_arguments` dispatch):

```zsh
__pmt_build_targets() {
  local -a __targets
  __targets=(${(f)"$(pmt build --list-targets 2>/dev/null)"})
  __targets=(${__targets%%$'\t'*})
  (( ${#__targets} )) && compadd -a __targets
}
```

(Emit unconditionally — one small function; it only runs when the `targets` alternative is attempted.)

- [ ] **Step 4: Run the completion test suites** (drift guard probes `build`'s flags against the real parser automatically; the zsh `-n`/`compinit` test validates the emitted helper)

Run: `cargo test -p mtc-post-machine --test completions_registry && cargo test -p mtc-post-machine --test completions_zsh && cargo test -p mtc-post-machine completions`
Expected: PASS. If the drift guard's probe reaches manifest-mode paths (e.g. probing `--run` errors with "no pmt.json"), that error message does NOT contain "unknown flag" — the guard checks for parser rejection, not success; adjust only if the guard's assertion style demands a specific error class.

- [ ] **Step 5: Gates + commit**

```bash
cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --check
git add -A && git commit -m "feat(post-machine): build shell completion — registry entry + dynamic target names via --list-targets"
```

---

### Task 8: Documentation

**Files:**
- Create: `docs/project.md`
- Modify: `docs/cli.md` (add the `pmt build` section after `pmt link`; update the subcommand list/count)
- Modify: `docs/lint.md` (project-file section points at `docs/project.md` for the `project` section)
- Modify: `README.md` (subcommand table/count; a short manifest example in the quickstart)
- Modify: `CLAUDE.md` (twelve subcommands; `project.rs` + `cli/driver.rs` in the architecture notes; `docs/project.md` in the docs list)

**Interfaces:** none — prose only. Published pages are ref-free and forge-agnostic.

- [ ] **Step 1: Write `docs/project.md`** covering, in this order (draft the prose from the spec's "The manifest" + CLI sections; every claim must match the implemented behavior — copy examples from the tests):
  1. What the project file is: `pmt.json`, the one project config file; schema version **0.2** (0.1 was the lint-only shape); the `lint` section cross-referencing `docs/lint.md`.
  2. Per-section discovery: lint = nearest `pmt.json`; project = nearest `pmt.json` WITH a `project` key; both stop at first hit, never merge; a lint-only file is transparent to the project walk.
  3. The full annotated example (use the spec's `app`/`bench` example verbatim).
  4. Key-by-key reference: `stdlib`, `sources`, `libraries.dirs`/`libraries.link` (first-wins order, shadowed by user definitions, lazy reachability), `profiles` (debug/release bases mirroring the CLI presets, per-key overrides), `targets.<name>` (`sources`, `libraries`, `entry` default `main`, `output` default `<name>.pmx`, `run` block keys and the `tape` XOR `tape-block` / `head`-requires-`tape` rules).
  5. Path rules: relative to the manifest dir, `../` allowed, absolute rejected, lexical normalization only (symlink aliases undetected), duplicate/collision errors.
  6. How `pmt build` consumes it (short; deep link to `docs/cli.md`), including the undeclared-external refinement rule and that `compile` stays per-file honest.
- [ ] **Step 2: Add the `pmt build` section to `docs/cli.md`** — both usage forms, the flag table split (compile side / link side argv-only / common), mode dispatch rule (file extension vs target name, no mixing), manifest-mode flag rejection list, `--run` exit codes (0 `stp` / 2 `hlt` / 3 trap after a successful build), `--list-targets` format (`NAME`, tab, `run`), `--keep-objects` placement (next to each source, both modes). Update the doc's subcommand enumeration to twelve.
- [ ] **Step 3: Point `docs/lint.md` at `docs/project.md`** — one sentence in the project-file section: the same `pmt.json` may also carry a `project` section (see `docs/project.md`); its presence does not change lint discovery.
- [ ] **Step 4: README + CLAUDE.md** — README: add `build` to the subcommand table with the one-liner from `USAGE`; add a 10-line manifest example under quickstart. CLAUDE.md: "eleven subcommands" → "twelve subcommands (+build)"; add `project.rs` (manifest: schema/validation/discovery, one-loader rule) and `cli/driver.rs` to the architecture section; add `project.md` to the documentation-authority list; note `pmt.json` schema 0.2 in the version-spaces section.
- [ ] **Step 5: Verify claims against behavior**

Run: `cargo test -p mtc-post-machine 2>&1 | tail -3` (green baseline), then manually cross-check each `docs/project.md` claim that has a test (discovery, defaults, XOR rules, exit codes) against the test expectations from Tasks 2–6.
Expected: no doc claim without a matching implemented behavior.

- [ ] **Step 6: Commit**

```bash
git add -A && git commit -m "docs: project manifest reference (schema 0.2), pmt build CLI page, twelve-subcommand counts"
```

---

## Final verification (after all tasks)

- [ ] `cargo test --workspace` — everything green
- [ ] `cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --check`
- [ ] Smoke: in a scratch dir, write the spec's example manifest + two sources; `pmt build`, `pmt build --list-targets`, `pmt build --run bench`; confirm outputs, marker format, exit code 2
- [ ] `pmt completions zsh | zsh -n /dev/stdin` — script parses
