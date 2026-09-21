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

/// Mutation it catches: compare only the alphabet NAMES and a drifted
/// glyph list links silently — the exact hazard item 4 demonstrated.
#[test]
fn a_drifted_imported_alphabet_is_refused() {
    let mut lib = asm(&exporter(1));
    lib.interface
        .as_mut()
        .expect("the exporter declares an interface")
        .alphabets
        .push(mtc_core::formats::object::ExportedAlphabet {
            name: "lib::bits".to_string(),
            glyphs: vec!["_".into(), "0".into(), "1".into()],
        });
    let mut app = asm(&consumer(1));
    app.interface
        .as_mut()
        .expect("the consumer declares an interface")
        .imports
        .push(mtc_core::formats::object::ImportedAlphabet {
            name: "lib::bits".to_string(),
            glyphs: vec!["_".into(), "1".into(), "0".into()],
        });
    let err = link(&fake_syntax(), &[app], &[lib], opts(CallMech::Frames))
        .expect_err("a drifted import must stop the link");
    assert!(
        matches!(&err, LinkError::AlphabetDrift { position: 1, .. }),
        "{err:?}"
    );
}

/// Mutation it catches: compare with `!=` inverted and the agreeing pair
/// is refused instead.
#[test]
fn an_agreeing_imported_alphabet_links() {
    let mut lib = asm(&exporter(1));
    lib.interface
        .as_mut()
        .expect("the exporter declares an interface")
        .alphabets
        .push(mtc_core::formats::object::ExportedAlphabet {
            name: "lib::bits".to_string(),
            glyphs: vec!["_".into(), "0".into(), "1".into()],
        });
    let mut app = asm(&consumer(1));
    app.interface
        .as_mut()
        .expect("the consumer declares an interface")
        .imports
        .push(mtc_core::formats::object::ImportedAlphabet {
            name: "lib::bits".to_string(),
            glyphs: vec!["_".into(), "0".into(), "1".into()],
        });
    link(&fake_syntax(), &[app], &[lib], opts(CallMech::Frames))
        .expect("agreeing imported alphabets link");
}

/// The imported list is a strict PREFIX of the exported one: `zip` stops
/// at the shorter list and reports no difference, so `position` must
/// come from the `.or_else` arm — the shorter list's length — rather
/// than `None`. Mutation it catches: drop the `.or_else` arm and a
/// truncated import links silently instead of being refused.
#[test]
fn a_prefix_imported_alphabet_is_refused() {
    let mut lib = asm(&exporter(1));
    lib.interface
        .as_mut()
        .expect("the exporter declares an interface")
        .alphabets
        .push(mtc_core::formats::object::ExportedAlphabet {
            name: "lib::bits".to_string(),
            glyphs: vec!["_".into(), "0".into(), "1".into()],
        });
    let mut app = asm(&consumer(1));
    app.interface
        .as_mut()
        .expect("the consumer declares an interface")
        .imports
        .push(mtc_core::formats::object::ImportedAlphabet {
            name: "lib::bits".to_string(),
            glyphs: vec!["_".into(), "0".into()],
        });
    let err = link(&fake_syntax(), &[app], &[lib], opts(CallMech::Frames))
        .expect_err("a truncated import must stop the link");
    assert!(
        matches!(&err, LinkError::AlphabetDrift { position: 2, .. }),
        "{err:?}"
    );
}

// --- `tail-call-no-continuation` --------------------------------------
//
// A call site with no continuation — either it is literally the last
// instruction of its function, or it is followed only by the dialect's
// own safety trap — into a callee that CAN return
// (docs/core.md (link warnings)).

/// A `call` as the function's LAST instruction, into a callee that ends
/// in `ret`: the bare shape, no trap involved.
///
/// Mutation it catches: drop the "bare" disjunct (only ever consult
/// [`is_lone_trap`]) and this fixture — whose blob ends right at the
/// call, with nothing after it to decode at all — goes silent.
#[test]
fn a_bare_tail_call_into_a_returning_callee_warns() {
    const SRC: &str = "\
.routine main, tapes=1, alpha=(3)
.routine sub, tapes=1, alpha=(3)
.section code
.func main
        call    sub
.func sub
        ret
";
    let out = link(&fake_syntax(), &[asm(SRC)], &[], opts(CallMech::Frames))
        .expect("a warning does not stop the link");
    assert!(
        out.report
            .diagnostics
            .iter()
            .any(|d| d.code == "tail-call-no-continuation"),
        "{:?}",
        out.report.diagnostics
    );
}

/// The same call, with a `stp` after it: an ordinary continuation, not a
/// tail call at all.
///
/// Mutation it catches: in [`is_lone_trap`]'s wire, drop the opcode
/// match (treat ANY single instruction after the call as if it were the
/// dialect's trap) and this fixture — whose `stp` is the whole rest of
/// the blob, so "one instruction, then the end" still holds — warns.
#[test]
fn a_call_followed_by_an_ordinary_instruction_is_silent() {
    const SRC: &str = "\
.routine main, tapes=1, alpha=(3)
.routine sub, tapes=1, alpha=(3)
.section code
.func main
        call    sub
        stp
.func sub
        ret
";
    let out = link(&fake_syntax(), &[asm(SRC)], &[], opts(CallMech::Frames)).expect("links");
    assert!(
        out.report
            .diagnostics
            .iter()
            .all(|d| d.code != "tail-call-no-continuation"),
        "{:?}",
        out.report.diagnostics
    );
}

/// The bare shape into an HONEST `noreturn` callee: the header says
/// `noreturn` and the body backs it up (no `ret` anywhere), so there is
/// truly nothing to warn about.
///
/// Mutation it catches: drop the `callee_can_return` guard entirely
/// (never consult the callee at all) and this fixture, which has no
/// trap to fall back on either, warns.
#[test]
fn a_bare_tail_call_into_an_honest_noreturn_callee_is_silent() {
    const SRC: &str = "\
.routine main, tapes=1, alpha=(3)
.param t, ('_', '0', '1')
.routine sub, tapes=1, alpha=(3), noreturn
.param p, ('_', '0', '1')
.section code
.func main
        call    sub
.func sub
        stp
";
    let out = link(&fake_syntax(), &[asm(SRC)], &[], opts(CallMech::Frames)).expect("links");
    assert!(
        out.report
            .diagnostics
            .iter()
            .all(|d| d.code != "tail-call-no-continuation"),
        "{:?}",
        out.report.diagnostics
    );
}

/// The bare shape into a LYING `noreturn` callee: the header says
/// `noreturn`, but the body holds a live `ret` — `callee_can_return`
/// reads the body, exactly as `check_splice_site`'s exit-bearing refusal
/// already does (mirrors `link_exits.rs`'s
/// `a_noreturn_header_does_not_excuse_a_body_that_returns`, which pins
/// the same fact for the OTHER caller of `callee_can_return`).
///
/// Mutation it catches: trust the interface's `returns` bit alone (skip
/// the body scan) inside `callee_can_return` and this fixture — where
/// the bit lies — goes silent.
#[test]
fn a_lying_noreturn_callee_still_warns_on_a_bare_tail_call() {
    const SRC: &str = "\
.routine main, tapes=1, alpha=(3)
.param t, ('_', '0', '1')
.routine sub, tapes=1, alpha=(3), noreturn
.param p, ('_', '0', '1')
.section code
.func main
        call    sub
.func sub
        ret
";
    let out = link(&fake_syntax(), &[asm(SRC)], &[], opts(CallMech::Frames))
        .expect("a warning does not stop the link");
    assert!(
        out.report
            .diagnostics
            .iter()
            .any(|d| d.code == "tail-call-no-continuation"),
        "{:?}",
        out.report.diagnostics
    );
}

/// A call followed by a LONE safety trap — the shape a compiler emits
/// for a call written with no `then` (docs/core.md (link warnings)):
/// nothing after the call but the dialect's own trap, and the trap is
/// itself the function's last instruction.
///
/// Mutation it catches: drop the lone-trap arm entirely (only ever
/// consult "bare") and this fixture — whose call is NOT itself the last
/// instruction, the trap is — goes silent.
#[test]
fn a_lone_safety_trap_after_a_call_warns() {
    const SRC: &str = "\
.routine main, tapes=1, alpha=(3)
.routine sub, tapes=1, alpha=(3)
.section code
.func main
        call    sub
        trap    #0
.func sub
        ret
";
    let out = link(&fake_syntax(), &[asm(SRC)], &[], opts(CallMech::Frames))
        .expect("a warning does not stop the link");
    assert!(
        out.report
            .diagnostics
            .iter()
            .any(|d| d.code == "tail-call-no-continuation"),
        "{:?}",
        out.report.diagnostics
    );
}

/// A call, a trap, and one MORE instruction: the trap is no longer the
/// function's last instruction, so it is not a lone safety trap — an
/// ordinary (if unusual) continuation.
///
/// Mutation it catches: in [`is_lone_trap`], drop the "last instruction
/// of the blob" check (accept a trap anywhere right after the call) and
/// this fixture, whose trap has a `stp` after it, warns.
#[test]
fn a_trap_followed_by_more_code_is_not_a_lone_trap() {
    const SRC: &str = "\
.routine main, tapes=1, alpha=(3)
.routine sub, tapes=1, alpha=(3)
.section code
.func main
        call    sub
        trap    #0
        stp
.func sub
        ret
";
    let out = link(&fake_syntax(), &[asm(SRC)], &[], opts(CallMech::Frames)).expect("links");
    assert!(
        out.report
            .diagnostics
            .iter()
            .all(|d| d.code != "tail-call-no-continuation"),
        "{:?}",
        out.report.diagnostics
    );
}

/// A lone safety trap after a call into a callee that CANNOT return: the
/// honest-`noreturn` shape again, reached through the trap arm instead
/// of the bare one.
///
/// Mutation it catches: drop the `callee_can_return` guard entirely and
/// this fixture — whose trap would otherwise satisfy the lone-trap arm
/// on its own — warns regardless of the callee.
#[test]
fn a_lone_trap_after_a_call_into_a_noreturn_callee_is_silent() {
    const SRC: &str = "\
.routine main, tapes=1, alpha=(3)
.param t, ('_', '0', '1')
.routine sub, tapes=1, alpha=(3), noreturn
.param p, ('_', '0', '1')
.section code
.func main
        call    sub
        trap    #0
.func sub
        stp
";
    let out = link(&fake_syntax(), &[asm(SRC)], &[], opts(CallMech::Frames)).expect("links");
    assert!(
        out.report
            .diagnostics
            .iter()
            .all(|d| d.code != "tail-call-no-continuation"),
        "{:?}",
        out.report.diagnostics
    );
}

/// The lone-trap match is by OPCODE alone — never the immediate, which a
/// compiler may spend on any trap kind. `#7` names no kind this dialect
/// defines; the check does not care.
///
/// Mutation it catches: require the immediate to be `#0` inside
/// `is_lone_trap` and this fixture, whose trap carries `#7`, goes
/// silent.
#[test]
fn the_lone_trap_match_ignores_the_immediate() {
    const SRC: &str = "\
.routine main, tapes=1, alpha=(3)
.routine sub, tapes=1, alpha=(3)
.section code
.func main
        call    sub
        trap    #7
.func sub
        ret
";
    let out = link(&fake_syntax(), &[asm(SRC)], &[], opts(CallMech::Frames))
        .expect("a warning does not stop the link");
    assert!(
        out.report
            .diagnostics
            .iter()
            .any(|d| d.code == "tail-call-no-continuation"),
        "{:?}",
        out.report.diagnostics
    );
}

/// A RELOCATED TAIL JUMP as the function's last instruction:
/// `SiteKind::Plain` covers a genuine call and a jump the tail-call pass
/// substituted for one alike, but a jump pushes no return address — there
/// is no continuation to miss, and ending right there is exactly how
/// tail-call elimination is supposed to look.
///
/// Mutation it catches: drop the `is_call` guard in
/// `tail_call_no_continuation` and this fixture — a bare `jmp`, which
/// `continuation` cannot distinguish from a bare `call` by offset math
/// alone — warns.
#[test]
fn a_relocated_tail_jump_is_not_a_bare_tail_call() {
    const SRC: &str = "\
.routine main, tapes=1, alpha=(3)
.routine sub, tapes=1, alpha=(3)
.section code
.func main
        jmp     @sub
.func sub
        ret
";
    let out = link(&fake_syntax(), &[asm(SRC)], &[], opts(CallMech::Frames)).expect("links");
    assert!(
        out.report
            .diagnostics
            .iter()
            .all(|d| d.code != "tail-call-no-continuation"),
        "{:?}",
        out.report.diagnostics
    );
}

/// A BOUND call declaring exits, in tail position, under FRAMES: a
/// framed call never falls through at all (it leaves through the exit
/// vector's runtime dispatch, not a pushed return address), so tail
/// position costs it nothing — the same shape `link_exits.rs`'s
/// `TAIL_POSITION_NORETURN` proves at the executable level, from the
/// diagnostics side instead.
///
/// Mutation it catches: drop the `record.exits.is_empty()` gate on the
/// `SiteKind::Bound` arm (grade every bound site the same as a
/// transparent one) and this fixture, exit-bearing and tail-positioned
/// on purpose, warns.
#[test]
fn an_exit_bearing_tail_position_bound_call_does_not_warn_under_frames() {
    const SRC: &str = "\
.routine main, tapes=1, alpha=(3)
.param t, ('_', '0', '1')
.routine sub, tapes=1, alpha=(3), exits=1
.param p, ('_', '0', '1')
.section code
.func main
won:    nop
        call    sub [0] exits=(won)
.func sub
        ret
";
    let out = link(&fake_syntax(), &[asm(SRC)], &[], opts(CallMech::Frames)).expect("links");
    assert!(
        out.report
            .diagnostics
            .iter()
            .all(|d| d.code != "tail-call-no-continuation"),
        "{:?}",
        out.report.diagnostics
    );
}
