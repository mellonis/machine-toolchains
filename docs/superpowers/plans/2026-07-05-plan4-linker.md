# Plan 4/7: Linker

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `MO` objects → `MX` executables: symbol resolution with library (lazy) semantics, dead-function elimination via reachability from `main`, deterministic layout, call relocation patching, **linker relaxation** (far→short calls) with blob re-decoding to re-patch spanning jumps, the `.pmx.map` JSON sidecar, and the first *linked* program running on the Machine — plus the two disassembler adjustments the Plan 3 final review deferred here.

**Architecture:** Spec §9 + §4.4 + §6. The linker lives in `mtc_core::linker`, arch-generic (driven by `ArchSyntax`, sharing Plan 3's decoder). Relaxation is the user-approved design: fixpoint over call widths (monotone shrink), and each affected blob is **re-emitted from a fresh decode of the original blob** — jump offsets recomputed (always safe: spans only shrink, widths never break), relocation and debug offsets remapped through the same original→new offset table.

**Tech Stack:** Rust stable, edition 2024. NEW runtime deps in `mtc-core`: `serde` (derive) + `serde_json` — sanctioned by spec §10 for JSON artifacts (the map sidecar).

**Spec:** `docs/superpowers/specs/2026-07-04-post-machine-toolchain-design.md` §9 (linker), §6.1 (`MX`), §10 (build modes / map sidecar).

## Global Constraints

- Link pipeline (spec §9): collect symbols → error on duplicates among user objects and on `main` missing → reachability from `main` (only reached functions emit — dead-function elimination; libraries contribute lazily) → deterministic layout (`main` first at offset 0, then BFS discovery order) → patch call relocations IP-relative → relaxation fixpoint (`LinkOptions { relax: bool }`, default true) → emit `Executable { arch, entry: 0, code }`.
- **Unresolved symbols error only if REACHABLE from `main`** (lazy semantics — unreachable functions may reference anything; they're dropped).
- Library precedence: user objects first (duplicates among them = error); then libraries in argument order, first-wins, silently shadowed by user definitions (spec §9 stdlib semantics).
- All objects and libraries must carry the same arch byte → `LinkError::ArchMismatch`.
- Relaxation safety invariant (approved design): shrinking only decreases spans; every previously-chosen width still fits; far jumps stay far even if newly shortable (correct, merely non-optimal).
- The map sidecar is ALWAYS built in-memory (`LinkOutput { executable, map }`); function names/ranges come from symbols, labels/lines only when objects carry debug sections. JSON via serde; schema below is the contract.
- Disassembler adjustments (deferred from Plan 3's final review): (a) executable form prints the FAR mnemonic for short calls (`call`, not `call.s`) — width is the linker's choice, and `call.s NAME` is rejected by the assembler, so this keeps dis output assemblable; (b) object-form call sites with no relocation emit `.byte` fallback (the old `L{t:04X}` printed an undefined label).
- Quality gates on every commit: `cargo test --workspace` green, `cargo clippy --workspace --all-targets -- -D warnings` clean, `cargo fmt --check` clean; no attribution footers; per-task commits pre-approved, path-scoped, never push.

## Interfaces Established by This Plan

```rust
// mtc_core::asm (visibility change, Task 1)
// decode machinery moves to asm/decode.rs, pub(crate): Decoded, Body,
// DecodedOperand, decode_at, decode_stream — shared by disassembler + linker.

// mtc_core::linker (new module: linker/{mod,resolve,layout}.rs)
#[derive(Debug, PartialEq, Eq)]
pub enum LinkError {
    DuplicateSymbol(String),
    Unresolved(Vec<String>),        // reachable-from-main unresolved names, sorted
    NoEntrySymbol,                  // no `main` among user objects (or libraries)
    ArchMismatch { expected: u8, found: u8 },
    MalformedBlob { symbol: String, at: u32 },  // decode failed during relaxation
}
#[derive(Debug, Clone, Copy)]
pub struct LinkOptions { pub relax: bool }
impl Default for LinkOptions { fn default() -> Self { Self { relax: true } } }

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct MapFile {
    pub arch: u8,
    pub alphabet: Vec<String>,      // presentation glyphs; empty if unknown
    pub functions: Vec<MapFunction>,
}
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct MapFunction {
    pub name: String,
    pub start: u32,                 // absolute code offset of the function's ent
    pub end: u32,                   // exclusive
    pub labels: Vec<(String, u32)>, // absolute offsets; empty without -g objects
    pub lines: Vec<(u32, u32)>,     // (absolute code offset, source line)
}
impl MapFile {
    pub fn to_json(&self) -> String;                       // serde_json pretty
    pub fn from_json(s: &str) -> Result<MapFile, String>;  // error stringified
}

/// Structured account of what the linker did — the CLI renders it under
/// `-v` (Plan 7); libraries never print (library-first principle).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkReport {
    pub dropped: Vec<String>,       // defined but unreachable, sorted
    pub relaxed_calls: u32,
    pub far_calls: u32,
}
pub struct LinkOutput { pub executable: Executable, pub map: MapFile, pub report: LinkReport }
pub fn link(
    syntax: &ArchSyntax,
    objects: &[ObjectFile],
    libraries: &[ObjectFile],
    options: LinkOptions,
) -> Result<LinkOutput, LinkError>;

// mtc_post_machine::asm (addition)
pub fn link(objects: &[ObjectFile], libraries: &[ObjectFile], options: LinkOptions)
    -> Result<LinkOutput, LinkError>;   // pm1_syntax + PM alphabet [" ", "*"] in the map
```

Entry symbol name is the literal `"main"` (spec §3.1/§9); `entry` in the emitted `Executable` is always 0 (main laid out first — this also discharges the "pre-root gap" constraint recorded from Plan 3).

---

### Task 1: Share the decoder; disassembler adjustments

**Files:**
- Create: `crates/core/src/asm/decode.rs` (moved machinery, `pub(crate)`)
- Modify: `crates/core/src/asm/disassembler.rs` (import from `decode`; the two behavior adjustments + tests)
- Modify: `crates/core/src/asm/mod.rs` (add `pub(crate) mod decode;`)

**Interfaces:**
- Produces (crate-internal): `decode::{Decoded, Body, DecodedOperand, decode_at, decode_stream}` exactly as they exist today (move, not rewrite).
- Behavior changes in the disassembler ONLY:
  1. Executable form: a `Flow::Call` instruction whose opcode is the `short` of a relax pair prints the FAR partner's mnemonic (operand rules unchanged).
  2. Object form: a call site with no relocation entry → `.byte` fallback for the whole instruction (replaces the undefined-label `L{t:04X}` print).

- [ ] **Step 1: Write the failing tests**

Append to `disassembler.rs`'s test module:

```rust
    #[test]
    fn short_call_in_executable_prints_far_mnemonic() {
        let syntax = test_syntax();
        // Add a short-call opcode to a LOCAL syntax copy: fixture has none.
        let mut syntax = syntax;
        syntax.entries.push(SyntaxEntry {
            opcode: 0x31,
            mnemonic: "call.s",
            operand: OperandKind::RelI8,
            flow: Flow::Call,
        });
        syntax.relax_pairs.push(RelaxPair { far: 0x21, short: 0x31 });
        // f at 0 short-calls g at 4: call.s at 1, end 3, off = +1.
        let code = vec![0x0E, 0x31, 0x01, 0x02, 0x0E, 0x0B];
        let exe = Executable { arch: 0x7E, entry: 0, code };
        let text = disassemble_executable(&syntax, &exe);
        assert!(text.contains("call    func_0004"), "short call prints far mnemonic:\n{text}");
        assert!(!text.contains("call.s"), "call.s must not appear:\n{text}");
    }

    #[test]
    fn object_call_without_relocation_falls_back_to_bytes() {
        let syntax = test_syntax();
        let obj = crate::formats::object::ObjectFile {
            arch: 0x7E,
            symbols: vec![crate::formats::object::Symbol {
                name: "f".into(),
                def: crate::formats::object::SymbolDef::Defined { blob: 0 },
            }],
            // ent, call with a PATCHED (non-hole) offset and NO reloc, stop
            blobs: vec![vec![0x0E, 0x21, 0x02, 0x00, 0x00, 0x00, 0x02]],
            relocations: vec![],
            debug: None,
        };
        let text = disassemble_object(&syntax, &obj);
        assert!(text.contains(".byte   33"), "0x21 opcode dumps as byte:\n{text}");
        assert!(!text.contains("L0"), "no phantom labels:\n{text}");
        // Round-trip still holds through the fallback:
        let back = crate::asm::assembler::assemble(&syntax, 0x7E, &text, false).unwrap();
        assert_eq!(back.blobs, obj.blobs);
    }
```

(`ArchSyntax`'s fields are public — the local-copy mutation is legitimate test setup.)

- [ ] **Step 2: RED**

Run: `cargo test -p mtc-core disassembler`
Expected: both new tests fail against current behavior (`call.s` printed; `L0008`-style label emitted).

- [ ] **Step 3: Implement**

Move `Decoded`/`Body`/`DecodedOperand`/`decode_at`/`decode_stream` verbatim into `crates/core/src/asm/decode.rs` with `pub(crate)` visibility; `disassembler.rs` imports them. Then:

1. In the executable form's mnemonic rendering, compute the display mnemonic:
```rust
    let display_mnemonic = |entry: &SyntaxEntry| -> &'static str {
        if entry.flow == Flow::Call {
            if let Some(pair) = syntax.relax_pairs.iter().find(|p| p.short == entry.opcode) {
                if let Some(far) = syntax.by_opcode(pair.far) {
                    return far.mnemonic;
                }
            }
        }
        entry.mnemonic
    };
```
and use it wherever the executable form prints a call mnemonic.
2. In the object form, replace the reloc-miss arm (`None => format!("L{t:04X}")`) with the same `.byte`-fallback emission used for cross-region jumps (whole instruction as `.byte` lines, label only on the first).

- [ ] **Step 4: GREEN + full gates**

Run: `cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings && cargo fmt`
Expected: all green (existing round-trip and grid tests unchanged).

- [ ] **Step 5: Commit (path-scoped)**

```bash
git commit crates/core/src/asm -m "refactor(core): shared asm decode module; dis prints far call mnemonic, byte-fallback for reloc-less calls"
```

---

### Task 2: Symbol resolution + reachability

**Files:**
- Create: `crates/core/src/linker/mod.rs`
- Create: `crates/core/src/linker/resolve.rs`
- Modify: `crates/core/src/lib.rs` (add `pub mod linker;`)

**Interfaces:**
- Produces: `LinkError` (public, per header block); crate-internal resolution result consumed by Task 3:
  ```rust
  pub(crate) struct Resolved<'a> {
      /// Functions in layout order: main first, then BFS discovery order.
      pub order: Vec<FuncRef<'a>>,
  }
  pub(crate) struct FuncRef<'a> {
      pub name: &'a str,
      pub blob: &'a [u8],
      pub debug: Option<&'a BlobDebug>,
      /// Call sites in blob order: (hole offset in blob, callee index in `order`).
      pub calls: Vec<(u32, usize)>,
  }
  pub(crate) fn resolve<'a>(objects: &'a [ObjectFile], libraries: &'a [ObjectFile])
      -> Result<Resolved<'a>, LinkError>;
  ```
- Semantics: arch consistency checked first (against the first object's arch; empty input → `NoEntrySymbol`); namespace = user Defined (dup → `DuplicateSymbol`, name-sorted first offender) then libraries first-wins; BFS from `"main"` (missing → `NoEntrySymbol`); every reached function's relocations resolve through the namespace — reached-but-undefined names accumulate into `Unresolved` (sorted, deduped); callee indices assigned in discovery order (queue seeded with `main`, neighbors pushed in relocation-offset order).

- [ ] **Step 1: Write the failing tests**

Test module at the bottom of `resolve.rs` (builders keep tests terse):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::formats::object::{ObjectFile, Relocation, Symbol, SymbolDef};

    /// Object with `funcs` = (name, callees-by-name). Blob content is a
    /// stub: [0x0E] + one 5-byte call hole per callee (opcode 0x21).
    fn obj(arch: u8, funcs: &[(&str, &[&str])]) -> ObjectFile {
        let mut symbols: Vec<Symbol> = funcs
            .iter()
            .enumerate()
            .map(|(i, (n, _))| Symbol { name: (*n).into(), def: SymbolDef::Defined { blob: i as u32 } })
            .collect();
        let mut blobs = Vec::new();
        let mut relocations = Vec::new();
        for (bi, (_, callees)) in funcs.iter().enumerate() {
            let mut blob = vec![0x0E];
            for callee in *callees {
                let sym = symbols.iter().position(|s| s.name == **callee).unwrap_or_else(|| {
                    symbols.push(Symbol { name: (*callee).into(), def: SymbolDef::External });
                    symbols.len() - 1
                });
                blob.push(0x21);
                relocations.push(Relocation {
                    blob: bi as u32,
                    offset: blob.len() as u32,
                    symbol: sym as u32,
                });
                blob.extend([0u8; 4]);
            }
            blob.push(0x02);
            blobs.push(blob);
        }
        ObjectFile { arch, symbols, blobs, relocations, debug: None }
    }

    #[test]
    fn bfs_order_is_main_first_discovery_order() {
        let a = obj(0x7E, &[("helper", &[]), ("main", &["helper", "second"]), ("second", &["helper"])]);
        let r = resolve(std::slice::from_ref(&a), &[]).unwrap();
        let names: Vec<&str> = r.order.iter().map(|f| f.name).collect();
        assert_eq!(names, vec!["main", "helper", "second"]);
        assert_eq!(r.order[0].calls, vec![(2, 1), (7, 2)]); // holes at 2 and 7
    }

    #[test]
    fn dead_functions_are_dropped_and_may_be_broken() {
        // "dead" calls a missing symbol — fine, it's unreachable.
        let a = obj(0x7E, &[("main", &[]), ("dead", &["missing"])]);
        let r = resolve(std::slice::from_ref(&a), &[]).unwrap();
        assert_eq!(r.order.len(), 1);
        assert_eq!(r.order[0].name, "main");
    }

    #[test]
    fn reachable_unresolved_errors_sorted() {
        let a = obj(0x7E, &[("main", &["zeta", "alpha"])]);
        let e = resolve(std::slice::from_ref(&a), &[]).unwrap_err();
        assert_eq!(e, LinkError::Unresolved(vec!["alpha".into(), "zeta".into()]));
    }

    #[test]
    fn libraries_resolve_lazily_and_users_shadow() {
        let user = obj(0x7E, &[("main", &["go"]), ("go", &[])]);
        let lib = obj(0x7E, &[("go", &[]), ("unused_pulls_nothing", &["ghost"])]);
        // user's `go` shadows the library's; the library's broken function
        // is never reached, so `ghost` doesn't error.
        let r = resolve(std::slice::from_ref(&user), std::slice::from_ref(&lib)).unwrap();
        let names: Vec<&str> = r.order.iter().map(|f| f.name).collect();
        assert_eq!(names, vec!["main", "go"]);

        let needy = obj(0x7E, &[("main", &["go"])]);
        let r2 = resolve(std::slice::from_ref(&needy), std::slice::from_ref(&lib)).unwrap();
        assert_eq!(r2.order.len(), 2); // library's go pulled in
    }

    #[test]
    fn duplicate_user_symbols_error_but_library_shadowing_does_not() {
        let a = obj(0x7E, &[("main", &[]), ("f", &[])]);
        let b = obj(0x7E, &[("f", &[])]);
        let e = resolve(&[a.clone(), b], &[]).unwrap_err();
        assert_eq!(e, LinkError::DuplicateSymbol("f".into()));
        let lib1 = obj(0x7E, &[("f", &[])]);
        let lib2 = obj(0x7E, &[("f", &[])]);
        assert!(resolve(std::slice::from_ref(&a), &[lib1, lib2]).is_ok()); // first-wins, silent
    }

    #[test]
    fn no_main_and_arch_mismatch() {
        let a = obj(0x7E, &[("helper", &[])]);
        assert_eq!(resolve(std::slice::from_ref(&a), &[]).unwrap_err(), LinkError::NoEntrySymbol);
        let b = obj(0x11, &[("main", &[])]);
        let mixed = [obj(0x7E, &[("x", &[])]), b];
        assert_eq!(
            resolve(&mixed, &[]).unwrap_err(),
            LinkError::ArchMismatch { expected: 0x7E, found: 0x11 }
        );
    }
}
```

- [ ] **Step 2: RED** — `cargo test -p mtc-core linker` → compile error.

- [ ] **Step 3: Implement**

`linker/mod.rs`: `LinkError` (+ `Display`/`Error` impls, message per variant), `pub(crate) mod resolve;` and re-exports. `resolve.rs`:

```rust
use std::collections::{BTreeSet, HashMap, VecDeque};

use super::LinkError;
use crate::formats::object::{BlobDebug, ObjectFile, SymbolDef};

pub(crate) struct FuncRef<'a> {
    pub name: &'a str,
    pub blob: &'a [u8],
    pub debug: Option<&'a BlobDebug>,
    pub calls: Vec<(u32, usize)>,
}

pub(crate) struct Resolved<'a> {
    pub order: Vec<FuncRef<'a>>,
}

/// (object index within the user+library concatenation, blob index)
type Site = (usize, u32);

pub(crate) fn resolve<'a>(
    objects: &'a [ObjectFile],
    libraries: &'a [ObjectFile],
) -> Result<Resolved<'a>, LinkError> {
    let all: Vec<&ObjectFile> = objects.iter().chain(libraries).collect();
    let Some(first) = all.first() else { return Err(LinkError::NoEntrySymbol) };
    let expected = first.arch;
    if let Some(bad) = all.iter().find(|o| o.arch != expected) {
        return Err(LinkError::ArchMismatch { expected, found: bad.arch });
    }

    // Namespace: user objects (dup = error), then libraries (first-wins).
    let mut namespace: HashMap<&str, Site> = HashMap::new();
    for (oi, object) in objects.iter().enumerate() {
        for symbol in &object.symbols {
            if let SymbolDef::Defined { blob } = symbol.def {
                if namespace.insert(symbol.name.as_str(), (oi, blob)).is_some() {
                    return Err(LinkError::DuplicateSymbol(symbol.name.clone()));
                }
            }
        }
    }
    for (li, library) in libraries.iter().enumerate() {
        for symbol in &library.symbols {
            if let SymbolDef::Defined { blob } = symbol.def {
                namespace.entry(symbol.name.as_str()).or_insert((objects.len() + li, blob));
            }
        }
    }

    let object_at = |oi: usize| -> &'a ObjectFile {
        if oi < objects.len() { &objects[oi] } else { &libraries[oi - objects.len()] }
    };

    // BFS from main.
    let Some(&main_site) = namespace.get("main") else { return Err(LinkError::NoEntrySymbol) };
    let mut order_sites: Vec<Site> = vec![main_site];
    let mut index_of: HashMap<Site, usize> = HashMap::from([(main_site, 0)]);
    let mut queue: VecDeque<Site> = VecDeque::from([main_site]);
    let mut unresolved: BTreeSet<String> = BTreeSet::new();
    // calls per discovered function, resolved lazily to final indices.
    let mut calls_by_site: HashMap<Site, Vec<(u32, Site)>> = HashMap::new();

    while let Some(site) = queue.pop_front() {
        let object = object_at(site.0);
        let mut calls = Vec::new();
        let mut relocs: Vec<_> = object
            .relocations
            .iter()
            .filter(|r| r.blob == site.1)
            .collect();
        relocs.sort_by_key(|r| r.offset);
        for reloc in relocs {
            let name = object.symbols[reloc.symbol as usize].name.as_str();
            match namespace.get(name) {
                None => {
                    unresolved.insert(name.to_string());
                }
                Some(&callee) => {
                    if !index_of.contains_key(&callee) {
                        index_of.insert(callee, order_sites.len());
                        order_sites.push(callee);
                        queue.push_back(callee);
                    }
                    calls.push((reloc.offset, index_of[&callee]));
                }
            }
        }
        calls_by_site.insert(site, calls);
    }

    if !unresolved.is_empty() {
        return Err(LinkError::Unresolved(unresolved.into_iter().collect()));
    }

    let order = order_sites
        .iter()
        .map(|&site| {
            let object = object_at(site.0);
            let name = object
                .symbols
                .iter()
                .find(|s| s.def == SymbolDef::Defined { blob: site.1 })
                .map(|s| s.name.as_str())
                .expect("site came from a Defined symbol");
            FuncRef {
                name,
                blob: &object.blobs[site.1 as usize],
                debug: object.debug.as_ref().map(|d| &d[site.1 as usize]),
                calls: calls_by_site.remove(&site).unwrap_or_default(),
            }
        })
        .collect();
    Ok(Resolved { order })
}
```

(Note the borrow of `calls_by_site` in the final map — use `let mut calls_by_site` and `remove`; adjust to satisfy the borrow checker while preserving semantics.)

- [ ] **Step 4: GREEN + gates** — `cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings && cargo fmt`

- [ ] **Step 5: Commit (path-scoped)**

```bash
git commit crates/core/src/linker crates/core/src/lib.rs -m "feat(core): linker symbol resolution with lazy libraries and reachability"
```

---

### Task 3: Layout, relaxation, emission

**Files:**
- Create: `crates/core/src/linker/layout.rs`
- Modify: `crates/core/src/linker/mod.rs` (public `link`, `LinkOptions`, `LinkOutput`, `MapFile` skeleton)

**Interfaces:**
- Produces: `link(syntax, objects, libraries, options) -> Result<LinkOutput, LinkError>` per the header block. In THIS task `MapFile` is the plain struct WITHOUT serde derives (Task 4 adds serde + JSON); functions get names/start/end; labels/lines filled from debug when present.
- Algorithm (approved design):
  1. `resolve()` → ordered functions.
  2. Per function, decode the ORIGINAL blob once (`decode::decode_stream` from offset 0 — includes the leading ent): instruction list `(orig_addr, len, kind)` where kind ∈ {plain bytes, jump {far/short, orig_target}, call-site {hole orig offset → callee idx}}. Jumps recognized by `Flow::Jump | Flow::Branch` with `RelTarget`; call sites matched by hole offset = `orig_addr + 1` against `FuncRef.calls` (decode failure or unmatched call instruction → `MalformedBlob`).
  3. Width vector: every call site starts FAR. Fixpoint (skipped when `!options.relax` or the call opcode has no short partner): compute function sizes (orig size − 3 × #short-calls-in-it), prefix-sum bases (main at 0); for each still-far call: `instr_end = base_f + new_off_of(call site) + 5`; `off = callee_base − instr_end`; if `i8` fits → mark short, repeat. Monotone: shrinking only decreases distances; converges.
  4. Emit: per function, walk original instructions building the original→new offset map; calls emit far (opcode + i32 LE) or SHORT partner opcode + i8; jumps re-encode with the SAME width, offset recomputed via the map (`new_target − new_end`; the width always fits — assert it); everything else copied verbatim. Patch call offsets against callee bases.
  5. `Executable { arch, entry: 0, code }` + `MapFile` with function ranges and remapped debug labels/lines (absolute offsets) + `LinkReport` (dropped = namespace's Defined names minus reached, sorted; relaxed/far call counts from the final width vector). `resolve()` therefore also returns the dropped-name list (extend `Resolved` with `pub dropped: Vec<String>`).
- Terminology: “hole offset” is blob-relative and points at the 4-byte operand; the call OPCODE is at `hole − 1`.

- [ ] **Step 1: Write the failing tests**

Test module at the bottom of `layout.rs` — uses the SAME fixture syntax as Plan 3 (`crate::asm::syntax::fixture::test_syntax`) and the assembler to produce real objects:

```rust
#[cfg(test)]
mod tests {
    use super::super::{link, LinkOptions};
    use crate::asm::assembler::assemble;
    use crate::asm::syntax::fixture::test_syntax;
    use crate::vm::OperandKind;
    use crate::asm::{Flow, RelaxPair, SyntaxEntry};

    /// Fixture + a short call (0x31) so relaxation has a target form.
    fn syntax_with_short_call() -> crate::asm::ArchSyntax {
        let mut s = test_syntax();
        s.entries.push(SyntaxEntry {
            opcode: 0x31, mnemonic: "call.s", operand: OperandKind::RelI8, flow: Flow::Call,
        });
        s.relax_pairs.push(RelaxPair { far: 0x21, short: 0x31 });
        s
    }

    const TWO_FUNCS: &str = "\
.func main
        call    go
        stop
.func go
        nop
        ret
";

    #[test]
    fn links_two_functions_with_relaxed_call() {
        let syntax = syntax_with_short_call();
        let obj = assemble(&syntax, 0x7E, TWO_FUNCS, false).unwrap();
        let out = link(&syntax, &[obj], &[], LinkOptions::default()).unwrap();
        // main: [0E][31 off][02] = 4 bytes; go at 4: [0E][01][0B].
        // call.s at 1, end 3, target 4 → off +1.
        assert_eq!(out.executable.code, vec![0x0E, 0x31, 0x01, 0x02, 0x0E, 0x01, 0x0B]);
        assert_eq!(out.executable.entry, 0);
        assert_eq!(out.map.functions.len(), 2);
        assert_eq!((out.map.functions[0].name.as_str(), out.map.functions[0].start, out.map.functions[0].end), ("main", 0, 4));
        assert_eq!((out.map.functions[1].name.as_str(), out.map.functions[1].start, out.map.functions[1].end), ("go", 4, 7));
    }

    #[test]
    fn no_relax_keeps_far_calls() {
        let syntax = syntax_with_short_call();
        let obj = assemble(&syntax, 0x7E, TWO_FUNCS, false).unwrap();
        let out = link(&syntax, &[obj], &[], LinkOptions { relax: false }).unwrap();
        // main: [0E][21 off32][02] = 7 bytes; go at 7; call end 6 → off +1.
        assert_eq!(
            out.executable.code,
            vec![0x0E, 0x21, 0x01, 0x00, 0x00, 0x00, 0x02, 0x0E, 0x01, 0x0B]
        );
    }

    #[test]
    fn jump_spanning_a_shrunk_call_is_repatched() {
        // THE approved-design case: a backward jump over a call site.
        // L: nop ; call go ; jmp L ; stop  — the jmp crosses the call hole.
        let src = "\
.func main
L:      nop
        call    go
        jmp     L
        stop
.func go
        ret
";
        let syntax = syntax_with_short_call();
        let obj = assemble(&syntax, 0x7E, src, false).unwrap();
        // Original blob: [0E][01][21 hole][30 off][02]: jmp.s at 7..9, end 9,
        // target 1 → orig off = -8.
        assert_eq!(obj.blobs[0][7], 0x30);
        assert_eq!(obj.blobs[0][8] as i8, -8);
        let out = link(&syntax, &[obj], &[], LinkOptions::default()).unwrap();
        // After shrink: [0E][01][31 off][30 off'][02] = 6 bytes; go at 6.
        // call.s at 2, end 4, target 6 → +2. jmp.s at 4..6, end 6, target 1 → -5.
        assert_eq!(
            out.executable.code,
            vec![0x0E, 0x01, 0x31, 0x02, 0x30, 0xFB, 0x02, 0x0E, 0x0B]
        );
    }

    #[test]
    fn debug_offsets_are_remapped() {
        let src = "\
.func main
        call    go
X:      stop
.func go
        ret
";
        let syntax = syntax_with_short_call();
        let obj = assemble(&syntax, 0x7E, src, true).unwrap();
        // Original: X at blob offset 6 (after ent + 5-byte call).
        let out = link(&syntax, &[obj], &[], LinkOptions::default()).unwrap();
        // Relaxed: call.s is 2 bytes → X moves to absolute 3.
        assert_eq!(out.map.functions[0].labels, vec![("X".to_string(), 3)]);
        assert!(!out.map.functions[0].lines.is_empty());
    }

    #[test]
    fn far_call_when_out_of_short_range() {
        // Pad main so the callee lands beyond +127 from the call site.
        let mut src = String::from(".func main\n        call    go\n");
        for _ in 0..150 {
            src.push_str("        nop\n");
        }
        src.push_str("        stop\n.func go\n        ret\n");
        let syntax = syntax_with_short_call();
        let obj = assemble(&syntax, 0x7E, &src, false).unwrap();
        let out = link(&syntax, &[obj], &[], LinkOptions::default()).unwrap();
        assert_eq!(out.executable.code[1], 0x21, "call must stay far");
    }
}
```

- [ ] **Step 2: RED** — compile error.

- [ ] **Step 3: Implement**

`layout.rs` implements the algorithm in the interface notes. Key structures:

```rust
enum Piece {
    Verbatim { orig: u32, bytes: Vec<u8> },
    Jump { orig: u32, opcode: u8, width: u8 /*1|4*/, orig_target: u32 },
    CallSite { orig: u32 /*opcode addr*/, callee: usize },
}
```

Per function: decode via `crate::asm::decode::decode_stream(syntax, blob, 0, blob.len() as u32)`; classify each `Decoded` (Raw → `MalformedBlob`; Instr with Flow Jump/Branch + RelTarget → `Jump` [recover `orig_target` from the decoded target]; Instr with Flow Call → must match a `FuncRef.calls` hole at `orig + 1`, else `MalformedBlob`; everything else → `Verbatim`, re-emitting original bytes `blob[orig..orig+len]`). Sizes/fixpoint/emission per the notes; when emitting a `Jump`, recompute `new_off = new_target - new_end` through the offset map and `debug_assert!` the width still fits (guaranteed by the shrink-only invariant). Map building: function start/end from final bases; labels/lines pushed through the same offset map (+ base).

`mod.rs` gains `LinkOptions`, `LinkOutput`, plain `MapFile`/`MapFunction`, and `pub fn link(...)` orchestrating resolve → layout.

- [ ] **Step 4: GREEN + gates**

If `jump_spanning_a_shrunk_call_is_repatched` fails, re-derive by hand before touching code — the expected bytes above were derived twice (assembler layout AND post-shrink layout); a mismatch means the offset-map plumbing is wrong, not the test.

- [ ] **Step 5: Commit (path-scoped)**

```bash
git commit crates/core/src/linker -m "feat(core): linker layout, call relaxation with blob re-encoding, MX emission"
```

---

### Task 4: The map sidecar (serde)

**Files:**
- Modify: `crates/core/Cargo.toml` (add `serde = { version = "1", features = ["derive"] }`, `serde_json = "1"`)
- Modify: `crates/core/src/linker/mod.rs` (derives + `to_json`/`from_json` + tests)

**Interfaces:**
- Produces: `MapFile::{to_json, from_json}`; serde derives on `MapFile`/`MapFunction` exactly as the header block shows. JSON schema = the natural serde output of those structs (documented by the round-trip test).

- [ ] **Step 1: Write the failing tests**

In `linker/mod.rs`'s test module:

```rust
    #[test]
    fn map_json_round_trips() {
        let map = MapFile {
            arch: 1,
            alphabet: vec![" ".into(), "*".into()],
            functions: vec![MapFunction {
                name: "main".into(),
                start: 0,
                end: 7,
                labels: vec![("X".into(), 3)],
                lines: vec![(1, 2), (3, 4)],
            }],
        };
        let json = map.to_json();
        assert!(json.contains("\"main\""));
        assert!(json.contains("\"alphabet\""));
        let back = MapFile::from_json(&json).unwrap();
        assert_eq!(back, map);
        assert!(MapFile::from_json("{not json").is_err());
    }
```

- [ ] **Step 2: RED** → compile error (no serde).

- [ ] **Step 3: Implement** — add the deps, derive `Serialize`/`Deserialize`, and:

```rust
impl MapFile {
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).expect("map serialization is infallible")
    }

    pub fn from_json(s: &str) -> Result<MapFile, String> {
        serde_json::from_str(s).map_err(|e| e.to_string())
    }
}
```

- [ ] **Step 4: GREEN + gates** (Cargo.lock will grow — commit it too).

- [ ] **Step 5: Commit (path-scoped)**

```bash
git commit crates/core/Cargo.toml Cargo.lock crates/core/src/linker/mod.rs -m "feat(core): pmx.map JSON sidecar via serde"
```

---

### Task 5: PM-1 wrapper + linked end-to-end

**Files:**
- Modify: `crates/post-machine/src/asm/mod.rs` (add `link` wrapper)
- Create: `crates/post-machine/tests/link_programs.rs`

**Interfaces:**
- Produces: `mtc_post_machine::asm::link(objects, libraries, options)` — calls core `link` with `pm1_syntax()` and stamps the map's `alphabet` to `[" ", "*"]`.

- [ ] **Step 1: Implementation (thin) + the tests (the substance)**

Wrapper in `asm/mod.rs`:
```rust
pub fn link(
    objects: &[ObjectFile],
    libraries: &[ObjectFile],
    options: mtc_core::linker::LinkOptions,
) -> Result<mtc_core::linker::LinkOutput, mtc_core::linker::LinkError> {
    let mut out = mtc_core::linker::link(&pm1_syntax(), objects, libraries, options)?;
    out.map.alphabet = vec![" ".into(), "*".into()];
    Ok(out)
}
```

`crates/post-machine/tests/link_programs.rs`:

```rust
//! The first LINKED Post-machine programs: assemble → link → run,
//! relaxation economics measured in tacts, and the linked-executable
//! disassembly round trip.

use mtc_core::linker::LinkOptions;
use mtc_core::vm::{ArchRegistry, InfiniteTape, Machine, Outcome, RunOptions, RunStats};
use mtc_post_machine::arch::opcodes::*;
use mtc_post_machine::arch::Pm1;
use mtc_post_machine::asm::{assemble, disassemble_executable, link};

const SPEC_SAMPLE: &str = "\
.func goToEnd
L1:     rgt
        jm      L1
        lft
        ret

.func main
        call    goToEnd
        rgt
        wr      1
        stp
";

fn registry() -> ArchRegistry {
    let mut r = ArchRegistry::new();
    r.register(Box::new(Pm1));
    r
}

#[test]
fn spec_sample_links_byte_exact_and_runs() {
    let obj = assemble(SPEC_SAMPLE, false).unwrap();
    let out = link(&[obj], &[], LinkOptions::default()).unwrap();
    // Layout: main first. Relaxed: main = [ENT][CALL_S off][RGT][WR 81][STP]
    // = 7 bytes; goToEnd at 7 = [ENT][RGT][JM_S FD][LFT][RET].
    // call.s at 1, end 3 → off = 7 − 3 = 4.
    assert_eq!(
        out.executable.code,
        vec![ENT, CALL_S, 0x04, RGT, WR, 0x81, STP, ENT, RGT, JM_S, 0xFD, LFT, RET]
    );
    assert_eq!(out.executable.entry, 0);
    assert_eq!(out.executable.arch, mtc_core::formats::ARCH_PM1);

    // Run on marks [0,1,2], head 0.
    let reg = registry();
    let machine = Machine::from_executable(&out.executable, &reg).unwrap();
    let mut tape = InfiniteTape::from_cells([true, true, true], 0, 0);
    let result = machine.run(&mut tape, RunOptions::default());
    assert_eq!(result.outcome, Outcome::Stopped);
    // goToEnd walks to head 3, lft → head 2, ret; main: rgt → head 3, wr 1.
    assert_eq!(tape.head(), 3);
    assert_eq!(tape.marked_cells(), vec![0, 1, 2, 3]);
    // Tacts (electronic), derived by hand — see plan self-review:
    // core: ent 2 + call.s 5 + [ent 2 + 3×rgt 2 + 3×jm.s 3 + lft 2 + ret 3]
    //       + rgt 2 + wr 3 + stp 1 = 35; stall: moves/writes/latches = 12.
    assert_eq!(result.stats, RunStats { steps: 14, core_tacts: 35, stall_tacts: 12 });
}

#[test]
fn relaxation_saves_exactly_three_fetch_tacts() {
    let obj = assemble(SPEC_SAMPLE, false).unwrap();
    let relaxed = link(&[obj.clone()], &[], LinkOptions::default()).unwrap();
    let far = link(&[obj], &[], LinkOptions { relax: false }).unwrap();
    assert_eq!(far.executable.code.len(), relaxed.executable.code.len() + 3);

    let reg = registry();
    let mut t1 = InfiniteTape::from_cells([true, true, true], 0, 0);
    let mut t2 = InfiniteTape::from_cells([true, true, true], 0, 0);
    let r1 = Machine::from_executable(&relaxed.executable, &reg).unwrap().run(&mut t1, RunOptions::default());
    let r2 = Machine::from_executable(&far.executable, &reg).unwrap().run(&mut t2, RunOptions::default());
    assert_eq!(t1.marked_cells(), t2.marked_cells()); // same behavior
    assert_eq!(r2.stats.core_tacts, r1.stats.core_tacts + 3); // 3 more operand fetches
    assert_eq!(r2.stats.stall_tacts, r1.stats.stall_tacts);
}

#[test]
fn linked_executable_disassembly_reassembles_and_relinks_identically() {
    let obj = assemble(SPEC_SAMPLE, false).unwrap();
    let out = link(&[obj], &[], LinkOptions::default()).unwrap();
    let text = disassemble_executable(&out.executable);
    // Short call prints as far `call` with the synthesized root name:
    assert!(text.contains("call    func_0007"), "{text}");
    assert!(!text.contains("call.s"), "{text}");
    let obj2 = assemble(&text, false).unwrap();
    let out2 = link(&[obj2], &[], LinkOptions::default()).unwrap();
    assert_eq!(out2.executable.code, out.executable.code);
}

#[test]
fn map_names_the_functions() {
    let obj = assemble(SPEC_SAMPLE, true).unwrap();
    let out = link(&[obj], &[], LinkOptions::default()).unwrap();
    assert_eq!(out.map.alphabet, vec![" ".to_string(), "*".to_string()]);
    let names: Vec<&str> = out.map.functions.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(names, vec!["main", "goToEnd"]);
    assert_eq!(out.map.functions[1].labels, vec![("L1".to_string(), 8)]); // ent at 7, L1 at 8
    let json = out.map.to_json();
    assert_eq!(mtc_core::linker::MapFile::from_json(&json).unwrap(), out.map);
}

#[test]
fn report_accounts_for_drops_and_relaxations() {
    let obj = assemble(SPEC_SAMPLE, false).unwrap();
    let lib = assemble(".func spare\n        hlt\n", false).unwrap();
    let out = link(&[obj], &[lib], LinkOptions::default()).unwrap();
    assert_eq!(out.report.dropped, vec!["spare".to_string()]);
    assert_eq!(out.report.relaxed_calls, 1);
    assert_eq!(out.report.far_calls, 0);
}

#[test]
fn library_supplies_go_to_end_lazily() {
    let main_only = assemble(
        ".func main\n        call    goToEnd\n        stp\n",
        false,
    )
    .unwrap();
    let lib = assemble(
        ".func goToEnd\nL:      rgt\n        jm      L\n        lft\n        ret\n.func unusedHelper\n        hlt\n",
        false,
    )
    .unwrap();
    let out = link(&[main_only], &[lib], LinkOptions::default()).unwrap();
    let names: Vec<&str> = out.map.functions.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(names, vec!["main", "goToEnd"]); // unusedHelper dropped
    assert!(!out.executable.code.contains(&HLT));
}
```

- [ ] **Step 2: RED → GREEN**

`cargo test -p mtc-post-machine --test link_programs`. If the byte-exact or tact assertions fail, re-derive by hand first (derivations in the plan's self-review); BLOCKED if you believe an expected value is wrong.

- [ ] **Step 3: Full gates + commit (path-scoped)**

```bash
git commit crates/post-machine -m "feat(post-machine): pm link wrapper; first linked programs with relaxation economics"
```

---

## Self-Review Notes

- **Spec coverage:** §9 pipeline complete — dup/unresolved/no-main errors, lazy libraries + shadowing, reachability DCE, layout, IP-relative patching, relaxation fixpoint with `relax: false` escape (spec's `--no-relax`), `.pmx` emit with entry 0; §10 map sidecar (functions/labels/lines/alphabet, JSON); Plan 3 deferrals resolved: shared decoder, far-mnemonic printing for short calls (round-trip preserved), `.byte` fallback for reloc-less calls, pre-root gap discharged by main-at-0.
- **Approved relax design implemented as decided:** original-blob re-decode each round; widths never change for jumps (shrink-only invariant, debug_assert'ed); call sites tracked by original hole offsets.
- **Hand-derived values (all derived twice):**
  - `links_two_functions_with_relaxed_call`: main 4 bytes `[0E][31 01][02]`, go at 4.
  - `jump_spanning_a_shrunk_call_is_repatched`: orig jmp.s off −8 → post-shrink −5 (`0xFB`); code `[0E][01][31 02][30 FB][02][0E][0B]`.
  - Spec sample linked: 13 bytes, `call.s` off 4, `jm.s` off `0xFD`; run: steps 14, core 35 (= 2+5+2+6+9+2+3+2+3+1), stall 12 (= 3 rgt·2 + lft·2 + rgt·2 + wr·2); final tape `[0,1,2,3]`, head 3.
  - Relaxation delta: exactly +3 bytes and +3 core tacts for the far build, stall unchanged.
  - Map: `goToEnd` at 7..13, label `L1` at absolute 8.
- **Type consistency:** `FuncRef.calls` uses hole offsets (operand start), `Piece::CallSite.orig` uses opcode addresses (hole − 1) — conversion happens once in classification; `MapFile` is plain in Task 3 and gains serde in Task 4 without field changes.
- **Known deferrals:** `-l`/`-L` search paths and writing `app.pmx.map` to disk are CLI concerns (Plan 7); `.pml` archives out of scope (spec §13); map consumption by the disassembler (named exe dis) lands with the CLI.
