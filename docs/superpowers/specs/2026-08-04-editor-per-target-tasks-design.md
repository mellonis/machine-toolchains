# Design: editor per-target tasks, manifest schemas, and bundling

Manifest + build design, plan 3 (the editor round). Closes
[#58](https://github.com/mellonis/machine-toolchains/issues/58),
[#54](https://github.com/mellonis/machine-toolchains/issues/54), and
[#24](https://github.com/mellonis/machine-toolchains/issues/24).
Milestone: [Plan 3 — editor per-target
tasks](https://github.com/mellonis/machine-toolchains/milestone/2).

Driving spec: `docs/superpowers/specs/2026-07-12-project-manifest-and-build-design.md`,
§ Editors (with its 2026-07-21 TM-1 amendment).

## 1. Round scope

Three of the milestone's four issues ship together, because all three
open the same four plugin manifests and doing them in three passes means
opening those files three times:

- **#58** — per-target `build` / `build --run` tasks in both VS Code
  extensions, generated from `--list-targets`.
- **#54** — a JSON Schema per project-manifest file, bundled with the
  plugins, plus the mandatory drift guard and its prerequisite.
- **#24** — esbuild bundling and a tightened `.vscodeignore` for both
  extensions.

**Out of this round**: #51 prong 2 (LSP4IJ `SemanticTokensColorsProvider`)
— Kotlin, both JetBrains plugins, no overlap with the surfaces above. Its
blocker is gone (plan 2, #56, closed 2026-07-26), so it can run as its own
round whenever.

### 1.1 Already shipped — do not re-do

- **#51 prong 1** (re-scope the grammars' keyword tier) shipped in the
  pre-release-polish round: all four grammars are on
  `keyword.control.<lang>`.
- **The `$pmt` / `$tmt` problem matchers** already ship in both VS Code
  `package.json`s and already fit `build`'s `file:line:col` error
  format. The driving spec lists a matcher as part of this round; it is
  done.
- **`--list-targets` itself**, emitting `NAME[\trun]\n` per target, and
  pinned byte-exactly by existing tests (§5.1).

### 1.2 Rulings taken

| Question | Ruling |
|---|---|
| Manifest lookup (#58) | Per workspace folder — §2.1 |
| Target-list caching (#58) | Watcher-invalidated, not mtime — §2.3 |
| `$schema` key in the manifests (#54) | Editor mapping only; manifest files unchanged, accepted-key set unchanged, both schema versions stay 0.2 — §3.5 |
| One schema or two (#54) | Two, per toolchain — §3.3 |
| JetBrains integration | Documented recipe only, no plugin code — §2.5, §3.5 |
| `MIN_TESTED_*` floors | Deferred to release prep — §6 |

## 2. #58 — per-target tasks

### 2.1 The extension never looks for a manifest

`pmt build --list-targets` already performs nearest-ancestor discovery
from its own working directory (`docs/pmt/project.md` (discovery)).
Shelling it with `cwd` set to a workspace folder's root therefore
delegates the entire walk to the binary:

```
for each workspace folder:
    execFile(<toolPath>, ['build', '--list-targets'], { cwd: folder.uri.fsPath })
      ok   → parse `NAME[\trun]` lines
             → one `build <target>` task per target
             → one `build --run <target>` task where the marker is present
      fail → no per-target tasks for this folder
```

No discovery logic is duplicated in TypeScript, and none can drift from
the CLI's. A folder with no project simply yields nothing.

### 2.2 Task shape

`provideTasks()` returns today's file-scoped tasks — `compile`/`asm`,
`lint`, `fmt-check` against the active editor's document, **unchanged** —
concatenated with the per-target tasks. Per-target tasks are scoped to
their **workspace folder**, not `vscode.TaskScope.Workspace`; that is what
makes a multi-root workspace contribute one target set per folder.

The task definition, which `resolveTask` must also accept so a
hand-authored `tasks.json` entry resolves:

```jsonc
{ "type": "tmt", "command": "build", "target": "flagship", "run": true }
```

Execution is `ProcessExecution(<toolPath>, ['build', <target>], { cwd: <folder> })`
for the plain variant and `['build', '--run', <target>]` for the run
variant, with the existing `$pmt` / `$tmt` problem matcher attached.

**`resolveTask` must derive the cwd, not default it.** The definition
carries no folder, and today's `resolveTask` hardcodes
`vscode.TaskScope.Workspace`. Since `--list-targets` discovery is
cwd-driven, a per-target task resolved with the wrong cwd does not fail —
it silently builds *a different project's* target of the same name. So:
take the cwd from `task.scope` when VS Code supplies a `WorkspaceFolder`,
and when it does not (a `build` definition at workspace scope in a
multi-root window), return `undefined` rather than guessing. The
file-scoped commands keep their existing resolution path unchanged.

### 2.3 Caching and invalidation

A `Map` keyed by workspace-folder URI holds the parsed target list.
Invalidated by:

- a `FileSystemWatcher` on the folder's `pmt.json` / `tmt.json` —
  create, change, **and** delete;
- `onDidChangeWorkspaceFolders`.

#58 offers mtime-comparison caching as an option. The watcher supersedes
it: #58 already requires a watcher for task refresh, so mtime comparison
would be a second, less precise mechanism for the same job.

### 2.4 Failure surface

`execFile` rejection, a non-zero exit, and unparseable output all resolve
to "no targets for this folder", recorded in an output channel.
`provideTasks` never throws. An invalid manifest must not cost the user
`lint` and `fmt-check`, which have nothing to do with the project file.

### 2.5 What this round does not do

- **No shared TypeScript module between the two extensions.** They are
  independent npm packages already duplicating ~85 lines of near-identical
  client code; sharing source would need the `copy-grammar` treatment.
  Duplication is the existing house layout for these two files — a
  conscious choice, not an oversight.
- **No JetBrains plugin code.** Both JetBrains READMEs gain a documented
  run-configuration recipe around `build`. The real plugin-side
  integration is [#29](https://github.com/mellonis/machine-toolchains/issues/29).

## 3. #54 — manifest schemas

### 3.1 Where the guard lives decides whether it runs

The repository has one CI workflow (`audit.yml`); the real quality gate
is `cargo test --workspace`. The drift guard is therefore a **Rust test
per crate**, reading `../../editors/schemas/*.json` through
`CARGO_MANIFEST_DIR` — the file-reading shape
`crates/*/tests/editor_grammar.rs` already uses to guard the shared
TextMate grammars against the parser. A guard written in npm would not
run.

**It is a unit test, not an integration test.** `mod project;` is private
in both crates' `lib.rs`, so a test under `tests/` cannot see the key
inventories, and making the module public purely so a test can read it
would widen the crate's public API for no other reason. The guard lives
in the `#[cfg(test)] mod tests` block that each `project.rs` already
has.

### 3.2 The prerequisite: load-bearing key inventories

Each level's accepted keys are currently string literals in
`match key.as_str()` arms in `project.rs` — nothing can enumerate them.

Rather than adding a parallel const list that a test probes
(unidirectional, the self-documented weakness of
`completions_registry.rs`), the list becomes **load-bearing**: the parse
loop checks membership before dispatching, so the const *is* the
acceptance authority and cannot drift from behaviour. This is the shape
`recognized_directives(caps)` uses — one source consulted by both the
inventory and the recognizer.

The guard then reduces to a set-compare, per level, in both directions,
between the schema's `properties` keys and the crate's const lists —
plus enum values, whose sources (`opt` levels; TM-1's `call-mech`
spellings) are likewise const.

**The pre-check must not swallow value errors.** Several arms today
produce their own diagnostics for a *recognized* key holding a *bad
value* — `parse_profile`'s `unknown opt level \`{other}\` (O0 | O1)`,
`parse_call_mech_value`'s `unknown call-mech \`{other}\``. A blanket
membership check placed carelessly would reroute those into a generic
unknown-*key* error and lose the good message. The check gates keys only;
value parsing is untouched. The existing `UnknownKey` unit tests cannot
catch a regression here — they exercise keys — so §5.2 adds a
bad-enum-value gate.

### 3.3 Two schemas, not one

`editors/schemas/pmt.schema.json` and `editors/schemas/tmt.schema.json`,
shared into both plugin pairs by extending the existing `copy-grammar`
script — the same single-source-plus-copy pattern as `editors/grammars/`.

`pmt.json` and `tmt.json` are independently versioned contracts that
already diverge at 0.2 (TM-1's `call-mech`; its `.tmt`-path, tape-only
run block). One conditional schema would misrepresent that, and the split
matches the per-toolchain `docs/{pmt,tmt}/project.md` split.

### 3.4 What the schema expresses

**Expressible, and expressed**: key names, types, enums, and both
shapes of cross-key rule the run blocks actually use.

#54's bullet list files these under "cross-key and inexpressible", but
two bullets later states that JSON Schema covers "`oneOf`-shaped
exclusivity". **The issue overstates — they are expressible — but they
are not all one shape**, and the schemas must not treat them as one.
Read from the two walks:

| Rule | Toolchain | Shape | Schema mechanism |
|---|---|---|---|
| `tape` vs `tape-block` | PM-1 | mutual exclusion | `not: { required: ["tape", "tape-block"] }` |
| `head` requires `tape` | PM-1 | **implication**, not exclusion | `dependencies: { head: ["tape"] }` |
| `max-steps` vs `no-step-limit` | TM-1 | mutual exclusion | `not: { required: ["max-steps", "no-step-limit"] }` |

The `head` leg is an asymmetric dependency — `head` alone is an error,
`tape` alone is fine — so an exclusivity construct would express the
opposite of the rule.

The two run blocks otherwise diverge outright: PM-1 has `tape-block`,
`head`, `strict-cells`, and `tact-profile`; TM-1 has `no-step-limit` and
none of those. This is §3.3's per-toolchain split earning its keep, not
an incidental difference.

**Schema draft**: draft-07, declared in each file's `$schema`. It has the
broadest validator support across VS Code and JetBrains, and its
`dependencies` keyword covers the implication leg. (2019-09 and later
spell the same rule `dependentRequired`; if a target validator turns out
to reject draft-07, that is the one-line change, and the drift guard is
unaffected either way.)

**Genuinely inexpressible**, staying in `project.rs` alone:

- two targets whose `output` paths collide after lexical normalization
  (cross-target comparison)
- a source appearing twice in a target's *effective* list (project-level
  ++ target-level)
- absolute-path rejection, with the message naming the
  manifest-relative rule

The runtime validation walk stays authoritative and unchanged in
behaviour. The CLI must reject a bad manifest with precise errors whether
or not an editor was in the loop; the schema is an editing affordance
layered on top, never the authority.

### 3.5 Delivery

- `contributes.jsonValidation` in both VS Code `package.json`s, matching
  `pmt.json` / `tmt.json` to the bundled schema.
- **No `$schema` key in the manifest files.** The strict unknown-key walk
  stays intact, the accepted-key set is unchanged, and both manifest
  schema versions stay at **0.2**. The cost is that validation works
  where our plugins are installed and nowhere else — accepted.
- JetBrains: a documented Settings → JSON Schema Mappings recipe in both
  READMEs. No plugin code.

## 4. #24 — bundling

esbuild each extension to a single `out/extension.js` — CJS,
`platform=node`, `vscode` external, minified. `main` repoints to it.
`.vscodeignore` gains `node_modules/**` and the build script; `syntaxes/`,
the new `schemas/`, both `language-configuration*.json`, README and
LICENSE stay as assets. The `copy-grammar` step survives, extended to
copy the schemas too.

**The trap: esbuild strips types without checking them.** Today
`compile` runs `tsc -p .`, so a type error fails the build. Replacing it
with esbuild would silently lose that — on the very files #58 is adding.
The scripts therefore split:

| script | does |
|---|---|
| `typecheck` | `tsc --noEmit -p .` |
| `bundle` | copy assets, then esbuild |
| `package` | `typecheck` && `bundle` && `vsce package` |

The `package-lock.json` refresh riding this round clears the open **high**
`brace-expansion` dependabot alert on
`editors/vscode-pm/package-lock.json`.

Expected effect, per #24's own measurement: 362 files / ~600 KB down to a
handful of files well under 100 KB, with faster activation.

## 5. Testing

### 5.1 A gate that already exists, and gains a consumer

`--list-targets`' output format is already pinned byte-exactly —
`crates/turing-machine/tests/build_driver.rs::list_targets_prints_name_and_run_marker`
asserts `"app\trun\nnotape\nzmono\n"`, and PM-1 has its twin in
`crates/post-machine/tests/build_driver.rs`. No new CLI test is needed.

But those tests now have a **second consumer that is invisible from where
they sit**: the task provider parses this format. Each gets a doc-comment
note saying so, or a future tidy-the-output change breaks both editors
silently.

### 5.2 Gate matrix

| Gate | Where | What it proves |
|---|---|---|
| key-inventory refactor is behaviour-neutral | `cargo test --workspace`; each crate's existing `project.rs` unit tests asserting `ConfigError::UnknownKey` | key acceptance unchanged |
| bad enum values keep their own diagnostics | new unit tests per crate: `opt: "O2"`, and TM-1 `call-mech: "nope"` | the §3.2 pre-check gates keys only, never values |
| schema ↔ parser agreement | new unit tests in each crate's `src/project.rs` `#[cfg(test)] mod tests` | bidirectional set-compare per level, incl. enum values |
| `--list-targets` format | existing `build_driver.rs` tests | the provider's input contract holds |
| quality | `cargo clippy --workspace --all-targets -- -D warnings`, `cargo fmt --check` | round floor |
| extensions typecheck | `npm run typecheck` in both | covers esbuild's blind spot |
| packaging | `npm run package` in both | vsix builds; record file count and size before → after |

### 5.3 Live verification

Sideloading both vsix and confirming per-target tasks appear, run, and
report through the problem matcher — plus schema completion and squiggles
on a hand-edited manifest — is the maintainer's post-merge step. The
checklists ship **unticked**, per house convention.

## 6. Deferred to release prep

Recorded here so it is not lost:

- All four `MIN_TESTED_PMT` / `MIN_TESTED_TMT` floors bump. The floor must
  name the release that first ships a `build` subcommand; that version is
  not fixed yet, so setting it now would be a guess.
- The four plugin version bumps.

Both belong to the release cut, alongside the CHANGELOG version block.

## 7. Documentation

- **Both VS Code READMEs** — per-target tasks documented; the
  hand-written pipeline snippet demotes to a "custom pipelines" note.
- **Both JetBrains READMEs** — the run-configuration recipe around
  `build`, and the JSON-schema-mapping recipe.
- **`docs/pmt/project.md`, `docs/tmt/project.md`** — a short
  editor-integration note each.
- All published content stays forge-agnostic per the workspace's
  published-docs policy: substance in prose, no issue or PR numbers, no
  hosting-provider URLs. New code comments cite durable pages by page plus
  parenthetical keyword; no `docs/superpowers/` citation.
