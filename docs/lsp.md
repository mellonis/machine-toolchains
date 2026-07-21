# The language servers

This repository ships **two** Language Server Protocol servers, one per
toolchain, each serving **two** languages from a single process on stdio:

| Command | Serves | Project file |
|---|---|---|
| `pmt lsp` | `.pmc` (the Post-machine source language) and `.pma` (PM-1 assembly) | `pmt.json` |
| `tmt lsp` | `.tmc` (the Turing-machine source language) and `.tma` (TM-1 assembly) | `tmt.json` |

Each is built on the exact same lexer, parser, compiler/assembler,
optimizer-free analysis, linter and formatter its CLI uses. Nothing a
server reports is a re-implementation: a diagnostic, a quickfix, or a
formatted document is the same answer `pmt compile`/`pmt lint`/`pmt fmt`
— or `tmt compile`/`tmt lint`/`tmt fmt` — would give for the same source.
See `docs/pmt/cli.md` (`pmt lsp`) and `docs/tmt/cli.md` (`tmt lsp`) for
each subcommand's flags, stdio contract, and lifecycle exit codes.

The four services are independent implementations of one framework trait,
and they are **not** feature-identical. The framework is described once
below; then each feature, with the per-service differences named where
they exist; then a profile of each service. Where a service does not
offer something, this page says so rather than leaving the gap to be
inferred.

## The framework

The server loop, transport, routing, capability merge, document store and
position mapping live in `mtc-core` and know nothing about any language
(`docs/core.md`). A language plugs in as a service: it declares its
language id, its file extensions, its completion trigger characters, its
semantic-token legend and its watched globs, and it answers document-sync
and feature requests. Everything below this heading is therefore identical
for `pmt lsp` and `tmt lsp`.

### Runtime model

A server is a blocking loop: it reads Content-Length-framed JSON-RPC
messages off stdin, dispatches each against the bound language service(s),
and enforces the LSP lifecycle — initialize/initialized/shutdown/exit
gating, unknown-method handling, and decode-error responses. Document sync
(`didOpen`/`didChange`/`didClose`) drives the per-document store and
republishes diagnostics through one shared publish path — the same path
the config- and watched-file-triggered republish-all sweeps use. Feature
requests (completion, definition, hover, code actions, document symbols,
semantic tokens, formatting) convert the bound service's output to wire
types at the position-encoding boundary (**Position encoding**, below).

### Routing

A freshly opened document binds to exactly one service, once, on
`textDocument/didOpen`; every later message for that URI — `didChange`,
`didClose`, any feature request — is served by whichever service it bound
to. Each service owns its own per-document state and answers only for the
documents bound to it: opening a `.pmc` file never perturbs a `.pma`
session, or a `.tmc` file a `.tma` one, and editing or closing one never
republishes the other's diagnostics.

The routing tries, in order: the client's own `languageId` (an exact match
against a service's declared id — `pmc`, `pma`, `tmc`, `tma`), then the
URI's file extension when the languageId matches no service — matched
case-insensitively, so `Foo.TMA` binds the same as `foo.tma` — and finally
the **first registered service** as a last-resort default — a document
neither identifier recognizes still gets *some* answer instead of silence.
`pmt lsp` registers `.pmc` first and `tmt lsp` registers `.tmc` first, so
the fallback is the source language in both. A client that always reports
an accurate `languageId` never falls through past the first check.

### Capability merge

`initialize` answers with one merged capability set, not two: every
feature either bound service supports is advertised once. The
semantic-tokens legend is the concrete shape of it — the merged legend
**concatenates** the services' type lists in registration order, with no
deduplication, while the modifier lists **dedup-union by name**:

| Server | Merged token types | Merged modifiers |
|---|---|---|
| `pmt lsp` | `.pmc`'s `namespace`, `function`, `number`, then `.pma`'s `function`, `variable`, `number` — six entries | `declaration`, `defaultLibrary` (both services name the same two) |
| `tmt lsp` | `.tmc`'s `namespace`, `type`, `function`, `variable`, `string`, `number`, then `.tma`'s `function`, `variable`, `type`, `number` — ten entries | `declaration` (the only modifier either service names) |

Every token a service emits is relocated from its own local legend index
into this shared index space before it reaches the wire, so a client sees
one consistent legend for the whole session regardless of which document
it is asking about. Trigger characters and watched globs merge the same
way, as an ordered, deduplicated union: `pmt lsp` triggers on `@`, `:`,
`.` and watches `**/pmt.json`; `tmt lsp` triggers on `:`, `[`, `,`, `=`,
`>`, `@`, `.` and watches `**/tmt.json`.

Because the merge is a union, a capability may be advertised that the
service answering a given document does not implement — `hoverProvider`
is advertised by both servers, but neither assembly service ever answers
a hover (**Hover**, below).

### Error containment

A handler panic is caught per request: the routed call runs under
`catch_unwind`, so one bad handler cannot take the session down. A
panicking request answers `INTERNAL_ERROR` carrying the panic's own text
(no response can have been written yet — every handler either panics or
writes exactly one response, never both); a panicking notification
produces no output beyond a concise stderr line. Either way the loop
always continues, and the next message is served normally.

### Position encoding

A server always negotiates `utf-16` — the one encoding every LSP client
supports. Internally, every position is 1-based and counts Unicode scalar
values (characters), the same currency the compilers' own diagnostics use;
the char-to-UTF-16 conversion happens once, at the wire boundary, against
the document's current text.

### Staged analysis

No feature ever answers from a stale position. Instead each service runs a
**staged analysis** over the document's current text, keeps every stage's
partial result, and gates each feature on the lowest stage that can answer
it honestly: a feature whose stage failed degrades to a defined answer
(`null`, an empty list, an omitted diagnostic channel) rather than
guessing from an older parse.

The one sanctioned exception is **names**. The source-language services
keep a last-good roster of names and symbols so that completion candidates
survive a failed re-analysis — a completion list that empties out the
moment a bracket is unbalanced is useless, and mid-edit is exactly when
completion matters. Only names and glyphs can ever be one edit old;
nothing positional is ever retained.

The stages differ per language, so each service's tier table lives in its
own profile below. The two assembly services stage differently from the
two source services in one structural way: their assembly CST is
**total** — every line parses into *something* — so their structural
features answer over a document that fails to assemble, with no
resolution tier to gate on.

## Configuration

A project has exactly one config file — `pmt.json` for the PM-1 toolchain,
`tmt.json` for the TM-1 one — read by both the CLI and the server, for
either of that toolchain's languages. There is no per-language config file
and no override: the discovery rule is nearest ancestor wins, never a
cascade. The schemas are documented at `docs/pmt/lint.md` and
`docs/tmt/cli.md` respectively.

Both services of a server read the same file and the same IDE-settings
channel. A `lint.allow` entry applies uniformly no matter which language's
rule table it names: the allow-list is validated against the union of the
server's two rule catalogs, so a code belonging to only one of them never
errors as unknown just because the document currently open happens to be
the other language. The two toolchains keep **separate** namespaces from
each other, though — `pmt.json` never configures `tmt lint`, or the
reverse.

The server adds one source on top of the project file: IDE settings,
forwarded over the standard LSP configuration channel
(`initializationOptions` at startup, live afterward) as
`{ "lint": { "allow": [...] } }`, or the same object wrapped under a
`"pmt"` / `"tmt"` key for clients that forward whole settings sections.
Wherever more than one source applies to a document — the discovered
project file and the IDE setting — the effective allow-list is their
**union**, exactly as the lint pages describe for the CLI's
file-plus-flags case. Every other key in the settings object is
client-owned (binary path, trace switches) and deliberately ignored.

One asymmetry: `tmt lint` has an opt-in rule tier (`--warn`), but
`tmt.json`'s schema is `lint.allow` and nothing else, so IDE settings are
the only channel that can turn a default-off `.tmc` rule on for the editor
— through `{ "lint": { "warn": [...] } }`, read by the `.tmc` service
only. The `.tma` service has no opt-in tier and reads no `warn` key.

A server watches its project file glob through the client's file-watch
capability and re-publishes every open document's diagnostics after a
change, after a live settings update, and — via a `(path, mtime)` cache —
after the winning file itself is edited on disk. Discovery itself re-runs
on every analysis, so a newly created nearer project file wins
immediately; only the parse of the winner is cached. An invalid project
file or an invalid IDE setting surfaces as one `invalid-config` warning
diagnostic at the top of the affected document rather than a CLI-style
hard error, since the server has no terminal to fail loudly on; the
remaining valid sources still apply.

## Diagnostics

On every open or edit a service re-runs the real front half of its
pipeline and republishes the document's complete diagnostic set. The merge
order is the same in all four: any `invalid-config` warnings first, then
either the single fatal or the span-ordered merge of the non-fatal
channels. A fatal is always **exactly one** error — the pipelines are
fail-fast, and a cascade of guesses after the first failure would be
noise, not information.

| Service | Non-fatal channels |
|---|---|
| `.pmc` | compile warnings (undeclared externals, unused imports, unused functions) and lint findings (`docs/pmt/lint.md`) |
| `.pma` | lint findings only (`docs/pmt/lint.md`) — there is no separate compile-warning channel |
| `.tmc` | compile warnings (unused imports) and lint findings (`docs/tmt/lint.md`) |
| `.tma` | lint findings (`docs/tmt/lint.md`) plus the frame-descriptor channel described in its profile below |

Lint findings carry the source `"pmt lint"` / `"tmt lint"`; compile
warnings and fatals carry the bare tool name. A finding's code is sent as
the diagnostic code so a client can display and filter on it.

The `.tmc` service runs one stage beyond what `tmt compile`'s analysis
needs for names: after a clean resolve it also runs range and graft
expansion, purely for that stage's fatal. Expansion is where the
binding-map legality rules live, so without it a whole class of errors
`tmt compile` reports would stay invisible in the editor — and the map
quickfix would have no trigger.

## Completions

Every candidate stamps the cursor's own replace span, so the client
replaces exactly the token being typed. A server never filters by the
typed prefix; that is the client's job over the replace span.

**`.pmc`** completes in four contexts: after `@` (callable names visible
from the cursor's scope), after `use ` or a `::` prefix (namespace members
and the standard library), and at statement/label/comma-group position
(the reserved command words and, after `goto `, the enclosing function's
labels).

**`.tmc`** classifies the cursor over the current *token stream* rather
than the CST — a document being typed into is a document that does not
parse, and anchoring on the CST would switch completions off exactly when
they are wanted. Its contexts:

- **Top-level and world-item position** — the reserved words legal there
  (`alphabet`, `export`, `graph`, `machine`, `namespace`, `routine`,
  `use`; inside a world `bind`, `entry`, `graft`, `state`, and `tape` in
  the machine block).
- **`use` paths** — the importable alphabet and world names.
- **An alphabet reference** — the declared alphabet names.
- **A vector cell** — the enclosing world's tape at *that* vector
  position supplies its alphabet's symbols, spelled the way source spells
  them, alongside the vector's own literal vocabulary: `*` in a pattern,
  `-` in a write vector, and the closed, tape-independent `<` / `>` / `.`
  in a move vector.
- **An action, a `goto` target, or a continuation** — the enclosing
  world's states, state parameters and graft instances, plus the action
  keywords and the `halt` / `return` / `stop` terminators.
- **A `call`/`graft`/`bind` target** — the world names reachable from the
  cursor's scope.
- **A binding argument** — the parameter names of the *callee*'s
  signature on the left of the `=`, and on the right the vocabulary that
  parameter's half of the signature takes: a tape parameter offers the
  enclosing world's tapes, a state parameter its continuations. When the
  parameter cannot be classified (an unresolvable callee, a name not in
  the signature, a roster one edit stale) the union of both is offered
  instead — degrading to more candidates is a nuisance, degrading to none
  is a dead list.
- **A `with map` pair** — the host tape's alphabet on the source side and
  the callee tape's alphabet on the destination side, each resolved
  through the signature rather than guessed.

**`.pma`** and **`.tma`** classify from their total CST at the cursor's
line. Both offer every mnemonic and directive at instruction-word
position, and resolve operand position by the mnemonic's own operand kind
and control-flow role: a label operand offers the *enclosing function's*
labels (labels are function-scoped), a callable operand the document's
callable names, and `@` completes a symbol reference. `.tma` adds the
operand roles its dialect has — a table operand offers the labeled
tables, a frame operand the `.frame` descriptors — and completes code
labels inside a `.targets`/`.target` entry and an `.exits` target, which
is what makes authoring a dispatch table bearable. Those are doc-wide,
because a table lives outside any function and has no enclosing scope to
resolve against.

### Candidate detail

Any service may attach a short `detail` string to a candidate, omitted
entirely when there is nothing worth adding: the shared rule is "nothing
invented" — `detail` only ever carries information the service already has
cheaply in hand, never a guess or a derived label.

- **`.pmc`** sets it to the candidate's fully-qualified name — a scope
  mapping's own value, a standard-library roster entry's own path, or a
  nested function's dot-mangled name reapplied from the same formula
  flatten uses — whenever it differs from the bare label already shown in
  the list. An unnamespaced top-level candidate has nothing to add and
  carries no detail at all.
- **`.tmc`** sets it to the candidate's kind and, where one applies, its
  alphabet: `alphabet <name>` on a glyph, `tape: <alphabet>` on a tape,
  `tape param: <alphabet>` on a signature parameter, `state` on a
  transition target, and a short gloss on a vector literal.
- **`.pma`** and **`.tma`** set it to an operand hint on every mnemonic
  candidate that takes an operand, derived from the mnemonic's own operand
  kind and control-flow role rather than a per-mnemonic table — so a
  mnemonic added to the arch's syntax table gets a hint with no new case
  to write. A no-operand mnemonic carries no detail. Directives carry
  their own fixed hints, having no mnemonic operand-table entry of their
  own to derive one from.

## Navigation

Go-to-definition and hover are two questions about one thing: a service
resolves what the cursor sits on — the **reference** side — and then asks
either where that is declared or what it says. Both funnel through the
same walk, so the two can never disagree about what the cursor meant.

### Go-to-definition

**`.pmc`** resolves local and nested functions, import bindings, qualified
internal and external calls, label references, and standard-library
routines (through the materialized copy described below). Clients that
declare link support get a response scoped to the exact reference span
under the cursor, so the editor underlines only that reference instead of
guessing a word boundary; clients that do not declare it get a plain
location. Every service answers this way.

**`.tmc`** resolves alphabet references, world (routine/graph) names, a
world's states, its bind and graft instances, its tapes, and a signature
parameter named on a binding argument's left-hand side. A graft instance
navigates to the *graph it splices*, which is where the states it
contributes are actually written. The reference side is answered against
the flat program, so navigation keeps working on a document whose
semantics do not yet check out; the target side consults the resolved
module where one exists, which is what lets a bind target resolve to the
world it actually names.

**`.pma`** and **`.tma`** resolve the operand token under the cursor from
their total CST, so a reference resolves even while something else in the
document refuses to assemble. `.tma` has four reference shapes: a **label**
reference resolves within the enclosing function only; a **callable**
reference resolves doc-wide, preferring the function that defines the body
and falling back to the `.routine` signature — a routine defined in
another translation unit is declared here, and jumping to its signature is
the best answer this file can give; a **table** reference resolves to the
label on the directive that opens the table; a **frame** reference (a
framed call's second operand) resolves to the `.frame` header's label. Two
further arrows run the other way, out of a `.targets`/`.target` entry or
an `.exits` target into the code label it names — without them the tables
section is a dead end. Those resolve doc-wide and the first matching label
wins, which is an approximation the linker's own binding does not make: it
can pick the wrong same-named label, never invent one. An operand carrying
an unexpanded `.rept` template marker names no identifier and resolves to
nothing.

### Hover

Hover is answered by the two **source-language** services only. Content is
always plain text — neither renders markdown, matching the `?`/`!` doc-line
grammars' own plain-prose rule.

**`.pmc`** answers on a call site, a `use` path segment that resolves to a
documented function, or a function's own declaration name. Its body is up
to three groups, blank-line separated, in this fixed order:

1. **Paragraphs** — every `?`-line paragraph, in source order, each
   already blank-line separated from the next.
2. **Deprecation callout** — present only when the function carries a
   `[deprecated]` attention line: `deprecated` alone, or
   `deprecated: MESSAGE` when the attribute carried one.
3. **Attention notes** — every bare-prose `!` line (the `[deprecated]`
   line itself excluded, having already surfaced as the callout above),
   each rendered as its own `note: TEXT` line.

**Content-emptiness rule:** a function with none of the three groups —
undocumented, or documented with only blank `?` lines (they still parse to
a doc record, but every field reduces to empty) — answers `null` rather
than an empty popup. Hover never surfaces on the mere presence of a doc
record; it surfaces on there being something to show.

A `std::` call resolves through the embedded standard library's own
analysis, run once per process the same way every requesting document's
own analysis runs, since a requesting document's analysis only ever holds
its own functions — a plain in-memory lookup, unlike go-to-definition's
on-disk materialization, because hover has text to render rather than a
location to open.

**`.tmc`** answers on the same references its go-to-definition resolves,
and leads with a **signature line** — an alphabet with its glyphs, a world
with its parameters and their alphabets, a bind instance with the mangled
routine it targets and each argument's bound value — then the
declaration's own doc paragraphs and deprecation callout under it. The
signature line is the part the source text alone does not give a reader:
the bound values come from the resolved module.

**`.pma` and `.tma` never answer hover.** `hoverProvider` is still
advertised as one merged capability, but assembly text has no
doc/attention-line grammar for a hover to render, so every request returns
`null`. This is permanent, not a first-version placeholder.

## Code actions

Quickfixes reach the editor two ways, and which ways are live differs by
service.

**From lint findings.** A finding carrying a fix converts mechanically to
a code action, with the same preferred/not-preferred distinction as
`pmt lint --fix` vs `--fix --force`: a machine-applicable fix is marked
preferred, a maybe-incorrect one is not. Only findings whose span overlaps
the requested range contribute.

**From a fatal.** The `.tmc` service builds two actions from the
compiler's fatal, which carries no fix of its own because the batch
pipeline has nowhere to apply one. Both reconstruct the missing source
from what the analysis already knows, so neither invents a shape the
language would reject:

- an **undefined state** offers a stub of the right tape arity, inserted
  on its own line before the enclosing world's closing brace, indented one
  level in from that brace — read off the brace rather than assumed, so a
  world nested in a namespace gets the depth `tmt fmt --check` expects;
- an **identity-glyph mismatch** on a binding offers the `with map` pairs
  it needs, derived from the two alphabets the analysis already resolved.

What this means per service today: `.pmc` and `.pma` offer lint-derived
quickfixes. `.tma` offers them too, from the arch-agnostic assembly rules
it shares with `.pma` (`docs/core.md`) — `redundant-jump-to-next` and
`leftover-debugger` are the two that carry fixes on that path. No `.tmc`
rule and no TM-1 `.tma` rule addition emits a fix of its own, so `.tmc`
code actions are the two fatal-derived quickfixes above and nothing else.

## Semantic tokens

Semantic tokens are a small resolution-aware legend layered on top of the
editor's static highlighting, never a replacement for it. Reserved words
and mnemonics are deliberately not emitted — keyword colouring is the
grammar's job, and emitting it here would fight it. An unresolved
reference emits nothing for the name: a quiet cue, rather than a colour
that would be a lie.

| Service | Legend types | Gated on |
|---|---|---|
| `.pmc` | `namespace`, `function`, `number` | a full successful analysis (resolution-aware) |
| `.pma` | `function`, `variable`, `number` — labels ride `variable`, with `declaration` on definitions | the total CST |
| `.tmc` | `namespace`, `type`, `function`, `variable`, `string`, `number` | the token stream alone — each identifier takes its type from the keyword or punctuation around it, so highlighting does not switch off the moment a brace is unbalanced |
| `.tma` | `function`, `variable`, `type`, `number` | the total CST |

`.tma` adds one distinction PM-1 assembly has no need for: a table or
frame label rides `type` rather than `variable`, because a dispatch table
and a frame descriptor are data structures, not jump targets, and a reader
scanning the tables section benefits from seeing them apart from the code
labels they point at.

## Document symbols

The outline, structural in every service and never gated on resolution.

- **`.pmc`** — namespace blocks (reopened blocks stay separate siblings,
  as in source) containing functions, with nested functions as children.
- **`.tmc`** — alphabets, namespaces (their items as children), routines
  and graphs (their world items as children), and the machine block.
- **`.pma`** — functions with their own labels as children.
- **`.tma`** — one node per function with its code labels as children, one
  per `.routine` signature, and one per labeled table or frame descriptor.
  The protocol's symbol kinds have no label variant, so code labels reuse
  the function kind, while tables and frames — data, not code — ride the
  namespace kind to read distinctly in an outline.

## Formatting

Whole-document formatting is identical to the toolchain's `fmt`
subcommand: the same formatter, over the document store's text. The
framework diffs the returned text against exactly what the last
`didChange` delivered, never a re-read from disk.

The gate is a successful parse. `.pmc`, `.tmc` and `.tma` answer `null`
when the document does not parse — the parse error is already on screen as
a diagnostic. `.pma` gates only on the structural `raw-line` code (a line
that is not assembly-shaped at all); any other semantic error, an unknown
mnemonic for instance, still formats.

## Tags

Two LSP tag surfaces mark a reference to a deprecated declaration. Both
are additive fields, omitted from the wire entirely rather than sent as an
explicit negative:

- **`deprecated-call` diagnostics** carry `DiagnosticTag.Deprecated` (wire
  `"tags":[2]`), so a client renders the finding's range struck through.
  Every other diagnostic code, on every service, is untagged.
- **Completion candidates** resolving to a deprecated declaration carry
  `CompletionItemTag.Deprecated` (wire `"tags":[1]`), so a client renders
  the item struck through in the completion list.

Both source languages have the `[deprecated]` attention-line attribute and
both tag accordingly; in `.tmc` only declarations — routines, graphs,
alphabets — can carry it. Neither assembly dialect has an equivalent
attribute grammar, so every `.pma` and `.tma` diagnostic and candidate
stays untagged — permanently, the same way their hover does.

## The `.pmc` service

`.pmc` has no project model — `use` binds a name that resolves at link
time, never at compile time — so each open document is a complete,
independently analyzable unit. No workspace indexing, no cross-file
invalidation. One well-known external library, the embedded standard
library, is available everywhere without configuration.

| Feature | Needs | Degrades to (when the tier fails) |
|---|---|---|
| Diagnostics: fatal error | the source lexes | one error at the failing stage, honest and singular |
| Diagnostics: compile warnings + lint findings | a full successful analysis | omitted — the fatal is the only entry |
| Completions | tokens/CST for cursor context | candidate *names* may fall back to the last successful analysis, so completion stays useful mid-edit |
| Hover | a full successful analysis (the resolution table) | `null` |
| Go-to-definition | a full successful analysis (the resolution table) | `null` |
| Code actions (quickfixes) | a full successful analysis (lint ran) | empty list |
| Semantic tokens | a full successful analysis (resolution-aware) | `null` — clients keep the previous tokens or static grammar coloring |
| Document symbols | a successful parse (CST only) | `null` |
| Formatting | a successful parse (CST only) | `null` — the parse error is already on screen as a diagnostic |

### Materialized standard library

Go-to-definition on a `std::` call has nowhere to point without a real
file on disk: the standard library ships embedded in the `pmt` binary, not
as a directory tree. On first demand the server writes the embedded source
to a per-version cache path — `$XDG_CACHE_HOME/pmt/<version>/std.pmc`,
falling back to `~/.cache` on Unix or `%LOCALAPPDATA%` on Windows — and
points definitions at spans inside that file. The write self-heals: a
missing or edited copy is checked and rewritten on first demand (once per
server run) — the check itself is memoized for the process's lifetime, so
a copy edited or deleted mid-session is not re-detected until the next
launch. Any IO failure along the way (an unwritable cache directory, for
instance) degrades go-to-definition on `std::` targets to `null` rather
than pointing at a file that does not exist; nothing else in the session
is affected.

The `.tmc` service has no counterpart to this. Its analysis knows only the
open document, so a `std::` reference in `.tmc` completes, navigates and
hovers no differently from any other name the document has not declared —
the TM-1 standard library enters at link time (`docs/tmt/stdlib.md`).

## The `.pma` service

`.pma` has the same "no project model" shape as `.pmc` — every open
document is a complete, independently analyzable unit. Its own analysis
has two tiers rather than `.pmc`'s three: a **total CST** (every line
parses into *something*, so completions, go-to-definition, document
symbols, and semantic tokens all answer over a document that fails to
assemble) and a fatal-or-lint split built on the same assembler and linter
`pmt asm`/`pmt lint` use.

| Feature | Needs | Degrades to (when the tier fails) |
|---|---|---|
| Diagnostics: fatal error | the total CST | one error — a semantic assemble failure, or the structural `raw-line` code for a line that is not assembly-shaped at all — honest and singular |
| Diagnostics: lint findings | a clean assemble (no fatal) | omitted — the fatal is the only entry; unlike `.pmc`, there is no separate compile-warning channel |
| Completions | the total CST at the cursor's line | empty list on no context match |
| Go-to-definition | the total CST (the operand token under the cursor) | `null` |
| Quickfix code actions | a clean assemble (lint ran) | empty list |
| Semantic tokens | the total CST | answers for any known document — no resolution tier to gate on |
| Document symbols | the total CST | answers for any known document — functions and their labels resolve structurally |
| Whole-document formatting | no `raw-line` in the source | `null` — the only structural gate; any other semantic error still formats |

## The `.tmc` service

`.tmc` likewise has no project model: a `use` path is resolved against the
open document's own declarations, and a name that is not declared there is
a link-time concern. The staged analysis runs lex → parse → resolve, each
stage keeping its partial result, plus the expansion stage the service
adds for its fatal (**Diagnostics**, above).

| Feature | Needs | Degrades to (when the tier fails) |
|---|---|---|
| Diagnostics: fatal error | the source lexes | one error at the failing stage, honest and singular |
| Diagnostics: compile warnings + lint findings | a clean resolve | omitted — the fatal is the only entry |
| Completions | the token stream at the cursor | names and glyphs may come from the last-good roster; an empty list when there has never been one |
| Hover | a successful parse; the resolved module enriches a bind's signature line | `null` |
| Go-to-definition | a successful parse | `null` |
| Code actions (quickfixes) | a fatal the service knows how to repair, overlapping the request | empty list |
| Semantic tokens | the source lexes | `null` |
| Document symbols | a successful parse (CST only) | `null` |
| Formatting | a successful parse (CST only) | `null` — the parse error is already on screen as a diagnostic |

Two of those tiers are lower than the `.pmc` equivalents, deliberately.
Navigation answers off the flat program rather than the resolution table,
so it survives a document whose semantics do not yet check out; semantic
tokens answer off the token stream, so highlighting survives an unbalanced
brace.

One limitation is worth stating plainly: the resolve stage stops at its
first offending span rather than accumulating, and raises its own
non-fatal findings only at the very end. A document that fatals partway
through resolution therefore surfaces exactly one diagnostic — the fatal —
and none of the warnings the earlier, unaffected declarations would have
produced. That is a property of the analysis seam, not of the service; the
last-good roster is the part the service can do something about.

## The `.tma` service

`.tma` mirrors `.pma`'s two-tier shape — a total CST plus a fatal-or-lint
split — over the TM-1 assembler, reusing the `.tmc` service's
config-resolution and code-action machinery. One `lint` call settles both
the fatal gate and the findings; it is the same entry `tmt lint` calls, so
the editor and the command line agree on every finding, including the
suppression of the arch-agnostic `unused-label` rule on this path
(`docs/tmt/lint.md`).

| Feature | Needs | Degrades to (when the tier fails) |
|---|---|---|
| Diagnostics: fatal error | the total CST | one error — a lower or assemble failure — honest and singular |
| Diagnostics: lint findings | a clean assemble (no fatal) | omitted |
| Diagnostics: frame-descriptor findings | the total CST | independent of the fatal gate — see below |
| Completions | the total CST at the cursor's line | empty list on no context match |
| Go-to-definition | the total CST (the operand token under the cursor) | `null` |
| Quickfix code actions | a clean assemble (lint ran) | empty list |
| Semantic tokens | the total CST | answers for any known document |
| Document symbols | the total CST | answers for any known document |
| Whole-document formatting | a successful CST parse | `null` |

**The frame-descriptor channel.** `.tma` adds one diagnostic channel
`.pma` has no need for: the `.frame` / `.map` / `.exits` field checks,
re-derived from the parsed CST. These are not new findings — every one
mirrors a rule the assembler itself enforces, and each publishes under the
assembler's own code carrying the assembler's own wording, so a user never
sees the editor object to something `tmt asm` would accept. What the CST
tier buys is that they surface **all at once and independently of the
fatal gate**: lowering stops at its first offending descriptor and never
runs at all when something unrelated earlier in the file refuses to
assemble, so a file with a stray mnemonic in its code section would
otherwise show nothing about the broken descriptors above it. The finding
that duplicates the published fatal is dropped where the channels merge,
so no defect is reported twice. Defects the assembler tolerates are out of
scope here — flagging those is a lint rule's job, on both surfaces at
once, not a service-only opinion.

**Line structure.** The `.pma` service recovers the line of each CST item
by zipping items against the source's non-blank lines, one item per line.
That invariant does not hold for `.tma`: the dialect enables the `.rept`
macro, and a `.rept` … `.endr` block collapses many source lines into one
item whose body items nest inside it. The `.tma` service walks the tree
instead, taking each item's line from its own span, so a cursor inside a
macro body classifies against the body line it is really on.

## Wiring a generic LSP client

Any client that speaks LSP 3.17 over stdio can launch `pmt lsp` or
`tmt lsp` directly — no special client extension is required. Two
examples, showing both servers side by side:

### Neovim (`vim.lsp.config` / `vim.lsp.enable`, 0.11+)

```lua
vim.lsp.config.pmt = {
  cmd = { "pmt", "lsp" },
  filetypes = { "pmc", "pma" },
  root_markers = { "pmt.json", ".git" },
}
vim.lsp.config.tmt = {
  cmd = { "tmt", "lsp" },
  filetypes = { "tmc", "tma" },
  root_markers = { "tmt.json", ".git" },
}
vim.lsp.enable({ "pmt", "tmt" })

-- Recognize the four extensions (no bundled filetype plugin ships):
vim.filetype.add({
  extension = { pmc = "pmc", pma = "pma", tmc = "tmc", tma = "tma" },
})
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

[[language]]
name = "tmc"
scope = "source.tmc"
file-types = ["tmc"]
roots = ["tmt.json"]
language-servers = ["tmt-lsp"]

[[language]]
name = "tma"
scope = "source.tma"
file-types = ["tma"]
roots = ["tmt.json"]
language-servers = ["tmt-lsp"]

[language-server.pmt-lsp]
command = "pmt"
args = ["lsp"]

[language-server.tmt-lsp]
command = "tmt"
args = ["lsp"]
```

### Editor shells

This repository ships two editor integration pairs under `editors/`: a
VS Code extension and a JetBrains plugin for the PM-1 toolchain
(`editors/vscode-pm/`, `editors/jetbrains-pm/`), and the same pair for the
TM-1 one (`editors/vscode-tm/`, `editors/jetbrains-tm/`). All four are
thin shells over the corresponding `lsp` subcommand plus the standalone
build/lint/format commands, and all four are sideload-only — built locally
from source, with no marketplace listing. Each directory's `README.md`
carries its own install, build, and sideload instructions plus a manual
test checklist. The wiring above talks to the same binaries those shells
launch, so nothing here is shell-specific: a generic client and a shipped
shell differ only in launch mechanics, never in a server's behavior.
