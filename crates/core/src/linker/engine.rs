//! The composition engine: the link-time pre-pass between `resolve` and
//! `layout` that lowers declarative bound calls under the FRAMES mechanism
//! (docs/formats.md (frames profile)). It cannot run inside layout —
//! layout is decode-once/shrink-only and injects no code — so it runs
//! first, rewriting each reachable routine's bound-call sites into framed
//! calls and computing the runtime compose table that selects a composite
//! per (active frame, call site).
//!
//! ## The model (ruled 2026-07-17)
//!
//! `call.m`'s operand table-half is a **call-site index** `S`. At runtime
//! `FR' = compose[FR][S]`, `descriptor = directory[FR'-1]`; `FR` is a
//! composite index (0 = the identity context). Composition happens at
//! link, so every descriptor in the image is absolute. The engine:
//!
//! - keeps ONE generic copy of each routine (the code is
//!   context-independent — the context lives in the compose table); every
//!   bound-call site becomes a framed call, or a plain call when the
//!   binding is a full pass-through (§5.6 identity collapse);
//! - enumerates the finite closure of `(routine, composite)` pairs reached
//!   from the entry at identity — following plain calls (context
//!   preserved) and bound calls (context composed) — deterministically
//!   (BFS, dedup by canonical key);
//! - synthesizes one frame descriptor per distinct composite (deduped),
//!   plus the raw hand-authored descriptors already in the objects;
//! - builds the `(K+1) × S` compose table: `compose[F][S]` is the index of
//!   `compose(directory[F], site-binding)` for every reachable `(F, S)`, or
//!   0 for an unreachable pair (which the runtime never consults).
//!
//! Raw `call.m` sites (hand-authored `.frame`/`call.m`) stay opaque: their
//! descriptor is an absolute placement, so each contributes a directory
//! entry and a CONSTANT compose column (the same index for every active
//! frame), preserving the 5a semantics even inside an engine-composed
//! routine.
//!
//! ## The blob rewrite
//!
//! A bound call assembles as a far-call-shaped 5-byte instruction with a
//! `BoundCall` record. Layout cannot classify it (its hole is not a
//! relocation), so the engine rewrites the blob first: a framed site
//! widens 5 → 9 bytes; every following offset shifts by +4 per preceding
//! widened site; internal jump targets are re-encoded, and relocation
//! holes, table fixups, debug labels/lines, and raw-descriptor exit
//! offsets all shift through the same per-function offset map. Layout then
//! treats the rewritten blob as an ordinary input.

use std::borrow::Cow;
use std::collections::{HashMap, HashSet, VecDeque};

use super::compose::{
    Composite, absolutize, canonical_key, compose, identity_composite, is_identity,
};
use super::resolve::FuncRef;
use super::{CallMech, LinkError};
use crate::asm::decode::{self, Body, DecodedOperand};
use crate::asm::{ArchSyntax, Flow};
use crate::formats::object::{BoundCall, RoutineSig};
use crate::vm::OperandKind;
use crate::vm::frame::descriptor_bytes;

/// Where a directory entry's descriptor bytes come from. Engine composites
/// are synthesized (address-independent — no exits); raw descriptors are
/// authored inside a function's table blob and located once layout knows
/// that function's table base.
pub(super) enum DirSource {
    /// `engine_descriptors[i]`, appended to the table section by layout.
    Engine(usize),
    /// The raw `.frame` descriptor at `table_offset` inside function
    /// `func`'s table blob (docs/formats.md (frame descriptors)).
    Raw { func: usize, table_offset: u32 },
}

/// The address-independent frames model the engine hands layout. Layout
/// resolves the two address-dependent pieces (engine-descriptor and
/// raw-descriptor table offsets) into the directory and emits the region
/// verbatim (docs/formats.md (frames region)).
pub(super) struct FramesPlan {
    /// One synthesized descriptor per engine composite, in engine-composite
    /// (directory) order. No `retx` exits, so byte-content is address-free.
    pub engine_descriptors: Vec<Vec<u8>>,
    /// The directory in final composite-index order: entry `i` is runtime
    /// composite index `i + 1` (index 0 is the identity context, no
    /// descriptor). Engine composites come first, then raw descriptors.
    pub directory: Vec<DirSource>,
    /// The compose table: `K + 1` rows (active frame 0..=K) each with `S`
    /// columns (framed-call sites in (function, piece) emission order),
    /// values = composite index (1..=K) or 0 (unreachable pair).
    pub compose: Vec<Vec<u16>>,
}

/// One control-transfer site in a routine's original blob, in offset order.
enum SiteKind<'a> {
    /// A relocated plain call or tail jump: the callee inherits the active
    /// frame (context preserved).
    Plain { callee: usize },
    /// A declarative bound call. `collapse` marks a full pass-through that
    /// lowers to a plain call (§5.6).
    Bound {
        addr: u32,
        callee: usize,
        record: &'a BoundCall,
        collapse: bool,
    },
    /// A hand-authored framed call (`call.m`): its descriptor is an
    /// absolute placement (constant compose column). `frame_hole` locates
    /// its frame-half table fixup, naming the descriptor.
    RawCallM { frame_hole: u32 },
}

/// Run the composition engine. Returns the (possibly rewritten) order and a
/// `FramesPlan` when any reachable routine carries a bound call; returns
/// `None` (order untouched) otherwise, so bindingless links stay on the
/// byte-identical 5a/T2 path. Only `CallMech::Frames` is implemented;
/// `Mono`/`Hybrid` error until the stamping engine lands.
pub(super) fn lower<'a>(
    syntax: &ArchSyntax,
    order: Vec<FuncRef<'a>>,
    machine_sig: &RoutineSig,
    call_mech: CallMech,
) -> Result<(Vec<FuncRef<'a>>, Option<FramesPlan>), LinkError> {
    // Scan every reached routine for its control sites. Bindingless links
    // (no bound call anywhere) skip the engine entirely.
    let sites: Vec<Vec<SiteKind>> = order
        .iter()
        .map(|f| scan_sites(syntax, f, machine_sig, &order))
        .collect::<Result<_, _>>()?;
    let has_bound = sites
        .iter()
        .any(|s| s.iter().any(|k| matches!(k, SiteKind::Bound { .. })));
    if !has_bound {
        return Ok((order, None));
    }

    // FRAMES is the only implemented mechanism this phase.
    if call_mech != CallMech::Frames {
        return Err(LinkError::UnsupportedCallMech(call_mech));
    }

    // Every routine that frames anything needs a framed-call opcode.
    let fc_opcode = syntax
        .framed_call_opcode()
        .ok_or_else(|| LinkError::BadBinding {
            callee: order[0].name.to_string(),
            message: "the dialect has no framed-call opcode to lower a bound call into".to_string(),
        })?;

    let machine_arity = machine_sig.arity as usize;

    // --- closure over (routine, composite): engine composites + columns ---
    let mut engine_comps: Vec<Composite> = Vec::new();
    let mut comp_index: HashMap<Vec<u8>, u16> = HashMap::new();
    // (function, bound-site addr) -> active-frame row -> child composite index.
    let mut site_columns: HashMap<(usize, u32), HashMap<u16, u16>> = HashMap::new();

    let mut visited: HashSet<(usize, Vec<u8>)> = HashSet::new();
    let mut queue: VecDeque<(usize, Composite)> = VecDeque::new();
    queue.push_back((0, identity_composite(machine_arity, 0)));

    while let Some((fi, ctx)) = queue.pop_front() {
        let ckey = canonical_key(&ctx);
        if !visited.insert((fi, ckey.clone())) {
            continue;
        }
        // Row of the active frame: 0 for identity, else the composite's
        // 1-based engine index (every non-identity context was interned
        // before it was enqueued).
        let fr_row = comp_index.get(&ckey).copied().unwrap_or(0);

        for site in &sites[fi] {
            match site {
                SiteKind::Plain { callee } => {
                    queue.push_back((*callee, ctx.clone()));
                }
                SiteKind::Bound {
                    callee,
                    record,
                    collapse: true,
                    ..
                } => {
                    // A full pass-through inherits the active frame — the
                    // plain-call semantics, in every context.
                    let _ = record;
                    queue.push_back((*callee, ctx.clone()));
                }
                SiteKind::Bound {
                    addr,
                    callee,
                    record,
                    collapse: false,
                } => {
                    let callee_sig = routine_sig(&order, *callee)?;
                    let child = compose(&ctx, *callee, &record.binding, callee_sig)
                        .map_err(|e| bad_binding(order[*callee].name, &e))?;
                    let idx = intern_composite(&mut engine_comps, &mut comp_index, child.clone());
                    site_columns
                        .entry((fi, *addr))
                        .or_default()
                        .insert(fr_row, idx);
                    queue.push_back((*callee, child));
                }
                SiteKind::RawCallM { .. } => {
                    // Opaque absolute placement: no composition, no closure
                    // descent (its callee is reached for layout via the
                    // relocation resolve already followed).
                }
            }
        }
    }

    let engine_count = engine_comps.len();

    // --- raw descriptors: directory entries + constant columns ---
    // Deterministic order: function order, then frame-half offset order.
    let mut raw_dir: Vec<(usize, u32)> = Vec::new();
    let mut raw_index: HashMap<(usize, u32), u16> = HashMap::new();
    for (fi, func_sites) in sites.iter().enumerate() {
        for site in func_sites {
            if let SiteKind::RawCallM { frame_hole, .. } = site {
                let table_off = order[fi]
                    .table_fixups
                    .iter()
                    .find(|(h, _)| h == frame_hole)
                    .map(|(_, t)| *t)
                    .ok_or_else(|| LinkError::MalformedBlob {
                        symbol: order[fi].name.to_string(),
                        at: *frame_hole,
                    })?;
                let next = engine_count + raw_dir.len() + 1;
                raw_index.entry((fi, table_off)).or_insert_with(|| {
                    raw_dir.push((fi, table_off));
                    u16::try_from(next).expect("composite index fits u16")
                });
            }
        }
    }
    let k = engine_count + raw_dir.len();

    // --- rewrite blobs and collect the ordered framed-call site list ---
    // Each entry is the column source for one framed-call piece, in the
    // (function, piece) order layout will emit them.
    enum ColSrc {
        Bound { func: usize, addr: u32 },
        Raw { func: usize, table_off: u32 },
    }
    let mut columns_src: Vec<ColSrc> = Vec::new();
    let mut new_order = Vec::with_capacity(order.len());
    for (f, func_sites) in order.into_iter().zip(&sites) {
        let (rewritten, framed) = rewrite_blob(syntax, fc_opcode, f, func_sites)?;
        let fi = new_order.len();
        new_order.push(rewritten);
        for site in framed {
            match site {
                FramedSite::Bound { addr } => columns_src.push(ColSrc::Bound { func: fi, addr }),
                FramedSite::Raw { table_off } => columns_src.push(ColSrc::Raw {
                    func: fi,
                    table_off,
                }),
            }
        }
    }
    let s = columns_src.len();

    // Every bound call collapsed to a plain call and nothing else frames:
    // the rewrite stands (bound holes are now relocations) but no frames
    // region is needed — hand the rewritten order back plan-less.
    if columns_src.is_empty() {
        return Ok((new_order, None));
    }

    // --- compose matrix: (K+1) rows × S columns ---
    let mut compose_rows: Vec<Vec<u16>> = Vec::with_capacity(k + 1);
    for row in 0..=u16::try_from(k).expect("composite count fits u16") {
        let mut cols = Vec::with_capacity(s);
        for src in &columns_src {
            let v = match src {
                ColSrc::Bound { func, addr } => site_columns
                    .get(&(*func, *addr))
                    .and_then(|per_row| per_row.get(&row).copied())
                    .unwrap_or(0),
                ColSrc::Raw { func, table_off } => {
                    // Constant column: the absolute descriptor's index in
                    // every row (the site ignores the active frame).
                    raw_index[&(*func, *table_off)]
                }
            };
            cols.push(v);
        }
        compose_rows.push(cols);
    }

    // --- directory + synthesized descriptors ---
    let mut engine_descriptors = Vec::with_capacity(engine_count);
    for c in &engine_comps {
        engine_descriptors.push(materialize(c, machine_sig, &new_order)?);
    }
    let mut directory: Vec<DirSource> = (0..engine_count).map(DirSource::Engine).collect();
    for (func, table_offset) in raw_dir {
        directory.push(DirSource::Raw { func, table_offset });
    }

    Ok((
        new_order,
        Some(FramesPlan {
            engine_descriptors,
            directory,
            compose: compose_rows,
        }),
    ))
}

/// A framed-call piece produced by the rewrite, in offset order — the
/// source of one compose column.
enum FramedSite {
    Bound { addr: u32 },
    Raw { table_off: u32 },
}

/// Validate every hand-authored frame descriptor's physical-tape indices
/// against the machine arity (docs/formats.md (frame descriptors)): a
/// `phys` at or past `machine_sig.arity` would project a virtual tape onto
/// a physical tape the machine does not have. Walks each function's table
/// for its frame descriptors (the frame-half table fixups). Independent of
/// the engine's bound-call lowering, so it also guards pure hand-authored
/// (5a) frames images.
pub(super) fn validate_frame_phys(
    syntax: &ArchSyntax,
    order: &[FuncRef],
    machine_sig: &RoutineSig,
) -> Result<(), LinkError> {
    let arity = u32::from(machine_sig.arity);
    for f in order {
        let blob: &[u8] = &f.blob;
        let table: &[u8] = &f.table;
        for &(hole, toff) in &f.table_fixups {
            let is_frame = hole
                .checked_sub(5)
                .and_then(|p| blob.get(p as usize))
                .and_then(|&op| syntax.by_opcode(op))
                .is_some_and(|e| e.operand == OperandKind::FramedCall);
            if is_frame {
                check_descriptor_phys(table, toff, arity, f.name)?;
            }
        }
    }
    Ok(())
}

/// Read a frame descriptor's per-tape `phys` bytes and check each is below
/// the machine arity. A malformed header is left to layout's own table walk
/// (it reports `MalformedTable`); this only reads the phys fields it can.
fn check_descriptor_phys(
    table: &[u8],
    start: u32,
    arity: u32,
    name: &str,
) -> Result<(), LinkError> {
    let len = table.len() as u32;
    let u16_at = |p: u32| -> Option<u32> {
        (p + 2 <= len).then(|| {
            u32::from(u16::from_le_bytes([
                table[p as usize],
                table[p as usize + 1],
            ]))
        })
    };
    let Some(&darity) = table.get(start as usize) else {
        return Ok(());
    };
    let mut pos = start + 3; // arity u8 + exit_count u16
    for _ in 0..darity {
        let Some(&phys) = table.get(pos as usize) else {
            return Ok(());
        };
        if u32::from(phys) >= arity {
            return Err(LinkError::BadFrameDescriptor {
                symbol: name.to_string(),
                message: format!("physical tape {phys} is at or past the machine arity {arity}"),
            });
        }
        pos += 1;
        for _ in 0..2 {
            let Some(map_len) = u16_at(pos) else {
                return Ok(());
            };
            pos += 2 + 2 * map_len;
        }
    }
    Ok(())
}

/// Intern a composite into the engine directory, deduped by canonical key.
/// Returns its 1-based directory index (engine composites occupy 1..=E).
fn intern_composite(
    comps: &mut Vec<Composite>,
    index: &mut HashMap<Vec<u8>, u16>,
    c: Composite,
) -> u16 {
    let key = canonical_key(&c);
    if let Some(&i) = index.get(&key) {
        return i;
    }
    comps.push(c);
    let i = u16::try_from(comps.len()).expect("composite index fits u16");
    index.insert(key, i);
    i
}

fn routine_sig<'a>(order: &[FuncRef<'a>], idx: usize) -> Result<&'a RoutineSig, LinkError> {
    order[idx].signature.ok_or_else(|| LinkError::BadBinding {
        callee: order[idx].name.to_string(),
        message: "callee has no routine signature to bind against".to_string(),
    })
}

fn bad_binding(callee: &str, e: &super::compose::ComposeError) -> LinkError {
    LinkError::BadBinding {
        callee: callee.to_string(),
        message: e.to_string(),
    }
}

/// Decode a routine's ORIGINAL blob into its ordered control sites, and
/// validate every bound call's binding once against the caller and callee
/// signatures (docs/formats.md (bound calls)).
fn scan_sites<'a>(
    syntax: &ArchSyntax,
    f: &FuncRef<'a>,
    machine_sig: &RoutineSig,
    order: &[FuncRef<'a>],
) -> Result<Vec<SiteKind<'a>>, LinkError> {
    let blob: &[u8] = &f.blob;
    let calls: HashMap<u32, usize> = f.calls.iter().copied().collect();
    let bound: HashMap<u32, (usize, &BoundCall)> =
        f.bound.iter().map(|&(h, c, r)| (h, (c, r))).collect();

    // The caller's per-virtual-tape alphabet: the routine's own signature
    // when it has one, else the machine signature (at the entry the caller
    // IS the machine). This is the invariant carrier of a routine's
    // virtual-tape cardinalities across every composite it runs under.
    let caller_sig = f.signature.unwrap_or(machine_sig);

    let mut out = Vec::new();
    for d in decode::decode_stream(syntax, blob, 0, blob.len() as u32) {
        let Body::Instr { mnemonic, operand } = d.body else {
            continue;
        };
        let entry = syntax
            .by_mnemonic(mnemonic)
            .expect("mnemonic came from a successful decode");
        match (entry.flow, &operand) {
            (Flow::Call, DecodedOperand::RelTarget(_)) => {
                let hole = d.addr + 1;
                if let Some(&(callee, record)) = bound.get(&hole) {
                    let callee_sig = routine_sig(order, callee)?;
                    let composite =
                        validate_binding(caller_sig, callee_sig, order[callee].name, record)?;
                    // §5.6 collapse (handoff): a site lowers to a plain call
                    // ONLY when the binding, absolutized at the caller's own
                    // identity, IS the caller-arity identity — a full
                    // pass-through. A projecting identity (fewer tapes than
                    // the caller) fails the arity check and stays a framed
                    // call.
                    let collapse = is_identity(&composite)
                        && composite.tapes.len() == caller_sig.arity as usize;
                    out.push(SiteKind::Bound {
                        addr: d.addr,
                        callee,
                        record,
                        collapse,
                    });
                } else if let Some(&callee) = calls.get(&hole) {
                    out.push(SiteKind::Plain { callee });
                }
            }
            (Flow::Call, DecodedOperand::FramedCall { .. }) => {
                out.push(SiteKind::RawCallM {
                    frame_hole: d.addr + 5,
                });
            }
            (Flow::Jump, DecodedOperand::RelTarget(_)) => {
                // A relocated tail jump transfers control preserving the
                // frame — a plain-call closure edge.
                if let Some(&callee) = calls.get(&(d.addr + 1)) {
                    out.push(SiteKind::Plain { callee });
                }
            }
            _ => {}
        }
    }
    Ok(out)
}

/// Validate one binding once (docs/formats.md (bound calls)) and return its
/// composite absolutized at the caller's own identity — the reference used
/// for the §5.6 collapse decision. Core legality comes through `absolutize`
/// (arity, caller-tape range, callee-symbol range, blank pinning,
/// per-direction conflict), plus the two checks the algebra leaves to the
/// linker: the caller symbol range and the equal-size bijection completion.
fn validate_binding(
    caller_sig: &RoutineSig,
    callee_sig: &RoutineSig,
    callee_name: &str,
    record: &BoundCall,
) -> Result<Composite, LinkError> {
    let caller_arity = caller_sig.arity as usize;
    let composite = absolutize(caller_arity, 0, &record.binding, callee_sig)
        .map_err(|e| bad_binding(callee_name, &e))?;

    for (k, tb) in record.binding.iter().enumerate() {
        let caller_card = caller_sig
            .cardinalities
            .get(usize::from(tb.caller_tape))
            .copied()
            .unwrap_or(0);
        // Caller-side symbol range: a bound caller symbol must lie in the
        // caller virtual tape's alphabet.
        for pair in &tb.pairs {
            if pair.src >= caller_card {
                return Err(LinkError::BadBinding {
                    callee: callee_name.to_string(),
                    message: format!(
                        "binding tape {k} maps caller symbol {} outside the caller \
                         alphabet (cardinality {caller_card})",
                        pair.src
                    ),
                });
            }
        }
        // Equal-size alphabets must identity-complete to a bijection: the
        // read map, filled with identity for unlisted symbols, must be
        // injective (unequal sizes stay hole-based, no completion).
        let callee_card = callee_sig.cardinalities.get(k).copied().unwrap_or(0);
        if caller_card == callee_card {
            let tape = &composite.tapes[k];
            let mut seen: HashSet<u16> = HashSet::new();
            for s in 0..caller_card {
                let Ok(s16) = u16::try_from(s) else { break };
                if let Some(v) = tape.rmap.apply(s16)
                    && !seen.insert(v)
                {
                    return Err(LinkError::BadBinding {
                        callee: callee_name.to_string(),
                        message: format!(
                            "binding tape {k} is not injective on an equal-size alphabet: \
                             identity completion collides on {v}"
                        ),
                    });
                }
            }
        }
    }
    Ok(composite)
}

/// Materialize a composite into frame-descriptor bytes (docs/formats.md
/// (frame descriptors)): per virtual tape a physical index and dense
/// read/write maps sized to the relevant alphabet — identity where it fits,
/// `0xFFFF` holes where a symbol has no image in the target alphabet
/// (unequal-size, hole-based). No exits (a declarative bound call is
/// single-exit — it returns through the pushed return address).
fn materialize(
    c: &Composite,
    machine_sig: &RoutineSig,
    order: &[FuncRef],
) -> Result<Vec<u8>, LinkError> {
    let callee_sig = routine_sig(order, c.routine)?;
    let mut dense: Vec<(u8, Vec<u16>, Vec<u16>)> = Vec::with_capacity(c.tapes.len());
    for (k, t) in c.tapes.iter().enumerate() {
        let phys_card = machine_sig
            .cardinalities
            .get(usize::from(t.phys))
            .copied()
            .ok_or_else(|| LinkError::BadFrameDescriptor {
                symbol: order[c.routine].name.to_string(),
                message: format!(
                    "frame descriptor physical tape {} is at or past the machine arity {}",
                    t.phys, machine_sig.arity
                ),
            })?;
        let callee_card = callee_sig.cardinalities.get(k).copied().unwrap_or(0);
        // Read map: physical symbol (0..phys_card) -> virtual (< callee_card).
        let rmap = dense_map(
            |s| t.rmap.apply(s),
            t.rmap.is_identity(),
            phys_card,
            callee_card,
        );
        // Write map: virtual symbol (0..callee_card) -> physical (< phys_card).
        let wmap = dense_map(
            |s| t.wmap.apply(s),
            t.wmap.is_identity(),
            callee_card,
            phys_card,
        );
        dense.push((t.phys, rmap, wmap));
    }
    let entries: Vec<(u8, &[u16], &[u16])> = dense
        .iter()
        .map(|(p, r, w)| (*p, r.as_slice(), w.as_slice()))
        .collect();
    Ok(descriptor_bytes(&entries, &[]))
}

/// A dense symbol map over `domain_card` inputs whose images must lie in
/// `codomain_card`: empty (identity, no translation) when the sparse map is
/// identity and the alphabets match; otherwise one `u16` per input —
/// `0xFFFF` for a hole or an identity symbol with no image in the codomain.
fn dense_map(
    apply: impl Fn(u16) -> Option<u16>,
    is_identity: bool,
    domain_card: u32,
    codomain_card: u32,
) -> Vec<u16> {
    if is_identity && domain_card == codomain_card {
        return Vec::new();
    }
    (0..domain_card)
        .map(|s| {
            let Ok(s16) = u16::try_from(s) else {
                return 0xFFFF;
            };
            match apply(s16) {
                Some(v) if u32::from(v) < codomain_card => v,
                _ => 0xFFFF,
            }
        })
        .collect()
}

/// Rewrite one routine's blob: framed bound-call sites widen 5 → 9 bytes,
/// every following offset shifts by +4 per preceding widened site, and the
/// function's internal offsets (jump targets, relocation holes, table
/// fixups, debug labels/lines, dispatch entries, raw-descriptor exits)
/// remap through the same offset function. Returns the rewritten `FuncRef`
/// and the ordered list of framed-call sites it now carries (bound framed
/// sites + raw `call.m`), the sources of the compose columns.
fn rewrite_blob<'a>(
    syntax: &ArchSyntax,
    fc_opcode: u8,
    f: FuncRef<'a>,
    sites: &[SiteKind<'a>],
) -> Result<(FuncRef<'a>, Vec<FramedSite>), LinkError> {
    // Widening points: the addresses of framed (non-collapsed) bound sites.
    let mut widen_at: Vec<u32> = sites
        .iter()
        .filter_map(|s| match s {
            SiteKind::Bound {
                addr,
                collapse: false,
                ..
            } => Some(*addr),
            _ => None,
        })
        .collect();
    widen_at.sort_unstable();

    // Nothing to rewrite when no bound call touches this function: the blob,
    // its relocations, debug, and table stay exactly as resolved (borrowed).
    // Its raw `call.m` sites still surface as framed-call columns.
    let has_bound = sites.iter().any(|s| matches!(s, SiteKind::Bound { .. }));
    if !has_bound {
        let framed = raw_framed_sites(&f, sites)?;
        return Ok((f, framed));
    }

    // `new(old) = old + 4 * (framed sites strictly before old)` — total on
    // every offset, boundary or not, since the shift depends only on how
    // many widened sites precede it.
    let shift =
        |old: u32| -> u32 { old + 4 * widen_at.iter().filter(|&&a| a < old).count() as u32 };

    // Index bound sites for the emit walk.
    let bound_at: HashMap<u32, (usize, bool)> = sites
        .iter()
        .filter_map(|s| match s {
            SiteKind::Bound {
                addr,
                callee,
                collapse,
                ..
            } => Some((*addr, (*callee, *collapse))),
            _ => None,
        })
        .collect();
    let plain_hole: HashMap<u32, usize> = f.calls.iter().copied().collect();

    let orig: Vec<u8> = f.blob.to_vec();
    let mut new_blob = Vec::with_capacity(orig.len() + 4 * widen_at.len());
    let mut new_calls: Vec<(u32, usize)> = Vec::new();
    let mut framed: Vec<FramedSite> = Vec::new();

    for d in decode::decode_stream(syntax, &orig, 0, orig.len() as u32) {
        let addr = d.addr;
        let len = d.len;
        let new_addr = shift(addr);
        debug_assert_eq!(new_addr as usize, new_blob.len());

        match &d.body {
            Body::Raw(_) => {
                // A byte no instruction covers — copied verbatim; layout's
                // own decode will reject it if it truly is garbage.
                new_blob.extend_from_slice(&orig[addr as usize..(addr + len) as usize]);
            }
            Body::Instr { mnemonic, operand } => {
                let entry = syntax
                    .by_mnemonic(mnemonic)
                    .expect("mnemonic came from a successful decode");
                if let Some(&(callee, collapse)) = bound_at.get(&addr) {
                    if collapse {
                        // §5.6 collapse: keep the 5-byte call, promote the
                        // bound hole to a relocation — layout relaxes it.
                        new_blob.extend_from_slice(&orig[addr as usize..(addr + len) as usize]);
                        new_calls.push((new_addr + 1, callee));
                    } else {
                        // Widen to a framed call: opcode + 4-byte rel hole
                        // (relocated to the callee) + 4-byte frame half (the
                        // site index, written by layout).
                        new_blob.push(fc_opcode);
                        new_blob.extend_from_slice(&[0u8; 8]);
                        new_calls.push((new_addr + 1, callee));
                        framed.push(FramedSite::Bound { addr });
                    }
                    continue;
                }
                match (entry.flow, operand) {
                    (Flow::Call, DecodedOperand::FramedCall { table, .. }) => {
                        // A raw `call.m`: copy verbatim (both halves are
                        // overwritten downstream — rel by the relocation,
                        // frame by the site index) and relocate the rel half.
                        new_blob.extend_from_slice(&orig[addr as usize..(addr + len) as usize]);
                        let callee =
                            *plain_hole
                                .get(&(addr + 1))
                                .ok_or(LinkError::MalformedBlob {
                                    symbol: f.name.to_string(),
                                    at: addr + 1,
                                })?;
                        new_calls.push((new_addr + 1, callee));
                        framed.push(FramedSite::Raw { table_off: *table });
                    }
                    (Flow::Call, DecodedOperand::RelTarget(_)) => {
                        // A plain relocated call: copy verbatim, re-relocate.
                        new_blob.extend_from_slice(&orig[addr as usize..(addr + len) as usize]);
                        if let Some(&callee) = plain_hole.get(&(addr + 1)) {
                            new_calls.push((new_addr + 1, callee));
                        }
                    }
                    (Flow::Jump | Flow::Branch, DecodedOperand::RelTarget(target)) => {
                        if let Some(&callee) = plain_hole.get(&(addr + 1)) {
                            // Relocated tail jump: copy verbatim, re-relocate.
                            new_blob.extend_from_slice(&orig[addr as usize..(addr + len) as usize]);
                            new_calls.push((new_addr + 1, callee));
                        } else {
                            // Intra-function jump: re-encode the displacement
                            // through the offset map so its decoded target is
                            // the target instruction's rewritten-blob offset.
                            let new_target = shift(*target);
                            let new_end = new_addr + len;
                            let off = i64::from(new_target) - i64::from(new_end);
                            new_blob.push(entry.opcode);
                            match len - 1 {
                                1 => {
                                    let off8 = i8::try_from(off).map_err(|_| {
                                        LinkError::MalformedBlob {
                                            symbol: f.name.to_string(),
                                            at: addr,
                                        }
                                    })?;
                                    new_blob.push(off8 as u8);
                                }
                                4 => new_blob.extend_from_slice(
                                    &i32::try_from(off)
                                        .expect("jump offset fits i32")
                                        .to_le_bytes(),
                                ),
                                _ => unreachable!("relative jump width is 1 or 4"),
                            }
                        }
                    }
                    _ => {
                        new_blob.extend_from_slice(&orig[addr as usize..(addr + len) as usize]);
                    }
                }
            }
        }
    }

    // Relocation order: layout wants call holes in blob order.
    new_calls.sort_by_key(|&(h, _)| h);

    // Shift the table fixups' code holes; the table offsets are unchanged.
    let new_fixups: Vec<(u32, u32)> = f
        .table_fixups
        .iter()
        .map(|&(hole, toff)| (shift(hole), toff))
        .collect();

    // Shift the code offsets held INSIDE the table (dispatch entries and
    // raw `.frame` descriptor exits are blob-relative — docs/formats.md
    // (frame descriptors)); match rows and descriptor maps are symbol data.
    let new_table = shift_table(syntax, &orig, &f.table, &f.table_fixups, f.name, &shift)?;

    // Shift debug label/line offsets.
    let new_debug = f.debug.as_deref().map(|d| {
        let labels = d
            .labels
            .iter()
            .map(|(n, off)| (n.clone(), shift(*off)))
            .collect();
        let lines = d.lines.iter().map(|(off, l)| (shift(*off), *l)).collect();
        crate::formats::object::BlobDebug { labels, lines }
    });

    Ok((
        FuncRef {
            name: f.name,
            blob: Cow::Owned(new_blob),
            debug: new_debug.map(Cow::Owned),
            calls: new_calls,
            bound: Vec::new(),
            table: Cow::Owned(new_table),
            table_fixups: new_fixups,
            signature: f.signature,
        },
        framed,
    ))
}

/// The raw `call.m` sites of a function that itself carries no bound call
/// (so its blob is not rewritten) — still framed-call columns, named by the
/// descriptor's table offset (the frame-half fixup).
fn raw_framed_sites(f: &FuncRef, sites: &[SiteKind]) -> Result<Vec<FramedSite>, LinkError> {
    let mut out = Vec::new();
    for s in sites {
        if let SiteKind::RawCallM { frame_hole, .. } = s {
            let table_off = f
                .table_fixups
                .iter()
                .find(|(h, _)| h == frame_hole)
                .map(|(_, t)| *t)
                .ok_or(LinkError::MalformedBlob {
                    symbol: f.name.to_string(),
                    at: *frame_hole,
                })?;
            out.push(FramedSite::Raw { table_off });
        }
    }
    Ok(out)
}

/// Copy a table blob, shifting the blob-relative code offsets it holds —
/// dispatch-table entries and frame-descriptor exit vectors — through
/// `shift`, leaving match rows and descriptor maps (symbol data) verbatim.
/// Mirrors the linker's table walk (docs/formats.md (executable image)); a
/// truncated or off-boundary table is a `MalformedTable`.
fn shift_table(
    syntax: &ArchSyntax,
    orig_blob: &[u8],
    table: &[u8],
    fixups: &[(u32, u32)],
    name: &str,
    shift: &impl Fn(u32) -> u32,
) -> Result<Vec<u8>, LinkError> {
    if table.is_empty() {
        return Ok(Vec::new());
    }
    use std::collections::BTreeMap;
    #[derive(Clone, Copy)]
    enum Kind {
        Match,
        Dispatch,
        Frame,
    }
    let malformed = |at: u32| LinkError::MalformedTable {
        symbol: name.to_string(),
        at,
    };
    // Classify each referenced table start by the opcode at its fixup hole.
    let mut starts: BTreeMap<u32, Kind> = BTreeMap::new();
    for &(hole, toff) in fixups {
        let kind = if hole
            .checked_sub(1)
            .and_then(|p| orig_blob.get(p as usize))
            .and_then(|&op| syntax.by_opcode(op))
            .is_some_and(|e| e.operand == OperandKind::TableRef)
        {
            let op = orig_blob[(hole - 1) as usize];
            if syntax.by_opcode(op).unwrap().flow == Flow::FallThrough {
                Kind::Match
            } else {
                Kind::Dispatch
            }
        } else if hole
            .checked_sub(5)
            .and_then(|p| orig_blob.get(p as usize))
            .and_then(|&op| syntax.by_opcode(op))
            .is_some_and(|e| e.operand == OperandKind::FramedCall)
        {
            Kind::Frame
        } else {
            Kind::Match
        };
        starts.entry(toff).or_insert(kind);
    }

    let len = table.len() as u32;
    let u16_at = |p: u32| u16::from_le_bytes([table[p as usize], table[p as usize + 1]]);
    let mut out = Vec::with_capacity(table.len());
    let mut pos = 0u32;
    for (&start, &kind) in &starts {
        if start != pos {
            return Err(malformed(pos.min(start)));
        }
        let end = match kind {
            Kind::Match => {
                if start + 3 > len {
                    return Err(malformed(start));
                }
                let width = u32::from(table[start as usize]);
                let count = u32::from(u16_at(start + 1));
                let end = start + 3 + width * count;
                if end > len {
                    return Err(malformed(start));
                }
                out.extend_from_slice(&table[start as usize..end as usize]);
                end
            }
            Kind::Dispatch => {
                if start + 2 > len {
                    return Err(malformed(start));
                }
                let count = u32::from(u16_at(start));
                let entries = start + 2;
                let end = entries + 4 * count;
                if end > len {
                    return Err(malformed(start));
                }
                out.extend_from_slice(&table[start as usize..entries as usize]);
                for e in 0..count {
                    let at = (entries + 4 * e) as usize;
                    let off = u32::from_le_bytes(table[at..at + 4].try_into().unwrap());
                    out.extend_from_slice(&shift(off).to_le_bytes());
                }
                end
            }
            Kind::Frame => shift_frame_descriptor(table, start, len, shift, &malformed, &mut out)?,
        };
        pos = end;
    }
    if pos != len {
        return Err(malformed(pos));
    }
    Ok(out)
}

/// Walk one frame descriptor (docs/formats.md (frame descriptors)), copying
/// its header and maps verbatim and shifting its exit vector's blob-relative
/// code offsets. Returns the descriptor's end offset.
fn shift_frame_descriptor(
    table: &[u8],
    start: u32,
    len: u32,
    shift: &impl Fn(u32) -> u32,
    malformed: &impl Fn(u32) -> LinkError,
    out: &mut Vec<u8>,
) -> Result<u32, LinkError> {
    let u16_at = |p: u32| -> Option<u32> {
        if p + 2 > len {
            return None;
        }
        Some(u32::from(u16::from_le_bytes([
            table[p as usize],
            table[p as usize + 1],
        ])))
    };
    let Some(&arity) = table.get(start as usize) else {
        return Err(malformed(start));
    };
    if arity == 0 || arity > 16 {
        return Err(malformed(start));
    }
    let mut pos = start + 1;
    let Some(exit_count) = u16_at(pos) else {
        return Err(malformed(start));
    };
    pos += 2;
    for _ in 0..arity {
        if pos >= len {
            return Err(malformed(start));
        }
        pos += 1; // phys
        for _ in 0..2 {
            let Some(map_len) = u16_at(pos) else {
                return Err(malformed(start));
            };
            pos += 2 + 2 * map_len;
        }
    }
    if pos > len {
        return Err(malformed(start));
    }
    out.extend_from_slice(&table[start as usize..pos as usize]);
    let end = pos + 4 * exit_count;
    if end > len {
        return Err(malformed(start));
    }
    for e in 0..exit_count {
        let at = (pos + 4 * e) as usize;
        let off = u32::from_le_bytes(table[at..at + 4].try_into().unwrap());
        out.extend_from_slice(&shift(off).to_le_bytes());
    }
    Ok(end)
}
