//! `.pmc` compiler driver and shared diagnostics (spec §7).
//!
//! Every pipeline stage (lexer → parser → lowering → codegen) reports
//! fatals through [`CompileError`]; non-fatal findings accumulate as
//! [`Warning`]s — library code never prints (spec §10).

use mtc_core::formats::object::ObjectFile;

use crate::codegen::{CodegenOptions, emit_program};
use crate::ir::IrProgram;
use crate::optimizer::{OptLevel, OptOptions, OptReport, optimize};

/// 1-based `line`; 1-based `col` counted in characters, or 0 when the
/// error is attributed to a whole line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompileError {
    pub line: u32,
    pub col: u32,
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
    /// A reserved word used as a function name.
    ReservedFunctionName(String),
    /// A bare identifier statement that is not a builtin (spec §3.3).
    UnknownCommand(String),
    /// `@` applied to a builtin name (`@left()`).
    BuiltinCalled(String),
    DuplicateFunction(String),
    DuplicateLabel(u32),
    /// `goto`/`check`/successor names a label the function never declares.
    UndefinedLabel(u32),
    /// `goto !` — spec §3.2: put `(!)` on the preceding command instead.
    GotoReturn,
    /// A comma-group position rule violated (spec §3.2, last table row).
    GroupPosition(&'static str),
    /// A label at the end of a function body binds to nothing.
    DanglingLabel(u32),
    /// The generated `.pma` failed to assemble — a compiler bug, not a
    /// user error; the message carries the assembler diagnostic.
    Internal(String),
    /// `export` on a nested definition — nesting is always local.
    NestedExport,
}

impl std::fmt::Display for CompileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.col > 0 {
            write!(f, "line {}:{}: ", self.line, self.col)?;
        } else {
            write!(f, "line {}: ", self.line)?;
        }
        match &self.kind {
            CompileErrorKind::Lex(m) => write!(f, "{m}"),
            CompileErrorKind::Expected { what, found } => {
                write!(f, "expected {what}, found {found}")
            }
            CompileErrorKind::ReservedFunctionName(n) => {
                write!(f, "`{n}` is a reserved word and cannot name a function")
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
            CompileErrorKind::DuplicateFunction(n) => write!(f, "duplicate function `{n}`"),
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
        }
    }
}

impl std::error::Error for CompileError {}

/// A non-fatal finding, reported (never printed) via `CompileReport`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Warning {
    pub line: u32,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CompileOptions {
    /// `-g`: record label/line debug info in the object, with lines
    /// remapped to `.pmc` sources.
    pub debug_info: bool,
    /// `--strip-debugger`: drop `brk` at codegen (spec §10). The
    /// optimizer runs BEFORE stripping, so `brk` barriers always hold.
    pub strip_debugger: bool,
    /// `-O0` (default) or `-O1` (spec §8 passes, 6a subset).
    pub opt_level: OptLevel,
    /// Pass names to disable (`--fno-<pass>`), e.g. `"cell-state"`.
    pub disabled_passes: Vec<String>,
    /// Capture per-stage IR snapshots (`--emit-ir=<stage>` backing):
    /// `"lowered"`, `"after:<pass>"` per changing pass, `"final"`.
    pub capture_ir: bool,
}

/// Structured stage report — `pmt -v` renders it; the library never
/// prints (spec §10, the LinkReport pattern).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompileReport {
    pub warnings: Vec<Warning>,
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

/// `.pmc` source → object file (spec §7): lex → parse → lower → emit
/// `.pma` → assemble. Assembly failure of GENERATED text is a compiler
/// bug and reports as `CompileErrorKind::Internal`.
pub fn compile(source: &str, options: CompileOptions) -> Result<CompileOutput, CompileError> {
    let tokens = crate::lexer::lex(source)?;
    let program = flatten(crate::parser::parse(&tokens)?);
    let (mut ir, mut warnings) = crate::ir::lower(&program)?;
    warnings.extend(visibility_warnings(&program));
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
            line: 0,
            col: 0,
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
        report: CompileReport { warnings, opt },
    })
}

/// Flatten nested definitions (spec §3): dot-mangle names
/// (`outer.inner`), resolve calls lexically (innermost scope outward,
/// then top level, else external), compute symbol locality. Infallible:
/// unresolved names simply stay external.
fn flatten(program: crate::parser::Program) -> crate::parser::Program {
    use crate::parser::{Function, Item, Program};
    use std::collections::HashMap;

    let top: HashMap<String, String> = program
        .functions
        .iter()
        .map(|f| (f.name.clone(), f.name.clone()))
        .collect();

    fn emit(
        mut f: Function,
        prefix: &str,
        scopes: &[HashMap<String, String>],
        out: &mut Vec<Function>,
    ) {
        let full = if prefix.is_empty() {
            f.name.clone()
        } else {
            format!("{prefix}.{}", f.name)
        };
        // This function's own children are visible inside its body.
        let child_map: HashMap<String, String> = f
            .nested
            .iter()
            .map(|c| (c.name.clone(), format!("{full}.{}", c.name)))
            .collect();
        let mut inner = scopes.to_vec();
        inner.push(child_map);

        for stmt in &mut f.body {
            for item in &mut stmt.items {
                if let Item::Call { name, .. } = item {
                    for scope in inner.iter().rev() {
                        if let Some(m) = scope.get(name) {
                            *name = m.clone();
                            break;
                        }
                    }
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
            emit(c, &full, &inner, out);
        }
    }

    let mut out = Vec::new();
    let imports = program.imports.clone();
    for f in program.functions {
        emit(f, "", std::slice::from_ref(&top), &mut out);
    }
    Program {
        functions: out,
        imports,
    }
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

/// Import & liveness warnings (spec §3.3 as amended): undeclared
/// externals (once per name), unused imports, and unused functions —
/// reachability from `main` + exports; sound because unexported
/// functions are invisible outside this module.
fn visibility_warnings(program: &crate::parser::Program) -> Vec<Warning> {
    use crate::parser::Item;
    use std::collections::{HashMap, HashSet, VecDeque};

    let defined: HashSet<&str> = program.functions.iter().map(|f| f.name.as_str()).collect();
    let imported: HashSet<&str> = program.imports.iter().map(|i| i.name.as_str()).collect();

    let mut warnings = Vec::new();
    let mut external_called: HashSet<&str> = HashSet::new();
    let mut warned_undeclared: HashSet<&str> = HashSet::new();
    let mut edges: HashMap<&str, Vec<&str>> = HashMap::new();

    for f in &program.functions {
        let mut callees = Vec::new();
        for stmt in &f.body {
            for item in &stmt.items {
                if let Item::Call { name, line, .. } = item {
                    if defined.contains(name.as_str()) {
                        callees.push(name.as_str());
                    } else {
                        external_called.insert(name.as_str());
                        if !imported.contains(name.as_str())
                            && warned_undeclared.insert(name.as_str())
                        {
                            warnings.push(Warning {
                                line: *line,
                                message: format!(
                                    "call to undeclared external `{name}` — declare it with `use {name};`"
                                ),
                            });
                        }
                    }
                }
            }
        }
        edges.insert(f.name.as_str(), callees);
    }

    for import in &program.imports {
        if !external_called.contains(import.name.as_str()) {
            warnings.push(Warning {
                line: import.line,
                message: format!("unused import `{}`", import.name),
            });
        }
    }

    // Unused functions: reachability from main + exports.
    let mut reached: HashSet<&str> = HashSet::new();
    let mut queue: VecDeque<&str> = program
        .functions
        .iter()
        .filter(|f| f.exported || f.name == "main")
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
    for f in &program.functions {
        if !reached.contains(f.name.as_str()) {
            warnings.push(Warning {
                line: f.line,
                message: format!("unused function `{}` (not exported, never called)", f.name),
            });
        }
    }
    warnings
}

#[cfg(test)]
mod tests {
    use super::*;
    use mtc_core::formats::object::SymbolDef;

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
        assert!(out.report.warnings.is_empty());
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
        assert_eq!(out.report.warnings.len(), 1);
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
            .warnings
            .iter()
            .filter(|w| w.message.contains("undeclared"))
            .count();
        assert_eq!(n, 1);
        let out = compile("use go; main() { @go(); }", CompileOptions::default()).unwrap();
        assert!(
            out.report
                .warnings
                .iter()
                .all(|w| !w.message.contains("undeclared"))
        );
    }

    #[test]
    fn unused_imports_and_unused_functions_warn() {
        let out = compile("use ghost; main() { mark; }", CompileOptions::default()).unwrap();
        assert!(
            out.report
                .warnings
                .iter()
                .any(|w| w.message.contains("unused import `ghost`"))
        );

        let out = compile(
            "dead() { left; } main() { mark; }",
            CompileOptions::default(),
        )
        .unwrap();
        assert!(
            out.report
                .warnings
                .iter()
                .any(|w| w.message.contains("unused function `dead`"))
        );

        // Transitively dead: a called only by dead — both warn.
        let out = compile(
            "a() { left; } dead() { @a(); } main() { mark; }",
            CompileOptions::default(),
        )
        .unwrap();
        let n = out
            .report
            .warnings
            .iter()
            .filter(|w| w.message.contains("unused function"))
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
                .warnings
                .iter()
                .all(|w| !w.message.contains("unused function"))
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
                .warnings
                .iter()
                .any(|w| w.message.contains("unused function `api.helper`")),
            "{:?}",
            out.report.warnings
        );
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
            .warnings
            .iter()
            .filter(|w| w.message.contains("undeclared"))
            .count();
        assert_eq!(n, 1);
    }
}
