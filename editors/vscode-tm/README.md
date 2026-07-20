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
- This extension is version 0.1.0, targeting `tmt` 0.2.0 as its tested
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

## Build and sideload the extension

From `editors/vscode-tm`:

```sh
npm install
npm run package
```

`npm run package` copies in the shared `.tmc` and `.tma` TextMate grammars
from `editors/grammars/`, compiles the extension, and runs `vsce package`,
producing `tmc-0.1.0.vsix` in this directory. Install it into VS Code:

```sh
code --install-extension tmc-0.1.0.vsix
```

Reload the window (or restart VS Code) after installing or upgrading.

This extension and the PM-1 one (`editors/vscode-pm/`) are independent and
can be installed side by side — they claim disjoint file extensions and
launch different binaries.

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
**Developer: Reload Window**) for the new path to take effect, both for the
language server and for the auto-provided tasks. `tmt.lint.allow` and
`tmt.lint.warn` have no such caveat — they push live.

## Tasks

The extension registers a task provider for the `tmt` task type. With a
`.tmc` file open, three file-scoped tasks become available under
**Terminal → Run Task…**, each running against the active editor's file:

| Task | Runs |
|---|---|
| `tmt compile` | `tmt compile <file>` |
| `tmt lint` | `tmt lint <file>` |
| `tmt fmt-check` | `tmt fmt --check <file>` |

With a `.tma` file open the same three appear, except that `tmt compile` is
replaced by `tmt asm` (`tmt asm <file>`) — each language has its own front
end, and both are single-file commands.

All are wired to the bundled `$tmt` problem matcher, which parses
`FILE:LINE:COL: SEVERITY: MESSAGE [code]` lines (`error`, `warning`, or
`lint`) into the Problems panel.

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

### A full build-and-run pipeline

The task provider only emits the three single-file tasks above — it
deliberately does not generate a compile → link → run pipeline, since
linking and running need choices (which objects, which tape block, which
call mechanism) it can't infer from one open file. Paste this into
`.vscode/tasks.json` for a minimal one, treating the current file as the
program:

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

Running the linked `.tmx` needs a tape block, which `tmt tape new`/`set`
builds and which depends entirely on the program under test — so it is left
out of the generic pipeline above rather than guessed at. Add a `tmt run`
task with the `--tape-block` your program expects. `tmt link`'s
`--call-mech`, `--entry`, and `--nostdlib` flags and the full `tmt run`
surface are documented in `docs/cli.md` in this repository.

The tasks use `"type": "process"` rather than `"shell"` — `args` go to `tmt`
verbatim, as the extension's own tasks do, so glyph arguments reach `tmt`
exactly as written instead of being reinterpreted by a shell.

## Manual test checklist

This release has no automated editor end-to-end test — walk this by hand
against a built `.vsix` and a `tmt` on `PATH`, after any change that touches
the client or the server's editor-facing surface. The server-side behavior
each step exercises is covered by the Rust test suite; what this checklist
adds is that the *client wiring* delivers it into the editor.

Create a scratch file, e.g. `check.tmc`:

```tmc
alphabet marks { '_', 'x', 'y' }

routine markSpot(tape t: marks) {
  entry state put {
    [*] -> write ['x'] return;
  }
}

routine unusedHelper(tape t: marks) {
  entry state idle {
    [*] -> return;
  }
}

machine {
  tape work: marks;

  entry state scan {
    ['x'] -> debugger write ['_'] stop;
    ['y'] -> call markSpot(t = work) then scan;
      [*] ->    move [>] goto scan;
  }
}
```

- [ ] **Open** `check.tmc`. Confirm syntax colors appear (the `alphabet` /
      `routine` / `machine` keywords, the `'x'` glyph literals, the `->`
      rule arrows, the `//` comments if you add one) — this is the shared
      TextMate grammar (`editors/grammars/tmc.tmLanguage.json`), copied in
      by `copy-grammar.js`. Confirm **two** squiggles appear without any
      manual trigger: one on `unusedHelper` (`unused-routine`) and one on
      the `debugger` marker (`leftover-debugger`).
- [ ] **Completion**: put the cursor at the start of an action, after a
      `->`, and press Ctrl+Space. Confirm the action keywords appear
      (`write`, `move`, `goto`, `call`, `return`, `stop`, `halt`,
      `debugger`) *and* that the in-scope state name `scan` is offered —
      completion is context-aware, not one flat keyword list.
- [ ] **Go-to-definition**: invoke it on `markSpot` in the
      `call markSpot(...)` line. Confirm it jumps to the
      `routine markSpot(tape t: marks) {` declaration in this file.
      Navigation is **single-file** in this release — there is no
      materialized standard-library target the way a cross-file toolchain
      would offer; a `use std::…` name resolves within the document's own
      analysis.
- [ ] **Hover**: hover over `markSpot` at the same call site. Confirm a
      tooltip showing the routine's signature, `routine markSpot(tape t:
      marks)`. Hover over a tape name and confirm its alphabet is named.
- [ ] **Semantic tokens**: confirm coloring beyond what the TextMate grammar
      alone can give — state names and call targets should read distinctly
      from bare identifiers, which a regex grammar cannot resolve.
- [ ] **Outline**: open the Outline view (or **Go to Symbol in Editor…**).
      Confirm it lists `marks`, `markSpot`, `unusedHelper`, and `machine`.
- [ ] **Task — lint**: run the `tmt lint` task. Confirm the Problems panel
      populates with both findings (with an empty code column, per the
      bracketed-codes note above).
- [ ] **Config file-watch — `tmt.json` on disk**: with `check.tmc` still
      open and both findings showing, create a `tmt.json` next to it
      containing `{"lint": {"allow": ["leftover-debugger"]}}`. Confirm the
      `debugger` squiggle disappears **without touching any VS Code
      setting** — this is the server's watch firing on the on-disk file,
      not the `tmt.lint.allow` setting. Confirm the `unused-routine`
      squiggle is still there (only the named code was suppressed). Delete
      `tmt.json` and confirm the squiggle returns before continuing.
- [ ] **Opt-in lint via settings**: add `state-may-trap` to
      `tmt.lint.warn` in VS Code settings. Confirm new findings appear
      live, without a reload — this rule is off by default and only runs
      when named. Remove it again before continuing.
- [ ] **Task — fmt-check, and its caveat**: run the `tmt fmt-check` task.
      The last rule's indentation is deliberately off-grid, so the task
      fails (non-zero exit, visible in the terminal) — but confirm the
      Problems panel does **not** gain an entry for it, per the caveat
      above.
- [ ] **Format-on-save**: with `editor.formatOnSave` enabled for `.tmc` (or
      run **Format Document**), confirm the state block snaps to its
      canonical grid — the `->` arrows aligned down the block — and that
      nothing but whitespace changes.
- [ ] **Quickfix from a fatal**: change `goto scan` on the last rule to
      `goto missing`, save, and confirm one fatal squiggle appears
      (`undefined-state`). Open the lightbulb / Quick Fix menu on it and
      confirm a **declare state `missing`** action is offered as the
      preferred fix; apply it and confirm a `state missing { [*] -> stop; }`
      stub is inserted with the right tape arity. Undo.
      (`.tmc` lint findings themselves carry no machine-applicable fixes in
      this release — every quickfix on this side is derived from a compiler
      fatal. This is expected, not a gap in the wiring.)
- [ ] **Task — compile, with a fatal**: break the file (e.g. delete a
      closing `]`), save, and run the `tmt compile` task. Confirm the
      Problems panel shows exactly one fatal entry carrying its bracketed
      code. Undo the edit.
- [ ] **Dogfood — the embedded standard library**: open
      `crates/turing-machine/src/stdlib/std.tmc` from this repository
      directly. Confirm **zero diagnostics**, that semantic tokens are
      visible, and that running **Format Document** is a **no-op** — no
      diff, no dirty-buffer indicator — the checked-in file is already
      canonically formatted.

### `.tma` checklist

`tmt lsp` serves `.tma` through the same process and connection as `.tmc`
above — walk this checklist in the same editor session, without restarting
the extension, so the last step has something to confirm.

Create a second scratch file, e.g. `check.tma`:

```tma
.routine main, tapes=3, alpha=(3, 3, 3)

.section tables
Tscan:  .row    [1, *, *]
        .row    [1, 2, *]
        .row    [*, *, *]
Dscan:  .targets L_hit, L_dead, L_step

.section code
.func main
L_loop: rd
        mtc     Tscan
        djmp    Dscan
L_hit:  wr      [0, -, -]
        stp
L_dead: hlt
L_step: mov     [>, ., .]
        jmp     L_loop
```

- [ ] **Open** `check.tma`. Confirm syntax colors appear (the `.section` /
      `.routine` / `.row` / `.targets` directives, the mnemonics, the
      `Tscan:` / `L_hit:` labels, the `*` wildcards in the vector operands,
      a `;` comment if you add one) — this is
      `editors/grammars/tma.tmLanguage.json`.
- [ ] **Shadowed row**: confirm a warning on the second `.row` line — the
      `shadowed-wildcard-rows` finding, because `[1, *, *]` already covers
      `[1, 2, *]` in the same match table.
- [ ] **Typo mnemonic**: change `jmp L_loop` to `jpm L_loop`. Confirm a
      squiggle carrying the `unknown-mnemonic` code. **Undo** before
      continuing — a fatal hides lint findings entirely on this side, so
      the next steps need a clean assemble.
- [ ] **Go-to-definition on a table label**: invoke it on the `Tscan`
      operand of `mtc Tscan`. Confirm it jumps to the `Tscan:` label in
      the table section. Repeat on `Dscan` in `djmp Dscan`, and on
      `L_loop` in `jmp L_loop` — table-space and code-space labels both
      resolve.
- [ ] **No hover on `.tma`**: hover over a mnemonic and confirm **nothing**
      appears. This is deliberate and permanent — assembly text has no
      doc-line grammar for a hover to render, so the `.tma` service
      declines hover by design rather than answering emptily.
- [ ] **Completion**: press Ctrl+Space at the start of an instruction line.
      Confirm mnemonics are offered, each with its operand shape as the
      completion detail.
- [ ] **Outline**: open the Outline view. Confirm it shows the function
      `main` alongside the table runs `Tscan` and `Dscan`.
- [ ] **Format Document**: mangle the indentation, then run it. Confirm the
      file snaps back to the canonical column grid — labels at column 0,
      mnemonics at column 8, operands at column 16.
- [ ] **No `unused-label` findings**: add an unreferenced label (e.g.
      `SPARE: nop`) inside `main` and confirm **no** warning appears. The
      arch-agnostic `unused-label` rule is deliberately suppressed on the
      `.tma` path: it cannot see label references made through `.targets`,
      `.target`, or `.exits`, so leaving it on would false-flag every
      dispatch and exit target. The code stays valid in an allow-list.
      Remove the label.
- [ ] **Raw-line paste**: replace the `stp` line with a
      `tmt dis --listing`-shaped row (address, raw hex bytes, resolved
      target — not reassembleable input). Confirm a fatal error with the
      `raw-line` code. Undo the paste.
- [ ] **`.tmc` still works**: switch back to `check.tmc`, still in this same
      window/session. Confirm its diagnostics are still live — opening and
      editing `.tma` documents never perturbed the `.tmc` service. One
      process, two independent language services.

## License

GPL-3.0-or-later, same as the rest of this repository (see `LICENSE`).
