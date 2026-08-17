# PMC — Post-machine toolchain support for JetBrains IDEs

Language support for `.pmc`, the C-like source language of the Post-machine
toolchain in this repository, and `.pma`, its PM-1 assembly dialect. This
plugin is a thin client on top of
[LSP4IJ](https://plugins.jetbrains.com/plugin/23257-lsp4ij): it launches
`pmt lsp` and renders whatever the server reports — diagnostics,
completions, hover, go-to-definition, quickfixes, semantic tokens,
document symbols, and formatting — over the standard Language Server
Protocol. Nothing here is a reimplementation; every answer comes from the
same compiler, assembler, linter, and formatter the `pmt` command-line
tool uses. Syntax coloring comes from a bundled TextMate grammar, shared
byte-for-byte with the VS Code extension. Debugging rides the same
thin-client pattern: the plugin registers `pmt dap` as an LSP4IJ debug
adapter server, and the IDE's own debugger UI (gutter breakpoints,
stepping, variables) talks to it over the Debug Adapter Protocol.
`.pma` support is syntax highlighting plus the full `pmt lsp` surface
plus run configurations and breakpoints — see the `.pma` checklist below
for its own manual walkthrough.

## Requirements

- A `pmt` binary reachable on `PATH`, or pointed to with the settings
  page's binary-path field (below).
- **LSP4IJ**, installed from the JetBrains Marketplace *before* you
  sideload this plugin (below) — a sideloaded plugin does not auto-install
  its own plugin dependencies, so skipping this step leaves the IDE unable
  to load the plugin at all.
- This plugin is version 0.2.0. It has been tested against `pmt` 0.3.0; on
  startup it runs `pmt --version` and shows a warning notification (not a
  hard failure) if the binary reports something older, or an error
  notification if the binary can't be found at all. The plugin's own
  version number and the tested `pmt` version are independent numbers.
- Debugging specifically needs a `pmt` with the `dap` subcommand, which
  postdates the 0.3.0 release — until the next release, build the binary
  from current sources (next section) rather than from the release tag.
- Built and verified against **LSP4IJ 0.20.1** on an IntelliJ Platform
  2024.3 baseline (IntelliJ IDEA Community works — no Ultimate-only APIs
  are used).

## Install the server

Build `pmt` from this repository and put it on `PATH`:

```sh
cargo install --path crates/post-machine
```

Any released `pmt` binary already on `PATH` works too — the plugin only
shells out to it; it never bundles or builds one itself. To point at a
binary that isn't on `PATH`, set the binary path in Settings | Tools | pmt
(below).

Note for macOS: an IDE launched from the Dock or JetBrains Toolbox may
not inherit your shell's `PATH` (symptom: a `Cannot run program "pmt"`
error on opening a `.pmc` file, alongside the plugin's own "pmt not
found" notification). Set the absolute path — e.g.
`~/.cargo/bin/pmt` — in Settings | Tools | pmt instead of relying on
`PATH`, then restart the IDE.

## Install LSP4IJ first

This plugin depends on **LSP4IJ** ("LSP4IJ" by Red Hat, plugin id
`com.redhat.devtools.lsp4ij`) to speak the Language Server Protocol —
Settings → Plugins → Marketplace, search "LSP4IJ", Install, then restart
the IDE if prompted. Do this *before* sideloading the plugin below: a
sideloaded plugin is installed from a local file, not from the
Marketplace, so the IDE has no opportunity to resolve and auto-install a
declared plugin dependency the way it would for a Marketplace install.
Skipping this step leaves this plugin disabled with an unsatisfied-
dependency error. The shipped build was verified against LSP4IJ 0.20.1;
a newer 0.x/1.x release should work unless its own compatibility range
excludes this plugin's IntelliJ Platform baseline (2024.3).

## Build and sideload the plugin

From `editors/jetbrains-pm`, with `JAVA_HOME` pointed at a JDK capable of
running Gradle itself — a JetBrains IDE's own bundled JBR works, e.g. on
macOS:

```sh
export JAVA_HOME="$HOME/Applications/<SomeIDE>.app/Contents/jbr/Contents/Home"
./gradlew buildPlugin
```

(Substitute the `.app` for whichever JetBrains IDE Toolbox installed —
for example `RustRover.app`; the bundled JBR is just a JDK most
JetBrains-IDE users already have on disk without a separate install.)
The compilation itself always targets a pinned JDK 17 toolchain
(`kotlin { jvmToolchain(17) }` in `build.gradle.kts`), regardless of
which JDK `JAVA_HOME` points at — Gradle auto-provisions JDK 17 via the
`foojay-resolver-convention` plugin (`settings.gradle.kts`) if the
`JAVA_HOME` JDK doesn't already supply one. This has been verified
building under a JetBrains-bundled JBR newer than 17 (JBR 25); other
JDKs as `JAVA_HOME` are untested.

`buildPlugin` produces `build/distributions/pmc-0.2.0.zip`. Install it:

1. Settings → Plugins → the ⚙ (gear) icon in the top-right of the
   Plugins page → **Install Plugin from Disk…**
2. Pick `build/distributions/pmc-0.2.0.zip`.
3. Restart the IDE when prompted.

This works on Community editions — the plugin is built against the
IntelliJ Platform Community baseline and uses no Ultimate-only APIs.

## Settings

**Settings | Tools | pmt** holds two fields:

| Field | Default | Meaning |
|---|---|---|
| pmt binary path | `pmt` | Path (or bare command resolved on `PATH`) to the `pmt` binary. The plugin launches it as `pmt lsp` for the language server, and reuses the same path for run configurations, target listing, and the `pmt dap` debug adapter (below). |
| Lint allow-list (comma-separated) | *(empty)* | Lint codes to suppress, forwarded to the server and kept live as you edit the setting — no IDE or server restart needed. This list is union-merged with any `pmt.json` project file the server discovers for the open document — either source suppressing a code is enough to suppress it, and neither can un-suppress a code the other disables. See `docs/pmt/lint.md` in this repository for the rule catalog and the `pmt.json` schema. |

Changing the allow-list and applying the settings page pushes the new list
straight to every already-running `pmt lsp` server (one per open project)
over the standard LSP configuration-change notification; each server
re-publishes diagnostics for its open documents immediately, so a
previously-squiggled suppressed code clears without reopening the file or
restarting anything.

The binary path is different: the language server reads it only when a
`pmtLsp` process starts, so editing it and applying the settings page has
no effect on an already-running server (restart the language server for
the project, or restart the IDE, to pick up a new path there). Run
configurations read the current path fresh on every run, so no restart is
needed for those.

## Run configurations

**Run → Edit Configurations… → +  → pmt** adds a thin `pmt <subcommand>`
process wrapper — no build-system ambitions (no compile-before-run graph,
no artifact tracking):

| Field | Meaning |
|---|---|
| Subcommand | One of `build`, `compile`, `lint`, `run`, selected from a fixed dropdown. |
| Target | `build` only: a target name from the `pmt.json` project manifest. Editable combo; the **Refresh** button fills it by running `pmt build --list-targets` in the working directory (the driver's own nearest-ancestor manifest discovery starts there), and the status line under it reports how many targets were found and how many declare a run block. Leave blank to build source files passed via Arguments instead (the driver's argv mode). |
| Run the target after building | `build` only: adds `--run`, so the built target immediately runs against its manifest-declared tape. Manifest-mode only — the driver rejects `--run` when sources are passed instead of a target, with its own message. |
| Arguments | Free-form, shell-quoting-aware (parsed like a program-arguments field, so quoted strings and spaces behave as expected) — appended after the subcommand (and, for `build`, after the target) verbatim. |
| Working directory | Defaults to the project's base directory. |

Output streams to the Run tool window's console, including the process's
exit code on completion.

The `build` subcommand is the manifest-backed way to produce (and with
`--run`, immediately execute) a `.pmx`: one configuration per target
replaces the previous README revision's Shell-Script recipe. The dropdown
still does not offer `link` — a hand-rolled compile-then-link pipeline is
exactly what a `pmt.json` target already models (`docs/pmt/project.md`),
so declare one and `build` it; failing that, run `pmt link` from a
terminal (same scope line VS Code's task provider draws: see
`editors/vscode-pm/README.md`'s "Custom pipelines" section for the
equivalent gap there).

## Debugging

The plugin registers **pmt dap** as a debug adapter server with LSP4IJ's
DAP support, which bridges it into the IDE debugger: breakpoints in the
gutter of `.pmc`/`.pma` files, stepping, pause, threads/variables views.
The adapter is the same editor-agnostic `pmt dap` stdio server the VS
Code extension uses; what the machine-level debug surface looks like
(registers, tape scopes, stepping granularity, disassembly) is documented
in this repository's `docs/dap.md`.

To debug:

1. Set a breakpoint in a `.pmc` (or `.pma`) file.
2. **Run → Edit Configurations… → + → Debug Adapter Protocol → DAP**,
   pick server **pmt dap** on the *Server* tab. The *Command* field may
   be left blank — the plugin then launches `<binary path from the
   settings page> dap`. Set the working directory to the directory
   holding your `pmt.json` (defaults to the project root).
3. On the *Configuration* tab pick one of the two launch templates and
   edit it in place:
   - **pmt: launch target** — `"target"` names a manifest target, built
     in process with debug info forced on (the same driver `pmt build`
     uses), then debugged. The manifest is discovered walking up from
     the working directory; an optional `"project"` key overrides the
     walk's starting point.
   - **pmt: launch program** — `"program"` points at a prebuilt `.pmx`,
     used as-is (build it with debug info — `pmt build --debug`, or
     `pmt compile -g` + `pmt link` — or source breakpoints answer
     unverified and stepping only stops at function boundaries);
     `"tape"` optionally points at a `.pmt` tape snapshot (omit for the
     empty tape). `${workspaceFolder}` in these paths resolves to the
     configuration's working directory.
   - Both shapes accept `"stopOnEntry"` (break before the first
     instruction) and `"trace"` (per-instruction trace lines in the
     debug console, matching `pmt run --trace`).
4. Start the configuration with the **Debug** action.

The two launch templates carry inert `type`/`request`/`name` keys so the
JSON is copy-pasteable to and from a VS Code `launch.json` — the adapter
reads only its own arguments.

## Manifest validation

Since plugin 0.2.0 the bundled `pmt.json` JSON Schema (Draft-07, single-
sourced from `editors/schemas/`) is contributed automatically: any file
named `pmt.json` validates and completes against it, and the schema shows
in the editor's bottom-right schema switcher. The contribution sits
behind an optional dependency on the IDE's JSON plugin, which every
JetBrains IDE bundles; if you had added the manual mapping a previous
README revision described (**Settings → Languages & Frameworks → Schemas
and DTDs → JSON Schema Mappings**), remove it — two mappings for the same
file make the IDE ask which to use.

## Manual test checklist

v1 has no automated editor end-to-end test — walk this by hand against a
sideloaded plugin and a `pmt` on `PATH`, after any change that touches the
client or the server's editor-facing surface. This mirrors the VS Code
README's checklist shape and scratch file — sideloading both shells
against the same workspace `pmt` binary and walking both checklists with
the same file is a reasonable single pass.

Create a scratch file, e.g. `check.pmc`:

```pmc
use std::goToEnd;

main() {
    @goToEnd();
    right;
    debugger;
    check(1,2);
 1: right;
 2: halt;
}
```

- [x] **Coloring — check this FIRST.** Open `check.pmc`. Confirm syntax
      colors appear (keywords, the `@` call, the label, the comment
      punctuation if you add one) rather than plain uncolored text. This
      is the one part of the shell that was compile-verified only, never
      observed at runtime: a plugin-registered file type otherwise
      disables TextMate's own coloring for the same extension, and the
      `editorHighlighterProvider` bridge in `plugin.xml` that restores it
      has not been exercised in a running IDE. If colors are missing,
      that bridge needs attention before anything below is worth testing
      — see the comment next to `editorHighlighterProvider` in
      `src/main/resources/META-INF/plugin.xml`.
- [x] **Squiggles**: confirm a warning underline on the `debugger;` line
      (the `leftover-debugger` lint finding) — diagnostics are live on
      open, no manual trigger needed.
- [x] **Completion**: on a new line inside `main`, type `@g`. Confirm
      `goToEnd` and `std::goToEnd` appear in the completion popup. (A bare
      `@` with nothing typed after it is itself a lexical error and won't
      show candidates — type at least one more character. If the list
      still doesn't appear, invoke Code Completion manually — the editor
      may have cached the empty result from the moment right after `@`.)
      After observing the popup, **undo the typed text** to restore a
      parse-clean state before continuing — only completions tolerate a
      broken parse; the following steps need valid syntax.
- [x] **Go-to-definition**: invoke it (Go to Declaration) on `goToEnd`,
      either in `use std::goToEnd;` or inside the `@goToEnd()` call.
      Confirm it jumps into a materialized copy of the standard library —
      a cached `std.pmc` outside this project, not a file you're editing —
      landing on `export goToEnd() {`. See `docs/lsp.md` in this
      repository for where that cache lives.
- [ ] **Hover**: hover over `goToEnd`, either in `use std::goToEnd;` or
      inside `@goToEnd();`. Confirm a tooltip appears with the routine's
      documentation text ("Moves the head to the last mark of the
      section it starts on…") — this is a `std::` call, so the text
      comes from the embedded standard library's own analysis, not this
      file's (`docs/lsp.md`, "Hover"). Then temporarily add a deprecated
      function above `main`, plus a call to it inside `main`:
      ```pmc
      ? Old helper, kept for the walk.
      ! [deprecated] use goToEnd instead.
      old() { right; }
      ```
      (`@old();` as a new line inside `main`). Confirm hovering `old` —
      its declaration, or the new `@old();` call site — shows a
      `deprecated: use goToEnd instead.` line under the doc text, and
      that the `@old();` call site itself renders struck through (the
      `deprecated-call` lint finding's tag). Undo both edits to restore
      the base scratch file before continuing.
- [ ] **Semantic colors**: in a `.pmc` file, function names render colored
      (not underline-only) under both light and dark themes.
- [x] **Quickfix**: on the `debugger;` squiggle, open the intention menu
      (Alt+Enter / ⌥Enter) and apply the fix. This one is gated
      (equivalent to `pmt lint --fix --force`), so it may show as a
      secondary, not the single default action — confirm the
      `debugger;` statement is deleted either way. **Undo** the fix
      afterward to restore `debugger;` — the allow-list step below needs
      the finding present again.
- [x] **Reformat**: Code → Reformat Code. Confirm `check(1,2)` becomes
      `check(1, 2)` and nothing else changes — formatting is layout-only,
      and it won't touch the (now-restored) `debugger;` line.
- [x] **Settings allow-list live-suppress**: open Settings | Tools | pmt,
      add `leftover-debugger` to the lint allow-list, and apply. Confirm
      the squiggle on `debugger;` disappears in the still-open file
      *without* reopening it or restarting the IDE. Remove
      `leftover-debugger` from the allow-list and apply again to confirm
      the squiggle comes back, then leave the field empty (the default)
      when you're done.
- [x] **Config file-watch — `pmt.json` on disk**: with `check.pmc` still
      open and `debugger;` squiggled again (previous step left it that
      way), create a `pmt.json` file next to it containing
      `{"lint": {"allow": ["leftover-debugger"]}}` (schema: `docs/pmt/lint.md`
      in this repository). Confirm the squiggle disappears **without
      touching Settings | Tools | pmt** — this exercises LSP4IJ's support
      for the server's `workspace/didChangeWatchedFiles` registration,
      which is unconfirmed for LSP4IJ 0.20.1 and is exactly what this
      walk needs to probe. If nothing happens on the disk edit, the
      fallback path is the server's own mtime re-check on the next edit
      keystroke inside `check.pmc` — type and undo a character to force
      one and see whether the squiggle clears then; if it still doesn't,
      **record the failure** (which of the two paths didn't fire) rather
      than silently moving on. Delete `pmt.json` and confirm the squiggle
      returns before continuing.
- [ ] **Run-config smoke**: produce a `.pmx` first — from a terminal (or
      a `compile`-subcommand run configuration plus a terminal `pmt link`
      call; the manifest-backed `build` subcommand gets its own checklist
      below), e.g.:
      ```sh
      pmt compile check.pmc -o check.pmo
      pmt link check.pmo -o check.pmx
      ```
      Then add a `pmt` run configuration with subcommand `run` and
      arguments `check.pmx --tape-cells " * *"`, and run it. Confirm the
      console shows the run's tape output and the process's exit code (0 =
      the program executed `stp`, 2 = `hlt`, 3 = a trap — see
      `docs/pmt/cli.md`).
      ```text
      /Users/mellonis/.cargo/bin/pmt run check.pmx --tape-cells " * *"
      outcome: Halted
      steps 12, core tacts 32, stall tacts 8 (total 40)
      origin 1, head 2 reads ' '
      |* *|

      Process finished with exit code 2
      ```
- [x] **Dogfood — the embedded standard library**: open
      `crates/post-machine/src/stdlib/std.pmc` from this repository
      directly (not the go-to-definition-materialized cache copy from
      earlier). Confirm **zero diagnostics**, that semantic tokens are
      visible (coloring beyond what the bundled TextMate grammar alone
      gave `check.pmc`), and that **Reformat Code** (Code → Reformat
      Code) is a **no-op** — no diff, no dirty-buffer indicator — the
      checked-in file is already canonically formatted. This is the
      editor-observed half of the dogfood check that `cargo test -p
      mtc-post-machine --lib lsp` already covers on the server side
      alone.

### `.pma` checklist

`pmt lsp` serves `.pma`, the PM-1 assembly dialect, through the same
process and connection as `.pmc` above (`docs/lsp.md`, "Languages") — walk
this checklist in the same IDE session as the `.pmc` one above, without
restarting the IDE, so the last step below has something to confirm. The
`PmaFileType`/`editorHighlighterProvider`/`fileTypeMapping` wiring in
`plugin.xml` is new and, like `.pmc`'s coloring bridge above, has only
been compile-verified, never observed at runtime — check coloring first.

Create a second scratch file, e.g. `check.pma`:

```pma
.func goToEnd
L1: rgt
    jm L1
    lft
    ret

.func main
    call goToEnd
UNUSED: nop
    rgt
    wr 1
    stp
```

- [x] **Coloring — check this FIRST.** Open `check.pma`. Confirm syntax
      colors appear (the `.func` directive, mnemonics, the `L1`/`UNUSED`
      labels, a `;` comment if you add one) rather than plain uncolored
      text — the same `editorHighlighterProvider`-restores-TextMate bridge
      as `.pmc`, now wired for the `PMA` file type too. If colors are
      missing, that bridge needs attention before anything below is worth
      testing.
- [x] **Typo mnemonic**: change `jm L1` to `jpm L1`. Confirm an error
      underline on `jpm` carrying the `unknown-mnemonic` code. **Undo**
      the typo back to `jm L1` before continuing — per `docs/lsp.md`, a
      fatal error hides lint findings entirely on the `.pma` side (no
      separate compile-warning channel), so the next step needs a clean
      assemble to have anything to show.
- [x] **Unused label + quickfix**: confirm a warning underline on the
      `UNUSED:` label (the `unused-label` lint finding). Open the
      intention menu (Alt+Enter / ⌥Enter) and apply the fix — unlike
      `.pmc`'s gated `leftover-debugger` fix, this one is machine-
      applicable, so it should be the single default action. Confirm only
      the `UNUSED:` label is removed, leaving `nop` behind (and the
      warning disappears).
- [x] **Go-to-definition**: invoke it (Go to Declaration) on the `L1`
      operand in `jm L1` (inside `goToEnd`). Confirm it jumps to the `L1:`
      label definition on the line directly above, in the same file —
      `.pma` has no external/materialized target the way `.pmc`'s
      `std::` calls do. Known limitation: the Cmd/Ctrl+hover underline
      may span the whole document rather than just the identifier — the
      server sends a word-precise origin span, but LSP4IJ does not yet
      use it for the underline on TextMate-backed file types (reported
      upstream). The jump itself landing on the definition is what this
      item verifies.
- [x] **Outline**: open the Structure tool window (⌘7 / Alt+7, or **File
      Structure…**). Confirm it shows `goToEnd` and `main` as functions,
      each containing its labels as children (`L1` under `goToEnd`;
      `UNUSED` under `main`, until the previous step deleted it).
- [x] **Reformat**: Code → Reformat Code. Confirm the file snaps to the
      canonical column grid — labels at column 0, mnemonics at column 8,
      operands at column 16 (`docs/formats.md`, "assembly text") — turning
      the scratch file's loose indentation into aligned columns.
- [x] **Raw-line paste**: replace the `stp` line with this
      `pmt dis --listing`-shaped row (address, raw hex bytes, resolved
      call target — not reassembleable input):
      ```
        0004:  21 05 00 00 00  call    0x0005 <goToEnd>
      ```
      Confirm a fatal error with the `raw-line` code — the line isn't
      assembly-shaped at all. Undo the paste to restore `stp`.
- [x] **`.pmc` still works**: switch back to (or reopen) `check.pmc` from
      the checklist above, still in this same project/session. Confirm
      its diagnostics (the `leftover-debugger` squiggle) are still live —
      opening and editing `.pma` documents never perturbed the `.pmc`
      service, per `docs/lsp.md`'s "one process, two independent language
      services."

### Target-build & debug checklist

New in plugin 0.2.0, never yet observed at runtime — the run-config
`build` subcommand and the whole DAP bridge below are compile-verified
only. Needs a `pmt` built from current sources (the `dap` subcommand
postdates 0.3.0). In the scratch project from the `.pmc` checklist above,
add a `pmt.json` next to `check.pmc`:

```json
{
  "project": {
    "targets": {
      "check": {
        "sources": ["check.pmc"],
        "run": { "tape": " * *" }
      }
    }
  }
}
```

(Schema reference: `docs/pmt/project.md`.)

- [ ] **Manifest schema**: with the `pmt.json` from above open, confirm
      the bottom-right schema widget shows the bundled `pmt` schema and
      that an unknown key (e.g. `"bogus": 1` inside the target) gets a
      validation warning; remove it after.
- [ ] **Target listing**: add a `pmt` run configuration, subcommand
      `build`. Press **Refresh** next to the Target field — the combo
      fills with `check` and the status line reports one target. Break
      the manifest (temporarily rename the `sources` key), refresh again,
      and confirm the driver's own error lands in the status line instead
      of the list silently emptying; restore the manifest.
- [ ] **Build**: select target `check` and run the configuration.
      Confirm the console shows the driver's build output and exit code 0,
      and a `.pmx` appears where the manifest's output settings put it.
- [ ] **Build --run**: check *Run the target after building* and run
      again. Confirm the build is followed by the run's tape output in
      the same console session.
- [ ] **Breakpoint + launch target**: set a gutter breakpoint on an
      executable `.pmc` line in `check.pmc`. Create the DAP run
      configuration per the Debugging section (server **pmt dap**,
      command blank, template *pmt: launch target* with `"target":
      "check"`), start it with Debug, and confirm the session stops on
      the breakpoint with the editor line highlighted.
- [ ] **Machine state**: while stopped, confirm the variables view shows
      the machine scopes `docs/dap.md` describes (registers; tape), and
      that stepping advances the highlighted line.
- [ ] **stopOnEntry + program mode**: switch the launch JSON to the
      *pmt: launch program* template pointing at the `.pmx` built above
      (keep `"stopOnEntry": true`, drop `"tape"` for the empty-tape
      default), restart, and confirm the session stops before the first
      instruction.
- [ ] **Blank-command fallback**: confirm the *Command* field of the DAP
      configuration is still blank and the session nevertheless launched
      the settings-page binary (the fallback documented in the Debugging
      section) — then set an explicit command `pmt dap` and confirm that
      still works too.

## License

GPL-3.0-or-later, same as the rest of this repository (see `LICENSE`).
