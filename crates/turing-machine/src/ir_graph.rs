//! The Mermaid renderings of one IR world (`tmt ir graph`): the row-for-row
//! RAW view, the lossless MERGED view (the default), and the SHAPE view —
//! one arrow per pair of states (docs/tmt/cli.md (tmt ir)). The label
//! notation the merged view writes — ranges, sets and run compression — is
//! specified once in docs/formats.md (graph label notation), and the two
//! functions that produce it, [`read_cell_label`] and [`compress`], are
//! public so every consumer renders the same text.
//!
//! # What merging may and may not do
//!
//! Two rows of one state merge only when they agree on everything but ONE
//! read cell — same write vector, same move vector, same `debugger` flag,
//! same transition (a call's whole binding record and exit vector included)
//! — and differ in that one cell by DISJOINT symbol sets. The merged row
//! takes the union there; merged rows merge again under the same rule
//! until nothing changes. A `*` cell equals only a `*` cell and never joins
//! a set. Together these make merging lossless: the cartesian product of a
//! merged row's cells is exactly the set of raw rows it replaced, with
//! multiplicity (the disjointness is what keeps two identical raw rows two
//! rows). Rows that differ in two cells never merge, because a per-cell
//! union of `[1,2]` and `[2,1]` would also describe `[1,1]` and `[2,2]`.
//!
//! A merged row is drawn where its FIRST raw row stood, so merging moves
//! the later rows up. Among the rows that carry a wildcard, source order is
//! what decides which one fires (docs/tmt/language.md (which rule fires)),
//! so a row is never moved above an earlier wildcard-bearing row whose
//! pattern overlaps its own: the merged view keeps every such pair in its
//! original order. Exact rows and the catch-all are not ordered by
//! position, so they move freely.

use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;

use crate::ir::{IrCell, IrMove, IrRule, IrThen, IrTransition, IrWorld, IrWrite};

/// Which rendering `ir graph` prints.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum GraphView {
    /// Parallel rows merged losslessly, labels in the compact notation.
    #[default]
    Merged,
    /// One arrow per (source, target) pair, labelled with its row count.
    Shape,
    /// One arrow per row, exactly as the IR lists them.
    Raw,
}

impl GraphView {
    pub fn parse(s: &str) -> Option<GraphView> {
        match s {
            "merged" => Some(GraphView::Merged),
            "shape" => Some(GraphView::Shape),
            "raw" => Some(GraphView::Raw),
            _ => None,
        }
    }
}

/// One read cell of a merged row: `*`, or a non-empty set of symbol
/// indices, ascending and distinct.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ReadCell {
    Any,
    Set(Vec<u32>),
}

impl ReadCell {
    fn overlaps(&self, other: &ReadCell) -> bool {
        match (self, other) {
            (ReadCell::Any, _) | (_, ReadCell::Any) => true,
            (ReadCell::Set(a), ReadCell::Set(b)) => a.iter().any(|x| b.binary_search(x).is_ok()),
        }
    }
}

/// A row of the merged view: the per-cell read sets, the action and
/// transition every raw row in it shares, and those raw rows' positions in
/// the state's rule list (ascending — `rows[0]` is where it is drawn).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergedRow {
    pub pattern: Vec<ReadCell>,
    pub write: Option<Vec<IrWrite>>,
    pub moves: Option<Vec<IrMove>>,
    pub debugger: bool,
    pub transition: IrTransition,
    pub rows: Vec<usize>,
}

impl MergedRow {
    /// Every raw pattern this row stands for: the cartesian product of its
    /// cells, in ascending order cell by cell.
    pub fn expand(&self) -> Vec<Vec<IrCell>> {
        let mut out: Vec<Vec<IrCell>> = vec![Vec::new()];
        for cell in &self.pattern {
            let choices: Vec<IrCell> = match cell {
                ReadCell::Any => vec![IrCell::Wildcard],
                ReadCell::Set(s) => s.iter().map(|&index| IrCell::Index { index }).collect(),
            };
            out = out
                .into_iter()
                .flat_map(|prefix| {
                    choices.iter().map(move |c| {
                        let mut p = prefix.clone();
                        p.push(*c);
                        p
                    })
                })
                .collect();
        }
        out
    }

    fn partial(&self) -> bool {
        self.pattern.contains(&ReadCell::Any) && self.pattern.iter().any(|c| *c != ReadCell::Any)
    }
}

/// Merge one state's rules under the lossless rule (see the module doc),
/// returning the merged rows in the order of their first raw row.
pub fn merge_rows(rules: &[IrRule]) -> Vec<MergedRow> {
    // Arrow `i` starts as rule `i`; a merge folds the later arrow into the
    // earlier one, so a live arrow's index is always its first raw row.
    let mut arrows: Vec<Option<MergedRow>> = rules
        .iter()
        .enumerate()
        .map(|(i, r)| {
            Some(MergedRow {
                pattern: r
                    .pattern
                    .iter()
                    .map(|c| match c {
                        IrCell::Wildcard => ReadCell::Any,
                        IrCell::Index { index } => ReadCell::Set(vec![*index]),
                    })
                    .collect(),
                write: r.write.clone(),
                moves: r.moves.clone(),
                debugger: r.debugger,
                transition: r.transition.clone(),
                rows: vec![i],
            })
        })
        .collect();
    // Everything but the pattern, as a comparable key: rows merge only
    // within one key.
    let keys: Vec<usize> = {
        let mut seen: Vec<&IrRule> = Vec::new();
        rules
            .iter()
            .map(|r| {
                match seen.iter().position(|s| {
                    s.write == r.write
                        && s.moves == r.moves
                        && s.debugger == r.debugger
                        && s.transition == r.transition
                }) {
                    Some(k) => k,
                    None => {
                        seen.push(r);
                        seen.len() - 1
                    }
                }
            })
            .collect()
    };
    let arity = rules.first().map_or(0, |r| r.pattern.len());
    loop {
        let mut changed = false;
        for c in 0..arity {
            // Buckets of live arrows agreeing on the key and on every cell
            // but `c`, in first-row order.
            let mut bucket_of: HashMap<(usize, Vec<ReadCell>), usize> = HashMap::new();
            let mut buckets: Vec<Vec<usize>> = Vec::new();
            for (i, a) in arrows.iter().enumerate() {
                let Some(a) = a else { continue };
                let mut rest = a.pattern.clone();
                rest[c] = ReadCell::Any;
                let b = *bucket_of.entry((keys[i], rest)).or_insert_with(|| {
                    buckets.push(Vec::new());
                    buckets.len() - 1
                });
                buckets[b].push(i);
            }
            for bucket in buckets {
                let mut accs: Vec<usize> = Vec::new();
                for i in bucket {
                    let merged_into = accs.iter().copied().find(|&b| {
                        let (Some(ab), Some(ai)) = (&arrows[b], &arrows[i]) else {
                            return false;
                        };
                        let (ReadCell::Set(sb), ReadCell::Set(si)) =
                            (&ab.pattern[c], &ai.pattern[c])
                        else {
                            return false;
                        };
                        sb.iter().all(|x| si.binary_search(x).is_err())
                            && keeps_order(&arrows, b, i)
                    });
                    match merged_into {
                        Some(b) => {
                            let moved = arrows[i].take().expect("live arrow");
                            let into = arrows[b].as_mut().expect("live accumulator");
                            if let (ReadCell::Set(dst), ReadCell::Set(src)) =
                                (&mut into.pattern[c], moved.pattern[c].clone())
                            {
                                dst.extend(src);
                                dst.sort_unstable();
                            }
                            into.rows.extend(moved.rows);
                            into.rows.sort_unstable();
                            changed = true;
                        }
                        None => accs.push(i),
                    }
                }
            }
        }
        if !changed {
            break;
        }
    }
    arrows.into_iter().flatten().collect()
}

/// Whether arrow `later` may be drawn at arrow `earlier`'s position: no
/// live wildcard-bearing arrow between the two may overlap it, when it
/// carries a wildcard itself (docs/tmt/language.md (which rule fires)).
fn keeps_order(arrows: &[Option<MergedRow>], earlier: usize, later: usize) -> bool {
    let moving = arrows[later].as_ref().expect("live arrow");
    if !moving.partial() {
        return true;
    }
    arrows[earlier + 1..later].iter().flatten().all(|z| {
        !z.partial()
            || !z
                .pattern
                .iter()
                .zip(&moving.pattern)
                .all(|(a, b)| a.overlaps(b))
    })
}

/// The label of one read cell holding the symbol set `set` (ascending,
/// distinct, non-empty): a single index as itself, a run of three or more
/// consecutive indices as `a–b`, anything else in braces with such runs
/// inside — `{1,2}`, `{1–3,7}` (docs/formats.md (graph label notation)).
pub fn read_cell_label(set: &[u32]) -> String {
    let mut runs: Vec<(u32, u32)> = Vec::new();
    for &v in set {
        match runs.last_mut() {
            Some((_, hi)) if v == *hi + 1 => *hi = v,
            _ => runs.push((v, v)),
        }
    }
    let run = |(lo, hi): (u32, u32)| match hi - lo {
        0 => lo.to_string(),
        1 => format!("{lo},{hi}"),
        _ => format!("{lo}\u{2013}{hi}"),
    };
    match runs.as_slice() {
        [single] if single.1 - single.0 != 1 => run(*single),
        _ => format!(
            "{{{}}}",
            runs.iter().map(|r| run(*r)).collect::<Vec<_>>().join(",")
        ),
    }
}

/// One vector's cells joined by commas, three or more identical adjacent
/// cells written once as `cell×k` (docs/formats.md (graph label notation)).
pub fn compress(cells: &[String]) -> String {
    let mut parts: Vec<String> = Vec::new();
    let mut i = 0;
    while i < cells.len() {
        let mut j = i + 1;
        while j < cells.len() && cells[j] == cells[i] {
            j += 1;
        }
        let k = j - i;
        if k >= 3 {
            parts.push(format!("{}\u{00D7}{k}", cells[i]));
        } else {
            parts.extend(cells[i..j].iter().cloned());
        }
        i = j;
    }
    parts.join(",")
}

/// Render `world` in `view`.
pub fn render(world: &IrWorld, view: GraphView) -> String {
    let mut out = String::from("flowchart TD\n");
    for st in &world.states {
        let _ = writeln!(out, "    S{}[\"{}\"]", st.id, escape(&st.name));
    }
    // Every drawn arrow as (source, target, label), in drawing order.
    let mut arrows: Vec<(u32, Node, String)> = Vec::new();
    for st in &world.states {
        match view {
            GraphView::Raw => {
                for r in &st.rules {
                    let label = raw_label(r);
                    for (node, text) in edges(&r.transition, &label) {
                        arrows.push((st.id, node, text));
                    }
                }
            }
            GraphView::Merged => {
                for m in merge_rows(&st.rules) {
                    let label = merged_label(&m);
                    for (node, text) in edges(&m.transition, &label) {
                        arrows.push((st.id, node, text));
                    }
                }
            }
            GraphView::Shape => {
                // Distinct rows per target, targets in first-drawn order.
                let mut order: Vec<Node> = Vec::new();
                let mut rows: HashMap<Node, HashSet<usize>> = HashMap::new();
                for (i, r) in st.rules.iter().enumerate() {
                    for (node, _) in edges(&r.transition, "") {
                        let set = rows.entry(node).or_insert_with(|| {
                            order.push(node);
                            HashSet::new()
                        });
                        set.insert(i);
                    }
                }
                for node in order {
                    arrows.push((st.id, node, rows[&node].len().to_string()));
                }
            }
        }
    }
    // Terminal nodes are declared once each, in first-use order, after the
    // states and before any edge.
    let mut declared: HashSet<Node> = HashSet::new();
    let mut edges_text = String::new();
    for (src, node, label) in &arrows {
        if let Some(text) = node.terminal_text()
            && declared.insert(*node)
        {
            let _ = writeln!(out, "    {}((\"{text}\"))", node.id());
        }
        let _ = writeln!(
            edges_text,
            "    S{src} -->|\"{}\"| {}",
            mtc_core::mermaid::edge_label(label),
            node.id()
        );
    }
    out.push_str(&edges_text);
    out
}

/// An arrow's target: a state of the world, or one of the shared round
/// terminal nodes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Node {
    State(u32),
    Ret,
    RetExit(u32),
    Stp,
    Hlt,
    Tail,
    Trap(u8),
}

impl Node {
    fn id(self) -> String {
        match self {
            Node::State(s) => format!("S{s}"),
            Node::Ret => "T_ret".into(),
            Node::RetExit(k) => format!("T_ret{k}"),
            Node::Stp => "T_stp".into(),
            Node::Hlt => "T_hlt".into(),
            Node::Tail => "T_tail".into(),
            Node::Trap(k) => format!("T_trap{k}"),
        }
    }

    fn terminal_text(self) -> Option<String> {
        Some(match self {
            Node::State(_) => return None,
            Node::Ret => "ret".into(),
            Node::RetExit(k) => format!("ret #{k}"),
            Node::Stp => "stp".into(),
            Node::Hlt => "hlt".into(),
            Node::Tail => "tail".into(),
            Node::Trap(k) => format!("trap #{k}"),
        })
    }
}

/// The arrows one row draws, each with its full label, given the row's
/// pattern/action summary `label`. A call draws one arrow per declared
/// exit and then one for its `then` (none for an omitted `then`: control
/// never comes back to that call).
fn edges(transition: &IrTransition, label: &str) -> Vec<(Node, String)> {
    match transition {
        IrTransition::Goto { state } => vec![(Node::State(*state), label.to_string())],
        IrTransition::CallThen {
            target,
            exits,
            then,
            ..
        } => {
            let call = format!("{label} call {}", escape(target));
            let mut out: Vec<(Node, String)> = exits
                .iter()
                .enumerate()
                .map(|(k, state)| (Node::State(*state), format!("{call} exit #{k}")))
                .collect();
            if let Some(then) = then {
                let node = match then {
                    IrThen::Goto { state } => Node::State(*state),
                    IrThen::Return => Node::Ret,
                    IrThen::ReturnExit { exit } => Node::RetExit(*exit),
                    IrThen::Stop => Node::Stp,
                    IrThen::Halt => Node::Hlt,
                };
                out.push((node, call));
            }
            out
        }
        IrTransition::TailCall { target } => {
            vec![(Node::Tail, format!("{label} tail {}", escape(target)))]
        }
        IrTransition::Return => vec![(Node::Ret, label.to_string())],
        IrTransition::ReturnExit { exit } => vec![(Node::RetExit(*exit), label.to_string())],
        IrTransition::Stop => vec![(Node::Stp, label.to_string())],
        IrTransition::Halt => vec![(Node::Hlt, label.to_string())],
        IrTransition::TrapRead => vec![(Node::Trap(0), label.to_string())],
        IrTransition::TrapWrite => vec![(Node::Trap(1), label.to_string())],
        IrTransition::TrapContract => vec![(Node::Trap(2), label.to_string())],
    }
}

fn write_cell(c: &IrWrite) -> String {
    match c {
        IrWrite::Keep => "-".into(),
        IrWrite::Index { index } => index.to_string(),
    }
}

fn move_cell(d: &IrMove) -> String {
    match d {
        IrMove::Left => "<",
        IrMove::Right => ">",
        IrMove::Stay => ".",
    }
    .into()
}

/// Assemble a label from already-rendered vectors: `brk `, the pattern,
/// then `w[…]` / `m[…]` when the row writes / moves.
fn label_of(
    debugger: bool,
    pattern: String,
    write: Option<String>,
    moves: Option<String>,
) -> String {
    let mut s = String::new();
    if debugger {
        s.push_str("brk ");
    }
    let _ = write!(s, "[{pattern}]");
    if let Some(w) = write {
        let _ = write!(s, " w[{w}]");
    }
    if let Some(m) = moves {
        let _ = write!(s, " m[{m}]");
    }
    s
}

/// The raw view's row summary: every cell written out.
fn raw_label(r: &IrRule) -> String {
    let pattern: Vec<String> = r
        .pattern
        .iter()
        .map(|c| match c {
            IrCell::Wildcard => "*".into(),
            IrCell::Index { index } => index.to_string(),
        })
        .collect();
    label_of(
        r.debugger,
        pattern.join(","),
        r.write
            .as_ref()
            .map(|w| w.iter().map(write_cell).collect::<Vec<_>>().join(",")),
        r.moves
            .as_ref()
            .map(|m| m.iter().map(move_cell).collect::<Vec<_>>().join(",")),
    )
}

/// The merged view's row summary, in the compact notation — the label
/// every arrow the row draws starts with.
pub fn merged_label(m: &MergedRow) -> String {
    let pattern: Vec<String> = m
        .pattern
        .iter()
        .map(|c| match c {
            ReadCell::Any => "*".into(),
            ReadCell::Set(s) => read_cell_label(s),
        })
        .collect();
    label_of(
        m.debugger,
        compress(&pattern),
        m.write
            .as_ref()
            .map(|w| compress(&w.iter().map(write_cell).collect::<Vec<_>>())),
        m.moves
            .as_ref()
            .map(|d| compress(&d.iter().map(move_cell).collect::<Vec<_>>())),
    )
}

/// Strip the two characters Mermaid quoted labels cannot carry.
fn escape(s: &str) -> String {
    s.replace(['"', '|'], "")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(v: &[u32]) -> String {
        read_cell_label(v)
    }

    #[test]
    fn read_cell_labels() {
        assert_eq!(set(&[4]), "4");
        assert_eq!(set(&[1, 2]), "{1,2}");
        assert_eq!(set(&[1, 2, 3]), "1\u{2013}3");
        assert_eq!(set(&[1, 3]), "{1,3}");
        assert_eq!(
            set(&[1, 2, 3, 7, 9, 10, 11, 12]),
            "{1\u{2013}3,7,9\u{2013}12}"
        );
    }
}
