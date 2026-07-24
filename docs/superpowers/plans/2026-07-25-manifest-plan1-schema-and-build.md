# Project Manifest + `pmt build` / `tmt build` (Plan 1 of 3) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship the `project` section (schema 0.2) in BOTH project files —
`pmt.json` and `tmt.json` — and both driver subcommands, `pmt build` and
`tmt build`, each with argv mode, manifest mode, `--run`, `--list-targets`,
shell completion, bare manifest-driven `lint`/`fmt`, and per-toolchain
reference docs, per the spec
`docs/superpowers/specs/2026-07-12-project-manifest-and-build-design.md`
(amended 2026-07-21 for shipped TM-1). Plan 2 (LSP overlay) and Plan 3
(editors) follow off the same spec.

**Architecture:** Manifest types + validation + discovery live in a new
`project.rs` **per crate** (`crates/post-machine/src/project.rs`,
`crates/turing-machine/src/project.rs`), each becoming the ONE loader that
validates a whole project file (both sections) regardless of consumer;
each `config.rs` keeps its public shape and delegates. Core stays
arch-agnostic and holds no manifest knowledge (spec §168). Each driver is
a new `cli/driver.rs` composing the existing compile/asm/link/run
internals in memory. **No core changes at all** — `LinkOptions.entry`
landed with the TM-1 arc.

**Tech Stack:** Rust, `serde_json` only (zero new deps), hand-rolled CLI
(`cli::Args`), in-process integration tests via `cli::execute` +
spawned-binary tests via `env!("CARGO_BIN_EXE_pmt")` /
`env!("CARGO_BIN_EXE_tmt")` for cwd-dependent discovery.

**Task order:** PM-1 first (Tasks 1–8), then TM-1 (Tasks 9–16). The TM
tasks port shipped PM code with a documented divergence list, so the PM
half must be merged (or at least in the tree) before Task 9 starts. This
mirrors how `config.rs`, the lint layer, and the completions registry
were twinned during the TM-1 arc.

## Global Constraints

- **Zero new dependencies** — `serde/serde_json` runtime, `proptest` dev-only. No tempfile, no clap, no glob crates.
- **Thin-renderer rule** — library code never prints; every terminal byte originates in `cli/` behind structured reports.
- **Strict unknown-key errors** in both project files at every level, in `config.rs`'s precise style ("unknown key `X`").
- **Manifest paths**: relative to the manifest's directory; `../` allowed; absolute rejected; lexical normalization only.
- **Schema version 0.2** for BOTH files (0.1 = the retroactive lint-only shape). Independent contracts that happen to move together (spec §586–599); documented on `docs/pmt/project.md` and `docs/tmt/project.md`, not inside the files themselves.
- **No cross-toolchain manifest** — `pmt build` reads only `pmt.json`, `tmt build` only `tmt.json`; the two never merge (spec §168).
- **Published docs** (README, `docs/`) are forge-agnostic and ref-free: no issue/PR numbers, no URLs. Code comments cite durable pages by page + parenthetical keyword; never a `docs/superpowers/` path.
- **PM-1 byte-identity** is a standing gate: `pm1_syntax()` never opts into `AsmCaps`, and no task here touches codegen or the assembler.
- **Quality gates**: `cargo clippy --workspace --all-targets -- -D warnings` and `cargo fmt --check` must pass at every commit.
- **Commit style**: conventional commits with scope (`feat(post-machine):`, `feat(turing-machine):`, `test(...)`, `docs:`).
- **Commit permission**: the user's standing rule forbids commits without explicit permission. At execution start, ask for blanket per-task commit permission for this plan; if not granted, skip every commit step and stop after each task for review.

## Verified starting state (master `ddb4d24`, audited 2026-07-25)

Facts every task below depends on; re-check only if master has moved.
The full audit is `docs/superpowers/plans/2026-07-25-manifest-plan1-revalidation.md`.

- `LinkOptions` is `{ relax: bool, entry: Option<String>, call_mech: CallMech }` (`crates/core/src/linker/mod.rs:183`). **No core change is needed.** Literals in this plan therefore always spread: `LinkOptions { relax, entry, ..Default::default() }`.
- `TactProfile` has FIVE fields (`move_cost`, `read_cost`, `write_cost`, `table_read_cost`, `frame_load_cost`). Any construction spreads `..TactProfile::ELECTRONIC`.
- `CompileOptions` already derives `Clone` in both crates.
- `Diagnostic` is `mtc_core::diagnostics::Diagnostic { code: &'static str, span: Span, message: String, fix: Option<Fix> }`.
- Both crates emit the `undeclared-external` warning (PM `compiler.rs:747`, TM `compiler.rs:2490`), so the refinement rule transfers unchanged.
- `docs/` is split: `docs/pmt/*` and `docs/tmt/*` plus shared root pages. `pmt.json` is documented today in `docs/pmt/lint.md` (§ Project file), `tmt.json` in `docs/tmt/cli.md` (§ tmt.json).
- **`tests/cli_docs.rs` exists in BOTH crates** and asserts every subcommand in the completion registry has a verbatim `--help` block on its cli page. Registering `build` without the page block turns it red — which is why the doc block lands in the same task as the registry entry (Tasks 6 and 14), not in the docs task.
- `Args` methods (`flag`/`value`/`values`/`positionals`) and `CliOutput::ok` are `pub(crate)` in each crate's `cli/mod.rs`.

---

### Task 1: `project.rs` — PM manifest schema types + validation walk

**Files:**
- Create: `crates/post-machine/src/project.rs`
- Modify: `crates/post-machine/src/lib.rs` (add `mod project;` next to `mod config;`)
- Modify: `crates/post-machine/src/config.rs` (add `Invalid` variant to `ConfigError` + `path()`/`detail()` arms)
- Test: unit tests inside `project.rs`

**Interfaces:**
- Consumes: `crate::config::ConfigError`, `crate::optimizer::OptLevel`. (`crate::lint::validate_allow` is NOT consumed here — the lint walk moves into `project.rs` in Task 2, which is where `parse_lint` calls it.)
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

- [ ] **Step 0: Confirm `ConfigError` is outside the error-code registries**

Run: `grep -n "fn code" crates/post-machine/src/config.rs`
Expected: NO match — `ConfigError` exposes `path()`/`detail()` only, and the
`CODES` registries with docs-inventory drift guards
(`tests/error_code_docs.rs`) cover `CompileErrorKind` and `AsmErrorKind`,
not config errors. If a `code()` HAS appeared since the audit, stop and ask:
adding `Invalid` would then owe a registry row, a docs-table row on
`docs/pmt/cli.md`, and a guard update, which is a separate decision.

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

- [ ] **Step 2: Write the failing validation-matrix tests** (in `project.rs` `mod tests`; tests here validate over `serde_json::Value` directly so most need no filesystem)

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
//! schema, validation, discovery (docs/pmt/project.md (schema)). Shared
//! by `pmt build` (cli/driver.rs) and the LSP. One loader validates the
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
/// directory are allowed — docs/pmt/project.md (path rules)). Lexical
/// only: symlink aliases are not detected, documented not solved.
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

### Task 2: One PM loader for the whole file + per-section discovery

**Files:**
- Modify: `crates/post-machine/src/project.rs` (add `PmtFile`, `load_file`, `discover_manifest`)
- Modify: `crates/post-machine/src/config.rs` (`load` delegates; move the lint walk into `project.rs`)
- Test: unit tests in both files

**Interfaces:**
- Consumes: Task 1's `validate_manifest`; `config::discover` (unchanged).
- Produces (`pub(crate)`): `struct PmtFile { allow: Vec<String>, manifest: Option<Manifest> }`; `fn load_file(path: &Path) -> Result<PmtFile, ConfigError>`; `fn discover_manifest(start: &Path) -> Result<Option<(PathBuf, Manifest)>, ConfigError>` (nearest ancestor `pmt.json` WITH a `project` key; a lint-only file on the walk is transparent; a malformed file on the walk is an error). `config::load` keeps its exact signature `(path) -> Result<ProjectConfig, ConfigError>`.

- [ ] **Step 1: Write the failing tests** (in `project.rs` `mod tests`; copy `config.rs`'s `unique_tmp_dir` helper with label prefix `pmt-project-test` — it exists there because each crate defines its own local test helpers, no shared support module)

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
/// per-section discovery rule (docs/pmt/project.md (discovery)): a
/// lint-only file on the walk is transparent to THIS walk (while
/// `config::discover` still stops at it for lint). A malformed
/// candidate is an error, not a skip: we cannot know whether it had a
/// project section.
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

Delete the now-unused direct-walk code from `config.rs` (and the `serde_json` imports it no longer needs); keep `discover`/`discover_from` untouched.

- [ ] **Step 4: Run the full crate test suite** (config tests must keep passing through the delegation)

Run: `cargo test -p mtc-post-machine`
Expected: PASS. If a message-shape test fails on wording, align the expected string with `as_str_array`'s message — the no-prefix contract itself must hold.

- [ ] **Step 5: Gates + commit**

```bash
cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --check
git add -A && git commit -m "feat(post-machine): one pmt.json loader — full-file validation, per-section manifest discovery"
```

---

### Task 3: `cli/driver.rs` — PM argv mode (the cc driver)

**Files:**
- Create: `crates/post-machine/src/cli/driver.rs`
- Modify: `crates/post-machine/src/cli/mod.rs` (add `mod driver;`, dispatch `Some("build")`, add the `build` line to `USAGE`)
- Modify: `crates/post-machine/src/cli/build.rs` (make `out_path`, `render_warnings`, `render_opt_report`, `read_object`, `find_library`, `sidecar_path`, `take_disabled_passes` `pub(super)`)
- Modify: `docs/pmt/cli.md` — one line only, adding `build` to the root `SUBCOMMANDS:` list. **Forced, not optional:** `tests/cli_docs.rs` quotes the ROOT usage block verbatim too, not just the per-subcommand ones, so adding the `build` line to `USAGE` in `cli/mod.rs` turns that guard red in this task. The full `## pmt build` section still belongs to Task 6, which registers `build_spec()` and trips the guard's other assertion.
- Test: create `crates/post-machine/tests/build_driver.rs`

**Interfaces:**
- Consumes: `compiler::compile` (as `compile_source`), `crate::asm::assemble(&str, bool)`, `crate::asm::link(&[ObjectFile], &[ObjectFile], LinkOptions)`, `stdlib::object()`, the `cli/build.rs` helpers above.
- Produces: `pub(super) fn build(raw: &[String]) -> Result<CliOutput, String>`; internal `struct Flags`; `fn undeclared_name(&str) -> Option<&str>`; `fn defined_names(&[ObjectFile], &[ObjectFile]) -> HashSet<String>`; `fn refine_reports(&mut [(PathBuf, CompileReport)], &HashSet<String>)`. Task 4 extends this file with manifest mode; the dispatch in `build()` (files vs targets) is written HERE with manifest mode stubbed as an error string, replaced in Task 4.

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
//! `pmt build`: the cc-style driver (docs/pmt/cli.md (build)). Two modes
//! by positional shape — file inputs (argv mode, manifest never read) or
//! target names/none (manifest mode, docs/pmt/project.md). Both compose
//! the same internals `compile`/`asm`/`link` expose; objects stay in
//! memory unless --keep-objects.

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
    Err("manifest mode lands in the next task".to_string()) // Task 4 replaces this
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

    // LinkOptions has three fields (relax / entry / call_mech); argv mode
    // takes the defaults for the latter two, exactly as `pmt link` does.
    let linked = crate::asm::link(
        &objects,
        &libraries,
        LinkOptions { relax: !flags.no_relax, ..Default::default() },
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

/// The undeclared-external refinement (docs/pmt/cli.md (build)): a bare
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

(`CompileOptions` already derives `Clone` — no derive change needed.)

- [ ] **Step 4: Run tests**

Run: `cargo test -p mtc-post-machine --test build_driver && cargo test -p mtc-post-machine driver::`
Expected: PASS (all Step-1 tests + the extraction pin)

- [ ] **Step 5: Gates + commit**

```bash
cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --check
git add -A && git commit -m "feat(post-machine): pmt build argv mode — in-memory cc driver with undeclared-external refinement"
```

---

### Task 4: `cli/driver.rs` — PM manifest mode + `--list-targets`

**Files:**
- Modify: `crates/post-machine/src/cli/driver.rs` (replace the `manifest_mode` stub)
- Test: extend `crates/post-machine/tests/build_driver.rs` (spawned-binary tests — manifest discovery starts at the process cwd)

**Interfaces:**
- Consumes: Task 2's `project::discover_manifest`, Task 1's `Manifest`/`Target`/`Profiles::resolve`/`normalize_rel`, the existing `LinkOptions.entry` field.
- Produces: working `fn manifest_mode(targets: &[String], flags: &Flags) -> Result<CliOutput, String>`; `fn build_one_target(root: &Path, manifest: &Manifest, name: &str, target: &Target, flags: &Flags) -> Result<(PathBuf, String), String>` returning `(output_path, stderr_chunk)` — Task 5's `--run` consumes the output path.

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

Note: `scratch` dirs persist across runs under `CARGO_TARGET_TMPDIR` — each test writes its full fixture, so reruns are self-overwriting; tests asserting absence (`!bench.pmx exists`) must use their own scratch name (they do). Scratch names are unique per test for the same reason the hygiene sweep's isolation fixes were needed: these run in parallel.

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p mtc-post-machine --test build_driver manifest 2>&1 | tail -5`
Expected: FAIL — stub error "manifest mode lands in the next task"

- [ ] **Step 3: Implement manifest mode** (replace the stub; `--run` gate lands here, execution in Task 5)

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
        return run_target(&root, output, target.run.as_ref(), stderr); // Task 5
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
    // (docs/pmt/cli.md (build)): only the individual flags (-g, -O*,
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

    // `entry` threads the manifest's per-target key into the linker's
    // BFS root; `call_mech` has no PM-1 analogue, so it stays default.
    let linked = crate::asm::link(
        &objects,
        &libraries,
        LinkOptions {
            relax: !flags.no_relax,
            entry: target.entry.clone(),
            ..Default::default()
        },
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
    Err("--run lands in the next task".to_string()) // Task 5 replaces this
}
```

Note: `project::Manifest`, `Target`, `RunSpec`, `normalize_rel` must be visible from `cli/` — they are `pub(crate)`.

- [ ] **Step 4: Run tests**

Run: `cargo test -p mtc-post-machine --test build_driver`
Expected: PASS except any `--run` test (none yet)

- [ ] **Step 5: Gates + commit**

```bash
cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --check
git add -A && git commit -m "feat(post-machine): pmt build manifest mode — targets, profiles, list-targets, declared-model flag rejection"
```

---

### Task 5: PM `--run` + `run.rs` settings/execution split

**Files:**
- Modify: `crates/post-machine/src/cli/run.rs` (extract `RunSettings` + `execute_run`; `run()` becomes parse→delegate)
- Modify: `crates/post-machine/src/cli/driver.rs` (replace `run_target` stub)
- Test: extend `crates/post-machine/tests/build_driver.rs`

**Interfaces:**
- Consumes: Task 4's `build_one_target` output path; `RunSpec` from the manifest.
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

`run()` keeps its exact current argument parsing (`--trace`, `-v` no-op, `--strict-cells`, `--no-step-limit`, `--max-steps`, `--max-tacts`, `--tact-profile`, `--tape-block`, `--tape`, `--head`, `--save-tape-block`), then builds a `RunSettings` from the parsed values and returns `execute_run(exe_path, &settings, trace_out)`. Behavior must be byte-identical — the existing `cli_programs.rs` run tests are the guard.

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
        // TactProfile has five fields since the TM-1 arc (table_read_cost,
        // frame_load_cost); the manifest declares only the three PM-1 ones,
        // so the rest come from the ELECTRONIC base.
        profile: spec.tact_profile.map_or(TactProfile::ELECTRONIC, |[m, r, w]| TactProfile {
            move_cost: m,
            read_cost: r,
            write_cost: w,
            ..TactProfile::ELECTRONIC
        }),
        trace: false,
    };
    let mut run_out = super::run::execute_run(output, &settings, &mut std::io::sink())?;
    run_out.stderr = format!("{build_stderr}{}", run_out.stderr);
    Ok(run_out)
}
```

(`RunSettings`/`execute_run` are `pub(super)` = visible throughout `cli/`.)

- [ ] **Step 5: Run the full suite** (run.rs refactor must not disturb existing run tests)

Run: `cargo test -p mtc-post-machine`
Expected: PASS — including `cli_programs.rs`'s run/trace tests and the two new `build_run` tests

- [ ] **Step 6: Gates + commit**

```bash
cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --check
git add -A && git commit -m "feat(post-machine): pmt build --run — manifest run blocks through a settings-split pmt run"
```

---

### Task 6: PM shell completion + the `docs/pmt/cli.md` build section

**Why one task:** `crates/post-machine/tests/cli_docs.rs` asserts that every
top-level subcommand the completion registry knows about has a verbatim
`--help` block on `docs/pmt/cli.md`. Registering `build_spec()` without the
page block turns that guard red, so the registry entry and its documented
usage block ship together.

**Files:**
- Modify: `crates/post-machine/src/completions/registry.rs` (new `PositionalHint::FilesOrTargets`, `build_spec()`, register it, `top_level_help`, the `registry()` doc-comment count 11 → 12)
- Modify: `crates/post-machine/src/completions/zsh.rs` (render the new hint + emit the `__pmt_build_targets` helper)
- Modify: `docs/pmt/cli.md` (the `pmt build` section + subcommand enumeration)
- Modify: `crates/post-machine/tests/cli_docs.rs` (add `build` to `quoted_blocks()`)
- Test: existing `tests/completions_registry.rs` (drift guard runs as-is), `tests/completions_zsh.rs`, `tests/cli_docs.rs`, unit tests in both source files

**Interfaces:**
- Consumes: Task 4's `--list-targets` output format (`NAME[\trun]` per line).
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

In `tests/cli_docs.rs`, add to `quoted_blocks()` after the `link` row:

```rust
        (Some("build"), vec!["build", "--help"]),
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p mtc-post-machine completions 2>&1 | tail -5; cargo test -p mtc-post-machine --test cli_docs 2>&1 | tail -5`
Expected: COMPILE ERROR (`FilesOrTargets` doesn't exist) and, once that compiles, a `cli_docs` failure naming `build` as undocumented.

- [ ] **Step 3: Implement the registry entry**

`registry.rs` — add the variant next to `File(FileHint)`:

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

Register `build_spec()` in `registry()` after `link_spec()`; add `"build" => "compile+link driver: .pmc/.pma/.pmo inputs or manifest targets",` to `top_level_help`; update the `registry()` doc comment from 11 to 12 top-level subcommands.

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

- [ ] **Step 4: Write the `pmt build` section on `docs/pmt/cli.md`**

Place it after the `pmt link` section. It must contain the `pmt build --help`
output **verbatim** in a fenced block (that is what `cli_docs.rs` compares
against `cli::execute(["build", "--help"])`), plus prose covering:

- both usage forms and the mode-dispatch rule (positional shape: any
  `.pmc`/`.pma`/`.pmo` ⇒ argv mode and the manifest is never read;
  otherwise target names ⇒ manifest mode; mixing is an error);
- the flag table split — compile side / link side (argv-only) / common —
  and why `-S`/`--emit-ir` are deliberately absent (per-file inspection
  stays `pmt compile`'s job);
- the manifest-mode rejection list (`-o`, `-L`, `-l`, `--nostdlib`);
- profile selection and flag-override precedence (flags win);
- `--run` exit codes (0 `stp` / 2 `hlt` / 3 trap, after a successful build);
- `--list-targets` format (`NAME`, tab, `run`);
- `--keep-objects` placement (next to each source, both modes);
- the undeclared-external refinement, and that `pmt compile` stays per-file honest;
- a pointer to `docs/pmt/project.md` for the manifest schema itself.

Update the page's subcommand enumeration to twelve.

- [ ] **Step 5: Run the completion + docs test suites**

Run: `cargo test -p mtc-post-machine --test completions_registry && cargo test -p mtc-post-machine --test completions_zsh && cargo test -p mtc-post-machine --test cli_docs && cargo test -p mtc-post-machine completions`
Expected: PASS. If the usage block and the page disagree, fix the PAGE — the binary's rendering is the source of truth. If the registry drift guard's probe reaches manifest-mode paths (e.g. probing `--run` errors with "no pmt.json"), that error does NOT contain "unknown flag" — the guard checks parser rejection, not success.

- [ ] **Step 6: Gates + commit**

```bash
cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --check
git add -A && git commit -m "feat(post-machine): build shell completion + documented usage — dynamic target names via --list-targets"
```

---

### Task 7: Bare `pmt lint` / `pmt fmt` over the manifest's declared source set

**Files:**
- Modify: `crates/post-machine/src/cli/lint.rs` (no-positional path)
- Modify: `crates/post-machine/src/cli/fmt.rs` (no-positional path)
- Modify: `crates/post-machine/src/project.rs` (add `Manifest::all_sources`)
- Test: extend `crates/post-machine/tests/build_driver.rs` (or a new `tests/manifest_lint_fmt.rs` — spawned binary, cwd-dependent)

**Interfaces:**
- Consumes: Task 2's `project::discover_manifest`, Task 1's `Manifest`.
- Produces (`pub(crate)`): `Manifest::all_sources(&self) -> Vec<String>` — the deduped union of every target's effective sources, in first-seen order, with `.pmo` entries dropped (nothing to lint or format in an object file).

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn bare_lint_uses_the_manifests_declared_source_set() {
    let dir = scratch("manifest_bare_lint");
    write_project(&dir);
    // A file NOT in the manifest must not be linted, even though it sits
    // in the same directory — the declared set is the set, never a scan.
    fs::write(dir.join("src/stray.pmc"), "main() { mark; mark; }").unwrap();

    let out = pmt().arg("lint").current_dir(&dir).output().unwrap();
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(!combined.contains("stray.pmc"), "undeclared file must not be linted: {combined}");
}

#[test]
fn bare_lint_without_a_manifest_errors_naming_what_was_searched() {
    let empty = scratch("manifest_bare_lint_absent");
    let out = pmt().arg("lint").current_dir(&empty).output().unwrap();
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("pmt.json"), "{stderr}");
    assert!(stderr.contains("project"), "{stderr}");
}

#[test]
fn bare_lint_rejects_no_config() {
    let dir = scratch("manifest_bare_lint_noconfig");
    write_project(&dir);
    let out = pmt().args(["lint", "--no-config"]).current_dir(&dir).output().unwrap();
    assert!(!out.status.success());
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("--no-config"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn bare_fmt_formats_exactly_the_declared_set() {
    let dir = scratch("manifest_bare_fmt");
    write_project(&dir);
    let stray = dir.join("src/stray.pmc");
    let unformatted = "main(){mark;}";
    fs::write(&stray, unformatted).unwrap();
    fs::write(dir.join("src/app.pmc"), "main(){@util();}").unwrap();

    let out = pmt().arg("fmt").current_dir(&dir).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(
        fs::read_to_string(&stray).unwrap(),
        unformatted,
        "an undeclared file must be left untouched"
    );
    assert_ne!(
        fs::read_to_string(dir.join("src/app.pmc")).unwrap(),
        "main(){@util();}",
        "a declared file is formatted in place"
    );
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p mtc-post-machine --test build_driver bare_ 2>&1 | tail -5`
Expected: FAIL — bare `pmt lint` today errors with its "needs at least one path" usage message.

- [ ] **Step 3: Add `all_sources` to `project.rs`**

```rust
impl Manifest {
    /// The union of every target's effective sources, first-seen order,
    /// deduped after lexical normalization, with `.pmo` entries dropped —
    /// the file set a bare `pmt lint` / `pmt fmt` operates on
    /// (docs/pmt/project.md (the declared source set)). Objects carry no
    /// text, so there is nothing in them to lint or format.
    pub(crate) fn all_sources(&self) -> Vec<String> {
        let mut seen: HashSet<PathBuf> = HashSet::new();
        let mut out: Vec<String> = Vec::new();
        for target in self.targets.values() {
            for raw in self.effective_sources(target) {
                if raw.ends_with(".pmo") {
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
}
```

Unit test in `project.rs` `mod tests`:

```rust
    #[test]
    fn all_sources_dedupes_across_targets_and_drops_objects() {
        let m = v(json!({
            "sources": ["shared.pmc"],
            "targets": {
                "a": { "sources": ["a.pmc", "vendor.pmo"] },
                "b": { "sources": ["b.pmc"] }
            }
        }))
        .unwrap();
        assert_eq!(
            m.all_sources(),
            vec!["shared.pmc".to_string(), "a.pmc".to_string(), "b.pmc".to_string()],
            "shared.pmc appears once, the .pmo is dropped"
        );
    }
```

- [ ] **Step 4: Implement the no-positional path in `lint.rs` and `fmt.rs`**

In both, where the current code errors on an empty positional list, first
try manifest discovery:

```rust
    // Bare invocation: the nearest manifest's declared source set, the
    // same discovery `pmt build` uses (docs/pmt/project.md (the declared
    // source set)). Never a directory scan — undeclared files are not
    // part of the project.
    if paths.is_empty() {
        if no_config {
            return Err(format!(
                "--no-config cannot combine with a bare invocation: the manifest IS the input\n\n{LINT_USAGE}"
            ));
        }
        let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
        let Some((manifest_path, manifest)) =
            crate::project::discover_manifest(&cwd).map_err(|e| e.to_string())?
        else {
            return Err(
                "no pmt.json with a `project` section found from the current directory upward"
                    .into(),
            );
        };
        let root = manifest_path.parent().expect("pmt.json has a parent");
        paths = manifest
            .all_sources()
            .iter()
            .map(|raw| {
                crate::project::normalize_rel(raw)
                    .map(|rel| root.join(rel).to_string_lossy().into_owned())
            })
            .collect::<Result<Vec<_>, _>>()?;
    }
```

`fmt.rs` uses the same block with `FMT_USAGE` in the `--no-config` message.
Both keep their existing behavior for every non-empty positional list, and
`fmt`'s in-place default is unchanged — the set is smaller and explicitly
declared, so no extra guard is warranted (the issue asked; this is the
answer). Update each `USAGE` string's positional line to note that omitting
paths uses the manifest's declared source set.

- [ ] **Step 5: Run tests**

Run: `cargo test -p mtc-post-machine`
Expected: PASS — including the existing lint/fmt CLI tests, which all pass explicit paths and are unaffected.

- [ ] **Step 6: Gates + commit**

```bash
cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --check
git add -A && git commit -m "feat(post-machine): bare pmt lint/fmt over the manifest's declared source set"
```

---

### Task 8: PM documentation — `docs/pmt/project.md`, README, CLAUDE.md

**Files:**
- Create: `docs/pmt/project.md`
- Modify: `docs/pmt/lint.md` (the "Project file: `pmt.json`" section points at `docs/pmt/project.md` for the `project` section)
- Modify: `README.md` (subcommand count; a short manifest example in the quickstart)
- Modify: `CLAUDE.md` (twelve `pmt` subcommands; `project.rs` + `cli/driver.rs` in the architecture notes; `docs/pmt/project.md` in the documentation-authority list; `pmt.json` schema 0.2 in the version-spaces section)

**Interfaces:** none — prose only. Published pages are ref-free and forge-agnostic.

Placement note: per-toolchain pages, not one shared page — the schema is
parallel but each toolchain has its own source kinds and run block (and
TM-1 alone has `call-mech`), so `project.md` follows the existing
cli/lint/fmt split. This matches the spec's Documentation section.

- [ ] **Step 1: Write `docs/pmt/project.md`** covering, in this order (draft from the spec's manifest + CLI sections; every claim must match implemented behavior — copy examples from the tests):
  1. What the project file is: `pmt.json`, the one project config file; schema version **0.2** (0.1 was the lint-only shape); the `lint` section cross-referencing `docs/pmt/lint.md`.
  2. Per-section discovery: lint = nearest `pmt.json`; project = nearest `pmt.json` WITH a `project` key; both stop at first hit, never merge; a lint-only file is transparent to the project walk. One loader validates the whole file, so a typo in either section surfaces for both consumers.
  3. The full annotated example (the spec's `app`/`bench` example).
  4. Key-by-key reference: `stdlib`, `sources`, `libraries.dirs`/`libraries.link` (first-wins order, shadowed by user definitions, lazy reachability), `profiles` (debug/release bases mirroring the CLI presets, per-key overrides), `targets.<name>` (`sources`, `libraries`, `entry` default `main` and must be exported, `output` default `<name>.pmx`, `run` block keys and the `tape` XOR `tape-block` / `head`-requires-`tape` rules).
  5. Path rules: relative to the manifest dir, `../` allowed, absolute rejected, lexical normalization only (symlink aliases undetected), duplicate/collision errors.
  6. The declared source set: what bare `pmt lint` / `pmt fmt` operate on (union across targets, deduped, `.pmo` dropped), and that `--no-config` cannot combine with a bare invocation.
  7. How `pmt build` consumes it (short; deep link to `docs/pmt/cli.md`), including the undeclared-external refinement rule and that `pmt compile` stays per-file honest.
- [ ] **Step 2: Point `docs/pmt/lint.md` at the new page** — one sentence in the project-file section: the same `pmt.json` may also carry a `project` section (see `docs/pmt/project.md`); its presence does not change lint discovery, and a bare `pmt lint` uses that section's declared source set.
- [ ] **Step 3: README + CLAUDE.md** — README: add `build` to the `pmt` subcommand listing with the one-liner from `USAGE`; add a ~10-line manifest example under the quickstart; add `docs/pmt/project.md` to the docs list. CLAUDE.md: `pmt`'s "eleven subcommands" → "twelve"; add `project.rs` (manifest schema/validation/discovery, one-loader rule) and `cli/driver.rs` to the post-machine architecture bullet; add `docs/pmt/project.md` to the documentation-authority paragraph; add the `pmt.json` schema 0.2 row to the version-spaces section.
- [ ] **Step 4: Verify claims against behavior**

Run: `cargo test -p mtc-post-machine 2>&1 | tail -3` (green baseline), then cross-check each `docs/pmt/project.md` claim that has a test (discovery, defaults, XOR rules, exit codes, declared source set) against the test expectations from Tasks 1–7.
Expected: no doc claim without a matching implemented behavior.

- [ ] **Step 5: Commit**

```bash
git add -A && git commit -m "docs(pmt): project manifest reference (schema 0.2), build CLI section, twelve-subcommand counts"
```

---

## TM-1 half (Tasks 9–16)

The TM tasks port the PM code that Tasks 1–8 put in the tree. That is a
deliberate duplication, not an oversight: the spec rules `project.rs` lives
per crate so core stays arch-agnostic and holds no manifest knowledge, the
same way each crate already carries its own `config.rs`, lint layer, and
completions registry. Where a task says "port `<file>`", the source file is
real and readable at that point — read it, copy it, then apply the
divergence list in the same task. Nothing here is "similar to Task N": every
divergence is spelled out with its code.

**The TM divergence list** (spec §203–248), applied throughout:

| Axis | PM-1 | TM-1 |
|---|---|---|
| Source kinds | `.pmc` / `.pma` / `.pmo` | `.tmc` / `.tma` / `.tmo` |
| Default output | `<name>.pmx` | `<name>.tmx` |
| Run block | `tape` XOR `tape-block`, `head`, `strict-cells`, `max-steps`, `max-tacts`, `tact-profile` | `tape` (a `.tmt` path, **required**), `max-steps`, `no-step-limit`, `max-tacts` — nothing else |
| `--run` with no run block | runs on `pmt run` defaults | **pointed error** — `tmt run` has no empty-tape default |
| Lowering | — | `call-mech` key (project default + per-target), `--call-mech` accepted as an override in manifest mode |
| Manifest-mode rejects | `-o`, `-L`, `-l`, `--nostdlib` | those **plus `--entry`** |
| Flag-only, never manifest keys | `--fno-<pass>` | `--fno-<pass>`, `--foutline` |
| Argv-mode exclusions | `-S`, `--emit-ir` | those **plus `--stamped-asm`** |
| `CompileOptions` extras | — | `outline: bool`, `stamped_asm: bool` |

---

### Task 9: `project.rs` — TM manifest schema types + validation walk

**Files:**
- Create: `crates/turing-machine/src/project.rs`
- Modify: `crates/turing-machine/src/lib.rs` (add `mod project;` next to `mod config;`)
- Modify: `crates/turing-machine/src/config.rs` (add `Invalid` variant to `ConfigError` + `path()`/`detail()` arms)
- Test: unit tests inside `project.rs`

**Interfaces:**
- Consumes: `crate::config::ConfigError`, `crate::optimizer::OptLevel`, `crate::lint::validate_allow`, `mtc_core::linker::CallMech`.
- Produces (all `pub(crate)`): the same surface as PM's Task 1 — `Manifest`, `Libraries`, `Profiles`, `ProfileOverrides`, `Target`, `RunSpec`, `ResolvedProfile`, `validate_manifest`, `normalize_rel`, `effective_sources`, `effective_libraries`, `output_of` — with the TM shapes below.

- [ ] **Step 1: Port the PM module**

Copy `crates/post-machine/src/project.rs` to `crates/turing-machine/src/project.rs`
verbatim, then apply Steps 2–4. Add the `Invalid` variant to
`crates/turing-machine/src/config.rs`'s `ConfigError` exactly as PM Task 1
Step 1 did (same doc comment, same `path()`/`detail()` arms). Add
`mod project;` to `lib.rs`.

Rewrite the module doc comment for the TM home:

```rust
//! The `project` section of `tmt.json`: the declared project model —
//! schema, validation, discovery (docs/tmt/project.md (schema)). The
//! strict twin of PM-1's `project.rs`, one crate over, because core
//! stays arch-agnostic and holds no manifest knowledge. Shared by
//! `tmt build` (cli/driver.rs) and the LSP. One loader validates the
//! WHOLE file (both sections) regardless of consumer.
```

- [ ] **Step 2: Apply the type divergences**

`Manifest` gains a project-level `call_mech`, `Target` gains a per-target one:

```rust
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

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Target {
    pub sources: Vec<String>,
    pub libraries: Libraries,
    pub entry: Option<String>,
    pub output: Option<String>,
    pub call_mech: Option<CallMech>,
    pub run: Option<RunSpec>,
}
```

`RunSpec` shrinks to what `tmt run` actually accepts — no `head`, no
`strict_cells`, no `tact_profile`, and `tape` is a `.tmt` path rather than
an inline glyph string:

```rust
/// A TM-1 run block. `tmt run` always drives a whole multi-tape band
/// loaded from a `.tmt` snapshot: there is no inline-glyph form, no
/// head, no strict-cells decorator, no tact-profile knob
/// (docs/tmt/cli.md (run)). `tape` is therefore required for the block
/// to be runnable at all.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct RunSpec {
    pub tape: Option<String>,
    pub max_steps: Option<u64>,
    pub no_step_limit: bool,
    pub max_tacts: Option<u64>,
}
```

In the ported `impl Manifest` block, `output_of` switches extension and a
lowering resolver joins it (`effective_sources` / `effective_libraries` port
unchanged):

```rust
impl Manifest {
    pub(crate) fn output_of(&self, name: &str, target: &Target) -> String {
        target.output.clone().unwrap_or_else(|| format!("{name}.tmx"))
    }

    /// Effective lowering for one target: its own key, else the
    /// project default, else `None` (the linker's own default). The
    /// `--call-mech` flag overrides the result at the driver — flags
    /// win, as the profile keys do (docs/tmt/project.md (call-mech)).
    pub(crate) fn effective_call_mech(&self, target: &Target) -> Option<CallMech> {
        target.call_mech.or(self.call_mech)
    }
}
```

- [ ] **Step 3: Apply the parse divergences**

Replace `parse_run` wholesale:

```rust
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
```

Add a `call-mech` value parser:

```rust
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
```

Wire both new keys into their walks — in `parse_target`'s match:

```rust
            "call-mech" => target.call_mech = Some(parse_call_mech_value(path, val)?),
```

and in `validate_manifest`'s top-level match:

```rust
            "call-mech" => manifest.call_mech = Some(parse_call_mech_value(path, val)?),
```

(Both `Manifest` and `Target` literals in this file gain `call_mech: None`
in their initializers.)

- [ ] **Step 4: Rewrite the tests for TM shapes**

Port PM's `mod tests` with `.pmc` → `.tmc`, `.pmx` → `.tmx`,
`/x/pmt.json` → `/x/tmt.json`, and replace the run-block test with:

```rust
    #[test]
    fn run_block_accepts_only_the_tmt_keys() {
        let base = |run: serde_json::Value| {
            json!({ "targets": { "a": { "sources": ["m.tmc"], "run": run } } })
        };
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
            v(base(json!({ "tape": "t.tmt", "no-step-limit": true, "max-steps": 10 }))).is_err(),
            "max-steps and no-step-limit contradict"
        );
        assert!(v(base(json!({}))).is_ok(), "an empty run block parses (but cannot --run)");
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
        assert_eq!(m.effective_call_mech(&m.targets["a"]), Some(CallMech::Frames));
        assert_eq!(m.effective_call_mech(&m.targets["b"]), Some(CallMech::Mono));

        let err = v(json!({
            "call-mech": "monolithic",
            "targets": { "a": { "sources": ["a.tmc"] } }
        }))
        .unwrap_err();
        assert!(err.detail().contains("mono, frames, hybrid"), "{}", err.detail());
    }

    #[test]
    fn default_output_is_tmx() {
        let m = v(json!({ "targets": { "utm": { "sources": ["utm.tmc"] } } })).unwrap();
        assert_eq!(m.output_of("utm", &m.targets["utm"]), "utm.tmx");
    }
```

- [ ] **Step 5: Run tests, gates, commit**

Run: `cargo test -p mtc-turing-machine project::`
Expected: PASS

```bash
cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --check
git add -A && git commit -m "feat(turing-machine): tmt.json project section — schema types + validation walk (schema 0.2)"
```

---

### Task 10: One TM loader for the whole file + per-section discovery

**Files:**
- Modify: `crates/turing-machine/src/project.rs` (add `TmtFile`, `load_file`, `discover_manifest`)
- Modify: `crates/turing-machine/src/config.rs` (`load` delegates; move the lint walk into `project.rs`)
- Test: unit tests in both files

**Interfaces:**
- Consumes: Task 9's `validate_manifest`; `config::discover` (unchanged).
- Produces (`pub(crate)`): `struct TmtFile { allow: Vec<String>, manifest: Option<Manifest> }`; `fn load_file(path: &Path) -> Result<TmtFile, ConfigError>`; `fn discover_manifest(start: &Path) -> Result<Option<(PathBuf, Manifest)>, ConfigError>`.

- [ ] **Step 1: Port PM Task 2 verbatim, with three substitutions**

Copy the `PmtFile`/`load_file`/`discover_manifest` block and the
`config.rs` delegation from `crates/post-machine/src/project.rs` +
`config.rs`, substituting throughout:

- the type name `PmtFile` → `TmtFile`,
- the filename literal `"pmt.json"` → `"tmt.json"` (it appears in
  `discover_manifest`'s walk),
- doc-comment citations `docs/pmt/project.md` → `docs/tmt/project.md`.

The lint-section walk (`parse_lint`) ports unchanged — TM's
`lint::validate_allow` has the same signature and the same
`LintError::UnknownAllowCode` shape, and TM's allow namespace is already
shared across its four surfaces.

- [ ] **Step 2: Port the tests**

Copy PM Task 2's four tests with `pmt.json` → `tmt.json`, `.pmc` → `.tmc`,
and an allow code that exists in the TM catalog (`"leftover-debugger"` is
present on both the `.tmc` and `.tma` sides):

```rust
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

        let (found, manifest) = discover_manifest(&sub).unwrap().expect("project above");
        assert_eq!(found, root.join("tmt.json"));
        assert!(manifest.targets.contains_key("app"));

        assert_eq!(crate::config::discover(&sub), Some(sub.join("tmt.json")));
    }
```

plus the `one_loader_a_broken_project_section_fails_the_lint_load_too`,
`load_file_reads_both_sections`, and
`discover_manifest_errors_on_a_malformed_candidate` equivalents.

- [ ] **Step 3: Run the full crate suite**

Run: `cargo test -p mtc-turing-machine`
Expected: PASS — the existing `tmt.json` config tests must survive the delegation.

- [ ] **Step 4: Gates + commit**

```bash
cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --check
git add -A && git commit -m "feat(turing-machine): one tmt.json loader — full-file validation, per-section manifest discovery"
```

---

### Task 11: `cli/driver.rs` — TM argv mode

**Files:**
- Create: `crates/turing-machine/src/cli/driver.rs`
- Modify: `crates/turing-machine/src/cli/mod.rs` (add `mod driver;`, dispatch `Some("build")`, add the `build` line to `USAGE`)
- Modify: `crates/turing-machine/src/cli/build.rs` (make `out_path`, `render_warnings`, `render_opt_report`, `read_object`, `find_library`, `sidecar_path`, `take_disabled_passes`, `parse_call_mech` `pub(super)`)
- Modify: `docs/tmt/cli.md` — one line only, adding `build` to the root `SUBCOMMANDS:` list. **Forced, not optional**, exactly as its PM twin was in Task 3: `crates/turing-machine/tests/cli_docs.rs` quotes the ROOT usage block verbatim, so adding the `build` line to `USAGE` turns that guard red here. The full `## tmt build` section belongs to Task 14.
- Test: create `crates/turing-machine/tests/build_driver.rs`

**Interfaces:**
- Consumes: `compiler::compile`, `crate::asm::assemble(&str, bool)`, `crate::asm::link`, `stdlib::object()`, the `cli/build.rs` helpers above (including `parse_call_mech`, reused rather than re-parsed).
- Produces: `pub(super) fn build(raw: &[String]) -> Result<CliOutput, String>`; `struct Flags`; `undeclared_name`; `defined_names`; `refine_reports` — same shapes as PM Task 3.

- [ ] **Step 1: Port PM Task 3's test file**

Copy `crates/post-machine/tests/build_driver.rs` to
`crates/turing-machine/tests/build_driver.rs` with `mtc_post_machine` →
`mtc_turing_machine`, extensions `.pmc`/`.pma`/`.pmo`/`.pmx` →
`.tmc`/`.tma`/`.tmo`/`.tmx`, and `.tmc` fixture sources in place of the
`.pmc` ones. Use the smallest programs that exercise the same paths —
draw them from `crates/turing-machine/tests/tmc_golden.rs`'s existing
fixtures rather than inventing syntax. Extend the excluded-flag test:

```rust
#[test]
fn argv_mode_rejects_s_emit_ir_and_stamped_asm() {
    let dir = scratch("argv_no_inspect_flags");
    let main = dir.join("main.tmc");
    fs::write(&main, MAIN_TMC).unwrap();
    for flag in ["-S", "--emit-ir", "--stamped-asm"] {
        let err = execute(&args(&["build", flag, main.to_str().unwrap()])).unwrap_err();
        assert!(err.contains("unknown flag"), "{flag}: {err}");
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p mtc-turing-machine --test build_driver 2>&1 | tail -5`
Expected: FAIL — `unknown subcommand \`build\``

- [ ] **Step 3: Port `cli/driver.rs` with the TM flag set**

Copy PM's `cli/driver.rs`, then:

Rewrite `BUILD_USAGE` for `tmt` (this string is what the `cli_docs` guard
will compare against `docs/tmt/cli.md` in Task 14 — keep it accurate):

```rust
const BUILD_USAGE: &str = "\
USAGE: tmt build [INPUT.tmc|.tma|.tmo ...] [-o OUT.tmx] [FLAGS]   (argv mode)
       tmt build [TARGET ...] [FLAGS]                             (manifest mode)

Argv mode compiles/assembles/loads every input in memory, links with
the stdlib, and writes OUT.tmx (+ .tmx.map). Manifest mode discovers
the nearest tmt.json with a `project` section from the current
directory and builds its targets (all of them when none is named).

COMPILE FLAGS (argv mode; manifest mode: override the profile):
  --debug | --release   presets (manifest mode: profile selection)
  -O0 | -O1             optimization level
  -g                    record debug info
  --strip-debugger      drop `brk` at codegen
  --fno-<pass>          disable one optimizer pass (repeatable)
  --foutline            enable the default-off `outline` pass
  -Werror               treat (post-refinement) warnings as errors

LINK FLAGS (argv mode only; the manifest declares these):
  --nostdlib            do not link the built-in std
  -L DIR / -l NAME      library search dir / library (repeatable)
  --entry NAME          link NAME as the program entry (default: main)
  -o OUT.tmx            output path

COMMON:
  --no-relax            keep every symbol site in far form
  --call-mech MECH      bound-call lowering: mono | frames | hybrid
  --keep-objects        write each intermediate .tmo next to its source
  --run [TARGET]        manifest mode: build, then run the target's run block
  --list-targets        manifest mode: print `NAME[\\trun]` per target
  -v                    render the build report
";
```

`Flags` gains four fields and loses none:

```rust
    outline: bool,
    entry: Option<String>,
    call_mech: Option<CallMech>,
```

parsed as:

```rust
        outline: args.flag("--foutline"),
        entry: args.value("--entry")?,
        // `--call-mech` is COMMON, not argv-only: manifest mode accepts it
        // as an override of the declared lowering (docs/tmt/project.md
        // (call-mech)). Parsed with the same function `tmt link` uses.
        call_mech: {
            let raw = args.value("--call-mech")?;
            match raw {
                Some(_) => Some(super::build::parse_call_mech(raw)?),
                None => None,
            }
        },
```

The `is_file` predicate switches extensions:

```rust
    let is_file = |s: &str| {
        s.ends_with(".tmc") || s.ends_with(".tma") || s.ends_with(".tmo")
    };
```

`argv_compile_options` gains the two TM fields:

```rust
        outline: flags.outline,
        stamped_asm: false, // --stamped-asm is a compile-only emit knob
```

The argv-mode link call threads all three fields explicitly (TM's own
`link` does the same — there is no default to lean on for `call_mech`
once the flag exists):

```rust
    let linked = crate::asm::link(
        &objects,
        &libraries,
        LinkOptions {
            relax: !flags.no_relax,
            entry: flags.entry.clone(),
            call_mech: flags.call_mech.unwrap_or_default(),
        },
    )
    .map_err(|e| e.to_string())?;
```

and the output extension becomes `"tmx"` in the `out_path` call. Doc
comments cite `docs/tmt/cli.md (build)` and `docs/tmt/project.md`.

In `cli/mod.rs`: `mod driver;`, `Some("build") => driver::build(&args[1..]),`
after `Some("link")`, and the `USAGE` line:

```
  build        compile+link driver: .tmc/.tma/.tmo inputs or manifest targets
```

In `cli/build.rs`: widen the seven helpers plus `parse_call_mech` to `pub(super)`.

- [ ] **Step 4: Run tests**

Run: `cargo test -p mtc-turing-machine --test build_driver && cargo test -p mtc-turing-machine driver::`
Expected: PASS

- [ ] **Step 5: Gates + commit**

```bash
cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --check
git add -A && git commit -m "feat(turing-machine): tmt build argv mode — in-memory cc driver with undeclared-external refinement"
```

---

### Task 12: `cli/driver.rs` — TM manifest mode + `--list-targets`

**Files:**
- Modify: `crates/turing-machine/src/cli/driver.rs` (replace the `manifest_mode` stub)
- Test: extend `crates/turing-machine/tests/build_driver.rs`

**Interfaces:**
- Consumes: Task 10's `project::discover_manifest`, Task 9's `Manifest`/`Target`/`Profiles::resolve`/`effective_call_mech`/`normalize_rel`.
- Produces: working `manifest_mode`; `build_one_target(root, manifest, name, target, flags) -> Result<(PathBuf, String), String>`.

- [ ] **Step 1: Write the failing E2E tests**

```rust
use std::process::Command;

fn tmt() -> Command {
    Command::new(env!("CARGO_BIN_EXE_tmt"))
}

/// Three targets over two real `.tmc` programs. `app` carries a run block
/// with a `.tmt` tape; `notape` deliberately has none, for Task 13's
/// pointed-error test. The sources are the committed golden fixtures — do
/// not invent `.tmc` syntax here.
///
/// `A1_REPLACE_B` is the text of
/// `crates/turing-machine/tests/golden/a1_replace_b.tmc`:
///
/// ```text
/// ? Walk right; replace every 'b' with 'a'; stop at the first blank.
///
/// alphabet ab { '_', 'a', 'b' }
///
/// machine {
///   tape main: ab;
///
///   entry state scan {
///     ['b'] -> write ['a'] move [>] goto scan;
///     ['a'] ->            move [>] goto scan;
///     ['_'] -> stop;
///   }
/// }
/// ```
///
/// `A2_BINARY_PLUS_ONE` is the text of `golden/a2_binary_plus_one.tmc`
/// (11 lines, same shape). Read both files and paste them as `const`s
/// rather than retyping — they are the durable teaching fixtures the
/// golden suite already pins.
fn write_project(dir: &PathBuf) {
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::write(dir.join("src/app.tmc"), A1_REPLACE_B).unwrap();
    fs::write(dir.join("src/other.tmc"), A2_BINARY_PLUS_ONE).unwrap();
    fs::write(
        dir.join("tmt.json"),
        r#"{ "project": {
            "call-mech": "hybrid",
            "targets": {
                "app":    { "sources": ["src/app.tmc"],
                            "run": { "tape": "tapes/app-in.tmt", "max-steps": 100000 } },
                "notape": { "sources": ["src/other.tmc"] },
                "zmono":  { "sources": ["src/other.tmc"], "call-mech": "mono",
                            "output": "out/zmono.tmx" }
            }
        } }"#,
    )
    .unwrap();

    // The tape must match the image's band count, so mint it from a built
    // image with the real tooling rather than hand-rolling bytes:
    // `tmt tape new --from APP.tmx -o OUT.tmt`, then seed cells with
    // `tmt tape set` (docs/tmt/cli.md (tape)).
    tmt().args(["build", "app"]).current_dir(dir).output().unwrap();
    fs::create_dir_all(dir.join("tapes")).unwrap();
    let out = tmt()
        .args(["tape", "new", "--from", "app.tmx", "-o", "tapes/app-in.tmt"])
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
}

#[test]
fn manifest_mode_rejects_declared_model_flags_including_entry() {
    let dir = scratch("tm_manifest_reject_flags");
    write_project(&dir);
    for flagset in [
        vec!["-o", "x.tmx"],
        vec!["-L", "libs"],
        vec!["-l", "x"],
        vec!["--nostdlib"],
        vec!["--entry", "other"],
    ] {
        let mut cmd = tmt();
        cmd.arg("build").args(&flagset).arg("app").current_dir(&dir);
        let out = cmd.output().unwrap();
        assert!(!out.status.success(), "{flagset:?} must be rejected in manifest mode");
        assert!(
            String::from_utf8_lossy(&out.stderr).contains("manifest"),
            "{flagset:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
}

#[test]
fn call_mech_flag_overrides_the_manifest_declaration() {
    let dir = scratch("tm_manifest_call_mech");
    write_project(&dir);
    // Accepted (unlike -o/-L/-l/--nostdlib/--entry): the manifest records
    // the committed lowering, the flag exists for experiments against it.
    let out = tmt()
        .args(["build", "--call-mech", "frames", "app"])
        .current_dir(&dir)
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(dir.join("app.tmx").is_file());
}
```

plus the ports of PM Task 4's `manifest_mode_bare_build_builds_all_targets_alphabetically`,
`manifest_mode_named_target_builds_only_it`,
`manifest_mode_discovery_walks_up_from_a_subdirectory`,
`manifest_mode_unknown_target_and_missing_manifest_error`,
`list_targets_prints_name_and_run_marker`, and
`release_flag_selects_the_release_profile`, with `pmt`→`tmt`,
`pmt.json`→`tmt.json`, `.pmx`→`.tmx`.

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p mtc-turing-machine --test build_driver manifest 2>&1 | tail -5`
Expected: FAIL — the stub error

- [ ] **Step 3: Port PM's `manifest_mode` + `build_one_target` with the TM divergences**

The rejection guard adds `--entry` and deliberately omits `--call-mech`:

```rust
    if flags.out.is_some()
        || !flags.search_dirs.is_empty()
        || !flags.lib_names.is_empty()
        || flags.nostdlib
        || flags.entry.is_some()
    {
        return Err(format!(
            "-o/-L/-l/--nostdlib/--entry contradict the manifest — it declares outputs, libraries and entries\n\n{BUILD_USAGE}"
        ));
    }
```

`build_one_target`'s compile options carry the TM extras:

```rust
    let mut options = CompileOptions {
        debug_info: if flags.debug_info { true } else { profile.debug_info },
        strip_debugger: if flags.strip_debugger { true } else { profile.strip_debugger },
        opt_level: profile.opt_level,
        disabled_passes: flags.disabled_passes.clone(),
        capture_ir: false,
        // Flag-only axes: never manifest keys (docs/tmt/project.md
        // (profiles)). `--stamped-asm` is a compile-emit knob the driver
        // does not expose at all.
        outline: flags.outline,
        stamped_asm: false,
    };
```

and its link call resolves the lowering flag-first, then target, then project:

```rust
    let linked = crate::asm::link(
        &objects,
        &libraries,
        LinkOptions {
            relax: !flags.no_relax,
            entry: target.entry.clone(),
            // Flags win over the declared lowering, exactly as the
            // profile flags win over profile keys.
            call_mech: flags
                .call_mech
                .or_else(|| manifest.effective_call_mech(target))
                .unwrap_or_default(),
        },
    )
    .map_err(|e| format!("target `{name}`: {e}"))?;
```

Source dispatch matches on `"tmc"` / `"tma"` (everything else loads as an
object), and the output resolves through `manifest.output_of` (`.tmx`).

The `run_target` stub carries the target name from the start, so Task 13
replaces a body rather than changing a signature:

```rust
fn run_target(
    _root: &Path,
    _output: &Path,
    _name: &str,
    _run: Option<&crate::project::RunSpec>,
    _stderr: String,
) -> Result<CliOutput, String> {
    Err("--run lands in the next task".to_string()) // Task 13 replaces this
}
```

and `manifest_mode`'s `--run` arm calls it as
`run_target(&root, output, name, target.run.as_ref(), stderr)`.

- [ ] **Step 4: Run tests**

Run: `cargo test -p mtc-turing-machine --test build_driver`
Expected: PASS except `--run` tests (none yet)

- [ ] **Step 5: Gates + commit**

```bash
cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --check
git add -A && git commit -m "feat(turing-machine): tmt build manifest mode — targets, profiles, call-mech resolution, list-targets"
```

---

### Task 13: TM `--run` + `run.rs` settings/execution split

**Files:**
- Modify: `crates/turing-machine/src/cli/run.rs` (extract `RunSettings` + `execute_run`)
- Modify: `crates/turing-machine/src/cli/driver.rs` (replace `run_target` stub)
- Test: extend `crates/turing-machine/tests/build_driver.rs`

**Interfaces:**
- Consumes: Task 12's `build_one_target` output path; Task 9's `RunSpec`.
- Produces in `run.rs` (`pub(super)`): `struct RunSettings { tape: Option<String>, no_step_limit: bool, max_steps: Option<u64>, max_tacts: Option<u64>, trace: bool }` with `Default`; `fn execute_run(exe_path: &Path, settings: &RunSettings, trace_out: &mut dyn std::io::Write) -> Result<CliOutput, String>`.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn build_run_adopts_the_machine_exit_code() {
    let dir = scratch("tm_manifest_run");
    write_project(&dir);

    // Asserted as an EQUIVALENCE rather than a hardcoded number: whatever
    // `tmt run` reports for this image+tape, `tmt build --run` must report
    // the same (0 stopped / 2 hlt / 3 trap). That stays true if the fixture
    // is ever swapped for a different golden program.
    tmt().args(["build", "app"]).current_dir(&dir).output().unwrap();
    let direct = tmt()
        .args(["run", "--tape", "tapes/app-in.tmt", "app.tmx"])
        .current_dir(&dir)
        .output()
        .unwrap();
    let driven = tmt().args(["build", "--run", "app"]).current_dir(&dir).output().unwrap();
    assert_eq!(
        driven.status.code(),
        direct.status.code(),
        "build --run must adopt tmt run's outcome code: {}",
        String::from_utf8_lossy(&driven.stderr)
    );
}

#[test]
fn build_run_on_a_target_without_a_tape_is_a_pointed_error() {
    let dir = scratch("tm_manifest_run_no_tape");
    write_project(&dir);
    // `notape` declares no run block; `tmt run` has no empty-tape default,
    // so this must name the problem rather than run something invented.
    let out = tmt().args(["build", "--run", "notape"]).current_dir(&dir).output().unwrap();
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("tape"), "{stderr}");
    assert!(stderr.contains("notape"), "{stderr}");
}

#[test]
fn build_run_without_a_named_target_needs_exactly_one() {
    let dir = scratch("tm_manifest_run_ambiguous");
    write_project(&dir);
    let out = tmt().args(["build", "--run"]).current_dir(&dir).output().unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("exactly one"));
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p mtc-turing-machine --test build_driver build_run 2>&1 | tail -5`
Expected: FAIL — the stub error

- [ ] **Step 3: Refactor `run.rs`**

Same shape as PM Task 5 Step 3, with TM's smaller settings struct:

```rust
#[derive(Debug, Clone, Default)]
pub(super) struct RunSettings {
    /// `--tape PATH.tmt`. `None` reaches `execute_run` only from a bare
    /// `tmt run` with no flag, which errors there as it does today —
    /// the driver checks earlier so it can name the target.
    pub tape: Option<String>,
    pub no_step_limit: bool,
    pub max_steps: Option<u64>,
    pub max_tacts: Option<u64>,
    pub trace: bool,
}

pub(super) fn execute_run(
    exe_path: &Path,
    settings: &RunSettings,
    trace_out: &mut dyn std::io::Write,
) -> Result<CliOutput, String> {
    // body: run()'s current lines from `let bytes = fs::read(exe_path)…`
    // onward — including the `run needs --tape TAPES.tmt` guard and the
    // band-count check — with each local replaced by settings.*.
}
```

`run()` keeps its exact current parsing (`--trace`, `-v` no-op,
`--no-step-limit`, `--max-steps`, `--max-tacts`, `--tape`) and delegates.
Behavior must be byte-identical; `cli_programs.rs`'s run tests are the guard.

- [ ] **Step 4: Implement `run_target` in `driver.rs`**

```rust
fn run_target(
    root: &Path,
    output: &Path,
    name: &str,
    run: Option<&crate::project::RunSpec>,
    build_stderr: String,
) -> Result<CliOutput, String> {
    // `tmt run` always drives a band loaded from a .tmt snapshot — there
    // is no empty-tape default to fall back on, so a target without a
    // declared tape cannot be --run (docs/tmt/project.md (run blocks)).
    let Some(spec) = run else {
        return Err(format!(
            "target `{name}` declares no `run` block: --run needs one with a `tape`"
        ));
    };
    let Some(raw_tape) = spec.tape.clone() else {
        return Err(format!(
            "target `{name}`'s run block declares no `tape`: tmt run needs a .tmt snapshot"
        ));
    };
    let settings = super::run::RunSettings {
        tape: Some(
            root.join(crate::project::normalize_rel(&raw_tape)?)
                .to_string_lossy()
                .into_owned(),
        ),
        no_step_limit: spec.no_step_limit,
        max_steps: spec.max_steps,
        max_tacts: spec.max_tacts,
        trace: false,
    };
    let mut run_out = super::run::execute_run(output, &settings, &mut std::io::sink())?;
    run_out.stderr = format!("{build_stderr}{}", run_out.stderr);
    Ok(run_out)
}
```

Thread the target name through `manifest_mode`'s call site so the errors
above can name it.

- [ ] **Step 5: Run the full suite**

Run: `cargo test -p mtc-turing-machine`
Expected: PASS

- [ ] **Step 6: Gates + commit**

```bash
cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --check
git add -A && git commit -m "feat(turing-machine): tmt build --run — declared .tmt run blocks through a settings-split tmt run"
```

---

### Task 14: TM shell completion + the `docs/tmt/cli.md` build section

Same coupling as Task 6: `crates/turing-machine/tests/cli_docs.rs` fails the
moment the registry knows a subcommand the page does not quote, so the
registry entry and the documented usage block ship together.

**Files:**
- Modify: `crates/turing-machine/src/completions/registry.rs` (`PositionalHint::FilesOrTargets`, `build_spec()`, register it, `top_level_help`, the `registry()` doc-comment count)
- Modify: `crates/turing-machine/src/completions/zsh.rs` (render the hint + emit `__tmt_build_targets`)
- Modify: `docs/tmt/cli.md` (the `tmt build` section + subcommand enumeration; the page has a `cli_docs` verbatim-quote guard)
- Modify: `crates/turing-machine/tests/cli_docs.rs` (add `build` to `quoted_blocks()`)
- Test: `tests/completions_registry.rs`, `tests/completions_zsh.rs`, `tests/cli_docs.rs`, unit tests in both source files

- [ ] **Step 1: Write the failing tests** — port Task 6 Step 1 with `tmt` names, `["tmc", "tma", "tmo"]` extensions, `__tmt_build_targets`, and an extra assertion that `--call-mech` and `--foutline` are registered:

```rust
        assert!(build.flags.iter().any(|f| f.name == "--call-mech"));
        assert!(build.flags.iter().any(|f| f.name == "--foutline"));
        assert!(build.flags.iter().any(|f| f.name == "--entry"));
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p mtc-turing-machine completions 2>&1 | tail -5; cargo test -p mtc-turing-machine --test cli_docs 2>&1 | tail -5`
Expected: COMPILE ERROR, then a `cli_docs` failure naming `build`.

- [ ] **Step 3: Implement the registry entry**

Port PM's `FilesOrTargets` variant and `build_spec()`, with the TM flag table:

```rust
fn build_spec() -> CommandSpec {
    CommandSpec {
        path: strings(&["build"]),
        positional: Positional::OneOrMore(PositionalHint::FilesOrTargets(ext(&[
            "tmc", "tma", "tmo",
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
            FlagSpec::boolean("--foutline", "enable the default-off `outline` pass"),
            FlagSpec::boolean("-Werror", "treat post-refinement warnings as errors"),
            FlagSpec::boolean("--no-relax", "keep every symbol site in far form"),
            FlagSpec::boolean("--nostdlib", "argv mode: do not link the built-in std"),
            FlagSpec::value("-L", "argv mode: library search directory", ValueHint::Directory)
                .repeatable(),
            FlagSpec::value("-l", "argv mode: link NAME.tmo from the search path", ValueHint::Text)
                .repeatable(),
            FlagSpec::value("--entry", "argv mode: link NAME as the program entry", ValueHint::Text),
            FlagSpec::value(
                "--call-mech",
                "bound-call lowering (overrides the manifest)",
                ValueHint::Choices(strings(&["mono", "frames", "hybrid"])),
            ),
            FlagSpec::value("-o", "argv mode: output path", ValueHint::File(any_file())),
            FlagSpec::boolean("--keep-objects", "write each intermediate .tmo next to its source"),
            FlagSpec::boolean("--run", "manifest mode: build then run the target"),
            FlagSpec::boolean("--list-targets", "manifest mode: print NAME[\\trun] per target"),
            FlagSpec::boolean("-v", "render the build report"),
            FlagSpec::boolean("--help", "show subcommand help"),
        ],
    }
}
```

Register after `link_spec()`; add the `top_level_help` row; update the
`registry()` doc comment (its current wording is "ten top-level
subcommands … plus `completions`" — make the count and the list include
`build`). `zsh.rs` gets the same two match arms plus:

```zsh
__tmt_build_targets() {
  local -a __targets
  __targets=(${(f)"$(tmt build --list-targets 2>/dev/null)"})
  __targets=(${__targets%%$'\t'*})
  (( ${#__targets} )) && compadd -a __targets
}
```

- [ ] **Step 4: Write the `tmt build` section on `docs/tmt/cli.md`**

Same content list as Task 6 Step 4, with the TM divergences documented
explicitly: `.tmc`/`.tma`/`.tmo` inputs and `.tmx` output; the rejection
list including `--entry`; `--call-mech` accepted in manifest mode as an
override and why (the manifest records the committed lowering, the flag is
for experiments); `--foutline`/`--fno-<pass>` flag-only; `-S`/`--emit-ir`/
`--stamped-asm` excluded; and that `--run` needs a run block with a `tape`
because `tmt run` has no empty-tape default. The `tmt build --help` block
must be verbatim. Update the subcommand enumeration to twelve.

- [ ] **Step 5: Run the completion + docs suites**

Run: `cargo test -p mtc-turing-machine --test completions_registry && cargo test -p mtc-turing-machine --test completions_zsh && cargo test -p mtc-turing-machine --test cli_docs`
Expected: PASS

- [ ] **Step 6: Gates + commit**

```bash
cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --check
git add -A && git commit -m "feat(turing-machine): build shell completion + documented usage — dynamic target names via --list-targets"
```

---

### Task 15: Bare `tmt lint` / `tmt fmt` over the manifest's declared source set

**Files:**
- Modify: `crates/turing-machine/src/cli/lint.rs`, `crates/turing-machine/src/cli/fmt.rs`
- Modify: `crates/turing-machine/src/project.rs` (add `Manifest::all_sources`)
- Test: extend `crates/turing-machine/tests/build_driver.rs`

**Interfaces:**
- Consumes: Task 10's `discover_manifest`, Task 9's `Manifest`.
- Produces: `Manifest::all_sources(&self) -> Vec<String>` — deduped union across targets, `.tmo` dropped. `.tma` entries are KEPT: unlike `.pmo`, hand-written assembly is a lintable, formattable source on the TM side (both languages have lint and fmt layers).

- [ ] **Step 1: Port PM Task 7 Steps 1–4** with `pmt`→`tmt`, `.pmc`→`.tmc`, `.pmo`→`.tmo`, and one added test:

```rust
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
```

The `all_sources` body drops only `.tmo`:

```rust
                if raw.ends_with(".tmo") {
                    continue;
                }
```

- [ ] **Step 2: Run tests**

Run: `cargo test -p mtc-turing-machine`
Expected: PASS — the existing lint/fmt CLI tests pass explicit paths and are unaffected.

- [ ] **Step 3: Gates + commit**

```bash
cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --check
git add -A && git commit -m "feat(turing-machine): bare tmt lint/fmt over the manifest's declared source set"
```

---

### Task 16: TM documentation + the version block

**Files:**
- Create: `docs/tmt/project.md`
- Modify: `docs/tmt/cli.md` (the existing `## tmt.json` section points at `docs/tmt/project.md` for the `project` section)
- Modify: `docs/tmt/lint.md` (project-file section likewise)
- Modify: `README.md` (`tmt` subcommand count; the manifest mention covers both tools)
- Modify: `CLAUDE.md` (twelve `tmt` subcommands; `project.rs` + `cli/driver.rs` in the turing-machine architecture bullet; `docs/tmt/project.md` in the documentation-authority list; the `tmt.json` schema 0.2 row in version spaces)

**Interfaces:** none — prose only, ref-free and forge-agnostic.

- [ ] **Step 1: Write `docs/tmt/project.md`** — the same seven-part structure as `docs/pmt/project.md`, with the TM content: `.tmc`/`.tma`/`.tmo` sources, `<name>.tmx` outputs, the `call-mech` key (project default + per-target, `--call-mech` overrides, the three values), the `tape`-only run block and why (`tmt run` drives a band from a `.tmt` snapshot, no inline glyphs, no head, no strict-cells, no tact-profile), the `--run` requires-a-tape rule, and the TM-1 entry semantics the PM design never faced: a non-`main` entry fixes the machine's tape arity and, in sectioned/frames output, must carry a routine signature — the manifest names the entry, the linker enforces the rest.
- [ ] **Step 2: Cross-link** — `docs/tmt/cli.md`'s `## tmt.json` section and `docs/tmt/lint.md`'s project-file section each gain one sentence: the same `tmt.json` may carry a `project` section (see `docs/tmt/project.md`); its presence does not change lint discovery, and a bare `tmt lint` uses that section's declared source set.
- [ ] **Step 3: README + CLAUDE.md** — `tmt`'s subcommand count to twelve; `project.rs` + `cli/driver.rs` in the turing-machine architecture bullet; `docs/tmt/project.md` in the documentation-authority paragraph.
- [ ] **Step 4: Version block** — record BOTH schema rows for the release cut: `pmt.json` schema 0.1 → **0.2** and `tmt.json` schema 0.1 → **0.2**. They are independent contracts (house precedent: the `.pma`/`.tma` dialects and `PMC_`/`TMC_LANG_VERSION` version independently) that happen to move together here, and they diverge at 0.2 — `call-mech` and the differently-shaped run block exist only on the TM side. Everything else (`.pmc`/`.tmc` languages, both dialects, IR versions, container formats) is unchanged by this plan; the crates bump at the cut, not here.
- [ ] **Step 5: Verify claims against behavior**

Run: `cargo test -p mtc-turing-machine 2>&1 | tail -3`, then cross-check each `docs/tmt/project.md` claim that has a test (discovery, defaults, call-mech resolution, run-block rules, the tape requirement, the declared source set) against Tasks 9–15's expectations.
Expected: no doc claim without a matching implemented behavior.

- [ ] **Step 6: Commit**

```bash
git add -A && git commit -m "docs(tmt): project manifest reference (schema 0.2), build CLI section, twelve-subcommand counts"
```

---

## Final verification (after all tasks)

- [ ] `cargo test --workspace` — everything green
- [ ] `cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --check`
- [ ] **PM-1 byte-identity**: `cargo test -p mtc-post-machine --test golden_programs && cargo test -p mtc-post-machine --test pm1_programs` — no codegen path was touched, and these prove it
- [ ] **`crates/core` neutrality**: `git diff --stat master -- crates/core` shows nothing (this plan needs no core change — `LinkOptions.entry` already exists)
- [ ] PM smoke: in a scratch dir, write the spec's example manifest + two sources; `pmt build`, `pmt build --list-targets`, `pmt build --run bench`; confirm outputs, marker format, exit code 2; then bare `pmt lint` and `pmt fmt --check`
- [ ] TM smoke: same shape with a `.tmt` tape — `tmt build`, `tmt build --list-targets`, `tmt build --run <target>`, `tmt build --call-mech mono <target>`; confirm the mono and frames images both run; then bare `tmt lint` and `tmt fmt --check`
- [ ] `pmt completions zsh | zsh -n /dev/stdin` and `tmt completions zsh | zsh -n /dev/stdin` — both scripts parse
- [ ] Both `cli_docs` guards green (`cargo test -p mtc-post-machine --test cli_docs && cargo test -p mtc-turing-machine --test cli_docs`)
