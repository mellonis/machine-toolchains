//! `.pmc` compiler driver and shared diagnostics.
//!
//! Every pipeline stage (lexer → parser → lowering → codegen) reports
//! fatals through [`CompileError`]; non-fatal findings accumulate as
//! span-carrying, coded [`Diagnostic`]s — library code never prints
//! (docs/cli.md).

use std::collections::HashMap;

use mtc_core::diagnostics::{Diagnostic, Span};
use mtc_core::formats::object::ObjectFile;

use crate::codegen::{CodegenOptions, emit_program};
use crate::ir::IrProgram;
use crate::lexer::Token;
use crate::optimizer::{OptLevel, OptOptions, OptReport, optimize};
use crate::parser::Program;

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
    /// A bare identifier statement that is not a builtin (docs/language.md).
    UnknownCommand(String),
    /// `@` applied to a builtin name (`@left()`).
    BuiltinCalled(String),
    /// Empty `()` on a tape builtin (`left()`), docs/language.md: parens
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
    /// `goto !` — docs/language.md: put `(!)` on the preceding command instead.
    GotoReturn,
    /// A comma-group position rule violated (docs/language.md, the
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
}

impl std::fmt::Display for CompileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "line {}:{}: {}",
            self.span.start.line, self.span.start.col, self.kind
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
        }
    }
}

impl std::error::Error for CompileError {}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CompileOptions {
    /// `-g`: record label/line debug info in the object, with lines
    /// remapped to `.pmc` sources.
    pub debug_info: bool,
    /// `--strip-debugger`: drop `brk` at codegen (docs/cli.md). The
    /// optimizer runs BEFORE stripping, so `brk` barriers always hold.
    pub strip_debugger: bool,
    /// `-O0` (default) or `-O1` (docs/language.md (optimization)).
    pub opt_level: OptLevel,
    /// Pass names to disable (`--fno-<pass>`), e.g. `"cell-state"`.
    pub disabled_passes: Vec<String>,
    /// Capture per-stage IR snapshots (`--emit-ir=<stage>` backing):
    /// `"lowered"`, `"after:<pass>"` per changing pass, `"final"`.
    pub capture_ir: bool,
}

/// Structured stage report — `pmt -v` renders it; the library never
/// prints (docs/cli.md, the same pattern as the linker's `LinkReport`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompileReport {
    pub diagnostics: Vec<Diagnostic>,
    pub opt: OptReport,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompileOutput {
    pub object: ObjectFile,
    /// The generated assembly (`-S` output). The object is assembled from
    /// exactly this text, so the code bytes can never disagree; under `-g`
    /// the object's debug LINES are additionally remapped to `.pmc`
    /// sources, so `object != assemble(pma, true)` on that side table.
    pub pma: String,
    /// The FINAL CFG (post-optimizer at -O1; the lowered CFG at -O0).
    pub ir: IrProgram,
    /// Per-stage IR snapshots when `capture_ir` was set; empty otherwise.
    pub ir_snapshots: Vec<(String, IrProgram)>,
    pub report: CompileReport,
}

/// flatten's per-scope name maps, retained for scope-aware lint rules
/// instead of being discarded.
pub(crate) struct ScopeSummary {
    /// ns path -> (bare name -> full mangled name)
    pub defs: HashMap<Vec<String>, HashMap<String, String>>,
    /// ns path -> (bare name -> (import index, full `::` path))
    pub bindings: HashMap<Vec<String>, HashMap<String, (usize, String)>>,
}

/// The codegen-free front half of the pipeline: everything the lint
/// layer (and a future LSP) needs, nothing it doesn't.
pub(crate) struct AnalysisOutput {
    pub tokens: Vec<Token>,
    pub ast: Program,
    pub ir: IrProgram,
    pub diagnostics: Vec<Diagnostic>,
    pub scopes: ScopeSummary,
}

/// lex → parse → duplicate-binding check → flatten → lower. Stops before
/// the optimizer; `compile()` composes this with the back half.
pub(crate) fn analyze(source: &str) -> Result<AnalysisOutput, CompileError> {
    let tokens = crate::lexer::lex(source)?;
    let parsed = crate::parser::parse(&tokens)?;
    check_duplicate_bindings(&parsed)?;
    let (program, scopes, vis) = flatten(parsed);
    let (ir, mut diagnostics) = crate::ir::lower(&program)?;
    diagnostics.extend(vis);
    Ok(AnalysisOutput {
        tokens,
        ast: program,
        ir,
        diagnostics,
        scopes,
    })
}

/// `.pmc` source → object file: lex → parse → lower → emit `.pma` →
/// assemble. Assembly failure of GENERATED text is a compiler bug and
/// reports as `CompileErrorKind::Internal`.
pub fn compile(source: &str, options: CompileOptions) -> Result<CompileOutput, CompileError> {
    let analysis = analyze(source)?;
    let AnalysisOutput {
        mut ir,
        diagnostics,
        ..
    } = analysis;
    let mut ir_snapshots = Vec::new();
    if options.capture_ir {
        ir_snapshots.push(("lowered".to_string(), ir.clone()));
    }
    let opt = optimize(
        &mut ir,
        &OptOptions {
            level: options.opt_level,
            disabled: options.disabled_passes.iter().cloned().collect(),
            capture: options.capture_ir,
        },
        &mut ir_snapshots,
    );
    if options.capture_ir {
        ir_snapshots.push(("final".to_string(), ir.clone()));
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
    Ok(CompileOutput {
        object,
        pma: pma.text,
        ir,
        ir_snapshots,
        report: CompileReport { diagnostics, opt },
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
/// symbol self-decomposes at the last `::`.
fn full_name(ns: &[String], name: &str) -> String {
    if ns.is_empty() {
        name.to_string()
    } else {
        format!("{}::{}", ns.join("::"), name)
    }
}

/// Flatten definitions and resolve calls (docs/language.md (visibility)):
/// mangle names (`::` for namespaces, `.` for nesting), resolve
/// each call innermost-outward — the function's own nested maps (defs
/// only), then per enclosing namespace prefix (longest first) that
/// level's definitions THEN its import bindings — and compute symbol
/// locality. Call names containing `::` are ABSOLUTE: they skip the
/// scope chain and imports, stay verbatim, and are self-declaring (no
/// undeclared warning). Infallible — unresolved bare names simply stay
/// external; all visibility warnings (undeclared externals, unused
/// imports, unused functions) are produced here.
fn flatten(
    program: crate::parser::Program,
) -> (crate::parser::Program, ScopeSummary, Vec<Diagnostic>) {
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

    struct Ctx {
        defs: HashMap<Vec<String>, HashMap<String, String>>,
        bindings: HashMap<Vec<String>, HashMap<String, (usize, String)>>,
        imports_used: Vec<bool>,
        warned_undeclared: HashSet<String>,
        warnings: Vec<Diagnostic>,
    }

    let mut ctx = Ctx {
        defs,
        bindings,
        imports_used: vec![false; imports.len()],
        warned_undeclared: HashSet::new(),
        warnings: Vec::new(),
    };

    /// Resolve one call name in place, innermost scope outward.
    fn resolve(
        name: &mut String,
        span: mtc_core::diagnostics::Span,
        nested: &[HashMap<String, String>],
        ns: &[String],
        ctx: &mut Ctx,
    ) {
        // Absolute: leave verbatim, no warning, no import consumption
        // (self-declaring; resolves internally iff this module defines
        // that symbol, else stays external).
        if name.contains("::") {
            return;
        }
        for scope in nested.iter().rev() {
            if let Some(m) = scope.get(name.as_str()) {
                *name = m.clone();
                return;
            }
        }
        // Enclosing namespace levels, innermost outward; each level's
        // definitions outrank its import bindings.
        for k in (0..=ns.len()).rev() {
            let prefix = &ns[..k];
            if let Some(m) = ctx.defs.get(prefix).and_then(|d| d.get(name.as_str())) {
                *name = m.clone();
                return;
            }
            if let Some(&(idx, ref path)) =
                ctx.bindings.get(prefix).and_then(|b| b.get(name.as_str()))
            {
                ctx.imports_used[idx] = true;
                *name = path.clone();
                return;
            }
        }
        // Total miss: the call stays a bare external; warn once per name.
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

    (
        Program {
            functions: out,
            imports,
        },
        ScopeSummary { defs, bindings },
        warnings,
    )
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
    use super::*;
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
        assert_eq!(format!("{e}"), "line 1:7: undefined label `3`");
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
        assert_eq!(out.object, direct);
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
}
