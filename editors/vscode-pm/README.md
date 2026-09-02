# PMC — Post-machine toolchain support for VS Code

Language support for `.pmc`, the C-like source language of the Post-machine
toolchain in this repository, and `.pma`, its PM-1 assembly dialect. This
extension is a thin client: it launches `pmt lsp` and renders whatever the
server reports — diagnostics, completions, hover, go-to-definition,
quickfixes, semantic tokens, document symbols, and formatting — over the
standard Language Server Protocol. Nothing here is a reimplementation;
every answer comes from the same compiler, assembler, linter, and
formatter the `pmt` command-line tool uses. `.pma` support is currently
syntax highlighting + the full `pmt lsp` surface + the `lint`/`fmt-check`
tasks below — see the Tasks section for the one gap (`compile`) that's
`.pmc`-only.

## Requirements

- A `pmt` binary reachable on `PATH`, or pointed to with the `pmt.path`
  setting (below).
- This extension is version 0.2.0. It has been tested against `pmt` 0.4.0;
  on activation it runs `pmt --version` and shows a warning (not a hard
  failure) if the binary reports something older. The extension's own
  version number and the tested `pmt` version are independent numbers.

## Install the server

Build `pmt` from this repository and put it on `PATH`:

```sh
cargo install --path crates/post-machine
```

Any released `pmt` binary already on `PATH` works too — the extension only
shells out to it; it never bundles or builds one itself. To point at a
binary that isn't on `PATH`, set `pmt.path` to its full path (below).

Note for macOS: VS Code launched from the Dock may not inherit your
shell's `PATH` (symptom: the "pmt not found" error notification on
activation). Set `pmt.path` to the absolute path — e.g.
`~/.cargo/bin/pmt` — then reload the window.

Building the extension from source and the manual test checklist live in
`MAINTAINERS.md` next to this file.

## Settings

| Setting | Default | Meaning |
|---|---|---|
| `pmt.path` | `pmt` | Path (or bare command resolved on `PATH`) to the `pmt` binary. The extension launches it as `pmt lsp` for the language server, and reuses the same path for the auto-provided tasks below. |
| `pmt.lint.allow` | `[]` | Lint codes to suppress, forwarded to the server and kept live as you edit the setting. This list is union-merged with any `pmt.json` project file the server discovers for the open document — either source suppressing a code is enough to suppress it, and neither can un-suppress a code the other disables. See `docs/pmt/lint.md` in this repository for the rule catalog and the `pmt.json` schema. |

`pmt.path` is read once, at activation — the extension does not watch
it for live changes. After editing it, reload the window (Command
Palette → **Developer: Reload Window**) for the new path to take
effect, for the language server, the auto-provided tasks above, and
debug sessions (below) alike. `pmt.lint.allow` has no such caveat — it
pushes live, as the table says.

## Tasks

Two families are offered under the `pmt` task type.

**Per-target tasks** come from the project manifest. When a workspace
folder resolves a `pmt.json` with a `project` section, every declared
target gets a `pmt build <target>` task, and every target carrying a
`run` block also gets `pmt build --run <target>`. The extension does not
search for the manifest itself: it runs `pmt build --list-targets` with
its working directory set to the workspace folder root and lets the
binary's own nearest-ancestor discovery answer. The list is cached for a
few seconds and refreshes sooner when the project file changes. In a
multi-root window each folder contributes its own targets.

A folder with no project file, an invalid manifest, or a missing `pmt`
binary simply contributes no per-target tasks for that folder — the
file-scoped tasks below keep working regardless. The reason is logged to
the `pmt` output channel.

**File-scoped tasks** act on the active editor's file. With a `.pmc` file
open, three become available under **Terminal → Run Task…**:

| Task | Runs |
|---|---|
| `pmt compile` | `pmt compile <file>` |
| `pmt lint` | `pmt lint <file>` |
| `pmt fmt-check` | `pmt fmt --check <file>` |

With a `.pma` file open, only `pmt lint` and `pmt fmt-check` are offered.
`pmt compile` stays `.pmc`-only — a `.pma` file assembles via `pmt asm`,
not `pmt compile`, and driving that from the task provider is out of this
v1's scope; assemble it from a terminal instead.

Both families report through the bundled `$pmt` problem matcher, which
parses `FILE:LINE:COL: SEVERITY: MESSAGE [code]` lines (`error`, `warning`,
or `lint`) into the Problems panel.

**`fmt-check` caveat:** `pmt fmt --check` reports a file that would be
reformatted as a bare path, with no line or column — there is nothing
position-shaped for `$pmt` to parse, and it deliberately doesn't try. A
dirty file makes the `fmt-check` task fail (non-zero exit, visible in the
terminal and as a failed task run), but the **Problems panel stays empty**
for it. Reformat with `pmt fmt` (or format-on-save, below) and re-run to
confirm clean.

### Custom pipelines

Per-target tasks cover the common case; they don't cover a build shape the
manifest doesn't express. For that, write the stages by hand in
`.vscode/tasks.json` and chain them with `dependsOn`. A minimal
compile → link → run pipeline, treating the current file as `main`:

```json
{
  "version": "2.0.0",
  "tasks": [
    {
      "label": "pmc: compile",
      "type": "process",
      "command": "${config:pmt.path}",
      "args": ["compile", "${file}", "-o", "${fileDirname}/${fileBasenameNoExtension}.pmo"],
      "problemMatcher": "$pmt"
    },
    {
      "label": "pmc: link",
      "type": "process",
      "command": "${config:pmt.path}",
      "args": ["link", "${fileDirname}/${fileBasenameNoExtension}.pmo", "-o", "${fileDirname}/${fileBasenameNoExtension}.pmx"],
      "problemMatcher": "$pmt"
    },
    {
      "label": "pmc: run",
      "type": "process",
      "command": "${config:pmt.path}",
      "args": ["run", "${fileDirname}/${fileBasenameNoExtension}.pmx", "--tape-cells", " * *"],
      "problemMatcher": "$pmt"
    },
    {
      "label": "pmc: build and run",
      "dependsOrder": "sequence",
      "dependsOn": ["pmc: compile", "pmc: link", "pmc: run"]
    }
  ]
}
```

`pmc: build and run` compiles the open file, links it (against the embedded
standard library, added implicitly unless `--nostdlib` is passed) into a
`.pmx`, then runs it against an inline three-cell tape (`--tape-cells " * *"`).
The three tasks use `"type": "process"` rather than `"shell"` — `args` go
to `pmt` verbatim, as the extension's own tasks do, so the leading space
and `*` glyphs in the tape argument reach `pmt` exactly as written instead
of being reinterpreted by a shell. Swap the `link`/`run` arguments for
whatever the program under test actually needs — additional `.pmo` inputs,
`--tape-block`, `--max-steps`, and so on are all documented in
`docs/pmt/cli.md` in this repository.

## Debugging

The `pmt` debugger type this extension contributes launches `pmt dap` —
the same binary, resolved the same way as the language server and the
tasks above — as a Debug Adapter Protocol server on stdio. It supports
two launch shapes, matching `pmt dap`'s own two modes.

**Target mode** builds a `pmt.json` manifest target in process (the
same driver `pmt build TARGET` uses) and always forces debug info on,
regardless of the target's own profile — nothing to remember. This is
what **Run and Debug → create a launch.json** offers by default:

```json
{
  "version": "0.2.0",
  "configurations": [
    {
      "name": "pmt: launch target",
      "type": "pmt",
      "request": "launch",
      "target": "main",
      "stopOnEntry": true
    }
  ]
}
```

**Program mode** debugs a prebuilt `.pmx` as-is, against an optional
`.pmt` tape snapshot (omit `tape` for the empty tape):

```json
{
  "name": "pmt: launch program",
  "type": "pmt",
  "request": "launch",
  "program": "${workspaceFolder}/main.pmx",
  "tape": "${workspaceFolder}/main.pmt",
  "stopOnEntry": true
}
```

Build the `.pmx` with `-g` first (`pmt compile -g` / `pmt link`, or
`pmt build --debug`) — target mode injects `-g` for you, but program
mode debugs whatever executable you hand it: without debug info, a
source breakpoint answers unverified (with a "build with -g" message
instead of a squiggle-free stop), and default-granularity (line/
statement) stepping only stops at function boundaries instead of
`.pmc` source lines — a single-function program with no calls runs to
completion (or to a breakpoint) on the very first step. Request
`"granularity": "instruction"` explicitly to step one instruction at a
time regardless of debug info.

Both modes share two more options: `"stopOnEntry": true` breaks before
the first instruction runs, instead of running to the first breakpoint
(or to completion); `"trace": true` streams a per-instruction trace
line to the Debug Console as `console`-category output, the same lines
`pmt run --trace` prints (build diagnostics, streamed separately in
target mode, are the only `stderr`-category output this adapter
emits). Program mode additionally accepts `"strictCells": true`, the
same semantics as `pmt run --strict-cells`.

The session supports source and instruction breakpoints, step
in/over/out, a Variables view over the machine's registers and tape
(including editing a value via `setVariable`), and VS Code's built-in
**Open Disassembly View** — right-click a stack frame, or run it from
the Command Palette.

## License

GPL-3.0-or-later, same as the rest of this repository (see `LICENSE`).
