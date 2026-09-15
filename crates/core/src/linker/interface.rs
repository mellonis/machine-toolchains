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
//! Placed here rather than in `resolve`: `resolve` is shared with
//! `resolve_names`, the standalone name-resolution query the editor
//! overlays run against, which has no business failing over a binding.
//!
//! This module is also where a symbolic form's resolution can FAIL, and
//! it is the only place it can: a parameter the callee does not declare,
//! a glyph outside its alphabet, an open binding into a tape it does not
//! declare opaque, an exit vector whose length disagrees with the
//! declared exit count. Every one of them is refused before the
//! composition engine reads a binding, so no mechanism can mis-lower one
//! — the single gate all three pass through.

use super::LinkError;
use super::resolve::FuncRef;
use crate::formats::object::{BoundCall, ObjectFile, RoutineInterface, TapeBinding};
use std::collections::HashMap;

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
    // How many exits the callee DECLARES. A callee that describes no
    // interface declares no state parameters at all, so 0 is its true
    // count, not a missing value: an exit-FREE site into it is simply
    // transparent, and an exit-BEARING one is refused a few lines below
    // by `require_interface`, whose message names the missing interface
    // rather than a count mismatch (docs/core.md (symbolic resolution)).
    let declared = usize::from(callee.interface.map_or(0, |i| i.exits));
    if named == 0 && !labelled && !open && record.exits.is_empty() && declared == 0 {
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
    // The exit arity is checked on EVERY site into a callee that declares
    // exits, not only on one that spells a vector: a site supplying none
    // into an exit-bearing callee leaves the callee's `retx` indexing a
    // vector that is not there (docs/core.md (call mechanisms)). The
    // exit-bearing arm goes through `require_interface` first, so an
    // interfaceless callee is named as such rather than as "declares 0".
    if !record.exits.is_empty() {
        let iface = require_interface(callee, "an exit vector")?;
        if record.exits.len() != usize::from(iface.exits) {
            return Err(bad(
                callee,
                exit_count_mismatch(&callee.name, record.exits.len(), declared),
            ));
        }
    } else if declared != 0 {
        return Err(bad(
            callee,
            exit_count_mismatch(&callee.name, record.exits.len(), declared),
        ));
    }
    Ok(BoundCall {
        binding,
        ..record.clone()
    })
}

/// The exit-arity refusal's text, shared by every site that raises it — a
/// bound site that spells the wrong number of exits, one that spells none
/// into a callee that declares some, and a PLAIN site (`engine::check_sites`),
/// which always supplies zero. One format string, so no two spellings can
/// drift ("supplies 0 exit(s), but `sub` declares 2").
pub(super) fn exit_count_mismatch(callee_name: &str, supplied: usize, declared: usize) -> String {
    format!("the call site supplies {supplied} exit(s), but `{callee_name}` declares {declared}")
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

/// The ONE construction path for [`LinkError::BadBinding`] against a
/// named callee — this module's own refusals and the site-grading pass
/// in `engine` alike, so a variant that gains a field cannot be filled
/// two ways.
pub(super) fn bad(callee: &FuncRef, message: String) -> LinkError {
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

/// Compare every recorded library graft against the digest its exporter
/// declares (docs/core.md (graft drift)). The exporter is found in the
/// linker's own namespace order — user objects first, then libraries,
/// first-wins — so a user object that also exports the graph shadows a
/// library exactly as it shadows a symbol. A graph no input exports is
/// NOT checked: that is a header-only library, the one place a header is
/// trusted.
///
/// Object-level, not per-blob: `Interface::graphs` and
/// `ObjectFile::grafts` describe a whole unit, so this runs over the
/// inputs rather than over the reached order, and reachability does not
/// gate it — a unit either spliced that body or it did not.
pub(super) fn check_graft_drift(
    objects: &[ObjectFile],
    libraries: &[ObjectFile],
    sources: &[Option<String>],
) -> Result<(), LinkError> {
    let inputs: Vec<&ObjectFile> = objects.iter().chain(libraries).collect();
    let name_of = |i: usize| -> String {
        sources
            .get(i)
            .and_then(Option::as_ref)
            .cloned()
            .unwrap_or_else(|| format!("input #{i}"))
    };
    // First-wins exporter map, in the namespace's own order.
    let mut exporter: HashMap<&str, (usize, u32)> = HashMap::new();
    for (i, obj) in inputs.iter().enumerate() {
        let Some(iface) = obj.interface.as_ref() else {
            continue;
        };
        for g in &iface.graphs {
            exporter.entry(g.name.as_str()).or_insert((i, g.digest));
        }
    }
    for (i, obj) in inputs.iter().enumerate() {
        for graft in &obj.grafts {
            let Some(&(lib, digest)) = exporter.get(graft.graph.as_str()) else {
                continue; // header-only library: nothing to check against
            };
            if digest != graft.digest {
                return Err(LinkError::GraftDrift {
                    graph: graft.graph.clone(),
                    consumer: name_of(i),
                    library: name_of(lib),
                });
            }
        }
    }
    Ok(())
}

/// Compare every recorded alphabet import against the exporting object's
/// own declaration (docs/core.md (graft drift)): same first-wins,
/// object-level, header-only-is-unchecked shape as `check_graft_drift`,
/// over `Interface::imports`/`Interface::alphabets` instead of
/// `ObjectFile::grafts`/`Interface::graphs`.
pub(super) fn check_imported_alphabets(
    objects: &[ObjectFile],
    libraries: &[ObjectFile],
    sources: &[Option<String>],
) -> Result<(), LinkError> {
    let inputs: Vec<&ObjectFile> = objects.iter().chain(libraries).collect();
    let name_of = |i: usize| -> String {
        sources
            .get(i)
            .and_then(Option::as_ref)
            .cloned()
            .unwrap_or_else(|| format!("input #{i}"))
    };
    // First-wins exporter map, in the namespace's own order.
    let mut exporter: HashMap<&str, (usize, &Vec<String>)> = HashMap::new();
    for (i, obj) in inputs.iter().enumerate() {
        let Some(iface) = obj.interface.as_ref() else {
            continue;
        };
        for a in &iface.alphabets {
            exporter.entry(a.name.as_str()).or_insert((i, &a.glyphs));
        }
    }
    for (i, obj) in inputs.iter().enumerate() {
        let Some(iface) = obj.interface.as_ref() else {
            continue;
        };
        for imported in &iface.imports {
            let Some(&(lib, exported_glyphs)) = exporter.get(imported.name.as_str()) else {
                continue; // header-only library: nothing to check against
            };
            let differs = imported
                .glyphs
                .iter()
                .zip(exported_glyphs)
                .position(|(a, b)| a != b)
                .or_else(|| {
                    (imported.glyphs.len() != exported_glyphs.len())
                        .then_some(imported.glyphs.len().min(exported_glyphs.len()))
                });
            if let Some(position) = differs {
                return Err(LinkError::AlphabetDrift {
                    alphabet: imported.name.clone(),
                    consumer: name_of(i),
                    library: name_of(lib),
                    position,
                });
            }
        }
    }
    Ok(())
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
