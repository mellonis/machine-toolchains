//! Symbol resolution and reachability (docs/core.md (linking)): build
//! the user+library namespace (user duplicates error, libraries
//! first-wins and shadowed silently by user definitions), then BFS from
//! the entry symbol (default `main`, or the `--entry` override) so only
//! reachable functions are linked in — dead functions are dropped and may
//! reference anything, even names that don't exist. Reachability follows
//! both relocation call sites and declarative bound-call sites.
//!
//! A name may carry TWO definitions — the normal and volatile build
//! columns of one function — so the namespace maps each name to a
//! [`ColumnPair`] rather than a single site. The program's volatile bit,
//! read off the object that defines the entry symbol, picks the column;
//! a name that ships only the other one is linked anyway and reported in
//! `variant_fallbacks`. Everything downstream — BFS, relaxation, emission,
//! the map sidecar — sees one chosen blob per name, exactly as before.
//!
//! This BFS runs once, before the composition engine lowers any bound
//! call. Under `mono`/`hybrid` stamping, a routine reached here can still
//! end up with no caller once every site is retargeted to a specialized
//! copy; the stamping pass re-walks the (by then retargeted) call graph
//! afterward so that promise still holds over the final image
//! (docs/core.md (linking)).

use std::borrow::Cow;
use std::collections::{BTreeSet, HashMap, VecDeque};

use super::LinkError;
use crate::formats::object::{
    BlobDebug, BlobVariant, BoundCall, ObjectFile, RoutineSig, SymbolDef,
};

/// A blob's build column (docs/formats.md (MO)). An object carrying no
/// variant records at all — a legacy object, a hand-assembled one, any
/// object from an architecture without volatile builds — reads as
/// all-`Normal`: it offers exactly one column, and a volatile program
/// linking it takes a counted fallback rather than an error.
fn variant_of(object: &ObjectFile, blob: u32) -> BlobVariant {
    object
        .variants
        .as_ref()
        .and_then(|tags| tags.get(blob as usize).copied())
        .unwrap_or(BlobVariant::Normal)
}

/// One exported name's two build columns (docs/core.md (linking)): the
/// definition a normal build links, and the one a volatile build links. A
/// `Both`-tagged blob — a function whose two columns compiled identical
/// and deduped — fills both slots with the same site. A name defined in
/// only one column leaves the other empty. At least one slot is always
/// filled, and both always come from ONE object: a name's columns are
/// never mixed across inputs.
#[derive(Debug, Clone, Copy, Default)]
struct ColumnPair {
    normal: Option<Site>,
    volatile: Option<Site>,
}

impl ColumnPair {
    /// Claim the slot(s) `tag` covers. `Err` when a claimed slot is
    /// already filled: the name then has two definitions in one column,
    /// which is a duplicate however the two are tagged — so a
    /// `{Normal, Volatile}` pair is the ONLY same-name pair one object may
    /// carry.
    fn fill(&mut self, tag: BlobVariant, site: Site) -> Result<(), ()> {
        let claims_normal = !matches!(tag, BlobVariant::Volatile);
        let claims_volatile = !matches!(tag, BlobVariant::Normal);
        if (claims_normal && self.normal.is_some()) || (claims_volatile && self.volatile.is_some())
        {
            return Err(());
        }
        if claims_normal {
            self.normal = Some(site);
        }
        if claims_volatile {
            self.volatile = Some(site);
        }
        Ok(())
    }

    /// The object both columns came from.
    fn owner(&self) -> usize {
        self.slot(false)
            .or_else(|| self.slot(true))
            .expect("a namespace entry holds at least one column")
            .0
    }

    fn slot(&self, volatile: bool) -> Option<Site> {
        if volatile { self.volatile } else { self.normal }
    }

    /// The site a program with this volatile bit links, and whether it had
    /// to fall back to the other column because the wanted one is absent.
    ///
    /// A fallback is a mixed link by design, not a coherence hole: the
    /// borrowed body keeps its own object's intra-object edges (a volatile
    /// body's private callees stay volatile — those bind blobs directly),
    /// while every name it resolves through this namespace is chosen by
    /// the program's bit again. Counting the name is the signal that this
    /// happened; the caller surfaces it (docs/core.md (linking)).
    fn choose(&self, volatile: bool) -> (Site, bool) {
        match self.slot(volatile) {
            Some(site) => (site, false),
            None => (
                self.slot(!volatile)
                    .expect("a namespace entry holds at least one column"),
                true,
            ),
        }
    }
}

/// One object's exported names as column pairs, in first-appearance symbol
/// order (so a reported duplicate names the first offending symbol, as it
/// always did). Local symbols are skipped: not exported, not shadowable.
///
/// `strict` — user objects — reports a repeated column as a duplicate.
/// A library stays LENIENT, which is today's behaviour: a name it defines
/// twice silently keeps the first definition, exactly as `first-wins`
/// already silently shadows a second library's copy.
///
/// Lenient means first-RECORD-wins, not first-slot-wins: [`ColumnPair::fill`]
/// refuses before claiming anything, so a rejected definition is dropped
/// whole, including any claim it had on a slot nobody was contesting. A
/// library listing `f` as `Normal` and then as `Both` therefore offers one
/// column, not two, and a volatile program takes a fallback it need not
/// have. Consistent with first-wins, and unreachable from compiler output —
/// the two-column compiler emits `{Normal, Volatile}` or a lone `Both` for
/// a name, never a mix — so it is recorded here rather than special-cased.
fn object_columns(
    object: &ObjectFile,
    oi: usize,
    strict: bool,
) -> Result<Vec<(&str, ColumnPair)>, LinkError> {
    let mut order: Vec<&str> = Vec::new();
    let mut pairs: HashMap<&str, ColumnPair> = HashMap::new();
    for symbol in &object.symbols {
        let SymbolDef::Defined { blob } = symbol.def else {
            continue;
        };
        let name = symbol.name.as_str();
        let pair = pairs.entry(name).or_insert_with(|| {
            order.push(name);
            ColumnPair::default()
        });
        if pair.fill(variant_of(object, blob), (oi, blob)).is_err() && strict {
            return Err(LinkError::DuplicateSymbol(symbol.name.clone()));
        }
    }
    Ok(order.into_iter().map(|name| (name, pairs[name])).collect())
}

/// The producer's guarantee, checked rather than assumed: a `Both` blob
/// only ever references `Both` blobs INSIDE its own object. The two-column
/// compiler enforces it with a call-graph fixpoint — a dedup candidate
/// that calls a function which split, splits with it — because a `Both`
/// blob's relocations name one column and would otherwise pin a volatile
/// program to the normal call graph. Cross-object references are exempt:
/// those resolve by name, so the column is chosen per program.
///
/// Checked under `debug_assert` only. Nothing in the container format
/// enforces it, so a hand-crafted object could trip it — the same standing
/// as this module's `expect`s on well-formed symbol tables.
fn both_is_column_closed(object: &ObjectFile, caller: Site, callee: Site) -> bool {
    variant_of(object, caller.1) != BlobVariant::Both
        || callee.0 != caller.0
        || variant_of(object, callee.1) == BlobVariant::Both
}

#[derive(Debug)]
pub(crate) struct FuncRef<'a> {
    /// The function's name. `Borrowed` from the object for a resolved
    /// function; `Owned` for a composition-engine synthetic (a mono stamp
    /// `<callee>.<digest8>`), which has no backing symbol.
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
    /// Index of the input that supplied this definition, counting through
    /// the user objects then the libraries — provenance for the
    /// name-resolution query surface (docs/core.md (name resolution)).
    pub(crate) origin: usize,
}

#[derive(Debug)]
pub(crate) struct Resolved<'a> {
    /// Functions in layout order: main first, then BFS discovery order.
    pub order: Vec<FuncRef<'a>>,
    /// Sorted names whose winning (post-shadowing) definition went
    /// unreached; shadowed library copies are not reported.
    pub dropped: Vec<String>,
    /// Sorted names that linked the build column NOT matching the
    /// program's volatile bit, because the wanted one was absent.
    pub variant_fallbacks: Vec<String>,
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
    // Each name maps to its column PAIR, filled from one object.
    let mut namespace: HashMap<&str, ColumnPair> = HashMap::new();
    for (oi, object) in objects.iter().enumerate() {
        for (name, pair) in object_columns(object, oi, true)? {
            if namespace.insert(name, pair).is_some() {
                return Err(LinkError::DuplicateSymbol(name.to_string()));
            }
        }
    }
    for (li, library) in libraries.iter().enumerate() {
        // First-wins, silent: a name already defined — by a user object or
        // an earlier library — keeps the definition it has, both columns
        // of it, so a name's columns never span two inputs.
        for (name, pair) in object_columns(library, objects.len() + li, false)? {
            namespace.entry(name).or_insert(pair);
        }
    }

    let object_at = |oi: usize| -> &'a ObjectFile {
        if oi < objects.len() {
            &objects[oi]
        } else {
            &libraries[oi - objects.len()]
        }
    };

    // BFS from the entry symbol. The program's volatile bit — which
    // column every name resolves to — is the bit carried by the object
    // that DEFINES the entry symbol.
    let Some(&entry_pair) = namespace.get(entry) else {
        return Err(LinkError::NoEntrySymbol(entry.to_string()));
    };
    let program_volatile = object_at(entry_pair.owner()).program_volatile;
    // Names that linked the other column. The entry counts like any other
    // name: it never passes through a relocation, so a volatile program
    // whose `main` ships only a normal body would otherwise read as a
    // clean link.
    let mut fallbacks: BTreeSet<String> = BTreeSet::new();
    let (entry_site, entry_fell_back) = entry_pair.choose(program_volatile);
    if entry_fell_back {
        fallbacks.insert(entry.to_string());
    }
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
    // shadowed, and it carries no column choice because the relocation
    // already names one blob (docs/core.md (linking)) — otherwise it goes
    // through the namespace, where the program's bit picks the column.
    // The flag is `true` when that pick had to fall back.
    let resolve_target = |object: &ObjectFile, oi: usize, sym: u32| -> Option<(Site, bool)> {
        match object.symbols[sym as usize].def {
            SymbolDef::Local { blob } => Some(((oi, blob), false)),
            _ => namespace
                .get(object.symbols[sym as usize].name.as_str())
                .map(|pair| pair.choose(program_volatile)),
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
                Some((callee, fell_back)) => {
                    if fell_back {
                        fallbacks.insert(object.symbols[reloc.symbol as usize].name.clone());
                    }
                    debug_assert!(
                        both_is_column_closed(object, site, callee),
                        "a Both blob may only call Both blobs in its own object"
                    );
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
                Some((callee, fell_back)) => {
                    if fell_back {
                        fallbacks.insert(object.symbols[bc.symbol as usize].name.clone());
                    }
                    debug_assert!(
                        both_is_column_closed(object, site, callee),
                        "a Both blob may only bind Both blobs in its own object"
                    );
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
    // name to the ONE site that would have been linked — the column this
    // program's bit chooses — so a name is dropped exactly when that
    // winning site went unreached. Shadowed library copies were never
    // candidates and are not reported. The fallback flag is deliberately
    // discarded here: `variant_fallbacks` names what LINKED, and nothing
    // in this loop did.
    let mut dropped: BTreeSet<String> = BTreeSet::new();
    for (&name, pair) in &namespace {
        let (site, _) = pair.choose(program_volatile);
        if !index_of.contains_key(&site) {
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
                origin: site.0,
            }
        })
        .collect();
    Ok(Resolved {
        order,
        dropped: dropped.into_iter().collect(),
        variant_fallbacks: fallbacks.into_iter().collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::formats::object::{BoundCall, ObjectFile, Relocation, Symbol, SymbolDef};
    use crate::linker::{ResolvedName, ResolvedNames, SymbolOrigin, resolve_names};

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

    /// Demote the named symbols to `Local` defs.
    fn make_local(object: &mut ObjectFile, locals: &[&str]) {
        for s in &mut object.symbols {
            if locals.contains(&s.name.as_str())
                && let SymbolDef::Defined { blob } = s.def
            {
                s.def = SymbolDef::Local { blob };
            }
        }
    }

    /// Like `obj`, but functions whose name is in `locals` get Local defs.
    fn obj_with_locals(arch: u8, funcs: &[(&str, &[&str])], locals: &[&str]) -> ObjectFile {
        let mut o = obj(arch, funcs);
        make_local(&mut o, locals);
        o
    }

    /// Like `obj`, but every entry carries its build-variant tag and is one
    /// BLOB, so a split function appears twice under one name. Bodies are
    /// distinguishable: each carries one `0x01` filler per column (Normal
    /// 1, Volatile 2, Both 3) right after its `ent`. Callee names bind
    /// **column-coherently** — from a split callee a `Normal` caller takes
    /// the normal symbol and a `Volatile` caller the volatile one, the
    /// shape the two-column compiler emits.
    fn variant_obj(arch: u8, funcs: &[(&str, BlobVariant, &[&str])]) -> ObjectFile {
        let fillers = |tag: BlobVariant| match tag {
            BlobVariant::Normal => 1,
            BlobVariant::Volatile => 2,
            BlobVariant::Both => 3,
        };
        // The symbol a `caller` column binds for `name`: the sole
        // definition when the name has one, else the matching column.
        let column_symbol = |name: &str, caller: BlobVariant| -> Option<usize> {
            let columns: Vec<usize> = funcs
                .iter()
                .enumerate()
                .filter(|(_, (n, _, _))| *n == name)
                .map(|(i, _)| i)
                .collect();
            match columns.len() {
                0 => None,
                1 => Some(columns[0]),
                _ => columns
                    .iter()
                    .find(|&&i| funcs[i].1 == caller)
                    .or(columns.first())
                    .copied(),
            }
        };
        let mut symbols: Vec<Symbol> = funcs
            .iter()
            .enumerate()
            .map(|(i, (n, _, _))| Symbol {
                name: (*n).into(),
                def: SymbolDef::Defined { blob: i as u32 },
            })
            .collect();
        let mut blobs = Vec::new();
        let mut relocations = Vec::new();
        for (bi, (_, tag, callees)) in funcs.iter().enumerate() {
            let mut blob = vec![0x0E];
            blob.resize(blob.len() + fillers(*tag), 0x01);
            for callee in *callees {
                let sym = column_symbol(callee, *tag).unwrap_or_else(|| {
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
        let variants = funcs.iter().map(|&(_, tag, _)| tag).collect();
        let mut object = ObjectFile::v2(arch, symbols, blobs, relocations, None);
        object.variants = Some(variants);
        object
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

    /// A `{Normal, Volatile}` same-name pair in one object is ONE
    /// definition in two build columns, not a duplicate: the name's
    /// namespace entry holds both, and a program with the bit clear links
    /// the normal one. Untagged, the same shape is two normal columns for
    /// one name — a duplicate, as it always was.
    #[test]
    fn a_variant_pair_in_one_object_is_not_a_duplicate_symbol() {
        // main is built twice; the volatile column's blob is a distinct
        // body, tagged so the namespace can tell the two apart.
        let mut a = obj(0x7E, &[("main", &[]), ("main", &[])]);
        a.blobs[1].push(0x0E);
        a.variants = Some(vec![BlobVariant::Normal, BlobVariant::Volatile]);
        let r = resolve(std::slice::from_ref(&a), &[], "main").expect("a variant pair links");
        let names: Vec<&str> = r.order.iter().map(|f| f.name.as_ref()).collect();
        assert_eq!(names, vec!["main"], "one definition reaches the image");
        assert_eq!(
            r.order[0].blob.as_ref(),
            a.blobs[0].as_slice(),
            "the normal column is the one linked"
        );

        // Without the tags the same shape stays a duplicate.
        let mut untagged = a.clone();
        untagged.variants = None;
        assert_eq!(
            resolve(std::slice::from_ref(&untagged), &[], "main").unwrap_err(),
            LinkError::DuplicateSymbol("main".into())
        );
    }

    #[test]
    fn the_program_bit_selects_the_entry_column_and_its_call_graph() {
        // The two-column compiler's own shape, in miniature: `main` splits
        // and its PRIVATE helper split with it, relocations
        // column-coherent. The program bit rides on the object defining
        // the entry symbol, and it decides both ends of the graph.
        let build = |program_volatile: bool| {
            let mut a = variant_obj(
                0x7E,
                &[
                    ("main", BlobVariant::Normal, &["helper"][..]),
                    ("main", BlobVariant::Volatile, &["helper"][..]),
                    ("helper", BlobVariant::Normal, &[][..]),
                    ("helper", BlobVariant::Volatile, &[][..]),
                ],
            );
            make_local(&mut a, &["helper"]);
            a.program_volatile = program_volatile;
            a
        };

        let volatile = build(true);
        let r = resolve(std::slice::from_ref(&volatile), &[], "main").unwrap();
        let linked: Vec<&[u8]> = r.order.iter().map(|f| f.blob.as_ref()).collect();
        assert_eq!(
            linked,
            vec![volatile.blobs[1].as_slice(), volatile.blobs[3].as_slice(),],
            "a volatile program enters the volatile main and reaches the volatile helper"
        );
        assert!(r.variant_fallbacks.is_empty(), "{:?}", r.variant_fallbacks);

        // The same object with the bit clear is today's link, unchanged.
        let normal = build(false);
        let r = resolve(std::slice::from_ref(&normal), &[], "main").unwrap();
        let linked: Vec<&[u8]> = r.order.iter().map(|f| f.blob.as_ref()).collect();
        assert_eq!(
            linked,
            vec![normal.blobs[0].as_slice(), normal.blobs[2].as_slice()],
        );
        assert!(r.variant_fallbacks.is_empty(), "{:?}", r.variant_fallbacks);
    }

    #[test]
    fn a_missing_column_falls_back_and_is_counted() {
        // A tag-free library is all-normal (no variant records at all), so
        // a volatile program takes its normal column — visible in the
        // report, never an error. The names are sorted and each is named
        // once however many times it is referenced.
        // `main` is deduped, so only the library's missing column can
        // force a fallback — the entry itself never does.
        let mut user = variant_obj(
            0x7E,
            &[("main", BlobVariant::Both, &["zeta", "alpha", "zeta"][..])],
        );
        user.program_volatile = true;
        let lib = obj(0x7E, &[("zeta", &[]), ("alpha", &[])]);
        assert!(lib.variants.is_none(), "the library is tag-free");
        let r = resolve(
            std::slice::from_ref(&user),
            std::slice::from_ref(&lib),
            "main",
        )
        .unwrap();
        assert_eq!(
            r.variant_fallbacks,
            vec!["alpha".to_string(), "zeta".to_string()]
        );

        // The same library in a normal program is no fallback at all.
        let mut plain = user.clone();
        plain.program_volatile = false;
        let r = resolve(
            std::slice::from_ref(&plain),
            std::slice::from_ref(&lib),
            "main",
        )
        .unwrap();
        assert!(r.variant_fallbacks.is_empty(), "{:?}", r.variant_fallbacks);
    }

    #[test]
    fn an_unreached_missing_column_is_not_counted() {
        // The counter names what LINKED: `dead` is defined in the tag-free
        // library and never reached, so it is dropped, not a fallback.
        let mut user = variant_obj(0x7E, &[("main", BlobVariant::Volatile, &[][..])]);
        user.program_volatile = true;
        let lib = obj(0x7E, &[("dead", &[])]);
        let r = resolve(
            std::slice::from_ref(&user),
            std::slice::from_ref(&lib),
            "main",
        )
        .unwrap();
        assert!(r.variant_fallbacks.is_empty(), "{:?}", r.variant_fallbacks);
        assert_eq!(r.dropped, vec!["dead".to_string()]);
    }

    #[test]
    fn a_fallback_entry_is_counted_too() {
        // A program bit with no volatile entry column to match — a
        // hand-written `.volatile` program whose `main` ships one body.
        // The entry never goes through a relocation, so it needs counting
        // at the entry lookup or it reads as a clean link.
        let mut a = variant_obj(0x7E, &[("main", BlobVariant::Normal, &[][..])]);
        a.program_volatile = true;
        let r = resolve(std::slice::from_ref(&a), &[], "main").unwrap();
        assert_eq!(r.order[0].blob.as_ref(), a.blobs[0].as_slice());
        assert_eq!(r.variant_fallbacks, vec!["main".to_string()]);
    }

    #[test]
    fn a_both_column_serves_either_program_without_a_fallback() {
        for program_volatile in [false, true] {
            let mut a = variant_obj(
                0x7E,
                &[
                    ("main", BlobVariant::Both, &["helper"][..]),
                    ("helper", BlobVariant::Both, &[][..]),
                ],
            );
            a.program_volatile = program_volatile;
            let r = resolve(std::slice::from_ref(&a), &[], "main").unwrap();
            let linked: Vec<&[u8]> = r.order.iter().map(|f| f.blob.as_ref()).collect();
            assert_eq!(
                linked,
                vec![a.blobs[0].as_slice(), a.blobs[1].as_slice()],
                "the one deduped column serves both program kinds"
            );
            assert!(
                r.variant_fallbacks.is_empty(),
                "program_volatile = {program_volatile}: {:?}",
                r.variant_fallbacks
            );
        }
    }

    #[test]
    fn only_a_normal_volatile_pair_shares_one_name_in_one_object() {
        // Two columns of one name are ONE definition; anything else under
        // one name is the duplicate it has always been.
        for tags in [
            [BlobVariant::Normal, BlobVariant::Normal],
            [BlobVariant::Volatile, BlobVariant::Volatile],
            [BlobVariant::Both, BlobVariant::Both],
            [BlobVariant::Both, BlobVariant::Normal],
            [BlobVariant::Volatile, BlobVariant::Both],
        ] {
            let a = variant_obj(
                0x7E,
                &[("main", tags[0], &[][..]), ("main", tags[1], &[][..])],
            );
            assert_eq!(
                resolve(std::slice::from_ref(&a), &[], "main").unwrap_err(),
                LinkError::DuplicateSymbol("main".into()),
                "{tags:?} is not a legal column pair"
            );
        }
    }

    #[test]
    fn a_shadowing_normal_only_pair_forces_a_counted_fallback() {
        // Rules 2 and 3 compose. A name's pair is filled from ONE input,
        // and shadowing takes that input's pair WHOLE — so a user object
        // offering only `f`'s normal column shadows a library shipping the
        // complete pair, and a volatile program falls back on `f` and
        // counts it although a volatile column WAS on the link line. This
        // pins that pairs are never completed across inputs: a refactor
        // borrowing the library's volatile column to fill the user's empty
        // slot would violate rule 2 with every other test still green.
        let mut user = variant_obj(
            0x7E,
            &[
                ("main", BlobVariant::Normal, &["f"][..]),
                ("main", BlobVariant::Volatile, &["f"][..]),
                ("f", BlobVariant::Normal, &[][..]),
            ],
        );
        user.program_volatile = true;
        let lib = variant_obj(
            0x7E,
            &[
                ("f", BlobVariant::Normal, &[][..]),
                ("f", BlobVariant::Volatile, &[][..]),
            ],
        );
        let r = resolve(
            std::slice::from_ref(&user),
            std::slice::from_ref(&lib),
            "main",
        )
        .unwrap();
        assert_eq!(r.variant_fallbacks, vec!["f".to_string()]);
        let f = r
            .order
            .iter()
            .find(|func| func.name == "f")
            .expect("f is reached");
        // Provenance is the discriminator: the two normal bodies are
        // byte-identical, so only the origin index tells them apart.
        assert_eq!(
            f.origin, 0,
            "the user object's column won, not the library's"
        );
        assert_eq!(
            f.blob.as_ref(),
            user.blobs[2].as_slice(),
            "the shadowing normal body linked, not the library's volatile column"
        );

        // The same rule between two libraries: first-wins keeps the
        // earlier library's one-column pair whole.
        let mut caller = variant_obj(0x7E, &[("main", BlobVariant::Volatile, &["g"][..])]);
        caller.program_volatile = true;
        let first = variant_obj(0x7E, &[("g", BlobVariant::Normal, &[][..])]);
        let second = variant_obj(
            0x7E,
            &[
                ("g", BlobVariant::Normal, &[][..]),
                ("g", BlobVariant::Volatile, &[][..]),
            ],
        );
        let libs = [first, second];
        let r = resolve(std::slice::from_ref(&caller), &libs, "main").unwrap();
        assert_eq!(r.variant_fallbacks, vec!["g".to_string()]);
        let g = r
            .order
            .iter()
            .find(|func| func.name == "g")
            .expect("g is reached");
        assert_eq!(
            g.origin, 1,
            "the FIRST library won (origin = objects.len() + 0)"
        );
    }

    #[test]
    fn columns_of_one_name_never_span_two_objects() {
        // Rule: a name's pair is filled from ONE object. Two user objects
        // each contributing a column is the duplicate it was before.
        let a = variant_obj(
            0x7E,
            &[
                ("main", BlobVariant::Normal, &[][..]),
                ("f", BlobVariant::Normal, &[][..]),
            ],
        );
        let b = variant_obj(0x7E, &[("f", BlobVariant::Volatile, &[][..])]);
        assert_eq!(
            resolve(&[a, b], &[], "main").unwrap_err(),
            LinkError::DuplicateSymbol("f".into())
        );
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

    #[test]
    fn resolve_names_reports_reached_with_provenance_and_dropped() {
        // main (object 0) calls lib_fn (library 0); helper in object 0 unreached.
        let a = obj(0x7E, &[("main", &["lib_fn"]), ("helper", &[])]);
        let lib = obj(0x7E, &[("lib_fn", &[])]);
        let names =
            resolve_names(std::slice::from_ref(&a), std::slice::from_ref(&lib), "main").unwrap();
        assert_eq!(
            names,
            ResolvedNames {
                reached: vec![
                    ResolvedName {
                        name: "main".into(),
                        origin: SymbolOrigin::Object(0),
                    },
                    ResolvedName {
                        name: "lib_fn".into(),
                        // Pins the off-by-one: the library is the sole
                        // library, so it MUST report Library(0), not
                        // Library(objects.len()) (= Library(1)).
                        origin: SymbolOrigin::Library(0),
                    },
                ],
                dropped: vec!["helper".to_string()],
            }
        );
    }

    #[test]
    fn resolve_names_user_definition_shadows_library() {
        // "dup" defined in object 0 AND library 0; main calls dup.
        let a = obj(0x7E, &[("main", &["dup"]), ("dup", &[])]);
        let lib = obj(0x7E, &[("dup", &[])]);
        let names =
            resolve_names(std::slice::from_ref(&a), std::slice::from_ref(&lib), "main").unwrap();
        let dup = names
            .reached
            .iter()
            .find(|r| r.name == "dup")
            .expect("dup is reached");
        assert_eq!(dup.origin, SymbolOrigin::Object(0));
    }

    #[test]
    fn resolve_names_reachable_unresolved_is_an_error() {
        // main references "ghost" defined nowhere.
        let a = obj(0x7E, &[("main", &["ghost"])]);
        let e = resolve_names(std::slice::from_ref(&a), &[], "main").unwrap_err();
        assert_eq!(e, LinkError::Unresolved(vec!["ghost".into()]));
    }

    #[test]
    fn resolve_names_dead_code_may_be_broken() {
        // unreached fn references "ghost" -> Ok, ghost never mentioned,
        // the broken fn appears in dropped.
        let a = obj(0x7E, &[("main", &[]), ("dead", &["ghost"])]);
        let names = resolve_names(std::slice::from_ref(&a), &[], "main").unwrap();
        assert_eq!(
            names,
            ResolvedNames {
                reached: vec![ResolvedName {
                    name: "main".into(),
                    origin: SymbolOrigin::Object(0),
                }],
                dropped: vec!["dead".to_string()],
            }
        );
    }
}
