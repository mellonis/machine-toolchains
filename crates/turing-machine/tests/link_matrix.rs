//! The three call mechanisms agree on the shapes phase 2 adds: an
//! exit-bearing call (from one site and from three), a fold inside a
//! stamped copy, an open binding, a mixed splice-and-frame caller, a
//! cross-object bound call, a shared fold group reached only through a
//! frame (no identity-world member at all), a lone exit-bearing site
//! inside a framed callee (no fold group at all), one callee spliced
//! twice with a site inside each copy, and a tail-position framed call
//! observation. Driven from `.tma`, because the `.tmc` front end has no
//! `state` parameters yet (docs/core.md (call mechanisms)).
//!
//! Mono splices a per-site copy of an exit-bearing callee, entered by a
//! jump and leaving through jumps; frames gives every site a descriptor
//! carrying its exits; hybrid decides per fold group and can produce an
//! image that does BOTH — a stamped copy reaching a shared generic body
//! through a framed call. The three images differ by construction; what
//! must not differ is what they compute.
//!
//! One shape runs two of the three, and says so at its own test: a
//! framed call nested inside an EXIT-BEARING framed callee takes its
//! compose column from the identity row today, so the twice-spliced
//! callee's pure-frames image traps and only mono and hybrid are
//! compared there.
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

// ── exits, open bindings, and cross-object calls ──────────────────────────

/// ONE exit-bearing site: hybrid splices it (one site never pays to
/// share), mono splices it, frames descriptors it. All three must leave
/// the same tape. `main` seeds physical `1` before the call so the
/// dispatch hits `T0` row 1 and `retx #1` (exit index 1, `lost`) — the
/// exit an unseeded blank tape would never reach, since a blank read
/// always matches row 0 and `retx #0` (`won`).
const ONE_EXIT_SITE: &str = "\
.routine main, tapes=1, alpha=(3)
.param t, ('_', '0', '1')
.routine pick, tapes=1, alpha=(3), exits=2
.param n, ('_', '0', '1')
.section tables
T0:     .row    [0]
        .row    [1]
        .row    [2]
T1:     .targets zero, one, two
.section code
.func main
        wrmv    [1], [.]
        call    pick [n: 0] exits=(won, lost)
        stp
won:    wrmv    [1], [.]
        stp
lost:   wrmv    [2], [.]
        stp
.func pick
        rd
        mtc     T0
        djmp    T1
zero:   retx    #0
one:    retx    #1
two:    ret
";

/// THREE exit-bearing sites into one routine: the shape hybrid's byte
/// rule may share. Whatever it decides, the three mechanisms must agree
/// on the tape.
const THREE_EXIT_SITES: &str = "\
.routine main, tapes=1, alpha=(3)
.param t, ('_', '0', '1')
.routine pick, tapes=1, alpha=(3), exits=1
.param n, ('_', '0', '1')
.section tables
T0:     .row    [0]
        .row    [*]
T1:     .targets zero, rest
.section code
.func main
        call    pick [n: 0] exits=(a)
        stp
a:      call    pick [n: 0] exits=(b)
        stp
b:      call    pick [n: 0] exits=(c)
        stp
c:      wrmv    [2], [.]
        stp
.func pick
        rd
        mtc     T0
        djmp    T1
zero:   retx    #0
rest:   ret
";

/// An open binding: a 5-symbol band into a 3-symbol callee that declares
/// its tape opaque and carries a `*` row. Modelled on the probe's
/// hand-authored descriptor, written declaratively. `main` seeds three
/// cells before the call — physical `a`, `b`, then an opaque symbol
/// (`c`, with no image in `swapABopen`'s alphabet) — so the callee's walk
/// actually reaches `swapA`, `swapB`, and the `*`-row opaque passthrough,
/// not just the blank-read `done` exit.
const OPEN: &str = "\
.routine main, tapes=1, alpha=(5)
.param t, ('_', 'a', 'b', 'c', 'd')
.routine swapABopen, tapes=1, alpha=(3)
.param n, ('_', 'a', 'b'), writes=('a', 'b'), opaque
.section tables
T0:     .row    [0]
        .row    [1]
        .row    [2]
        .row    [*]
T1:     .targets done, swapA, swapB, pass
.section code
.func main
        wrmv    [1], [>]
        wrmv    [2], [>]
        wrmv    [3], [<]
        wrmv    [-], [<]
        call    swapABopen [0{1->1, 2->2, *}]
        stp
.func swapABopen
walk:   rd
        mtc     T0
        djmp    T1
swapA:  wrmv    [2], [>]
        jmp     walk
swapB:  wrmv    [1], [>]
        jmp     walk
done:   ret
pass:   wrmv    [-], [>]
        jmp     walk
";

/// The cross-object caller and callee, assembled separately and linked
/// together.
const XO_CALLER: &str = "\
.routine main, tapes=1, alpha=(5)
.param t, ('_', 'a', 'b', '0', '1')
.section code
.func main
        call    mylib::plusOne [num: 0{3->'0', 4->'1'}]
        stp
";

const XO_CALLEE: &str = "\
.routine mylib::plusOne, tapes=1, alpha=(3)
.param num, ('_', '0', '1'), writes=('0', '1')
.section code
.func mylib::plusOne
        rd
        wrmv    [2], [.]
        ret
";

/// Mutation it catches: break any one mechanism's exit lowering — mono's
/// `retx → jmp`, frames' descriptor rebase, hybrid's routing — and the
/// three stop agreeing on the final tape.
#[test]
fn an_exit_bearing_program_agrees_across_mechanisms() {
    for src in [ONE_EXIT_SITE, THREE_EXIT_SITES] {
        let results: Vec<_> = MECHS.iter().map(|&m| run(&build(src, m), &[3])).collect();
        for (m, r) in MECHS.iter().zip(&results).skip(1) {
            assert_eq!(
                (&results[0].0, &results[0].1),
                (&r.0, &r.1),
                "mono vs {m} diverged on an exit-bearing program"
            );
        }
    }
}

/// Mutation it catches: revert the open rule anywhere — the sparse map,
/// `dense_map`'s guard, or mono's `read_image` — and the opaque symbols
/// trap under at least one mechanism, so the outcomes diverge.
#[test]
fn an_open_binding_program_agrees_across_mechanisms() {
    let results: Vec<_> = MECHS.iter().map(|&m| run(&build(OPEN, m), &[5])).collect();
    for (m, r) in MECHS.iter().zip(&results).skip(1) {
        assert_eq!(
            (&results[0].0, &results[0].1),
            (&r.0, &r.1),
            "mono vs {m} diverged on an open binding"
        );
    }
}

/// Mutation it catches: resolve a cross-object binding against the wrong
/// object's interface and the callee writes through the wrong glyph, so
/// at least one mechanism's tape differs.
#[test]
fn a_cross_object_program_agrees_across_mechanisms() {
    let caller = assemble(XO_CALLER, false).expect("assembles");
    let callee = assemble(XO_CALLEE, false).expect("assembles");
    let images: Vec<_> = MECHS
        .iter()
        .map(|&m| {
            link(
                &[caller.clone(), callee.clone()],
                &[],
                LinkOptions {
                    call_mech: m,
                    ..Default::default()
                },
            )
            .unwrap_or_else(|e| panic!("the {m} link failed: {e}"))
            .executable
        })
        .collect();
    let results: Vec<_> = images.iter().map(|e| run(e, &[5])).collect();
    for (m, r) in MECHS.iter().zip(&results).skip(1) {
        assert_eq!(
            (&results[0].0, &results[0].1),
            (&r.0, &r.1),
            "mono vs {m} diverged on a cross-object bound call"
        );
    }
}

// ── the mixed splice-and-frame caller ──────────────────────────────────────

/// One caller, two sites: an exit-bearing one hybrid splices, and a
/// holey one it frames. The framed site widens, shifting the splice's
/// `then` and its exits — the only shape in this file where the offsets
/// a splice fixup names are not the ones the record carried.
const MIXED_SPLICE_AND_FRAME: &str = "\
.routine main, tapes=1, alpha=(5)
.param t, ('_', 'a', 'b', 'c', 'd')
.routine pick, tapes=1, alpha=(5), exits=1
.param n, ('_', 'a', 'b', 'c', 'd')
.routine holey, tapes=1, alpha=(3)
.param m, ('_', 'a', 'b')
.section tables
T0:     .row    [0]
        .row    [*]
T1:     .targets zero, rest
.section code
.func main
        call    pick [0] exits=(won)
        stp
won:    call    holey [0{1->1, 2->2}]
        wrmv    [3], [.]
        stp
.func pick
        rd
        mtc     T0
        djmp    T1
zero:   retx    #0
rest:   ret
.func holey
        ret
";

/// Mutation it catches: drop `splice_shift` on the hybrid path and the
/// splice's `then` lands on the wrong instruction (or misses the offset
/// map and fails the link), so hybrid stops agreeing with mono.
#[test]
fn a_mixed_splice_and_frame_caller_agrees_across_mechanisms() {
    let results: Vec<_> = MECHS
        .iter()
        .map(|&m| run(&build(MIXED_SPLICE_AND_FRAME, m), &[5]))
        .collect();
    for (m, r) in MECHS.iter().zip(&results).skip(1) {
        assert_eq!(
            (&results[0].0, &results[0].1),
            (&r.0, &r.1),
            "mono vs {m} diverged on a mixed splice/frame caller"
        );
    }
}

// ── a shared fold group reached only through a frame ──────────────────────

/// `outer` is a mono seed (an exit-free bijection reached at the machine
/// identity through a swap), so the closure probe finds all three
/// exit-bearing calls to `big` INSIDE its stamped copy — none at the
/// identity world itself, unlike `closure_fold`'s mixed 1-plus-2 split.
/// The whole group therefore lives under FR ≠ 0 (the swap composite) with
/// no identity-world member at all. `main` separately calls `holey`
/// through an unequal-cardinality (non-bijection) binding, which is never
/// a mono seed and can never join the group — it exists only to put a
/// SECOND, distinct composite into the link (the swap, and holey's own),
/// so the engine's total composite count (2) differs from the group's
/// site count (3): a byte-rule sharer that sized its runtime compose
/// column count from the wrong one of the two would be caught here and
/// not by `closure_fold`, where they coincide.
///
/// Sizing mirrors `closure_fold()` exactly (same 4-symbol alphabet, same
/// swap composite, same `big` body), so the byte-rule arithmetic is
/// already proven: `(3 - 1) * 56 = 112 > 84`, so hybrid shares. A function
/// rather than a plain const, for the same reason `closure_fold` is one:
/// the `nops(50)` body-size knob has to be interpolated by `format!`,
/// which needs a literal format string at the call site.
fn shared_under_frame() -> String {
    format!(
        "\
.routine main, tapes=1, alpha=(4)
.param t, ('_', 'x', 'y', 'z')
.routine outer, tapes=1, alpha=(4)
.param u, ('_', 'x', 'y', 'z')
.routine big, tapes=1, alpha=(4), exits=1
.param n, ('_', 'x', 'y', 'z')
.routine holey, tapes=1, alpha=(3)
.param m, ('_', 'x', 'y')
.section code
.func main
        call    outer [0{{1->2, 2->1}}]
        call    holey [0{{1->1, 2->2}}]
        stp
.func outer
        call    big [0] exits=(p)
        ret
p:      call    big [0] exits=(q)
        ret
q:      call    big [0] exits=(r)
        ret
r:      ret
.func big
        wrmv    [1], [>]
{}        retx    #0
.func holey
        ret
",
        nops(50)
    )
}

/// Mutation it catches: size the frames compose matrix from the engine's
/// total composite count (2) instead of the group's full K (3) — the
/// shared site's descriptor under FR ≠ 0 then composes against the wrong
/// column, and hybrid stops agreeing with mono.
#[test]
fn a_shared_group_under_an_active_frame_agrees_across_mechanisms() {
    let src = shared_under_frame();

    let out = build_full(&src, CallMech::Hybrid);
    let fold = out
        .report
        .folds
        .iter()
        .find(|f| f.routine == "big")
        .unwrap_or_else(|| panic!("no fold decision for `big`: {:?}", out.report));
    assert_eq!(
        fold.sites, 3,
        "all three sites live inside outer's copy: {fold:?}"
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

    let results: Vec<_> = MECHS.iter().map(|&m| run(&build(&src, m), &[4])).collect();
    for (m, r) in MECHS.iter().zip(&results).skip(1) {
        assert_eq!(
            (&results[0].0, &results[0].1),
            (&r.0, &r.1),
            "mono vs {m} diverged on a shared group reached only through a frame"
        );
    }
}

// ── one callee spliced twice, and the sites inside both copies ────────────

/// The same `(routine, composite)` reached through TWO distinct splice
/// sites, each copy carrying an exit-bearing site of its own. `body` is
/// `big`'s `nop` padding — the knob that moves ITS group across the byte
/// rule while everything above stays fixed.
///
/// `outer` is the only exit-free bijection seed (a swap at the machine
/// identity), so everything under it is met inside a copy. Its two calls
/// to `mid` agree on callee and composite — both bindings are the
/// identity under the swap — so they are ONE group; they differ in where
/// they return to, which is what makes them two distinct splices. That
/// group is refused (`(2 - 1) * 10 = 10` is not more than the `2 * 28 =
/// 56` its descriptors would cost, each descriptor being the 28 bytes
/// `closure_fold` derives for this swap and one exit), so `mid` is copied
/// once per site and each copy carries its own splice of `big`.
///
/// `big`'s group therefore has TWO members, one per `mid` copy. A walk
/// that stopped at a `(routine, composite)` it had already seen would
/// find only the first — it walks `mid` once where the builder builds it
/// twice — and at 60 `nop`s that is the difference between a group that
/// shares a body and a lone site that cannot.
fn twice_spliced_callee(body: usize) -> String {
    format!(
        "\
.routine main, tapes=1, alpha=(4)
.param t, ('_', 'x', 'y', 'z')
.routine outer, tapes=1, alpha=(4)
.param u, ('_', 'x', 'y', 'z')
.routine mid, tapes=1, alpha=(4), exits=1
.param v, ('_', 'x', 'y', 'z')
.routine big, tapes=1, alpha=(4), exits=1
.param n, ('_', 'x', 'y', 'z')
.section code
.func main
        call    outer [0{{1->2, 2->1}}]
        stp
.func outer
        call    mid [0] exits=(p)
        ret
p:      call    mid [0] exits=(q)
        ret
q:      ret
.func mid
        call    big [0] exits=(r)
        retx    #0
r:      retx    #0
.func big
        wrmv    [1], [>]
{}        retx    #0
",
        nops(body)
    )
}

/// Mutation it catches: key the closure walk on `(routine, composite)`
/// rather than on the stamp intern key, and the site inside the SECOND
/// copy of `mid` goes uncounted — `big`'s group reports one site where
/// two are built, and at the 60-`nop` sizing below it splices twice
/// instead of sharing one body.
///
/// Pure frames is not in the comparison, alone in this file: a framed
/// call nested inside an EXIT-BEARING framed callee takes its compose
/// column from the identity row today and the image traps there. That is
/// a frames-path defect this shape exposes, not something the fold count
/// steers — mono and hybrid are the two mechanisms it does steer.
#[test]
fn a_site_inside_each_copy_of_a_twice_spliced_callee_is_counted() {
    let small = twice_spliced_callee(20);
    let out = build_full(&small, CallMech::Hybrid);
    let fold = |out: &LinkOutput, name: &str| {
        out.report
            .folds
            .iter()
            .find(|f| f.routine == name)
            .unwrap_or_else(|| panic!("no fold decision for `{name}`: {:?}", out.report))
            .clone()
    };
    let mid = fold(&out, "mid");
    assert_eq!(mid.sites, 2, "both calls inside outer's copy: {mid:?}");
    assert_eq!(
        mid.body_bytes, 10,
        "1 ent + 5 call + 2 retx + 2 retx: {mid:?}"
    );
    assert_eq!(mid.descriptor_bytes, 56, "two 28-byte descriptors: {mid:?}");
    assert!(
        !mid.shared,
        "10 is not more than 56, so mid splices: {mid:?}"
    );

    let big = fold(&out, "big");
    assert_eq!(
        big.sites, 2,
        "one inside each copy of mid, not one for the pair: {big:?}"
    );
    assert_eq!(
        big.body_bytes, 26,
        "1 ent + 3 wrmv + 20 nop + 2 retx: {big:?}"
    );
    assert_eq!(big.descriptor_bytes, 56, "two 28-byte descriptors: {big:?}");
    assert!(
        !big.shared,
        "26 is not more than 56, so big splices: {big:?}"
    );
    assert_eq!(
        out.report.instantiations, 5,
        "one outer, two mids, one big per mid: {:?}",
        out.report
    );

    // The count is not cosmetic. At 60 `nop`s the same shape crosses the
    // byte rule — `(2 - 1) * 66 = 66 > 56` — and hybrid shares ONE `big`,
    // framed from inside each spliced copy of `mid`. A one-site group
    // could never get there, whatever its body size.
    let large = twice_spliced_callee(60);
    let out = build_full(&large, CallMech::Hybrid);
    let big = fold(&out, "big");
    assert_eq!(big.sites, 2, "the same two sites: {big:?}");
    assert_eq!(
        big.body_bytes, 66,
        "1 ent + 3 wrmv + 60 nop + 2 retx: {big:?}"
    );
    assert!(big.shared, "66 > 56, so hybrid shares: {big:?}");

    for src in [&small, &large] {
        let mono = run(&build(src, CallMech::Mono), &[4]);
        let hybrid = run(&build(src, CallMech::Hybrid), &[4]);
        assert_eq!(
            (&mono.0, &mono.1),
            (&hybrid.0, &hybrid.1),
            "mono vs hybrid diverged on a twice-spliced callee"
        );
        // What they agree ON: `big` runs once per `mid` call, writing its
        // virtual 1 — physical 2 under the swap — and stepping right.
        assert_eq!(mono.0, Outcome::Stopped, "the program runs to a stop");
        let seen: Vec<u8> = (0..3).map(|p| cell_at(&mono.1[0], p)).collect();
        assert_eq!(seen, vec![2, 2, 0], "two swapped writes, then a blank");
    }
}

// ── an exit-bearing site inside a framed callee ────────────────────────────

/// `outer` is reached only through a holey (unequal-cardinality) binding
/// from `main`, so — like `shared_under_frame`'s `holey` — it is never in
/// `identity_world` and never a mono seed: its own `call pick [0]
/// exits=(a)` is never grouped by the byte rule at all (no `FoldDecision`
/// is ever produced for `pick`). It becomes an ordinary framed call from
/// inside `outer`'s own already-framed, ungrouped body — the shape where
/// the exit's landing address has to be computed relative to a caller
/// that is ITSELF visited under a non-identity `fr_row`, not the machine's
/// own frame. `pick` dispatches and returns through its exit exactly as
/// `ONE_EXIT_SITE`'s does.
const EXITS_UNDER_FRAME: &str = "\
.routine main, tapes=1, alpha=(5)
.param t, ('_', 'a', 'b', 'c', 'd')
.routine outer, tapes=1, alpha=(3)
.param u, ('_', 'x', 'y')
.routine pick, tapes=1, alpha=(3), exits=1
.param n, ('_', 'x', 'y')
.section tables
T0:     .row    [0]
        .row    [*]
T1:     .targets zero, rest
.section code
.func main
        call    outer [0{1->1, 2->2}]
        stp
.func outer
        call    pick [0] exits=(a)
        ret
a:      wrmv    [1], [.]
        ret
.func pick
        rd
        mtc     T0
        djmp    T1
zero:   retx    #0
rest:   ret
";

/// Mutation it catches: drop the exit shift into post-rewrite offsets for
/// a site visited under a non-identity `fr_row` in `build_plan`, and the
/// framed exit lands on the wrong instruction under frames/hybrid — no
/// fold decision exists for this site to check instead, so the tape
/// compare is the only signal.
#[test]
fn an_exit_bearing_site_inside_a_framed_callee_agrees_across_mechanisms() {
    let results: Vec<_> = MECHS
        .iter()
        .map(|&m| run(&build(EXITS_UNDER_FRAME, m), &[5]))
        .collect();
    for (m, r) in MECHS.iter().zip(&results).skip(1) {
        assert_eq!(
            (&results[0].0, &results[0].1),
            (&r.0, &r.1),
            "mono vs {m} diverged on an exit-bearing site inside a framed callee"
        );
    }
}

// ── a tail-position framed call: observation ───────────────────────────────

/// A caller whose LAST instruction is a holey bound call, with nothing
/// after it in the blob — no `stp`, no `ret`. Under mono every bound call
/// stamps regardless of shape, so this becomes a plain `call` into a
/// stamped copy of `holey` (mono has no frames machinery to route through
/// in the first place). Under frames and hybrid the holey binding is
/// never a mono seed (unequal cardinalities fail `is_bijection`), so
/// `holey` is reached through a framed call, both mechanisms taking the
/// same `seeds.is_empty()` early return to pure frames.
///
/// `holey` itself ends in a plain `ret`, so whatever return continuation
/// each mechanism records for this call site — a stamped copy's return
/// address into a caller blob that has nothing after the call, or a
/// framed call's own continuation bookkeeping — is exercised here for the
/// first time in this file. This is an OBSERVATION fixture: the ruling on
/// whether the linker should refuse this shape outright is for the
/// controller, not this test.
///
/// Observed 2026-09-15: all three mechanisms link the shape without
/// refusing it, and all three run to the SAME trap kind,
/// `trapped:stack-underflow` — `ret` pops a call stack `main`'s own
/// tail-position call left nothing further to return into, and that
/// happens identically under a stamped copy's return address and under a
/// framed call's own bookkeeping.
const TAIL_POSITION_FRAMED_CALL: &str = "\
.routine main, tapes=1, alpha=(4)
.param t, ('_', 'x', 'y', 'z')
.routine holey, tapes=1, alpha=(3)
.param m, ('_', 'x', 'y')
.section code
.func main
        call    holey [0{1->1, 2->2}]
.func holey
        ret
";

/// The outcome KIND alone (mono and frames lay out code differently, so a
/// trap's `at` offset legitimately differs — docs/core.md (call
/// mechanisms), the trap-taxonomy claim carried over from
/// `mode_equivalence.rs`).
fn outcome_kind_only(a: &Outcome, b: &Outcome) -> bool {
    match (a, b) {
        (Outcome::Trapped(ta), Outcome::Trapped(tb)) => {
            std::mem::discriminant(ta) == std::mem::discriminant(tb)
        }
        _ => a == b,
    }
}

#[test]
fn a_tail_position_framed_call_is_observed() {
    let observed: Vec<(CallMech, Result<Outcome, String>)> = MECHS
        .iter()
        .map(|&m| {
            let obj = assemble(TAIL_POSITION_FRAMED_CALL, false).expect("assembles");
            let link_result = link(
                &[obj],
                &[],
                LinkOptions {
                    call_mech: m,
                    ..Default::default()
                },
            );
            match link_result {
                Ok(out) => {
                    let (outcome, _snaps) = run(&out.executable, &[4]);
                    (m, Ok(outcome))
                }
                Err(e) => (m, Err(e.to_string())),
            }
        })
        .collect();

    let first = &observed[0].1;
    let all_agree = observed.iter().all(|(_, r)| match (first, r) {
        (Ok(a), Ok(b)) => outcome_kind_only(a, b),
        // Two refusals agree only when they say the SAME thing — comparing
        // by `Display` text catches one mechanism refusing for a
        // different reason than another, which a blanket `(Err, Err) =>
        // true` would hide.
        (Err(ea), Err(eb)) => ea == eb,
        _ => false,
    });

    if !all_agree {
        // Per-mechanism observation, spelled out so a failure report never
        // has to re-derive it:
        for (m, r) in &observed {
            match r {
                Ok(o) => eprintln!("{m}: linked, ran to {o:?}"),
                Err(e) => eprintln!("{m}: link refused: {e}"),
            }
        }
    }
    assert!(
        all_agree,
        "tail-position framed call: mechanisms diverge, ruling pending: {observed:?}"
    );
}
