//! The `tmt interface` printer (docs/tmt/language.md (headers)): one
//! canonical rendering of a unit's EXPORTED declarations, reachable from
//! two inputs — a `.tmc` source (the complete arm: exported alphabets,
//! exported maps in a later round (none exist in the language yet, and a
//! header is a valid, total rendering without them), `export routine`
//! signatures with their contracts and `?` doc lines, and `export graph`
//! bodies in full) or a compiled `.tmo` object (the reduced arm:
//! signatures, contracts, and exported alphabets only — an object carries
//! no graph body, no map, and no doc line to print).
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
//! needs, on the SOURCE arm only** (docs/tmt/cli.md (interface)): an
//! import from `Program::imports`, declared exactly at that namespace,
//! reprints as `use path[ as alias];` iff its bound short name is
//! referenced, unqualified, by something this render prints in that same
//! scope — a tape signature's alphabet name, or (inside a printed
//! `export graph` body) a bare `graft`/`bind` target or a bare `call`
//! target in a rule's transition. An import whose bound name nothing
//! printed there references is dropped, exactly like an import whose
//! TARGET is never printed at all (a non-exported routine or graph, or an
//! alphabet nothing exported reaches) — printing either would be text
//! that cannot resolve when the header is read back through the strict
//! reader. This is what makes std.tmc's volatile-twin namespaces
//! (`binaryNumbersVolatile`, `binaryNumbersBareVolatile`), which import
//! their representation alphabet from a SIBLING namespace via an explicit
//! `use`, reprint as a header that reparses. The OBJECT arm prints no
//! `use` line at all, on any routine: `Interface::imports` (docs/formats.md
//! (routine interfaces)) records only a GENUINELY cross-unit reference — a
//! name a `use` or a qualified path reaches that this unit's own
//! declarations do not define, resolved against an external declarations
//! module at compile time. A same-unit sibling-namespace `use`, the shape
//! std.tmc's volatile twins use, resolves against this unit's OWN
//! declarations and never touches it, so it carries nothing the object arm
//! could read a `use` line back from for that shape either.
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
//! `RoutineInterface` carries a tape's glyph list, never an identifier for
//! it. The reconstruction is matching a tape's glyph list, by content,
//! against exported alphabets the routine could spell UNQUALIFIED in
//! source — its own namespace, or any ENCLOSING namespace (an unqualified
//! name resolves outward through enclosing scopes); the first match (in
//! wire order) wins, and two such exported alphabets sharing one glyph
//! list are genuinely indistinguishable from the object alone — the
//! printer accepts that ambiguity rather than erroring on it. A content
//! match in a SIBLING or otherwise unrelated namespace — reachable only
//! through an explicit `use` alias, like std.tmc's volatile twins
//! importing their representation alphabet from a SIBLING namespace — is
//! deliberately not used, and `Interface::imports` (docs/formats.md
//! (routine interfaces)) does not help here EITHER: that record carries
//! only a GENUINELY cross-unit import (a name resolved at compile time
//! against another unit's declarations table), and a same-unit
//! sibling-namespace `use` never becomes one — it resolves locally, so no
//! entry for it ever reaches the wire (verified: the compiled embedded
//! stdlib, whose volatile twins are exactly this shape, carries zero
//! `Interface::imports` records). A genuinely cross-unit import DOES carry
//! a name and glyphs on the wire, but nothing here reads it yet — the
//! shipped corpus has no fixture that would exercise it, since std.tmc's
//! own cross-namespace `use`s are all same-unit. A tape whose alphabet no
//! reachable export matches — whether none matches at all, or only an
//! unrelated one does — gets a SYNTHESIZED, deterministic plain-`alphabet`
//! declaration instead of an error: `<routine>__<param>` (the routine's
//! own mangled name with `::` replaced by `_`, joined to the parameter
//! name), declared at the top level, before the namespace block that uses
//! it. The object arm never fails to render a routine for want of an
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

use mtc_core::formats::object::{ExportedAlphabet, ObjectFile, RoutineInterface, SymbolDef};

use crate::codegen::{render_glyph_element, render_glyph_list};
use crate::compiler::{
    self, CompileError, ReadMode, Resolved, ResolvedWorld, full_name, published_writes,
};
use crate::declarations::Declarations;
use crate::footprint::{self, FootprintTable};
use crate::parser::{
    Bind, BindingArg, BindingValue, Doc, FoldExprKind, FoldExprNode, FoldOp, Graft, Graph, Import,
    MapArrow, MoveDir, Pattern, PatternCellKind, Program, Routine, Rule, SigParam, SigParamKind,
    Signature, State, SymLit, SymMap, TermKind, Transition, WriteCellKind,
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
    Ok(render_source(
        &analysis.program,
        &analysis.resolved,
        &footprint,
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
/// no doc-line field at all). No `use` lines either, on any routine: the
/// wire's `Interface` records no imports yet, so this arm has nothing to
/// read one back from.
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
            // A tape's glyph list is matched only against exported
            // alphabets the routine could spell UNQUALIFIED in source: its
            // own namespace, or any ENCLOSING namespace (an unqualified
            // name resolves outward through enclosing scopes —
            // docs/tmt/language.md (namespaces)), never a SIBLING or
            // otherwise unrelated namespace reached only through an
            // explicit `use` alias. A `use`-imported alphabet (like
            // std.tmc's volatile twins importing their representation
            // alphabet from a sibling namespace) is exactly the case this
            // excludes: the wire has no record of that `use` edge
            // (`Interface::imports` is unpopulated — see the module doc),
            // so nothing here could tell that content match apart from a
            // coincidental one, and it synthesizes instead.
            let reachable_alphabets: Vec<&ExportedAlphabet> = interface
                .alphabets
                .iter()
                .filter(|a| ns.starts_with(&split_ns(&a.name).0))
                .collect();
            // A tape whose glyph list matches no exported alphabet in its
            // OWN namespace gets a synthesized one, declared at the top
            // level — BEFORE this routine's own namespace block prints,
            // since insertion order is print order and the routine itself
            // is inserted next.
            for (param_name, glyphs) in routine.params.iter().zip(&routine.glyphs) {
                if !reachable_alphabets.iter().any(|a| &a.glyphs == glyphs) {
                    let synth = synthesized_alphabet_name(&symbol.name, param_name);
                    root.insert(&[], alphabet_lines(&synth, glyphs, false));
                }
            }
            let lines = object_routine_lines(&symbol.name, local, routine, &reachable_alphabets);
            root.insert(&ns, lines);
        }
    }

    let mut out = String::new();
    root.render(0, &mut out);
    Ok(out)
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

fn render_source(program: &Program, resolved: &Resolved, footprint: &FootprintTable) -> String {
    let worlds: HashMap<&str, &ResolvedWorld> = resolved
        .worlds
        .iter()
        .map(|w| (w.name.as_str(), w))
        .collect();

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

    // Every declaration this render will ITSELF print, by full qualified
    // name — an exported alphabet, an alphabet merely referenced (see
    // above), an exported routine, or an exported graph. This is the
    // "printed" half of the `use`-line rule (docs/tmt/cli.md
    // (interface)): a `use` line is printed only when this scope's
    // printed content references its name AND the header prints its
    // target — a `use` whose target is not in this set could not
    // possibly resolve when the header is read back, no matter how the
    // scope that declared it prints, so it is never a candidate to keep.
    let mut printed_full_names: HashSet<String> = HashSet::new();
    for alphabet in &program.alphabets {
        let full = full_name(&alphabet.ns, &alphabet.name);
        if alphabet.exported || referenced_alphabets.contains(full.as_str()) {
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
    for routine in &program.routines {
        if !routine.exported {
            continue;
        }
        let full = full_name(&routine.ns, &routine.name);
        let world = worlds[full.as_str()];
        root.insert(
            &routine.ns,
            routine_lines(routine, world, resolved, footprint),
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
    for ns in &import_scopes {
        let needed = needed_imports(
            ns,
            &program.imports,
            &program.routines,
            &program.graphs,
            &printed_full_names,
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
/// scope's printed content references its name and the header prints
/// its target; a `use` whose target is not printed is dropped — it could
/// not resolve in the header.
///
/// - referenced: the bound short name (`Import::binding`) is used,
///   unqualified, by a PRINTED declaration in that same scope — a tape
///   signature's alphabet name, or, inside a printed `export graph`
///   body, a bare `graft`/`bind` target or a bare `call` target in a
///   rule's transition. Only EXPORTED routines/graphs are scanned: those
///   are the only ones this printer ever renders a signature or body
///   for, so a reference from something the header drops (a
///   non-exported world, or a routine's own dropped body) does not count.
/// - printed: the import's TARGET (`Import::full_path`) is itself one of
///   `printed_full_names` — an exported alphabet, an alphabet this same
///   render prints because something exported references it, an
///   exported routine, or an exported graph. A target that never prints
///   (a private routine or graph reached only through the SAME import)
///   would leave the `use` line pointing at a name the header never
///   declares, so it is dropped too, even when referenced.
///
/// Source order preserved: `imports` is walked in its own (already
/// source-ordered) sequence, filtered rather than resorted.
fn needed_imports<'a>(
    ns: &[String],
    imports: &'a [Import],
    routines: &[Routine],
    graphs: &[Graph],
    printed_full_names: &HashSet<String>,
) -> Vec<&'a Import> {
    let mut referenced: HashSet<&str> = HashSet::new();
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
                && printed_full_names.contains(&imp.full_path())
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
    }
    for bind in &graph.binds {
        if let [only] = bind.target.segments.as_slice() {
            out.insert(only.as_str());
        }
    }
    for state in &graph.states {
        for rule in &state.rules {
            if let Transition::Call { target, .. } = &rule.transition
                && let [only] = target.segments.as_slice()
            {
                out.insert(only.as_str());
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
) -> Vec<String> {
    let mut lines = doc_lines(routine.doc.as_ref());
    let sig = signature_text(&routine.sig, world, resolved, footprint);
    lines.push(format!("export routine {}({});", routine.name, sig));
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

/// One tape parameter's rendered text — `tape NAME: ALPHABET writes { … }`
/// — the ONE renderer both the source arm (`sig_param_text`) and the object
/// arm (`object_routine_lines`) call, so the two can never drift apart on
/// how a parameter is formatted, only on what write set they pass in (which
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
        Transition::Call {
            target, args, then, ..
        } => format!(
            "call {}({}) then {}",
            target.joined(),
            binding_args_text(args),
            continuation_text(then)
        ),
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
        BindingValue::Named { target, map, .. } => match map {
            Some(m) => format!("{target} with map {{ {} }}", map_pairs_text(m)),
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
    map.pairs
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

fn object_routine_lines(
    routine_full_name: &str,
    local_name: &str,
    routine: &RoutineInterface,
    alphabets: &[&ExportedAlphabet],
) -> Vec<String> {
    let mut params = Vec::with_capacity(routine.params.len());
    for ((param_name, glyphs), writes) in routine
        .params
        .iter()
        .zip(&routine.glyphs)
        .zip(&routine.writes)
    {
        // A tape's alphabet has no name on the wire (docs/formats.md
        // (routine interfaces) records only its glyphs); resolving it back
        // to the identifier a `.tmc` header must spell means matching this
        // tape's full glyph list, by content, against an alphabet in the
        // routine's OWN namespace that this same object exports (`alphabets`
        // is already filtered to that namespace by the caller — a
        // cross-namespace content match is not usable without a qualified
        // alphabet reference, which the language does not have yet). A tape
        // whose alphabet no same-namespace export matches gets the SAME
        // synthesized name `from_object` already declared for it at the top
        // level (see the module doc and `synthesized_alphabet_name`) — never
        // an error, since a routine over a private alphabet is legal.
        let alphabet_name = alphabets
            .iter()
            .find(|a| &a.glyphs == glyphs)
            .map(|a| short_name(&a.name).to_string())
            .unwrap_or_else(|| synthesized_alphabet_name(routine_full_name, param_name));
        params.push(tape_param_text(param_name, &alphabet_name, writes));
    }
    vec![format!(
        "export routine {local_name}({});",
        params.join(", ")
    )]
}

/// A deterministic stand-in name for a tape's alphabet when the object
/// exports nothing with matching glyph content: the routine's own mangled
/// name with `::` replaced by `_`, joined to the parameter name by `__`
/// (docs/tmt/cli.md (interface)). Two different routines can never
/// collide on this scheme — their mangled names differ — and reusing it
/// consistently between the declaration `from_object` emits and the
/// reference `object_routine_lines` prints is what keeps the two in sync
/// without passing the synthesized name across the two call sites
/// directly.
fn synthesized_alphabet_name(routine_full_name: &str, param_name: &str) -> String {
    format!("{}__{param_name}", routine_full_name.replace("::", "_"))
}
