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
/// Coverage is checked against the specific repository pattern that must
/// carry each word — NOT the whole file text. A whole-file check is
/// blind to a deletion whenever the word also appears in prose (the
/// grammar's top-level `comment` field mentions `jm.s`, `.func`, and
/// `local`), so it would stay green with the actual pattern gutted.
/// Each parsed pattern string has its regex escapes (`\.`, `\b`)
/// stripped so a dotted mnemonic like `jm.s` is found as a contiguous
/// substring of the alternation it must live in.
#[test]
fn pma_grammar_is_valid_and_covers_pm1_mnemonics() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../editors/grammars/pma.tmLanguage.json"
    );
    let text = std::fs::read_to_string(path).expect("shared pma grammar exists");
    let json: serde_json::Value = serde_json::from_str(&text).expect("pma grammar is valid JSON");
    assert_eq!(json["scopeName"], "source.pma");
    let pattern = |rule: &str| {
        json["repository"][rule]["match"]
            .as_str()
            .unwrap_or_else(|| panic!("pma grammar has a `{rule}` match pattern"))
            .replace('\\', "")
    };
    let mnemonics = pattern("mnemonics");
    for entry in mtc_post_machine::asm::pm1_syntax().entries {
        assert!(
            mnemonics.contains(entry.mnemonic),
            "pma mnemonics pattern misses `{}`",
            entry.mnemonic
        );
    }
    let func_directive = pattern("funcDirective");
    for word in [".func", "local"] {
        assert!(
            func_directive.contains(word),
            "pma funcDirective pattern misses `{word}`"
        );
    }
    assert!(
        pattern("byteDirective").contains(".byte"),
        "pma byteDirective pattern misses `.byte`"
    );
}
