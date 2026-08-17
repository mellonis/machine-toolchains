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

## Build and sideload the extension

From `editors/vscode-pm`:

```sh
npm install
npm run package
```

`npm run package` copies in the shared `.pmc` and `.pma` TextMate
grammars, compiles the extension, and runs `vsce package`, producing
`pmc-0.1.3.vsix` in this directory. Install it into VS Code:

```sh
code --install-extension pmc-0.1.3.vsix
```

Reload the window (or restart VS Code) after installing or upgrading.

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

## Manual test checklist

v1 has no automated editor end-to-end test — walk this by hand against a
built `.vsix` and a `pmt` on `PATH`, after any change that touches the
client or the server's editor-facing surface.

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

- [x] **Open** `check.pmc`. Confirm a squiggle on the `debugger;` line (the
      `leftover-debugger` lint finding) — diagnostics are live on open, no
      manual trigger needed.
- [x] **Completion**: on a new line inside `main`, type `@g`. Confirm
      `goToEnd` and `std::goToEnd` appear in the completion list. (A bare
      `@` with nothing typed after it is itself a lexical error and won't
      show candidates — type at least one more character. If the list
      still doesn't appear, press Ctrl+Space to retrigger it — the editor
      may have cached the empty result from the moment right after `@`.)
      After observing the completion popup, **undo the typed text** to
      restore a parse-clean state before continuing — only completions
      tolerate a broken parse; the following steps need valid syntax.
- [x] **Go-to-definition**: invoke it on `goToEnd`, either in
      `use std::goToEnd;` or inside the `@goToEnd()` call. Confirm it jumps
      into a materialized copy of the standard library — a cached
      `std.pmc` outside this workspace, not a file you're editing — landing
      on `export goToEnd() {`. See `docs/lsp.md` in this repository for
      where that cache lives.
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
      `deprecated-call` lint finding's `DiagnosticTag`). Undo both
      edits to restore the base scratch file before continuing.
- [x] **Task — lint**: run the `pmt lint` task. Confirm the Problems panel
      populates with the `leftover-debugger` finding.
- [x] **Config file-watch — `pmt.json` on disk**: with `check.pmc` still
      open and the `leftover-debugger` finding still showing (previous
      step), create a `pmt.json` file next to it containing
      `{"lint": {"allow": ["leftover-debugger"]}}` (schema:
      `docs/pmt/lint.md` in this repository). Confirm the squiggle on
      `debugger;` disappears **without touching any VS Code setting** —
      this is the server's `workspace/didChangeWatchedFiles` watch firing
      on the on-disk file, not the `pmt.lint.allow` setting. Delete
      `pmt.json` and confirm the squiggle returns before continuing — the
      steps below need the finding present again.
- [x] **Task — fmt-check, and its caveat**: run the `pmt fmt-check` task.
      `check(1,2)` is missing its canonical space, so the task fails
      (non-zero exit, visible in the terminal) — but confirm the Problems
      panel does **not** gain an entry for it, per the caveat above.
- [x] **Quickfix**: on the `debugger;` squiggle, open the lightbulb / Quick
      Fix menu and apply the fix. This one is gated (equivalent to
      `pmt lint --fix --force`), so it may show as a secondary, not the
      single default action — confirm the `debugger;` statement is deleted
      either way.
- [x] **Format-on-save**: with `editor.formatOnSave` enabled for `.pmc` (or
      run **Format Document**), confirm `check(1,2)` becomes `check(1, 2)`
      and nothing else changes — formatting is layout-only.
- [x] **Task — compile, with a fatal**: break the file (e.g. delete the
      closing `)` so the line reads `check(1, 2;`), save, and run the
      `pmt compile` task. Confirm the Problems panel shows exactly one
      fatal entry carrying its bracketed code (`[unexpected-token]`).
      Undo the edit.
- [x] **Dogfood — the embedded standard library**: open
      `crates/post-machine/src/stdlib/std.pmc` from this repository
      directly (not the go-to-definition-materialized cache copy from
      earlier). Confirm **zero diagnostics**, that semantic tokens are
      visible (coloring beyond what the TextMate grammar alone gave
      `check.pmc` — e.g. call-site identifiers colored distinctly from
      keywords), and that running **Format Document** is a **no-op** — no
      diff, no dirty-buffer indicator — the checked-in file is already
      canonically formatted. This is the editor-observed half of the
      dogfood check that `cargo test -p mtc-post-machine --lib lsp`
      already covers on the server side alone.

### `.pma` checklist

`pmt lsp` serves `.pma`, the PM-1 assembly dialect, through the same
process and connection as `.pmc` above (`docs/lsp.md`, "Languages") — walk
this checklist in the same editor session as the `.pmc` one above, without
restarting the extension, so the last step below has something to confirm.

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

- [x] **Open** `check.pma`. Confirm syntax colors appear (the `.func`
      directive, mnemonics, the `L1`/`UNUSED` labels, a `;` comment if you
      add one) — this is the shared TextMate grammar
      (`editors/grammars/pma.tmLanguage.json`), copied in by
      `copy-assets.js` alongside the `.pmc` one.
- [x] **Typo mnemonic**: change `jm L1` to `jpm L1`. Confirm a squiggle on
      `jpm` carrying the `unknown-mnemonic` code. **Undo** the typo back to
      `jm L1` before continuing — per `docs/lsp.md`, a fatal error hides
      lint findings entirely (no separate compile-warning channel on the
      `.pma` side), so the next step needs a clean assemble to have
      anything to show.
- [x] **Unused label + quickfix**: confirm a warning on the `UNUSED:`
      label (the `unused-label` lint finding). Open the lightbulb / Quick
      Fix menu and apply the fix — unlike `.pmc`'s gated `leftover-debugger`
      fix, this one is machine-applicable, so it should be the single
      default action. Confirm only the `UNUSED:` label is removed, leaving
      `nop` behind (and the warning disappears).
- [x] **Go-to-definition**: invoke it on the `L1` operand in `jm L1`
      (inside `goToEnd`). Confirm it jumps to the `L1:` label definition
      on the line directly above, in the same file — `.pma` has no
      external/materialized target the way `.pmc`'s `std::` calls do.
- [x] **Outline**: open the Outline view (or **Go to Symbol in
      Editor…**). Confirm it shows `goToEnd` and `main` as functions, each
      containing its labels as children (`L1` under `goToEnd`; `UNUSED`
      under `main`, until the previous step deleted it).
- [x] **Format Document**: run it. Confirm the file snaps to the
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
      the checklist above, still in this same window/session. Confirm its
      diagnostics (the `leftover-debugger` squiggle) are still live —
      opening and editing `.pma` documents never perturbed the `.pmc`
      service, per `docs/lsp.md`'s "one process, two independent language
      services."

### Debug session

Not yet walked by hand — live verification of the `pmt` debugger type
is the maintainer's step.

Create a scratch `pmt.json` and `debug.pmc` in the same folder:

```json
{ "project": { "targets": { "main": { "sources": ["debug.pmc"] } } } }
```

```pmc
main() {
    right;
    mark;
    right;
    mark;
}
```

- [ ] **Launch, breakpoint, tape, setVariable, disassembly, step**: add
      a "pmt: launch target" configuration (Run and Debug → create a
      launch.json, or the **pmt: Launch target** snippet) with `target:
      "main"` and `stopOnEntry: true`, then press F5. Confirm the
      session stops before the first instruction. Set a source
      breakpoint on the second `mark;` line and **Continue**; confirm
      it stops there with a "breakpoint" reason (not "entry"). Open the
      Variables view, expand the **Tapes** scope, and confirm cell `[1]`
      already reads `'*'` (the first `mark`) while the current cell —
      named `» [2]` — still reads blank. Edit cell `[3]`'s value there
      (`setVariable`, off the head's path so it can't interact with the
      next step) to `*` and confirm the tape view updates to match
      immediately. Right-click the stack frame (or use the Command
      Palette) and **Open Disassembly View**; confirm PM-1 instructions
      (`ent`/`rgt`/`wr`/`stp`) with addresses are listed. **Step Over**
      once — this is the program's last statement, so confirm it runs to
      completion and the session terminates cleanly.

## License

GPL-3.0-or-later, same as the rest of this repository (see `LICENSE`).
