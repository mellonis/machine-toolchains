//! Open bindings written in `.tmc`: `with map { …, * }` at a call or bind
//! site, and the `opaque` bit the compiler infers for the callee's tape
//! (docs/tmt/language.md (symbol maps); docs/formats.md (bound calls)).
//!
//! The wire form already existed — a hand-written `.tma` spells both the
//! `*` in a call operand and the `opaque` suffix on a `.param` line, and
//! `crates/core/tests/link_open.rs` proves the linker's half against a
//! neutral dialect. What these tests hold is the `.tmc` END of it: that
//! the marker parses only where it means something, that the inference
//! publishes `opaque` on exactly the tapes no state discriminates, and
//! that a program written in the language lands on the same wire shape
//! the hand-written assembly does.

use mtc_core::formats::executable::Executable;
use mtc_core::formats::tapeblock::TapeSnapshot;
use mtc_core::linker::{CallMech, LinkError, LinkOptions};
use mtc_core::vm::{ArchRegistry, Machine, Outcome, RunLimits, RunOptions, Tape, WideTape};
use mtc_turing_machine::arch::Tm1;
use mtc_turing_machine::asm::link;
use mtc_turing_machine::compiler::{CompileOptions, CompileOutput, compile};

/// Compile at the default level, or panic with the diagnostic.
fn build(src: &str) -> CompileOutput {
    compile(src, CompileOptions::default()).unwrap_or_else(|e| panic!("compiles: {e:?}"))
}

/// The diagnostic CODE a refused source reports — the stable half of a
/// compile error (`compiler::code`), which is what a test should key on.
fn code(src: &str) -> &'static str {
    let err = compile(src, CompileOptions::default()).expect_err("this source must be refused");
    err.kind.code()
}

/// The whole rendered diagnostic, for the one test that reads a message.
fn message(src: &str) -> String {
    let err = compile(src, CompileOptions::default()).expect_err("this source must be refused");
    err.kind.to_string()
}

/// Link a compiled object under `mech`, returning the image or the error.
fn link_image(out: &CompileOutput, mech: CallMech) -> Result<Executable, LinkError> {
    link(
        std::slice::from_ref(&out.object),
        &[],
        LinkOptions {
            call_mech: mech,
            ..Default::default()
        },
    )
    .map(|o| o.executable)
}

/// Link a compiled object under `mech`, discarding the image.
fn link_under(out: &CompileOutput, mech: CallMech) -> Result<(), LinkError> {
    link_image(out, mech).map(|_| ())
}

/// Run a one-tape image on `seed`, head at 0, returning the outcome and
/// the final snapshot. Mirrors `tests/opt_equivalence.rs::run`.
fn run_one_tape(exe: &Executable, seed: &[u8]) -> (Outcome, TapeSnapshot) {
    assert_eq!(exe.tape_count, 1, "this harness seeds a single tape");
    let mut registry = ArchRegistry::new();
    registry.register(Box::new(Tm1::new(exe.tape_count)));
    let machine = Machine::from_executable(exe, &registry).expect("loads");
    let mut tape = WideTape::from_snapshot(
        &TapeSnapshot {
            origin: 0,
            cells: seed.to_vec(),
            head: 0,
            alphabet: None,
        },
        exe.alphabet_cardinalities[0],
    )
    .expect("the seed fits the tape width");
    let mut devices: Vec<&mut dyn Tape> = vec![&mut tape];
    let result = machine
        .run_tapes(
            &mut devices,
            RunOptions {
                limits: RunLimits {
                    max_steps: Some(100_000),
                    ..Default::default()
                },
                ..Default::default()
            },
        )
        .expect("run set-up ok");
    drop(devices);
    (result.outcome, tape.to_snapshot())
}

const MECHS: [CallMech; 3] = [CallMech::Mono, CallMech::Frames, CallMech::Hybrid];

// ── fixtures ───────────────────────────────────────────────────────────────

/// The `.tmc` twin of `tests/link_matrix.rs`'s hand-written `OPEN`: a
/// five-symbol caller band bound into a three-symbol callee whose one
/// state dispatches every glyph it knows AND carries a `*` row, so no
/// glyph is ever rejected for being unlisted. The two unlisted caller
/// glyphs (`'c'`, `'d'`) have no image in the callee's alphabet and reach
/// the `*` row as the opaque index.
const OPEN: &str = "\
alphabet five { '_', 'a', 'b', 'c', 'd' }
alphabet three { '_', 'a', 'b' }

routine swapABopen(tape n: three) {
  entry state walk {
    ['_'] -> return;
    ['a'] -> write ['b'] move [>] goto walk;
    ['b'] -> write ['a'] move [>] goto walk;
    [*]   -> move [>] goto walk;
  }
}

machine {
  tape t: five;
  entry state go {
    [*] -> call swapABopen(n = t with map { 'a' -> 'a', 'b' -> 'b', * }) then done;
  }
  state done { [*] -> stop; }
}
";

/// The near miss: a callee with TWO reading states, one carrying a `*`
/// row and one — `check` — whose rows name only concrete glyphs. It is
/// not opaque, because an opaque symbol arriving in `check` matches no
/// row at all.
///
/// Written with two states on purpose: a one-state near miss would still
/// be refused under a weakened inference that asks only whether SOME
/// reading state has a `*` cell, and so would prove nothing about the
/// rule actually implemented.
const NEAR_MISS: &str = "\
alphabet five { '_', 'a', 'b', 'c', 'd' }
alphabet three { '_', 'a', 'b' }

routine nearMiss(tape n: three) {
  entry state walk {
    ['_'] -> return;
    [*]   -> move [>] goto check;
  }
  state check {
    ['_'] -> return;
    ['a'] -> write ['b'] move [>] goto walk;
    ['b'] -> write ['a'] move [>] goto walk;
  }
}

machine {
  tape t: five;
  entry state go {
    [*] -> call nearMiss(n = t with map { 'a' -> 'a', 'b' -> 'b', * }) then done;
  }
  state done { [*] -> stop; }
}
";

// ── the grammar ────────────────────────────────────────────────────────────

/// Mutation it catches: leave the site map's pair loop as it was and the
/// marker is still `expected a glyph or number, found `*``, so nothing in
/// the language can open a binding at all.
#[test]
fn a_site_map_may_end_with_a_star() {
    let out = build(OPEN);
    assert!(
        out.tma.contains("call    swapABopen [0{1->1, 2->2, *}]"),
        "the open marker must reach the call operand:\n{}",
        out.tma
    );
}

/// `with map { * }` — the marker alone, no pairs. Mutation it catches:
/// print the braces only when the map carries pairs and a pairless open
/// map loses its marker, silently closing the binding.
#[test]
fn a_map_may_carry_the_marker_alone() {
    let src = OPEN.replace("{ 'a' -> 'a', 'b' -> 'b', * }", "{ * }");
    let out = build(&src);
    assert!(
        out.tma.contains("call    swapABopen [0{*}]"),
        "a pairless open map must still print its marker:\n{}",
        out.tma
    );
}

/// A trailing comma after the marker is accepted, exactly as it is after
/// a pair. Mutation it catches: demand `}` immediately after the `*` and
/// `{ 'a' -> 'a', *, }` stops compiling.
#[test]
fn a_trailing_comma_after_the_marker_is_accepted() {
    let src = OPEN.replace(
        "{ 'a' -> 'a', 'b' -> 'b', * }",
        "{ 'a' -> 'a', 'b' -> 'b', *, }",
    );
    let out = build(&src);
    assert!(
        out.tma.contains("call    swapABopen [0{1->1, 2->2, *}]"),
        "a trailing comma must not change what is emitted:\n{}",
        out.tma
    );
}

/// The marker is the map's LAST entry (docs/tmt/language.md (symbol
/// maps)). Mutation it catches: accept the marker anywhere in the loop
/// and `{ *, 'a' -> 'a' }` compiles, with the pairs after it reading as
/// if the map were still closed.
#[test]
fn a_marker_before_another_entry_is_refused() {
    let src = OPEN.replace("{ 'a' -> 'a', 'b' -> 'b', * }", "{ *, 'a' -> 'a' }");
    assert_eq!(code(&src), "unexpected-token");
    // The refusal names the RULE, not merely the offending token: the old
    // blanket "expected a glyph or number, found `*`" would satisfy a
    // looser assertion while saying nothing about where a marker belongs.
    assert_eq!(
        message(&src),
        "expected `}` — a map's `*` is its last entry, found glyph `a`"
    );
}

/// "Last entry" means ONE marker and at most ONE trailing comma. A second
/// marker and a second comma are both refused by the same rule, naming
/// the token that broke it — the two shapes a careless edit produces, and
/// the two the `break` after the marker means the loop never sees again.
///
/// Mutation it catches: accepting whatever follows the marker as long as
/// a `}` eventually arrives (looping instead of breaking) — `{ …, *, * }`
/// compiles with the second marker silently swallowed.
#[test]
fn a_second_marker_or_a_second_comma_is_refused() {
    let two = OPEN.replace("{ 'a' -> 'a', 'b' -> 'b', * }", "{ 'a' -> 'a', *, * }");
    assert_eq!(code(&two), "unexpected-token");
    assert_eq!(
        message(&two),
        "expected `}` — a map's `*` is its last entry, found `*`"
    );
    let comma = OPEN.replace("{ 'a' -> 'a', 'b' -> 'b', * }", "{ 'a' -> 'a', *, , }");
    assert_eq!(code(&comma), "unexpected-token");
    assert_eq!(
        message(&comma),
        "expected `}` — a map's `*` is its last entry, found `,`"
    );
}

/// A top-level `map` declaration keeps refusing the marker: its pairs are
/// discarded at the declaration and re-read at every site that names it,
/// so a marker there would be swallowed with no open binding anywhere.
/// Mutation it catches: put the marker's lookahead in the SHARED pair-body
/// production instead of the site-map arm.
#[test]
fn a_named_map_declaration_still_refuses_the_marker() {
    let src = "\
alphabet big { '_', 'a', 'b' }
alphabet small { '_', 'a', 'b' }

map m: big -> small { 'a' -> 'a', * }

machine {
  tape t: big;
  entry state s { [*] -> stop; }
}
";
    assert_eq!(code(src), "unexpected-token");
}

/// `* as v` stays the `wildcard-binding` error — the guarantee that no
/// substitution can copy the opaque index into a write. Mutation it
/// catches: reach the marker's lookahead before the pattern-cell binding
/// check and `* as v` starts parsing as an open marker.
#[test]
fn a_bound_wildcard_is_still_refused() {
    let src = "\
alphabet ab { '_', 'a', 'b' }

machine {
  tape t: ab;
  entry state s { [* as v] -> write [v] stop; }
}
";
    assert_eq!(code(src), "wildcard-binding");
}

/// A graft splices its body into the host's own tape frame: there is no
/// callee cardinality, so there is no opaque index for unlisted symbols
/// to land on. Mutation it catches: accept the marker at a graft site and
/// the composite silently drops it, holing exactly the symbols the author
/// asked to keep.
#[test]
fn an_open_map_at_a_graft_site_is_refused() {
    let src = "\
alphabet five { '_', 'a', 'b', 'c', 'd' }
alphabet three { '_', 'a', 'b' }

graph g(tape n: three, state done) {
  entry state s { [*] -> goto done; }
}

machine {
  tape t: five;
  entry graft g(n = t with map { 'a' -> 'a', * }, done = s2) as inst;
  state s2 { [*] -> stop; }
}
";
    assert_eq!(code(src), "open-graft-unsupported");
}

// ── the inference ──────────────────────────────────────────────────────────

/// The `.param` line for a named routine tape, as the generated assembly
/// carries it.
fn param_line(tma: &str, routine: &str, param: &str) -> String {
    let mut after = false;
    for line in tma.lines() {
        if line.starts_with(&format!(".routine {routine},")) {
            after = true;
            continue;
        }
        if after {
            if let Some(rest) = line.strip_prefix(&format!(".param {param},")) {
                return format!(".param {param},{rest}");
            }
            if line.starts_with(".routine") || line.starts_with(".func") {
                break;
            }
        }
    }
    panic!("no `.param {param}` under `.routine {routine}` in:\n{tma}");
}

/// A tape every reading state reads through a `*` cell is opaque, even
/// though the same state discriminates every glyph it knows — a `*` row
/// means no glyph is ever rejected for being unlisted, which is the whole
/// property. Mutation it catches: infer opacity as "no state reads this
/// tape" or as "no state discriminates its glyphs"; under either this
/// routine stops being opaque and the open binding below is refused.
#[test]
fn a_tape_read_through_a_wildcard_row_is_opaque() {
    let out = build(OPEN);
    assert_eq!(
        param_line(&out.tma, "swapABopen", "n"),
        ".param n, ('_', 'a', 'b'), writes=('a', 'b'), opaque"
    );
}

/// One reading state without a `*` cell is enough to close the tape.
/// Mutation it catches: weaken the quantifier to "SOME reading state has
/// a `*` cell" — `walk` has one, so the routine would publish `opaque`
/// and the open binding would link.
#[test]
fn one_reading_state_without_a_wildcard_closes_the_tape() {
    let out = build(NEAR_MISS);
    assert_eq!(
        param_line(&out.tma, "nearMiss", "n"),
        ".param n, ('_', 'a', 'b'), writes=('a', 'b')"
    );
}

/// A state with no rules reads nothing (it traps on entry), so a routine
/// whose every state is rowless is opaque VACUOUSLY — the universal
/// quantifier over an empty set. Recorded as behaviour, not discovered
/// later as a surprise. Mutation it catches: count a rowless state as a
/// reader (say, by asking whether the state exists rather than whether it
/// has rules) and the answer flips.
#[test]
fn a_routine_no_state_reads_is_vacuously_opaque() {
    let src = "\
alphabet three { '_', 'a', 'b' }

routine rowless(tape n: three) {
  entry state s { }
}

machine {
  tape t: three;
  entry state go { [*] -> call rowless(n = t) then done; }
  state done { [*] -> stop; }
}
";
    let out = build(src);
    assert_eq!(
        param_line(&out.tma, "rowless", "n"),
        ".param n, ('_', 'a', 'b'), opaque"
    );
}

/// The machine's own tapes publish no `opaque` bit, exactly as they
/// publish no `writes`/`enters`/`leaves`: `main` is never a callee, so
/// nothing reads its interface entry, and emitting a suffix there would
/// move the bytes of every `machine`-bearing program for no observable
/// gain. Mutation it catches: infer opacity for every world rather than
/// for routines alone — `go`'s only row is `[*]`, so `main`'s tape would
/// publish `opaque`.
#[test]
fn a_machine_tape_publishes_no_opaque_bit() {
    let out = build(OPEN);
    assert_eq!(
        param_line(&out.tma, "main", "t"),
        ".param t, ('_', 'a', 'b', 'c', 'd')"
    );
}

/// Opening a binding CLOSES its write half, whatever the cardinalities:
/// the opaque index is read-only, so the linker holes a callee symbol the
/// map does not name even where the two alphabets are the same size and a
/// closed map would have identity-completed. The caller's inferred write
/// set must say so — `inner` writes `'b'`, the map names only `'a'`, and
/// the write-back therefore delivers nothing, leaving `outer` with no
/// `writes=` suffix at all.
///
/// Mutation it catches: identity-complete an open map's unlisted symbols
/// on equal cardinalities (the closed map's rule, applied blindly) —
/// `outer` starts publishing `writes=('b')`, a promise the machine does
/// not keep.
#[test]
fn opening_a_binding_closes_its_write_half_on_equal_cardinalities() {
    let src = "\
alphabet three { '_', 'a', 'b' }

routine inner(tape n: three) {
  entry state go {
    ['a'] -> write ['b'] return;
    [*]   -> return;
  }
}

routine outer(tape t: three) {
  entry state s { [*] -> call inner(n = t with map { 'a' -> 'a', * }) then done; }
  state done { [*] -> return; }
}

machine {
  tape m: three;
  entry state g { [*] -> call outer(t = m) then f; }
  state f { [*] -> stop; }
}
";
    let out = build(src);
    assert_eq!(
        param_line(&out.tma, "outer", "t"),
        ".param t, ('_', 'a', 'b'), opaque"
    );
    // Non-vacuity: with the map CLOSED the same equal-size binding
    // identity-completes and the write does travel back, so the assertion
    // above is reading the open rule and not an empty program.
    let closed = build(&src.replace("{ 'a' -> 'a', * }", "{ 'a' -> 'a' }"));
    assert_eq!(
        param_line(&closed.tma, "outer", "t"),
        ".param t, ('_', 'a', 'b'), writes=('b'), opaque"
    );
}

// ── the linker's verdict ───────────────────────────────────────────────────

/// The whole round trip: a `.tmc` open binding into a genuinely opaque
/// callee links under every mechanism. Mutation it catches: drop the
/// `open` bit on the way to the wire (the IR field, the call operand, or
/// the `opaque` suffix) and the linker refuses what the source asked for.
#[test]
fn an_open_binding_into_an_opaque_callee_links_under_every_mechanism() {
    let out = build(OPEN);
    for mech in MECHS {
        link_under(&out, mech)
            .unwrap_or_else(|e| panic!("the open binding must link under {mech}: {e}"));
    }
}

/// What the open binding actually COMPUTES, derived before it is run.
///
/// The caller's band is `('_', 'a', 'b', 'c', 'd')` and the callee's
/// `('_', 'a', 'b')`; the map names `'a' -> 'a'` and `'b' -> 'b'` and
/// leaves `'c'` and `'d'` to the marker. Seed `'a', 'b', 'c'` (indices 1,
/// 2, 3): the callee reads `'a'`, writes `'b'` and steps right; reads
/// `'b'`, writes `'a'` and steps right; reads the opaque index — index 3,
/// the callee's cardinality, which no row names but the `*` row matches —
/// keeps the cell and steps right; reads the blank and returns. So the
/// derived final band is `'b', 'a', 'c'` (2, 1, 3), the head rests on the
/// blank at offset 3, and the run stops. The snapshot spans every cell
/// the head visited, so the blank it came to rest on is part of it: the
/// derived band is `2, 1, 3, 0`.
///
/// Mutation it catches: an opaque symbol that holes instead (the closed
/// rule) traps `unmapped-read` at the third cell rather than stopping; one
/// that identity-completes writes through the wrong glyph.
#[test]
fn the_open_bindings_run_matches_its_derivation() {
    let out = build(OPEN);
    for mech in MECHS {
        let exe = link_image(&out, mech).unwrap_or_else(|e| panic!("links under {mech}: {e}"));
        let (outcome, snap) = run_one_tape(&exe, &[1, 2, 3]);
        assert_eq!(outcome, Outcome::Stopped, "under {mech}");
        assert_eq!(snap.origin, 0, "under {mech}");
        assert_eq!(snap.cells, vec![2, 1, 3, 0], "under {mech}");
        assert_eq!(snap.head, 3, "under {mech}");
    }
}

/// The near miss: the compiler INFERS, the linker JUDGES. Nothing in the
/// compiler refuses this program — it is the link that names the callee
/// and the parameter. Mutation it catches: publish `opaque` unconditionally
/// (or from the wrong quantifier) and a routine that can only misread an
/// opaque symbol accepts one.
#[test]
fn an_open_binding_into_a_non_opaque_callee_is_refused_at_link_time() {
    let out = build(NEAR_MISS);
    for mech in MECHS {
        let err = link_under(&out, mech).expect_err("a non-opaque callee must refuse");
        let LinkError::OpenBindingUnsupported {
            callee,
            tape,
            param,
        } = &err
        else {
            panic!("expected OpenBindingUnsupported under {mech}, got {err:?}");
        };
        assert_eq!(
            (callee.as_str(), *tape, param.as_deref()),
            ("nearMiss", 0, Some("n"))
        );
    }
}

/// The same program with the marker DROPPED links cleanly — so the
/// refusal above is the open bit's doing and not something else about the
/// fixture. Non-vacuity, not a property of its own.
#[test]
fn the_near_miss_links_once_the_map_is_closed() {
    let src = NEAR_MISS.replace(
        "{ 'a' -> 'a', 'b' -> 'b', * }",
        "{ 'a' -> 'a', 'b' -> 'b' }",
    );
    let out = build(&src);
    for mech in MECHS {
        link_under(&out, mech)
            .unwrap_or_else(|e| panic!("the closed form must link under {mech}: {e}"));
    }
}
