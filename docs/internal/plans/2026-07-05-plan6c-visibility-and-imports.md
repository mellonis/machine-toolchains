# Plan 6c — Symbol Visibility & Imports Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Private functions and edit-time external checking, before Plan 7 writes the stdlib: a **Local** symbol kind in `MO` (defined-but-not-exported), `.pmc` **nested function definitions** (visibility-only, post-machine-js semantics), **hidden-by-default visibility** with an **`export`** modifier (user ruling: least-privilege default; `main` auto-exports), plus **`use`** import declarations that let the compiler warn on calls to undeclared externals (IDE-grade "undefined identifier" signal without breaking §3.3's no-boilerplate linking), and **open `namespace` blocks** (nestable, reopenable, multiple per module; qualified `use ns::name [as alias]` and absolute `@ns::name()` calls are the only qualified-name surfaces — the stdlib will ship as `namespace std`).

**Architecture:** Visibility is enforced at the symbol layer, not by name tricks: `SymbolDef` gains `Local { blob }` (wire kind 2, object format version 2), and `resolve.rs` binds relocations to local symbols **directly within their object** — locals never enter the global namespace, so they can neither shadow nor be shadowed (this also closes the practical footgun of the 6b interposition ruling for library helpers). The `.pmc` front end flattens nested definitions with dot-mangled names (`outer.inner`), all implicitly local; top-level functions are LOCAL BY DEFAULT and exported only via the `export` modifier (`main` always exports; the keyword is `export`, not `external` — External already means "import" in the MO vocabulary); both `export` and `use` are **contextual keywords** (never reserved — `export() {…}` is still a function named export). The `.pma` level stays explicit and C-like: `.func name` = exported, `.func name local` = local; the language owns the default, the assembly mirrors the object model. Imports feed compile warnings only; a strictness flag is Plan 7 CLI territory.

**Tech Stack:** Rust edition 2024, no new dependencies. Baseline: 268 workspace tests green at master/5f6e78b.

## Spec deltas (controller applies on plan approval, before Task 1)

1. **§3 language:** nested function definitions (`outer() { inner() { … } … @inner(); }`) — flat code, scoped callability: an inner function is callable from its parent's body and deeper contexts only; call resolution is local-first, then outward, then top-level, then external. Nested functions are always local. **Top-level functions are local by default**; `export name() {…}` makes a function visible to the linker's namespace. `main` is always exported — un-namespaced top level ONLY (a namespaced `main` is an ordinary function, see delta 7; `export main` is legal and redundant); `export` on a nested definition is an error. `export` and `use` are contextual keywords, not reserved words.
2. **§3.3 rules:** calling an undefined function still links via an external symbol, but the compiler now WARNS unless the name is declared with `use` (bare form `use name[, name…];`) or called fully qualified (`@ns::name()`, self-declaring). Task 6 generalizes `use` to `::` paths with `as` aliases and namespace-block scoping; the bare form is the path-length-1 degenerate case, meaning unchanged. Unused imports and unused (unexported, unreached) functions also warn. Errors instead of warnings are a CLI-level strictness choice (Plan 7).
3. **§6.2 `.pmo`:** symbol kinds are Defined (1) / External (0) / **Local (2)**; object format version bumps to **2** (readers accept 1–2; MX/MT stay at 1 via per-container version constants).
4. **§6.4 `.pma`:** `.func name local` declares a local function; object disassembly prints it. Symbol names (`.func`, call/jump operands) accept `::`-separated segments of dotted identifiers (`std::api.helper`); LABELS remain colon-free (label-grammar soundness).
5. **§9 linker:** local symbols never enter the resolution namespace — bound directly within their object; a local name may repeat across objects freely; calling another object's local is an unresolved-symbol error. Shadowing/interposition is now an OPT-IN property of exported names (§9's "user definitions shadow stdlib" applies to exports; stdlib helpers stay unexported and are structurally unshadowable). Unreached locals are silently omitted, never reported in `LinkReport.dropped`.
6. **§7 IR:** `IrFunction` gains `local: bool`; nested names arrive pre-mangled; IR JSON version bumps to **3**.

## Global Constraints

- Contextual-keyword disambiguation, exactly: at top level, `use` followed by an identifier begins an import declaration; `export` followed by an identifier begins an exported function definition; otherwise both parse as ordinary identifiers (so `use() {…}` and `export() {…}` remain valid function definitions); `namespace` followed by an identifier then `{` begins a namespace block (`namespace() {…}` is a function named namespace); `as` is contextual inside `use` declarations only (`as() {…}` is a function named as). Inside a body, a non-reserved identifier followed by `(` `)` `{` begins a nested definition; `export` on a nested definition is an error ("nested functions are always local").
- Visibility, exactly: top-level default = Local symbol; `export` (or being the un-namespaced top-level `main`) = Defined symbol. Sanctioned test updates (visibility flip fallout, enumerated in Task 3): multi-file `.pmc` tests gain `export` on shared functions; `LinkReport.dropped` assertions for unexported `.pmc` functions become empty (locals are silently omitted). LINKED EXECUTABLE BYTES DO NOT CHANGE anywhere — only object symbol kinds.
- Mangling, exactly: NESTING joins with `.` (`outer.inner`); NAMESPACES join with `::` and end with `::` before the function part (`std::api`, `a::b::f.g` = namespaces a,b, function f, nested g) — the separator split makes every symbol SELF-DECOMPOSING (namespace part before the last `::`, function-nesting part after), cancelling the v2 map-structure item. `.pmc` identifiers can contain neither `.` nor `:`, so mangled names cannot collide with user-written names. `.pma`: labels stay colon-free (label-scan soundness); `.func` names and call/jump operands use a symbol-name rule accepting `::`-separated dotted segments.
- Resolution, exactly: a call `@n()` resolves to the innermost enclosing scope defining `n`, then outward, then top-level, then stays external. Duplicate names in the SAME scope are errors; shadowing an outer name is legal. Nested definitions may appear ANYWHERE in the body and are visible throughout it (hoisted — the flatten pass builds each scope map from all of a function's nested defs before resolving its body), matching both top-level declaration-order freedom and post-machine-js's order-independent subroutine keys. `.`-path calls (`@main.walk()`) are deliberately NOT syntax — nested-function names stay compiler-internal, visibility unbypassable. `::`-QUALIFIED calls ARE syntax (user ruling, Task 6): `@std::goToEnd()` — `::` segments only (a qualified path can never name a nested function), resolved ABSOLUTELY (the path is the full symbol; scope chain skipped), self-declaring (no undeclared-external warning; bare externals still warn without `use`).
- Locals and the linker: locals bypass the namespace (direct intra-object binding); they are never reported in `LinkReport.dropped` (namespace-based; documented); unreached locals are silently omitted.
- Wire compatibility: object READER accepts versions 1 and 2; writer stamps 2. `Executable`/`TapeBlockFile` keep version 1 untouched.
- `-O0` output for programs using none of the new syntax stays bit-identical; all existing goldens (flagship 7B, spec-sample 18→14, tail-merge 7→6, 13-byte tail-call, resource-trap pins) must not move.
- Gates per task: `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo fmt --check`. Commits path-scoped, never push, no attribution footers. BLOCK with derivation on golden mismatch; never adjust numbers to observed output.

## File Structure

- **Task 1 (core):** `crates/core/src/formats/object.rs` (Local kind, version 2), `crates/core/src/linker/resolve.rs` (direct binding), `crates/core/src/linker/mod.rs` (doc).
- **Task 2 (core):** `crates/core/src/asm/parser.rs` (`.func … local`, dotted idents), `crates/core/src/asm/assembler.rs`, `crates/core/src/asm/disassembler.rs`.
- **Task 3:** `crates/post-machine/src/parser.rs` (nested defs, `export`), `crates/post-machine/src/compiler.rs` (flatten pass + error kinds), `crates/post-machine/src/ir.rs` (IrFunction.local, IR v3), `crates/post-machine/src/codegen.rs`, `crates/post-machine/tests/compile_programs.rs` (version assert 3).
- **Task 4:** `crates/post-machine/src/parser.rs` (`use`), `crates/post-machine/src/compiler.rs` (import warnings).
- **Task 5:** goldens in `crates/post-machine/tests/` (new `visibility_programs.rs`).
- **Task 6:** `crates/post-machine/src/lexer.rs` (ColonColon token), `parser.rs` (namespace blocks, qualified `use` + `as`), `compiler.rs` (namespace scope chain, import bindings), `tests/visibility_programs.rs` (namespace goldens incl. reopening + interposition-by-injection).

---

### Task 1: `SymbolDef::Local` + direct linker binding

**Files:**
- Modify: `crates/core/src/formats/object.rs`, `crates/core/src/linker/resolve.rs`, `crates/core/src/linker/mod.rs` (LinkReport.dropped doc note)

**Interfaces:**
- Produces: `SymbolDef::Local { blob: u32 }` (wire kind 2); `OBJECT_FORMAT_VERSION: u16 = 2` (reader accepts 1..=2); resolve.rs binds relocations whose own-object symbol is `Local` directly to `(same object, blob)`, bypassing the namespace; locals never enter the namespace or `dropped`.

- [ ] **Step 1: object.rs.** Add the variant and version constant:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SymbolDef {
    Defined { blob: u32 },
    /// Defined but NOT exported: bound directly within its own object,
    /// invisible to cross-object resolution (spec §6.2 kind 2, §9).
    Local { blob: u32 },
    External,
}
```

```rust
/// MO format version within epoch 0x01. v2 added symbol kind 2 (Local).
pub const OBJECT_FORMAT_VERSION: u16 = 2;
```

Writer: stamp `OBJECT_FORMAT_VERSION` instead of `FORMAT_VERSION`; the symbol-emission match gains `SymbolDef::Local { blob } => { out.push(2); put_u32(&mut out, blob); }`. Reader: accept `1..=OBJECT_FORMAT_VERSION` (else `UnsupportedVersion`); kind `2` parses like kind 1 (blob-range validated) into `Local`. Update the invariants doc block: "every `Defined`/`Local` symbol indexes into `blobs`". Keep `FORMAT_VERSION` in formats/mod.rs for MX/MT (add a doc line saying MO has its own constant now).

- [ ] **Step 2: resolve.rs.** Three edits:

(a) Namespace loops: unchanged code — but add a comment at the user loop: `// Local symbols never enter the namespace: not exported, not shadowable.` (the existing `if let SymbolDef::Defined { blob }` patterns already skip `Local`).

(b) Reloc resolution inside the BFS — replace the name lookup:

```rust
        for reloc in relocs {
            let symbol = &object.symbols[reloc.symbol as usize];
            let target: Option<Site> = match symbol.def {
                // Locals bind directly within their own object — never
                // through the namespace, so they can't shadow or be
                // shadowed (spec §9).
                SymbolDef::Local { blob } => Some((site.0, blob)),
                _ => namespace.get(symbol.name.as_str()).copied(),
            };
            match target {
                None => {
                    unresolved.insert(symbol.name.clone());
                }
                Some(callee) => {
                    let idx = *index_of.entry(callee).or_insert_with(|| {
                        order_sites.push(callee);
                        queue.push_back(callee);
                        order_sites.len() - 1
                    });
                    calls.push((reloc.offset, idx));
                }
            }
        }
```

(c) The final `FuncRef` name lookup must match locals too:

```rust
            let name = object
                .symbols
                .iter()
                .find(|s| {
                    matches!(s.def,
                        SymbolDef::Defined { blob } | SymbolDef::Local { blob }
                            if blob == site.1)
                })
                .map(|s| s.name.as_str())
                .expect("site came from a Defined or Local symbol");
```

In `linker/mod.rs`, extend `LinkReport.dropped`'s doc: "Name-level and namespace-based: local symbols never appear here — unreached locals are silently omitted."

- [ ] **Step 3: Tests.** In object.rs tests: extend `sample()`-based round trips with a Local symbol (`Symbol { name: "helper".into(), def: SymbolDef::Local { blob: 0 } }` on a second blob), assert round-trip equality and that the wire version field reads 2; add a version-1 acceptance test (take valid v2 bytes of an object WITHOUT locals, patch the version u16 at offset 3 to 1, restamp CRC via `crate::formats::crc32::stamp_crc(&mut bytes, 7)`, assert `from_bytes` succeeds); add `local_symbol_with_bad_blob_rejected` mirroring the Defined case. In resolve.rs tests (extend the `obj` helper with a variant that marks named functions local, or hand-build):

```rust
    /// Like `obj`, but functions whose name is in `locals` get Local defs.
    fn obj_with_locals(arch: u8, funcs: &[(&str, &[&str])], locals: &[&str]) -> ObjectFile {
        let mut o = obj(arch, funcs);
        for s in &mut o.symbols {
            if locals.contains(&s.name.as_str())
                && let SymbolDef::Defined { blob } = s.def
            {
                s.def = SymbolDef::Local { blob };
            }
        }
        o
    }

    #[test]
    fn locals_bind_directly_and_may_repeat_across_objects() {
        // Both objects define a LOCAL `helper`; each binds to its own.
        let a = obj_with_locals(0x7E, &[("main", &["helper", "api"]), ("helper", &[])], &["helper"]);
        let b = obj_with_locals(0x7E, &[("api", &["helper"]), ("helper", &[])], &["helper"]);
        let r = resolve(&[a, b], &[]).unwrap();
        let names: Vec<&str> = r.order.iter().map(|f| f.name).collect();
        // main, its own helper, api, api's own helper: BOTH helpers linked.
        assert_eq!(names, vec!["main", "helper", "api", "helper"]);
    }

    #[test]
    fn foreign_locals_are_unresolvable_and_locals_never_shadow() {
        // Object B's `helper` is local; A's external ref must NOT see it.
        let a = obj(0x7E, &[("main", &["helper"])]);
        let b = obj_with_locals(0x7E, &[("helper", &[])], &["helper"]);
        let e = resolve(&[a, b], &[]).unwrap_err();
        assert_eq!(e, LinkError::Unresolved(vec!["helper".into()]));
    }

    #[test]
    fn local_and_global_same_name_coexist_without_duplicate_error() {
        // A exports `helper`; B has a LOCAL `helper` — no DuplicateSymbol,
        // and B's caller binds to B's own local, not A's export.
        let a = obj(0x7E, &[("main", &["api"]), ("helper", &[])]);
        let b = obj_with_locals(0x7E, &[("api", &["helper"]), ("helper", &[])], &["helper"]);
        let r = resolve(&[a, b], &[]).unwrap();
        // api's call resolved into object B (site-identity, not name):
        let api = r.order.iter().position(|f| f.name == "api").unwrap();
        let callee_idx = r.order[api].calls[0].1;
        // B's local helper blob is [0x0E, 0x02] (no calls); A's exported
        // helper has the same shape — distinguish by checking the callee
        // is NOT the same FuncRef the unreached A-helper would be: A's
        // helper must be in dropped (unreached), B's local not reported.
        assert_eq!(r.dropped, vec!["helper".to_string()]);
        assert!(callee_idx < r.order.len());
    }
```

(The third test's dropped assertion is the discriminator: A's exported `helper` goes unreached → dropped; if `api` had wrongly bound through the namespace, A's helper would be reached instead.)

- [ ] **Step 4: Gates, then commit.**

```bash
git add crates/core/src/formats/object.rs crates/core/src/linker/resolve.rs crates/core/src/linker/mod.rs
git commit -m "feat(core): Local symbol kind (MO v2) — direct intra-object binding, invisible to the namespace"
```

---

### Task 2: `.pma` surface — `.func name local`, dotted identifiers

**Files:**
- Modify: `crates/core/src/asm/parser.rs`, `crates/core/src/asm/assembler.rs`, `crates/core/src/asm/disassembler.rs`

**Interfaces:**
- Produces: `.func name local` → `SymbolDef::Local`; object disassembly prints the modifier; `is_ident` accepts `.` in continue position (mangled names travel); round-trip law extended.

- [ ] **Step 1: parser.rs.** `SourceFunction` gains `pub local: bool`. In the `.func` directive arm, split the remainder into words:

```rust
        if directive.next() == Some(".func") {
            if !pending_labels.is_empty() {
                return Err(err(
                    line_no,
                    AsmErrorKind::Syntax("label at end of function"),
                ));
            }
            let rest = directive.next().unwrap_or("").trim();
            let mut words = rest.split_whitespace();
            let name = words.next().unwrap_or("");
            let local = match words.next() {
                None => false,
                Some("local") => {
                    if words.next().is_some() {
                        return Err(err(line_no, AsmErrorKind::Syntax("junk after `local`")));
                    }
                    true
                }
                Some(_) => {
                    return Err(err(
                        line_no,
                        AsmErrorKind::Syntax("expected `local` or end of line after the name"),
                    ));
                }
            };
            if !is_ident(name) {
                return Err(err(line_no, AsmErrorKind::Syntax("bad function name")));
            }
            if functions.iter().any(|f| f.name == name) {
                return Err(err(
                    line_no,
                    AsmErrorKind::DuplicateFunction(name.to_string()),
                ));
            }
            functions.push(SourceFunction {
                name: name.to_string(),
                local,
                items: Vec::new(),
            });
            continue;
        }
```

And symbol naming splits from label naming: `is_ident` (labels) is UNCHANGED (colon-free keeps the label-scan loop sound); add `is_symbol_name` for `.func` names and jump/call operands, and switch those call sites (`.func` arm, the RelI8/RelI32 operand arm incl. the `@`-stripped path) to it:

```rust
/// Symbol names: `::`-separated namespace segments, then a dotted
/// function path (`std::api.helper`). Labels do NOT use this rule.
fn is_symbol_name(s: &str) -> bool {
    !s.is_empty()
        && s.split("::").all(|segment| {
            let mut chars = segment.chars();
            match chars.next() {
                Some(c) if c.is_alphabetic() || c == '_' => {}
                _ => return false,
            }
            chars.all(|c| c.is_alphanumeric() || c == '_' || c == '.')
        })
}
```

(Instruction lines like `call std::api` are safe in the label scanner by construction — the pre-colon head contains the mnemonic's space and fails `is_ident`; a handwritten LABEL containing `::` errors as UnknownMnemonic rather than misparsing. Add a parser test pinning both facts.)

- [ ] **Step 2: assembler.rs.** Where per-`.func` symbols are created (the `symbols` vec at the top of `assemble`), honor the flag:

```rust
    let mut symbols: Vec<Symbol> = functions
        .iter()
        .enumerate()
        .map(|(i, f)| Symbol {
            name: f.name.clone(),
            def: if f.local {
                SymbolDef::Local { blob: i as u32 }
            } else {
                SymbolDef::Defined { blob: i as u32 }
            },
        })
        .collect();
```

(`SymbolDef::Local` import.) Intra-file `call`/`jmp @` to a local name already works — the name is in `symbol_index`, the reloc points at the Local entry, and Task 1's resolve binds it directly.

- [ ] **Step 3: disassembler.rs, `disassemble_object`.** Iterate locals too and print the modifier:

```rust
    for symbol in &obj.symbols {
        let (blob, local) = match symbol.def {
            SymbolDef::Defined { blob } => (blob, false),
            SymbolDef::Local { blob } => (blob, true),
            SymbolDef::External => continue,
        };
        let code = &obj.blobs[blob as usize];
        out.push_str(&format!(
            ".func {}{}\n",
            symbol.name,
            if local { " local" } else { "" }
        ));
```

(rest of the loop unchanged — `blob` binding replaces the old `let SymbolDef::Defined { blob } … else continue`).

- [ ] **Step 4: Tests.**

parser tests: `.func f local` parses with `local == true`; `.func f loco` errors; `.func f local extra` errors; dotted name `.func outer.inner local` accepted; namespaced `.func std::api.helper local` accepted; `call outer.inner` and `call std::api` operands accepted; a label line `std::x:` errors (does not misparse); labels with `.` still fine.
assembler tests: local + call round trip —

```rust
    #[test]
    fn local_functions_get_local_symbols_and_intra_file_calls_bind() {
        let obj = asm(".func api\n        call helper\n        stop\n.func helper local\n        ret\n");
        assert!(matches!(obj.symbols[1].def, SymbolDef::Local { blob: 1 }));
        assert_eq!(obj.relocations.len(), 1);
        assert_eq!(obj.symbols[obj.relocations[0].symbol as usize].name, "helper");
    }
```

disassembler tests: round-trip law with a local —

```rust
    #[test]
    fn local_functions_round_trip_through_object_disassembly() {
        let syntax = test_syntax();
        let src = ".func api\n        call helper\n        stop\n.func helper local\n        ret\n";
        let obj1 = assemble(&syntax, 0x7E, src, false).unwrap();
        let text = disassemble_object(&syntax, &obj1);
        assert!(text.contains(".func helper local"), "{text}");
        let obj2 = assemble(&syntax, 0x7E, &text, false).unwrap();
        assert_eq!(obj1, obj2);
    }
```

- [ ] **Step 5: Gates, then commit.**

```bash
git add crates/core/src/asm/parser.rs crates/core/src/asm/assembler.rs crates/core/src/asm/disassembler.rs
git commit -m "feat(core): .func name local + dotted identifiers — locals assemble and round-trip"
```

---

### Task 3: `.pmc` — `export`, nested definitions, flattening

**Files:**
- Modify: `crates/post-machine/src/parser.rs`, `crates/post-machine/src/compiler.rs`, `crates/post-machine/src/ir.rs`, `crates/post-machine/src/codegen.rs`
- Sanctioned test updates (visibility flip; LINKED BYTES CHANGE NOWHERE): `crates/post-machine/tests/compile_programs.rs` (IR version assert 2→3; `spec_sample_runs_and_drops_the_dead_function` expects `dropped == []`; `a_pmc_compiled_library_links_lazily`'s library source gains `export ` before `goToEnd`), `crates/post-machine/tests/opt_equivalence.rs` (`spec_sample_inlines_at_o1` expects `dropped == vec![]`).

**Interfaces:**
- Produces: AST `Function { …, exported: bool, local: bool, nested: Vec<Function> }`; `compiler::flatten(Program) -> Program` (infallible: nested emptied, names mangled, calls lexically resolved, `local` computed = `!exported` at top level with `main` always exported, `true` for nested); `IrFunction.local` (IR_VERSION = 3); codegen emits `.func name local` for locals. New `CompileErrorKind::NestedExport`.

- [ ] **Step 1: parser.rs.** `Function` gains `pub exported: bool, pub local: bool, pub nested: Vec<Function>` (parser sets `local: false`; flatten owns it). In `program()`:

```rust
    fn program(mut self) -> Result<Program, CompileError> {
        let mut imports = Vec::new(); // populated in Task 4; declared now
        let mut functions: Vec<Function> = Vec::new();
        while !matches!(self.peek().kind, TokenKind::Eof) {
            // Contextual keyword: `export` + identifier = exported def;
            // `export` + `(` is a function NAMED export.
            let exported = if matches!(&self.peek().kind, TokenKind::Ident(w) if w == "export")
                && matches!(
                    self.tokens.get(self.pos + 1).map(|t| &t.kind),
                    Some(TokenKind::Ident(_))
                ) {
                self.bump();
                true
            } else {
                false
            };
            let mut f = self.function()?;
            f.exported = exported || f.name == "main"; // main always exports
            if functions.iter().any(|g| g.name == f.name) {
                return Err(CompileError {
                    line: f.line,
                    col: f.col,
                    kind: CompileErrorKind::DuplicateFunction(f.name),
                });
            }
            functions.push(f);
        }
        Ok(Program { functions, imports })
    }
```

(`Program` gains `pub imports: Vec<Import>` NOW with `Import { pub name: String, pub line: u32 }` — Task 4 fills it; declaring the field here keeps Task 4's diff surgical.) In `function()`'s body loop, BEFORE the label-collection block each iteration, detect nested definitions (4-token lookahead; reserved words never start one):

```rust
            // Nested definition: IDENT ( ) {  — visibility-only nesting.
            let is_nested_def = matches!(&self.peek().kind, TokenKind::Ident(w)
                    if !RESERVED.contains(&w.as_str()))
                && matches!(self.tokens.get(self.pos + 1).map(|t| &t.kind), Some(TokenKind::LParen))
                && matches!(self.tokens.get(self.pos + 2).map(|t| &t.kind), Some(TokenKind::RParen))
                && matches!(self.tokens.get(self.pos + 3).map(|t| &t.kind), Some(TokenKind::LBrace));
            if is_nested_def {
                let child = self.function()?;
                if nested.iter().any(|g: &Function| g.name == child.name) {
                    return Err(CompileError {
                        line: child.line,
                        col: child.col,
                        kind: CompileErrorKind::DuplicateFunction(child.name),
                    });
                }
                nested.push(child);
                continue;
            }
            // `export` before a nested definition is an error.
            if matches!(&self.peek().kind, TokenKind::Ident(w) if w == "export")
                && matches!(
                    self.tokens.get(self.pos + 1).map(|t| &t.kind),
                    Some(TokenKind::Ident(_))
                )
            {
                let t = self.peek();
                return Err(CompileError {
                    line: t.line,
                    col: t.col,
                    kind: CompileErrorKind::NestedExport,
                });
            }
```

(`let mut nested = Vec::new();` at the top of `function()`; include `nested` and `exported: false, local: false` in the returned `Function`.) Note the recursion: `self.function()` inside a body parses arbitrarily deep nesting for free.

`compiler.rs` `CompileErrorKind` gains:

```rust
    /// `export` on a nested definition — nesting is always local.
    NestedExport,
```

with Display arm: `write!(f, "nested functions are always local — remove \`export\`")`.

- [ ] **Step 2: the flatten pass** (append to `compiler.rs`, called between `parse` and `lower` in `compile()`):

```rust
/// Flatten nested definitions (spec §3): dot-mangle names
/// (`outer.inner`), resolve calls lexically (innermost scope outward,
/// then top level, else external), compute symbol locality. Infallible:
/// unresolved names simply stay external.
fn flatten(program: crate::parser::Program) -> crate::parser::Program {
    use crate::parser::{Function, Item, Program};
    use std::collections::HashMap;

    let top: HashMap<String, String> = program
        .functions
        .iter()
        .map(|f| (f.name.clone(), f.name.clone()))
        .collect();

    fn emit(
        mut f: Function,
        prefix: &str,
        scopes: &[HashMap<String, String>],
        out: &mut Vec<Function>,
    ) {
        let full = if prefix.is_empty() {
            f.name.clone()
        } else {
            format!("{prefix}.{}", f.name)
        };
        // This function's own children are visible inside its body.
        let child_map: HashMap<String, String> = f
            .nested
            .iter()
            .map(|c| (c.name.clone(), format!("{full}.{}", c.name)))
            .collect();
        let mut inner = scopes.to_vec();
        inner.push(child_map);

        for stmt in &mut f.body {
            for item in &mut stmt.items {
                if let Item::Call { name, .. } = item {
                    for scope in inner.iter().rev() {
                        if let Some(m) = scope.get(name) {
                            *name = m.clone();
                            break;
                        }
                    }
                }
            }
        }

        let children = std::mem::take(&mut f.nested);
        let is_nested = !prefix.is_empty();
        f.local = is_nested || !f.exported;
        f.exported = f.exported && !is_nested;
        f.name = full.clone();
        out.push(f);
        for c in children {
            emit(c, &full, &inner, out);
        }
    }

    let mut out = Vec::new();
    let imports = program.imports.clone();
    for f in program.functions {
        emit(f, "", std::slice::from_ref(&top), &mut out);
    }
    Program {
        functions: out,
        imports,
    }
}
```

Wire into `compile()`: `let program = flatten(crate::parser::parse(&tokens)?);`

- [ ] **Step 3: IR + codegen.** `ir.rs`: `IR_VERSION = 3`; `IrFunction` gains `pub local: bool` (serde, plain field); `lower_function` copies `f.local`. `codegen.rs` `emit_function`:

```rust
    e.push(
        format!(".func {}{}", f.name, if f.local { " local" } else { "" }),
        f.line,
    );
```

- [ ] **Step 4: Tests** (parser + compiler units; adapt existing IR/AST-constructing test literals mechanically — the compiler enumerates them):

```rust
    // parser tests
    #[test]
    fn export_is_contextual_and_main_auto_exports() {
        let p = parse_src("export api() { left; } helper() { right; } main() { mark; }").unwrap();
        assert!(p.functions[0].exported);
        assert!(!p.functions[1].exported);
        assert!(p.functions[2].exported); // main
        let p = parse_src("export() { left; } main() { @export(); }").unwrap();
        assert_eq!(p.functions[0].name, "export"); // a function NAMED export
    }

    #[test]
    fn nested_definitions_parse_recursively() {
        let p = parse_src("main() { walk() { step() { right; } @step(); } @walk(); }").unwrap();
        let main = &p.functions[0];
        assert_eq!(main.nested.len(), 1);
        assert_eq!(main.nested[0].name, "walk");
        assert_eq!(main.nested[0].nested[0].name, "step");
    }

    #[test]
    fn nested_export_and_same_scope_duplicates_error() {
        let e = parse_src("main() { export inner() { left; } }").unwrap_err();
        assert!(matches!(e.kind, CompileErrorKind::NestedExport));
        let e = parse_src("main() { f() { left; } f() { right; } }").unwrap_err();
        assert!(matches!(e.kind, CompileErrorKind::DuplicateFunction(n) if n == "f"));
    }

    // compiler (flatten) tests — in compiler.rs
    #[test]
    fn flatten_mangles_resolves_and_localizes() {
        let out = compile(
            "export api() { helper() { right; } @helper(); } helper() { left; } main() { @api(); }",
            CompileOptions::default(),
        )
        .unwrap();
        let names: Vec<(&str, bool)> = out
            .ir
            .functions
            .iter()
            .map(|f| (f.name.as_str(), f.local))
            .collect();
        assert!(names.contains(&("api", false)));
        assert!(names.contains(&("api.helper", true)));
        assert!(names.contains(&("helper", true))); // shadowed, untouched
        assert!(names.contains(&("main", false)));
        let api = out.ir.functions.iter().find(|f| f.name == "api").unwrap();
        assert!(api.blocks.iter().any(|b| b.ops.iter().any(|op| matches!(
            op, crate::ir::IrOp::Call { name, .. } if name == "api.helper"
        ))));
    }

    #[test]
    fn codegen_prints_the_local_modifier() {
        let out = compile(
            "helper() { right; } main() { @helper(); }",
            CompileOptions::default(),
        )
        .unwrap();
        assert!(out.pma.contains(".func helper local"), "{}", out.pma);
        assert!(out.pma.contains(".func main\n"), "{}", out.pma);
    }
```

Update `ir.rs`'s JSON version test to 3.

- [ ] **Step 5: Sanctioned updates** exactly as enumerated in Files, then full gates. VERIFY byte stability explicitly: `spec_sample_inlines_at_o1`'s `(18, 14)` and every other byte golden must pass UNCHANGED — if any linked-byte assertion moves, that is a defect in THIS task; BLOCK.

- [ ] **Step 6: Commit.**

```bash
git add crates/post-machine/src/parser.rs crates/post-machine/src/compiler.rs crates/post-machine/src/ir.rs crates/post-machine/src/codegen.rs crates/post-machine/tests/compile_programs.rs crates/post-machine/tests/opt_equivalence.rs
git commit -m "feat(post-machine): export-by-default visibility, nested definitions, dot-mangled flattening (IR v3)"
```

---

### Task 4: `use` imports + the warning suite

**Files:**
- Modify: `crates/post-machine/src/parser.rs` (imports), `crates/post-machine/src/compiler.rs` (warnings)

**Interfaces:**
- Produces: import parsing into `Program.imports` (field exists since Task 3); three compile-report warnings: **undeclared external** (call to a name neither defined nor imported — once per name), **unused import**, **unused function** (reachability-based: roots = `main` if present + all exports; every unreached function warns, mangled nested ones included — sound ONLY because unexported functions are invisible to other objects; user ruling).

- [ ] **Step 1: parser.rs.** In `program()`, before the `export` check each iteration:

```rust
            // Contextual keyword: `use` + identifier = import declaration;
            // `use` + `(` is a function NAMED use.
            if matches!(&self.peek().kind, TokenKind::Ident(w) if w == "use")
                && matches!(
                    self.tokens.get(self.pos + 1).map(|t| &t.kind),
                    Some(TokenKind::Ident(_))
                )
            {
                self.bump();
                loop {
                    let t = self.peek().clone();
                    let TokenKind::Ident(name) = &t.kind else {
                        return Err(Self::expected(&t, "an imported function name"));
                    };
                    if RESERVED.contains(&name.as_str()) {
                        return Err(Self::expected(&t, "an imported function name"));
                    }
                    imports.push(Import {
                        name: name.clone(),
                        line: t.line,
                    });
                    self.bump();
                    let sep = self.peek().clone();
                    match sep.kind {
                        TokenKind::Comma => {
                            self.bump();
                        }
                        TokenKind::Semi => {
                            self.bump();
                            break;
                        }
                        _ => return Err(Self::expected(&sep, "`,` or `;`")),
                    }
                }
                continue;
            }
```

- [ ] **Step 2: warnings** (append to `compiler.rs`; call in `compile()` after `flatten`, append results to the lowering warnings before building the report):

```rust
/// Import & liveness warnings (spec §3.3 as amended): undeclared
/// externals (once per name), unused imports, and unused functions —
/// reachability from `main` + exports; sound because unexported
/// functions are invisible outside this module.
fn visibility_warnings(program: &crate::parser::Program) -> Vec<Warning> {
    use crate::parser::Item;
    use std::collections::{HashMap, HashSet, VecDeque};

    let defined: HashSet<&str> = program.functions.iter().map(|f| f.name.as_str()).collect();
    let imported: HashSet<&str> = program.imports.iter().map(|i| i.name.as_str()).collect();

    let mut warnings = Vec::new();
    let mut external_called: HashSet<&str> = HashSet::new();
    let mut warned_undeclared: HashSet<&str> = HashSet::new();
    let mut edges: HashMap<&str, Vec<&str>> = HashMap::new();

    for f in &program.functions {
        let mut callees = Vec::new();
        for stmt in &f.body {
            for item in &stmt.items {
                if let Item::Call { name, line, .. } = item {
                    if defined.contains(name.as_str()) {
                        callees.push(name.as_str());
                    } else {
                        external_called.insert(name.as_str());
                        if !imported.contains(name.as_str())
                            && warned_undeclared.insert(name.as_str())
                        {
                            warnings.push(Warning {
                                line: *line,
                                message: format!(
                                    "call to undeclared external `{name}` — declare it with `use {name};`"
                                ),
                            });
                        }
                    }
                }
            }
        }
        edges.insert(f.name.as_str(), callees);
    }

    for import in &program.imports {
        if !external_called.contains(import.name.as_str()) {
            warnings.push(Warning {
                line: import.line,
                message: format!("unused import `{}`", import.name),
            });
        }
    }

    // Unused functions: reachability from main + exports.
    let mut reached: HashSet<&str> = HashSet::new();
    let mut queue: VecDeque<&str> = program
        .functions
        .iter()
        .filter(|f| f.exported || f.name == "main")
        .map(|f| f.name.as_str())
        .collect();
    while let Some(name) = queue.pop_front() {
        if !reached.insert(name) {
            continue;
        }
        if let Some(callees) = edges.get(name) {
            for c in callees {
                queue.push_back(c);
            }
        }
    }
    for f in &program.functions {
        if !reached.contains(f.name.as_str()) {
            warnings.push(Warning {
                line: f.line,
                message: format!("unused function `{}` (not exported, never called)", f.name),
            });
        }
    }
    warnings
}
```

- [ ] **Step 3: Tests** (compiler.rs):

```rust
    #[test]
    fn undeclared_external_warns_once_and_use_silences() {
        let out = compile("main() { @go(); right; @go(); }", CompileOptions::default()).unwrap();
        let n = out.report.warnings.iter()
            .filter(|w| w.message.contains("undeclared")).count();
        assert_eq!(n, 1);
        let out = compile("use go; main() { @go(); }", CompileOptions::default()).unwrap();
        assert!(out.report.warnings.iter().all(|w| !w.message.contains("undeclared")));
    }

    #[test]
    fn unused_imports_and_unused_functions_warn() {
        let out = compile("use ghost; main() { mark; }", CompileOptions::default()).unwrap();
        assert!(out.report.warnings.iter().any(|w| w.message.contains("unused import `ghost`")));

        let out = compile("dead() { left; } main() { mark; }", CompileOptions::default()).unwrap();
        assert!(out.report.warnings.iter().any(|w| w.message.contains("unused function `dead`")));

        // Transitively dead: a called only by dead — both warn.
        let out = compile(
            "a() { left; } dead() { @a(); } main() { mark; }",
            CompileOptions::default(),
        )
        .unwrap();
        let n = out.report.warnings.iter()
            .filter(|w| w.message.contains("unused function")).count();
        assert_eq!(n, 2);

        // Exported functions never warn (outside callers unknowable).
        let out = compile("export api() { left; } main() { mark; }", CompileOptions::default())
            .unwrap();
        assert!(out.report.warnings.iter().all(|w| !w.message.contains("unused function")));
    }

    #[test]
    fn use_named_function_still_parses() {
        assert!(compile("use() { left; } main() { @use(); }", CompileOptions::default()).is_ok());
    }
```

- [ ] **Step 4: Gates, then commit.**

```bash
git add crates/post-machine/src/parser.rs crates/post-machine/src/compiler.rs
git commit -m "feat(post-machine): use imports + undeclared-external, unused-import, unused-function warnings"
```

---

### Task 5: Visibility goldens

**Files:**
- Create: `crates/post-machine/tests/visibility_programs.rs`

- [ ] **Step 1: Write the suite** (note: `mtc_post_machine::asm::disassemble_object` is the PM wrapper taking one arg — distinct from core's two-arg form used in Task 2's tests):

```rust
//! Visibility end-to-end (spec §3/§6.2/§9 as amended by plan 6c):
//! locals coexist across objects, foreign locals are unreachable,
//! nesting mangles and runs, and the visibility flip changed no bytes.

use mtc_core::linker::{LinkError, LinkOptions};
use mtc_core::vm::{ArchRegistry, InfiniteTape, Machine, Outcome, RunLimits, RunOptions};
use mtc_post_machine::arch::Pm1;
use mtc_post_machine::asm::{disassemble_object, link};
use mtc_post_machine::compiler::{CompileOptions, compile};
use mtc_post_machine::optimizer::OptLevel;

fn run_exe(
    exe: &mtc_core::formats::executable::Executable,
    cells: &[bool],
    head: i64,
) -> (Outcome, Vec<i64>, i64) {
    let mut registry = ArchRegistry::new();
    registry.register(Box::new(Pm1));
    let machine = Machine::from_executable(exe, &registry).expect("loads");
    let mut tape = InfiniteTape::from_cells(cells.iter().copied(), 0, head);
    let options = RunOptions {
        limits: RunLimits {
            max_steps: Some(10_000),
            ..Default::default()
        },
        ..Default::default()
    };
    let r = machine.run(&mut tape, options);
    (r.outcome, tape.marked_cells(), tape.head())
}

const LIB: &str = "helper() { right; } export api() { @helper(); mark(!); }";

#[test]
fn same_named_locals_coexist_across_objects() {
    // Library's local helper moves RIGHT; user's local helper moves LEFT.
    // Both link; neither shadows the other; DuplicateSymbol impossible.
    let lib = compile(LIB, CompileOptions::default()).unwrap();
    let user = compile(
        "helper() { left; } main() { @api(); @helper(); }",
        CompileOptions::default(),
    )
    .unwrap();
    let linked = link(&[user.object], &[lib.object], LinkOptions::default()).unwrap();
    // Blank tape, head 0: api → lib helper: right (1), mark(!) writes 1;
    // main → user helper: left (0). Stop.
    let (outcome, marks, head) = run_exe(&linked.executable, &[false], 0);
    assert_eq!(outcome, Outcome::Stopped);
    assert_eq!(marks, vec![1]);
    assert_eq!(head, 0);
}

#[test]
fn foreign_locals_are_unresolved() {
    let lib = compile(LIB, CompileOptions::default()).unwrap();
    let user = compile("main() { @helper(); }", CompileOptions::default()).unwrap();
    let e = link(&[user.object], &[lib.object], LinkOptions::default()).unwrap_err();
    assert_eq!(e, LinkError::Unresolved(vec!["helper".into()]));
}

#[test]
fn nested_functions_mangle_run_and_round_trip() {
    let src = "main() { walk() { right; check(1, !); 1: @walk(!); } @walk(); mark; }";
    let out = compile(src, CompileOptions::default()).unwrap();
    assert!(out.pma.contains(".func main.walk local"), "{}", out.pma);
    let text = disassemble_object(&out.object);
    assert!(text.contains(".func main.walk local"), "{text}");

    let linked = link(&[out.object], &[], LinkOptions::default()).unwrap();
    let (outcome, marks, head) = run_exe(&linked.executable, &[true, true, false], 0);
    assert_eq!(outcome, Outcome::Stopped);
    assert_eq!(marks, vec![0, 1, 2]);
    assert_eq!(head, 2);

    // -O1: "main.walk" != "main", so its self-call tail-converts; behavior
    // must match (this program terminates quickly on every tape used).
    let o1 = compile(
        src,
        CompileOptions {
            opt_level: OptLevel::O1,
            ..Default::default()
        },
    )
    .unwrap();
    let l1 = link(&[o1.object], &[], LinkOptions::default()).unwrap();
    assert_eq!(
        run_exe(&l1.executable, &[true, true, false], 0),
        (Outcome::Stopped, vec![0, 1, 2], 2)
    );
}

#[test]
fn visibility_flip_changed_no_linked_bytes() {
    // The 6b inline golden lengths: symbol kinds changed, bytes did not.
    let src = "\
goToEnd() {
1:  right;
    check(1, 2);
2:  left;
}

main() {
    @goToEnd();
    right;
    check(3, 4);
3:  unmark(!);
4:  mark;
}
";
    let o1 = compile(
        src,
        CompileOptions {
            opt_level: OptLevel::O1,
            ..Default::default()
        },
    )
    .unwrap();
    let l1 = link(&[o1.object], &[], LinkOptions::default()).unwrap();
    assert_eq!(l1.executable.code.len(), 14);
    let o0 = compile(src, CompileOptions::default()).unwrap();
    let l0 = link(&[o0.object], &[], LinkOptions::default()).unwrap();
    assert_eq!(l0.executable.code.len(), 18);
}

#[test]
fn locals_still_appear_in_the_map() {
    let lib = compile(LIB, CompileOptions::default()).unwrap();
    let user = compile("main() { @api(); }", CompileOptions::default()).unwrap();
    let linked = link(&[user.object], &[lib.object], LinkOptions::default()).unwrap();
    let names: Vec<&str> = linked.map.functions.iter().map(|f| f.name.as_str()).collect();
    assert!(names.contains(&"helper"), "{names:?}"); // local, reached, mapped
}
```

- [ ] **Step 2: Full gates, then commit.**

```bash
git add crates/post-machine/tests/visibility_programs.rs
git commit -m "test(post-machine): visibility goldens — coexisting locals, foreign-local errors, nesting e2e, byte stability"
```

---

### Task 6: Namespaces — blocks, nesting, qualified imports

**Files:**
- Modify: `crates/post-machine/src/lexer.rs` (ColonColon token), `crates/post-machine/src/parser.rs`, `crates/post-machine/src/compiler.rs` (flatten scope chain + warnings), `crates/post-machine/tests/visibility_programs.rs`

**Interfaces:**
- Produces: `namespace NAME { …top-level items… }` blocks (contextual keyword; multiple per file; nestable; functions, namespaces, AND `use` declarations inside — imports are SCOPED to their enclosing block (user ruling): the binding is visible in that block and nested scopes only; reopened blocks share bindings (scopes merge by path); `use` remains illegal inside function bodies). `Function.ns: Vec<String>` (namespace path, parser-set). Full symbol name = `ns.join("::") + "::" + function-and-nested path dot-joined` (`std::api`, `a::b::f.g`); un-namespaced names have no `::`. `main` is the entry ONLY un-namespaced (a namespaced `main` is an ordinary function; auto-export applies only to the bare top-level one). `Import { path: Vec<String>, alias: Option<String>, line, ns: Vec<String> }` replaces Task 4's shape (`ns` = the declaring block's path; empty = file level) — `use std::goToEnd;` binds the tail (`goToEnd`), `use std::goToEnd as go;` binds the alias (`as` contextual); the resolved call target is the `::`-joined path.
- UNIFORM IMPORT SEMANTICS (user ruling): every `use PATH [as alias];` declares an external symbol by its FULL name and binds ONE bare name. Plain `use goToEnd;` and qualified `use std::goToEnd;` declare DIFFERENT symbols (`goToEnd` vs `std::goToEnd`) — no symbol collision; Task 4's plain form is the path-length-1 degenerate case, unchanged in meaning. BINDING collisions are errors, keyed on the BINDING NAME AFTER ALIASING (alias if present, else path tail) within one scope — `use goToEnd as qq;` + `use std::goToEnd as qq;` collide even though the paths differ; the same import repeated in DIFFERENT scopes (file level and inside a namespace block) is legal, inner shadowing outer → new `CompileErrorKind::DuplicateBinding(String)` at the second import's line ("`X` is bound twice — qualify the call (`@ns::X()`) or disambiguate with `as`"); an exactly-duplicate `use` line → warning only; a binding shadowed by a local definition → non-error (definitions outrank imports; the import warns as unused). Resolution order per call: enclosing nested scopes → enclosing namespace levels (innermost outward, each level = functions with exactly that ns path) → file top level → import bindings → external (bare, warned if undeclared).
- §9 consequence (spec delta 7): interposing a namespaced export requires declaring inside the same namespace (`namespace std { export goToEnd() {…} }` produces the same symbol; user-beats-library does the rest). Accidental collision impossible; deliberate override explicit.
- Namespaces are OPEN (user ruling): reopening `namespace std { … }` in the same file MERGES (scope maps are keyed by path, not block — members of separate same-named blocks are mutually bare-visible; duplicates detected per (path, name)); extension across modules is inherent (any object may define `std.*` symbols — no sealing, no ownership; the linker's existing arbitration rules are the only referee). Sealing = explicit v1 non-goal.

- [ ] **Step 1: lexer.** Add `TokenKind::ColonColon`, lexed greedily: on `':'`, if the next char is `':'` consume both → ColonColon, else the existing single Colon (labels `1:` unaffected). No Dot token — `use` paths name EXPORTS, which are always namespaces + one function name, so paths need only `::`.

- [ ] **Step 2: parser.** `Function` gains `pub ns: Vec<String>`. `program()` grows a namespace-aware item loop — restructure the top-level loop into `fn top_items(&mut self, ns: &[String], functions: &mut Vec<Function>, imports: &mut Vec<Import>, terminator: Option<&TokenKind>) -> Result<(), CompileError>` handling, per iteration: `use` (legal at ANY namespace depth; stamp `Import.ns = ns.to_vec()`), `namespace NAME {` (contextual: Ident("namespace") + Ident + LBrace → recurse with extended path; `namespace` + `(` stays a function named namespace), `export`, function defs (stamp `ns: ns.to_vec()`), and the terminator (RBrace for blocks, Eof for the file). Duplicate detection: per (ns-path, name) — two `f`s in DIFFERENT namespaces are legal — and NAMESPACE NAMES SHARE THE NAME POOL with function names in the same scope (declaring both `namespace a { }` and `a() { }` in one scope is a `DuplicateFunction` error; ditto two sibling namespaces only when reopening semantics don't apply — reopening the SAME namespace is legal and merges). The rule is for HUMAN clarity, not collision safety — since the `::`/`.` separator split, `a::helper` (namespace) and `a.helper` (nesting) are distinct strings by construction and cannot collide; the pool rule simply stops both spellings coexisting confusingly in one file. Exported names still decompose unambiguously (export is illegal on nested functions: an exported path is always namespaces + one function name).

`use` path grammar replaces Task 4's Step 1 list items:

```rust
                // path := IDENT (`::` IDENT)*  [ `as` IDENT ]
                let mut path = vec![/* first ident as before */];
                while matches!(self.peek().kind, TokenKind::ColonColon) {
                    self.bump();
                    let t = self.peek().clone();
                    let TokenKind::Ident(seg) = &t.kind else {
                        return Err(Self::expected(&t, "a name after `::`"));
                    };
                    path.push(seg.clone());
                    self.bump();
                }
                let alias = if matches!(&self.peek().kind, TokenKind::Ident(w) if w == "as") {
                    self.bump();
                    let t = self.peek().clone();
                    let TokenKind::Ident(a) = &t.kind else {
                        return Err(Self::expected(&t, "an alias after `as`"));
                    };
                    self.bump();
                    Some(a.clone())
                } else {
                    None
                };
```

- [ ] **Step 3: flatten + resolution** (compiler.rs). The scope CHAIN for a function with `ns = [n1, n2]` is: `[top-level (ns==[]), level [n1], level [n1,n2], …its own nested maps…]`, where each namespace level maps bare name → full dotted name for functions with EXACTLY that ns path. Binding maps are PER SCOPE: the resolution chain consults, at each level from innermost outward, that level's definitions THEN that level's import bindings, before moving outward; after all levels fail, the call stays external (bare, warned unless qualified). `DuplicateBinding` is checked per scope (two blocks may bind the same bare name independently; inner bindings shadow outer ones legally). The resolved call target is the `::`-joined path — an internal symbol if this module defines it, otherwise external. Track which imports actually bound a call: that set feeds the unused-import warning (replacing Task 4's `external_called` check for path imports). Auto-export/auto-entry: only `ns.is_empty() && name == "main"`. Locality: unchanged rule, applied to the FULL name (`export` inside a namespace → Defined `std.f`; unexported → Local `std.f`).

- [ ] **Step 3a: the Task-4 warning function is REWRITTEN, not patched.** Task 4's `visibility_warnings` reads `Import.name`, which no longer exists — resolution and import tracking move INTO `flatten`, whose signature becomes `fn flatten(program: Program) -> (Program, Vec<Warning>)`. The comma/semicolon list loop from Task 4's Step 1 IS RETAINED around the new per-item path grammar (`use a, std::b as c;` is legal: each list item is a full path with optional alias). Shape:

```rust
fn flatten(program: crate::parser::Program) -> (crate::parser::Program, Vec<Warning>) {
    // Per-scope structures, keyed by ns path:
    //   defs:     ns-path -> (bare name -> full name)
    //   bindings: ns-path -> (bare name -> (import index, full "::" path))
    // imports_used: vec![false; imports.len()]
    //
    // emit(f, ...) resolves each call, innermost outward:
    //   1. name contains "::"  -> ABSOLUTE: leave verbatim, no warning,
    //      no import consumption (self-declaring).
    //   2. the function's own nested maps (defs only), then per enclosing
    //      ns prefix (longest first): defs(prefix), THEN bindings(prefix)
    //      — a binding hit rewrites the call to the full path and marks
    //      imports_used[idx].
    //   3. total miss on a bare name -> record for the undeclared-external
    //      warning (once per name, first line).
    //
    // After emission:
    //   - unused-import warnings from unmarked imports_used slots
    //     (message text unchanged from Task 4);
    //   - unused-function warnings: reachability over the FLATTENED
    //     functions, roots = exports + bare top-level main, edges from
    //     resolved internal calls (Task 4's algorithm, full names).
    //   - DuplicateBinding (error, not warning) is detected at map-build
    //     time: two bindings for one bare name in one scope -> return is
    //     impossible here (flatten is infallible) — so the CHECK LIVES IN
    //     THE PARSER-ADJACENT VALIDATION: perform it in compile() right
    //     after parse, before flatten (iterate imports grouped by (ns,
    //     binding name)); emit CompileError there.
}
```

`compile()` becomes: parse → DuplicateBinding check → `let (program, mut vis) = flatten(…)` → lower → append `vis` to the report warnings. Task 4's standalone `visibility_warnings` function is DELETED in this task.

- [ ] **Step 3b: qualified calls.** The `@` arm of `item()` accepts a `::` path: `@ IDENT (:: IDENT)* ( … )` — build the `::`-joined name into `Item::Call.name`. Flatten treats any call name CONTAINING `::` as absolute: skip the scope chain and import table, keep verbatim (it may resolve internally if this module defines that symbol, else stays external). Add a `QualifiedNestedCall`? NOT needed — the grammar cannot produce `.` in paths, so no error case exists.

- [ ] **Step 4: warnings interplay.** Undeclared-external: bare unresolved calls only — QUALIFIED external calls are self-declaring and never warn. Unused-function reachability roots: all exports + bare `main` (namespaced exports are roots — outside callers unknowable). Unused import: an import none of whose bindings resolved any call.

- [ ] **Step 5: Tests** (parser/compiler units + goldens appended to `visibility_programs.rs`):

```rust
    #[test]
    fn namespaces_prefix_nest_and_disambiguate() {
        let out = compile(
            "namespace a { export f() { left; } namespace b { export f() { right; } } } main() { mark; }",
            CompileOptions::default(),
        )
        .unwrap();
        let names: Vec<&str> = out.ir.functions.iter().map(|f| f.name.as_str()).collect();
        assert!(names.contains(&"a::f"));
        assert!(names.contains(&"a::b::f")); // namespace nesting + same bare name, distinct symbols
    }

    #[test]
    fn qualified_use_binds_and_aliases() {
        let lib = compile(
            "namespace std { export goToEnd() { 1: right; check(1, 2); 2: left; } }",
            CompileOptions::default(),
        )
        .unwrap();
        let user = compile(
            "use std::goToEnd as go; main() { @go(); mark; }",
            CompileOptions::default(),
        )
        .unwrap();
        let linked = link(&[user.object], &[lib.object], LinkOptions::default()).unwrap();
        let (outcome, ..) = run_exe(&linked.executable, &[true, true, false], 0);
        assert_eq!(outcome, Outcome::Stopped);
    }

    #[test]
    fn namespace_members_are_bare_inside_qualified_outside() {
        // Inside the block, bare @helper works; outside, bare @helper is
        // an undeclared external (warned) and unresolved at link.
        let src = "namespace std { helper() { right; } export api() { @helper(); } } main() { @api(); }";
        let out = compile(src, CompileOptions::default()).unwrap();
        assert!(out.report.warnings.iter().all(|w| !w.message.contains("undeclared")));
        let api = out.ir.functions.iter().find(|f| f.name == "std::api").unwrap();
        assert!(api.blocks.iter().any(|b| b.ops.iter().any(
            |op| matches!(op, crate::ir::IrOp::Call { name, .. } if name == "std::helper")
        )));
    }

    #[test]
    fn deliberate_interposition_via_namespace_injection() {
        // User re-declares inside `std` — same symbol name, user wins.
        let lib = compile(
            "namespace std { export step() { right; } } export walk() { @std_step_caller_placeholder(); }",
            CompileOptions::default(),
        );
        // (Keep this test simple: link a user object defining std.step
        // against a library defining std.step — user's wins.)
        let lib = compile("namespace std { export step() { right; } }", CompileOptions::default()).unwrap();
        let user = compile(
            "namespace std { export step() { left; } } use std::step; main() { @step(); }",
            CompileOptions::default(),
        )
        .unwrap();
        let linked = link(&[user.object], &[lib.object], LinkOptions::default()).unwrap();
        let (_, _, head) = run_exe(&linked.executable, &[false], 0);
        assert_eq!(head, -1, "user's std.step (left) must win over the library's (right)");
    }

    #[test]
    fn reopened_namespaces_merge_within_a_file() {
        // Two `namespace std` blocks: members mutually bare-visible.
        let src = "namespace std { helper() { right; } } namespace std { export api() { @helper(); } } main() { @api(); }";
        let out = compile(src, CompileOptions::default()).unwrap();
        assert!(out.report.warnings.iter().all(|w| !w.message.contains("undeclared")));
        let api = out.ir.functions.iter().find(|f| f.name == "std::api").unwrap();
        assert!(api.blocks.iter().any(|b| b.ops.iter().any(
            |op| matches!(op, crate::ir::IrOp::Call { name, .. } if name == "std::helper")
        )));
        // And a real duplicate across the two blocks still errors:
        let e = compile(
            "namespace std { f() { left; } } namespace std { f() { right; } } main() { mark; }",
            CompileOptions::default(),
        );
        assert!(e.is_err());
    }

    #[test]
    fn namespace_scoped_imports_bind_inside_only() {
        let lib = compile(
            "namespace std { export goToEnd() { 1: right; check(1, 2); 2: left; } }",
            CompileOptions::default(),
        )
        .unwrap();
        // Binding qq lives inside ns; main outside must not see it.
        let user = compile(
            "namespace ns { use std::goToEnd as qq; export walk() { @qq(); } } main() { @ns::walk(); }",
            CompileOptions::default(),
        )
        .unwrap();
        assert!(user.report.warnings.iter().all(|w| !w.message.contains("undeclared")));
        let outside = compile(
            "namespace ns { use std::goToEnd as qq; } main() { @qq(); }",
            CompileOptions::default(),
        )
        .unwrap();
        // qq out of scope at file level → bare undeclared external warning.
        assert!(outside.report.warnings.iter().any(|w| w.message.contains("undeclared")));
    }

    #[test]
    fn qualified_calls_are_absolute_and_self_declaring() {
        // No `use` needed: the qualification is the declaration.
        let lib = compile(
            "namespace std { export goToEnd() { 1: right; check(1, 2); 2: left; } }",
            CompileOptions::default(),
        )
        .unwrap();
        let user = compile("main() { @std::goToEnd(); mark; }", CompileOptions::default()).unwrap();
        assert!(user.report.warnings.iter().all(|w| !w.message.contains("undeclared")));
        let linked = link(&[user.object, lib.object], &[], LinkOptions::default());
        // lib passed as a USER object here for simplicity: main + std::goToEnd.
        assert!(linked.is_ok());
        // Inside a namespace, absolute self-reference equals bare:
        let ok = compile(
            "namespace std { helper() { right; } export api() { @std::helper(); } } main() { @std::api(); }",
            CompileOptions::default(),
        )
        .unwrap();
        assert!(ok.report.warnings.iter().all(|w| !w.message.contains("undeclared")));
    }

    #[test]
    fn conflicting_bindings_error_and_aliases_disambiguate() {
        let e = compile(
            "use goToEnd; use std::goToEnd; main() { @goToEnd(); }",
            CompileOptions::default(),
        );
        assert!(e.is_err(), "two imports binding one bare name must error");
        let ok = compile(
            "use goToEnd; use std::goToEnd as stdGoToEnd; main() { @goToEnd(); @stdGoToEnd(); }",
            CompileOptions::default(),
        );
        assert!(ok.is_ok());
        // Collision key is the binding name AFTER aliasing:
        let e = compile(
            "use goToEnd as qq; use std::goToEnd as qq; main() { @qq(); }",
            CompileOptions::default(),
        );
        assert!(e.is_err(), "alias collisions are binding collisions");
        // Same import in DIFFERENT scopes: legal, inner shadows outer.
        let ok = compile(
            "use goToEnd; namespace ns { use goToEnd; export w() { @goToEnd(); } } main() { @ns::w(); }",
            CompileOptions::default(),
        );
        assert!(ok.is_ok());
    }

    #[test]
    fn namespace_and_function_names_share_one_pool() {
        // Human-clarity guard (not collision — the ::/. split already
        // prevents that): namespace `a` + function `a` in one scope would
        // put a::helper and a.helper in the same file. Confusing; error.
        let e = compile(
            "namespace a { helper() { left; } } a() { helper() { right; } } main() { mark; }",
            CompileOptions::default(),
        );
        assert!(e.is_err());
    }

    #[test]
    fn namespaced_main_is_not_the_entry_and_keywords_stay_contextual() {
        let out = compile("namespace app { main() { mark; } }", CompileOptions::default()).unwrap();
        let e = link(&[out.object], &[], LinkOptions::default()).unwrap_err();
        assert_eq!(e, LinkError::NoEntrySymbol);
        // Even EXPORTED, a namespaced main is qq::main — not the entry.
        let out = compile("namespace qq { export main() { mark; } }", CompileOptions::default()).unwrap();
        let e = link(&[out.object], &[], LinkOptions::default()).unwrap_err();
        assert_eq!(e, LinkError::NoEntrySymbol);
        assert!(compile("namespace() { left; } as() { right; } main() { @namespace(); @as(); }",
            CompileOptions::default()).is_ok());
    }
```

NOTE for the implementer on `deliberate_interposition_via_namespace_injection`: the first `let lib = …` block above is authoring residue — keep only the SECOND, simple version (library = `namespace std { export step() { right; } }`); delete BOTH the first `let lib = …` statement AND its trailing `// (Keep this test simple…)` comment. The test's substance is the final four statements.

- [ ] **Step 6: Full gates, then commit.**

```bash
git add crates/post-machine/src/lexer.rs crates/post-machine/src/parser.rs crates/post-machine/src/compiler.rs crates/post-machine/tests/visibility_programs.rs
git commit -m "feat(post-machine): namespace blocks (nestable, multiple per module) + qualified use with aliases"
```

---

## Additional spec delta (applies with the others at plan approval)

7. **§3 + §9 namespaces:** `namespace NAME { … }` blocks — naming/scope construct only (no runtime meaning): multiple per file, nestable, exports inside become `ns::path::name` symbols (namespaces join with `::`, function nesting keeps `.` — every symbol self-decomposes at the last `::`); members are bare-callable inside their block and reachable elsewhere via `use ns::path::name [as alias];` bindings or ABSOLUTE qualified calls `@ns::path::name()` (`::` segments only — nested functions stay unnameable; qualified externals are self-declaring, exempt from the undeclared warning). Only a top-level un-namespaced `main` is the entry. Interposing a namespaced export = declaring inside the same namespace in a user object (same symbol; user-beats-library applies) — accidental collision impossible, deliberate override explicit. Namespaces are OPEN: same-file reopening merges (path-keyed scopes); cross-module extension is inherent (any object may define `ns.*` — no sealing/ownership in v1; linker arbitration rules are the referee). The stdlib ships as `namespace std { … }`; §9's "user definitions shadow stdlib naturally" is superseded by this explicit-injection rule.

---

## Plan Self-Review Notes

- **Spec deltas 1–7 covered:** T1 (§6.2, §9), T2 (§6.4), T3 (§3 visibility + nesting, §7 IR v3), T4 (§3.3 imports + warnings, including the user-requested **unused-function warning** — reachability over `main`+exports, sound only under hidden-by-default; exports exempt by construction), T5 (goldens), T6 (delta 7: namespaces — blocks/reopening/nesting, `::` mangling + denamespacing, scoped `use` with paths/aliases, absolute qualified calls, DuplicateBinding, name-pool rule; T6 REWRITES T4's warning function into flatten — see T6 Step 3a).
- **Byte-stability invariant** stated three times (Global Constraints, T3 Step 5 verify, T5 regression test): the visibility flip changes symbol kinds, never linked bytes.
- **Type consistency hand-check:** `Function{exported, local, nested}` flows parser → flatten → lower (`IrFunction.local`) → codegen (`.func … local`) → asm parser (`SourceFunction.local`) → object (`SymbolDef::Local`) → linker (direct bind). `Program.imports` declared in T3 (so T4's diff is additive), parsed in T4 (bare form), GENERALIZED in T6 (`Import{path, alias, line, ns}`; T4's `visibility_warnings` deleted, logic absorbed into flatten). `Function.ns` is introduced by T6 alone.
- **Keyword ruling recorded:** `export`, not `external` (External = import in MO vocabulary); both `export` and `use` contextual, LL(2)-disambiguated, with function-named-export/use pins.
- **Deliberately absent:** warnings-as-errors strictness (Plan 7 CLI); `.pma` default flip (assembly mirrors the object model explicitly); `export main` special-casing (redundant-legal).
- **Sanctioned golden edits enumerated** in T3's Files block; nothing else may move. Rounds not pinned anywhere (6a/6b lesson).

