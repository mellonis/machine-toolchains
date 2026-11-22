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
//! **The printer never emits `never writes`.** A contract clause always
//! prints as `writes { … }` carrying the tape's PUBLISHED write set
//! (`compiler::published_writes`, the one function both `ir::lower` and
//! this module call): the declared EFFECTIVE set when the tape declares
//! `writes` or `never writes`, or — when neither clause is written — the
//! compiler's own INFERRED write set for that tape, never the whole
//! alphabet as a stand-in for "no restriction declared" (the wire has no
//! way to spell that — docs/formats.md (routine interfaces)). `never writes`
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
//! digest be computed over this printer's own output. The digest is
//! taken over [`graph_body_lines`], BEFORE the one layout step the
//! printed header alone goes through — [`canonical`], which hands the
//! assembled text to the `.tmc` formatter so that `tmt fmt --check`
//! accepts a generated header unchanged. Two determinism
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
//! at that namespace, reprints as `use path[ as alias];` iff something
//! this render prints RESOLVES THROUGH IT — a tape signature's alphabet
//! name, or (inside a printed `export graph` body) a bare `graft`/`bind`
//! target or a bare `call` target in a rule's transition, each resolved
//! from its own declaration's namespace through the compiler's own scope
//! walk, so that an import written at an OUTER scope and referenced only
//! from a nested one is kept (`used_import_indices`) — AND EITHER the header
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

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use mtc_core::diagnostics::Span;
use mtc_core::formats::crc32::crc32;
use mtc_core::formats::object::{
    ExportedAlphabet, ExportedGraph, Interface, ObjectFile, SymbolDef,
};

use crate::codegen::{render_glyph_element, render_glyph_list};
use crate::compiler::{
    self, CompileError, CompileErrorKind, PeerChecks, Provenance, ReadMode, Resolved,
    ResolvedCallTarget, ResolvedWorld, WorldKind, clause_glyphs, full_name, position_named_cells,
    published_writes, symset_glyphs,
};
use crate::declarations::{Declarations, Origin};
use crate::footprint::{self, FootprintTable};
use crate::parser::{
    Bind, BindingArg, BindingValue, Doc, FoldExprKind, FoldExprNode, FoldOp, Graft, Graph, Import,
    MapArrow, MapDecl, MapPair, MoveDir, Pattern, PatternCellKind, Program, Routine, Rule,
    SigParam, SigParamKind, Signature, State, SymLit, SymMap, TermKind, Transition, WriteCellKind,
};

/// Render every exported declaration of a `.tmc` source as a header — the
/// complete arm. `externals` is the declarations table a library that
/// itself depends on another unit's alphabet, map or graph needs
/// (`--extern`/`--nostdlib` on `tmt interface`, docs/tmt/cli.md
/// (interface)) — callers with no such dependency pass
/// `&Declarations::stdlib()`, the implicit default every other reader in
/// this module already assumes.
pub(crate) fn from_source(source: &str, externals: &Declarations) -> Result<String, CompileError> {
    render_from_source(source, ReadMode::Program, externals)
}

/// [`from_source`]'s declarations-only twin: the SAME reader, in
/// [`ReadMode::DeclarationsOnly`] (docs/tmt/language.md (headers)) — a
/// `.tmh`, or a `.tmc` read as one (`tmt interface`'s own extension rule).
/// Every routine is bodiless and every graph carries its body, so
/// rendering it back reproduces the identical text `from_source` would
/// have printed for the program it was itself rendered from — the
/// round-trip `tmt interface` promises. See [`from_source`] for
/// `externals`.
pub(crate) fn from_declarations(
    source: &str,
    externals: &Declarations,
) -> Result<String, CompileError> {
    render_from_source(source, ReadMode::DeclarationsOnly, externals)
}

/// The shared body of [`from_source`]/[`from_declarations`]: the mode is a
/// flag on this ONE reader, not a fork — same lexer, same green parse,
/// same `extract_program` either way (docs/tmt/language.md (headers)).
fn render_from_source(
    source: &str,
    mode: ReadMode,
    externals: &Declarations,
) -> Result<String, CompileError> {
    // The SAME externals `compiler::analyze` resolves against and the SAME
    // inference `ir::lower` runs. Both go through
    // `compiler::published_writes`, the one function that decides a tape's
    // published write set (docs/tmt/cli.md (interface)).
    let analysis = compiler::analyze_with_mode(source, externals, mode)?;
    let footprint = footprint::infer_resolved_with(&analysis.resolved, &externals.modules());
    let returns = inferred_returns(&analysis.program, &analysis.resolved, externals);
    // The same scope substrate `resolve_program` built while analyzing this
    // very source — rebuilt here from the same `Program` rather than
    // threaded out of the analysis, so nothing on the compile path grows a
    // field for the printer's sake. It is a pure function of the AST, so
    // building it twice is one implementation run twice; the `?` is
    // unreachable in practice, `analyze_with_mode` having just succeeded
    // through the identical call.
    let scopes = compiler::Scopes::build(&analysis.program)?;
    let text = render_source(
        &analysis.program,
        &analysis.resolved,
        &scopes,
        &footprint,
        &returns,
    );
    // The renderer's own spans point into the header it just printed, not
    // into `source`, so a failure is reported without one: it is a printer
    // bug, never a fault in the input.
    canonical(&text).map_err(|message| CompileError {
        span: Span::point(0, 0),
        kind: CompileErrorKind::Internal(message),
    })
}

/// Every BODIED routine's inferred "can return" fact, keyed by mangled
/// name — the ONE inference both a header's source arm
/// ([`render_from_source`]) and a lenient declarations read
/// ([`read_declarations_with_mode`]) take `noreturn` from, so `tmt
/// interface` and `tmt build`'s sibling read can never disagree on it
/// (docs/tmt/project.md (Declaration derivation)). It runs the identical
/// `ir::body_can_return` the compiler itself runs over a freshly-expanded
/// module — never a second walk. [`ReadMode::DeclarationsOnly`] gives
/// every routine an EMPTY body (a header has none to infer from), so
/// `expand::expand` is skipped there entirely and the map is empty.
///
/// A declarations reader answers what a unit DECLARES, not what it
/// compiles to — `analyze` (resolution) is as far as it otherwise goes,
/// and expansion errors (a descending range, a graft map conflict, …) are
/// strictly LATER than that. A unit with such an error in one routine must
/// still declare every OTHER routine's signature, so a failed expansion
/// here is not propagated: the map is left EMPTY instead, and
/// [`noreturn_fact`] falls back to each routine's DECLARED clause — the
/// identical fallback a bodiless routine always takes, so the two failure
/// modes share one path rather than needing a second.
fn inferred_returns(
    program: &Program,
    resolved: &Resolved,
    externals: &Declarations,
) -> HashMap<String, bool> {
    if !program.routines.iter().any(|r| r.has_body) {
        return HashMap::new();
    }
    crate::expand::expand(resolved, externals)
        .map(|expanded| {
            expanded
                .worlds
                .iter()
                .map(|w| (w.name.clone(), crate::ir::body_can_return(w, resolved)))
                .collect()
        })
        .unwrap_or_default()
}

/// Whether routine `name` is `noreturn`: INFERRED when its fact is in
/// `returns` ([`inferred_returns`] — a bodied routine whose unit
/// expanded); echoed from the DECLARED clause otherwise — a bodiless
/// routine has no body to infer from at all, and a bodied one whose unit
/// failed to expand has no provable fact either, so the author's own
/// assertion is the best available answer, and its absence conservatively
/// means "may return" rather than guessing. The one rule both the header
/// printer and a lenient declarations read apply.
fn noreturn_fact(returns: &HashMap<String, bool>, name: &str, declared: bool) -> bool {
    match returns.get(name) {
        Some(&can_return) => !can_return,
        None => declared,
    }
}

/// The printed header's canonical layout: the assembled text passed once
/// through `crate::fmt::format`, the same function `tmt fmt` runs, so a
/// generated header is `fmt`-clean by construction rather than by a second
/// printer imitating the formatter's grid and wrapping (docs/tmt/fmt.md
/// (the state-block grid), docs/tmt/fmt.md (argument lists and the width
/// threshold)).
/// Both arms end here — [`render_from_source`] and [`from_object`].
///
/// Only the PRINTED text goes through it. [`graph_digest`] hashes
/// [`graph_body_lines`]' own rendering and never this output, so a change
/// in the formatter's layout never moves a graft digest.
///
/// A header carries no comments, so the formatter's comment-sensitive
/// layout rules never arise. An error here means the renderer produced
/// text the `.tmc` grammar rejects — surfaced, never papered over by
/// printing the unformatted text.
fn canonical(text: &str) -> Result<String, String> {
    crate::fmt::format(text).map_err(|e| {
        format!(
            "the generated header does not re-read at {}:{}: {}",
            e.span.start.line, e.span.start.col, e.kind
        )
    })
}

/// Read one text declaration source's own shape — a `.tmc`/`.tmh` file
/// given to `tmt compile --extern`, `tmt interface --extern`, or one of
/// `tmt build`'s own sibling sources — against a CALLER-SUPPLIED
/// declarations context (docs/tmt/project.md (Declaration derivation)).
/// A `.tmh` extension (case-insensitive, matching `cli/interface.rs`'s
/// identical rule) selects STRICT reading — [`ReadMode::DeclarationsOnly`],
/// which rejects a routine body or a `machine` block outright, but KEEPS
/// every graph's body (a graph's only form is its source, so a header
/// cannot omit it — this is what makes a sibling's or a library's
/// exported graph graftable); anything else (a `.tmc`) is read LENIENTLY
/// as [`ReadMode::Program`]: bodies and a `machine` block are accepted
/// and a routine's body contributes what its generated header would print
/// ([`settle_declarations`]) — its `noreturn`, inferred exactly as `tmt
/// interface` infers it ([`inferred_returns`]), and its uncontracted
/// tapes' inferred write sets — while a graph's body is read and kept exactly as
/// the strict arm keeps it. Text has no container magic to tell a header
/// from a full source by, so — exactly as in `cli/interface.rs` — the
/// extension is the one place it IS the signal, never a second front end.
///
/// `externals` is never a fixed default: the compiler's own module-
/// resolution stage — cross-unit alphabet resolution AND the write-
/// contract check — runs during declarations-only extraction exactly as
/// it does during a real compile, so a source that itself references
/// another declared unit needs that unit's declarations in hand just to
/// extract its OWN shape. [`resolve_declarations`] is what builds this
/// context, growing it as more sources resolve.
#[cfg(test)]
pub(crate) fn read_extern(
    path: &Path,
    source: &str,
    externals: &Declarations,
) -> Result<Resolved, CompileError> {
    read_extern_with(path, source, externals, PeerChecks::Enforced)
}

/// [`read_extern`] with an explicit [`PeerChecks`] — what
/// [`resolve_declarations`] reads a text source through.
fn read_extern_with(
    path: &Path,
    source: &str,
    externals: &Declarations,
    checks: PeerChecks,
) -> Result<Resolved, CompileError> {
    read_declarations_with_mode(
        source,
        header_mode_for(path),
        externals,
        Provenance::Text,
        checks,
    )
}

/// `.tmh` (case-insensitive) selects STRICT [`ReadMode::DeclarationsOnly`]
/// reading; anything else (a `.tmc`) reads LENIENTLY as [`ReadMode::
/// Program`] — the one place [`read_extern`] decides which.
fn header_mode_for(path: &Path) -> ReadMode {
    if path
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("tmh"))
    {
        ReadMode::DeclarationsOnly
    } else {
        ReadMode::Program
    }
}

/// The shared body of [`read_extern`] and [`declarations_from_object`]'s
/// own strict read: mode AND the declarations context are both flags on
/// this ONE reader, exactly as [`render_from_source`] shares one reader
/// between [`from_source`]/[`from_declarations`].
fn read_declarations_with_mode(
    source: &str,
    mode: ReadMode,
    externals: &Declarations,
    provenance: Provenance,
    checks: PeerChecks,
) -> Result<Resolved, CompileError> {
    let analysis = compiler::analyze_reading(source, externals, mode, provenance, checks)?;
    let mut resolved = analysis.resolved;
    settle_declarations(&analysis.program, &mut resolved, externals, mode);
    Ok(resolved)
}

/// Settle what a declarations reading of `program` publishes
/// (docs/tmt/language.md (declarations)): stamp every graph's digest,
/// settle every routine's `noreturn` from its body, and — for a
/// [`ReadMode::Program`] read only — publish each routine/graph tape's
/// write set as the header prints it. The ONE place these facts are
/// settled: a declarations reading publishes them through it, and the
/// own-definition check (`published::own_forms`) runs it on a copy of the
/// unit being compiled, so a unit is compared with its table as the
/// unit's own reading would be published.
pub(crate) fn settle_declarations(
    program: &Program,
    resolved: &mut Resolved,
    externals: &Declarations,
    mode: ReadMode,
) {
    // Every graph world DOES carry its body (see the mode doc above), so
    // this is exactly where a later graft of one needs its digest to come
    // from — this module's own AST is about to be dropped, and `Resolved`
    // alone carries no way to recompute it (`stamp_graph_digests`).
    let footprint = footprint::infer_resolved_with(resolved, &externals.modules());
    // A routine's `noreturn` is the one fact a consumer reads off a
    // routine world that a lenient `.tmc` read would otherwise lose: the
    // body is about to be dropped with the AST, and `ir::known_noreturn`
    // believes only `declared_noreturn` for an out-of-unit callee. So the
    // fact is settled HERE, by the same inference and the same rule `tmt
    // interface` prints it by — a strict read (a header, or an object's
    // rendered one) has no bodies, `returns` is empty, and every world
    // keeps its declared clause untouched.
    let returns = inferred_returns(program, resolved, externals);
    for world in resolved
        .worlds
        .iter_mut()
        .filter(|w| w.kind == WorldKind::Routine)
    {
        let noreturn = noreturn_fact(&returns, &world.name, world.declared_noreturn.is_some());
        // An inferred fact with no clause behind it carries the routine's
        // own name as its span — the one place a diagnostic about the
        // routine's return behaviour can point to.
        world.declared_noreturn =
            noreturn.then(|| world.declared_noreturn.unwrap_or(world.name_span));
    }
    // Stamped BEFORE the write sets below are rewritten: the digest is
    // computed from the tapes as the source declared them (the printed
    // signature goes through `published_writes` either way, so the order
    // cannot change a digest; stamping first makes that independent of it).
    stamp_graph_digests(program, resolved, &footprint);
    // A routine's or graph's tape publishes the set its header would
    // print (`published_writes`): the declared effective set where a
    // clause exists, else the body's inferred set. Never for a header
    // read: a bodiless routine's inferred set is empty, and publishing it
    // would claim the routine writes nothing.
    if mode == ReadMode::Program {
        for world in resolved
            .worlds
            .iter_mut()
            .filter(|w| w.kind != WorldKind::Machine)
        {
            for i in 0..world.tapes.len() {
                let inferred = footprint
                    .worlds
                    .get(&world.name)
                    .and_then(|wf| wf.tapes.get(i).copied());
                let tape = &mut world.tapes[i];
                tape.writes = Some(published_writes(tape, inferred));
                tape.never_writes = None;
            }
        }
    }
}

/// The ONE path from a compiled object to a [`Declarations`] module
/// (docs/tmt/project.md (Declaration derivation)): render the object's
/// header text through [`from_object`] — the identical rendering `tmt
/// interface` prints for a `.tmo` input — and read it back through the
/// SAME strict reader (`read_declarations_with_mode`, [`ReadMode::
/// DeclarationsOnly`]) a real `.tmh` file goes through, against the SAME
/// caller-supplied context [`read_extern`] takes. Never a second,
/// hand-rolled object→declarations converter: an object's declared
/// routine signatures and exported alphabets reach the table exactly as
/// they would if a human had copied `tmt interface`'s own output into a
/// `.tmh` by hand — and, like a real header, this object-derived one
/// carries no graph body of its own, since the wire has none to read
/// back (`docs/formats.md (routine interfaces)`); a library shipping
/// both a header and an object is trusted on the header for exactly this
/// reason.
///
/// An object carrying NO interface section at all (an `an_object_without_
/// interface_content_carries_none`-shaped `.tma`/`.tmo` with no `.routine`/
/// `.graph` directives — the ordinary shape of a sibling or library that
/// exports nothing) yields an EMPTY declarations module rather than
/// [`from_object`]'s own "carries no interface section" refusal: that
/// refusal answers a user's DIRECT `tmt interface` request, where an
/// object with nothing to show is worth naming; here, feeding an object
/// with nothing to declare to another unit is not a mistake at all — it
/// declares nothing because it exports nothing.
#[cfg(test)]
pub(crate) fn declarations_from_object(
    obj: &ObjectFile,
    externals: &Declarations,
) -> Result<Resolved, UnresolvedSource> {
    declarations_from_object_with(obj, externals, PeerChecks::Enforced)
}

/// [`declarations_from_object`] with an explicit [`PeerChecks`] — what
/// [`resolve_declarations`] reads an object source through.
fn declarations_from_object_with(
    obj: &ObjectFile,
    externals: &Declarations,
    checks: PeerChecks,
) -> Result<Resolved, UnresolvedSource> {
    if obj.interface.is_none() {
        return Ok(empty_resolved());
    }
    let text = from_object(obj).map_err(|message| UnresolvedSource {
        message,
        code: OBJECT_INTERFACE_UNREADABLE,
    })?;
    read_declarations_with_mode(
        &text,
        ReadMode::DeclarationsOnly,
        externals,
        Provenance::Object,
        checks,
    )
    .map(|mut resolved| {
        resolved.from_object = true;
        resolved
    })
    .map_err(|e| UnresolvedSource {
        message: format!(
            "{}:{}: error: {} [{}]",
            e.span.start.line,
            e.span.start.col,
            e.kind,
            e.kind.code()
        ),
        code: e.kind.code(),
    })
}

/// A sentinel [`UnresolvedSource::code`] for the one `declarations_from_
/// object` failure that never comes from the compiler's own diagnostics
/// — an object whose interface section cannot even be rendered back to
/// text (`from_object`'s defensive "no interface record for its own
/// blob" refusal, unreachable outside a hand-corrupted object). It
/// shares none of the four "declarations were/are not given" codes and
/// is never `writes-outside-contract`, so a caller classifying by code
/// (`cli/build.rs::render_unresolved`) always treats it as a genuine
/// defect of its own source, never as a symptom of another source's
/// failure.
const OBJECT_INTERFACE_UNREADABLE: &str = "object-interface-unreadable";

/// One source's own failure to resolve inside [`resolve_declarations`]'s
/// shared fixpoint: a fully rendered `message` — the one a user reads,
/// already carrying its own path — paired with the diagnostic `code` a
/// caller classifies and orders by (`cli/build.rs::render_unresolved`,
/// docs/tmt/project.md (Declaration derivation)), so that classification
/// never has to parse prose out of `message`.
#[derive(Debug, Clone)]
pub(crate) struct UnresolvedSource {
    pub message: String,
    pub code: &'static str,
}

/// One member of the shared fixpoint [`resolve_declarations`] runs: text
/// (a `.tmc`/`.tmh` file, read through [`read_extern`]) or an already-
/// built object (its own interface section, through
/// [`declarations_from_object`]). Reading is deferred — a
/// `DeclarationSource` only carries what it needs to read itself once a
/// context is available, never a `Resolved` up front.
#[derive(Debug)]
pub(crate) enum DeclarationText {
    Source {
        path: PathBuf,
        text: String,
    },
    // Boxed: an `ObjectFile` dwarfs the `Source` variant, and this enum
    // travels in `Vec<DeclarationSource>` — one per build source — so an
    // unboxed object would inflate every entry to the largest variant's
    // size regardless of which one it actually holds.
    Object {
        path: PathBuf,
        object: Box<ObjectFile>,
    },
}

/// One declaration source taking part in [`resolve_declarations`]'s
/// shared fixpoint, tagged with the [`Origin`] its declarations are
/// pushed under — both while it is a PEER another source reads against
/// during the fixpoint, and in the final table the caller assembles
/// afterward from the same results.
pub(crate) struct DeclarationSource {
    pub origin: Origin,
    pub text: DeclarationText,
}

impl DeclarationSource {
    /// The file this source was read from, for a diagnostic.
    fn path(&self) -> &Path {
        match &self.text {
            DeclarationText::Source { path, .. } | DeclarationText::Object { path, .. } => path,
        }
    }

    fn read(
        &self,
        externals: &Declarations,
        checks: PeerChecks,
    ) -> Result<Resolved, UnresolvedSource> {
        match &self.text {
            DeclarationText::Source { path, text } => {
                read_extern_with(path, text, externals, checks).map_err(|e| UnresolvedSource {
                    message: format!(
                        "{}:{}:{}: error: {} [{}]",
                        path.display(),
                        e.span.start.line,
                        e.span.start.col,
                        e.kind,
                        e.kind.code()
                    ),
                    code: e.kind.code(),
                })
            }
            DeclarationText::Object { path, object } => {
                declarations_from_object_with(object, externals, checks).map_err(|inner| {
                    UnresolvedSource {
                        message: format!("{}: {}", path.display(), inner.message),
                        code: inner.code,
                    }
                })
            }
        }
    }

    /// One read of this source against `externals`, as the settling loop
    /// of [`resolve_declarations`] takes it: the reading (when one can be
    /// had) and the verdict of the full, checked read (`None` when it read
    /// clean). The checked read comes first, so a source that reads clean
    /// is read exactly once. A refusal by one of the peer-dependent checks
    /// (`compiler::PEER_CHECK_CODES`) does not cost the source its
    /// reading: those checks change nothing a reading publishes, so the
    /// source is read again with them deferred and its peers still see
    /// what it declares; whether the refusal stands is decided by the
    /// read made against the settled set.
    fn read_settling(
        &self,
        externals: &Declarations,
    ) -> (Option<Resolved>, Option<UnresolvedSource>) {
        match self.read(externals, PeerChecks::Enforced) {
            Ok(reading) => (Some(reading), None),
            Err(refusal) if compiler::PEER_CHECK_CODES.contains(&refusal.code) => (
                self.read(externals, PeerChecks::Deferred).ok(),
                Some(refusal),
            ),
            Err(refusal) => (None, Some(refusal)),
        }
    }
}

/// What [`resolve_declarations`] did, for the tests that pin its cost.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct SettleStats {
    /// Sweeps over the sources.
    pub passes: usize,
    /// Checked reads made (a deferred re-read after a provisional refusal
    /// is not counted separately).
    pub reads: usize,
    /// The largest sweep guard computed — `passes` stays at or below it
    /// unless the readings failed to settle.
    pub guard: usize,
    /// Readings that REPLACED an earlier reading of their source without
    /// narrowing it ([`narrowing_left`] did not drop). The sweep guard
    /// assumes there are none; a test holds this at zero over every
    /// fixture, so a field that starts to change on its own shows up.
    pub non_narrowing_changes: usize,
}

/// A sentinel [`UnresolvedSource::code`] for a set of sources whose
/// declarations did not settle within the pass limit of
/// [`resolve_declarations`] — never one of the compiler's own codes.
const DECLARATIONS_UNSETTLED: &str = "declarations-unsettled";

/// Read every source in `sources` against the declarations of all the
/// OTHERS, until what they publish stops changing
/// (docs/tmt/project.md (Declaration derivation)). The result does not
/// depend on the order the sources are given in.
///
/// What a source publishes depends on its peers: a `.tmc` read publishes
/// each uncontracted tape's INFERRED write set and each routine's inferred
/// `noreturn` ([`settle_declarations`]), and both inferences read the
/// callees' declarations — a callee nobody has declared yet is assumed to
/// write the whole alphabet and to return. So a source is read again
/// whenever a peer's reading has changed since its last read, each read
/// against the latest reading of EVERY other source that has one (then
/// the embedded standard library, pushed last unless `stdlib` is false —
/// the final table's own order, so a conflict names the same origins in
/// either place), and its new reading REPLACES the old one: one source
/// contributes exactly one reading. The inferences only narrow as
/// declarations arrive (a write set shrinks, `noreturn` goes from unknown
/// to known), so the readings settle, to the same result whatever the
/// order: with no cycle of calls between the sources each sweep settles
/// at least one more, so `sources.len() + 1` sweeps suffice; a cycle of
/// calls through maps that shift glyphs can lose one glyph per trip and
/// take longer. The loop simply runs until nothing is due a read. The
/// narrowing has one corner this function cannot rule out: a unit whose
/// expansion fails only against a richer table makes [`inferred_returns`]
/// fall back to the routine's declared `noreturn` clause, which can widen
/// a reading instead of narrowing it. No input that reaches that corner
/// is known. Its only guard is a safety net for the monotonicity it
/// relies on, and no input known to reach it: every sweep but the last changes a reading,
/// and a change is a first reading or a narrowing, so the sweeps cannot
/// exceed the number of sources plus what the readings in hand can still
/// lose ([`settle_height`]). Past that, every source still due a read
/// fails with an internal-consistency error rather than looping or being
/// accepted unsettled.
///
/// A read made before the set has settled can be refused for a reason the
/// settled set removes — a caller's `writes` contract violated only
/// because its callee had not yet been read against ITS callee, or two
/// readings of one unit that disagree only until the later one narrows.
/// Such a refusal (`compiler::PEER_CHECK_CODES`) is PROVISIONAL: the
/// source keeps a reading taken with those checks deferred
/// ([`DeclarationSource::read_settling`]), and its verdict is the one its
/// last read gave — which, once nothing is due a read, was made against
/// the settled set. A refusal that persists there is reported as before;
/// every other error (syntax, an unresolved name) costs the source its
/// new reading at once, and an earlier reading of it, if any, stays for
/// its peers.
///
/// This is what lets a library header depend on another library, a
/// sibling on another sibling, or an `--extern` file on another `--extern`
/// file, of any dependency depth and regardless of the order they were
/// given in; the FINAL table's assembly (siblings, then libraries in `-l`
/// order, then stdlib; or `--extern` files in command-line order, then
/// stdlib) happens separately, in the caller, from these same results.
///
/// Returns one outcome per input source, same order, same length: `Ok`
/// with its settled reading, or `Err` with the error of its own LAST read
/// — naming its own path. Two sources that each need the OTHER's
/// declarations merely to be read (each naming an alphabet the other
/// declares, say) never get a reading; both come back `Err`
/// (docs/tmt/project.md (Declaration derivation) — give one of them a
/// hand-written header instead). The caller — never this function —
/// decides how to render several such `Err`s together
/// (`cli/build.rs::render_unresolved`).
///
/// Cost: a source that reads clean and whose peers then stay as they are
/// is read once — a lone source is read exactly once. Otherwise each
/// change of a peer costs one more read.
pub(crate) fn resolve_declarations(
    sources: &[DeclarationSource],
    stdlib: bool,
) -> Vec<Result<Resolved, UnresolvedSource>> {
    resolve_declarations_counted(sources, stdlib).0
}

/// How many times one reading can still narrow: one step per routine or
/// graph (its `noreturn`) and one per glyph of each of its tapes (its
/// published write set).
fn settle_height(reading: &Resolved) -> usize {
    reading
        .worlds
        .iter()
        .filter(|w| w.kind != WorldKind::Machine)
        .map(|w| 1 + w.tapes.iter().map(|t| t.cardinality).sum::<usize>())
        .sum()
}

/// What a reading can still lose by narrowing: per routine or graph, one
/// for a `noreturn` not yet known plus the size of each tape's published
/// write set (the whole alphabet where none is stated). Every change of a
/// reading the settling loop expects lowers this.
fn narrowing_left(reading: &Resolved) -> usize {
    reading
        .worlds
        .iter()
        .filter(|w| w.kind != WorldKind::Machine)
        .map(|w| {
            usize::from(w.declared_noreturn.is_none())
                + w.tapes
                    .iter()
                    .map(|t| t.writes.map_or(t.cardinality, |set| set.len() as usize))
                    .sum::<usize>()
        })
        .sum()
}

/// [`resolve_declarations`], also reporting what it cost.
pub(crate) fn resolve_declarations_counted(
    sources: &[DeclarationSource],
    stdlib: bool,
) -> (Vec<Result<Resolved, UnresolvedSource>>, SettleStats) {
    let n = sources.len();
    let mut readings: Vec<Option<Resolved>> = vec![None; n];
    // The verdict of each source's LAST read: `None` once it read clean.
    let mut verdicts: Vec<Option<UnresolvedSource>> = vec![None; n];
    // Due a read: never read, or a peer's reading changed since.
    let mut due = vec![true; n];
    let mut stats = SettleStats::default();
    while due.contains(&true) {
        // Every sweep but the last changes a reading, and a change is a
        // first reading (at most one per source) or a narrowing: a glyph
        // leaves a published write set, or a routine becomes `noreturn`.
        // So the readings in hand bound the sweeps still possible.
        let narrowings: usize = readings.iter().flatten().map(settle_height).sum();
        stats.guard = stats.guard.max(n + narrowings);
        if stats.passes > n + narrowings {
            for i in (0..n).filter(|&i| due[i]) {
                verdicts[i] = Some(UnresolvedSource {
                    message: format!(
                        "{}: internal error: the declarations of this build's sources did \
                         not settle after {} passes over them; this is a defect in tmt, \
                         not in the source [{DECLARATIONS_UNSETTLED}]",
                        sources[i].path().display(),
                        stats.passes
                    ),
                    code: DECLARATIONS_UNSETTLED,
                });
            }
            break;
        }
        stats.passes += 1;
        for i in 0..n {
            if !due[i] {
                continue;
            }
            due[i] = false;
            let mut context = Declarations::none();
            for (j, reading) in readings.iter().enumerate() {
                if i != j
                    && let Some(r) = reading
                {
                    context.push(sources[j].origin.clone(), r.clone());
                }
            }
            // Pushed AFTER every peer — see this function's own doc
            // comment above for why the order matters.
            if stdlib {
                context.push_stdlib();
            }
            stats.reads += 1;
            let (reading, verdict) = sources[i].read_settling(&context);
            verdicts[i] = verdict;
            if let Some(reading) = reading
                && readings[i].as_ref() != Some(&reading)
            {
                if let Some(old) = &readings[i]
                    && narrowing_left(&reading) >= narrowing_left(old)
                {
                    stats.non_narrowing_changes += 1;
                }
                // REPLACED, never added beside the old one: the slot is
                // the source's one reading.
                readings[i] = Some(reading);
                for (j, flag) in due.iter_mut().enumerate() {
                    if j != i {
                        *flag = true;
                    }
                }
            }
        }
    }
    let results = readings
        .into_iter()
        .zip(verdicts)
        .map(|(reading, verdict)| match verdict {
            Some(refusal) => Err(refusal),
            None => Ok(reading.expect("a source with no refusal read clean")),
        })
        .collect();
    (results, stats)
}

/// A [`Resolved`] declaring nothing — [`declarations_from_object`]'s
/// answer for an object with no interface section at all.
fn empty_resolved() -> Resolved {
    Resolved {
        alphabets: HashMap::new(),
        sets: HashMap::new(),
        set_refs: HashSet::new(),
        maps: HashMap::new(),
        worlds: Vec::new(),
        entry_world: None,
        docs: HashMap::new(),
        from_object: false,
    }
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
            for (((param_name, glyphs), writes), (enters, leaves)) in routine
                .params
                .iter()
                .zip(&routine.glyphs)
                .zip(&routine.writes)
                .zip(routine.enters.iter().zip(&routine.leaves))
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
                params.push(tape_param_text(
                    param_name,
                    &alphabet_name,
                    writes,
                    enters.as_deref(),
                    leaves.as_deref(),
                ));
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
    canonical(&out)
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
    scopes: &compiler::Scopes,
    footprint: &FootprintTable,
    returns: &HashMap<String, bool>,
) -> String {
    let worlds: HashMap<&str, &ResolvedWorld> = resolved
        .worlds
        .iter()
        .map(|w| (w.name.as_str(), w))
        .collect();

    // Every graph reached from an EXPORTED graph's own body (transitively —
    // a referenced graph's own body may itself graft another non-exported
    // one), by mangled name — `resolved`'s own copies (`ResolvedGraft::
    // target`, already the fully mangled identity `resolve_world_reuse`
    // resolved it to, local or declarations-table alike). Printed as a
    // PLAIN `graph` (`graph_body_lines`'s own doc), the same "referenced,
    // printed even though not exported" rule local alphabets and local
    // maps already get — otherwise the header cannot re-parse a graft
    // reaching a non-exported sibling. An EXTERNAL target (one this unit
    // does not itself declare — e.g. a nested `std::…` graft) is left out:
    // that is the `use`-line rule's own job, exactly as an external
    // alphabet/map reference already is, and there is no local declaration
    // for this render to print for it regardless.
    let printed_graphs = printed_graph_names(program, resolved);
    let referenced_graphs: HashSet<&str> = printed_graphs.iter().map(String::as_str).collect();
    // A graph this render will PRINT (exported, or reached from a printed
    // graph's own body) — the same test the alphabet/map/`use`-line rules
    // below extend to cover it with.
    let is_printed_graph =
        |g: &Graph| g.exported || referenced_graphs.contains(full_name(&g.ns, &g.name).as_str());

    // Every named map a PRINTED graph body's binding args reach — the only
    // place a `with map NAME` reference can print at all, since a
    // routine's own body never prints (only its signature does). Reads
    // `resolved`'s own copies: `compiler::expand_named_maps` already
    // rewrote a resolved site's `SymMap::named` to the DECLARATION'S OWN
    // mangled name, so this is a direct membership test, never a second
    // resolution of the written text (`Program`'s own copies, which the
    // printer elsewhere reads for the WRITTEN spelling, are untouched by
    // that rewrite).
    let mut referenced_maps: HashSet<&str> = HashSet::new();
    for graph in &program.graphs {
        if is_printed_graph(graph) {
            let full = full_name(&graph.ns, &graph.name);
            resolved_map_refs(worlds[full.as_str()], &mut referenced_maps);
        }
    }

    // Every set a PRINTED graph body's pattern cells name, by the mangled
    // name resolution stored on each cell — the one place a header prints
    // a set NAME rather than a set's members, so the set it names must
    // print too: the same "referenced, printed even if not itself
    // exported" rule a map gets.
    let mut referenced_sets: HashSet<&str> = HashSet::new();
    for graph in &program.graphs {
        if is_printed_graph(graph) {
            let full = full_name(&graph.ns, &graph.name);
            resolved_set_refs(worlds[full.as_str()], &mut referenced_sets);
        }
    }

    // Every alphabet an EXPORTED routine or a PRINTED graph's tape
    // parameter draws from, by its mangled name — printed even when the
    // alphabet itself is not exported (a plain `alphabet`, not `export
    // alphabet`; see the module doc). A purely local routine's own
    // alphabet never lands in this set, so it still prints nothing, same
    // as before this rule existed.
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
        if is_printed_graph(graph) {
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
    for set in &program.sets {
        let full = full_name(&set.ns, &set.name);
        if set.exported || referenced_sets.contains(full.as_str()) {
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
        if is_printed_graph(graph) {
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
    // Named glyph sets: exported ones, and the ones a printed graph body's
    // pattern cells name, printed as their EXPANDED members
    // (docs/tmt/cli.md (interface)). An alphabet and every contract clause
    // print their resolved glyphs, so a pattern cell is the only printed
    // text that names a set; a set built from other sets (this unit's or
    // an imported one) still reads back with no `use` line and no sibling
    // declaration behind it.
    for set in &program.sets {
        let full = full_name(&set.ns, &set.name);
        if !set.exported && !referenced_sets.contains(full.as_str()) {
            continue;
        }
        let resolved_set = resolved
            .sets
            .get(&full)
            .expect("resolution guarantees every declared set is resolved");
        root.insert(&set.ns, set_lines(resolved_set, &set.name, set.exported));
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
        // Inferred when this routine's own fact is in `returns`, echoed
        // from the declared clause otherwise (`noreturn_fact`).
        let noreturn = noreturn_fact(returns, &full, routine.noreturn.is_some());
        root.insert(
            &routine.ns,
            routine_lines(routine, world, resolved, footprint, noreturn),
        );
    }
    for graph in &program.graphs {
        if !is_printed_graph(graph) {
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
        .chain(program.sets.iter().map(|s| full_name(&s.ns, &s.name)))
        .chain(program.maps.iter().map(|m| full_name(&m.ns, &m.name)))
        .chain(program.routines.iter().map(|r| full_name(&r.ns, &r.name)))
        .chain(program.graphs.iter().map(|g| full_name(&g.ns, &g.name)))
        .collect();
    let used = used_import_indices(program, scopes, &referenced_graphs, &referenced_maps);
    for ns in &import_scopes {
        let needed = needed_imports(ns, program, &used, &printed_full_names, &local_names);
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

/// Every `use` line this render's own printed declarations resolve
/// THROUGH, as indices into `Program::imports` — the "referenced" half of
/// the `use`-line rule (docs/tmt/cli.md (interface)).
///
/// The reference set is gathered per PRINTED declaration — an exported
/// map, an exported routine's signature, a printed graph's signature and
/// body — and each bare name is handed to the compiler's own scope
/// resolver FROM THAT DECLARATION'S OWN NAMESPACE. That is what makes an
/// outer-scope `use` reprint: in `.tmc` a `use` written at file scope is
/// visible to every namespace inside it, so a name referenced from a
/// nested namespace may well resolve through an import declared several
/// scopes out, and a scan keyed on the import's own namespace alone would
/// call that import unreferenced and drop it — leaving a header whose
/// printed signature names an alphabet nothing declares. Asking the
/// resolver also settles shadowing for free and for the same reason: it
/// walks innermost-out and takes a declaration over an import, so a
/// nested redeclaration of the short name claims the reference and the
/// outer import does not.
///
/// A map is scanned when this render PRINTS it — exported, or reached
/// from a printed graph body — matching `render_source`'s own printing
/// rule; a printed-but-unexported map's two alphabet names need the
/// identical `use` line an exported map's would.
fn used_import_indices(
    program: &Program,
    scopes: &compiler::Scopes,
    referenced_graphs: &HashSet<&str>,
    referenced_maps: &HashSet<&str>,
) -> HashSet<usize> {
    let mut used: HashSet<usize> = HashSet::new();
    let mark = |names: HashSet<&str>, ns: &[String], used: &mut HashSet<usize>| {
        for name in names {
            if let Some(idx) = scopes.import_index(name, ns) {
                used.insert(idx);
            }
        }
    };
    for map in &program.maps {
        let full = full_name(&map.ns, &map.name);
        if !map.exported && !referenced_maps.contains(full.as_str()) {
            continue;
        }
        let mut names: HashSet<&str> = HashSet::new();
        names.insert(map.src.as_str());
        names.insert(map.dst.as_str());
        mark(names, &map.ns, &mut used);
    }
    for routine in &program.routines {
        if !routine.exported {
            continue;
        }
        let mut names: HashSet<&str> = HashSet::new();
        collect_sig_refs(&routine.sig, &mut names);
        mark(names, &routine.ns, &mut used);
    }
    // A graph PRINTED here — exported, or reached from a printed graph's
    // own body (`render_source`'s own `is_printed_graph`) — contributes
    // its own references too: a non-exported-but-printed graph's own
    // `with map NAME` or nested graft target needs the identical `use`
    // line an exported graph's own body would.
    for graph in &program.graphs {
        let printed = graph.exported
            || referenced_graphs.contains(full_name(&graph.ns, &graph.name).as_str());
        if !printed {
            continue;
        }
        let mut names: HashSet<&str> = HashSet::new();
        collect_sig_refs(&graph.sig, &mut names);
        collect_graph_body_refs(graph, &mut names);
        mark(names, &graph.ns, &mut used);
    }
    used
}

/// The imports declared exactly at `ns` that this render both NEEDS and
/// CAN reprint — two independent conditions, both required
/// (docs/tmt/cli.md (interface)): a `use` line is printed only when
/// something this render prints resolves through it AND EITHER the header
/// prints its target OR the target lives in ANOTHER unit — reached
/// through the declarations table, never through this unit's own
/// declarations (`local_names`), which is exactly how a genuinely
/// cross-unit alphabet reference (`use std::binaryNumbers::symbols;`
/// against the embedded stdlib, say) resolves.
///
/// - referenced: this import's index is in `used`, the set
///   [`used_import_indices`] built by resolving every printed
///   declaration's bare references from that declaration's own scope.
///   Only declarations this render actually prints are scanned there, so
///   a reference from something the header drops (a non-exported world,
///   or a routine's own dropped body) does not count.
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
/// The line prints at the import's OWN namespace, exactly where it was
/// written, which is what keeps it visible to the nested scopes that may
/// have been the only things referencing it.
///
/// Source order preserved: `imports` is walked in its own (already
/// source-ordered) sequence, filtered rather than resorted.
fn needed_imports<'a>(
    ns: &[String],
    program: &'a Program,
    used: &HashSet<usize>,
    printed_full_names: &HashSet<String>,
    local_names: &HashSet<String>,
) -> Vec<&'a Import> {
    program
        .imports
        .iter()
        .enumerate()
        .filter(|(idx, imp)| {
            imp.ns.as_slice() == ns
                && used.contains(idx)
                && (printed_full_names.contains(&imp.full_path())
                    || !local_names.contains(&imp.full_path()))
        })
        .map(|(_, imp)| imp)
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
/// target inside a rule's transition — and every bare set name a pattern
/// cell spells. std.tmc's own exported graphs never
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
            // A bare set name in a pattern cell resolves through an import
            // exactly as a bare graft target does.
            for cell in &rule.pattern.cells {
                if let PatternCellKind::SetRef { name, .. } = &cell.kind
                    && !name.contains("::")
                {
                    out.insert(name.as_str());
                }
            }
            // A `where` guard's `in` conjunct names a set exactly as a
            // pattern cell does — a bare name resolves through an import
            // (docs/tmt/language.md (rule guards)).
            if let Some(g) = &rule.guard {
                for n in crate::guard::in_conjuncts(g) {
                    if !n.set.name.contains("::") {
                        out.insert(n.set.name.as_str());
                    }
                }
            }
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

/// Every set one RESOLVED world's pattern cells name, by the MANGLED name
/// resolution stored on each cell — the set-cell counterpart of
/// [`resolved_map_refs`], deciding whether a printed graph body's set name
/// needs its declaration printed alongside it.
fn resolved_set_refs<'a>(world: &'a ResolvedWorld, out: &mut HashSet<&'a str>) {
    for state in &world.states {
        for rule in &state.rules {
            for cell in &rule.pattern.cells {
                if let PatternCellKind::SetRef {
                    resolved: Some(r), ..
                } = &cell.kind
                {
                    out.insert(r.set.as_str());
                }
            }
            // A `where` guard's `in` conjunct names a set the same way —
            // by NAME alone in the printed body, so the set it names must
            // print alongside it (docs/tmt/language.md (rule guards)).
            if let Some(g) = &rule.guard {
                for n in crate::guard::in_conjuncts(g) {
                    if let Some(r) = &n.resolved {
                        out.insert(r.set.as_str());
                    }
                }
            }
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

/// A glyph set as its members — [`alphabet_lines`]' spelling with `set` in
/// place of `alphabet`: `export set` for an exported set, a plain `set` for
/// one printed only because a printed graph body names it. A member whose
/// label names a number (`patterns::label_number`, fold arithmetic's rule)
/// prints bare, whatever its width and however the source quoted it — `'7'`
/// and `7` are one symbol, and the bare spelling says it is one a fold may
/// read. An empty set prints `{}`, which the strict reader reads back as
/// the same empty set.
fn set_lines(set: &compiler::ResolvedSet, name: &str, exported: bool) -> Vec<String> {
    let keyword = if exported { "export set" } else { "set" };
    let members = if set.glyphs.is_empty() {
        "{}".to_string()
    } else {
        let spelled: Vec<String> = set
            .glyphs
            .iter()
            .map(|g| crate::patterns::label_spelling(g))
            .collect();
        format!("{{ {} }}", spelled.join(", "))
    };
    vec![format!("{keyword} {name} {members}")]
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

/// A graph's canonical SIGNATURE AND BODY — `export graph NAME(...) { … }`
/// for an exported graph, `graph NAME(...) { … }` for one printed only
/// because a printed graph's own body reaches it (the same "referenced,
/// printed even though not exported" spelling `alphabet_lines`/
/// `map_lines` already give their own local declarations) — without the
/// leading `?` doc-line prefix [`graph_lines`] adds. [`graph_digest`]'s
/// own input: a graph's documentation is deliberately left out of what
/// gets hashed, so a doc-only edit never moves the digest.
fn graph_body_lines(
    graph: &Graph,
    world: &ResolvedWorld,
    resolved: &Resolved,
    footprint: &FootprintTable,
) -> Vec<String> {
    let sig = signature_text(&graph.sig, world, resolved, footprint);
    let keyword = if graph.exported {
        "export graph"
    } else {
        "graph"
    };
    let mut lines = vec![format!("{keyword} {}({}) {{", graph.name, sig)];
    // The body prints the POSITIONAL form whatever the source spelled: a
    // name is a spelling, and a generated artifact speaks the semantics
    // (docs/tmt/language.md (named vector cells)). The fold is the same one
    // resolution runs over `world.states`; over this source-form copy it is
    // infallible (the unit resolved before printing) or a no-op (the object
    // arm builds positional rules from rows) — running it is what keeps both
    // arms, and `graph_digest` over these lines, on one text.
    let mut states = graph.states.clone();
    position_named_cells(
        &mut states,
        &world
            .tapes
            .iter()
            .map(|t| t.name.as_str())
            .collect::<Vec<_>>(),
    )
    .expect("a body prints only for a unit that resolved");
    for state in &states {
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

fn graph_lines(
    graph: &Graph,
    world: &ResolvedWorld,
    resolved: &Resolved,
    footprint: &FootprintTable,
) -> Vec<String> {
    let mut lines = doc_lines(graph.doc.as_ref());
    lines.extend(graph_body_lines(graph, world, resolved, footprint));
    lines
}

/// The digest a graft records for the body it spliced, and an exporting
/// unit records for each graph it exports (docs/formats.md (routine
/// interfaces)): the CRC-32 of [`graph_body_lines`]'s own text — this
/// module's canonical rendering of a graph's signature and body, proven
/// insensitive to source whitespace and comments
/// (`the_printer_is_insensitive_to_comments_and_whitespace`) and
/// deliberately excluding the graph's own `?` doc lines, so documenting a
/// graph never raises a graft-drift finding.
///
/// ONE function computes it, called from both sides: the exporting unit's
/// own `compile()`, directly over its own `Program`/`Resolved`/footprint,
/// and a grafting consumer, indirectly — [`stamp_graph_digests`] runs this
/// same function once, when a `Resolved` is about to be handed to
/// [`crate::declarations::Declarations`] (`read_extern`, the embedded
/// stdlib's own header), and the consumer reads the result straight off
/// the [`ResolvedWorld`] it spliced. Never a second computation to drift
/// from this one.
pub(crate) fn graph_digest(
    graph: &Graph,
    world: &ResolvedWorld,
    resolved: &Resolved,
    footprint: &FootprintTable,
) -> u32 {
    let mut text = graph_body_lines(graph, world, resolved, footprint).join("\n");
    // The content a body names only by name (docs/tmt/language.md (grafting a
    // graph from another unit)): every glyph set a pattern cell names and every
    // named map an argument names, each once, by qualified name — so a change
    // to a member or a pair moves the digest. Nothing is appended for a body
    // that names neither, which keeps every such digest where it was.
    let mut named: BTreeMap<String, String> = BTreeMap::new();
    for state in &world.states {
        for rule in &state.rules {
            for cell in &rule.pattern.cells {
                if let PatternCellKind::SetRef {
                    resolved: Some(m), ..
                } = &cell.kind
                {
                    let members: Vec<String> = m
                        .members
                        .iter()
                        .map(|g| crate::patterns::label_spelling(&g.label))
                        .collect();
                    named.insert(
                        m.set.clone(),
                        format!("set {} {{ {} }}", m.set, members.join(", ")),
                    );
                }
            }
            // A `where` guard's `in` conjunct names a set BY NAME alone in
            // the body — member drift moves no body line, so the appendix
            // is where the digest learns of it (docs/tmt/language.md (rule
            // guards); the GraftDrift half of this round).
            if let Some(g) = &rule.guard {
                for n in crate::guard::in_conjuncts(g) {
                    if let Some(m) = &n.resolved {
                        let members: Vec<String> = m
                            .members
                            .iter()
                            .map(|g| crate::patterns::label_spelling(&g.label))
                            .collect();
                        named.insert(
                            m.set.clone(),
                            format!("set {} {{ {} }}", m.set, members.join(", ")),
                        );
                    }
                }
            }
        }
    }
    let mut take_args = |args: &[BindingArg]| {
        for a in args {
            if let BindingValue::Named { map: Some(m), .. } = &a.value
                && let Some((name, _)) = &m.named
            {
                let open = if m.open.is_some() { ", *" } else { "" };
                named.insert(
                    name.clone(),
                    format!("map {} {{ {}{open} }}", name, map_pair_list_text(&m.pairs)),
                );
            }
        }
    };
    for g in &world.grafts {
        take_args(&g.args);
    }
    for b in &world.binds {
        take_args(&b.args);
    }
    for c in &world.calls {
        if let ResolvedCallTarget::Routine { args, .. } = &c.target {
            take_args(args);
        }
    }
    if !named.is_empty() {
        text.push('\n');
        text.push_str(&named.into_values().collect::<Vec<_>>().join("\n"));
    }
    crc32(text.as_bytes())
}

/// Every graph this unit's header prints: each exported graph and, by
/// mangled name, every local graph an exported body reaches through its
/// grafts, transitively (docs/tmt/language.md (headers)). A target this
/// unit does not itself declare (a nested `std::…` graft) is not this
/// unit's to print.
pub(crate) fn printed_graph_names(program: &Program, resolved: &Resolved) -> BTreeSet<String> {
    let worlds: HashMap<&str, &ResolvedWorld> = resolved
        .worlds
        .iter()
        .map(|w| (w.name.as_str(), w))
        .collect();
    let mut names: BTreeSet<String> = BTreeSet::new();
    let mut frontier: Vec<&ResolvedWorld> = Vec::new();
    for g in program.graphs.iter().filter(|g| g.exported) {
        let full = full_name(&g.ns, &g.name);
        if let Some(&w) = worlds.get(full.as_str()) {
            names.insert(full);
            frontier.push(w);
        }
    }
    while let Some(world) = frontier.pop() {
        for graft in &world.grafts {
            let target = graft.target.as_str();
            if let Some(&target_world) = worlds.get(target)
                && target_world.kind == WorldKind::Graph
                && names.insert(target.to_string())
            {
                frontier.push(target_world);
            }
        }
    }
    names
}

/// `.graph` records for every graph [`printed_graph_names`] names, each with
/// its [`graph_digest`] — the exporting half of the graft-drift check
/// (docs/formats.md (routine interfaces)): `compiler::compile` writes these
/// as `.graph <name>, <digest>` lines (`, private` for a graph the header
/// prints without exporting), which the assembler parses back into
/// `Interface::graphs`.
pub(crate) fn interface_graph_digests(
    program: &Program,
    resolved: &Resolved,
    footprint: &FootprintTable,
) -> Vec<ExportedGraph> {
    let worlds: HashMap<&str, &ResolvedWorld> = resolved
        .worlds
        .iter()
        .map(|w| (w.name.as_str(), w))
        .collect();
    let printed = printed_graph_names(program, resolved);
    program
        .graphs
        .iter()
        .filter_map(|graph| {
            let full = full_name(&graph.ns, &graph.name);
            if !printed.contains(&full) {
                return None;
            }
            let world = worlds.get(full.as_str())?;
            Some(ExportedGraph {
                digest: graph_digest(graph, world, resolved, footprint),
                name: full,
                exported: graph.exported,
            })
        })
        .collect()
}

/// Stamp [`ResolvedWorld::digest`] on every graph world of a just-parsed
/// declarations module, via [`graph_digest`] — the ONE place outside an
/// exporting unit's own `compile()` this digest is ever computed. Run right
/// where `program` (the AST `graph_digest` needs) is still in scope, before
/// the [`Resolved`] alone is handed off to [`crate::declarations::
/// Declarations`] (`read_extern`, the embedded stdlib's own header): a
/// `Declarations` module carries no `Program`, so a consumer reading it
/// back later has nothing to recompute the digest FROM — it reads this
/// stamped field instead.
pub(crate) fn stamp_graph_digests(
    program: &Program,
    resolved: &mut Resolved,
    footprint: &FootprintTable,
) {
    let digests: Vec<(usize, u32)> = program
        .graphs
        .iter()
        .filter_map(|graph| {
            let full = full_name(&graph.ns, &graph.name);
            let idx = resolved
                .worlds
                .iter()
                .position(|w| w.kind == WorldKind::Graph && w.name == full)?;
            let digest = graph_digest(graph, &resolved.worlds[idx], resolved, footprint);
            Some((idx, digest))
        })
        .collect();
    for (idx, digest) in digests {
        resolved.worlds[idx].digest = Some(digest);
    }
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
        // exactly the line the module doc draws for `never writes`. The
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
            let writes = symset_glyphs(published, alphabet_glyphs);
            // The SAME index-to-label step `writes` above just went
            // through (`compiler::symset_glyphs`), applied to `enters`/
            // `leaves` (`compiler::clause_glyphs`, over
            // `ResolvedTape::enters`/`::leaves` — `resolve_contract_
            // clause`'s own output, the walk that raises
            // `ContractSymbolUnknown`).
            let enters = clause_glyphs(tape.enters, alphabet_glyphs);
            let leaves = clause_glyphs(tape.leaves, alphabet_glyphs);
            tape_param_text(
                &param.name,
                alphabet,
                &writes,
                enters.as_deref(),
                leaves.as_deref(),
            )
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

/// One tape parameter's rendered text — `tape NAME: ALPHABET writes { … }`,
/// optionally followed by `enters { … }` and/or `leaves { … }` — the ONE
/// renderer both the source arm (`sig_param_text`) and the object arm
/// (`from_object`) call, so the two can never drift apart on how a
/// parameter is formatted, only on what clauses they pass in (which
/// `compiler::published_writes`/`compiler::clause_glyphs` also unify — see
/// the module doc). `enters`/`leaves` print only when `Some` — unlike
/// `writes`, which always prints (`writes {}` included) because it carries
/// an inferred fact even when nothing was declared, an absent head-position
/// clause declares no restriction at all, and a present one is never
/// empty, so there is no `{}` form to fall back to.
///
/// The `opaque` bit is deliberately NOT printed here, on either arm, for
/// the reason `volatile` is not (see [`sig_param_text`]) and one more.
/// `opaque` is not a DECLARATION: it is a fact the compiler infers from a
/// routine's body — whether every state that reads the tape reads it
/// through a `*` cell — and publishes to the object for the linker to
/// judge an open binding against (docs/formats.md (routine interfaces)).
/// A header is the declarations surface, and the `.tmh` grammar is the
/// `.tmc` grammar under a declarations-only reading; `.tmc` has no
/// spelling for the bit and is not given one, because a bodiless
/// declaration has no body for the fact to be true of and a reader of one
/// could not re-derive it. Printing a word the language cannot read back
/// would break the round trip that makes `tmt interface`'s output a
/// header at all. What a caller needs the bit for — may I open this
/// binding? — is answered by the linker against the OBJECT, which carries
/// it, not against the header.
fn tape_param_text(
    name: &str,
    alphabet: &str,
    writes: &[String],
    enters: Option<&[String]>,
    leaves: Option<&[String]>,
) -> String {
    let mut out = format!("tape {name}: {alphabet} writes {}", braced_list(writes));
    if let Some(glyphs) = enters {
        let _ = write!(out, " enters {}", braced_list(glyphs));
    }
    if let Some(glyphs) = leaves {
        let _ = write!(out, " leaves {}", braced_list(glyphs));
    }
    out
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
    // A `where` guard is semantics, like a range: the header prints its
    // CANONICAL text (`guard_text`), between the pattern and the arrow
    // (docs/tmt/language.md (rule guards)). `fmt` alone prints the written
    // tokens; the two agree on every tree.
    let guard = rule
        .guard
        .as_ref()
        .map(|g| format!(" where {{ {} }}", crate::guard::guard_text(g)))
        .unwrap_or_default();
    format!("{pattern}{guard} -> {};", action.join(" "))
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
                        format!("{}..{}", range_end_text(lo), range_end_text(hi))
                    }
                    // The name as written; `render_source` prints the set
                    // it names alongside, so the header reads back.
                    PatternCellKind::SetRef { name, .. } => name.clone(),
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

/// A range endpoint as it was written (`patterns::written_spelling`): a
/// glyph always quoted, with none of [`sym_lit_text`]'s bare-number
/// shortcut, and a number bare. A pattern cell's range may have an endpoint
/// of several characters (docs/tmt/language.md (pattern ranges)) —
/// `'9'..'10'` — which that shortcut would restyle as `'9'..10`, and a body
/// range needs its two endpoints to keep one kind.
fn range_end_text(sym: &SymLit) -> String {
    crate::patterns::written_spelling(sym)
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

/// An inline `with map { … }`'s interior: its pairs, then the open
/// marker when the site left the map open — the marker is the list's last
/// entry, the only position the grammar admits it (docs/tmt/language.md
/// (symbol maps)), so a printed header re-reads as the same site.
fn map_pairs_text(map: &SymMap) -> String {
    let pairs = map_pair_list_text(&map.pairs);
    match (map.open.is_some(), pairs.is_empty()) {
        (false, _) => pairs,
        (true, true) => "*".to_string(),
        (true, false) => format!("{pairs}, *"),
    }
}

/// One comma-`, `-joined pair list — shared by an inline `with map { … }`
/// ([`map_pairs_text`]) and a named map's own declared body
/// ([`map_lines`]).
pub(crate) fn map_pair_list_text(pairs: &[MapPair]) -> String {
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

#[cfg(test)]
mod settle_tests {
    //! The settling loop of [`resolve_declarations`]: one outcome in every
    //! order of the sources, what it costs, and the assumption its sweep
    //! guard rests on (docs/tmt/project.md (Declaration derivation)).

    use super::*;

    const A_MID: &str = "namespace a {\n  export alphabet ab { '_', 'x', 'y' }\n  export routine mid(tape t: ab) {\n    entry state s { [*] -> call b::leaf(t = t) then d; }\n    state d { [*] -> return; }\n  }\n}\n";
    const B_LEAF: &str = "namespace b {\n  export alphabet ab { '_', 'x', 'y' }\n  export routine leaf(tape t: ab) {\n    entry state s { [*] -> write ['x'] return; }\n  }\n}\n";
    const B_RELAY: &str = "namespace b {\n  export alphabet ab { '_', 'x', 'y' }\n  export routine leaf(tape t: ab) {\n    entry state s { [*] -> call d::leaf(t = t) then e; }\n    state e { [*] -> return; }\n  }\n}\n";
    const D_LEAF: &str = "namespace d {\n  export alphabet ab { '_', 'x', 'y' }\n  export routine leaf(tape t: ab) {\n    entry state s { [*] -> write ['x'] return; }\n  }\n}\n";
    const C_TOP: &str = "use a::ab;\nroutine top(tape t: ab writes { 'x' }) {\n  entry state s { [*] -> call a::mid(t = t) then d; }\n  state d { [*] -> return; }\n}\nmachine {\n  tape t: ab;\n  entry state go { [*] -> call top(t = t) then fin; }\n  state fin { [*] -> stop; }\n}\n";

    /// `a::f` and `b::g` over one five-glyph alphabet call each other, `f`
    /// through a map that shifts the glyphs: each trip round the cycle
    /// drops one glyph from the write sets the two publish.
    const CYC_A: &str = "namespace a {\n  export alphabet ab { '_', 'a', 'b', 'c', 'd' }\n  export routine f(tape t: ab) {\n    entry state s { [*] -> call b::g(t = t with map { 'a' -> 'b', 'b' -> 'c', 'c' -> 'd', 'd' => '_' }) then e; }\n    state e { [*] -> return; }\n  }\n}\n";
    const CYC_B: &str = "namespace b {\n  export alphabet ab { '_', 'a', 'b', 'c', 'd' }\n  export routine g(tape t: ab) {\n    entry state s { [*] -> call a::f(t = t) then e; }\n    state e { [*] -> return; }\n  }\n}\n";
    const CYC_C: &str = "use a::ab;\nmachine {\n  tape t: ab;\n  entry state go { [*] -> call a::f(t = t) then fin; }\n  state fin { [*] -> stop; }\n}\n";

    /// The same cycle over alphabets of five and four glyphs, each call
    /// through a shifting map; `c`'s `top` promises to write nothing, which
    /// the settled cycle still breaks by the blank alone.
    const CYC2_A: &str = "namespace a {\n  export alphabet big { '_', 'a', 'b', 'c', 'd' }\n  export routine f(tape t: big) {\n    entry state s {\n      ['_'] -> return;\n      [*]   -> call b::g(t = t with map { 'a' -> 'x', 'b' -> 'y', 'c' -> 'z' }) then e;\n    }\n    state e { [*] -> return; }\n  }\n}\n";
    const CYC2_B: &str = "namespace b {\n  export alphabet small { '_', 'x', 'y', 'z' }\n  export routine g(tape t: small) {\n    entry state s {\n      ['_'] -> return;\n      [*]   -> call a::f(t = t with map { 'x' -> 'b', 'y' -> 'c', 'z' -> 'd' }) then e;\n    }\n    state e { [*] -> return; }\n  }\n}\n";
    const CYC2_C: &str = "use a::big;\nroutine top(tape t: big writes {}) {\n  entry state s { [*] -> call a::f(t = t) then d; }\n  state d { [*] -> return; }\n}\nmachine {\n  tape t: big;\n  entry state go { [*] -> call top(t = t) then fin; }\n  state fin { [*] -> stop; }\n}\n";

    /// Two sources whose routines call each other, each under `writes
    /// { '0' }`, and the same pair with `bb::rb` really writing `'1'`
    /// under a contract that allows it — which breaks `aa::ra`'s.
    const PAIR_A: &str = "namespace aa {\n  export alphabet t { '_', '0', '1' }\n  export routine ra(tape x: t writes { '0' }) {\n    entry state s {\n      ['1'] -> write ['0'] move [>] call bb::rb(x = x) then done;\n      [*]   -> return;\n    }\n    state done { [*] -> return; }\n  }\n}\n";
    const PAIR_B: &str = "namespace bb {\n  export alphabet t { '_', '0', '1' }\n  export routine rb(tape x: t writes { '0' }) {\n    entry state s {\n      ['1'] -> write ['0'] move [>] call aa::ra(x = x) then done;\n      [*]   -> return;\n    }\n    state done { [*] -> return; }\n  }\n}\n";
    const PAIR_B_LOUD: &str = "namespace bb {\n  export alphabet t { '_', '0', '1' }\n  export routine rb(tape x: t writes { '0', '1' }) {\n    entry state s {\n      ['0'] -> write ['1'] move [>] call aa::ra(x = x) then done;\n      [*]   -> return;\n    }\n    state done { [*] -> return; }\n  }\n}\n";

    type Outcome = Vec<(String, Result<Resolved, String>)>;

    fn settle(order: &[(&str, &str)]) -> (Outcome, SettleStats) {
        let sources: Vec<DeclarationSource> = order
            .iter()
            .map(|(name, text)| DeclarationSource {
                origin: Origin::Sibling(PathBuf::from(name)),
                text: DeclarationText::Source {
                    path: PathBuf::from(name),
                    text: text.to_string(),
                },
            })
            .collect();
        let (results, stats) = resolve_declarations_counted(&sources, false);
        assert_eq!(results.len(), order.len(), "one outcome per source");
        let mut outcome: Outcome = order
            .iter()
            .zip(results)
            .map(|((name, _), r)| (name.to_string(), r.map_err(|e| e.message)))
            .collect();
        outcome.sort_by(|a, b| a.0.cmp(&b.0));
        (outcome, stats)
    }

    fn orderings<'a>(items: &[(&'a str, &'a str)]) -> Vec<Vec<(&'a str, &'a str)>> {
        if items.len() <= 1 {
            return vec![items.to_vec()];
        }
        let mut all = Vec::new();
        for i in 0..items.len() {
            let mut rest = items.to_vec();
            let head = rest.remove(i);
            for mut tail in orderings(&rest) {
                tail.insert(0, head);
                all.push(tail);
            }
        }
        all
    }

    /// Settle `sources` in every order: the outcome of each source —
    /// its whole reading, or its error text — is the same in all of them,
    /// the sweeps stay within the guard, and no reading was replaced by one
    /// that did not narrow. Returns that one outcome and the most sweeps
    /// any order took.
    fn one_outcome_in_every_order(sources: &[(&str, &str)]) -> (Outcome, usize) {
        let mut first: Option<Outcome> = None;
        let mut most = 0;
        for order in orderings(sources) {
            let names: Vec<&str> = order.iter().map(|s| s.0).collect();
            let (outcome, stats) = settle(&order);
            assert!(stats.passes <= stats.guard, "{names:?}: {stats:?}");
            assert_eq!(stats.non_narrowing_changes, 0, "{names:?}: {stats:?}");
            most = most.max(stats.passes);
            match &first {
                None => first = Some(outcome),
                Some(expected) => assert!(expected == &outcome, "{names:?} settled differently"),
            }
        }
        (first.expect("at least one order"), most)
    }

    fn published_writes_of(outcome: &Outcome, source: &str, routine: &str) -> Vec<String> {
        let reading = outcome
            .iter()
            .find(|(name, _)| name == source)
            .and_then(|(_, r)| r.as_ref().ok())
            .unwrap_or_else(|| panic!("{source} has a reading"));
        let world = reading
            .worlds
            .iter()
            .find(|w| w.name == routine)
            .expect("the routine is declared");
        let tape = &world.tapes[0];
        let glyphs = &reading.alphabets[&tape.alphabet].glyphs;
        symset_glyphs(
            tape.writes.expect("a source reading publishes a set"),
            glyphs,
        )
    }

    /// A lone source is read exactly once, in one sweep: the checked read
    /// comes first and nothing can make it due again. Mutation: mark a
    /// source itself due when its own reading changes (drop the `j != i`
    /// test) — it is then read twice.
    #[test]
    fn a_lone_source_is_read_exactly_once() {
        let (outcome, stats) = settle(&[("b.tmc", B_LEAF)]);
        assert!(outcome[0].1.is_ok());
        assert_eq!((stats.passes, stats.reads), (1, 1), "{stats:?}");
    }

    /// The three-source and four-source chains settle to one outcome in
    /// every order: every source reads clean and `a::mid` publishes only
    /// the glyph the leaf really writes. Without a cycle of calls each
    /// sweep settles one more source, so `sources + 1` sweeps suffice.
    /// Mutation: read each source once (mark a peer due only while it has
    /// no reading) — in the orders reading `a` before `b`, `a::mid`
    /// publishes the whole alphabet and `c` is refused.
    #[test]
    fn a_chain_of_sources_settles_to_one_outcome_in_every_order() {
        for sources in [
            vec![("c.tmc", C_TOP), ("a.tmc", A_MID), ("b.tmc", B_LEAF)],
            vec![
                ("c.tmc", C_TOP),
                ("a.tmc", A_MID),
                ("b.tmc", B_RELAY),
                ("d.tmc", D_LEAF),
            ],
        ] {
            let (outcome, most) = one_outcome_in_every_order(&sources);
            for (name, result) in &outcome {
                assert!(result.is_ok(), "{name}: {:?}", result.as_ref().err());
            }
            assert_eq!(published_writes_of(&outcome, "a.tmc", "a::mid"), ["x"]);
            assert!(most <= sources.len() + 1, "{most} sweeps");
        }
    }

    /// A cycle of calls through a glyph-shifting map loses one glyph per
    /// trip, so it takes MORE sweeps than `sources + 1` — and still settles
    /// to one outcome in every order, within the guard: the write sets
    /// `f` and `g` publish shrink from the whole alphabet to the one or
    /// two glyphs the maps cannot shift away. Mutation: guard the sweeps by the number
    /// of sources plus one (drop `narrowings` from the guard) — these
    /// sources are then refused with the internal-consistency error, in
    /// some orders and not others.
    #[test]
    fn a_cycle_of_calls_through_shifting_maps_settles_to_one_outcome_in_every_order() {
        let pair = [("a.tmc", CYC_A), ("b.tmc", CYC_B)];
        let (outcome, most) = one_outcome_in_every_order(&pair);
        assert!(most > pair.len() + 1, "the cycle settled in {most} sweeps");
        assert_eq!(published_writes_of(&outcome, "a.tmc", "a::f"), ["_", "a"]);
        assert_eq!(published_writes_of(&outcome, "b.tmc", "b::g"), ["_", "a"]);

        let (outcome, _) =
            one_outcome_in_every_order(&[("c.tmc", CYC_C), ("a.tmc", CYC_A), ("b.tmc", CYC_B)]);
        assert!(outcome.iter().all(|(_, r)| r.is_ok()), "{outcome:?}");

        let three = [("c.tmc", CYC2_C), ("a.tmc", CYC2_A), ("b.tmc", CYC2_B)];
        let (outcome, most) = one_outcome_in_every_order(&three);
        assert!(most > three.len() + 1, "the cycle settled in {most} sweeps");
        assert_eq!(published_writes_of(&outcome, "a.tmc", "a::f"), ["_"]);
        assert_eq!(published_writes_of(&outcome, "b.tmc", "b::g"), ["_"]);
        let refusal = outcome[2]
            .1
            .as_ref()
            .expect_err("`top` promises to write nothing");
        assert_eq!(
            refusal,
            "c.tmc:2:13: error: `top` may write '_' on tape `t`, which its contract forbids \
             [writes-outside-contract]"
        );
    }

    /// Two sources whose routines call each other under `writes`
    /// contracts both read clean, in either order: a contract refusal met
    /// before the peer had a reading is provisional, and against the
    /// settled set each body holds its contract given the other's.
    /// Mutation: treat provisional refusals as final (give a source
    /// refused by a peer check no reading and keep its refusal) — both
    /// sources then fail `writes-outside-contract`.
    #[test]
    fn two_sources_calling_each_other_under_contracts_both_read_clean() {
        let (outcome, _) = one_outcome_in_every_order(&[("a.tmc", PAIR_A), ("b.tmc", PAIR_B)]);
        for (name, result) in &outcome {
            assert!(result.is_ok(), "{name}: {:?}", result.as_ref().err());
        }
    }

    /// The near miss: the same pair with `bb::rb` really writing `'1'`.
    /// `aa::ra`'s contract is then broken through the call, and that is
    /// refused in either order with the one diagnostic of the settled set;
    /// `bb`, whose own contract holds, reads clean. Mutation: accept a
    /// source once it has a reading, whatever its last checked read said
    /// (drop the verdict of a read that still yielded a reading) — `aa` is
    /// then accepted.
    #[test]
    fn a_contract_really_broken_through_a_mutual_call_is_refused_in_either_order() {
        let (outcome, _) = one_outcome_in_every_order(&[("a.tmc", PAIR_A), ("b.tmc", PAIR_B_LOUD)]);
        assert_eq!(
            outcome[0]
                .1
                .as_ref()
                .expect_err("`aa::ra` breaks its contract"),
            "a.tmc:3:21: error: `aa::ra` may write '1' on tape `x`, which its contract forbids \
             [writes-outside-contract]"
        );
        assert!(outcome[1].1.is_ok(), "{:?}", outcome[1].1.as_ref().err());
    }

    /// One source contributes exactly one reading: the table assembled
    /// from the settled results holds each name once, so the
    /// one-declaration check — which compares every pair of readings —
    /// finds nothing, though `a::mid` had a wider reading on the way.
    /// Mutation: read each source once (mark a peer due only while it has
    /// no reading) — `c` is then left refused against `a::mid`'s first,
    /// wider reading and there is no settled table to assemble.
    #[test]
    fn a_settled_source_contributes_one_reading() {
        let order = [("c.tmc", C_TOP), ("a.tmc", A_MID), ("b.tmc", B_LEAF)];
        let (outcome, stats) = settle(&order);
        assert!(stats.passes > 1, "the fixture must re-read a source");
        let mut table = Declarations::none();
        for (name, result) in outcome {
            table.push(
                Origin::Sibling(PathBuf::from(&name)),
                result.unwrap_or_else(|e| panic!("{name}: {e}")),
            );
        }
        assert_eq!(table.len(), order.len());
        assert!(table.check_conflicts().is_ok());
    }
}
