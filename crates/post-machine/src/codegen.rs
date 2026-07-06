//! CFG → `.pma` text. The generated text is fed to the core assembler
//! (the compile → assemble pipeline), which supplies encoding,
//! intra-function jump relaxation, and the `ent` prologue via `.func` —
//! codegen never touches bytes.
//!
//! Layout invariant (docs/language.md (optimization), active even at
//! `-O0`): an unconditional transfer to the physically next instruction
//! is never emitted — blocks are laid out in order and fall-through is
//! selected instead.

use std::collections::{HashMap, HashSet};

use mtc_core::asm::grid_line as grid;

use crate::ir::{IrBlock, IrFunction, IrOp, IrProgram, IrTerm};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CodegenOptions {
    /// Drop `brk` ops (`--strip-debugger`).
    pub strip_debugger: bool,
}

/// Generated assembly plus the pma→pmc line correspondence that lets the
/// driver remap assembler debug lines back to `.pmc` sources.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PmaOutput {
    pub text: String,
    /// `(pma_line, pmc_line)`, 1-based on both sides, for every generated
    /// line that carries an instruction or `.func` directive.
    pub line_map: Vec<(u32, u32)>,
}

struct Emitter {
    lines: Vec<String>,
    line_map: Vec<(u32, u32)>,
}

impl Emitter {
    /// `pmc_line == 0` → no source correspondence (label lines, synthetic
    /// return blocks).
    fn push(&mut self, text: String, pmc_line: u32) {
        self.lines.push(text);
        if pmc_line != 0 {
            self.line_map.push((self.lines.len() as u32, pmc_line));
        }
    }
}

pub fn emit_program(ir: &IrProgram, options: CodegenOptions) -> PmaOutput {
    let mut e = Emitter {
        lines: Vec::new(),
        line_map: Vec::new(),
    };
    for f in &ir.functions {
        emit_function(f, options, &mut e);
    }
    let mut text = e.lines.join("\n");
    text.push('\n');
    PmaOutput {
        text,
        line_map: e.line_map,
    }
}

/// Canonical `.pma` name for a block: its first source label (`L5`), or
/// the block id (`B3`) for synthetic blocks. The prefixes cannot collide.
fn block_name(b: &IrBlock) -> String {
    match b.labels.first() {
        Some(l) => format!("L{l}"),
        None => format!("B{}", b.id),
    }
}

fn emit_function(f: &IrFunction, options: CodegenOptions, e: &mut Emitter) {
    let name_of: HashMap<u32, String> = f.blocks.iter().map(|b| (b.id, block_name(b))).collect();
    let next_id = |i: usize| f.blocks.get(i + 1).map(|b| b.id);

    // Pass 1: which blocks need a label line — exactly those that some
    // emitted jump will reference (fall-through references nothing).
    let mut referenced: HashSet<u32> = HashSet::new();
    for (i, b) in f.blocks.iter().enumerate() {
        let next = next_id(i);
        match &b.term {
            IrTerm::FallThrough { to } | IrTerm::Goto { to } => {
                if next != Some(*to) {
                    referenced.insert(*to);
                }
            }
            IrTerm::Check { marked, blank } => {
                if next == Some(*blank) {
                    referenced.insert(*marked);
                } else if next == Some(*marked) {
                    referenced.insert(*blank);
                } else {
                    referenced.insert(*marked);
                    referenced.insert(*blank);
                }
            }
            IrTerm::Return | IrTerm::Halt | IrTerm::TailCall { .. } => {}
        }
    }

    e.push(
        format!(".func {}{}", f.name, if f.local { " local" } else { "" }),
        f.line,
    );
    for (i, b) in f.blocks.iter().enumerate() {
        if referenced.contains(&b.id) {
            if b.labels.is_empty() {
                e.push(format!("B{}:", b.id), 0);
            } else {
                // Every source label names the block; jumps use the first.
                for l in &b.labels {
                    e.push(format!("L{l}:"), 0);
                }
            }
        }

        for op in &b.ops {
            match op {
                IrOp::Lft { line } => e.push(grid(None, "lft", ""), *line),
                IrOp::Rgt { line } => e.push(grid(None, "rgt", ""), *line),
                IrOp::Wr { index, line } => e.push(grid(None, "wr", &index.to_string()), *line),
                IrOp::Brk { line } => {
                    if !options.strip_debugger {
                        e.push(grid(None, "brk", ""), *line);
                    }
                }
                IrOp::Call { name, line } => e.push(grid(None, "call", name), *line),
            }
        }

        let next = next_id(i);
        let target = |id: u32| {
            name_of
                .get(&id)
                .expect("terminator targets an existing block")
                .clone()
        };
        match &b.term {
            IrTerm::FallThrough { to } | IrTerm::Goto { to } => {
                if next != Some(*to) {
                    e.push(grid(None, "jmp", &target(*to)), b.term_line);
                }
            }
            IrTerm::Check { marked, blank } => {
                if next == Some(*blank) {
                    e.push(grid(None, "jm", &target(*marked)), b.term_line);
                } else if next == Some(*marked) {
                    e.push(grid(None, "jnm", &target(*blank)), b.term_line);
                } else {
                    e.push(grid(None, "jm", &target(*marked)), b.term_line);
                    e.push(grid(None, "jmp", &target(*blank)), b.term_line);
                }
            }
            IrTerm::Return => {
                // Returning from main stops the machine (docs/language.md).
                let mnemonic = if f.name == "main" { "stp" } else { "ret" };
                e.push(grid(None, mnemonic, ""), b.term_line);
            }
            IrTerm::Halt => e.push(grid(None, "hlt", ""), b.term_line),
            IrTerm::TailCall { name } => {
                e.push(grid(None, "jmp", &format!("@{name}")), b.term_line)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn emit_src(src: &str, strip: bool) -> PmaOutput {
        let program = crate::parser::parse(&crate::lexer::lex(src).unwrap()).unwrap();
        let (ir, _) = crate::ir::lower(&program).unwrap();
        emit_program(
            &ir,
            CodegenOptions {
                strip_debugger: strip,
            },
        )
    }

    #[test]
    fn go_to_end_emits_the_plan3_shape() {
        let out = emit_src("goToEnd() { 1: right; check(1, 2); 2: left; }", false);
        assert_eq!(
            out.text,
            "\
.func goToEnd
L1:
        rgt
        jm      L1
        lft
        ret
"
        );
    }

    #[test]
    fn goto_to_next_vanishes_and_unreferenced_labels_drop() {
        let out = emit_src(
            "goToBegin() { 1: left(2); 2: check(1, 3); 3: right(!); }",
            false,
        );
        assert_eq!(
            out.text,
            "\
.func goToBegin
L1:
        lft
        jm      L1
        rgt
        ret
"
        );
    }

    #[test]
    fn check_with_neither_arm_adjacent_emits_branch_plus_jump() {
        let out = emit_src(
            "f() { 1: check(2, 3); mark; 2: left(!); 3: right(!); }",
            false,
        );
        assert_eq!(
            out.text,
            "\
.func f
        jm      L2
        jmp     L3
        wr      1
L2:
        lft
        ret
L3:
        rgt
        ret
"
        );
    }

    #[test]
    fn main_returns_as_stp_and_the_synthetic_exit_gets_a_b_label() {
        let out = emit_src("main() { 1: check(!, 2); mark; 2: left; }", false);
        assert_eq!(
            out.text,
            "\
.func main
        jm      B3
        jmp     L2
        wr      1
L2:
        lft
        stp
B3:
        stp
"
        );
    }

    #[test]
    fn strip_debugger_drops_brk() {
        let kept = emit_src("f() { debugger; left; }", false);
        assert!(kept.text.contains("brk"));
        let stripped = emit_src("f() { debugger; left; }", true);
        assert!(!stripped.text.contains("brk"));
    }

    #[test]
    fn calls_and_halt_emit() {
        let out = emit_src("f() { @helper(); halt; }", false);
        assert_eq!(out.text, ".func f\n        call    helper\n        hlt\n");
    }

    #[test]
    fn line_map_points_instructions_at_their_pmc_lines() {
        let out = emit_src("f() {\n    left;\n    right(!);\n}", false);
        assert_eq!(out.text, ".func f\n        lft\n        rgt\n        ret\n");
        // .func ← line 1, lft ← 2, rgt ← 3, ret ← 3 (the `(!)` successor).
        assert_eq!(out.line_map, vec![(1, 1), (2, 2), (3, 3), (4, 3)]);
    }
}
