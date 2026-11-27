//! `enters { … }` / `leaves { … }` on a signature tape parameter
//! (docs/tmt/cli.md (compile errors)): the two head-position clauses take
//! the same clause grammar `writes`/`never writes` already have — the same
//! alphabet-body element list, the same membership check against the
//! parameter's own alphabet, and the same canonical-order and duplicate
//! checks — extended to the pair `writes` < `never writes` < `enters` <
//! `leaves`. Unlike `writes {}`/`never writes {}` (a legal, meaningful empty
//! SET), an empty `enters {}`/`leaves {}` has no meaning — there is no
//! symbol-less moment for the head to be at — so it is its own error,
//! `empty-head-clause`.
//!
//! This file checks the grammar and the AST-level clause checks
//! (canonical order, duplicates, the empty-clause rejection) only; the
//! object/header carriage and the static `enters`/`leaves` compile
//! checks each have their own test files.

use mtc_core::syntax::SyntaxNode;
use mtc_turing_machine::CompileErrorKind;
use mtc_turing_machine::compiler::{CompileOptions, compile};
use mtc_turing_machine::parser::{SigParamKind, parse_green};
use mtc_turing_machine::syntax::extract_program;

/// A source carrying both new clauses on the same tape parameter, alongside
/// an already-legal empty `writes {}` on it — the near-miss `writes {}`
/// stays accepted, proving "empty clause" and "empty set" are not
/// conflated.
const BOTH_CLAUSES: &str = "alphabet sym { '_', '^', '$', '0', '1' }

routine walk(tape num: sym writes {} enters { '^', '0', '1', '$' } leaves { '$' }) {
  entry state go {
    ['$'] -> return;
    [*]   -> move [>] goto go;
  }
}

machine {
  tape t: sym;
  entry state s { [*] -> call walk(num = t) then done; }
  state done { [*] -> stop; }
}
";

#[test]
fn both_head_clauses_round_trip_losslessly() {
    let tree = parse_green(BOTH_CLAUSES).expect("parses");
    let root = SyntaxNode::new_root(tree);
    assert_eq!(root.text(), BOTH_CLAUSES, "lossless law");
}

#[test]
fn both_head_clauses_extract_into_the_ast() {
    let tree = parse_green(BOTH_CLAUSES).expect("parses");
    let root = SyntaxNode::new_root(tree);
    let program = extract_program(&root, BOTH_CLAUSES);
    let r = &program.routines[0];
    let SigParamKind::Tape {
        writes,
        enters,
        leaves,
        ..
    } = &r.sig.params[0].kind
    else {
        panic!("expected a tape parameter");
    };
    let writes = writes.as_ref().expect("a `writes` clause");
    assert!(writes.elems.is_empty(), "writes {{}} means writes nothing");
    let enters = enters.as_ref().expect("an `enters` clause");
    assert_eq!(enters.elems.len(), 4);
    let leaves = leaves.as_ref().expect("a `leaves` clause");
    assert_eq!(leaves.elems.len(), 1);
}

#[test]
fn both_head_clauses_compile_cleanly() {
    compile(BOTH_CLAUSES, CompileOptions::default())
        .unwrap_or_else(|e| panic!("expected a clean compile: {e}"));
}

/// A minimal routine/machine pair wrapping one tape parameter's clause
/// text, so each fixture below differs only in what it appends after
/// `sym`.
fn wrap(clauses: &str) -> String {
    format!(
        "alphabet sym {{ '_', '^', '$', '0', '1' }}\n\n\
         routine walk(tape num: sym{clauses}) {{\n  \
         entry state go {{\n    ['$'] -> return;\n    [*]   -> move [>] goto go;\n  }}\n}}\n\n\
         machine {{\n  \
         tape t: sym;\n  \
         entry state s {{ [*] -> call walk(num = t) then done; }}\n  \
         state done {{ [*] -> stop; }}\n\
         }}\n"
    )
}

fn err(src: &str) -> mtc_turing_machine::CompileError {
    compile(src, CompileOptions::default()).expect_err("expected a compile error")
}

fn code(src: &str) -> &'static str {
    err(src).kind.code()
}

/// Mutation this catches: skipping the membership check for the two new
/// clause kinds — the existing `writes`/`never writes` fixtures stay green
/// under that mutation, so only this test moves.
#[test]
fn a_clause_glyph_outside_the_alphabet_is_rejected() {
    let src = wrap(" enters { 'z' }");
    assert_eq!(code(&src), "contract-symbol-unknown");
}

/// Mutation this catches: accepting an empty `enters {}` as `Some(vec![])`.
#[test]
fn an_empty_enters_clause_is_rejected() {
    let src = wrap(" enters {}");
    assert_eq!(code(&src), "empty-head-clause");
}

/// Mutation this catches: the same acceptance bug on `leaves` — a separate
/// guard, since the two arms are written separately.
#[test]
fn an_empty_leaves_clause_is_rejected() {
    let src = wrap(" leaves {}");
    assert_eq!(code(&src), "empty-head-clause");
}

/// Near miss: `writes {}` must stay accepted — conflating "empty clause"
/// with "empty set" would break `writes {}`'s established meaning ("writes
/// nothing").
#[test]
fn an_empty_writes_clause_still_means_writes_nothing() {
    let src = wrap(" writes {}");
    compile(&src, CompileOptions::default())
        .unwrap_or_else(|e| panic!("`writes {{}}` must stay accepted: {e}"));
}

/// Mutation this catches: ordering the new arms with no order check at
/// all — `leaves` written before `enters`.
#[test]
fn leaves_before_enters_is_a_clause_order_error() {
    let src = wrap(" writes {} leaves { '$' } enters { '^' }");
    assert_eq!(code(&src), "contract-clause-order");
}

/// Mutation this catches: checking order only among the new pair and not
/// against the old pair — `never writes` written after `enters`.
#[test]
fn never_writes_after_enters_is_a_clause_order_error() {
    let src = wrap(" writes {} enters { '^' } never writes { '0' }");
    assert_eq!(code(&src), "contract-clause-order");
}

/// Mutation this catches: a single `seen` flag shared by both new
/// keywords.
#[test]
fn a_second_enters_clause_is_a_duplicate() {
    let src = wrap(" enters { '^' } enters { '0' }");
    let e = err(&src);
    assert_eq!(e.kind.code(), "duplicate-contract-clause");
    assert!(
        matches!(&e.kind, CompileErrorKind::DuplicateContractClause { what } if *what == "enters"),
        "{:?}",
        e.kind
    );
}

/// Mutation this catches: the shared-flag bug above, plus a `what` hard-
/// coded to one keyword.
#[test]
fn a_second_leaves_clause_is_a_duplicate() {
    let src = wrap(" leaves { '^' } leaves { '0' }");
    let e = err(&src);
    assert_eq!(e.kind.code(), "duplicate-contract-clause");
    assert!(
        matches!(&e.kind, CompileErrorKind::DuplicateContractClause { what } if *what == "leaves"),
        "{:?}",
        e.kind
    );
}
