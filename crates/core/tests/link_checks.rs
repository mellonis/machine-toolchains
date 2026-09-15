//! What the linker checks at a call site that binds by INDEX: a plain
//! call, and a bound call whose map is omitted
//! (docs/core.md (link warnings)). An explicit map — `{}` included — is
//! the author's statement and silences the glyph comparison.
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
/// and an unconditional `jmp` — a relocated tail jump is a
/// `SiteKind::Plain` site exactly like a relocated call, so the
/// exit-count tests need one to reach a callee that way too.
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

/// A 5-symbol caller plainly calling a 3-symbol callee.
const NARROW: &str = "\
.routine main, tapes=1, alpha=(5)
.routine sub, tapes=1, alpha=(3)
.section code
.func main
        call    sub
        stp
.func sub
        ret
";

/// A 3-symbol caller plainly calling a 5-symbol callee.
const WIDER_ALPHABET: &str = "\
.routine main, tapes=1, alpha=(3)
.routine sub, tapes=1, alpha=(5)
.section code
.func main
        call    sub
        stp
.func sub
        ret
";

/// A one-tape caller plainly calling a two-tape callee.
const WIDER_TAPES: &str = "\
.routine main, tapes=1, alpha=(3)
.routine sub, tapes=2, alpha=(3, 3)
.section code
.func main
        call    sub
        stp
.func sub
        ret
";

/// A two-tape caller plainly calling a one-tape callee: narrower in tape
/// count, which is exactly how a transparent call is meant to work.
const NARROWER_TAPES: &str = "\
.routine main, tapes=2, alpha=(3, 3)
.routine sub, tapes=1, alpha=(3)
.section code
.func main
        call    sub
        stp
.func sub
        ret
";

/// Equal widths, DIFFERENT glyphs, both sides declaring an interface.
const REORDERED: &str = "\
.routine main, tapes=1, alpha=(3)
.param t, ('_', '0', '1')
.routine sub, tapes=1, alpha=(3)
.param n, ('_', '1', '0')
.section code
.func main
        call    sub
        stp
.func sub
        ret
";

/// Mutation it catches: skip the narrower-alphabet arm and this program
/// links with no diagnostic at all.
#[test]
fn a_narrower_callee_alphabet_warns() {
    let out = link(&fake_syntax(), &[asm(NARROW)], &[], opts(CallMech::Frames))
        .expect("a warning does not stop the link");
    assert_eq!(
        out.report.diagnostics.len(),
        1,
        "{:?}",
        out.report.diagnostics
    );
    assert_eq!(out.report.diagnostics[0].code, "narrow-alphabet");
    assert_eq!(out.report.diagnostics[0].function, "main");
}

/// Mutation it catches: grade a wider alphabet as a warning and this
/// link succeeds, letting the callee write an index the band cannot
/// hold.
#[test]
fn a_wider_callee_alphabet_is_an_error() {
    let err = link(
        &fake_syntax(),
        &[asm(WIDER_ALPHABET)],
        &[],
        opts(CallMech::Frames),
    )
    .expect_err("a wider callee alphabet must stop the link");
    assert!(
        matches!(&err, LinkError::CalleeWider { callee, what, .. }
            if callee == "sub" && what.contains("alphabet")),
        "{err:?}"
    );
}

/// Mutation it catches: compare only the alphabets and a callee that
/// addresses a band the caller does not have links silently.
#[test]
fn a_wider_callee_tape_count_is_an_error() {
    let err = link(
        &fake_syntax(),
        &[asm(WIDER_TAPES)],
        &[],
        opts(CallMech::Frames),
    )
    .expect_err("a wider callee arity must stop the link");
    assert!(
        matches!(&err, LinkError::CalleeWider { what, .. } if what == "tape count"),
        "{err:?}"
    );
}

/// Mutation it catches: treat ANY arity difference as an error and every
/// transparent call on the first k bands breaks.
#[test]
fn a_narrower_callee_tape_count_is_silent() {
    let out = link(
        &fake_syntax(),
        &[asm(NARROWER_TAPES)],
        &[],
        opts(CallMech::Frames),
    )
    .expect("a narrower callee arity is how it is meant to work");
    assert!(
        out.report.diagnostics.is_empty(),
        "{:?}",
        out.report.diagnostics
    );
}

/// Mutation it catches: compare the glyph LISTS by length instead of
/// element-wise and a reordered alphabet — the item-4 hazard, silently
/// wrong output — goes unreported.
#[test]
fn differing_glyphs_at_equal_width_warn_and_name_the_first_difference() {
    let out = link(
        &fake_syntax(),
        &[asm(REORDERED)],
        &[],
        opts(CallMech::Frames),
    )
    .expect("a warning does not stop the link");
    let d = out
        .report
        .diagnostics
        .iter()
        .find(|d| d.code == "glyph-mismatch")
        .unwrap_or_else(|| panic!("{:?}", out.report.diagnostics));
    assert!(d.message.contains("position 1"), "{}", d.message);
}

/// A MIXED hybrid link — one bound site that stays a bijection (promoted
/// to a mono stamp) alongside one that stays holey (frames-lowered) —
/// classifies bound sites twice: once when `lower_hybrid` groups seeds,
/// once more in its OWN internal `scan_sites` re-scan over the
/// mono-rewritten order (`stamp.rs`'s `new_sites`, feeding the frames
/// sub-pass). A fixture with only a plain call and no bound call at all
/// never reaches that re-scan (`lower`'s `has_bound` early-out returns
/// before the `CallMech` dispatch), so it cannot exercise this; `swap`
/// and `narrow` below force the real mixed path, with `sub` supplying
/// the one warned PLAIN site whose diagnostic must not double.
///
/// Mutation it catches: move the check into `scan_sites` (called once
/// per mechanism up front AND again by `lower_hybrid`'s internal
/// re-scan) and hybrid reports the `sub` warning twice while mono and
/// frames still report it once — the per-mechanism loop below catches
/// the asymmetry mono/frames alone cannot.
#[test]
fn a_diagnostic_is_reported_once_under_every_mechanism() {
    const MIXED_WITH_WARNING: &str = "\
.routine main, tapes=1, alpha=(4)
.routine swap, tapes=1, alpha=(4)
.routine narrow, tapes=1, alpha=(2)
.routine sub, tapes=1, alpha=(3)
.section code
.func main
        call    swap [0{1->2, 2->1}]
        call    narrow [0{1=>0}]
        call    sub
        stp
.func swap
        wr [1]
        ret
.func narrow
        wr [1]
        ret
.func sub
        ret
";
    for mech in MECHS {
        let out = link(&fake_syntax(), &[asm(MIXED_WITH_WARNING)], &[], opts(mech))
            .unwrap_or_else(|e| panic!("under {mech}: {e}"));
        assert_eq!(
            out.report.diagnostics.len(),
            1,
            "under {mech}: {:?}",
            out.report.diagnostics
        );
        assert_eq!(out.report.diagnostics[0].code, "narrow-alphabet");
    }
}

/// A bound site whose ONE tape omits its map into a callee with the same
/// width but different glyphs. Mutation it catches: restrict the glyph
/// check to plain sites and the omitted-map case — item 4's other half —
/// goes unreported.
///
/// `[1, 0]` is POSITIONAL: entry 0 binds callee tape `p` to caller tape
/// 1, entry 1 binds callee tape `q` to caller tape 0. The comparison is
/// therefore `main`'s tape-1 glyphs against `sub`'s `p` glyphs — both 3
/// wide, `p` reordered, so the warning fires.
#[test]
fn an_omitted_map_on_a_bound_site_warns_like_a_plain_one() {
    const OMITTED: &str = "\
.routine main, tapes=2, alpha=(3, 3)
.param a, ('_', '0', '1')
.param b, ('_', '0', '1')
.routine sub, tapes=2, alpha=(3, 3)
.param p, ('_', '1', '0')
.param q, ('_', '0', '1')
.section code
.func main
        call    sub [1, 0]
        stp
.func sub
        ret
";
    let out = link(&fake_syntax(), &[asm(OMITTED)], &[], opts(CallMech::Frames)).expect("links");
    assert!(
        out.report
            .diagnostics
            .iter()
            .any(|d| d.code == "glyph-mismatch"),
        "{:?}",
        out.report.diagnostics
    );
}

/// The empty map `{}` is "bind by index, on purpose" and silences the
/// comparison for that binding. Mutation it catches: ignore
/// `map_written` and `{}` warns exactly like the bare form, so the
/// author has no local opt-out.
#[test]
fn a_written_empty_map_silences_the_glyph_comparison() {
    const WRITTEN: &str = "\
.routine main, tapes=2, alpha=(3, 3)
.param a, ('_', '0', '1')
.param b, ('_', '0', '1')
.routine sub, tapes=2, alpha=(3, 3)
.param p, ('_', '1', '0')
.param q, ('_', '0', '1')
.section code
.func main
        call    sub [1{}, 0{}]
        stp
.func sub
        ret
";
    let out = link(&fake_syntax(), &[asm(WRITTEN)], &[], opts(CallMech::Frames)).expect("links");
    assert!(
        !out.report
            .diagnostics
            .iter()
            .any(|d| d.code == "glyph-mismatch"),
        "`{{}}` must silence the comparison: {:?}",
        out.report.diagnostics
    );
}

/// `sub` declares one state parameter; `main` reaches it through a PLAIN
/// call, which supplies no exit vector at all — the callee's `retx #0`
/// would index a vector that is not there.
///
/// Mutation it catches: skip exit-count grading on `SiteKind::Plain` and
/// this program links, leaving the callee's exit contract silently
/// unmet.
#[test]
fn a_plain_call_into_an_exit_bearing_callee_is_refused() {
    const EXIT_CALLEE: &str = "\
.routine main, tapes=1, alpha=(3)
.param x, ('_', '0', '1')
.routine sub, tapes=1, alpha=(3), exits=1
.param p, ('_', '0', '1')
.section code
.func main
        call    sub
        stp
.func sub
        ret
";
    let err = link(
        &fake_syntax(),
        &[asm(EXIT_CALLEE)],
        &[],
        opts(CallMech::Frames),
    )
    .expect_err("a plain call into an exit-bearing callee must be refused");
    assert!(
        matches!(&err, LinkError::BadBinding { callee, message }
            if callee == "sub" && message.contains("declares 1")),
        "{err:?}"
    );
}

/// The same refusal, reached through a RELOCATED TAIL JUMP rather than a
/// `call`: `SiteKind::Plain` covers both, because the tail-call pass
/// turns calls into jumps, and grading only `call` opcodes would let the
/// optimizer route around the check.
///
/// Mutation it catches: match on the `call` mnemonic instead of
/// `SiteKind::Plain` and a relocated `jmp` into an exit-bearing callee
/// links unrefused.
#[test]
fn a_relocated_jump_into_an_exit_bearing_callee_is_refused() {
    const EXIT_JUMP: &str = "\
.routine main, tapes=1, alpha=(3)
.param x, ('_', '0', '1')
.routine sub, tapes=1, alpha=(3), exits=1
.param p, ('_', '0', '1')
.section code
.func main
        jmp     @sub
.func sub
        ret
";
    let err = link(
        &fake_syntax(),
        &[asm(EXIT_JUMP)],
        &[],
        opts(CallMech::Frames),
    )
    .expect_err("a relocated jump into an exit-bearing callee must be refused");
    assert!(
        matches!(&err, LinkError::BadBinding { callee, message }
            if callee == "sub" && message.contains("declares 1")),
        "{err:?}"
    );
}

/// Two warned sites in `main`, one more in a second function `helper`
/// reached through it: `LinkReport.diagnostics` promises function order
/// then blob offset, not scan or discovery order.
///
/// Mutation it catches: collect diagnostics through anything that loses
/// scan order (a `HashSet`, a sort by message text, or visiting `helper`
/// before finishing `main`) and this exact ordering breaks — main is
/// always `order[0]`, so a correct check can never interleave its two
/// sites with `helper`'s one, nor report them out of blob-offset order.
#[test]
fn diagnostics_are_reported_in_function_then_offset_order() {
    const MULTI_WARN: &str = "\
.routine main, tapes=1, alpha=(5)
.routine sub, tapes=1, alpha=(3)
.routine helper, tapes=1, alpha=(5)
.routine subhelp, tapes=1, alpha=(3)
.section code
.func main
        call    sub
        call    sub
        call    helper
        stp
.func sub
        ret
.func helper
        call    subhelp
        ret
.func subhelp
        ret
";
    let out = link(
        &fake_syntax(),
        &[asm(MULTI_WARN)],
        &[],
        opts(CallMech::Frames),
    )
    .expect("warnings do not stop the link");
    let ds = &out.report.diagnostics;
    assert_eq!(ds.len(), 3, "{ds:?}");
    assert_eq!(ds[0].function, "main");
    assert_eq!(ds[1].function, "main");
    assert!(
        ds[0].offset < ds[1].offset,
        "main's two sites must report in blob-offset order: {ds:?}"
    );
    assert_eq!(ds[2].function, "helper");
}

/// An exporter and a consumer that agree, and one that does not. The
/// `.graph` and `.grafted` directives are object-level, before the first
/// `.func` (docs/formats.md (routine interfaces)).
fn exporter(digest: u32) -> String {
    format!(
        "\
.graph lib::findA, {digest}
.routine lib::facade, tapes=1, alpha=(3)
.param t, ('_', '0', '1')
.section code
.func lib::facade
        ret
"
    )
}

fn consumer(digest: u32) -> String {
    format!(
        "\
.grafted lib::findA, {digest}
.routine main, tapes=1, alpha=(3)
.param t, ('_', '0', '1')
.section code
.func main
        call    lib::facade
        stp
"
    )
}

/// Mutation it catches: compare the graph NAMES only and a drifted body
/// links silently — the failure mode the digest exists to prevent.
#[test]
fn a_drifted_graft_digest_is_refused() {
    let err = link(
        &fake_syntax(),
        &[asm(&consumer(42))],
        &[asm(&exporter(7))],
        opts(CallMech::Frames),
    )
    .expect_err("a drifted digest must stop the link");
    assert!(
        matches!(&err, LinkError::GraftDrift { graph, .. } if graph == "lib::findA"),
        "{err:?}"
    );
}

/// Mutation it catches: compare with `!=` inverted and the agreeing pair
/// is refused instead.
#[test]
fn an_agreeing_graft_digest_links() {
    link(
        &fake_syntax(),
        &[asm(&consumer(42))],
        &[asm(&exporter(42))],
        opts(CallMech::Frames),
    )
    .expect("agreeing digests link");
}

/// A header-only library exports nothing to check against. Mutation it
/// catches: treat a missing exporter as a mismatch and every header-only
/// library stops linking.
#[test]
fn a_graft_with_no_exporter_in_the_link_is_unchecked() {
    const ALONE: &str = "\
.grafted lib::findA, 42
.routine main, tapes=1, alpha=(3)
.param t, ('_', '0', '1')
.section code
.func main
        stp
";
    link(&fake_syntax(), &[asm(ALONE)], &[], opts(CallMech::Frames))
        .expect("an unexported graft is not checked");
}

/// A user object that ALSO exports the graph shadows the library's
/// export, exactly as a user symbol shadows a library one: the consumer
/// is checked against the user object's digest, not the library's.
///
/// Mutation it catches: resolve the exporter last-wins (or library-first)
/// and the consumer, which agrees with the user object and disagrees
/// with the library, is refused.
#[test]
fn a_user_export_shadows_a_library_export_for_the_check() {
    const USER_EXPORTER: &str = "\
.graph lib::findA, 42
.routine helper, tapes=1, alpha=(3)
.param t, ('_', '0', '1')
.section code
.func helper
        ret
";
    link(
        &fake_syntax(),
        &[asm(&consumer(42)), asm(USER_EXPORTER)],
        &[asm(&exporter(7))],
        opts(CallMech::Frames),
    )
    .expect("the user object's export wins, and it agrees");
}
