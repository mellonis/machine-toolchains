//! The `.tmc` formatter's objective guard: on every `.tmc` source in the
//! repository — the Appendix-A examples, the nested-graft fixture, the
//! worked doc examples (the flagship brainfuck UTM and the rpn family),
//! and the embedded standard library — formatting must be IDEMPOTENT and
//! must not change a single token.
//!
//! "Not a single token" is checked by re-lexing the formatted text and
//! comparing the token stream with the original's, not by checking that the
//! output still parses: a printer that dropped a `move` vector or rewrote a
//! number's spelling would still parse fine.
//!
//! `every_tmc_source_is_already_fmt_clean` (mirrors
//! `crates/post-machine/tests/fmt_programs.rs`'s
//! `dogfood_stdlib_and_goldens_are_already_fmt_clean`) is the dogfood lock:
//! every file `corpus()` enumerates must already be in canonical form, so
//! `format` is a no-op on it byte-for-byte.

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
    // The worked doc examples, one directory each under docs/examples/
    // — the guard test below set-compares this list against the real
    // directory in both directions, so a new example lands here or
    // fails loudly.
    for name in [
        "brainfuck-utm/brainfuck-utm.tmc",
        "pow2/pow2.tmc",
        "rpn/rpn.tmc",
        "rpnhex/rpnhex.tmc",
        "rpnreg/rpnreg.tmc",
        "rpnwide/rpnwide.tmc",
    ] {
        out.push((
            format!("docs/examples/{name}"),
            std::fs::read_to_string(
                std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../docs/examples"))
                    .join(name),
            )
            .unwrap_or_else(|e| panic!("doc example {name} is unreadable: {e}")),
        ));
    }
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

/// The dogfood lock (mirrors `crates/post-machine/tests/fmt_programs.rs`'s
/// `dogfood_stdlib_and_goldens_are_already_fmt_clean`): every file `corpus()`
/// enumerates — not a hand-written list, so a fixture added later is covered
/// automatically — must already be in canonical form, so `format` is a no-op
/// on it byte-for-byte. This is the regression guard: any future printer
/// change that would reformat a shipped source fails here first, not
/// silently on the next `tmt fmt` run.
#[test]
fn every_tmc_source_is_already_fmt_clean() {
    for (name, source) in corpus() {
        let formatted = format(&source).unwrap_or_else(|e| panic!("{name}: {e:?}"));
        assert_eq!(formatted, source, "{name} is not fmt-clean");
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

/// `volatile tape …` prints its modifier back — the formatter must not
/// silently drop it. Mixed run: name padding stays name-based, so the
/// `volatile ` prefix does not enter the width calculation and a volatile
/// line does not column-align across the modifier with its plain neighbor.
#[test]
fn volatile_tape_declarations_format_canonically() {
    let src = "alphabet bits { '_', '0', '1' }\n\n\
               machine {\n\
               \x20 volatile   tape  sensor:bits;\n\
               \x20 tape scratch : bits;\n\
               \x20 entry state s { [*] -> stop; }\n\
               }\n";
    let out = format(src).expect("formats");
    let sensor_line = out
        .lines()
        .find(|l| l.contains("sensor"))
        .expect("the sensor declaration survives");
    assert!(
        sensor_line
            .trim_start()
            .starts_with("volatile tape sensor:"),
        "the volatile modifier prints ahead of the declaration, got: {sensor_line:?}"
    );
    let scratch_line = out
        .lines()
        .find(|l| l.contains("scratch"))
        .expect("the scratch declaration survives");
    assert!(
        scratch_line.trim_start().starts_with("tape scratch:"),
        "the plain declaration carries no modifier, got: {scratch_line:?}"
    );
    let twice = format(&out).expect("the formatted output re-formats");
    assert_eq!(out, twice, "formatting the output is not idempotent");
}

/// The same modifier in a signature parameter position.
#[test]
fn volatile_signature_params_format_canonically() {
    let src = "alphabet bits { '_', '0', '1' }\n\n\
               routine w(volatile   tape  t:bits) { entry state g { [*] -> stop; } }\n\n\
               machine {\n\
               \x20 tape m: bits;\n\
               \x20 entry state s { [*] -> call w(t = m) then stop; }\n\
               }\n";
    let out = format(src).expect("formats");
    let sig_line = out
        .lines()
        .find(|l| l.starts_with("routine w"))
        .expect("the routine signature survives");
    assert_eq!(
        sig_line, "routine w(volatile tape t: bits) {",
        "the volatile modifier prints in the signature parameter, got: {sig_line:?}"
    );
    let twice = format(&out).expect("the formatted output re-formats");
    assert_eq!(out, twice, "formatting the output is not idempotent");
}

/// A signature tape parameter's `writes { … }` and `preserves { … }` clauses
/// print in canonical form: a single space ahead of each keyword, and the
/// brace body spaced the same way an `alphabet` body is (`{ elem, elem }`).
#[test]
fn writes_and_preserves_clauses_format_canonically() {
    let src = "alphabet bits { '_', '0', '1', '#' }\n\n\
               routine w(tape t:bits writes{'0','1'}preserves{'#'}) { entry state g { [*] -> stop; } }\n\n\
               machine {\n\
               \x20 tape m: bits;\n\
               \x20 entry state s { [*] -> call w(t = m) then stop; }\n\
               }\n";
    let out = format(src).expect("formats");
    let sig_line = out
        .lines()
        .find(|l| l.starts_with("routine w"))
        .expect("the routine signature survives");
    assert_eq!(
        sig_line, "routine w(tape t: bits writes { '0', '1' } preserves { '#' }) {",
        "the contract clauses print in canonical form, got: {sig_line:?}"
    );
    let twice = format(&out).expect("the formatted output re-formats");
    assert_eq!(out, twice, "formatting the output is not idempotent");
}

/// An already-canonical clause is a fixed point — formatting it a first time
/// changes nothing, distinct from the messy-input case above.
#[test]
fn an_already_canonical_contract_clause_is_a_fixed_point() {
    let src = "alphabet bits { '_', '0', '1', '#' }\n\n\
               routine w(tape t: bits writes { '0', '1' } preserves { '#' }) { entry state g { [*] -> stop; } }\n\n\
               machine {\n\
               \x20 tape m: bits;\n\
               \x20 entry state s { [*] -> call w(t = m) then stop; }\n\
               }\n";
    let out = format(src).expect("formats");
    let sig_line = out
        .lines()
        .find(|l| l.starts_with("routine w"))
        .expect("the routine signature survives");
    assert_eq!(
        sig_line, "routine w(tape t: bits writes { '0', '1' } preserves { '#' }) {",
        "an already-canonical clause round-trips unchanged, got: {sig_line:?}"
    );
}

/// A clause-bearing signature is the `formatting_never_changes_a_token`
/// property (module doc) narrowed to a single fixture: the printer must not
/// silently drop the `writes`/`preserves` tokens (or their brace bodies) the
/// way an entry that only reads `alphabet` off `SigParamKind::Tape` would.
#[test]
fn formatting_never_changes_a_token_with_contract_clauses() {
    let src = "alphabet bits { '_', '0', '1', '#' }\n\n\
               routine w(tape t: bits writes { '0', '1' } preserves { '#' }) { entry state g { [*] -> stop; } }\n\n\
               machine {\n\
               \x20 tape m: bits;\n\
               \x20 entry state s { [*] -> call w(t = m) then stop; }\n\
               }\n";
    let formatted = format(src).expect("formats");
    assert_eq!(
        token_signature(src),
        token_signature(&formatted),
        "the formatted text does not lex to the same token stream when contract clauses are present"
    );
}

/// The empty clause `writes {}` (distinct from no clause at all — see
/// `resolve_contract_clause` in the compiler) prints with no inner space,
/// never the alphabet formatter's `{ }`.
#[test]
fn an_empty_contract_clause_prints_with_no_inner_space() {
    let src = "alphabet bits { '_', '0', '1' }\n\n\
               routine w(tape t:bits writes{}) { entry state g { [*] -> stop; } }\n\n\
               machine {\n\
               \x20 tape m: bits;\n\
               \x20 entry state s { [*] -> call w(t = m) then stop; }\n\
               }\n";
    let out = format(src).expect("formats");
    let sig_line = out
        .lines()
        .find(|l| l.starts_with("routine w"))
        .expect("the routine signature survives");
    assert_eq!(
        sig_line, "routine w(tape t: bits writes {}) {",
        "an empty clause has no inner space, got: {sig_line:?}"
    );
    let twice = format(&out).expect("the formatted output re-formats");
    assert_eq!(out, twice, "formatting the output is not idempotent");
}

/// `volatile` and a `writes` clause compose on the same tape parameter.
#[test]
fn volatile_and_writes_clause_compose_in_a_signature_param() {
    // A second `state` parameter after the clause-bearing one exercises the
    // comma-adjacency the printer's `paren_list` actually joins entries
    // with — the shape where a stray space ahead of the `,` would show.
    let src = "alphabet symbols { '0', '1' }\n\n\
               routine w(volatile   tape  num:symbols   writes{'0'},state done) { entry state g { [*] -> stop; } }\n\n\
               machine {\n\
               \x20 tape m: symbols;\n\
               \x20 entry state s { [*] -> call w(num = m) then stop; }\n\
               }\n";
    let out = format(src).expect("formats");
    let sig_line = out
        .lines()
        .find(|l| l.starts_with("routine w"))
        .expect("the routine signature survives");
    assert_eq!(
        sig_line, "routine w(volatile tape num: symbols writes { '0' }, state done) {",
        "volatile and a writes clause compose in signature position, got: {sig_line:?}"
    );
    let twice = format(&out).expect("the formatted output re-formats");
    assert_eq!(out, twice, "formatting the output is not idempotent");
}

/// A clause wide enough to push the whole parameter past the line width
/// forces the signature list to break one parameter per line (the existing
/// `paren_list` behavior) — the clause itself never wraps internally.
#[test]
fn a_wide_writes_clause_breaks_the_param_list_without_wrapping_the_clause() {
    let elems: Vec<String> = (0..40).map(|n| n.to_string()).collect();
    let src = format!(
        "alphabet bytes {{ 0..99 }}\n\n\
         routine w(tape t: bytes writes {{{}}}) {{ entry state g {{ [*] -> stop; }} }}\n\n\
         machine {{\n\
         \x20 tape m: bytes;\n\
         \x20 entry state s {{ [*] -> call w(t = m) then stop; }}\n\
         }}\n",
        elems.join(",")
    );
    let out = format(&src).expect("formats");
    let lines: Vec<&str> = out.lines().collect();
    let head_idx = lines
        .iter()
        .position(|l| *l == "routine w(")
        .expect("the wide signature breaks onto its own opening line");
    let entry_line = lines[head_idx + 1];
    let expected_entry = format!("  tape t: bytes writes {{ {} }}", elems.join(", "));
    assert_eq!(
        entry_line, expected_entry,
        "the single wide clause entry is not itself wrapped, got: {entry_line:?}"
    );
    assert!(
        entry_line.chars().count() > 80,
        "the fixture must actually exceed the line width to exercise the break"
    );
    assert_eq!(
        lines[head_idx + 2],
        ") {",
        "the signature list closes on its own line after a break"
    );
    let twice = format(&out).expect("the formatted output re-formats");
    assert_eq!(out, twice, "formatting the output is not idempotent");
}

/// `volatile` and BOTH clauses compose on the same tape parameter — the
/// three modifiers together, not just the pairwise cases above.
#[test]
fn volatile_and_both_clauses_compose_in_a_signature_param() {
    let src = "alphabet symbols { '0', '1', '#' }\n\n\
               routine w(volatile   tape  num:symbols   writes{'0'}preserves{'#'}) { entry state g { [*] -> stop; } }\n\n\
               machine {\n\
               \x20 tape m: symbols;\n\
               \x20 entry state s { [*] -> call w(num = m) then stop; }\n\
               }\n";
    let out = format(src).expect("formats");
    let sig_line = out
        .lines()
        .find(|l| l.starts_with("routine w"))
        .expect("the routine signature survives");
    assert_eq!(
        sig_line, "routine w(volatile tape num: symbols writes { '0' } preserves { '#' }) {",
        "volatile and both clauses compose in signature position, got: {sig_line:?}"
    );
    let twice = format(&out).expect("the formatted output re-formats");
    assert_eq!(out, twice, "formatting the output is not idempotent");
}

/// A bare `preserves`-only clause (no `writes`) formats canonically — the
/// grammar allows `preserves` with no preceding `writes`.
#[test]
fn a_preserves_only_clause_formats_canonically() {
    let src = "alphabet bits { '_', '0', '1' }\n\n\
               routine w(tape t:bits preserves{'_'}) { entry state g { [*] -> stop; } }\n\n\
               machine {\n\
               \x20 tape m: bits;\n\
               \x20 entry state s { [*] -> call w(t = m) then stop; }\n\
               }\n";
    let out = format(src).expect("formats");
    let sig_line = out
        .lines()
        .find(|l| l.starts_with("routine w"))
        .expect("the routine signature survives");
    assert_eq!(
        sig_line, "routine w(tape t: bits preserves { '_' }) {",
        "a bare preserves-only clause formats canonically, got: {sig_line:?}"
    );
    let twice = format(&out).expect("the formatted output re-formats");
    assert_eq!(out, twice, "formatting the output is not idempotent");
}

/// A range element (`lo..hi`) inside a clause re-encodes losslessly, the
/// same as inside an alphabet body.
#[test]
fn a_range_element_inside_a_clause_formats_canonically() {
    let src = "alphabet bits { '_', '0', '1', '#' }\n\n\
               routine w(tape t:bits writes{'0'..'1'}preserves{'#'}) { entry state g { [*] -> stop; } }\n\n\
               machine {\n\
               \x20 tape m: bits;\n\
               \x20 entry state s { [*] -> call w(t = m) then stop; }\n\
               }\n";
    let out = format(src).expect("formats");
    let sig_line = out
        .lines()
        .find(|l| l.starts_with("routine w"))
        .expect("the routine signature survives");
    assert_eq!(
        sig_line, "routine w(tape t: bits writes { '0'..'1' } preserves { '#' }) {",
        "a range element re-encodes losslessly inside a clause, got: {sig_line:?}"
    );
    let twice = format(&out).expect("the formatted output re-formats");
    assert_eq!(out, twice, "formatting the output is not idempotent");
}

/// The adversarial fixtures, by name. A directory scan alone cannot guard
/// this set: renaming a fixture off `.tmc` leaves its `.fmt` sidecar
/// orphaned and is caught, but DELETING both files shrinks the set with
/// every scan-based check still green — which a bare count floor (this
/// test once demanded `seen >= 6` against eight files) allows outright.
/// The list is the floor; the scan below compares against it in both
/// directions.
const ADVERSARIAL: [&str; 8] = [
    "brace_comments",
    "divergence_semicolon_block_comment",
    "doc_run_interior_comment",
    "docs_and_attention",
    "interior_lists",
    "quirk_bracket_space",
    "quirk_keyword_name",
    "trailing_and_blanks",
];

/// The adversarial sources — shapes the shipped corpus does not carry, one
/// per derived field the pre-green CST stored. They are deliberately NOT
/// fmt-clean (seven of the eight change under the printer), so `corpus()`,
/// whose dogfood lock demands exactly that, must never sweep them: each
/// carries a committed `.fmt` sidecar holding its canonical output
/// instead.
///
/// The sidecars are what pins these files' bytes. They were generated from
/// the printer that formatted them before the green-tree cutover, so the
/// assertion here is the same one the pre-cutover differential made — that
/// the formatter's output on these shapes has not moved — with the text
/// itself standing in for the printer that is gone. A sidecar is never
/// regenerated from a run whose output changed: a diff here means the
/// formatter changed, which on a whitespace-only canonical printer is a
/// finding, not a fixture to refresh.
#[test]
fn every_adversarial_source_formats_to_its_committed_sidecar() {
    let dir = std::path::Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fmt_adversarial"
    ));
    let mut seen: Vec<String> = std::fs::read_dir(dir)
        .expect("the adversarial directory exists")
        .map(|e| e.expect("a readable entry").path())
        .filter(|p| p.extension().and_then(|x| x.to_str()) == Some("tmc"))
        .map(|p| {
            p.file_stem()
                .expect("a fixture has a stem")
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    seen.sort();
    assert_eq!(
        seen,
        ADVERSARIAL.map(str::to_string).to_vec(),
        "the adversarial set changed — update ADVERSARIAL deliberately"
    );
    for name in ADVERSARIAL {
        let src =
            std::fs::read_to_string(dir.join(format!("{name}.tmc"))).expect("a readable fixture");
        let expected = std::fs::read_to_string(dir.join(format!("{name}.fmt")))
            .unwrap_or_else(|e| panic!("{name}.fmt is missing or unreadable: {e}"));
        let out = format(&src).unwrap_or_else(|e| panic!("{name}.tmc does not format: {e:?}"));
        assert_eq!(out, expected, "{name}.tmc no longer formats to {name}.fmt");
    }
}

/// The dogfood lock's REACH, pinned in both directions. `corpus()` walks
/// `tests/golden` but names `src/stdlib/std.tmc` and the worked doc
/// examples explicitly, so a `.tmc` dropped into the stdlib directory or
/// into a `docs/examples/` example directory would otherwise ship with
/// no byte pin at all — and the whole-corpus differential that used to
/// walk those directories is gone with the C1 printer. This is the guard
/// that replaces its enumeration: the stdlib directory holds exactly the
/// one embedded source, and the `docs/examples/` tree (each example a
/// directory of its own, one level deep) set-compares against the
/// `docs/examples/` names `corpus()` carries — a file without a corpus
/// entry and a corpus entry without a file both fail.
#[test]
fn the_corpus_names_every_tmc_source_outside_the_golden_directory() {
    let stdlib_dir = concat!(env!("CARGO_MANIFEST_DIR"), "/src/stdlib");
    let mut stdlib_found: Vec<String> = std::fs::read_dir(stdlib_dir)
        .unwrap_or_else(|e| panic!("{stdlib_dir} is unreadable: {e}"))
        .map(|e| e.expect("a readable entry").path())
        .filter(|p| p.extension().and_then(|x| x.to_str()) == Some("tmc"))
        .map(|p| {
            p.file_name()
                .expect("a name")
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    stdlib_found.sort();
    assert_eq!(
        stdlib_found,
        vec!["std.tmc".to_string()],
        "{stdlib_dir} holds a .tmc source `corpus()` does not name"
    );

    let root = std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../docs/examples"));
    let mut found: Vec<String> = Vec::new();
    for entry in std::fs::read_dir(root).expect("docs/examples is readable") {
        let path = entry.expect("a readable entry").path();
        let name = |p: &std::path::Path| {
            p.file_name()
                .expect("a name")
                .to_string_lossy()
                .into_owned()
        };
        if path.is_dir() {
            for sub in std::fs::read_dir(&path).expect("an example directory is readable") {
                let sub = sub.expect("a readable entry").path();
                if sub.extension().and_then(|x| x.to_str()) == Some("tmc") {
                    found.push(format!("docs/examples/{}/{}", name(&path), name(&sub)));
                }
            }
        } else if path.extension().and_then(|x| x.to_str()) == Some("tmc") {
            found.push(format!("docs/examples/{}", name(&path)));
        }
    }
    found.sort();
    let mut named: Vec<String> = corpus()
        .into_iter()
        .map(|(name, _)| name)
        .filter(|n| n.starts_with("docs/examples/"))
        .collect();
    named.sort();
    assert_eq!(
        found, named,
        "docs/examples and corpus() disagree about the shipped .tmc set"
    );
}

/// `.tmc` fmt is idempotent with one exception, and this is it: a comment
/// written between a declaration's keyword and its name relocates into the
/// brace-delimited body (pass 1), where it re-parses as a comment on the
/// opening brace — which forces the body multi-line (pass 2). Stable from
/// pass 2 on. Pinned by value rather than by "settles eventually", so a
/// printer that changed either intermediate form fails here
/// (docs/tmt/fmt.md (idempotency)).
#[test]
fn a_comment_between_a_keyword_and_its_name_settles_on_the_second_pass() {
    let pass1 = format("alphabet /* a */ ab { '_' }\n").expect("formats");
    assert_eq!(pass1, "alphabet ab { /* a */ '_' }\n");
    let pass2 = format(&pass1).expect("formats");
    assert_eq!(pass2, "alphabet ab { /* a */\n  '_'\n}\n");
    let pass3 = format(&pass2).expect("formats");
    assert_eq!(pass3, pass2, "the settle must be stable from pass 2");
}
