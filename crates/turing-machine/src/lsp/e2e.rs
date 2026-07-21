//! End-to-end scripted sessions: the two REAL services driven through core's
//! blocking server loop over in-memory pipes, exactly as `tmt lsp` drives them
//! over stdio. The CLI subcommand differs only in which reader and writer it
//! hands to `mtc_core::lsp::server::run`, and in the service slice it builds —
//! which these tests build identically, `.tmc` first, so what they prove is the
//! wiring the shipped binary runs rather than the framework exercised against
//! fakes.
//!
//! What routing has to get right, and what these sessions pin:
//!
//! - A document goes to the service whose `extensions()` (or `language_id()`)
//!   matches it, and the binding is made once at didOpen and remembered. Two
//!   documents of different languages therefore coexist in one session, each
//!   answered by its own backend.
//! - A document matching NO service falls back to service 0 — `.tmc` here,
//!   because it is listed first — so an unknown extension still lands somewhere
//!   instead of going unbound and silently answering nothing.
//! - `initialize` merges both services' capabilities: the semantic-token legend
//!   is the concatenation of the two legends, and the framework remaps each
//!   service's own indices into that merged space.

use serde_json::{Value, json};

use mtc_core::lsp::transport::{read_message, write_message};

use super::{TmaLanguageService, TmcLanguageService};

/// Frames `client_messages` into an in-memory buffer, drives the real server
/// loop against BOTH real services in the order `cli/lsp.rs` builds them, and
/// decodes every framed response back to JSON.
fn run_session(client_messages: &[Value]) -> (Vec<Value>, i32) {
    let mut input = Vec::new();
    for msg in client_messages {
        write_message(&mut input, &msg.to_string()).expect("write into a Vec cannot fail");
    }

    let mut tmc = TmcLanguageService::new();
    let mut tma = TmaLanguageService::new();
    let mut output = Vec::new();
    let mut reader = &input[..];
    let mut services: [&mut dyn mtc_core::lsp::LanguageService; 2] = [&mut tmc, &mut tma];
    let exit_code = mtc_core::lsp::server::run(
        &mut reader,
        &mut output,
        &mut services,
        mtc_core::lsp::server::ServerIdentity {
            name: "tmt lsp",
            version: "0.0.0-test",
        },
    );

    let mut out_reader = &output[..];
    let mut outputs = Vec::new();
    while let Some(payload) = read_message(&mut out_reader).expect("recorded output frames") {
        outputs.push(serde_json::from_str(&payload).expect("recorded output is valid json"));
    }
    (outputs, exit_code)
}

fn init(id: i64) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "method": "initialize", "params": {}})
}

fn open(uri: &str, language_id: &str, text: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "method": "textDocument/didOpen",
        "params": {"textDocument": {
            "uri": uri, "languageId": language_id, "version": 1, "text": text,
        }},
    })
}

fn request(id: i64, method: &str, params: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params})
}

/// A `.tmc` program whose only finding is `leftover-debugger` — a rule with no
/// `.tma` counterpart at all, so its presence alone proves the `.tmc` backend
/// answered.
const TMC_DEBUGGER: &str = "\
alphabet bits { '_', '1' }
machine {
  tape t: bits;
  entry state s { [*] -> debugger move [>] stop; }
}
";

/// A `.tma` program whose only finding is `unreachable-code` — dead code after
/// `stp`. Deliberately NOT in the canonical grid, so the formatting assertion
/// below is a real transformation rather than an identity.
const TMA_DEAD_CODE: &str = "\
.routine main, tapes=2, alpha=(2, 2)
.section code
.func main
  stp
   nop
";

/// A `.tma` program exercising the TM-specific navigation: `mtc T0` names a
/// match table declared in the tables section.
const TMA_TABLE: &str = "\
.routine main, tapes=2, alpha=(2, 2)
.section tables
T0: .row [1, 1]
    .row [*, *]
.section code
.func main
        rd
        mtc     T0
        stp
";

#[test]
fn one_session_routes_a_tmc_and_a_tma_document_to_their_own_services() {
    let tmc_uri = "file:///e2e.tmc";
    let tma_uri = "file:///e2e.tma";

    let (outputs, _) = run_session(&[
        init(1),
        open(tmc_uri, "tmc", TMC_DEBUGGER),
        open(tma_uri, "tma", TMA_DEAD_CODE),
    ]);
    assert_eq!(outputs.len(), 3, "{outputs:?}");

    // initialize: the merged legend is `.tmc`'s six token types followed by
    // `.tma`'s four, concatenated without dedup — proof both services
    // registered, `.tmc` first, exactly as `cli/lsp.rs` builds the slice.
    let legend: Vec<&str> =
        outputs[0]["result"]["capabilities"]["semanticTokensProvider"]["legend"]["tokenTypes"]
            .as_array()
            .expect("a token-type legend")
            .iter()
            .filter_map(|v| v.as_str())
            .collect();
    assert_eq!(
        legend,
        vec![
            "namespace",
            "type",
            "function",
            "variable",
            "string",
            "number", // .tmc
            "function",
            "variable",
            "type",
            "number", // .tma
        ],
        "{legend:?}"
    );

    // The trigger characters merge as a union across both services.
    let triggers: Vec<&str> =
        outputs[0]["result"]["capabilities"]["completionProvider"]["triggerCharacters"]
            .as_array()
            .expect("trigger characters")
            .iter()
            .filter_map(|v| v.as_str())
            .collect();
    for expected in [":", "[", ",", "=", ">", "@", "."] {
        assert!(triggers.contains(&expected), "{triggers:?}");
    }

    // The `.tmc` document went to the `.tmc` service: `leftover-debugger` has
    // no `.tma` counterpart.
    assert_eq!(outputs[1]["params"]["uri"], json!(tmc_uri));
    let tmc_diags = outputs[1]["params"]["diagnostics"]
        .as_array()
        .expect("diagnostics");
    assert_eq!(tmc_diags.len(), 1, "{tmc_diags:?}");
    assert_eq!(tmc_diags[0]["code"], json!("leftover-debugger"));
    assert_eq!(tmc_diags[0]["source"], json!("tmt lint"));

    // The `.tma` document, opened in the SAME session, went to the `.tma`
    // service and is unaffected by sharing the server.
    assert_eq!(outputs[2]["params"]["uri"], json!(tma_uri));
    let tma_diags = outputs[2]["params"]["diagnostics"]
        .as_array()
        .expect("diagnostics");
    assert_eq!(tma_diags.len(), 1, "{tma_diags:?}");
    assert_eq!(tma_diags[0]["code"], json!("unreachable-code"));
    assert_eq!(tma_diags[0]["source"], json!("tmt lint"));
}

#[test]
fn each_document_is_answered_by_its_own_backend_for_every_request_kind() {
    let tmc_uri = "file:///req.tmc";
    let tma_uri = "file:///req.tma";

    let (outputs, _) = run_session(&[
        init(1),
        open(tmc_uri, "tmc", TMC_DEBUGGER),
        open(tma_uri, "tma", TMA_TABLE),
        // Completion on the `.tma` document, in `mtc`'s operand slot: the
        // table candidate is a `.tma`-only concept.
        request(
            2,
            "textDocument/completion",
            json!({
                "textDocument": {"uri": tma_uri},
                "position": {"line": 7, "character": 16},
            }),
        ),
        // Go-to-definition on the same operand: the TM-specific table-label
        // navigation, reached over the wire.
        request(
            3,
            "textDocument/definition",
            json!({
                "textDocument": {"uri": tma_uri},
                "position": {"line": 7, "character": 17},
            }),
        ),
        // Formatting the `.tma` document: the canonical grid under the TM
        // dialect's own capabilities.
        request(
            4,
            "textDocument/formatting",
            json!({"textDocument": {"uri": tma_uri}}),
        ),
        // Hover on the `.tmc` document: `.tma` has none, so a non-null answer
        // here can only have come from the `.tmc` backend.
        request(
            5,
            "textDocument/hover",
            json!({
                "textDocument": {"uri": tmc_uri},
                "position": {"line": 0, "character": 10},
            }),
        ),
    ]);
    assert_eq!(outputs.len(), 7, "{outputs:?}");

    // Completion: the table candidate, with the `.tma` service's own operand
    // hint as its detail.
    let items = outputs[3]["result"].as_array().expect("completion items");
    let t0 = items
        .iter()
        .find(|c| c["label"] == json!("T0"))
        .unwrap_or_else(|| panic!("no `T0` candidate: {items:?}"));
    assert_eq!(t0["detail"], json!(".row"));

    // Definition: the label on the directive that opens the table, line 3
    // (0-based 2) at columns 0..2.
    let target = &outputs[4]["result"];
    assert_eq!(target["uri"], json!(tma_uri));
    assert_eq!(
        target["range"],
        json!({
            "start": {"line": 2, "character": 0},
            "end": {"line": 2, "character": 2},
        }),
        "{target:?}"
    );

    // Formatting: byte-identical to what the grid printer itself produces
    // under `tm1_syntax()`'s capabilities — not a re-implementation.
    let edits: Vec<mtc_core::lsp::types::TextEdit> =
        serde_json::from_value(outputs[5]["result"].clone()).expect("text edits");
    assert_eq!(edits.len(), 1, "{edits:?}");
    let canonical =
        mtc_core::asm::format_asm_with(TMA_TABLE, crate::asm::tm1_syntax().caps).expect("formats");
    assert_eq!(edits[0].new_text, canonical);

    // Hover on the `.tmc` alphabet name: only the `.tmc` service answers
    // hover at all, so a non-null result proves the routing.
    assert_eq!(outputs[6]["result"]["contents"]["kind"], json!("plaintext"));
    assert!(
        outputs[6]["result"]["contents"]["value"]
            .as_str()
            .is_some_and(|v| v.contains('1')),
        "the alphabet's own symbols: {:?}",
        outputs[6]["result"]
    );
}

#[test]
fn a_document_matching_no_service_falls_back_to_the_first_one() {
    // `.tmc` is listed first in the slice `cli/lsp.rs` builds, so it is the
    // fallback an unmatched document binds to. A didOpen whose languageId and
    // URI extension both miss every service still lands somewhere instead of
    // going unbound and silently answering nothing.
    let odd_uri = "file:///mystery.unknown";
    let (outputs, _) = run_session(&[init(1), open(odd_uri, "plaintext", TMC_DEBUGGER)]);

    assert_eq!(outputs.len(), 2, "{outputs:?}");
    assert_eq!(outputs[1]["params"]["uri"], json!(odd_uri));
    let diags = outputs[1]["params"]["diagnostics"]
        .as_array()
        .expect("diagnostics");
    assert_eq!(diags.len(), 1, "{diags:?}");
    assert_eq!(
        diags[0]["code"],
        json!("leftover-debugger"),
        "the `.tmc` service, as service 0, took it"
    );
}

#[test]
fn a_tma_document_binds_by_uri_extension_even_when_the_language_id_is_wrong() {
    // Editors do not always send a languageId the server knows. The extension
    // is the second signal, and it has to be enough on its own.
    let tma_uri = "file:///byext.tma";
    let (outputs, _) = run_session(&[init(1), open(tma_uri, "plaintext", TMA_DEAD_CODE)]);

    assert_eq!(outputs.len(), 2, "{outputs:?}");
    let diags = outputs[1]["params"]["diagnostics"]
        .as_array()
        .expect("diagnostics");
    assert_eq!(diags.len(), 1, "{diags:?}");
    assert_eq!(
        diags[0]["code"],
        json!("unreachable-code"),
        "the `.tma` service took it on the extension alone"
    );
}

#[test]
fn a_shutdown_exit_pair_closes_the_session_cleanly() {
    let (_, exit_code) = run_session(&[
        init(1),
        request(2, "shutdown", json!(null)),
        json!({"jsonrpc": "2.0", "method": "exit"}),
    ]);
    assert_eq!(exit_code, 0, "exit after shutdown is a clean close");
}

#[test]
fn a_malformed_document_on_either_service_does_not_take_the_session_down() {
    // The robustness bar at the session level: junk in either language must
    // publish diagnostics and leave the server answering, not kill the
    // process the user's editor is talking to.
    let tmc_uri = "file:///junk.tmc";
    let tma_uri = "file:///junk.tma";
    let (outputs, exit_code) = run_session(&[
        init(1),
        open(tmc_uri, "tmc", "machine { tape"),
        open(tma_uri, "tma", ".frame tapes=(\n.map 9\n@@@\n"),
        request(
            2,
            "textDocument/completion",
            json!({
                "textDocument": {"uri": tma_uri},
                "position": {"line": 99, "character": 99},
            }),
        ),
        request(
            3,
            "textDocument/definition",
            json!({
                "textDocument": {"uri": tmc_uri},
                "position": {"line": 99, "character": 99},
            }),
        ),
        request(4, "shutdown", json!(null)),
        json!({"jsonrpc": "2.0", "method": "exit"}),
    ]);

    // Both opens published, both requests answered, and the session still
    // shut down cleanly.
    assert_eq!(exit_code, 0);
    assert!(outputs.len() >= 5, "{outputs:?}");
    let answered: Vec<i64> = outputs
        .iter()
        .filter_map(|o| o.get("id").and_then(|id| id.as_i64()))
        .collect();
    assert!(answered.contains(&2), "{answered:?}");
    assert!(answered.contains(&3), "{answered:?}");
    assert!(answered.contains(&4), "{answered:?}");
}
