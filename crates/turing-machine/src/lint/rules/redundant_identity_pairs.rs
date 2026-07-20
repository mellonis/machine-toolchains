//! `redundant-identity-pairs`: a `with map { x -> x }` bidirectional pair that
//! an identity mapping would supply anyway.
//!
//! Soundness: identity completion is INDEX-based and applies only across
//! EQUAL-size alphabets (a closed, unequal-size map turns unlisted symbols into
//! holes, so there `x -> x` is load-bearing, not redundant — docs/formats.md
//! (bound calls)). Rather than reason about per-symbol indices, this rule fires
//! only when the caller tape and the bound callee tape draw from an IDENTICAL
//! alphabet (same glyphs, same order): then every glyph sits at the same index
//! on both sides, so a bidirectional `x -> x` is exactly what completion gives.
//! Any subtler equal-size-but-reordered case is left unflagged — the rule
//! under-reports rather than risk a false positive. Report-only.

use std::collections::HashMap;

use mtc_core::diagnostics::Diagnostic;

use crate::compiler::{ResolvedCallTarget, ResolvedWorld};
use crate::lint::LintContext;
use crate::lint::patterns::glyph_label;
use crate::parser::{BindingArg, BindingValue, MapArrow};

/// The glyph vector of `world`'s tape named `tape_name`, if it has one.
fn tape_glyphs<'a>(
    ctx: &'a LintContext,
    world: &ResolvedWorld,
    tape_name: &str,
) -> Option<&'a [String]> {
    let tape = world.tapes.iter().find(|t| t.name == tape_name)?;
    crate::lint::alphabet_glyphs(ctx.resolved, &tape.alphabet)
}

/// Flag each redundant identity pair in one binding whose caller tape and
/// bound callee tape share an identical alphabet.
fn check_binding(
    ctx: &LintContext,
    caller: &ResolvedWorld,
    callee: &ResolvedWorld,
    args: &[BindingArg],
    out: &mut Vec<Diagnostic>,
) {
    for arg in args {
        let BindingValue::Named {
            target,
            map: Some(map),
            ..
        } = &arg.value
        else {
            continue;
        };
        // The callee param `arg.name` and the caller tape `target` must draw
        // from the very same alphabet for an `x -> x` to be redundant.
        let (Some(graph), Some(host)) = (
            tape_glyphs(ctx, callee, &arg.name),
            tape_glyphs(ctx, caller, target),
        ) else {
            continue;
        };
        if host != graph {
            continue;
        }
        for pair in &map.pairs {
            if pair.arrow == MapArrow::Bidirectional
                && glyph_label(&pair.src) == glyph_label(&pair.dst)
            {
                let g = glyph_label(&pair.src);
                out.push(Diagnostic {
                    code: "redundant-identity-pairs",
                    span: pair.span,
                    message: format!(
                        "identity pair `{g} -> {g}` is redundant — an identity mapping already supplies it"
                    ),
                    fix: None,
                });
            }
        }
    }
}

pub(crate) fn check(ctx: &LintContext, out: &mut Vec<Diagnostic>) {
    let by_name: HashMap<&str, &ResolvedWorld> = ctx
        .resolved
        .worlds
        .iter()
        .map(|w| (w.name.as_str(), w))
        .collect();
    let callee = |name: &str| by_name.get(name).copied();

    for caller in &ctx.resolved.worlds {
        for call in &caller.calls {
            if let ResolvedCallTarget::Routine {
                name,
                external: false,
                args,
            } = &call.target
                && let Some(c) = callee(name)
            {
                check_binding(ctx, caller, c, args, out);
            }
        }
        for graft in &caller.grafts {
            if let Some(c) = callee(&graft.target) {
                check_binding(ctx, caller, c, &graft.args, out);
            }
        }
        for bind in &caller.binds {
            if !bind.external
                && let Some(c) = callee(&bind.target)
            {
                check_binding(ctx, caller, c, &bind.args, out);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::lint::{LintOptions, lint};

    fn findings(src: &str) -> Vec<String> {
        lint(src, LintOptions::default())
            .unwrap()
            .diagnostics
            .into_iter()
            .filter(|d| d.code == "redundant-identity-pairs")
            .map(|d| d.message)
            .collect()
    }

    #[test]
    fn an_identity_pair_over_one_shared_alphabet_fires() {
        // Caller tape `m` and callee param `t` both draw from `ab`, so
        // `'a' -> 'a'` is exactly what identity completion would give.
        let src = "\
alphabet ab { '_', 'a', 'b' }
routine echo(tape t: ab) { entry state s { [*] -> return; } }
machine {
  tape m: ab;
  entry state go { [*] -> call echo(t = m with map { 'a' -> 'a' }) then done; }
  state done { [*] -> stop; }
}
";
        let f = findings(src);
        assert_eq!(f.len(), 1, "{f:?}");
        assert!(f[0].contains("`a -> a`"), "{f:?}");
    }

    #[test]
    fn an_identity_pair_across_different_alphabets_is_quiet() {
        // Host `wide` and graph `bits` differ; `'0' -> '0'` maps wide index 3
        // to bits index 1 — a real, non-redundant remap.
        let src = "\
alphabet bits { '_', '0', '1' }
alphabet wide { '_', 'a', 'b', '0', '1' }
routine plusOne(tape num: bits) { entry state s { [*] -> return; } }
machine {
  tape ctl:  bits;
  tape data: wide;
  entry state go { [*, *] -> call plusOne(num = data with map { '0' -> '0' }) then done; }
  state done { [*, *] -> stop; }
}
";
        assert!(findings(src).is_empty(), "{:?}", findings(src));
    }

    #[test]
    fn a_non_identity_pair_over_a_shared_alphabet_is_quiet() {
        // `'a' -> 'b'` is a genuine swap, not an identity.
        let src = "\
alphabet ab { '_', 'a', 'b' }
routine echo(tape t: ab) { entry state s { [*] -> return; } }
machine {
  tape m: ab;
  entry state go { [*] -> call echo(t = m with map { 'a' -> 'b', 'b' -> 'a' }) then done; }
  state done { [*] -> stop; }
}
";
        assert!(findings(src).is_empty(), "{:?}", findings(src));
    }

    #[test]
    fn allow_suppresses_the_finding() {
        let src = "\
alphabet ab { '_', 'a', 'b' }
routine echo(tape t: ab) { entry state s { [*] -> return; } }
machine {
  tape m: ab;
  entry state go { [*] -> call echo(t = m with map { 'a' -> 'a' }) then done; }
  state done { [*] -> stop; }
}
";
        let report = lint(
            src,
            LintOptions {
                allow: vec!["redundant-identity-pairs".to_string()],
                warn: Vec::new(),
            },
        )
        .unwrap();
        assert!(
            report
                .diagnostics
                .iter()
                .all(|d| d.code != "redundant-identity-pairs")
        );
    }
}
