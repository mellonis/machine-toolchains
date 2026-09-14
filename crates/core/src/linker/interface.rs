//! Interface-aware link-stage resolution (docs/core.md (symbolic
//! resolution)). The object format carries a bound call in a SYMBOLIC
//! form — a parameter name instead of a list position, a glyph label
//! instead of a callee symbol index — because a compiler that has never
//! seen the callee cannot spell either as a number
//! (docs/formats.md (bound calls)). Turning them into numbers is the
//! link stage's own step, and it happens here, once, before the
//! composition engine reads a single binding.
//!
//! The resolved records live in an ARENA the caller owns: an
//! `ObjectFile` is never mutated, because a labelled pair carries `dst`
//! 0 by the writer's invariant and writing a resolved index into it
//! would break `from_bytes(to_bytes(x)) == x`
//! (docs/formats.md (bound calls)).
//!
//! Placed here rather than in `resolve` for the reason the old refusal
//! guard documented: `resolve` is shared with `resolve_names`, the
//! standalone name-resolution query the editor overlays run against,
//! which has no business failing over a binding.

use super::LinkError;
use super::resolve::FuncRef;
use crate::formats::object::BoundCall;

/// Resolve every reached bound call's binding against its callee's
/// interface. Returns one `Vec<BoundCall>` per function, parallel to
/// `FuncRef::bound`, holding the numeric records the engine consumes.
///
/// The resolved record preserves everything the symbolic one carried
/// EXCEPT the two symbolic fields: `caller_tape`, `map_written`, `open`,
/// `one_way`, `src` and `exits` all survive verbatim, and only `param`
/// and `dst_label` are cleared (with `dst` filled in). `open` in
/// particular is SEMANTIC, not symbolic — the composition algebra reads
/// it (docs/formats.md (bound calls)) — so a pre-pass that normalized it
/// away would silently close every open binding.
pub(super) fn resolve_bindings(order: &[FuncRef]) -> Result<Vec<Vec<BoundCall>>, LinkError> {
    order
        .iter()
        .map(|f| {
            f.bound
                .iter()
                .map(|&(_, callee, record)| resolve_one(&order[callee], record))
                .collect::<Result<Vec<_>, _>>()
        })
        .collect()
}

/// One site. The identity for now; each later resolution rule is added
/// here, one at a time.
fn resolve_one(_callee: &FuncRef, record: &BoundCall) -> Result<BoundCall, LinkError> {
    Ok(record.clone())
}

/// Re-point every `FuncRef::bound` entry at its arena record, so the
/// composition engine — and hybrid's second `scan_sites` pass over the
/// mono-rewritten order — can only ever see the resolved form. Takes
/// `order` BY VALUE: `FuncRef<'a>` is covariant in `'a`, so a
/// `Vec<FuncRef<'obj>>` coerces to `Vec<FuncRef<'arena>>` at the call;
/// `&mut Vec<FuncRef<'a>>` would be invariant and would not compile.
pub(super) fn rebind<'a>(
    mut order: Vec<FuncRef<'a>>,
    arena: &'a [Vec<BoundCall>],
) -> Vec<FuncRef<'a>> {
    for (f, resolved) in order.iter_mut().zip(arena) {
        debug_assert_eq!(
            f.bound.len(),
            resolved.len(),
            "the arena is parallel to FuncRef::bound"
        );
        for (slot, rec) in f.bound.iter_mut().zip(resolved) {
            slot.2 = rec;
        }
    }
    order
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::formats::object::{BoundCall, MapPair, ObjectFile, Symbol, SymbolDef, TapeBinding};

    /// One object, `main` bound-calling `sub` through a purely NUMERIC
    /// binding: positional entry (`param: None`), a closed map, a
    /// positional destination (`dst_label: None`), no exit vector. Nothing
    /// here is symbolic, so the pre-pass must be the identity over it.
    fn numeric_fixture() -> Vec<ObjectFile> {
        let symbols = ["main", "sub"]
            .iter()
            .enumerate()
            .map(|(i, n)| Symbol {
                name: (*n).into(),
                def: SymbolDef::Defined { blob: i as u32 },
            })
            .collect();
        // A minimal `ent; stp` body — valid code, never decoded here.
        let blobs = vec![vec![0x0E, 0x02], vec![0x0E, 0x02]];
        let mut object = ObjectFile::v2(0x7F, symbols, blobs, Vec::new(), None);
        object.bound_calls.push(BoundCall {
            blob: 0,
            offset: 1,
            symbol: 1, // "sub"
            binding: vec![TapeBinding {
                caller_tape: 0,
                param: None,
                map_written: true,
                open: false,
                pairs: vec![MapPair {
                    src: 0,
                    dst: 1,
                    dst_label: None,
                    one_way: false,
                }],
            }],
            exits: Vec::new(),
        });
        vec![object]
    }

    /// The pre-pass is the identity on a purely numeric binding, and
    /// `rebind` really re-points the `FuncRef`s at the arena: after it,
    /// every `bound` entry's record is the arena's own allocation, not
    /// the object's.
    ///
    /// Mutation it catches: make `rebind` a no-op (or have it point at
    /// anything other than the arena entry the pre-pass produced) and the
    /// `ptr::eq` assertion fails — which is precisely the failure the
    /// hybrid re-scan would otherwise hit silently, since the object's
    /// record and the arena's are EQUAL for a numeric binding and no
    /// value comparison can tell them apart.
    #[test]
    fn rebind_points_every_site_at_its_arena_record() {
        let objects = numeric_fixture();
        let order = crate::linker::resolve::resolve(&objects, &[], "main")
            .expect("the fixture resolves")
            .order;
        // Non-vacuity: every assertion below lives inside a zip over
        // `bound`, so an unattached site would make the whole test green
        // without checking anything.
        assert_eq!(
            order.iter().map(|f| f.bound.len()).sum::<usize>(),
            1,
            "the fixture must present exactly one bound site"
        );
        let arena = resolve_bindings(&order).expect("a numeric binding resolves");
        // The identity half: nothing symbolic, so nothing changed.
        for (f, resolved) in order.iter().zip(&arena) {
            for (&(_, _, original), r) in f.bound.iter().zip(resolved) {
                assert_eq!(original, r, "the pre-pass altered a numeric record");
            }
        }
        // The identity half again, sharper: equal but NOT the same object.
        for (f, resolved) in order.iter().zip(&arena) {
            for (&(_, _, original), r) in f.bound.iter().zip(resolved) {
                assert!(
                    !std::ptr::eq(original, r),
                    "the arena must own its records, not alias the object's"
                );
            }
        }
        let order = rebind(order, &arena);
        for (f, resolved) in order.iter().zip(&arena) {
            for (&(_, _, record), r) in f.bound.iter().zip(resolved) {
                assert!(
                    std::ptr::eq(record, r),
                    "`{}` still reads the object's record, not the arena's",
                    f.name
                );
            }
        }
        drop(objects);
    }
}
