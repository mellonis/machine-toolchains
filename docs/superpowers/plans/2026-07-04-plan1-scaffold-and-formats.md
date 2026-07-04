# Plan 1/7: Cargo Workspace Scaffold + Container Formats

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A building, testing cargo workspace with the three binary container formats (`MO`/`MX`/`MT`) fully implemented and round-trip-tested in `crates/core`.

**Architecture:** Workspace with two crates: `mtc-core` (lib, arch-agnostic toolchain core — this plan implements its `formats` module) and `mtc-post-machine` (lib, stub for now). Formats are pure byte codecs: `to_bytes()`/`from_bytes()` with CRC32 integrity, no I/O, no arch knowledge (the arch byte is data, not behavior).

**Tech Stack:** Rust stable, edition 2024, cargo workspace. Dev-dep `proptest` (Task 7 only). No runtime dependencies in this plan.

**Spec:** `docs/superpowers/specs/2026-07-04-post-machine-toolchain-design.md` §6 (file formats), §10 (project shape).

## Global Constraints

- All multi-byte integers **little-endian** (spec §6).
- Magics: object `4D 4F 01` (`"MO"+0x01`), executable `4D 58 01` (`"MX"+0x01`), tape-block `4D 54 01` (`"MT"+0x01`). The third byte is the epoch.
- `u16 format version` = `1` everywhere in v1.
- CRC32 (IEEE, polynomial `0xEDB88320`) covers the **whole file with the 4 crc bytes zeroed**; writers stamp last, readers verify before decoding anything (spec §6.1).
- Arch byte: `0x01` = PM-1. The formats layer stores/returns it without judging it (core is arch-agnostic).
- Quality gates on every commit: `cargo test` green, `cargo clippy --workspace --all-targets -- -D warnings` clean, `cargo fmt --check` clean.
- Package names `mtc-core` / `mtc-post-machine` (a crate cannot be named `core`); directories `crates/core` / `crates/post-machine`.
- Commit policy: per-task commits in this repo are pre-approved by the user. Never push.

---

### Task 1: Workspace scaffold

**Files:**
- Create: `Cargo.toml` (workspace root)
- Create: `rust-toolchain.toml`
- Create: `LICENSE` (GPL-3.0 text — copy verbatim from `../turing-machine-js/LICENSE`)
- Create: `.gitignore`
- Create: `crates/core/Cargo.toml`, `crates/core/src/lib.rs`
- Create: `crates/post-machine/Cargo.toml`, `crates/post-machine/src/lib.rs`
- Create: `README.md`

**Interfaces:**
- Consumes: nothing (first task).
- Produces: workspace where `cargo test`/`clippy`/`fmt` run; crate `mtc-core` with empty `pub mod formats;` placeholder NOT yet declared (added in Task 2).

- [ ] **Step 1: Create the workspace files**

`Cargo.toml`:
```toml
[workspace]
resolver = "3"
members = ["crates/core", "crates/post-machine"]

[workspace.package]
edition = "2024"
license = "GPL-3.0-or-later"
repository = "https://github.com/mellonis/machine-toolchains"
```

`rust-toolchain.toml`:
```toml
[toolchain]
channel = "stable"
```

`.gitignore`:
```gitignore
/target
```

`crates/core/Cargo.toml`:
```toml
[package]
name = "mtc-core"
version = "0.1.0"
edition.workspace = true
license.workspace = true
repository.workspace = true

[dependencies]
```

`crates/core/src/lib.rs`:
```rust
//! Arch-agnostic machine-toolchains core: container formats, VM core,
//! linker, assembler/disassembler frameworks, tape devices.
```

`crates/post-machine/Cargo.toml`:
```toml
[package]
name = "mtc-post-machine"
version = "0.1.0"
edition.workspace = true
license.workspace = true
repository.workspace = true

[dependencies]
mtc-core = { path = "../core" }
```

`crates/post-machine/src/lib.rs`:
```rust
//! Post-machine toolchain: PM-1 arch module, `.pmc` compiler, stdlib, `pmt`.
```

`README.md`:
```markdown
# machine-toolchains

A Rust toolchain family for tape machines: compiler, assembler/disassembler,
linker, and a bus-accurate processor VM. First machine: the Post machine
(`pmt`). Design: `docs/superpowers/specs/2026-07-04-post-machine-toolchain-design.md`.
```

- [ ] **Step 2: Verify the workspace builds and gates pass**

Run: `cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --check`
Expected: builds, "running 0 tests", no clippy warnings, fmt clean.

- [ ] **Step 3: Commit**

```bash
git add -A
git commit -m "chore: cargo workspace scaffold (mtc-core, mtc-post-machine)"
```

---

### Task 2: CRC32 + stamp/verify helpers

**Files:**
- Create: `crates/core/src/formats/mod.rs`
- Create: `crates/core/src/formats/crc32.rs`
- Modify: `crates/core/src/lib.rs`

**Interfaces:**
- Consumes: Task 1 workspace.
- Produces: `mtc_core::formats::crc32::{crc32(data: &[u8]) -> u32, stamp_crc(buf: &mut [u8], at: usize), verify_crc(buf: &[u8], at: usize) -> Result<(), FormatError>}` and `mtc_core::formats::FormatError` (variants used by all later tasks: `BadMagic`, `BadCrc { stored: u32, computed: u32 }`, `UnsupportedVersion(u16)`, `Truncated`, `Malformed(&'static str)`).

- [ ] **Step 1: Write the failing tests**

In `crates/core/src/formats/crc32.rs` (bottom of the file that Step 3 creates — write the test module first; it won't compile yet, which is this cycle's "failing" state):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::formats::FormatError;

    #[test]
    fn crc32_check_vector() {
        // The canonical IEEE CRC-32 check value.
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    }

    #[test]
    fn crc32_empty() {
        assert_eq!(crc32(b""), 0);
    }

    #[test]
    fn stamp_then_verify_round_trips() {
        let mut buf = vec![b'M', b'X', 0x01, 1, 0, 1, 0, 0, 0, 0, 0, 7, 7];
        stamp_crc(&mut buf, 7);
        assert!(verify_crc(&buf, 7).is_ok());
    }

    #[test]
    fn verify_detects_corruption() {
        let mut buf = vec![0u8; 16];
        stamp_crc(&mut buf, 4);
        buf[12] ^= 0xFF; // flip a payload byte
        match verify_crc(&buf, 4) {
            Err(FormatError::BadCrc { .. }) => {}
            other => panic!("expected BadCrc, got {other:?}"),
        }
    }

    #[test]
    fn verify_truncated_buffer() {
        assert!(matches!(verify_crc(&[0u8; 3], 4), Err(FormatError::Truncated)));
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p mtc-core`
Expected: compile error — `formats` module and functions not defined.

- [ ] **Step 3: Implement**

`crates/core/src/formats/mod.rs`:
```rust
//! Binary container formats shared by all machine toolchains (spec §6).
//! Pure byte codecs: no I/O, no architecture knowledge.

pub mod crc32;

/// Format version written into every v1 container.
pub const FORMAT_VERSION: u16 = 1;

#[derive(Debug, PartialEq, Eq)]
pub enum FormatError {
    BadMagic,
    BadCrc { stored: u32, computed: u32 },
    UnsupportedVersion(u16),
    Truncated,
    Malformed(&'static str),
}

impl std::fmt::Display for FormatError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BadMagic => write!(f, "bad magic"),
            Self::BadCrc { stored, computed } => {
                write!(f, "crc mismatch: stored {stored:#010x}, computed {computed:#010x}")
            }
            Self::UnsupportedVersion(v) => write!(f, "unsupported format version {v}"),
            Self::Truncated => write!(f, "truncated file"),
            Self::Malformed(what) => write!(f, "malformed file: {what}"),
        }
    }
}

impl std::error::Error for FormatError {}
```

`crates/core/src/formats/crc32.rs` (above the test module from Step 1):
```rust
//! CRC-32 (IEEE 802.3, reflected, poly 0xEDB88320) — spec §6.1.

use super::FormatError;

pub fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &byte in data {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

/// Zero the 4 crc bytes at `at`, compute the crc of the whole buffer,
/// store it at `at` (little-endian). Writers call this last.
pub fn stamp_crc(buf: &mut [u8], at: usize) {
    buf[at..at + 4].fill(0);
    let crc = crc32(buf);
    buf[at..at + 4].copy_from_slice(&crc.to_le_bytes());
}

/// Verify a buffer stamped by [`stamp_crc`]. Readers call this before
/// decoding anything else.
pub fn verify_crc(buf: &[u8], at: usize) -> Result<(), FormatError> {
    if buf.len() < at + 4 {
        return Err(FormatError::Truncated);
    }
    let stored = u32::from_le_bytes(buf[at..at + 4].try_into().unwrap());
    let mut copy = buf.to_vec();
    copy[at..at + 4].fill(0);
    let computed = crc32(&copy);
    if stored != computed {
        return Err(FormatError::BadCrc { stored, computed });
    }
    Ok(())
}
```

`crates/core/src/lib.rs` — replace the file with:
```rust
//! Arch-agnostic machine-toolchains core: container formats, VM core,
//! linker, assembler/disassembler frameworks, tape devices.

pub mod formats;
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p mtc-core`
Expected: 5 tests pass.

- [ ] **Step 5: Gates + commit**

Run: `cargo clippy --workspace --all-targets -- -D warnings && cargo fmt`
```bash
git add -A
git commit -m "feat(core): crc32 with stamp/verify, FormatError"
```

---

### Task 3: Little-endian Reader/Writer helpers

**Files:**
- Create: `crates/core/src/formats/io.rs`
- Modify: `crates/core/src/formats/mod.rs` (add `pub(crate) mod io;`)

**Interfaces:**
- Consumes: `FormatError` from Task 2.
- Produces (crate-internal, used by Tasks 4–6):
  - `Reader::new(&[u8]) -> Reader`, methods `u8() u16() u32() i64() -> Result<_, FormatError>`, `bytes(n: usize) -> Result<&[u8], FormatError>`, `finish(self) -> Result<(), FormatError>` (errors `Malformed("trailing bytes")` if unconsumed input remains).
  - Writer free functions: `put_u16(&mut Vec<u8>, u16)`, `put_u32(&mut Vec<u8>, u32)`, `put_i64(&mut Vec<u8>, i64)`.

- [ ] **Step 1: Write the failing tests**

In `crates/core/src/formats/io.rs` (test module first):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::formats::FormatError;

    #[test]
    fn reads_back_what_writers_wrote() {
        let mut buf = Vec::new();
        buf.push(0xAB);
        put_u16(&mut buf, 0x1234);
        put_u32(&mut buf, 0xDEAD_BEEF);
        put_i64(&mut buf, -5);
        buf.extend_from_slice(b"xyz");

        let mut r = Reader::new(&buf);
        assert_eq!(r.u8().unwrap(), 0xAB);
        assert_eq!(r.u16().unwrap(), 0x1234);
        assert_eq!(r.u32().unwrap(), 0xDEAD_BEEF);
        assert_eq!(r.i64().unwrap(), -5);
        assert_eq!(r.bytes(3).unwrap(), b"xyz");
        assert!(r.finish().is_ok());
    }

    #[test]
    fn truncation_is_reported() {
        let mut r = Reader::new(&[0x01]);
        assert!(matches!(r.u32(), Err(FormatError::Truncated)));
    }

    #[test]
    fn trailing_bytes_are_reported() {
        let mut r = Reader::new(&[1, 2]);
        r.u8().unwrap();
        assert!(matches!(
            r.finish(),
            Err(FormatError::Malformed("trailing bytes"))
        ));
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p mtc-core io`
Expected: compile error — `Reader`/`put_*` not defined.

- [ ] **Step 3: Implement**

`crates/core/src/formats/io.rs` (above the tests):
```rust
//! Little-endian byte cursor + writer helpers for the container codecs.

use super::FormatError;

pub(crate) struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    pub(crate) fn new(buf: &'a [u8]) -> Self {
        Self { buf, pos: 0 }
    }

    pub(crate) fn bytes(&mut self, n: usize) -> Result<&'a [u8], FormatError> {
        let end = self.pos.checked_add(n).ok_or(FormatError::Truncated)?;
        if end > self.buf.len() {
            return Err(FormatError::Truncated);
        }
        let out = &self.buf[self.pos..end];
        self.pos = end;
        Ok(out)
    }

    pub(crate) fn u8(&mut self) -> Result<u8, FormatError> {
        Ok(self.bytes(1)?[0])
    }

    pub(crate) fn u16(&mut self) -> Result<u16, FormatError> {
        Ok(u16::from_le_bytes(self.bytes(2)?.try_into().unwrap()))
    }

    pub(crate) fn u32(&mut self) -> Result<u32, FormatError> {
        Ok(u32::from_le_bytes(self.bytes(4)?.try_into().unwrap()))
    }

    pub(crate) fn i64(&mut self) -> Result<i64, FormatError> {
        Ok(i64::from_le_bytes(self.bytes(8)?.try_into().unwrap()))
    }

    pub(crate) fn finish(self) -> Result<(), FormatError> {
        if self.pos == self.buf.len() {
            Ok(())
        } else {
            Err(FormatError::Malformed("trailing bytes"))
        }
    }
}

pub(crate) fn put_u16(out: &mut Vec<u8>, v: u16) {
    out.extend_from_slice(&v.to_le_bytes());
}

pub(crate) fn put_u32(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_le_bytes());
}

pub(crate) fn put_i64(out: &mut Vec<u8>, v: i64) {
    out.extend_from_slice(&v.to_le_bytes());
}
```

In `crates/core/src/formats/mod.rs`, after `pub mod crc32;` add:
```rust
pub(crate) mod io;
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p mtc-core`
Expected: all tests pass (5 from Task 2 + 3 new).

- [ ] **Step 5: Gates + commit**

Run: `cargo clippy --workspace --all-targets -- -D warnings && cargo fmt`
```bash
git add -A
git commit -m "feat(core): little-endian Reader/Writer helpers for formats"
```

---

### Task 4: Executable format (`MX`)

**Files:**
- Create: `crates/core/src/formats/executable.rs`
- Modify: `crates/core/src/formats/mod.rs`

**Interfaces:**
- Consumes: `crc32::{stamp_crc, verify_crc}`, `io::{Reader, put_u16, put_u32}`, `FormatError`, `FORMAT_VERSION`.
- Produces (public, consumed by the linker in Plan 4 and the loader in Plan 2):
  ```rust
  pub const MAGIC_EXECUTABLE: [u8; 3]; // b"MX" + 0x01
  pub const ARCH_PM1: u8 = 0x01;      // in formats/mod.rs
  pub struct Executable { pub arch: u8, pub entry: u32, pub code: Vec<u8> }
  impl Executable {
      pub fn to_bytes(&self) -> Vec<u8>;
      pub fn from_bytes(bytes: &[u8]) -> Result<Executable, FormatError>;
  }
  ```
- Byte layout (spec §6.1): `magic[3] | version u16 | arch u8 | flags u8 | crc32 u32 @7 | entry u32 | code_size u32 | code`.

- [ ] **Step 1: Write the failing tests**

In `crates/core/src/formats/executable.rs` (test module first):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::formats::{FormatError, ARCH_PM1};

    fn sample() -> Executable {
        Executable {
            arch: ARCH_PM1,
            entry: 0,
            code: vec![0x0D, 0x05, 0x02], // ent, rgt, stp
        }
    }

    #[test]
    fn round_trip() {
        let bytes = sample().to_bytes();
        let back = Executable::from_bytes(&bytes).unwrap();
        assert_eq!(back.arch, ARCH_PM1);
        assert_eq!(back.entry, 0);
        assert_eq!(back.code, vec![0x0D, 0x05, 0x02]);
    }

    #[test]
    fn layout_is_exact() {
        let bytes = sample().to_bytes();
        assert_eq!(&bytes[0..3], b"MX\x01");
        assert_eq!(u16::from_le_bytes(bytes[3..5].try_into().unwrap()), 1); // version
        assert_eq!(bytes[5], ARCH_PM1);
        assert_eq!(bytes[6], 0); // flags
        // [7..11] crc, [11..15] entry, [15..19] code size
        assert_eq!(u32::from_le_bytes(bytes[15..19].try_into().unwrap()), 3);
        assert_eq!(bytes.len(), 19 + 3);
    }

    #[test]
    fn corruption_is_rejected_before_decode() {
        let mut bytes = sample().to_bytes();
        let last = bytes.len() - 1;
        bytes[last] ^= 0xFF;
        assert!(matches!(
            Executable::from_bytes(&bytes),
            Err(FormatError::BadCrc { .. })
        ));
    }

    #[test]
    fn bad_magic_is_rejected() {
        let mut bytes = sample().to_bytes();
        bytes[1] = b'Z';
        assert!(matches!(
            Executable::from_bytes(&bytes),
            Err(FormatError::BadMagic)
        ));
    }

    #[test]
    fn unsupported_version_is_rejected() {
        let mut bytes = sample().to_bytes();
        bytes[3] = 9; // version 9
        crate::formats::crc32::stamp_crc(&mut bytes, 7);
        assert!(matches!(
            Executable::from_bytes(&bytes),
            Err(FormatError::UnsupportedVersion(9))
        ));
    }

    #[test]
    fn entry_outside_code_is_rejected() {
        let mut exe = sample();
        exe.entry = 99;
        let bytes = exe.to_bytes();
        assert!(matches!(
            Executable::from_bytes(&bytes),
            Err(FormatError::Malformed("entry offset outside code"))
        ));
    }

    #[test]
    fn truncated_and_trailing_are_rejected() {
        let bytes = sample().to_bytes();
        assert!(matches!(
            Executable::from_bytes(&bytes[..bytes.len() - 1]),
            Err(FormatError::BadCrc { .. }) | Err(FormatError::Truncated)
        ));
        let mut extended = bytes.clone();
        extended.push(0);
        assert!(Executable::from_bytes(&extended).is_err());
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p mtc-core executable`
Expected: compile error — `Executable` not defined.

- [ ] **Step 3: Implement**

In `crates/core/src/formats/mod.rs` add (near `FORMAT_VERSION`):
```rust
/// Architecture ids carried in `MO`/`MX` headers. The formats layer
/// stores them verbatim; only the VM's arch registry judges them.
pub const ARCH_PM1: u8 = 0x01;

pub mod executable;
```

`crates/core/src/formats/executable.rs` (above the tests):
```rust
//! `MX` executable container (spec §6.1).

use super::crc32::{stamp_crc, verify_crc};
use super::io::{put_u16, put_u32, Reader};
use super::{FormatError, FORMAT_VERSION};

pub const MAGIC_EXECUTABLE: [u8; 3] = [b'M', b'X', 0x01];
const CRC_OFFSET: usize = 7;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Executable {
    pub arch: u8,
    pub entry: u32,
    pub code: Vec<u8>,
}

impl Executable {
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(19 + self.code.len());
        out.extend_from_slice(&MAGIC_EXECUTABLE);
        put_u16(&mut out, FORMAT_VERSION);
        out.push(self.arch);
        out.push(0); // flags
        put_u32(&mut out, 0); // crc placeholder
        put_u32(&mut out, self.entry);
        put_u32(&mut out, u32::try_from(self.code.len()).expect("code fits u32"));
        out.extend_from_slice(&self.code);
        stamp_crc(&mut out, CRC_OFFSET);
        out
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, FormatError> {
        if bytes.len() < 3 {
            return Err(FormatError::Truncated);
        }
        if bytes[0..3] != MAGIC_EXECUTABLE {
            return Err(FormatError::BadMagic);
        }
        verify_crc(bytes, CRC_OFFSET)?;

        let mut r = Reader::new(&bytes[3..]);
        let version = r.u16()?;
        if version != FORMAT_VERSION {
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
        Ok(Self { arch, entry, code })
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p mtc-core`
Expected: all tests pass.

- [ ] **Step 5: Gates + commit**

Run: `cargo clippy --workspace --all-targets -- -D warnings && cargo fmt`
```bash
git add -A
git commit -m "feat(core): MX executable container codec"
```

---

### Task 5: Object format (`MO`)

**Files:**
- Create: `crates/core/src/formats/object.rs`
- Modify: `crates/core/src/formats/mod.rs` (add `pub mod object;`)

**Interfaces:**
- Consumes: Tasks 2–3 helpers.
- Produces (public; the assembler emits these in Plan 3, the linker consumes them in Plan 4):
  ```rust
  pub const MAGIC_OBJECT: [u8; 3]; // b"MO" + 0x01
  pub struct ObjectFile {
      pub arch: u8,
      pub symbols: Vec<Symbol>,
      pub blobs: Vec<Vec<u8>>,          // per-function code, starts with ent
      pub relocations: Vec<Relocation>, // call sites: 4-byte holes
      pub debug: Option<Vec<BlobDebug>>, // parallel to blobs when present
  }
  pub struct Symbol { pub name: String, pub def: SymbolDef }
  pub enum SymbolDef { Defined { blob: u32 }, External }
  pub struct Relocation { pub blob: u32, pub offset: u32, pub symbol: u32 }
  pub struct BlobDebug { pub labels: Vec<(String, u32)>, pub lines: Vec<(u32, u32)> } // (code offset, source line)
  impl ObjectFile {
      pub fn to_bytes(&self) -> Vec<u8>;
      pub fn from_bytes(bytes: &[u8]) -> Result<ObjectFile, FormatError>;
  }
  ```
- Byte layout: header `magic[3] | version u16 | arch u8 | flags u8 (bit0 = has debug) | crc32 u32 @7`, then sections in order:
  - strings: `u32 count`, each `u16 len + UTF-8 bytes` (deduplicated pool; symbol/label names reference it by `u32` index)
  - symbols: `u32 count`, each `u32 name_idx | u8 kind (0 external, 1 defined) | u32 blob_idx (0xFFFF_FFFF for external)`
  - blobs: `u32 count`, each `u32 len + bytes`
  - relocations: `u32 count`, each `u32 blob | u32 offset | u32 symbol`
  - debug (only if flags bit0): per blob: `u32 label count` (`u32 name_idx | u32 offset` each), `u32 line count` (`u32 code_offset | u32 source_line` each)
- Validation on read (all `Malformed`): string indices in range; defined symbol's `blob` in range; external symbol's blob field must be `0xFFFF_FFFF`; relocation `blob`/`symbol` in range and `offset + 4 <= blob.len()`; debug section blob count equals blobs count.

- [ ] **Step 1: Write the failing tests**

In `crates/core/src/formats/object.rs` (test module first):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::formats::{FormatError, ARCH_PM1};

    fn sample() -> ObjectFile {
        ObjectFile {
            arch: ARCH_PM1,
            symbols: vec![
                Symbol { name: "main".into(), def: SymbolDef::Defined { blob: 0 } },
                Symbol { name: "goToEnd".into(), def: SymbolDef::External },
            ],
            // ent, call <4-byte hole>, stp
            blobs: vec![vec![0x0D, 0x0B, 0, 0, 0, 0, 0x02]],
            relocations: vec![Relocation { blob: 0, offset: 2, symbol: 1 }],
            debug: None,
        }
    }

    #[test]
    fn round_trip_without_debug() {
        let bytes = sample().to_bytes();
        assert_eq!(&bytes[0..3], b"MO\x01");
        let back = ObjectFile::from_bytes(&bytes).unwrap();
        assert_eq!(back, sample());
    }

    #[test]
    fn round_trip_with_debug() {
        let mut obj = sample();
        obj.debug = Some(vec![BlobDebug {
            labels: vec![("L1".into(), 1)],
            lines: vec![(0, 3), (1, 4)],
        }]);
        let bytes = obj.to_bytes();
        let back = ObjectFile::from_bytes(&bytes).unwrap();
        assert_eq!(back, obj);
    }

    #[test]
    fn crc_corruption_rejected() {
        let mut bytes = sample().to_bytes();
        let last = bytes.len() - 1;
        bytes[last] ^= 1;
        assert!(matches!(
            ObjectFile::from_bytes(&bytes),
            Err(FormatError::BadCrc { .. })
        ));
    }

    #[test]
    fn reloc_offset_out_of_blob_rejected() {
        let mut obj = sample();
        obj.relocations[0].offset = 5; // 5 + 4 > blob len 7
        let bytes = obj.to_bytes();
        assert!(matches!(
            ObjectFile::from_bytes(&bytes),
            Err(FormatError::Malformed("relocation outside blob"))
        ));
    }

    #[test]
    fn defined_symbol_with_bad_blob_rejected() {
        let mut obj = sample();
        obj.symbols[0].def = SymbolDef::Defined { blob: 7 };
        let bytes = obj.to_bytes();
        assert!(matches!(
            ObjectFile::from_bytes(&bytes),
            Err(FormatError::Malformed("symbol blob index out of range"))
        ));
    }

    #[test]
    fn unicode_symbol_names_survive() {
        let mut obj = sample();
        obj.symbols[0].name = "иди_в_конец".into();
        let bytes = obj.to_bytes();
        let back = ObjectFile::from_bytes(&bytes).unwrap();
        assert_eq!(back.symbols[0].name, "иди_в_конец");
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p mtc-core object`
Expected: compile error — types not defined.

- [ ] **Step 3: Implement**

`crates/core/src/formats/object.rs` (above the tests):
```rust
//! `MO` object container (spec §6.2).

use super::crc32::{stamp_crc, verify_crc};
use super::io::{put_u16, put_u32, Reader};
use super::{FormatError, FORMAT_VERSION};

pub const MAGIC_OBJECT: [u8; 3] = [b'M', b'O', 0x01];
const CRC_OFFSET: usize = 7;
const EXTERNAL_BLOB: u32 = 0xFFFF_FFFF;
const FLAG_HAS_DEBUG: u8 = 0b0000_0001;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObjectFile {
    pub arch: u8,
    pub symbols: Vec<Symbol>,
    pub blobs: Vec<Vec<u8>>,
    pub relocations: Vec<Relocation>,
    pub debug: Option<Vec<BlobDebug>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Symbol {
    pub name: String,
    pub def: SymbolDef,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SymbolDef {
    Defined { blob: u32 },
    External,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Relocation {
    pub blob: u32,
    pub offset: u32,
    pub symbol: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BlobDebug {
    pub labels: Vec<(String, u32)>,
    pub lines: Vec<(u32, u32)>,
}

/// Build-time string pool: dedups names, hands out u32 indices.
struct StringPool {
    strings: Vec<String>,
}

impl StringPool {
    fn new() -> Self {
        Self { strings: Vec::new() }
    }

    fn intern(&mut self, s: &str) -> u32 {
        if let Some(i) = self.strings.iter().position(|x| x == s) {
            return i as u32;
        }
        self.strings.push(s.to_owned());
        (self.strings.len() - 1) as u32
    }
}

impl ObjectFile {
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut pool = StringPool::new();
        let symbol_names: Vec<u32> =
            self.symbols.iter().map(|s| pool.intern(&s.name)).collect();
        let debug_label_names: Vec<Vec<u32>> = match &self.debug {
            Some(per_blob) => per_blob
                .iter()
                .map(|d| d.labels.iter().map(|(n, _)| pool.intern(n)).collect())
                .collect(),
            None => Vec::new(),
        };

        let mut out = Vec::new();
        out.extend_from_slice(&MAGIC_OBJECT);
        put_u16(&mut out, FORMAT_VERSION);
        out.push(self.arch);
        out.push(if self.debug.is_some() { FLAG_HAS_DEBUG } else { 0 });
        put_u32(&mut out, 0); // crc placeholder

        put_u32(&mut out, pool.strings.len() as u32);
        for s in &pool.strings {
            put_u16(&mut out, s.len() as u16);
            out.extend_from_slice(s.as_bytes());
        }

        put_u32(&mut out, self.symbols.len() as u32);
        for (sym, &name_idx) in self.symbols.iter().zip(&symbol_names) {
            put_u32(&mut out, name_idx);
            match sym.def {
                SymbolDef::Defined { blob } => {
                    out.push(1);
                    put_u32(&mut out, blob);
                }
                SymbolDef::External => {
                    out.push(0);
                    put_u32(&mut out, EXTERNAL_BLOB);
                }
            }
        }

        put_u32(&mut out, self.blobs.len() as u32);
        for blob in &self.blobs {
            put_u32(&mut out, blob.len() as u32);
            out.extend_from_slice(blob);
        }

        put_u32(&mut out, self.relocations.len() as u32);
        for reloc in &self.relocations {
            put_u32(&mut out, reloc.blob);
            put_u32(&mut out, reloc.offset);
            put_u32(&mut out, reloc.symbol);
        }

        if let Some(per_blob) = &self.debug {
            for (d, names) in per_blob.iter().zip(&debug_label_names) {
                put_u32(&mut out, d.labels.len() as u32);
                for ((_, offset), &name_idx) in d.labels.iter().zip(names) {
                    put_u32(&mut out, name_idx);
                    put_u32(&mut out, *offset);
                }
                put_u32(&mut out, d.lines.len() as u32);
                for (code_offset, line) in &d.lines {
                    put_u32(&mut out, *code_offset);
                    put_u32(&mut out, *line);
                }
            }
        }

        stamp_crc(&mut out, CRC_OFFSET);
        out
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, FormatError> {
        if bytes.len() < 3 {
            return Err(FormatError::Truncated);
        }
        if bytes[0..3] != MAGIC_OBJECT {
            return Err(FormatError::BadMagic);
        }
        verify_crc(bytes, CRC_OFFSET)?;

        let mut r = Reader::new(&bytes[3..]);
        let version = r.u16()?;
        if version != FORMAT_VERSION {
            return Err(FormatError::UnsupportedVersion(version));
        }
        let arch = r.u8()?;
        let flags = r.u8()?;
        let _crc = r.u32()?;

        let string_count = r.u32()? as usize;
        let mut strings = Vec::with_capacity(string_count);
        for _ in 0..string_count {
            let len = r.u16()? as usize;
            let raw = r.bytes(len)?;
            let s = std::str::from_utf8(raw)
                .map_err(|_| FormatError::Malformed("string not utf-8"))?;
            strings.push(s.to_owned());
        }
        let name_of = |idx: u32| -> Result<String, FormatError> {
            strings
                .get(idx as usize)
                .cloned()
                .ok_or(FormatError::Malformed("string index out of range"))
        };

        let symbol_count = r.u32()? as usize;
        let mut raw_symbols = Vec::with_capacity(symbol_count);
        for _ in 0..symbol_count {
            let name_idx = r.u32()?;
            let kind = r.u8()?;
            let blob = r.u32()?;
            raw_symbols.push((name_idx, kind, blob));
        }

        let blob_count = r.u32()? as usize;
        let mut blobs = Vec::with_capacity(blob_count);
        for _ in 0..blob_count {
            let len = r.u32()? as usize;
            blobs.push(r.bytes(len)?.to_vec());
        }

        let reloc_count = r.u32()? as usize;
        let mut relocations = Vec::with_capacity(reloc_count);
        for _ in 0..reloc_count {
            relocations.push(Relocation {
                blob: r.u32()?,
                offset: r.u32()?,
                symbol: r.u32()?,
            });
        }

        let debug = if flags & FLAG_HAS_DEBUG != 0 {
            let mut per_blob = Vec::with_capacity(blob_count);
            for _ in 0..blob_count {
                let label_count = r.u32()? as usize;
                let mut labels = Vec::with_capacity(label_count);
                for _ in 0..label_count {
                    let name = name_of(r.u32()?)?;
                    let offset = r.u32()?;
                    labels.push((name, offset));
                }
                let line_count = r.u32()? as usize;
                let mut lines = Vec::with_capacity(line_count);
                for _ in 0..line_count {
                    lines.push((r.u32()?, r.u32()?));
                }
                per_blob.push(BlobDebug { labels, lines });
            }
            Some(per_blob)
        } else {
            None
        };

        r.finish()?;

        let mut symbols = Vec::with_capacity(symbol_count);
        for (name_idx, kind, blob) in raw_symbols {
            let name = name_of(name_idx)?;
            let def = match kind {
                0 => {
                    if blob != EXTERNAL_BLOB {
                        return Err(FormatError::Malformed("external symbol carries a blob"));
                    }
                    SymbolDef::External
                }
                1 => {
                    if blob as usize >= blobs.len() {
                        return Err(FormatError::Malformed("symbol blob index out of range"));
                    }
                    SymbolDef::Defined { blob }
                }
                _ => return Err(FormatError::Malformed("unknown symbol kind")),
            };
            symbols.push(Symbol { name, def });
        }

        for reloc in &relocations {
            let blob = blobs
                .get(reloc.blob as usize)
                .ok_or(FormatError::Malformed("relocation blob index out of range"))?;
            if reloc.symbol as usize >= symbols.len() {
                return Err(FormatError::Malformed("relocation symbol index out of range"));
            }
            if reloc.offset as usize + 4 > blob.len() {
                return Err(FormatError::Malformed("relocation outside blob"));
            }
        }

        Ok(Self { arch, symbols, blobs, relocations, debug })
    }
}
```

In `crates/core/src/formats/mod.rs` add:
```rust
pub mod object;
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p mtc-core`
Expected: all tests pass.

- [ ] **Step 5: Gates + commit**

Run: `cargo clippy --workspace --all-targets -- -D warnings && cargo fmt`
```bash
git add -A
git commit -m "feat(core): MO object container codec with string pool and validation"
```

---

### Task 6: Tape-block format (`MT`)

**Files:**
- Create: `crates/core/src/formats/tapeblock.rs`
- Modify: `crates/core/src/formats/mod.rs` (add `pub mod tapeblock;`)

**Interfaces:**
- Consumes: Tasks 2–3 helpers.
- Produces (public; VM input/output in Plan 2, `pmt tape` in Plan 7):
  ```rust
  pub const MAGIC_TAPEBLOCK: [u8; 3]; // b"MT" + 0x01
  pub struct TapeBlockFile { pub alphabet: Vec<String>, pub tapes: Vec<TapeSnapshot> }
  pub struct TapeSnapshot { pub origin: i64, pub cells: Vec<u8>, pub head: i64 }
  impl TapeBlockFile {
      pub fn to_bytes(&self) -> Vec<u8>;
      pub fn from_bytes(bytes: &[u8]) -> Result<TapeBlockFile, FormatError>;
  }
  ```
- Byte layout (spec §6.3): `magic[3] | version u16 | flags u8 | crc32 u32 @6 | alphabet (u8 count, each u16 len + UTF-8 glyph) | u8 tape count | per tape: i64 origin, u32 length, u8 cells[length], i64 head`.
- Validation: alphabet count ≥ 1; tape count ≥ 1; every cell index < alphabet count (`Malformed("cell index outside alphabet")`).

- [ ] **Step 1: Write the failing tests**

In `crates/core/src/formats/tapeblock.rs` (test module first):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::formats::FormatError;

    fn sample() -> TapeBlockFile {
        TapeBlockFile {
            alphabet: vec![" ".into(), "*".into()],
            tapes: vec![TapeSnapshot { origin: -2, cells: vec![0, 1, 1, 0, 1], head: 1 }],
        }
    }

    #[test]
    fn round_trip() {
        let bytes = sample().to_bytes();
        assert_eq!(&bytes[0..3], b"MT\x01");
        assert_eq!(TapeBlockFile::from_bytes(&bytes).unwrap(), sample());
    }

    #[test]
    fn multi_tape_and_multibyte_glyphs() {
        let block = TapeBlockFile {
            alphabet: vec!["·".into(), "↵".into(), "★".into()],
            tapes: vec![
                TapeSnapshot { origin: 0, cells: vec![2, 1, 0], head: 0 },
                TapeSnapshot { origin: -100, cells: vec![0], head: -100 },
            ],
        };
        let back = TapeBlockFile::from_bytes(&block.to_bytes()).unwrap();
        assert_eq!(back, block);
    }

    #[test]
    fn cell_outside_alphabet_rejected() {
        let mut block = sample();
        block.tapes[0].cells[0] = 9;
        let bytes = block.to_bytes();
        assert!(matches!(
            TapeBlockFile::from_bytes(&bytes),
            Err(FormatError::Malformed("cell index outside alphabet"))
        ));
    }

    #[test]
    fn empty_alphabet_rejected() {
        let block = TapeBlockFile { alphabet: vec![], tapes: sample().tapes };
        let bytes = block.to_bytes();
        assert!(matches!(
            TapeBlockFile::from_bytes(&bytes),
            Err(FormatError::Malformed("empty alphabet"))
        ));
    }

    #[test]
    fn corruption_rejected() {
        let mut bytes = sample().to_bytes();
        bytes[12] ^= 0xFF;
        assert!(matches!(
            TapeBlockFile::from_bytes(&bytes),
            Err(FormatError::BadCrc { .. })
        ));
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p mtc-core tapeblock`
Expected: compile error — types not defined.

- [ ] **Step 3: Implement**

`crates/core/src/formats/tapeblock.rs` (above the tests):
```rust
//! `MT` tape-block container (spec §6.3).

use super::crc32::{stamp_crc, verify_crc};
use super::io::{put_i64, put_u16, put_u32, Reader};
use super::{FormatError, FORMAT_VERSION};

pub const MAGIC_TAPEBLOCK: [u8; 3] = [b'M', b'T', 0x01];
const CRC_OFFSET: usize = 6;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TapeBlockFile {
    pub alphabet: Vec<String>,
    pub tapes: Vec<TapeSnapshot>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TapeSnapshot {
    pub origin: i64,
    pub cells: Vec<u8>,
    pub head: i64,
}

impl TapeBlockFile {
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&MAGIC_TAPEBLOCK);
        put_u16(&mut out, FORMAT_VERSION);
        out.push(0); // flags
        put_u32(&mut out, 0); // crc placeholder

        out.push(self.alphabet.len() as u8);
        for glyph in &self.alphabet {
            put_u16(&mut out, glyph.len() as u16);
            out.extend_from_slice(glyph.as_bytes());
        }

        out.push(self.tapes.len() as u8);
        for tape in &self.tapes {
            put_i64(&mut out, tape.origin);
            put_u32(&mut out, tape.cells.len() as u32);
            out.extend_from_slice(&tape.cells);
            put_i64(&mut out, tape.head);
        }

        stamp_crc(&mut out, CRC_OFFSET);
        out
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, FormatError> {
        if bytes.len() < 3 {
            return Err(FormatError::Truncated);
        }
        if bytes[0..3] != MAGIC_TAPEBLOCK {
            return Err(FormatError::BadMagic);
        }
        verify_crc(bytes, CRC_OFFSET)?;

        let mut r = Reader::new(&bytes[3..]);
        let version = r.u16()?;
        if version != FORMAT_VERSION {
            return Err(FormatError::UnsupportedVersion(version));
        }
        let _flags = r.u8()?;
        let _crc = r.u32()?;

        let alphabet_count = r.u8()? as usize;
        if alphabet_count == 0 {
            return Err(FormatError::Malformed("empty alphabet"));
        }
        let mut alphabet = Vec::with_capacity(alphabet_count);
        for _ in 0..alphabet_count {
            let len = r.u16()? as usize;
            let raw = r.bytes(len)?;
            let glyph = std::str::from_utf8(raw)
                .map_err(|_| FormatError::Malformed("glyph not utf-8"))?;
            alphabet.push(glyph.to_owned());
        }

        let tape_count = r.u8()? as usize;
        if tape_count == 0 {
            return Err(FormatError::Malformed("no tapes"));
        }
        let mut tapes = Vec::with_capacity(tape_count);
        for _ in 0..tape_count {
            let origin = r.i64()?;
            let length = r.u32()? as usize;
            let cells = r.bytes(length)?.to_vec();
            let head = r.i64()?;
            if cells.iter().any(|&c| c as usize >= alphabet_count) {
                return Err(FormatError::Malformed("cell index outside alphabet"));
            }
            tapes.push(TapeSnapshot { origin, cells, head });
        }
        r.finish()?;

        Ok(Self { alphabet, tapes })
    }
}
```

In `crates/core/src/formats/mod.rs` add:
```rust
pub mod tapeblock;
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p mtc-core`
Expected: all tests pass.

- [ ] **Step 5: Gates + commit**

Run: `cargo clippy --workspace --all-targets -- -D warnings && cargo fmt`
```bash
git add -A
git commit -m "feat(core): MT tape-block container codec"
```

---

### Task 7: Property tests for format round-trips

**Files:**
- Modify: `crates/core/Cargo.toml` (add dev-dependency)
- Create: `crates/core/tests/format_roundtrips.rs`

**Interfaces:**
- Consumes: the three public codecs from Tasks 4–6.
- Produces: confidence — arbitrary well-formed values survive `to_bytes → from_bytes`, and arbitrary byte noise never panics (returns `Err`, never crashes).

- [ ] **Step 1: Add the dev-dependency**

In `crates/core/Cargo.toml`:
```toml
[dev-dependencies]
proptest = "1"
```

- [ ] **Step 2: Write the property tests**

`crates/core/tests/format_roundtrips.rs`:
```rust
use mtc_core::formats::executable::Executable;
use mtc_core::formats::object::{ObjectFile, Relocation, Symbol, SymbolDef};
use mtc_core::formats::tapeblock::{TapeBlockFile, TapeSnapshot};
use proptest::prelude::*;

proptest! {
    #[test]
    fn executable_round_trips(
        arch in any::<u8>(),
        code in proptest::collection::vec(any::<u8>(), 1..512),
        entry_seed in any::<u32>(),
    ) {
        let entry = entry_seed % code.len() as u32;
        let exe = Executable { arch, entry, code };
        let back = Executable::from_bytes(&exe.to_bytes()).unwrap();
        prop_assert_eq!(back, exe);
    }

    #[test]
    fn executable_never_panics_on_noise(noise in proptest::collection::vec(any::<u8>(), 0..64)) {
        let _ = Executable::from_bytes(&noise); // must return Err, not panic
    }

    #[test]
    fn object_round_trips(
        blob in proptest::collection::vec(any::<u8>(), 5..64),
        name in "[a-zA-Z_][a-zA-Z0-9_]{0,12}",
        offset_seed in any::<u32>(),
    ) {
        let offset = offset_seed % (blob.len() as u32 - 4);
        let obj = ObjectFile {
            arch: 1,
            symbols: vec![
                Symbol { name: name.clone(), def: SymbolDef::Defined { blob: 0 } },
                Symbol { name: format!("{name}_ext"), def: SymbolDef::External },
            ],
            blobs: vec![blob],
            relocations: vec![Relocation { blob: 0, offset, symbol: 1 }],
            debug: None,
        };
        let back = ObjectFile::from_bytes(&obj.to_bytes()).unwrap();
        prop_assert_eq!(back, obj);
    }

    #[test]
    fn object_never_panics_on_noise(noise in proptest::collection::vec(any::<u8>(), 0..64)) {
        let _ = ObjectFile::from_bytes(&noise);
    }

    #[test]
    fn tapeblock_round_trips(
        origin in any::<i64>(),
        head in any::<i64>(),
        cells in proptest::collection::vec(0u8..2, 1..128),
    ) {
        let block = TapeBlockFile {
            alphabet: vec![" ".into(), "*".into()],
            tapes: vec![TapeSnapshot { origin, cells, head }],
        };
        let back = TapeBlockFile::from_bytes(&block.to_bytes()).unwrap();
        prop_assert_eq!(back, block);
    }

    #[test]
    fn tapeblock_never_panics_on_noise(noise in proptest::collection::vec(any::<u8>(), 0..64)) {
        let _ = TapeBlockFile::from_bytes(&noise);
    }
}
```

- [ ] **Step 3: Run the property tests**

Run: `cargo test -p mtc-core --test format_roundtrips`
Expected: 6 property tests pass (256 cases each by default). If a noise test panics, that's a real codec bug — fix the codec (add the missing bounds check), not the test.

- [ ] **Step 4: Full gates**

Run: `cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --check`
Expected: everything green.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "test(core): property-based round-trip and no-panic tests for MO/MX/MT"
```

---

## Self-Review Notes

- **Spec coverage (this plan's slice):** §6 intro (LE, magics+epoch, extension-vs-magic — extensions are caller concerns, codecs don't dispatch on them ✓), §6.1 layout+crc ✓, §6.2 layout+string pool+validation+optional debug ✓, §6.3 layout+alphabet+multi-tape ✓, §10 workspace/crates/gates ✓. Deliberately NOT here (later plans): `.pmx.map` sidecar (Plan 4, it's a linker output), `--tape` string parsing (Plan 7, CLI), arch validation (Plan 2, VM registry).
- **Type consistency:** `FormatError` variants used in Tasks 4–6 are all declared in Task 2. `ARCH_PM1` declared in Task 4's mod.rs edit, used in Tasks 4–5 tests. `Reader::finish` trailing check used by all three codecs.
- **Known simplification:** `verify_crc` copies the buffer to zero the crc field — fine at these file sizes; optimize only if profiling ever cares.
