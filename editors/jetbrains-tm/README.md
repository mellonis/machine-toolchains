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
byte-for-byte with the VS Code extension.

One `tmt lsp` process serves both languages. The server routes each open
document to its own language service by file extension, so a `.tmc` file
and a `.tma` file coexist in one session without perturbing each other.

## Requirements

- A `tmt` binary reachable on `PATH`, or pointed to with the settings
  page's binary-path field (below).
- **LSP4IJ**, installed from the JetBrains Marketplace *before* you
  sideload this plugin (below) — a sideloaded plugin does not auto-install
  its own plugin dependencies, so skipping this step leaves the IDE unable
  to load the plugin at all.
- This plugin is version 0.1.0, targeting `tmt` 0.2.0 as its tested
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
the IDE if prompted. Do this *before* sideloading the plugin below: a
sideloaded plugin is installed from a local file, not from the Marketplace,
so the IDE has no opportunity to resolve and auto-install a declared plugin
dependency the way it would for a Marketplace install. Skipping this step
leaves this plugin disabled with an unsatisfied-dependency error. The
shipped build was compiled against LSP4IJ 0.20.1 — a build-time
compatibility check, not a runtime one; a newer 0.x/1.x release should
work unless its own compatibility range excludes this plugin's IntelliJ
Platform baseline (2024.3).

## Build and sideload the plugin

From `editors/jetbrains-tm`, with `JAVA_HOME` pointed at any JDK 17+ — a
JetBrains IDE's own bundled JBR works, e.g. on macOS:

```sh
export JAVA_HOME="$HOME/Applications/<SomeIDE>.app/Contents/jbr/Contents/Home"
./gradlew buildPlugin
```

(Substitute the `.app` for whichever JetBrains IDE Toolbox installed — for
example `RustRover.app`. Any JDK 17 or newer on `PATH`/`JAVA_HOME` works
equally well; the bundled JBR is just a JDK most JetBrains-IDE users
already have on disk without a separate install.)

`buildPlugin` produces `build/distributions/tmc-0.1.0.zip`. Install it:

1. Settings → Plugins → the ⚙ (gear) icon in the top-right of the Plugins
   page → **Install Plugin from Disk…**
2. Pick `build/distributions/tmc-0.1.0.zip`.
3. Restart the IDE when prompted.

The plugin is built against the IntelliJ Platform Community baseline and
references no Ultimate-only APIs — a build-time fact, not a runtime one;
whether it actually works on Community editions has not yet been
observed in a running IDE.

## Settings

**Settings | Tools | tmt** holds three fields:

| Field | Default | Meaning |
|---|---|---|
| tmt binary path | `tmt` | Path (or bare command resolved on `PATH`) to the `tmt` binary. The plugin launches it as `tmt lsp` for the language server, and reuses the same path for run configurations (below). |
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
| Subcommand | One of `compile`, `asm`, `lint`, `run`, selected from a fixed dropdown. |
| Arguments | Free-form, shell-quoting-aware (parsed like a program-arguments field, so quoted strings and spaces behave as expected) — appended after the subcommand verbatim. |
| Working directory | Defaults to the project's base directory. |

Output streams to the Run tool window's console, including the process's
exit code on completion.

The dropdown deliberately does not offer `link` — building a runnable
`.tmx` needs a `tmt compile` (or `tmt asm`) step followed by a `tmt link`
step, and this run-configuration type doesn't model a multi-step pipeline
(the same scope line VS Code's task provider draws: see
`editors/vscode-tm/README.md`'s "full build-and-run pipeline" section for
the equivalent gap there). Produce a `.tmo`/`.tmx` from a terminal (or a
`compile`/`asm`-subcommand run configuration for that half), then point a
`run`-subcommand configuration at the resulting `.tmx` with the
`--tape-block` its program expects.

## Known limitation — Cmd+hover underlines the whole file

On TextMate-backed file types, LSP4IJ ignores the `originSelectionRange`
the server sends with a go-to-definition response, so the Cmd/Ctrl+hover
highlight can underline the entire file rather than just the identifier
under the cursor. Navigation itself is correct — the jump lands on the
right target; only the hover underline is mis-scoped. This is an upstream
LSP4IJ behavior, reported there, and affects the PM-1 plugin identically.

## Manual test checklist

This release has no automated editor end-to-end test — walk this by hand
against a sideloaded plugin and a `tmt` on `PATH`, after any change that
touches the client or the server's editor-facing surface. This mirrors the
VS Code README's checklist shape and scratch files; sideloading both shells
and walking both lists is the intended verification. The server-side
behavior each step exercises is covered by the Rust test suite; what this
checklist adds is that the *client wiring* delivers it into the IDE.

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

- [ ] **Plugin loads**: after the restart, confirm **Settings | Tools |
      tmt** exists and that no unsatisfied-dependency error appears on the
      Plugins page (that error means LSP4IJ is missing — see above).
- [ ] **Open** `check.tmc`. Confirm syntax colors appear (the `alphabet` /
      `routine` / `machine` keywords, the `'x'` glyph literals, the `->`
      rule arrows) — this is the bundled TextMate grammar. Confirm **two**
      squiggles appear without any manual trigger: one on `unusedHelper`
      (`unused-routine`) and one on the `debugger` marker
      (`leftover-debugger`).
- [ ] **LSP4IJ console**: open the **LSP Consoles** tool window and confirm
      a `tmt lsp` server is listed as started for this project, with no
      error traffic. This is the fastest way to distinguish "the binary
      isn't found" from "the server is running but quiet".
- [ ] **Completion**: put the caret at the start of an action, after a
      `->`, and press Ctrl+Space. Confirm the action keywords appear
      (`write`, `move`, `goto`, `call`, `return`, `stop`, `halt`,
      `debugger`) *and* that the in-scope state name `scan` is offered —
      completion is context-aware, not one flat keyword list.
- [ ] **Go-to-definition**: invoke it (Cmd/Ctrl+B, or Cmd/Ctrl+click) on
      `markSpot` in the `call markSpot(...)` line. Confirm it jumps to the
      `routine markSpot(tape t: marks) {` declaration in this file. The
      whole-file underline on hover is the known limitation above — the
      jump itself must land correctly. Navigation is **single-file** in
      this release.
- [ ] **Hover**: hover over `markSpot` at the same call site. Confirm a
      tooltip showing the routine's signature, `routine markSpot(tape t:
      marks)`.
- [ ] **Semantic tokens**: confirm coloring beyond what the TextMate
      grammar alone can give — state names and call targets should read
      distinctly from bare identifiers, which a regex grammar cannot
      resolve.
- [ ] **Structure view**: open the Structure tool window. Confirm it lists
      `marks`, `markSpot`, `unusedHelper`, and `machine`.
- [ ] **Settings-driven allow-list, live**: put `leftover-debugger` in the
      settings page's Lint allow-list and click Apply. Confirm the
      `debugger` squiggle disappears **without** restarting the IDE or the
      server, and that the `unused-routine` squiggle stays. Clear the field
      and Apply again; confirm the squiggle returns.
- [ ] **Config file-watch — `tmt.json` on disk**: create a `tmt.json` next
      to the scratch file containing
      `{"lint": {"allow": ["leftover-debugger"]}}`. Confirm the `debugger`
      squiggle disappears **without touching the settings page** — this is
      the server's watch firing on the on-disk file. Delete `tmt.json` and
      confirm the squiggle returns.
- [ ] **Opt-in lint**: add `state-may-trap` to the settings page's Opt-in
      lint rules field and Apply. Confirm new findings appear live. Clear
      it again before continuing.
- [ ] **Quickfix from a fatal**: change `goto scan` on the last rule to
      `goto missing` and confirm one fatal squiggle appears
      (`undefined-state`). Invoke the intention/quick-fix popup
      (Alt+Enter) on it and confirm a **declare state `missing`** action is
      offered; apply it and confirm a `state missing { [*] -> stop; }` stub
      is inserted with the right tape arity. Undo.
      (`.tmc` lint findings themselves carry no machine-applicable fixes in
      this release — every quickfix on this side is derived from a compiler
      fatal. This is expected, not a gap in the wiring.)
- [ ] **Reformat Code**: run it (Cmd/Ctrl+Alt+L). Confirm the state block
      snaps to its canonical grid — the `->` arrows aligned down the
      block — and that nothing but whitespace changes.
- [ ] **Run configuration — lint**: create a `tmt` run configuration with
      subcommand `lint` and the scratch file's path as the argument. Run
      it and confirm both findings appear in the console with a non-zero
      exit code.
- [ ] **Dogfood — the embedded standard library**: open
      `crates/turing-machine/src/stdlib/std.tmc` from this repository
      directly. Confirm **zero diagnostics**, that semantic tokens are
      visible, and that **Reformat Code** is a **no-op** — the checked-in
      file is already canonically formatted.

### `.tma` checklist

`tmt lsp` serves `.tma` through the same process and connection as `.tmc`
above — walk this checklist in the same IDE session, without restarting
anything, so the last step has something to confirm.

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
      `Tscan:` / `L_hit:` labels, the `*` wildcards in the vector
      operands).
- [ ] **Shadowed row**: confirm a warning on the second `.row` line — the
      `shadowed-wildcard-rows` finding, because `[1, *, *]` already covers
      `[1, 2, *]` in the same match table.
- [ ] **Typo mnemonic**: change `jmp L_loop` to `jpm L_loop`. Confirm a
      squiggle carrying the `unknown-mnemonic` code. **Undo** before
      continuing — a fatal hides lint findings entirely on this side, so
      the next steps need a clean assemble.
- [ ] **Go-to-definition on a table label**: invoke it on the `Tscan`
      operand of `mtc Tscan`. Confirm it jumps to the `Tscan:` label in the
      table section. Repeat on `Dscan` in `djmp Dscan`, and on `L_loop` in
      `jmp L_loop` — table-space and code-space labels both resolve.
- [ ] **No hover on `.tma`**: hover over a mnemonic and confirm **nothing**
      appears. This is deliberate and permanent — assembly text has no
      doc-line grammar for a hover to render, so the `.tma` service
      declines hover by design rather than answering emptily.
- [ ] **Completion**: press Ctrl+Space at the start of an instruction line.
      Confirm mnemonics are offered, each with its operand shape as the
      completion detail.
- [ ] **Structure view**: confirm it shows the function `main` alongside the
      table runs `Tscan` and `Dscan`.
- [ ] **Reformat Code**: mangle the indentation, then run it. Confirm the
      file snaps back to the canonical column grid — labels at column 0,
      mnemonics at column 8, operands at column 16.
- [ ] **No `unused-label` findings**: add an unreferenced label (e.g.
      `SPARE: nop`) inside `main` and confirm **no** warning appears. The
      arch-agnostic `unused-label` rule is deliberately suppressed on the
      `.tma` path: it cannot see label references made through `.targets`,
      `.target`, or `.exits`, so leaving it on would false-flag every
      dispatch and exit target. The code stays valid in an allow-list.
      Remove the label.
- [ ] **`.tmc` still works**: switch back to `check.tmc`, still in this same
      IDE session. Confirm its diagnostics are still live — opening and
      editing `.tma` documents never perturbed the `.tmc` service. One
      process, two independent language services.

## License

GPL-3.0-or-later, same as the rest of this repository (see the repository
root's `LICENSE`).
