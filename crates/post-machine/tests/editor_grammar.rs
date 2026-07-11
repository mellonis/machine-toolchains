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
