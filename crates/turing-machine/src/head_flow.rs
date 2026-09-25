//! Head positions: for every world, state and tape, the glyph set the head
//! MAY be sitting on when control reaches that state — and, read off it, the
//! `call`/`bind` sites whose callee's `enters` clause that set can escape
//! (docs/tmt/language.md (head-position clauses)).
//!
//! # The soundness contract: over-approximation
//!
//! Same direction as the write footprint next door: a computed set is a
//! SUPERSET of where a run can actually leave the head. A glyph OUTSIDE a
//! set provably never sits under the head there; one inside it merely may.
//! Every uncertain step therefore ADDS — a move lands the head on a cell
//! nothing in the row read, so the whole alphabet follows it; a `{expr}`
//! write folds per expanded row, which this pre-expansion walk does not do,
//! so it answers the whole alphabet too; and a call whose callee nothing
//! vouches for returns with the head anywhere.
//!
//! That is why the finding this feeds is a WARNING whose message says "may
//! be on": the authority on what one run does is the `enters` assert a debug
//! build plants (`crate::contracts::head`), which answers exactly, for the
//! path it took. This walk answers approximately, for every path at once.
//!
//! # Where it stops, deliberately
//!
//! The walk runs over the RESOLVED module, before graft splicing and range
//! expansion, because that is all the lint layer ever sees.
//!
//! A GRAFT is the one construct that costs real precision there. Its body is
//! not spliced yet, so where it leaves the head is unknowable here — and a
//! graft carries control flow, not just rules: the states it exits into are
//! reachable ONLY through it. Ignoring it would leave every one of them
//! unreachable and their own sites silently unchecked, which on a
//! graft-driven program is most of the program. So a graft instance takes
//! part as a pseudo-state: reaching it is tracked (it is addressable by name
//! in the same space states are, and a `goto` may name one), and a reached
//! one hands the WHOLE ALPHABET to each host state its args name as an exit.
//! Coarse, sound, and — unlike silence — able to report.
//!
//! What remains under-approximated, costing findings and never inventing
//! one: a state no walked path reaches keeps no set at all, and its own rows
//! are never examined.
//!
//! # Per world, not per program
//!
//! Unlike the write footprint, this needs no cross-world fixpoint. A world's
//! entry is seeded from the world's OWN `enters` clause, which is a promise
//! its callers are held to separately — by this very rule at their sites, and
//! by the planted assert at run time. So each world settles on its own, and
//! the loop is bounded by its tapes' cardinalities.

use std::collections::{BTreeMap, HashMap};

use mtc_core::diagnostics::Span;

use crate::compiler::{Resolved, ResolvedCall, ResolvedCallTarget, ResolvedWorld, symset_glyphs};
use crate::declarations::Declarations;
use crate::footprint::{
    SitePlacement, SymSet, find_external, glyph_index, project_forward, project_write_back,
    site_placement,
};
use crate::parser::{
    BindingValue, Continuation, MoveDir, PatternCell, PatternCellKind, Rule, Transition, WriteCell,
    WriteCellKind,
};
use crate::patterns::cell_labels;

/// One `call`/`bind` site whose callee declares `enters` on a tape parameter
/// that the caller's own head set may escape.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EntersGap {
    /// The `call` transition's own span — the ROW, never the `bind`
    /// declaration: one bind instance may be called from several rows, and
    /// where the head is is a fact about the row.
    pub(crate) site: Span,
    /// The callee's mangled name.
    pub(crate) callee: String,
    /// The callee tape parameter carrying the clause.
    pub(crate) param: String,
    /// The declared clause, in the CALLEE's alphabet frame, band order.
    pub(crate) declared: Vec<String>,
    /// The glyphs the head may be on that the clause does not list, same
    /// frame and order. Never empty — a gap with nothing offending is not
    /// reported at all.
    pub(crate) offending: Vec<String>,
}

/// Whether any world in `module` declares an `enters` clause. The whole
/// analysis is skipped unless one does somewhere, so a module that declares
/// none — every source this repository ships today — pays a scan of the
/// signatures and nothing else.
fn declares_enters(module: &Resolved) -> bool {
    module
        .worlds
        .iter()
        .any(|w| w.tapes.iter().any(|t| t.enters.is_some()))
}

/// Every site in `resolved` whose callee's `enters` clause the caller's head
/// set may escape, in source order.
///
/// `externals` is the declaration table this unit was analyzed against, and
/// it is the ONLY way a callee outside the unit is seen: a call whose
/// callee's signature is in neither `resolved` nor `externals` is silent,
/// since nothing here can tell "unmet" from "unknown".
pub(crate) fn enters_gaps(resolved: &Resolved, externals: &Declarations) -> Vec<EntersGap> {
    let modules = externals.modules();
    if !declares_enters(resolved) && !modules.iter().any(|m| declares_enters(m)) {
        return Vec::new();
    }
    let mut out = Vec::new();
    for world in &resolved.worlds {
        walk_world(resolved, &modules, world, &mut out);
    }
    out.sort_by_key(|g| g.site.start);
    out
}

/// A call site's callee, resolved: the world, the module its alphabets live
/// in, and the placement of its tapes onto the host's.
struct Site<'a> {
    callee: &'a ResolvedWorld,
    module: &'a Resolved,
    placement: SitePlacement,
}

/// Resolve one `call`/`bind` transition's callee the way the footprint walk
/// resolves a reuse edge: a target this module defines is found in
/// `resolved`; one it does not is looked up in the declaration modules, in
/// their own first-match order. `None` — a callee nothing vouches for — is
/// the silent case.
fn resolve_site<'a>(
    resolved: &'a Resolved,
    modules: &[&'a Resolved],
    host: &ResolvedWorld,
    call: &'a ResolvedCall,
) -> Option<Site<'a>> {
    let (target, external, args) = match &call.target {
        ResolvedCallTarget::Routine {
            name,
            external,
            args,
        } => (name.as_str(), *external, args.as_slice()),
        // A bind-call's binding lives on the `bind` declaration, shared by
        // every call of that instance.
        ResolvedCallTarget::Bind { name } => {
            let bind = host.binds.iter().find(|b| b.name == *name)?;
            (bind.target.as_str(), bind.external, bind.args.as_slice())
        }
    };
    let (module, callee) = if external {
        find_external(modules, target)?
    } else {
        let callee = resolved.worlds.iter().find(|w| w.name == target)?;
        (resolved, callee)
    };
    Some(Site {
        callee,
        module,
        placement: site_placement(resolved, module, host, callee, args),
    })
}

/// The glyph labels of an alphabet by mangled name, or `None` when the
/// module carries no such alphabet.
fn alphabet<'a>(module: &'a Resolved, mangled: &str) -> Option<&'a [String]> {
    module.alphabets.get(mangled).map(|a| a.glyphs.as_slice())
}

/// The symbol set a pattern cell matches, in its tape's own frame. A cell
/// naming nothing the alphabet carries yields the EMPTY set — the row can
/// never fire, exactly as expansion drops it; an unresolvable range answers
/// the whole alphabet, since nothing about it can be ruled out.
///
/// The membership itself is [`cell_labels`]', the primitive the coverage
/// lints and the static `enters` check read through
/// [`crate::patterns::accepted_glyphs`]; only the WILDCARD case takes a
/// short cut, answering `full` instead of materializing every glyph of the
/// alphabet as a `String` and looking each one back up. That is the same
/// answer by `cell_labels`' own first rule, it is by far the commonest cell,
/// and it is on the hot path of a fixpoint — `the_two_cell_readings_agree`
/// holds the two in step over every cell shape.
fn cell_set(cell: &PatternCell, glyphs: &[String], card: u32) -> SymSet {
    if matches!(cell.kind, PatternCellKind::Wildcard) {
        return SymSet::full(card);
    }
    let Some(labels) = cell_labels(cell, glyphs) else {
        return SymSet::full(card);
    };
    let mut out = SymSet::empty();
    for label in labels {
        if let Some(ix) = glyphs.iter().position(|g| *g == label) {
            out.insert(ix as u32);
        }
    }
    out
}

/// Where a row leaves the head on each tape, given what its pattern matched.
///
/// A move lands the head on a cell the row never read, so the whole alphabet
/// follows it. Otherwise a literal write pins the glyph, a `{expr}` write is
/// folded per expanded row by a stage this walk runs ahead of, and an
/// unwritten cell keeps what the pattern matched. A write or move vector
/// whose width disagrees with the world's arity answers the whole alphabet
/// on every tape rather than reading a missing cell as "keep", which would
/// under-approximate.
fn post_glyphs(
    rule: &Rule,
    matched: &[SymSet],
    arity: usize,
    glyphs: &[&[String]],
    cards: &[u32],
) -> Vec<SymSet> {
    let width_mismatch = rule.write.as_ref().is_some_and(|w| w.cells.len() != arity)
        || rule.mov.as_ref().is_some_and(|m| m.cells.len() != arity);
    if width_mismatch {
        return cards.iter().map(|c| SymSet::full(*c)).collect();
    }
    let mut out = matched.to_vec();
    for (k, slot) in out.iter_mut().enumerate() {
        let moved = rule
            .mov
            .as_ref()
            .is_some_and(|m| matches!(m.cells.get(k), Some(c) if c.dir != MoveDir::Stay));
        if moved {
            *slot = SymSet::full(cards[k]);
            continue;
        }
        match rule.write.as_ref().and_then(|w| w.cells.get(k)) {
            None
            | Some(WriteCell {
                kind: WriteCellKind::Keep,
                ..
            }) => {}
            Some(WriteCell {
                kind: WriteCellKind::Lit(lit),
                ..
            }) => {
                *slot = match glyph_index(glyphs[k], lit) {
                    Some(ix) => {
                        let mut one = SymSet::empty();
                        one.insert(ix);
                        one
                    }
                    // A literal outside the tape's own alphabet: resolution
                    // rejects it downstream, and nothing can be ruled out.
                    None => SymSet::full(cards[k]),
                };
            }
            Some(WriteCell {
                kind: WriteCellKind::Subst { .. },
                ..
            }) => *slot = SymSet::full(cards[k]),
        }
    }
    out
}

/// Where a call leaves the head on each HOST tape once it returns.
///
/// A tape the callee reaches resumes from its declared `leaves`, mapped back
/// through the binding by the write direction of the very projection the
/// site check reads forward (`crate::footprint`) — the whole alphabet where
/// the callee declares no clause, which is the answer an opaque callee gets
/// on every tape it can reach. A host tape the binding does not name is not
/// the callee's to touch, so it keeps the row's own post-glyph.
///
/// `post` and `cards` are both indexed by HOST tape position and are both
/// as long as `host.tapes` — `walk_world` builds them from that very list,
/// and every index used below is either a `TapeLink::host` (a position IN
/// that list by construction) or a callee tape index the identity arm has
/// already bounded against it.
fn after_call(
    site: Option<&Site>,
    post: &[SymSet],
    host: &ResolvedWorld,
    cards: &[u32],
) -> Vec<SymSet> {
    let full = || -> Vec<SymSet> { cards.iter().map(|c| SymSet::full(*c)).collect() };
    let Some(site) = site else {
        return full();
    };
    let mut out = post.to_vec();
    match &site.placement {
        SitePlacement::Opaque => return full(),
        SitePlacement::Identity => {
            if site.callee.tapes.len() > host.tapes.len() {
                return full();
            }
            for (k, ct) in site.callee.tapes.iter().enumerate() {
                out[k] = match ct.leaves {
                    // Identity placement: symbols unchanged, clamped to the
                    // host's own alphabet.
                    Some(set) => set.intersect(SymSet::full(cards[k])),
                    None => SymSet::full(cards[k]),
                };
            }
        }
        SitePlacement::Bound(links) => {
            for (k, link) in links.iter().enumerate() {
                let Some(ct) = site.callee.tapes.get(k) else {
                    return full();
                };
                let host_card = cards[link.host];
                out[link.host] = match (ct.leaves, &link.pairs) {
                    (Some(set), Some(pairs)) => {
                        project_write_back(set, pairs, host_card, ct.cardinality as u32)
                    }
                    _ => SymSet::full(host_card),
                };
            }
        }
    }
    out
}

/// The head set a callee tape RECEIVES at one site: the host tape's own set
/// projected forward through the binding into the callee's alphabet frame.
/// `None` when the site says nothing about that tape — an unreadable map, or
/// an identity placement with no host tape at that position.
fn received(
    site: &Site,
    k: usize,
    post: &[SymSet],
    cards: &[u32],
    callee_card: u32,
) -> Option<SymSet> {
    match &site.placement {
        SitePlacement::Opaque => None,
        // Identity placement: callee tape `k` IS host tape `k`, symbols
        // unchanged, clamped to the callee's own alphabet.
        SitePlacement::Identity => post
            .get(k)
            .map(|set| set.intersect(SymSet::full(callee_card))),
        SitePlacement::Bound(links) => {
            let link = links.get(k)?;
            let pairs = link.pairs.as_ref()?;
            Some(project_forward(
                *post.get(link.host)?,
                pairs,
                *cards.get(link.host)?,
                callee_card,
            ))
        }
    }
}

/// Follow one in-world transfer by NAME, over the single space states and
/// graft instances share: a local state takes the set, a graft instance is
/// merely marked reached (its spliced body decides where the head goes, and
/// it is not here to ask). A name that is neither — a routine's or graph's
/// own `state` parameter — leaves the world, and nothing flows.
fn reach(
    heads: &mut [Option<Vec<SymSet>>],
    reached: &mut [bool],
    by_state: &HashMap<&str, usize>,
    by_graft: &HashMap<&str, usize>,
    name: &str,
    add: &[SymSet],
) -> bool {
    if let Some(&target) = by_state.get(name) {
        return flow(heads, target, add);
    }
    if let Some(&gi) = by_graft.get(name) {
        return !std::mem::replace(&mut reached[gi], true);
    }
    false
}

/// Union `add` into the head set of `target`, reporting whether it GREW —
/// the fixpoint's only stop condition. A state no path has reached yet holds
/// no set at all, and the first flow into it installs one.
fn flow(heads: &mut [Option<Vec<SymSet>>], target: usize, add: &[SymSet]) -> bool {
    match &mut heads[target] {
        Some(current) => {
            let mut grew = false;
            for (slot, a) in current.iter_mut().zip(add) {
                grew |= slot.union_with(*a);
            }
            grew
        }
        None => {
            heads[target] = Some(add.to_vec());
            true
        }
    }
}

fn walk_world(
    resolved: &Resolved,
    modules: &[&Resolved],
    world: &ResolvedWorld,
    out: &mut Vec<EntersGap>,
) {
    let Some(glyphs) = world
        .tapes
        .iter()
        .map(|t| alphabet(resolved, &t.alphabet))
        .collect::<Option<Vec<_>>>()
    else {
        return;
    };
    let cards: Vec<u32> = world.tapes.iter().map(|t| t.cardinality as u32).collect();
    let by_state: HashMap<&str, usize> = world
        .states
        .iter()
        .enumerate()
        .map(|(i, s)| (s.name.as_str(), i))
        .collect();
    // Graft instances are addressable by name in the very same space states
    // are, and a `goto` may name one — so they take part in the walk as
    // pseudo-states: reaching one is tracked, and a reached one hands the
    // whole alphabet to the host states its args name as exits. The spliced
    // body itself is not here to walk, so where it leaves the head is not
    // knowable at this stage; the whole alphabet is the sound answer, and
    // without it every state a graft leads to would be unreachable and its
    // own sites silently unchecked.
    let by_graft: HashMap<&str, usize> = world
        .grafts
        .iter()
        .enumerate()
        .filter_map(|(i, g)| g.as_name.as_deref().map(|n| (n, i)))
        .collect();
    // Every site resolved ONCE, not per fixpoint round: resolving one builds
    // its whole binding placement, pair lists and all, and a round may
    // revisit the same row many times. Keyed by the transition's own span —
    // the key `resolve_world_calls` itself records, and the one thing that
    // ties a rule's raw transition back to its resolved (mangled) target.
    let sites: BTreeMap<Span, Option<Site>> = world
        .calls
        .iter()
        .map(|call| (call.span, resolve_site(resolved, modules, world, call)))
        .collect();

    let mut heads: Vec<Option<Vec<SymSet>>> = vec![None; world.states.len()];
    let mut reached: Vec<bool> = vec![false; world.grafts.len()];
    match world.states.iter().position(|s| s.entry) {
        Some(entry) => {
            heads[entry] = Some(
                world
                    .tapes
                    .iter()
                    .zip(&cards)
                    .map(|(t, card)| t.enters.unwrap_or_else(|| SymSet::full(*card)))
                    .collect(),
            );
        }
        // An entry GRAFT: the world's own `enters` would seed the spliced
        // body, which is not here — but the states it exits into are, so the
        // walk starts from them rather than not at all.
        None => match world.grafts.iter().position(|g| g.entry) {
            Some(g) => reached[g] = true,
            // No body at all — a bodiless routine read in declarations mode.
            None => return,
        },
    }

    // Uncapped for the same reason the footprint's loop is: a flow can only
    // grow a set, and a set is bounded by its tape's alphabet, so it
    // terminates by monotonicity. A round cap would stop the walk early and
    // under-approximate.
    loop {
        let mut grew = false;
        for si in 0..world.states.len() {
            let Some(inset) = heads[si].clone() else {
                continue;
            };
            for rule in &world.states[si].rules {
                let Some(post) = row_post(rule, &inset, world, &glyphs, &cards) else {
                    continue;
                };
                match &rule.transition {
                    Transition::Goto { name, .. } => {
                        grew |= reach(&mut heads, &mut reached, &by_state, &by_graft, name, &post);
                    }
                    // An omitted transition stays in the current state.
                    Transition::Stay { .. } => grew |= flow(&mut heads, si, &post),
                    Transition::Call { span, then, .. } => {
                        let Some(Continuation::State { name, .. }) = then else {
                            continue;
                        };
                        let site = sites.get(span).and_then(Option::as_ref);
                        let after = after_call(site, &post, world, &cards);
                        grew |= reach(&mut heads, &mut reached, &by_state, &by_graft, name, &after);
                    }
                    Transition::Return { .. }
                    | Transition::Stop { .. }
                    | Transition::Halt { .. } => {}
                }
            }
        }
        // A reached graft hands the whole alphabet to every host state its
        // args name. An arg naming a TAPE is the graft's tape binding, and
        // the state lookup passes over it — the same discrimination the
        // footprint walk makes on a site's args, read the other way round.
        // Tapes and states are separate namespaces, so one name can be both;
        // reading it as both only widens a set, which is the direction this
        // walk is allowed to err in.
        let full: Vec<SymSet> = cards.iter().map(|c| SymSet::full(*c)).collect();
        for (gi, graft) in world.grafts.iter().enumerate() {
            if !reached[gi] {
                continue;
            }
            for arg in &graft.args {
                if let BindingValue::Named { target, .. } = &arg.value
                    && let Some(&exit) = by_state.get(target.as_str())
                {
                    grew |= flow(&mut heads, exit, &full);
                }
            }
        }
        if !grew {
            break;
        }
    }

    // The settled sets, read once at every site.
    for (si, state) in world.states.iter().enumerate() {
        let Some(inset) = &heads[si] else { continue };
        for rule in &state.rules {
            let Transition::Call { span, .. } = &rule.transition else {
                continue;
            };
            let Some(post) = row_post(rule, inset, world, &glyphs, &cards) else {
                continue;
            };
            let Some(Some(site)) = sites.get(span) else {
                continue;
            };
            check_site(site, &post, &cards, *span, out);
        }
    }
}

/// What a row leaves under the head, or `None` when the row can never fire —
/// its pattern's arity disagrees with the world's, or some tape's matched
/// set is empty, which no input can satisfy.
fn row_post(
    rule: &Rule,
    inset: &[SymSet],
    world: &ResolvedWorld,
    glyphs: &[&[String]],
    cards: &[u32],
) -> Option<Vec<SymSet>> {
    if rule.pattern.cells.len() != world.tapes.len() {
        return None;
    }
    let matched: Vec<SymSet> = rule
        .pattern
        .cells
        .iter()
        .enumerate()
        .map(|(k, cell)| cell_set(cell, glyphs[k], cards[k]).intersect(inset[k]))
        .collect();
    // The pattern is a conjunction across tapes: one empty cell and the row
    // never fires at all.
    if matched.iter().any(|s| s.is_empty()) {
        return None;
    }
    Some(post_glyphs(
        rule,
        &matched,
        world.tapes.len(),
        glyphs,
        cards,
    ))
}

/// Report every callee tape whose declared `enters` the head set reaching it
/// may escape.
fn check_site(site: &Site, post: &[SymSet], cards: &[u32], span: Span, out: &mut Vec<EntersGap>) {
    for (k, ct) in site.callee.tapes.iter().enumerate() {
        let Some(enters) = ct.enters else { continue };
        let callee_card = ct.cardinality as u32;
        let Some(receives) = received(site, k, post, cards, callee_card) else {
            continue;
        };
        let Some(callee_glyphs) = alphabet(site.module, &ct.alphabet) else {
            continue;
        };
        let mut offending = SymSet::empty();
        for index in receives.iter() {
            if !enters.contains(index) {
                offending.insert(index);
            }
        }
        if offending.is_empty() {
            continue;
        }
        out.push(EntersGap {
            site: span,
            callee: site.callee.name.clone(),
            param: ct.name.clone(),
            declared: symset_glyphs(enters, callee_glyphs),
            offending: symset_glyphs(offending, callee_glyphs),
        });
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use mtc_core::diagnostics::Span;

    use super::{cell_set, enters_gaps};
    use crate::compiler::analyze;
    use crate::declarations::Declarations;
    use crate::footprint::SymSet;
    use crate::parser::{PatternCell, PatternCellKind, SymLit};
    use crate::patterns::cell_labels;

    fn span() -> Span {
        Span::new(1, 1, 1, 2)
    }

    fn cell(kind: PatternCellKind) -> PatternCell {
        PatternCell {
            kind,
            binding: None,
            span: span(),
        }
    }

    fn glyph(value: &str) -> SymLit {
        SymLit::Glyph {
            value: value.to_string(),
            span: span(),
        }
    }

    /// [`cell_set`] short-cuts the wildcard instead of asking
    /// [`cell_labels`], so the two readings must agree on every cell shape —
    /// including the ones only `cell_labels` can answer. Mutation it
    /// catches: a wildcard short cut that answers anything but the tape's
    /// whole alphabet (`full(card - 1)`, say, or `full(MAX)`), which no
    /// behavioural fixture would notice on an alphabet whose every glyph
    /// some rule already names.
    #[test]
    fn the_two_cell_readings_agree() {
        let glyphs: Vec<String> = ["_", "a", "b", "c"].iter().map(|g| g.to_string()).collect();
        let card = glyphs.len() as u32;
        let via_labels = |c: &PatternCell| -> Option<SymSet> {
            let labels = cell_labels(c, &glyphs)?;
            let mut out = SymSet::empty();
            for label in labels {
                if let Some(ix) = glyphs.iter().position(|g| *g == label) {
                    out.insert(ix as u32);
                }
            }
            Some(out)
        };
        for kind in [
            PatternCellKind::Wildcard,
            PatternCellKind::Single(glyph("a")),
            // A literal the alphabet does not carry: the empty set, a row
            // that can never fire.
            PatternCellKind::Single(glyph("z")),
            PatternCellKind::Range {
                lo: glyph("a"),
                hi: glyph("c"),
            },
            // A range resolution would reject: `cell_labels` declines, and
            // both readings fall back to the whole alphabet.
            PatternCellKind::Range {
                lo: glyph("c"),
                hi: glyph("a"),
            },
        ] {
            let c = cell(kind);
            let expected = via_labels(&c).unwrap_or_else(|| SymSet::full(card));
            assert_eq!(cell_set(&c, &glyphs, card), expected, "{:?}", c.kind);
        }
    }

    /// The widest source the repository ships — fourteen tapes over a
    /// seventeen-glyph alphabet — where `expand` already dominates a
    /// keystroke's work.
    fn rpnwide() -> String {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../docs/examples/rpnwide/rpnwide.tmc");
        std::fs::read_to_string(&path).expect("the widest example is present")
    }

    fn median<F: FnMut()>(rounds: u32, mut body: F) -> Duration {
        let mut samples: Vec<Duration> = (0..rounds)
            .map(|_| {
                let t = Instant::now();
                body();
                t.elapsed()
            })
            .collect();
        samples.sort_unstable();
        samples[samples.len() / 2]
    }

    /// An explicit-run measurement instrument, not an assertion: what this
    /// analysis adds to one keystroke's work on the widest source, against
    /// the two stages an editor already pays there.
    ///
    /// cargo test -p mtc-turing-machine --lib head_flow -- --ignored --nocapture
    #[test]
    #[ignore = "prints a timing table; run explicitly"]
    fn per_keystroke_cost_on_the_widest_source() {
        let shipped = rpnwide();
        // The same source with one clause added, so the analysis actually
        // runs: without one anywhere it exits on a signature scan, which is
        // what every source in the repository pays today.
        let annotated = shipped.replace(
            "routine setZero(tape nA: hex,",
            "routine setZero(tape nA: hex enters { 0..15 },",
        );
        assert_ne!(annotated, shipped, "the clause injection found its anchor");
        let externals = Declarations::stdlib();
        println!(
            "{:<12} {:>12} {:>12} {:>12}",
            "source", "analyze", "expand", "enters_gaps"
        );
        for (label, source) in [("as shipped", &shipped), ("+1 clause", &annotated)] {
            let analysis = analyze(source).expect("the example analyzes");
            let a = median(15, || {
                analyze(source).expect("analyzes");
            });
            let e = median(15, || {
                let _ = crate::expand::expand(&analysis.resolved, &externals);
            });
            let g = median(15, || {
                let _ = enters_gaps(&analysis.resolved, &externals);
            });
            println!(
                "{label:<12} {:>11.3}ms {:>11.3}ms {:>11.3}ms",
                a.as_secs_f64() * 1e3,
                e.as_secs_f64() * 1e3,
                g.as_secs_f64() * 1e3,
            );
            println!(
                "             findings: {}",
                enters_gaps(&analysis.resolved, &externals).len()
            );
        }
    }
}
