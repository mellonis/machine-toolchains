//! The `.tma` service battery: the parity surface, the TM-specific extras, the
//! CLI/server agreement on findings, and the robustness bar a language server
//! has to clear — a panic on a request path takes the user's editor with it.

use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::json;

use mtc_core::lsp::CandidateKind;

use super::*;

/// A fresh scratch directory, unique per call. This crate has no shared
/// test-support module — each file defines its own local helpers.
fn unique_tmp_dir(label: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "tmt-lsp-tma-test-{label}-{}-{n}",
        std::process::id()
    ));
    fs::create_dir_all(&dir).expect("scratch dir");
    dir
}

fn file_uri(path: &Path) -> String {
    format!("file://{}", path.display())
}

const URI: &str = "untitled:Untitled-1";

fn opened(src: &str) -> TmaLanguageService {
    let mut service = TmaLanguageService::new();
    service.did_update(URI, src);
    service
}

fn labels(candidates: &[Candidate]) -> Vec<String> {
    candidates.iter().map(|c| c.label.clone()).collect()
}

/// The full frames + tables surface, clean: every reference resolves and no
/// rule fires. `main` dispatches through a match table into a framed call on
/// `helper`, which returns through the descriptor's exits. Line and column
/// numbers are asserted literally below, so the layout is load-bearing:
///
/// ```text
///  1 .routine main …          11 .func main
///  2 .routine helper …        12         rd
///  3 .section tables          13         mtc     T0
///  4 T0: .row [1, 1]          14         djmp    D0
///  5     .row [*, *]          15 hit:    call.m  helper, F0
///  6 D0: .targets hit, miss   16 done:   stp
///  7 F0: .frame tapes=(1, 0)  17 other:  hlt
///  8     .map 0, rmap=(1->1)  18 miss:   hlt
///  9     .exits done, other   19 .func helper
/// 10 .section code            20         wr      [1, -]
///                             21         retx    #1
/// ```
const FULL: &str = "\
.routine main, tapes=2, alpha=(2, 2)
.routine helper, tapes=2, alpha=(2, 2)
.section tables
T0: .row [1, 1]
    .row [*, *]
D0: .targets hit, miss
F0: .frame tapes=(1, 0)
    .map 0, rmap=(1->1)
    .exits done, other
.section code
.func main
        rd
        mtc     T0
        djmp    D0
hit:    call.m  helper, F0
done:   stp
other:  hlt
miss:   hlt
.func helper
        wr      [1, -]
        retx    #1
";

// Definition sites in FULL, by construction of the fixture above.
fn t0_label() -> Span {
    Span::new(4, 1, 4, 3)
}
fn d0_label() -> Span {
    Span::new(6, 1, 6, 3)
}
fn f0_label() -> Span {
    Span::new(7, 1, 7, 3)
}
fn hit_label() -> Span {
    Span::new(15, 1, 15, 4)
}
fn done_label() -> Span {
    Span::new(16, 1, 16, 5)
}
fn helper_func_name() -> Span {
    Span::new(19, 7, 19, 13)
}
fn main_func_name() -> Span {
    Span::new(11, 7, 11, 11)
}

// Reference sites in FULL.
const MTC_OPERAND: Pos = Pos { line: 13, col: 18 };
const DJMP_OPERAND: Pos = Pos { line: 14, col: 18 };
const CALLM_TARGET: Pos = Pos { line: 15, col: 18 };
const CALLM_FRAME: Pos = Pos { line: 15, col: 26 };
const TARGETS_ENTRY: Pos = Pos { line: 6, col: 15 };
const EXITS_ENTRY: Pos = Pos { line: 9, col: 13 };

// ---- the parity surface ----

#[test]
fn advertises_the_tma_language_surface() {
    let service = TmaLanguageService::new();
    assert_eq!(service.language_id(), "tma");
    assert_eq!(service.extensions(), &[".tma"]);
    assert_eq!(service.trigger_characters(), &['@', '.', ',', '[']);
    assert_eq!(service.watched_globs(), &["**/tmt.json"]);
}

#[test]
fn a_clean_document_reports_nothing() {
    let mut service = TmaLanguageService::new();
    let diags = service.did_update(URI, FULL);
    assert!(diags.is_empty(), "{diags:?}");
}

#[test]
fn an_unknown_mnemonic_is_one_error_at_its_own_word() {
    let mut service = TmaLanguageService::new();
    let diags = service.did_update(URI, ".func main\n        bogus\n");

    assert_eq!(diags.len(), 1, "{diags:?}");
    assert_eq!(diags[0].severity, ServiceSeverity::Error);
    assert_eq!(diags[0].source, "tmt");
    assert_eq!(diags[0].code, Some("unknown-mnemonic"));
    assert_eq!(diags[0].span, Span::new(2, 9, 2, 14));
}

#[test]
fn a_lint_finding_rides_the_lint_channel() {
    // Dead code after `stp` is core's `unreachable-code` rule, reaching the
    // service through the same `lint_tma` entry the CLI uses.
    let mut service = TmaLanguageService::new();
    let diags = service.did_update(
        URI,
        ".routine main, tapes=2, alpha=(2, 2)\n.section code\n.func main\n        stp\n        nop\n",
    );
    assert_eq!(diags.len(), 1, "{diags:?}");
    assert_eq!(diags[0].severity, ServiceSeverity::Warning);
    assert_eq!(diags[0].source, "tmt lint");
    assert_eq!(diags[0].code, Some("unreachable-code"));
}

#[test]
fn did_close_forgets_the_document() {
    let mut service = opened(FULL);
    service.did_close(URI);
    assert_eq!(service.document_symbols(URI), None);
    assert_eq!(service.semantic_tokens(URI), None);
    assert_eq!(service.format(URI), None);
    assert!(service.completion(URI, Pos { line: 1, col: 1 }).is_empty());
}

#[test]
fn hover_is_permanently_none() {
    // Assembly text has no doc-line grammar; the sibling `.pma` service makes
    // the same call.
    let mut service = opened(FULL);
    assert_eq!(service.hover(URI, CALLM_TARGET), None);
}

#[test]
fn formatting_delegates_to_the_canonical_grid_and_is_idempotent() {
    // Deliberately ragged: a spaced colon and inconsistent indentation the
    // grid printer normalizes.
    const SCRAMBLED: &str =
        ".routine main, tapes=2, alpha=(2, 2)\n.section code\n.func main\nL1 :  rd\n stp\n";
    let mut service = opened(SCRAMBLED);

    let via_service = service.format(URI).expect("valid source formats");
    let direct = format_asm_with(SCRAMBLED, tm1_syntax().caps).expect("valid source formats");
    assert_eq!(via_service, direct, "the single-source contract");
    assert_ne!(via_service, SCRAMBLED, "sanity: really was scrambled");

    service.did_update(URI, &via_service);
    assert_eq!(
        service.format(URI).as_deref(),
        Some(via_service.as_str()),
        "idempotent through the service"
    );
}

#[test]
fn formatting_a_document_that_does_not_assemble_returns_nothing() {
    let mut service = opened("<not assembly>\n");
    assert_eq!(service.format(URI), None);
}

// ---- configuration ----

const DEAD_CODE: &str =
    ".routine main, tapes=2, alpha=(2, 2)\n.section code\n.func main\n        stp\n        nop\n";

#[test]
fn a_project_file_suppresses_a_rule_and_the_ide_channel_unions_with_it() {
    let dir = unique_tmp_dir("allow-union");
    fs::write(
        dir.join("tmt.json"),
        r#"{"lint":{"allow":["unreachable-code"]}}"#,
    )
    .expect("write config");
    let uri = file_uri(&dir.join("prog.tma"));

    let mut service = TmaLanguageService::new();
    assert!(
        service.did_update(&uri, DEAD_CODE).is_empty(),
        "the project file suppresses the rule"
    );

    // Union, not a cascade: an unrelated code from the IDE channel leaves the
    // project file's own suppression standing.
    service.did_change_config(json!({"lint": {"allow": ["line-too-long"]}}));
    assert!(service.did_update(&uri, DEAD_CODE).is_empty());
}

#[test]
fn the_ide_channel_alone_suppresses_a_rule() {
    let mut service = TmaLanguageService::new();
    service.did_change_config(json!({"lint": {"allow": ["unreachable-code"]}}));
    assert!(service.did_update(URI, DEAD_CODE).is_empty());
}

#[test]
fn a_wrapped_configuration_section_is_unwrapped() {
    // Clients that forward whole configuration sections wrap them under the
    // server's own key.
    let mut service = TmaLanguageService::new();
    service.did_change_config(json!({"tmt": {"lint": {"allow": ["unreachable-code"]}}}));
    assert!(service.did_update(URI, DEAD_CODE).is_empty());
}

#[test]
fn an_unknown_ide_rule_code_becomes_an_invalid_config_warning() {
    let mut service = TmaLanguageService::new();
    service.did_change_config(json!({"lint": {"allow": ["no-such-rule"]}}));
    let diags = service.did_update(URI, FULL);

    assert_eq!(diags.len(), 1, "{diags:?}");
    assert_eq!(diags[0].code, Some("invalid-config"));
    assert_eq!(diags[0].severity, ServiceSeverity::Warning);
    assert!(
        diags[0].message.contains("no-such-rule"),
        "{}",
        diags[0].message
    );
}

#[test]
fn a_tma_only_rule_code_is_valid_in_the_shared_allow_namespace() {
    // One allow namespace serves both languages, so a `.tma` addition's code
    // validates here rather than reading as unknown.
    let mut service = TmaLanguageService::new();
    service.did_change_config(json!({"lint": {"allow": ["retx-exit-bounds"]}}));
    let diags = service.did_update(URI, FULL);
    assert!(
        diags.iter().all(|d| d.code != Some("invalid-config")),
        "{diags:?}"
    );
}

#[test]
fn an_invalid_project_file_surfaces_invalid_config_first() {
    let dir = unique_tmp_dir("invalid-config");
    let config_path = dir.join("tmt.json");
    fs::write(&config_path, r#"{"lints":{}}"#).expect("write config");

    let mut service = TmaLanguageService::new();
    let uri = file_uri(&dir.join("prog.tma"));
    let diags = service.did_update(&uri, DEAD_CODE);

    assert_eq!(diags.len(), 2, "{diags:?}");
    assert_eq!(diags[0].code, Some("invalid-config"));
    assert_eq!(diags[0].span, Span::point(1, 1));
    assert!(
        diags[0]
            .message
            .contains(&config_path.display().to_string()),
        "names the file at fault: {}",
        diags[0].message
    );
    // The lint channel still ran with the remaining sources.
    assert_eq!(diags[1].code, Some("unreachable-code"));
}

// ---- CLI / server agreement ----

#[test]
fn the_service_publishes_exactly_what_the_cli_lint_entry_reports() {
    // Both surfaces route through `lint_tma`, so this is a real equality
    // rather than two implementations agreeing by luck.
    let cli_findings = lint_tma(DEAD_CODE, &[]).expect("assembles");
    let mut service = TmaLanguageService::new();
    let published = service.did_update(URI, DEAD_CODE);

    let cli_codes: Vec<&str> = cli_findings.iter().map(|d| d.code).collect();
    let served_codes: Vec<&str> = published.iter().filter_map(|d| d.code).collect();
    assert_eq!(cli_codes, served_codes);
    assert!(!cli_codes.is_empty(), "sanity: the fixture does report");
}

#[test]
fn unused_label_stays_suppressed_on_the_server_exactly_as_on_the_cli() {
    // Every code label in FULL is reached only through a `.targets` dispatch
    // entry or a `.exits` descriptor — references core's own rule cannot see,
    // which is why the `.tma` path suppresses it. The editor must agree with
    // the command line rather than flagging all four as unused.
    let mut service = TmaLanguageService::new();
    let diags = service.did_update(URI, FULL);
    assert!(
        diags.iter().all(|d| d.code != Some("unused-label")),
        "{diags:?}"
    );
    assert!(
        lint_tma(FULL, &[])
            .expect("assembles")
            .iter()
            .all(|d| d.code != "unused-label"),
        "sanity: the CLI agrees"
    );
}

// ---- the TM extras: table and frame navigation ----

#[test]
fn an_mtc_operand_navigates_to_the_match_tables_own_label() {
    let mut service = opened(FULL);
    let target = service
        .definition(URI, MTC_OPERAND)
        .expect("T0 is defined in this document");
    assert_eq!(target.uri, URI);
    assert_eq!(target.span, t0_label(), "the label, not the row");
    assert_eq!(target.origin, Some(Span::new(13, 17, 13, 19)));
}

#[test]
fn a_djmp_operand_navigates_to_the_dispatch_tables_own_label() {
    let mut service = opened(FULL);
    let target = service
        .definition(URI, DJMP_OPERAND)
        .expect("D0 is defined in this document");
    assert_eq!(target.span, d0_label());
}

#[test]
fn a_framed_calls_two_operands_navigate_to_the_callee_and_to_the_frame() {
    let mut service = opened(FULL);

    let callee = service
        .definition(URI, CALLM_TARGET)
        .expect("helper is defined in this document");
    assert_eq!(
        callee.span, helper_func_name(),
        "the `.func helper` body, preferred over the `.routine` signature"
    );

    let frame = service
        .definition(URI, CALLM_FRAME)
        .expect("F0 is declared in this document");
    assert_eq!(frame.span, f0_label());
}

#[test]
fn a_call_target_with_no_local_body_falls_back_to_its_routine_signature() {
    // `absent` is declared by a `.routine` signature and defined in another
    // translation unit; the signature is the best answer this file has.
    let src = "\
.routine main, tapes=2, alpha=(2, 2)
.routine absent, tapes=2, alpha=(2, 2)
.section code
.func main
        call    absent
        stp
";
    let mut service = opened(src);
    let target = service
        .definition(URI, Pos { line: 5, col: 18 })
        .expect("resolves to the signature");
    assert_eq!(target.span, Span::new(2, 10, 2, 16));
}

#[test]
fn a_dispatch_entry_navigates_back_into_the_code_it_targets() {
    // The arrow that makes a dispatch table navigable at all: from a
    // `.targets` entry to the code label it names.
    let mut service = opened(FULL);
    let target = service
        .definition(URI, TARGETS_ENTRY)
        .expect("hit is a code label");
    assert_eq!(target.span, hit_label());
}

#[test]
fn an_exit_target_navigates_back_into_the_code_it_returns_to() {
    let mut service = opened(FULL);
    let target = service
        .definition(URI, EXITS_ENTRY)
        .expect("done is a code label");
    assert_eq!(target.span, done_label());
}

#[test]
fn a_label_reference_never_crosses_into_a_same_named_label_in_another_function() {
    let src = "\
.routine main, tapes=2, alpha=(2, 2)
.section code
.func f
L1:     nop
        jmp     L1
.func g
L1:     nop
        jmp     L1
";
    let mut service = opened(src);
    let f_label = Span::new(4, 1, 4, 3);
    let g_label = Span::new(7, 1, 7, 3);

    let from_f = service
        .definition(URI, Pos { line: 5, col: 18 })
        .expect("f's own jmp resolves inside f");
    assert_eq!(from_f.span, f_label);

    let from_g = service
        .definition(URI, Pos { line: 8, col: 18 })
        .expect("g's own jmp resolves inside g");
    assert_eq!(from_g.span, g_label);
}

#[test]
fn a_templated_rept_operand_resolves_to_nothing() {
    // `L{v}` names a template, not an identifier — the expanded spellings
    // exist only after lowering, so navigation stays quiet rather than
    // reporting a false miss.
    let src = "\
.routine main, tapes=2, alpha=(2, 2)
.section code
.func main
.rept v, 0, 1
        jmp     L{v}
.endr
        stp
";
    let mut service = opened(src);
    assert_eq!(service.definition(URI, Pos { line: 5, col: 18 }), None);
}

#[test]
fn navigation_still_answers_on_a_document_that_fails_to_assemble() {
    let src = FULL.replace("        rd\n", "        bogus\n");
    let mut service = TmaLanguageService::new();
    let diags = service.did_update(URI, &src);
    assert!(
        diags.iter().any(|d| d.code == Some("unknown-mnemonic")),
        "sanity: {diags:?}"
    );

    let target = service
        .definition(URI, MTC_OPERAND)
        .expect("the total CST still resolves T0 despite the broken line");
    assert_eq!(target.span, t0_label());
}

// ---- the TM extras: descriptor field diagnostics ----

#[test]
fn a_map_naming_a_tape_the_frame_does_not_have_is_an_error() {
    let src = "\
.routine main, tapes=2, alpha=(2, 2)
.section tables
F0: .frame tapes=(1)
    .map 3, rmap=(1->1)
.section code
.func main
        stp
";
    let mut service = TmaLanguageService::new();
    let diags = service.did_update(URI, src);
    let finding = diags
        .iter()
        .find(|d| d.message.contains("frame arity"))
        .unwrap_or_else(|| panic!("{diags:?}"));
    assert_eq!(finding.severity, ServiceSeverity::Error);
    assert_eq!(finding.code, Some("bad-frame"));
}

#[test]
fn an_orphan_map_and_an_orphan_exits_are_both_errors() {
    let src = "\
.routine main, tapes=2, alpha=(2, 2)
.section tables
    .map 0, rmap=(1->1)
    .exits done
.section code
.func main
done:   stp
";
    let mut service = TmaLanguageService::new();
    let diags = service.did_update(URI, src);
    assert!(
        diags
            .iter()
            .any(|d| d.message.contains("`.map` has no preceding")),
        "{diags:?}"
    );
    assert!(
        diags
            .iter()
            .any(|d| d.message.contains("`.exits` has no preceding")),
        "{diags:?}"
    );
}

#[test]
fn every_broken_descriptor_field_surfaces_at_once_not_just_the_first() {
    // The whole point of the CST tier: the assembler stops at the first
    // offending descriptor, so three defects in one file would come back one
    // edit at a time. All three must show together.
    let src = "\
.routine main, tapes=2, alpha=(2, 2)
.section tables
F0: .frame tapes=(1, 0)
    .map 9, rmap=(1->1)
    .map 9, rmap=(1->1)
    .exits done
    .exits done
.section code
.func main
done:   stp
";
    let mut service = TmaLanguageService::new();
    let diags = service.did_update(URI, src);
    let messages: Vec<&str> = diags
        .iter()
        .filter(|d| d.code == Some("bad-frame"))
        .map(|d| d.message.as_str())
        .collect();
    assert!(
        messages.iter().any(|m| m.contains("frame arity")),
        "{messages:?}"
    );
    assert!(
        messages.iter().any(|m| m.contains("duplicate `.map")),
        "{messages:?}"
    );
    assert!(
        messages.iter().any(|m| m.contains("at most once per frame")),
        "{messages:?}"
    );
}

#[test]
fn descriptor_findings_survive_an_unrelated_fatal_elsewhere_in_the_file() {
    // The other half of the point: an unknown mnemonic stops lowering dead
    // before it ever reaches the descriptor, so without the CST tier the
    // broken `.map` below would be invisible in the editor until the
    // unrelated typo above it was fixed.
    let src = "\
.routine main, tapes=2, alpha=(2, 2)
.section code
.func main
        bogus
.section tables
F0: .frame tapes=(1)
    .map 4, rmap=(1->1)
";
    let mut service = TmaLanguageService::new();
    let diags = service.did_update(URI, src);
    assert!(
        diags.iter().any(|d| d.code == Some("unknown-mnemonic")),
        "{diags:?}"
    );
    assert!(
        diags
            .iter()
            .any(|d| d.code == Some("bad-frame") && d.message.contains("frame arity")),
        "the descriptor defect is visible behind the fatal: {diags:?}"
    );
}

#[test]
fn a_descriptor_defect_that_is_itself_the_fatal_is_reported_once() {
    // When lowering DOES reach the bad descriptor it becomes the published
    // fatal; the CST tier must not then report the same span a second time.
    let src = "\
.routine main, tapes=2, alpha=(2, 2)
.section tables
F0: .frame tapes=(1)
    .map 4, rmap=(1->1)
.section code
.func main
        stp
";
    let mut service = TmaLanguageService::new();
    let diags = service.did_update(URI, src);
    let arity_findings = diags
        .iter()
        .filter(|d| d.message.contains("frame arity"))
        .count();
    assert_eq!(arity_findings, 1, "reported exactly once: {diags:?}");
}

#[test]
fn a_one_way_pair_is_rejected_in_wmap_and_accepted_in_rmap() {
    // `=>` is read-direction only: legal in `rmap`, meaningless in `wmap`.
    let with_wmap = "\
.routine main, tapes=2, alpha=(2, 2)
.section tables
F0: .frame tapes=(1, 0)
    .map 0, wmap=(1=>1)
.section code
.func main
        stp
";
    let mut service = TmaLanguageService::new();
    let diags = service.did_update(URI, with_wmap);
    assert!(
        diags.iter().any(|d| d.message.contains("read-direction")),
        "{diags:?}"
    );

    // The same pair in `rmap` is fine — FULL itself uses the plain form, and
    // a one-way rmap pair must not be flagged.
    let with_rmap = with_wmap.replace("wmap=(1=>1)", "rmap=(1=>1)");
    let diags = service.did_update(URI, &with_rmap);
    assert!(
        diags.iter().all(|d| !d.message.contains("read-direction")),
        "{diags:?}"
    );
}

#[test]
fn a_map_that_unpins_blank_is_an_error() {
    let src = "\
.routine main, tapes=2, alpha=(2, 2)
.section tables
F0: .frame tapes=(1, 0)
    .map 0, rmap=(0->1)
.section code
.func main
        stp
";
    let mut service = TmaLanguageService::new();
    let diags = service.did_update(URI, src);
    assert!(
        diags.iter().any(|d| d.message.contains("unpins blank")),
        "{diags:?}"
    );
}

#[test]
fn a_clean_descriptor_produces_no_descriptor_findings() {
    let mut service = TmaLanguageService::new();
    let diags = service.did_update(URI, FULL);
    assert!(
        diags.iter().all(|d| d.code != Some("bad-frame")),
        "{diags:?}"
    );
}

// ---- completions ----

#[test]
fn the_word_position_offers_every_mnemonic_and_directive() {
    let mut service = opened("");
    let pos = Pos { line: 1, col: 1 };
    let candidates = service.completion(URI, pos);

    let names = labels(&candidates);
    for expected in ["mtc", "djmp", "call.m", "retx", "wrmv", ".frame", ".exits"] {
        assert!(names.contains(&expected.to_string()), "{names:?}");
    }
    assert!(candidates.iter().all(|c| c.kind == CandidateKind::Keyword));
    assert!(candidates.iter().all(|c| c.replace_span == Span {
        start: pos,
        end: pos
    }));
}

#[test]
fn mnemonic_hints_come_from_the_operand_kind_and_flow_alone() {
    let mut service = opened("");
    let candidates = service.completion(URI, Pos { line: 1, col: 1 });
    let detail = |label: &str| {
        candidates
            .iter()
            .find(|c| c.label == label)
            .unwrap_or_else(|| panic!("no `{label}` candidate"))
            .detail
            .clone()
    };
    assert_eq!(detail("mtc"), Some("mtc <table>".to_string()));
    assert_eq!(detail("djmp"), Some("djmp <table>".to_string()));
    assert_eq!(
        detail("call.m"),
        Some("call.m <target>, <frame>".to_string())
    );
    assert_eq!(detail("retx"), Some("retx #<n>".to_string()));
    assert_eq!(detail("mov"), Some("mov [<moves>]".to_string()));
    assert_eq!(
        detail("wrmv"),
        Some("wrmv [<symbols>], [<moves>]".to_string())
    );
    assert_eq!(detail("jmp"), Some("jmp <label>".to_string()));
    assert_eq!(detail("call"), Some("call <function>".to_string()));
    assert_eq!(detail("nop"), None, "a no-operand mnemonic carries no hint");
}

#[test]
fn a_table_operand_offers_the_documents_tables_with_their_kind() {
    let mut service = opened(FULL);
    let candidates = service.completion(URI, MTC_OPERAND);
    assert_eq!(labels(&candidates), vec!["T0", "D0"]);
    assert_eq!(candidates[0].detail, Some(".row".to_string()));
    assert_eq!(candidates[1].detail, Some(".targets".to_string()));
}

#[test]
fn a_framed_calls_first_slot_offers_callables_and_its_second_offers_frames() {
    let mut service = opened(FULL);

    let callees = service.completion(URI, CALLM_TARGET);
    assert_eq!(labels(&callees), vec!["helper", "main"]);

    let frames = service.completion(URI, CALLM_FRAME);
    assert_eq!(labels(&frames), vec!["F0"]);
    assert_eq!(frames[0].detail, Some(".frame, 2 tapes".to_string()));
}

#[test]
fn an_exits_target_offers_the_documents_code_labels() {
    let mut service = opened(FULL);
    let candidates = service.completion(URI, EXITS_ENTRY);
    let names = labels(&candidates);
    for expected in ["hit", "done", "other", "miss"] {
        assert!(names.contains(&expected.to_string()), "{names:?}");
    }
    assert!(candidates.iter().all(|c| c.kind == CandidateKind::Value));
}

#[test]
fn a_dispatch_entry_offers_the_documents_code_labels() {
    let mut service = opened(FULL);
    let candidates = service.completion(URI, TARGETS_ENTRY);
    assert!(labels(&candidates).contains(&"miss".to_string()));
}

#[test]
fn a_branch_operand_offers_only_the_enclosing_functions_labels() {
    let src = "\
.routine main, tapes=2, alpha=(2, 2)
.section code
.func f
A:      nop
        jmp
.func g
B:      nop
        stp
";
    let mut service = opened(src);
    let candidates = service.completion(URI, Pos { line: 5, col: 17 });
    assert_eq!(labels(&candidates), vec!["A"], "never g's own B");
}

#[test]
fn an_immediate_operand_offers_nothing() {
    let mut service = opened(FULL);
    // `retx #1`'s own operand slot.
    assert!(service.completion(URI, Pos { line: 21, col: 18 }).is_empty());
}

#[test]
fn a_row_operand_offers_nothing() {
    // A `.row` carries a symbol vector, not a name.
    let mut service = opened(FULL);
    assert!(service.completion(URI, Pos { line: 4, col: 12 }).is_empty());
}

// ---- semantic tokens ----

#[test]
fn tables_and_frames_are_typed_apart_from_code_labels() {
    let mut service = opened(FULL);
    let tokens = service.semantic_tokens(URI).expect("a known document");
    let at = |span: Span| {
        tokens
            .iter()
            .find(|t| t.span == span)
            .unwrap_or_else(|| panic!("no token at {span:?} in {tokens:?}"))
    };

    // Table and frame labels ride `type` and carry the declaration modifier.
    assert_eq!(at(t0_label()).token_type, TOKEN_TYPE_TYPE);
    assert_eq!(at(t0_label()).modifiers, MODIFIER_DECLARATION);
    assert_eq!(at(f0_label()).token_type, TOKEN_TYPE_TYPE);
    // A code label rides `variable`; a function name rides `function`.
    assert_eq!(at(hit_label()).token_type, TOKEN_TYPE_VARIABLE);
    assert_eq!(at(main_func_name()).token_type, TOKEN_TYPE_FUNCTION);
    // The `mtc T0` reference is typed as a table too, without the modifier.
    let mtc_ref = at(Span::new(13, 17, 13, 19));
    assert_eq!(mtc_ref.token_type, TOKEN_TYPE_TYPE);
    assert_eq!(mtc_ref.modifiers, 0, "a reference, not a declaration");
}

#[test]
fn the_token_legend_matches_the_emitters_own_constants() {
    // Drift guard: the legend's arrays and the index constants are two
    // spellings of one fact.
    let service = TmaLanguageService::new();
    let (types, modifiers) = service.token_legend();
    assert_eq!(types[TOKEN_TYPE_FUNCTION as usize], "function");
    assert_eq!(types[TOKEN_TYPE_VARIABLE as usize], "variable");
    assert_eq!(types[TOKEN_TYPE_TYPE as usize], "type");
    assert_eq!(types[TOKEN_TYPE_NUMBER as usize], "number");
    assert_eq!(
        modifiers[MODIFIER_DECLARATION.trailing_zeros() as usize],
        "declaration"
    );
}

#[test]
fn an_unresolved_reference_emits_no_token_for_its_name() {
    let src = "\
.routine main, tapes=2, alpha=(2, 2)
.section code
.func main
        mtc     NOSUCH
        stp
";
    let mut service = opened(src);
    let tokens = service.semantic_tokens(URI).expect("a known document");
    let name = Span::new(4, 17, 4, 23);
    assert!(tokens.iter().all(|t| t.span != name), "{tokens:?}");
}

#[test]
fn semantic_tokens_survive_a_document_that_fails_to_assemble() {
    let mut service = opened("<not assembly>\n");
    assert!(service.semantic_tokens(URI).is_some());
}

// ---- document symbols ----

#[test]
fn document_symbols_name_the_routines_tables_frames_and_functions() {
    let mut service = opened(FULL);
    let symbols = service.document_symbols(URI).expect("a known document");
    let names: Vec<&str> = symbols.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(
        names,
        vec!["main", "helper", "T0", "D0", "F0", "main", "helper"],
        "the two signatures, the tables and the frame, then the function bodies"
    );

    // A function carries its own code labels as children.
    let main_body = symbols
        .iter()
        .rfind(|s| s.name == "main")
        .expect("the .func main node");
    let children: Vec<&str> = main_body.children.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(children, vec!["hit", "done", "other", "miss"]);
}

#[test]
fn document_symbols_survive_a_document_that_fails_to_assemble() {
    let mut service = opened(".func main\n        bogus\n");
    let symbols = service.document_symbols(URI).expect("CST-tier symbols");
    assert_eq!(symbols.len(), 1);
    assert_eq!(symbols[0].name, "main");
}

// ---- line recovery over `.rept`, the divergence from the `.pma` service ----

#[test]
fn rept_bodies_are_flattened_onto_their_own_source_lines() {
    // The `.pma` service recovers lines by zipping items against non-blank
    // lines, one per line. A `.rept` block breaks that invariant — it is ONE
    // item spanning many lines with its body nested inside — so the `.tma`
    // service walks the tree instead. A cursor on a body line must classify
    // against that body line, not against the block header.
    let src = "\
.routine main, tapes=2, alpha=(2, 2)
.section code
.func main
L0:     nop
.rept v, 0, 1
        jmp     L0
.endr
        stp
";
    let mut service = opened(src);
    let target = service
        .definition(URI, Pos { line: 6, col: 18 })
        .expect("a body-line reference resolves");
    assert_eq!(target.span, Span::new(4, 1, 4, 3));
}

#[test]
fn a_comment_after_a_rept_block_does_not_displace_later_items() {
    // Comments carry no line of their own and are seated by a cursor walk;
    // the `.endr` line shapes no item, so it must be skipped explicitly or
    // every item after the block shifts by one line.
    let src = "\
.routine main, tapes=2, alpha=(2, 2)
.section code
.func main
.rept v, 0, 1
        nop
.endr
; a trailing note
L9:     stp
";
    let mut service = opened(src);
    let symbols = service.document_symbols(URI).expect("a known document");
    let main = symbols
        .iter()
        .rfind(|s| s.name == "main")
        .expect("the .func main node");
    let children: Vec<&str> = main.children.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(children, vec!["L9"]);

    // And the reference on line 8 still classifies on line 8.
    let candidates = service.completion(URI, Pos { line: 8, col: 9 });
    assert!(
        candidates.iter().any(|c| c.label == "stp"),
        "the word position on the real line 8: {candidates:?}"
    );
}

// ---- robustness: a panic on a request path kills the user's editor ----

#[test]
fn every_request_survives_every_truncation_of_a_real_document() {
    // Every prefix of a real document is what a file being typed looks like.
    // The assertion is simply that the service answers at all — the position
    // walks are full of index arithmetic over an item stream that may end
    // anywhere.
    let mut service = TmaLanguageService::new();
    for cut in 0..=FULL.len() {
        if !FULL.is_char_boundary(cut) {
            continue;
        }
        let src = &FULL[..cut];
        service.did_update(URI, src);
        let line = src.matches('\n').count() as u32 + 1;
        let col = src.rsplit('\n').next().map_or(0, |l| l.chars().count()) as u32 + 1;
        let pos = Pos { line, col };
        service.completion(URI, pos);
        service.definition(URI, pos);
        service.hover(URI, pos);
        service.code_actions(
            URI,
            Span {
                start: pos,
                end: pos,
            },
        );
        service.document_symbols(URI);
        service.semantic_tokens(URI);
        service.format(URI);
    }
}

/// Documents that break the assumptions each feature module makes:
/// unterminated vectors and groups, directives with missing or junk operands,
/// descriptor continuations with no header, a `.rept` with no `.endr`, a sigil
/// with no name, non-ASCII where a name is expected, and an inverted range.
const MALFORMED: &[&str] = &[
    "",
    "\n\n\n",
    "   \n\t\n",
    ".func",
    ".func \n",
    ".routine",
    ".routine main, tapes=, alpha=(",
    ".routine main, tapes=99999999999, alpha=(2)",
    ".section",
    ".section nosuchsection\n",
    ".row [",
    ".row [1, ",
    ".targets",
    ".targets ,,,",
    ".target",
    ".frame",
    ".frame tapes=(",
    ".frame tapes=()",
    "F0: .frame tapes=(1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16,17)",
    "F0: .frame tapes=(99999)",
    ".map",
    ".map 0, rmap=(",
    ".map 0, rmap=(0->1)",
    ".map 0, wmap=(1=>2)",
    ".map 99999999, rmap=(1->1)",
    ".exits",
    ".exits ,",
    ".rept",
    ".rept v, 0, 1\n        nop\n",
    ".rept v, 1, 0\n.endr\n",
    ".rept v, 0, 1\n.rept w, 0, 1\n.endr\n.endr\n",
    "        mtc\n",
    "        mtc     \n",
    "        djmp    {\n",
    "        call.m\n",
    "        call.m  ,\n",
    "        call.m  a, b, c\n",
    "        retx\n",
    "        retx    #\n",
    "        trap    #999\n",
    "        wr      [\n",
    "        wrmv    [1], \n",
    "        jmp     @\n",
    "@@@\n",
    "; just a comment\n",
    ";\n;\n;\n",
    "\u{1F600} \u{4F60}\u{597D}\n",
    ".func \u{4F60}\u{597D}\n        stp\n",
    "L1: L2: L3:\n",
    ".section tables\n.map 0\n.exits x\n.map 0\n",
    ".section tables\nF0: .frame tapes=(1)\n; note\n    .map 0\n",
];

#[test]
fn every_request_survives_malformed_documents_at_every_position() {
    let mut service = TmaLanguageService::new();
    for (i, src) in MALFORMED.iter().enumerate() {
        let uri = format!("untitled:malformed-{i}");
        service.did_update(&uri, src);
        // One line past the end, and column 0 — what a client that forgot to
        // convert from 0-based positions sends.
        let line_count = src.matches('\n').count() as u32 + 2;
        for line in 1..=line_count {
            for col in 0..=48u32 {
                let pos = Pos { line, col };
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
            }
        }
        service.document_symbols(&uri);
        service.semantic_tokens(&uri);
        service.format(&uri);
        service.did_close(&uri);
    }
}

#[test]
fn every_request_survives_every_truncation_of_every_malformed_document() {
    // Truncation and malformation compose: a half-typed broken line is the
    // common case, not the exotic one.
    let mut service = TmaLanguageService::new();
    for src in MALFORMED {
        for cut in 0..=src.len() {
            if !src.is_char_boundary(cut) {
                continue;
            }
            let prefix = &src[..cut];
            service.did_update(URI, prefix);
            let line = prefix.matches('\n').count() as u32 + 1;
            let col = prefix.rsplit('\n').next().map_or(0, |l| l.chars().count()) as u32 + 1;
            let pos = Pos { line, col };
            service.completion(URI, pos);
            service.definition(URI, pos);
            service.code_actions(
                URI,
                Span {
                    start: pos,
                    end: pos,
                },
            );
            service.document_symbols(URI);
            service.semantic_tokens(URI);
            service.format(URI);
        }
    }
}

#[test]
fn requests_against_an_unknown_document_are_all_empty_never_a_panic() {
    let mut service = TmaLanguageService::new();
    let pos = Pos { line: 1, col: 1 };
    assert!(service.completion("untitled:never-opened", pos).is_empty());
    assert_eq!(service.definition("untitled:never-opened", pos), None);
    assert_eq!(service.hover("untitled:never-opened", pos), None);
    assert!(
        service
            .code_actions(
                "untitled:never-opened",
                Span {
                    start: pos,
                    end: pos
                }
            )
            .is_empty()
    );
    assert_eq!(service.document_symbols("untitled:never-opened"), None);
    assert_eq!(service.semantic_tokens("untitled:never-opened"), None);
    assert_eq!(service.format("untitled:never-opened"), None);
    // Closing a document that was never opened is a no-op, not a panic.
    service.did_close("untitled:never-opened");
}
