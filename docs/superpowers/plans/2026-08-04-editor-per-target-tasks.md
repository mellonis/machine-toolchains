# Editor Per-Target Tasks Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give both VS Code extensions per-target `build` / `build --run` tasks generated from the project manifest, bundle a validating JSON Schema for `pmt.json` / `tmt.json` with a drift guard, and bundle both extensions with esbuild.

**Architecture:** The extensions never discover a manifest themselves — they shell `<tool> build --list-targets` with `cwd` set to a workspace-folder root and let the binary's own nearest-ancestor walk answer, so no discovery logic is duplicated in TypeScript. Each crate's `project.rs` gains per-level `const` key inventories that the parse loops actually consult, making them the acceptance authority; a unit test set-compares them against the bundled schema in both directions.

**Tech Stack:** Rust (`serde_json`, no new deps), TypeScript (`vscode`, `vscode-languageclient`), esbuild, JSON Schema draft-07.

Spec: `docs/superpowers/specs/2026-08-04-editor-per-target-tasks-design.md`.

## Global Constraints

- **No new Rust dependencies.** The workspace is `serde`/`serde_json` only, `proptest` as a dev-dep. No clap.
- **Thin-renderer rule:** library code never prints. Every byte of terminal output originates in `cli/`.
- **PM-1 byte-identity is a standing regression gate.** `pm1_syntax()` never opts into `AsmCaps`.
- **`crates/core` must not change in this round.** Zero diff.
- **Both manifest schema versions stay at 0.2.** No key is added to or removed from any manifest level. In particular **no `$schema` key** is accepted by either walk.
- **Commit style:** conventional commits with scope — `feat(cli):`, `fix(core):`, `test(post-machine):`, `docs(plan):`, `polish(post-machine):`.
- **Published docs are forge-agnostic:** no issue/PR numbers, no hosting-provider URLs in `README.md`, `docs/`, or code comments. Code comments cite durable pages by page plus parenthetical keyword, e.g. `docs/pmt/project.md (discovery)`. Never cite `docs/superpowers/`.
- **No Claude attribution** in commit messages or any artifact.
- **Do NOT bump** any `MIN_TESTED_PMT` / `MIN_TESTED_TMT` floor or any plugin version. Those are release-prep work (spec §6).
- **Quality gates**, run before every commit:
  - `cargo test --workspace`
  - `cargo clippy --workspace --all-targets -- -D warnings`
  - `cargo fmt --check`

## File Structure

**Rust — key inventories and drift guards**

| File | Responsibility |
|---|---|
| `crates/post-machine/src/project.rs` | Add `const` key inventories; parse loops consult them; unit tests for the guard and bad-enum-values |
| `crates/turing-machine/src/project.rs` | Same, for `tmt.json` |

**Schemas — single source, copied into both plugin pairs**

| File | Responsibility |
|---|---|
| `editors/schemas/pmt.schema.json` | draft-07 schema for `pmt.json` |
| `editors/schemas/tmt.schema.json` | draft-07 schema for `tmt.json` |
| `editors/vscode-pm/scripts/copy-assets.js` | Replaces `copy-grammar.js`; copies grammars **and** the schema |
| `editors/vscode-tm/scripts/copy-assets.js` | Same, TM side |

**TypeScript — task providers and packaging**

| File | Responsibility |
|---|---|
| `editors/vscode-pm/src/extension.ts` | Per-target tasks, watcher-invalidated cache, `resolveTask` cwd derivation |
| `editors/vscode-tm/src/extension.ts` | Same, TM side |
| `editors/vscode-{pm,tm}/package.json` | `taskDefinitions`, `jsonValidation`, esbuild scripts |
| `editors/vscode-{pm,tm}/esbuild.js` | Bundle entry |
| `editors/vscode-{pm,tm}/.vscodeignore` | Exclude `node_modules`, sources, build scripts |

**Docs**

| File | Responsibility |
|---|---|
| `editors/vscode-{pm,tm}/README.md` | Per-target tasks; pipeline snippet demoted |
| `editors/jetbrains-{pm,tm}/README.md` | Run-configuration recipe; JSON schema mapping recipe |
| `docs/pmt/project.md`, `docs/tmt/project.md` | Editor-integration note |
| `crates/{post-machine,turing-machine}/tests/build_driver.rs` | Doc-comment note that the task provider parses `--list-targets` |

---

### Task 1: PM-1 key inventories

Make `pmt.json`'s accepted keys enumerable *and* load-bearing, so a later schema can be set-compared against them and cannot drift.

**Files:**
- Modify: `crates/post-machine/src/project.rs`

**Interfaces:**
- Consumes: nothing (first task).
- Produces: module-private consts in `crates/post-machine/src/project.rs`, visible to that file's `#[cfg(test)] mod tests`:
  - `MANIFEST_KEYS: &[&str]`
  - `LIBRARIES_KEYS: &[&str]`
  - `PROFILES_KEYS: &[&str]`
  - `PROFILE_KEYS: &[&str]`
  - `TARGET_KEYS: &[&str]`
  - `RUN_KEYS: &[&str]`
  - `OPT_VALUES: &[&str]`

- [ ] **Step 1: Write the failing tests**

Add to the existing `#[cfg(test)] mod tests` block at the bottom of `crates/post-machine/src/project.rs` (it starts at roughly line 591):

```rust
    /// Every key each inventory lists must be ACCEPTED by the walk. This
    /// is the half that catches an inventory naming a key the parser
    /// never had.
    #[test]
    fn every_inventoried_key_is_accepted() {
        // (level JSON pointer builder, the level's inventory, a valid
        // value for each key at that level)
        let cases: &[(&str, &[&str])] = &[
            ("manifest", MANIFEST_KEYS),
            ("libraries", LIBRARIES_KEYS),
            ("profiles", PROFILES_KEYS),
            ("profile", PROFILE_KEYS),
            ("target", TARGET_KEYS),
            ("run", RUN_KEYS),
        ];
        for (level, keys) in cases {
            assert!(!keys.is_empty(), "{level} inventory is empty");
            let mut sorted = keys.to_vec();
            sorted.sort_unstable();
            assert_eq!(
                &sorted[..],
                *keys,
                "{level} inventory must be sorted so the schema set-compare reads cleanly"
            );
            let mut deduped = sorted.clone();
            deduped.dedup();
            assert_eq!(deduped.len(), keys.len(), "{level} inventory has a duplicate");
        }
    }

    /// A key at a level's inventory boundary: `stdlib` belongs to the
    /// manifest level and must NOT be accepted inside `run`.
    #[test]
    fn inventories_do_not_overlap_across_levels() {
        assert!(MANIFEST_KEYS.contains(&"stdlib"));
        assert!(!RUN_KEYS.contains(&"stdlib"));
        assert!(RUN_KEYS.contains(&"tape-block"));
        assert!(!TARGET_KEYS.contains(&"tape-block"));
    }

    /// A recognized key holding a BAD VALUE keeps its own diagnostic —
    /// the membership pre-check gates keys only, never values. The
    /// existing UnknownKey tests exercise keys and cannot catch this.
    #[test]
    fn a_bad_opt_value_is_not_reported_as_an_unknown_key() {
        // `unique_tmp_dir` already exists in this test module — it is the
        // crate's no-tempfile-dependency scratch helper (pid + atomic
        // counter, collision-free under a parallel test run).
        let dir = unique_tmp_dir("bad-opt");
        let path = dir.join("pmt.json");
        std::fs::write(
            &path,
            r#"{ "project": { "profiles": { "debug": { "opt": "O2" } } } }"#,
        )
        .unwrap();
        let err = load_file(&path).expect_err("`O2` is not an opt level");
        assert!(
            !matches!(err, crate::config::ConfigError::UnknownKey { .. }),
            "a bad VALUE must not be reported as an unknown KEY: {err:?}"
        );
        let rendered = format!("{err:?}");
        assert!(
            rendered.contains("unknown opt level"),
            "the opt-level diagnostic must survive: {rendered}"
        );
        std::fs::remove_dir_all(&dir).ok();
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p mtc-post-machine project::tests -- --nocapture`

Expected: FAIL to compile — `cannot find value MANIFEST_KEYS in this scope` (and the other five inventories).

- [ ] **Step 3: Add the inventories**

Insert immediately above `fn parse_libraries` in `crates/post-machine/src/project.rs`:

```rust
// ---------------------------------------------------------------------------
// Accepted-key inventories.
// ---------------------------------------------------------------------------

/// The keys each level of a `pmt.json` `project` section accepts
/// (docs/pmt/project.md (schema)).
///
/// These lists are LOAD-BEARING, not documentation: every parse loop
/// below checks membership here before dispatching, so an inventory is
/// the acceptance authority and cannot drift from behaviour. The bundled
/// editor JSON Schema is set-compared against them in this file's tests.
///
/// Each list is sorted; the tests assert that, because the schema
/// set-compare reads far more clearly against a sorted inventory.
const MANIFEST_KEYS: &[&str] = &["libraries", "profiles", "sources", "stdlib", "targets"];
const LIBRARIES_KEYS: &[&str] = &["dirs", "link"];
const PROFILES_KEYS: &[&str] = &["debug", "release"];
const PROFILE_KEYS: &[&str] = &["debug-info", "opt", "strip-debugger", "werror"];
const TARGET_KEYS: &[&str] = &["entry", "libraries", "output", "run", "sources"];
const RUN_KEYS: &[&str] = &[
    "head",
    "max-steps",
    "max-tacts",
    "strict-cells",
    "tact-profile",
    "tape",
    "tape-block",
];

/// The `opt` key's accepted spellings, in the order the error message
/// lists them.
const OPT_VALUES: &[&str] = &["O0", "O1"];
```

- [ ] **Step 4: Run the tests to verify the first two pass**

Run: `cargo test -p mtc-post-machine project::tests -- --nocapture`

Expected: `every_inventoried_key_is_accepted` and `inventories_do_not_overlap_across_levels` PASS. `a_bad_opt_value_is_not_reported_as_an_unknown_key` PASS too — the pre-check does not exist yet, so nothing has broken it. That is fine: it is a regression guard for Step 5, not a driver.

- [ ] **Step 5: Make the inventories load-bearing**

In each of the five parse loops, replace the `other => return Err(unknown_key(path, other))` arm with a membership pre-check plus an `unreachable!`. The point is that the const, not the match, decides acceptance.

`parse_libraries`:

```rust
    for (key, val) in obj {
        if !LIBRARIES_KEYS.contains(&key.as_str()) {
            return Err(unknown_key(path, key));
        }
        match key.as_str() {
            "dirs" => libs.dirs = as_str_array(path, val, "libraries.dirs")?,
            "link" => libs.link = as_str_array(path, val, "libraries.link")?,
            _ => unreachable!("LIBRARIES_KEYS gates this match"),
        }
    }
```

`parse_profile` — note the `opt` arm keeps its OWN error for a bad value; only the key check moves out:

```rust
    for (key, val) in obj {
        if !PROFILE_KEYS.contains(&key.as_str()) {
            return Err(unknown_key(path, key));
        }
        match key.as_str() {
            "opt" => {
                over.opt = Some(match as_str(path, val, "opt")?.as_str() {
                    "O0" => OptLevel::O0,
                    "O1" => OptLevel::O1,
                    other => {
                        return Err(invalid(
                            path,
                            format!("unknown opt level `{other}` ({})", OPT_VALUES.join(" | ")),
                        ));
                    }
                });
            }
            "debug-info" => over.debug_info = Some(as_bool(path, val, "debug-info")?),
            "strip-debugger" => over.strip_debugger = Some(as_bool(path, val, "strip-debugger")?),
            "werror" => over.werror = Some(as_bool(path, val, "werror")?),
            _ => unreachable!("PROFILE_KEYS gates this match"),
        }
    }
```

`parse_run`:

```rust
    for (key, val) in obj {
        if !RUN_KEYS.contains(&key.as_str()) {
            return Err(unknown_key(path, key));
        }
        match key.as_str() {
            // ... existing arms unchanged ...
            _ => unreachable!("RUN_KEYS gates this match"),
        }
    }
```

`parse_target` gets the same treatment with `TARGET_KEYS`, and `validate_manifest`'s top-level loop with `MANIFEST_KEYS`. The nested `profiles` loop inside `validate_manifest` — the one matching `"debug"` / `"release"` — gets `PROFILES_KEYS`.

Leave `load_file`'s `"lint"` / `"project"` loop alone: those are file-level keys, not part of the `project` section the schema describes, and widening the inventory to cover them would make the set-compare wrong.

- [ ] **Step 6: Verify the error message is unchanged**

The `opt` diagnostic was `unknown opt level \`{other}\` (O0 | O1)`; `OPT_VALUES.join(" | ")` reproduces `O0 | O1` exactly. Confirm no test asserted the old literal:

Run: `rg 'unknown opt level' crates/`

Expected: only the source line and, if one exists, a test asserting the same rendered text.

- [ ] **Step 7: Run the full gates**

Run:
```bash
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --check
```
Expected: all pass. The existing `ConfigError::UnknownKey` unit tests in `project.rs` prove key acceptance is unchanged.

- [ ] **Step 8: Commit**

```bash
git add crates/post-machine/src/project.rs
git commit -m "refactor(post-machine): make pmt.json accepted keys a load-bearing inventory

Each level's accepted keys move from bare match arms into a sorted const
list the parse loop consults before dispatching, so the list is the
acceptance authority rather than a second description of it. The bundled
editor JSON Schema is set-compared against these lists.

The pre-check gates keys only: a recognized key holding a bad value keeps
its own diagnostic, now covered by a test the existing unknown-key tests
could not catch."
```

---

### Task 2: TM-1 key inventories

Same as Task 1 for `tmt.json`, whose manifest and run levels genuinely differ.

**Files:**
- Modify: `crates/turing-machine/src/project.rs`

**Interfaces:**
- Consumes: nothing from Task 1 — the crates are independent.
- Produces: module-private consts in `crates/turing-machine/src/project.rs`:
  - `MANIFEST_KEYS`, `LIBRARIES_KEYS`, `PROFILES_KEYS`, `PROFILE_KEYS`, `TARGET_KEYS`, `RUN_KEYS`, `OPT_VALUES`, `CALL_MECH_VALUES` — all `&[&str]`

- [ ] **Step 1: Write the failing tests**

Add to the existing `#[cfg(test)] mod tests` block at the bottom of `crates/turing-machine/src/project.rs` (starts at roughly line 633):

```rust
    /// Every inventory must be non-empty, sorted, and duplicate-free —
    /// the schema set-compare reads against sorted lists.
    #[test]
    fn every_inventory_is_sorted_and_unique() {
        let cases: &[(&str, &[&str])] = &[
            ("manifest", MANIFEST_KEYS),
            ("libraries", LIBRARIES_KEYS),
            ("profiles", PROFILES_KEYS),
            ("profile", PROFILE_KEYS),
            ("target", TARGET_KEYS),
            ("run", RUN_KEYS),
        ];
        for (level, keys) in cases {
            assert!(!keys.is_empty(), "{level} inventory is empty");
            let mut sorted = keys.to_vec();
            sorted.sort_unstable();
            assert_eq!(&sorted[..], *keys, "{level} inventory must be sorted");
            let mut deduped = sorted.clone();
            deduped.dedup();
            assert_eq!(deduped.len(), keys.len(), "{level} inventory has a duplicate");
        }
    }

    /// Every key an inventory lists must be REACHABLE in its parse loop.
    ///
    /// The membership pre-check means an inventory entry with no match arm
    /// panics on `unreachable!` instead of failing gracefully. This walks
    /// every inventoried key through a minimal document that places it at
    /// its own level and asserts the walk does not reject it as an unknown
    /// key. A key that also violates a cross-key rule fails with a
    /// DIFFERENT error, which is the point: this asserts acceptance of the
    /// KEY, not validity of the document.
    ///
    /// What it cannot check — matching this crate's other registry guards
    /// — is a key the walk gained a match arm for but which was never
    /// added to its inventory. Rust cannot enumerate match arms; the
    /// pre-check makes such an arm dead, so any test exercising that key
    /// fails instead.
    #[test]
    fn every_inventoried_key_is_reachable_in_its_parse_loop() {
        // A right-TYPED value per key. Document validity is not the point.
        let value_for = |key: &str| -> serde_json::Value {
            match key {
                "sources" => json!([]),
                "libraries" => json!({}),
                "stdlib" => json!(true),
                "profiles" => json!({}),
                "targets" => json!({}),
                "call-mech" => json!("mono"),
                "dirs" | "link" => json!([]),
                "debug" | "release" => json!({}),
                "opt" => json!("O0"),
                "debug-info" | "strip-debugger" | "werror" => json!(true),
                "entry" => json!("main"),
                "output" => json!("app.tmx"),
                "run" => json!({}),
                "tape" => json!("start.tmt"),
                "max-steps" | "max-tacts" => json!(1),
                "no-step-limit" => json!(true),
                other => panic!("no test value for inventoried key `{other}`"),
            }
        };

        // Places a one-key object at each level's position in the tree.
        // `v` takes the `project` section itself, so the manifest level is
        // the bare leaf.
        let at_level = |level: &str, key: &str, value: serde_json::Value| -> serde_json::Value {
            let leaf = json!({ key: value });
            match level {
                "manifest" => leaf,
                "libraries" => json!({ "libraries": leaf }),
                "profiles" => json!({ "profiles": leaf }),
                "profile" => json!({ "profiles": { "debug": leaf } }),
                "target" => json!({ "targets": { "app": leaf } }),
                "run" => json!({ "targets": { "app": { "run": leaf } } }),
                other => panic!("unknown level `{other}`"),
            }
        };

        for (level, keys) in [
            ("manifest", MANIFEST_KEYS),
            ("libraries", LIBRARIES_KEYS),
            ("profiles", PROFILES_KEYS),
            ("profile", PROFILE_KEYS),
            ("target", TARGET_KEYS),
            ("run", RUN_KEYS),
        ] {
            for key in keys {
                let doc = at_level(level, key, value_for(key));
                if let Err(err) = v(doc.clone()) {
                    assert!(
                        !matches!(err, crate::config::ConfigError::UnknownKey { .. }),
                        "`{key}` is in the {level} inventory but the walk rejects it \
                         as an unknown key: {doc}"
                    );
                }
            }
        }
    }

    /// TM-1's manifest and run levels differ from PM-1's by contract, not
    /// by accident (docs/tmt/project.md (schema)): `call-mech` exists at
    /// the manifest and target levels, and the run block is `.tmt`-tape
    /// only with a `no-step-limit` switch and none of PM-1's cell knobs.
    #[test]
    fn tm_specific_keys_are_where_the_contract_puts_them() {
        assert!(MANIFEST_KEYS.contains(&"call-mech"));
        assert!(TARGET_KEYS.contains(&"call-mech"));
        assert!(RUN_KEYS.contains(&"no-step-limit"));
        assert!(!RUN_KEYS.contains(&"tape-block"));
        assert!(!RUN_KEYS.contains(&"head"));
        assert!(!RUN_KEYS.contains(&"strict-cells"));
        assert!(!RUN_KEYS.contains(&"tact-profile"));
    }

    /// A recognized key holding a bad value keeps its own diagnostic.
    /// Covers BOTH of TM-1's enums, since each has its own message.
    #[test]
    fn bad_enum_values_are_not_reported_as_unknown_keys() {
        for (label, body, needle) in [
            (
                "bad-opt",
                r#"{ "project": { "profiles": { "debug": { "opt": "O2" } } } }"#,
                "unknown opt level",
            ),
            (
                "bad-call-mech",
                r#"{ "project": { "call-mech": "nope" } }"#,
                "unknown call-mech",
            ),
        ] {
            // `unique_tmp_dir` already exists in this test module — the
            // crate's no-tempfile-dependency scratch helper (pid + atomic
            // counter, collision-free under a parallel test run).
            let dir = unique_tmp_dir(label);
            let path = dir.join("tmt.json");
            std::fs::write(&path, body).unwrap();
            let err = load_file(&path).expect_err("the value is invalid");
            assert!(
                !matches!(err, crate::config::ConfigError::UnknownKey { .. }),
                "a bad VALUE must not be reported as an unknown KEY: {err:?}"
            );
            let rendered = format!("{err:?}");
            assert!(
                rendered.contains(needle),
                "the `{needle}` diagnostic must survive: {rendered}"
            );
            std::fs::remove_dir_all(&dir).ok();
        }
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p mtc-turing-machine project::tests -- --nocapture`

Expected: FAIL to compile — `cannot find value MANIFEST_KEYS in this scope`.

- [ ] **Step 3: Add the inventories**

Insert immediately above `fn parse_libraries` in `crates/turing-machine/src/project.rs`:

```rust
// ---------------------------------------------------------------------------
// Accepted-key inventories.
// ---------------------------------------------------------------------------

/// The keys each level of a `tmt.json` `project` section accepts
/// (docs/tmt/project.md (schema)).
///
/// These lists are LOAD-BEARING, not documentation: every parse loop
/// below checks membership here before dispatching, so an inventory is
/// the acceptance authority and cannot drift from behaviour. The bundled
/// editor JSON Schema is set-compared against them in this file's tests.
///
/// TM-1's inventory is NOT PM-1's: `call-mech` exists at the manifest and
/// target levels, and the run block drives a band from a `.tmt` snapshot,
/// so it has `no-step-limit` and none of PM-1's per-cell knobs.
const MANIFEST_KEYS: &[&str] = &[
    "call-mech",
    "libraries",
    "profiles",
    "sources",
    "stdlib",
    "targets",
];
const LIBRARIES_KEYS: &[&str] = &["dirs", "link"];
const PROFILES_KEYS: &[&str] = &["debug", "release"];
const PROFILE_KEYS: &[&str] = &["debug-info", "opt", "strip-debugger", "werror"];
const TARGET_KEYS: &[&str] = &[
    "call-mech",
    "entry",
    "libraries",
    "output",
    "run",
    "sources",
];
const RUN_KEYS: &[&str] = &["max-steps", "max-tacts", "no-step-limit", "tape"];

/// The `opt` key's accepted spellings, in the order the error lists them.
const OPT_VALUES: &[&str] = &["O0", "O1"];

/// The `call-mech` key's accepted spellings — the same three
/// `tmt link --call-mech` accepts, in the order the error lists them.
const CALL_MECH_VALUES: &[&str] = &["mono", "frames", "hybrid"];
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p mtc-turing-machine project::tests -- --nocapture`

Expected: PASS.

- [ ] **Step 5: Make the inventories load-bearing**

Apply the same pre-check-plus-`unreachable!` transformation as Task 1 Step 5 to `parse_libraries` (`LIBRARIES_KEYS`), `parse_profile` (`PROFILE_KEYS`), `parse_run` (`RUN_KEYS`), `parse_target` (`TARGET_KEYS`), `validate_manifest`'s top-level loop (`MANIFEST_KEYS`), and its nested `profiles` loop (`PROFILES_KEYS`).

Additionally, route both enum messages through their consts so the schema's `enum` array and the CLI's error can never disagree:

```rust
fn parse_call_mech_value(path: &Path, value: &Value) -> Result<CallMech, ConfigError> {
    match as_str(path, value, "call-mech")?.as_str() {
        "mono" => Ok(CallMech::Mono),
        "frames" => Ok(CallMech::Frames),
        "hybrid" => Ok(CallMech::Hybrid),
        other => Err(invalid(
            path,
            format!(
                "unknown call-mech `{other}` (expected one of: {})",
                CALL_MECH_VALUES.join(", ")
            ),
        )),
    }
}
```

and in `parse_profile`, `format!("unknown opt level \`{other}\` ({})", OPT_VALUES.join(" | "))`.

Leave `load_file`'s `"lint"` / `"project"` loop alone — file-level keys, outside the schema's `project` section.

- [ ] **Step 6: Verify both messages render identically to before**

The originals were `unknown opt level \`{other}\` (O0 | O1)` and `unknown call-mech \`{other}\` (expected one of: mono, frames, hybrid)`. `OPT_VALUES.join(" | ")` gives `O0 | O1`; `CALL_MECH_VALUES.join(", ")` gives `mono, frames, hybrid`.

Run: `rg 'unknown call-mech|unknown opt level' crates/`

Expected: source lines plus any test asserting the same rendered text; no mismatch.

- [ ] **Step 7: Run the full gates**

Run:
```bash
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --check
```
Expected: all pass.

- [ ] **Step 8: Commit**

```bash
git add crates/turing-machine/src/project.rs
git commit -m "refactor(turing-machine): make tmt.json accepted keys a load-bearing inventory

Mirrors the PM-1 change: each level's accepted keys move into a sorted
const list the parse loop consults before dispatching. Both enum
diagnostics now render from the same consts the editor schema will be
set-compared against, so the CLI message and the schema cannot disagree.

Records in tests that TM-1's manifest and run levels differ from PM-1's
by contract — call-mech at two levels, a tape-only run block with
no-step-limit and none of PM-1's per-cell knobs."
```

---

### Task 3: PM-1 schema, drift guard, and plugin wiring

**Files:**
- Create: `editors/schemas/pmt.schema.json`
- Create: `editors/vscode-pm/scripts/copy-assets.js`
- Delete: `editors/vscode-pm/scripts/copy-grammar.js`
- Modify: `crates/post-machine/src/project.rs` (tests only)
- Modify: `editors/vscode-pm/package.json`

**Interfaces:**
- Consumes: `MANIFEST_KEYS`, `LIBRARIES_KEYS`, `PROFILES_KEYS`, `PROFILE_KEYS`, `TARGET_KEYS`, `RUN_KEYS`, `OPT_VALUES` from Task 1.
- Produces: `editors/schemas/pmt.schema.json`, whose `properties` key sets per level equal those inventories; and `npm run copy-assets` in `editors/vscode-pm`.

- [ ] **Step 1: Write the failing drift guard**

Add to `crates/post-machine/src/project.rs`'s `#[cfg(test)] mod tests`:

```rust
    /// The bundled editor JSON Schema must describe EXACTLY the keys the
    /// walk accepts, per level, in both directions: a key the walk gained
    /// without a schema entry fails, and a schema entry naming a key the
    /// walk does not accept fails too.
    ///
    /// The schema is an editing affordance layered on the walk, never the
    /// authority — but a stale affordance is worse than none, so this is
    /// a hard gate (docs/pmt/project.md (schema)).
    #[test]
    fn the_bundled_schema_matches_the_key_inventories() {
        use std::collections::BTreeSet;

        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../editors/schemas/pmt.schema.json"
        );
        let text = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{path}: {e}"));
        let schema: serde_json::Value =
            serde_json::from_str(&text).unwrap_or_else(|e| panic!("{path} is valid JSON: {e}"));

        assert_eq!(
            schema["$schema"], "http://json-schema.org/draft-07/schema#",
            "the schema declares draft-07; `dependencies` (the head-requires-tape leg) is a draft-07 keyword"
        );

        // (a human name, the schema node holding this level's
        //  `properties`, the crate's inventory for that level)
        let levels: &[(&str, &serde_json::Value, &[&str])] = &[
            ("manifest", &schema["properties"]["project"], MANIFEST_KEYS),
            (
                "libraries",
                &schema["definitions"]["libraries"],
                LIBRARIES_KEYS,
            ),
            ("profiles", &schema["definitions"]["profiles"], PROFILES_KEYS),
            ("profile", &schema["definitions"]["profile"], PROFILE_KEYS),
            ("target", &schema["definitions"]["target"], TARGET_KEYS),
            ("run", &schema["definitions"]["run"], RUN_KEYS),
        ];

        for (name, node, inventory) in levels {
            let props = node["properties"]
                .as_object()
                .unwrap_or_else(|| panic!("schema level `{name}` has a properties object"));
            let in_schema: BTreeSet<&str> = props.keys().map(String::as_str).collect();
            let in_walk: BTreeSet<&str> = inventory.iter().copied().collect();
            assert_eq!(
                in_schema, in_walk,
                "schema level `{name}` disagrees with the walk's inventory"
            );
            assert_eq!(
                node["additionalProperties"], false,
                "schema level `{name}` must reject unknown keys, like the walk does"
            );
        }

        // Enum values come from the same const the CLI error renders from.
        let opt_enum: Vec<&str> = schema["definitions"]["profile"]["properties"]["opt"]["enum"]
            .as_array()
            .expect("opt has an enum")
            .iter()
            .map(|v| v.as_str().expect("opt enum entries are strings"))
            .collect();
        assert_eq!(opt_enum, OPT_VALUES, "the opt enum must match the walk's");
    }

    /// The two cross-key run rules are DIFFERENT SHAPES and the schema
    /// must not conflate them: `tape` vs `tape-block` is a mutual
    /// exclusion, while `head` requires `tape` is an implication — an
    /// exclusivity construct there would encode the opposite rule.
    #[test]
    fn the_schema_encodes_both_run_rule_shapes() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../editors/schemas/pmt.schema.json"
        );
        let schema: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        let run = &schema["definitions"]["run"];

        let excluded = run["not"]["required"]
            .as_array()
            .expect("run states the tape/tape-block exclusion via `not: { required: [...] }`");
        let excluded: Vec<&str> = excluded.iter().map(|v| v.as_str().unwrap()).collect();
        assert_eq!(excluded, vec!["tape", "tape-block"]);

        let dependency = run["dependencies"]["head"]
            .as_array()
            .expect("run states `head` requires `tape` via draft-07 `dependencies`");
        let dependency: Vec<&str> = dependency.iter().map(|v| v.as_str().unwrap()).collect();
        assert_eq!(dependency, vec!["tape"]);
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p mtc-post-machine project::tests::the_bundled_schema -- --nocapture`

Expected: FAIL — the schema file does not exist yet, panicking with the path and `No such file or directory`.

- [ ] **Step 3: Write the schema**

Create `editors/schemas/pmt.schema.json`:

```json
{
  "$schema": "http://json-schema.org/draft-07/schema#",
  "title": "pmt.json",
  "description": "Project configuration for the PM-1 toolchain. The runtime walk in the pmt binary remains authoritative; this schema is an editing affordance and cannot express path normalization or cross-target comparison.",
  "type": "object",
  "properties": {
    "lint": {
      "type": "object",
      "description": "Lint configuration (schema 0.1; predates the project section).",
      "properties": {
        "allow": {
          "type": "array",
          "items": { "type": "string" },
          "description": "Lint codes to allow. Unioned with editor settings, never a cascade."
        }
      },
      "additionalProperties": false
    },
    "project": {
      "type": "object",
      "description": "Project manifest, schema 0.2.",
      "properties": {
        "stdlib": {
          "type": "boolean",
          "description": "Link the embedded standard library. Default true."
        },
        "sources": {
          "type": "array",
          "items": { "type": "string" },
          "description": "Sources shared by every target, as manifest-relative paths."
        },
        "libraries": { "$ref": "#/definitions/libraries" },
        "profiles": { "$ref": "#/definitions/profiles" },
        "targets": {
          "type": "object",
          "description": "Named build targets.",
          "additionalProperties": { "$ref": "#/definitions/target" }
        }
      },
      "additionalProperties": false
    }
  },
  "additionalProperties": false,
  "definitions": {
    "libraries": {
      "type": "object",
      "properties": {
        "dirs": {
          "type": "array",
          "items": { "type": "string" },
          "description": "Library search directories."
        },
        "link": {
          "type": "array",
          "items": { "type": "string" },
          "description": "Library names to link."
        }
      },
      "additionalProperties": false
    },
    "profiles": {
      "type": "object",
      "properties": {
        "debug": { "$ref": "#/definitions/profile" },
        "release": { "$ref": "#/definitions/profile" }
      },
      "additionalProperties": false
    },
    "profile": {
      "type": "object",
      "properties": {
        "opt": {
          "type": "string",
          "enum": ["O0", "O1"],
          "description": "Optimization level."
        },
        "debug-info": {
          "type": "boolean",
          "description": "Emit the .pmx.map debug sidecar."
        },
        "strip-debugger": {
          "type": "boolean",
          "description": "Strip debugger breakpoints."
        },
        "werror": {
          "type": "boolean",
          "description": "Treat warnings as errors."
        }
      },
      "additionalProperties": false
    },
    "target": {
      "type": "object",
      "properties": {
        "sources": {
          "type": "array",
          "items": { "type": "string" },
          "description": "Sources for this target, appended to the project-level set."
        },
        "libraries": { "$ref": "#/definitions/libraries" },
        "entry": {
          "type": "string",
          "description": "Entry function name."
        },
        "output": {
          "type": "string",
          "description": "Output path, manifest-relative."
        },
        "run": { "$ref": "#/definitions/run" }
      },
      "additionalProperties": false
    },
    "run": {
      "type": "object",
      "description": "Settings for `pmt build --run <target>`. A target without this block cannot be run.",
      "properties": {
        "tape": {
          "type": "string",
          "description": "Initial tape as a literal cell string. Mutually exclusive with tape-block."
        },
        "tape-block": {
          "type": "string",
          "description": "Path to a .pmt tape snapshot. Mutually exclusive with tape."
        },
        "head": {
          "type": "integer",
          "description": "Initial head position. Only meaningful alongside tape."
        },
        "strict-cells": {
          "type": "boolean",
          "description": "Fault on writing a cell's existing value."
        },
        "max-steps": {
          "type": "integer",
          "minimum": 0,
          "description": "Step limit."
        },
        "max-tacts": {
          "type": "integer",
          "minimum": 0,
          "description": "Tact limit."
        },
        "tact-profile": {
          "type": "array",
          "items": { "type": "integer", "minimum": 0 },
          "minItems": 3,
          "maxItems": 3,
          "description": "Device stall costs as [move, read, write]."
        }
      },
      "additionalProperties": false,
      "not": { "required": ["tape", "tape-block"] },
      "dependencies": { "head": ["tape"] }
    }
  }
}
```

- [ ] **Step 4: Run to verify the guard passes**

Run: `cargo test -p mtc-post-machine project::tests -- --nocapture`

Expected: PASS, including both new tests.

- [ ] **Step 5: Replace the copy script**

Create `editors/vscode-pm/scripts/copy-assets.js`:

```js
// Copies the single-source editor assets into this extension's package
// directory: the shared TextMate grammars and the shared JSON Schema.
// Both live one level up so the -pm and -tm pairs consume one copy.
const fs = require('fs'), path = require('path');
const editorsDir = path.join(__dirname, '..', '..');

const grammarsSrc = path.join(editorsDir, 'grammars');
const grammarsDst = path.join(__dirname, '..', 'syntaxes');
fs.mkdirSync(grammarsDst, { recursive: true });
for (const name of ['pmc.tmLanguage.json', 'pma.tmLanguage.json']) {
  fs.copyFileSync(path.join(grammarsSrc, name), path.join(grammarsDst, name));
}

const schemasSrc = path.join(editorsDir, 'schemas');
const schemasDst = path.join(__dirname, '..', 'schemas');
fs.mkdirSync(schemasDst, { recursive: true });
fs.copyFileSync(
  path.join(schemasSrc, 'pmt.schema.json'),
  path.join(schemasDst, 'pmt.schema.json'),
);
```

Delete the old script:

```bash
rm editors/vscode-pm/scripts/copy-grammar.js
```

- [ ] **Step 6: Wire the schema and the new script into the manifest**

In `editors/vscode-pm/package.json`, add to `contributes` (after `problemMatchers`):

```json
    "jsonValidation": [{
      "fileMatch": "pmt.json",
      "url": "./schemas/pmt.schema.json"
    }],
```

and replace the `scripts` block:

```json
  "scripts": {
    "copy-assets": "node scripts/copy-assets.js",
    "compile": "npm run copy-assets && tsc -p .",
    "package": "npm run compile && vsce package"
  },
```

- [ ] **Step 7: Verify the extension still builds and the schema lands**

Run:
```bash
cd editors/vscode-pm && npm install && npm run compile
ls schemas/pmt.schema.json syntaxes/
```
Expected: compile succeeds; `schemas/pmt.schema.json` and both `syntaxes/*.tmLanguage.json` exist.

- [ ] **Step 8: Gitignore the copied schema, as the copied grammars already are**

`editors/vscode-pm/.gitignore` currently reads:

```
node_modules/
out/
syntaxes/
*.vsix
```

`syntaxes/` is ignored because it is a copy of the single source in
`editors/grammars/`. The copied schema is the same kind of artifact, so
add it in the same place:

```
node_modules/
out/
syntaxes/
schemas/
*.vsix
```

Run: `git check-ignore -v editors/vscode-pm/schemas/pmt.schema.json`

Expected: the rule matches, i.e. the copy is ignored. The single source
`editors/schemas/pmt.schema.json` is outside the extension directory and
is committed.

- [ ] **Step 9: Run the full gates**

Run:
```bash
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --check
```
Expected: all pass.

- [ ] **Step 10: Commit**

```bash
git add editors/schemas/pmt.schema.json editors/vscode-pm crates/post-machine/src/project.rs
git rm --cached editors/vscode-pm/scripts/copy-grammar.js 2>/dev/null || true
git commit -m "feat(editors): bundle a validating JSON Schema for pmt.json

Adds a draft-07 schema describing every key the manifest walk accepts,
contributed to VS Code via jsonValidation so a hand-edited pmt.json gets
key completion, hover text, and inline errors.

A unit test set-compares the schema's per-level property sets against the
walk's key inventories in both directions, and pins the two cross-key run
rules to their correct and DIFFERENT shapes: tape versus tape-block is a
mutual exclusion, while head requires tape is an implication.

The walk stays authoritative — the schema cannot express path
normalization or cross-target output collision, and the CLI must reject a
bad manifest whether or not an editor was in the loop.

The grammar copy step generalizes to copy-assets, which now also stages
the shared schema."
```

---

### Task 4: TM-1 schema, drift guard, and plugin wiring

**Files:**
- Create: `editors/schemas/tmt.schema.json`
- Create: `editors/vscode-tm/scripts/copy-assets.js`
- Delete: `editors/vscode-tm/scripts/copy-grammar.js`
- Modify: `crates/turing-machine/src/project.rs` (tests only)
- Modify: `editors/vscode-tm/package.json`

**Interfaces:**
- Consumes: `MANIFEST_KEYS`, `LIBRARIES_KEYS`, `PROFILES_KEYS`, `PROFILE_KEYS`, `TARGET_KEYS`, `RUN_KEYS`, `OPT_VALUES`, `CALL_MECH_VALUES` from Task 2.
- Produces: `editors/schemas/tmt.schema.json`; `npm run copy-assets` in `editors/vscode-tm`.

- [ ] **Step 1: Write the failing drift guard**

Add to `crates/turing-machine/src/project.rs`'s `#[cfg(test)] mod tests`:

```rust
    /// The bundled editor JSON Schema must describe EXACTLY the keys the
    /// walk accepts, per level, in both directions
    /// (docs/tmt/project.md (schema)).
    #[test]
    fn the_bundled_schema_matches_the_key_inventories() {
        use std::collections::BTreeSet;

        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../editors/schemas/tmt.schema.json"
        );
        let text = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{path}: {e}"));
        let schema: serde_json::Value =
            serde_json::from_str(&text).unwrap_or_else(|e| panic!("{path} is valid JSON: {e}"));

        assert_eq!(
            schema["$schema"], "http://json-schema.org/draft-07/schema#",
            "the schema declares draft-07"
        );

        let levels: &[(&str, &serde_json::Value, &[&str])] = &[
            ("manifest", &schema["properties"]["project"], MANIFEST_KEYS),
            (
                "libraries",
                &schema["definitions"]["libraries"],
                LIBRARIES_KEYS,
            ),
            ("profiles", &schema["definitions"]["profiles"], PROFILES_KEYS),
            ("profile", &schema["definitions"]["profile"], PROFILE_KEYS),
            ("target", &schema["definitions"]["target"], TARGET_KEYS),
            ("run", &schema["definitions"]["run"], RUN_KEYS),
        ];

        for (name, node, inventory) in levels {
            let props = node["properties"]
                .as_object()
                .unwrap_or_else(|| panic!("schema level `{name}` has a properties object"));
            let in_schema: BTreeSet<&str> = props.keys().map(String::as_str).collect();
            let in_walk: BTreeSet<&str> = inventory.iter().copied().collect();
            assert_eq!(
                in_schema, in_walk,
                "schema level `{name}` disagrees with the walk's inventory"
            );
            assert_eq!(
                node["additionalProperties"], false,
                "schema level `{name}` must reject unknown keys, like the walk does"
            );
        }

        let opt_enum: Vec<&str> = schema["definitions"]["profile"]["properties"]["opt"]["enum"]
            .as_array()
            .expect("opt has an enum")
            .iter()
            .map(|v| v.as_str().expect("opt enum entries are strings"))
            .collect();
        assert_eq!(opt_enum, OPT_VALUES);

        // `call-mech` appears at TWO levels and both must carry the same
        // enum as the CLI's own error.
        for pointer in [
            &schema["properties"]["project"]["properties"]["call-mech"],
            &schema["definitions"]["target"]["properties"]["call-mech"],
        ] {
            let values: Vec<&str> = pointer["enum"]
                .as_array()
                .expect("call-mech has an enum")
                .iter()
                .map(|v| v.as_str().expect("call-mech enum entries are strings"))
                .collect();
            assert_eq!(values, CALL_MECH_VALUES);
        }
    }

    /// TM-1's only cross-key run rule is a mutual exclusion. It has no
    /// implication leg — PM-1's `head requires tape` has no TM-1 analogue,
    /// because the TM run block drives a band from a `.tmt` snapshot.
    #[test]
    fn the_schema_encodes_the_step_limit_exclusion() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../editors/schemas/tmt.schema.json"
        );
        let schema: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        let run = &schema["definitions"]["run"];

        let excluded = run["not"]["required"]
            .as_array()
            .expect("run states the max-steps/no-step-limit exclusion");
        let excluded: Vec<&str> = excluded.iter().map(|v| v.as_str().unwrap()).collect();
        assert_eq!(excluded, vec!["max-steps", "no-step-limit"]);

        assert!(
            run["dependencies"].is_null(),
            "TM-1's run block has no implication rule; adding one silently would misdescribe it"
        );
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p mtc-turing-machine project::tests::the_bundled_schema -- --nocapture`

Expected: FAIL — schema file missing.

- [ ] **Step 3: Write the schema**

Create `editors/schemas/tmt.schema.json`:

```json
{
  "$schema": "http://json-schema.org/draft-07/schema#",
  "title": "tmt.json",
  "description": "Project configuration for the TM-1 toolchain. The runtime walk in the tmt binary remains authoritative; this schema is an editing affordance and cannot express path normalization or cross-target comparison.",
  "type": "object",
  "properties": {
    "lint": {
      "type": "object",
      "description": "Lint configuration (schema 0.1; predates the project section).",
      "properties": {
        "allow": {
          "type": "array",
          "items": { "type": "string" },
          "description": "Lint codes to allow. Unioned with editor settings, never a cascade."
        }
      },
      "additionalProperties": false
    },
    "project": {
      "type": "object",
      "description": "Project manifest, schema 0.2.",
      "properties": {
        "stdlib": {
          "type": "boolean",
          "description": "Link the embedded standard library. Default true."
        },
        "sources": {
          "type": "array",
          "items": { "type": "string" },
          "description": "Sources shared by every target, as manifest-relative paths."
        },
        "libraries": { "$ref": "#/definitions/libraries" },
        "call-mech": {
          "type": "string",
          "enum": ["mono", "frames", "hybrid"],
          "description": "Project-wide default call lowering. A target may override it, and --call-mech overrides both."
        },
        "profiles": { "$ref": "#/definitions/profiles" },
        "targets": {
          "type": "object",
          "description": "Named build targets.",
          "additionalProperties": { "$ref": "#/definitions/target" }
        }
      },
      "additionalProperties": false
    }
  },
  "additionalProperties": false,
  "definitions": {
    "libraries": {
      "type": "object",
      "properties": {
        "dirs": {
          "type": "array",
          "items": { "type": "string" },
          "description": "Library search directories."
        },
        "link": {
          "type": "array",
          "items": { "type": "string" },
          "description": "Library names to link."
        }
      },
      "additionalProperties": false
    },
    "profiles": {
      "type": "object",
      "properties": {
        "debug": { "$ref": "#/definitions/profile" },
        "release": { "$ref": "#/definitions/profile" }
      },
      "additionalProperties": false
    },
    "profile": {
      "type": "object",
      "properties": {
        "opt": {
          "type": "string",
          "enum": ["O0", "O1"],
          "description": "Optimization level."
        },
        "debug-info": {
          "type": "boolean",
          "description": "Emit the debug map sidecar."
        },
        "strip-debugger": {
          "type": "boolean",
          "description": "Strip debugger breakpoints."
        },
        "werror": {
          "type": "boolean",
          "description": "Treat warnings as errors."
        }
      },
      "additionalProperties": false
    },
    "target": {
      "type": "object",
      "properties": {
        "sources": {
          "type": "array",
          "items": { "type": "string" },
          "description": "Sources for this target, appended to the project-level set."
        },
        "libraries": { "$ref": "#/definitions/libraries" },
        "entry": {
          "type": "string",
          "description": "Entry graph or routine name."
        },
        "output": {
          "type": "string",
          "description": "Output path, manifest-relative."
        },
        "call-mech": {
          "type": "string",
          "enum": ["mono", "frames", "hybrid"],
          "description": "Call lowering for this target, overriding the project default."
        },
        "run": { "$ref": "#/definitions/run" }
      },
      "additionalProperties": false
    },
    "run": {
      "type": "object",
      "description": "Settings for `tmt build --run <target>`. A target without this block cannot be run: tmt run drives a band from a .tmt snapshot and has no empty-tape default.",
      "properties": {
        "tape": {
          "type": "string",
          "description": "Path to the .tmt tape-block snapshot to run against."
        },
        "max-steps": {
          "type": "integer",
          "minimum": 0,
          "description": "Step limit. Mutually exclusive with no-step-limit."
        },
        "no-step-limit": {
          "type": "boolean",
          "description": "Run without a step limit. Mutually exclusive with max-steps."
        },
        "max-tacts": {
          "type": "integer",
          "minimum": 0,
          "description": "Tact limit."
        }
      },
      "additionalProperties": false,
      "not": { "required": ["max-steps", "no-step-limit"] }
    }
  }
}
```

- [ ] **Step 4: Run to verify the guard passes**

Run: `cargo test -p mtc-turing-machine project::tests -- --nocapture`

Expected: PASS.

- [ ] **Step 5: Replace the copy script**

Create `editors/vscode-tm/scripts/copy-assets.js`:

```js
// Copies the single-source editor assets into this extension's package
// directory: the shared TextMate grammars and the shared JSON Schema.
// Both live one level up so the -pm and -tm pairs consume one copy.
const fs = require('fs'), path = require('path');
const editorsDir = path.join(__dirname, '..', '..');

const grammarsSrc = path.join(editorsDir, 'grammars');
const grammarsDst = path.join(__dirname, '..', 'syntaxes');
fs.mkdirSync(grammarsDst, { recursive: true });
for (const name of ['tmc.tmLanguage.json', 'tma.tmLanguage.json']) {
  fs.copyFileSync(path.join(grammarsSrc, name), path.join(grammarsDst, name));
}

const schemasSrc = path.join(editorsDir, 'schemas');
const schemasDst = path.join(__dirname, '..', 'schemas');
fs.mkdirSync(schemasDst, { recursive: true });
fs.copyFileSync(
  path.join(schemasSrc, 'tmt.schema.json'),
  path.join(schemasDst, 'tmt.schema.json'),
);
```

Delete the old script:

```bash
rm editors/vscode-tm/scripts/copy-grammar.js
```

- [ ] **Step 6: Wire the schema and the new script into the manifest**

In `editors/vscode-tm/package.json`, add to `contributes` after `problemMatchers`:

```json
    "jsonValidation": [{
      "fileMatch": "tmt.json",
      "url": "./schemas/tmt.schema.json"
    }],
```

and replace `scripts`:

```json
  "scripts": {
    "copy-assets": "node scripts/copy-assets.js",
    "compile": "npm run copy-assets && tsc -p .",
    "package": "npm run compile && vsce package"
  },
```

- [ ] **Step 7: Verify the extension builds and the schema lands**

Run:
```bash
cd editors/vscode-tm && npm install && npm run compile
ls schemas/tmt.schema.json syntaxes/
```
Expected: compile succeeds; all three files exist.

- [ ] **Step 8: Gitignore the copied schema, as the copied grammars already are**

`editors/vscode-tm/.gitignore` currently reads:

```
node_modules/
out/
syntaxes/
*.vsix
```

Add `schemas/` on its own line after `syntaxes/`, for the same reason:
both directories are copies of a single source under `editors/`.

Run: `git check-ignore -v editors/vscode-tm/schemas/tmt.schema.json`

Expected: the rule matches.

- [ ] **Step 9: Run the full gates**

Run:
```bash
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --check
```
Expected: all pass.

- [ ] **Step 10: Commit**

```bash
git add editors/schemas/tmt.schema.json editors/vscode-tm crates/turing-machine/src/project.rs
git rm --cached editors/vscode-tm/scripts/copy-grammar.js 2>/dev/null || true
git commit -m "feat(editors): bundle a validating JSON Schema for tmt.json

The TM-1 twin of the PM-1 schema, and deliberately not a copy: call-mech
appears at both the project and target levels, and the run block is
tape-only with a no-step-limit switch and none of PM-1's per-cell knobs.
Two schemas rather than one conditional schema, because the two manifest
contracts are independently versioned and already diverge.

A unit test set-compares each level against the walk's inventories in
both directions, checks both call-mech enum sites against the same const
the CLI error renders from, and asserts the run block has NO implication
rule — so gaining one silently would fail rather than misdescribe."
```

---

### Task 5: PM-1 per-target tasks

**Files:**
- Modify: `editors/vscode-pm/src/extension.ts`
- Modify: `editors/vscode-pm/package.json` (`taskDefinitions`)

**Interfaces:**
- Consumes: `pmt build --list-targets`, which prints one line per target, `NAME` optionally followed by a TAB and the literal `run` — pinned by `crates/post-machine/tests/build_driver.rs`.
- Produces: task definition `{ type: 'pmt', command: 'build', target: string, run?: boolean }`.

- [ ] **Step 1: Extend the task definition contract**

In `editors/vscode-pm/package.json`, replace the `taskDefinitions` block:

```json
    "taskDefinitions": [{
      "type": "pmt",
      "required": ["command"],
      "properties": {
        "command": { "type": "string", "enum": ["compile", "lint", "fmt-check", "build"] },
        "file": { "type": "string" },
        "target": { "type": "string", "description": "Target name; `build` only." },
        "run": { "type": "boolean", "description": "Run the target after building; `build` only." }
      }
    }]
```

- [ ] **Step 2: Rewrite the task provider**

Replace the `PmtTaskProvider` class in `editors/vscode-pm/src/extension.ts` (currently lines 56-85) with:

```ts
/** One entry of `pmt build --list-targets` output. */
interface TargetEntry { name: string; run: boolean; }

/**
 * Parses `pmt build --list-targets` stdout: one line per target, the
 * name optionally followed by a TAB and the literal `run` when the
 * target declares a run block. The format is pinned by the crate's
 * build_driver tests.
 */
function parseTargets(stdout: string): TargetEntry[] {
  return stdout
    .split('\n')
    .filter((line) => line.length > 0)
    .map((line) => {
      const [name, marker] = line.split('\t');
      return { name, run: marker === 'run' };
    })
    .filter((entry) => entry.name.length > 0);
}

class PmtTaskProvider implements vscode.TaskProvider {
  /** Target lists by workspace-folder URI; invalidated by the watcher. */
  private cache = new Map<string, TargetEntry[]>();

  constructor(private pmtPath: string, private log: vscode.OutputChannel) {}

  /**
   * Drops every cached target list. Deliberately not per-folder: a
   * project file appearing or disappearing changes WHICH folders resolve
   * targets at all, and the cache holds at most one entry per workspace
   * folder, so a whole-cache clear costs nothing worth optimizing.
   */
  invalidate() {
    this.cache.clear();
  }

  async provideTasks(): Promise<vscode.Task[]> {
    return [...this.fileTasks(), ...(await this.targetTasks())];
  }

  /** The file-scoped tasks, unchanged: they follow the active editor. */
  private fileTasks(): vscode.Task[] {
    const doc = vscode.window.activeTextEditor?.document;
    if (!doc || (doc.languageId !== 'pmc' && doc.languageId !== 'pma')) { return []; }
    const file = doc.uri.fsPath;
    const tasks = [
      this.fileTask('lint', ['lint', file], file),
      this.fileTask('fmt-check', ['fmt', '--check', file], file),
    ];
    // `compile` stays .pmc-only — a .pma file assembles via `pmt asm`,
    // which this task provider doesn't offer (see the README).
    if (doc.languageId === 'pmc') {
      tasks.unshift(this.fileTask('compile', ['compile', file], file));
    }
    return tasks;
  }

  /**
   * One `build <target>` task per declared target, plus `build --run
   * <target>` where a run block exists. The extension never looks for a
   * manifest: `pmt build --list-targets` does its own nearest-ancestor
   * discovery from its working directory, so running it at the folder
   * root delegates the whole walk to the binary.
   */
  private async targetTasks(): Promise<vscode.Task[]> {
    const folders = vscode.workspace.workspaceFolders ?? [];
    const out: vscode.Task[] = [];
    for (const folder of folders) {
      for (const entry of await this.targetsFor(folder)) {
        out.push(this.buildTask(folder, entry.name, false));
        if (entry.run) { out.push(this.buildTask(folder, entry.name, true)); }
      }
    }
    return out;
  }

  private async targetsFor(folder: vscode.WorkspaceFolder): Promise<TargetEntry[]> {
    const key = folder.uri.toString();
    const cached = this.cache.get(key);
    if (cached) { return cached; }
    let entries: TargetEntry[] = [];
    try {
      entries = parseTargets(await this.listTargets(folder.uri.fsPath));
    } catch (err) {
      // No manifest, an invalid manifest, or a missing binary. None of
      // these may cost the user the file-scoped tasks, so this degrades
      // to "no targets here" and is only reported to the log.
      this.log.appendLine(`[${folder.name}] build --list-targets: ${err}`);
    }
    this.cache.set(key, entries);
    return entries;
  }

  private listTargets(cwd: string): Promise<string> {
    return new Promise((resolve, reject) => {
      execFile(this.pmtPath, ['build', '--list-targets'], { cwd }, (err, stdout, stderr) => {
        if (err) { reject(stderr.trim() || err.message); } else { resolve(stdout); }
      });
    });
  }

  resolveTask(task: vscode.Task): vscode.Task | undefined {
    const def = task.definition as unknown as vscode.TaskDefinition & {
      command: string; file?: string; target?: string; run?: boolean;
    };
    if (def.command === 'build') {
      // A per-target task MUST know its folder: `--list-targets`
      // discovery is cwd-driven, so resolving with the wrong cwd would
      // not fail — it would silently build a different project's target
      // of the same name. Refuse rather than guess.
      const scope = task.scope;
      if (!scope || typeof scope === 'number' || !def.target) { return undefined; }
      return this.buildTask(scope, def.target, def.run === true);
    }
    const file = def.file ?? '${file}';
    const args = def.command === 'fmt-check' ? ['fmt', '--check', file] : [def.command, file];
    return new vscode.Task(def, vscode.TaskScope.Workspace, `pmt ${def.command}`, 'pmt',
      new vscode.ProcessExecution(this.pmtPath, args), '$pmt');
  }

  private buildTask(folder: vscode.WorkspaceFolder, target: string, run: boolean): vscode.Task {
    const def: vscode.TaskDefinition = { type: 'pmt', command: 'build', target, run };
    const args = run ? ['build', '--run', target] : ['build', target];
    const name = run ? `pmt build --run ${target}` : `pmt build ${target}`;
    return new vscode.Task(def, folder, name, 'pmt',
      new vscode.ProcessExecution(this.pmtPath, args, { cwd: folder.uri.fsPath }), '$pmt');
  }

  private fileTask(command: string, args: string[], file: string): vscode.Task {
    const def: vscode.TaskDefinition = { type: 'pmt', command, file };
    return new vscode.Task(def, vscode.TaskScope.Workspace, `pmt ${command}`, 'pmt',
      new vscode.ProcessExecution(this.pmtPath, args), '$pmt');
  }
}
```

- [ ] **Step 3: Register the watcher and the output channel**

In `activate`, replace the single `context.subscriptions.push(...)` call with:

```ts
  const log = vscode.window.createOutputChannel('pmt');
  const provider = new PmtTaskProvider(pmtPath, log);
  // The project file is the target list's only input, so watching it is
  // a more precise invalidation than comparing mtimes on every
  // provideTasks call (docs/pmt/project.md (discovery)).
  const watcher = vscode.workspace.createFileSystemWatcher('**/pmt.json');
  watcher.onDidCreate(() => provider.invalidate());
  watcher.onDidChange(() => provider.invalidate());
  watcher.onDidDelete(() => provider.invalidate());
  context.subscriptions.push(
    log,
    watcher,
    vscode.workspace.onDidChangeWorkspaceFolders(() => provider.invalidate()),
    vscode.tasks.registerTaskProvider('pmt', provider),
  );
```

- [ ] **Step 4: Typecheck**

Run: `cd editors/vscode-pm && npm run compile`

Expected: no TypeScript errors. If `provideTasks` returning a Promise is rejected by the `TaskProvider` interface, that is a real error — `provideTasks` is declared as returning `ProviderResult<Task[]>`, which accepts a `Thenable`, so an `async` method is valid.

- [ ] **Step 5: Verify against a real manifest**

Build the binary and confirm the exact bytes the parser will see:

```bash
cargo build --release
PMT="$(git rev-parse --show-toplevel)/target/release/pmt"
cd /tmp && rm -rf pmtaskcheck && mkdir pmtaskcheck && cd pmtaskcheck
cat > pmt.json <<'EOF'
{ "project": { "sources": ["main.pmc"], "targets": {
    "app":    { "output": "app.pmx", "run": { "tape": "111" } },
    "notape": { "output": "notape.pmx" } } } }
EOF
printf 'main() {\n 1: halt;\n}\n' > main.pmc
"$PMT" build --list-targets | cat -A | head
```

Expected: `app^Irun$` then `notape$` — confirming TAB separation and the `run` marker, which is what `parseTargets` splits on.

- [ ] **Step 6: Commit**

```bash
git add editors/vscode-pm
git commit -m "feat(editors): per-target build tasks in the pmc extension

Adds one `pmt build <target>` task per declared target, plus `pmt build
--run <target>` where the target carries a run block, alongside the
existing file-scoped three.

The extension never looks for a manifest: `build --list-targets` already
does nearest-ancestor discovery from its working directory, so running it
at each workspace-folder root delegates the walk to the binary and no
discovery logic can drift from the CLI's. Per-target tasks are scoped to
their workspace folder, so a multi-root window contributes one set per
folder.

The target list is cached and invalidated by a project-file watcher.
A missing or invalid manifest degrades to no target tasks and a log line;
it never costs the user lint and fmt-check. resolveTask refuses a build
definition it cannot pin to a folder rather than guessing a working
directory, since a wrong cwd would silently build a different project's
same-named target."
```

---

### Task 6: TM-1 per-target tasks

**Files:**
- Modify: `editors/vscode-tm/src/extension.ts`
- Modify: `editors/vscode-tm/package.json` (`taskDefinitions`)

**Interfaces:**
- Consumes: `tmt build --list-targets`, same `NAME[\trun]` format, pinned by `crates/turing-machine/tests/build_driver.rs::list_targets_prints_name_and_run_marker`.
- Produces: task definition `{ type: 'tmt', command: 'build', target: string, run?: boolean }`.

- [ ] **Step 1: Extend the task definition contract**

In `editors/vscode-tm/package.json`, replace `taskDefinitions`:

```json
    "taskDefinitions": [{
      "type": "tmt",
      "required": ["command"],
      "properties": {
        "command": { "type": "string", "enum": ["compile", "asm", "lint", "fmt-check", "build"] },
        "file": { "type": "string" },
        "target": { "type": "string", "description": "Target name; `build` only." },
        "run": { "type": "boolean", "description": "Run the target after building; `build` only." }
      }
    }]
```

- [ ] **Step 2: Rewrite the task provider**

Replace the `TmtTaskProvider` class in `editors/vscode-tm/src/extension.ts` (currently lines 66-97) with the following. It mirrors the PM provider; the one behavioural difference is that `.tma` files get an `asm` task where PM offers none.

```ts
/** One entry of `tmt build --list-targets` output. */
interface TargetEntry { name: string; run: boolean; }

/**
 * Parses `tmt build --list-targets` stdout: one line per target, the
 * name optionally followed by a TAB and the literal `run` when the
 * target declares a run block. The format is pinned by the crate's
 * build_driver tests.
 */
function parseTargets(stdout: string): TargetEntry[] {
  return stdout
    .split('\n')
    .filter((line) => line.length > 0)
    .map((line) => {
      const [name, marker] = line.split('\t');
      return { name, run: marker === 'run' };
    })
    .filter((entry) => entry.name.length > 0);
}

class TmtTaskProvider implements vscode.TaskProvider {
  /** Target lists by workspace-folder URI; invalidated by the watcher. */
  private cache = new Map<string, TargetEntry[]>();

  constructor(private tmtPath: string, private log: vscode.OutputChannel) {}

  /**
   * Drops every cached target list. Deliberately not per-folder: a
   * project file appearing or disappearing changes WHICH folders resolve
   * targets at all, and the cache holds at most one entry per workspace
   * folder, so a whole-cache clear costs nothing worth optimizing.
   */
  invalidate() {
    this.cache.clear();
  }

  async provideTasks(): Promise<vscode.Task[]> {
    return [...this.fileTasks(), ...(await this.targetTasks())];
  }

  /** The file-scoped tasks, unchanged: they follow the active editor. */
  private fileTasks(): vscode.Task[] {
    const doc = vscode.window.activeTextEditor?.document;
    if (!doc || (doc.languageId !== 'tmc' && doc.languageId !== 'tma')) { return []; }
    const file = doc.uri.fsPath;
    const tasks = [
      this.fileTask('lint', ['lint', file], file),
      this.fileTask('fmt-check', ['fmt', '--check', file], file),
    ];
    // Each language gets its own front end: `.tmc` compiles, `.tma`
    // assembles. Both are single-file commands, so both are offered.
    if (doc.languageId === 'tmc') {
      tasks.unshift(this.fileTask('compile', ['compile', file], file));
    } else {
      tasks.unshift(this.fileTask('asm', ['asm', file], file));
    }
    return tasks;
  }

  /**
   * One `build <target>` task per declared target, plus `build --run
   * <target>` where a run block exists. The extension never looks for a
   * manifest: `tmt build --list-targets` does its own nearest-ancestor
   * discovery from its working directory, so running it at the folder
   * root delegates the whole walk to the binary.
   */
  private async targetTasks(): Promise<vscode.Task[]> {
    const folders = vscode.workspace.workspaceFolders ?? [];
    const out: vscode.Task[] = [];
    for (const folder of folders) {
      for (const entry of await this.targetsFor(folder)) {
        out.push(this.buildTask(folder, entry.name, false));
        if (entry.run) { out.push(this.buildTask(folder, entry.name, true)); }
      }
    }
    return out;
  }

  private async targetsFor(folder: vscode.WorkspaceFolder): Promise<TargetEntry[]> {
    const key = folder.uri.toString();
    const cached = this.cache.get(key);
    if (cached) { return cached; }
    let entries: TargetEntry[] = [];
    try {
      entries = parseTargets(await this.listTargets(folder.uri.fsPath));
    } catch (err) {
      // No manifest, an invalid manifest, or a missing binary. None of
      // these may cost the user the file-scoped tasks, so this degrades
      // to "no targets here" and is only reported to the log.
      this.log.appendLine(`[${folder.name}] build --list-targets: ${err}`);
    }
    this.cache.set(key, entries);
    return entries;
  }

  private listTargets(cwd: string): Promise<string> {
    return new Promise((resolve, reject) => {
      execFile(this.tmtPath, ['build', '--list-targets'], { cwd }, (err, stdout, stderr) => {
        if (err) { reject(stderr.trim() || err.message); } else { resolve(stdout); }
      });
    });
  }

  resolveTask(task: vscode.Task): vscode.Task | undefined {
    const def = task.definition as unknown as vscode.TaskDefinition & {
      command: string; file?: string; target?: string; run?: boolean;
    };
    if (def.command === 'build') {
      // A per-target task MUST know its folder: `--list-targets`
      // discovery is cwd-driven, so resolving with the wrong cwd would
      // not fail — it would silently build a different project's target
      // of the same name. Refuse rather than guess.
      const scope = task.scope;
      if (!scope || typeof scope === 'number' || !def.target) { return undefined; }
      return this.buildTask(scope, def.target, def.run === true);
    }
    const file = def.file ?? '${file}';
    const args = def.command === 'fmt-check' ? ['fmt', '--check', file] : [def.command, file];
    return new vscode.Task(def, vscode.TaskScope.Workspace, `tmt ${def.command}`, 'tmt',
      new vscode.ProcessExecution(this.tmtPath, args), '$tmt');
  }

  private buildTask(folder: vscode.WorkspaceFolder, target: string, run: boolean): vscode.Task {
    const def: vscode.TaskDefinition = { type: 'tmt', command: 'build', target, run };
    const args = run ? ['build', '--run', target] : ['build', target];
    const name = run ? `tmt build --run ${target}` : `tmt build ${target}`;
    return new vscode.Task(def, folder, name, 'tmt',
      new vscode.ProcessExecution(this.tmtPath, args, { cwd: folder.uri.fsPath }), '$tmt');
  }

  private fileTask(command: string, args: string[], file: string): vscode.Task {
    const def: vscode.TaskDefinition = { type: 'tmt', command, file };
    return new vscode.Task(def, vscode.TaskScope.Workspace, `tmt ${command}`, 'tmt',
      new vscode.ProcessExecution(this.tmtPath, args), '$tmt');
  }
}
```

- [ ] **Step 3: Register the watcher and the output channel**

In `activate`, replace the `context.subscriptions.push(...)` call with:

```ts
  const log = vscode.window.createOutputChannel('tmt');
  const provider = new TmtTaskProvider(tmtPath, log);
  // The project file is the target list's only input, so watching it is
  // a more precise invalidation than comparing mtimes on every
  // provideTasks call (docs/tmt/project.md (discovery)).
  const watcher = vscode.workspace.createFileSystemWatcher('**/tmt.json');
  watcher.onDidCreate(() => provider.invalidate());
  watcher.onDidChange(() => provider.invalidate());
  watcher.onDidDelete(() => provider.invalidate());
  context.subscriptions.push(
    log,
    watcher,
    vscode.workspace.onDidChangeWorkspaceFolders(() => provider.invalidate()),
    vscode.tasks.registerTaskProvider('tmt', provider),
  );
```

- [ ] **Step 4: Typecheck**

Run: `cd editors/vscode-tm && npm run compile`

Expected: no TypeScript errors.

- [ ] **Step 5: Verify against a real manifest**

The TM run block is `.tmt`-tape only, so a runnable target needs a snapshot:

```bash
cargo build --release
TMT="$(git rev-parse --show-toplevel)/target/release/tmt"
cd /tmp && rm -rf tmtaskcheck && mkdir tmtaskcheck && cd tmtaskcheck
cat > main.tmc <<'EOF'
alphabet bits { '_', '0', '1' }

machine {
  tape t: bits;
  entry state s { [*] -> stop; }
}
EOF
"$TMT" tape-block new --from main.tmc -o start.tmt
cat > tmt.json <<'EOF'
{ "project": { "sources": ["main.tmc"], "targets": {
    "app":    { "output": "app.tmx", "run": { "tape": "start.tmt" } },
    "notape": { "output": "notape.tmx" } } } }
EOF
"$TMT" build --list-targets | cat -A
```

Expected: `app^Irun$` then `notape$`.

The `tape-block new` invocation above is the shipped usage —
`tmt tape-block new [--from APP.tmx | --from APP.tmc] [-o OUT.tmt] [EDITS]`.
Note `--list-targets` reads the manifest and never opens the tape, so the
snapshot only has to exist for the `run` marker to appear.

- [ ] **Step 6: Commit**

```bash
git add editors/vscode-tm
git commit -m "feat(editors): per-target build tasks in the tmc extension

The TM-1 twin of the pmc provider: one `tmt build <target>` task per
declared target plus `tmt build --run <target>` where a run block exists,
alongside the existing file-scoped tasks, which keep the .tmc-compiles /
.tma-assembles split.

As on the PM side, the extension never looks for a manifest — running
`build --list-targets` at each workspace-folder root delegates discovery
to the binary. Per-target tasks are folder-scoped, the list is cached and
invalidated by a project-file watcher, a bad manifest degrades to a log
line, and resolveTask refuses a build definition it cannot pin to a
folder."
```

---

### Task 7: Bundle the pmc extension with esbuild

**Files:**
- Create: `editors/vscode-pm/esbuild.js`
- Modify: `editors/vscode-pm/package.json`
- Modify: `editors/vscode-pm/.vscodeignore`

**Interfaces:**
- Consumes: `editors/vscode-pm/src/extension.ts` from Task 5.
- Produces: `npm run typecheck`, `npm run bundle`, `npm run package` scripts; `out/extension.js` as a single bundled file.

- [ ] **Step 1: Record the baseline**

Run:
```bash
cd editors/vscode-pm && npm install && npm run package
ls -la *.vsix
unzip -l *.vsix | tail -1
```
Expected: a vsix in the hundreds of KB and a file count in the hundreds. Write both numbers down for the commit message.

- [ ] **Step 2: Add the bundle script**

Create `editors/vscode-pm/esbuild.js`:

```js
// Bundles the extension entry into a single CJS file so the vsix ships
// one script instead of the whole language-client dependency tree.
// `vscode` is provided by the host and must stay external.
const esbuild = require('esbuild');

esbuild.build({
  entryPoints: ['src/extension.ts'],
  bundle: true,
  outfile: 'out/extension.js',
  platform: 'node',
  format: 'cjs',
  target: 'node18',
  external: ['vscode'],
  minify: true,
}).catch(() => process.exit(1));
```

- [ ] **Step 3: Split the scripts so typechecking survives**

esbuild strips types WITHOUT checking them, so replacing `tsc -p .` with esbuild would silently disable typechecking. In `editors/vscode-pm/package.json`:

```json
  "scripts": {
    "copy-assets": "node scripts/copy-assets.js",
    "typecheck": "tsc --noEmit -p .",
    "bundle": "npm run copy-assets && node esbuild.js",
    "compile": "npm run typecheck && npm run bundle",
    "package": "npm run compile && vsce package"
  },
```

and add esbuild to `devDependencies`:

```json
  "devDependencies": {
    "@types/node": "^20.0.0", "@types/vscode": "^1.91.0",
    "@vscode/vsce": "^3.9.2", "esbuild": "^0.25.0", "typescript": "^5.9.0"
  }
```

- [ ] **Step 4: Prove typechecking still fails on a type error**

Temporarily append a type error to `src/extension.ts`:

```ts
const deliberateTypeError: number = 'not a number';
```

Run: `cd editors/vscode-pm && npm run compile`

Expected: FAIL, with `Type 'string' is not assignable to type 'number'`. Then **delete that line** and re-run to confirm it passes. This is the whole point of Step 3 — verify it, do not assume it.

- [ ] **Step 5: Tighten `.vscodeignore`**

Replace `editors/vscode-pm/.vscodeignore`:

```
.vscode/**
.gitignore
src/**
scripts/**
node_modules/**
esbuild.js
tsconfig.json
**/*.map
*.vsix
```

`syntaxes/`, `schemas/`, both `language-configuration*.json`, `README.md`, `LICENSE`, and `package.json` are NOT listed, so they ship.

- [ ] **Step 6: Repackage and compare**

Run:
```bash
cd editors/vscode-pm && rm -f *.vsix && npm run package
ls -la *.vsix
unzip -l *.vsix | tail -1
unzip -l *.vsix | grep -E 'schemas/|syntaxes/|language-configuration'
```
Expected: far fewer files and a much smaller archive; the schema, both grammars, and both language-configuration files still present.

- [ ] **Step 7: Refresh the lockfile**

Run:
```bash
cd editors/vscode-pm && rm -rf node_modules package-lock.json && npm install
npm audit --omit=dev
```
Expected: the transitive `brace-expansion` advisory no longer appears. If `npm audit` still reports it, note which dependency pulls it and whether a newer `@vscode/vsce` clears it — do not force an unrelated major upgrade to chase it.

- [ ] **Step 8: Full gates**

Run:
```bash
cd editors/vscode-pm && npm run compile
cd ../.. && cargo test --workspace
```
Expected: both pass.

- [ ] **Step 9: Commit**

```bash
git add editors/vscode-pm
git commit -m "build(editors): bundle the pmc extension with esbuild

The vsix shipped the language-client dependency tree unbundled. esbuild
now emits a single out/extension.js and .vscodeignore drops node_modules,
sources, and the build scripts, keeping the grammars, the schema, both
language-configuration files, README and LICENSE as assets.

esbuild strips types without checking them, so the scripts split:
typecheck runs tsc --noEmit and package runs it before bundling.
Otherwise this change would have quietly disabled typechecking.

The lockfile refresh that rides this clears the transitive
brace-expansion advisory."
```

---

### Task 8: Bundle the tmc extension with esbuild

**Files:**
- Create: `editors/vscode-tm/esbuild.js`
- Modify: `editors/vscode-tm/package.json`
- Modify: `editors/vscode-tm/.vscodeignore`

**Interfaces:**
- Consumes: `editors/vscode-tm/src/extension.ts` from Task 6.
- Produces: `npm run typecheck`, `npm run bundle`, `npm run package`; `out/extension.js`.

- [ ] **Step 1: Record the baseline**

Run:
```bash
cd editors/vscode-tm && npm install && npm run package
ls -la *.vsix
unzip -l *.vsix | tail -1
```
Write both numbers down for the commit message.

- [ ] **Step 2: Add the bundle script**

Create `editors/vscode-tm/esbuild.js`:

```js
// Bundles the extension entry into a single CJS file so the vsix ships
// one script instead of the whole language-client dependency tree.
// `vscode` is provided by the host and must stay external.
const esbuild = require('esbuild');

esbuild.build({
  entryPoints: ['src/extension.ts'],
  bundle: true,
  outfile: 'out/extension.js',
  platform: 'node',
  format: 'cjs',
  target: 'node18',
  external: ['vscode'],
  minify: true,
}).catch(() => process.exit(1));
```

- [ ] **Step 3: Split the scripts so typechecking survives**

In `editors/vscode-tm/package.json`:

```json
  "scripts": {
    "copy-assets": "node scripts/copy-assets.js",
    "typecheck": "tsc --noEmit -p .",
    "bundle": "npm run copy-assets && node esbuild.js",
    "compile": "npm run typecheck && npm run bundle",
    "package": "npm run compile && vsce package"
  },
```

and add esbuild to `devDependencies`:

```json
  "devDependencies": {
    "@types/node": "^20.0.0", "@types/vscode": "^1.91.0",
    "@vscode/vsce": "^3.9.2", "esbuild": "^0.25.0", "typescript": "^5.9.0"
  }
```

- [ ] **Step 4: Prove typechecking still fails on a type error**

Temporarily append to `src/extension.ts`:

```ts
const deliberateTypeError: number = 'not a number';
```

Run: `cd editors/vscode-tm && npm run compile`

Expected: FAIL with `Type 'string' is not assignable to type 'number'`. **Delete the line** and re-run to confirm it passes.

- [ ] **Step 5: Tighten `.vscodeignore`**

Replace `editors/vscode-tm/.vscodeignore`:

```
.vscode/**
.gitignore
src/**
scripts/**
node_modules/**
esbuild.js
tsconfig.json
**/*.map
*.vsix
```

- [ ] **Step 6: Repackage and compare**

Run:
```bash
cd editors/vscode-tm && rm -f *.vsix && npm run package
ls -la *.vsix
unzip -l *.vsix | tail -1
unzip -l *.vsix | grep -E 'schemas/|syntaxes/|language-configuration'
```
Expected: far fewer files, much smaller archive, all assets present.

- [ ] **Step 7: Refresh the lockfile**

Run:
```bash
cd editors/vscode-tm && rm -rf node_modules package-lock.json && npm install
npm audit --omit=dev
```

- [ ] **Step 8: Full gates**

Run:
```bash
cd editors/vscode-tm && npm run compile
cd ../.. && cargo test --workspace
```

- [ ] **Step 9: Commit**

```bash
git add editors/vscode-tm
git commit -m "build(editors): bundle the tmc extension with esbuild

The TM-1 twin of the pmc bundling change: a single out/extension.js, a
.vscodeignore that drops node_modules and sources while keeping the
grammars, the schema and both language-configuration files, and a
typecheck script so replacing tsc with esbuild does not silently disable
type errors."
```

---

### Task 9: Documentation

**Files:**
- Modify: `editors/vscode-pm/README.md`, `editors/vscode-tm/README.md`
- Modify: `editors/jetbrains-pm/README.md`, `editors/jetbrains-tm/README.md`
- Modify: `docs/pmt/project.md`, `docs/tmt/project.md`
- Modify: `crates/post-machine/tests/build_driver.rs`, `crates/turing-machine/tests/build_driver.rs`

**Interfaces:**
- Consumes: everything from Tasks 1-8.
- Produces: no code interface.

- [ ] **Step 1: Note the second consumer on both `--list-targets` tests**

The output format is now parsed by the task providers, which is invisible from the test. Add to the doc comment above `list_targets_prints_name_and_run_marker` in `crates/turing-machine/tests/build_driver.rs`, and its PM-1 twin in `crates/post-machine/tests/build_driver.rs`:

```rust
/// This exact byte format has a SECOND consumer that is not visible from
/// here: the VS Code extension's task provider splits these lines on TAB
/// to build its per-target tasks. Tidying the output would break the
/// editors silently, so this assertion is a contract, not a convenience.
```

Run: `cargo test --workspace` — expected to still pass; this is a comment-only change.

- [ ] **Step 2: Rewrite the two VS Code READMEs' task sections**

In each of `editors/vscode-pm/README.md` and `editors/vscode-tm/README.md`, document the per-target tasks and demote the hand-written pipeline snippet. Use `pmt`/`pmc` or `tmt`/`tmc` as appropriate:

```markdown
## Tasks

Two families are offered under the `pmt` task type.

**Per-target tasks** come from the project manifest. When a workspace
folder resolves a `pmt.json` with a `project` section, every declared
target gets a `pmt build <target>` task, and every target carrying a
`run` block also gets `pmt build --run <target>`. The list refreshes when
the project file changes. In a multi-root window each folder contributes
its own targets.

**File-scoped tasks** act on the active editor's file: `compile`, `lint`,
and `fmt-check`.

Both families report through the `$pmt` problem matcher, so errors land
in the Problems panel.

A folder with no project file, or with a manifest the toolchain rejects,
simply contributes no per-target tasks — the file-scoped ones keep
working. The reason appears in the `pmt` output channel.

### Custom pipelines

For a build shape the manifest does not express, write the stages by hand
in `tasks.json` and chain them with `dependsOn`.
```

Keep whatever pipeline example each README already carries, moved under
that final heading.

- [ ] **Step 3: Add the JetBrains recipes**

Append to `editors/jetbrains-pm/README.md` (and the TM twin, substituting `tmt`/`tmt.json`):

```markdown
## Building a target

The plugin ships no build integration — LSP features arrive through the
server, and builds run as ordinary IDE run configurations.

To add one: **Run → Edit Configurations → + → Shell Script**, set
*Script text* to `pmt build <target>` and *Working directory* to the
directory holding your `pmt.json`. Add `--run` to build and then run the
target. `pmt build --list-targets` prints the declared target names, and
marks the runnable ones.

## Manifest validation

`pmt.json` has a bundled JSON Schema, but JetBrains maps schemas through
its own settings rather than a plugin contribution. To enable it:
**Settings → Languages & Frameworks → Schemas and DTDs → JSON Schema
Mappings → +**, point *Schema file or URL* at
`editors/schemas/pmt.schema.json` from this repository, select schema
version *Draft-07*, and add a file-path pattern of `pmt.json`.
```

- [ ] **Step 4: Add the editor-integration note to both project docs**

Append to `docs/pmt/project.md` (and the TM twin):

```markdown
## Editor integration

The VS Code extension turns each declared target into a task — one to
build it, and one to build and run it where a `run` block exists. It
discovers the manifest by running `pmt build --list-targets` at the
workspace folder root, so editor and command line always agree on which
project answers.

The extension also bundles a JSON Schema for this file, giving key
completion, hover text, and inline errors while editing it. The schema
describes key names, types, and the mutually exclusive pairs; it cannot
express the rules that compare paths or span targets, so the toolchain's
own validation stays authoritative and a manifest that an editor shows as
clean can still be rejected with a precise error.
```

- [ ] **Step 5: Check for forge references**

Run:
```bash
rg -n 'github\.com|#[0-9]{1,3}\b' editors/*/README.md docs/pmt/project.md docs/tmt/project.md
```
Expected: only the pre-existing `repository` URLs in package manifests, which are allowed. No issue numbers, no hosting URLs in the prose you added. Fix any you introduced.

- [ ] **Step 6: Full gates**

Run:
```bash
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --check
```

- [ ] **Step 7: Commit**

```bash
git add editors docs crates/post-machine/tests/build_driver.rs crates/turing-machine/tests/build_driver.rs
git commit -m "docs(editors): document per-target tasks and manifest validation

Both VS Code READMEs describe the per-target and file-scoped task
families and what happens when a folder has no usable manifest; the
hand-written pipeline snippets demote to a custom-pipelines note now that
the common case is a real task.

Both JetBrains READMEs gain a run-configuration recipe around build and a
JSON-schema-mapping recipe, since JetBrains maps schemas through its own
settings rather than a plugin contribution.

Both project.md pages gain an editor-integration section stating plainly
that the schema is an affordance and the toolchain's own validation stays
authoritative.

The two --list-targets tests record that their byte format has a second
consumer the test cannot see."
```

---

## Final verification

- [ ] **All gates green from a clean tree**

```bash
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --check
cd editors/vscode-pm && npm run compile && cd ../..
cd editors/vscode-tm && npm run compile && cd ../..
```

- [ ] **`crates/core` is untouched**

Run: `git diff --stat master...HEAD -- crates/core`

Expected: empty. Core neutrality is a standing constraint.

- [ ] **No version or floor moved**

Run: `rg -n 'MIN_TESTED_PMT|MIN_TESTED_TMT' editors/ && git diff master...HEAD -- editors/*/package.json editors/jetbrains-*/build.gradle.kts | rg '^[+-].*version'`

Expected: floors still `0.2.0`; no `version` line changed in any plugin manifest. Those are release-prep work.

- [ ] **Live sideload verification** — the maintainer's step, deliberately left unticked

Install both vsix, then confirm in a real window:

- [ ] a folder with a manifest lists `build <target>` for each target
- [ ] a target with a `run` block also lists `build --run <target>`
- [ ] editing the project file refreshes the list without reloading the window
- [ ] a folder with no manifest still offers `lint` and `fmt-check`
- [ ] a deliberately broken manifest degrades gracefully and logs to the output channel
- [ ] a build error lands in the Problems panel via the matcher
- [ ] typing an unknown key in `pmt.json` / `tmt.json` squiggles
- [ ] hovering a manifest key shows its description
- [ ] `tape` alongside `tape-block` squiggles in `pmt.json`; `max-steps` alongside `no-step-limit` squiggles in `tmt.json`
