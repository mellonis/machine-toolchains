//! `contract-clause-overlap`: a signature tape parameter's `writes` clause
//! names a glyph its `preserves` clause also names. The checker's effective
//! allowed set is `writes MINUS preserves` — a symbol in both is cancelled
//! before it ever reaches the inferred-footprint comparison, so the `writes`
//! entry naming it contributes nothing: it is inert.
//!
//! Purely syntactic-plus-resolution: unlike most of the checker's own work,
//! deciding overlap needs no footprint inference at all. It compares the two
//! DECLARED symbol sets the resolver already built (`ResolvedTape::writes` /
//! `::preserves`), so this rule stays cheap regardless of how large a world's
//! body is, and reads exactly the sets the checker itself subtracts — it never
//! re-resolves a clause's glyphs on its own.
//!
//! # One finding per source element
//!
//! The finding is per SOURCE ELEMENT of the `writes` clause, not per glyph. A
//! `writes` range straddling the overlap only partially — some of its glyphs
//! are also in `preserves`, some are not — still gets exactly one finding,
//! naming just the glyphs that overlap, and ships no fix: splitting a range
//! into "the part that stays" and "the part that goes" is not a
//! whitespace-safe single text edit. An element every one of whose glyphs
//! overlaps (a single symbol, or a range entirely swallowed by `preserves`)
//! gets the removal fix.
//!
//! # The fix, and the one case it changes shape
//!
//! Removing an element takes its adjacent comma with it — the comma AFTER it
//! for every element but the last, the comma BEFORE it (so the remaining list
//! still parses) when it is last.
//!
//! Doing that to the clause's ONLY element would leave `writes {}` behind —
//! and `writes {}` is a first-class, far more restrictive declaration
//! (explicitly "write nothing") than the vacuous clause it would be replacing
//! (a clause whose one listed symbol contributed nothing, because `preserves`
//! already cancelled it). So when the overlapping element is the clause's
//! only one, the fix removes the whole clause instead, leaving no `writes`
//! restriction at all rather than minting a new, stronger one nobody wrote.
//! That widening is a real semantics change (unlike an ordinary element
//! removal, which is inert by construction — the removed entry was already
//! excluded by the `preserves` subtraction), so the whole-clause fix is
//! `MaybeIncorrect`, the same tier every other whole-declaration deletion in
//! this crate uses (`unused-alphabet` and its siblings); the ordinary
//! element-removal fix stays `MachineApplicable`.
//!
//! # Comments inside the deletion span
//!
//! The analysis this rule reads is comment-free, but the SOURCE is not: a
//! `writes {'0', /* … */ '1'}` can carry a comment between two elements.
//! Silently deleting it would be the same defect fmt takes care to avoid (it
//! relocates a clause-interior comment rather than dropping it), so before
//! offering either fix this rule checks the comment-INCLUSIVE token stream
//! for one landing inside the edit's own span and withholds the fix if it
//! finds one — the finding still reports, same posture as a partial-range
//! overlap.

use mtc_core::diagnostics::{Applicability, Diagnostic, Edit, Fix, Span};

use crate::compiler::full_name;
use crate::footprint::SymSet;
use crate::lexer::{Token, TokenKind};
use crate::lint::LintContext;
use crate::lint::patterns::{glyph_label, range_labels};
use crate::parser::{AlphabetElem, ContractClause, Program, SigParam, SigParamKind};

/// One alphabet-body element's own source span — a clause body uses the same
/// element grammar as an alphabet body.
fn elem_span(elem: &AlphabetElem) -> Span {
    match elem {
        AlphabetElem::Single(s) => s.span(),
        AlphabetElem::Range { span, .. } => *span,
    }
}

/// `elem`'s own indices in the tape's alphabet frame: one for a single
/// symbol, one per expanded glyph for a range. `None` only on a shape
/// resolution would already have rejected (an unresolvable range, a glyph
/// absent from the alphabet) — unreachable past a clean analysis, but the
/// rule stays silent here rather than panicking on it.
fn elem_indices(elem: &AlphabetElem, glyphs: &[String]) -> Option<Vec<u32>> {
    let labels = match elem {
        AlphabetElem::Single(s) => vec![glyph_label(s)],
        AlphabetElem::Range { lo, hi, .. } => range_labels(lo, hi)?,
    };
    labels
        .into_iter()
        .map(|l| glyphs.iter().position(|g| *g == l).map(|ix| ix as u32))
        .collect()
}

/// `indices`' glyphs, ascending by index — the compiler's own
/// offending-glyph rendering order (`WritesOutsideContract`).
fn ascending_glyphs(glyphs: &[String], indices: &[u32]) -> Vec<String> {
    let mut sorted = indices.to_vec();
    sorted.sort_unstable();
    sorted
        .into_iter()
        .map(|ix| glyphs[ix as usize].clone())
        .collect()
}

/// A glyph list rendered the way `WritesOutsideContract` renders its
/// offending set: each glyph single-quoted, comma-space joined.
fn quoted_list(glyphs: &[String]) -> String {
    glyphs
        .iter()
        .map(|g| format!("'{g}'"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// The AST signature parameters of the routine or graph resolved as
/// `world_name`. A contract clause's source-level elements and the whole
/// clause's own span live only on the AST — the resolved module keeps the
/// resolved SET (`ResolvedTape::writes`), not where each element sits in the
/// source. `None` for a machine world: a machine tape declaration carries no
/// contract grammar at all, so its tapes' `writes`/`preserves` are always
/// `None` and never reach this lookup.
fn world_sig<'a>(program: &'a Program, world_name: &str) -> Option<&'a [SigParam]> {
    if let Some(r) = program
        .routines
        .iter()
        .find(|r| full_name(&r.ns, &r.name) == world_name)
    {
        return Some(&r.sig.params);
    }
    program
        .graphs
        .iter()
        .find(|g| full_name(&g.ns, &g.name) == world_name)
        .map(|g| g.sig.params.as_slice())
}

/// The `writes` clause AST node of the tape parameter named `tape_name`, if
/// it declared one.
fn writes_clause_for<'a>(params: &'a [SigParam], tape_name: &str) -> Option<&'a ContractClause> {
    let p = params.iter().find(|p| p.name == tape_name)?;
    match &p.kind {
        SigParamKind::Tape { writes, .. } => writes.as_ref(),
        SigParamKind::State => None,
    }
}

/// The removal fix for a fully-covered `writes` element at index `i` of
/// `clause`, naming the glyph(s) it removes. The clause's ONLY element takes
/// the whole clause with it (module head) — a semantics-widening edit, so
/// `MaybeIncorrect`; any other element takes just itself and its adjacent
/// comma — the comma AFTER it, unless it is the last element, in which case
/// the comma BEFORE it (so the remaining list still parses) — an inert edit,
/// so `MachineApplicable`.
fn removal_fix(clause: &ContractClause, i: usize, named: &str) -> Fix {
    let n = clause.elems.len();
    if n == 1 {
        return Fix {
            description: "remove the emptied `writes` clause".to_string(),
            applicability: Applicability::MaybeIncorrect,
            edits: vec![Edit {
                span: clause.span,
                replacement: String::new(),
            }],
        };
    }
    let span = if i == n - 1 {
        Span {
            start: elem_span(&clause.elems[i - 1]).end,
            end: elem_span(&clause.elems[i]).end,
        }
    } else {
        Span {
            start: elem_span(&clause.elems[i]).start,
            end: elem_span(&clause.elems[i + 1]).start,
        }
    };
    Fix {
        description: format!("remove {named} from the `writes` clause"),
        applicability: Applicability::MachineApplicable,
        edits: vec![Edit {
            span,
            replacement: String::new(),
        }],
    }
}

/// Whether `a` and `b` share any source position — the half-open-range
/// overlap test, over [`Span`]'s derived `Ord`.
fn spans_overlap(a: Span, b: Span) -> bool {
    a.start < b.end && b.start < a.end
}

/// Whether any comment token (from the comment-INCLUSIVE stream — the
/// ordinary `ctx.tokens` never carries one) lands inside `span`. A candidate
/// fix whose deletion span answers yes here must not be offered — deleting it
/// would silently take the comment with it (module head).
fn span_touches_a_comment(comment_tokens: &[Token], span: Span) -> bool {
    comment_tokens
        .iter()
        .any(|t| matches!(t.kind, TokenKind::Comment(_)) && spans_overlap(t.span(), span))
}

pub(crate) fn check(ctx: &LintContext, out: &mut Vec<Diagnostic>) {
    for world in &ctx.resolved.worlds {
        let Some(params) = world_sig(ctx.program, &world.name) else {
            continue;
        };
        for tape in &world.tapes {
            // Both clauses must be DECLARED for an overlap to be possible at
            // all — an absent clause contributes nothing to intersect with.
            let (Some(writes_set), Some(preserves_set)) = (tape.writes, tape.preserves) else {
                continue;
            };
            let overlap = writes_set.intersect(preserves_set);
            if overlap == SymSet::empty() {
                continue;
            }
            let Some(clause) = writes_clause_for(params, &tape.name) else {
                continue;
            };
            let Some(glyphs) = crate::lint::alphabet_glyphs(ctx.resolved, &tape.alphabet) else {
                continue;
            };
            for (i, elem) in clause.elems.iter().enumerate() {
                let Some(indices) = elem_indices(elem, glyphs) else {
                    continue;
                };
                let overlapping: Vec<u32> = indices
                    .iter()
                    .copied()
                    .filter(|ix| overlap.contains(*ix))
                    .collect();
                if overlapping.is_empty() {
                    continue;
                }
                let fully_covered = overlapping.len() == indices.len();
                let named = quoted_list(&ascending_glyphs(glyphs, &overlapping));
                let fix = fully_covered
                    .then(|| removal_fix(clause, i, &named))
                    .filter(|f| {
                        !f.edits
                            .iter()
                            .any(|e| span_touches_a_comment(ctx.comment_tokens, e.span))
                    });
                out.push(Diagnostic {
                    code: "contract-clause-overlap",
                    span: elem_span(elem),
                    message: format!(
                        "{named} is in both `writes` and `preserves`; `preserves` wins, so the `writes` entry is inert"
                    ),
                    fix,
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use mtc_core::diagnostics::{Applicability, Diagnostic, Edit, Pos};

    use crate::compiler::{CompileOptions, compile};
    use crate::lint::{LintOptions, lint};

    fn findings(src: &str) -> Vec<Diagnostic> {
        lint(src, LintOptions::default())
            .unwrap()
            .diagnostics
            .into_iter()
            .filter(|d| d.code == "contract-clause-overlap")
            .collect()
    }

    /// (line, col) → byte offset, char-counted. Shared by `apply` and the
    /// span-slicing test below.
    fn byte_of(src: &str, pos: Pos) -> usize {
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

    /// Apply a fix's edits to `src`, applied descending so earlier offsets
    /// stay valid. Mirrors `lint_programs.rs`'s `apply_fix` — no shared
    /// test-support module in this crate, so each file keeps its own copy.
    fn apply(src: &str, edits: &[Edit]) -> String {
        let mut ranges: Vec<(usize, usize, String)> = edits
            .iter()
            .map(|e| {
                (
                    byte_of(src, e.span.start),
                    byte_of(src, e.span.end),
                    e.replacement.clone(),
                )
            })
            .collect();
        ranges.sort_by(|a, b| b.0.cmp(&a.0));
        let mut out = src.to_string();
        for (s, e, rep) in ranges {
            out.replace_range(s..e, &rep);
        }
        out
    }

    #[test]
    fn a_single_overlapping_glyph_fires_at_the_writes_element_span() {
        let src = "\
alphabet bits { '_', '0', '1' }
routine mark(tape t: bits writes {'0', '1'} preserves {'1'}) {
  entry state s { [*] -> write ['0'] return; }
}
";
        let f = findings(src);
        assert_eq!(f.len(), 1, "{f:?}");
        assert_eq!(
            f[0].message,
            "'1' is in both `writes` and `preserves`; `preserves` wins, so the `writes` entry is inert"
        );
        // The span names exactly the writes-clause's `'1'` element, not the
        // preserves one — slicing it out of the source proves it, and the
        // span must sit BEFORE the `preserves` keyword to prove it's the
        // `writes`-side `'1'`, not the (textually identical) `preserves` one.
        let start = byte_of(src, f[0].span.start);
        let end = byte_of(src, f[0].span.end);
        assert_eq!(&src[start..end], "'1'");
        assert!(
            !src[..start].contains("preserves"),
            "the span sits inside `writes`, before `preserves` even starts"
        );

        // `'1'` is the LAST of the two writes elements, so the fix must take
        // the comma BEFORE it (the "leading comma if last" branch) rather
        // than one after — there is no element after it to borrow from.
        let fix = f[0]
            .fix
            .clone()
            .expect("a single glyph is always fully covered");
        let fixed = apply(src, &fix.edits);
        assert!(
            fixed.contains("writes {'0'} preserves {'1'}"),
            "the trailing element and its LEADING comma are removed:\n{fixed}"
        );
        assert!(
            findings(&fixed).is_empty(),
            "re-lint is clean: {:?}",
            findings(&fixed)
        );
    }

    #[test]
    fn disjoint_clauses_are_quiet() {
        let src = "\
alphabet bits { '_', '0', '1' }
routine mark(tape t: bits writes {'0'} preserves {'1'}) {
  entry state s { [*] -> write ['0'] return; }
}
";
        assert!(findings(src).is_empty(), "{:?}", findings(src));
    }

    #[test]
    fn a_partially_overlapping_range_names_only_the_overlap_and_ships_no_fix() {
        let src = "\
alphabet bits { '_', '0', '1', '2' }
routine mark(tape t: bits writes {'0'..'2'} preserves {'1'}) {
  entry state s { [*] -> write ['0'] return; }
}
";
        let f = findings(src);
        assert_eq!(f.len(), 1, "{f:?}");
        assert_eq!(
            f[0].message,
            "'1' is in both `writes` and `preserves`; `preserves` wins, so the `writes` entry is inert"
        );
        assert!(
            f[0].fix.is_none(),
            "a partial range overlap ships no fix — splitting a range isn't a single edit"
        );
    }

    #[test]
    fn the_fix_on_a_middle_element_removes_it_with_its_comma_and_recompiles_identically() {
        let src = "\
alphabet bits { '_', '0', '1', '2' }
routine mark(tape t: bits writes {'0', '1', '2'} preserves {'1'}) {
  entry state s { [*] -> write ['0'] return; }
}
";
        let f = findings(src);
        assert_eq!(f.len(), 1, "{f:?}");
        let fix = f[0]
            .fix
            .clone()
            .expect("a single glyph is always fully covered");
        assert_eq!(fix.description, "remove '1' from the `writes` clause");
        // Inert (the removed entry was already excluded by the `preserves`
        // subtraction), unlike the only-element whole-clause fix — pins the
        // applicability split between the two branches.
        assert_eq!(fix.applicability, Applicability::MachineApplicable);

        let fixed = apply(src, &fix.edits);
        assert!(
            fixed.contains("writes {'0', '2'} preserves {'1'}"),
            "element and its comma removed cleanly:\n{fixed}"
        );

        assert!(
            findings(&fixed).is_empty(),
            "re-lint is clean: {:?}",
            findings(&fixed)
        );

        // Contracts are IR-inert (checked, never emitted): the redundant
        // entry was already ignored by the effective-set subtraction, so
        // dropping it moves nothing at codegen — that IS the redundancy
        // claim this rule makes.
        let before = compile(src, CompileOptions::default()).expect("the source compiles");
        let after = compile(&fixed, CompileOptions::default()).expect("the fixed source compiles");
        assert_eq!(before.tma, after.tma, "the emitted assembly must not move");
        assert_eq!(
            before.object.to_bytes(),
            after.object.to_bytes(),
            "removing an inert writes entry must be object-neutral"
        );
    }

    #[test]
    fn the_fix_on_the_only_element_removes_the_whole_clause_and_recompiles_identically() {
        let src = "\
alphabet bits { '_', '1' }
routine mark(tape t: bits writes {'1'} preserves {'1'}) {
  entry state s { [*] -> return; }
}
";
        let f = findings(src);
        assert_eq!(f.len(), 1, "{f:?}");
        let fix = f[0]
            .fix
            .clone()
            .expect("a single glyph is always fully covered");
        assert_eq!(fix.description, "remove the emptied `writes` clause");
        // Widens the declared contract (writes-nothing → full-minus-preserves)
        // — a semantics change, unlike plain element removal — so this tier
        // matches every other whole-declaration deletion fix in the crate.
        assert_eq!(fix.applicability, Applicability::MaybeIncorrect);

        let fixed = apply(src, &fix.edits);
        assert!(
            !fixed.contains("writes"),
            "the whole clause is gone:\n{fixed}"
        );
        assert!(
            fixed.contains("preserves {'1'}"),
            "the preserves clause survives untouched:\n{fixed}"
        );

        assert!(
            findings(&fixed).is_empty(),
            "re-lint is clean: {:?}",
            findings(&fixed)
        );

        let before = compile(src, CompileOptions::default()).expect("the source compiles");
        let after = compile(&fixed, CompileOptions::default()).expect("the fixed source compiles");
        assert_eq!(before.tma, after.tma, "the emitted assembly must not move");
        assert_eq!(
            before.object.to_bytes(),
            after.object.to_bytes(),
            "removing the vacuous writes clause must be object-neutral"
        );
    }

    #[test]
    fn a_fully_covered_range_element_gets_the_removal_fix() {
        // The range `'0'..'1'` is entirely swallowed by `preserves` — every
        // glyph it names overlaps, so (unlike the partial-overlap case) it
        // gets the fix, naming both glyphs it removes.
        let src = "\
alphabet bits { '_', '0', '1', '2' }
routine mark(tape t: bits writes {'0'..'1', '2'} preserves {'0'..'1'}) {
  entry state s { [*] -> write ['2'] return; }
}
";
        let f = findings(src);
        assert_eq!(f.len(), 1, "{f:?}");
        assert_eq!(
            f[0].message,
            "'0', '1' is in both `writes` and `preserves`; `preserves` wins, so the `writes` entry is inert"
        );
        let fix = f[0]
            .fix
            .clone()
            .expect("a fully-covered range gets the removal fix too");
        assert_eq!(fix.description, "remove '0', '1' from the `writes` clause");

        let fixed = apply(src, &fix.edits);
        assert!(
            fixed.contains("writes {'2'} preserves {'0'..'1'}"),
            "the range element and its trailing comma are removed:\n{fixed}"
        );
        assert!(
            findings(&fixed).is_empty(),
            "re-lint is clean: {:?}",
            findings(&fixed)
        );

        let before = compile(src, CompileOptions::default()).expect("the source compiles");
        let after = compile(&fixed, CompileOptions::default()).expect("the fixed source compiles");
        assert_eq!(
            before.object.to_bytes(),
            after.object.to_bytes(),
            "removing an inert range element must be object-neutral"
        );
    }

    #[test]
    fn a_contract_on_a_graph_param_is_checked_too() {
        // A graph is an inferred world like any other — its signature tapes
        // take contracts, and this rule's AST lookup must resolve BOTH kinds
        // (`world_sig`'s graph arm, exercised by no other test here).
        let src = "\
alphabet bits { '_', '0', '1' }
graph g(tape t: bits writes {'0', '1'} preserves {'1'}, state done) {
  entry state s { ['0'] -> done; [*] -> write ['0'] goto s; }
}
";
        let f = findings(src);
        assert_eq!(f.len(), 1, "{f:?}");
        assert_eq!(
            f[0].message,
            "'1' is in both `writes` and `preserves`; `preserves` wins, so the `writes` entry is inert"
        );
    }

    #[test]
    fn a_namespaced_routines_overlap_is_found_under_its_mangled_name() {
        // `world.name` is the world's MANGLED name (`lib::mark`), never the
        // bare declared one — a namespaced world is the only shape where the
        // two differ, so it is the only shape that can catch `world_sig`'s
        // keying drifting to the bare `r.name` (mirrors the compiler's own
        // guard for `check_contracts`, `a_contract_in_a_namespace_is_checked_
        // under_its_mangled_name`).
        let src = "\
namespace lib {
  alphabet bits { '_', '0', '1' }
  routine mark(tape t: bits writes {'0', '1'} preserves {'1'}) {
    entry state s { [*] -> write ['0'] return; }
  }
}
";
        let f = findings(src);
        assert_eq!(f.len(), 1, "{f:?}");
        assert_eq!(
            f[0].message,
            "'1' is in both `writes` and `preserves`; `preserves` wins, so the `writes` entry is inert"
        );
        let start = byte_of(src, f[0].span.start);
        let end = byte_of(src, f[0].span.end);
        assert_eq!(&src[start..end], "'1'");
    }

    #[test]
    fn a_comment_inside_the_deletion_span_withholds_the_fix() {
        // `'1'` is the last (and only overlapping) element, so its removal
        // span would otherwise run from the end of `'0'` through the end of
        // `'1'` — exactly the range the interior comment sits in. The finding
        // still reports; only the fix is withheld, the same posture as a
        // partial-range overlap.
        let src = "\
alphabet bits { '_', '0', '1' }
routine mark(tape t: bits writes {'0', /* keep me */ '1'} preserves {'1'}) {
  entry state s { [*] -> write ['0'] return; }
}
";
        let f = findings(src);
        assert_eq!(f.len(), 1, "{f:?}");
        assert_eq!(
            f[0].message,
            "'1' is in both `writes` and `preserves`; `preserves` wins, so the `writes` entry is inert"
        );
        assert!(
            f[0].fix.is_none(),
            "a fix spanning a comment must be withheld"
        );
    }
}
