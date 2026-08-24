//! Typed views over the `.tmc` green tree. These tests pin the view
//! CONTRACT — which node a view accepts and which it refuses — and each
//! accessor's answer for the shapes that do and do not have the child it
//! names. Extraction parity is a separate file; a view that returns the
//! wrong child still round-trips, so losslessness cannot catch it.

use mtc_core::syntax::{AstNode, SyntaxNode};
use mtc_turing_machine::parser::parse_green;
use mtc_turing_machine::syntax::{
    AlphabetView, MachineView, NamespaceView, RootView, TmcKind, UsePathView, UseView,
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

/// A namespace's own items, not the file's.
#[test]
fn namespace_items_are_scoped_to_it() {
    let root = tree("alphabet outer { '_' }\n\nnamespace n {\n  alphabet inner { '_' }\n}\n");
    let ns = NamespaceView::cast(first_of(&root, TmcKind::Namespace)).expect("ns");
    assert_eq!(ns.name(), "n");
    let names: Vec<String> = ns
        .items()
        .filter_map(|i| AlphabetView::cast(i.node().clone()))
        .map(|a| a.name_token().text().to_string())
        .collect();
    assert_eq!(
        names,
        vec!["inner"],
        "the outer alphabet is not this namespace's"
    );
}

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
