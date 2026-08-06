//! Per-surface coverage for interior list comments: each affected list, at
//! each of the positions a comment can occupy inside it — slot 0 own-line,
//! slot 0 same-line (immediately after the opening delimiter), between two
//! entries, and the tail slot (after the last entry, before the closer) —
//! plus a LINE-vs-BLOCK check wherever the two behave differently, including
//! an own-line BLOCK cell: that axis is exactly where a renderer's inline
//! branch can silently flip a comment's `own_line` flag on reprint, which
//! [`format_checked`] below catches for every case in this file.
//!
//! The corpus guards in `fmt_tmc.rs` prove the property holds over real
//! sources; these prove WHICH surface broke when it stops holding. Every
//! defect that shipped on this branch was destruction (a comment silently
//! dropped, a `;` swallowed into a comment, or a comment silently migrating
//! into a NEIGHBORING statement) in a position nothing covered — slot 0
//! same-line for `paren_list`-based lists, slot 0 same-line for `with map`,
//! right after the `use` keyword, a trailing comment on the last `use`
//! path, and a comment written after a `use` statement's own `;`. So every
//! assertion below checks the comment SURVIVES first (`line_with` panics
//! with the whole output if it is missing — a dropped comment fails loudly,
//! not silently), checks it landed in the right place, and — via
//! [`format_checked`] — that formatting the result a second time changes
//! nothing.
//!
//! Two surfaces (`alphabet`, `use`) render a lone SAME-LINE BLOCK comment
//! inline, keeping the list on one line (an OWN-LINE block comment still
//! forces the break there too — see the `_own_line_block_comment_forces_break`
//! tests below); the five `paren_list`/`with-map` surfaces
//! (signature, graft, bind, call, with-map) currently break to multiple
//! lines whenever ANY interior comment is present, LINE or BLOCK — there is
//! no inline-with-comments branch for those renderers (`paren_list`'s
//! one-line path has no comment-interleaving code at all). That is a real
//! gap against the module doc's blanket "a `/* … */` comment does not
//! [force multi-line]" claim, which reads as covering all these lists —
//! see the crate CLAUDE.md task-6 report for the reproduction. It is NOT
//! data loss: the comment survives and rides the correct entry either way,
//! which is what the `_block_comment_survives_and_rides_its_entry` tests
//! below pin (survival + placement only, deliberately not asserting
//! one-line-vs-broken either way, so a future inline-collapse fix does not
//! fail this file).

use mtc_turing_machine::fmt::format;

/// Formats `src` and returns the line carrying `needle`, panicking with the
/// whole output when it is absent — a relocated comment is still present in
/// the text, so the useful failure names the line it landed on. Doubles as
/// the "the comment survives" check every position assertion needs first.
fn line_with<'a>(out: &'a str, needle: &str) -> &'a str {
    out.lines()
        .find(|l| l.contains(needle))
        .unwrap_or_else(|| panic!("`{needle}` missing from:\n{out}"))
}

/// The line index carrying `needle`, panicking with the whole output when it
/// is absent (same survival check as [`line_with`], returning a position
/// instead of the line itself so the caller can inspect a NEIGHBOUR line —
/// what every own-line "precedes the next thing" assertion needs).
fn index_of(out: &str, needle: &str) -> usize {
    out.lines()
        .position(|l| l.contains(needle))
        .unwrap_or_else(|| panic!("`{needle}` missing from:\n{out}"))
}

/// Formats `src`, then formats the RESULT, and asserts the two agree. Every
/// case in this file goes through this instead of calling [`format`]
/// directly, so a non-idempotent renderer (e.g. one that inlines an
/// own-line comment and flips its `own_line` flag on reprint) fails right
/// here — not only in the couple of cases that used to check by hand.
fn format_checked(src: &str) -> String {
    let out = format(src).expect("formats");
    let twice = format(&out).expect("the formatted output must still parse");
    assert_eq!(out, twice, "formatting the output is not idempotent");
    out
}

const MACHINE: &str = "machine { tape t: bits; entry state s { [*] -> stop; } }\n";

// ---------------------------------------------------------------------------
// Alphabet body.
// ---------------------------------------------------------------------------

#[test]
fn alphabet_slot0_own_line() {
    let src = format!("alphabet bits {{\n  // note\n  '_', '0', '1' }}\n\n{MACHINE}");
    let out = format_checked(&src);
    let idx = index_of(&out, "// note");
    assert!(
        out.lines().nth(idx + 1).unwrap().contains("'_'"),
        "the comment precedes the first entry, got:\n{out}"
    );
}

/// A comment on the SAME line as the opening `{`, before the first entry,
/// is captured by the alphabet's OWN `open_trailing` field rather than by
/// the interior-list bucket's slot 0 — `capture_open_trailing` greedily
/// consumes any comment still on the brace's OWN line before the list loop
/// ever runs. That is true only when the comment shares `{`'s physical
/// line; a comment on the HEADER's line, with `{` wrapped to the next line,
/// is a genuine interior slot-0 same-line comment instead — see
/// [`alphabet_header_line_comment_before_a_wrapped_brace_survives`], which
/// used to destroy it outright.
#[test]
fn alphabet_slot0_same_line_rides_the_brace_via_open_trailing() {
    let src = format!("alphabet bits {{ // note\n  '_', '0', '1' }}\n\n{MACHINE}");
    let out = format_checked(&src);
    let header = line_with(&out, "// note");
    assert!(
        header.starts_with("alphabet bits {"),
        "the comment rides the header line, got: {header:?}"
    );
    let idx = index_of(&out, "// note");
    assert!(
        out.lines().nth(idx + 1).unwrap().contains("'_'"),
        "it precedes the first entry, got:\n{out}"
    );
}

/// A comment on the header's own line, with the opening `{` WRAPPED to the
/// next line, is an interior slot-0 same-line comment — `capture_open_trailing`
/// does not see it (it only claims a comment sharing `{`'s own line). This
/// used to be destroyed outright; `render_alphabet`'s multi-line branch now
/// prints it riding the (now-inlined) opening `{`.
#[test]
fn alphabet_header_line_comment_before_a_wrapped_brace_survives() {
    let src = format!("alphabet bits // note\n{{ '_', '0', '1' }}\n\n{MACHINE}");
    let out = format_checked(&src);
    let header = line_with(&out, "// note");
    assert!(
        header.starts_with("alphabet bits {"),
        "the comment rides the header line alongside `{{`, got: {header:?}"
    );
}

#[test]
fn alphabet_between_entries() {
    let src = format!("alphabet bits {{ '_', // note\n  '0', '1' }}\n\n{MACHINE}");
    let out = format_checked(&src);
    assert!(line_with(&out, "// note").contains("'_'"));
}

#[test]
fn alphabet_tail_slot_same_line() {
    let src = format!("alphabet bits {{ '_', '0', '1' // note\n}}\n\n{MACHINE}");
    let out = format_checked(&src);
    let idx = index_of(&out, "// note");
    assert_eq!(
        out.lines().nth(idx + 1).unwrap(),
        "}",
        "it precedes the closer"
    );
}

#[test]
fn alphabet_block_comment_stays_inline() {
    let src = format!("alphabet bits {{ '_', /* x */ '0', '1' }}\n\n{MACHINE}");
    let out = format_checked(&src);
    let header = line_with(&out, "/* x */");
    assert!(
        header.contains("'_'"),
        "the block comment stays inline, got: {header:?}"
    );
}

/// An own-line BLOCK comment must force the multi-line body, not be inlined
/// onto its entry's line — inlining it would silently flip its `own_line`
/// flag true→false on reprint, a defect [`format_checked`] would catch here
/// via the idempotence check, pinned explicitly by the placement assertion
/// below too.
#[test]
fn alphabet_own_line_block_comment_forces_break() {
    let src = format!("alphabet bits {{\n  /* x */\n  '_', '0', '1' }}\n\n{MACHINE}");
    let out = format_checked(&src);
    let idx = index_of(&out, "/* x */");
    assert!(
        out.lines().nth(idx + 1).unwrap().contains("'_'"),
        "the own-line block comment precedes the first entry, got:\n{out}"
    );
}

// ---------------------------------------------------------------------------
// Signature parameter list (`routine`/`graph`), rendered by `paren_list`.
// ---------------------------------------------------------------------------

const SIG_PRELUDE_TAIL: &str = "\n\
                                 \x20 entry state g { ['_'] -> goto done; }\n\
                                 }\n\n\
                                 machine { tape m: bits; entry state s { [*] -> call walk(t = m, done = stop) then stop; } }\n";

fn sig_src(params: &str) -> String {
    format!("alphabet bits {{ '_', '0', '1' }}\n\nroutine walk({params}) {{{SIG_PRELUDE_TAIL}")
}

#[test]
fn signature_slot0_own_line() {
    let out = format_checked(&sig_src("\n  // note\n  tape t: bits, state done"));
    let idx = index_of(&out, "// note");
    assert!(
        out.lines().nth(idx + 1).unwrap().contains("tape t: bits"),
        "the comment precedes the first parameter, got:\n{out}"
    );
}

#[test]
fn signature_slot0_same_line() {
    let out = format_checked(&sig_src("// note\n  tape t: bits, state done"));
    let opener = line_with(&out, "walk(");
    assert!(
        opener.contains("// note"),
        "the comment rides the opening `(`, got: {opener:?}"
    );
}

#[test]
fn signature_between_entries() {
    let out = format_checked(&sig_src("tape t: bits, // note\n  state done"));
    assert!(line_with(&out, "// note").contains("tape t: bits"));
}

#[test]
fn signature_tail_slot_own_line() {
    let out = format_checked(&sig_src("tape t: bits, state done\n  // note\n"));
    let idx = index_of(&out, "// note");
    assert!(
        out.lines()
            .nth(idx + 1)
            .unwrap()
            .trim_start()
            .starts_with(')'),
        "it precedes the closer, got:\n{out}"
    );
}

#[test]
fn signature_block_comment_survives_and_rides_its_entry() {
    let out = format_checked(&sig_src("tape t: bits, /* mid */ state done"));
    assert!(line_with(&out, "/* mid */").contains("tape t: bits"));
}

/// An own-line BLOCK comment leading a `paren_list`-rendered signature —
/// this renderer has no "stay inline" branch, so unlike `alphabet`/`use`
/// there was never a hoisting/flip risk here, but the position is worth
/// pinning explicitly alongside the other list surfaces.
#[test]
fn signature_own_line_block_comment_forces_break() {
    let out = format_checked(&sig_src("\n  /* note */\n  tape t: bits, state done"));
    let idx = index_of(&out, "/* note */");
    assert!(
        out.lines().nth(idx + 1).unwrap().contains("tape t: bits"),
        "the own-line block comment precedes the first parameter, got:\n{out}"
    );
}

// ---------------------------------------------------------------------------
// Graft argument list, rendered by `paren_list`.
// ---------------------------------------------------------------------------

fn graft_src(params: &str) -> String {
    format!(
        "alphabet bits {{ '_', '0', '1' }}\n\n\
         graph walk(tape t: bits, state done) {{\n\
         \x20 entry state g {{ ['_'] -> goto done; }}\n\
         }}\n\n\
         machine {{\n\
         \x20 tape m: bits;\n\
         \x20 entry graft walk({params});\n\
         }}\n"
    )
}

#[test]
fn graft_slot0_own_line() {
    let out = format_checked(&graft_src("\n    // note\n    t = m, done = stop"));
    let idx = index_of(&out, "// note");
    assert!(
        out.lines().nth(idx + 1).unwrap().contains("t = m"),
        "the comment precedes the first argument, got:\n{out}"
    );
}

#[test]
fn graft_slot0_same_line() {
    let out = format_checked(&graft_src("// note\n    t = m, done = stop"));
    let opener = line_with(&out, "graft walk(");
    assert!(
        opener.contains("// note"),
        "the comment rides the opening `(`, got: {opener:?}"
    );
}

#[test]
fn graft_between_entries() {
    let out = format_checked(&graft_src("t = m, // note\n    done = stop"));
    assert!(line_with(&out, "// note").contains("t = m"));
}

#[test]
fn graft_tail_slot_own_line() {
    let out = format_checked(&graft_src("t = m, done = stop\n    // note\n  "));
    let idx = index_of(&out, "// note");
    assert!(
        out.lines()
            .nth(idx + 1)
            .unwrap()
            .trim_start()
            .starts_with(')'),
        "it precedes the closer, got:\n{out}"
    );
}

#[test]
fn graft_block_comment_survives_and_rides_its_entry() {
    let out = format_checked(&graft_src("t = m, /* mid */ done = stop"));
    assert!(line_with(&out, "/* mid */").contains("t = m"));
}

#[test]
fn graft_own_line_block_comment_forces_break() {
    let out = format_checked(&graft_src("\n    /* note */\n    t = m, done = stop"));
    let idx = index_of(&out, "/* note */");
    assert!(
        out.lines().nth(idx + 1).unwrap().contains("t = m"),
        "the own-line block comment precedes the first argument, got:\n{out}"
    );
}

// ---------------------------------------------------------------------------
// Bind argument list, rendered by `paren_list`.
// ---------------------------------------------------------------------------

fn bind_src(params: &str) -> String {
    format!(
        "alphabet bits {{ '_', '0', '1' }}\n\n\
         routine walk(tape t: bits, state done) {{\n\
         \x20 entry state g {{ ['_'] -> goto done; }}\n\
         }}\n\n\
         machine {{\n\
         \x20 tape m: bits;\n\
         \x20 entry state s {{ [*] -> goto s2; }}\n\
         \x20 state s2 {{ [*] -> stop; }}\n\
         \x20 bind walk({params}) as w;\n\
         }}\n"
    )
}

#[test]
fn bind_slot0_own_line() {
    let out = format_checked(&bind_src("\n    // note\n    t = m, done = stop"));
    let idx = index_of(&out, "// note");
    assert!(
        out.lines().nth(idx + 1).unwrap().contains("t = m"),
        "the comment precedes the first argument, got:\n{out}"
    );
}

#[test]
fn bind_slot0_same_line() {
    let out = format_checked(&bind_src("// note\n    t = m, done = stop"));
    let opener = line_with(&out, "bind walk(");
    assert!(
        opener.contains("// note"),
        "the comment rides the opening `(`, got: {opener:?}"
    );
}

#[test]
fn bind_between_entries() {
    let out = format_checked(&bind_src("t = m, // note\n    done = stop"));
    assert!(line_with(&out, "// note").contains("t = m"));
}

#[test]
fn bind_tail_slot_own_line() {
    let out = format_checked(&bind_src("t = m, done = stop\n    // note\n  "));
    let idx = index_of(&out, "// note");
    assert!(
        out.lines()
            .nth(idx + 1)
            .unwrap()
            .trim_start()
            .starts_with(')'),
        "it precedes the closer, got:\n{out}"
    );
}

#[test]
fn bind_block_comment_survives_and_rides_its_entry() {
    let out = format_checked(&bind_src("t = m, /* mid */ done = stop"));
    assert!(line_with(&out, "/* mid */").contains("t = m"));
}

#[test]
fn bind_own_line_block_comment_forces_break() {
    let out = format_checked(&bind_src("\n    /* note */\n    t = m, done = stop"));
    let idx = index_of(&out, "/* note */");
    assert!(
        out.lines().nth(idx + 1).unwrap().contains("t = m"),
        "the own-line block comment precedes the first argument, got:\n{out}"
    );
}

// ---------------------------------------------------------------------------
// Call binding list (a `call` transition's own `(…)`), rendered by
// `paren_list` via `RuleCst::call_args`.
// ---------------------------------------------------------------------------

fn call_src(params: &str) -> String {
    format!(
        "alphabet bits {{ '_', '0', '1' }}\n\n\
         routine w(tape t: bits, state d) {{ entry state g {{ ['_'] -> goto d; }} }}\n\n\
         machine {{\n\
         \x20 tape m: bits;\n\
         \x20 entry state s {{ [*] -> call w({params}) then stop; }}\n\
         }}\n"
    )
}

#[test]
fn call_slot0_own_line() {
    let out = format_checked(&call_src(
        "\n             // note\n             t = m, d = stop",
    ));
    let idx = index_of(&out, "// note");
    assert!(
        out.lines().nth(idx + 1).unwrap().contains("t = m"),
        "the comment precedes the first argument, got:\n{out}"
    );
}

#[test]
fn call_slot0_same_line() {
    let out = format_checked(&call_src("// note\n             t = m, d = stop"));
    let opener = line_with(&out, "call w(");
    assert!(
        opener.contains("// note"),
        "the comment rides the opening `(`, got: {opener:?}"
    );
}

#[test]
fn call_between_entries() {
    let out = format_checked(&call_src("t = m, // note\n             d = stop"));
    assert!(line_with(&out, "// note").contains("t = m"));
}

#[test]
fn call_tail_slot_own_line() {
    let out = format_checked(&call_src(
        "t = m, d = stop\n             // note\n           ",
    ));
    let idx = index_of(&out, "// note");
    assert!(
        out.lines()
            .nth(idx + 1)
            .unwrap()
            .trim_start()
            .starts_with(')'),
        "it precedes the closer, got:\n{out}"
    );
}

#[test]
fn call_block_comment_survives_and_rides_its_entry() {
    let out = format_checked(&call_src("t = m, /* mid */ d = stop"));
    assert!(line_with(&out, "/* mid */").contains("t = m"));
}

#[test]
fn call_own_line_block_comment_forces_break() {
    let out = format_checked(&call_src(
        "\n             /* note */\n             t = m, d = stop",
    ));
    let idx = index_of(&out, "/* note */");
    assert!(
        out.lines().nth(idx + 1).unwrap().contains("t = m"),
        "the own-line block comment precedes the first argument, got:\n{out}"
    );
}

// ---------------------------------------------------------------------------
// `with map` pair list, rendered by `sym_map_text` — one level down from a
// binding list, via a side-car keyed by the owning argument's index.
// ---------------------------------------------------------------------------

fn map_src(pairs: &str) -> String {
    format!(
        "alphabet bits {{ '_', '0', '1' }}\n\
         alphabet wide {{ '_', 'x', 'y' }}\n\n\
         routine walk(tape t: bits) {{ entry state g {{ ['_'] -> stop; }} }}\n\n\
         machine {{\n\
         \x20 tape m: wide;\n\
         \x20 entry state s {{ [*] -> call walk(t = m with map {{ {pairs} }}) then stop; }}\n\
         }}\n"
    )
}

#[test]
fn with_map_slot0_own_line() {
    let out = format_checked(&map_src(
        "\n                     // note\n                     'x' -> '0', 'y' -> '1'",
    ));
    let idx = index_of(&out, "// note");
    assert!(
        out.lines().nth(idx + 1).unwrap().contains("'x' -> '0'"),
        "the comment precedes the first pair, got:\n{out}"
    );
}

#[test]
fn with_map_slot0_same_line() {
    let out = format_checked(&map_src(
        "// note\n                     'x' -> '0', 'y' -> '1'",
    ));
    let opener = line_with(&out, "with map {");
    assert!(
        opener.contains("// note"),
        "the comment rides the opening `{{`, got: {opener:?}"
    );
}

#[test]
fn with_map_between_entries() {
    let out = format_checked(&map_src(
        "'x' -> '0', // note\n                     'y' -> '1'",
    ));
    assert!(line_with(&out, "// note").contains("'x' -> '0'"));
}

#[test]
fn with_map_tail_slot_own_line() {
    let out = format_checked(&map_src(
        "'x' -> '0', 'y' -> '1'\n                     // note\n                   ",
    ));
    let idx = index_of(&out, "// note");
    assert!(
        out.lines()
            .nth(idx + 1)
            .unwrap()
            .trim_start()
            .starts_with('}'),
        "it precedes the closer, got:\n{out}"
    );
}

#[test]
fn with_map_block_comment_survives_and_rides_its_entry() {
    let out = format_checked(&map_src("'x' -> '0', /* mid */ 'y' -> '1'"));
    assert!(line_with(&out, "/* mid */").contains("'x' -> '0'"));
}

#[test]
fn with_map_own_line_block_comment_forces_break() {
    let out = format_checked(&map_src(
        "\n                     /* note */\n                     'x' -> '0', 'y' -> '1'",
    ));
    let idx = index_of(&out, "/* note */");
    assert!(
        out.lines().nth(idx + 1).unwrap().contains("'x' -> '0'"),
        "the own-line block comment precedes the first pair, got:\n{out}"
    );
}

// ---------------------------------------------------------------------------
// `use` path list, rendered by `render_use`'s own bespoke printer (not
// `paren_list`) — the one surface where defects swallowed the terminator or
// migrated a comment into a neighboring statement, not just dropped it.
// ---------------------------------------------------------------------------

fn use_prelude() -> String {
    "alphabet bits { '_', '0', '1' }\n\n\
     namespace lib {\n\
     \x20 export routine p(tape t: bits) { entry state g { [*] -> stop; } }\n\
     \x20 export routine q(tape t: bits) { entry state g { [*] -> stop; } }\n\
     }\n\n"
        .to_string()
}

fn use_tail() -> String {
    "\n\nmachine { tape m: bits; entry state s { [*] -> call p(t = m) then stop; } }\n".to_string()
}

/// A slot-0 own-line comment must print AFTER the `use` keyword, not
/// hoisted above it — hoisting it would reorder the token stream. The old
/// check here only asserted the comment preceded the first path, which
/// holds whether the comment is correctly placed after `use` OR hoisted
/// above it; this pins the actual requirement.
#[test]
fn use_slot0_own_line() {
    let src = format!(
        "{}use\n  // note\n  lib::p, lib::q;{}",
        use_prelude(),
        use_tail()
    );
    let out = format_checked(&src);
    let use_idx = out
        .lines()
        .position(|l| l.trim_start().starts_with("use"))
        .expect("the file has a `use` line");
    let idx = index_of(&out, "// note");
    assert!(
        idx > use_idx,
        "the comment must print AFTER the `use` keyword, not hoisted above it, got:\n{out}"
    );
    assert!(
        out.lines().nth(idx + 1).unwrap().contains("lib::p"),
        "the comment precedes the first path, got:\n{out}"
    );
}

#[test]
fn use_slot0_same_line() {
    let src = format!(
        "{}use // note\n     lib::p, lib::q;{}",
        use_prelude(),
        use_tail()
    );
    let out = format_checked(&src);
    let use_line = out
        .lines()
        .find(|l| l.starts_with("use"))
        .expect("the file has a `use` line");
    assert_eq!(
        use_line, "use // note",
        "the comment rides `use`'s own line, got: {use_line:?}"
    );
}

/// Source order must survive when slot 0 carries BOTH a same-line comment
/// (right after `use`) and an own-line one (before the first path): the
/// same-line comment stays on `use`'s own line, the own-line one follows on
/// its own continuation line. Printing them in the opposite order — the
/// pre-fix behavior — would silently swap which comment reads as
/// documenting `use` itself vs. the first path.
#[test]
fn use_slot0_same_line_then_own_line_keeps_source_order() {
    let src = format!(
        "{}use // A\n    // B\n    lib::p, lib::q;{}",
        use_prelude(),
        use_tail()
    );
    let out = format_checked(&src);
    let use_line = out
        .lines()
        .find(|l| l.starts_with("use"))
        .expect("the file has a `use` line");
    assert_eq!(
        use_line, "use // A",
        "the same-line comment stays on `use`'s own line, got: {use_line:?}"
    );
    let idx_a = index_of(&out, "// A");
    let idx_b = index_of(&out, "// B");
    assert!(
        idx_a < idx_b,
        "source order must survive (A before B), got:\n{out}"
    );
    assert!(
        out.lines().nth(idx_b + 1).unwrap().contains("lib::p"),
        "`// B` precedes the first path, got:\n{out}"
    );
}

#[test]
fn use_between_entries() {
    let src = format!(
        "{}use lib::p, // note\n     lib::q;{}",
        use_prelude(),
        use_tail()
    );
    let out = format_checked(&src);
    assert!(line_with(&out, "// note").contains("lib::p"));
}

#[test]
fn use_tail_slot_own_line() {
    let src = format!(
        "{}use lib::p, lib::q\n     // note\n     ;{}",
        use_prelude(),
        use_tail()
    );
    let out = format_checked(&src);
    let idx = index_of(&out, "// note");
    let closer = out.lines().nth(idx + 1).unwrap();
    assert!(
        closer.trim() == ";",
        "it precedes the terminator, got:\n{out}"
    );
}

/// A same-line LINE comment trailing the LAST `use` path must not swallow
/// the `;` — appending it directly after the comment text would merge the
/// terminator into the comment, and the formatted output would no longer
/// parse.
#[test]
fn use_tail_slot_same_line_does_not_swallow_the_semicolon() {
    let src = format!(
        "{}use lib::p, lib::q // note\n     ;{}",
        use_prelude(),
        use_tail()
    );
    let out = format_checked(&src);
    let comment_line = line_with(&out, "// note");
    assert!(
        !comment_line.contains(';'),
        "the terminator must not merge into the LINE comment, got: {comment_line:?}"
    );
}

/// A comment written AFTER a `use` statement's own `;` must stay outside
/// that statement — it documents whatever comes NEXT, not the list that
/// just closed. The parser's `interior_comments` drain used to run once the
/// loop had already bumped past `;`, so it claimed this comment as if it
/// were the first `use`'s own tail-slot comment, migrating it into the
/// FIRST import's list instead of leaving it to document the SECOND.
#[test]
fn use_trailing_comment_does_not_migrate_into_the_next_use() {
    let src = format!(
        "{}use lib::p;\n// the fallback path\nuse lib::q;{}",
        use_prelude(),
        use_tail()
    );
    let out = format_checked(&src);
    let first_use = out
        .lines()
        .position(|l| l.trim() == "use lib::p;")
        .expect("the first `use` prints unbroken on its own line");
    let comment_idx = index_of(&out, "// the fallback path");
    assert!(
        comment_idx > first_use,
        "the comment must not be swallowed into the first `use`, got:\n{out}"
    );
    assert_eq!(
        out.lines().nth(comment_idx + 1).unwrap().trim(),
        "use lib::q;",
        "the comment must precede the SECOND `use`, got:\n{out}"
    );
}

#[test]
fn use_block_comment_stays_inline() {
    let src = format!(
        "{}use lib::p, /* mid */ lib::q;{}",
        use_prelude(),
        use_tail()
    );
    let out = format_checked(&src);
    let use_line = line_with(&out, "/* mid */");
    assert!(
        use_line.starts_with("use") && use_line.contains("lib::p"),
        "the block comment stays inline on the `use` line, got: {use_line:?}"
    );
}

/// An own-line BLOCK comment (no LINE comment anywhere in the list) must
/// still force the multi-line layout — inlining it would silently flip its
/// `own_line` flag true→false on reprint, which [`format_checked`] would
/// catch via the idempotence check; pinned explicitly here too, at the
/// position the original defect used.
#[test]
fn use_own_line_block_comment_forces_break() {
    let src = format!(
        "{}use lib::p,\n     /* mid */\n     lib::q;{}",
        use_prelude(),
        use_tail()
    );
    let out = format_checked(&src);
    let idx = index_of(&out, "/* mid */");
    assert!(
        out.lines().nth(idx + 1).unwrap().contains("lib::q"),
        "the own-line block comment precedes the following path, got:\n{out}"
    );
}
