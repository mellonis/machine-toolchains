//! Drift guards for the two shared TextMate grammars the TM editor plugins
//! ship (`editors/grammars/tmc.tmLanguage.json` and `tma.tmLanguage.json`).
//!
//! Both guards are generated from the language's own source of truth rather
//! than a second hand-written list, and both check SET EQUALITY rather than
//! one-directional coverage: a word added to the language with no grammar
//! entry fails, a grammar entry deleted while the language kept the word
//! fails, and a word invented in the grammar that the language does not have
//! fails too.
//!
//! Coverage is asserted against the specific repository pattern that must
//! carry each word, never the whole file text — a whole-file check is blind
//! to a gutted pattern whenever the word also appears in the grammar's own
//! prose `comment` fields (both grammars have several).

use std::collections::BTreeSet;

/// Reads a grammar, asserts it is valid JSON, and asserts its `scopeName`.
fn load(file: &str, scope: &str) -> serde_json::Value {
    let path = format!(
        "{}/../../editors/grammars/{file}",
        env!("CARGO_MANIFEST_DIR")
    );
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
    let json: serde_json::Value =
        serde_json::from_str(&text).unwrap_or_else(|e| panic!("{path} is valid JSON: {e}"));
    assert_eq!(json["scopeName"], scope, "{path} scopeName");
    json
}

/// The words of a `\b(a|b|c)\b`-shaped alternation, regex escapes stripped.
/// Panics if the pattern is not of that shape — a rule that stops being a
/// plain alternation must be reflected here deliberately, not silently
/// dropped from the guard's view.
fn alternation_words(pattern: &str) -> Vec<String> {
    let open = pattern
        .find('(')
        .unwrap_or_else(|| panic!("pattern `{pattern}` has an alternation group"));
    let close = pattern
        .rfind(')')
        .unwrap_or_else(|| panic!("pattern `{pattern}` closes its alternation group"));
    pattern[open + 1..close]
        .split('|')
        .map(|w| w.replace('\\', ""))
        .collect()
}

/// The `.tmc` grammar must carry EXACTLY the reserved keyword set. Every
/// reserved word lives in a repository rule whose key starts with `keyword`;
/// this collects those rules' alternations and compares the union with
/// [`mtc_turing_machine::lexer::RESERVED`] as a set.
///
/// What it cannot catch: whether the scope *names* a keyword is painted with
/// are the ones an editor theme colors well, and whether the non-keyword
/// rules (glyph literals, the `?` / `!` doc lines, `->` / `=>` / `..`,
/// interpolation braces) still match what the lexer produces — those are
/// visual, and the manual checklist in each plugin's README covers them.
/// The `declaration` rule deliberately repeats a few keywords in a capture
/// so the name after them colors as an entity; that copy is outside this set
/// and may lag without failing here.
#[test]
fn tmc_grammar_covers_exactly_the_reserved_keywords() {
    let json = load("tmc.tmLanguage.json", "source.tmc");
    let repository = json["repository"]
        .as_object()
        .expect("tmc grammar has a repository");

    let mut in_grammar: BTreeSet<String> = BTreeSet::new();
    let mut rules_seen = 0;
    for (key, rule) in repository {
        if !key.starts_with("keyword") {
            continue;
        }
        rules_seen += 1;
        let pattern = rule["match"]
            .as_str()
            .unwrap_or_else(|| panic!("tmc rule `{key}` has a match pattern"));
        for word in alternation_words(pattern) {
            assert!(
                in_grammar.insert(word.clone()),
                "tmc grammar lists `{word}` in more than one keyword rule"
            );
        }
    }
    assert!(
        rules_seen >= 2,
        "tmc grammar keeps its keywords in `keyword*` repository rules \
         (found {rules_seen}) — the guard reads them by that naming convention"
    );

    let reserved: BTreeSet<String> = mtc_turing_machine::lexer::RESERVED
        .iter()
        .map(|w| (*w).to_string())
        .collect();
    assert_eq!(
        in_grammar, reserved,
        "the tmc grammar's keyword rules and lexer::RESERVED must agree exactly"
    );
}

/// The `.tmc` grammar's non-keyword rules an editor visibly depends on must
/// still exist. This is a structural presence check, not a behavioral one —
/// it fails on a deleted rule, not on a subtly wrong regex.
#[test]
fn tmc_grammar_keeps_its_non_keyword_rules() {
    let json = load("tmc.tmLanguage.json", "source.tmc");
    for rule in [
        "comments",
        "docLine",
        "attentionLine",
        "declaration",
        "glyph",
        "interpolation",
        "operators",
        "wildcard",
        "number",
        "punctuation",
    ] {
        assert!(
            !json["repository"][rule].is_null(),
            "tmc grammar misses the `{rule}` rule"
        );
    }
    // `[deprecated]` is an attribute word inside an attention line, not a
    // reserved keyword, so it is invisible to the RESERVED set check above.
    let attention = serde_json::to_string(&json["repository"]["attentionLine"])
        .expect("attentionLine serializes");
    assert!(
        attention.contains("deprecated"),
        "tmc attentionLine rule misses the `[deprecated]` attribute"
    );
}

/// The `.tma` grammar's mnemonic alternation must be exactly `tm1_syntax()`'s
/// mnemonic table — generated from the arch table, never hand-listed, so a
/// new opcode fails this test until the grammar catches up.
///
/// Ordering is checked too: a dotted form (`call.m`) must precede its bare
/// prefix (`call`) in the alternation. `.` is not a word character, so a
/// trailing `\b` does not stop `call` from winning inside `call.m` under
/// Oniguruma's first-match alternation.
#[test]
fn tma_grammar_covers_exactly_the_tm1_mnemonics() {
    let json = load("tma.tmLanguage.json", "source.tma");
    let pattern = json["repository"]["mnemonics"]["match"]
        .as_str()
        .expect("tma grammar has a mnemonics match pattern");
    let listed = alternation_words(pattern);

    let in_grammar: BTreeSet<String> = listed.iter().cloned().collect();
    let from_arch: BTreeSet<String> = mtc_turing_machine::tm1_syntax()
        .entries
        .iter()
        .map(|e| e.mnemonic.to_string())
        .collect();
    assert_eq!(
        in_grammar, from_arch,
        "the tma grammar's mnemonic alternation and tm1_syntax() must agree exactly"
    );

    for (i, earlier) in listed.iter().enumerate() {
        for later in &listed[i + 1..] {
            assert!(
                !later.starts_with(earlier.as_str()),
                "tma mnemonic alternation lists `{earlier}` before `{later}`; \
                 the longer form must come first or it is never matched"
            );
        }
    }
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

/// The `.tma` grammar's directive rules must paint EXACTLY the directive
/// words core's assembler framework recognizes under TM-1's caps —
/// `mtc_core::asm::recognized_directives` is the generated source of
/// truth, so a directive added to the assembler with no grammar entry
/// fails, a grammar rule deleted while the assembler kept the word
/// fails, and a directive invented in the grammar fails too. Every
/// directive lives in a repository rule whose key ends with `Directive`;
/// the guard reads them by that naming convention (the mirror of the
/// `keyword*` convention in the tmc guard above).
///
/// A second, behavioral layer probes the real assembler with each word
/// and asserts its complaint — if any — is not "unknown mnemonic" naming
/// it; an arity or discipline complaint proves the word reached a real
/// directive handler. This keeps the inventory honest from the other
/// side: a word added to the inventory without a real recognizer cannot
/// be laundered through the grammar by adding it there too.
#[test]
fn tma_grammar_directives_match_the_recognized_inventory() {
    let json = load("tma.tmLanguage.json", "source.tma");
    let repository = json["repository"]
        .as_object()
        .expect("tma grammar has a repository");
    let mut in_grammar: BTreeSet<String> = BTreeSet::new();
    let mut rules_seen = 0;
    for (key, rule) in repository {
        if !key.ends_with("Directive") {
            continue;
        }
        rules_seen += 1;
        let pattern = rule["match"]
            .as_str()
            .unwrap_or_else(|| panic!("tma grammar has a `{key}` match pattern"));
        for word in directive_words(pattern) {
            assert!(
                in_grammar.insert(word.clone()),
                "tma grammar lists `{word}` in more than one directive rule"
            );
        }
    }
    assert!(
        rules_seen >= 7,
        "tma grammar keeps its directives in `*Directive` repository rules \
         (found {rules_seen}) — the guard reads them by that naming convention"
    );

    let recognized: BTreeSet<String> =
        mtc_core::asm::recognized_directives(mtc_turing_machine::tm1_syntax().caps)
            .into_iter()
            .map(str::to_string)
            .collect();
    assert_eq!(
        in_grammar, recognized,
        "the tma grammar's directive rules and core's recognized-directive \
         inventory must agree exactly"
    );

    for directive in &in_grammar {
        // `.rept` / `.endr` are only ever directives as a matched PAIR — the
        // assembly CST recognizes the block, not either line alone — so their
        // probe supplies both. Every other directive stands on its own line;
        // the ones that additionally need `.section tables` around them answer
        // with a table-discipline complaint rather than "unknown mnemonic",
        // which is all this probe asks for.
        let source = match directive.as_str() {
            ".rept" | ".endr" => ".func probe\n.rept v, 0, 0\nnop\n.endr\nstp\n".to_string(),
            // Table-space directives are only directives inside the table
            // section; in code they are ordinary unknown words.
            ".row" | ".target" | ".targets" | ".frame" | ".map" | ".exits" => {
                format!(".section tables\nT:      {directive}\n.section code\n.func probe\nstp\n")
            }
            ".section" => ".section code\n.func probe\nstp\n".to_string(),
            _ => format!(".func probe\n{directive}\nstp\n"),
        };
        if let Err(e) = mtc_turing_machine::asm::assemble(&source, false)
            && let mtc_core::asm::AsmErrorKind::UnknownMnemonic(word) = &e.kind
        {
            assert_ne!(
                word, directive,
                "the tma grammar paints `{directive}`, but the assembler \
                 rejects it as an unknown mnemonic"
            );
        }
    }
}

/// The `.tma` grammar's non-directive rules an editor visibly depends on.
#[test]
fn tma_grammar_keeps_its_non_directive_rules() {
    let json = load("tma.tmLanguage.json", "source.tma");
    for rule in [
        "comments",
        "label",
        "mnemonics",
        "interpolation",
        "operators",
        "wildcard",
        "symbol",
        "number",
    ] {
        assert!(
            !json["repository"][rule].is_null(),
            "tma grammar misses the `{rule}` rule"
        );
    }
}
