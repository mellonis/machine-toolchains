//! End-to-end tour of everything implemented through Plan 5:
//! `.pmc` source → compile → link → disassemble → run on a tape.
//!
//!     cargo run -p mtc-post-machine --example compile_and_run

use mtc_core::formats::executable::Executable;
use mtc_core::linker::{LinkOptions, MapFile};
use mtc_core::vm::{ArchRegistry, InfiniteTape, Machine, OperandKind, RunOptions};
use mtc_post_machine::arch::Pm1;
use mtc_post_machine::asm::{disassemble_executable, link, pm1_syntax};
use mtc_post_machine::compiler::{CompileOptions, compile};

const SOURCE: &str = "\
walkToBlank() {
    1: check(2, !);
    2: right(1);
}

main() {
    10: @walkToBlank();
    20: mark;
    30: right;
    40: @walkToBlank();
    50: mark(!);
}
";

fn main() {
    // 1. Compile `.pmc` → object (plus .pma text, IR JSON, warnings).
    let out = compile(
        SOURCE,
        CompileOptions {
            debug_info: false, // -g: pmc line numbers in the map
            strip_debugger: false,
            ..Default::default()
        },
    )
    .expect("source compiles");

    println!("== generated .pma (-S output) ==\n{}", out.pma);
    for w in &out.report.warnings {
        println!("warning: line {}: {}", w.line, w.message);
    }
    println!("== lowered CFG IR (--emit-ir) ==\n{}\n", out.ir.to_json());

    // 2. Link objects → executable (+ map sidecar + report).
    let linked = link(&[out.object], &[], LinkOptions::default()).expect("links");
    println!("== link report ==");
    println!("dropped (dead functions): {:?}", linked.report.dropped);
    println!(
        "calls relaxed to short: {}, kept far: {}",
        linked.report.relaxed_calls, linked.report.far_calls
    );
    println!(
        "executable: {} bytes, entry at {}\n",
        linked.executable.code.len(),
        linked.executable.entry
    );
    println!("== .pmx.map sidecar (JSON) ==\n{}\n", linked.map.to_json());

    // 3. Disassemble the executable (recursive-descent discovery).
    println!(
        "== disassembly of the executable ==\n{}",
        disassemble_executable(&linked.executable)
    );

    // 3b. Listing form: address + raw bytes + mnemonic, debugger-style.
    // Plan 7's DebugSession makes this a library feature; here it is
    // decoded by hand from the public ArchSyntax opcode table.
    println!("== listing (addresses) ==");
    print_listing(&linked.executable, &linked.map);
    println!();

    // 4. Run it: marks at cells 0..=2, head at 0.
    let mut registry = ArchRegistry::new();
    registry.register(Box::new(Pm1));
    let machine = Machine::from_executable(&linked.executable, &registry).expect("loads");
    let mut tape = InfiniteTape::from_cells([true, true, true], 0, 0);
    // A step limit so a non-terminating program traps (Trap::StepLimit)
    // instead of hanging this example forever.
    let options = RunOptions {
        limits: mtc_core::vm::RunLimits {
            max_steps: Some(100_000),
            ..Default::default()
        },
        ..Default::default()
    };
    let result = machine.run(&mut tape, options);

    println!("== run ==");
    println!("outcome: {:?}", result.outcome);
    println!(
        "steps: {}, core tacts: {}, stall tacts: {}",
        result.stats.steps, result.stats.core_tacts, result.stats.stall_tacts
    );
    println!("final head: {}", tape.head());
    println!("marked cells: {:?}", tape.marked_cells());
}

/// Debugger-style listing: every byte accounted for, one line per
/// instruction, jump/call targets resolved to names via the map.
fn print_listing(exe: &Executable, map: &MapFile) {
    let syntax = pm1_syntax();
    // A code address's name: a function start or a `function.label`.
    let name_at = |addr: u32| -> Option<String> {
        map.functions.iter().find_map(|f| {
            if f.start == addr {
                return Some(f.name.clone());
            }
            f.labels
                .iter()
                .find(|(_, a)| *a == addr)
                .map(|(label, _)| format!("{}.{}", f.name, label))
        })
    };
    let fmt_target = |target: u32| match name_at(target) {
        Some(name) => format!("{target:#06x} <{name}>"),
        None => format!("{target:#06x}"),
    };

    let code = &exe.code;
    let mut addr = 0usize;
    while addr < code.len() {
        if let Some(f) = map.functions.iter().find(|f| f.start as usize == addr) {
            println!("{}:", f.name);
        }
        let opcode = code[addr];
        // (length in bytes, mnemonic, operand text)
        let (len, mnemonic, operand) = match syntax.by_opcode(opcode) {
            None => (1, ".byte", opcode.to_string()),
            Some(entry) => match entry.operand {
                OperandKind::None => (1, entry.mnemonic, String::new()),
                OperandKind::RelI8 if addr + 2 <= code.len() => {
                    let off = code[addr + 1] as i8;
                    let target = (addr as i64 + 2 + i64::from(off)) as u32;
                    (2, entry.mnemonic, fmt_target(target))
                }
                OperandKind::RelI32 if addr + 5 <= code.len() => {
                    let bytes: [u8; 4] = code[addr + 1..addr + 5].try_into().unwrap();
                    let off = i32::from_le_bytes(bytes);
                    let target = (addr as i64 + 5 + i64::from(off)) as u32;
                    (5, entry.mnemonic, fmt_target(target))
                }
                OperandKind::SymbolVec => {
                    // Self-delimiting: 7-bit indices, high bit marks the last.
                    let mut indices = Vec::new();
                    let mut end = addr + 1;
                    while end < code.len() {
                        let b = code[end];
                        indices.push((b & 0x7F).to_string());
                        end += 1;
                        if b & 0x80 != 0 {
                            break;
                        }
                    }
                    (end - addr, entry.mnemonic, indices.join(", "))
                }
                // Truncated operand at the end of the image.
                _ => (1, ".byte", opcode.to_string()),
            },
        };
        let bytes_hex = code[addr..addr + len]
            .iter()
            .map(|b| format!("{b:02X}"))
            .collect::<Vec<_>>()
            .join(" ");
        let line = format!("  {addr:04x}:  {bytes_hex:<15} {mnemonic:<8}{operand}");
        println!("{}", line.trim_end());
        addr += len;
    }
}
