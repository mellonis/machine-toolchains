//! The `.rept` re-detection emitter (`crate::rept_emit`): codegen emits
//! STAMPED assembly (one labeled block or table row per value); this pass
//! rewrites arithmetic families back into `.rept` loops so the `-S` artifact
//! reads like a hand-written program. The invariant the pass lives under is
//! that it changes only how the text READS, never what it ASSEMBLES — every
//! test here assembles the stamped input AND the compressed output through
//! `tm1_syntax()` and asserts the object bytes are identical.

use mtc_turing_machine::asm::assemble;
use mtc_turing_machine::rept_emit::compress_asm;
use mtc_turing_machine::tm1_syntax;

/// Assemble `.tma` text (no debug info) and return its object bytes — the
/// same byte image the pass's own self-check compares.
fn object_bytes(text: &str) -> Vec<u8> {
    assemble(text, false)
        .unwrap_or_else(|e| panic!("fixture failed to assemble: {e}\n---\n{text}"))
        .to_bytes()
}

/// A five-block affine family: `L{i}` writing the literal `i+3`, `i = 0..4`.
/// The label's trailing decimal and the write operand both vary by `+1` per
/// block, so both collapse to a single `.rept` — `L{v}` and `{v+3}`.
const AFFINE: &str = "\
.routine main, tapes=1, alpha=(9)
.func main
        jmp     L0
L0:
        wr      [3]
        jmp     done
L1:
        wr      [4]
        jmp     done
L2:
        wr      [5]
        jmp     done
L3:
        wr      [6]
        jmp     done
L4:
        wr      [7]
        jmp     done
done:
        stp
";

#[test]
fn affine_family_compresses() {
    let (out, report) = compress_asm(AFFINE, &tm1_syntax());
    assert!(!report.fell_back, "affine family must self-check clean");
    assert_eq!(report.runs_compressed, 1, "one .rept run: {out}");
    assert!(out.contains(".rept v, 0, 4"), "header missing:\n{out}");
    assert!(out.contains("L{v}:"), "affine label expr missing:\n{out}");
    assert!(out.contains("{v+3}"), "affine operand expr missing:\n{out}");
    assert!(out.contains(".endr"), "endr missing:\n{out}");
    assert_eq!(
        object_bytes(AFFINE),
        object_bytes(&out),
        "compressed object must be byte-identical to stamped"
    );
}

/// A six-block modular family writing `(i+1) % 6` for `i = 0..5` — values
/// `1,2,3,4,5,0`. The wrap at `i = 5` forces the modular form `{(v+1)%6}`
/// (the affine form would run off the end of the ring).
const MODULAR: &str = "\
.routine main, tapes=1, alpha=(9)
.func main
        jmp     L0
L0:
        wr      [1]
        jmp     done
L1:
        wr      [2]
        jmp     done
L2:
        wr      [3]
        jmp     done
L3:
        wr      [4]
        jmp     done
L4:
        wr      [5]
        jmp     done
L5:
        wr      [0]
        jmp     done
done:
        stp
";

#[test]
fn modular_family_compresses() {
    let (out, report) = compress_asm(MODULAR, &tm1_syntax());
    assert!(!report.fell_back, "modular family must self-check clean");
    assert_eq!(report.runs_compressed, 1, "one .rept run:\n{out}");
    assert!(out.contains(".rept v, 0, 5"), "header missing:\n{out}");
    assert!(
        out.contains("{(v+1)%6}"),
        "modular operand expr missing:\n{out}"
    );
    assert_eq!(object_bytes(MODULAR), object_bytes(&out));
}

/// Three blocks — one short of the four-member minimum — so the run stays
/// stamped verbatim.
const SUB_FOUR: &str = "\
.routine main, tapes=1, alpha=(9)
.func main
        jmp     L0
L0:
        wr      [3]
        jmp     done
L1:
        wr      [4]
        jmp     done
L2:
        wr      [5]
        jmp     done
done:
        stp
";

#[test]
fn sub_four_run_stays_stamped() {
    let (out, report) = compress_asm(SUB_FOUR, &tm1_syntax());
    assert!(!report.fell_back);
    assert_eq!(report.runs_compressed, 0, "sub-four must not compress");
    assert_eq!(out, SUB_FOUR, "text must be unchanged");
}

/// Nine blocks with a differently-shaped block (`Lm`, a `nop` body) breaking
/// the run in the middle: `L0..L3` are one affine family, `L5..L8` another,
/// with `Lm` stamped between them. Two `.rept` runs; the whole-text object
/// stays byte-identical.
const GAP: &str = "\
.routine main, tapes=1, alpha=(20)
.func main
        jmp     L0
L0:
        wr      [3]
        jmp     done
L1:
        wr      [4]
        jmp     done
L2:
        wr      [5]
        jmp     done
L3:
        wr      [6]
        jmp     done
Lm:
        nop
        jmp     done
L5:
        wr      [10]
        jmp     done
L6:
        wr      [11]
        jmp     done
L7:
        wr      [12]
        jmp     done
L8:
        wr      [13]
        jmp     done
done:
        stp
";

#[test]
fn gap_splits_run() {
    let (out, report) = compress_asm(GAP, &tm1_syntax());
    assert!(
        !report.fell_back,
        "gap family must self-check clean:\n{out}"
    );
    assert_eq!(
        report.runs_compressed, 2,
        "two runs split by the gap:\n{out}"
    );
    assert!(
        out.contains("Lm:"),
        "the stamped gap block must survive:\n{out}"
    );
    assert!(
        out.contains("nop"),
        "the gap block body must survive:\n{out}"
    );
    assert_eq!(object_bytes(GAP), object_bytes(&out));
}

/// Five parallel blocks whose only per-block difference is a NON-integer
/// token (the jump target `alpha`/`beta`/…). A varying non-integer cannot be
/// parameterized, so no four consecutive blocks share a template and the run
/// stays stamped.
const NON_INTEGER: &str = "\
.routine main, tapes=1, alpha=(9)
.func main
        jmp     L0
L0:
        wr      [3]
        jmp     alpha
L1:
        wr      [4]
        jmp     beta
L2:
        wr      [5]
        jmp     gamma
L3:
        wr      [6]
        jmp     delta
L4:
        wr      [7]
        jmp     epsilon
alpha:
        stp
beta:
        stp
gamma:
        stp
delta:
        stp
epsilon:
        stp
";

#[test]
fn non_integer_variance_stays_stamped() {
    let (out, report) = compress_asm(NON_INTEGER, &tm1_syntax());
    assert!(!report.fell_back);
    assert_eq!(
        report.runs_compressed, 0,
        "non-integer variance must not compress"
    );
    assert_eq!(out, NON_INTEGER, "text must be unchanged");
}

/// A match table built as one labeled `.row` head plus eight unlabeled
/// continuation rows, one varying index per row (`emit_table`'s exact shape).
/// The same-label `.rept` rewrite emits `T0:` every iteration, which the
/// assembler continues into ONE wide table — byte-identical to the stamped
/// labeled-head-plus-continuations form.
const TABLE_ROWS: &str = "\
.routine main, tapes=1, alpha=(10)
.section tables
T0:     .row    [0]
        .row    [1]
        .row    [2]
        .row    [3]
        .row    [4]
        .row    [5]
        .row    [6]
        .row    [7]
        .row    [8]
D0:     .targets a, b, c, d, e, f, g, h, k
.section code
.func main
        rd
        mtc     T0
        djmp    D0
a:
        stp
b:
        stp
c:
        stp
d:
        stp
e:
        stp
f:
        stp
g:
        stp
h:
        stp
k:
        stp
";

#[test]
fn table_rows_compress() {
    let (out, report) = compress_asm(TABLE_ROWS, &tm1_syntax());
    assert!(!report.fell_back, "table run must self-check clean:\n{out}");
    assert_eq!(report.runs_compressed, 1, "one table-row run:\n{out}");
    assert!(out.contains(".rept v, 0, 8"), "header missing:\n{out}");
    assert!(
        out.contains("T0:"),
        "the same label must ride every iteration:\n{out}"
    );
    assert!(out.contains("[{v}]"), "row index expr missing:\n{out}");
    assert_eq!(object_bytes(TABLE_ROWS), object_bytes(&out));
}
