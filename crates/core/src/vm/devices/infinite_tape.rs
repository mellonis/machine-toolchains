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
}

#[cfg(test)]
mod tests {
    use super::*;

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
