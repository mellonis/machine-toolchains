//! `index-identity-map` (OPT-IN): a `call` or `bind` with an OMITTED symbol
//! map binds a caller tape to a callee tape param whose alphabets are not
//! glyph-for-glyph equal. With no explicit `with map { … }`, the binding maps
//! by INDEX (docs/formats.md (bound calls)) — caller symbol at index `i`
//! becomes the callee symbol at the same index — so a glyph the caller reads
//! as one thing the callee reads as another. That is occasionally intended (a
//! deliberate re-labelling by position), so the rule is off by default:
//! enable it per run with `--warn index-identity-map`, never by removing an
//! allow.
//!
//! # What it sees
//!
//! Only `call` and `bind` (a graft's index binding is out of scope). The check
//! mirrors `redundant-identity-pairs`, inverted: that rule fires when the two
//! alphabets are IDENTICAL and an `x -> x` pair is therefore redundant; this
//! one fires when the omitted map spans alphabets that DIFFER at some shared
//! index. The message names the first differing index and both glyphs (caller
//! side first, then callee, matching the caller → callee direction of the
//! map).
//!
//! # Silent when
//!
//! A map is written (`with map { … }` — the author is explicit and owns the
//! reinterpretation); the two alphabets are glyph-for-glyph equal over their
//! shared indices (the index map preserves every glyph); the callee's alphabet
//! is not visible in this compilation (an external routine, resolved at link —
//! its tape signature is unknown here); or the binding argument is not a
//! tape-to-tape map at all (a state continuation, whose name resolves to no
//! tape on either side). Every path errs toward silence.
//!
//! No fix ships: writing the intended map needs the author's intent — which
//! glyph should become which — that the tool cannot guess.

use std::collections::HashMap;

use mtc_core::diagnostics::Diagnostic;

use crate::compiler::{ResolvedCallTarget, ResolvedWorld};
use crate::lint::LintContext;
use crate::parser::{BindingArg, BindingValue};

/// The glyph vector of `world`'s tape named `tape_name`, if it has one.
fn tape_glyphs<'a>(
    ctx: &'a LintContext,
    world: &ResolvedWorld,
    tape_name: &str,
) -> Option<&'a [String]> {
    let tape = world.tapes.iter().find(|t| t.name == tape_name)?;
    crate::lint::alphabet_glyphs(ctx.resolved, &tape.alphabet)
}

/// Flag each omitted-map tape binding whose caller tape and bound callee tape
/// draw from alphabets that differ at some shared index. `verb` is the source
/// construct doing the binding (`"call"` / `"bind"`).
fn check_binding(
    ctx: &LintContext,
    caller: &ResolvedWorld,
    callee: &ResolvedWorld,
    args: &[BindingArg],
    verb: &str,
    out: &mut Vec<Diagnostic>,
) {
    for arg in args {
        // Only an OMITTED map is index-mapped; a written map is the author's.
        let BindingValue::Named {
            target, map: None, ..
        } = &arg.value
        else {
            continue;
        };
        // A tape binding: the callee param `arg.name` and the caller tape
        // `target` must each name a tape (a state-continuation arg resolves to
        // neither, so at least one lookup misses and the arg is skipped).
        let (Some(callee_glyphs), Some(caller_glyphs)) = (
            tape_glyphs(ctx, callee, &arg.name),
            tape_glyphs(ctx, caller, target),
        ) else {
            continue;
        };
        // The first shared index whose glyphs differ. Glyph-for-glyph equal
        // alphabets (and equal shared prefixes) never fire.
        if let Some((i, caller_glyph, callee_glyph)) = caller_glyphs
            .iter()
            .zip(callee_glyphs)
            .enumerate()
            .find_map(|(i, (a, b))| (a != b).then_some((i, a.as_str(), b.as_str())))
        {
            out.push(Diagnostic {
                code: "index-identity-map",
                span: arg.span,
                message: format!(
                    "{verb} maps by index across differently-glyphed alphabets ('{caller_glyph}' vs '{callee_glyph}' at index {i}); glyphs change meaning here"
                ),
                fix: None,
            });
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
            // An external callee's signature is not visible here — skip it (a
            // plain bind-name call carries no args of its own; its binding was
            // checked at the `bind`).
            if let ResolvedCallTarget::Routine {
                name,
                external: false,
                args,
            } = &call.target
                && let Some(c) = callee(name)
            {
                check_binding(ctx, caller, c, args, "call", out);
            }
        }
        for bind in &caller.binds {
            if !bind.external
                && let Some(c) = callee(&bind.target)
            {
                check_binding(ctx, caller, c, &bind.args, "bind", out);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::lint::{LintOptions, lint};

    fn warn_opts() -> LintOptions {
        LintOptions {
            allow: Vec::new(),
            warn: vec!["index-identity-map".to_string()],
        }
    }

    fn findings(src: &str, opts: LintOptions) -> Vec<String> {
        lint(src, opts)
            .unwrap()
            .diagnostics
            .into_iter()
            .filter(|d| d.code == "index-identity-map")
            .map(|d| d.message)
            .collect()
    }

    // Caller tape `m: ab` (`_`,`a`) bound to callee param `t: xy` (`_`,`x`)
    // with NO map — index 1 differs ('a' vs 'x').
    const FIRE_CALL: &str = "\
alphabet ab { '_', 'a' }
alphabet xy { '_', 'x' }
routine echo(tape t: xy) { entry state s { [*] -> return; } }
machine {
  tape m: ab;
  entry state go { [*] -> call echo(t = m) then done; }
  state done { [*] -> stop; }
}
";

    // The same mismatch reached through a `bind … as` instead of a direct call.
    const FIRE_BIND: &str = "\
alphabet ab { '_', 'a' }
alphabet xy { '_', 'x' }
routine echo(tape t: xy) { entry state s { [*] -> return; } }
machine {
  tape m: ab;
  bind echo(t = m) as h;
  entry state go { [*] -> call h() then done; }
  state done { [*] -> stop; }
}
";

    #[test]
    fn off_by_default_even_on_a_mismatch() {
        assert!(findings(FIRE_CALL, LintOptions::default()).is_empty());
    }

    #[test]
    fn warn_enables_it_and_a_call_mismatch_fires() {
        let f = findings(FIRE_CALL, warn_opts());
        assert_eq!(f.len(), 1, "{f:?}");
        assert_eq!(
            f[0],
            "call maps by index across differently-glyphed alphabets ('a' vs 'x' at index 1); glyphs change meaning here"
        );
    }

    #[test]
    fn warn_enables_it_and_a_bind_mismatch_fires() {
        let f = findings(FIRE_BIND, warn_opts());
        assert_eq!(f.len(), 1, "{f:?}");
        assert_eq!(
            f[0],
            "bind maps by index across differently-glyphed alphabets ('a' vs 'x' at index 1); glyphs change meaning here"
        );
    }

    #[test]
    fn allow_beats_warn() {
        let opts = LintOptions {
            allow: vec!["index-identity-map".to_string()],
            warn: vec!["index-identity-map".to_string()],
        };
        assert!(findings(FIRE_CALL, opts).is_empty());
    }

    // Silent case 1: a map is written — the author owns the reinterpretation.
    #[test]
    fn a_written_map_is_silent() {
        let src = "\
alphabet ab { '_', 'a' }
alphabet xy { '_', 'x' }
routine echo(tape t: xy) { entry state s { [*] -> return; } }
machine {
  tape m: ab;
  entry state go { [*] -> call echo(t = m with map { 'a' => 'x' }) then done; }
  state done { [*] -> stop; }
}
";
        assert!(
            findings(src, warn_opts()).is_empty(),
            "{:?}",
            findings(src, warn_opts())
        );
    }

    // Silent case 2: caller and callee draw from the same alphabet — index
    // mapping preserves every glyph.
    #[test]
    fn glyph_equal_alphabets_are_silent() {
        let src = "\
alphabet ab { '_', 'a' }
routine echo(tape t: ab) { entry state s { [*] -> return; } }
machine {
  tape m: ab;
  entry state go { [*] -> call echo(t = m) then done; }
  state done { [*] -> stop; }
}
";
        assert!(
            findings(src, warn_opts()).is_empty(),
            "{:?}",
            findings(src, warn_opts())
        );
    }

    // Silent case 3: the callee is external (undeclared here) — its alphabet
    // is not visible in this compilation, so no mismatch can be proven.
    #[test]
    fn an_external_callee_is_silent() {
        let src = "\
alphabet ab { '_', 'a' }
machine {
  tape m: ab;
  entry state go { [*] -> call ext(t = m) then done; }
  state done { [*] -> stop; }
}
";
        assert!(
            findings(src, warn_opts()).is_empty(),
            "{:?}",
            findings(src, warn_opts())
        );
    }

    // Silent case 4: a binding argument that is a state continuation, not a
    // tape-to-tape map — its name resolves to no tape, so it is skipped. (The
    // tape arg draws from the same alphabet, so it too is silent.)
    #[test]
    fn a_state_continuation_arg_is_silent() {
        let src = "\
alphabet ab { '_', 'a' }
routine echo(tape t: ab, state k) { entry state s { [*] -> goto k; } }
machine {
  tape m: ab;
  entry state go { [*] -> call echo(t = m, k = done) then done; }
  state done { [*] -> stop; }
}
";
        assert!(
            findings(src, warn_opts()).is_empty(),
            "{:?}",
            findings(src, warn_opts())
        );
    }
}
