//! Core's glyph-list parser and the `.tmc` alphabet resolver must accept the
//! same notation and produce the same glyphs. The CLI's `--alphabet` uses the
//! former; a program's `alphabet { … }` uses the latter. They are separate
//! implementations, so this pins them together.

use mtc_core::formats::parse_glyph_list;
use mtc_turing_machine::compiler::machine_tape_layout;

/// Every case is a body legal inside `alphabet name { … }`.
const CORPUS: &[&str] = &[
    "' ','s','b','k','1'",
    "' ','1'",
    "'0'..'9'",
    "0..7",
    "' ','a'..'e','z'",
    "'ab','c'",
    r"' ','\'','\\'",
    "05,'x'",
    "' ','0'..'3',9",
];

fn probe_source(body: &str) -> String {
    format!(
        "alphabet probe {{ {body} }}\nmachine {{ tape t: probe;\n  entry state s {{ [*] -> stop; }}\n}}\n"
    )
}

fn glyphs_via_tmc(body: &str) -> Vec<String> {
    let layout = machine_tape_layout(&probe_source(body))
        .unwrap_or_else(|e| panic!("`{body}` did not resolve through .tmc: {e:?}"))
        .expect("the probe source declares a machine block");
    layout
        .into_iter()
        .next()
        .expect("the machine declares one tape")
        .glyphs
}

#[test]
fn core_parser_agrees_with_the_tmc_resolver_on_every_corpus_entry() {
    for body in CORPUS {
        let via_core = parse_glyph_list(body)
            .unwrap_or_else(|e| panic!("`{body}` rejected by core's parser: {e}"));
        let via_tmc = glyphs_via_tmc(body);
        assert_eq!(
            via_core, via_tmc,
            "glyph notation drifted between core and .tmc for `{body}`"
        );
    }
}

/// The inverse direction: what one rejects, the other must reject too.
#[test]
fn both_reject_the_same_malformed_lists() {
    const BAD: &[&str] = &["'a','a'", "'9'..'0'", "'ab'..'z'", "'a'..3", "'a"];
    for body in BAD {
        assert!(
            parse_glyph_list(body).is_err(),
            "core's parser accepted malformed `{body}`"
        );
        assert!(
            machine_tape_layout(&probe_source(body)).is_err(),
            ".tmc accepted malformed `{body}`"
        );
    }
}

#[test]
fn tape_layout_reports_names_and_glyphs_in_declaration_order() {
    let source = "\
alphabet mainAlpha { ' ', 's', 'b', 'k', '1' }
alphabet workAlpha { ' ', '1' }

machine {
  tape main: mainAlpha;
  tape cnt:  workAlpha;

  entry state s { [*, *] -> stop; }
}
";
    let layout = machine_tape_layout(source)
        .expect("resolves")
        .expect("declares a machine block");
    let names: Vec<&str> = layout.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(names, vec!["main", "cnt"]);
    assert_eq!(layout[0].glyphs, vec![" ", "s", "b", "k", "1"]);
    assert_eq!(layout[1].glyphs, vec![" ", "1"]);
}

/// A library is a legitimate source with no single band to describe, so it is
/// `Ok(None)` rather than an error — the caller decides whether it can use it.
#[test]
fn a_source_with_no_machine_block_reports_none() {
    let source =
        "alphabet a { ' ', '1' }\nroutine r(tape t: a) { entry state s { [*] -> stop; } }\n";
    assert_eq!(machine_tape_layout(source).expect("resolves"), None);
}
