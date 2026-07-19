//! outline (state-graph form): the inverse of `inline`. Where inline dissolves
//! a call, outline HOISTS a repeated subgraph into a shared routine and replaces
//! each copy with a call. Program-level, default-OFF (gated by `--foutline` /
//! [`OptOptions::outline`](super::OptOptions), run AFTER inline so inline's
//! splices settle before sharing is sought). Part of the `-O1` pipeline
//! (optimizer/mod.rs).
//!
//! # What it folds
//!
//! Within ONE world, outline looks for **groups of ≥2 disjoint subgraphs that
//! are structurally identical modulo state renumbering** — the shape repeated
//! graft instances leave behind. A candidate subgraph must be:
//!
//! * **exit-free** — every internal rule is a `goto` (no `return`/`stop`/`halt`,
//!   no trap, no call): the hoisted routine returns through its escapes, so any
//!   terminator inside would change which frame ends;
//! * **single-junction** — exactly one ROOT (the only member with inbound edges
//!   from outside the subgraph) and exactly one external escape target, the
//!   JUNCTION every leaving edge converges on;
//! * **brk-free** — a `debugger` row anywhere in the subgraph refuses it, so no
//!   observable pause address is ever moved into a routine (the brk barrier);
//! * **large enough** — at least [`OUTLINE_MIN_STATES`] states (below that a
//!   group stays as written).
//!
//! # The fold
//!
//! One copy is hoisted into a new synthesized routine (`<world>.outline<n>` —
//! a `.`-separated name the `.tmc` grammar cannot mint, so it never collides
//! with a user routine, and one the `.tma` assembler accepts, since codegen
//! re-emits it). The routine's tapes mirror the host's verbatim; its entry is
//! the copied root; each escape (`goto` → junction) becomes a `return`. Every
//! occurrence's root is rewritten to a one-row trampoline — a **bindless** call
//! into the routine (a plain same-frame call, mode-independent, so the object
//! stays linkable under mono / frames / hybrid alike) resuming at that
//! occurrence's junction:
//! `[*] -> call <routine>() then goto <junction>`. The now-orphaned non-root
//! copies are left for `dce` to delete.
//!
//! # Canonical form and the near-miss
//!
//! Two subgraphs fold only when their canonical serializations are equal — a
//! BFS from the root assigns LOCAL ids, and each row is serialized with those
//! local ids, escapes marked as EXIT, and **the junction id in the key head**.
//! Because the junction is part of the key, two subgraphs with identical bodies
//! but DIFFERENT junction successors get different keys and never fold (they
//! would resume at different places). The serialization IS the bucket key, so
//! bucket membership is exact structural equality — there is no separate
//! collision-verify step. Complexity is `O(states²)` region growth per world
//! (a fixpoint grown from each root) plus `O(states)` canonicalization, which
//! the modest world sizes keep cheap.
//!
//! # Threshold disjointness with `inline` (both numbers, one place)
//!
//! `inline` splices a callee of at most `INLINE_MAX_RULES` (= 6) rows.
//! `OUTLINE_MIN_STATES` (= 7) is set strictly above it: a subgraph of ≥7 states
//! hoists to a routine of ≥7 rows (≥1 row per state), which exceeds inline's
//! 6-row cap, so inline can never re-splice what outline just hoisted. The two
//! passes therefore cannot ping-pong; the compile-time assertion below pins the
//! relationship, and the round cap (optimizer/mod.rs) backstops any other
//! interaction.

use std::collections::{HashMap, HashSet, VecDeque};

use crate::ir::{
    IrCell, IrDispatch, IrProgram, IrRule, IrState, IrThen, IrTransition, IrWorld, IrWorldKind,
};

/// Minimum state count for an outline candidate subgraph. Above
/// `inline::INLINE_MAX_RULES` so the two passes cannot ping-pong (see the module
/// doc for the full statement of both numbers).
const OUTLINE_MIN_STATES: usize = 7;
const _: () = assert!(OUTLINE_MIN_STATES > super::inline::INLINE_MAX_RULES);

/// A validated candidate subgraph in one world.
struct Region {
    /// The root state id (host id space); the only member with external inbound.
    root: u32,
    /// The single external escape target (host id space).
    junction: u32,
    /// Member state ids in BFS-from-root order (member[0] == root).
    members: Vec<u32>,
    /// The canonical serialization — the fold key (junction id included).
    key: Vec<u8>,
}

pub fn run(ir: &mut IrProgram) -> u32 {
    let orig_len = ir.worlds.len();
    let mut new_worlds: Vec<IrWorld> = Vec::new();
    let mut changes = 0u32;
    // Iterate the original worlds only; hoisted routines are appended after the
    // scan, so this run never rescans what it just created.
    for wi in 0..orig_len {
        changes += outline_world(&mut ir.worlds[wi], &mut new_worlds);
    }
    ir.worlds.extend(new_worlds);
    changes
}

/// Detect and hoist every foldable group in one world. Returns the number of
/// groups hoisted (one synthesized routine each).
fn outline_world(w: &mut IrWorld, new_worlds: &mut Vec<IrWorld>) -> u32 {
    let exitfree: Vec<bool> = w.states.iter().map(is_exit_free).collect();
    let dbg: Vec<bool> = w.states.iter().map(has_debugger).collect();
    let inbound = compute_inbound(w);

    // Grow a candidate region from every exit-free, brk-free root.
    let mut regions: Vec<Region> = Vec::new();
    for st in &w.states {
        let id = st.id;
        if exitfree[id as usize]
            && !dbg[id as usize]
            && let Some(region) = grow_region(w, id, &exitfree, &dbg, &inbound)
        {
            regions.push(region);
        }
    }
    if regions.len() < 2 {
        return 0;
    }

    // Bucket by the canonical key; a bucket with ≥2 regions is a fold group.
    let mut buckets: HashMap<Vec<u8>, Vec<usize>> = HashMap::new();
    for (i, reg) in regions.iter().enumerate() {
        buckets.entry(reg.key.clone()).or_default().push(i);
    }
    let mut bucket_list: Vec<(Vec<u8>, Vec<usize>)> =
        buckets.into_iter().filter(|(_, v)| v.len() >= 2).collect();
    // Largest subgraphs first (so an enclosing region claims its states before a
    // sub-region could), then key bytes for a deterministic order.
    bucket_list.sort_by(|a, b| {
        regions[b.1[0]]
            .members
            .len()
            .cmp(&regions[a.1[0]].members.len())
            .then_with(|| a.0.cmp(&b.0))
    });

    let mut used: HashSet<u32> = HashSet::new();
    let mut counter = 0u32;
    let mut changes = 0u32;
    for (_key, idxs) in bucket_list {
        // Select pairwise-disjoint regions that do not touch an already-claimed
        // state, deterministically by root id.
        let mut sorted = idxs;
        sorted.sort_by_key(|&i| regions[i].root);
        let mut selected: Vec<(u32, u32, Vec<u32>)> = Vec::new(); // (root, junction, members)
        let mut claimed: HashSet<u32> = HashSet::new();
        for i in sorted {
            let reg = &regions[i];
            if reg
                .members
                .iter()
                .any(|m| used.contains(m) || claimed.contains(m))
            {
                continue;
            }
            claimed.extend(reg.members.iter().copied());
            selected.push((reg.root, reg.junction, reg.members.clone()));
        }
        if selected.len() < 2 {
            continue;
        }

        // Hoist ONE copy (the first selected region) while the host is intact,
        // then trampoline every occurrence.
        let routine_name = format!("{}.outline{counter}", w.name);
        counter += 1;
        let routine = build_routine(w, &selected[0].2, &routine_name);
        new_worlds.push(routine);
        for (root, junction, members) in &selected {
            trampoline(w, *root, &routine_name, *junction);
            for &m in members {
                used.insert(m);
            }
        }
        changes += 1;
    }
    changes
}

/// True when every rule of `st` transfers with a plain `goto` (no terminator,
/// trap, or call) — the exit-free condition.
fn is_exit_free(st: &IrState) -> bool {
    st.rules
        .iter()
        .all(|r| matches!(r.transition, IrTransition::Goto { .. }))
}

/// True when any rule of `st` carries a `debugger` (`brk`) — the barrier veto.
fn has_debugger(st: &IrState) -> bool {
    st.rules.iter().any(|r| r.debugger)
}

/// Every intra-world reference to each state: the sources of `goto` and
/// `call … then goto` edges. Used to test the "all inbound within the region"
/// privacy condition during growth.
fn compute_inbound(w: &IrWorld) -> HashMap<u32, Vec<u32>> {
    let mut inbound: HashMap<u32, Vec<u32>> = HashMap::new();
    for st in &w.states {
        for r in &st.rules {
            match &r.transition {
                IrTransition::Goto { state }
                | IrTransition::CallThen {
                    then: IrThen::Goto { state },
                    ..
                } => inbound.entry(*state).or_default().push(st.id),
                _ => {}
            }
        }
    }
    inbound
}

/// Grow the maximal private exit-free region rooted at `root`, then validate the
/// single-junction / size / reachable-root conditions and canonicalize it.
/// Returns `None` if any condition fails (the pass is conservative — an
/// ambiguous shape is never folded).
fn grow_region(
    w: &IrWorld,
    root: u32,
    exitfree: &[bool],
    dbg: &[bool],
    inbound: &HashMap<u32, Vec<u32>>,
) -> Option<Region> {
    let entry = w.entry;
    let mut region: HashSet<u32> = HashSet::new();
    region.insert(root);

    // Fixpoint: a state joins iff it is a goto target of the region, is not the
    // world entry (which carries an implicit external inbound), is exit-free and
    // brk-free, and ALL its inbound edges originate within the region (it is
    // private to the region). Mutually-recursive non-root cycles never satisfy
    // this and are left unfolded — conservative but always sound.
    loop {
        let mut targets: Vec<u32> = Vec::new();
        for &id in &region {
            for r in &w.states[id as usize].rules {
                if let IrTransition::Goto { state } = &r.transition {
                    targets.push(*state);
                }
            }
        }
        let mut added = false;
        for t in targets {
            if region.contains(&t) || t == root || t == entry {
                continue;
            }
            if !exitfree[t as usize] || dbg[t as usize] {
                continue;
            }
            let ins = inbound.get(&t).map(Vec::as_slice).unwrap_or(&[]);
            if ins.iter().all(|src| region.contains(src)) {
                region.insert(t);
                added = true;
            }
        }
        if !added {
            break;
        }
    }

    // Escapes: goto targets outside the region. Exactly one → the junction.
    let mut escapes: HashSet<u32> = HashSet::new();
    for &id in &region {
        for r in &w.states[id as usize].rules {
            if let IrTransition::Goto { state } = &r.transition
                && !region.contains(state)
            {
                escapes.insert(*state);
            }
        }
    }
    if escapes.len() != 1 {
        return None;
    }
    let junction = *escapes.iter().next().unwrap();
    if region.len() < OUTLINE_MIN_STATES {
        return None;
    }

    // The root must be reachable from outside the region (else the region is
    // dead — nothing to fold usefully).
    let root_ins = inbound.get(&root).map(Vec::as_slice).unwrap_or(&[]);
    let root_reachable = entry == root || root_ins.iter().any(|src| !region.contains(src));
    if !root_reachable {
        return None;
    }

    let (members, key) = canonicalize(w, root, &region, junction)?;
    Some(Region {
        root,
        junction,
        members,
        key,
    })
}

/// BFS the region from its root assigning local ids, then serialize every state
/// with those local ids (escapes marked EXIT, junction id in the key head).
/// Returns the members in BFS order and the canonical key, or `None` if the
/// region is not fully connected from the root (defensive — growth should
/// guarantee it).
fn canonicalize(
    w: &IrWorld,
    root: u32,
    region: &HashSet<u32>,
    junction: u32,
) -> Option<(Vec<u32>, Vec<u8>)> {
    let mut local: HashMap<u32, u32> = HashMap::new();
    let mut order: Vec<u32> = Vec::new();
    let mut queue: VecDeque<u32> = VecDeque::new();
    local.insert(root, 0);
    order.push(root);
    queue.push_back(root);
    while let Some(id) = queue.pop_front() {
        for r in &w.states[id as usize].rules {
            if let IrTransition::Goto { state } = &r.transition
                && region.contains(state)
                && !local.contains_key(state)
            {
                let l = local.len() as u32;
                local.insert(*state, l);
                order.push(*state);
                queue.push_back(*state);
            }
        }
    }
    if local.len() != region.len() {
        return None;
    }

    let mut key: Vec<u8> = Vec::new();
    key.push(b'J');
    key.extend_from_slice(&junction.to_le_bytes());
    key.extend_from_slice(&(order.len() as u32).to_le_bytes());
    for &id in &order {
        serialize_state(&mut key, &w.states[id as usize], &local);
    }
    Some((order, key))
}

/// Serialize one region state into the canonical key: dispatch hint, then each
/// row's pattern / write / moves / synthesized flag / transition. A `goto` to a
/// region member serializes its LOCAL id; an escape serializes an EXIT marker
/// (the junction id is fixed in the key head). Region states are exit-free, so
/// no other transition kind appears.
fn serialize_state(key: &mut Vec<u8>, st: &IrState, local: &HashMap<u32, u32>) {
    key.push(match st.dispatch {
        IrDispatch::Table => 0,
        IrDispatch::Branch => 1,
    });
    key.extend_from_slice(&(st.rules.len() as u32).to_le_bytes());
    for r in &st.rules {
        key.extend_from_slice(&(r.pattern.len() as u32).to_le_bytes());
        for c in &r.pattern {
            match c {
                IrCell::Wildcard => key.push(0),
                IrCell::Index { index } => {
                    key.push(1);
                    key.extend_from_slice(&index.to_le_bytes());
                }
            }
        }
        match &r.write {
            None => key.push(0),
            Some(v) => {
                key.push(1);
                key.extend_from_slice(&(v.len() as u32).to_le_bytes());
                for wc in v {
                    match wc {
                        crate::ir::IrWrite::Keep => key.push(0),
                        crate::ir::IrWrite::Index { index } => {
                            key.push(1);
                            key.extend_from_slice(&index.to_le_bytes());
                        }
                    }
                }
            }
        }
        match &r.moves {
            None => key.push(0),
            Some(v) => {
                key.push(1);
                key.extend_from_slice(&(v.len() as u32).to_le_bytes());
                for m in v {
                    key.push(match m {
                        crate::ir::IrMove::Left => 0,
                        crate::ir::IrMove::Right => 1,
                        crate::ir::IrMove::Stay => 2,
                    });
                }
            }
        }
        key.push(r.synthesized as u8);
        match &r.transition {
            IrTransition::Goto { state } => match local.get(state) {
                Some(&l) => {
                    key.push(0);
                    key.extend_from_slice(&l.to_le_bytes());
                }
                None => key.push(1), // EXIT — to the junction fixed in the key head
            },
            _ => unreachable!("region states are exit-free"),
        }
    }
}

/// Build the hoisted routine from one occurrence's members (in BFS order): copy
/// each state with a local dense id, rewrite internal `goto`s to local ids, and
/// rewrite each escape (`goto` → junction) to a `return`. Tapes mirror the host
/// verbatim; the routine is `local` (hidden).
fn build_routine(w: &IrWorld, members: &[u32], name: &str) -> IrWorld {
    let local: HashMap<u32, u32> = members
        .iter()
        .enumerate()
        .map(|(l, &id)| (id, l as u32))
        .collect();
    let mut states = Vec::with_capacity(members.len());
    for (l, &id) in members.iter().enumerate() {
        let mut st = w.states[id as usize].clone();
        st.id = l as u32;
        for r in &mut st.rules {
            match &r.transition {
                IrTransition::Goto { state } => {
                    r.transition = match local.get(state) {
                        Some(&ln) => IrTransition::Goto { state: ln },
                        None => IrTransition::Return, // escape → the routine returns
                    };
                }
                _ => unreachable!("region states are exit-free"),
            }
        }
        states.push(st);
    }
    IrWorld {
        name: name.to_string(),
        kind: IrWorldKind::Routine,
        arity: w.arity,
        tapes: w.tapes.clone(),
        entry: 0,
        states,
        local: true,
        line: 0,
    }
}

/// Replace an occurrence's root with a one-row trampoline: a bindless call into
/// the hoisted routine resuming at the occurrence's junction. The non-root
/// copies are left orphaned for `dce`.
fn trampoline(w: &mut IrWorld, root: u32, routine_name: &str, junction: u32) {
    let arity = w.arity as usize;
    let pos = root as usize; // dense ids: id == position
    w.states[pos].rules = vec![IrRule {
        pattern: vec![IrCell::Wildcard; arity],
        write: None,
        moves: None,
        debugger: false,
        transition: IrTransition::CallThen {
            target: routine_name.to_string(),
            binding: vec![],
            then: IrThen::Goto { state: junction },
        },
        synthesized: false,
        line: 0,
    }];
    w.states[pos].dispatch = IrDispatch::Table;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compiler::analyze;
    use crate::expand::expand;
    use crate::ir::{lower, validate_world};

    fn ir_of(src: &str) -> IrProgram {
        let a = analyze(src).unwrap_or_else(|e| panic!("analyze: {e}"));
        let ex = expand(&a.resolved).unwrap_or_else(|e| panic!("expand: {e}"));
        lower(&ex, &a.resolved)
            .unwrap_or_else(|e| panic!("lower: {e}"))
            .0
    }

    /// A machine with two `len`-state exit-free chains — the 'a' branch escapes
    /// to `junction_a`, the 'b' branch to `junction_b` — plus the terminals. The
    /// chains are structurally identical; they fold iff the junctions match.
    fn two_chain_program(len: usize, junction_a: &str, junction_b: &str) -> String {
        let mut s = String::from("alphabet ab { '_', 'a', 'b' }\nmachine {\n  tape t: ab;\n");
        s.push_str("  entry state start { ['a'] -> goto a0; ['b'] -> goto b0; [*] -> stop; }\n");
        for (pfx, junc) in [("a", junction_a), ("b", junction_b)] {
            for i in 0..len {
                if i + 1 < len {
                    s.push_str(&format!(
                        "  state {pfx}{i} {{ [*] -> move [>] goto {pfx}{next}; }}\n",
                        next = i + 1
                    ));
                } else {
                    s.push_str(&format!("  state {pfx}{i} {{ [*] -> goto {junc}; }}\n"));
                }
            }
        }
        s.push_str("  state mid { [*] -> stop; }\n");
        s.push_str("  state alt { [*] -> halt; }\n");
        s.push_str("}\n");
        s
    }

    #[test]
    fn two_identical_regions_fold_into_one_shared_routine() {
        // Both 7-state chains escape to `mid` — identical bodies, same junction,
        // so they fold into ONE routine and both roots become trampolines.
        let mut ir = ir_of(&two_chain_program(7, "mid", "mid"));
        let before = ir.worlds.len();
        assert_eq!(run(&mut ir), 1, "one group hoisted");
        assert_eq!(ir.worlds.len(), before + 1, "one routine synthesized");

        let routine = ir
            .worlds
            .iter()
            .find(|w| w.name == "main.outline0")
            .expect("the hoisted routine");
        assert_eq!(routine.kind, IrWorldKind::Routine);
        assert!(routine.local);
        assert_eq!(routine.states.len(), 7, "the 7-state body was hoisted");
        assert_eq!(routine.entry, 0);
        // The routine's last (escape) state now returns.
        assert_eq!(routine.states[6].rules[0].transition, IrTransition::Return);
        // The rest goto the next local id.
        assert_eq!(
            routine.states[0].rules[0].transition,
            IrTransition::Goto { state: 1 }
        );
        validate_world(routine).unwrap();

        // Both a0 and b0 are now one-row bindless-call trampolines resuming at
        // `mid`.
        let main = ir.worlds.iter().find(|w| w.name == "main").unwrap();
        let mid_id = main.states.iter().find(|s| s.name == "mid").unwrap().id;
        let trampolines: Vec<&IrState> = main
            .states
            .iter()
            .filter(|s| {
                matches!(
                    s.rules.first().map(|r| &r.transition),
                    Some(IrTransition::CallThen { target, binding, then })
                        if target == "main.outline0"
                            && binding.is_empty()
                            && *then == IrThen::Goto { state: mid_id }
                )
            })
            .collect();
        assert_eq!(
            trampolines.len(),
            2,
            "both occurrences trampolined to `mid`"
        );
        validate_world(main).unwrap();
    }

    #[test]
    fn a_differing_junction_does_not_fold() {
        // Same two 7-state chains, but the 'b' chain escapes to `alt` instead of
        // `mid`. Different junctions → different canonical keys → NO fold.
        let mut ir = ir_of(&two_chain_program(7, "mid", "alt"));
        let before = ir.worlds.len();
        assert_eq!(run(&mut ir), 0, "differing junctions never fold");
        assert_eq!(ir.worlds.len(), before, "no routine synthesized");
    }

    #[test]
    fn a_group_below_the_state_threshold_stays() {
        // Two identical chains, but only 6 states each (< OUTLINE_MIN_STATES) —
        // below threshold, so they are left as written.
        let mut ir = ir_of(&two_chain_program(6, "mid", "mid"));
        assert_eq!(run(&mut ir), 0, "sub-threshold groups do not fold");
    }

    #[test]
    fn a_brk_inside_a_region_refuses_the_fold() {
        // A `debugger` on one chain's states makes that subgraph non-candidate,
        // so the group drops below two and nothing folds — the brk barrier.
        let mut src = two_chain_program(7, "mid", "mid");
        // Put a brk on `a3` (a member of the 'a' chain).
        src = src.replace(
            "  state a3 { [*] -> move [>] goto a4; }\n",
            "  state a3 { [*] -> debugger move [>] goto a4; }\n",
        );
        let mut ir = ir_of(&src);
        assert_eq!(run(&mut ir), 0, "a brk in a region blocks the fold");
        assert!(
            ir.worlds
                .iter()
                .any(|w| w.states.iter().any(|s| s.rules.iter().any(|r| r.debugger))),
            "the brk row survives"
        );
    }
}
