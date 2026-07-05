//! Strict-cells decorator: 2006/2007 semantics — writing the value a
//! cell already holds is an error (spec §4.2).

use super::Tape;
use crate::vm::trap::DeviceFault;

#[derive(Debug)]
pub struct StrictTape<T: Tape> {
    inner: T,
}

impl<T: Tape> StrictTape<T> {
    pub fn new(inner: T) -> Self {
        Self { inner }
    }

    pub fn into_inner(self) -> T {
        self.inner
    }
}

impl<T: Tape> Tape for StrictTape<T> {
    fn alphabet_size(&self) -> u32 {
        self.inner.alphabet_size()
    }

    fn left(&mut self) {
        self.inner.left();
    }

    fn right(&mut self) {
        self.inner.right();
    }

    fn read(&self) -> u32 {
        self.inner.read()
    }

    fn write(&mut self, index: u32) -> Result<(), DeviceFault> {
        if index < self.alphabet_size() && self.inner.read() == index {
            return Err(DeviceFault::StrictCellViolation);
        }
        self.inner.write(index)
    }

    fn head(&self) -> i64 {
        self.inner.head()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vm::devices::{InfiniteTape, Tape};
    use crate::vm::trap::DeviceFault;

    #[test]
    fn double_mark_and_double_erase_fault() {
        let mut tape = StrictTape::new(InfiniteTape::new());
        assert_eq!(tape.write(0), Err(DeviceFault::StrictCellViolation)); // erase blank
        tape.write(1).unwrap();
        assert_eq!(tape.write(1), Err(DeviceFault::StrictCellViolation)); // mark marked
        tape.write(0).unwrap();
    }

    #[test]
    fn moves_and_reads_delegate() {
        let mut tape = StrictTape::new(InfiniteTape::new());
        tape.write(1).unwrap();
        tape.right();
        assert_eq!(tape.read(), 0);
        tape.left();
        assert_eq!(tape.read(), 1);
        assert_eq!(tape.alphabet_size(), 2);
    }

    #[test]
    fn out_of_alphabet_write_reports_inner_fault_not_strict_violation() {
        let mut tape = StrictTape::new(InfiniteTape::new());
        assert_eq!(
            tape.write(9),
            Err(DeviceFault::IndexOutsideAlphabet { index: 9 })
        );
    }
}
