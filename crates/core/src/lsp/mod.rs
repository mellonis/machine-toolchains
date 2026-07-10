//! Language-agnostic LSP server framework (LSP 3.17 subset): framing,
//! JSON-RPC, protocol structs, position mapping, document store, and the
//! blocking server loop behind the `LanguageService` seam. Carries zero
//! architecture or language knowledge by contract — exercised against a
//! crate-private fake service (docs/lsp.md; docs/cli.md (thin-renderer rule)).

pub mod docstore;
pub mod jsonrpc;
pub mod position;
pub mod server;
pub mod transport;
pub mod types;

use crate::diagnostics::{Edit, Pos, Span};

/// Presentation severity of a [`ServiceDiagnostic`]; the toolchain's own
/// `diagnostics::Diagnostic` carries no severity (that's a service/
/// presentation concern, not a compiler one).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceSeverity {
    Error,
    Warning,
}

/// A diagnostic as the service speaks it: toolchain span + presentation
/// (severity/source/code are presentation, chosen by the service —
/// core::diagnostics::Diagnostic has neither).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceDiagnostic {
    pub span: Span,
    pub severity: ServiceSeverity,
    pub source: &'static str,
    pub code: Option<&'static str>,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CandidateKind {
    Function,
    Module,
    Keyword,
    Value,
}

/// One completion candidate; inserts via textEdit over the exact token
/// prefix (`replace_span`) so replacement never depends on client-side
/// word heuristics. A zero-width span at the cursor means plain insert.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub label: String,
    pub kind: CandidateKind,
    pub replace_span: Span,
    pub insert_text: String,
}

/// Definition target; `uri` may name a document other than the requester
/// (e.g. a materialized library file on disk).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DefTarget {
    pub uri: String,
    pub span: Span,
}

/// A quickfix: edits apply to the requesting document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Action {
    pub title: String,
    pub preferred: bool,
    pub edits: Vec<Edit>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SymbolNodeKind {
    Namespace,
    Function,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SymbolNode {
    pub name: String,
    pub kind: SymbolNodeKind,
    pub span: Span,           // full extent
    pub selection_span: Span, // the declaration name
    pub children: Vec<SymbolNode>,
}

/// One absolute semantic token. `span` MUST be single-line (contract;
/// the packer debug_asserts it). `token_type` indexes the legend's types;
/// `modifiers` is a bitset over the legend's modifiers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SemToken {
    pub span: Span,
    pub token_type: u32,
    pub modifiers: u32,
}

/// The seam a language plugs into the framework through (docs/lsp.md).
/// Core's server loop (a later task) drives every LSP-visible behavior
/// exclusively through this trait — it carries no `.pmc`/architecture
/// knowledge itself.
pub trait LanguageService {
    fn language_id(&self) -> &'static str;
    fn trigger_characters(&self) -> &[char];
    /// (token types, token modifiers) — the legend advertised in capabilities.
    fn token_legend(&self) -> (&'static [&'static str], &'static [&'static str]);
    /// Client file-watch globs (e.g. a project config file). May be empty.
    fn watched_globs(&self) -> &'static [&'static str];
    /// Called on didOpen and didChange (framework owns the text); the return
    /// is published as the document's complete diagnostic set. Also re-run by
    /// the framework for every open document after a config or watched-file
    /// change.
    fn did_update(&mut self, uri: &str, text: &str) -> Vec<ServiceDiagnostic>;
    fn did_close(&mut self, uri: &str);
    /// Opaque settings JSON: initializationOptions at startup and
    /// workspace/didChangeConfiguration payloads live.
    fn did_change_config(&mut self, settings: serde_json::Value);
    fn completion(&mut self, uri: &str, pos: Pos) -> Vec<Candidate>;
    fn definition(&mut self, uri: &str, pos: Pos) -> Option<DefTarget>;
    fn code_actions(&mut self, uri: &str, span: Span) -> Vec<Action>;
    fn document_symbols(&mut self, uri: &str) -> Option<Vec<SymbolNode>>;
    fn semantic_tokens(&mut self, uri: &str) -> Option<Vec<SemToken>>;
    /// Full replacement text, or None (degraded / nothing to format against).
    fn format(&mut self, uri: &str) -> Option<String>;
}

/// A deterministic toy [`LanguageService`] fixture, exercised by this
/// crate's own tests and by the server-loop tests in later tasks
/// (docs/lsp.md). Speaks a made-up "fake" language only — no `.pmc` or
/// architecture knowledge leaks in here.
#[cfg(test)]
pub(crate) mod fake {
    use super::{
        Action, Candidate, CandidateKind, DefTarget, LanguageService, ServiceDiagnostic,
        ServiceSeverity, SymbolNode, SymbolNodeKind,
    };
    use crate::diagnostics::{Edit, Pos, Span};
    use crate::lsp::SemToken;
    use std::collections::HashMap;

    /// The `function` token type's legend index.
    const TOKEN_TYPE_FUNCTION: u32 = 0;
    /// The `declaration` token modifier's bit.
    const MODIFIER_DECLARATION: u32 = 1 << 0;

    pub(crate) struct FakeService {
        texts: HashMap<String, String>,
        config_revision: u32,
    }

    impl FakeService {
        pub(crate) fn new() -> Self {
            FakeService {
                texts: HashMap::new(),
                config_revision: 0,
            }
        }
    }

    /// Every non-overlapping, char-precise occurrence of `needle` in
    /// `text`, one line at a time (`needle` itself is assumed
    /// single-line — true for every needle this fixture searches for).
    fn find_occurrences(text: &str, needle: &str) -> Vec<Span> {
        let needle: Vec<char> = needle.chars().collect();
        let mut spans = Vec::new();

        for (line_ix, line) in text.split('\n').enumerate() {
            let line_no = (line_ix + 1) as u32;
            let chars: Vec<char> = line.chars().collect();
            let mut i = 0;
            while i + needle.len() <= chars.len() {
                if chars[i..i + needle.len()] == needle[..] {
                    let start_col = (i + 1) as u32;
                    let end_col = start_col + needle.len() as u32;
                    spans.push(Span::new(line_no, start_col, line_no, end_col));
                    i += needle.len();
                } else {
                    i += 1;
                }
            }
        }

        spans
    }

    /// Half-open span overlap: `a.start < b.end && b.start < a.end`.
    fn spans_overlap(a: Span, b: Span) -> bool {
        a.start < b.end && b.start < a.end
    }

    impl LanguageService for FakeService {
        fn language_id(&self) -> &'static str {
            "fake"
        }

        fn trigger_characters(&self) -> &[char] {
            &['.']
        }

        fn token_legend(&self) -> (&'static [&'static str], &'static [&'static str]) {
            (&["function"], &["declaration"])
        }

        fn watched_globs(&self) -> &'static [&'static str] {
            &["**/fake.json"]
        }

        fn did_update(&mut self, uri: &str, text: &str) -> Vec<ServiceDiagnostic> {
            self.texts.insert(uri.to_string(), text.to_string());

            // The containment probe: lets Task 7+ tests exercise the
            // framework's fault isolation around a panicking service.
            if text.contains("panic-now") {
                panic!("fake service panic");
            }

            find_occurrences(text, "bad")
                .into_iter()
                .map(|span| ServiceDiagnostic {
                    span,
                    severity: ServiceSeverity::Error,
                    source: "fake",
                    code: Some("bad-word"),
                    message: format!("bad word (config rev {})", self.config_revision),
                })
                .collect()
        }

        fn did_close(&mut self, uri: &str) {
            self.texts.remove(uri);
        }

        fn did_change_config(&mut self, _settings: serde_json::Value) {
            self.config_revision += 1;
        }

        fn completion(&mut self, _uri: &str, pos: Pos) -> Vec<Candidate> {
            vec![Candidate {
                label: "alpha".to_string(),
                kind: CandidateKind::Function,
                replace_span: Span {
                    start: pos,
                    end: pos,
                },
                insert_text: "alpha".to_string(),
            }]
        }

        fn definition(&mut self, uri: &str, _pos: Pos) -> Option<DefTarget> {
            let text = self.texts.get(uri)?;
            find_occurrences(text, "def")
                .into_iter()
                .next()
                .map(|span| DefTarget {
                    uri: uri.to_string(),
                    span,
                })
        }

        fn code_actions(&mut self, uri: &str, span: Span) -> Vec<Action> {
            let Some(text) = self.texts.get(uri) else {
                return Vec::new();
            };

            find_occurrences(text, "bad")
                .into_iter()
                .filter(|bad_span| spans_overlap(*bad_span, span))
                .map(|bad_span| Action {
                    title: "remove bad".to_string(),
                    preferred: true,
                    edits: vec![Edit {
                        span: bad_span,
                        replacement: String::new(),
                    }],
                })
                .collect()
        }

        fn document_symbols(&mut self, uri: &str) -> Option<Vec<SymbolNode>> {
            let text = self.texts.get(uri).map(String::as_str).unwrap_or("");
            if text.is_empty() {
                return Some(Vec::new());
            }

            let first_line = text.split('\n').next().unwrap_or("");
            let end_col = first_line.chars().count() as u32 + 1;
            let span = Span::new(1, 1, 1, end_col);

            Some(vec![SymbolNode {
                name: "root".to_string(),
                kind: SymbolNodeKind::Function,
                span,
                selection_span: span,
                children: Vec::new(),
            }])
        }

        fn semantic_tokens(&mut self, uri: &str) -> Option<Vec<SemToken>> {
            let text = self.texts.get(uri).map(String::as_str).unwrap_or("");
            Some(
                find_occurrences(text, "fn")
                    .into_iter()
                    .map(|span| SemToken {
                        span,
                        token_type: TOKEN_TYPE_FUNCTION,
                        modifiers: MODIFIER_DECLARATION,
                    })
                    .collect(),
            )
        }

        fn format(&mut self, uri: &str) -> Option<String> {
            let text = self.texts.get(uri)?;
            Some(text.replace('\t', "    "))
        }
    }
}

#[cfg(test)]
mod tests {
    use self::fake::FakeService;
    use super::*;

    #[test]
    fn advertises_the_fake_language_surface() {
        let service = FakeService::new();
        assert_eq!(service.language_id(), "fake");
        assert_eq!(service.trigger_characters(), &['.']);
        assert_eq!(
            service.token_legend(),
            (&["function"][..], &["declaration"][..])
        );
        assert_eq!(service.watched_globs(), &["**/fake.json"]);
    }

    #[test]
    fn did_update_reports_a_char_precise_diagnostic_per_bad_occurrence() {
        let mut service = FakeService::new();
        let diagnostics = service.did_update("file:///a.fake", "bad bad");

        assert_eq!(
            diagnostics,
            vec![
                ServiceDiagnostic {
                    span: Span::new(1, 1, 1, 4),
                    severity: ServiceSeverity::Error,
                    source: "fake",
                    code: Some("bad-word"),
                    message: "bad word (config rev 0)".to_string(),
                },
                ServiceDiagnostic {
                    span: Span::new(1, 5, 1, 8),
                    severity: ServiceSeverity::Error,
                    source: "fake",
                    code: Some("bad-word"),
                    message: "bad word (config rev 0)".to_string(),
                },
            ]
        );
    }

    #[test]
    fn did_change_config_bumps_the_revision_embedded_in_diagnostics() {
        let mut service = FakeService::new();
        service.did_change_config(serde_json::json!({}));
        service.did_change_config(serde_json::json!({}));

        let diagnostics = service.did_update("file:///a.fake", "bad");
        assert_eq!(diagnostics[0].message, "bad word (config rev 2)");
    }

    #[test]
    fn did_update_panics_on_the_containment_probe() {
        let mut service = FakeService::new();
        let prev_hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {})); // silence the panic backtrace

        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            service.did_update("file:///a.fake", "panic-now")
        }));

        std::panic::set_hook(prev_hook);

        let err = result.expect_err("did_update must panic on panic-now");
        let message = err
            .downcast_ref::<&str>()
            .copied()
            .or_else(|| err.downcast_ref::<String>().map(String::as_str))
            .expect("panic payload should be a string");
        assert_eq!(message, "fake service panic");
    }

    #[test]
    fn did_close_removes_the_stored_text() {
        let mut service = FakeService::new();
        service.did_update("file:///a.fake", "bad");
        service.did_close("file:///a.fake");

        // With the text gone, format has nothing to format against.
        assert_eq!(service.format("file:///a.fake"), None);
    }

    #[test]
    fn completion_offers_the_single_alpha_candidate_at_the_cursor() {
        let mut service = FakeService::new();
        let pos = Pos { line: 3, col: 5 };
        let candidates = service.completion("file:///a.fake", pos);

        assert_eq!(
            candidates,
            vec![Candidate {
                label: "alpha".to_string(),
                kind: CandidateKind::Function,
                replace_span: Span {
                    start: pos,
                    end: pos
                },
                insert_text: "alpha".to_string(),
            }]
        );
    }

    #[test]
    fn definition_targets_the_first_def_occurrence_when_present() {
        let mut service = FakeService::new();
        service.did_update("file:///a.fake", "x def def");

        let target = service.definition("file:///a.fake", Pos { line: 1, col: 1 });
        assert_eq!(
            target,
            Some(DefTarget {
                uri: "file:///a.fake".to_string(),
                span: Span::new(1, 3, 1, 6),
            })
        );
    }

    #[test]
    fn definition_is_none_without_a_def_occurrence() {
        let mut service = FakeService::new();
        service.did_update("file:///a.fake", "no such word");

        assert_eq!(
            service.definition("file:///a.fake", Pos { line: 1, col: 1 }),
            None
        );
    }

    #[test]
    fn code_actions_cover_every_overlapping_bad_span() {
        let mut service = FakeService::new();
        service.did_update("file:///a.fake", "bad bad");

        // A request span covering only the first "bad" (cols 1..4).
        let actions = service.code_actions("file:///a.fake", Span::new(1, 1, 1, 4));
        assert_eq!(
            actions,
            vec![Action {
                title: "remove bad".to_string(),
                preferred: true,
                edits: vec![Edit {
                    span: Span::new(1, 1, 1, 4),
                    replacement: String::new(),
                }],
            }]
        );

        // A request span covering the whole line hits both.
        let actions = service.code_actions("file:///a.fake", Span::new(1, 1, 1, 8));
        assert_eq!(actions.len(), 2);
    }

    #[test]
    fn document_symbols_is_empty_for_empty_text_and_root_otherwise() {
        let mut service = FakeService::new();
        service.did_update("file:///a.fake", "");
        assert_eq!(service.document_symbols("file:///a.fake"), Some(vec![]));

        service.did_update("file:///a.fake", "fn one\nfn two");
        let span = Span::new(1, 1, 1, 7);
        assert_eq!(
            service.document_symbols("file:///a.fake"),
            Some(vec![SymbolNode {
                name: "root".to_string(),
                kind: SymbolNodeKind::Function,
                span,
                selection_span: span,
                children: Vec::new(),
            }])
        );
    }

    #[test]
    fn semantic_tokens_marks_every_fn_occurrence() {
        let mut service = FakeService::new();
        service.did_update("file:///a.fake", "fn one\nfn two");

        assert_eq!(
            service.semantic_tokens("file:///a.fake"),
            Some(vec![
                SemToken {
                    span: Span::new(1, 1, 1, 3),
                    token_type: 0,
                    modifiers: 1,
                },
                SemToken {
                    span: Span::new(2, 1, 2, 3),
                    token_type: 0,
                    modifiers: 1,
                },
            ])
        );
    }

    #[test]
    fn format_replaces_tabs_and_is_unchanged_without_them() {
        let mut service = FakeService::new();

        service.did_update("file:///a.fake", "a\tb");
        assert_eq!(service.format("file:///a.fake"), Some("a    b".to_string()));

        service.did_update("file:///b.fake", "a b");
        assert_eq!(service.format("file:///b.fake"), Some("a b".to_string()));
    }
}
