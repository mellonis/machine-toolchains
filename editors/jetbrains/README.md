# PMC — Post-machine toolchain support for JetBrains IDEs

Language support for `.pmc`, the C-like source language of the Post-machine
toolchain in this repository. This plugin is a thin client on top of
[LSP4IJ](https://plugins.jetbrains.com/plugin/23257-lsp4ij): it launches
`pmt lsp` and renders whatever the server reports — diagnostics,
completions, go-to-definition, quickfixes, semantic tokens, document
symbols, and formatting — over the standard Language Server Protocol.
Nothing here is a reimplementation; every answer comes from the same
compiler, linter, and formatter the `pmt` command-line tool uses. Syntax
coloring comes from a bundled TextMate grammar, shared byte-for-byte with
the VS Code extension.

## Requirements

- A `pmt` binary reachable on `PATH`, or pointed to with the settings
  page's binary-path field (below).
- **LSP4IJ**, installed from the JetBrains Marketplace *before* you
  sideload this plugin (below) — a sideloaded plugin does not auto-install
  its own plugin dependencies, so skipping this step leaves the IDE unable
  to load the plugin at all.
- This plugin is version 0.1.0. It has been tested against `pmt` 0.1.0; on
  startup it runs `pmt --version` and shows a warning notification (not a
  hard failure) if the binary reports something older, or an error
  notification if the binary can't be found at all. The plugin's own
  version number and the tested `pmt` version are independent numbers
  that happen to both read 0.1.0 today.
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

From `editors/jetbrains`, with `JAVA_HOME` pointed at any JDK 17+ — a
JetBrains IDE's own bundled JBR works, e.g. on macOS:

```sh
export JAVA_HOME="$HOME/Applications/<SomeIDE>.app/Contents/jbr/Contents/Home"
./gradlew buildPlugin
```

(Substitute the `.app` for whichever JetBrains IDE Toolbox installed —
for example `RustRover.app`. Any JDK 17 or newer on `PATH`/`JAVA_HOME`
works equally well; the bundled JBR is just a JDK most JetBrains-IDE
users already have on disk without a separate install.)

`buildPlugin` produces `build/distributions/pmc-0.1.0.zip`. Install it:

1. Settings → Plugins → the ⚙ (gear) icon in the top-right of the
   Plugins page → **Install Plugin from Disk…**
2. Pick `build/distributions/pmc-0.1.0.zip`.
3. Restart the IDE when prompted.

This works on Community editions — the plugin is built against the
IntelliJ Platform Community baseline and uses no Ultimate-only APIs.

## Settings

**Settings | Tools | pmt** holds two fields:

| Field | Default | Meaning |
|---|---|---|
| pmt binary path | `pmt` | Path (or bare command resolved on `PATH`) to the `pmt` binary. The plugin launches it as `pmt lsp` for the language server, and reuses the same path for run configurations (below). |
| Lint allow-list (comma-separated) | *(empty)* | Lint codes to suppress, forwarded to the server and kept live as you edit the setting — no IDE or server restart needed. This list is union-merged with any `pmt.json` project file the server discovers for the open document — either source suppressing a code is enough to suppress it, and neither can un-suppress a code the other disables. See `docs/lint.md` in this repository for the rule catalog and the `pmt.json` schema. |

Changing the allow-list and applying the settings page pushes the new list
straight to every already-running `pmt lsp` server (one per open project)
over the standard LSP configuration-change notification; each server
re-publishes diagnostics for its open documents immediately, so a
previously-squiggled suppressed code clears without reopening the file or
restarting anything.

## Run configurations

**Run → Edit Configurations… → +  → pmt** adds a thin `pmt <subcommand>`
process wrapper — no build-system ambitions (no compile-before-run graph,
no artifact tracking):

| Field | Meaning |
|---|---|
| Subcommand | One of `compile`, `lint`, `run`, selected from a fixed dropdown. |
| Arguments | Free-form, shell-quoting-aware (parsed like a program-arguments field, so quoted strings and spaces behave as expected) — appended after the subcommand verbatim. |
| Working directory | Defaults to the project's base directory. |

Output streams to the Run tool window's console, including the process's
exit code on completion.

The dropdown deliberately does not offer `link` — building a runnable
`.pmx` needs a `pmt compile` step followed by a `pmt link` step, and this
run-configuration type doesn't model a multi-step pipeline (same scope
line VS Code's task provider draws: see `editors/vscode/README.md`'s "full
build-and-run pipeline" section for the equivalent gap there). Produce a
`.pmo`/`.pmx` with `pmt compile`/`pmt link` from a terminal (or a
`compile`-subcommand run configuration for the compile half), then point a
`run`-subcommand configuration at the resulting `.pmx`.

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

- [ ] **Coloring — check this FIRST.** Open `check.pmc`. Confirm syntax
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
- [ ] **Squiggles**: confirm a warning underline on the `debugger;` line
      (the `leftover-debugger` lint finding) — diagnostics are live on
      open, no manual trigger needed.
- [ ] **Completion**: on a new line inside `main`, type `@g`. Confirm
      `goToEnd` and `std::goToEnd` appear in the completion popup. (A bare
      `@` with nothing typed after it is itself a lexical error and won't
      show candidates — type at least one more character. If the list
      still doesn't appear, invoke Code Completion manually — the editor
      may have cached the empty result from the moment right after `@`.)
      After observing the popup, **undo the typed text** to restore a
      parse-clean state before continuing — only completions tolerate a
      broken parse; the following steps need valid syntax.
- [ ] **Go-to-definition**: invoke it (Go to Declaration) on `goToEnd`,
      either in `use std::goToEnd;` or inside the `@goToEnd()` call.
      Confirm it jumps into a materialized copy of the standard library —
      a cached `std.pmc` outside this project, not a file you're editing —
      landing on `export goToEnd() {`. See `docs/lsp.md` in this
      repository for where that cache lives.
- [ ] **Quickfix**: on the `debugger;` squiggle, open the intention menu
      (Alt+Enter / ⌥Enter) and apply the fix. This one is gated
      (equivalent to `pmt lint --fix --force`), so it may show as a
      secondary, not the single default action — confirm the
      `debugger;` statement is deleted either way. **Undo** the fix
      afterward to restore `debugger;` — the allow-list step below needs
      the finding present again.
- [ ] **Reformat**: Code → Reformat Code. Confirm `check(1,2)` becomes
      `check(1, 2)` and nothing else changes — formatting is layout-only,
      and it won't touch the (now-restored) `debugger;` line.
- [ ] **Settings allow-list live-suppress**: open Settings | Tools | pmt,
      add `leftover-debugger` to the lint allow-list, and apply. Confirm
      the squiggle on `debugger;` disappears in the still-open file
      *without* reopening it or restarting the IDE. Remove
      `leftover-debugger` from the allow-list and apply again to confirm
      the squiggle comes back, then leave the field empty (the default)
      when you're done.
- [ ] **Run-config smoke**: produce a `.pmx` first — from a terminal (or
      a `compile`-subcommand run configuration plus a terminal `pmt link`
      call, per the run-configurations gap noted above), e.g.:
      ```sh
      pmt compile check.pmc -o check.pmo
      pmt link check.pmo -o check.pmx
      ```
      Then add a `pmt` run configuration with subcommand `run` and
      arguments `check.pmx --tape " * *"`, and run it. Confirm the console
      shows the run's tape output and the process's exit code (0 = the
      program executed `stp`, 2 = `hlt`, 3 = a trap — see `docs/cli.md`).

## License

GPL-3.0-or-later, same as the rest of this repository (see `LICENSE`).
