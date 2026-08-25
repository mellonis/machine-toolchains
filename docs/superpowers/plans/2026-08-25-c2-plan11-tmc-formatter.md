# C2 Plan 11 — the `.tmc` formatter onto the green tree

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Move `tmt fmt`'s `.tmc` printer off the C1 CST and onto the green
tree, so that after this plan `parse_cst` has **zero production callers in
either crate** and survives only as half the differential oracle.

**Architecture:** The green printer is a **parallel implementation**, exactly
as plan 6 built PM's. `fmt.rs` becomes a directory (`fmt/mod.rs`, a pure move
in Task 1) and gains `fmt/trivia.rs` — everything C1 stored as a *derived*
field, re-derived from green trivia tokens — and `fmt/print.rs`, the green
printer itself. Values come from the typed views and the `syntax::extract_*`
helpers, which already reproduce `lower_cst`'s decisions and are oracle-tested.
The C1 printer stays untouched as the differential reference; each task widens
the set of sources on which `print::format_green(src) == mod::format(src)`
holds; Task 8 swaps `format()` over and deletes the C1 printer in one commit.

**Tech Stack:** Rust (workspace pinned by `rust-toolchain.toml`), `mtc-core`'s
`syntax` framework (green/red model, `children_with_tokens`, `TextLineIndex`),
`mtc-turing-machine`'s `syntax` module (`TmcKind`, views, `extract_*`) and
`parser`'s `reparse_*` shims.

**Spec:** `docs/superpowers/specs/2026-08-17-c2-green-tree-syntax-design.md`
(§5.1 fmt; §6 oracles and gates). Read §5.1 and §6 before Task 1.

---

## Global Constraints

- **Byte-identical output is THE gate.** Every existing fmt fixture, the
  corpus, the adversarial set Task 1 commits, and the property tests must
  produce output identical to today's, byte for byte. If a green walk would
  render something *better*, it still renders it the old way in this plan.
  Behavior fixes are a separate, deliberate change — mixing one in destroys
  the only oracle that proves this rewrite faithful. Three quirks were
  measured before this plan was written and **must be reproduced**; they are
  named under "Quirks that must survive" below.
- **The compiled-stdlib byte-identity gate applies to this plan**: compile the
  embedded stdlib before and after at both opt levels and byte-compare the
  object. This is the standing gate for any text-only or formatter change.
- **TM only, and no `crates/core` diff.** Check at every commit with
  `git diff --stat master -- crates/core crates/post-machine`, which must
  print nothing. Everything the printer needs already exists in core's
  `syntax` (`children_with_tokens`, `prev_sibling_or_token`,
  `next_sibling_or_token`, `first_token`, `last_token`, `TextLineIndex`) and
  was verified present before this plan was written. A diff in either crate
  here is a design smell, not a convenience — raise it as a ruling instead of
  taking it.
- **ALL SEVEN doc-run kinds retro-wrap. This is the INVERSE of plan 6's PM
  constraint — do not pattern-match plan 6.** PM's plan says "FUNCTION green
  nodes retro-wrap their bound doc run; NAMESPACE nodes do NOT", because a doc
  run before `namespace` is a `DanglingDocRun` fatal on the PM side. On the TM
  side a documented `namespace` is legal and its NAMESPACE node retro-wraps
  like every other. Measured directly for NAMESPACE, REUSE, STATE and GRAFT
  (dumps below); ALPHABET, MACHINE and BIND carry the same `doc_run` field and
  the same parser shape. The seven: **ALPHABET, REUSE, MACHINE, NAMESPACE,
  STATE, GRAFT, BIND.**
- **Brace-line comments live in two different places.** There are **five**
  brace owners in the C1 CST (`open_trailing`/`close_trailing` exist on
  ALPHABET, REUSE, MACHINE, NAMESPACE, STATE — GRAFT and BIND are
  `;`-terminated and have neither). For **ALPHABET, NAMESPACE and STATE** the
  braces are direct children of the declaration node. For **MACHINE and
  REUSE** the braces belong to the interposed `WORLD` node, so `open_trailing`
  must be asked of `WORLD`, not of the declaration. One primitive, two nodes
  to ask.
- **`close_trailing` and a `;`-terminated declaration's `trailing` are the
  same primitive.** A comment after a node's last token is the next sibling
  token in the **parent's** stream, whether that last token is `}` or `;`. The
  printer already unifies them in `Rendered::trailing`; the trivia layer
  exposes exactly one function.
- **Values from extraction, trivia from `fmt/trivia.rs`.** Anything C1 stored
  as a parsed VALUE (`Pattern`, `WriteVec`, `MoveVec`, `Transition`,
  `AlphabetElem`, `Signature`, `BindingArg`, `SymMap`, `QualName`,
  `DocRunItem`) comes from the `syntax::extract_*` helpers and the `reparse_*`
  shims they call — those are oracle-tested against `lower_cst` and are not to
  be reimplemented. Anything C1 stored as a DERIVED field (`blank_before`,
  `trailing`, `open_trailing`, `close_trailing`, `interior`, `sig_interior`,
  `call_args`, `map_pairs`, `pattern_cells`, `write_cells`, `move_cells`, and
  the three `Comment` variants of `TopKind`/`WorldKind`/`RuleKind`) is
  re-derived by `fmt/trivia.rs`.
- **Duplicate; do not share.** The C1 printer is the differential reference. A
  helper shared between the two printers makes the oracle blind to any bug
  introduced inside it. Copy what you need into `print.rs` and delete the C1
  side at Task 8. The transient duplication is intended — do not "DRY" them
  together mid-plan, and do not edit the C1 printer's functions in Tasks 1–7.
- **The C1 printer stays working until Task 8.** Tasks 2–7 keep every new test
  **inside the crate** as `#[cfg(test)]` unit tests, because `format_green` is
  `pub(crate)` and integration tests cannot see it. Do not widen its
  visibility to make an integration test reachable; Task 8 is where the public
  entry point becomes green.
- Conventional commits with scope, e.g. `feat(turing-machine):`,
  `test(turing-machine):`, `docs:`.
- No AI/Claude attribution in any commit message or artifact.
- Code comments cite durable `docs/` pages by page + parenthetical keyword
  (`docs/tmt/fmt.md (interior comments)`). Never cite a `docs/superpowers/`
  spec or plan from code, and never let `spec §N` notation reach a doc
  comment. Published docs are forge-agnostic: no issue/PR numbers, no
  hosting-provider URLs.

---

## Quirks that must survive

All four measured against the real `format()` before this plan was written.
The first three are **behavior**; the fourth is a **known C1/green divergence
that would break byte-identity** and is the single hardest item in this plan.

### 1. A comment between a declaration's keyword and its name relocates

```
state /* mid */ s {          →    state s {
  [*] -> stop;                      /* mid */
}                                   [*] -> stop;
                                  }
```

and, with an `entry` keyword and a line comment, relocates **and emits a blank
line that was not written**:

```
entry // why                 →    entry state s {
state s {                           // why
  [*] -> stop;
}                                     [*] -> stop;
                                  }
```

Both are idempotent. Reproduce them.

### 2. A leading space inside a bracket is eaten

`[ /* p */ *]` prints as `[/* p */ *]`. Idempotent. Reproduce it.

### 3. `.tmc` fmt has its own two-pass settle — a SECOND idempotency exception

```
pass 0:  alphabet /* a */ ab { '_' }
pass 1:  alphabet ab { /* a */ '_' }
pass 2:  alphabet ab { /* a */
           '_'
         }
pass 3:  == pass 2
```

The mechanism, verified against the code rather than guessed: quirk 1
relocates the comment to just after the `{`; on re-parse it is no longer an
interior comment but `open_trailing`, and `render_alphabet`'s first two
branches both require `open_trailing.is_empty()`, so the body is forced
multi-line. **One class, not two unrelated quirks: a comment relocated into a
brace-delimited body re-parses as `open_trailing`.** `CLAUDE.md` currently
claims fmt is "idempotent with one inherited exception" and names only the
`.pmc` labels/command case — a false sentence about `.tmc`. Task 9 corrects
it. Task 1 pins the settle with its own two-pass fixture, because the corpus
is fmt-clean and contains no source of this shape: without the fixture a green
printer that accidentally *fixed* the settle would pass every test in the
repository while silently changing behavior under a byte-identity constraint.

### 4. `prev_end_line` diverges on a multi-line block comment riding a `;` — and fmt makes it visible

`syntax::extract::prev_end_line` carries a documented, deliberately tolerated
divergence from the C1 parser's stateful `prev_end_line`: a MULTI-LINE block
comment riding a `;` is claimed by C1's `Parser::take_trailing`, the one
comment-capturing helper that leaves `prev_end_line` at the `;`, while the
green side takes the last non-whitespace character — the comment's `*/`. The
existing test `a_block_comment_riding_a_semicolon_diverges_on_blank_before_only`
asserts the divergence and records that it affects exactly one field of
exactly one item: the FIRST `DocRunItem`'s `blank_before`. That was safe
because `reduce_doc_run` folds over `kind` alone, so the field is dead in a
`Program`.

**`fmt` reads that exact field.** `leads_with_blank` returns
`doc_run.first().blank_before`, which becomes `Rendered::blank_before`.
Measured on the test's own source:

```
use a; /* one            C1  →    use a; /* one
two */                            two */
? doc                                          ← blank line C1 emits
alphabet b { '0' }                ? doc
                                  alphabet b { '0' }
```

A green printer built naively on `extract_doc_items` emits **no** blank line
there. That is a byte-identity failure on a four-line source.

**Ruling: fix the green side to agree with C1, in `extract::prev_end_line`,
as Task 2's first step.** The alternatives were weighed. Reproducing C1's rule
inside `fmt` only would leave two different `prev_end_line` rules in one
crate. Accepting the difference would carve a hole in the one gate that makes
this rewrite verifiable. Fixing the shared helper costs one narrow rule, makes
the two paths agree everywhere measured, and retires a documented wart from
the arc.

The rule, and why it is this narrow: C1 updates `prev_end_line` from a
`}`-closing declaration's trailing comment (`close_trailing`, newline count
included) but **not** from a `;`-terminated declaration's trailing comment. So
the two sides already agree for every brace-closed declaration, and for every
single-line trailing comment. They diverge only when a **multi-line** comment
rides a **`;`**. Task 2 makes the green walk skip exactly that comment and use
the `;`'s line.

---

## The tree shape this plan is built on

Verified against the real parser before this plan was written, by dumping
`parse_green_from_tokens` over two hand-built comment-rich sources. Trivia
flushes into the current node before a child opens, so **a node starts at its
first significant token — or at its retro-wrapped `DOC_RUN` — and trivia sits
between a parent's children.**

Doc runs are inside the node they document, and `ATTR` wraps an attention line
that opens with a valid attribute:

```
ALPHABET@63..221
  DOC_RUN@63..116
    DOC_LINE@63..77 "? doc line one"
    WHITESPACE@77..78 "\n"
    DOC_LINE@78..92 "? doc line two"
    WHITESPACE@92..93 "\n"
    ATTR@93..116
      ATTENTION_LINE@93..116 "![deprecated] attention"
  WHITESPACE@116..117 "\n"          <- the run→declaration gap
  IDENT@117..125 "alphabet"
  IDENT@126..128 "ab"
  L_BRACE@129..130 "{"
  WHITESPACE@130..131 " "
  LINE_COMMENT@131..152 "// open brace comment"      <- open_trailing
  ...
  GLYPH@155..158 "'_'"  COMMA  LINE_COMMENT("// after underscore")
  BLOCK_COMMENT("/* before a */")  GLYPH@197..200 "'a'"
  LINE_COMMENT@203..219 "// before closer"
  R_BRACE@220..221 "}"
WHITESPACE@221..222 " "
LINE_COMMENT@222..239 "// close trailing"            <- in the PARENT stream
```

A `machine`/`routine`/`graph` interposes `WORLD`, which owns the braces:

```
REUSE@257..447
  DOC_RUN@257..270
  IDENT("routine")  IDENT("r")  L_PAREN  SIG_PARAM@283..293  R_PAREN
  WORLD@295..447
    L_BRACE@295..296 "{"
    LINE_COMMENT@297..304 "// open"      <- open_trailing lives HERE
    STATE@309..443
      IDENT("entry") IDENT("state") IDENT("s") L_BRACE
      RULE@331..368
        L_BRACKET GLYPH R_BRACKET ARROW IDENT("write")
        WRITE_VEC@346..351  IDENT("move")  MOVE_VEC@357..360
        TRANSITION@361..367  SEMI
      LINE_COMMENT@369..385 "// rule trailing"     <- STATE's stream
      LINE_COMMENT@392..416 "// own-line inside state"
      RULE@423..437
      R_BRACE@442..443
    R_BRACE@446..447
LINE_COMMENT@448..456 "// close"          <- NAMESPACE's stream
```

Binding lists and maps nest as nodes, so a comma scan never has to track
depth:

```
GRAFT@232..427
  DOC_RUN@232..250                      <- retro-wrapped
  IDENT("entry") IDENT("graft") IDENT("n") COLON_COLON IDENT("g") L_PAREN
  LINE_COMMENT@275..304 "// interior of the graft list"    <- interior, index 0
  BINDING_ARG@309..398
    IDENT("t") EQ IDENT("main") IDENT("with")
    SYM_MAP@323..398
      IDENT("map") L_BRACE
      LINE_COMMENT@335..357 "// interior of the map"       <- map_pairs (0, 0)
      GLYPH ARROW GLYPH COMMA GLYPH ARROW GLYPH R_BRACE
  COMMA  BINDING_ARG@404..414  R_PAREN  IDENT("as") IDENT("inst") SEMI
```

Three consequences worth naming, because each is easy to get wrong:

1. **A comment after a closing `}` or a `;` is not inside the node it
   follows.** It is the next sibling token in the parent's stream. `trailing`
   and `close_trailing` are one function.
2. **`Rendered::blank_before` collapses to one rule on the green tree.** C1
   needed `leads_with_blank` to branch — a documented declaration repurposes
   its own `blank_before` for the run→declaration gap, moving the outer
   decision onto `doc_run[0].blank_before`. In green both cases are *the gap
   before the node's first token*, because the run is inside the node. The
   branch disappears; the run→declaration gap becomes a separate, smaller
   query (`blank_before_decl`).
3. **A rule's PATTERN has no node.** It is RULE's own leading tokens between
   `[` and `]`, ahead of the `->`. Likewise an alphabet's elements, a map's
   pairs, and a vector's cells are bare token runs, not nodes. Interior-comment
   attribution over those is **"entries started so far"**, never a comma count
   — a comment after the last entry has `n-1` commas before it but must key to
   index `n`.

---

## The trivia census — this plan's coverage checklist

The set of derived fields in `crates/turing-machine/src/cst.rs` is the only
enumeration of trivia that is complete by construction, because it is exactly
what C1 stores. Every row needs at least one adversarial source (Task 1) and
at least one differential test (Tasks 3–7).

| C1 field | Owners | Green source |
|---|---|---|
| `blank_before` | `TopItem`, `WorldItem`, `RuleItem` | gap before the unit's first token |
| `DocRunItem::blank_before` | doc-run lines | `extract_doc_items` (Task 2 fixes item 0) |
| `TopKind::Comment` / `WorldKind::Comment` / `RuleKind::Comment` | 3 container levels | own-line comment tokens in the container's stream |
| `trailing` | `UseCst`, `TapeCst`, `RuleCst`, `GraftCst`, `BindCst` | next sibling comment on the node's last line |
| `close_trailing` | `AlphabetCst`, `ReuseCst`, `MachineCst`, `NamespaceCst`, `StateCst` | same primitive as `trailing` |
| `open_trailing` | the same five | comments after `{` on the `{`'s line — in `WORLD` for MACHINE/REUSE |
| `doc_run` | the seven retro-wrapping kinds | `DocRunView` + `extract_doc_items` |
| `interior` | `UseCst` (`use` … `;`), `AlphabetCst`, `GraftCst`, `BindCst` | entries-started-so-far over the list's element stream — **which begins at the declaration's HEADER, not at the opening delimiter; see below** |
| `sig_interior` | `ReuseCst` (the `()` around `sig.params`) | same primitive, same header rule |
| `call_args` | `RuleCst` (a `call` transition's `()`) | same primitive, inside `TRANSITION` |
| `map_pairs` | `RuleCst`, `GraftCst`, `BindCst` — keyed `(arg index, pair index)` | same primitive, inside each `SYM_MAP`, one level down |
| `pattern_cells` | `RuleCst` (`[]` before `->`) | same primitive over RULE's leading bracket run |
| `write_cells` / `move_cells` | `RuleCst` | same primitive inside `WRITE_VEC` / `MOVE_VEC` |


### The interior stream starts at the HEADER, not at the delimiter

Measured against the real formatter, and the single easiest thing in this plan to
get wrong, because the obvious reading — "the elements between `{` and `}`" — is
wrong on every delimited surface:

| written | C1 prints |
|---|---|
| `alphabet /* a */ ab { '_' }` | `alphabet ab { /* a */ '_' }` |
| `alphabet ab /* a */ { '_' }` | `alphabet ab { /* a */ '_' }` |
| `routine /* c */ r(tape t: ab)` | `routine r( /* c */` … |
| `graft /* c */ n::g(t = main, …)` | `graft n::g( /* c */` … |

The mechanism is C1's `Parser::interior_comments`, which drains on a GLOBAL
cursor (`while self.comments[self.cpos].sig_index <= self.pos`): the list's first
drain sweeps up every comment still unclaimed since the previous one, which
includes everything written in the declaration's header. So the element stream a
surface hands `trivia::interior` is:

> the declaration's header (its first token through the opening delimiter),
> **plus** the delimiter's interior, **minus** whatever `open_trailing` already
> claimed — in that order.

A naive header-through-closer slice double-counts the brace-line comments the
open run took; a naive delimiter-only slice silently DROPS the header ones. Note
that the header half is the same slice `trivia::units`'s head scan already
computes, so it is available rather than needing a new walk.

This is why `constraints.md`'s mandated quirk (3) — `alphabet /* a */ ab { '_' }`
settling on the second pass — is a Task 7 obligation and not merely a Task 1
fixture: without the header half, that comment is dropped and the quirk cannot
reproduce.

---

## File Structure

| File | Responsibility |
|---|---|
| `crates/turing-machine/src/fmt/mod.rs` (moved from `fmt.rs`, Task 1) | Public door + the C1 printer, verbatim, until Task 8 deletes the printer. |
| `crates/turing-machine/src/fmt/trivia.rs` (new, Task 2) | Comment/blank-line classification over green trivia. Pure functions on `SyntaxNode`/`SyntaxToken`; no printing, no output text. |
| `crates/turing-machine/src/fmt/print.rs` (new, Task 3) | The green printer. Grows Tasks 3–7; becomes the only printer at Task 8. |
| `crates/turing-machine/src/syntax/extract.rs` (modified, Task 2) | `prev_end_line` agrees with C1; several `extract_*` helpers become `pub(crate)`. |
| `crates/turing-machine/tests/fmt_adversarial/*.tmc` (new, Task 1) | Sources exercising every census row. Parse-only; not fmt-clean by design. |
| `crates/turing-machine/tests/fmt_tmc.rs` (modified, Tasks 1 and 8) | The two-pass settle fixture; the green corpus×adversarial byte-identity check. |
| `CLAUDE.md`, `docs/tmt/fmt.md`, `docs/lsp.md` (modified, Task 9) | The C1-shaped descriptions of how fmt reads source, and the idempotency claim. |

### The three buckets — what to copy and what to rewrite

`fmt.rs` is 1682 lines, but only a third of it faces the tree. Sorting its
functions once turns a rewrite into a bounded conversion. **Copy verbatim**
means copy: these are correct, tested, and any edit is a byte-identity risk
with no upside.

- **AST-value → text — copy verbatim.** `normalize_comment_text`,
  `comment_line`, `open_trailing_text`, `interior_lines`, `interior_trailing`,
  `doc_run_text`, `doc_line`, `glyph_text`, `sym_text`, `alphabet_elem_text`,
  `pattern_text`, `join_cells_with_interior`, `pattern_cell_text`,
  `move_cell_text`, `move_vec_text`, `binding_arg_text`, `binding_value_text`,
  `sym_map_text`, `map_pair_count`, `map_interior_for`, `binding_entries`,
  `term_text`, `continuation_text`, `signature_params`, `use_path_text`,
  `fold_token_text`, `transition_text`, `glyph_vec_multiline`.
- **Layout machinery — copy verbatim.** `Rendered`, `flush`,
  `trailing_spacing`, `Interior`, `bucket`, `Grid`, `paren_list`, `col_after`,
  `INDENT_UNIT`, `LINE_WIDTH`.
- **Tree-facing — rewrite against views + trivia.** `print_cst`,
  `render_top_items`, `render_top_item`, `render_use`, `render_alphabet`,
  `render_namespace`, `render_reuse`, `render_machine`, `render_world_items`,
  `render_tape`, `render_graft`, `render_bind`, `render_block_state`,
  `render_inline_state`, `inline_state_line`, `inline_state_runs`,
  `inline_candidate`, `state_header_text`, `render_rule_item`, `render_rule`,
  `render_rule_off_grid`, `grid_for`, `RuleCst::breaks_the_grid`,
  `tape_name_widths`, `leads_with_blank` (deleted — see consequence 2),
  `write_cell_text`/`write_vec_text` (lose their `tokens` parameter), and
  `subst_body_text` (collapses to the node's own text).

---

## Task 1: the module move, the adversarial set, and the two-pass fixture

**Files:**
- Move: `crates/turing-machine/src/fmt.rs` → `crates/turing-machine/src/fmt/mod.rs`
- Create: `crates/turing-machine/tests/fmt_adversarial/*.tmc`
- Modify: `crates/turing-machine/tests/fmt_tmc.rs`

**Interfaces:**
- Produces: the adversarial corpus every later task's differential test reads,
  and the settle fixture Task 8's gate must still satisfy.

- [ ] **Step 1: Move the file, changing nothing else**

```bash
git mv crates/turing-machine/src/fmt.rs crates/turing-machine/src/fmt/mod.rs
```

`lib.rs` already says `pub mod fmt;` and needs no change. Run
`cargo test -p mtc-turing-machine` and `cargo fmt --check`; both must be green
with a **zero-line content diff** (`git show --stat` shows a pure rename).
Commit this alone — a later task's diff must never mix a 1682-line move with
real logic, or the reviewer can see neither.

```bash
git commit -m "refactor(turing-machine): fmt.rs becomes fmt/mod.rs ahead of the green printer"
```

- [ ] **Step 2: Write the adversarial sources**

One file per census row that the shipped corpus does not already cover. Every
file must **parse** — run each through `format()` before committing; a fixture
that does not parse is a fixture that tests nothing. (This is not
hypothetical: four of the six sources drafted for this plan failed to parse on
first write and were corrected against the real parser.)

Create `crates/turing-machine/tests/fmt_adversarial/` with at least these,
named for what they carry:

`quirk_keyword_name.tmc`
```tmc
alphabet ab { '_', 'a' }

machine {
  tape main: ab;
  state /* mid */ s {
    [*] -> stop;
  }
  entry // why
  state t {
    [*] -> stop;
  }
}
```

`quirk_bracket_space.tmc`
```tmc
alphabet ab { '_', 'a' }

machine {
  tape main: ab;
  entry state s {
    [ /* p */ *] -> write [ /* w */ 'a' ] move [ /* m */ . ] stop;
  }
}
```

`divergence_semicolon_block_comment.tmc`
```tmc
use a; /* one
two */
? doc
alphabet b { '0' }
```

`brace_comments.tmc` — `open_trailing` and `close_trailing` on all five brace
owners, `WORLD`-owned and node-owned alike:
```tmc
alphabet ab { // alphabet open
  '_',
  'a'
} // alphabet close

namespace n { // namespace open
  routine r(tape t: ab) { // reuse open
    entry state s { // state open
      [*] -> stop;
    } // state close
  } // reuse close
} // namespace close

machine { // machine open
  tape main: ab;
  entry state go {
    [*] -> stop;
  }
} // machine close
```

`interior_lists.tmc` — one interior comment in each list surface, including a
comment before the closer (index `n`), a nested map (`(arg, pair)`), and a
`{expr}` substitution cell proving a flat comma scan is safe:
```tmc
use // before the first path
  a::b, // after the first
  c::d
  // before the semicolon
;

alphabet ab {
  // before the first glyph
  '_', /* after the first */
  'a'
  // before the closer
}

namespace n {
  export graph g(
    // before the first parameter
    tape t: ab, /* after the first */
    state done
    // before the closer
  ) {
    entry state s {
      [*] -> done;
    }
  }
}

machine {
  tape main: ab;

  entry graft n::g(
    // before the first binding
    t = main with map {
      // before the first pair
      '_' -> '_', /* after the first pair */
      'a' -> 'a'
      // before the closing brace
    },
    done = fin
    // before the closing paren
  ) as inst;

  state fin {
    [ /* first cell */ *] -> write [{ 0 + 1 }] move [.] stop;
    [*] -> call n::g(
             // before the first call binding
             t = main,
             done = fin
           ) then stop;
  }
}
```

`trailing_and_blanks.tmc` — `trailing` on all five `;`-terminated kinds,
own-line comments at all three container levels, and blank runs of two and
three lines:
```tmc
// a file-level own-line comment

use a::b; // use trailing


alphabet ab { '_', 'a' } // alphabet trailing

namespace n {
  // a namespace-level own-line comment
  routine r(tape t: ab) {
    entry state s {
      // a state-level own-line comment
      [*] -> stop; // rule trailing
    }
  }
}

machine {
  tape main: ab; // tape trailing

  // a world-level own-line comment
  entry graft n::r(t = main) as one; // graft trailing
  bind n::r(t = main) as two; // bind trailing

  state fin { [*] -> stop; }
}
```

`docs_and_attention.tmc` — a doc run on all seven retro-wrapping kinds, an
`![deprecated]` attribute, blank lines inside a run, and a blank between a run
and its declaration:
```tmc
? a documented namespace
namespace n {
  ? a documented alphabet
  !
  ![deprecated] use the other one
  export alphabet ab { '_', 'a' }

  ? a documented graph
  ?
  ? with a blank line inside the run

  export graph g(tape t: ab, state done) {
    ? a documented state
    entry state s {
      [*] -> done;
    }
  }
}

? a documented machine
machine {
  tape main: ab;

  ? a documented graft
  entry graft n::g(t = main, done = fin) as inst;

  ? a documented bind
  bind n::g(t = main, done = fin) as other;

  state fin { [*] -> stop; }
}
```

- [ ] **Step 3: Prove every source parses and record what it does today**

Add to `crates/turing-machine/tests/fmt_tmc.rs`:

```rust
/// The adversarial sources — shapes the shipped corpus does not carry, one
/// per derived field the pre-green CST stored. They are not required to be
/// fmt-clean (several are not), so `corpus()`, whose dogfood lock demands
/// exactly that, must never sweep them. This test asserts only that each
/// parses and formats, which is the precondition every later differential
/// check depends on.
#[test]
fn every_adversarial_source_formats() {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fmt_adversarial");
    let mut seen = 0;
    let mut paths: Vec<std::path::PathBuf> = std::fs::read_dir(dir)
        .expect("the adversarial directory exists")
        .map(|e| e.expect("a readable entry").path())
        .filter(|p| p.extension().and_then(|x| x.to_str()) == Some("tmc"))
        .collect();
    paths.sort();
    for path in paths {
        let src = std::fs::read_to_string(&path).expect("a readable fixture");
        format(&src).unwrap_or_else(|e| panic!("{} does not format: {e:?}", path.display()));
        seen += 1;
    }
    assert!(seen >= 6, "expected the whole adversarial set, saw {seen}");
}
```

Run: `cargo test -p mtc-turing-machine --test fmt_tmc`
Expected: PASS. If any fixture fails to parse, fix the FIXTURE — the sources
above are drafts, and the real parser is the authority.

- [ ] **Step 4: Pin the two-pass settle**

Also in `fmt_tmc.rs`. This is the fixture that stops a green printer from
silently *fixing* the settle:

```rust
/// `.tmc` fmt is idempotent with one exception, and this is it: a comment
/// written between a declaration's keyword and its name relocates into the
/// brace-delimited body (pass 1), where it re-parses as a comment on the
/// opening brace — which forces the body multi-line (pass 2). Stable from
/// pass 2 on. Pinned by value rather than by "settles eventually", so a
/// printer that changed either intermediate form fails here
/// (docs/tmt/fmt.md (idempotency)).
#[test]
fn a_comment_between_a_keyword_and_its_name_settles_on_the_second_pass() {
    let pass1 = format("alphabet /* a */ ab { '_' }\n").expect("formats");
    assert_eq!(pass1, "alphabet ab { /* a */ '_' }\n");
    let pass2 = format(&pass1).expect("formats");
    assert_eq!(pass2, "alphabet ab { /* a */\n  '_'\n}\n");
    let pass3 = format(&pass2).expect("formats");
    assert_eq!(pass3, pass2, "the settle must be stable from pass 2");
}
```

Run: `cargo test -p mtc-turing-machine --test fmt_tmc`
Expected: PASS (these are the measured values).

- [ ] **Step 5: Commit**

```bash
git add crates/turing-machine/tests
git commit -m "test(turing-machine): the adversarial fmt sources and the two-pass settle"
```

---

## Task 2: `fmt/trivia.rs`, and `prev_end_line` agrees with C1

**Files:**
- Modify: `crates/turing-machine/src/syntax/extract.rs`
- Create: `crates/turing-machine/src/fmt/trivia.rs`
- Modify: `crates/turing-machine/src/fmt/mod.rs` (declare the module)

**Interfaces:**
- Consumes: `mtc_core::syntax::{SyntaxNode, SyntaxToken, SyntaxElement, TextLineIndex}`,
  `crate::lexer::{Comment, CommentKind}`. **Not `extract::token_from_syntax`** —
  it maps SIGNIFICANT token kinds only and has no comment arm, so it would
  panic on trivia; this module needs its own small comment converter.
- Produces:
  ```rust
  pub(crate) struct Unit { pub blank_before: bool, pub kind: UnitKind, pub trailing: Option<Comment> }
  pub(crate) enum UnitKind { Comment(Comment), Node(SyntaxNode) }
  pub(crate) fn units(container: &SyntaxNode, index: &TextLineIndex) -> Vec<Unit>;
  pub(crate) fn blank_before_decl(node: &SyntaxNode) -> bool;
  pub(crate) fn open_trailing(brace_owner: &SyntaxNode, index: &TextLineIndex) -> Vec<Comment>;
  pub(crate) fn interior(elems: impl Iterator<Item = SyntaxElement>) -> Vec<(usize, Comment)>;
  ```
  and, from `syntax::extract`, `pub(crate)` on `extract_doc_items`,
  `extract_rule`, `extract_tape`, `extract_alphabet`'s element half,
  `extract_import`, `extract_graft`, `extract_bind` as the later tasks need
  them. Widen visibility one helper at a time, in the task that needs it.

- [ ] **Step 1: Write the failing test for the `prev_end_line` fix**

Rewrite the existing divergence test in `syntax/extract.rs`'s test module. Its
current name and `assert_ne!` encode the old behavior:

```rust
    /// A MULTI-LINE block comment riding a `;` is claimed by C1's
    /// `Parser::take_trailing`, which leaves `prev_end_line` at the `;`.
    /// The green walk must do the same, because the field it feeds — the
    /// FIRST doc item's `blank_before` — is what `fmt` turns into a blank
    /// line before a doc run. The two agreed everywhere else already: C1
    /// DOES advance past a `}`-closing declaration's trailing comment, and
    /// a single-line comment ends on the line it starts.
    #[test]
    fn a_block_comment_riding_a_semicolon_agrees_on_blank_before() {
        let src = "use a; /* one\ntwo */\n? doc\nalphabet b { '0' }\n";

        let cst = parse_cst(&lex_with(src, LexMode::WithComments).unwrap()).unwrap();
        let TopKind::Alphabet(alphabet) = &cst.items[1].kind else {
            panic!("expected the second item to be an alphabet");
        };
        let c1 = alphabet.doc_run.clone();

        let root =
            RootView::cast(SyntaxNode::new_root(parse_green(src).unwrap())).expect("root is ROOT");
        let index = TextLineIndex::new(src);
        let TopView::Alphabet(view) = root.items().last().expect("a last item") else {
            panic!("expected the last item to be an ALPHABET");
        };
        let green = extract_doc_items(&view.doc_run().expect("a doc run"), src, &index);

        assert_eq!(green, c1, "the two paths must agree on the whole run");
        assert!(
            c1[0].blank_before,
            "C1 keeps prev_end_line at the `;`, so the run reads as blank-separated"
        );
    }
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p mtc-turing-machine --lib syntax::extract`
Expected: FAIL on `assert_eq!` — green reports `blank_before: false`.

- [ ] **Step 3: Fix `prev_end_line`**

Walk back from the node's start over whitespace. If what precedes is a comment
token that STARTS on the same line as the last significant token before it,
**and** that significant token is a `;`, return that `;`'s line. Otherwise keep
today's answer (the last non-whitespace character's line). Rewrite the
function's doc comment: it currently documents the divergence as tolerated and
names the old test. State the rule and why it is keyed on `;` — the C1 parser
updates `prev_end_line` from a close-brace trailing comment but not from
`take_trailing`'s.

- [ ] **Step 4: Run it to verify it passes; run the whole crate**

Run: `cargo test -p mtc-turing-machine --lib syntax::extract`, then
`cargo test -p mtc-turing-machine --no-fail-fast`
Expected: PASS. Both differential oracles (`tests/syntax_parity.rs`,
`tests/tmc_property.rs`) must stay green — the field is dead in a `Program`,
so nothing there should move. If something does, stop and report it.

- [ ] **Step 5: Commit**

```bash
git add crates/turing-machine/src/syntax/extract.rs
git commit -m "fix(turing-machine): prev_end_line agrees with the C1 parser on a semicolon's trailing comment"
```

- [ ] **Step 6: Write the failing tests for the trivia layer**

New file `crates/turing-machine/src/fmt/trivia.rs`, tests first. Each of these
is a fact this plan measured; none is a guess:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::{LexMode, lex_with};
    use crate::parser::parse_green_from_tokens;
    use crate::syntax::TmcKind;
    use mtc_core::syntax::SyntaxNode;

    fn tree(src: &str) -> (SyntaxNode, TextLineIndex) {
        let tokens = lex_with(src, LexMode::WithComments).expect("lexes");
        let green = parse_green_from_tokens(src, &tokens).expect("parses");
        (SyntaxNode::new_root(green), TextLineIndex::new(src))
    }

    /// An own-line comment is a unit of its own; a same-line comment after a
    /// node rides that node. The two are told apart by the whitespace before
    /// the comment, never by a line number computed twice.
    #[test]
    fn own_line_comments_are_units_and_same_line_comments_are_trailing() {
        let src = "// standalone\nuse a::b; // trailing\n";
        let (root, index) = tree(src);
        let units = units(&root, &index);
        assert_eq!(units.len(), 2);
        assert!(matches!(&units[0].kind, UnitKind::Comment(c) if c.text == "// standalone"));
        assert!(units[0].trailing.is_none());
        assert!(matches!(&units[1].kind, UnitKind::Node(n) if n.kind() == TmcKind::Use.into()));
        assert_eq!(
            units[1].trailing.as_ref().map(|c| c.text.as_str()),
            Some("// trailing")
        );
    }

    /// A blank line is presence, not a count: two blank lines and three
    /// report the same, because the printer collapses any run to one.
    #[test]
    fn blank_before_is_presence_not_a_count() {
        for gap in ["\n\n", "\n\n\n", "\n\n\n\n"] {
            let src = format!("use a::b;{gap}use c::d;\n");
            let (root, index) = tree(&src);
            let units = units(&root, &index);
            assert!(units[1].blank_before, "gap {gap:?} must read as blank");
        }
        let (root, index) = tree("use a::b;\nuse c::d;\n");
        assert!(!units(&root, &index)[1].blank_before);
    }

    /// A documented declaration's unit starts at its retro-wrapped DOC_RUN,
    /// so ONE rule answers what C1 needed `leads_with_blank` to branch for.
    /// All seven doc-run kinds retro-wrap on this language — including
    /// `namespace`, which is where the PM sibling's rule does NOT apply.
    #[test]
    fn a_documented_declarations_unit_starts_at_its_doc_run() {
        let src = "use a::b;\n\n? doc\nnamespace n {\n}\n";
        let (root, index) = tree(src);
        let units = units(&root, &index);
        assert!(
            units[1].blank_before,
            "the blank sits before the DOC_RUN, and the unit starts there"
        );
        let UnitKind::Node(ns) = &units[1].kind else {
            panic!("expected a node")
        };
        assert_eq!(ns.kind(), TmcKind::Namespace.into());
        assert!(
            !blank_before_decl(ns),
            "no blank between the run and `namespace`"
        );

        let src = "use a::b;\n? doc\n\nnamespace n {\n}\n";
        let (root, index) = tree(src);
        let units = units(&root, &index);
        assert!(!units[1].blank_before);
        let UnitKind::Node(ns) = &units[1].kind else {
            panic!("expected a node")
        };
        assert!(blank_before_decl(ns), "the run→declaration gap is its own query");
    }

    /// Brace comments live in two different streams: the declaration's own
    /// for ALPHABET/NAMESPACE/STATE, the interposed WORLD's for
    /// MACHINE/REUSE. One primitive, two nodes to ask — asking the wrong one
    /// silently yields an empty vector, which is why this test names both.
    #[test]
    fn open_trailing_comes_from_the_node_that_owns_the_brace() {
        let src = "alphabet ab { // on the brace\n  '_'\n}\n";
        let (root, index) = tree(src);
        let alphabet = root
            .children()
            .find(|n| n.kind() == TmcKind::Alphabet.into())
            .expect("an ALPHABET");
        let comments = open_trailing(&alphabet, &index);
        assert_eq!(comments.len(), 1);
        assert_eq!(comments[0].text, "// on the brace");

        let src = "alphabet ab { '_' }\nmachine { // on the brace\n  tape t: ab;\n}\n";
        let (root, index) = tree(src);
        let machine = root
            .children()
            .find(|n| n.kind() == TmcKind::Machine.into())
            .expect("a MACHINE");
        assert!(
            open_trailing(&machine, &index).is_empty(),
            "MACHINE does not own its brace — WORLD does"
        );
        let world = machine
            .children()
            .find(|n| n.kind() == TmcKind::World.into())
            .expect("a WORLD");
        assert_eq!(open_trailing(&world, &index)[0].text, "// on the brace");
    }

    /// Interior attribution counts ENTRIES STARTED, never commas: a comment
    /// after the last entry has `n-1` commas before it and must key to `n`.
    #[test]
    fn interior_keys_a_trailing_comment_to_the_entry_count() {
        let src = "alphabet ab {\n  // zero\n  '_', // one\n  'a'\n  // two\n}\n";
        let (root, index) = tree(src);
        let alphabet = root
            .children()
            .find(|n| n.kind() == TmcKind::Alphabet.into())
            .expect("an ALPHABET");
        let elems = between_braces(&alphabet);
        let found = interior(elems);
        let keys: Vec<usize> = found.iter().map(|(i, _)| *i).collect();
        assert_eq!(keys, vec![0, 1, 2], "two entries, so the last key is 2");
    }

    /// A `{expr}` substitution cell carries braces but never a comma, so a
    /// flat scan over a vector's elements needs no depth tracking. Pinned
    /// because the alternative — assuming it — is exactly the assumption a
    /// nested map would break.
    #[test]
    fn a_substitution_cell_does_not_confuse_the_entry_scan() {
        let src = "alphabet ab { '_', 'a' }\nmachine {\n  tape t: ab;\n  entry state s {\n    [*] -> write [/* c */ {0 + 1}] stop;\n  }\n}\n";
        let (root, index) = tree(src);
        // `SyntaxNode` has `children`, `children_with_tokens`, `ancestors`
        // and `descendant_tokens` — there is NO `descendants()`. Walk down
        // with a small local recursion; core is frozen for this plan.
        fn find_kind(n: &SyntaxNode, k: TmcKind) -> Option<SyntaxNode> {
            if n.kind() == k.into() {
                return Some(n.clone());
            }
            n.children().find_map(|c| find_kind(&c, k))
        }
        let vec_node = find_kind(&root, TmcKind::WriteVec).expect("a WRITE_VEC");
        let found = interior(between_brackets(&vec_node));
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].0, 0, "one cell, comment before it");
    }
}
```

`between_braces` / `between_brackets` are two-line helpers this module owns:
the direct children strictly between the node's first `{`/`[` and its last
`}`/`]`. Write them beside the tests, not in the test module — the printer
needs them too.

- [ ] **Step 7: Run the tests to verify they fail**

Run: `cargo test -p mtc-turing-machine --lib fmt::trivia`
Expected: FAIL to compile — nothing in `trivia` exists yet.

- [ ] **Step 8: Write the implementation**

`units` walks `container.children_with_tokens()`. A comment token is own-line
when the whitespace token before it contains a newline, or it is the
container's first element; otherwise it is the previous unit's `trailing`.
`blank_before` is `true` when the whitespace immediately before the unit's
first token contains **two or more** newlines. **Do not compute line numbers
and subtract them** — a multi-line block comment makes that wrong, and it is
the mistake `prev_end_line` already cost this arc once.

Build `Comment` values with a small converter local to this module.
`extract::token_from_syntax` cannot serve: it maps the SIGNIFICANT token kinds
and has no arm for `LineComment`/`BlockComment`, because `sig_tokens` filters
trivia out before it is ever called — handing it a comment token panics. Pin the
local converter against `lex_with(.., WithComments)` over real fixtures so its
`kind`, `line` and `col` provably agree with the lexer's own, which is the
guarantee the shared converter would have bought.

`open_trailing` scans forward from the node's first `L_BRACE` child, taking
comment tokens until a whitespace token containing a newline. `interior`
tracks `entries_started`, incrementing when a significant element follows the
opener or a top-level comma, and keys each comment token to the current count.

- [ ] **Step 9: Run the tests to verify they pass**

Run: `cargo test -p mtc-turing-machine --lib fmt::trivia`
Expected: PASS, 6 tests.

- [ ] **Step 10: Commit**

```bash
git add crates/turing-machine/src/fmt
git commit -m "feat(turing-machine): re-derive the .tmc formatter's trivia from the green tree"
```

---

## Task 3: `fmt/print.rs` — the file skeleton

**Files:**
- Create: `crates/turing-machine/src/fmt/print.rs`
- Modify: `crates/turing-machine/src/fmt/mod.rs` (declare the module)

**Interfaces:**
- Consumes: Task 2's `trivia`; `crate::syntax::views::{RootView, TopView, UseView, AlphabetView, NamespaceView}`;
  `crate::syntax::extract`'s import/alphabet/doc helpers.
- Produces: `pub(crate) fn format_green(source: &str) -> Result<String, CompileError>`,
  handling `use`, `alphabet`, `namespace` (nested), doc runs, file- and
  namespace-level own-line comments, trailing comments and blank lines.
  Comment-free lists only; interior comments are Task 7.

- [ ] **Step 1: Write the failing differential test**

In `print.rs`'s own test module. This is the harness every later task widens —
write it so adding a source is one line:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    /// The differential oracle: the green printer and the C1 printer must
    /// agree byte for byte. A green walk that renders something BETTER still
    /// fails here, and that is the point — the C1 printer is the only
    /// reference that proves this rewrite faithful.
    #[track_caller]
    fn agrees(src: &str) {
        let green = format_green(src).expect("the green printer formats");
        let c1 = crate::fmt::format(src).expect("the C1 printer formats");
        assert_eq!(green, c1, "printers diverged for:\n{src}");
    }

    #[test]
    fn the_file_skeleton_agrees() {
        agrees("");
        agrees("use a::b;\n");
        agrees("use a::b, c::d as e;\n");
        agrees("// standalone\n\nuse a::b; // trailing\n");
        agrees("alphabet ab { '_', 'a' }\n");
        agrees("export alphabet ab { '_'..'z' }\n");
        agrees("? doc\n![deprecated] gone\nalphabet ab { '_' }\n");
        agrees("? doc\n\nalphabet ab { '_' }\n");
        agrees("namespace n {\n  namespace m {\n    alphabet ab { '_' }\n  }\n}\n");
        agrees("namespace n { // open\n  alphabet ab { '_' }\n} // close\n");
        agrees("use a::b;\n\n\nuse c::d;\n");
    }
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p mtc-turing-machine --lib fmt::print`
Expected: FAIL to compile — `format_green` does not exist.

- [ ] **Step 3: Write the implementation**

`format_green` lexes with `LexMode::WithComments`, calls
`parse_green_from_tokens`, builds a `TextLineIndex`, and walks `RootView`.
Copy the layout machinery and the value→text functions listed in "The three
buckets" **verbatim** from `mod.rs` — copy, do not port, and do not import
them from `mod.rs`.

`render_top_items` becomes a walk over `trivia::units(container, &index)`:
`UnitKind::Comment` renders through the copied `comment_line`;
`UnitKind::Node` dispatches on `TopView::cast`. `Rendered::blank_before` is
`unit.blank_before` directly — **`leads_with_blank` is not copied**, because
on this tree the doc run is inside the node and the two cases coincide (see
"The tree shape", consequence 2). `doc_run_text`'s `blank_before_decl`
argument comes from `trivia::blank_before_decl`.

**A comment written INSIDE a doc run is this task's to fix — measured, and
invisible to the entire corpus.** `extract_doc_items` reparses `sig_tokens`,
which filters trivia, so an ordinary comment interleaved in a `?`/`!` run is
DROPPED; worse, the item after it then measures its gap against the previous doc
line instead of against the comment, so a blank line nobody wrote is INVENTED.
Both halves measured:

| source | C1 `doc_run` | `extract_doc_items` today |
|---|---|---|
| `"? doc\n/* c */\nalphabet b { '0' }\n"` | 2 items, no blanks | 1 item |
| `"? doc\n// c\n? more\nalphabet b { '0' }\n"` | 3 items, no blanks | 2 items, `? more` has `blank_before: true` |

The real formatter leaves both sources unchanged — they are already canonical —
so this is a byte-identity break on a four-line file. The shape occurs in none of
the seven adversarial fixtures, none of the corpus, and neither the stdlib nor the
flagship, which is why no differential test can catch it: **add
`tests/fmt_adversarial/doc_run_interior_comment.tmc` carrying both shapes as part
of this task.**

Fixing it means letting comment tokens reach the doc-run items — a BODY change in
`syntax::extract`, which the standing visibility-only ruling does not cover.
It is in scope here for the same reason `prev_end_line` was in scope in Task 2:
a field the compiler path could ignore becomes observable the moment `fmt` reads
it. `reduce_doc_run` treats a comment item as inert, so the reduced `Doc` the
compiler consumes is unaffected — verify that rather than assuming it, and keep
both differential oracles green.

Note the two surfaces differ, and a fix that handles one silently misses the
other: for NAMESPACE and STATE the comment is a direct child token before the
`{`, so `trivia::units(&node)` hands it back as the body's FIRST unit while the
formatter keeps it above the keyword; for ALPHABET — and for MACHINE/REUSE, where
it sits in the declaration's stream ahead of `WORLD` — nothing picks it up at all.

An alphabet's elements come from the existing extraction path, not from a new
walk: reuse whatever `syntax::extract`'s alphabet helper already builds, and
widen its visibility rather than duplicating the element decode. Same for a
`use` declaration's paths.

- [ ] **Step 4: Run it to verify it passes**

Run: `cargo test -p mtc-turing-machine --lib fmt::print`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/turing-machine/src/fmt
git commit -m "feat(turing-machine): a green .tmc printer for the file skeleton"
```

---

## Task 4: reuse, machine, world — signatures, tapes, grafts, binds

**Files:**
- Modify: `crates/turing-machine/src/fmt/print.rs`

**Interfaces:**
- Consumes: Task 3's skeleton; `ReuseView`, `MachineView`, `WorldView`,
  `TapeView`, `GraftView`, `BindView`, `SigParamView`, `BindingArgView`.
- Produces: rendering for `routine`/`graph`/`machine` headers and bodies, tape
  declarations with their name-column alignment, grafts and binds. States are
  Task 5 — a world containing one may render its states however is convenient
  until then, as long as no test asserts on it.

- [ ] **Step 1: Write the failing tests**

Add to `print.rs`'s test module:

```rust
    #[test]
    fn worlds_tapes_grafts_and_binds_agree() {
        agrees("alphabet ab { '_' }\nmachine {\n  tape main: ab;\n}\n");
        agrees("alphabet ab { '_' }\nmachine {\n  tape m: ab;\n  tape longer: ab;\n  volatile tape x: ab;\n}\n");
        agrees("alphabet ab { '_' }\nmachine { // open\n  tape main: ab; // trailing\n} // close\n");
        agrees("alphabet ab { '_' }\nnamespace n {\n  routine r(tape t: ab) {\n  }\n}\n");
        agrees("alphabet ab { '_' }\nnamespace n {\n  export graph g(tape t: ab, state done) {\n  }\n}\n");
        agrees("alphabet ab { '_' }\nnamespace n {\n  export routine r(\n    tape t: ab writes { '_' }\n  ) {\n  }\n}\n");
        agrees("alphabet ab { '_' }\nnamespace n {\n  graph g(tape t: ab, state d) {\n  }\n}\nmachine {\n  tape main: ab;\n  entry graft n::g(t = main, done = fin) as inst; // trailing\n  bind n::g(t = main, done = fin) as other;\n}\n");
        agrees("alphabet ab { '_', 'a' }\nnamespace n {\n  graph g(tape t: ab, state d) {\n  }\n}\nmachine {\n  tape main: ab;\n  bind n::g(t = main with map { '_' -> '_', 'a' -> 'a' }, d = fin) as one;\n}\n");
    }
```

Every one of these must parse — run them through the real parser as you write
them, and correct the FIXTURE when it does not. A world with no states is
accepted by the parser; if a case above turns out not to be, add a minimal
state and say so in your report.

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p mtc-turing-machine --lib fmt::print`
Expected: FAIL — the new shapes are unhandled.

- [ ] **Step 3: Write the implementation**

**Two comment positions `trivia::units` cannot reach on its own — measured in
Task 2, and each one is a DROPPED COMMENT if unhandled.**

1. A comment between a `machine`/`routine`/`graph` header and its brace —
   `machine /* x */ {` — sits in the **declaration's** stream, not `WORLD`'s,
   because `WORLD` opens at the `{`. `units(&world)` therefore never sees it,
   while C1 relocates it into the body. Reach back from `WORLD` to its parent's
   element stream for the comment tokens between the header's last token and the
   `{`, and render them where C1 does. Add a differential case for it.
2. A comment written INSIDE a `;`-terminated declaration — `tape main /* c */:
   ab;` — is relocated by C1 to after the `;`, because `Parser::take_trailing`
   tests `sig_index <= pos`. It lives inside the node, so it must be found while
   rendering that node rather than in the parent's stream. Add a differential
   case for it here, and one for a `graft`/`bind` in the same shape.

`render_reuse` and `render_machine` ask `trivia::open_trailing` of the
**`WORLD`** node, and take their `Rendered::trailing` (C1's `close_trailing`)
from the unit, exactly as a `;`-terminated declaration does. `render_namespace`
and `render_alphabet` ask the declaration node itself. A `WorldView`'s items
come from `trivia::units(world.syntax(), &index)` — **not** from
`WorldView::tapes()/states()/grafts()/binds()`, which are per-kind filters and
lose source order.

`tape_name_widths` groups over the same unit list. Signature parameters and
binding arguments come from `extract`'s `reparse_sig_param` /
`reparse_binding_arg` path — widen visibility rather than re-decoding.

- [ ] **Step 4: Run to verify they pass**

Run: `cargo test -p mtc-turing-machine --lib fmt::print`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/turing-machine/src/fmt/print.rs
git commit -m "feat(turing-machine): green printing for worlds, tapes, grafts and binds"
```

---

## Task 5: states and rules — the block form and the grid

**Files:**
- Modify: `crates/turing-machine/src/fmt/print.rs`

**Interfaces:**
- Consumes: Task 4; `StateView`, `RuleView`, `WriteVecView`, `MoveVecView`,
  `TransitionView`; `extract::extract_rule`.
- Produces: `render_block_state`, `render_rule`, `render_rule_off_grid`,
  `grid_for` and the `breaks_the_grid` predicate, against views.

- [ ] **Step 1: Write the failing tests**

```rust
    #[test]
    fn block_states_and_the_grid_agree() {
        let head = "alphabet ab { '_', 'a', 'b' }\nmachine {\n  tape main: ab;\n";
        agrees(&format!("{head}  entry state s {{\n    ['b'] -> write ['a'] move [>] goto s;\n    ['a'] ->             move [>] goto s;\n    ['_'] -> stop;\n  }}\n}}\n"));
        agrees(&format!("{head}  entry state s {{ // open\n    // own-line\n    ['a'] -> stop; // trailing\n\n    ['_'] -> stop;\n  }} // close\n}}\n"));
        agrees(&format!("{head}  entry state s {{\n    [*] -> debugger goto s;\n    ['a'] -> write [{{0 + 1}}] move [.] stop;\n    ['_'] -> halt;\n  }}\n}}\n"));
        agrees(&format!("{head}  entry state s {{\n    [*] -> goto s;\n  }}\n  state z {{\n  }}\n}}\n"));
        agrees(&format!("{head}  ? documented\n  entry state s {{\n    [*] -> stop;\n  }}\n}}\n"));
    }
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p mtc-turing-machine --lib fmt::print`
Expected: FAIL.

- [ ] **Step 3: Write the implementation**

**`trivia::unclaimed_inside`'s RULE arm answers empty today, and filling it in is
this task's obligation — nothing will fail if you skip it.** Task 4 built that
helper for the `;`-terminated declarations it renders and left the RULE arm empty
because no rule was rendered yet. RULE is `;`-terminated like the others, and the
relocation is real. Measured:

```
[*] -> stop /* c */;      C1 prints:   [*] -> stop; /* c */
```

Green drops that comment unless the arm is filled in, and **no existing test
covers the shape**, so the suite stays green while a comment disappears. Fill the
arm, add a differential case for it here, and add one for a comment inside a rule
whose transition is a `call` — the longest form, where the pending comment has the
furthest to travel. Task 6 inherits this through inline states, which embed rules:
if a rule carrying such a comment is a candidate for a single-line state, check
what C1 does with it before assuming the inline path is safe.

A state's rule list is `trivia::units(state.syntax(), &index)`; the grid is
computed over the `RULE` units only, and own-line comments and blank lines
inside a state do **not** split the grid (a state is one table). A rule's
value is `extract_rule(&RuleView, &index)`, which already reproduces
`lower_cst`'s `Rule` and is oracle-tested; `write_cell_text` loses its
`tokens` parameter, and `subst_body_text` collapses to the substitution's own
node text — the `{`/`}` excluded, matching what C1 sliced from the span.

A zero-row state is valid and must render; do not special-case it into an
error.

- [ ] **Step 4: Run to verify they pass**

Run: `cargo test -p mtc-turing-machine --lib fmt::print`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/turing-machine/src/fmt/print.rs
git commit -m "feat(turing-machine): green printing for states, rules and the grid"
```

---

## Task 6: single-line state runs

**Files:**
- Modify: `crates/turing-machine/src/fmt/print.rs`

**Interfaces:**
- Consumes: Task 5.
- Produces: `inline_candidate`, `inline_state_runs`, `inline_state_line`,
  `render_inline_state` against views.

- [ ] **Step 1: Write the failing tests**

```rust
    #[test]
    fn single_line_state_runs_agree() {
        let head = "alphabet ab { '_', 'a' }\nmachine {\n  tape main: ab;\n";
        // A maximal run of adjacent inline-capable states shares one grid.
        agrees(&format!("{head}  entry state a {{ ['a'] -> goto b; }}\n  state b {{ ['_'] -> stop; }}\n}}\n"));
        // A blank line between two of them ends the run.
        agrees(&format!("{head}  entry state a {{ ['a'] -> goto b; }}\n\n  state b {{ ['_'] -> stop; }}\n}}\n"));
        // A doc run disqualifies a state from a run entirely.
        agrees(&format!("{head}  entry state a {{ ['a'] -> goto b; }}\n  ? doc\n  state b {{ ['_'] -> stop; }}\n}}\n"));
        // A comment on the brace, a trailing comment, or an own-line comment
        // inside the body each force the block form.
        agrees(&format!("{head}  entry state a {{ // c\n    ['a'] -> stop;\n  }}\n}}\n"));
        agrees(&format!("{head}  entry state a {{ ['a'] -> stop; }} // c\n}}\n"));
        // A member that would cross the line limit drops the WHOLE run to
        // block form — the case a per-state check would get wrong.
        agrees(&format!("{head}  entry state aaaaaaaaaaaaaaaaaaaaaaaaaaaaaa {{ ['a'] -> goto bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb; }}\n  state bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb {{ ['_'] -> stop; }}\n}}\n"));
    }
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p mtc-turing-machine --lib fmt::print`
Expected: FAIL — every state still renders in block form.

- [ ] **Step 3: Write the implementation**

The run-membership rule is unchanged from C1: a run is maximal over adjacent
inline-capable, **undocumented** states with no blank line between them, and
if any member would cross `LINE_WIDTH` the whole run falls back to block form.
Compute membership over the same `trivia::units` list Task 4 introduced, so a
comment unit between two states ends the run the way C1's `WorldKind::Comment`
item did.

`inline_candidate`'s exclusions carry over unchanged: a comment on the `{`,
any comment in the body, a rule with a trailing comment, a rule with an
interior comment in its binding list or map, and a rule off the grid. The
interior-comment half of that predicate has no implementation until Task 7 —
stub it to `false` ("no rule carries an interior comment yet") with a comment
saying so, and wire it in Task 7. Stubbing it to `true` would silently drop
states out of every run and the differential tests would not catch it, because
no test source in this task carries such a rule.

- [ ] **Step 4: Run to verify they pass**

Run: `cargo test -p mtc-turing-machine --lib fmt::print`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/turing-machine/src/fmt/print.rs
git commit -m "feat(turing-machine): green printing for single-line state runs"
```

---

## Task 7: interior list comments, everywhere

**Files:**
- Modify: `crates/turing-machine/src/fmt/print.rs`

**Interfaces:**
- Consumes: Task 6; `trivia::interior`.
- Produces: interior-comment rendering for all of `use` paths, `alphabet`
  bodies, signature parameter lists, the three binding lists
  (`call`/`graft`/`bind`), `with map` pair lists (keyed two levels deep), and
  the pattern / `write` / `move` vectors — plus the wiring of
  `inline_candidate`'s stubbed half from Task 6.

- [ ] **Step 1: Write the failing tests**

The census table's `interior` rows are the checklist; there are eight list
surfaces and the plan expects at least one case each, plus the two that are
easy to get wrong:

```rust
    #[test]
    fn interior_list_comments_agree_on_every_surface() {
        agrees("use // before\n  a::b, /* mid */\n  c::d\n  // before the semicolon\n;\n");
        agrees("alphabet ab {\n  // before\n  '_', /* after */\n  'a'\n  // before the closer\n}\n");
        agrees("alphabet ab { /* stays inline */ '_', 'a' }\n");
        let head = "alphabet ab { '_', 'a' }\n";
        agrees(&format!("{head}namespace n {{\n  graph g(\n    // before\n    tape t: ab,\n    state d\n    // before the closer\n  ) {{\n  }}\n}}\n"));
        agrees(&format!("{head}namespace n {{\n  graph g(tape t: ab, state d) {{\n  }}\n}}\nmachine {{\n  tape main: ab;\n  entry graft n::g(\n    // before\n    t = main,\n    d = fin\n  ) as i;\n  state fin {{ [*] -> stop; }}\n}}\n"));
        agrees(&format!("{head}namespace n {{\n  graph g(tape t: ab, state d) {{\n  }}\n}}\nmachine {{\n  tape main: ab;\n  bind n::g(t = main with map {{\n    // before the pair\n    '_' -> '_',\n    'a' -> 'a'\n  }}, d = fin) as o;\n  state fin {{ [*] -> stop; }}\n}}\n"));
        agrees(&format!("{head}machine {{\n  tape main: ab;\n  entry state s {{\n    [/* p */ *] -> write [/* w */ 'a'] move [/* m */ .] stop;\n  }}\n}}\n"));
        agrees(&format!("{head}machine {{\n  tape main: ab;\n  entry state s {{\n    [*] -> write [\n      // own-line takes the rule off the grid\n      'a'\n    ] stop;\n    ['a'] -> stop;\n  }}\n}}\n"));
    }

    /// The whole adversarial set, and the whole shipped corpus, through the
    /// differential oracle. This is the check that stops a green walk from
    /// being right about every shape someone remembered to write a test for.
    #[test]
    fn the_corpus_and_the_adversarial_set_agree() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let mut checked = 0;
        for dir in ["tests/golden", "src/stdlib", "../../docs/examples", "tests/fmt_adversarial"] {
            let full = root.join(dir);
            let mut paths: Vec<std::path::PathBuf> = std::fs::read_dir(&full)
                .unwrap_or_else(|e| panic!("{} is unreadable: {e}", full.display()))
                .map(|e| e.expect("a readable entry").path())
                .filter(|p| p.extension().and_then(|x| x.to_str()) == Some("tmc"))
                .collect();
            paths.sort();
            assert!(!paths.is_empty(), "{} contributed nothing", full.display());
            for path in paths {
                let src = std::fs::read_to_string(&path).expect("readable");
                let green = format_green(&src).expect("the green printer formats");
                let c1 = crate::fmt::format(&src).expect("the C1 printer formats");
                assert_eq!(green, c1, "{} formats differently", path.display());
                checked += 1;
            }
        }
        assert!(checked >= 15, "expected corpus plus adversarial, saw {checked}");
    }
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p mtc-turing-machine --lib fmt::print`
Expected: FAIL.

- [ ] **Step 3: Write the implementation**

**`trivia::interior` was never exercised on the two-level `map_pairs` surface
in Task 2** — do not assume that coverage exists. The nested case is this
task's to prove: a `with map { … }` carrying an interior comment, inside a
binding list that also carries one, with BOTH indices asserted by value.

Each surface hands `trivia::interior` the element stream strictly between its
delimiters — the `use` list is the one with none, running from the `use`
keyword's next element to the `;`. A `SYM_MAP`'s comments are keyed by
`(binding-arg index, pair index)`, so the walk is nested: for each
`BINDING_ARG`, for each `SYM_MAP` inside it. The copied `bucket`/`Interior`
machinery consumes exactly the `(index, Comment)` shape `interior` returns.

Preserve the documented rendering gap rather than fixing it: the five
`paren_list` / `with-map` surfaces break to multi-line on ANY interior
comment. It is unasserted on purpose so a future fix does not fail the
fixtures — do not add an assertion pinning it, and do not fix it here.

Wire `inline_candidate`'s stubbed half: a rule whose binding list or map
carries an interior comment, or which is off the grid, disqualifies its state.

- [ ] **Step 4: Run to verify they pass**

Run: `cargo test -p mtc-turing-machine --lib fmt::print`
Expected: PASS. This is the first point at which the green printer is
believed complete.

- [ ] **Step 5: Commit**

```bash
git add crates/turing-machine/src/fmt/print.rs
git commit -m "feat(turing-machine): green printing for interior list comments"
```

---

## Task 8: swap `format()` over, delete the C1 printer, run the gates

**Files:**
- Modify: `crates/turing-machine/src/fmt/mod.rs`, `crates/turing-machine/src/fmt/print.rs`
- Modify: `crates/turing-machine/tests/fmt_tmc.rs`

**Interfaces:**
- Produces: `fmt::format` delegating to `print::format`, the C1 printer gone,
  and `parse_cst` with zero production callers in the workspace.

- [ ] **Step 1: Capture the stdlib gate's "before"**

The compiled-stdlib byte-identity gate is the standing gate for any
text-only or formatter change, and the crate already owns the builder it
needs: `stdlib_object(OptLevel)` in
`crates/turing-machine/tests/stdlib_golden.rs` (mirrored as
`stdlib_object_bytes` in `tests/opt_equivalence.rs`). Add a TEMPORARY dumper
beside it — it is deleted at Step 5 of this task, and it is `#[ignore]`d so
it never runs in CI:

```rust
#[test]
#[ignore = "temporary: the compiled-stdlib byte-identity gate for the fmt swap"]
fn dump_stdlib_objects() {
    for (level, name) in [(OptLevel::O0, "O0"), (OptLevel::O1, "O1")] {
        std::fs::write(
            format!("/tmp/tm-std-{name}.bin"),
            stdlib_object(level).to_bytes(),
        )
        .expect("writable");
    }
}
```

```bash
cargo test -p mtc-turing-machine --test stdlib_golden dump_stdlib_objects -- --ignored
cp /tmp/tm-std-O0.bin /tmp/tm-std-O0.before.bin
cp /tmp/tm-std-O1.bin /tmp/tm-std-O1.before.bin
```

Do this BEFORE the swap. Step 4 re-runs it and byte-compares.

- [ ] **Step 2: Swap and delete**

`fmt/mod.rs` keeps `pub fn format(source: &str) -> Result<String, CompileError>`
delegating to `print::format`, plus its own unit tests. Delete every C1
printer function, the `Rendered`/`flush`/`Interior`/`Grid`/`paren_list`
machinery that now lives only in `print.rs`, and the `crate::cst::*` and
`parse_cst` imports. Rename `print::format_green` to `print::format` and drop
`pub(crate)` where it is no longer needed. Move the module-level printer
documentation from `mod.rs` to `print.rs`, updating the one sentence that says
the printer walks the CST.

The differential tests in `print.rs` now have no C1 side. **Do not delete
them** — convert each to a golden assertion by capturing the current output,
or move the whole list into `agrees_with_itself`-style idempotency plus the
existing token-preservation battery. Say which you chose and why; silently
dropping the corpus×adversarial check would remove this plan's only
comprehensive guard.

- [ ] **Step 3: Prove no production caller of the C1 path remains**

```bash
grep -rn "parse_cst\|lower_cst" crates/turing-machine/src --include='*.rs' \
  | grep -v "cfg(test)" | grep -v "^crates/turing-machine/src/parser/tests.rs"
```

Expected: only doc-comment mentions and the definitions themselves, plus
`parser.rs`'s `parse` seam. Read every surviving line; any that is a real call
from a production path is a bug in this task.

- [ ] **Step 4: Run every gate**

```bash
cargo test --workspace --no-fail-fast
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --check
git diff --stat master -- crates/core crates/post-machine     # must print nothing
```

Then the stdlib byte-identity gate:

```bash
cargo test -p mtc-turing-machine --test stdlib_golden dump_stdlib_objects -- --ignored
cmp /tmp/tm-std-O0.before.bin /tmp/tm-std-O0.bin
cmp /tmp/tm-std-O1.before.bin /tmp/tm-std-O1.bin
```

Both `cmp`s must be silent. A difference means the printer changed the
compiled stdlib — stop and report it; that is the gate this plan exists to
satisfy. Delete the temporary dumper once both compare clean, in the same
commit as Step 5.

- [ ] **Step 5: Commit**

```bash
git add crates/turing-machine
git commit -m "feat(turing-machine): tmt fmt prints from the green tree"
```

---

## Task 9: documentation

**Files:**
- Modify: `CLAUDE.md`, `docs/tmt/fmt.md`, `docs/lsp.md`

- [ ] **Step 1: Correct the idempotency claim**

`CLAUDE.md`'s fmt paragraph says fmt is "idempotent with one inherited
exception" and names only the `.pmc` labels/command case. That is false about
`.tmc`, which has its own — measured in this plan, pinned by Task 1's fixture.
Write the shared mechanism rather than two anecdotes: **a comment relocated
into a brace-delimited body re-parses as a comment on the opening brace, which
forces the body multi-line; the file settles on the second pass.** Name both
languages' shapes as instances.

Also update the same paragraph's `.tmc` clause — it currently reads "`.pmc`
prints from the green syntax tree (`syntax/`), `.tmc` still from its own
lossless CST". Both now print from the green tree.

- [ ] **Step 2: Update the architecture prose**

`CLAUDE.md`'s "The `.tmc` front end" section says `parse_cst` "survives past
the front in two roles: `fmt`'s actual input, and half of the differential
oracle" and that "Routing `fmt` off the C1 CST is later work". After this plan
it is the oracle only, in both crates. Say so, and say that the C1 CSTs
themselves are the next plan's subject — do not claim they are gone.

- [ ] **Step 3: Update `docs/tmt/fmt.md` and `docs/lsp.md`**

`docs/tmt/fmt.md`: any sentence describing the printer as walking a CST, and
the idempotency section, which must now state the exception. `docs/lsp.md`'s
`.tmc` Formatting row is currently marked `(CST only)` — correct it the way
the document-symbols row was corrected in plan 10.

While editing these files, migrate the `docs/tmt/fmt.md (interior comments)`
citations that no longer resolve **in the files this plan touches only** — the
opportunistic rule, not a sweep. Do not let this gate the task.

- [ ] **Step 4: Verify every claim you wrote**

For each sentence you added or changed, name the file and line that makes it
true, and check it. Twelve false documentation sentences shipped across this
arc's earlier plans, four of them written inside a fix for another — a claim
that "reads right" is exactly the kind that survives review.

- [ ] **Step 5: Commit**

```bash
git add CLAUDE.md docs
git commit -m "docs(turing-machine): tmt fmt runs the green tree"
```

---

## Exit criteria

- `cargo test --workspace --no-fail-fast` green; `cargo clippy --workspace
  --all-targets -- -D warnings` and `cargo fmt --check` clean.
- `git diff --stat master -- crates/core crates/post-machine` prints nothing.
- `parse_cst`/`lower_cst` have **zero production callers in the workspace** —
  PM's went in plan 6, TM's in Task 8 — and survive only as the differential
  oracles and inside `#[cfg(test)]` modules.
- The compiled stdlib is byte-identical at both opt levels, before and after.
- Every row of the trivia census has at least one adversarial source and at
  least one test that reads it.
- All four measured quirks still reproduce: the two keyword/name relocations,
  the eaten bracket space, the two-pass settle (pinned by value at passes 1,
  2 and 3), and the `;`-trailing-comment blank line — the last now because
  green AGREES with C1 rather than because fmt works around it.
- `CLAUDE.md`, `docs/tmt/fmt.md` and `docs/lsp.md` carry no sentence that
  describes `.tmc` fmt as reading a CST, and the idempotency exception is
  stated as one mechanism covering both languages.

## What this plan deliberately does not do

- **It does not fix the comment relocation.** The keyword/name relocation and
  its two-pass settle are behavior, reproduced under the byte-identity gate
  and pinned by a fixture. Fixing them is a separate, deliberate change, and
  it belongs with the systemic comment-preservation work already tracked —
  not mixed into the rewrite that proves itself by producing identical bytes.
- **It does not delete the C1 CSTs.** `cst.rs` in both crates, `parse_cst`,
  `lower_cst`, and both differential oracles survive this plan intact. Plan 12
  removes them and writes new tests wherever that drops coverage.
- **It does not touch the five-surface multi-line rendering gap.** Documented,
  deliberately unasserted, and preserved.
