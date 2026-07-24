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

/// The directive words a grammar rule's match pattern paints, regex
/// escapes resolved. Directive rules are either `.word\b…` or
/// `.(a|b|c)\b`-shaped; `\b` is replaced first — stripping every
/// backslash blindly would fuse the word boundary's `b` onto the
/// directive name (`.byte\b` → `.byteb`).
fn directive_words(pattern: &str) -> Vec<String> {
    let bare = pattern.replace("\\b", " ").replace('\\', "");
    let start = bare.find('.').expect("a directive pattern names a word");
    let rest = &bare[start + 1..];
    if let Some(stripped) = rest.strip_prefix('(') {
        let close = stripped
            .find(')')
            .expect("a grouped directive pattern closes");
        stripped[..close]
            .split('|')
            .map(|word| format!(".{word}"))
            .collect()
    } else {
        let end = rest
            .find(|c: char| !c.is_ascii_alphanumeric())
            .unwrap_or(rest.len());
        vec![format!(".{}", &rest[..end])]
    }
}

/// The `.pma` grammar's directive rules must paint EXACTLY the directive
/// words core's assembler framework recognizes under PM-1's caps —
/// `mtc_core::asm::recognized_directives` over `pm1_syntax()`'s
/// caps-off surface, which is `.func` and `.byte` alone. Set equality
/// makes the guard bidirectional: a directive added to the
/// caps-independent assembler surface with no `.pma` grammar entry
/// fails, and a directive invented in the grammar fails too. Every
/// directive lives in a repository rule whose key ends with `Directive`;
/// the guard reads them by that naming convention (the mirror of the
/// `.tma` guard in the turing-machine crate).
#[test]
fn pma_grammar_directives_match_the_recognized_inventory() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../editors/grammars/pma.tmLanguage.json"
    );
    let text = std::fs::read_to_string(path).expect("shared pma grammar exists");
    let json: serde_json::Value = serde_json::from_str(&text).expect("pma grammar is valid JSON");
    let repository = json["repository"]
        .as_object()
        .expect("pma grammar has a repository");
    let mut in_grammar: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    let mut rules_seen = 0;
    for (key, rule) in repository {
        if !key.ends_with("Directive") {
            continue;
        }
        rules_seen += 1;
        let pattern = rule["match"]
            .as_str()
            .unwrap_or_else(|| panic!("pma grammar has a `{key}` match pattern"));
        for word in directive_words(pattern) {
            assert!(
                in_grammar.insert(word.clone()),
                "pma grammar lists `{word}` in more than one directive rule"
            );
        }
    }
    assert!(
        rules_seen >= 2,
        "pma grammar keeps its directives in `*Directive` repository rules \
         (found {rules_seen}) — the guard reads them by that naming convention"
    );

    let recognized: std::collections::BTreeSet<String> =
        mtc_core::asm::recognized_directives(mtc_post_machine::asm::pm1_syntax().caps)
            .into_iter()
            .map(str::to_string)
            .collect();
    assert_eq!(
        in_grammar, recognized,
        "the pma grammar's directive rules and core's recognized-directive \
         inventory must agree exactly"
    );
}
