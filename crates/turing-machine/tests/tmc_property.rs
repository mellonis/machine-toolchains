//! `.tmc` property tests. A deterministic generator of GRAMMAR-VALID
//! programs (docs/tmt/language.md (program structure)), asserting the
//! lossless law the green tree (docs/core.md (syntax trees)) must hold
//! over every one of them.
//! Hand-written fixtures test the shapes someone thought of; this tests
//! the ones nobody did — the sibling `.pmc` migration ran six plans and
//! eight mutation-armed reviews on hand-written fixtures alone and still
//! shipped two Critical bugs living in shapes no fixture contained; a
//! generator built afterward reproduced both within thirteen cases. This
//! plan pays that cost up front.
//!
//! Every generated program is valid BY CONSTRUCTION — the generator's job
//! is to explore the space of ACCEPTED programs, not of rejected ones;
//! the parser's own tests (`parser/tests.rs`) cover rejection. A large
//! family of `.tmc` rules — duplicate names, undefined call/graft/bind
//! targets and goto labels, tape-count/vector-width mismatches, missing
//! or extra call-site bindings, entry-count, alphabet cardinality and
//! resolution, contract-clause footprint inference, `return` outside a
//! routine, and more — are enforced by `compiler.rs`'s later semantic
//! passes, never by the parser (confirmed by reading every parser
//! function this generator drives). `parse_green` never reaches any of
//! those checks, so this generator does not track them either: names,
//! `call`/`graft`/`bind` targets, `goto` labels, and alphabet references
//! never need to resolve to anything real, and pattern/write/move vector
//! widths never need to match a world's declared tape count.
//!
//! [`Cursor`] turns a `proptest`-supplied byte seed into a deterministic
//! sequence of grammar-directed choices — the same technique
//! `fmt_property.rs` uses on the sibling `.pmc` side, generalized past
//! its own narrower scope (that generator's own doc comment says
//! plainly that it emits no comments, imports, or namespaces; this one
//! must, because that narrowness is exactly what let both Critical bugs
//! through undetected).
//!
//! **Scope — deliberately covered.** Every top-level item kind (`use`,
//! `alphabet`, `routine`, `graph`, `namespace` — including a reopened
//! and a one-level-nested namespace (a namespace's body may itself
//! contain another `namespace` block, one level deep — the documented
//! `namespace std { namespace binaryNumbers { … } }` shape), `machine`);
//! both `entry` and plain forms of the two constructs that have both
//! (`state`, `graft`, the latter's non-entry form additionally
//! exercising the parser's `as`-name requirement); tape declarations,
//! plain and `volatile`; routine/graph signatures with both parameter
//! kinds (`tape`, `state`) and, on a tape parameter, `writes`/`preserves`
//! independently (neither, either alone, or both in their one legal
//! order); every pattern-cell shape (a literal, a range, a wildcard, and
//! — on either a literal or a range, never a wildcard — an `as` binding)
//! over both symbol kinds (glyph and numeric), including escaped glyphs
//! (`'\''`, `'\\'`); every write-cell shape (`-` keep, a literal,
//! a passthrough substitution, an arithmetic fold over `+ - * %` and
//! parens); every move direction; every transition shape (`goto`, the
//! bare-name sugar, `call … then` — its own binding arguments
//! occasionally carrying a nested `with map` using both arrows — against
//! every continuation kind (a state name, `return`, `stop`, `halt`), and
//! the omitted-transition "stay" shape); `bind`; qualified
//! `call`/`graft`/`bind` TARGETs (`qual_name`'s own grammar, `IDENT (::
//! IDENT)*` — usually one segment, occasionally the documented
//! `std::binaryNumbers::plusOne` multi-segment shape, and occasionally a
//! same-line block comment right after a `::`, see [`gen_qual_target`]); doc
//! runs and attention lines (including the `[deprecated]` attribute, a
//! blank `?` paragraph break, and comments interleaved inside and after
//! a run); comments in every position the grammar attaches by source
//! position rather than by an attachment pass — own-line, trailing
//! (after `;`), riding an opening brace, and, in each of four
//! representative list shapes (a `use` path list, an alphabet element
//! list, a rule's pattern vector, and a binding-argument list), both an
//! interior position and dangling after the last entry, before the
//! list's own TERMINATOR — a closing bracket for three of the four, but
//! `;` for `use`: `parser.rs::parse_use`'s `Semi` arm drains that
//! position's interior comments deliberately BEFORE consuming `;`, with
//! its own comment explaining why (draining after would wrongly claim a
//! comment documenting the NEXT `use`) — a hand-reasoned attachment
//! hazard worth generating specifically, not just the three bracketed
//! siblings that happen to share a helper; plus both single-line and
//! embedded multi-line block comments, the latter specifically because
//! `docs/core.md (syntax trees)` calls out a block comment as the one
//! token whose span crosses lines; and blank lines between every item
//! kind this generator places in a list.
//!
//! **Scope — deliberately left out, and why.** Interior comments in a
//! signature's parameter list, a write/move vector, and a symbol map's
//! pair list: every one of those list shapes drains its interior
//! comments through the same `Parser::interior_comments` helper into
//! the same `GreenSink::flush` this generator's four covered list shapes
//! already exercise, so a fifth (through eighth) instance tests the
//! shared machinery again rather than a new one. A contract clause's own
//! body deliberately accepts no interior comment at all — that is a
//! parser design choice (`parser.rs::contract_clause`'s doc comment), so
//! generating one there would test comment RELOCATION on a future `fmt`
//! pass this task's crate does not yet have, not the lossless law. `call`
//! inside a `graph` body and `return` outside a `routine`: both parse
//! successfully (neither check lives in `parser.rs`), but both are
//! compiler-rejected shapes no real `.tmc` program contains, so
//! including them would buy grammar coverage this generator already has
//! another way (`call`/`return` are exercised inside routines) at the
//! cost of programs that read as nonsense. Namespace nesting stops at
//! two levels and a machine's tape count stays small (1-3) — both are
//! open-ended in the grammar, but the trivia machinery under test does
//! not care about nesting depth or vector width once one non-trivial
//! instance is reached, so deeper recursion buys iteration cost, not
//! coverage.
//!
//! **Two shapes stay skewed rather than uniform, on purpose.** A `graph`
//! world always gets an `entry graft` and a `routine` world always gets
//! an `entry state` (`gen_reuse` ties `prefer_entry_graft` to `is_graph`
//! outright); only `machine` varies its entry shape at random. Both
//! entry shapes DO appear across the corpus — the doc's own claim above
//! is about that, and it holds — but a future property assuming every
//! WORLD KIND sees both shapes, rather than the corpus as a whole, would
//! be surprised; this is deliberate, not an oversight (a `graph`'s whole
//! purpose is to be reusable via grafting, so `entry graft` is its
//! canonical shape, and symmetrically for `routine`/`entry state`), but
//! it is a real skew worth a plan-8-through-12 author knowing about
//! before writing a per-world-kind property against this generator.
//! Separately, [`push_comment_break`]'s four established list positions
//! always insert a line break after a comment, so a same-line interior
//! BLOCK comment in one of THOSE four positions (e.g. `['a', /* n */
//! 'b']`, as opposed to after the whole vector) stays structurally
//! unreachable there — a real spelling gap, not a correctness risk
//! (parsing is safe either way; only one alternative spelling is
//! under-generated). Left as-is rather than special-cased per comment
//! kind, to keep every one of those four call sites governed by one
//! simple, always-safe rule; [`gen_qual_target`]'s qualified-path comment
//! (added specifically because a reviewer's own example showed this
//! exact shape) exercises a same-line block comment in a DIFFERENT
//! position instead, as a narrow, deliberate exception rather than a fix
//! to the four established generators.
//!
//! **What this file does NOT check.** The property below asserts exactly
//! one law — `text() == src` — and nothing about tree SHAPE. Tagging
//! every `LineComment` as `BlockComment` at emission (text byte-identical,
//! kind wrong) leaves this property green across the full case count.
//! Comment-kind and node-extent correctness are a separate law needing a
//! separate property; whoever adds one should know this file never
//! covered it, rather than reading its green result as evidence.
//!
//! Nor does anything here assert that extraction reads a generated tree
//! CORRECTLY. It once did — a struct-equality property against the C1
//! lowering of the same source — and that comparison retired with the C1
//! parse path it was one half of. What replaces it is weaker and
//! deliberately so: the coverage oracle at the bottom of this file holds
//! the set of constructs the generator WROTE equal to the set extraction
//! reports, per program and in both directions, which catches a
//! construct dropped or invented wholesale but not one rebuilt with the
//! wrong contents. Extraction's per-field fidelity is pinned by the
//! hand-written value tests in `syntax::extract`'s own module and by the
//! goldens, not here.

use mtc_core::syntax::SyntaxNode;
use mtc_turing_machine::lexer::{LexMode, lex_with};
use mtc_turing_machine::parser::{
    Continuation, FoldExprKind, FoldOp, MapArrow, MoveDir, PatternCellKind, Program, SigParamKind,
    SymLit, TermKind, Transition, WriteCellKind, parse_green, parse_green_from_tokens,
};
use mtc_turing_machine::syntax::extract_program;
use proptest::prelude::*;
use std::collections::BTreeSet;

/// A deterministic cursor over a byte seed, cycling forever so the
/// generator never has to handle running out of randomness.
struct Cursor<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        assert!(!bytes.is_empty(), "cursor needs at least one byte");
        Cursor { bytes, pos: 0 }
    }

    fn next_u8(&mut self) -> u8 {
        let b = self.bytes[self.pos % self.bytes.len()];
        self.pos += 1;
        b
    }

    /// A choice in `0..n`. Guards `n == 0` with `.max(1)` (returning 0
    /// rather than dividing by zero) even though every call site in this
    /// file happens to pass a nonzero `n` today — every dynamic-length
    /// site guards emptiness itself before calling in. The guard is
    /// cheap insurance for whichever plan next copies this exact
    /// deterministic-cursor technique (`fmt_property.rs`'s own `pick`
    /// carries the identical guard): without it, a future call site that
    /// forgets to check emptiness turns into a divide-by-zero panic
    /// instead of a shrinkable `proptest` failure.
    fn choose(&mut self, n: usize) -> usize {
        (self.next_u8() as usize) % n.max(1)
    }

    /// `true` with probability `num / den`.
    fn chance(&mut self, num: usize, den: usize) -> bool {
        self.choose(den) < num
    }
}

/// What the generator DECIDED to emit, recorded at the point each branch
/// is taken — the other side of the coverage oracle.
///
/// The invariant the floor test below enforces: for every generated
/// program, the labels recorded here and the CHOSEN labels
/// [`stamp_program`] reads back off the extracted `Program` are the SAME
/// SET, in both directions — a construct the generator emits and
/// extraction drops fails, and so does a label extraction invents.
/// [`CHOSEN_CONSTRUCTS`] is the closed list of labels this type is
/// allowed to carry, and its doc comment explains which labels are
/// deliberately absent.
#[derive(Default)]
struct Emitted(BTreeSet<&'static str>);

impl Emitted {
    fn mark(&mut self, label: &'static str) {
        self.0.insert(label);
    }
}

/// A monotonically increasing id, threaded through the whole generator so
/// every minted name (alphabet, tape, state, routine, graph, namespace,
/// signature parameter, bind/graft instance, comment tag) is unique across
/// one generated program. `.tmc` doesn't require that at parse time — none
/// of the uniqueness rules the language documents are enforced by the
/// parser (see the module doc) — but keeping identifiers distinct anyway
/// means a failing case never leaves a reader wondering whether two `s0`s
/// were meant to be the same state.
fn uid(n: &mut u32) -> u32 {
    let v = *n;
    *n += 1;
    v
}

// ---------------------------------------------------------------------------
// Literals: comments, symbols, ranges
// ---------------------------------------------------------------------------

const GLYPHS: &[&str] = &["'a'", "'b'", "'c'", "'_'", "'x'", "'^'", "'$'"];
/// The two glyph escapes the language allows (docs/tmt/language.md
/// (alphabets)): an escaped quote and an escaped backslash, each a glyph
/// whose SOURCE spelling differs from its one-character content.
const ESCAPED_GLYPHS: &[&str] = &[r"'\''", r"'\\'"];
const NUMBERS: &[&str] = &["0", "1", "2", "5", "10", "42", "007", "126"];

/// One comment, own-line or trailing depending on where the caller places
/// it (own-line-ness is positional — the lexer decides it from whether the
/// comment starts its line, not from anything the generator stamps). A
/// minority are block comments whose content embeds a real newline: the
/// one token kind whose span crosses source lines
/// (docs/core.md (syntax trees)), so `layout.rs`'s per-token end-byte math
/// gets exercised on the shape most likely to expose an off-by-line bug.
fn gen_comment(cur: &mut Cursor, n: &mut u32) -> String {
    let tag = uid(n);
    match cur.choose(8) {
        0 => format!("/* multi\n   line note {tag} */"),
        1..=3 => format!("/* note {tag} */"),
        _ => format!("// note {tag}"),
    }
}

/// Append a comment, then a hard line break — the one always-safe way to
/// place a comment INSIDE a list (an element list, a pattern vector, a
/// binding-argument list, …) ahead of more content on a later line. A `//`
/// comment consumes the rest of its source line by construction
/// (docs/tmt/language.md (program structure)), so anything meant to
/// follow it — the next list entry, or the list's own terminator — MUST
/// start a new line; a
/// block comment tolerates either spelling, so always breaking is safe for
/// both comment kinds and keeps every interior-comment call site simple.
fn push_comment_break(cur: &mut Cursor, n: &mut u32, out: &mut String) {
    out.push_str(&gen_comment(cur, n));
    out.push_str("\n    ");
}

/// One symbol literal, glyph or numeric, plus whether it's a glyph — the
/// two literal forms a range's endpoints must agree on
/// (docs/tmt/language.md (alphabets)).
fn gen_sym(cur: &mut Cursor, em: &mut Emitted) -> (String, bool) {
    if cur.chance(1, 8) {
        em.mark("sym.glyph");
        em.mark("sym.glyph.escaped");
        (
            ESCAPED_GLYPHS[cur.choose(ESCAPED_GLYPHS.len())].to_string(),
            true,
        )
    } else if cur.chance(2, 3) {
        em.mark("sym.glyph");
        (GLYPHS[cur.choose(GLYPHS.len())].to_string(), true)
    } else {
        em.mark("sym.number");
        (NUMBERS[cur.choose(NUMBERS.len())].to_string(), false)
    }
}

/// A same-kind low/high pair for a range endpoint, matching `lo`'s kind —
/// the parser rejects only a KIND mismatch (`RangeKindMismatch`), never an
/// out-of-order or improbable-looking pair (that check is a later semantic
/// one; see the module doc), so any same-kind `hi` is grammar-valid.
fn gen_range_hi(cur: &mut Cursor, em: &mut Emitted, lo_is_glyph: bool) -> String {
    if lo_is_glyph {
        em.mark("sym.glyph");
        GLYPHS[cur.choose(GLYPHS.len())].to_string()
    } else {
        em.mark("sym.number");
        NUMBERS[cur.choose(NUMBERS.len())].to_string()
    }
}

/// One alphabet-body element: a single symbol or a `lo..hi` range
/// (docs/tmt/language.md (alphabets)) — the same element grammar an
/// alphabet body and a contract clause body share.
fn gen_alphabet_elem(cur: &mut Cursor, em: &mut Emitted) -> String {
    let (lo, is_glyph) = gen_sym(cur, em);
    if cur.chance(1, 3) {
        em.mark("alphabet.elem.range");
        format!("{lo}..{}", gen_range_hi(cur, em, is_glyph))
    } else {
        em.mark("alphabet.elem.single");
        lo
    }
}

/// A comma-separated element list body (no brackets), used by both an
/// `alphabet { … }` body and a `writes`/`preserves { … }` contract clause.
/// `contract` says which: a clause body may be empty (`{}` is a real,
/// meaningful shape — "written nowhere" — distinct from an absent clause;
/// see docs/tmt/language.md (contract clauses)) and is the only one of
/// the two whose emptiness extraction stamps a label for, while a bare
/// `alphabet` body stays non-empty here since an empty one is a later
/// semantic error, not a parse one, and this generator otherwise keeps to
/// realistic shapes (see the module doc's left-out list). `interior`
/// gates one of this generator's four covered interior-comment list
/// positions.
fn gen_elem_list(
    cur: &mut Cursor,
    n: &mut u32,
    em: &mut Emitted,
    contract: bool,
    interior: bool,
    out: &mut String,
) {
    let count = if contract && cur.chance(1, 5) {
        0
    } else {
        1 + cur.choose(4)
    };
    if contract {
        em.mark(if count == 0 {
            "contract.empty"
        } else {
            "contract.non-empty"
        });
    }
    for i in 0..count {
        if i > 0 {
            out.push_str(", ");
        }
        if interior && cur.chance(1, 6) {
            push_comment_break(cur, n, out);
        }
        out.push_str(&gen_alphabet_elem(cur, em));
    }
    if interior && cur.chance(1, 8) {
        if count > 0 {
            out.push(' ');
        }
        push_comment_break(cur, n, out);
    }
}

// ---------------------------------------------------------------------------
// Doc runs
// ---------------------------------------------------------------------------

/// A `?`/`!` run (docs/tmt/language.md (doc lines and attention lines)): an
/// optional `?` block (occasionally including a blank `?` paragraph break),
/// then an optional `!` block whose first line may carry the one known
/// attribute, `[deprecated]` — any other bracketed word is a parse-time
/// `UnknownAttribute` error, so this generator never writes one. Ordinary
/// comments interleaved between lines, and one trailing the run before the
/// declaration, stay attached without breaking it
/// (docs/tmt/language.md (doc lines and attention lines)).
///
/// `record` is false for the one run whose content never reaches an
/// extracted `Program`: a `namespace`'s own doc run. A namespace is not a
/// declaration in `Program` — its path is stamped onto the declarations
/// inside it — so nothing stamps a label off its doc, and recording one
/// here would invent a label extraction can never produce.
fn gen_doc_run(
    cur: &mut Cursor,
    n: &mut u32,
    em: &mut Emitted,
    record: bool,
    pad: &str,
    out: &mut String,
) {
    // Up to THREE `?` lines, not two. At two, a blank break can only ever
    // land on the last line, and `crate::parser::reduce_doc_run` flushes
    // the pending paragraph only when it is non-empty — so a trailing
    // blank adds nothing and the reduced `Doc` never held more than one
    // paragraph, no matter how many cases ran. Measured by the coverage
    // floor below, which reported `doc.paragraphs.many` unreached; three
    // lines let the break sit BETWEEN two real ones, which is the only
    // spelling that splits.
    let docs = cur.choose(4);
    let attentions = if docs == 0 { 1 } else { cur.choose(2) };
    for i in 0..docs {
        out.push_str(pad);
        if i > 0 && cur.chance(1, 5) {
            out.push_str("?\n");
        } else {
            out.push_str(&format!("? doc line {i}\n"));
        }
        if cur.chance(1, 6) {
            out.push_str(pad);
            out.push_str(&gen_comment(cur, n));
            out.push('\n');
        }
    }
    for i in 0..attentions {
        out.push_str(pad);
        if i == 0 && cur.chance(1, 3) {
            if record {
                em.mark("doc.deprecated");
            }
            out.push_str("! [deprecated] superseded\n");
        } else {
            if record {
                em.mark("doc.attention");
            }
            out.push_str(&format!("! attention line {i}\n"));
        }
    }
    if cur.chance(1, 4) {
        out.push_str(pad);
        out.push_str(&gen_comment(cur, n));
        out.push('\n');
    }
}

// ---------------------------------------------------------------------------
// use
// ---------------------------------------------------------------------------

/// One `use a, mylib::b as c;` (docs/tmt/language.md (namespaces,
/// visibility, and imports)). `use` never accepts a doc run (absent from
/// `next_is_top_doc_accepting`), so this is never called with one pending.
/// Interior comments here are this generator's use-path-list coverage,
/// including the DANGLING position after the last path, before the `;` —
/// `parser.rs::parse_use`'s `Semi` arm drains interior comments there
/// deliberately BEFORE bumping past `;`, with its own comment explaining
/// why: draining after would wrongly claim a comment that documents the
/// NEXT `use`, not this one.
fn gen_use(cur: &mut Cursor, n: &mut u32, em: &mut Emitted, in_ns: bool, out: &mut String) {
    out.push_str("use ");
    let paths = 1 + cur.choose(3);
    for i in 0..paths {
        if i > 0 {
            out.push_str(", ");
            if cur.chance(1, 5) {
                push_comment_break(cur, n, out);
            }
        }
        // Always two segments, so every generated import is a
        // multi-segment one.
        em.mark("import.multi-segment");
        em.mark(if in_ns {
            "import.namespaced"
        } else {
            "import.file-level"
        });
        out.push_str(&format!("ns{}::sym{}", cur.choose(3), cur.choose(4)));
        if cur.chance(1, 4) {
            em.mark("import.alias");
            out.push_str(&format!(" as alias{}", uid(n)));
        } else {
            em.mark("import.bare");
        }
    }
    if cur.chance(1, 5) {
        out.push(' ');
        push_comment_break(cur, n, out);
    }
    out.push_str(";\n");
}

// ---------------------------------------------------------------------------
// Rules: patterns, write/move vectors, transitions
// ---------------------------------------------------------------------------

/// One pattern cell (docs/tmt/language.md (pattern cells)): a wildcard
/// (never bound — `* as v` is a parse-time `WildcardBinding` error), or a
/// literal/range optionally bound with `as NAME`. Returns the cell text
/// plus the binding this cell introduced, if any (name, is-glyph) — the
/// write vector needs to know which bound names are glyph-kind, since a
/// non-passthrough substitution over a glyph binding is a parse-time
/// `CharArithmetic` error (docs/tmt/language.md (substitution)).
fn gen_pattern_cell(
    cur: &mut Cursor,
    n: &mut u32,
    em: &mut Emitted,
) -> (String, Option<(String, bool)>) {
    if cur.chance(1, 6) {
        em.mark("pattern.wildcard");
        em.mark("pattern.unbound");
        return ("*".to_string(), None);
    }
    let (lo, is_glyph) = gen_sym(cur, em);
    let body = if cur.chance(1, 3) {
        em.mark("pattern.range");
        format!("{lo}..{}", gen_range_hi(cur, em, is_glyph))
    } else {
        em.mark("pattern.single");
        lo
    };
    if cur.chance(1, 2) {
        em.mark("pattern.bound");
        let name = format!("v{}", uid(n));
        (format!("{body} as {name}"), Some((name, is_glyph)))
    } else {
        em.mark("pattern.unbound");
        (body, None)
    }
}

/// A bracketed pattern vector of `arity` cells, plus the bindings it
/// introduced. `interior` gates this generator's pattern-vector interior-
/// comment coverage (one of its four covered list positions).
fn gen_pattern(
    cur: &mut Cursor,
    n: &mut u32,
    em: &mut Emitted,
    arity: usize,
    interior: bool,
) -> (String, Vec<(String, bool)>) {
    let mut out = String::from("[");
    let mut bindings = Vec::new();
    for i in 0..arity {
        if i > 0 {
            out.push_str(", ");
        }
        if interior && cur.chance(1, 6) {
            push_comment_break(cur, n, &mut out);
        }
        let (cell, binding) = gen_pattern_cell(cur, n, em);
        out.push_str(&cell);
        if let Some(b) = binding {
            bindings.push(b);
        }
    }
    if interior && cur.chance(1, 8) {
        out.push(' ');
        push_comment_break(cur, n, &mut out);
    }
    out.push(']');
    (out, bindings)
}

/// A fold-expression atom over `bindings` (docs/tmt/language.md
/// (substitution)) — never a glyph-bound name outside a bare passthrough,
/// which is enforced by never calling this for the passthrough case (see
/// `gen_write_cell`). `depth` bounds recursion through `(expr)`.
fn gen_fold_atom(cur: &mut Cursor, em: &mut Emitted, numeric: &[String], depth: u32) -> String {
    if depth > 0 && cur.chance(1, 5) {
        return format!("({})", gen_fold_expr(cur, em, numeric, depth - 1));
    }
    if !numeric.is_empty() && cur.chance(1, 2) {
        numeric[cur.choose(numeric.len())].clone()
    } else {
        // An integer literal atom, wherever it sits in the expression:
        // `stamp_fold` walks the whole tree, so one anywhere stamps
        // `write.subst.int`.
        em.mark("write.subst.int");
        NUMBERS[cur.choose(NUMBERS.len())].to_string()
    }
}

/// `mul := atom (('*' | '%') atom)*` (docs/tmt/language.md (substitution)).
/// The `*` closure is hard-capped at 3 extra terms: `cur.chance` reads the
/// SAME cursor this loop's own condition depends on, so a seed that
/// happens to make every draw come out "continue" (an all-zero seed —
/// `proptest`'s shrinker tries exactly that — always does, since `0 % 3
/// == 0` is forever inside the `1-in-3` chance) would otherwise loop
/// forever rather than merely improbably long.
fn gen_fold_mul(cur: &mut Cursor, em: &mut Emitted, numeric: &[String], depth: u32) -> String {
    let mut s = gen_fold_atom(cur, em, numeric, depth);
    for _ in 0..3 {
        if !cur.chance(1, 3) {
            break;
        }
        let op = if cur.chance(1, 2) { '*' } else { '%' };
        em.mark("write.subst.fold");
        em.mark(if op == '*' { "fold.mul" } else { "fold.rem" });
        s = format!("{s}{op}{}", gen_fold_atom(cur, em, numeric, depth));
    }
    s
}

/// `expr := mul (('+' | '-') mul)*` (docs/tmt/language.md (substitution)) —
/// capped the same way and for the same reason as [`gen_fold_mul`].
fn gen_fold_expr(cur: &mut Cursor, em: &mut Emitted, numeric: &[String], depth: u32) -> String {
    let mut s = gen_fold_mul(cur, em, numeric, depth);
    for _ in 0..3 {
        if !cur.chance(1, 3) {
            break;
        }
        let op = if cur.chance(1, 2) { '+' } else { '-' };
        em.mark("write.subst.fold");
        em.mark(if op == '+' { "fold.add" } else { "fold.sub" });
        s = format!("{s}{op}{}", gen_fold_mul(cur, em, numeric, depth));
    }
    s
}

/// One write cell: `-` (keep), a literal, or a substitution `{…}`
/// (docs/tmt/language.md (write and move vectors)). A substitution is
/// either the passthrough of one bound name (any binding kind — always
/// parse-safe, `check_char_arithmetic` exempts a bare `Var`) or an
/// arithmetic fold restricted to `bindings`' NUMERIC names plus integer
/// literals, since a fold over a glyph binding is `CharArithmetic`.
fn gen_write_cell(cur: &mut Cursor, em: &mut Emitted, bindings: &[(String, bool)]) -> String {
    match cur.choose(4) {
        0 => {
            em.mark("write.keep");
            "-".to_string()
        }
        1 => {
            em.mark("write.lit");
            gen_sym(cur, em).0
        }
        2 if !bindings.is_empty() => {
            let (name, _) = &bindings[cur.choose(bindings.len())];
            format!("{{{name}}}")
        }
        _ => {
            let numeric: Vec<String> = bindings
                .iter()
                .filter(|(_, is_glyph)| !is_glyph)
                .map(|(name, _)| name.clone())
                .collect();
            format!("{{{}}}", gen_fold_expr(cur, em, &numeric, 1))
        }
    }
}

/// A bracketed write vector of `arity` cells, or `None` (the whole vector
/// omitted keeps every cell — docs/tmt/language.md (write and move
/// vectors)).
fn gen_write_vec(
    cur: &mut Cursor,
    em: &mut Emitted,
    arity: usize,
    bindings: &[(String, bool)],
) -> Option<String> {
    if cur.chance(1, 4) {
        return None;
    }
    let mut out = String::from("write [");
    for i in 0..arity {
        if i > 0 {
            out.push_str(", ");
        }
        out.push_str(&gen_write_cell(cur, em, bindings));
    }
    out.push(']');
    Some(out)
}

/// A bracketed move vector of `arity` cells (`<`/`>`/`.`), or `None`.
fn gen_move_vec(cur: &mut Cursor, em: &mut Emitted, arity: usize) -> Option<String> {
    if cur.chance(1, 4) {
        return None;
    }
    let mut out = String::from("move [");
    for i in 0..arity {
        if i > 0 {
            out.push_str(", ");
        }
        let dir = cur.choose(3);
        em.mark(["move.left", "move.right", "move.stay"][dir]);
        out.push(["<", ">", "."][dir].chars().next().unwrap());
    }
    out.push(']');
    Some(out)
}

/// A `call`/`graft`/`bind` TARGET — a qualified name
/// (`IDENT (:: IDENT)*`, docs/tmt/language.md (namespaces, visibility, and
/// imports)) parsed by the one shared `qual_name` function all three
/// route through. Usually one bare segment, occasionally two or three —
/// `std::binaryNumbers::plusOne` is the documented spelling — and rarely
/// carrying a same-line block comment right after a `::`, no space on
/// either side (`call a::/* n */b(…)` parses and round-trips: `qual_name` never
/// calls `interior_comments`, so a comment written here is just ordinary
/// trivia between two tokens, picked up by whatever list drain runs next
/// rather than claimed as an "interior" slot the parser tracks — unlike
/// `push_comment_break`'s four list positions, nothing here requires a
/// line break first, so this is the one place in the generator that
/// deliberately keeps a block comment on the same line as what follows
/// it; see the module doc for why the other four stay break-only).
fn gen_qual_target(cur: &mut Cursor, n: &mut u32, em: &mut Emitted, prefix: &str) -> String {
    let mut out = format!("{prefix}{}", cur.choose(3));
    let extra_segments = cur.choose(3);
    em.mark(if extra_segments == 0 {
        "qualname.single-segment"
    } else {
        "qualname.multi-segment"
    });
    for _ in 0..extra_segments {
        out.push_str("::");
        if cur.chance(1, 5) {
            out.push_str(&format!("/* n{} */", uid(n)));
        }
        out.push_str(&format!("{prefix}{}", cur.choose(3)));
    }
    out
}

/// One `with map { pairs }` (docs/tmt/language.md (symbol maps)): pairs use
/// both arrows, `->` (two-way) and `=>` (read-only) — the blank-pinning and
/// completion-injectivity rules are later semantic checks the parser never
/// runs, so any same-kind-agnostic pair is grammar-valid here (symbol maps
/// are not required to relate to any real alphabet at parse time).
fn gen_sym_map(cur: &mut Cursor, em: &mut Emitted) -> String {
    let mut out = String::from("with map { ");
    let n = 1 + cur.choose(3);
    for i in 0..n {
        if i > 0 {
            out.push_str(", ");
        }
        let bidirectional = cur.chance(1, 2);
        let arrow = if bidirectional { "->" } else { "=>" };
        em.mark(if bidirectional {
            "map.bidirectional"
        } else {
            "map.read-only"
        });
        out.push_str(&format!(
            "{} {arrow} {}",
            gen_sym(cur, em).0,
            gen_sym(cur, em).0
        ));
    }
    out.push_str(" }");
    out
}

/// One `name = value` binding argument (docs/tmt/language.md (`call`,
/// `graft`, `bind`)): a bare name (optionally carrying a `with map`) or a
/// terminator (`return`/`stop`/`halt`). Neither the parameter name nor the
/// target need resolve to anything real at parse time (see the module
/// doc), so both are drawn from small synthetic pools.
fn gen_binding_arg(cur: &mut Cursor, n: &mut u32, em: &mut Emitted) -> String {
    let param = format!("p{}", cur.choose(4));
    match cur.choose(5) {
        0 => {
            em.mark("arg.terminator.return");
            format!("{param} = return")
        }
        1 => {
            em.mark("arg.terminator.stop");
            format!("{param} = stop")
        }
        2 => {
            em.mark("arg.terminator.halt");
            format!("{param} = halt")
        }
        _ => {
            let target = format!("tgt{}", uid(n));
            if cur.chance(1, 3) {
                em.mark("arg.named.mapped");
                format!("{param} = {target} {}", gen_sym_map(cur, em))
            } else {
                em.mark("arg.named.bare");
                format!("{param} = {target}")
            }
        }
    }
}

/// A `(args)` binding list, 0-3 arguments (docs/tmt/language.md (reuse) —
/// every one of `call`/`graft`/`bind` shares this list shape). `interior`
/// gates this generator's binding-argument-list interior-comment coverage
/// (the fourth of its four covered list positions). Returns the list text
/// and how many arguments it holds: `graft` and `bind` each stamp their
/// own emptiness label off that count, and the shape of those two labels
/// differs (a graft stamps both sides, a bind only the non-empty one), so
/// the caller decides rather than this shared helper.
fn gen_binding_args(
    cur: &mut Cursor,
    n: &mut u32,
    em: &mut Emitted,
    interior: bool,
) -> (String, usize) {
    let mut out = String::from("(");
    let count = cur.choose(4);
    for i in 0..count {
        if i > 0 {
            out.push_str(", ");
        }
        if interior && cur.chance(1, 6) {
            push_comment_break(cur, n, &mut out);
        }
        out.push_str(&gen_binding_arg(cur, n, em));
    }
    if interior && count > 0 && cur.chance(1, 8) {
        out.push(' ');
        push_comment_break(cur, n, &mut out);
    }
    out.push(')');
    (out, count)
}

/// A `call TARGET(args) then CONT` continuation
/// (docs/tmt/language.md (transitions)): a state name, or one of
/// `return`/`stop`/`halt` — the same four shapes a binding-argument value
/// takes, but never `with map` (a continuation is not a tape/state
/// binding).
fn gen_continuation(cur: &mut Cursor, em: &mut Emitted, states: &[String]) -> String {
    match cur.choose(4) {
        0 => {
            em.mark("continuation.return");
            "return".to_string()
        }
        1 => {
            em.mark("continuation.stop");
            "stop".to_string()
        }
        2 => {
            em.mark("continuation.halt");
            "halt".to_string()
        }
        _ if !states.is_empty() => {
            em.mark("continuation.state");
            states[cur.choose(states.len())].clone()
        }
        _ => {
            em.mark("continuation.state");
            "elsewhere".to_string()
        }
    }
}

/// One rule's transition (docs/tmt/language.md (transitions)): `goto`,
/// the bare-name sugar, `stop`, `halt`, or omitted entirely (legal only
/// when `has_action` is true — "stay in the current state",
/// docs/tmt/language.md (transitions)) are always available. `call …
/// then …` is offered only when `allow_call` — this generator never
/// writes one inside a `graph` body (see the module doc's left-out
/// list). `return` is offered only when `in_routine`
/// (`ReturnOutsideRoutine` is a later semantic check the parser never
/// runs, but this generator keeps to realistic shapes).
fn gen_transition(
    cur: &mut Cursor,
    n: &mut u32,
    em: &mut Emitted,
    states: &[String],
    in_routine: bool,
    allow_call: bool,
    has_action: bool,
) -> String {
    // The always-available 4 (goto, bare-name, stop, halt), plus `call`
    // and/or `return` when this world allows them — indices into this
    // list are what `choice` below selects, so the shape a given index
    // means depends on which of the two optional slots are present.
    let mut shapes: Vec<u8> = vec![0, 1, 2, 3];
    if allow_call {
        shapes.push(4);
    }
    if in_routine {
        shapes.push(5);
    }
    let total = if has_action {
        shapes.len() + 1
    } else {
        shapes.len()
    };
    let choice = cur.choose(total);
    if has_action && choice == shapes.len() {
        em.mark("transition.stay");
        return String::new(); // omitted — "stay in the current state"
    }
    match shapes[choice] {
        0 => {
            em.mark("transition.goto.explicit");
            format!("goto {}", gen_target(cur, n, states))
        }
        1 => {
            em.mark("transition.goto.sugar");
            gen_target(cur, n, states) // bare-name sugar
        }
        2 => {
            em.mark("transition.stop");
            "stop".to_string()
        }
        3 => {
            em.mark("transition.halt");
            "halt".to_string()
        }
        4 => {
            em.mark("transition.call");
            let callee = gen_qual_target(cur, n, em, "callee");
            let interior = cur.chance(1, 4);
            let (args, _) = gen_binding_args(cur, n, em, interior);
            let then = gen_continuation(cur, em, states);
            format!("call {callee}{args} then {then}")
        }
        _ => {
            em.mark("transition.return");
            "return".to_string()
        }
    }
}

/// A `goto`/bare-name/`call…then` target: an existing state of this world
/// (favored, when there is one) or a synthetic name — neither needs to
/// resolve at parse time (see the module doc).
fn gen_target(cur: &mut Cursor, n: &mut u32, states: &[String]) -> String {
    if !states.is_empty() && cur.choose(3) != 0 {
        states[cur.choose(states.len())].clone()
    } else {
        format!("s{}", uid(n))
    }
}

/// One `pattern -> action transition;` rule (docs/tmt/language.md (the
/// rule triple)), appended as a whole line at `pad`, with 0-2 own-line
/// comments above it and an occasional trailing comment. Grammar-valid
/// but not shipped-corpus-spelled: when a write or move vector precedes
/// an omitted ("stay") transition, the vector's own trailing space
/// combines with the unconditional `;` pushed below to spell
/// `move [>] ;` — a stray space before `;` no hand-written `.tmc` file
/// is spelled with — worth knowing before hanging a formatter-
/// idempotence property on this generator.
#[allow(clippy::too_many_arguments)]
fn gen_rule(
    cur: &mut Cursor,
    n: &mut u32,
    em: &mut Emitted,
    pad: &str,
    arity: usize,
    states: &[String],
    in_routine: bool,
    allow_call: bool,
    out: &mut String,
) {
    for _ in 0..cur.choose(2) {
        out.push_str(pad);
        out.push_str(&gen_comment(cur, n));
        out.push('\n');
    }
    out.push_str(pad);
    let pattern_interior = cur.chance(1, 3);
    let (pattern, bindings) = gen_pattern(cur, n, em, arity, pattern_interior);
    out.push_str(&pattern);
    out.push_str(" -> ");
    let debugger = cur.chance(1, 8);
    em.mark(if debugger {
        "rule.debugger"
    } else {
        "rule.no-debugger"
    });
    if debugger {
        out.push_str("debugger ");
    }
    let write = gen_write_vec(cur, em, arity, &bindings);
    em.mark(if write.is_some() {
        "rule.write-vec"
    } else {
        "rule.no-write-vec"
    });
    if let Some(w) = &write {
        out.push_str(w);
        out.push(' ');
    }
    let mov = gen_move_vec(cur, em, arity);
    em.mark(if mov.is_some() {
        "rule.move-vec"
    } else {
        "rule.no-move-vec"
    });
    if let Some(m) = &mov {
        out.push_str(m);
        out.push(' ');
    }
    let has_action = debugger || write.is_some() || mov.is_some();
    let transition = gen_transition(cur, n, em, states, in_routine, allow_call, has_action);
    out.push_str(&transition);
    out.push(';');
    if cur.chance(1, 5) {
        out.push(' ');
        out.push_str(&gen_comment(cur, n));
    }
    out.push('\n');
}

// ---------------------------------------------------------------------------
// World bodies: state, graft, bind, tape
// ---------------------------------------------------------------------------

/// A `[entry] state NAME { rules }` (docs/tmt/language.md (worlds));
/// `entry` and plain are both exercised (`entry_ok` gates whether this
/// call MAY be the world's one entry — the caller reserves that for at
/// most the first state, exactly like `fmt_property.rs`'s `volatile_ok`
/// reserves `volatile` for `main`). An occasional comment rides the
/// opening `{`.
#[allow(clippy::too_many_arguments)]
fn gen_state(
    cur: &mut Cursor,
    n: &mut u32,
    em: &mut Emitted,
    pad: &str,
    name: &str,
    entry: bool,
    arity: usize,
    states: &[String],
    in_routine: bool,
    allow_call: bool,
    out: &mut String,
) {
    if cur.chance(1, 3) {
        em.mark("state.documented");
        gen_doc_run(cur, n, em, true, pad, out);
    }
    out.push_str(pad);
    em.mark(if entry { "state.entry" } else { "state.plain" });
    if entry {
        out.push_str("entry ");
    }
    out.push_str(&format!("state {name} {{"));
    if cur.chance(1, 4) {
        out.push(' ');
        out.push_str(&gen_comment(cur, n));
    }
    out.push('\n');
    let inner_pad = format!("{pad}  ");
    let rules = if cur.chance(1, 6) {
        0
    } else {
        1 + cur.choose(4)
    };
    if rules == 0 {
        em.mark("state.ruleless");
    }
    for i in 0..rules {
        if i > 0 && cur.chance(1, 5) {
            out.push('\n');
        }
        gen_rule(
            cur, n, em, &inner_pad, arity, states, in_routine, allow_call, out,
        );
    }
    if cur.chance(1, 6) {
        out.push_str(&inner_pad);
        out.push_str(&gen_comment(cur, n));
        out.push('\n');
    }
    out.push_str(pad);
    out.push('}');
    if cur.chance(1, 6) {
        out.push(' ');
        out.push_str(&gen_comment(cur, n));
    }
    out.push('\n');
}

/// A `[entry] graft TARGET(args) [as NAME];` (docs/tmt/language.md
/// (`graft`)). A non-entry graft MUST carry `as NAME` — omitting it is a
/// parse-time `GraftNeedsName` error — so `entry` and plain are the two
/// shapes exercised, and only the entry form ever omits the name.
fn gen_graft(
    cur: &mut Cursor,
    n: &mut u32,
    em: &mut Emitted,
    pad: &str,
    entry: bool,
    out: &mut String,
) {
    if cur.chance(1, 4) {
        em.mark("graft.documented");
        gen_doc_run(cur, n, em, true, pad, out);
    }
    out.push_str(pad);
    em.mark(if entry { "graft.entry" } else { "graft.plain" });
    if entry {
        out.push_str("entry ");
    }
    let target = gen_qual_target(cur, n, em, "graph");
    let interior = cur.chance(1, 4);
    let (args, arg_count) = gen_binding_args(cur, n, em, interior);
    em.mark(if arg_count == 0 {
        "graft.args.empty"
    } else {
        "graft.args.non-empty"
    });
    out.push_str(&format!("graft {target}{args}"));
    let needs_name = !entry || cur.chance(2, 3);
    em.mark(if needs_name {
        "graft.named"
    } else {
        "graft.anonymous"
    });
    if needs_name {
        out.push_str(&format!(" as inst{}", uid(n)));
    }
    out.push(';');
    if cur.chance(1, 5) {
        out.push(' ');
        out.push_str(&gen_comment(cur, n));
    }
    out.push('\n');
}

/// A `bind TARGET(args) as NAME;` (docs/tmt/language.md (`bind`)) — the
/// instance name is always mandatory, unlike a graft's.
fn gen_bind(cur: &mut Cursor, n: &mut u32, em: &mut Emitted, pad: &str, out: &mut String) {
    em.mark("bind");
    if cur.chance(1, 4) {
        em.mark("bind.documented");
        gen_doc_run(cur, n, em, true, pad, out);
    }
    out.push_str(pad);
    let target = gen_qual_target(cur, n, em, "callee");
    let interior = cur.chance(1, 4);
    let (args, arg_count) = gen_binding_args(cur, n, em, interior);
    if arg_count > 0 {
        em.mark("bind.args.non-empty");
    }
    out.push_str(&format!("bind {target}{args} as h{};", uid(n)));
    if cur.chance(1, 5) {
        out.push(' ');
        out.push_str(&gen_comment(cur, n));
    }
    out.push('\n');
}

/// A `[volatile] tape NAME: ALPHABET;` (docs/tmt/language.md (tapes and
/// heads), (volatile tapes)) — grammatical only inside a `machine` body;
/// a routine/graph takes tapes from its signature instead
/// (`TapeNotInMachine` is a parse-time error this generator never
/// triggers by construction, since it only calls this from `gen_machine`).
fn gen_tape(cur: &mut Cursor, n: &mut u32, em: &mut Emitted, pad: &str, out: &mut String) {
    out.push_str(pad);
    let volatile = cur.chance(1, 3);
    em.mark(if volatile {
        "tape.volatile"
    } else {
        "tape.plain"
    });
    if volatile {
        out.push_str("volatile ");
    }
    let alphabet = ["ab", "bytes", "chars", "mixed"][cur.choose(4)];
    out.push_str(&format!("tape t{}: {alphabet};", cur.choose(6)));
    if cur.chance(1, 5) {
        out.push(' ');
        out.push_str(&gen_comment(cur, n));
    }
    out.push('\n');
}

/// A world body's states/grafts/binds, interleaved with own-line comments
/// and blank lines. Reserves the entry slot for the FIRST state (mirrors
/// `fmt_property.rs`'s reservation pattern for a single per-scope
/// property), unless `prefer_entry_graft` asks for an entry graft
/// instead — so both entry-bearing constructs get generated somewhere in
/// the corpus. Both callers pin `prefer_entry_graft` to their own world
/// kind rather than randomizing it (`gen_reuse`: `is_graph`; `gen_machine`
/// alone randomizes) — see the module doc's "two shapes stay skewed"
/// paragraph for why that's a deliberate per-world-kind choice, not an
/// oversight.
#[allow(clippy::too_many_arguments)]
fn gen_world_items(
    cur: &mut Cursor,
    n: &mut u32,
    em: &mut Emitted,
    pad: &str,
    arity: usize,
    in_routine: bool,
    allow_call: bool,
    prefer_entry_graft: bool,
    out: &mut String,
) {
    let mut states: Vec<String> = Vec::new();
    let item_count = 1 + cur.choose(4);
    let mut entry_placed = false;
    for i in 0..item_count {
        if i > 0 && cur.chance(1, 5) {
            out.push('\n');
        }
        if i > 0 && cur.chance(1, 8) {
            out.push_str(pad);
            out.push_str(&gen_comment(cur, n));
            out.push('\n');
        }
        let want_entry = !entry_placed && (i == item_count - 1 || cur.chance(1, 3));
        if want_entry && prefer_entry_graft {
            entry_placed = true;
            gen_graft(cur, n, em, pad, true, out);
            continue;
        }
        match cur.choose(4) {
            0 => {
                let entry = want_entry;
                entry_placed |= entry;
                let name = format!("s{}", uid(n));
                gen_state(
                    cur, n, em, pad, &name, entry, arity, &states, in_routine, allow_call, out,
                );
                states.push(name);
            }
            1 => gen_graft(cur, n, em, pad, false, out),
            _ => gen_bind(cur, n, em, pad, out),
        }
    }
    if !entry_placed {
        // Every generated world gets exactly one entry — reserved here
        // when the loop above never happened to place one (a small
        // remaining-probability case), always as an entry state so the
        // shape stays simple.
        let name = format!("s{}", uid(n));
        out.push('\n');
        gen_state(
            cur, n, em, pad, &name, true, arity, &states, in_routine, allow_call, out,
        );
    }
}

// ---------------------------------------------------------------------------
// Signatures, routine/graph, machine, namespace, program
// ---------------------------------------------------------------------------

/// A `routine`/`graph` signature (docs/tmt/language.md (tapes and heads)):
/// `tape NAME: ALPHABET` parameters, each occasionally `volatile` and
/// occasionally carrying `writes { … }`, `preserves { … }`, both, or
/// neither — the two clauses are each independently optional
/// (docs/tmt/language.md (contract clauses)), and when both appear they
/// stay in their one legal order, `writes` then `preserves`
/// (`ContractClauseOrder`/`DuplicateContractClause` are parse-time errors
/// this generator avoids by construction, never generating either clause
/// twice or `preserves` before `writes`); a `graph` additionally gets
/// `state` exit parameters. Returns the tape
/// arity (the vector width every rule in this world's body must use) and
/// the exit-state parameter names — present for the `state exit0` shape
/// itself, but the caller discards them (`gen_reuse`'s `_exits`): a real
/// graft site binding them by name lives in a DIFFERENT declaration (the
/// host world), disconnected from this signature in this generator's flat
/// per-declaration generation, so a graft's binding-argument names are
/// synthetic (`p{N}`) rather than these exit names — grammar-valid either
/// way, since a binding argument's name need not resolve at parse time
/// (see the module doc).
fn gen_signature(
    cur: &mut Cursor,
    n: &mut u32,
    em: &mut Emitted,
    is_graph: bool,
    out: &mut String,
) -> (usize, Vec<String>) {
    out.push('(');
    let tapes = 1 + cur.choose(2);
    let mut exits = Vec::new();
    for i in 0..tapes {
        if i > 0 {
            out.push_str(", ");
        }
        em.mark("sig.param.tape");
        let volatile = cur.chance(1, 4);
        em.mark(if volatile {
            "sig.tape.volatile"
        } else {
            "sig.tape.plain"
        });
        if volatile {
            out.push_str("volatile ");
        }
        let alphabet = ["ab", "bytes", "chars"][cur.choose(3)];
        out.push_str(&format!("tape tp{}: {alphabet}", cur.choose(4)));
        // `writes` and `preserves` are each INDEPENDENTLY optional
        // (docs/tmt/language.md (contract clauses)) — `preserves {}` is
        // legal with no `writes` at all — so the two draws below are
        // separate chances, not nested. When both fire they still emit
        // in the one legal order, `writes` then `preserves`.
        let want_writes = cur.chance(1, 3);
        let want_preserves = cur.chance(1, 3);
        em.mark(match (want_writes, want_preserves) {
            (true, true) => "sig.tape.both-clauses",
            (true, false) => "sig.tape.writes-only",
            (false, true) => "sig.tape.preserves-only",
            (false, false) => "sig.tape.no-clause",
        });
        if want_writes {
            out.push_str(" writes { ");
            gen_elem_list(cur, n, em, true, false, out);
            out.push_str(" }");
        }
        if want_preserves {
            out.push_str(" preserves { ");
            gen_elem_list(cur, n, em, true, false, out);
            out.push_str(" }");
        }
    }
    if is_graph {
        let exit_count = 1 + cur.choose(2);
        for _ in 0..exit_count {
            em.mark("sig.param.state");
            out.push_str(", ");
            let name = format!("exit{}", uid(n));
            out.push_str(&format!("state {name}"));
            exits.push(name);
        }
    }
    out.push(')');
    (tapes, exits)
}

/// One `export? routine|graph NAME(sig) { … }` (docs/tmt/language.md
/// (worlds)).
fn gen_reuse(
    cur: &mut Cursor,
    n: &mut u32,
    em: &mut Emitted,
    pad: &str,
    ns_depth: usize,
    is_graph: bool,
    out: &mut String,
) {
    em.mark(if is_graph { "graph" } else { "routine" });
    if ns_depth > 0 {
        em.mark(if is_graph {
            "graph.namespaced"
        } else {
            "routine.namespaced"
        });
    }
    if ns_depth > 1 {
        em.mark("namespace.nested");
    }
    if cur.chance(1, 3) {
        em.mark(if is_graph {
            "graph.documented"
        } else {
            "routine.documented"
        });
        gen_doc_run(cur, n, em, true, pad, out);
    }
    out.push_str(pad);
    if cur.chance(1, 3) {
        em.mark(if is_graph {
            "graph.exported"
        } else {
            "routine.exported"
        });
        out.push_str("export ");
    }
    let kw = if is_graph { "graph" } else { "routine" };
    let name = format!("{}{}", if is_graph { "g" } else { "r" }, uid(n));
    out.push_str(&format!("{kw} {name}"));
    let (arity, _exits) = gen_signature(cur, n, em, is_graph, out);
    out.push_str(" {");
    if cur.chance(1, 4) {
        out.push(' ');
        out.push_str(&gen_comment(cur, n));
    }
    out.push('\n');
    // `call` is deliberately never generated inside a `graph` body — see
    // the module doc's left-out list — so `allow_call`, like
    // `in_routine`, is true only for an actual routine.
    gen_world_items(
        cur,
        n,
        em,
        &format!("{pad}  "),
        arity,
        !is_graph,
        !is_graph,
        is_graph,
        out,
    );
    out.push_str(pad);
    out.push('}');
    if cur.chance(1, 6) {
        out.push(' ');
        out.push_str(&gen_comment(cur, n));
    }
    out.push('\n');
}

/// The single `machine { … }` block (docs/tmt/language.md (worlds)): 1-3
/// `tape` declarations (plain and `volatile`), then world items. A
/// program has at most one `machine` — a second is a parse-time
/// `MultipleMachines` error — so `generate_program` calls this exactly
/// once.
fn gen_machine(cur: &mut Cursor, n: &mut u32, em: &mut Emitted, out: &mut String) {
    em.mark("machine");
    if cur.chance(1, 3) {
        em.mark("machine.documented");
        gen_doc_run(cur, n, em, true, "", out);
    }
    out.push_str("machine {");
    if cur.chance(1, 4) {
        out.push(' ');
        out.push_str(&gen_comment(cur, n));
    }
    out.push('\n');
    let arity = 1 + cur.choose(3);
    for _ in 0..arity {
        gen_tape(cur, n, em, "  ", out);
    }
    out.push('\n');
    let prefer_entry_graft = cur.chance(1, 3);
    // `return` is a routine-only shape (see the module doc); `call` IS
    // legitimate directly in a machine body (docs/tmt/language.md
    // (worlds) shows exactly this: `call touch(t = main) then done;`).
    gen_world_items(
        cur,
        n,
        em,
        "  ",
        arity,
        false,
        true,
        prefer_entry_graft,
        out,
    );
    out.push('}');
    if cur.chance(1, 6) {
        out.push(' ');
        out.push_str(&gen_comment(cur, n));
    }
    out.push('\n');
}

/// A `namespace NAME { … }` (docs/tmt/language.md (namespaces,
/// visibility, and imports)): `use`, `alphabet`, `routine`, `graph`, and
/// a nested `namespace` (bounded to one further level — see the module
/// doc). `machine` is deliberately never generated here: `parser.rs`
/// rejects one nested in a namespace at parse time (`Expected` naming
/// the reason), so `generate_program` places the program's one `machine`
/// only at file scope.
///
/// `ns_names` is the pool of namespace names minted so far in this
/// program; reusing one — a REOPENED namespace, "its own node,
/// declarations accumulate under the same path"
/// (docs/tmt/language.md (namespaces, visibility, and imports)) — is a
/// real, explicitly legal shape distinct from two differently-named
/// siblings, so this generator occasionally spells one.
#[allow(clippy::too_many_arguments)]
fn gen_namespace(
    cur: &mut Cursor,
    n: &mut u32,
    em: &mut Emitted,
    pad: &str,
    depth: u32,
    ns_depth: usize,
    ns_names: &mut Vec<String>,
    out: &mut String,
) {
    if cur.chance(1, 3) {
        // `record: false` — a namespace's own doc run reaches no
        // extracted declaration; see `gen_doc_run`.
        gen_doc_run(cur, n, em, false, pad, out);
    }
    out.push_str(pad);
    let name = if !ns_names.is_empty() && cur.chance(1, 4) {
        ns_names[cur.choose(ns_names.len())].clone()
    } else {
        let fresh = format!("ns{}", uid(n));
        ns_names.push(fresh.clone());
        fresh
    };
    out.push_str(&format!("namespace {name} {{"));
    if cur.chance(1, 4) {
        out.push(' ');
        out.push_str(&gen_comment(cur, n));
    }
    out.push('\n');
    let inner_pad = format!("{pad}  ");
    let items = 1 + cur.choose(3);
    for i in 0..items {
        if i > 0 && cur.chance(1, 3) {
            out.push('\n');
        }
        gen_top_item(cur, n, em, &inner_pad, depth, ns_depth + 1, ns_names, out);
    }
    out.push_str(pad);
    out.push('}');
    if cur.chance(1, 6) {
        out.push(' ');
        out.push_str(&gen_comment(cur, n));
    }
    out.push('\n');
}

/// One file- or namespace-level item: `use`, `alphabet`, `routine`,
/// `graph`, or (when `depth` allows) a nested `namespace`. Never a
/// `machine` — see `gen_namespace`'s doc; `generate_program` is the only
/// caller that ever places one, and only at `depth == 0`.
///
/// `depth` is the recursion budget for nested `namespace` blocks;
/// `ns_depth` is how many namespaces this item is written INSIDE, which
/// is what extraction stamps onto the declaration as its `ns` path. The
/// two count in opposite directions and are not interchangeable.
#[allow(clippy::too_many_arguments)]
fn gen_top_item(
    cur: &mut Cursor,
    n: &mut u32,
    em: &mut Emitted,
    pad: &str,
    depth: u32,
    ns_depth: usize,
    ns_names: &mut Vec<String>,
    out: &mut String,
) {
    let choices = if depth > 0 { 5 } else { 4 };
    match cur.choose(choices) {
        0 => {
            out.push_str(pad);
            gen_use(cur, n, em, ns_depth > 0, out);
        }
        1 => {
            if ns_depth > 0 {
                em.mark("alphabet.namespaced");
            }
            if ns_depth > 1 {
                em.mark("namespace.nested");
            }
            if cur.chance(1, 3) {
                em.mark("alphabet.documented");
                gen_doc_run(cur, n, em, true, pad, out);
            } else {
                em.mark("alphabet.undocumented");
            }
            out.push_str(pad);
            let exported = cur.chance(1, 3);
            em.mark(if exported {
                "alphabet.exported"
            } else {
                "alphabet.private"
            });
            if exported {
                out.push_str("export ");
            }
            let name = format!("ab{}", uid(n));
            out.push_str(&format!("alphabet {name} {{"));
            if cur.chance(1, 4) {
                out.push(' ');
                // An open-brace trailing comment must be followed by a
                // line break before the element list continues — see
                // `push_comment_break`'s doc; the same rule applies here
                // even though this is a brace comment, not a list one,
                // since a `//` form still consumes the rest of its line.
                push_comment_break(cur, n, out);
            } else {
                out.push(' ');
            }
            let elems_interior = cur.chance(1, 3);
            gen_elem_list(cur, n, em, false, elems_interior, out);
            out.push_str(" }");
            if cur.chance(1, 6) {
                out.push(' ');
                out.push_str(&gen_comment(cur, n));
            }
            out.push('\n');
        }
        2 => gen_reuse(cur, n, em, pad, ns_depth, false, out),
        3 => gen_reuse(cur, n, em, pad, ns_depth, true, out),
        _ => gen_namespace(cur, n, em, pad, depth - 1, ns_depth, ns_names, out),
    }
}

/// A whole grammar-valid `.tmc` program: optional file-leading comments,
/// then 2-5 top-level items with the program's one `machine` block
/// spliced in at a random position among them, separated by the author's
/// own blank lines.
///
/// Returns the source and the set of construct labels the generator
/// CHOSE while writing it — see [`Emitted`].
fn generate_program(seed: &[u8]) -> (String, Emitted) {
    let mut cur = Cursor::new(seed);
    let mut n = 0u32;
    let mut em = Emitted::default();
    let mut out = String::new();
    let mut ns_names: Vec<String> = Vec::new();

    for _ in 0..cur.choose(3) {
        out.push_str(&gen_comment(&mut cur, &mut n));
        out.push('\n');
    }

    let units = 2 + cur.choose(4);
    let machine_at = cur.choose(units);
    for u in 0..units {
        if u > 0 && cur.chance(1, 2) {
            out.push('\n');
        }
        if u == machine_at {
            gen_machine(&mut cur, &mut n, &mut em, &mut out);
        } else {
            // depth = 2: a top-level namespace (depth 2 -> 1) may itself
            // contain one nested namespace (depth 1 -> 0), matching
            // "Namespace nesting stops at two levels" below. `depth = 1`
            // here was a bug — it let `gen_top_item` pick the namespace
            // arm at the top level, but that namespace's own body was
            // then generated at depth 0, where the arm is unreachable, so
            // a namespace could never actually contain another one.
            gen_top_item(&mut cur, &mut n, &mut em, "", 2, 0, &mut ns_names, &mut out);
        }
    }
    (out, em)
}

/// The `Program` one generated source extracts to, through the same
/// front end the compiler runs: a `WithComments` lex, the green parse,
/// then extraction.
fn extracted(src: &str) -> Program {
    let tokens = lex_with(src, LexMode::WithComments)
        .unwrap_or_else(|e| panic!("generator emitted an unlexable program: {e:?}\n{src}"));
    let green = parse_green_from_tokens(src, &tokens)
        .unwrap_or_else(|e| panic!("generator emitted an invalid program: {e:?}\n{src}"));
    extract_program(&SyntaxNode::new_root(green), src)
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(2000))]

    /// The lossless law over generated programs: the green tree's own
    /// text reconstructs the source, byte for byte
    /// (docs/core.md (syntax trees)).
    #[test]
    fn generated_programs_round_trip(seed in prop::collection::vec(any::<u8>(), 1..512)) {
        let (src, _) = generate_program(&seed);
        let tree = parse_green(&src)
            .unwrap_or_else(|e| panic!("generator emitted an invalid program: {e:?}\n{src}"));
        let root = SyntaxNode::new_root(tree);
        prop_assert_eq!(root.text(), src);
    }
}

// ---------------------------------------------------------------------------
// Coverage floor
// ---------------------------------------------------------------------------

/// Every construct `syntax::extract` has its own branch for, as a label
/// the tally below stamps when it observes one in an EXTRACTED
/// `Program`. Measuring the extracted AST rather than the generated text
/// is the point: `src.contains("writes")` would measure the generator,
/// while `SigParamKind::Tape { writes: Some(_), .. }` measures what
/// extraction actually had to rebuild.
///
/// This is the SWEEP-WIDE floor: every label here must be observed at
/// least once across the whole sweep, and no label outside it may ever
/// be stamped. Both directions, matching this repo's other drift guards
/// — a required label never stamped means the sweep runs blind on that
/// construct, and a label stamped but not required means the tally and
/// this list disagree about a name. The stricter PER-PROGRAM check lives
/// in [`CHOSEN_CONSTRUCTS`].
///
/// **Three axes are deliberately absent, each for a measured reason.**
/// `machine: None` (a library `.tmc`) — `generate_program` always emits
/// exactly one `machine` block, and the shape is covered by the shipped
/// corpus, where the embedded stdlib is exactly that.
/// `entry state` inside a `graph` and `entry graft` inside a `routine` —
/// the generator ties the entry shape to the world kind on purpose (its
/// own module doc says why), and extraction reads `entry` off the STATE
/// or GRAFT view with no knowledge of the enclosing world, so the cross
/// is not a distinct branch; the two axes themselves (`state.entry`,
/// `graft.entry`) are required below. `doc.deprecated` with an empty
/// message — `gen_doc_run` always writes `! [deprecated] superseded`,
/// and the empty-message reduction is a `reduce_doc_run` behaviour the
/// green path shares with the CST path rather than re-deriving.
const REQUIRED_CONSTRUCTS: &[&str] = &[
    // imports
    "import.alias",
    "import.bare",
    "import.multi-segment",
    "import.namespaced",
    "import.file-level",
    // alphabets
    "alphabet.exported",
    "alphabet.private",
    "alphabet.documented",
    "alphabet.undocumented",
    "alphabet.namespaced",
    "alphabet.elem.single",
    "alphabet.elem.range",
    // symbol literals, wherever they occur
    "sym.glyph",
    "sym.glyph.escaped",
    "sym.number",
    "sym.number.leading-zero",
    // reuse declarations
    "routine",
    "routine.exported",
    "routine.documented",
    "routine.namespaced",
    "graph",
    "graph.exported",
    "graph.documented",
    "graph.namespaced",
    "namespace.nested",
    // signatures
    "sig.param.state",
    "sig.param.tape",
    "sig.tape.volatile",
    "sig.tape.plain",
    "sig.tape.writes-only",
    "sig.tape.preserves-only",
    "sig.tape.both-clauses",
    "sig.tape.no-clause",
    "contract.empty",
    "contract.non-empty",
    // machine
    "machine",
    "machine.documented",
    "tape.volatile",
    "tape.plain",
    // world bodies
    "state.entry",
    "state.plain",
    "state.documented",
    "state.ruleless",
    "graft.entry",
    "graft.plain",
    "graft.named",
    "graft.anonymous",
    "graft.documented",
    "graft.args.empty",
    "graft.args.non-empty",
    "bind",
    "bind.documented",
    "bind.args.non-empty",
    "qualname.single-segment",
    "qualname.multi-segment",
    // rules
    "rule.debugger",
    "rule.no-debugger",
    "rule.write-vec",
    "rule.no-write-vec",
    "rule.move-vec",
    "rule.no-move-vec",
    "pattern.wildcard",
    "pattern.single",
    "pattern.range",
    "pattern.bound",
    "pattern.unbound",
    "write.keep",
    "write.lit",
    "write.subst.passthrough",
    "write.subst.int",
    "write.subst.fold",
    "fold.add",
    "fold.sub",
    "fold.mul",
    "fold.rem",
    "fold.nested",
    "move.left",
    "move.right",
    "move.stay",
    // transitions
    "transition.goto.explicit",
    "transition.goto.sugar",
    "transition.call",
    "transition.return",
    "transition.stop",
    "transition.halt",
    "transition.stay",
    "continuation.state",
    "continuation.return",
    "continuation.stop",
    "continuation.halt",
    // binding arguments and symbol maps
    "arg.named.mapped",
    "arg.named.bare",
    "arg.terminator.return",
    "arg.terminator.stop",
    "arg.terminator.halt",
    "map.bidirectional",
    "map.read-only",
    // reduced docs
    "doc.paragraphs.one",
    "doc.paragraphs.many",
    "doc.attention",
    "doc.deprecated",
];

/// The CHOSEN subset of [`REQUIRED_CONSTRUCTS`]: the labels the
/// generator can record at the moment it takes the branch, so that for
/// every single generated program the generator's recorded set and
/// [`stamp_program`]'s set restricted to this list must be EQUAL. That
/// per-program equality is the strong half of the coverage oracle; the
/// sweep-wide floor above is the weak half.
///
/// **The five labels deliberately left out are DERIVED, not chosen.**
/// Each is a consequence of a shape rather than of a decision, so the
/// generator could only record it by re-deriving what extraction does —
/// and a two-directional set-compare over a re-derivation fails on
/// precisely the interesting cases. They stay covered by the
/// sweep-wide floor, which is what they can honestly carry:
///
/// - `sym.number.leading-zero` — stamped from the WRITTEN spelling
///   (`written.len() > 1 && written.starts_with('0')`), a property of
///   which entry of `NUMBERS` a draw landed on, not of a branch.
/// - `doc.paragraphs.one` / `doc.paragraphs.many` — the count comes out
///   of `crate::parser::reduce_doc_run`'s fold over blank `?` lines, not
///   out of how many `?` lines were written.
/// - `write.subst.passthrough` — stamped when the substitution's WHOLE
///   expression is a bare `Var`, which depends on how the fold's
///   operators and parens nest, not on which `gen_write_cell` arm ran
///   (the arithmetic arm can produce a bare `Var` too).
/// - `fold.nested` — stamped when a `Bin` node holds another, i.e. on
///   the total operator count of one cell's expression across its
///   parenthesised sub-expressions.
const CHOSEN_CONSTRUCTS: &[&str] = &[
    "import.alias",
    "import.bare",
    "import.multi-segment",
    "import.namespaced",
    "import.file-level",
    "alphabet.exported",
    "alphabet.private",
    "alphabet.documented",
    "alphabet.undocumented",
    "alphabet.namespaced",
    "alphabet.elem.single",
    "alphabet.elem.range",
    "sym.glyph",
    "sym.glyph.escaped",
    "sym.number",
    "routine",
    "routine.exported",
    "routine.documented",
    "routine.namespaced",
    "graph",
    "graph.exported",
    "graph.documented",
    "graph.namespaced",
    "namespace.nested",
    "sig.param.state",
    "sig.param.tape",
    "sig.tape.volatile",
    "sig.tape.plain",
    "sig.tape.writes-only",
    "sig.tape.preserves-only",
    "sig.tape.both-clauses",
    "sig.tape.no-clause",
    "contract.empty",
    "contract.non-empty",
    "machine",
    "machine.documented",
    "tape.volatile",
    "tape.plain",
    "state.entry",
    "state.plain",
    "state.documented",
    "state.ruleless",
    "graft.entry",
    "graft.plain",
    "graft.named",
    "graft.anonymous",
    "graft.documented",
    "graft.args.empty",
    "graft.args.non-empty",
    "bind",
    "bind.documented",
    "bind.args.non-empty",
    "qualname.single-segment",
    "qualname.multi-segment",
    "rule.debugger",
    "rule.no-debugger",
    "rule.write-vec",
    "rule.no-write-vec",
    "rule.move-vec",
    "rule.no-move-vec",
    "pattern.wildcard",
    "pattern.single",
    "pattern.range",
    "pattern.bound",
    "pattern.unbound",
    "write.keep",
    "write.lit",
    "write.subst.int",
    "write.subst.fold",
    "fold.add",
    "fold.sub",
    "fold.mul",
    "fold.rem",
    "move.left",
    "move.right",
    "move.stay",
    "transition.goto.explicit",
    "transition.goto.sugar",
    "transition.call",
    "transition.return",
    "transition.stop",
    "transition.halt",
    "transition.stay",
    "continuation.state",
    "continuation.return",
    "continuation.stop",
    "continuation.halt",
    "arg.named.mapped",
    "arg.named.bare",
    "arg.terminator.return",
    "arg.terminator.stop",
    "arg.terminator.halt",
    "map.bidirectional",
    "map.read-only",
    "doc.attention",
    "doc.deprecated",
];

/// A splitmix64 stream, turning one test-fixed root seed into the byte
/// vectors the coverage sweep feeds [`generate_program`]. Deterministic
/// on purpose: a coverage floor that drifts case to case reports a
/// different gap every run and stops being a floor.
struct SplitMix(u64);

impl SplitMix {
    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// One seed shaped like the lossless property's own `1..512` byte
    /// vector, so the sweep measures the SAME distribution that property
    /// runs over rather than a differently-shaped one.
    fn seed(&mut self) -> Vec<u8> {
        let len = 1 + (self.next_u64() % 511) as usize;
        (0..len).map(|_| self.next_u64() as u8).collect()
    }
}

fn stamp_sym(sym: &SymLit, seen: &mut BTreeSet<&'static str>) {
    match sym {
        SymLit::Glyph { value, .. } => {
            seen.insert("sym.glyph");
            // The two escapes whose SOURCE spelling differs from their
            // one-character content (docs/tmt/language.md (alphabets));
            // a shim that reprinted the value instead of keeping the
            // written form would diverge here and nowhere else.
            if value == "'" || value == "\\" {
                seen.insert("sym.glyph.escaped");
            }
        }
        SymLit::Number { written, .. } => {
            seen.insert("sym.number");
            if written.len() > 1 && written.starts_with('0') {
                seen.insert("sym.number.leading-zero");
            }
        }
    }
}

fn stamp_elems(
    elems: &[mtc_turing_machine::parser::AlphabetElem],
    seen: &mut BTreeSet<&'static str>,
) {
    for elem in elems {
        match elem {
            mtc_turing_machine::parser::AlphabetElem::Single(sym) => {
                seen.insert("alphabet.elem.single");
                stamp_sym(sym, seen);
            }
            mtc_turing_machine::parser::AlphabetElem::Range { lo, hi, .. } => {
                seen.insert("alphabet.elem.range");
                stamp_sym(lo, seen);
                stamp_sym(hi, seen);
            }
        }
    }
}

fn stamp_doc(doc: &Option<mtc_turing_machine::parser::Doc>, seen: &mut BTreeSet<&'static str>) {
    let Some(doc) = doc else { return };
    match doc.paragraphs.len() {
        0 => {}
        1 => {
            seen.insert("doc.paragraphs.one");
        }
        _ => {
            seen.insert("doc.paragraphs.many");
        }
    }
    if !doc.attention.is_empty() {
        seen.insert("doc.attention");
    }
    if doc.deprecated.is_some() {
        seen.insert("doc.deprecated");
    }
}

fn stamp_qual(name: &mtc_turing_machine::parser::QualName, seen: &mut BTreeSet<&'static str>) {
    if name.segments.len() > 1 {
        seen.insert("qualname.multi-segment");
    } else {
        seen.insert("qualname.single-segment");
    }
}

fn stamp_args(args: &[mtc_turing_machine::parser::BindingArg], seen: &mut BTreeSet<&'static str>) {
    for arg in args {
        match &arg.value {
            mtc_turing_machine::parser::BindingValue::Named { map, .. } => {
                if let Some(map) = map {
                    seen.insert("arg.named.mapped");
                    for pair in &map.pairs {
                        stamp_sym(&pair.src, seen);
                        stamp_sym(&pair.dst, seen);
                        match pair.arrow {
                            MapArrow::Bidirectional => seen.insert("map.bidirectional"),
                            MapArrow::ReadOnly => seen.insert("map.read-only"),
                        };
                    }
                } else {
                    seen.insert("arg.named.bare");
                }
            }
            mtc_turing_machine::parser::BindingValue::Terminator { kind, .. } => {
                match kind {
                    TermKind::Return => seen.insert("arg.terminator.return"),
                    TermKind::Stop => seen.insert("arg.terminator.stop"),
                    TermKind::Halt => seen.insert("arg.terminator.halt"),
                };
            }
        }
    }
}

fn stamp_fold(node: &mtc_turing_machine::parser::FoldExprNode, seen: &mut BTreeSet<&'static str>) {
    match &node.kind {
        FoldExprKind::Var(_) => {}
        FoldExprKind::Int(_) => {
            seen.insert("write.subst.int");
        }
        FoldExprKind::Bin { op, lhs, rhs } => {
            seen.insert("write.subst.fold");
            match op {
                FoldOp::Add => seen.insert("fold.add"),
                FoldOp::Sub => seen.insert("fold.sub"),
                FoldOp::Mul => seen.insert("fold.mul"),
                FoldOp::Rem => seen.insert("fold.rem"),
            };
            if matches!(lhs.kind, FoldExprKind::Bin { .. })
                || matches!(rhs.kind, FoldExprKind::Bin { .. })
            {
                seen.insert("fold.nested");
            }
            stamp_fold(lhs, seen);
            stamp_fold(rhs, seen);
        }
    }
}

fn stamp_rule(rule: &mtc_turing_machine::parser::Rule, seen: &mut BTreeSet<&'static str>) {
    seen.insert(if rule.debugger {
        "rule.debugger"
    } else {
        "rule.no-debugger"
    });
    for cell in &rule.pattern.cells {
        match &cell.kind {
            PatternCellKind::Wildcard => {
                seen.insert("pattern.wildcard");
            }
            PatternCellKind::Single(sym) => {
                seen.insert("pattern.single");
                stamp_sym(sym, seen);
            }
            PatternCellKind::Range { lo, hi } => {
                seen.insert("pattern.range");
                stamp_sym(lo, seen);
                stamp_sym(hi, seen);
            }
        }
        seen.insert(if cell.binding.is_some() {
            "pattern.bound"
        } else {
            "pattern.unbound"
        });
    }
    match &rule.write {
        Some(vec) => {
            seen.insert("rule.write-vec");
            for cell in &vec.cells {
                match &cell.kind {
                    WriteCellKind::Keep => {
                        seen.insert("write.keep");
                    }
                    WriteCellKind::Lit(sym) => {
                        seen.insert("write.lit");
                        stamp_sym(sym, seen);
                    }
                    WriteCellKind::Subst { expr } => {
                        if matches!(expr.kind, FoldExprKind::Var(_)) {
                            seen.insert("write.subst.passthrough");
                        }
                        stamp_fold(expr, seen);
                    }
                }
            }
        }
        None => {
            seen.insert("rule.no-write-vec");
        }
    }
    match &rule.mov {
        Some(vec) => {
            seen.insert("rule.move-vec");
            for cell in &vec.cells {
                match cell.dir {
                    MoveDir::Left => seen.insert("move.left"),
                    MoveDir::Right => seen.insert("move.right"),
                    MoveDir::Stay => seen.insert("move.stay"),
                };
            }
        }
        None => {
            seen.insert("rule.no-move-vec");
        }
    }
    match &rule.transition {
        Transition::Goto { explicit, .. } => {
            seen.insert(if *explicit {
                "transition.goto.explicit"
            } else {
                "transition.goto.sugar"
            });
        }
        Transition::Call {
            target, args, then, ..
        } => {
            seen.insert("transition.call");
            stamp_qual(target, seen);
            stamp_args(args, seen);
            match then {
                Continuation::State { .. } => seen.insert("continuation.state"),
                Continuation::Return { .. } => seen.insert("continuation.return"),
                Continuation::Stop { .. } => seen.insert("continuation.stop"),
                Continuation::Halt { .. } => seen.insert("continuation.halt"),
            };
        }
        Transition::Return { .. } => {
            seen.insert("transition.return");
        }
        Transition::Stop { .. } => {
            seen.insert("transition.stop");
        }
        Transition::Halt { .. } => {
            seen.insert("transition.halt");
        }
        Transition::Stay { .. } => {
            seen.insert("transition.stay");
        }
    }
}

fn stamp_world(
    states: &[mtc_turing_machine::parser::State],
    grafts: &[mtc_turing_machine::parser::Graft],
    binds: &[mtc_turing_machine::parser::Bind],
    seen: &mut BTreeSet<&'static str>,
) {
    for state in states {
        seen.insert(if state.entry {
            "state.entry"
        } else {
            "state.plain"
        });
        if state.doc.is_some() {
            seen.insert("state.documented");
        }
        if state.rules.is_empty() {
            seen.insert("state.ruleless");
        }
        stamp_doc(&state.doc, seen);
        for rule in &state.rules {
            stamp_rule(rule, seen);
        }
    }
    for graft in grafts {
        seen.insert(if graft.entry {
            "graft.entry"
        } else {
            "graft.plain"
        });
        seen.insert(if graft.as_name.is_some() {
            "graft.named"
        } else {
            "graft.anonymous"
        });
        if graft.doc.is_some() {
            seen.insert("graft.documented");
        }
        seen.insert(if graft.args.is_empty() {
            "graft.args.empty"
        } else {
            "graft.args.non-empty"
        });
        stamp_doc(&graft.doc, seen);
        stamp_qual(&graft.target, seen);
        stamp_args(&graft.args, seen);
    }
    for bind in binds {
        seen.insert("bind");
        if bind.doc.is_some() {
            seen.insert("bind.documented");
        }
        if !bind.args.is_empty() {
            seen.insert("bind.args.non-empty");
        }
        stamp_doc(&bind.doc, seen);
        stamp_qual(&bind.target, seen);
        stamp_args(&bind.args, seen);
    }
}

fn stamp_signature(sig: &mtc_turing_machine::parser::Signature, seen: &mut BTreeSet<&'static str>) {
    for param in &sig.params {
        match &param.kind {
            SigParamKind::State => {
                seen.insert("sig.param.state");
            }
            SigParamKind::Tape {
                volatile,
                writes,
                preserves,
                ..
            } => {
                seen.insert("sig.param.tape");
                seen.insert(if *volatile {
                    "sig.tape.volatile"
                } else {
                    "sig.tape.plain"
                });
                seen.insert(match (writes.is_some(), preserves.is_some()) {
                    (true, true) => "sig.tape.both-clauses",
                    (true, false) => "sig.tape.writes-only",
                    (false, true) => "sig.tape.preserves-only",
                    (false, false) => "sig.tape.no-clause",
                });
                for clause in [writes, preserves].into_iter().flatten() {
                    seen.insert(if clause.elems.is_empty() {
                        "contract.empty"
                    } else {
                        "contract.non-empty"
                    });
                    stamp_elems(&clause.elems, seen);
                }
            }
        }
    }
}

/// Stamp every construct one extracted `Program` contains.
fn stamp_program(program: &Program, seen: &mut BTreeSet<&'static str>) {
    for import in &program.imports {
        seen.insert(if import.alias.is_some() {
            "import.alias"
        } else {
            "import.bare"
        });
        if import.path.len() > 1 {
            seen.insert("import.multi-segment");
        }
        seen.insert(if import.ns.is_empty() {
            "import.file-level"
        } else {
            "import.namespaced"
        });
    }
    for alphabet in &program.alphabets {
        seen.insert(if alphabet.exported {
            "alphabet.exported"
        } else {
            "alphabet.private"
        });
        seen.insert(if alphabet.doc.is_some() {
            "alphabet.documented"
        } else {
            "alphabet.undocumented"
        });
        if !alphabet.ns.is_empty() {
            seen.insert("alphabet.namespaced");
        }
        if alphabet.ns.len() > 1 {
            seen.insert("namespace.nested");
        }
        stamp_doc(&alphabet.doc, seen);
        stamp_elems(&alphabet.elems, seen);
    }
    for routine in &program.routines {
        seen.insert("routine");
        if routine.exported {
            seen.insert("routine.exported");
        }
        if routine.doc.is_some() {
            seen.insert("routine.documented");
        }
        if !routine.ns.is_empty() {
            seen.insert("routine.namespaced");
        }
        if routine.ns.len() > 1 {
            seen.insert("namespace.nested");
        }
        stamp_doc(&routine.doc, seen);
        stamp_signature(&routine.sig, seen);
        stamp_world(&routine.states, &routine.grafts, &routine.binds, seen);
    }
    for graph in &program.graphs {
        seen.insert("graph");
        if graph.exported {
            seen.insert("graph.exported");
        }
        if graph.doc.is_some() {
            seen.insert("graph.documented");
        }
        if !graph.ns.is_empty() {
            seen.insert("graph.namespaced");
        }
        if graph.ns.len() > 1 {
            seen.insert("namespace.nested");
        }
        stamp_doc(&graph.doc, seen);
        stamp_signature(&graph.sig, seen);
        stamp_world(&graph.states, &graph.grafts, &graph.binds, seen);
    }
    if let Some(machine) = &program.machine {
        seen.insert("machine");
        if machine.doc.is_some() {
            seen.insert("machine.documented");
        }
        stamp_doc(&machine.doc, seen);
        for tape in &machine.tapes {
            seen.insert(if tape.volatile {
                "tape.volatile"
            } else {
                "tape.plain"
            });
        }
        stamp_world(&machine.states, &machine.grafts, &machine.binds, seen);
    }
}

/// The construct-coverage oracle, in two strengths.
///
/// **Per program, the strong one**: the labels the generator RECORDED
/// while writing the source and the CHOSEN labels [`stamp_program`] reads
/// back off the extracted `Program` are the same set, compared in both
/// directions. The generator is the independent side here — it decides
/// in TEXT, extraction rebuilds from a tree — so a construct the
/// generator wrote and extraction dropped fails, and so does a construct
/// extraction reports that nobody wrote. [`CHOSEN_CONSTRUCTS`] names the
/// labels this half covers, and its doc comment names the five it cannot
/// and why.
///
/// **Across the sweep, the floor**: every label in
/// [`REQUIRED_CONSTRUCTS`] — the CHOSEN ones and the five derived ones
/// alike — is observed at least once, and nothing outside that list is
/// ever stamped. A floor, not a snapshot: narrowing the generator so a
/// construct stops being emitted fails here rather than silently
/// weakening every property in this file.
///
/// **The derived set is pinned BY VALUE, not by a subset check.** A
/// subset check would let a label leave `CHOSEN_CONSTRUCTS`, or join
/// `REQUIRED_CONSTRUCTS` without joining `CHOSEN_CONSTRUCTS`, and quietly
/// demote it to floor-only: the per-program compare would stop covering
/// it and every test here would stay green. That is the exact failure
/// this whole check exists to prevent, one step removed. The five
/// derived labels are therefore listed as a closed literal below — a new
/// label belongs in `CHOSEN_CONSTRUCTS` unless it is genuinely derived,
/// and making it derived means editing that literal on purpose.
#[test]
fn the_generator_reaches_every_construct_extraction_rebuilds() {
    let required: BTreeSet<&'static str> = REQUIRED_CONSTRUCTS.iter().copied().collect();
    assert_eq!(
        required.len(),
        REQUIRED_CONSTRUCTS.len(),
        "REQUIRED_CONSTRUCTS has a duplicate label"
    );
    let chosen: BTreeSet<&'static str> = CHOSEN_CONSTRUCTS.iter().copied().collect();
    assert_eq!(
        chosen.len(),
        CHOSEN_CONSTRUCTS.len(),
        "CHOSEN_CONSTRUCTS has a duplicate label"
    );
    assert!(
        chosen.is_subset(&required),
        "CHOSEN_CONSTRUCTS names a label REQUIRED_CONSTRUCTS does not: {:?}",
        chosen.difference(&required).collect::<Vec<_>>()
    );
    // Sorted, because both sides are `BTreeSet`s.
    assert_eq!(
        required.difference(&chosen).copied().collect::<Vec<_>>(),
        vec![
            "doc.paragraphs.many",
            "doc.paragraphs.one",
            "fold.nested",
            "sym.number.leading-zero",
            "write.subst.passthrough",
        ],
        "the derived set is closed — a new label belongs in CHOSEN_CONSTRUCTS \
         unless it is derived, and CHOSEN_CONSTRUCTS's own doc says what that means"
    );

    let mut rng = SplitMix(0x7A5C_0DE5_1234_5678);
    let mut seen: BTreeSet<&'static str> = BTreeSet::new();
    for case in 0..600 {
        let (src, emitted) = generate_program(&rng.seed());
        let program = extracted(&src);
        let mut stamped: BTreeSet<&'static str> = BTreeSet::new();
        stamp_program(&program, &mut stamped);

        let unlisted: Vec<&&str> = emitted.0.difference(&chosen).collect();
        assert!(
            unlisted.is_empty(),
            "case {case}: the generator recorded labels CHOSEN_CONSTRUCTS does not list: \
             {unlisted:?}"
        );
        let stamped_chosen: BTreeSet<&'static str> =
            stamped.intersection(&chosen).copied().collect();
        let dropped: Vec<&&str> = emitted.0.difference(&stamped_chosen).collect();
        let invented: Vec<&&str> = stamped_chosen.difference(&emitted.0).collect();
        assert!(
            dropped.is_empty() && invented.is_empty(),
            "case {case}: the generator wrote {dropped:?} and extraction did not rebuild it; \
             extraction reported {invented:?} and the generator never wrote it. On:\n{src}"
        );

        seen.extend(stamped);
    }

    let unreached: Vec<&&str> = required.difference(&seen).collect();
    assert!(
        unreached.is_empty(),
        "the generator never produced: {unreached:?}"
    );
    let unlisted: Vec<&&str> = seen.difference(&required).collect();
    assert!(
        unlisted.is_empty(),
        "the tally stamps labels REQUIRED_CONSTRUCTS does not list: {unlisted:?}"
    );
}
