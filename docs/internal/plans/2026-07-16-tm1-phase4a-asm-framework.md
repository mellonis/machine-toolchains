# TM-1 Phase 4a: Assembler-framework extensions — sections, tables, macros

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Extend the arch-agnostic assembler framework (`crates/core/src/asm/`)
with the three per-dialect opt-in mechanisms the `.tma` dialect needs
(spec §8.1): source **sections** (`.section code` / `.section tables`),
**table directives** (`.row [vec]`, `.targets L1, L2, …`, `.target L`), and
**repetition macros** (`.rept v, lo, hi … {expr} … .endr`), plus the
**vector operand** syntax (`[1, *, -, <, .]`) and a **table-reference operand
kind**. All of it is capability-gated so the `.pma` dialect's acceptance stays
byte-for-byte unchanged (dialect 0.3 contract), and all of it is tested
through a crate-private fake dialect — zero TM-1 knowledge enters core.

**Architecture:** Follows the existing framework shape: lexer→CST→lower→
assembler. New syntax is unlocked by an `AsmCaps` capability struct carried on
`ArchSyntax` (defaults = today's behavior). The lossless-CST contract holds:
macros are preserved as written (fmt never expands); expansion happens at
lower. Tables are built per function into a per-blob table blob (`ObjectFile.
table_blobs`), `TableRef` operands become `table_fixups`, and dispatch-table
entries hold **blob-relative code offsets** (dispatch targets are always
intra-routine — a state's targets are its own rule bodies; the cross-routine
case does not exist by construction, and the linker rebases in phase 5).
`.frame` is DEFERRED to phase 5 (frames emit belongs to the linker; the UTM
doesn't use it).

**Tech Stack:** Rust; no new dependencies.

## Global Constraints

- **`.pma` acceptance is byte-compatible**: with default caps, the lexer/CST/
  lower behave exactly as today — every existing pma/pmc test (incl.
  `asm_acceptance`, `cli_programs`, fmt idempotency, LSP) stays green
  untouched. `cargo test --workspace` at the end of every task.
- **Core stays arch-agnostic**: new mechanisms are exercised ONLY by a
  crate-private fake dialect in core's tests (like `test_arch`); no TM-1/PM-1
  names in the new core code.
- The lossless-CST contract: `parse → print` (fmt) reproduces semantics-
  preserving canonical text; macros print AS WRITTEN; `Raw` lines remain a
  hard error in lower (unchanged).
- Match-table bytes follow the layout the VM walks (`vm/table.rs`): width u8,
  row_count u16 LE, rows = row_count × width payload bytes, `0x7F` =
  wildcard/transparent. Dispatch tables: entry_count u16 LE + u32 LE entries.
  The assembler VALIDATES table discipline (exact rows first: sorted,
  pairwise disjoint; then wildcard rows; optional all-wildcard catch-all
  last); the VM trusts.
- `cargo clippy --workspace --all-targets -- -D warnings`, `cargo fmt --check`
  clean before every commit. No new deps. No spec-§ refs in code comments.
- Commit style: conventional with scope. NEVER add any Claude/AI attribution
  footer. Commits require the maintainer's explicit go-ahead in the executing
  session.

---

### Task 1: AsmCaps + opt-in lexer tokens (vectors, substitution)

**Files:**
- Modify: `crates/core/src/asm/syntax.rs` (add `AsmCaps` + field on `ArchSyntax`)
- Modify: `crates/core/src/asm/lexer.rs` (capability-gated token kinds)
- Modify: `crates/core/src/asm/cst.rs` (only the `parse_asm_cst_with` entry; default fn unchanged)
- Modify: `crates/post-machine/src/asm/mod.rs` (`pm1_syntax()` gains `caps: AsmCaps::default()` — one line)
- Test: inline tests in lexer.rs

**Interfaces:**
- Produces:

```rust
/// Per-dialect syntax capabilities. Defaults = the classic surface
/// (everything off) — the .pma dialect's acceptance is unchanged.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AsmCaps {
    /// `.section` regions, `.row`/`.targets`/`.target` directives.
    pub tables: bool,
    /// `.rept v, lo, hi` … `.endr` with `{expr}` substitution.
    pub rept: bool,
    /// `[a, *, -, <, >, .]` vector operand tokens.
    pub vectors: bool,
}
```

`ArchSyntax` gains `pub caps: AsmCaps`. Lexer: `lex_asm(source, caps)` (the
existing entry keeps its signature by taking caps from the caller chain —
follow how `parse_asm_cst` invokes the lexer and add
`parse_asm_cst_with(source, caps)`; the existing `parse_asm_cst(source)`
delegates with `AsmCaps::default()`).

New `AsmTokenKind` variants, emitted ONLY when the corresponding cap is on
(off → these characters remain `Junk`, preserving today's Raw-line behavior):
- vectors on: `LBracket`, `RBracket`, `Star`, `Dash`, `Lt`, `Gt`, `Dot`
  (`.` ONLY inside a bracketed vector context — simplest correct rule: the
  lexer tracks bracket depth; inside `[...]`, `.` `<` `>` `-` `*` lex as their
  tokens; outside, behavior is unchanged so `.func`-style words still lex as
  `Word`).
- rept on: `LBrace`, `RBrace` (substitution `{`/`}`), plus `LParen`,
  `RParen`, `Plus`, `Percent` INSIDE braces (track brace depth like brackets;
  `-` inside braces lexes as `Dash` for subtraction; `*` as `Star` for
  multiplication).

- [ ] **Step 1: Write the failing tests** (lexer.rs `mod tests`)

```rust
    #[test]
    fn default_caps_keep_symbols_junk() {
        let toks = lex_for_test("wr [1,*]", AsmCaps::default());
        assert!(toks.iter().any(|t| matches!(t.kind, AsmTokenKind::Junk('['))));
    }

    #[test]
    fn vector_caps_tokenize_brackets_and_markers() {
        let caps = AsmCaps { vectors: true, ..Default::default() };
        let kinds = kinds_for_test("wr [1, *, -, <, >, .]", caps);
        assert!(kinds.contains(&AsmTokenKind::LBracket));
        assert!(kinds.contains(&AsmTokenKind::Star));
        assert!(kinds.contains(&AsmTokenKind::Dash));
        assert!(kinds.contains(&AsmTokenKind::Lt));
        assert!(kinds.contains(&AsmTokenKind::Gt));
        assert!(kinds.contains(&AsmTokenKind::Dot));
        assert!(kinds.contains(&AsmTokenKind::RBracket));
    }

    #[test]
    fn dot_outside_vectors_still_words() {
        let caps = AsmCaps { vectors: true, rept: true, tables: true };
        let toks = lex_for_test(".section tables", caps);
        assert!(matches!(&toks[0].kind, AsmTokenKind::Word(w) if w == ".section"));
    }

    #[test]
    fn rept_caps_tokenize_substitution() {
        let caps = AsmCaps { rept: true, ..Default::default() };
        let kinds = kinds_for_test("Linc{(v+1)%127}", caps);
        assert!(kinds.contains(&AsmTokenKind::LBrace));
        assert!(kinds.contains(&AsmTokenKind::LParen));
        assert!(kinds.contains(&AsmTokenKind::Plus));
        assert!(kinds.contains(&AsmTokenKind::Percent));
        assert!(kinds.contains(&AsmTokenKind::RBrace));
    }
```

(Write the two tiny local helpers `lex_for_test`/`kinds_for_test` around the
real lexer entry — mirror how existing lexer tests drive it.)

- [ ] **Step 2: Verify they fail** (no `AsmCaps`).

- [ ] **Step 3: Implement.** Keep the default path literally the old code
path (guard new match arms on the caps + depth trackers). `pm1_syntax()` sets
`caps: AsmCaps::default()`.

- [ ] **Step 4:** `cargo test --workspace` — every pma/pmc suite green
(byte-compat), new lexer tests green. clippy + fmt.

- [ ] **Step 5: Commit**

```bash
git add crates/core crates/post-machine
git commit -m "feat(core): AsmCaps — capability-gated lexer tokens for vectors and substitution"
```

---

### Task 2: CST nodes — sections, table directives, rept blocks (lossless)

**Files:**
- Modify: `crates/core/src/asm/cst.rs`
- Test: inline tests in cst.rs

**Interfaces:**
- Produces new `AsmItemKind` variants (shaped ONLY when the caps allow;
  otherwise the old shaping applies unchanged):

```rust
    /// `.section NAME` (caps.tables)
    Section(SectionCst),
    /// `.row [..]` / `.targets L1, ..` / `.target L` (caps.tables)
    TableDirective(TableDirectiveCst),
    /// `.rept v, lo, hi … .endr` (caps.rept) — body kept AS WRITTEN.
    Rept(ReptCst),
```

```rust
pub struct SectionCst { pub name: String, pub span: (usize, usize), pub trailing: Option<TrailingComment> }

pub enum TableDirectiveKind { Row, Targets, Target }
pub struct TableDirectiveCst {
    pub labels: Vec<LabelCst>,          // `Tfetch: .row [..]`
    pub kind: TableDirectiveKind,
    /// Row: the vector elements; Targets/Target: label names.
    pub operands: Vec<OperandToken>,    // raw text slices, lossless
    pub span: (usize, usize),
    pub trailing: Option<TrailingComment>,
}

pub struct ReptCst {
    pub var: String,                    // `v`
    pub lo: i64,
    pub hi: i64,
    pub body: Vec<AsmItem>,             // shaped recursively, AS WRITTEN
    pub span: (usize, usize),
    pub trailing: Option<TrailingComment>,
}
```

Shaping rules: `.section` / `.row` / `.targets` / `.target` / `.rept` /
`.endr` recognized as leading Words when the cap is on (the `.func`
special-case at cst.rs ~163 is the pattern to mirror); `.endr` without an
open `.rept` and an unterminated `.rept` are shape errors surfaced the way
malformed `.func` is (study that path and mirror). Vector operands (bracketed
token runs) are captured into a single `OperandToken` whose `text` is the
verbatim `[..]` slice (lossless; parsing happens at lower). Nested `.rept` is
NOT supported: shape it as an error (`Malformed`-style AsmError at lower or a
Raw-degradation — pick the error path consistent with unterminated-rept, and
say which in the code comment).

- [ ] **Step 1: Write the failing tests**

```rust
    fn caps_all() -> AsmCaps { AsmCaps { tables: true, rept: true, vectors: true } }

    #[test]
    fn shapes_sections_and_table_directives() {
        let src = ".section tables\nTfetch: .row [1, *, *]\nDfetch: .targets A, B\n.section code\n";
        let cst = parse_asm_cst_with(src, caps_all());
        assert!(matches!(&cst.items[0].kind, AsmItemKind::Section(s) if s.name == "tables"));
        assert!(matches!(&cst.items[1].kind, AsmItemKind::TableDirective(d)
            if matches!(d.kind, TableDirectiveKind::Row) && d.labels[0].name == "Tfetch"));
        assert!(matches!(&cst.items[2].kind, AsmItemKind::TableDirective(d)
            if matches!(d.kind, TableDirectiveKind::Targets) && d.operands.len() == 2));
        assert!(matches!(&cst.items[3].kind, AsmItemKind::Section(s) if s.name == "code"));
    }

    #[test]
    fn rept_body_is_kept_verbatim() {
        let src = ".rept v, 0, 2\nLinc{v}: nop\n.endr\n";
        let cst = parse_asm_cst_with(src, caps_all());
        let AsmItemKind::Rept(r) = &cst.items[0].kind else { panic!("not a rept") };
        assert_eq!((r.var.as_str(), r.lo, r.hi), ("v", 0, 2));
        assert_eq!(r.body.len(), 1); // one line, unexpanded
    }

    #[test]
    fn default_caps_shape_unchanged() {
        // The same source under default caps: every unknown-directive line
        // becomes Raw (via Junk) or a Line, exactly as before this task.
        let cst = parse_asm_cst(".section tables\n");
        assert!(!matches!(&cst.items[0].kind, AsmItemKind::Section(_)));
    }
```

- [ ] **Step 2: Verify they fail.**

- [ ] **Step 3: Implement** (shaping + the two error paths). Losslessness:
every new node stores verbatim operand text + spans + trailing comments,
mirroring `LineCst`.

- [ ] **Step 4:** `cargo test --workspace` green. clippy + fmt.

- [ ] **Step 5: Commit**

```bash
git add crates/core
git commit -m "feat(core): lossless CST nodes for sections, table directives, and rept blocks"
```

---

### Task 3: Macro expansion at lower — the {expr} evaluator

**Files:**
- Create: `crates/core/src/asm/subst.rs` (the evaluator + textual substitution — pure, unit-testable)
- Modify: `crates/core/src/asm/lower.rs` (Rept expansion before item lowering)
- Modify: `crates/core/src/asm/mod.rs` (module decl; new AsmErrorKind variants: `BadRept`, `BadSubstitution`)
- Test: subst.rs inline + lower tests

**Interfaces:**
- `subst.rs` produces:

```rust
/// Evaluate a substitution expression over the loop variable.
/// Grammar: expr := mul (('+'|'-') mul)* ; mul := atom (('*'|'%') atom)* ;
/// atom := var | integer | '(' expr ')'. i64 arithmetic; '%' is Rust `%`
/// (operands are non-negative in practice; a negative result is an error).
pub(crate) fn eval_expr(text: &str, var: &str, value: i64) -> Result<i64, String>;

/// Replace every `{expr}` occurrence in `text` with the evaluated decimal.
/// Unbalanced braces or an eval error surface as Err.
pub(crate) fn substitute(text: &str, var: &str, value: i64) -> Result<String, String>;
```

- Lower: when it meets `AsmItemKind::Rept`, it iterates `value in lo..=hi`
  and, per iteration, applies `substitute` to every word, label name, and
  operand text of the body items, then lowers the substituted items as if
  they had been written inline (positions/spans point at the original body
  for diagnostics). `lo > hi` → `BadRept`. The expansion result feeds the
  SAME per-item lowering used everywhere else — no duplicate lowering logic.

- [ ] **Step 1: Write the failing tests** (subst.rs)

```rust
    #[test]
    fn evaluates_the_utm_expression() {
        assert_eq!(eval_expr("(v+1)%127", "v", 126).unwrap(), 0);
        assert_eq!(eval_expr("(v+1)%127", "v", 5).unwrap(), 6);
        assert_eq!(eval_expr("v", "v", 42).unwrap(), 42);
        assert_eq!(eval_expr("v*2+1", "v", 3).unwrap(), 7);
    }

    #[test]
    fn substitutes_all_occurrences() {
        assert_eq!(substitute("Linc{v}: wr {v+1}", "v", 9).unwrap(), "Linc9: wr 10");
    }

    #[test]
    fn errors_are_reported() {
        assert!(eval_expr("v+", "v", 0).is_err());
        assert!(eval_expr("w", "v", 0).is_err());       // unknown var
        assert!(substitute("{v", "v", 0).is_err());     // unbalanced
    }
```

Plus a lower-level test (in lower.rs tests, using a minimal fake syntax with
`caps.rept`): a `.rept v, 0, 2` around `nop` lowers to three instructions;
a rept around a labeled line yields three distinct labels.

- [ ] **Step 2: Verify they fail.**

- [ ] **Step 3: Implement** (recursive-descent evaluator ~60 lines; textual
substitution; the lower hook).

- [ ] **Step 4:** `cargo test --workspace` green. clippy + fmt.

- [ ] **Step 5: Commit**

```bash
git add crates/core
git commit -m "feat(core): rept macro expansion with {expr} substitution at lower"
```

---

### Task 4: Tables in the assembler — blobs, fixups, discipline

**Files:**
- Modify: `crates/core/src/vm/arch.rs` (`OperandKind::TableRef` — a u32 LE absolute table-section offset; fetch decode + `encode_operand` arm)
- Modify: `crates/core/src/asm/lower.rs` + `assembler.rs` (vector-operand parsing; section state; table building)
- Modify: `crates/core/src/asm/mod.rs` (AsmErrorKind: `BadVector`, `BadTable`, `TableDiscipline`, `UnknownTableLabel`)
- Test: assembler tests with a crate-private fake dialect (`caps` all on; a
  fake syntax whose entries include a `mtc`-like TableRef mnemonic, a
  `djmp`-like TableRef mnemonic, a `wr`-like vector mnemonic — names must be
  NEUTRAL, e.g. `tmatch`/`tdispatch`/`vwrite`, proving zero TM-1 knowledge)

**Interfaces / semantics:**
- **Vector operands**: a bracketed vector parses per element into
  `u32 payload | Wildcard(*) | Keep(-) | MoveLeft(<) | MoveRight(>) | Stay(.)`.
  Which elements are legal depends on the consuming context: match rows allow
  payload/wildcard; write vectors payload/keep; move vectors `<`/`>`/`.`.
  Wildcard and Keep both encode as `0x7F`; moves encode stay=0/left=1/right=2
  (the vm/table + spec conventions). The generic lower produces a
  `SourceOperand::Vector(Vec<VecElem>)`; the per-mnemonic encoding remains
  the dialect's job in 4b — in 4a the FAKE dialect exercises the plumbing.
- **Sections**: default section = code (so cap-off dialects never notice).
  `.section tables` switches; functions are legal only in code; table
  directives only in tables. Violations → `BadTable`.
- **Table building** (per object, since table labels are file-scoped like the
  UTM uses them): each labeled run of `.row`s builds ONE match table (width =
  the rows' common element count; differing widths → `TableDiscipline`);
  `.targets`/`.target` runs build dispatch tables whose entries are CODE
  labels resolved AFTER function layout to **blob-relative offsets** (targets
  must live in exactly one function; referencing labels across functions →
  `UnknownTableLabel`). Discipline validation on match tables: exact rows
  (no wildcard) must come first, sorted lexicographically by payload vector
  and pairwise disjoint; wildcard rows follow in source order; an all-wildcard
  row may only be last. Violations → `TableDiscipline` with the row's span.
- **Emission**: the object's `table_blobs[blob]` gets the concatenated tables
  built from that function's dispatch references… — NO: simpler and correct:
  table blobs are PER-OBJECT single-blob in 4a? They are per-blob in MO. Rule:
  every table (match or dispatch) is attributed to the ONE function whose code
  references it (via TableRef operands or `.targets` labels); a table
  referenced by no function or by two functions → `BadTable` (phase-5
  composition owns sharing). The function's `table_blobs` entry concatenates
  its tables in source order; each `TableRef` operand hole becomes a
  `TableFixup { blob, offset, table_offset }` with `table_offset` = the
  table's offset within that blob's table blob. Dispatch entries are written
  as u32 LE blob-relative code offsets (known post-relaxation).
- `assemble()` keeps its signature; the extra outputs ride the ObjectFile
  fields that already exist (`table_blobs`, `table_fixups`).

- [ ] **Step 1: Write the failing tests** (assembler tests; fake dialect)

Real test bodies required — the shape:

```rust
    fn fake_syntax() -> ArchSyntax { /* neutral mnemonics:
        tmatch (TableRef), tdispatch (TableRef), vwrite (SymbolVec-for-now…
        see note), nop/stp/ent as usual; caps all on */ }

    #[test]
    fn builds_match_table_and_fixup() {
        let src = "\
.section tables
T0: .row [1, 2]
    .row [1, *]
.section code
.func main
    tmatch T0
    stp
";
        let obj = assemble(&fake_syntax(), 0x7E, src, false).unwrap();
        let tables = obj.table_blobs.as_ref().unwrap();
        // width 2, 2 rows: [2, 2, 0, 1, 2, 1, 0x7F]
        assert_eq!(tables[0], vec![2, 2, 0, 1, 2, 1, 0x7F]);
        assert_eq!(obj.table_fixups.len(), 1);
        assert_eq!(obj.table_fixups[0].table_offset, 0);
    }

    #[test]
    fn dispatch_entries_are_blob_relative_code_offsets() { /* .targets over
        two labels inside main; assert the dispatch blob decodes to the two
        instruction offsets the assembler laid out (derive them from the
        encoding: ent byte + instruction sizes) */ }

    #[test]
    fn discipline_violations_are_rejected() { /* unsorted exact rows;
        wildcard before exact; catch-all not last; differing widths —
        each yields TableDiscipline */ }

    #[test]
    fn table_in_code_section_rejected() { /* .row outside .section tables */ }
```

(Derive the exact expected bytes/offsets by hand in comments, the
derivation-first convention.)

- [ ] **Step 2: Verify they fail.**

- [ ] **Step 3: Implement.** `OperandKind::TableRef` in core arch (fetch:
4-byte LE u32, like RelI32 but unsigned absolute; `encode_operand` arm;
extend the operand-codec property tests). Then the lower/assembler work per
the semantics above.

- [ ] **Step 4:** `cargo test --workspace` green (esp. operand_codec property
tests + all pma suites untouched). clippy + fmt.

- [ ] **Step 5: Commit**

```bash
git add crates/core
git commit -m "feat(core): assembler builds table blobs with fixups and discipline validation"
```

---

### Task 5: fmt + disassembler awareness

**Files:**
- Modify: `crates/core/src/asm/fmt.rs` (Section/TableDirective/Rept rendering — macros AS WRITTEN; grid columns for directives)
- Modify: `crates/core/src/asm/disassembler.rs` (render a `table_blobs`-bearing object's tables back as `.section tables` + `.row`/`.targets` directives before the code section; `disassemble_object` only — executables carry tables in phase 4b's link path)
- Test: fmt idempotency tests over section/table/rept sources (with caps); a dis round-trip test on the task-4 fake-dialect object

- [ ] **Step 1: Failing tests** — fmt: `format(format(x)) == format(x)` on a
source using all three mechanisms; the formatted output preserves the `.rept`
block unexpanded. Dis: assemble the task-4 sample, disassemble, assert the
listing contains `.section tables`, a `.row [1, 2]` line, and the `tmatch T0`
reference by label (the disassembler regenerates table labels `T0, T1, …` in
blob order — document the naming rule in a comment).

- [ ] **Step 2: Verify they fail.**

- [ ] **Step 3: Implement.** fmt renders new nodes with the existing column
grid; `.endr` alignment matches `.rept`. Disassembler decodes each table blob
back into rows (width from the header byte; `0x7F` renders as `*`) and
dispatch tables into `.targets` label lists (labels synthesized from entry
offsets; entries that don't land on an instruction boundary render as raw
offsets with a comment — defensive, should not happen for assembler output).

- [ ] **Step 4:** `cargo test --workspace` green (fmt idempotency for pma
unchanged). clippy + fmt.

- [ ] **Step 5: Commit**

```bash
git add crates/core
git commit -m "feat(core): fmt and disassembler understand sections, tables, and rept blocks"
```

---

### Task 6: Negative/property coverage + phase-4a gate

**Files:**
- Modify: `crates/core/tests/format_roundtrips.rs` or a new `crates/core/tests/asm_tables.rs` (integration home for the fake-dialect assembly round-trips)
- Test-only.

- [ ] **Step 1:** Property test: arbitrary well-formed match tables (width
1..=4, rows 1..=8 with random payloads < 0x7F, exact rows deduped+sorted by
the generator) assemble → the emitted table blob byte-equals the
independently-derived encoding; plus never-panic on adversarial `.tma`-ish
sources (random directive soup with caps on — `assemble` returns Err, never
panics).

- [ ] **Step 2:** Full gate: `cargo test --workspace` ·
`cargo clippy --workspace --all-targets -- -D warnings` · `cargo fmt --check` ·
`git status --short crates/post-machine/tests/golden/` (empty).

- [ ] **Step 3: Commit**

```bash
git add crates/core
git commit -m "test(core): property and adversarial coverage for the table-assembly framework"
```

---

## Self-review notes (spec → plan coverage)

- Spec §8.1 mechanisms 1–3: sections (T2/T4), table directives (T2/T4/T5),
  `.rept` with `{expr}` (T1/T2/T3, evaluator covers the UTM's `(v+1)%127`).
  `.frame` deliberately deferred to phase 5 (frames emit is the linker's;
  noted in the plan header).
- Capability gating preserves the `.pma` 0.3 acceptance contract (T1 lock
  tests + the whole existing suite as the gate); `parse_asm_cst` keeps its
  signature, `_with` variant added — pma/pmc LSP untouched.
- Table encoding matches `vm/table.rs` exactly (the walk is the normative
  reader); discipline validation implements the assembler-verifies/VM-trusts
  contract.
- Dispatch entries as blob-relative intra-routine offsets is the phase-4
  simplification (targets are a state's own rule bodies by construction);
  the table→code rebase record belongs to phase 5's composition engine —
  recorded here so 4b's link-lite knows the assumption.
- Fake neutral dialect keeps core provably arch-agnostic (mirrors `test_arch`).
- 4b (TM-1 crate, tmt CLI, UTM) is a separate follow-up plan.
