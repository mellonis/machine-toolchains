//! `.tmc` lint layer: hygiene findings over the compiler's analysis, the
//! front-end mirror of the `.pmc` lint layer in the sibling PM-1 crate.
//! Library-only — the CLI renders (docs/core.md (thin-renderer rule)). Strict
//! channel split: lint reports lint findings ONLY; the compile warnings stay on
//! the compile channel (`tmt compile`) and are never re-reported here — with
//! one deliberate exception, the `unused-import`/`unused-routine` re-exposure
//! (below), which surfaces existing hygiene warnings under allow control so a
//! `tmt lint` run and its allow-list cover them too.
//!
//! # What the rules see
//!
//! Rules read one [`crate::compiler::Analysis`] — tokens, the flat program, the
//! resolved module (worlds / alphabets / docs), and analyze's own non-fatal
//! diagnostics. The lint runs only over the resolution stage's output, never
//! `expand`/`lower`: the two later-stage hygiene warnings this layer also
//! carries (`unused-routine` and `binding-product-threshold`, which the
//! compiler raises during IR lowering and expansion respectively) are
//! re-detected here at source level over `Resolved`, not harvested by running
//! those stages (which could fatal on input the resolve stage accepted).
//!
//! # Staged-seam limitation
//!
//! Lint runs on successfully-*analyzed* input: `lint()` bails with a fatal if
//! resolution does not complete. A source that fatals partway through
//! resolution therefore yields no lint findings at all, even for the earlier,
//! unaffected declarations — the resolve stage stops at the first offending
//! span rather than accumulating. This mainly matters for the future editor
//! service (which wants findings on broken-in-the-middle documents); the
//! batch CLI reports the fatal and moves on, so it is not a `tmt lint` defect.
//! Not fixed here.

mod docs_drift;
pub mod rules;
pub mod tma;

use std::rc::Rc;

use mtc_core::diagnostics::{Diagnostic, Span};
use mtc_core::syntax::{SyntaxNode, TextLineIndex};

use crate::compiler::{self, CompileError, ReadMode, Resolved};
use crate::declarations::Declarations;
use crate::lexer::Token;
use crate::parser::Program;

#[derive(Debug, Clone, Default)]
pub struct LintOptions {
    /// Rule codes to suppress. Unknown codes are an error (typo protection).
    pub allow: Vec<String>,
    /// Opt-in rule codes to ENABLE (the default-off rules, e.g.
    /// `state-may-trap`). Explicit enablement, never allow-removal; unknown
    /// codes are an error, same as `allow`.
    pub warn: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct LintReport {
    /// Lint findings only, source-ordered by span start (stable).
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug)]
pub enum LintError {
    /// Lint requires a program that parses and resolves.
    Compile(CompileError),
    /// `--allow`/`--warn` named a code no rule declares.
    UnknownAllowCode(String),
}

impl std::fmt::Display for LintError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LintError::Compile(e) => write!(f, "{e}"),
            LintError::UnknownAllowCode(code) => {
                write!(f, "unknown lint rule `{code}`")
            }
        }
    }
}

impl std::error::Error for LintError {}

impl From<CompileError> for LintError {
    fn from(e: CompileError) -> Self {
        LintError::Compile(e)
    }
}

/// Everything a rule may read. Rules never mutate the analysis. Every rule
/// works off the resolved module (worlds with source-form rules, alphabets,
/// the doc map); the `unused-import` re-exposure additionally reads analyze's
/// own non-fatal diagnostics.
pub(crate) struct LintContext<'a> {
    /// The resolved module — worlds (rules in source form), alphabets, and the
    /// top-level doc map.
    pub resolved: &'a Resolved,
    /// analyze's own non-fatal diagnostics. The `unused-import` rule re-exposes
    /// its entries under allow control (the compile channel keeps them too).
    pub diagnostics: &'a [Diagnostic],
    /// The parsed AST — source-level detail the resolved module elides (a
    /// signature parameter's own span, read by `unused-exit`).
    pub program: &'a Program,
    /// The green tree's root (docs/core.md (syntax trees)) — the one
    /// `analyze`/`analyze_staged` built. Every quickfix span is a range
    /// query over it (`rules::spans`): the innermost node of a kind
    /// containing an anchor the resolved module keeps, then that node's
    /// own range or a token pair inside it. No rule reads a comment-free
    /// token stream: a comment anywhere in a declaration lands inside the
    /// node's range, where the guard below sees it, instead of voiding or
    /// truncating a span computed by token adjacency.
    pub root: &'a SyntaxNode,
    /// Byte offsets ↔ line/column over the same source, for the range
    /// queries above (`Span` stays the diagnostics' currency).
    pub index: &'a TextLineIndex,
    /// The COMMENT-INCLUSIVE token stream. Read by `run_rules`' shared
    /// guard, which withholds any fix whose edit span holds a comment
    /// (deleting one silently would be a defect fmt itself avoids by
    /// relocating rather than dropping). Filling it costs neither entry
    /// path an extra lex: both are REQUIRED to lex `WithComments` for
    /// reasons of their own, and already hold the stream when they build
    /// this context.
    ///
    /// That requirement is a standing one, not a fact about today's
    /// implementation. `syntax::layout` reconstructs a token's verbatim text
    /// and its surrounding trivia from the source and the stream together,
    /// and asserts that only whitespace separates consecutive tokens — so
    /// any path feeding a green parse must lex `WithComments` or PANIC, not
    /// merely lose comments. Whatever a given entry path parses through, do
    /// not "optimize" it to a comment-free lex on the grounds that its
    /// current parser would tolerate one.
    pub comment_tokens: &'a [Token],
    /// The declaration modules `resolved` was itself resolved against —
    /// `Declarations::stdlib()`, `lint()`'s own fixed default (batch lint
    /// takes no `--extern`). Read by `unreachable-continuation` to decide
    /// whether an EXTERNAL call's callee is KNOWN to be `noreturn`: an
    /// in-unit callee's own `resolved` entry answers that directly, but an
    /// out-of-unit one has no body here to infer from, only whatever
    /// declarations this table happens to carry (docs/tmt/language.md
    /// (declarations)).
    pub externals: &'a Declarations,
}

/// A lint rule: reads the analysis context, pushes any findings.
type Rule = fn(&LintContext, &mut Vec<Diagnostic>);

/// The default-on rule table. One entry per rule, keyed by its defect-named
/// code; registration order is irrelevant (findings are sorted by span).
pub(crate) const RULES: &[(&str, Rule)] = &[
    ("leftover-debugger", rules::leftover_debugger::check),
    ("unused-import", rules::unused_import::check),
    ("unused-routine", rules::unused_routine::check),
    ("unused-graph", rules::unused_graph::check),
    ("unused-binding", rules::unused_binding::check),
    ("unused-graft-instance", rules::unused_graft_instance::check),
    ("unused-graft-name", rules::unused_graft_name::check),
    (
        "duplicate-graft-instance",
        rules::duplicate_graft_instance::check,
    ),
    ("unused-alphabet", rules::unused_alphabet::check),
    ("unused-map", rules::unused_map::check),
    ("unused-set", rules::unused_set::check),
    ("unused-tape", rules::unused_tape::check),
    ("unused-exit", rules::unused_exit::check),
    ("deprecated-call", rules::deprecated_call::check),
    ("dead-rule", rules::dead_rule::check),
    ("dead-map-pair", rules::dead_map_pair::check),
    (
        "redundant-identity-pairs",
        rules::redundant_identity_pairs::check,
    ),
    (
        "binding-product-threshold",
        rules::binding_product_threshold::check,
    ),
    (
        "writes-through-collapse",
        rules::writes_through_collapse::check,
    ),
    (
        "contract-clause-overlap",
        rules::contract_clause_overlap::check,
    ),
    (
        "unreachable-continuation",
        rules::unreachable_continuation::check,
    ),
    ("enters-unmet", rules::enters_unmet::check),
];

/// The opt-in rule table: off by default, run only when `--warn` names the
/// code (the totality lints, deliberately noisy). In the known-code namespace
/// (so a shared allow-list may still name one) but never run unless enabled.
pub(crate) const OPT_IN_RULES: &[(&str, Rule)] = &[
    ("state-may-trap", rules::state_may_trap::check),
    ("index-identity-map", rules::index_identity_map::check),
];

/// The rules that also run on a `.tmh` header ([`lint_header`];
/// docs/tmt/lint.md (linting a header)) — every other code in [`RULES`] and
/// [`OPT_IN_RULES`] is off there. A header is the list of what a unit
/// offers its consumers: its routines carry no body, its graphs' bodies are
/// copies of the library's own source (linted where they are written), and
/// an exported declaration nothing in the header names is a header's
/// normal shape, not dead code. What stays on reads only what a header
/// itself owns — its imports and its signatures' contract clauses. Every
/// entry names a registered rule (guarded below).
pub(crate) const HEADER_RULES: &[&str] = &["unused-import", "contract-clause-overlap"];

/// True when `code` names any rule in this crate's `.tmc` tables, its `.tma`
/// additions ([`tma::TMA_RULES`]), core's arch-agnostic asm rule table
/// (`mtc_core::asm::lint::RULES`), OR core's link-warning catalog
/// (`mtc_core::linker::DIAGNOSTIC_CODES`) — the shared allow namespace, five
/// surfaces wide. One `tmt.json` serves both languages, so a `.tma`-only
/// code must not error when validated for a `.tmc` file, and vice versa.
pub(crate) fn known_code(code: &str) -> bool {
    RULES.iter().any(|(c, _)| *c == code)
        || OPT_IN_RULES.iter().any(|(c, _)| *c == code)
        || tma::TMA_RULES.iter().any(|(c, _)| *c == code)
        || mtc_core::asm::lint::RULES.iter().any(|(c, _)| *c == code)
        // The fifth surface: link warnings share the one allow namespace,
        // so `lint.allow` in `tmt.json` and `--allow` on `link`/`build`
        // suppress them with no new key (docs/tmt/lint.md (the allow
        // namespace)).
        || mtc_core::linker::DIAGNOSTIC_CODES.iter().any(|(c, _)| *c == code)
}

/// `--allow`/`--warn` codes must each name a real rule (typo protection), over
/// the shared namespace. Split out of `lint()` so a caller (the future editor
/// service, `tmt.json` loading) can validate an allow-list up front,
/// independently of running the rules over any particular analysis.
pub fn validate_allow(codes: &[String]) -> Result<(), LintError> {
    for code in codes {
        if !known_code(code) {
            return Err(LintError::UnknownAllowCode(code.clone()));
        }
    }
    Ok(())
}

/// Run every enabled, non-allowed rule over `ctx`, source-ordered by span
/// start (stable). The default table runs unless allowed; an opt-in rule runs
/// only when `warn` names it and `allow` does not. Split out of `lint()` so the
/// editor service can lint an `Analysis` it already has, instead of re-running
/// `compiler::analyze`.
pub(crate) fn run_rules(ctx: &LintContext, allow: &[String], warn: &[String]) -> Vec<Diagnostic> {
    run_rules_where(ctx, allow, warn, |_| true)
}

/// [`run_rules`] restricted to the codes `applies` accepts — every code for
/// a source, [`HEADER_RULES`] for a header.
fn run_rules_where(
    ctx: &LintContext,
    allow: &[String],
    warn: &[String],
    applies: impl Fn(&str) -> bool,
) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    for (code, rule) in RULES {
        if !applies(code) || allow.iter().any(|a| a == code) {
            continue;
        }
        rule(ctx, &mut diagnostics);
    }
    for (code, rule) in OPT_IN_RULES {
        if !applies(code) || !warn.iter().any(|w| w == code) || allow.iter().any(|a| a == code) {
            continue;
        }
        rule(ctx, &mut diagnostics);
    }
    // The comment guard: a fix whose edit span contains a comment token is
    // withheld — the finding stays, the remedy goes — because applying it
    // would silently delete the comment (docs/tmt/lint.md (quickfix
    // availability)). ONE chokepoint over every rule's output rather than a
    // check inside each rule, so a fix-emitting rule added later is covered
    // by construction; `tests/lint_fix_comment_guard.rs` pins the posture per
    // current rule anyway.
    for d in &mut diagnostics {
        let withheld = d.fix.as_ref().is_some_and(|f| {
            f.edits
                .iter()
                .any(|e| span_touches_a_comment(ctx.comment_tokens, e.span))
        });
        if withheld {
            d.fix = None;
        }
    }
    diagnostics.sort_by_key(|d| d.span.start); // stable; Pos is Ord
    diagnostics
}

/// Whether any comment token (from the comment-INCLUSIVE stream) lands
/// inside `span`, under the half-open-range overlap test over [`Span`]'s
/// derived `Ord`.
fn span_touches_a_comment(comment_tokens: &[Token], span: Span) -> bool {
    comment_tokens.iter().any(|t| {
        matches!(t.kind, crate::lexer::TokenKind::Comment(_))
            && t.span().start < span.end
            && span.start < t.span().end
    })
}

/// The glyph labels of a resolved alphabet by mangled name, in position order.
pub(crate) fn alphabet_glyphs<'a>(resolved: &'a Resolved, mangled: &str) -> Option<&'a [String]> {
    resolved.alphabets.get(mangled).map(|a| a.glyphs.as_slice())
}

pub fn lint(source: &str, options: LintOptions) -> Result<LintReport, LintError> {
    lint_in_mode(source, options, ReadMode::Program)
}

/// Lint a `.tmh` header: the source is read declarations-only
/// (docs/tmt/language.md (headers)), so a `machine` block, a bodied
/// routine or a bodiless graph is the same kind of fatal a `.tmc` parse
/// error is, and only [`HEADER_RULES`] run (docs/tmt/lint.md (linting a
/// header)). `allow`/`warn` validate over the whole shared namespace as for
/// a source; naming a rule that does not run on a header is not an error.
pub fn lint_header(source: &str, options: LintOptions) -> Result<LintReport, LintError> {
    lint_in_mode(source, options, ReadMode::DeclarationsOnly)
}

fn lint_in_mode(
    source: &str,
    options: LintOptions,
    mode: ReadMode,
) -> Result<LintReport, LintError> {
    validate_allow(&options.allow)?;
    validate_allow(&options.warn)?;
    // Resolved against `Declarations::stdlib()`, `compiler::analyze`'s own
    // fixed default (batch lint takes no `--extern`) — built here and
    // passed in, since the rules read it too and `Analysis` does not retain
    // the `Declarations` it resolved with (the identical choice
    // `header::render_from_source` makes for `tmt interface`).
    let externals = Declarations::stdlib();
    let analysis = compiler::analyze_with_mode(source, &externals, mode)?;
    // `analyze` lexes WithComments (the green parse needs the trivia); that
    // one stream is the guard's comment channel, and the tree it parsed
    // into is where every quickfix span comes from — the editor path
    // builds the identical context off its staged analysis.
    let root = SyntaxNode::new_root(Rc::clone(&analysis.green));
    let index = TextLineIndex::new(source);
    let ctx = LintContext {
        resolved: &analysis.resolved,
        diagnostics: &analysis.diagnostics,
        program: &analysis.program,
        root: &root,
        index: &index,
        comment_tokens: &analysis.tokens,
        externals: &externals,
    };
    let diagnostics = match mode {
        ReadMode::Program => run_rules(&ctx, &options.allow, &options.warn),
        ReadMode::DeclarationsOnly => run_rules_where(&ctx, &options.allow, &options.warn, |c| {
            HEADER_RULES.contains(&c)
        }),
    };
    Ok(LintReport { diagnostics })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_program_with_no_rules_yields_empty_report() {
        let src = "\
alphabet bit { '_', '1' }
machine {
  tape t: bit;
  entry state s { [*] -> move [>] stop; }
}
";
        let report = lint(src, LintOptions::default()).unwrap();
        assert!(report.diagnostics.is_empty(), "{:?}", report.diagnostics);
    }

    #[test]
    fn unknown_allow_code_is_an_error() {
        let err = lint(
            "machine { }",
            LintOptions {
                allow: vec!["no-such-rule".into()],
                warn: Vec::new(),
            },
        )
        .unwrap_err();
        assert!(matches!(err, LintError::UnknownAllowCode(ref c) if c == "no-such-rule"));
        assert!(err.to_string().contains("no-such-rule"));
    }

    #[test]
    fn fatal_parse_error_propagates() {
        let err = lint("machine {", LintOptions::default()).unwrap_err();
        assert!(matches!(err, LintError::Compile(_)));
    }

    #[test]
    fn validate_allow_accepts_known_codes_and_rejects_unknown_ones() {
        assert!(validate_allow(&["leftover-debugger".to_string()]).is_ok());
        // The opt-in rule is a known code too (a shared allow-list may name it).
        assert!(validate_allow(&["state-may-trap".to_string()]).is_ok());
        assert!(validate_allow(&[]).is_ok());
        let err = validate_allow(&["no-such-rule".to_string()]).unwrap_err();
        assert!(matches!(err, LintError::UnknownAllowCode(ref c) if c == "no-such-rule"));
    }

    #[test]
    fn validate_allow_also_accepts_tma_addition_codes() {
        // The four `.tma` additions share the one namespace: a `tmt.json`
        // naming one must not error when validated for a `.tmc` file, just as
        // an asm-only or `.tmc`-only code must not error the other path.
        for code in [
            "shadowed-wildcard-rows",
            "retx-exit-bounds",
            "rept-var-unused",
            "duplicate-map-source",
        ] {
            assert!(
                !RULES.iter().any(|(c, _)| *c == code),
                "{code} is a .tma addition, not a .tmc rule"
            );
            assert!(tma::TMA_RULES.iter().any(|(c, _)| *c == code));
            assert!(validate_allow(&[code.to_string()]).is_ok());
        }
    }

    #[test]
    fn validate_allow_also_accepts_asm_only_codes() {
        // "unreachable-code" names no `.tmc` rule — it's asm-only
        // (`mtc_core::asm::lint::RULES`). A `tmt.json` shared by both
        // languages must not choke on it while validating for `.tmc`.
        assert!(!RULES.iter().any(|(c, _)| *c == "unreachable-code"));
        assert!(
            mtc_core::asm::lint::RULES
                .iter()
                .any(|(c, _)| *c == "unreachable-code")
        );
        assert!(validate_allow(&["unreachable-code".to_string()]).is_ok());
    }

    /// A header whose one set nothing names: `unused-set` would flag it in
    /// a `.tmc`. The bodiless routine keeps it a header — the program read
    /// rejects it.
    const HEADER_UNUSED_SET: &str = "\
alphabet bit { '_', '1' }

export set ones { '1' }

export routine clear(tape t: bit writes { '_' });
";

    /// Every [`HEADER_RULES`] entry names a registered rule. Mutation it
    /// catches: a typo'd or retired code left in the header list, which
    /// would silently run nothing on a header while reading as enabled.
    #[test]
    fn every_header_rule_names_a_registered_rule() {
        for code in HEADER_RULES {
            assert!(
                RULES.iter().chain(OPT_IN_RULES).any(|(c, _)| c == code),
                "{code} is not a registered .tmc rule"
            );
        }
    }

    /// A construct a body rule flags stays silent in a header. The first
    /// half is the positive control: `unused-set` itself, run over the same
    /// declarations-only analysis, DOES flag the set — so the silence below
    /// is the header flag's doing, not the rule's. Mutation it catches: a
    /// header flag declared but never consulted (the header path running
    /// the full rule table).
    #[test]
    fn a_body_rule_construct_in_a_header_is_silent() {
        let externals = Declarations::stdlib();
        let analysis =
            compiler::analyze_with_mode(HEADER_UNUSED_SET, &externals, ReadMode::DeclarationsOnly)
                .expect("the fixture reads as a header");
        let root = SyntaxNode::new_root(Rc::clone(&analysis.green));
        let index = TextLineIndex::new(HEADER_UNUSED_SET);
        let ctx = LintContext {
            resolved: &analysis.resolved,
            diagnostics: &analysis.diagnostics,
            program: &analysis.program,
            root: &root,
            index: &index,
            comment_tokens: &analysis.tokens,
            externals: &externals,
        };
        let mut control = Vec::new();
        rules::unused_set::check(&ctx, &mut control);
        assert_eq!(
            control.iter().map(|d| d.code).collect::<Vec<_>>(),
            ["unused-set"],
            "positive control: the rule fires on this analysis"
        );
        assert!(!HEADER_RULES.contains(&"unused-set"));

        let report = lint_header(HEADER_UNUSED_SET, LintOptions::default()).unwrap();
        assert!(report.diagnostics.is_empty(), "{:?}", report.diagnostics);
    }

    /// Constructs a header rule flags do report in a header: an unused
    /// import, and a `writes`/`preserves` overlap in a bodiless signature.
    /// Mutation it catches: a header flag that suppresses everything (an
    /// empty [`HEADER_RULES`], or a filter that never admits a code).
    #[test]
    fn a_header_rule_construct_in_a_header_reports() {
        let src = "\
use lib::helper;

alphabet bits { '_', '0', '1' }

export routine mark(tape t: bits writes { '0', '1' } preserves { '1' });
";
        let report = lint_header(src, LintOptions::default()).unwrap();
        let codes: Vec<&str> = report.diagnostics.iter().map(|d| d.code).collect();
        assert_eq!(codes, ["unused-import", "contract-clause-overlap"]);
    }

    /// A header is read declarations-only: the bodiless routine the
    /// fixtures above carry is a program-read fatal, and a `machine` block
    /// is a header-read one. Mutation it catches: `lint_header` analyzing
    /// in the program read mode.
    #[test]
    fn a_header_is_read_declarations_only() {
        assert!(lint(HEADER_UNUSED_SET, LintOptions::default()).is_err());
        let err = lint_header(
            "alphabet b { '_' }\nmachine { tape t: b; entry state s { [*] -> stop; } }\n",
            LintOptions::default(),
        )
        .unwrap_err();
        assert!(
            matches!(&err, LintError::Compile(e) if e.kind.code() == "machine-in-declarations"),
            "{err}"
        );
    }

    /// The fifth surface of the allow namespace: a link-warning code is a
    /// known code. Mutation it catches: drop the `DIAGNOSTIC_CODES` arm of
    /// `known_code` and `--allow narrow-alphabet` on `link`/`build` starts
    /// rejecting a legal code.
    #[test]
    fn validate_allow_also_accepts_link_warning_codes() {
        for (code, _) in mtc_core::linker::DIAGNOSTIC_CODES {
            assert!(
                !RULES.iter().any(|(c, _)| c == code),
                "{code} is not a lint rule"
            );
            assert!(validate_allow(&[(*code).to_string()]).is_ok(), "{code}");
        }
    }
}
