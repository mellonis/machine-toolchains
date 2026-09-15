//! The call-mechanism matrix on programs that carry EXIT VECTORS
//! (docs/core.md (call mechanisms)), run on the real TM-1 arch.
//!
//! Mono splices a per-site copy of an exit-bearing callee, entered by a
//! jump and leaving through jumps; frames gives every site a descriptor
//! carrying its exits; hybrid decides per fold group and can produce an
//! image that does BOTH — a stamped copy reaching a shared generic body
//! through a framed call. The three images differ by construction; what
//! must not differ is what they compute.
//!
//! This is where that claim is EXECUTED. Core's `link_exits.rs` proves the
//! byte arithmetic and the jump displacements against a fake dialect it
//! cannot run; nothing before this file ran a spliced or a shared image at
//! all.

use mtc_core::formats::executable::Executable;
use mtc_core::formats::tapeblock::TapeSnapshot;
use mtc_core::linker::{CallMech, LinkOptions, LinkOutput};
use mtc_core::vm::{ArchRegistry, Machine, Outcome, RunLimits, RunOptions, Tape, WideTape};
use mtc_turing_machine::arch::Tm1;
use mtc_turing_machine::asm::{assemble, link};

// ── harness ────────────────────────────────────────────────────────────────

const MECHS: [CallMech; 3] = [CallMech::Mono, CallMech::Frames, CallMech::Hybrid];

/// Assemble + link `src` under `mech`, keeping the whole link output: a
/// mechanism test usually has to check the REPORT as well as the image, to
/// be sure the mechanism it means to exercise is the one that ran.
fn build_full(src: &str, mech: CallMech) -> LinkOutput {
    let obj = assemble(src, false).expect("assembles");
    link(
        &[obj],
        &[],
        LinkOptions {
            call_mech: mech,
            ..Default::default()
        },
    )
    .unwrap_or_else(|e| panic!("the {mech} link failed: {e}"))
}

/// The image alone.
fn build(src: &str, mech: CallMech) -> Executable {
    build_full(src, mech).executable
}

/// Run `exe` on blank tapes of the given per-tape alphabet widths.
fn run(exe: &Executable, widths: &[u32]) -> (Outcome, Vec<TapeSnapshot>) {
    let mut registry = ArchRegistry::new();
    registry.register(Box::new(Tm1::new(exe.tape_count)));
    let machine = Machine::from_executable(exe, &registry).expect("loads");
    let mut tapes: Vec<WideTape> = widths.iter().map(|&w| WideTape::new(w)).collect();
    let mut devices: Vec<&mut dyn Tape> = tapes.iter_mut().map(|t| t as &mut dyn Tape).collect();
    let result = machine
        .run_tapes(
            &mut devices,
            RunOptions {
                limits: RunLimits {
                    max_steps: Some(100_000),
                    ..Default::default()
                },
                ..Default::default()
            },
        )
        .expect("run set-up ok");
    drop(devices);
    let snaps = tapes.iter().map(WideTape::to_snapshot).collect();
    (result.outcome, snaps)
}

/// The cell at absolute position `pos` in a snapshot (blank past the ends).
fn cell_at(snap: &TapeSnapshot, pos: i64) -> u8 {
    let idx = pos - snap.origin;
    if idx < 0 || idx as usize >= snap.cells.len() {
        0
    } else {
        snap.cells[idx as usize]
    }
}

/// `n` `nop` lines — a fold fixture's body-size knob, written as a helper
/// so the count the arithmetic names is the count in the source.
fn nops(n: usize) -> String {
    "        nop\n".repeat(n)
}

// ── the closure fold, executed ─────────────────────────────────────────────

/// The closure-fold shape on TM-1: `big` is reached once at the identity
/// world (through a swap binding) and twice inside `outer`'s stamped copy
/// (transparently, so all three compose to the SAME swap composite). Under
/// hybrid the byte rule shares the body, so the image carries a stamp of
/// `outer` whose two calls into the shared generic `big` are framed —
/// mono splices all three instead, frames descriptors all three, and every
/// one of them must leave the same tape.
///
/// The arithmetic, on TM-1 widths: the swap composite over a 4-symbol
/// alphabet makes both dense maps 4 `u16`s, so each descriptor is
/// `1 + 2 + [1 + 2 + 8 + 2 + 8] + 4` = 28 bytes and `sum(d_i)` = 84.
/// `big` is 1 `ent` + 3 (`wrmv [1], [>]`) + 50 `nop` + 2 (`retx #0`) = 56,
/// and `(3 - 1) * 56 = 112 > 84`. The flip point is 37 `nop`s.
///
/// `big` WRITES through the binding rather than merely running: it writes
/// its own virtual symbol 1, which the swap sends to physical 2, once per
/// call, walking right. A descriptor built from the site's raw binding
/// instead of `compose(C, binding)` would send it somewhere else, and the
/// tape would say so — 50 `nop`s alone could not.
fn closure_fold() -> String {
    format!(
        "\
.routine main, tapes=1, alpha=(4)
.param t, ('_', 'x', 'y', 'z')
.routine outer, tapes=1, alpha=(4)
.param u, ('_', 'x', 'y', 'z')
.routine big, tapes=1, alpha=(4), exits=1
.param n, ('_', 'x', 'y', 'z')
.section code
.func main
        call    outer [0{{1->2, 2->1}}]
        call    big [0{{1->2, 2->1}}] exits=(a)
        stp
a:      wrmv    [1], [.]
        stp
.func outer
        call    big [0] exits=(p)
        ret
p:      call    big [0] exits=(q)
        ret
q:      ret
.func big
        wrmv    [1], [>]
{}        retx    #0
",
        nops(50)
    )
}

/// The mixed image computes what the two pure ones do.
///
/// Mutation it catches: build the stamp's descriptor from the SITE's
/// binding instead of `compose(C, binding)` and the shared `big` writes
/// through the wrong symbol map from inside the copy, so hybrid's tape
/// stops matching mono's. Skip the exit remap into the copy's own blob and
/// hybrid stops linking at all.
///
/// The hybrid preconditions are asserted FIRST, and deliberately: a change
/// that quietly stopped sharing would leave this test green while covering
/// nothing but two pure mechanisms.
#[test]
fn a_closure_fold_program_agrees_across_mechanisms() {
    let src = closure_fold();

    let out = build_full(&src, CallMech::Hybrid);
    let fold = out
        .report
        .folds
        .iter()
        .find(|f| f.routine == "big")
        .unwrap_or_else(|| panic!("no fold decision for `big`: {:?}", out.report));
    assert_eq!(
        fold.sites, 3,
        "one at the identity world, two inside the copy: {fold:?}"
    );
    assert_eq!(
        fold.body_bytes, 56,
        "1 ent + 3 wrmv + 50 nop + 2 retx: {fold:?}"
    );
    assert_eq!(
        fold.descriptor_bytes, 84,
        "three 28-byte descriptors: {fold:?}"
    );
    assert!(fold.shared, "112 > 84, so hybrid shares: {fold:?}");
    assert_eq!(
        out.report.instantiations, 1,
        "only `outer` is stamped; the shared `big` stays generic: {:?}",
        out.report
    );

    let results: Vec<_> = MECHS.iter().map(|&m| run(&build(&src, m), &[4])).collect();
    for (m, r) in MECHS.iter().zip(&results).skip(1) {
        assert_eq!(
            (&results[0].0, &results[0].1),
            (&r.0, &r.1),
            "mono vs {m} diverged on a closure-fold program"
        );
    }

    // And what they agree ON is the right thing: three calls to `big`,
    // each writing its virtual 1 — physical 2 under the swap — and
    // stepping right, then `main`'s own exit handler writing physical 1 at
    // the machine identity.
    let (outcome, snaps) = &results[0];
    assert_eq!(*outcome, Outcome::Stopped, "the program runs to a stop");
    let seen: Vec<u8> = (0..4).map(|p| cell_at(&snaps[0], p)).collect();
    assert_eq!(
        seen,
        vec![2, 2, 2, 1],
        "three swapped writes then one identity write"
    );
}
