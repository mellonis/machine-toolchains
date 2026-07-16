//! Loader + facade: Executable → validated Machine → runs (docs/isa.md
//! (loading)).

use crate::formats::executable::Executable;

use super::arch::Arch;
use super::core::Core;
use super::debug::DebugSession;
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

/// A run could not start: the supplied devices don't match the image's
/// tape header (docs/formats.md (executable image)).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunSetupError {
    /// The image declares `expected` tapes; `got` devices were supplied.
    DeviceCount { expected: u8, got: usize },
    /// Device `tape`'s alphabet size doesn't match the image's declared
    /// cardinality for that tape.
    AlphabetMismatch { tape: u8, expected: u32, got: u32 },
}

impl std::fmt::Display for RunSetupError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DeviceCount { expected, got } => {
                write!(f, "image expects {expected} tape device(s), got {got}")
            }
            Self::AlphabetMismatch {
                tape,
                expected,
                got,
            } => write!(
                f,
                "tape {tape} has alphabet size {got}, image expects {expected}"
            ),
        }
    }
}

impl std::error::Error for RunSetupError {}

pub struct Machine<'a> {
    arch: &'a dyn Arch,
    code: Vec<u8>,
    entry: u32,
    /// Match/dispatch table ROM (docs/formats.md (executable image)); empty
    /// for a v1 code-only image.
    tables: Vec<u8>,
    /// Tape devices the image expects; a v1 code-only image is single-tape.
    tape_count: u8,
    /// Per-tape alphabet cardinalities; empty for a v1 code-only image (no
    /// per-tape check runs then).
    alphabet_cardinalities: Vec<u32>,
}

impl<'a> std::fmt::Debug for Machine<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Machine")
            .field("code", &self.code)
            .field("entry", &self.entry)
            .field("tape_count", &self.tape_count)
            .finish()
    }
}

impl<'a> Machine<'a> {
    /// A code-only machine (v1 image shape): no table ROM, single tape, no
    /// per-tape alphabet check — mirrors `Executable::code_only`'s defaults.
    pub fn with_arch(
        arch: &'a dyn Arch,
        code: Vec<u8>,
        entry: u32,
    ) -> Result<Machine<'a>, LoadError> {
        match code.get(entry as usize) {
            Some(&byte) if arch.is_entry_marker(byte) => Ok(Machine {
                arch,
                code,
                entry,
                tables: Vec::new(),
                tape_count: 1,
                alphabet_cardinalities: Vec::new(),
            }),
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
        let mut machine = Machine::with_arch(arch, exe.code.clone(), exe.entry)?;
        // Carry the whole v2 image; a v1 code-only exe leaves these at the
        // with_arch defaults (docs/formats.md (executable image)).
        machine.tables = exe.tables.clone();
        machine.tape_count = exe.tape_count;
        machine.alphabet_cardinalities = exe.alphabet_cardinalities.clone();
        Ok(machine)
    }

    pub fn entry(&self) -> u32 {
        self.entry
    }

    pub fn code(&self) -> &[u8] {
        &self.code
    }

    /// Shared run engine (docs/formats.md (executable image)): builds the
    /// core/stack, optionally latches the initial mark, and drives the whole
    /// image (code + table ROM) against `devices`. `preload_mark` is the PM-1
    /// loading-step latch — set only on the legacy single-tape `run`.
    fn drive(
        &self,
        devices: &mut [&mut dyn Tape],
        opts: RunOptions,
        preload_mark: bool,
    ) -> RunResult {
        let mut core = Core::new(self.arch, self.entry);
        if preload_mark {
            // Loading step (docs/isa.md (loading)): latch initial MF from the
            // mark device, tact-free (loading, not execution). PM-1 matches
            // against the mark index 1.
            core.set_mf(devices[0].read() == 1);
        }
        let mut stack = ReturnStack::new(opts.stack_depth);
        run(
            &mut core,
            &self.code,
            &mut stack,
            devices,
            &self.tables,
            opts.profile,
            opts.limits,
        )
    }

    /// Legacy single-tape run (the PM-1 shape): latches the initial mark from
    /// the device (loading step), then runs. A thin wrapper over `drive`; the
    /// table ROM is empty for a v1 image, so this stays byte-identical.
    pub fn run(&self, device: &mut dyn Tape, opts: RunOptions) -> RunResult {
        self.drive(&mut [device], opts, true)
    }

    /// Multi-tape run (docs/formats.md (executable image)): validates the
    /// device set against the image's tape header, then runs against the
    /// carried table ROM. No mark preload — MR starts 0; the head symbols
    /// enter via explicit read micro-ops.
    pub fn run_tapes(
        &self,
        devices: &mut [&mut dyn Tape],
        opts: RunOptions,
    ) -> Result<RunResult, RunSetupError> {
        if devices.len() != self.tape_count as usize {
            return Err(RunSetupError::DeviceCount {
                expected: self.tape_count,
                got: devices.len(),
            });
        }
        if !self.alphabet_cardinalities.is_empty() {
            for (i, (device, &expected)) in
                devices.iter().zip(&self.alphabet_cardinalities).enumerate()
            {
                let got = device.alphabet_size();
                if got != expected {
                    return Err(RunSetupError::AlphabetMismatch {
                        tape: u8::try_from(i).expect("tape count fits u8"),
                        expected,
                        got,
                    });
                }
            }
        }
        Ok(self.drive(devices, opts, false))
    }

    /// A debug session over this machine's image (docs/isa.md
    /// (DebugSession)). The session owns its core/stack; the device
    /// arrives per call. Legacy single-tape shape: preloads the mark, no
    /// table ROM.
    pub fn debug(&self, opts: RunOptions) -> DebugSession<'a> {
        DebugSession::new(
            Core::new(self.arch, self.entry),
            self.code.clone(),
            ReturnStack::new(opts.stack_depth),
            opts.profile,
            opts.limits,
        )
    }

    /// A multi-tape debug session (docs/formats.md (executable image)):
    /// carries the table ROM and does not preload the mark, mirroring
    /// `run_tapes`. Drive it with `step_in_tapes`.
    pub fn debug_tapes(&self, opts: RunOptions) -> DebugSession<'a> {
        DebugSession::new(
            Core::new(self.arch, self.entry),
            self.code.clone(),
            ReturnStack::new(opts.stack_depth),
            opts.profile,
            opts.limits,
        )
        .with_tables(self.tables.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::formats::executable::Executable;
    use crate::vm::arch::test_arch::TestArch;
    use crate::vm::debug::{DebugEvent, PauseCause};
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
        let exe = Executable::code_only(0x7F, 0, vec![0x0E, 0x02]);
        assert!(Machine::from_executable(&exe, &registry).is_ok());
        let alien = Executable::code_only(0x09, 0, vec![0x0E, 0x02]);
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
        // layout: [0]=0x0E, [1..6]=jm +1, [6]=halt (skipped if taken), [7]=stop
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

    #[test]
    fn run_reports_faulting_ip_and_empty_stack_on_trap() {
        // entry at 2 (ent); [3]=jmp +... targets 0, where [0]=jmp with an
        // offset so far negative the target computation itself traps.
        // [0]=jmp rel8 0x80 (-128); [2]=ent (entry); [3]=jmp rel8 to 0.
        // jmp at 3: instr_end 5, off -5 -> target 0.
        let arch = TestArch;
        let code = vec![0x08, 0x80, 0x0E, 0x08, 0xFB];
        let machine = Machine::with_arch(&arch, code, 2).unwrap();
        let mut tape = InfiniteTape::new();
        let result = machine.run(&mut tape, RunOptions::default());
        assert_eq!(
            result.outcome,
            Outcome::Trapped(crate::vm::trap::Trap::CodeOutOfBounds { at: 0 })
        );
        assert_eq!(result.ip, 0); // the jmp at address 0, not the entry
        assert!(result.stack.is_empty());
    }

    #[test]
    fn run_reports_return_stack_on_trap_inside_a_call() {
        // [0]=ent (entry); [1]=call +1 -> target 7; [6]=stp (never reached);
        // [7]=ent (callee); [8]=invalid opcode -> traps with the call
        // frame still on the stack (no ret ever pops it).
        let arch = TestArch;
        let code = vec![0x0E, 0x0A, 0x01, 0x00, 0x00, 0x00, 0x02, 0x0E, 0x55];
        let machine = Machine::with_arch(&arch, code, 0).unwrap();
        let mut tape = InfiniteTape::new();
        let result = machine.run(&mut tape, RunOptions::default());
        assert!(matches!(
            result.outcome,
            Outcome::Trapped(crate::vm::trap::Trap::InvalidOpcode { opcode: 0x55, .. })
        ));
        assert_eq!(result.stack, vec![6]); // return address pushed by the call
    }

    // A two-device program driven by a match/dispatch table. Both devices
    // are read; a width-2 match table folds their head symbols into MR; a
    // dispatch jump lands on the terminating stp. The `mtc` walk reads the
    // table ROM, so this only runs Stopped when the whole v2 image (code +
    // tables) is carried through the load.
    //   [0]  0x0E  entry (Nop)
    //   [1]  0x10  read dev0->slot0, dev1->slot1
    //   [2]  0x11  mtc  @table 0        [3..7]  = Table(0)
    //   [7]  0x12  djmp @table 5        [8..12] = Table(5)
    //   [12] 0x02  stp
    // tables: match@0 (width 2, one row [1,1]); dispatch@5 (one entry -> 12)
    fn two_device_table_exe(cardinalities: Vec<u32>) -> Executable {
        let mut code = vec![0x0E, 0x10, 0x11];
        code.extend(0u32.to_le_bytes());
        code.push(0x12);
        code.extend(5u32.to_le_bytes());
        code.push(0x02); // stp at 12
        let tables = vec![2, 1, 0, 1, 1, 1, 0, 12, 0, 0, 0];
        Executable::sectioned(0x7F, 0, code, tables, 2, 0, cardinalities)
    }

    fn test_registry() -> ArchRegistry {
        let mut registry = ArchRegistry::new();
        registry.register(Box::new(TestArch));
        registry
    }

    #[test]
    fn from_executable_carries_v2_metadata() {
        let registry = test_registry();
        let exe = two_device_table_exe(vec![2, 2]);
        let machine = Machine::from_executable(&exe, &registry).unwrap();
        assert_eq!(machine.tape_count, 2);
        assert_eq!(machine.alphabet_cardinalities, vec![2, 2]);
        assert_eq!(machine.tables, exe.tables);
    }

    #[test]
    fn from_executable_defaults_v1_metadata() {
        let registry = test_registry();
        let exe = Executable::code_only(0x7F, 0, vec![0x0E, 0x02]);
        let machine = Machine::from_executable(&exe, &registry).unwrap();
        assert_eq!(machine.tape_count, 1); // format truth: code-only images are single-tape
        assert!(machine.alphabet_cardinalities.is_empty());
        assert!(machine.tables.is_empty());
    }

    #[test]
    fn run_tapes_drives_two_devices_through_a_table() {
        let registry = test_registry();
        let machine =
            Machine::from_executable(&two_device_table_exe(vec![2, 2]), &registry).unwrap();
        let mut t0 = InfiniteTape::from_cells([true], 0, 0);
        let mut t1 = InfiniteTape::from_cells([true], 0, 0);
        let mut devs: [&mut dyn Tape; 2] = [&mut t0, &mut t1];
        let r = machine.run_tapes(&mut devs, RunOptions::default()).unwrap();
        assert_eq!(r.outcome, Outcome::Stopped);
    }

    #[test]
    fn run_tapes_rejects_wrong_device_count() {
        let registry = test_registry();
        let machine =
            Machine::from_executable(&two_device_table_exe(vec![2, 2]), &registry).unwrap();
        let mut t0 = InfiniteTape::from_cells([true], 0, 0);
        let mut devs: [&mut dyn Tape; 1] = [&mut t0];
        let err = machine
            .run_tapes(&mut devs, RunOptions::default())
            .unwrap_err();
        assert_eq!(
            err,
            RunSetupError::DeviceCount {
                expected: 2,
                got: 1
            }
        );
    }

    #[test]
    fn run_tapes_rejects_alphabet_mismatch() {
        let registry = test_registry();
        // The image expects a 3-symbol second tape; both supplied tapes are
        // 2-symbol, so tape 1 mismatches.
        let machine =
            Machine::from_executable(&two_device_table_exe(vec![2, 3]), &registry).unwrap();
        let mut t0 = InfiniteTape::from_cells([true], 0, 0);
        let mut t1 = InfiniteTape::from_cells([true], 0, 0);
        let mut devs: [&mut dyn Tape; 2] = [&mut t0, &mut t1];
        let err = machine
            .run_tapes(&mut devs, RunOptions::default())
            .unwrap_err();
        assert_eq!(
            err,
            RunSetupError::AlphabetMismatch {
                tape: 1,
                expected: 3,
                got: 2
            }
        );
    }

    #[test]
    fn run_tapes_does_not_preload_mf() {
        // Same MF-latch probe as `initial_mf_is_latched_from_device_tact_free`,
        // but through run_tapes: no loading-step preload, so a marked start
        // cell does NOT make the leading `jm` taken — it falls into halt.
        let registry = test_registry();
        let code = vec![0x0E, 0x09, 0x01, 0x00, 0x00, 0x00, 0x03, 0x02];
        let exe = Executable::sectioned(0x7F, 0, code, Vec::new(), 1, 0, Vec::new());
        let machine = Machine::from_executable(&exe, &registry).unwrap();
        let mut marked = InfiniteTape::from_cells([true], 0, 0);
        let mut devs: [&mut dyn Tape; 1] = [&mut marked];
        let r = machine.run_tapes(&mut devs, RunOptions::default()).unwrap();
        assert_eq!(r.outcome, Outcome::Halted);
    }

    #[test]
    fn debug_tapes_steps_two_devices_through_a_table_with_a_breakpoint() {
        let registry = test_registry();
        let machine =
            Machine::from_executable(&two_device_table_exe(vec![2, 2]), &registry).unwrap();
        let mut session = machine.debug_tapes(RunOptions::default());
        session.add_breakpoint(12); // the stp; step_in mirrors — no pause here
        let mut t0 = InfiniteTape::from_cells([true], 0, 0);
        let mut t1 = InfiniteTape::from_cells([true], 0, 0);
        let mut devs: [&mut dyn Tape; 2] = [&mut t0, &mut t1];

        // entry, read, mtc, djmp -> stp: every step is a plain Step, even at
        // the breakpoint address (step_in_tapes does not consult breakpoints).
        for expected_ip in [1u32, 2, 7, 12] {
            assert_eq!(
                session.step_in_tapes(&mut devs),
                DebugEvent::Paused(PauseCause::Step)
            );
            assert_eq!(session.ip(), expected_ip);
        }
        // Reaching the djmp target (12) proves debug_tapes carried the table
        // ROM — an empty ROM would have trapped the mtc walk.
        assert_eq!(
            session.step_in_tapes(&mut devs),
            DebugEvent::Finished(Outcome::Stopped)
        );
    }
}
