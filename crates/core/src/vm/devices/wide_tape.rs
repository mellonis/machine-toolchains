//! Unbounded wide-alphabet tape with paged sparse storage (docs/isa.md (the
//! tape and device bus)). `InfiniteTape` is physically two-symbol (a packed
//! bit array); this device generalizes it to per-cell symbol indices in
//! `0..width`, for architectures whose tapes declare alphabets wider than
//! two. Blank cells (index 0) are never stored, so memory stays O(non-blank
//! cells); the layout mirrors `InfiniteTape`'s so their snapshots agree.

use std::collections::HashMap;

use super::Tape;
use crate::vm::trap::DeviceFault;

#[derive(Debug)]
pub struct WideTape {
    cells: HashMap<i64, u32>,
    width: u32,
    head: i64,
}

impl WideTape {
    /// A blank tape over an alphabet of `width` symbols (indices `0..width`).
    /// `width` must be `1..=256`: an empty alphabet cannot back a tape, and
    /// the snapshot representation (docs/formats.md (tape-block snapshot))
    /// stores cells as `u8`, so the largest index (`width - 1`) must fit a
    /// byte. An out-of-range `width` is a caller bug.
    pub fn new(width: u32) -> Self {
        assert!(width >= 1, "wide tape needs a non-empty alphabet");
        assert!(
            width <= 256,
            "wide tape alphabet exceeds 256 symbols (snapshot cells are u8)"
        );
        Self {
            cells: HashMap::new(),
            width,
            head: 0,
        }
    }

    pub fn head(&self) -> i64 {
        self.head
    }

    /// Sorted coordinates of the non-blank cells (index != 0).
    fn nonblank_cells(&self) -> Vec<i64> {
        let mut out: Vec<i64> = self.cells.keys().copied().collect();
        out.sort_unstable();
        out
    }

    fn get(&self, coord: i64) -> u32 {
        self.cells.get(&coord).copied().unwrap_or(0)
    }

    fn set(&mut self, coord: i64, index: u32) {
        if index == 0 {
            self.cells.remove(&coord); // blank cells are never stored
        } else {
            self.cells.insert(coord, index);
        }
    }

    /// Build from a `TapeSnapshot` (docs/formats.md). Snapshot cells are `u8`;
    /// any cell `>= width` is outside this tape's alphabet and is rejected
    /// (mirrors `InfiniteTape::from_snapshot`, which rejects cells `> 1`).
    /// `width` shares `new`'s `1..=256` bound — an out-of-range `width`
    /// panics there.
    pub fn from_snapshot(
        s: &crate::formats::tapeblock::TapeSnapshot,
        width: u32,
    ) -> Result<Self, DeviceFault> {
        if let Some(&bad) = s.cells.iter().find(|&&c| u32::from(c) >= width) {
            return Err(DeviceFault::IndexOutsideAlphabet {
                index: u32::from(bad),
            });
        }
        let mut tape = Self::new(width);
        tape.head = s.head;
        for (i, &cell) in s.cells.iter().enumerate() {
            tape.set(s.origin + i as i64, u32::from(cell));
        }
        Ok(tape)
    }

    /// Dense snapshot spanning non-blank cells ∪ head (blank tape → one blank
    /// cell at the head). Trim/origin policy is byte-identical to
    /// `InfiniteTape::to_snapshot` so downstream goldens agree.
    ///
    /// Cell indices narrow `u32 -> u8`, and the narrowing is total, not
    /// lossy: `TapeSnapshot.cells` is `Vec<u8>` and the MT container's
    /// per-tape alphabet-count field is a `u8` (docs/formats.md (tape-block
    /// snapshot)), so any snapshot-representable tape has `width <= 256` and
    /// every cell index (`< width`) is `<= 255`, which fits a byte. `new`
    /// enforces `width <= 256` at construction, so the bound always holds
    /// here. The `expect` is a hard invariant check — matching the
    /// `u8`-narrowing serialization asserts in the `.pmt` codec — rather than
    /// a silent truncation.
    pub fn to_snapshot(&self) -> crate::formats::tapeblock::TapeSnapshot {
        let marks = self.nonblank_cells();
        let lo = marks.first().copied().unwrap_or(self.head).min(self.head);
        let hi = marks.last().copied().unwrap_or(self.head).max(self.head);
        let cells = (lo..=hi)
            .map(|c| u8::try_from(self.get(c)).expect("cell index fits u8"))
            .collect();
        crate::formats::tapeblock::TapeSnapshot {
            origin: lo,
            cells,
            head: self.head,
            alphabet: None,
        }
    }
}

impl PartialEq for WideTape {
    fn eq(&self, other: &Self) -> bool {
        // Blank cells are never stored, so the maps are normalized: equal
        // contents ⇔ equal maps. Width is excluded, mirroring `InfiniteTape`
        // excluding its fixed alphabet size.
        self.cells == other.cells && self.head == other.head
    }
}

impl Tape for WideTape {
    fn alphabet_size(&self) -> u32 {
        self.width
    }

    fn left(&mut self) {
        self.head -= 1;
    }

    fn right(&mut self) {
        self.head += 1;
    }

    fn read(&self) -> u32 {
        self.get(self.head)
    }

    fn write(&mut self, index: u32) -> Result<(), DeviceFault> {
        if index >= self.width {
            return Err(DeviceFault::IndexOutsideAlphabet { index });
        }
        self.set(self.head, index);
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
            cells: vec![2, 0, 1, 2, 0],
            head: 1,
            alphabet: None,
        };
        let tape = WideTape::from_snapshot(&snap, 3).unwrap();
        assert_eq!(tape.nonblank_cells(), vec![-2, 0, 1]);
        assert_eq!(tape.head(), 1);
        assert_eq!(tape.read(), 2); // cell at head (origin -2 + index 3)
        assert_eq!(tape.alphabet_size(), 3);
    }

    #[test]
    fn from_snapshot_rejects_cells_at_or_above_width() {
        let snap = TapeSnapshot {
            origin: 0,
            cells: vec![0, 1, 3], // 3 is outside a width-3 alphabet (0..3)
            head: 0,
            alphabet: None,
        };
        assert_eq!(
            WideTape::from_snapshot(&snap, 3),
            Err(DeviceFault::IndexOutsideAlphabet { index: 3 })
        );
    }

    #[test]
    fn snapshot_round_trip_law() {
        let snap = TapeSnapshot {
            origin: -3,
            cells: vec![1, 2, 0, 1],
            head: 2,
            alphabet: None,
        };
        let mut tape = WideTape::from_snapshot(&snap, 3).unwrap();
        tape.write(2).unwrap();
        let back = WideTape::from_snapshot(&tape.to_snapshot(), 3).unwrap();
        assert_eq!(back, tape); // exercises the manual PartialEq (cells + head)
    }

    #[test]
    fn to_snapshot_covers_marks_and_head() {
        // A width-3 tape with two non-blank cells, head driven past the data.
        let snap = TapeSnapshot {
            origin: 0,
            cells: vec![1, 0, 2],
            head: 0,
            alphabet: None,
        };
        let mut tape = WideTape::from_snapshot(&snap, 3).unwrap();
        for _ in 0..5 {
            tape.right(); // head 5, past the data
        }
        let out = tape.to_snapshot();
        assert_eq!(out.origin, 0);
        assert_eq!(out.cells, vec![1, 0, 2, 0, 0, 0]); // span 0..=5 (marks ∪ head)
        assert_eq!(out.head, 5);
    }

    #[test]
    fn blank_tape_snapshot_is_single_cell_at_head() {
        let mut tape = WideTape::new(9);
        tape.left();
        tape.left();
        let out = tape.to_snapshot();
        assert_eq!(out.origin, -2);
        assert_eq!(out.cells, vec![0]);
        assert_eq!(out.head, -2);
    }

    #[test]
    fn blank_tape_reads_zero_everywhere_without_allocating() {
        let mut tape = WideTape::new(127);
        for _ in 0..10_000 {
            tape.right();
            assert_eq!(tape.read(), 0);
        }
        for _ in 0..20_000 {
            tape.left();
            assert_eq!(tape.read(), 0);
        }
        assert!(tape.cells.is_empty()); // reads never allocate
        assert_eq!(tape.head(), -10_000);
    }

    #[test]
    fn erasing_a_cell_frees_the_slot() {
        let mut tape = WideTape::new(3);
        tape.write(2).unwrap();
        assert_eq!(tape.cells.len(), 1);
        tape.write(0).unwrap(); // back to blank
        assert!(tape.cells.is_empty());
        assert_eq!(tape.read(), 0);
    }

    #[test]
    fn out_of_width_write_faults() {
        let mut tape = WideTape::new(3);
        assert_eq!(
            tape.write(3), // 3 is outside a width-3 alphabet (0..3)
            Err(DeviceFault::IndexOutsideAlphabet { index: 3 })
        );
        // The boundary symbol (width - 1) writes fine.
        tape.write(2).unwrap();
        assert_eq!(tape.read(), 2);
    }

    #[test]
    fn width_two_behaves_like_a_binary_tape() {
        let mut tape = WideTape::new(2);
        tape.write(1).unwrap();
        assert_eq!(tape.read(), 1);
        assert_eq!(
            tape.write(2),
            Err(DeviceFault::IndexOutsideAlphabet { index: 2 })
        );
    }

    #[test]
    #[should_panic(expected = "exceeds 256")]
    fn width_above_256_panics() {
        // Snapshot cells are `u8`, so a width past 256 (index 256 = 0x100)
        // could not narrow; the bound is enforced at construction.
        let _ = WideTape::new(257);
    }

    #[test]
    fn width_256_is_accepted_and_max_index_round_trips() {
        // The upper alphabet bound: width 256 means indices 0..256, so the
        // largest symbol (255) must round-trip through the u8 snapshot cells.
        let mut tape = WideTape::new(256);
        tape.write(255).unwrap();
        assert_eq!(tape.read(), 255);
        assert_eq!(tape.to_snapshot().cells, vec![255]);
        // 256 itself is outside the 0..256 index range.
        assert_eq!(
            tape.write(256),
            Err(DeviceFault::IndexOutsideAlphabet { index: 256 })
        );
    }

    #[test]
    fn strict_decoration_faults_on_rewriting_the_held_symbol() {
        use crate::vm::devices::StrictTape;

        let mut tape = StrictTape::new(WideTape::new(3));
        tape.write(2).unwrap();
        assert_eq!(tape.alphabet_size(), 3);
        // Rewriting the value a cell already holds is the strict violation.
        assert_eq!(tape.write(2), Err(DeviceFault::StrictCellViolation));
        // A different in-alphabet symbol writes fine; an out-of-width index
        // still surfaces the inner tape's fault, not a strict violation.
        tape.write(1).unwrap();
        assert_eq!(
            tape.write(5),
            Err(DeviceFault::IndexOutsideAlphabet { index: 5 })
        );
    }
}
