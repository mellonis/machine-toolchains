//! Property + adversarial coverage for the arch-generic table-assembly
//! framework (docs/formats.md (assembly text)). The normative match-table
//! reader is the VM walk (`vm/table.rs`); here the encoding is derived
//! independently in the test and byte-compared against what the assembler
//! emits. Everything runs through a neutral fake dialect (caps all on) so
//! core stays provably arch-agnostic — no PM-1 knowledge is imported.

use mtc_core::asm::{ArchSyntax, AsmCaps, AsmError, Flow, SyntaxEntry, assemble};
use mtc_core::formats::object::ObjectFile;
use mtc_core::vm::OperandKind;
use proptest::prelude::*;

/// Minimal neutral table dialect. Task 4's `fake_syntax` lives in a
/// crate-private `#[cfg(test)]` module, so it cannot be imported from an
/// integration test — this re-declares the equivalent surface the brief
/// pins down: `tmatch` references a table (TableRef), plus `stp`/`nop`/
/// `ent`. Caps all on.
fn fake_syntax() -> ArchSyntax {
    use Flow::{FallThrough, Stop};
    ArchSyntax {
        entries: vec![
            SyntaxEntry {
                opcode: 0x01,
                mnemonic: "nop",
                operand: OperandKind::None,
                flow: FallThrough,
            },
            SyntaxEntry {
                opcode: 0x02,
                mnemonic: "stp",
                operand: OperandKind::None,
                flow: Stop,
            },
            SyntaxEntry {
                opcode: 0x11,
                mnemonic: "tmatch",
                operand: OperandKind::TableRef,
                flow: FallThrough,
            },
            SyntaxEntry {
                opcode: 0x0E,
                mnemonic: "ent",
                operand: OperandKind::None,
                flow: FallThrough,
            },
        ],
        relax_pairs: vec![],
        entry_opcode: 0x0E,
        break_opcode: None,
        trap_opcode: None,
        caps: AsmCaps {
            tables: true,
            rept: true,
            vectors: true,
        },
    }
}

fn asm(src: &str) -> Result<ObjectFile, AsmError> {
    assemble(&fake_syntax(), 0x7E, src, false)
}

// --------------------------------------------------------------------------
// 1a. Property: match-table encoding == independent derivation
// --------------------------------------------------------------------------

/// One element of a wildcard row.
#[derive(Clone, Debug)]
enum Elem {
    Payload(u8),
    Wild,
}

/// A well-formed match table: exact rows (sorted, deduped, strictly
/// ascending — so discipline passes) then optional wildcard rows (each a
/// genuine mix) then an optional trailing all-wildcard catch-all.
#[derive(Clone, Debug)]
struct MatchTable {
    width: usize,
    /// Each inner row has `width` payloads, all `< 0x7E`.
    exact: Vec<Vec<u8>>,
    /// Each inner row has `width` elements with ≥1 wildcard and ≥1 payload.
    wildcard: Vec<Vec<Elem>>,
    catch_all: bool,
}

/// Turn a raw `(is_wildcard, payload)` cell vector into a wildcard row that
/// is guaranteed to mix — ≥1 wildcard AND ≥1 payload. Only ever called at
/// `width >= 2`, so both fixup indices exist and are distinct.
fn build_wildcard_row(cells: &[(bool, u8)]) -> Vec<Elem> {
    let mut row: Vec<Elem> = cells
        .iter()
        .map(|&(is_wild, p)| {
            if is_wild {
                Elem::Wild
            } else {
                Elem::Payload(p)
            }
        })
        .collect();
    if !row.iter().any(|e| matches!(e, Elem::Wild)) {
        row[0] = Elem::Wild;
    }
    if !row.iter().any(|e| matches!(e, Elem::Payload(_))) {
        row[1] = Elem::Payload(0);
    }
    row
}

fn match_table() -> impl Strategy<Value = MatchTable> {
    (1usize..=4)
        .prop_flat_map(|width| {
            let exact_rows = prop::collection::vec(prop::collection::vec(0u8..0x7E, width), 1..=8);
            // A width-1 cell is either an exact payload or the all-wildcard
            // catch-all — a non-catch-all wildcard row (mix required) cannot
            // exist, so only generate them at width >= 2.
            let wildcard_rows = if width >= 2 {
                prop::collection::vec(
                    prop::collection::vec((any::<bool>(), 0u8..0x7E), width),
                    0..=2,
                )
                .boxed()
            } else {
                Just(Vec::<Vec<(bool, u8)>>::new()).boxed()
            };
            (Just(width), exact_rows, wildcard_rows, any::<bool>())
        })
        .prop_map(|(width, exact_rows, wildcard_raw, catch_all)| {
            let mut exact = exact_rows;
            exact.sort();
            exact.dedup();
            let wildcard = wildcard_raw
                .iter()
                .map(|cells| build_wildcard_row(cells))
                .collect();
            MatchTable {
                width,
                exact,
                wildcard,
                catch_all,
            }
        })
}

fn elem_str(e: &Elem) -> String {
    match e {
        Elem::Payload(p) => p.to_string(),
        Elem::Wild => "*".to_string(),
    }
}

/// Render the table into fake-dialect source. The first `.row` carries the
/// `T0:` label; the rest continue the same run.
fn render(t: &MatchTable) -> String {
    let mut rows: Vec<String> = Vec::new();
    for r in &t.exact {
        rows.push(
            r.iter()
                .map(|p| p.to_string())
                .collect::<Vec<_>>()
                .join(", "),
        );
    }
    for r in &t.wildcard {
        rows.push(r.iter().map(elem_str).collect::<Vec<_>>().join(", "));
    }
    if t.catch_all {
        rows.push(vec!["*"; t.width].join(", "));
    }

    let mut src = String::from(".section tables\n");
    for (i, row) in rows.iter().enumerate() {
        if i == 0 {
            src.push_str(&format!("T0: .row [{row}]\n"));
        } else {
            src.push_str(&format!("    .row [{row}]\n"));
        }
    }
    src.push_str(".section code\n.func main\n    tmatch T0\n    stp\n");
    src
}

/// Derive the match-table blob independently of the assembler: width u8,
/// row_count u16 LE, then one byte per row position (payload, or 0x7F for a
/// wildcard). Mirrors `vm/table.rs` / `build_tables`.
fn encode(t: &MatchTable) -> Vec<u8> {
    let total = t.exact.len() + t.wildcard.len() + usize::from(t.catch_all);
    let mut out = vec![t.width as u8];
    out.extend((total as u16).to_le_bytes());
    for r in &t.exact {
        out.extend(r.iter().copied()); // payloads are < 0x7E — direct bytes
    }
    for r in &t.wildcard {
        for e in r {
            out.push(match e {
                Elem::Payload(p) => *p,
                Elem::Wild => 0x7F,
            });
        }
    }
    if t.catch_all {
        out.extend(vec![0x7Fu8; t.width]);
    }
    out
}

proptest! {
    /// The assembled table blob byte-equals the test's independent
    /// derivation, for every well-formed match table.
    #[test]
    fn match_table_encoding_matches_independent_derivation(t in match_table()) {
        let obj = asm(&render(&t)).expect("a well-formed match table assembles");
        let blobs = obj.table_blobs.as_ref().expect("the table blob is present");
        prop_assert_eq!(blobs.len(), 1);
        prop_assert_eq!(&blobs[0], &encode(&t));
    }
}

// --------------------------------------------------------------------------
// 1b. Never-panic on adversarial directive soup
// --------------------------------------------------------------------------

fn soup_fragment() -> impl Strategy<Value = String> {
    prop_oneof![
        Just(".section tables".to_string()),
        Just(".section code".to_string()),
        Just(".row [1,2]".to_string()),
        Just(".row [1,*]".to_string()),
        Just(".targets A, B".to_string()),
        Just(".rept v, 0, 3".to_string()),
        Just(".endr".to_string()),
        Just("{v}".to_string()),
        Just("T0:".to_string()),
        Just(".func f".to_string()),
        Just("nop".to_string()),
        Just("stp".to_string()),
        Just("[".to_string()),
        Just("]".to_string()),
        Just("*".to_string()),
        "[a-z]{1,6}",
    ]
}

proptest! {
    /// Random multi-line sources from a directive/token vocabulary: the
    /// assembler must return Ok or Err, never panic.
    #[test]
    fn assemble_never_panics_on_directive_soup(
        lines in prop::collection::vec(soup_fragment(), 0..24),
    ) {
        let src = lines.join("\n");
        let _ = asm(&src); // Ok or Err — the assertion is "did not panic".
    }
}

// --------------------------------------------------------------------------
// 1c. Durable serialize round-trip across the empty-table-blob branch
// --------------------------------------------------------------------------

/// Two functions — one owns a table, one is table-free (its table blob is
/// empty) — round-trip through `to_bytes`/`from_bytes` unchanged. Exercises
/// the empty-blob serialize branch end to end.
#[test]
fn object_round_trips_with_table_owning_and_table_free_functions() {
    let src = "\
.section tables
T0: .row [1, 2]
    .row [1, *]
.section code
.func owner
    tmatch T0
    stp
.func plain
    nop
    stp
";
    let obj = asm(src).expect("assembles");
    assert_eq!(obj.blobs.len(), 2, "two functions");
    let tables = obj.table_blobs.as_ref().expect("table present");
    assert_eq!(tables.len(), 2, "one table blob per function");
    assert!(
        tables[1].is_empty(),
        "the table-free function's blob is empty"
    );

    let back = ObjectFile::from_bytes(&obj.to_bytes()).unwrap();
    assert_eq!(back, obj);
}

/// Signatures and tables together: a `.routine`-signed two-function file
/// with a match table engages BOTH flag-gated version-3 sections from
/// real assembly, and the object round-trips through
/// `to_bytes`/`from_bytes` unchanged.
#[test]
fn object_round_trips_with_signatures_and_tables_together() {
    use mtc_core::formats::object::RoutineSig;
    let src = "\
.routine owner, tapes=2, alpha=(3, 5)
.routine plain, tapes=1, alpha=(2)
.section tables
T0: .row [1, 2]
    .row [1, *]
.section code
.func owner
    tmatch T0
    stp
.func plain
    nop
    stp
";
    let obj = asm(src).expect("assembles");
    assert_eq!(
        obj.signatures,
        Some(vec![
            RoutineSig {
                arity: 2,
                cardinalities: vec![3, 5],
            },
            RoutineSig {
                arity: 1,
                cardinalities: vec![2],
            },
        ])
    );
    assert!(obj.table_blobs.is_some(), "table section present too");

    let bytes = obj.to_bytes();
    // Wire version field sits right after the 3-byte magic: version 3.
    assert_eq!(u16::from_le_bytes([bytes[3], bytes[4]]), 3);
    let back = ObjectFile::from_bytes(&bytes).unwrap();
    assert_eq!(back, obj);
}
