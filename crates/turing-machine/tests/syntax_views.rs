//! Typed views over the `.tmc` green tree. These tests pin the view
//! CONTRACT — which node a view accepts and which it refuses — and each
//! accessor's answer for the shapes that do and do not have the child it
//! names. Extraction parity is a separate file; a view that returns the
//! wrong child still round-trips, so losslessness cannot catch it.

use mtc_core::syntax::{AstNode, SyntaxNode};
use mtc_turing_machine::parser::parse_green;
use mtc_turing_machine::syntax::{AlphabetView, MachineView, RootView, TmcKind};

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
/// sampled: a view added later without a cast test is the gap this
/// catches. The list is the node half of `TmcKind`.
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
    for (kind, casts) in checks {
        let node = if kind == TmcKind::Root {
            root.clone()
        } else {
            first_of(&root, kind)
        };
        assert!(casts(node), "no view casts from {kind:?}");
    }
}
