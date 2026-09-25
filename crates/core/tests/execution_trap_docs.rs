//! Drift guard for `docs/core.md (execution)`: the page publishes the
//! processor's full trap taxonomy — one table row per `Trap` variant,
//! which readers and architecture pages alike treat as the inventory —
//! and a published inventory without a check rots silently. This file
//! set-compares the page's table against the enum itself, in both
//! directions, so a variant added or renamed in source fails here until
//! the page follows, and a row left behind for a deleted variant fails
//! the same way.
//!
//! `Trap` publishes no `CODES`-style const, and Rust cannot enumerate an
//! enum's variants without one, so the registry side is the exhaustive
//! `match` in [`published_name`] plus the [`EVERY_VARIANT`] sample list
//! it is read over. The match is what makes a new variant a compile
//! break of this guard rather than a silent pass; the set-compare is
//! what then makes the page's row mandatory, since a sample missing from
//! the list leaves the page's row unmatched in the page→enum direction.

use mtc_core::vm::{DeviceFault, Trap};

fn doc() -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/core.md");
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()))
}

/// The lines of the section opened by `heading`, up to the next heading
/// of any level.
fn section<'a>(doc: &'a str, heading: &str) -> Vec<&'a str> {
    let mut lines = doc.lines();
    for line in lines.by_ref() {
        if line.trim_end() == heading {
            break;
        }
    }
    lines.take_while(|l| !l.starts_with('#')).collect()
}

/// The first `|`-delimited cell of every table row, stripped of its
/// backticks — the trap name the page publishes.
fn table_rows(lines: &[&str]) -> Vec<String> {
    lines
        .iter()
        .filter(|l| l.starts_with("| `"))
        .map(|l| {
            l.split('|')
                .nth(1)
                .expect("row has a name cell")
                .trim()
                .trim_matches('`')
                .to_string()
        })
        .collect()
}

/// The name the page publishes for a trap. Exhaustive on purpose: a new
/// `Trap` variant does not compile until it is named here AND added to
/// [`EVERY_VARIANT`], and the page's table must then carry its row.
fn published_name(t: &Trap) -> &'static str {
    match t {
        Trap::InvalidOpcode { .. } => "InvalidOpcode",
        Trap::CodeOutOfBounds { .. } => "CodeOutOfBounds",
        Trap::BadOperand { .. } => "BadOperand",
        Trap::CallTargetNotEntry { .. } => "CallTargetNotEntry",
        Trap::StackOverflow => "StackOverflow",
        Trap::StackUnderflow => "StackUnderflow",
        Trap::StepLimit => "StepLimit",
        Trap::TactLimit => "TactLimit",
        Trap::Device { .. } => "Device",
        Trap::NoTransition { .. } => "NoTransition",
        Trap::TableOutOfBounds { .. } => "TableOutOfBounds",
        Trap::DispatchOutOfRange { .. } => "DispatchOutOfRange",
        Trap::UnmappedRead { .. } => "UnmappedRead",
        Trap::UnmappedWrite { .. } => "UnmappedWrite",
        Trap::ExitOutOfRange { .. } => "ExitOutOfRange",
        Trap::ProfileViolation { .. } => "ProfileViolation",
        Trap::Contract { .. } => "Contract",
    }
}

/// One value per `Trap` variant. The payloads are arbitrary — only the
/// variant matters — and the list is read through [`published_name`], so
/// a variant listed here without an arm there does not compile.
const EVERY_VARIANT: &[Trap] = &[
    Trap::InvalidOpcode { opcode: 0, at: 0 },
    Trap::CodeOutOfBounds { at: 0 },
    Trap::BadOperand { at: 0 },
    Trap::CallTargetNotEntry { target: 0 },
    Trap::StackOverflow,
    Trap::StackUnderflow,
    Trap::StepLimit,
    Trap::TactLimit,
    Trap::Device {
        fault: DeviceFault::StrictCellViolation,
    },
    Trap::NoTransition { at: 0 },
    Trap::TableOutOfBounds { at: 0 },
    Trap::DispatchOutOfRange { at: 0 },
    Trap::UnmappedRead { at: 0 },
    Trap::UnmappedWrite { at: 0 },
    Trap::ExitOutOfRange { at: 0 },
    Trap::ProfileViolation { at: 0 },
    Trap::Contract { at: 0 },
];

/// The published taxonomy lists exactly the enum's variants — no row
/// without a variant, no variant without a row.
///
/// Mutation it catches: delete a row from the page's "Trap causes"
/// table (or add one naming a trap that does not exist) and the
/// set-compare fails; both directions were silent before this guard
/// existed.
#[test]
fn the_published_taxonomy_lists_exactly_the_trap_variants() {
    let doc = doc();
    let mut published = table_rows(&section(&doc, "## Execution"));
    assert!(
        !published.is_empty(),
        "the trap-causes table should have rows"
    );
    published.sort();
    let mut variants: Vec<String> = EVERY_VARIANT
        .iter()
        .map(|t| published_name(t).to_string())
        .collect();
    variants.sort();
    assert_eq!(
        published, variants,
        "docs/core.md (execution) and the `Trap` enum disagree — update \
         the page's table to list exactly the enum's variants"
    );
}
