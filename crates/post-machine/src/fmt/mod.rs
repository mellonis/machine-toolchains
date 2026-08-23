//! `.pmc` pretty-printer (`docs/pmt/fmt.md`). Thin renderer, same
//! discipline as [`crate::compile`] and [`crate::lint`]: [`format`]
//! returns a `Result` and never prints — `cli/fmt.rs` is the only place
//! that renders errors or touches the filesystem.
//!
//! The printing itself lives in [`print`], which walks the lossless
//! green syntax tree (`docs/core.md` (syntax trees)). That module is
//! where the whole canonical-form contract is written down —
//! indentation and namespace nesting, label/command-column alignment,
//! comma-group layout and its greedy-fill fallback, the blank-line
//! policy, doc and attention runs, `use` lists, and every comment
//! position the language admits. [`trivia`] holds the raw
//! sibling-token queries comment attribution is re-derived from. This
//! module is the public door and nothing else.

mod print;
mod trivia;

use crate::compiler::CompileError;

/// `.pmc` source → canonical text: a full reprint that re-derives every
/// space and newline from the tree and never changes the token stream
/// (`docs/pmt/fmt.md` (indentation)). A lex/parse error is returned as
/// `Err`, never printed (thin renderer).
pub fn format(source: &str) -> Result<String, CompileError> {
    print::format(source)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compiler::CompileErrorKind;

    #[test]
    fn empty_file_is_one_final_newline() {
        assert_eq!(format("").unwrap(), "\n");
    }

    #[test]
    fn single_unlabeled_statement() {
        assert_eq!(
            format("main() { right; }").unwrap(),
            "main() {\n    right;\n}\n"
        );
    }

    #[test]
    fn exported_function_header() {
        assert_eq!(
            format("export f() { left; }").unwrap(),
            "export f() {\n    left;\n}\n"
        );
    }

    #[test]
    fn volatile_function_header() {
        assert_eq!(
            format("volatile main() { right; }").unwrap(),
            "volatile main() {\n    right;\n}\n"
        );
    }

    #[test]
    fn volatile_export_function_header_keeps_fixed_order() {
        assert_eq!(
            format("volatile export main() { right; }").unwrap(),
            "volatile export main() {\n    right;\n}\n"
        );
    }

    #[test]
    fn comma_group_joins_on_one_line() {
        assert_eq!(
            format("f() { left, right; }").unwrap(),
            "f() {\n    left, right;\n}\n"
        );
    }

    #[test]
    fn multiple_top_level_functions_and_a_call() {
        assert_eq!(
            format("f() { right; @g(); } g() { left; }").unwrap(),
            "f() {\n    right;\n    @g();\n}\ng() {\n    left;\n}\n"
        );
    }

    #[test]
    fn renders_every_trivial_item_shape() {
        assert_eq!(
            format("f() { right(5); mark(!); @g(3); @h(!); check(1, !); goto 2; halt; debugger; }")
                .unwrap(),
            "f() {\n    right(5);\n    mark(!);\n    @g(3);\n    @h(!);\n    check(1, !);\n    goto 2;\n    halt;\n    debugger;\n}\n"
        );
    }

    #[test]
    fn empty_function_body_has_no_blank_line() {
        assert_eq!(format("f() { }").unwrap(), "f() {\n}\n");
    }

    #[test]
    fn parse_error_returns_err() {
        let e = format("f() { 1: right; 1: left; }").unwrap_err();
        assert!(matches!(e.kind, CompileErrorKind::DuplicateLabel(1)));
    }

    #[test]
    fn idempotent_on_supported_shapes() {
        for src in [
            "main() { right; }",
            "f() { right; @g(); } g() { left; }",
            "f() { left, right, mark; }",
            "volatile main() { right; }",
            "volatile export main() { right; }",
        ] {
            let once = format(src).unwrap();
            let twice = format(&once).unwrap();
            assert_eq!(twice, once, "not idempotent for {src:?}");
        }
    }

    // -- Label/command alignment ------------------------------------

    #[test]
    fn a_single_inline_label_command_column_4() {
        assert_eq!(
            format("main() { 1: right; check(1, 2); }").unwrap(),
            "main() {\n 1: right;\n    check(1, 2);\n}\n"
        );
    }

    #[test]
    fn b_widest_inline_label_pads_narrower_ones_left() {
        // `stop` in the brief's illustration isn't a real `.pmc` command;
        // substituted with `halt` (identical 4-char width, so the
        // alignment columns this test pins are unaffected).
        assert_eq!(
            format("main() { 11111: right; left; 12: halt; }").unwrap(),
            "main() {\n 11111: right;\n        left;\n    12: halt;\n}\n"
        );
    }

    #[test]
    fn c_own_line_labels_fit_and_overflow() {
        // `12:` is own-line but fits (right-aligns like an inline label);
        // `999999999:` is own-line and too long (hangs at 1 space). Both
        // commands land on the same command column (8) set by `11111:`.
        let src = "main() {\n11111: right;\n12:\nleft;\n999999999:\nhalt;\n}\n";
        assert_eq!(
            format(src).unwrap(),
            "main() {\n 11111: right;\n    12:\n        left;\n 999999999:\n        halt;\n}\n"
        );
    }

    #[test]
    fn d_stacked_labels_round_up_command_column() {
        // Prefix `1: 2:` has width 5 -> C = max(4, 7) = 7, rounded up to
        // 8 (the only multiple of 4 satisfying the round-up rule) -> a
        // 2-space left margin (8 - 1 - 5 = 2). NOTE: the task brief's
        // illustrative code block shows a single leading space (i.e. an
        // un-rounded C=7); that contradicts the brief's own stated
        // algorithm (explicitly "rounded to 8" in the same block) and the
        // mandatory P=3 unit test above (which only round-up produces).
        // Implemented per the stated round-up rule; see task-5-report.md.
        assert_eq!(
            format("main() { 1: 2: right; }").unwrap(),
            "main() {\n  1: 2: right;\n}\n"
        );
    }

    // -- Task 6: comma-group layout (Y + greedy-fill) ----------------

    #[test]
    fn e_rule_1_no_newline_fits_stays_a_single_line() {
        // Unchanged from Task 5 — no `newline_before` anywhere and the
        // one-line form fits comfortably under 80.
        assert_eq!(
            format("f() { left, right; }").unwrap(),
            "f() {\n    left, right;\n}\n"
        );
        assert_eq!(
            format("main() { left, right, mark; }").unwrap(),
            "main() {\n    left, right, mark;\n}\n"
        );
    }

    #[test]
    fn f_rule_3_preserves_the_authors_line_break() {
        // Brief's byte test: `1:` -> C=4; author put a newline before
        // `mark`, so `left, right` stays on the label's line (trailing
        // comma) and `mark` continues at the command column.
        assert_eq!(
            format("main() {\n1: left, right,\nmark;\n}").unwrap(),
            "main() {\n 1: left, right,\n    mark;\n}\n"
        );
    }

    #[test]
    fn g_rule_2_greedy_fill_breaks_after_the_last_fitting_comma() {
        // No author newline; the one-line join overflows 80. Four
        // identical 20-char calls (`@` + a 17-char name + `()`), command
        // column 4 (unlabeled `main`): one-line width = 4 + (4*20 + 3*2)
        // + 1 (`;`) = 4 + 86 + 1 = 91 > 80, so rule 2 applies.
        //
        // Hand trace (col starts at the command column, 4):
        //   call0: col = 4 + 20 = 24 (first on line, placed unconditionally)
        //   call1: 24 + 2 + 20 + 1 (reserve) = 47 <= 80 -> fits -> col = 46
        //   call2: 46 + 2 + 20 + 1 = 69 <= 80 -> fits -> col = 68
        //   call3: 68 + 2 + 20 + 1 = 91 > 80 -> breaks to the command column
        // Line 1 ends up 4 + "call, call, call," (65) = 69 chars; line 2 is
        // 4 + "call;" (21) = 25 chars — both <= 80.
        const CALL: &str = "@abcdefghijklmnopq()";
        let src =
            format!("main() {{ {CALL}, {CALL}, {CALL}, {CALL}; }} abcdefghijklmnopq() {{ halt; }}");
        let expected = format!(
            "main() {{\n    {CALL}, {CALL}, {CALL},\n    {CALL};\n}}\nabcdefghijklmnopq() {{\n    halt;\n}}\n"
        );
        let out = format(&src).unwrap();
        assert_eq!(out, expected);
        // The whole point of greedy-fill: no emitted line may exceed 80
        // (fmt IS the `line-too-long` fix — spec "Line limit").
        assert!(out.lines().all(|l| l.chars().count() <= 80));

        // Idempotent: reformatting the already-wrapped output must be a
        // no-op (the harness pins this generically; this test pins the
        // exact bytes for this specific overflow shape too).
        assert_eq!(format(&expected).unwrap(), expected);
    }

    #[test]
    fn i_greedy_fill_boundary_at_exactly_80_chars() {
        // Pins the `+ 1` reserve exactly at the 80-char edge, not just
        // gross overflow: `item0` is `@aaaaaaa()` (10 chars, name = 7
        // `a`s), command column 4 (unlabeled `main`), so col after item0
        // = 4 + 10 = 14.
        let name0 = "a".repeat(7);

        // `name1_fits` = 60 `a`s -> item1 width 63. Joined = 10+2+63=75;
        // whole line = 4 + 75 + 1 (`;`) = 80 -- exactly the limit, still
        // one line (rule 1, not rule 2).
        let name1_fits = "a".repeat(60);
        let src_fits = format!("main() {{ @{name0}(), @{name1_fits}(); }}");
        let expected_fits = format!("main() {{\n    @{name0}(), @{name1_fits}();\n}}\n");
        let out_fits = format(&src_fits).unwrap();
        assert_eq!(out_fits, expected_fits);
        assert!(out_fits.lines().all(|l| l.chars().count() <= 80));

        // One `a` longer (61 `a`s -> item1 width 64): the one-line join is
        // now 81 chars -- one over the limit -- so rule 2 wraps, breaking
        // right after item0 (item1 alone can't share the line: even by
        // itself, `col(14) + 2 + 64 = 80`, which the `< LINE_WIDTH` check
        // rejects since it must also leave room for the trailing `;`).
        let name1_overflows = "a".repeat(61);
        let src_overflow = format!("main() {{ @{name0}(), @{name1_overflows}(); }}");
        let expected_overflow =
            format!("main() {{\n    @{name0}(),\n    @{name1_overflows}();\n}}\n");
        let out_overflow = format(&src_overflow).unwrap();
        assert_eq!(out_overflow, expected_overflow);
        assert!(out_overflow.lines().all(|l| l.chars().count() <= 80));
    }

    #[test]
    fn h_rule_3_line_that_still_overflows_gets_greedy_filled_too() {
        // A preserved (author-split) group whose FIRST line alone already
        // overflows 80 falls back to rule 2's greedy-fill for THAT line
        // only; the second preserved line (`mark`) is untouched. Reusing
        // `g`'s four-call group (same arithmetic: breaks after the 3rd
        // call), followed by an author newline before `mark`.
        const CALL: &str = "@abcdefghijklmnopq()";
        let src = format!(
            "main() {{ {CALL}, {CALL}, {CALL}, {CALL},\nmark; }} abcdefghijklmnopq() {{ halt; }}"
        );
        let expected = format!(
            "main() {{\n    {CALL}, {CALL}, {CALL},\n    {CALL},\n    mark;\n}}\nabcdefghijklmnopq() {{\n    halt;\n}}\n"
        );
        let out = format(&src).unwrap();
        assert_eq!(out, expected);
        assert!(out.lines().all(|l| l.chars().count() <= 80));

        // Re-parsing the wrapped output re-derives a DIFFERENT grouping
        // (3 preserved lines instead of 2 — the greedy-fill break now
        // itself reads back as an author newline), but the rendered bytes
        // must still be stable.
        assert_eq!(format(&expected).unwrap(), expected);
    }

    #[test]
    fn idempotent_on_multi_line_groups() {
        const CALL: &str = "@abcdefghijklmnopq()";
        for src in [
            "main() {\n1: left, right,\nmark;\n}".to_string(),
            format!("main() {{ {CALL}, {CALL}, {CALL}, {CALL}; }} abcdefghijklmnopq() {{ halt; }}"),
            format!(
                "main() {{ {CALL}, {CALL}, {CALL}, {CALL},\nmark; }} abcdefghijklmnopq() {{ halt; }}"
            ),
        ] {
            let once = format(&src).unwrap();
            let twice = format(&once).unwrap();
            assert_eq!(twice, once, "not idempotent for {src:?}");
        }
    }

    #[test]
    fn m2_multiline_comment_greedy_fill_uses_last_line_width() {
        // M2 regression: a mid-comma-group BLOCK comment
        // (`CommaItem::leading`) that spans two physical source lines
        // forces the FOLLOWING item (`right`) to start a NEW group --
        // `newline_before` compares raw source LINE NUMBERS
        // (`parser.rs`: `item_start_line > last_item_end_line`), and any
        // embedded `\n` inside a leading comment necessarily advances the
        // line count between the previous item and this one, regardless
        // of source formatting. That makes the comment-prefixed `right`
        // text the `first` item of `greedy_fill_group`'s SECOND group.
        //
        // The old buggy width tracker measured `first.chars().count()`,
        // summing BOTH physical lines of the comment as if they sat on
        // one line, instead of the cursor's TRUE resulting column (the
        // width of the text AFTER the comment's closing `*/` -- its own
        // last physical line, printed verbatim with no re-indent).
        //
        // Hand trace (command_col = 4, unlabeled `main`):
        //   comment's first line: "/* " + 70 `x`s = 73 chars (77 with the
        //   4-space indent -- under 80, unaffected by this fix either
        //   way).
        //   comment's last line "y */" (4) + " right" (6) = 10 -- the
        //   TRUE column after this item (`line_width_after`, no `+
        //   command_col`: the tail line is raw, un-indented content).
        //   Old (buggy) total char count of the whole comment+item text
        //   (both lines summed, `\n` counted as 1 char): 78 (comment) + 1
        //   (space) + 5 ("right") = 84 -> buggy col = 4 + 84 = 88.
        //
        //   Next item `mark` (w = 4): buggy check `88 + 2 + 4 = 94` is
        //   NOT `< 80` -> spurious break. Corrected check `10 + 2 + 4 =
        //   16` IS `< 80` -> fits, and each following `mark` also fits
        //   (16, 22, 28, 34, 40, all < 80) -- none of the five `mark`s
        //   need to break once the width tracker is correct.
        let comment = format!("/* {}\ny */", "x".repeat(70));
        let src = format!("main() {{ left, {comment} right, mark, mark, mark, mark, mark; }}");
        let expected = format!(
            "main() {{\n    left,\n    {comment} right, mark, mark, mark, mark, mark;\n}}\n"
        );
        let out = format(&src).unwrap();
        assert_eq!(out, expected);
        // Every physical line the fixed width math produces must be
        // <= 80 (spec "Line limit" -- the whole point of greedy-fill).
        assert!(out.lines().all(|l| l.chars().count() <= 80));
        // Idempotent: reformatting the wrapped output is a no-op.
        assert_eq!(format(&expected).unwrap(), expected);
    }

    #[test]
    fn m2_control_single_line_comment_unaffected() {
        // Control for the fix above: the SAME shape, but the comment
        // collapsed to ONE physical line. No embedded `\n` -> no advance
        // in `newline_before`'s line-number comparison -> `right` stays
        // in `left`'s own group (not a fresh one), and greedy-fill's
        // width math is entirely UNTOUCHED by this fix (the
        // `text.contains('\n')` branch this fix adds is never taken; the
        // width comes out through the exact same `base + chars(text)`
        // arithmetic as before). The single long comment+item still
        // can't share a line with `left` (plain `chars().count()`, no
        // bug involved here) and lands alone on its own over-80 physical
        // line, matching `greedy_fill_group`'s already-documented
        // behavior for an over-wide item placed first on a line ("a
        // single over-wide command stays overlong -- `line-too-long`
        // lint's job, not fmt's").
        let comment = format!("/* {} y */", "x".repeat(70));
        let src = format!("main() {{ left, {comment} right, mark, mark, mark, mark, mark; }}");
        let expected = format!(
            "main() {{\n    left,\n    {comment} right,\n    mark, mark, mark, mark, mark;\n}}\n"
        );
        let out = format(&src).unwrap();
        assert_eq!(out, expected);
        assert_eq!(format(&expected).unwrap(), expected);
    }

    // -- Task 7: comments -------------------------------------------

    #[test]
    fn j_leading_comments_stay_above_the_node_at_its_indent() {
        // A run of blank_before-false own-line comments, immediately
        // above `f`, no blank anywhere — a byte-identical round trip
        // pins both content and (no) reflow.
        let src = "// leading comment stays above f at indent 0\n// a note\nf() {\n    right;\n}\n";
        assert_eq!(format(src).unwrap(), src);
    }

    #[test]
    fn k_trailing_lone_comment_gets_one_space() {
        let src = "f() {\n    right; // go\n}\n";
        assert_eq!(format(src).unwrap(), src);
    }

    #[test]
    fn l_trailing_run_aligned_in_source_is_maintained() {
        // `mark;` (code width 9 incl `;`) and `check(1, 2);` (width 16)
        // — the run's alignment column is 16 + 1 = 17, an 8-space pad
        // for `mark` and a 1-space pad for `check`, landing both `//` at
        // the same absolute source column (18). Byte-identical round
        // trip pins alignment maintenance AND idempotence together.
        let src = format!(
            "f() {{\n    mark;{}// a\n    check(1, 2); // b\n}}\n",
            " ".repeat(8)
        );
        assert_eq!(format(&src).unwrap(), src);
    }

    #[test]
    fn m_trailing_run_ragged_in_source_stays_one_space_each() {
        // Both lines have one space in source, but at DIFFERENT absolute
        // columns (`mark;` is shorter) — not author-aligned, so ragged:
        // stays one space each, unchanged.
        let src = "f() {\n    mark; // a\n    check(1, 2); // b\n}\n";
        assert_eq!(format(src).unwrap(), src);
    }

    #[test]
    fn n_dangling_comment_before_closing_brace() {
        let src = "f() {\n    right;\n    // dangling\n}\n";
        assert_eq!(format(src).unwrap(), src);
    }

    #[test]
    fn o_standalone_comment_keeps_its_blank_separation() {
        let src = "f() {\n    right;\n\n    // standalone\n\n    left;\n}\n";
        assert_eq!(format(src).unwrap(), src);
    }

    #[test]
    fn p_block_comment_interior_line_reindents_first_line_only() {
        // The comment sits flush-left in source (a leading comment for
        // `right;`, inside the body); its first line moves to the body
        // indent, but the interior line's OWN 3-space indent is untouched
        // (design doc, content fidelity: block comments never reflow).
        let src = "f() {\n/* line one\n   line two */\n    right;\n}\n";
        let expected = "f() {\n    /* line one\n   line two */\n    right;\n}\n";
        assert_eq!(format(src).unwrap(), expected);
    }

    #[test]
    fn q_mid_comma_group_block_comment_stays_inline() {
        assert_eq!(
            format("f() { 1: left, /* mid */ right; }").unwrap(),
            "f() {\n 1: left, /* mid */ right;\n}\n"
        );
    }

    #[test]
    fn r_mid_comma_group_line_comment_forces_a_break() {
        assert_eq!(
            format("f() { left, // note\nright; }").unwrap(),
            "f() {\n    left, // note\n    right;\n}\n"
        );
    }

    /// Every Task-7 source above must be idempotent — the comment-
    /// fidelity harness (`tests/fmt_programs.rs`) pins the same set at
    /// the corpus level; this pins it locally too, one failure per shape.
    #[test]
    fn idempotent_on_commented_shapes() {
        let aligned_run = format!(
            "f() {{\n    mark;{}// a\n    check(1, 2); // b\n}}\n",
            " ".repeat(8)
        );
        for src in [
            "// leading comment stays above f at indent 0\n// a note\nf() {\n    right;\n}\n"
                .to_string(),
            "f() {\n    right; // go\n}\n".to_string(),
            aligned_run,
            "f() {\n    mark; // a\n    check(1, 2); // b\n}\n".to_string(),
            "f() {\n    right;\n    // dangling\n}\n".to_string(),
            "f() {\n    right;\n\n    // standalone\n\n    left;\n}\n".to_string(),
            "f() {\n    /* line one\n   line two */\n    right;\n}\n".to_string(),
            "f() {\n 1: left, /* mid */ right;\n}\n".to_string(),
            "f() {\n    left, // note\n    right;\n}\n".to_string(),
        ] {
            let once = format(&src).unwrap();
            let twice = format(&once).unwrap();
            assert_eq!(twice, once, "not idempotent for {src:?}");
        }
    }

    // -- Task 8a: namespaces, blank lines, imports, export verbatim --

    #[test]
    fn s_namespace_prints_at_plus_one_indent() {
        // Brief §A's byte example verbatim.
        let src = "namespace ns {\n    f() {\n        right;\n    }\n}\n";
        assert_eq!(format(src).unwrap(), src);
    }

    #[test]
    fn t_namespace_body_feeds_the_deeper_base_indent_into_command_column() {
        // A namespaced function's body indent is 8 (namespace +4, function
        // +4) — `command_column` already treats this as
        // `base_body_indent`; this end-to-end test proves the recursive
        // wiring, not just the pure function (already pinned by
        // `print::tests::command_column_namespaced_base_indent`).
        // P=2 (`1:`):
        // command_column(2, 8) = max(8, 4) = 8, so the label right-aligns
        // with a 5-space margin (8 - 1 - 2) and `left;` sits at indent 8.
        let src = "namespace ns {\n    f() {\n     1: right;\n        left;\n    }\n}\n";
        assert_eq!(format(src).unwrap(), src);
    }

    #[test]
    fn namespace_nesting_recurses_at_increasing_indent() {
        // Namespaces nest — a namespace inside a namespace, proving
        // `print_items`' recursion (not just a function nested one level
        // deep, covered above).
        let src = "namespace a {\n    namespace b {\n        f() {\n            right;\n        }\n    }\n}\n";
        assert_eq!(format(src).unwrap(), src);
    }

    #[test]
    fn u_blank_line_preserved_between_declarations() {
        // Brief §B's byte example verbatim.
        let src = "f() {\n    right;\n}\n\ng() {\n    left;\n}\n";
        assert_eq!(format(src).unwrap(), src);
    }

    #[test]
    fn v_blank_line_run_collapses_to_one() {
        let src = "f() {\n    right;\n}\n\n\n\ng() {\n    left;\n}\n";
        let expected = "f() {\n    right;\n}\n\ng() {\n    left;\n}\n";
        assert_eq!(format(src).unwrap(), expected);
    }

    #[test]
    fn w_blank_line_suppressed_at_brace_edges() {
        // A blank right after `{` is suppressed (index 0 never gets a
        // blank); a blank right before `}` never reaches the CST at all
        // (no BodyItem follows the last statement to carry it) — both
        // edges land on the same one-liner.
        let src = "f() {\n\n    right;\n\n}\n";
        let expected = "f() {\n    right;\n}\n";
        assert_eq!(format(src).unwrap(), expected);
    }

    #[test]
    fn x_use_list_grouping_and_spacing() {
        // Brief §C's byte example verbatim — one `use` node per statement,
        // never split/merged; `,` tight + one space, `::` tight, ` as `
        // one space each side.
        let src = "use std::goToEnd;\nuse a, b::c as d;\n";
        assert_eq!(format(src).unwrap(), src);
    }

    #[test]
    fn y_export_keyword_printed_verbatim_when_written() {
        assert_eq!(
            format("export main() { right; }").unwrap(),
            "export main() {\n    right;\n}\n"
        );
    }

    #[test]
    fn z_bare_main_stays_bare() {
        assert_eq!(
            format("main() { right; }").unwrap(),
            "main() {\n    right;\n}\n"
        );
    }

    #[test]
    fn idempotent_on_task_8a_shapes() {
        for src in [
            "namespace ns {\n    f() {\n        right;\n    }\n}\n".to_string(),
            "namespace ns {\n    f() {\n     1: right;\n        left;\n    }\n}\n".to_string(),
            "namespace a {\n    namespace b {\n        f() {\n            right;\n        }\n    }\n}\n"
                .to_string(),
            "f() {\n    right;\n}\n\ng() {\n    left;\n}\n".to_string(),
            "f() {\n    right;\n}\n\n\n\ng() {\n    left;\n}\n".to_string(),
            "f() {\n\n    right;\n\n}\n".to_string(),
            "use std::goToEnd;\nuse a, b::c as d;\n".to_string(),
            "export main() { right; }".to_string(),
            "main() { right; }".to_string(),
        ] {
            let once = format(&src).unwrap();
            let twice = format(&once).unwrap();
            assert_eq!(twice, once, "not idempotent for {src:?}");
        }
    }

    // -- Spacing table, spaced-form normalization, hygiene, edge cases --
    //
    // The printer reads a token's VALUE off the tree, never the author's
    // original spacing or line endings, so most of the spacing table falls
    // out of the full reprint for free; these tests PIN that rather than
    // describing behaviour anything had to be written for.

    // The full intra-statement spacing table, one test per row
    // (`docs/pmt/fmt.md` (spacing)).

    #[test]
    fn spacing_table_call() {
        // `@` tight to name (grammar-level, can't even be spaced), name
        // tight to `(`, contents tight: `@f()`, `@f(5)`, `@f(!)`.
        assert_eq!(
            format("f() { @f(); @f(5); @f(!); }").unwrap(),
            "f() {\n    @f();\n    @f(5);\n    @f(!);\n}\n"
        );
    }

    #[test]
    fn spacing_table_builtin_and_successor() {
        // No space before `(`, contents tight; bare form has no parens at
        // all (`FallThrough`, grammar 0.2 forbids empty builtin `()`).
        assert_eq!(
            format("f() { left; left(5); mark(!); }").unwrap(),
            "f() {\n    left;\n    left(5);\n    mark(!);\n}\n"
        );
    }

    #[test]
    fn spacing_table_check() {
        // Tight `(`/`)`, exactly one space after the arm comma, both arm
        // shapes (label, `!`).
        assert_eq!(
            format("f() { check(1, 3); check(!, 1); }").unwrap(),
            "f() {\n    check(1, 3);\n    check(!, 1);\n}\n"
        );
    }

    #[test]
    fn spacing_table_goto() {
        assert_eq!(
            format("f() { goto 5; }").unwrap(),
            "f() {\n    goto 5;\n}\n"
        );
    }

    #[test]
    fn spacing_table_label_single_and_stacked() {
        // Single label `1:` then one space before the command (regression
        // pin, already covered structurally by Task 5's
        // `a_single_inline_label_command_column_4`, isolated here as the
        // pure spacing-table row); stacked `1: 2:` — one space between
        // the two, one space after the final colon (Task 5's
        // `d_stacked_labels_round_up_command_column` pins the same
        // bytes under its command-column-rounding framing).
        assert_eq!(
            format("f() { 1: right; }").unwrap(),
            "f() {\n 1: right;\n}\n"
        );
        assert_eq!(
            format("f() { 1: 2: right; }").unwrap(),
            "f() {\n  1: 2: right;\n}\n"
        );
    }

    #[test]
    fn spacing_table_path() {
        // `::` tight, including a 3-segment path (already-tight source —
        // confirms the canonical form is a pass-through, not just a
        // 2-segment special case).
        assert_eq!(
            format("f() { @std::api::run(); }").unwrap(),
            "f() {\n    @std::api::run();\n}\n"
        );
    }

    #[test]
    fn spacing_table_comma_and_semicolon() {
        // `,` tight to the preceding token, one space after; `;` tight to
        // the preceding token, newline after.
        assert_eq!(
            format("f() { left, right, mark; }").unwrap(),
            "f() {\n    left, right, mark;\n}\n"
        );
    }

    #[test]
    fn spacing_table_as_alias() {
        // `as` (imports): one space each side.
        assert_eq!(
            format("use their::name as alias;").unwrap(),
            "use their::name as alias;\n"
        );
    }

    #[test]
    fn spacing_table_bang() {
        // `!` tight in both positions it can appear: a call/builtin
        // successor and a `check` arm.
        assert_eq!(
            format("f() { @f(!); check(!, 1); }").unwrap(),
            "f() {\n    @f(!);\n    check(!, 1);\n}\n"
        );
    }

    // §B: spaced-form normalization — the grammar accepts extra
    // whitespace around `:` and `::`; the printer never reprints
    // interior spacing (only a number's own digits, which the CST
    // carries as written — see the module doc's "Numbers are the one
    // exception"), so these normalize to tight without any renderer
    // change (pinned, not fixed).

    #[test]
    fn spaced_label_normalizes_to_tight() {
        assert_eq!(
            format("main() { 1 : right; }").unwrap(),
            "main() {\n 1: right;\n}\n"
        );
    }

    #[test]
    fn spaced_path_normalizes_in_import_and_call() {
        assert_eq!(
            format("use std :: goToEnd;").unwrap(),
            "use std::goToEnd;\n"
        );
        assert_eq!(
            format("f() { @std :: goToEnd(); }").unwrap(),
            "f() {\n    @std::goToEnd();\n}\n"
        );
    }

    // §C: textual hygiene.

    #[test]
    fn hygiene_no_trailing_whitespace_even_when_source_has_it() {
        // Trailing whitespace on every source line — the full reprint is
        // CST-driven, not a textual copy, so none of it survives.
        let src = "f() {   \n    right;   \n}   \n";
        let out = format(src).unwrap();
        assert_eq!(out, "f() {\n    right;\n}\n");
        assert!(
            out.lines().all(|l| l == l.trim_end()),
            "trailing whitespace in {out:?}"
        );
    }

    #[test]
    fn hygiene_exactly_one_final_newline_regardless_of_trailing_blanks() {
        // A run of blank lines at the very end of the file has no
        // following item to carry `blank_before` (module doc's
        // "Blank-line presence" note) — it disappears, leaving exactly
        // the one final `\n` the last item already prints.
        assert_eq!(
            format("f() { right; }\n\n\n").unwrap(),
            "f() {\n    right;\n}\n"
        );
    }

    #[test]
    fn hygiene_crlf_and_tabs_reprint_as_lf_and_spaces() {
        // CRLF line endings and a tab-indented body — the full reprint
        // discards ALL input whitespace (indentation is fmt's own, in
        // spaces), so the only surviving shape is the parsed structure.
        let src = "f() {\r\n\tright;\r\n}\r\n";
        assert_eq!(format(src).unwrap(), "f() {\n    right;\n}\n");
    }

    // The three cases above only exercise CODE lines — a comment's own
    // text is raw lexer trivia (module doc's "Comments = trivia-tokens
    // native in the CST"), captured character-for-character from source,
    // so it carries its own trailing whitespace / CRLF independently of
    // anything the renderer decides about layout. `normalize_comment_text`
    // is the fix; these three pin it directly.

    #[test]
    fn hygiene_trailing_whitespace_stripped_from_a_trailing_comment() {
        let src = "f() {\n    right; // note   \n}\n";
        assert_eq!(format(src).unwrap(), "f() {\n    right; // note\n}\n");
    }

    #[test]
    fn hygiene_crlf_stripped_from_a_trailing_comment() {
        // CRLF puts a `\r` right before the `\n` a line comment's capture
        // loop stops at — the token's raw text ends in `\r` unless
        // normalized.
        let src = "f() {\r\n    right; // note\r\n}\r\n";
        let out = format(src).unwrap();
        assert_eq!(out, "f() {\n    right; // note\n}\n");
        assert!(!out.contains('\r'), "CR leaked into {out:?}");
    }

    #[test]
    fn hygiene_crlf_stripped_from_a_block_comment_interior() {
        // A block comment's interior line keeps its LEADING whitespace
        // verbatim (content fidelity) but must still lose a CRLF's
        // trailing `\r` — the two rules coexist because `trim_end` only
        // touches the end of each line.
        let src = "/* a\r\n b */\nf() { right; }";
        let out = format(src).unwrap();
        assert_eq!(out, "/* a\n b */\nf() {\n    right;\n}\n");
        assert!(!out.contains('\r'), "CR leaked into {out:?}");
    }

    // §D: edge cases (spec "Edge cases").

    #[test]
    fn edge_whitespace_only_file_is_one_final_newline() {
        // Complements `empty_file_is_one_final_newline` (literal `""`):
        // a file that is whitespace but has no tokens at all.
        assert_eq!(format("   \n\t\n  \n").unwrap(), "\n");
    }

    #[test]
    fn edge_comments_only_file_reprints_verbatim() {
        // No declarations at all — every item is `TopKind::Comment`;
        // reprints the comments with one final newline.
        let src = "// a\n// b\n";
        assert_eq!(format(src).unwrap(), src);
    }

    #[test]
    fn edge_empty_function_body_pin() {
        // Regression pin alongside `empty_function_body_has_no_blank_line`
        // (Task 4): header line + closing brace on its own line, no
        // blank line between.
        assert_eq!(format("f() { }").unwrap(), "f() {\n}\n");
    }

    #[test]
    fn idempotent_on_task_8b_shapes() {
        for src in [
            "f() { @f(); @f(5); @f(!); }".to_string(),
            "f() { left; left(5); mark(!); }".to_string(),
            "f() { check(1, 3); check(!, 1); }".to_string(),
            "f() { goto 5; }".to_string(),
            "f() { 1: 2: right; }".to_string(),
            "f() { @std::api::run(); }".to_string(),
            "use their::name as alias;".to_string(),
            "f() { @f(!); check(!, 1); }".to_string(),
            "main() { 1 : right; }".to_string(),
            "use std :: goToEnd;\nf() { @std :: goToEnd(); }".to_string(),
            "f() {   \n    right;   \n}   \n".to_string(),
            "f() { right; }\n\n\n".to_string(),
            "f() {\r\n\tright;\r\n}\r\n".to_string(),
            "// a\n// b\n".to_string(),
            "f() { }".to_string(),
            "f() {\n    right; // note   \n}\n".to_string(),
            "f() {\r\n    right; // note\r\n}\r\n".to_string(),
            "/* a\r\n b */\nf() { right; }".to_string(),
        ] {
            let once = format(&src).unwrap();
            let twice = format(&once).unwrap();
            assert_eq!(twice, once, "not idempotent for {src:?}");
        }
    }

    // -- Finalize: c-brace comment fix + M3 regression --------------
    //
    // §1: a comment on the SAME line as a function's opening `{` or
    // closing `}` used to be forced onto its own line (treated like an
    // ordinary leading/dangling body comment); it now stays on the brace
    // line it started on, and code after it reflows (`print_function`'s
    // c-brace doc). §2 pins the M3 path — a LINE comment leading a
    // statement's FIRST comma-group item (between the label and the
    // first command) forces the group onto multiple lines — which was
    // exercised structurally but never asserted byte-for-byte.

    #[test]
    fn cbrace_a_trailing_the_close_brace_stays_on_its_line() {
        assert_eq!(
            format("f() { right; } // t").unwrap(),
            "f() {\n    right;\n} // t\n"
        );
    }

    #[test]
    fn cbrace_b_line_comment_after_the_open_brace_stays_on_the_header() {
        assert_eq!(
            format("f() { // note\n right;\n}").unwrap(),
            "f() { // note\n    right;\n}\n"
        );
    }

    #[test]
    fn cbrace_c_block_comment_after_the_open_brace_stays_on_the_header() {
        // The block comment is BEFORE the (unlabeled) statement and
        // ADJACENT TO `{`, so it stays on the `{` line and `right` drops
        // to the body — unlike a mid-comma-group block comment (see
        // `cbrace_d_labeled_statement_block_comment_stays_inline` below),
        // there is no "stays inline" case at a brace.
        assert_eq!(
            format("f() { /* c */ right; }").unwrap(),
            "f() { /* c */\n    right;\n}\n"
        );
    }

    #[test]
    fn cbrace_d_labeled_statement_block_comment_stays_inline() {
        // The distinction the brief calls out as UNCHANGED: `/* c */`
        // here is inside the statement, after the label — not adjacent
        // to `{` (a real command, `1:`, sits between them) — so this
        // stays on the `q_mid_comma_group_block_comment_stays_inline`
        // path, not the c-brace one.
        assert_eq!(
            format("f() { 1: /* c */ right; }").unwrap(),
            "f() {\n 1: /* c */ right;\n}\n"
        );
    }

    #[test]
    fn idempotent_on_cbrace_shapes() {
        for src in [
            "f() { right; } // t".to_string(),
            "f() { // note\n right;\n}".to_string(),
            "f() { /* c */ right; }".to_string(),
            "f() { 1: /* c */ right; }".to_string(),
        ] {
            let once = format(&src).unwrap();
            let twice = format(&once).unwrap();
            assert_eq!(twice, once, "not idempotent for {src:?}");
        }
    }

    #[test]
    fn m3_item0_leading_line_comment_forces_a_comma_group_break() {
        // A comment between an own-line label's `:` and the first
        // command (`statement`'s "rare" leading trivia, `parser.rs`) —
        // the LINE comment makes `layouts[0].forced_break` true in
        // `render_items`, which `emit_forced_break` handles BEFORE the
        // main group loop (there's no preceding `,` to attach it to).
        // The label is also own-line here (`label_break`) since its
        // only inline neighbor is the comment, not a command.
        assert_eq!(
            format("f() { 1: // c\n left, right; }").unwrap(),
            "f() {\n 1:\n    // c\n    left, right;\n}\n"
        );
    }

    #[test]
    fn idempotent_on_m3_shape() {
        let src = "f() { 1: // c\n left, right; }";
        let once = format(src).unwrap();
        let twice = format(&once).unwrap();
        assert_eq!(twice, once, "not idempotent for {src:?}");
    }

    // -- Namespace c-brace fix (mirrors FunctionCst's open_trailing /
    // close_trailing onto NamespaceCst) --------------------------------
    //
    // Same gap `cbrace_a`/`cbrace_b` fixed for functions, applied to
    // `namespace NAME { … }`: a comment on the SAME line as the
    // namespace's opening `{` or closing `}` used to be forced onto its
    // own line inside/after the block; it now rides the brace line.

    #[test]
    fn ns_cbrace_a_trailing_the_close_brace_stays_on_its_line() {
        assert_eq!(
            format("namespace ns { f() { right; } } // t").unwrap(),
            "namespace ns {\n    f() {\n        right;\n    }\n} // t\n"
        );
    }

    #[test]
    fn ns_cbrace_b_line_comment_after_the_open_brace_stays_on_the_header() {
        assert_eq!(
            format("namespace ns { // note\n f() { right; }\n}").unwrap(),
            "namespace ns { // note\n    f() {\n        right;\n    }\n}\n"
        );
    }

    #[test]
    fn idempotent_on_ns_cbrace_shapes() {
        for src in [
            "namespace ns { f() { right; } } // t".to_string(),
            "namespace ns { // note\n f() { right; }\n}".to_string(),
        ] {
            let once = format(&src).unwrap();
            let twice = format(&once).unwrap();
            assert_eq!(twice, once, "not idempotent for {src:?}");
        }
    }

    #[test]
    fn ns_cbrace_c_nested_namespaces_each_close_trailing_binds_to_its_own_brace() {
        // The one interaction the flat single-namespace tests above don't
        // exercise: `close_trailing` is threaded back out of `top_items`
        // across a RECURSIVE call (the inner namespace's own `top_items`
        // call returns ITS close_trailing to the inner-namespace caller,
        // not the outer's). `// t1` must bind to `b`'s `}`, `// t2` to
        // `a`'s — never swapped or duplicated.
        assert_eq!(
            format("namespace a { namespace b { f() { right; } } // t1\n} // t2").unwrap(),
            "namespace a {\n    namespace b {\n        f() {\n            right;\n        }\n    } // t1\n} // t2\n"
        );
    }

    #[test]
    fn idempotent_on_nested_ns_cbrace_shape() {
        let src = "namespace a { namespace b { f() { right; } } // t1\n} // t2";
        let once = format(src).unwrap();
        let twice = format(&once).unwrap();
        assert_eq!(twice, once, "not idempotent for {src:?}");
    }

    // A leading-zero spelling survives outside
    // a label DEFINITION too — a CHECK ARM and a SUCCESSOR are both
    // number-carrying operands rendered through `render_check_arm`/
    // `render_successor`, the same written-text discipline pinned for
    // labels by `leading_zero_label_preserves_spelling_and_aligns_by_\
    // written_width` (crates/post-machine/tests/fmt_programs.rs). Parse
    // + format only, deliberately NOT added to `SIMPLE`
    // (crates/post-machine/tests/fmt_programs.rs) — that corpus also
    // feeds the O0/O1 execution suites, and these numbers need not
    // resolve to a real label for a syntax-only printer test.

    #[test]
    fn leading_zero_check_arm_preserves_spelling() {
        assert_eq!(
            format("f() { check(007, !); }").unwrap(),
            "f() {\n    check(007, !);\n}\n"
        );
    }

    #[test]
    fn leading_zero_successor_preserves_spelling() {
        // Both successor-bearing shapes: a builtin's explicit successor
        // (`right(008)`) and a call's successor (`@f(008)`).
        assert_eq!(
            format("f() { right(008); @f(008); }").unwrap(),
            "f() {\n    right(008);\n    @f(008);\n}\n"
        );
    }

    // -- doc/attention runs (`docs/pmt/fmt.md`, doc and attention runs) ------
    //
    // Printing rules under test: a run's own lines print immediately
    // above the bound declaration, at the declaration's OWN indent
    // (`crates/post-machine/tests/fmt_programs.rs` carries the corpus-
    // level partner fixtures — same shapes, feeding the objective-guard
    // idempotence/behaviour/comment-fidelity/token-spelling sweep).

    /// The canonical shape (also in the corpus, `fmt_programs.rs`
    /// `SIMPLE`): a two-paragraph top-level doc (an empty `?` line is the
    /// paragraph break) + a `[deprecated]` attention line with a
    /// message, then a nested function with its own one-line doc,
    /// printed at the nested (body) indent. Already fmt-clean.
    const CANONICAL_DOC_RUN: &str = "? Adds one to the accumulator, wrapping through cell 007 as a sentinel.\n?\n? Steps the head by calling the nested helper below.\n! [deprecated] use addTwoAndKeep instead\nexport addOne() {\n    ? Moves the head one cell to the right.\n    step() {\n        right;\n    }\n    @step();\n}\n";

    #[test]
    fn canonical_doc_run_top_level_and_nested_reprints_byte_identically() {
        assert_eq!(format(CANONICAL_DOC_RUN).unwrap(), CANONICAL_DOC_RUN);
    }

    #[test]
    fn scrambled_doc_run_spacing_normalizes_to_canonical() {
        // Every `?`/`!` line's canonical single space scrambled: dropped
        // where present (`?Adds…`, `![deprecated]…`), added as a bare
        // trailing space on the empty paragraph-break line (`? `) — the
        // lexer's one-leading-space-stripped rule already stores the
        // SAME text either way (`parser.rs`'s
        // `doc_run_round_trips_and_keeps_text_verbatim`), so fmt must
        // reprint the canonical single-space form regardless.
        let scrambled = "?Adds one to the accumulator, wrapping through cell 007 as a sentinel.\n? \n?Steps the head by calling the nested helper below.\n![deprecated] use addTwoAndKeep instead\nexport addOne() {\n    ?Moves the head one cell to the right.\n    step() {\n        right;\n    }\n    @step();\n}\n";
        assert_eq!(format(scrambled).unwrap(), CANONICAL_DOC_RUN);
    }

    #[test]
    fn empty_doc_line_prints_bare_sigil_as_a_paragraph_break() {
        let src = "? First paragraph.\n?\n? Second paragraph.\nmain() {\n    right;\n}\n";
        assert_eq!(format(src).unwrap(), src);
    }

    #[test]
    fn attention_bare_prose_prints_verbatim() {
        let src = "! internal use only, not part of the public surface\nmain() {\n    right;\n}\n";
        assert_eq!(format(src).unwrap(), src);
    }

    #[test]
    fn doc_run_binds_to_a_nested_function_at_its_own_indent() {
        let src =
            "main() {\n    ? step one\n    step() {\n        right;\n    }\n    @step();\n}\n";
        assert_eq!(format(src).unwrap(), src);
    }

    #[test]
    fn comment_inside_a_doc_run_prints_under_existing_comment_rules() {
        let src = "? first\n// mid comment\n? second\nmain() {\n    right;\n}\n";
        assert_eq!(format(src).unwrap(), src);
    }

    #[test]
    fn doc_run_blank_line_run_collapses_to_one() {
        // Two blank source lines between doc paragraphs (as opposed to
        // an empty `?` line, pinned separately above) collapse to one,
        // same policy as everywhere else (spec "Blank lines").
        let src = "? first\n\n\n\n? second\nmain() {\n    right;\n}\n";
        let expected = "? first\n\n? second\nmain() {\n    right;\n}\n";
        assert_eq!(format(src).unwrap(), expected);
    }

    #[test]
    fn doc_run_blank_line_before_declaration_preserved() {
        // A blank line between the run's last line and the bound
        // declaration is a DIFFERENT gap from blank lines inside the run
        // (`parser.rs`'s `doc_run` doc: the bound item's own
        // `blank_before` is repurposed for exactly this gap once a run
        // is attached) — round trips byte-identically either way.
        let src = "? doc\n\nmain() {\n    right;\n}\n";
        assert_eq!(format(src).unwrap(), src);
    }

    #[test]
    fn doc_run_blank_line_before_the_whole_run_preserved() {
        // The blank line separating a previous sibling declaration from
        // the NEXT one's run sits before the RUN, not between the run and
        // the declaration it binds to — `trivia::blank_before_unit` keys
        // off the whole unit's start, and a FUNCTION node already
        // retro-wraps its doc run, so it sees that blank.
        let src = "f() {\n    right;\n}\n\n? doc for g\ng() {\n    left;\n}\n";
        assert_eq!(format(src).unwrap(), src);
    }

    #[test]
    fn doc_run_blank_before_run_and_before_declaration_both_preserved() {
        // Both gaps (before the run, and between the run and the
        // declaration) present in the same source, independently.
        let src = "f() {\n    right;\n}\n\n? doc for g\n\ng() {\n    left;\n}\n";
        assert_eq!(format(src).unwrap(), src);
    }

    #[test]
    fn idempotent_on_doc_run_shapes() {
        for src in [
            CANONICAL_DOC_RUN.to_string(),
            "? First paragraph.\n?\n? Second paragraph.\nmain() {\n    right;\n}\n".to_string(),
            "! internal use only, not part of the public surface\nmain() {\n    right;\n}\n"
                .to_string(),
            "main() {\n    ? step one\n    step() {\n        right;\n    }\n    @step();\n}\n"
                .to_string(),
            "? first\n// mid comment\n? second\nmain() {\n    right;\n}\n".to_string(),
            "? first\n\n\n\n? second\nmain() {\n    right;\n}\n".to_string(),
            "? doc\n\nmain() {\n    right;\n}\n".to_string(),
            "f() {\n    right;\n}\n\n? doc for g\ng() {\n    left;\n}\n".to_string(),
            "f() {\n    right;\n}\n\n? doc for g\n\ng() {\n    left;\n}\n".to_string(),
        ] {
            let once = format(&src).unwrap();
            let twice = format(&once).unwrap();
            assert_eq!(twice, once, "not idempotent for {src:?}");
        }
    }
}
