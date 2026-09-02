//! build() is the compile channel plus the link: warnings ride along, a
//! fatal is the one error. The Program carries the map, so the line
//! table and tape layouts the browser needs are queries, not re-parses.

use mtc_wasm::inner::Lang;
use mtc_wasm::inner::diagnostics::Severity;
use mtc_wasm::inner::program::build;

const PMC_INC: &str = "main() {\n    1: right(2);\n    2: check(1, 3);\n    3: mark(4);\n    4: left(5);\n    5: check(4, 6);\n    6: right(!);\n}\n";
const TMC_REPLACE_B: &str = "alphabet ab { '_', 'a', 'b' }\n\nmachine {\n  tape main: ab;\n\n  entry state scan {\n    ['b'] -> write ['a'] move [>] goto scan;\n    ['a'] ->             move [>] goto scan;\n    ['_'] -> stop;\n  }\n}\n";

#[test]
fn pmc_builds_with_a_single_binary_band() {
    let (program, warnings) = build(Lang::Pmc, PMC_INC, 1).unwrap();
    assert!(warnings.iter().all(|d| d.severity == Severity::Warning));
    assert_eq!(program.exe.arch, 0x01);
    let tapes = program.tapes();
    assert_eq!(tapes.len(), 1);
    assert_eq!(
        tapes[0].glyphs,
        vec![" ".to_string(), "*".to_string()],
        "the CLI's PM-1 glyphs"
    );
    assert!(!program.bytes().is_empty());
    assert!(program.map_json().contains("\"functions\""));
    assert!(
        program.disassembly().contains("main"),
        "reassembleable text names main"
    );
}

#[test]
fn tmc_builds_with_named_glyph_bands() {
    let (program, _) = build(Lang::Tmc, TMC_REPLACE_B, 1).unwrap();
    assert_eq!(program.exe.arch, 0x02);
    assert_eq!(program.exe.tape_count, 1);
    let tapes = program.tapes();
    assert_eq!(tapes.len(), 1);
    assert_eq!(tapes[0].name, "main");
    assert_eq!(tapes[0].glyphs, vec!["_", "a", "b"]);
}

#[test]
fn line_table_round_trips_through_the_map() {
    for (lang, src, some_line) in [(Lang::Pmc, PMC_INC, 3u32), (Lang::Tmc, TMC_REPLACE_B, 8u32)] {
        let (program, _) = build(lang, src, 0).unwrap();
        let addr = program
            .address_for_line(some_line)
            .unwrap_or_else(|| panic!("{lang:?}: line {some_line} has an address under -g"));
        let loc = program.line_of(addr).expect("resolves");
        assert_eq!(loc.line, Some(some_line), "{lang:?}");
        assert!(!loc.function.is_empty());
        assert!(program.line_of(0xFFFF_FFF0).is_none(), "outside the image");
    }
}

#[test]
fn a_fatal_is_one_error_and_no_program() {
    let err = build(Lang::Pmc, "main() { nope", 1).unwrap_err();
    assert_eq!(err.severity, Severity::Error);
    let err = build(Lang::Tmc, "alphabet a { '_' }\nmachine {", 1).unwrap_err();
    assert_eq!(err.severity, Severity::Error);
}

#[test]
fn opt_levels_both_build_and_o0_is_not_smaller() {
    let (o0, _) = build(Lang::Tmc, TMC_REPLACE_B, 0).unwrap();
    let (o1, _) = build(Lang::Tmc, TMC_REPLACE_B, 1).unwrap();
    assert!(o0.exe.code.len() >= o1.exe.code.len());
}
