//! Mono stamping rewrites a callee's match rows through the binding's read
//! preimage, which renames cells and so can permute rows out of the canonical
//! row order (`docs/core.md (match tables)`). A disassembly prints rows in
//! stored order, so a permuted table would disassemble to text the assembler
//! refuses with `table-discipline` — an image no longer expressible as
//! assembly, which `docs/tmt/asm.md (dis output is valid assembler input)`
//! promises it always is.
//!
//! This is the regression: the stamped table comes out canonically ordered,
//! with every dispatch entry carried alongside its row, and the whole
//! disassembly re-assembles.
//!
//! Derivation-first: the expected rows AND their targets are derived here
//! from the binding's maps, never captured from a run.

use mtc_core::formats::object::ObjectFile;
use mtc_core::linker::{CallMech, LinkOptions};
use mtc_turing_machine::asm::{assemble, disassemble_executable_with_map, link};
use mtc_turing_machine::compiler::{CompileOptions, compile};

/// A one-tape machine calls a one-tape routine under a bijective binding that
/// CROSSES two symbols: caller `'a'`(1) reads as callee `'x'`(2) and caller
/// `'b'`(2) reads as callee `'y'`(1). The callee's own rows are authored (and
/// emitted) ascending — `'_'`(0), `'y'`(1), `'x'`(2) — so the preimage rewrite
/// maps them to 0, 2, 1: out of order unless the stamp restores it.
const CROSSED_BINDING: &str = "\
alphabet outer { '_', 'a', 'b' }
alphabet inner { '_', 'y', 'x' }
routine callee(tape p: inner) {
  entry state s {
    ['x'] -> write ['y'] goto s;
    ['y'] -> write ['x'] goto s;
    ['_'] -> return;
  }
}
machine {
  tape t: outer;
  entry state go { [*] -> call callee(p = t with map { 'a' -> 'x', 'b' -> 'y' }) then fin; }
  state fin { [*] -> stop; }
}
";

fn object(src: &str) -> ObjectFile {
    compile(src, CompileOptions::default())
        .unwrap_or_else(|e| panic!("the fixture must compile: {e}"))
        .object
}

/// Link `obj` under `mech` and disassemble the image the way `tmt dis` does.
fn dis(obj: &ObjectFile, mech: CallMech) -> String {
    let out = link(
        std::slice::from_ref(obj),
        &[],
        LinkOptions {
            call_mech: mech,
            ..Default::default()
        },
    )
    .unwrap_or_else(|e| panic!("the {mech} link failed: {e}"));
    disassemble_executable_with_map(&out.executable, &out.map)
}

/// Collapse runs of whitespace so an assertion reads against the text rather
/// than against the canonical column grid.
fn squeeze(line: &str) -> String {
    line.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Every `.row` operand in the disassembly, in stored order.
fn rows(dis: &str) -> Vec<String> {
    dis.lines()
        .filter_map(|l| squeeze(l).split_once(".row ").map(|(_, op)| op.to_string()))
        .collect()
}

/// The `.targets` operand list, in MR order (MR 1 selects the first).
fn targets(dis: &str) -> Vec<String> {
    let line = dis
        .lines()
        .map(squeeze)
        .find(|l| l.contains(".targets "))
        .expect("the stamp's dispatch table disassembles as a `.targets` line");
    let (_, ops) = line.split_once(".targets ").expect("checked by `find`");
    ops.split(',').map(|t| t.trim().to_string()).collect()
}

/// The instruction the label `name` sits on.
fn instr_at(dis: &str, name: &str) -> String {
    let prefix = format!("{name}:");
    let line = dis
        .lines()
        .map(squeeze)
        .find(|l| l.starts_with(&prefix))
        .unwrap_or_else(|| panic!("dispatch target `{name}` labels no line:\n{dis}"));
    line[prefix.len()..].trim().to_string()
}

/// Mono and hybrid both stamp this site (the binding is a completed bijection,
/// which is exactly what hybrid's classifier routes to mono), so both must
/// produce the canonical order.
#[test]
fn a_stamped_match_table_keeps_the_canonical_row_order() {
    let obj = object(CROSSED_BINDING);
    for mech in [CallMech::Mono, CallMech::Hybrid] {
        let text = dis(&obj, mech);

        // Derivation. The binding's read map sends physical → virtual as
        // 0→0, 1('a')→2('x'), 2('b')→1('y'), so the preimage of each virtual
        // cell is 0→[0], 1→[2], 2→[1]. The callee's three rows are [0], [1],
        // [2]; rewriting them gives [0], [2], [1], which sorts to [0], [1],
        // [2] — three exact rows at width 1, so the canonical order is just
        // ascending.
        assert_eq!(
            rows(&text),
            vec!["[0]", "[1]", "[2]"],
            "stamped rows must be canonically ordered under {mech}:\n{text}"
        );

        // The targets must have moved with their rows, since MR is the
        // matched row's ordinal. Row [0] is physical `'_'`, which reads as
        // callee `'_'` → `return`. Row [1] is physical `'a'`, which reads as
        // `'x'` → the callee writes `'y'`, and the write map sends `'y'` back
        // to physical 2 (`'b'`) → `wrmv [2]`. Row [2] is physical `'b'`,
        // reading as `'y'` → the callee writes `'x'` → physical 1 (`'a'`).
        let t = targets(&text);
        assert_eq!(t.len(), 3, "one dispatch entry per row under {mech}");
        assert_eq!(instr_at(&text, &t[0]), "ret", "row [0] returns ({mech})");
        assert_eq!(
            instr_at(&text, &t[1]),
            "wrmv [2], [.]",
            "row [1] ('a' reads as 'x') writes 'y' → physical 2 ({mech})"
        );
        assert_eq!(
            instr_at(&text, &t[2]),
            "wrmv [1], [.]",
            "row [2] ('b' reads as 'y') writes 'x' → physical 1 ({mech})"
        );
    }
}

/// The round trip the row order exists to keep: the disassembly of a stamped
/// image is valid assembler input. Frames rides along as the control — it
/// lowers this site through a runtime descriptor instead of a stamp, so it
/// never had the defect.
#[test]
fn a_stamped_image_disassembles_to_valid_assembler_input() {
    let obj = object(CROSSED_BINDING);
    for mech in [CallMech::Mono, CallMech::Hybrid, CallMech::Frames] {
        let text = dis(&obj, mech);
        assemble(&text, false).unwrap_or_else(|e| {
            panic!("the {mech} image's disassembly must re-assemble: {e}\n{text}")
        });
    }
}
