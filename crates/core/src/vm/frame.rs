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

pub(crate) enum FrameStep {
    NeedByte(u32),
    Done(FrameDescriptor),
    Malformed,
}

/// The field the walk is currently accumulating. Descriptor bytes are
/// strictly sequential, so the walk is a field-width automaton over one
/// running offset (unlike `MatchWalk`, which skips between rows).
enum Field {
    Arity,
    ExitCount,
    Phys,
    RmapLen,
    RmapEntry { remaining: u16 },
    WmapLen,
    WmapEntry { remaining: u16 },
    Exit { remaining: u16 },
}

impl Field {
    fn width(&self) -> usize {
        match self {
            Field::Arity | Field::Phys => 1,
            Field::ExitCount
            | Field::RmapLen
            | Field::RmapEntry { .. }
            | Field::WmapLen
            | Field::WmapEntry { .. } => 2,
            Field::Exit { .. } => 4,
        }
    }
}

/// Byte-at-a-time descriptor load: request-per-byte over the bus, like
/// `MatchWalk`. Malformity here is only the arity bounds (1..=16);
/// truncation surfaces as a request past the table end, which the core
/// maps to a table-bounds trap.
pub(crate) struct FrameWalk {
    base: u32,
    consumed: u32,
    pending: Vec<u8>,
    field: Field,
    exit_count: u16,
    arity: u8,
    entries: Vec<FrameEntry>,
    cur: Option<FrameEntry>,
    exits: Vec<u32>,
}

impl FrameWalk {
    pub(crate) fn new(base: u32) -> Self {
        Self {
            base,
            consumed: 0,
            pending: Vec::new(),
            field: Field::Arity,
            exit_count: 0,
            arity: 0,
            entries: Vec::new(),
            cur: None,
            exits: Vec::new(),
        }
    }

    fn need_next(&self) -> FrameStep {
        FrameStep::NeedByte(self.base + self.consumed)
    }

    /// One virtual tape fully decoded: bank it and move to the next
    /// entry, the exit vector, or completion.
    fn finish_entry(&mut self) -> Option<FrameStep> {
        let entry = self.cur.take().expect("an entry is under construction");
        self.entries.push(entry);
        if self.entries.len() < usize::from(self.arity) {
            self.field = Field::Phys;
            None
        } else if self.exit_count > 0 {
            self.field = Field::Exit {
                remaining: self.exit_count,
            };
            None
        } else {
            Some(self.done())
        }
    }

    fn done(&mut self) -> FrameStep {
        FrameStep::Done(FrameDescriptor {
            entries: std::mem::take(&mut self.entries),
            exits: std::mem::take(&mut self.exits),
        })
    }

    pub(crate) fn feed(&mut self, byte: Option<u8>) -> FrameStep {
        let Some(b) = byte else {
            // feed(None) is only legal on a fresh walk; anything else is
            // a core-side protocol bug.
            if self.consumed == 0 {
                return self.need_next();
            }
            return FrameStep::Malformed;
        };
        self.pending.push(b);
        self.consumed += 1;
        if self.pending.len() < self.field.width() {
            return self.need_next();
        }
        let raw = std::mem::take(&mut self.pending);
        let u16_of = |raw: &[u8]| u16::from_le_bytes([raw[0], raw[1]]);
        match self.field {
            Field::Arity => {
                self.arity = raw[0];
                if self.arity == 0 || self.arity > 16 {
                    return FrameStep::Malformed;
                }
                self.field = Field::ExitCount;
            }
            Field::ExitCount => {
                self.exit_count = u16_of(&raw);
                self.field = Field::Phys;
            }
            Field::Phys => {
                self.cur = Some(FrameEntry {
                    phys: raw[0],
                    rmap: Vec::new(),
                    wmap: Vec::new(),
                });
                self.field = Field::RmapLen;
            }
            Field::RmapLen => {
                let len = u16_of(&raw);
                if len == 0 {
                    self.field = Field::WmapLen;
                } else {
                    self.field = Field::RmapEntry { remaining: len };
                }
            }
            Field::RmapEntry { remaining } => {
                let entry = self.cur.as_mut().expect("an entry is under construction");
                entry.rmap.push(u16_of(&raw));
                if remaining == 1 {
                    self.field = Field::WmapLen;
                } else {
                    self.field = Field::RmapEntry {
                        remaining: remaining - 1,
                    };
                }
            }
            Field::WmapLen => {
                let len = u16_of(&raw);
                if len == 0 {
                    if let Some(step) = self.finish_entry() {
                        return step;
                    }
                } else {
                    self.field = Field::WmapEntry { remaining: len };
                }
            }
            Field::WmapEntry { remaining } => {
                let entry = self.cur.as_mut().expect("an entry is under construction");
                entry.wmap.push(u16_of(&raw));
                if remaining == 1 {
                    if let Some(step) = self.finish_entry() {
                        return step;
                    }
                } else {
                    self.field = Field::WmapEntry {
                        remaining: remaining - 1,
                    };
                }
            }
            Field::Exit { remaining } => {
                self.exits
                    .push(u32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]]));
                if remaining == 1 {
                    return self.done();
                }
                self.field = Field::Exit {
                    remaining: remaining - 1,
                };
            }
        }
        self.need_next()
    }
}

/// Test-only descriptor encoder — the inverse of `FrameWalk`, shared by
/// the frame, core, and driver test modules.
#[cfg(test)]
pub(crate) mod test_support {
    /// Encode a descriptor per the wire layout in the module doc:
    /// `entries` is `(phys, rmap, wmap)` per virtual tape.
    pub(crate) fn descriptor_bytes(entries: &[(u8, &[u16], &[u16])], exits: &[u32]) -> Vec<u8> {
        let mut out = vec![entries.len() as u8];
        out.extend((exits.len() as u16).to_le_bytes());
        for (phys, rmap, wmap) in entries {
            out.push(*phys);
            out.extend((rmap.len() as u16).to_le_bytes());
            for &m in *rmap {
                out.extend(m.to_le_bytes());
            }
            out.extend((wmap.len() as u16).to_le_bytes());
            for &m in *wmap {
                out.extend(m.to_le_bytes());
            }
        }
        for &e in exits {
            out.extend(e.to_le_bytes());
        }
        out
    }

    /// Encode a frames region (docs/formats.md (frames region)): `K u16`,
    /// `S u16`, directory (`K × u32` descriptor offsets), compose table
    /// (`(K+1) × S × u16`, one row per active FR 0..=K). `compose` is the
    /// row-major table the caller supplies verbatim, so a test derives the
    /// exact bytes from the region layout rather than from a run.
    pub(crate) fn region_bytes(directory: &[u32], compose: &[&[u16]]) -> Vec<u8> {
        let k = directory.len() as u16;
        let s = compose.first().map_or(0, |row| row.len()) as u16;
        let mut out = Vec::new();
        out.extend(k.to_le_bytes());
        out.extend(s.to_le_bytes());
        for &off in directory {
            out.extend(off.to_le_bytes());
        }
        for row in compose {
            for &c in *row {
                out.extend(c.to_le_bytes());
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::descriptor_bytes;
    use super::*;

    /// Run a FrameWalk to completion against an in-memory blob laid at
    /// `base` within a larger table space.
    fn walk_at(table: &[u8], base: u32) -> Result<FrameDescriptor, &'static str> {
        let mut w = FrameWalk::new(base);
        let mut input = None;
        loop {
            match w.feed(input) {
                FrameStep::NeedByte(addr) => {
                    input = Some(*table.get(addr as usize).ok_or("out of table")?);
                }
                FrameStep::Done(desc) => return Ok(desc),
                FrameStep::Malformed => return Err("malformed"),
            }
        }
    }

    fn walk(table: &[u8]) -> Result<FrameDescriptor, &'static str> {
        walk_at(table, 0)
    }

    #[test]
    fn decodes_a_full_descriptor() {
        // arity 2: virtual 0 → phys 3 with non-identity maps (incl. a
        // hole), virtual 1 → phys 0 identity; two exits.
        let blob = descriptor_bytes(
            &[(3, &[1, 0, 0xFFFF], &[2, 0xFFFF]), (0, &[], &[])],
            &[0x11223344, 7],
        );
        let desc = walk(&blob).unwrap();
        assert_eq!(desc.entries.len(), 2);
        assert_eq!(desc.entries[0].phys, 3);
        assert_eq!(desc.entries[0].rmap, vec![1, 0, 0xFFFF]);
        assert_eq!(desc.entries[0].wmap, vec![2, 0xFFFF]);
        assert_eq!(desc.entries[1].phys, 0);
        assert!(desc.entries[1].rmap.is_empty());
        assert!(desc.entries[1].wmap.is_empty());
        assert_eq!(desc.exits, vec![0x11223344, 7]);
    }

    #[test]
    fn decodes_at_a_non_zero_base() {
        // The same descriptor placed after 5 pad bytes — every request
        // must be base-relative, or the pads would corrupt the decode.
        let mut table = vec![0xEE; 5];
        table.extend(descriptor_bytes(&[(1, &[0, 1], &[])], &[9]));
        let desc = walk_at(&table, 5).unwrap();
        assert_eq!(desc.entries[0].phys, 1);
        assert_eq!(desc.entries[0].rmap, vec![0, 1]);
        assert_eq!(desc.exits, vec![9]);
    }

    #[test]
    fn zero_exit_count_yields_empty_exits() {
        let desc = walk(&descriptor_bytes(&[(0, &[], &[])], &[])).unwrap();
        assert!(desc.exits.is_empty());
        assert_eq!(desc.entries.len(), 1);
    }

    #[test]
    fn arity_zero_is_malformed() {
        assert_eq!(walk(&descriptor_bytes(&[], &[1])), Err("malformed"));
    }

    #[test]
    fn arity_over_sixteen_is_malformed() {
        let entries: Vec<(u8, &[u16], &[u16])> =
            (0..17).map(|i| (i as u8, &[][..], &[][..])).collect();
        assert_eq!(walk(&descriptor_bytes(&entries, &[1])), Err("malformed"));
    }

    #[test]
    fn arity_sixteen_is_accepted() {
        let entries: Vec<(u8, &[u16], &[u16])> =
            (0..16).map(|i| (i as u8, &[][..], &[][..])).collect();
        let desc = walk(&descriptor_bytes(&entries, &[1])).unwrap();
        assert_eq!(desc.entries.len(), 16);
    }

    #[test]
    fn truncation_runs_off_the_table() {
        // Every strict prefix of a valid descriptor must keep requesting
        // (i.e. run off the blob) rather than complete or mis-decode.
        let blob = descriptor_bytes(&[(3, &[1, 0], &[2])], &[11, 12]);
        for cut in 0..blob.len() {
            assert_eq!(
                walk(&blob[..cut]),
                Err("out of table"),
                "prefix of {cut} bytes must not decode"
            );
        }
        assert!(walk(&blob).is_ok());
    }

    proptest::proptest! {
        /// Arbitrary/noise descriptor bytes must always terminate the walk in
        /// a decode, a malformed verdict, or off-the-end (which the core maps
        /// to a table-bounds trap) — never a panic or a hang. The fixed
        /// truncation and arity-bound cases above cover specific malformity;
        /// this fuzzes the whole byte space.
        #[test]
        fn framewalk_never_panics_on_noise(
            noise in proptest::collection::vec(proptest::prelude::any::<u8>(), 0..64),
        ) {
            let _ = walk(&noise); // returns Ok/Err, never panics or hangs
        }
    }
}
