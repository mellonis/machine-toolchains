//! A match cell names only symbols of the alphabet of the tape it reads
//! (docs/tmt/language.md (rules)): a single symbol, a set member, or a range
//! with no walk over that alphabet is a compile error naming the symbol and
//! the alphabet — never an alternative dropped in silence. A contract clause
//! was already strict, under its own code, and stays so.
//!
//! A range written in a pattern cell or a contract clause walks the declared
//! order of the alphabet it is written against (docs/tmt/language.md
//! (pattern ranges)); only a range in an alphabet or set BODY walks Unicode
//! succession, since that is what creates the order.
//!
//! Each firing fixture has a near miss one symbol away that compiles.

use mtc_turing_machine::compiler::{CompileOptions, CompileOutput, compile};

fn compiles(src: &str) -> CompileOutput {
    compile(src, CompileOptions::default()).unwrap_or_else(|e| panic!("must compile: {e}"))
}

/// The fatal's code, and its rendered message for the fixtures that pin
/// what it names.
fn refused(src: &str) -> (&'static str, String) {
    let err = compile(src, CompileOptions::default()).expect_err("must be refused");
    (err.kind.code(), err.to_string())
}

fn one_tape(alphabet: &str, rules: &str) -> String {
    format!(
        "alphabet w {{ {alphabet} }}\n\
         machine {{\n  tape t: w;\n  entry state s {{\n{rules}\n  }}\n}}\n"
    )
}

#[test]
fn a_range_the_alphabet_lacks_is_an_error_and_a_full_one_compiles() {
    let rule = "    ['0'..'9'] -> stop;\n    [*] -> stop;";
    let (code, message) = refused(&one_tape("'_', '0', '1'", rule));
    assert_eq!(code, "range-outside-alphabet");
    assert!(
        message.contains("'9'") && message.contains("`w`"),
        "the error names the missing endpoint and the alphabet: {message}"
    );
    compiles(&one_tape("'_', '0'..'9'", rule));
}

#[test]
fn a_set_member_off_the_tape_is_an_error_and_a_covered_set_compiles() {
    let program = |alphabet: &str| {
        format!(
            "set marks {{ 'x', 'y' }}\n{}",
            one_tape(alphabet, "    [marks] -> stop;\n    [*] -> stop;")
        )
    };
    let (code, message) = refused(&program("'_', 'x'"));
    assert_eq!(code, "set-outside-alphabet");
    assert!(
        message.contains("'y'") && message.contains("`marks`") && message.contains("`w`"),
        "the error names the member, the set and the alphabet: {message}"
    );
    compiles(&program("'_', 'x', 'y'"));
}

/// A lone symbol off the tape is the same authoring fact as a set member
/// off it, but no set is involved, so it has a code of its own.
#[test]
fn a_single_symbol_off_the_tape_is_an_error_and_one_on_it_compiles() {
    let (code, message) = refused(&one_tape(
        "'_', 'a'",
        "    ['z'] -> stop;\n    [*] -> stop;",
    ));
    assert_eq!(code, "symbol-outside-alphabet");
    assert!(
        message.contains("'z'") && message.contains("`w`"),
        "{message}"
    );
    compiles(&one_tape(
        "'_', 'a', 'z'",
        "    ['z'] -> stop;\n    [*] -> stop;",
    ));
}

/// A set with no members matches nothing; naming one in a cell is a rule
/// that can never fire, refused like every other such cell.
#[test]
fn an_empty_set_in_a_pattern_cell_is_an_error_and_a_non_empty_one_compiles() {
    let program = |members: &str| {
        format!(
            "set some {{ {members} }}\n{}",
            one_tape("'_', 'a'", "    [some] -> stop;\n    [*] -> stop;")
        )
    };
    let (code, message) = refused(&program(""));
    assert_eq!(code, "empty-set-in-pattern");
    assert!(message.contains("`some`"), "{message}");
    compiles(&program("'a'"));
}

/// A range walks the tape alphabet's declared order, so over
/// `'_', 'a', 'z', 'b'` the cell `['a'..'z']` is `'a'` and `'z'` — and
/// `['b']` beside it is no conflict. Mutation: walking Unicode succession
/// in the cell takes in `'b'` as well, and the two rules then match the
/// same symbol (`exact-row-conflict`).
#[test]
fn a_cell_range_walks_the_declared_order() {
    compiles(&one_tape(
        "'_', 'a', 'z', 'b'",
        "    ['a'..'z'] -> stop;\n    ['b'] -> halt;\n    [*] -> stop;",
    ));
}

/// The declared order decides which endpoint comes first: `['z'..'a']` is
/// ascending over `'_', 'z', 'a'` and reversed over `'_', 'a', 'z'`, where
/// the error says which to write first.
#[test]
fn a_reversed_cell_range_is_an_error_that_says_which_comes_first() {
    let rule = "    ['z'..'a'] -> stop;\n    [*] -> stop;";
    let (code, message) = refused(&one_tape("'_', 'a', 'z'", rule));
    assert_eq!(code, "range-outside-alphabet");
    assert!(
        message.contains("'a' comes before 'z'"),
        "the error names the order: {message}"
    );
    compiles(&one_tape("'_', 'z', 'a'", rule));
}

/// With no succession to walk, an endpoint of several characters is legal
/// in a cell; a BODY range still walks succession and still needs
/// single-scalar endpoints.
#[test]
fn a_multi_character_endpoint_is_legal_in_a_cell_but_not_in_a_body() {
    compiles(&one_tape(
        "'_', 'ab', 'cd', 'ef'",
        "    ['ab'..'cd'] -> stop;\n    [*] -> stop;",
    ));
    let (code, _) = refused(&one_tape("'_', 'ab'..'cd'", "    [*] -> stop;"));
    assert_eq!(code, "range-endpoint-not-scalar");
}

/// A contract clause keeps its own code for a symbol the parameter's
/// alphabet lacks — a single, a set member, or a range endpoint alike —
/// never either pattern-cell code. Mutation: routing clause elements
/// through the pattern-cell strictness (`set-outside-alphabet` here).
#[test]
fn a_clause_symbol_off_the_alphabet_keeps_contract_symbol_unknown() {
    let routine = |clause: &str| {
        format!(
            "set marks {{ 'a', 'q' }}\n\
             alphabet w {{ '_', 'a', 'b' }}\n\
             export routine r(tape t: w writes {{ {clause} }}) {{\n\
               entry state s {{ [*] -> return; }}\n\
             }}\n"
        )
    };
    for clause in ["marks", "'a'..'q'", "'q'"] {
        let (code, message) = refused(&routine(clause));
        assert_eq!(code, "contract-symbol-unknown", "`{clause}`: {message}");
        assert!(message.contains("'q'"), "`{clause}`: {message}");
    }
    compiles(&routine("'a'..'b'"));
}

/// A reversed clause range has both endpoints in the alphabet, so it is not
/// an unknown symbol: it is the same fact a reversed cell range is.
#[test]
fn a_reversed_clause_range_is_range_outside_alphabet() {
    let routine = |clause: &str| {
        format!(
            "alphabet w {{ '_', 'b', 'a' }}\n\
             export routine r(tape t: w writes {{ {clause} }}) {{\n\
               entry state s {{ [*] -> return; }}\n\
             }}\n"
        )
    };
    let (code, message) = refused(&routine("'a'..'b'"));
    assert_eq!(code, "range-outside-alphabet", "{message}");
    compiles(&routine("'b'..'a'"));
}

/// A clause range reaches the IR as exactly the alphabet's declared run
/// between its endpoints. Mutation: expanding a clause range by succession
/// — `'b'` joins the run, and over this alphabet `'c'`..`'y'` are unknown
/// symbols besides.
#[test]
fn a_clause_range_reaches_the_ir_as_the_declared_run() {
    let out = compiles(
        "alphabet w { '_', 'a', 'z', 'b' }\n\
         export routine r(tape t: w enters { 'a'..'z' }) {\n\
           entry state s { [*] -> return; }\n\
         }\n",
    );
    let world = out
        .ir
        .worlds
        .iter()
        .find(|w| w.name == "r")
        .expect("the routine lowers");
    assert_eq!(
        world.tapes[0].enters.as_deref(),
        Some(["a".to_string(), "z".to_string()].as_slice())
    );
}

/// A `writes` clause range is the declared run too, so a routine writing
/// the symbol declared after `'z'` breaks a `writes { 'a'..'z' }` contract.
#[test]
fn a_writes_range_excludes_what_the_alphabet_declares_past_it() {
    let routine = |written: &str| {
        format!(
            "alphabet w {{ '_', 'a', 'z', 'b' }}\n\
             export routine r(tape t: w writes {{ 'a'..'z' }}) {{\n\
               entry state s {{ [*] -> write ['{written}'] return; }}\n\
             }}\n"
        )
    };
    let (code, message) = refused(&routine("b"));
    assert_eq!(code, "writes-outside-contract", "{message}");
    compiles(&routine("z"));
}

/// A number range binds every symbol the declared order puts between its
/// endpoints, glyphs included, so a fold over it is refused when one of
/// them names no number — found at expansion, where the members are known.
/// Mutation: dropping that refusal lets the glyph reach fold arithmetic,
/// which a debug build asserts can never happen.
#[test]
fn a_fold_over_a_number_range_with_a_glyph_inside_is_char_arithmetic() {
    let program = |alphabet: &str| {
        one_tape(
            alphabet,
            "    [0..9 as d] -> write [{d+1}] stop;\n    [*] -> stop;",
        )
    };
    let (code, message) = refused(&program("'_', 0, 'x', 1, 9, 10"));
    assert_eq!(code, "char-arithmetic", "{message}");
    compiles(&program("'_', 0..10"));
}

/// Quotes never change which symbol a label names, and a cell or clause
/// range only looks its endpoints up by label, so `'0'..9` is the same
/// walk as `'0'..'9'` — the same compiled object. A BODY range walks
/// Unicode succession, which needs both endpoints of one kind, so there
/// the mixed pair is still refused. Mutation: keeping the refusal in the
/// grammar every position shares — the cell and the clause are refused
/// too.
#[test]
fn a_mixed_spelling_range_is_one_walk_in_a_cell_or_clause_but_not_in_a_body() {
    let cell = |range: &str| {
        compiles(&one_tape(
            "'_', '0'..'9'",
            &format!("    [{range}] -> stop;\n    [*] -> halt;"),
        ))
        .object
    };
    assert_eq!(cell("'0'..9"), cell("'0'..'9'"));
    compiles(
        "alphabet w { '_', '0'..'9' }\n\
         export routine r(tape t: w writes { '0'..9 }) {\n\
           entry state s { [*] -> write ['5'] return; }\n\
         }\n",
    );
    let (code, message) = refused(&one_tape("'_', '0'..9", "    [*] -> stop;"));
    assert_eq!(code, "range-kind-mismatch", "{message}");
}

/// The static `enters` check reads a cell through the same declared-order
/// walk: over `'_', 'a', 'z', 'b'`, an entry state whose only rule is
/// `['a'..'z']` accepts `'a'` and `'z'` but not `'b'`, so an `enters`
/// clause naming `'b'` is refused and one naming `'z'` compiles. Mutation:
/// the source-level cell reading walking Unicode succession — `'b'` then
/// counts as accepted and the refusal disappears.
#[test]
fn the_static_enters_check_reads_a_cell_range_in_declared_order() {
    let routine = |entered: &str| {
        format!(
            "alphabet w {{ '_', 'a', 'z', 'b' }}\n\
             export routine r(tape t: w enters {{ {entered} }}) {{\n\
               entry state s {{ ['a'..'z'] -> return; }}\n\
             }}\n"
        )
    };
    let (code, message) = refused(&routine("'b'"));
    assert_eq!(code, "enters-not-accepted", "{message}");
    compiles(&routine("'z'"));
}

/// A message spells a symbol the way the source wrote it: a number bare,
/// a glyph quoted — the header's rule for a range endpoint.
/// Mutation: always quoting — `[5]` reads back as `'5'`.
#[test]
fn a_message_spells_each_symbol_in_its_written_kind() {
    let (_, message) = refused(&one_tape("'_', 'a'", "    [5] -> stop;\n    [*] -> stop;"));
    assert!(message.contains("5 in this pattern cell"), "{message}");
    let (_, message) = refused(&one_tape(
        "'_', 'a'",
        "    ['z'] -> stop;\n    [*] -> stop;",
    ));
    assert!(message.contains("'z' in this pattern cell"), "{message}");
    let (_, message) = refused(&one_tape(
        "'_', 0..9",
        "    [9..0] -> stop;\n    [*] -> stop;",
    ));
    assert!(
        message.contains("0 comes before 9") && message.contains("write it 0..9"),
        "{message}"
    );
    let (_, message) = refused(&one_tape(
        "'_', 0..9",
        "    [0..'x'] -> stop;\n    [*] -> stop;",
    ));
    assert!(
        message.contains("'x', an endpoint of the range 0..'x'"),
        "{message}"
    );
}

/// A set name is written on its own, never as a range endpoint, and the
/// refusal says so — in a pattern cell and in an alphabet body, at either
/// end. Mutation: the generic "expected `,` or `]`" refusal.
#[test]
fn a_set_name_as_a_range_endpoint_is_refused_with_a_reason() {
    let sets = "set lo { 'a' }\nset hi { 'b' }\n";
    for src in [
        format!(
            "{sets}{}",
            one_tape("'_', 'a', 'b'", "    [lo..'b'] -> stop;")
        ),
        format!(
            "{sets}{}",
            one_tape("'_', 'a', 'b'", "    ['a'..hi] -> stop;")
        ),
        format!("{sets}alphabet w {{ '_', lo..'b' }}\n"),
        format!("{sets}alphabet w {{ '_', 'a'..hi }}\n"),
    ] {
        let (code, message) = refused(&src);
        assert_eq!(code, "unexpected-token", "{src}: {message}");
        assert!(message.contains("never a set"), "{src}: {message}");
    }
}
