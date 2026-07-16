//! `tmt run`: execute a `.tmx` on a multi-tape `.tmt` block; the sync front
//! of the VM. Mirrors `pmt run`, but drives the whole tape band through the
//! v2 multi-tape entry points (`run_tapes` / `debug_tapes` / `step_in_tapes`)
//! with a fresh `Tm1` arch sized to the image's tape count.

use std::fmt::Write as _;
use std::fs;
use std::path::Path;

use mtc_core::formats::PROFILE_FRAMES;
use mtc_core::formats::executable::Executable;
use mtc_core::formats::tapeblock::TapeBlockFile;
use mtc_core::linker::MapFile;
use mtc_core::vm::{
    ArchRegistry, DebugEvent, Machine, Outcome, PauseCause, RunLimits, RunOptions, RunStats, Tape,
    WideTape,
};

use crate::arch::Tm1;

use super::{Args, CliOutput, render_tape};

const RUN_USAGE: &str = "\
USAGE: tmt run APP.tmx --tape TAPES.tmt [FLAGS]

TAPE:
  --tape TAPES.tmt    load the initial tape band from an MT snapshot
                      (one band per image tape; alphabets sized per band)

LIMITS:
  --max-steps N       step budget (default 10000000)
  --no-step-limit     remove the step budget
  --max-tacts N       tact budget

OUTPUT:
  --trace             stream per-instruction listing lines to stderr, live,
                      each with post-state `; MF=<0|1> heads=[..]`
                      (a frames-profile image also appends ` FR=<n>`)
  -v                  no extra effect yet (stats always print)

EXIT CODE: 0 stopped | 2 halted (hlt) | 3 trapped | 1 tool error.
";

const DEFAULT_MAX_STEPS: u64 = 10_000_000;

pub(super) fn run(raw: &[String], trace_out: &mut dyn std::io::Write) -> Result<CliOutput, String> {
    let mut args = Args::new(raw);
    if args.flag("--help") {
        return Ok(CliOutput::ok(RUN_USAGE.into(), String::new()));
    }
    let trace = args.flag("--trace");
    // -v is accepted and currently a no-op (stats always print) — it must
    // be consumed or positionals() rejects it.
    let _verbose = args.flag("-v");
    let no_step_limit = args.flag("--no-step-limit");
    let max_steps = match args.value("--max-steps")? {
        Some(text) => Some(
            text.parse::<u64>()
                .map_err(|_| format!("bad --max-steps `{text}`"))?,
        ),
        None => None,
    };
    let max_tacts = match args.value("--max-tacts")? {
        Some(text) => Some(
            text.parse::<u64>()
                .map_err(|_| format!("bad --max-tacts `{text}`"))?,
        ),
        None => None,
    };
    let tape_path = args.value("--tape")?;
    let inputs = args.positionals()?;
    let [exe_path] = inputs.as_slice() else {
        return Err(format!("run takes exactly one executable\n\n{RUN_USAGE}"));
    };
    let exe_path = Path::new(exe_path);

    let bytes =
        fs::read(exe_path).map_err(|e| format!("cannot read {}: {e}", exe_path.display()))?;
    let exe = Executable::from_bytes(&bytes).map_err(|e| format!("{}: {e}", exe_path.display()))?;

    let Some(tape_path) = tape_path else {
        return Err(format!("run needs --tape TAPES.tmt\n\n{RUN_USAGE}"));
    };
    let tape_bytes = fs::read(&tape_path).map_err(|e| format!("cannot read {tape_path}: {e}"))?;
    let block = TapeBlockFile::from_bytes(&tape_bytes).map_err(|e| format!("{tape_path}: {e}"))?;

    // The band count must equal the image's tape count. This is a tool
    // error (exit 1), distinct from a runtime trap (exit 3); the message
    // names both numbers.
    let expected = usize::from(exe.tape_count);
    if block.tapes.len() != expected {
        return Err(format!(
            "{tape_path} has {} tape(s), but {} expects {expected}",
            block.tapes.len(),
            exe_path.display(),
        ));
    }

    // Each band renders/round-trips through its own effective alphabet: its
    // override if present, else the block fallback.
    let alphabets: Vec<Vec<String>> = block
        .tapes
        .iter()
        .map(|t| t.alphabet.clone().unwrap_or_else(|| block.alphabet.clone()))
        .collect();
    // A `WideTape` per band, sized to that band's effective-alphabet length —
    // a binary tape is just width 2. `InfiniteTape` is physically two-symbol,
    // so TM-1's wide alphabets need the general device (docs/isa.md (the tape
    // and device bus)).
    let mut tapes: Vec<WideTape> = block
        .tapes
        .iter()
        .enumerate()
        .map(|(i, snap)| {
            let width = u32::try_from(alphabets[i].len()).expect("alphabet width fits u32");
            WideTape::from_snapshot(snap, width)
                .map_err(|e| format!("{tape_path}: tape {i}: {e:?}"))
        })
        .collect::<Result<_, _>>()?;

    let limits = RunLimits {
        max_steps: if no_step_limit {
            None
        } else {
            Some(max_steps.unwrap_or(DEFAULT_MAX_STEPS))
        },
        max_tacts,
    };
    let options = RunOptions {
        limits,
        ..Default::default()
    };

    let mut registry = ArchRegistry::new();
    registry.register(Box::new(Tm1::new(exe.tape_count)));
    let machine = Machine::from_executable(&exe, &registry).map_err(|e| e.to_string())?;

    let map = super::inspect::sidecar_map(exe_path); // sidecar discovery, shared with `dis`

    let mut devices: Vec<&mut dyn Tape> = tapes.iter_mut().map(|t| t as &mut dyn Tape).collect();
    let (outcome, stats) = if trace {
        drive_traced(
            &machine,
            &exe,
            &mut devices,
            options,
            trace_out,
            map.as_ref(),
        )
    } else {
        let result = machine
            .run_tapes(&mut devices, options)
            .map_err(|e| e.to_string())?;
        (result.outcome, result.stats)
    };
    drop(devices);

    let mut stdout = String::new();
    let _ = writeln!(stdout, "outcome: {outcome:?}");
    let _ = writeln!(
        stdout,
        "steps {}, core tacts {}, stall tacts {} (total {})",
        stats.steps,
        stats.core_tacts,
        stats.stall_tacts,
        stats.total_tacts()
    );
    for (i, tape) in tapes.iter().enumerate() {
        let _ = write!(
            stdout,
            "tape {i}: {}",
            render_tape(&tape.to_snapshot(), &alphabets[i])
        );
    }

    let code = match outcome {
        Outcome::Stopped => 0,
        Outcome::Halted => 2,
        Outcome::Trapped(_) => 3,
    };
    Ok(CliOutput {
        stdout,
        stderr: String::new(), // trace streams straight to trace_out, not buffered here
        code,
    })
}

/// Traced multi-tape run: DebugSession stepping with one listing line per
/// executed instruction streamed live to the writer. The line is written
/// after its instruction retires so `MF`/heads reflect that instruction's
/// effect. Mirrors `pmt run --trace`, but drives the tape band and renders
/// every head.
fn drive_traced(
    machine: &Machine,
    exe: &Executable,
    devices: &mut [&mut dyn Tape],
    options: RunOptions,
    trace_out: &mut dyn std::io::Write,
    map: Option<&MapFile>,
) -> (Outcome, RunStats) {
    let syntax = crate::asm::tm1_syntax();
    let resolve = |target: u32| -> Option<String> {
        let m = map?;
        m.functions.iter().find_map(|f| {
            if f.start == target {
                return Some(f.name.clone());
            }
            f.labels
                .iter()
                .find(|(_, a)| *a == target)
                .map(|(l, _)| format!("{}.{l}", f.name))
        })
    };
    let mut session = machine.debug_tapes(options);
    loop {
        let ip = session.ip();
        let event = session.step_in_tapes(devices);
        // `ip` is the address the just-retired instruction was fetched
        // from; when a fault runs off the end of the code image, render a
        // synthetic line instead of calling `listing_line` (which requires
        // `addr < code.len()`), so a traced run reports the same trap the
        // untraced run hits without panicking.
        let line = if (ip as usize) < exe.code.len() {
            let (line, _) = mtc_core::asm::listing_line(&syntax, &exe.code, ip, &resolve);
            line
        } else {
            format!("  {ip:04x}:  <beyond code image>")
        };
        let heads: Vec<String> = devices.iter().map(|d| d.head().to_string()).collect();
        // The frames profile appends the FR register after the heads segment;
        // a base-profile image's line stays byte-identical (empty suffix).
        let fr_suffix = if exe.profile == PROFILE_FRAMES {
            format!(" FR={}", session.fr())
        } else {
            String::new()
        };
        let _ = writeln!(
            trace_out,
            "{line}  ; MF={} heads=[{}]{fr_suffix}",
            u8::from(session.mf()),
            heads.join(", ")
        );
        match event {
            // A trap pause is terminal for a non-interactive trace: the
            // faulting line was just written — looping again would print it
            // twice via the Finished repeat.
            DebugEvent::Paused(PauseCause::Trap(_)) => {
                return (
                    session.finished().expect("trap pause implies finished"),
                    session.stats(),
                );
            }
            DebugEvent::Paused(_) => {}
            DebugEvent::Finished(outcome) => return (outcome, session.stats()),
        }
    }
}
