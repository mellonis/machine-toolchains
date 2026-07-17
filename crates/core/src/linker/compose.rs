//! The composition algebra: absolute composites, binding ingestion, and
//! composite-on-composite composition (docs/formats.md (bound calls) — the
//! frame descriptor a bound call lowers to). Pure data plus a total
//! algebra: no I/O, no linker-pipeline coupling, no architecture knowledge.
//!
//! A **binding** (what a `call target [binding]` site declares) maps, for
//! each callee virtual tape, a caller tape and a set of caller-symbol to
//! callee-symbol pairs. Ordinary `->` pairs are bidirectional; one-way `=>`
//! pairs read only (several caller symbols may collapse onto one callee
//! symbol) and are excluded from write-back and from the injectivity checks.
//! The blank pair 0<->0 is always present and bidirectional; a non-blank
//! symbol may fold onto blank one-way, but blank never reads or writes as
//! anything else.
//!
//! A **composite** is the absolute form the composition engine works in:
//! per callee virtual tape, the physical machine tape plus a read map
//! (physical symbol -> virtual, the binding's read direction) and a write
//! map (virtual -> physical, the bidirectional pairs' inverses), each a
//! partial function whose gaps are **holes** that trap when crossed.
//!
//! Composition is the load-bearing operation: `compose_composites(E, F)`
//! projects `F`'s tapes through `E`, so `F`'s "caller" symbols are `E`'s
//! virtual symbols. Maps compose as functions; holes compose as outer holes
//! union the preimages of inner holes. Composites are closed under this
//! operation, so it is associative unconditionally — binding-level
//! partiality (one-way pairs, holes) never obstructs it, because the result
//! is always representable as a composite even when it is not representable
//! as a single binding.
//!
//! Maps are kept **sparse** here (explicit non-identity pairs plus an
//! explicit hole set, everything else identity); the dense u16 descriptor
//! tables are materialized only by the descriptor emitter in a later
//! phase-5b task.

use crate::formats::crc32::crc32;
use crate::formats::object::{RoutineSig, TapeBinding};
use std::collections::{BTreeMap, BTreeSet};

/// Highest representable symbol index. The dense descriptor map is u16 with
/// `0xFFFF` reserved as the hole sentinel (docs/formats.md (frame
/// descriptors)), so a symbol index must stay strictly below it.
const MAX_SYMBOL: u32 = 0xFFFE;

/// A partial symbol map with an implicit identity default: `pairs` lists
/// only the symbols that remap to a *different* symbol, `holes` lists the
/// symbols that trap when crossed, and every other symbol maps to itself.
/// The two sets are disjoint (a symbol is remapped, a hole, or identity —
/// never two of those). Canonical by construction: no identity pair is ever
/// stored.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct SparseMap {
    pairs: BTreeMap<u16, u16>,
    holes: BTreeSet<u16>,
}

impl SparseMap {
    /// The identity map: every symbol maps to itself, no holes.
    pub(crate) fn identity() -> Self {
        Self::default()
    }

    /// Apply the map. `None` marks a hole (crossing it traps); `Some(d)` is
    /// the image, with the implicit identity default for unlisted symbols.
    pub(crate) fn apply(&self, s: u16) -> Option<u16> {
        if self.holes.contains(&s) {
            None
        } else {
            Some(self.pairs.get(&s).copied().unwrap_or(s))
        }
    }

    /// True when the map is the identity (no holes, no non-identity pairs).
    /// Robust against a not-yet-canonicalized map that stored an `s -> s`
    /// pair.
    pub(crate) fn is_identity(&self) -> bool {
        self.holes.is_empty() && self.pairs.iter().all(|(s, d)| s == d)
    }

    /// Drop identity pairs and any pair shadowed by a hole, leaving the
    /// canonical form. Idempotent. Construction paths already keep the map
    /// canonical; this is the belt-and-braces normalizer the dedup key
    /// relies on.
    fn canonicalize(&mut self) {
        self.pairs.retain(|s, d| s != d && !self.holes.contains(s));
    }

    /// Append this map's contribution to the canonical serialization: pair
    /// count then sorted `(src, dst)` pairs, hole count then sorted holes,
    /// all little-endian. `BTreeMap`/`BTreeSet` iteration is already sorted,
    /// so the bytes are independent of the order pairs were inserted.
    fn write_key(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&(self.pairs.len() as u16).to_le_bytes());
        for (src, dst) in &self.pairs {
            out.extend_from_slice(&src.to_le_bytes());
            out.extend_from_slice(&dst.to_le_bytes());
        }
        out.extend_from_slice(&(self.holes.len() as u16).to_le_bytes());
        for hole in &self.holes {
            out.extend_from_slice(&hole.to_le_bytes());
        }
    }
}

/// One callee virtual tape's absolute placement: the physical machine tape
/// it projects onto, and its read/write maps. In a *relative* composite
/// (the direct lowering of one binding) `phys` indexes the caller's virtual
/// tapes rather than the machine; [`compose_composites`] resolves it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CompositeTape {
    pub phys: u8,
    pub rmap: SparseMap,
    pub wmap: SparseMap,
}

/// A composed call frame: the callee routine plus one [`CompositeTape`] per
/// callee virtual tape. The identity composite for an N-tape machine maps
/// tape `k` to physical tape `k` with identity maps and no holes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Composite {
    pub routine: usize,
    pub tapes: Vec<CompositeTape>,
}

/// Which map direction a conflict was found in — read (physical -> virtual)
/// or write (virtual -> physical).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MapDir {
    Read,
    Write,
}

impl std::fmt::Display for MapDir {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Read => "read",
            Self::Write => "write",
        })
    }
}

/// Why a binding could not be lowered to a composite. Module-local: the
/// linker wraps these into `LinkError` (adding the callee name it knows and
/// this module does not) in a later phase-5b task.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ComposeError {
    /// The binding has a different number of tapes than the callee's arity.
    Arity { expected: u8, got: usize },
    /// A binding tape names a caller tape the caller does not have.
    CallerTape {
        tape: usize,
        caller_tape: u8,
        caller_arity: usize,
    },
    /// A mapped symbol falls outside the callee tape's alphabet (or the u16
    /// wire bound). Only the callee side is range-checked here — the caller
    /// alphabet is the caller's to police, and this module is handed only
    /// the callee signature.
    SymbolRange {
        tape: usize,
        symbol: u32,
        cardinality: u32,
    },
    /// Two pairs disagree: the same symbol is mapped to two different
    /// images in one direction (a read collision, or a two-way write-back
    /// collision — the injectivity failure the completed-bijection rule
    /// forbids statically).
    Conflict {
        tape: usize,
        dir: MapDir,
        symbol: u16,
        existing: u16,
        incoming: u16,
    },
    /// A pair would move the blank symbol off itself: `0 -> x` (blank must
    /// read as blank) or a two-way `y -> 0` (its write-back `0 -> y` would
    /// un-pin blank; a read-only `y => 0` collapse is the legal spelling).
    Blank { tape: usize, src: u16, dst: u16 },
}

impl std::fmt::Display for ComposeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Arity { expected, got } => {
                write!(
                    f,
                    "binding arity {got} does not match callee arity {expected}"
                )
            }
            Self::CallerTape {
                tape,
                caller_tape,
                caller_arity,
            } => write!(
                f,
                "binding tape {tape} names caller tape {caller_tape}, \
                 but the caller has only {caller_arity} tapes"
            ),
            Self::SymbolRange {
                tape,
                symbol,
                cardinality,
            } => write!(
                f,
                "binding tape {tape} maps symbol {symbol} outside the \
                 alphabet (cardinality {cardinality})"
            ),
            Self::Conflict {
                tape,
                dir,
                symbol,
                existing,
                incoming,
            } => write!(
                f,
                "binding tape {tape} maps {dir} symbol {symbol} to both \
                 {existing} and {incoming}"
            ),
            Self::Blank { tape, src, dst } => {
                if *src == 0 {
                    write!(
                        f,
                        "binding tape {tape} maps blank symbol 0 to {dst}; blank is pinned"
                    )
                } else {
                    write!(
                        f,
                        "binding tape {tape} folds symbol {src} onto blank two-way; \
                         spell it => for a read-only collapse"
                    )
                }
            }
        }
    }
}

/// Lower one binding to a **relative** composite for `callee_routine`:
/// `phys` is the binding's `caller_tape` (an index into the caller's
/// virtual tapes, resolved later by [`compose_composites`]); the read map
/// carries every pair, the write map only the bidirectional ones. Validates
/// arity, caller-tape range, callee-symbol range, per-direction conflicts,
/// and blank pinning (the authority for GC7's mapping legality).
fn binding_to_composite(
    caller_arity: usize,
    callee_routine: usize,
    binding: &[TapeBinding],
    sig: &RoutineSig,
) -> Result<Composite, ComposeError> {
    if binding.len() != sig.arity as usize {
        return Err(ComposeError::Arity {
            expected: sig.arity,
            got: binding.len(),
        });
    }

    let mut tapes = Vec::with_capacity(binding.len());
    for (k, tb) in binding.iter().enumerate() {
        if usize::from(tb.caller_tape) >= caller_arity {
            return Err(ComposeError::CallerTape {
                tape: k,
                caller_tape: tb.caller_tape,
                caller_arity,
            });
        }
        // `RoutineSig` guarantees `cardinalities.len() == arity`; degrade to
        // a zero cardinality (which rejects every symbol) rather than panic
        // on an internally malformed signature.
        let card = sig.cardinalities.get(k).copied().unwrap_or(0);

        let mut rmap = SparseMap::identity();
        let mut wmap = SparseMap::identity();
        for pair in &tb.pairs {
            // Callee (dst) symbols must lie in the callee tape's alphabet;
            // both sides must fit the u16 wire form.
            if pair.dst > MAX_SYMBOL || pair.dst >= card {
                return Err(ComposeError::SymbolRange {
                    tape: k,
                    symbol: pair.dst,
                    cardinality: card,
                });
            }
            if pair.src > MAX_SYMBOL {
                return Err(ComposeError::SymbolRange {
                    tape: k,
                    symbol: pair.src,
                    cardinality: MAX_SYMBOL + 1,
                });
            }
            let src = pair.src as u16;
            let dst = pair.dst as u16;

            // Blank reads as blank, always.
            if src == 0 && dst != 0 {
                return Err(ComposeError::Blank { tape: k, src, dst });
            }
            // Read direction: every pair, one-way or not.
            insert_checked(&mut rmap, k, MapDir::Read, src, dst)?;

            // Write direction: bidirectional pairs only.
            if !pair.one_way {
                // A two-way fold onto blank would write blank back as a
                // non-blank symbol — forbidden. Blank collapses must be
                // one-way.
                if dst == 0 && src != 0 {
                    return Err(ComposeError::Blank { tape: k, src, dst });
                }
                insert_checked(&mut wmap, k, MapDir::Write, dst, src)?;
            }
        }

        rmap.canonicalize();
        wmap.canonicalize();
        tapes.push(CompositeTape {
            phys: tb.caller_tape,
            rmap,
            wmap,
        });
    }

    Ok(Composite {
        routine: callee_routine,
        tapes,
    })
}

/// Insert `src -> dst`, treating a repeat with a different image as a
/// conflict. Identity pairs are stored during ingestion so a later
/// disagreeing pair is still caught; `canonicalize` drops them afterwards.
fn insert_checked(
    map: &mut SparseMap,
    tape: usize,
    dir: MapDir,
    src: u16,
    dst: u16,
) -> Result<(), ComposeError> {
    match map.pairs.get(&src) {
        Some(&existing) if existing != dst => Err(ComposeError::Conflict {
            tape,
            dir,
            symbol: src,
            existing,
            incoming: dst,
        }),
        Some(_) => Ok(()),
        None => {
            map.pairs.insert(src, dst);
            Ok(())
        }
    }
}

/// Compose two symbol maps: `second ∘ first` (apply `first`, then
/// `second`). Holes propagate: the result holes are `first`'s holes plus
/// the preimages under `first` of `second`'s holes. Every symbol where the
/// composite could differ from identity has its trigger in one of the four
/// listed sets, so enumerating their union is exhaustive; everything else
/// stays identity.
fn compose_map(first: &SparseMap, second: &SparseMap) -> SparseMap {
    let mut out = SparseMap::identity();
    let candidates: BTreeSet<u16> = first
        .pairs
        .keys()
        .chain(first.holes.iter())
        .chain(second.pairs.keys())
        .chain(second.holes.iter())
        .copied()
        .collect();
    for s in candidates {
        match first.apply(s).and_then(|mid| second.apply(mid)) {
            None => {
                out.holes.insert(s);
            }
            Some(dst) if dst != s => {
                out.pairs.insert(s, dst);
            }
            Some(_) => {} // identity — left implicit
        }
    }
    out
}

/// Project `inner`'s tapes through `outer`: `inner`'s `phys` fields index
/// `outer`'s virtual tapes (the routine that produced `inner` runs under
/// `outer`, so its tapes *are* `outer`'s virtual tapes). The result is the
/// absolute composite reached by taking `outer` then `inner`. Total: both
/// operands are already validated, so composition never fails. The result
/// carries `inner`'s routine (the callee actually reached).
///
/// Contract: `inner.tapes[*].phys < outer.tapes.len()` — guaranteed by the
/// binding-ingestion caller-tape check.
pub(crate) fn compose_composites(outer: &Composite, inner: &Composite) -> Composite {
    let tapes = inner
        .tapes
        .iter()
        .map(|it| {
            let j = usize::from(it.phys);
            debug_assert!(j < outer.tapes.len(), "inner phys out of outer range");
            let ot = &outer.tapes[j];
            CompositeTape {
                phys: ot.phys,
                // read: physical -> outer-virtual -> inner-virtual
                rmap: compose_map(&ot.rmap, &it.rmap),
                // write: inner-virtual -> outer-virtual -> physical
                wmap: compose_map(&it.wmap, &ot.wmap),
            }
        })
        .collect();
    Composite {
        routine: inner.routine,
        tapes,
    }
}

/// The identity composite for an `arity`-tape machine running `routine`:
/// tape `k` -> physical `k`, identity maps, no holes. `compose_composites`
/// with this as `outer` absolutizes an inner relative composite; with it as
/// `inner` it is a no-op.
pub(crate) fn identity_composite(arity: usize, routine: usize) -> Composite {
    let tapes = (0..arity)
        .map(|k| CompositeTape {
            phys: k as u8,
            rmap: SparseMap::identity(),
            wmap: SparseMap::identity(),
        })
        .collect();
    Composite { routine, tapes }
}

/// Absolutize a binding at the machine's top frame (the identity context):
/// the caller *is* the machine, so `caller_tape` is already a physical
/// tape. Equivalent to `compose(identity, ...)`.
pub(crate) fn absolutize(
    machine_arity: usize,
    callee_routine: usize,
    binding: &[TapeBinding],
    sig: &RoutineSig,
) -> Result<Composite, ComposeError> {
    binding_to_composite(machine_arity, callee_routine, binding, sig)
}

/// Compose an outer composite with a binding declared at a call site inside
/// `outer`'s routine: the binding's caller tapes index `outer`'s virtual
/// tapes. Validates the binding against `outer`'s arity and the callee
/// signature, then projects it through `outer` (GC6/GC7).
pub(crate) fn compose(
    outer: &Composite,
    callee_routine: usize,
    binding: &[TapeBinding],
    sig: &RoutineSig,
) -> Result<Composite, ComposeError> {
    let inner = binding_to_composite(outer.tapes.len(), callee_routine, binding, sig)?;
    Ok(compose_composites(outer, &inner))
}

/// True when the composite is the identity endomorphism on its own tapes:
/// tape `k` -> physical `k`, identity maps, no holes. This inspects only the
/// SPARSE maps — it says nothing about the target alphabet's width, so
/// [`is_full_passthrough`] (not this) is the authority for §5.6's collapse.
pub(crate) fn is_identity(c: &Composite) -> bool {
    c.tapes
        .iter()
        .enumerate()
        .all(|(k, t)| usize::from(t.phys) == k && t.rmap.is_identity() && t.wmap.is_identity())
}

/// True when the composite is a genuine full pass-through into `callee` and
/// may be lowered to a plain `call` (§5.6 identity collapse): the callee
/// inherits the active frame and reads/writes the domain's symbols directly,
/// with no translation and no trap ever owed.
///
/// Beyond [`is_identity`] (identity placement and identity sparse maps —
/// hence hole-free in the SPARSE form the algebra stores), a true
/// pass-through also needs the domain and callee alphabets to match on every
/// tape. The sparse composite does NOT encode cardinality-truncation holes:
/// an identity map into a NARROWER callee reads every domain symbol as
/// itself, yet domain symbols at or past the callee's cardinality have no
/// image there — a read hole the materialized descriptor (frames) or a mono
/// stamp's synthesized trap rows turn into an `UnmappedRead` trap. Into a
/// WIDER callee the callee may write symbols with no domain image, a write
/// hole trapping `UnmappedWrite`. Collapsing such a site to a plain call
/// discards those traps and lets the out-of-range symbol flow through raw, so
/// the per-tape cardinalities MUST be equal for the collapse to be sound
/// (docs/formats.md (bound calls)).
///
/// `is_identity` forces tape `k` onto domain tape `k`, so comparing the two
/// cardinality vectors elementwise is exactly the per-tape check; unequal
/// arity compares unequal (different lengths) and correctly refuses. Both the
/// site-level (`engine`, absolutized at the caller) and in-stamp
/// (`stamp`, composed at the machine) collapse decisions call this one
/// predicate, so they cannot drift.
pub(crate) fn is_full_passthrough(
    c: &Composite,
    domain_sig: &RoutineSig,
    callee_sig: &RoutineSig,
) -> bool {
    is_identity(c)
        && c.tapes.len() == domain_sig.arity as usize
        && domain_sig.cardinalities == callee_sig.cardinalities
}

/// Normalize in place: canonicalize every tape's maps. Idempotent. The
/// dedup key and digest are only stable across a canonicalized composite.
// Consumed by the map-sidecar binding-label renderer (a later phase-5b
// task); the engine reaches composites already canonical by construction.
#[allow(dead_code)]
pub(crate) fn canonicalize(c: &mut Composite) {
    for tape in &mut c.tapes {
        tape.rmap.canonicalize();
        tape.wmap.canonicalize();
    }
}

/// The dedup key: a deterministic byte serialization of the composite —
/// routine, tape count, then per tape the physical index and both maps.
/// Two composites share a key iff they are the same instantiation (same
/// callee, same absolute placement). Independent of the order pairs were
/// inserted, since the sparse maps are `BTree`-ordered. Canonicalize first
/// for a stable key across pre-/post-normalization forms.
pub(crate) fn canonical_key(c: &Composite) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&(c.routine as u64).to_le_bytes());
    out.extend_from_slice(&(c.tapes.len() as u16).to_le_bytes());
    for tape in &c.tapes {
        out.push(tape.phys);
        tape.rmap.write_key(&mut out);
        tape.wmap.write_key(&mut out);
    }
    out
}

/// A 32-bit content address for a composite: CRC-32 of its
/// [`canonical_key`] (the container checksum algorithm — docs/formats.md
/// (bound calls)). Stable across builds and across pair-insertion order.
// The digest is the map-sidecar label's collision fallback (a later
// phase-5b task); the engine dedups on the full `canonical_key` directly.
#[allow(dead_code)]
pub(crate) fn digest(c: &Composite) -> u32 {
    crc32(&canonical_key(c))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::formats::object::MapPair;
    use proptest::prelude::*;

    // ----- fixture builders -------------------------------------------------

    fn sig(cardinalities: &[u32]) -> RoutineSig {
        RoutineSig {
            arity: cardinalities.len() as u8,
            cardinalities: cardinalities.to_vec(),
        }
    }

    fn pair(src: u32, dst: u32, one_way: bool) -> MapPair {
        MapPair { src, dst, one_way }
    }

    fn tape(caller_tape: u8, pairs: Vec<MapPair>) -> TapeBinding {
        TapeBinding { caller_tape, pairs }
    }

    /// A composite tape built directly from explicit pairs and holes, so
    /// tests can exercise holes the ingestion path never mints on its own.
    fn ctape(
        phys: u8,
        rpairs: &[(u16, u16)],
        rholes: &[u16],
        wpairs: &[(u16, u16)],
        wholes: &[u16],
    ) -> CompositeTape {
        let mk = |pairs: &[(u16, u16)], holes: &[u16]| {
            let mut m = SparseMap::identity();
            for &(s, d) in pairs {
                m.pairs.insert(s, d);
            }
            for &h in holes {
                m.holes.insert(h);
            }
            m.canonicalize();
            m
        };
        CompositeTape {
            phys,
            rmap: mk(rpairs, rholes),
            wmap: mk(wpairs, wholes),
        }
    }

    // ----- ingestion: legality (GC7) ---------------------------------------

    #[test]
    fn arity_mismatch_is_rejected() {
        let s = sig(&[4, 4]);
        let err = absolutize(3, 0, &[tape(0, vec![])], &s).unwrap_err();
        assert_eq!(
            err,
            ComposeError::Arity {
                expected: 2,
                got: 1
            }
        );
    }

    #[test]
    fn caller_tape_out_of_range_is_rejected() {
        let s = sig(&[4]);
        let err = absolutize(2, 0, &[tape(5, vec![])], &s).unwrap_err();
        assert!(matches!(
            err,
            ComposeError::CallerTape { caller_tape: 5, .. }
        ));
    }

    #[test]
    fn callee_symbol_out_of_cardinality_is_rejected() {
        let s = sig(&[3]); // symbols 0,1,2
        let err = absolutize(1, 0, &[tape(0, vec![pair(1, 7, false)])], &s).unwrap_err();
        assert!(matches!(
            err,
            ComposeError::SymbolRange {
                symbol: 7,
                cardinality: 3,
                ..
            }
        ));
    }

    #[test]
    fn conflicting_read_pairs_are_rejected() {
        let s = sig(&[5]);
        // caller symbol 1 mapped to two different callee symbols.
        let err = absolutize(
            1,
            0,
            &[tape(0, vec![pair(1, 2, false), pair(1, 3, false)])],
            &s,
        )
        .unwrap_err();
        assert!(matches!(
            err,
            ComposeError::Conflict {
                dir: MapDir::Read,
                symbol: 1,
                ..
            }
        ));
    }

    #[test]
    fn two_way_write_collision_is_rejected() {
        let s = sig(&[5]);
        // two distinct caller symbols fold two-way onto callee 2 => the
        // write-back is ambiguous (injectivity failure).
        let err = absolutize(
            1,
            0,
            &[tape(0, vec![pair(1, 2, false), pair(3, 2, false)])],
            &s,
        )
        .unwrap_err();
        assert!(matches!(
            err,
            ComposeError::Conflict {
                dir: MapDir::Write,
                symbol: 2,
                ..
            }
        ));
    }

    #[test]
    fn one_way_collapse_is_excluded_from_the_write_direction() {
        let s = sig(&[5]);
        // the same collapse, but one-way: legal, because `=>` does not
        // contribute to write-back, so there is no injectivity failure.
        let c = absolutize(
            1,
            0,
            &[tape(0, vec![pair(1, 2, true), pair(3, 2, true)])],
            &s,
        )
        .unwrap();
        // read collapses both onto 2; write-back is pure identity (no pairs).
        assert_eq!(c.tapes[0].rmap.apply(1), Some(2));
        assert_eq!(c.tapes[0].rmap.apply(3), Some(2));
        assert!(c.tapes[0].wmap.pairs.is_empty());
    }

    #[test]
    fn blank_read_pin_is_enforced() {
        let s = sig(&[5]);
        let err = absolutize(1, 0, &[tape(0, vec![pair(0, 2, false)])], &s).unwrap_err();
        assert!(matches!(err, ComposeError::Blank { src: 0, dst: 2, .. }));
    }

    #[test]
    fn two_way_collapse_onto_blank_is_rejected_but_one_way_is_allowed() {
        let s = sig(&[5]);
        // two-way `2 -> 0` would un-pin blank on write-back.
        let err = absolutize(1, 0, &[tape(0, vec![pair(2, 0, false)])], &s).unwrap_err();
        assert!(matches!(err, ComposeError::Blank { src: 2, dst: 0, .. }));
        // one-way `2 => 0` is the canonical marker collapse — legal.
        let c = absolutize(1, 0, &[tape(0, vec![pair(2, 0, true)])], &s).unwrap();
        assert_eq!(c.tapes[0].rmap.apply(2), Some(0));
        assert!(c.tapes[0].wmap.pairs.is_empty());
    }

    // ----- absolutize / identity -------------------------------------------

    #[test]
    fn absolutize_projects_and_maps() {
        let s = sig(&[4, 4]);
        // callee tape 0 <- caller tape 2 with 1<->3, 2=>0; tape 1 <- caller 0.
        let binding = vec![
            tape(2, vec![pair(1, 3, false), pair(2, 0, true)]),
            tape(0, vec![]),
        ];
        let c = absolutize(4, 9, &binding, &s).unwrap();
        assert_eq!(c.routine, 9);
        assert_eq!(c.tapes[0].phys, 2);
        assert_eq!(c.tapes[0].rmap.apply(1), Some(3)); // read
        assert_eq!(c.tapes[0].rmap.apply(2), Some(0)); // one-way collapse
        assert_eq!(c.tapes[0].wmap.apply(3), Some(1)); // two-way inverse
        assert_eq!(c.tapes[0].wmap.apply(2), Some(2)); // one-way excluded => identity
        assert_eq!(c.tapes[1].phys, 0);
        assert!(is_identity(&identity_composite(4, 0)));
        assert!(!is_identity(&c));
    }

    #[test]
    fn compose_threads_a_binding_through_a_non_identity_outer() {
        // Outer E: one callee tape on physical tape 2, reading physical 4 as
        // virtual 1 (and back on write).
        let e = Composite {
            routine: 1,
            tapes: vec![ctape(2, &[(4, 1)], &[], &[(1, 4)], &[])],
        };
        // Inner binding at a call site inside E's routine: callee tape 0
        // binds E's virtual tape 0, remapping virtual 1 -> callee 3.
        let s = sig(&[4]);
        let composed = compose(&e, 2, &[tape(0, vec![pair(1, 3, false)])], &s).unwrap();
        assert_eq!(composed.routine, 2);
        // physical resolves through E: callee tape 0 lands on physical tape 2.
        assert_eq!(composed.tapes[0].phys, 2);
        // read: physical 4 -> E-virtual 1 -> callee 3.
        assert_eq!(composed.tapes[0].rmap.apply(4), Some(3));
        // write: callee 3 -> E-virtual 1 -> physical 4.
        assert_eq!(composed.tapes[0].wmap.apply(3), Some(4));
        // compose validates the binding against the OUTER arity (1 tape here).
        let bad = compose(&e, 2, &[tape(1, vec![])], &s);
        assert!(matches!(
            bad,
            Err(ComposeError::CallerTape { caller_tape: 1, .. })
        ));
    }

    #[test]
    fn identity_binding_absolutizes_to_the_identity_composite() {
        let s = sig(&[4, 4]);
        // pass each callee tape straight through, no symbol remaps.
        let binding = vec![tape(0, vec![]), tape(1, vec![])];
        let c = absolutize(2, 7, &binding, &s).unwrap();
        assert!(is_identity(&c));
    }

    // ----- identity laws ----------------------------------------------------

    #[test]
    fn left_identity_law() {
        // id ∘ F == F exactly (routine included).
        let f = Composite {
            routine: 3,
            tapes: vec![ctape(1, &[(1, 2)], &[3], &[(2, 1)], &[])],
        };
        let id = identity_composite(2, 99);
        let composed = compose_composites(&id, &f);
        assert_eq!(canonical_key(&composed), canonical_key(&f));
    }

    #[test]
    fn right_identity_law() {
        // E ∘ id == E exactly, with id carrying E's routine.
        let e = Composite {
            routine: 5,
            tapes: vec![
                ctape(0, &[(1, 2)], &[], &[(2, 1)], &[]),
                ctape(2, &[(3, 4)], &[5], &[(4, 3)], &[]),
            ],
        };
        let id = identity_composite(e.tapes.len(), e.routine);
        let composed = compose_composites(&e, &id);
        assert_eq!(canonical_key(&composed), canonical_key(&e));
    }

    #[test]
    fn is_identity_iff_no_op_on_compose() {
        // A composite the constructor labels identity is a no-op as the
        // inner operand of any compatible outer.
        let e = Composite {
            routine: 8,
            tapes: vec![ctape(0, &[(1, 2)], &[3], &[(2, 1)], &[])],
        };
        let id = identity_composite(1, 8);
        assert!(is_identity(&id));
        assert_eq!(
            canonical_key(&compose_composites(&e, &id)),
            canonical_key(&e)
        );
    }

    // ----- full-pass-through collapse predicate ----------------------------

    #[test]
    fn is_full_passthrough_requires_equal_cardinalities() {
        // Equal-size identity across every tape: a true pass-through, safe to
        // collapse to a plain call.
        assert!(is_full_passthrough(
            &identity_composite(2, 7),
            &sig(&[4, 4]),
            &sig(&[4, 4])
        ));
        // A NARROWER callee: the identity map hides a read hole (domain
        // symbol 3 has no image in a 3-symbol callee) — NOT a pass-through.
        assert!(!is_full_passthrough(
            &identity_composite(1, 0),
            &sig(&[4]),
            &sig(&[3])
        ));
        // A WIDER callee: it can write virtual symbol 3 with no domain image
        // (a write hole) — NOT a pass-through either. "Hole-free in BOTH
        // directions" is what equal cardinality buys.
        assert!(!is_full_passthrough(
            &identity_composite(1, 0),
            &sig(&[3]),
            &sig(&[4])
        ));
        // Per-tape, not aggregate: one narrower tape is enough to refuse.
        assert!(!is_full_passthrough(
            &identity_composite(2, 0),
            &sig(&[4, 4]),
            &sig(&[4, 3])
        ));
        // A non-identity composite is never a pass-through, regardless of
        // matching cardinalities.
        let swap = Composite {
            routine: 0,
            tapes: vec![ctape(0, &[(1, 2), (2, 1)], &[], &[(1, 2), (2, 1)], &[])],
        };
        assert!(!is_full_passthrough(&swap, &sig(&[4]), &sig(&[4])));
        // A projecting identity (fewer tapes than the domain) fails the arity
        // gate before cardinality is even considered.
        assert!(!is_full_passthrough(
            &identity_composite(1, 0),
            &sig(&[4, 4]),
            &sig(&[4])
        ));
    }

    // ----- canonicalize / digest -------------------------------------------

    #[test]
    fn canonicalize_is_idempotent_and_elides_identity_pairs() {
        let mut m = SparseMap::identity();
        m.pairs.insert(3, 3); // identity pair, should be dropped
        m.pairs.insert(1, 2);
        m.holes.insert(4);
        m.pairs.insert(4, 9); // shadowed by the hole, should be dropped
        let mut c = Composite {
            routine: 0,
            tapes: vec![CompositeTape {
                phys: 0,
                rmap: m,
                wmap: SparseMap::identity(),
            }],
        };
        canonicalize(&mut c);
        let once = canonical_key(&c);
        canonicalize(&mut c);
        assert_eq!(once, canonical_key(&c), "canonicalize is idempotent");
        assert_eq!(c.tapes[0].rmap.pairs.get(&3), None);
        assert_eq!(c.tapes[0].rmap.pairs.get(&4), None);
        assert_eq!(c.tapes[0].rmap.pairs.get(&1), Some(&2));
    }

    #[test]
    fn digest_is_stable_across_pair_insertion_order() {
        let mut a = SparseMap::identity();
        for &(s, d) in &[(1u16, 5u16), (2, 6), (3, 7)] {
            a.pairs.insert(s, d);
        }
        let mut b = SparseMap::identity();
        for &(s, d) in &[(3u16, 7u16), (1, 5), (2, 6)] {
            b.pairs.insert(s, d);
        }
        let ca = Composite {
            routine: 1,
            tapes: vec![CompositeTape {
                phys: 0,
                rmap: a,
                wmap: SparseMap::identity(),
            }],
        };
        let cb = Composite {
            routine: 1,
            tapes: vec![CompositeTape {
                phys: 0,
                rmap: b,
                wmap: SparseMap::identity(),
            }],
        };
        assert_eq!(digest(&ca), digest(&cb));
        assert_eq!(canonical_key(&ca), canonical_key(&cb));
    }

    #[test]
    fn digest_distinguishes_routine_and_maps() {
        let base = Composite {
            routine: 1,
            tapes: vec![ctape(0, &[(1, 2)], &[], &[(2, 1)], &[])],
        };
        let other_routine = Composite {
            routine: 2,
            ..base.clone()
        };
        let other_map = Composite {
            routine: 1,
            tapes: vec![ctape(0, &[(1, 3)], &[], &[(3, 1)], &[])],
        };
        assert_ne!(digest(&base), digest(&other_routine));
        assert_ne!(digest(&base), digest(&other_map));
    }

    // ----- property tests: the load-bearing oracle -------------------------

    /// A sparse map over the alphabet `0..card`, blank kept pinned (0 is
    /// never a hole and never remapped). Pairs and holes are disjoint.
    fn arb_map(card: u16) -> impl Strategy<Value = SparseMap> {
        let syms: Vec<u16> = (1..card).collect();
        // for each non-blank symbol: identity, remap to some symbol, or hole
        proptest::collection::vec(0u8..3, syms.len()).prop_flat_map(move |kinds| {
            let syms = syms.clone();
            let dst_choices = proptest::collection::vec(0u16..card, syms.len());
            dst_choices.prop_map(move |dsts| {
                let mut m = SparseMap::identity();
                for (i, &s) in syms.iter().enumerate() {
                    match kinds[i] {
                        1 => {
                            m.pairs.insert(s, dsts[i]);
                        }
                        2 => {
                            m.holes.insert(s);
                        }
                        _ => {}
                    }
                }
                m.canonicalize();
                m
            })
        })
    }

    fn arb_ctape(outer_arity: usize, card: u16) -> impl Strategy<Value = CompositeTape> {
        (0..outer_arity, arb_map(card), arb_map(card)).prop_map(|(phys, rmap, wmap)| {
            CompositeTape {
                phys: phys as u8,
                rmap,
                wmap,
            }
        })
    }

    /// A composite of `arity` tapes whose phys indices land inside
    /// `outer_arity`, so it composes onto an `outer_arity`-tape composite.
    fn arb_composite(
        routine: usize,
        arity: usize,
        outer_arity: usize,
        card: u16,
    ) -> impl Strategy<Value = Composite> {
        proptest::collection::vec(arb_ctape(outer_arity, card), arity)
            .prop_map(move |tapes| Composite { routine, tapes })
    }

    proptest! {
        /// The load-bearing oracle: for every physical symbol, walking the
        /// two composites step by step yields the same read result (value or
        /// hole) as the single composed read map, and likewise for writes.
        /// This is the direct check of GC6's hole law
        /// (outer holes ∪ preimages of inner holes) and of map composition.
        #[test]
        fn compose_matches_step_by_step_simulation(
            outer in arb_composite(1, 3, 3, 6),
            inner in arb_composite(2, 3, 3, 6),
        ) {
            let card: u16 = 6;
            let result = compose_composites(&outer, &inner);
            prop_assert_eq!(result.routine, inner.routine);
            for (k, it) in inner.tapes.iter().enumerate() {
                let ot = &outer.tapes[usize::from(it.phys)];
                prop_assert_eq!(result.tapes[k].phys, ot.phys);
                for p in 0..card {
                    // read: physical -> outer-virtual -> inner-virtual
                    let expected_read = ot.rmap.apply(p).and_then(|m| it.rmap.apply(m));
                    prop_assert_eq!(result.tapes[k].rmap.apply(p), expected_read,
                        "read mismatch at tape {}, symbol {}", k, p);
                    // write: inner-virtual -> outer-virtual -> physical
                    let expected_write = it.wmap.apply(p).and_then(|m| ot.wmap.apply(m));
                    prop_assert_eq!(result.tapes[k].wmap.apply(p), expected_write,
                        "write mismatch at tape {}, symbol {}", k, p);
                }
            }
        }

        /// Associativity on the composite chain: `(A ∘ B) ∘ C` and
        /// `A ∘ (B ∘ C)` are byte-identical composites. Composites are closed
        /// under composition, so this holds unconditionally — no binding-level
        /// partiality can obstruct it.
        #[test]
        fn composition_is_associative(
            a in arb_composite(10, 3, 3, 6),
            b in arb_composite(20, 3, 3, 6),
            c in arb_composite(30, 3, 3, 6),
        ) {
            let left = compose_composites(&compose_composites(&a, &b), &c);
            let right = compose_composites(&a, &compose_composites(&b, &c));
            prop_assert_eq!(canonical_key(&left), canonical_key(&right));
        }

        /// Both identity laws: id ∘ F == F and E ∘ id == E (routine matched).
        #[test]
        fn identity_laws_hold(f in arb_composite(4, 3, 3, 6)) {
            let left_id = identity_composite(3, 4);
            prop_assert_eq!(
                canonical_key(&compose_composites(&left_id, &f)),
                canonical_key(&f)
            );
            let right_id = identity_composite(f.tapes.len(), f.routine);
            prop_assert_eq!(
                canonical_key(&compose_composites(&f, &right_id)),
                canonical_key(&f)
            );
        }

        /// canonical_key round-trips through an extra canonicalize unchanged,
        /// and digest agrees with it.
        #[test]
        fn canonicalize_stable_under_repetition(f in arb_composite(0, 3, 3, 6)) {
            let mut once = f.clone();
            canonicalize(&mut once);
            let key = canonical_key(&once);
            let d = digest(&once);
            canonicalize(&mut once);
            prop_assert_eq!(canonical_key(&once), key);
            prop_assert_eq!(digest(&once), d);
        }
    }
}
