//! `pmt run`: execute a .pmx on a tape; the sync front of the VM.

use std::fmt::Write as _;
use std::fs;
use std::path::Path;

use mtc_core::formats::executable::Executable;
use mtc_core::formats::tapeblock::TapeBlockFile;
use mtc_core::linker::MapFile;
use mtc_core::vm::{
    ArchRegistry, DebugEvent, InfiniteTape, Machine, Outcome, PauseCause, RunLimits, RunOptions,
    StrictTape, TactProfile, Tape,
};

use crate::arch::{DEFAULT_GLYPHS, Pm1};

use super::{Args, CliOutput, render_tape};

const RUN_USAGE: &str = "\
USAGE: pmt run APP.pmx [FLAGS]

TAPE (default: empty, head 0):
  --tape-block IN.pmt        load the initial tape from a snapshot
  --tape \" * *\" [--head N]   build the initial tape inline
  --save-tape-block OUT.pmt  write the final tape as a snapshot

LIMITS AND SEMANTICS:
  --max-steps N       step budget (default 10000000)
  --no-step-limit     remove the step budget
  --max-tacts N       tact budget
  --strict-cells      trap on double-mark/double-unmark
  --tact-profile M,R,W  device costs (move,read,write; default 1,1,1)

OUTPUT:
  --trace             stream per-instruction listing lines to stderr,
                      live, each with post-state `; MF=<0|1> head=<n>`
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
    let strict = args.flag("--strict-cells");
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
    let profile = match args.value("--tact-profile")? {
        Some(text) => parse_profile(&text)?,
        None => TactProfile::ELECTRONIC,
    };
    let tape_block = args.value("--tape-block")?;
    let tape_inline = args.value("--tape")?;
    let head: i64 = match args.value("--head")? {
        Some(text) => text.parse().map_err(|_| format!("bad --head `{text}`"))?,
        None => 0,
    };
    let save = args.value("--save-tape-block")?;
    let inputs = args.positionals()?;
    let [exe_path] = inputs.as_slice() else {
        return Err(format!("run takes exactly one executable\n\n{RUN_USAGE}"));
    };
    let exe_path = Path::new(exe_path);

    let bytes =
        fs::read(exe_path).map_err(|e| format!("cannot read {}: {e}", exe_path.display()))?;
    let exe = Executable::from_bytes(&bytes).map_err(|e| format!("{}: {e}", exe_path.display()))?;

    let (mut tape, alphabet) = initial_tape(tape_block.as_deref(), tape_inline.as_deref(), head)?;

    let limits = RunLimits {
        max_steps: if no_step_limit {
            None
        } else {
            Some(max_steps.unwrap_or(DEFAULT_MAX_STEPS))
        },
        max_tacts,
    };
    let options = RunOptions {
        profile,
        limits,
        ..Default::default()
    };

    let mut registry = ArchRegistry::new();
    registry.register(Box::new(Pm1));
    let machine = Machine::from_executable(&exe, &registry).map_err(|e| e.to_string())?;

    let map = super::inspect::sidecar_map(exe_path); // sidecar discovery, shared with `dis`

    let stderr = String::new(); // trace streams straight to trace_out, not buffered here
    let (outcome, stats) = if strict {
        let mut wrapped = StrictTape::new(tape);
        let r = drive(
            &machine,
            &exe,
            &mut wrapped,
            options,
            trace_to(trace, trace_out),
            map.as_ref(),
        );
        tape = wrapped.into_inner();
        r
    } else {
        drive(
            &machine,
            &exe,
            &mut tape,
            options,
            trace_to(trace, trace_out),
            map.as_ref(),
        )
    };

    let snapshot = tape.to_snapshot();
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
    stdout.push_str(&render_tape(&snapshot, &alphabet));

    if let Some(out_path) = save {
        let block = TapeBlockFile {
            alphabet: alphabet.clone(),
            tapes: vec![snapshot],
        };
        fs::write(&out_path, block.to_bytes())
            .map_err(|e| format!("cannot write {out_path}: {e}"))?;
    }

    let code = match outcome {
        Outcome::Stopped => 0,
        Outcome::Halted => 2,
        Outcome::Trapped(_) => 3,
    };
    Ok(CliOutput {
        stdout,
        stderr,
        code,
    })
}

/// `Some(w)` iff tracing is on. A free function, not a closure: a
/// closure capturing `trace: bool` and typed on `&mut dyn Write` pins a
/// single concrete lifetime for its `Fn` impl, which fails to reborrow
/// `trace_out` at two separate call sites (mutable references are
/// invariant); a plain function gets a fresh lifetime per call instead.
fn trace_to(trace: bool, w: &mut dyn std::io::Write) -> Option<&mut dyn std::io::Write> {
    trace.then_some(w)
}

fn parse_profile(text: &str) -> Result<TactProfile, String> {
    let parts: Vec<&str> = text.split(',').collect();
    let [m, r, w] = parts.as_slice() else {
        return Err(format!("bad --tact-profile `{text}` (want M,R,W)"));
    };
    let parse = |s: &str| {
        s.trim()
            .parse::<u32>()
            .map_err(|_| format!("bad cost `{s}`"))
    };
    Ok(TactProfile {
        move_cost: parse(m)?,
        read_cost: parse(r)?,
        write_cost: parse(w)?,
        // PM-1 never issues a table read or a frame load; the
        // `--tact-profile M,R,W` surface stays three-valued. Default the
        // rest to the electronic prices.
        table_read_cost: TactProfile::ELECTRONIC.table_read_cost,
        frame_load_cost: TactProfile::ELECTRONIC.frame_load_cost,
    })
}

/// Initial tape + the alphabet used for rendering/saving: a loaded block
/// brings its own glyphs (PM-1 blocks hold exactly one tape); otherwise
/// the arch defaults.
fn initial_tape(
    block: Option<&str>,
    inline: Option<&str>,
    head: i64,
) -> Result<(InfiniteTape, Vec<String>), String> {
    if block.is_some() && inline.is_some() {
        return Err("--tape-block and --tape are mutually exclusive".into());
    }
    let default_alphabet: Vec<String> = DEFAULT_GLYPHS.iter().map(|g| g.to_string()).collect();
    if let Some(path) = block {
        let bytes = fs::read(path).map_err(|e| format!("cannot read {path}: {e}"))?;
        let file = TapeBlockFile::from_bytes(&bytes).map_err(|e| format!("{path}: {e}"))?;
        let [snapshot] = file.tapes.as_slice() else {
            return Err(format!("{path}: PM-1 blocks hold exactly one tape"));
        };
        let tape = InfiniteTape::from_snapshot(snapshot).map_err(|e| format!("{path}: {e:?}"))?;
        return Ok((tape, file.alphabet));
    }
    if let Some(pattern) = inline {
        let cells: Result<Vec<bool>, String> = pattern
            .chars()
            .map(|c| match c {
                ' ' => Ok(false),
                '*' => Ok(true),
                other => Err(format!("bad cell character `{other}`")),
            })
            .collect();
        return Ok((InfiniteTape::from_cells(cells?, 0, head), default_alphabet));
    }
    Ok((InfiniteTape::new(), default_alphabet))
}

/// Plain run, or traced run: DebugSession stepping with one listing
/// line per executed instruction streamed live to the writer, in the
/// `--trace` format (`docs/pmt/cli.md`). The line is written after its
/// instruction retires so `MF`/`head` reflect that instruction's effect.
fn drive(
    machine: &Machine,
    exe: &Executable,
    tape: &mut dyn Tape,
    options: RunOptions,
    trace: Option<&mut dyn std::io::Write>,
    map: Option<&MapFile>,
) -> (Outcome, mtc_core::vm::RunStats) {
    let Some(w) = trace else {
        let result = machine.run(tape, options);
        return (result.outcome, result.stats);
    };
    let syntax = crate::asm::pm1_syntax();
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
    let mut session = machine.debug(options);
    loop {
        let ip = session.ip();
        let event = session.step_in(tape);
        // `ip` is the address the just-retired instruction was fetched
        // from; when it faults by running off the end of the code image
        // (fetch at ip == exe.code.len()), `listing_line` requires
        // `addr < code.len()` and must not be called — render a synthetic
        // line instead so a traced run reports the same trap the
        // untraced run hits, without panicking.
        let line = if (ip as usize) < exe.code.len() {
            let (line, _) = mtc_core::asm::listing_line(&syntax, &exe.code, ip, &resolve);
            line
        } else {
            format!("  {ip:04x}:  <beyond code image>")
        };
        let _ = writeln!(
            w,
            "{line}  ; MF={} head={}",
            u8::from(session.mf()),
            tape.head()
        );
        match event {
            // A trap pause is terminal for a non-interactive trace: the
            // faulting line was just written — looping again would print
            // it twice via the Finished repeat.
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
