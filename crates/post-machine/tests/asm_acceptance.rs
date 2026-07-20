//! Acceptance-parity sweep for the `.pma` CST front-end (docs/formats.md
//! (assembly text)): compiled `.pmc` programs re-assemble byte-identically
//! from their own emitted `-S` text, survive a disassemble-then-reassemble
//! round trip, and the assembler's rejection surface is pinned kind by
//! kind with exact spans. This is a regression net over already-landed
//! behavior — every test here is expected to pass as written; a failure
//! is a front-end regression, not a spec gap.
//!
//! A `.byte`-producing program is deliberately absent from the battery:
//! codegen (`crate::codegen::emit_program`) never emits `.byte` — it is
//! purely an assembler/disassembler-level construct (a raw byte the
//! disassembler falls back to for undecoded gaps, or hand-written `.pma`).
//! No `.pmc` source can reach it, so pinning it here would be vacuous;
//! the core asm module's inline tests (`crates/core/src/asm/`) already
//! cover `.byte` acceptance and round-tripping at the assembler level.

use mtc_core::asm::AsmErrorKind;
use mtc_post_machine::asm::{assemble, disassemble_object};
use mtc_post_machine::compiler::{CompileOptions, compile};
use mtc_post_machine::optimizer::OptLevel;
use mtc_post_machine::stdlib;

// --- The program battery -------------------------------------------------

/// No labels, no branches, no calls.
const STRAIGHT_LINE: &str = "\
main() {
    right;
    mark;
    right;
    mark;
    left;
    unmark;
}
";

/// Labels, `check`, and an explicit `goto` (a scan-to-blank loop, unlike
/// the docs/pmt/language.md sample which never needs `goto` because both its
/// `check` arms are label-local).
const LABELS_AND_BRANCHES: &str = "\
scan() {
1:  right;
    check(2, 3);
2:  goto 1;
3:  mark;
}

main() {
    @scan(!);
}
";

/// A nested (dot-mangled) subroutine call.
const SUBROUTINE_CALLS: &str = "\
outer() {
    inner() {
        right;
        mark;
    }
    @inner();
    left;
}

main() {
    @outer(!);
}
";

/// A single namespaced `use std::…` import and call.
const NAMESPACED_STD_USE: &str = "\
use std::goToEnd;

main() {
    @goToEnd(!);
}
";

/// Several stdlib routines pulled into one program.
const SEVERAL_STDLIB_ROUTINES: &str = "\
use std::goToEnd;
use std::goToBegin;
use std::appendMark;
use std::removeFirstMark;

main() {
    @goToEnd();
    @appendMark();
    @goToBegin();
    @removeFirstMark(!);
}
";

/// Composed: an own (non-std) namespace mixing a local and an exported
/// member, reached through a qualified call — exercises `::`-mangled
/// `.func` names end to end.
const NAMESPACE_QUALIFIED_CALL: &str = "\
namespace ns {
    helper() {
        right;
        mark;
    }

    export walk() {
        @helper();
        left;
    }
}

main() {
    @ns::walk(!);
}
";

/// Six inline programs plus the embedded stdlib itself
/// (`crate::stdlib::SOURCE`, the same `include_str!` the toolchain ships).
const PROGRAMS: &[(&str, &str)] = &[
    ("straight-line", STRAIGHT_LINE),
    ("labels-and-branches", LABELS_AND_BRANCHES),
    ("subroutine-calls", SUBROUTINE_CALLS),
    ("namespaced-std-use", NAMESPACED_STD_USE),
    ("several-stdlib-routines", SEVERAL_STDLIB_ROUTINES),
    ("namespace-qualified-call", NAMESPACE_QUALIFIED_CALL),
    ("embedded-stdlib", stdlib::SOURCE),
];

const OPT_LEVELS: [OptLevel; 2] = [OptLevel::O0, OptLevel::O1];

// --- Property 1: compile -> -S -> asm ≡ compile -> object ---------------

#[test]
fn emitted_pma_reassembles_byte_identically_to_the_direct_object() {
    for level in OPT_LEVELS {
        for &(name, src) in PROGRAMS {
            let options = CompileOptions {
                opt_level: level,
                ..Default::default()
            };
            let out = compile(src, options)
                .unwrap_or_else(|e| panic!("{name} at {level:?}: compile failed: {e}"));
            let reassembled = assemble(&out.pma, false).unwrap_or_else(|e| {
                panic!(
                    "{name} at {level:?}: emitted .pma failed to reassemble: {e}\n{}",
                    out.pma
                )
            });
            assert_eq!(
                reassembled.to_bytes(),
                out.object.to_bytes(),
                "{name} at {level:?}: -S reassembly diverged from the directly compiled object\n{}",
                out.pma
            );
        }
    }
}

// --- Property 2: dis -> asm round trip -----------------------------------

#[test]
fn compiled_objects_survive_a_disassemble_reassemble_round_trip() {
    // docs/formats.md (assembly text): "`pmt dis` output is always valid
    // assembler input — round-tripping through `asm` reproduces the
    // original bytes exactly." Full struct equality (not just blob
    // bytes) pins that stronger, documented invariant.
    for level in OPT_LEVELS {
        for &(name, src) in PROGRAMS {
            let options = CompileOptions {
                opt_level: level,
                ..Default::default()
            };
            let out = compile(src, options)
                .unwrap_or_else(|e| panic!("{name} at {level:?}: compile failed: {e}"));
            let text = disassemble_object(&out.object);
            let back = assemble(&text, false).unwrap_or_else(|e| {
                panic!("{name} at {level:?}: disassembly failed to reassemble: {e}\n{text}")
            });
            assert_eq!(
                back, out.object,
                "{name} at {level:?}: dis -> asm round trip diverged\n{text}"
            );
        }
    }
}

// --- Property 3: rejection pinning ---------------------------------------

#[test]
fn rejections_pin_kind_and_span_start() {
    // (case name, source, expected kind, expected span.start as (line, col))
    let cases: Vec<(&str, String, AsmErrorKind, (u32, u32))> = vec![
        (
            "dangling label at end of function",
            ".func f\nL1:\n".to_string(),
            AsmErrorKind::Syntax("label at end of function"),
            (2, 1),
        ),
        (
            // Sanctioned delta: a dotted label is no longer accepted
            // (legacy parsed it as a label; the tightened grammar rejects
            // it here instead).
            "dotted label rejected",
            ".func f\nfoo.bar:  nop\n".to_string(),
            AsmErrorKind::Syntax("label names use letters, digits, underscore"),
            (2, 1),
        ),
        (
            // Sanctioned delta: a namespaced label name is a bad label,
            // not (as legacy misparsed it) an unknown mnemonic.
            "namespaced label rejected",
            ".func f\nstd::x:  nop\n".to_string(),
            AsmErrorKind::Syntax("label names use letters, digits, underscore"),
            (2, 1),
        ),
        (
            "unknown mnemonic",
            ".func f\n        bogus\n".to_string(),
            AsmErrorKind::UnknownMnemonic("bogus".to_string()),
            (2, 9),
        ),
        (
            "code outside a function",
            "        nop\n".to_string(),
            AsmErrorKind::OutsideFunction,
            (1, 9),
        ),
        (
            "duplicate function",
            ".func f\n.func f\n        nop\n".to_string(),
            AsmErrorKind::DuplicateFunction("f".to_string()),
            (2, 7),
        ),
        (
            "duplicate label",
            ".func f\nL:      nop\nL:      nop\n".to_string(),
            AsmErrorKind::DuplicateLabel("L".to_string()),
            (3, 1),
        ),
        (
            "unknown label",
            ".func f\n        jmp NOWHERE\n".to_string(),
            AsmErrorKind::UnknownLabel("NOWHERE".to_string()),
            (2, 13),
        ),
        (
            "bad operand: jump target is a number, not a name",
            ".func f\n        jmp 5\n".to_string(),
            AsmErrorKind::BadOperand("jump/call operands are names, not numbers"),
            (2, 13),
        ),
        (
            "encode error: symbol payload exceeds 7 bits",
            ".func f\n        wr 300\n".to_string(),
            AsmErrorKind::EncodeError("symbol payload exceeds 7 bits"),
            (2, 9),
        ),
        (
            "raw line: stray angle-bracket text",
            "<goToEnd>\n".to_string(),
            AsmErrorKind::RawLine,
            (1, 1),
        ),
        (
            // Explicitly requested by the sweep: a `--listing`-shaped
            // snippet (address column, hex bytes, `<name>` target) is not
            // assembly-shaped and is rejected with the trimmed extent.
            "raw line: a --listing-shaped row",
            "  0004:  21 05 00 00 00  call    0x0005 <goToEnd>\n".to_string(),
            AsmErrorKind::RawLine,
            (1, 3),
        ),
        (
            // `A: 5` — a label followed by a bare number in the
            // instruction-word slot is not assembly-shaped (the canonical
            // `AsmErrorKind::RawLine` example).
            "raw line: label followed by a bare number",
            "A: 5\n".to_string(),
            AsmErrorKind::RawLine,
            (1, 1),
        ),
        (
            // A `jmp.s` forced past its short-offset range: pad the gap
            // past ±127 so the fixed-short form cannot reach `END`.
            "short offset out of range",
            {
                let mut src = String::from(".func f\n        jmp.s END\n");
                for _ in 0..130 {
                    src.push_str("        nop\n");
                }
                src.push_str("END:    stp\n");
                src
            },
            AsmErrorKind::ShortOffsetOutOfRange {
                target: "END".to_string(),
            },
            (2, 15),
        ),
    ];

    for (name, src, expected_kind, expected_start) in cases {
        let err = match assemble(&src, false) {
            Err(e) => e,
            Ok(_) => panic!("{name}: expected a rejection, source assembled cleanly"),
        };
        assert_eq!(err.kind, expected_kind, "{name}: kind mismatch");
        assert_eq!(
            (err.span.start.line, err.span.start.col),
            expected_start,
            "{name}: span.start mismatch"
        );
    }
}

// --- Property 4: the fused write+move mnemonics assemble and disassemble --

/// The fused `wrl`/`wrr` opcodes (docs/pmt/isa.md) reach the assembler and
/// disassembler by name through `pm1_syntax()`: a `.pma` function using
/// both assembles, and its disassembly names each mnemonic with its
/// symbol operand.
#[test]
fn wrl_wrr_round_trip_through_asm_and_dis() {
    let src = ".func f\n        wrl 1\n        wrr 0\n        stp\n";
    let object = assemble(src, false).expect("wrl/wrr assemble via pm1_syntax()");
    let listing = disassemble_object(&object);
    // Collapse the grid formatter's column padding so the mnemonic sits
    // next to its operand for a substring check.
    let flattened = listing.split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(
        flattened.contains("wrl 1"),
        "disassembly is missing `wrl 1`:\n{listing}"
    );
    assert!(
        flattened.contains("wrr 0"),
        "disassembly is missing `wrr 0`:\n{listing}"
    );
}
