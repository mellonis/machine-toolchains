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

// Nothing in the crate reads the table at this point in the build-out, and
// `SymSet` deliberately ships as a whole set primitive rather than trimmed to
// today's call sites: a set type missing `len` or `is_superset` is a worse
// primitive than one carrying an unused method.
#![allow(dead_code)]

use std::collections::HashMap;
use std::fmt;

use crate::ir::{IrMapPair, IrProgram, IrTapeBinding, IrTransition, IrWorld, IrWrite};

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
    pub(crate) fn len(&self) -> u32 {
        self.0.count_ones()
    }

    /// Whether the set is empty.
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

#[cfg(test)]
mod tests {
    use super::*;
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
