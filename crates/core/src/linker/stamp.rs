//! Mono stamping and hybrid classification for the composition engine
//! (docs/formats.md (frames profile)).
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
//!   rows PREPENDED (first-match): one machine-width row per hole physical
//!   symbol, dispatching to a shared `trap #0` stub.
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
    EngineStats, FramesPlan, SiteKind, bad_binding, lower_frames, routine_sig, scan_sites,
};
use super::resolve::FuncRef;
use crate::asm::decode::{self, Body, DecodedOperand};
use crate::asm::{ArchSyntax, Flow};
use crate::formats::object::{BoundCall, RoutineSig};
use crate::vm::OperandKind;

/// One (routine, composite) pair to stamp, with its map-visible name.
struct StampNode {
    routine: usize,
    composite: Composite,
    name: String,
}

/// The finished bytes of one stamped function, ready to wrap in a `FuncRef`.
struct StampBody {
    blob: Vec<u8>,
    table: Vec<u8>,
    table_fixups: Vec<(u32, u32)>,
    calls: Vec<(u32, usize)>,
    /// Unmapped-read trap rows synthesized into this stamp's match tables.
    trap_rows: u32,
    /// Extra match rows this stamp gained from one-way collapse expansion.
    expanded_rows: u32,
}

/// Report counters accumulated while building the mono stamp set — folded
/// into the link report's engine counters (docs/cli.md (the link report)).
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
/// the only path that routes the prepended trap rows to the trap stub. A
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
/// `FramesPlan` is produced.
pub(super) fn lower_mono<'a>(
    syntax: &ArchSyntax,
    order: Vec<FuncRef<'a>>,
    sites: &[Vec<SiteKind<'a>>],
    machine_sig: &RoutineSig,
) -> Result<(Vec<FuncRef<'a>>, Option<FramesPlan>, EngineStats), LinkError> {
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
    // original routine (§5.6), never a stamp.
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
                seeds.push((fi, *addr, *callee, record));
            }
        }
    }

    let (stamps, seed_target, stats) = mono_stamps(syntax, &order, sites, machine_sig, &seeds)?;
    let engine_stats = EngineStats {
        instantiations: u32::try_from(stamps.len()).unwrap_or(u32::MAX),
        dedup_savings: stats.dedup_savings,
        synthesized_trap_rows: stats.synthesized_trap_rows,
        expanded_rows: stats.expanded_rows,
    };

    // Retarget every original's bound sites to a plain call. Reachable sites
    // hit the stamp (or the original, for a collapse); a bound site in a
    // routine unreachable at the machine's frame never runs, so it points
    // harmlessly at the original callee.
    let mut out: Vec<FuncRef> = Vec::with_capacity(n + stamps.len());
    for (fi, mut f) in order.into_iter().enumerate() {
        for site in &sites[fi] {
            if let SiteKind::Bound {
                addr,
                callee,
                collapse,
                ..
            } = site
            {
                let target = if id_world[fi] && !collapse {
                    seed_target[&(fi, *addr)]
                } else {
                    *callee
                };
                f.calls.push((*addr + 1, target));
            }
        }
        f.bound = Vec::new();
        f.calls.sort_by_key(|&(h, _)| h);
        out.push(f);
    }
    out.extend(stamps);
    Ok((out, None, engine_stats))
}

// -- HYBRID ------------------------------------------------------------------

/// Lower under HYBRID: mono-stamp the completed-bijection sites, hand the
/// rest to the frames path. If every non-collapse site is a bijection this
/// is pure mono; if none is, pure frames; otherwise both, and the image is
/// FRAMES.
pub(super) fn lower_hybrid<'a>(
    syntax: &ArchSyntax,
    order: Vec<FuncRef<'a>>,
    sites: &[Vec<SiteKind<'a>>],
    machine_sig: &RoutineSig,
) -> Result<(Vec<FuncRef<'a>>, Option<FramesPlan>, EngineStats), LinkError> {
    let n = order.len();
    let id_world = identity_world(sites, n);

    // Classify the machine-frame bound sites: a bijection is a mono seed,
    // anything holey/one-way is a frames site.
    let mut seeds: Vec<(usize, u32, usize, &BoundCall)> = Vec::new();
    let mut mono_holes: Vec<HashSet<u32>> = vec![HashSet::new(); n];
    let mut any_frames = false;
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
                    seeds.push((fi, *addr, *callee, record));
                    mono_holes[fi].insert(*addr);
                } else {
                    any_frames = true;
                }
            }
        }
    }

    // The two degenerate cases route straight to a single mechanism.
    if seeds.is_empty() {
        return lower_frames(syntax, order, sites, machine_sig);
    }
    if !any_frames {
        return lower_mono(syntax, order, sites, machine_sig);
    }

    // Mixed: build the mono stamps, promote the bijection bound sites to
    // plain calls into them (dropping those bound records), then let the
    // frames path lower whatever bound records remain.
    let (stamps, seed_target, mono_stats) =
        mono_stamps(syntax, &order, sites, machine_sig, &seeds)?;
    let instantiations = u32::try_from(stamps.len()).unwrap_or(u32::MAX);

    let mut new_order: Vec<FuncRef> = Vec::with_capacity(n + stamps.len());
    for (fi, mut f) in order.into_iter().enumerate() {
        if !mono_holes[fi].is_empty() {
            for &addr in &mono_holes[fi] {
                f.calls.push((addr + 1, seed_target[&(fi, addr)]));
            }
            let monos = &mono_holes[fi];
            f.bound.retain(|&(hole, _, _)| !monos.contains(&(hole - 1)));
            f.calls.sort_by_key(|&(h, _)| h);
        }
        new_order.push(f);
    }
    new_order.extend(stamps);

    // The frames path re-scans the mono-rewritten order; the stamps carry no
    // bound calls, so they flow through as ordinary functions.
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
    Ok((order, plan, stats))
}

/// A completed bijection (mono-eligible): every bound tape equal-size (so
/// identity completion is total) with no one-way `=>` pair (which is
/// excluded from write-back). Injectivity on equal-size bindings is already
/// enforced by the engine's `validate_binding`, and total + injective on
/// equal finite alphabets is surjective — so this is the totality check the
/// classifier owns, completing the bijection determination.
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
        if tb.pairs.iter().any(|p| p.one_way) {
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

/// Build the mono stamp set reachable from `seeds` (machine-frame bound
/// sites to specialize), closing over each stamp's own calls (mono all the
/// way down). Returns the stamp `FuncRef`s (order indices `order.len()..`)
/// and, per seed, its stamp order index.
#[allow(clippy::type_complexity)]
fn mono_stamps<'a>(
    syntax: &ArchSyntax,
    order: &[FuncRef<'a>],
    sites: &[Vec<SiteKind<'a>>],
    machine_sig: &RoutineSig,
    seeds: &[(usize, u32, usize, &'a BoundCall)],
) -> Result<(Vec<FuncRef<'a>>, HashMap<(usize, u32), usize>, StampStats), LinkError> {
    let ma = machine_sig.arity as usize;
    let id = identity_composite(ma, 0);

    let mut nodes: Vec<StampNode> = Vec::new();
    let mut key_to_slot: HashMap<Vec<u8>, usize> = HashMap::new();
    let mut worklist: VecDeque<usize> = VecDeque::new();
    let mut seed_target: HashMap<(usize, u32), usize> = HashMap::new();
    let mut stats = StampStats::default();

    // Seed: compose each site's binding at the machine identity.
    for &(fi, addr, callee, record) in seeds {
        let callee_sig = routine_sig(order, callee)?;
        let child = compose(&id, callee, &record.binding, callee_sig)
            .map_err(|e| bad_binding(&order[callee].name, &e))?;
        let (idx, dup) = intern(
            &mut nodes,
            &mut key_to_slot,
            &mut worklist,
            order,
            callee,
            child,
        );
        if dup {
            stats.dedup_savings += 1;
        }
        seed_target.insert((fi, addr), idx);
    }

    // Close over each stamp's calls. A plain call inherits the stamp's
    // composite (the callee runs under the same frame); a bound call composes
    // its binding onto it; both stay mono. A raw `call.m` is a frames
    // instruction — refused.
    let mut stamp_targets: Vec<HashMap<u32, usize>> = Vec::new();
    while let Some(slot) = worklist.pop_front() {
        let routine = nodes[slot].routine;
        let comp = nodes[slot].composite.clone();
        let mut targets: HashMap<u32, usize> = HashMap::new();
        for site in &sites[routine] {
            match site {
                SiteKind::RawCallM { .. } => {
                    return Err(LinkError::MonoRawFrame(order[routine].name.to_string()));
                }
                SiteKind::Plain { addr, callee } => {
                    let mut child = comp.clone();
                    child.routine = *callee;
                    let (idx, dup) = intern(
                        &mut nodes,
                        &mut key_to_slot,
                        &mut worklist,
                        order,
                        *callee,
                        child,
                    );
                    if dup {
                        stats.dedup_savings += 1;
                    }
                    targets.insert(*addr, idx);
                }
                SiteKind::Bound {
                    addr,
                    callee,
                    record,
                    ..
                } => {
                    let callee_sig = routine_sig(order, *callee)?;
                    let child = compose(&comp, *callee, &record.binding, callee_sig)
                        .map_err(|e| bad_binding(&order[*callee].name, &e))?;
                    // A binding that composes back to a genuine full
                    // pass-through — identity placement and maps AND the callee
                    // alphabet as wide as the machine's on every tape — lowers
                    // to the original routine. A narrower or wider callee keeps
                    // a cardinality hole, so it is stamped instead (its trap
                    // rows are synthesized from the alphabet gap in build_stamp).
                    let idx = if is_full_passthrough(&child, machine_sig, callee_sig) {
                        *callee
                    } else {
                        let (idx, dup) = intern(
                            &mut nodes,
                            &mut key_to_slot,
                            &mut worklist,
                            order,
                            *callee,
                            child,
                        );
                        if dup {
                            stats.dedup_savings += 1;
                        }
                        idx
                    };
                    targets.insert(*addr, idx);
                }
            }
        }
        if stamp_targets.len() <= slot {
            stamp_targets.resize_with(slot + 1, HashMap::new);
        }
        stamp_targets[slot] = targets;
    }
    stamp_targets.resize_with(nodes.len(), HashMap::new);

    // Materialize each stamp's body.
    let mut stamp_funcs = Vec::with_capacity(nodes.len());
    for (slot, node) in nodes.iter().enumerate() {
        let callee = &order[node.routine];
        let callee_sig = routine_sig(order, node.routine)?;
        let body = build_stamp(
            syntax,
            callee,
            &node.composite,
            machine_sig,
            callee_sig,
            &stamp_targets[slot],
        )?;
        stats.synthesized_trap_rows += body.trap_rows;
        stats.expanded_rows += body.expanded_rows;
        stamp_funcs.push(FuncRef {
            name: Cow::Owned(node.name.clone()),
            blob: Cow::Owned(body.blob),
            debug: None,
            calls: body.calls,
            bound: Vec::new(),
            table: Cow::Owned(body.table),
            table_fixups: body.table_fixups,
            signature: None,
        });
    }

    Ok((stamp_funcs, seed_target, stats))
}

/// Intern a (routine, composite) into the stamp set, deduped by canonical
/// key. Returns its ORDER index (`order.len() + slot`) and whether it
/// resolved to an ALREADY-built stamp (a stamp the dedup avoided).
fn intern(
    nodes: &mut Vec<StampNode>,
    key_to_slot: &mut HashMap<Vec<u8>, usize>,
    worklist: &mut VecDeque<usize>,
    order: &[FuncRef],
    routine: usize,
    mut composite: Composite,
) -> (usize, bool) {
    composite.routine = routine;
    let key = canonical_key(&composite);
    if let Some(&slot) = key_to_slot.get(&key) {
        return (order.len() + slot, true);
    }
    let slot = nodes.len();
    let name = format!("{}${:08x}", order[routine].name, digest(&composite));
    nodes.push(StampNode {
        routine,
        composite,
        name,
    });
    key_to_slot.insert(key, slot);
    worklist.push_back(slot);
    (order.len() + slot, false)
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
        Some(v) if u32::from(v) < callee_card => Some(v),
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

/// Re-emit the callee's generic body at the machine width, projecting every
/// tape op and match/dispatch table through the composite (docs/formats.md
/// (frames profile)).
fn build_stamp(
    syntax: &ArchSyntax,
    callee: &FuncRef,
    comp: &Composite,
    machine_sig: &RoutineSig,
    callee_sig: &RoutineSig,
    targets: &HashMap<u32, usize>,
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

        // A call or tail jump to a child stamp / original: a plain far call.
        if let Some(&target) = targets.get(&old_addr) {
            blob.push(entry.opcode);
            let hole = blob.len() as u32;
            blob.extend_from_slice(&[0u8; 4]);
            calls.push((hole, target));
            continue;
        }

        match entry.operand {
            OperandKind::None => {
                blob.extend_from_slice(&blob_bytes[old_addr as usize..(old_addr + d.len) as usize]);
            }
            OperandKind::Imm8 => {
                // A multi-exit return is a frames instruction — refused under
                // mono. A hand-authored `trap #k` passes through.
                if entry.flow == Flow::Stop {
                    return Err(LinkError::MonoRawFrame(callee.name.to_string()));
                }
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
    // symbol would match a prepended trap row and be taken as a real match: a
    // silent misroute. Refuse the stamp (docs/formats.md (frames profile)).
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
/// (`0x7F`). A payload with no physical image turns the whole instruction
/// into `trap #1`.
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
            None => {
                // No physical image: the whole write traps unmapped-write.
                blob.push(trap_opcode(syntax, name)?);
                blob.push(1); // trap #1 (unmapped write)
                return Ok(());
            }
        }
    }
    blob.push(opcode);
    encode_vec_into(blob, &out);
    Ok(())
}

/// Project a move vector to machine width: `phys(k)` gets callee tape `k`'s
/// move code; every unbound position stays (`0`). Moves are physical motion,
/// so they are not symbol-translated.
fn emit_move(blob: &mut Vec<u8>, opcode: u8, vec: &[u32], projs: &[TapeProj], ma: usize) {
    let mut out = vec![0u8; ma]; // stay
    for k in 0..vec.len().min(projs.len()) {
        out[projs[k].phys] = vec[k] as u8;
    }
    blob.push(opcode);
    encode_vec_into(blob, &out);
}

/// Encode a self-delimiting symbol/move vector: 7-bit payloads, high bit on
/// the last element (docs/formats.md (assembly text)). Every value ≤ `0x7F`.
fn encode_vec_into(blob: &mut Vec<u8>, vals: &[u8]) {
    let last = vals.len() - 1;
    for (i, &v) in vals.iter().enumerate() {
        blob.push(if i == last { v | 0x80 } else { v });
    }
}

/// Rewrite a match table from callee width to machine width, prepending
/// unmapped-read trap rows and expanding/dropping rows per the read
/// preimage. Returns the new table bytes, the dispatch-entry sources in the
/// new row order, the count of synthesized trap rows, and the count of EXTRA
/// rows one-way collapse expansion produced (the growth beyond one row per
/// surviving original — docs/cli.md (the link report)).
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

    let mut out = vec![ma as u8];
    out.extend_from_slice(&(rows.len() as u16).to_le_bytes());
    for row in &rows {
        out.extend_from_slice(row);
    }
    Ok((out, remap, trap_rows, expanded))
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
