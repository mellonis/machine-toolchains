//! The `.tmc` service battery. Every test drives the real service
//! IN-PROCESS through the `LanguageService` trait — `did_update` first (the
//! framework's own order), then the request under test — so what is
//! exercised is exactly what the server loop will call, with no transport
//! in the way.

use std::fs;
use std::sync::atomic::{AtomicU64, Ordering};

use mtc_core::diagnostics::{Edit, Fix};
use serde_json::json;

use super::*;

/// A fresh scratch directory under `std::env::temp_dir()`, unique per call
/// (process id + an atomic counter — this crate has no tempfile
/// dependency, matching the zero-new-deps constraint).
fn unique_tmp_dir(label: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("tmt-lsp-test-{label}-{}-{n}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn file_uri(path: &Path) -> String {
    format!("file://{}", path.display())
}

/// 1-based (line, col) of the first byte of `anchor`'s occurrence in `src`,
/// plus a `skip` char offset into the anchor.
fn pos_after(src: &str, anchor: &str, skip: usize) -> Pos {
    let start = src
        .find(anchor)
        .unwrap_or_else(|| panic!("{anchor:?} not found in fixture"));
    pos_at_byte(src, start + skip)
}

fn pos_at_byte(src: &str, byte_idx: usize) -> Pos {
    let prefix = &src[..byte_idx];
    let line = prefix.matches('\n').count() as u32 + 1;
    let col = match prefix.rfind('\n') {
        Some(nl) => prefix[nl + 1..].chars().count() as u32 + 1,
        None => prefix.chars().count() as u32 + 1,
    };
    Pos { line, col }
}

fn span_of(src: &str, anchor: &str) -> Span {
    span_of_nth(src, anchor, 0)
}

/// The span of `anchor`'s `n`th occurrence (0-based) — how a test names
/// the DECLARATION of a name that is also referenced earlier in the file.
fn span_of_nth(src: &str, anchor: &str, n: usize) -> Span {
    let start = src
        .match_indices(anchor)
        .nth(n)
        .unwrap_or_else(|| panic!("{anchor:?} occurrence {n} not found in fixture"))
        .0;
    let start = pos_at_byte(src, start);
    Span::new(
        start.line,
        start.col,
        start.line,
        start.col + anchor.chars().count() as u32,
    )
}

/// A service with one open document at an `untitled:` URI (no filesystem,
/// so no `tmt.json` discovery can interfere).
fn opened(src: &str) -> (TmcLanguageService, String) {
    let mut service = TmcLanguageService::new();
    let uri = "untitled:doc.tmc".to_string();
    service.did_update(&uri, src);
    (service, uri)
}

/// Applies one edit to `src`, so an assertion can be about the TEXT the
/// fix produces rather than about coordinates.
fn apply(src: &str, edit: &Edit) -> String {
    let byte_of = |pos: Pos| {
        let mut line = 1;
        let mut col = 1;
        for (i, c) in src.char_indices() {
            if line == pos.line && col == pos.col {
                return i;
            }
            if c == '\n' {
                line += 1;
                col = 1;
            } else {
                col += 1;
            }
        }
        src.len()
    };
    let (start, end) = (byte_of(edit.span.start), byte_of(edit.span.end));
    format!("{}{}{}", &src[..start], edit.replacement, &src[end..])
}

fn labels(candidates: &[Candidate]) -> Vec<String> {
    candidates.iter().map(|c| c.label.clone()).collect()
}

/// A two-tape machine over two DIFFERENT alphabets — the fixture the cell
/// contexts need, since a per-cell alphabet is only observably per-cell
/// when the tapes disagree.
const TWO_TAPE: &str = "\
alphabet bits { '_', '0', '1' }
alphabet wide { '_', 'a', 'b' }

machine {
  tape ctl: bits;
  tape data: wide;

  entry state main {
    ['1', *] -> write ['0', 'a'] move [>, .] goto done;
    [*, *] -> stop;
  }

  state done { [*, *] -> stop; }
}
";

/// A library + program with a namespaced routine, an import, a call with a
/// binding map, and a graft — one fixture covering every cross-world
/// reference navigation and completion has to resolve.
const CROSS_WORLD: &str = "\
alphabet bits { '_', '0', '1' }
alphabet wide { '_', 'a', 'b', '0', '1' }

namespace mylib {
?Adds one to the number under the head.
! [deprecated] use plusTwo instead.
  export routine plusOne(tape num: bits) {
    entry state inc {
      ['1'] -> write ['0'] move [<] goto inc;
      [*] -> write ['1'] return;
    }
  }
}

export graph findX(tape t: wide, state found) {
  entry state walk {
    ['a'] -> found;
    [*] -> move [>] goto walk;
  }
}

use mylib::plusOne;

machine {
  tape ctl: bits;
  tape data: wide;

  bind plusOne(num = ctl) as inc1;

  entry state main {
    ['1', *] -> call plusOne(num = ctl) then done;
    [*, *] -> call inc1() then done;
  }

  graft findX(t = data, found = done) as seek;

  state done { [*, *] -> stop; }
}
";

// -- diagnostics ---------------------------------------------------------

#[test]
fn a_clean_document_reports_nothing() {
    let (mut service, uri) = opened(TWO_TAPE);
    assert!(service.did_update(&uri, TWO_TAPE).is_empty());
}

#[test]
fn a_lex_failure_reports_one_error_and_keeps_no_stage() {
    let src = "alphabet a { '_' } /* never closed";
    let mut service = TmcLanguageService::new();
    let diagnostics = service.did_update("untitled:x.tmc", src);
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].severity, ServiceSeverity::Error);
    let state = &service.docs["untitled:x.tmc"];
    assert!(state.tokens.is_none());
    assert!(state.cst.is_none());
}

#[test]
fn a_parse_failure_keeps_the_tokens_and_reports_one_error() {
    let mut service = TmcLanguageService::new();
    let diagnostics = service.did_update("untitled:x.tmc", "machine {");
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].severity, ServiceSeverity::Error);
    let state = &service.docs["untitled:x.tmc"];
    assert!(state.tokens.is_some());
    assert!(state.cst.is_none());
    assert!(state.program.is_none());
}

#[test]
fn a_resolve_failure_keeps_the_program_and_reports_one_error() {
    let src = "\
alphabet bits { '_', '1' }
machine {
  tape t: bits;
  entry state s { [*] -> goto nowhere; }
}
";
    let mut service = TmcLanguageService::new();
    let diagnostics = service.did_update("untitled:x.tmc", src);
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].code, Some("undefined-state"));
    let state = &service.docs["untitled:x.tmc"];
    assert!(state.cst.is_some());
    assert!(state.program.is_some());
    assert!(state.resolved.is_none());
    // The staged seam raises its non-fatal findings only after the whole
    // resolve stage completes, so a document that fatals inside it shows
    // the fatal alone — never the warnings its unaffected declarations
    // would have produced. Pinned here so the behaviour is a decision
    // rather than a surprise.
    assert!(state.warnings.is_empty());
    assert!(state.lint.is_none());
}

#[test]
fn an_expansion_failure_reports_the_error_and_keeps_the_lint_channel() {
    // The binding-map legality rules run past the staged seam. The service
    // runs that stage too, so the error surfaces — and because resolution
    // DID complete, the hygiene findings stay valid and stay visible.
    let src = "\
alphabet marks { '_', 'x', 'y' }
alphabet other { '_', 'q' }

graph findX(tape t: marks, state found) {
  entry state walk {
    ['x'] -> found;
    [*] -> move [>] goto walk;
  }
}

machine {
  tape work: other;
  entry graft findX(t = work, found = done) as seek;
  state done { [*] -> debugger stop; }
}
";
    let (mut service, uri) = opened(src);
    let diagnostics = service.did_update(&uri, src);
    let codes: Vec<_> = diagnostics.iter().map(|d| d.code).collect();
    assert!(
        codes.contains(&Some("identity-glyph-mismatch")),
        "{codes:?}"
    );
    assert!(codes.contains(&Some("leftover-debugger")), "{codes:?}");
}

#[test]
fn lint_findings_ride_the_lint_channel_and_sort_with_the_warnings() {
    let src = "\
alphabet bits { '_', '1' }
machine {
  tape t: bits;
  entry state s { [*] -> debugger stop; }
}
";
    let (mut service, uri) = opened(src);
    let diagnostics = service.did_update(&uri, src);
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].source, "tmt lint");
    assert_eq!(diagnostics[0].code, Some("leftover-debugger"));
    assert_eq!(diagnostics[0].severity, ServiceSeverity::Warning);
}

#[test]
fn did_close_forgets_the_document() {
    let (mut service, uri) = opened(TWO_TAPE);
    service.did_close(&uri);
    assert!(service.docs.is_empty());
    assert!(service.completion(&uri, Pos { line: 1, col: 1 }).is_empty());
    assert!(service.definition(&uri, Pos { line: 1, col: 1 }).is_none());
}

// -- configuration -------------------------------------------------------

#[test]
fn a_project_file_suppresses_a_rule_and_the_ide_channel_unions_with_it() {
    let dir = unique_tmp_dir("config");
    let src = "\
alphabet bits { '_', '1' }
machine {
  tape t: bits;
  entry state s { [*] -> debugger stop; }
}
";
    let doc = dir.join("m.tmc");
    fs::write(&doc, src).unwrap();
    fs::write(
        dir.join("tmt.json"),
        r#"{"lint": {"allow": ["leftover-debugger"]}}"#,
    )
    .unwrap();

    let mut service = TmcLanguageService::new();
    let uri = file_uri(&doc);
    assert!(service.did_update(&uri, src).is_empty());

    // The IDE channel unions in, never cascades over: an allow from
    // either source suppresses.
    service.did_change_config(json!({"lint": {"allow": ["dead-rule"]}}));
    assert!(service.did_update(&uri, src).is_empty());
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn an_unknown_ide_rule_code_becomes_an_invalid_config_warning() {
    let (mut service, uri) = opened(TWO_TAPE);
    service.did_change_config(json!({"tmt": {"lint": {"allow": ["no-such-rule"]}}}));
    let diagnostics = service.did_update(&uri, TWO_TAPE);
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].code, Some("invalid-config"));
    assert!(diagnostics[0].message.contains("no-such-rule"));
}

#[test]
fn the_ide_warn_channel_turns_an_opt_in_rule_on() {
    let src = "\
alphabet bits { '_', '1' }
machine {
  tape t: bits;
  entry state s { ['1'] -> stop; }
}
";
    let (mut service, uri) = opened(src);
    assert!(service.did_update(&uri, src).is_empty());
    service.did_change_config(json!({"lint": {"warn": ["state-may-trap"]}}));
    let codes: Vec<_> = service
        .did_update(&uri, src)
        .iter()
        .map(|d| d.code)
        .collect();
    assert!(codes.contains(&Some("state-may-trap")), "{codes:?}");
}

// -- completions ---------------------------------------------------------

/// Completion at the seam of `prefix + suffix`, after the document has
/// SETTLED as `prefix + settled + suffix`.
///
/// This is the real editor sequence, and the only one that exercises the
/// service honestly: a document that resolved, then an edit that broke it
/// (a half-typed cell rarely parses), then a completion request. The names
/// come from the roster the settled text left behind; the position comes
/// from the broken text's own tokens.
fn complete_typing(prefix: &str, settled: &str, suffix: &str) -> Vec<Candidate> {
    let mut service = TmcLanguageService::new();
    let uri = "untitled:doc.tmc".to_string();
    service.did_update(&uri, &format!("{prefix}{settled}{suffix}"));
    let src = format!("{prefix}{suffix}");
    let pos = pos_at_byte(&src, prefix.len());
    service.did_update(&uri, &src);
    service.completion(&uri, pos)
}

/// Completion in a document that needs no repair — `prefix + suffix` is
/// already valid, so the roster is the current one.
fn complete_between(prefix: &str, suffix: &str) -> Vec<Candidate> {
    let src = format!("{prefix}{suffix}");
    let pos = pos_at_byte(&src, prefix.len());
    let (mut service, uri) = opened(&src);
    service.completion(&uri, pos)
}

#[test]
fn a_pattern_cell_offers_the_alphabet_of_the_tape_at_that_position() {
    let head = "\
alphabet bits { '_', '0', '1' }
alphabet wide { '_', 'a', 'b' }

machine {
  tape ctl: bits;
  tape data: wide;

  entry state main {
    [";
    let tail = "] -> stop;\n  }\n}\n";

    // Cell 0 draws from `ctl`'s alphabet…
    let first = labels(&complete_typing(head, "*, *", tail));
    assert!(first.contains(&"'0'".to_string()), "{first:?}");
    assert!(first.contains(&"'1'".to_string()), "{first:?}");
    assert!(!first.contains(&"'a'".to_string()), "{first:?}");
    assert!(first.contains(&"*".to_string()), "{first:?}");

    // …and cell 1 from `data`'s, which is the whole point.
    let second = labels(&complete_typing(&format!("{head}'0', "), "*", tail));
    assert!(second.contains(&"'a'".to_string()), "{second:?}");
    assert!(second.contains(&"'b'".to_string()), "{second:?}");
    assert!(!second.contains(&"'1'".to_string()), "{second:?}");
}

#[test]
fn a_write_cell_offers_the_same_alphabet_plus_the_keep_marker() {
    let head = "\
alphabet bits { '_', '0', '1' }
alphabet wide { '_', 'a', 'b' }

machine {
  tape ctl: bits;
  tape data: wide;

  entry state main {
    [*, *] -> write ['0', ";
    let tail = "] stop;\n  }\n}\n";
    let got = labels(&complete_typing(head, "'a'", tail));
    assert!(got.contains(&"-".to_string()), "{got:?}");
    assert!(got.contains(&"'a'".to_string()), "{got:?}");
    assert!(!got.contains(&"'1'".to_string()), "{got:?}");
}

#[test]
fn a_move_cell_offers_the_three_directions_and_no_glyphs() {
    let head = "\
alphabet bits { '_', '0', '1' }

machine {
  tape ctl: bits;

  entry state main {
    [*] -> move [";
    let tail = "] stop;\n  }\n}\n";
    let got = labels(&complete_typing(head, ".", tail));
    assert_eq!(got, vec!["<", ">", "."]);
}

#[test]
fn a_goto_offers_the_worlds_states_graft_instances_and_state_params() {
    let head = "\
alphabet bits { '_', '1' }

graph g(tape t: bits, state done) {
  entry state walk { [*] -> stop; }
}

machine {
  tape ctl: bits;
  graft g(t = ctl, done = fin) as seek;
  entry state main { [*] -> goto ";
    let tail = ";\n  }\n  state fin { [*] -> stop; }\n}\n";
    let got = labels(&complete_typing(head, "fin", tail));
    assert!(got.contains(&"main".to_string()), "{got:?}");
    assert!(got.contains(&"fin".to_string()), "{got:?}");
    assert!(got.contains(&"seek".to_string()), "{got:?}");
}

#[test]
fn a_call_target_offers_routines_and_bind_instances_but_not_graphs() {
    let head = "\
alphabet bits { '_', '1' }

routine r(tape t: bits) { entry state s { [*] -> return; } }
graph g(tape t: bits, state done) { entry state s { [*] -> done; } }

machine {
  tape ctl: bits;
  bind r(t = ctl) as r1;
  entry state main { [*] -> call ";
    let tail = "() then main;\n  }\n}\n";
    let got = labels(&complete_typing(head, "r1", tail));
    assert!(got.contains(&"r".to_string()), "{got:?}");
    assert!(got.contains(&"r1".to_string()), "{got:?}");
    assert!(!got.contains(&"g".to_string()), "{got:?}");
}

#[test]
fn a_graft_target_offers_graphs_only() {
    let head = "\
alphabet bits { '_', '1' }

routine r(tape t: bits) { entry state s { [*] -> return; } }
graph g(tape t: bits, state done) { entry state s { [*] -> done; } }

machine {
  tape ctl: bits;
  entry graft ";
    let tail = "(t = ctl, done = fin) as seek;\n  state fin { [*] -> stop; }\n}\n";
    let got = labels(&complete_typing(head, "g", tail));
    assert!(got.contains(&"g".to_string()), "{got:?}");
    assert!(!got.contains(&"r".to_string()), "{got:?}");
}

#[test]
fn a_binding_argument_offers_the_targets_parameter_names_then_its_own_values() {
    let head = "\
alphabet bits { '_', '1' }

graph g(tape t: bits, state done) { entry state s { [*] -> done; } }

machine {
  tape ctl: bits;
  entry graft g(";
    let tail = ") as seek;\n  state fin { [*] -> stop; }\n}\n";
    let names = labels(&complete_typing(head, "t = ctl, done = fin", tail));
    assert!(names.contains(&"t".to_string()), "{names:?}");
    assert!(names.contains(&"done".to_string()), "{names:?}");

    let values = labels(&complete_typing(
        &format!("{head}t = "),
        "ctl, done = fin",
        tail,
    ));
    assert!(values.contains(&"ctl".to_string()), "{values:?}");
    assert!(values.contains(&"fin".to_string()), "{values:?}");
}

#[test]
fn a_map_pair_offers_the_host_alphabet_left_and_the_callee_alphabet_right() {
    let head = "\
alphabet bits { '_', '0', '1' }
alphabet wide { '_', 'a', 'b', 'c' }

routine r(tape num: bits) { entry state s { [*] -> return; } }

machine {
  tape data: wide;
  entry state main { [*] -> call r(num = data with map { ";
    let tail = " }) then main;\n  }\n}\n";

    // Left of the arrow: the HOST tape's alphabet (`data`, wide).
    let src_side = labels(&complete_typing(head, "'a' -> '1'", tail));
    assert!(src_side.contains(&"'a'".to_string()), "{src_side:?}");
    assert!(!src_side.contains(&"'1'".to_string()), "{src_side:?}");

    // Right of it: the CALLEE tape parameter's alphabet (`num`, bits).
    let dst_side = labels(&complete_typing(&format!("{head}'a' -> "), "'1'", tail));
    assert!(dst_side.contains(&"'1'".to_string()), "{dst_side:?}");
    assert!(!dst_side.contains(&"'a'".to_string()), "{dst_side:?}");
}

#[test]
fn a_tape_declaration_offers_alphabet_names() {
    let head = "\
alphabet bits { '_', '1' }
alphabet wide { '_', 'a' }

machine {
  tape ctl: ";
    let tail = ";\n  entry state main { [*] -> stop; }\n}\n";
    let got = labels(&complete_typing(head, "bits", tail));
    assert!(got.contains(&"bits".to_string()), "{got:?}");
    assert!(got.contains(&"wide".to_string()), "{got:?}");
}

#[test]
fn item_boundaries_offer_the_keywords_of_the_enclosing_block() {
    let top = labels(&complete_between("", "\n"));
    assert!(top.contains(&"machine".to_string()), "{top:?}");
    assert!(top.contains(&"alphabet".to_string()), "{top:?}");
    assert!(!top.contains(&"state".to_string()), "{top:?}");

    let head = "\
alphabet bits { '_', '1' }

machine {
  tape ctl: bits;
  ";
    let inside = labels(&complete_between(
        head,
        "\n  entry state s { [*] -> stop; }\n}\n",
    ));
    assert!(inside.contains(&"state".to_string()), "{inside:?}");
    assert!(inside.contains(&"tape".to_string()), "{inside:?}");
    assert!(!inside.contains(&"machine".to_string()), "{inside:?}");
}

#[test]
fn a_routine_body_does_not_offer_the_machine_only_tape_keyword() {
    let head = "\
alphabet bits { '_', '1' }

routine r(tape t: bits) {
  ";
    let got = labels(&complete_between(
        head,
        "\n  entry state s { [*] -> return; }\n}\n",
    ));
    assert!(got.contains(&"state".to_string()), "{got:?}");
    assert!(!got.contains(&"tape".to_string()), "{got:?}");
}

#[test]
fn completions_survive_a_document_that_no_longer_resolves() {
    // The roster is the sanctioned staleness exception: names stay
    // available across an edit that breaks resolution, because positions
    // still come from the current tokens.
    let (mut service, uri) = opened(TWO_TAPE);
    let broken = TWO_TAPE.replace("state done", "state done extra");
    service.did_update(&uri, &broken);
    let pos = pos_after(&broken, "['1', *]", 1);
    let got = labels(&service.completion(&uri, pos));
    assert!(got.contains(&"'0'".to_string()), "{got:?}");
}

#[test]
fn every_candidate_replaces_a_span_that_touches_the_cursor() {
    let head = "\
alphabet bits { '_', '0', '1' }

machine {
  tape ctl: bits;
  entry state main { [";
    let tail = "] -> stop; }\n}\n";
    let src = format!("{head}'0{tail}");
    let pos = pos_at_byte(&src, head.len() + 2);
    let (mut service, uri) = opened(&src);
    for candidate in service.completion(&uri, pos) {
        let span = candidate.replace_span;
        assert_eq!(span.start.line, pos.line, "{candidate:?}");
        assert!(span.start <= pos && pos <= span.end, "{candidate:?}");
    }
}

// -- go to definition ----------------------------------------------------

#[test]
fn a_goto_navigates_to_the_state_it_names() {
    let (mut service, uri) = opened(TWO_TAPE);
    let target = service
        .definition(&uri, pos_after(TWO_TAPE, "goto done", 6))
        .expect("a definition");
    assert_eq!(target.span, span_of_nth(TWO_TAPE, "done", 1));
    assert_eq!(target.origin, Some(span_of_nth(TWO_TAPE, "done", 0)));
}

#[test]
fn a_graft_instance_navigates_to_the_graph_it_splices() {
    let (mut service, uri) = opened(CROSS_WORLD);
    let target = service
        .definition(&uri, pos_after(CROSS_WORLD, "as seek", 3))
        .expect("a definition");
    assert_eq!(target.span, span_of_nth(CROSS_WORLD, "findX", 0));
}

#[test]
fn a_call_target_navigates_through_the_import_to_the_routine() {
    let (mut service, uri) = opened(CROSS_WORLD);
    let target = service
        .definition(&uri, pos_after(CROSS_WORLD, "call plusOne", 5))
        .expect("a definition");
    assert_eq!(target.span, span_of_nth(CROSS_WORLD, "plusOne", 0));
}

#[test]
fn a_call_on_a_bind_instance_navigates_to_the_bind_not_a_routine() {
    let (mut service, uri) = opened(CROSS_WORLD);
    let target = service
        .definition(&uri, pos_after(CROSS_WORLD, "call inc1", 5))
        .expect("a definition");
    assert_eq!(target.span, span_of_nth(CROSS_WORLD, "inc1", 0));
}

#[test]
fn a_tape_declarations_alphabet_navigates_to_the_alphabet() {
    let (mut service, uri) = opened(CROSS_WORLD);
    let target = service
        .definition(&uri, pos_after(CROSS_WORLD, "tape ctl: bits", 10))
        .expect("a definition");
    assert_eq!(target.span, span_of_nth(CROSS_WORLD, "bits", 0));
}

#[test]
fn a_use_path_navigates_to_the_routine_it_imports() {
    let (mut service, uri) = opened(CROSS_WORLD);
    let target = service
        .definition(&uri, pos_after(CROSS_WORLD, "use mylib::plusOne", 5))
        .expect("a definition");
    assert_eq!(target.span, span_of_nth(CROSS_WORLD, "plusOne", 0));
}

#[test]
fn definition_survives_a_resolve_stage_fatal() {
    // The program outlives resolution, and every reference span lives on
    // it — so navigation keeps working on a document that does not yet
    // check out.
    let broken = CROSS_WORLD.replace("then done;", "then nowhere;");
    let (mut service, uri) = opened(&broken);
    assert!(service.docs[&uri].resolved.is_none());
    let target = service
        .definition(&uri, pos_after(&broken, "call plusOne", 5))
        .expect("a definition");
    assert_eq!(target.span, span_of_nth(&broken, "plusOne", 0));
}

// -- hover ---------------------------------------------------------------

#[test]
fn hovering_a_routine_shows_its_signature_with_tape_alphabets_and_its_doc() {
    let (mut service, uri) = opened(CROSS_WORLD);
    let hover = service
        .hover(&uri, pos_after(CROSS_WORLD, "call plusOne", 5))
        .expect("a hover");
    assert!(
        hover
            .text
            .contains("routine mylib::plusOne(tape num: bits)"),
        "{}",
        hover.text
    );
    assert!(hover.text.contains("Adds one"), "{}", hover.text);
    assert!(
        hover.text.contains("deprecated: use plusTwo instead."),
        "{}",
        hover.text
    );
}

#[test]
fn hovering_a_graph_shows_its_state_parameters_too() {
    let (mut service, uri) = opened(CROSS_WORLD);
    let hover = service
        .hover(&uri, pos_after(CROSS_WORLD, "graft findX", 6))
        .expect("a hover");
    assert!(
        hover
            .text
            .contains("graph findX(tape t: wide, state found)"),
        "{}",
        hover.text
    );
}

#[test]
fn hovering_a_bind_shows_the_resolved_binding() {
    let (mut service, uri) = opened(CROSS_WORLD);
    let hover = service
        .hover(&uri, pos_after(CROSS_WORLD, "call inc1", 5))
        .expect("a hover");
    assert!(
        hover
            .text
            .contains("bind mylib::plusOne(num = ctl) as inc1"),
        "{}",
        hover.text
    );
}

#[test]
fn hovering_an_alphabet_lists_its_symbols() {
    let (mut service, uri) = opened(CROSS_WORLD);
    let hover = service
        .hover(&uri, pos_after(CROSS_WORLD, "tape ctl: bits", 10))
        .expect("a hover");
    assert!(hover.text.contains("alphabet bits"), "{}", hover.text);
    assert!(hover.text.contains("_, 0, 1"), "{}", hover.text);
}

#[test]
fn hovering_something_undocumented_and_unnameable_shows_nothing() {
    let (mut service, uri) = opened(TWO_TAPE);
    // A move direction is not a reference to anything.
    assert!(
        service
            .hover(&uri, pos_after(TWO_TAPE, "move [>", 6))
            .is_none()
    );
}

// -- quickfixes ----------------------------------------------------------

#[test]
fn an_unresolved_goto_offers_a_state_stub_of_the_right_arity() {
    let src = TWO_TAPE.replace("goto done", "goto nowhere");
    let (mut service, uri) = opened(&src);
    let at = span_of(&src, "nowhere");
    let actions = service.code_actions(&uri, at);
    assert_eq!(actions.len(), 1, "{actions:?}");
    assert_eq!(actions[0].title, "declare state `nowhere`");
    assert!(actions[0].preferred);
    // Two tapes → a two-cell catch-all, so the stub is legal where it lands.
    assert_eq!(
        actions[0].edits[0].replacement,
        "  state nowhere { [*, *] -> stop; }\n"
    );
    // Inserted on the world's closing-brace line, at column 1.
    let close_line = src.lines().count() as u32;
    assert_eq!(actions[0].edits[0].span.start.line, close_line);
}

#[test]
fn an_omitted_binding_map_offers_the_pairs_it_needs() {
    let src = "\
alphabet marks { '_', 'x', 'y' }
alphabet other { '_', 'q', 'r' }

graph findX(tape t: marks, state found) {
  entry state walk {
    ['x'] -> found;
    [*] -> move [>] goto walk;
  }
}

machine {
  tape work: other;
  entry graft findX(t = work, found = done) as seek;
  state done { [*] -> stop; }
}
";
    let (mut service, uri) = opened(src);
    let at = span_of(src, "t = work");
    let actions = service.code_actions(&uri, at);
    let fix = actions
        .iter()
        .find(|a| a.title.contains("with map"))
        .unwrap_or_else(|| panic!("no map fix in {actions:?}"));
    assert_eq!(
        fix.edits[0].replacement,
        " with map { 'q' -> 'x', 'r' -> 'y' }"
    );
    // A zero-width insertion right after the bound tape name: applying it
    // produces the argument the compiler would have accepted.
    assert_eq!(fix.edits[0].span.start, fix.edits[0].span.end);
    assert!(
        apply(src, &fix.edits[0]).contains("t = work with map { 'q' -> 'x', 'r' -> 'y' }"),
        "{}",
        apply(src, &fix.edits[0])
    );
}

#[test]
fn a_lint_finding_that_carries_a_fix_becomes_an_action() {
    // No `.tmc` lint rule ships a `Fix` yet, so this exercises the
    // conversion itself rather than any one rule: the day a rule gains a
    // fix, it reaches the client through exactly this path.
    let finding = Diagnostic {
        code: "dead-rule",
        span: Span::new(4, 3, 4, 9),
        message: "unreachable".to_string(),
        fix: Some(Fix {
            description: "delete the rule".to_string(),
            applicability: Applicability::MachineApplicable,
            edits: vec![Edit {
                span: Span::new(4, 3, 4, 9),
                replacement: String::new(),
            }],
        }),
    };
    let overlapping = actions_from_findings(std::slice::from_ref(&finding), Span::new(4, 5, 4, 6));
    assert_eq!(overlapping.len(), 1);
    assert_eq!(overlapping[0].title, "delete the rule");
    assert!(overlapping[0].preferred);
    // A request elsewhere in the document gets nothing.
    assert!(actions_from_findings(&[finding], Span::new(9, 1, 9, 2)).is_empty());
}

// -- symbols, tokens, formatting -----------------------------------------

#[test]
fn document_symbols_name_the_alphabets_worlds_and_their_members() {
    let (mut service, uri) = opened(CROSS_WORLD);
    let symbols = service.document_symbols(&uri).expect("symbols");
    let top: Vec<&str> = symbols.iter().map(|s| s.name.as_str()).collect();
    assert!(top.contains(&"bits"), "{top:?}");
    assert!(top.contains(&"mylib"), "{top:?}");
    assert!(top.contains(&"machine"), "{top:?}");
    let machine = symbols.iter().find(|s| s.name == "machine").unwrap();
    let members: Vec<&str> = machine.children.iter().map(|s| s.name.as_str()).collect();
    assert!(members.contains(&"main"), "{members:?}");
    assert!(members.contains(&"seek"), "{members:?}");
    assert!(members.contains(&"inc1"), "{members:?}");
}

#[test]
fn document_symbols_survive_a_resolve_stage_fatal() {
    let broken = CROSS_WORLD.replace("then done;", "then nowhere;");
    let (mut service, uri) = opened(&broken);
    assert!(service.document_symbols(&uri).is_some());
}

#[test]
fn semantic_tokens_separate_declarations_references_and_literals() {
    let (mut service, uri) = opened(TWO_TAPE);
    let tokens = service.semantic_tokens(&uri).expect("tokens");
    let at = |anchor: &str| {
        let span = span_of(TWO_TAPE, anchor);
        tokens
            .iter()
            .find(|t| t.span.start == span.start)
            .unwrap_or_else(|| panic!("no token at {anchor:?}"))
    };
    assert_eq!(at("bits {").token_type, TOKEN_TYPE_TYPE);
    assert_eq!(at("bits {").modifiers, MODIFIER_DECLARATION);
    assert_eq!(at("ctl: bits").token_type, TOKEN_TYPE_VARIABLE);
    assert_eq!(at("'1'").token_type, TOKEN_TYPE_STRING);
    // Every span the framework packs must be single-line.
    assert!(tokens.iter().all(|t| t.span.start.line == t.span.end.line));
}

#[test]
fn semantic_tokens_survive_a_parse_failure() {
    let (mut service, uri) = opened("machine { tape t: bits;");
    assert!(service.semantic_tokens(&uri).is_some());
}

#[test]
fn formatting_delegates_to_the_formatter_and_is_idempotent() {
    let messy = "alphabet bits{'_','1'}\nmachine{tape t:bits;entry state s{[*]->stop;}}\n";
    let (mut service, uri) = opened(messy);
    let once = service.format(&uri).expect("formatted");
    assert_eq!(once, crate::fmt::format(messy).unwrap());
    service.did_update(&uri, &once);
    assert_eq!(service.format(&uri).as_deref(), Some(once.as_str()));
}

#[test]
fn formatting_a_document_that_does_not_parse_returns_nothing() {
    let (mut service, uri) = opened("machine {");
    assert!(service.format(&uri).is_none());
}

// -- the trait's own surface ---------------------------------------------

#[test]
fn the_service_declares_the_tmc_language_and_its_watched_config() {
    let service = TmcLanguageService::new();
    assert_eq!(service.language_id(), "tmc");
    assert_eq!(service.extensions(), [".tmc"]);
    assert_eq!(service.watched_globs(), ["**/tmt.json"]);
    assert!(!service.trigger_characters().is_empty());
}

#[test]
fn formatting_relocates_a_comment_out_of_a_binding_list() {
    // The formatter's one documented exception: a comment inside a binding
    // list, a signature parameter list, or an alphabet body cannot stay
    // where it was written and becomes an own-line comment after the
    // enclosing item. The service inherits that verbatim — it is worth
    // pinning here so the behaviour is visibly the formatter's contract
    // and not a surprise introduced by the LSP path.
    let src = "\
alphabet bits { '_', '1' }

routine r(tape t: bits) { entry state s { [*] -> return; } }

machine {
  tape ctl: bits;
  bind r(t = ctl /* why */) as r1;
  entry state main { [*] -> call r1() then main; }
}
";
    let (mut service, uri) = opened(src);
    let formatted = service.format(&uri).expect("formatted");
    assert!(formatted.contains("/* why */"), "{formatted}");
    assert!(!formatted.contains("ctl /* why */"), "{formatted}");
    // Still idempotent through the service after the relocation.
    service.did_update(&uri, &formatted);
    assert_eq!(service.format(&uri).as_deref(), Some(formatted.as_str()));
}

#[test]
fn every_request_survives_every_truncation_of_a_real_document() {
    // The position walks are full of index arithmetic over a token stream
    // that may end anywhere, which is exactly what a document being typed
    // looks like. Every prefix of a real document is fed through every
    // request; the assertion is that the service answers at all.
    let mut service = TmcLanguageService::new();
    let uri = "untitled:doc.tmc".to_string();
    for cut in 0..=CROSS_WORLD.len() {
        if !CROSS_WORLD.is_char_boundary(cut) {
            continue;
        }
        let src = &CROSS_WORLD[..cut];
        service.did_update(&uri, src);
        let pos = pos_at_byte(src, cut);
        service.completion(&uri, pos);
        service.definition(&uri, pos);
        service.hover(&uri, pos);
        service.code_actions(
            &uri,
            Span {
                start: pos,
                end: pos,
            },
        );
        service.document_symbols(&uri);
        service.semantic_tokens(&uri);
        service.format(&uri);
    }
}
