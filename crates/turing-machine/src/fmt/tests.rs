//! Fixtures locking each layout decision [`super::print`]'s module doc
//! states. Every case asserts the exact canonical text AND that a second
//! pass is a fixed point (the `stable` helper), so an idempotence
//! regression surfaces on the shape that caused it rather than only on the
//! whole-corpus battery.

use super::format;

/// Formats, asserting the source is accepted.
fn f(source: &str) -> String {
    format(source).unwrap_or_else(|e| panic!("expected `{source}` to format: {e:?}"))
}

/// Formats and asserts the result is a fixed point.
fn stable(source: &str) -> String {
    let once = f(source);
    let twice = f(&once);
    assert_eq!(once, twice, "fmt is not idempotent on:\n{source}");
    once
}

fn check(source: &str, expected: &str) {
    assert_eq!(stable(source), expected);
}

// -- the state-block grid ---------------------------------------------------

#[test]
fn the_grid_aligns_the_arrow_and_the_action_keywords() {
    check(
        "\
machine {
tape t: ab;
entry state scan {
['b'] -> write ['a'] move [>] goto scan;
['a'] -> move [>] goto scan;
['_'] -> stop;
}
}
",
        "\
machine {
  tape t: ab;
  entry state scan {
    ['b'] -> write ['a'] move [>] goto scan;
    ['a'] ->             move [>] goto scan;
    ['_'] -> stop;
  }
}
",
    );
}

#[test]
fn a_rules_last_column_is_never_padded() {
    // `write [1]` is narrower than the column, but nothing follows it on that
    // row — padding it would only push `stop` away from its own action.
    check(
        "\
machine {
entry state inc {
[1..125 as v] -> write [{v+1}] stop;
[126] -> halt;
[0] -> write [1] stop;
}
}
",
        "\
machine {
  entry state inc {
    [1..125 as v] -> write [{v+1}] stop;
    [126]         -> halt;
    [0]           -> write [1] stop;
  }
}
",
    );
}

#[test]
fn the_debugger_keyword_has_its_own_column() {
    check(
        "\
machine {
entry state s {
[*] -> debugger write [-] move [>] goto s;
['a'] -> write [-] move [<] goto s;
}
}
",
        "\
machine {
  entry state s {
    [*]   -> debugger write [-] move [>] goto s;
    ['a'] ->          write [-] move [<] goto s;
  }
}
",
    );
}

#[test]
fn fmt_preserves_omitted_transition() {
    // An omitted transition (stay in the current state) is printed with no
    // `goto` inserted: the `;` abuts the last action just as an explicit
    // transition would. Idempotent via `stable`.
    let out = stable(
        "\
machine {
tape t: ab;
entry state scan {
['a'] -> write ['b'] move [>];
['_'] -> stop;
}
}
",
    );
    assert_eq!(
        out,
        "\
machine {
  tape t: ab;
  entry state scan {
    ['a'] -> write ['b'] move [>];
    ['_'] -> stop;
  }
}
"
    );
    assert!(!out.contains("goto"), "fmt inserted a transition:\n{out}");
}

#[test]
fn a_comment_or_blank_line_inside_a_state_does_not_split_the_grid() {
    check(
        "\
machine {
entry state s {
// carry
['1'] -> write ['0'] move [<] goto s;

[*] -> stop;
}
}
",
        "\
machine {
  entry state s {
    // carry
    ['1'] -> write ['0'] move [<] goto s;

    [*]   -> stop;
  }
}
",
    );
}

#[test]
fn multi_tape_vectors_and_bindings_reprint_canonically() {
    check(
        "\
machine {
entry state copy {
['0'..'1' as c,*] -> write [-,{c}] move [>,>] goto copy;
['_',*] -> stop;
}
}
",
        "\
machine {
  entry state copy {
    ['0'..'1' as c, *] -> write [-, {c}] move [>, >] goto copy;
    ['_', *]           -> stop;
  }
}
",
    );
}

// -- token fidelity ---------------------------------------------------------

#[test]
fn tokens_are_reprinted_exactly_as_written() {
    // The bare-name transition sugar stays bare, an explicit `goto` stays
    // explicit, a number keeps its written digits, and a glyph keeps only the
    // escapes the lexer requires.
    check(
        "\
machine {
entry state s {
[007] -> next;
['\\''] -> goto s;
['\\\\'] -> write [{v-2}] stop;
}
}
",
        "\
machine {
  entry state s {
    [007]  -> next;
    ['\\''] -> goto s;
    ['\\\\'] -> write [{v-2}] stop;
  }
}
",
    );
}

// -- write-cell fold expressions --------------------------------------------

#[test]
fn a_fold_expression_prints_tight_keeping_source_parens() {
    // Sprinkled spaces collapse; the formatter is whitespace-only, so it
    // reprints the author's parens verbatim. Here those parens are also
    // load-bearing — `(v+1)` under `%` is an `Add` beneath a tighter `Rem` —
    // but their survival is source-driven, not a precedence calculation (the
    // sibling test keeps redundant parens too).
    check(
        "\
machine {
entry state s {
[0..9 as v] -> write [{ ( v + 1 ) % 6 }] goto s;
}
}
",
        "\
machine {
  entry state s {
    [0..9 as v] -> write [{(v+1)%6}] goto s;
  }
}
",
    );
}

#[test]
fn a_fold_expression_drops_parens_precedence_makes_redundant() {
    // `v*2` binds tighter than `+1`, so no parens are printed; left-
    // associative `-` keeps its natural left-to-right reading unparenthesized.
    check(
        "\
machine {
entry state s {
[0..9 as v] -> write [{ v * 2 + 1 }] goto s;
[0..9 as w] -> write [{ w - 3 - 1 }] goto s;
}
}
",
        "\
machine {
  entry state s {
    [0..9 as v] -> write [{v*2+1}] goto s;
    [0..9 as w] -> write [{w-3-1}] goto s;
  }
}
",
    );
}

#[test]
fn a_fold_expression_keeps_redundant_source_parens() {
    // The formatter is whitespace-only: it reprints a substitution from its
    // source tokens, so parens the author wrote survive even where precedence
    // makes them redundant (`v*2` would bind tighter than `+1` without them).
    // Whitespace still collapses; only the tokens are load-bearing.
    check(
        "\
machine {
entry state s {
[0..9 as v] -> write [{ ( v * 2 ) + 1 }] goto s;
}
}
",
        "\
machine {
  entry state s {
    [0..9 as v] -> write [{(v*2)+1}] goto s;
  }
}
",
    );
}

// -- single-line states -----------------------------------------------------

#[test]
fn a_run_of_single_line_states_shares_a_brace_column_and_a_grid() {
    check(
        "\
machine {
state celebrate { [*] -> write ['_'] stop; }
state giveUp { [*] -> halt; }
}
",
        "\
machine {
  state celebrate { [*] -> write ['_'] stop; }
  state giveUp    { [*] -> halt; }
}
",
    );
}

#[test]
fn a_blank_line_ends_a_single_line_state_run() {
    check(
        "\
machine {
state celebrate { [*] -> stop; }

state giveUp { [*] -> halt; }
}
",
        "\
machine {
  state celebrate { [*] -> stop; }

  state giveUp { [*] -> halt; }
}
",
    );
}

#[test]
fn a_multi_line_state_is_never_collapsed_onto_one_line() {
    check(
        "\
machine {
state s {
[*] -> stop;
}
}
",
        "\
machine {
  state s {
    [*] -> stop;
  }
}
",
    );
}

#[test]
fn an_over_wide_single_line_run_expands_to_block_form() {
    let source = "\
machine {
state aVeryLongStateNameIndeed { [*] -> call someRatherLongRoutineName(num = num) then done; }
state b { [*] -> halt; }
}
";
    let out = stable(source);
    assert_eq!(
        out,
        "\
machine {
  state aVeryLongStateNameIndeed {
    [*] -> call someRatherLongRoutineName(num = num) then done;
  }
  state b {
    [*] -> halt;
  }
}
"
    );
}

#[test]
fn a_state_with_an_interior_comment_cannot_stay_on_one_line() {
    check(
        "\
machine {
state s { /* why */ [*] -> stop; }
}
",
        "\
machine {
  state s { /* why */
    [*] -> stop;
  }
}
",
    );
}

// -- argument lists and the width threshold ---------------------------------

#[test]
fn a_call_that_would_cross_the_limit_breaks_one_binding_per_line() {
    check(
        "\
machine {
entry state s {
[*] -> call std::binaryNumbersBare::invertNumber(num = num with map { '^' => '_', '$' => '_' }) then return;
}
}
",
        "\
machine {
  entry state s {
    [*] -> call std::binaryNumbersBare::invertNumber(
             num = num with map { '^' => '_', '$' => '_' }
           ) then return;
  }
}
",
    );
}

#[test]
fn a_graft_breaks_against_its_own_first_token() {
    check(
        "\
machine {
entry graft findSomethingRatherSpecific(t = work, found = celebrateLoudly, missing = giveUpQuietly) as seek;
}
",
        "\
machine {
  entry graft findSomethingRatherSpecific(
    t = work,
    found = celebrateLoudly,
    missing = giveUpQuietly
  ) as seek;
}
",
    );
}

#[test]
fn a_signature_and_an_alphabet_break_the_same_way() {
    check(
        "\
export graph aGraphWithAGenerouslyLongName(tape num: symbols, state doneAndDusted, state alsoDone) {
state s { [*] -> stop; }
}
alphabet wideEnoughToWrap { '_', 'aaaaaaaaaa', 'bbbbbbbbbb', 'cccccccccc', 'dddddddddd' }
",
        "\
export graph aGraphWithAGenerouslyLongName(
  tape num: symbols,
  state doneAndDusted,
  state alsoDone
) {
  state s { [*] -> stop; }
}
alphabet wideEnoughToWrap {
  '_',
  'aaaaaaaaaa',
  'bbbbbbbbbb',
  'cccccccccc',
  'dddddddddd'
}
",
    );
}

// -- blank lines, comments, doc runs ----------------------------------------

// The next three fixtures lock the interior-comment placement described in
// `print`'s module doc "Trivia-preserving" bullet: an
// alphabet body, a signature parameter list, and a graft/bind binding list
// each carry a comment slot, keyed by the index of the entry the comment
// precedes, so a comment written inside one of these lists prints where its
// author put it rather than being relocated below the enclosing item. A
// `call` transition's own binding list and any `with map` pair list nested
// inside one of these binding lists behave the same way, via a side-car on
// the enclosing RuleCst/GraftCst/BindCst rather than a field on the CST node
// itself — those two nest inside a type the AST takes verbatim, so their
// comment slot can't live on the entry. The one remaining case — a comment
// inside a pattern, write, or move vector — still relocates to its own line
// after the enclosing rule; those vectors are positional and walked per row
// by the compiler, so giving them per-entry trivia is tracked separately.

#[test]
fn a_comment_inside_an_alphabet_body_prints_in_place() {
    check(
        "\
alphabet ab {
  '_', // blank
  'a'
}
",
        "\
alphabet ab {
  '_', // blank
  'a'
}
",
    );
}

#[test]
fn a_comment_inside_a_grafts_binding_list_prints_in_place() {
    check(
        "\
machine {
entry graft findSomething(
  t = work, // note
  found = celebrateLoudly
) as seek;
}
",
        "\
machine {
  entry graft findSomething(
    t = work, // note
    found = celebrateLoudly
  ) as seek;
}
",
    );
}

#[test]
fn a_comment_inside_a_signature_prints_in_place() {
    check(
        "\
export graph walk(
  tape t: ab, // note
  state done
) {
state s { [*] -> done; }
}
",
        "\
export graph walk(
  tape t: ab, // note
  state done
) {
  state s { [*] -> done; }
}
",
    );
}

#[test]
fn blank_runs_collapse_to_one_and_are_never_forced() {
    check(
        "\
alphabet ab { '_' }



alphabet cd { '_' }
alphabet ef { '_' }
",
        "\
alphabet ab { '_' }

alphabet cd { '_' }
alphabet ef { '_' }
",
    );
}

#[test]
fn trailing_comments_align_in_a_run_and_stay_tight_alone() {
    check(
        "\
machine {
tape num: bits; // the only tape
entry state inc {
[126] -> halt; // overflow
[0] -> write [1] stop; // blank cell
[1] -> stop;
}
}
",
        "\
machine {
  tape num: bits; // the only tape
  entry state inc {
    [126] -> halt;           // overflow
    [0]   -> write [1] stop; // blank cell
    [1]   -> stop;
  }
}
",
    );
}

#[test]
fn a_run_aligns_even_when_a_member_then_crosses_eighty() {
    // Alignment wins for every member of the run, even one whose
    // comment then crosses 80 columns. Before this, the long member
    // kept a single space and dropped out of the run. No lint reports
    // the crossing: `.tmc` has no line-length rule of its own, unlike
    // `.pma`/`.tma`'s `line-too-long`.
    let src = concat!(
        "machine {\n",
        "  tape prog: ops; // brainfuck source + 'H'; the head IS the instruction pointer\n",
        "  tape cnt:  levels; // unary stack of bracket-nesting levels\n",
        "}\n",
    );
    let out = stable(src);
    let cols: Vec<usize> = out
        .lines()
        .filter(|l| l.contains("//"))
        .map(|l| l.find("//").unwrap())
        .collect();
    assert_eq!(cols[0], cols[1], "both members share the run's column");
    assert!(out.lines().any(|l| l.chars().count() > 80));
}

#[test]
fn tape_declarations_line_their_alphabets_up() {
    check(
        "\
machine {
tape ctl: bits;
tape data: wide;
}
",
        "\
machine {
  tape ctl:  bits;
  tape data: wide;
}
",
    );
}

#[test]
fn brace_line_comments_ride_their_brace() {
    check(
        "\
namespace n { // opens
alphabet ab { '_' }
} // closes
",
        "\
namespace n { // opens
  alphabet ab { '_' }
} // closes
",
    );
}

#[test]
fn a_doc_run_stays_above_its_declaration() {
    check(
        "\
? Adds one.
?
? The head ends on the '$'.
! [deprecated] use plusOneFast

export routine plusOne(tape num: symbols) {
entry graft plusOneGraph(num = num, done = return) as body;
}
",
        "\
? Adds one.
?
? The head ends on the '$'.
! [deprecated] use plusOneFast

export routine plusOne(tape num: symbols) {
  entry graft plusOneGraph(num = num, done = return) as body;
}
",
    );
}

#[test]
fn a_documented_declaration_keeps_the_blank_above_its_run() {
    check(
        "\
alphabet ab { '_' }

? Walks right.
export graph walk(tape t: ab, state done) {
state s { [*] -> done; }
}
",
        "\
alphabet ab { '_' }

? Walks right.
export graph walk(tape t: ab, state done) {
  state s { [*] -> done; }
}
",
    );
}

#[test]
fn nested_namespaces_indent_two_spaces_per_level() {
    check(
        "\
namespace std {
namespace binaryNumbers {
export alphabet symbols { '_', '0', '1' }
}
}
",
        "\
namespace std {
  namespace binaryNumbers {
    export alphabet symbols { '_', '0', '1' }
  }
}
",
    );
}

#[test]
fn use_lists_keep_their_grouping_and_order() {
    check(
        "use  mylib::plusOne ,  other as o ;\nuse third;\n", // spacing is the author's
        "use mylib::plusOne, other as o;\nuse third;\n",
    );
}

// -- edge cases -------------------------------------------------------------

#[test]
fn an_empty_file_reprints_as_one_newline() {
    assert_eq!(f(""), "\n");
    assert_eq!(f("\n\n\n"), "\n");
}

#[test]
fn an_empty_machine_keeps_its_braces() {
    check("machine {}\n", "machine {\n}\n");
}

#[test]
fn crlf_and_tabs_and_trailing_spaces_do_not_survive() {
    let out = stable("machine {\r\n\tstate s { [*] -> stop; }   \r\n}\r\n");
    assert_eq!(out, "machine {\n  state s { [*] -> stop; }\n}\n");
}

#[test]
fn a_lex_or_parse_error_is_returned_not_printed() {
    assert!(format("machine {").is_err());
    assert!(format("alphabet ab { 'unterminated }\n").is_err());
}
