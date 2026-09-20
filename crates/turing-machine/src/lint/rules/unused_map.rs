//! `unused-map`: a `map` declaration nothing names by `with map NAME`.
//! Export-independent, mirroring `unused-alphabet`: a named map has no
//! cross-module consumer to protect either — an exported-but-unused map is
//! as dead as a private one, since a name is only ever reached by a
//! binding-arg site (`docs/tmt/language.md` (named maps)) naming it. New on
//! the lint channel, detected source-level over `Resolved`.
//!
//! Usage is decided by RESOLVED identity, matching `unused-alphabet`'s own
//! rule: `compiler::expand_named_maps` rewrites a resolved site's own
//! `SymMap::named` text to the declaration's mangled `ResolvedMapDecl::name`
//! (a `Resolved`-only rewrite — the two SOURCE printers read `Program`,
//! untouched by it, and keep printing what the author wrote), so a site's
//! `named` here IS the mangled name it resolved to, never the raw written
//! spelling. Matching on that avoids the trap a bare SHORT-name compare
//! would fall into: two different namespaces may each declare a `map m`,
//! and a use of one must never silence the other.
//!
//! The fix deletes the whole declaration, including any leading doc/
//! attention run — an orphaned `?`/`!` run is a parse error, so the doc
//! goes with the map it documents, mirroring every sibling `unused-*` rule
//! in this crate.

use std::collections::HashSet;

use mtc_core::diagnostics::{Applicability, Diagnostic, Edit, Fix};

use crate::lint::LintContext;
use crate::lint::rules::spans::decl_span;
use crate::parser::BindingValue;
use crate::syntax::MapDeclView;

/// This binding value's own named-map reference, by its RESOLVED mangled
/// name (`compiler::expand_named_maps` already rewrote it), if any.
fn named_map_ref(value: &BindingValue) -> Option<&str> {
    let BindingValue::Named { map: Some(m), .. } = value else {
        return None;
    };
    let (name, _) = m.named.as_ref()?;
    Some(name.as_str())
}

pub(crate) fn check(ctx: &LintContext, out: &mut Vec<Diagnostic>) {
    // Every mangled name a `with map NAME` site resolved to, across every
    // graft, bind, and call in the module.
    let mut used: HashSet<&str> = HashSet::new();
    for world in &ctx.resolved.worlds {
        for graft in &world.grafts {
            used.extend(graft.args.iter().filter_map(|a| named_map_ref(&a.value)));
        }
        for bind in &world.binds {
            used.extend(bind.args.iter().filter_map(|a| named_map_ref(&a.value)));
        }
        for call in &world.calls {
            if let crate::compiler::ResolvedCallTarget::Routine { args, .. } = &call.target {
                used.extend(args.iter().filter_map(|a| named_map_ref(&a.value)));
            }
        }
    }

    for (name, map) in &ctx.resolved.maps {
        if !used.contains(name.as_str()) {
            let fix = decl_span::<MapDeclView>(ctx, map.name_span).map(|span| Fix {
                description: format!("delete the unused map `{name}`"),
                applicability: Applicability::MaybeIncorrect,
                edits: vec![Edit {
                    span,
                    replacement: String::new(),
                }],
            });
            out.push(Diagnostic {
                code: "unused-map",
                span: map.name_span,
                message: format!("map `{name}` is never used by any binding"),
                fix,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use mtc_core::diagnostics::{Applicability, Edit, Pos};

    use crate::lint::{LintOptions, lint};

    fn findings(src: &str) -> Vec<String> {
        lint(src, LintOptions::default())
            .unwrap()
            .diagnostics
            .into_iter()
            .filter(|d| d.code == "unused-map")
            .map(|d| d.message)
            .collect()
    }

    const SRC: &str = "\
alphabet wide { '_', '^', '$', '0', '1' }
alphabet bits { '_', '0', '1' }
map wideToBits: wide -> bits { '^' => '_', '$' => '_', '0' -> '0', '1' -> '1' }

routine plusOne(tape num: bits) {
  entry state s { [*] -> return; }
}

machine {
  tape data: wide;
  entry state go { [*] -> call plusOne(num = data with map wideToBits) then stop; }
}
";

    /// Mutation: counting a declaration as used by its own declaration
    /// site (e.g. matching on `map.name_span` itself, or on the
    /// declaration's own `src`/`dst` alphabet references) rather than by a
    /// genuine `with map NAME` use — dropping the one real call site above
    /// would then still report nothing, when it should fire.
    #[test]
    fn unused_map_fires_on_an_unused_declaration() {
        let src = SRC.replace(
            "call plusOne(num = data with map wideToBits) then stop;",
            "stop;",
        );
        let f = findings(&src);
        assert_eq!(f.len(), 1, "{f:?}");
        assert!(f[0].contains("wideToBits"), "{f:?}");
    }

    #[test]
    fn and_not_on_a_used_one() {
        assert!(findings(SRC).is_empty(), "{:?}", findings(SRC));
    }

    /// Two namespaces each declare a map with the SAME short name (`m`);
    /// only `used::m` is ever named at a site. Mutation: matching by short
    /// name (`rsplit("::")`) instead of the resolved mangled name — that
    /// would see ANY `with map m` as covering BOTH declarations, so
    /// `dead::m` would never be reported.
    #[test]
    fn a_namespaced_declaration_is_reported_even_when_a_same_short_named_one_elsewhere_is_used() {
        let src = "\
alphabet a2 { '_', 'x' }
alphabet b2 { '_', 'y' }
namespace used {
  map m: a2 -> b2 { 'x' -> 'y' }
}
namespace dead {
  map m: a2 -> b2 { 'x' -> 'y' }
}
routine plusOne(tape num: b2) {
  entry state s { [*] -> return; }
}
machine {
  tape data: a2;
  entry state go { [*] -> call plusOne(num = data with map used::m) then stop; }
}
";
        let f = findings(src);
        assert_eq!(f.len(), 1, "{f:?}");
        assert!(f[0].contains("dead::m"), "{f:?}");
        assert!(!f[0].contains("used::m"), "{f:?}");
    }

    /// A `use`-imported bare name still counts as a use of the declaration
    /// it resolves to, not of anything else sharing its short name.
    #[test]
    fn a_use_imported_name_counts_for_the_right_declaration() {
        let src = "\
alphabet a2 { '_', 'x' }
alphabet b2 { '_', 'y' }
namespace used {
  export map m: a2 -> b2 { 'x' -> 'y' }
}
namespace dead {
  map m: a2 -> b2 { 'x' -> 'y' }
}
use used::m;
routine plusOne(tape num: b2) {
  entry state s { [*] -> return; }
}
machine {
  tape data: a2;
  entry state go { [*] -> call plusOne(num = data with map m) then stop; }
}
";
        let f = findings(src);
        assert_eq!(f.len(), 1, "{f:?}");
        assert!(f[0].contains("dead::m"), "{f:?}");
    }

    // The comment-guard withhold test lives alongside its siblings in
    // `tests/lint_fix_comment_guard.rs`
    // (`unused_map_withholds_the_fix_when_the_declaration_holds_a_comment`),
    // matching `unused-alphabet`'s own placement there rather than this
    // file — `run_rules`' shared guard is exercised once per rule, in one
    // roster, not duplicated per rule module.

    /// Apply one fix's edits (char positions → byte offsets, descending) —
    /// mirrors `dead_map_pair`'s own test helper.
    fn apply(src: &str, edits: &[Edit]) -> String {
        fn byte_offset(src: &str, pos: Pos) -> usize {
            let (mut line, mut col) = (1u32, 1u32);
            for (i, c) in src.char_indices() {
                if line == pos.line && col == pos.col {
                    return i;
                }
                if c == '\n' {
                    line += 1;
                    col = 1;
                } else {
                    col += 1;
                }
            }
            src.len()
        }
        let mut ranges: Vec<(usize, usize, &str)> = edits
            .iter()
            .map(|e| {
                (
                    byte_offset(src, e.span.start),
                    byte_offset(src, e.span.end),
                    e.replacement.as_str(),
                )
            })
            .collect();
        ranges.sort_by_key(|r| std::cmp::Reverse(r.0));
        let mut out = src.to_string();
        for (s, e, rep) in ranges {
            out.replace_range(s..e, rep);
        }
        out
    }

    #[test]
    fn the_fix_deletes_the_whole_declaration() {
        let src = SRC.replace(
            "call plusOne(num = data with map wideToBits) then stop;",
            "stop;",
        );
        let report = lint(&src, LintOptions::default()).unwrap();
        let found = report
            .diagnostics
            .iter()
            .find(|d| d.code == "unused-map")
            .expect("fires");
        let fix = found.fix.clone().expect("a delete fix");
        assert_eq!(fix.applicability, Applicability::MaybeIncorrect);
        let fixed = apply(&src, &fix.edits);
        assert!(!fixed.contains("wideToBits"), "{fixed}");
        crate::compiler::compile(&fixed, crate::compiler::CompileOptions::default())
            .expect("the source compiles with the unused map deleted");
    }
}
