# `pmt fmt` — the `.pmc` formatter — design

Date: 2026-07-07
Status: draft (awaiting human review of this written spec before any implementation)
Tracker: [machine-toolchains#3](https://github.com/mellonis/machine-toolchains/issues/3)

## Context

`pmt lint` shipped from
`docs/superpowers/specs/2026-07-06-pmc-lint-layer-design.md` and ruled,
during that review, that layout normalization is **not** lint's job:
indentation and blank-line policy belong to a formatter that reprints
wholesale, not to per-finding edits. The lint spec parked a requirements
list "for the fmt phase" and moved two textual-hygiene rules
(`trailing-whitespace`, `final-newline`) out of the lint catalog into
fmt. `pmt fmt` is a follow-on phase of the same issue (#3), with its own
spec (this document) and plan cycle. It is the fix side of lint's
report-only `line-too-long` rule.

The parked requirements (lint spec, "Parked for the fmt phase") are this
design's input:

- Blank-line policy.
- Canonical indentation within functions.
- Multi-command (comma-group) wrapping.
- Overlong-line rewrapping (the `line-too-long` fix).
- Canonical intra-statement token spacing (`@qq();`, `1:`, `std::x`, no
  space before `,`/`;`, one space after `,`).
- Contracts: idempotent, behavior-preserving, comments preserved,
  `--check` for CI.
- Trailing-whitespace removal + exactly-one-final-newline.
- The prerequisite the lexer does not have today: **comments retained
  with positions** so a reprinter can re-emit them.
- IDE/LSP note: fmt integrates as the document-formatting provider, never
  as per-position diagnostics.

**No longer fmt's job — resolved upstream.** The lint spec also parked
"builtin successor paren normalization (`left()` → `left`)". Since then,
empty parens on a tape builtin became a **syntax error** in grammar 0.2
(a separate, already-shipped tightening — parens on a builtin, if
present, must carry a successor; call parens `@f()` stay legal). So
`left()` can never reach fmt, and fmt performs **no** token normalization
for it. This matters for the whole design: **fmt makes zero token
changes** (see Decisions). It is a pure whitespace / blank-line /
comment-position transform.

### The load-bearing finding: the compiler's parse output is unusable for reprinting

The lint spec's working assumption — "fmt reuses lex/parse but not
flatten/lower" — is a first approximation that reading the code
falsifies. Two losses make the compiler's *current* `parse()` output
(`Program`) unfit to drive a structure-preserving reprint:

1. **`parse()` flattens namespace blocks at parse time.** `parser.rs`
   has no `namespace { }` node: `top_items` recurses into a block and
   stamps each definition with an `ns: Vec<String>` tag, then discards
   the block boundary. `Program { functions, imports }` is a *flat* list
   across all namespaces. Reprinting from it would have to *reconstruct*
   blocks from `ns` tags — forcibly **merging reopened blocks** (which the
   language allows and the author may have split deliberately) and
   losing the interleaving of imports, functions, and nested blocks.
2. **`parse()` splits a body's nested functions out of source order.**
   `Function.body: Vec<Statement>` and `Function.nested: Vec<Function>`
   are separate lists; a nested definition that appeared *between* two
   statements loses its position. Nested functions are hoisted, so this
   is layout-only — but a formatter is exactly the tool that must not
   move it.

And `analyze().ast` is worse still: `flatten()` mangles names to their
full compiled form (`std::api.helper`), hoists nested functions to the
top level, and rewrites call names to resolved symbols. A reprint from
that would rename the user's source.

**Consequence:** fmt needs a *source-faithful* view — top-level item
order, namespace block structure and reopening, nested-function /
statement interleaving, token spellings as written, and comments. The
next section is how that view is produced.

## Architecture: one unified lossless CST (option C / C1)

The earlier draft of this spec built a **dedicated fmt-side** concrete
syntax tree and kept the compiler's parser untouched. This draft takes
the option the human chose instead:

- **`parse()` builds one lossless concrete syntax tree (CST)** — the
  single source-faithful tree, retaining namespace blocks, top-level and
  in-body ordering, token spellings, and comments (as trivia). There is
  **one parser**, not two.
- **The compiler lowers a *copy* of the CST into the existing owned AST**
  (`Program`) — this is flavor **C1**. The namespace-flattening and
  nested-function hoisting that `parse()` does *today* become this
  CST → AST lower-copy step; the CST keeps the pre-flatten structure fmt
  needs. Trivia is dropped in the copy.
- **The compiler's semantic passes stay UNCHANGED.** `flatten` (now fed
  by the lower-copy), `ir::lower`, the optimizer, and codegen see the
  same `Program` they see today.
- **fmt reads the CST directly** and pretty-prints it.
- **The future LSP (#4) reuses the same CST** — this is the reason to
  build one shared tree rather than a throwaway fmt-only one.

**Honest framing of the cost.** This is a *larger* change than the
earlier draft's "additive lexer channel, parser untouched". C1
restructures `parse()`'s internals: `tokens → CST → lower-copy → AST`
replaces `tokens → AST`. What is preserved is the parser's **output**:
the `Program` the compiler consumes is provably identical, and every
pass downstream of it is untouched. Two guards carry that weight (they
matter far more than when the change was additive-only):

1. **Parity test** — the `Program` produced by (CST → lower-copy) is
   equal to the `Program` produced by the old `parse()`, and the two
   accept/reject the same inputs, across the whole corpus and every
   parser grammar test case.
2. **Byte-identical compile/lint** — `pmt compile` and `pmt lint` output
   is byte-for-byte unchanged; `PMC_LANG_VERSION` stays **0.2**.

**C2 (zero-copy typed views over the CST, rust-analyzer/rowan style) is
out of scope → issue #14.** C1's lower-*copy* allocates two trees, cheap
at `.pmc` scale; C2 revisits only under large-file perf pressure or if
the LSP prefers a view layer.

### Comments = trivia-tokens native in the CST

The lexer discards `//` and `/* */` today. It gains a mode:

- **`WithoutComments`** (the compiler's path) — the significant-token
  stream is byte-identical to today; the lower-copy AST is unchanged.
- **`WithComments`** (fmt's path) — comments are retained as **trivia**,
  interleaved at their source positions in the token stream and carried
  into the CST as trivia nodes between significant nodes.

Because comments live in the CST at their real positions, they are
"attached by position" structurally — there is no separate side-channel
list and no post-parse attachment pass. The pretty-printer's only job is
*how* to re-emit each comment, per its position:

- **Leading** — an own-line comment (or a run of them, no blank line
  between) directly above a node: re-emitted on its own line(s) at the
  node's indent. This is the `std.pmc` doc-comment shape.
- **Trailing** — a comment after a node's last token on the same physical
  line: re-emitted on that line after the node's text, one space before
  the `//`.
- **Standalone** — an own-line comment separated by a blank line from the
  surrounding nodes: kept in place, its blank-line separation preserved
  (subject to the blank-line policy).
- **Dangling** — own-line comments at the end of a scope with no
  following node: emitted at the scope's body indent before the closing
  `}` / EOF.

Re-indentation (content fidelity):

- **Line comments** (`//`): re-indent each line of a leading/dangling
  group to the target indent; the text after `//` is untouched.
- **Block comments** (`/* */`): set the indent of the **first** line
  only; interior lines are preserved **verbatim** — the safest choice for
  ASCII-art or aligned block content.

**Position is preserved — no comment ever moves lines.** A comment
*inside* a statement is handled by the general trivia mechanism, not a
special case: a mid-group **block** comment (`a, /* x */ b;`) stays
inline; a mid-group **line** comment (`a, // x`) ends its physical line,
so the comma group now contains a newline — it is "broken", and the
comma-group rule below preserves the author's line split. The earlier
draft's "attach mid-statement comments as trailing to the whole
statement (moved to end of line)" rule is **deleted**: it was an artifact
of the side-channel model and contradicts position preservation.

## Decisions

| Decision | Choice |
|---|---|
| Surface | `pmt fmt` subcommand only; compile/lint/asm/link unchanged |
| Model | **Wholesale reprint** from the lossless CST. Never per-`Edit` splicing |
| Source view | The compiler's `parse()` builds **one** lossless CST; fmt reads it directly; compile lowers a *copy* to the existing `Program` AST (C1). No second parser, no dedicated fmt tree |
| Gate | fmt formats any file that **lexes + parses**; it does NOT require flatten/resolve. A file with an undefined-label or duplicate-binding (post-parse, semantic) still formats — layout is semantics-independent, and WIP should format. Parse/lex failure → typed error, reported + skipped, batch continues. Deliberately a **weaker gate than lint's** (lint requires parse AND resolve) |
| Comments | Retained by the lexer as **trivia** in `WithComments` mode, native in the CST at source position; the compiler lexes `WithoutComments` and its token stream is byte-identical |
| Token content | fmt makes **zero token changes** — a pure whitespace / blank-line / comment-position transform. Every token spelling is verbatim: leading zeros stay (`007` is lint's `leading-zeros` fix, not fmt's), call names are never re-mangled (`@goToEnd` stays `@goToEnd`), and empty builtin `()` cannot occur (grammar 0.2 rejects it). The two channels never double-fix |
| Indentation | **4 spaces per block level**, never tabs; grounded in the committed corpus (`std.pmc`) |
| Comma-group layout | Author's line breaks respected; single line when it fits, greedy-fill on overflow (see Formatting rules) |
| Line limit | 80 characters, matching lint's `line-too-long`. fmt is that rule's fix |
| CLI write behavior | **In-place by default** for `PATH...` (the directory-walk batch model forces it); `--check` is the dry-run; `pmt fmt -` streams stdin → stdout |
| `--check` | Exit 1 if any file would change; print the list of files that would be reformatted, write nothing. Exit 0 = all already formatted |
| Batch model | `pmt fmt PATH...` mirrors lint: files + recursive `*.pmc` dir walk (sorted, no symlinks, dot-entries skipped), `--exclude PATH` (prefix, wins over explicit), zero-match PATH is an error, per-file independence |
| stdin/stdout | `pmt fmt -` reads one `.pmc` from stdin, writes the formatted text to stdout; single input, no paths, no dir walk. Editor "format on save" and pipeline use. Shares the library `format()` with the future LSP |
| Grammar/version | fmt changes **no** accepted grammar; whitespace is already insignificant except the sigil, which the grammar enforces. `PMC_LANG_VERSION` stays **0.2** (the empty-builtin-paren tightening that made it 0.2 shipped separately, before fmt). No version space moves |
| Thin renderer | Same rule as lint: formatting is library-side and returns a typed result — `format(source: &str) -> Result<String, CompileError>`; `cli/fmt.rs` is the only place that renders errors and touches the filesystem |
| Config | None — opinionated, fixed rules (the gofmt model). Width 80 / indent 4 are not tunable this round (a manifest is the parked future home, per the lint spec) |
| Dogfood | `fmt(std.pmc)` byte-identical to committed `std.pmc` is a hard acceptance criterion (the mirror of lint's "stdlib lints clean") |

Out of scope this round: `.pma`/assembler formatting (the assembler has
no CST); configurable width or indent; import reordering/merging (order
is preserved verbatim — reordering is a code transform, not layout, and
first-wins duplicate semantics make order significant); comment
*rewrapping* or *content* edits (fidelity over prettiness); the LSP
formatting provider (issue #4 consumes the CST this spec introduces);
**C2** zero-copy CST views (issue #14); **lint over stdin** — a linter
reading stdin also needs a `--stdin-filename` to label its `FILE:LINE:COL`
findings for the editor, unlike fmt whose output carries no path;
deferred to an LSP-era follow-on, where the LSP is the proper in-editor
lint channel.

## Formatting model

Pipeline (all library-side, in a new `post-machine/src/fmt/` module,
sharing `parse()`):

```
source ──parse (WithComments)──▶ lossless CST (trivia at source positions)
              │
              ├─ compile / lint: lower-copy CST → Program (trivia dropped)
              │                   → flatten → ir::lower → optimize → codegen  (UNCHANGED)
              │
              └─ fmt: pretty-print CST ──▶ Result<String, CompileError>
```

- fmt lexes once `WithComments`, `parse()` builds the CST, the
  pretty-printer walks it.
- The **pretty-printer** emits canonical layout per the rules below and
  re-emits trivia at their positions.
- A file that fails to lex or parse yields `Err(CompileError)`; the CLI
  renders it (`FILE:LINE:COL: error: …`, the compiler's rendering) and
  skips the file; the batch continues.

### Formatting rules

All concrete, no placeholders.

**Indentation.** 4 spaces per block level, never tabs. File level = 0.
`namespace ns { … }` contents = +1. A function body = +1 from its
header. A nested function body = +1 from its header. (In `std.pmc`,
`export goToEnd()` sits at 4 spaces inside `namespace std {`, its body at
8 — the grounding corpus.) Input tabs and CRLF are normalized away by the
full reprint; output is LF with 4-space indents.

**Headers and braces.** A declaration header (`name() {`,
`export name() {`, `namespace ns {`) sits at its enclosing indent; the
opening `{` stays on the header line, preceded by exactly one space; the
closing `}` is alone on its own line at the header's indent. Functions
take no parameters, so the parens are always `()` tight.

**Statements.** One statement per line, `;`-terminated. Two statements
the author placed on one line (`left; right;`) are split to one per line
— this is canonical, deliberately unlike the comma-group rule below: a
comma group is one statement's internal layout, distinct statements are
not.

- Label prefix at the body indent: `N:` (number tight to colon); stacked
  labels are each `N:` separated by one space (`1: 2:`); one space follows
  the final colon, then the first command. An unlabeled statement's first
  command sits at the body indent directly. (Commands are therefore *not*
  aligned to a fixed column across labeled and unlabeled lines — this
  matches `std.pmc`, where `check(...)` starts left of `right` under
  `1: right`.)

**Comma-group layout.** A statement's comma group (`cmd, cmd, cmd;`) is
laid out by respecting the author's line breaks, with a width fallback.
The signal is whether the author put a **newline inside the group**:

1. **No newline, fits (≤ 80):** one line — `cmd, cmd, cmd;`, each `,`
   tight to the preceding command, one space after.
2. **No newline, overflows (> 80):** *greedy-fill*. Pack commands onto
   the line while they fit within 80; break after the **last comma that
   fit**, the comma trailing the line it closes; the remainder shifts to
   a new line indented to the first command's column (body indent + the
   width of the `N: ` label prefix, or just the body indent when
   unlabeled); repeat for the remainder. The final `;` rides the last
   command's line.
3. **Newline present (author split it):** preserve the author's line
   grouping — the per-line command counts are kept as written (different
   counts per line are fine) — and align each line's first command to the
   continuation column. Greedy-fill (rule 2) is applied **only** to a
   preserved line that itself exceeds 80.

```
# rule 1 (fits)                # rule 3 (author split 2 + 1, preserved)
1: left, right, mark;          1: left, right,
                                  mark;
```

A statement with no comma to break on (a single long command, e.g. a
long qualified call) cannot be wrapped and stays overlong; `line-too-long`
still reports it. fmt lays out statement-level comma groups only — not
check-arm commas or import-list commas (no break point is specified for
them, and neither appears overlong in the corpus).

This layout is **idempotent**: after rule 2, every emitted line is ≤ 80,
so a re-run sees "newline present" (rule 3) and preserves; an
author-split group is preserved as-is on every pass.

**Intra-statement token spacing** (canonical):

| Construct | Canonical |
|---|---|
| Call | `@name(...)` — `@` tight to name (grammar), name tight to `(` |
| Builtin + successor | `left(5)`, `mark(!)` — no space before `(`, contents tight |
| `check` | `check(1, 3)` — tight `(`, one space after the arm comma, tight `)` |
| `goto` | `goto 5` — one space |
| Label | `1:` tight; stacked `1: 2:`; one space after the final colon |
| Path | `std::api::run` — `::` tight |
| `,` `;` | tight to the preceding token; one space after `,`, newline after `;` |
| `as` (imports) | one space each side: `their::name as alias` |
| `!` | `(!)`, `check(!, 1)` — tight |

Spaced forms the grammar still accepts (`1 : right`, `std :: goToEnd`,
`@qq ()` is already a lex error) are normalized to the tight form above.
fmt strips no tokens: empty builtin `()` cannot occur (grammar 0.2), and
mandatory call parens (`@f()`) are never touched.

**Imports.** Spacing normalized (`use a, std::b as c;`); order and the
grouping of paths within each `use` list preserved verbatim — fmt neither
reorders nor merges/splits `use` statements.

**Blank lines** (layered; more specific wins):

1. **Cap:** a run of 2+ consecutive blank lines collapses to **one**.
2. **Brace edges:** no blank line immediately after `{` or immediately
   before `}` (function bodies, namespace blocks).
3. **Otherwise, preserve:** the author's blank lines are kept as written,
   subject to rules 1–2. fmt **never adds or removes** a lone blank line
   (or its absence) — between declarations, between statements, between
   adjacent `use` statements, or around standalone comments. A
   declaration's leading comment group travels with it; a blank the
   author put above the comment stays above the comment.

There is deliberately **no** "exactly one blank between declarations"
rule — fmt respects the author's vertical rhythm (matching the
comma-group choice), collapsing only runs. On the committed `std.pmc`
(one blank between each function, no runs), this is a no-op, so the
dogfood criterion holds.

**Textual hygiene** (falls out of the full reprint): trailing whitespace
removed on every line; exactly one final newline; LF line endings.

**Edge cases.** An empty file, or a file of only comments (no
declarations), reprints its comments with one final newline. An empty
function body `f() { }` prints as `f() {\n}` (header + closing brace, no
blank line between). A dangling comment before the closing brace prints
at the body indent.

## CLI: `pmt fmt`

```
pmt fmt PATH... [--exclude PATH]... [--check]
pmt fmt -       [--check]
```

- One dispatch arm + a line in the top-level `USAGE` (`cli/mod.rs`); a new
  thin renderer `cli/fmt.rs`. All formatting is library-side via
  `format(source) -> Result<String, CompileError>`.
- **Batch model (`PATH...`)** — identical to lint (`cli/lint.rs`'s walk is
  the template; share or mirror it): each PATH is a file or directory;
  directories walk recursively for `*.pmc` in sorted order, never
  following symlinks, skipping dot-entries. `--exclude PATH` (repeatable,
  prefix semantics, wins over explicit args). A zero-match PATH is an
  error. Files format independently; a per-file parse/lex fatal is
  reported on stderr and the batch continues.
- **Default (no `--check`):** format each file and rewrite it in place — a
  write happens only when the formatted text differs from the file's
  current contents (no spurious mtime churn on already-formatted files).
- **`--check`:** format in memory, write nothing; print the path of each
  file whose formatted text differs; exit 1 if any file differs, else 0.
- **stdin/stdout (`-`):** read one `.pmc` from stdin, write the formatted
  text to stdout. `-` is the sole input — it does not combine with
  `PATH...` and triggers no directory walk. On a parse/lex error nothing
  is written to stdout, the error goes to stderr, and the exit is nonzero.
  With `--check`, `-` writes nothing and exits 0 (already formatted) or 1
  (would change). This is the "format on save" channel for editors without
  an LSP, and for shell pipelines / git filters.
- **Exit codes:** 0 = success (all formatted / nothing to change);
  1 = (with `--check`) at least one file / stdin would change, OR any
  tool/parse error anywhere (matching lint's and `-Werror`'s precedent).
  A file with a fatal parse error is never written.

Draft USAGE (final wording lands with the plan, mirroring the lint
block's density):

```
USAGE: pmt fmt PATH... [--exclude PATH]... [--check]
       pmt fmt - [--check]

PATH is a .pmc file or a directory; directories are walked recursively
for *.pmc (sorted order, symlinks not followed, dot-entries skipped).
`-` reads one .pmc from stdin and writes the result to stdout.

FLAGS:
  --exclude PATH  skip a file or prune a directory subtree (repeatable;
                  plain paths compared as spelled — no globs)
  --check         do not write; with PATH..., list files that would be
                  reformatted and exit 1 if any would change; with -,
                  exit 1 if stdin would change (CI mode)
```

## Contracts

- **Idempotence** — `fmt(fmt(x)) == fmt(x)`. The reprint is a pure
  function of the CST + comment layout, both canonicalized by the first
  pass (the comma-group layout is idempotent by construction, above).
  Tested by formatting the corpus and every fixture twice and asserting
  the second pass is a no-op, plus a property test over generated
  programs.
- **Behavior preservation** — `fmt(x)` and `x` compile to the same bytes.
  Verified three ways, weakest to strongest:
  1. **Token equivalence:** lex both `WithoutComments`, assert identical
     `(kind, value)` sequences. (fmt changes no tokens, so this is exact —
     no normalization clause needed.)
  2. **Comment fidelity:** the sequence of comment texts (trimmed) is
     identical between `x` and `fmt(x)`.
  3. **Compiled-output identity (the strong check):** `compile(fmt(x)) ==
     compile(x)` byte-for-byte at `-O0` and `-O1`, for every corpus
     program. The derivation-first mirror of the lint spec's
     "byte-identical object" criterion; subsumes (1).
- **`--check` (CI mode)** — exit non-zero iff any input's formatted text
  differs from its current contents; nothing is modified. Tested: exit 0
  + no output + no write on formatted input; exit 1 + the path listed +
  no write on unformatted input; the `-` variants.
- **C1 parity (the compiler-path guard)** — the `Program` from
  (CST → lower-copy) equals the `Program` from the old `parse()`, and the
  two accept/reject identically, across the corpus and every parser
  grammar test case. Together with byte-identical `compile`/`lint` output
  and an unchanged `PMC_LANG_VERSION`, this bounds the C1 restructure to
  zero observable behavior change.

## Testing (mirrors lint's structure)

- **Per-rule unit tests** in the fmt module (`#[cfg(test)] mod tests`):
  inline source → `fmt` → assert exact output. One focused case per rule
  — indentation, blank-line policy (cap-to-one, brace edges, author
  preservation), comma-group single-line vs greedy-fill overflow vs
  author-split-preserved, the label-width-dependent continuation column,
  token spacing, spaced-form normalization,
  leading/trailing/standalone/dangling comment placement, block-comment
  interior preservation, statement-splitting (`left; right;` → two lines).
- **`fmt_programs.rs`** integration suite (`crates/post-machine/tests/`):
  multi-construct programs; idempotence (double-format no-op);
  behavior-preservation (`compile(fmt(src)) == compile(src)`); comment
  fidelity.
- **CLI tests** in `cli_programs.rs`: in-place write round-trip (write
  only when changed); `--check` exit 0/1 and no-write; `-` stdin→stdout
  and `-` + `--check`; the batch model (dir walk sorted, `--exclude`
  prunes a subtree and an explicit file, a dot-dir skipped, a zero-match
  PATH errors, a per-file parse fatal reported without stopping the
  batch). Reuse the lint batch harness shape.
- **C1 parity guard:** a test asserting (CST → lower-copy) `Program`
  equals the old `parse()` `Program` and that the two accept/reject
  identically across the corpus and every parser grammar test case — the
  guard that the shared-CST restructure changed no compiler behavior.
- **Property tests** (`proptest`, already a core dev-dep; add to
  post-machine): idempotence and token-equivalence over generated
  well-formed programs.
- **Dogfood:** `fmt(std.pmc)` is byte-identical to the committed
  `std.pmc`. If it differs, `std.pmc` (or the rule) is fixed first — the
  same discipline as lint's "stdlib lints clean". The golden `sum.pmc` /
  `ty.pmc` and the lint fixtures are asserted fmt-clean (or committed in
  fmt-clean form).

## Documentation

- **New `docs/fmt.md`:** the canonical style (indent, blank lines,
  spacing table, comma-group layout, comment handling), `--check` and `-`
  semantics, exit codes. Ref-free prose (published-docs policy — no
  issue/PR numbers, no forge URLs). It must describe the blank-line
  policy as "preserved, runs collapsed to one, none forced" — **not**
  "one blank between declarations".
- **`docs/cli.md`:** a `fmt` subcommand section (USAGE block + prose)
  mirroring the `lint` section, including `-`; add `fmt` to the top-level
  subcommand list in `cli/mod.rs`'s `USAGE`.
- **`docs/lint.md`:** `line-too-long`'s note already says "a formatter's
  job" — extend it to name `pmt fmt` as the fix (comma-group layout), and
  to state that a line overlong due to a single long command or a trailing
  comment is not fmt-fixable and stays reported.
- **`docs/language.md`:** its illustrative snippet uses a pre-fmt
  indentation that the canonical style supersedes. Regenerate the snippet
  to canonical style (or add a one-line pointer that `docs/fmt.md` owns
  canonical layout). Minor; no grammar change.
- **`README.md`:** one-line `pmt fmt` mention in the CLI overview.
- **Version block:** fmt's release notes state every version space
  `unchanged` — no grammar/IR/`.pma`/container move (the lexer comment
  channel is additive; the empty-builtin-paren tightening that moved the
  grammar to 0.2 shipped earlier, separately).
- **Completion registry:** add a `fmt_spec()` to `completions::registry`
  (mirroring `lint_spec()`), so `pmt completions` covers `fmt`; the
  registry drift guard already probes each entry against the real parser.

## Acceptance criteria

1. `fmt(std.pmc)` is byte-identical to the committed `std.pmc`; the
   golden `sum.pmc`/`ty.pmc` and lint fixtures are fmt-clean (or committed
   in fmt-clean form).
2. For every committed `.pmc` program, `compile(fmt(src))` is
   byte-identical to `compile(src)` at `-O0` and `-O1`, and
   `fmt(fmt(src)) == fmt(src)`.
3. Comment fidelity: the trimmed sequence of comment texts in `fmt(src)`
   equals that in `src` for the corpus and a dedicated comment fixture
   (leading/trailing/standalone/dangling, line and block); no comment
   changes line position.
4. `pmt fmt --check` exits 0 with no output and no write on formatted
   input, and exits 1 listing the file with no write on unformatted
   input; `pmt fmt` writes in place and is a no-op (no write) on
   already-formatted files; `pmt fmt -` streams stdin → stdout and
   `pmt fmt - --check` mirrors the exit semantics; the batch survives a
   per-file parse fatal.
5. C1 parity: (CST → lower-copy) `Program` equals the old `parse()`
   `Program`, with identical accept/reject, across the corpus and the
   parser grammar suite.
6. `pmt compile` and `pmt lint` output is byte-identical to pre-change;
   `PMC_LANG_VERSION` is still `0.2`; `cargo test --workspace`,
   `cargo clippy --workspace --all-targets -- -D warnings`, and
   `cargo fmt --check` all pass.

## Open decisions for human review

The design is settled; the earlier draft's open forks were resolved
during design:

- **Foundation (one parser vs two)** — resolved as **one unified lossless
  CST + C1 lower-copy** (shared with the LSP), not a dedicated fmt tree.
  C2 (zero-copy views) parked → issue #14.
- **Comma-group layout** — resolved as **respect the author's breaks +
  greedy-fill on overflow** (rules 1–3 above), not one-command-per-line.
- **Mid-statement comments** — dissolved: trivia-in-CST + position
  preservation handle block (inline) and line (group-becomes-broken)
  comments with no special rule; the earlier "move to end of line" rule
  is deleted.
- **Blank-line policy** — resolved as **preserve, collapse runs to one,
  force nothing**.

No open forks remain. The one item left for the reviewer's ruling is
cosmetic: **regenerate the `docs/language.md` snippet** to canonical
style, or leave it and point to `docs/fmt.md`? (Noted so the reviewer
rules rather than the implementer guessing.)
