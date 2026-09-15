//! Mono stamping and hybrid classification for the composition engine
//! (docs/core.md (the composition engine)).
//!
//! MONO lowers each reachable declarative bound-call site to a plain call
//! into a **stamp**: a specialized copy of the callee's generic body,
//! re-emitted at the CALLER's machine width with the composite's per-tape
//! symbol maps folded in. The result runs on the base profile — no runtime
//! compose table, no frame descriptors. Because a stamp reads and writes
//! physical symbols directly, the projection materializes the maps three
//! ways:
//!
//! - `wr`/`mov` vector operands are rewritten by PROJECTION: the output
//!   vector is machine-width, position `phys(k)` carries the translated
//!   element from callee position `k`, and every unbound position keeps
//!   (`0x7F` for writes) or stays (`0` for moves). A write payload with no
//!   physical image (a `wmap` hole) turns the whole instruction into the
//!   dialect's `trap #1` (unmapped write).
//! - match-table rows expand from callee width to machine width. A cell is
//!   a callee-virtual symbol the routine expects to READ; the stamp reads
//!   physical symbols, so the cell becomes the physical symbols whose `rmap`
//!   image is that virtual one — the `rmap` PREIMAGE. A one-way collapse
//!   (several physical symbols reading as one virtual) expands the row into
//!   one row per preimage; a virtual cell with no physical preimage makes
//!   the row dead (dropped, with the paired dispatch entry removed).
//! - each bound tape with `rmap` holes gets synthesized unmapped-read trap
//!   rows: one machine-width row per hole physical symbol, dispatching to a
//!   shared `trap #0` stub. Given each machine position is projected by at
//!   most one callee tape, a hole symbol reaches its trap row because no
//!   surviving row can match it, not because of where the row sits.
//!
//! Renaming cells permutes rows out of the order the callee's table was
//! authored in, so the finished table is sorted back into the canonical row
//! order (docs/core.md (match tables)) with each row's dispatch entry moving
//! alongside it — a stamped table is as expressible in assembly as the
//! authored one it specializes.
//!
//! HYBRID classifies per site: a completed bijection (equal-size,
//! injective, no holes, no one-way pairs) is mono-stamped; anything holey or
//! one-way keeps the frames path. One image can carry both — its profile is
//! FRAMES whenever any frames site survives.

use std::borrow::Cow;
use std::collections::{HashMap, HashSet, VecDeque};

use super::LinkError;
use super::compose::{
    Composite, CompositeTape, canonical_key, compose, digest, identity_composite,
    is_full_passthrough,
};
use super::engine::{
    EngineStats, Lowered, SiteKind, bad_binding, lower_frames, routine_sig, scan_sites,
};
use super::resolve::FuncRef;
use crate::asm::decode::{self, Body, DecodedOperand};
use crate::asm::{ArchSyntax, Flow, MatchRowClass, classify_match_row};
use crate::formats::object::{BoundCall, RoutineSig};
use crate::vm::OperandKind;

/// One (routine, composite) pair to stamp, with its map-visible name. An
/// EXIT-BEARING node also carries the call site it splices (docs/core.md
/// (call mechanisms)).
struct StampNode {
    routine: usize,
    composite: Composite,
    name: String,
    site: Option<PendingSite>,
}

/// Where a mono exit-bearing copy returns to (docs/core.md (call
/// mechanisms)). TM-1 has no pop opcode, so such a copy is ENTERED by a
/// jump and leaves through jumps too: a plain `ret` lands on `then` — the
/// instruction after the call — and `retx #k` on exit `k`.
#[derive(Clone, Debug)]
struct SpliceSite {
    /// The calling function's index in `order`.
    caller: usize,
    /// The blob offset of the instruction after the call — where `ret`
    /// lands.
    then: u32,
    /// The blob offsets of the site's exits — where `retx #k` lands.
    exits: Vec<u32>,
}

/// A splice site as the stamp closure records it. The offsets a fixup
/// needs are offsets in the CALLER's final blob, and for a site nested
/// inside another stamp that blob does not exist until the closure is
/// done — so the coordinate system travels with the site and the
/// translation happens at materialization (docs/core.md (call
/// mechanisms)).
#[derive(Clone, Debug)]
enum PendingSite {
    /// The caller is a hand-written routine, whose post-rewrite blob is
    /// already determined: the offsets are final.
    Resolved(SpliceSite),
    /// The caller is another stamp, emitted later in the materialization
    /// loop: the offsets are in the ORIGINAL routine's blob and are
    /// translated through that stamp's own offset map once it is built.
    InStamp {
        slot: usize,
        then: u32,
        exits: Vec<u32>,
    },
}

impl PendingSite {
    /// The `(caller, then, exits)` triple that widens the stamp intern key
    /// (docs/core.md (the composition engine)). Each variant reports it in
    /// its OWN coordinate system, which is all the key needs: it must tell
    /// two distinct splices apart, never compare across variants — a site
    /// inside a stamp and a site inside a hand-written routine already
    /// differ in the caller index.
    fn key_parts(&self, order_len: usize) -> (usize, u32, &[u32]) {
        match self {
            PendingSite::Resolved(s) => (s.caller, s.then, &s.exits),
            PendingSite::InStamp { slot, then, exits } => (order_len + slot, *then, exits),
        }
    }
}

/// The finished bytes of one stamped function, ready to wrap in a `FuncRef`.
struct StampBody {
    blob: Vec<u8>,
    table: Vec<u8>,
    table_fixups: Vec<(u32, u32)>,
    calls: Vec<(u32, usize)>,
    /// Cross-function jump fixups this copy's `ret`/`retx` rewrites left
    /// for layout (docs/core.md (call mechanisms)); empty for an ordinary
    /// stamp.
    site_fixups: Vec<(u32, usize, u32)>,
    /// This copy's offset map: the original routine's blob offsets to the
    /// copy's own. A splice nested inside this copy is translated through
    /// it.
    offsets: HashMap<u32, u32>,
    /// Unmapped-read trap rows synthesized into this stamp's match tables.
    trap_rows: u32,
    /// Extra match rows this stamp gained from one-way collapse expansion.
    expanded_rows: u32,
}

/// Report counters accumulated while building the mono stamp set — folded
/// into the link report's engine counters (docs/core.md (the link report)).
#[derive(Debug, Default, Clone, Copy)]
struct StampStats {
    /// (routine, composite) pairs that resolved to an already-built stamp.
    dedup_savings: u32,
    synthesized_trap_rows: u32,
    expanded_rows: u32,
}

/// A rewritten match row's dispatch source: the shared read-trap stub, or a
/// surviving/expanded original row (its old dispatch entry follows it).
enum EntrySrc {
    TrapStub,
    OldRow(u16),
}

/// Whether a pending match-row remap carries a synthesized unmapped-read
/// trap row. Such a remap MUST be consumed by a dispatch rebuild — that is
/// the only path that routes those trap rows to the trap stub. A
/// trap-bearing remap left unconsumed (or overwritten by a later match
/// table) means a hole symbol would match a trap row and be read through a
/// branch as a real match — the guard in `build_stamp` refuses that stamp.
fn remap_has_trap(remap: &Option<Vec<EntrySrc>>) -> bool {
    remap
        .as_deref()
        .is_some_and(|r| r.iter().any(|e| matches!(e, EntrySrc::TrapStub)))
}

/// A rebuilt dispatch entry pending offset resolution: the trap stub (its
/// stamp-blob offset is known only after the body is emitted) or an original
/// dispatch target (an old callee-blob code offset, remapped through the
/// stamp's own offset map).
enum DispEntry {
    TrapStub,
    OldCode(u32),
}

// -- MONO --------------------------------------------------------------------

/// Lower every reachable declarative bound call under MONO: stamp a
/// specialized copy per (routine, composite) and retarget each site to a
/// plain call into it. The image stays on the base profile, so no
/// `FramesPlan` is produced. The last element is the sorted names
/// `prune_unreachable` dropped — folded into the link report's `dropped`
/// list by the caller (docs/core.md (the link report)).
pub(super) fn lower_mono<'a>(
    syntax: &ArchSyntax,
    order: Vec<FuncRef<'a>>,
    sites: &[Vec<SiteKind<'a>>],
    machine_sig: &RoutineSig,
) -> Result<Lowered<'a>, LinkError> {
    let n = order.len();
    let id_world = identity_world(sites, n);

    // A raw framed call reachable at the machine's own frame is a
    // contradiction under mono — the base-profile image has no compose
    // machinery to activate its descriptor (nested reach is caught by the
    // stamp closure below).
    for (fi, in_world) in id_world.iter().enumerate() {
        if *in_world
            && sites[fi]
                .iter()
                .any(|s| matches!(s, SiteKind::RawCallM { .. }))
        {
            return Err(LinkError::MonoRawFrame(order[fi].name.to_string()));
        }
    }

    // Seed the stamp closure from every non-collapse bound site reachable at
    // the machine's own frame; collapse sites lower to a plain call into the
    // original routine (the identity collapse), never a stamp.
    let mut seeds: Vec<(usize, u32, usize, &BoundCall)> = Vec::new();
    for (fi, in_world) in id_world.iter().enumerate() {
        if !in_world {
            continue;
        }
        for site in &sites[fi] {
            if let SiteKind::Bound {
                addr,
                callee,
                record,
                collapse: false,
            } = site
            {
                // An exit-bearing site is entered by a jump, not a call
                // (docs/core.md (call mechanisms)) — check here, while the
                // caller and the callee's name are still in hand, so the
                // retarget loop below cannot fail.
                if !record.exits.is_empty() {
                    check_splice_site(syntax, &order[fi], *addr, &order[*callee].name)?;
                }
                seeds.push((fi, *addr, *callee, record));
            }
        }
    }

    // Mono rewrites no blob, so nothing shifts: every splice offset is
    // already in its caller's final coordinates.
    let unshifted: Vec<HashSet<u32>> = vec![HashSet::new(); n];
    // Pure mono shares nothing: every exit-bearing site splices, wherever
    // it sits (docs/core.md (call mechanisms)).
    let nothing_shared: HashSet<GroupKey> = HashSet::new();
    let (stamps, seed_target, stats) = mono_stamps(
        syntax,
        &order,
        sites,
        machine_sig,
        &seeds,
        &unshifted,
        &nothing_shared,
    )?;
    // `dedup_savings`, `synthesized_trap_rows`, and `expanded_rows` are
    // summed over every stamp `mono_stamps` builds, before the prune below
    // runs. That is sound because `mono_stamps` closes "mono all the way
    // down" from a seed already known reachable (`id_world`) — every stamp
    // it mints keeps at least the caller that seeded or called it, so
    // nothing built here is ever orphaned by the prune (the `debug_assert!`
    // near the prune call checks exactly this, in debug builds). Unlike
    // `instantiations` below, re-deriving these three from the post-prune
    // survivors would need `mono_stamps` to attribute each count to a
    // specific stamp rather than fold it into one running total as it
    // builds — `dedup_savings` in particular is a per-call-site event, not
    // a per-stamp one, so there is no single survivor to attribute a saving
    // to. Not worth the restructuring for counters this diagnostic.
    let stamp_names: HashSet<String> = stamps.iter().map(|f| f.name.to_string()).collect();

    // Retarget every original's bound sites to a plain call. Reachable sites
    // hit the stamp (or the original, for a collapse); a bound site in a
    // routine unreachable at the machine's frame never runs, so it points
    // harmlessly at the original callee.
    // An exit-bearing site is ENTERED by `jmp`, not `call`: the copy never
    // returns through a pushed address (docs/core.md (call mechanisms)).
    // The site keeps its 5-byte shape and its relocation hole, so only the
    // opcode byte changes and no offset moves — layout relocates the jump
    // through the same call-site path it relocates a call through, which is
    // why the `calls` entry below stays exactly as it was.
    let jmp = syntax.jump_opcode();
    let mut out: Vec<FuncRef> = Vec::with_capacity(n + stamps.len());
    for (fi, mut f) in order.into_iter().enumerate() {
        for site in &sites[fi] {
            if let SiteKind::Bound {
                addr,
                callee,
                record,
                collapse,
            } = site
            {
                let target = if id_world[fi] && !collapse {
                    seed_target[&(fi, *addr)]
                } else {
                    *callee
                };
                f.calls.push((*addr + 1, target));
                if id_world[fi] && !collapse && !record.exits.is_empty() {
                    let op = jmp.expect("an exit-bearing seed resolved the jump opcode above");
                    f.blob.to_mut()[*addr as usize] = op;
                }
            }
        }
        f.bound = Vec::new();
        f.calls.sort_by_key(|&(h, _)| h);
        out.push(f);
    }
    out.extend(stamps);

    // Every site that used to call a now-orphaned generic was just
    // retargeted above (docs/core.md (the composition engine)) — restore the
    // resolve-time reachability guarantee over the retargeted graph.
    let (out, orphaned) = prune_unreachable(out);

    // `instantiations` — unlike the three counters above — is cheap to make
    // immune to a future closure-invariant break: count the minted names
    // actually still present in `out`, not `stamps.len()` from before the
    // prune ran. Under the invariant this equals `stamp_names.len()`
    // exactly, so the `debug_assert!` below stays a meaningful check in
    // debug builds; a violation would silently under-report here in EVERY
    // build, release included, rather than just tripping the assert.
    let instantiations = u32::try_from(
        out.iter()
            .filter(|f| stamp_names.contains(f.name.as_ref()))
            .count(),
    )
    .unwrap_or(u32::MAX);
    let engine_stats = EngineStats {
        instantiations,
        dedup_savings: stats.dedup_savings,
        synthesized_trap_rows: stats.synthesized_trap_rows,
        expanded_rows: stats.expanded_rows,
    };
    debug_assert!(
        stamp_names
            .iter()
            .all(|name| out.iter().any(|f| f.name == *name)),
        "mono stamping must never orphan a stamp it just minted"
    );
    Ok(Lowered {
        order: out,
        plan: None,
        stats: engine_stats,
        orphaned,
        diagnostics: Vec::new(),
        folds: Vec::new(),
    })
}

// -- HYBRID ------------------------------------------------------------------

/// A fold group's identity: the callee's index in `order` paired with the
/// canonical key of the composite its sites reach (docs/core.md (call
/// mechanisms)). Two exit-bearing sites fold together exactly when they
/// agree on both — wherever in the reachable set each was met.
type GroupKey = (usize, Vec<u8>);

/// One fold group's members (docs/core.md (call mechanisms)).
type GroupSites<'a> = Vec<GroupSite<'a>>;

/// One exit-bearing bijection site in a fold group. An IDENTITY-WORLD site
/// names the calling function in `order` and its blob offset there, which
/// is what a splice needs to seed itself. A CLOSURE site was met inside a
/// copy, where neither number means anything in `order`'s coordinates:
/// such a member is spliced by the stamp closure itself when its group is
/// refused, so the only thing the decision needs from it is its record.
enum GroupSite<'a> {
    Identity {
        caller: usize,
        addr: u32,
        record: &'a BoundCall,
    },
    InClosure {
        record: &'a BoundCall,
    },
}

impl<'a> GroupSite<'a> {
    /// The site's call record, whichever variant carries it — the exit
    /// count is what sizes its would-be descriptor.
    fn record(&self) -> &'a BoundCall {
        match self {
            GroupSite::Identity { record, .. } | GroupSite::InClosure { record } => record,
        }
    }
}

/// One exit-bearing bijection site the closure probe met inside a copy
/// (docs/core.md (call mechanisms)).
struct ClosureSite<'a> {
    /// The callee the site reaches.
    callee: usize,
    record: &'a BoundCall,
    /// `compose(enclosing, binding)` — absolute, and exactly the descriptor
    /// a shared site is given.
    composite: Composite,
}

/// What a stamped body does at one of its own call sites.
enum StampTarget {
    /// A plain call into a child stamp, or into the original on a full
    /// pass-through. `splice` marks a site ENTERED by a jump rather than a
    /// call — an exit-bearing site whose group was refused sharing
    /// (docs/core.md (call mechanisms)).
    Plain { target: usize, splice: bool },
    /// A framed call into the ONE generic copy of the callee, through a
    /// descriptor the stamp carries in its own table blob.
    Framed {
        callee: usize,
        composite: Composite,
        /// The site's exits, as offsets in the ENCLOSING routine's original
        /// blob; `build_stamp` remaps them into the stamp's own blob before
        /// writing them into the descriptor.
        exits: Vec<u32>,
    },
}

/// Lower under HYBRID: mono-stamp the completed-bijection sites, hand the
/// rest to the frames path. If every non-collapse site is a bijection this
/// is pure mono; if none is, pure frames; otherwise both, and the image is
/// FRAMES. The last element is the sorted names pruned along the way (empty
/// on the pure-frames branch, which never stamps and so never orphans
/// anything) — see `lower_mono`.
pub(super) fn lower_hybrid<'a>(
    syntax: &ArchSyntax,
    order: Vec<FuncRef<'a>>,
    sites: &[Vec<SiteKind<'a>>],
    machine_sig: &RoutineSig,
) -> Result<Lowered<'a>, LinkError> {
    let n = order.len();
    let id_world = identity_world(sites, n);

    // Classify the machine-frame bound sites: a bijection is a mono seed,
    // anything holey/one-way is a frames site.
    let mut seeds: Vec<(usize, u32, usize, &BoundCall)> = Vec::new();
    let mut mono_holes: Vec<HashSet<u32>> = vec![HashSet::new(); n];
    // The subset of `mono_holes` that splices: those sites are entered by
    // a jump, so their opcode byte changes (docs/core.md (call
    // mechanisms)).
    let mut mono_splices: Vec<HashSet<u32>> = vec![HashSet::new(); n];
    let mut any_frames = false;
    // Exit-bearing bijection sites are GROUPED rather than seeded
    // directly: whether they share one framed body or each splice a copy
    // is a byte count over the whole group, not a per-site property
    // (docs/core.md (call mechanisms)).
    //
    // The fold count runs over the whole reachable set — an exit-bearing
    // site met inside a stamped copy joins its group by the same
    // (routine, composite) key. This loop sees the machine-frame sites;
    // the closure probe that reports the rest merges into the same map
    // before the decision is taken.
    let mut groups: HashMap<GroupKey, GroupSites> = HashMap::new();
    // Every member of a group reaches the key's composite by
    // construction, so the first member's is the group's.
    let mut group_composite: HashMap<GroupKey, Composite> = HashMap::new();
    // The decision phase drives the same [`Closure`] the probe below
    // walks: a group key is `compose(identity, binding)` for a site at the
    // machine's own frame, which is the very composition the walk does for
    // a seed — taken from the one place that owns it.
    let mut closure = Closure::new(&order, machine_sig);
    let id = closure.identity();
    for (fi, in_world) in id_world.iter().enumerate() {
        if !in_world {
            continue;
        }
        for site in &sites[fi] {
            if let SiteKind::Bound {
                addr,
                callee,
                record,
                collapse: false,
            } = site
            {
                let callee_sig = routine_sig(&order, *callee)?;
                let caller_sig = order[fi].signature.unwrap_or(machine_sig);
                if is_bijection(caller_sig, callee_sig, record) {
                    if record.exits.is_empty() {
                        seeds.push((fi, *addr, *callee, record));
                        mono_holes[fi].insert(*addr);
                    } else {
                        let (composite, _) = closure.bound_child(&id, fi, *callee, record)?;
                        let key = (*callee, canonical_key(&composite));
                        group_composite.entry(key.clone()).or_insert(composite);
                        groups.entry(key).or_default().push(GroupSite::Identity {
                            caller: fi,
                            addr: *addr,
                            record,
                        });
                    }
                } else {
                    any_frames = true;
                }
            }
        }
    }

    // The fold count runs over the WHOLE reachable set, not only the
    // identity world (docs/core.md (call mechanisms)): an exit-bearing
    // site met while a routine is copied under composite `C` joins its
    // group, and a shared group is reached from inside that copy through
    // a descriptor `compose(C, binding)` plus the site's exits.
    for cs in mono_closure_probe(&mut closure, sites, &seeds) {
        let key = (cs.callee, canonical_key(&cs.composite));
        group_composite.entry(key.clone()).or_insert(cs.composite);
        groups
            .entry(key)
            .or_default()
            .push(GroupSite::InClosure { record: cs.record });
    }

    // The byte rule (docs/core.md (call mechanisms)): with `k` sites, a
    // body of `B` bytes and would-be descriptors of `d_i` bytes, share iff
    // `k >= 2 && (k - 1) * B > sum(d_i)`. Both counts are pre-layout, so
    // the decision is deterministic and a relink is byte-identical.
    //
    // This runs BEFORE both fast paths below: it is what fills `seeds` for
    // a spliced group and what sets `any_frames` for a shared one, so a
    // fast path taken ahead of it would branch on a state the rule has not
    // produced yet.
    let mut folds: Vec<super::FoldDecision> = Vec::new();
    // The `(callee, composite)` pairs the rule shared: consulted by the
    // stamp closure, which frames such a site instead of minting a child.
    let mut shared_pairs: HashSet<GroupKey> = HashSet::new();
    let mut keys: Vec<&GroupKey> = groups.keys().collect();
    keys.sort();
    for key in keys {
        let sites_in_group = &groups[key];
        let callee = key.0;
        let body = u32::try_from(order[callee].blob.len() + order[callee].table.len())
            .expect("a body size fits u32");
        let k = u32::try_from(sites_in_group.len()).expect("a group size fits u32");
        let descriptors: u32 = sites_in_group
            .iter()
            .map(|s| {
                descriptor_cost(
                    &group_composite[key],
                    machine_sig,
                    &order,
                    &s.record().exits,
                )
            })
            .sum::<Result<u32, LinkError>>()?;
        let shared = k >= 2 && u64::from(k - 1) * u64::from(body) > u64::from(descriptors);
        folds.push(super::FoldDecision {
            routine: order[callee].name.to_string(),
            sites: k,
            body_bytes: body,
            descriptor_bytes: descriptors,
            shared,
        });
        if shared {
            // A shared group keeps its IDENTITY-WORLD sites in `f.bound`,
            // so the frames path lowers them — with their exit vectors —
            // against the one generic body. A closure member reaches the
            // same body through a descriptor the stamp carries in its own
            // table blob, which the stamp closure emits when it finds the
            // pair here.
            any_frames = true;
            shared_pairs.insert(key.clone());
        } else {
            // Only an identity-world member is seeded: a closure member
            // names no function in `order` and no offset in one, and is
            // spliced by the stamp closure itself — which is exactly what
            // it does for every pair NOT in `shared_pairs`.
            for s in sites_in_group {
                let GroupSite::Identity {
                    caller: fi,
                    addr,
                    record,
                } = *s
                else {
                    continue;
                };
                // Checked here, while the caller and the callee's name are
                // in hand, so the promotion loop below cannot fail. A
                // SHARED group needs neither a jump opcode nor a `then`,
                // which is why the check sits on this branch alone.
                check_splice_site(syntax, &order[fi], addr, &order[callee].name)?;
                seeds.push((fi, addr, callee, record));
                mono_holes[fi].insert(addr);
                // `mono_splices` moves WITH `mono_holes`: a site that is a
                // mono seed and carries exits is entered by a jump, and a
                // reclassification that moved only one of the two would
                // leave the site a `call` whose copy returns through an
                // address nobody pushed.
                mono_splices[fi].insert(addr);
            }
        }
    }
    // Stable, so groups that tie on (routine, sites) keep the sorted-key
    // order above rather than the hash map's.
    folds.sort_by(|a, b| (&a.routine, a.sites).cmp(&(&b.routine, b.sites)));

    // The two degenerate cases route straight to a single mechanism. Each
    // returns somebody else's `Lowered`, so the fold decisions — taken
    // above, and reportable from nowhere else — are attached here.
    if seeds.is_empty() {
        let (order, plan, stats) = lower_frames(syntax, order, sites, machine_sig)?;
        return Ok(Lowered {
            order,
            plan,
            stats,
            orphaned: Vec::new(),
            diagnostics: Vec::new(),
            folds,
        });
    }
    if !any_frames {
        // Sound only because nothing was shared: sharing sets
        // `any_frames`, so reaching here means every exit-bearing group
        // spliced, which is exactly what `lower_mono` does with the seeds
        // it re-derives from `sites` for itself.
        let mut lowered = lower_mono(syntax, order, sites, machine_sig)?;
        lowered.folds = folds;
        return Ok(lowered);
    }

    // Mixed: build the mono stamps, promote the bijection bound sites to
    // plain calls into them (dropping those bound records), then let the
    // frames path lower whatever bound records remain. `dedup_savings`,
    // `synthesized_trap_rows`, and `expanded_rows` are summed before the
    // prune below runs, for the same closure-invariant reason `lower_mono`
    // documents at its own `mono_stamps` call.
    // The offsets the frames path will shift, per function: the bound
    // sites that are still framed once the mono/frames split is decided.
    // A splice fixup names an offset in the CALLER's blob and layout's
    // map is keyed by POST-rewrite offsets, so `mono_stamps` must know
    // which sites `lower_frames` is about to widen 5 → 9 bytes below.
    let widened: Vec<HashSet<u32>> = (0..n)
        .map(|fi| {
            sites[fi]
                .iter()
                .filter_map(|s| match s {
                    SiteKind::Bound {
                        addr,
                        collapse: false,
                        ..
                    } if !mono_holes[fi].contains(addr) => Some(*addr),
                    _ => None,
                })
                .collect()
        })
        .collect();
    let (stamps, seed_target, mono_stats) = mono_stamps(
        syntax,
        &order,
        sites,
        machine_sig,
        &seeds,
        &widened,
        &shared_pairs,
    )?;
    let stamp_names: HashSet<String> = stamps.iter().map(|f| f.name.to_string()).collect();

    let jmp = syntax.jump_opcode();
    let mut new_order: Vec<FuncRef> = Vec::with_capacity(n + stamps.len());
    for (fi, mut f) in order.into_iter().enumerate() {
        if !mono_holes[fi].is_empty() {
            for &addr in &mono_holes[fi] {
                f.calls.push((addr + 1, seed_target[&(fi, addr)]));
            }
            // An exit-bearing site is entered by `jmp`, exactly as under
            // pure mono (docs/core.md (call mechanisms)); the rewrite
            // below then treats it as a relocated tail jump and the site
            // keeps its 5-byte shape.
            for &addr in &mono_splices[fi] {
                let op = jmp.expect("an exit-bearing seed resolved the jump opcode above");
                f.blob.to_mut()[addr as usize] = op;
            }
            let monos = &mono_holes[fi];
            f.bound.retain(|&(hole, _, _)| !monos.contains(&(hole - 1)));
            f.calls.sort_by_key(|&(h, _)| h);
        }
        new_order.push(f);
    }
    new_order.extend(stamps);

    // Same reachability re-check as pure mono, run BEFORE the frames path
    // re-scans below — so any orphaned generic is gone from the graph
    // `lower_frames` builds its directory and compose-column order over,
    // never needing a post-hoc index remap (docs/core.md (the composition
    // engine)).
    let (new_order, orphaned) = prune_unreachable(new_order);

    // Same immunity as `lower_mono`: count survivors, not `stamps.len()`
    // from before the prune ran.
    let instantiations = u32::try_from(
        new_order
            .iter()
            .filter(|f| stamp_names.contains(f.name.as_ref()))
            .count(),
    )
    .unwrap_or(u32::MAX);
    debug_assert!(
        stamp_names
            .iter()
            .all(|name| new_order.iter().any(|f| f.name == *name)),
        "mono stamping must never orphan a stamp it just minted"
    );

    // The frames path re-scans the mono-rewritten order; the stamps carry no
    // bound calls, so they flow through as ordinary functions — except that
    // a stamp reaching a SHARED body carries a raw framed call, which the
    // re-scan reports as `RawCallM` and the frames path gives a directory
    // entry and a constant compose column, exactly as it does a
    // hand-authored one (docs/core.md (call mechanisms)).
    let new_sites: Vec<Vec<SiteKind>> = new_order
        .iter()
        .map(|f| scan_sites(syntax, f, machine_sig, &new_order))
        .collect::<Result<_, _>>()?;
    let (order, plan, frames_stats) = lower_frames(syntax, new_order, &new_sites, machine_sig)?;
    // A hybrid image accounts for BOTH mechanisms: the mono stamps' counters
    // plus the frames path's descriptor dedup.
    let stats = EngineStats {
        instantiations,
        dedup_savings: mono_stats.dedup_savings + frames_stats.dedup_savings,
        synthesized_trap_rows: mono_stats.synthesized_trap_rows,
        expanded_rows: mono_stats.expanded_rows,
    };
    Ok(Lowered {
        order,
        plan,
        stats,
        orphaned,
        diagnostics: Vec::new(),
        folds,
    })
}

// -- the one closure walk ----------------------------------------------------

/// The stamp closure's walk, driven by both the fold probe and the stamp
/// builder (docs/core.md (the composition engine)). It owns the queue, the
/// node table and its intern key, and the ONE place a child composite is
/// composed from a call site under the composite its caller runs under.
///
/// The two drivers ask different questions of the same closure — the
/// probe only counts what it meets, the builder emits bodies and refuses
/// what mono cannot lower — but they must agree on its SHAPE: which pairs
/// are distinct, which sites descend, and what each child composes to. A
/// walk copied into both is a walk whose every difference has to be
/// re-argued on every change; this is that walk, once.
struct Closure<'a, 'o> {
    order: &'o [FuncRef<'a>],
    machine_sig: &'o RoutineSig,
    /// The interned nodes, in intern order: slot `i` is `nodes[i]`, and a
    /// node nested inside another always interns after it.
    nodes: Vec<StampNode>,
    key_to_slot: HashMap<Vec<u8>, usize>,
    worklist: VecDeque<usize>,
    /// Every name already in play — every hand-written routine `order`
    /// carries, then every stamp minted as the walk runs — so `intern`
    /// can refuse a freshly computed name that collides with either.
    used_names: HashSet<String>,
}

impl<'a, 'o> Closure<'a, 'o> {
    fn new(order: &'o [FuncRef<'a>], machine_sig: &'o RoutineSig) -> Self {
        Closure {
            order,
            machine_sig,
            nodes: Vec::new(),
            key_to_slot: HashMap::new(),
            worklist: VecDeque::new(),
            used_names: order.iter().map(|f| f.name.to_string()).collect(),
        }
    }

    /// The composite the machine's own frame runs under — where a seed
    /// composes from.
    fn identity(&self) -> Composite {
        identity_composite(self.machine_sig.arity as usize, 0)
    }

    /// The composite a BOUND site's callee runs under: `compose(enclosing,
    /// binding)`, where `enclosing` is the composite the CALLER runs under
    /// — the machine identity for a seed, the enclosing copy's own
    /// composite for a site met inside one. The caller's declared
    /// cardinalities carry the closed-on-unequal binding rule
    /// (docs/formats.md (bound calls)). The callee's signature comes back
    /// alongside because every caller needs both and neither driver should
    /// look it up a second way.
    fn bound_child(
        &self,
        enclosing: &Composite,
        caller: usize,
        callee: usize,
        record: &BoundCall,
    ) -> Result<(Composite, &'a RoutineSig), LinkError> {
        let callee_sig = routine_sig(self.order, callee)?;
        let caller_cards = self.order[caller]
            .signature
            .map(|s| s.cardinalities.as_slice())
            .unwrap_or(self.machine_sig.cardinalities.as_slice());
        let child = compose(enclosing, caller_cards, callee, &record.binding, callee_sig)
            .map_err(|e| bad_binding(&self.order[callee].name, &e))?;
        Ok((child, callee_sig))
    }

    /// The composite a PLAIN call's callee runs under: the caller's own,
    /// re-pointed at the callee — a plain call changes no frame.
    fn plain_child(&self, enclosing: &Composite, callee: usize) -> Composite {
        let mut child = enclosing.clone();
        child.routine = callee;
        child
    }

    /// Whether a bound site needs no copy at all: a binding that composes
    /// back to a genuine full pass-through — identity placement and maps
    /// AND the callee alphabet as wide as the machine's on every tape —
    /// reaches the original routine. A narrower or wider callee keeps a
    /// cardinality hole, so it is stamped instead (its trap rows are
    /// synthesized from the alphabet gap in `build_stamp`). An
    /// EXIT-BEARING site never collapses either, whatever its binding: a
    /// plain call returns through the pushed return address and has
    /// nowhere to put the other exits (docs/core.md (call mechanisms)) —
    /// it is a SPLICE, and a splice is never a plain call into the
    /// generic.
    fn collapses(&self, child: &Composite, callee_sig: &RoutineSig, record: &BoundCall) -> bool {
        record.exits.is_empty() && is_full_passthrough(child, self.machine_sig, callee_sig)
    }

    /// The pending splice site of a bound call met INSIDE the copy at
    /// `slot`: its offsets are the ORIGINAL routine's, translated through
    /// that copy's own offset map when it is built (docs/core.md (call
    /// mechanisms)). `None` for an exit-free site, which splices nothing.
    fn nested_site(&self, slot: usize, addr: u32, record: &BoundCall) -> Option<PendingSite> {
        (!record.exits.is_empty()).then(|| PendingSite::InStamp {
            slot,
            then: addr + 5,
            exits: record.exits.clone(),
        })
    }

    /// The next node to walk, in FIFO order.
    fn pop(&mut self) -> Option<usize> {
        self.worklist.pop_front()
    }

    /// A slot's index in the final order, where the stamps follow the
    /// hand-written routines.
    fn stamp_index(&self, slot: usize) -> usize {
        self.order.len() + slot
    }

    /// Intern a (routine, composite) into the node table, deduped by
    /// canonical key. Returns its ORDER index and whether it resolved to
    /// an ALREADY-interned node (a stamp the dedup avoids building).
    ///
    /// An EXIT-BEARING node widens the KEY — never the digest — with its
    /// call site's `(caller, then, exits)`: two sites into the same
    /// routine under the same composite are different splices, returning
    /// to different places, and must not share a copy. The caller index is
    /// in the key because `then` and the exit offsets are
    /// caller-blob-relative and mean nothing without it. Widening the
    /// DIGEST instead would rename every existing stamp and move every
    /// existing mono image, so an exit-free node's key, and therefore its
    /// `<routine>.<digest8>` name, is unchanged.
    ///
    /// This is the ONE identity policy for the closure: the probe counts
    /// nodes by exactly the key the builder builds them by, so a pair
    /// reached through two distinct splices is walked twice because it IS
    /// built twice. A consequence worth naming: a cycle of exit-bearing
    /// bound calls mints a fresh node per turn, here exactly as in the
    /// builder — such a program has no mono lowering, and neither walk
    /// terminates on one.
    ///
    /// The map-visible name is `<routine>.<digest8>` — a period, not the
    /// `$` an earlier scheme used, because `.tma` identifiers cannot
    /// contain `$` at all (docs/formats.md (assembly text)) and a
    /// disassembled stamp must re-lex. A period IS legal in a hand-written
    /// routine name, so unlike the `$` scheme this one cannot rule out a
    /// collision by character choice alone; `used_names` is what makes
    /// collision-freedom a checked guarantee instead of an assumption — a
    /// freshly minted name is rejected with a typed [`LinkError`] if it
    /// already names another routine or an earlier stamp (astronomically
    /// unlikely: it needs either a hand-written name that happens to match
    /// `<routine>.<digest8>` exactly, or two distinct composites whose
    /// 32-bit digests collide — docs/core.md (the composition engine)).
    fn intern(
        &mut self,
        routine: usize,
        mut composite: Composite,
        site: Option<PendingSite>,
    ) -> Result<(usize, bool), LinkError> {
        composite.routine = routine;
        let mut key = canonical_key(&composite);
        if let Some(s) = &site {
            let (caller, then, exits) = s.key_parts(self.order.len());
            key.extend_from_slice(&(caller as u64).to_le_bytes());
            key.extend_from_slice(&then.to_le_bytes());
            for e in exits {
                key.extend_from_slice(&e.to_le_bytes());
            }
        }
        if let Some(&slot) = self.key_to_slot.get(&key) {
            return Ok((self.stamp_index(slot), true));
        }
        let slot = self.nodes.len();
        let mut name = format!("{}.{:08x}", self.order[routine].name, digest(&composite));
        if site.is_some() {
            // Two exit-bearing splices of one (routine, composite) share a
            // digest by construction — the exits are not in it. Number them
            // rather than refuse (docs/core.md (the composition engine)).
            let base = name.clone();
            let mut n = 1u32;
            while self.used_names.contains(&name) {
                name = format!("{base}.{n}");
                n += 1;
            }
        }
        if !self.used_names.insert(name.clone()) {
            return Err(LinkError::StampNameCollision(name));
        }
        self.nodes.push(StampNode {
            routine,
            composite,
            name,
            site,
        });
        self.key_to_slot.insert(key, slot);
        self.worklist.push_back(slot);
        Ok((self.stamp_index(slot), false))
    }
}

/// Enumerate the stamp closure from the identity-world mono seeds
/// WITHOUT building any body, reporting every exit-bearing bijection
/// site met inside a copy with its binding already composed against the
/// enclosing composite (docs/core.md (call mechanisms)).
///
/// Hybrid needs this before it can size a fold group: a group's members
/// are not all visible at the machine's own frame, and a stamp that
/// reaches a SHARED callee emits a framed call rather than recursing, so
/// the closure's own shape depends on the decision. Probing first, then
/// deciding, then building is what breaks that circle.
///
/// It drives the SAME [`Closure`] the builder does — one composition of a
/// child from a site, one intern key — so a pair the builder builds twice
/// is walked twice here, and the sites inside both copies are counted.
/// What is left is its own failure policy, its seeds, and one deliberate
/// one-way drift:
///
/// - **It never FAILS.** A site it cannot compose, a name it cannot mint,
///   or a raw framed call inside a copy is skipped rather than reported as
///   an error, so the probe can never turn a linkable program into a
///   refusal. A refusal belongs in `mono_stamps`, which walks the
///   authoritative closure; one raised here would be raised over a walk
///   that is deliberately neither a subset nor a superset of what gets
///   built, for the two reasons below.
/// - **It UNDER-reports from its seeds.** The seeds are the EXIT-FREE
///   bijection sites alone, because those are the copies that exist
///   whatever the decision is — an exit-bearing site nested inside a copy
///   that only exists because another exit-bearing site was REFUSED
///   sharing is invisible here, and cannot be otherwise: whether that copy
///   exists is the very thing being decided. Sound: an under-counted group
///   is likelier to fall under the byte rule and splice, and splicing is
///   available to every exit-bearing site.
/// - **It OVER-reports one way.** After an exit-bearing site it keeps
///   descending, exactly as the builder does when that site splices — but
///   if the site's group ends up SHARED the builder frames the call and
///   never copies the subtree, so sites the probe met down there belong to
///   no copy at all. Sound too: a phantom member can only push its group
///   over the byte rule into sharing, sharing only sets `any_frames` and a
///   `shared` pair the builder never reaches, and a frames image is a
///   correct image for any site. The visible cost is a `FoldDecision` for
///   a group whose members are not all in the emitted image.
fn mono_closure_probe<'a>(
    closure: &mut Closure<'a, '_>,
    sites: &[Vec<SiteKind<'a>>],
    seeds: &[(usize, u32, usize, &'a BoundCall)],
) -> Vec<ClosureSite<'a>> {
    // The caller composed its own group keys with this same walk and
    // interned nothing, so the node table starts empty here.
    debug_assert!(closure.nodes.is_empty(), "the probe walks a fresh closure");
    let order = closure.order;
    let machine_sig = closure.machine_sig;
    let id = closure.identity();
    let mut met: Vec<ClosureSite<'a>> = Vec::new();

    for &(fi, _, callee, record) in seeds {
        let Ok((child, _)) = closure.bound_child(&id, fi, callee, record) else {
            continue;
        };
        let _ = closure.intern(callee, child, None);
    }

    while let Some(slot) = closure.pop() {
        let routine = closure.nodes[slot].routine;
        let comp = closure.nodes[slot].composite.clone();
        for site in &sites[routine] {
            match site {
                // A raw framed call inside a copy is the mono refusal
                // `mono_stamps` raises; leave it to raise it, so the
                // probe never changes which error a link reports.
                SiteKind::RawCallM { .. } => {}
                SiteKind::Plain { callee, .. } => {
                    let child = closure.plain_child(&comp, *callee);
                    let _ = closure.intern(*callee, child, None);
                }
                SiteKind::Bound {
                    addr,
                    callee,
                    record,
                    ..
                } => {
                    let Ok((child, callee_sig)) =
                        closure.bound_child(&comp, routine, *callee, record)
                    else {
                        continue;
                    };
                    // An exit-bearing bijection site is a fold-group
                    // candidate wherever it sits. Everything else keeps
                    // descending exactly as the builder will.
                    if !record.exits.is_empty()
                        && is_bijection(
                            order[routine].signature.unwrap_or(machine_sig),
                            callee_sig,
                            record,
                        )
                    {
                        met.push(ClosureSite {
                            callee: *callee,
                            record,
                            composite: child.clone(),
                        });
                    }
                    if closure.collapses(&child, callee_sig, record) {
                        continue;
                    }
                    let site = closure.nested_site(slot, *addr, record);
                    let _ = closure.intern(*callee, child, site);
                }
            }
        }
    }
    met
}

/// The bytes one site's frames descriptor costs — EXACT, not an estimate.
/// The composite is already known at decision time (it is what the group
/// key was computed from), so the size is taken from the bytes themselves
/// rather than re-derived by a second formula that could disagree with the
/// emitter (docs/core.md (call mechanisms)).
///
/// The length is determined by the composite, the machine signature, the
/// callee's own signature and the exit COUNT — never by the exit VALUES,
/// which are blob offsets layout rebases to absolute addresses afterwards
/// (docs/formats.md (frame descriptors)). That is why a pre-layout
/// decision survives the rebase and why a relink is byte-identical.
fn descriptor_cost(
    composite: &Composite,
    machine_sig: &RoutineSig,
    order: &[FuncRef],
    exits: &[u32],
) -> Result<u32, LinkError> {
    Ok(
        u32::try_from(super::engine::materialize(composite, machine_sig, order, exits)?.len())
            .expect("a descriptor size fits u32"),
    )
}

/// A completed bijection (mono-eligible): every bound tape equal-size (so
/// identity completion is total), with no one-way `=>` pair (which is
/// excluded from write-back) and not OPEN. Injectivity on equal-size
/// bindings is already enforced by the engine's `validate_binding`, and
/// total + injective on equal finite alphabets is surjective — so this is
/// the totality check the classifier owns, completing the bijection
/// determination.
///
/// An open tape fails it on both halves at once, whatever the
/// cardinalities: its unlisted symbols read onto ONE opaque index (not
/// injective) and write back through nothing (not total), so it is exactly
/// the holey/one-way shape the classifier promises to leave on the frames
/// path (docs/formats.md (bound calls)).
fn is_bijection(caller_sig: &RoutineSig, callee_sig: &RoutineSig, record: &BoundCall) -> bool {
    if record.binding.len() != callee_sig.arity as usize {
        return false;
    }
    for (k, tb) in record.binding.iter().enumerate() {
        let caller_card = caller_sig
            .cardinalities
            .get(usize::from(tb.caller_tape))
            .copied()
            .unwrap_or(0);
        let callee_card = callee_sig.cardinalities.get(k).copied().unwrap_or(0);
        if caller_card != callee_card {
            return false;
        }
        if tb.open || tb.pairs.iter().any(|p| p.one_way) {
            return false;
        }
    }
    true
}

// -- the stamp closure -------------------------------------------------------

/// Functions reachable from the entry through the machine's own frame:
/// plain calls and full-pass-through (collapse) bound calls preserve it.
/// Only these routines are stamped at the machine identity; a routine
/// reached solely through a projecting bound call runs under a composite,
/// and its stamp is minted where that call is closed over.
fn identity_world(sites: &[Vec<SiteKind>], n: usize) -> Vec<bool> {
    let mut in_world = vec![false; n];
    let mut queue = VecDeque::new();
    if n > 0 {
        in_world[0] = true;
        queue.push_back(0usize);
    }
    while let Some(fi) = queue.pop_front() {
        for site in &sites[fi] {
            let next = match site {
                SiteKind::Plain { callee, .. } => Some(*callee),
                SiteKind::Bound {
                    callee,
                    collapse: true,
                    ..
                } => Some(*callee),
                _ => None,
            };
            if let Some(c) = next
                && !in_world[c]
            {
                in_world[c] = true;
                queue.push_back(c);
            }
        }
    }
    in_world
}

/// Re-check reachability after stamping retargets every bound-call site to
/// its specialized copy: a generic routine that lost its last caller this
/// way is unreachable, and the linker's promise that unreachable functions
/// never ship applies to it exactly as it does before lowering (docs/core.md
/// (linking)). Walks the same BFS as name resolution, now over the
/// (already-retargeted) `calls` and any still-pending `bound` edges, and
/// reindexes the survivors' targets so the result is layout-ready. When
/// nothing is orphaned the input `Vec` comes back untouched — not merely
/// equivalent — so a link with no orphan hands layout the identical order it
/// always did. The second return is the pruned names, sorted, for folding
/// into the link report's `dropped` list alongside `resolve`'s pre-lowering
/// ones (docs/core.md (the link report)) — the two are mutually exclusive by
/// construction, since a name `resolve` already dropped never entered
/// `order` and so cannot be found here.
fn prune_unreachable(order: Vec<FuncRef>) -> (Vec<FuncRef>, Vec<String>) {
    let n = order.len();
    if n == 0 {
        return (order, Vec::new());
    }
    let mut reached = vec![false; n];
    reached[0] = true;
    let mut queue: VecDeque<usize> = VecDeque::from([0usize]);
    while let Some(fi) = queue.pop_front() {
        let edges = order[fi]
            .calls
            .iter()
            .map(|&(_, c)| c)
            .chain(order[fi].bound.iter().map(|&(_, c, _)| c));
        for c in edges {
            if !reached[c] {
                reached[c] = true;
                queue.push_back(c);
            }
        }
    }
    if reached.iter().all(|&r| r) {
        return (order, Vec::new());
    }

    let mut orphaned: Vec<String> = order
        .iter()
        .zip(&reached)
        .filter(|&(_, &r)| !r)
        .map(|(f, _)| f.name.to_string())
        .collect();
    orphaned.sort();

    let mut new_index = vec![usize::MAX; n];
    let mut next = 0usize;
    for (i, &r) in reached.iter().enumerate() {
        if r {
            new_index[i] = next;
            next += 1;
        }
    }
    let pruned = order
        .into_iter()
        .zip(reached)
        .filter(|(_, r)| *r)
        .map(|(mut f, _)| {
            for (_, c) in &mut f.calls {
                *c = new_index[*c];
            }
            for (_, c, _) in &mut f.bound {
                *c = new_index[*c];
            }
            // A splice's `ret`/`retx` jumps name a position inside another
            // function by index, exactly as a call edge does, so they
            // reindex alongside (docs/core.md (call mechanisms)). Mono
            // stamping orphans generics as a matter of course — that is
            // what this prune is for — so an un-remapped fixup would point
            // at whichever function slid into the dropped index.
            for (_, target, _) in &mut f.site_fixups {
                debug_assert_ne!(
                    new_index[*target],
                    usize::MAX,
                    "a splice's caller is reachable at the machine frame, so the prune keeps it"
                );
                *target = new_index[*target];
            }
            f
        })
        .collect();
    (pruned, orphaned)
}

/// The dialect's far unconditional jump, as the instruction an
/// exit-bearing site is ENTERED by (docs/core.md (call mechanisms)): the
/// copy never returns through a pushed address, so the site is a jump and
/// not a call.
fn enter_jump_opcode(syntax: &ArchSyntax, name: &str) -> Result<u8, LinkError> {
    syntax.jump_opcode().ok_or_else(|| LinkError::BadBinding {
        callee: name.to_string(),
        message: "the dialect has no unconditional far jump to enter an exit-bearing copy with"
            .to_string(),
    })
}

/// What an exit-bearing site needs before a mono path commits to splicing
/// it (docs/core.md (call mechanisms)): the dialect's far jump to enter
/// the copy with, and an instruction AFTER the call for the copy's plain
/// `ret` to land on. A site in tail position has no such instruction —
/// `then` would be the offset one past the end of the caller's blob — and
/// is named here rather than left to surface downstream as a malformed
/// blob at an offset nothing in the source points at.
///
/// Called at the three points a mono path first commits to copying a
/// site: the seed loop, hybrid's classifier, and the stamp closure's own
/// bound arm. In the closure the caller is the GENERIC routine, but the
/// copy re-emits one instruction per original instruction, so tail
/// position in the generic is tail position in the copy.
fn check_splice_site(
    syntax: &ArchSyntax,
    caller: &FuncRef,
    addr: u32,
    callee_name: &str,
) -> Result<(), LinkError> {
    enter_jump_opcode(syntax, callee_name)?;
    if addr as usize + 5 >= caller.blob.len() {
        return Err(LinkError::ExitBearingTailCall(caller.name.to_string()));
    }
    Ok(())
}

/// The same opcode, as the instruction a splice LEAVES through — its
/// `ret → jmp <then>` and `retx #k → jmp <exit_k>` rewrites (docs/core.md
/// (call mechanisms)).
fn splice_jump_opcode(syntax: &ArchSyntax, name: &str) -> Result<u8, LinkError> {
    syntax.jump_opcode().ok_or_else(|| LinkError::BadBinding {
        callee: name.to_string(),
        message: "the dialect has no unconditional far jump to splice an exit-bearing call site \
                  into"
            .to_string(),
    })
}

/// The offsets the frames path shifts inside one function: the addresses
/// of the bound sites that are still framed once the mono/frames split is
/// decided, each widening 5 → 9 bytes. Empty for every function under pure
/// mono, which rewrites no blob at all.
fn splice_shift(widened: &HashSet<u32>, old: u32) -> u32 {
    old + 4 * u32::try_from(widened.iter().filter(|&&a| a < old).count())
        .expect("widened-site count fits u32")
}

/// The [`SpliceSite`] for one exit-bearing site in a hand-written routine,
/// with its caller offsets shifted into the post-rewrite blob layout.
/// `None` for an exit-free site, which splices nothing.
///
/// Under mono the site keeps its 5-byte shape (a `jmp` where the `call`
/// was) and nothing else in the blob moves, so the shift is the identity.
/// Under HYBRID it is not: `lower_frames` runs afterwards and widens every
/// bound site that is still framed, shifting exactly the caller offsets
/// these fixups name (docs/core.md (call mechanisms)).
fn site_for(
    caller: usize,
    addr: u32,
    record: &BoundCall,
    widened: &[HashSet<u32>],
) -> Option<SpliceSite> {
    if record.exits.is_empty() {
        return None;
    }
    let w = &widened[caller];
    Some(SpliceSite {
        caller,
        then: splice_shift(w, addr + 5),
        exits: record.exits.iter().map(|&e| splice_shift(w, e)).collect(),
    })
}

/// Build the mono stamp set reachable from `seeds` (machine-frame bound
/// sites to specialize), closing over each stamp's own calls (mono all the
/// way down). Returns the stamp `FuncRef`s (order indices `order.len()..`)
/// and, per seed, its stamp order index.
///
/// `widened` is parallel to `order`: the bound-site addresses the frames
/// path will widen in each function once the mono/frames split is decided
/// (all empty under pure mono, which rewrites no blob).
///
/// `shared` names the `(callee, composite)` pairs hybrid's byte rule folded
/// onto ONE generic body (docs/core.md (call mechanisms)). A site inside a
/// copy that reaches such a pair is emitted as a framed call through a
/// descriptor in the copy's own table blob, rather than recursing into a
/// child stamp — which is what keeps the shared body shared. Pure mono
/// shares nothing and passes an empty set.
#[allow(clippy::type_complexity, clippy::too_many_arguments)]
fn mono_stamps<'a>(
    syntax: &ArchSyntax,
    order: &[FuncRef<'a>],
    sites: &[Vec<SiteKind<'a>>],
    machine_sig: &RoutineSig,
    seeds: &[(usize, u32, usize, &'a BoundCall)],
    widened: &[HashSet<u32>],
    shared: &HashSet<GroupKey>,
) -> Result<(Vec<FuncRef<'a>>, HashMap<(usize, u32), usize>, StampStats), LinkError> {
    let mut closure = Closure::new(order, machine_sig);
    let id = closure.identity();
    let mut seed_target: HashMap<(usize, u32), usize> = HashMap::new();
    let mut stats = StampStats::default();

    // Seed: compose each site's binding at the machine identity, the
    // composite the seed's caller (routine `fi`) runs under.
    for &(fi, addr, callee, record) in seeds {
        let (child, _) = closure.bound_child(&id, fi, callee, record)?;
        let (idx, dup) = closure.intern(
            callee,
            child,
            site_for(fi, addr, record, widened).map(PendingSite::Resolved),
        )?;
        if dup {
            stats.dedup_savings += 1;
        }
        seed_target.insert((fi, addr), idx);
    }

    // Close over each stamp's calls. A plain call inherits the stamp's
    // composite (the callee runs under the same frame); a bound call composes
    // its binding onto it; both stay mono. A raw `call.m` is a frames
    // instruction — refused.
    // Per stamp slot: the copy-blob address of each call site and what the
    // copy does there (docs/core.md (call mechanisms)).
    let mut stamp_targets: Vec<HashMap<u32, StampTarget>> = Vec::new();
    while let Some(slot) = closure.pop() {
        let routine = closure.nodes[slot].routine;
        let comp = closure.nodes[slot].composite.clone();
        let mut targets: HashMap<u32, StampTarget> = HashMap::new();
        for site in &sites[routine] {
            match site {
                SiteKind::RawCallM { .. } => {
                    return Err(LinkError::MonoRawFrame(order[routine].name.to_string()));
                }
                SiteKind::Plain { addr, callee } => {
                    let child = closure.plain_child(&comp, *callee);
                    let (idx, dup) = closure.intern(*callee, child, None)?;
                    if dup {
                        stats.dedup_savings += 1;
                    }
                    targets.insert(
                        *addr,
                        StampTarget::Plain {
                            target: idx,
                            splice: false,
                        },
                    );
                }
                SiteKind::Bound {
                    addr,
                    callee,
                    record,
                    ..
                } => {
                    // A group the byte rule SHARED is reached through one
                    // generic body: the copy frames the call instead of
                    // minting a child, through a descriptor
                    // `compose(C, binding)` it carries in its own table
                    // blob (docs/core.md (call mechanisms)). Decided before
                    // anything else, because such a site needs neither a
                    // jump opcode nor a `then` — it returns through the
                    // frame, exactly as the identity-world members of the
                    // same group do.
                    // The caller is this stamp's own routine, running under
                    // this stamp's own composite.
                    let (child, callee_sig) =
                        closure.bound_child(&comp, routine, *callee, record)?;
                    if shared.contains(&(*callee, canonical_key(&child))) {
                        targets.insert(
                            *addr,
                            StampTarget::Framed {
                                callee: *callee,
                                composite: child,
                                exits: record.exits.clone(),
                            },
                        );
                        continue;
                    }
                    // A bound site NESTED inside a routine being copied
                    // splices exactly as a top-level one does, except that
                    // the caller is this stamp: its offsets are the
                    // ORIGINAL routine's, translated through the copy's own
                    // offset map when it is built (docs/core.md (call
                    // mechanisms)).
                    if !record.exits.is_empty() {
                        check_splice_site(syntax, &order[routine], *addr, &order[*callee].name)?;
                    }
                    let site = closure.nested_site(slot, *addr, record);
                    let splice = site.is_some();
                    // A site that collapses onto the generic mints no child
                    // and is not walked into: the generic itself already is
                    // that copy.
                    let idx = if closure.collapses(&child, callee_sig, record) {
                        *callee
                    } else {
                        let (idx, dup) = closure.intern(*callee, child, site)?;
                        if dup {
                            stats.dedup_savings += 1;
                        }
                        idx
                    };
                    targets.insert(
                        *addr,
                        StampTarget::Plain {
                            target: idx,
                            splice,
                        },
                    );
                }
            }
        }
        if stamp_targets.len() <= slot {
            stamp_targets.resize_with(slot + 1, HashMap::new);
        }
        stamp_targets[slot] = targets;
    }
    let nodes = closure.nodes;
    stamp_targets.resize_with(nodes.len(), HashMap::new);

    // Materialize each stamp's body, in slot order. A node nested inside
    // another stamp always interns AFTER the stamp it sits in — its key
    // names that stamp as its caller, so it can never dedup onto an
    // earlier slot, and the worklist is FIFO — so by the time such a node
    // is built, the caller copy's offset map is already in `stamp_offsets`
    // (docs/core.md (call mechanisms)).
    let mut stamp_funcs = Vec::with_capacity(nodes.len());
    let mut stamp_offsets: Vec<HashMap<u32, u32>> = Vec::with_capacity(nodes.len());
    for (slot, node) in nodes.iter().enumerate() {
        let callee = &order[node.routine];
        let callee_sig = routine_sig(order, node.routine)?;
        let site = resolve_site(node, &nodes, &stamp_offsets, order.len())?;
        let body = build_stamp(
            syntax,
            callee,
            &node.composite,
            machine_sig,
            callee_sig,
            order,
            &stamp_targets[slot],
            site.as_ref(),
        )?;
        stats.synthesized_trap_rows += body.trap_rows;
        stats.expanded_rows += body.expanded_rows;
        stamp_offsets.push(body.offsets);
        stamp_funcs.push(FuncRef {
            name: Cow::Owned(node.name.clone()),
            blob: Cow::Owned(body.blob),
            debug: None,
            calls: body.calls,
            bound: Vec::new(),
            table: Cow::Owned(body.table),
            table_fixups: body.table_fixups,
            site_fixups: body.site_fixups,
            signature: None,
            // The composite permutes tape order and glyph indices, so the
            // callee's interface record describes another coordinate
            // system and must not be attached to the copy; stamps carry
            // no bound sites, so nothing reads it.
            interface: None,
            origin: callee.origin,
        });
    }

    Ok((stamp_funcs, seed_target, stats))
}

/// One node's splice site in final coordinates, ready for `build_stamp`.
/// A `Resolved` site passes straight through; an `InStamp` one names
/// offsets in the ORIGINAL caller routine, which the caller COPY's offset
/// map translates into the copy's own blob — and the copy is what the
/// splice must return into, since the generic is orphaned the moment its
/// last site is retargeted (docs/core.md (call mechanisms)).
fn resolve_site(
    node: &StampNode,
    nodes: &[StampNode],
    stamp_offsets: &[HashMap<u32, u32>],
    order_len: usize,
) -> Result<Option<SpliceSite>, LinkError> {
    match &node.site {
        None => Ok(None),
        Some(PendingSite::Resolved(s)) => Ok(Some(s.clone())),
        Some(PendingSite::InStamp { slot, then, exits }) => {
            // `get` rather than an index: it doubles as the check that the
            // caller copy really was built first, reporting a typed error
            // instead of panicking if that ordering ever breaks.
            let map = stamp_offsets
                .get(*slot)
                .ok_or_else(|| LinkError::MalformedBlob {
                    symbol: node.name.clone(),
                    at: *then,
                })?;
            let xlat = |off: u32| -> Result<u32, LinkError> {
                map.get(&off).copied().ok_or(LinkError::MalformedBlob {
                    symbol: nodes[*slot].name.clone(),
                    at: off,
                })
            };
            Ok(Some(SpliceSite {
                caller: order_len + slot,
                then: xlat(*then)?,
                exits: exits.iter().map(|&e| xlat(e)).collect::<Result<_, _>>()?,
            }))
        }
    }
}

// -- one stamp body ----------------------------------------------------------

/// One callee virtual tape's read projection: its physical tape and
/// cardinality, the ascending physical preimage of each virtual symbol, and
/// the physical symbols that read as no valid virtual symbol (holes).
struct TapeProj {
    phys: usize,
    phys_card: u32,
    preimage: HashMap<u16, Vec<u8>>,
    holes: Vec<u8>,
}

/// The virtual symbol physical symbol `p` reads as, or `None` when it maps to
/// no symbol inside the callee alphabet (a read hole).
fn read_image(t: &CompositeTape, p: u16, callee_card: u32) -> Option<u16> {
    match t.rmap.apply(p) {
        // `callee_card` is the opaque index (docs/formats.md (bound
        // calls)); it is an image, so the stamp synthesizes no trap row
        // for it and its preimage expands the callee's `*` row.
        Some(v) if u32::from(v) <= callee_card => Some(v),
        _ => None,
    }
}

/// The physical symbol virtual symbol `v` writes as, or `None` when it maps
/// outside the physical alphabet (a write hole).
fn write_image(t: &CompositeTape, v: u16, phys_card: u32) -> Option<u16> {
    match t.wmap.apply(v) {
        Some(p) if u32::from(p) < phys_card => Some(p),
        _ => None,
    }
}

/// Emit `jmp <placeholder>` and record the cross-function fixup layout
/// will patch (docs/core.md (call mechanisms)). The placeholder
/// displacement is `-5` — a jump to ITSELF, always an instruction boundary
/// of this blob — so layout's own decode of the copy resolves it cleanly
/// before the patch lands. A displacement of 0 would name the byte after
/// the jump, which is past the end of the blob whenever the rewritten
/// return is the body's last instruction.
fn emit_splice_jump(
    blob: &mut Vec<u8>,
    jmp: u8,
    caller: usize,
    target: u32,
    fixups: &mut Vec<(u32, usize, u32)>,
) {
    blob.push(jmp);
    let hole = blob.len() as u32;
    blob.extend_from_slice(&(-5i32).to_le_bytes());
    fixups.push((hole, caller, target));
}

/// Re-emit the callee's generic body at the machine width, projecting every
/// tape op and match/dispatch table through the composite (docs/core.md (the
/// composition engine)).
///
/// With a `site`, the copy is an exit-bearing SPLICE: it is entered by a
/// jump rather than a call, so its `ret` becomes a jump to the site's
/// continuation and its `retx #k` a jump to exit `k` — both recorded as
/// cross-function fixups for layout (docs/core.md (call mechanisms)).
///
/// `order` is the whole pre-stamp function order: `materialize` reads the
/// callee's own signature out of it by `composite.routine` when this copy
/// carries a framed call into a SHARED body.
#[allow(clippy::too_many_arguments)]
fn build_stamp(
    syntax: &ArchSyntax,
    callee: &FuncRef,
    comp: &Composite,
    machine_sig: &RoutineSig,
    callee_sig: &RoutineSig,
    order: &[FuncRef],
    targets: &HashMap<u32, StampTarget>,
    site: Option<&SpliceSite>,
) -> Result<StampBody, LinkError> {
    let ma = machine_sig.arity as usize;

    // Per-tape read projections (also the source of the trap rows).
    let mut projs: Vec<TapeProj> = Vec::with_capacity(comp.tapes.len());
    for (k, t) in comp.tapes.iter().enumerate() {
        let phys_card = *machine_sig
            .cardinalities
            .get(usize::from(t.phys))
            .ok_or_else(|| LinkError::BadFrameDescriptor {
                symbol: callee.name.to_string(),
                message: format!(
                    "stamped tape {k} projects onto physical tape {} at or past the machine arity {ma}",
                    t.phys
                ),
            })?;
        let callee_card = callee_sig.cardinalities.get(k).copied().unwrap_or(0);
        let mut preimage: HashMap<u16, Vec<u8>> = HashMap::new();
        let mut holes: Vec<u8> = Vec::new();
        for p in 0..phys_card {
            // Match cells and vector payloads are 7-bit (0x7F = wildcard/keep).
            let Ok(p8) = u8::try_from(p) else { break };
            if p8 > 0x7E {
                break;
            }
            match read_image(t, u16::from(p8), callee_card) {
                Some(v) => preimage.entry(v).or_default().push(p8),
                None => holes.push(p8),
            }
        }
        projs.push(TapeProj {
            phys: usize::from(t.phys),
            phys_card,
            preimage,
            holes,
        });
    }

    let mut blob: Vec<u8> = Vec::new();
    let mut table: Vec<u8> = Vec::new();
    let mut table_fixups: Vec<(u32, u32)> = Vec::new();
    let mut calls: Vec<(u32, usize)> = Vec::new();
    let mut site_fixups: Vec<(u32, usize, u32)> = Vec::new();
    // Per framed call into a shared body: where its exit slot sits in this
    // copy's table blob, and the ENCLOSING routine's blob offset it names.
    let mut frame_exit_fixups: Vec<(usize, u32)> = Vec::new();
    let mut old_to_new: HashMap<u32, u32> = HashMap::new();
    let mut jump_fixups: Vec<(u32, u32, u8)> = Vec::new();
    let mut dispatch_fixups: Vec<(usize, DispEntry)> = Vec::new();
    let mut pending_remap: Option<Vec<EntrySrc>> = None;
    let mut needs_trap_stub = false;
    let mut trap_rows = 0u32;
    let mut expanded_rows = 0u32;

    let blob_bytes: &[u8] = &callee.blob;
    for d in decode::decode_stream(syntax, blob_bytes, 0, blob_bytes.len() as u32) {
        let old_addr = d.addr;
        old_to_new.insert(old_addr, blob.len() as u32);
        let Body::Instr { mnemonic, operand } = &d.body else {
            // A byte no instruction covers — copied verbatim (defensive; a
            // valid callee decodes cleanly).
            if let Body::Raw(b) = &d.body {
                blob.push(*b);
            }
            continue;
        };
        let entry = syntax
            .by_mnemonic(mnemonic)
            .expect("mnemonic came from a successful decode");

        // A call or tail jump to a child stamp / original: a plain far
        // call — except an exit-bearing site, which is ENTERED by a jump
        // (docs/core.md (call mechanisms)). Either way the instruction
        // keeps its 5-byte opcode + RelI32 shape and its relocation hole.
        match targets.get(&old_addr) {
            Some(&StampTarget::Plain { target, splice }) => {
                let opcode = if splice {
                    enter_jump_opcode(syntax, &callee.name)?
                } else {
                    entry.opcode
                };
                blob.push(opcode);
                let hole = blob.len() as u32;
                blob.extend_from_slice(&[0u8; 4]);
                calls.push((hole, target));
                continue;
            }
            Some(StampTarget::Framed {
                callee: target,
                composite,
                exits,
            }) => {
                // A framed call into the SHARED generic body: opcode,
                // displacement (relocated to the body like a far call),
                // then the frame half, which names a descriptor in THIS
                // stamp's own table blob — the hand-authored `.frame`
                // shape, which layout already rebases
                // (docs/formats.md (frame descriptors)).
                let fc = syntax
                    .framed_call_opcode()
                    .ok_or_else(|| LinkError::BadBinding {
                        callee: callee.name.to_string(),
                        message: "the dialect has no framed-call opcode to reach a \
                                  shared exit-bearing body"
                            .to_string(),
                    })?;
                blob.push(fc);
                let disp = blob.len() as u32;
                blob.extend_from_slice(&[0u8; 4]);
                calls.push((disp, *target));
                let frame_hole = blob.len() as u32;
                blob.extend_from_slice(&[0u8; 4]);
                // The descriptor, with PLACEHOLDER exits: they are the
                // enclosing routine's own blob offsets here, and are
                // remapped into this copy's blob once the body is emitted.
                // Only their COUNT affects the descriptor's length, so
                // materializing before the remap sizes it correctly
                // (docs/formats.md (frame descriptors)).
                let desc_off = table.len() as u32;
                let bytes = super::engine::materialize(composite, machine_sig, order, exits)?;
                let exits_at = table.len() + bytes.len() - 4 * exits.len();
                table.extend_from_slice(&bytes);
                for (i, &old) in exits.iter().enumerate() {
                    frame_exit_fixups.push((exits_at + 4 * i, old));
                }
                table_fixups.push((frame_hole, desc_off));
                continue;
            }
            None => {}
        }

        match entry.operand {
            OperandKind::None => {
                // Inside an exit-bearing splice the plain return becomes a
                // jump to the call site's continuation: the copy is
                // entered by `jmp`, so no return address was pushed
                // (docs/core.md (call mechanisms)). `stp`/`hlt` are left
                // alone — only the dialect's declared return is rewritten.
                if let Some(s) = site
                    && Some(entry.opcode) == syntax.return_opcode
                {
                    let jmp = splice_jump_opcode(syntax, &callee.name)?;
                    emit_splice_jump(&mut blob, jmp, s.caller, s.then, &mut site_fixups);
                    continue;
                }
                blob.extend_from_slice(&blob_bytes[old_addr as usize..(old_addr + d.len) as usize]);
            }
            OperandKind::Imm8 => {
                if entry.flow == Flow::Stop {
                    // A multi-exit return. Inside an exit-bearing splice
                    // it becomes a jump to exit `k` of the site's vector;
                    // anywhere else it is a frames instruction the base
                    // profile cannot run (docs/core.md (call mechanisms)).
                    let Some(s) = site else {
                        return Err(LinkError::MonoRawFrame(callee.name.to_string()));
                    };
                    let DecodedOperand::Imm(k) = operand else {
                        unreachable!("Imm8 decodes to Imm")
                    };
                    let Some(&target) = s.exits.get(usize::from(*k)) else {
                        return Err(LinkError::BadBinding {
                            callee: callee.name.to_string(),
                            message: format!(
                                "the body returns through exit {k}, but the call site \
                                 supplies {} exit(s)",
                                s.exits.len()
                            ),
                        });
                    };
                    let jmp = splice_jump_opcode(syntax, &callee.name)?;
                    emit_splice_jump(&mut blob, jmp, s.caller, target, &mut site_fixups);
                    continue;
                }
                // A hand-authored `trap #k` passes through.
                blob.extend_from_slice(&blob_bytes[old_addr as usize..(old_addr + d.len) as usize]);
            }
            OperandKind::SymbolVec => {
                let DecodedOperand::Ints(vec) = operand else {
                    unreachable!("SymbolVec decodes to Ints")
                };
                emit_write(
                    &mut blob,
                    syntax,
                    entry.opcode,
                    vec,
                    comp,
                    &projs,
                    ma,
                    &callee.name,
                )?;
            }
            OperandKind::MoveVec => {
                let DecodedOperand::Ints(vec) = operand else {
                    unreachable!("MoveVec decodes to Ints")
                };
                emit_move(&mut blob, entry.opcode, vec, &projs, ma);
            }
            OperandKind::WriteMoveVec => {
                // A fused write+move: project both halves like the separate
                // `wr`/`mov` would be (all writes precede all moves — one
                // formal step), then re-emit the two-group wire form.
                let DecodedOperand::WriteMove { writes, moves } = operand else {
                    unreachable!("WriteMoveVec decodes to WriteMove")
                };
                emit_write_move(
                    &mut blob,
                    syntax,
                    entry.opcode,
                    writes,
                    moves,
                    comp,
                    &projs,
                    ma,
                    &callee.name,
                )?;
            }
            OperandKind::TableRef => {
                let DecodedOperand::TableAddr(t_off) = operand else {
                    unreachable!("TableRef decodes to TableAddr")
                };
                if entry.flow == Flow::FallThrough {
                    // A match table: rewrite its rows and remember the row
                    // remapping for the dispatch that consumes its MR.
                    let (bytes, remap, tr, ex) =
                        rewrite_match_table(&callee.table, *t_off, comp, &projs, ma, &callee.name)?;
                    trap_rows += tr;
                    expanded_rows += ex;
                    if remap.iter().any(|e| matches!(e, EntrySrc::TrapStub)) {
                        needs_trap_stub = true;
                    }
                    let new_off = table.len() as u32;
                    table.extend_from_slice(&bytes);
                    blob.push(entry.opcode);
                    let hole = blob.len() as u32;
                    blob.extend_from_slice(&[0u8; 4]);
                    table_fixups.push((hole, new_off));
                    // A prior trap-bearing remap that no dispatch consumed
                    // before this match table replaces it would leave its
                    // trap rows unrouted — the same misroute the end-of-body
                    // guard catches, just mid-body.
                    if remap_has_trap(&pending_remap) {
                        return Err(LinkError::MonoHoleyMatchBranch(callee.name.to_string()));
                    }
                    pending_remap = Some(remap);
                } else {
                    // A dispatch table: rebuild its entries in the rewritten
                    // row order (trap rows dispatch to the stub; dropped rows
                    // vanish; expanded rows duplicate their target).
                    let remap = pending_remap.take().ok_or(LinkError::MalformedTable {
                        symbol: callee.name.to_string(),
                        at: *t_off,
                    })?;
                    let old_entries = read_dispatch(&callee.table, *t_off, &callee.name)?;
                    let new_off = table.len() as u32;
                    table.extend_from_slice(&(remap.len() as u16).to_le_bytes());
                    for src in &remap {
                        let pos = table.len();
                        table.extend_from_slice(&[0u8; 4]);
                        let de = match src {
                            EntrySrc::TrapStub => DispEntry::TrapStub,
                            EntrySrc::OldRow(r) => {
                                DispEntry::OldCode(*old_entries.get(usize::from(*r)).ok_or(
                                    LinkError::MalformedTable {
                                        symbol: callee.name.to_string(),
                                        at: *t_off,
                                    },
                                )?)
                            }
                        };
                        dispatch_fixups.push((pos, de));
                    }
                    blob.push(entry.opcode);
                    let hole = blob.len() as u32;
                    blob.extend_from_slice(&[0u8; 4]);
                    table_fixups.push((hole, new_off));
                }
            }
            OperandKind::RelI8 | OperandKind::RelI32 => {
                // An intra-function jump (calls/tails were handled above):
                // re-encode its displacement through the offset map.
                let DecodedOperand::RelTarget(target) = operand else {
                    unreachable!("RelI8/RelI32 decode to RelTarget")
                };
                let width = (d.len - 1) as u8;
                blob.push(entry.opcode);
                let op_pos = blob.len() as u32;
                blob.extend(std::iter::repeat_n(0u8, usize::from(width)));
                jump_fixups.push((op_pos, *target, width));
            }
            OperandKind::FramedCall => {
                return Err(LinkError::MonoRawFrame(callee.name.to_string()));
            }
        }
    }

    // A holey binding synthesizes unmapped-read trap rows into a match table,
    // and only a dispatch jump routes them to the trap stub. If the body
    // finishes with a trap-bearing remap no dispatch consumed — the match
    // table feeds a conditional branch, or nothing reads its result — a hole
    // symbol would match a synthesized trap row and be taken as a real match: a
    // silent misroute. Refuse the stamp (docs/core.md (the composition engine)).
    if remap_has_trap(&pending_remap) {
        return Err(LinkError::MonoHoleyMatchBranch(callee.name.to_string()));
    }

    // The shared read-trap stub, appended once after the body (control never
    // falls into it — the routine ends with a return).
    let mut trap_stub: Option<u32> = None;
    if needs_trap_stub {
        let op = trap_opcode(syntax, &callee.name)?;
        trap_stub = Some(blob.len() as u32);
        blob.push(op);
        blob.push(0); // trap #0 (unmapped read)
    }

    // Patch intra-function jumps now that every boundary is placed.
    for (op_pos, old_target, width) in jump_fixups {
        let new_target = *old_to_new
            .get(&old_target)
            .ok_or(LinkError::MalformedBlob {
                symbol: callee.name.to_string(),
                at: old_target,
            })?;
        let end = op_pos + u32::from(width);
        let off = i64::from(new_target) - i64::from(end);
        match width {
            1 => {
                let o = i8::try_from(off).map_err(|_| LinkError::MalformedBlob {
                    symbol: callee.name.to_string(),
                    at: op_pos,
                })?;
                blob[op_pos as usize] = o as u8;
            }
            4 => {
                let o = i32::try_from(off).expect("stamp jump offset fits i32");
                blob[op_pos as usize..op_pos as usize + 4].copy_from_slice(&o.to_le_bytes());
            }
            _ => unreachable!("relative jump width is 1 or 4"),
        }
    }

    // A framed call's descriptor names its exits as offsets in THIS copy's
    // blob; layout rebases those to absolute addresses the way it does for
    // any hand-authored descriptor (docs/formats.md (frames region)). The
    // recorded offsets are the enclosing routine's, so they translate
    // through the same map the copy's own jumps do.
    for (pos, old) in frame_exit_fixups {
        let new = *old_to_new.get(&old).ok_or(LinkError::MalformedBlob {
            symbol: callee.name.to_string(),
            at: old,
        })?;
        table[pos..pos + 4].copy_from_slice(&new.to_le_bytes());
    }

    // Resolve rebuilt dispatch entries to stamp-blob offsets.
    for (pos, de) in dispatch_fixups {
        let val = match de {
            DispEntry::TrapStub => trap_stub.expect("a trap row implies the stub was allocated"),
            DispEntry::OldCode(old) => *old_to_new.get(&old).ok_or(LinkError::MalformedTable {
                symbol: callee.name.to_string(),
                at: old,
            })?,
        };
        table[pos..pos + 4].copy_from_slice(&val.to_le_bytes());
    }

    Ok(StampBody {
        blob,
        table,
        table_fixups,
        calls,
        site_fixups,
        offsets: old_to_new,
        trap_rows,
        expanded_rows,
    })
}

fn trap_opcode(syntax: &ArchSyntax, name: &str) -> Result<u8, LinkError> {
    syntax.trap_opcode.ok_or_else(|| LinkError::BadBinding {
        callee: name.to_string(),
        message: "the dialect has no trap opcode to synthesize an unmapped-symbol trap".to_string(),
    })
}

/// Project a write vector to machine width: `phys(k)` gets the callee
/// element mapped through the write map; every unbound position keeps
/// (`0x7F`). Returns `None` at the first payload with no physical image (a
/// write map hole) — the caller lowers the whole instruction to a trap.
fn project_writes(
    vec: &[u32],
    comp: &Composite,
    projs: &[TapeProj],
    ma: usize,
    name: &str,
) -> Result<Option<Vec<u8>>, LinkError> {
    let mut out = vec![0x7Fu8; ma]; // keep
    for k in 0..vec.len().min(comp.tapes.len()) {
        let v = vec[k];
        if v == 0x7F {
            continue; // keep at phys(k)
        }
        match write_image(&comp.tapes[k], v as u16, projs[k].phys_card) {
            Some(p) if p <= 0x7E => out[projs[k].phys] = p as u8,
            Some(_) => {
                return Err(LinkError::BadFrameDescriptor {
                    symbol: name.to_string(),
                    message: "a stamped write maps onto a physical symbol past the 7-bit budget"
                        .to_string(),
                });
            }
            None => return Ok(None), // write map hole
        }
    }
    Ok(Some(out))
}

/// Project a move vector to machine width: `phys(k)` gets callee tape `k`'s
/// move code; every unbound position stays (`0`). Moves are physical motion,
/// so they are not symbol-translated.
fn project_moves(vec: &[u32], projs: &[TapeProj], ma: usize) -> Vec<u8> {
    let mut out = vec![0u8; ma]; // stay
    for k in 0..vec.len().min(projs.len()) {
        out[projs[k].phys] = vec[k] as u8;
    }
    out
}

/// Emit a projected write: [`project_writes`] onto the machine width, or a
/// `trap #1` when a payload has no physical image (an unmapped write).
#[allow(clippy::too_many_arguments)]
fn emit_write(
    blob: &mut Vec<u8>,
    syntax: &ArchSyntax,
    opcode: u8,
    vec: &[u32],
    comp: &Composite,
    projs: &[TapeProj],
    ma: usize,
    name: &str,
) -> Result<(), LinkError> {
    let Some(out) = project_writes(vec, comp, projs, ma, name)? else {
        // No physical image: the whole write traps unmapped-write.
        blob.push(trap_opcode(syntax, name)?);
        blob.push(1); // trap #1 (unmapped write)
        return Ok(());
    };
    blob.push(opcode);
    encode_vec_into(blob, &out);
    Ok(())
}

/// Emit a projected move: [`project_moves`] onto the machine width.
fn emit_move(blob: &mut Vec<u8>, opcode: u8, vec: &[u32], projs: &[TapeProj], ma: usize) {
    let out = project_moves(vec, projs, ma);
    blob.push(opcode);
    encode_vec_into(blob, &out);
}

/// Stamps a fused `wrmv [w…], [m…]` through the composite: the write half
/// projects exactly as [`emit_write`] does (a virtual write onto a map
/// hole traps unmapped-write, replacing the whole instruction — the move
/// never happens under a trap-stop), the move half exactly as
/// [`emit_move`] does, and the two physical vectors re-emit as the fused
/// two-group wire form (docs/formats.md (assembly text)). Behaviorally the
/// stamped `wr; mov` pair, in one instruction.
#[allow(clippy::too_many_arguments)]
fn emit_write_move(
    blob: &mut Vec<u8>,
    syntax: &ArchSyntax,
    opcode: u8,
    writes: &[u32],
    moves: &[u32],
    comp: &Composite,
    projs: &[TapeProj],
    ma: usize,
    name: &str,
) -> Result<(), LinkError> {
    let Some(wout) = project_writes(writes, comp, projs, ma, name)? else {
        // No physical image: the whole fused step traps unmapped-write; the
        // move half is dropped (execution stops before the projected moves).
        blob.push(trap_opcode(syntax, name)?);
        blob.push(1); // trap #1 (unmapped write)
        return Ok(());
    };
    let mout = project_moves(moves, projs, ma);
    blob.push(opcode);
    encode_vec_into(blob, &wout);
    encode_vec_into(blob, &mout);
    Ok(())
}

/// Encode a self-delimiting symbol/move vector: 7-bit payloads, high bit on
/// the last element (docs/formats.md (assembly text)). Every value ≤ `0x7F`.
fn encode_vec_into(blob: &mut Vec<u8>, vals: &[u8]) {
    let last = vals.len() - 1;
    for (i, &v) in vals.iter().enumerate() {
        blob.push(if i == last { v | 0x80 } else { v });
    }
}

/// Rewrite a match table from callee width to machine width, synthesizing
/// unmapped-read trap rows and expanding/dropping rows per the read
/// preimage, then restoring the canonical row order. Returns the new table
/// bytes, the dispatch-entry sources in the new row order, the count of
/// synthesized trap rows, and the count of EXTRA
/// rows one-way collapse expansion produced (the growth beyond one row per
/// surviving original — docs/core.md (the link report)).
fn rewrite_match_table(
    table: &[u8],
    t_off: u32,
    comp: &Composite,
    projs: &[TapeProj],
    ma: usize,
    name: &str,
) -> Result<(Vec<u8>, Vec<EntrySrc>, u32, u32), LinkError> {
    let base = t_off as usize;
    let malformed = || LinkError::MalformedTable {
        symbol: name.to_string(),
        at: t_off,
    };
    if base + 3 > table.len() {
        return Err(malformed());
    }
    let width = usize::from(table[base]);
    let count = usize::from(u16::from_le_bytes([table[base + 1], table[base + 2]]));
    if base + 3 + width * count > table.len() {
        return Err(malformed());
    }

    let mut rows: Vec<Vec<u8>> = Vec::new();
    let mut remap: Vec<EntrySrc> = Vec::new();
    let mut trap_rows = 0u32;
    let mut expanded = 0u32;

    // Synthesized read-trap rows, first-match: one per hole physical symbol.
    for proj in projs {
        for &u in &proj.holes {
            let mut row = vec![0x7Fu8; ma];
            row[proj.phys] = u;
            rows.push(row);
            remap.push(EntrySrc::TrapStub);
            trap_rows += 1;
        }
    }

    // The original rows, translated through the read preimage.
    for r in 0..count {
        let old = &table[base + 3 + r * width..base + 3 + (r + 1) * width];
        let mut opts: Vec<(usize, Vec<u8>)> = Vec::new();
        let mut dead = false;
        for (k, cell) in old.iter().enumerate().take(comp.tapes.len()) {
            let proj = &projs[k];
            if *cell == 0x7F {
                opts.push((proj.phys, vec![0x7F])); // wildcard stays wildcard
            } else {
                match proj.preimage.get(&u16::from(*cell)) {
                    Some(ps) if !ps.is_empty() => opts.push((proj.phys, ps.clone())),
                    _ => {
                        dead = true; // no physical symbol reads as this cell
                        break;
                    }
                }
            }
        }
        if dead {
            continue; // drop the row (and, in step, its dispatch entry)
        }
        let combos = cartesian(&opts);
        // A one-way collapse gives a cell several physical preimages, so one
        // original row expands into several — the extra rows are the growth.
        expanded += u32::try_from(combos.len().saturating_sub(1)).unwrap_or(u32::MAX);
        for combo in combos {
            let mut row = vec![0x7Fu8; ma];
            for (pos, val) in combo {
                row[pos] = val;
            }
            rows.push(row);
            remap.push(EntrySrc::OldRow(r as u16));
        }
    }

    let (rows, remap) = canonical_row_order(rows, remap);

    let mut out = vec![ma as u8];
    out.extend_from_slice(&(rows.len() as u16).to_le_bytes());
    for row in &rows {
        out.extend_from_slice(row);
    }
    Ok((out, remap, trap_rows, expanded))
}

/// Sort rewritten rows back into the canonical row order — exact rows
/// ascending, then partial-wildcard rows, the catch-all last (docs/core.md
/// (match tables)) — carrying each row's dispatch source with it.
///
/// The preimage rewrite renames cells, which permutes rows out of the order
/// the callee's table was authored in. Nothing at run time minds — given each
/// machine position is projected by at most one callee tape, the exact rows a
/// stamp emits are pairwise disjoint, and disjoint from every trap row, so
/// first-match reaches the same row either way. (Two callee tapes projecting
/// onto ONE machine position break that premise: the later projection
/// overwrites the earlier in a row, which can leave a row exact that overlaps
/// a trap row. Such a binding already disagrees with the frames lowering
/// before any reordering; it is a divergent shape under separate adjudication,
/// not one this ordering is claimed to preserve.) A disassembly does mind — it
/// prints rows in stored order, and an out-of-order table is text the
/// assembler refuses, breaking the round trip that keeps an image expressible
/// as assembly.
///
/// Row position IS load-bearing for one consumer: MR is the matched row's
/// 1-based ordinal and the dispatch table is indexed by it, so a row and its
/// dispatch source move as a pair or the table dispatches to the wrong
/// target. The sort is STABLE, so rows sharing a key keep their relative
/// order and first-match still selects the same one among them.
fn canonical_row_order(rows: Vec<Vec<u8>>, remap: Vec<EntrySrc>) -> (Vec<Vec<u8>>, Vec<EntrySrc>) {
    let mut ordered: Vec<(MatchRowClass, Vec<u8>, EntrySrc)> = rows
        .into_iter()
        .zip(remap)
        .map(|(row, src)| {
            // A stored cell is a 7-bit payload; 0x7F is the wildcard.
            let class = classify_match_row(
                row.iter()
                    .map(|&cell| (cell != 0x7F).then_some(u32::from(cell))),
            );
            (class, row, src)
        })
        .collect();
    ordered.sort_by(|a, b| a.0.cmp(&b.0));

    let mut sorted_rows = Vec::with_capacity(ordered.len());
    let mut sorted_remap = Vec::with_capacity(ordered.len());
    for (_, row, src) in ordered {
        sorted_rows.push(row);
        sorted_remap.push(src);
    }
    (sorted_rows, sorted_remap)
}

/// The cartesian product of per-position option lists, preserving position
/// order and each list's order (ascending physical preimage).
fn cartesian(opts: &[(usize, Vec<u8>)]) -> Vec<Vec<(usize, u8)>> {
    let mut result: Vec<Vec<(usize, u8)>> = vec![Vec::new()];
    for (pos, vals) in opts {
        let mut next = Vec::with_capacity(result.len() * vals.len());
        for combo in &result {
            for &v in vals {
                let mut c = combo.clone();
                c.push((*pos, v));
                next.push(c);
            }
        }
        result = next;
    }
    result
}

/// Read a dispatch table's entries (blob-relative code offsets, MR order).
fn read_dispatch(table: &[u8], d_off: u32, name: &str) -> Result<Vec<u32>, LinkError> {
    let base = d_off as usize;
    let malformed = || LinkError::MalformedTable {
        symbol: name.to_string(),
        at: d_off,
    };
    if base + 2 > table.len() {
        return Err(malformed());
    }
    let count = usize::from(u16::from_le_bytes([table[base], table[base + 1]]));
    if base + 2 + count * 4 > table.len() {
        return Err(malformed());
    }
    let mut out = Vec::with_capacity(count);
    for i in 0..count {
        let at = base + 2 + i * 4;
        out.push(u32::from_le_bytes(table[at..at + 4].try_into().unwrap()));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn func_ref(name: &str) -> FuncRef<'static> {
        FuncRef {
            name: Cow::Owned(name.to_string()),
            blob: Cow::Owned(Vec::new()),
            debug: None,
            calls: Vec::new(),
            bound: Vec::new(),
            table: Cow::Owned(Vec::new()),
            table_fixups: Vec::new(),
            site_fixups: Vec::new(),
            signature: None,
            interface: None,
            origin: 0,
        }
    }

    /// A one-tape, one-symbol machine — enough for a [`Closure`], which
    /// reads the signature only to compose children these name-minting
    /// tests never ask for.
    fn machine_sig() -> RoutineSig {
        RoutineSig {
            arity: 1,
            cardinalities: vec![1],
        }
    }

    /// A fresh, non-colliding stamp mints as `<routine>.<digest8>` — the
    /// period separator, never the reserved-and-unlexable `$` the earlier
    /// scheme used. Pinning this at the unit level means a regression back
    /// to `$` (or to any other separator) fails here even if every
    /// higher-level fixture happened not to exercise mono stamping that day.
    #[test]
    fn a_fresh_stamp_name_is_dot_separated() {
        let order = vec![func_ref("main"), func_ref("sub")];
        let sig = machine_sig();
        let mut closure = Closure::new(&order, &sig);

        let (idx, dup) = closure
            .intern(1, identity_composite(1, 1), None)
            .expect("a fresh name mints cleanly");
        assert!(!dup);
        assert_eq!(idx, order.len(), "the stamp lands right past order");
        assert_eq!(closure.nodes.len(), 1);
        assert!(
            closure.nodes[0].name.starts_with("sub."),
            "stamp name should be `sub.<digest>`: {}",
            closure.nodes[0].name
        );
        assert!(
            !closure.nodes[0].name.contains('$'),
            "must not regress to the unlexable `$` separator: {}",
            closure.nodes[0].name
        );
    }

    /// A hand-written routine that happens to occupy the exact name a stamp
    /// would mint refuses the stamp instead of silently colliding two
    /// distinct identities under one name. `expected_name` is computed with
    /// the SAME `digest`/format call `intern` makes internally, so this
    /// exercises a real, deterministic collision rather than a name chosen
    /// to merely look plausible — a test that skipped that step could pass
    /// against a version of `intern` that checked the wrong string.
    #[test]
    fn a_name_matching_an_existing_routine_is_refused() {
        let mut keyed = identity_composite(1, 1);
        keyed.routine = 1;
        let expected_name = format!("sub.{:08x}", digest(&keyed));

        let order = vec![func_ref("main"), func_ref("sub"), func_ref(&expected_name)];
        let sig = machine_sig();
        let mut closure = Closure::new(&order, &sig);

        let err = closure
            .intern(1, identity_composite(1, 1), None)
            .expect_err("the reserved name is already taken by a hand-written routine");
        assert_eq!(err, LinkError::StampNameCollision(expected_name));
        assert!(
            closure.nodes.is_empty(),
            "a refused intern must not leave a partial node behind"
        );
    }
}
