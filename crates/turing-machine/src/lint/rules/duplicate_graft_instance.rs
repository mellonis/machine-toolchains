//! `duplicate-graft-instance`: two `graft` sites in the SAME world that
//! splice the identical graph with the identical bindings and the
//! identical continuation — the expander already treats them as one
//! subgraph (`expand.rs`'s own instance dedup: a graft site's key is its
//! target's graph, its tape composite, and its continuation substitution;
//! two sites that agree on all three alias to one splice and never emit a
//! second copy of the graph's states), so a second, third, … site earns
//! its own name and its own line of source without earning its own
//! subgraph. This rule reads the KEY that dedup decision is made on
//! straight from the expander (`crate::expand::graft_instance_key`) rather
//! than re-deriving it — a second copy of that byte-building logic would
//! drift out of step with the expander's actual dedup and either miss a
//! true duplicate or flag two sites the expander does NOT alias.
//!
//! Dedup — here and in the expander — is scoped to ONE world's own graft
//! list: two identical grafts in a MACHINE and in a ROUTINE, or in two
//! different graphs, are never compared against each other, because the
//! expander never compares them against each other either (each host
//! world's splice runs with its own fresh dedup table).
//!
//! # The fix
//!
//! Deletes the duplicate's whole `graft … ;` statement and rewrites every
//! `goto` inside the world that named its instance to name the surviving
//! (earlier) instance instead — the non-trivial half, since an orphaned
//! `goto` would be a dangling reference, not a clean removal.
//!
//! The fix is WITHHELD, never emitted partially, for any shape it cannot
//! rewrite safely:
//!
//! - **The duplicate carries `entry`.** Its own entry-ness would need to
//!   move onto the surviving instance's own declaration — an insertion
//!   this fix does not attempt — so it withholds rather than dropping the
//!   world's entry silently.
//! - **The removed name is referenced by anything other than a bare
//!   `goto`** — a `call … then NAME` continuation, or a binding argument
//!   naming it as a state continuation. Rewriting those needs the same
//!   name-substitution the goto case gets, but a bare binding-argument
//!   target is not yet classified as a tape target vs. a state
//!   continuation at this stage (the same over-approximation
//!   `unused-graft-instance` documents), so a blind rewrite risks
//!   renaming what is actually a tape name — withheld rather than guessed.
//! - **The surviving instance carries no `as` name** while some `goto`
//!   still names the removed one — there is nothing to redirect onto.
//! - **Any computed span is `None`**, or the comment guard
//!   (`crate::lint::run_rules`) finds a comment inside one — the shared
//!   chokepoint every rule's fix goes through, pinned here the same way
//!   `tests/lint_fix_comment_guard.rs` pins every other fix-emitting rule.

use std::collections::HashMap;

use mtc_core::diagnostics::{Applicability, Diagnostic, Edit, Fix, Span};

use crate::compiler::{ResolvedGraft, ResolvedWorld};
use crate::expand::graft_instance_key;
use crate::lint::LintContext;
use crate::lint::rules::spans::{decl_span, goto_target_span};
use crate::parser::{BindingArg, BindingValue, Continuation, Transition};
use crate::syntax::GraftView;

/// Every `goto`/bare-name transition inside `world` naming `name` — the
/// transition's own span, feeding [`goto_target_span`] at fix time.
fn goto_sites(world: &ResolvedWorld, name: &str) -> Vec<Span> {
    world
        .states
        .iter()
        .flat_map(|s| &s.rules)
        .filter_map(|r| match &r.transition {
            Transition::Goto { name: n, span, .. } if n == name => Some(*span),
            _ => None,
        })
        .collect()
}

/// Whether `name` is referenced anywhere in `world` OTHER than a bare
/// `goto` — a `call … then NAME` continuation, or a bare binding-argument
/// target (`x = NAME`) on any graft or bind in the world. Either shape
/// this rule's own redirect cannot rewrite (see the module doc); finding
/// either withholds the fix.
fn other_reference_exists(world: &ResolvedWorld, name: &str) -> bool {
    let names_target = |args: &[BindingArg]| {
        args.iter()
            .any(|a| matches!(&a.value, BindingValue::Named { target, .. } if target == name))
    };
    if world.grafts.iter().any(|g| names_target(&g.args)) {
        return true;
    }
    if world.binds.iter().any(|b| names_target(&b.args)) {
        return true;
    }
    world.states.iter().any(|s| {
        s.rules.iter().any(|r| match &r.transition {
            Transition::Call { args, then, .. } => {
                names_target(args)
                    || matches!(then, Some(Continuation::State { name: n, .. }) if n == name)
            }
            _ => false,
        })
    })
}

/// The quickfix for `dup`, a duplicate of `canonical` in `world` — `None`
/// for any shape the module doc withholds. Never returns a PARTIAL set of
/// edits: every span lookup below must succeed, or the whole fix is
/// dropped.
fn build_fix(
    ctx: &LintContext,
    world: &ResolvedWorld,
    dup: &ResolvedGraft,
    canonical: &ResolvedGraft,
) -> Option<Fix> {
    if dup.entry {
        return None;
    }
    let mut edits = vec![Edit {
        span: decl_span::<GraftView>(ctx, dup.target_span)?,
        replacement: String::new(),
    }];

    let mut redirected_to = None;
    if let Some(dup_name) = &dup.as_name {
        if other_reference_exists(world, dup_name) {
            return None;
        }
        let sites = goto_sites(world, dup_name);
        if !sites.is_empty() {
            let canonical_name = canonical.as_name.as_ref()?;
            for span in sites {
                edits.push(Edit {
                    span: goto_target_span(ctx, span)?,
                    replacement: canonical_name.clone(),
                });
            }
            redirected_to = Some(canonical_name.as_str());
        }
    }

    let description = match redirected_to {
        Some(name) => format!("delete the duplicate graft and redirect its `goto`s to `{name}`"),
        None => "delete the duplicate graft".to_string(),
    };
    Some(Fix {
        description,
        applicability: Applicability::MaybeIncorrect,
        edits,
    })
}

pub(crate) fn check(ctx: &LintContext, out: &mut Vec<Diagnostic>) {
    let ext_modules = ctx.externals.modules();
    for world in &ctx.resolved.worlds {
        let mut seen: HashMap<Vec<u8>, &ResolvedGraft> = HashMap::new();
        for graft in &world.grafts {
            let Some(key) = graft_instance_key(graft, world, ctx.resolved, &ext_modules) else {
                continue;
            };
            let Some(&canonical) = seen.get(&key) else {
                seen.insert(key, graft);
                continue;
            };
            let message = match &graft.as_name {
                Some(name) => format!(
                    "graft instance `{name}` duplicates an earlier graft of `{}` with the same bindings and continuation",
                    graft.target
                ),
                None => format!(
                    "this graft of `{}` duplicates an earlier graft with the same bindings and continuation",
                    graft.target
                ),
            };
            out.push(Diagnostic {
                code: "duplicate-graft-instance",
                span: graft.span,
                message,
                fix: build_fix(ctx, world, graft, canonical),
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::lint::{LintOptions, lint};

    fn findings(src: &str) -> Vec<mtc_core::diagnostics::Diagnostic> {
        lint(src, LintOptions::default())
            .unwrap()
            .diagnostics
            .into_iter()
            .filter(|d| d.code == "duplicate-graft-instance")
            .collect()
    }

    const FIRING: &str = "\
alphabet marks { '_', 'x' }
graph findX(tape t: marks, state found, state missing) {
  entry state walk { ['x'] -> found; ['_'] -> missing; [*] -> move [>] goto walk; }
}
machine {
  tape work: marks;
  graft findX(t = work, found = win, missing = lose) as one;
  graft findX(t = work, found = win, missing = lose) as two;
  entry state go { ['x'] -> goto one; [*] -> goto two; }
  state win  { [*] -> stop; }
  state lose { [*] -> halt; }
}
";

    #[test]
    fn two_identical_grafts_fire_exactly_once() {
        // Mutation this guards: if the key ever stopped including the
        // composite/continuation and used the target alone, this still
        // fires (correctly) — the NEAR MISS test below is what that
        // mutation actually breaks.
        let f = findings(FIRING);
        assert_eq!(f.len(), 1, "{f:?}");
        assert!(f[0].message.contains("two"), "{:?}", f[0].message);
        assert!(f[0].fix.is_some(), "{:?}", f[0]);
    }

    #[test]
    fn a_lone_graft_is_quiet() {
        let src = "\
alphabet marks { '_', 'x' }
graph findX(tape t: marks, state found, state missing) {
  entry state walk { ['x'] -> found; ['_'] -> missing; [*] -> move [>] goto walk; }
}
machine {
  tape work: marks;
  graft findX(t = work, found = win, missing = lose) as one;
  entry state go { [*] -> goto one; }
  state win  { [*] -> stop; }
  state lose { [*] -> halt; }
}
";
        assert!(findings(src).is_empty(), "{:?}", findings(src));
    }

    #[test]
    fn a_near_miss_differing_only_in_continuation_is_quiet() {
        // Same target, same composite, but `two`'s `missing` binds a
        // DIFFERENT state than `one`'s — distinct continuations, not a
        // duplicate. Mutation this guards: keying on the graft target
        // ALONE (dropping the composite/continuation bytes) makes this
        // fire; verified by hand against `expand.rs::graft_site_key_bytes`
        // — dropping its `comp.key()`/`cont_key(cont)` extensions and
        // re-running this test turns it red.
        let src = "\
alphabet marks { '_', 'x' }
graph findX(tape t: marks, state found, state missing) {
  entry state walk { ['x'] -> found; ['_'] -> missing; [*] -> move [>] goto walk; }
}
machine {
  tape work: marks;
  graft findX(t = work, found = win, missing = lose) as one;
  graft findX(t = work, found = win, missing = lose2) as two;
  entry state go { ['x'] -> goto one; [*] -> goto two; }
  state win   { [*] -> stop; }
  state lose  { [*] -> halt; }
  state lose2 { [*] -> halt; }
}
";
        assert!(findings(src).is_empty(), "{:?}", findings(src));
    }

    #[test]
    fn two_graphs_of_different_owners_sharing_a_simple_name_are_not_conflated() {
        // `walker` (top-level) and `lib::walker` (inside `namespace lib`)
        // are two DIFFERENT declared graphs that happen to share a bare
        // name — different owning scopes, identical bindings and
        // continuation on both sites. Mutation this guards: keying on the
        // graft target's LAST `::`-segment alone (stripping the `lib::`
        // qualifier) conflates them into one key; verified by hand — a
        // stripped-target key run over this source produces one finding,
        // where the real key (the full, unstripped `graft.target` string)
        // produces none.
        let src = "\
alphabet marks { '_', 'x' }
graph walker(tape t: marks, state done) {
  entry state w { [*] -> done; }
}
namespace lib {
  graph walker(tape t: marks, state done) {
    entry state w { [*] -> done; }
  }
}
machine {
  tape work: marks;
  graft walker(t = work, done = fin) as one;
  graft lib::walker(t = work, done = fin) as two;
  entry state go { ['x'] -> goto one; [*] -> goto two; }
  state fin { [*] -> stop; }
}
";
        assert!(findings(src).is_empty(), "{:?}", findings(src));
    }

    #[test]
    fn an_entry_duplicate_withholds_the_fix() {
        // `two` is the SECOND site but carries `entry` — moving entry-ness
        // onto `one`'s own declaration is an edit this fix does not
        // attempt, so it withholds rather than silently dropping the
        // world's entry. Mutation this guards: a fix builder that skips
        // the `dup.entry` check would offer a fix here; asserting `None`
        // catches that regression directly.
        let src = "\
alphabet marks { '_', 'x' }
graph findX(tape t: marks, state found, state missing) {
  entry state walk { ['x'] -> found; ['_'] -> missing; [*] -> move [>] goto walk; }
}
machine {
  tape work: marks;
  graft findX(t = work, found = win, missing = lose) as one;
  entry graft findX(t = work, found = win, missing = lose) as two;
  state win  { [*] -> stop; }
  state lose { [*] -> halt; }
}
";
        let f = findings(src);
        assert_eq!(f.len(), 1, "{f:?}");
        assert!(f[0].fix.is_none(), "{:?}", f[0]);
    }

    #[test]
    fn a_removed_name_reached_by_a_call_then_continuation_withholds_the_fix() {
        // `two` is reached only by `call helper(...) then two`, never a bare
        // `goto` — this rule's redirect only rewrites goto targets, so it
        // withholds rather than leaving `then two` dangling. Mutation this
        // guards: an `other_reference_exists` that ignores `Continuation::
        // State` would offer a fix here that produces a broken program.
        let src = "\
alphabet marks { '_', 'x' }
graph findX(tape t: marks, state found, state missing) {
  entry state walk { ['x'] -> found; ['_'] -> missing; [*] -> move [>] goto walk; }
}
routine helper(tape t: marks) { entry state s { [*] -> return; } }
machine {
  tape work: marks;
  graft findX(t = work, found = win, missing = lose) as one;
  graft findX(t = work, found = win, missing = lose) as two;
  entry state go { ['x'] -> goto one; [*] -> call helper(t = work) then two; }
  state win  { [*] -> stop; }
  state lose { [*] -> halt; }
}
";
        let f = findings(src);
        assert_eq!(f.len(), 1, "{f:?}");
        assert!(f[0].fix.is_none(), "{:?}", f[0]);
    }
}
