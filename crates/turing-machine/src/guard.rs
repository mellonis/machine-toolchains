//! Rule-guard typing — the cone-local context rules behind `where { … }`
//! (docs/tmt/language.md (rule guards)).
//!
//! One guard carries two value domains: an offset-free `==`/`!=` compares
//! **glyph identity** and needs no sequence at all; anything order-shaped
//! (a `<`/`<=`/`>`/`>=`, or a comparison with a constant offset) counts
//! **positions** in one common sequence. A binding's default sequence is
//! its tape's alphabet in declared order; an `in S` conjunct narrows it to
//! `S`'s member order — within its cone only: the subtree of the maximal
//! upward chain of `&&` nodes from the conjunct, which is to say the whole
//! conjunction that conjunct sits in, `||` operands under it included, and
//! nothing outside (a sibling `||` branch knows nothing of the narrowing —
//! "we don't know why the conjunction is not true"). The author shares a
//! narrowing across branches by hoisting it into the conjunction and
//! parenthesizing the `||`.
//!
//! The refusals, all at the offending operand and never the whole guard:
//! `GuardOrderContext` (a position comparison with no one common sequence —
//! including a binding narrowed to two different sets in one cone, where
//! its position would have to live in both), `GuardUnknownBinding`,
//! `GuardSetOutsideAlphabet` (`in S` where `S` is not within the tape's
//! alphabet), and `GuardConstantComparison` (no binding on either side —
//! nothing varies per row).

use mtc_core::diagnostics::Span;

use crate::compiler::{CompileError, CompileErrorKind};
use crate::parser::{
    CmpOp, Guard, GuardBase, GuardCmp, GuardExpr, GuardIn, GuardTerm, Rule, SetCellMembers, SymLit,
};

/// What guard typing needs about the enclosing world, in tape order: each
/// tape's alphabet glyph labels (`tape_labels`, position order — what a
/// binding's positions count against) and that alphabet's name
/// (`tape_alphabets`, message text). Built at resolution from the world's
/// own resolved tape table; row evaluation takes the same value.
pub struct GuardContext {
    pub tape_labels: Vec<Vec<String>>,
    pub tape_alphabets: Vec<String>,
}

/// One `in S` narrowing in effect at an occurrence.
#[derive(Clone)]
struct Narrowing {
    binding: String,
    /// The set name as written — what a refusal says.
    written: String,
    /// The set's resolved identity — what sequence-equality tests.
    resolved: String,
    /// The set's members, in the set's own order: the sequence itself.
    members: Vec<String>,
}

/// The one sequence context an operand's positions live in at one
/// occurrence.
#[derive(Clone)]
enum Seq {
    Alphabet(usize),
    Set(Narrowing),
}

impl Seq {
    fn label(&self, ctx: &GuardContext) -> String {
        match self {
            Seq::Alphabet(t) => ctx.tape_alphabets[*t].clone(),
            Seq::Set(n) => n.written.clone(),
        }
    }

    fn holds(&self, ctx: &GuardContext, label: &str) -> bool {
        match self {
            Seq::Alphabet(t) => ctx.tape_labels[*t].iter().any(|l| l == label),
            Seq::Set(n) => n.members.iter().any(|l| l == label),
        }
    }
}

/// Type-check one rule's guard and fill every `in` conjunct's resolution.
/// `resolve_set` is the world's own set lookup behind a closure (this
/// module keeps no scope machinery); it runs exactly once per conjunct,
/// and its own refusals — an unknown or wrong-kind set name — surface
/// before any of this module's.
pub(crate) fn check_rule_guard(
    rule: &mut Rule,
    ctx: &GuardContext,
    mut resolve_set: impl FnMut(&str, Span) -> Result<SetCellMembers, CompileError>,
) -> Result<(), CompileError> {
    // The pattern is the world's own positional fill by now: cells[i]
    // speaks of tape i.
    let tape_of = |name: &str| binding_tape(&rule.pattern, name);
    match &mut rule.guard {
        None => Ok(()),
        Some(g) => type_expr(
            &mut g.expr,
            &mut Vec::new(),
            ctx,
            &tape_of,
            &mut resolve_set,
        ),
    }
}

/// Resolve one `in` conjunct (idempotently — the walk can reach a node
/// once) and answer the narrowing it contributes.
fn resolve_in_cell(
    in_node: &mut GuardIn,
    ctx: &GuardContext,
    tape_of: &dyn Fn(&str) -> Option<usize>,
    resolve_set: &mut dyn FnMut(&str, Span) -> Result<SetCellMembers, CompileError>,
) -> Result<Narrowing, CompileError> {
    let tape = tape_of(&in_node.binding.name).ok_or(CompileError {
        span: in_node.binding.span,
        kind: CompileErrorKind::GuardUnknownBinding {
            name: in_node.binding.name.clone(),
        },
    })?;
    if in_node.resolved.is_none() {
        let set = resolve_set(&in_node.set.name, in_node.set.span)?;
        // `S ⊆ alphabet`: every member must be a symbol the binding's own
        // tape can hold (any other spelling is a name the row can never
        // carry).
        if set
            .members
            .iter()
            .any(|m| !ctx.tape_labels[tape].contains(&m.label))
        {
            return Err(CompileError {
                span: in_node.set.span,
                kind: CompileErrorKind::GuardSetOutsideAlphabet {
                    // As written: the refusal names what the author typed.
                    set: in_node.set.name.clone(),
                    alphabet: ctx.tape_alphabets[tape].clone(),
                },
            });
        }
        in_node.resolved = Some(set);
    }
    let resolved = in_node.resolved.as_ref().expect("just resolved");
    Ok(Narrowing {
        binding: in_node.binding.name.clone(),
        written: in_node.set.name.clone(),
        resolved: resolved.set.clone(),
        members: resolved.members.iter().map(|m| m.label.clone()).collect(),
    })
}

/// Cone-local typing over the guard tree: every occurrence is typed against
/// the narrowings in effect in its OWN cone.
fn type_expr(
    expr: &mut GuardExpr,
    active: &mut Vec<Narrowing>,
    ctx: &GuardContext,
    tape_of: &dyn Fn(&str) -> Option<usize>,
    resolve_set: &mut dyn FnMut(&str, Span) -> Result<SetCellMembers, CompileError>,
) -> Result<(), CompileError> {
    match expr {
        GuardExpr::In(in_node) => {
            resolve_in_cell(in_node, ctx, tape_of, resolve_set)?;
            Ok(())
        }
        GuardExpr::Cmp(cmp) => type_cmp(cmp, active, ctx, tape_of),
        GuardExpr::Or(a, b) => {
            // A sibling branch knows nothing of the other's narrowing: each
            // keeps the cone's own scope, unchanged.
            type_expr(a, active, ctx, tape_of, resolve_set)?;
            type_expr(b, active, ctx, tape_of, resolve_set)
        }
        GuardExpr::And(a, b) => {
            // This conjunction's own narrowings — `in` conjuncts joined
            // through `&&` only — narrow every occurrence under this
            // conjunction, `||` operands included, and nothing else.
            let mut own = Vec::new();
            collect_chain_narrowings(a, &mut own, ctx, tape_of, resolve_set)?;
            collect_chain_narrowings(b, &mut own, ctx, tape_of, resolve_set)?;
            let mut scope = active.clone();
            scope.extend(own);
            let mut conj = Vec::new();
            flatten_chain(&mut conj, a, b);
            for c in conj {
                type_expr(c, &mut scope, ctx, tape_of, resolve_set)?;
            }
            Ok(())
        }
    }
}

/// The conjuncts of one `&&` chain, nested `&&` transparent.
fn flatten_chain<'e>(
    into: &mut Vec<&'e mut GuardExpr>,
    a: &'e mut GuardExpr,
    b: &'e mut GuardExpr,
) {
    match a {
        GuardExpr::And(x, y) => flatten_chain(into, x, y),
        other => into.push(other),
    }
    match b {
        GuardExpr::And(x, y) => flatten_chain(into, x, y),
        other => into.push(other),
    }
}

/// Narrowings contributed to the cone by one side of an `&&` chain: the
/// `in` conjuncts reachable crossing only `&&` — an `in` under a `||` has
/// its own branch cone and is typed when that branch is visited.
fn collect_chain_narrowings(
    e: &mut GuardExpr,
    out: &mut Vec<Narrowing>,
    ctx: &GuardContext,
    tape_of: &dyn Fn(&str) -> Option<usize>,
    resolve_set: &mut dyn FnMut(&str, Span) -> Result<SetCellMembers, CompileError>,
) -> Result<(), CompileError> {
    match e {
        GuardExpr::In(n) => {
            let narrowing = resolve_in_cell(n, ctx, tape_of, resolve_set)?;
            out.push(narrowing);
            Ok(())
        }
        GuardExpr::And(a, b) => {
            collect_chain_narrowings(a, out, ctx, tape_of, resolve_set)?;
            collect_chain_narrowings(b, out, ctx, tape_of, resolve_set)
        }
        // Crossing a `||` stops the cone.
        GuardExpr::Or(_, _) | GuardExpr::Cmp(_) => Ok(()),
    }
}

/// One operand's position sequence at this occurrence.
fn seq_of(
    term: &GuardTerm,
    active: &[Narrowing],
    _ctx: &GuardContext,
    tape_of: &dyn Fn(&str) -> Option<usize>,
) -> Result<Seq, CompileError> {
    let GuardBase::Binding(n) = &term.base else {
        // A literal has no sequence of its own: it rides the other side's.
        unreachable!("seq_of is called on bindings only");
    };
    let tape = tape_of(&n.name).ok_or(CompileError {
        span: n.span,
        kind: CompileErrorKind::GuardUnknownBinding {
            name: n.name.clone(),
        },
    })?;
    let mut narrows = active.iter().filter(|x| x.binding == n.name);
    let first = narrows.next();
    if let Some(other) = narrows.find(|x| x.resolved != first.expect("one narrowing").resolved) {
        // Two different sets narrowing one binding in one cone: its
        // position would have to live in both sequences at once. Identity
        // comparisons never get here — they need no sequence.
        return Err(CompileError {
            span: term.span,
            kind: CompileErrorKind::GuardOrderContext {
                left: first.expect("one narrowing").written.clone(),
                right: other.written.clone(),
            },
        });
    }
    Ok(match first {
        Some(n) => Seq::Set(n.clone()),
        None => Seq::Alphabet(tape),
    })
}

fn lit_label(sym: &SymLit) -> String {
    match sym {
        SymLit::Glyph { value, .. } => value.clone(),
        // The expander's canon (`expand::glyph_label`): a number literal's
        // label is its decimal value, not its written spelling — the label
        // space guards compare in is the alphabet's.
        SymLit::Number { value, .. } => value.to_string(),
    }
}

/// The tape a binding sits on: wherever its `as name` is, in the
/// world's own positional pattern fill (a name on two cells resolves to
/// the first — duplicates are their own earlier refusal).
fn binding_tape(pattern: &crate::parser::Pattern, name: &str) -> Option<usize> {
    pattern
        .cells
        .iter()
        .position(|c| c.binding.as_ref().is_some_and(|b| b.name == name))
}

fn type_cmp(
    cmp: &GuardCmp,
    active: &[Narrowing],
    ctx: &GuardContext,
    tape_of: &dyn Fn(&str) -> Option<usize>,
) -> Result<(), CompileError> {
    // A comparison with no binding at all varies with nothing a rule could
    // ever match differently.
    if matches!(cmp.left.base, GuardBase::Lit(_)) && matches!(cmp.right.base, GuardBase::Lit(_)) {
        return Err(CompileError {
            span: cmp.span,
            kind: CompileErrorKind::GuardConstantComparison,
        });
    }

    // Offset-free `==`/`!=` is glyph identity — no sequence, no license.
    // Checked before the narrowing-clash test: identity deliberately lives
    // under two different narrowings ("the restrictions simply
    // intersect").
    let identity =
        matches!(cmp.op, CmpOp::Eq | CmpOp::Ne) && cmp.left.offset == 0 && cmp.right.offset == 0;
    if identity {
        // Still every name must be a binding.
        for t in [&cmp.left, &cmp.right] {
            if let GuardBase::Binding(n) = &t.base
                && tape_of(&n.name).is_none()
            {
                return Err(CompileError {
                    span: n.span,
                    kind: CompileErrorKind::GuardUnknownBinding {
                        name: n.name.clone(),
                    },
                });
            }
        }
        return Ok(());
    }

    // Everything else counts positions: the pair needs ONE common
    // sequence — both bindings of one alphabet, or both narrowed in this
    // cone to one set; a literal joins the other side's sequence and must
    // be a member of it.
    let ls = match &cmp.left.base {
        GuardBase::Binding(_) => Some(seq_of(&cmp.left, active, ctx, tape_of)?),
        GuardBase::Lit(_) => None,
    };
    let rs = match &cmp.right.base {
        GuardBase::Binding(_) => Some(seq_of(&cmp.right, active, ctx, tape_of)?),
        GuardBase::Lit(_) => None,
    };
    match (&ls, &rs) {
        (Some(l), Some(r)) => {
            let licensed = match (l, r) {
                (Seq::Alphabet(a), Seq::Alphabet(b)) => {
                    ctx.tape_alphabets[*a] == ctx.tape_alphabets[*b]
                }
                (Seq::Set(a), Seq::Set(b)) => a.resolved == b.resolved && a.members == b.members,
                _ => false,
            };
            if licensed {
                Ok(())
            } else {
                Err(CompileError {
                    span: cmp.span,
                    kind: CompileErrorKind::GuardOrderContext {
                        left: l.label(ctx),
                        right: r.label(ctx),
                    },
                })
            }
        }
        (Some(s), None) => lic_lit(&cmp.right, s, ctx, cmp),
        (None, Some(s)) => lic_lit(&cmp.left, s, ctx, cmp),
        (None, None) => unreachable!("the both-literal case returned above"),
    }
}

/// A literal half of a position comparison: its label takes the other
/// side's sequence and must be a member of it.
fn lic_lit(
    lit_term: &GuardTerm,
    seq: &Seq,
    ctx: &GuardContext,
    cmp: &GuardCmp,
) -> Result<(), CompileError> {
    let l = match &lit_term.base {
        GuardBase::Lit(sym) => lit_label(sym),
        GuardBase::Binding(_) => unreachable!("the caller checked which side is the literal"),
    };
    if seq.holds(ctx, &l) {
        Ok(())
    } else {
        Err(CompileError {
            span: cmp.span,
            kind: CompileErrorKind::GuardOrderContext {
                left: l,
                right: seq.label(ctx),
            },
        })
    }
}

// ---------------------------------------------------------------------------
// Evaluation — the row filter `expand` runs. The cone bookkeeping mirrors
// typing's exactly: an occurrence counts positions in its own cone's
// sequence, a value off that sequence matches nothing (the same off-line
// rule as `y - 1` for the sequence's first member).
// ---------------------------------------------------------------------------

use std::collections::HashMap;

use crate::expand::BoundVal;

/// The filter `expand` runs per candidate row: does the guard keep this
/// one? An unguarded rule keeps everything.
pub(crate) fn row_matches(rule: &Rule, env: &HashMap<&str, &BoundVal>, ctx: &GuardContext) -> bool {
    let Some(g) = &rule.guard else {
        return true;
    };
    let tape_of = |name: &str| binding_tape(&rule.pattern, name);
    eval_expr(&g.expr, &mut Vec::new(), env, ctx, &tape_of)
}

/// The conjuncts of one `&&` chain, nested `&&` transparent (shared-refs
/// variant for evaluation — typing's mutable one rebuilds `In`
/// resolutions).
fn flatten_chain_ref<'e>(into: &mut Vec<&'e GuardExpr>, a: &'e GuardExpr, b: &'e GuardExpr) {
    match a {
        GuardExpr::And(x, y) => flatten_chain_ref(into, x, y),
        other => into.push(other),
    }
    match b {
        GuardExpr::And(x, y) => flatten_chain_ref(into, x, y),
        other => into.push(other),
    }
}

/// The narrowings one side of an `&&` chain contributes at evaluation:
/// `in` conjuncts reachable crossing only `&&`, reconstructed from the
/// RESOLUTION typing already filled in (an `in` under a `||` stays in its
/// own branch).
fn narrows_at(e: &GuardExpr, out: &mut Vec<Narrowing>) {
    match e {
        GuardExpr::In(n) => {
            if let Some(r) = &n.resolved {
                out.push(Narrowing {
                    binding: n.binding.name.clone(),
                    written: n.set.name.clone(),
                    resolved: r.set.clone(),
                    members: r.members.iter().map(|m| m.label.clone()).collect(),
                });
            }
        }
        GuardExpr::And(a, b) => {
            narrows_at(a, out);
            narrows_at(b, out);
        }
        GuardExpr::Or(_, _) | GuardExpr::Cmp(_) => {}
    }
}

fn eval_expr(
    e: &GuardExpr,
    active: &mut Vec<Narrowing>,
    env: &HashMap<&str, &BoundVal>,
    ctx: &GuardContext,
    tape_of: &dyn Fn(&str) -> Option<usize>,
) -> bool {
    match e {
        GuardExpr::In(n) => {
            // Restriction: the row's glyph must be one of the set's.
            let Some(v) = env.get(n.binding.name.as_str()) else {
                return false;
            };
            n.resolved
                .as_ref()
                .is_some_and(|r| r.members.iter().any(|m| m.label == v.glyph))
        }
        GuardExpr::Cmp(c) => eval_cmp(c, active, env, ctx, tape_of),
        GuardExpr::Or(a, b) => {
            eval_expr(a, active, env, ctx, tape_of) || eval_expr(b, active, env, ctx, tape_of)
        }
        GuardExpr::And(a, b) => {
            // A conjunction's own narrowings scope every conjunct under it,
            // `||` operands included — and then every conjunct must hold.
            let mut own = active.clone();
            narrows_at(a, &mut own);
            narrows_at(b, &mut own);
            let mut conj = Vec::new();
            flatten_chain_ref(&mut conj, a, b);
            conj.into_iter()
                .all(|c| eval_expr(c, &mut own, env, ctx, tape_of))
        }
    }
}

/// A binding operand's sequence at this occurrence — `None` for a literal,
/// which rides the other side's.
fn seq_at(
    term: &GuardTerm,
    active: &[Narrowing],
    _ctx: &GuardContext,
    tape_of: &dyn Fn(&str) -> Option<usize>,
) -> Option<Seq> {
    let GuardBase::Binding(n) = &term.base else {
        return None;
    };
    let tape = tape_of(&n.name)?;
    let mut narrows = active.iter().filter(|x| x.binding == n.name);
    Some(match narrows.next() {
        // Two narrowings in one cone never reach evaluation — typing
        // refused that shape wherever a position was needed; the first is
        // the sequence here by construction.
        Some(x) => Seq::Set(x.clone()),
        None => Seq::Alphabet(tape),
    })
}

fn pos_in(seq: &Seq, ctx: &GuardContext, label: &str) -> Option<i64> {
    let where_ = |labels: &[String]| labels.iter().position(|l| l == label).map(|i| i as i64);
    match seq {
        Seq::Alphabet(t) => where_(&ctx.tape_labels[*t]),
        Seq::Set(n) => where_(&n.members),
    }
}

fn eval_cmp(
    c: &GuardCmp,
    active: &[Narrowing],
    env: &HashMap<&str, &BoundVal>,
    ctx: &GuardContext,
    tape_of: &dyn Fn(&str) -> Option<usize>,
) -> bool {
    let label = |t: &GuardTerm| -> Option<String> {
        match &t.base {
            GuardBase::Binding(n) => env.get(n.name.as_str()).map(|v| v.glyph.clone()),
            GuardBase::Lit(sym) => Some(lit_label(sym)),
        }
    };
    let Some(l) = label(&c.left) else {
        return false;
    };
    let Some(r) = label(&c.right) else {
        return false;
    };

    // Offset-free `==`/`!=` is glyph identity — the label space, whatever
    // sequences the cone carries.
    if matches!(c.op, CmpOp::Eq | CmpOp::Ne) && c.left.offset == 0 && c.right.offset == 0 {
        return (l == r) == matches!(c.op, CmpOp::Eq);
    }

    // Position domain: each binding operand counts in its OWN sequence, a
    // literal in the pair's one effective sequence. A value off its
    // sequence matches nothing.
    let ls = seq_at(&c.left, active, ctx, tape_of);
    let rs = seq_at(&c.right, active, ctx, tape_of);
    let (lp, rp) = match (&ls, &rs) {
        (Some(a), Some(b)) => {
            let (Some(lp), Some(rp)) = (pos_in(a, ctx, &l), pos_in(b, ctx, &r)) else {
                return false;
            };
            (lp, rp)
        }
        (lseq, rseq) => {
            let eff = lseq.as_ref().or(rseq.as_ref());
            let Some(eff) = eff else {
                return false;
            };
            let (Some(lp), Some(rp)) = (pos_in(eff, ctx, &l), pos_in(eff, ctx, &r)) else {
                return false;
            };
            (lp, rp)
        }
    };
    let l = lp + c.left.offset;
    let r = rp + c.right.offset;
    match c.op {
        CmpOp::Eq => l == r,
        CmpOp::Ne => l != r,
        CmpOp::Lt => l < r,
        CmpOp::Le => l <= r,
        CmpOp::Gt => l > r,
        CmpOp::Ge => l >= r,
    }
}

// ---------------------------------------------------------------------------
// Printing — the canonical render the header printer puts in a `.tmh`'s
// rule text (docs/tmt/language.md (rule guards); headers print semantics,
// fmt prints the written tokens). Written parentheses carry no node, so
// the renderer reinserts exactly the ones PRECEDENCE needs: a `||` column
// under `&&` keeps its parens or the print would re-parse through a
// different cone; a redundant pair is dropped — one spelling per tree.
// ---------------------------------------------------------------------------

/// The guard's canonical text between `where {` and `}`.
pub(crate) fn guard_text(g: &Guard) -> String {
    expr_text(&g.expr, 0)
}

/// One operand's canonical text: the base's written spelling, its
/// constant offset spelled out (`y - 1`), bare at offset 0.
fn term_text(t: &GuardTerm) -> String {
    let base = match &t.base {
        GuardBase::Binding(n) => n.name.clone(),
        GuardBase::Lit(s) => crate::patterns::written_spelling(s),
    };
    match t.offset {
        0 => base,
        n if n < 0 => format!("{base} - {}", -n),
        n => format!("{base} + {n}"),
    }
}

fn op_text(op: CmpOp) -> &'static str {
    match op {
        CmpOp::Eq => "==",
        CmpOp::Ne => "!=",
        CmpOp::Lt => "<",
        CmpOp::Le => "<=",
        CmpOp::Gt => ">",
        CmpOp::Ge => ">=",
    }
}

/// Precedence-climbed: `&&` binds tighter than `||` (the grammar has
/// nothing else), and a subexpression PRECEDENCE would re-bind differently
/// takes parentheses.
fn expr_text(e: &GuardExpr, min_prec: u8) -> String {
    let binary = |a: &GuardExpr, b: &GuardExpr, op: &str, prec: u8| {
        let t = format!("{} {op} {}", expr_text(a, prec), expr_text(b, prec));
        if prec < min_prec { format!("({t})") } else { t }
    };
    match e {
        GuardExpr::Cmp(c) => format!(
            "{} {} {}",
            term_text(&c.left),
            op_text(c.op),
            term_text(&c.right)
        ),
        GuardExpr::In(n) => format!("{} in {}", n.binding.name, n.set.name),
        GuardExpr::And(a, b) => binary(a, b, "&&", 2),
        GuardExpr::Or(a, b) => binary(a, b, "||", 1),
    }
}

/// Every `in S` conjunct of a guard, `&&`- and `||`-nested alike: the set
/// names a header must print alongside the body that names them, and the
/// names a digest's named-content appendix must cover — a guard names a
/// set by NAME only, so member drift moves nothing without these.
pub(crate) fn in_conjuncts(g: &Guard) -> Vec<&GuardIn> {
    fn walk<'e>(e: &'e GuardExpr, out: &mut Vec<&'e GuardIn>) {
        match e {
            GuardExpr::In(n) => out.push(n),
            GuardExpr::And(a, b) | GuardExpr::Or(a, b) => {
                walk(a, out);
                walk(b, out);
            }
            GuardExpr::Cmp(_) => {}
        }
    }
    let mut out = Vec::new();
    walk(&g.expr, &mut out);
    out
}
