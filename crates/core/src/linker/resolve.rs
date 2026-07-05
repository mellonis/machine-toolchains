//! Symbol resolution and reachability (spec §9): build the user+library
//! namespace (user duplicates error, libraries first-wins and shadowed
//! silently by user definitions), then BFS from `main` so only reachable
//! functions are linked in — dead functions are dropped and may reference
//! anything, even names that don't exist.

use std::collections::{BTreeSet, HashMap, VecDeque};

use super::LinkError;
use crate::formats::object::{BlobDebug, ObjectFile, SymbolDef};

#[derive(Debug)]
pub(crate) struct FuncRef<'a> {
    pub name: &'a str,
    pub blob: &'a [u8],
    pub debug: Option<&'a BlobDebug>,
    /// Call sites in blob order: (hole offset in blob, callee index in `order`).
    pub calls: Vec<(u32, usize)>,
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
) -> Result<Resolved<'a>, LinkError> {
    let all: Vec<&ObjectFile> = objects.iter().chain(libraries).collect();
    let Some(first) = all.first() else {
        return Err(LinkError::NoEntrySymbol);
    };
    let expected = first.arch;
    if let Some(bad) = all.iter().find(|o| o.arch != expected) {
        return Err(LinkError::ArchMismatch {
            expected,
            found: bad.arch,
        });
    }

    // Namespace: user objects (dup = error), then libraries (first-wins).
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

    // BFS from main.
    let Some(&main_site) = namespace.get("main") else {
        return Err(LinkError::NoEntrySymbol);
    };
    let mut order_sites: Vec<Site> = vec![main_site];
    let mut index_of: HashMap<Site, usize> = HashMap::from([(main_site, 0)]);
    let mut queue: VecDeque<Site> = VecDeque::from([main_site]);
    let mut unresolved: BTreeSet<String> = BTreeSet::new();
    // calls per discovered function, resolved to final indices as callees
    // are discovered (an index is known the moment it's pushed).
    let mut calls_by_site: HashMap<Site, Vec<(u32, usize)>> = HashMap::new();

    while let Some(site) = queue.pop_front() {
        let object = object_at(site.0);
        let mut calls = Vec::new();
        let mut relocs: Vec<_> = object
            .relocations
            .iter()
            .filter(|r| r.blob == site.1)
            .collect();
        relocs.sort_by_key(|r| r.offset);
        for reloc in relocs {
            let name = object.symbols[reloc.symbol as usize].name.as_str();
            match namespace.get(name) {
                None => {
                    unresolved.insert(name.to_string());
                }
                Some(&callee) => {
                    let idx = *index_of.entry(callee).or_insert_with(|| {
                        order_sites.push(callee);
                        queue.push_back(callee);
                        order_sites.len() - 1
                    });
                    calls.push((reloc.offset, idx));
                }
            }
        }
        calls_by_site.insert(site, calls);
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
                .find(|s| s.def == SymbolDef::Defined { blob: site.1 })
                .map(|s| s.name.as_str())
                .expect("site came from a Defined symbol");
            FuncRef {
                name,
                blob: &object.blobs[site.1 as usize],
                debug: object.debug.as_ref().map(|d| &d[site.1 as usize]),
                calls: calls_by_site.remove(&site).unwrap_or_default(),
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
    use crate::formats::object::{ObjectFile, Relocation, Symbol, SymbolDef};

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
        ObjectFile {
            arch,
            symbols,
            blobs,
            relocations,
            debug: None,
        }
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
        let r = resolve(std::slice::from_ref(&a), &[]).unwrap();
        let names: Vec<&str> = r.order.iter().map(|f| f.name).collect();
        assert_eq!(names, vec!["main", "helper", "second"]);
        assert_eq!(r.order[0].calls, vec![(2, 1), (7, 2)]); // holes at 2 and 7
    }

    #[test]
    fn dead_functions_are_dropped_and_may_be_broken() {
        // "dead" calls a missing symbol — fine, it's unreachable.
        let a = obj(0x7E, &[("main", &[]), ("dead", &["missing"])]);
        let r = resolve(std::slice::from_ref(&a), &[]).unwrap();
        assert_eq!(r.order.len(), 1);
        assert_eq!(r.order[0].name, "main");
    }

    #[test]
    fn reachable_unresolved_errors_sorted() {
        let a = obj(0x7E, &[("main", &["zeta", "alpha"])]);
        let e = resolve(std::slice::from_ref(&a), &[]).unwrap_err();
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
        let r = resolve(std::slice::from_ref(&user), std::slice::from_ref(&lib)).unwrap();
        let names: Vec<&str> = r.order.iter().map(|f| f.name).collect();
        assert_eq!(names, vec!["main", "go"]);
        // dropped is name-level, post-shadowing: the library's `go` was
        // never a candidate (user's `go` won and IS in the binary), so it
        // must not be reported; only `unused_pulls_nothing` is dropped.
        assert_eq!(r.dropped, vec!["unused_pulls_nothing".to_string()]);

        let needy = obj(0x7E, &[("main", &["go"])]);
        let r2 = resolve(std::slice::from_ref(&needy), std::slice::from_ref(&lib)).unwrap();
        assert_eq!(r2.order.len(), 2); // library's go pulled in
        assert_eq!(r2.dropped, vec!["unused_pulls_nothing".to_string()]);
    }

    #[test]
    fn duplicate_user_symbols_error_but_library_shadowing_does_not() {
        let a = obj(0x7E, &[("main", &[]), ("f", &[])]);
        let b = obj(0x7E, &[("f", &[])]);
        let e = resolve(&[a.clone(), b], &[]).unwrap_err();
        assert_eq!(e, LinkError::DuplicateSymbol("f".into()));
        let lib1 = obj(0x7E, &[("f", &[])]);
        let lib2 = obj(0x7E, &[("f", &[])]);
        assert!(resolve(std::slice::from_ref(&a), &[lib1, lib2]).is_ok()); // first-wins, silent
    }

    #[test]
    fn no_main_and_arch_mismatch() {
        let a = obj(0x7E, &[("helper", &[])]);
        assert_eq!(
            resolve(std::slice::from_ref(&a), &[]).unwrap_err(),
            LinkError::NoEntrySymbol
        );
        let b = obj(0x11, &[("main", &[])]);
        let mixed = [obj(0x7E, &[("x", &[])]), b];
        assert_eq!(
            resolve(&mixed, &[]).unwrap_err(),
            LinkError::ArchMismatch {
                expected: 0x7E,
                found: 0x11
            }
        );
    }
}
