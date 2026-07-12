# .pmc doc lines (?) and attention lines (!) — design

Date: 2026-07-12
Status: approved 2026-07-12 (brainstorm walked question-by-question, sections approved)
Tracker: [machine-toolchains#17](https://github.com/mellonis/machine-toolchains/issues/17); folds in [#25](https://github.com/mellonis/machine-toolchains/issues/25) (completion detail)

## Context

`.pmc` functions have no in-source documentation, so the LSP has nothing
to show for hover (parked as a follow-up in the LSP design), the
embedded stdlib cannot describe its own routines, and there is no way to
mark a function deprecated. The tracker proposal (from the LSP spec
review) sketches `?` doc lines and `!` attention lines attaching to the
following function declaration. This design makes them real grammar —
the fmt/lint architecture's invariant that comments are pure trivia,
never semantic, is preserved by NOT using doc-comments.

Sequencing: this round lands before the first release cut so the
CHANGELOG carries the resulting language version in one cut.

## Decisions (settled during brainstorming, 2026-07-12)

| Decision | Choice |
|---|---|
| Representation | Real grammar, `PMC_LANG_VERSION` 0.2 → 0.3 (doc-comments rejected: they would make comment trivia semantic) |
| `!` semantics | Attention lines: `! [attr] optional prose` and `! bare prose` both legal; machine-readable attributes are a subset; the statement-position `!` is unambiguous (no statement starts with `!`; successor `!` lives inside parens) |
| Run shape | A run is at most TWO contiguous blocks in grammar-fixed order: `?` block first, then `!` block. Interleaving (`?!?`) is a parse error; wrong order is a parse error. fmt never reorders |
| Indentation | The sigil is recognized as the first non-whitespace character of a line at item position — runs sit at the bound declaration's own indent (nested subroutines included) |
| Attachment | The run binds to the NEXT function declaration; blank lines and ordinary comments may appear within/after the run; a dangling run (next item not a function declaration, or end of scope) is a compile error, span on the run's first line |
| Documentable set | Functions only, including nested. Namespaces (`use` too) deferred — a run before them stays an error until a later round legalizes it |
| Doc text | Consecutive `?` lines join in order — within a paragraph, lines soft-wrap joined by a single space; an EMPTY `?` line is a paragraph break; content is plain prose (no markdown interpretation in v1 — a markdown subset can be declared later without a grammar change) |
| Attribute vocabulary | v1 = `[deprecated]` exactly; the rest of the `!` line after the attr is its message. Unknown `[attr]` = compile error (the pmt.json posture). Bare-prose `!` lines carry no attribute |
| Deprecation channel | New lint rule `deprecated-call` (suppressible via the shared allow union; report-only). Plain `pmt compile` does not report it — lint and the LSP do |
| Carriage | Same-file + stdlib (the embedded `std.pmc` is analyzed from source in-process). Docs in `.pmo` object metadata (cross-library carriage) deferred — no container format moves |
| Data flow | CST carries the runs (lossless, fmt-visible); `flatten` — which owns name qualification — builds `Analysis.docs: Map<qualified name, DocInfo { paragraphs, deprecated: Option<message> }>`; every consumer (lint rule, hover, completion tags, stdlib) reads that one map. CST-only per-consumer extraction rejected (re-creates the duplicated-walk disease the walk-dedup tracker item complains about) |
| #25 folded in | The `Candidate` reshape for deprecation tags is the same struct/wire touch #25 needs, so completion `detail` ships in this round |

## Grammar (`PMC_LANG_VERSION` 0.3)

At item position (top level or inside a function body), a line whose
first non-whitespace character is `?` is a **doc line**; `!` is an
**attention line**. Both consume raw text to end of line.

- Attention line shape: `! [ident] rest-of-line` or `! rest-of-line`.
  The optional leading `[ident]` is an attribute; v1 accepts exactly
  `deprecated` — any other identifier is a compile error with the
  attribute's span. The rest of the line is the attribute's message
  (or free prose when no attribute).
- A **run** = one optional contiguous `?` block followed by one optional
  contiguous `!` block (at least one line total). A `!` line followed by
  a `?` line within a run, or any interleave, is a parse error
  ("doc lines come before attention lines").
- The run binds to the next function declaration at the same scope.
  Blank lines and ordinary `//`-comments may appear between run lines
  and between the run and the declaration without breaking attachment.
  A run not followed by a function declaration is a compile error
  ("dangling doc/attention lines"), span = the run's first line.
- Multiple `[deprecated]` attributes on one function: compile error
  (duplicate attribute).

Lexer: new spanned tokens `DocLine(text)` / `AttentionLine(text)`
(verbatim text after the sigil, leading space not required but
canonical). CST: `FunctionCst` gains a `doc_run` field holding the
ordered lines with spans and raw text (lossless; trivia rules for
interspersed comments follow the existing CST comment machinery).
`lower_cst` carries the run onto the AST function as one additive
optional field (paragraphs + deprecation, spans dropped) that every
compiler pass ignores; `flatten` — which owns qualification — copies it
into the `Analysis.docs` map. IR (`IR_VERSION` unchanged), optimizer,
codegen, containers: untouched; `-O0` bit-identity unaffected.

## fmt

- Run lines print at the bound declaration's indent (col 0 top level,
  body indent for nested), one space after the sigil; an empty doc line
  prints as a bare `?`.
- Prose is verbatim token text — zero-token-changes applies to it.
- No wrapping (line width is `line-too-long`'s business).
- Blank lines inside/around runs follow the existing collapse policy.
- New corpus fixtures: documented top-level + nested functions,
  paragraph breaks, deprecated-with-message; idempotence and the
  spelling-strength token guard cover them.

## Lint

New rule `deprecated-call` (registry order: appended last): fires on a
call item whose resolved target's qualified name is deprecated in
`Analysis.docs`. Message: `call to deprecated function 'NAME'` with
`: MESSAGE` appended when the attribute carried one. Report-only (no
fix). Joins the shared allow namespace (union validation already covers
new codes mechanically). Documented in `docs/lint.md`.

## LSP

- **Hover** — first new capability since the LSP round: core framework
  gains `hoverProvider: true`, the `textDocument/hover` handler, and
  `LanguageService::hover(uri, pos) -> Option<HoverContent>` (no default
  impl; the `.pma` service answers `None` in this round). The `.pmc`
  service answers on call sites, `use` path segments resolving to a
  documented function, and declaration names: plain-text paragraphs
  plus a `deprecated: MESSAGE` callout line when applicable.
- **Tags** — `deprecated-call` diagnostics carry
  `DiagnosticTag.Deprecated` (client renders strikethrough);
  completion candidates resolving to deprecated functions carry
  `CompletionItemTag.Deprecated`. Protocol types gain the tag fields
  (additive).
- **`Candidate` reshape (the #25 fold-in)** — `Candidate` gains
  `detail: Option<String>` and `deprecated: bool`, mapped to the wire's
  `detail` / `tags`. The `.pma` service fills `detail` with operand
  hints derived from `OperandKind`/`Flow` (`jm <label>`,
  `wr <indices>`, `call <function>`); the `.pmc` service fills
  `detail` where it has something real to say (v1: the qualified name
  for cross-namespace candidates; nothing invented) and `deprecated`
  from the doc map.
- Config, routing, the mux: untouched.

## Stdlib

`crates/post-machine/src/stdlib/std.pmc` gains `?` docs for all 11
exported routines (and `!` lines only where genuinely warranted — none
expected). Hover on `std::` calls works through the same analyze path
that already serves the roster and go-to-definition materialization.
The stdlib compiles at 0.3 by construction; its docs are the
self-documentation deliverable and double as the hover dogfood.

## Versioning and docs impact

- `PMC_LANG_VERSION` `"0.2"` → `"0.3"`; surfaced as today (constant,
  `docs/language.md` header, `pmt --version`).
- `.pma` dialect (0.2), `IR_VERSION`, MO/MX/MT: **unchanged** — stated
  in the release-notes version block when the release cut happens.
- `docs/language.md`: grammar section for `?`/`!` lines + version
  history entry. `docs/lint.md`: `deprecated-call`. `docs/lsp.md`:
  hover + tags + candidate detail. `docs/fmt.md` (or the fmt section's
  home): run formatting rules. Editor READMEs: hover checklist items.
  All ref-free per the published-docs policy.

## Testing

- Grammar acceptance/rejection table: legal runs (docs only, attention
  only, both, nested at indent, blanks/comments interleaved), rejects
  (interleaved `?!?`, `!`-before-`?`, dangling run at each position,
  unknown attribute, duplicate `[deprecated]`).
- `flatten` doc-map: qualified names for top-level, nested
  (dot-mangled), and namespaced functions; paragraphs join; deprecation
  message capture.
- fmt: fixtures + idempotence + token-spelling guard over the new
  shapes.
- Lint: `deprecated-call` fires on direct and namespaced calls; allow
  suppression; no finding for the declaring function itself.
- LSP: hover e2e (call site + declaration + std:: routine), both tags
  on the wire, pma candidates carry operand-hint detail.
- Acceptance guard: `PMC_LANG_VERSION == "0.3"` pinned; `-O0`
  bit-identity suite untouched and green.

## Out of scope

Namespace/`use` documentation (runs before them stay errors), markdown
doc rendering, `.pmo`/link-time doc carriage, signature help (`.pmc`
functions are nullary — nothing to sign), doc coverage lint rules, and
any `.pma`-side doc syntax.
