//! Loader + facade: Executable → validated Machine → runs (spec §4.3).

use crate::formats::executable::Executable;

use super::arch::Arch;
use super::core::Core;
use super::devices::Tape;
use super::driver::{ReturnStack, RunLimits, RunResult, TactProfile, run};

#[derive(Default)]
pub struct ArchRegistry {
    archs: Vec<Box<dyn Arch>>,
}

impl ArchRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, arch: Box<dyn Arch>) {
        self.archs.push(arch);
    }

    pub fn get(&self, id: u8) -> Option<&dyn Arch> {
        self.archs
            .iter()
            .find(|a| a.arch_id() == id)
            .map(|a| a.as_ref())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoadError {
    UnknownArch(u8),
    EntryNotEntryMarker { at: u32 },
}

impl std::fmt::Display for LoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownArch(id) => write!(f, "unknown architecture {id:#04x}"),
            Self::EntryNotEntryMarker { at } => {
                write!(f, "entry point {at:#010x} is not an entry marker")
            }
        }
    }
}

impl std::error::Error for LoadError {}

#[derive(Debug, Clone, Copy)]
pub struct RunOptions {
    pub stack_depth: usize,
    pub profile: TactProfile,
    pub limits: RunLimits,
}

impl Default for RunOptions {
    fn default() -> Self {
        Self {
            stack_depth: 1024,
            profile: TactProfile::ELECTRONIC,
            limits: RunLimits::default(),
        }
    }
}

pub struct Machine<'a> {
    arch: &'a dyn Arch,
    code: Vec<u8>,
    entry: u32,
}

impl<'a> std::fmt::Debug for Machine<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Machine")
            .field("code", &self.code)
            .field("entry", &self.entry)
            .finish()
    }
}

impl<'a> Machine<'a> {
    pub fn with_arch(
        arch: &'a dyn Arch,
        code: Vec<u8>,
        entry: u32,
    ) -> Result<Machine<'a>, LoadError> {
        match code.get(entry as usize) {
            Some(&byte) if arch.is_entry_marker(byte) => Ok(Machine { arch, code, entry }),
            _ => Err(LoadError::EntryNotEntryMarker { at: entry }),
        }
    }

    pub fn from_executable(
        exe: &Executable,
        registry: &'a ArchRegistry,
    ) -> Result<Machine<'a>, LoadError> {
        let arch = registry
            .get(exe.arch)
            .ok_or(LoadError::UnknownArch(exe.arch))?;
        Machine::with_arch(arch, exe.code.clone(), exe.entry)
    }

    pub fn entry(&self) -> u32 {
        self.entry
    }

    pub fn code(&self) -> &[u8] {
        &self.code
    }

    pub fn run(&self, device: &mut dyn Tape, opts: RunOptions) -> RunResult {
        let mut core = Core::new(self.arch, self.entry);
        // Spec §4.3 step 4: latch initial MF from the device, tact-free
        // (loading, not execution). PM-1 matches against the mark index 1.
        core.set_mf(device.read() == 1);
        let mut stack = ReturnStack::new(opts.stack_depth);
        run(
            &mut core,
            &self.code,
            &mut stack,
            device,
            opts.profile,
            opts.limits,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::formats::executable::Executable;
    use crate::vm::arch::test_arch::TestArch;
    use crate::vm::devices::InfiniteTape;
    use crate::vm::driver::Outcome;

    // TestArch entry marker: 0x0E

    #[test]
    fn with_arch_rejects_bad_entry() {
        let arch = TestArch;
        let err = Machine::with_arch(&arch, vec![0x01, 0x02], 0).unwrap_err();
        assert_eq!(err, LoadError::EntryNotEntryMarker { at: 0 });
        assert!(Machine::with_arch(&arch, vec![0x0E, 0x02], 0).is_ok());
    }

    #[test]
    fn registry_resolves_arch_or_errors() {
        let mut registry = ArchRegistry::new();
        registry.register(Box::new(TestArch));
        let exe = Executable {
            arch: 0x7F,
            entry: 0,
            code: vec![0x0E, 0x02],
        };
        assert!(Machine::from_executable(&exe, &registry).is_ok());
        let alien = Executable {
            arch: 0x09,
            entry: 0,
            code: vec![0x0E, 0x02],
        };
        assert_eq!(
            Machine::from_executable(&alien, &registry).unwrap_err(),
            LoadError::UnknownArch(0x09)
        );
    }

    #[test]
    fn run_executes_and_reports() {
        let arch = TestArch;
        // entry, right (move+latch), stop
        let machine = Machine::with_arch(&arch, vec![0x0E, 0x06, 0x02], 0).unwrap();
        let mut tape = InfiniteTape::new();
        let result = machine.run(&mut tape, RunOptions::default());
        assert_eq!(result.outcome, Outcome::Stopped);
        assert_eq!(tape.head(), 1);
    }

    #[test]
    fn initial_mf_is_latched_from_device_tact_free() {
        let arch = TestArch;
        // jm rel32 +1 (instr_end 5, target 6): taken only if MF was latched true
        // [0]=0x0E entry? — entry must be entry marker; but we start at entry 1
        // layout: [0]=0x0E, [1..6]=jm +1, [6]=halt (skipped if taken), [7]=stop
        // Wait: taken jump target = 6+1=7? instr_end of jm at 1 is 6; off +1 → 7.
        let code = vec![0x0E, 0x09, 0x01, 0x00, 0x00, 0x00, 0x03, 0x02];
        let machine = Machine::with_arch(&arch, code, 0).unwrap();

        // Marked start cell → MF true → jump skips the halt, reaches stop.
        let mut marked = InfiniteTape::from_cells([true], 0, 0);
        let r1 = machine.run(&mut marked, RunOptions::default());
        assert_eq!(r1.outcome, Outcome::Stopped);

        // Blank start cell → MF false → falls into halt.
        let mut blank = InfiniteTape::new();
        let r2 = machine.run(&mut blank, RunOptions::default());
        assert_eq!(r2.outcome, Outcome::Halted);

        // The latch read is tact-free: identical stats except the outcome path.
        assert_eq!(r1.stats.stall_tacts, 0); // no device commands executed at all
    }

    #[test]
    fn accessors_expose_code_and_entry() {
        let arch = TestArch;
        let machine = Machine::with_arch(&arch, vec![0x02, 0x0E, 0x02], 1).unwrap();
        assert_eq!(machine.entry(), 1);
        assert_eq!(machine.code(), &[0x02, 0x0E, 0x02]);
    }
}
