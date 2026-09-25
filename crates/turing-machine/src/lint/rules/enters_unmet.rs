//! `enters-unmet`: a `call`/`bind` site whose callee declares `enters` on a
//! tape parameter, where the caller's own head-position analysis says the
//! head MAY be sitting on a glyph the clause does not list
//! (docs/tmt/language.md (head-position clauses)).
//!
//! The rule is a renderer only. The analysis it reads is
//! [`crate::head_flow::enters_gaps`], which lives beside the write-footprint
//! fixpoint in the compiler for the same reason that one does: it is a
//! whole-module dataflow over resolved source, not a per-declaration
//! inspection, and the compile side may want the same answer.
//!
//! Warn tier, no quickfix. The analysis over-approximates — the message says
//! "may be on" — and there is no single correct edit for a site that may be
//! wrong: a wider `enters`, a narrower call site, and a guard row ahead of
//! the call are all remedies, and nothing here can tell which was meant. The
//! authority on what a given run does is the `enters` assert a debug build
//! plants, not this finding (docs/tmt/lint.md (enters-unmet)).

use mtc_core::diagnostics::Diagnostic;

use crate::head_flow::enters_gaps;
use crate::lint::LintContext;

/// A glyph list as the diagnostics render one everywhere else: `'0', '1'`.
fn named(glyphs: &[String]) -> String {
    glyphs
        .iter()
        .map(|g| format!("'{g}'"))
        .collect::<Vec<_>>()
        .join(", ")
}

pub(crate) fn check(ctx: &LintContext, out: &mut Vec<Diagnostic>) {
    for gap in enters_gaps(ctx.resolved, ctx.externals) {
        out.push(Diagnostic {
            code: "enters-unmet",
            span: gap.site,
            message: format!(
                "the head may be on {} here, outside the `enters {{ {} }}` that `{}`'s tape `{}` declares",
                named(&gap.offending),
                named(&gap.declared),
                gap.callee,
                gap.param,
            ),
            fix: None,
        });
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
            .filter(|d| d.code == "enters-unmet")
            .map(|d| d.message)
            .collect()
    }

    /// Every other finding a fixture produces — the fixtures below are
    /// meant to be otherwise clean, and a stray `unused-…` would make a
    /// silence test pass for the wrong reason.
    fn other_codes(src: &str) -> Vec<&'static str> {
        lint(src, LintOptions::default())
            .unwrap()
            .diagnostics
            .into_iter()
            .filter(|d| d.code != "enters-unmet")
            .map(|d| d.code)
            .collect()
    }

    /// A callee whose one tape parameter declares `enters { '0', '1' }`;
    /// its entry state accepts both, so the static `enters-not-accepted`
    /// check is satisfied and only the SITE side is under test.
    const PLUS_ONE: &str = "\
routine plusOne(tape num: bits enters { '0', '1' }) {
  entry state inc {
    ['1'] -> write ['0'] move [<] goto inc;
    [*]   -> write ['1'] return;
  }
}
";

    /// The same routine with no clause at all.
    const PLUS_ONE_BARE: &str = "\
routine plusOne(tape num: bits) {
  entry state inc {
    ['1'] -> write ['0'] move [<] goto inc;
    [*]   -> write ['1'] return;
  }
}
";

    /// A one-row callee that always leaves `'0'` under the head, declaring
    /// `enters` but NOT `leaves`.
    const SETTLE: &str = "\
routine settle(tape num: bits enters { '0', '1' }) {
  entry state go { [*] -> write ['0'] return; }
}
";

    /// The same routine declaring `leaves { '0' }` as well — the body's own
    /// exit row already proves it, which is what the static
    /// `leaves-outside-contract` check holds it to.
    const SETTLE_LEAVES: &str = "\
routine settle(tape num: bits enters { '0', '1' } leaves { '0' }) {
  entry state go { [*] -> write ['0'] return; }
}
";

    fn program(routines: &str, machine: &str) -> String {
        format!("alphabet bits {{ '_', '0', '1' }}\n\n{routines}\n{machine}")
    }

    /// The firing fixture the spec names: `[*] -> call plusOne(…)` in a
    /// machine's entry state, where nothing has constrained the head yet
    /// and the callee declares `enters`. The head may be on the blank,
    /// which the clause does not list.
    ///
    /// Mutation: never comparing the projected set against the clause at
    /// all (the rule reports nothing) — this is the only fixture that goes
    /// red under it; every other one below asserts a SILENCE.
    #[test]
    fn an_unconstrained_call_into_a_callee_with_enters_is_a_finding() {
        let src = program(
            PLUS_ONE,
            "\
machine {
  tape data: bits;

  entry state s { [*] -> call plusOne(num = data) then done; }
  state done { [*] -> stop; }
}
",
        );
        assert_eq!(
            findings(&src),
            vec![
                "the head may be on '_' here, outside the `enters { '0', '1' }` that `plusOne`'s tape `num` declares"
            ]
        );
        assert!(other_codes(&src).is_empty(), "{:?}", other_codes(&src));
    }

    /// The same program with the calling row's PATTERN CELL narrowed to the
    /// callee's declared set — silent. The head cannot be on the blank at a
    /// row that only fires on `'0'` or `'1'`.
    ///
    /// Mutation: dropping the pattern-cell intersection, so a row carries
    /// its whole state's set forward — this fixture starts firing.
    #[test]
    fn a_pattern_constrained_call_site_is_silent() {
        let src = program(
            PLUS_ONE,
            "\
machine {
  tape data: bits;

  entry state s {
    ['0'..'1'] -> call plusOne(num = data) then done;
    [*]        -> stop;
  }
  state done { [*] -> stop; }
}
",
        );
        assert!(findings(&src).is_empty(), "{:?}", findings(&src));
        assert!(other_codes(&src).is_empty(), "{:?}", other_codes(&src));
    }

    /// A caller that declares its OWN `enters` and then calls from an
    /// unconstrained `[*]` row — silent, because the world's entry state is
    /// seeded from that clause rather than from the whole alphabet. The
    /// machine's own call into it is pattern-constrained, so it stays
    /// silent under the mutation too and the fixture reports exactly one
    /// new finding when the seed breaks.
    ///
    /// Mutation: seeding every entry state with the whole alphabet
    /// unconditionally instead of the world's own `enters` — `walk`'s call
    /// into `plusOne` starts firing.
    #[test]
    fn a_callers_own_enters_clause_seeds_its_entry_state() {
        let src = program(
            &format!(
                "{PLUS_ONE}
routine walk(tape num: bits enters {{ '0', '1' }}) {{
  entry state go {{ [*] -> call plusOne(num = num) then done; }}
  state done {{ [*] -> return; }}
}}
"
            ),
            "\
machine {
  tape data: bits;

  entry state s {
    ['0'..'1'] -> call walk(num = data) then fin;
    [*]        -> stop;
  }
  state fin { [*] -> stop; }
}
",
        );
        assert!(findings(&src).is_empty(), "{:?}", findings(&src));
        assert!(other_codes(&src).is_empty(), "{:?}", other_codes(&src));
    }

    /// The firing fixture's unconstrained `[*]` site against a callee that
    /// declares NO `enters` — silent. An absent clause permits every glyph;
    /// it is not an empty set.
    ///
    /// The second routine is load-bearing, not scenery: the analysis skips
    /// a module where nothing declares `enters` at all, so without a clause
    /// SOMEWHERE this fixture would be silent for a reason that has nothing
    /// to do with the property under test — and would stay silent under its
    /// own mutation. `settle` supplies the clause and is called from a
    /// pattern-constrained row, so it contributes no finding of its own.
    ///
    /// Mutation: reading a missing clause as the empty set — every glyph
    /// the head may be on becomes offending and the `plusOne` site fires.
    #[test]
    fn a_callee_that_declares_no_enters_is_never_reported() {
        let src = program(
            &format!("{PLUS_ONE_BARE}\n{SETTLE}"),
            "\
machine {
  tape data: bits;

  entry state s { [*] -> call plusOne(num = data) then done; }
  state done {
    ['0'..'1'] -> call settle(num = data) then fin;
    [*]        -> stop;
  }
  state fin { [*] -> stop; }
}
",
        );
        assert!(findings(&src).is_empty(), "{:?}", findings(&src));
        assert!(other_codes(&src).is_empty(), "{:?}", other_codes(&src));
    }

    /// A callee that declares `leaves { '0' }` narrows the continuation
    /// state it returns into, so a second call from there is silent even
    /// though the row that reaches it is a bare `[*]`.
    ///
    /// Mutation: dropping the `leaves` arm, so a continuation always
    /// resumes with the whole alphabet — this fixture starts firing.
    #[test]
    fn a_callees_declared_leaves_narrows_its_continuation_state() {
        let src = program(
            &format!("{PLUS_ONE}\n{SETTLE_LEAVES}"),
            "\
machine {
  tape data: bits;

  entry state s {
    ['0'..'1'] -> call settle(num = data) then again;
    [*]        -> stop;
  }
  state again { [*] -> call plusOne(num = data) then done; }
  state done { [*] -> stop; }
}
",
        );
        assert!(findings(&src).is_empty(), "{:?}", findings(&src));
        assert!(other_codes(&src).is_empty(), "{:?}", other_codes(&src));
    }

    /// The same program with the callee's `leaves` clause removed: the
    /// continuation resumes unconstrained, so the second call fires. The
    /// twin of the test above — together they pin that the continuation's
    /// set comes from the clause and not from a constant.
    ///
    /// Mutation: seeding every continuation from the callee's `enters`
    /// instead of its `leaves` (or from any other fixed set) — this
    /// fixture goes silent.
    #[test]
    fn without_a_leaves_clause_a_continuation_resumes_unconstrained() {
        let src = program(
            &format!("{PLUS_ONE}\n{SETTLE}"),
            "\
machine {
  tape data: bits;

  entry state s {
    ['0'..'1'] -> call settle(num = data) then again;
    [*]        -> stop;
  }
  state again { [*] -> call plusOne(num = data) then done; }
  state done { [*] -> stop; }
}
",
        );
        assert_eq!(
            findings(&src),
            vec![
                "the head may be on '_' here, outside the `enters { '0', '1' }` that `plusOne`'s tape `num` declares"
            ]
        );
    }

    /// A `bind` site: the binding lives on the declaration, the head
    /// position at the CALL row. The finding is reported at the row, not
    /// at the `bind`, because one instance may be called from several.
    ///
    /// Mutation: resolving only `ResolvedCallTarget::Routine` and skipping
    /// the bind arm — this fixture goes silent while the direct-call
    /// fixtures stay green.
    #[test]
    fn a_bind_site_is_checked_at_the_call_row() {
        let src = program(
            PLUS_ONE,
            "\
machine {
  tape data: bits;

  bind plusOne(num = data) as inc;

  entry state s { [*] -> call inc() then done; }
  state done { [*] -> stop; }
}
",
        );
        assert_eq!(
            findings(&src),
            vec![
                "the head may be on '_' here, outside the `enters { '0', '1' }` that `plusOne`'s tape `num` declares"
            ]
        );
        assert!(other_codes(&src).is_empty(), "{:?}", other_codes(&src));
    }

    /// The blank is pinned through a CLOSED map, and the message names the
    /// projected glyphs in the CALLEE's frame: `'C'` is a glyph the caller's
    /// alphabet does not even spell, reached from `'s'` through a one-way
    /// read pair (which reads exactly like a two-way one).
    ///
    /// Every non-blank caller glyph is listed here, on purpose: the hole
    /// rule therefore has nothing to decide, and this fixture answers for
    /// the blank pin alone.
    ///
    /// Mutation: dropping the blank pin, so index 0 falls to the closed
    /// map's hole rule — `'_'` leaves the offending list and the message
    /// changes.
    #[test]
    fn the_blank_is_pinned_through_a_closed_map() {
        let src = "\
alphabet wide  { '_', 'p', 'q', 'r', 's' }
alphabet small { '_', 'A', 'B', 'C' }

routine mark(tape t: small enters { 'A' }) {
  entry state go { [*] -> write ['A'] return; }
}

machine {
  tape data: wide;

  entry state s {
    [*] -> call mark(
             t = data with map { 'p' -> 'A', 'q' => 'C', 'r' => 'C', 's' => 'C' }
           ) then done;
  }
  state done { [*] -> stop; }
}
";
        assert_eq!(
            findings(src),
            vec![
                "the head may be on '_', 'C' here, outside the `enters { 'A' }` that `mark`'s tape `t` declares"
            ]
        );
    }

    /// The twin property: under a CLOSED map (the cardinalities differ) an
    /// unlisted non-blank caller glyph is a HOLE — reading it takes the
    /// `UnmappedRead` trap rather than delivering a glyph to the callee's
    /// head — so it is not reported. `'q'`, `'r'` and `'s'` are the holes
    /// here, and the calling row's cell excludes the blank, so the blank pin
    /// has nothing to decide and this fixture answers for the hole rule
    /// alone.
    ///
    /// Mutation: identity-completing an unlisted symbol regardless of the
    /// two cardinalities — `'q'` and `'r'` become the callee's `'B'` and
    /// `'C'`, and this silent fixture fires.
    #[test]
    fn a_closed_maps_holes_do_not_reach_the_callees_head() {
        let src = "\
alphabet wide  { '_', 'p', 'q', 'r', 's' }
alphabet small { '_', 'A', 'B', 'C' }

routine mark(tape t: small enters { 'A' }) {
  entry state go { [*] -> write ['A'] return; }
}

machine {
  tape data: wide;

  entry state s {
    ['p'..'s'] -> call mark(t = data with map { 'p' -> 'A' }) then done;
    [*]        -> stop;
  }
  state done { [*] -> stop; }
}
";
        assert!(findings(src).is_empty(), "{:?}", findings(src));
    }

    /// A row that MOVES the head before calling reaches the callee with the
    /// head on a cell nothing read, so the calling row's own pattern cell
    /// proves nothing about it. Here that cell would otherwise prove the
    /// site safe — `['0']` is inside the callee's declared set — and the
    /// move is the only reason the site is reported.
    ///
    /// Mutation: keeping the pattern cell's set after a move instead of
    /// widening to the whole alphabet — this fixture goes silent.
    #[test]
    fn a_row_that_moves_before_calling_is_reported() {
        let src = program(
            PLUS_ONE,
            "\
machine {
  tape data: bits;

  entry state s {
    ['0'] -> move [>] call plusOne(num = data) then done;
    [*]   -> stop;
  }
  state done { [*] -> stop; }
}
",
        );
        assert_eq!(
            findings(&src),
            vec![
                "the head may be on '_' here, outside the `enters { '0', '1' }` that `plusOne`'s tape `num` declares"
            ]
        );
        assert!(other_codes(&src).is_empty(), "{:?}", other_codes(&src));
    }

    /// A state wired as a callee's EXIT resumes with the whole alphabet:
    /// control arrives there with the head wherever the callee's own body
    /// left it, which is not what its `leaves` clause describes and what no
    /// planted check constrains.
    ///
    /// The fixture is built so the gap shows on a state the walk ALREADY
    /// examines rather than on an unreachable one: `won` is reached both by
    /// `['0'] -> goto won` (which alone would leave it holding `'0'`, inside
    /// the callee's declared set, and silent) and through `pick`'s `hit`
    /// exit, which fires from a `['_']` row. Joining the exit path is what
    /// makes the head's `'_'` visible at `won`'s own call site.
    ///
    /// Mutation: not flowing a call's exit arguments — `won` keeps only
    /// `{'0'}` and this fixture goes silent, with no unreachable state to
    /// make the loss obvious.
    #[test]
    fn a_state_wired_as_a_callees_exit_resumes_unconstrained() {
        let src = program(
            &format!(
                "{PLUS_ONE}
routine pick(tape t: bits, state hit, state miss) {{
  entry state look {{
    ['_'] -> goto hit;
    ['0'] -> goto miss;
    ['1'] -> return;
  }}
}}
"
            ),
            "\
machine {
  tape data: bits;

  entry state s {
    ['0'] -> goto won;
    [*]   -> call pick(t = data, hit = won, miss = lost) then done;
  }
  state won  { [*] -> call plusOne(num = data) then done; }
  state lost { [*] -> stop; }
  state done { [*] -> stop; }
}
",
        );
        assert_eq!(
            findings(&src),
            vec![
                "the head may be on '_' here, outside the `enters { '0', '1' }` that `plusOne`'s tape `num` declares"
            ]
        );
        assert!(other_codes(&src).is_empty(), "{:?}", other_codes(&src));
    }

    /// A GRAPH's own `enters` does not seed its body. Nothing enforces a
    /// graph's clause — no check is planted for one, and a graft edge is not
    /// a site this rule reports — so believing it would narrow the graph's
    /// body on a promise nobody is held to. A routine's clause IS believed,
    /// which the seeding fixture above pins; this is that rule's other half.
    ///
    /// Mutation: seeding a graph from its own `enters` the way a routine is
    /// seeded — this fixture goes silent.
    #[test]
    fn a_graphs_own_enters_does_not_seed_its_body() {
        let src = program(
            &format!(
                "{PLUS_ONE}
export graph bump(tape n: bits enters {{ '0', '1' }}, state done) {{
  entry state go {{ [*] -> call plusOne(num = n) then done; }}
}}
"
            ),
            "\
machine {
  tape data: bits;

  entry state s { [*] -> move [>] stop; }
}
",
        );
        assert_eq!(
            findings(&src),
            vec![
                "the head may be on '_' here, outside the `enters { '0', '1' }` that `plusOne`'s tape `num` declares"
            ]
        );
        assert!(other_codes(&src).is_empty(), "{:?}", other_codes(&src));
    }

    /// A `bind` declaration may carry the callee's EXIT wiring as well as
    /// its tape binding, and the exits must be followed from there — the
    /// bind's args ARE the site's args, one list shared by every call of
    /// that instance. The fixture is the exit fixture above with `pick`
    /// bound instead of called directly, so `won` is again reached both by
    /// a `goto` holding `{'0'}` and through the `hit` exit.
    ///
    /// Mutation: reading a bind-call's args as empty — the direct-call
    /// fixtures stay green (they resolve through the routine arm) and this
    /// one goes silent.
    #[test]
    fn a_bind_declarations_exit_wiring_is_followed_too() {
        let src = program(
            &format!(
                "{PLUS_ONE}
routine pick(tape t: bits, state hit, state miss) {{
  entry state look {{
    ['_'] -> goto hit;
    ['0'] -> goto miss;
    ['1'] -> return;
  }}
}}
"
            ),
            "\
machine {
  tape data: bits;

  bind pick(t = data, hit = won, miss = lost) as p;

  entry state s {
    ['0'] -> goto won;
    [*]   -> call p() then done;
  }
  state won  { [*] -> call plusOne(num = data) then done; }
  state lost { [*] -> stop; }
  state done { [*] -> stop; }
}
",
        );
        assert_eq!(
            findings(&src),
            vec![
                "the head may be on '_' here, outside the `enters { '0', '1' }` that `plusOne`'s tape `num` declares"
            ]
        );
        assert!(other_codes(&src).is_empty(), "{:?}", other_codes(&src));
    }

    /// A state reachable only through a `graft` is still walked, and it
    /// resumes with the WHOLE alphabet: the graft's spliced body is not here
    /// to ask where it left the head. Without the flow, every state a graft
    /// leads to would be unreachable to the analysis and its sites silently
    /// unchecked — most of a graft-driven program.
    ///
    /// The grafted graph declares `leaves { '0', '1' }`, which its own exit
    /// rows prove and which would put the head inside `plusOne`'s declared
    /// set. It is deliberately NOT read: a `leaves` clause on a graph is not
    /// enforced by any planted check, and this fixture is what makes the
    /// difference between "the whole alphabet" and "the graph's `leaves`"
    /// observable at all.
    ///
    /// Mutations, each of which makes it silent: not flowing into a reached
    /// graft's exit states (`hit` is never reached), and narrowing those
    /// exits to the grafted graph's `leaves`.
    #[test]
    fn a_state_reached_only_through_a_graft_is_still_checked() {
        let src = program(
            &format!(
                "{PLUS_ONE}
graph pick(tape n: bits leaves {{ '0', '1' }}, state yes, state no) {{
  entry state look {{
    ['1'] -> goto yes;
    ['0'] -> goto no;
  }}
}}
"
            ),
            "\
machine {
  tape data: bits;

  entry state s { [*] -> goto chose; }
  graft pick(n = data, yes = hit, no = miss) as chose;
  state hit  { [*] -> call plusOne(num = data) then done; }
  state miss { [*] -> stop; }
  state done { [*] -> stop; }
}
",
        );
        assert_eq!(
            findings(&src),
            vec![
                "the head may be on '_' here, outside the `enters { '0', '1' }` that `plusOne`'s tape `num` declares"
            ]
        );
        assert!(other_codes(&src).is_empty(), "{:?}", other_codes(&src));
    }

    /// `--allow enters-unmet` suppresses it, like every other rule.
    #[test]
    fn allow_suppresses_the_finding() {
        let src = program(
            PLUS_ONE,
            "\
machine {
  tape data: bits;

  entry state s { [*] -> call plusOne(num = data) then done; }
  state done { [*] -> stop; }
}
",
        );
        let report = lint(
            &src,
            LintOptions {
                allow: vec!["enters-unmet".to_string()],
                warn: Vec::new(),
            },
        )
        .unwrap();
        assert!(report.diagnostics.iter().all(|d| d.code != "enters-unmet"));
    }
}
