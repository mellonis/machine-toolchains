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
    CmpOp, GuardBase, GuardCmp, GuardExpr, GuardIn, GuardTerm, Rule, SetCellMembers, SymLit,
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
    // speaks of tape i. A binding's tape is wherever `as name` sits; a name
    // on two cells resolves to the first (duplicates are their own earlier
    // refusal).
    let tape_of = |name: &str| -> Option<usize> {
        rule.pattern
            .cells
            .iter()
            .position(|c| c.binding.as_ref().is_some_and(|b| b.name == name))
    };
    match &mut rule.guard {
        None => Ok(()),
        Some(g) => type_expr(&mut g.expr, &mut Vec::new(), ctx, &tape_of, &mut resolve_set),
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
            .any(|m| !ctx.tape_labels[tape].iter().any(|l| *l == m.label))
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
fn flatten_chain<'e>(into: &mut Vec<&'e mut GuardExpr>, a: &'e mut GuardExpr, b: &'e mut GuardExpr) {
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
    ctx: &GuardContext,
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
        SymLit::Number { written, .. } => written.clone(),
    }
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
    let identity = matches!(cmp.op, CmpOp::Eq | CmpOp::Ne)
        && cmp.left.offset == 0
        && cmp.right.offset == 0;
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
                (Seq::Set(a), Seq::Set(b)) => {
                    a.resolved == b.resolved && a.members == b.members
                }
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
