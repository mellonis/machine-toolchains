//! The canonical binding-label renderer (docs/formats.md (binding labels)).
//! One notation, shared by the map sidecar's `bindings` records and `tmt
//! dis`'s frames legend, so a composite reads the same everywhere.
//!
//! ## Grammar
//!
//! ```text
//! label = name "@[" entry ("," entry)* "]"
//! entry = physIdx [ "{" pairs "}" ]          ; list position = virtual tape
//! pairs = pair ("," pair)*                    ; decimal, sorted by src
//! pair  = src "->" dst | src "=>" dst         ; => marks a one-way (read-only) pair
//! ```
//!
//! - equal-size (a completed bijection): identity pairs and the empty `{}`
//!   are omitted;
//! - holey (unequal-size): ALL mapped pairs are listed — identity completion
//!   does not exist across differently-sized alphabets, so an absent src IS a
//!   hole (no collision with the identity-omission rule);
//! - the blank pair `0->0` is never written;
//! - more than 8 displayed pairs collapse to a digest `{#xxxxxxxx}` — see
//!   [`entry_digest`].
//!
//! ## Working form
//!
//! The renderer takes each virtual tape's DENSE descriptor maps (`rmap`
//! indexed by physical symbol, `wmap` by virtual symbol, `0xFFFF` = hole) —
//! the exact wire form in the frames region's descriptors. This is the one
//! representation both producers already hold: the composition engine
//! materializes it for the descriptor bytes, and `tmt dis` decodes it back
//! from the image. Rendering from it (rather than a sparse `Composite`) makes
//! the sidecar label and the map-less dis label byte-identical for free, and
//! it is the only form that carries the cardinality-truncation holes a sparse
//! map elides — the holey case needs them to distinguish a hole from an
//! in-range identity.

use std::collections::HashMap;

use super::{MapBinding, MapBindingTape};
use crate::formats::crc32::crc32;

/// The `0xFFFF` dense-map hole sentinel (docs/formats.md (frame descriptors)).
const HOLE: u16 = 0xFFFF;

/// A displayed-pair budget: past this many pairs an entry renders as a digest
/// content address instead (docs/formats.md (binding labels)).
const MAX_PAIRS: usize = 8;

/// One virtual tape's dense descriptor maps, borrowed for rendering. `rmap`
/// is indexed by physical symbol and yields the virtual symbol (or `HOLE`);
/// `wmap` is indexed by virtual symbol and yields the physical symbol. An
/// empty map is the identity over an equal-size alphabet (the descriptor's
/// own encoding — docs/formats.md (frame descriptors)).
pub(crate) struct LabelTape<'a> {
    pub phys: u8,
    pub rmap: &'a [u16],
    pub wmap: &'a [u16],
}

/// Render `routine`'s composite as `name@[entry, …]` (docs/formats.md
/// (binding labels)). One entry per virtual tape, in tape order.
pub(crate) fn label(routine: &str, tapes: &[LabelTape]) -> String {
    // Entries join with a bare comma — no space — per the canonical grammar
    // (docs/formats.md (binding labels)); the same separator the pair list
    // inside `{…}` already uses, so the whole label is space-free.
    let entries = tapes.iter().map(entry).collect::<Vec<_>>().join(",");
    format!("{routine}@[{entries}]")
}

/// Append a deterministic `.2`, `.3`, … suffix to every label that collides
/// with an earlier one, in the given (directory) order, so one image's label
/// set has no display collisions (docs/formats.md (binding labels)). The
/// first occurrence stays bare; semantics always come from the structured
/// record, never the disambiguated string.
pub(crate) fn disambiguate(labels: &mut [String]) {
    let mut seen: HashMap<String, u32> = HashMap::new();
    for l in labels.iter_mut() {
        let n = seen.entry(l.clone()).or_insert(0);
        *n += 1;
        if *n > 1 {
            *l = format!("{l}.{n}");
        }
    }
}

/// Decode one tape's dense maps into the structured sidecar record
/// (docs/formats.md (sidecar bindings)): the non-identity read pairs (`src`,
/// `dst`, `one_way`) plus the explicit read/write hole sets. Identity read
/// pairs are implicit; a `one_way` pair is one whose write map carries no
/// inverse.
#[allow(clippy::type_complexity)]
pub(crate) fn tape_record(
    rmap: &[u16],
    wmap: &[u16],
) -> (Vec<(u32, u32, bool)>, Vec<u32>, Vec<u32>) {
    let mut pairs = Vec::new();
    let mut read_holes = Vec::new();
    for (s, &d) in rmap.iter().enumerate() {
        let s16 = s as u16;
        if d == HOLE {
            read_holes.push(s as u32);
        } else if d != s16 {
            pairs.push((s as u32, u32::from(d), !is_two_way(wmap, d, s16)));
        }
    }
    let write_holes = wmap
        .iter()
        .enumerate()
        .filter(|&(_, &p)| p == HOLE)
        .map(|(v, _)| v as u32)
        .collect();
    (pairs, read_holes, write_holes)
}

/// A read pair `src -> dst` is two-way when the write map sends `dst` back to
/// `src`; an empty or short write map is the identity, so an off-diagonal
/// read pair is then one-way.
fn is_two_way(wmap: &[u16], dst: u16, src: u16) -> bool {
    wmap.get(usize::from(dst)).copied().unwrap_or(dst) == src
}

/// One decoded frame descriptor's dense per-tape maps (docs/formats.md (frame
/// descriptors)): `(phys, rmap, wmap)` per virtual tape. Exits are code
/// offsets irrelevant to a binding label and are skipped.
type DenseTape = (u8, Vec<u16>, Vec<u16>);

/// Decode a frame descriptor at `offset` in a table section into its dense
/// per-tape maps (docs/formats.md (frame descriptors)). `None` on truncation
/// (the linker never emits one). The exit vector is walked past but dropped.
fn decode_descriptor(tables: &[u8], offset: u32) -> Option<Vec<DenseTape>> {
    let mut pos = offset as usize;
    let arity = *tables.get(pos)?;
    pos += 1;
    // exit_count (skipped — a label does not need the exits).
    let _exit_count = u16::from_le_bytes([*tables.get(pos)?, *tables.get(pos + 1)?]);
    pos += 2;
    let read_map = |pos: &mut usize| -> Option<Vec<u16>> {
        let len = u16::from_le_bytes([*tables.get(*pos)?, *tables.get(*pos + 1)?]) as usize;
        *pos += 2;
        let mut m = Vec::with_capacity(len);
        for _ in 0..len {
            m.push(u16::from_le_bytes([
                *tables.get(*pos)?,
                *tables.get(*pos + 1)?,
            ]));
            *pos += 2;
        }
        Some(m)
    };
    let mut tapes = Vec::with_capacity(arity as usize);
    for _ in 0..arity {
        let phys = *tables.get(pos)?;
        pos += 1;
        let rmap = read_map(&mut pos)?;
        let wmap = read_map(&mut pos)?;
        tapes.push((phys, rmap, wmap));
    }
    Some(tapes)
}

/// Build the map sidecar's binding records (docs/formats.md (sidecar
/// bindings)) from a linked image's table section: decode every directory
/// descriptor for its structure, pair it with the callee routine names
/// threaded in directory order, render the canonical label, and disambiguate
/// display collisions across the whole set. Empty when the image carries no
/// frames region.
pub(crate) fn build_bindings(
    tables: &[u8],
    frames_offset: u32,
    routines: &[String],
) -> Vec<MapBinding> {
    if frames_offset == 0 {
        return Vec::new();
    }
    let base = frames_offset as usize;
    let Some(k_bytes) = tables.get(base..base + 2) else {
        return Vec::new();
    };
    let k = u16::from_le_bytes(k_bytes.try_into().unwrap()) as usize;
    // Directory: K descriptor offsets, right after the K/S header.
    let dir_base = base + 4;
    let mut records: Vec<MapBinding> = Vec::with_capacity(k);
    let mut labels: Vec<String> = Vec::with_capacity(k);
    for i in 0..k {
        let at = dir_base + i * 4;
        let Some(off_bytes) = tables.get(at..at + 4) else {
            break;
        };
        let desc_off = u32::from_le_bytes(off_bytes.try_into().unwrap());
        let Some(dense) = decode_descriptor(tables, desc_off) else {
            break;
        };
        let routine = routines.get(i).cloned().unwrap_or_default();
        let tapes: Vec<LabelTape> = dense
            .iter()
            .map(|(phys, rmap, wmap)| LabelTape {
                phys: *phys,
                rmap,
                wmap,
            })
            .collect();
        labels.push(label(&routine, &tapes));
        let map_tapes = dense
            .iter()
            .map(|(phys, rmap, wmap)| {
                let (pairs, read_holes, write_holes) = tape_record(rmap, wmap);
                MapBindingTape {
                    phys: *phys,
                    pairs,
                    read_holes,
                    write_holes,
                }
            })
            .collect();
        records.push(MapBinding {
            index: u16::try_from(i + 1).unwrap_or(u16::MAX),
            routine,
            label: String::new(), // filled after disambiguation
            tapes: map_tapes,
        });
    }
    disambiguate(&mut labels);
    for (rec, label) in records.iter_mut().zip(labels) {
        rec.label = label;
    }
    records
}

/// Whether a tape is holey (unequal-size). An explicit hole in either map is
/// decisive; otherwise an empty map pins its two cardinalities equal (that
/// direction is the identity over one alphabet), so only two non-empty maps
/// of different lengths are holey.
fn is_holey(t: &LabelTape) -> bool {
    if t.rmap.contains(&HOLE) || t.wmap.contains(&HOLE) {
        return true;
    }
    match (t.rmap.len(), t.wmap.len()) {
        (0, _) | (_, 0) => false,
        (p, v) => p != v,
    }
}

/// One `physIdx[{pairs}]` entry.
fn entry(t: &LabelTape) -> String {
    let holey = is_holey(t);
    let mut pairs: Vec<(u16, u16, bool)> = Vec::new();
    for (s, &d) in t.rmap.iter().enumerate() {
        let s16 = s as u16;
        if d == HOLE || s16 == 0 {
            // A hole is shown by absence; the blank pair is never written.
            continue;
        }
        if !holey && s16 == d {
            // Identity is implicit in a completed bijection.
            continue;
        }
        pairs.push((s16, d, !is_two_way(t.wmap, d, s16)));
    }
    if pairs.is_empty() {
        return t.phys.to_string();
    }
    if pairs.len() > MAX_PAIRS {
        return format!("{}{{#{:08x}}}", t.phys, entry_digest(t));
    }
    let inner = pairs
        .iter()
        .map(|&(s, d, ow)| format!("{s}{}{d}", if ow { "=>" } else { "->" }))
        .collect::<Vec<_>>()
        .join(",");
    format!("{}{{{inner}}}", t.phys)
}

/// The CRC-32 content address for an over-budget entry (docs/formats.md
/// (binding labels)): the container checksum ([`crc32`]) of the tape's
/// COMPLETED dense maps — a length-prefixed serialization of `rmap` then
/// `wmap`. It reuses the same checksum algorithm the composition algebra's
/// composite digest uses; the input differs deliberately — this is the
/// per-tape completed (dense) map, not the per-composite sparse
/// `canonical_key`, because the grammar's digest is per-entry and must come
/// out identical whether computed from a live composite or re-derived from
/// the image's descriptor bytes. The digest is a content address matched like
/// a short hash, never decoded.
fn entry_digest(t: &LabelTape) -> u32 {
    let mut buf = Vec::with_capacity(8 + 2 * (t.rmap.len() + t.wmap.len()));
    buf.extend_from_slice(&(t.rmap.len() as u32).to_le_bytes());
    for &v in t.rmap {
        buf.extend_from_slice(&v.to_le_bytes());
    }
    buf.extend_from_slice(&(t.wmap.len() as u32).to_le_bytes());
    for &v in t.wmap {
        buf.extend_from_slice(&v.to_le_bytes());
    }
    crc32(&buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tape<'a>(phys: u8, rmap: &'a [u16], wmap: &'a [u16]) -> LabelTape<'a> {
        LabelTape { phys, rmap, wmap }
    }

    #[test]
    fn empty_maps_are_a_bare_phys_index() {
        // A full pass-through tape: identity, equal-size — no braces at all.
        assert_eq!(label("f", &[tape(0, &[], &[])]), "f@[0]");
        assert_eq!(
            label("f", &[tape(2, &[], &[]), tape(0, &[], &[])]),
            "f@[2,0]"
        );
    }

    #[test]
    fn equal_size_bijection_omits_identity_pairs() {
        // rmap present, equal-size (wmap empty pins V==P): a swap of 1<->2
        // over a 4-symbol alphabet shows only the off-diagonal pairs; the
        // identity entries 0, 3 are omitted.
        let rmap = [0, 2, 1, 3];
        let wmap = [0, 2, 1, 3];
        assert_eq!(label("f", &[tape(1, &rmap, &wmap)]), "f@[1{1->2,2->1}]");
    }

    #[test]
    fn one_way_pair_renders_with_a_fat_arrow() {
        // read 3 -> 1 with no write inverse (wmap identity) is one-way.
        let rmap = [0, 1, 2, 1]; // 3 reads as 1; a collapse
        let wmap = []; // identity write => the 3->1 read is one-way
        assert_eq!(label("g", &[tape(0, &rmap, &wmap)]), "g@[0{3=>1}]");
    }

    #[test]
    fn holey_tape_lists_all_mapped_pairs_including_identity() {
        // Unequal-size: physical alphabet 0..4, virtual 0..2. Physical 0,1
        // map identity (in range), 2 and 3 read as holes (out of the virtual
        // alphabet). Holey mode lists the in-range identity 1->1 and drops
        // blank; the holes 2, 3 are shown by absence.
        let rmap = [0, 1, HOLE, HOLE];
        let wmap = [0, 1];
        assert_eq!(label("h", &[tape(0, &rmap, &wmap)]), "h@[0{1->1}]");
    }

    #[test]
    fn blank_pair_is_never_written() {
        // A collapse-onto-blank read (3 => 0) plus the pinned 0->0: only the
        // collapse shows; the blank pair is dropped in every mode.
        let rmap = [0, 1, 2, 0]; // 3 reads as blank
        let wmap = []; // one-way collapse (no write inverse)
        assert_eq!(label("k", &[tape(0, &rmap, &wmap)]), "k@[0{3=>0}]");
    }

    #[test]
    fn over_budget_entry_collapses_to_a_digest() {
        // Nine off-diagonal pairs (a rotation of a 10-symbol alphabet) exceeds
        // the 8-pair budget, so the entry is the content-address digest.
        let rmap: Vec<u16> = (0..10u16).map(|s| (s + 1) % 10).collect();
        let mut wmap = vec![0u16; 10];
        for s in 0..10u16 {
            wmap[usize::from((s + 1) % 10)] = s;
        }
        let text = label("big", &[tape(0, &rmap, &wmap)]);
        assert!(
            text.starts_with("big@[0{#") && text.ends_with("}]"),
            "{text}"
        );
        // Deterministic and content-addressed: the same maps digest the same.
        assert_eq!(text, label("big", &[tape(0, &rmap, &wmap)]));
    }

    #[test]
    fn digest_is_stable_and_distinguishes_content() {
        let ra: Vec<u16> = (0..10u16).map(|s| (s + 1) % 10).collect();
        let rb: Vec<u16> = (0..10u16).map(|s| (s + 2) % 10).collect();
        let t_a = tape(0, &ra, &[]);
        let t_b = tape(0, &rb, &[]);
        assert_eq!(entry_digest(&t_a), entry_digest(&t_a));
        assert_ne!(entry_digest(&t_a), entry_digest(&t_b));
    }

    #[test]
    fn disambiguate_suffixes_collisions_deterministically() {
        let mut labels = vec![
            "f@[0]".to_string(),
            "g@[1]".to_string(),
            "f@[0]".to_string(),
            "f@[0]".to_string(),
        ];
        disambiguate(&mut labels);
        assert_eq!(
            labels,
            vec!["f@[0]", "g@[1]", "f@[0].2", "f@[0].3"],
            "first bare, later collisions get .2/.3"
        );
    }

    #[test]
    fn tape_record_is_sparse_truth_with_holes() {
        // rmap over physical 0..5: 1->2 (two-way — wmap sends 2 back to 1),
        // 2->2 identity (elided), 3=>1 (one-way — no write inverse), 4 a read
        // hole. wmap over virtual 0..4: 2->1, virtual 3 a write hole.
        let rmap = [0, 2, 2, 1, HOLE];
        let wmap = [0, 0, 1, HOLE];
        let (pairs, read_holes, write_holes) = tape_record(&rmap, &wmap);
        assert_eq!(read_holes, vec![4]);
        assert_eq!(write_holes, vec![3]);
        // Only the two non-identity read pairs, with correct one-way flags.
        assert_eq!(pairs, vec![(1, 2, false), (3, 1, true)]);
    }
}
