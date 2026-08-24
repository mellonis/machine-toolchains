//! Green-parser goldens for `.tmc`: emission woven into the existing
//! parser for the outer container productions. Expected shapes are
//! derived by hand from the tree-shape rules — trivia flushes into the
//! current node before a child opens, so a node starts at its first
//! significant token — never pasted from a run.

use mtc_core::syntax::{SyntaxNode, debug_dump};
use mtc_turing_machine::compiler::CompileError;
use mtc_turing_machine::lexer::lex;
use mtc_turing_machine::parser::parse as parse_ast;
use mtc_turing_machine::parser::parse_green;
use mtc_turing_machine::syntax::kind_name;

fn dump(source: &str) -> String {
    let tree = parse_green(source).expect("parses");
    let root = SyntaxNode::new_root(tree);
    assert_eq!(root.text(), source, "lossless law");
    debug_dump(&root, &|k| kind_name(k).to_string())
}

fn parse(source: &str) -> SyntaxNode {
    let tree = parse_green(source).expect("parses");
    let root = SyntaxNode::new_root(tree);
    assert_eq!(root.text(), source, "lossless law");
    root
}

fn kind_of(node: &SyntaxNode) -> &'static str {
    kind_name(node.kind())
}

/// `"alphabet ab { '_' }\n"` — ALPHABET spans `alphabet`..`}` inclusive.
/// The trailing newline belongs to ROOT, not to ALPHABET: the node
/// closes right after its `}`.
///
/// An `assert!(d.contains(...))`/`ends_with(...)` pair, as originally
/// drafted, cannot discriminate a correctly-shaped tree from a
/// differently-shaped one: `debug_dump` always renders a token line as
/// `KIND@lo..hi "text"\n`, so the string NEVER ends with the bare word
/// `WHITESPACE` — the quoted text always follows it. That check could
/// not pass for ANY tree, correct or not (confirmed by running it: it
/// failed on the tree this task's own derivation says is right). The
/// exact dump below is the fix — hand-derived from the source's byte
/// offsets (`alphabet`=0..8, ` `=8..9, `ab`=9..11, ` `=11..12,
/// `{`=12..13, ` `=13..14, `'_'`=14..17, ` `=17..18, `}`=18..19,
/// `\n`=19..20) — and it is strictly stronger: it pins every child's
/// kind, span and text, not merely that some node contains the word
/// `ALPHABET` somewhere.
#[test]
fn an_alphabet_declaration() {
    let d = dump("alphabet ab { '_' }\n");
    let expected = "ROOT@0..20\n\
        \x20 ALPHABET@0..19\n\
        \x20   IDENT@0..8 \"alphabet\"\n\
        \x20   WHITESPACE@8..9 \" \"\n\
        \x20   IDENT@9..11 \"ab\"\n\
        \x20   WHITESPACE@11..12 \" \"\n\
        \x20   L_BRACE@12..13 \"{\"\n\
        \x20   WHITESPACE@13..14 \" \"\n\
        \x20   GLYPH@14..17 \"'_'\"\n\
        \x20   WHITESPACE@17..18 \" \"\n\
        \x20   R_BRACE@18..19 \"}\"\n\
        \x20 WHITESPACE@19..20 \"\\n\"\n";
    assert_eq!(d, expected);
}

/// A leading comment is a ROOT-level token before ALPHABET opens, not a
/// child of it — trivia flushes into the CURRENT node, and ALPHABET is
/// not open yet.
#[test]
fn a_leading_comment_belongs_to_the_root() {
    let d = dump("// lead\nalphabet ab { '_' }\n");
    let lead = d.find("LINE_COMMENT").expect("comment present");
    let alpha = d.find("ALPHABET").expect("alphabet present");
    assert!(lead < alpha, "comment must precede the node it leads: {d}");

    // The `lead < alpha` ordering above only rules out the comment
    // landing as a DESCENDANT of ALPHABET (a child node's own line
    // always prints after its parent's opening line, so a wrongly
    // nested comment would print `ALPHABET@..` first and the comment
    // line after it, flipping the order). It does not by itself prove
    // the comment is a ROOT child rather than, say, a second unrelated
    // top-level node — the direct-tree walk below closes that gap:
    // ROOT's first child token is the comment, its second is the
    // leading whitespace, and ALPHABET is ROOT's only child NODE.
    let root = parse("// lead\nalphabet ab { '_' }\n");
    let mut children = root.children_with_tokens();
    let first = children.next().expect("a leading token");
    assert_eq!(kind_name(first.kind()), "LINE_COMMENT");
    let mtc_core::syntax::SyntaxElement::Token(first_tok) = &first else {
        panic!("ROOT's leading child must be a token, not a node: {first:?}");
    };
    assert_eq!(first_tok.text(), "// lead");
    let alphabet = root.children().next().expect("an ALPHABET child node");
    assert_eq!(kind_of(&alphabet), "ALPHABET");
}

/// A weak `contains("MACHINE")` check would still pass for a MACHINE
/// node closed the instant the `machine` keyword is consumed, with the
/// whole `{ … }` body left dangling as ROOT-level tokens beside it —
/// the substring is present either way. Asserting the node's own
/// `text()` against the hand-derived expected span (the whole
/// declaration, trailing `\n` excluded — same rule as ALPHABET's)
/// closes that gap: it only holds if MACHINE actually wraps its body.
#[test]
fn a_machine_with_one_tape() {
    let source = "machine {\n  tape main: ab;\n}\n";
    let root = parse(source);
    let machine = root.children().next().expect("a MACHINE child");
    assert_eq!(kind_of(&machine), "MACHINE");
    assert_eq!(
        machine.text(),
        "machine {\n  tape main: ab;\n}",
        "MACHINE must span the whole block, body included, trailing \\n excluded"
    );
}

/// Namespaces nest, and a `machine` block may NOT sit inside one — that
/// is a language rule (docs/tmt/language.md (namespaces)), so the
/// nesting fixture uses declarations, not a machine.
///
/// A `matches("NAMESPACE").count() == 2` plus a textual `first_ns <
/// alpha` ordering check, as originally drafted, would both still pass
/// for a tree where the inner `namespace b` and its ALPHABET were
/// closed too early and reopened as ROOT-level SIBLINGS of `namespace
/// a` rather than actual descendants — the count only counts opens,
/// and every one of these nodes still appears in source order
/// regardless of nesting depth, so the ordering check can't tell a
/// sibling from a child either. Walking the real tree via `children()`
/// one level at a time is the only check that actually proves each
/// node is nested INSIDE the previous one, not merely printed after it.
#[test]
fn nested_namespaces() {
    let source = "namespace a {\n  namespace b {\n    export alphabet ab { '_', 'a' }\n  }\n}\n";
    let root = parse(source);
    let outer = root.children().next().expect("outer NAMESPACE");
    assert_eq!(kind_of(&outer), "NAMESPACE");
    let inner = outer
        .children()
        .next()
        .expect("inner NAMESPACE nested inside the outer");
    assert_eq!(kind_of(&inner), "NAMESPACE");
    let alphabet = inner
        .children()
        .next()
        .expect("ALPHABET nested inside the inner namespace");
    assert_eq!(kind_of(&alphabet), "ALPHABET");
    // The fixture's `export` prefix is deliberate: ALPHABET's own
    // checkpoint is taken before the top-level dispatch match runs, so
    // it must retroactively wrap `export` too, not just the `alphabet`
    // keyword onward — the same gap a plain kind/nesting check above
    // cannot see, since a mis-scoped checkpoint that drops `export` as
    // a loose child of the inner namespace still leaves ALPHABET
    // correctly nested one level down.
    assert_eq!(alphabet.text(), "export alphabet ab { '_', 'a' }");
    // Both NAMESPACE extents, pinned directly: neither the kind/nesting
    // walk above nor the corpus test (lossless-only — it can't tell a
    // node's tokens from a sibling's) would notice if OUTER stopped
    // including its own `{`/`}` pair, or if INNER's checkpoint drifted
    // to include or exclude the wrong indentation.
    assert_eq!(
        outer.text(),
        "namespace a {\n  namespace b {\n    export alphabet ab { '_', 'a' }\n  }\n}"
    );
    assert_eq!(
        inner.text(),
        "namespace b {\n    export alphabet ab { '_', 'a' }\n  }"
    );
    // Every NAMESPACE the corpus contains is one of these two — no
    // stray third one hiding somewhere else in the tree.
    assert_eq!(
        outer
            .children()
            .filter(|c| kind_of(c) == "NAMESPACE")
            .count(),
        1
    );
    assert_eq!(
        inner
            .children()
            .filter(|c| kind_of(c) == "NAMESPACE")
            .count(),
        0
    );
}

/// A `matches("USE_PATH").count() == 2` check, as originally drafted,
/// says nothing about WHERE the two nodes sit or what text each one
/// actually claims — a USE_PATH that swallowed the separating comma
/// into its own span, or one that left a stray path as a ROOT-level
/// sibling instead of a USE child, would still count to 2. Walking
/// USE's actual children and asserting their exact text (each path's
/// own span, comma and surrounding whitespace excluded — the same
/// "closes right after its own last token" rule as UsePath's alias
/// case) closes both gaps.
#[test]
fn a_use_declaration_with_two_paths() {
    let source = "use std::binaryNumbers,\n    other::thing;\n";
    let root = parse(source);
    let use_node = root.children().next().expect("a USE child");
    assert_eq!(kind_of(&use_node), "USE");
    let paths: Vec<SyntaxNode> = use_node.children().collect();
    assert_eq!(paths.len(), 2, "two USE_PATH children");
    for p in &paths {
        assert_eq!(kind_of(p), "USE_PATH");
    }
    assert_eq!(paths[0].text(), "std::binaryNumbers");
    assert_eq!(paths[1].text(), "other::thing");
    // USE's own extent, both endpoints at once: this catches USE opened
    // late (after the `use` keyword instead of at it) as readily as USE
    // closed early (before the `;`) — the corpus test can't see either,
    // since it only asserts losslessness, which holds even when a
    // token is emitted OUTSIDE the node it should be inside.
    assert_eq!(
        use_node.text(),
        "use std::binaryNumbers,\n    other::thing;"
    );
}

/// The `alias`, when present, is USE_PATH's own last token — a claim
/// the `g_finish` comment inside `parse_use` makes but nothing
/// verifies: the two-path fixture above has no alias, and no shipped
/// `.tmc` file uses `use … as` either, so the whole corpus is silent on
/// this shape.
#[test]
fn a_use_path_with_an_alias() {
    let source = "use std::binaryNumbers as bn;\n";
    let root = parse(source);
    let use_node = root.children().next().expect("a USE child");
    let path = use_node
        .children()
        .next()
        .expect("a USE_PATH child carrying the alias");
    assert_eq!(kind_of(&path), "USE_PATH");
    assert_eq!(path.text(), "std::binaryNumbers as bn");
}

/// Acceptance parity is the whole point of the sink: it only mirrors an
/// UNCHANGED grammar walk, so a rejecting source must fail identically
/// through both parse paths — same error kind, same span, via
/// `CompileError`'s derived `PartialEq` — and an accepting one must
/// succeed on both. This is the one check in this file the six goldens
/// above cannot stand in for: every one of them parses `WithComments`
/// through `parse_green` alone, so a divergence that only shows up on a
/// REJECTING source, or only under `WithoutComments` lexing (the CST
/// path via `parse`), is invisible to them.
///
/// Two failure modes this specifically guards against: `GreenSink`'s own
/// `debug_assert!`s (`flush`'s out-of-order guard, `token`'s
/// already-emitted guard) firing where the CST path returns a clean
/// `Err` — a panic instead of an error IS an acceptance divergence — and
/// a builder left with an unclosed node by an error raised mid-
/// production, untested by the corpus test below since every shipped
/// `.tmc` file parses cleanly.
///
/// None of the fixtures below contain a comment, so `WithComments` vs
/// `WithoutComments` lexing produces the same token positions either
/// way (only `Comment` itself is mode-gated — `DocLine`/`AttentionLine`
/// are emitted in both), making `parse` (comment-free) and `parse_green`
/// (`WithComments`) a fair apples-to-apples comparison on these sources.
///
/// Task 5 opens far more nodes on a mid-production error path than Task
/// 4 did — every one of STATE/GRAFT/BIND/TAPE/REUSE/DOC_RUN/ATTR can be
/// left open (never `g_finish`ed) when its own production errors out
/// partway through, a shape none of the original 7 rejecting sources
/// reach (none of them get past `top_items`). The world-body-reaching
/// entries below extend the same differential check onto those paths;
/// each source and its expected code is taken from the existing
/// `parser::tests` battery, so the code is independently pinned there
/// too (`tape_declaration_outside_a_machine_is_rejected`,
/// `state_redirect_form_is_rejected`, `bare_single_tape_pattern_is_rejected`,
/// `non_entry_graft_needs_a_name`, `dangling_doc_run_is_rejected`).
#[test]
fn errors_agree_with_the_cst_path() {
    let rejecting = [
        "namespace n { machine { } }",              // unexpected-token
        "alphabet state { '_' }",                   // reserved-name
        "routine goto() { }",                       // reserved-name
        "use mylib::graph;",                        // reserved-name
        "machine { } machine { }",                  // multiple-machines
        "? orphan\nuse mylib::x;",                  // dangling-doc-run
        "? orphan\n",                               // dangling-doc-run (nothing follows)
        "routine r() { tape x: bits; }",            // tape-not-in-machine
        "machine { entry tape t: bits; }",          // "state" or "graft" after "entry"
        "machine { state s; }",                     // state-redirect
        "machine { entry state s { * -> stop; } }", // naked-pattern
        "machine { graft findX(t = work); }",       // graft-needs-name
        "machine {\n? orphan\ntape t: bits;\n}",    // dangling-doc-run, inside a world body
        "machine { entry state s {",                // Eof inside a state body
        "machine {",                                // Eof inside a world body
    ];
    for src in rejecting {
        let cst_err = parse_ast(&lex(src).expect("lexes")).expect_err("rejected on the CST path");
        let green_err: CompileError = parse_green(src).expect_err("rejected on the green path");
        assert_eq!(
            cst_err, green_err,
            "{src}: kind and span must agree exactly"
        );
    }

    let accepting = [
        "alphabet ab { '_' }\n",
        "machine {\n  tape main: ab;\n}\n",
        "namespace a {\n  namespace b {\n    export alphabet ab { '_', 'a' }\n  }\n}\n",
        "use std::binaryNumbers,\n    other::thing;\n",
        "routine r() {\n  entry state s { [*] -> stop; }\n}\n",
        "export graph g() {\n  entry state s { [*] -> stop; }\n}\n",
        "machine {\n  entry graft findX(t = work);\n}\n",
        "machine {\n  bind findX(t = work) as fx;\n}\n",
        "machine {\n  ? doc\n  entry state s { [*] -> stop; }\n}\n",
    ];
    for src in accepting {
        assert!(parse_ast(&lex(src).expect("lexes")).is_ok(), "{src}");
        assert!(parse_green(src).is_ok(), "{src}");
    }
}

/// `parse_green` computes `eof_pos = sig.len() - 1` and calls
/// `finish_tree(eof_pos)` — a path none of the goldens above exercise,
/// since every one of them has at least one significant token before
/// EOF. Empty and whitespace-only source have EOF as their only
/// significant token (`sig.len() == 1`, `eof_pos == 0`), the edge where
/// an off-by-one here would underflow or hand `finish_tree` the wrong
/// position. Both must still satisfy the lossless law.
#[test]
fn empty_and_whitespace_only_source_stay_lossless() {
    for src in ["", "\n\n\n"] {
        let tree = parse_green(src).expect("parses");
        let root = SyntaxNode::new_root(tree);
        assert_eq!(root.text(), src, "{src:?} is not lossless");
    }
}

// ---------------------------------------------------------------------------
// The remaining containers: WORLD, TAPE, STATE, RULE, GRAFT, BIND, REUSE,
// DOC_RUN, ATTR — after these, no `.tmc` construct is unstructured.
// ---------------------------------------------------------------------------

/// The brief's `matches("RULE").count() == 3` plus a textual
/// `state < first_rule` ordering check would both still pass for a tree
/// where the three RULEs sit as WORLD-level siblings of STATE rather
/// than its own children — source order (and the opening-position
/// ordering the brief checks) survives regardless of nesting depth — or
/// for a RULE that swallowed the FOLLOWING rule's leading `[` into its
/// own trailing trivia. Walking WORLD then STATE's actual children and
/// asserting each RULE's exact `text()` (its own `;` included, the next
/// rule's leading whitespace excluded) closes both gaps. Expected texts
/// are located via `find`, not hand-copied, to keep the fixture and its
/// derivation from silently drifting apart.
#[test]
fn a_state_with_three_rules() {
    let source = "machine {\n  tape main: ab;\n  entry state s {\n\
         ['b'] -> write ['a'] move [>] goto s;\n\
         ['a'] ->             move [>] goto s;\n\
         ['_'] -> stop;\n  }\n}\n";
    let d = dump(source);
    assert_eq!(d.matches("RULE").count(), 3, "{d}");
    let root = parse(source);
    let machine = root.children().next().expect("a MACHINE child");
    let world = machine.children().next().expect("a WORLD child of MACHINE");
    assert_eq!(kind_of(&world), "WORLD");
    let state = world
        .children()
        .find(|c| kind_of(c) == "STATE")
        .expect("a STATE child of WORLD");
    assert_eq!(
        state.text(),
        "entry state s {\n\
         ['b'] -> write ['a'] move [>] goto s;\n\
         ['a'] ->             move [>] goto s;\n\
         ['_'] -> stop;\n  }"
    );
    let rules: Vec<SyntaxNode> = state.children().collect();
    assert_eq!(rules.len(), 3, "three RULE children of STATE: {source}");
    for r in &rules {
        assert_eq!(kind_of(r), "RULE");
    }
    // A running cursor, not a fresh `source.find` per marker: rule 0's
    // own `write ['a']` contains the literal text `['a']` too, so an
    // unanchored search for rule 1's marker would find rule 0's write
    // vector instead of rule 1's pattern.
    let mut cursor = 0usize;
    let mut next_rule = |marker: &str| -> String {
        let rel = source[cursor..].find(marker).expect("marker present");
        let start = cursor + rel;
        let end = source[start..].find(';').expect("a `;`") + start + 1;
        cursor = end;
        source[start..end].to_string()
    };
    assert_eq!(rules[0].text(), next_rule("['b']"));
    assert_eq!(rules[1].text(), next_rule("['a']"));
    assert_eq!(rules[2].text(), next_rule("['_']"));
}

/// The brief's `matches("TAPE").count() == 2` says nothing about each
/// node's own extent — a TAPE that swallowed the following `;` into a
/// sibling, or that opened one token late, would still count to 2.
/// Walking WORLD's children and pinning each TAPE's exact `text()`
/// closes that gap.
#[test]
fn a_tape_declaration_is_its_own_node() {
    let source = "machine {\n  tape main: ab;\n  tape work: ab;\n}\n";
    let d = dump(source);
    assert_eq!(d.matches("TAPE").count(), 2, "{d}");
    let root = parse(source);
    let machine = root.children().next().expect("a MACHINE child");
    let world = machine.children().next().expect("a WORLD child");
    let tapes: Vec<SyntaxNode> = world.children().filter(|c| kind_of(c) == "TAPE").collect();
    assert_eq!(tapes.len(), 2, "{source}");
    assert_eq!(tapes[0].text(), "tape main: ab;");
    assert_eq!(tapes[1].text(), "tape work: ab;");
}

/// `volatile` is TAPE's own first token when present, the same
/// retroactive-checkpoint shape `export`/`entry` use elsewhere in this
/// file. No shipped `.tmc` file exercises a body-level
/// `volatile tape NAME: ALPHABET;` declaration — the stdlib's own
/// `volatile tape` occurrences are all the unrelated SIGNATURE-PARAMETER
/// form (`routine f(volatile tape t: alph) { … }`, a different
/// production, `sig_param`), so this fixture is the only coverage of
/// the body-declaration form.
#[test]
fn a_volatile_tape_declaration_includes_the_modifier() {
    let source = "machine {\n  volatile tape sensor: ab;\n}\n";
    let root = parse(source);
    let machine = root.children().next().expect("a MACHINE child");
    let world = machine.children().next().expect("a WORLD child");
    let tape = world.children().next().expect("a TAPE child");
    assert_eq!(kind_of(&tape), "TAPE");
    assert_eq!(tape.text(), "volatile tape sensor: ab;");
}

/// The brief's `d.contains("DOC_RUN")` plus a DOC_LINE count would both
/// still pass for a tree where DOC_RUN sits as ALPHABET's SIBLING rather
/// than its CHILD — precisely the shape Step 1's retro-wrap decision
/// changes (`crate::syntax`'s module doc). The tree walk below is the
/// actual test of that decision: ALPHABET is ROOT's first child node,
/// DOC_RUN is ALPHABET's own first child (not a preceding sibling), and
/// ALPHABET's own text begins at the run's first token, not at
/// `alphabet`.
#[test]
fn a_doc_run_before_a_declaration() {
    let source = "? one\n? two\nalphabet ab { '_' }\n";
    let d = dump(source);
    assert!(d.contains("DOC_RUN"), "{d}");
    assert_eq!(d.matches("DOC_LINE").count(), 2, "{d}");
    let root = parse(source);
    let alphabet = root.children().next().expect("an ALPHABET child of ROOT");
    assert_eq!(kind_of(&alphabet), "ALPHABET");
    let doc_run = alphabet
        .children()
        .next()
        .expect("a DOC_RUN child of ALPHABET — the retro-wrap");
    assert_eq!(kind_of(&doc_run), "DOC_RUN");
    assert_eq!(doc_run.text(), "? one\n? two");
    assert_eq!(alphabet.text(), "? one\n? two\nalphabet ab { '_' }");
}

/// The brief's `d.contains("ATTENTION_LINE")` passes whether or not
/// ATTR ever wraps anything — ATTENTION_LINE is the token kind, present
/// for ANY `!` line, attributed or not. Walking to the ATTR child
/// directly, and pinning its text to the whole line (the lexer folds
/// `[deprecated] …` into ONE token payload — `crate::syntax`'s module
/// doc — so ATTR can only ever wrap that single token, never a
/// sub-span), is the actual test of the attribute case.
#[test]
fn an_attention_line_with_an_attribute() {
    let source = "? doc\n! [deprecated] use the other one\nalphabet ab { '_' }\n";
    let root = parse(source);
    let alphabet = root.children().next().expect("an ALPHABET child");
    let doc_run = alphabet.children().next().expect("a DOC_RUN child");
    let attr = doc_run
        .children()
        .find(|c| kind_of(c) == "ATTR")
        .expect("an ATTR child of DOC_RUN");
    assert_eq!(attr.text(), "! [deprecated] use the other one");
}

/// A plain attention line — no `[ident]` prefix — never gets an ATTR
/// wrap: `parse_attr` returns `None`, so the retroactive checkpoint
/// taken at its site is simply never used (harmless, same as any other
/// unused checkpoint in this parser). The positive fixture above cannot
/// show this by itself: it only proves ATTR appears when an attribute
/// IS present, not that it is absent when one is not.
#[test]
fn a_plain_attention_line_has_no_attr_node() {
    let source = "! plain prose, no brackets\nalphabet ab { '_' }\n";
    let root = parse(source);
    let alphabet = root.children().next().expect("an ALPHABET child");
    let doc_run = alphabet.children().next().expect("a DOC_RUN child");
    assert!(
        doc_run.children().all(|c| kind_of(&c) != "ATTR"),
        "no ATTR expected for an attribute-less attention line: {}",
        doc_run.text()
    );
}

/// WORLD spans the whole brace-delimited body — braces included — the
/// shared shape `machine`/`routine`/`graph` all funnel through
/// `world_body`. MACHINE's own text is exactly `machine` followed by
/// WORLD's, with nothing in between swallowed or dropped.
#[test]
fn world_spans_the_whole_brace_delimited_body() {
    let source = "machine {\n  tape main: ab;\n}\n";
    let root = parse(source);
    let machine = root.children().next().expect("a MACHINE child");
    let world = machine.children().next().expect("a WORLD child of MACHINE");
    assert_eq!(kind_of(&world), "WORLD");
    assert_eq!(world.text(), "{\n  tape main: ab;\n}");
    assert_eq!(machine.text(), format!("machine {}", world.text()));
}

/// REUSE wraps `export`, a bound doc run, the signature, and WORLD, all
/// from ONE checkpoint — the same retro-wrap/`export`-inclusion
/// mechanism already proven for ALPHABET (`nested_namespaces`), now on
/// the shape ALPHABET's own fixtures can't exercise: a signature
/// between the name and the body.
#[test]
fn a_documented_exported_routine_wraps_its_doc_run_and_signature() {
    let source =
        "? does a thing\nexport routine r(tape t: ab) {\n  entry state s { [*] -> stop; }\n}\n";
    let root = parse(source);
    let reuse = root.children().next().expect("a REUSE child of ROOT");
    assert_eq!(kind_of(&reuse), "REUSE");
    let doc_run = reuse
        .children()
        .next()
        .expect("a DOC_RUN child of REUSE — the retro-wrap");
    assert_eq!(kind_of(&doc_run), "DOC_RUN");
    assert_eq!(doc_run.text(), "? does a thing");
    assert!(
        reuse
            .text()
            .starts_with("? does a thing\nexport routine r(tape t: ab) {"),
        "{}",
        reuse.text()
    );
    let world = reuse
        .children()
        .find(|c| kind_of(c) == "WORLD")
        .expect("a WORLD child of REUSE");
    assert_eq!(world.text(), "{\n  entry state s { [*] -> stop; }\n}");
}

/// `entry` is GRAFT's own first token when present — mirrors STATE's
/// `entry` inclusion, on the sibling production the brief's own STATE
/// fixture doesn't exercise.
#[test]
fn an_entry_graft_includes_the_entry_keyword() {
    let source = "machine {\n  entry graft findX(t = work);\n}\n";
    let root = parse(source);
    let machine = root.children().next().expect("a MACHINE child");
    let world = machine.children().next().expect("a WORLD child");
    let graft = world
        .children()
        .find(|c| kind_of(c) == "GRAFT")
        .expect("a GRAFT child");
    assert_eq!(graft.text(), "entry graft findX(t = work);");
}

/// BIND is its own node, `bind` keyword through `;` inclusive — never
/// `entry`-prefixed (the grammar has no `entry bind` form), so unlike
/// GRAFT/STATE it always opens at its own first token.
#[test]
fn a_bind_declaration_is_its_own_node() {
    let source = "machine {\n  bind findX(t = work) as fx;\n}\n";
    let root = parse(source);
    let machine = root.children().next().expect("a MACHINE child");
    let world = machine.children().next().expect("a WORLD child");
    let bind = world
        .children()
        .find(|c| kind_of(c) == "BIND")
        .expect("a BIND child");
    assert_eq!(bind.text(), "bind findX(t = work) as fx;");
}

/// Step 1's decision is that a declaration retro-wraps its bound doc
/// run — proven at file/namespace level by `a_doc_run_before_a_declaration`
/// and `a_documented_exported_routine_wraps_its_doc_run_and_signature`,
/// BOTH of which only exercise `top_items`'s checkpoint. `world_body` has
/// its own, separately-taken checkpoint for the identical mechanism one
/// level down, and nothing above pins it: deleting `world_body`'s
/// DOC_RUN wrap entirely, or moving ITS `cp` back below the doc-run
/// block, would leave every other test in this file green (the tokens
/// are still mirrored by `bump()`, the tree stays lossless, and
/// `errors_agree_with_the_cst_path`'s one world-body-doc-run fixture
/// only asserts that both parse paths ACCEPT the source, not what shape
/// either tree takes). This test is the missing pin: STATE's own first
/// child must be DOC_RUN, not a preceding sibling.
#[test]
fn a_doc_run_before_a_world_body_state_retro_wraps_into_it() {
    let source = "machine {\n  ? doc\n  entry state s { [*] -> stop; }\n}\n";
    let root = parse(source);
    let machine = root.children().next().expect("a MACHINE child");
    let world = machine.children().next().expect("a WORLD child");
    let state = world
        .children()
        .find(|c| kind_of(c) == "STATE")
        .expect("a STATE child of WORLD");
    let doc_run = state
        .children()
        .next()
        .expect("a DOC_RUN child of STATE — the retro-wrap, one level below top_items");
    assert_eq!(kind_of(&doc_run), "DOC_RUN");
    assert_eq!(doc_run.text(), "? doc");
    // STATE's own extent, derived from the source rather than hand-typed
    // (the earlier `a_state_with_three_rules` transcription slip is the
    // reason): starts at `? doc` (the checkpoint precedes it), ends at
    // this state's own closing `}` — the `\n  ` between `? doc` and
    // `entry` is flushed into WORLD before `g_start_at(cp, State)` runs,
    // then pulled in retroactively along with everything else since the
    // checkpoint, so it lands inside STATE too.
    let doc_start = source.find("? doc").expect("marker present");
    let marker = "stop; }";
    let state_close = source.find(marker).expect("marker present") + marker.len();
    assert_eq!(state.text(), &source[doc_start..state_close]);
}

/// The brief's four goldens plus this task's own additions leave several
/// dispatch sites with no assertion at all: plain (non-`export`)
/// `routine`/`graph` REUSE, `export graph` (only `export routine` is
/// assertion-pinned, by `a_documented_exported_routine_…` above), and
/// plain (non-`entry`) `state`/`graft`. One library-file fixture,
/// exercising all five at once, closes the gap cheaper than five
/// separate tests would.
#[test]
fn plain_and_exported_reuse_sites_carry_plain_states_and_grafts() {
    let source = "routine r() {\n  state s { [*] -> stop; }\n  graft findX(t = work) as fx;\n}\n\
                  graph g() {\n  state s { [*] -> stop; }\n  graft findX(t = work) as fx;\n}\n\
                  export graph eg() {\n  state s { [*] -> stop; }\n}\n";
    let root = parse(source);
    let reuses: Vec<SyntaxNode> = root.children().collect();
    assert_eq!(reuses.len(), 3, "{source}");
    for r in &reuses[..2] {
        assert_eq!(kind_of(r), "REUSE");
        let world = r
            .children()
            .find(|c| kind_of(c) == "WORLD")
            .expect("a WORLD child");
        let state = world
            .children()
            .find(|c| kind_of(c) == "STATE")
            .expect("a plain STATE child");
        assert_eq!(state.text(), "state s { [*] -> stop; }");
        let graft = world
            .children()
            .find(|c| kind_of(c) == "GRAFT")
            .expect("a plain GRAFT child");
        assert_eq!(graft.text(), "graft findX(t = work) as fx;");
    }
    assert!(reuses[0].text().starts_with("routine r()"));
    assert!(reuses[1].text().starts_with("graph g()"));
    // `export graph` — the one REUSE dispatch site the tests above never
    // touch (`export routine` is pinned by
    // `a_documented_exported_routine_wraps_its_doc_run_and_signature`,
    // plain `routine`/`graph` just above): `export` must be included in
    // REUSE's own extent, the same inclusion ALPHABET already proved for
    // `export alphabet`.
    assert_eq!(kind_of(&reuses[2]), "REUSE");
    assert_eq!(
        reuses[2].text(),
        "export graph eg() {\n  state s { [*] -> stop; }\n}"
    );
}

/// The law over every `.tmc` the repo ships, including the flagship
/// brainfuck universal machine and the embedded stdlib.
#[test]
fn the_whole_shipped_corpus_is_lossless() {
    let mut checked = 0;
    for dir in ["tests/golden", "src/stdlib", "../../docs/examples"] {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        for entry in entries {
            let path = entry.expect("entry").path();
            if path.extension().and_then(|e| e.to_str()) != Some("tmc") {
                continue;
            }
            let src = std::fs::read_to_string(&path).expect("readable");
            let tree = parse_green(&src)
                .unwrap_or_else(|e| panic!("{} failed to parse: {e:?}", path.display()));
            let root = SyntaxNode::new_root(tree);
            assert_eq!(root.text(), src, "{} is not lossless", path.display());
            checked += 1;
        }
    }
    assert!(
        checked >= 9,
        "expected the whole .tmc corpus, saw {checked}"
    );
}
