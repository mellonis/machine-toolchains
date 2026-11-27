# Language tightening — one definition per name, stricter checks, three lints, an honest graph digest

Status: APPROVED by the maintainer 2026-09-30 (§2.5 added after the hygiene review, with no objection). Destination: `docs/internal/specs/2026-09-30-language-tightening-design.md`.
Phase 4 of the binding arc, language spec A of three (B: rule guards and named vector cells; C: a cell-preserving `preserves`). Tracker: #138 (the one-definition rule), #127, #125. Decisions below were taken with the maintainer on 2026-09-30; the brainstorm log is in the phase-4 ledger.

## Goal

Remove the places where the toolchain silently picks one of two things, silently accepts something it documents as an error, or reports a lint the author cannot act on. No new syntax. Every change is an error, a lint, or a digest that tells the truth.

## What is in, and what is not

In: (1) one definition per fully qualified name, in both toolchains; (2) four compiler/assembler/linker checks; (3) three lints and one lint exemption; (4) a graph digest that covers set and map content; (5) two documentation statements.

Not in: a duplicate diagnostic in the editor's cross-file overlay (the editor spec — the overlay must end up showing what the linker shows); overloading and an explicit `override` (reserved directions, documented as such, no keyword reserved); rule guards and named vector cells (spec B); `preserves` (spec C).

## 1. One definition per fully qualified name

**The rule.** A fully qualified name has one definition in a link and one declaration in a compile. Namespaces stay open: adding a new name under `std::` is legal; defining an existing one a second time is not.

**Why a ban rather than first-wins.** It is the only choice compatible with overloading, should the languages get it: an error can later be relaxed to "allowed when the signatures differ" without changing the meaning of an existing program; a silent first-match cannot. Test stubs (`pmt test` / `tmt test`) will want replacement; that gets an explicit spelling later instead of implicit shadowing.

### 1.1 Link (the shared core: `pmt`, `tmt`, hand-written assembly)

| pair | today | after |
|---|---|---|
| user object vs user object, a code symbol | `duplicate symbol` | unchanged (always, reached or not) |
| user object vs library, a code symbol | the user definition silently shadows | an error when the name is reached |
| library vs library, a code symbol | the first silently wins | an error when the name is reached |
| a graph or an alphabet exported by two inputs (interface records) | first wins | identical records are one export; different records are an error when some input consumes the name (a recorded graft of the graph, a recorded import of the alphabet) — and always between two user objects |

"Reached" is the linker's existing reachability from the entry: a name is contested only if a reference that ends up in the program resolves to it. Two unrelated libraries that share an unused name still link together. Inside ONE library object the existing lenient first-record-wins stays (compiler output cannot produce it).

The error names the symbol and both defining inputs. The escape for replacing the standard library is the existing one: `--nostdlib` and one's own set.

The two-column rule (a name offered as normal and volatile blobs) is untouched: a definition is one input's column pair; a second input offering the same name is a duplicate whatever its columns.

Core stays arch-neutral: the rule is tested against the fake architecture. PM-1 output must not move (goldens, `asm_volatile`).

### 1.2 Compile (`.tmc`, the declarations table)

Two declaration modules that export one name:

- **identical published forms** are one declaration seen twice — a unit's header and its source, one library by two paths — and merge silently;
- **different forms** are a new compile error, `conflicting-declaration`, naming the name and both origins.

Published form, per kind: a routine's signature with its contract clauses and `noreturn`; a graph's digest (§4); an alphabet's glyph sequence; a map's pairs; a set's members. Doc lines and comments are not part of it. Table order no longer decides anything.

The same rule holds between the unit's OWN definition and a declaration of that name in the table: a unit that defines `std::binaryNumbers::goToNumber` with the standard library on is refused; a unit compiled beside its own up-to-date header is fine; against its own stale header it is refused (wanted: it is a stale header caught at the first compile; manifest-mode `tmt build` reads sibling sources, so it cannot arise there).

A guard the plan must include: for every unit the repository ships, the declarations read from its source and from its generated header are identical under this comparison — otherwise the rule would refuse honest builds.

### 1.3 `.pmc`

No cross-unit declarations exist; the rule is link-only. The grammar does not move.

### 1.4 Baseline (probed 2026-09-30, before any change)

A unit defining `std::binaryNumbers::goToNumber` itself, standard library on, compiles and links silently. One unit given twice as declarations compiles silently (must stay legal). Two user objects exporting one routine are already refused. An `--extern` re-declaring a `std::` graph with the same signature and another body compiles, splices the other body, and is refused at link by graft drift unless the user supplies the shadowing object. Core tests that pin today's shadowing and will be rewritten: `resolve.rs::libraries_resolve_lazily_and_users_shadow`, `duplicate_user_symbols_error_but_library_shadowing_does_not`, `a_shadowing_normal_only_pair_forces_a_counted_fallback`.

## 2. Checks

**2.1 `duplicate-pattern-binding` (#127).** A binding name repeated within one rule's pattern (`[0..7 as x, 0..7 as x]`) is a compile error naming the name, both cells and the rule. Today it compiles and the rightmost cell wins in a write. Equality of two cells is what a rule guard will express (spec B); this error keeps the spelling free for it.

**2.2 `return` handed to a `state` parameter outside a routine.** The language already defines `return-outside-routine` for "a `return` transition, continuation, or `state` argument outside a routine body". A graft's state argument escapes it: `graft g(…, done = return)` in a `machine` block compiles with only an `unreachable-state` warning, for a local and a library graph alike. The implementation is brought to the documented rule; the stray warning is root-caused, not suppressed.

**2.3 One argument check across units.** A graft of a declared graph already goes through the shared argument check. A `call`/`bind` into a declared routine still uses a separate copy in the IR lowering. It moves to the shared check, against the declared signature (its parameter order is recorded). Visible changes: a terminator given to a tape parameter (`call r(a = stop, …)`) reports `wrong-arg-kind`, as a local call does (today `missing-arg`); a `missing-arg` on a `bind` points at the bind. A call with NO declarations for its callee is unchanged: the check is deferred to the linker.

**2.4 `undeclared-exit` becomes an error, at assembly and at link.** Only hand-written `.tma` can produce it: a routine whose `.routine` line declares `exits=N` and whose body executes `retx #k` with `k ≥ N`. Today it is a link warning; mono and hybrid refuse only where they copy the body, and otherwise the program links and traps when that branch runs. After: the assembler refuses the file (the contradiction is inside one routine), and the linker refuses an object that carries it, under every mechanism. The code keeps its name, leaves the link-warning table, and `--allow` no longer applies to it. The assembler reuses the linker's arch-neutral predicate rather than learning the opcode.

**2.5 A name of the wrong kind across units says what it is.** When a name resolves, in the declarations given to the compile, to a declaration of another kind, the message must say so rather than claim "its declarations were not given". That is already true where a set is expected and for a local name used as a tape type; it is still false where an ALPHABET is expected across units (`tape t: lib::someset` with the library's declarations given) and where a MAP is expected. Both are brought in line, and the lookup that answers "what kind is this external name" becomes one function shared with the existing per-kind lookups, so the two cannot drift. Error codes do not change (a cross-unit wrong-kind reference keeps the code of the kind that was expected; whether it should be `wrong-target-kind`, as in the local case, is left open deliberately — codes are a contract).

## 3. Lints

**3.1 `unused-tape-parameter` and `unused-fold-variable` (#125).** A routine's or graph's tape parameter that the body never touches (the definition `unused-tape` already uses); a pattern binding (`as x`) that nothing reads — a read inside a future rule guard counts. Both default-on, neither offers a fix (removing a parameter changes a signature; a binding may be positional). `unused-tape-parameter` fires regardless of `export`: a parameter cannot be used from elsewhere. Both must be silent over the examples, the goldens and the standard library, or the corpus is corrected first.

**3.2 `inert-never-writes`.** With both clauses written, a glyph named in `never writes` that `writes` does not name changes nothing. Default-on, source only: a header publishes each tape's effective `writes` set as one positive clause and never prints `never writes`.

**3.3 Exported declarations are never "unused".** `unused-alphabet`, `unused-map` and `unused-set` skip a declaration marked `export`: another unit may import it through the header, and the deletion fix would remove something a consumer needs. `unused-tape`, `unused-graft-name` and `unused-exit` do not change — nothing there is exportable. This reverses the earlier "export-independent by design" statement for these three rules; the lint page and the repository's standing notes are rewritten to say so.

## 4. The graph digest covers content

A graph's digest is the CRC of its body as the header printer renders it. Where the body names a glyph set in a cell, or a named map at a nested graft, the digest covers the NAME only; changing the set's members or the map's pairs leaves the digest alone and the link raises no graft drift. The language page documents this as a limit.

After: the digested text is the body followed by the canonical rendering of every set and every named map the body names — sets transitively, each once, in a fixed order (by qualified name). Both sides compute it the same way: the exporter from its source, the consumer from the declarations it read. A change to a member or a pair moves the digest and the link reports drift. The documented limit is deleted.

Every graph that names a set or a named map gets a new digest. The object and header formats are unreleased revisions, so nothing is migrated; pinned digests regenerate.

## 5. Documentation statements

On the language page, beside the head contracts: `enters` and `leaves` constrain the GLYPH under the head at entry and exit, not the head's position; a routine that returns one cell short of where it entered satisfies them. And the idiom that does state "returns home" today: a marker glyph at the home cell and `leaves { marker }`.

## Amendments from the code survey (2026-09-30, after approval)

Reading the code for the plan turned up facts that sharpen the sections above. None changes what was decided; each is recorded so the plan and the spec agree.

- **§1.1 — naming both inputs.** The duplicate error carries only a bare name today and name resolution receives no input names; `pmt link` and `tmt link` pass none, and the build drivers pass them for user objects only. Input names are threaded through for user objects, libraries and the embedded standard library, so the error can say "defined by A and by B".
- **§1.1 — a documented feature goes away.** "Overriding std" (a user definition replacing one standard-library routine) is a documented feature on both standard-library pages, pinned by two PM-1 tests. It is removed; the pages say instead that the library is replaced wholesale with `--nostdlib`. The editor overlay's faithfulness tests, which compare the overlay against the linker on a sibling-versus-library collision, are updated with the rule (the overlay's own diagnostic stays in the editor spec).
- **§1.2 — reading a source must publish what its header publishes.** Today a `.tmc` read for its declarations leaves an uncontracted tape's `writes` unset (meaning the whole alphabet) while the generated header prints the inferred set, so a caller with a `writes` contract compiles against the header and is refused against the source. The source read publishes the inferred set, exactly as the header does. Without this the source-versus-header guard fails on every uncontracted routine; with it, `tmt build` (which reads sibling sources) and the `--extern` header route agree.
- **§1.2 — where the conflict is reported.** A conflict between two declaration modules has no position in the unit being compiled; it is reported against the compile as a whole, naming both origins. A conflict with the unit's own definition is reported at that definition.
- **§2.1 — the code.** `duplicate-binding` already exists and means "two `use` imports bind one bare name". The pattern-cell error gets its own code, `duplicate-pattern-binding`.
- **§2.2 — the escape is wider than one shape.** The argument check accepts a terminator for a `state` parameter without knowing the host. Every state argument is covered: a graft in a `machine` block, a graft inside a GRAPH body (a graph leaves through its own state parameters), and a `bind` in a machine even when nothing uses it.
- **§2.3 — a bypass closes.** When every argument of a call into a declared routine is a terminator (`call q(a = stop, b = halt)`), today's check returns early and the call compiles. Under the shared check it is `wrong-arg-kind`.
- **§2.5 — a third site.** Grafting a declared ROUTINE reports `undefined-graph` with "its declarations were not given"; it joins the two sites already listed.
- **§2.4 — consequences.** The link-time scan runs unconditionally (today it is skipped when the entry function has no signature). `--allow undeclared-exit`, and a manifest `lint.allow` naming it, become "unknown code" errors — a breaking note for the CHANGELOG. Tests that build such a body through the assembler construct the object directly instead.
- **§3.1 — a false positive in the existing rule.** `unused-tape` counts a tape as forwarded only when an argument names it. A transparent call, graft or bind with no argument list binds tapes by position, so a machine whose only rule is `call std::binaryNumbers::plusOne()` is reported as never touching its tape. Both `unused-tape` and the new `unused-tape-parameter` treat an argument-less call, graft or bind into another unit as touching every tape. `.tmc` has no repeat variable (that is an assembly construct), so "fold variable" means a pattern binding and nothing else.
- **§3.3 — the flag.** Sets and maps already carry their `export` flag in resolved form; alphabets gain it.
- **§1.1 / §4 — exported and private graph records.** A graph record in an object says whether the graph is exported (a flag byte in the graph entry; `.graph NAME, DIGEST, private` in assembly; the container stays at its unreleased revision). Only EXPORTED records take part in the export-conflict check: a private graph is not a definition in the shared namespace, exactly as a local symbol is neither exported nor shadowable, so two sibling units each holding a private graph of one name still link. Private records serve the drift check only: a consumer's recorded graft of a graph no input exports must match the record of at least one input that prints it; a graph no input records stays unchecked, as today.
- **§4 — nested library grafts are recorded.** Only a directly grafted library graph is recorded in the consumer's object today; a library graph that reaches the consumer through another library graph's body is not, so a change inside it escapes the drift check. The consumer records every library graph a splice pulled in, nested ones included, each with the digest of the body spliced. No digest changes.
- **§2.5 — a fourth site.** A `call` or `bind` that names a declared GRAPH resolves through a lookup with no kind filter. It reports `wrong-target-kind`, the local case's code, naming what the declared name is.
- **§1.2 — state-parameter names.** They are part of a routine's published form where both readings carry real names; a reading taken from an object carries none (the object records only the count), and the comparison falls back to the count there.
- **§4 — the standard library does not move.** No standard-library graph names a set or a named map, so its pinned digests and its generated header stay byte-identical; nothing is appended to the digested text of a body that names none.

## Versions

Nothing moves on account of this spec beyond the revisions the arc has already opened and not released: `.tmc` 0.2, the `.tma` dialect 0.5, the object container 4. `.pmc` stays 0.4 — a link behaviour is not grammar. Versions are raised once, at the release cut, over the whole scope, minimally. The CHANGELOG at the cut carries the breaking notes: a library definition can no longer be shadowed (both toolchains); `duplicate-pattern-binding`; `undeclared-exit` refused at assembly and link; stricter argument kinds across units; graph digests move.

## Testing and gates

Each rule is written test-first, each test states the mutation that reddens it and is seen red. The error-code tables and the lint page are drift-guarded in both directions, so every new code and rule lands with its row. Probes of the corpus come first: what relies on shadowing, and that the new lints are silent over the examples, the goldens and the standard library.

Standing gates: PM-1 byte-identity (the linker change is in core); core neutrality; the optimization × mechanism matrix; compiled-stdlib byte-identity at both optimization levels — expected to hold unless a standard-library graph names a set or a named map, in which case the digest move is called out and the pin regenerated deliberately.

## Order of work

Corpus probes → one definition at link (core, both toolchains) → one definition at compile → `duplicate-pattern-binding` → the `return` rule on grafts → one argument check across units → wrong-kind names across units → `undeclared-exit` at assembly and link → the three lints and the exemption → the digest → the documentation statements.
