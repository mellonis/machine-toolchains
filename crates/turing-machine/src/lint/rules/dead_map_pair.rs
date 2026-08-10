//! `dead-map-pair`: a bidirectional (`->`) symbol-map pair whose write-back
//! half can never fire, because the callee never writes the callee-side glyph
//! the pair names.
//!
//! A two-way pair says two things at once: read the host glyph `src` as the
//! callee glyph `dst`, and write a callee `dst` back as a host `src`
//! (docs/formats.md (bound calls)). The write half is the half the write
//! footprint can decide: if the callee's body — and everything it in turn
//! reuses — provably never writes `dst`, that half is dead surface, and the
//! pair means exactly what the one-way `src => dst` spelling means.
//!
//! The read half is NOT decided here and never fires this rule: whether a host
//! `src` ever reaches the callee depends on the caller's own writes and on the
//! tape's initial content, neither of which is a compile-time fact. One-way
//! pairs are therefore never reported — they carry no write half to be dead.
//!
//! Soundness rests on the footprint being an OVER-approximation: a glyph
//! outside the inferred set provably never lands, while a glyph inside it
//! merely may. So `dst ∉ set` is a proof, and every uncertainty the inference
//! meets (a callee outside the compilation unit, a `{expr}` write cell, an
//! unresolvable glyph) widens the set and silences this rule rather than
//! risking a false positive.
//!
//! # The fix, and the one case it is withheld
//!
//! The remedy is DEMOTION — rewrite the pair's `->` as `=>` — never deletion:
//! deleting the pair would take the live READ half with it. Demotion drops one
//! write-map entry that nothing consults, so the run is unchanged.
//!
//! Whether the program is still ACCEPTED is a second question, and the answer
//! is not the same everywhere. Across differently sized alphabets a demoted
//! pair's callee glyph merely becomes a write hole, which — being never
//! written — no row ever crosses. Across EQUAL-sized alphabets the two-way
//! pairs must identity-complete to a bijection (docs/formats.md (bound
//! calls)), and a map satisfying that is a permutation: dropping any entry
//! that is not a fixed point leaves the identity fill colliding with the
//! unique entry that used to produce that image. That requirement holds for
//! every site kind — a graft meets it while splicing, a bound call or bind
//! when its composite is built — so demotion is not merely a graft-side
//! hazard. The fix is therefore offered only where it provably cannot change
//! acceptance — unequal cardinalities, or a pair whose two glyphs sit at the
//! same index — and the finding still reports without one otherwise.

use std::collections::HashMap;

use mtc_core::diagnostics::{Applicability, Diagnostic, Edit, Fix, Span};

use crate::compiler::{ResolvedCallTarget, ResolvedWorld};
use crate::footprint::{self, FootprintTable};
use crate::lexer::{Token, TokenKind};
use crate::lint::LintContext;
use crate::lint::patterns::glyph_label;
use crate::parser::{BindingArg, BindingValue, MapArrow, MapPair};

/// The `->` token inside one map pair — the span the demote edit replaces.
/// Recovered from the comment-free token stream: the arrow's position survives
/// in no other artifact (`MapPair` keeps the two literals' spans and the pair's
/// own, and reduces the arrow to a [`MapArrow`] discriminant). Nothing but the
/// arrow can sit between the two literals, so the search is exact rather than
/// positional. `None` if the token shape is unexpected — the finding then
/// ships without a fix.
fn arrow_span(tokens: &[Token], pair: &MapPair) -> Option<Span> {
    let after_src = pair.src.span().end;
    let before_dst = pair.dst.span().start;
    tokens
        .iter()
        .find(|t| {
            matches!(t.kind, TokenKind::Arrow)
                && after_src <= t.span().start
                && t.span().end <= before_dst
        })
        .map(|t| t.span())
}

/// The position of `glyph` in a glyph vector.
fn index_of(glyphs: &[String], glyph: &str) -> Option<u32> {
    glyphs.iter().position(|g| g == glyph).map(|i| i as u32)
}

/// Whether demoting a pair provably cannot change whether the program is
/// ACCEPTED — the precondition for offering the fix at all (module head).
///
/// Unequal cardinalities: no bijection requirement applies at all, and the
/// demoted glyph becomes a write hole nothing crosses. Equal cardinalities:
/// safe only for a fixed point, where the identity fill reproduces exactly the
/// entry that was dropped. The test is deliberately kind-agnostic — the
/// bijection requirement is the same for a graft, a bound call and a bind. A
/// host side that does not resolve leaves the question undecidable, so the fix
/// is withheld.
fn demotion_preserves_acceptance(
    host_glyphs: Option<&[String]>,
    callee_glyphs: &[String],
    src: &str,
    dst_index: u32,
) -> bool {
    let Some(host_glyphs) = host_glyphs else {
        return false;
    };
    if host_glyphs.len() != callee_glyphs.len() {
        return true;
    }
    index_of(host_glyphs, src) == Some(dst_index)
}

/// Flag every dead write-back half in one binding site's args.
fn check_binding(
    ctx: &LintContext,
    footprints: &FootprintTable,
    host: &ResolvedWorld,
    callee: &ResolvedWorld,
    args: &[BindingArg],
    out: &mut Vec<Diagnostic>,
) {
    // A callee the inference did not table (it walks every resolved world, so
    // this is a shape that should not arise) decides nothing.
    let Some(footprint) = footprints.worlds.get(&callee.name) else {
        return;
    };
    for arg in args {
        let BindingValue::Named {
            target,
            map: Some(map),
            ..
        } = &arg.value
        else {
            continue;
        };
        // `arg.name` names the CALLEE parameter. One that is not a tape is a
        // state continuation, which carries no symbol map to judge.
        let Some((k, callee_tape)) = callee
            .tapes
            .iter()
            .enumerate()
            .find(|(_, t)| t.name == arg.name)
        else {
            continue;
        };
        // The callee's own frame is the frame `dst` is written in, and the
        // frame the footprint is inferred in — never the host's.
        let (Some(callee_glyphs), Some(written)) = (
            crate::lint::alphabet_glyphs(ctx.resolved, &callee_tape.alphabet),
            footprint.tapes.get(k),
        ) else {
            continue;
        };
        // Only the fix guard reads the host side; the verdict itself is a
        // property of the callee alone, so an unresolvable host tape or
        // alphabet costs the fix, never the finding.
        let host_glyphs = host
            .tapes
            .iter()
            .find(|t| t.name == *target)
            .and_then(|t| crate::lint::alphabet_glyphs(ctx.resolved, &t.alphabet));

        for pair in &map.pairs {
            // A one-way pair has no write half to be dead (module head).
            if pair.arrow != MapArrow::Bidirectional {
                continue;
            }
            let dst = glyph_label(&pair.dst);
            // A `dst` outside the callee's alphabet cannot be looked up, and
            // is a fatal of its own further down the pipeline.
            let Some(dst_index) = index_of(callee_glyphs, &dst) else {
                continue;
            };
            if written.contains(dst_index) {
                continue;
            }
            let src = glyph_label(&pair.src);
            let fix = demotion_preserves_acceptance(host_glyphs, callee_glyphs, &src, dst_index)
                .then(|| arrow_span(ctx.tokens, pair))
                .flatten()
                .map(|span| Fix {
                    description: format!("demote to a one-way pair (`'{src}' => '{dst}'`)"),
                    applicability: Applicability::MachineApplicable,
                    edits: vec![Edit {
                        span,
                        replacement: "=>".to_string(),
                    }],
                });
            out.push(Diagnostic {
                code: "dead-map-pair",
                span: pair.span,
                message: format!(
                    "the write-back half of `'{src}' -> '{dst}'` never fires: `{}` never writes '{dst}'",
                    callee.name
                ),
                fix,
            });
        }
    }
}

pub(crate) fn check(ctx: &LintContext, out: &mut Vec<Diagnostic>) {
    // One whole-module inference per lint run. `run_rules` offers no
    // cross-rule cache, and the walk is a monotone fixpoint over a table with
    // one small bit-set per tape, so recomputing it here costs far less than
    // threading a cache through every rule for one consumer.
    let footprints = footprint::infer_resolved(ctx.resolved);
    let by_name: HashMap<&str, &ResolvedWorld> = ctx
        .resolved
        .worlds
        .iter()
        .map(|w| (w.name.as_str(), w))
        .collect();
    let callee = |name: &str| by_name.get(name).copied();

    for host in &ctx.resolved.worlds {
        for call in &host.calls {
            // A call on a bind name carries no args of its own — the binding
            // lives on the declaration, which the `binds` loop below scans
            // ONCE however many call sites share it.
            if let ResolvedCallTarget::Routine {
                name,
                external: false,
                args,
            } = &call.target
                && let Some(c) = callee(name)
            {
                check_binding(ctx, &footprints, host, c, args, out);
            }
        }
        // A graft's target is always a locally defined graph.
        for graft in &host.grafts {
            if let Some(c) = callee(&graft.target) {
                check_binding(ctx, &footprints, host, c, &graft.args, out);
            }
        }
        for bind in &host.binds {
            if !bind.external
                && let Some(c) = callee(&bind.target)
            {
                check_binding(ctx, &footprints, host, c, &bind.args, out);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use mtc_core::diagnostics::{Applicability, Diagnostic, Edit, Pos};

    use crate::compiler::{CompileErrorKind, CompileOptions, compile};
    use crate::lint::{LintOptions, lint};

    fn findings(src: &str) -> Vec<Diagnostic> {
        lint(src, LintOptions::default())
            .unwrap()
            .diagnostics
            .into_iter()
            .filter(|d| d.code == "dead-map-pair")
            .collect()
    }

    fn messages(src: &str) -> Vec<String> {
        findings(src).into_iter().map(|d| d.message).collect()
    }

    /// Apply one fix's edits (char positions → byte offsets, descending).
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
        ranges.sort_by(|a, b| b.0.cmp(&a.0));
        let mut out = src.to_string();
        for (s, e, rep) in ranges {
            out.replace_range(s..e, rep);
        }
        out
    }

    /// The stdlib's real shape — markers collapsed one-way onto the callee's
    /// blank, digits carried two-way — over a graph that writes only `'0'`,
    /// so the `'1' -> '1'` pair's write-back half is dead and the `'0' -> '0'`
    /// one is live. Cardinalities differ (5 vs 3), which is what makes the
    /// demotion offerable here.
    const GRAFT_SRC: &str = "\
alphabet host5 { '_', '^', '$', '0', '1' }
alphabet bare3 { '_', '0', '1' }

graph zeroing(tape v: bare3, state done) {
  entry state s {
    ['1'] -> write ['0'] move [>] goto s;
    [*] -> done;
  }
}

machine {
  tape t: host5;
  entry graft zeroing(v = t with map { '^' => '_', '$' => '_', '0' -> '0', '1' -> '1' }, done = fin) as z;
  state fin { [*] -> stop; }
}
";

    #[test]
    fn a_dead_bidirectional_pair_is_reported_on_a_graft() {
        let f = findings(GRAFT_SRC);
        assert_eq!(f.len(), 1, "{f:?}");
        // The message format is quoted verbatim in the published rule
        // reference, so it is pinned whole rather than by keyword.
        assert_eq!(
            f[0].message,
            "the write-back half of `'1' -> '1'` never fires: `zeroing` never writes '1'"
        );
        // The squiggle covers the pair, matching the sibling map-pair rules.
        assert_eq!(f[0].span.start.line, 13);
    }

    #[test]
    fn a_live_pair_is_not_reported() {
        // The same site over a graph that writes BOTH digits: neither
        // write-back half is dead.
        let src = GRAFT_SRC.replace(
            "    [*] -> done;",
            "    ['0'] -> write ['1'] move [>] goto s;\n    [*] -> done;",
        );
        assert!(messages(&src).is_empty(), "{:?}", messages(&src));
    }

    #[test]
    fn a_one_way_pair_is_never_reported() {
        // `'1' => '1'` names the very same unwritten callee glyph; with no
        // write half there is nothing to be dead.
        let src = GRAFT_SRC.replace("'1' -> '1'", "'1' => '1'");
        assert!(messages(&src).is_empty(), "{:?}", messages(&src));
    }

    #[test]
    fn a_call_and_a_bind_are_scanned_too() {
        // One direct call and one bind instance CALLED TWICE. The bind's
        // pairs live on the declaration, so the dead one is a single finding
        // on the `bind` line however many call sites share it.
        let src = "\
alphabet host5 { '_', '^', '$', '0', '1' }
alphabet bare3 { '_', '0', '1' }

routine zeroOut(tape v: bare3) {
  entry state s {
    ['1'] -> write ['0'] move [>] goto s;
    [*] -> return;
  }
}

machine {
  tape t: host5;
  bind zeroOut(v = t with map { '^' => '_', '0' -> '0', '1' -> '1' }) as z;
  entry state go { [*] -> call zeroOut(v = t with map { '^' => '_', '0' -> '0', '1' -> '1' }) then two; }
  state two { [*] -> call z() then three; }
  state three { [*] -> call z() then stop; }
}
";
        let f = findings(src);
        let lines: Vec<u32> = f.iter().map(|d| d.span.start.line).collect();
        assert_eq!(lines, vec![13, 14], "{f:?}");
        assert!(
            f.iter()
                .all(|d| d.message.contains("`zeroOut` never writes '1'")),
            "{f:?}"
        );
    }

    #[test]
    fn an_unresolvable_target_is_silent() {
        // Neither callee's body is in this unit, so nothing about what they
        // write is known — the rule says nothing rather than guessing.
        let src = "\
alphabet host5 { '_', '^', '$', '0', '1' }

machine {
  tape t: host5;
  bind other::wipe(v = t with map { '0' -> '1' }) as w;
  entry state go { [*] -> call other::helper(v = t with map { '0' -> '1' }) then two; }
  state two { [*] -> call w() then stop; }
}
";
        assert!(messages(src).is_empty(), "{:?}", messages(src));
    }

    #[test]
    fn a_bind_on_the_machine_world_name_is_silent() {
        // `main` is the MACHINE world's mangled name. An out-of-unit bind that
        // happens to be spelled `main` resolves as external, yet the name
        // still hits the world table — so a lookup unguarded by the external
        // flag would hand this site the machine itself as its callee and judge
        // the pair against the machine's own footprint. The bind's parameter
        // name is deliberately a real machine tape, so nothing but the
        // external guard stands between this source and a false finding.
        let src = "\
alphabet host5 { '_', '^', '$', '0', '1' }

machine {
  tape t: host5;
  bind main(t = t with map { '0' -> '1' }) as b;
  entry state go { [*] -> write ['0'] stop; }
}
";
        assert!(messages(src).is_empty(), "{:?}", messages(src));
    }

    #[test]
    fn the_fix_demotes_the_arrow_only() {
        let f = findings(GRAFT_SRC);
        let fix = f[0].fix.clone().expect("a demote fix");
        assert_eq!(fix.description, "demote to a one-way pair (`'1' => '1'`)");
        assert_eq!(fix.applicability, Applicability::MachineApplicable);
        assert_eq!(fix.edits.len(), 1);
        assert_eq!(fix.edits[0].replacement, "=>");

        let fixed = apply(GRAFT_SRC, &fix.edits);
        assert!(fixed.contains("'0' -> '0', '1' => '1'"), "{fixed}");
        assert!(!fixed.contains("'1' -> '1'"), "{fixed}");
        assert!(messages(&fixed).is_empty(), "{:?}", messages(&fixed));
        compile(&fixed, CompileOptions::default()).expect("the demoted source compiles");
    }

    #[test]
    fn an_equal_cardinality_permutation_reports_without_a_fix() {
        // Equal-sized alphabets make the two-way pairs a permutation that must
        // stay a bijection. `'x' -> 'b'` is dead (the graph writes only `'a'`),
        // but demoting it would leave `'y' -> 'a'` and an identity fill both
        // producing `'a'`, so the finding ships without the fix. A graft is
        // rejected while splicing, as below; the same demotion on a bound call
        // or bind is rejected when its composite is built instead — the
        // requirement is one, only the stage it bites at differs.
        let src = "\
alphabet hostA { '_', 'x', 'y' }
alphabet grA { '_', 'a', 'b' }

graph gg(tape v: grA, state done) {
  entry state s { [*] -> write ['a'] done; }
}

machine {
  tape t: hostA;
  entry graft gg(v = t with map { 'x' -> 'b', 'y' -> 'a' }, done = fin) as z;
  state fin { [*] -> stop; }
}
";
        let f = findings(src);
        assert_eq!(f.len(), 1, "{f:?}");
        assert!(f[0].message.contains("`'x' -> 'b'`"), "{f:?}");
        assert!(f[0].fix.is_none(), "{f:?}");

        // The reason, pinned: as written it compiles, demoted it does not.
        compile(src, CompileOptions::default()).expect("the fixture compiles as written");
        let demoted = src.replace("'x' -> 'b'", "'x' => 'b'");
        let err = compile(&demoted, CompileOptions::default())
            .expect_err("demoting a permutation entry breaks the bijection");
        assert!(
            matches!(err.kind, CompileErrorKind::MapNotInjective { .. }),
            "{:?}",
            err.kind
        );
    }

    #[test]
    fn the_stdlib_has_no_dead_map_pairs() {
        // The corpus checkpoint. The one map in `std.tmc` collapses its
        // markers one-way and carries the digits two-way, and bare invert
        // writes both digits — every write-back half there is live.
        let f = findings(crate::stdlib::SOURCE);
        assert!(f.is_empty(), "{f:?}");
    }
}
