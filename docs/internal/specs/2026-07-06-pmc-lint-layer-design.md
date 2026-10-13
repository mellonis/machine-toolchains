# .pmc lint layer — design

Date: 2026-07-06
Status: approved 2026-07-06 (full-review pass + pre-plan audit done)
Tracker: [machine-toolchains#3](https://github.com/mellonis/machine-toolchains/issues/3)

## Context

v1 shipped a structured-diagnostics channel: four compiler warnings
(`Warning { line, message }` accumulated into `CompileReport`) plus
`-Werror`, rendered by the CLI under the thin-renderer rule. Issue #3 grows
that channel into a lint layer. The first substantial user program (an
8-bit summing showcase, ~230 statements) settled the flagship rule's
semantics: five unused labels found by manual sweep, invisible to the
existing warnings by design — labels cost zero bytes in the binary, so
unused ones are pure source hygiene, which belongs at lint level, not as a
compile warning.

The v2 backlog's LSP issue (#4) wants the same underlying refactor:
machine-readable diagnostics with codes and real spans. This design serves
both consumers with one model (LSP severity levels are the future
adapter's mapping job, derived from the rule code).

## Decisions (settled during brainstorming, 2026-07-06)

| Decision | Choice |
|---|---|
| Scope | `.pmc` rules + framework; `.pma` lints deferred (assembler has no diagnostics channel yet) |
| Surface | `pmt lint` subcommand only; `pmt compile` unchanged |
| Diagnostic model | codes + structured fix data (a severity field was considered and dropped during spec review — after the strict channel split its value would be fully determined by the producing entrypoint, i.e. redundant state; levels retrofit additively when LSP or `--deny` needs them) |
| Unification | one `Diagnostic` type; the 4 existing compile warnings migrate onto it; `Warning` deleted |
| Spans | full span refactor everywhere — `CompileError`, warnings, lints |
| Rule config | `--allow <code>` (repeatable) suppression; unknown code is an error |
| Lint output | lint findings only — `pmt lint` does not re-report the compile warnings (strict channel split: compile → warnings, lint → lints) |
| Type home | diagnostic primitives live in `mtc-core` (arch-agnostic; the `.pma` follow-up and future `tmt` reuse them) |
| Architecture | dedicated lint module over a shared codegen-free `analyze()` entrypoint |
| `--fix` | in scope: in-place rewrite, general bottom-up edit applier, remaining-findings exit code |
| Fix applicability | two tiers on `Fix` (rustc's model): `MachineApplicable` applied by plain `--fix`; `MaybeIncorrect` requires `--fix --force`. Policy line: plain `--fix` never deletes or rewrites user-written constructs on an ambiguous diagnosis — the gated set grew to `unused-label`, `redundant-jump-to-next`, `leftover-debugger` (deletions) and `identical-check-arms` (replacement) |
| Textual hygiene | `trailing-whitespace` + `final-newline` were briefly lint rules (added to give plain `--fix` occupants while fmt was unplanned); once fmt became a committed phase of this issue they moved to the fmt phase — whitespace normalization is a formatter competency. `leading-zeros` keeps the safe tier non-empty. Layout normalization was never lint's job |
| fmt tracking | `pmt fmt` is folded into issue #3 as a follow-on phase (author's ruling during spec review, over a separate-issue recommendation): lint ships first from this spec; fmt gets its own spec/plan cycle under #3, seeded by the parked requirements below |
| Single-statement sibling rule | folded into `unused-label` (subsumed: unreferenced ⇒ already caught; referenced ⇒ self-loop, undeletable) |
| Rules added in review | `non-camel-case`, `line-too-long`, `leading-zeros` joined the catalog at user request during spec review (born as `camel-case` / `line-length`; defect-renamed so every code reads as plain-English permission under `--allow`) |
| Adjacency rules | settled after several iterations (label colon and `::` paths were each briefly ruled errors, both reverted): **only the sigil is grammar-tight** — `@ name` is a syntax error (sigil-adjacency precedent: Ruby, Rust). Spaced label colons (`1 :`) and spaced paths (`std :: x`) are tolerated, matching C/C++/Rust, and fmt normalizes both |
| Error ergonomics | the misleading `namespace {}` message (parser reads the contextual keyword as a function name) gains a targeted did-you-mean hint; empty `namespace ns {}` considered and left legal (harmless, occurs mid-refactor) |
| stdlib-shadow dropped | removed from the catalog during review, re-homed as a future **link-time warning**: compile-side guessing false-positives under `--nostdlib` and fights documented interposition (shadowing library internals is a supported technique); the linker sees link flags and all libraries precisely — lands with the deferred core diagnostics channel |
| Batch linting | `pmt lint PATH...` accepts files AND directories (recursive `*.pmc` walk: sorted order, no symlinks, dot-entries skipped) + repeatable `--exclude PATH` (plain paths, prefix semantics, wins over explicit args; no globs — shell covers the include side). Zero-match PATH errors. Per-file independence; batch survives per-file fatals |
| Grammar versioning | the `.pmc` grammar is versioned per-language, owned by the language's crate. (Placement borrows the `IR_VERSION` pattern — a versioned artifact owned by its module, independent of crate versions — but the two version different KINDS of things: `IR_VERSION` is a serialization-format version, bumped when the JSON encoding changes; the language version is an **acceptance contract**, bumped when the set of accepted programs changes.) Scheme: **major.minor**, starting pre-stability: while <1.0 the version is **0.N and N bumps on ANY grammar change** (breaking or additive — no major exists to carry the distinction); at 1.0, declared when the author judges the language stable, the axes activate (major = breaking acceptance change, minor = additive syntax). **No patch digit** — spec-text corrections are errata (frozen-spec precedent) and implementation-conformance fixes live in the crate changelog; neither moves the grammar. The v1 grammar is retroactively **0.1**; this round's tightenings make it **0.2**. Surfaced, not enforced: `PMC_LANG_VERSION` constant, `docs/language.md` header line, second line in `pmt --version`; no source pragma, no multi-version parsing (single implemented grammar — a pragma is an additive retrofit if an old-corpus need ever appears). TM-1's future language versions independently in its own crate; the toolchain version is NOT the carrier. **`.pma` dialects are the same kind of thing** (acceptance contracts, per-arch, NOT `IR_VERSION`, which encodes JSON): each arch's dialect gets its own version constant when it first changes — PM-1's `.pma` is implicitly 0.1 until then; ruled here, introduced in the deferred `.pma` round |
| Release-notes convention | ruled alongside grammar versioning: release notes open with a **version block** listing ALL version spaces explicitly (toolchain crates, `.pmc` language, `.pma` dialect per arch, IR format, container formats) with `unchanged` stated where nothing moved — the block doubles as a compatibility matrix across releases; component sections follow only where changes exist. Durable home: the repo `CLAUDE.md` release section (this row is the pointer); a future `CHANGELOG.md` uses the same structure in ref-free prose per the published-docs policy, tracker links live in GH release notes |
| Grammar-audit adoptions | reserved words barred in all `::` path segments (was head-only); six-fix error-message pack (evolved in review: the `@goto()` "is a builtin" candidate was dropped — its advice is correct, only its taxonomy loose; a top-level-statement item was added after verification); four audit-sourced lint rules (`redundant-jump-to-next`, `identical-check-arms`, `leftover-debugger`, `namespaced-main`). Parked candidates: self-goto/cycle detection (needs design), redundant-export-on-main, duplicate-use-path, redundant-alias, keyword-named-functions, empty bodies (ruled harmless) |

Out of scope this round: `.pma`/assembler lints (unreachable after `stp`,
canonical grid drift, label ordering), JSON output, `--deny` escalation,
multi-error parser recovery, exposing `analyze()`/the AST as public API,
the LSP server itself, and layout normalization of any kind — indentation
and blank-line policy belong to `pmt fmt` (wholesale reprinting), not to
per-finding lint fixes.

A **project manifest** (config file for lint `allow`/`exclude`, fmt
knobs, compile/link/run flags) was raised and parked as its own future
concern: it's a philosophy change to a deliberately manifest-free,
flags-explicit toolchain and forces format/discovery/precedence/schema
decisions that deserve their own design cycle. Notes for that cycle:
`serde_json` is already in-tree (zero-dependency JSON config); lint's
`allow`/`exclude` and fmt's future knobs are candidate customers; today
the entire persistent need fits in one CI line
(`pmt lint . --exclude vendor --allow line-too-long`). **The strongest
future customer, identified later in review, is dependency
declaration**: third-party libraries link via `-L DIR`/`-l NAME` today
(explicit, per-invocation, no standard on-disk location by design), and
because libraries are first-wins, their ORDER is semantically
significant — a manifest is where "which libraries, from where, in what
deterministic order" becomes a committed artifact instead of a
shell-history fact.

### Parked for the fmt phase (same issue, own spec later)

Requirements accumulated during this review, to seed the fmt design:

- Blank-line policy: no more than 2 consecutive blank lines.
- A blank line before and after each function declaration.
- Canonical indentation within functions (statements, labels, check arms).
- Multi-command statement wrapping: break after a comma (the comma trails
  the line it closes, never opens the next); continuation lines align
  with the first command's column, i.e. just after the `N: ` label
  prefix, so the alignment is label-width-dependent:

  ```
  1: left,
     left;
  ```

  Open sub-question for the fmt design: is one-command-per-line always
  canonical, or only when the single-line form exceeds the line limit?
- Rewrapping overlong lines — the fix side of lint's report-only
  `line-too-long` rule (breaks follow the comma convention above).
- Builtin successor parens: empty `left()` normalizes to bare `left`
  (builtin parens are optional; call parens `@f()` are mandatory and
  stay).
- Canonical intra-statement token spacing. The lexer discards whitespace
  between tokens, so `@qq ( ) ;` parses identically to its tight form;
  fmt normalizes: no space before the call parens (`@qq();`), successor
  labels tight in the parentheses (`@name(5)`, `@name(!)` — the
  committed corpus is uniformly tight), label colon tight to its number
  (`1:`), path `::` tight (`std::goToEnd`), no space before `,` or `;`.
  (Sigil spacing `@ qq` is NOT fmt's — it's a syntax error; see the
  compiler riders below.)
- Contracts: idempotent (formatting a formatted file is a no-op),
  behavior-preserving (token stream identical modulo whitespace),
  comments stay attached to their constructs; `--check` mode for CI.
- Trailing-whitespace removal and exactly-one-final-newline
  normalization (moved out of the lint catalog once fmt became a
  committed phase — whitespace normalization is a formatter competency).
- Known prerequisite lint does not need: comment preservation — the lexer
  must retain comments with positions for a reprinter to re-emit them.
- IDE integration (for the future LSP round): errors, warnings, and lint
  findings publish as positional diagnostics (squiggles; `Fix` data maps
  to quick-fix code actions, applicability tiers to preferred vs
  non-preferred). fmt integrates as the document-formatting provider
  (format-on-save / Format Document), never as per-position diagnostics
  — layout drift squiggles would bury the findings that need human eyes.

## Compiler riders

Small compiler changes riding this round (the lexer/parser are already
open for the span refactor): **two grammar tightenings** (new syntax
errors) and an **error-message pack** (message-only, no acceptance
changes). The grammar audit's root finding — the parser compares token
kinds only, never positions, so whitespace and comments may sit between
any two tokens — is deliberately left in place everywhere except the
sigil: spaced label colons (`1 :`) and spaced paths (`std :: x`) are
tolerated exactly as C/C++/Rust tolerate them, and fmt normalizes both;
only the sigil, which mainstream languages also lex tight, is enforced:

**Sigil adjacency** — whitespace between `@` and the callee name is
rejected. The sigil is part of the name's spelling, not punctuation
between tokens — `@ qq();` and `@ qq ();` error; `@qq ();` stays legal
(space before parens is layout, normalized by fmt).

- Implementation: on `@`, the lexer requires the immediately following
  character to be an identifier start; otherwise it errors `expected a
  function name immediately after '@'`. This also catches `@5`, `@(`,
  and a trailing `@` at lex time (today some of these surface later, in
  the parser).

**Reserved-word path guard** — the reserved-word check covers only the
head segment of a `::` path, so `@std::goto()` parses today. Reserved
names are undefinable in `.pmc`, so such a call can never resolve from
source; the guard extends to every path segment and the call site errors
like the definition site would.

**Error-message pack** — six message-only fixes for errors that
diagnose the wrong thing (found by imagining the user input behind each
`expected(...)` site; none changes what's accepted):

1. `namespace {}` / `use {}` / `export {}` — the contextual keyword
   parses as an identifier and the parser assumes a function named
   `namespace`, reporting `expected '(' after the function name`. Gains
   a did-you-mean hint, e.g. `did you mean 'namespace <name> { … }'?`.
   (The adjacent `namespace ns {}` — a legal, empty namespace — stays
   legal: harmless, occurs mid-refactor.)
2. `use` / `namespace` inside a function body reports "unknown command
   `use` (user functions are called `@use()`)" — the real issue is that
   imports/namespaces aren't allowed inside bodies; say that.
3. A single `:` where `::` was meant (`use std:b;`, `@f:g()`) gains a
   `did you mean '::'?` hint.
4. Namespace naming errors reuse function wording (`namespace goto {}` →
   "cannot name a *function*"; `namespace a {} a() {}` → "duplicate
   *function*") — wording distinguishes namespaces.
5. An unclosed function body dies with "expected a command, found end of
   file" — it now mentions the missing `}`, matching the namespace
   path's existing good message.
6. A statement at top level misdiagnoses (verified): `left;` / `goto 1;`
   → "`left` is a reserved word and cannot name a function"; `@foo();` →
   "expected a function name, found `@`". When top level encounters a
   reserved command word or `@`, the error states the real rule:
   statements are not allowed at top level — commands and calls live
   inside function bodies.

(Another candidate — `@goto()`'s "`goto` is a builtin — write it without
`@`" — was reviewed and **kept as-is**: the advice is functionally
correct for all four control keywords, and `goto() {}` can never be
defined; only the word "builtin" is taxonomically loose.)

**Language version surfacing** — the two tightenings are the first
breaking grammar changes since v1, so this round introduces the grammar
version: `pub const PMC_LANG_VERSION: &str = "0.2";` in the parser
module — the grammar's implementation home — re-exported from the crate
root (pre-1.0 scheme: 0.N bumps on any grammar change; major/minor axes
activate at a future declared 1.0; no patch digit — errata and
conformance fixes don't move the grammar). `pmt --version` prints
`pmc language 0.2` as a second line; `docs/language.md`'s header states
the version it describes. No pragma, no multi-version parsing — the
compiler implements exactly one grammar.

- Technically the two tightenings are breaking grammar changes; the
  committed corpus exercises neither (the audit confirmed the corpus is
  uniformly tight and well-formed), so nothing breaks in practice.
  `docs/language.md` documents both and records the 0.1 → 0.2 bump.
- Tests: lexer unit cases for `@ qq`, `@5`, `@` at end of input; parser
  cases for `@std::goto()` (error), plus `1 : right;`, `1: 2: right;`,
  and `use std :: goToEnd;` (all stay legal), and one message-assertion
  per pack item (`namespace {}` hint, `use` in a body, `use std:b;`
  hint, `namespace goto {}` wording, unclosed body mentioning `}`,
  top-level `left;` / `@foo();` stating the statements-inside-functions
  rule).

## 1. Diagnostic primitives (`mtc-core`)

New module `core/src/diagnostics.rs`. Arch-agnostic by contract — zero
PM-1 knowledge. The core assembler does NOT adopt it this round.

```rust
pub struct Pos  { pub line: u32, pub col: u32 }   // 1-based, character-counted
pub struct Span { pub start: Pos, pub end: Pos }  // end-exclusive
pub struct Diagnostic {
    pub code: &'static str,   // canonical rule id, e.g. "unused-label"
    pub span: Span,
    pub message: String,
    pub fix: Option<Fix>,
}
pub struct Fix {
    pub description: String,
    pub applicability: Applicability,
    pub edits: Vec<Edit>,
}
pub enum Applicability { MachineApplicable, MaybeIncorrect }  // rustc's tiers
pub struct Edit { pub span: Span, pub replacement: String }   // "" = delete
```

### Migrations in `mtc-post-machine`

- `Warning { line, message }` is deleted. The four existing warnings
  become `Diagnostic`s:

  | Code | Detection stage | Span anchor |
  |---|---|---|
  | `undeclared-external` | flatten | first bare call site's name token (dedup per name, program-wide, unchanged) |
  | `unused-import` | flatten | the import path token in the `use` line |
  | `unused-function` | flatten | the function name token |
  | `unreachable-code` | `ir::lower` | the unreachable statement |

- `CompileReport.warnings: Vec<Warning>` → `CompileReport.diagnostics:
  Vec<Diagnostic>`.
- `CompileError { line, col, kind }` → `CompileError { span, kind }`. The
  `col == 0` means-whole-line convention dies; every error site produces a
  real span.
- The lexer's tokens already carry start `(line, col)`; token ends come
  from lexeme length. The parser retains positions where diagnostics need
  them: statement labels become `{ value: u32, span: Span }` with the span
  running from the number's start to the colon's end — spanning any
  interior whitespace, since spaced `1 :` is legal — exactly the
  delete-fix target; function/name tokens keep their spans; items carry
  spans instead of bare lines.
- CLI rendering: `{file}:{line}:{col}: warning: {message}` and
  `{file}:{line}:{col}: lint: {message}`. The prefix is a channel
  property, not a per-diagnostic field: the compile renderer prints
  `warning:`, the lint renderer prints `lint:`. `-Werror` on `compile`
  behaves exactly as today — it escalates any diagnostics present in the
  compile report (lint rules never run under `compile`, so nothing else
  can appear there).

## 2. Analysis entrypoint and lint framework

### `analyze()` split

`compile()` splits internally:

- **Front half** — `analyze(source, opts) -> AnalysisOutput`, running
  lexer → parser → duplicate-binding checks → flatten → `ir::lower`, and
  stopping before the optimizer. `AnalysisOutput` carries the AST, the
  unoptimized `IrProgram`, the accumulated diagnostics, the token stream
  (the `leading-zeros` rule walks it), and a crate-private scope summary
  retained from flatten (per-scope name → binding-kind maps; today
  flatten builds and discards these — the `shadowed-import` rule needs
  them).
- **Back half** — optimize → codegen → assemble, unchanged. `compile()`
  composes both; `-O0` bit-identity is untouched (lint never feeds
  codegen).

`analyze` stays `pub(crate)` this round. The public lint surface does not
commit the AST as API; the LSP issue revisits that.

### Lint module

New `post-machine/src/lint/`:

```rust
pub fn lint(source: &str, options: LintOptions) -> Result<LintReport, CompileError>
pub struct LintOptions { pub allow: Vec<String> }
pub struct LintReport  { pub diagnostics: Vec<Diagnostic> }
```

- `LintReport.diagnostics` = lint findings only, source-ordered by span
  start (stable). The four compile warnings stay on the compile channel —
  `pmt lint` does not re-report them. With no severity/class field on
  `Diagnostic`, this split is structural: cross-leakage between the two
  channels cannot even be represented.
- Rules are plain functions `fn(&LintContext, &mut Vec<Diagnostic>)` over
  `LintContext { source, tokens, ast, ir, scopes }`, registered in a const
  table keyed by code — one file per rule, independently unit-testable.
  Most rules walk the AST/IR; `line-too-long` scans `source` directly;
  `leading-zeros` walks the token stream (which `analyze()` retains for
  this purpose — comment-safe by construction).
- Lint runs on the **unoptimized** IR + AST: rules judge source hygiene,
  not optimizer output.
- `allow` suppresses lint rule codes; the valid set is exactly the rule
  table's. An unknown code in `allow` is an error, not silently ignored
  (typo protection).
- A fatal `CompileError` aborts the lint of that file with normal error
  rendering — lint requires a program that parses and resolves. (This is
  the per-file library contract; the CLI's batch continues with the
  remaining files, per §3.)

### Fix application

```rust
pub fn apply_fixes(source: &str, diagnostics: &[Diagnostic]) -> FixOutcome
pub struct FixOutcome { pub fixed_source: String, pub applied: usize, pub skipped: usize }
```

Pure function in the lint module. All fixes apply as **one batch pass**
against original-source coordinates — no re-analysis between edits. Edits
sort bottom-up by start position, so applying one never shifts the spans
of those still pending above it. A `Fix`'s edits apply atomically; a fix
whose edits overlap an already-applied edit is skipped whole and counted.
(The five fixable rules — `unused-label`, `leading-zeros`,
`redundant-jump-to-next`, `leftover-debugger`, `identical-check-arms` —
produce disjoint edits, so conflicts stay theoretical, but the applier
is general.) **Cascades exist and are handled by reporting, not
looping**: deleting a redundant `goto 5` removes a reference to label 5
and may leave it newly unused; rewriting `check(5, 5)` to `goto 5` may
itself become a `redundant-jump-to-next` finding. The verification
re-lint surfaces such fix-exposed findings in the same run's output (and
exit code), and repeated `--fix --force` converges: every fix strictly
shrinks or simplifies the program, so no oscillation is possible. No
automatic fixpoint loop this round; if one is ever wanted, the re-lint
is the building block, capped.

## 3. CLI: `pmt lint`

```
pmt lint PATH... [--exclude PATH]... [--allow CODE]... [--fix [--force]]
```

- One dispatch arm + USAGE line in `cli/mod.rs`; new thin renderer
  `cli/lint.rs`. All analysis is library-side (thin-renderer rule).
- **Batch model:** each PATH is a file or a directory. Directories walk
  recursively collecting `*.pmc` in sorted order (deterministic output),
  never following symlinks, skipping dot-prefixed entries (`.git`,
  scratch dirs). Files lint independently, in order; a fatal
  `CompileError` in one file is reported and the batch continues. A PATH
  that yields zero `.pmc` files is an error (typo protection, same
  stance as unknown `--allow` codes).
- **`--exclude PATH`** (repeatable): plain paths, not globs — an
  excluded directory prunes its whole subtree, an excluded file is
  skipped exactly, and exclusion wins even over explicitly listed files
  (one rule, no precedence surprises). Shell globs already serve the
  include side; glob patterns can layer onto `--exclude` later if
  practice demands.
- **Exit codes:** 0 = every file clean, 1 = findings or errors anywhere;
  tool errors are also 1 (matching the `-Werror` precedent).
- A finding with a fix renders an indented hint line; a gated fix names
  its gate, so a plain `--fix` run explains why it left things in place:

  ```
  sum.pmc:12:3: lint: label 5 is never referenced (function 'sumBits')
    fix (requires --force): remove the label prefix '5:'
  ```

- **`--fix`:** applies `MachineApplicable` fixes and rewrites each fixed
  file in place (a write happens only for files where at least one edit
  applied), then lints each fixed source again in-memory; rendered
  findings and the exit code come from the re-runs (eslint convention:
  exit reflects *remaining* findings). Plain `pmt lint` is the dry-run —
  it already prints the fix hints. A file with a fatal error is never
  written.
- **`--fix --force`:** additionally applies `MaybeIncorrect` fixes.
  `--force` without `--fix` is an argument error. Tier occupancy this
  round: plain `--fix` applies `leading-zeros` rewrites; `--force`
  unlocks the deletions (`unused-label`, `redundant-jump-to-next`,
  `leftover-debugger`) and the `identical-check-arms` replacement.
  Reclassifying a fix is a one-line change if practice argues otherwise.

## 4. Rule catalog

All rules on by default.

### `unused-label`

Per function — the same scope as label resolution. A label is unused iff
nothing in its function references it: no `goto`, no check arm, no command
successor. Detected on the AST: declared label set (with spans) minus
referenced label set; one diagnostic per unused label.

- Message: `label 5 is never referenced (function 'std::api.helper')` —
  the function is named by its **fully qualified compiled name**
  (namespace path + dot-mangled nesting), the same symbol grammar the
  object format, map sidecar, and graph exports use; for a top-level
  function that's just its bare name.
- Fix: delete the `N:` prefix (its span runs from the number's start to
  the colon's end — including any whitespace between, since spaced
  `1 :` is legal).
  Applicability `MaybeIncorrect` — the edit is behavior-preserving
  (labels cost zero bytes), but an unused label has two readings:
  leftover cruft, or evidence of a jump the author forgot to write.
  Deletion is wrong for the second, so the fix requires `--force`.
  `docs/lint.md` states the caveat: review findings before forcing —
  the fix removes the label, not the underlying omission.
- The single-statement-body observation from the tracker discussion is an
  instance of this rule, not a sibling: an unreferenced label on a
  single-statement body is caught here; a referenced one is a self-loop
  and cannot be deleted. `docs/lint.md` documents this explicitly.

### `shadowed-import`

A function definition whose name outranks an import binding of the same
bare name in the same scope (legal — the language rules say a definition
always wins — but silently confusing: a bare `@name()` call hits the local
function while the `use` line suggests the external). Detected from the
retained flatten scope summary. Same-scope only; cross-scope import
shadowing (inner scope over outer) is legal layering and not flagged.

- Message: `function 'name' shadows the import of 'full::path' — bare calls resolve to the local definition`
- No fix (intent is ambiguous: renaming either side is plausible).

### `redundant-jump-to-next`

A `goto N;` statement — or a builtin/call successor `(N)` — whose target
labels the lexically next statement. Fall-through is identical (codegen's
fall-through layout even elides such jumps), so the explicit jump is
noise. Covers both the statement form and the successor form.

- Message: `goto 5 targets the next statement — fall-through is
  identical` / `successor (5) targets the next statement — drop it`
- Fix: delete the `goto` statement or the `(N)` successor. Applicability
  `MaybeIncorrect` (deletes user-written code, per the policy line).
  **Constraint**: the statement-form fix is offered only when the `goto`
  statement itself carries no labels — deleting a labeled statement
  would orphan references to it. (The successor-form fix deletes only
  the `(N)` inside the line; no such hazard.)

### `identical-check-arms`

`check(N, N)` — both arms name the same label, so the branch is
unconditional: either `goto N` was meant or one arm is a typo.
**`check(!, !)` is exempt**: the language has no `return` keyword and
`(!)` successors need a carrier action, so identical-`!` arms are the
only pure mid-function return — legitimate, and nothing better exists to
suggest.

- Message: `both check arms target 5 — replace with 'goto 5'`
- Fix (standalone `check(N, N);` statement only): replace with
  `goto N` — a replacement, not a deletion, so statement labels stay
  attached; applicability `MaybeIncorrect` (identical arms may be a typo
  for two different targets — forcing bakes in one reading). A
  group-final check (`right, check(5, 5);`) is report-only: the rewrite
  folds into the predecessor's successor (`right(5);`), a
  context-dependent transform left to the human (`goto` is barred from
  comma groups, so simple substitution is not legal there).

### `leftover-debugger`

A `debugger` statement in source. Breakpoints are development
scaffolding — builds strip them with `--strip-debugger` — and an
un-stripped `brk` is an optimizer observability barrier, so shipping one
accidentally also pessimizes `-O1` output. (eslint's `no-debugger`
precedent.)

- Message: `leftover 'debugger' statement`
- Fix: delete the statement. Applicability `MaybeIncorrect` (deletion —
  and the author may be mid-debugging on purpose). **Constraint**: same
  as `redundant-jump-to-next` — offered only when the statement carries
  no labels, so no reference is orphaned.

### `namespaced-main`

A function named `main` inside a namespace. Only the un-namespaced
top-level `main` is ever the program entry, and namespaced `main` is not
auto-exported either — it silently becomes an ordinary local function.
Almost always a misunderstanding.

- Message: `'ns::main' is not the program entry (only top-level 'main'
  is)`
- No fix (intent is unknowable: rename it or move it out).

### `line-too-long`

A line longer than 80 characters (character count, matching the
diagnostic model's character-based positions). Scans the raw source. The
limit is fixed at 80 this round — the whole committed `.pmc` corpus
already fits (stdlib max is 74) — and `--allow line-too-long` is the
opt-out; a configurable width can come later if practice demands it.

- Message: `line is 94 characters long (limit 80)`
- Span: column 81 through end of line (the offending excess).
- No fix: where to break a statement is layout policy — the fmt phase's
  job, not a per-finding edit.

### `leading-zeros`

Any numeric token written with leading zeros — label definitions (`007:`),
`goto` targets, check arms, call successors. The lexer parses digit runs
straight to `u32`, so `007` and `7` denote the same label while looking
unrelated; worse, `07:` plus `7:` in one function is a duplicate-label
error that puzzles anyone who thought they differed. Walks the token
stream (never fires inside comments).

- Message: `'007' has leading zeros — write '7'`
- Fix: rewrite the token to its canonical decimal form. Applicability
  `MachineApplicable` — the value is identical by construction.

### `non-camel-case`

Definition names the user owns — functions, namespaces, and import
bindings — must be lowerCamelCase: `^[a-z][a-zA-Z0-9]*$`. This encodes the
project's de-facto house style (the stdlib is uniformly lowerCamelCase).
Labels are numeric and out of scope. An imported *path* is not the user's
to rename, but its binding is: a default binding that violates the
convention is reported with an alias suggestion.

- Message: `function 'sum_bits' is not camelCase — rename to 'sumBits'`;
  for imports: `import binding 'do_thing' is not camelCase — alias it:
  'use their::do_thing as doThing'`
- No fix, deliberately: a rename is a multi-site edit, and renaming an
  exported function changes its mangled symbol name — link-time ABI, not
  text hygiene. The message carries the mechanically derived suggestion
  instead.
- The most opinionated rule in the set; `--allow non-camel-case` is the
  escape hatch for other styles.

### `confusable-names`

Two definitions or bindings visible in the same scope whose names differ
only under a confusability normalization: lowercase, strip `_`, then map
`1 → l`, `i → l`, `0 → o`. Equal normal forms with unequal raw names
produce one finding per pair, reported at the later definition and naming
the earlier one. Deterministic, same-scope only.

- Message: `'sum_bits' is confusable with 'sumBits' (defined at line 4)`
- No fix.

## 5. Testing

- **Per-rule unit tests** in each rule's file (`#[cfg(test)] mod tests`),
  following the existing pattern: run `lint()` on inline source, assert
  over `report.diagnostics` (now by `code` instead of
  `message.contains`).
- **`lint_programs.rs`** integration suite in
  `crates/post-machine/tests/`: multi-rule programs, source ordering,
  `allow` filtering, fix application (idempotence: linting the fixed
  source reports no fixable findings; applied/skipped accounting).
- **CLI tests** in `cli_programs.rs`: exit 0/1, `--allow`, unknown-code
  rejection, `--fix` file round-trip, applicability gating (plain `--fix`
  leaves gated fixes in place and renders the `requires --force` hint;
  `--fix --force` applies them; `--force` without `--fix` errors), and
  the batch model (directory walk finds nested `.pmc` in sorted order;
  `--exclude` prunes a subtree and an explicit file; a dot-dir is
  skipped; a zero-match PATH errors; one bad file doesn't stop the
  batch and still fails the exit code).
- **Migration:** existing warning tests (`compiler.rs`, `ir.rs`,
  `compile_programs.rs`, `visibility_programs.rs`, `cli_programs.rs`)
  move to the `Diagnostic` shape; span assertions added where valuable.
  Golden/stdlib suites keep asserting empty diagnostics.
- **Dogfood:** the embedded stdlib must lint clean — a test runs `lint()`
  over `std.pmc` and asserts zero findings; `std.pmc` gets fixed first if
  findings appear.

## 6. Documentation

- New `docs/lint.md`: rule catalog — code, semantics, example, fix
  behavior, `--allow` usage. Ref-free prose per the published-docs policy.
- `docs/cli.md`: `lint` subcommand section (flags, exit codes, `--fix`
  semantics).
- `docs/language.md`: the header states the language version it
  describes (0.2) and defines the versioning scheme (0.N pre-stability;
  major/minor axes at 1.0); the warnings paragraph gains a pointer to
  the lint layer; the grammar prose documents the two tightenings (`@`
  tight to the function name; reserved words barred in every `::` path
  segment), states that spaced label colons and spaced paths remain
  legal, and records the 0.1 → 0.2 history.
- `README.md`: one-line mention of `pmt lint` in the CLI overview.
- `docs/language.md` accuracy fixes surfaced by the grammar audit: the
  duplicate-`use` exception (exactly-identical `use` lines are tolerated
  and demoted to an unused-import warning — the doc currently overstates
  "error"; doc aligns to code); the "every command takes an optional
  successor" overstatement (only builtins and `@`-calls do — the doc's
  own table is already right); legal-but-undocumented empty function
  bodies and stacked labels (`1: 2: left;`); comment edge cases (block
  comments don't nest, a lone `/` is an error, an unterminated block
  comment is a lex error); `goto`'s total exclusion from comma groups;
  `export main()` being a redundant no-op; and namespaced `main`
  (`namespace ns { main… }`) NOT being the program entry.

## Acceptance criteria

1. A committed lint fixture reproducing the unused-label shapes from the
   8-bit summing showcase (a label on a single-statement helper body; a
   label on a fall-through-only statement, e.g. the check after a call)
   yields exactly the expected findings; `pmt lint --fix --force` removes
   exactly the offending prefixes and a re-lint is clean, while plain
   `--fix` leaves the label prefixes in place (it applies only
   `MachineApplicable` fixes). (The showcase itself,
   `.superpowers/sum8/sum8.pmc`, is a local untracked artifact — tests
   cannot reference it; it serves as a manual smoke check only.)
2. `pmt compile` output — warnings text aside — is byte-identical to
   pre-change output at `-O0` and `-O1`; `-Werror` behavior unchanged.
3. All four migrated warnings carry real spans; `CompileError` carries a
   span at every construction site.
4. The stdlib lints clean.
5. `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D
   warnings`, `cargo fmt --check` all pass.
