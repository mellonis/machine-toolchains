//! Typed views over the `.tmc` green tree. These tests pin the view
//! CONTRACT — which node a view accepts and which it refuses — and each
//! accessor's answer for the shapes that do and do not have the child it
//! names. Extraction parity is a separate file; a view that returns the
//! wrong child still round-trips, so losslessness cannot catch it.

use mtc_core::syntax::{AstNode, SyntaxNode, TreeBuilder};
use mtc_turing_machine::parser::parse_green;
use mtc_turing_machine::syntax::{
    AlphabetView, AttrView, BindView, BindingArgView, DocRunView, GraftView, MachineView,
    NamespaceView, ReuseKind, ReuseView, RootView, RuleView, SigParamKind, StateView, TapeView,
    TmcKind, UsePathView, UseView,
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
/// space: node kinds occupy the contiguous discriminant run `32..=53`,
/// spelled here as literals — `TmcKind::SymMap as u16` would be the
/// table comparing itself to itself. A node kind inserted anywhere but the
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
    // Verified against the real parser: it parses, and it yields all
    // twenty-two node kinds with none missing (checked by dumping the tree
    // and set-comparing the kind names against `syntax::kind_name`). Five
    // things it must keep — `use` accepts NO doc run (`DanglingDocRun`
    // otherwise; only export/alphabet/routine/graph/machine/namespace do), a
    // signature parameter writes its keyword FIRST (`tape t: ab`,
    // `state done`), a namespace must be present or NAMESPACE has no
    // instance to cast from, a signature tape parameter must carry a
    // `writes`/`preserves` clause or CONTRACT_CLAUSE has none, and a binding
    // argument must carry `with map { … }` or SYM_MAP has none.
    let src = "use a::b;\n\n? doc\n! [deprecated] old\nalphabet ab { '_', 'a' }\n\n\
               namespace n {\n  alphabet inner { '_' }\n}\n\n\
               graph g(tape t: ab writes { '_' }, state done) {\n\
               \x20 entry state gs { [*] -> done; }\n}\n\n\
               routine r(tape t: ab) {\n\
               \x20 entry graft g(t = t with map { '_' -> 'a' }, done = return);\n}\n\n\
               machine {\n  tape main: ab;\n  bind r() as hh;\n\
               \x20 entry state s {\n    ['a'] -> write ['_'] move [>] goto s;\n\
               \x20   [*] -> stop;\n  }\n}\n";
    let root = tree(src);
    // Each entry: the kind, and a closure proving a view casts from it.
    let checks: [(TmcKind, fn(SyntaxNode) -> bool); 22] = [
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
        (TmcKind::SigParam, |n| {
            mtc_turing_machine::syntax::SigParamView::cast(n).is_some()
        }),
        (TmcKind::ContractClause, |n| {
            mtc_turing_machine::syntax::ContractClauseView::cast(n).is_some()
        }),
        (TmcKind::WriteVec, |n| {
            mtc_turing_machine::syntax::WriteVecView::cast(n).is_some()
        }),
        (TmcKind::MoveVec, |n| {
            mtc_turing_machine::syntax::MoveVecView::cast(n).is_some()
        }),
        (TmcKind::Transition, |n| {
            mtc_turing_machine::syntax::TransitionView::cast(n).is_some()
        }),
        (TmcKind::BindingArg, |n| {
            mtc_turing_machine::syntax::BindingArgView::cast(n).is_some()
        }),
        (TmcKind::SymMap, |n| {
            mtc_turing_machine::syntax::SymMapView::cast(n).is_some()
        }),
    ];
    // The table IS the node half of the kind space, not a sample of it.
    let mut listed: Vec<u16> = checks.iter().map(|(k, _)| *k as u16).collect();
    listed.sort_unstable();
    let expected: Vec<u16> = (32..=53).collect();
    assert_eq!(
        listed, expected,
        "the view table is not the contiguous node run 32..=53"
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

// -- the interior boundaries ---------------------------------------------
//
// Each test below asserts a value a consumer of the flat token run could
// not produce without re-deriving a parser decision, and each was checked
// against a deliberately-dropped `g_start`/`g_finish` pair in the parser
// before it was kept.

/// Every node of `kind` anywhere under `root`, in document order.
fn all_of(root: &SyntaxNode, kind: TmcKind) -> Vec<SyntaxNode> {
    fn go(n: &SyntaxNode, k: TmcKind, out: &mut Vec<SyntaxNode>) {
        for c in n.children() {
            if c.kind() == k.into() {
                out.push(c.clone());
            }
            go(&c, k, out);
        }
    }
    let mut out = Vec::new();
    go(root, kind, &mut out);
    out
}

/// The shape that motivated bracketing the signature at all: two
/// parameters, two commas, and only ONE of them a separator — so a
/// consumer splitting the flat run reads THREE parameters where there
/// are two. The assertion on the comma count is not decoration: it is
/// the measurement that makes the rest of the test meaningful, because
/// it shows exactly what a comma-splitting consumer would have found.
#[test]
fn a_writes_clause_puts_commas_where_a_splitter_would_read_separators() {
    let root = tree(
        "alphabet x { '0', '1' }\nalphabet y { '_' }\n\
         routine r(tape a: x writes { '0', '1' }, tape b: y) {\n\
         \x20 entry state s { [*, *] -> return; }\n}\n",
    );
    let r = ReuseView::cast(first_of(&root, TmcKind::Reuse)).expect("reuse");

    let commas = r
        .signature()
        .iter()
        .filter(|t| t.kind() == TmcKind::Comma.into())
        .count();
    assert_eq!(
        commas, 2,
        "the flat signature run carries two commas; only one separates parameters, \
         so a splitter reads three parameters where there are two"
    );

    let params: Vec<_> = r.params().collect();
    assert_eq!(params.len(), 2, "two parameters, not four");
    let names: Vec<String> = params
        .iter()
        .map(|p| p.name_token().text().to_string())
        .collect();
    assert_eq!(names, vec!["a", "b"]);
    let alphabets: Vec<Option<String>> = params
        .iter()
        .map(|p| p.alphabet_token().map(|t| t.text().to_string()))
        .collect();
    assert_eq!(
        alphabets,
        vec![Some("x".to_string()), Some("y".to_string())],
        "the alphabet is the IDENT after `:`, not the `writes` keyword"
    );
    assert_eq!(
        params[0].syntax().text(),
        "tape a: x writes { '0', '1' }",
        "a parameter's extent runs through its own clause, not to the first comma"
    );
}

/// A clause is identified by its own keyword, never by its position:
/// both are optional, so position says nothing on a parameter carrying
/// only one of them (measured — on `tape a: x preserves { '1' }` the
/// clause at position 0 is `preserves`).
///
/// The two extra shapes earn their place for different reasons, both
/// measured by mutation. `volatile` is the one that breaks a NAIVE
/// positional read: with `name_token` forced to `.nth(1)` this test
/// reads `"tape"` for the name, and with `alphabet_token` forced to
/// `.nth(2)` it reads `Some("a")` for the alphabet. The `state`
/// parameter breaks neither of those — a naive read answers it
/// correctly — and is here instead as the only parameter form with no
/// alphabet and no clause, pinning the `State` arm (forcing `kind()`
/// to return `Tape` fails this test) alongside the `None` and empty
/// answers.
#[test]
fn contract_clauses_are_named_by_their_own_keyword() {
    let root = tree(
        "alphabet x { '0', '1' }\n\
         routine r(volatile tape a: x writes { '0' } preserves { '1' }, state done) {\n\
         \x20 entry state s { [*] -> done; }\n}\n",
    );
    let r = ReuseView::cast(first_of(&root, TmcKind::Reuse)).expect("reuse");
    let params: Vec<_> = r.params().collect();
    assert_eq!(params.len(), 2);

    assert!(params[0].volatile());
    assert_eq!(params[0].kind(), SigParamKind::Tape);
    assert_eq!(params[0].name_token().text(), "a");
    assert_eq!(
        params[0].alphabet_token().map(|t| t.text().to_string()),
        Some("x".to_string()),
        "`volatile` shifts the name and alphabet by one IDENT"
    );
    let clauses: Vec<_> = params[0].contract_clauses().collect();
    let keywords: Vec<String> = clauses
        .iter()
        .map(|c| c.keyword_token().text().to_string())
        .collect();
    assert_eq!(keywords, vec!["writes", "preserves"]);
    assert_eq!(clauses[0].syntax().text(), "writes { '0' }");
    assert_eq!(clauses[1].syntax().text(), "preserves { '1' }");

    assert!(!params[1].volatile());
    assert_eq!(params[1].kind(), SigParamKind::State);
    assert_eq!(params[1].name_token().text(), "done");
    assert!(
        params[1].alphabet_token().is_none(),
        "a state parameter declares no alphabet"
    );
    assert_eq!(params[1].contract_clauses().count(), 0);
}

/// Three bracket groups of identical shape in one rule; only the node's
/// KIND tells them apart, and only a per-rule walk shows which rule has
/// which. The second rule is the one that matters: it carries a move
/// vector and no write vector, so a consumer indexing bracket groups
/// positionally would read its move vector as a write.
#[test]
fn a_rules_write_and_move_vectors_are_told_apart_by_kind_not_position() {
    let root = tree(
        "alphabet x { '0', '1' }\nmachine {\n  tape m: x;\n\
         \x20 entry state s {\n    ['0'] -> write ['1'] move [>] goto s;\n\
         \x20   ['1'] -> move [<] goto s;\n  }\n}\n",
    );
    let rules = all_of(&root, TmcKind::Rule);
    assert_eq!(rules.len(), 2);

    let writes: Vec<String> = all_of(&rules[0], TmcKind::WriteVec)
        .iter()
        .map(|n| n.text())
        .collect();
    let moves: Vec<String> = all_of(&rules[0], TmcKind::MoveVec)
        .iter()
        .map(|n| n.text())
        .collect();
    assert_eq!(
        writes,
        vec!["['1']"],
        "the node opens at `[`, not at `write`"
    );
    assert_eq!(moves, vec!["[>]"]);

    assert_eq!(
        all_of(&rules[1], TmcKind::WriteVec).len(),
        0,
        "the second rule writes nothing"
    );
    let moves: Vec<String> = all_of(&rules[1], TmcKind::MoveVec)
        .iter()
        .map(|n| n.text())
        .collect();
    assert_eq!(
        moves,
        vec!["[<]"],
        "its lone bracket group after `->` is a MOVE vector"
    );
}

/// `Transition::Stay` is the ABSENCE of a TRANSITION node. This is the
/// one fact about a rule that no token run can carry: an omitted
/// transition leaves nothing behind but the `;` that would have followed
/// it either way.
#[test]
fn an_omitted_transition_is_the_absence_of_a_transition_node() {
    let root = tree(
        "alphabet x { '0', '1' }\nmachine {\n  tape m: x;\n\
         \x20 entry state s {\n    ['0'] -> write ['1'];\n\
         \x20   ['1'] -> debugger;\n\
         \x20   [*] -> move [.] goto s;\n\
         \x20   [*] -> stop;\n  }\n}\n",
    );
    let rules = all_of(&root, TmcKind::Rule);
    assert_eq!(rules.len(), 4);

    let written: Vec<Vec<String>> = rules
        .iter()
        .map(|r| {
            all_of(r, TmcKind::Transition)
                .iter()
                .map(|n| n.text())
                .collect()
        })
        .collect();
    assert_eq!(
        written,
        vec![
            Vec::<String>::new(),
            Vec::<String>::new(),
            vec!["goto s".to_string()],
            vec!["stop".to_string()],
        ],
        "rules 1 and 2 omit their transition — `Stay` — and 3 and 4 write one"
    );
}

/// A binding list splits around an interior symbol map for the same
/// reason a signature splits around a contract clause, and the map's own
/// node opens at `map`, not at the `with` that introduces it — so its
/// extent is `SymMap::span` exactly.
#[test]
fn binding_arguments_split_around_an_interior_symbol_map() {
    let root = tree(
        "alphabet ab { '_', 'a' }\nroutine r(tape t: ab, state done) {\n\
         \x20 entry state s { [*] -> done; }\n}\n\
         machine {\n  tape main: ab;\n\
         \x20 bind r(t = main with map { '_' -> 'a', 'a' => '_' }, done = stop) as bb;\n\
         \x20 entry state s { [*] -> stop; }\n}\n",
    );
    let bind = first_of(&root, TmcKind::Bind);

    let commas = bind
        .descendant_tokens()
        .filter(|t| t.kind() == TmcKind::Comma.into())
        .count();
    assert_eq!(
        commas, 2,
        "two commas in the bind; only one separates arguments, so a splitter reads \
         three arguments where there are two"
    );

    let args: Vec<_> = all_of(&bind, TmcKind::BindingArg)
        .into_iter()
        .map(|n| BindingArgView::cast(n).expect("binding arg"))
        .collect();
    assert_eq!(args.len(), 2, "two arguments, not four");
    let names: Vec<String> = args
        .iter()
        .map(|a| a.name_token().text().to_string())
        .collect();
    assert_eq!(names, vec!["t", "done"]);
    assert_eq!(
        args[0].syntax().text(),
        "t = main with map { '_' -> 'a', 'a' => '_' }",
        "an argument's extent runs through its own map, not to the first comma"
    );
    assert_eq!(
        args[0]
            .sym_map()
            .expect("the first argument carries a map")
            .syntax()
            .text(),
        "map { '_' -> 'a', 'a' => '_' }",
        "SYM_MAP opens at `map`, so `with` stays a token of the argument"
    );
    assert!(
        args[1].sym_map().is_none(),
        "the second argument carries no map"
    );
}

/// Every one of the seven interior nodes has an extent — its green-tree
/// text range — pinned here against a literal source position measured
/// by hand from this fixture. `syntax::kinds`'s module doc states a
/// stronger property, that this extent equals the AST span of the value
/// the node carries; this test pins only the GREEN side of that, by
/// value, and does not build the extracted `Program` to check the
/// other side (`write_and_move_vec_spans_are_pinned_by_value`, in
/// `syntax/extract.rs`'s own tests, pins the AST side directly, for
/// the two shapes — `WriteVec`/`MoveVec` — nothing else in the crate
/// reads). The measured extents are still the reason `SYM_MAP` opens
/// at `map` rather than at `with` and `WRITE_VEC` at `[` rather than
/// at `write`.
///
/// Worth a test rather than prose because extraction leans on it
/// directly: a bracket placed one token off still round-trips (the
/// lossless law sees the same bytes) and still casts (the kind is
/// right), so nothing else in this file would notice.
///
/// Every span below is a LITERAL, captured from the C1 lowering of this
/// fixture while that path was still callable. That is what makes the
/// check independent: reading the expected span back off the extracted
/// `Program` instead would move both sides together, since extraction
/// derives a node's span by reparsing that node's own tokens — a node
/// opening one token early would shift the span with it and pass.
/// `TRANSITION` is checked on three variants, since each carries its own
/// span field.
#[test]
fn each_new_nodes_extent_matches_its_measured_literal_span() {
    use mtc_core::diagnostics::Span;
    use mtc_core::syntax::TextLineIndex;
    use mtc_turing_machine::parser::parse_green;

    let src = "alphabet x { '0', '1' }\n\
               routine r(volatile tape a: x writes { '0' } preserves { '1' }, state done) {\n\
               \x20 entry state s { [*] -> return; }\n\
               }\n\
               machine {\n\
               \x20 tape m: x;\n\
               \x20 bind r(a = m with map { '0' -> '1', '1' => '0' }, done = stop) as bb;\n\
               \x20 entry state s {\n\
               \x20   ['0'] -> write ['1'] move [>] goto s;\n\
               \x20   [*] -> stop;\n\
               \x20 }\n\
               }\n";
    let index = TextLineIndex::new(src);
    let root = SyntaxNode::new_root(parse_green(src).expect("parses"));

    // The green node's extent, expressed the way an AST `Span` is.
    let extent = |n: &SyntaxNode| {
        let r = n.text_range();
        let (sl, sc) = index.line_col(r.start);
        let (el, ec) = index.line_col(r.end);
        Span::new(sl, sc, el, ec)
    };

    #[track_caller]
    fn check(
        nodes: &[SyntaxNode],
        expected: &[Span],
        extent: &dyn Fn(&SyntaxNode) -> Span,
        what: &str,
    ) {
        assert_eq!(nodes.len(), expected.len(), "{what}: node count");
        for (i, (node, span)) in nodes.iter().zip(expected).enumerate() {
            assert_eq!(extent(node), *span, "{what}[{i}] extent != its AST span");
        }
    }

    check(
        &all_of(&root, TmcKind::SigParam),
        &[Span::new(2, 11, 2, 62), Span::new(2, 64, 2, 74)],
        &extent,
        "SIG_PARAM",
    );
    check(
        &all_of(&root, TmcKind::ContractClause),
        &[Span::new(2, 30, 2, 44), Span::new(2, 45, 2, 62)],
        &extent,
        "CONTRACT_CLAUSE — `writes` then `preserves`",
    );
    // The `write`/`move` keywords stay OUTSIDE their vectors; the spans
    // open at the `[`.
    check(
        &all_of(&root, TmcKind::WriteVec),
        &[Span::new(9, 20, 9, 25)],
        &extent,
        "WRITE_VEC",
    );
    check(
        &all_of(&root, TmcKind::MoveVec),
        &[Span::new(9, 31, 9, 34)],
        &extent,
        "MOVE_VEC",
    );
    // Three variants: the routine's `return`, the machine's `goto s`,
    // and its `stop`.
    check(
        &all_of(&root, TmcKind::Transition),
        &[
            Span::new(3, 26, 3, 32),
            Span::new(9, 35, 9, 41),
            Span::new(10, 12, 10, 16),
        ],
        &extent,
        "TRANSITION",
    );
    check(
        &all_of(&root, TmcKind::BindingArg),
        &[Span::new(7, 10, 7, 51), Span::new(7, 53, 7, 64)],
        &extent,
        "BINDING_ARG",
    );
    // `with` stays outside the map, so the span opens at `map`.
    check(
        &all_of(&root, TmcKind::SymMap),
        &[Span::new(7, 21, 7, 51)],
        &extent,
        "SYM_MAP",
    );
}

// -- body accessors -----------------------------------------------------

/// `is_entry` reads the `entry` marker; a world has exactly one.
#[test]
fn states_expose_entry_and_their_rules() {
    let root = tree(
        "machine {\n  tape main: ab;\n\
         \x20 entry state s {\n    ['a'] -> write ['_'] move [>] goto t;\n\
         \x20   [*] -> stop;\n  }\n\
         \x20 state t { [*] -> halt; }\n}\n",
    );
    let w = MachineView::cast(first_of(&root, TmcKind::Machine))
        .expect("machine")
        .world()
        .expect("world");
    let states: Vec<StateView> = w.states().collect();
    assert_eq!(states.len(), 2);
    assert!(states[0].is_entry());
    assert!(!states[1].is_entry());
    assert_eq!(states[0].name_token().text(), "s");
    assert_eq!(states[0].rules().count(), 2);
    assert_eq!(states[1].rules().count(), 1);
}

/// A doc run's three direct-child shapes, which are NOT interchangeable:
/// `?` lines are DOC_LINE tokens; a `! [ident] …` line is folded by the
/// lexer into one ATTENTION_LINE token that ATTR then wraps; a bare-prose
/// `!` line carries no `[ident]`, so no ATTR is emitted and its token
/// stays a direct child. `attention_lines` means the bare ones — an
/// implementation returning every ATTENTION_LINE, tagged ones included,
/// must fail this test, which is why the fixture carries one of each.
///
/// ```text
/// ROOT@0..75
///   ALPHABET@0..74
///     DOC_RUN@0..54
///       DOC_LINE@0..5 "? one"
///       WHITESPACE@5..6 "\n"
///       DOC_LINE@6..11 "? two"
///       WHITESPACE@11..12 "\n"
///       ATTENTION_LINE@12..25 "! plain prose"
///       WHITESPACE@25..26 "\n"
///       ATTR@26..54
///         ATTENTION_LINE@26..54 "! [deprecated] use the other"
///     WHITESPACE@54..55 "\n"
///     IDENT@55..63 "alphabet"
///     WHITESPACE@63..64 " "
///     IDENT@64..66 "ab"
///     WHITESPACE@66..67 " "
///     L_BRACE@67..68 "{"
///     WHITESPACE@68..69 " "
///     GLYPH@69..72 "'_'"
///     WHITESPACE@72..73 " "
///     R_BRACE@73..74 "}"
///   WHITESPACE@74..75 "\n"
/// ```
#[test]
fn doc_runs_split_their_lines_and_expose_attributes() {
    let root =
        tree("? one\n? two\n! plain prose\n! [deprecated] use the other\nalphabet ab { '_' }\n");
    let run = DocRunView::cast(first_of(&root, TmcKind::DocRun)).expect("run");
    assert_eq!(run.doc_lines().len(), 2);
    assert_eq!(
        run.attention_lines().len(),
        1,
        "only the bare-prose line — the tagged one lives inside an ATTR"
    );
    let attrs: Vec<AttrView> = run.attrs().collect();
    assert_eq!(attrs.len(), 1);
    assert!(
        attrs[0].line_token().text().contains("[deprecated]"),
        "ATTR wraps the whole attention line, payload and all"
    );
}

/// A graft names its target and its instance; `as_name` is absent only
/// on an entry graft, which may omit it.
#[test]
fn grafts_expose_target_and_instance_name() {
    let root = tree(
        "graph g(tape t: ab, state done) {\n  entry state s { [*] -> done; }\n}\n\n\
         machine {\n  tape main: ab;\n\
         \x20 entry graft g(t = main, done = stop) as gg;\n}\n",
    );
    let g = GraftView::cast(first_of(&root, TmcKind::Graft)).expect("graft");
    assert!(g.is_entry());
    assert_eq!(g.target_token().text(), "g");
    assert_eq!(
        g.as_name().map(|t| t.text().to_string()),
        Some("gg".to_string())
    );
    assert_eq!(g.bindings().count(), 2);
}

/// `StateView::doc_run` is the state's own retro-wrapped first child,
/// distinct from a doc run bound to the enclosing machine — mirrors
/// `a_machines_doc_run_is_the_run_it_retro_wraps`.
#[test]
fn a_states_doc_run_is_the_run_it_retro_wraps() {
    let on_machine =
        tree("? doc\nmachine {\n  tape main: ab;\n  entry state s { [*] -> stop; }\n}\n");
    let s = StateView::cast(first_of(&on_machine, TmcKind::State)).expect("state");
    assert!(
        s.doc_run().is_none(),
        "the run belongs to the machine, not the state"
    );

    let on_state =
        tree("machine {\n  tape main: ab;\n  ? doc\n  entry state s { [*] -> stop; }\n}\n");
    let s = StateView::cast(first_of(&on_state, TmcKind::State)).expect("state");
    assert!(
        s.doc_run().is_some(),
        "the run is the state's own retro-wrapped first child"
    );
}

/// `BindView` mirrors `GraftView` without the `entry`/`is_entry` axis —
/// `bind` never carries an `entry` prefix. A non-argless binding list
/// discriminates `bindings().count()` from a vacuous pass, and a target
/// distinct from its instance name catches a target/`as_name` mix-up.
#[test]
fn binds_expose_target_bindings_and_instance_name() {
    let root = tree(
        "machine {\n  tape main: ab;\n\
         \x20 bind r(t = main, done = stop) as hh;\n\
         \x20 entry state s { [*] -> stop; }\n}\n",
    );
    let b = BindView::cast(first_of(&root, TmcKind::Bind)).expect("bind");
    assert_eq!(b.target_token().text(), "r");
    assert_eq!(b.as_name().text(), "hh");
    let names: Vec<String> = b
        .bindings()
        .map(|a| a.name_token().text().to_string())
        .collect();
    assert_eq!(names, vec!["t".to_string(), "done".to_string()]);
}

/// The brief's own claim — "`as_name` is absent only on an entry
/// graft" — is unexercised by `grafts_expose_target_and_instance_name`
/// alone, which never writes a graft missing it. An entry graft may
/// omit `as name` entirely; the parser accepts it (`GraftNeedsName` is
/// raised only for a NON-entry graft missing it).
#[test]
fn an_entry_graft_may_omit_its_instance_name() {
    let root = tree(
        "graph g(tape t: ab, state done) {\n  entry state s { [*] -> done; }\n}\n\n\
         machine {\n  tape main: ab;\n\
         \x20 entry graft g(t = main, done = stop);\n}\n",
    );
    let g = GraftView::cast(first_of(&root, TmcKind::Graft)).expect("graft");
    assert!(g.is_entry());
    assert!(g.as_name().is_none(), "no `as` was written");
}

/// Every other `GraftView` fixture in this file writes `entry graft`.
/// The one other non-entry graft in the whole corpus
/// (`world_states_grafts_and_binds_are_kind_filtered_and_ordered`, task
/// 3b) is reached only through `.syntax().text()`, never through a
/// typed accessor — so nothing here ever drove `is_entry()`'s FALSE
/// branch, and nothing pinned `target_token()`'s index on the shape
/// where `entry` is absent from the header run entirely (measured via
/// `debug_dump`: a non-entry GRAFT's own IDENTs are exactly
/// `["graft", "g"]`, two elements, not three — the header never grows
/// a phantom `entry` slot to compensate). This test walks a plain,
/// non-entry graft through the whole `GraftView` surface.
#[test]
fn a_non_entry_grafts_whole_surface_is_read_correctly() {
    let root = tree(
        "graph g(tape t: ab, state done) {\n  entry state s { [*] -> done; }\n}\n\n\
         machine {\n  tape main: ab;\n\
         \x20 entry state s { [*] -> stop; }\n\
         \x20 graft g(t = main, done = stop) as gg;\n}\n",
    );
    let w = MachineView::cast(first_of(&root, TmcKind::Machine))
        .expect("machine")
        .world()
        .expect("world");
    let g = w.grafts().next().expect("one graft");
    assert!(!g.is_entry(), "no `entry` was written on this graft");
    assert_eq!(g.target_token().text(), "g");
    assert_eq!(
        g.as_name().map(|t| t.text().to_string()),
        Some("gg".to_string())
    );
    assert_eq!(g.bindings().count(), 2);
}

/// `target_token` answers only a qualified target's FIRST segment —
/// stated already in its own doc, never measured before now. `ns::g`
/// lexes as two IDENTs joined by `COLON_COLON`; the accessor reads the
/// one right after the `graft` keyword, which is `ns`, not `g`. Task 5
/// rebuilds the full qualified name straight from the tree rather than
/// from this accessor, so a silent change here would surface only as
/// an oracle failure much later — pinned now instead.
#[test]
fn a_qualified_grafts_target_token_is_only_the_first_segment() {
    let root = tree(
        "machine {\n  tape main: ab;\n\
         \x20 entry graft ns::g(t = main, done = stop) as gg;\n}\n",
    );
    let g = GraftView::cast(first_of(&root, TmcKind::Graft)).expect("graft");
    assert_eq!(
        g.target_token().text(),
        "ns",
        "target_token answers only the qualified path's first segment, never the whole path"
    );
}

/// `pattern_tokens` stops at `->`, not at the pattern's own `]` — a
/// rule that continues past the arrow with `write`/`move`/a transition
/// is the fixture that discriminates a correct Arrow-terminated scan
/// from one that keeps going and picks up `write`/the write vector's
/// own tokens/`;`.
#[test]
fn a_rules_pattern_tokens_stop_at_the_arrow() {
    let root = tree(
        "machine {\n  tape main: ab;\n\
         \x20 entry state s {\n    ['a'] -> write ['_'] move [>] goto t;\n  }\n\
         \x20 state t { [*] -> halt; }\n}\n",
    );
    let rule = RuleView::cast(first_of(&root, TmcKind::Rule)).expect("rule");
    let tokens: Vec<String> = rule
        .pattern_tokens()
        .iter()
        .map(|t| t.text().to_string())
        .collect();
    assert_eq!(
        tokens,
        vec!["[".to_string(), "'a'".to_string(), "]".to_string()],
        "pattern_tokens must stop at `->`, not run into write/move/the transition"
    );
}

/// `write_vec`/`move_vec` are told apart by the child NODE's own kind,
/// never by bracket-group position — the second rule carries a move
/// vector and no write vector, so a positional "first bracket group
/// after `->`" implementation would misread its move vector as a write.
#[test]
fn a_rules_write_and_move_accessors_are_told_apart_by_kind() {
    let root = tree(
        "alphabet x { '0', '1' }\nmachine {\n  tape m: x;\n\
         \x20 entry state s {\n    ['0'] -> write ['1'] move [>] goto s;\n\
         \x20   ['1'] -> move [<] goto s;\n  }\n}\n",
    );
    let w = MachineView::cast(first_of(&root, TmcKind::Machine))
        .expect("machine")
        .world()
        .expect("world");
    let s = w.states().next().expect("one state");
    let rules: Vec<RuleView> = s.rules().collect();
    assert_eq!(rules.len(), 2);

    assert_eq!(
        rules[0].write_vec().map(|v| v.syntax().text().to_string()),
        Some("['1']".to_string())
    );
    assert_eq!(
        rules[0].move_vec().map(|v| v.syntax().text().to_string()),
        Some("[>]".to_string())
    );

    assert!(
        rules[1].write_vec().is_none(),
        "the second rule writes nothing"
    );
    assert_eq!(
        rules[1].move_vec().map(|v| v.syntax().text().to_string()),
        Some("[<]".to_string()),
        "its lone bracket group after `->` is a MOVE vector"
    );
}

/// `transition()` returning `None` IS `Transition::Stay` — a rule may
/// omit its transition only once it already carries an action, so by
/// the time a RULE node exists at all, `None` here can only mean
/// "stay in the current state", never a missing/erroneous transition.
/// Both a `write`-only rule and a `debugger`-only rule omit theirs;
/// the third and fourth rules pin the `Some` side against each other so
/// a "first `TRANSITION` in the whole tree" bug (rather than THIS
/// rule's own child) cannot pass by accident.
#[test]
fn an_omitted_transition_accessor_answers_none() {
    let root = tree(
        "alphabet x { '0', '1' }\nmachine {\n  tape m: x;\n\
         \x20 entry state s {\n    ['0'] -> write ['1'];\n\
         \x20   ['1'] -> debugger;\n\
         \x20   [*] -> move [.] goto s;\n\
         \x20   [*] -> stop;\n  }\n}\n",
    );
    let w = MachineView::cast(first_of(&root, TmcKind::Machine))
        .expect("machine")
        .world()
        .expect("world");
    let s = w.states().next().expect("one state");
    let rules: Vec<RuleView> = s.rules().collect();
    assert_eq!(rules.len(), 4);

    assert!(
        rules[0].transition().is_none(),
        "a write-only rule omits its transition — Stay"
    );
    assert!(
        rules[1].transition().is_none(),
        "a debugger-only rule omits its transition — Stay"
    );
    assert_eq!(
        rules[2].transition().map(|t| t.syntax().text().to_string()),
        Some("goto s".to_string())
    );
    assert_eq!(
        rules[3].transition().map(|t| t.syntax().text().to_string()),
        Some("stop".to_string())
    );
}
