//! `.pmc` compiler driver and shared diagnostics.
//!
//! Every pipeline stage (lexer → parser → lowering → codegen) reports
//! fatals through [`CompileError`]; non-fatal findings accumulate as
//! span-carrying, coded [`Diagnostic`]s — library code never prints
//! (docs/pmt/cli.md).

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use mtc_core::diagnostics::{Diagnostic, Span};
use mtc_core::formats::object::{
    BlobDebug, BlobVariant, ObjectFile, Relocation, Symbol, SymbolDef,
};
use mtc_core::syntax::{GreenNode, SyntaxNode};

use crate::codegen::{CodegenOptions, emit_program};
use crate::ir::IrProgram;
use crate::lexer::{LexMode, Token};
use crate::optimizer::{OptLevel, OptOptions, OptReport, gated_pass_names, optimize};
use crate::parser::{FnDoc, Program};

/// Fatal compile error at a real source span (1-based, char-counted,
/// end-exclusive; see mtc_core::diagnostics).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompileError {
    pub span: Span,
    pub kind: CompileErrorKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompileErrorKind {
    /// Lexical error (unexpected character, unterminated comment, …).
    Lex(String),
    /// The parser needed one thing and saw another.
    Expected {
        what: &'static str,
        found: String,
    },
    /// A reserved word used to name something (`what`: "function",
    /// "namespace", "path segment").
    ReservedName {
        name: String,
        what: &'static str,
    },
    /// A bare identifier statement that is not a builtin (docs/pmt/language.md).
    UnknownCommand(String),
    /// `@` applied to a builtin name (`@left()`).
    BuiltinCalled(String),
    /// Empty `()` on a tape builtin (`left()`), docs/pmt/language.md: parens
    /// on a builtin, if present, must carry a successor. Call parens
    /// (`@f()`) are unaffected.
    EmptyBuiltinParens {
        name: String,
    },
    /// A name already taken in this scope (`what` names the EXISTING
    /// entity: "function" or "namespace").
    DuplicateName {
        name: String,
        what: &'static str,
    },
    DuplicateLabel(u32),
    /// `goto`/`check`/successor names a label the function never declares.
    UndefinedLabel(u32),
    /// `goto !` — docs/pmt/language.md: put `(!)` on the preceding command instead.
    GotoReturn,
    /// A comma-group position rule violated (docs/pmt/language.md, the
    /// statement table's last row).
    GroupPosition(&'static str),
    /// A label at the end of a function body binds to nothing.
    DanglingLabel(u32),
    /// The generated `.pma` failed to assemble — a compiler bug, not a
    /// user error; the message carries the assembler diagnostic.
    Internal(String),
    /// `export` on a nested definition — nesting is always local.
    NestedExport,
    /// Two imports bind one bare name in one scope. Keyed on the binding
    /// name AFTER aliasing (alias if present, else path tail); the same
    /// binding in DIFFERENT scopes is legal (inner shadows outer).
    DuplicateBinding(String),
    /// `namespace {` / `use {` / `export {` — keyword with no name.
    KeywordNeedsName(&'static str),
    /// `use` / `namespace` inside a function body.
    KeywordInBody(&'static str),
    /// A single `:` where a `::` path separator was meant.
    SingleColonInPath,
    /// A command or call at top level (outside any function body).
    TopLevelStatement(String),
    /// A doc/attention run (docs/pmt/language.md (doc lines)) not
    /// immediately followed by a function declaration at its scope.
    /// Span = the run's first line.
    DanglingDocRun,
    /// A `?` doc line appears after the run has already entered its `!`
    /// block — interleaved, or the whole run written `!`-then-`?`.
    DocLineOrder,
    /// An attention line's leading `[ident]` names something other than
    /// the v1 attribute vocabulary (`deprecated`).
    UnknownAttribute(String),
    /// A second `[deprecated]` attribute inside one run.
    DuplicateAttribute,
    /// `volatile` on a definition other than the un-namespaced top-level
    /// `main` — the offending definition's name.
    VolatileNotOnMain(String),
}

/// Binds each [`CompileErrorKind`] variant to its stable code exactly
/// once, expanding to BOTH the exhaustive `code()` match and the `CODES`
/// registry table, so the two cannot diverge: a new variant fails to
/// compile until it gets a row here, and the row lands in the table the
/// completeness and docs drift guards read (docs/pmt/cli.md (compile
/// errors)).
macro_rules! code_registry {
    ($($variant:pat => $code:literal,)+) => {
        /// Every code a [`CompileErrorKind`] can render, in declaration
        /// order — the registry the drift guards set-compare against
        /// the published inventory (docs/pmt/cli.md (compile errors)).
        pub const CODES: &[&str] = &[$($code),+];

        /// Stable kebab-case code, one per variant (docs/pmt/cli.md (compile
        /// errors)). Frozen once published — these are permanent
        /// user-visible identifiers: the CLI brackets them into every fatal
        /// rendering, and the language server carries them in the LSP
        /// diagnostic `code` field (the message stays the kind's own
        /// `Display`, which is why the suffix lives on [`CompileError`]'s
        /// `Display`, not here).
        pub fn code(&self) -> &'static str {
            match self {
                $($variant => $code,)+
            }
        }
    };
}

impl CompileErrorKind {
    code_registry! {
        CompileErrorKind::Lex(_) => "lex-error",
        CompileErrorKind::Expected { .. } => "unexpected-token",
        CompileErrorKind::ReservedName { .. } => "reserved-name",
        CompileErrorKind::UnknownCommand(_) => "unknown-command",
        CompileErrorKind::BuiltinCalled(_) => "builtin-called",
        CompileErrorKind::EmptyBuiltinParens { .. } => "empty-builtin-parens",
        CompileErrorKind::DuplicateName { .. } => "duplicate-name",
        CompileErrorKind::DuplicateLabel(_) => "duplicate-label",
        CompileErrorKind::UndefinedLabel(_) => "undefined-label",
        CompileErrorKind::GotoReturn => "goto-return",
        CompileErrorKind::GroupPosition(_) => "group-position",
        CompileErrorKind::DanglingLabel(_) => "dangling-label",
        CompileErrorKind::Internal(_) => "internal-error",
        CompileErrorKind::NestedExport => "nested-export",
        CompileErrorKind::DuplicateBinding(_) => "duplicate-binding",
        CompileErrorKind::KeywordNeedsName(_) => "keyword-needs-name",
        CompileErrorKind::KeywordInBody(_) => "keyword-in-body",
        CompileErrorKind::SingleColonInPath => "single-colon-in-path",
        CompileErrorKind::TopLevelStatement(_) => "top-level-statement",
        CompileErrorKind::DanglingDocRun => "dangling-doc-run",
        CompileErrorKind::DocLineOrder => "doc-line-order",
        CompileErrorKind::UnknownAttribute(_) => "unknown-attribute",
        CompileErrorKind::DuplicateAttribute => "duplicate-attribute",
        CompileErrorKind::VolatileNotOnMain(_) => "volatile-not-on-main",
    }
}

impl std::fmt::Display for CompileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "line {}:{}: {} [{}]",
            self.span.start.line,
            self.span.start.col,
            self.kind,
            self.kind.code()
        )
    }
}

impl std::fmt::Display for CompileErrorKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CompileErrorKind::Lex(m) => write!(f, "{m}"),
            CompileErrorKind::Expected { what, found } => {
                write!(f, "expected {what}, found {found}")
            }
            CompileErrorKind::ReservedName { name, what } => {
                write!(f, "`{name}` is a reserved word and cannot name a {what}")
            }
            CompileErrorKind::UnknownCommand(n) => {
                write!(
                    f,
                    "unknown command `{n}` (user functions are called `@{n}()`)"
                )
            }
            CompileErrorKind::BuiltinCalled(n) => {
                write!(f, "`{n}` is a builtin — write it without `@`")
            }
            CompileErrorKind::EmptyBuiltinParens { name } => {
                write!(
                    f,
                    "empty parentheses on builtin `{name}` — omit them (`{name}`) or add a successor (`{name}(N)` / `{name}(!)`)"
                )
            }
            CompileErrorKind::DuplicateName { name, what } => {
                write!(
                    f,
                    "duplicate name `{name}` — already used by a {what} in this scope"
                )
            }
            CompileErrorKind::DuplicateLabel(l) => write!(f, "duplicate label `{l}`"),
            CompileErrorKind::UndefinedLabel(l) => write!(f, "undefined label `{l}`"),
            CompileErrorKind::GotoReturn => {
                write!(
                    f,
                    "`goto !` is not allowed — put `(!)` on the preceding command"
                )
            }
            CompileErrorKind::GroupPosition(m) => write!(f, "{m}"),
            CompileErrorKind::DanglingLabel(l) => {
                write!(f, "label `{l}` at end of function binds to nothing")
            }
            CompileErrorKind::Internal(m) => write!(f, "internal compiler error: {m}"),
            CompileErrorKind::NestedExport => {
                write!(f, "nested functions are always local — remove `export`")
            }
            CompileErrorKind::DuplicateBinding(n) => {
                write!(
                    f,
                    "`{n}` is bound twice — qualify the call (`@ns::{n}()`) or disambiguate with `as`"
                )
            }
            CompileErrorKind::KeywordNeedsName(kw) => match *kw {
                "use" => write!(f, "`use` needs a name — did you mean `use <name>;`?"),
                "export" => write!(
                    f,
                    "`export` needs a name — did you mean `export <name>() {{ … }}`?"
                ),
                _ => write!(
                    f,
                    "`namespace` needs a name — did you mean `namespace <name> {{ … }}`?"
                ),
            },
            CompileErrorKind::KeywordInBody(kw) => {
                write!(
                    f,
                    "`{kw}` is not allowed inside a function body — imports and namespaces live at file or namespace level"
                )
            }
            CompileErrorKind::SingleColonInPath => {
                write!(f, "single `:` in a name path — did you mean `::`?")
            }
            CompileErrorKind::TopLevelStatement(found) => {
                write!(
                    f,
                    "statements are not allowed at top level — commands and calls live inside function bodies (found {found})"
                )
            }
            CompileErrorKind::DanglingDocRun => {
                write!(
                    f,
                    "doc/attention run is not attached to a function declaration"
                )
            }
            CompileErrorKind::DocLineOrder => {
                write!(
                    f,
                    "doc lines (`?`) must come before attention lines (`!`) in a run"
                )
            }
            CompileErrorKind::UnknownAttribute(name) => {
                write!(
                    f,
                    "unknown attribute `[{name}]` — the only recognized attribute is `[deprecated]`"
                )
            }
            CompileErrorKind::DuplicateAttribute => {
                write!(f, "duplicate `[deprecated]` attribute in the same run")
            }
            CompileErrorKind::VolatileNotOnMain(name) => {
                write!(
                    f,
                    "`volatile` is only allowed on the top-level `main` — remove it from `{name}`"
                )
            }
        }
    }
}

impl std::error::Error for CompileError {}

/// Which build columns a compilation emits. Every function is built
/// twice — the normal pipeline and the gated one a volatile program needs
/// — unless the caller already knows which column will be linked
/// (docs/pmt/language.md (volatile programs)).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum VariantColumns {
    /// Both columns, deduped per function. The rule for anything that
    /// outlives the invocation: an on-disk `.pmo` cannot know its future
    /// program.
    #[default]
    Both,
    NormalOnly,
    VolatileOnly,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CompileOptions {
    /// `-g`: record label/line debug info in the object, with lines
    /// remapped to `.pmc` sources.
    pub debug_info: bool,
    /// `--strip-debugger`: drop `brk` at codegen (docs/pmt/cli.md). The
    /// optimizer runs BEFORE stripping, so `brk` barriers always hold.
    pub strip_debugger: bool,
    /// `-O0` (default) or `-O1` (docs/pmt/language.md (optimization)).
    pub opt_level: OptLevel,
    /// Pass names to disable (`--fno-<pass>`), e.g. `"cell-state"`.
    pub disabled_passes: Vec<String>,
    /// Capture per-stage IR snapshots (`--emit-ir=<stage>` backing):
    /// `"lowered"`, `"after:<pass>"` per changing pass, `"final"`.
    pub capture_ir: bool,
    /// Which build columns to emit (default: both).
    pub columns: VariantColumns,
    /// Override `inline`'s size cap (`None` = the shipped constant). A
    /// measurement knob for the optimizer sweep, not CLI surface — no flag,
    /// no completions entry.
    pub inline_cap: Option<usize>,
}

/// Structured stage report — `pmt -v` renders it; the library never
/// prints (docs/pmt/cli.md, the same pattern as the linker's `LinkReport`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompileReport {
    pub diagnostics: Vec<Diagnostic>,
    pub opt: OptReport,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompileOutput {
    /// The ONE object every built column merges into: per function, a
    /// single `Both`-tagged blob when the columns came out identical, else
    /// the normal and volatile blobs adjacent under one name
    /// (docs/formats.md (MO)).
    pub object: ObjectFile,
    /// The generated assembly (`-S` output) of the column this compile
    /// RENDERS — the normal one, unless only the volatile column was asked
    /// for. Each column is assembled from its own listing, so the code
    /// bytes of the rendered column can never disagree with the object's;
    /// the object as a whole additionally carries the other column's blobs
    /// and the variant tags, so `object != assemble(pma, …)` for a
    /// two-column build. Under `-g` the object's debug LINES are also
    /// remapped to `.pmc` sources, so they differ on that side table too.
    pub pma: String,
    /// The FINAL CFG of the same column `pma` renders (post-optimizer at
    /// -O1; the lowered CFG at -O0).
    pub ir: IrProgram,
    /// The volatile column's final CFG whenever that column was built —
    /// including a volatile-only build, where it equals `ir`. `None` only
    /// when the volatile column was skipped.
    pub ir_volatile: Option<IrProgram>,
    /// Per-stage IR snapshots when `capture_ir` was set; empty otherwise.
    /// One column's — the same one `ir` and `pma` describe.
    pub ir_snapshots: Vec<(String, IrProgram)>,
    pub report: CompileReport,
}

/// flatten's per-scope name maps, retained for scope-aware lint rules
/// instead of being discarded. `Clone` because the LSP's `DocState`
/// keeps a last-good copy for completion staleness (docs/lsp.md).
#[derive(Clone)]
pub(crate) struct ScopeSummary {
    /// ns path -> (bare name -> full mangled name)
    pub defs: HashMap<Vec<String>, HashMap<String, String>>,
    /// ns path -> (bare name -> (import index, full `::` path))
    pub bindings: HashMap<Vec<String>, HashMap<String, (usize, String)>>,
}

/// How flatten resolved one call site (docs/lsp.md (go-to-definition)).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Resolution {
    /// A function this module defines (incl. nested and qualified-internal).
    Local { def_name_span: Span },
    /// A bare name bound by a `use` import.
    ImportBinding { use_span: Span, full_path: String },
    /// A `@ns::name()` call whose target this module does NOT define.
    QualifiedExternal { full_path: String },
    /// A bare undeclared external.
    Unresolved,
}

/// The codegen-free front half of the pipeline: everything the lint
/// layer (and a future LSP) needs, nothing it doesn't.
pub(crate) struct AnalysisOutput {
    pub tokens: Vec<Token>,
    pub ast: Program,
    pub ir: IrProgram,
    pub diagnostics: Vec<Diagnostic>,
    pub scopes: ScopeSummary,
    /// Every documented function's [`FnDoc`], keyed by its fully-qualified
    /// flattened name — the same map `Analysis.docs` carries for the
    /// staged pipeline. `lint()`'s `LintContext` reads it from here.
    pub docs: HashMap<String, FnDoc>,
}

/// `ir::lower` the flattened AST and merge its diagnostics with flatten's
/// visibility warnings — the shared tail of `analyze()` and
/// `analyze_staged()`. Ordering contract: `ir::lower`'s diagnostics come
/// FIRST, then flatten's `vis` warnings, appended in `vis`'s own order.
/// Both callers rely on this exact order (docs/lsp.md (staged analysis)
/// documents it on `Analysis.warnings`); stated once here so it can't
/// drift between the two call sites.
fn lower_and_merge(
    program: &Program,
    vis: Vec<Diagnostic>,
) -> Result<(IrProgram, Vec<Diagnostic>), CompileError> {
    let (ir, mut diagnostics) = crate::ir::lower(program)?;
    diagnostics.extend(vis);
    Ok((ir, diagnostics))
}

/// lex → green parse → extract → duplicate-binding check → flatten →
/// lower. Stops before the optimizer; `compile()` composes this with the
/// back half.
///
/// The parse is the green one (docs/core.md (syntax trees)): one
/// `WithComments` lex feeds both `parse_green_from_tokens` and — through
/// `significant_tokens` — the `tokens` field, which stays exactly the
/// `WithoutComments` stream it has always been (pinned by
/// `tests/syntax_green.rs::corpus_token_provenance_law`). `extract_program`
/// rebuilds the same `Program` the C1 path built, held to it by
/// `tests/syntax_green.rs::corpus_extraction_parity`.
pub(crate) fn analyze(source: &str) -> Result<AnalysisOutput, CompileError> {
    let lexed = crate::lexer::lex_with(source, LexMode::WithComments)?;
    let green = crate::parser::parse_green_from_tokens(source, &lexed)?;
    let parsed = crate::syntax::extract_program(&SyntaxNode::new_root(green), source);
    let tokens = crate::parser::significant_tokens(&lexed);
    check_duplicate_bindings(&parsed)?;
    let Flattened {
        program,
        scopes,
        warnings: vis,
        resolutions: _,
        docs,
    } = flatten(parsed);
    let (ir, diagnostics) = lower_and_merge(&program, vis)?;
    Ok(AnalysisOutput {
        tokens,
        ast: program,
        ir,
        diagnostics,
        scopes,
        docs,
    })
}

/// The post-parse half of a staged analysis (docs/lsp.md (staged
/// analysis)): the flattened AST, its scope summary, the merged warnings
/// (ir diagnostics then flatten's visibility warnings — the same order
/// `analyze()` produces), and the per-call-site resolution table.
pub(crate) struct Analysis {
    pub ast: Program,
    pub scopes: ScopeSummary,
    pub warnings: Vec<Diagnostic>,
    pub resolutions: Vec<(Span, Resolution)>,
    /// Every documented function's [`FnDoc`], keyed by the SAME
    /// fully-qualified name `flatten` mangles onto `Function::name`
    /// (nested dot-mangling, namespace `::` paths). Undocumented
    /// functions are absent, not present with an empty `FnDoc`. Read by
    /// the `deprecated-call` lint rule via `LintContext.docs`, and by
    /// hover (`lsp::mod::hover`, docs/lsp.md (hover)) — for a `std::…`
    /// name this map always misses; hover falls back to
    /// `crate::stdlib::docs()`, the same map built from THIS field over
    /// the embedded stdlib's own analysis.
    pub docs: HashMap<String, FnDoc>,
}

/// The LSP's pipeline entry (docs/lsp.md (staged analysis)): every stage's
/// outcome, retained independently, so a document that fails partway
/// through still serves whatever the earlier stages produced. `None`
/// fields past the first failure; `fatal` carries that one error.
pub(crate) struct StagedAnalysis {
    /// WithComments — `None` only if lexing itself failed.
    pub tokens: Option<Vec<Token>>,
    /// Green syntax tree of the current text (docs/core.md (syntax
    /// trees)); `None` when lexing or parsing failed. The `.pmc` language
    /// service's position walks index by byte range against this tree.
    pub green: Option<Rc<GreenNode>>,
    /// `None` if any stage failed (parse, duplicate-binding check, or
    /// lowering).
    pub analysis: Option<Analysis>,
    /// The first (only) fatal, at whichever stage produced it.
    pub fatal: Option<CompileError>,
}

/// lex (WithComments) → green parse → extract → duplicate-binding check
/// → flatten → ir::lower, retaining each stage's outcome instead of
/// stopping at the first failure. Extraction and `flatten` are
/// infallible, so the only post-parse fatals are `DuplicateBinding` (the
/// binding check) and `UndefinedLabel` (`ir::lower`) — the pipeline
/// always runs through `ir::lower`, never stopping at `flatten`. The
/// `IrProgram` itself is discarded once `ir::lower` has had its say: the
/// LSP's tiers only need the flattened `Analysis`, not the CFG.
pub(crate) fn analyze_staged(source: &str) -> StagedAnalysis {
    let lexed = match crate::lexer::lex_with(source, LexMode::WithComments) {
        Ok(tokens) => tokens,
        Err(fatal) => {
            return StagedAnalysis {
                tokens: None,
                green: None,
                analysis: None,
                fatal: Some(fatal),
            };
        }
    };
    let green = match crate::parser::parse_green_from_tokens(source, &lexed) {
        Ok(green) => green,
        Err(fatal) => {
            return StagedAnalysis {
                tokens: Some(lexed),
                green: None,
                analysis: None,
                fatal: Some(fatal),
            };
        }
    };
    let green_retained = Some(Rc::clone(&green));
    let program = crate::syntax::extract_program(&SyntaxNode::new_root(green), source);
    let tokens = lexed;
    if let Err(fatal) = check_duplicate_bindings(&program) {
        return StagedAnalysis {
            tokens: Some(tokens),
            green: green_retained,
            analysis: None,
            fatal: Some(fatal),
        };
    }
    let Flattened {
        program,
        scopes,
        warnings: vis,
        resolutions,
        docs,
    } = flatten(program);
    match lower_and_merge(&program, vis) {
        Ok((_ir, warnings)) => StagedAnalysis {
            tokens: Some(tokens),
            green: green_retained,
            analysis: Some(Analysis {
                ast: program,
                scopes,
                warnings,
                resolutions,
                docs,
            }),
            fatal: None,
        },
        Err(fatal) => StagedAnalysis {
            tokens: Some(tokens),
            green: green_retained,
            analysis: None,
            fatal: Some(fatal),
        },
    }
}

/// `.pmc` source → object file: lex → parse → lower → optimize each
/// column → emit `.pma` → assemble → merge. Assembly failure of GENERATED
/// text is a compiler bug and reports as `CompileErrorKind::Internal`.
///
/// One lowering feeds BOTH columns (docs/pmt/language.md (volatile
/// programs)): the normal one is today's pipeline exactly, the volatile
/// one runs it with the gated passes disabled on top of the caller's own
/// `--fno-` set. Functions whose two columns assemble identically dedup to
/// a single `Both`-tagged blob.
pub fn compile(source: &str, options: CompileOptions) -> Result<CompileOutput, CompileError> {
    let analysis = analyze(source)?;
    let AnalysisOutput {
        ast,
        ir: lowered,
        diagnostics,
        ..
    } = analysis;
    // The program bit records what the SOURCE declared, never which
    // columns this invocation happened to build: a single-column build of
    // a plain program is still a non-volatile program. Only the top-level
    // `main` can carry the modifier (the parser rejects it anywhere else).
    let program_volatile = ast.functions.iter().any(|f| f.volatile);

    let build_normal = !matches!(options.columns, VariantColumns::VolatileOnly);
    let build_volatile = !matches!(options.columns, VariantColumns::NormalOnly);

    let mut ir_snapshots = Vec::new();
    if options.capture_ir {
        ir_snapshots.push(("lowered".to_string(), lowered.clone()));
    }
    let user_disabled: HashSet<String> = options.disabled_passes.iter().cloned().collect();
    let mut gated_disabled = user_disabled.clone();
    gated_disabled.extend(gated_pass_names().iter().map(|name| (*name).to_string()));

    // `pma`, `ir`, `ir_snapshots` and the opt report all describe the
    // PRIMARY column — normal unless only the volatile one was asked for.
    // The secondary column's snapshots are discarded so the `--emit-ir`
    // stage list stays one column's.
    let mut discarded_snapshots = Vec::new();
    let normal = if build_normal {
        Some(build_column(
            lowered.clone(),
            &options,
            &user_disabled,
            options.capture_ir,
            &mut ir_snapshots,
        )?)
    } else {
        None
    };
    let volatile = if build_volatile {
        let primary = !build_normal;
        let snapshots = if primary {
            &mut ir_snapshots
        } else {
            &mut discarded_snapshots
        };
        Some(build_column(
            lowered,
            &options,
            &gated_disabled,
            options.capture_ir && primary,
            snapshots,
        )?)
    } else {
        None
    };

    let (object, pma, ir, ir_volatile, opt) = match (normal, volatile) {
        (Some(n), Some(v)) => (
            merge_columns(&n.object, &v.object, program_volatile)?,
            n.pma,
            n.ir,
            Some(v.ir),
            n.opt,
        ),
        (Some(n), None) => (
            tag_single_column(n.object, BlobVariant::Normal, program_volatile),
            n.pma,
            n.ir,
            None,
            n.opt,
        ),
        (None, Some(v)) => (
            tag_single_column(v.object, BlobVariant::Volatile, program_volatile),
            v.pma,
            v.ir.clone(),
            Some(v.ir),
            v.opt,
        ),
        (None, None) => unreachable!("every VariantColumns builds at least one column"),
    };

    Ok(CompileOutput {
        object,
        pma,
        ir,
        ir_volatile,
        ir_snapshots,
        report: CompileReport { diagnostics, opt },
    })
}

/// One build column's outputs.
struct Column {
    ir: IrProgram,
    pma: String,
    object: ObjectFile,
    opt: OptReport,
}

/// Optimize one copy of the lowered CFG under `disabled`, emit its `.pma`,
/// assemble it, and remap its debug lines with ITS OWN line map. The two
/// columns number their listings differently, so the remap belongs here,
/// per column, before any merge can compare debug side tables.
fn build_column(
    mut ir: IrProgram,
    options: &CompileOptions,
    disabled: &HashSet<String>,
    capture: bool,
    snapshots: &mut Vec<(String, IrProgram)>,
) -> Result<Column, CompileError> {
    let opt = optimize(
        &mut ir,
        &OptOptions {
            level: options.opt_level,
            disabled: disabled.clone(),
            capture,
            inline_cap: options.inline_cap,
        },
        snapshots,
    );
    if capture {
        snapshots.push(("final".to_string(), ir.clone()));
    }
    let pma = emit_program(
        &ir,
        CodegenOptions {
            strip_debugger: options.strip_debugger,
        },
    );
    let mut object =
        crate::asm::assemble(&pma.text, options.debug_info).map_err(|e| CompileError {
            span: Span::point(0, 0),
            kind: CompileErrorKind::Internal(format!("generated .pma failed to assemble: {e}")),
        })?;
    if options.debug_info {
        remap_debug_lines(&mut object, &pma.line_map);
    }
    Ok(Column {
        ir,
        pma: pma.text,
        object,
        opt,
    })
}

/// Tag a single-column object: every blob is that column, and the program
/// bit still comes from the source.
fn tag_single_column(
    mut object: ObjectFile,
    tag: BlobVariant,
    program_volatile: bool,
) -> ObjectFile {
    object.variants = Some(vec![tag; object.blobs.len()]);
    object.program_volatile = program_volatile;
    object
}

/// A compiler-bug fatal at no particular source span.
fn internal(message: String) -> CompileError {
    CompileError {
        span: Span::point(0, 0),
        kind: CompileErrorKind::Internal(message),
    }
}

/// Everything about one blob that decides whether the two columns built
/// the same function: its code, its relocations as `(offset, callee NAME)`
/// — the two columns number their symbol tables independently, so indices
/// are not comparable — and its debug side table.
type BlobRecord<'a> = (&'a [u8], Vec<(u32, &'a str)>, Option<&'a BlobDebug>);

fn blob_record(object: &ObjectFile, blob: usize) -> BlobRecord<'_> {
    let mut relocations: Vec<(u32, &str)> = object
        .relocations
        .iter()
        .filter(|r| r.blob as usize == blob)
        .map(|r| (r.offset, object.symbols[r.symbol as usize].name.as_str()))
        .collect();
    relocations.sort_unstable();
    (
        object.blobs[blob].as_slice(),
        relocations,
        object.debug.as_ref().map(|per_blob| &per_blob[blob]),
    )
}

/// Merge the two assembled columns into one object.
///
/// Both columns come from one lowering and no pass adds, drops or reorders
/// functions, so blob `i` is the same function in both — the merge pairs
/// by index and checks that rather than trusting it. Per function: kept as
/// ONE `Both`-tagged blob under one symbol, or emitted as the normal blob
/// then the volatile blob adjacent, with two symbols carrying the same
/// name and the same visibility.
///
/// Deduping is **transitive**, not per function. A function may be `Both`
/// only if its own two records match AND every function it calls inside
/// this object is `Both` too. Record equality alone is not enough: a
/// function that merely calls others is byte-identical in both columns and
/// names the same callees, so it would dedup — and then its single blob
/// would have to bind ONE column of a callee that split, silently pinning
/// a volatile program's entry to the normal call graph. Demotion
/// propagates to a fixpoint (it only ever removes `Both`, so it
/// terminates); the cost is more splitting, never a wrong edge. Calls that
/// leave the object are excluded — those resolve by name at link time.
/// Self-recursion is not a demotion: a `Both` function calling itself
/// still only references `Both`.
///
/// Relocations are rewritten **column-coherent**: a call from the normal
/// column binds the callee's normal symbol, a call from the volatile
/// column binds its volatile one (a deduped callee has just the one). That
/// is what keeps a volatile program's whole call graph gated — the
/// linker's local-symbol path binds a relocation's blob directly
/// (docs/core.md (linking)).
fn merge_columns<'a>(
    normal: &'a ObjectFile,
    volatile: &'a ObjectFile,
    program_volatile: bool,
) -> Result<ObjectFile, CompileError> {
    for (which, object) in [("normal", normal), ("volatile", volatile)] {
        if object.signatures.is_some()
            || object.table_blobs.is_some()
            || !object.table_fixups.is_empty()
            || !object.bound_calls.is_empty()
        {
            return Err(internal(format!(
                "{which} column carries table or signature records PM-1 never emits"
            )));
        }
    }
    let count = normal.blobs.len();
    if count != volatile.blobs.len() {
        return Err(internal(format!(
            "columns disagree on function count: normal {count}, volatile {}",
            volatile.blobs.len()
        )));
    }

    // Pass 1: per-function record equality, then demote to a fixpoint any
    // candidate that calls a function which is not itself deduped.
    let mut deduped: Vec<bool> = (0..count)
        .map(|index| blob_record(normal, index) == blob_record(volatile, index))
        .collect();
    let index_of: HashMap<&str, usize> = (0..count)
        .map(|index| (normal.symbols[index].name.as_str(), index))
        .collect();
    // Intra-object callees per function. A dedup candidate's two columns
    // reference the same names by construction (the record comparison
    // includes relocations keyed by callee name), so reading the normal
    // column's is enough for every function whose status can still change.
    let callees: Vec<Vec<usize>> = (0..count)
        .map(|index| {
            let mut targets: Vec<usize> = normal
                .relocations
                .iter()
                .filter(|r| r.blob as usize == index)
                .filter_map(|r| {
                    index_of
                        .get(normal.symbols[r.symbol as usize].name.as_str())
                        .copied()
                })
                .collect();
            targets.sort_unstable();
            targets.dedup();
            targets
        })
        .collect();
    loop {
        let mut demoted = false;
        for (index, targets) in callees.iter().enumerate() {
            if deduped[index] && targets.iter().any(|&callee| !deduped[callee]) {
                deduped[index] = false;
                demoted = true;
            }
        }
        if !demoted {
            break;
        }
    }

    let mut blobs: Vec<Vec<u8>> = Vec::with_capacity(count);
    let mut variants: Vec<BlobVariant> = Vec::with_capacity(count);
    let mut symbols: Vec<Symbol> = Vec::with_capacity(count);
    let mut debug: Option<Vec<BlobDebug>> = normal.debug.as_ref().map(|_| Vec::new());
    // Callee name -> (normal symbol index, volatile symbol index); the two
    // are equal for a deduped function.
    let mut defined: HashMap<&str, (u32, u32)> = HashMap::new();
    // Where each merged blob came from, for the relocation pass.
    let mut sources: Vec<(bool /* from the volatile column */, usize)> = Vec::new();

    for (index, &keep) in deduped.iter().enumerate() {
        let symbol = &normal.symbols[index];
        let blob = match symbol.def {
            SymbolDef::Defined { blob } | SymbolDef::Local { blob } => blob as usize,
            SymbolDef::External => usize::MAX,
        };
        if blob != index || volatile.symbols[index].name != symbol.name {
            return Err(internal(format!(
                "columns disagree on function {index}: normal `{}`, volatile `{}`",
                symbol.name, volatile.symbols[index].name
            )));
        }

        let mut push = |from_volatile: bool, tag: BlobVariant| -> u32 {
            let object = if from_volatile { volatile } else { normal };
            let merged = blobs.len() as u32;
            blobs.push(object.blobs[index].clone());
            variants.push(tag);
            if let Some(per_blob) = debug.as_mut() {
                per_blob
                    .push(object.debug.as_ref().expect("both columns carry debug")[index].clone());
            }
            let symbol_index = symbols.len() as u32;
            symbols.push(Symbol {
                name: symbol.name.clone(),
                def: match symbol.def {
                    SymbolDef::Local { .. } => SymbolDef::Local { blob: merged },
                    _ => SymbolDef::Defined { blob: merged },
                },
            });
            sources.push((from_volatile, index));
            symbol_index
        };

        if keep {
            let only = push(false, BlobVariant::Both);
            defined.insert(symbol.name.as_str(), (only, only));
        } else {
            let n = push(false, BlobVariant::Normal);
            let v = push(true, BlobVariant::Volatile);
            defined.insert(symbol.name.as_str(), (n, v));
        }
    }

    // External symbols are minted in first-reference order over the merged
    // blobs — the order re-assembling the merged listing would produce.
    let mut relocations: Vec<Relocation> = Vec::new();
    let mut externals: HashMap<&str, u32> = HashMap::new();
    for (merged, &(from_volatile, index)) in sources.iter().enumerate() {
        let object = if from_volatile { volatile } else { normal };
        let mut per_blob: Vec<&Relocation> = object
            .relocations
            .iter()
            .filter(|r| r.blob as usize == index)
            .collect();
        per_blob.sort_by_key(|r| r.offset);
        for relocation in per_blob {
            let name = object.symbols[relocation.symbol as usize].name.as_str();
            let symbol = match defined.get(name) {
                Some(&(n, v)) => {
                    if from_volatile {
                        v
                    } else {
                        n
                    }
                }
                None => *externals.entry(name).or_insert_with(|| {
                    let fresh = symbols.len() as u32;
                    symbols.push(Symbol {
                        name: name.to_string(),
                        def: SymbolDef::External,
                    });
                    fresh
                }),
            };
            relocations.push(Relocation {
                blob: merged as u32,
                offset: relocation.offset,
                symbol,
            });
        }
    }

    Ok(ObjectFile {
        arch: normal.arch,
        symbols,
        blobs,
        relocations,
        debug,
        signatures: None,
        table_blobs: None,
        table_fixups: Vec::new(),
        bound_calls: Vec::new(),
        variants: Some(variants),
        program_volatile,
    })
}

/// Two imports binding one bare name in one scope collide — checked
/// right after parse, keyed on `(ns, binding name)` where the binding
/// name is the POST-ALIAS one. An exactly-duplicate `use` (same path
/// and alias) is tolerated: the duplicate surfaces as an unused-import
/// warning from flatten instead.
fn check_duplicate_bindings(program: &crate::parser::Program) -> Result<(), CompileError> {
    use std::collections::HashMap;
    let mut seen: HashMap<(&[String], &str), &crate::parser::Import> = HashMap::new();
    for import in &program.imports {
        match seen.entry((import.ns.as_slice(), import.binding())) {
            std::collections::hash_map::Entry::Occupied(prev) => {
                let p = prev.get();
                if p.path != import.path || p.alias != import.alias {
                    return Err(CompileError {
                        span: import.span,
                        kind: CompileErrorKind::DuplicateBinding(import.binding().to_string()),
                    });
                }
            }
            std::collections::hash_map::Entry::Vacant(v) => {
                v.insert(import);
            }
        }
    }
    Ok(())
}

/// The full symbol name of a top-level function: namespaces join with
/// `::` (`std::api`); un-namespaced names have no `::`. Function
/// nesting appends `.` segments later (`std::api.helper`) — every
/// symbol self-decomposes at the last `::`. `pub(crate)`: the LSP's
/// completion layer reuses this exact formula (never a re-derivation)
/// to qualify nested-def candidates against `Analysis.docs`' keys.
pub(crate) fn full_name(ns: &[String], name: &str) -> String {
    if ns.is_empty() {
        name.to_string()
    } else {
        format!("{}::{}", ns.join("::"), name)
    }
}

/// flatten's full output: the mangled program, per-scope name maps (for
/// lint), the visibility warnings, the per-call-site resolution table
/// (for a future LSP), and the qualified doc map (for a future
/// `deprecated-call` lint rule and hover).
struct Flattened {
    program: crate::parser::Program,
    scopes: ScopeSummary,
    warnings: Vec<Diagnostic>,
    resolutions: Vec<(Span, Resolution)>,
    docs: HashMap<String, FnDoc>,
}

/// Flatten definitions and resolve calls (docs/pmt/language.md (visibility)):
/// mangle names (`::` for namespaces, `.` for nesting), resolve
/// each call innermost-outward — the function's own nested maps (defs
/// only), then per enclosing namespace prefix (longest first) that
/// level's definitions THEN its import bindings — and compute symbol
/// locality. Call names containing `::` are ABSOLUTE: they skip the
/// scope chain and imports, stay verbatim, and are self-declaring (no
/// undeclared warning). Infallible — unresolved bare names simply stay
/// external; all visibility warnings (undeclared externals, unused
/// imports, unused functions) are produced here. Every call site also
/// records a [`Resolution`] into the returned table — a pure side
/// channel that does not influence mangling, warnings, or codegen.
/// Every documented function's [`FnDoc`] is also copied into `docs`,
/// keyed by the exact fully-qualified `name` this pass just mangled
/// onto it — the doc map's qualification piggybacks on mangling instead
/// of recomputing it.
fn flatten(program: crate::parser::Program) -> Flattened {
    use crate::parser::{Function, Item, Program};
    use std::collections::{HashSet, VecDeque};

    let Program { functions, imports } = program;

    // Per-scope structures, keyed by ns path:
    //   defs:     ns-path -> (bare name -> full name)
    //   bindings: ns-path -> (bare name -> (import index, full "::" path))
    let mut defs: HashMap<Vec<String>, HashMap<String, String>> = HashMap::new();
    for f in &functions {
        defs.entry(f.ns.clone())
            .or_default()
            .insert(f.name.clone(), full_name(&f.ns, &f.name));
    }
    let mut bindings: HashMap<Vec<String>, HashMap<String, (usize, String)>> = HashMap::new();
    for (i, imp) in imports.iter().enumerate() {
        // First-wins: an exactly-duplicate import keeps the first slot,
        // so the duplicate is never marked used and warns as unused.
        bindings
            .entry(imp.ns.clone())
            .or_default()
            .entry(imp.binding().to_string())
            .or_insert_with(|| (i, imp.full_path()));
    }
    // Qualified (`::`) calls can only target namespace-level functions
    // (never `.`-nested ones — `defs` above is built from the top-level
    // `functions` list only), so this union is exactly the set of full
    // names a qualified call may legally hit.
    let defs_by_full_name: HashSet<String> =
        defs.values().flat_map(|m| m.values().cloned()).collect();

    /// Internal mirror of [`Resolution`], recorded during the walk. Local
    /// carries the MANGLED name (a call can target a nested function not
    /// yet emitted into `out`); ImportBinding carries the import index.
    /// A post-pass at the end of `flatten` converts both once `out` (and
    /// `imports`) are in hand — see the post-pass comment below.
    enum RawResolution {
        Local { mangled: String },
        ImportBinding { index: usize },
        QualifiedExternal { full_path: String },
        Unresolved,
    }

    struct Ctx {
        defs: HashMap<Vec<String>, HashMap<String, String>>,
        bindings: HashMap<Vec<String>, HashMap<String, (usize, String)>>,
        defs_by_full_name: HashSet<String>,
        imports_used: Vec<bool>,
        warned_undeclared: HashSet<String>,
        warnings: Vec<Diagnostic>,
        resolutions: Vec<(Span, RawResolution)>,
    }

    let mut ctx = Ctx {
        defs,
        bindings,
        defs_by_full_name,
        imports_used: vec![false; imports.len()],
        warned_undeclared: HashSet::new(),
        warnings: Vec::new(),
        resolutions: Vec::new(),
    };

    /// Resolve one call name in place, innermost scope outward, and
    /// record exactly one [`RawResolution`] for this call site.
    fn resolve(
        name: &mut String,
        span: mtc_core::diagnostics::Span,
        nested: &[HashMap<String, String>],
        ns: &[String],
        ctx: &mut Ctx,
    ) {
        // Absolute: leave verbatim, no warning, no import consumption
        // (self-declaring; resolves internally iff this module defines
        // that symbol, else stays external). Qualified calls can only
        // hit namespace-level defs (`defs_by_full_name`'s contract).
        if name.contains("::") {
            let raw = if ctx.defs_by_full_name.contains(name.as_str()) {
                RawResolution::Local {
                    mangled: name.clone(),
                }
            } else {
                RawResolution::QualifiedExternal {
                    full_path: name.clone(),
                }
            };
            ctx.resolutions.push((span, raw));
            return;
        }
        for scope in nested.iter().rev() {
            if let Some(m) = scope.get(name.as_str()) {
                *name = m.clone();
                ctx.resolutions.push((
                    span,
                    RawResolution::Local {
                        mangled: name.clone(),
                    },
                ));
                return;
            }
        }
        // Enclosing namespace levels, innermost outward; each level's
        // definitions outrank its import bindings.
        for k in (0..=ns.len()).rev() {
            let prefix = &ns[..k];
            if let Some(m) = ctx.defs.get(prefix).and_then(|d| d.get(name.as_str())) {
                *name = m.clone();
                ctx.resolutions.push((
                    span,
                    RawResolution::Local {
                        mangled: name.clone(),
                    },
                ));
                return;
            }
            if let Some(&(idx, ref path)) =
                ctx.bindings.get(prefix).and_then(|b| b.get(name.as_str()))
            {
                ctx.imports_used[idx] = true;
                *name = path.clone();
                ctx.resolutions
                    .push((span, RawResolution::ImportBinding { index: idx }));
                return;
            }
        }
        // Total miss: the call stays a bare external; warn once per name
        // (the resolution entry, unlike the warning, is unconditional).
        if ctx.warned_undeclared.insert(name.clone()) {
            ctx.warnings.push(Diagnostic {
                code: "undeclared-external",
                span,
                message: format!(
                    "call to undeclared external `{name}` — declare it with `use {name};`"
                ),
                fix: None,
            });
        }
        ctx.resolutions.push((span, RawResolution::Unresolved));
    }

    fn emit(
        mut f: Function,
        prefix: &str,
        ns: &[String],
        nested_scopes: &[HashMap<String, String>],
        ctx: &mut Ctx,
        out: &mut Vec<Function>,
    ) {
        let full = if prefix.is_empty() {
            full_name(ns, &f.name)
        } else {
            format!("{prefix}.{}", f.name)
        };
        // This function's own children are visible inside its body.
        let child_map: HashMap<String, String> = f
            .nested
            .iter()
            .map(|c| (c.name.clone(), format!("{full}.{}", c.name)))
            .collect();
        let mut inner = nested_scopes.to_vec();
        inner.push(child_map);

        for stmt in &mut f.body {
            for item in &mut stmt.items {
                if let Item::Call {
                    name, name_span, ..
                } = item
                {
                    resolve(name, *name_span, &inner, ns, ctx);
                }
            }
        }

        let children = std::mem::take(&mut f.nested);
        let is_nested = !prefix.is_empty();
        f.local = is_nested || !f.exported;
        f.exported = f.exported && !is_nested;
        f.name = full.clone();
        out.push(f);
        for c in children {
            emit(c, &full, ns, &inner, ctx, out);
        }
    }

    let mut out: Vec<Function> = Vec::new();
    for f in functions {
        let ns = f.ns.clone();
        emit(f, "", &ns, &[], &mut ctx, &mut out);
    }

    let Ctx {
        defs,
        bindings,
        imports_used,
        mut warnings,
        resolutions: raw_resolutions,
        ..
    } = ctx;

    // Unused imports: none of the import's bindings resolved any call.
    for (i, imp) in imports.iter().enumerate() {
        if !imports_used[i] {
            warnings.push(Diagnostic {
                code: "unused-import",
                span: imp.span,
                message: format!("unused import `{}`", imp.full_path()),
                fix: None,
            });
        }
    }

    // Unused functions: reachability over the FLATTENED functions;
    // roots = exports + the bare top-level `main` (namespaced exports
    // are roots — outside callers unknowable); edges from resolved
    // internal calls (full names, qualified self-calls included).
    let defined: HashSet<&str> = out.iter().map(|f| f.name.as_str()).collect();
    let mut edges: HashMap<&str, Vec<&str>> = HashMap::new();
    for f in &out {
        let mut callees = Vec::new();
        for stmt in &f.body {
            for item in &stmt.items {
                if let Item::Call { name, .. } = item
                    && defined.contains(name.as_str())
                {
                    callees.push(name.as_str());
                }
            }
        }
        edges.insert(f.name.as_str(), callees);
    }
    let mut reached: HashSet<&str> = HashSet::new();
    let mut queue: VecDeque<&str> = out
        .iter()
        .filter(|f| f.exported)
        .map(|f| f.name.as_str())
        .collect();
    while let Some(name) = queue.pop_front() {
        if !reached.insert(name) {
            continue;
        }
        if let Some(callees) = edges.get(name) {
            for c in callees {
                queue.push_back(c);
            }
        }
    }
    for f in &out {
        if !reached.contains(f.name.as_str()) {
            warnings.push(Diagnostic {
                code: "unused-function",
                span: f.name_span,
                message: format!("unused function `{}` (not exported, never called)", f.name),
                fix: None,
            });
        }
    }

    // Post-pass: RawResolution -> Resolution. Done here, after `out` is
    // fully built, rather than inline in `resolve` — a call can target a
    // nested function that hasn't been emitted into `out` yet at the
    // point it's resolved, so `mangled -> def_name_span` can only be
    // looked up once every flattened function is in hand. Post-mangle
    // names are unique, so this map is exact.
    let def_name_span_by_mangled: HashMap<&str, Span> =
        out.iter().map(|f| (f.name.as_str(), f.name_span)).collect();
    // Doc-map qualification piggybacks on the mangling `emit` already did
    // — `f.name` IS the fully-qualified name by this point (nested
    // dot-mangling, namespace `::` paths). Undocumented functions
    // (`f.doc == None`) are simply absent from the map.
    let docs: HashMap<String, FnDoc> = out
        .iter()
        .filter_map(|f| f.doc.clone().map(|doc| (f.name.clone(), doc)))
        .collect();
    let resolutions: Vec<(Span, Resolution)> = raw_resolutions
        .into_iter()
        .map(|(span, raw)| {
            let resolution = match raw {
                RawResolution::Local { mangled } => Resolution::Local {
                    def_name_span: *def_name_span_by_mangled
                        .get(mangled.as_str())
                        .unwrap_or_else(|| {
                            panic!(
                                "flatten: resolved local `{mangled}` has no flattened definition"
                            )
                        }),
                },
                RawResolution::ImportBinding { index } => Resolution::ImportBinding {
                    use_span: imports[index].span,
                    full_path: imports[index].full_path(),
                },
                RawResolution::QualifiedExternal { full_path } => {
                    Resolution::QualifiedExternal { full_path }
                }
                RawResolution::Unresolved => Resolution::Unresolved,
            };
            (span, resolution)
        })
        .collect();

    Flattened {
        program: Program {
            functions: out,
            imports,
        },
        scopes: ScopeSummary { defs, bindings },
        warnings,
        resolutions,
        docs,
    }
}

/// The name inside the first backtick pair of an `undeclared-external`
/// message — this function's own fixed format
/// ("call to undeclared external `NAME` — ..."), pinned by
/// `undeclared_name_matches_the_warning_format` below.
pub(crate) fn undeclared_name(message: &str) -> Option<&str> {
    let start = message.find('`')? + 1;
    let rest = &message[start..];
    Some(&rest[..rest.find('`')?])
}

/// The build driver and the language server refine this warning the same
/// way wherever a full link set is declared: a bare call the declared set
/// defines stops warning (docs/pmt/cli.md (undeclared-external)).
pub(crate) fn refine_undeclared(
    diags: &mut Vec<Diagnostic>,
    defined: &std::collections::HashSet<String>,
) {
    diags.retain(|d| {
        !(d.code == "undeclared-external"
            && undeclared_name(&d.message).is_some_and(|n| defined.contains(n)))
    });
}

/// The assembler recorded `(code_offset, pma_line)`; compose with the
/// codegen's `(pma_line, pmc_line)` map so debug info speaks `.pmc`.
/// Offsets with no source correspondence (synthetic returns) are dropped.
fn remap_debug_lines(object: &mut ObjectFile, line_map: &[(u32, u32)]) {
    let to_pmc: std::collections::HashMap<u32, u32> = line_map.iter().copied().collect();
    if let Some(per_blob) = &mut object.debug {
        for d in per_blob {
            d.lines = d
                .lines
                .iter()
                .filter_map(|&(off, pma_line)| to_pmc.get(&pma_line).map(|&l| (off, l)))
                .collect();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;
    use crate::lexer::TokenKind;
    use mtc_core::formats::object::SymbolDef;

    #[test]
    fn compile_error_carries_a_real_span() {
        // Pins the shape this task introduces: a `CompileError` is built
        // from a genuine `span: Span` (not a degenerate point) plus
        // `kind`, and `Display` reads the span's START only.
        let e = CompileError {
            span: Span::new(1, 7, 1, 12),
            kind: CompileErrorKind::UndefinedLabel(3),
        };
        assert_eq!((e.span.start.col, e.span.end.col), (7, 12));
        assert_eq!(
            format!("{e}"),
            "line 1:7: undefined label `3` [undefined-label]"
        );
    }

    #[test]
    fn error_codes_are_pairwise_distinct() {
        // One representative kind per variant (all 24), in declaration
        // order. The `code_registry!` expansion already ties every
        // variant to a `CODES` row structurally (one list feeds both
        // the match and the table); this witness list additionally
        // proves the rows come out in declaration order and stay
        // pairwise distinct kebab-case.
        let kinds = [
            CompileErrorKind::Lex("x".into()),
            CompileErrorKind::Expected {
                what: "x",
                found: "y".into(),
            },
            CompileErrorKind::ReservedName {
                name: "x".into(),
                what: "y",
            },
            CompileErrorKind::UnknownCommand("x".into()),
            CompileErrorKind::BuiltinCalled("x".into()),
            CompileErrorKind::EmptyBuiltinParens { name: "x".into() },
            CompileErrorKind::DuplicateName {
                name: "x".into(),
                what: "y",
            },
            CompileErrorKind::DuplicateLabel(1),
            CompileErrorKind::UndefinedLabel(1),
            CompileErrorKind::GotoReturn,
            CompileErrorKind::GroupPosition("x"),
            CompileErrorKind::DanglingLabel(1),
            CompileErrorKind::Internal("x".into()),
            CompileErrorKind::NestedExport,
            CompileErrorKind::DuplicateBinding("x".into()),
            CompileErrorKind::KeywordNeedsName("use"),
            CompileErrorKind::KeywordInBody("use"),
            CompileErrorKind::SingleColonInPath,
            CompileErrorKind::TopLevelStatement("x".into()),
            CompileErrorKind::DanglingDocRun,
            CompileErrorKind::DocLineOrder,
            CompileErrorKind::UnknownAttribute("x".into()),
            CompileErrorKind::DuplicateAttribute,
            CompileErrorKind::VolatileNotOnMain("x".into()),
        ];
        let witnessed: Vec<&str> = kinds.iter().map(|k| k.code()).collect();
        assert_eq!(
            witnessed,
            CompileErrorKind::CODES,
            "the witness list and the CODES registry disagree"
        );
        let unique: std::collections::HashSet<&str> = witnessed.iter().copied().collect();
        assert_eq!(unique.len(), kinds.len(), "duplicate code: {witnessed:?}");
        for code in CompileErrorKind::CODES {
            assert!(
                !code.is_empty()
                    && !code.starts_with('-')
                    && !code.ends_with('-')
                    && !code.contains("--")
                    && code
                        .chars()
                        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'),
                "code `{code}` is not kebab-case"
            );
        }
    }

    #[test]
    fn duplicate_label_display_carries_the_bracketed_code() {
        let e = CompileError {
            span: Span::new(3, 7, 3, 8),
            kind: CompileErrorKind::DuplicateLabel(5),
        };
        assert_eq!(
            format!("{e}"),
            "line 3:7: duplicate label `5` [duplicate-label]"
        );
    }

    #[test]
    fn compiles_to_an_object_with_symbols_and_relocations() {
        let out = compile(
            "use goToEnd; main() { @goToEnd(); mark; }",
            CompileOptions::default(),
        )
        .unwrap();
        assert!(
            out.object
                .symbols
                .iter()
                .any(|s| s.name == "main" && matches!(s.def, SymbolDef::Defined { .. }))
        );
        assert!(
            out.object
                .symbols
                .iter()
                .any(|s| s.name == "goToEnd" && matches!(s.def, SymbolDef::External))
        );
        assert_eq!(out.object.relocations.len(), 1);
        assert!(out.report.diagnostics.is_empty());
    }

    #[test]
    fn object_equals_assembly_of_the_emitted_pma() {
        let out = compile("f() { 1: right; check(1, !); }", CompileOptions::default()).unwrap();
        let direct = crate::asm::assemble(&out.pma, false).unwrap();
        // At -O0 the two build columns are identical by construction, so
        // they dedup and the object holds exactly the emitted listing's
        // blobs — plus the variant tags, which a directive-free listing
        // does not carry.
        assert_eq!(out.object.variants, Some(vec![BlobVariant::Both]));
        let mut object = out.object;
        object.variants = None;
        assert_eq!(object, direct);
    }

    #[test]
    fn debug_lines_speak_pmc_not_pma() {
        let src = "main() {\n    right;\n    mark;\n}";
        let out = compile(
            src,
            CompileOptions {
                debug_info: true,
                strip_debugger: false,
                ..Default::default()
            },
        )
        .unwrap();
        let debug = out.object.debug.as_ref().unwrap();
        let lines = &debug[0].lines;
        // Blob: ent@0, rgt@1, wr@2..3, stp@4. Sources: right; = pmc line 2,
        // mark; = line 3, implicit stp ← the line-3 statement.
        assert!(lines.contains(&(1, 2)), "{lines:?}");
        assert!(lines.contains(&(2, 3)), "{lines:?}");
        assert!(lines.contains(&(4, 3)), "{lines:?}");
    }

    #[test]
    fn warnings_flow_into_the_report() {
        let out = compile(
            "f() { goto 1; right; 1: left; } main() { @f(); }",
            CompileOptions::default(),
        )
        .unwrap();
        assert_eq!(out.report.diagnostics.len(), 1);
    }

    #[test]
    fn strip_debugger_reaches_the_bytes() {
        let src = "main() { debugger; mark; }";
        let kept = compile(src, CompileOptions::default()).unwrap();
        assert!(kept.object.blobs[0].contains(&crate::arch::opcodes::BRK));
        let stripped = compile(
            src,
            CompileOptions {
                debug_info: false,
                strip_debugger: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(!stripped.object.blobs[0].contains(&crate::arch::opcodes::BRK));
    }

    #[test]
    fn o1_on_unoptimizable_program_is_identity() {
        let src = "main() { right; mark; }";
        let o0 = compile(src, CompileOptions::default()).unwrap();
        let o1 = compile(
            src,
            CompileOptions {
                opt_level: crate::optimizer::OptLevel::O1,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(o0.object, o1.object);
        assert_eq!(o1.report.opt.rounds, 1);
        assert!(o1.report.opt.changes.is_empty());
    }

    #[test]
    fn inline_cap_widens_what_the_program_pass_admits() {
        // An 8-op callee called from two sites: default (`None`) keeps the
        // shipped `INLINE_MAX_OPS` cap (6), so the call survives (the
        // single-call-site escape doesn't fire at two sites); `Some(12)`
        // widens the size arm and both calls splice away. End-to-end
        // through `compile()`, proving `CompileOptions.inline_cap` actually
        // reaches `OptOptions.inline_cap` — `optimizer::inline::run` is
        // tested directly in optimizer/inline.rs; this pins the
        // compiler-level plumbing on top of it. Eight IDENTICAL moves, not
        // an out-and-back sequence: an adjacent inverse pair (`right;
        // left;`) is exactly what `move-elim` deletes, which would shrink
        // this callee below the cap across -O1's later rounds regardless
        // of the override under test and defeat the fixture.
        let src = "big() { right; right; right; right; right; right; right; right; } \
                   main() { @big(); @big(); }";

        let default_out = compile(
            src,
            CompileOptions {
                opt_level: OptLevel::O1,
                ..Default::default()
            },
        )
        .unwrap();
        let main = default_out
            .ir
            .functions
            .iter()
            .find(|f| f.name == "main")
            .unwrap();
        assert!(
            main.blocks
                .iter()
                .flat_map(|b| &b.ops)
                .any(|op| matches!(op, crate::ir::IrOp::Call { name, .. } if name == "big")),
            "default cap keeps the call to the oversize, multiply-called callee"
        );

        let widened_out = compile(
            src,
            CompileOptions {
                opt_level: OptLevel::O1,
                inline_cap: Some(12),
                ..Default::default()
            },
        )
        .unwrap();
        let main = widened_out
            .ir
            .functions
            .iter()
            .find(|f| f.name == "main")
            .unwrap();
        assert!(
            main.blocks.iter().all(|b| b
                .ops
                .iter()
                .all(|op| !matches!(op, crate::ir::IrOp::Call { .. }))),
            "CompileOptions.inline_cap: Some(12) reaches the optimizer and admits the callee"
        );
    }

    #[test]
    fn capture_ir_yields_lowered_and_final() {
        let out = compile(
            "main() { mark; }",
            CompileOptions {
                capture_ir: true,
                ..Default::default()
            },
        )
        .unwrap();
        let stages: Vec<&str> = out.ir_snapshots.iter().map(|(s, _)| s.as_str()).collect();
        assert_eq!(stages, vec!["lowered", "final"]);
        assert_eq!(out.ir_snapshots[0].1, out.ir_snapshots[1].1); // -O0: identical
    }

    #[test]
    fn flatten_mangles_resolves_and_localizes() {
        let out = compile(
            "export api() { helper() { right; } @helper(); } helper() { left; } main() { @api(); }",
            CompileOptions::default(),
        )
        .unwrap();
        let names: Vec<(&str, bool)> = out
            .ir
            .functions
            .iter()
            .map(|f| (f.name.as_str(), f.local))
            .collect();
        assert!(names.contains(&("api", false)));
        assert!(names.contains(&("api.helper", true)));
        assert!(names.contains(&("helper", true))); // shadowed, untouched
        assert!(names.contains(&("main", false)));
        let api = out.ir.functions.iter().find(|f| f.name == "api").unwrap();
        assert!(api.blocks.iter().any(|b| b.ops.iter().any(|op| matches!(
            op, crate::ir::IrOp::Call { name, .. } if name == "api.helper"
        ))));
    }

    #[test]
    fn codegen_prints_the_local_modifier() {
        let out = compile(
            "helper() { right; } main() { @helper(); }",
            CompileOptions::default(),
        )
        .unwrap();
        assert!(out.pma.contains(".func helper local"), "{}", out.pma);
        assert!(out.pma.contains(".func main\n"), "{}", out.pma);
    }

    #[test]
    fn undeclared_external_warns_once_and_use_silences() {
        let out = compile("main() { @go(); right; @go(); }", CompileOptions::default()).unwrap();
        let n = out
            .report
            .diagnostics
            .iter()
            .filter(|d| d.code == "undeclared-external")
            .count();
        assert_eq!(n, 1);
        assert!(
            out.report
                .diagnostics
                .iter()
                .any(|d| d.code == "undeclared-external" && d.message.contains("undeclared"))
        );
        let out = compile("use go; main() { @go(); }", CompileOptions::default()).unwrap();
        assert!(
            out.report
                .diagnostics
                .iter()
                .all(|d| d.code != "undeclared-external")
        );
    }

    #[test]
    fn unused_imports_and_unused_functions_warn() {
        let out = compile("use ghost; main() { mark; }", CompileOptions::default()).unwrap();
        assert!(
            out.report
                .diagnostics
                .iter()
                .any(|d| d.code == "unused-import" && d.message.contains("unused import `ghost`"))
        );

        let out = compile(
            "dead() { left; } main() { mark; }",
            CompileOptions::default(),
        )
        .unwrap();
        assert!(
            out.report.diagnostics.iter().any(
                |d| d.code == "unused-function" && d.message.contains("unused function `dead`")
            )
        );

        // Transitively dead: a called only by dead — both warn.
        let out = compile(
            "a() { left; } dead() { @a(); } main() { mark; }",
            CompileOptions::default(),
        )
        .unwrap();
        let n = out
            .report
            .diagnostics
            .iter()
            .filter(|d| d.code == "unused-function")
            .count();
        assert_eq!(n, 2);

        // Exported functions never warn (outside callers unknowable).
        let out = compile(
            "export api() { left; } main() { mark; }",
            CompileOptions::default(),
        )
        .unwrap();
        assert!(
            out.report
                .diagnostics
                .iter()
                .all(|d| d.code != "unused-function")
        );
    }

    #[test]
    fn duplicate_bindings_error_and_exact_duplicates_only_warn() {
        let e = compile(
            "use goToEnd; use std::goToEnd; main() { @goToEnd(); }",
            CompileOptions::default(),
        )
        .unwrap_err();
        assert!(format!("{e}").contains("bound twice"));
        assert!(matches!(e.kind, CompileErrorKind::DuplicateBinding(n) if n == "goToEnd"));
        // An exactly-duplicate `use` line is tolerated — the duplicate
        // never binds a call, so it surfaces as an unused import.
        let out = compile(
            "use go; use go; main() { @go(); }",
            CompileOptions::default(),
        )
        .unwrap();
        assert!(
            out.report
                .diagnostics
                .iter()
                .any(|d| d.code == "unused-import" && d.message.contains("unused import `go`"))
        );
        assert!(
            out.report
                .diagnostics
                .iter()
                .all(|d| d.code != "undeclared-external")
        );
    }

    #[test]
    fn namespaced_locality_follows_export() {
        // Locality is the unchanged rule applied to the FULL name:
        // `export` inside a namespace → Defined `std::f`; unexported →
        // Local `std::g`.
        let out = compile(
            "namespace std { export f() { left; } g() { right; } } main() { @std::f(); @std::g(); }",
            CompileOptions::default(),
        )
        .unwrap();
        assert!(
            out.object
                .symbols
                .iter()
                .any(|s| s.name == "std::f" && matches!(s.def, SymbolDef::Defined { .. }))
        );
        assert!(
            out.object
                .symbols
                .iter()
                .any(|s| s.name == "std::g" && matches!(s.def, SymbolDef::Local { .. }))
        );
    }

    #[test]
    fn use_named_function_still_parses() {
        assert!(
            compile(
                "use() { left; } main() { @use(); }",
                CompileOptions::default()
            )
            .is_ok()
        );
    }

    #[test]
    fn uncalled_nested_functions_warn_under_their_mangled_name() {
        let out = compile(
            "export api() { helper() { right; } left; } main() { @api(); }",
            CompileOptions::default(),
        )
        .unwrap();
        assert!(
            out.report
                .diagnostics
                .iter()
                .any(|d| d.code == "unused-function"
                    && d.message.contains("unused function `api.helper`")),
            "{:?}",
            out.report.diagnostics
        );
    }

    #[test]
    fn compile_warnings_carry_codes_and_spans() {
        let src = "use std::go;\nmain() { right; }\nhelper() { left; }\n";
        let out = compile(src, CompileOptions::default()).unwrap();
        let codes: Vec<&str> = out.report.diagnostics.iter().map(|d| d.code).collect();
        assert!(codes.contains(&"unused-import"));
        assert!(codes.contains(&"unused-function"));
        let unused_import = out
            .report
            .diagnostics
            .iter()
            .find(|d| d.code == "unused-import")
            .unwrap();
        // "std::go" on line 1, cols 5..12
        assert_eq!(
            (unused_import.span.start.line, unused_import.span.start.col),
            (1, 5)
        );
        assert!(out.report.diagnostics.iter().all(|d| d.fix.is_none()));
    }

    #[test]
    fn analyze_stops_before_the_optimizer_and_keeps_the_raw_material() {
        let src = "use std::go;\nmain() { right; }\n";
        let a = analyze(src).unwrap();
        assert!(!a.tokens.is_empty());
        assert_eq!(a.ir.functions.len(), 1);
        assert!(a.diagnostics.iter().any(|d| d.code == "unused-import"));
        // flatten's scope summary is retained, not discarded:
        assert!(a.scopes.defs.contains_key(&Vec::<String>::new()));
        assert!(a.scopes.bindings.contains_key(&Vec::<String>::new()));
        // compile() reports exactly what analyze() found:
        let out = compile(src, CompileOptions::default()).unwrap();
        assert_eq!(out.report.diagnostics, a.diagnostics);
    }

    #[test]
    fn undeclared_external_dedup_is_program_wide() {
        let out = compile(
            "a() { @go(); } main() { @a(); @go(); }",
            CompileOptions::default(),
        )
        .unwrap();
        let n = out
            .report
            .diagnostics
            .iter()
            .filter(|d| d.code == "undeclared-external")
            .count();
        assert_eq!(n, 1);
    }

    #[test]
    fn undeclared_external_warning_span_is_the_first_occurrence() {
        // `warned_undeclared.insert` (above) short-circuits every
        // occurrence after the first, so the ONE warning that survives
        // the repeated-name dedup carries the FIRST call site's span,
        // never the second's — the detail the dedup-count tests above
        // don't pin.
        let out = compile("main() { @go(); right; @go(); }", CompileOptions::default()).unwrap();
        let warnings: Vec<_> = out
            .report
            .diagnostics
            .iter()
            .filter(|d| d.code == "undeclared-external")
            .collect();
        assert_eq!(warnings.len(), 1, "{warnings:#?}");
        assert_eq!(warnings[0].span, Span::new(1, 11, 1, 13), "the first `go`");
    }

    #[test]
    fn flatten_records_a_resolution_per_call_site() {
        // One call per line so each `name_span` has a hand-checkable
        // column. Exercises every `Resolution` arm:
        //   @helper()        -> Local (nested)
        //   @ns::inner()     -> Local (qualified-internal)
        //   @inner()         -> Unresolved (ns member not visible bare)
        //   @ext()           -> ImportBinding (unaliased)
        //   @ge()            -> ImportBinding (aliased std import)
        //   @other::thing()  -> QualifiedExternal
        //   @mystery()       -> Unresolved (bare undeclared external)
        let src = "use ext;\nuse std::goToEnd as ge;\nnamespace ns { export inner() { right; } }\nexport main() {\n    helper() { left; }\n    @helper();\n    @ns::inner();\n    @inner();\n    @ext();\n    @ge();\n    @other::thing();\n    @mystery();\n}\n";
        let staged = analyze_staged(src);
        let a = staged.analysis.expect("the pipeline succeeded");
        assert_eq!(a.resolutions.len(), 7, "{:#?}", a.resolutions);

        // Exact-span lookup (not just the start position): the brief's
        // point is that each entry is keyed by the call's REAL
        // `name_span`, so pin both ends.
        let at = |call_span: Span| -> Resolution {
            a.resolutions
                .iter()
                .find(|(span, _)| *span == call_span)
                .unwrap_or_else(|| {
                    panic!(
                        "no resolution recorded at {call_span:?}: {:#?}",
                        a.resolutions
                    )
                })
                .1
                .clone()
        };

        // "helper" name_span at its nested definition, line 5 cols 5..11.
        assert_eq!(
            at(Span::new(6, 6, 6, 12)), // "helper" in "@helper()"
            Resolution::Local {
                def_name_span: Span::new(5, 5, 5, 11)
            }
        );
        // "inner" name_span at its namespace-level definition, line 3.
        assert_eq!(
            at(Span::new(7, 6, 7, 15)), // "ns::inner" in "@ns::inner()"
            Resolution::Local {
                def_name_span: Span::new(3, 23, 3, 28)
            }
        );
        assert_eq!(at(Span::new(8, 6, 8, 11)), Resolution::Unresolved); // "inner" in "@inner()"
        assert_eq!(
            at(Span::new(9, 6, 9, 9)), // "ext" in "@ext()"
            Resolution::ImportBinding {
                use_span: Span::new(1, 5, 1, 8),
                full_path: "ext".to_string(),
            }
        );
        assert_eq!(
            at(Span::new(10, 6, 10, 8)), // "ge" in "@ge()"
            Resolution::ImportBinding {
                use_span: Span::new(2, 5, 2, 17),
                full_path: "std::goToEnd".to_string(),
            }
        );
        assert_eq!(
            at(Span::new(11, 6, 11, 18)), // "other::thing" in "@other::thing()"
            Resolution::QualifiedExternal {
                full_path: "other::thing".to_string(),
            }
        );
        assert_eq!(at(Span::new(12, 6, 12, 13)), Resolution::Unresolved); // "mystery" in "@mystery()"

        // Pure side channel: the existing undeclared-external warnings
        // (one per bare-miss name: "inner", "mystery") still fire.
        let undeclared = a
            .warnings
            .iter()
            .filter(|d| d.code == "undeclared-external")
            .count();
        assert_eq!(undeclared, 2, "{:#?}", a.warnings);
    }

    #[test]
    fn resolution_table_is_a_pure_side_channel() {
        // Same source, compiled through the full pipeline: object bytes,
        // pma text, and diagnostics are unaffected by the resolution
        // table's existence — it rides alongside, nothing reads it here.
        let src = "use ext;\nuse std::goToEnd as ge;\nnamespace ns { export inner() { right; } }\nexport main() {\n    helper() { left; }\n    @helper();\n    @ns::inner();\n    @inner();\n    @ext();\n    @ge();\n    @other::thing();\n    @mystery();\n}\n";
        let out = compile(src, CompileOptions::default()).unwrap();
        assert!(out.pma.contains(".func main"));
        assert!(out.pma.contains(".func ns::inner"));
    }

    #[test]
    fn flatten_qualifies_docs_by_the_same_fully_mangled_name_it_computes() {
        // Four documented shapes: top-level, top-level NESTED
        // (dot-mangled, no `::` prefix), namespaced, and namespaced
        // nested (dot-mangled onto the namespaced name) — plus one
        // undocumented top-level function and a bare `main`, which must
        // both stay absent from the map.
        let src = "\
? top-level doc
export documented() { right; }

undocumented() { left; }

parent() {
? top-level nested doc
    child() { right; }
    right;
}

namespace ns {
? namespaced doc
export inner() {
? nested doc
! [deprecated] use outer instead
    nested_helper() { right; }
    right;
}
}

main() { mark; }
";
        let staged = analyze_staged(src);
        let a = staged.analysis.expect("the pipeline succeeded");

        assert_eq!(
            a.docs
                .keys()
                .map(String::as_str)
                .collect::<std::collections::HashSet<_>>(),
            [
                "documented",
                "parent.child",
                "ns::inner",
                "ns::inner.nested_helper"
            ]
            .into_iter()
            .collect::<std::collections::HashSet<_>>(),
            "{:#?}",
            a.docs
        );

        let top = &a.docs["documented"];
        assert_eq!(top.paragraphs, vec!["top-level doc".to_string()]);
        assert!(top.attention.is_empty());
        assert_eq!(top.deprecated, None);

        let top_nested = &a.docs["parent.child"];
        assert_eq!(
            top_nested.paragraphs,
            vec!["top-level nested doc".to_string()]
        );
        assert_eq!(top_nested.deprecated, None);

        let ns_fn = &a.docs["ns::inner"];
        assert_eq!(ns_fn.paragraphs, vec!["namespaced doc".to_string()]);
        assert_eq!(ns_fn.deprecated, None);

        let nested = &a.docs["ns::inner.nested_helper"];
        assert_eq!(nested.paragraphs, vec!["nested doc".to_string()]);
        assert_eq!(nested.deprecated, Some("use outer instead".to_string()));
    }

    #[test]
    fn analyze_staged_on_clean_source_matches_analyze_at_every_stage() {
        // Tier 1: nothing fails — all four fields Some/None as documented,
        // and the staged pipeline's findings agree with analyze()'s on
        // the same source. A leading comment proves `staged.tokens` is a
        // genuine WithComments stream (not just coincidentally equal to
        // WithoutComments because the fixture had nothing to filter) —
        // filtering it of Comment entries must still match analyze()'s
        // WithoutComments stream exactly.
        let src = "// a leading comment\nuse ext;\nuse std::goToEnd as ge;\nnamespace ns { export inner() { right; } }\nexport main() {\n    helper() { left; }\n    @helper();\n    @ns::inner();\n    @inner();\n    @ext();\n    @ge();\n    @other::thing();\n    @mystery();\n}\n";
        let staged = analyze_staged(src);
        assert!(staged.fatal.is_none());
        let tokens = staged.tokens.as_ref().expect("lexing succeeded");
        assert!(staged.green.is_some(), "parsing succeeded");
        let analysis = staged.analysis.as_ref().expect("the pipeline succeeded");

        let a = analyze(src).unwrap();

        // WithComments is genuinely in effect: at least one Comment token
        // is present (otherwise the filter below would be a no-op and
        // wouldn't prove anything).
        assert!(
            tokens
                .iter()
                .any(|t| matches!(t.kind, TokenKind::Comment(_))),
            "fixture's leading comment should surface as a Comment token"
        );

        // The WithComments significant-token stream filtered of Comment
        // trivia is byte-identical to the WithoutComments stream.
        let significant: Vec<TokenKind> = tokens
            .iter()
            .filter(|t| !matches!(t.kind, TokenKind::Comment(_)))
            .map(|t| t.kind.clone())
            .collect();
        let expected: Vec<TokenKind> = a.tokens.iter().map(|t| t.kind.clone()).collect();
        assert_eq!(significant, expected);

        assert_eq!(analysis.ast, a.ast);
        assert_eq!(analysis.warnings, a.diagnostics);
        assert_eq!(analysis.scopes.defs, a.scopes.defs);
        assert_eq!(analysis.scopes.bindings, a.scopes.bindings);
    }

    #[test]
    fn analyze_staged_lex_failure_degrades_everything_to_none() {
        // Tier 2: lexing itself fails — no tokens, no tree, no analysis.
        let staged = analyze_staged("/* never closed");
        assert!(staged.tokens.is_none());
        assert!(staged.green.is_none());
        assert!(staged.analysis.is_none());
        let fatal = staged.fatal.expect("a fatal is recorded");
        assert_eq!(fatal.kind.code(), "lex-error");
    }

    #[test]
    fn analyze_staged_parse_failure_keeps_tokens_but_not_the_tree() {
        // Tier 3: lexing succeeds, parsing does not (a bare identifier
        // statement that is not a builtin — CompileErrorKind::UnknownCommand).
        let staged = analyze_staged("f() { gibberish; }");
        assert!(staged.tokens.is_some(), "lexing still succeeded");
        assert!(staged.green.is_none());
        assert!(staged.analysis.is_none());
        let fatal = staged.fatal.expect("a fatal is recorded");
        assert_eq!(fatal.kind.code(), "unknown-command");
    }

    #[test]
    fn analyze_staged_duplicate_binding_keeps_the_tree_but_not_analysis() {
        // Tier 4: parsing succeeds (the green tree is a faithful reprint
        // of two legal `use` lines), the post-parse duplicate-binding
        // check fails before flatten/ir::lower ever run.
        let staged = analyze_staged("use goToEnd; use std::goToEnd; main() { @goToEnd(); }");
        assert!(staged.tokens.is_some());
        assert!(staged.green.is_some(), "parsing succeeded");
        assert!(staged.analysis.is_none());
        let fatal = staged.fatal.expect("a fatal is recorded");
        assert_eq!(fatal.kind.code(), "duplicate-binding");
    }

    #[test]
    fn analyze_staged_lower_failure_proves_the_pipeline_reaches_ir_lower() {
        // Tier 5: parsing and the duplicate-binding check both succeed;
        // flatten (infallible) runs; only ir::lower fails, on an
        // undefined label. This is the case that proves analyze_staged
        // does not stop at flatten.
        let staged = analyze_staged("main() { goto 99; }");
        assert!(staged.tokens.is_some());
        assert!(staged.green.is_some(), "parsing succeeded");
        assert!(staged.analysis.is_none());
        let fatal = staged.fatal.expect("a fatal is recorded");
        assert_eq!(fatal.kind.code(), "undefined-label");
    }

    /// Pins the extraction against this module's REAL warning format — if
    /// the message ever changes shape, this fails here rather than
    /// silently breaking the refinement (moved from cli/driver.rs, which
    /// now delegates to `refine_undeclared` here).
    #[test]
    fn undeclared_name_matches_the_warning_format() {
        let out = compile("main() { @go(); }", CompileOptions::default()).unwrap();
        let diag = out
            .report
            .diagnostics
            .iter()
            .find(|d| d.code == "undeclared-external")
            .expect("bare @go() warns");
        assert_eq!(undeclared_name(&diag.message), Some("go"));
    }

    #[test]
    fn refine_undeclared_drops_only_defined_undeclared_externals() {
        // All three diagnostics come from a real compile — not hand-typed
        // strings — so this test cannot silently drift away from the
        // compiler's actual message shapes the way a copied literal could.
        let out = compile(
            "use ghost; main() { @a(); @b(); }",
            CompileOptions::default(),
        )
        .unwrap();
        let mut diags = out.report.diagnostics;
        assert_eq!(
            diags
                .iter()
                .filter(|d| d.code == "undeclared-external")
                .count(),
            2,
            "both bare calls should warn undeclared"
        );
        assert!(diags.iter().any(|d| d.code == "unused-import"));

        let defined: HashSet<String> = ["a".to_string()].into_iter().collect();
        refine_undeclared(&mut diags, &defined);

        assert!(
            !diags
                .iter()
                .any(|d| d.code == "undeclared-external" && d.message.contains("`a`")),
            "the defined name's warning is dropped"
        );
        assert!(
            diags
                .iter()
                .any(|d| d.code == "undeclared-external" && d.message.contains("`b`")),
            "the undefined name's warning survives"
        );
        assert!(
            diags.iter().any(|d| d.code == "unused-import"),
            "unrelated diagnostics are untouched"
        );
    }

    /// `analyze`'s own contract, pinned by value rather than against a
    /// second implementation of it. The C1 front end it used to be
    /// compared against is gone; what survives is the part that was
    /// never a tautology — token provenance, which is a claim about
    /// `analyze`'s lex MODE, not about its parse: it lexes `WithComments`
    /// internally (feeding the green parse) yet publishes the
    /// `WithoutComments` stream on `AnalysisOutput.tokens`
    /// (`significant_tokens`, `docs/core.md (syntax trees)`).
    ///
    /// The AST/diagnostics/scopes halves are covered by this module's
    /// other tests and by `tests/compile_programs.rs`; re-asserting them
    /// against `flatten(parse(src))` would now compare `analyze` with
    /// itself.
    #[test]
    fn analyze_keeps_token_provenance_against_a_plain_lex() {
        let src = "// lead\nuse std::goToEnd as end;\nnamespace ns { export inner() { right; } }\n? documented\nexport main() {\n    helper() { left; }\n    007: @helper();\n    @ns::inner();\n    @end();\n    goto 007;\n}\n";
        let a = analyze(src).expect("analyzes");
        let plain = crate::lexer::lex(src).expect("lexes");
        assert_eq!(a.tokens, plain, "token provenance law");
        let with_comments = crate::lexer::lex_with(src, LexMode::WithComments).expect("lexes");
        assert!(
            with_comments.len() > a.tokens.len(),
            "fixture sanity: the source must carry a comment, or this proves nothing"
        );
    }

    /// A source that fails to lex still fails at the lex stage, with
    /// the same error — `analyze` now lexes `WithComments`, and the
    /// mode must not change which diagnostic a malformed program gets.
    #[test]
    fn analyze_reports_the_same_lex_error_as_before() {
        let src = "/* never closed\nmain() { right; }\n";
        assert_eq!(
            analyze(src).map(|_| ()).unwrap_err(),
            crate::lexer::lex(src).map(|_| ()).unwrap_err()
        );
    }
}
