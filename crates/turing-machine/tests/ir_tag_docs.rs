//! Drift guard for the `.tmc` state-graph IR's tag vocabulary: the IR is
//! a documented, versioned JSON artifact (docs/formats.md (IR JSON)), and
//! that page lists, in prose, every tag each tagged shape can carry — the
//! world kinds, the pattern- and write-cell kinds, the moves, the
//! transition kinds, the `call_then` resume-point kinds and the dispatch
//! hints. A published vocabulary without a check rots silently, so each
//! list here is read out of the page by its own anchored phrase and
//! set-compared in both directions against the tags the code serializes.
//!
//! The code side never spells a tag by hand: one sample per variant is
//! serialized through `serde_json`, so the types' own `rename_all`
//! attributes stay the authority. Each sample list sits beside a match
//! with no catch-all arm, so a new variant does not compile until it has
//! a sample here — and then fails the compare until the page lists it.

use std::collections::BTreeSet;

use mtc_turing_machine::ir::{
    IrCell, IrDispatch, IrMove, IrThen, IrTransition, IrWorldKind, IrWrite,
};
use serde::Serialize;

/// The `.tmc` IR section of docs/formats.md, whitespace runs collapsed to
/// one space so a list wrapped across lines reads as one phrase.
fn section() -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/formats.md");
    let doc = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
    let mut lines = doc.lines();
    for line in lines.by_ref() {
        if line.trim_end() == "### The `.tmc` state-graph IR" {
            break;
        }
    }
    let body: Vec<&str> = lines.take_while(|l| !l.starts_with('#')).collect();
    assert!(
        !body.is_empty(),
        "docs/formats.md lost its `.tmc` IR section"
    );
    body.join(" ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// The backticked tokens at parenthesis depth 0 between `start` and the
/// first `end` after it. Panics when either anchor is missing, so a
/// reworded page fails loudly instead of yielding an empty list.
fn documented(section: &str, start: &str, end: &str) -> BTreeSet<String> {
    let from = section
        .find(start)
        .unwrap_or_else(|| panic!("anchor {start:?} not found in the `.tmc` IR section"))
        + start.len();
    let to = section[from..]
        .find(end)
        .unwrap_or_else(|| panic!("end anchor {end:?} not found after {start:?}"))
        + from;
    let mut out = BTreeSet::new();
    let mut depth = 0usize;
    let mut chars = section[from..to].chars();
    while let Some(c) = chars.next() {
        match c {
            '(' => depth += 1,
            ')' => depth = depth.saturating_sub(1),
            '`' => {
                let token: String = chars.by_ref().take_while(|&t| t != '`').collect();
                if depth == 0 {
                    out.insert(token);
                }
            }
            _ => {}
        }
    }
    assert!(!out.is_empty(), "no tags between {start:?} and {end:?}");
    out
}

/// The tag a value serializes to: the `kind` field of an internally
/// tagged enum, or the bare string of a unit-only one.
fn tag<T: Serialize>(value: &T) -> String {
    match serde_json::to_value(value).expect("IR values serialize") {
        serde_json::Value::String(s) => s,
        serde_json::Value::Object(map) => map
            .get("kind")
            .and_then(|k| k.as_str())
            .expect("a tagged IR value carries a string `kind`")
            .to_string(),
        other => panic!("unexpected IR tag shape: {other}"),
    }
}

fn tags<T: Serialize>(values: &[T]) -> BTreeSet<String> {
    values.iter().map(tag).collect()
}

fn world_kinds() -> Vec<IrWorldKind> {
    let all = vec![IrWorldKind::Machine, IrWorldKind::Routine];
    for v in &all {
        match v {
            IrWorldKind::Machine | IrWorldKind::Routine => {}
        }
    }
    all
}

fn cells() -> Vec<IrCell> {
    let all = vec![IrCell::Wildcard, IrCell::Index { index: 1 }];
    for v in &all {
        match v {
            IrCell::Wildcard | IrCell::Index { .. } => {}
        }
    }
    all
}

fn writes() -> Vec<IrWrite> {
    let all = vec![IrWrite::Keep, IrWrite::Index { index: 1 }];
    for v in &all {
        match v {
            IrWrite::Keep | IrWrite::Index { .. } => {}
        }
    }
    all
}

fn moves() -> Vec<IrMove> {
    let all = vec![IrMove::Left, IrMove::Right, IrMove::Stay];
    for v in &all {
        match v {
            IrMove::Left | IrMove::Right | IrMove::Stay => {}
        }
    }
    all
}

fn transitions() -> Vec<IrTransition> {
    let all = vec![
        IrTransition::Goto { state: 0 },
        IrTransition::CallThen {
            target: "r".into(),
            binding: Vec::new(),
            exits: Vec::new(),
            then: Some(IrThen::Return),
        },
        IrTransition::Return,
        IrTransition::ReturnExit { exit: 0 },
        IrTransition::Stop,
        IrTransition::Halt,
        IrTransition::TailCall { target: "r".into() },
        IrTransition::TrapRead,
        IrTransition::TrapWrite,
        IrTransition::TrapContract,
    ];
    for v in &all {
        match v {
            IrTransition::Goto { .. }
            | IrTransition::CallThen { .. }
            | IrTransition::Return
            | IrTransition::ReturnExit { .. }
            | IrTransition::Stop
            | IrTransition::Halt
            | IrTransition::TailCall { .. }
            | IrTransition::TrapRead
            | IrTransition::TrapWrite
            | IrTransition::TrapContract => {}
        }
    }
    all
}

fn thens() -> Vec<IrThen> {
    let all = vec![
        IrThen::Goto { state: 0 },
        IrThen::Return,
        IrThen::ReturnExit { exit: 0 },
        IrThen::Stop,
        IrThen::Halt,
    ];
    for v in &all {
        match v {
            IrThen::Goto { .. }
            | IrThen::Return
            | IrThen::ReturnExit { .. }
            | IrThen::Stop
            | IrThen::Halt => {}
        }
    }
    all
}

fn dispatches() -> Vec<IrDispatch> {
    let all = vec![IrDispatch::Table, IrDispatch::Branch];
    for v in &all {
        match v {
            IrDispatch::Table | IrDispatch::Branch => {}
        }
    }
    all
}

/// Each tag vocabulary the page publishes equals the one the code
/// serializes, in both directions.
///
/// Mutation it catches: rename one variant's wire tag (a
/// `#[serde(rename = "trap_readx")]` on `IrTransition::TrapRead`) — the
/// code side gains `trap_readx` and loses `trap_read`, and the transition
/// compare fails both ways; likewise a new variant sampled here but not
/// listed on the page, or a tag the page keeps after the code drops it.
#[test]
fn the_published_ir_tags_are_exactly_the_serialized_ones() {
    let s = section();
    let checks: [(&str, BTreeSet<String>, BTreeSet<String>); 7] = [
        (
            "world kinds",
            documented(&s, "`kind` per world is ", ". Graphs"),
            tags(&world_kinds()),
        ),
        (
            "pattern cells",
            documented(&s, "a pattern cell is ", ";"),
            tags(&cells()),
        ),
        (
            "write cells",
            documented(&s, "a write cell is ", ". Moves"),
            tags(&writes()),
        ),
        (
            "moves",
            documented(&s, "Moves are ", ". `write`"),
            tags(&moves()),
        ),
        (
            "transitions",
            documented(
                &s,
                "Per-transition tags (`kind` field, snake_case): ",
                "A `binding` entry",
            ),
            tags(&transitions()),
        ),
        (
            "call_then resume points",
            documented(&s, "resume point that is itself a ", " when present"),
            tags(&thens()),
        ),
        (
            "dispatch hints",
            documented(&s, "`dispatch` is a codegen hint, ", ". `tail_call`"),
            tags(&dispatches()),
        ),
    ];
    let mut drift = Vec::new();
    for (what, page, code) in &checks {
        if page != code {
            drift.push(format!(
                "{what}: only on the page {:?}, only in the code {:?}",
                page.difference(code).collect::<Vec<_>>(),
                code.difference(page).collect::<Vec<_>>()
            ));
        }
    }
    assert!(
        drift.is_empty(),
        "docs/formats.md (IR JSON) and the IR's serialized tags disagree:\n{}",
        drift.join("\n")
    );
}
