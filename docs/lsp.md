# The `.pmc` language server — `pmt lsp`

`pmt lsp` runs a Language Server Protocol server for `.pmc` on stdio,
built on the exact same lexer, parser, compiler, optimizer-free
analysis, and linter the CLI uses. Nothing the server reports is a
re-implementation: a diagnostic, a quickfix, or a formatted document is
the same answer `pmt compile`/`pmt lint`/`pmt fmt` would give for the
same source. See `docs/cli.md` (`pmt lsp`) for the subcommand's flags,
stdio contract, and lifecycle exit codes.

## What it serves

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

`.pma` (the assembler dialect) is not served by this milestone.

## Capabilities

Every feature answers from the document's current text or degrades
predictably — never a stale position, never a resolution-free guess.
Three analysis tiers gate what a feature can answer:

| Feature | Needs | Degrades to (when the tier fails) |
|---|---|---|
| Diagnostics: fatal error | the source lexes | one error at the failing stage, honest and singular |
| Diagnostics: compile warnings + lint findings | a full successful analysis | omitted — the fatal is the only entry |
| Completions | tokens/CST for cursor context | candidate *names* may fall back to the last successful analysis, so completion stays useful mid-edit — the one sanctioned staleness exception |
| Go-to-definition | a full successful analysis (the resolution table) | `null` |
| Code actions (quickfixes) | a full successful analysis (lint ran) | empty list |
| Semantic tokens | a full successful analysis (resolution-aware) | `null` — clients keep the previous tokens or static grammar coloring |
| Document symbols | a successful parse (CST only) | `null` |
| Formatting | a successful parse (CST only) | `null` — the parse error is already on screen as a diagnostic |

A handler panic is caught per request: it never takes the session
down, degrades that one answer to an internal error, and the next
message is served normally.

## Wiring a generic LSP client

Any client that speaks LSP 3.17 over stdio can launch `pmt lsp`
directly — no special client extension is required. Two examples:

### Neovim (`vim.lsp.config` / `vim.lsp.enable`, 0.11+)

```lua
vim.lsp.config.pmc = {
  cmd = { "pmt", "lsp" },
  filetypes = { "pmc" },
  root_markers = { "pmt.json", ".git" },
}
vim.lsp.enable("pmc")

-- Recognize the extension (no bundled filetype plugin ships yet):
vim.filetype.add({ extension = { pmc = "pmc" } })
```

### Helix (`languages.toml`)

```toml
[[language]]
name = "pmc"
scope = "source.pmc"
file-types = ["pmc"]
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

`.pmc` projects have exactly one config file, `pmt.json`, read by both
the CLI and the server — see `docs/lint.md` for its schema, discovery
rule (nearest ancestor wins, never a cascade), and union semantics.
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
