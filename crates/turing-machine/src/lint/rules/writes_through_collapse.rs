//! `writes-through-collapse`: a `call`/`graft`/`bind` whose one-way (`=>`)
//! symbol map collapses onto a callee glyph the callee then WRITES.
//!
//! A one-way pair `src => dst` maps the caller glyph `src` to the callee glyph
//! `dst` on READ only — it is deliberately excluded from write-back
//! (docs/formats.md (bound calls)). So when the callee body writes `dst`, that
//! write bypasses the map and lands on the host as identity, not back through
//! the collapse — usually a surprise, since the author used `=>` precisely to
//! say "read-collapse, do not write here." This flags exactly that: a `=>`
//! pair whose `dst` a LOCAL callee provably writes as a literal at the bound
//! tape's position. A `Subst`/`Keep` write is not provably `dst`, so it does
//! not fire (no false positives); an external callee's body is unseen and is
//! skipped. Report-only.

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

fn check_binding(callee: &ResolvedWorld, args: &[BindingArg], out: &mut Vec<Diagnostic>) {
    let written = written_literals(callee);
    for arg in args {
        let BindingValue::Named {
            map: Some(map), ..
        } = &arg.value
        else {
            continue;
        };
        // The callee tape position the map applies to.
        let Some(k) = callee.tapes.iter().position(|t| t.name == arg.name) else {
            continue;
        };
        for pair in &map.pairs {
            if pair.arrow != MapArrow::ReadOnly {
                continue;
            }
            let dst = glyph_label(&pair.dst);
            if written.contains(&(k, dst.clone())) {
                out.push(Diagnostic {
                    code: "writes-through-collapse",
                    span: pair.span,
                    message: format!(
                        "one-way map collapses onto `{dst}`, which `{}` writes — the write bypasses the collapse",
                        callee.name
                    ),
                    fix: None,
                });
            }
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

    for world in &ctx.resolved.worlds {
        for call in &world.calls {
            if let ResolvedCallTarget::Routine {
                name,
                external: false,
                args,
            } = &call.target
                && let Some(c) = callee(name)
            {
                check_binding(c, args, out);
            }
        }
        for graft in &world.grafts {
            if let Some(c) = callee(&graft.target) {
                check_binding(c, &graft.args, out);
            }
        }
        for bind in &world.binds {
            if !bind.external
                && let Some(c) = callee(&bind.target)
            {
                check_binding(c, &bind.args, out);
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
