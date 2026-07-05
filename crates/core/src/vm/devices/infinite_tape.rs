//! Unbounded two-symbol tape with paged sparse storage (spec §4.2):
//! `TBelt`'s packed bit array, generalized to an infinite tape.

use std::collections::HashMap;

use super::Tape;
use crate::vm::trap::DeviceFault;

const PAGE_BITS: i64 = 64;

#[derive(Debug, Default)]
pub struct InfiniteTape {
    pages: HashMap<i64, u64>,
    head: i64,
}

impl InfiniteTape {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_cells(
        cells: impl IntoIterator<Item = bool>,
        first_cell_at: i64,
        head: i64,
    ) -> Self {
        let mut tape = Self {
            pages: HashMap::new(),
            head,
        };
        for (i, marked) in cells.into_iter().enumerate() {
            if marked {
                tape.set(first_cell_at + i as i64, true);
            }
        }
        tape
    }

    pub fn head(&self) -> i64 {
        self.head
    }

    pub fn page_count(&self) -> usize {
        self.pages.len()
    }

    pub fn marked_cells(&self) -> Vec<i64> {
        let mut out = Vec::new();
        for (&page, &bits) in &self.pages {
            for bit in 0..PAGE_BITS {
                if bits & (1u64 << bit) != 0 {
                    out.push(page * PAGE_BITS + bit);
                }
            }
        }
        out.sort_unstable();
        out
    }

    fn get(&self, coord: i64) -> bool {
        let page = coord.div_euclid(PAGE_BITS);
        let bit = coord.rem_euclid(PAGE_BITS);
        self.pages
            .get(&page)
            .is_some_and(|bits| bits & (1u64 << bit) != 0)
    }

    fn set(&mut self, coord: i64, marked: bool) {
        let page = coord.div_euclid(PAGE_BITS);
        let bit = coord.rem_euclid(PAGE_BITS);
        if marked {
            *self.pages.entry(page).or_insert(0) |= 1u64 << bit;
        } else if let Some(bits) = self.pages.get_mut(&page) {
            *bits &= !(1u64 << bit);
            if *bits == 0 {
                self.pages.remove(&page); // freed: memory stays O(non-blank pages)
            }
        }
    }

    /// Build from a `TapeSnapshot` (spec §6.3). Cells must be 0/1 —
    /// a wider index is the snapshot's problem, not this tape's.
    pub fn from_snapshot(s: &crate::formats::tapeblock::TapeSnapshot) -> Result<Self, DeviceFault> {
        if let Some(&bad) = s.cells.iter().find(|&&c| c > 1) {
            return Err(DeviceFault::IndexOutsideAlphabet {
                index: u32::from(bad),
            });
        }
        Ok(Self::from_cells(
            s.cells.iter().map(|&c| c == 1),
            s.origin,
            s.head,
        ))
    }

    /// Dense snapshot spanning marked cells ∪ head (blank tape → one
    /// blank cell at the head).
    pub fn to_snapshot(&self) -> crate::formats::tapeblock::TapeSnapshot {
        let marks = self.marked_cells();
        let lo = marks.first().copied().unwrap_or(self.head).min(self.head);
        let hi = marks.last().copied().unwrap_or(self.head).max(self.head);
        let cells = (lo..=hi).map(|c| u8::from(self.get(c))).collect();
        crate::formats::tapeblock::TapeSnapshot {
            origin: lo,
            cells,
            head: self.head,
        }
    }
}

impl PartialEq for InfiniteTape {
    fn eq(&self, other: &Self) -> bool {
        self.marked_cells() == other.marked_cells() && self.head() == other.head()
    }
}

impl Tape for InfiniteTape {
    fn alphabet_size(&self) -> u32 {
        2
    }

    fn left(&mut self) {
        self.head -= 1;
    }

    fn right(&mut self) {
        self.head += 1;
    }

    fn read(&self) -> u32 {
        u32::from(self.get(self.head))
    }

    fn write(&mut self, index: u32) -> Result<(), DeviceFault> {
        if index >= self.alphabet_size() {
            return Err(DeviceFault::IndexOutsideAlphabet { index });
        }
        self.set(self.head, index == 1);
        Ok(())
    }

    fn head(&self) -> i64 {
        self.head()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::formats::tapeblock::TapeSnapshot;

    #[test]
    fn from_snapshot_places_cells_and_head() {
        let snap = TapeSnapshot {
            origin: -2,
            cells: vec![1, 0, 1, 1, 0],
            head: 1,
        };
        let tape = InfiniteTape::from_snapshot(&snap).unwrap();
        assert_eq!(tape.marked_cells(), vec![-2, 0, 1]);
        assert_eq!(tape.head(), 1);
        assert_eq!(tape.read(), 1);
    }

    #[test]
    fn from_snapshot_rejects_wide_alphabet_cells() {
        let snap = TapeSnapshot {
            origin: 0,
            cells: vec![0, 2],
            head: 0,
        };
        assert_eq!(
            InfiniteTape::from_snapshot(&snap),
            Err(DeviceFault::IndexOutsideAlphabet { index: 2 })
        );
    }

    #[test]
    fn to_snapshot_covers_marks_and_head() {
        let mut tape = InfiniteTape::from_cells([true, false, true], 0, 0);
        for _ in 0..5 {
            tape.right(); // head 5, past the data
        }
        let snap = tape.to_snapshot();
        assert_eq!(snap.origin, 0);
        assert_eq!(snap.cells, vec![1, 0, 1, 0, 0, 0]); // span 0..=5 (marks ∪ head)
        assert_eq!(snap.head, 5);
    }

    #[test]
    fn blank_tape_snapshot_is_single_cell_at_head() {
        let mut tape = InfiniteTape::new();
        tape.left();
        tape.left();
        let snap = tape.to_snapshot();
        assert_eq!(snap.origin, -2);
        assert_eq!(snap.cells, vec![0]);
        assert_eq!(snap.head, -2);
    }

    #[test]
    fn snapshot_round_trip_law() {
        let mut tape = InfiniteTape::from_cells([true, true, false, true], -3, 2);
        tape.write(1).unwrap();
        let back = InfiniteTape::from_snapshot(&tape.to_snapshot()).unwrap();
        assert_eq!(back, tape); // exercises the manual PartialEq (marks + head)
    }

    #[test]
    fn blank_tape_reads_zero_everywhere_without_allocating() {
        let mut tape = InfiniteTape::new();
        for _ in 0..10_000 {
            tape.right();
            assert_eq!(tape.read(), 0);
        }
        for _ in 0..20_000 {
            tape.left();
            assert_eq!(tape.read(), 0);
        }
        assert_eq!(tape.page_count(), 0); // reads never allocate
        assert_eq!(tape.head(), -10_000);
    }

    #[test]
    fn write_read_round_trip_across_page_boundaries() {
        let mut tape = InfiniteTape::new();
        // mark cells -1, 0, 63, 64 (spans three pages: -1, 0, 1)
        for target in [-1i64, 0, 63, 64] {
            while tape.head() < target {
                tape.right();
            }
            while tape.head() > target {
                tape.left();
            }
            tape.write(1).unwrap();
        }
        assert_eq!(tape.marked_cells(), vec![-1, 0, 63, 64]);
        assert_eq!(tape.page_count(), 3);
    }

    #[test]
    fn erasing_last_mark_frees_the_page() {
        let mut tape = InfiniteTape::new();
        tape.write(1).unwrap();
        assert_eq!(tape.page_count(), 1);
        tape.write(0).unwrap();
        assert_eq!(tape.page_count(), 0);
    }

    #[test]
    fn idempotent_writes_are_ok() {
        let mut tape = InfiniteTape::new();
        tape.write(1).unwrap();
        tape.write(1).unwrap(); // marking a marked cell: fine on a default tape
        assert_eq!(tape.read(), 1);
        tape.write(0).unwrap();
        tape.write(0).unwrap();
        assert_eq!(tape.read(), 0);
    }

    #[test]
    fn out_of_alphabet_write_faults() {
        let mut tape = InfiniteTape::new();
        assert_eq!(
            tape.write(2),
            Err(DeviceFault::IndexOutsideAlphabet { index: 2 })
        );
    }

    #[test]
    fn from_cells_places_data_and_head() {
        let tape = InfiniteTape::from_cells([false, true, true, false, true], 0, 2);
        assert_eq!(tape.marked_cells(), vec![1, 2, 4]);
        assert_eq!(tape.head(), 2);
        assert_eq!(tape.read(), 1);
    }
}
