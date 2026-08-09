//! `pmt fmt`'s self-canonical guarantee, PM-1 edition (docs/formats.md
//! (assembly text)): the toolchain's own two `.pma` emitters — `pmt
//! compile -S` and `pmt dis` — already produce the canonical grid, so
//! running `format_asm` over their output must be the identity. This is
//! the corpus-level partner to `crates/core/src/asm/fmt.rs`'s own
//! `self_canonical_over_disassembled_objects` unit test (which exercises
//! a core test-fixture arch, not real PM-1 mnemonics) and to
//! `tests/asm_acceptance.rs`'s reassembly/round-trip sweep — same
//! program battery, one more property checked over it.

use mtc_post_machine::asm::{disassemble_object, format_asm};
use mtc_post_machine::compiler::{CompileOptions, compile};
use mtc_post_machine::optimizer::OptLevel;
use mtc_post_machine::stdlib;

/// Labels, `check`, a subroutine call, and a stdlib `use` — enough
/// surface (own-line labels, mnemonics at every operand shape, comments)
/// to exercise the grid printer's branches over a real program, plus the
/// embedded stdlib itself for a larger, organically-written source.
const PROGRAMS: &[(&str, &str)] = &[
    (
        "labels-and-branches",
        "\
scan() {
1:  right;
    check(2, 3);
2:  goto 1;
3:  mark;
}

main() {
    @scan(!);
}
",
    ),
    (
        "subroutine-and-stdlib",
        "\
use std::goToEnd;

outer() {
    inner() {
        right;
        mark;
    }
    @inner();
    left;
}

main() {
    @outer();
    @goToEnd(!);
}
",
    ),
    ("embedded-stdlib", stdlib::SOURCE),
];

const OPT_LEVELS: [OptLevel; 2] = [OptLevel::O0, OptLevel::O1];

#[test]
fn compile_s_output_is_already_canonical() {
    for level in OPT_LEVELS {
        for &(name, src) in PROGRAMS {
            let options = CompileOptions {
                opt_level: level,
                ..Default::default()
            };
            let out = compile(src, options)
                .unwrap_or_else(|e| panic!("{name} at {level:?}: compile failed: {e}"));
            let formatted = format_asm(&out.pma).unwrap_or_else(|e| {
                panic!(
                    "{name} at {level:?}: compile -S output failed to format: {e}\n{}",
                    out.pma
                )
            });
            assert_eq!(
                formatted, out.pma,
                "{name} at {level:?}: compile -S output is not already canonical"
            );
        }
    }
}

#[test]
fn dis_output_is_already_canonical() {
    for level in OPT_LEVELS {
        for &(name, src) in PROGRAMS {
            let options = CompileOptions {
                opt_level: level,
                ..Default::default()
            };
            let out = compile(src, options)
                .unwrap_or_else(|e| panic!("{name} at {level:?}: compile failed: {e}"));
            let dis = disassemble_object(&out.object);
            let formatted = format_asm(&dis).unwrap_or_else(|e| {
                panic!("{name} at {level:?}: dis output failed to format: {e}\n{dis}")
            });
            assert_eq!(
                formatted, dis,
                "{name} at {level:?}: dis output is not already canonical"
            );
        }
    }
}

/// The `.volatile` directive prints as a column-0 structural line, like
/// `.func` — so a two-column listing is already canonical and formatting
/// it is the identity (docs/formats.md (assembly text)).
#[test]
fn the_volatile_directive_formats_at_column_zero() {
    const CANONICAL: &str = "\
.volatile
.func main
        call    helper
        stp
.func main
.volatile
        rgt
        stp
.func helper
        ret
";
    assert_eq!(
        format_asm(CANONICAL).expect("formats"),
        CANONICAL,
        "the canonical two-column grid must be a fixed point"
    );
    // And an off-grid spelling normalizes to it.
    let scrambled = CANONICAL.replace("\n.volatile\n        rgt", "\n   .volatile\n rgt");
    assert_ne!(scrambled, CANONICAL);
    assert_eq!(format_asm(&scrambled).expect("formats"), CANONICAL);
}
