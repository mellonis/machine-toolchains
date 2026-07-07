//! `pmt fmt` property tests
//! (`docs/superpowers/specs/2026-07-07-pmc-fmt-design.md`, "Contracts").
//! Complements `tests/fmt_programs.rs`'s hand-picked/corpus checks with a
//! generator of GRAMMAR-VALID `.pmc` programs (docs/language.md), asserting
//! two properties the printer must hold over every one of them:
//!
//! 1. **Idempotence**: `format(format(x)?)? == format(x)?`.
//! 2. **Token equivalence**: lexing `x` and `format(x)?` `WithoutComments`
//!    yields identical `TokenKind` sequences — fmt changes no tokens, only
//!    layout.
//!
//! [`generate_program`] builds source text deterministically from a byte
//! seed via a small [`Cursor`] (cycling index into the seed, never
//! panicking on a short slice) rather than a composed tree of `proptest`
//! strategies — the grammar's positional constraints (`check`/`halt` only
//! last in a comma group, `goto` never in one at all, a non-last group
//! member takes no successor, per-function label uniqueness) are simpler
//! to enforce procedurally than to encode as combinators, and the
//! generator's job is to make EVERY output valid by construction, not to
//! explore the space of invalid ones (parser_parity.rs already exercises
//! rejection paths). No namespaces, imports, or comments — the corpus in
//! `fmt_programs.rs` already covers those; this generator's scope is the
//! function-body grammar named in the brief: builtins with/without a
//! successor, `@calls`, `check`, `goto`, `halt`, `debugger`, labels
//! (including stacked), and comma groups.
//!
//! Call targets (`@calleeN()`) and `goto`/`check`/successor label numbers
//! are never required to resolve to anything real: `parse_cst` (what
//! `format` runs) never checks that — label uniqueness and dangling-label
//! are its only label-shaped diagnostics, both satisfied by construction
//! here (`UndefinedLabel` is a much later `ir::lower` semantic check this
//! test never reaches, since it only calls `format`, not `compile`).

use mtc_post_machine::format;
use mtc_post_machine::lexer::{LexMode, TokenKind, lex_with};
use proptest::prelude::*;

/// A deterministic cursor over a byte seed, used to make grammar-directed
/// choices. Cycles through `bytes` forever (`bytes` is never empty — the
/// strategy below always supplies at least one) so the generator never
/// has to handle running out of randomness.
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

    /// A choice in `0..n` (`n > 0`).
    fn choose(&mut self, n: usize) -> usize {
        (self.next_u8() as usize) % n
    }

    /// `true` with probability `num / den`.
    fn chance(&mut self, num: usize, den: usize) -> bool {
        self.choose(den) < num
    }

    /// A small positive label/successor target — never required to
    /// resolve to a real label (see module doc).
    fn small_number(&mut self) -> u32 {
        1 + self.choose(50) as u32
    }
}

/// One `check` arm (docs/language.md): a label number or `!`, never
/// fall-through.
fn gen_check_arm(cur: &mut Cursor) -> String {
    if cur.chance(1, 2) {
        cur.small_number().to_string()
    } else {
        "!".to_string()
    }
}

/// One tape builtin (`left`/`right`/`mark`/`unmark`). `allow_succ` is
/// false for every non-last comma-group member (docs/language.md: "only
/// the last command in a comma group may take a successor") — those
/// always render bare (no parens at all; empty `()` is a dedicated
/// grammar-0.2 syntax error, `EmptyBuiltinParens`).
fn gen_builtin(cur: &mut Cursor, allow_succ: bool) -> String {
    let name = ["left", "right", "mark", "unmark"][cur.choose(4)];
    if !allow_succ {
        return name.to_string();
    }
    match cur.choose(3) {
        0 => name.to_string(),
        1 => format!("{name}({})", cur.small_number()),
        _ => format!("{name}(!)"),
    }
}

/// A user call. Call parens are mandatory but their contents follow the
/// same successor shape as a builtin's; the callee name need not resolve
/// (see module doc) so a small fixed pool keeps the generator simple.
fn gen_call(cur: &mut Cursor, allow_succ: bool) -> String {
    let callee = format!("callee{}", cur.choose(3));
    if !allow_succ {
        return format!("@{callee}()");
    }
    match cur.choose(3) {
        0 => format!("@{callee}()"),
        1 => format!("@{callee}({})", cur.small_number()),
        _ => format!("@{callee}(!)"),
    }
}

/// One comma-group item. `is_last` gates `check`/`halt`/a successor-
/// bearing builtin-or-call (docs/language.md, "the statement table's last
/// row"); `is_sole` (only true when the group has exactly one member)
/// additionally allows `goto`, which the grammar forbids in a comma group
/// entirely, at ANY position, even the last.
fn gen_item(cur: &mut Cursor, is_last: bool, is_sole: bool) -> String {
    if is_last {
        let choices = if is_sole { 6 } else { 5 };
        match cur.choose(choices) {
            0 => gen_builtin(cur, true),
            1 => gen_call(cur, true),
            2 => format!("check({}, {})", gen_check_arm(cur), gen_check_arm(cur)),
            3 => "halt".to_string(),
            4 => "debugger".to_string(),
            _ => format!("goto {}", cur.small_number()),
        }
    } else {
        match cur.choose(3) {
            0 => gen_builtin(cur, false),
            1 => gen_call(cur, false),
            _ => "debugger".to_string(),
        }
    }
}

/// One `;`-terminated statement: 0-2 stacked labels (each numerically
/// unique within the function via `next_label`, docs/language.md's
/// per-function `DuplicateLabel` check) followed by a 1-3 item comma
/// group.
fn gen_statement(cur: &mut Cursor, next_label: &mut u32) -> String {
    let mut out = String::new();
    for _ in 0..cur.choose(3) {
        out.push_str(&format!("{}: ", *next_label));
        *next_label += 1;
    }
    let group_size = 1 + cur.choose(3);
    let items: Vec<String> = (0..group_size)
        .map(|gi| gen_item(cur, gi == group_size - 1, group_size == 1))
        .collect();
    out.push_str(&items.join(", "));
    out.push_str("; ");
    out
}

/// One function: a unique, non-reserved name, 1-6 statements.
fn gen_function(cur: &mut Cursor, idx: usize) -> String {
    let mut next_label: u32 = 1;
    let mut body = String::new();
    for _ in 0..1 + cur.choose(6) {
        body.push_str(&gen_statement(cur, &mut next_label));
    }
    format!("pf{idx}() {{ {body} }}")
}

/// A whole grammar-valid `.pmc` program: 1-3 top-level functions, no
/// namespaces/imports/comments (module doc — out of this generator's
/// scope, covered by the corpus instead).
fn generate_program(seed: &[u8]) -> String {
    let mut cur = Cursor::new(seed);
    let num_fns = 1 + cur.choose(3);
    (0..num_fns)
        .map(|i| gen_function(&mut cur, i))
        .collect::<Vec<_>>()
        .join(" ")
}

/// `TokenKind` sequence, comments stripped — the same view `compiler.rs`
/// feeds the parser, and what §B's "token equivalence" property compares.
fn kinds(src: &str) -> Vec<TokenKind> {
    lex_with(src, LexMode::WithoutComments)
        .expect("lexes")
        .into_iter()
        .map(|t| t.kind)
        .collect()
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(200))]

    #[test]
    fn generated_programs_are_idempotent_and_token_preserving(
        seed in proptest::collection::vec(any::<u8>(), 64..512),
    ) {
        let src = generate_program(&seed);

        // The generator is built to produce only grammar-valid `.pmc`
        // (see module doc); a parse failure here is a generator defect,
        // not something to assert on — filtered per the brief rather
        // than failing the property.
        let parsed = format(&src);
        prop_assume!(parsed.is_ok(), "generator produced unparsable pmc: {:?}", src);
        let once = parsed.expect("checked by prop_assume above");

        let twice = format(&once).expect("fmt's own output must always re-parse");
        prop_assert_eq!(&twice, &once, "not idempotent for generated source:\n{}", src);

        prop_assert_eq!(
            kinds(&src),
            kinds(&once),
            "token sequence changed for generated source:\n{}",
            src
        );
    }
}
