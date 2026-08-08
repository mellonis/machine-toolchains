//! The `.tmc` service battery. Every test drives the real service
//! IN-PROCESS through the `LanguageService` trait — `did_update` first (the
//! framework's own order), then the request under test — so what is
//! exercised is exactly what the server loop will call, with no transport
//! in the way.

use std::fs;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

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
  entry state s { [*] -> debugger move [>] stop; }
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
fn undeclared_external_still_published_for_bare_names() {
    // A bare (no `::`) call target nothing declares publishes through
    // `did_update` exactly as the batch compiler warns — the channel a
    // later round narrows to genuinely-undeclared names, pinned here so
    // that narrowing has something to change against.
    let src = "\
alphabet bits { '_', '1' }
machine {
  tape t: bits;
  entry state s { [*] -> call helper() then s; }
}
";
    let (mut service, uri) = opened(src);
    let diagnostics = service.did_update(&uri, src);
    let codes: Vec<_> = diagnostics.iter().map(|d| d.code).collect();
    assert!(codes.contains(&Some("undeclared-external")), "{codes:?}");
}

// -- cross-file diagnostics refinement through the overlay --------------

#[test]
fn undeclared_external_is_refined_by_the_overlay_call_variant() {
    // A bare call the overlay resolves stops warning; a bare call nothing
    // resolves is the live positive control proving the refinement — and
    // diagnostics generally — are still working, in the SAME project.
    let dir = unique_tmp_dir("refine-call");
    fs::write(
        dir.join("tmt.json"),
        r#"{"project":{"targets":{"app":{"sources":["app.tmc","helper.tmc"]}}}}"#,
    )
    .unwrap();
    fs::write(
        dir.join("helper.tmc"),
        "alphabet b { '_', '0' }\nexport routine helper(tape t: b) { entry state s { [*] -> return; } }\n",
    )
    .unwrap();

    let app_src = "\
alphabet bits { '_', '1' }
machine {
  tape t: bits;
  entry state s { ['_'] -> call helper() then g; ['1'] -> call ghost() then g; }
  state g { [*] -> stop; }
}
";
    let mut service = TmcLanguageService::new();
    let app_uri = file_uri(&dir.join("app.tmc"));
    let diags = service.did_update(&app_uri, app_src);

    assert!(
        !diags
            .iter()
            .any(|d| d.code == Some("undeclared-external") && d.message.contains("`helper`")),
        "the sibling's export refines the bare call away: {diags:?}"
    );
    assert!(
        diags
            .iter()
            .any(|d| d.code == Some("undeclared-external") && d.message.contains("`ghost`")),
        "no sibling defines ghost — the live positive control: {diags:?}"
    );
}

#[test]
fn undeclared_external_stays_without_a_manifest_on_a_real_file_path() {
    // A real `file:` path with no tmt.json anywhere on the walk — the
    // OTHER route to a `None` overlay besides an untitled buffer
    // (`undeclared_external_still_published_for_bare_names` covers that
    // one). The SAME name the call-variant test above resolves, so this
    // also serves as that test's control: `helper` really does warn on
    // its own when no link set is declared at all.
    let dir = unique_tmp_dir("refine-no-manifest");
    let app_src = "\
alphabet bits { '_', '1' }
machine {
  tape t: bits;
  entry state s { [*] -> call helper() then s; }
}
";
    let mut service = TmcLanguageService::new();
    let app_uri = file_uri(&dir.join("app.tmc"));
    let diags = service.did_update(&app_uri, app_src);

    assert!(
        diags
            .iter()
            .any(|d| d.code == Some("undeclared-external") && d.message.contains("`helper`")),
        "no manifest anywhere on the walk — per-file honest: {diags:?}"
    );
    let state = service.docs.get(&app_uri).unwrap();
    assert!(
        state.overlay.is_none(),
        "no tmt.json on the walk — single-file degrade"
    );
}

#[test]
fn stdlib_resolved_bare_name_is_not_suppressed() {
    // The stdlib defines `std::binaryNumbersBare::plusOne`, never a bare
    // `plusOne` — a bare call to it must keep warning even though the
    // project has the stdlib enabled (the default), exactly matching what
    // the build driver's own `defined_names` union produces. `helper`,
    // resolved by an actual sibling in the SAME project, is the live
    // positive control proving the overlay refinement is genuinely active
    // here, not merely inert.
    let dir = unique_tmp_dir("refine-stdlib");
    fs::write(
        dir.join("tmt.json"),
        r#"{"project":{"targets":{"app":{"sources":["app.tmc","helper.tmc"]}}}}"#,
    )
    .unwrap();
    fs::write(
        dir.join("helper.tmc"),
        "alphabet b { '_', '0' }\nexport routine helper(tape t: b) { entry state s { [*] -> return; } }\n",
    )
    .unwrap();

    let app_src = "\
alphabet bits { '_', '1' }
machine {
  tape t: bits;
  entry state s { ['_'] -> call helper() then g; ['1'] -> call plusOne() then g; }
  state g { [*] -> stop; }
}
";
    let mut service = TmcLanguageService::new();
    let app_uri = file_uri(&dir.join("app.tmc"));
    let diags = service.did_update(&app_uri, app_src);

    assert!(
        !diags
            .iter()
            .any(|d| d.code == Some("undeclared-external") && d.message.contains("`helper`")),
        "the sibling's export refines the bare call away: {diags:?}"
    );
    assert!(
        diags
            .iter()
            .any(|d| d.code == Some("undeclared-external") && d.message.contains("`plusOne`")),
        "std::binaryNumbersBare::plusOne is not a bare plusOne: {diags:?}"
    );

    // The `helper` control above only proves an overlay exists — it says
    // nothing about the STDLIB leg specifically. Pin that leg directly:
    // the project really does have `stdlib` on, and the roster's
    // NAMESPACED path is in the defined set while the bare name is not —
    // the exact reason the bare call above keeps warning.
    let state = service.docs.get(&app_uri).unwrap();
    let overlay = state.overlay.as_ref().expect("a real project");
    assert!(overlay.stdlib, "the manifest defaults the stdlib on");
    let defined = overlay.defined_names();
    assert!(
        defined.contains("std::binaryNumbersBare::plusOne"),
        "the roster leg really is in the defined set: {defined:?}"
    );
    assert!(
        !defined.contains("plusOne"),
        "…and only as a namespaced path — never the bare name: {defined:?}"
    );
}

#[test]
fn undeclared_external_is_refined_by_the_overlay_bind_variant() {
    // This crate's warning fires for bare BIND targets too
    // (`compiler::tests::bind_target_must_be_a_routine`); the overlay
    // refinement must apply there identically to the call variant. A
    // third bind, through a `use` import, pins the orthogonal invariant
    // that a qualified/imported target never warns in the first place
    // (`warn_undeclared_if_bare`'s bare-only check) — regardless of
    // whether an overlay exists at all.
    let dir = unique_tmp_dir("refine-bind");
    fs::write(
        dir.join("tmt.json"),
        r#"{"project":{"targets":{"app":{"sources":["app.tmc","helper.tmc"]}}}}"#,
    )
    .unwrap();
    fs::write(
        dir.join("helper.tmc"),
        "alphabet b { '_', '0' }\nexport routine helper(tape t: b) { entry state s { [*] -> return; } }\n",
    )
    .unwrap();

    let app_src = "\
alphabet abc { '_', 'a', 'b' }
use lib::r;
machine {
  tape t: abc;
  bind helper(t = t) as h1;
  bind ghost(t = t) as h2;
  bind r(t = t) as h3;
  entry state s { ['_'] -> call h1() then s; ['a'] -> call h2() then s; ['b'] -> call h3() then s; }
}
";
    let mut service = TmcLanguageService::new();
    let app_uri = file_uri(&dir.join("app.tmc"));
    let diags = service.did_update(&app_uri, app_src);

    assert!(
        !diags
            .iter()
            .any(|d| d.code == Some("undeclared-external") && d.message.contains("`helper`")),
        "the sibling's export refines the bare bind target away: {diags:?}"
    );
    assert!(
        diags
            .iter()
            .any(|d| d.code == Some("undeclared-external") && d.message.contains("`ghost`")),
        "no sibling defines ghost — the live positive control: {diags:?}"
    );
    assert!(
        !diags
            .iter()
            .any(|d| d.code == Some("undeclared-external") && d.message.contains("`r`")),
        "a qualified/imported bind target never warns regardless of the overlay: {diags:?}"
    );
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
  entry state s { [*] -> debugger move [>] stop; }
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
fn config_cache_stays_bounded_across_many_distinct_project_roots() {
    // More distinct `tmt.json` roots than the eviction bound, so an
    // unbounded cache would visibly outgrow it (docs/lsp.md
    // (configuration)).
    let src = "\
alphabet bits { '_', '1' }
machine {
  tape t: bits;
  entry state s { [*] -> stop; }
}
";
    let mut service = TmcLanguageService::new();
    for i in 0..(CONFIG_CACHE_LIMIT + 8) {
        let dir = unique_tmp_dir(&format!("cache-bound-{i}"));
        fs::write(dir.join("tmt.json"), "{}").unwrap();
        let uri = file_uri(&dir.join("prog.tmc"));
        service.did_update(&uri, src);
    }
    assert!(
        service.config_cache.len() <= CONFIG_CACHE_LIMIT,
        "cache grew past its bound: {} entries",
        service.config_cache.len()
    );
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

/// [`complete_typing`]'s settle-then-break sequence, but driven against
/// `uri` on an ALREADY-CONSTRUCTED `service` instead of a fresh untitled
/// one — for a project `file:` document, so the cross-file overlay
/// (docs/lsp.md (project overlay)) is genuinely active for the completion
/// request under test.
fn complete_typing_in(
    service: &mut TmcLanguageService,
    uri: &str,
    prefix: &str,
    settled: &str,
    suffix: &str,
) -> Vec<Candidate> {
    service.did_update(uri, &format!("{prefix}{settled}{suffix}"));
    let src = format!("{prefix}{suffix}");
    let pos = pos_at_byte(&src, prefix.len());
    service.did_update(uri, &src);
    service.completion(uri, pos)
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
fn a_call_target_candidate_carries_its_targets_deprecation() {
    // The same declaration the diagnostic tags and hover spells out must
    // arrive tagged in the completion list too — an editor that strikes
    // through a deprecated name at the call site should strike it through in
    // the list the call site was picked from.
    let head = "\
alphabet bits { '_', '1' }

? still here, but on the way out.
! [deprecated] use fresh instead.
routine stale(tape t: bits) { entry state s { [*] -> return; } }
routine fresh(tape t: bits) { entry state s { [*] -> return; } }

machine {
  tape ctl: bits;
  entry state main { [*] -> call ";
    let tail = "(t = ctl) then main;\n  }\n}\n";
    let got = complete_typing(head, "fresh", tail);
    let tagged = |name: &str| {
        got.iter()
            .find(|c| c.label == name)
            .unwrap_or_else(|| panic!("{name} missing from {:?}", labels(&got)))
            .deprecated
    };
    assert!(tagged("stale"), "the deprecated routine should be tagged");
    assert!(!tagged("fresh"), "the live routine should not be tagged");
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
fn use_path_completion_offers_std_and_its_members() {
    let src = "\
use std::binaryNumbers::goToNumber;

alphabet a { '_', '0' }

machine {
  tape t: a;
  entry state s { [*] -> stop; }
}
";
    let (mut service, uri) = opened(src);
    let got = labels(&service.completion(&uri, pos_after(src, "use ", 4)));
    assert!(got.contains(&"std".to_string()), "{got:?}");
    assert!(
        got.contains(&"std::binaryNumbers::goToNumber".to_string()),
        "{got:?}"
    );
    assert!(
        got.contains(&"std::binaryNumbersBare::plusOne".to_string()),
        "{got:?}"
    );
    // Routines only — the stdlib's graphs and alphabets contribute no
    // linkable symbol a `use` path could ever bind.
    assert!(
        !got.contains(&"std::binaryNumbers::plusOneGraph".to_string()),
        "{got:?}"
    );
    assert!(
        !got.contains(&"std::binaryNumbers::symbols".to_string()),
        "{got:?}"
    );
}

#[test]
fn call_target_completion_offers_qualified_std_routines() {
    let call_head = "\
alphabet bits { '_', '1' }

machine {
  tape ctl: bits;
  entry state main { [*] -> call ";
    let call_tail = "() then main;\n  }\n}\n";
    let call_got = labels(&complete_typing(
        call_head,
        "std::binaryNumbers::goToNumber",
        call_tail,
    ));
    assert!(
        call_got.contains(&"std::binaryNumbers::goToNumber".to_string()),
        "{call_got:?}"
    );

    let bind_head = "\
alphabet bits { '_', '1' }

machine {
  tape ctl: bits;
  bind ";
    // Argless, like the transparent form `call` uses across a link
    // boundary — a BOUND tape argument into an external routine is
    // `external-binding-unsupported` at `tmt compile` (docs/tmt/stdlib.md
    // (transparent call)), and the fixture should stay legal code, not
    // merely something the LSP's staged analysis happens to tolerate.
    let bind_tail = "() as inc1;\n  entry state main { [*] -> call inc1() then main; }\n}\n";
    let bind_got = labels(&complete_typing(
        bind_head,
        "std::binaryNumbersBare::plusOne",
        bind_tail,
    ));
    assert!(
        bind_got.contains(&"std::binaryNumbersBare::plusOne".to_string()),
        "{bind_got:?}"
    );
}

#[test]
fn graft_target_completion_never_offers_std_names() {
    // R1: across a link boundary a graft has no source to splice, so the
    // stdlib contributes nothing here — same fixture as the graphs-only
    // test above, plus the negative assertion.
    let head = "\
alphabet bits { '_', '1' }

graph g(tape t: bits, state done) { entry state s { [*] -> done; } }

machine {
  tape ctl: bits;
  entry graft ";
    let tail = "(t = ctl, done = fin) as seek;\n  state fin { [*] -> stop; }\n}\n";
    let got = labels(&complete_typing(head, "g", tail));
    assert!(got.contains(&"g".to_string()), "{got:?}");
    assert!(!got.iter().any(|l| l.starts_with("std::")), "{got:?}");
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

    // Each value slot then offers the vocabulary ITS parameter takes: `t`
    // is a tape parameter, `done` a state parameter.
    let tape_value = labels(&complete_typing(
        &format!("{head}t = "),
        "ctl, done = fin",
        tail,
    ));
    assert!(tape_value.contains(&"ctl".to_string()), "{tape_value:?}");
    assert!(!tape_value.contains(&"fin".to_string()), "{tape_value:?}");

    let state_value = labels(&complete_typing(
        &format!("{head}t = ctl, done = "),
        "fin",
        tail,
    ));
    assert!(state_value.contains(&"fin".to_string()), "{state_value:?}");
    assert!(!state_value.contains(&"ctl".to_string()), "{state_value:?}");
}

/// A machine with three tapes, three transition targets, and a routine
/// whose signature has ONE tape parameter and ONE state parameter — the
/// fixture that makes a binding value's two vocabularies tell apart.
const SIGNATURE_WORLD: &str = "\
alphabet green { '_', 'g' }

routine work(tape gg: green, state back) {
  entry state s { [*] -> back; }
}

graph gr(tape t: green, state done) { entry state s { [*] -> done; } }

machine {
  tape a: green;
  tape b: green;
  tape c: green;

  graft gr(t = a, done = fin) as sk;

  entry state main { [*, *, *] -> stop; }
  state done { [*, *, *] -> stop; }
  state fin { [*, *, *] -> stop; }

  bind work(";

const SIGNATURE_WORLD_TAIL: &str = ") as w1;\n}\n";

#[test]
fn a_tape_parameters_value_offers_tapes_and_nothing_a_continuation_takes() {
    let got = labels(&complete_typing(
        &format!("{SIGNATURE_WORLD}gg = "),
        "a, back = done",
        SIGNATURE_WORLD_TAIL,
    ));
    assert_eq!(got, vec!["a", "b", "c"], "{got:?}");
}

#[test]
fn a_state_parameters_value_offers_continuations_and_no_tapes() {
    let got = labels(&complete_typing(
        &format!("{SIGNATURE_WORLD}gg = a, back = "),
        "done",
        SIGNATURE_WORLD_TAIL,
    ));
    for target in ["main", "done", "fin", "sk", "halt", "return", "stop"] {
        assert!(got.contains(&target.to_string()), "{got:?}");
    }
    for tape in ["a", "b", "c"] {
        assert!(!got.contains(&tape.to_string()), "{got:?}");
    }
}

#[test]
fn an_unresolvable_binding_parameter_falls_back_to_the_union() {
    // Degrading to MORE candidates is a nuisance; degrading to none is a
    // dead list. A callee that does not exist — the ordinary state of a
    // half-typed name — must therefore keep offering both vocabularies.
    let settled = format!("{SIGNATURE_WORLD}gg = a, back = done{SIGNATURE_WORLD_TAIL}");
    let (mut service, uri) = opened(&settled);
    let head = SIGNATURE_WORLD.replace("bind work(", "bind mystery(zz = ");
    service.did_update(&uri, &head);
    let pos = pos_at_byte(&head, head.len());
    let got = labels(&service.completion(&uri, pos));
    for name in ["a", "b", "c", "done", "fin", "sk", "halt", "return", "stop"] {
        assert!(got.contains(&name.to_string()), "{got:?}");
    }
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
    assert!(inside.contains(&"volatile".to_string()), "{inside:?}");
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
    assert!(!got.contains(&"volatile".to_string()), "{got:?}");
}

#[test]
fn after_volatile_the_item_boundary_completion_offers_nothing_yet() {
    // `volatile` is the first two-token item-boundary keyword: after it, the
    // cursor is no longer AT a boundary (the previous token is an `Ident`,
    // not `;` / `{` / `}`), so `classify_context`'s `at_boundary` check
    // returns `None` rather than re-offering `tape`. This is an accepted
    // gap, not a fix owed here: the follow-on keyword is a single word the
    // language reference already spells out in full
    // (docs/tmt/language.md (volatile tapes)), and closing it needs a
    // widened boundary check that must NOT also re-offer `bind`/`entry`/
    // `graft`/`state` after `volatile` — a small feature of its own.
    let head = "\
alphabet bits { '_', '1' }

machine {
  volatile ";
    let got = labels(&complete_between(
        head,
        "tape sensor: bits;\n  entry state s { [*] -> stop; }\n}\n",
    ));
    assert!(got.is_empty(), "{got:?}");
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
fn definition_on_a_std_call_target_jumps_into_materialized_std_tmc() {
    let src = "\
use std::binaryNumbers::goToNumber;

alphabet a { '_', '^', '$', '0', '1' }

machine {
  tape num: a;
  entry state s { [*] -> call goToNumber() then done; }
  state done { [*] -> stop; }
}
";
    let (mut service, uri) = opened(src);
    let target = service
        .definition(&uri, pos_after(src, "call goToNumber", 5))
        .expect("a definition");

    let want_uri = crate::stdlib::materialized_std_uri().expect("materializes in test env");
    assert_eq!(target.uri, want_uri);

    let entry = crate::stdlib::roster()
        .iter()
        .find(|e| e.full_path == "std::binaryNumbers::goToNumber")
        .expect("goToNumber is in the roster");
    assert_eq!(target.span, entry.name_span);
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
fn hovering_a_routine_prefixes_its_volatile_signature_parameters() {
    let src = "\
alphabet bits { '_', '1' }

routine r(volatile tape sensor: bits, tape scratch: bits) {
  entry state s { [*, *] -> return; }
}

machine {
  tape a: bits;
  tape b: bits;
  entry state main { [*, *] -> call r(sensor = a, scratch = b) then done; }
  state done { [*, *] -> stop; }
}
";
    let (mut service, uri) = opened(src);
    let hover = service
        .hover(&uri, pos_after(src, "call r", 5))
        .expect("a hover");
    assert!(
        hover
            .text
            .contains("routine r(volatile tape sensor: bits, tape scratch: bits)"),
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
fn hover_on_a_std_call_target_returns_its_doc() {
    // The reported gap this reproduces, inverted: a `std::` call target
    // now hovers with the routine's own doc, resolved through the
    // embedded stdlib's analysis rather than this document's (which never
    // holds a std entry) — previously such a hover returned nothing.
    let src = "\
alphabet a { '_', '^', '$', '0', '1' }

machine {
  tape num: a;
  entry state s { [*] -> call std::binaryNumbers::goToNumber() then done; }
  state done { [*] -> stop; }
}
";
    let (mut service, uri) = opened(src);
    let hover = service
        .hover(
            &uri,
            pos_after(src, "call std::binaryNumbers::goToNumber", 5),
        )
        .expect("a hover");
    assert!(
        hover
            .text
            .contains("Walk right to the current number's end marker"),
        "{}",
        hover.text
    );
}

#[test]
fn hovering_a_genuine_external_with_no_std_doc_shows_nothing() {
    // `External` covers any qualified/aliased name this document does not
    // declare, not only std ones. With no doc to show, the head alone
    // would just echo the reference's own text back at the cursor — the
    // emptiness rule says that is not a hover, so this must stay `None`
    // exactly like an undocumented LOCAL declaration does.
    let src = "\
alphabet a { '_', '0' }
use lib::helper;
machine {
  tape t: a;
  entry state s { [*] -> call helper() then s; }
}
";
    let (mut service, uri) = opened(src);
    assert!(
        service
            .hover(&uri, pos_after(src, "call helper", 5))
            .is_none()
    );
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
fn a_state_stub_in_a_nested_world_lands_at_the_depth_fmt_expects() {
    // The stub's indent is read off the enclosing block's closing brace, so
    // a world one namespace deep gets one level more than a top-level
    // `machine`. Formatting the fixed text is the assertion: an indent a
    // level short would leave `tmt fmt --check` failing on a file the
    // quickfix itself produced.
    let src = "\
alphabet bits { '_', '1' }

namespace lib {
  routine work(tape t: bits) {
    entry state s { [*] -> goto nowhere; }
  }
}

machine {
  tape ctl: bits;
  entry state main { [*] -> stop; }
}
";
    let (mut service, uri) = opened(src);
    let at = span_of(src, "nowhere");
    let actions = service.code_actions(&uri, at);
    assert_eq!(actions.len(), 1, "{actions:?}");
    assert_eq!(
        actions[0].edits[0].replacement,
        "    state nowhere { [*] -> stop; }\n"
    );
    let fixed = apply(src, &actions[0].edits[0]);
    assert_eq!(crate::fmt::format(&fixed).expect("formats"), fixed);
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
    // A synthetic finding exercises the fix->action conversion in isolation,
    // independent of which real rules ship a `Fix`: every rule that carries
    // one reaches the client through exactly this path.
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
fn formatting_prints_a_comment_in_place_inside_a_binding_list() {
    // A comment inside a binding list, a signature parameter list, or an
    // alphabet body prints where it was written rather than relocating
    // below the enclosing item. The service inherits that verbatim — it is
    // worth pinning here so the behaviour is visibly the formatter's
    // contract and not a surprise introduced by the LSP path.
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
    assert!(formatted.contains("ctl /* why */"), "{formatted}");
    // Still idempotent through the service.
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

// -- cross-file completion, navigation, and hover through the overlay ----
//
// The surface is deliberately narrower than the diagnostics refinement
// above: only `use`-path context and a bare `call`/`bind` TARGET position
// ever offer an overlay candidate — a transparent, argless `call` is the
// one shape that works across a compiled-object boundary
// (docs/tmt/stdlib.md (transparent call)). Graft-target, binding-name,
// binding-value, and map contexts must never see one; each negative below
// is paired with a live positive control proving the context itself still
// works, not merely that the overlay stayed out of it.

#[test]
fn use_path_completion_offers_sibling_namespaces_and_routines() {
    let dir = unique_tmp_dir("use-path-overlay");
    fs::write(
        dir.join("tmt.json"),
        r#"{"project":{"targets":{"app":{"sources":["app.tmc","helper.tmc"]}}}}"#,
    )
    .unwrap();
    // The sibling's OWN `machine` block matters here, not just its
    // routines: a machine always contributes its linker symbol `main` to
    // the overlay table (the diagnostics refinement this table also
    // serves treats it as a legitimately defined external name), but
    // `main` is a program's own entry, never a `use`-able unit — this is
    // the live negative for that exclusion, not a vacuous one, since
    // without the sibling machine block `main` could never have leaked
    // in the first place.
    fs::write(
        dir.join("helper.tmc"),
        "alphabet b { '_', '0' }\n\
         namespace mylib {\n  export routine plusOne(tape num: b) { entry state s { [*] -> return; } }\n}\n\
         export routine bare(tape t: b) { entry state s { [*] -> return; } }\n\
         machine {\n  tape t: b;\n  entry state s { [*] -> stop; }\n}\n",
    )
    .unwrap();

    // A fully-formed `use` statement already in place — `Context::UsePath`
    // doesn't care what the path already spells, only that the cursor
    // sits somewhere inside one (mirrors `use_path_completion_offers_std_
    // and_its_members`, which positions the cursor the same way).
    let app_src = "\
alphabet bits { '_', '1' }
use std::binaryNumbers::goToNumber;

machine {
  tape t: bits;
  entry state s { [*] -> stop; }
}
";
    let mut service = TmcLanguageService::new();
    let app_uri = file_uri(&dir.join("app.tmc"));
    service.did_update(&app_uri, app_src);
    let got = labels(&service.completion(&app_uri, pos_after(app_src, "use ", 4)));

    // Member ROOTS: a namespace worth typing past, the same convenience
    // the hardcoded "std" literal offers for the embedded library.
    assert!(got.contains(&"mylib".to_string()), "{got:?}");
    // Members: the full qualified path, pickable whole.
    assert!(got.contains(&"mylib::plusOne".to_string()), "{got:?}");
    // A bare top-level sibling export.
    assert!(got.contains(&"bare".to_string()), "{got:?}");
    // The sibling's OWN machine entry never leaks in as a `use`-able name.
    assert!(!got.contains(&"main".to_string()), "{got:?}");
    // Positive control: the overlay's own additions don't crowd out the
    // pre-existing stdlib offer.
    assert!(got.contains(&"std".to_string()), "{got:?}");
}

// This test is also Task 17's TM-only negative: it already exercises every
// context the plan calls out by name — graft-target (`Target(Graft)`),
// `BindingName`, `BindingValue`, `VectorCell`, `MapSrc`, `MapDst` — each a
// leak-negative paired with a live LOCAL positive control, driven through a
// real `did_update` with a genuinely active overlay (the sanity check just
// below the call-target assertions). A second, separate test would only
// repeat this one's own fixture and assertions.
#[test]
fn call_and_bind_target_completion_offers_sibling_exported_routines() {
    let dir = unique_tmp_dir("target-overlay");
    fs::write(
        dir.join("tmt.json"),
        r#"{"project":{"targets":{"app":{"sources":["app.tmc","helper.tmc"]}}}}"#,
    )
    .unwrap();
    // The sibling's OWN `machine` block: its linker symbol `main` must
    // never surface at a call/bind target either — the negative both
    // probes below assert, right alongside the positive `mylib::plusOne`.
    // `plusOne` itself carries a `! [deprecated]` doc line; `freshOne`
    // stays undeprecated as its control — the candidate's `deprecated`
    // flag must come from the SIBLING's own doc, not default to false or
    // to true across the board.
    fs::write(
        dir.join("helper.tmc"),
        "alphabet b { '_', '0' }\nnamespace mylib {\n\
         ! [deprecated] use freshOne instead.\n  export routine plusOne(tape num: b) { entry state s { [*] -> return; } }\n\
         export routine freshOne(tape num: b) { entry state s { [*] -> return; } }\n}\n\
         machine {\n  tape t: b;\n  entry state s { [*] -> stop; }\n}\n",
    )
    .unwrap();

    let mut service = TmcLanguageService::new();
    let app_uri = file_uri(&dir.join("app.tmc"));

    // A `call` target: the sibling's qualified routine joins the local one.
    // The inline `t = ctl` binding is load-bearing — an ARGLESS direct call
    // to a LOCAL routine with a required tape parameter is a resolve-stage
    // fatal (unlike an external target, whose binding validation the
    // compiler skips entirely), which would leave no roster to complete
    // from at all.
    let call_head = "\
alphabet bits { '_', '1' }

routine localR(tape t: bits) { entry state s { [*] -> return; } }

machine {
  tape ctl: bits;
  entry state main { [*] -> call ";
    let call_tail = "(t = ctl) then main;\n  }\n}\n";
    let call_candidates =
        complete_typing_in(&mut service, &app_uri, call_head, "localR", call_tail);
    let call_got = labels(&call_candidates);
    assert!(
        call_got.contains(&"mylib::plusOne".to_string()),
        "{call_got:?}"
    );
    assert!(
        call_got.contains(&"localR".to_string()),
        "local control: {call_got:?}"
    );
    assert!(
        !call_got.contains(&"main".to_string()),
        "the sibling's machine entry is not a call target: {call_got:?}"
    );
    // Sanity: every negative in this test (graft target, binding name,
    // binding value, vector-cell glyph, map source, map destination, all
    // below) is only meaningful with a real, active overlay behind it —
    // `mylib::plusOne` surfacing above already proves that behaviorally,
    // but a literal peek at the overlay's own table closes the gap a
    // regression that disabled the overlay outright would otherwise slip
    // through (every negative assertion below would pass vacuously).
    let overlay = service
        .docs
        .get(&app_uri)
        .unwrap()
        .overlay
        .as_ref()
        .expect("a real, active overlay exists for this document");
    assert!(
        overlay.symbols.contains_key("mylib::plusOne"),
        "{:?}",
        overlay.symbols.keys().collect::<Vec<_>>()
    );
    // The deprecation tag comes from the SIBLING's own doc, not this
    // document's roster: `plusOne` carries `! [deprecated]`, `freshOne`
    // (also exported by the sibling, so a live positive control rather
    // than an untested field) does not.
    let deprecated_of = |label: &str| {
        call_candidates
            .iter()
            .find(|c| c.label == label)
            .unwrap_or_else(|| panic!("{label} missing from {call_got:?}"))
            .deprecated
    };
    assert!(deprecated_of("mylib::plusOne"), "{call_got:?}");
    assert!(!deprecated_of("mylib::freshOne"), "{call_got:?}");

    // A `bind` target: the same qualified label.
    let bind_head = "\
alphabet bits { '_', '1' }

routine localR(tape t: bits) { entry state s { [*] -> return; } }

machine {
  tape ctl: bits;
  bind ";
    let bind_tail = "(t = ctl) as b1;\n  entry state main { [*] -> call b1() then main; }\n}\n";
    let bind_got = labels(&complete_typing_in(
        &mut service,
        &app_uri,
        bind_head,
        "localR",
        bind_tail,
    ));
    assert!(
        bind_got.contains(&"mylib::plusOne".to_string()),
        "{bind_got:?}"
    );
    assert!(
        bind_got.contains(&"localR".to_string()),
        "local control: {bind_got:?}"
    );
    assert!(
        !bind_got.contains(&"main".to_string()),
        "the sibling's machine entry is not a bind target: {bind_got:?}"
    );

    // R1: a graft target never sees the overlay — a local graph is the
    // positive control that graft completion itself still works here.
    let graft_head = "\
alphabet bits { '_', '1' }

graph localG(tape t: bits, state done) { entry state s { [*] -> done; } }

machine {
  tape ctl: bits;
  entry graft ";
    let graft_tail = "(t = ctl, done = fin) as seek;\n  state fin { [*] -> stop; }\n}\n";
    let graft_got = labels(&complete_typing_in(
        &mut service,
        &app_uri,
        graft_head,
        "localG",
        graft_tail,
    ));
    assert!(
        graft_got.contains(&"localG".to_string()),
        "local control: {graft_got:?}"
    );
    assert!(
        !graft_got.contains(&"mylib::plusOne".to_string()),
        "{graft_got:?}"
    );

    // R2: a binding NAME context (the callee's own parameter names) never
    // sees the overlay — the positive control is the local routine's own
    // parameter `t` still being offered.
    let bname_head = "\
alphabet bits { '_', '1' }

routine localR(tape t: bits) { entry state s { [*] -> return; } }

machine {
  tape ctl: bits;
  bind localR(";
    let bname_tail = ") as b1;\n  entry state main { [*] -> call b1() then main; }\n}\n";
    let bname_got = labels(&complete_typing_in(
        &mut service,
        &app_uri,
        bname_head,
        "t = ctl",
        bname_tail,
    ));
    assert!(
        bname_got.contains(&"t".to_string()),
        "local param control: {bname_got:?}"
    );
    assert!(
        !bname_got.contains(&"mylib::plusOne".to_string()),
        "{bname_got:?}"
    );
    assert!(!bname_got.contains(&"mylib".to_string()), "{bname_got:?}");

    // R2: a binding VALUE context (this world's own tapes/states) never
    // sees the overlay — the positive control is the enclosing machine's
    // own tape `ctl`.
    let bvalue_head = "\
alphabet bits { '_', '1' }

routine localR(tape t: bits) { entry state s { [*] -> return; } }

machine {
  tape ctl: bits;
  bind localR(t = ";
    let bvalue_tail = ") as b1;\n  entry state main { [*] -> call b1() then main; }\n}\n";
    let bvalue_got = labels(&complete_typing_in(
        &mut service,
        &app_uri,
        bvalue_head,
        "ctl",
        bvalue_tail,
    ));
    assert!(
        bvalue_got.contains(&"ctl".to_string()),
        "local tape control: {bvalue_got:?}"
    );
    assert!(
        !bvalue_got.contains(&"mylib::plusOne".to_string()),
        "{bvalue_got:?}"
    );

    // R1/R2: a vector-cell glyph slot never sees the overlay — a cell's
    // glyphs resolve through a LOCAL alphabet by definition. The local
    // alphabet's own glyphs are the positive control.
    let vcell_head = "\
alphabet bits { '_', '1' }

machine {
  tape ctl: bits;
  entry state main { [";
    let vcell_tail = "] -> stop;\n  }\n}\n";
    let vcell_got = labels(&complete_typing_in(
        &mut service,
        &app_uri,
        vcell_head,
        "'1'",
        vcell_tail,
    ));
    assert!(
        vcell_got.contains(&"'1'".to_string()),
        "local glyph control: {vcell_got:?}"
    );
    assert!(
        !vcell_got.contains(&"mylib::plusOne".to_string()),
        "{vcell_got:?}"
    );

    // R1/R2: a binding map's source and destination glyph slots never see
    // the overlay either — each resolves through a LOCAL alphabet (the
    // host tape's, then the callee's own signature alphabet). The host
    // and callee alphabets are deliberately DIFFERENT here (mirrors
    // `a_map_pair_offers_the_host_alphabet_left_and_the_callee_alphabet_
    // right`), so each side's own glyphs are a positive control that
    // distinguishes the two.
    let map_head = "\
alphabet bits { '_', '0', '1' }
alphabet wide { '_', 'a', 'b' }

routine localR(tape t: wide) { entry state s { [*] -> return; } }

machine {
  tape ctl: bits;
  entry state main { [*] -> call localR(t = ctl with map { ";
    let map_tail = " }) then main;\n  }\n}\n";

    let mapsrc_got = labels(&complete_typing_in(
        &mut service,
        &app_uri,
        map_head,
        "'0' -> 'a'",
        map_tail,
    ));
    assert!(
        mapsrc_got.contains(&"'0'".to_string()),
        "host alphabet control: {mapsrc_got:?}"
    );
    assert!(
        !mapsrc_got.contains(&"mylib::plusOne".to_string()),
        "{mapsrc_got:?}"
    );

    let mapdst_head = format!("{map_head}'0' -> ");
    let mapdst_got = labels(&complete_typing_in(
        &mut service,
        &app_uri,
        &mapdst_head,
        "'a'",
        map_tail,
    ));
    assert!(
        mapdst_got.contains(&"'a'".to_string()),
        "callee alphabet control: {mapdst_got:?}"
    );
    assert!(
        !mapdst_got.contains(&"mylib::plusOne".to_string()),
        "{mapdst_got:?}"
    );
}

#[test]
fn definition_and_hover_reach_a_tmc_sibling() {
    let dir = unique_tmp_dir("nav-tmc-sibling");
    fs::write(
        dir.join("tmt.json"),
        r#"{"project":{"targets":{"app":{"sources":["app.tmc","helper.tmc"]}}}}"#,
    )
    .unwrap();
    // A real `?` doc line — the fixture that closes the gap left by every
    // OTHER overlay test, none of which exercises `ExportedSym.doc` with
    // an actual `Some(Doc)`.
    let helper_src = "\
alphabet b { '_', '0' }
namespace ns {
?Adds one to the number under the head.
  export routine addOne(tape num: b) { entry state s { [*] -> return; } }
}
";
    fs::write(dir.join("helper.tmc"), helper_src).unwrap();

    let app_src = "\
alphabet bits { '_', '1' }
machine {
  tape ctl: bits;
  entry state main { [*] -> call ns::addOne() then main; }
}
";
    let mut service = TmcLanguageService::new();
    let app_uri = file_uri(&dir.join("app.tmc"));
    service.did_update(&app_uri, app_src);
    let helper_uri = file_uri(&dir.join("helper.tmc"));

    let pos = pos_after(app_src, "call ", 5);

    let target = service
        .definition(&app_uri, pos)
        .expect("the call target resolves through the overlay to the sibling's export");
    assert_eq!(target.uri, helper_uri, "{target:?}");
    assert_eq!(target.span, span_of(helper_src, "addOne"), "{target:?}");

    let hover = service
        .hover(&app_uri, pos)
        .expect("hover resolves through the overlay to the sibling's export");
    assert!(
        hover
            .text
            .contains("Adds one to the number under the head."),
        "the sibling's OWN doc text must surface, not merely SOME text: {hover:?}"
    );
}

#[test]
fn definition_reaches_a_tma_sibling_and_tmo_names_navigate_null() {
    let dir = unique_tmp_dir("nav-tma-tmo");
    fs::write(
        dir.join("tmt.json"),
        r#"{"project":{"targets":{"app":{"sources":["app.tmc","sibling.tma","sibling.tmo"]}}}}"#,
    )
    .unwrap();

    // Both names are `::`-qualified — a BARE, unqualified external
    // reference isn't recognized as an `External` navigation target at all
    // (`external_path` only resolves a `::`-qualified name or a `use`-bound
    // alias), a pre-existing single-file limitation this test must not
    // exercise by accident.
    let tma_src = ".func ns::tmaFn\nhlt\n";
    fs::write(dir.join("sibling.tma"), tma_src).unwrap();

    let tmo_bytes = crate::compiler::compile(
        "alphabet b { '_', '0' }\nnamespace ns2 {\n  export routine tmoFn(tape t: b) { entry state s { [*] -> return; } }\n}\n",
        crate::compiler::CompileOptions::default(),
    )
    .expect("tmoFn compiles")
    .object
    .to_bytes();
    fs::write(dir.join("sibling.tmo"), &tmo_bytes).unwrap();

    let app_src = "\
alphabet bits { '_', '1' }
machine {
  tape ctl: bits;
  entry state s { ['_'] -> call ns::tmaFn() then g; ['1'] -> call ns2::tmoFn() then g; }
  state g { [*] -> stop; }
}
";
    let mut service = TmcLanguageService::new();
    let app_uri = file_uri(&dir.join("app.tmc"));
    service.did_update(&app_uri, app_src);

    // The `.tma` sibling's non-`local` `.func` carries its own name span —
    // real navigation, URI and span alike.
    let tma_pos = pos_after(app_src, "call ns::tmaFn", 5);
    let target = service
        .definition(&app_uri, tma_pos)
        .expect("a .tma sibling's non-local func navigates");
    assert_eq!(target.uri, file_uri(&dir.join("sibling.tma")), "{target:?}");
    assert_eq!(target.span, span_of(tma_src, "ns::tmaFn"), "{target:?}");

    // The `.tmo` sibling's defined symbol carries no source location at
    // all — the overlay OWNS the name (it resolves calls, refines
    // diagnostics), but navigation must answer null rather than guess.
    let tmo_pos = pos_after(app_src, "call ns2::tmoFn", 5);
    assert_eq!(
        service.definition(&app_uri, tmo_pos),
        None,
        "a `.tmo`-backed overlay symbol carries no source location to jump to"
    );
}

#[test]
fn a_shadowing_sibling_wins_over_the_stdlib_at_every_leg() {
    // The plan's governing rule, stated in the brief: resolution order is
    // local file, then declared sources, then declared libraries
    // (first-wins), then the embedded stdlib LAST — mirroring the
    // linker's own user-object-beats-library precedence. A sibling that
    // mangles to the IDENTICAL qualified name an embedded roster entry
    // carries is the exact shadowed case the sibling crate shipped
    // BACKWARDS (stdlib-first) and needed a dedicated follow-up fix for.
    // The faithfulness fixtures elsewhere in this file only cover the
    // UNSHADOWED path, where overlay and stdlib agree by construction —
    // this is the one where they disagree, and the sibling's own answer
    // must win at every leg: navigation, hover, and completion's dedupe.
    let dir = unique_tmp_dir("shadow-stdlib");
    fs::write(
        dir.join("tmt.json"),
        r#"{"project":{"targets":{"app":{"sources":["app.tmc","helper.tmc"]}}}}"#,
    )
    .unwrap();
    // Nested namespaces mangle to `full_name`'s "::"-joined form exactly
    // like the embedded roster's own entries — `std::binaryNumbers::
    // goToNumber` here is the SAME string the roster carries for the
    // real routine of that name.
    let helper_src = "\
alphabet b { '_', '0' }
namespace std {
  namespace binaryNumbers {
? Shadows the embedded stdlib routine of the same qualified name.
    export routine goToNumber(tape num: b) { entry state s { [*] -> return; } }
  }
}
";
    fs::write(dir.join("helper.tmc"), helper_src).unwrap();
    let helper_uri = file_uri(&dir.join("helper.tmc"));

    let app_src = "\
alphabet bits { '_', '1' }
machine {
  tape ctl: bits;
  entry state main { [*] -> call std::binaryNumbers::goToNumber() then main; }
}
";
    let mut service = TmcLanguageService::new();
    let app_uri = file_uri(&dir.join("app.tmc"));
    service.did_update(&app_uri, app_src);
    let pos = pos_after(app_src, "call std::binaryNumbers::goToNumber", 5);

    // Navigation: the SIBLING's own declaration, never the materialized
    // stdlib copy.
    let target = service
        .definition(&app_uri, pos)
        .expect("the shadowing sibling still resolves");
    assert_eq!(target.uri, helper_uri, "{target:?}");
    assert_eq!(target.span, span_of(helper_src, "goToNumber"), "{target:?}");

    // Hover: the SIBLING's own doc line, never the embedded routine's.
    let hover = service
        .hover(&app_uri, pos)
        .expect("the shadowing sibling still hovers");
    assert!(
        hover.text.contains("Shadows the embedded stdlib routine"),
        "{hover:?}"
    );

    // Completion: exactly ONE candidate for the shadowed label — the
    // dedupe proof, not merely a shadow-blind union that would surface it
    // twice.
    let call_head = "\
alphabet bits { '_', '1' }
machine {
  tape ctl: bits;
  entry state main { [*] -> call ";
    let call_tail = "() then main;\n  }\n}\n";
    let call_got = complete_typing_in(
        &mut service,
        &app_uri,
        call_head,
        "std::binaryNumbers::goToNumber",
        call_tail,
    );
    let matches = call_got
        .iter()
        .filter(|c| c.label == "std::binaryNumbers::goToNumber")
        .count();
    assert_eq!(
        matches,
        1,
        "the shadowed label must surface exactly once: {:?}",
        labels(&call_got)
    );

    // Positive control: a DIFFERENT, genuinely UNSHADOWED roster entry
    // still appears — proving the whole stdlib completion lane didn't
    // die, rather than merely deduplicating one label away.
    assert!(
        labels(&call_got).contains(&"std::binaryNumbersBare::plusOne".to_string()),
        "{:?}",
        labels(&call_got)
    );
}

#[test]
fn stdlib_false_gates_std_surfaces_tm_side() {
    // Task 6/7's matrix, TM spellings: every std surface this service
    // offers — completion, hover, go-to-definition — turns off under a
    // manifest's `"stdlib": false`, while a live positive control (a
    // genuinely LOCAL declaration, in the SAME project) keeps every
    // feature working. Without each control, a regression that broke a
    // feature OUTRIGHT whenever `state.overlay` is `Some` would pass every
    // negative assertion here vacuously.
    let dir = unique_tmp_dir("stdlib-false-tm");
    fs::write(
        dir.join("tmt.json"),
        r#"{"project":{"stdlib":false,"targets":{"app":{"sources":["app.tmc"]}}}}"#,
    )
    .unwrap();

    let app_src = "\
alphabet bits { '_', '1' }

?Local doc.
routine localR(tape t: bits) { entry state s { [*] -> return; } }

use std::binaryNumbers::goToNumber;

machine {
  tape ctl: bits;
  entry state main {
    ['_'] -> call localR(t = ctl) then main;
    ['1'] -> call plusOne() then main;
  }
}
";
    let mut service = TmcLanguageService::new();
    let app_uri = file_uri(&dir.join("app.tmc"));
    let diags = service.did_update(&app_uri, app_src);

    // Gate 1 + control: no "std" root in the `use` path root list, but the
    // genuinely local export is unaffected.
    let use_got = labels(&service.completion(&app_uri, pos_after(app_src, "use ", 4)));
    assert!(!use_got.contains(&"std".to_string()), "{use_got:?}");
    assert!(
        use_got.contains(&"localR".to_string()),
        "a genuinely local export must still be offered under stdlib:false: {use_got:?}"
    );

    // Gate 2 + control: no qualified `std::` label at a call target, but
    // the local routine still completes there.
    let call_head = "\
alphabet bits { '_', '1' }

routine localR(tape t: bits) { entry state s { [*] -> return; } }

machine {
  tape ctl: bits;
  entry state main { [*] -> call ";
    let call_tail = "(t = ctl) then main;\n  }\n}\n";
    let call_got = labels(&complete_typing_in(
        &mut service,
        &app_uri,
        call_head,
        "localR",
        call_tail,
    ));
    assert!(
        !call_got.iter().any(|l| l.starts_with("std::")),
        "{call_got:?}"
    );
    assert!(
        call_got.contains(&"localR".to_string()),
        "a genuinely local routine must still complete under stdlib:false: {call_got:?}"
    );

    // Gates 3 + 4 + controls: no std hover, no materialized go-to-
    // definition — but a genuinely local call in the SAME project keeps
    // hovering and navigating.
    let std_src = "\
alphabet bits { '_', '1' }
use std::binaryNumbers::goToNumber;
machine {
  tape ctl: bits;
  entry state main { [*] -> call std::binaryNumbers::goToNumber() then main; }
}
";
    service.did_update(&app_uri, std_src);
    let std_pos = pos_after(std_src, "call std::binaryNumbers::goToNumber", 5);
    assert_eq!(
        service.hover(&app_uri, std_pos),
        None,
        "stdlib:false kills std hover"
    );
    assert_eq!(
        service.definition(&app_uri, std_pos),
        None,
        "stdlib:false kills the materialized jump"
    );

    let local_src = "\
alphabet bits { '_', '1' }

?Local doc.
routine localR(tape t: bits) { entry state s { [*] -> return; } }

machine {
  tape ctl: bits;
  entry state main { [*] -> call localR(t = ctl) then main; }
}
";
    service.did_update(&app_uri, local_src);
    let local_pos = pos_after(local_src, "call localR", 5);
    let local_hover = service
        .hover(&app_uri, local_pos)
        .expect("a local, non-std call still hovers under stdlib:false");
    assert!(local_hover.text.contains("Local doc."), "{local_hover:?}");
    let local_target = service
        .definition(&app_uri, local_pos)
        .expect("a local, non-std call still navigates under stdlib:false");
    assert_eq!(local_target.uri, app_uri);
    assert_eq!(local_target.span, span_of(local_src, "localR"));

    // Gate 5: a bare std-shaped call still warns — `stdlib:false` only
    // ever removes `std::`-keyed names from `Overlay::defined_names`; a
    // BARE `plusOne` (unrelated to the `goToNumber` import above, so
    // `resolve_written`'s import-binding leg can't quietly resolve it
    // either) was never one of those — the stdlib exports only namespaced
    // paths, never a bare name — so nothing new is suppressed here.
    assert!(
        diags
            .iter()
            .any(|d| d.code == Some("undeclared-external") && d.message.contains("`plusOne`")),
        "a bare plusOne() is not a std:: name — must keep warning: {diags:?}"
    );
}

// --- Task 17: overlay integration matrix — the remaining cells the plan
// calls for, each a real `TmcLanguageService` driven through `did_update` +
// feature calls over a temp tree. Two cells the matrix names are already
// pinned elsewhere and are NOT repeated here:
//   - `stdlib_false_gates_std_surfaces_tm_side` (above) is already the
//     stdlib:false matrix, TM spellings, with a live control on every gate.
//   - `overlay::tests::broken_sibling_contributes_nothing_others_still_do`
//     already pins one unparseable sibling contributing nothing while a
//     fine sibling's export still does — at the `build_overlay` layer,
//     which is the exact claim; a service-level repeat would add nothing.
// The TM-only overlay-leak negative is likewise not a new test here — see
// the comment above `call_and_bind_target_completion_offers_sibling_
// exported_routines`, which already covers it. ---

#[test]
fn manifest_edit_changes_the_next_answer() {
    let dir = unique_tmp_dir("manifest-edit");
    let manifest_path = dir.join("tmt.json");
    fs::write(
        &manifest_path,
        r#"{"project":{"targets":{"app":{"sources":["app.tmc","helper.tmc"]}}}}"#,
    )
    .unwrap();
    fs::write(
        dir.join("helper.tmc"),
        "alphabet b { '_', '0' }\nexport routine bare(tape t: b) { entry state s { [*] -> return; } }\n",
    )
    .unwrap();

    let mut service = TmcLanguageService::new();
    let app_uri = file_uri(&dir.join("app.tmc"));
    // A fully-formed `use` statement already in place — `Context::UsePath`
    // doesn't care what the path already spells, only that the cursor sits
    // somewhere inside one (mirrors `use_path_completion_offers_sibling_
    // namespaces_and_routines`, which positions the cursor the same way).
    let app_src = "\
alphabet bits { '_', '1' }
use std::binaryNumbers::goToNumber;

machine {
  tape t: bits;
  entry state s { [*] -> stop; }
}
";
    let pos = pos_after(app_src, "use ", 4);

    service.did_update(&app_uri, app_src);
    let before = labels(&service.completion(&app_uri, pos));
    assert!(
        before.contains(&"bare".to_string()),
        "the sibling's export starts out visible: {before:?}"
    );

    // Rewrite the manifest dropping helper.tmc from the target, with a
    // guaranteed-newer mtime (the manifest cache is mtime-keyed; the
    // filesystem's own timestamp granularity is not to be trusted in a
    // fast test — mirrors `overlay::tests::manifest_cache_is_mtime_keyed_
    // and_bounded`'s own rewrite sequence).
    let old_mtime = fs::metadata(&manifest_path).unwrap().modified().unwrap();
    fs::write(
        &manifest_path,
        r#"{"project":{"targets":{"app":{"sources":["app.tmc"]}}}}"#,
    )
    .unwrap();
    fs::File::options()
        .write(true)
        .open(&manifest_path)
        .unwrap()
        .set_modified(old_mtime + Duration::from_secs(2))
        .unwrap();

    // Core's own `republish_all` re-runs `did_update` on every open
    // document when a watched `tmt.json` changes in production; a test
    // drives that re-update itself.
    service.did_update(&app_uri, app_src);

    // A positive control before re-checking the narrowed completion list:
    // `app.tmc` is still a target member post-edit (the rewritten manifest
    // only dropped `helper.tmc` from the sources, not the target itself),
    // so the overlay must still be `Some` — a regression where the second
    // `did_update` degraded to `overlay: None` (project membership lost
    // outright, not merely narrowed) would also make `!after.contains
    // (&"bare")` pass below, since a `None` overlay offers no sibling
    // exports either.
    assert!(
        service.docs.get(&app_uri).unwrap().overlay.is_some(),
        "app.tmc must still resolve to a project overlay after the edit"
    );

    let after = labels(&service.completion(&app_uri, pos));
    assert!(
        !after.contains(&"bare".to_string()),
        "the manifest edit must drop the now-undeclared sibling: {after:?}"
    );
}

#[test]
fn untitled_documents_keep_the_single_file_view() {
    // An untitled buffer has no filesystem path at all, so it can never be
    // discovered as a project member. This closes the two legs that
    // actually consume a `None` overlay: cross-file refinement stays off
    // (a bare undeclared call keeps warning), and the embedded stdlib
    // surface (hover, go-to-definition) is exactly what it was before the
    // overlay feature existed.
    let src = "\
alphabet bits { '_', '1' }
machine {
  tape t: bits;
  entry state s {
    ['_'] -> call ghost() then g;
    ['1'] -> call std::binaryNumbers::goToNumber() then g;
  }
  state g { [*] -> stop; }
}
";
    let (mut service, uri) = opened(src);
    let diags = service.did_update(&uri, src);

    assert!(
        service.docs.get(&uri).unwrap().overlay.is_none(),
        "no `file:` path at all — single-file degrade"
    );
    assert!(
        diags
            .iter()
            .any(|d| d.code == Some("undeclared-external") && d.message.contains("`ghost`")),
        "no overlay to refine the bare call away: {diags:?}"
    );

    let std_pos = pos_after(src, "call std::binaryNumbers::goToNumber", 5);
    let hover = service
        .hover(&uri, std_pos)
        .expect("std:: hover is untouched without an overlay");
    // Deliberately coupled to `goToNumber`'s own doc prose (the same
    // substring `hover_on_a_std_call_target_returns_its_doc` pins in full)
    // — this test's job is proving the untitled route reaches the SAME
    // embedded doc, not re-deriving its wording.
    assert!(
        hover
            .text
            .contains("Walk right to the current number's end marker"),
        "the embedded stdlib's own doc: {hover:?}"
    );

    let target = service
        .definition(&uri, std_pos)
        .expect("std:: go-to-definition is untouched without an overlay");
    let std_uri =
        crate::stdlib::materialized_std_uri().expect("materialization succeeds in this env");
    assert_eq!(target.uri, std_uri);
}

#[test]
fn dotdot_declared_source_gets_exact_features_when_opened_from_the_tree() {
    // Mirrors `overlay::tests::dotdot_membership_resolves_lexically`,
    // driven through the real service instead of `project_view` directly:
    // `proj/tmt.json` declares `"../shared.tmc"`, and `proj/app.tmc` — a
    // member of `proj`'s own `app` target — is the document actually
    // opened, so discovery starts at `proj/`, the opened document's own
    // directory. The sibling's export is reached through a `::`-qualified
    // name (`sh::shared`): `external_path` never resolves a bare,
    // unqualified reference (`definition_reaches_a_tma_sibling_and_tmo_
    // names_navigate_null` documents that limitation), so a bare call is
    // needed for navigation to even have something to resolve — but a
    // QUALIFIED name (any `::`-qualified reference, not only a resolved
    // one) never produces `undeclared-external` in the first place
    // (`Analysis::warn_undeclared_if_bare`'s own `!name.contains("::")`
    // gate), so this test carries the `../`-declared-source claim through
    // `definition`/`hover` alone rather than through a diagnostics
    // assertion that would pass identically with no `../` source declared
    // at all (confirmed by temporarily dropping the declaration and
    // rerunning: `definition` failed exactly as expected, at its own
    // `.expect`, with no diagnostics assertion anywhere near the failure).
    let root = unique_tmp_dir("dotdot-service");
    let proj = root.join("proj");
    fs::create_dir_all(&proj).unwrap();
    fs::write(
        proj.join("tmt.json"),
        r#"{"project":{"sources":["../shared.tmc"],"targets":{"app":{"sources":["app.tmc"]}}}}"#,
    )
    .unwrap();
    let shared_src = "\
alphabet b { '_', '0' }
namespace sh {
?Shared doc.
  export routine shared(tape num: b) { entry state s { [*] -> return; } }
}
";
    fs::write(root.join("shared.tmc"), shared_src).unwrap();

    let mut service = TmcLanguageService::new();
    let app_uri = file_uri(&proj.join("app.tmc"));
    let app_src = "\
alphabet bits { '_', '1' }
machine {
  tape t: bits;
  entry state main { [*] -> call sh::shared() then main; }
}
";
    service.did_update(&app_uri, app_src);

    let pos = pos_after(app_src, "call sh::shared", 5);
    let target = service
        .definition(&app_uri, pos)
        .expect("../shared.tmc's export navigates");
    assert_eq!(
        target.uri,
        file_uri(&root.join("shared.tmc")),
        "must resolve to shared.tmc's own file, not a literal `..` segment"
    );
    assert_eq!(target.span, span_of(shared_src, "shared"), "{target:?}");

    let hover = service
        .hover(&app_uri, pos)
        .expect("../shared.tmc's own doc surfaces through the overlay");
    assert!(hover.text.contains("Shared doc."), "{hover:?}");
}

#[test]
fn semantic_tokens_are_unchanged_by_the_overlay() {
    // R2: the overlay must never touch semantic tokens — a purely lexical
    // layer with no resolution tier. The SAME text, analyzed once with no
    // overlay at all and once inside a real project whose sibling
    // genuinely resolves the fixture's own bare call, must produce
    // byte-for-byte identical token streams.
    let app_src = "\
alphabet bits { '_', '1' }
machine {
  tape ctl: bits;
  entry state main { [*] -> call helper() then main; }
}
";
    let (mut plain_service, plain_uri) = opened(app_src);
    let without_overlay = plain_service
        .semantic_tokens(&plain_uri)
        .expect("tokens without an overlay");

    let dir = unique_tmp_dir("tokens-overlay");
    fs::write(
        dir.join("tmt.json"),
        r#"{"project":{"targets":{"app":{"sources":["app.tmc","helper.tmc"]}}}}"#,
    )
    .unwrap();
    fs::write(
        dir.join("helper.tmc"),
        "alphabet b { '_', '0' }\nexport routine helper(tape t: b) { entry state s { [*] -> return; } }\n",
    )
    .unwrap();
    let mut overlaid_service = TmcLanguageService::new();
    let app_uri = file_uri(&dir.join("app.tmc"));
    overlaid_service.did_update(&app_uri, app_src);
    assert!(
        overlaid_service
            .docs
            .get(&app_uri)
            .unwrap()
            .overlay
            .is_some(),
        "sanity: a real, active overlay exists here"
    );
    let with_overlay = overlaid_service
        .semantic_tokens(&app_uri)
        .expect("tokens with an overlay");

    assert_eq!(without_overlay, with_overlay);
}
