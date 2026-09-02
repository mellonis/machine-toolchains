//! `build` on `.pma`/`.tma`: the assembler with its line table, linked
//! against the stdlib like a source build, the same `Program` out. `check`
//! and `format` follow for both dialects.

use mtc_wasm::inner::diagnostics::{CheckError, CheckOptions, Severity, check, format};
use mtc_wasm::inner::program::{SourceFile, build};
use mtc_wasm::inner::session::{Event, Limits, OutcomeInfo, Seed, Session};
use mtc_wasm::inner::{Arch, Lang};

/// `pmt compile -S` of the increment program: right to the blank, mark it,
/// left to the blank, right, stop.
const PMA_INC: &str = ".func main\nL1:\n        rgt\n        jm      L1\n        wr      1\nL4:\n        lft\n        jm      L4\n        rgt\n        stp\n";
/// Calls the stdlib: to the section's last mark, one right, mark.
const PMA_STD: &str =
    ".func main\n        call    std::goToEnd\n        rgt\n        wr      1\n        stp\n";
/// `tmt compile -S` of the replace-every-b program.
const TMA_RB: &str = ".section tables\nT0:     .row    [0]\n        .row    [1]\n        .row    [2]\nD0:     .targets scan__2, scan__1, scan__0\n.section code\n.routine main, tapes=1, alpha=(3)\n.func main\nscan:\n        rd\n        mtc     T0\n        djmp    D0\nscan__0:\n        wrmv    [1], [>]\n        jmp     scan\nscan__1:\n        wrmv    [-], [>]\n        jmp     scan\nscan__2:\n        stp\n";
const PMC_INC: &str = "main() {\n    1: right(2);\n    2: check(1, 3);\n    3: mark(4);\n    4: left(5);\n    5: check(4, 6);\n    6: right(!);\n}\n";
const TMC_REPLACE_B: &str = "alphabet ab { '_', 'a', 'b' }\n\nmachine {\n  tape main: ab;\n\n  entry state scan {\n    ['b'] -> write ['a'] move [>] goto scan;\n    ['a'] ->             move [>] goto scan;\n    ['_'] -> stop;\n  }\n}\n";

fn seed(cells: &[u8]) -> Seed {
    Seed {
        cells: cells.to_vec(),
        head: 0,
        origin: 0,
    }
}

fn run_to_end(s: &mut Session) -> mtc_wasm::inner::session::Finished {
    match s.pump(None).unwrap() {
        Event::Finished(f) => f,
        other => panic!("unexpected {other:?}"),
    }
}

#[test]
fn lang_has_two_axes() {
    assert_eq!(Lang::parse("pma"), Some(Lang::Pma));
    assert_eq!(Lang::parse("tma"), Some(Lang::Tma));
    assert_eq!(Lang::parse("PMA"), None, "case-sensitive, like the rest");
    for lang in Lang::ALL {
        assert_eq!(Lang::parse(lang.as_str()), Some(lang));
    }
    assert_eq!(Lang::Pma.arch(), Arch::Pm1);
    assert_eq!(Lang::Tma.arch(), Arch::Tm1);
    assert!(Lang::Pma.is_asm() && Lang::Tma.is_asm());
    assert!(!Lang::Pmc.is_asm() && !Lang::Tmc.is_asm());
}

#[test]
fn pma_builds_runs_and_maps_its_physical_lines() {
    let (program, warnings) = build(Lang::Pma, PMA_INC, 1).unwrap();
    assert!(warnings.is_empty(), "the assembler has no warning channel");
    assert_eq!((program.lang, program.arch), (Lang::Pma, Arch::Pm1));
    assert_eq!(program.tapes()[0].glyphs, vec![" ", "*"]);

    let mut s = Session::new(&program, &[seed(&[1, 1, 1])], Limits::default()).unwrap();
    let fin = run_to_end(&mut s);
    assert!(matches!(fin.outcome, OutcomeInfo::Stopped));
    let snap = s.snapshot(0).unwrap();
    assert_eq!(&snap.cells[..4], &[1, 1, 1, 1]);
    assert_eq!(snap.head, 0);

    // Line 3 is `rgt`, the first instruction after the entry; the label
    // line above it compiles to nothing and plants at the same address.
    let addr = program
        .address_for_line(3, SourceFile::User)
        .expect("an instruction line has an address");
    let loc = program.line_of(addr).unwrap();
    assert_eq!((loc.file, loc.line), (SourceFile::User, Some(3)));
    assert_eq!(loc.function, "main");
    assert_eq!(program.address_for_line(2, SourceFile::User), Some(addr));
    let last = program.address_for_line(10, SourceFile::User).unwrap();
    assert_eq!(program.line_of(last).unwrap().line, Some(10), "`stp`");
}

#[test]
fn tma_builds_with_image_labelled_bands() {
    let (program, _) = build(Lang::Tma, TMA_RB, 1).unwrap();
    assert_eq!(program.arch, Arch::Tm1);
    assert_eq!(program.exe.tape_count, 1);
    let tapes = program.tapes();
    assert_eq!(tapes.len(), 1);
    assert_eq!(tapes[0].name, "tape0", "an image carries no names");
    assert_eq!(
        tapes[0].glyphs,
        vec!["0", "1", "2"],
        "decimal labels per cardinality"
    );

    let mut s = Session::new(&program, &[seed(&[2, 2, 2])], Limits::default()).unwrap();
    let fin = run_to_end(&mut s);
    assert!(matches!(fin.outcome, OutcomeInfo::Stopped));
    let snap = s.snapshot(0).unwrap();
    assert_eq!(&snap.cells[..3], &[1, 1, 1]);
    assert_eq!(snap.head, 3);
    assert_eq!(snap.glyphs, vec!["0", "1", "2"]);

    // Line 10 is `rd`, the first instruction of `scan`.
    let addr = program.address_for_line(10, SourceFile::User).unwrap();
    assert_eq!(program.line_of(addr).unwrap().line, Some(10));
}

#[test]
fn an_assembler_refusal_is_one_coded_error_at_its_span() {
    for lang in [Lang::Pma, Lang::Tma] {
        let err = build(lang, ".func main\n        bogus\n", 1).unwrap_err();
        assert_eq!(err.severity, Severity::Error, "{lang:?}");
        assert_eq!(err.code, "unknown-mnemonic", "{lang:?}");
        // ".func main\n" is 11 units; the mnemonic sits after 8 spaces.
        assert_eq!(err.from, 19, "{lang:?}: {}", err.message);
        assert!(err.message.contains("bogus"), "{lang:?}: {}", err.message);
    }
}

#[test]
fn assembly_links_the_stdlib() {
    let (program, _) = build(Lang::Pma, PMA_STD, 1).unwrap();
    let std_fn = program
        .map
        .functions
        .iter()
        .find(|f| f.name == "std::goToEnd")
        .expect("the reachable routine is linked in");
    assert_eq!(std_fn.source.as_deref(), Some("std"));
    let mut s = Session::new(&program, &[seed(&[1, 1])], Limits::default()).unwrap();
    let fin = run_to_end(&mut s);
    assert!(matches!(fin.outcome, OutcomeInfo::Stopped));
    let snap = s.snapshot(0).unwrap();
    assert_eq!(
        &snap.cells[..3],
        &[1, 1, 1],
        "a third mark after the section"
    );
    assert_eq!(snap.head, 2);
}

#[test]
fn opt_level_is_ignored_on_assembly() {
    let (o0, _) = build(Lang::Pma, PMA_INC, 0).unwrap();
    let (o1, _) = build(Lang::Pma, PMA_INC, 1).unwrap();
    assert_eq!(o0.exe.code, o1.exe.code);
}

/// The text-expressibility gate through the browser: what `build` on a
/// source language produces, its own disassembly reassembles to.
#[test]
fn disassembly_reassembles_to_the_same_image() {
    for (src_lang, src, asm_lang) in [
        (Lang::Pmc, PMC_INC, Lang::Pma),
        (Lang::Tmc, TMC_REPLACE_B, Lang::Tma),
    ] {
        let (compiled, _) = build(src_lang, src, 1).unwrap();
        let text = compiled.disassembly();
        let (assembled, _) = build(asm_lang, &text, 1)
            .unwrap_or_else(|e| panic!("{asm_lang:?}: {}\n{text}", e.message));
        assert_eq!(assembled.exe.code, compiled.exe.code, "{asm_lang:?}");
        assert_eq!(assembled.exe.tape_count, compiled.exe.tape_count);
    }
}

#[test]
fn check_on_assembly_is_the_asm_lint_behind_the_assemble_gate() {
    let opts = CheckOptions::default();
    let diags = check(Lang::Pma, ".func main\nL1:\n        stp\n", &opts).unwrap();
    let d = diags
        .iter()
        .find(|d| d.code == "unused-label")
        .expect("core's asm lint runs");
    assert_eq!(d.severity, Severity::Warning);
    assert_eq!(d.from, 11, "the label at the start of line 2");

    let diags = check(Lang::Tma, TMA_RB, &opts).unwrap();
    assert!(
        diags.iter().all(|d| d.severity == Severity::Warning),
        "{diags:?}"
    );

    let fatal = check(Lang::Tma, ".func main\n        bogus\n", &opts).unwrap();
    assert_eq!(fatal.len(), 1);
    assert_eq!(
        (fatal[0].severity, fatal[0].code.as_str()),
        (Severity::Error, "unknown-mnemonic")
    );

    let allowed = CheckOptions {
        allow: vec!["unused-label".to_string()],
        warn: vec![],
    };
    assert!(
        check(Lang::Pma, ".func main\nL1:\n        stp\n", &allowed)
            .unwrap()
            .iter()
            .all(|d| d.code != "unused-label")
    );
    let bogus = CheckOptions {
        allow: vec!["no-such-rule".to_string()],
        warn: vec![],
    };
    for lang in [Lang::Pma, Lang::Tma] {
        assert_eq!(
            check(lang, ".func main\n        stp\n", &bogus).unwrap_err(),
            CheckError::UnknownAllowCode("no-such-rule".to_string()),
            "{lang:?}"
        );
    }
}

#[test]
fn format_on_assembly_is_the_canonical_grid() {
    let once = format(Lang::Pma, ".func main\n  L1:  rgt\n jm L1\n").unwrap();
    assert_eq!(once, ".func main\nL1:     rgt\n        jm      L1\n");
    assert_eq!(format(Lang::Pma, &once).unwrap(), once, "idempotent");
    let tma = format(Lang::Tma, TMA_RB).unwrap();
    assert_eq!(tma, TMA_RB, "`compile -S` output is already canonical");
    // Whitespace-only: a semantic refusal (an unknown mnemonic) is not the
    // formatter's to make; a line that is not assembly-shaped is.
    assert_eq!(
        format(Lang::Tma, ".func main\n        bogus\n").unwrap(),
        ".func main\n        bogus\n"
    );
    let err = format(Lang::Tma, "  ???\n").unwrap_err();
    assert_eq!((err.code.as_str(), err.from), ("raw-line", 2));
}
