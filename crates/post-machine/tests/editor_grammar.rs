//! The shared TextMate grammar must stay valid JSON and cover exactly the
//! command vocabulary the parser reserves — a RESERVED change must touch
//! the grammar in the same commit.

#[test]
fn textmate_grammar_is_valid_and_covers_the_reserved_words() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../editors/grammars/pmc.tmLanguage.json"
    );
    let text = std::fs::read_to_string(path).expect("shared grammar exists");
    let json: serde_json::Value = serde_json::from_str(&text).expect("grammar is valid JSON");
    assert_eq!(json["scopeName"], "source.pmc");
    for word in mtc_post_machine::parser::RESERVED {
        assert!(text.contains(word), "grammar misses reserved word `{word}`");
    }
    for word in ["use", "namespace", "export", "as"] {
        assert!(text.contains(word), "grammar misses keyword `{word}`");
    }
}

/// Mirrors the `.pmc` guard above for the `.pma` assembly grammar: the
/// mnemonic vocabulary is generated from `pm1_syntax()` (not hardcoded)
/// so a future mnemonic addition to the arch table fails this test until
/// the grammar catches up.
///
/// Coverage is checked against `text` with JSON's doubled backslashes
/// (`\\.` — how JSON encodes a single regex-escaped `\.`) collapsed to
/// one, so dotted mnemonics like `jm.s` are found as a contiguous
/// substring even though the grammar correctly escapes the `.` for
/// Oniguruma. This does not weaken the guard: the grammar must still
/// spell every mnemonic, escaped or not, somewhere in the file.
#[test]
fn pma_grammar_is_valid_and_covers_pm1_mnemonics() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../editors/grammars/pma.tmLanguage.json"
    );
    let text = std::fs::read_to_string(path).expect("shared pma grammar exists");
    let json: serde_json::Value = serde_json::from_str(&text).expect("pma grammar is valid JSON");
    assert_eq!(json["scopeName"], "source.pma");
    let unescaped = text.replace("\\\\", "");
    for entry in mtc_post_machine::asm::pm1_syntax().entries {
        assert!(
            unescaped.contains(entry.mnemonic),
            "pma grammar misses mnemonic `{}`",
            entry.mnemonic
        );
    }
    for word in [".func", ".byte", "local"] {
        assert!(
            unescaped.contains(word),
            "pma grammar misses keyword `{word}`"
        );
    }
}
