//! `redundant-guard-narrowing`: a `where` guard's `in S` conjunct that
//! narrows nothing — every symbol the binding's pattern cell can carry is
//! already in `S`, and the guard reads the same without the conjunct
//! (docs/tmt/language.md (rule guards)). An author writing `in` for
//! documentation keeps the finding off with `--allow`/`lint.allow`, the
//! shared allow namespace every rule lives in.
//!
//! # What it sees
//!
//! The NARROWING positions — the `in` conjuncts of an `&&` chain
//! (the ones typing scopes over the chain's subtree, `||` operands
//! included), and a lone `where { x in S }`. Two halves, BOTH required:
//!
//! 1. **Restriction-inert**: the binding's cell can only carry symbols
//!    already in `S` (the same facts `binding-product-threshold::factor`
//!    reads), so the conjunct never drops a row;
//! 2. **License-free**: every order/offset comparison of the binding in
//!    the chain still reads the same with the conjunct removed — with it,
//!    the binding counts positions in `S`'s member order; without it, in
//!    whatever is left (its own tape's alphabet, or another narrowing).
//!    When those two lists differ, the conjunct is the pair's common
//!    context and MUST stay.
//!
//! # Silent when
//!
//! The cell can carry a symbol outside `S` (the conjunct cuts rows); the
//! removed conjunct would change a comparison's operand sequence —
//! including a conjunct that is restriction-inert on its own tape but part
//! of a cross-alphabet pair's one common context; the `in` sits under a
//! `||` (a disjunct filter, never a narrowing); or a cell's or set's
//! members are not computable here (a dead cell, an unresolved set).
//! Every path errs toward silence.
//!
//! No fix ships: dropping a conjunct is the author's edit to make.

use mtc_core::diagnostics::Diagnostic;

use crate::lint::LintContext;
use crate::parser::{CmpOp, GuardBase, GuardExpr, GuardIn, PatternCell, PatternCellKind};
use crate::patterns::{declared_range, glyph_label};

pub(crate) fn check(ctx: &LintContext, out: &mut Vec<Diagnostic>) {
    for world in &ctx.resolved.worlds {
        let Some(tape_glyphs): Option<Vec<&[String]>> = world
            .tapes
            .iter()
            .map(|t| crate::lint::alphabet_glyphs(ctx.resolved, &t.alphabet))
            .collect()
        else {
            continue;
        };
        for state in &world.states {
            for rule in &state.rules {
                check_rule(rule, &tape_glyphs, out);
            }
        }
    }
}

/// The conjuncts of one `&&` chain (nested `&&` transparent) — the
/// NARROWING positions: typing collects `in` conjuncts reachable crossing
/// only `&&`, and scopes them over the chain's subtree, `||` operands
/// included. `and_clusters` gathers one member list per chain.
fn and_clusters<'e>(e: &'e GuardExpr, out: &mut Vec<Vec<&'e GuardExpr>>) {
    match e {
        GuardExpr::And(_, _) => {
            let mut chain = Vec::new();
            flatten_and(&mut chain, e);
            for m in &chain {
                if matches!(m, GuardExpr::Or(_, _)) {
                    and_clusters(m, out);
                }
            }
            out.push(chain);
        }
        GuardExpr::Or(a, b) => {
            and_clusters(a, out);
            and_clusters(b, out);
        }
        // A bare `in` or comparison outside any chain is no narrowing: an
        // `in` under a `||` is a disjunct filter, whose removal would
        // rewrite the disjunction.
        GuardExpr::In(_) | GuardExpr::Cmp(_) => {}
    }
}

fn flatten_and<'e>(into: &mut Vec<&'e GuardExpr>, e: &'e GuardExpr) {
    match e {
        GuardExpr::And(a, b) => {
            flatten_and(into, a);
            flatten_and(into, b);
        }
        other => into.push(other),
    }
}

/// Every comparison under `e`, `&&`/`||` transparent.
fn comparisons<'e>(e: &'e GuardExpr, out: &mut Vec<&'e crate::parser::GuardCmp>) {
    match e {
        GuardExpr::Cmp(c) => out.push(c),
        GuardExpr::And(a, b) | GuardExpr::Or(a, b) => {
            comparisons(a, out);
            comparisons(b, out);
        }
        GuardExpr::In(_) => {}
    }
}

/// Whether the comparison counts POSITIONS (anything but an offset-free
/// `==`/`!=`, whose glyph identity needs no sequence at all) — the same
/// gate `crate::guard`'s typing uses.
fn needs_sequence(c: &crate::parser::GuardCmp) -> bool {
    !matches!(c.op, CmpOp::Eq | CmpOp::Ne) || c.left.offset != 0 || c.right.offset != 0
}

fn touches(c: &crate::parser::GuardCmp, binding: &str) -> bool {
    [&c.left, &c.right]
        .iter()
        .any(|t| matches!(&t.base, GuardBase::Binding(n) if n.name == binding))
}

/// The labels one pattern cell can produce on its tape, mirroring
/// `binding-product-threshold::factor`: `None` when they are not
/// computable here (a single off its tape, a range with no walk, an
/// unresolved set cell) — silence beats a guess in every such case.
fn cell_labels(cell: &PatternCell, tape_glyphs: &[String]) -> Option<Vec<String>> {
    match &cell.kind {
        PatternCellKind::Wildcard => Some(tape_glyphs.to_vec()),
        PatternCellKind::Single(s) => {
            let label = glyph_label(s);
            tape_glyphs.contains(&label).then(|| vec![label])
        }
        PatternCellKind::Range { lo, hi } => {
            declared_range(lo, hi, tape_glyphs).ok().map(|g| g.to_vec())
        }
        PatternCellKind::SetRef {
            resolved: Some(r), ..
        } => Some(
            r.members
                .iter()
                .map(|m| m.label.clone())
                .filter(|l| tape_glyphs.contains(l))
                .collect(),
        ),
        PatternCellKind::SetRef { resolved: None, .. } => None,
    }
}

fn check_rule(rule: &crate::parser::Rule, tape_glyphs: &[&[String]], out: &mut Vec<Diagnostic>) {
    let Some(g) = &rule.guard else {
        return;
    };
    let mut clusters: Vec<Vec<&GuardExpr>> = Vec::new();
    and_clusters(&g.expr, &mut clusters);
    // A lone `where { x in S }` is its own degenerate narrowing: the
    // conjunct is the whole guard, and inert there too means nothing.
    if matches!(g.expr, GuardExpr::In(_)) {
        clusters.push(vec![&g.expr]);
    }
    for chain in &clusters {
        let ins: Vec<&GuardIn> = chain
            .iter()
            .filter_map(|m| match m {
                GuardExpr::In(n) => Some(n),
                _ => None,
            })
            .collect();
        let mut cmps = Vec::new();
        for m in chain {
            comparisons(m, &mut cmps);
        }
        for n in &ins {
            let binding = &n.binding.name;
            let Some(r) = &n.resolved else {
                continue;
            };
            let Some(tape) = crate::guard::binding_tape(&rule.pattern, binding) else {
                continue;
            };
            // (1) restriction-inert: every symbol the cell can carry is
            // already in the set, so the conjunct never drops a row.
            let Some(cell_labels) = cell_labels(&rule.pattern.cells[tape], tape_glyphs[tape])
            else {
                continue;
            };
            let set_labels: Vec<String> = r.members.iter().map(|m| m.label.clone()).collect();
            if !cell_labels.iter().all(|l| set_labels.contains(l)) {
                continue;
            }
            // (2) license-free: a position comparison of this binding
            // reads its operand's sequence; the conjunct survives whenever
            // removing it would change that sequence's list.
            let leans = cmps
                .iter()
                .any(|c| needs_sequence(c) && touches(c, binding));
            if leans {
                let others: Vec<&&GuardIn> = ins
                    .iter()
                    .filter(|m| m.binding.name == *binding && m.span != n.span)
                    .collect();
                let seq_after: Option<Vec<String>> = match others.len() {
                    0 => Some(tape_glyphs[tape].to_vec()),
                    1 => match others[0].resolved.as_ref() {
                        Some(o) => Some(o.members.iter().map(|m| m.label.clone()).collect()),
                        None => continue,
                    },
                    // Two rival narrowings on one binding: typing refused
                    // every position this pair could read. Silence.
                    _ => continue,
                };
                if seq_after != Some(set_labels.clone()) {
                    continue;
                }
            }
            out.push(Diagnostic {
                code: "redundant-guard-narrowing",
                span: n.span,
                message: format!(
                    "this `in` narrows nothing here: the cell can only carry symbols already \
                     in `{}`, and the guard reads the same without it",
                    n.set.name
                ),
                fix: None,
            });
        }
    }
}
