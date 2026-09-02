# TMC — Turing-machine toolchain support for JetBrains IDEs

Language support for `.tmc`, the source language of the TM-1 Turing-machine
toolchain in this repository, and `.tma`, its TM-1 assembly dialect. This
plugin is a thin client on top of
[LSP4IJ](https://plugins.jetbrains.com/plugin/23257-lsp4ij): it launches
`tmt lsp` and renders whatever the server reports — diagnostics,
completions, hover, go-to-definition, quickfixes, semantic tokens, document
symbols, and formatting — over the standard Language Server Protocol.
Nothing here is a reimplementation; every answer comes from the same
compiler, assembler, linter, and formatter the `tmt` command-line tool
uses. Syntax coloring comes from bundled TextMate grammars, shared
byte-for-byte with the VS Code extension. Debugging rides the same
thin-client pattern: the plugin registers `tmt dap` as an LSP4IJ debug
adapter server, and the IDE's own debugger UI (gutter breakpoints,
stepping, variables) talks to it over the Debug Adapter Protocol.

One `tmt lsp` process serves both languages. The server routes each open
document to its own language service by file extension, so a `.tmc` file
and a `.tma` file coexist in one session without perturbing each other.

## Requirements

- A `tmt` binary reachable on `PATH`, or pointed to with the settings
  page's binary-path field (below).
- **LSP4IJ**, installed from the JetBrains Marketplace *before* you
  sideload this plugin (`MAINTAINERS.md`) — a sideloaded plugin does not auto-install
  its own plugin dependencies, so skipping this step leaves the IDE unable
  to load the plugin at all.
- This plugin is version 0.2.1, targeting `tmt` 0.4.0 as its tested
  floor: on startup it runs `tmt --version` and shows a warning
  notification (not a hard failure) if the binary reports something
  older, or an error notification if the binary can't be found at all.
  The plugin's own version number and the `tmt` floor version are
  independent numbers.
- Built against **LSP4IJ 0.20.1** on an IntelliJ Platform 2024.3
  baseline — the Gradle build resolves and compiles against both pinned
  versions, which demonstrates API compatibility; the build target is
  IntelliJ IDEA Community, so no Ultimate-only APIs are referenced. None
  of this has been exercised in a running IDE yet — whether the plugin
  actually loads and behaves correctly there is unobserved.

This plugin and the PM-1 one (`editors/jetbrains-pm/`) are independent and
can be installed side by side — they carry distinct plugin ids, claim
disjoint file extensions, and launch different binaries.

## Install the server

Build `tmt` from this repository and put it on `PATH`:

```sh
cargo install --path crates/turing-machine
```

Any released `tmt` binary already on `PATH` works too — the plugin only
shells out to it; it never bundles or builds one itself. To point at a
binary that isn't on `PATH`, set the binary path in Settings | Tools | tmt
(below).

Note for macOS: an IDE launched from the Dock or JetBrains Toolbox may not
inherit your shell's `PATH` (symptom: a `Cannot run program "tmt"` error on
opening a `.tmc` file, alongside the plugin's own "tmt not found"
notification). Set the absolute path — e.g. `~/.cargo/bin/tmt` — in
Settings | Tools | tmt instead of relying on `PATH`, then restart the IDE.

## Install LSP4IJ first

This plugin depends on **LSP4IJ** ("LSP4IJ" by Red Hat, plugin id
`com.redhat.devtools.lsp4ij`) to speak the Language Server Protocol —
Settings → Plugins → Marketplace, search "LSP4IJ", Install, then restart
the IDE if prompted. Do this *before* sideloading the plugin (`MAINTAINERS.md`): a
sideloaded plugin is installed from a local file, not from the Marketplace,
so the IDE has no opportunity to resolve and auto-install a declared plugin
dependency the way it would for a Marketplace install. Skipping this step
leaves this plugin disabled with an unsatisfied-dependency error. The
shipped build was compiled against LSP4IJ 0.20.1 — a build-time
compatibility check, not a runtime one; a newer 0.x/1.x release should
work unless its own compatibility range excludes this plugin's IntelliJ
Platform baseline (2024.3).

Building and sideloading the plugin from source, how it owns its files,
and the manual test checklist live in `MAINTAINERS.md` next to this file.

## Settings

**Settings | Tools | tmt** holds three fields:

| Field | Default | Meaning |
|---|---|---|
| tmt binary path | `tmt` | Path (or bare command resolved on `PATH`) to the `tmt` binary. The plugin launches it as `tmt lsp` for the language server, and reuses the same path for run configurations, target listing, and the `tmt dap` debug adapter (below). |
| Lint allow-list (comma-separated) | *(empty)* | Lint codes to suppress, forwarded to the server and kept live as you edit the setting — no IDE or server restart needed. This list is union-merged with any `tmt.json` project file the server discovers for the open document — either source suppressing a code is enough to suppress it, and neither can un-suppress a code the other disables. |
| Opt-in lint rules (comma-separated) | *(empty)* | Lint codes to *enable* — the totality lints, off by default (`state-may-trap` is the one that ships). IDE-side only: `tmt.json` carries `lint.allow` and nothing else, so an opt-in rule is enabled per-IDE or per-invocation (`tmt lint --warn CODE`), never per-project. |

One allow namespace spans both languages, so a `.tma`-only code is valid in
a list that also serves `.tmc` files, and vice versa.

Changing either list and applying the settings page pushes the new lists
straight to every already-running `tmt lsp` server (one per open project)
over the standard LSP configuration-change notification; each server
re-publishes diagnostics for its open documents immediately, so a
previously-squiggled suppressed code clears without reopening the file or
restarting anything.

The binary path is different: the language server reads it only when a
`tmtLsp` process starts, so editing it and applying the settings page has no
effect on an already-running server (restart the language server for the
project, or restart the IDE, to pick up a new path there). Run
configurations read the current path fresh on every run, so no restart is
needed for those.

## Run configurations

**Run → Edit Configurations… → + → tmt** adds a thin `tmt <subcommand>`
process wrapper — no build-system ambitions (no compile-before-run graph,
no artifact tracking):

| Field | Meaning |
|---|---|
| Subcommand | One of `build`, `compile`, `asm`, `lint`, `run`, selected from a fixed dropdown. |
| Target | `build` only: a target name from the `tmt.json` project manifest. Editable combo; the **Refresh** button fills it by running `tmt build --list-targets` in the working directory (the driver's own nearest-ancestor manifest discovery starts there), and the status line under it reports how many targets were found and how many declare a run block. Leave blank to build source files passed via Arguments instead (the driver's argv mode). |
| Run the target after building | `build` only: adds `--run`, so the built target immediately runs against its manifest-declared `.tmt` tape. Manifest-mode only — the driver rejects `--run` when sources are passed instead of a target — and only for targets whose run block declares a tape: TM-1 has no empty-tape default, so a target without one cannot be `--run` (that gating too is the driver's, with its own message). |
| Arguments | Free-form, shell-quoting-aware (parsed like a program-arguments field, so quoted strings and spaces behave as expected) — appended after the subcommand (and, for `build`, after the target) verbatim. `--call-mech` here overrides a target's committed lowering per invocation, the one link-side flag manifest mode accepts. |
| Working directory | Defaults to the project's base directory. |

Output streams to the Run tool window's console, including the process's
exit code on completion.

The `build` subcommand is the manifest-backed way to produce (and with
`--run`, immediately execute) a `.tmx`: one configuration per target
replaces the previous README revision's Shell-Script recipe. The dropdown
still does not offer `link` — a hand-rolled compile-then-link pipeline is
exactly what a `tmt.json` target already models (`docs/tmt/project.md`),
so declare one and `build` it; failing that, run `tmt link` from a
terminal (the same scope line VS Code's task provider draws: see
`editors/vscode-tm/README.md`'s "Custom pipelines" section for the
equivalent gap there).

## Debugging

The plugin registers **tmt dap** as a debug adapter server with LSP4IJ's
DAP support, which bridges it into the IDE debugger: breakpoints in the
gutter of `.tmc`/`.tma` files, stepping, pause, threads/variables views.
The adapter is the same editor-agnostic `tmt dap` stdio server the VS
Code extension uses; what the machine-level debug surface looks like
(registers, per-tape scopes, stepping granularity, disassembly) is
documented in this repository's `docs/dap.md`.

To debug:

1. Set a breakpoint in a `.tmc` (or `.tma`) file.
2. **Run → Edit Configurations… → + → Debug Adapter Protocol → DAP**,
   pick server **tmt dap** on the *Server* tab. The *Command* field may
   be left blank — the plugin then launches `<binary path from the
   settings page> dap`. Set the working directory to the directory
   holding your `tmt.json` (defaults to the project root).
3. On the *Configuration* tab pick one of the two launch templates and
   edit it in place:
   - **tmt: launch target** — `"target"` names a manifest target, built
     in process with debug info forced on (the same driver `tmt build`
     uses), then debugged against the target's manifest-declared tape.
     The manifest is discovered walking up from the working directory;
     an optional `"project"` key overrides the walk's starting point.
   - **tmt: launch program** — `"program"` points at a prebuilt `.tmx`,
     used as-is (build it with debug info — `tmt build --debug`, or
     `tmt compile -g` + `tmt link` — or source breakpoints answer
     unverified and stepping only stops at function boundaries);
     `"tape"` is **required** and points at a `.tmt` tape snapshot —
     TM-1 has no empty-tape default. `${workspaceFolder}` in these
     paths resolves to the configuration's working directory.
   - Both shapes accept `"stopOnEntry"` (break before the first
     instruction) and `"trace"` (per-instruction trace lines in the
     debug console, matching `tmt run --trace`).
4. Start the configuration with the **Debug** action.

The two launch templates carry inert `type`/`request`/`name` keys so the
JSON is copy-pasteable to and from a VS Code `launch.json` — the adapter
reads only its own arguments.

## Manifest validation

Since plugin 0.2.0 the bundled `tmt.json` JSON Schema (Draft-07, single-
sourced from `editors/schemas/`) is contributed automatically: any file
named `tmt.json` validates and completes against it, and the schema shows
in the editor's bottom-right schema switcher. The contribution sits
behind an optional dependency on the IDE's JSON plugin, which every
JetBrains IDE bundles; if you had added the manual mapping a previous
README revision described (**Settings → Languages & Frameworks → Schemas
and DTDs → JSON Schema Mappings**), remove it — two mappings for the same
file make the IDE ask which to use.

## License

GPL-3.0-or-later, same as the rest of this repository (see the repository
root's `LICENSE`).
