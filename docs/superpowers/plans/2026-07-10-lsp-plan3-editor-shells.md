# `pmt lsp` Plan 3/3 — editor shells: shared grammar, VS Code, JetBrains

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax.

**Goal:** The in-repo editor shells under `editors/`: a single-sourced TextMate grammar, a sideloadable VS Code extension (LSP client + settings + problem matcher + task provider) and a sideloadable JetBrains plugin on LSP4IJ (LSP client + settings page + run configurations + TextMate registration), each with a README carrying the manual test checklist.

**Architecture:** Version-thin clients — the load-bearing rule: no plugin contains language knowledge. Neither shell parses `.pmc`, reads `pmt.json`, or knows `PMC_LANG_VERSION`; everything language-versioned arrives over the wire from the user's `pmt` binary. The grammar is the single language-coupled artifact and it is cosmetic-only. Skew policy: check `pmt --version` on startup — **warn, not block** — against the shell's declared minimum-tested version.

**Tech Stack:** TypeScript + `vscode-languageclient` + `@vscode/vsce` (VS Code); Kotlin + Gradle + IntelliJ Platform Gradle Plugin + LSP4IJ (JetBrains). npm/Gradle dependencies live only under `editors/` — the Rust workspace stays dependency-clean. **Prerequisite: plans 1+2 landed** (`pmt lsp` works on stdio). Design authority: `docs/superpowers/specs/2026-07-07-pmt-lsp-design.md` (Editor shells, Version compatibility sections).

## Global Constraints

- **No language knowledge in shells.** No `.pmc` parsing, no `pmt.json` reading, no grammar duplication. The grammar lives ONLY at `editors/grammars/pmc.tmLanguage.json`; both shells obtain it at build time (copy step) — **no second copy is ever committed**.
- **Sideload-only.** No marketplace publishing, no publisher tokens, no release CI. Artifacts (`.vsix`, plugin zip) are built locally and attached to GitHub releases by hand.
- **Rust gates still green at every commit** (`cargo test --workspace`, clippy `-D warnings`, `cargo fmt --check`) — Task 1 adds one Rust drift test; shell code itself is outside cargo. Shell build checks: `npm run package` and `./gradlew buildPlugin` must succeed from a clean checkout (run them before each commit of the respective shell).
- **Warn, not block** on version skew; a pre-LSP binary already fails loudly (no `lsp` subcommand).
- Shell READMEs are published docs: **forge-agnostic prose** (no issue/PR numbers; the repo URL only where an install command genuinely needs it — prefer "this repository").
- **Conventional commits**, scope `feat(editors):` / `docs(editors):`. **No AI/Claude attribution footers.**
- Do **NOT** merge or push; the branch is left for the user's review.
- External-ecosystem versions (VS Code engine, LSP4IJ, Gradle plugin, IDE baseline) are pinned in this plan as of its writing — **verify each against its registry at implementation time** and bump the pin if a newer stable exists; record what you pinned in the commit message.

## File Structure

- `.gitignore` — editors build outputs (Task 1).
- `editors/grammars/pmc.tmLanguage.json` — the shared grammar (Task 1).
- `crates/post-machine/tests/editor_grammar.rs` — grammar drift guard (Task 1).
- `editors/vscode/` — `package.json`, `language-configuration.json`, `tsconfig.json`, `scripts/copy-grammar.js`, `src/extension.ts`, `README.md`, `.gitignore` (Tasks 2–3).
- `editors/jetbrains/` — `build.gradle.kts`, `settings.gradle.kts`, gradle wrapper, `src/main/resources/META-INF/plugin.xml`, `src/main/resources/textmate/pmc/package.json` (bundle manifest — not a grammar copy), Kotlin sources under `src/main/kotlin/` (Tasks 4–5), `README.md` (Task 6).

---

### Task 1: `editors/` scaffold + the shared TextMate grammar + drift guard

**Files:**
- Modify: `.gitignore` (repo root)
- Create: `editors/grammars/pmc.tmLanguage.json`
- Create: `crates/post-machine/tests/editor_grammar.rs`

**`.gitignore` additions:**

```gitignore
editors/vscode/node_modules/
editors/vscode/out/
editors/vscode/syntaxes/
editors/vscode/*.vsix
editors/jetbrains/.gradle/
editors/jetbrains/build/
```

**The grammar** (scope `source.pmc`; cosmetic-only — the server's semantic tokens carry the resolution-aware layer):

```json
{
  "$schema": "https://raw.githubusercontent.com/martinring/tmlanguage/master/tmlanguage.json",
  "name": "PMC",
  "scopeName": "source.pmc",
  "patterns": [
    { "include": "#comments" },
    { "include": "#keywords" },
    { "include": "#commands" },
    { "include": "#call" },
    { "include": "#definition" },
    { "include": "#label" },
    { "include": "#number" },
    { "include": "#punctuation" }
  ],
  "repository": {
    "comments": {
      "patterns": [
        { "name": "comment.line.double-slash.pmc", "match": "//.*$" },
        { "name": "comment.block.pmc", "begin": "/\\*", "end": "\\*/" }
      ]
    },
    "keywords": {
      "patterns": [
        { "name": "keyword.control.import.pmc", "match": "\\buse\\b" },
        {
          "match": "\\b(namespace)\\s+([A-Za-z_][A-Za-z0-9_]*)",
          "captures": {
            "1": { "name": "keyword.other.namespace.pmc" },
            "2": { "name": "entity.name.namespace.pmc" }
          }
        },
        { "name": "storage.modifier.pmc", "match": "\\bexport\\b" },
        { "name": "keyword.other.as.pmc", "match": "\\bas\\b" }
      ]
    },
    "commands": {
      "patterns": [
        { "name": "keyword.control.pmc", "match": "\\b(goto|check|halt)\\b" },
        { "name": "support.function.builtin.pmc", "match": "\\b(left|right|mark|unmark)\\b" },
        { "name": "keyword.other.debugger.pmc", "match": "\\bdebugger\\b" }
      ]
    },
    "call": {
      "match": "(@)\\s*([A-Za-z_][A-Za-z0-9_]*(?:\\s*::\\s*[A-Za-z_][A-Za-z0-9_]*)*)",
      "captures": {
        "1": { "name": "punctuation.definition.call.pmc" },
        "2": { "name": "entity.name.function.call.pmc" }
      }
    },
    "definition": {
      "match": "\\b([A-Za-z_][A-Za-z0-9_]*)\\s*(?=\\(\\)\\s*\\{)",
      "captures": { "1": { "name": "entity.name.function.pmc" } }
    },
    "label": {
      "match": "(?:^|(?<=[;{,:]))\\s*(\\d+)\\s*(:)",
      "captures": {
        "1": { "name": "entity.name.label.pmc" },
        "2": { "name": "punctuation.separator.label.pmc" }
      }
    },
    "number": { "name": "constant.numeric.pmc", "match": "\\b\\d+\\b" },
    "punctuation": {
      "patterns": [
        { "name": "punctuation.separator.namespace.pmc", "match": "::" },
        { "name": "keyword.operator.return.pmc", "match": "!" },
        { "name": "punctuation.terminator.pmc", "match": ";" },
        { "name": "punctuation.separator.comma.pmc", "match": "," }
      ]
    }
  }
}
```

**The drift guard** — a Rust test so `cargo test --workspace` keeps the single-source promise honest:

```rust
// crates/post-machine/tests/editor_grammar.rs
//! The shared TextMate grammar must stay valid JSON and cover exactly the
//! command vocabulary the parser reserves — a RESERVED change must touch
//! the grammar in the same commit.

#[test]
fn textmate_grammar_is_valid_and_covers_the_reserved_words() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../editors/grammars/pmc.tmLanguage.json"
    );
    let text = std::fs::read_to_string(path).expect("shared grammar exists");
    let json: serde_json::Value = serde_json::from_str(&text).expect("grammar is valid JSON");
    assert_eq!(json["scopeName"], "source.pmc");
    for word in mtc_post_machine::parser::RESERVED {
        assert!(text.contains(word), "grammar misses reserved word `{word}`");
    }
    for word in ["use", "namespace", "export", "as"] {
        assert!(text.contains(word), "grammar misses keyword `{word}`");
    }
}
```

**Steps:**
- [ ] Write the drift test first; run `cargo test -p mtc-post-machine --test editor_grammar` — fails (no file).
- [ ] Add `.gitignore` entries + the grammar file; test green.
- [ ] Eyeball the coloring manually once the VS Code shell exists (Task 2's checklist covers it); grammar fixes later are cosmetic-only by contract.
- [ ] Full Rust gates. Commit: `feat(editors): shared pmc TextMate grammar + reserved-word drift guard`

---

### Task 2: VS Code extension — language, client, settings, version check

**Files:**
- Create: `editors/vscode/package.json`, `language-configuration.json`, `tsconfig.json`, `scripts/copy-grammar.js`, `src/extension.ts`, `.gitignore` (`node_modules/`, `out/`, `syntaxes/`, `*.vsix` — belt and braces with the root one)

**`package.json`** (the load-bearing parts — fill standard fields around them):

```jsonc
{
  "name": "pmc",
  "displayName": "PMC (Post machine toolchain)",
  "description": "Language support for .pmc via pmt lsp: diagnostics, completions, navigation, formatting, semantic tokens.",
  "version": "0.1.0",
  "publisher": "mellonis",
  "license": "GPL-3.0-or-later",
  "engines": { "vscode": "^1.90.0" },
  "categories": ["Programming Languages"],
  "main": "./out/extension.js",
  "activationEvents": ["onLanguage:pmc", "onTaskType:pmt"],
  "contributes": {
    "languages": [{
      "id": "pmc", "extensions": [".pmc"], "aliases": ["PMC"],
      "configuration": "./language-configuration.json"
    }],
    "grammars": [{
      "language": "pmc", "scopeName": "source.pmc",
      "path": "./syntaxes/pmc.tmLanguage.json"
    }],
    "configuration": {
      "title": "pmt",
      "properties": {
        "pmt.path": {
          "type": "string", "default": "pmt",
          "description": "Path to the pmt binary."
        },
        "pmt.lint.allow": {
          "type": "array", "items": { "type": "string" }, "default": [],
          "description": "Lint codes to allow (merged by union with pmt.json)."
        }
      }
    },
    "problemMatchers": [{
      "name": "pmt",
      "owner": "pmt",
      "fileLocation": ["autoDetect", "${workspaceFolder}"],
      "severity": "warning",
      "pattern": {
        "regexp": "^(.+?):(\\d+):(\\d+): (error|warning|lint): (.+?)(?: \\[([a-z-]+)\\])?$",
        "file": 1, "line": 2, "column": 3, "severity": 4, "message": 5, "code": 6
      }
    }],
    "taskDefinitions": [{
      "type": "pmt",
      "required": ["command"],
      "properties": {
        "command": { "type": "string", "enum": ["compile", "lint", "fmt-check"] },
        "file": { "type": "string" }
      }
    }]
  },
  "scripts": {
    "copy-grammar": "node scripts/copy-grammar.js",
    "compile": "npm run copy-grammar && tsc -p .",
    "package": "npm run compile && vsce package"
  },
  "dependencies": { "vscode-languageclient": "^9.0.1" },
  "devDependencies": {
    "@types/node": "^20.0.0", "@types/vscode": "^1.90.0",
    "@vscode/vsce": "^3.0.0", "typescript": "^5.5.0"
  }
}
```

Problem-matcher notes: the regexp accepts the bracketed fatal-code suffix from day one (group 6, optional); the `lint:` prefix is not a recognized severity keyword, so those lines fall back to the matcher-level `"severity": "warning"` — exactly right, while explicit `error`/`warning` matches override it.

**`scripts/copy-grammar.js`** (the single-sourcing mechanism — `syntaxes/` is gitignored, created at build):

```js
const fs = require('fs'), path = require('path');
const src = path.join(__dirname, '..', '..', 'grammars', 'pmc.tmLanguage.json');
const dstDir = path.join(__dirname, '..', 'syntaxes');
fs.mkdirSync(dstDir, { recursive: true });
fs.copyFileSync(src, path.join(dstDir, 'pmc.tmLanguage.json'));
```

**`language-configuration.json`:** `comments: { lineComment: "//", blockComment: ["/*", "*/"] }`, brackets/autoclosing for `{}` and `()`.

**`src/extension.ts`:**

```ts
import * as vscode from 'vscode';
import { execFile } from 'child_process';
import {
  LanguageClient, LanguageClientOptions, ServerOptions,
} from 'vscode-languageclient/node';

const MIN_TESTED_PMT = '0.1.0';
let client: LanguageClient | undefined;

export async function activate(context: vscode.ExtensionContext) {
  const config = vscode.workspace.getConfiguration('pmt');
  const pmtPath = config.get<string>('path', 'pmt');
  checkVersion(pmtPath);

  const serverOptions: ServerOptions = { command: pmtPath, args: ['lsp'] };
  const clientOptions: LanguageClientOptions = {
    documentSelector: [{ language: 'pmc' }],
    // Forwards the whole `pmt` section as workspace/didChangeConfiguration
    // ({ settings: { pmt: {...} } }) at startup and live on change — the
    // server unwraps the `pmt` key.
    synchronize: { configurationSection: 'pmt' },
    initializationOptions: { lint: { allow: config.get<string[]>('lint.allow', []) } },
  };
  client = new LanguageClient('pmt', 'pmt lsp', serverOptions, clientOptions);
  await client.start();
  context.subscriptions.push(
    vscode.tasks.registerTaskProvider('pmt', new PmtTaskProvider(pmtPath)),
  );
}

export function deactivate(): Thenable<void> | undefined {
  return client?.stop();
}

function checkVersion(pmtPath: string) {
  execFile(pmtPath, ['--version'], (err, stdout) => {
    if (err) {
      vscode.window.showErrorMessage(
        `pmt not found at '${pmtPath}' — set pmt.path or install with ` +
        `'cargo install --path crates/post-machine'.`);
      return;
    }
    const found = /^pmt (\d+)\.(\d+)\.(\d+)/.exec(stdout);
    if (found && older(found.slice(1).map(Number), MIN_TESTED_PMT.split('.').map(Number))) {
      vscode.window.showWarningMessage(
        `pmt ${found[1]}.${found[2]}.${found[3]} is older than the tested ` +
        `${MIN_TESTED_PMT}; some features may misbehave — update pmt.`);
    }
  });
}
function older(a: number[], b: number[]): boolean {
  for (let i = 0; i < 3; i++) { if (a[i] !== b[i]) return a[i] < b[i]; }
  return false;
}

class PmtTaskProvider implements vscode.TaskProvider {
  constructor(private pmtPath: string) {}
  provideTasks(): vscode.Task[] {
    const doc = vscode.window.activeTextEditor?.document;
    if (!doc || doc.languageId !== 'pmc') { return []; }
    const file = doc.uri.fsPath;
    return [
      this.task('compile', ['compile', file], file),
      this.task('lint', ['lint', file], file),
      this.task('fmt-check', ['fmt', '--check', file], file),
    ];
  }
  resolveTask(task: vscode.Task): vscode.Task | undefined {
    const def = task.definition as { command: string; file?: string };
    const file = def.file ?? '${file}';
    const args = def.command === 'fmt-check' ? ['fmt', '--check', file] : [def.command, file];
    return new vscode.Task(def, vscode.TaskScope.Workspace, `pmt ${def.command}`, 'pmt',
      new vscode.ProcessExecution(this.pmtPath, args), '$pmt');
  }
  private task(command: string, args: string[], file: string): vscode.Task {
    const def: vscode.TaskDefinition = { type: 'pmt', command, file };
    return new vscode.Task(def, vscode.TaskScope.Workspace, `pmt ${command}`, 'pmt',
      new vscode.ProcessExecution(this.pmtPath, args), '$pmt');
  }
}
```

**Steps:**
- [ ] Scaffold the files; `npm install && npm run compile` clean.
- [ ] Launch the extension dev host (F5 with a standard `.vscode/launch.json`, committed) against a repo `.pmc` file; verify: static colors (grammar), squiggles on a bad file, completion after `@`, jump-to-def into the materialized std, quickfix on an unused label, format-on-save, the settings UI shows `pmt.path` + `pmt.lint.allow`, and changing the allow-list live re-publishes (no restart).
- [ ] `npm run package` produces `pmc-0.1.0.vsix` from a clean checkout.
- [ ] Rust gates still green (untouched). Commit: `feat(editors): vscode extension — pmc language, lsp client, settings, version check`

---

### Task 3: VS Code README + manual checklist

**Files:**
- Create: `editors/vscode/README.md`

Content (forge-agnostic):
- **Install the server**: `cargo install --path crates/post-machine` (or any release `pmt` on `PATH`); the extension launches `pmt lsp` via the `pmt.path` setting.
- **Build + sideload**: `npm install && npm run package`, then `code --install-extension pmc-0.1.0.vsix`.
- **Settings**: `pmt.path`, `pmt.lint.allow` (union-merged with `pmt.json` — pointer to the lint docs page by name).
- **Tasks**: the auto-provided file-scoped tasks (compile / lint / fmt-check) with the `$pmt` problem matcher; plus a ready-to-paste `tasks.json` snippet for a full pipeline the task provider deliberately does not generate (compile → link → run with a tape), each step wired to `"problemMatcher": "$pmt"` — include the concrete JSON in the README.
- **Tested `pmt` range**: `0.1.0` (the shell warns on older).
- **Manual test checklist** (the spec's bar — v1 has no automated editor e2e): open file → squiggles; type `@` → completion; jump-to-def incl. a `std::` target; apply a quickfix; format-on-save; run each provided task and confirm the problem matcher populates the Problems panel (including a fatal with its bracketed code).

**Steps:**
- [ ] Write the README; walk the checklist once end-to-end against the built `.vsix` and the workspace `pmt` binary; fix anything it surfaces.
- [ ] Commit: `docs(editors): vscode README — install, sideload, tasks, manual checklist`

---

### Task 4: JetBrains plugin — scaffold, file type, LSP4IJ client, TextMate grammar, version check

**Files:**
- Create: `editors/jetbrains/build.gradle.kts`, `settings.gradle.kts`, `gradle.properties`, gradle wrapper (`gradlew`, `gradlew.bat`, `gradle/wrapper/*`)
- Create: `src/main/resources/META-INF/plugin.xml`
- Create: `src/main/resources/textmate/pmc/package.json` (bundle **manifest** — points at the grammar copied in at build; not a grammar copy)
- Create: `src/main/kotlin/ru/mellonis/pmc/` — `PmcFileType.kt`, `PmtLanguageServerFactory.kt`, `PmcTextMateBundleProvider.kt`, `PmtVersionCheck.kt`

**`build.gradle.kts`** (pins as of plan time — verify current stables at implementation):

```kotlin
plugins {
    id("org.jetbrains.kotlin.jvm") version "2.1.0"
    id("org.jetbrains.intellij.platform") version "2.2.1"
}
group = "ru.mellonis"
version = "0.1.0"
repositories {
    mavenCentral()
    intellijPlatform { defaultRepositories() }
}
dependencies {
    intellijPlatform {
        intellijIdeaCommunity("2024.3")
        plugin("com.redhat.devtools.lsp4ij:0.9.0")   // verify the current LSP4IJ release
        bundledPlugin("org.jetbrains.plugins.textmate")
    }
}
// Single-sourcing: the shared grammar rides into the bundle dir at build.
tasks.processResources {
    from("../grammars") { include("pmc.tmLanguage.json"); into("textmate/pmc") }
}
```

**`plugin.xml`:**

```xml
<idea-plugin>
    <id>ru.mellonis.pmc</id>
    <name>PMC (Post machine toolchain)</name>
    <vendor>mellonis</vendor>
    <description>Language support for .pmc via pmt lsp (LSP4IJ): diagnostics,
        completions, navigation, formatting, semantic tokens; pmt run configurations.</description>
    <depends>com.intellij.modules.platform</depends>
    <depends>com.redhat.devtools.lsp4ij</depends>
    <depends>org.jetbrains.plugins.textmate</depends>
    <extensions defaultExtensionNs="com.intellij">
        <fileType name="PMC" implementationClass="ru.mellonis.pmc.PmcFileType"
                  fieldName="INSTANCE" extensions="pmc"/>
        <textmate.bundleProvider implementation="ru.mellonis.pmc.PmcTextMateBundleProvider"/>
    </extensions>
    <extensions defaultExtensionNs="com.redhat.devtools.lsp4ij">
        <server id="pmtLsp" name="pmt lsp"
                factoryClass="ru.mellonis.pmc.PmtLanguageServerFactory"/>
        <fileTypeMapping fileType="PMC" serverId="pmtLsp" languageId="pmc"/>
    </extensions>
</idea-plugin>
```

(The `textmate.bundleProvider` EP name and the LSP4IJ extension schema are the two spots most likely to have drifted — verify both against the installed plugin versions' own `plugin.xml`/docs while implementing; the *contract* is fixed: the grammar file registered is the build-time copy of the shared one, and LSP4IJ maps the PMC file type to `pmt lsp` with language id `pmc`.)

**Bundle manifest** `src/main/resources/textmate/pmc/package.json` (VSCode-style bundle the TextMate plugin reads):

```json
{
  "name": "pmc",
  "contributes": {
    "languages": [{ "id": "pmc", "extensions": [".pmc"] }],
    "grammars": [{ "language": "pmc", "scopeName": "source.pmc", "path": "./pmc.tmLanguage.json" }]
  }
}
```

**Kotlin sources** (shapes; adapt to the pinned LSP4IJ API):

```kotlin
// PmcFileType.kt
object PmcFileType : FileType {
    override fun getName() = "PMC"
    override fun getDescription() = "Post machine toolchain source"
    override fun getDefaultExtension() = "pmc"
    override fun getIcon() = AllIcons.FileTypes.Text
    override fun isBinary() = false
}

// PmtLanguageServerFactory.kt
class PmtLanguageServerFactory : LanguageServerFactory {
    override fun createConnectionProvider(project: Project): StreamConnectionProvider =
        PmtConnectionProvider()
}
class PmtConnectionProvider :
    ProcessStreamConnectionProvider(listOf(PmtSettings.instance.state.pmtPath, "lsp")) {
    override fun getInitializationOptions(rootUri: VirtualFile?): Any =
        mapOf("lint" to mapOf("allow" to PmtSettings.instance.state.lintAllow))
}

// PmcTextMateBundleProvider.kt — returns the bundled textmate/pmc dir
// (extracted from plugin resources to a temp dir on first call).

// PmtVersionCheck.kt — a ProjectActivity (postStartupActivity): run
// `pmt --version`, parse `pmt X.Y.Z`, compare to MIN_TESTED_PMT = "0.1.0";
// older → a warning Notification naming both versions and the fix;
// binary missing → an error Notification pointing at the settings page.
```

(`PmtSettings` arrives in Task 5 — for this task, stub it with the defaults so the factory compiles: path `pmt`, empty allow.)

**Steps:**
- [ ] Scaffold; `./gradlew buildPlugin` succeeds from clean.
- [ ] `./gradlew runIde` sandbox: install nothing else (LSP4IJ resolves as a declared plugin dependency in the sandbox); open a `.pmc` file → static TextMate colors, squiggles, completion, jump-to-def incl. std, quickfix, formatting (Code → Reformat), semantic tokens over the static colors.
- [ ] Rust gates untouched/green. Commit: `feat(editors): jetbrains plugin — pmc file type, lsp4ij client, textmate bundle, version check`

---

### Task 5: JetBrains settings page + run configurations

**Files:**
- Create: `src/main/kotlin/ru/mellonis/pmc/PmtSettings.kt`, `PmtSettingsConfigurable.kt`, `PmtRunConfigurationType.kt`, `PmtRunConfiguration.kt`, `PmtRunSettingsEditor.kt`; extend `plugin.xml`.

**Interfaces:**

```kotlin
// PmtSettings.kt — application-level persistent state:
@State(name = "PmtSettings", storages = [Storage("pmt.xml")])
class PmtSettings : PersistentStateComponent<PmtSettings.State> {
    data class State(
        var pmtPath: String = "pmt",
        var lintAllow: MutableList<String> = mutableListOf(),
    )
    companion object { val instance: PmtSettings get() = service() }
    // getState/loadState boilerplate
}

// PmtSettingsConfigurable.kt — the settings page: a text field for the
// binary path + a text field for the allow-list (comma-separated, split
// and trimmed into State.lintAllow). apply() persists, then pushes
// workspace/didChangeConfiguration with { "pmt": { "lint": { "allow": [...] } } }
// to the running pmtLsp server through LSP4IJ's server-management API;
// if the pinned LSP4IJ version exposes no notification hook, restart the
// server instead (LanguageServerManager stop/start) — the server re-reads
// initializationOptions on the new session. Either path re-publishes
// without user-visible loss.
```

```kotlin
// PmtRunConfigurationType: id "PmtRun", display "pmt", one factory.
// PmtRunConfiguration fields: subcommand preset (combo: compile | lint | run),
// arguments (raw string, appended verbatim), working directory.
// getState() → CommandLineState building GeneralCommandLine(
//   PmtSettings.instance.state.pmtPath, subcommand, *parsedArgs)
//   .withWorkDirectory(workingDir) — thin process wrappers, no
//   build-system ambitions. Output lands in the run console.
// PmtRunSettingsEditor: combo + two text fields.
```

`plugin.xml` additions:

```xml
<extensions defaultExtensionNs="com.intellij">
    <applicationService serviceImplementation="ru.mellonis.pmc.PmtSettings"/>
    <applicationConfigurable groupId="tools" id="ru.mellonis.pmc.settings"
        displayName="pmt" instance="ru.mellonis.pmc.PmtSettingsConfigurable"/>
    <configurationType implementation="ru.mellonis.pmc.PmtRunConfigurationType"/>
</extensions>
```

**Steps:**
- [ ] Implement; `./gradlew buildPlugin` clean.
- [ ] `runIde` verification: the settings page holds path + allow-list; adding a lint code to the allow-list makes the corresponding squiggle disappear live (or after the automatic server restart — whichever mechanism landed); a `pmt run` run-configuration executes against a compiled `.pmx` and shows output + exit code in the console.
- [ ] Commit: `feat(editors): jetbrains settings page (binary path + lint allow) and pmt run configurations`

---

### Task 6: JetBrains README + final acceptance sweep

**Files:**
- Create: `editors/jetbrains/README.md`

README content (forge-agnostic): install the server (`cargo install --path crates/post-machine`); **install LSP4IJ from the JetBrains Marketplace first** (a sideloaded plugin does not auto-install its plugin dependencies); build (`./gradlew buildPlugin`) and sideload the zip from `build/distributions/` (Settings → Plugins → ⚙ → Install Plugin from Disk); works on Community editions; the settings page; run configurations; tested `pmt` range `0.1.0`; the manual test checklist (same shape as VS Code's: open → squiggles, completion, jump-to-def incl. std, quickfix, reformat, run-config smoke).

**Final acceptance sweep (spec Acceptance criteria, shell rows):**
- [ ] From a clean checkout: `npm install && npm run package` (vscode) and `./gradlew buildPlugin` (jetbrains) both produce artifacts.
- [ ] Sideload both against the SAME workspace `pmt` binary; walk both READMEs' checklists end-to-end.
- [ ] The dogfood file: open the repo's `std.pmc` in each editor → zero diagnostics, semantic tokens present, format is a no-op.
- [ ] Configuration end-to-end in at least one editor: a `pmt.json` allow-list suppresses a finding in both `pmt lint` and the editor; changing the IDE setting re-publishes without restart; editing `pmt.json` on disk re-publishes via the file watch.
- [ ] Rust gates green (nothing in the Rust tree changed since Task 1).
- [ ] Commit: `docs(editors): jetbrains README + manual checklists; shells complete`

---

## Release note (for the eventual v-bump, recorded here for the ledger)

Version block: **toolchain moved** (the `pmt lsp` release); `.pmc` language `0.2` unchanged; PM-1 `.pma` dialect unchanged; `IR_VERSION` 3 unchanged; MO/MX/MT containers unchanged. Shells version independently (`0.1.0` each), ship as sideloadable artifacts attached to the GitHub release; each README states its tested `pmt` range. Fatal error codes became stable identifiers in plan 2's release.
