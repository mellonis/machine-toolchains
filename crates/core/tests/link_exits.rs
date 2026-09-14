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

/// Mutation it catches: skip the exit-count check and a site that
/// supplies too few exits links, leaving `retx #1` to read past the
/// vector at run time.
#[test]
fn a_wrong_exit_count_is_refused() {
    let err = link(
        &fake_syntax(),
        &[asm(WRONG_COUNT)],
        &[],
        opts(CallMech::Frames),
    )
    .expect_err("a short exit vector must be refused");
    assert!(
        matches!(&err, LinkError::BadBinding { message, .. }
            if message.contains("supplies 1 exit(s), but `sub` declares 2")),
        "{err:?}"
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
