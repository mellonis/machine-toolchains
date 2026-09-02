# PMC for VS Code — maintainer notes

Building the extension from source and the manual verification
checklist that runs against a built `.vsix`. The user-facing page is
`README.md`; this file is excluded from the packaged extension.

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

