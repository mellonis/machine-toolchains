//! Per-surface coverage for interior list comments: each affected list, at
//! each of the positions a comment can occupy inside it — slot 0 own-line,
//! slot 0 same-line (immediately after the opening delimiter), between two
//! entries, and the tail slot (after the last entry, before the closer) —
//! plus a LINE-vs-BLOCK check wherever the two behave differently.
//!
//! The corpus guards in `fmt_tmc.rs` prove the property holds over real
//! sources; these prove WHICH surface broke when it stops holding. Every
//! defect that shipped on this branch was destruction (a comment silently
//! dropped, or a `;` swallowed into a comment) in a position nothing
//! covered — slot 0 same-line for `paren_list`-based lists, slot 0
//! same-line for `with map`, right after the `use` keyword, and a trailing
//! comment on the last `use` path. So every assertion below checks the
//! comment SURVIVES first (`line_with` panics with the whole output if it
//! is missing — a dropped comment fails loudly, not silently) and only then
//! checks it landed in the right place.
//!
//! Two surfaces (`alphabet`, `use`) render a lone BLOCK comment inline,
//! keeping the list on one line; the five `paren_list`/`with-map` surfaces
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

const MACHINE: &str = "machine { tape t: bits; entry state s { [*] -> stop; } }\n";

// ---------------------------------------------------------------------------
// Alphabet body.
// ---------------------------------------------------------------------------

#[test]
fn alphabet_slot0_own_line() {
    let src = format!("alphabet bits {{\n  // note\n  '_', '0', '1' }}\n\n{MACHINE}");
    let out = format(&src).expect("formats");
    let idx = index_of(&out, "// note");
    assert!(
        out.lines().nth(idx + 1).unwrap().contains("'_'"),
        "the comment precedes the first entry, got:\n{out}"
    );
}

/// A comment on the SAME line as the opening `{`, before the first entry,
/// is captured by the alphabet's OWN `open_trailing` field rather than by
/// the interior-list bucket's slot 0 — `capture_open_trailing` greedily
/// consumes any comment still on the brace's line before the list loop
/// ever runs, so an interior slot-0 same-line comment is structurally
/// unreachable for this surface. Named to say so: this pins the observable
/// requirement (a comment right after the opening delimiter is not
/// destroyed), not interior-slot-0 coverage.
#[test]
fn alphabet_slot0_same_line_rides_the_brace_via_open_trailing() {
    let src = format!("alphabet bits {{ // note\n  '_', '0', '1' }}\n\n{MACHINE}");
    let out = format(&src).expect("formats");
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

#[test]
fn alphabet_between_entries() {
    let src = format!("alphabet bits {{ '_', // note\n  '0', '1' }}\n\n{MACHINE}");
    let out = format(&src).expect("formats");
    assert!(line_with(&out, "// note").contains("'_'"));
}

#[test]
fn alphabet_tail_slot_same_line() {
    let src = format!("alphabet bits {{ '_', '0', '1' // note\n}}\n\n{MACHINE}");
    let out = format(&src).expect("formats");
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
    let out = format(&src).expect("formats");
    let header = line_with(&out, "/* x */");
    assert!(
        header.contains("'_'"),
        "the block comment stays inline, got: {header:?}"
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
    let out = format(&sig_src("\n  // note\n  tape t: bits, state done")).expect("formats");
    let idx = index_of(&out, "// note");
    assert!(
        out.lines().nth(idx + 1).unwrap().contains("tape t: bits"),
        "the comment precedes the first parameter, got:\n{out}"
    );
}

#[test]
fn signature_slot0_same_line() {
    let out = format(&sig_src("// note\n  tape t: bits, state done")).expect("formats");
    let opener = line_with(&out, "walk(");
    assert!(
        opener.contains("// note"),
        "the comment rides the opening `(`, got: {opener:?}"
    );
}

#[test]
fn signature_between_entries() {
    let out = format(&sig_src("tape t: bits, // note\n  state done")).expect("formats");
    assert!(line_with(&out, "// note").contains("tape t: bits"));
}

#[test]
fn signature_tail_slot_own_line() {
    let out = format(&sig_src("tape t: bits, state done\n  // note\n")).expect("formats");
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
    let out = format(&sig_src("tape t: bits, /* mid */ state done")).expect("formats");
    assert!(line_with(&out, "/* mid */").contains("tape t: bits"));
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
    let out = format(&graft_src("\n    // note\n    t = m, done = stop")).expect("formats");
    let idx = index_of(&out, "// note");
    assert!(
        out.lines().nth(idx + 1).unwrap().contains("t = m"),
        "the comment precedes the first argument, got:\n{out}"
    );
}

#[test]
fn graft_slot0_same_line() {
    let out = format(&graft_src("// note\n    t = m, done = stop")).expect("formats");
    let opener = line_with(&out, "graft walk(");
    assert!(
        opener.contains("// note"),
        "the comment rides the opening `(`, got: {opener:?}"
    );
}

#[test]
fn graft_between_entries() {
    let out = format(&graft_src("t = m, // note\n    done = stop")).expect("formats");
    assert!(line_with(&out, "// note").contains("t = m"));
}

#[test]
fn graft_tail_slot_own_line() {
    let out = format(&graft_src("t = m, done = stop\n    // note\n  ")).expect("formats");
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
    let out = format(&graft_src("t = m, /* mid */ done = stop")).expect("formats");
    assert!(line_with(&out, "/* mid */").contains("t = m"));
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
    let out = format(&bind_src("\n    // note\n    t = m, done = stop")).expect("formats");
    let idx = index_of(&out, "// note");
    assert!(
        out.lines().nth(idx + 1).unwrap().contains("t = m"),
        "the comment precedes the first argument, got:\n{out}"
    );
}

#[test]
fn bind_slot0_same_line() {
    let out = format(&bind_src("// note\n    t = m, done = stop")).expect("formats");
    let opener = line_with(&out, "bind walk(");
    assert!(
        opener.contains("// note"),
        "the comment rides the opening `(`, got: {opener:?}"
    );
}

#[test]
fn bind_between_entries() {
    let out = format(&bind_src("t = m, // note\n    done = stop")).expect("formats");
    assert!(line_with(&out, "// note").contains("t = m"));
}

#[test]
fn bind_tail_slot_own_line() {
    let out = format(&bind_src("t = m, done = stop\n    // note\n  ")).expect("formats");
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
    let out = format(&bind_src("t = m, /* mid */ done = stop")).expect("formats");
    assert!(line_with(&out, "/* mid */").contains("t = m"));
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
    let out = format(&call_src(
        "\n             // note\n             t = m, d = stop",
    ))
    .expect("formats");
    let idx = index_of(&out, "// note");
    assert!(
        out.lines().nth(idx + 1).unwrap().contains("t = m"),
        "the comment precedes the first argument, got:\n{out}"
    );
}

#[test]
fn call_slot0_same_line() {
    let out = format(&call_src("// note\n             t = m, d = stop")).expect("formats");
    let opener = line_with(&out, "call w(");
    assert!(
        opener.contains("// note"),
        "the comment rides the opening `(`, got: {opener:?}"
    );
}

#[test]
fn call_between_entries() {
    let out = format(&call_src("t = m, // note\n             d = stop")).expect("formats");
    assert!(line_with(&out, "// note").contains("t = m"));
}

#[test]
fn call_tail_slot_own_line() {
    let out = format(&call_src(
        "t = m, d = stop\n             // note\n           ",
    ))
    .expect("formats");
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
    let out = format(&call_src("t = m, /* mid */ d = stop")).expect("formats");
    assert!(line_with(&out, "/* mid */").contains("t = m"));
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
    let out = format(&map_src(
        "\n                     // note\n                     'x' -> '0', 'y' -> '1'",
    ))
    .expect("formats");
    let idx = index_of(&out, "// note");
    assert!(
        out.lines().nth(idx + 1).unwrap().contains("'x' -> '0'"),
        "the comment precedes the first pair, got:\n{out}"
    );
}

#[test]
fn with_map_slot0_same_line() {
    let out = format(&map_src(
        "// note\n                     'x' -> '0', 'y' -> '1'",
    ))
    .expect("formats");
    let opener = line_with(&out, "with map {");
    assert!(
        opener.contains("// note"),
        "the comment rides the opening `{{`, got: {opener:?}"
    );
}

#[test]
fn with_map_between_entries() {
    let out = format(&map_src(
        "'x' -> '0', // note\n                     'y' -> '1'",
    ))
    .expect("formats");
    assert!(line_with(&out, "// note").contains("'x' -> '0'"));
}

#[test]
fn with_map_tail_slot_own_line() {
    let out = format(&map_src(
        "'x' -> '0', 'y' -> '1'\n                     // note\n                   ",
    ))
    .expect("formats");
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
    let out = format(&map_src("'x' -> '0', /* mid */ 'y' -> '1'")).expect("formats");
    assert!(line_with(&out, "/* mid */").contains("'x' -> '0'"));
}

// ---------------------------------------------------------------------------
// `use` path list, rendered by `render_use`'s own bespoke printer (not
// `paren_list`) — the one surface where a defect swallowed the terminator,
// not just the comment.
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

#[test]
fn use_slot0_own_line() {
    let src = format!(
        "{}use\n  // note\n  lib::p, lib::q;{}",
        use_prelude(),
        use_tail()
    );
    let out = format(&src).expect("formats");
    let idx = index_of(&out, "// note");
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
    let out = format(&src).expect("formats");
    let use_line = out
        .lines()
        .find(|l| l.starts_with("use"))
        .expect("the file has a `use` line");
    assert_eq!(
        use_line, "use // note",
        "the comment rides `use`'s own line, got: {use_line:?}"
    );
}

#[test]
fn use_between_entries() {
    let src = format!(
        "{}use lib::p, // note\n     lib::q;{}",
        use_prelude(),
        use_tail()
    );
    let out = format(&src).expect("formats");
    assert!(line_with(&out, "// note").contains("lib::p"));
}

#[test]
fn use_tail_slot_own_line() {
    let src = format!(
        "{}use lib::p, lib::q\n     // note\n     ;{}",
        use_prelude(),
        use_tail()
    );
    let out = format(&src).expect("formats");
    let idx = index_of(&out, "// note");
    let closer = out.lines().nth(idx + 1).unwrap();
    assert!(
        closer.trim() == ";",
        "it precedes the terminator, got:\n{out}"
    );
    let twice = format(&out).expect("the formatted output re-formats");
    assert_eq!(out, twice, "formatting the output is not idempotent");
}

/// The fourth of the four defects this branch shipped and fixed: a same-line
/// LINE comment trailing the LAST `use` path must not swallow the `;` —
/// appending it directly after the comment text would merge the terminator
/// into the comment, and the formatted output would no longer parse.
#[test]
fn use_tail_slot_same_line_does_not_swallow_the_semicolon() {
    let src = format!(
        "{}use lib::p, lib::q // note\n     ;{}",
        use_prelude(),
        use_tail()
    );
    let out = format(&src).expect("formats");
    let comment_line = line_with(&out, "// note");
    assert!(
        !comment_line.contains(';'),
        "the terminator must not merge into the LINE comment, got: {comment_line:?}"
    );
    // The strongest check: a corrupted terminator makes the output
    // unparseable, so re-formatting it would fail outright.
    let twice = format(&out).expect("the formatted output must still parse");
    assert_eq!(out, twice, "formatting the output is not idempotent");
}

#[test]
fn use_block_comment_stays_inline() {
    let src = format!(
        "{}use lib::p, /* mid */ lib::q;{}",
        use_prelude(),
        use_tail()
    );
    let out = format(&src).expect("formats");
    let use_line = line_with(&out, "/* mid */");
    assert!(
        use_line.starts_with("use") && use_line.contains("lib::p"),
        "the block comment stays inline on the `use` line, got: {use_line:?}"
    );
}
