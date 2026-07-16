//! Frame descriptors: the decoded form of the table-ROM records that back
//! the frames execution profile, and the byte-at-a-time walk that loads
//! them over the bus. Pure state machines — the core owns the bus, this
//! module owns the descriptor semantics (mirrors `table.rs`).
//!
//! Frame descriptor byte layout (normative here until the durable formats
//! page gains its frame-descriptor section):
//!
//! ```text
//! offset 0:  arity       u8   — virtual tapes (1..=16)
//! offset 1:  exit_count  u16  LE
//! offset 3:  arity × [ phys      u8      — physical tape for this virtual tape
//!                      rmap_len  u16 LE
//!                      rmap      rmap_len × u16 LE — indexed by PHYSICAL
//!                                symbol, yielding the virtual symbol
//!                      wmap_len  u16 LE
//!                      wmap      wmap_len × u16 LE — indexed by VIRTUAL
//!                                symbol, yielding the physical symbol ]
//! then:      exits       exit_count × u32 LE — absolute code addresses
//! ```
//!
//! A map entry of `0xFFFF` is a hole (crossing it traps); a `*_len` of 0
//! is the identity map.

/// One virtual tape of a frame: its physical target and symbol maps.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FrameEntry {
    pub(crate) phys: u8,
    pub(crate) rmap: Vec<u16>,
    pub(crate) wmap: Vec<u16>,
}

/// A decoded frame descriptor. `entries.len()` is the arity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FrameDescriptor {
    pub(crate) entries: Vec<FrameEntry>,
    pub(crate) exits: Vec<u32>,
}
