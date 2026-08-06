//! The `.tmc` formatter's objective guard: on every `.tmc` source in the
//! repository — the Appendix-A examples, the nested-graft fixture, and the
//! embedded standard library — formatting must be IDEMPOTENT and must not
//! change a single token.
//!
//! "Not a single token" is checked by re-lexing the formatted text and
//! comparing the token stream with the original's, not by checking that the
//! output still parses: a printer that dropped a `move` vector or rewrote a
//! number's spelling would still parse fine.

use mtc_turing_machine::fmt::format;
use mtc_turing_machine::lexer::{Comment, LexMode, Token, TokenKind, lex_with};

/// Every `.tmc` source the repository ships, as (name, text).
fn corpus() -> Vec<(String, String)> {
    let root = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/golden");
    let mut out: Vec<(String, String)> = std::fs::read_dir(root)
        .expect("the golden directory exists")
        .map(|entry| entry.expect("a readable directory entry").path())
        .filter(|path| path.extension().and_then(|x| x.to_str()) == Some("tmc"))
        .map(|path| {
            (
                path.file_name()
                    .expect("a fixture has a file name")
                    .to_string_lossy()
                    .into_owned(),
                std::fs::read_to_string(&path).expect("a readable fixture"),
            )
        })
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out.push((
        "std.tmc".to_string(),
        std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/stdlib/std.tmc"))
            .expect("the embedded stdlib source is readable"),
    ));
    out
}

/// A token reduced to what a whitespace-only reprint must preserve. Comment
/// text is compared with each line's trailing whitespace stripped — that is
/// the one normalization the printer applies to trivia, and it is
/// whitespace-only by construction.
#[derive(Debug, PartialEq, Eq)]
enum Sig {
    Kind(TokenKind),
    Comment { text: String, own_line: bool },
}

fn signature(tokens: &[Token]) -> Vec<Sig> {
    tokens
        .iter()
        .map(|t| match &t.kind {
            TokenKind::Comment(Comment {
                text,
                kind,
                own_line,
            }) => Sig::Comment {
                text: format!(
                    "{kind:?}:{}",
                    text.split('\n')
                        .map(str::trim_end)
                        .collect::<Vec<_>>()
                        .join("\n")
                ),
                own_line: *own_line,
            },
            other => Sig::Kind(other.clone()),
        })
        .collect()
}

fn token_signature(source: &str) -> Vec<Sig> {
    signature(&lex_with(source, LexMode::WithComments).expect("the source lexes"))
}

#[test]
fn every_tmc_source_formats_idempotently() {
    for (name, source) in corpus() {
        let once = format(&source).unwrap_or_else(|e| panic!("{name}: {e:?}"));
        let twice = format(&once).unwrap_or_else(|e| panic!("{name} (second pass): {e:?}"));
        assert_eq!(once, twice, "{name}: fmt is not idempotent");
    }
}

#[test]
fn formatting_never_changes_a_token() {
    for (name, source) in corpus() {
        let formatted = format(&source).unwrap_or_else(|e| panic!("{name}: {e:?}"));
        assert_eq!(
            token_signature(&source),
            token_signature(&formatted),
            "{name}: the formatted text does not lex to the same token stream"
        );
    }
}

#[test]
fn every_tmc_source_ends_in_exactly_one_newline() {
    for (name, source) in corpus() {
        let formatted = format(&source).unwrap_or_else(|e| panic!("{name}: {e:?}"));
        assert!(formatted.ends_with('\n'), "{name}: no final newline");
        assert!(
            !formatted.ends_with("\n\n"),
            "{name}: a blank line before EOF"
        );
    }
}

#[test]
fn no_line_carries_trailing_whitespace() {
    for (name, source) in corpus() {
        let formatted = format(&source).unwrap_or_else(|e| panic!("{name}: {e:?}"));
        for (n, line) in formatted.lines().enumerate() {
            assert_eq!(
                line.trim_end(),
                line,
                "{name}:{}: trailing whitespace",
                n + 1
            );
        }
    }
}

/// An interior list comment prints where it was written, not relocated
/// below the enclosing item. A trailing comment (`own_line == false`)
/// rides the preceding entry's line; an own-line comment keeps its own
/// line. Either way a LINE comment forces the list multi-line, because
/// nothing can follow `//` on its physical line.
#[test]
fn interior_list_comments_print_in_place() {
    let src = "alphabet bits { '_', // the blank\n  '0', '1' }\n\n\
               machine { tape t: bits; entry state s { [*] -> stop; } }\n";
    let out = format(src).expect("formats");
    let alphabet: Vec<&str> = out
        .lines()
        .take_while(|l| !l.starts_with("machine"))
        .filter(|l| !l.is_empty())
        .collect();
    assert_eq!(
        alphabet,
        vec![
            "alphabet bits {",
            "  '_', // the blank",
            "  '0',",
            "  '1'",
            "}"
        ],
        "the comment stays on the entry it was written against"
    );
}

/// A comment after the last entry prints before the closer, still inside
/// the list — the position a per-entry scheme could not express.
#[test]
fn a_comment_after_the_last_entry_prints_before_the_closer() {
    let src = "alphabet bits { '_', '0', '1' // the last\n}\n\n\
               machine { tape t: bits; entry state s { [*] -> stop; } }\n";
    let out = format(src).expect("formats");
    let closer = out
        .lines()
        .position(|l| l == "}")
        .expect("the alphabet closes on its own line");
    assert!(
        out.lines().nth(closer - 1).unwrap().contains("// the last"),
        "the comment is the last thing inside the body, got:\n{out}"
    );
}

/// A BLOCK comment with no LINE comment beside it does not force a break:
/// something can follow `*/` on the same physical line.
#[test]
fn an_interior_block_comment_keeps_the_list_on_one_line() {
    let src = "alphabet bits { '_', /* x */ '0', '1' }\n\n\
               machine { tape t: bits; entry state s { [*] -> stop; } }\n";
    let out = format(src).expect("formats");
    assert!(
        out.lines().next().unwrap().contains("/* x */"),
        "the block comment stays inline, got:\n{out}"
    );
}

/// A comment inside a `call`'s binding list prints in place, not below the
/// rule that carries the call.
#[test]
fn interior_call_binding_comments_print_in_place() {
    let src = "alphabet bits { '_', '0', '1' }\n\n\
               routine walk(tape t: bits, state done) {\n\
               \x20 entry state g { ['_'] -> goto done; }\n\
               }\n\n\
               machine {\n\
               \x20 tape m: bits;\n\
               \x20 entry state s { [*] -> call walk(t = m, // the work tape\n\
               \x20                                  done = stop) then stop; }\n\
               }\n";
    let out = format(src).expect("formats");
    let comment_line = out
        .lines()
        .find(|l| l.contains("// the work tape"))
        .expect("the comment survives");
    assert!(
        comment_line.contains("t = m"),
        "it rides the binding it was written against, got: {comment_line:?}"
    );
}

/// A comment inside a `with map` pair list prints in place.
#[test]
fn interior_map_pair_comments_print_in_place() {
    let src = "alphabet bits { '_', '0', '1' }\n\
               alphabet wide { '_', 'x', 'y' }\n\n\
               routine walk(tape t: bits) { entry state g { ['_'] -> stop; } }\n\n\
               machine {\n\
               \x20 tape m: wide;\n\
               \x20 entry state s { [*] -> call walk(t = m with map { 'x' -> '0', // low\n\
               \x20                                                   'y' -> '1' }) then stop; }\n\
               }\n";
    let out = format(src).expect("formats");
    let comment_line = out
        .lines()
        .find(|l| l.contains("// low"))
        .expect("the comment survives");
    assert!(
        comment_line.contains("'x' -> '0'"),
        "it rides the pair it was written against, got: {comment_line:?}"
    );
}

/// A same-line comment written immediately after a binding list's opening
/// `(` prints there instead of being dropped — `paren_list` is shared by
/// `call`'s binding list, signature parameter lists, graft argument lists,
/// and `bind` argument lists, so this one call proves all four.
#[test]
fn a_same_line_comment_after_the_opening_paren_prints_in_place() {
    let src = "alphabet bits { '_', '0', '1' }\n\n\
               routine w(tape t: bits, state d) { entry state g { ['_'] -> goto d; } }\n\n\
               machine {\n\
               \x20 tape m: bits;\n\
               \x20 entry state s { [*] -> call w( /* slot 0 */ t = m, d = stop) then stop; }\n\
               }\n";
    let out = format(src).expect("formats");
    let call_open = out
        .lines()
        .find(|l| l.contains("call w("))
        .expect("the call opens on its own line");
    assert!(
        call_open.contains("/* slot 0 */"),
        "the comment rides the opening `(`, got: {call_open:?}"
    );
    let twice = format(&out).expect("the formatted output re-formats");
    assert_eq!(out, twice, "formatting the output is not idempotent");
}

/// The same defect as the binding-list case, one level down: a same-line
/// comment written immediately after a `with map`'s opening `{` prints
/// there instead of being dropped — `sym_map_text` mirrors `paren_list`'s
/// shape but is a separate renderer, so it needed the same fix on its own.
#[test]
fn a_same_line_comment_after_the_with_map_opening_brace_prints_in_place() {
    let src = "alphabet bits { '_', '0', '1' }\n\
               alphabet wide { '_', 'x', 'y' }\n\n\
               routine walk(tape t: bits) { entry state g { ['_'] -> stop; } }\n\n\
               machine {\n\
               \x20 tape m: wide;\n\
               \x20 entry state s { [*] -> call walk(t = m with map { /* map slot 0 */ 'x' -> '0', 'y' -> '1' }) then stop; }\n\
               }\n";
    let out = format(src).expect("formats");
    let map_open = out
        .lines()
        .find(|l| l.contains("with map {"))
        .expect("the map opens on its own line");
    assert!(
        map_open.contains("/* map slot 0 */"),
        "the comment rides the opening `{{`, got: {map_open:?}"
    );
    let twice = format(&out).expect("the formatted output re-formats");
    assert_eq!(out, twice, "formatting the output is not idempotent");
}

/// A same-line comment written immediately after the `use` keyword prints
/// there instead of being dropped — and prints AFTER `use`, never before
/// it, since moving it earlier would reorder the token stream.
#[test]
fn a_same_line_comment_after_use_prints_in_place() {
    let src = "alphabet bits { '_', '0', '1' }\n\n\
               namespace mylib {\n\
               \x20 export routine plusOne(tape t: bits) { entry state g { [*] -> stop; } }\n\
               }\n\n\
               use // the only import\n\
               \x20   mylib::plusOne;\n\n\
               machine {\n\
               \x20 tape m: bits;\n\
               \x20 entry state s { [*] -> call plusOne(t = m) then stop; }\n\
               }\n";
    let out = format(src).expect("formats");
    let use_line = out
        .lines()
        .find(|l| l.starts_with("use"))
        .expect("the file has a `use` line");
    assert_eq!(
        use_line, "use // the only import",
        "the comment rides `use`'s own line, got: {use_line:?}"
    );
    let twice = format(&out).expect("the formatted output re-formats");
    assert_eq!(out, twice, "formatting the output is not idempotent");
}

/// A same-line LINE comment trailing the last `use` path forces the
/// terminator onto its own line — it cannot ride the comment's physical
/// line, so appending `;` directly after it would merge the terminator
/// into the comment text and make the output unparseable.
#[test]
fn a_trailing_line_comment_on_the_last_use_path_does_not_swallow_the_semicolon() {
    let src = "alphabet bits { '_', '0', '1' }\n\n\
               namespace mylib {\n\
               \x20 export routine plusOne(tape t: bits) { entry state g { [*] -> stop; } }\n\
               }\n\n\
               use mylib::plusOne // trailing comment\n\
               \x20   ;\n\n\
               machine {\n\
               \x20 tape m: bits;\n\
               \x20 entry state s { [*] -> call plusOne(t = m) then stop; }\n\
               }\n";
    let out = format(src).expect("formats");
    let comment_line = out
        .lines()
        .find(|l| l.contains("// trailing comment"))
        .expect("the comment survives");
    assert!(
        !comment_line.contains(';'),
        "the terminator must not merge into the LINE comment, got: {comment_line:?}"
    );
    // The strongest check: a corrupted terminator makes the output
    // unparseable, so re-formatting it would fail outright.
    let twice = format(&out).expect("the formatted output must still parse");
    assert_eq!(out, twice, "formatting the output is not idempotent");
}
