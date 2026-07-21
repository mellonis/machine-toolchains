//! Fixtures locking each layout decision the module doc states. Every case
//! asserts the exact canonical text AND that a second pass is a fixed point
//! (the `stable` helper), so an idempotence regression surfaces on the shape
//! that caused it rather than only on the whole-corpus battery.

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
fn a_fold_expression_prints_tight_with_minimal_parens() {
    // Sprinkled spaces collapse; parens survive only where precedence needs
    // them. `(v+1)` must stay parenthesized under `%` (an `Add` under a
    // tighter `Rem`).
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

// The next three fixtures pin a KNOWN LIMITATION named in the module doc's
// "Trivia-preserving, with one exception" bullet, not a desired shape: the
// CST has no comment slot on an alphabet element, a signature parameter, or
// a binding argument, so a comment written inside one of those lists cannot
// stay in place. It survives — reprinted as an own-line comment right after
// the enclosing item — but a reader can misattribute it to whatever follows.
// If the CST ever grows slots for these, these fixtures should change to
// keep the comment in place, not merely stay passing.

#[test]
fn a_comment_inside_an_alphabet_body_relocates_after_it() {
    check(
        "\
alphabet ab {
  '_', // blank
  'a'
}
",
        "\
alphabet ab { '_', 'a' }
// blank
",
    );
}

#[test]
fn a_comment_inside_a_grafts_binding_list_relocates_after_it() {
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
  entry graft findSomething(t = work, found = celebrateLoudly) as seek;
  // note
}
",
    );
}

#[test]
fn a_comment_inside_a_signature_relocates_after_it() {
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
export graph walk(tape t: ab, state done) {
  // note

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
