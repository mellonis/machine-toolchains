//! The `tmt interface` printer (docs/tmt/language.md (headers)): one
//! canonical rendering of a unit's EXPORTED declarations, reachable from
//! two inputs — a `.tmc` source (the complete arm: exported alphabets,
//! exported maps (docs/tmt/language.md (named maps)), `export routine`
//! signatures with their contracts and `?` doc lines, and `export graph`
//! bodies in full) or a compiled `.tmo` object (the reduced arm:
//! signatures, contracts, and exported alphabets only — no graph body, no
//! map, no doc line). The two fields diverge for opposite reasons: an
//! exported alphabet's glyphs ride the wire (`Interface::alphabets`), so
//! the object arm CAN reprint them and does; a named map has no assembly
//! spelling of its own at all, so the object arm has nothing to read one
//! back from and prints none.
//!
//! **The printer never emits `volatile` either**, for the identical
//! reason: the language reference states outright that the modifier is
//! compile-time-only and leaves no trace in the generated assembly
//! (docs/tmt/language.md (volatile tapes)), so an object has no bit to
//! read it back from, and it fixes only how a routine's OWN body
//! compiles — never checked at a call site — so it is not part of what a
//! caller may rely on either. Without this, std.tmc's volatile-twin
//! namespaces (`binaryNumbersVolatile`, `binaryNumbersBareVolatile`)
//! would break the two-arm agreement below on every routine they declare.
//!
//! **The printer never emits `preserves`.** A contract clause always
//! prints as `writes { … }` carrying the tape's PUBLISHED write set
//! (`compiler::published_writes`, the one function both `ir::lower` and
//! this module call): the declared EFFECTIVE set when the tape declares
//! `writes` or `preserves`, or — when neither clause is written — the
//! compiler's own INFERRED write set for that tape, never the whole
//! alphabet as a stand-in for "no restriction declared" (the wire has no
//! way to spell that — docs/formats.md (routine interfaces)). `preserves`
//! itself is source-level sugar with no representation on the wire, so an
//! object-arm render could not reproduce it even in principle, and using
//! it as a stand-in for the uncontracted case would diverge from the
//! object arm, which reads the compiler's already-resolved write set off
//! the wire either way.
//!
//! **This is the canonical rendering** — deterministic, and independent
//! of the input's whitespace and comments — which is why it renders from
//! parsed/resolved data structures throughout, never by slicing or
//! echoing source text. That independence is what lets a later graph
//! digest be computed over this printer's own output. Two determinism
//! hazards worth naming because they are easy to reintroduce: this module
//! never iterates `Resolved::alphabets` (a `HashMap`) directly — the
//! source arm walks the flat, source-order `Program::alphabets` /
//! `routines` / `graphs` vectors instead, exactly as `compiler::compile`
//! already does when it fills an object's own `Interface::alphabets`;
//! and every namespace/declaration grouping below
//! is built from those same source-order vectors, never from a hash
//! table's iteration order.
//!
//! **Section order is canonicalized, not source-preserved, at two
//! levels.** Within one namespace, its needed `use` lines print first (see
//! below), then every exported alphabet, then every exported routine,
//! then every exported graph — the three `Program` vectors are walked one
//! after another rather than interleaved by source position. Within one
//! graph's body, `state` blocks print first, then `graft` instances, then
//! `bind` instances — the shape `Graph` already splits them into
//! (`states`, `grafts`, `binds`), so printing in that fixed order needs
//! no interleaved source-position bookkeeping. A signature's own
//! parameter order IS preserved (`Signature::params`, tape and state
//! parameters mixed as written), since nothing else records it.
//!
//! **A namespace prints the `use` lines its own printed content actually
//! needs, on BOTH arms** (docs/tmt/cli.md (interface)), though the two
//! arms reach that decision from different data.
//!
//! On the SOURCE arm: an import from `Program::imports`, declared exactly
//! at that namespace, reprints as `use path[ as alias];` iff its bound
//! short name is referenced, unqualified, by something this render
//! prints in that same scope — a tape signature's alphabet name, or
//! (inside a printed `export graph` body) a bare `graft`/`bind` target or
//! a bare `call` target in a rule's transition — AND EITHER the header
//! prints the import's target itself (a non-exported routine or graph,
//! or an alphabet nothing exported reaches, drops the `use` alongside
//! it — printing either would be text that cannot resolve when the
//! header is read back) OR the target lives in ANOTHER unit: reached
//! through the compile's declarations table rather than through this
//! unit's own declarations (`needed_imports`'s own doc has the exact
//! rule), which a program that compiled at all could only have done by
//! resolving the name externally. This is what makes std.tmc's
//! volatile-twin namespaces (`binaryNumbersVolatile`,
//! `binaryNumbersBareVolatile`), which import their representation
//! alphabet from a SIBLING namespace of the SAME unit, reprint as a
//! header that reparses (the target IS printed here, from the
//! "printed" branch), and is what lets a genuinely cross-unit `use` (a
//! user program's `use std::binaryNumbers::symbols;` against the
//! embedded stdlib, say) reprint too (the "external" branch — nothing in
//! THIS unit ever prints `symbols`, but the reader's own declarations
//! table, stdlib or `--extern`, resolves it independently).
//!
//! On the OBJECT arm: [`resolve_object_alphabet`] decides a tape's
//! alphabet identifier by a four-rule match (its own doc has the details)
//! and prints a `use <qualified name>;` line in the routine's own
//! namespace exactly when that match crosses into another namespace of
//! the same object or into an imported-alphabet record — never for a
//! same-namespace or enclosing-scope match, which needs no `use` at all.
//!
//! Grafts and binds print without their own doc lines: only `alphabet`,
//! `routine`, and `graph` declarations carry one here, even though
//! `docs/tmt/language.md` (doc lines and attention lines) lists more
//! declaration kinds that may accept a doc run in general — the shipped
//! corpus never docs an individual graft or bind instance, and this
//! printer's header shape does not need to.
//!
//! **A routine over a non-exported alphabet is legal, and both arms
//! render it.** On the source arm, every alphabet an EXPORTED routine or
//! graph references prints — as `export alphabet` when the alphabet
//! itself is exported, as a plain `alphabet` (no `export`) when it is
//! only referenced, never exported on its own — alongside every alphabet
//! that IS exported outright, whether referenced or not. An alphabet
//! referenced by nothing exported (like a purely local routine's own
//! private alphabet) still prints nothing, exactly as before.
//!
//! **The object arm has no alphabet NAME to read per tape** — the wire's
//! `RoutineInterface` carries a tape's glyph list, never an identifier
//! for it, so [`resolve_object_alphabet`] reconstructs one by CONTENT
//! match, trying four sources in order: an exported alphabet reachable
//! unqualified from the routine's own namespace (its own, or any
//! ENCLOSING one); failing that, an exported alphabet in ANY OTHER
//! namespace of the same object, named via a `use` line; failing that, an
//! alphabet this object IMPORTED from another unit (`Interface::imports`,
//! docs/formats.md (routine interfaces)), likewise via a `use` line; and
//! failing all three, a SYNTHESIZED, deterministic plain-`alphabet`
//! declaration instead of an error: `<routine>__<param>` (the routine's
//! own mangled name with `::` replaced by `_`, joined to the parameter
//! name), declared at the top level, before the namespace block that uses
//! it. Two exported alphabets (or two imports) sharing one glyph list are
//! genuinely indistinguishable from the object alone — the printer
//! accepts that ambiguity rather than erroring on it, taking the first
//! match in wire order, same as before this rule had four tiers instead
//! of one. The object arm never fails to render a routine for want of an
//! alphabet name.
//!
//! **The object arm skips the entry world.** A `machine` block always
//! compiles to the literal symbol name `main` (a program cannot also
//! declare a top-level `main` routine/graph), and unlike an exported
//! routine it is never a CALLEE — nothing binds against it or reads its
//! own interface entry — so it publishes no write set and has no
//! declaration to render; printing it would falsely claim it writes
//! nothing. The source arm never had this problem: a `machine` block has
//! no `export` keyword to make it eligible in the first place.

use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::path::Path;

use mtc_core::formats::object::{ExportedAlphabet, Interface, ObjectFile, SymbolDef};

use crate::codegen::{render_glyph_element, render_glyph_list};
use crate::compiler::{
    self, CompileError, ReadMode, Resolved, ResolvedCallTarget, ResolvedWorld, full_name,
    published_writes,
};
use crate::declarations::Declarations;
use crate::footprint::{self, FootprintTable};
use crate::parser::{
    Bind, BindingArg, BindingValue, Doc, FoldExprKind, FoldExprNode, FoldOp, Graft, Graph, Import,
    MapArrow, MapDecl, MapPair, MoveDir, Pattern, PatternCellKind, Program, Routine, Rule,
    SigParam, SigParamKind, Signature, State, SymLit, SymMap, TermKind, Transition, WriteCellKind,
};

/// Render every exported declaration of a `.tmc` source as a header — the
/// complete arm.
pub(crate) fn from_source(source: &str) -> Result<String, CompileError> {
    render_from_source(source, ReadMode::Program)
}

/// [`from_source`]'s declarations-only twin: the SAME reader, in
/// [`ReadMode::DeclarationsOnly`] (docs/tmt/language.md (headers)) — a
/// `.tmh`, or (once `--extern` lands) a `.tmc` read as one. Every routine
/// is bodiless and every graph carries its body, so rendering it back
/// reproduces the identical text `from_source` would have printed for the
/// program it was itself rendered from — the round-trip
/// `tmt interface` promises.
pub(crate) fn from_declarations(source: &str) -> Result<String, CompileError> {
    render_from_source(source, ReadMode::DeclarationsOnly)
}

/// The shared body of [`from_source`]/[`from_declarations`]: the mode is a
/// flag on this ONE reader, not a fork — same lexer, same green parse,
/// same `extract_program` either way (docs/tmt/language.md (headers)).
fn render_from_source(source: &str, mode: ReadMode) -> Result<String, CompileError> {
    // The SAME externals `compiler::analyze` resolves against (its own
    // default) and the SAME inference `ir::lower` runs — computed here
    // rather than threaded out of `analyze`, since `Analysis` does not
    // retain the `Declarations` it resolved with. Both go through
    // `compiler::published_writes`, the one function that decides a tape's
    // published write set (docs/tmt/cli.md (interface)).
    let externals = Declarations::stdlib();
    let analysis = compiler::analyze_with_mode(source, &externals, mode)?;
    let footprint = footprint::infer_resolved_with(&analysis.resolved, &externals.modules());
    // A routine's `noreturn` fact: INFERRED from its body for a BODIED
    // routine (`ReadMode::Program`, the ONLY mode where `has_body` is ever
    // true), through the identical `ir::body_can_return` the compiler
    // itself runs over a freshly-expanded module — never re-derived by a
    // second walk. `ReadMode::DeclarationsOnly` gives every routine an
    // EMPTY body (a header has none to infer from), so `expand::expand`
    // is skipped there entirely and `routine_lines` falls back to echoing
    // the DECLARED clause instead (`Routine::noreturn`) — the only source
    // of truth a bodiless signature has.
    let returns: HashMap<String, bool> = if analysis.program.routines.iter().any(|r| r.has_body) {
        let expanded = crate::expand::expand(&analysis.resolved)?;
        expanded
            .worlds
            .iter()
            .map(|w| {
                (
                    w.name.clone(),
                    crate::ir::body_can_return(w, &analysis.resolved),
                )
            })
            .collect()
    } else {
        HashMap::new()
    };
    Ok(render_source(
        &analysis.program,
        &analysis.resolved,
        &footprint,
        &returns,
    ))
}

/// Read one `--extern FILE`'s declarations for `tmt compile`
/// (docs/tmt/cli.md (compile)) — not a render, [`Resolved`] itself, the
/// same shape [`Declarations`] pushes for the embedded stdlib. A `.tmh`
/// extension (case-insensitive, matching `cli/interface.rs`'s identical
/// rule for a `.tmh` on `tmt interface`) selects STRICT reading —
/// [`ReadMode::DeclarationsOnly`], which rejects a routine body or a
/// `machine` block outright — and anything else (a `.tmc`) is read
/// LENIENTLY as [`ReadMode::Program`]: bodies and a `machine` block are
/// accepted and simply unused, since [`Resolved`] retains no body content
/// for [`Declarations`] to keep either way. Text has no container magic to
/// tell a header from a full source by, so — exactly as in
/// `cli/interface.rs` — the extension is the one place it IS the signal,
/// never a second front end.
///
/// Resolved against the embedded standard library as its OWN external
/// context, unconditionally — the same choice [`render_from_source`]
/// makes for `tmt interface`, independent of whatever `--nostdlib`/
/// `--extern` set the PRIMARY compile this file feeds was itself given
/// (this function has no visibility into that set, and reading one
/// `--extern` file's own declarations against another is cross-unit name
/// resolution, not this task's — docs/tmt/cli.md (compile)).
pub(crate) fn read_extern(path: &Path, source: &str) -> Result<Resolved, CompileError> {
    let mode = if path
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("tmh"))
    {
        ReadMode::DeclarationsOnly
    } else {
        ReadMode::Program
    };
    let analysis = compiler::analyze_with_mode(source, &Declarations::stdlib(), mode)?;
    Ok(analysis.resolved)
}

/// Render the exported declarations a compiled object still carries — the
/// reduced arm: routine signatures and exported alphabets, no graphs, no
/// maps, no doc lines (docs/formats.md (routine interfaces): the wire has
/// no doc-line field at all). A routine's tape parameter names its
/// alphabet through [`resolve_object_alphabet`]'s four-rule matching
/// order, printing a `use` line ahead of a namespace's own declarations
/// (matching the source arm's placement — see the module doc) whenever
/// that order resolves a tape through another namespace of this same
/// object or through an imported-alphabet record.
pub(crate) fn from_object(obj: &ObjectFile) -> Result<String, String> {
    let interface = obj
        .interface
        .as_ref()
        .ok_or_else(|| "carries no interface section".to_string())?;

    let mut root = NsNode::default();
    for alphabet in &interface.alphabets {
        let (ns, local) = split_ns(&alphabet.name);
        root.insert(&ns, alphabet_lines(local, &alphabet.glyphs, true));
    }

    // Per-namespace short-name claims (rule (1)'s own reachable names,
    // seeded lazily, plus every `use` this render decides to print) and
    // the `use` lines themselves, in first-claimed order —
    // `resolve_object_alphabet`'s own doc has the full collision rule.
    let mut claimed: HashMap<Vec<String>, HashMap<String, String>> = HashMap::new();
    let mut use_lines: Vec<(Vec<String>, Vec<String>)> = Vec::new();

    for symbol in &obj.symbols {
        // The entry world is skipped on the object arm: a `machine` block
        // always compiles to the literal symbol name `main`, and unlike an
        // exported routine it is never a CALLEE — nothing binds against
        // `main` or reads its own interface entry — so it publishes no
        // write set and has no declaration to render here. Printing it as
        // an "exported routine" would falsely claim it writes nothing,
        // when in truth nothing was ever asked.
        //
        // This skip is sound BY CONSTRUCTION, not just for a program that
        // happens to declare a `machine` block: `compiler.rs`'s
        // machine/`main`-name clash check is now UNCONDITIONAL — a
        // top-level `main` routine or graph is never legal in any unit,
        // with or without a `machine` block — so `symbol.name == "main"`
        // can only ever be the entry world, never a routine this arm
        // ought to have printed. A namespaced `ns::main` is unaffected:
        // its own mangled symbol is never the bare name `main`.
        if symbol.name == "main" {
            continue;
        }
        if let SymbolDef::Defined { blob } = symbol.def {
            let routine = interface.routines.get(blob as usize).ok_or_else(|| {
                format!("`{}`: no interface record for its own blob", symbol.name)
            })?;
            let (ns, local) = split_ns(&symbol.name);
            let mut params = Vec::with_capacity(routine.params.len());
            for ((param_name, glyphs), writes) in routine
                .params
                .iter()
                .zip(&routine.glyphs)
                .zip(&routine.writes)
            {
                let alphabet_name = match resolve_object_alphabet(
                    interface,
                    &ns,
                    &symbol.name,
                    param_name,
                    glyphs,
                    &mut claimed,
                    &mut use_lines,
                ) {
                    Ok(name) => name,
                    Err(synth) => {
                        // A tape whose alphabet no rule (1)/(2)/(3) match
                        // claims gets a synthesized one, declared at the
                        // top level — BEFORE this routine's own namespace
                        // block prints, since insertion order is print
                        // order and the routine itself is inserted next.
                        root.insert(&[], alphabet_lines(&synth, glyphs, false));
                        synth
                    }
                };
                params.push(tape_param_text(param_name, &alphabet_name, writes));
            }
            // The wire carries the exit COUNT and no names — a `state`
            // parameter's name is compile-time material the object never
            // holds (docs/formats.md (routine interfaces)) — so the exits
            // print positionally. A caller reading this header binds them
            // by position, which is exactly how the vector travels. The
            // minted names are freshened against the tape parameters this
            // routine already prints: a tape literally named `exit0` would
            // otherwise yield a signature naming one parameter twice,
            // which the strict reader rejects — a header that does not
            // re-parse.
            let mut taken: HashSet<String> = routine.params.iter().cloned().collect();
            for k in 0..routine.exits {
                params.push(format!("state {}", fresh_param_name(&mut taken, k)));
            }
            // `noreturn` reads straight off the wire's `returns` bit — the
            // one fact a bodiless header has no body to infer, so this arm
            // echoes it exactly as `routine.exits`/`writes` already do
            // (docs/formats.md (routine interfaces)).
            let noreturn = if routine.returns { "" } else { " noreturn" };
            let lines = vec![format!(
                "export routine {local}({}){noreturn};",
                params.join(", ")
            )];
            root.insert(&ns, lines);
        }
    }

    // `use` lines print ahead of a namespace's own declarations, exactly
    // like the source arm's own `NsNode::prepend` pass — every namespace
    // referenced here already exists in `root` by construction (a `use`
    // is only ever recorded alongside the routine that needed it, which
    // this loop has already inserted).
    for (ns, lines) in use_lines {
        root.prepend(&ns, lines);
    }

    let mut out = String::new();
    root.render(0, &mut out);
    Ok(out)
}

/// One tape's alphabet identifier on the object arm — the first rule that
/// matches, in order (docs/tmt/cli.md (interface)):
///
/// 1. An exported alphabet reachable UNQUALIFIED from the routine's own
///    namespace (its own, or any ENCLOSING one — an unqualified name
///    resolves outward through enclosing scopes,
///    docs/tmt/language.md (namespaces)) — printed by short name, no
///    `use` needed, exactly as before this rule had siblings.
/// 2. An exported alphabet in ANY OTHER namespace of this same object — a
///    `use <qualified name>;` line in the routine's own namespace (the
///    same placement and spelling the source arm's `use_line_text` gives
///    one), printed by short name. This is what makes std.tmc's
///    volatile-twin namespaces (`binaryNumbersVolatile`,
///    `binaryNumbersBareVolatile`), which import their representation
///    alphabet from a SIBLING namespace, match the source arm exactly.
/// 3. An alphabet this object IMPORTED from ANOTHER unit
///    (`Interface::imports`, docs/formats.md (routine interfaces)) —
///    likewise a `use <qualified name>;` line and the short name; no
///    local `alphabet` declaration exists for it here, the same way the
///    source arm's cross-unit `use` prints no local declaration either —
///    the header trusts the reader's OWN declarations table to resolve
///    it.
/// 4. Otherwise the SYNTHESIZED, deterministic private name
///    (`synthesized_alphabet_name`) — never an error, since a routine
///    over a private alphabet is legal.
///
/// Within (1)/(2)/(3), the first CONTENT match (by glyph list) wins, in
/// wire order — two exported alphabets (or two imports) sharing one
/// glyph list are genuinely indistinguishable from the object alone, and
/// this printer accepts that ambiguity rather than erroring on it, same
/// as before this rule had siblings.
///
/// A short-name COLLISION inside the routine's own namespace — a (2) or
/// (3) candidate whose short name is already claimed by a DIFFERENT full
/// path in that same namespace, whether claimed by an earlier (2)/(3)
/// `use` or already occupied by a (1) reachable declaration — is refused
/// rather than printed: `Err`, so the caller falls back to (4) for
/// whichever candidate lost the race, instead of emitting a `use` that
/// would shadow or be shadowed. `Ok` is returned both for a fresh claim
/// and for a REPEAT of the identical `(ns, full path)` pair (two tapes in
/// one namespace importing the same alphabet resolve to the same short
/// name without a duplicate `use` line).
fn resolve_object_alphabet(
    interface: &Interface,
    ns: &[String],
    routine_full_name: &str,
    param_name: &str,
    glyphs: &[String],
    claimed: &mut HashMap<Vec<String>, HashMap<String, String>>,
    use_lines: &mut Vec<(Vec<String>, Vec<String>)>,
) -> Result<String, String> {
    if !claimed.contains_key(ns) {
        let mut seed: HashMap<String, String> = HashMap::new();
        for a in reachable_alphabets(&interface.alphabets, ns) {
            seed.insert(short_name(&a.name).to_string(), a.name.clone());
        }
        claimed.insert(ns.to_vec(), seed);
    }

    if let Some(a) = reachable_alphabets(&interface.alphabets, ns)
        .into_iter()
        .find(|a| a.glyphs == glyphs)
    {
        return Ok(short_name(&a.name).to_string());
    }

    let other_alphabets: Vec<&ExportedAlphabet> = interface
        .alphabets
        .iter()
        .filter(|a| !ns.starts_with(&split_ns(&a.name).0))
        .collect();
    if let Some(a) = other_alphabets.into_iter().find(|a| a.glyphs == glyphs)
        && let Some(name) = try_claim_use(ns, &a.name, claimed, use_lines)
    {
        return Ok(name);
    }

    if let Some(imp) = interface.imports.iter().find(|imp| imp.glyphs == glyphs)
        && let Some(name) = try_claim_use(ns, &imp.name, claimed, use_lines)
    {
        return Ok(name);
    }

    Err(synthesized_alphabet_name(routine_full_name, param_name))
}

/// Every alphabet reachable UNQUALIFIED from a routine printed at `ns` —
/// rule (1) of [`resolve_object_alphabet`].
fn reachable_alphabets<'a>(
    alphabets: &'a [ExportedAlphabet],
    ns: &[String],
) -> Vec<&'a ExportedAlphabet> {
    alphabets
        .iter()
        .filter(|a| ns.starts_with(&split_ns(&a.name).0))
        .collect()
}

/// Bind `full`'s short name inside `ns`, or refuse on a collision —
/// [`resolve_object_alphabet`]'s own doc has the full rule. `None` means
/// `ns` already binds that short name to a DIFFERENT full path.
fn try_claim_use(
    ns: &[String],
    full: &str,
    claimed: &mut HashMap<Vec<String>, HashMap<String, String>>,
    use_lines: &mut Vec<(Vec<String>, Vec<String>)>,
) -> Option<String> {
    let short = short_name(full).to_string();
    let scope = claimed.entry(ns.to_vec()).or_default();
    match scope.get(&short) {
        Some(existing) if existing == full => Some(short),
        Some(_) => None,
        None => {
            scope.insert(short.clone(), full.to_string());
            match use_lines.iter_mut().find(|(n, _)| n == ns) {
                Some((_, lines)) => lines.push(format!("use {full};")),
                None => use_lines.push((ns.to_vec(), vec![format!("use {full};")])),
            }
            Some(short)
        }
    }
}

// ---------------------------------------------------------------------------
// A namespace tree: children print as `namespace NAME { … }` blocks in the
// order their first item was inserted, interleaved with this level's own
// items in that same first-seen order — reconstructing the nesting
// `Alphabet`/`Routine`/`Graph::ns` paths imply without a second pass over
// source spans.
// ---------------------------------------------------------------------------

#[derive(Default)]
struct NsNode {
    order: Vec<NsEntry>,
    children: HashMap<String, NsNode>,
}

enum NsEntry {
    /// One declaration's rendered lines, unindented — `NsNode::render`
    /// applies the namespace-depth indent uniformly.
    Item(Vec<String>),
    Child(String),
}

impl NsNode {
    fn insert(&mut self, ns: &[String], lines: Vec<String>) {
        let Some((head, rest)) = ns.split_first() else {
            self.order.push(NsEntry::Item(lines));
            return;
        };
        if !self.children.contains_key(head) {
            self.children.insert(head.clone(), NsNode::default());
            self.order.push(NsEntry::Child(head.clone()));
        }
        self.children
            .get_mut(head)
            .expect("just inserted")
            .insert(rest, lines);
    }

    /// Like [`insert`](Self::insert), but places its one `Item` BEFORE
    /// everything already in that scope rather than after — for a scope's
    /// `use` lines, which print ahead of its own declarations
    /// (docs/tmt/cli.md (interface)). Lazily creates the scope exactly
    /// as `insert` does, for the edge case of a namespace whose only
    /// printed content turns out to be its `use` lines.
    fn prepend(&mut self, ns: &[String], lines: Vec<String>) {
        let Some((head, rest)) = ns.split_first() else {
            self.order.insert(0, NsEntry::Item(lines));
            return;
        };
        if !self.children.contains_key(head) {
            self.children.insert(head.clone(), NsNode::default());
            self.order.push(NsEntry::Child(head.clone()));
        }
        self.children
            .get_mut(head)
            .expect("just inserted")
            .prepend(rest, lines);
    }

    fn render(&self, depth: usize, out: &mut String) {
        let pad = "  ".repeat(depth);
        for entry in &self.order {
            match entry {
                NsEntry::Item(lines) => {
                    for line in lines {
                        if line.is_empty() {
                            let _ = writeln!(out);
                        } else {
                            let _ = writeln!(out, "{pad}{line}");
                        }
                    }
                }
                NsEntry::Child(name) => {
                    let _ = writeln!(out, "{pad}namespace {name} {{");
                    self.children[name].render(depth + 1, out);
                    let _ = writeln!(out, "{pad}}}");
                }
            }
        }
    }
}

/// Split a mangled `a::b::c` name into its namespace path and local name;
/// an unnamespaced name splits to an empty path.
fn split_ns(full: &str) -> (Vec<String>, &str) {
    match full.rsplit_once("::") {
        Some((ns, short)) => (ns.split("::").map(String::from).collect(), short),
        None => (Vec::new(), full),
    }
}

fn short_name(full: &str) -> &str {
    full.rsplit_once("::").map_or(full, |(_, short)| short)
}

// ---------------------------------------------------------------------------
// Doc lines
// ---------------------------------------------------------------------------

/// `?` doc lines for one declaration, printed VERBATIM line-for-line — each
/// written `?` line becomes its own output line, never paragraph-joined —
/// with a blank `?` between paragraphs (the same shape a run of consecutive
/// `?` lines followed by a blank `?` line parses back into —
/// docs/tmt/language.md (doc lines and attention lines)). Reads
/// `Doc::paragraph_lines` (the per-line form) rather than `Doc::paragraphs`
/// (the space-joined form other consumers, like hover text, want) for
/// exactly this reason: `paragraphs` has already discarded the original
/// line breaks, so it cannot round-trip them. Attention lines (`!`) are out
/// of scope here (see the module doc).
fn doc_lines(doc: Option<&Doc>) -> Vec<String> {
    let mut lines = Vec::new();
    if let Some(doc) = doc {
        for (i, paragraph) in doc.paragraph_lines.iter().enumerate() {
            if i > 0 {
                lines.push("?".to_string());
            }
            for line in paragraph {
                lines.push(format!("? {line}"));
            }
        }
    }
    lines
}

// ---------------------------------------------------------------------------
// Source arm
// ---------------------------------------------------------------------------

fn render_source(
    program: &Program,
    resolved: &Resolved,
    footprint: &FootprintTable,
    returns: &HashMap<String, bool>,
) -> String {
    let worlds: HashMap<&str, &ResolvedWorld> = resolved
        .worlds
        .iter()
        .map(|w| (w.name.as_str(), w))
        .collect();

    // Every named map an EXPORTED graph body's binding args reach — the
    // only place a `with map NAME` reference can print at all, since a
    // routine's own body never prints (only its signature does). Reads
    // `resolved`'s own copies: `compiler::expand_named_maps` already
    // rewrote a resolved site's `SymMap::named` to the DECLARATION'S OWN
    // mangled name, so this is a direct membership test, never a second
    // resolution of the written text (`Program`'s own copies, which the
    // printer elsewhere reads for the WRITTEN spelling, are untouched by
    // that rewrite).
    let mut referenced_maps: HashSet<&str> = HashSet::new();
    for graph in &program.graphs {
        if graph.exported {
            let full = full_name(&graph.ns, &graph.name);
            resolved_map_refs(worlds[full.as_str()], &mut referenced_maps);
        }
    }

    // Every alphabet an EXPORTED routine or graph's tape parameter draws
    // from, by its mangled name — printed even when the alphabet itself is
    // not exported (a plain `alphabet`, not `export alphabet`; see the
    // module doc). A purely local routine's own alphabet never lands in
    // this set, so it still prints nothing, same as before this rule
    // existed.
    let mut referenced_alphabets: HashSet<&str> = HashSet::new();
    for routine in &program.routines {
        if routine.exported {
            let full = full_name(&routine.ns, &routine.name);
            for tape in &worlds[full.as_str()].tapes {
                referenced_alphabets.insert(tape.alphabet.as_str());
            }
        }
    }
    for graph in &program.graphs {
        if graph.exported {
            let full = full_name(&graph.ns, &graph.name);
            for tape in &worlds[full.as_str()].tapes {
                referenced_alphabets.insert(tape.alphabet.as_str());
            }
        }
    }
    // Every map this render will itself print (exported, or referenced
    // from a printed graph body) contributes its own two alphabets, the
    // same "referenced, printed even if not itself exported" rule.
    for map in &program.maps {
        let full = full_name(&map.ns, &map.name);
        if map.exported || referenced_maps.contains(full.as_str()) {
            let decl = &resolved.maps[full.as_str()];
            referenced_alphabets.insert(decl.src.as_str());
            referenced_alphabets.insert(decl.dst.as_str());
        }
    }

    // Every declaration this render will ITSELF print, by full qualified
    // name — an exported alphabet, an alphabet merely referenced (see
    // above), an exported routine, or an exported graph. This is the
    // "printed" half of the `use`-line rule (docs/tmt/cli.md
    // (interface)): a `use` line is printed only when this scope's
    // printed content references its name AND EITHER the header prints
    // its target OR the target lives in another unit (`needed_imports`'s
    // own doc carries that second half) — a SAME-unit target that is
    // neither printed here nor reached externally could not possibly
    // resolve when the header is read back, so it stays dropped.
    let mut printed_full_names: HashSet<String> = HashSet::new();
    for alphabet in &program.alphabets {
        let full = full_name(&alphabet.ns, &alphabet.name);
        if alphabet.exported || referenced_alphabets.contains(full.as_str()) {
            printed_full_names.insert(full);
        }
    }
    for map in &program.maps {
        let full = full_name(&map.ns, &map.name);
        if map.exported || referenced_maps.contains(full.as_str()) {
            printed_full_names.insert(full);
        }
    }
    for routine in &program.routines {
        if routine.exported {
            printed_full_names.insert(full_name(&routine.ns, &routine.name));
        }
    }
    for graph in &program.graphs {
        if graph.exported {
            printed_full_names.insert(full_name(&graph.ns, &graph.name));
        }
    }

    let mut root = NsNode::default();
    for alphabet in &program.alphabets {
        let full = full_name(&alphabet.ns, &alphabet.name);
        if !alphabet.exported && !referenced_alphabets.contains(full.as_str()) {
            continue;
        }
        let glyphs = &resolved
            .alphabets
            .get(&full)
            .expect("resolution guarantees every declared alphabet is resolved")
            .glyphs;
        root.insert(
            &alphabet.ns,
            alphabet_lines(&alphabet.name, glyphs, alphabet.exported),
        );
    }
    // Named maps: exported, or referenced from a printed graph body — the
    // same rule an alphabet gets. A map an exported graph names but this
    // unit does not itself declare (an imported one) has no declaration
    // to print here; its own `use` line covers it instead
    // (`needed_imports`, below).
    for map in &program.maps {
        let full = full_name(&map.ns, &map.name);
        let referenced = referenced_maps.contains(full.as_str());
        if !map.exported && !referenced {
            continue;
        }
        root.insert(&map.ns, map_lines(map, map.exported));
    }
    for routine in &program.routines {
        if !routine.exported {
            continue;
        }
        let full = full_name(&routine.ns, &routine.name);
        let world = worlds[full.as_str()];
        // Inferred for a bodied routine (`returns` was built from one),
        // echoed from the declared clause otherwise (`render_from_source`'s
        // own doc).
        let noreturn = if routine.has_body {
            !returns.get(&full).copied().unwrap_or(true)
        } else {
            routine.noreturn.is_some()
        };
        root.insert(
            &routine.ns,
            routine_lines(routine, world, resolved, footprint, noreturn),
        );
    }
    for graph in &program.graphs {
        if !graph.exported {
            continue;
        }
        let full = full_name(&graph.ns, &graph.name);
        let world = worlds[full.as_str()];
        root.insert(&graph.ns, graph_lines(graph, world, resolved, footprint));
    }

    // `use` lines print before a scope's own declarations (see the module
    // doc); this runs AFTER the three loops above so every scope they
    // touch already exists in `root`, and prepending only ever reorders
    // items WITHIN one scope, never which sibling namespace was created
    // first. Distinct import scopes, in first-appearance-in-`Program::
    // imports` order — that order is otherwise unobservable, since each
    // scope's own OWN import order is what matters and stays source-order
    // by construction (`needed_imports` filters `program.imports` in
    // place, without reordering it).
    let mut import_scopes: Vec<Vec<String>> = Vec::new();
    for import in &program.imports {
        if !import_scopes.contains(&import.ns) {
            import_scopes.push(import.ns.clone());
        }
    }
    // Every name THIS unit declares itself, mangled — alphabets, routines
    // and graphs alike. An import whose target is not in this set, in a
    // program that compiled at all, resolved through the declarations
    // table rather than through this unit's own declarations: it lives in
    // ANOTHER unit (docs/tmt/cli.md (interface)), which is the second half
    // of the `use`-line rule below.
    let local_names: HashSet<String> = program
        .alphabets
        .iter()
        .map(|a| full_name(&a.ns, &a.name))
        .chain(program.maps.iter().map(|m| full_name(&m.ns, &m.name)))
        .chain(program.routines.iter().map(|r| full_name(&r.ns, &r.name)))
        .chain(program.graphs.iter().map(|g| full_name(&g.ns, &g.name)))
        .collect();
    for ns in &import_scopes {
        let needed = needed_imports(
            ns,
            &program.imports,
            &program.maps,
            &program.routines,
            &program.graphs,
            &printed_full_names,
            &local_names,
        );
        if needed.is_empty() {
            continue;
        }
        let lines: Vec<String> = needed.iter().map(|imp| use_line_text(imp)).collect();
        root.prepend(ns, lines);
    }

    let mut out = String::new();
    root.render(0, &mut out);
    out
}

/// `use path[ as alias];` — the canonical spelling for one printed import
/// path (docs/tmt/cli.md (interface)).
fn use_line_text(import: &Import) -> String {
    let mut path = import.path.join("::");
    if let Some(alias) = &import.alias {
        path.push_str(" as ");
        path.push_str(alias);
    }
    format!("use {path};")
}

/// The imports declared exactly at `ns` that this render both NEEDS and
/// CAN reprint — two independent conditions, both required
/// (docs/tmt/cli.md (interface)): a `use` line is printed only when this
/// scope's printed content references its name AND EITHER the header
/// prints its target OR the target lives in ANOTHER unit — reached
/// through the declarations table, never through this unit's own
/// declarations (`local_names`), which is exactly how a genuinely
/// cross-unit alphabet reference (`use std::binaryNumbers::symbols;`
/// against the embedded stdlib, say) resolves.
///
/// - referenced: the bound short name (`Import::binding`) is used,
///   unqualified, by a PRINTED declaration in that same scope — a tape
///   signature's alphabet name, or, inside a printed `export graph`
///   body, a bare `graft`/`bind` target or a bare `call` target in a
///   rule's transition. Only EXPORTED routines/graphs are scanned: those
///   are the only ones this printer ever renders a signature or body
///   for, so a reference from something the header drops (a
///   non-exported world, or a routine's own dropped body) does not count.
/// - printed-or-external: the import's TARGET (`Import::full_path`) is
///   either one of `printed_full_names` — an exported alphabet, an
///   alphabet this same render prints because something exported
///   references it, an exported routine, or an exported graph — or
///   absent from `local_names` altogether. A program that compiled at
///   all and references a name neither declared locally nor printed here
///   must have resolved that name through the declarations table (this
///   unit has no other way to make it resolve), so the `use` line is the
///   only way the printed reference could ever reparse — it is kept
///   rather than dropped. A SAME-unit target that is neither printed nor
///   locally declared cannot occur (it would not have compiled), so this
///   is not a loophole for the "private target, same import" case the
///   printed-here rule alone already drops.
///
/// Source order preserved: `imports` is walked in its own (already
/// source-ordered) sequence, filtered rather than resorted.
fn needed_imports<'a>(
    ns: &[String],
    imports: &'a [Import],
    maps: &'a [MapDecl],
    routines: &[Routine],
    graphs: &[Graph],
    printed_full_names: &HashSet<String>,
    local_names: &HashSet<String>,
) -> Vec<&'a Import> {
    let mut referenced: HashSet<&str> = HashSet::new();
    for map in maps {
        if map.exported && map.ns.as_slice() == ns {
            if !map.src.contains("::") {
                referenced.insert(map.src.as_str());
            }
            if !map.dst.contains("::") {
                referenced.insert(map.dst.as_str());
            }
        }
    }
    for routine in routines {
        if routine.exported && routine.ns.as_slice() == ns {
            collect_sig_refs(&routine.sig, &mut referenced);
        }
    }
    for graph in graphs {
        if graph.exported && graph.ns.as_slice() == ns {
            collect_sig_refs(&graph.sig, &mut referenced);
            collect_graph_body_refs(graph, &mut referenced);
        }
    }
    imports
        .iter()
        .filter(|imp| {
            imp.ns.as_slice() == ns
                && referenced.contains(imp.binding())
                && (printed_full_names.contains(&imp.full_path())
                    || !local_names.contains(&imp.full_path()))
        })
        .collect()
}

/// Every tape parameter's alphabet name, exactly as WRITTEN in source
/// (`SigParamKind::Tape::alphabet` — the same string `sig_param_text`
/// prints), for one signature.
fn collect_sig_refs<'p>(sig: &'p Signature, out: &mut HashSet<&'p str>) {
    for param in &sig.params {
        if let SigParamKind::Tape { alphabet, .. } = &param.kind {
            out.insert(alphabet.as_str());
        }
    }
}

/// Every BARE (single-segment) reuse target a printed graph body spells
/// unqualified — its top-level `graft`/`bind` instances, plus any `call`
/// target inside a rule's transition. std.tmc's own exported graphs never
/// exercise the `call` case (a graph carries no `call` in this library's
/// design — see its own header comment — every cross-namespace call is a
/// plain routine's), but the printer stays correct for one regardless: a
/// bare `Transition::Call` target is exactly as printable, and exactly as
/// import-dependent, as a bare `graft` target.
fn collect_graph_body_refs<'p>(graph: &'p Graph, out: &mut HashSet<&'p str>) {
    for graft in &graph.grafts {
        if let [only] = graft.target.segments.as_slice() {
            out.insert(only.as_str());
        }
        collect_binding_map_refs(&graft.args, out);
    }
    for bind in &graph.binds {
        if let [only] = bind.target.segments.as_slice() {
            out.insert(only.as_str());
        }
        collect_binding_map_refs(&bind.args, out);
    }
    for state in &graph.states {
        for rule in &state.rules {
            if let Transition::Call { target, args, .. } = &rule.transition {
                if let [only] = target.segments.as_slice() {
                    out.insert(only.as_str());
                }
                collect_binding_map_refs(args, out);
            }
        }
    }
}

/// A binding-arg list's own `with map NAME` references, BARE
/// (unqualified) ones only — the same "single segment" rule
/// `collect_graph_body_refs` applies to a graft/bind/call target, over a
/// named map's joined reference text rather than a [`crate::parser::
/// QualName`]'s segments (`SymMap::named` stores the joined string; a
/// qualified one self-resolves and needs no `use` line).
fn collect_binding_map_refs<'p>(args: &'p [BindingArg], out: &mut HashSet<&'p str>) {
    for arg in args {
        if let BindingValue::Named { map: Some(m), .. } = &arg.value
            && let Some((name, _)) = &m.named
            && !name.contains("::")
        {
            out.insert(name.as_str());
        }
    }
}

/// Every named-map reference one RESOLVED world's own grafts, binds, and
/// direct calls reach, by MANGLED name — `resolved`'s own copies, whose
/// `SymMap::named` `compiler::expand_named_maps` already rewrote from the
/// written text to the declaration's own mangled identity, so this reads
/// that identity directly rather than re-resolving anything. Decides
/// whether a printed graph body's `with map NAME` needs its own
/// declaration printed alongside it (`render_source`), independently of
/// [`collect_binding_map_refs`], which reads the UNRESOLVED written text
/// off `Program` for the separate `use`-line decision.
fn resolved_map_refs<'a>(world: &'a ResolvedWorld, out: &mut HashSet<&'a str>) {
    fn note<'a>(args: &'a [BindingArg], out: &mut HashSet<&'a str>) {
        for arg in args {
            if let BindingValue::Named { map: Some(m), .. } = &arg.value
                && let Some((name, _)) = &m.named
            {
                out.insert(name.as_str());
            }
        }
    }
    for graft in &world.grafts {
        note(&graft.args, out);
    }
    for bind in &world.binds {
        note(&bind.args, out);
    }
    for call in &world.calls {
        if let ResolvedCallTarget::Routine { args, .. } = &call.target {
            note(args, out);
        }
    }
}

fn alphabet_lines(name: &str, glyphs: &[String], exported: bool) -> Vec<String> {
    let keyword = if exported {
        "export alphabet"
    } else {
        "alphabet"
    };
    vec![format!("{keyword} {name} {}", braced_list(glyphs))]
}

/// `{ … }` with the elements space-padded, or the bare `{}` a genuinely
/// empty set (a routine's `writes {}`, or an alphabet — never empty by
/// grammar, but the helper stays total) collapses to, matching how this
/// printer renders every brace-delimited glyph list.
fn braced_list(glyphs: &[String]) -> String {
    if glyphs.is_empty() {
        "{}".to_string()
    } else {
        format!("{{ {} }}", render_glyph_list(glyphs))
    }
}

fn routine_lines(
    routine: &Routine,
    world: &ResolvedWorld,
    resolved: &Resolved,
    footprint: &FootprintTable,
    noreturn: bool,
) -> Vec<String> {
    let mut lines = doc_lines(routine.doc.as_ref());
    let sig = signature_text(&routine.sig, world, resolved, footprint);
    let suffix = if noreturn { " noreturn" } else { "" };
    lines.push(format!("export routine {}({}){suffix};", routine.name, sig));
    lines
}

fn graph_lines(
    graph: &Graph,
    world: &ResolvedWorld,
    resolved: &Resolved,
    footprint: &FootprintTable,
) -> Vec<String> {
    let mut lines = doc_lines(graph.doc.as_ref());
    let sig = signature_text(&graph.sig, world, resolved, footprint);
    lines.push(format!("export graph {}({}) {{", graph.name, sig));
    for state in &graph.states {
        lines.extend(indented(state_lines(state)));
    }
    for graft in &graph.grafts {
        lines.push(indent_one(graft_text(graft)));
    }
    for bind in &graph.binds {
        lines.push(indent_one(bind_text(bind)));
    }
    lines.push("}".to_string());
    lines
}

fn signature_text(
    sig: &Signature,
    world: &ResolvedWorld,
    resolved: &Resolved,
    footprint: &FootprintTable,
) -> String {
    sig.params
        .iter()
        .map(|p| sig_param_text(p, world, resolved, footprint))
        .collect::<Vec<_>>()
        .join(", ")
}

fn sig_param_text(
    param: &SigParam,
    world: &ResolvedWorld,
    resolved: &Resolved,
    footprint: &FootprintTable,
) -> String {
    match &param.kind {
        SigParamKind::State => format!("state {}", param.name),
        // `volatile` is deliberately NOT printed here, on either arm: the
        // language reference states it plainly — the modifier is
        // compile-time-only, dropped at codegen, so "the generated
        // assembly carries no trace of it at all" (docs/tmt/language.md
        // (volatile tapes)). It fixes how the routine's OWN body compiles
        // and is never checked at a call site (binding a volatile machine
        // tape into a non-volatile-declared parameter "is not
        // diagnosed"), so it is not part of what a caller may rely on —
        // exactly the line the module doc draws for `preserves`. The
        // object arm has no wire bit to read it back from either way, so
        // dropping it here is what keeps the two arms in agreement over
        // std.tmc's volatile-twin routines.
        SigParamKind::Tape { alphabet, .. } => {
            let (index, tape) = world
                .tapes
                .iter()
                .enumerate()
                .find(|(_, t)| t.name == param.name)
                .expect("every signature tape parameter resolves to a ResolvedTape");
            // The SAME published write set `ir::lower` computes for this
            // tape (`compiler::published_writes`): the declared effective
            // set when a clause exists, else this world's own INFERRED
            // entry from the footprint this function was handed — never
            // `declared_effective` alone, which would ignore an
            // uncontracted tape's real inferred writes and diverge from
            // the object arm on every routine that has one
            // (docs/tmt/cli.md (interface)).
            let inferred = footprint
                .worlds
                .get(&world.name)
                .and_then(|wf| wf.tapes.get(index).copied());
            let published = published_writes(tape, inferred);
            let alphabet_glyphs = &resolved
                .alphabets
                .get(&tape.alphabet)
                .expect("resolution guarantees every tape alphabet is resolved")
                .glyphs;
            let writes: Vec<String> = published
                .iter()
                .filter_map(|index| alphabet_glyphs.get(index as usize).cloned())
                .collect();
            tape_param_text(&param.name, alphabet, &writes)
        }
    }
}

/// The positional name exit `k` prints under on the object arm: `exit<k>`
/// unless something already printed in this signature claims it, then
/// `exit<k>_1`, `exit<k>_2`, … Deterministic given the signature, and
/// distinct from every name beside it, so the rendered header re-parses.
fn fresh_param_name(taken: &mut HashSet<String>, k: u8) -> String {
    let base = format!("exit{k}");
    if taken.insert(base.clone()) {
        return base;
    }
    let mut i = 1;
    loop {
        let cand = format!("{base}_{i}");
        if taken.insert(cand.clone()) {
            return cand;
        }
        i += 1;
    }
}

/// One tape parameter's rendered text — `tape NAME: ALPHABET writes { … }`
/// — the ONE renderer both the source arm (`sig_param_text`) and the object
/// arm (`from_object`) call, so the two can never drift apart on how a
/// parameter is formatted, only on what write set they pass in (which
/// `compiler::published_writes` also unifies — see the module doc).
fn tape_param_text(name: &str, alphabet: &str, writes: &[String]) -> String {
    format!("tape {name}: {alphabet} writes {}", braced_list(writes))
}

fn state_lines(state: &State) -> Vec<String> {
    let mut lines = doc_lines(state.doc.as_ref());
    let entry = if state.entry { "entry " } else { "" };
    lines.push(format!("{entry}state {} {{", state.name));
    for rule in &state.rules {
        lines.push(indent_one(rule_text(rule)));
    }
    lines.push("}".to_string());
    lines
}

fn rule_text(rule: &Rule) -> String {
    let pattern = pattern_text(&rule.pattern);
    let mut action = Vec::new();
    if rule.debugger {
        action.push("debugger".to_string());
    }
    if let Some(write) = &rule.write {
        action.push(format!(
            "write [{}]",
            write
                .cells
                .iter()
                .map(|c| match &c.kind {
                    WriteCellKind::Keep => "-".to_string(),
                    WriteCellKind::Lit(s) => sym_lit_text(s),
                    WriteCellKind::Subst { expr } => format!("{{{}}}", fold_expr_text(expr)),
                })
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    if let Some(mov) = &rule.mov {
        action.push(format!(
            "move [{}]",
            mov.cells
                .iter()
                .map(|c| match c.dir {
                    MoveDir::Left => "<",
                    MoveDir::Right => ">",
                    MoveDir::Stay => ".",
                })
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    if let Some(text) = transition_text(&rule.transition) {
        action.push(text);
    }
    format!("{pattern} -> {};", action.join(" "))
}

fn pattern_text(pattern: &Pattern) -> String {
    format!(
        "[{}]",
        pattern
            .cells
            .iter()
            .map(|cell| {
                let base = match &cell.kind {
                    PatternCellKind::Wildcard => "*".to_string(),
                    PatternCellKind::Single(s) => sym_lit_text(s),
                    PatternCellKind::Range { lo, hi } => {
                        format!("{}..{}", sym_lit_text(lo), sym_lit_text(hi))
                    }
                };
                match &cell.binding {
                    Some(b) => format!("{base} as {}", b.name),
                    None => base,
                }
            })
            .collect::<Vec<_>>()
            .join(", ")
    )
}

fn sym_lit_text(sym: &SymLit) -> String {
    match sym {
        SymLit::Glyph { value, .. } => render_glyph_element(value),
        SymLit::Number { value, .. } => value.to_string(),
    }
}

/// A write-cell fold expression, precedence-climbed so parentheses appear
/// exactly where evaluation order would otherwise change (`*`/`%` bind
/// tighter than `+`/`-`; all four are left-associative —
/// docs/tmt/language.md (substitution)).
fn fold_expr_text(expr: &FoldExprNode) -> String {
    fold_expr_text_at(expr, 0, false)
}

fn fold_expr_text_at(expr: &FoldExprNode, min_prec: u8, is_right: bool) -> String {
    match &expr.kind {
        FoldExprKind::Var(name) => name.clone(),
        FoldExprKind::Int(n) => n.to_string(),
        FoldExprKind::Bin { op, lhs, rhs } => {
            let prec = fold_op_prec(*op);
            let lhs_text = fold_expr_text_at(lhs, prec, false);
            let rhs_text = fold_expr_text_at(rhs, prec, true);
            let text = format!("{lhs_text} {} {rhs_text}", fold_op_symbol(*op));
            if prec < min_prec || (prec == min_prec && is_right) {
                format!("({text})")
            } else {
                text
            }
        }
    }
}

fn fold_op_prec(op: FoldOp) -> u8 {
    match op {
        FoldOp::Add | FoldOp::Sub => 1,
        FoldOp::Mul | FoldOp::Rem => 2,
    }
}

fn fold_op_symbol(op: FoldOp) -> &'static str {
    match op {
        FoldOp::Add => "+",
        FoldOp::Sub => "-",
        FoldOp::Mul => "*",
        FoldOp::Rem => "%",
    }
}

/// `None` for `Transition::Stay` — an omitted transition prints nothing,
/// same as the source that produced it (docs/tmt/language.md (rules)).
fn transition_text(transition: &Transition) -> Option<String> {
    Some(match transition {
        Transition::Goto { name, .. } => format!("goto {name}"),
        // A graph body — the only place this printer renders a full rule —
        // never contains a `call` at all (`0.1` rejects a graft whose graph
        // body holds one), so `then` is always written when this arm does
        // fire; the `None` fallback exists only to keep the match total.
        Transition::Call {
            target, args, then, ..
        } => match then {
            Some(then) => format!(
                "call {}({}) then {}",
                target.joined(),
                binding_args_text(args),
                continuation_text(then)
            ),
            None => format!("call {}({})", target.joined(), binding_args_text(args)),
        },
        Transition::Return { .. } => "return".to_string(),
        Transition::Stop { .. } => "stop".to_string(),
        Transition::Halt { .. } => "halt".to_string(),
        Transition::Stay { .. } => return None,
    })
}

fn continuation_text(cont: &crate::parser::Continuation) -> String {
    use crate::parser::Continuation;
    match cont {
        Continuation::State { name, .. } => name.clone(),
        Continuation::Return { .. } => "return".to_string(),
        Continuation::Stop { .. } => "stop".to_string(),
        Continuation::Halt { .. } => "halt".to_string(),
    }
}

fn binding_args_text(args: &[BindingArg]) -> String {
    args.iter()
        .map(|a| format!("{} = {}", a.name, binding_value_text(&a.value)))
        .collect::<Vec<_>>()
        .join(", ")
}

fn binding_value_text(value: &BindingValue) -> String {
    match value {
        // A named reference (`SymMap::named`, `Some`) prints its own name
        // verbatim: this renderer reads the RAW, pre-resolution `Program`
        // (the module doc's determinism rule — never `Resolved`'s
        // world-order-dependent data for this), so a named map's `pairs`
        // are still empty here (`compiler::expand_named_maps` fills them
        // in later, for the compile pipeline, never for `Program` itself)
        // — printing them would be `with map {  }`, silently wrong.
        BindingValue::Named { target, map, .. } => match map {
            Some(m) => match &m.named {
                Some((name, _)) => format!("{target} with map {name}"),
                None => format!("{target} with map {{ {} }}", map_pairs_text(m)),
            },
            None => target.clone(),
        },
        BindingValue::Terminator { kind, .. } => match kind {
            TermKind::Return => "return".to_string(),
            TermKind::Stop => "stop".to_string(),
            TermKind::Halt => "halt".to_string(),
        },
    }
}

fn map_pairs_text(map: &SymMap) -> String {
    map_pair_list_text(&map.pairs)
}

/// One comma-`, `-joined pair list — shared by an inline `with map { … }`
/// ([`map_pairs_text`]) and a named map's own declared body
/// ([`map_lines`]).
fn map_pair_list_text(pairs: &[MapPair]) -> String {
    pairs
        .iter()
        .map(|pair| {
            let arrow = match pair.arrow {
                MapArrow::Bidirectional => "->",
                MapArrow::ReadOnly => "=>",
            };
            format!(
                "{} {arrow} {}",
                sym_lit_text(&pair.src),
                sym_lit_text(&pair.dst)
            )
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// One `export? map NAME: SRC -> DST { pairs }` — mirrors
/// [`alphabet_lines`]'s shape exactly, `exported` included: a map
/// referenced from a printed graph body but not itself exported prints
/// unqualified (`map NAME: …`), the same "referenced, printed even if not
/// itself exported" rule an alphabet gets (see the caller in
/// [`render_source`]). This renderer never wraps a body across lines,
/// matching every other declaration here.
fn map_lines(map: &MapDecl, exported: bool) -> Vec<String> {
    let keyword = if exported { "export map" } else { "map" };
    let mut lines = doc_lines(map.doc.as_ref());
    let pairs = if map.pairs.is_empty() {
        "{}".to_string()
    } else {
        format!("{{ {} }}", map_pair_list_text(&map.pairs))
    };
    lines.push(format!(
        "{keyword} {}: {} -> {} {pairs}",
        map.name, map.src, map.dst
    ));
    lines
}

fn graft_text(graft: &Graft) -> String {
    let entry = if graft.entry { "entry " } else { "" };
    let as_part = graft
        .as_name
        .as_ref()
        .map(|n| format!(" as {}", n.name))
        .unwrap_or_default();
    format!(
        "{entry}graft {}({}){as_part};",
        graft.target.joined(),
        binding_args_text(&graft.args)
    )
}

fn bind_text(bind: &Bind) -> String {
    format!(
        "bind {}({}) as {};",
        bind.target.joined(),
        binding_args_text(&bind.args),
        bind.as_name.name
    )
}

fn indent_one(line: String) -> String {
    format!("  {line}")
}

fn indented(lines: Vec<String>) -> Vec<String> {
    lines.into_iter().map(indent_one).collect()
}

// ---------------------------------------------------------------------------
// Object arm
// ---------------------------------------------------------------------------

/// A deterministic stand-in name for a tape's alphabet when nothing this
/// object exports or imports has matching glyph content
/// ([`resolve_object_alphabet`]'s rule (4)): the routine's own mangled
/// name with `::` replaced by `_`, joined to the parameter name by `__`
/// (docs/tmt/cli.md (interface)). Two different routines can never
/// collide on this scheme — their mangled names differ — and reusing it
/// consistently between the declaration `from_object` inserts and the
/// reference `resolve_object_alphabet` returns is what keeps the two in
/// sync without passing the synthesized name across the two sites
/// directly.
fn synthesized_alphabet_name(routine_full_name: &str, param_name: &str) -> String {
    format!("{}__{param_name}", routine_full_name.replace("::", "_"))
}
