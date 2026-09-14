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
        stp
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

/// Until the jump-entered copies land, an exit vector under MONO (and
/// under hybrid, which delegates a bijection wholesale to mono) is
/// refused explicitly — never carried, and never silently dropped. Both
/// fixtures are bijections, so hybrid classifies them to the mono path.
///
/// `RET_ONLY_CALLEE` is the case that needs the site check: mono's other
/// refusals (`MonoRawFrame` on a multi-exit return, on a raw `call.m`)
/// all fire on an instruction in the copied body, and a `ret`-only body
/// has none of them — without this check that program links with its
/// exits gone.
///
/// The jump-entered copies flip this: when mono can splice a per-site
/// copy, these two links succeed and this test becomes a value test.
///
/// Each fixture reaches a DIFFERENT one of the three places a mono path
/// commits to copying a site, which is why all four are here:
/// `IDENTITY_WITH_EXITS` and `RET_ONLY_CALLEE` reach the seed loop (and
/// hybrid reaches it too — with every site classified to mono it takes
/// its `!any_frames` fast path and delegates wholesale to `lower_mono`);
/// `MIXED_WITH_EXITS` is the only one that reaches hybrid's OWN
/// classifier; `NESTED_WITH_EXITS` is the only one that reaches the stamp
/// closure's bound arm.
///
/// Mutation it catches: drop the `refuse_exits_under_mono` call from the
/// mono seed loop and `RET_ONLY_CALLEE` links under `Mono`; drop it from
/// hybrid's classifier and `MIXED_WITH_EXITS` links under `Hybrid`; drop
/// it from the stamp closure and `NESTED_WITH_EXITS` links under both.
/// Dropping the `exits.is_empty()` conjunct in `scan_sites` also fires it
/// — the site then collapses to a plain call and the seed loop, which
/// only sees non-collapsing sites, never gets to refuse.
#[test]
fn an_exit_vector_is_refused_under_mono_and_hybrid() {
    let cases = [
        (IDENTITY_WITH_EXITS, "sub"),
        (RET_ONLY_CALLEE, "sub"),
        (MIXED_WITH_EXITS, "sub"),
        (NESTED_WITH_EXITS, "inner"),
    ];
    for (src, name) in cases {
        for mech in [CallMech::Mono, CallMech::Hybrid] {
            let err = link(&fake_syntax(), &[asm(src)], &[], opts(mech))
                .expect_err("an exit vector must be refused on a mono path");
            assert!(
                matches!(&err, LinkError::BadBinding { callee, message }
                    if callee == name
                        && message.contains("carries an exit vector")
                        && message.contains("--call-mech=frames")),
                "under {mech} into `{name}`: {err:?}"
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
