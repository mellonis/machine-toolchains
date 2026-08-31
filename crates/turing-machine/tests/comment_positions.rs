//! WHERE a comment may sit after `tmt fmt` — the never-move rule, as an
//! executable audit: a comment is printed between the same two significant
//! tokens it was written between; the formatter may change the whitespace
//! around it, never which tokens it sits between
//! (docs/tmt/fmt.md (comments are never moved)).
//!
//! One template per grammatical position a comment can occupy — 61 of them,
//! each with a `@C@` slot run in BOTH flavours (`/* c */` and `// c`), ported
//! from the measured 2026-08-30 audit
//! (docs/superpowers/specs/2026-08-30-tmc-comment-audit.md). Each case runs
//! three mechanical gates: the comment's significant-token NEIGHBOURS are
//! unchanged, the significant-token stream is unchanged (fmt is
//! whitespace-only), and the output is a fixed point.
//!
//! The groups mirror the work plan: `ALREADY_CORRECT` is the regression
//! guard — 33 positions where the current printer already satisfies the rule
//! — and each `#[ignore]`d group is one task's work list, its ignore reason
//! recording the MEASURED destination the comment moves to today.
//! Un-ignoring a group is how its task proves its surface.
//!
//! Baseline facts, measured on this harness at the branch base: no comment
//! is ever lost, no fixture fails to parse, and the only non-idempotent
//! shape is a BLOCK comment in an `alphabet` header (settles at pass 2).
//! Two findings this harness added over the audit's line-based measure —
//! the neighbour metric is stricter: `use/inside-path` moves (the audit's
//! "use, every position, stays" held only line-wise), and `tape/before-semi`
//! moves past its `;` (the audit's before-closer group counted only braced
//! closers).

use mtc_turing_machine::fmt::format;
use mtc_turing_machine::lexer::{LexMode, TokenKind, lex_with};

/// The comment's nearest significant token on each side, by lexing with
/// comments retained. Positions are deliberately NOT part of the answer —
/// layout may change; neighbours may not.
fn neighbours(src: &str) -> (Option<String>, Option<String>) {
    let tokens = lex_with(src, LexMode::WithComments).expect("lexes");
    let ci = tokens
        .iter()
        .position(|t| matches!(t.kind, TokenKind::Comment(_)))
        .expect("the fixture carries one comment");
    let before = tokens[..ci]
        .iter()
        .rev()
        .find(|t| !matches!(t.kind, TokenKind::Comment(_)))
        .map(|t| format!("{:?}", t.kind));
    let after = tokens[ci + 1..]
        .iter()
        .find(|t| !matches!(t.kind, TokenKind::Comment(_)))
        .map(|t| format!("{:?}", t.kind));
    (before, after)
}

/// The comment-free token-kind stream — the whitespace-only gate's view.
fn significant_kinds(src: &str) -> Vec<String> {
    lex_with(src, LexMode::WithoutComments)
        .expect("lexes")
        .into_iter()
        .map(|t| format!("{:?}", t.kind))
        .collect()
}

/// Runs one template in both flavours through the three gates.
#[track_caller]
fn never_moves(id: &str, tpl: &str) {
    for sub in ["/* c */", "// c"] {
        let src = tpl.replace("@C@", sub);
        let out = format(&src).unwrap_or_else(|e| panic!("{id} [{sub}]: fmt failed: {e:?}"));
        assert_eq!(
            neighbours(&src),
            neighbours(&out),
            "{id} [{sub}]: the comment moved\n--- input ---\n{src}\n--- output ---\n{out}"
        );
        assert_eq!(
            significant_kinds(&src),
            significant_kinds(&out),
            "{id} [{sub}]: fmt changed a token\n--- input ---\n{src}\n--- output ---\n{out}"
        );
        let again = format(&out).unwrap_or_else(|e| panic!("{id} [{sub}]: second pass: {e:?}"));
        assert_eq!(
            out, again,
            "{id} [{sub}]: fmt is not idempotent\n--- input ---\n{src}"
        );
    }
}

#[track_caller]
fn run_group(group: &[(&str, &str)]) {
    for (id, tpl) in group {
        never_moves(id, tpl);
    }
}

// ---------------------------------------------------------------------------
// The regression guard: 33 positions the current printer already gets right.
// These pass at the branch base and must never stop passing.
// ---------------------------------------------------------------------------

const ALREADY_CORRECT: &[(&str, &str)] = &[
    ("file/before-first", "@C@\nalphabet ab { '_', 'a' }\n"),
    (
        "file/between-items",
        "alphabet ab { '_', 'a' }\n@C@\nalphabet cd { '_', 'a' }\n",
    ),
    ("file/after-last", "alphabet ab { '_', 'a' }\n@C@\n"),
    (
        "use/after-keyword",
        "use @C@\n  a::b;\nalphabet ab { '_', 'a' }\n",
    ),
    (
        "use/between-paths",
        "use a::b, @C@\n  c::d;\nalphabet ab { '_', 'a' }\n",
    ),
    (
        "use/before-semi",
        "use a::b @C@\n  ;\nalphabet ab { '_', 'a' }\n",
    ),
    ("use/trailing", "use a::b; @C@\nalphabet ab { '_', 'a' }\n"),
    ("alphabet/open-brace", "alphabet ab { @C@\n  '_' }\n"),
    ("alphabet/before-first", "alphabet ab {\n  @C@\n  '_' }\n"),
    (
        "alphabet/between-elems",
        "alphabet ab {\n  '_', @C@\n  'a' }\n",
    ),
    ("alphabet/before-close", "alphabet ab {\n  '_'\n  @C@\n}\n"),
    ("alphabet/trailing", "alphabet ab { '_' } @C@\n"),
    (
        "namespace/open-brace",
        "alphabet ab { '_', 'a' }\nnamespace n { @C@\n}\n",
    ),
    (
        "namespace/between",
        "alphabet ab { '_', 'a' }\nnamespace n {\n  @C@\n}\n",
    ),
    (
        "namespace/trailing",
        "alphabet ab { '_', 'a' }\nnamespace n {\n} @C@\n",
    ),
    (
        "routine/open-paren",
        "alphabet ab { '_', 'a' }\nnamespace n {\n  routine r( @C@\n  tape t: ab) {\n    entry state s { [*] -> stop; }\n  }\n}\n",
    ),
    (
        "routine/before-close",
        "alphabet ab { '_', 'a' }\nnamespace n {\n  routine r(tape t: ab @C@\n  ) {\n    entry state s { [*] -> stop; }\n  }\n}\n",
    ),
    (
        "machine/open-brace",
        "alphabet ab { '_', 'a' }\nmachine { @C@\n  tape main: ab;\n  state fin { [*] -> stop; }\n}\n",
    ),
    (
        "machine/before-close",
        "alphabet ab { '_', 'a' }\nmachine {\n  tape main: ab;\n  state fin { [*] -> stop; }\n  @C@\n}\n",
    ),
    (
        "tape/trailing",
        "alphabet ab { '_', 'a' }\nmachine {\n  tape main: ab; @C@\n  state fin { [*] -> stop; }\n}\n",
    ),
    (
        "state/open-brace",
        "alphabet ab { '_', 'a' }\nmachine {\n  tape main: ab;\n  entry state s { @C@\n    [*] -> stop;\n  }\n  state fin { [*] -> stop; }\n}\n",
    ),
    (
        "state/between-rules",
        "alphabet ab { '_', 'a' }\nmachine {\n  tape main: ab;\n  entry state s {\n    [*] -> stop;\n    @C@\n    ['a'] -> stop;\n  }\n  state fin { [*] -> stop; }\n}\n",
    ),
    (
        "state/before-close",
        "alphabet ab { '_', 'a' }\nmachine {\n  tape main: ab;\n  entry state s {\n    [*] -> stop;\n    @C@\n  }\n  state fin { [*] -> stop; }\n}\n",
    ),
    (
        "state/trailing",
        "alphabet ab { '_', 'a' }\nmachine {\n  tape main: ab;\n  entry state s { [*] -> stop; } @C@\n  state fin { [*] -> stop; }\n}\n",
    ),
    (
        "rule/before-pattern",
        "alphabet ab { '_', 'a' }\nmachine {\n  tape main: ab;\n  entry state s {\n    @C@\n    [*] -> stop;\n  }\n  state fin { [*] -> stop; }\n}\n",
    ),
    (
        "rule/in-pattern",
        "alphabet ab { '_', 'a' }\nmachine {\n  tape main: ab;\n  entry state s {\n    [ @C@\n    *] -> stop;\n  }\n  state fin { [*] -> stop; }\n}\n",
    ),
    (
        "rule/in-write-vec",
        "alphabet ab { '_', 'a' }\nmachine {\n  tape main: ab;\n  entry state s {\n    [*] -> write [ @C@\n    'a'] stop;\n  }\n  state fin { [*] -> stop; }\n}\n",
    ),
    (
        "rule/in-move-vec",
        "alphabet ab { '_', 'a' }\nmachine {\n  tape main: ab;\n  entry state s {\n    [*] -> move [ @C@\n    >] stop;\n  }\n  state fin { [*] -> stop; }\n}\n",
    ),
    (
        "rule/trailing",
        "alphabet ab { '_', 'a' }\nmachine {\n  tape main: ab;\n  entry state s {\n    [*] -> stop; @C@\n  }\n  state fin { [*] -> stop; }\n}\n",
    ),
    (
        "graft/open-paren",
        "alphabet ab { '_', 'a' }\nnamespace n {\n  graph g(tape t: ab, state d) {\n    entry state s { [*] -> d; }\n  }\n}\nmachine {\n  tape main: ab;\n  entry graft n::g( @C@\n  t = main, d = fin) as i;\n  state fin { [*] -> stop; }\n}\n",
    ),
    (
        "bind/in-map",
        "alphabet ab { '_', 'a' }\nnamespace n {\n  graph g(tape t: ab, state d) {\n    entry state s { [*] -> d; }\n  }\n}\nmachine {\n  tape main: ab;\n  bind n::g(t = main with map { @C@\n    '_' -> '_', 'a' -> 'a' }, d = fin) as x;\n  state fin { [*] -> stop; }\n}\n",
    ),
    (
        "doc/inside-run",
        "? one\n@C@\n? two\nalphabet ab { '_', 'a' }\n",
    ),
    ("doc/run-to-decl", "? one\n@C@\nalphabet ab { '_' }\n"),
];

#[test]
fn already_correct_positions_stay() {
    run_group(ALREADY_CORRECT);
}

// ---------------------------------------------------------------------------
// The work lists. Each group is one task; its ignore reason records the
// measured destination. Un-ignoring the group is how its task proves itself.
// ---------------------------------------------------------------------------

/// The `alphabet` header — the worst destination and the only unstable one:
/// the comment lands inside the `{`, inline with the elements, and the BLOCK
/// flavour needs a second pass to settle.
const ALPHABET_HEADER: &[(&str, &str)] = &[
    ("alphabet/kw-name", "alphabet @C@\n  ab { '_' }\n"),
    ("alphabet/name-brace", "alphabet ab @C@\n  { '_' }\n"),
];

#[test]
fn alphabet_header_comments_stay_in_the_header() {
    run_group(ALPHABET_HEADER);
}

/// The remaining header families. Measured destinations: `namespace`,
/// `machine`, `state` → own line inside the body (after the `{`);
/// `routine`, `graft`, `bind` → riding the argument list's `(`;
/// `tape` → trailing after the whole statement's `;`.
const OTHER_HEADERS: &[(&str, &str)] = &[
    (
        "namespace/kw-name",
        "alphabet ab { '_', 'a' }\nnamespace @C@\n  n {\n}\n",
    ),
    (
        "namespace/name-brace",
        "alphabet ab { '_', 'a' }\nnamespace n @C@\n  {\n}\n",
    ),
    (
        "routine/kw-name",
        "alphabet ab { '_', 'a' }\nnamespace n {\n  routine @C@\n  r(tape t: ab) {\n    entry state s { [*] -> stop; }\n  }\n}\n",
    ),
    (
        "routine/name-paren",
        "alphabet ab { '_', 'a' }\nnamespace n {\n  routine r @C@\n  (tape t: ab) {\n    entry state s { [*] -> stop; }\n  }\n}\n",
    ),
    (
        "routine/paren-brace",
        "alphabet ab { '_', 'a' }\nnamespace n {\n  routine r(tape t: ab) @C@\n  {\n    entry state s { [*] -> stop; }\n  }\n}\n",
    ),
    (
        "machine/kw-brace",
        "alphabet ab { '_', 'a' }\nmachine @C@\n{\n  tape main: ab;\n  state fin { [*] -> stop; }\n}\n",
    ),
    (
        "tape/kw-name",
        "alphabet ab { '_', 'a' }\nmachine {\n  tape @C@\n  main: ab;\n  state fin { [*] -> stop; }\n}\n",
    ),
    (
        "tape/name-colon",
        "alphabet ab { '_', 'a' }\nmachine {\n  tape main @C@\n  : ab;\n  state fin { [*] -> stop; }\n}\n",
    ),
    (
        "tape/colon-alpha",
        "alphabet ab { '_', 'a' }\nmachine {\n  tape main: @C@\n  ab;\n  state fin { [*] -> stop; }\n}\n",
    ),
    (
        "tape/before-semi",
        "alphabet ab { '_', 'a' }\nmachine {\n  tape main: ab @C@\n  ;\n  state fin { [*] -> stop; }\n}\n",
    ),
    (
        "state/entry-kw",
        "alphabet ab { '_', 'a' }\nmachine {\n  tape main: ab;\n  entry @C@\n  state s { [*] -> stop; }\n  state fin { [*] -> stop; }\n}\n",
    ),
    (
        "state/kw-name",
        "alphabet ab { '_', 'a' }\nmachine {\n  tape main: ab;\n  entry state @C@\n  s { [*] -> stop; }\n  state fin { [*] -> stop; }\n}\n",
    ),
    (
        "state/name-brace",
        "alphabet ab { '_', 'a' }\nmachine {\n  tape main: ab;\n  entry state s @C@\n  { [*] -> stop; }\n  state fin { [*] -> stop; }\n}\n",
    ),
    (
        "graft/kw-target",
        "alphabet ab { '_', 'a' }\nnamespace n {\n  graph g(tape t: ab, state d) {\n    entry state s { [*] -> d; }\n  }\n}\nmachine {\n  tape main: ab;\n  entry graft @C@\n  n::g(t = main, d = fin) as i;\n  state fin { [*] -> stop; }\n}\n",
    ),
    (
        "graft/before-as",
        "alphabet ab { '_', 'a' }\nnamespace n {\n  graph g(tape t: ab, state d) {\n    entry state s { [*] -> d; }\n  }\n}\nmachine {\n  tape main: ab;\n  entry graft n::g(t = main, d = fin) @C@\n  as i;\n  state fin { [*] -> stop; }\n}\n",
    ),
    (
        "bind/kw-target",
        "alphabet ab { '_', 'a' }\nnamespace n {\n  graph g(tape t: ab, state d) {\n    entry state s { [*] -> d; }\n  }\n}\nmachine {\n  tape main: ab;\n  bind @C@\n  n::g(t = main, d = fin) as x;\n  state fin { [*] -> stop; }\n}\n",
    ),
];

#[test]
fn header_comments_stay_in_their_headers() {
    run_group(OTHER_HEADERS);
}

/// Interior list-entry positions: the comment is pushed to the entry's end.
/// `use/inside-path` and `alphabet/in-range` are this harness's two findings
/// beyond the audit — the audit's line-based measure missed both.
const LIST_INTERIORS: &[(&str, &str)] = &[
    (
        "use/inside-path",
        "use a:: @C@\n  b;\nalphabet ab { '_', 'a' }\n",
    ),
    (
        "alphabet/in-range",
        "alphabet ab {\n  '_', 'a' @C@\n  ..'z' }\n",
    ),
    (
        "routine/inside-param",
        "alphabet ab { '_', 'a' }\nnamespace n {\n  routine r(tape @C@\n  t: ab) {\n    entry state s { [*] -> stop; }\n  }\n}\n",
    ),
    (
        "graft/in-arg",
        "alphabet ab { '_', 'a' }\nnamespace n {\n  graph g(tape t: ab, state d) {\n    entry state s { [*] -> d; }\n  }\n}\nmachine {\n  tape main: ab;\n  entry graft n::g(t = @C@\n  main, d = fin) as i;\n  state fin { [*] -> stop; }\n}\n",
    ),
];

#[test]
fn list_interior_comments_stay_in_their_entries() {
    run_group(LIST_INTERIORS);
}

/// The rule's action slot: a comment after the pattern, the arrow, or an
/// action keyword is pushed to the rule's tail (after the `;`), and one
/// inside a `write` vector's keyword slot migrates into the vector.
const RULE_ACTION_SLOT: &[(&str, &str)] = &[
    (
        "rule/after-pattern",
        "alphabet ab { '_', 'a' }\nmachine {\n  tape main: ab;\n  entry state s {\n    [*] @C@\n    -> stop;\n  }\n  state fin { [*] -> stop; }\n}\n",
    ),
    (
        "rule/after-arrow",
        "alphabet ab { '_', 'a' }\nmachine {\n  tape main: ab;\n  entry state s {\n    [*] -> @C@\n    stop;\n  }\n  state fin { [*] -> stop; }\n}\n",
    ),
    (
        "rule/write-kw-bracket",
        "alphabet ab { '_', 'a' }\nmachine {\n  tape main: ab;\n  entry state s {\n    [*] -> write @C@\n    ['a'] stop;\n  }\n  state fin { [*] -> stop; }\n}\n",
    ),
    (
        "rule/in-subst",
        "alphabet ab { '_', 'a' }\nmachine {\n  tape main: ab;\n  entry state s {\n    [*] -> write [{ 0 @C@\n    + 1 }] stop;\n  }\n  state fin { [*] -> stop; }\n}\n",
    ),
    (
        "rule/before-transition",
        "alphabet ab { '_', 'a' }\nmachine {\n  tape main: ab;\n  entry state s {\n    [*] -> move [>] @C@\n    stop;\n  }\n  state fin { [*] -> stop; }\n}\n",
    ),
    (
        "rule/before-semi",
        "alphabet ab { '_', 'a' }\nmachine {\n  tape main: ab;\n  entry state s {\n    [*] -> stop @C@\n    ;\n  }\n  state fin { [*] -> stop; }\n}\n",
    ),
];

#[test]
#[ignore = "moves to the rule's tail past the ; (measured; write-kw-bracket instead migrates into the vector, in-subst past the })"]
fn rule_action_comments_stay_in_their_slot() {
    run_group(RULE_ACTION_SLOT);
}
