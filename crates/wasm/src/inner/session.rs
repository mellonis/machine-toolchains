//! A pumped run over owned tapes. The embedder (the JS worker) drives it
//! by calling `pump`; the pause priority and budget semantics are core's
//! (`docs/core.md (async session)`) and are not restated here. The
//! JS-facing contract is docs/wasm.md (sessions).

use mtc_core::formats::tapeblock::TapeSnapshot;
use mtc_core::vm::{
    AsyncSession, AsyncTapeDevice, Machine, Outcome, PauseCause, PumpEvent, RunLimits, RunOptions,
    RunResult, RunStats, SyncAsAsync, Trap, WideTape,
};

use super::Lang;
use super::program::{Program, TapeLayout};
use super::registry::registry;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Seed {
    pub cells: Vec<u8>,
    pub head: i64,
    pub origin: i64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Limits {
    pub max_steps: Option<u64>,
    pub max_tacts: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrapInfo {
    pub kind: &'static str,
    pub at: Option<u32>,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OutcomeInfo {
    Stopped,
    Halted,
    Trapped(TrapInfo),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stats {
    pub steps: u64,
    pub core_tacts: u64,
    pub stall_tacts: u64,
    pub total_tacts: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finished {
    pub outcome: OutcomeInfo,
    pub stats: Stats,
    pub ip: u32,
    pub stack: Vec<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Cause {
    Step,
    Brk,
    Manual,
    Breakpoint(u32),
    Trap(TrapInfo),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    DeviceWait,
    BudgetSpent,
    Paused(Cause),
    Finished(Finished),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    pub band: u32,
    pub name: String,
    pub glyphs: Vec<String>,
    pub origin: i64,
    pub cells: Vec<u8>,
    pub head: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionError {
    /// `stop()` already consumed the session.
    Stopped,
    TooManySeeds {
        given: usize,
        bands: usize,
    },
    BadSeed {
        band: u32,
        index: u8,
        width: u32,
    },
    NoSuchBand(u32),
    Load(String),
}

/// The trap's kind, spelled as the equivalence harnesses spell it. Exhaustive
/// on purpose: a new `Trap` variant must be named here.
pub fn trap_kind(t: &Trap) -> &'static str {
    match t {
        Trap::InvalidOpcode { .. } => "invalid-opcode",
        Trap::CodeOutOfBounds { .. } => "code-out-of-bounds",
        Trap::BadOperand { .. } => "bad-operand",
        Trap::CallTargetNotEntry { .. } => "call-target-not-entry",
        Trap::StackOverflow => "stack-overflow",
        Trap::StackUnderflow => "stack-underflow",
        Trap::StepLimit => "step-limit",
        Trap::TactLimit => "tact-limit",
        Trap::Device { .. } => "device",
        Trap::NoTransition { .. } => "no-transition",
        Trap::TableOutOfBounds { .. } => "table-out-of-bounds",
        Trap::DispatchOutOfRange { .. } => "dispatch-out-of-range",
        Trap::UnmappedRead { .. } => "unmapped-read",
        Trap::UnmappedWrite { .. } => "unmapped-write",
        Trap::ExitOutOfRange { .. } => "exit-out-of-range",
        Trap::ProfileViolation { .. } => "profile-violation",
    }
}

fn trap_info(t: &Trap) -> TrapInfo {
    let at = match t {
        Trap::InvalidOpcode { at, .. }
        | Trap::CodeOutOfBounds { at }
        | Trap::BadOperand { at }
        | Trap::NoTransition { at }
        | Trap::TableOutOfBounds { at }
        | Trap::DispatchOutOfRange { at }
        | Trap::UnmappedRead { at }
        | Trap::UnmappedWrite { at }
        | Trap::ExitOutOfRange { at }
        | Trap::ProfileViolation { at } => Some(*at),
        Trap::CallTargetNotEntry { target } => Some(*target),
        Trap::StackOverflow
        | Trap::StackUnderflow
        | Trap::StepLimit
        | Trap::TactLimit
        | Trap::Device { .. } => None,
    };
    TrapInfo {
        kind: trap_kind(t),
        at,
        detail: t.to_string(),
    }
}

fn stats(s: RunStats) -> Stats {
    Stats {
        steps: s.steps,
        core_tacts: s.core_tacts,
        stall_tacts: s.stall_tacts,
        total_tacts: s.total_tacts(),
    }
}

fn finished(r: &RunResult) -> Finished {
    Finished {
        outcome: match &r.outcome {
            Outcome::Stopped => OutcomeInfo::Stopped,
            Outcome::Halted => OutcomeInfo::Halted,
            Outcome::Trapped(t) => OutcomeInfo::Trapped(trap_info(t)),
        },
        stats: stats(r.stats),
        ip: r.ip,
        stack: r.stack.clone(),
    }
}

fn cause(c: PauseCause) -> Cause {
    match c {
        PauseCause::Step => Cause::Step,
        PauseCause::Brk => Cause::Brk,
        PauseCause::Manual => Cause::Manual,
        PauseCause::Breakpoint(a) => Cause::Breakpoint(a),
        PauseCause::Trap(t) => Cause::Trap(trap_info(&t)),
    }
}

/// A TM-1 image's declared tape count must be `1..=16` before it ever
/// reaches the machine: `Tm1::new` (and the core's device wiring) treat an
/// out-of-range count as a caller bug and panic on it, but a `tape_count`
/// this guard rejects can only get into an `Executable` through a
/// hand-crafted or corrupted image, never through the compiler/linker — so
/// refusing it here, before `Machine::from_executable` is ever called,
/// turns that corruption into an ordinary `SessionError::Load` instead of
/// a panic. The compiler/linker never emits a multi-tape PM-1 image, so
/// the guard is a no-op for `Lang::Pmc`.
pub fn check_tape_count(lang: Lang, tape_count: u8) -> Result<(), SessionError> {
    match lang {
        Lang::Pmc => Ok(()),
        Lang::Tmc => {
            if (1..=16).contains(&tape_count) {
                Ok(())
            } else {
                Err(SessionError::Load(format!(
                    "tape count {tape_count} is out of range 1..=16"
                )))
            }
        }
    }
}

/// The device slot. One variant today; a JS-implemented `AsyncTapeDevice`
/// on one band is a later variant, not a redesign.
enum Device {
    Owned(SyncAsAsync<WideTape>),
}

impl Device {
    fn as_async(&mut self) -> &mut dyn AsyncTapeDevice {
        match self {
            Device::Owned(d) => d,
        }
    }

    fn snapshot(&self) -> TapeSnapshot {
        match self {
            Device::Owned(d) => d.get_ref().to_snapshot(),
        }
    }
}

pub struct Session {
    inner: Option<AsyncSession<'static>>,
    devices: Vec<Device>,
    layouts: Vec<TapeLayout>,
}

impl Session {
    pub fn new(program: &Program, seeds: &[Seed], limits: Limits) -> Result<Session, SessionError> {
        let exe = &program.exe;
        check_tape_count(program.lang, exe.tape_count)?;
        let bands = exe.tape_count.max(1) as usize;
        if seeds.len() > bands {
            return Err(SessionError::TooManySeeds {
                given: seeds.len(),
                bands,
            });
        }
        // PM-1 images carry no cardinalities: one binary band.
        let widths: Vec<u32> = if exe.alphabet_cardinalities.is_empty() {
            vec![2; bands]
        } else {
            exe.alphabet_cardinalities.clone()
        };
        let mut devices = Vec::with_capacity(bands);
        for band in 0..bands {
            let width = widths.get(band).copied().unwrap_or(2);
            let tape = match seeds.get(band) {
                None => WideTape::new(width),
                Some(seed) => {
                    if let Some(&bad) = seed.cells.iter().find(|&&c| u32::from(c) >= width) {
                        return Err(SessionError::BadSeed {
                            band: band as u32,
                            index: bad,
                            width,
                        });
                    }
                    let snap = TapeSnapshot {
                        origin: seed.origin,
                        cells: seed.cells.clone(),
                        head: seed.head,
                        alphabet: None,
                    };
                    WideTape::from_snapshot(&snap, width)
                        .map_err(|e| SessionError::Load(format!("band {band}: {e:?}")))?
                }
            };
            devices.push(Device::Owned(SyncAsAsync::new(tape)));
        }
        let registry = registry();
        let machine = Machine::from_executable(exe, registry)
            .map_err(|e| SessionError::Load(format!("{e:?}")))?;
        let opts = RunOptions {
            limits: RunLimits {
                max_steps: limits.max_steps,
                max_tacts: limits.max_tacts,
            },
            ..Default::default()
        };
        // PM-1 latches the initial mark through device 0 on the first pump;
        // TM-1 never latches and carries its table ROM.
        let inner = match program.lang {
            Lang::Pmc => machine.async_session(opts),
            Lang::Tmc => machine.async_session_tapes(opts),
        };
        let layouts = program.tapes().to_vec();
        Ok(Session {
            inner: Some(inner),
            devices,
            layouts,
        })
    }

    fn live(&self) -> Result<&AsyncSession<'static>, SessionError> {
        self.inner.as_ref().ok_or(SessionError::Stopped)
    }

    fn live_mut(&mut self) -> Result<&mut AsyncSession<'static>, SessionError> {
        self.inner.as_mut().ok_or(SessionError::Stopped)
    }

    pub fn bands(&self) -> usize {
        self.devices.len()
    }

    pub fn pump(&mut self, budget: Option<u64>) -> Result<Event, SessionError> {
        let session = self.inner.as_mut().ok_or(SessionError::Stopped)?;
        let mut refs: Vec<&mut dyn AsyncTapeDevice> =
            self.devices.iter_mut().map(Device::as_async).collect();
        Ok(match session.pump(&mut refs, budget) {
            PumpEvent::DeviceWait => Event::DeviceWait,
            PumpEvent::BudgetSpent => Event::BudgetSpent,
            PumpEvent::Paused(c) => Event::Paused(cause(c)),
            PumpEvent::Finished(r) => Event::Finished(finished(&r)),
        })
    }

    pub fn pause(&mut self) -> Result<(), SessionError> {
        self.live_mut()?.pause();
        Ok(())
    }

    pub fn add_breakpoint(&mut self, addr: u32) -> Result<(), SessionError> {
        self.live_mut()?.add_breakpoint(addr);
        Ok(())
    }

    pub fn remove_breakpoint(&mut self, addr: u32) -> Result<(), SessionError> {
        self.live_mut()?.remove_breakpoint(addr);
        Ok(())
    }

    pub fn snapshot(&self, band: u32) -> Result<Snapshot, SessionError> {
        self.live()?;
        let device = self
            .devices
            .get(band as usize)
            .ok_or(SessionError::NoSuchBand(band))?;
        let snap = device.snapshot();
        let layout = self.layouts.get(band as usize);
        Ok(Snapshot {
            band,
            name: layout
                .map(|l| l.name.clone())
                .unwrap_or_else(|| format!("tape{band}")),
            glyphs: layout.map(|l| l.glyphs.clone()).unwrap_or_default(),
            origin: snap.origin,
            cells: snap.cells,
            head: snap.head,
        })
    }

    pub fn snapshots(&self) -> Result<Vec<Snapshot>, SessionError> {
        (0..self.devices.len() as u32)
            .map(|b| self.snapshot(b))
            .collect()
    }

    pub fn ip(&self) -> Result<u32, SessionError> {
        Ok(self.live()?.ip())
    }

    pub fn mf(&self) -> Result<bool, SessionError> {
        Ok(self.live()?.mf())
    }

    pub fn fr(&self) -> Result<u32, SessionError> {
        Ok(self.live()?.fr())
    }

    pub fn depth(&self) -> Result<usize, SessionError> {
        Ok(self.live()?.depth())
    }

    pub fn stack(&self) -> Result<Vec<u32>, SessionError> {
        Ok(self.live()?.stack().to_vec())
    }

    pub fn stats(&self) -> Result<Stats, SessionError> {
        Ok(stats(self.live()?.stats()))
    }

    pub fn finished(&self) -> Result<Option<Finished>, SessionError> {
        Ok(self.live()?.finished().map(finished))
    }

    /// Consumes the inner session; every later call reports `Stopped`.
    pub fn stop(&mut self) -> Result<Stats, SessionError> {
        let session = self.inner.take().ok_or(SessionError::Stopped)?;
        Ok(stats(session.stop()))
    }
}
