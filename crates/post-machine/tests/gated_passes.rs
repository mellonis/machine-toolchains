//! The volatile column's gated pass set — one discriminating test per
//! verdict (the contract lives in crates/post-machine/src/optimizer/mod.rs).
//!
//! Two columns are compiled from the same source at `-O1`: the NORMAL one
//! (today's pipeline) and the VOLATILE one (the same pipeline minus
//! `gated_pass_names()`). A **gated** verdict is pinned by a program whose
//! tape-transaction sequence the pass would change: the volatile column
//! must keep the transactions the source wrote. A **clean** verdict is
//! pinned the other way — the pass must still FIRE in the volatile column,
//! proven against a control that disables just that pass on top of the
//! gate, so an over-broad gate fails loudly instead of passing silently.

use mtc_core::linker::LinkOptions;
use mtc_core::vm::{
    ArchRegistry, DeviceFault, InfiniteTape, Machine, Outcome, RunLimits, RunOptions, StrictTape,
    Trap,
};
use mtc_post_machine::arch::Pm1;
use mtc_post_machine::asm::link;
use mtc_post_machine::compiler::{CompileOptions, compile};
use mtc_post_machine::optimizer::{OptLevel, gated_pass_names, pass_names};

fn options(disabled: &[&str]) -> CompileOptions {
    CompileOptions {
        opt_level: OptLevel::O1,
        disabled_passes: disabled.iter().map(|s| (*s).to_string()).collect(),
        ..Default::default()
    }
}

/// The `-S` listing the object is assembled from, at `-O1` with `disabled` off.
fn listing(src: &str, disabled: &[&str]) -> String {
    compile(src, options(disabled)).expect("compiles").pma
}

/// Today's `-O1` pipeline: the NORMAL column.
fn normal(src: &str) -> String {
    listing(src, &[])
}

/// The VOLATILE column: the same pipeline minus the gated set.
fn volatile(src: &str) -> String {
    listing(src, gated_pass_names())
}

/// The volatile column with ONE more pass disabled — the control that
/// proves a clean pass really fired in the column above it.
fn volatile_without(src: &str, pass: &str) -> String {
    let mut disabled: Vec<&str> = gated_pass_names().to_vec();
    disabled.push(pass);
    listing(src, &disabled)
}

/// Instruction mnemonics of a listing, in emission order. Instruction
/// lines are indented; `.func` directives and `L<n>:` labels are not.
fn mnemonics(listing: &str) -> Vec<&str> {
    listing
        .lines()
        .filter(|l| l.starts_with(char::is_whitespace))
        .filter_map(|l| l.split_whitespace().next())
        .collect()
}

fn count(listing: &str, mnemonic: &str) -> usize {
    mnemonics(listing)
        .iter()
        .filter(|m| **m == mnemonic)
        .count()
}

/// Link one column and run it on a blank strict-cell tape.
fn run_strict(src: &str, disabled: &[&str]) -> Outcome {
    let out = compile(src, options(disabled)).expect("compiles");
    let exe = link(&[out.object], &[], LinkOptions::default())
        .expect("links")
        .executable;
    let mut registry = ArchRegistry::new();
    registry.register(Box::new(Pm1));
    let machine = Machine::from_executable(&exe, &registry).expect("loads");
    let mut tape = StrictTape::new(InfiniteTape::new());
    let run_options = RunOptions {
        limits: RunLimits {
            max_steps: Some(10_000),
            ..Default::default()
        },
        ..Default::default()
    };
    machine.run(&mut tape, run_options).outcome
}

// ---------------------------------------------------------------- gated

#[test]
fn cell_state_keeps_the_second_write() {
    // Rule 1 (idempotent write): the second `mark` is dropped only because
    // the first one's value is assumed to read back.
    let src = "main() { mark; mark; }";
    assert_eq!(count(&normal(src), "wr"), 1, "{}", normal(src));
    assert_eq!(count(&volatile(src), "wr"), 2, "{}", volatile(src));
}

#[test]
fn cell_state_dead_store_survives() {
    // Rule 2 (block-local dead store): `mark, unmark` loses the `wr 1`
    // normally — a transaction the source asked a device to perform.
    let src = "main() { mark, unmark; }";
    assert_eq!(count(&normal(src), "wr"), 1, "{}", normal(src));
    assert_eq!(count(&volatile(src), "wr"), 2, "{}", volatile(src));
}

#[test]
fn cell_state_strict_cell_fault_is_kept_by_the_volatile_column() {
    // End-to-end observable: on a blank strict cell the SECOND `mark`
    // rewrites a marked cell and faults. The normal column dropped it and
    // stops cleanly; the volatile column performs it and traps.
    let src = "main() { mark; mark; }";
    assert_eq!(run_strict(src, &[]), Outcome::Stopped);
    assert_eq!(
        run_strict(src, gated_pass_names()),
        Outcome::Trapped(Trap::Device {
            fault: DeviceFault::StrictCellViolation
        })
    );
}

#[test]
fn fuse_tape_ops_keeps_write_and_move_as_two_transactions() {
    // `wrr` performs write+move+latch in one instruction: the intermediate
    // latch READ of the written cell never happens. Two source-written
    // transactions must stay two.
    let src = "main() { mark; right; }";
    let n = normal(src);
    assert_eq!(
        (count(&n, "wrr"), count(&n, "wr"), count(&n, "rgt")),
        (1, 0, 0),
        "{n}"
    );
    let v = volatile(src);
    assert_eq!(
        (count(&v, "wrr"), count(&v, "wr"), count(&v, "rgt")),
        (0, 1, 1),
        "{v}"
    );
}

/// move-elim is GATED: `right; left;` after a write re-couples MF on a
/// real tape but is two observable transactions on a volatile one.
const MOVE_PAIR: &str =
    "main() {\n    mark;\n    right;\n    left;\n    check(1, !);\n1:  unmark;\n}\n";

fn count_moves(listing: &str) -> usize {
    count(listing, "lft") + count(listing, "rgt")
}

#[test]
fn move_elim_is_gated() {
    assert!(count_moves(&normal(MOVE_PAIR)) < count_moves(&volatile(MOVE_PAIR)));
}

#[test]
fn branch_fold_does_not_predict_the_check_from_a_written_value() {
    // `mark; check(...)`: the normal column folds the branch because a
    // write is assumed to read back. The volatile column must test the
    // real match flag — the conditional jump and the blank arm both stay.
    let src = "main() { mark; check(1, 2); 1: right(!); 2: left; }";
    let n = normal(src);
    assert_eq!((count(&n, "jnm"), count(&n, "lft")), (0, 0), "{n}");
    let v = volatile(src);
    assert_eq!((count(&v, "jnm"), count(&v, "lft")), (1, 1), "{v}");
}

// ---------------------------------------------------------------- clean

#[test]
fn check_fold_still_fires_in_the_volatile_column() {
    // Identical arms decide nothing whatever the match flag holds.
    let src = "main() { right; check(5, 5); 5: mark; }";
    assert_eq!(count(&volatile(src), "jm"), 0, "{}", volatile(src));
    assert_eq!(count(&volatile_without(src, "check-fold"), "jm"), 1);
}

#[test]
fn jump_threading_still_fires_in_the_volatile_column() {
    // Empty forwarder blocks carry no transactions to preserve.
    let src = "main() { goto 2; 1: mark(!); 2: goto 1; }";
    assert_eq!(count(&volatile(src), "jmp"), 0, "{}", volatile(src));
    assert_eq!(count(&volatile_without(src, "jump-threading"), "jmp"), 2);
}

#[test]
fn dce_still_fires_in_the_volatile_column() {
    // Unreachable code performs no transactions at all.
    let src = "main() { goto 1; right; 1: left; }";
    assert_eq!(count(&volatile(src), "rgt"), 0, "{}", volatile(src));
    assert_eq!(count(&volatile_without(src, "dce"), "rgt"), 1);
}

#[test]
fn inline_still_fires_in_the_volatile_column() {
    // Splicing a leaf callee removes `call`/`ret`, never a tape access.
    let src = "export f() { right; } main() { @f(); mark; }";
    assert_eq!(count(&volatile(src), "call"), 0, "{}", volatile(src));
    assert_eq!(count(&volatile_without(src, "inline"), "call"), 1);
}

#[test]
fn tail_call_still_fires_in_the_volatile_column() {
    // `f` is no leaf, so nothing inlines: `g`'s trailing call becomes the
    // tail jump. Stack traffic is not tape traffic.
    let src =
        "export g() { left; @f(); } export f() { @h(); } export h() { right; } main() { @g(); }";
    assert_eq!(count(&volatile(src), "jmp"), 1, "{}", volatile(src));
    assert_eq!(count(&volatile_without(src, "tail-call"), "jmp"), 0);
}

#[test]
fn tail_merge_still_fires_in_the_volatile_column() {
    // The two arms run the same ops; sharing one copy leaves every path's
    // executed transaction sequence exactly as written.
    let src = "main() { 1: check(2, 3); 2: mark, right(!); 3: mark, right(!); }";
    assert_eq!(count(&volatile(src), "wr"), 1, "{}", volatile(src));
    assert_eq!(count(&volatile_without(src, "tail-merge"), "wr"), 2);
}

#[test]
fn tail_sink_still_fires_in_the_volatile_column() {
    // Both arms' trailing `right; right;` are the same instruction
    // whichever arm ran — sinking them into the shared join drops the
    // duplicate static copy without touching any executed access.
    let src = "main() {\n    check(1, 2);\n1:  mark;\n    right;\n    right;\n    goto 3;\n2:  left;\n    right;\n    right;\n3:  unmark;\n}\n";
    assert_eq!(count_moves(&volatile(src)), 3, "{}", volatile(src));
    assert_eq!(count_moves(&volatile_without(src, "tail-sink")), 5);
}

// ----------------------------------------------------------- drift guard

#[test]
fn gated_set_is_a_nonempty_duplicate_free_subset_of_the_pipeline() {
    let all = pass_names();
    let gated = gated_pass_names();
    assert!(
        !gated.is_empty(),
        "the volatile column gates at least cell-state and fuse-tape-ops"
    );
    for name in gated {
        assert!(
            all.contains(name),
            "gated pass `{name}` is not a pipeline pass — was it renamed? {all:?}"
        );
    }
    let mut unique = gated.to_vec();
    unique.sort_unstable();
    unique.dedup();
    assert_eq!(unique.len(), gated.len(), "duplicate entry in {gated:?}");
}
