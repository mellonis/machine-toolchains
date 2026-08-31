//! `pmt fmt` property tests. Complements `tests/fmt_programs.rs`'s
//! hand-picked/corpus checks with a generator of GRAMMAR-VALID `.pmc`
//! programs (docs/pmt/language.md), asserting five properties the printer
//! must hold over every one of them:
//!
//! 1. **Idempotence**: `format(format(x)?)? == format(x)?`.
//! 2. **Token equivalence**: lexing `x` and `format(x)?` `WithoutComments`
//!    yields identical `TokenKind` sequences — fmt changes no tokens, only
//!    layout.
//! 3. **One command column per body**: every command in one function body
//!    starts at that body's shared command column, a split comma group's
//!    continuation lines included (docs/pmt/fmt.md (label and command
//!    alignment), docs/pmt/fmt.md (comma groups)).
//! 4. **Inter-label breaks do not reach the output**: one program written
//!    with its stacked labels on one line and the same program written with
//!    them on separate lines format to the same text — only the break
//!    before the COMMAND is the author's to keep
//!    (docs/pmt/fmt.md (own-line labels)).
//! 5. **Comment preservation**: lexing `x` and `format(x)?` `WithComments`
//!    yields the same comments, in the same order, texts intact up to
//!    per-line trailing-whitespace trim — fmt relocates, never drops
//!    (docs/pmt/fmt.md (comments)).
//!
//! None of §3-§5 is implied by §1-§2, and none implies another. A layout
//! bug can be perfectly idempotent and touch no token:
//! a mis-measured own-line-label break moves a whole body to a narrower
//! command column, which the body still agrees with itself about — that one
//! is only visible as a difference between two spellings of the same
//! program (§4). A continuation line indented to the wrong place is visible
//! within one body but identical across spellings (§3). The unbounded
//! double-print of an open-brace comment that the whole-plan review found
//! is invisible to both and breaks idempotence (§1). And a printer that
//! silently DELETES a comment passes §1-§4 outright — §2 lexes
//! `WithoutComments`, so a dropped comment changes no token it compares —
//! which is how the green printer shipped deleting every header-interior
//! comment until §5 existed (see `comment_texts`).
//!
//! [`generate_program`] builds source text deterministically from a byte
//! seed via a small [`Cursor`] (cycling index into the seed, never
//! panicking on a short slice) rather than a composed tree of `proptest`
//! strategies — the grammar's positional constraints (`check`/`halt` only
//! last in a comma group, `goto` never in one at all, a non-last group
//! member takes no successor, per-function label uniqueness, a `?`/`!` run
//! only ever directly above a function declaration) are simpler to enforce
//! procedurally than to encode as combinators, and the generator's job is
//! to make EVERY output valid by construction, not to explore the space of
//! invalid ones (parser_parity.rs already exercises rejection paths).
//!
//! **Scope: everything that decides layout.** The function-body grammar —
//! builtins with/without a successor, `@calls`, `check`, `goto`, `halt`,
//! `debugger`, comma groups — plus the two spaces where layout bugs
//! actually hide: comments in every position that has its own printer path
//! (file-leading, own-line, trailing, after an opening `{`, dangling before
//! a closing `}`, inside a function's or namespace's header — the
//! relocated-into-the-body path — interleaved into a `use` list and into a
//! `?`/`!` run) and
//! label shapes (inline, stacked, stacked across lines, own-line, and
//! widths wide enough to move the command column), wrapped in the
//! declaration forms that nest them (`export`, `volatile`, `namespace`,
//! `use`). Generated source is therefore LINE-ORIENTED: a `//` comment
//! consumes the rest of its line and a `?`/`!` line is recognised by being
//! the first non-whitespace on its own line, so neither is expressible in
//! the single-line-per-function shape this generator emitted before
//! comments entered its scope.
//!
//! Two deliberate omissions, each a shape whose printer path is covered by
//! `fmt_programs.rs`'s real-program corpus instead, and each excluded for a
//! reason a future widening should re-read before undoing it:
//!
//! * **A comment between a statement's labels and its command.** Formatting
//!   that shape is not idempotent — the printer draws the comment up onto
//!   the label line, which pushes the command down a line, and the second
//!   pass then reads that as an author-written own-line-label break. It
//!   converges after one further pass and is long-standing behaviour, not a
//!   regression of this suite's own properties.
//! * **Nested function declarations inside a body**, which would make
//!   [`command_columns_agree`]'s statement-start walk need a full nested
//!   declaration parser to tell `step() {` from a command.
//!
//! Call targets (`@calleeN()`) and `goto`/`check`/successor label numbers
//! are never required to resolve to anything real: the parser `format` runs
//! never checks that — label uniqueness and dangling-label are its only
//! label-shaped diagnostics, both satisfied by construction here
//! (`UndefinedLabel` is a much later `ir::lower` semantic check this test
//! never reaches, since it only calls `format`, not `compile`).

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

/// Knobs the seed does not decide, so one seed can be rendered twice in
/// two spellings that must format identically.
///
/// Every choice point still consumes the same cursor bytes whichever way
/// a knob is set, so two renderings of one seed differ ONLY in the text
/// the knob controls — the programs stay the same program.
#[derive(Clone, Copy)]
struct Opts {
    /// `None` lets the seed decide each gap between two stacked labels;
    /// `Some(true)` breaks every one across lines, `Some(false)` keeps
    /// every one on a single line.
    stack_labels: Option<bool>,
    /// Whether a statement may carry a trailing comment. Off for the
    /// label-spelling pair: the aligned-or-ragged verdict for a run of
    /// trailing comments is read off the author's own source COLUMNS
    /// (docs/pmt/fmt.md (comments)), which a label break moves — the one
    /// way two spellings of one program may legitimately format
    /// differently.
    trailing_comments: bool,
}

impl Default for Opts {
    fn default() -> Self {
        Opts {
            stack_labels: None,
            trailing_comments: true,
        }
    }
}

/// Per-function label supply. Numbers only ever increase, so uniqueness
/// within the function (docs/pmt/language.md's per-function
/// `DuplicateLabel` check) holds by construction; the occasional large
/// jump is what makes label WIDTHS vary, and the widest inline label is
/// exactly what sets the body's command column
/// (docs/pmt/fmt.md (label and command alignment)). A body whose labels
/// were all one digit could never tell a right command column from a
/// wrong one, since every candidate rounds to the same multiple of 4.
struct Labels {
    next: u32,
}

impl Labels {
    fn new() -> Self {
        Labels { next: 1 }
    }

    fn take(&mut self, cur: &mut Cursor) -> u32 {
        let n = self.next;
        self.next += match cur.choose(4) {
            0 => 1 + cur.choose(900) as u32,
            1 => 1 + cur.choose(90) as u32,
            _ => 1,
        };
        n
    }
}

/// One comment, in either lexical form. The cursor position it stamps in
/// only keeps distinct comments distinguishable when a failure is printed.
fn gen_comment(cur: &mut Cursor) -> String {
    let n = cur.pos;
    if cur.chance(3, 4) {
        format!("// note {n}")
    } else {
        format!("/* note {n} */")
    }
}

/// One `check` arm (docs/pmt/language.md): a label number or `!`, never
/// fall-through.
fn gen_check_arm(cur: &mut Cursor) -> String {
    if cur.chance(1, 2) {
        cur.small_number().to_string()
    } else {
        "!".to_string()
    }
}

/// One tape builtin (`left`/`right`/`mark`/`unmark`). `allow_succ` is
/// false for every non-last comma-group member (docs/pmt/language.md: "only
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
/// bearing builtin-or-call (docs/pmt/language.md, "the statement table's last
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

/// One `;`-terminated statement, appended to `out` as whole lines at
/// `pad`: any own-line comments the author put above it, then 0-3 stacked
/// labels, then a 1-3 item comma group.
///
/// Three independent layout choices are made here, and each one is a
/// distinct printer path: whether consecutive labels share a line or
/// stack across lines; whether the command follows the last label or
/// starts on its own line (docs/pmt/fmt.md (own-line labels)); and whether
/// the comma group is written on one line or split across several
/// (docs/pmt/fmt.md (comma groups)). No comment is ever emitted BETWEEN the
/// labels and the command — see the module doc for why that one position
/// is out of scope.
fn gen_statement(cur: &mut Cursor, labels: &mut Labels, pad: &str, opts: Opts, out: &mut String) {
    for _ in 0..cur.choose(3) {
        out.push_str(pad);
        out.push_str(&gen_comment(cur));
        out.push('\n');
    }

    out.push_str(pad);
    let label_count = cur.choose(4);
    for i in 0..label_count {
        if i > 0 {
            // Stacked labels either share their line or break across
            // lines. Neither spelling is preserved: only the break before
            // the COMMAND is, so both must format identically.
            let seeded = cur.chance(1, 3);
            if opts.stack_labels.unwrap_or(seeded) {
                out.push('\n');
                out.push_str(pad);
            } else {
                out.push(' ');
            }
        }
        out.push_str(&format!("{}:", labels.take(cur)));
    }
    if label_count > 0 {
        if cur.chance(1, 3) {
            out.push('\n');
            out.push_str(pad);
            out.push_str("    ");
        } else {
            out.push(' ');
        }
    }

    let group_size = 1 + cur.choose(3);
    for gi in 0..group_size {
        if gi > 0 {
            out.push(',');
            if cur.chance(1, 3) {
                out.push('\n');
                out.push_str(pad);
                out.push_str("    ");
            } else {
                out.push(' ');
            }
        }
        out.push_str(&gen_item(cur, gi == group_size - 1, group_size == 1));
    }
    out.push(';');
    if cur.chance(1, 4) && opts.trailing_comments {
        out.push_str(" // trailing ");
        out.push_str(&cur.pos.to_string());
    }
    out.push('\n');
}

/// A `?`/`!` run above a function declaration: at most two contiguous
/// blocks in a fixed order, `?` then `!` (docs/pmt/language.md (doc lines)).
/// Ordinary comments and blank lines may sit inside the run and between it
/// and the declaration without breaking the attachment, so both appear
/// here — that interleaving is its own printer path.
fn gen_doc_run(cur: &mut Cursor, pad: &str, out: &mut String) {
    let docs = cur.choose(3);
    let attentions = if docs == 0 { 1 } else { cur.choose(2) };
    for i in 0..docs {
        out.push_str(pad);
        // An empty `?` line is a real lexed shape — a paragraph break.
        if i > 0 && cur.chance(1, 5) {
            out.push_str("?\n");
        } else {
            out.push_str(&format!("? doc line {i}\n"));
        }
        if cur.chance(1, 6) {
            out.push_str(pad);
            out.push_str(&gen_comment(cur));
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
        out.push('\n');
    }
    if cur.chance(1, 4) {
        out.push_str(pad);
        out.push_str(&gen_comment(cur));
        out.push('\n');
    }
}

/// One function at `pad`: an optional doc/attention run, an optional
/// `volatile`/`export` header prefix, an optional comment riding the
/// opening `{`, 1-6 statements, an optional comment dangling before the
/// closing `}`, and an optional comment trailing it. `name` is unique
/// across the program.
///
/// `volatile_ok` is true only for a top-level `main`
/// (docs/pmt/language.md (volatile programs)) — anywhere else the keyword
/// is a `volatile-not-on-main` error, so the generator would stop being
/// valid by construction. The two keywords have one fixed order,
/// `volatile export` (docs/pmt/fmt.md (spacing)); the other order lexes
/// `volatile` as the function's own name and is a `reserved-name` error.
fn gen_function(
    cur: &mut Cursor,
    name: &str,
    pad: &str,
    volatile_ok: bool,
    opts: Opts,
    out: &mut String,
) {
    if cur.chance(1, 3) {
        gen_doc_run(cur, pad, out);
    }
    out.push_str(pad);
    if volatile_ok && cur.chance(1, 2) {
        out.push_str("volatile ");
    }
    if cur.chance(1, 4) {
        out.push_str("export ");
    }
    out.push_str(name);
    // A header-interior comment (between the name and its `()`): block
    // form only, since a `//` here would consume the rest of the header.
    // The shape whose deletion the comment-preservation property exists
    // to catch — fmt relocates it into the body
    // (docs/pmt/fmt.md (comments)).
    if cur.chance(1, 5) {
        let n = cur.pos;
        out.push_str(&format!(" /* note {n} */"));
    }
    out.push_str("() {");
    if cur.chance(1, 3) {
        out.push(' ');
        out.push_str(&gen_comment(cur));
    }
    out.push('\n');

    // A body with no statements at all is exactly the shape the
    // open-brace comment double-printed in, so it must stay reachable.
    let stmt_pad = format!("{pad}    ");
    let mut labels = Labels::new();
    let stmts = if cur.chance(1, 8) {
        0
    } else {
        1 + cur.choose(6)
    };
    for i in 0..stmts {
        if i > 0 && cur.chance(1, 5) {
            out.push('\n');
        }
        gen_statement(cur, &mut labels, &stmt_pad, opts, out);
    }
    if cur.chance(1, 5) {
        out.push_str(&stmt_pad);
        out.push_str(&gen_comment(cur));
        out.push('\n');
    }
    out.push_str(pad);
    out.push('}');
    if cur.chance(1, 6) {
        out.push(' ');
        out.push_str(&gen_comment(cur));
    }
    out.push('\n');
}

/// One `use` declaration: 1-3 paths, optionally aliased, optionally split
/// across lines with a comment interleaved (docs/pmt/fmt.md (comments
/// inside a `use` list)). No comment is ever placed INSIDE a path — the
/// printer reattributes one written there to the next slot, a shape the
/// corpus pins by name rather than by property.
fn gen_use(cur: &mut Cursor, out: &mut String) {
    out.push_str("use ");
    let paths = 1 + cur.choose(3);
    for i in 0..paths {
        if i > 0 {
            out.push(',');
            // The break is chosen FIRST: a `//` comment consumes the rest
            // of its line, so one may only be written where a newline
            // follows. Without a break the comment has to be a block.
            let broken = cur.chance(2, 3);
            if cur.chance(1, 3) {
                out.push(' ');
                if broken {
                    out.push_str(&gen_comment(cur));
                } else {
                    out.push_str(&format!("/* note {} */", cur.pos));
                }
            }
            if broken {
                out.push_str("\n    ");
            } else {
                out.push(' ');
            }
        }
        out.push_str(&format!("ns{}::fn{}", cur.choose(3), cur.choose(4)));
        if cur.chance(1, 5) {
            out.push_str(&format!(" as alias{}", cur.pos));
        }
    }
    out.push_str(";\n");
}

/// A whole grammar-valid `.pmc` program: file-leading comments, then 1-4
/// top-level units — `use` declarations, functions, and namespaces of
/// functions — separated by the author's own blank lines.
fn generate_program(seed: &[u8]) -> String {
    generate_program_with(seed, Opts::default())
}

/// [`generate_program`] with the spelling knobs pinned — see [`Opts`].
fn generate_program_with(seed: &[u8], opts: Opts) -> String {
    let mut cur = Cursor::new(seed);
    let mut out = String::new();
    let mut fn_id = 0usize;
    let mut main_taken = false;

    for _ in 0..cur.choose(3) {
        out.push_str(&gen_comment(&mut cur));
        out.push('\n');
    }

    let units = 1 + cur.choose(4);
    for u in 0..units {
        if u > 0 && cur.chance(1, 2) {
            out.push('\n');
        }
        match cur.choose(5) {
            0 => gen_use(&mut cur, &mut out),
            1 => {
                out.push_str(&format!("namespace ns{u}"));
                // A header-interior comment (between the name and `{`)
                // — the namespace half of the relocation surface.
                if cur.chance(1, 5) {
                    let n = cur.pos;
                    out.push_str(&format!(" /* note {n} */"));
                }
                out.push_str(" {");
                if cur.chance(1, 3) {
                    out.push(' ');
                    out.push_str(&gen_comment(&mut cur));
                }
                out.push('\n');
                for i in 0..1 + cur.choose(2) {
                    if i > 0 && cur.chance(1, 2) {
                        out.push('\n');
                    }
                    let name = format!("pf{fn_id}");
                    fn_id += 1;
                    gen_function(&mut cur, &name, "    ", false, opts, &mut out);
                }
                if cur.chance(1, 5) {
                    out.push_str("    ");
                    out.push_str(&gen_comment(&mut cur));
                    out.push('\n');
                }
                out.push('}');
                if cur.chance(1, 6) {
                    out.push(' ');
                    out.push_str(&gen_comment(&mut cur));
                }
                out.push('\n');
            }
            _ => {
                // One `main` per program at most, and only there may
                // `volatile` appear.
                let name = if !main_taken && cur.chance(1, 3) {
                    main_taken = true;
                    "main".to_string()
                } else {
                    let n = format!("pf{fn_id}");
                    fn_id += 1;
                    n
                };
                gen_function(&mut cur, &name, "", name == "main", opts, &mut out);
            }
        }
    }
    out
}

/// `TokenKind` sequence, comments stripped — the same view `compiler.rs`
/// feeds the parser, and what the "token equivalence" property compares.
fn kinds(src: &str) -> Vec<TokenKind> {
    lex_with(src, LexMode::WithoutComments)
        .expect("lexes")
        .into_iter()
        .map(|t| t.kind)
        .collect()
}

/// Every comment's text, in source order, trailing whitespace trimmed
/// per line (the one rewrite the printer's `normalize_comment_text` is
/// allowed) — what the "comment preservation" property compares. Token
/// equivalence cannot stand in for it: that property lexes
/// `WithoutComments`, so a printer that silently DELETES a comment
/// passes it, stays idempotent, and moves no command column — exactly
/// the header-comment regression this property was added to catch.
fn comment_texts(src: &str) -> Vec<String> {
    lex_with(src, LexMode::WithComments)
        .expect("lexes")
        .into_iter()
        .filter_map(|t| match t.kind {
            TokenKind::Comment(c) => Some(
                c.text
                    .split('\n')
                    .map(str::trim_end)
                    .collect::<Vec<_>>()
                    .join("\n"),
            ),
            _ => None,
        })
        .collect()
}

/// Which kind of `{ … }` a brace opened, for [`command_columns_agree`]'s
/// walk. Only a function body has a command column; a namespace's interior
/// is just more declarations.
enum Body {
    Namespace,
    /// The column of the first command seen in this body, once one has
    /// been.
    Function(Option<u32>),
}

/// Property 3: within one function body, every command starts at the same
/// column — the body's command column, which a preserved or greedy-filled
/// comma group's continuation lines share
/// (docs/pmt/fmt.md (label and command alignment),
/// docs/pmt/fmt.md (comma groups)). Labels do not: they right-align into
/// the space before that column, and one too wide for it hangs at a single
/// leading space.
///
/// Walks `formatted`'s own token stream rather than its lines, so a
/// comment or a label never has to be told apart from a command by
/// pattern-matching text. Two kinds of position are measured, and both
/// come from a different printer path: the first command of each statement
/// (reached by skipping the `Number Colon` pairs that make up its label
/// prefix, whether or not the prefix has a line of its own), and every
/// token that OPENS a line thereafter — which inside a statement can only
/// be a comma group's continuation.
///
/// Returns the offending body's two disagreeing columns, for the failure
/// message.
fn command_columns_agree(formatted: &str) -> Result<(), String> {
    let tokens: Vec<_> = lex_with(formatted, LexMode::WithoutComments)
        .expect("fmt's own output lexes")
        .into_iter()
        .filter(|t| t.kind != TokenKind::Eof)
        .collect();

    let mut stack: Vec<Body> = Vec::new();
    let mut i = 0usize;
    // True at every position a statement (or a declaration) may start:
    // right after entering a body, and after each `;` or `}`.
    let mut at_stmt_start = true;
    while i < tokens.len() {
        let t = &tokens[i];
        match &t.kind {
            TokenKind::LBrace => {
                let is_fn = i > 0 && tokens[i - 1].kind == TokenKind::RParen;
                stack.push(if is_fn {
                    Body::Function(None)
                } else {
                    Body::Namespace
                });
                at_stmt_start = true;
                i += 1;
            }
            TokenKind::RBrace => {
                stack.pop();
                at_stmt_start = true;
                i += 1;
            }
            TokenKind::Semi => {
                at_stmt_start = true;
                i += 1;
            }
            _ => {
                let in_fn = matches!(stack.last(), Some(Body::Function(_)));
                if !in_fn {
                    i += 1;
                    continue;
                }
                // A label prefix: skip the `Number Colon` pairs, staying
                // at statement start, and measure the token after them.
                if at_stmt_start
                    && matches!(t.kind, TokenKind::Number(_, _))
                    && tokens.get(i + 1).map(|n| &n.kind) == Some(&TokenKind::Colon)
                {
                    i += 2;
                    continue;
                }
                let opens_a_line = i == 0 || tokens[i - 1].line != t.line;
                if !at_stmt_start && !opens_a_line {
                    i += 1;
                    continue;
                }
                let col = t.col;
                at_stmt_start = false;
                i += 1;
                match stack.last_mut() {
                    Some(Body::Function(seen @ None)) => *seen = Some(col),
                    Some(Body::Function(Some(first))) if *first != col => {
                        return Err(format!(
                            "command column {col} disagrees with {first} earlier in the same body \
                             (line {})",
                            t.line
                        ));
                    }
                    _ => {}
                }
            }
        }
    }
    Ok(())
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
        // not something to assert on — filtered rather than failing the
        // property.
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

        // Property 5: every comment reprints, in order, text intact
        // (docs/pmt/fmt.md (comments) — "trivia-preserving"). See
        // [`comment_texts`] for why none of the other properties
        // implies it.
        prop_assert_eq!(
            comment_texts(&src),
            comment_texts(&once),
            "a comment was dropped, reordered or rewritten for generated source:\n{}\nformatted \
             to:\n{}",
            src,
            once
        );

        if let Err(why) = command_columns_agree(&once) {
            return Err(TestCaseError::fail(format!(
                "{why}\nfor generated source:\n{src}\nformatted to:\n{once}"
            )));
        }
    }

    /// Property 4: where the author put the line breaks BETWEEN a
    /// statement's stacked labels does not reach the output. Only the
    /// break before the command does (docs/pmt/fmt.md (own-line labels)),
    /// so one program written `1:` / `2: left;` and the same program
    /// written `1: 2: left;` must format to the same text, byte for byte.
    ///
    /// This is the property the other three cannot express. A printer
    /// that reads the first newline after ANY label as the own-line-label
    /// break is perfectly idempotent, touches no token, and still agrees
    /// with itself about the command column — it just formats the whole
    /// body to the wrong one, because a label prefix wrongly ruled
    /// own-line stops counting toward the column's width and every
    /// command in the body moves left with it.
    ///
    /// Trailing comments are switched off on both sides ([`Opts`]): their
    /// aligned-or-ragged verdict is the one thing fmt reads off the
    /// author's source columns, which a label break legitimately moves.
    #[test]
    fn inter_label_line_breaks_do_not_reach_the_output(
        seed in proptest::collection::vec(any::<u8>(), 64..512),
    ) {
        let inline = generate_program_with(&seed, Opts {
            stack_labels: Some(false),
            trailing_comments: false,
        });
        let split = generate_program_with(&seed, Opts {
            stack_labels: Some(true),
            trailing_comments: false,
        });

        let a = format(&inline);
        prop_assume!(a.is_ok(), "generator produced unparsable pmc: {:?}", inline);
        let b = format(&split);
        prop_assume!(b.is_ok(), "generator produced unparsable pmc: {:?}", split);

        prop_assert_eq!(
            a.expect("checked by prop_assume above"),
            b.expect("checked by prop_assume above"),
            "the two label spellings of one program formatted differently:\n{}\n---\n{}",
            inline,
            split
        );
    }
}
