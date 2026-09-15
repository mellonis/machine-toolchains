//! Exit vectors on a declarative bound call (docs/core.md (call
//! mechanisms)), on a neutral fake dialect. Frames carries them in the
//! descriptor; mono splices a per-site copy; hybrid folds.
//!
//! Everything runs on a per-file fake dialect (per-file-helper
//! convention), so core stays provably arch-agnostic.

use mtc_core::asm::{ArchSyntax, AsmCaps, Flow, RelaxPair, SyntaxEntry, assemble};
use mtc_core::formats::object::ObjectFile;
use mtc_core::linker::{CallMech, LinkError, LinkOptions, LinkOutput, link};
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
            // A match table (a pure lookup) and the dispatch that consumes
            // its result: a callee carrying one has a non-empty table blob,
            // which is the half of `B` a fold decision would miss if it
            // counted the code alone.
            SyntaxEntry {
                opcode: 0x11,
                mnemonic: "tmatch",
                operand: OperandKind::TableRef,
                flow: FT,
            },
            SyntaxEntry {
                opcode: 0x12,
                mnemonic: "tdispatch",
                operand: OperandKind::TableRef,
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

/// The frames region's COMPOSE MATRIX, decoded positionally from the
/// region header rather than searched for (docs/formats.md (frames
/// region)): `K u16`, `S u16`, `K × u32` directory, then `(K + 1) × S`
/// little-endian `u16`s — row = active frame `0..=K`, column = framed-call
/// site in emission order.
fn compose_matrix(exe: &mtc_core::formats::executable::Executable) -> Vec<Vec<u16>> {
    assert_ne!(exe.frames_offset, 0, "no frames region in the image");
    let t = &exe.tables;
    let mut p = exe.frames_offset as usize;
    let k = usize::from(u16::from_le_bytes([t[p], t[p + 1]]));
    let s = usize::from(u16::from_le_bytes([t[p + 2], t[p + 3]]));
    p += 4 + 4 * k;
    (0..=k)
        .map(|_| {
            (0..s)
                .map(|_| {
                    let v = u16::from_le_bytes([t[p], t[p + 1]]);
                    p += 2;
                    v
                })
                .collect()
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

/// A framed bound site NESTED inside an EXIT-BEARING framed callee.
/// `main`'s swap into `mid` carries an exit, so under frames `mid` runs
/// under a composite whose directory entry is interned with that site's
/// exit vector in its key — and `mid`'s own exit-bearing call into `big`
/// must compose in THAT composite's row, not the identity row.
///
/// Two framed sites, in emission order: `main`'s (column 0) and `mid`'s
/// (column 1); two composites, `mid`'s (row 1) and `big`'s (row 2).
const NESTED_UNDER_EXIT_BEARING: &str = "\
.routine main, tapes=1, alpha=(3)
.param t, ('_', '0', '1')
.routine mid, tapes=1, alpha=(3), exits=1
.param m, ('_', '0', '1')
.routine big, tapes=1, alpha=(3), exits=1
.param n, ('_', '0', '1')
.section code
.func main
        call    mid [0{1->2, 2->1}] exits=(p)
        stp
p:      stp
.func mid
        call    big [0] exits=(r)
        retx    #0
r:      retx    #0
.func big
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

/// A callee whose BODY disagrees with its own declared exit count: it
/// declares `exits=1` and returns through `retx #1`. Ruling 19 pins the
/// site's vector to the declaration, so the splice is the only place this
/// can surface — nothing validates a routine's body against its interface.
const BODY_OVERRUNS_ITS_EXITS: &str = "\
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
        retx    #1
";

/// A spliced callee whose body holds a `ret` AND a `stp`. Both are
/// `OperandKind::None` + `Flow::Stop`, so only the dialect's declared
/// `return_opcode` tells them apart: exactly one of the two becomes a
/// jump, and the `stp` survives verbatim.
const RET_THEN_STP: &str = "\
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
        stp
";

/// Two callers with BYTE-IDENTICAL shapes, each bound-calling `sub` with
/// one exit at the same offset. Their sites therefore share a composite,
/// a `then` (6) and an exit offset (7) — everything in the stamp key
/// except which function they sit in. `sub` comes back through `retx #0`,
/// so each copy's one jump lands on ITS caller's exit label, which is what
/// makes the two distinguishable in the image.
const TWO_CALLERS: &str = "\
.routine main, tapes=1, alpha=(3)
.param t, ('_', '0', '1')
.routine a, tapes=1, alpha=(3)
.param p, ('_', '0', '1')
.routine b, tapes=1, alpha=(3)
.param q, ('_', '0', '1')
.routine sub, tapes=1, alpha=(3), exits=1
.param n, ('_', '0', '1')
.section code
.func main
        call    a
        call    b
        stp
.func a
        call    sub [0] exits=(la)
        stp
la:     wr      [1]
        stp
.func b
        call    sub [0] exits=(lb)
        stp
lb:     wr      [1]
        stp
.func sub
        retx    #0
";

/// A nested exit-bearing site whose CALLER COPY shifts. `outer` runs on
/// one tape inside a two-tape machine, so its `wr` re-emits one byte wider
/// in the copy — and that `wr` sits AHEAD of the call, so every later
/// offset in the copy is one past its offset in the generic. Only a
/// coordinate translation through the copy's own offset map lands the
/// splice on the right instruction.
const NESTED_SHIFTED: &str = "\
.routine main, tapes=2, alpha=(3, 3)
.param t, ('_', '0', '1')
.param u, ('_', '0', '1')
.routine outer, tapes=1, alpha=(3)
.param o, ('_', '0', '1')
.routine inner, tapes=1, alpha=(3), exits=1
.param i, ('_', '0', '1')
.section code
.func main
        call    outer [0]
        stp
.func outer
        wr      [1]
        call    inner [0] exits=(k)
        ret
k:      wr      [1]
        ret
.func inner
        retx    #0
";

/// A two-exit callee whose body reaches BOTH returns, so the copy mints
/// two jumps and their order pins `k` positionally: `retx #0` must land on
/// the first exit and `retx #1` on the second.
const BOTH_EXITS: &str = "\
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
        retx    #1
";

/// An exit-bearing site that is the LAST instruction of its function, so
/// the copy's `ret` would have no instruction to return to. `sub` comes
/// back through a plain `ret`, which is the return that actually has
/// nowhere to land.
const TAIL_POSITION: &str = "\
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
won:    wr      [1]
        call    sub [0] exits=(won)
.func sub
        ret
";

/// The same tail-position site, into a callee that CANNOT return: `sub`
/// declares `noreturn` and leaves only through `retx`. The copy rewrites
/// that into a jump to the site's own exit, so it needs no instruction
/// after the call and the site is legal on every mechanism.
const TAIL_POSITION_NORETURN: &str = "\
.routine main, tapes=1, alpha=(3)
.param t, ('_', '0', '1')
.routine mid, tapes=1, alpha=(3)
.param u, ('_', '0', '1')
.routine sub, tapes=1, alpha=(3), exits=1, noreturn
.param n, ('_', '0', '1')
.section code
.func main
        call    mid
        stp
.func mid
won:    wr      [1]
        call    sub [0] exits=(won)
.func sub
        retx    #0
";

/// A callee whose header says `noreturn` and whose BODY returns anyway.
/// Nothing in the assembler checks one against the other — the bit is a
/// declaration — so the linker reads the body, and this site keeps the
/// tail-position refusal.
const TAIL_POSITION_LYING_NORETURN: &str = "\
.routine main, tapes=1, alpha=(3)
.param t, ('_', '0', '1')
.routine mid, tapes=1, alpha=(3)
.param u, ('_', '0', '1')
.routine sub, tapes=1, alpha=(3), exits=1, noreturn
.param n, ('_', '0', '1')
.section code
.func main
        call    mid
        stp
.func mid
won:    wr      [1]
        call    sub [0] exits=(won)
.func sub
        ret
";

/// A tail-position site into a `noreturn` callee NESTED inside a routine
/// that is itself stamped: `main`'s swap into `outer` is an exit-free
/// bijection, so `outer` is copied, and `outer`'s own last instruction is
/// the exit-bearing call into `inner`. The splice's coordinates are then
/// the ORIGINAL `outer`'s, translated through the copy's offset map — and
/// the continuation one past the end of `outer` is in no such map, which
/// is why a site with no continuation must carry none rather than a
/// number.
const TAIL_POSITION_IN_STAMP: &str = "\
.routine main, tapes=1, alpha=(3)
.param t, ('_', '0', '1')
.routine outer, tapes=1, alpha=(3)
.param o, ('_', '0', '1')
.routine inner, tapes=1, alpha=(3), exits=1, noreturn
.param i, ('_', '0', '1')
.section code
.func main
        call    outer [0{1->2, 2->1}]
        stp
.func outer
k:      wr      [1]
        call    inner [0] exits=(k)
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

/// A body that returns through an exit its own interface does not declare
/// is refused where the splice would have to resolve it. The site's vector
/// is already pinned to the declared count by name resolution, so this is
/// the one place the disagreement can be seen at all.
///
/// Mutation it catches: index `s.exits` directly instead of taking the
/// `get(k)` refusal and the splice jumps to whatever follows the vector —
/// or panics — rather than naming the disagreement.
#[test]
fn a_body_returning_through_an_undeclared_exit_is_refused() {
    for mech in [CallMech::Mono, CallMech::Hybrid] {
        let err = link(
            &fake_syntax(),
            &[asm(BODY_OVERRUNS_ITS_EXITS)],
            &[],
            opts(mech),
        )
        .expect_err("a body overrunning its exit vector must be refused");
        assert!(
            matches!(&err, LinkError::BadBinding { callee, message }
                if callee == "sub"
                    && message.contains("the body returns through exit 1, but the call site \
                                         supplies 1 exit(s)")),
            "under {mech}: {err:?}"
        );
    }
}

/// `ret`, `stp` and `hlt` are all `OperandKind::None` + `Flow::Stop`, so
/// the copy can only tell the RETURN apart by the dialect's declared
/// `return_opcode`. `sub`'s body holds a `ret` and then a `stp`: exactly
/// one jump is minted, and the `stp` is copied verbatim.
///
/// Mutation it catches: rewrite on `entry.flow == Flow::Stop` instead of
/// `Some(entry.opcode) == syntax.return_opcode` and the `stp` becomes a
/// second jump back to the call site — two jumps, and a program that can
/// no longer stop.
#[test]
fn a_splice_rewrites_the_declared_return_and_nothing_else_that_stops() {
    let obj = assemble(&fake_syntax(), ARCH, RET_THEN_STP, true).expect("assembles with -g");
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
    assert_eq!(
        landed,
        vec![label_addr(main, "back")],
        "exactly one jump, to the call site's continuation"
    );
    // The `stp` survived: it is the copy's last byte, still the stop
    // opcode rather than the tail of a second jump's displacement.
    let stp = fake_syntax()
        .by_mnemonic("stp")
        .expect("the fake dialect has `stp`")
        .opcode;
    assert_eq!(
        out.executable.code[copy.end as usize - 1],
        stp,
        "the `stp` must be copied verbatim"
    );
}

/// Two callers of the same shape produce sites that agree on EVERYTHING
/// in the stamp key except which function they sit in: the same composite,
/// the same `then`, the same exit offset. They still need two copies,
/// because each returns into its own caller.
///
/// Mutation it catches: drop `caller` from the stamp key and the two sites
/// dedup onto one copy — `b` then jumps into a copy that returns into `a`,
/// at `a`'s exit label, which the per-caller decode below exposes.
#[test]
fn two_identical_callers_do_not_share_one_copy() {
    let obj = assemble(&fake_syntax(), ARCH, TWO_CALLERS, true).expect("assembles with -g");
    let out = link(
        &fake_syntax(),
        std::slice::from_ref(&obj),
        &[],
        opts(CallMech::Mono),
    )
    .expect("links under mono");
    assert_eq!(
        out.report.instantiations, 2,
        "two callers, two copies: {:?}",
        out.report
    );
    let code = &out.executable.code;
    let mut entered = Vec::new();
    for (caller, label) in [("a", "la"), ("b", "lb")] {
        let f = func(&out, caller);
        let targets = jump_targets(code, f.start, f.end);
        assert_eq!(
            targets.len(),
            1,
            "`{caller}` enters its copy by exactly one jump: {targets:?}"
        );
        let copy_start = targets[0];
        // That copy returns into THIS caller, at THIS caller's exit.
        let copy = out
            .map
            .functions
            .iter()
            .find(|c| c.start == copy_start)
            .unwrap_or_else(|| panic!("`{caller}` jumps to {copy_start}, which is no function"));
        assert!(
            copy.name.starts_with("sub."),
            "`{caller}` must jump into a copy of `sub`, not `{}`",
            copy.name
        );
        assert!(
            jump_targets(code, copy.start, copy.end).contains(&label_addr(f, label)),
            "`{}` must return to `{label}` in `{caller}`",
            copy.name
        );
        entered.push(copy_start);
    }
    assert_ne!(
        entered[0], entered[1],
        "the two callers must not share one copy"
    );
}

/// The nested coordinate translation, positionally. `outer`'s copy
/// re-emits its leading `wr [1]` at the machine's two-tape width — three
/// bytes where the generic had two — so every offset past it shifts by one:
/// the exit label `k`, at blob offset 9 in the generic, is at offset 10 in
/// the copy. That +1 is the whole point of translating through the copy's
/// own offset map.
///
/// Mutation it catches: skip the `xlat` in `resolve_site` and use the raw
/// generic offsets — the exit (9 in the generic) lands on the copy's `ret`
/// at offset 9 instead of on its `wr` at 10. The link still SUCCEEDS
/// (measured): `inner`'s body returns only through `retx #0`, so the
/// un-translated `then` (8, not an instruction boundary of the copy at
/// all) is never emitted as a fixup and never checked. A silently wrong
/// jump into a live instruction is exactly what the decode below is for.
#[test]
fn a_nested_splice_translates_its_offsets_into_the_caller_copy() {
    let out = link(
        &fake_syntax(),
        &[asm(NESTED_SHIFTED)],
        &[],
        opts(CallMech::Mono),
    )
    .expect("links under mono");
    let outer = copy_of(&out, "outer");
    let inner = copy_of(&out, "inner");
    let landed = jump_targets(&out.executable.code, inner.start, inner.end);
    let want = outer.start + 10;
    assert_eq!(
        landed,
        vec![want],
        "the exit must land on the copy's second `wr` at {want}, not on the \
         un-translated offset {}",
        outer.start + 9
    );
    // Self-describing: that address really is the widened `wr`.
    let wr = fake_syntax()
        .by_mnemonic("wr")
        .expect("the fake dialect has `wr`")
        .opcode;
    assert_eq!(
        out.executable.code[want as usize], wr,
        "the splice's exit must land on a `wr`"
    );
}

/// `retx #k` is indexed, and the index has to be honoured: `retx #0` lands
/// on the first exit, `retx #1` on the second. The copy mints one jump per
/// return, in body order, so the decoded pair is directly comparable.
///
/// Mutation it catches: index the site's vector with a constant (or read
/// `k` from the wrong operand) and both jumps land on `won`.
#[test]
fn retx_lands_on_the_exit_its_index_names() {
    let obj = assemble(&fake_syntax(), ARCH, BOTH_EXITS, true).expect("assembles with -g");
    let out = link(
        &fake_syntax(),
        std::slice::from_ref(&obj),
        &[],
        opts(CallMech::Mono),
    )
    .expect("links under mono");
    let mid = func(&out, "mid");
    let copy = copy_of(&out, "sub");
    assert_eq!(
        jump_targets(&out.executable.code, copy.start, copy.end),
        vec![label_addr(mid, "won"), label_addr(mid, "lost")],
        "`retx #0` to the first exit, `retx #1` to the second"
    );
}

/// An exit-bearing call in TAIL position into a callee that CAN return has
/// no instruction after it for that return, so the copy's `ret` would have
/// nowhere to land. Refused by name rather than left to surface as a
/// malformed blob at an offset the author cannot trace back to this line.
///
/// The FRAMES arm is the other half of the claim: the same program links
/// there, because a framed site returns through its descriptor and not
/// through the instruction after the call. Without it the refusal reads as
/// a property of tail position itself rather than of the copy path.
///
/// Mutation it catches: drop the tail check from `check_splice_site` and
/// the copy-path arms still fail — but as `MalformedBlob` naming an offset
/// one past the end of `mid`, which says nothing about the cause. Move the
/// check into the mechanism-independent site scan instead and the frames
/// arm goes red.
#[test]
fn an_exit_bearing_call_in_tail_position_is_refused_by_name() {
    for mech in [CallMech::Mono, CallMech::Hybrid] {
        let err = link(&fake_syntax(), &[asm(TAIL_POSITION)], &[], opts(mech))
            .expect_err("a tail-position exit-bearing call must be refused");
        assert_eq!(
            err,
            LinkError::ExitBearingTailCall("mid".to_string()),
            "under {mech}"
        );
        assert!(
            err.to_string()
                .contains("cannot be the last instruction of `mid`"),
            "the message must name the cause: {err}"
        );
    }
    link(
        &fake_syntax(),
        &[asm(TAIL_POSITION)],
        &[],
        opts(CallMech::Frames),
    )
    .expect("frames reaches the callee through a descriptor, so it links the same program");
}

/// The same site into a callee that cannot return links on EVERY
/// mechanism, and under the copy path the copy's one jump lands on the
/// site's own exit: there is no plain return to need a continuation for.
///
/// Mutation it catches: make the tail refusal unconditional again (drop
/// the `callee_can_return` conjunct) and the two copy-path arms go red.
#[test]
fn a_tail_position_call_into_a_callee_that_cannot_return_is_not_refused() {
    let obj = assemble(&fake_syntax(), ARCH, TAIL_POSITION_NORETURN, true)
        .expect("assembles with -g, so the sidecar carries `won`");
    for mech in MECHS {
        let out = link(&fake_syntax(), std::slice::from_ref(&obj), &[], opts(mech))
            .unwrap_or_else(|e| panic!("under {mech}: {e}"));
        if mech == CallMech::Frames {
            continue;
        }
        let mid = func(&out, "mid");
        let copy = copy_of(&out, "sub");
        assert_eq!(
            jump_targets(&out.executable.code, copy.start, copy.end),
            vec![label_addr(mid, "won")],
            "under {mech} the copy's `retx #0` must jump to the site's own exit"
        );
    }
}

/// `noreturn` is a DECLARATION the assembler never checks against the
/// body, so the linker reads the body: a routine that says `noreturn` and
/// returns anyway keeps the refusal.
///
/// Mutation it catches: gate the tail arm on the interface's `returns` bit
/// alone, without looking at the body, and this link succeeds — emitting a
/// copy whose `ret` jumps to an offset one past the end of `mid`.
#[test]
fn a_noreturn_header_does_not_excuse_a_body_that_returns() {
    for mech in [CallMech::Mono, CallMech::Hybrid] {
        let err = link(
            &fake_syntax(),
            &[asm(TAIL_POSITION_LYING_NORETURN)],
            &[],
            opts(mech),
        )
        .expect_err("the body returns, so the site is still refused");
        assert_eq!(
            err,
            LinkError::ExitBearingTailCall("mid".to_string()),
            "under {mech}"
        );
    }
}

/// A tail-position site into a `noreturn` callee NESTED inside a routine
/// that is itself copied. The splice's offsets are the generic `outer`'s,
/// translated through that copy's own offset map — and the continuation
/// one past the end of `outer` is in no map, so a site with no
/// continuation must carry none rather than an offset. The copy's exit
/// jump must still land inside the CALLER COPY, not in the orphaned
/// generic.
///
/// Mutation it catches: carry the one-past-the-end offset as a number and
/// translate it anyway, and the link fails with `MalformedBlob` naming
/// `outer`.
#[test]
fn a_nested_tail_position_call_into_a_callee_that_cannot_return_links() {
    for mech in [CallMech::Mono, CallMech::Hybrid] {
        let out = link(
            &fake_syntax(),
            &[asm(TAIL_POSITION_IN_STAMP)],
            &[],
            opts(mech),
        )
        .unwrap_or_else(|e| panic!("under {mech}: {e}"));
        let outer_copy = copy_of(&out, "outer");
        let inner_copy = copy_of(&out, "inner");
        let landed = jump_targets(&out.executable.code, inner_copy.start, inner_copy.end);
        assert_eq!(landed.len(), 1, "under {mech}: one exit, one jump");
        assert!(
            landed[0] >= outer_copy.start && landed[0] < outer_copy.end,
            "under {mech}: the exit must land inside the caller COPY \
             [{}, {}), not at {}",
            outer_copy.start,
            outer_copy.end,
            landed[0]
        );
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
    // The MIXED path returns its fold decisions too — not just the two fast
    // paths. One group, one site, refused on `k >= 2`; `pick` is 1 `ent` +
    // 2 bytes of `retx #0` against a 12-byte identity descriptor.
    //
    // Mutation it catches: leave `folds` off the mixed path's own `Lowered`
    // (the one return the two fast-path tests cannot reach) and this comes
    // back empty.
    let fold = out
        .report
        .folds
        .iter()
        .find(|f| f.routine == "pick")
        .unwrap_or_else(|| panic!("the mixed path reports no folds: {:?}", out.report));
    assert_eq!(fold.sites, 1, "{fold:?}");
    assert!(!fold.shared, "{fold:?}");
    assert_eq!(fold.body_bytes, 3, "1 ent + 2 retx: {fold:?}");
    assert_eq!(
        fold.descriptor_bytes, 12,
        "one identity descriptor: {fold:?}"
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

// -- P4: hybrid's exit-bearing fold -----------------------------------------

/// A body large enough that sharing three sites beats three copies:
/// 20 `nop`s, so `B` = 23 against `sum(d_i)` = 36 and `2 * 23 > 36`.
/// The flip point is 16 nops, so the fixture is clear of the boundary and
/// a one-byte drift in any instruction width cannot silently flip the
/// test's meaning. The arithmetic is asserted below, not trusted.
const THREE_SITES_BIG_BODY: &str = "\
.routine main, tapes=1, alpha=(3)
.param t, ('_', '0', '1')
.routine big, tapes=1, alpha=(3), exits=1
.param n, ('_', '0', '1')
.section code
.func main
        call    big [0] exits=(x)
        call    big [0] exits=(y)
        call    big [0] exits=(z)
        stp
x:      wr      [1]
        stp
y:      wr      [2]
        stp
z:      wr      [1]
        stp
.func big
        nop
        nop
        nop
        nop
        nop
        nop
        nop
        nop
        nop
        nop
        nop
        nop
        nop
        nop
        nop
        nop
        nop
        nop
        nop
        nop
        retx    #0
";

/// ONE exit-bearing site: the byte rule refuses to share it, so the site
/// is seeded to mono and `TWO_EXITS` has no other bound site — which means
/// `any_frames` stays false and hybrid takes its `!any_frames` →
/// `lower_mono` fast path. That is precisely why `folds` has to be
/// attached to that return too: the decision was taken before the fast
/// path was chosen, and it is the only place it can be reported from.
///
/// Mutation it catches: leave `folds` off the `lower_mono` return (or take
/// a fast path before the decision loop) and `report.folds` comes back
/// empty, so the `unwrap_or_else` below fires.
///
/// The `k >= 2` conjunct of the rule is NOT pinned here, and no fixture
/// can pin it: with `k == 1` the product `(k - 1) * B` is 0 and
/// `0 > sum(d_i)` is false for every group, because a descriptor is never
/// zero bytes. The conjunct states the intent and guards the `k - 1`
/// subtraction; it changes no outcome anywhere. Recorded rather than
/// faked.
#[test]
fn one_exit_bearing_site_splices_under_hybrid() {
    let out = link(
        &fake_syntax(),
        &[asm(TWO_EXITS)],
        &[],
        opts(CallMech::Hybrid),
    )
    .expect("links under hybrid");
    assert_eq!(out.report.composites, 0, "{:?}", out.report);
    let fold = out
        .report
        .folds
        .iter()
        .find(|f| f.routine == "sub")
        .unwrap_or_else(|| {
            panic!(
                "no fold decision survived the mono fast path: {:?}",
                out.report
            )
        });
    assert!(!fold.shared, "{fold:?}");
    assert_eq!(fold.sites, 1, "{fold:?}");
    assert!(out.report.instantiations >= 1, "{:?}", out.report);
}

/// Three exit-bearing sites over a body big enough that two extra copies
/// cost more than three descriptors: hybrid shares them under frames.
///
/// Mutation it catches: invert the inequality and this shares nothing —
/// `composites` drops to 0 and `shared` goes false. The two numeric
/// assertions are the ONLY pin on the exact cost: `fold.shared` alone has
/// slack (drop the `4 * |exits|` term from the descriptor and the group
/// still shares at 46 > 24), so they are load-bearing and must not be
/// relaxed.
#[test]
fn three_exit_bearing_sites_over_a_large_body_share_under_hybrid() {
    let out = link(
        &fake_syntax(),
        &[asm(THREE_SITES_BIG_BODY)],
        &[],
        opts(CallMech::Hybrid),
    )
    .expect("links under hybrid");
    let fold = out
        .report
        .folds
        .iter()
        .find(|f| f.routine == "big")
        .expect("a fold decision for `big`");
    assert!(fold.shared, "{fold:?}");
    assert_eq!(fold.sites, 3, "{fold:?}");
    // The arithmetic, pinned: if either number moves, the instruction
    // widths are not what the fixture assumes and the `nop` count must be
    // re-derived.
    assert_eq!(fold.body_bytes, 23, "1 ent + 20 nop + 2 retx: {fold:?}");
    assert_eq!(
        fold.descriptor_bytes, 36,
        "three 12-byte descriptors: {fold:?}"
    );
    // A shared group gets ONE body and one descriptor PER SITE: the three
    // sites agree on the composite and differ only in their exit labels,
    // and the engine's intern key carries the exits, so they never dedup
    // onto one directory entry. `> 0` would also be satisfied by a single
    // group-wide descriptor, which is the wrong lowering.
    assert_eq!(
        out.report.composites, 3,
        "one descriptor per site: {:?}",
        out.report
    );
}

// -- P4b: the fold count runs over the whole reachable set -------------------

/// `n` `nop` lines. A fold fixture's whole size knob is the body length, and
/// the arithmetic in each comment below names `n` directly — a helper keeps
/// the two from drifting apart the way fifty literal lines invite.
fn nops(n: usize) -> String {
    "        nop\n".repeat(n)
}

/// `big` is reached from ONE exit-bearing site at the identity world
/// (`main`'s, through a swap binding) and from TWO more inside `outer`'s
/// stamped copy (transparent, so they compose to the same composite the
/// swap does). The three are one group only if the closure sites are
/// counted.
///
/// The arithmetic, EXACT and stated so the fixture is auditable rather
/// than tuned. All three sites reach the SAME composite — the swap —
/// because `outer` is stamped under it and its two calls to `big` are
/// transparent, so `compose(C_swap, identity)` is `C_swap` again. That
/// composite's maps are not identity, so `dense_map` emits one `u16`
/// per symbol in each direction over the 4-symbol alphabet and
/// `descriptor_bytes` writes
/// `1 (arity) + 2 (exit_count) + [1 (phys) + 2 + 2*4 + 2 + 2*4] + 4 (one exit)`
/// = **28 bytes** per site — the same 28 for the swap site and for each
/// transparent one, since they share the composite. `sum(d_i)` = **84**.
/// `big`'s blob is the implicit 1-byte `ent` prologue + 50 `nop`s +
/// 2 bytes of `retx #0`, with no table, so `B` = **53**. With all three
/// sites, `(3 - 1) * 53 = 106 > 84` and the group SHARES; the flip point
/// is 40 nops, so the fixture is clear of the boundary. With only the
/// identity-world site, `k` is 1 and the rule refuses on `k >= 2` alone,
/// whatever `B` is.
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
a:      wr      [1]
        stp
.func outer
        call    big [0] exits=(p)
        ret
p:      call    big [0] exits=(q)
        ret
q:      ret
.func big
{}        retx    #0
",
        nops(50)
    )
}

/// The fold group spans the identity world and the inside of a stamped
/// copy.
///
/// Mutation it catches: drop the probe (group over the identity world
/// only) and `fold.sites` is 1 and `fold.shared` is false — the two
/// closure sites each splice a per-site copy instead, which
/// `instantiations` shows.
///
/// The three numeric assertions are the pin on the EXACT cost, exactly as
/// in the identity-world case: `shared` alone has slack.
#[test]
fn exit_bearing_sites_inside_a_stamped_copy_join_their_group() {
    let out = link(
        &fake_syntax(),
        &[asm(&closure_fold())],
        &[],
        opts(CallMech::Hybrid),
    )
    .expect("links under hybrid");
    let fold = out
        .report
        .folds
        .iter()
        .find(|f| f.routine == "big")
        .unwrap_or_else(|| panic!("no fold decision for `big`: {:?}", out.report.folds));
    assert_eq!(
        fold.sites, 3,
        "one identity-world site plus two inside the copy: {fold:?}"
    );
    assert_eq!(fold.body_bytes, 53, "1 ent + 50 nop + 2 retx: {fold:?}");
    assert_eq!(
        fold.descriptor_bytes, 84,
        "three 28-byte descriptors over one shared composite: {fold:?}"
    );
    assert!(fold.shared, "106 > 84, so the group shares: {fold:?}");
}

/// A shared closure site becomes a framed call inside the copy, NOT a
/// child stamp: `outer` is stamped once and `big` keeps its single
/// generic copy, so the image carries exactly one stamp.
///
/// Mutation it catches: ignore the `shared` set in `mono_stamps` and the
/// copy interns two child stamps of `big`, so `instantiations` is 3
/// instead of 1. It ALSO catches dropping `any_frames = true` from the
/// shared branch: hybrid then takes its `!any_frames` fast path into
/// `lower_mono`, which re-seeds every identity-world bound site with an
/// EMPTY shared set — four stamps, not one.
#[test]
fn a_shared_closure_site_frames_instead_of_stamping_a_child() {
    let out = link(
        &fake_syntax(),
        &[asm(&closure_fold())],
        &[],
        opts(CallMech::Hybrid),
    )
    .expect("links under hybrid");
    assert_eq!(
        out.report.instantiations, 1,
        "only `outer` is stamped; `big` stays generic: {:?}",
        out.report
    );
    // One directory entry PER SITE, exactly as the identity-world sibling
    // pins: the engine composite for `main`'s own site, plus one raw
    // descriptor for each of the copy's two framed calls. `>= 2` would
    // also be satisfied by two sites sharing one entry, which is the
    // wrong lowering — the exits differ, so they must not dedup.
    assert_eq!(
        out.report.composites, 3,
        "one entry per site: 1 engine + 2 raw: {:?}",
        out.report
    );
    // `big` survives as a generic routine — a shared group's whole point.
    assert!(
        out.map.functions.iter().any(|f| f.name == "big"),
        "the shared body must be in the image"
    );
}

/// The same program under the other two mechanisms: mono splices
/// everything, frames descriptors everything, and both must still link.
/// The three images differ; what must not differ is that each is
/// well-formed and reproducible.
///
/// Mutation it catches: emit the stamp's descriptor with exits in the
/// ENCLOSING routine's offsets (skip the `old_to_new` remap) and layout
/// rejects the raw descriptor as malformed table data, so hybrid stops
/// linking while mono and frames still do.
#[test]
fn the_closure_fold_program_links_and_relinks_under_every_mechanism() {
    let src = closure_fold();
    for mech in MECHS {
        let a = link(&fake_syntax(), &[asm(&src)], &[], opts(mech))
            .unwrap_or_else(|e| panic!("under {mech}: {e}"));
        let b = link(&fake_syntax(), &[asm(&src)], &[], opts(mech))
            .unwrap_or_else(|e| panic!("under {mech}: {e}"));
        assert_eq!(
            a.executable.to_bytes(),
            b.executable.to_bytes(),
            "the {mech} image is not reproducible"
        );
    }
}

/// The same shape with `main`'s own exit-bearing site REMOVED: `big`'s
/// only two sites both sit inside `outer`'s stamped copy, so the group
/// has no identity-world member at all — and with 60 `nop`s it SHARES.
///
/// `B` = 1 `ent` + 60 `nop` + 2 `retx` = **63** against two 28-byte
/// descriptors, `sum(d_i)` = **56**; `(2 - 1) * 63 = 63 > 56`, with the
/// flip point at 54 `nop`s. That makes this the one program where the
/// shared body is reached from NOWHERE but inside a stamped copy, which
/// is a corner in two places at once:
///
/// - **the prune**: nothing in `main` points at `big` any more, so the
///   only edge keeping it in the image is the stamp's own framed-call
///   displacement relocation, and `prune_unreachable` has to follow it;
/// - **the frames path**: the closure from the entry meets no bound site
///   at all, so `engine_count` is 0 and the whole directory is raw
///   descriptors from inside a stamp.
fn closure_only_fold() -> String {
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
        stp
.func outer
        call    big [0] exits=(p)
        ret
p:      call    big [0] exits=(q)
        ret
q:      ret
.func big
{}        retx    #0
",
        nops(60)
    )
}

/// A group whose members are ALL closure sites is decided like any other,
/// and the body it shares survives on the strength of the stamp's own
/// framed call.
///
/// Mutation it catches: leave `group_composite` unpopulated in the probe
/// merge and the decision loop's `group_composite[key]` index panics —
/// `closure_fold` masks that, because its identity-world member fills the
/// key first. Drop the framed call's `calls.push` (the displacement
/// relocation into the shared body) and `prune_unreachable` finds no edge
/// to `big` at all: it is dropped from the image and the link fails on
/// the dangling framed call. The splice branch seeding a non-`Identity`
/// member has no caller or address to seed WITH; the skip that avoids it
/// is exercised by the closure-only groups in
/// `a_nested_pass_through_site_with_exits_still_splices` and
/// `an_exit_vector_links_under_mono_and_hybrid`, which are refused rather
/// than shared.
#[test]
fn a_group_with_only_closure_sites_is_decided_and_shared() {
    let out = link(
        &fake_syntax(),
        &[asm(&closure_only_fold())],
        &[],
        opts(CallMech::Hybrid),
    )
    .expect("links under hybrid");
    let fold = out
        .report
        .folds
        .iter()
        .find(|f| f.routine == "big")
        .unwrap_or_else(|| panic!("no fold decision for `big`: {:?}", out.report.folds));
    assert_eq!(fold.sites, 2, "both sites sit inside the copy: {fold:?}");
    assert_eq!(fold.body_bytes, 63, "1 ent + 60 nop + 2 retx: {fold:?}");
    assert_eq!(
        fold.descriptor_bytes, 56,
        "two 28-byte descriptors: {fold:?}"
    );
    assert!(fold.shared, "63 > 56, so the group shares: {fold:?}");
    // Only `outer` is stamped; `big` keeps its single generic body.
    assert_eq!(
        out.report.instantiations, 1,
        "the shared body is not copied: {:?}",
        out.report
    );
    // The whole directory is raw descriptors emitted from inside the
    // stamp: the closure from the entry meets no bound site at all, so
    // `engine_count` is 0 and these two are all there is.
    assert_eq!(
        out.report.composites, 2,
        "one raw descriptor per framed site: {:?}",
        out.report
    );
    // And the body survived the prune on the strength of the stamp's
    // framed-call edge alone — nothing else in the image names it.
    assert!(
        out.map.functions.iter().any(|f| f.name == "big"),
        "the shared body must survive the prune: {:?}",
        out.map
            .functions
            .iter()
            .map(|f| &f.name)
            .collect::<Vec<_>>()
    );
}

/// Two fold groups in ONE link, one of them over a TABLE-BEARING callee.
/// Both bindings are the identity over equal 3-symbol alphabets, so every
/// descriptor's dense maps are empty and each costs
/// `1 + 2 + [1 + 2 + 0 + 2 + 0] + 4` = **12 bytes**; two sites per group
/// gives `sum(d_i)` = **24**.
///
/// `abe` is 1 `ent` + 25 `nop` + 2 `retx` = **28** bytes of code and no
/// table. `zed` is 1 `ent` + 5 (`tmatch`) + 5 (`tdispatch`) + 2 (`wr`) +
/// 2 (`retx`) + 2 (`wr`) + 2 (`retx`) = **19** bytes of code PLUS a table
/// blob of 5 (match: width 1, 2 rows) + 10 (dispatch: count + 2 entries)
/// = 15, so `B` = **34**. Both clear `(2 - 1) * B > 24` and share.
///
/// `zed` is declared and called FIRST, so it takes the lower `order`
/// index and the decision loop — which walks the group keys in
/// `(callee index, composite)` order — reaches it first. `folds` comes
/// back sorted by name, so `abe` precedes it: insertion order and
/// reported order genuinely differ.
fn two_groups_one_table() -> String {
    format!(
        "\
.routine main, tapes=1, alpha=(3)
.param t, ('_', '0', '1')
.routine zed, tapes=1, alpha=(3), exits=1
.param v, ('_', '0', '1')
.routine abe, tapes=1, alpha=(3), exits=1
.param w, ('_', '0', '1')
.section tables
T0: .row [1]
    .row [*]
D0: .targets P, Q
.section code
.func main
        call    zed [0] exits=(e1)
        call    zed [0] exits=(e2)
        call    abe [0] exits=(e3)
        call    abe [0] exits=(e4)
        stp
e1:     wr      [1]
        stp
e2:     wr      [2]
        stp
e3:     wr      [1]
        stp
e4:     wr      [2]
        stp
.func zed
        tmatch  T0
        tdispatch D0
P:      wr      [1]
        retx    #0
Q:      wr      [2]
        retx    #0
.func abe
{}        retx    #0
",
        nops(25)
    )
}

/// A callee's TABLE counts toward `B`, and two groups report in sorted
/// order.
///
/// Mutation it catches: drop `order[callee].table.len()` from `B` and
/// `zed`'s body falls to 19, which loses `19 > 24` — the table-bearing
/// group stops sharing and both the `body_bytes` and the `shared`
/// assertions below fail. Reverse the `folds` sort and the two
/// `routine` assertions fail.
#[test]
fn a_table_bearing_callee_counts_its_table_and_folds_report_in_sorted_order() {
    let out = link(
        &fake_syntax(),
        &[asm(&two_groups_one_table())],
        &[],
        opts(CallMech::Hybrid),
    )
    .expect("links under hybrid");
    let folds = &out.report.folds;
    assert_eq!(folds.len(), 2, "one decision per group: {folds:?}");
    assert_eq!(folds[0].routine, "abe", "sorted by name: {folds:?}");
    assert_eq!(folds[1].routine, "zed", "sorted by name: {folds:?}");
    assert_eq!(
        folds[0].body_bytes, 28,
        "1 ent + 25 nop + 2 retx: {folds:?}"
    );
    assert_eq!(
        folds[1].body_bytes, 34,
        "19 bytes of code plus a 15-byte table blob: {folds:?}"
    );
    for f in folds {
        assert_eq!(f.sites, 2, "{f:?}");
        assert_eq!(f.descriptor_bytes, 24, "two 12-byte descriptors: {f:?}");
        assert!(f.shared, "{f:?}");
    }
}

// -- a recursive exit-bearing bound call -------------------------------------

/// A CYCLE of exit-bearing bound calls. `main`'s swap into `outer` is the
/// exit-free bijection seed, so the walk enters `outer`'s copy; `outer`
/// calls `a` with an exit, and `a`'s own exit-bearing call goes straight
/// back into `a`. Every turn returns somewhere new, so every turn is a
/// distinct splice and a distinct copy: the copy path has no finite
/// lowering for this at all.
///
/// Both of `a`'s incoming sites bind transparently under the swap, so they
/// compose to the SAME composite and form ONE fold group of two — which is
/// what lets the same source pin both halves of hybrid's decision at two
/// body sizes.
///
/// The arithmetic, EXACT: the composite's maps are the swap's, so
/// `dense_map` emits one `u16` per symbol in each direction over the
/// 3-symbol alphabet and each descriptor is
/// `1 (arity) + 2 (exit_count) + [1 (phys) + 2 + 2*3 + 2 + 2*3] + 4 (one
/// exit)` = **24 bytes**; `sum(d_i)` = **48**. `a`'s blob is the implicit
/// 1-byte `ent` prologue + 5 bytes of `call` + `body` `nop`s + two 2-byte
/// `retx`es, so `B` = **10 + body** and the group shares exactly when
/// `body > 38`.
fn recursive_exit_bearing(body: usize) -> String {
    format!(
        "\
.routine main, tapes=1, alpha=(3)
.param t, ('_', '0', '1')
.routine outer, tapes=1, alpha=(3)
.param u, ('_', '0', '1')
.routine a, tapes=1, alpha=(3), exits=1
.param n, ('_', '0', '1')
.section code
.func main
        call    outer [0{{1->2, 2->1}}]
        stp
.func outer
        call    a [0] exits=(p)
        ret
p:      ret
.func a
        call    a [0] exits=(s)
{}        retx    #0
s:      retx    #0
",
        nops(body)
    )
}

/// A cycle of exit-bearing calls BROKEN by a plain call, which the copy
/// path lowers perfectly well: `outer` splices `b`, `b` PLAIN-calls `c`,
/// and `c` splices `b` again. `c` is minted on a plain edge, so it keys on
/// its composite alone — the second lap reaches the same composite of `c`
/// and dedups onto the first, and the walk closes with four copies
/// (`outer`, `b`, `c`, and `b` again for `c`'s own splice).
///
/// The shape is deliberately minimal: one non-splice hop is all it takes
/// to make an exit-bearing cycle finite, so this is the boundary between
/// what the refusal must catch and what it must leave alone.
const BROKEN_CYCLE: &str = "\
.routine main, tapes=1, alpha=(3)
.param t, ('_', '0', '1')
.routine outer, tapes=1, alpha=(3)
.param u, ('_', '0', '1')
.routine b, tapes=1, alpha=(3), exits=1
.param n, ('_', '0', '1')
.routine c, tapes=1, alpha=(3)
.param m, ('_', '0', '1')
.section code
.func main
        call    outer [0{1->2, 2->1}]
        stp
.func outer
        call    b [0] exits=(p)
        ret
p:      ret
.func b
        call    c
        retx    #0
.func c
        call    b [0] exits=(q)
        ret
q:      ret
";

/// Mutation it catches: stop resetting the splice chain at a non-splice
/// node — walk past `c` to the `b` above it — and this legal program is
/// refused `RecursiveExitBearingCall`, which is the over-refusal the
/// exact rule exists to avoid.
#[test]
fn an_exit_bearing_cycle_broken_by_a_plain_call_still_copies() {
    for mech in [CallMech::Mono, CallMech::Hybrid] {
        let out = link_bounded(BROKEN_CYCLE, mech)
            .unwrap_or_else(|e| panic!("{mech} refused a finite copy path: {e}"));
        assert_eq!(
            out.report.instantiations, 4,
            "one `outer`, one `b` per splice site, and ONE `c` — the \
             second lap's `c` dedups by composite: {:?}",
            out.report
        );
    }
    // Hybrid reaches the copy path here rather than sharing `b`: two sites
    // at 24 bytes of descriptor each against a body far under 48, so the
    // byte rule refuses the group and both sites splice.
    let out = link_bounded(BROKEN_CYCLE, CallMech::Hybrid).expect("links under hybrid");
    let fold = out
        .report
        .folds
        .iter()
        .find(|f| f.routine == "b")
        .unwrap_or_else(|| panic!("no fold decision for `b`: {:?}", out.report));
    assert_eq!(fold.sites, 2, "`outer`'s site and `c`'s: {fold:?}");
    assert!(
        !fold.shared,
        "the group splices, so copies are made: {fold:?}"
    );
}

/// The same cycle closed through a THIRD routine: `a` splices `b` and `b`
/// splices back into `a`, so no node's immediate parent ever names the
/// routine it is about to copy — only the chain does.
const RECURSIVE_THROUGH_A_THIRD: &str = "\
.routine main, tapes=1, alpha=(3)
.param t, ('_', '0', '1')
.routine outer, tapes=1, alpha=(3)
.param u, ('_', '0', '1')
.routine a, tapes=1, alpha=(3), exits=1
.param n, ('_', '0', '1')
.routine b, tapes=1, alpha=(3), exits=1
.param m, ('_', '0', '1')
.section code
.func main
        call    outer [0{1->2, 2->1}]
        stp
.func outer
        call    a [0] exits=(p)
        ret
p:      ret
.func a
        call    b [0] exits=(s)
        retx    #0
s:      retx    #0
.func b
        call    a [0] exits=(u)
        retx    #0
u:      retx    #0
";

/// Link `src` under `mech` on a spawned thread and fail if it has not
/// returned within the timeout. The defect these tests pin is a walk that
/// mints forever, and a test that reproduces one by hanging reports
/// nothing — it just never finishes.
fn link_bounded(src: &str, mech: CallMech) -> Result<LinkOutput, LinkError> {
    let owned = src.to_string();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(link(&fake_syntax(), &[asm(&owned)], &[], opts(mech)));
    });
    rx.recv_timeout(std::time::Duration::from_secs(20))
        .expect("the link returned rather than minting copies forever")
}

/// Mutation it catches: drop the splice-chain check in the closure's
/// intern and neither the mono link nor hybrid's own copy path ever
/// returns — the bound is what turns that into a failure instead of a hung
/// job.
#[test]
fn a_recursive_exit_bearing_call_is_refused_on_the_copy_path() {
    let src = recursive_exit_bearing(20);
    let refusal = LinkError::RecursiveExitBearingCall("a".to_string());
    assert_eq!(
        link_bounded(&src, CallMech::Mono).expect_err("mono always copies"),
        refusal
    );
    // At 20 `nop`s `(2 - 1) * 30` is not more than 48, so hybrid's byte
    // rule refuses the group sharing and the copy path is the one that
    // runs — the same refusal, reached through hybrid's own decision.
    assert_eq!(
        link_bounded(&src, CallMech::Hybrid).expect_err("hybrid splices this group"),
        refusal
    );
    // Frames copies nothing: one generic body, one descriptor per site,
    // and the loop closes at run time through the frame register.
    link_bounded(&src, CallMech::Frames).expect("frames lowers a recursive exit-bearing call");
}

/// Mutation it catches: compare against the immediate parent alone instead
/// of walking the splice chain, and this shape mints forever — `a`'s
/// parent is `b` and `b`'s is `a`, so no single step ever repeats. The
/// refusal names the routine the chain re-enters.
#[test]
fn a_recursive_exit_bearing_call_through_a_third_routine_is_refused() {
    assert_eq!(
        link_bounded(RECURSIVE_THROUGH_A_THIRD, CallMech::Mono).expect_err("mono always copies"),
        LinkError::RecursiveExitBearingCall("a".to_string())
    );
    link_bounded(RECURSIVE_THROUGH_A_THIRD, CallMech::Frames)
        .expect("frames lowers the cycle through descriptors");
}

/// The refusal is the COPY path's, not the program's: the identical source
/// links under hybrid as soon as its byte rule shares the group, because a
/// shared group is reached through a descriptor and never copied.
///
/// Mutation it catches: raise the refusal before the sharing decision (in
/// the probe, say, instead of swallowing it there) and this link refuses
/// too — a program that has a correct lowering would stop having one.
#[test]
fn a_recursive_exit_bearing_call_links_under_hybrid_when_its_group_shares() {
    let src = recursive_exit_bearing(60);
    let out = link_bounded(&src, CallMech::Hybrid).expect("hybrid shares this group");
    let fold = out
        .report
        .folds
        .iter()
        .find(|f| f.routine == "a")
        .unwrap_or_else(|| panic!("no fold decision for `a`: {:?}", out.report));
    assert_eq!(
        fold.sites, 2,
        "`outer`'s site and `a`'s own re-entry: {fold:?}"
    );
    assert_eq!(
        fold.body_bytes, 70,
        "1 ent + 5 call + 60 nop + 4 retx: {fold:?}"
    );
    assert_eq!(
        fold.descriptor_bytes, 48,
        "two 24-byte descriptors: {fold:?}"
    );
    assert!(fold.shared, "70 > 48, so hybrid shares: {fold:?}");
    // Sharing is hybrid's alone — mono has no such decision to take, so
    // the same source at the same size still refuses there.
    assert_eq!(
        link_bounded(&src, CallMech::Mono).expect_err("mono always copies"),
        LinkError::RecursiveExitBearingCall("a".to_string())
    );
}

/// Mutation it catches: derive a nested site's active-frame row by looking
/// its enclosing composite up under the BARE canonical key instead of
/// carrying the index it was interned under, and `mid`'s compose column
/// lands in row 0 while row 1 — the row `mid` actually runs under — keeps
/// the reserved-invalid 0. The link still reports success; the image traps
/// on a bad operand the first time `mid` calls `big`.
#[test]
fn a_site_nested_under_an_exit_bearing_frame_composes_in_its_own_row() {
    let out = link(
        &fake_syntax(),
        &[asm(NESTED_UNDER_EXIT_BEARING)],
        &[],
        opts(CallMech::Frames),
    )
    .expect("links under frames");
    assert_eq!(
        directory(&out.executable).len(),
        2,
        "one composite for `mid`, one for `big`"
    );
    assert_eq!(
        compose_matrix(&out.executable),
        vec![vec![1, 0], vec![0, 2], vec![0, 0]],
        "row 0 is the machine frame, where only `main`'s site is live and \
         selects `mid`'s composite; row 1 is inside `mid`, where its own \
         site selects `big`'s; row 2 is inside `big`, which frames nothing"
    );
}
