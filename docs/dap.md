# The debug adapters

This repository ships **two** Debug Adapter Protocol servers, one per
toolchain, each debugging one running machine over stdio:

| Command | Debugs | Manifest (target mode) |
|---|---|---|
| `pmt dap` | a `.pmx` PM-1 executable | `pmt.json` |
| `tmt dap` | a `.tmx` TM-1 executable | `tmt.json` |

Both are built on `mtc_core::dap`, the sibling of the LSP framework
(`docs/core.md`, `docs/lsp.md`): the two share exactly the
`Content-Length` framing codec extracted for this purpose, and nothing
above it — DAP's envelope, loop shape, and lifecycle all diverge enough
from LSP's synchronous request/response dance that the frameworks stay
separate by design, not by omission. See `docs/pmt/cli.md` (`pmt dap`)
and `docs/tmt/cli.md` (`tmt dap`) for each subcommand's own usage text
and process exit codes.

The two adapters are independent implementations of one framework
trait, mirroring `PmDapAdapter`'s shape function-for-function apart from
where PM-1's single-tape, boolean-flag model and TM-1's multi-tape,
general-register model genuinely differ. Everything below is common
unless a toolchain difference is named explicitly.

## The framework

A server is a blocking loop: one background thread owns stdin, decoding
`Content-Length` frames onto a channel; the main loop owns dispatch and
stdout, and is the only place `seq` numbers are minted. Unlike an LSP
session — always waiting on the next client message — a DAP session can
be *running*, advancing the debuggee on its own between requests: while
the adapter reports itself running, the main loop alternates a
non-blocking drain of the reader channel with one bounded `tick()` call,
so a queued `pause` or `setBreakpoints` is noticed within one slice
rather than waiting for a long or non-terminating run to yield on its
own. A slice is a fixed instruction budget (10,000 steps — sub-millisecond
for anything either compiler produces) and exhausting it without pausing
is invisible to the client: no event, no state change, just another
`tick`. The CLI's own multi-million-step run ceiling does not apply to
an interactive session — a human `pause` button is the real limit.

**Lifecycle.** Every request but `initialize` is rejected until
`initialize` has succeeded; every request, including a repeat
`disconnect`, is rejected once `disconnect` has succeeded — the session
stays alive just long enough to say no to a straggler, never to do more
work. A payload that fails to decode as a request (malformed JSON, or a
response/event — v1 has no reverse requests, so the client never sends
either) is dropped silently: there is no `request_seq` to answer against.
An unrecognized command answers `unrecognized request: '<command>'`,
worded identically across both adapters.

**Threads.** The debuggee is always exactly one thread — `threads`
answers `{id: 1, name: "machine"}`, and every `stopped` event carries
`threadId: 1` and `allThreadsStopped: true`. Both are stamped by the
framework itself, not supplied by the per-arch adapter: the single-thread
model is a framework-wide commitment, not per-machine data.

**Two exit-code spaces, easy to conflate.** The `pmt dap`/`tmt dap`
*process* exits 0 after a clean `disconnect` response was sent, 1 if the
transport ends before one ever was — the same convention `pmt lsp`/
`tmt lsp` use. Separately, the debuggee's own termination is reported on
the wire as an `exited` event carrying an `exitCode` of 0 (stopped), 2
(halted), or 3 (trapped) — the exact mapping `pmt run`/`tmt run` already
use for their own process exit codes. The two numbers answer different
questions (did the DAP session end cleanly; how did the machine finish)
and share no relationship beyond both borrowing the CLI's own
stopped/halted/trapped vocabulary.

## Launch

`launch` recognizes two mutually exclusive shapes, named by which
argument the request carries; giving both is a clean launch-time error
naming the conflict, never a silent "target wins."

**Target mode** — `"target": "<name>"`, plus an optional `"project"`
path overriding the manifest-discovery starting point. The named
`pmt.json`/`tmt.json` target builds **in process**, through the same
driver `pmt build TARGET`/`tmt build TARGET` itself runs — never
shelling out to a subprocess — and **always with `-g` forced**,
regardless of the target's own resolved profile: a debug session with no
line map is crippled, so this is not left to the manifest's discretion.
Compile diagnostics stream as `stderr`-category `output` events, one per
diagnostic line, before `initialized`; a failed build fails the `launch`
request and pushes nothing else. The tape comes from the target's own
`run` settings, resolved through the exact rules `pmt build --run`/
`tmt build --run` use — this surface does not reimplement tape
resolution, it calls the same code. TM-1 additionally enforces its own
tape-only run-block rule here (next paragraph) at launch time, before any
event is pushed.

**Program mode** — `"program": "<path>"`, a prebuilt executable used
as-is, plus a toolchain-dependent tape argument (below). Nothing about
program mode injects `-g`: the executable's own map sidecar — always
written by `pmt link`/`tmt link`, whether or not `-g` was used, carrying
at minimum every function's address range — decides what is possible.
Built with `-g`, breakpoints resolve to source lines and stepping
degrades gracefully; without it, function ranges are still known but no
line is, with consequences spelled out under **Breakpoints and stepping**
below.

**Tape resolution differs by toolchain, matching each `run` subcommand's
own default.** PM-1 defaults to the empty tape when `"tape"` is omitted
in program mode, mirroring `pmt run`'s own default; program mode
additionally accepts `"strictCells": true`, the same semantics as
`pmt run --strict-cells` (TM-1 has no CLI equivalent to mirror, so
`tmt dap` does not invent one). TM-1 has no empty-tape default at all —
`tmt run` itself always requires `--tape-block` — so a TM-1 program-mode
launch with no `"tape"` argument is a clean launch-time error, the same
shape as any other pre-`initialized` failure. In target mode neither
toolchain accepts a `"tape"`/`"strictCells"` argument of its own: the
tape always comes from the target's `run` block, and for TM-1 a target
with no `run` block, or a `run` block that declares no `tape`, cannot be
launched — the same two guard conditions `tmt build --run` itself
enforces, sharing one helper with it.

**Common to both modes:** `"stopOnEntry": bool` (default `false` — pause
before the very first instruction, mirroring the real client sequence of
`launch` → `initialized` → configuration requests → `configurationDone`,
where a client that omits `stopOnEntry` never sends its own `continue` —
`configurationDone` starts the run directly in that case) and
`"trace": bool` (default `false` — the opt-in per-instruction stream
under **Output events**, below).

## Protocol surface

**v1 commands:** `initialize`, `launch`, `setBreakpoints`,
`setInstructionBreakpoints`, `configurationDone`, `threads`,
`stackTrace`, `scopes`, `variables`, `setVariable`, `continue`, `next`,
`stepIn`, `stepOut`, `pause`, `disassemble`, `disconnect`.
**v1 events:** `initialized`, `stopped`, `output`, `terminated`,
`exited`. **Capabilities declared:** `supportsConfigurationDoneRequest`,
`supportsSetVariable`, `supportsSteppingGranularity`,
`supportsDisassembleRequest`, `supportsInstructionBreakpoints` — every
other DAP capability answers `false` or is simply absent, so `evaluate`,
watch expressions, conditional/hit-count/logpoint breakpoints, data and
function breakpoints, `attach` (there is no external process to attach
to), variable paging beyond the fixed window described below, and
`restart` are all out of scope for this surface, not partially wired.

## Run control and pause mapping

`continue`, `pause`, `next`/`stepIn`/`stepOut` (**Breakpoints and
stepping**, below), and the driving `tick` all funnel through one
`PauseCause`-to-`stopped`-reason mapping:

| Cause | `stopped` reason | `description` |
|---|---|---|
| a planted address (source or instruction breakpoint) | `"breakpoint"` | absent |
| a `debugger`/debug-break instruction retired | `"breakpoint"` | `"debugger statement"` |
| a stepping command completed | `"step"` | absent |
| a client `pause` request | `"pause"` | absent |
| `stopOnEntry`, before the first instruction runs | `"entry"` | absent |
| a trap | `"exception"` | the trap's own text |

A trap is never surfaced as an adapter error: state stays fully
inspectable at the fault (stack, tapes, registers), and it is only
**further** stepping or `continue` that drives the session the rest of
the way to termination — the two-phase shape a trapping program always
goes through. Program termination (a natural stop, a halt, or a trap
driven to completion) pushes a summary `output` event, then `terminated`,
then `exited` with the exit-code mapping above; the summary line and the
mapping are byte-identical to what `pmt run`/`tmt run` print for the same
outcome.

## Output events

Exactly three kinds exist, and nothing else does — no `stdout` category
is ever sent (the machine has no output channel of its own; the tape is
the result), no `telemetry`, and a pause's own annotation rides
`stopped.description` rather than a separate line:

1. **Build diagnostics** during a target-mode launch — `stderr`
   category, one event per diagnostic line, streamed before
   `initialized`.
2. **One termination summary** — `console` category, immediately before
   `terminated`: the outcome, steps, and core/stall tacts, the same
   numbers `pmt run`/`tmt run` print.
3. **Opt-in trace lines** under `"trace": true` — `console` category,
   one event per retired instruction, byte-identical to `pmt run --trace`/
   `tmt run --trace`'s own lines (TM's carries every tape's head instead
   of PM's single `head=N`, and an `FR=<n>` suffix on a frames-profile
   image — the exact conditions `tmt run --trace` itself uses).

Trace parity with `run --trace` includes the terminal line — the last
instruction retired before a `stp`/`hlt`/trap prints too, not just every
instruction up to it — with one nuance the CLI path never has to handle:
a trap's two-phase flow (above) means a *second* `continue` can call the
same step primitive again after the session has already privately
finished at the fault. Trace mode detects this and emits nothing on that
second call, so the faulting instruction's trace line is never printed
twice.

## Breakpoints and stepping

`setBreakpoints` and `setInstructionBreakpoints` are independent
kinds — DAP REPLACE semantics apply per kind, so a fresh
`setBreakpoints` call never disturbs instruction breakpoints, and vice
versa. Within the source kind, replacement follows DAP's own per-source
contract: a request's list is the whole new set *for the file it names*,
so a client holding breakpoints in two files (one `setBreakpoints` call
each, as real clients send them) keeps both sets planted. A request
resolved without a file match replaces the one global list instead.

When the map carries source provenance (see **Source provenance**
below), a `setBreakpoints` request's own `source.path` is matched
against the map's source records and the line search is confined to
that one file's functions — so two translation units that share a line
number can no longer capture each other's breakpoints. A request naming
a file the map's records never mention answers every line
`verified: false` with its own message (`"no code in this program comes
from this file (per the map sidecar's source records)"`) rather than
falling back to the global table — a fallback would plant an unrelated
file's identical line numbers, the exact collision the filter exists to
prevent. A map with no provenance at all (a pre-provenance sidecar, or
one written by `pmt link`/`tmt link` over prebuilt objects) keeps the
old behavior: one global line table, resolved regardless of which file
the request named.

A source breakpoint resolves through the map's line table; an unmapped
line — no `-g` map at all, or a specific line the map simply has no code
for — answers `verified: false` with a message pointing at the fix
(`"no code at this line — build with -g and place the breakpoint on an
executable line"`), worded identically on both toolchains. A verified
entry's `line` is the *resolved* line (the snapping rule may plant later
than requested) and it carries an `instructionReference` naming the
planted address. An instruction breakpoint needs no map at all — any
address that parses as hex is legal to plant.

**Stepping granularity.** Line granularity (the default, and DAP's own
`"statement"`) repeats the underlying instruction step until the
resolved `(function, line)` position differs from where the step began —
a transition into code the map has no line entry for counts as a change
too, so a step can never silently swallow a whole function that carries
no line data of its own while its neighbours do. The comparison is the
*whole* position, function name included, not the line number alone:
two independently compiled sources each restart their own line numbering
at 1, so a return from one file's line 2 into a different file's line 2
is a real change that a line-only comparison would misread as "no
motion." A breakpoint, a debug-break instruction, or a trap encountered
partway through either granularity interrupts it immediately and reports
its own reason instead of `"step"`. `granularity: "instruction"` — which
VS Code sends automatically once its own Disassembly view has focus —
always advances exactly one retired instruction, independent of map
state. `stepOut` has no separate granularity of its own: "return to the
caller" already names its own stopping point. At the outermost frame it
falls back to running the program to completion rather than erroring,
inheriting that behavior from the underlying session primitive unchanged.

**Degradation without `-g`.** A linked executable always carries a map
naming every function's address range, `-g` or not — only the *line*
table inside each function is empty without it. Line-granularity
stepping over such a function therefore stops only at a function
boundary (a call or a return), not at every instruction: a
single-function program with no calls runs to completion (or to a
breakpoint) on the very first default-granularity step, not to the next
instruction. `granularity: "instruction"` is the one setting that is
exactly one instruction regardless of map state — a client stepping a
`-g`-less program-mode launch should request it explicitly rather than
rely on the default. (Target mode never hits this: `-g` is always
forced.) An executable with no map sidecar at all on disk — one deleted
after linking, or produced by something other than this toolchain's own
linker — degrades one step further: no address resolves to anything, so
even the function-boundary stop never fires, and default-granularity
stepping runs unconstrained to the next breakpoint or to completion.

## The Disassembly view

`disassemble` renders instruction-listing text through the same renderer
`dis` and `run --trace` already use, with sidecar label resolution where
one exists — so a `disassemble` response and a `pmt dis`/`tmt dis`
listing never disagree about how an instruction reads. Every stack frame
carries an `instructionPointerReference` (a hex address), which is what
the Disassembly view resolves to highlight the current instruction and
track it across every step. `setInstructionBreakpoints` maps the view's
own breakpoint gutter directly onto the session's address breakpoints —
the same underlying set a source breakpoint's resolved address is
planted into, so a breakpoint set through either request pauses
execution identically, regardless of which surface set it.

The response window is strictly **positional**: row `i` is the
instruction `instructionOffset + i` places from the referenced one,
never shifted or truncated. That is a client contract, not a nicety — a
client learns a previously unseen reference's memory address from the
row at index `-instructionOffset` of the response, so an adapter that
slides a head-overflowing window to the image start teaches it a wrong
address for every reference within `-instructionOffset` instructions of
the entry, and the view's current-instruction marker pins to one late
address no matter where execution actually is. Positions the image has
no instruction for carry a placeholder instead (`instruction:
"<out of range>"`, `presentationHint: "invalid"`): past the last
instruction the placeholder addresses continue one byte at a time from
the code end, and *before* the first instruction they are negative —
skipping `-1` itself, the one value clients treat as an
ignore-this-row sentinel. Placeholder or real, every address in a
response is strictly increasing and never repeats, since a Disassembly
view routinely prefetches windows past the loaded code and a repeated
address across rows would be a real, visible glitch rather than a
hypothetical one.

## Assembly-line debugging

A hand-assembled `.pma`/`.tma` file, launched via program mode, carries
its own `-g` line table naming *its own* lines — an assembly source is a
legitimate debugging target in its own right, with breakpoints and
stepping resolving directly against it, no `.pmc`/`.tmc` file involved
at all. Only the compiled path remaps a `.pma`/`.tma` line to the
`.pmc`/`.tmc` source line that produced it; assembling a file directly
skips that remap; and its address ranges and lines resolve exactly the
same way through the same map either way.

## Source provenance

A map written by `pmt build`/`tmt build` records, per function, the
source file its defining input was built from (`docs/formats.md` — the
sidecar's source-provenance field). The adapters put that record to two
uses: the per-file breakpoint filter above, and a DAP `source` object —
`{ "name": <file leaf>, "path": <absolute file> }` — attached to every
stack frame whose function carries provenance. The `source` object is
what lets a client treat a frame as *openable*: focus it automatically
on a stop, reveal the file in the editor, and highlight the current
line. A frame whose function has no provenance (a prebuilt object, the
embedded stdlib, a pre-provenance sidecar) still carries
`name`/`line`/`instructionPointerReference`, exactly as before — it is
usable in the Disassembly view, just not openable as a file.

Resolution back from a sidecar's stored (typically relative) path to
the absolute one handed to the client anchors at the sidecar's own
directory and is purely lexical, the same policy the emission side uses
(`docs/formats.md`). Two guards temper it. A resolved file that does
not exist on disk — a tree moved after building, a sidecar copied
without its sources — omits the `source` object rather than handing the
client a dead path, degrading to the sourceless frame above. And for
the breakpoint filter's *comparison* (matching the editor's request
path against a resolved record), both sides are canonicalized through
the filesystem when the file exists, so a symlinked workspace — where
the editor and the sidecar legitimately spell one file two ways — still
matches; only a path that cannot be canonicalized falls back to the
lexical spelling. That comparison is the one place the adapters go
beyond the LSP overlay's lexical-only identity (`docs/lsp.md`), because
a debug session compares paths from two independent producers rather
than its own URIs round-tripped.

## State: threads, stack, scopes, variables

`stackTrace` answers frame 0 as the current instruction pointer, then
older frames from the session's own return-address stack, most recent
call first. Every frame resolves through the map: a resolvable address
names its containing function (and, if the function's line table covers
it, a source line); an unresolvable one — no map at all, or an address
outside every known function — falls back to its own hex address as the
name, line 0. Every frame carries `instructionPointerReference`
regardless, so a frame stays usable in the Disassembly view even when it
resolves to nothing else. A frame whose function record names its source
file additionally carries the DAP `source` object — see **Source
provenance** above for what it enables and when it is omitted.

**Scopes are identical for any selected frame** — machine state is
global, not per-frame — so `scopes` never inspects which frame id was
requested. Two scopes always answer: **Registers** and **Tapes**.

- **Registers** — PM-1: `IP` (hex, read-only), `MF` (writable). TM-1:
  `IP` (hex, read-only), `MR` (writable, the general-register view of the
  same one flag PM-1's `MF` names as a boolean), and `FR` (read-only,
  present *only* on a frames-profile image — a base-profile image shows
  no `FR` variable at all, matching the same condition `tmt run --trace`
  uses for its own conditional `FR=` suffix). Both toolchains append
  read-only `steps`, `core tacts`, `stall tacts`.
- **Tapes** — one child per tape (`"tape 0"`, `"tape 1"`, … — PM-1
  always has exactly one; TM-1 has as many as the launched image
  declares). Expanding a tape reveals a fixed head±8 window (17 cells),
  each rendered `[position]` with the head's own cell prefixed `» `, and
  a quoted glyph (`'x'`) resolved from the launch tape block's alphabet
  where one was given, or the raw cell index otherwise. Two different
  tapes render through their own alphabets independently — nothing is
  shared across tapes.

**The `variablesReference` handles are fixed constants for the life of a
session** — the Registers scope, the Tapes scope, and each tape's own
window always resolve to the same handle, launch to launch and step to
step. A client is free to cache them after the first `scopes`/`variables`
round trip without re-querying on every stop; nothing about a handle's
meaning ever depends on which frame, which pause, or how many steps have
run.

## Writable state

`supportsSetVariable: true`; a set is accepted only while the session is
genuinely paused (not running, and not after termination — though the
tape itself stays *readable* after termination, since a poked cell has
to remain visible for its own persistence to be provable).

- **Tape cells**, via the tape trait's own `poke(position, glyph)`: walk
  the head to the position, write, walk back — including on a fault, so
  a failed set never leaves the head displaced. A `StrictTape`-launched
  session (PM-1's `"strictCells": true`) surfaces a strict-cell violation
  as a *failed* `setVariable` carrying the fault text, not a silent
  no-op. An unknown glyph — one the launch alphabet doesn't declare, or
  (with no alphabet known) an out-of-range raw index — is rejected before
  ever touching the tape, naming every legal glyph. Poking the cell
  currently under the head does **not** re-latch the match flag/register:
  the flag stays exactly as the program itself last latched it, the same
  way a hardware probe touching a tape cell would not retroactively
  change what the processor already read. (`poke`'s contract also
  defines a `PositionUnreachable` fault for a position no walk from the
  current head could ever reach — relevant to a wrap-bounded tape device;
  neither adapter currently launches one, so this fault is not reachable
  through either `pmt dap` or `tmt dap` today, only through the trait
  contract itself.)
- **Flag registers** — PM-1's `MF`, TM-1's `MR` — are writable directly.
  **`IP` is read-only on both**, and **`FR` is read-only on TM-1**:
  overwriting either could desynchronize the return stack or the frames
  discipline, and both stay deferred until a real need for writing them
  shows up.

## Error handling and disconnection

A request that violates the lifecycle (`stackTrace` before `launch`,
anything at all after `disconnect`) answers a uniform DAP error response
naming the violation — never a silent no-op and never a crash. A trap is
handled entirely through the pause/state-inspection path above, never as
an adapter error. `disconnect` always succeeds and ends the session;
there is no `restart` and no `attach` (the debuggee has no external
process to attach to — it exists only inside this adapter's own memory).

## Wiring a client

Both `editors/vscode-pm/` and `editors/vscode-tm/` contribute a
`pmt`/`tmt` debugger type resolving the same binary the language client
already uses, launched as `pmt dap`/`tmt dap` on stdio — no separate
binary-resolution setting. Each extension's README carries a full
walkthrough; the two launch shapes look like this from `launch.json`:

```json
{
  "type": "pmt",
  "request": "launch",
  "target": "main",
  "stopOnEntry": true
}
```

```json
{
  "type": "pmt",
  "request": "launch",
  "program": "${workspaceFolder}/main.pmx",
  "tape": "${workspaceFolder}/main.pmt",
  "stopOnEntry": true
}
```

Swap `"type": "tmt"` and drop to TM-1's schema (`"tape"` mandatory in
program mode, no `"strictCells"`) for the Turing-machine extension. Any
client speaking DAP over stdio can launch either binary directly with no
special support beyond the `dap` subcommand itself — the VS Code
contribution is a packaging convenience, not the only way in.
