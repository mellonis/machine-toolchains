# Standard-Library Write Contracts, Believed by the Inference — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A caller that reaches the standard library gets a truthful
write footprint: the inference takes an external callee's DECLARED
effective set (`writes` minus `preserves`) when the callee's resolved
signature is visible, and every standard-library export declares one —
the tracker issue "the standard library should tell the truth about
what it writes, and the footprint inference should believe it".

**Architecture:** the source-walk fixpoint (`footprint::infer_resolved`)
gains an external-modules parameter: on an edge whose callee is outside
the compilation unit it looks the callee up by full path in those
modules' resolved worlds and projects the callee's declared effective
sets through the binding exactly as it projects a local callee's
inferred sets (identity placement for a transparent call, symbol maps
for a bound one, resolved against the callee module's own alphabets);
a callee found nowhere keeps today's whole-alphabet answer. The
standard library's resolved module is the one external module every
consumer passes by default (checker, `dead-map-pair` lint, hover); the
library's OWN analysis passes none, which is also what breaks the
would-be cycle through the once-per-process stdlib cache. Objects at
the link boundary carry no contracts, so a `.tmo` library callee stays
conservative — documented, not changed. Then the library declares
`writes { … }` on every remaining export, exact to its own inferred
set, with a pin that no export ships without one.

**Tech Stack:** `footprint.rs` (`SymSet`, `binding_contribution`,
`edges_of`), `compiler.rs` (`ResolvedTape.writes/preserves`,
`check_contracts`, `CompileOptions`), `stdlib/mod.rs` (`analysis()`
cache), `std.tmc`, docs/tmt/language.md, docs/tmt/stdlib.md, docs/lsp.md.

**Spec:** the tracker issue's two parts; the contract-clause semantics
in docs/tmt/language.md (contract clauses) are the law the change
obeys — an inferred set stays a superset of what a run writes.

## Global Constraints

- Soundness: `inferred ⊇ actual` still holds. A declared effective set
  is a promise the checker enforced on the callee's own body, so
  believing it is exactly as sound as believing a local callee's
  inferred set.
- Compiled-stdlib byte identity: contracts are checks, not codegen —
  `stdlib::object()` is byte-identical before and after `std.tmc` gains
  clauses (the probe records the identity before the edit).
- The stdlib's own analysis never consults the stdlib cache (no
  re-entrant `OnceLock` init).
- Gates per task: workspace tests, clippy `-D warnings`, fmt, no_std.

---

### Task 1: the inference believes external declared contracts

- [x] RED (`footprint.rs` tests): a host calling
  `std::binaryNumbers::goToNumbersStart()` transparently — that routine
  declares `writes {}` — infers only the host's own writes under
  `infer_resolved_with(&resolved, &[stdlib::resolved()])`, and the
  whole alphabet under `infer_resolved_with(&resolved, &[])`.
- [x] RED (`compiler.rs` tests): the same host may declare
  `writes { '$' }` on that tape and compile; today it is
  `writes-outside-contract`.
- [x] Implement: `ExternalContracts { Stdlib, None }` on
  `CompileOptions` + `analyze_with`/`analyze_staged_with`;
  `stdlib::resolved()` (the cache keeps the whole `Resolved`);
  `Edge.external: Option<&str>`; `declared_effective(tape)` shared with
  `check_contracts`; `binding_contribution` takes the callee module's
  alphabets.
- [x] GREEN + `cargo test -p mtc-turing-machine`.

### Task 2: every standard-library export declares its set

- [x] Probe (ignored test) prints, per export lacking a clause, the
  exact inferred set as clause text; edit `std.tmc`; pin: every
  exported world declares `writes` on every signature tape; the object
  identity from the pre-edit probe is unchanged.
- [x] docs/tmt/stdlib.md: the roster paragraph and the Contract column.

### Task 3: words and gates

- [x] docs/tmt/language.md (contract clauses: external callees),
  docs/lsp.md (hover: the footprint believes the library's promises),
  `footprint.rs` module doc; a hover pin on a `setZero`-shaped fixture.
- [x] Full gates.
