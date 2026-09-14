//! Exit vectors on a declarative bound call (docs/core.md (call
//! mechanisms)), on a neutral fake dialect. Frames carries them in the
//! descriptor; mono splices a per-site copy; hybrid folds.
//!
//! Everything runs on a per-file fake dialect (per-file-helper
//! convention), so core stays provably arch-agnostic.

use mtc_core::asm::{ArchSyntax, AsmCaps, Flow, RelaxPair, SyntaxEntry, assemble};
use mtc_core::formats::object::ObjectFile;
use mtc_core::linker::{CallMech, LinkError, LinkOptions, link};
use mtc_core::vm::OperandKind;

const ARCH: u8 = 0x7E;

/// A neutral fake dialect with the interface capability on: nop/stp/ret/
/// ent, a relaxable far/short call pair, the framed call the frames path
/// lowers into, the read/write/move/trap surface mono stamping projects,
/// an unconditional `jmp`, and the multi-exit return `retx` an
/// exit-bearing callee comes back through.
fn fake_syntax() -> ArchSyntax {
    use Flow::{Call, FallThrough as FT, Jump, Stop};
    ArchSyntax {
        entries: vec![
            SyntaxEntry {
                opcode: 0x01,
                mnemonic: "nop",
                operand: OperandKind::None,
                flow: FT,
            },
            SyntaxEntry {
                opcode: 0x02,
                mnemonic: "stp",
                operand: OperandKind::None,
                flow: Stop,
            },
            SyntaxEntry {
                opcode: 0x0B,
                mnemonic: "ret",
                operand: OperandKind::None,
                flow: Stop,
            },
            SyntaxEntry {
                opcode: 0x21,
                mnemonic: "call",
                operand: OperandKind::RelI32,
                flow: Call,
            },
            SyntaxEntry {
                opcode: 0x31,
                mnemonic: "call.s",
                operand: OperandKind::RelI8,
                flow: Call,
            },
            SyntaxEntry {
                opcode: 0x14,
                mnemonic: "fcall",
                operand: OperandKind::FramedCall,
                flow: Call,
            },
            SyntaxEntry {
                opcode: 0x04,
                mnemonic: "rd",
                operand: OperandKind::None,
                flow: FT,
            },
            SyntaxEntry {
                opcode: 0x07,
                mnemonic: "wr",
                operand: OperandKind::SymbolVec,
                flow: FT,
            },
            SyntaxEntry {
                opcode: 0x0F,
                mnemonic: "mov",
                operand: OperandKind::MoveVec,
                flow: FT,
            },
            SyntaxEntry {
                opcode: 0x18,
                mnemonic: "trap",
                operand: OperandKind::Imm8,
                flow: FT,
            },
            SyntaxEntry {
                opcode: 0x0E,
                mnemonic: "ent",
                operand: OperandKind::None,
                flow: FT,
            },
            SyntaxEntry {
                opcode: 0x20,
                mnemonic: "jmp",
                operand: OperandKind::RelI32,
                flow: Jump,
            },
            SyntaxEntry {
                opcode: 0x1A,
                mnemonic: "retx",
                operand: OperandKind::Imm8,
                flow: Stop,
            },
        ],
        relax_pairs: vec![RelaxPair {
            far: 0x21,
            short: 0x31,
        }],
        entry_opcode: 0x0E,
        break_opcode: None,
        trap_opcode: Some(0x18),
        return_opcode: Some(0x0B),
        caps: AsmCaps {
            tables: true,
            rept: true,
            vectors: true,
            volatile: false,
            interface: true,
        },
    }
}

fn asm(src: &str) -> ObjectFile {
    assemble(&fake_syntax(), ARCH, src, false).expect("assembles")
}

const MECHS: [CallMech; 3] = [CallMech::Mono, CallMech::Frames, CallMech::Hybrid];

fn opts(mech: CallMech) -> LinkOptions {
    LinkOptions {
        call_mech: mech,
        ..Default::default()
    }
}

/// The frames directory's absolute descriptor offsets, read out of the
/// emitted region (docs/formats.md (frames region)): `K u16`, `S u16`,
/// then `K × u32`.
fn directory(exe: &mtc_core::formats::executable::Executable) -> Vec<u32> {
    assert_ne!(exe.frames_offset, 0, "no frames region in the image");
    let t = &exe.tables;
    let mut p = exe.frames_offset as usize;
    let k = u16::from_le_bytes([t[p], t[p + 1]]);
    p += 4;
    (0..k)
        .map(|_| {
            let v = u32::from_le_bytes(t[p..p + 4].try_into().unwrap());
            p += 4;
            v
        })
        .collect()
}

/// One frame descriptor's EXIT VECTOR, decoded from its own header rather
/// than searched for (docs/formats.md (frame descriptors)): `arity u8`,
/// `exit_count u16`, then per tape `phys u8` + two length-prefixed `u16`
/// maps, and finally `exit_count` little-endian `u32`s. Walking the
/// header is what makes the assertion positional — a byte search would
/// also be satisfied by the right numbers landing in the wrong field.
fn descriptor_exits(exe: &mtc_core::formats::executable::Executable, at: u32) -> Vec<u32> {
    let t = &exe.tables;
    let start = at as usize;
    let arity = t[start];
    let exit_count = u16::from_le_bytes([t[start + 1], t[start + 2]]);
    let mut p = start + 3;
    for _ in 0..arity {
        p += 1; // phys
        for _ in 0..2 {
            let len = u16::from_le_bytes([t[p], t[p + 1]]) as usize;
            p += 2 + 2 * len;
        }
    }
    (0..exit_count)
        .map(|_| {
            let v = u32::from_le_bytes(t[p..p + 4].try_into().unwrap());
            p += 4;
            v
        })
        .collect()
}

/// A caller whose `mid` bound-calls a two-exit `sub`, which returns
/// through `retx`. Every routine declares its interface, so the exit
/// count is checkable.
///
/// The exit-bearing call deliberately sits in `mid`, NOT in the entry
/// `main`: the entry is laid at code offset 0, where an exit's absolute
/// address and its un-rebased blob offset are the same number and no
/// assertion could tell a missing rebase from a correct one. One
/// interposed function gives `mid` a non-zero base and separates them.
const TWO_EXITS: &str = "\
.routine main, tapes=1, alpha=(3)
.param t, ('_', '0', '1')
.routine mid, tapes=1, alpha=(3)
.param u, ('_', '0', '1')
.routine sub, tapes=1, alpha=(3), exits=2
.param n, ('_', '0', '1')
.section code
.func main
        call    mid
        stp
.func mid
        call    sub [0] exits=(won, lost)
        stp
won:    wr      [1]
        stp
lost:   wr      [2]
        stp
.func sub
        retx    #0
";

/// The same program whose site supplies ONE exit against a callee that
/// declares two.
const WRONG_COUNT: &str = "\
.routine main, tapes=1, alpha=(3)
.param t, ('_', '0', '1')
.routine sub, tapes=1, alpha=(3), exits=2
.param n, ('_', '0', '1')
.section code
.func main
        call    sub [0] exits=(won)
        stp
won:    wr      [1]
        stp
.func sub
        retx    #0
";

/// The same program whose site supplies NO exits at all against a callee
/// that declares two — the arity hole a check gated on "the site spells a
/// vector" would let through.
const ZERO_EXITS: &str = "\
.routine main, tapes=1, alpha=(3)
.param t, ('_', '0', '1')
.routine sub, tapes=1, alpha=(3), exits=2
.param n, ('_', '0', '1')
.section code
.func main
        call    sub [0]
        stp
.func sub
        retx    #0
";

/// Two bound sites in ONE function, composing to the very same composite
/// (the same callee, the same identity binding) and differing ONLY in the
/// exit labels they name. Without the exits in the engine's intern key the
/// two sites dedup onto one directory entry, and the second site's exits
/// are lost.
const TWO_SITES_SAME_COMPOSITE: &str = "\
.routine main, tapes=1, alpha=(3)
.param t, ('_', '0', '1')
.routine mid, tapes=1, alpha=(3)
.param u, ('_', '0', '1')
.routine sub, tapes=1, alpha=(3), exits=1
.param n, ('_', '0', '1')
.section code
.func main
        call    mid
        stp
.func mid
        call    sub [0] exits=(a)
        call    sub [0] exits=(b)
        stp
a:      wr      [1]
        stp
b:      wr      [2]
        stp
.func sub
        retx    #0
";

/// An exit-bearing site whose callee returns through a PLAIN `ret` — no
/// `retx` anywhere. This is the silent-drop shape: mono's pre-existing
/// refusals all fire on instructions in the copied body, and a body like
/// this carries none of them, so only an explicit site check catches it.
const RET_ONLY_CALLEE: &str = "\
.routine main, tapes=1, alpha=(3)
.param t, ('_', '0', '1')
.routine sub, tapes=1, alpha=(3), exits=1
.param n, ('_', '0', '1')
.section code
.func main
        call    sub [0] exits=(won)
back:   stp
won:    wr      [1]
        stp
.func sub
        ret
";

/// A MIXED image for hybrid: one holey site (`narrow`'s alphabet is
/// smaller, so the binding is not a bijection and stays on the frames
/// path) plus one exit-bearing bijection site. The holey site is what
/// makes `any_frames` true, so hybrid takes its mixed path instead of
/// the `!any_frames` fast path that delegates wholesale to `lower_mono`
/// — which is the only way the classifier's own check is reached.
/// `sub`'s body is a plain `ret`, so nothing in the copied body would
/// refuse it either.
const MIXED_WITH_EXITS: &str = "\
.routine main, tapes=1, alpha=(3)
.param t, ('_', '0', '1')
.routine narrow, tapes=1, alpha=(2)
.param m, ('_', '0')
.routine sub, tapes=1, alpha=(3), exits=1
.param n, ('_', '0', '1')
.section code
.func main
        call    narrow [0]
        call    sub [0] exits=(won)
        stp
won:    wr      [1]
        stp
.func narrow
        ret
.func sub
        ret
";

/// An exit-bearing site NESTED inside a routine that is itself being
/// stamped: `main`'s swap into `outer` is a bijection and no exit vector,
/// so the seed loop waves it through; `outer`'s own call into `inner`
/// carries exits and is seen only by the stamp closure. `inner`'s body is
/// a plain `ret`, so nothing in the copied body refuses it either.
const NESTED_WITH_EXITS: &str = "\
.routine main, tapes=1, alpha=(3)
.param t, ('_', '0', '1')
.routine outer, tapes=1, alpha=(3)
.param o, ('_', '0', '1')
.routine inner, tapes=1, alpha=(3), exits=1
.param i, ('_', '0', '1')
.section code
.func main
        call    outer [0{1->2, 2->1}]
        stp
.func outer
        call    inner [0] exits=(k)
        ret
k:      wr      [1]
        ret
.func inner
        ret
";

/// An exit-bearing site whose binding is the full identity — the exact
/// shape that WOULD collapse to a plain call without P3.
const IDENTITY_WITH_EXITS: &str = "\
.routine main, tapes=1, alpha=(3)
.param t, ('_', '0', '1')
.routine sub, tapes=1, alpha=(3), exits=1
.param n, ('_', '0', '1')
.section code
.func main
        call    sub [0] exits=(won)
        stp
won:    wr      [1]
        stp
.func sub
        retx    #0
";

/// Two sites into the SAME routine under the same (identity) composite,
/// naming DIFFERENT exits. They must not share a copy: each copy's `retx`
/// jumps to its own site's exit.
const TWO_SITES: &str = "\
.routine main, tapes=1, alpha=(3)
.param t, ('_', '0', '1')
.routine sub, tapes=1, alpha=(3), exits=1
.param n, ('_', '0', '1')
.section code
.func main
        call    sub [0] exits=(a)
        call    sub [0] exits=(b)
        stp
a:      wr      [1]
        stp
b:      wr      [2]
        stp
.func sub
        retx    #0
";

/// Two sites into the same routine naming the SAME exit label — so the
/// exit vectors are equal and only the CONTINUATION differs. `sub` comes
/// back through a plain `ret`, which lands on the instruction after its
/// own call, so the two copies must still be distinct.
const TWO_SITES_SAME_EXIT: &str = "\
.routine main, tapes=1, alpha=(3)
.param t, ('_', '0', '1')
.routine sub, tapes=1, alpha=(3), exits=1
.param n, ('_', '0', '1')
.section code
.func main
        call    sub [0] exits=(won)
        call    sub [0] exits=(won)
        stp
won:    wr      [1]
        stp
.func sub
        ret
";

/// A splice whose caller's own index MOVES when the prune runs. `main`
/// stamps `dead` (a swap binding, so never a collapse) and thereby orphans
/// it; `mid` — the splice's caller — then slides down past the hole.
/// `sub` is orphaned too, the moment its only site is retargeted.
///
/// Both of `main`'s sites are BOUND, and `mid`'s is the identity
/// pass-through that collapses to a plain call: name resolution walks
/// relocations before bound calls, so this is what puts the orphan-to-be
/// at a LOWER index than the splice's caller. With the two swapped, or
/// with `mid` reached by a plain call, `mid` lands at index 1 and the
/// reindex is a no-op — the fixture would pin nothing.
const ORPHANS: &str = "\
.routine main, tapes=1, alpha=(3)
.param t, ('_', '0', '1')
.routine dead, tapes=1, alpha=(3)
.param d, ('_', '0', '1')
.routine mid, tapes=1, alpha=(3)
.param u, ('_', '0', '1')
.routine sub, tapes=1, alpha=(3), exits=1
.param n, ('_', '0', '1')
.section code
.func main
        call    dead [0{1->2, 2->1}]
        call    mid [0]
        stp
.func dead
        ret
.func mid
        call    sub [0] exits=(won)
        stp
won:    wr      [1]
        stp
.func sub
        retx    #0
";

/// One caller holding BOTH a framed holey site and a spliced exit-bearing
/// one, in that order — so the frames path's 5 → 9 widening of the first
/// shifts every offset the splice names. `holey`'s alphabet is narrower
/// than the machine's, which is what keeps it off the mono path.
const MIXED_SPLICE_AND_FRAME: &str = "\
.routine main, tapes=1, alpha=(5)
.param t, ('_', 'a', 'b', 'c', 'd')
.routine holey, tapes=1, alpha=(3)
.param m, ('_', 'a', 'b')
.routine pick, tapes=1, alpha=(5), exits=1
.param n, ('_', 'a', 'b', 'c', 'd')
.section code
.func main
        call    holey [0{1->1, 2->2}]
        call    pick [0] exits=(won)
        stp
won:    wr      [1]
        stp
.func holey
        ret
.func pick
        retx    #0
";

/// A nested exit-bearing site whose composite is the full identity: `main`
/// swaps into `outer`, `outer` swaps back into `inner`, so the composed
/// binding is a genuine pass-through of the machine's own tapes. Without
/// P3's conjunct at the stamp closure this site collapses to a plain call
/// into the GENERIC `inner` and its exits vanish.
const NESTED_PASSTHROUGH_WITH_EXITS: &str = "\
.routine main, tapes=1, alpha=(3)
.param t, ('_', '0', '1')
.routine outer, tapes=1, alpha=(3)
.param o, ('_', '0', '1')
.routine inner, tapes=1, alpha=(3), exits=1
.param i, ('_', '0', '1')
.section code
.func main
        call    outer [0{1->2, 2->1}]
        stp
.func outer
        call    inner [0{1->2, 2->1}] exits=(k)
        ret
k:      wr      [1]
        ret
.func inner
        retx    #0
";

/// The absolute targets of every far `jmp` in `[start, end)` of the
/// image's code. The sweep is linear, so it can read an operand byte as an
/// opcode and add a spurious entry — harmless, because every assertion
/// below asks whether a WANTED address is among the targets, never that it
/// is the only one.
fn jump_targets(code: &[u8], start: u32, end: u32) -> Vec<u32> {
    let jmp = fake_syntax()
        .jump_opcode()
        .expect("the fake dialect has exactly one far jump");
    let mut landed = Vec::new();
    let mut at = start as usize;
    while at + 5 <= end as usize {
        if code[at] == jmp {
            let disp = i32::from_le_bytes(code[at + 1..at + 5].try_into().unwrap());
            landed.push((at as i64 + 5 + i64::from(disp)) as u32);
            at += 5;
        } else {
            at += 1;
        }
    }
    landed
}

/// One sidecar function, by name.
fn func<'a>(
    out: &'a mtc_core::linker::LinkOutput,
    name: &str,
) -> &'a mtc_core::linker::MapFunction {
    out.map
        .functions
        .iter()
        .find(|f| f.name == name)
        .unwrap_or_else(|| panic!("no `{name}` in the sidecar"))
}

/// The one splice copy of `routine` — the sidecar function whose name is
/// `<routine>.<digest8>`, possibly numbered.
fn copy_of<'a>(
    out: &'a mtc_core::linker::LinkOutput,
    routine: &str,
) -> &'a mtc_core::linker::MapFunction {
    let prefix = format!("{routine}.");
    out.map
        .functions
        .iter()
        .find(|f| f.name.starts_with(&prefix))
        .unwrap_or_else(|| panic!("no copy of `{routine}` in the sidecar"))
}

/// A label's absolute address, from the sidecar.
fn label_addr(f: &mtc_core::linker::MapFunction, name: &str) -> u32 {
    f.labels
        .iter()
        .find(|(n, _)| n == name)
        .map(|(_, a)| *a)
        .unwrap_or_else(|| panic!("no `{name}` in `{}`'s labels", f.name))
}

/// Mutation it catches: drop the `exits.is_empty()` conjunct at
/// `scan_sites` and this site collapses to a plain call — the image then
/// carries no frames region at all and `composites` is 0.
#[test]
fn an_identity_binding_with_exits_does_not_collapse() {
    let out = link(
        &fake_syntax(),
        &[asm(IDENTITY_WITH_EXITS)],
        &[],
        opts(CallMech::Frames),
    )
    .expect("links under frames");
    assert!(
        out.report.composites > 0,
        "an exit-bearing identity binding must stay framed: {:?}",
        out.report
    );
}

/// The count check is the resolution pre-pass's, which runs ahead of the
/// point the three mechanisms diverge — so it must refuse under every one
/// of them, not only the mechanism that would have carried the vector.
///
/// Mutation it catches: skip the exit-count check and a site that
/// supplies too few exits links, leaving `retx #1` to read past the
/// vector at run time.
#[test]
fn a_wrong_exit_count_is_refused_under_every_mechanism() {
    for mech in MECHS {
        let err = link(&fake_syntax(), &[asm(WRONG_COUNT)], &[], opts(mech))
            .expect_err("a short exit vector must be refused");
        assert!(
            matches!(&err, LinkError::BadBinding { message, .. }
                if message.contains("supplies 1 exit(s), but `sub` declares 2")),
            "under {mech}: {err:?}"
        );
    }
}

/// A site supplying NO exits into a callee that declares some is the same
/// arity error, and reads as one: the two arms share a format string. It
/// is not a symbolic form at all, so the check cannot hang off "the site
/// spells a vector" — the callee's declared count is what selects it.
///
/// Mutation it catches: gate the check on `!record.exits.is_empty()` (or
/// leave `declared == 0` out of the early-out) and this program links,
/// leaving `sub`'s `retx #0` to index a descriptor declaring no exits.
#[test]
fn a_site_supplying_no_exits_into_an_exit_bearing_callee_is_refused() {
    for mech in MECHS {
        let err = link(&fake_syntax(), &[asm(ZERO_EXITS)], &[], opts(mech))
            .expect_err("a missing exit vector must be refused");
        assert!(
            matches!(&err, LinkError::BadBinding { message, .. }
                if message.contains("supplies 0 exit(s), but `sub` declares 2")),
            "under {mech}: {err:?}"
        );
    }
}

/// An exit vector under MONO (and under hybrid, which delegates a
/// bijection wholesale to mono) is CARRIED: the site is lowered as a jump
/// into a per-site copy whose returns are jumps back into the caller. Each
/// fixture must link, and a copy of the exit-bearing callee must appear in
/// the image — a plain call into the generic would mean the exits were
/// dropped.
///
/// Each fixture reaches a DIFFERENT one of the three places a mono path
/// commits to copying a site, which is why all four are here:
/// `IDENTITY_WITH_EXITS` and `RET_ONLY_CALLEE` reach the seed loop (and
/// hybrid reaches it too — with every site classified to mono it takes
/// its `!any_frames` fast path and delegates wholesale to `lower_mono`);
/// `MIXED_WITH_EXITS` is the only one that reaches hybrid's OWN
/// classifier; `NESTED_WITH_EXITS` is the only one that reaches the stamp
/// closure's bound arm, where the caller is itself a copy.
///
/// `RET_ONLY_CALLEE` is the shape with no `retx` anywhere: only the `ret`
/// rewrite carries its exit-bearing site, so a splice that rewrote just
/// `retx` would leave it returning through an address nobody pushed.
///
/// Mutation it catches: leave the site out of the stamp key (or off the
/// seed altogether) and the copy is an ordinary stamp again — under
/// `NESTED_WITH_EXITS` it is not even minted, because the closure's bound
/// arm would collapse or refuse instead.
#[test]
fn an_exit_vector_links_under_mono_and_hybrid() {
    let cases = [
        (IDENTITY_WITH_EXITS, "sub"),
        (RET_ONLY_CALLEE, "sub"),
        (MIXED_WITH_EXITS, "sub"),
        (NESTED_WITH_EXITS, "inner"),
    ];
    for (src, name) in cases {
        for mech in [CallMech::Mono, CallMech::Hybrid] {
            let out = link(&fake_syntax(), &[asm(src)], &[], opts(mech))
                .unwrap_or_else(|e| panic!("under {mech} into `{name}`: {e}"));
            assert!(
                out.map
                    .functions
                    .iter()
                    .any(|f| f.name.starts_with(&format!("{name}."))),
                "under {mech}: no per-site copy of `{name}`: {:?}",
                out.map
                    .functions
                    .iter()
                    .map(|f| &f.name)
                    .collect::<Vec<_>>()
            );
        }
    }
}

/// Under MONO an exit-bearing site is a splice: the caller jumps into a
/// per-site copy whose `retx #k` becomes a jump to exit `k` and whose
/// `ret` becomes a jump to the instruction after the call. The image runs
/// on the base profile — no frames region at all.
///
/// Mutation it catches: leave `build_stamp`'s `MonoRawFrame` refusal on
/// `Imm8 + Flow::Stop` unconditional and this link fails; emit the frames
/// region anyway and `composites` stops being 0.
#[test]
fn an_exit_bearing_site_splices_under_mono() {
    let out = link(&fake_syntax(), &[asm(TWO_EXITS)], &[], opts(CallMech::Mono))
        .expect("an exit-bearing site must splice under mono");
    assert_eq!(
        out.report.composites, 0,
        "a mono image carries no frames region: {:?}",
        out.report
    );
    assert!(
        out.report.instantiations >= 1,
        "the site must produce a copy: {:?}",
        out.report
    );
}

/// The `ret` rewrite, positionally: `sub` comes back through a plain
/// `ret`, and the copy is entered by a jump, so that `ret` must become a
/// jump to `back` — the instruction after the call site.
///
/// Mutation it catches: drop the `OperandKind::None` rewrite and the copy
/// keeps its `ret`, returning through a return address nobody pushed —
/// the copy then holds no far jump at all and `back` is nowhere among its
/// targets.
#[test]
fn a_splice_returns_to_the_instruction_after_the_call() {
    let obj = assemble(&fake_syntax(), ARCH, RET_ONLY_CALLEE, true).expect("assembles with -g");
    let out = link(
        &fake_syntax(),
        std::slice::from_ref(&obj),
        &[],
        opts(CallMech::Mono),
    )
    .expect("links under mono");
    let main = func(&out, "main");
    let copy = copy_of(&out, "sub");
    let landed = jump_targets(&out.executable.code, copy.start, copy.end);
    assert!(
        landed.contains(&label_addr(main, "back")),
        "the splice returns to {landed:?}, not to `back` ({})",
        label_addr(main, "back")
    );
    // And the site ENTERS the copy by a jump: a `call` would push a return
    // address the rewritten `ret` above never consumes.
    assert!(
        jump_targets(&out.executable.code, main.start, main.end).contains(&copy.start),
        "the caller must jump into the copy at {}",
        copy.start
    );
}

/// Two sites into the same routine with DIFFERENT exits must not share a
/// copy: each copy's `retx` jumps to its own site's exit.
///
/// Mutation it catches: leave the SITE out of the stamp key entirely and
/// the two sites dedup onto one copy, whose `retx` jumps to whichever
/// site's exits interned first.
///
/// It does NOT isolate the `exits` component of that key, and no test
/// can: `(caller, then)` already identifies a site uniquely — `then` is
/// `addr + 5` — so the exits ride along as recorded-but-redundant. With
/// `then` alone removed this program still splits (the exit labels
/// differ); the companion test below is the one that pins `then`.
#[test]
fn two_sites_with_different_exits_get_different_copies() {
    let out = link(&fake_syntax(), &[asm(TWO_SITES)], &[], opts(CallMech::Mono))
        .expect("links under mono");
    assert_eq!(
        out.report.instantiations, 2,
        "two exit vectors, two copies: {:?}",
        out.report
    );
}

/// Two sites naming the SAME exit still need two copies, because they
/// return to different continuations.
///
/// Mutation it catches: leave `then` out of the stamp key and the two
/// sites dedup onto one copy, whose `ret` jumps back to whichever site
/// interned first.
#[test]
fn two_sites_with_the_same_exit_but_different_continuations_get_different_copies() {
    let out = link(
        &fake_syntax(),
        &[asm(TWO_SITES_SAME_EXIT)],
        &[],
        opts(CallMech::Mono),
    )
    .expect("links under mono");
    assert_eq!(
        out.report.instantiations, 2,
        "same exits, different continuations — two copies: {:?}",
        out.report
    );
}

/// A splice whose caller's index MOVES when the prune runs: `dead` is
/// orphaned by its own stamping, so `mid` slides from index 2 to 1, and
/// `sub` is orphaned by the retarget too.
///
/// Mutation it catches: leave `site_fixups` out of `prune_unreachable`'s
/// reindex and the splice's `retx` jump names whatever function slid into
/// the dropped index — a wrong-target jump nothing about `dropped` or the
/// function list would show. Decoding the jump is what exposes it.
#[test]
fn a_splice_survives_a_prune_that_reindexes_its_caller() {
    let obj = assemble(&fake_syntax(), ARCH, ORPHANS, true).expect("assembles with -g");
    let out = link(
        &fake_syntax(),
        std::slice::from_ref(&obj),
        &[],
        opts(CallMech::Mono),
    )
    .expect("links under mono");
    for name in ["dead", "sub"] {
        assert!(
            out.report.dropped.contains(&name.to_string()),
            "`{name}` must be pruned: {:?}",
            out.report.dropped
        );
    }
    let mid = func(&out, "mid");
    let copy = copy_of(&out, "sub");
    let want = label_addr(mid, "won");
    let landed = jump_targets(&out.executable.code, copy.start, copy.end);
    assert!(
        landed.contains(&want),
        "the splice's jump lands at {landed:?}, not at `won` ({want})"
    );
}

/// A caller holding BOTH a spliced exit-bearing site and a framed holey
/// one: under hybrid the frames path widens the framed site 5 → 9 bytes,
/// shifting every later offset in that blob — including the splice's
/// `then` and its exits.
///
/// Mutation it catches: leave `splice_shift` out (use the raw record
/// offsets) and the fixup's lookup misses the post-rewrite instruction
/// boundary entirely, so the link fails; make it shift the wrong way and
/// the decoded jump lands somewhere other than `won`.
#[test]
fn a_spliced_site_and_a_framed_site_in_one_caller_agree_under_hybrid() {
    for mech in MECHS {
        link(
            &fake_syntax(),
            &[asm(MIXED_SPLICE_AND_FRAME)],
            &[],
            opts(mech),
        )
        .unwrap_or_else(|e| panic!("the mixed program must link under {mech}: {e}"));
    }
    let obj =
        assemble(&fake_syntax(), ARCH, MIXED_SPLICE_AND_FRAME, true).expect("assembles with -g");
    let out = link(
        &fake_syntax(),
        std::slice::from_ref(&obj),
        &[],
        opts(CallMech::Hybrid),
    )
    .expect("links under hybrid");
    let main = func(&out, "main");
    let copy = copy_of(&out, "pick");
    let want = label_addr(main, "won");
    let landed = jump_targets(&out.executable.code, copy.start, copy.end);
    assert!(
        landed.contains(&want),
        "the splice's exit jump lands at {landed:?}, not at `won` ({want})"
    );
    // The hybrid promotion loop changes the site's opcode too.
    assert!(
        jump_targets(&out.executable.code, main.start, main.end).contains(&copy.start),
        "the caller must jump into the copy at {}",
        copy.start
    );
}

/// A NESTED exit-bearing site whose composite is the full identity is
/// still a splice — the one shape P3's conjunct at the stamp closure
/// exists for.
///
/// Mutation it catches: drop `record.exits.is_empty() &&` from the
/// closure's collapse condition and this site becomes a plain call into
/// the GENERIC `inner`, so no copy of `inner` is minted at all and its
/// exits are gone. (The same conjunct in `scan_sites` is pinned
/// separately by `an_identity_binding_with_exits_does_not_collapse`;
/// nothing else reaches this one, since the closure is the only place a
/// nested site appears.)
#[test]
fn a_nested_pass_through_site_with_exits_still_splices() {
    for mech in [CallMech::Mono, CallMech::Hybrid] {
        let out = link(
            &fake_syntax(),
            &[asm(NESTED_PASSTHROUGH_WITH_EXITS)],
            &[],
            opts(mech),
        )
        .unwrap_or_else(|e| panic!("must link under {mech}: {e}"));
        assert!(
            out.map
                .functions
                .iter()
                .any(|f| f.name.starts_with("inner.")),
            "under {mech}: the nested site must splice, not collapse: {:?}",
            out.map
                .functions
                .iter()
                .map(|f| &f.name)
                .collect::<Vec<_>>()
        );
        // The nested site is entered by a jump from the COPY of `outer`,
        // never from the generic — the generic is orphaned the moment its
        // own site is retargeted, so a splice returning into it would
        // return into code the image does not carry.
        let outer = copy_of(&out, "outer");
        let inner = copy_of(&out, "inner");
        assert!(
            jump_targets(&out.executable.code, outer.start, outer.end).contains(&inner.start),
            "under {mech}: `{}` must jump into `{}` at {}",
            outer.name,
            inner.name,
            inner.start
        );
        // And the copy returns into the COPY of `outer`, at `outer`'s own
        // continuation and exit — both inside that copy's range.
        for target in jump_targets(&out.executable.code, inner.start, inner.end) {
            assert!(
                (outer.start..outer.end).contains(&target),
                "under {mech}: the splice returns to {target}, outside `{}` ({}..{})",
                outer.name,
                outer.start,
                outer.end
            );
        }
    }
}

/// Two sites composing to the SAME composite but naming different exits
/// need two directory entries: the exit vector is part of the descriptor,
/// so the engine's intern key must distinguish them even though their
/// canonical composite keys are equal.
///
/// The expected addresses come from the sidecar, whose own agreement with
/// the `+4`-per-widened-site shift is pinned independently by
/// `the_frames_descriptor_exits_are_the_exact_absolute_addresses`.
///
/// Mutation it catches: revert the `key.extend_from_slice` block in
/// `intern_composite` and the two sites dedup onto ONE entry — `K` drops
/// to 1 and the second site's exits vanish from the image entirely.
#[test]
fn two_sites_with_the_same_composite_and_different_exits_do_not_dedup() {
    // `-g`, so the sidecar carries `mid`'s label addresses.
    let obj =
        assemble(&fake_syntax(), ARCH, TWO_SITES_SAME_COMPOSITE, true).expect("assembles with -g");
    let out = link(
        &fake_syntax(),
        std::slice::from_ref(&obj),
        &[],
        opts(CallMech::Frames),
    )
    .expect("links under frames");
    let mid = out
        .map
        .functions
        .iter()
        .find(|f| f.name == "mid")
        .expect("`mid` is in the sidecar");
    let addr_of = |name: &str| -> u32 {
        mid.labels
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, a)| *a)
            .unwrap_or_else(|| panic!("no `{name}` in the sidecar"))
    };
    let dir = directory(&out.executable);
    assert_eq!(
        dir.len(),
        2,
        "two sites, two descriptors — the exits are part of the key: {dir:?}"
    );
    // Directory order is intern order, which is the sites' own blob order.
    assert_eq!(
        descriptor_exits(&out.executable, dir[0]),
        vec![addr_of("a")],
        "the first site's exit vector"
    );
    assert_eq!(
        descriptor_exits(&out.executable, dir[1]),
        vec![addr_of("b")],
        "the second site's exit vector"
    );
}

/// The descriptor's exit words must be the EXACT absolute addresses of
/// `won` and `lost`, in that order — not the blob-relative placeholders
/// the engine wrote.
///
/// The expected values are DERIVED, not transcribed: assemble with `-g`
/// so the object carries each label's original blob offset, then
/// `absolute = mid.start + raw + 4`, the `+ 4` being the one widened
/// bound site (`mid`'s only bound call, at offset 1) that precedes both
/// labels. The sidecar's own label addresses must agree with that
/// arithmetic, which pins the shift independently of the descriptor.
///
/// The vector is decoded through the descriptor's own header, not
/// searched for in the image: a byte search is satisfied by the right
/// numbers landing anywhere, including on top of a map.
///
/// Mutation it catches: skip the rebase in `emit_planned_region` and the
/// image carries the shifted BLOB offsets instead — small numbers, which
/// the assertions explicitly forbid. A range check would not catch it,
/// since a small offset can also fall inside `mid`'s range. Drop the
/// exits on the way into `materialize` and the descriptor declares no
/// exit vector at all, which the decode reports as an empty one.
#[test]
fn the_frames_descriptor_exits_are_the_exact_absolute_addresses() {
    let obj = assemble(&fake_syntax(), ARCH, TWO_EXITS, true).expect("assembles with -g");
    let raw_of = |name: &str| -> u32 {
        obj.debug
            .as_ref()
            .expect("-g")
            .iter()
            .flat_map(|b| b.labels.iter())
            .find(|(n, _)| n == name)
            .map(|(_, off)| *off)
            .unwrap_or_else(|| panic!("no label `{name}` in the object"))
    };
    let out = link(
        &fake_syntax(),
        std::slice::from_ref(&obj),
        &[],
        opts(CallMech::Frames),
    )
    .expect("links under frames");
    let mid = out
        .map
        .functions
        .iter()
        .find(|f| f.name == "mid")
        .expect("`mid` is in the sidecar");
    // The interposed function really does move `mid` off offset 0, which
    // is what makes the un-rebased placeholders distinguishable below.
    assert!(mid.start > 0, "`mid` must not be laid at code offset 0");
    let want_addr = |name: &str| mid.start + raw_of(name) + 4;
    // The sidecar agrees with the arithmetic: the shift is +4, once.
    for name in ["won", "lost"] {
        let sidecar = mid
            .labels
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, a)| *a)
            .unwrap_or_else(|| panic!("no `{name}` in the sidecar"));
        assert_eq!(sidecar, want_addr(name), "`{name}`'s address");
    }
    // The one directory entry's descriptor carries exactly those two
    // words, in vector order — read out of its own exit field.
    let dir = directory(&out.executable);
    assert_eq!(dir.len(), 1, "one composite: {dir:?}");
    let exits = descriptor_exits(&out.executable, dir[0]);
    assert_eq!(
        exits,
        vec![want_addr("won"), want_addr("lost")],
        "the descriptor's exit vector"
    );
    // And the UN-rebased placeholders are nowhere in the image.
    let bytes = out.executable.to_bytes();
    let bad: Vec<u8> = [raw_of("won") + 4, raw_of("lost") + 4]
        .iter()
        .flat_map(|a| a.to_le_bytes())
        .collect();
    assert!(
        !bytes.windows(bad.len()).any(|w| w == bad),
        "the exit vector was never rebased"
    );
}
