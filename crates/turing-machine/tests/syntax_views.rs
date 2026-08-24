//! Typed views over the `.tmc` green tree. These tests pin the view
//! CONTRACT — which node a view accepts and which it refuses — and each
//! accessor's answer for the shapes that do and do not have the child it
//! names. Extraction parity is a separate file; a view that returns the
//! wrong child still round-trips, so losslessness cannot catch it.

use mtc_core::syntax::{AstNode, SyntaxNode, TreeBuilder};
use mtc_turing_machine::parser::parse_green;
use mtc_turing_machine::syntax::{
    AlphabetView, MachineView, NamespaceView, ReuseKind, ReuseView, RootView, TapeView, TmcKind,
    UsePathView, UseView,
};

fn tree(src: &str) -> SyntaxNode {
    SyntaxNode::new_root(parse_green(src).expect("parses"))
}

fn first_of(root: &SyntaxNode, kind: TmcKind) -> SyntaxNode {
    fn go(n: &SyntaxNode, k: TmcKind, out: &mut Option<SyntaxNode>) {
        for c in n.children() {
            if out.is_none() && c.kind() == k.into() {
                *out = Some(c.clone());
            }
            go(&c, k, out);
        }
    }
    let mut out = None;
    go(root, kind, &mut out);
    out.expect("node present")
}

/// A view casts from its own kind and refuses every other. This is the
/// whole contract the rest of the layer rests on: an accessor that
/// silently accepted a foreign node would return plausible nonsense.
#[test]
fn a_view_accepts_its_own_kind_and_refuses_others() {
    let root = tree("alphabet ab { '_' }\n\nmachine {\n  tape main: ab;\n}\n");
    let alphabet = first_of(&root, TmcKind::Alphabet);
    let machine = first_of(&root, TmcKind::Machine);

    assert!(AlphabetView::cast(alphabet.clone()).is_some());
    assert!(MachineView::cast(machine.clone()).is_some());
    assert!(RootView::cast(root.clone()).is_some());

    assert!(
        AlphabetView::cast(machine).is_none(),
        "AlphabetView took a MACHINE"
    );
    assert!(
        MachineView::cast(alphabet).is_none(),
        "MachineView took an ALPHABET"
    );
    assert!(RootView::cast(first_of(&root, TmcKind::Tape)).is_none());
}

/// Every view kind casts from the node it names. Enumerated rather than
/// sampled, and the enumeration is itself checked against the kind
/// space: node kinds occupy the contiguous discriminant run `32..=46`,
/// spelled here as literals — `TmcKind::Root as u16` would be the table
/// comparing itself to itself. A node kind inserted anywhere but the
/// very end of that run renumbers every kind after it, so a table left
/// stale for the new kind produces a set that diverges from the run.
/// What this catches: a listed view that stops casting, and a table
/// left stale across a mid-run insertion. What it does NOT catch: a
/// node kind appended after `Root` with no view and no entry here,
/// since nothing already listed moves. Same blind spot, by the same
/// argument, as the significant-token census in `syntax::kinds`.
#[test]
#[allow(clippy::type_complexity)] // the checks table's own type, not worth a named alias for one test
fn every_node_kind_has_a_view_that_casts_from_it() {
    // Verified against the real parser before this plan shipped: it parses,
    // and it yields all fifteen node kinds with none missing. Three things
    // it must keep — `use` accepts NO doc run (`DanglingDocRun` otherwise;
    // only export/alphabet/routine/graph/machine/namespace do), a signature
    // parameter writes its keyword FIRST (`tape t: ab`, `state done`), and
    // a namespace must be present or NAMESPACE has no instance to cast from.
    let src = "use a::b;\n\n? doc\n! [deprecated] old\nalphabet ab { '_', 'a' }\n\n\
               namespace n {\n  alphabet inner { '_' }\n}\n\n\
               graph g(tape t: ab, state done) {\n\
               \x20 entry state gs { [*] -> done; }\n}\n\n\
               routine r(tape t: ab) {\n\
               \x20 entry graft g(t = t, done = return);\n}\n\n\
               machine {\n  tape main: ab;\n  bind r() as hh;\n\
               \x20 entry state s {\n    ['a'] -> write ['_'] move [>] goto s;\n\
               \x20   [*] -> stop;\n  }\n}\n";
    let root = tree(src);
    // Each entry: the kind, and a closure proving a view casts from it.
    let checks: [(TmcKind, fn(SyntaxNode) -> bool); 15] = [
        (TmcKind::Root, |n| RootView::cast(n).is_some()),
        (TmcKind::Use, |n| {
            mtc_turing_machine::syntax::UseView::cast(n).is_some()
        }),
        (TmcKind::UsePath, |n| {
            mtc_turing_machine::syntax::UsePathView::cast(n).is_some()
        }),
        (TmcKind::Alphabet, |n| AlphabetView::cast(n).is_some()),
        (TmcKind::Reuse, |n| {
            mtc_turing_machine::syntax::ReuseView::cast(n).is_some()
        }),
        (TmcKind::Machine, |n| MachineView::cast(n).is_some()),
        (TmcKind::Namespace, |n| {
            mtc_turing_machine::syntax::NamespaceView::cast(n).is_some()
        }),
        (TmcKind::World, |n| {
            mtc_turing_machine::syntax::WorldView::cast(n).is_some()
        }),
        (TmcKind::Tape, |n| {
            mtc_turing_machine::syntax::TapeView::cast(n).is_some()
        }),
        (TmcKind::State, |n| {
            mtc_turing_machine::syntax::StateView::cast(n).is_some()
        }),
        (TmcKind::Rule, |n| {
            mtc_turing_machine::syntax::RuleView::cast(n).is_some()
        }),
        (TmcKind::Graft, |n| {
            mtc_turing_machine::syntax::GraftView::cast(n).is_some()
        }),
        (TmcKind::Bind, |n| {
            mtc_turing_machine::syntax::BindView::cast(n).is_some()
        }),
        (TmcKind::DocRun, |n| {
            mtc_turing_machine::syntax::DocRunView::cast(n).is_some()
        }),
        (TmcKind::Attr, |n| {
            mtc_turing_machine::syntax::AttrView::cast(n).is_some()
        }),
    ];
    // The table IS the node half of the kind space, not a sample of it.
    let mut listed: Vec<u16> = checks.iter().map(|(k, _)| *k as u16).collect();
    listed.sort_unstable();
    let expected: Vec<u16> = (32..=46).collect();
    assert_eq!(
        listed, expected,
        "the view table is not the contiguous node run 32..=46"
    );
    for (kind, casts) in checks {
        let node = if kind == TmcKind::Root {
            root.clone()
        } else {
            first_of(&root, kind)
        };
        assert!(casts(node), "no view casts from {kind:?}");
    }
}

// -- container accessors ------------------------------------------------

/// `items()` yields the top-level declarations in source order, and
/// nothing else — not the trivia between them, not the doc runs folded
/// into them.
#[test]
fn root_items_are_the_declarations_in_source_order() {
    let root = tree("use a::b;\n\nalphabet ab { '_' }\n\nmachine {\n  tape main: ab;\n}\n");
    let view = RootView::cast(root).expect("root");
    let kinds: Vec<TmcKind> = view.items().map(|i| i.kind()).collect();
    assert_eq!(
        kinds,
        vec![TmcKind::Use, TmcKind::Alphabet, TmcKind::Machine]
    );
}

/// A namespace's own items, not the file's — and not its nested
/// namespace's either. The nested case is the one that matters: a
/// descendant walk passes the sibling-isolation check, so only a
/// namespace inside a namespace tells the two walks apart.
#[test]
fn namespace_items_are_scoped_to_it() {
    let root = tree(
        "alphabet outer { '_' }\n\n\
         namespace n {\n  alphabet direct { '_' }\n\
         \x20 namespace inner {\n    alphabet nested { '_' }\n  }\n}\n",
    );
    let ns = NamespaceView::cast(first_of(&root, TmcKind::Namespace)).expect("ns");
    assert_eq!(ns.name(), "n");

    let names: Vec<String> = ns
        .items()
        .filter_map(|i| AlphabetView::cast(i.node().clone()))
        .map(|a| a.name_token().text().to_string())
        .collect();
    assert_eq!(
        names,
        vec!["direct"],
        "items() must yield this namespace's own alphabets — not the file's, not the nested namespace's"
    );

    // The nested namespace itself IS a direct child, and appears as one item.
    let kinds: Vec<TmcKind> = ns.items().map(|i| i.kind()).collect();
    assert_eq!(kinds, vec![TmcKind::Alphabet, TmcKind::Namespace]);
}

// A parity gap with the PM sibling, recorded so nobody goes looking for
// the missing test: PM's `use_path_as_marker_is_positional_not_textual`
// (`crates/post-machine/src/syntax/views.rs`) builds `use as as as;` to
// prove its alias marker is found by POSITION, never by comparing a
// segment's text against `"as"` — in `.pmc`, a name can legally spell
// `as`, so the two ways of finding the marker could in principle
// disagree and the test pins that they don't. `.tmc` has no counterpart
// and cannot: `as` is one of the 27 words in `lexer::RESERVED`, so
// `Parser::name()` rejects it wherever a name is expected and `use as
// as as;` never parses — a segment can never literally spell `as`, so
// `use_path_parts`'s positional split and a hypothetical textual one
// would never have anything to disagree about. The hazard PM's test
// guards against does not exist on this side of the language.

/// A path's segments, and its alias when written.
#[test]
fn use_paths_expose_segments_and_alias() {
    let root = tree("use a::b::c, d::e as f;\n");
    let u = UseView::cast(first_of(&root, TmcKind::Use)).expect("use");
    let paths: Vec<UsePathView> = u.paths().collect();
    assert_eq!(paths.len(), 2);

    let segs: Vec<String> = paths[0]
        .segments()
        .iter()
        .map(|t| t.text().to_string())
        .collect();
    assert_eq!(segs, vec!["a", "b", "c"]);
    assert!(paths[0].alias_token().is_none(), "no alias was written");

    assert_eq!(
        paths[1].alias_token().map(|t| t.text().to_string()),
        Some("f".to_string())
    );
}

/// `exported` reads the `export` keyword's presence, and `doc_run` finds
/// the run the declaration retro-wraps.
#[test]
fn alphabet_exposes_export_glyphs_and_its_doc_run() {
    let root = tree("? doc\nexport alphabet ab { '_', 'a' }\n");
    let a = AlphabetView::cast(first_of(&root, TmcKind::Alphabet)).expect("alphabet");
    assert!(a.exported());
    assert_eq!(a.name_token().text(), "ab");
    let glyphs: Vec<String> = a
        .glyph_tokens()
        .iter()
        .map(|t| t.text().to_string())
        .collect();
    assert_eq!(glyphs, vec!["'_'", "'a'"]);
    assert!(
        a.doc_run().is_some(),
        "the run is the alphabet's own first child"
    );

    let plain = tree("alphabet ab { '_' }\n");
    let p = AlphabetView::cast(first_of(&plain, TmcKind::Alphabet)).expect("alphabet");
    assert!(!p.exported());
    assert!(p.doc_run().is_none());
}

/// `glyph_tokens` walks DIRECT children, never descendants. No parsed
/// fixture can pin this: the grammar cannot nest a glyph-carrying node
/// inside an ALPHABET, so both walks agree on every program that
/// exists. The shape is built directly instead, because the requirement
/// is about what the accessor PROMISES, not about what today's grammar
/// happens to allow — a descendant walk would start answering
/// differently the day the grammar nests, and losslessness would not
/// notice.
#[test]
fn alphabet_glyphs_do_not_descend_into_child_nodes() {
    let mut b = TreeBuilder::new();
    b.start_node(TmcKind::Alphabet.into());
    b.token(TmcKind::Ident.into(), "ab");
    b.token(TmcKind::LBrace.into(), "{");
    b.token(TmcKind::Glyph.into(), "'_'");
    // A child NODE carrying a glyph of its own: synthetic, unreachable
    // through the parser, and exactly what a descendant walk would take.
    b.start_node(TmcKind::DocRun.into());
    b.token(TmcKind::Glyph.into(), "'x'");
    b.finish_node();
    b.token(TmcKind::RBrace.into(), "}");
    b.finish_node();

    let a = AlphabetView::cast(SyntaxNode::new_root(b.finish())).expect("alphabet");
    let glyphs: Vec<String> = a
        .glyph_tokens()
        .iter()
        .map(|t| t.text().to_string())
        .collect();
    assert_eq!(
        glyphs,
        vec!["'_'"],
        "glyph_tokens descended into a child node"
    );
}

/// `exported()`'s scan is a token-only walk over ALPHABET's own
/// header, so it never descends into a child NODE — this fixture's
/// tree (`mtc_core::syntax::debug_dump`, via
/// `crates/turing-machine/src/syntax/kinds.rs`'s `kind_name`) is the
/// case that actually exercises that: ATTR nests inside DOC_RUN, not
/// beside it, and `exported()` still reads the right IDENT.
///
/// ```text
/// ROOT@0..53
///   ALPHABET@0..52
///     DOC_RUN@0..25
///       DOC_LINE@0..5 "? doc"
///       WHITESPACE@5..6 "\n"
///       ATTR@6..25
///         ATTENTION_LINE@6..25 "! [deprecated] gone"
///     WHITESPACE@25..26 "\n"
///     IDENT@26..32 "export"
///     WHITESPACE@32..33 " "
///     IDENT@33..41 "alphabet"
///     WHITESPACE@41..42 " "
///     IDENT@42..44 "ab"
///     WHITESPACE@44..45 " "
///     L_BRACE@45..46 "{"
///     WHITESPACE@46..47 " "
///     GLYPH@47..50 "'_'"
///     WHITESPACE@50..51 " "
///     R_BRACE@51..52 "}"
///   WHITESPACE@52..53 "\n"
/// ```
#[test]
fn an_attribute_before_an_alphabet_does_not_shift_its_header() {
    let root = tree("? doc\n! [deprecated] gone\nexport alphabet ab { '_' }\n");
    let a = AlphabetView::cast(first_of(&root, TmcKind::Alphabet)).expect("alphabet");
    assert!(
        a.exported(),
        "the attribute shifted which IDENT reads as first"
    );
    assert_eq!(a.name_token().text(), "ab");
}

// -- world accessors ------------------------------------------------------

/// WORLD sits between a machine and its body items, so `world()` is the
/// step a caller must take before reaching tapes or states. This test
/// exists because forgetting it yields an empty body rather than an
/// error.
#[test]
fn a_machines_body_is_reached_through_its_world() {
    let root = tree("machine {\n  tape main: ab;\n  tape work: ab;\n}\n");
    let m = MachineView::cast(first_of(&root, TmcKind::Machine)).expect("machine");
    let w = m.world().expect("every machine has a world");
    let names: Vec<String> = w
        .tapes()
        .map(|t| t.name_token().text().to_string())
        .collect();
    assert_eq!(names, vec!["main", "work"]);
}

/// `volatile` is a modifier on the tape, not a separate declaration.
#[test]
fn tapes_expose_their_alphabet_and_volatility() {
    let root = tree("machine {\n  tape main: ab;\n  volatile tape scratch: ab;\n}\n");
    let w = MachineView::cast(first_of(&root, TmcKind::Machine))
        .expect("machine")
        .world()
        .expect("world");
    let tapes: Vec<TapeView> = w.tapes().collect();
    assert_eq!(tapes.len(), 2);
    assert_eq!(tapes[0].alphabet_token().text(), "ab");
    assert!(!tapes[0].volatile());
    assert!(
        tapes[1].volatile(),
        "the modifier belongs to the second tape"
    );
}

/// `WorldView::states/grafts/binds` each filter to their own node
/// kind. A world carrying a tape, two states, two grafts and two
/// binds all at once is the fixture that discriminates a correct
/// per-kind filter from one that accidentally yields the union of
/// several kinds, or the wrong kind outright — and comparing each
/// node's own source text (rather than just a count, and rather than
/// a kind-specific accessor: `StateView`/`GraftView`/`BindView` carry
/// none yet, that is a later task) pins both identity and document
/// order in one assertion per accessor.
#[test]
fn world_states_grafts_and_binds_are_kind_filtered_and_ordered() {
    let root = tree(
        "machine {\n  tape main: ab;\n  tape work: ab;\n\
         \x20 entry state s { [*] -> stop; }\n  state t { [*] -> halt; }\n\
         \x20 graft g(t = main, done = stop) as gg1;\n\
         \x20 graft g(t = work, done = stop) as gg2;\n\
         \x20 bind r() as hh1;\n  bind r() as hh2;\n}\n",
    );
    let w = MachineView::cast(first_of(&root, TmcKind::Machine))
        .expect("machine")
        .world()
        .expect("world");

    let states: Vec<String> = w.states().map(|s| s.syntax().text()).collect();
    assert_eq!(
        states,
        vec![
            "entry state s { [*] -> stop; }".to_string(),
            "state t { [*] -> halt; }".to_string(),
        ]
    );

    let grafts: Vec<String> = w.grafts().map(|g| g.syntax().text()).collect();
    assert_eq!(
        grafts,
        vec![
            "graft g(t = main, done = stop) as gg1;".to_string(),
            "graft g(t = work, done = stop) as gg2;".to_string(),
        ]
    );

    let binds: Vec<String> = w.binds().map(|b| b.syntax().text()).collect();
    assert_eq!(
        binds,
        vec![
            "bind r() as hh1;".to_string(),
            "bind r() as hh2;".to_string(),
        ]
    );
}

/// A routine and a graph are both REUSE nodes; the view distinguishes
/// them, because everything downstream treats them differently.
#[test]
fn reuse_distinguishes_a_routine_from_a_graph() {
    let root = tree(
        "routine r(tape t: ab) {\n  entry state s { [*] -> stop; }\n}\n\n\
         graph g(tape t: ab, state done) {\n  entry state s { [*] -> done; }\n}\n",
    );
    let reuses: Vec<ReuseView> = RootView::cast(root)
        .expect("root")
        .items()
        .filter_map(|i| ReuseView::cast(i.node().clone()))
        .collect();
    assert_eq!(reuses.len(), 2);
    assert_eq!(reuses[0].kind(), ReuseKind::Routine);
    assert_eq!(reuses[1].kind(), ReuseKind::Graph);
    assert_eq!(reuses[0].name_token().text(), "r");
}

/// `export routine`, not plain `routine`, is the fixture that actually
/// pins `kind()`'s "second-to-last header IDENT" rule: on a plain
/// `routine r(...)` the keyword IS the first header IDENT, so a wrong
/// implementation reading `idents[0]` instead of `idents[len - 2]`
/// would pass the test above too. This also closes the one-fixture gap
/// left by the export/no-export split: `signature`, `world` and
/// `doc_run` had never run on a `REUSE` at all before this test, and
/// `exported` had never run on one that answers `true`.
#[test]
fn an_exported_reuse_still_reads_its_keyword_name_and_signature() {
    let root =
        tree("? doc\nexport routine r(tape t: ab) {\n  entry graft g(t = t, done = return);\n}\n");
    let r = ReuseView::cast(first_of(&root, TmcKind::Reuse)).expect("reuse");
    assert_eq!(r.kind(), ReuseKind::Routine);
    assert_eq!(r.name_token().text(), "r");
    assert!(r.exported(), "export precedes the keyword IDENT");
    let sig: Vec<String> = r.signature().iter().map(|t| t.text().to_string()).collect();
    assert_eq!(
        sig,
        vec!["(", "tape", "t", ":", "ab", ")"],
        "signature() must keep the parens and drop interstitial whitespace"
    );
    assert!(
        r.world().is_some(),
        "every parsed REUSE carries a WORLD body"
    );
    assert!(
        r.doc_run().is_some(),
        "the run is the reuse's own retro-wrapped first child"
    );
}

/// `MachineView::doc_run` sourced its claim from the module doc's list
/// of doc-run-accepting declarations rather than from a run — this
/// pins it against the real parser.
#[test]
fn a_machines_doc_run_is_the_run_it_retro_wraps() {
    let root = tree("? doc\nmachine {\n  tape main: ab;\n}\n");
    let m = MachineView::cast(first_of(&root, TmcKind::Machine)).expect("machine");
    assert!(
        m.doc_run().is_some(),
        "the run is the machine's own retro-wrapped first child"
    );
    assert!(
        m.world().is_some(),
        "world() must still find WORLD past a leading DOC_RUN"
    );

    let plain = tree("machine {\n  tape main: ab;\n}\n");
    let p = MachineView::cast(first_of(&plain, TmcKind::Machine)).expect("machine");
    assert!(p.doc_run().is_none(), "no run was written");
}
