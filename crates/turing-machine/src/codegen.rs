//! TM IR → `.tma` text — the spec's `-O0` canonical lowering (§11.1). The
//! generated text is fed to the core assembler (the compile → assemble
//! pipeline), which supplies encoding, table-section layout, intra-function
//! jump relaxation, and the `ent` prologue via `.func` — codegen never
//! touches bytes and never emits `ent` (the `.func` directive prepends it,
//! exactly as PM-1 codegen relies on).
//!
//! The canon, per world:
//!
//! * a `.routine <name>, tapes=N, alpha=(c1,…,cN)` signature then a
//!   `.func <name>[ local]`; the entry state is laid out first so control
//!   falls from the `ent` prologue straight into it;
//! * a **conditional** state (more than one rule, or any rule with a concrete
//!   match cell) lowers to `rd; mtc T<n>; djmp D<n>` plus a match table and a
//!   dispatch table in `.section tables`; each rule's action + transition
//!   becomes a dispatch-target block;
//! * a **straight-line** state (its one rule matches `[*,…]`) lowers to the
//!   action + transition with no match at all — and an unconditional chain of
//!   such states collapses for free through fall-through elision;
//! * a rule's action is ONE fused `wrmv [w…], [m…]` (the write vector then the
//!   move vector); an all-keep write with an all-stay move emits nothing;
//! * `debugger` emits `brk` at the rule's code head (before the `wrmv`),
//!   dropped under `--strip-debugger`;
//! * transitions: `goto` → `jmp <label>` (or fall-through), `call … then` →
//!   `call <name>[ [<binding>]]` then the resume, `return`/`stop`/`halt` →
//!   `ret`/`stp`/`hlt`, and the synthesized graft-hole traps → `trap #0` /
//!   `trap #1`.
//!
//! **Match-table discipline (§4 / GC4):** the exact rows (every cell concrete)
//! are sorted lexicographically and their dispatch targets move with them as
//! pairs — MR numbering and the `.targets` entries are emitted together, so
//! the sort is behaviour-preserving; partial-wildcard rows keep source order;
//! the catch-all `[*,…]` row is last. No catch-all is synthesized when the
//! source omits one — a non-match leaves MR = 0 and `djmp` traps, which is the
//! spec's deliberate NoTransition behaviour. Exact-row disjointness is a
//! front-end guarantee (T5), so the sort never sees a duplicate.
//!
//! **Layout invariant** (mirrors PM-1 codegen): an unconditional transfer to
//! the physically next block is never emitted — blocks are laid out in order
//! and fall-through is selected instead.

use std::cmp::Ordering;
use std::collections::HashSet;

use mtc_core::asm::grid_line as grid;

use crate::ir::{
    IrCell, IrMove, IrProgram, IrRule, IrState, IrTapeBinding, IrThen, IrTransition, IrWorld,
    IrWrite,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CodegenOptions {
    /// Drop `brk` ops (`--strip-debugger`).
    pub strip_debugger: bool,
}

/// Generated assembly plus the tma→tmc line correspondence that lets the
/// driver remap assembler debug lines back to `.tmc` sources.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TmaOutput {
    pub text: String,
    /// `(tma_line, tmc_line)`, 1-based on both sides, for every generated
    /// line that carries an instruction or the `.func` directive. Directives
    /// that name no source (`.section`, `.routine`, tables, labels) are absent.
    pub line_map: Vec<(u32, u32)>,
}

struct Emitter {
    lines: Vec<String>,
    line_map: Vec<(u32, u32)>,
}

impl Emitter {
    /// `tmc_line == 0` → no source correspondence (labels, tables, section
    /// and routine directives).
    fn push(&mut self, text: String, tmc_line: u32) {
        self.lines.push(text);
        if tmc_line != 0 {
            self.line_map.push((self.lines.len() as u32, tmc_line));
        }
    }
}

/// One instruction line inside a block: mnemonic + operand + its source line.
struct Instr {
    mnemonic: &'static str,
    operand: String,
    line: u32,
}

/// A control transfer at the end of a block.
enum Term {
    /// `djmp D<n>` — the conditional-state head's dispatch (always emitted).
    Djmp(String),
    /// `goto <state>` — `jmp <label>` or elided fall-through.
    Goto(String),
    /// `call <name>[ [<binding>]]` then a resume.
    Call {
        operand: String,
        then: Then,
    },
    Ret,
    Stop,
    Halt,
    TrapRead,
    TrapWrite,
}

/// A `call … then` resume point.
enum Then {
    Goto(String),
    Ret,
    Stop,
    Halt,
}

/// A laid-out code block: a label, whether the label must always print (a
/// dispatch target — reached only through `djmp`), body instructions, and a
/// terminator with its source line.
struct Block {
    label: String,
    force_label: bool,
    body: Vec<Instr>,
    term: Term,
    term_line: u32,
}

/// A match table plus its dispatch table, in emitted MR order (row K → target
/// K, both 1-based via `djmp`).
struct Table {
    label_t: String,
    label_d: String,
    rows: Vec<Vec<IrCell>>,
    targets: Vec<String>,
}

/// A world's lowering: its tables (for `.section tables`) and its laid-out
/// blocks (for its `.func`).
struct WorldPlan {
    tables: Vec<Table>,
    blocks: Vec<Block>,
}

pub fn emit_program(ir: &IrProgram, options: CodegenOptions) -> TmaOutput {
    let mut e = Emitter {
        lines: Vec::new(),
        line_map: Vec::new(),
    };

    // One traversal assigns global table indices (T<n>/D<n> are unique across
    // the whole file — the tables section is module-scoped), and builds each
    // world's tables and blocks.
    let mut table_idx = 0usize;
    let plans: Vec<WorldPlan> = ir
        .worlds
        .iter()
        .map(|w| build_world_plan(w, options, &mut table_idx))
        .collect();

    // Tables first (only when any exist); the code section follows. A
    // table-free program stays in the default code section, no directives.
    if plans.iter().any(|p| !p.tables.is_empty()) {
        e.push(".section tables".to_string(), 0);
        for p in &plans {
            for t in &p.tables {
                emit_table(t, &mut e);
            }
        }
        e.push(".section code".to_string(), 0);
    }

    for (w, p) in ir.worlds.iter().zip(&plans) {
        emit_func(w, p, &mut e);
    }

    let mut text = e.lines.join("\n");
    text.push('\n');
    TmaOutput {
        text,
        line_map: e.line_map,
    }
}

/// Build a world's tables + blocks. The entry state is placed first so the
/// `.func` prologue falls into it; the rest follow in id order.
fn build_world_plan(w: &IrWorld, options: CodegenOptions, table_idx: &mut usize) -> WorldPlan {
    // Emit order: entry state, then every other state by id.
    let mut order: Vec<usize> = vec![w.entry as usize];
    order.extend((0..w.states.len()).filter(|&i| i != w.entry as usize));

    // Collision-proof synthetic dispatch-target labels: seeded with the
    // world's state names, each `fresh` avoids every prior user or minted
    // name (mirrors expand's NameGen). State labels are the source names
    // (alnum+`_`, legal bare labels); dispatch-target labels are minted.
    let mut used: HashSet<String> = w.states.iter().map(|s| s.name.clone()).collect();

    let mut tables = Vec::new();
    let mut blocks = Vec::new();
    for &i in &order {
        let st = &w.states[i];
        if is_straight_line(st) {
            blocks.push(straight_block(w, st, options));
        } else {
            let n = *table_idx;
            *table_idx += 1;
            let (table, rule_blocks) = conditional(w, st, options, n, &mut used);
            tables.push(table);
            // Head: rd; mtc T<n>; djmp D<n>. The djmp is the terminator.
            blocks.push(Block {
                label: st.name.clone(),
                force_label: false,
                body: vec![
                    Instr {
                        mnemonic: "rd",
                        operand: String::new(),
                        line: st.line,
                    },
                    Instr {
                        mnemonic: "mtc",
                        operand: format!("T{n}"),
                        line: st.line,
                    },
                ],
                term: Term::Djmp(format!("D{n}")),
                term_line: st.line,
            });
            blocks.extend(rule_blocks);
        }
    }

    WorldPlan { tables, blocks }
}

/// A state is straight-line iff its single rule matches every tape (`[*,…]`):
/// no match is needed, the action + transition run directly.
fn is_straight_line(st: &IrState) -> bool {
    st.rules.len() == 1
        && st.rules[0]
            .pattern
            .iter()
            .all(|c| matches!(c, IrCell::Wildcard))
}

/// The single block a straight-line state lowers to.
fn straight_block(w: &IrWorld, st: &IrState, options: CodegenOptions) -> Block {
    let r = &st.rules[0];
    Block {
        label: st.name.clone(),
        force_label: false,
        body: rule_body(r, options),
        term: term_of(w, r),
        term_line: r.line,
    }
}

/// A conditional state's match/dispatch table plus one block per rule (the
/// dispatch targets), in source (row) order. The table's rows and `.targets`
/// are ordered per GC4; the blocks stay in source order for readable layout.
fn conditional(
    w: &IrWorld,
    st: &IrState,
    options: CodegenOptions,
    n: usize,
    used: &mut HashSet<String>,
) -> (Table, Vec<Block>) {
    // Mint a dispatch-target label per rule, in source order.
    let rule_labels: Vec<String> = (0..st.rules.len())
        .map(|k| fresh(used, &format!("{}__{k}", st.name)))
        .collect();

    // Classify + order the rows: sorted exact, then partial (source order),
    // then catch-all (source order) — each carrying its target.
    let mut exact: Vec<(Vec<IrCell>, String)> = Vec::new();
    let mut partial: Vec<(Vec<IrCell>, String)> = Vec::new();
    let mut catch_all: Vec<(Vec<IrCell>, String)> = Vec::new();
    for (k, r) in st.rules.iter().enumerate() {
        let cells = r.pattern.clone();
        let tgt = rule_labels[k].clone();
        if cells.iter().all(|c| matches!(c, IrCell::Index { .. })) {
            exact.push((cells, tgt));
        } else if cells.iter().all(|c| matches!(c, IrCell::Wildcard)) {
            catch_all.push((cells, tgt));
        } else {
            partial.push((cells, tgt));
        }
    }
    exact.sort_by(|a, b| cmp_row(&a.0, &b.0));

    let mut rows = Vec::new();
    let mut targets = Vec::new();
    for (cells, tgt) in exact.into_iter().chain(partial).chain(catch_all) {
        rows.push(cells);
        targets.push(tgt);
    }

    let table = Table {
        label_t: format!("T{n}"),
        label_d: format!("D{n}"),
        rows,
        targets,
    };

    let blocks: Vec<Block> = st
        .rules
        .iter()
        .enumerate()
        .map(|(k, r)| Block {
            label: rule_labels[k].clone(),
            force_label: true,
            body: rule_body(r, options),
            term: term_of(w, r),
            term_line: r.line,
        })
        .collect();

    (table, blocks)
}

/// A rule's body: an optional `brk` (unless stripped) then an optional fused
/// `wrmv`. An all-keep write with an all-stay move (both elided in the IR)
/// emits no `wrmv` at all.
fn rule_body(r: &IrRule, options: CodegenOptions) -> Vec<Instr> {
    let mut body = Vec::new();
    if r.debugger && !options.strip_debugger {
        body.push(Instr {
            mnemonic: "brk",
            operand: String::new(),
            line: r.line,
        });
    }
    if let Some(op) = wrmv_operand(r) {
        body.push(Instr {
            mnemonic: "wrmv",
            operand: op,
            line: r.line,
        });
    }
    body
}

/// The `[w…], [m…]` operand for a rule's action, or `None` when neither a
/// write nor a move is present. The canon emits ONE fused `wrmv`, so a
/// write-only rule still supplies an all-stay move vector and a move-only rule
/// an all-keep write vector — canon over bytes; 6b's passes may narrow.
fn wrmv_operand(r: &IrRule) -> Option<String> {
    if r.write.is_none() && r.moves.is_none() {
        return None;
    }
    let arity = r.pattern.len();
    let writes: Vec<String> = match &r.write {
        Some(v) => v
            .iter()
            .map(|w| match w {
                IrWrite::Keep => "-".to_string(),
                IrWrite::Index { index } => index.to_string(),
            })
            .collect(),
        None => vec!["-".to_string(); arity],
    };
    let moves: Vec<String> = match &r.moves {
        Some(v) => v.iter().map(move_glyph).collect(),
        None => vec![".".to_string(); arity],
    };
    Some(format!("[{}], [{}]", writes.join(", "), moves.join(", ")))
}

fn move_glyph(m: &IrMove) -> String {
    match m {
        IrMove::Left => "<",
        IrMove::Right => ">",
        IrMove::Stay => ".",
    }
    .to_string()
}

/// A rule's control transfer as a [`Term`]. `goto`/`then`-`goto` targets
/// resolve to their state's block label (its source name).
fn term_of(w: &IrWorld, r: &IrRule) -> Term {
    match &r.transition {
        IrTransition::Goto { state } => Term::Goto(state_label(w, *state)),
        IrTransition::CallThen {
            target,
            binding,
            then,
        } => {
            let operand = if binding.is_empty() {
                target.clone()
            } else {
                format!("{} {}", target, render_binding(binding))
            };
            let then = match then {
                IrThen::Goto { state } => Then::Goto(state_label(w, *state)),
                IrThen::Return => Then::Ret,
                IrThen::Stop => Then::Stop,
                IrThen::Halt => Then::Halt,
            };
            Term::Call { operand, then }
        }
        IrTransition::Return => Term::Ret,
        IrTransition::Stop => Term::Stop,
        IrTransition::Halt => Term::Halt,
        IrTransition::TrapRead => Term::TrapRead,
        IrTransition::TrapWrite => Term::TrapWrite,
    }
}

fn state_label(w: &IrWorld, id: u32) -> String {
    w.states[id as usize].name.clone()
}

/// Render the binding-call operand's bracket interior (docs/formats.md
/// (bound calls)): one entry per callee virtual tape, `<physIdx>` with an
/// optional `{ <pair>, … }` symbol map (`->` two-way, `=>` one-way). Every
/// pair the record carries is rendered, in order.
fn render_binding(binding: &[IrTapeBinding]) -> String {
    let entries: Vec<String> = binding
        .iter()
        .map(|b| {
            if b.pairs.is_empty() {
                b.caller_tape.to_string()
            } else {
                let pairs: Vec<String> = b
                    .pairs
                    .iter()
                    .map(|p| {
                        let arrow = if p.one_way { "=>" } else { "->" };
                        format!("{}{}{}", p.src, arrow, p.dst)
                    })
                    .collect();
                format!("{}{{{}}}", b.caller_tape, pairs.join(", "))
            }
        })
        .collect();
    format!("[{}]", entries.join(", "))
}

/// Lexicographic order over two exact rows (all cells concrete). Wildcards
/// sort last defensively; exact rows never carry one.
fn cmp_row(a: &[IrCell], b: &[IrCell]) -> Ordering {
    for (x, y) in a.iter().zip(b) {
        match cell_key(x).cmp(&cell_key(y)) {
            Ordering::Equal => continue,
            o => return o,
        }
    }
    Ordering::Equal
}

fn cell_key(c: &IrCell) -> u64 {
    match c {
        IrCell::Index { index } => *index as u64,
        IrCell::Wildcard => u64::MAX,
    }
}

fn row_str(cells: &[IrCell]) -> String {
    let elems: Vec<String> = cells
        .iter()
        .map(|c| match c {
            IrCell::Wildcard => "*".to_string(),
            IrCell::Index { index } => index.to_string(),
        })
        .collect();
    format!("[{}]", elems.join(", "))
}

/// Emit a match/dispatch table pair into `.section tables`. The first `.row`
/// carries the `T<n>` label (opening the table run); continuation rows are
/// unlabeled; the `D<n>: .targets …` line closes it.
fn emit_table(t: &Table, e: &mut Emitter) {
    for (i, row) in t.rows.iter().enumerate() {
        let op = row_str(row);
        let label = if i == 0 {
            Some(t.label_t.as_str())
        } else {
            None
        };
        e.push(grid(label, ".row", &op), 0);
    }
    e.push(grid(Some(&t.label_d), ".targets", &t.targets.join(", ")), 0);
}

/// Emit one world's `.routine` signature + `.func` + laid-out blocks.
fn emit_func(w: &IrWorld, p: &WorldPlan, e: &mut Emitter) {
    let alpha: Vec<String> = w.tapes.iter().map(|t| t.cardinality.to_string()).collect();
    e.push(
        format!(
            ".routine {}, tapes={}, alpha=({})",
            w.name,
            w.arity,
            alpha.join(", ")
        ),
        0,
    );
    e.push(
        format!(".func {}{}", w.name, if w.local { " local" } else { "" }),
        w.line,
    );

    // Which labels print: every dispatch target (reached only via `djmp`),
    // plus any state reached by a NON-elided `jmp` (a `goto`/`then` whose
    // target is not the physically next block).
    let next_label = |i: usize| p.blocks.get(i + 1).map(|b| b.label.as_str());
    let mut printed: HashSet<&str> = p
        .blocks
        .iter()
        .filter(|b| b.force_label)
        .map(|b| b.label.as_str())
        .collect();
    for (i, b) in p.blocks.iter().enumerate() {
        let goto_target = match &b.term {
            Term::Goto(t) => Some(t.as_str()),
            Term::Call {
                then: Then::Goto(t),
                ..
            } => Some(t.as_str()),
            _ => None,
        };
        if let Some(t) = goto_target
            && next_label(i) != Some(t)
        {
            printed.insert(t);
        }
    }

    for (i, b) in p.blocks.iter().enumerate() {
        if printed.contains(b.label.as_str()) {
            e.push(format!("{}:", b.label), 0);
        }
        for ins in &b.body {
            e.push(grid(None, ins.mnemonic, &ins.operand), ins.line);
        }
        let emit_goto = |e: &mut Emitter, t: &str| {
            if next_label(i) != Some(t) {
                e.push(grid(None, "jmp", t), b.term_line);
            }
        };
        match &b.term {
            Term::Djmp(d) => e.push(grid(None, "djmp", d), b.term_line),
            Term::Goto(t) => emit_goto(e, t),
            Term::Call { operand, then } => {
                e.push(grid(None, "call", operand), b.term_line);
                match then {
                    Then::Goto(t) => emit_goto(e, t),
                    Then::Ret => e.push(grid(None, "ret", ""), b.term_line),
                    Then::Stop => e.push(grid(None, "stp", ""), b.term_line),
                    Then::Halt => e.push(grid(None, "hlt", ""), b.term_line),
                }
            }
            Term::Ret => e.push(grid(None, "ret", ""), b.term_line),
            Term::Stop => e.push(grid(None, "stp", ""), b.term_line),
            Term::Halt => e.push(grid(None, "hlt", ""), b.term_line),
            Term::TrapRead => e.push(grid(None, "trap", "#0"), b.term_line),
            Term::TrapWrite => e.push(grid(None, "trap", "#1"), b.term_line),
        }
    }
}

/// A collision-proof label minter (mirrors expand's `NameGen`): seeded with
/// the world's state names, each call returns a name absent from every prior
/// user or minted name (bumping a numeric suffix).
fn fresh(used: &mut HashSet<String>, base: &str) -> String {
    if used.insert(base.to_string()) {
        return base.to_string();
    }
    let mut i = 1;
    loop {
        let cand = format!("{base}_{i}");
        if used.insert(cand.clone()) {
            return cand;
        }
        i += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // The six Appendix A programs (spec §A, verbatim). The expected `.tma`
    // text below each is HAND-DERIVED from the -O0 canon (row sort, wrmv
    // uniformity, fall-through elision) and confirmed to assemble — never
    // regenerated from output.
    const A1: &str = "\
alphabet ab { '_', 'a', 'b' }
machine {
  tape main: ab;
  entry state scan {
    ['b'] -> write ['a'] move [>] goto scan;
    ['a'] ->            move [>] goto scan;
    ['_'] -> stop;
  }
}";

    const A2: &str = "\
alphabet bits { '_', '0', '1' }
machine {
  tape num: bits;
  entry state inc {
    ['1'] -> write ['0'] move [<] goto inc;
    ['0'] -> write ['1'] stop;
    ['_'] -> write ['1'] stop;
  }
}";

    const A3: &str = "\
alphabet bits { '_', '0', '1' }
machine {
  tape src: bits;
  tape dst: bits;
  entry state copy {
    ['0'..'1' as c, *] -> write [-, {c}] move [>, >] goto copy;
    ['_', *]           -> stop;
  }
}";

    const A4: &str = "\
alphabet bytes { 0..126 }
machine {
  tape cell: bytes;
  entry state inc {
    [1..125 as v] -> write [{v+1}] stop;
    [126]         -> halt;
    [0]           -> write [1] stop;
  }
}";

    const A5: &str = "\
alphabet bits { '_', '0', '1' }
alphabet wide { '_', 'a', 'b', '0', '1' }
namespace mylib {
  export routine plusOne(tape num: bits) {
    entry state inc {
      ['1'] -> write ['0'] move [<] goto inc;
      [*]   -> write ['1'] return;
    }
  }
}
use mylib::plusOne;
machine {
  tape ctl:  bits;
  tape data: wide;
  entry state main {
    ['1', *] -> call plusOne(num = data with map { '0'->'0', '1'->'1' }) then done;
    [*, *]   -> move [>, .] goto main;
  }
  state done { [*, *] -> stop; }
}";

    const A6: &str = "\
alphabet marks { '_', 'x', 'y', 'z' }
export graph findX(tape t: marks, state found, state missing) {
  entry state walk {
    ['x'] -> found;
    ['_'] -> missing;
    [*]   -> move [>] goto walk;
  }
}
machine {
  tape work: marks;
  entry graft findX(t = work, found = celebrate, missing = giveUp) as seek;
  state celebrate { [*] -> write ['_'] stop; }
  state giveUp    { [*] -> halt; }
}";

    fn ir_of(src: &str) -> IrProgram {
        let a = crate::compiler::analyze(src).expect("analyze");
        let ex = crate::expand::expand(&a.resolved).expect("expand");
        let (ir, _) = crate::ir::lower(&ex, &a.resolved).expect("lower");
        ir
    }

    fn emit(src: &str) -> String {
        emit_program(&ir_of(src), CodegenOptions::default()).text
    }

    /// Every Appendix A example, once emitted, assembles cleanly — the object
    /// is what the compile pipeline then hands to the linker.
    fn assert_assembles(text: &str) {
        crate::asm::assemble(text, false)
            .unwrap_or_else(|e| panic!("generated .tma failed to assemble: {e}\n{text}"));
    }

    // -- Full-text snapshots (the small examples; A.4 is 127 rows and gets a
    //    structural check instead) --------------------------------------------

    #[test]
    fn a1_replace_b() {
        // scan has three exact rows in source order [b=2],[a=1],[_=0]; the
        // sort reorders them ascending [0],[1],[2] and their targets move as
        // pairs → D0 = scan__2, scan__1, scan__0. The write-only rule keeps an
        // all-stay move (`wrmv [-], [>]`).
        let expected = "\
.section tables
T0:     .row    [0]
        .row    [1]
        .row    [2]
D0:     .targets scan__2, scan__1, scan__0
.section code
.routine main, tapes=1, alpha=(3)
.func main
scan:
        rd
        mtc     T0
        djmp    D0
scan__0:
        wrmv    [1], [>]
        jmp     scan
scan__1:
        wrmv    [-], [>]
        jmp     scan
scan__2:
        stp
";
        let out = emit(A1);
        assert_eq!(out, expected);
        assert_assembles(&out);
    }

    #[test]
    fn a2_binary_plus_one() {
        // Two write-only rules emit `wrmv [w], [.]` (canon over a bare `wr`).
        let expected = "\
.section tables
T0:     .row    [0]
        .row    [1]
        .row    [2]
D0:     .targets inc__2, inc__1, inc__0
.section code
.routine main, tapes=1, alpha=(3)
.func main
inc:
        rd
        mtc     T0
        djmp    D0
inc__0:
        wrmv    [1], [<]
        jmp     inc
inc__1:
        wrmv    [2], [.]
        stp
inc__2:
        wrmv    [2], [.]
        stp
";
        let out = emit(A2);
        assert_eq!(out, expected);
        assert_assembles(&out);
    }

    #[test]
    fn a3_two_tape_copy() {
        // The `'0'..'1' as c` binding expands to two partial rows [1,*],[2,*]
        // (mix of concrete + wildcard) — partial rows keep source order, so no
        // sort applies; the '_' rule is [0,*], also partial.
        let expected = "\
.section tables
T0:     .row    [1, *]
        .row    [2, *]
        .row    [0, *]
D0:     .targets copy__0, copy__1, copy__2
.section code
.routine main, tapes=2, alpha=(3, 3)
.func main
copy:
        rd
        mtc     T0
        djmp    D0
copy__0:
        wrmv    [-, 1], [>, >]
        jmp     copy
copy__1:
        wrmv    [-, 2], [>, >]
        jmp     copy
copy__2:
        stp
";
        let out = emit(A3);
        assert_eq!(out, expected);
        assert_assembles(&out);
    }

    #[test]
    fn a5_routine_call_across_alphabets() {
        // Two worlds: the exported routine (no ` local`) emitted first with
        // T0/D0, then the machine with T1/D1 (the table counter is global).
        // The cross-alphabet call renders the binding-call operand
        // `call mylib::plusOne [1{3->1, 4->2}]` (host tape 1 = data; wide '0'
        // = idx 3 → bits '0' = idx 1, wide '1' = idx 4 → bits '1' = idx 2).
        let expected = "\
.section tables
T0:     .row    [2]
        .row    [*]
D0:     .targets inc__0, inc__1
T1:     .row    [2, *]
        .row    [*, *]
D1:     .targets main__0, main__1
.section code
.routine mylib::plusOne, tapes=1, alpha=(3)
.func mylib::plusOne
inc:
        rd
        mtc     T0
        djmp    D0
inc__0:
        wrmv    [1], [<]
        jmp     inc
inc__1:
        wrmv    [2], [.]
        ret
.routine main, tapes=2, alpha=(3, 5)
.func main
main:
        rd
        mtc     T1
        djmp    D1
main__0:
        call    mylib::plusOne [1{3->1, 4->2}]
        jmp     done
main__1:
        wrmv    [-, -], [>, .]
        jmp     main
done:
        stp
";
        let out = emit(A5);
        assert_eq!(out, expected);
        assert_assembles(&out);
    }

    #[test]
    fn a6_graph_graft_entry_instance() {
        // The graph is spliced away; only `main` is emitted. The entry graft
        // instance `seek` is the world entry, laid out first (walk's states
        // renamed). Its exact rows [x=1]→celebrate, [_=0]→giveUp sort to
        // [0],[1] and the catch-all [*] lands last → D0 = seek__1, seek__0,
        // seek__2. The state-param continuations resolved to host states.
        let expected = "\
.section tables
T0:     .row    [0]
        .row    [1]
        .row    [*]
D0:     .targets seek__1, seek__0, seek__2
.section code
.routine main, tapes=1, alpha=(4)
.func main
seek:
        rd
        mtc     T0
        djmp    D0
seek__0:
        jmp     celebrate
seek__1:
        jmp     giveUp
seek__2:
        wrmv    [-], [>]
        jmp     seek
celebrate:
        wrmv    [0], [.]
        stp
giveUp:
        hlt
";
        let out = emit(A6);
        assert_eq!(out, expected);
        assert_assembles(&out);
    }

    #[test]
    fn a4_byte_increment_assembles_with_a_sorted_127_row_table() {
        // 127 symbols → the `1..125 as v` binding expands to 125 rows, plus
        // [126] and [0]: 127 exact rows total. Snapshotting 127 rows is
        // pointless; assert the structural canon instead.
        let out = emit(A4);
        assert_assembles(&out);
        // One conditional state → one table, 127 rows.
        assert_eq!(out.matches(".row").count(), 127, "{out}");
        // The exact rows sort ascending; the [0] rule is the last-authored one
        // (source index 126), so the first dispatch target is inc__126.
        assert!(
            out.contains("D0:     .targets inc__126, inc__0, inc__1,"),
            "{out}"
        );
        // v=1 writes v+1=2 (`wrmv [2], [.]`); the [0] rule writes 1; [126] halts.
        assert!(out.contains("        wrmv    [2], [.]"), "{out}");
        assert!(out.contains("        wrmv    [1], [.]"), "{out}");
        assert!(out.contains("        hlt"), "{out}");
    }

    // -- Focused canon checks ------------------------------------------------

    #[test]
    fn table_sort_pairs_targets_with_their_rows() {
        // Exact rows authored 2,1,0 — the sort reorders the rows to 0,1,2 AND
        // moves each dispatch target with its row (behaviour-preserving).
        let out = emit(A1);
        let tables = out.split(".section code").next().unwrap();
        // Rows appear ascending…
        let p0 = tables.find("[0]").unwrap();
        let p1 = tables.find("[1]").unwrap();
        let p2 = tables.find("[2]").unwrap();
        assert!(p0 < p1 && p1 < p2, "rows not ascending: {tables}");
        // …and the targets are the rules those rows came from, in that order:
        // row [0] was rule 2 (scan__2), [1] rule 1, [2] rule 0.
        assert!(
            out.contains("D0:     .targets scan__2, scan__1, scan__0"),
            "{out}"
        );
    }

    #[test]
    fn fall_through_elision_collapses_an_unconditional_chain() {
        // a -> b -> stop, both straight-line: the goto to the physically next
        // state emits no `jmp` — the chain collapses to straight-line code.
        let src = "\
alphabet bits { '_', '0', '1' }
machine {
  tape t: bits;
  entry state a { [*] -> move [>] goto b; }
  state b { [*] -> stop; }
}";
        let out = emit(src);
        assert!(!out.contains("jmp"), "chain should collapse, no jmp: {out}");
        assert_assembles(&out);

        // A self-loop is NOT adjacent to itself as the next block, so its jmp
        // survives (and its label prints).
        let loop_src = "\
alphabet bits { '_', '0', '1' }
machine {
  tape t: bits;
  entry state a { [*] -> move [>] goto a; }
}";
        assert!(
            emit(loop_src).contains("        jmp     a"),
            "{}",
            emit(loop_src)
        );
    }

    #[test]
    fn strip_debugger_drops_brk() {
        let src = "\
alphabet bits { '_', '0', '1' }
machine {
  tape t: bits;
  entry state s { [*] -> debugger move [>] stop; }
}";
        let ir = ir_of(src);
        let kept = emit_program(&ir, CodegenOptions::default()).text;
        assert!(kept.contains("        brk"), "{kept}");
        let stripped = emit_program(
            &ir,
            CodegenOptions {
                strip_debugger: true,
            },
        )
        .text;
        assert!(!stripped.contains("brk"), "{stripped}");
        // The instruction stream is otherwise unchanged; both assemble.
        assert_assembles(&kept);
        assert_assembles(&stripped);
    }

    #[test]
    fn line_map_points_tma_lines_at_their_tmc_sources() {
        // A1: `.func` ← the `machine {` line (2); the `scan` head's rd/mtc/djmp
        // ← the state decl line (4); each rule's wrmv/jmp/stp ← its rule line
        // (5/6/7).
        let out = emit_program(&ir_of(A1), CodegenOptions::default());
        let map: std::collections::HashMap<u32, u32> = out.line_map.iter().copied().collect();
        // Locate the emitted lines by content and check their tmc source.
        let lines: Vec<&str> = out.text.lines().collect();
        let tma_of = |needle: &str| {
            lines
                .iter()
                .position(|l| l.trim_start().starts_with(needle))
                .map(|i| i as u32 + 1)
                .unwrap_or_else(|| panic!("no line {needle:?}"))
        };
        assert_eq!(map.get(&tma_of(".func main")), Some(&2));
        assert_eq!(map.get(&tma_of("rd")), Some(&4));
        assert_eq!(map.get(&tma_of("stp")), Some(&7));
        // Label / table / section lines carry no source correspondence.
        assert_eq!(map.get(&tma_of(".section tables")), None);
        assert_eq!(map.get(&tma_of("scan:")), None);
    }
}
