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
    let program = crate::parser::parse(&tokens)?;
    let (mut ir, warnings) = crate::ir::lower(&program)?;
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
    fn compiles_to_an_object_with_symbols_and_relocations() {
        let out = compile("main() { @goToEnd(); mark; }", CompileOptions::default()).unwrap();
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
        let out = compile("f() { goto 1; right; 1: left; }", CompileOptions::default()).unwrap();
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
}
