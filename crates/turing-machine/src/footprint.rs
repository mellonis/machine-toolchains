//! Write footprints: for every world, per tape, the set of symbol indices its
//! body may ever write — its own rows' write cells plus everything the calls
//! it makes can write back through their bindings.
//!
//! # The soundness contract: over-approximation
//!
//! An inferred set is a SUPERSET of what a run can actually write:
//! `inferred ⊇ actual`. Every consumer must reason in that direction only —
//! a symbol OUTSIDE a set provably never lands on that tape, while a symbol
//! inside it merely may. Wherever a rule is uncertain the analysis ADDS to
//! the set and never trims: a call whose target is not in the program
//! contributes the caller tape's whole alphabet, and so does a binding whose
//! shape does not fit the callee it names.
//!
//! Two facts bound the representation. A tape's alphabet holds at most 127
//! glyphs (docs/tmt/language.md (alphabets)), so a single `u128` is a whole
//! symbol set with a bit to spare. And every set the table hands out is
//! clamped to its own tape's cardinality — `set ⊆ SymSet::full(cardinality)`
//! holds for every entry — so a consumer may use any member as an index into
//! that tape's glyph table.
//!
//! # Two walks, one relation
//!
//! The same footprint is inferred at two stages, and they do not agree
//! exactly: on every world both compute, `infer_resolved ⊇ infer_ir`. The
//! source walk sees a world before expansion, so it is coarser in four ways —
//! a `{expr}` write cell answers with the whole alphabet where expansion folds
//! it to concrete symbols per row; a graft's writes are projected rather than
//! spliced, and the splice drops rules whose pattern no host symbol reads as
//! and turns a write with no host image into a trap; a bound call on a callee
//! outside the compilation unit answers conservatively where lowering rejects
//! the program outright; and rules a later catch-all shadows are still present.
//! Consumers that see only source form (the lint layer) get the coarser
//! answer, which is the safe one.

use std::collections::HashMap;
use std::fmt;

use crate::compiler::{Resolved, ResolvedCallTarget, ResolvedWorld};
use crate::ir::{IrMapPair, IrProgram, IrTapeBinding, IrTransition, IrWorld, IrWrite};
use crate::lint::patterns::glyph_label;
use crate::parser::{BindingArg, BindingValue, MapArrow, SymLit, SymMap, WriteCellKind};

/// One past the highest symbol index a [`SymSet`] can hold. The alphabet
/// ceiling is 127 glyphs (docs/tmt/language.md (alphabets)), so this bound is
/// never reached by a well-formed program.
const MAX_SYMBOLS: u32 = 128;

/// A set of symbol indices on one tape.
#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct SymSet(u128);

impl SymSet {
    /// The empty set — writes nothing.
    pub(crate) fn empty() -> Self {
        SymSet(0)
    }

    /// Every symbol of an alphabet of `cardinality` glyphs: bits
    /// `0..cardinality`. The conservative answer, and the clamp every
    /// inferred set is intersected with.
    pub(crate) fn full(cardinality: u32) -> Self {
        if cardinality >= MAX_SYMBOLS {
            SymSet(u128::MAX)
        } else {
            SymSet((1u128 << cardinality) - 1)
        }
    }

    /// Add one symbol index.
    pub(crate) fn insert(&mut self, index: u32) {
        // The bound is a premise, not a policy: an index at or above it means
        // an alphabet wider than the language allows reached this analysis.
        // The explicit guard is load-bearing beyond the assertion — a release
        // build masks the shift count, so an unguarded `1 << 128` would set
        // bit 0 and quietly corrupt the set rather than overflow.
        debug_assert!(
            index < MAX_SYMBOLS,
            "symbol index {index} exceeds the alphabet ceiling"
        );
        if index < MAX_SYMBOLS {
            self.0 |= 1u128 << index;
        }
    }

    /// Whether `index` is a member.
    pub(crate) fn contains(&self, index: u32) -> bool {
        index < MAX_SYMBOLS && self.0 & (1u128 << index) != 0
    }

    /// Add every member of `other`, reporting whether this set GREW. That
    /// answer is the fixpoint's only stop condition.
    pub(crate) fn union_with(&mut self, other: SymSet) -> bool {
        let before = self.0;
        self.0 |= other.0;
        self.0 != before
    }

    /// The intersection — how a projected contribution is clamped to the
    /// receiving tape's alphabet.
    pub(crate) fn intersect(self, other: SymSet) -> SymSet {
        SymSet(self.0 & other.0)
    }

    /// Whether every member of `other` is a member here.
    // No consumer outside this module's own tests yet — every current
    // superset-shaped question is answered by `union_with`'s growth flag
    // instead. Kept because a set primitive missing `is_superset` is a
    // worse primitive than one carrying an unused method.
    #[allow(dead_code)]
    pub(crate) fn is_superset(&self, other: SymSet) -> bool {
        self.0 & other.0 == other.0
    }

    /// The members, ascending.
    pub(crate) fn iter(&self) -> impl Iterator<Item = u32> + '_ {
        let mut bits = self.0;
        std::iter::from_fn(move || {
            if bits == 0 {
                return None;
            }
            let next = bits.trailing_zeros();
            bits &= bits - 1;
            Some(next)
        })
    }

    /// How many symbols are in the set.
    // No consumer yet: every current member-count question is answered by
    // rendering `iter()`'s members directly (the `ir footprints` report) or
    // by `union_with`'s growth flag, never a bare cardinality.
    #[allow(dead_code)]
    pub(crate) fn len(&self) -> u32 {
        self.0.count_ones()
    }

    /// Whether the set is empty.
    // No consumer yet: every current emptiness question is answered by
    // `iter()` naturally yielding nothing, never a dedicated check.
    #[allow(dead_code)]
    pub(crate) fn is_empty(&self) -> bool {
        self.0 == 0
    }
}

impl fmt::Debug for SymSet {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_set().entries(self.iter()).finish()
    }
}

/// The write sets of one world, one entry per tape in the world's tape order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct WorldFootprint {
    pub(crate) tapes: Vec<SymSet>,
}

/// Every world's footprint, keyed by mangled world name (`main`, `ns::name`).
#[derive(Clone, Debug, Default)]
pub(crate) struct FootprintTable {
    pub(crate) worlds: HashMap<String, WorldFootprint>,
}

/// Project one callee tape's write set back into the caller's alphabet.
///
/// A binding's pair list is SPARSE: the lowering records the pairs the source
/// authored and nothing else — it identity-completes nothing, and it does not
/// even list the blank. What the map does with a callee symbol no pair names
/// is therefore decided here, from the two cardinalities, the way the linker's
/// composition engine decides it (docs/tmt/language.md (symbol maps)):
///
/// * a two-way pair writes its `dst` back as its `src`;
/// * a one-way pair writes nothing back — "one way" is a property of the
///   PAIR, not of the callee symbol it names. That symbol is simply UNLISTED
///   in the write direction, so the two completion rules below still apply to
///   it;
/// * the blank (index 0) is pinned in both directions and always writes back
///   as the blank;
/// * equal cardinalities identity-complete: an unlisted callee symbol writes
///   back as the same index;
/// * unequal cardinalities close the map: an unlisted non-blank callee symbol
///   is a hole, and writing it takes the unmapped-write trap instead of
///   landing on the caller's tape, so it contributes nothing.
fn project_write_back(
    callee: SymSet,
    pairs: &[IrMapPair],
    caller_card: u32,
    callee_card: u32,
) -> SymSet {
    let identity_completes = caller_card == callee_card;
    let mut out = SymSet::empty();
    for symbol in callee.iter() {
        let mut listed = false;
        for pair in pairs {
            if !pair.one_way && pair.dst == symbol {
                // A repeat with a different image is a link-time conflict;
                // taking every image keeps the answer on the safe side.
                out.insert(pair.src);
                listed = true;
            }
        }
        if !listed && (symbol == 0 || identity_completes) {
            out.insert(symbol);
        }
    }
    out
}

/// Every caller tape's whole alphabet — the conservative answer whenever a
/// call site cannot be projected.
fn full_alphabets(caller: &IrWorld) -> Vec<SymSet> {
    caller
        .tapes
        .iter()
        .map(|t| SymSet::full(t.cardinality))
        .collect()
}

/// What one call site contributes to its caller, per caller tape.
fn call_contribution(
    program: &IrProgram,
    by_name: &HashMap<&str, usize>,
    sets: &[Vec<SymSet>],
    caller_ix: usize,
    target: &str,
    binding: &[IrTapeBinding],
) -> Vec<SymSet> {
    let caller = &program.worlds[caller_ix];

    let Some(&callee_ix) = by_name.get(target) else {
        // An external or library callee: its body is not here to walk, so it
        // may write anything on every tape it can reach. A binding names the
        // tapes it reaches; a bindless call reaches them all.
        if binding.is_empty() {
            return full_alphabets(caller);
        }
        let mut out = vec![SymSet::empty(); caller.tapes.len()];
        for tb in binding {
            let tape = tb.caller_tape as usize;
            if let Some(cardinality) = caller.tapes.get(tape).map(|t| t.cardinality) {
                out[tape] = SymSet::full(cardinality);
            }
        }
        return out;
    };
    let callee = &program.worlds[callee_ix];
    let callee_sets = &sets[callee_ix];

    if binding.is_empty() {
        // A bindless call rides the identity composite: callee tape `k` IS
        // caller tape `k`, symbols unchanged.
        if callee.tapes.len() > caller.tapes.len() {
            return full_alphabets(caller);
        }
        let mut out = vec![SymSet::empty(); caller.tapes.len()];
        for (k, set) in callee_sets.iter().enumerate() {
            out[k].union_with(*set);
        }
        return out;
    }

    // A binding has one record per callee tape. A count that disagrees with
    // the callee's arity is rejected before a run exists; answering in the
    // safe direction beats projecting half a binding.
    if binding.len() != callee.tapes.len() {
        return full_alphabets(caller);
    }

    let mut out = vec![SymSet::empty(); caller.tapes.len()];
    for (k, tb) in binding.iter().enumerate() {
        let tape = tb.caller_tape as usize;
        let (Some(caller_tape), Some(callee_tape), Some(callee_set)) = (
            caller.tapes.get(tape),
            callee.tapes.get(k),
            callee_sets.get(k),
        ) else {
            return full_alphabets(caller);
        };
        let projected = project_write_back(
            *callee_set,
            &tb.pairs,
            caller_tape.cardinality,
            callee_tape.cardinality,
        );
        out[tape].union_with(projected);
    }
    out
}

/// Infer every world's write footprint from the lowered IR.
pub(crate) fn infer_ir(program: &IrProgram) -> FootprintTable {
    let by_name: HashMap<&str, usize> = program
        .worlds
        .iter()
        .enumerate()
        .map(|(i, w)| (w.name.as_str(), i))
        .collect();
    // The per-tape clamps that keep every set inside its own alphabet.
    let caps: Vec<Vec<SymSet>> = program
        .worlds
        .iter()
        .map(|w| {
            w.tapes
                .iter()
                .map(|t| SymSet::full(t.cardinality))
                .collect()
        })
        .collect();
    let mut sets: Vec<Vec<SymSet>> = caps
        .iter()
        .map(|world| vec![SymSet::empty(); world.len()])
        .collect();

    // Seed with the rows' own write cells. A `write: None` row is all-keep,
    // and a `Keep` cell writes nothing; both contribute nothing. Indices are
    // in bounds by validation, and the clamp keeps the invariant total.
    for (wi, world) in program.worlds.iter().enumerate() {
        for state in &world.states {
            for rule in &state.rules {
                let Some(write) = &rule.write else { continue };
                for (tape, cell) in write.iter().enumerate() {
                    let IrWrite::Index { index } = cell else {
                        continue;
                    };
                    let mut one = SymSet::empty();
                    one.insert(*index);
                    if let (Some(slot), Some(cap)) = (sets[wi].get_mut(tape), caps[wi].get(tape)) {
                        slot.union_with(one.intersect(*cap));
                    }
                }
            }
        }
    }

    // Then close over the call edges. The loop is deliberately uncapped: a
    // contribution can only ever grow a set, and a set is bounded by its
    // tape's alphabet, so it terminates by monotonicity. A round cap would
    // stop the walk early and under-approximate — the one direction this
    // analysis may not take.
    loop {
        let mut grew = false;
        for (wi, world) in program.worlds.iter().enumerate() {
            for state in &world.states {
                for rule in &state.rules {
                    // The two transitions that name a callee world; a `bind`
                    // site lowers to a `CallThen`, so this covers it too.
                    let (target, binding) = match &rule.transition {
                        IrTransition::CallThen {
                            target, binding, ..
                        } => (target.as_str(), binding.as_slice()),
                        IrTransition::TailCall { target } => (target.as_str(), [].as_slice()),
                        _ => continue,
                    };
                    let contribution =
                        call_contribution(program, &by_name, &sets, wi, target, binding);
                    for (tape, add) in contribution.into_iter().enumerate() {
                        if let (Some(slot), Some(cap)) =
                            (sets[wi].get_mut(tape), caps[wi].get(tape))
                        {
                            grew |= slot.union_with(add.intersect(*cap));
                        }
                    }
                }
            }
        }
        if !grew {
            break;
        }
    }

    FootprintTable {
        worlds: program
            .worlds
            .iter()
            .zip(sets)
            .map(|(w, tapes)| (w.name.clone(), WorldFootprint { tapes }))
            .collect(),
    }
}

// ---------------------------------------------------------------------------
// The source-level walk. Same rules, one stage earlier: worlds are still in
// SOURCE form — patterns unexpanded, grafts unspliced, `{expr}` writes
// unfolded — and every symbol is a GLYPH in its own world's alphabet frame
// rather than an index the lowering already resolved.
// ---------------------------------------------------------------------------

/// The whole alphabet of every one of a source world's tapes.
fn full_alphabets_src(host: &ResolvedWorld) -> Vec<SymSet> {
    host.tapes
        .iter()
        .map(|t| SymSet::full(t.cardinality as u32))
        .collect()
}

/// A symbol literal's position in a glyph vector — the source-frame analog of
/// an already-lowered symbol index. `None` when the alphabet does not carry
/// the glyph, which resolution rejects downstream.
fn glyph_index(glyphs: &[String], lit: &SymLit) -> Option<u32> {
    let label = glyph_label(lit);
    glyphs.iter().position(|g| *g == label).map(|i| i as u32)
}

/// Resolve a source `with map` into the same sparse pair list the IR lowering
/// records: `src` glyphs against the HOST alphabet, `dst` glyphs against the
/// callee's or grafted graph's, and `=>` marking the read-only direction.
/// `None` when a glyph is outside the alphabet it is resolved against — the
/// caller then declines to project that tape.
fn source_pairs(
    map: &SymMap,
    host_glyphs: &[String],
    callee_glyphs: &[String],
) -> Option<Vec<IrMapPair>> {
    map.pairs
        .iter()
        .map(|p| {
            Some(IrMapPair {
                src: glyph_index(host_glyphs, &p.src)?,
                dst: glyph_index(callee_glyphs, &p.dst)?,
                one_way: p.arrow == MapArrow::ReadOnly,
            })
        })
        .collect()
}

/// What one binding site — a `call`, a bind-call, or a `graft` — contributes
/// to its host, per host tape.
///
/// Calls and grafts share this one projection because they share one algebra:
/// `ir.rs`'s call lowering and `expand.rs`'s graft composite resolve `src`
/// against the host alphabet and `dst` against the callee's, then hand the
/// result to the same completion rule (docs/tmt/language.md (symbol maps)).
/// The two differ only in strictness about an OMITTED map — a graft demands
/// glyph-for-glyph equal alphabets where a call binds by index — and that
/// difference rejects programs rather than changing what a legal one writes.
fn binding_contribution(
    resolved: &Resolved,
    host: &ResolvedWorld,
    callee: &ResolvedWorld,
    callee_sets: &[SymSet],
    args: &[BindingArg],
) -> Vec<SymSet> {
    // A bare name is a tape target or a state continuation; only the callee's
    // tape signature tells them apart, so the named args are filtered by it
    // below. A site with no named args at all carries no binding: it rides the
    // identity placement, callee tape `k` onto host tape `k`.
    let named: Vec<&BindingArg> = args
        .iter()
        .filter(|a| matches!(a.value, BindingValue::Named { .. }))
        .collect();
    if named.is_empty() {
        if callee.tapes.len() > host.tapes.len() {
            return full_alphabets_src(host);
        }
        let mut out = vec![SymSet::empty(); host.tapes.len()];
        for (k, s) in callee_sets.iter().enumerate() {
            out[k].union_with(*s);
        }
        return out;
    }

    let mut out = vec![SymSet::empty(); host.tapes.len()];
    for (k, ct) in callee.tapes.iter().enumerate() {
        // Every callee tape is bound and every target resolves, or the program
        // does not compile; answering full beats projecting half a binding.
        let Some(arg) = named.iter().find(|a| a.name == ct.name) else {
            return full_alphabets_src(host);
        };
        let BindingValue::Named {
            target: host_name,
            map,
            ..
        } = &arg.value
        else {
            unreachable!("the named args are Named by construction");
        };
        let Some(phys) = host.tapes.iter().position(|t| t.name == *host_name) else {
            return full_alphabets_src(host);
        };
        let host_tape = &host.tapes[phys];
        let host_card = host_tape.cardinality as u32;
        let Some(callee_set) = callee_sets.get(k) else {
            return full_alphabets_src(host);
        };

        let pairs = match map {
            // An omitted map is no pairs at all: the completion rule below
            // decides the whole projection from the two cardinalities.
            None => Vec::new(),
            Some(m) => {
                let glyphs = resolved
                    .alphabets
                    .get(&host_tape.alphabet)
                    .zip(resolved.alphabets.get(&ct.alphabet));
                match glyphs.and_then(|(h, c)| source_pairs(m, &h.glyphs, &c.glyphs)) {
                    Some(pairs) => pairs,
                    // A glyph outside its alphabet, or an alphabet resolution
                    // never produced: the map cannot be read, so nothing about
                    // this tape can be ruled out.
                    None => {
                        out[phys].union_with(SymSet::full(host_card));
                        continue;
                    }
                }
            }
        };
        out[phys].union_with(project_write_back(
            *callee_set,
            &pairs,
            host_card,
            ct.cardinality as u32,
        ));
    }
    out
}

/// The host tapes an unresolvable callee may write: the ones its args
/// IDENTIFY by name, or — when no arg identifies one — all of them.
///
/// The fallback is keyed on the identified tapes, never on the raw arg list.
/// An arg naming no host tape says nothing about where the callee writes: it
/// may be a state continuation, a typo'd tape name, or a `call` on a local
/// graph that resolution routed to `external`. Reading "names nothing
/// recognizable" as "reaches nothing" would also make the answer non-monotone,
/// since adding a useless arg would shrink it, so a site with args but no
/// identifiable tape gets exactly the argless site's answer.
fn unresolved_contribution(host: &ResolvedWorld, args: &[BindingArg]) -> Vec<SymSet> {
    let bound: Vec<usize> = args
        .iter()
        .filter_map(|a| match &a.value {
            BindingValue::Named { target, .. } => host.tapes.iter().position(|t| t.name == *target),
            BindingValue::Terminator { .. } => None,
        })
        .collect();
    if bound.is_empty() {
        return full_alphabets_src(host);
    }
    let mut out = vec![SymSet::empty(); host.tapes.len()];
    for phys in bound {
        out[phys] = SymSet::full(host.tapes[phys].cardinality as u32);
    }
    out
}

/// One reuse edge out of a world: the callee's mangled name (`None` when this
/// module cannot see its body) and the source-form binding args.
struct Edge<'a> {
    target: Option<&'a str>,
    args: &'a [BindingArg],
}

/// Every edge a world reaches another world through: its `call` transitions
/// (direct or through a world-local `bind`) and its `graft` declarations.
fn edges_of(world: &ResolvedWorld) -> Vec<Edge<'_>> {
    let mut edges = Vec::new();
    for call in &world.calls {
        match &call.target {
            ResolvedCallTarget::Routine {
                name,
                external,
                args,
            } => edges.push(Edge {
                target: (!external).then_some(name.as_str()),
                args,
            }),
            // A bind-call's binding lives on the `bind` declaration, shared by
            // every call of that instance.
            ResolvedCallTarget::Bind { name } => {
                match world.binds.iter().find(|b| b.name == *name) {
                    Some(b) => edges.push(Edge {
                        target: (!b.external).then_some(b.target.as_str()),
                        args: &b.args,
                    }),
                    // A bind name with no declaration cannot happen (the call
                    // resolved AS a bind by matching one); answer full anyway.
                    None => edges.push(Edge {
                        target: None,
                        args: &[],
                    }),
                }
            }
        }
    }
    // A graft target is always a locally defined graph — resolution rejects an
    // external one, because splicing needs the graph's source.
    for graft in &world.grafts {
        edges.push(Edge {
            target: Some(graft.target.as_str()),
            args: &graft.args,
        });
    }
    edges
}

/// Infer every world's write footprint from the resolved SOURCE module.
///
/// The table covers GRAPHS as well as routines and the machine — a graph is a
/// world with its own tape frame, and it is where a grafted body's writes are
/// stated. Graphs take part in the same fixpoint: a graph may graft another
/// graph even though it may never call a routine.
pub(crate) fn infer_resolved(resolved: &Resolved) -> FootprintTable {
    let by_name: HashMap<&str, usize> = resolved
        .worlds
        .iter()
        .enumerate()
        .map(|(i, w)| (w.name.as_str(), i))
        .collect();
    let caps: Vec<Vec<SymSet>> = resolved
        .worlds
        .iter()
        .map(|w| {
            w.tapes
                .iter()
                .map(|t| SymSet::full(t.cardinality as u32))
                .collect()
        })
        .collect();
    let mut sets: Vec<Vec<SymSet>> = caps
        .iter()
        .map(|world| vec![SymSet::empty(); world.len()])
        .collect();

    // Seed with the rows' own write cells, resolved in the world's own frame.
    // A `-` keeps; a literal outside the tape's alphabet and a `{expr}` fold
    // both answer with the whole alphabet — the fold's value is decided per
    // expanded row from the symbols the pattern matched, and this walk does
    // not expand (docs/tmt/language.md (substitution)).
    for (wi, world) in resolved.worlds.iter().enumerate() {
        for state in &world.states {
            for rule in &state.rules {
                let Some(write) = &rule.write else { continue };
                // A vector is one cell per tape, by position — but that width
                // is enforced during expansion, not resolution, so a walk over
                // a merely-resolved module can meet a short one (an editor
                // sees one on every half-typed rule). Reading a missing cell
                // as `keep` would UNDER-approximate, so a width that does not
                // match the world's arity answers full on every tape instead.
                if write.cells.len() != world.tapes.len() {
                    for (slot, cap) in sets[wi].iter_mut().zip(&caps[wi]) {
                        slot.union_with(*cap);
                    }
                    continue;
                }
                for (tape, cell) in write.cells.iter().enumerate() {
                    let (Some(rt), Some(cap)) = (world.tapes.get(tape), caps[wi].get(tape)) else {
                        continue;
                    };
                    let add = match &cell.kind {
                        WriteCellKind::Keep => continue,
                        WriteCellKind::Subst { .. } => *cap,
                        WriteCellKind::Lit(lit) => {
                            let index = resolved
                                .alphabets
                                .get(&rt.alphabet)
                                .and_then(|a| glyph_index(&a.glyphs, lit));
                            match index {
                                Some(index) => {
                                    let mut one = SymSet::empty();
                                    one.insert(index);
                                    one
                                }
                                None => *cap,
                            }
                        }
                    };
                    if let Some(slot) = sets[wi].get_mut(tape) {
                        slot.union_with(add.intersect(*cap));
                    }
                }
            }
        }
    }

    // Then close over the reuse edges. Uncapped for the same reason the IR
    // walk's loop is: sets only grow and are bounded by their alphabets, so it
    // terminates by monotonicity, and a cap would under-approximate. The
    // rounds are load-bearing here — the resolved world order puts routines
    // before the graphs they graft, so a graft edge points forward.
    loop {
        let mut grew = false;
        for (wi, world) in resolved.worlds.iter().enumerate() {
            for edge in edges_of(world) {
                let contribution = match edge.target.and_then(|t| by_name.get(t)) {
                    Some(&callee_ix) => binding_contribution(
                        resolved,
                        world,
                        &resolved.worlds[callee_ix],
                        &sets[callee_ix],
                        edge.args,
                    ),
                    // A callee outside this compilation unit: its body is not
                    // here to walk, so it may write anything it can reach.
                    None => unresolved_contribution(world, edge.args),
                };
                for (tape, add) in contribution.into_iter().enumerate() {
                    if let (Some(slot), Some(cap)) = (sets[wi].get_mut(tape), caps[wi].get(tape)) {
                        grew |= slot.union_with(add.intersect(*cap));
                    }
                }
            }
        }
        if !grew {
            break;
        }
    }

    FootprintTable {
        worlds: resolved
            .worlds
            .iter()
            .zip(sets)
            .map(|(w, tapes)| (w.name.clone(), WorldFootprint { tapes }))
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compiler::{CompileOptions, analyze, compile};
    use crate::ir::{
        IrCell, IrDispatch, IrMapPair, IrProgram, IrRule, IrState, IrTape, IrTapeBinding, IrThen,
        IrTransition, IrWorld, IrWorldKind, IrWrite, TM_IR_VERSION,
    };

    // -- fixture builders (the `ir.rs::serde_tags_are_frozen` construction
    // style, trimmed to the fields this analysis reads) -------------------

    fn tape(name: &str, cardinality: u32) -> IrTape {
        IrTape {
            name: name.into(),
            alphabet: format!("al_{name}"),
            cardinality,
            volatile: false,
        }
    }

    /// A write vector: `Some(i)` writes index `i`, `None` keeps.
    fn wr(cells: &[Option<u32>]) -> Option<Vec<IrWrite>> {
        Some(
            cells
                .iter()
                .map(|c| match c {
                    Some(index) => IrWrite::Index { index: *index },
                    None => IrWrite::Keep,
                })
                .collect(),
        )
    }

    fn rule(arity: usize, write: Option<Vec<IrWrite>>, transition: IrTransition) -> IrRule {
        IrRule {
            pattern: vec![IrCell::Wildcard; arity],
            write,
            moves: None,
            debugger: false,
            transition,
            synthesized: false,
            direct: false,
            line: 0,
        }
    }

    fn world(name: &str, tapes: Vec<IrTape>, rules: Vec<IrRule>) -> IrWorld {
        IrWorld {
            name: name.into(),
            kind: IrWorldKind::Routine,
            arity: tapes.len() as u32,
            tapes,
            entry: 0,
            states: vec![IrState {
                id: 0,
                name: "s".into(),
                line: 0,
                rules,
                dispatch: IrDispatch::Table,
            }],
            local: true,
            line: 0,
        }
    }

    fn program(worlds: Vec<IrWorld>) -> IrProgram {
        IrProgram {
            version: TM_IR_VERSION,
            worlds,
            entry_world: Some(0),
        }
    }

    fn call(target: &str, binding: Vec<IrTapeBinding>) -> IrTransition {
        IrTransition::CallThen {
            target: target.into(),
            binding,
            then: IrThen::Return,
        }
    }

    fn bind1(caller_tape: u32, pairs: &[(u32, u32, bool)]) -> Vec<IrTapeBinding> {
        vec![IrTapeBinding {
            caller_tape,
            pairs: pairs
                .iter()
                .map(|(src, dst, one_way)| IrMapPair {
                    src: *src,
                    dst: *dst,
                    one_way: *one_way,
                })
                .collect(),
        }]
    }

    fn set(indices: &[u32]) -> SymSet {
        let mut s = SymSet::empty();
        for i in indices {
            s.insert(*i);
        }
        s
    }

    /// The inferred sets of one world, by mangled name.
    fn tapes_of<'a>(table: &'a FootprintTable, world: &str) -> &'a [SymSet] {
        &table
            .worlds
            .get(world)
            .expect("world is in the table")
            .tapes
    }

    // -- the primitive ----------------------------------------------------

    #[test]
    fn symset_full_iter_and_superset() {
        assert_eq!(SymSet::empty().len(), 0);
        assert_eq!(SymSet::full(0), SymSet::empty());

        let three = SymSet::full(3);
        assert_eq!(three.len(), 3);
        assert_eq!(three.iter().collect::<Vec<_>>(), vec![0, 1, 2]);
        assert!(three.contains(2));
        assert!(!three.contains(3));

        // The 127-glyph ceiling still fits one u128 with a bit to spare.
        assert_eq!(SymSet::full(127).len(), 127);

        assert!(three.is_superset(set(&[0, 2])));
        assert!(!three.is_superset(set(&[2, 3])));
        assert!(three.is_superset(SymSet::empty()));

        // `union_with` reports growth — the fixpoint's only stop condition.
        let mut acc = set(&[1]);
        assert!(acc.union_with(set(&[2])));
        assert_eq!(acc, set(&[1, 2]));
        assert!(!acc.union_with(set(&[1, 2])));
        assert!(!acc.union_with(SymSet::empty()));

        // Ascending, gaps and all.
        assert_eq!(set(&[7, 0, 3]).iter().collect::<Vec<_>>(), vec![0, 3, 7]);
    }

    // -- direct writes ----------------------------------------------------

    #[test]
    fn direct_writes_are_collected() {
        let p = program(vec![world(
            "main",
            vec![tape("a", 5), tape("b", 5), tape("c", 5)],
            vec![
                rule(3, wr(&[Some(1), Some(2), None]), IrTransition::Return),
                rule(3, wr(&[None, Some(3), None]), IrTransition::Stop),
                // An all-keep row (`write: None`) contributes nothing.
                rule(3, None, IrTransition::Halt),
            ],
        )]);

        let table = infer_ir(&p);
        let t = tapes_of(&table, "main");
        assert_eq!(t[0], set(&[1]));
        assert_eq!(t[1], set(&[2, 3]));
        assert_eq!(t[2], SymSet::empty(), "a keep-only tape writes nothing");
    }

    // -- projection through a binding -------------------------------------

    #[test]
    fn call_projects_through_the_binding_pairs() {
        // Caller tape 0 (card 5) feeds the callee's only tape (card 3): the
        // cardinalities differ, so the map is closed.
        let p = program(vec![
            world(
                "caller",
                vec![tape("host", 5)],
                vec![rule(
                    1,
                    None,
                    call("callee", bind1(0, &[(2, 1, false), (3, 2, true)])),
                )],
            ),
            world(
                "callee",
                vec![tape("v", 3)],
                vec![
                    rule(1, wr(&[Some(1)]), IrTransition::Return),
                    rule(1, wr(&[Some(2)]), IrTransition::Return),
                ],
            ),
        ]);

        let table = infer_ir(&p);
        assert_eq!(tapes_of(&table, "callee")[0], set(&[1, 2]));
        assert_eq!(
            tapes_of(&table, "caller")[0],
            set(&[2]),
            "callee symbol 1 writes back as caller 2; the one-way pair's \
             symbol 2 never writes back, so caller 3 is not in the set"
        );
    }

    /// The Step-1 artifact pin: the exact binding `ir::lower` emits for the
    /// `a5_call_across_alphabets` golden — sparse, authored pairs only, with
    /// the callee's blank left unlisted.
    #[test]
    fn a5_binding_projects_as_the_lowerer_emits_it() {
        let p = program(vec![
            world(
                "main",
                vec![tape("ctl", 3), tape("data", 5)],
                vec![rule(
                    2,
                    None,
                    call("mylib::plusOne", bind1(1, &[(3, 1, false), (4, 2, false)])),
                )],
            ),
            world(
                "mylib::plusOne",
                vec![tape("num", 3)],
                vec![
                    rule(1, wr(&[Some(1)]), IrTransition::Goto { state: 0 }),
                    rule(1, wr(&[Some(2)]), IrTransition::Return),
                ],
            ),
        ]);

        let table = infer_ir(&p);
        assert_eq!(tapes_of(&table, "mylib::plusOne")[0], set(&[1, 2]));
        let main = tapes_of(&table, "main");
        assert_eq!(main[0], SymSet::empty(), "ctl is unbound at the call");
        assert_eq!(main[1], set(&[3, 4]), "'0'/'1' land back on the wide tape");
    }

    #[test]
    fn equal_cardinality_identity_completes_unlisted() {
        let p = program(vec![
            world(
                "caller",
                vec![tape("host", 4)],
                vec![rule(1, None, call("callee", bind1(0, &[(1, 2, false)])))],
            ),
            world(
                "callee",
                vec![tape("v", 4)],
                vec![
                    rule(1, wr(&[Some(2)]), IrTransition::Return),
                    rule(1, wr(&[Some(3)]), IrTransition::Return),
                ],
            ),
        ]);

        assert_eq!(
            tapes_of(&infer_ir(&p), "caller")[0],
            set(&[1, 3]),
            "callee 2 writes back through the pair; unlisted callee 3 \
             identity-completes across equal-size alphabets"
        );
    }

    #[test]
    fn unequal_cardinality_holes_unlisted_but_pins_blank() {
        let p = program(vec![
            world(
                "caller",
                vec![tape("host", 5)],
                vec![rule(1, None, call("callee", bind1(0, &[(3, 1, false)])))],
            ),
            world(
                "callee",
                vec![tape("v", 3)],
                vec![
                    rule(1, wr(&[Some(0)]), IrTransition::Return),
                    rule(1, wr(&[Some(1)]), IrTransition::Return),
                    rule(1, wr(&[Some(2)]), IrTransition::Return),
                ],
            ),
        ]);

        assert_eq!(
            tapes_of(&infer_ir(&p), "caller")[0],
            set(&[0, 3]),
            "the blank stays pinned; callee 1 maps through the pair; \
             unlisted callee 2 is a hole (an unmapped write traps)"
        );
    }

    #[test]
    fn one_way_pair_dst_identity_completes_on_equal_cardinality() {
        // "One-way never writes back" is a property of the PAIR, not of the
        // callee symbol it names: across equal-size alphabets that symbol is
        // still unlisted in the write direction, so it identity-completes.
        let p = program(vec![
            world(
                "caller",
                vec![tape("host", 4)],
                vec![rule(1, None, call("callee", bind1(0, &[(1, 2, true)])))],
            ),
            world(
                "callee",
                vec![tape("v", 4)],
                vec![rule(1, wr(&[Some(2)]), IrTransition::Return)],
            ),
        ]);

        let table = infer_ir(&p);
        let caller = tapes_of(&table, "caller")[0];
        assert_eq!(caller, set(&[2]));
        assert!(!caller.contains(1), "a one-way pair never writes back");
    }

    #[test]
    fn empty_binding_is_identity() {
        let p = program(vec![
            world(
                "caller",
                vec![tape("a", 4), tape("b", 4)],
                vec![rule(2, None, call("callee", Vec::new()))],
            ),
            world(
                "callee",
                vec![tape("x", 4), tape("y", 4)],
                vec![rule(2, wr(&[None, Some(1)]), IrTransition::Return)],
            ),
        ]);

        let table = infer_ir(&p);
        let caller = tapes_of(&table, "caller");
        assert_eq!(caller[0], SymSet::empty());
        assert_eq!(caller[1], set(&[1]), "callee tape k rides caller tape k");
    }

    #[test]
    fn tail_call_is_identity_on_all_tapes() {
        let p = program(vec![
            world(
                "caller",
                vec![tape("a", 4), tape("b", 4)],
                vec![rule(
                    2,
                    None,
                    IrTransition::TailCall {
                        target: "callee".into(),
                    },
                )],
            ),
            world(
                "callee",
                vec![tape("x", 4), tape("y", 4)],
                vec![rule(2, wr(&[Some(1), Some(2)]), IrTransition::Return)],
            ),
        ]);

        let table = infer_ir(&p);
        let caller = tapes_of(&table, "caller");
        assert_eq!(caller[0], set(&[1]));
        assert_eq!(caller[1], set(&[2]));
    }

    #[test]
    fn projection_is_clamped_to_the_caller_alphabet() {
        // A bindless call into a wider callee: identity would carry callee
        // symbol 4 onto a two-symbol caller tape. The contribution is
        // clamped, so every set stays inside its own tape's alphabet.
        let p = program(vec![
            world(
                "caller",
                vec![tape("narrow", 2)],
                vec![rule(1, None, call("callee", Vec::new()))],
            ),
            world(
                "callee",
                vec![tape("wide", 5)],
                vec![
                    rule(1, wr(&[Some(1)]), IrTransition::Return),
                    rule(1, wr(&[Some(4)]), IrTransition::Return),
                ],
            ),
        ]);

        let table = infer_ir(&p);
        assert_eq!(tapes_of(&table, "callee")[0], set(&[1, 4]));
        let caller = tapes_of(&table, "caller")[0];
        assert!(
            SymSet::full(2).is_superset(caller),
            "inferred set {caller:?} escaped the caller's alphabet"
        );
        assert_eq!(caller, set(&[1]));
    }

    // -- recursion --------------------------------------------------------

    #[test]
    fn mutual_recursion_reaches_a_fixpoint() {
        // a -> b -> a. Completing at all is the termination guard.
        let p = program(vec![
            world(
                "a",
                vec![tape("t", 4)],
                vec![rule(1, wr(&[Some(1)]), call("b", Vec::new()))],
            ),
            world(
                "b",
                vec![tape("t", 4)],
                vec![rule(1, wr(&[Some(2)]), call("a", Vec::new()))],
            ),
        ]);

        let table = infer_ir(&p);
        assert_eq!(tapes_of(&table, "a")[0], set(&[1, 2]));
        assert_eq!(tapes_of(&table, "b")[0], set(&[1, 2]));
    }

    #[test]
    fn a_call_chain_propagates_across_rounds() {
        // a -> b -> c, walked in that order: one round carries c's write only
        // as far as b, so `a` seeing it proves the walk re-runs to a fixpoint
        // rather than settling for a single pass.
        let p = program(vec![
            world(
                "a",
                vec![tape("t", 5)],
                vec![rule(1, wr(&[Some(1)]), call("b", Vec::new()))],
            ),
            world(
                "b",
                vec![tape("t", 5)],
                vec![rule(1, wr(&[Some(2)]), call("c", Vec::new()))],
            ),
            world(
                "c",
                vec![tape("t", 5)],
                vec![rule(1, wr(&[Some(3)]), IrTransition::Return)],
            ),
        ]);

        let table = infer_ir(&p);
        assert_eq!(tapes_of(&table, "a")[0], set(&[1, 2, 3]));
        assert_eq!(tapes_of(&table, "b")[0], set(&[2, 3]));
        assert_eq!(tapes_of(&table, "c")[0], set(&[3]));
    }

    // -- the conservative fallbacks ---------------------------------------

    #[test]
    fn unknown_target_is_conservatively_full() {
        let p = program(vec![world(
            "caller",
            vec![tape("a", 3), tape("b", 4)],
            vec![rule(2, None, call("lib::helper", bind1(0, &[])))],
        )]);

        let table = infer_ir(&p);
        let caller = tapes_of(&table, "caller");
        assert_eq!(
            caller[0],
            SymSet::full(3),
            "an unresolved callee may write anything"
        );
        assert_eq!(
            caller[1],
            SymSet::empty(),
            "tape b is not bound at the site"
        );
    }

    #[test]
    fn unknown_bindless_target_is_full_on_every_tape() {
        let p = program(vec![world(
            "caller",
            vec![tape("a", 3), tape("b", 4)],
            vec![rule(
                2,
                None,
                IrTransition::TailCall {
                    target: "lib::helper".into(),
                },
            )],
        )]);

        let table = infer_ir(&p);
        let caller = tapes_of(&table, "caller");
        assert_eq!(caller[0], SymSet::full(3));
        assert_eq!(caller[1], SymSet::full(4));
    }

    #[test]
    fn bindless_call_into_a_wider_callee_is_conservatively_full() {
        // Identity placement has nowhere to put the callee's second tape.
        // Rejected at link; here it answers full rather than dropping the
        // tapes that do not line up.
        let p = program(vec![
            world(
                "caller",
                vec![tape("a", 3)],
                vec![rule(1, None, call("callee", Vec::new()))],
            ),
            world(
                "callee",
                vec![tape("x", 3), tape("y", 3)],
                vec![rule(2, wr(&[None, Some(1)]), IrTransition::Return)],
            ),
        ]);

        let table = infer_ir(&p);
        let caller = tapes_of(&table, "caller");
        assert_eq!(caller.len(), 1);
        assert_eq!(caller[0], SymSet::full(3));
    }

    // -- the source-level walk --------------------------------------------

    /// The resolved module of a `.tmc` source, or the analysis error.
    fn resolve(source: &str) -> Resolved {
        analyze(source)
            .unwrap_or_else(|e| panic!("the fixture analyzes: {:?} at {:?}", e.kind, e.span))
            .resolved
    }

    /// The glyph position of `glyph` in a world's tape `k` — the frame every
    /// assertion below is written in.
    fn index_of(resolved: &Resolved, world: &str, tape: usize, glyph: &str) -> u32 {
        let w = resolved
            .worlds
            .iter()
            .find(|w| w.name == world)
            .expect("the world is in the module");
        let al = &resolved.alphabets[&w.tapes[tape].alphabet];
        al.glyphs
            .iter()
            .position(|g| g == glyph)
            .unwrap_or_else(|| panic!("{glyph} is in {}", al.name)) as u32
    }

    /// A two-routine call chain: `main` calls `twice` across alphabets under
    /// an explicit map, and `twice` calls `flip` within one alphabet under an
    /// omitted (identity) map.
    const CALL_CHAIN_SRC: &str = "\
alphabet wide { '_', 'a', 'b', '0', '1' }
alphabet bits { '_', '0', '1' }

routine flip(tape num: bits) {
  entry state s {
    ['0'] -> write ['1'] return;
    ['1'] -> write ['0'] return;
    ['_'] -> return;
  }
}

routine twice(tape num: bits) {
  entry state a { [*] -> call flip(num = num) then b; }
  state b { [*] -> call flip(num = num) then return; }
}

machine {
  tape ctl: bits;
  tape data: wide;
  entry state s {
    [*, *] -> call twice(num = data with map { '0' -> '0', '1' -> '1' }) then stop;
  }
}
";

    #[test]
    fn source_walk_matches_the_ir_walk_on_a_call_chain() {
        let resolved = resolve(CALL_CHAIN_SRC);
        let source = infer_resolved(&resolved);

        // The IR side is compiled at `-O0`, so `TailCall` — an optimizer
        // product — never appears: the comparison exercises the `CallThen`
        // arm only, which is the one a source `call` lowers to.
        let out = compile(CALL_CHAIN_SRC, CompileOptions::default()).expect("the fixture compiles");
        let ir = infer_ir(&out.ir);

        // Pinned, not merely non-empty: a shrinking world set would leave the
        // loop below asserting almost nothing while still passing.
        assert_eq!(ir.worlds.len(), 3, "main, twice and flip: {:?}", ir.worlds);
        for (name, iw) in &ir.worlds {
            let sw = source
                .worlds
                .get(name)
                .unwrap_or_else(|| panic!("{name} is in the source table too"));
            assert_eq!(
                sw.tapes, iw.tapes,
                "the two walks disagree on {name}: source {:?} vs ir {:?}",
                sw.tapes, iw.tapes
            );
        }

        // And the value both agree on, spelled out in the source frame.
        let main = &source.worlds["main"].tapes;
        assert_eq!(main[0], SymSet::empty(), "ctl is unbound at the call");
        assert_eq!(
            main[1],
            set(&[
                index_of(&resolved, "main", 1, "0"),
                index_of(&resolved, "main", 1, "1"),
            ]),
            "the callee's digits write back onto the wide tape"
        );
        assert_eq!(source.worlds["twice"].tapes[0], set(&[1, 2]));
    }

    #[test]
    fn a_graph_footprint_is_direct_and_in_its_own_frame() {
        // A graft-free graph's footprint is its own rows' writes: no call may
        // appear in a graph that is ever grafted, and this one grafts nothing
        // either. (Graphs are NOT fixpoint-free in general — a graph may graft
        // another graph, which `a_graft_chain_propagates_across_rounds`
        // covers.)
        let src = "\
alphabet tri { '_', '0', '1' }

export graph flipOnes(tape v: tri, state done) {
  entry state s {
    ['0'] -> write ['1'] move [>] goto s;
    [*] -> done;
  }
}
";
        let resolved = resolve(src);
        let table = infer_resolved(&resolved);
        assert_eq!(
            table.worlds["flipOnes"].tapes[0],
            set(&[index_of(&resolved, "flipOnes", 0, "1")]),
            "the graph writes '1' and nothing else, in its OWN alphabet frame"
        );
        assert_eq!(index_of(&resolved, "flipOnes", 0, "1"), 2);
    }

    #[test]
    fn a_graft_projects_into_the_host_frame() {
        // The graph writes its blank and its '0'. The host binds '^' one-way
        // onto the graph's blank and '0' two-way onto the graph's '0', across
        // differently-sized alphabets (5 vs 3), so the map is CLOSED.
        //
        // The fixture also shows the walk over-approximating the splice on
        // purpose, which is why it stops at `analyze`: host '1' reads as no
        // graph symbol, so the graph's `['1'] -> write ['_']` rule has an
        // empty read preimage and the splice would DROP it — the blank this
        // test asserts the host gains can never actually land. Keeping it is
        // the safe direction.
        let src = "\
alphabet host5 { '_', '^', '$', '0', '1' }
alphabet bare3 { '_', '0', '1' }

export graph zeroing(tape v: bare3, state done) {
  entry state s {
    ['0'] -> write ['0'] move [>] goto s;
    ['1'] -> write ['_'] done;
    ['_'] -> done;
  }
}

export routine hosted(tape t: host5) {
  entry graft zeroing(v = t with map { '^' => '_', '0' -> '0' }, done = return) as z;
}
";
        let resolved = resolve(src);
        let table = infer_resolved(&resolved);
        let ix = |g: &str| index_of(&resolved, "hosted", 0, g);

        assert_eq!(
            table.worlds["zeroing"].tapes[0],
            set(&[0, 1]),
            "the graph writes its own blank and its own '0'"
        );

        let host = table.worlds["hosted"].tapes[0];
        assert!(
            host.contains(ix("0")),
            "the two-way pair writes the graph's '0' back as the host's"
        );
        assert!(
            host.contains(ix("_")),
            "the blank is pinned both ways, so a graph blank lands as a host blank"
        );
        assert!(
            !host.contains(ix("^")),
            "the '^' pair is ONE-WAY: it never writes back, so the graph's \
             blank must not land as '^'"
        );
        assert!(
            !host.contains(ix("$")),
            "'$' is unlisted across unequal alphabets — a closed map holes it"
        );
        assert!(
            !host.contains(ix("1")),
            "the graph never writes the symbol '1' maps from"
        );
        assert_eq!(host, set(&[ix("_"), ix("0")]));
    }

    #[test]
    fn a_graft_chain_propagates_across_rounds() {
        // routine -> graph -> graph, and the resolved world order is routines
        // first, then graphs: every graft edge points FORWARD, so one pass
        // carries `inner`'s write only as far as `mid`. `outer` seeing it is
        // what proves the source walk iterates to a fixpoint.
        let src = "\
alphabet tri { '_', '0', '1' }

export graph inner(tape v: tri, state done) {
  entry state s { [*] -> write ['1'] done; }
}

export graph mid(tape v: tri, state done) {
  entry state s { [*] -> goto i; }
  graft inner(v = v, done = done) as i;
}

export routine outer(tape v: tri) {
  entry graft mid(v = v, done = return) as m;
}
";
        let resolved = resolve(src);
        let table = infer_resolved(&resolved);
        let one = set(&[index_of(&resolved, "inner", 0, "1")]);
        assert_eq!(table.worlds["inner"].tapes[0], one);
        assert_eq!(table.worlds["mid"].tapes[0], one, "one graft hop");
        assert_eq!(table.worlds["outer"].tapes[0], one, "two graft hops");
    }

    #[test]
    fn a_substitution_write_is_conservatively_full() {
        // A `{expr}` write cell's value is decided per expanded row from the
        // symbols the pattern matched; the source walk does not expand, so it
        // answers with the tape's whole alphabet.
        let src = "\
alphabet tri { '_', '0', '1' }

export routine echoIt(tape v: tri) {
  entry state s { ['0'..'1' as c] -> write [{c}] return; }
}
";
        let resolved = resolve(src);
        let table = infer_resolved(&resolved);
        assert_eq!(table.worlds["echoIt"].tapes[0], SymSet::full(3));
    }

    #[test]
    fn a_malformed_write_vector_is_conservatively_full() {
        // The vector-width check lives in expansion, NOT in resolution, so a
        // source walk over `analyze` output really can meet a write vector
        // narrower than the world's arity — an editor sees it on every
        // half-typed rule. Walking it positionally would read the missing
        // cells as `keep` and UNDER-approximate, the one direction this
        // analysis may never take.
        let src = "\
alphabet bits { '_', '0', '1' }
alphabet wide { '_', 'a', 'b' }

machine {
  tape a: bits;
  tape b: wide;
  entry state s { [*, *] -> write ['1'] stop; }
}
";
        let resolved = resolve(src);
        let main = &infer_resolved(&resolved).worlds["main"].tapes;
        assert_eq!(main[0], SymSet::full(3));
        assert_eq!(
            main[1],
            SymSet::full(3),
            "the tape the short vector never reached must not read as keep"
        );
    }

    #[test]
    fn an_unresolved_site_naming_no_host_tape_is_full_on_every_tape() {
        // An arg that names no host tape tells this walk NOTHING about where
        // the callee writes — it may be a state continuation, a typo'd tape
        // name, or a `call` on a local graph that resolution routed to
        // `external`. Reading "names nothing recognizable" as "reaches
        // nothing" would make the answer non-monotone: adding a useless arg
        // would SHRINK it. Both shapes must match the argless site.
        let site = |args: &str| {
            let src = format!(
                "\
alphabet bits {{ '_', '0', '1' }}
alphabet wide {{ '_', 'a', 'b' }}

machine {{
  tape a: bits;
  tape b: wide;
  entry state s {{ [*, *] -> call other::helper({args}) then stop; }}
}}
"
            );
            let resolved = resolve(&src);
            infer_resolved(&resolved).worlds["main"].tapes.clone()
        };

        let argless = site("");
        assert_eq!(
            argless,
            vec![SymSet::full(3), SymSet::full(3)],
            "an argless site rides identity placement and reaches every tape"
        );
        assert_eq!(
            site("done = s"),
            argless,
            "a state-continuation arg identifies no tape, so nothing may be ruled out"
        );
        assert_eq!(
            site("num = notATape"),
            argless,
            "a target that names no host tape identifies no tape either"
        );
        assert_eq!(
            site("done = return"),
            argless,
            "a terminator arg binds no tape at all — it is a continuation"
        );
        // The precision is kept where an arg DOES identify a tape: the
        // fallback widens only when none of them does.
        assert_eq!(
            site("num = a, done = s"),
            vec![SymSet::full(3), SymSet::empty()],
            "one arg identifies tape a, so tape b stays out"
        );
    }

    #[test]
    fn an_unresolved_call_is_full_on_the_bound_tape_only() {
        let src = "\
alphabet bits { '_', '0', '1' }
alphabet wide { '_', 'a', 'b' }

machine {
  tape a: bits;
  tape b: wide;
  entry state s { [*, *] -> call other::helper(num = a) then stop; }
}
";
        let resolved = resolve(src);
        let table = infer_resolved(&resolved);
        let main = &table.worlds["main"].tapes;
        assert_eq!(
            main[0],
            SymSet::full(3),
            "a callee outside this unit may write anything on the tape it binds"
        );
        assert_eq!(main[1], SymSet::empty(), "tape b is not bound at the site");
    }

    // -- the standard library ---------------------------------------------

    /// The spec's load-bearing stdlib claim, in the source the claim is
    /// written on: `std.tmc`'s delimited `invertNumber` collapses its markers
    /// one-way onto the bare callee's blank and says they "survive the call
    /// because bare invert never writes a blank".
    #[test]
    fn bare_invert_never_writes_a_blank() {
        let resolved = resolve(crate::stdlib::SOURCE);
        let table = infer_resolved(&resolved);
        const BARE: &str = "std::binaryNumbersBare::invertNumber";

        let tapes = &table
            .worlds
            .get(BARE)
            .unwrap_or_else(|| panic!("{BARE} is in the table"))
            .tapes;
        let ix = |g: &str| index_of(&resolved, BARE, 0, g);
        assert_eq!(
            tapes[0],
            set(&[ix("0"), ix("1")]),
            "bare invert writes exactly the two digits"
        );
        assert!(
            !tapes[0].contains(0),
            "the claim the delimited caller relies on: no blank is ever written"
        );
    }

    /// The relation between the two walks, over the largest real program the
    /// crate carries: source ⊇ IR on every world both compute.
    #[test]
    fn the_stdlib_source_walk_covers_the_ir_walk() {
        let resolved = resolve(crate::stdlib::SOURCE);
        let source = infer_resolved(&resolved);
        let out =
            compile(crate::stdlib::SOURCE, CompileOptions::default()).expect("the stdlib compiles");
        let ir = infer_ir(&out.ir);

        assert!(ir.worlds.len() >= 10, "the stdlib has many worlds");
        for (name, iw) in &ir.worlds {
            let sw = source
                .worlds
                .get(name)
                .unwrap_or_else(|| panic!("{name} is in the source table too"));
            assert_eq!(sw.tapes.len(), iw.tapes.len(), "{name} arity");
            for (k, (s, i)) in sw.tapes.iter().zip(&iw.tapes).enumerate() {
                assert!(
                    s.is_superset(*i),
                    "{name} tape {k}: source {s:?} does not cover ir {i:?}"
                );
            }
        }
    }

    #[test]
    fn malformed_binding_arity_is_conservatively_full() {
        // Rejected upstream by `validate_world`; the analysis still answers
        // in the safe direction rather than projecting a partial binding.
        let p = program(vec![
            world(
                "caller",
                vec![tape("a", 3), tape("b", 4)],
                vec![rule(2, None, call("callee", bind1(0, &[])))],
            ),
            world(
                "callee",
                vec![tape("x", 3), tape("y", 4)],
                vec![rule(2, wr(&[Some(1), None]), IrTransition::Return)],
            ),
        ]);

        let table = infer_ir(&p);
        let caller = tapes_of(&table, "caller");
        assert_eq!(caller[0], SymSet::full(3));
        assert_eq!(caller[1], SymSet::full(4));
    }
}
