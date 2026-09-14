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
use crate::formats::object::{BoundCall, RoutineInterface, TapeBinding};

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

/// One site's binding, resolved against the callee's interface.
fn resolve_one(callee: &FuncRef, record: &BoundCall) -> Result<BoundCall, LinkError> {
    let named = record
        .binding
        .iter()
        .filter(|tb| tb.param.is_some())
        .count();
    if named != 0 && named != record.binding.len() {
        return Err(bad(
            callee,
            "the binding mixes named and positional entries; write every entry \
             one way or the other"
                .to_string(),
        ));
    }
    let labelled = record
        .binding
        .iter()
        .any(|tb| tb.pairs.iter().any(|p| p.dst_label.is_some()));
    let open = record.binding.iter().any(|tb| tb.open);
    if named == 0 && !labelled && !open && record.exits.is_empty() {
        return Ok(record.clone());
    }

    // Names first: after the reorder, entry `k` IS callee tape `k`, which
    // is what makes the per-tape glyph list the right one to look a label
    // up in.
    let mut binding = if named == 0 {
        record.binding.clone()
    } else {
        let iface = require_interface(callee, "a named entry")?;
        reorder_named(callee, iface, &record.binding)?
    };
    if labelled {
        let iface = require_interface(callee, "a glyph-labelled destination")?;
        resolve_labels(callee, iface, &mut binding)?;
    }
    if open {
        check_opaque(callee, &binding)?;
    }
    if !record.exits.is_empty() {
        let iface = require_interface(callee, "an exit vector")?;
        if record.exits.len() != usize::from(iface.exits) {
            return Err(bad(
                callee,
                format!(
                    "the call site supplies {} exit(s), but `{}` declares {}",
                    record.exits.len(),
                    callee.name,
                    iface.exits
                ),
            ));
        }
    }
    Ok(BoundCall {
        binding,
        ..record.clone()
    })
}

/// Turn every `dst_label` into the glyph's position in the callee's
/// declared alphabet for that tape (docs/formats.md (bound calls)). The
/// label is cleared as it is consumed, so nothing downstream can read a
/// stale one.
fn resolve_labels(
    callee: &FuncRef,
    iface: &RoutineInterface,
    binding: &mut [TapeBinding],
) -> Result<(), LinkError> {
    for (k, tb) in binding.iter_mut().enumerate() {
        let Some(glyphs) = iface.glyphs.get(k) else {
            return Err(bad(
                callee,
                format!(
                    "binding tape {k} is outside `{}`'s declared interface \
                     ({} parameter(s))",
                    callee.name,
                    iface.glyphs.len()
                ),
            ));
        };
        for pair in tb.pairs.iter_mut() {
            let Some(label) = pair.dst_label.take() else {
                continue;
            };
            let Some(idx) = glyphs.iter().position(|g| *g == label) else {
                return Err(bad(
                    callee,
                    format!(
                        "binding tape {k} names glyph `{label}`, which is not in \
                         `{}`'s alphabet for parameter `{}`",
                        callee.name,
                        iface
                            .params
                            .get(k)
                            .map_or_else(|| k.to_string(), String::clone)
                    ),
                ));
            };
            pair.dst = u32::try_from(idx).expect("a glyph index fits u32");
        }
    }
    Ok(())
}

/// Refuse an open binding into a tape the callee does not declare
/// opaque, and into a callee that describes no interface at all
/// (docs/core.md (symbolic resolution)). Runs after the reorder, so
/// entry `k` is callee tape `k`.
fn check_opaque(callee: &FuncRef, binding: &[TapeBinding]) -> Result<(), LinkError> {
    for (k, tb) in binding.iter().enumerate() {
        if !tb.open {
            continue;
        }
        let opaque = callee
            .interface
            .and_then(|i| i.opaque.get(k).copied())
            .unwrap_or(false);
        if !opaque {
            return Err(LinkError::OpenBindingUnsupported {
                callee: callee.name.to_string(),
                tape: k,
                param: callee.interface.and_then(|i| i.params.get(k)).cloned(),
            });
        }
    }
    Ok(())
}

/// The callee's interface, or the refusal that replaces
/// `external-binding-unsupported` for a callee that describes none
/// (docs/core.md (symbolic resolution)).
fn require_interface<'a>(
    callee: &FuncRef<'a>,
    form: &str,
) -> Result<&'a RoutineInterface, LinkError> {
    callee.interface.ok_or_else(|| {
        bad(
            callee,
            format!(
                "the call site uses {form}, but `{}` describes no interface; \
                 only a transparent call can reach it",
                callee.name
            ),
        )
    })
}

/// Reorder a fully-named binding into the callee's own tape order,
/// clearing `param` as it goes. Every parameter must be bound exactly
/// once: a binding names every entry or none
/// (docs/tmt/language.md (symbol maps)).
fn reorder_named(
    callee: &FuncRef,
    iface: &RoutineInterface,
    binding: &[TapeBinding],
) -> Result<Vec<TapeBinding>, LinkError> {
    let mut slots: Vec<Option<TapeBinding>> = vec![None; iface.params.len()];
    for tb in binding {
        let name = tb.param.as_deref().expect("checked fully named");
        let Some(k) = iface.params.iter().position(|p| p == name) else {
            return Err(bad(
                callee,
                format!(
                    "the binding names parameter `{name}`, which `{}` does not declare",
                    callee.name
                ),
            ));
        };
        if slots[k].is_some() {
            return Err(bad(
                callee,
                format!("the binding names parameter `{name}` twice"),
            ));
        }
        slots[k] = Some(TapeBinding {
            param: None,
            ..tb.clone()
        });
    }
    slots
        .into_iter()
        .enumerate()
        .map(|(k, slot)| {
            slot.ok_or_else(|| {
                bad(
                    callee,
                    format!(
                        "the binding does not bind parameter `{}`; an argument list \
                         is complete",
                        iface.params[k]
                    ),
                )
            })
        })
        .collect()
}

fn bad(callee: &FuncRef, message: String) -> LinkError {
    LinkError::BadBinding {
        callee: callee.name.to_string(),
        message,
    }
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
    debug_assert_eq!(order.len(), arena.len(), "the arena is parallel to `order`");
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
    use crate::formats::object::{
        BoundCall, Interface, MapPair, ObjectFile, Symbol, SymbolDef, TapeBinding,
    };

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

    /// A minimal one-tape `RoutineInterface`: one parameter over the given
    /// glyphs, no writes/enters/leaves/exits contract.
    fn one_param_interface(param: &str, glyphs: &[&str]) -> RoutineInterface {
        RoutineInterface {
            params: vec![param.to_string()],
            glyphs: vec![glyphs.iter().map(|g| (*g).to_string()).collect()],
            writes: vec![Vec::new()],
            enters: vec![None],
            leaves: vec![None],
            opaque: vec![false],
            exits: 0,
            returns: true,
        }
    }

    /// A zero-tape placeholder interface — `main` never plays callee in
    /// this fixture, so its own entry only needs to exist so `sub`'s sits
    /// at the right index in `Interface::routines` (parallel to `blobs`).
    fn empty_interface() -> RoutineInterface {
        RoutineInterface {
            params: Vec::new(),
            glyphs: Vec::new(),
            writes: Vec::new(),
            enters: Vec::new(),
            leaves: Vec::new(),
            opaque: Vec::new(),
            exits: 0,
            returns: true,
        }
    }

    /// `main` bound-calling `sub` (one parameter `p` over `('_', 'a')`)
    /// through a single labelled pair: `src: 0`, `dst_label: Some("a")`,
    /// the `dst: 0` a labelled pair is written with.
    fn labelled_fixture() -> Vec<ObjectFile> {
        let symbols = ["main", "sub"]
            .iter()
            .enumerate()
            .map(|(i, n)| Symbol {
                name: (*n).into(),
                def: SymbolDef::Defined { blob: i as u32 },
            })
            .collect();
        let blobs = vec![vec![0x0E, 0x02], vec![0x0E, 0x02]];
        let mut object = ObjectFile::v2(0x7F, symbols, blobs, Vec::new(), None);
        object.interface = Some(Interface {
            routines: vec![empty_interface(), one_param_interface("p", &["_", "a"])],
            ..Default::default()
        });
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
                    dst: 0,
                    dst_label: Some("a".to_string()),
                    one_way: false,
                }],
            }],
            exits: Vec::new(),
        });
        vec![object]
    }

    /// `resolve_labels` CLEARS the label it consumes, not merely reads it:
    /// nothing downstream re-checks `dst_label`, so a stale `Some` left
    /// behind would be silently ignored rather than caught anywhere else.
    ///
    /// Mutation it catches: change `resolve_labels`'s
    /// `pair.dst_label.take()` to `.clone()` — the resolved `dst` is still
    /// correct (1), but the label stays attached instead of being
    /// cleared, and this is the only test that reads the field to notice.
    #[test]
    fn resolve_labels_clears_the_label_it_consumes() {
        let objects = labelled_fixture();
        let order = crate::linker::resolve::resolve(&objects, &[], "main")
            .expect("the fixture resolves")
            .order;
        let arena = resolve_bindings(&order).expect("a labelled binding resolves");
        let resolved = &arena[0][0];
        assert_eq!(
            resolved.binding[0].pairs[0].dst, 1,
            "glyph `a` is index 1 in sub's declared alphabet ('_', 'a')"
        );
        assert!(
            resolved.binding[0].pairs[0].dst_label.is_none(),
            "the label must be cleared once resolved"
        );
        drop(objects);
    }
}
