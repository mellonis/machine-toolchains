//! Ring-shaped bounded tape — the historical `TBelt` (spec §4.2).

use super::Tape;
use crate::vm::trap::DeviceFault;

#[derive(Debug)]
pub struct AnnularTape {
    words: Vec<u64>,
    size: u32,
    head: u32,
}

impl AnnularTape {
    pub fn new(size: u32) -> Self {
        assert!(size > 0, "annular tape needs at least one cell");
        Self {
            words: vec![0; size.div_ceil(64) as usize],
            size,
            head: 0,
        }
    }

    pub fn head(&self) -> u32 {
        self.head
    }

    fn get(&self, at: u32) -> bool {
        self.words[(at / 64) as usize] & (1u64 << (at % 64)) != 0
    }
}

impl Tape for AnnularTape {
    fn alphabet_size(&self) -> u32 {
        2
    }

    fn left(&mut self) {
        self.head = if self.head == 0 {
            self.size - 1
        } else {
            self.head - 1
        };
    }

    fn right(&mut self) {
        self.head = if self.head == self.size - 1 {
            0
        } else {
            self.head + 1
        };
    }

    fn read(&self) -> u32 {
        u32::from(self.get(self.head))
    }

    fn write(&mut self, index: u32) -> Result<(), DeviceFault> {
        if index >= self.alphabet_size() {
            return Err(DeviceFault::IndexOutsideAlphabet { index });
        }
        let word = &mut self.words[(self.head / 64) as usize];
        let bit = 1u64 << (self.head % 64);
        if index == 1 {
            *word |= bit;
        } else {
            *word &= !bit;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vm::devices::Tape;
    use crate::vm::trap::DeviceFault;

    #[test]
    fn wraps_both_directions() {
        let mut tape = AnnularTape::new(4);
        assert_eq!(tape.head(), 0);
        tape.left();
        assert_eq!(tape.head(), 3);
        tape.right();
        tape.right();
        tape.right();
        tape.right();
        tape.right();
        assert_eq!(tape.head(), 0);
    }

    #[test]
    fn a_full_lap_returns_to_written_cell() {
        let mut tape = AnnularTape::new(100);
        tape.write(1).unwrap();
        for _ in 0..100 {
            tape.right();
        }
        assert_eq!(tape.read(), 1); // the wrap detector's mark, found again
    }

    #[test]
    fn spans_multiple_words() {
        let mut tape = AnnularTape::new(130); // 3 u64 words
        for _ in 0..129 {
            tape.right();
        }
        tape.write(1).unwrap();
        assert_eq!(tape.head(), 129);
        assert_eq!(tape.read(), 1);
        tape.right(); // wraps to 0
        assert_eq!(tape.read(), 0);
    }

    #[test]
    fn out_of_alphabet_faults() {
        let mut tape = AnnularTape::new(8);
        assert_eq!(
            tape.write(7),
            Err(DeviceFault::IndexOutsideAlphabet { index: 7 })
        );
    }
}
