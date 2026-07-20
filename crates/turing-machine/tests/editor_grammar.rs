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

/// Every directive the `.tma` grammar paints must be one the assembler
/// actually recognizes. The probe assembles a one-line source carrying the
/// directive and asserts the assembler's complaint — if any — is not
/// "unknown mnemonic" naming it; an arity or discipline complaint proves the
/// word reached a real directive handler.
///
/// What it cannot catch: a directive ADDED to the assembler with no grammar
/// entry. There is no directive table in the core assembler to generate from
/// — the words are recognized by scattered string matches in the assembly
/// CST and lowering — so this direction has no source of truth to compare
/// against, unlike the mnemonics above. The list below is therefore
/// hand-maintained, and the guard's job is to keep it honest rather than
/// complete.
#[test]
fn tma_grammar_directives_are_real_directives() {
    let json = load("tma.tmLanguage.json", "source.tma");
    let mut in_grammar: BTreeSet<String> = BTreeSet::new();
    for rule in [
        "sectionDirective",
        "funcDirective",
        "routineDirective",
        "repeatDirective",
        "tableDirective",
        "frameDirective",
        "byteDirective",
    ] {
        let pattern = json["repository"][rule]["match"]
            .as_str()
            .unwrap_or_else(|| panic!("tma grammar has a `{rule}` match pattern"));
        // `\b` first — stripping every backslash blindly would fuse the word
        // boundary's `b` onto the directive name (`.byte\b` → `.byteb`).
        let bare = pattern.replace("\\b", " ").replace('\\', "");
        // Directive rules are either `.word\b…` or `.(a|b|c)\b`.
        let start = bare.find('.').expect("a directive pattern names a word");
        let rest = &bare[start + 1..];
        if let Some(stripped) = rest.strip_prefix('(') {
            let close = stripped
                .find(')')
                .expect("a grouped directive pattern closes");
            for word in stripped[..close].split('|') {
                in_grammar.insert(format!(".{word}"));
            }
        } else {
            let end = rest
                .find(|c: char| !c.is_ascii_alphanumeric())
                .unwrap_or(rest.len());
            in_grammar.insert(format!(".{}", &rest[..end]));
        }
    }
    assert!(
        in_grammar.len() >= 10,
        "expected the tma grammar to paint the full directive surface, found {in_grammar:?}"
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

    // The reverse direction has no generated source; this pins the words the
    // grammar is known to need so a deletion is loud.
    for expected in [
        ".section", ".func", ".routine", ".rept", ".endr", ".row", ".target", ".targets", ".frame",
        ".map", ".exits", ".byte",
    ] {
        assert!(
            in_grammar.contains(expected),
            "tma grammar misses the `{expected}` directive"
        );
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
