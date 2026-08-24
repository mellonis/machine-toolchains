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
//! bare-name sugar, `call … then` against every continuation kind
//! including a nested `with map` using both arrows, `return`, `stop`,
//! `halt`, and the omitted-transition "stay" shape); `bind`; qualified
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
//! kind wrong) leaves this property green across the full case count:
//! kind and node-extent correctness are a later plan's properties to add,
//! not a gap in this one. Recorded here so plan 8 inherits this as a
//! known starting boundary rather than rediscovering it.

use mtc_core::syntax::SyntaxNode;
use mtc_turing_machine::parser::parse_green;
use proptest::prelude::*;

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
fn gen_sym(cur: &mut Cursor) -> (String, bool) {
    if cur.chance(1, 8) {
        (
            ESCAPED_GLYPHS[cur.choose(ESCAPED_GLYPHS.len())].to_string(),
            true,
        )
    } else if cur.chance(2, 3) {
        (GLYPHS[cur.choose(GLYPHS.len())].to_string(), true)
    } else {
        (NUMBERS[cur.choose(NUMBERS.len())].to_string(), false)
    }
}

/// A same-kind low/high pair for a range endpoint, matching `lo`'s kind —
/// the parser rejects only a KIND mismatch (`RangeKindMismatch`), never an
/// out-of-order or improbable-looking pair (that check is a later semantic
/// one; see the module doc), so any same-kind `hi` is grammar-valid.
fn gen_range_hi(cur: &mut Cursor, lo_is_glyph: bool) -> String {
    if lo_is_glyph {
        GLYPHS[cur.choose(GLYPHS.len())].to_string()
    } else {
        NUMBERS[cur.choose(NUMBERS.len())].to_string()
    }
}

/// One alphabet-body element: a single symbol or a `lo..hi` range
/// (docs/tmt/language.md (alphabets)) — the same element grammar an
/// alphabet body and a contract clause body share.
fn gen_alphabet_elem(cur: &mut Cursor) -> String {
    let (lo, is_glyph) = gen_sym(cur);
    if cur.chance(1, 3) {
        format!("{lo}..{}", gen_range_hi(cur, is_glyph))
    } else {
        lo
    }
}

/// A comma-separated element list body (no brackets), used by both an
/// `alphabet { … }` body and a `writes`/`preserves { … }` contract clause.
/// `allow_empty` covers the clause body's `{}` (a real, meaningful shape —
/// "written nowhere" — distinct from an absent clause; see
/// docs/tmt/language.md (contract clauses)); a bare `alphabet` body stays
/// non-empty here since an empty one is a later semantic error, not a
/// parse one, and this generator otherwise keeps to realistic shapes
/// (see the module doc's left-out list). `interior` gates one of this
/// generator's four covered interior-comment list positions.
fn gen_elem_list(
    cur: &mut Cursor,
    n: &mut u32,
    allow_empty: bool,
    interior: bool,
    out: &mut String,
) {
    let count = if allow_empty && cur.chance(1, 5) {
        0
    } else {
        1 + cur.choose(4)
    };
    for i in 0..count {
        if i > 0 {
            out.push_str(", ");
        }
        if interior && cur.chance(1, 6) {
            push_comment_break(cur, n, out);
        }
        out.push_str(&gen_alphabet_elem(cur));
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
fn gen_doc_run(cur: &mut Cursor, n: &mut u32, pad: &str, out: &mut String) {
    let docs = cur.choose(3);
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
            out.push_str("! [deprecated] superseded\n");
        } else {
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
fn gen_use(cur: &mut Cursor, n: &mut u32, out: &mut String) {
    out.push_str("use ");
    let paths = 1 + cur.choose(3);
    for i in 0..paths {
        if i > 0 {
            out.push_str(", ");
            if cur.chance(1, 5) {
                push_comment_break(cur, n, out);
            }
        }
        out.push_str(&format!("ns{}::sym{}", cur.choose(3), cur.choose(4)));
        if cur.chance(1, 4) {
            out.push_str(&format!(" as alias{}", uid(n)));
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
fn gen_pattern_cell(cur: &mut Cursor, n: &mut u32) -> (String, Option<(String, bool)>) {
    if cur.chance(1, 6) {
        return ("*".to_string(), None);
    }
    let (lo, is_glyph) = gen_sym(cur);
    let body = if cur.chance(1, 3) {
        format!("{lo}..{}", gen_range_hi(cur, is_glyph))
    } else {
        lo
    };
    if cur.chance(1, 2) {
        let name = format!("v{}", uid(n));
        (format!("{body} as {name}"), Some((name, is_glyph)))
    } else {
        (body, None)
    }
}

/// A bracketed pattern vector of `arity` cells, plus the bindings it
/// introduced. `interior` gates this generator's pattern-vector interior-
/// comment coverage (one of its four covered list positions).
fn gen_pattern(
    cur: &mut Cursor,
    n: &mut u32,
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
        let (cell, binding) = gen_pattern_cell(cur, n);
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
fn gen_fold_atom(cur: &mut Cursor, numeric: &[String], depth: u32) -> String {
    if depth > 0 && cur.chance(1, 5) {
        return format!("({})", gen_fold_expr(cur, numeric, depth - 1));
    }
    if !numeric.is_empty() && cur.chance(1, 2) {
        numeric[cur.choose(numeric.len())].clone()
    } else {
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
fn gen_fold_mul(cur: &mut Cursor, numeric: &[String], depth: u32) -> String {
    let mut s = gen_fold_atom(cur, numeric, depth);
    for _ in 0..3 {
        if !cur.chance(1, 3) {
            break;
        }
        let op = if cur.chance(1, 2) { '*' } else { '%' };
        s = format!("{s}{op}{}", gen_fold_atom(cur, numeric, depth));
    }
    s
}

/// `expr := mul (('+' | '-') mul)*` (docs/tmt/language.md (substitution)) —
/// capped the same way and for the same reason as [`gen_fold_mul`].
fn gen_fold_expr(cur: &mut Cursor, numeric: &[String], depth: u32) -> String {
    let mut s = gen_fold_mul(cur, numeric, depth);
    for _ in 0..3 {
        if !cur.chance(1, 3) {
            break;
        }
        let op = if cur.chance(1, 2) { '+' } else { '-' };
        s = format!("{s}{op}{}", gen_fold_mul(cur, numeric, depth));
    }
    s
}

/// One write cell: `-` (keep), a literal, or a substitution `{…}`
/// (docs/tmt/language.md (write and move vectors)). A substitution is
/// either the passthrough of one bound name (any binding kind — always
/// parse-safe, `check_char_arithmetic` exempts a bare `Var`) or an
/// arithmetic fold restricted to `bindings`' NUMERIC names plus integer
/// literals, since a fold over a glyph binding is `CharArithmetic`.
fn gen_write_cell(cur: &mut Cursor, bindings: &[(String, bool)]) -> String {
    match cur.choose(4) {
        0 => "-".to_string(),
        1 => gen_sym(cur).0,
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
            format!("{{{}}}", gen_fold_expr(cur, &numeric, 1))
        }
    }
}

/// A bracketed write vector of `arity` cells, or `None` (the whole vector
/// omitted keeps every cell — docs/tmt/language.md (write and move
/// vectors)).
fn gen_write_vec(cur: &mut Cursor, arity: usize, bindings: &[(String, bool)]) -> Option<String> {
    if cur.chance(1, 4) {
        return None;
    }
    let mut out = String::from("write [");
    for i in 0..arity {
        if i > 0 {
            out.push_str(", ");
        }
        out.push_str(&gen_write_cell(cur, bindings));
    }
    out.push(']');
    Some(out)
}

/// A bracketed move vector of `arity` cells (`<`/`>`/`.`), or `None`.
fn gen_move_vec(cur: &mut Cursor, arity: usize) -> Option<String> {
    if cur.chance(1, 4) {
        return None;
    }
    let mut out = String::from("move [");
    for i in 0..arity {
        if i > 0 {
            out.push_str(", ");
        }
        out.push(["<", ">", "."][cur.choose(3)].chars().next().unwrap());
    }
    out.push(']');
    Some(out)
}

/// A `call`/`graft`/`bind` TARGET — a qualified name
/// (`IDENT (:: IDENT)*`, docs/tmt/language.md (namespaces, visibility, and
/// imports)) parsed by the one shared `qual_name` function all three
/// route through. Usually one bare segment, occasionally two or three —
/// `std::binaryNumbers::plusOne` is the documented spelling — and rarely
/// carrying a same-line block comment right after a `::`
/// (`call a:: /* n */ b(…)` parses and round-trips: `qual_name` never
/// calls `interior_comments`, so a comment written here is just ordinary
/// trivia between two tokens, picked up by whatever list drain runs next
/// rather than claimed as an "interior" slot the parser tracks — unlike
/// `push_comment_break`'s four list positions, nothing here requires a
/// line break first, so this is the one place in the generator that
/// deliberately keeps a block comment on the same line as what follows
/// it; see the module doc for why the other four stay break-only).
fn gen_qual_target(cur: &mut Cursor, n: &mut u32, prefix: &str) -> String {
    let mut out = format!("{prefix}{}", cur.choose(3));
    let extra_segments = cur.choose(3);
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
fn gen_sym_map(cur: &mut Cursor) -> String {
    let mut out = String::from("with map { ");
    let n = 1 + cur.choose(3);
    for i in 0..n {
        if i > 0 {
            out.push_str(", ");
        }
        let arrow = if cur.chance(1, 2) { "->" } else { "=>" };
        out.push_str(&format!("{} {arrow} {}", gen_sym(cur).0, gen_sym(cur).0));
    }
    out.push_str(" }");
    out
}

/// One `name = value` binding argument (docs/tmt/language.md (`call`,
/// `graft`, `bind`)): a bare name (optionally carrying a `with map`) or a
/// terminator (`return`/`stop`/`halt`). Neither the parameter name nor the
/// target need resolve to anything real at parse time (see the module
/// doc), so both are drawn from small synthetic pools.
fn gen_binding_arg(cur: &mut Cursor, n: &mut u32) -> String {
    let param = format!("p{}", cur.choose(4));
    match cur.choose(5) {
        0 => format!("{param} = return"),
        1 => format!("{param} = stop"),
        2 => format!("{param} = halt"),
        _ => {
            let target = format!("tgt{}", uid(n));
            if cur.chance(1, 3) {
                format!("{param} = {target} {}", gen_sym_map(cur))
            } else {
                format!("{param} = {target}")
            }
        }
    }
}

/// A `(args)` binding list, 0-3 arguments (docs/tmt/language.md (reuse) —
/// every one of `call`/`graft`/`bind` shares this list shape). `interior`
/// gates this generator's binding-argument-list interior-comment coverage
/// (the fourth of its four covered list positions).
fn gen_binding_args(cur: &mut Cursor, n: &mut u32, interior: bool) -> String {
    let mut out = String::from("(");
    let count = cur.choose(4);
    for i in 0..count {
        if i > 0 {
            out.push_str(", ");
        }
        if interior && cur.chance(1, 6) {
            push_comment_break(cur, n, &mut out);
        }
        out.push_str(&gen_binding_arg(cur, n));
    }
    if interior && count > 0 && cur.chance(1, 8) {
        out.push(' ');
        push_comment_break(cur, n, &mut out);
    }
    out.push(')');
    out
}

/// A `call TARGET(args) then CONT` continuation
/// (docs/tmt/language.md (transitions)): a state name, or one of
/// `return`/`stop`/`halt` — the same four shapes a binding-argument value
/// takes, but never `with map` (a continuation is not a tape/state
/// binding).
fn gen_continuation(cur: &mut Cursor, states: &[String]) -> String {
    match cur.choose(4) {
        0 => "return".to_string(),
        1 => "stop".to_string(),
        2 => "halt".to_string(),
        _ if !states.is_empty() => states[cur.choose(states.len())].clone(),
        _ => "elsewhere".to_string(),
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
        return String::new(); // omitted — "stay in the current state"
    }
    match shapes[choice] {
        0 => format!("goto {}", gen_target(cur, n, states)),
        1 => gen_target(cur, n, states), // bare-name sugar
        2 => "stop".to_string(),
        3 => "halt".to_string(),
        4 => {
            let callee = gen_qual_target(cur, n, "callee");
            let interior = cur.chance(1, 4);
            let args = gen_binding_args(cur, n, interior);
            let then = gen_continuation(cur, states);
            format!("call {callee}{args} then {then}")
        }
        _ => "return".to_string(),
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
/// comments above it and an occasional trailing comment.
#[allow(clippy::too_many_arguments)]
fn gen_rule(
    cur: &mut Cursor,
    n: &mut u32,
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
    let (pattern, bindings) = gen_pattern(cur, n, arity, pattern_interior);
    out.push_str(&pattern);
    out.push_str(" -> ");
    let debugger = cur.chance(1, 8);
    if debugger {
        out.push_str("debugger ");
    }
    let write = gen_write_vec(cur, arity, &bindings);
    if let Some(w) = &write {
        out.push_str(w);
        out.push(' ');
    }
    let mov = gen_move_vec(cur, arity);
    if let Some(m) = &mov {
        out.push_str(m);
        out.push(' ');
    }
    let has_action = debugger || write.is_some() || mov.is_some();
    let transition = gen_transition(cur, n, states, in_routine, allow_call, has_action);
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
        gen_doc_run(cur, n, pad, out);
    }
    out.push_str(pad);
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
    for i in 0..rules {
        if i > 0 && cur.chance(1, 5) {
            out.push('\n');
        }
        gen_rule(
            cur, n, &inner_pad, arity, states, in_routine, allow_call, out,
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
fn gen_graft(cur: &mut Cursor, n: &mut u32, pad: &str, entry: bool, out: &mut String) {
    if cur.chance(1, 4) {
        gen_doc_run(cur, n, pad, out);
    }
    out.push_str(pad);
    if entry {
        out.push_str("entry ");
    }
    let target = gen_qual_target(cur, n, "graph");
    let interior = cur.chance(1, 4);
    out.push_str(&format!(
        "graft {target}{}",
        gen_binding_args(cur, n, interior)
    ));
    let needs_name = !entry || cur.chance(2, 3);
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
fn gen_bind(cur: &mut Cursor, n: &mut u32, pad: &str, out: &mut String) {
    if cur.chance(1, 4) {
        gen_doc_run(cur, n, pad, out);
    }
    out.push_str(pad);
    let target = gen_qual_target(cur, n, "callee");
    let interior = cur.chance(1, 4);
    out.push_str(&format!(
        "bind {target}{} as h{};",
        gen_binding_args(cur, n, interior),
        uid(n)
    ));
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
fn gen_tape(cur: &mut Cursor, n: &mut u32, pad: &str, out: &mut String) {
    out.push_str(pad);
    if cur.chance(1, 3) {
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
            gen_graft(cur, n, pad, true, out);
            continue;
        }
        match cur.choose(4) {
            0 => {
                let entry = want_entry;
                entry_placed |= entry;
                let name = format!("s{}", uid(n));
                gen_state(
                    cur, n, pad, &name, entry, arity, &states, in_routine, allow_call, out,
                );
                states.push(name);
            }
            1 => gen_graft(cur, n, pad, false, out),
            _ => gen_bind(cur, n, pad, out),
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
            cur, n, pad, &name, true, arity, &states, in_routine, allow_call, out,
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
        if cur.chance(1, 4) {
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
        if want_writes {
            out.push_str(" writes { ");
            gen_elem_list(cur, n, true, false, out);
            out.push_str(" }");
        }
        if want_preserves {
            out.push_str(" preserves { ");
            gen_elem_list(cur, n, true, false, out);
            out.push_str(" }");
        }
    }
    if is_graph {
        let exit_count = 1 + cur.choose(2);
        for _ in 0..exit_count {
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
fn gen_reuse(cur: &mut Cursor, n: &mut u32, pad: &str, is_graph: bool, out: &mut String) {
    if cur.chance(1, 3) {
        gen_doc_run(cur, n, pad, out);
    }
    out.push_str(pad);
    if cur.chance(1, 3) {
        out.push_str("export ");
    }
    let kw = if is_graph { "graph" } else { "routine" };
    let name = format!("{}{}", if is_graph { "g" } else { "r" }, uid(n));
    out.push_str(&format!("{kw} {name}"));
    let (arity, _exits) = gen_signature(cur, n, is_graph, out);
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
fn gen_machine(cur: &mut Cursor, n: &mut u32, out: &mut String) {
    if cur.chance(1, 3) {
        gen_doc_run(cur, n, "", out);
    }
    out.push_str("machine {");
    if cur.chance(1, 4) {
        out.push(' ');
        out.push_str(&gen_comment(cur, n));
    }
    out.push('\n');
    let arity = 1 + cur.choose(3);
    for _ in 0..arity {
        gen_tape(cur, n, "  ", out);
    }
    out.push('\n');
    let prefer_entry_graft = cur.chance(1, 3);
    // `return` is a routine-only shape (see the module doc); `call` IS
    // legitimate directly in a machine body (docs/tmt/language.md
    // (worlds) shows exactly this: `call touch(t = main) then done;`).
    gen_world_items(cur, n, "  ", arity, false, true, prefer_entry_graft, out);
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
fn gen_namespace(
    cur: &mut Cursor,
    n: &mut u32,
    pad: &str,
    depth: u32,
    ns_names: &mut Vec<String>,
    out: &mut String,
) {
    if cur.chance(1, 3) {
        gen_doc_run(cur, n, pad, out);
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
        gen_top_item(cur, n, &inner_pad, depth, ns_names, out);
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
fn gen_top_item(
    cur: &mut Cursor,
    n: &mut u32,
    pad: &str,
    depth: u32,
    ns_names: &mut Vec<String>,
    out: &mut String,
) {
    let choices = if depth > 0 { 5 } else { 4 };
    match cur.choose(choices) {
        0 => {
            out.push_str(pad);
            gen_use(cur, n, out);
        }
        1 => {
            if cur.chance(1, 3) {
                gen_doc_run(cur, n, pad, out);
            }
            out.push_str(pad);
            if cur.chance(1, 3) {
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
            gen_elem_list(cur, n, false, elems_interior, out);
            out.push_str(" }");
            if cur.chance(1, 6) {
                out.push(' ');
                out.push_str(&gen_comment(cur, n));
            }
            out.push('\n');
        }
        2 => gen_reuse(cur, n, pad, false, out),
        3 => gen_reuse(cur, n, pad, true, out),
        _ => gen_namespace(cur, n, pad, depth - 1, ns_names, out),
    }
}

/// A whole grammar-valid `.tmc` program: optional file-leading comments,
/// then 2-5 top-level items with the program's one `machine` block
/// spliced in at a random position among them, separated by the author's
/// own blank lines.
fn generate_program(seed: &[u8]) -> String {
    let mut cur = Cursor::new(seed);
    let mut n = 0u32;
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
            gen_machine(&mut cur, &mut n, &mut out);
        } else {
            // depth = 2: a top-level namespace (depth 2 -> 1) may itself
            // contain one nested namespace (depth 1 -> 0), matching
            // "Namespace nesting stops at two levels" below. `depth = 1`
            // here was a bug — it let `gen_top_item` pick the namespace
            // arm at the top level, but that namespace's own body was
            // then generated at depth 0, where the arm is unreachable, so
            // a namespace could never actually contain another one.
            gen_top_item(&mut cur, &mut n, "", 2, &mut ns_names, &mut out);
        }
    }
    out
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(2000))]

    /// The lossless law over generated programs: the green tree's own
    /// text reconstructs the source, byte for byte
    /// (docs/core.md (syntax trees)).
    #[test]
    fn generated_programs_round_trip(seed in prop::collection::vec(any::<u8>(), 1..512)) {
        let src = generate_program(&seed);
        let tree = parse_green(&src)
            .unwrap_or_else(|e| panic!("generator emitted an invalid program: {e:?}\n{src}"));
        let root = SyntaxNode::new_root(tree);
        prop_assert_eq!(root.text(), src);
    }
}
