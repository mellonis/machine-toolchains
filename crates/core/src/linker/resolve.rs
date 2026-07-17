//! Symbol resolution and reachability (docs/stdlib.md (linking)): build
//! the user+library namespace (user duplicates error, libraries
//! first-wins and shadowed silently by user definitions), then BFS from
//! the entry symbol (default `main`, or the `--entry` override) so only
//! reachable functions are linked in — dead functions are dropped and may
//! reference anything, even names that don't exist. Reachability follows
//! both relocation call sites and declarative bound-call sites.

use std::borrow::Cow;
use std::collections::{BTreeSet, HashMap, VecDeque};

use super::LinkError;
use crate::formats::object::{BlobDebug, BoundCall, ObjectFile, RoutineSig, SymbolDef};

#[derive(Debug)]
pub(crate) struct FuncRef<'a> {
    /// The function's name. `Borrowed` from the object for a resolved
    /// function; `Owned` for a composition-engine synthetic (a mono stamp
    /// `<callee>$<digest8>`), which has no backing symbol.
    pub name: Cow<'a, str>,
    /// The function's code blob. `Borrowed` straight from the object as
    /// resolved; the composition engine replaces it with an `Owned`
    /// rewritten blob (bound calls widened to framed calls) before layout.
    pub blob: Cow<'a, [u8]>,
    /// Debug info; `Owned` after the engine shifts label/line offsets past
    /// a widened bound-call site.
    pub debug: Option<Cow<'a, BlobDebug>>,
    /// Call sites in blob order: (hole offset in blob, callee index in `order`).
    pub calls: Vec<(u32, usize)>,
    /// Declarative bound-call sites in blob order, mirroring `calls`'
    /// shape: (operand hole offset in blob, callee index in `order`,
    /// the source record). The composition engine reads the binding from
    /// the record and rewrites each site to a framed call, after which
    /// `bound` is emptied.
    pub bound: Vec<(u32, usize, &'a BoundCall)>,
    /// This function's table blob — its match/dispatch table bytes
    /// (docs/formats.md (.pmo)); empty when the object carries none.
    /// `Owned` after the engine shifts a raw frame descriptor's exit
    /// offsets past a widened bound-call site.
    pub table: Cow<'a, [u8]>,
    /// TableRef operand holes within this blob, as (hole offset in blob,
    /// offset into `table`); the layout pass rebases them into the final
    /// table section (docs/formats.md (executable image)).
    pub table_fixups: Vec<(u32, u32)>,
    /// The function's generic-routine signature, when its object signs
    /// blobs (signatures are all-or-none per object, parallel to blobs).
    pub signature: Option<&'a RoutineSig>,
}

#[derive(Debug)]
pub(crate) struct Resolved<'a> {
    /// Functions in layout order: main first, then BFS discovery order.
    pub order: Vec<FuncRef<'a>>,
    /// Sorted names whose winning (post-shadowing) definition went
    /// unreached; shadowed library copies are not reported.
    pub dropped: Vec<String>,
}

/// (object index within the user+library concatenation, blob index)
type Site = (usize, u32);

pub(crate) fn resolve<'a>(
    objects: &'a [ObjectFile],
    libraries: &'a [ObjectFile],
    entry: &str,
) -> Result<Resolved<'a>, LinkError> {
    let all: Vec<&ObjectFile> = objects.iter().chain(libraries).collect();
    let Some(first) = all.first() else {
        return Err(LinkError::NoEntrySymbol(entry.to_string()));
    };
    let expected = first.arch;
    if let Some(bad) = all.iter().find(|o| o.arch != expected) {
        return Err(LinkError::ArchMismatch {
            expected,
            found: bad.arch,
        });
    }

    // Namespace: user objects (dup = error), then libraries (first-wins).
    // Local symbols never enter the namespace: not exported, not shadowable.
    let mut namespace: HashMap<&str, Site> = HashMap::new();
    for (oi, object) in objects.iter().enumerate() {
        for symbol in &object.symbols {
            if let SymbolDef::Defined { blob } = symbol.def
                && namespace.insert(symbol.name.as_str(), (oi, blob)).is_some()
            {
                return Err(LinkError::DuplicateSymbol(symbol.name.clone()));
            }
        }
    }
    for (li, library) in libraries.iter().enumerate() {
        for symbol in &library.symbols {
            if let SymbolDef::Defined { blob } = symbol.def {
                namespace
                    .entry(symbol.name.as_str())
                    .or_insert((objects.len() + li, blob));
            }
        }
    }

    let object_at = |oi: usize| -> &'a ObjectFile {
        if oi < objects.len() {
            &objects[oi]
        } else {
            &libraries[oi - objects.len()]
        }
    };

    // BFS from the entry symbol.
    let Some(&entry_site) = namespace.get(entry) else {
        return Err(LinkError::NoEntrySymbol(entry.to_string()));
    };
    let mut order_sites: Vec<Site> = vec![entry_site];
    let mut index_of: HashMap<Site, usize> = HashMap::from([(entry_site, 0)]);
    let mut queue: VecDeque<Site> = VecDeque::from([entry_site]);
    let mut unresolved: BTreeSet<String> = BTreeSet::new();
    // calls/bound sites per discovered function, resolved to final indices
    // as callees are discovered (an index is known the moment it's pushed).
    let mut calls_by_site: HashMap<Site, Vec<(u32, usize)>> = HashMap::new();
    let mut bound_by_site: HashMap<Site, Vec<(u32, usize, &'a BoundCall)>> = HashMap::new();

    // A symbol reference (a relocation callee or a bound callee) resolves
    // to a site the same way: a Local binds directly within its own
    // object — never through the namespace, so it can't shadow or be
    // shadowed (docs/language.md (visibility); docs/stdlib.md (linking)) —
    // otherwise it goes through the namespace.
    let resolve_target = |object: &ObjectFile, oi: usize, sym: u32| -> Option<Site> {
        match object.symbols[sym as usize].def {
            SymbolDef::Local { blob } => Some((oi, blob)),
            _ => namespace
                .get(object.symbols[sym as usize].name.as_str())
                .copied(),
        }
    };

    while let Some(site) = queue.pop_front() {
        let object = object_at(site.0);

        // A callee's order index is minted the moment it is first reached.
        let reach = |callee: Site,
                     index_of: &mut HashMap<Site, usize>,
                     order_sites: &mut Vec<Site>,
                     queue: &mut VecDeque<Site>|
         -> usize {
            *index_of.entry(callee).or_insert_with(|| {
                order_sites.push(callee);
                queue.push_back(callee);
                order_sites.len() - 1
            })
        };

        let mut calls = Vec::new();
        let mut relocs: Vec<_> = object
            .relocations
            .iter()
            .filter(|r| r.blob == site.1)
            .collect();
        relocs.sort_by_key(|r| r.offset);
        for reloc in relocs {
            match resolve_target(object, site.0, reloc.symbol) {
                None => {
                    unresolved.insert(object.symbols[reloc.symbol as usize].name.clone());
                }
                Some(callee) => {
                    let idx = reach(callee, &mut index_of, &mut order_sites, &mut queue);
                    calls.push((reloc.offset, idx));
                }
            }
        }
        calls_by_site.insert(site, calls);

        // Declarative bound calls (`call name [binding]`) reach their
        // callee like a relocation does; the composition engine consumes
        // the binding later. Processed after relocations, so BFS discovery
        // order is stable for objects that mix both.
        let mut bound = Vec::new();
        let mut binds: Vec<_> = object
            .bound_calls
            .iter()
            .filter(|b| b.blob == site.1)
            .collect();
        binds.sort_by_key(|b| b.offset);
        for bc in binds {
            match resolve_target(object, site.0, bc.symbol) {
                None => {
                    unresolved.insert(object.symbols[bc.symbol as usize].name.clone());
                }
                Some(callee) => {
                    let idx = reach(callee, &mut index_of, &mut order_sites, &mut queue);
                    bound.push((bc.offset, idx, bc));
                }
            }
        }
        bound_by_site.insert(site, bound);
    }

    if !unresolved.is_empty() {
        return Err(LinkError::Unresolved(unresolved.into_iter().collect()));
    }

    // Dropped names, post-shadowing: the namespace already resolved every
    // name to the ONE site that would have been linked, so a name is
    // dropped exactly when that winning site went unreached. Shadowed
    // library copies were never candidates and are not reported.
    let mut dropped: BTreeSet<String> = BTreeSet::new();
    for (&name, site) in &namespace {
        if !index_of.contains_key(site) {
            dropped.insert(name.to_string());
        }
    }

    let order = order_sites
        .into_iter()
        .map(|site| {
            let object = object_at(site.0);
            let name = object
                .symbols
                .iter()
                .find(|s| {
                    matches!(s.def,
                        SymbolDef::Defined { blob } | SymbolDef::Local { blob }
                            if blob == site.1)
                })
                .map(|s| s.name.as_str())
                .expect("site came from a Defined or Local symbol");
            FuncRef {
                name: Cow::Borrowed(name),
                blob: Cow::Borrowed(&object.blobs[site.1 as usize]),
                debug: object
                    .debug
                    .as_ref()
                    .map(|d| Cow::Borrowed(&d[site.1 as usize])),
                calls: calls_by_site.remove(&site).unwrap_or_default(),
                bound: bound_by_site.remove(&site).unwrap_or_default(),
                table: Cow::Borrowed(
                    object
                        .table_blobs
                        .as_ref()
                        .and_then(|t| t.get(site.1 as usize))
                        .map_or(&[][..], Vec::as_slice),
                ),
                table_fixups: object
                    .table_fixups
                    .iter()
                    .filter(|fx| fx.blob == site.1)
                    .map(|fx| (fx.offset, fx.table_offset))
                    .collect(),
                signature: object
                    .signatures
                    .as_ref()
                    .and_then(|s| s.get(site.1 as usize)),
            }
        })
        .collect();
    Ok(Resolved {
        order,
        dropped: dropped.into_iter().collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::formats::object::{BoundCall, ObjectFile, Relocation, Symbol, SymbolDef};

    /// Object with `funcs` = (name, callees-by-name). Blob content is a
    /// stub: [0x0E] + one 5-byte call hole per callee (opcode 0x21).
    fn obj(arch: u8, funcs: &[(&str, &[&str])]) -> ObjectFile {
        let mut symbols: Vec<Symbol> = funcs
            .iter()
            .enumerate()
            .map(|(i, (n, _))| Symbol {
                name: (*n).into(),
                def: SymbolDef::Defined { blob: i as u32 },
            })
            .collect();
        let mut blobs = Vec::new();
        let mut relocations = Vec::new();
        for (bi, (_, callees)) in funcs.iter().enumerate() {
            let mut blob = vec![0x0E];
            for callee in *callees {
                let sym = symbols
                    .iter()
                    .position(|s| s.name == **callee)
                    .unwrap_or_else(|| {
                        symbols.push(Symbol {
                            name: (*callee).into(),
                            def: SymbolDef::External,
                        });
                        symbols.len() - 1
                    });
                blob.push(0x21);
                relocations.push(Relocation {
                    blob: bi as u32,
                    offset: blob.len() as u32,
                    symbol: sym as u32,
                });
                blob.extend([0u8; 4]);
            }
            blob.push(0x02);
            blobs.push(blob);
        }
        ObjectFile::v2(arch, symbols, blobs, relocations, None)
    }

    #[test]
    fn bfs_order_is_main_first_discovery_order() {
        let a = obj(
            0x7E,
            &[
                ("helper", &[]),
                ("main", &["helper", "second"]),
                ("second", &["helper"]),
            ],
        );
        let r = resolve(std::slice::from_ref(&a), &[], "main").unwrap();
        let names: Vec<&str> = r.order.iter().map(|f| f.name.as_ref()).collect();
        assert_eq!(names, vec!["main", "helper", "second"]);
        assert_eq!(r.order[0].calls, vec![(2, 1), (7, 2)]); // holes at 2 and 7
    }

    #[test]
    fn dead_functions_are_dropped_and_may_be_broken() {
        // "dead" calls a missing symbol — fine, it's unreachable.
        let a = obj(0x7E, &[("main", &[]), ("dead", &["missing"])]);
        let r = resolve(std::slice::from_ref(&a), &[], "main").unwrap();
        assert_eq!(r.order.len(), 1);
        assert_eq!(r.order[0].name, "main");
    }

    #[test]
    fn reachable_unresolved_errors_sorted() {
        let a = obj(0x7E, &[("main", &["zeta", "alpha"])]);
        let e = resolve(std::slice::from_ref(&a), &[], "main").unwrap_err();
        assert_eq!(
            e,
            LinkError::Unresolved(vec!["alpha".into(), "zeta".into()])
        );
    }

    #[test]
    fn libraries_resolve_lazily_and_users_shadow() {
        let user = obj(0x7E, &[("main", &["go"]), ("go", &[])]);
        let lib = obj(0x7E, &[("go", &[]), ("unused_pulls_nothing", &["ghost"])]);
        // user's `go` shadows the library's; the library's broken function
        // is never reached, so `ghost` doesn't error.
        let r = resolve(
            std::slice::from_ref(&user),
            std::slice::from_ref(&lib),
            "main",
        )
        .unwrap();
        let names: Vec<&str> = r.order.iter().map(|f| f.name.as_ref()).collect();
        assert_eq!(names, vec!["main", "go"]);
        // dropped is name-level, post-shadowing: the library's `go` was
        // never a candidate (user's `go` won and IS in the binary), so it
        // must not be reported; only `unused_pulls_nothing` is dropped.
        assert_eq!(r.dropped, vec!["unused_pulls_nothing".to_string()]);

        let needy = obj(0x7E, &[("main", &["go"])]);
        let r2 = resolve(
            std::slice::from_ref(&needy),
            std::slice::from_ref(&lib),
            "main",
        )
        .unwrap();
        assert_eq!(r2.order.len(), 2); // library's go pulled in
        assert_eq!(r2.dropped, vec!["unused_pulls_nothing".to_string()]);
    }

    #[test]
    fn duplicate_user_symbols_error_but_library_shadowing_does_not() {
        let a = obj(0x7E, &[("main", &[]), ("f", &[])]);
        let b = obj(0x7E, &[("f", &[])]);
        let e = resolve(&[a.clone(), b], &[], "main").unwrap_err();
        assert_eq!(e, LinkError::DuplicateSymbol("f".into()));
        let lib1 = obj(0x7E, &[("f", &[])]);
        let lib2 = obj(0x7E, &[("f", &[])]);
        assert!(resolve(std::slice::from_ref(&a), &[lib1, lib2], "main").is_ok()); // first-wins, silent
    }

    #[test]
    fn no_main_and_arch_mismatch() {
        let a = obj(0x7E, &[("helper", &[])]);
        assert_eq!(
            resolve(std::slice::from_ref(&a), &[], "main").unwrap_err(),
            LinkError::NoEntrySymbol("main".into())
        );
        let b = obj(0x11, &[("main", &[])]);
        let mixed = [obj(0x7E, &[("x", &[])]), b];
        assert_eq!(
            resolve(&mixed, &[], "main").unwrap_err(),
            LinkError::ArchMismatch {
                expected: 0x7E,
                found: 0x11
            }
        );
    }

    /// Like `obj`, but functions whose name is in `locals` get Local defs.
    fn obj_with_locals(arch: u8, funcs: &[(&str, &[&str])], locals: &[&str]) -> ObjectFile {
        let mut o = obj(arch, funcs);
        for s in &mut o.symbols {
            if locals.contains(&s.name.as_str())
                && let SymbolDef::Defined { blob } = s.def
            {
                s.def = SymbolDef::Local { blob };
            }
        }
        o
    }

    #[test]
    fn locals_bind_directly_and_may_repeat_across_objects() {
        // Both objects define a LOCAL `helper`; each binds to its own.
        let a = obj_with_locals(
            0x7E,
            &[("main", &["helper", "api"]), ("helper", &[])],
            &["helper"],
        );
        let b = obj_with_locals(0x7E, &[("api", &["helper"]), ("helper", &[])], &["helper"]);
        let objs = [a, b];
        let r = resolve(&objs, &[], "main").unwrap();
        let names: Vec<&str> = r.order.iter().map(|f| f.name.as_ref()).collect();
        // main, its own helper, api, api's own helper: BOTH helpers linked.
        assert_eq!(names, vec!["main", "helper", "api", "helper"]);
    }

    #[test]
    fn foreign_locals_are_unresolvable_and_locals_never_shadow() {
        // Object B's `helper` is local; A's external ref must NOT see it.
        let a = obj(0x7E, &[("main", &["helper"])]);
        let b = obj_with_locals(0x7E, &[("helper", &[])], &["helper"]);
        let e = resolve(&[a, b], &[], "main").unwrap_err();
        assert_eq!(e, LinkError::Unresolved(vec!["helper".into()]));
    }

    #[test]
    fn local_and_global_same_name_coexist_without_duplicate_error() {
        // A exports `helper`; B has a LOCAL `helper` — no DuplicateSymbol,
        // and B's caller binds to B's own local, not A's export.
        let a = obj(0x7E, &[("main", &["api"]), ("helper", &[])]);
        let b = obj_with_locals(0x7E, &[("api", &["helper"]), ("helper", &[])], &["helper"]);
        let objs = [a, b];
        let r = resolve(&objs, &[], "main").unwrap();
        // api's call resolved into object B (site-identity, not name):
        let api = r.order.iter().position(|f| f.name == "api").unwrap();
        let callee_idx = r.order[api].calls[0].1;
        // B's local helper blob is [0x0E, 0x02] (no calls); A's exported
        // helper has the same shape — distinguish by checking the callee
        // is NOT the same FuncRef the unreached A-helper would be: A's
        // helper must be in dropped (unreached), B's local not reported.
        assert_eq!(r.dropped, vec!["helper".to_string()]);
        assert!(callee_idx < r.order.len());
    }

    /// Add a bound-call site to `obj`'s blob 0, targeting `callee` by
    /// name. Resolve reads only the record's `symbol`/`offset` — the
    /// binding payload is irrelevant to reachability — so it stays empty.
    fn push_bound(obj: &mut ObjectFile, offset: u32, callee: &str) {
        let symbol = obj
            .symbols
            .iter()
            .position(|s| s.name == callee)
            .unwrap_or_else(|| {
                obj.symbols.push(Symbol {
                    name: callee.into(),
                    def: SymbolDef::External,
                });
                obj.symbols.len() - 1
            }) as u32;
        obj.bound_calls.push(BoundCall {
            blob: 0,
            offset,
            symbol,
            binding: Vec::new(),
        });
    }

    #[test]
    fn bound_callees_enter_reachability_and_are_not_dropped() {
        // `sub` is reachable ONLY through a declarative binding, not a
        // relocation — the BFS must still reach it and keep it in `order`.
        let mut a = obj(0x7E, &[("main", &[]), ("sub", &[])]);
        push_bound(&mut a, 1, "sub");
        let r = resolve(std::slice::from_ref(&a), &[], "main").unwrap();
        let names: Vec<&str> = r.order.iter().map(|f| f.name.as_ref()).collect();
        assert_eq!(names, vec!["main", "sub"]);
        // hole at 1 -> order index 1, carrying the source record.
        let bound: Vec<(u32, usize)> = r.order[0].bound.iter().map(|&(o, i, _)| (o, i)).collect();
        assert_eq!(bound, vec![(1, 1)]);
        assert!(r.order[0].calls.is_empty());
        assert!(r.dropped.is_empty());
    }

    #[test]
    fn entry_override_selects_a_different_root() {
        // `alt` is unreachable from main; entry=alt makes it the BFS root
        // and drops main instead.
        let a = obj(0x7E, &[("main", &[]), ("alt", &[])]);
        let r = resolve(std::slice::from_ref(&a), &[], "alt").unwrap();
        let names: Vec<&str> = r.order.iter().map(|f| f.name.as_ref()).collect();
        assert_eq!(names, vec!["alt"]);
        assert_eq!(r.dropped, vec!["main".to_string()]);
    }

    #[test]
    fn unresolved_bound_callee_joins_the_unresolved_error() {
        // A bound call to an undefined symbol errors exactly like an
        // undefined relocation callee.
        let mut a = obj(0x7E, &[("main", &[])]);
        push_bound(&mut a, 1, "ghost");
        let e = resolve(std::slice::from_ref(&a), &[], "main").unwrap_err();
        assert_eq!(e, LinkError::Unresolved(vec!["ghost".into()]));
    }

    #[test]
    fn missing_entry_symbol_is_named() {
        let a = obj(0x7E, &[("main", &[])]);
        let e = resolve(std::slice::from_ref(&a), &[], "start").unwrap_err();
        assert_eq!(e, LinkError::NoEntrySymbol("start".into()));
    }
}
