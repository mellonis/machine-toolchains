# C2: a shared green-tree syntax framework and typed-view front ends

- **Date**: 2026-08-17
- **Status**: approved
- **Driving issue**: [#14](https://github.com/mellonis/machine-toolchains/issues/14) — CST: zero-copy typed-view AST (C2), deferred from the fmt/C1 decision
- **Supersedes at cutover**: the C1 hand-typed CSTs (`crates/post-machine/src/cst.rs`, `crates/turing-machine/src/cst.rs`), the AST containers (`Program`/`Function`/`Import`/`Statement` in PM, `Program` and its container family in TM), and both `lower_cst` passes

## 1. Context

The C1 decision (the fmt/lossless-CST round) gave each front end a hand-typed
lossless CST that `lower_cst` copies into the owned AST the compiler, lint,
and LSP consume. That bought a parity-provable parser refactor at the cost of
two trees per crate. #14 recorded the rust-analyzer-style alternative — one
tree, typed views — as deferred.

A 2026-08-17 code audit corrected #14's original scope claims before this
design was made (the issue body now carries the corrected version):

- **The optimizer and codegen sit behind the IR firewall.** Their only
  `parser::` imports are `#[cfg(test)]` fixture builders. Nothing below
  `ir.rs` changes in this arc.
- **The two trees already share their statement/rule internals.** PM's
  `StatementCst` embeds `parser::Item`/`Label` verbatim; TM's `RuleCst`
  embeds `parser::Rule` verbatim. The duplication is container-level.
- **`lower_cst` is ~100 lines per crate**; its real work is ns-path
  stamping, nested-function hoisting (PM) / world-body splitting (TM),
  per-path `use` flattening, and doc-run reduction. The rest is `clone()`.
- **The AST types serve two roles.** Besides being the parse output, they
  are the *synthesized intermediate*: PM `flatten` hands a mangled/resolved
  program to `ir::lower(&Program)`; TM's compiler produces
  `analysis.resolved` for `expand`. Views can only represent parsed source,
  never a pass's computed output — so an owned compiler-side representation
  survives this migration *by design*, not as a compromise.

Two approaches were weighed for the tree itself: a view/accessor layer over
the existing hand-typed CSTs, or the full rowan model — a homogeneous green
tree with generated typed views. **The full rowan model was chosen** (ruling,
2026-08-17), because taken seriously it is *shared infrastructure*: both
hand-typed CSTs and every future per-grammar-change CST edit collapse into
one core-owned framework; trivia becomes uniform tree data, making the
comment-misattachment defect class (the shape behind the fmt list-comment
bug fixed in the pre-v0.3.0 round) structurally unrepresentable; and the LSP
gets position-first syntax access, the thing red trees exist for.

A second ruling settles the provenance of the machinery: **hand-rolled in
`mtc-core`** — no dependency on the `rowan` crate, no vendored fork. The
workspace's minimal-deps stance holds (`serde`/`serde_json` only, `proptest`
as dev-dep), and core already hosts exactly this kind of language-agnostic
framework (asm, LSP). The implementation is sized to what `.pmc`/`.tmc`
need, not to all of rowan.

Delivery ruling: **one branch, both crates** — a single feature branch
carrying the core framework plus the PM and TM migrations, merged once.
Every commit on the branch keeps the full gate set green (§6).

## 2. Goals and non-goals

**Goals**

- One lossless syntax tree per parsed file; consumers read it through typed
  views. No parse-side copy, no `lower_cst`, no possibility of the "two
  trees disagree" failure class.
- The tree machinery is language-agnostic and core-owned; the two front
  ends declare only their kind spaces, views, and parsers.
- Behavior-neutral: no user-visible change of any kind. Diagnostics,
  spans, fmt output, LSP behavior, compiled artifacts — all byte/struct
  identical. The next release's version block declares every language,
  dialect, IR, and container space unchanged.

**Non-goals (filed as follow-ups, not built)**

- **Error-resilient parsing.** The fatal-error model is kept: on
  `CompileError`, no tree. Error nodes in the tree change diagnostic
  behavior and would break the behavior-neutral gate. Follow-up issue.
- **Migrating core's asm CST** (`asm/{lexer,cst,lower}.rs`) onto the
  framework. Candidate follow-up; out of scope here.
- **Incremental reparsing.** The green tree permits it; nothing needs it.
- Any optimizer, IR, linker, VM, DAP, or container work.

## 3. The core syntax framework (`crates/core/src/syntax/`)

Language-agnostic by the same contract as the asm and LSP frameworks: zero
`.pmc`/`.tmc` knowledge, tested against a crate-private fake kind space.
Gated behind core's std feature so `cargo build -p mtc-core
--no-default-features` (the no_std vm gate) is untouched.

### 3.1 Green layer

Immutable, structure-sharing value tree:

- `SyntaxKind(u16)` — an opaque newtype. Each language crate owns its kind
  space the way an arch owns its opcodes; core never interprets a kind
  beyond equality. Debug rendering goes through a caller-supplied
  kind-to-name function (`debug_dump`); a richer per-language trait is
  deferred until a front end needs it.
- `GreenToken { kind, text }` — owns its text. Whitespace and comments are
  ordinary tokens; there is no side-channel trivia storage anywhere.
- `GreenNode { kind, children, text_len }` — children are nodes or tokens;
  `text_len` is cached for O(depth) offset computation.

**The lossless contract is one law**: `tree.text() == source`, byte for
byte. It replaces C1's per-field trivia bookkeeping (`blank_before`,
`open_trailing`, `interior`, trailing-comment columns, …) wholesale — those
distinctions become *derivations over tokens* (§5.1), not stored fields.

### 3.2 Red layer

`SyntaxNode` cursors: parent pointer + absolute text offset, computed on
demand from the green tree. Every node and token knows its byte range.

Diagnostics keep today's line/col `Span` as the currency. A `TextLineIndex`
(built once per file from the source text) converts byte offsets to `Span`.
**Span parity is an explicit test surface**: pinned spans throughout the
existing suites must come out identical through the new path (§6.1d).

### 3.3 Builder

`TreeBuilder` with `start_node` / `checkpoint` / `start_node_at` /
`finish_node` / `token` — the standard shape a recursive-descent parser
emits through. Checkpoints cover the left-recursion-ish spots (e.g. wrapping
an already-emitted prefix once the parser knows the node kind).

### 3.4 Typed views

- `trait AstNode { fn cast(SyntaxNode) -> Option<Self>; fn syntax(&self) ->
  &SyntaxNode; }` in core.
- A small declarative macro (core-owned) to declare view structs with
  `cast` + child/token accessors, so per-language view modules stay
  boilerplate-free without proc-macro machinery.
- Concrete views live per language crate, never in core.

### 3.5 Core testing

- Property tests (proptest, fake kind space): `tree.text()` round-trip over
  arbitrary token sequences; builder/cursor navigation laws (parent/child
  inverses, offset monotonicity, range nesting); `LineIndex` offset↔line/col
  round-trips including multi-byte UTF-8 and final-line-without-newline.
- Zero-language-knowledge is proven the same way the VM core proves it:
  everything in core's own tests uses the fake kind space only.

## 4. Per-language adoption

PM first on the branch, then TM; identical shape.

### 4.1 Kind space and lexer

Each crate declares its `SyntaxKind` enum (`#[repr(u16)]`, converted at the
core boundary) covering every token kind and every node kind. The
**existing lexers are unchanged**: a per-crate layout pass
reconstructs each token's verbatim text and the whitespace gaps
between tokens from the source (token start positions are exact;
ends are derived per kind and validated by a
concatenation-equals-source invariant), and green emission is woven
into the existing parser behind an optional sink — same grammar walk,
same errors, with the C1-CST-building half deleted at cutover. The comment-free lex mode dies at that crate's
cutover: one parser, one mode; the compiler simply never looks at trivia
through views. Behavior-neutral — the compiler ignores what it previously
never received.

### 4.2 Parser

`parse_cst` is reimplemented as the same recursive-descent logic emitting
green nodes through `TreeBuilder`. Same errors, same spans, same fatal
model. The parse-time attachment pass for `?`/`!` doc runs stays in the
parser (a run bound to nothing is still a `DanglingDocRun` *error* at parse
time); the run itself is just tokens/nodes in the tree, and views expose
the bound run per declaration.

### 4.3 Views

Typed views replace the C1 CST structs: PM `FunctionNode`,
`NamespaceNode`, `UseNode`, `StatementNode`, item views; TM
`MachineNode`/world views, `RuleNode`, `AlphabetNode`, graft/bind views,
signature views. **The C1 field documentation — the real investment in
`cst.rs` — migrates onto the view accessors**, updated to describe
derivation from tree structure rather than stored fields.

Derived accessors (computed, never stored): `blank_before` (whitespace
token containing ≥2 newlines before the item), trailing comments (same
line, after the node's last non-trivia token, via `LineIndex`),
`label_break`, ns paths (walk ancestors), hoisted-function iteration,
reduced doc runs (`FnDoc` stays as a value type built by a view method),
per-path `use` flattening.

### 4.4 The owned compiler vocabulary

PM `flatten` and TM flatten/checks/resolution consume views and build their
owned input — `Item`, `Rule`, `Label`, the leaf enums (`Builtin`,
`Successor`, `CheckArm`, `SymLit`, `BindingArg`, `MoveDir`, …) survive as
the *lowered vocabulary* those passes construct. This is HIR lowering: the
legitimate owned tree. What dies is the parse-side copy. The leaf types
move out of `parser.rs` into a new `hir.rs` per crate — the compiler-owned
lowered vocabulary, which also hosts the flatten-output containers
(`parser.rs` itself shrinks to the green-emitting parser). `ir.rs`
downward is untouched, including its serialized shape.

## 5. Consumer migration

### 5.1 fmt

Both formatters are rebuilt to walk the green tree directly. Every
classification C1 stored a field for is re-derived from tokens: leading /
trailing / standalone / dangling / c-brace comments, `blank_before`,
`label_break`, trailing-`//` source-column alignment runs. Same decisions,
new source of truth. Interior list comments are ordinary tokens between
entry nodes, so per-entry trivia is automatic — the list-comment
re-attachment defect class is structurally unrepresentable.

Gates: **byte-identical output over every existing fmt fixture**,
idempotence, the whitespace-only contract, and the byte-identical compiled
stdlib at both opt levels (the property that proved the `.tmc` formatter
text-only).

### 5.2 Lint and LSP

- Lint rules (PM 5, TM 11 + `patterns.rs`) walk views. `LintContext`
  carries the tree instead of the AST; the shared allow namespace, rule
  ids, findings, spans, and quickfixes are unchanged observables.
- Both LSP services move onto views; ranges come from red-tree offsets
  through core's existing position mapping. The `.pma`/`.tma` services are
  untouched (asm CST out of scope). CLI≡editor parity stays structural —
  one lint entry feeds both.

### 5.3 Compiler front ends

PM `compiler.rs` checks/flatten and TM `compiler.rs` flatten/checks/
resolution take views as input (§4.4). `expand.rs`, `footprint.rs`
consume the owned vocabulary exactly as today. `analyze_staged`'s break
points keep their meaning; its parse stage yields the tree.

## 6. Oracles and gates

### 6.1 Differential oracles (per crate, alive until that crate's cutover)

a. `tree.text() == source` over the full corpus: every `.pmc`/`.tmc` test
   fixture, the examples, and the embedded stdlibs.
b. View-extraction output **struct-equal** to `lower_cst(parse_cst(src))`
   over the same corpus — the old path is the oracle for ns stamping,
   hoisting, import flattening, and doc reduction.
c. New fmt byte-equal to old fmt over all fixtures.
d. Span parity on pinned diagnostics: the existing suites' expected spans,
   unmodified, passing through the new path.

The cutover commit per crate deletes the C1 CST, the AST containers,
`lower_cst`, the comment-free lex mode, and oracles (b)/(c) together —
nothing imports them by then. Law (a) stays forever as a regression
property.

### 6.2 Standing gates, every commit on the branch

Full suite green; `-O0` bit-identity; PM-1 byte-identity; byte-identical
compiled stdlibs; `cargo clippy --workspace --all-targets -- -D warnings`;
`cargo fmt --check`; the no_std vm gate.

## 7. Sequencing (one branch)

1. Core `syntax/` framework + fake-kind tests.
2. PM: green parser (oracle a) → views (oracle b) → compiler front on
   views → lint → LSP → fmt (oracle c) → PM cutover.
3. TM: same order → TM cutover.
4. Docs pass (§7.1) and follow-up issues filed (§2 non-goals).

Every step is a reviewable commit (or small run of commits) with the full
gate set green; the branch merges once at the end per the delivery ruling.

### 7.1 Documentation pass

Published pages (forge-agnostic, ref-free per the published-docs policy):

- **`docs/core.md`** — a new syntax-framework section: green/red layers,
  the `tree.text() == source` law, `SyntaxKind` ownership, `TreeBuilder`,
  the `AstNode` view contract, `LineIndex`/`Span` conversion. This is the
  durable page new code cites (`docs/core.md (syntax tree)`); it lands in
  the same commits as the framework so citations never dangle.
- **Per-toolchain pages** (`docs/pmt/`, `docs/tmt/` — language, fmt, lint,
  cli, lsp) — audited, expected unchanged: they document behavior, and
  behavior is gate-pinned identical. Any page found describing the C1
  CST/AST internals is corrected in the same commit that changes them.
- **`README.md`** — audited the same way; expected unchanged.
- **`CHANGELOG.md`** — deliberately NOT touched this arc. The entry rides
  the next v-bump PR (0.5.0) per the release rule, describing the internal
  architecture change in ref-free prose under an all-`unchanged` version
  block.

Internal artifacts:

- **CLAUDE.md** — architecture section rewritten at merge: core gains the
  syntax framework; the pipeline description replaces
  `parse = lower_cst ∘ parse_cst` and the CST/AST pair with green tree +
  views + `hir.rs`; the crate boundary notes updated.
- **Code-comment citations** — the C1 `cst.rs` doc comments cite
  `docs/pmt/fmt.md` / `docs/pmt/language.md` by topic keyword; those
  citations migrate with the prose onto the view accessors (§4.3). New
  core code cites `docs/core.md (syntax tree)`.

Closing audit, house style: every claim the new/changed pages make is
re-verified against the built tools before merge.

## 8. Versioning and release posture

No release rides this arc. The work lands on master and ships with the
next cut (crates 0.5.0), whose version block declares `.pmc`, `.pma`,
`.tmc`, `.tma`, both IRs, all containers, and both manifest schemas
**unchanged** — that line is the arc's proof of internal-only impact.
