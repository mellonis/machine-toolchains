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
//! 2. **License-free, PAIR-shaped**: the edit that drops the conjunct
//!    must leave a guard that COMPILES and reads the same. Where a
//!    comparison leans on the conjuncts for its license, the edit drops
//!    every restriction-inert conjunct of that comparison's cluster at
//!    once, and the finding stays quiet unless the pair re-types licensed
//!    with all of them gone — both operands narrowing to one surviving
//!    set, or both sitting on one alphabet
//!    (docs/tmt/language.md (rule guards)) — and every operand keeps the
//!    exact sequence list it read before. A one-sided view of just this
//!    conjunct's own list is not enough: a set can coincide with one
//!    tape's order and still be the pair's only common context, where
//!    removing anything breaks the comparison.
//!
//! # Silent when
//!
//! The cell can carry a symbol outside `S` (the conjunct cuts rows); the
//! joint edit would not compile — a cross-alphabet pair leaning on `in`
//! on both sides reports nothing, whatever the lists do — or would change
//! some operand's sequence; the `in` sits under a `||` (a disjunct
//! filter, never a narrowing); or a cell's or set's members are not
//! computable here (a dead cell, an unresolved set). Every path errs
//! toward silence.
//!
//! No fix ships: dropping a conjunct is the author's edit to make.

use mtc_core::diagnostics::Diagnostic;

use crate::lint::LintContext;
use crate::parser::{CmpOp, GuardBase, GuardCmp, GuardExpr, GuardIn, PatternCell, PatternCellKind};
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
        let tape_alphabets: Vec<String> = world.tapes.iter().map(|t| t.alphabet.clone()).collect();
        for state in &world.states {
            for rule in &state.rules {
                check_rule(rule, &tape_glyphs, &tape_alphabets, out);
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
fn comparisons<'e>(e: &'e GuardExpr, out: &mut Vec<&'e GuardCmp>) {
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
fn needs_sequence(c: &GuardCmp) -> bool {
    !matches!(c.op, CmpOp::Eq | CmpOp::Ne) || c.left.offset != 0 || c.right.offset != 0
}

fn touches(c: &GuardCmp, binding: &str) -> bool {
    operand_names(c).iter().any(|n| n == binding)
}

/// Every binding name a comparison reads.
fn operand_names(c: &GuardCmp) -> Vec<String> {
    [&c.left, &c.right]
        .iter()
        .filter_map(|t| match &t.base {
            GuardBase::Binding(n) => Some(n.name.clone()),
            GuardBase::Lit(_) => None,
        })
        .collect()
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

/// Restriction-inert: every symbol the conjunct's cell can carry is
/// already in the set — `None` when either fact is not computable here.
fn inert(n: &GuardIn, rule: &crate::parser::Rule, tape_glyphs: &[&[String]]) -> Option<bool> {
    let tape = crate::guard::binding_tape(&rule.pattern, &n.binding.name)?;
    let cell = cell_labels(&rule.pattern.cells[tape], tape_glyphs[tape])?;
    let set: Vec<String> = n
        .resolved
        .as_ref()?
        .members
        .iter()
        .map(|m| m.label.clone())
        .collect();
    Some(cell.iter().all(|l| set.contains(l)))
}

/// The labels a conjunct's set names — the sequence its binding's
/// positions count in while it is in effect.
fn set_labels(n: &GuardIn) -> Option<Vec<String>> {
    Some(
        n.resolved
            .as_ref()?
            .members
            .iter()
            .map(|m| m.label.clone())
            .collect(),
    )
}

fn check_rule(
    rule: &crate::parser::Rule,
    tape_glyphs: &[&[String]],
    tape_alphabets: &[String],
    out: &mut Vec<Diagnostic>,
) {
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
            let binding = n.binding.name.clone();
            if crate::guard::binding_tape(&rule.pattern, &binding).is_none() {
                continue;
            }
            // (1) restriction-inert: every symbol the cell can carry is
            // already in the set, so the conjunct never drops a row.
            if inert(n, rule, tape_glyphs) != Some(true) {
                continue;
            }
            // (2) license-free, PAIR-shaped (see the module doc): where
            // comparisons lean on the conjuncts, the edit that must
            // compile drops the whole cluster of inert conjuncts at once,
            // and the finding stays quiet unless that pair re-licenses
            // with every operand's sequence list unchanged.
            if cmps
                .iter()
                .any(|c| needs_sequence(c) && touches(c, &binding))
            {
                // The cluster: every binding linked to this one by
                // sequence-needing comparisons in this chain, transitively.
                let mut cluster = vec![binding.clone()];
                loop {
                    let mut grew = false;
                    for c in &cmps {
                        if !needs_sequence(c) {
                            continue;
                        }
                        let names = operand_names(c);
                        if names.iter().any(|n| cluster.contains(n)) {
                            for name in names {
                                if !cluster.contains(&name) {
                                    cluster.push(name);
                                    grew = true;
                                }
                            }
                        }
                    }
                    if !grew {
                        break;
                    }
                }
                // The joint edit: every restriction-inert conjunct of the
                // cluster goes at once (this one is, per (1)); one that
                // cuts rows, or whose facts are not computable here,
                // survives and keeps its narrowing in place.
                let mut kept: Vec<&GuardIn> = Vec::new();
                for m in &ins {
                    if !cluster.contains(&m.binding.name) {
                        continue;
                    }
                    // one that cuts rows, or whose facts are not
                    // computable here, survives and keeps its narrowing in
                    // place
                    if inert(m, rule, tape_glyphs) != Some(true) {
                        kept.push(*m);
                    }
                }
                // What a binding's positions count AFTER the edit — its
                // surviving narrowing's member order, else its tape's
                // alphabet — with the tape's alphabet name beside it.
                let after_ctx = |name: &str| -> Option<(Option<String>, Vec<String>, &str)> {
                    let tape = crate::guard::binding_tape(&rule.pattern, name)?;
                    if let Some(m) = kept.iter().find(|m| m.binding.name == name) {
                        let r = m.resolved.as_ref()?;
                        return Some((
                            Some(r.set.clone()),
                            r.members.iter().map(|x| x.label.clone()).collect(),
                            tape_alphabets[tape].as_str(),
                        ));
                    }
                    Some((
                        None,
                        tape_glyphs[tape].to_vec(),
                        tape_alphabets[tape].as_str(),
                    ))
                };
                // What its positions counted before: the chain conjunct's
                // member order, else the tape's — unchanged if nothing of
                // its was dropped.
                let before_labels = |name: &str| -> Option<Vec<String>> {
                    match ins
                        .iter()
                        .find(|m| m.binding.name == name)
                        .map(|m| set_labels(m))
                    {
                        Some(labels) => labels,
                        None => {
                            let tape = crate::guard::binding_tape(&rule.pattern, name)?;
                            Some(tape_glyphs[tape].to_vec())
                        }
                    }
                };
                let mut ok = true;
                'pairs: for c in &cmps {
                    // the whole cluster, not just this binding
                    if !needs_sequence(c) || !operand_names(c).iter().any(|n| cluster.contains(n)) {
                        continue;
                    }
                    let mut sides = Vec::new();
                    for t in [&c.left, &c.right] {
                        match &t.base {
                            GuardBase::Binding(b) => {
                                let Some((set, labels, alphabet)) = after_ctx(&b.name) else {
                                    ok = false;
                                    break 'pairs;
                                };
                                if before_labels(&b.name) != Some(labels.clone()) {
                                    // the edit changes what some operand
                                    // counts: not the same guard
                                    ok = false;
                                    break 'pairs;
                                }
                                sides.push((set, alphabet));
                            }
                            // A literal rides its partner's sequence; its
                            // membership holds either way, since a set
                            // lies within its tape's alphabet.
                            GuardBase::Lit(_) => {}
                        }
                    }
                    // The pair re-types licensed: typing's one-common-
                    // sequence rule, names and all.
                    if sides.len() == 2 {
                        let licensed = match (&sides[0].0, &sides[1].0) {
                            (Some(a), Some(b)) => a == b,
                            (None, None) => sides[0].1 == sides[1].1,
                            // one narrowed, one bare: never a license
                            _ => false,
                        };
                        if !licensed {
                            ok = false;
                            break 'pairs;
                        }
                    }
                }
                if !ok {
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
