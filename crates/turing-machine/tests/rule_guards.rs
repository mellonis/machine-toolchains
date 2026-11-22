//! Rule guards (#133's `where { … }`): the typing and refusal surface —
//! cone-local contexts, one common sequence per order comparison, and the
//! four refusals, each pinned by kind AND span
//! (docs/tmt/language.md (rule guards), docs/internal/specs/…design.md).

use mtc_core::diagnostics::Span;
use mtc_turing_machine::compiler::{CompileError, CompileErrorKind, CompileOptions, compile};

fn parse_ok(src: &str) {
    compile(src, CompileOptions::default()).expect("the fixture must compile");
}

fn refusal(src: &str) -> CompileError {
    compile(src, CompileOptions::default())
        .expect_err("the fixture must be refused")
}

/// The 1-based `Pos` of a needle's first character — the parser battery's
/// own helper, copied for one column basis across test files.
fn at(src: &str, needle: &str) -> mtc_core::diagnostics::Pos {
    for (i, line) in src.lines().enumerate() {
        if let Some(col) = line.find(needle) {
            return mtc_core::diagnostics::Pos {
                line: i as u32 + 1,
                col: col as u32 + 1,
            };
        }
    }
    panic!("`{needle}` not in fixture");
}

/// The half-open span covering the first occurrence of `needle`.
fn cover(src: &str, needle: &str) -> Span {
    let p = at(src, needle);
    Span::new(p.line, p.col, p.line, p.col + needle.len() as u32)
}

const HEAD: &str = "\
alphabet ab { 'a', 'b', 'c' }
set pair { 'a', 'b' }
set trio { 'a', 'b', 'c' }
set foreign { 'q', 'r' }

machine {
  tape t: ab;
  tape u: ab;
  entry state s {
";

#[test]
fn one_sided_narrowing_names_both_contexts() {
    let src = format!("{HEAD}    ['a'..'c' as x, 'a'..'c' as y] where {{ x in pair && x < y }} -> stop;\n  }}\n}}\n");
    let e = refusal(&src);
    assert_eq!(
        e.kind,
        CompileErrorKind::GuardOrderContext {
            left: "pair".into(),
            right: "ab".into(),
        }
    );
    // The refusal spans the COMPARISON, not the whole guard.
    assert_eq!(e.span, cover(&src, "x < y"));
    // …and its message names both contexts.
    let msg = e.kind.to_string();
    assert!(msg.contains("pair") && msg.contains("ab"), "message: {msg}");
}

#[test]
fn a_second_narrowing_agrees_only_on_identity() {
    // `x` narrowed to two different sets: identity needs no sequence and
    // stays legal…
    parse_ok(&format!(
        "{HEAD}    ['a'..'c' as x, 'a'..'c' as y] where {{ x in pair && x in trio && x == y }} -> stop;\n  }}\n}}\n"
    ));
    // …but a position comparison has no single sequence to live in.
    let src = format!(
        "{HEAD}    ['a'..'c' as x, 'a'..'c' as y] where {{ x in pair && x in trio && x < y }} -> stop;\n  }}\n}}\n"
    );
    let e = refusal(&src);
    assert_eq!(
        e.kind,
        CompileErrorKind::GuardOrderContext {
            left: "pair".into(),
            right: "trio".into(),
        }
    );
}

#[test]
fn an_unknown_guard_name_is_refused() {
    let src = format!(
        "{HEAD}    ['a'..'c' as x, 'a'..'c' as y] where {{ zz == y }} -> stop;\n  }}\n}}\n"
    );
    let e = refusal(&src);
    assert_eq!(
        e.kind,
        CompileErrorKind::GuardUnknownBinding { name: "zz".into() }
    );
    // Span at the name itself.
    assert_eq!(e.span, cover(&src, "zz"));
}

#[test]
fn an_in_off_alphabet_is_refused() {
    let src = format!(
        "{HEAD}    ['a'..'c' as x, 'a'..'c' as y] where {{ x in foreign && x == y }} -> stop;\n  }}\n}}\n"
    );
    let e = refusal(&src);
    assert_eq!(
        e.kind,
        CompileErrorKind::GuardSetOutsideAlphabet {
            set: "foreign".into(),
            alphabet: "ab".into(),
        }
    );
    // The span is the set name AS USED in the guard (HEAD itself declares
    // the set, so the needle anchors past `x in `).
    let p = at(&src, "x in foreign");
    assert_eq!(
        e.span,
        Span::new(p.line, p.col + 5, p.line, p.col + 5 + "foreign".len() as u32)
    );
}

#[test]
fn a_comparison_with_no_binding_is_refused() {
    let src = format!(
        "{HEAD}    ['a'..'c' as x, 'a'..'c' as y] where {{ 'q' == 'r' }} -> stop;\n  }}\n}}\n"
    );
    let e = refusal(&src);
    assert_eq!(e.kind, CompileErrorKind::GuardConstantComparison);
    assert_eq!(e.span, cover(&src, "'q' == 'r'"));
}

#[test]
fn a_comparison_types_by_its_own_cone() {
    // The narrowed leg compares in `pair`'s order; the bare leg (a sibling
    // `||` branch, outside the narrowing's cone) is licensed by the shared
    // alphabet instead. Both legs legal, one span of one guard.
    parse_ok(&format!(
        "{HEAD}    ['a'..'c' as x, 'a'..'c' as y] where {{ (x in pair && y in pair && x < y) || x < y }} -> stop;\n  }}\n}}\n"
    ));
}
