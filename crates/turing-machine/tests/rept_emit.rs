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
use proptest::prelude::*;

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

/// A near-miss: five blocks that share a template (so the run is CONSIDERED)
/// but whose write operand — `0, 5, 2, 9, 1` — is neither affine nor modular.
/// No progression fits, so the run stays stamped and still assembles to the
/// identical object. This is the "template matches, progression fails" case,
/// distinct from `non_integer_variance_stays_stamped` (template mismatch).
const NEAR_MISS: &str = "\
.routine main, tapes=1, alpha=(20)
.func main
        jmp     L0
L0:
        wr      [0]
        jmp     done
L1:
        wr      [5]
        jmp     done
L2:
        wr      [2]
        jmp     done
L3:
        wr      [9]
        jmp     done
L4:
        wr      [1]
        jmp     done
done:
        stp
";

#[test]
fn near_miss_progression_stays_stamped() {
    let (out, report) = compress_asm(NEAR_MISS, &tm1_syntax());
    assert!(!report.fell_back);
    assert_eq!(
        report.runs_compressed, 0,
        "an unsupported progression must not fold:\n{out}"
    );
    assert_eq!(out, NEAR_MISS, "text must be unchanged");
    assert_eq!(object_bytes(NEAR_MISS), object_bytes(&out));
}

// ---------------------------------------------------------------------------
// Property test: no matter how a stamped family is shaped — affine, modular,
// constant, a near-miss that must stay stamped, or a mix — `compress_asm`'s
// output must assemble to the SAME object bytes as the stamped input. This is
// the pass's whole contract; it re-verifies the always-on self-check across
// randomized families rather than trusting detection cleverness.
// ---------------------------------------------------------------------------

/// Alphabet cardinality of the generated programs. Every emitted symbol index
/// stays below it so the fixtures always assemble.
const ALPHA: i64 = 60;

/// One stamped family's write-operand values (each `< ALPHA`).
#[derive(Debug, Clone)]
enum Family {
    /// `first + i`.
    Affine { first: i64, len: usize },
    /// `(first + i) % n`.
    Modular { first: i64, n: i64, len: usize },
    /// A single repeated value.
    Constant { value: i64, len: usize },
    /// Arbitrary values — usually not a supported progression, so it stays
    /// stamped; the self-check must still hold.
    NearMiss { values: Vec<i64> },
}

impl Family {
    fn values(&self) -> Vec<i64> {
        match self {
            Family::Affine { first, len } => (0..*len).map(|i| first + i as i64).collect(),
            Family::Modular { first, n, len } => {
                (0..*len).map(|i| (first + i as i64) % n).collect()
            }
            Family::Constant { value, len } => vec![*value; *len],
            Family::NearMiss { values } => values.clone(),
        }
    }
}

fn family_strategy() -> impl Strategy<Value = Family> {
    prop_oneof![
        (0i64..20, 1usize..8).prop_map(|(first, len)| Family::Affine { first, len }),
        (0i64..30, 2i64..30, 1usize..8).prop_map(|(first, n, len)| Family::Modular {
            first: first % n,
            n,
            len,
        }),
        (0i64..ALPHA, 1usize..8).prop_map(|(value, len)| Family::Constant { value, len }),
        prop::collection::vec(0i64..ALPHA, 1..8).prop_map(|values| Family::NearMiss { values }),
    ]
}

/// Render a list of families into an assemblable `.tma` program: each family
/// is a run of labeled blocks with a per-family letter prefix and a SINGLE
/// trailing decimal member index (`La0`, `La1`, … / `Lb0`, …), wrapped in a
/// minimal `main` that falls through to `done: stp`. The trailing-only decimal
/// is what makes the label a foldable hole — an interior digit would make
/// `tokenize_holes` bail and quietly turn the whole property vacuous.
fn program(families: &[Family]) -> String {
    let mut s =
        format!(".routine main, tapes=1, alpha=({ALPHA})\n.func main\n        jmp     done\n");
    for (f_idx, fam) in families.iter().enumerate() {
        let letter = char::from(b'a' + f_idx as u8);
        for (m_idx, value) in fam.values().iter().enumerate() {
            s.push_str(&format!(
                "L{letter}{m_idx}:\n        wr      [{value}]\n        jmp     done\n"
            ));
        }
    }
    s.push_str("done:\n        stp\n");
    s
}

/// The core property: for any batch of stamped families — affine, modular,
/// constant, near-miss, or a mix — `compress_asm`'s output assembles to the
/// SAME object bytes as the stamped input. Driven by a deterministic
/// `TestRunner` so the fold-fire count is reproducible, and it asserts that a
/// substantial fraction of the cases actually FOLD a run: if a generator
/// regression stopped folds from firing, the pass would silently test only
/// split/join identity, so that failure mode is made loud here.
#[test]
fn compressed_output_always_assembles_identically() {
    use proptest::strategy::ValueTree;
    use proptest::test_runner::TestRunner;

    const CASES: usize = 400;
    let mut runner = TestRunner::deterministic();
    let strategy = prop::collection::vec(family_strategy(), 1..4);
    let mut folded = 0usize;
    for _ in 0..CASES {
        let families = strategy.new_tree(&mut runner).unwrap().current();
        let src = program(&families);
        let stamped = object_bytes(&src);
        let (out, report) = compress_asm(&src, &tm1_syntax());
        assert_eq!(
            stamped,
            object_bytes(&out),
            "object bytes diverged (fell_back={}, runs={})\n---\n{out}",
            report.fell_back,
            report.runs_compressed,
        );
        if report.runs_compressed > 0 {
            folded += 1;
        }
    }
    // Non-vacuity guard: the generator MUST produce foldable families often.
    assert!(
        folded >= CASES / 4,
        "property near-vacuous: only {folded}/{CASES} cases folded a run"
    );
    eprintln!("fold-fire: {folded}/{CASES} generated programs folded >= 1 run");
}
