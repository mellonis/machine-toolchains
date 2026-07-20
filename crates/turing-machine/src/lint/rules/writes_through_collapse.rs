//! `writes-through-collapse`: a `call`/`graft`/`bind` whose one-way (`=>`)
//! symbol map collapses onto a callee glyph the callee then WRITES.
//!
//! A one-way pair `src => dst` maps the caller glyph `src` to the callee glyph
//! `dst` on READ only — it is deliberately excluded from write-back
//! (docs/formats.md (bound calls)). So when the callee body writes `dst`, that
//! write never travels back through the collapse — usually a surprise, since
//! the author used `=>` precisely to say "read-collapse, do not write here."
//! This flags exactly that: a `=>` pair whose `dst` a LOCAL callee provably
//! writes as a literal at the bound tape's position. A `Subst`/`Keep` write is
//! not provably `dst`, so it does not fire (no false positives); an external
//! callee's body is unseen and is skipped. Report-only.
//!
//! # Why the rule reads the alphabets
//!
//! What actually HAPPENS to that write depends on how the binding's maps
//! complete, and that turns on the two tapes' sizes:
//!
//! - **Equal-size** alphabets identity-complete, so the write lands on the
//!   host as identity — surprising, but the program runs.
//! - **Unequal** alphabets complete CLOSED: every callee symbol without a
//!   two-way pair writing back is a write hole, and crossing a hole traps.
//!   The program does not merely surprise, it stops.
//!
//! The two outcomes need different words, so the rule resolves both alphabets
//! and branches its message. When either alphabet does not resolve there is no
//! way to pick, and the rule stays quiet — under-reporting rather than
//! guessing, the same trade `redundant-identity-pairs` makes.
//!
//! That size split also decides the degenerate pair `x => x`. Across
//! equal-size alphabets identity completion sends the write straight back to
//! `x`, which is what a two-way `x -> x` does too: nothing surprising happens,
//! so the rule says nothing. Across unequal alphabets the very same pair
//! establishes no write-back at all, so `x` is a hole and the write traps —
//! well worth flagging.
//!
//! Known limitation: the unequal branch assumes `dst` is unlisted elsewhere in
//! the map. A two-way pair naming the same `dst` would supply write-back and
//! spare the trap; the rule does not distinguish that case and reports the
//! trap wording anyway.

use std::collections::HashSet;

use mtc_core::diagnostics::Diagnostic;

use crate::compiler::{ResolvedCallTarget, ResolvedWorld};
use crate::lint::LintContext;
use crate::lint::patterns::glyph_label;
use crate::parser::{BindingArg, BindingValue, MapArrow, WriteCellKind};

/// Every `(tape position, glyph)` the callee provably writes as a literal —
/// the sound "is this symbol written here" oracle (`Subst`/`Keep` excluded).
fn written_literals(callee: &ResolvedWorld) -> HashSet<(usize, String)> {
    let mut written = HashSet::new();
    for state in &callee.states {
        for rule in &state.rules {
            if let Some(write) = &rule.write {
                for (k, cell) in write.cells.iter().enumerate() {
                    if let WriteCellKind::Lit(s) = &cell.kind {
                        written.insert((k, glyph_label(s)));
                    }
                }
            }
        }
    }
    written
}

/// The glyph vector of `world`'s tape named `tape_name`, if it has one.
fn tape_glyphs<'a>(
    ctx: &'a LintContext,
    world: &ResolvedWorld,
    tape_name: &str,
) -> Option<&'a [String]> {
    let tape = world.tapes.iter().find(|t| t.name == tape_name)?;
    crate::lint::alphabet_glyphs(ctx.resolved, &tape.alphabet)
}

fn check_binding(
    ctx: &LintContext,
    caller: &ResolvedWorld,
    callee: &ResolvedWorld,
    args: &[BindingArg],
    out: &mut Vec<Diagnostic>,
) {
    let written = written_literals(callee);
    for arg in args {
        let BindingValue::Named {
            target,
            map: Some(map),
            ..
        } = &arg.value
        else {
            continue;
        };
        // The callee tape position the map applies to.
        let Some(k) = callee.tapes.iter().position(|t| t.name == arg.name) else {
            continue;
        };
        // Both sides must resolve: the completion rule — and so which of the
        // two messages is true — is a function of the two alphabets' sizes.
        let (Some(graph), Some(host)) = (
            tape_glyphs(ctx, callee, &arg.name),
            tape_glyphs(ctx, caller, target),
        ) else {
            continue;
        };
        let equal_size = host.len() == graph.len();
        for pair in &map.pairs {
            if pair.arrow != MapArrow::ReadOnly {
                continue;
            }
            let dst = glyph_label(&pair.dst);
            // `x => x` across equal-size alphabets is identity completion
            // spelled out — behaviourally a two-way pair, nothing to report.
            // Across unequal alphabets it is a write hole like any other.
            if equal_size && glyph_label(&pair.src) == dst {
                continue;
            }
            if !written.contains(&(k, dst.clone())) {
                continue;
            }
            let message = if equal_size {
                format!(
                    "one-way map collapses onto `{dst}`, which `{}` writes — the write bypasses the collapse",
                    callee.name
                )
            } else {
                format!(
                    "one-way map collapses onto `{dst}`, which `{}` writes — the alphabets differ in size, so that write is a hole and traps",
                    callee.name
                )
            };
            out.push(Diagnostic {
                code: "writes-through-collapse",
                span: pair.span,
                message,
                fix: None,
            });
        }
    }
}

pub(crate) fn check(ctx: &LintContext, out: &mut Vec<Diagnostic>) {
    let by_name: std::collections::HashMap<&str, &ResolvedWorld> = ctx
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
            .filter(|d| d.code == "writes-through-collapse")
            .map(|d| d.message)
            .collect()
    }

    #[test]
    fn a_one_way_collapse_onto_a_written_glyph_fires() {
        // `'a' => 'b'` collapses onto `'b'`; `flip` writes `'b'`.
        let src = "\
alphabet ab { '_', 'a', 'b' }
routine flip(tape t: ab) { entry state s { [*] -> write ['b'] return; } }
machine {
  tape m: ab;
  entry state go { [*] -> call flip(t = m with map { 'a' => 'b' }) then done; }
  state done { [*] -> stop; }
}
";
        let f = findings(src);
        assert_eq!(f.len(), 1, "{f:?}");
        assert!(f[0].contains("`b`"), "{f:?}");
        // Equal-size alphabets identity-complete, so this one really is the
        // "surprising but it runs" case and keeps the bypass wording.
        assert!(f[0].contains("bypasses the collapse"), "{f:?}");
    }

    #[test]
    fn a_one_way_collapse_the_callee_never_writes_is_quiet() {
        // `peek` reads `'b'` and moves, but never WRITES it — the sound
        // negative: the collapse loses nothing.
        let src = "\
alphabet ab { '_', 'a', 'b' }
routine peek(tape t: ab) { entry state s { ['b'] -> return; [*] -> move [>] goto s; } }
machine {
  tape m: ab;
  entry state go { [*] -> call peek(t = m with map { 'a' => 'b' }) then done; }
  state done { [*] -> stop; }
}
";
        assert!(findings(src).is_empty(), "{:?}", findings(src));
    }

    #[test]
    fn a_bidirectional_pair_onto_a_written_glyph_is_quiet() {
        // `'a' -> 'b'` has write-back, so a callee write of `'b'` maps home —
        // no collapse loss, no finding.
        let src = "\
alphabet ab { '_', 'a', 'b' }
routine flip(tape t: ab) { entry state s { [*] -> write ['b'] return; } }
machine {
  tape m: ab;
  entry state go { [*] -> call flip(t = m with map { 'a' -> 'b', 'b' -> 'a' }) then done; }
  state done { [*] -> stop; }
}
";
        assert!(findings(src).is_empty(), "{:?}", findings(src));
    }

    #[test]
    fn a_degenerate_self_pair_over_one_shared_alphabet_is_quiet() {
        // `'a' => 'a'` across EQUAL-size alphabets is what identity completion
        // supplies anyway: the write lands back on `'a'`, exactly as a two-way
        // pair would, so there is no surprise to report. Verified against the
        // VM — this program runs to `Stopped` with `'a'` on the host tape.
        let src = "\
alphabet ab { '_', 'a', 'b' }
routine w(tape t: ab) { entry state s { [*] -> write ['a'] return; } }
machine {
  tape m: ab;
  entry state go { [*] -> call w(t = m with map { 'a' => 'a' }) then done; }
  state done { [*] -> stop; }
}
";
        assert!(findings(src).is_empty(), "{:?}", findings(src));
    }

    #[test]
    fn a_collapse_across_unequal_alphabets_reports_the_trap() {
        // Host `big` (4) and graph `small` (3) differ in size, so the binding
        // completes CLOSED: `'x'` has no two-way pair writing back, the callee's
        // write of `'x'` crosses a hole, and the program traps rather than
        // merely surprising. Verified against the VM — `UnmappedWrite`.
        let src = "\
alphabet small { '_', 'x', 'y' }
alphabet big { '_', 'x', 'y', 'z' }
routine w(tape t: small) { entry state s { [*] -> write ['x'] return; } }
machine {
  tape m: big;
  entry state go { [*] -> call w(t = m with map { 'y' => 'x' }) then done; }
  state done { [*] -> stop; }
}
";
        let f = findings(src);
        assert_eq!(f.len(), 1, "{f:?}");
        assert!(f[0].contains("traps"), "{f:?}");
        assert!(f[0].contains("differ in size"), "{f:?}");
    }

    #[test]
    fn a_self_pair_across_unequal_alphabets_still_reports_the_trap() {
        // The degenerate spelling is only degenerate where identity completion
        // applies. Across unequal alphabets `'x' => 'x'` establishes no
        // write-back at all, so it holes `'x'` exactly like the pair above —
        // silencing it would hide a program that halts. Verified against the
        // VM — `UnmappedWrite`.
        let src = "\
alphabet small { '_', 'x', 'y' }
alphabet big { '_', 'x', 'y', 'z' }
routine w(tape t: small) { entry state s { [*] -> write ['x'] return; } }
machine {
  tape m: big;
  entry state go { [*] -> call w(t = m with map { 'x' => 'x' }) then done; }
  state done { [*] -> stop; }
}
";
        let f = findings(src);
        assert_eq!(f.len(), 1, "{f:?}");
        assert!(f[0].contains("traps"), "{f:?}");
    }

    #[test]
    fn a_graft_one_way_collapse_onto_a_written_glyph_fires() {
        let src = "\
alphabet ab { '_', 'a', 'b' }
graph paint(tape t: ab, state done) {
  entry state s { [*] -> write ['b'] done; }
}
machine {
  tape m: ab;
  entry graft paint(t = m with map { 'a' => 'b' }, done = fin) as g;
  state fin { [*] -> stop; }
}
";
        let f = findings(src);
        assert_eq!(f.len(), 1, "{f:?}");
    }

    #[test]
    fn allow_suppresses_the_finding() {
        let src = "\
alphabet ab { '_', 'a', 'b' }
routine flip(tape t: ab) { entry state s { [*] -> write ['b'] return; } }
machine {
  tape m: ab;
  entry state go { [*] -> call flip(t = m with map { 'a' => 'b' }) then done; }
  state done { [*] -> stop; }
}
";
        let report = lint(
            src,
            LintOptions {
                allow: vec!["writes-through-collapse".to_string()],
                warn: Vec::new(),
            },
        )
        .unwrap();
        assert!(
            report
                .diagnostics
                .iter()
                .all(|d| d.code != "writes-through-collapse")
        );
    }
}
