# PMC — Post-machine toolchain support for VS Code

Language support for `.pmc`, the C-like source language of the Post-machine
toolchain in this repository. This extension is a thin client: it launches
`pmt lsp` and renders whatever the server reports — diagnostics, completions,
go-to-definition, quickfixes, semantic tokens, document symbols, and
formatting — over the standard Language Server Protocol. Nothing here is a
reimplementation; every answer comes from the same compiler, linter, and
formatter the `pmt` command-line tool uses.

## Requirements

- A `pmt` binary reachable on `PATH`, or pointed to with the `pmt.path`
  setting (below).
- This extension is version 0.1.0. It has been tested against `pmt` 0.1.0;
  on activation it runs `pmt --version` and shows a warning (not a hard
  failure) if the binary reports something older. The extension's own
  version number and the tested `pmt` version are independent numbers that
  happen to both read 0.1.0 today.

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

From `editors/vscode`:

```sh
npm install
npm run package
```

`npm run package` copies in the shared `.pmc` TextMate grammar, compiles
the extension, and runs `vsce package`, producing `pmc-0.1.0.vsix` in
this directory. Install it into VS Code:

```sh
code --install-extension pmc-0.1.0.vsix
```

Reload the window (or restart VS Code) after installing or upgrading.

## Settings

| Setting | Default | Meaning |
|---|---|---|
| `pmt.path` | `pmt` | Path (or bare command resolved on `PATH`) to the `pmt` binary. The extension launches it as `pmt lsp` for the language server, and reuses the same path for the auto-provided tasks below. |
| `pmt.lint.allow` | `[]` | Lint codes to suppress, forwarded to the server and kept live as you edit the setting. This list is union-merged with any `pmt.json` project file the server discovers for the open document — either source suppressing a code is enough to suppress it, and neither can un-suppress a code the other disables. See `docs/lint.md` in this repository for the rule catalog and the `pmt.json` schema. |

`pmt.path` is read once, at activation — the extension does not watch
it for live changes. After editing it, reload the window (Command
Palette → **Developer: Reload Window**) for the new path to take
effect, both for the language server and for the auto-provided tasks
above. `pmt.lint.allow` has no such caveat — it pushes live, as the
table says.

## Tasks

The extension registers a task provider for the `pmt` task type. With a
`.pmc` file open, three file-scoped tasks become available under
**Terminal → Run Task…**, each running against the active editor's file:

| Task | Runs |
|---|---|
| `pmt compile` | `pmt compile <file>` |
| `pmt lint` | `pmt lint <file>` |
| `pmt fmt-check` | `pmt fmt --check <file>` |

All three are wired to the bundled `$pmt` problem matcher, which parses
`FILE:LINE:COL: SEVERITY: MESSAGE [code]` lines (`error`, `warning`, or
`lint`) into the Problems panel.

**`fmt-check` caveat:** `pmt fmt --check` reports a file that would be
reformatted as a bare path, with no line or column — there is nothing
position-shaped for `$pmt` to parse, and it deliberately doesn't try. A
dirty file makes the `fmt-check` task fail (non-zero exit, visible in the
terminal and as a failed task run), but the **Problems panel stays empty**
for it. Reformat with `pmt fmt` (or format-on-save, below) and re-run to
confirm clean.

### A full build-and-run pipeline

The task provider only emits the three single-file tasks above — it
deliberately does not generate a compile → link → run pipeline, since
linking and running need choices (which objects, which tape) it can't infer
from one open file. Paste this into `.vscode/tasks.json` for a minimal one,
treating the current file as `main`:

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
      "args": ["run", "${fileDirname}/${fileBasenameNoExtension}.pmx", "--tape", " * *"],
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
`.pmx`, then runs it against an inline three-cell tape (`--tape " * *"`).
The three tasks use `"type": "process"` rather than `"shell"` — `args` go
to `pmt` verbatim, as the extension's own tasks do, so the leading space
and `*` glyphs in the tape argument reach `pmt` exactly as written instead
of being reinterpreted by a shell. Swap the `link`/`run` arguments for
whatever the program under test actually needs — additional `.pmo` inputs,
`--tape-block`, `--max-steps`, and so on are all documented in
`docs/cli.md` in this repository.

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
- [x] **Task — lint**: run the `pmt lint` task. Confirm the Problems panel
      populates with the `leftover-debugger` finding.
- [x] **Config file-watch — `pmt.json` on disk**: with `check.pmc` still
      open and the `leftover-debugger` finding still showing (previous
      step), create a `pmt.json` file next to it containing
      `{"lint": {"allow": ["leftover-debugger"]}}` (schema:
      `docs/lint.md` in this repository). Confirm the squiggle on
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

## License

GPL-3.0-or-later, same as the rest of this repository (see `LICENSE`).
