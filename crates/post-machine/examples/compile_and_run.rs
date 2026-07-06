//! End-to-end tour of the toolchain: `.pmc` source → compile → link →
//! disassemble → run on a tape.
//!
//!     cargo run -p mtc-post-machine --example compile_and_run

use mtc_core::linker::LinkOptions;
use mtc_core::vm::{ArchRegistry, InfiniteTape, Machine, RunOptions};
use mtc_post_machine::arch::Pm1;
use mtc_post_machine::asm::{disassemble_executable, link, listing_executable};
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
    for d in &out.report.diagnostics {
        println!("warning: line {}: {}", d.span.start.line, d.message);
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
    println!("== listing (addresses) ==");
    print!(
        "{}",
        listing_executable(&linked.executable, Some(&linked.map))
    );
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
