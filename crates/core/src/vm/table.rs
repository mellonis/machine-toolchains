//! Match-table walk (spec: docs/isa.md (match tables) once phase 8 lands;
//! until then the layout comment below is normative). Pure state machine:
//! the core owns the bus, this module owns the table semantics.
//!
//! Match table byte layout (compact family — one byte per row position):
//!
//! ```text
//! offset 0:  width      u8   — positions per row (1..=16)
//! offset 1:  row_count  u16  LE
//! offset 3:  rows       row_count × width bytes; each byte is a 7-bit symbol
//!                       payload; 0x7F = wildcard ("transparent", spec §3.2)
//! ```

// This module is a pure library not yet driven by the core; every item is
// unreferenced in the lib build until the core wires the walk. A module-scoped
// allow keeps the `-D warnings` gate green without touching sibling modules.
#![allow(dead_code)]

pub(crate) enum WalkStep {
    NeedByte(u32),
    Done(u32),
    Malformed,
}

enum Stage {
    Width,
    CountLo {
        width: u8,
    },
    CountHi {
        width: u8,
        lo: u8,
    },
    Row {
        width: u8,
        rows: u16,
        row: u16,
        pos: u8,
    },
}

pub(crate) struct MatchWalk {
    base: u32,
    stage: Stage,
}

impl MatchWalk {
    pub(crate) fn new(table_addr: u32) -> Self {
        Self {
            base: table_addr,
            stage: Stage::Width,
        }
    }

    fn row_byte_addr(&self, width: u8, row: u16, pos: u8) -> u32 {
        self.base + 3 + u32::from(row) * u32::from(width) + u32::from(pos)
    }

    pub(crate) fn feed(&mut self, byte: Option<u8>, tr: &[u32]) -> WalkStep {
        match (&self.stage, byte) {
            (Stage::Width, None) => WalkStep::NeedByte(self.base),
            (Stage::Width, Some(w)) => {
                if w == 0 || w > 16 || usize::from(w) > tr.len() {
                    return WalkStep::Malformed;
                }
                self.stage = Stage::CountLo { width: w };
                WalkStep::NeedByte(self.base + 1)
            }
            (Stage::CountLo { width }, Some(lo)) => {
                let width = *width; // copy before assigning to self.stage (borrowck)
                self.stage = Stage::CountHi { width, lo };
                WalkStep::NeedByte(self.base + 2)
            }
            (Stage::CountHi { width, lo }, Some(hi)) => {
                let (width, lo) = (*width, *lo); // copy before assigning (borrowck)
                let rows = u16::from_le_bytes([lo, hi]);
                if rows == 0 {
                    return WalkStep::Done(0);
                }
                self.stage = Stage::Row {
                    width,
                    rows,
                    row: 0,
                    pos: 0,
                };
                WalkStep::NeedByte(self.row_byte_addr(width, 0, 0))
            }
            (
                Stage::Row {
                    width,
                    rows,
                    row,
                    pos,
                },
                Some(b),
            ) => {
                let (width, rows, row, pos) = (*width, *rows, *row, *pos);
                let matches = b == 0x7F || u32::from(b) == tr[usize::from(pos)];
                if matches && pos + 1 == width {
                    return WalkStep::Done(u32::from(row) + 1); // 1-based MR
                }
                let (next_row, next_pos) = if matches {
                    (row, pos + 1) // same row, next position
                } else {
                    (row + 1, 0) // row failed: skip to the next row's base
                };
                if next_row == rows {
                    return WalkStep::Done(0);
                }
                self.stage = Stage::Row {
                    width,
                    rows,
                    row: next_row,
                    pos: next_pos,
                };
                WalkStep::NeedByte(self.row_byte_addr(width, next_row, next_pos))
            }
            // feed(None) is only legal on a fresh walk; anything else is a
            // core-side driver bug.
            _ => WalkStep::Malformed,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Run a MatchWalk to completion against an in-memory table blob.
    fn walk(table: &[u8], tr: &[u32]) -> Result<u32, &'static str> {
        let mut w = MatchWalk::new(0);
        let mut input = None;
        loop {
            match w.feed(input, tr) {
                WalkStep::NeedByte(addr) => {
                    input = Some(*table.get(addr as usize).ok_or("out of table")?);
                }
                WalkStep::Done(mr) => return Ok(mr),
                WalkStep::Malformed => return Err("malformed"),
            }
        }
    }

    /// width=2, three rows: [1,2] [1,0x7F] [0x7F,0x7F]
    fn sample() -> Vec<u8> {
        vec![2, 3, 0, 1, 2, 1, 0x7F, 0x7F, 0x7F]
    }

    #[test]
    fn first_match_wins_exact() {
        assert_eq!(walk(&sample(), &[1, 2]), Ok(1));
    }

    #[test]
    fn wildcard_matches_any_symbol() {
        assert_eq!(walk(&sample(), &[1, 9]), Ok(2)); // row 2: [1, *]
        assert_eq!(walk(&sample(), &[8, 8]), Ok(3)); // catch-all
    }

    #[test]
    fn no_match_yields_zero() {
        // table without catch-all: width=1, one row [3]
        assert_eq!(walk(&[1, 1, 0, 3], &[4]), Ok(0));
    }

    #[test]
    fn short_circuits_failed_row() {
        // A row failing at position 0 must not read its remaining bytes:
        // truncate row 1's second byte — walk must still reach row 2.
        // width=2, 2 rows: [5,?][0x7F,0x7F]; tr=[1,1] fails row 1 at pos 0.
        let table = vec![2, 2, 0, 5, 0, 0x7F, 0x7F];
        assert_eq!(walk(&table, &[1, 1]), Ok(2));
    }

    #[test]
    fn malformed_widths_rejected() {
        assert_eq!(walk(&[0, 1, 0], &[1]), Err("malformed")); // width 0
        assert_eq!(walk(&[17, 1, 0], &[1; 16]), Err("malformed")); // width 17
        assert_eq!(walk(&[3, 1, 0, 1, 1, 1], &[1, 1]), Err("malformed")); // width > tr
    }
}
