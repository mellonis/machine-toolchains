# TMC for JetBrains IDEs — maintainer notes

Building the plugin from source, how it owns its files, and the manual
verification checklist that runs against a built plugin zip. The
user-facing page is `README.md`.

## Build and sideload the plugin

From `editors/jetbrains-tm`, with `JAVA_HOME` pointed at a JDK capable of
running Gradle itself — a JetBrains IDE's own bundled JBR works, e.g. on
macOS:

```sh
export JAVA_HOME="$HOME/Applications/<SomeIDE>.app/Contents/jbr/Contents/Home"
./gradlew buildPlugin
```

(Substitute the `.app` for whichever JetBrains IDE Toolbox installed — for
example `RustRover.app`; the bundled JBR is just a JDK most JetBrains-IDE
users already have on disk without a separate install.) The compilation
itself always targets a pinned JDK 17 toolchain (`kotlin {
jvmToolchain(17) }` in `build.gradle.kts`), regardless of which JDK
`JAVA_HOME` points at — Gradle auto-provisions JDK 17 via the
`foojay-resolver-convention` plugin (`settings.gradle.kts`) if the
`JAVA_HOME` JDK doesn't already supply one. This has been verified
building under a JetBrains-bundled JBR newer than 17 (JBR 25); other
JDKs as `JAVA_HOME` are untested.

`buildPlugin` produces `build/distributions/tmc-0.2.1.zip`. Install it:

1. Settings → Plugins → the ⚙ (gear) icon in the top-right of the Plugins
   page → **Install Plugin from Disk…**
2. Pick `build/distributions/tmc-0.2.1.zip`.
3. Restart the IDE when prompted.

The plugin is built against the IntelliJ Platform Community baseline and
references no Ultimate-only APIs — a build-time fact, not a runtime one;
whether it actually works on Community editions has not yet been
observed in a running IDE.

## How the plugin owns its files

Both file types are language file types backed by a deliberately
minimal parser: the whole text splits into word, whitespace and
punctuation tokens under one file node, and nothing reads that tree for
meaning — `tmt lsp` owns every semantic. The parser exists for two
reasons the platform imposes. A language file type without one gets a
plain-text PSI file whose language disagrees with the file's own, and
LSP4IJ then never connects the opened document to the server (no hover,
no navigation, silently). And the platform underlines the PSI element
under the caret on Cmd/Ctrl+hover, so word-grained tokens are what make
that underline cover just the identifier. The plugin also registers,
for its own languages, the per-language LSP4IJ extensions LSP4IJ itself
registers only for plain text and TextMate (the semantic-token view
provider behind the Cmd/Ctrl+hover quick-doc, the Structure view from
document symbols, folding, code blocks, parameter info).

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
      `routine markSpot(tape t: marks) {` declaration in this file, and
      that the Cmd/Ctrl+hover underline covers just `markSpot`, not the
      whole file. This checks local resolution
      only — the cross-file and standard-library cases have their own
      steps below (**Cross-file overlay and the standard-library
      bridge**).
- [ ] **Hover**: hover over `markSpot` at the same call site. Confirm a
      tooltip showing the routine's signature, `routine markSpot(tape t:
      marks)`.
- [ ] **Semantic tokens**: confirm coloring beyond what the TextMate
      grammar alone can give — state names and call targets should read
      distinctly from bare identifiers, which a regex grammar cannot
      resolve.
- [ ] **Semantic colors**: in a `.tmc` file, state and routine names render
      colored (not underline-only) under both light and dark themes.
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

### Cross-file overlay and the standard-library bridge

`tmt lsp` resolves names against a project's declared siblings and
libraries — and against the embedded standard library — for a document
that belongs to a target declared in a `tmt.json` project file
(`docs/lsp.md` in this repository, "Cross-file resolution (the project
overlay)"). Walk this in its own fresh scratch directory, separate from
`check.tmc` above, so its `tmt.json` never interacts with the settings
or file-watch steps earlier.

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
- [ ] **Go-to-definition — sibling**: invoke it (Cmd/Ctrl+B, or
      Cmd/Ctrl+click) on `sweep` in `call sweep() then go;`. Confirm it
      jumps into `sibling.tmc`, landing on
      `export routine sweep(tape t: marks) {` — a real cross-file jump,
      unlike the local-only case checked above.
- [ ] **Go-to-definition — standard library**: invoke it on `plusOne`,
      either in `use std::binaryNumbersBare::plusOne;` or inside
      `call plusOne() then go;`. Confirm it jumps into a materialized
      `std.tmc` — a cached copy outside this workspace, not a file
      you're editing — landing on
      `export routine plusOne(tape num: symbols) {` inside
      `namespace binaryNumbersBare`. See `docs/lsp.md` in this repository
      for where that cache lives.
- [ ] **Hover — standard library**: hover over `plusOne` at either
      reference. Confirm a tooltip showing its signature line and doc
      text ("Add one to a number…") — this is the `.tmc` standard-library
      bridge (`docs/lsp.md`, "The `.tmc` standard-library bridge").

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
- [ ] **`.tmc` still works**: switch back to `check.tmc`, still in this same
      IDE session. Confirm its diagnostics are still live — opening and
      editing `.tma` documents never perturbed the `.tmc` service. One
      process, two independent language services.

### Target-build & debug checklist

New in plugin 0.2.0 — the run-config `build` subcommand and the whole
DAP bridge below. Needs a `tmt` at 0.4.0 or newer (the release that
introduced the `dap` subcommand). In the scratch project from the
`.tmc` checklist
above, mint an input tape and add a `tmt.json` next to `check.tmc`:

```sh
tmt tape-block new --from check.tmc -o check-in.tmt
```

```json
{
  "project": {
    "targets": {
      "check": {
        "sources": ["check.tmc"],
        "run": { "tape": "check-in.tmt" }
      }
    }
  }
}
```

(Schema reference: `docs/tmt/project.md`; fill the tape's cells per
`docs/tmt/cli.md`'s `tape-block` section if `check.tmc` expects specific
input.)

- [ ] **Manifest schema**: with the `tmt.json` from above open, confirm
      the bottom-right schema widget shows the bundled `tmt` schema and
      that an unknown key (e.g. `"bogus": 1` inside the target) gets a
      validation warning; remove it after.
- [ ] **Target listing**: add a `tmt` run configuration, subcommand
      `build`. Press **Refresh** next to the Target field — the combo
      fills with `check` and the status line reports one target with a
      run block. Break the manifest (temporarily rename the `sources`
      key), refresh again, and confirm the driver's own error lands in
      the status line instead of the list silently emptying; restore the
      manifest.
- [ ] **Build**: select target `check` and run the configuration.
      Confirm the console shows the driver's build output and exit code
      0, and a `.tmx` appears where the manifest's output settings put
      it.
- [ ] **Build --run**: check *Run the target after building* and run
      again. Confirm the build is followed by the run against
      `check-in.tmt` in the same console session. Then temporarily drop
      the target's `run` block and confirm the driver's own
      not-runnable error surfaces in the console (TM-1 has no
      empty-tape default); restore it.
- [ ] **Breakpoint + launch target**: set a gutter breakpoint on an
      executable `.tmc` line in `check.tmc`. Create the DAP run
      configuration per the Debugging section (server **tmt dap**,
      command blank, template *tmt: launch target* with `"target":
      "check"`), start it with Debug, and confirm the session stops on
      the breakpoint with the editor line highlighted.
- [ ] **Machine state**: while stopped, confirm the variables view shows
      the machine scopes `docs/dap.md` describes (registers; one scope
      per tape), and that stepping advances the highlighted line.
- [ ] **stopOnEntry + program mode**: switch the launch JSON to the
      *tmt: launch program* template pointing at the `.tmx` built above
      and `check-in.tmt` (keep `"stopOnEntry": true` — and keep
      `"tape"`: program mode requires it), restart, and confirm the
      session stops before the first instruction.
- [ ] **Blank-command fallback**: confirm the *Command* field of the DAP
      configuration is still blank and the session nevertheless launched
      the settings-page binary (the fallback documented in the Debugging
      section) — then set an explicit command `tmt dap` and confirm that
      still works too.

