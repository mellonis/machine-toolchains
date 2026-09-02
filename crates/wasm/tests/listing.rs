//! The structured listing is the ip view's data: one row per instruction,
//! covering the image exactly once, agreeing with the text listing.

use mtc_wasm::inner::Lang;
use mtc_wasm::inner::listing::rows;
use mtc_wasm::inner::program::build;

const PMC_INC: &str = "main() {\n    1: right(2);\n    2: check(1, 3);\n    3: mark(4);\n    4: left(5);\n    5: check(4, 6);\n    6: right(!);\n}\n";
const TMC_REPLACE_B: &str = "alphabet ab { '_', 'a', 'b' }\n\nmachine {\n  tape main: ab;\n\n  entry state scan {\n    ['b'] -> write ['a'] move [>] goto scan;\n    ['a'] ->             move [>] goto scan;\n    ['_'] -> stop;\n  }\n}\n";

#[test]
fn rows_tile_the_code_image_exactly() {
    for (lang, src) in [(Lang::Pmc, PMC_INC), (Lang::Tmc, TMC_REPLACE_B)] {
        let (program, _) = build(lang, src, 1).unwrap();
        let rows = rows(&program);
        assert!(!rows.is_empty(), "{lang:?}");
        assert_eq!(rows[0].addr, 0);
        let mut expected_next = 0u32;
        for row in &rows {
            assert_eq!(row.addr, expected_next, "{lang:?}: rows are contiguous");
            assert!(!row.mnemonic.is_empty());
            assert!(!row.bytes.is_empty());
            expected_next = row.addr + row.bytes.split_whitespace().count() as u32;
        }
        assert_eq!(
            expected_next as usize,
            program.exe.code.len(),
            "{lang:?}: ends at the image end"
        );
    }
}

#[test]
fn function_starts_are_labelled_with_their_names() {
    let (program, _) = build(Lang::Pmc, PMC_INC, 1).unwrap();
    let rows = rows(&program);
    let main_start = program
        .map
        .functions
        .iter()
        .find(|f| f.name == "main")
        .unwrap()
        .start;
    let row = rows
        .iter()
        .find(|r| r.addr == main_start)
        .expect("a row starts main");
    assert_eq!(row.function.as_deref(), Some("main"));
    assert!(
        rows.iter().all(|r| r.function.is_some()),
        "every row knows its function"
    );
}

#[test]
fn every_mnemonic_appears_in_the_text_listing() {
    let (program, _) = build(Lang::Tmc, TMC_REPLACE_B, 1).unwrap();
    let text = mtc_turing_machine::asm::listing_executable(&program.exe, Some(&program.map));
    for row in rows(&program) {
        assert!(
            text.contains(&row.mnemonic),
            "{} missing from the text listing",
            row.mnemonic
        );
    }
}
