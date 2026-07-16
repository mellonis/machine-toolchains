# TM-1 Phase 3a: Formats — MX v2 (sectioned) + MT v2 (per-tape glyphs)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add version-2 codecs for the MX executable container (a sectioned image carrying a table section + TM-1 header fields) and the MT tape-block container (per-tape glyph tables), while PM-1 keeps emitting byte-identical v1 for both.

**Architecture:** Spec `docs/superpowers/specs/2026-07-16-tm1-and-tmt-design.md` §6.1 (MX v2), §6.3 (MT v2), §17 phase 3. All changes are in `crates/core/src/formats`. Per the maintainer's design decisions (2026-07-16): each container keeps its 3-byte magic and dispatches on the existing u16 version field (`sniff()` unchanged); MX and MT get their own per-container version constants (MO already has one); the type stays a SINGLE struct per container with v2 data added as new/optional fields, and `to_bytes()` selects v1 vs v2 by whether that data is present, so an unchanged code-only/shared-alphabet value emits byte-identical v1. MO v3 (call-site binding records) is deliberately OUT of this plan — it lands in a later phase-3b plan before the linker (phase 5) needs it.

**Tech Stack:** Rust, cargo workspace; no new dependencies (`serde`/`serde_json` runtime, `proptest` dev-dep already present). Tests via `cargo test`.

## Global Constraints

- **PM-1 byte-identity is the headline invariant.** The committed golden `.pmt` files (`crates/post-machine/tests/golden/`) are asserted byte-for-byte by `golden_programs`, and the CLI pipeline emits `.pmx`; adding v2 capability must NOT change one byte of what PM-1 emits today. `cargo test --workspace` — especially `golden_programs`, `cli_programs`, `opt_equivalence` — is the gate at the end of every task.
- `cargo clippy --workspace --all-targets -- -D warnings` and `cargo fmt --check` clean before every commit.
- **Every reader verifies CRC-32 before decoding anything** (existing contract) — v2 parse paths included.
- **Containers dispatch on magic via `sniff()`, never on file extension** — unchanged.
- `crates/core/src/formats` carries zero architecture knowledge — it stores `arch`/`profile`/cardinalities verbatim; only the VM judges them. No PM-1/TM-1 literals in format source.
- Property tests use `proptest` following the existing `formats` test style: round-trip (`from_bytes(x.to_bytes()) == x`) AND never-panic-on-arbitrary-bytes.
- No new dependencies.
- Commit style: conventional with scope (`feat(core):`, `test(core):`). NEVER add any Claude/AI attribution footer.
- Commits require the maintainer's explicit go-ahead in the executing session (repo rule); if not yet granted, stop at the commit step and ask.
- Byte layouts defined in this plan are the normative contract the emitters (phase 4 for MX, the tape CLI for MT) will target; document them in code comments and (task 7) in `docs/formats.md`.

---

### Task 1: MX — per-container version constant + v2 fields (no v2 emit yet)

Decouple MX versioning from the shared `FORMAT_VERSION` and add the v2 fields to `Executable`, defaulted so the current code-only shape is unchanged. No v2 serialization yet — this task only proves the refactor is byte-identical.

**Files:**
- Modify: `crates/core/src/formats/executable.rs` (imports, `Executable` struct, `to_bytes`, `from_bytes`, tests)
- Test: `crates/core/src/formats/executable.rs` (inline `mod tests`)

**Interfaces:**
- Produces (task 2 builds on these): `pub const MX_FORMAT_VERSION_V1: u16 = 1; pub const MX_FORMAT_VERSION_V2: u16 = 2;` and `Executable` gaining `pub tape_count: u8`, `pub profile: u8`, `pub alphabet_cardinalities: Vec<u32>`, `pub tables: Vec<u8>`. A constructor `Executable::code_only(arch: u8, entry: u32, code: Vec<u8>) -> Self` that sets the v2 fields to their v1 defaults (`tape_count: 1, profile: 0, alphabet_cardinalities: vec![], tables: vec![]`).

- [ ] **Step 1: Write the byte-identity lock test** (append to executable.rs `mod tests`)

```rust
    /// The v1 code-only shape must serialize byte-for-byte as before the
    /// v2 refactor — this pins PM-1's .pmx output.
    #[test]
    fn code_only_is_byte_identical_v1() {
        let exe = Executable::code_only(ARCH_PM1, 0, vec![0x0D, 0x05, 0x02]);
        let bytes = exe.to_bytes();
        // magic + version(1) + arch + flags + crc(4) + entry(4) + size(4) + code(3)
        assert_eq!(&bytes[0..3], b"MX\x01");
        assert_eq!(u16::from_le_bytes(bytes[3..5].try_into().unwrap()), 1);
        assert_eq!(bytes[5], ARCH_PM1);
        assert_eq!(bytes[6], 0);
        assert_eq!(u32::from_le_bytes(bytes[15..19].try_into().unwrap()), 3);
        assert_eq!(bytes.len(), 19 + 3);
        assert_eq!(Executable::from_bytes(&bytes).unwrap(), exe);
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p mtc-core code_only_is_byte_identical_v1`
Expected: compile error — `no function code_only`, missing fields.

- [ ] **Step 3: Implement the refactor**

In executable.rs, replace the `use super::{FORMAT_VERSION, FormatError};` line with `use super::FormatError;` and add the constants + fields:

```rust
pub const MX_FORMAT_VERSION_V1: u16 = 1;
pub const MX_FORMAT_VERSION_V2: u16 = 2;
```

Extend the struct:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Executable {
    pub arch: u8,
    pub entry: u32,
    pub code: Vec<u8>,
    /// v2 header fields; the v1 code-only shape leaves them at defaults
    /// (`tape_count: 1`, `profile: 0`, empty cardinalities, empty tables)
    /// and serializes as version 1 (docs/formats.md).
    pub tape_count: u8,
    pub profile: u8,
    pub alphabet_cardinalities: Vec<u32>,
    pub tables: Vec<u8>,
}
```

Add the constructor and a v1-shape predicate:

```rust
impl Executable {
    /// A version-1 code-only image (the shape PM-1 emits).
    pub fn code_only(arch: u8, entry: u32, code: Vec<u8>) -> Self {
        Self {
            arch,
            entry,
            code,
            tape_count: 1,
            profile: 0,
            alphabet_cardinalities: Vec::new(),
            tables: Vec::new(),
        }
    }

    /// True when the image carries no v2-only data and must serialize as v1.
    fn is_v1_shape(&self) -> bool {
        self.tape_count <= 1
            && self.profile == 0
            && self.alphabet_cardinalities.is_empty()
            && self.tables.is_empty()
    }
```

Make `to_bytes` keep exactly the v1 body for the v1 shape (this task never emits v2 yet — a non-v1 shape is unreachable until task 2, so guard it):

```rust
    pub fn to_bytes(&self) -> Vec<u8> {
        assert!(self.is_v1_shape(), "MX v2 emit lands in a later task");
        self.to_bytes_v1()
    }

    fn to_bytes_v1(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(19 + self.code.len());
        out.extend_from_slice(&MAGIC_EXECUTABLE);
        put_u16(&mut out, MX_FORMAT_VERSION_V1);
        out.push(self.arch);
        out.push(0); // flags
        put_u32(&mut out, 0); // crc placeholder
        put_u32(&mut out, self.entry);
        put_u32(&mut out, u32::try_from(self.code.len()).expect("code fits u32"));
        out.extend_from_slice(&self.code);
        stamp_crc(&mut out, CRC_OFFSET);
        out
    }
```

In `from_bytes`, replace the `if version != FORMAT_VERSION` check with a v1 branch that builds via the defaults, and reject non-1 versions for now (task 2 adds v2):

```rust
        let version = r.u16()?;
        if version != MX_FORMAT_VERSION_V1 {
            return Err(FormatError::UnsupportedVersion(version));
        }
        let arch = r.u8()?;
        let _flags = r.u8()?;
        let _crc = r.u32()?;
        let entry = r.u32()?;
        let code_size = r.u32()? as usize;
        let code = r.bytes(code_size)?.to_vec();
        r.finish()?;
        if entry as usize >= code.len() {
            return Err(FormatError::Malformed("entry offset outside code"));
        }
        Ok(Self::code_only(arch, entry, code))
```

Update the existing tests' `sample()` to `Executable::code_only(ARCH_PM1, 0, vec![0x0D, 0x05, 0x02])` and the `entry_outside_code_is_rejected` test to mutate `exe.entry` on a `code_only` value (it already builds via `sample()`, so only `sample()` changes).

- [ ] **Step 4: Run tests**

Run: `cargo test -p mtc-core --lib formats::executable` then `cargo test --workspace`
Expected: all PASS — the byte-identity lock passes and every existing MX test still holds.

- [ ] **Step 5: Commit**

```bash
git add crates/core/src/formats/executable.rs
git commit -m "feat(core): MX per-container version constants + v2 fields (v1 emit byte-identical)"
```

---

### Task 2: MX v2 — sectioned emit + parse

Add the version-2 body: TM-1 header fields (tape_count, profile, per-tape cardinalities) and a table section alongside the code section. `to_bytes` now dispatches; `from_bytes` accepts both versions.

**Files:**
- Modify: `crates/core/src/formats/executable.rs`
- Test: `crates/core/src/formats/executable.rs` (inline `mod tests`)

**MX v2 byte layout** (normative — phase 4 emits this):

```
offset 0:  magic [b'M', b'X', 0x01]        (3)
           version u16 LE = 2               (2)
           arch u8                          (1)
           flags u8 = 0                     (1)
           crc u32 LE                       (4)   — over everything after this field
           tape_count u8 (1..=16)           (1)
           profile u8 (0=base, 1=frames)    (1)
           entry u32 LE                     (4)
           code_size u32 LE                 (4)
           table_size u32 LE                (4)
           alphabet_cardinalities: tape_count × u32 LE
           code:   code_size bytes
           tables: table_size bytes
```

**Interfaces:**
- Produces: `Executable::sectioned(arch, entry, code, tables, tape_count, profile, alphabet_cardinalities) -> Self` (a v2 constructor); `to_bytes()` emits v2 for a non-v1 shape; `from_bytes()` parses both. Phase 4's `Machine::from_executable` and the TM-1 build path consume the v2 fields.

- [ ] **Step 1: Write the failing tests**

```rust
    fn sample_v2() -> Executable {
        Executable::sectioned(
            0x02,               // arch (TM-1)
            0,                  // entry
            vec![0x10, 0x02],   // code (rd; stp — placeholder)
            vec![1, 1, 0, 5],   // tables (a tiny match-table blob)
            2,                  // tape_count
            1,                  // profile = frames
            vec![3, 128],       // per-tape alphabet cardinalities
        )
    }

    #[test]
    fn v2_round_trips() {
        let exe = sample_v2();
        let bytes = exe.to_bytes();
        assert_eq!(&bytes[0..3], b"MX\x01");
        assert_eq!(u16::from_le_bytes(bytes[3..5].try_into().unwrap()), 2);
        assert_eq!(Executable::from_bytes(&bytes).unwrap(), exe);
    }

    #[test]
    fn v1_still_parses_after_v2_lands() {
        let v1 = Executable::code_only(ARCH_PM1, 0, vec![0x0D, 0x05, 0x02]);
        assert_eq!(Executable::from_bytes(&v1.to_bytes()).unwrap(), v1);
    }

    #[test]
    fn v2_corruption_rejected_before_decode() {
        let mut bytes = sample_v2().to_bytes();
        let last = bytes.len() - 1;
        bytes[last] ^= 0xFF;
        assert!(matches!(
            Executable::from_bytes(&bytes),
            Err(FormatError::BadCrc { .. })
        ));
    }

    #[test]
    fn v2_tape_count_zero_or_over_16_rejected() {
        for bad in [0u8, 17] {
            let mut exe = sample_v2();
            exe.tape_count = bad;
            exe.alphabet_cardinalities = vec![3; bad.max(1) as usize];
            let bytes = exe.to_bytes();
            assert!(matches!(
                Executable::from_bytes(&bytes),
                Err(FormatError::Malformed("tape count out of range"))
            ));
        }
    }

    #[test]
    fn v2_cardinality_count_must_match_tape_count() {
        // Hand-corrupt: tape_count says 2 but only 1 cardinality present.
        let exe = sample_v2();
        let mut bytes = exe.to_bytes();
        // tape_count is at offset 11 (after magic3+ver2+arch1+flags1+crc4)
        bytes[11] = 3; // claim 3 tapes; only 2 cardinalities follow → truncation/mismatch
        crate::formats::crc32::stamp_crc(&mut bytes, CRC_OFFSET);
        assert!(Executable::from_bytes(&bytes).is_err());
    }
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p mtc-core --lib formats::executable`
Expected: compile error (`no function sectioned`), then failures.

- [ ] **Step 3: Implement**

Add the constructor:

```rust
    /// A version-2 sectioned image (the shape TM-1 emits).
    #[allow(clippy::too_many_arguments)]
    pub fn sectioned(
        arch: u8,
        entry: u32,
        code: Vec<u8>,
        tables: Vec<u8>,
        tape_count: u8,
        profile: u8,
        alphabet_cardinalities: Vec<u32>,
    ) -> Self {
        Self { arch, entry, code, tape_count, profile, alphabet_cardinalities, tables }
    }
```

Change `to_bytes` to dispatch and add the v2 emitter:

```rust
    pub fn to_bytes(&self) -> Vec<u8> {
        if self.is_v1_shape() {
            self.to_bytes_v1()
        } else {
            self.to_bytes_v2()
        }
    }

    fn to_bytes_v2(&self) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&MAGIC_EXECUTABLE);
        put_u16(&mut out, MX_FORMAT_VERSION_V2);
        out.push(self.arch);
        out.push(0); // flags
        put_u32(&mut out, 0); // crc placeholder
        out.push(self.tape_count);
        out.push(self.profile);
        put_u32(&mut out, self.entry);
        put_u32(&mut out, u32::try_from(self.code.len()).expect("code fits u32"));
        put_u32(&mut out, u32::try_from(self.tables.len()).expect("tables fit u32"));
        for &card in &self.alphabet_cardinalities {
            put_u32(&mut out, card);
        }
        out.extend_from_slice(&self.code);
        out.extend_from_slice(&self.tables);
        stamp_crc(&mut out, CRC_OFFSET);
        out
    }
```

Rework `from_bytes` to dispatch on version after the CRC check:

```rust
        let version = r.u16()?;
        match version {
            MX_FORMAT_VERSION_V1 => {
                let arch = r.u8()?;
                let _flags = r.u8()?;
                let _crc = r.u32()?;
                let entry = r.u32()?;
                let code_size = r.u32()? as usize;
                let code = r.bytes(code_size)?.to_vec();
                r.finish()?;
                if entry as usize >= code.len() {
                    return Err(FormatError::Malformed("entry offset outside code"));
                }
                Ok(Self::code_only(arch, entry, code))
            }
            MX_FORMAT_VERSION_V2 => {
                let arch = r.u8()?;
                let _flags = r.u8()?;
                let _crc = r.u32()?;
                let tape_count = r.u8()?;
                if tape_count == 0 || tape_count > 16 {
                    return Err(FormatError::Malformed("tape count out of range"));
                }
                let profile = r.u8()?;
                let entry = r.u32()?;
                let code_size = r.u32()? as usize;
                let table_size = r.u32()? as usize;
                let mut alphabet_cardinalities = Vec::with_capacity(tape_count as usize);
                for _ in 0..tape_count {
                    alphabet_cardinalities.push(r.u32()?);
                }
                let code = r.bytes(code_size)?.to_vec();
                let tables = r.bytes(table_size)?.to_vec();
                r.finish()?;
                if entry as usize >= code.len() {
                    return Err(FormatError::Malformed("entry offset outside code"));
                }
                Ok(Self::sectioned(arch, entry, code, tables, tape_count, profile, alphabet_cardinalities))
            }
            other => Err(FormatError::UnsupportedVersion(other)),
        }
```

Note: the `v2_cardinality_count_must_match_tape_count` test relies on `r.finish()` catching the length mismatch (claiming 3 tapes reads one extra u32 from code/tables, then `finish` fails on trailing/short) — if the specific corruption needs a clearer error, adjust the test's mutation to produce a `Truncated`/`Malformed` and assert `is_err()` (the test already asserts only `is_err()`).

- [ ] **Step 4: Run tests**

Run: `cargo test -p mtc-core --lib formats::executable` then `cargo test --workspace`
Expected: all PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/core/src/formats/executable.rs
git commit -m "feat(core): MX v2 sectioned image — table section + TM-1 header fields"
```

---

### Task 3: MX v2 property tests

**Files:**
- Modify: the crate's property-test location for formats. First run `grep -rln "proptest" crates/core` to find where format codecs are property-tested (an existing `tests/` file or an inline `#[cfg(test)] proptest!` block); add the MX v2 cases there, matching the established style.
- Test: same file.

**Interfaces:**
- Consumes: `Executable::{code_only, sectioned, to_bytes, from_bytes}`.

- [ ] **Step 1: Write the property tests** (place beside the existing format property tests; if none exist for MX, create an inline `#[cfg(test)] mod proptests` in executable.rs using `proptest::prelude::*`)

```rust
proptest! {
    /// Any well-formed v2 image round-trips.
    #[test]
    fn mx_v2_round_trip(
        arch in any::<u8>(),
        tape_count in 1u8..=16,
        profile in 0u8..=1,
        code in prop::collection::vec(any::<u8>(), 1..64),
        tables in prop::collection::vec(any::<u8>(), 0..64),
    ) {
        let entry = 0u32; // always in-bounds for code.len() >= 1
        let cards = vec![3u32; tape_count as usize];
        let exe = Executable::sectioned(arch, entry, code, tables, tape_count, profile, cards);
        prop_assert_eq!(Executable::from_bytes(&exe.to_bytes()).unwrap(), exe);
    }

    /// from_bytes never panics on arbitrary bytes.
    #[test]
    fn mx_from_bytes_never_panics(bytes in prop::collection::vec(any::<u8>(), 0..256)) {
        let _ = Executable::from_bytes(&bytes);
    }
}
```

- [ ] **Step 2: Run to verify they compile and pass**

Run: `cargo test -p mtc-core mx_v2_round_trip mx_from_bytes_never_panics`
Expected: PASS (if a proptest shrink surfaces a real round-trip asymmetry, fix the codec, not the test).

- [ ] **Step 3: Commit**

```bash
git add crates/core/src/formats/executable.rs
git commit -m "test(core): property-test MX v2 round-trip and noise resistance"
```

---

### Task 4: MT — per-container version constant + per-tape alphabet field (v1 emit byte-identical)

Add an optional per-tape alphabet override to `TapeSnapshot`, defaulted to `None` (inherit the block alphabet) so the current shared-alphabet shape serializes byte-identical v1 — this is what preserves the golden `.pmt` files.

**Files:**
- Modify: `crates/core/src/formats/tapeblock.rs`
- Modify: any consumer that constructs `TapeSnapshot` with a struct literal — find with `grep -rn "TapeSnapshot {" crates/`. Each gains `alphabet: None`. (Likely: the tape CLI, golden test derivations, and visuals/tape helpers.)
- Test: `crates/core/src/formats/tapeblock.rs` (inline `mod tests`)

**Interfaces:**
- Produces: `pub const MT_FORMAT_VERSION_V1: u16 = 1; pub const MT_FORMAT_VERSION_V2: u16 = 2;`; `TapeSnapshot` gains `pub alphabet: Option<Vec<String>>` (None = use the block `alphabet`; Some = this tape's own glyph table). A `TapeBlockFile` is v2 iff any tape has `Some`.

- [ ] **Step 1: Write the byte-identity lock test**

```rust
    /// A shared-alphabet block (all tapes `alphabet: None`) serializes
    /// byte-for-byte as v1 — this pins the committed golden .pmt files.
    #[test]
    fn shared_alphabet_is_byte_identical_v1() {
        let block = sample(); // all tapes alphabet: None after this task's refactor
        let bytes = block.to_bytes();
        assert_eq!(&bytes[0..3], b"MT\x01");
        assert_eq!(u16::from_le_bytes(bytes[3..5].try_into().unwrap()), 1);
        assert_eq!(TapeBlockFile::from_bytes(&bytes).unwrap(), block);
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p mtc-core shared_alphabet_is_byte_identical_v1`
Expected: compile error (missing `alphabet` field on `TapeSnapshot`).

- [ ] **Step 3: Implement**

Replace `use super::{FORMAT_VERSION, FormatError};` with `use super::FormatError;` and add:

```rust
pub const MT_FORMAT_VERSION_V1: u16 = 1;
pub const MT_FORMAT_VERSION_V2: u16 = 2;
```

Extend `TapeSnapshot`:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TapeSnapshot {
    pub origin: i64,
    pub cells: Vec<u8>,
    pub head: i64,
    /// v2: this tape's own glyph table. `None` inherits the block-level
    /// `alphabet` (the v1 shape); `Some` triggers v2 emit (docs/formats.md).
    pub alphabet: Option<Vec<String>>,
}
```

Add the v1-shape predicate and split `to_bytes`:

```rust
impl TapeBlockFile {
    fn is_v1_shape(&self) -> bool {
        self.tapes.iter().all(|t| t.alphabet.is_none())
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        if self.is_v1_shape() {
            self.to_bytes_v1()
        } else {
            self.to_bytes_v2()
        }
    }

    fn to_bytes_v1(&self) -> Vec<u8> {
        // EXACTLY the current byte body, with MT_FORMAT_VERSION_V1.
        // (move the existing to_bytes contents here verbatim, swapping
        //  FORMAT_VERSION → MT_FORMAT_VERSION_V1)
        ...
    }
```

Move the existing `to_bytes` body into `to_bytes_v1` verbatim (only the version constant name changes). In `from_bytes`, dispatch on version — for v1, parse exactly as today and set every `TapeSnapshot.alphabet = None`. Add `alphabet: None` to the two `TapeSnapshot { .. }` constructions in the v1 parse and in every test `sample()`/literal. Update all external `TapeSnapshot { .. }` literals found by grep to add `alphabet: None`.

(v2 emit/parse comes in task 5 — for now `to_bytes_v2` can be `unimplemented!("MT v2 emit lands in task 5")` since `is_v1_shape()` is always true until a test sets `Some`; but to keep the build clean, write the empty v2 dispatch arm in `from_bytes` returning `UnsupportedVersion` for version 2 until task 5.)

- [ ] **Step 4: Run tests**

Run: `cargo test -p mtc-core --lib formats::tapeblock` then `cargo test --workspace`
Expected: all PASS — golden `.pmt` byte-identity holds (this is the key assertion; if `golden_programs` fails, the v1 body was not moved verbatim).

- [ ] **Step 5: Commit**

```bash
git add crates/core/src/formats/tapeblock.rs   # plus any consumer files grep surfaced
git commit -m "feat(core): MT per-tape alphabet field, defaulted to shared (v1 byte-identical)"
```

---

### Task 5: MT v2 — per-tape glyph emit + parse

Serialize per-tape alphabets when any tape carries `Some`, and parse them back; v1 shared-alphabet files still load (every tape `None`).

**Files:**
- Modify: `crates/core/src/formats/tapeblock.rs`
- Test: `crates/core/src/formats/tapeblock.rs` (inline `mod tests`)

**MT v2 byte layout** (normative — the tape CLI emits this):

```
offset 0:  magic [b'M', b'T', 0x01]     (3)
           version u16 LE = 2            (2)
           flags u8 = 0                  (1)
           crc u32 LE                    (4)
           block_alphabet_count u8       (1)   — the fallback/shared alphabet
           block_alphabet: count × (u16 len + utf8 bytes)
           tape_count u8                 (1)
           per tape:
             origin i64 LE
             cells_len u32 LE
             cells: cells_len bytes
             head i64 LE
             own_alphabet_count u8       — 0 means "inherit block alphabet"
             own_alphabet: own_count × (u16 len + utf8 bytes)
```

The v1 layout omits every tape's `own_alphabet_count` byte (there is no per-tape alphabet in v1). Cell-index bounds are checked against the tape's EFFECTIVE alphabet (own if `Some`, else block).

**Interfaces:**
- Consumes: task-4 fields/constants. Produces: nothing new — same `TapeBlockFile`/`TapeSnapshot` API, now v2-capable.

- [ ] **Step 1: Write the failing tests**

```rust
    fn sample_v2() -> TapeBlockFile {
        TapeBlockFile {
            alphabet: vec!["_".into()], // block fallback
            tapes: vec![
                TapeSnapshot {
                    origin: 0,
                    cells: vec![0, 1, 2],
                    head: 0,
                    alphabet: Some(vec!["_".into(), "0".into(), "1".into()]),
                },
                TapeSnapshot {
                    origin: 0,
                    cells: vec![0],
                    head: 0,
                    alphabet: None, // inherits block "_"
                },
            ],
        }
    }

    #[test]
    fn v2_round_trips_per_tape_alphabets() {
        let block = sample_v2();
        let bytes = block.to_bytes();
        assert_eq!(u16::from_le_bytes(bytes[3..5].try_into().unwrap()), 2);
        assert_eq!(TapeBlockFile::from_bytes(&bytes).unwrap(), block);
    }

    #[test]
    fn v2_cell_outside_own_alphabet_rejected() {
        let mut block = sample_v2();
        block.tapes[0].cells[0] = 9; // own alphabet has 3 symbols
        let bytes = block.to_bytes();
        assert!(matches!(
            TapeBlockFile::from_bytes(&bytes),
            Err(FormatError::Malformed("cell index outside alphabet"))
        ));
    }

    #[test]
    fn v2_emoji_glyphs_survive() {
        let block = TapeBlockFile {
            alphabet: vec!["_".into()],
            tapes: vec![TapeSnapshot {
                origin: 0,
                cells: vec![0, 1],
                head: 0,
                alphabet: Some(vec!["_".into(), "😀".into()]),
            }],
        };
        assert_eq!(TapeBlockFile::from_bytes(&block.to_bytes()).unwrap(), block);
    }

    #[test]
    fn v1_shared_alphabet_file_still_loads() {
        let v1 = sample(); // all None
        assert_eq!(TapeBlockFile::from_bytes(&v1.to_bytes()).unwrap(), v1);
    }
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p mtc-core --lib formats::tapeblock`
Expected: failures (`to_bytes_v2` unimplemented / version-2 rejected).

- [ ] **Step 3: Implement**

Implement `to_bytes_v2` per the layout above (block alphabet, then each tape with its `own_alphabet_count` byte + optional glyphs). Add a v2 arm to `from_bytes` that reads the block alphabet, then per tape reads the snapshot fields + `own_alphabet_count` (0 → `None`, else read glyphs → `Some`), validates each tape's cells against its effective alphabet (`own.as_ref().unwrap_or(&block_alphabet)`), and rejects an empty effective alphabet. Reuse the existing glyph read/write helpers (u16 len + utf8, with the `glyph not utf-8` malformed error).

- [ ] **Step 4: Run tests**

Run: `cargo test -p mtc-core --lib formats::tapeblock` then `cargo test --workspace`
Expected: all PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/core/src/formats/tapeblock.rs
git commit -m "feat(core): MT v2 per-tape glyph tables (v1 shared-alphabet files still load)"
```

---

### Task 6: MT v2 property tests

**Files:**
- Modify: same property-test location as task 3.
- Test: same file.

- [ ] **Step 1: Write the property tests**

```rust
proptest! {
    /// A block with arbitrary per-tape alphabets round-trips.
    #[test]
    fn mt_v2_round_trip(
        seed in prop::collection::vec(
            (prop::collection::vec("[a-z]{1,3}", 1..5), prop::collection::vec(0u8..4, 0..8)),
            1..4),
    ) {
        // Build tapes whose cells stay within their own alphabet.
        let tapes: Vec<_> = seed.into_iter().map(|(alpha, cells)| {
            let n = alpha.len() as u8;
            TapeSnapshot {
                origin: 0,
                cells: cells.into_iter().map(|c| c % n).collect(),
                head: 0,
                alphabet: Some(alpha),
            }
        }).collect();
        let block = TapeBlockFile { alphabet: vec!["_".into()], tapes };
        prop_assert_eq!(TapeBlockFile::from_bytes(&block.to_bytes()).unwrap(), block);
    }

    #[test]
    fn mt_from_bytes_never_panics(bytes in prop::collection::vec(any::<u8>(), 0..256)) {
        let _ = TapeBlockFile::from_bytes(&bytes);
    }
}
```

- [ ] **Step 2: Run to verify**

Run: `cargo test -p mtc-core mt_v2_round_trip mt_from_bytes_never_panics`
Expected: PASS (fix the codec, not the test, on any shrink failure).

- [ ] **Step 3: Commit**

```bash
git add crates/core/src/formats/tapeblock.rs
git commit -m "test(core): property-test MT v2 round-trip and noise resistance"
```

---

### Task 7: Docs + phase-3a gate

**Files:**
- Modify: `docs/formats.md` (add the MX v2 and MT v2 layout sections; note version-dispatch and v1 back-compat)
- No code changes beyond doc.

- [ ] **Step 1: Document the two v2 layouts**

In `docs/formats.md`, under the `.pmx`/MX and `.pmt`/MT sections, add the v2 byte layouts exactly as specified in tasks 2 and 5, plus a sentence each: version-dispatch is on the u16 header field (magic unchanged, `sniff()` unchanged); v1 readers still load PM-1 images; the layout is forge-agnostic prose (no issue/PR refs — this is a published `docs/` page).

- [ ] **Step 2: Full suite + gates**

Run:
```
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --check
```
Expected: all green/clean — especially `golden_programs` (MT v1 byte-identity) and `cli_programs`.

- [ ] **Step 3: Byte-identity spot check**

Confirm no golden `.pmt` file changed on disk:
```
git status --short crates/post-machine/tests/golden/
```
Expected: empty (goldens untouched — the v1 shape emitted identical bytes).

- [ ] **Step 4: Commit**

```bash
git add docs/formats.md
git commit -m "docs(formats): MX v2 sectioned + MT v2 per-tape glyph layouts"
```

---

## Self-review notes (spec → plan coverage)

- §6.1 MX v2 (sectioned, table section, tape_count/profile/cardinalities header, PM-1 stays v1, sniff+version dispatch) → tasks 1–3. The maintainer decision "single Executable + version tag, v1 byte-identical" is realized by `is_v1_shape()` dispatch + the task-1 lock test.
- §6.3 MT v2 (per-tape glyph tables, v1 shared-alphabet stays readable) → tasks 4–6, via the additive `Option<Vec<String>>` per-tape field (chosen over a breaking restructure to protect the golden `.pmt` byte-identity — an amendment noted to the maintainer).
- §6.2 MO v3 is explicitly DEFERRED to a phase-3b plan (maintainer decision) — not in scope here.
- The "every reader verifies CRC before decoding" and "dispatch on magic not extension" contracts are preserved (both v2 parse paths call `verify_crc` first; `sniff()` is untouched).
- Byte-layout normativity: both v2 layouts are documented in code comments (tasks 2, 5) and `docs/formats.md` (task 7), since phase 4 (MX emit) and the tape CLI (MT emit) target them.
- No architecture knowledge enters `formats` — `profile`/`arch`/cardinalities are stored verbatim.
