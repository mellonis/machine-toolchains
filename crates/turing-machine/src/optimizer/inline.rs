//! inline (state-graph form): splice a small leaf routine into a call site,
//! program-level, run FIRST in the round (optimizer/mod.rs). Dissolving the
//! call barrier is what lets the per-world passes see across the old boundary.
//! Part of the `-O1` pipeline.
//!
//! # Which calls splice
//!
//! A `call target(binding) then cont` is a splice site only when the binding is
//! a genuine **full pass-through** into an in-unit callee — no symbol map, no
//! hole ever owed, identity tape placement (docs/formats.md (bound calls)).
//! Three forms qualify:
//!
//! * a **bindless** call (empty binding) — a plain same-frame call: callee tape
//!   `k` IS caller tape `k`, no maps, no traps ever owed. The front end never
//!   mints one in-unit for a tape-bearing routine (it always binds), so the
//!   only bindless in-unit calls are the ones `outline` synthesizes; inline
//!   must still handle them, since it runs after outline in the round. The
//!   guard still checks the bound prefix's per-tape cardinalities match the
//!   caller's — the outline trampoline shares the caller's exact signature, so
//!   it stays trivially eligible, but a future bindless producer with a
//!   narrower or wider callee could not splice out-of-range symbol indices.
//! * an **equal-arity full-passthrough bound** call — identity placement
//!   (callee tape `k` drawn from caller tape `k`), the whole-alphabet identity
//!   map (an EMPTY pair list, hole-free by construction), and equal per-tape
//!   cardinalities (so no read/write hole is ever owed).
//! * an **arity-reducing identity projection** — the same identity placement
//!   and empty maps, but a callee of SMALLER arity than the caller (callee tape
//!   `k` drawn from caller tape `k` for `k < callee.arity`; the caller's extra
//!   tapes stay unbound). Each spliced row widens to the caller's arity with
//!   wildcard / keep / stay on those unbound tapes — the identity on them — so
//!   the splice reads and writes nothing there.
//!
//! An explicit map — even all-identity pairs — is conservatively refused on
//! either bound form: a partial pair list encodes cardinality-truncation holes
//! the materialized descriptor traps on, and the compose engine, not this pass,
//! is the authority on those.
//!
//! # Relation to the engine's collapse — a SOUND SUPERSET, not a twin
//!
//! This pass's splice decision is a sound SUPERSET of the linker composition
//! engine's `is_full_passthrough` collapse (crates/core compose.rs). The two
//! AGREE on the equal-arity forms — a same-arity full pass-through the engine
//! would lower to a plain `call` is exactly what inline splices, and an
//! opt-equivalence fixture compiles such a program and asserts no call survives.
//! They DIVERGE in the arity dimension: the engine's collapse demands equal
//! arity (its per-tape cardinality vectors compare unequal on different lengths
//! and refuse), so it deliberately keeps an arity-reducing projection as a
//! framed call — but inline splices it anyway. The splice is sound regardless
//! of arity: equal per-tape cardinalities on the bound prefix owe no read/write
//! hole, identity placement needs no per-rule remap, and the wildcard/keep/stay
//! padding is the identity on every unbound tape. The projection path is proven
//! behaviorally in the opt-equivalence matrix — a projecting call composed
//! framed at `-O0`, spliced at `-O1`, with observably identical runs (including
//! trap kinds, and an unbound tape's data left untouched) across mono / frames
//! / hybrid.
//!
//! A **permuted** projection (callee tape `k` drawn from caller tape `p(k)`,
//! `p ≠ identity`) is map-free and would be sound to splice with a per-rule
//! tape-index permutation, but this pass demands identity placement and leaves
//! it out; identity placement is the whole of the projection inline takes.
//!
//! # Callee eligibility
//!
//! A candidate callee is a **routine** (never the machine — the entry world's
//! terminators are `stp`/`hlt`, and nothing calls it), a **leaf** (no
//! `CallThen`/`TailCall` in its body — first-iteration simplicity, so a splice
//! never drags a nested call in), and **small** (rule count ≤ [`INLINE_MAX_RULES`],
//! the state-graph analog of the `.pmc` op-count cap). The candidate set is
//! fixed from the pre-pass program state (a routine that calls another is not a
//! leaf and so never a candidate, even though the callee it invokes may be
//! spliced INTO it).
//!
//! # The splice
//!
//! The callee's states are copied into the caller, their ids shifted past the
//! caller's existing dense block, and each copied row widened to the caller's
//! arity — unbound caller tapes pad with wildcard / keep / stay, exactly the
//! mono-stamp padding semantics. The callee's `return` rows are rewritten to the
//! call's `then` continuation (`goto`/`stop`/`halt` map straight across); the
//! call rule keeps its own pattern/write/move/`brk` and retargets to the spliced
//! entry. A `brk` row inside the callee copies verbatim — per-instance
//! duplication is the graft precedent, and the pause address is preserved at
//! every splice. Each copied state's `name` is freshened against every name
//! already present in the caller — its own states plus any earlier splice's
//! copies — before the copy is appended, mirroring codegen's own label-minting
//! idiom; nothing else keys off IR state names, so the rename is transparent
//! everywhere downstream. The pass never DELETES the now-uncalled routine
//! world: reachability warnings already exist at lower, and the linker drops
//! unreachable functions, so a fully-inlined routine lingers inert until link.

use std::collections::{HashMap, HashSet};

use crate::ir::{
    IrCell, IrMove, IrProgram, IrRule, IrTapeBinding, IrThen, IrTransition, IrWorld, IrWorldKind,
    IrWrite,
};

/// Rule-count cap for an inline candidate — the state-graph analog of the
/// `.pmc` optimizer's `INLINE_MAX_OPS`, and the same value. `outline`'s
/// `OUTLINE_MIN_STATES` is deliberately set above this so the two passes cannot
/// ping-pong (see outline.rs for the joint statement of both numbers).
pub(super) const INLINE_MAX_RULES: usize = 6;

/// True when the callee body carries no nested call (a leaf) — the
/// first-iteration eligibility condition.
fn is_leaf(w: &IrWorld) -> bool {
    w.states.iter().all(|st| {
        st.rules.iter().all(|r| {
            !matches!(
                r.transition,
                IrTransition::CallThen { .. } | IrTransition::TailCall { .. }
            )
        })
    })
}

/// Total rows across every state — the callee's "size" for the threshold.
fn rule_count(w: &IrWorld) -> usize {
    w.states.iter().map(|st| st.rules.len()).sum()
}

/// Whether the binding at a call site is a full pass-through into `callee` — a
/// sound SUPERSET of the engine's `is_full_passthrough`, agreeing on equal
/// arity and additionally accepting arity-reducing identity projections the
/// engine keeps framed (see the module doc). `caller.arity >= callee.arity` is
/// checked at the site, not here.
fn is_full_passthrough(binding: &[IrTapeBinding], caller: &IrWorld, callee: &IrWorld) -> bool {
    if binding.is_empty() {
        // A bindless call binds callee tape `k` to caller tape `k` implicitly.
        // The `.tmc` front end never mints one in-unit; the only producer today
        // is the outline trampoline, which shares the caller's exact tape
        // signature. Still guard the bound prefix's per-tape cardinalities so a
        // future bindless producer with a mismatched callee cannot splice
        // symbol indices out of the caller tape's range.
        return (0..callee.arity as usize).all(|k| {
            k < caller.tapes.len()
                && k < callee.tapes.len()
                && caller.tapes[k].cardinality == callee.tapes[k].cardinality
        });
    }
    binding.len() == callee.arity as usize
        && binding.iter().enumerate().all(|(k, tb)| {
            tb.caller_tape as usize == k
                && tb.pairs.is_empty()
                && k < callee.tapes.len()
                && (tb.caller_tape as usize) < caller.tapes.len()
                && caller.tapes[k].cardinality == callee.tapes[k].cardinality
        })
}

pub fn run(ir: &mut IrProgram) -> u32 {
    let entry = ir.entry_world;
    // Fix the candidate set from the pre-pass state (clone the eligible
    // routines): a routine mutated as a caller must not change its own
    // candidacy mid-pass.
    let candidates: HashMap<String, IrWorld> = ir
        .worlds
        .iter()
        .enumerate()
        .filter(|(i, w)| {
            w.kind == IrWorldKind::Routine
                && Some(*i) != entry
                && is_leaf(w)
                && rule_count(w) <= INLINE_MAX_RULES
        })
        .map(|(_, w)| (w.name.clone(), w.clone()))
        .collect();
    if candidates.is_empty() {
        return 0;
    }

    let mut changes = 0u32;
    for i in 0..ir.worlds.len() {
        while let Some((si, ri)) = find_site(&ir.worlds[i], &candidates) {
            splice(&mut ir.worlds[i], si, ri, &candidates);
            changes += 1;
        }
    }
    changes
}

/// The first `(state index, rule index)` in `caller` whose transition is a
/// full-passthrough call into a candidate. A self-call is never a site (a leaf
/// cannot call itself anyway); the arity gate lives here.
fn find_site(caller: &IrWorld, candidates: &HashMap<String, IrWorld>) -> Option<(usize, usize)> {
    for (si, st) in caller.states.iter().enumerate() {
        for (ri, r) in st.rules.iter().enumerate() {
            if let IrTransition::CallThen {
                target, binding, ..
            } = &r.transition
                && target != &caller.name
                && let Some(callee) = candidates.get(target)
                && callee.arity <= caller.arity
                && is_full_passthrough(binding, caller, callee)
            {
                return Some((si, ri));
            }
        }
    }
    None
}

/// Splice the candidate named at `caller.states[si].rules[ri]` into `caller`.
fn splice(caller: &mut IrWorld, si: usize, ri: usize, candidates: &HashMap<String, IrWorld>) {
    let (target, then) = match &caller.states[si].rules[ri].transition {
        IrTransition::CallThen { target, then, .. } => (target.clone(), *then),
        _ => unreachable!("find_site returns a CallThen site"),
    };
    let callee = &candidates[&target];
    let base = caller.states.len() as u32;
    let n = caller.arity as usize;

    // The call rule keeps its own action and brk; only its transition changes,
    // to a jump into the spliced entry. (The caller's write/move at the site
    // still fire, then control enters the copied body.)
    caller.states[si].rules[ri].transition = IrTransition::Goto {
        state: base + callee.entry,
    };

    // Copy the callee's dense states in, shifted by `base` (so ids stay dense
    // in append order), each row widened to the caller's arity and its
    // `return`/`goto` rewritten. `callee.states[j].id == j` (dense), so the new
    // id is `base + j`. Names are freshened against every name already in the
    // caller — its own states plus any earlier splice's copies, since
    // `caller.states` IS that accumulated set at this point in the loop —
    // so two splices of the same callee, or a callee state name that happens
    // to collide with the caller's own, never leave two states sharing a
    // name (codegen mints its labels straight from state names and only
    // collision-checks the synthetic ones it mints itself).
    let mut used: HashSet<String> = caller.states.iter().map(|s| s.name.clone()).collect();
    for cst in &callee.states {
        let mut st = cst.clone();
        st.id = base + cst.id;
        st.name = fresh(&mut used, &cst.name);
        for r in &mut st.rules {
            widen_rule(r, n);
            r.transition = remap_transition(&r.transition, base, then);
        }
        caller.states.push(st);
    }
}

/// Widen a copied callee row to the caller's arity `n`: pattern pads with
/// wildcards, an explicit write pads with `keep`, explicit moves pad with
/// `stay` — the unbound caller tapes the callee never names. An identity
/// (`None`) write/move vector stays `None` (all-keep / all-stay at every width).
fn widen_rule(r: &mut IrRule, n: usize) {
    if r.pattern.len() < n {
        r.pattern.resize(n, IrCell::Wildcard);
    }
    if let Some(w) = &mut r.write
        && w.len() < n
    {
        w.resize(n, IrWrite::Keep);
    }
    if let Some(m) = &mut r.moves
        && m.len() < n
    {
        m.resize(n, IrMove::Stay);
    }
}

/// Rewrite a copied callee transition into the caller's id space: in-world
/// `goto`s shift by `base`; a `return` becomes the call's `then` continuation
/// (its `goto` target is already a caller-space id); terminals and traps pass
/// through. Candidates are leaves, so no nested call is ever reached.
fn remap_transition(t: &IrTransition, base: u32, then: IrThen) -> IrTransition {
    match t {
        IrTransition::Goto { state } => IrTransition::Goto {
            state: base + state,
        },
        IrTransition::Return => match then {
            IrThen::Goto { state } => IrTransition::Goto { state },
            IrThen::Return => IrTransition::Return,
            IrThen::Stop => IrTransition::Stop,
            IrThen::Halt => IrTransition::Halt,
        },
        IrTransition::Stop => IrTransition::Stop,
        IrTransition::Halt => IrTransition::Halt,
        IrTransition::TrapRead => IrTransition::TrapRead,
        IrTransition::TrapWrite => IrTransition::TrapWrite,
        IrTransition::CallThen { .. } | IrTransition::TailCall { .. } => {
            unreachable!("inline candidates are leaves — no nested call to remap")
        }
    }
}

/// Mint a name distinct from every name in `used`, inserting it before
/// returning: try `base` itself, then `{base}_1`, `{base}_2`, … Mirrors
/// codegen's own synthetic-label minting idiom (a local copy, not a shared
/// call — this pass must not reach into codegen's private surface).
fn fresh(used: &mut HashSet<String>, base: &str) -> String {
    if used.insert(base.to_string()) {
        return base.to_string();
    }
    let mut i = 1;
    loop {
        let cand = format!("{base}_{i}");
        if used.insert(cand.clone()) {
            return cand;
        }
        i += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compiler::analyze;
    use crate::expand::expand;
    use crate::ir::{
        IrCell, IrDispatch, IrRule, IrState, IrTape, IrTransition, IrWorld, IrWorldKind, lower,
        validate_world,
    };

    fn ir_of(src: &str) -> IrProgram {
        let a = analyze(src).unwrap_or_else(|e| panic!("analyze: {e}"));
        let ex = expand(&a.resolved).unwrap_or_else(|e| panic!("expand: {e}"));
        lower(&ex, &a.resolved)
            .unwrap_or_else(|e| panic!("lower: {e}"))
            .0
    }

    fn world<'a>(ir: &'a IrProgram, name: &str) -> &'a IrWorld {
        ir.worlds.iter().find(|w| w.name == name).expect("world")
    }

    fn any_callthen(w: &IrWorld) -> bool {
        w.states.iter().any(|s| {
            s.rules
                .iter()
                .any(|r| matches!(r.transition, IrTransition::CallThen { .. }))
        })
    }

    #[test]
    fn an_identity_map_call_is_inlined_and_the_call_disappears() {
        // `main` calls a small leaf routine with the identity binding `t = t`
        // (empty pairs, same alphabet) — a full pass-through. After inline the
        // `CallThen` is gone from `main`; the routine world lingers (uncalled)
        // for the linker to drop.
        let mut ir = ir_of(
            "alphabet ab { '_', 'a' }
routine helper(tape t: ab) { entry state s { ['a'] -> write ['_'] return; [*] -> return; } }
machine {
  tape t: ab;
  entry state m { [*] -> call helper(t = t) then done; }
  state done     { [*] -> stop; }
}",
        );
        assert_eq!(run(&mut ir), 1, "the one full-passthrough call inlines");
        let main = world(&ir, "main");
        assert!(!any_callthen(main), "the call was spliced away");
        validate_world(main).unwrap();
    }

    #[test]
    fn a_widened_arity_call_pads_the_unbound_tape() {
        // A 2-tape machine calls a 1-tape helper binding its tape `x` (tape 0).
        // The spliced rows widen to arity 2: tape `y` (tape 1) pads with
        // wildcard / keep / stay — the unbound tape the callee never touched.
        let mut ir = ir_of(
            "alphabet ab { '_', 'a' }
routine helper(tape t: ab) { entry state s { ['a'] -> write ['_'] move [>] return; [*] -> return; } }
machine {
  tape x: ab;
  tape y: ab;
  entry state m { [*, *] -> call helper(t = x) then done; }
  state done     { [*, *] -> stop; }
}",
        );
        assert_eq!(run(&mut ir), 1);
        let main = world(&ir, "main");
        assert!(!any_callthen(main));
        // Every row is now arity-2; the spliced 'a'-row keeps 'a' on tape 0 and
        // a wildcard on the unbound tape 1.
        for st in &main.states {
            for r in &st.rules {
                assert_eq!(r.pattern.len(), 2, "widened to the caller arity");
            }
        }
        let widened = main.states.iter().flat_map(|s| &s.rules).any(|r| {
            matches!(r.pattern.first(), Some(IrCell::Index { index: 1 }))
                && matches!(r.pattern.get(1), Some(IrCell::Wildcard))
        });
        assert!(
            widened,
            "the bound tape keeps 'a', the unbound tape is wildcard"
        );
        validate_world(main).unwrap();
    }

    #[test]
    fn a_bindless_call_is_inlined() {
        // The bindless shape the front end never mints in-unit but `outline`
        // does: a machine whose entry bindless-calls a leaf routine of the same
        // arity. Built at the IR level; inline splices it exactly like the
        // bound identity case.
        let tapes = vec![IrTape {
            name: "t".into(),
            alphabet: "ab".into(),
            cardinality: 2,
            volatile: false,
        }];
        let machine = IrWorld {
            name: "main".into(),
            kind: IrWorldKind::Machine,
            arity: 1,
            tapes: tapes.clone(),
            entry: 0,
            states: vec![
                IrState {
                    id: 0,
                    name: "m".into(),
                    line: 0,
                    rules: vec![IrRule {
                        pattern: vec![IrCell::Wildcard],
                        write: None,
                        moves: None,
                        debugger: false,
                        transition: IrTransition::CallThen {
                            target: "r".into(),
                            binding: vec![],
                            then: IrThen::Goto { state: 1 },
                        },
                        synthesized: false,
                        direct: false,
                        line: 0,
                    }],
                    dispatch: IrDispatch::Table,
                },
                IrState {
                    id: 1,
                    name: "done".into(),
                    line: 0,
                    rules: vec![IrRule {
                        pattern: vec![IrCell::Wildcard],
                        write: None,
                        moves: None,
                        debugger: false,
                        transition: IrTransition::Stop,
                        synthesized: false,
                        direct: false,
                        line: 0,
                    }],
                    dispatch: IrDispatch::Table,
                },
            ],
            local: false,
            line: 0,
        };
        let routine = IrWorld {
            name: "r".into(),
            kind: IrWorldKind::Routine,
            arity: 1,
            tapes,
            entry: 0,
            states: vec![IrState {
                id: 0,
                name: "s".into(),
                line: 0,
                rules: vec![IrRule {
                    pattern: vec![IrCell::Wildcard],
                    write: Some(vec![IrWrite::Index { index: 1 }]),
                    moves: None,
                    debugger: false,
                    transition: IrTransition::Return,
                    synthesized: false,
                    direct: false,
                    line: 0,
                }],
                dispatch: IrDispatch::Table,
            }],
            local: true,
            line: 0,
        };
        let mut ir = IrProgram {
            version: crate::ir::TM_IR_VERSION,
            worlds: vec![machine, routine],
            entry_world: Some(0),
        };
        assert_eq!(run(&mut ir), 1, "the bindless call inlines");
        let main = world(&ir, "main");
        assert!(!any_callthen(main));
        // The routine's `return` became a goto to `done` (the call's then).
        let spliced = &main.states[2];
        assert_eq!(spliced.rules[0].transition, IrTransition::Goto { state: 1 });
        validate_world(main).unwrap();
    }

    #[test]
    fn a_bindless_call_with_a_mismatched_callee_is_refused() {
        // A bindless call whose callee tape is WIDER than the caller's
        // (cardinality 3 vs 2). No `.tmc` can produce this today — the front end
        // always binds in-unit tape-bearing routines, and outline synthesizes
        // only same-signature trampolines — so it is hand-built at the IR level.
        // The callee writes symbol index 2, valid in its own 3-wide alphabet but
        // out of range on the caller's 2-wide tape; splicing it would inject an
        // out-of-range write, so the bindless cardinality guard refuses and the
        // call survives.
        let caller_tapes = vec![IrTape {
            name: "t".into(),
            alphabet: "ab".into(),
            cardinality: 2,
            volatile: false,
        }];
        let callee_tapes = vec![IrTape {
            name: "t".into(),
            alphabet: "abc".into(),
            cardinality: 3,
            volatile: false,
        }];
        let machine = IrWorld {
            name: "main".into(),
            kind: IrWorldKind::Machine,
            arity: 1,
            tapes: caller_tapes,
            entry: 0,
            states: vec![
                IrState {
                    id: 0,
                    name: "m".into(),
                    line: 0,
                    rules: vec![IrRule {
                        pattern: vec![IrCell::Wildcard],
                        write: None,
                        moves: None,
                        debugger: false,
                        transition: IrTransition::CallThen {
                            target: "r".into(),
                            binding: vec![],
                            then: IrThen::Goto { state: 1 },
                        },
                        synthesized: false,
                        direct: false,
                        line: 0,
                    }],
                    dispatch: IrDispatch::Table,
                },
                IrState {
                    id: 1,
                    name: "done".into(),
                    line: 0,
                    rules: vec![IrRule {
                        pattern: vec![IrCell::Wildcard],
                        write: None,
                        moves: None,
                        debugger: false,
                        transition: IrTransition::Stop,
                        synthesized: false,
                        direct: false,
                        line: 0,
                    }],
                    dispatch: IrDispatch::Table,
                },
            ],
            local: false,
            line: 0,
        };
        let routine = IrWorld {
            name: "r".into(),
            kind: IrWorldKind::Routine,
            arity: 1,
            tapes: callee_tapes,
            entry: 0,
            states: vec![IrState {
                id: 0,
                name: "s".into(),
                line: 0,
                rules: vec![IrRule {
                    pattern: vec![IrCell::Wildcard],
                    write: Some(vec![IrWrite::Index { index: 2 }]),
                    moves: None,
                    debugger: false,
                    transition: IrTransition::Return,
                    synthesized: false,
                    direct: false,
                    line: 0,
                }],
                dispatch: IrDispatch::Table,
            }],
            local: true,
            line: 0,
        };
        let mut ir = IrProgram {
            version: crate::ir::TM_IR_VERSION,
            worlds: vec![machine, routine],
            entry_world: Some(0),
        };
        assert_eq!(
            run(&mut ir),
            0,
            "the mismatched bindless call is not inlined"
        );
        assert!(
            any_callthen(world(&ir, "main")),
            "the call to the wider-alphabet callee survives"
        );
    }

    #[test]
    fn a_non_leaf_callee_is_not_inlined() {
        // `outer` calls `inner`, so `outer` is not a leaf and cannot be spliced
        // into `main`; its call survives. (`inner`, a leaf, is spliced into
        // `outer` — candidates come from the pre-pass state.)
        let mut ir = ir_of(
            "alphabet ab { '_', 'a' }
routine inner(tape t: ab) { entry state s { [*] -> return; } }
routine outer(tape t: ab) { entry state s { [*] -> call inner(t = t) then return; } }
machine {
  tape t: ab;
  entry state m { [*] -> call outer(t = t) then done; }
  state done     { [*] -> stop; }
}",
        );
        run(&mut ir);
        let main = world(&ir, "main");
        assert!(
            main.states.iter().flat_map(|s| &s.rules).any(|r| matches!(
                &r.transition,
                IrTransition::CallThen { target, .. } if target == "outer"
            )),
            "the call to the non-leaf `outer` survives"
        );
    }

    #[test]
    fn a_callee_over_the_rule_threshold_is_not_inlined() {
        // `big` has 7 rows across a 7-state chain (> INLINE_MAX_RULES = 6), so
        // it is not a candidate; the call to it survives. (A chain, not repeated
        // rows in one state, since exact-duplicate rows are a front-end error.)
        let mut ir = ir_of(
            "alphabet ab { '_', 'a' }
routine big(tape t: ab) {
  entry state s0 { [*] -> move [>] goto s1; }
  state s1 { [*] -> move [>] goto s2; }
  state s2 { [*] -> move [>] goto s3; }
  state s3 { [*] -> move [>] goto s4; }
  state s4 { [*] -> move [>] goto s5; }
  state s5 { [*] -> move [>] goto s6; }
  state s6 { [*] -> return; }
}
machine {
  tape t: ab;
  entry state m { [*] -> call big(t = t) then done; }
  state done     { [*] -> stop; }
}",
        );
        run(&mut ir);
        assert!(
            any_callthen(world(&ir, "main")),
            "the oversize callee is not inlined"
        );
    }

    #[test]
    fn the_entry_world_is_never_a_candidate() {
        // A routine that bindlessly "calls main" (built at the IR level — no
        // `.tmc` can name the machine as a call target). `main` is the entry
        // world, excluded from candidates, so the call survives.
        let tapes = vec![IrTape {
            name: "t".into(),
            alphabet: "ab".into(),
            cardinality: 2,
            volatile: false,
        }];
        let stop_state = |id: u32, name: &str| IrState {
            id,
            name: name.into(),
            line: 0,
            rules: vec![IrRule {
                pattern: vec![IrCell::Wildcard],
                write: None,
                moves: None,
                debugger: false,
                transition: IrTransition::Stop,
                synthesized: false,
                direct: false,
                line: 0,
            }],
            dispatch: IrDispatch::Table,
        };
        let machine = IrWorld {
            name: "main".into(),
            kind: IrWorldKind::Machine,
            arity: 1,
            tapes: tapes.clone(),
            entry: 0,
            states: vec![stop_state(0, "m")],
            local: false,
            line: 0,
        };
        let routine = IrWorld {
            name: "r".into(),
            kind: IrWorldKind::Routine,
            arity: 1,
            tapes,
            entry: 0,
            states: vec![IrState {
                id: 0,
                name: "s".into(),
                line: 0,
                rules: vec![IrRule {
                    pattern: vec![IrCell::Wildcard],
                    write: None,
                    moves: None,
                    debugger: false,
                    transition: IrTransition::CallThen {
                        target: "main".into(),
                        binding: vec![],
                        then: IrThen::Return,
                    },
                    synthesized: false,
                    direct: false,
                    line: 0,
                }],
                dispatch: IrDispatch::Table,
            }],
            local: true,
            line: 0,
        };
        let mut ir = IrProgram {
            version: crate::ir::TM_IR_VERSION,
            worlds: vec![machine, routine],
            entry_world: Some(0),
        };
        assert_eq!(run(&mut ir), 0, "the entry world is not spliced");
        assert!(any_callthen(world(&ir, "r")), "the call to main survives");
    }

    #[test]
    fn inline_keeps_the_callers_tape_volatility() {
        // Same bindless-call fixture as `a_bindless_call_is_inlined`, with the
        // caller's tape marked volatile. `splice` only appends copied callee
        // states into `caller.states`; `caller.tapes` is never read back or
        // rebuilt, so the flag must survive untouched.
        let machine = IrWorld {
            name: "main".into(),
            kind: IrWorldKind::Machine,
            arity: 1,
            tapes: vec![IrTape {
                name: "t".into(),
                alphabet: "ab".into(),
                cardinality: 2,
                volatile: true,
            }],
            entry: 0,
            states: vec![
                IrState {
                    id: 0,
                    name: "m".into(),
                    line: 0,
                    rules: vec![IrRule {
                        pattern: vec![IrCell::Wildcard],
                        write: None,
                        moves: None,
                        debugger: false,
                        transition: IrTransition::CallThen {
                            target: "r".into(),
                            binding: vec![],
                            then: IrThen::Goto { state: 1 },
                        },
                        synthesized: false,
                        direct: false,
                        line: 0,
                    }],
                    dispatch: IrDispatch::Table,
                },
                IrState {
                    id: 1,
                    name: "done".into(),
                    line: 0,
                    rules: vec![IrRule {
                        pattern: vec![IrCell::Wildcard],
                        write: None,
                        moves: None,
                        debugger: false,
                        transition: IrTransition::Stop,
                        synthesized: false,
                        direct: false,
                        line: 0,
                    }],
                    dispatch: IrDispatch::Table,
                },
            ],
            local: false,
            line: 0,
        };
        let routine = IrWorld {
            name: "r".into(),
            kind: IrWorldKind::Routine,
            arity: 1,
            tapes: vec![IrTape {
                name: "t".into(),
                alphabet: "ab".into(),
                cardinality: 2,
                volatile: false,
            }],
            entry: 0,
            states: vec![IrState {
                id: 0,
                name: "s".into(),
                line: 0,
                rules: vec![IrRule {
                    pattern: vec![IrCell::Wildcard],
                    write: Some(vec![IrWrite::Index { index: 1 }]),
                    moves: None,
                    debugger: false,
                    transition: IrTransition::Return,
                    synthesized: false,
                    direct: false,
                    line: 0,
                }],
                dispatch: IrDispatch::Table,
            }],
            local: true,
            line: 0,
        };
        let mut ir = IrProgram {
            version: crate::ir::TM_IR_VERSION,
            worlds: vec![machine, routine],
            entry_world: Some(0),
        };
        let changed = run(&mut ir);
        assert!(changed > 0, "premise: inline must fire on this fixture");
        let main = world(&ir, "main");
        assert!(!any_callthen(main), "the call was spliced away");
        assert!(main.tapes[0].volatile, "the caller's tape stays volatile");
    }

    /// The transition a state's single rule became after splicing — every
    /// caller state built below has exactly one rule.
    fn goto_target(w: &IrWorld, state_name: &str) -> u32 {
        let st = w
            .states
            .iter()
            .find(|s| s.name == state_name)
            .unwrap_or_else(|| panic!("{state_name} is in {}", w.name));
        match st.rules[0].transition {
            IrTransition::Goto { state } => state,
            ref t => panic!("expected a goto after inline, got {t:?}"),
        }
    }

    /// All state names in `w`, asserting no two are equal.
    fn assert_names_pairwise_distinct(w: &IrWorld) {
        let names: Vec<&str> = w.states.iter().map(|s| s.name.as_str()).collect();
        let unique: HashSet<&str> = names.iter().copied().collect();
        assert_eq!(
            names.len(),
            unique.len(),
            "state names in {} are pairwise distinct: {names:?}",
            w.name
        );
    }

    #[test]
    fn splicing_the_same_leaf_callee_at_two_sites_freshens_the_second_copy() {
        // `main` calls the SAME leaf callee `flip` from two different states.
        // Before the fix, both copies kept the callee's verbatim state name
        // `s`, so the caller ended up with two states both named `s` —
        // codegen mints `.tma` labels straight from state names, so this
        // produced a duplicate label the assembler rejected. The fix
        // freshens each copy against the names already in the caller,
        // including any earlier splice's copies.
        let mut ir = ir_of(
            "alphabet ab { '_', 'a' }
routine flip(tape t: ab) { entry state s { ['a'] -> write ['_'] return; [*] -> return; } }
machine {
  tape t: ab;
  entry state m1 { [*] -> call flip(t = t) then m2; }
  state m2       { [*] -> call flip(t = t) then done; }
  state done     { [*] -> stop; }
}",
        );
        assert_eq!(run(&mut ir), 2, "both call sites inline");
        let main = world(&ir, "main");
        assert!(!any_callthen(main), "both calls were spliced away");

        // (a) both call sites became gotos into DISTINCT spliced entries.
        let t1 = goto_target(main, "m1");
        let t2 = goto_target(main, "m2");
        assert_ne!(
            t1, t2,
            "the two splices land at distinct spliced entries, not the same one"
        );

        // (b) all state names in the caller are pairwise distinct.
        assert_names_pairwise_distinct(main);
        validate_world(main).unwrap();
    }

    #[test]
    fn a_caller_state_colliding_with_the_callees_name_is_freshened() {
        // The caller's OWN state is named `s`, the same name as the leaf
        // callee's entry state — one call site, so this is the single-splice
        // half of the missing-freshening hole (no earlier copy involved).
        let mut ir = ir_of(
            "alphabet ab { '_', 'a' }
routine flip(tape t: ab) { entry state s { ['a'] -> write ['_'] return; [*] -> return; } }
machine {
  tape t: ab;
  entry state s  { [*] -> call flip(t = t) then done; }
  state done     { [*] -> stop; }
}",
        );
        assert_eq!(run(&mut ir), 1);
        let main = world(&ir, "main");
        assert!(!any_callthen(main), "the call was spliced away");
        assert_names_pairwise_distinct(main);
        // Pin the exact freshening scheme: `used` seeds from `{"s", "done"}`,
        // so the callee's own `s` collides and mints `s_1` — and that is the
        // name of the state the call site's goto now actually targets, not
        // merely some state carrying that name.
        let entry_id = goto_target(main, "s");
        let spliced = main
            .states
            .iter()
            .find(|st| st.id == entry_id)
            .unwrap_or_else(|| panic!("no state has id {entry_id}"));
        assert_eq!(
            spliced.name, "s_1",
            "the spliced entry is freshened to exactly `s_1`"
        );
        validate_world(main).unwrap();
    }
}
