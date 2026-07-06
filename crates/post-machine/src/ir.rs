//! Per-function CFG IR (spec §7, §7.1): a versioned, documented JSON
//! artifact, not an internal detail. Lowering makes every statement
//! successor an explicit block edge.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::compiler::{CompileError, CompileErrorKind, Warning};
use crate::parser::{Builtin, CheckArm, Item, Program, Successor};

pub const IR_VERSION: u32 = 3;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IrProgram {
    pub version: u32,
    pub functions: Vec<IrFunction>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IrFunction {
    pub name: String,
    /// Source line of the definition.
    pub line: u32,
    /// Entry is `blocks[0]`. Ids are unique within the function but need
    /// not stay dense once optimizer passes (Plan 6) delete blocks.
    pub blocks: Vec<IrBlock>,
    /// Hidden-by-default visibility (spec §3, §9): `true` unless the
    /// source marked the function `export` (`main` is always exported).
    pub local: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IrBlock {
    pub id: u32,
    /// Source labels naming this block (empty for synthetic blocks).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub labels: Vec<u32>,
    /// Source line of the block's first statement; 0 = synthetic.
    pub line: u32,
    pub ops: Vec<IrOp>,
    pub term: IrTerm,
    /// Source line of the statement that produced the terminator; 0 =
    /// synthetic (implicit return, shared exit block).
    pub term_line: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum IrOp {
    Lft { line: u32 },
    Rgt { line: u32 },
    Wr { index: u32, line: u32 },
    Brk { line: u32 },
    Call { name: String, line: u32 },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum IrTerm {
    FallThrough {
        to: u32,
    },
    Goto {
        to: u32,
    },
    Check {
        marked: u32,
        blank: u32,
    },
    Return,
    Halt,
    /// Optimizer-produced (spec §8 pass 8): jump to the callee's `ent`
    /// instead of `call` + `ret`. Never emitted by lowering.
    TailCall {
        name: String,
    },
}

impl IrProgram {
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).expect("IR serializes")
    }

    pub fn from_json(s: &str) -> Result<IrProgram, String> {
        serde_json::from_str(s).map_err(|e| e.to_string())
    }
}

impl IrFunction {
    /// Mermaid flowchart of the CFG (`pmt ir graph`). Node text: source
    /// labels, then ops, then a terminal marker for block-ending
    /// terminators; edges carry the check/goto semantics.
    pub fn to_mermaid(&self) -> String {
        use std::fmt::Write as _;
        let mut out = String::from("flowchart TD\n");
        for block in &self.blocks {
            let mut lines: Vec<String> = Vec::new();
            for &label in &block.labels {
                lines.push(format!("{label}:"));
            }
            for op in &block.ops {
                lines.push(match op {
                    IrOp::Lft { .. } => "lft".into(),
                    IrOp::Rgt { .. } => "rgt".into(),
                    IrOp::Wr { index, .. } => format!("wr {index}"),
                    IrOp::Brk { .. } => "brk".into(),
                    IrOp::Call { name, .. } => format!("call @{name}"),
                });
            }
            match &block.term {
                IrTerm::Return => lines.push("ret".into()),
                IrTerm::Halt => lines.push("hlt".into()),
                IrTerm::TailCall { name } => lines.push(format!("jmp @{name}")),
                IrTerm::FallThrough { .. } | IrTerm::Goto { .. } | IrTerm::Check { .. } => {}
            }
            if lines.is_empty() {
                lines.push("(empty)".into());
            }
            let _ = writeln!(out, "    B{}[\"{}\"]", block.id, lines.join("<br/>"));
        }
        for block in &self.blocks {
            match &block.term {
                IrTerm::FallThrough { to } => {
                    let _ = writeln!(out, "    B{} --> B{to}", block.id);
                }
                IrTerm::Goto { to } => {
                    let _ = writeln!(out, "    B{} -->|goto| B{to}", block.id);
                }
                IrTerm::Check { marked, blank } => {
                    let _ = writeln!(out, "    B{} -->|MF| B{marked}", block.id);
                    let _ = writeln!(out, "    B{} -->|!MF| B{blank}", block.id);
                }
                IrTerm::Return | IrTerm::Halt | IrTerm::TailCall { .. } => {}
            }
        }
        out
    }
}

/// Does this statement end its basic block?
fn terminates(stmt: &crate::parser::Statement) -> bool {
    match stmt.items.last().expect("parser: statements have items") {
        Item::Check { .. } | Item::Halt { .. } | Item::Goto { .. } => true,
        Item::Builtin { succ, .. } | Item::Call { succ, .. } => *succ != Successor::FallThrough,
        Item::Debugger { .. } => false,
    }
}

/// AST → CFG, plus the label-resolution half of semantic checking and
/// unreachable-code warnings.
pub fn lower(program: &Program) -> Result<(IrProgram, Vec<Warning>), CompileError> {
    let mut functions = Vec::with_capacity(program.functions.len());
    let mut warnings = Vec::new();
    for f in &program.functions {
        functions.push(lower_function(f, &mut warnings)?);
    }
    Ok((
        IrProgram {
            version: IR_VERSION,
            functions,
        },
        warnings,
    ))
}

fn lower_function(
    f: &crate::parser::Function,
    warnings: &mut Vec<Warning>,
) -> Result<IrFunction, CompileError> {
    if f.body.is_empty() {
        // `f() {}` — a single empty block: ent; ret.
        return Ok(IrFunction {
            name: f.name.clone(),
            line: f.line,
            blocks: vec![IrBlock {
                id: 0,
                labels: vec![],
                line: 0,
                ops: vec![],
                term: IrTerm::Return,
                term_line: 0,
            }],
            local: f.local,
        });
    }

    // Pass A: block boundaries. A statement starts a new block when it is
    // labeled or its predecessor terminated one.
    let mut starts = vec![false; f.body.len()];
    for (i, stmt) in f.body.iter().enumerate() {
        starts[i] = i == 0 || !stmt.labels.is_empty() || terminates(&f.body[i - 1]);
    }
    let mut block_of_stmt = vec![0u32; f.body.len()];
    let mut n_blocks = 0u32;
    for (i, &s) in starts.iter().enumerate() {
        if s {
            n_blocks += 1;
        }
        block_of_stmt[i] = n_blocks - 1;
    }

    // The shared synthetic return block: target of `!` check arms.
    let exit_id = n_blocks;
    let mut exit_used = false;

    let mut label_block: HashMap<u32, u32> = HashMap::new();
    for (i, stmt) in f.body.iter().enumerate() {
        for &l in &stmt.labels {
            label_block.insert(l, block_of_stmt[i]);
        }
    }
    let resolve = |label: u32, line: u32| -> Result<u32, CompileError> {
        label_block.get(&label).copied().ok_or(CompileError {
            line,
            col: 0,
            kind: CompileErrorKind::UndefinedLabel(label),
        })
    };

    enum Close {
        None,
        Term(IrTerm),
    }

    let mut blocks: Vec<IrBlock> = Vec::new();
    let mut current: Option<IrBlock> = None;

    for (i, stmt) in f.body.iter().enumerate() {
        if starts[i] {
            debug_assert!(current.is_none(), "predecessor closed the block");
            current = Some(IrBlock {
                id: block_of_stmt[i],
                labels: stmt.labels.clone(),
                line: stmt.line,
                ops: vec![],
                term: IrTerm::Return, // placeholder, always overwritten
                term_line: 0,
            });
        }
        let block = current.as_mut().expect("a block is always open here");

        for item in &stmt.items {
            match item {
                Item::Builtin { which, line, .. } => block.ops.push(match which {
                    Builtin::Left => IrOp::Lft { line: *line },
                    Builtin::Right => IrOp::Rgt { line: *line },
                    Builtin::Mark => IrOp::Wr {
                        index: 1,
                        line: *line,
                    },
                    Builtin::Unmark => IrOp::Wr {
                        index: 0,
                        line: *line,
                    },
                }),
                Item::Debugger { line } => block.ops.push(IrOp::Brk { line: *line }),
                Item::Call { name, line, .. } => block.ops.push(IrOp::Call {
                    name: name.clone(),
                    line: *line,
                }),
                Item::Check { .. } | Item::Halt { .. } | Item::Goto { .. } => {}
            }
        }

        let last = stmt.items.last().expect("parser: statements have items");
        let close = match last {
            Item::Goto { label, line } => Close::Term(IrTerm::Goto {
                to: resolve(*label, *line)?,
            }),
            Item::Halt { .. } => Close::Term(IrTerm::Halt),
            Item::Check {
                marked,
                blank,
                line,
            } => {
                let mut arm = |a: &CheckArm| -> Result<u32, CompileError> {
                    Ok(match a {
                        CheckArm::Label(l) => resolve(*l, *line)?,
                        CheckArm::Return => {
                            exit_used = true;
                            exit_id
                        }
                    })
                };
                Close::Term(IrTerm::Check {
                    marked: arm(marked)?,
                    blank: arm(blank)?,
                })
            }
            Item::Builtin { succ, line, .. } | Item::Call { succ, line, .. } => match succ {
                Successor::Label(l) => Close::Term(IrTerm::Goto {
                    to: resolve(*l, *line)?,
                }),
                Successor::Return => Close::Term(IrTerm::Return),
                Successor::FallThrough => Close::None,
            },
            Item::Debugger { .. } => Close::None,
        };

        let is_last_stmt = i + 1 == f.body.len();
        match close {
            Close::Term(term) => {
                let mut b = current.take().expect("block open");
                b.term = term;
                b.term_line = stmt.line;
                blocks.push(b);
            }
            Close::None => {
                if is_last_stmt {
                    // Falling off the end — implicit return (spec §3.2).
                    let mut b = current.take().expect("block open");
                    b.term = IrTerm::Return;
                    b.term_line = stmt.line;
                    blocks.push(b);
                } else if starts[i + 1] {
                    let mut b = current.take().expect("block open");
                    b.term = IrTerm::FallThrough {
                        to: block_of_stmt[i + 1],
                    };
                    b.term_line = stmt.line;
                    blocks.push(b);
                }
                // else: the same block continues into the next statement.
            }
        }
    }

    if exit_used {
        blocks.push(IrBlock {
            id: exit_id,
            labels: vec![],
            line: 0,
            ops: vec![],
            term: IrTerm::Return,
            term_line: 0,
        });
    }

    // Unreachable-code warnings: DFS over terminator edges from the entry.
    let index_of: HashMap<u32, usize> = blocks.iter().enumerate().map(|(i, b)| (b.id, i)).collect();
    let mut seen: HashSet<u32> = HashSet::new();
    let mut work = vec![blocks[0].id];
    while let Some(id) = work.pop() {
        if !seen.insert(id) {
            continue;
        }
        match &blocks[index_of[&id]].term {
            IrTerm::FallThrough { to } | IrTerm::Goto { to } => work.push(*to),
            IrTerm::Check { marked, blank } => {
                work.push(*marked);
                work.push(*blank);
            }
            IrTerm::Return | IrTerm::Halt | IrTerm::TailCall { .. } => {}
        }
    }
    for b in &blocks {
        if !seen.contains(&b.id) && b.line != 0 {
            warnings.push(Warning {
                line: b.line,
                message: format!("unreachable code in `{}`", f.name),
            });
        }
    }

    Ok(IrFunction {
        name: f.name.clone(),
        line: f.line,
        blocks,
        local: f.local,
    })
}

/// Structural invariants every optimizer pass must preserve (the Plan 5
/// final-review acceptance item): non-empty function, unique block ids,
/// every terminator target resolvable. `blocks[0]` remains the entry by
/// position; passes may delete or retarget but never leave a dangling
/// terminator.
pub fn validate_function(f: &IrFunction) -> Result<(), String> {
    if f.blocks.is_empty() {
        return Err(format!("{}: function has no blocks", f.name));
    }
    let mut ids = HashSet::new();
    for b in &f.blocks {
        if !ids.insert(b.id) {
            return Err(format!("{}: duplicate block id {}", f.name, b.id));
        }
    }
    for b in &f.blocks {
        let check = |t: u32| -> Result<(), String> {
            if ids.contains(&t) {
                Ok(())
            } else {
                Err(format!(
                    "{}: block {} terminator targets missing block {}",
                    f.name, b.id, t
                ))
            }
        };
        match &b.term {
            IrTerm::FallThrough { to } | IrTerm::Goto { to } => check(*to)?,
            IrTerm::Check { marked, blank } => {
                check(*marked)?;
                check(*blank)?;
            }
            IrTerm::Return | IrTerm::Halt | IrTerm::TailCall { .. } => {}
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::lex;
    use crate::parser::parse;

    fn ir_of(src: &str) -> (IrProgram, Vec<Warning>) {
        lower(&parse(&lex(src).unwrap()).unwrap()).unwrap()
    }

    #[test]
    fn lowers_go_to_end() {
        // 1: right; check(1,2); 2: left;  →  b0 {rgt | check b0,b1}, b1 {lft | ret}
        let (ir, warnings) = ir_of("goToEnd() { 1: right; check(1, 2); 2: left; }");
        assert!(warnings.is_empty());
        let f = &ir.functions[0];
        assert_eq!(f.blocks.len(), 2);
        assert_eq!(f.blocks[0].labels, vec![1]);
        assert_eq!(f.blocks[0].ops, vec![IrOp::Rgt { line: 1 }]);
        assert_eq!(
            f.blocks[0].term,
            IrTerm::Check {
                marked: 0,
                blank: 1
            }
        );
        assert_eq!(f.blocks[1].labels, vec![2]);
        assert_eq!(f.blocks[1].term, IrTerm::Return);
    }

    #[test]
    fn explicit_successors_become_gotos() {
        let (ir, _) = ir_of("goToBegin() { 1: left(2); 2: check(1, 3); 3: right(!); }");
        let f = &ir.functions[0];
        assert_eq!(f.blocks.len(), 3);
        assert_eq!(f.blocks[0].term, IrTerm::Goto { to: 1 });
        assert_eq!(
            f.blocks[1].term,
            IrTerm::Check {
                marked: 0,
                blank: 2
            }
        );
        assert_eq!(f.blocks[2].term, IrTerm::Return);
    }

    #[test]
    fn comma_groups_flatten_and_the_exit_block_is_shared() {
        let (ir, _) = ir_of("f() { 1: right, right, mark(5); 5: left, check(1, !); }");
        let f = &ir.functions[0];
        // b0: rgt rgt wr1 | goto b1; b1: lft | check(b0, exit); exit: ret
        assert_eq!(f.blocks.len(), 3);
        assert_eq!(
            f.blocks[0].ops,
            vec![
                IrOp::Rgt { line: 1 },
                IrOp::Rgt { line: 1 },
                IrOp::Wr { index: 1, line: 1 }
            ]
        );
        assert_eq!(f.blocks[0].term, IrTerm::Goto { to: 1 });
        assert_eq!(
            f.blocks[1].term,
            IrTerm::Check {
                marked: 0,
                blank: 2
            }
        );
        assert_eq!(f.blocks[2].id, 2);
        assert!(f.blocks[2].labels.is_empty());
        assert_eq!(f.blocks[2].line, 0); // synthetic
        assert_eq!(f.blocks[2].term, IrTerm::Return);
    }

    #[test]
    fn unlabeled_statements_merge_and_the_end_returns_implicitly() {
        let (ir, _) = ir_of("main() { @goToEnd(); right; check(3, 4); 3: unmark(!); 4: mark; }");
        let f = &ir.functions[0];
        assert_eq!(f.blocks.len(), 3);
        assert_eq!(
            f.blocks[0].ops,
            vec![
                IrOp::Call {
                    name: "goToEnd".into(),
                    line: 1
                },
                IrOp::Rgt { line: 1 }
            ]
        );
        assert_eq!(
            f.blocks[0].term,
            IrTerm::Check {
                marked: 1,
                blank: 2
            }
        );
        assert_eq!(f.blocks[1].ops, vec![IrOp::Wr { index: 0, line: 1 }]);
        assert_eq!(f.blocks[1].term, IrTerm::Return); // unmark(!)
        assert_eq!(f.blocks[2].term, IrTerm::Return); // implicit
    }

    #[test]
    fn halt_is_a_terminator_and_debugger_an_op() {
        let (ir, _) = ir_of("f() { debugger; halt; }");
        let f = &ir.functions[0];
        assert_eq!(f.blocks.len(), 1);
        assert_eq!(f.blocks[0].ops, vec![IrOp::Brk { line: 1 }]);
        assert_eq!(f.blocks[0].term, IrTerm::Halt);
    }

    #[test]
    fn empty_function_is_one_returning_block() {
        let (ir, _) = ir_of("f() { }");
        assert_eq!(ir.functions[0].blocks.len(), 1);
        assert_eq!(ir.functions[0].blocks[0].term, IrTerm::Return);
    }

    #[test]
    fn undefined_labels_error_wherever_they_are_referenced() {
        let e = lower(&parse(&lex("f() { goto 9; }").unwrap()).unwrap()).unwrap_err();
        assert!(matches!(e.kind, CompileErrorKind::UndefinedLabel(9)));
        let e = lower(&parse(&lex("f() { left(7); }").unwrap()).unwrap()).unwrap_err();
        assert!(matches!(e.kind, CompileErrorKind::UndefinedLabel(7)));
        let e = lower(&parse(&lex("f() { check(1, 2); 1: mark; }").unwrap()).unwrap()).unwrap_err();
        assert!(matches!(e.kind, CompileErrorKind::UndefinedLabel(2)));
    }

    #[test]
    fn unreachable_code_warns_with_its_line() {
        let (ir, warnings) = ir_of("f() {\n    goto 1;\n    right;\n1:  left;\n}");
        assert_eq!(ir.functions[0].blocks.len(), 3);
        assert_eq!(warnings.len(), 1);
        assert_eq!(warnings[0].line, 3);
        assert!(warnings[0].message.contains("unreachable"));
    }

    #[test]
    fn json_round_trips_with_a_version() {
        let (ir, _) = ir_of("main() { @go(); check(1, !); 1: mark(!); }");
        let json = ir.to_json();
        assert_eq!(IrProgram::from_json(&json).unwrap(), ir);
        assert!(json.contains("\"version\": 3"));
    }

    #[test]
    fn tail_call_serializes_with_its_own_tag() {
        let term = IrTerm::TailCall { name: "f".into() };
        let json = serde_json::to_string(&term).unwrap();
        assert!(json.contains("\"kind\":\"tail_call\""), "{json}");
        assert_eq!(serde_json::from_str::<IrTerm>(&json).unwrap(), term);
    }

    #[test]
    fn validate_function_accepts_lowered_ir_and_rejects_dangling_targets() {
        let (ir, _) = ir_of("f() { 1: right; check(1, !); }");
        for f in &ir.functions {
            validate_function(f).unwrap();
        }
        let mut broken = ir.functions[0].clone();
        broken.blocks[0].term = IrTerm::Goto { to: 99 };
        assert!(validate_function(&broken).is_err());
    }

    #[test]
    fn validate_function_rejects_empty_functions() {
        let (ir, _) = ir_of("f() { left; }");
        let mut broken = ir.functions[0].clone();
        broken.blocks.clear();
        assert!(validate_function(&broken).is_err());
    }

    #[test]
    fn validate_function_rejects_duplicate_ids() {
        let (ir, _) = ir_of("f() { left; }");
        let mut broken = ir.functions[0].clone();
        let dup = broken.blocks[0].clone();
        broken.blocks.push(dup);
        assert!(validate_function(&broken).is_err());
    }

    #[test]
    fn to_mermaid_renders_flowchart_with_check_edges() {
        let (ir, _) = ir_of("main() { 1: right; check(1, !); }");
        let mermaid = ir.functions[0].to_mermaid();
        assert!(mermaid.starts_with("flowchart TD\n"), "{mermaid}");
        assert!(mermaid.contains("B0[\"1:<br/>rgt\"]"), "{mermaid}");
        assert!(mermaid.contains("-->|MF|"), "{mermaid}");
        assert!(mermaid.contains("-->|!MF|"), "{mermaid}");
        assert!(
            mermaid.lines().any(|l| l.trim_end().ends_with("ret\"]")),
            "{mermaid}"
        );
    }
}
