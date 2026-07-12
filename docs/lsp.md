# The `pmt lsp` language server

`pmt lsp` runs one Language Server Protocol server, on stdio, for both
`.pmc` and `.pma` — built on the exact same lexer, parser,
compiler/assembler, optimizer-free analysis, and linter the CLI uses
for each. Nothing the server reports is a re-implementation: a
diagnostic, a quickfix, or a formatted document is the same answer
`pmt compile`/`pmt lint`/`pmt fmt` would give for the same source. See
`docs/cli.md` (`pmt lsp`) for the subcommand's flags, stdio contract,
and lifecycle exit codes; see **Languages** below for how the two
languages share the one process.

## What `.pmc` serves

`.pmc` has no project model — `use` binds a name that resolves at link
time, never at compile time — so each open document is a complete,
independently analyzable unit. No workspace indexing, no cross-file
invalidation. One well-known external library, the embedded standard
library, is available everywhere without configuration.

On every open or edit the server re-runs the real front half of the
compiler and republishes the document's complete diagnostic set:
a fatal compile error when one stage fails (one at a time — the
compiler is fail-fast, never a cascade of guesses), compile warnings
(undeclared externals, unused imports, unused functions), and lint
findings (`docs/lint.md`), merged and sorted by position. Beyond
diagnostics, the server offers:

- **Completions** in four contexts — after `@` (callable names visible
  from the cursor's scope), after `use ` or a `::` prefix (namespace
  members and the standard library), at statement/label/comma-group
  position (the reserved command words and, after `goto `, the
  enclosing function's labels).
- **Hover** on a call site, a `use` path segment, or a function's own
  declaration name, rendering that function's `?`/`!` documentation
  (`docs/language.md (doc lines and attention lines)`) — see **Hover**
  below for the exact content shape.
- **Go-to-definition** for local and nested functions, import
  bindings, qualified internal and external calls, label references,
  and standard-library routines (via the materialized copy below).
  Clients that declare link support get a response scoped to the exact
  reference span under the cursor, so the editor underlines only that
  reference instead of guessing a word boundary; clients that don't
  declare it get a plain location.
- **Quickfix code actions** built from lint's machine-applicable and
  gated fixes, with the same preferred/not-preferred distinction as
  `pmt lint --fix` vs `--fix --force`.
- **Semantic tokens**, a small resolution-aware legend layered on top
  of static highlighting (below).
- **Document symbols** — the outline: namespace blocks (reopened
  blocks stay separate siblings, as in source) containing functions,
  nested functions as children.
- **Whole-document formatting**, identical to `pmt fmt`.

`.pma`, the assembler dialect, is served by the same process, through
its own service — see **Languages** below for its feature table and
for how the server picks which service answers a given document.

## Capabilities

Every feature answers from the document's current text or degrades
predictably — never a stale position, never a resolution-free guess. A
staged analysis gates what a feature can answer, in three tiers:

| Feature | Needs | Degrades to (when the tier fails) |
|---|---|---|
| Diagnostics: fatal error | the source lexes | one error at the failing stage, honest and singular |
| Diagnostics: compile warnings + lint findings | a full successful analysis | omitted — the fatal is the only entry |
| Completions | tokens/CST for cursor context | candidate *names* may fall back to the last successful analysis, so completion stays useful mid-edit — the one sanctioned staleness exception |
| Hover | a full successful analysis (the resolution table) | `null` |
| Go-to-definition | a full successful analysis (the resolution table) | `null` |
| Code actions (quickfixes) | a full successful analysis (lint ran) | empty list |
| Semantic tokens | a full successful analysis (resolution-aware) | `null` — clients keep the previous tokens or static grammar coloring |
| Document symbols | a successful parse (CST only) | `null` |
| Formatting | a successful parse (CST only) | `null` — the parse error is already on screen as a diagnostic |

### Runtime model

`pmt lsp` is a blocking server loop: it reads Content-Length-framed
JSON-RPC messages off stdin, dispatches each against the bound
language service(s), and enforces the LSP lifecycle —
initialize/initialized/shutdown/exit gating, unknown-method handling,
and decode-error responses. Document sync (`didOpen`/`didChange`/
`didClose`) drives the per-document store and republishes diagnostics
through one shared publish path — the same path the config- and
watched-file-triggered republish-all sweeps use. Feature requests
(completion, definition, code actions, document symbols, semantic
tokens, formatting) convert the bound service's output to wire types
at the position-encoding boundary (**Position encoding**, below).

### Error containment

A handler panic is caught per request: the routed call runs under
`catch_unwind`, so one bad handler can't take the session down. A
panicking request answers `INTERNAL_ERROR` carrying the panic's own
text (no response can have been written yet — every handler either
panics or writes exactly one response, never both); a panicking
notification produces no output beyond a concise stderr line. Either
way the loop always continues, and the next message is served
normally.

## Hover

`textDocument/hover` answers on `.pmc` for a call site, a `use` path
segment that resolves to a documented function, or a function's own
declaration name — whichever the cursor sits on, resolved through the
same walks go-to-definition uses. Content is always plain text
(`MarkupContent.kind: "plaintext"`) — v1 renders no markdown, matching
the `?`/`!` grammar's own plain-prose rule
(`docs/language.md (doc lines and attention lines)`).

A hover body is up to three groups, blank-line separated, in this fixed
order:

1. **Paragraphs** — every `?`-line paragraph, in source order, each
   already blank-line separated from the next.
2. **Deprecation callout** — present only when the function carries a
   `[deprecated]` attention line: `deprecated` alone, or `deprecated:
   MESSAGE` when the attribute carried one.
3. **Attention notes** — every bare-prose `!` line (the `[deprecated]`
   line itself excluded — it already surfaced as the callout above),
   each rendered as its own `note: TEXT` line.

**Content-emptiness rule:** a function with none of the three groups —
undocumented, or documented with only blank `?` lines (they still parse
to a doc record, but every field reduces to empty) — answers `null`
rather than an empty popup. Hover never surfaces on the mere presence of
a doc record; it surfaces on there being something to show.

A `std::` call resolves through the embedded standard library's own
analysis, run once per process the same way every requesting document's
own analysis runs, since a requesting document's analysis only ever
holds ITS OWN functions — a plain in-memory lookup, unlike
go-to-definition's on-disk materialization (below), because hover has
text to render rather than a location to open.

`.pma` never answers hover: `hoverProvider` is still advertised as one
merged capability (Capability merge, below covers this in general), but
the dialect has no doc/attention-line grammar of its own, so every
`.pma` hover request returns `null` — permanently, not as a version-1
placeholder.

## Tags

Two LSP tag surfaces mark a reference to a deprecated function. Both are
additive fields, omitted from the wire entirely rather than sent as an
explicit negative:

- **`deprecated-call` diagnostics** (`docs/lint.md`) carry
  `DiagnosticTag.Deprecated` (wire `"tags":[2]`), so a client renders
  the finding's range struck through. Every other diagnostic code, on
  either service, is untagged.
- **Completion candidates** resolving to a deprecated function carry
  `CompletionItemTag.Deprecated` (wire `"tags":[1]`), so a client
  renders the item struck through in the completion list.

`.pma` has no `[deprecated]`-equivalent attribute grammar, so every
`.pma` diagnostic and candidate stays untagged permanently — the same
permanence `.pma`'s hover has, above.

## Completion detail

Either service may attach a short `detail` string to a completion
candidate (wire `detail`), omitted entirely when there is nothing worth
adding: the shared rule is "nothing invented" — `detail` only ever
carries information the service already has cheaply in hand, never a
guess or a derived label.

- **`.pmc`** sets `detail` to the candidate's fully-qualified name — a
  scope mapping's own value, a standard-library roster entry's own
  path, or a nested function's dot-mangled name reapplied from the same
  formula flatten uses — whenever it differs from the bare `label`
  already shown in the list (a cross-namespace or nested candidate); an
  unnamespaced top-level candidate has nothing to add and carries no
  `detail` at all.
- **`.pma`** sets `detail` to an operand hint on every mnemonic
  candidate that takes an operand, derived from the mnemonic's own
  operand kind and control-flow role rather than a per-mnemonic table:
  a symbol-vector operand (`wr`) hints its index-list shape (`wr
  <indices>`); a relative-address operand hints `<function>` on a
  call-flow mnemonic (`call`, `call.s`) and `<label>` on a jump- or
  branch-flow one (`jmp`, `jm`, `jnm`, and their short `.s` forms). A
  no-operand mnemonic (`nop`, `stp`, `ret`, and the rest) carries no
  `detail`. The `.byte`/`.func` directives carry their own fixed hints
  (`.byte <0..=255>`, `.func <name> [local]`) — they have no mnemonic
  operand table entry of their own to derive one from.

## Languages

One process, one stdio connection, two independent language services:
`.pmc` (above) and `.pma`, the assembler dialect. Each service owns
its own per-document state and answers only for the documents bound to
it — opening a `.pmc` file never perturbs a `.pma` session, or vice
versa, and editing or closing one never republishes the other's
diagnostics.

### Routing

A freshly opened document binds to exactly one service, once, on
`textDocument/didOpen`; every later message for that URI —
`didChange`, `didClose`, any feature request — is served by whichever
service it bound to. This multi-service routing tries, in order: the
client's own `languageId` (an exact match against `pmc` or `pma`),
then the URI's file extension (`.pmc` or `.pma`) when the languageId
matches neither service, and finally the `.pmc` service as a
last-resort default — a document neither identifier recognizes still
gets *some* answer instead of silence. A client that always reports an
accurate `languageId` never falls through past the first check.

### Capability merge

`initialize` answers with one merged capability set, not two: every
feature either service supports is advertised once, through the
capability merge this section describes. The semantic-tokens legend is
the concrete shape of it: `.pmc` registers its own token types
(`namespace`, `function`, `number`) and modifiers, `.pma` registers its
own (`function`, `variable`, `number`) and modifiers, and the merged
legend concatenates the two type lists in registration order — `.pmc`'s
block first, `.pma`'s second, six entries total, no deduplication —
while the modifier lists dedup-union by name (`declaration` and
`defaultLibrary` collapse to one bit apiece, since both services
happen to name the same two). Every token a service emits is relocated
from its own local legend index into this shared index space before it
reaches the wire, so a client sees one consistent legend for the whole
session regardless of which document it's asking about. Trigger
characters and watched globs merge the same way, as an ordered,
deduplicated union.

### What `.pma` serves

`.pma` has the same "no project model" shape as `.pmc` — every open
document is a complete, independently analyzable unit. Its own
analysis has two tiers rather than `.pmc`'s three: a total CST (every
line parses into *something*, so completions, go-to-definition,
document symbols, and semantic tokens all answer even over a document
that fails to assemble) and a fatal-or-lint split built on the same
assembler and linter `pmt asm`/`pmt lint` use.

| Feature | Needs | Degrades to (when the tier fails) |
|---|---|---|
| Diagnostics: fatal error | the total CST | one error — a semantic assemble failure, or the structural `raw-line` code for a line that isn't assembly-shaped at all — honest and singular |
| Diagnostics: lint findings | a clean assemble (no fatal) | omitted — the fatal is the only entry; unlike `.pmc`, there is no separate compile-warning channel |
| Completions | the total CST at the cursor's line | empty list on no context match |
| Go-to-definition | the total CST (the operand token under the cursor) | `null` |
| Quickfix code actions | a clean assemble (lint ran) | empty list |
| Semantic tokens | the total CST | answers for any known document — no resolution tier to gate on |
| Document symbols | the total CST | answers for any known document — functions and their labels resolve structurally |
| Whole-document formatting | no `raw-line` in the source | `null` — the only structural gate; any other semantic error (an unknown mnemonic, say) still formats |

### Shared configuration

Both services read the same `pmt.json` (see **Configuration** below)
and the same IDE-settings channel — there is no per-language config
file or override. A `lint.allow` entry applies uniformly no matter
which language's rule table it names: the allow-list is validated
against the union of `.pmc`'s and `.pma`'s rule codes, so a
`.pma`-only code (or a `.pmc`-only one) never errors as unknown just
because the document currently open happens to be the other language.

## Wiring a generic LSP client

Any client that speaks LSP 3.17 over stdio can launch `pmt lsp`
directly — no special client extension is required. Two examples:

### Neovim (`vim.lsp.config` / `vim.lsp.enable`, 0.11+)

```lua
vim.lsp.config.pmt = {
  cmd = { "pmt", "lsp" },
  filetypes = { "pmc", "pma" },
  root_markers = { "pmt.json", ".git" },
}
vim.lsp.enable("pmt")

-- Recognize both extensions (no bundled filetype plugin ships yet):
vim.filetype.add({ extension = { pmc = "pmc", pma = "pma" } })
```

### Helix (`languages.toml`)

```toml
[[language]]
name = "pmc"
scope = "source.pmc"
file-types = ["pmc"]
roots = ["pmt.json"]
language-servers = ["pmt-lsp"]

[[language]]
name = "pma"
scope = "source.pma"
file-types = ["pma"]
roots = ["pmt.json"]
language-servers = ["pmt-lsp"]

[language-server.pmt-lsp]
command = "pmt"
args = ["lsp"]
```

### Editor shells

This repository ships two ready-made editor integrations under
`editors/`: a VS Code extension (`editors/vscode/`) and a JetBrains
plugin (`editors/jetbrains/`), both thin shells over `pmt lsp` plus
the standalone build/lint/format commands. Both are sideload-only —
built locally from source, with no marketplace listing — and each
directory's `README.md` carries its own install, build, and sideload
instructions plus a manual test checklist. The wiring above talks to
the same binary those shells launch, so nothing here is
shell-specific: a generic client and a shipped shell differ only in
launch mechanics, never in the server's behavior.

## Position encoding

The server always negotiates `utf-16` — the one encoding every LSP
client supports. Internally, every position is 1-based and counts
Unicode scalar values (characters), the same currency the compiler's
own diagnostics use; the char-to-UTF-16 conversion happens once, at
the wire boundary, against the document's current text. A future
`positionEncoding` negotiation down to `utf-32` would make that
conversion the identity — framework-only work, no service change.

## The materialized standard library

Go-to-definition on a `std::` call has nowhere to point without a real
file on disk: the standard library ships embedded in the `pmt` binary,
not as a directory tree. On first demand the server writes the
embedded source to a per-version cache path —
`$XDG_CACHE_HOME/pmt/<version>/std.pmc`, falling back to `~/.cache` on
Unix or `%LOCALAPPDATA%` on Windows — and points definitions at spans
inside that file. The write self-heals: a missing or edited copy is
checked and rewritten on first demand (once per server run) — the
check itself is memoized for the process's lifetime, so a copy edited
or deleted mid-session is not re-detected until the next launch. Any
IO failure along the way (an unwritable cache directory, for instance)
degrades go-to-definition on `std::` targets to `null` rather than
pointing at a file that doesn't exist; nothing else in the session is
affected.

## Configuration

A project has exactly one config file, `pmt.json`, read by both the
CLI and the server for either language — see `docs/lint.md` for its
schema, discovery rule (nearest ancestor wins, never a cascade), and
union semantics.
The server adds one more source on top: IDE settings, forwarded over
the standard LSP configuration channel (`initializationOptions` at
startup, live afterward) as `{ "lint": { "allow": [...] } }`, or the
same object wrapped under a `"pmt"` key for clients that forward whole
settings sections. Wherever more than one source applies to a
document — the discovered `pmt.json` and the IDE setting — the
effective allow-list is their union, exactly as `docs/lint.md`
describes for the CLI's file-plus-flags case.

The server watches `**/pmt.json` through the client's file-watch
capability and re-publishes every open document's diagnostics after a
change, after a live settings update, and — via an `(path, mtime)`
cache — after the winning file itself is edited on disk. An invalid
`pmt.json` or an invalid IDE setting surfaces as one `invalid-config`
warning diagnostic at the top of the affected document rather than a
CLI-style hard error, since the server has no terminal to fail loudly
on; the remaining valid sources still apply.
