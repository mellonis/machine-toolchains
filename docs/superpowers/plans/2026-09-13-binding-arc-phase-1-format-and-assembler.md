# Binding Arc, Phase 1 — Format and Assembler Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Teach the MO object format (v4) and the core assembler/disassembler to carry a routine's interface (parameter names, glyphs, `writes`, exit count, `returns`), exported alphabets and graph digests, symbolic bound sites (parameter names, glyph labels, written-empty maps, exit vectors) and graft provenance — with dis → asm byte identity and no change to any program's behaviour.

**Architecture:** Everything lands in `crates/core` behind a new `AsmCaps::interface` capability (TM-1 on, PM-1 off) and a new MO flag bit, so PM-1 output stays byte-identical and v3 objects keep their bytes. The format layer validates structure only; the linker (phase 2) and compiler (phase 3) are untouched here except that they keep compiling against the widened structs.

**Tech Stack:** Rust (pinned toolchain in `rust-toolchain.toml`), `proptest` for format round trips, `cargo nextest run --workspace` as the final gate, no new dependencies.

**Spec:** `docs/superpowers/specs/2026-09-13-issue-95-binding-analysis.md` — sections "The change / By layer / Assembler" and "Object format", predicates P7 (text-expressibility), P8 (PM-1 byte identity), P9 (headers carry no authority).

## Global Constraints

- **PM-1 byte identity:** no byte of any `pmt` output may change. `pm1_syntax()` does not enable the new capability; `ObjectFile` values with no v4 content serialize exactly as before (v2 or v3).
- **Core neutrality:** `crates/core` learns nothing about TM-1 or PM-1; all tests inside core use the crate-private fake arch (`vm/arch.rs::test_arch`, arch id `0x7F`) and `crate::asm::syntax::fixture::test_syntax()`.
- **Text-expressibility:** every new object record has a hand-writable `.tma` spelling and `tmt dis` prints it; dis → asm must be byte-identical on the new fixtures. The single declared exception stays `-g` debug side-tables.
- **Drift guards are set-compares in both directions:** `recognized_directives(caps)` ↔ the real recognizer (`crates/core/src/asm/cst.rs`, test `recognized_directives_match_the_real_recognizer`, currently pinned at 13 words) ↔ the editor grammars (`crates/turing-machine/tests/editor_grammar.rs`, `crates/post-machine/tests/editor_grammar.rs`). Every task that adds a directive updates all three or fails.
- **Docs policy:** published pages (`docs/formats.md`, `docs/tmt/asm.md`) cite no issues, PRs or hosting URLs; code comments cite `docs/<page>.md (keyword)` only.
- **Commits:** conventional commits with scope (`feat(core):`, `test(core):`, `docs(formats):`). The repository owner runs every `git commit` personally; a step that says "Commit" means "stop and hand the diff to the owner with the message below".
- **Temp paths in tests:** PID + per-call atomic counter, never a fixed name.
- **Wire constants used throughout:** `OBJECT_FORMAT_VERSION_V4 = 4`, `FLAG_HAS_INTERFACE = 0b0010_0000` (flags bit 5), `NO_STRING: u32 = 0xFFFF_FFFF` (a "no string index" sentinel, reusing the value `EXTERNAL_BLOB` already uses for "no blob").

---

## File Structure

| File | Responsibility in this phase |
|---|---|
| `crates/core/src/asm/syntax.rs` | `AsmCaps::interface` field |
| `crates/core/src/formats/object.rs` | v4 records (`RoutineInterface`, `ExportedAlphabet`, `ExportedGraph`, `Interface`, `GraftProvenance`), widened `TapeBinding`/`MapPair`/`BoundCall`, v4 writer/reader, shape selection |
| `crates/core/tests/format_roundtrips.rs` | proptest round trips over v4 |
| `crates/core/src/asm/lexer.rs` | `Glyph` token under the interface cap |
| `crates/core/src/asm/cst.rs` | `.param`, `.graph`, `.grafted` directive nodes; `.routine` gains `exits=`/`noreturn`; `recognized_directives` inventory; named binding entries with glyph labels; `exits=(…)` operand |
| `crates/core/src/asm/lower.rs` | lowering + validation of the new directives; `SourceTapeBinding`/`SourceOperand::BoundCallOp` widened |
| `crates/core/src/asm/assembler.rs` | writes the new records into `ObjectFile` |
| `crates/core/src/asm/disassembler.rs` | prints the new directives and operand forms |
| `crates/core/src/asm/fmt.rs` | canonical grid for the new directive lines (they are `Structural` pieces like `.routine`) |
| `crates/turing-machine/src/asm/mod.rs` | `tm1_syntax()` enables `interface` |
| `crates/turing-machine/tests/tma_dialect.rs` | end-to-end round trip of a fixture using every new form |
| `editors/grammars/tma.tmLanguage.json` | paints the three new directives |
| `docs/formats.md`, `docs/tmt/asm.md`, `docs/core.md` | the MO v4 layout, the directive and operand grammar, the assembler-framework capability list |

---

### Task 1: The `interface` capability

**Files:**
- Modify: `crates/core/src/asm/syntax.rs:37-58` (`AsmCaps`)
- Modify: `crates/turing-machine/src/asm/mod.rs:200-207` (`tm1_syntax()` caps literal)
- Test: `crates/core/src/asm/cst.rs` (existing `recognized_directives_match_the_real_recognizer` — its tier list)

**Interfaces:**
- Produces: `AsmCaps { tables, rept, vectors, volatile, interface: bool }`. Every later task gates on `caps.interface`.

- [ ] **Step 1: Write the failing test**

Add to the `tiers` array in `recognized_directives_match_the_real_recognizer` (`crates/core/src/asm/cst.rs`, inside `mod tests`):

```rust
            AsmCaps {
                interface: true,
                ..AsmCaps::default()
            },
```

and change the `all_on` literal to spell the new field:

```rust
        let all_on = AsmCaps {
            tables: true,
            rept: true,
            vectors: true,
            volatile: true,
            interface: true,
        };
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p mtc-core recognized_directives_match_the_real_recognizer`
Expected: compile error `no field 'interface' on type 'AsmCaps'`.

- [ ] **Step 3: Add the field**

In `crates/core/src/asm/syntax.rs`, after the `volatile` field:

```rust
    /// The interface surface (docs/formats.md (routine interfaces)): the
    /// `.param` directive (parameter names, glyphs, `writes` sets), the
    /// `exits=`/`noreturn` fields of `.routine`, the object-level
    /// `.graph`/`.grafted` digest directives, quoted glyph labels and
    /// named entries in a binding-call operand, and the `exits=(…)`
    /// operand on `call`. Off by default like every capability; PM-1
    /// never enables it (docs/pmt/asm.md).
    pub interface: bool,
```

In `crates/turing-machine/src/asm/mod.rs`, in the `tm1_syntax()` caps literal, after `volatile: false,`:

```rust
            interface: true,
```

`pm1_syntax()` uses `..AsmCaps::default()` and needs no change.

- [ ] **Step 4: Run the guard and the PM-1 identity gates**

Run: `cargo test -p mtc-core recognized_directives_match_the_real_recognizer && cargo test -p mtc-post-machine --test asm_volatile && cargo test -p mtc-post-machine --test golden_programs`
Expected: all PASS (the inventory is still 13 words; the new tier lists the same words as `default()`).

- [ ] **Step 5: Commit**

```bash
git add crates/core/src/asm/syntax.rs crates/turing-machine/src/asm/mod.rs crates/core/src/asm/cst.rs
git commit -m "feat(core): AsmCaps::interface, the capability the binding arc's assembler surface rides on"
```

---

### Task 2: MO v4 records and the widened bound-call structs

**Files:**
- Modify: `crates/core/src/formats/object.rs:1-30` (constants), `:55-88` (`ObjectFile`), `:151-175` (`MapPair`, `TapeBinding`, `BoundCall`), `:200-235` (`v2`, `is_v2_shape`)
- Test: `crates/core/src/formats/object.rs` `mod tests`

**Interfaces:**
- Produces (public, used by every later phase):

```rust
pub const OBJECT_FORMAT_VERSION_V4: u16 = 4;

/// One routine's interface, parallel to `blobs` (docs/formats.md (routine interfaces)).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoutineInterface {
    pub params: Vec<String>,        // len == arity
    pub glyphs: Vec<Vec<String>>,   // len == arity; glyphs[k].len() == signature cardinalities[k]
    pub writes: Vec<Vec<String>>,   // len == arity; each element a subset of glyphs[k]
    pub exits: u8,                  // number of state parameters
    pub returns: bool,              // false = `noreturn`
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportedAlphabet { pub name: String, pub glyphs: Vec<String> }

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportedGraph { pub name: String, pub digest: u32 }

/// The object's interface section (flags bit 5).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Interface {
    pub routines: Vec<RoutineInterface>,   // parallel to blobs
    pub alphabets: Vec<ExportedAlphabet>,
    pub graphs: Vec<ExportedGraph>,
}

/// A library graph this unit spliced, with the digest of the body it spliced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GraftProvenance { pub graph: String, pub digest: u32 }

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MapPair {
    pub src: u32,
    pub dst: u32,                    // ignored when dst_label is Some
    pub dst_label: Option<String>,   // a glyph label the linker resolves (v4 only)
    pub one_way: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TapeBinding {
    pub caller_tape: u8,
    pub param: Option<String>,       // callee parameter name (v4 only); None = positional
    pub map_written: bool,           // `1{}` (true) versus `1` (false); v4 only
    pub pairs: Vec<MapPair>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoundCall {
    pub blob: u32,
    pub offset: u32,
    pub symbol: u32,
    pub binding: Vec<TapeBinding>,
    pub exits: Vec<u32>,             // blob-relative code offsets; v4 only
}
```

and on `ObjectFile`: `pub interface: Option<Interface>` and `pub grafts: Vec<GraftProvenance>`.

- `MapPair` loses `Copy` (it now holds a `String`). Every `MapPair { src, dst, one_way }` literal in the workspace gains `dst_label: None`; every `TapeBinding { caller_tape, pairs }` literal gains `param: None, map_written: false`; every `BoundCall { … }` literal gains `exits: Vec::new()`; every `ObjectFile { … }` literal gains `interface: None, grafts: Vec::new()`. `ObjectFile::v2(...)` sets both to absent.

- [ ] **Step 1: Write the failing tests**

Append to `mod tests` in `crates/core/src/formats/object.rs`:

```rust
    fn v4_sample() -> ObjectFile {
        let mut obj = sample();
        obj.signatures = Some(vec![RoutineSig {
            arity: 1,
            cardinalities: vec![3],
        }]);
        obj.interface = Some(Interface {
            routines: vec![RoutineInterface {
                params: vec!["num".into()],
                glyphs: vec![vec!["_".into(), "0".into(), "1".into()]],
                writes: vec![vec!["0".into(), "1".into()]],
                exits: 0,
                returns: true,
            }],
            alphabets: vec![ExportedAlphabet {
                name: "bits".into(),
                glyphs: vec!["_".into(), "0".into(), "1".into()],
            }],
            graphs: vec![ExportedGraph {
                name: "lib::findA".into(),
                digest: 0xDEAD_BEEF,
            }],
        });
        obj.grafts = vec![GraftProvenance {
            graph: "other::g".into(),
            digest: 0x1234_5678,
        }];
        obj
    }

    #[test]
    fn v4_shape_is_detected_and_v3_shape_stays_v3() {
        assert!(!sample().is_v4_shape());
        assert!(v4_sample().is_v4_shape());
        let mut symbolic = sample();
        symbolic.bound_calls.push(BoundCall {
            blob: 0,
            offset: 1,
            symbol: 0,
            binding: vec![TapeBinding {
                caller_tape: 0,
                param: Some("num".into()),
                map_written: false,
                pairs: Vec::new(),
            }],
            exits: Vec::new(),
        });
        assert!(symbolic.is_v4_shape(), "a named binding entry needs v4");
    }
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p mtc-core v4_shape_is_detected`
Expected: compile errors on the missing types and fields.

- [ ] **Step 3: Add the types and fields**

In `crates/core/src/formats/object.rs`, after `OBJECT_FORMAT_VERSION_V3`:

```rust
/// Version 4 appends the interface section (flags bit 5), the
/// graft-provenance list, and widens bound-call records with parameter
/// names, glyph labels, the written-empty-map flag and exit vectors
/// (docs/formats.md (.pmo)).
pub const OBJECT_FORMAT_VERSION_V4: u16 = 4;
```

After `FLAG_PROGRAM_VOLATILE`:

```rust
/// v4: the interface section is present (docs/formats.md (routine interfaces)).
const FLAG_HAS_INTERFACE: u8 = 0b0010_0000;
/// "No string" in a string-index field (the sentinel `EXTERNAL_BLOB` already uses for "no blob").
const NO_STRING: u32 = 0xFFFF_FFFF;
```

Add the five record types exactly as in **Interfaces** above, placed after `BoundCall`. Replace the `MapPair`, `TapeBinding` and `BoundCall` definitions with the widened ones from **Interfaces** (drop `Copy` from `MapPair`'s derive). Add to `ObjectFile`:

```rust
    /// The interface section, present iff flags bit 5 (v4). Requires
    /// `signatures` to be present too: an interface describes a signed routine.
    pub interface: Option<Interface>,
    /// Library graphs this unit spliced, with the digest of each body it
    /// spliced (v4; the linker compares against the exporter's `Interface::graphs`).
    pub grafts: Vec<GraftProvenance>,
```

In `ObjectFile::v2(...)` add `interface: None, grafts: Vec::new()` to the constructed value. Add next to `is_v2_shape`:

```rust
    /// True when any v4-only content is present: an interface section, a
    /// graft-provenance record, or a bound call carrying a parameter name,
    /// a glyph label, a written-empty map or an exit vector. Such an object
    /// serializes as v4; anything else keeps its v2/v3 bytes.
    pub fn is_v4_shape(&self) -> bool {
        self.interface.is_some()
            || !self.grafts.is_empty()
            || self.bound_calls.iter().any(|bc| {
                !bc.exits.is_empty()
                    || bc.binding.iter().any(|tb| {
                        tb.param.is_some()
                            || tb.map_written
                            || tb.pairs.iter().any(|p| p.dst_label.is_some())
                    })
            })
    }
```

Fix every literal the compiler now rejects (`cargo build --workspace` lists them: `object.rs` tests, `format_roundtrips.rs`, `linker/resolve.rs` tests, `linker/compose.rs` tests, `core/tests/link_tables.rs`, `asm/assembler.rs::source_binding_to_object`, `asm/disassembler.rs` tests, `turing-machine/src/ir.rs` codegen of bound calls if it constructs `MapPair`, `wasm/src/inner/*` if any). Each fix is the additive default named in **Interfaces**.

- [ ] **Step 4: Run the tests and the whole workspace build**

Run: `cargo build --workspace --all-targets && cargo test -p mtc-core v4_shape_is_detected`
Expected: build clean, test PASS.

- [ ] **Step 5: Commit**

```bash
git add -A crates
git commit -m "feat(core): MO v4 record types — routine interfaces, exported alphabets and graph digests, graft provenance, symbolic bound sites"
```

---

### Task 3: v4 writer and reader

**Files:**
- Modify: `crates/core/src/formats/object.rs:235-560` (`to_bytes`, `to_bytes_v3` → shared v3/v4 body), `:560-860` (`from_bytes`)
- Test: `crates/core/src/formats/object.rs` `mod tests`; `crates/core/tests/format_roundtrips.rs`

**Interfaces:**
- Consumes: Task 2's types.
- Produces: `ObjectFile::to_bytes` emits v4 iff `is_v4_shape()`; `from_bytes` accepts `1..=4`.

**Wire layout (v4 = the v3 layout, then):**

```
interface (present iff flags bit 5; requires bit 1), once per blob:
                per tape (arity from the signature): u32 param name (string index),
                    u8 glyph count, count × u32 glyph (string index),
                    u8 writes count, count × u32 glyph (string index)
                u8 exit count, u8 returns (0/1)
            then: u32 alphabet count, per alphabet: u32 name, u8 glyph count, count × u32 glyph
                  u32 graph count, per graph: u32 name, u32 digest
graft provenance (unconditional in v4): u32 count, per graft: u32 graph name, u32 digest
```

and the bound-call record becomes, per tape binding: `u8 caller tape, u32 param (or NO_STRING), u8 binding flags (bit 0 = map written), u16 pair count, per pair: u32 src, u32 dst, u8 flags (bit 0 = one-way, bit 1 = dst is a string index)`; per bound call after its bindings: `u8 exit count, count × u32 exit offset`. The v3 reader path keeps reading the v3 shape (no param, no binding flags, no exits) and rejects pair flag bit 1.

- [ ] **Step 1: Write the failing tests**

Append to `mod tests`:

```rust
    #[test]
    fn v4_full_round_trip() {
        let mut obj = v4_sample();
        obj.bound_calls.push(BoundCall {
            blob: 0,
            offset: 1,
            symbol: 0,
            binding: vec![TapeBinding {
                caller_tape: 1,
                param: Some("num".into()),
                map_written: true,
                pairs: vec![
                    MapPair { src: 3, dst: 0, dst_label: Some("0".into()), one_way: false },
                    MapPair { src: 4, dst: 2, dst_label: None, one_way: true },
                ],
            }],
            exits: vec![4],
        });
        let bytes = obj.to_bytes();
        assert_eq!(u16::from_le_bytes([bytes[3], bytes[4]]), OBJECT_FORMAT_VERSION_V4);
        assert_eq!(ObjectFile::from_bytes(&bytes).unwrap(), obj);
    }

    #[test]
    fn v3_content_keeps_its_v3_bytes() {
        let obj = sample_v3(); // the existing v3 fixture used by v3_full_round_trip_preserves_one_way
        let bytes = obj.to_bytes();
        assert_eq!(u16::from_le_bytes([bytes[3], bytes[4]]), OBJECT_FORMAT_VERSION_V3);
    }

    #[test]
    fn interface_without_signatures_is_rejected_by_the_writer_contract() {
        let mut obj = v4_sample();
        obj.signatures = None;
        let bytes = obj.to_bytes();
        assert!(matches!(
            ObjectFile::from_bytes(&bytes),
            Err(FormatError::Malformed("interface without signatures"))
        ));
    }

    #[test]
    fn interface_glyph_count_must_match_cardinality() {
        let mut obj = v4_sample();
        obj.interface.as_mut().unwrap().routines[0].glyphs[0].push("x".into());
        let bytes = obj.to_bytes();
        assert!(matches!(
            ObjectFile::from_bytes(&bytes),
            Err(FormatError::Malformed("interface glyph count differs from cardinality"))
        ));
    }

    #[test]
    fn v4_bound_call_exit_offset_must_lie_in_blob() {
        let mut obj = v4_sample();
        obj.bound_calls.push(BoundCall {
            blob: 0, offset: 1, symbol: 0,
            binding: vec![TapeBinding { caller_tape: 0, param: None, map_written: false, pairs: vec![] }],
            exits: vec![10_000],
        });
        let bytes = obj.to_bytes();
        assert!(matches!(
            ObjectFile::from_bytes(&bytes),
            Err(FormatError::Malformed("bound-call exit outside blob"))
        ));
    }

    #[test]
    fn pre_v4_object_claiming_flag_has_interface_rejected() {
        let mut bytes = sample_v3().to_bytes();
        bytes[6] |= 0b0010_0000; // flags byte (magic 3 + version 2 + arch 1)
        let crc = crate::formats::crc32::crc32(&bytes[11..]);
        bytes[7..11].copy_from_slice(&crc.to_le_bytes());
        assert!(matches!(
            ObjectFile::from_bytes(&bytes),
            Err(FormatError::Malformed("v4 flags in pre-v4 object"))
        ));
    }
```

(If `sample_v3()` does not exist under that name, use the fixture the existing `v3_full_round_trip_preserves_one_way` test builds, extracted into a helper with that name.)

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p mtc-core --lib formats::object`
Expected: `v4_full_round_trip` FAILS (writer still emits v3 and drops the new fields; or the reader rejects), the rejection tests FAIL on the wrong error.

- [ ] **Step 3: Implement the writer**

Rename `to_bytes_v3` to `to_bytes_v3_or_v4` and make `to_bytes` dispatch:

```rust
    pub fn to_bytes(&self) -> Vec<u8> {
        if self.is_v2_shape() {
            self.to_bytes_v2()
        } else if self.is_v4_shape() {
            self.to_bytes_v3_or_v4(OBJECT_FORMAT_VERSION_V4)
        } else {
            self.to_bytes_v3_or_v4(OBJECT_FORMAT_VERSION_V3)
        }
    }
```

Inside `to_bytes_v3_or_v4(&self, version: u16)`: intern, ahead of the pool dump, every string the v4 sections use (parameter names, glyphs, writes, alphabet names and glyphs, graph names, graft names, `dst_label`s, `param`s) — the pool must be complete before it is written; set `flags |= FLAG_HAS_INTERFACE` when `version == V4 && self.interface.is_some()`; write the version passed in. Replace the bound-call loop's inner body with:

```rust
        for call in &self.bound_calls {
            put_u32(&mut out, call.blob);
            put_u32(&mut out, call.offset);
            put_u32(&mut out, call.symbol);
            out.push(u8::try_from(call.binding.len()).expect("tape count fits u8"));
            for tape in &call.binding {
                out.push(tape.caller_tape);
                if version >= OBJECT_FORMAT_VERSION_V4 {
                    put_u32(&mut out, tape.param.as_deref().map_or(NO_STRING, |p| pool.intern(p)));
                    out.push(u8::from(tape.map_written));
                }
                put_u16(&mut out, u16::try_from(tape.pairs.len()).expect("pair count fits u16"));
                for pair in &tape.pairs {
                    put_u32(&mut out, pair.src);
                    let (dst, label_bit) = match &pair.dst_label {
                        Some(label) => (pool.intern(label), 0b10),
                        None => (pair.dst, 0),
                    };
                    put_u32(&mut out, dst);
                    out.push(u8::from(pair.one_way) | label_bit);
                }
            }
            if version >= OBJECT_FORMAT_VERSION_V4 {
                out.push(u8::try_from(call.exits.len()).expect("exit count fits u8"));
                for &e in &call.exits {
                    put_u32(&mut out, e);
                }
            }
        }
```

(`pool.intern` inside the loop must return the index assigned during the pre-pass, which it does because `intern` is idempotent; the pre-pass exists only so the pool's byte dump precedes its use.) After the bound-call section, when `version >= V4`:

```rust
            if let Some(iface) = &self.interface {
                debug_assert_eq!(iface.routines.len(), self.blobs.len(), "interface must parallel blobs");
                for r in &iface.routines {
                    for k in 0..r.params.len() {
                        put_u32(&mut out, pool.intern(&r.params[k]));
                        out.push(u8::try_from(r.glyphs[k].len()).expect("glyph count fits u8"));
                        for g in &r.glyphs[k] { put_u32(&mut out, pool.intern(g)); }
                        out.push(u8::try_from(r.writes[k].len()).expect("writes count fits u8"));
                        for g in &r.writes[k] { put_u32(&mut out, pool.intern(g)); }
                    }
                    out.push(r.exits);
                    out.push(u8::from(r.returns));
                }
                put_u32(&mut out, u32::try_from(iface.alphabets.len()).expect("alphabet count fits u32"));
                for a in &iface.alphabets {
                    put_u32(&mut out, pool.intern(&a.name));
                    out.push(u8::try_from(a.glyphs.len()).expect("glyph count fits u8"));
                    for g in &a.glyphs { put_u32(&mut out, pool.intern(g)); }
                }
                put_u32(&mut out, u32::try_from(iface.graphs.len()).expect("graph count fits u32"));
                for g in &iface.graphs {
                    put_u32(&mut out, pool.intern(&g.name));
                    put_u32(&mut out, g.digest);
                }
            }
            put_u32(&mut out, u32::try_from(self.grafts.len()).expect("graft count fits u32"));
            for g in &self.grafts {
                put_u32(&mut out, pool.intern(&g.graph));
                put_u32(&mut out, g.digest);
            }
```

- [ ] **Step 4: Implement the reader**

In `from_bytes`: widen the version gate to `1..=OBJECT_FORMAT_VERSION_V4`. In the v3-sections branch, read the bound-call bindings with the version in hand:

```rust
                        let caller_tape = r.u8()?;
                        if caller_tape >= 16 { return Err(FormatError::Malformed("bound-call caller tape >= 16")); }
                        let (param, map_written) = if version >= OBJECT_FORMAT_VERSION_V4 {
                            let p = r.u32()?;
                            let param = if p == NO_STRING { None } else { Some(name_of(p)?) };
                            let bflags = r.u8()?;
                            if bflags & !1 != 0 { return Err(FormatError::Malformed("reserved binding flags")); }
                            (param, bflags & 1 != 0)
                        } else {
                            (None, false)
                        };
                        let pair_count = r.u16()? as usize;
                        let mut pairs = Vec::with_capacity(pair_count.min(1 << 12));
                        for _ in 0..pair_count {
                            let src = r.u32()?;
                            let dst = r.u32()?;
                            let flags_byte = r.u8()?;
                            let allowed = if version >= OBJECT_FORMAT_VERSION_V4 { 0b11 } else { 0b01 };
                            if flags_byte & !allowed != 0 {
                                return Err(FormatError::Malformed("reserved map-pair flags"));
                            }
                            let dst_label = if flags_byte & 0b10 != 0 { Some(name_of(dst)?) } else { None };
                            pairs.push(MapPair { src, dst: if dst_label.is_some() { 0 } else { dst }, dst_label, one_way: flags_byte & 1 != 0 });
                        }
                        binding.push(TapeBinding { caller_tape, param, map_written, pairs });
```

and after the bindings of each call:

```rust
                    let exits = if version >= OBJECT_FORMAT_VERSION_V4 {
                        let n = r.u8()? as usize;
                        let mut exits = Vec::with_capacity(n);
                        for _ in 0..n {
                            let e = r.u32()?;
                            if u64::from(e) >= code.len() as u64 {
                                return Err(FormatError::Malformed("bound-call exit outside blob"));
                            }
                            exits.push(e);
                        }
                        exits
                    } else { Vec::new() };
```

(`code` is the blob the existing offset check already fetched.) Then the v4 sections:

```rust
        let (interface, grafts) = if version >= OBJECT_FORMAT_VERSION_V4 {
            let interface = if flags & FLAG_HAS_INTERFACE != 0 {
                let Some(sigs) = &signatures else {
                    return Err(FormatError::Malformed("interface without signatures"));
                };
                let mut routines = Vec::with_capacity(blob_count);
                for sig in sigs {
                    let mut params = Vec::new();
                    let mut glyphs = Vec::new();
                    let mut writes = Vec::new();
                    for &card in &sig.cardinalities {
                        params.push(name_of(r.u32()?)?);
                        let n = r.u8()? as usize;
                        if n as u32 != card {
                            return Err(FormatError::Malformed("interface glyph count differs from cardinality"));
                        }
                        let mut gl = Vec::with_capacity(n);
                        for _ in 0..n { gl.push(name_of(r.u32()?)?); }
                        let w = r.u8()? as usize;
                        let mut ws = Vec::with_capacity(w);
                        for _ in 0..w {
                            let g = name_of(r.u32()?)?;
                            if !gl.contains(&g) {
                                return Err(FormatError::Malformed("writes glyph outside its alphabet"));
                            }
                            ws.push(g);
                        }
                        glyphs.push(gl);
                        writes.push(ws);
                    }
                    let exits = r.u8()?;
                    let returns = match r.u8()? { 0 => false, 1 => true, _ => return Err(FormatError::Malformed("returns byte")) };
                    routines.push(RoutineInterface { params, glyphs, writes, exits, returns });
                }
                let alphabet_count = r.u32()? as usize;
                let mut alphabets = Vec::new();
                for _ in 0..alphabet_count {
                    let name = name_of(r.u32()?)?;
                    let n = r.u8()? as usize;
                    let mut gl = Vec::with_capacity(n);
                    for _ in 0..n { gl.push(name_of(r.u32()?)?); }
                    alphabets.push(ExportedAlphabet { name, glyphs: gl });
                }
                let graph_count = r.u32()? as usize;
                let mut graphs = Vec::new();
                for _ in 0..graph_count {
                    graphs.push(ExportedGraph { name: name_of(r.u32()?)?, digest: r.u32()? });
                }
                Some(Interface { routines, alphabets, graphs })
            } else { None };
            let graft_count = r.u32()? as usize;
            let mut grafts = Vec::new();
            for _ in 0..graft_count {
                grafts.push(GraftProvenance { graph: name_of(r.u32()?)?, digest: r.u32()? });
            }
            (interface, grafts)
        } else {
            if flags & FLAG_HAS_INTERFACE != 0 {
                return Err(FormatError::Malformed("v4 flags in pre-v4 object"));
            }
            (None, Vec::new())
        };
```

Reuse the existing "huge count" discipline (`huge_wire_count_is_rejected_without_allocating`): every `Vec::with_capacity` above caps at a small bound or is replaced by `Vec::new()` + push, matching the file's current practice. Finish the constructed `ObjectFile` with `interface, grafts`.

- [ ] **Step 5: Run the module tests**

Run: `cargo test -p mtc-core --lib formats::object`
Expected: all PASS, including every pre-existing v2/v3 test unchanged.

- [ ] **Step 6: Extend the property tests**

Append to `crates/core/tests/format_roundtrips.rs` inside the `proptest!` block:

```rust
    #[test]
    fn mo_v4_round_trip(
        blob in proptest::collection::vec(any::<u8>(), 6..32),
        params in proptest::collection::vec("[a-z]{1,6}", 1..4),
        digest in any::<u32>(),
        exit in 0u32..4,
    ) {
        let arity = params.len() as u8;
        let glyphs: Vec<Vec<String>> = (0..arity).map(|k| vec!["_".to_string(), format!("g{k}")]).collect();
        let obj = ObjectFile {
            arch: 0x7F,
            symbols: vec![
                Symbol { name: "f".into(), def: SymbolDef::Defined { blob: 0 } },
                Symbol { name: "ext".into(), def: SymbolDef::External },
            ],
            blobs: vec![blob.clone()],
            relocations: Vec::new(),
            debug: None,
            signatures: Some(vec![RoutineSig { arity, cardinalities: vec![2; arity as usize] }]),
            table_blobs: None,
            table_fixups: Vec::new(),
            bound_calls: vec![BoundCall {
                blob: 0,
                offset: 1,
                symbol: 1,
                binding: (0..arity).map(|k| TapeBinding {
                    caller_tape: k,
                    param: Some(params[k as usize].clone()),
                    map_written: k == 0,
                    pairs: vec![MapPair { src: 1, dst: 0, dst_label: Some(format!("g{k}")), one_way: false }],
                }).collect(),
                exits: vec![exit],
            }],
            variants: None,
            program_volatile: false,
            interface: Some(Interface {
                routines: vec![RoutineInterface {
                    params: params.clone(),
                    glyphs: glyphs.clone(),
                    writes: glyphs.iter().map(|g| vec![g[1].clone()]).collect(),
                    exits: 1,
                    returns: false,
                }],
                alphabets: vec![ExportedAlphabet { name: "ab".into(), glyphs: glyphs[0].clone() }],
                graphs: vec![ExportedGraph { name: "g".into(), digest }],
            }),
            grafts: vec![GraftProvenance { graph: "lib::g".into(), digest }],
        };
        let bytes = obj.to_bytes();
        prop_assert_eq!(ObjectFile::from_bytes(&bytes).unwrap(), obj);
    }
```

`object_never_panics_on_noise` already covers the reader on arbitrary bytes; no new noise test is needed, but run it.

- [ ] **Step 7: Run the property tests**

Run: `cargo test -p mtc-core --test format_roundtrips`
Expected: PASS.

- [ ] **Step 8: Commit**

```bash
git add crates/core/src/formats/object.rs crates/core/tests/format_roundtrips.rs
git commit -m "feat(core): MO v4 writer and reader — interface section, graft provenance, symbolic bound sites with exits"
```

---

### Task 4: `Glyph` tokens in the assembler lexer

**Files:**
- Modify: `crates/core/src/asm/lexer.rs:9-64` (`AsmTokenKind`), `:184-260` (`lex_line`)
- Test: `crates/core/src/asm/lexer.rs` `mod tests`

**Interfaces:**
- Produces: `AsmTokenKind::Glyph(String)` — the decoded glyph text (escapes resolved), emitted only when `caps.interface` is on; width for span purposes = the source length including quotes. With the cap off, `'` stays `Junk('\'')` exactly as today.

- [ ] **Step 1: Write the failing tests**

Append to `mod tests` in `lexer.rs` (use the file's existing helpers for kinds and caps):

```rust
    #[test]
    fn glyph_literals_lex_under_the_interface_cap() {
        let caps = AsmCaps { interface: true, ..AsmCaps::default() };
        let kinds: Vec<AsmTokenKind> = lex_line("'_', 'a', '\\'', '\\\\'", 1, caps)
            .into_iter().map(|t| t.kind).collect();
        assert_eq!(
            kinds,
            vec![
                AsmTokenKind::Glyph("_".into()), AsmTokenKind::Comma,
                AsmTokenKind::Glyph("a".into()), AsmTokenKind::Comma,
                AsmTokenKind::Glyph("'".into()), AsmTokenKind::Comma,
                AsmTokenKind::Glyph("\\".into()),
            ]
        );
    }

    #[test]
    fn glyph_span_covers_the_quotes() {
        let caps = AsmCaps { interface: true, ..AsmCaps::default() };
        let toks = lex_line("  '\\''", 1, caps);
        assert_eq!(toks[0].col, 2);
        assert_eq!(toks[0].len, 4);
    }

    #[test]
    fn quote_is_junk_without_the_interface_cap() {
        let toks = lex_line("'a'", 1, AsmCaps::default());
        assert!(matches!(toks[0].kind, AsmTokenKind::Junk('\'')));
    }

    #[test]
    fn unterminated_glyph_is_junk() {
        let caps = AsmCaps { interface: true, ..AsmCaps::default() };
        let toks = lex_line("'a", 1, caps);
        assert!(matches!(toks[0].kind, AsmTokenKind::Junk('\'')));
    }
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p mtc-core --lib asm::lexer`
Expected: compile error on `AsmTokenKind::Glyph`.

- [ ] **Step 3: Implement**

Add the variant after `FatArrow`:

```rust
    /// `'x'` (interface cap): a quoted glyph literal, decoded (`\'` and
    /// `\\` are the only escapes, mirroring `formats::glyphs`). The span
    /// covers the quotes.
    Glyph(String),
```

In `lex_line`, in the character dispatch before the `Junk` fallback, when `caps.interface` and the current char is `'`:

```rust
            '\'' if caps.interface => {
                // '<char>' or '\'' or '\\' — anything else stays Junk.
                let rest: Vec<char> = chars[i + 1..].iter().copied().collect();
                let (decoded, consumed) = match rest.as_slice() {
                    ['\\', '\'', '\'', ..] => ("'".to_string(), 4),
                    ['\\', '\\', '\'', ..] => ("\\".to_string(), 4),
                    [c, '\'', ..] if *c != '\\' && *c != '\'' => (c.to_string(), 3),
                    _ => {
                        tokens.push(AsmToken { kind: AsmTokenKind::Junk('\''), line: line_no, col: i, len: 1 });
                        i += 1;
                        continue;
                    }
                };
                tokens.push(AsmToken { kind: AsmTokenKind::Glyph(decoded), line: line_no, col: i, len: consumed });
                i += consumed;
                continue;
            }
```

Adapt the variable names (`chars`, `i`, `tokens`, `line_no`) to the ones `lex_line` actually uses; the width table near `lexer.rs:594` (`Arrow | FatArrow => 2`) gains no entry because `Glyph` carries its own `len`.

- [ ] **Step 4: Run the lexer tests**

Run: `cargo test -p mtc-core --lib asm::lexer`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/core/src/asm/lexer.rs
git commit -m "feat(core): quoted glyph tokens in the assembler lexer, under the interface capability"
```

---

### Task 5: `.param`, `exits=`/`noreturn` on `.routine`, `.graph`, `.grafted` — CST and lowering

**Files:**
- Modify: `crates/core/src/asm/cst.rs:40-110` (words, `recognized_directives`), `:206` (`AsmItemKind`), `:352-372` (`RoutineDirectiveCst`), `:1212-1275` (`routine_directive`), the shaping dispatch that recognizes `ROUTINE_WORD`
- Modify: `crates/core/src/asm/lower.rs:291-320` (`LowerCtx`), `:370-400` (end-of-input assembly of `signatures`), `:680-760` (`lower_routine_directive`), `LoweredSource`
- Test: `crates/core/src/asm/cst.rs` (`recognized_directives_match_the_real_recognizer`), `crates/core/src/asm/lower.rs` `mod tests`

**Interfaces:**
- Produces in `cst.rs`: `PARAM_WORD = ".param"`, `GRAPH_WORD = ".graph"`, `GRAFTED_WORD = ".grafted"`, all three in `recognized_directives` under `caps.interface`; `RoutineDirectiveCst` gains `exits: Option<(u32, Span)>` and `noreturn: Option<Span>`; new nodes:

```rust
/// `.param <name>, (<glyphs>)[, writes=(<glyphs>)]` — one tape of the
/// pending `.routine`'s interface, in tape order.
pub struct ParamDirectiveCst {
    pub name: String, pub name_span: Span,
    pub glyphs: Vec<String>, pub glyphs_span: Span,
    pub writes: Option<Vec<String>>, pub writes_span: Option<Span>,
    pub span: Span, pub trailing: Option<TrailingComment>,
}
/// `.graph <name>, <digest>` (an exported graph's digest) and
/// `.grafted <name>, <digest>` (a library graph this unit spliced).
pub struct DigestDirectiveCst {
    pub grafted: bool,
    pub name: String, pub name_span: Span,
    pub digest: u32, pub digest_span: Span,
    pub span: Span, pub trailing: Option<TrailingComment>,
}
```

  and `AsmItemKind::ParamDirective(ParamDirectiveCst)`, `AsmItemKind::DigestDirective(DigestDirectiveCst)`.
- Produces in `lower.rs`: `LoweredSource` gains `interface: Option<Interface>` (routines parallel to `functions`, alphabets empty — the assembler never authors alphabets, see the note below), `grafts: Vec<GraftProvenance>`; the pending-signature mechanism carries `(RoutineSig, Option<RoutineInterface>)`.
- Note: exported alphabets are a *compiler* fact (a `.tmc` declaration), not something a hand-written `.tma` states, so the assembler leaves `Interface::alphabets` empty and the compiler (phase 3) fills it after assembly. This is the one interface field with no `.tma` spelling; it is recorded as such in `docs/formats.md` and does not affect dis → asm identity of the *code*, but `tmt dis` prints exported alphabets as a comment block so the object stays inspectable.

- [ ] **Step 1: Write the failing tests**

In `cst.rs` tests, change the inventory assertion to 16 and add the interface tier probe words. The probe helper `recognized_probe(word)` must produce a well-formed line for each new word; add arms:

```rust
            ".param" => ".routine probe, tapes=1, alpha=(2)\n.param t, ('_', 'a')\n.func probe\nstop\n".to_string(),
            ".graph" => ".graph g, 1\n.func probe\nstop\n".to_string(),
            ".grafted" => ".grafted g, 1\n.func probe\nstop\n".to_string(),
```

In `lower.rs` tests (use the file's existing `lower_ok`/`lower_err` style helpers with `test_syntax()` and `interface: true` caps):

```rust
    fn iface_caps() -> AsmCaps {
        AsmCaps { tables: true, interface: true, ..AsmCaps::default() }
    }

    #[test]
    fn param_lines_build_the_routine_interface() {
        let src = "\
.routine f, tapes=2, alpha=(3, 2), exits=1, noreturn
.param  num, ('_', '0', '1'), writes=('0', '1')
.param  flag, ('_', 'x')
.func f
stop
";
        let lowered = lower_with(iface_caps(), src).unwrap();
        let iface = lowered.interface.expect("interface present");
        assert_eq!(iface.routines.len(), 1);
        let r = &iface.routines[0];
        assert_eq!(r.params, vec!["num", "flag"]);
        assert_eq!(r.glyphs[0], vec!["_", "0", "1"]);
        assert_eq!(r.writes[0], vec!["0", "1"]);
        assert!(r.writes[1].is_empty());
        assert_eq!(r.exits, 1);
        assert!(!r.returns);
    }

    #[test]
    fn param_count_must_equal_tapes() {
        let src = ".routine f, tapes=2, alpha=(3, 2)\n.param num, ('_', '0', '1')\n.func f\nstop\n";
        let e = lower_with(iface_caps(), src).unwrap_err();
        assert!(matches!(e.kind, AsmErrorKind::BadSignature(ref m) if m.contains("1 .param line(s) for tapes=2")));
    }

    #[test]
    fn param_glyph_count_must_equal_cardinality() {
        let src = ".routine f, tapes=1, alpha=(3)\n.param num, ('_', '0')\n.func f\nstop\n";
        let e = lower_with(iface_caps(), src).unwrap_err();
        assert!(matches!(e.kind, AsmErrorKind::BadSignature(ref m) if m.contains("2 glyphs for a cardinality of 3")));
    }

    #[test]
    fn writes_must_be_a_subset_of_the_alphabet() {
        let src = ".routine f, tapes=1, alpha=(2)\n.param num, ('_', 'a'), writes=('b')\n.func f\nstop\n";
        let e = lower_with(iface_caps(), src).unwrap_err();
        assert!(matches!(e.kind, AsmErrorKind::BadSignature(ref m) if m.contains("writes names `b`")));
    }

    #[test]
    fn interface_is_all_or_none_per_object() {
        let src = "\
.routine f, tapes=1, alpha=(2)
.param t, ('_', 'a')
.func f
stop
.routine g, tapes=1, alpha=(2)
.func g
stop
";
        let e = lower_with(iface_caps(), src).unwrap_err();
        assert!(matches!(e.kind, AsmErrorKind::BadSignature(ref m) if m.contains("function `g` lacks `.param` lines")));
    }

    #[test]
    fn graph_and_grafted_directives_collect_at_object_level() {
        let src = ".graph lib::g, 0xDEADBEEF\n.grafted other::h, 42\n.func f\nstop\n";
        let lowered = lower_with(iface_caps(), src).unwrap();
        assert_eq!(lowered.interface.as_ref().unwrap().graphs, vec![ExportedGraph { name: "lib::g".into(), digest: 0xDEAD_BEEF }]);
        assert_eq!(lowered.grafts, vec![GraftProvenance { graph: "other::h".into(), digest: 42 }]);
    }

    #[test]
    fn digest_directives_must_precede_the_first_func() {
        let src = ".func f\nstop\n.graph g, 1\n";
        let e = lower_with(iface_caps(), src).unwrap_err();
        assert!(matches!(e.kind, AsmErrorKind::Syntax("`.graph`/`.grafted` precede the first `.func`")));
    }

    #[test]
    fn interface_directives_are_unknown_without_the_cap() {
        let src = ".routine f, tapes=1, alpha=(2)\n.param t, ('_', 'a')\n.func f\nstop\n";
        let caps = AsmCaps { tables: true, ..AsmCaps::default() };
        let e = lower_with(caps, src).unwrap_err();
        assert!(matches!(e.kind, AsmErrorKind::UnknownMnemonic(ref w) if w == ".param"));
    }
```

(`lower_with(caps, src)` = assemble-to-`LoweredSource` through `test_syntax()` with `caps` overridden; if no such helper exists, add it next to the existing lowering test helpers.) Note that `.graph`/`.grafted` create an `Interface` with only `graphs` set when no `.param` lines exist; the reader then requires `signatures` — the assembler must emit `signatures` (all-or-none) whenever it emits an interface, which the last test above exercises implicitly via a `.routine`-less function: add to that test the expectation that lowering **fails** with `BadSignature("function `f` lacks a `.routine` signature")`, since an interface without signatures is not representable. Adjust the fixture to sign `f`.

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p mtc-core --lib asm::lower && cargo test -p mtc-core --lib asm::cst`
Expected: compile errors on the new nodes/fields, then "unknown mnemonic `.param`".

- [ ] **Step 3: Implement the CST**

In `cst.rs`: add the three word constants next to `VOLATILE_WORD`; in `recognized_directives` add

```rust
    if caps.interface {
        words.extend([PARAM_WORD, GRAPH_WORD, GRAFTED_WORD]);
    }
```

before the final sort. Extend `RoutineDirectiveCst` with `pub exits: Option<(u32, Span)>` and `pub noreturn: Option<Span>`, and extend `routine_directive` after it has consumed the `alpha=(…)` group: the `rest` slice after `rparen` may be empty, or `, exits=<int>`, or `, noreturn`, or both in that order; any other tail → `None` (the line degrades to Raw and lowering reports it). Add `ParamDirectiveCst` and `DigestDirectiveCst` as in **Interfaces** and shaping functions:

```rust
/// `.param <name>, (<glyphs>)[, writes=(<glyphs>)]`. Glyph lists are
/// captured verbatim between the parens and decoded by
/// `formats::glyphs::parse_glyph_list`, so `'a'..'z'` ranges and the
/// two escapes come along for free.
fn param_directive(src: &ItemText<'_>, body: &[AsmToken], span: Span, trailing: &Option<TrailingComment>) -> Option<ParamDirectiveCst> {
    let [name_tok, c1, lparen, rest @ ..] = &body[1..] else { return None; };
    let name = word_text(name_tok)?;
    if !matches!(c1.kind, AsmTokenKind::Comma) || !matches!(lparen.kind, AsmTokenKind::LParen) { return None; }
    let close = rest.iter().position(|t| matches!(t.kind, AsmTokenKind::RParen))?;
    let glyphs_text = src.slice(lparen.line, lparen.col + 1, rest[close].col);
    let glyphs = crate::formats::glyphs::parse_glyph_list(glyphs_text).ok()?;
    let glyphs_span = Span::new(lparen.line, lparen.col, rest[close].line, rest[close].col + 1);
    let tail = &rest[close + 1..];
    let (writes, writes_span) = match tail {
        [] => (None, None),
        [c2, w, eq, lp, wrest @ ..] if matches!(c2.kind, AsmTokenKind::Comma)
            && word_text(w) == Some("writes") && matches!(eq.kind, AsmTokenKind::Eq)
            && matches!(lp.kind, AsmTokenKind::LParen) =>
        {
            let [winner @ .., rp] = wrest else { return None; };
            if !matches!(rp.kind, AsmTokenKind::RParen) { return None; }
            let text = src.slice(lp.line, lp.col + 1, rp.col);
            let list = if winner.is_empty() { Vec::new() } else { crate::formats::glyphs::parse_glyph_list(text).ok()? };
            (Some(list), Some(Span::new(lp.line, lp.col, rp.line, rp.col + 1)))
        }
        _ => return None,
    };
    Some(ParamDirectiveCst { name: name.to_string(), name_span: name_tok.span(), glyphs, glyphs_span, writes, writes_span, span, trailing: trailing.clone() })
}

/// `.graph <name>, <digest>` / `.grafted <name>, <digest>`; the digest is
/// a canonical u32, decimal or `0x` hex.
fn digest_directive(grafted: bool, body: &[AsmToken], span: Span, trailing: &Option<TrailingComment>) -> Option<DigestDirectiveCst> {
    let [name_tok, c1, digest_tok] = &body[1..] else { return None; };
    let name = word_text(name_tok)?;
    if !matches!(c1.kind, AsmTokenKind::Comma) { return None; }
    let (digest, digest_span) = canonical_u32_or_hex(digest_tok)?;
    Some(DigestDirectiveCst { grafted, name: name.to_string(), name_span: name_tok.span(), digest, digest_span, span, trailing: trailing.clone() })
}
```

`canonical_u32_or_hex` is `canonical_u32` extended to accept `0x[0-9a-fA-F]{1,8}` (write it next to `canonical_u32`). Wire the three words into the shaping dispatch exactly where `ROUTINE_WORD` is recognized, gated by `caps.interface`.

- [ ] **Step 4: Implement the lowering**

In `lower.rs`: `LowerCtx` gains `pending_params: Vec<(String, Vec<ParamDirectiveCst>)>` (params accumulated per pending routine name, in order), `pending_iface: Vec<(String, u8, bool)>` (exits, returns per pending routine), `func_ifaces: Vec<Option<RoutineInterface>>` parallel to `func_sigs`, `graphs: Vec<ExportedGraph>`, `grafts: Vec<GraftProvenance>`. `lower_routine_directive` records `exits`/`noreturn` (default `0`/`returns = true`). A new `lower_param_directive` appends to the most recent pending routine's list (error `BadSignature("`.param` precedes no `.routine`")` when there is none), rejects a glyph count ≠ that tape's cardinality with `BadSignature(format!("`.param {name}` lists {n} glyphs for a cardinality of {c}"))` and a `writes` glyph outside the list with `BadSignature(format!("`.param {name}` writes names `{g}`, which is not in its alphabet"))`. `take_pending_sig` becomes `take_pending(ctx, name) -> Option<(RoutineSig, Option<RoutineInterface>)>`: when params exist, their count must equal `tapes` (`BadSignature(format!("{n} .param line(s) for tapes={t}"))`) and the interface is `RoutineInterface { params, glyphs, writes: writes.unwrap_or_default() per tape, exits, returns }`. `lower_digest_directive` pushes to `graphs`/`grafts`, erroring `Syntax("`.graph`/`.grafted` precede the first `.func`")` once `ctx.functions` is non-empty. At end of input, mirror the signatures' all-or-none rule for interfaces: if any function has one, every function must (`BadSignature(format!("function `{}` lacks `.param` lines", name))`), and an object with graphs/grafts but no signed function is `BadSignature("`.graph`/`.grafted` need `.routine` signatures on every function")`. `LoweredSource` gains `interface: Option<Interface>` (`Some` when any routine interface or any graph exists, with `alphabets: Vec::new()`) and `grafts`.

- [ ] **Step 5: Run the tests**

Run: `cargo test -p mtc-core --lib asm`
Expected: PASS, and `recognized_directives_match_the_real_recognizer` PASS at 16 words.

- [ ] **Step 6: Commit**

```bash
git add crates/core/src/asm/cst.rs crates/core/src/asm/lower.rs
git commit -m "feat(core): .param, exits=/noreturn on .routine, .graph and .grafted — interface directives in the assembler CST and lowering"
```

---

### Task 6: Symbolic binding operands and the `exits=(…)` operand

**Files:**
- Modify: `crates/core/src/asm/cst.rs:1498-1560` (`parse_binding`, `parse_binding_entry`, `parse_pairs`), the operand-token capture that hands `[..]` to lower
- Modify: `crates/core/src/asm/lower.rs:85-116` (`SourceOperand::BoundCallOp`, `SourceTapeBinding`), `:1404-1440` (`classify_operand`), `:1608-1700` (`classify_bound_call`)
- Test: `crates/core/src/asm/cst.rs`, `crates/core/src/asm/lower.rs` `mod tests`

**Interfaces:**
- Produces: `SourceTapeBinding { caller_tape: u8, param: Option<String>, map_written: bool, pairs: Vec<(u32, SourceDst, bool)> }` with `pub enum SourceDst { Index(u32), Label(String) }`; `SourceOperand::BoundCallOp { target, binding, exits: Vec<SpannedName> }`. `parse_binding` returns `Vec<BindingEntryCst { param: Option<String>, phys: u32, map_written: bool, pairs: Vec<FramePairCst> }>` and `FramePairCst.to` becomes `PairDst { Index(u32), Label(String) }` **only for the binding path** — the `.map` directive keeps its numeric grammar; give the binding path its own `BindingPairCst { from: u32, to: PairDst, one_way: bool }` rather than widening `FramePairCst`.

- [ ] **Step 1: Write the failing tests**

In `cst.rs` tests:

```rust
    #[test]
    fn named_binding_entries_with_glyph_labels() {
        let entries = parse_binding_with(iface_caps(), "num: 1{3->'0', 4=>'1'}, flag: 0{}", 1).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].param.as_deref(), Some("num"));
        assert_eq!(entries[0].phys, 1);
        assert!(!entries[0].map_written);
        assert_eq!(entries[0].pairs[0].to, PairDst::Label("0".into()));
        assert!(entries[0].pairs[1].one_way);
        assert!(entries[1].map_written);
        assert!(entries[1].pairs.is_empty());
    }

    #[test]
    fn positional_entries_still_parse_and_report_no_written_map() {
        let entries = parse_binding_with(iface_caps(), "2{1->3, 2=>0}, 0", 1).unwrap();
        assert_eq!(entries[0].param, None);
        assert!(!entries[0].map_written);
        assert_eq!(entries[0].pairs[0].to, PairDst::Index(3));
        assert_eq!(entries[1].phys, 0);
    }

    #[test]
    fn mixing_named_and_positional_entries_is_rejected_by_lowering() { /* lives in lower.rs, below */ }

    #[test]
    fn glyph_labels_need_the_interface_cap() {
        let caps = AsmCaps { tables: true, ..AsmCaps::default() };
        assert!(parse_binding_with(caps, "1{3->'0'}", 1).is_none());
    }
```

In `lower.rs` tests:

```rust
    #[test]
    fn bound_call_with_names_labels_and_exits_lowers() {
        let src = "\
.routine f, tapes=2, alpha=(5, 3)
.param data, ('_', 'a', 'b', '0', '1')
.param ctl, ('_', '0', '1')
.func f
        call    g [num: 1{3->'0', 4->'1'}] exits=(won, lost)
won:    stop
lost:   halt
";
        let lowered = lower_with(iface_caps(), src).unwrap();
        let op = first_operand_of(&lowered, "f", 0); // helper: the Nth instruction's SourceOperand
        let SourceOperand::BoundCallOp { target, binding, exits } = op else { panic!("bound call") };
        assert_eq!(target.name, "g");
        assert_eq!(binding[0].param.as_deref(), Some("num"));
        assert_eq!(binding[0].pairs[0], (3, SourceDst::Label("0".into()), false));
        assert_eq!(exits.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(), vec!["won", "lost"]);
    }

    #[test]
    fn mixed_named_and_positional_entries_are_rejected() {
        let src = ".func f\n        call g [num: 1, 0]\n        stop\n";
        let e = lower_with(iface_caps(), src).unwrap_err();
        assert!(matches!(e.kind, AsmErrorKind::BadFrame(ref m) if m == "a binding names every entry or none"));
    }

    #[test]
    fn exits_operand_only_on_call() {
        let src = ".func f\n        jmp L exits=(L)\nL:      stop\n";
        let e = lower_with(iface_caps(), src).unwrap_err();
        assert!(matches!(e.kind, AsmErrorKind::BadOperand("only a call takes an exit vector")));
    }

    #[ ] // sic: keep this a normal #[test]
    #[test]
    fn exits_labels_must_exist_in_the_function() {
        let src = ".func f\n        call g [0] exits=(nowhere)\n        stop\n";
        let e = lower_with(iface_caps(), src).unwrap_err();
        assert!(matches!(e.kind, AsmErrorKind::UndefinedLabel(ref l) if l == "nowhere"));
    }
```

(Remove the stray `#[ ]` line when copying; it is a reminder, not syntax.) Undefined-label resolution happens in the assembler (Task 7); this test moves there if lowering does not resolve labels — check where `Slot::Call` labels are resolved today and place the test at that layer.

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p mtc-core --lib asm::cst && cargo test -p mtc-core --lib asm::lower`
Expected: compile errors on `PairDst`, `BindingEntryCst`, `SourceDst`, `exits`.

- [ ] **Step 3: Implement the CST side**

In `cst.rs`: `parse_binding(inner, line_no, caps)` now takes the caller's caps (so `Glyph` tokens appear when `interface` is on) — keep `vectors: false` in the re-lex as today. Split entries at depth-0 commas as before. `parse_binding_entry(seg)`:

```rust
fn parse_binding_entry(seg: &[AsmToken]) -> Option<BindingEntryCst> {
    let (param, seg) = match seg {
        [w, colon, rest @ ..] if matches!(colon.kind, AsmTokenKind::Colon) => (Some(word_text(w)?.to_string()), rest),
        _ => (None, seg),
    };
    let (first, rest) = seg.split_first()?;
    let phys = canonical_u32(first)?.0;
    if rest.is_empty() {
        return Some(BindingEntryCst { param, phys, map_written: false, pairs: Vec::new() });
    }
    let [lbrace, mid @ .., rbrace] = rest else { return None; };
    if !matches!(lbrace.kind, AsmTokenKind::LBrace) || !matches!(rbrace.kind, AsmTokenKind::RBrace) { return None; }
    Some(BindingEntryCst { param, phys, map_written: true, pairs: parse_binding_pairs(mid)? })
}
```

`parse_binding_pairs` is `parse_pairs` with the destination accepting `Number` (→ `PairDst::Index`) or `Glyph` (→ `PairDst::Label`). Note the lexer's word rule: `num:` must lex as `Word("num")` + `Colon` — confirm with the lexer test that a trailing single colon is never part of a word (the `AsmTokenKind::Word` doc says so).

The `exits=(…)` operand: `classify_operand` today sees `call <target> [binding]` as target + bracket tokens. A third operand token `exits=(l1, l2)` is captured as one `OperandToken` whose text starts with `exits=(`; shape it in the same place the bracket is captured (the operand region walk that yields `OperandToken`s): under `caps.interface`, a `Word("exits")`, `Eq`, `LParen` … `RParen` run after the bracket becomes one token.

- [ ] **Step 4: Implement the lowering side**

`SourceTapeBinding` and `SourceOperand::BoundCallOp` as in **Interfaces**. In `classify_bound_call`, after `parse_binding`: reject a mix (`BadFrame("a binding names every entry or none")`), reject a glyph label or a param when `!caps.interface` (cannot happen — the lexer would not have produced them — but keep the check as defense: `BadFrame("named entries need the interface capability")`), map `PairDst::Index(n)` → `SourceDst::Index(n)`, `PairDst::Label(s)` → `SourceDst::Label(s)`, carry `map_written`. Parse the exits token: strip `exits=(`/`)`, split on commas, each a label name (`is_symbol_name` after trim), producing `Vec<SpannedName>`; a non-call mnemonic with an exits token → `BadOperand("only a call takes an exit vector")`; an exits token on a call with **no** bracket is allowed (`call g exits=(a)` — a transparent call with exits is meaningless today but the record supports it; lower it as a `BoundCallOp` with an empty binding, which the assembler writes as a bound call with `binding: []` — the format allows zero bindings? `BoundCall.binding.len()` is a `u8` count, zero is representable; the linker's arity check runs in phase 2). Simpler and safer: **require a bracket when exits are present** — `BadOperand("an exit vector needs a binding; write `[…]` (empty is allowed) before it")`, and accept `[]` as an empty binding when `caps.interface` (today `[]` is rejected; keep that rejection when the cap is off).

- [ ] **Step 5: Run the tests**

Run: `cargo test -p mtc-core --lib asm`
Expected: PASS except the label-resolution test, which belongs to Task 7.

- [ ] **Step 6: Commit**

```bash
git add crates/core/src/asm/cst.rs crates/core/src/asm/lower.rs
git commit -m "feat(core): named binding entries, glyph labels, written-empty maps and the exits=(…) operand on call"
```

---

### Task 7: Assembler emission and disassembler rendering

**Files:**
- Modify: `crates/core/src/asm/assembler.rs:120-180` (`Slot::BoundCall`), `:260-345` (object assembly), `:585-605` (slot creation), `:890-925` (hole emission), `:943-952` (`source_binding_to_object`)
- Modify: `crates/core/src/asm/disassembler.rs:272-300` (`render_binding`), `:656-690` (`routine_line`), `:840-860` (bound-call rendering), `:915-935` (per-function preamble), the object-header printing path
- Modify: `crates/core/src/asm/fmt.rs` (the canonical grid treats `.param`, `.graph`, `.grafted` lines as `Structural` pieces like `.routine`)
- Test: `crates/core/src/asm/disassembler.rs` `mod tests`; `crates/core/tests/link_tables.rs` (or a new `crates/core/tests/asm_interface.rs`)

**Interfaces:**
- Consumes: Task 5's `LoweredSource.interface`/`grafts`, Task 6's `SourceOperand::BoundCallOp { exits, .. }` and `SourceTapeBinding`.
- Produces: `ObjectFile.interface`, `ObjectFile.grafts`, `BoundCall { exits, binding: [TapeBinding { param, map_written, pairs: [MapPair { dst_label, .. }] }] }` on assembly; `disassemble_object` prints, in order, `.graph`/`.grafted` lines before the first function, `.routine name, tapes=N, alpha=(…)[, exits=K][, noreturn]` then one `.param` line per tape (when the object has an interface), then `.func`; a bound call prints `call g [num: 1{3->'0', 4=>'1'}, ctl: 0{}] exits=(won, lost)` — named form iff any entry has a `param`, `{}` iff `map_written` with no pairs, a label as `'g'` iff `dst_label`.

- [ ] **Step 1: Write the failing round-trip test**

Create `crates/core/tests/asm_interface.rs`:

```rust
//! dis → asm byte identity over the interface surface (docs/formats.md
//! (routine interfaces), (bound calls)), on the crate-private fake dialect.

use mtc_core::asm::{assemble, disassemble_object};
use mtc_core::asm::syntax::fixture::test_syntax;
use mtc_core::asm::AsmCaps;

fn syntax() -> mtc_core::asm::ArchSyntax {
    let mut s = test_syntax();
    s.caps = AsmCaps { tables: true, rept: true, vectors: true, volatile: false, interface: true };
    s
}

const SOURCE: &str = "\
.graph  lib::findA, 0xDEADBEEF
.grafted other::h, 42
.routine f, tapes=2, alpha=(5, 3), exits=2, noreturn
.param  data, ('_', 'a', 'b', '0', '1'), writes=('0', '1')
.param  ctl, ('_', '0', '1')
.func f
        call    g [num: 1{3->'0', 4=>'1'}, ctl: 0{}] exits=(won, lost)
won:    stop
lost:   stop
.routine g, tapes=1, alpha=(3)
.param  num, ('_', '0', '1')
.func g
        stop
";

#[test]
fn interface_surface_round_trips_byte_identically() {
    let obj = assemble(&syntax(), 0x7F, SOURCE, false).expect("assembles");
    assert!(obj.interface.is_some());
    assert_eq!(obj.grafts.len(), 1);
    let bc = &obj.bound_calls[0];
    assert_eq!(bc.exits.len(), 2);
    assert_eq!(bc.binding[0].param.as_deref(), Some("num"));
    assert_eq!(bc.binding[0].pairs[0].dst_label.as_deref(), Some("0"));
    assert!(bc.binding[1].map_written);
    let text = disassemble_object(&syntax(), &obj).expect("disassembles");
    let again = assemble(&syntax(), 0x7F, &text, false).expect("re-assembles");
    assert_eq!(again.to_bytes(), obj.to_bytes(), "dis → asm is byte-identical:\n{text}");
}

#[test]
fn v3_objects_disassemble_exactly_as_before() {
    // A source using none of the interface surface must produce a v3 object
    // and the same text the pre-v4 disassembler printed (pinned by the
    // existing link_tables.rs fixtures, re-run here for the fake dialect).
    let src = ".routine f, tapes=1, alpha=(2)\n.func f\n        call    g [0{1->1}]\n        stop\n.routine g, tapes=1, alpha=(2)\n.func g\n        stop\n";
    let obj = assemble(&syntax(), 0x7F, src, false).unwrap();
    assert_eq!(u16::from_le_bytes([obj.to_bytes()[3], obj.to_bytes()[4]]), 3);
    let text = disassemble_object(&syntax(), &obj).unwrap();
    assert!(text.contains("call    g [0{1->1}]"), "{text}");
    assert!(!text.contains(".param"), "{text}");
}
```

(Adapt `assemble`/`disassemble_object` signatures to the real ones in `crates/core/src/asm/mod.rs`; the fake dialect's mnemonic for a call and a stop are whatever `test_syntax()` defines — read `crates/core/src/asm/syntax.rs` `fixture` and substitute.)

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p mtc-core --test asm_interface`
Expected: FAIL — the assembler drops the interface, or the disassembler prints nothing for it.

- [ ] **Step 3: Implement the assembler side**

`Slot::BoundCall` gains `exits: Vec<SpannedName>`. Where the slot is emitted (`:897-915`), resolve each exit label through the same function-local label map `Slot::Call`'s jump targets use (undefined → `AsmErrorKind::UndefinedLabel`), and push `BoundCall { …, exits: resolved_offsets }`. `source_binding_to_object` maps `SourceDst::Index(n)` → `MapPair { dst: n, dst_label: None, .. }` and `SourceDst::Label(s)` → `MapPair { dst: 0, dst_label: Some(s), .. }`, and copies `param`/`map_written`. In the object assembly (`:260-345`): `object.interface = lowered.interface` (routines parallel to functions — the assembler's function order is the blob order; where a function ships two build columns the interface entry is duplicated per blob exactly as `signatures` are), `object.grafts = lowered.grafts`. The existing rule "signatures present ⇒ v3" extends: interface present ⇒ signatures present (Task 5 guarantees it).

- [ ] **Step 4: Implement the disassembler side**

`render_binding(binding)`: named form when any `param` is `Some` (`{param}: {phys}`), pairs render `dst_label` as `'{label}'` with the two escapes re-applied (`'` → `\'`, `\` → `\\`), and `{}` when `map_written && pairs.is_empty()`. At the bound-call site append ` exits=({labels})` when `bc.exits` is non-empty, naming each offset through the function's label map (the same map `.exits` rendering uses for raw frames; synthesize `L<offset>` only if no label exists, as the frame path does). `routine_line` gains `exits: u8, returns: bool` and appends `, exits={k}` when `k > 0` and `, noreturn` when `!returns`. After the `.routine` line print one `.param` line per tape from `obj.interface.routines[blob]`: `.param  {name}, ({glyphs})` plus `, writes=({glyphs})` when non-empty, glyphs rendered with `formats::glyphs`' quoting (reuse its renderer if one exists; otherwise `'{g}'` with the two escapes). Before the first function print `.graph {name}, 0x{digest:08X}` for each `interface.graphs` entry and `.grafted {name}, 0x{digest:08X}` for each graft. Exported alphabets print as a comment block `; alphabet {name}: ({glyphs})` after the digest lines (they have no directive; see Task 5's note). Update `fmt.rs` so the new directive lines are `Structural` pieces (they must not be re-gridded as instructions).

- [ ] **Step 5: Run the round trip and the whole core suite**

Run: `cargo test -p mtc-core`
Expected: PASS, including the byte-identity test and every pre-existing disassembler test.

- [ ] **Step 6: Commit**

```bash
git add crates/core/src/asm/assembler.rs crates/core/src/asm/disassembler.rs crates/core/src/asm/fmt.rs crates/core/tests/asm_interface.rs
git commit -m "feat(core): assemble and disassemble the interface surface with dis → asm byte identity"
```

---

### Task 8: TM-1 dialect end to end, editor grammar, PM-1 identity

**Files:**
- Modify: `crates/turing-machine/tests/tma_dialect.rs` (a new fixture using every new form through `tm1_syntax()`)
- Modify: `editors/grammars/tma.tmLanguage.json:42-48` (`routineDirective` rule gains the three words; or a sibling `interfaceDirective` rule — the drift guard counts `*Directive` repository rules and requires ≥ 7)
- Test: `crates/turing-machine/tests/editor_grammar.rs` (existing set-compare), `crates/post-machine/tests/editor_grammar.rs` (must still pass at 13 words for PM-1's caps)

- [ ] **Step 1: Write the failing tests**

Append to `crates/turing-machine/tests/tma_dialect.rs`:

```rust
/// Every interface form the TM-1 dialect spells, round-tripped at the
/// object level (docs/tmt/asm.md (interface directives)).
const INTERFACE_OBJECT: &str = "\
.graph  lib::findAGraph, 0x0000002A
.grafted std::binaryNumbers::plusOneGraph, 0x00000007
.routine main, tapes=2, alpha=(3, 5), exits=1, noreturn
.param  ctl, ('_', '0', '1')
.param  data, ('_', 'a', 'b', '0', '1'), writes=('0', '1')
.func main
        rd
        call    mylib::plusOne [num: 1{3->'0', 4->'1'}] exits=(done)
done:   stp
";

#[test]
fn interface_object_round_trips_byte_identically() {
    let obj = assemble(INTERFACE_OBJECT, false).expect("assembles");
    let text = disassemble_object(&obj).expect("disassembles");
    let again = assemble(&text, false).expect("re-assembles");
    assert_eq!(again.to_bytes(), obj.to_bytes(), "{text}");
    assert_eq!(text, INTERFACE_OBJECT, "the fixture is already canonical");
}
```

(Second assertion: the fixture is written on the canonical grid so `dis` reproduces it verbatim; adjust spacing to the real grid on the first run and keep the assertion.) Then run the grammar guard: `cargo test -p mtc-turing-machine --test editor_grammar` — it FAILS with "the tma grammar's directive rules and core's recognized-directive inventory must agree exactly" (three words missing from the grammar).

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p mtc-turing-machine --test tma_dialect interface_object && cargo test -p mtc-turing-machine --test editor_grammar`
Expected: the round trip may already pass (core did the work); the grammar guard FAILS.

- [ ] **Step 3: Extend the grammar**

In `editors/grammars/tma.tmLanguage.json`, add a repository rule and include it after `routineDirective`:

```json
    "interfaceDirective": {
      "match": "(\\.param|\\.graph|\\.grafted)\\b\\s*([A-Za-z_][A-Za-z0-9_.:]*)?",
      "captures": {
        "1": { "name": "keyword.control.tma" },
        "2": { "name": "entity.name.function.tma" }
      }
    },
```

and `{ "include": "#interfaceDirective" }` in the patterns list. Add a glyph-literal rule if the grammar has none for `'x'` (the `.tmc` grammar has one to copy — `editors/grammars/tmc.tmLanguage.json`). The drift guard's probe for the three words: extend its `match directive.as_str()` with

```rust
            ".param" => ".routine probe, tapes=1, alpha=(2)\n.param t, ('_', 'a')\n.func probe\nstp\n".to_string(),
            ".graph" | ".grafted" => format!("{directive} g, 1\n.routine probe, tapes=1, alpha=(2)\n.param t, ('_', 'a')\n.func probe\nstp\n"),
```

- [ ] **Step 4: Run every gate this phase touches**

Run:

```
cargo test -p mtc-turing-machine --test tma_dialect
cargo test -p mtc-turing-machine --test editor_grammar
cargo test -p mtc-post-machine --test editor_grammar
cargo test -p mtc-post-machine --test asm_volatile
cargo test -p mtc-post-machine --test golden_programs
cargo test -p mtc-turing-machine --test golden_programs
cargo test -p mtc-turing-machine --test opt_equivalence
cargo build -p mtc-core --no-default-features
cargo build --workspace --lib --target wasm32-unknown-unknown
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --check
```

Expected: all PASS. PM-1's grammar guard still sees 13 words (its caps never enable `interface`); the PM-1 goldens and `asm_volatile` byte-compare unchanged.

- [ ] **Step 5: Commit**

```bash
git add crates/turing-machine/tests/tma_dialect.rs crates/turing-machine/tests/editor_grammar.rs editors/grammars/tma.tmLanguage.json
git commit -m "test(turing-machine): the interface surface round-trips through the TM-1 dialect; grammar paints the three directives"
```

---

### Task 9: Documentation

**Files:**
- Modify: `docs/formats.md:181-330` (`.pmo`/`.tmo` layout — add the v4 sections and the widened bound-call record; bump "readers accept 1..=3" to `1..=4` and the writer rule), `:465-490` (routine signature — `exits=`, `noreturn`, the `.param` directive under a new subsection "Routine interfaces"), `:661-700` (bound calls — named entries, glyph labels, `{}`, `exits=(…)`), plus a short "Digest directives" subsection after "Frame descriptors"
- Modify: `docs/tmt/asm.md` (the dialect enables the interface capability; one example of each new form; the note that exported alphabets have no directive and print as comments)
- Modify: `docs/core.md` (the assembler framework's capability list gains `interface`)
- Test: `cargo test -p mtc-core --test error_code_docs` (unchanged codes — the new assembler errors reuse `BadSignature`/`BadFrame`/`BadOperand`/`Syntax`/`UndefinedLabel`, so no registry change; verify by running the guard)

- [ ] **Step 1: Write the layout block**

In `docs/formats.md` after the "version 3 appends five trailing sections" block, add:

```
── version 4 widens the bound-call record and appends three sections ──
bound calls (v4 shape): per tape binding: u8 caller tape, u32 parameter
                name (string index, or 0xFFFFFFFF for a positional entry),
                u8 binding flags (bit 0 = the map was written, even if empty),
                u16 pair count, then per pair: u32 src, u32 dst,
                u8 flags (bit 0 = one-way, bit 1 = dst is a string index — a
                glyph label the linker resolves against the callee's interface);
                then per bound call: u8 exit count, count × u32 exit offset
                (blob-relative, the caller-side labels `retx #k` returns to)
interface (present iff flags bit 5 is set; requires bit 1), once per blob:
                per tape: u32 parameter name, u8 glyph count, count × u32 glyph,
                u8 writes count, count × u32 glyph; then u8 exit count,
                u8 returns (0 = noreturn)
                then: u32 alphabet count, per alphabet: u32 name, u8 glyph count,
                count × u32 glyph; u32 graph count, per graph: u32 name, u32 digest
graft provenance: u32 count, then per record: u32 graph name, u32 digest
```

and the prose: what each record means (one paragraph each, in the style of the existing "Routine signatures / Table blobs / …" bullets), the all-or-none rule for interfaces, the "interface requires signatures" rule, "readers accept 1..=4; a reader rejects a pre-version-4 object that sets flags bit 5 or pair flag bit 1", and the writer rule "an object with no v4 content keeps its v2 or v3 bytes".

- [ ] **Step 2: Write the directive and operand grammar**

Under "Sections and the routine signature", extend the `.routine` grammar to `.routine <name>, tapes=<N>, alpha=(<c1>, …, <cN>)[, exits=<K>][, noreturn]` and add the subsection:

```markdown
### Routine interfaces

`.param <name>, (<glyphs>)[, writes=(<glyphs>)]` follows a `.routine` and
names one of its tapes, in tape order: the parameter name, the tape's
alphabet as a glyph list (the notation `tape-block` uses — `'_', '0', '1'`,
ranges and the two escapes included), and optionally the glyphs the routine
may write on it. A signed function either has one `.param` per tape or none;
an object either describes every function's interface or none. The glyph
count equals the cardinality `alpha` declares for that tape.

`exits=<K>` on `.routine` is the number of exits the routine leaves through
(`retx #0..K-1`); `noreturn` states that it never executes `ret`. Both
default to `0` and "returns".

`.graph <name>, <digest>` and `.grafted <name>, <digest>` stand before the
first `.func`: the former records the digest of an exported graph's
canonical header text, the latter that this unit spliced a library graph
whose body had that digest; the linker compares the two. Exported alphabets
have no directive — they are a compiler fact — and a disassembly prints them
as `; alphabet <name>: (<glyphs>)` comments.
```

Under "Bound calls — the binding call operand", add the named form:

```markdown
An entry may name the callee's parameter instead of relying on list
position — `num: 1{3->'0', 4=>'1'}` — and a pair's destination may be a
quoted glyph label instead of an index; a binding names every entry or
none. `{}` records that the map was written empty (identity on purpose),
distinct from omitting the braces. `exits=(<label>, …)` after the bracket
lists the caller-side labels the callee's `retx #k` returns to. All of these
need the dialect's interface capability; the linker resolves names and
labels against the callee's interface section (`docs/core.md (linking)`).
```

- [ ] **Step 3: Update `docs/tmt/asm.md` and `docs/core.md`**

`docs/tmt/asm.md`: state that `.tma` enables the interface capability, show `INTERFACE_OBJECT` from Task 8 as the example, and note what `tmt dis` prints. `docs/core.md`, the assembler framework's capability list: add `interface` with one sentence.

- [ ] **Step 4: Run the docs guards**

Run: `cargo test -p mtc-core --test error_code_docs && cargo test -p mtc-turing-machine --test cli_docs && cargo test -p mtc-post-machine --test cli_docs`
Expected: PASS (no code table changed).

- [ ] **Step 5: Commit**

```bash
git add docs/formats.md docs/tmt/asm.md docs/core.md
git commit -m "docs(formats): MO v4 — routine interfaces, symbolic bound sites, exit vectors, digest directives"
```

---

### Task 10: Final gate

- [ ] **Step 1: Run the workspace gate the way CI does**

Run: `cargo fmt --check && cargo clippy --workspace --all-targets -- -D warnings && cargo build -p mtc-core --no-default-features && cargo build --workspace --lib --target wasm32-unknown-unknown && cargo nextest run --workspace`
Expected: all green. `nextest` is the gate, not plain `cargo test` (one process per test).

- [ ] **Step 2: Confirm PM-1 byte identity one more time by hand**

Run: `git stash list >/dev/null; for f in crates/post-machine/tests/golden/*.pmc; do target/release/pmt compile "$f" -o /tmp/x.pmo && cmp /tmp/x.pmo "${f%.pmc}.pmo" 2>/dev/null; done` — or, simpler, rely on the golden tests above; this step exists so an executor states the PM-1 result explicitly in the hand-off.

- [ ] **Step 3: Hand off**

Report: the ten commits, the `nextest` summary line, and the two byte-identity statements (v3 objects unchanged; PM-1 unchanged). Phase 2 (linker) starts from `docs/superpowers/plans/2026-09-13-binding-arc-phase-2-linker.md` once written.

---

## Self-review against the spec

- **Spec coverage (phase 1 scope):** interface section ✔ (Tasks 2–3, 5, 7), exported alphabets ✔ (record + reader/writer; authored by phase 3, printed as comments), graph digests and graft provenance ✔ (Tasks 2–3, 5, 7), `.param` ✔, symbolic operand ✔ (Task 6), exits operand ✔ (Task 6–7), written-empty map flag ✔, `returns`/`exits` in the interface ✔ (spelled `exits=`/`noreturn` on `.routine` — a phase-1 addition the spec implied but did not spell; recorded in Task 9's docs), MO v4 ✔, disassembler round trip ✔ (Tasks 7–8), format proptests ✔ (Task 3), "no program's behaviour changes" ✔ (no linker/VM change; PM-1 gates re-run in Task 8).
- **Placeholder scan:** none of the forbidden phrases; every code step shows code; helper names that may not exist (`lower_with`, `first_operand_of`, `parse_binding_with`, `sample_v3`, `canonical_u32_or_hex`) are named as "add next to …" with their contract.
- **Type consistency:** `RoutineInterface`, `Interface`, `ExportedAlphabet`, `ExportedGraph`, `GraftProvenance`, `MapPair.dst_label`, `TapeBinding.param/map_written`, `BoundCall.exits`, `SourceDst`, `BindingEntryCst`, `PairDst`, `ParamDirectiveCst`, `DigestDirectiveCst`, `AsmCaps.interface`, `OBJECT_FORMAT_VERSION_V4`, `FLAG_HAS_INTERFACE`, `NO_STRING` are used with the same names and shapes in every task.

---

## Amendments from #121 (ratified 2026-09-13, before execution began)

The spec section "Folded in from #121" adds three fields to the interface record and one flag to the bound-site record. They land in the same tasks; an executor applies these deltas while doing the task named, not afterwards.

**Task 2 — types.** `RoutineInterface` gains, after `writes`:

```rust
    pub enters: Vec<Option<Vec<String>>>,   // len == arity; None = no clause; Some is never empty
    pub leaves: Vec<Option<Vec<String>>>,   // same shape
    pub opaque: Vec<bool>,                  // len == arity; every reading state has `*` on this tape
```

`TapeBinding` gains `pub open: bool` (the `*` entry of an open map). `is_v4_shape()` additionally returns true when any binding has `open`.

**Task 3 — wire.** In the interface section, per tape, after the `writes` list: `u8 flags (bit 0 = enters present, bit 1 = leaves present, bit 2 = opaque)`, then when bit 0: `u8 count, count × u32 glyph`; when bit 1: the same. In the bound-call record's binding flags byte: bit 1 = open (bit 0 stays "map written"). Reader: an `enters`/`leaves` glyph outside the tape's glyph list is `Malformed("contract glyph outside its alphabet")`, and a present list with zero glyphs is `Malformed("empty head clause")` (the language rejects `enters {}`/`leaves {}` as `empty-head-clause`, so the object never carries one); reserved bits (≥ 3 in the tape flags, ≥ 2 in the binding flags) are rejected. Extend `v4_full_round_trip` and `mo_v4_round_trip` with one tape carrying `enters: Some(vec!["$"])`, `leaves: Some(vec!["$"])`, `opaque: true`, and a binding with `open: true`; add a rejection test for a hand-built byte stream with a present zero-length `enters` list.

**Task 5 — directives.** `.param <name>, (<glyphs>)[, writes=(…)][, enters=(…)][, leaves=(…)][, opaque]` — the four suffixes in that fixed order, each at most once; an absent suffix is `None`; `enters=()`/`leaves=()` are rejected with `BadSignature("`enters=`/`leaves=` list at least one glyph; omit the suffix for no clause")`. Lowering validates each list against the tape's glyphs with `BadSignature(format!("`.param {name}` {clause} names `{g}`, which is not in its alphabet"))`. Add to the Step 1 tests:

```rust
    #[test]
    fn param_contract_suffixes_and_opaque() {
        let src = "\
.routine f, tapes=1, alpha=(3)
.param  num, ('_', '0', '1'), writes=('0', '1'), enters=('1'), leaves=('0', '1'), opaque
.func f
stop
";
        let r = &lower_with(iface_caps(), src).unwrap().interface.unwrap().routines[0];
        assert_eq!(r.enters[0].as_deref(), Some(&["1".to_string()][..]));
        assert_eq!(r.leaves[0].as_deref(), Some(&["0".to_string(), "1".to_string()][..]));
        assert!(r.opaque[0]);
    }
```

**Task 6 — operand.** `parse_binding_entry` accepts a trailing `*` inside the braces — `1{3->'0', *}` or `1{*}` — as the open marker (an `AsmTokenKind::Star` token; the re-lex in `parse_binding` must enable `vectors: true` for `*` to lex, so keep the arrows lexing correctly under both caps — verify with a test that `->` still lexes as `Arrow` at brace depth 1 with `vectors` on, or lex the interior with `rept: true` only and treat `Star` from the rept cap; pick whichever the lexer already supports and pin it). `BindingEntryCst` and `SourceTapeBinding` gain `open: bool`. `*` may appear once, last; elsewhere → `BadFrame("`*` closes a map: write it last, once")`. An open marker needs a written map, so `open` implies `map_written`.

**Task 7 — assembler/disassembler.** `source_binding_to_object` copies `open`; `render_binding` prints `*` last inside the braces when `open` (and `{*}` for an open map with no pairs); `.param` printing appends `, enters=(…)`, `, leaves=(…)`, `, opaque` in that order when present.

**Task 8 — fixture.** Extend `INTERFACE_OBJECT` with `enters=('$'), leaves=('$')` on `data`, `opaque` on `ctl`, and a second call `call mylib::skip [ctl: 0{*}]` — and keep the canonical-text assertion.

**Task 9 — docs.** The `.param` grammar and the operand section gain the three suffixes and the `*` marker; the interface wire block gains the per-tape flags byte and the two optional lists; the bound-site binding flags byte gains bit 1.

Nothing else in phase 1 changes: the runtime checks, the `opaque` inference, the `set` declaration and the linker's open-binding lowering are phases 2–3.
