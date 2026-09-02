# TMC for VS Code — maintainer notes

Building the extension from source and the manual verification
checklist that runs against a built `.vsix`. The user-facing page is
`README.md`; this file is excluded from the packaged extension.

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
      by `copy-assets.js`. Confirm **two** squiggles appear without any
      manual trigger: one on `unusedHelper` (`unused-routine`) and one on
      the `debugger` marker (`leftover-debugger`).
- [ ] **Completion**: put the cursor at the start of an action, after a
      `->`, and press Ctrl+Space. Confirm the action keywords appear
      (`write`, `move`, `goto`, `call`, `return`, `stop`, `halt`,
      `debugger`) *and* that the in-scope state name `scan` is offered —
      completion is context-aware, not one flat keyword list.
- [ ] **Go-to-definition**: invoke it on `markSpot` in the
      `call markSpot(...)` line. Confirm it jumps to the
      `routine markSpot(tape t: marks) {` declaration in this file. This
      checks local resolution only — the cross-file and standard-library
      cases have their own steps below (**Cross-file overlay and the
      standard-library bridge**).
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

### Cross-file overlay and the standard-library bridge

`tmt lsp` resolves names against a project's declared siblings and
libraries — and against the embedded standard library — for a document
that belongs to a target declared in a `tmt.json` project file
(`docs/lsp.md` in this repository, "Cross-file resolution (the project
overlay)"). Walk this in its own fresh scratch directory, separate from
`check.tmc` above, so its `tmt.json` never interacts with the file-watch
step earlier.

Create `tmt.json`:

```json
{
  "project": {
    "targets": {
      "app": { "sources": ["overlay.tmc", "sibling.tmc"] }
    }
  }
}
```

Create `sibling.tmc`:

```tmc
alphabet marks { '_', 'x', 'y' }

export routine sweep(tape t: marks) {
  entry state s {
    [*] -> return;
  }
}
```

Create `overlay.tmc`:

```tmc
alphabet marks { '_', 'x', 'y' }
alphabet bits { '_', '0', '1' }

use std::binaryNumbersBare::plusOne;

export routine useSibling(tape t: marks) {
  entry state go {
    [*] -> call sweep() then go;
  }
}

export routine useStd(tape num: bits) {
  entry state go {
    [*] -> call plusOne() then go;
  }
}
```

- [ ] **Open** `overlay.tmc`. Confirm **no** `undeclared-external` warning
      on `sweep` in `call sweep() then go;` — `sibling.tmc` is a declared
      source of the same target, so the overlay resolves the bare call
      before the compile warning ever fires. (Opening this file with no
      `tmt.json` above it would warn on `sweep`; that is the single-file
      behavior the overlay adds to.)
- [ ] **Go-to-definition — sibling**: invoke it on `sweep` in
      `call sweep() then go;`. Confirm it jumps into `sibling.tmc`,
      landing on `export routine sweep(tape t: marks) {` — a real
      cross-file jump, unlike the local-only case checked above.
- [ ] **Go-to-definition — standard library**: invoke it on `plusOne`,
      either in `use std::binaryNumbersBare::plusOne;` or inside
      `call plusOne() then go;`. Confirm it jumps into a materialized
      `std.tmc` — a cached copy outside this workspace, not a file you're
      editing — landing on `export routine plusOne(tape num: symbols) {`
      inside `namespace binaryNumbersBare`. See `docs/lsp.md` in this
      repository for where that cache lives.
- [ ] **Hover — standard library**: hover over `plusOne` at either
      reference. Confirm a tooltip showing its signature line and doc
      text ("Add one to a number…") — this is the `.tmc` standard-library
      bridge (`docs/lsp.md`, "The `.tmc` standard-library bridge").

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
- [ ] **`unused-label` resolves table references**: add a genuinely
      unreferenced label (e.g. `SPARE: nop`) inside `main` — nothing
      points to it via a jump, a call, a `.targets`/`.target` entry, or
      an `.exits` list. Confirm a warning appears (`unused-label`) — this
      arch-agnostic rule runs unmodified on `.tma`, with no suppression.
      Remove `SPARE`, then confirm `L_hit`, `L_dead`, and `L_step` —
      each reached only through `Dscan`'s `.targets` entry, never by a
      jump or call operand — carry **no** `unused-label` warning: the
      rule counts a dispatch or exit target as a reference, so it never
      false-flags the labels a table's own entries name.
- [ ] **Raw-line paste**: replace the `stp` line with a
      `tmt dis --listing`-shaped row (address, raw hex bytes, resolved
      target — not reassembleable input). Confirm a fatal error with the
      `raw-line` code. Undo the paste.
- [ ] **`.tmc` still works**: switch back to `check.tmc`, still in this same
      window/session. Confirm its diagnostics are still live — opening and
      editing `.tma` documents never perturbed the `.tmc` service. One
      process, two independent language services.

### Debug session

Not yet walked by hand — live verification of the `tmt` debugger type
is the maintainer's step, same as the rest of this checklist.

Create a scratch `debug.tmc`, tape block, and `tmt.json` in the same
folder:

```tmc
alphabet marks { '_', 'x' }

machine {
  tape work: marks;

  entry state go {
    [*] -> write ['x'] move [>] goto step;
  }

  state step { [*] -> write ['x'] stop; }
}
```

```sh
tmt tape-block new --from debug.tmc -o debug.tmt
```

```json
{ "project": { "targets": { "main": { "sources": ["debug.tmc"], "run": { "tape": "debug.tmt" } } } } }
```

- [ ] **Launch, breakpoint, tape, setVariable, disassembly, step**: add
      a "tmt: launch target" configuration (Run and Debug → create a
      launch.json, or the **tmt: Launch target** snippet) with `target:
      "main"` and `stopOnEntry: true`, then press F5. Confirm the
      session stops before the first instruction. Set a source
      breakpoint on the `state step { ... }` line and **Continue**;
      confirm it stops there with a "breakpoint" reason (not "entry").
      Open the Variables view, expand the **Tapes** scope, and expand
      `tape 0`; confirm cell `[0]` already reads `'x'` (from `go`'s
      write) while the current cell — named `» [1]` — still reads `'_'`.
      Edit `[1]`'s value there (`setVariable`) to `x` and confirm the
      tape view updates to match immediately. Right-click the stack
      frame (or use the Command Palette) and **Open Disassembly View**;
      confirm TM-1 instructions (`ent`/`wrmv`/`stp`) with addresses are
      listed. **Step Over** once — this is the program's last rule, so
      confirm it runs to completion and the session terminates cleanly
      (no crash on the already-marked, edited cell).

