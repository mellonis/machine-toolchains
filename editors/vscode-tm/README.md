# TMC — Turing-machine toolchain support for VS Code

Language support for `.tmc`, the source language of the TM-1 Turing-machine
toolchain in this repository, and `.tma`, its TM-1 assembly dialect. This
extension is a thin client: it launches `tmt lsp` and renders whatever the
server reports — diagnostics, completions, hover, go-to-definition,
quickfixes, semantic tokens, document symbols, and formatting — over the
standard Language Server Protocol. Nothing here is a reimplementation;
every answer comes from the same compiler, assembler, linter, and formatter
the `tmt` command-line tool uses.

One `tmt lsp` process serves both languages. The server routes each open
document to its own language service by file extension, so a `.tmc` file and
a `.tma` file coexist in one session without perturbing each other.

## Requirements

- A `tmt` binary reachable on `PATH`, or pointed to with the `tmt.path`
  setting (below).
- This extension is version 0.2.0, targeting `tmt` 0.4.0 as its tested
  floor: on activation it runs `tmt --version` and shows a warning (not a
  hard failure) if the binary reports something older. The extension's own
  version number and the `tmt` floor version are independent numbers.

## Install the server

Build `tmt` from this repository and put it on `PATH`:

```sh
cargo install --path crates/turing-machine
```

Any released `tmt` binary already on `PATH` works too — the extension only
shells out to it; it never bundles or builds one itself. To point at a
binary that isn't on `PATH`, set `tmt.path` to its full path (below).

Note for macOS: VS Code launched from the Dock may not inherit your shell's
`PATH` (symptom: the "tmt not found" error notification on activation). Set
`tmt.path` to the absolute path — e.g. `~/.cargo/bin/tmt` — then reload the
window.

Building the extension from source and the manual test checklist live in
`MAINTAINERS.md` next to this file.

## Settings

| Setting | Default | Meaning |
|---|---|---|
| `tmt.path` | `tmt` | Path (or bare command resolved on `PATH`) to the `tmt` binary. The extension launches it as `tmt lsp` for the language server, and reuses the same path for the auto-provided tasks below. |
| `tmt.lint.allow` | `[]` | Lint codes to suppress, forwarded to the server and kept live as you edit the setting. This list is union-merged with any `tmt.json` project file the server discovers for the open document — either source suppressing a code is enough to suppress it, and neither can un-suppress a code the other disables. |
| `tmt.lint.warn` | `[]` | Opt-in lint codes to *enable* (the totality lints, off by default — `state-may-trap` is the one that ships). This is IDE-side only: `tmt.json` carries `lint.allow` and nothing else, so an opt-in rule is enabled per-editor or per-invocation (`tmt lint --warn CODE`), never per-project. |

One allow namespace spans both languages, so a `.tma`-only code is valid in
a list that also serves `.tmc` files, and vice versa.

`tmt.path` is read once, at activation — the extension does not watch it for
live changes. After editing it, reload the window (Command Palette →
**Developer: Reload Window**) for the new path to take effect, for the
language server, the auto-provided tasks, and debug sessions (below) alike.
`tmt.lint.allow` and `tmt.lint.warn` have no such caveat — they push live.

## Tasks

Two families are offered under the `tmt` task type.

**Per-target tasks** come from the project manifest. When a workspace
folder resolves a `tmt.json` with a `project` section, every declared
target gets a `tmt build <target>` task, and every target carrying a
`run` block also gets `tmt build --run <target>`. The extension does not
search for the manifest itself: it runs `tmt build --list-targets` with
its working directory set to the workspace folder root and lets the
binary's own nearest-ancestor discovery answer. The list is cached for a
few seconds and refreshes sooner when the project file changes. In a
multi-root window each folder contributes its own targets.

A folder with no project file, an invalid manifest, or a missing `tmt`
binary simply contributes no per-target tasks for that folder — the
file-scoped tasks below keep working regardless. The reason is logged to
the `tmt` output channel.

**File-scoped tasks** act on the active editor's file. With a `.tmc` file
open, three become available under **Terminal → Run Task…**:

| Task | Runs |
|---|---|
| `tmt compile` | `tmt compile <file>` |
| `tmt lint` | `tmt lint <file>` |
| `tmt fmt-check` | `tmt fmt --check <file>` |

With a `.tma` file open the same three appear, except that `tmt compile` is
replaced by `tmt asm` (`tmt asm <file>`) — each language has its own front
end, and both are single-file commands.

Both families report through the bundled `$tmt` problem matcher, which
parses `FILE:LINE:COL: SEVERITY: MESSAGE [code]` lines (`error`, `warning`,
or `lint`) into the Problems panel.

**Bracketed codes:** compile and assemble fatals carry one (e.g.
`[undefined-state]`, `[table-discipline]`); `tmt lint`'s own findings print
without one, so those Problems-panel entries have an empty code column. The
same findings *do* carry their codes over LSP, so the live squiggles are
fully coded — this is a CLI rendering difference, not a missing code.

**`fmt-check` caveat:** `tmt fmt --check` reports a file that would be
reformatted as a bare path, with no line or column — there is nothing
position-shaped for `$tmt` to parse, and it deliberately doesn't try. A
dirty file makes the `fmt-check` task fail (non-zero exit, visible in the
terminal and as a failed task run), but the **Problems panel stays empty**
for it. Reformat with `tmt fmt` (or format-on-save, below) and re-run to
confirm clean.

### Custom pipelines

Per-target tasks cover the common case; they don't cover a build shape the
manifest doesn't express. For that, write the stages by hand in
`.vscode/tasks.json` and chain them with `dependsOn`. A minimal
compile → link pipeline, treating the current file as the program:

```json
{
  "version": "2.0.0",
  "tasks": [
    {
      "label": "tmc: compile",
      "type": "process",
      "command": "${config:tmt.path}",
      "args": ["compile", "${file}", "-o", "${fileDirname}/${fileBasenameNoExtension}.tmo"],
      "problemMatcher": "$tmt"
    },
    {
      "label": "tmc: link",
      "type": "process",
      "command": "${config:tmt.path}",
      "args": ["link", "${fileDirname}/${fileBasenameNoExtension}.tmo", "-o", "${fileDirname}/${fileBasenameNoExtension}.tmx"],
      "problemMatcher": "$tmt"
    },
    {
      "label": "tmc: build",
      "dependsOrder": "sequence",
      "dependsOn": ["tmc: compile", "tmc: link"]
    }
  ]
}
```

Running the linked `.tmx` needs a tape block, which `tmt tape-block new`/`set`
builds and which depends entirely on the program under test — so it is left
out of the generic pipeline above rather than guessed at. Add a `tmt run`
task with the `--tape-block` your program expects. `tmt link`'s `--call-mech`,
`--entry`, and `--nostdlib` flags and the full `tmt run` surface are
documented in `docs/tmt/cli.md` in this repository.

The tasks use `"type": "process"` rather than `"shell"` — `args` go to `tmt`
verbatim, as the extension's own tasks do, so glyph arguments reach `tmt`
exactly as written instead of being reinterpreted by a shell.

## Debugging

The `tmt` debugger type this extension contributes launches `tmt dap` —
the same binary, resolved the same way as the language server and the
tasks above — as a Debug Adapter Protocol server on stdio. It supports
two launch shapes, matching `tmt dap`'s own two modes.

**Target mode** builds a `tmt.json` manifest target in process (the
same driver `tmt build TARGET` uses) and always forces debug info on,
regardless of the target's own profile — nothing to remember. The
target's tape comes from its own `run` block, resolved exactly as
`tmt build --run` resolves it — a target with no `run` block, or one
whose `run` block declares no `tape`, fails to launch. This is what
**Run and Debug → create a launch.json** offers by default:

```json
{
  "version": "0.2.0",
  "configurations": [
    {
      "name": "tmt: launch target",
      "type": "tmt",
      "request": "launch",
      "target": "main",
      "stopOnEntry": true
    }
  ]
}
```

**Program mode** debugs a prebuilt `.tmx` as-is, against a `.tmt` tape
snapshot:

```json
{
  "name": "tmt: launch program",
  "type": "tmt",
  "request": "launch",
  "program": "${workspaceFolder}/main.tmx",
  "tape": "${workspaceFolder}/main.tmt",
  "stopOnEntry": true
}
```

`tape` is **required** in program mode — unlike `pmt`, TM-1 has no
empty-tape default (`tmt run` itself requires `--tape-block`), so a
program-mode launch with no `tape` fails cleanly rather than
substituting one.

Build the `.tmx` with `-g` first (`tmt compile -g` / `tmt link`, or
`tmt build --debug`) — target mode injects `-g` for you, but program
mode debugs whatever executable you hand it: without debug info, a
source breakpoint answers unverified (with a "build with -g" message
instead of a squiggle-free stop), and default-granularity (line/
statement) stepping only stops at function boundaries instead of
`.tmc` source lines — a single-function program with no calls runs to
completion (or to a breakpoint) on the very first step. Request
`"granularity": "instruction"` explicitly to step one instruction at a
time regardless of debug info.

Both modes share two more options: `"stopOnEntry": true` breaks before
the first instruction runs, instead of running to the first breakpoint
(or to completion); `"trace": true` streams a per-instruction trace
line to the Debug Console as `console`-category output, the same lines
`tmt run --trace` prints (build diagnostics, streamed separately in
target mode, are the only `stderr`-category output this adapter
emits). There is no `strictCells` option here — `tmt run` has
no `--strict-cells` flag for this adapter to mirror.

The session supports source and instruction breakpoints, step
in/over/out, a Variables view over the machine's registers and every
tape (including editing a value via `setVariable`), and VS Code's
built-in **Open Disassembly View** — right-click a stack frame, or run
it from the Command Palette.

## License

GPL-3.0-or-later, same as the rest of this repository (see `LICENSE`).
