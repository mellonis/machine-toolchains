//! Position classification: what is the cursor *in*? (docs/lsp.md
//! (completions)).
//!
//! # Why the token stream, not the CST
//!
//! A document being typed into is a document that does not parse. Anchoring
//! classification on the CST would switch completions off exactly when they
//! are wanted, so every judgement here is made over the current SIGNIFICANT
//! token stream (WithComments minus comment trivia), which survives every
//! failure except a lex error. The CST and the resolved module supply
//! ROSTERS — names, glyphs, tape tables — never positions.
//!
//! # The three walks
//!
//! 1. **Frames** — one forward scan over the tokens before the cursor
//!    builds the stack of open `{` blocks, each labelled from the header
//!    that precedes it (`namespace N`, `routine N(…)`, `graph N(…)`,
//!    `machine`, `alphabet N`, `state N`, `with map`). The stack yields the
//!    enclosing namespace path and the enclosing world's mangled name,
//!    which is how a cell later finds its tape table.
//! 2. **Brackets** — the nearest unclosed `[` before the cursor, plus the
//!    number of depth-0 commas since it. That comma count IS the tape's
//!    vector position, and the keyword before the `[` (`write`, `move`, or
//!    nothing) says which of the three vectors it is.
//! 3. **Call sites** — the nearest unclosed `(` before the cursor, the
//!    qualified target name written before it, and the keyword before
//!    that. Inside one, the current binding argument's parameter name and
//!    bound target are read off the argument's own tokens, which is what
//!    lets a `with map` cell resolve BOTH sides: the host tape from the
//!    argument's right-hand side against the enclosing world, and the
//!    callee tape from the parameter name against the target world.

use mtc_core::diagnostics::{Pos, Span};

use crate::lexer::{Token, TokenKind};

/// Which of the three bracketed vectors a `[…]` is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum VectorKind {
    Pattern,
    Write,
    Move,
}

/// The kind of construct a call site's `(…)` belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CallKind {
    Call,
    Graft,
    Bind,
}

/// What the cursor sits in. Every variant carries exactly what the
/// candidate builder needs; nothing is re-derived downstream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Context {
    /// Inside a `use …;` statement's path list.
    UsePath,
    /// At a file- or namespace-level item boundary.
    TopLevelItem,
    /// At a world-body item boundary; `machine` gates the `tape` keyword.
    WorldItem { machine: bool },
    /// The alphabet slot of `tape NAME: ▮` (a machine declaration or a
    /// signature parameter — one shape, one context).
    AlphabetRef,
    /// A cell of a bracketed vector at vector position `index`.
    VectorCell { kind: VectorKind, index: usize },
    /// Just after `->`, or after a completed `write […]` / `move […]`:
    /// the action keywords plus the bare-name transition sugar.
    ActionStart,
    /// Just after `goto`.
    GotoTarget,
    /// Just after `then`.
    Continuation,
    /// The target slot of `call ▮`, `graft ▮`, or `bind ▮`.
    Target(CallKind),
    /// A binding argument's parameter-name slot (`(▮` or `, ▮`).
    BindingName { target: Option<String> },
    /// A binding argument's value slot (`param = ▮`).
    BindingValue { param: Option<String> },
    /// The source side of a `with map { ▮ -> … }` pair — the HOST tape's
    /// alphabet, so `host_tape` names a tape of the ENCLOSING world.
    MapSrc { host_tape: Option<String> },
    /// The destination side of a map pair — the CALLEE tape's alphabet, so
    /// `param` names a tape parameter of `target`.
    MapDst {
        target: Option<String>,
        param: Option<String>,
    },
}

/// A frame on the open-block stack.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Frame {
    Namespace(String),
    /// A `machine` / `routine N` / `graph N` block; the name is as
    /// WRITTEN (`machine` becomes the mangled `main` at lookup time).
    World {
        name: String,
        machine: bool,
    },
    Alphabet,
    State,
    Map,
    Other,
}

/// The classified cursor: its context plus the scope the context resolves
/// names against.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Cursor {
    pub(crate) context: Context,
    /// Enclosing namespace path, outermost first.
    pub(crate) namespaces: Vec<String>,
    /// The enclosing world's mangled name (`main` for a machine block,
    /// `ns::name` otherwise), or `None` outside every world.
    pub(crate) world: Option<String>,
    /// The span a candidate replaces: the whole identifier-ish token the
    /// cursor touches, else zero-width at the cursor.
    pub(crate) replace_span: Span,
}

/// The token the cursor is completing over, and where a candidate's text
/// must land. An `Ident` / `Number` / `Glyph` whose span contains the
/// cursor (or ends exactly at it) becomes the whole-token `replace_span`;
/// otherwise the span is zero-width at the cursor. This is the single seam
/// that keeps `replace_span` always on the cursor's line and touching the
/// cursor — by construction, never by a follow-up check. The server never
/// text-filters by the typed prefix; that is the client's job over
/// `replace_span`.
fn prefix_anchor(sig: &[Token], pos: Pos) -> (Span, usize) {
    for (i, token) in sig.iter().enumerate() {
        if matches!(token.kind, TokenKind::Eof) {
            break;
        }
        let span = token.span();
        if span.start <= pos
            && pos <= span.end
            && matches!(
                token.kind,
                TokenKind::Ident(_) | TokenKind::Number(..) | TokenKind::Glyph(_)
            )
        {
            return (span, i);
        }
    }
    let empty = Span {
        start: pos,
        end: pos,
    };
    let idx = sig
        .iter()
        .position(|t| t.span().start >= pos || matches!(t.kind, TokenKind::Eof))
        .unwrap_or(sig.len());
    (empty, idx)
}

fn ident(token: &Token) -> Option<&str> {
    match &token.kind {
        TokenKind::Ident(s) => Some(s.as_str()),
        _ => None,
    }
}

fn is_kw(token: &Token, word: &str) -> bool {
    ident(token) == Some(word)
}

/// The open-block stack at `idx`, built by one forward scan. Each `{`
/// takes its label from the header immediately before it, so an unclosed
/// block simply stays on the stack — which is precisely the mid-edit case
/// this whole module exists to serve.
fn frames(sig: &[Token], idx: usize) -> Vec<Frame> {
    let mut stack: Vec<Frame> = Vec::new();
    for i in 0..idx.min(sig.len()) {
        match sig[i].kind {
            TokenKind::LBrace => stack.push(frame_for_brace(sig, i)),
            TokenKind::RBrace => {
                stack.pop();
            }
            _ => {}
        }
    }
    stack
}

/// Labels the `{` at `open` from the tokens before it: a signature's
/// `(…)` is stepped over first, then a `NAME` + keyword pair, else a lone
/// `machine` / `map` keyword.
fn frame_for_brace(sig: &[Token], open: usize) -> Frame {
    let Some(mut j) = open.checked_sub(1) else {
        return Frame::Other;
    };
    if matches!(sig[j].kind, TokenKind::RParen) {
        let mut depth = 0i32;
        loop {
            match sig[j].kind {
                TokenKind::RParen => depth += 1,
                TokenKind::LParen => {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                }
                _ => {}
            }
            match j.checked_sub(1) {
                Some(prev) => j = prev,
                None => return Frame::Other,
            }
        }
        match j.checked_sub(1) {
            Some(prev) => j = prev,
            None => return Frame::Other,
        }
    }
    if is_kw(&sig[j], "machine") {
        return Frame::World {
            name: "machine".to_string(),
            machine: true,
        };
    }
    if is_kw(&sig[j], "map") {
        return Frame::Map;
    }
    let Some(name) = ident(&sig[j]) else {
        return Frame::Other;
    };
    let Some(keyword) = j.checked_sub(1).and_then(|k| ident(&sig[k])) else {
        return Frame::Other;
    };
    match keyword {
        "namespace" => Frame::Namespace(name.to_string()),
        "routine" | "graph" => Frame::World {
            name: name.to_string(),
            machine: false,
        },
        "alphabet" => Frame::Alphabet,
        "state" => Frame::State,
        _ => Frame::Other,
    }
}

/// The nearest `[` before `idx` that is still open, with the number of
/// depth-0 commas since it — the cell's vector position.
fn open_bracket(sig: &[Token], idx: usize) -> Option<(usize, usize)> {
    let mut depth = 0i32;
    let mut i = idx;
    let open = loop {
        i = i.checked_sub(1)?;
        match sig[i].kind {
            TokenKind::RBracket => depth += 1,
            TokenKind::LBracket => {
                if depth == 0 {
                    break i;
                }
                depth -= 1;
            }
            // A rule never spans a block boundary: hitting one means the
            // cursor is not in a vector at all.
            TokenKind::LBrace | TokenKind::RBrace | TokenKind::Semi => return None,
            _ => {}
        }
    };
    let mut cell = 0usize;
    let mut nested = 0i32;
    for token in &sig[open + 1..idx] {
        match token.kind {
            TokenKind::LBracket | TokenKind::LParen | TokenKind::LBrace => nested += 1,
            TokenKind::RBracket | TokenKind::RParen | TokenKind::RBrace => nested -= 1,
            TokenKind::Comma if nested == 0 => cell += 1,
            _ => {}
        }
    }
    Some((open, cell))
}

fn vector_kind(sig: &[Token], open: usize) -> VectorKind {
    match open.checked_sub(1).and_then(|j| ident(&sig[j])) {
        Some("write") => VectorKind::Write,
        Some("move") => VectorKind::Move,
        _ => VectorKind::Pattern,
    }
}

/// A call/graft/bind argument list the cursor sits inside.
struct CallSite {
    open: usize,
    /// The `::`-joined target name written before the `(`.
    target: String,
}

/// The nearest `(` before `idx` that is still open, plus the target name
/// and construct keyword written before it.
fn call_site(sig: &[Token], idx: usize) -> Option<CallSite> {
    let mut depth = 0i32;
    let mut i = idx;
    let open = loop {
        i = i.checked_sub(1)?;
        match sig[i].kind {
            TokenKind::RParen => depth += 1,
            TokenKind::LParen => {
                if depth == 0 {
                    break i;
                }
                depth -= 1;
            }
            TokenKind::Semi => return None,
            _ => {}
        }
    };
    // Walk the qualified name backwards: IDENT (:: IDENT)*.
    let mut j = open.checked_sub(1)?;
    let mut segments: Vec<&str> = vec![ident(&sig[j])?];
    while j >= 2 && matches!(sig[j - 1].kind, TokenKind::ColonColon) {
        let seg = ident(&sig[j - 2])?;
        segments.push(seg);
        j -= 2;
    }
    segments.reverse();
    // Only these three constructs take an argument list, so anything else
    // before the `(` means the cursor is not in a binding list at all.
    if !matches!(
        j.checked_sub(1).and_then(|k| ident(&sig[k])),
        Some("call") | Some("graft") | Some("bind")
    ) {
        return None;
    }
    Some(CallSite {
        open,
        target: segments.join("::"),
    })
}

/// The token range of the binding argument the cursor is inside: from just
/// after the argument list's opening `(` or the last depth-0 comma, up to
/// the cursor.
fn current_argument(sig: &[Token], open: usize, idx: usize) -> (usize, usize) {
    let mut start = open + 1;
    let mut nested = 0i32;
    for (offset, token) in sig[open + 1..idx].iter().enumerate() {
        match token.kind {
            TokenKind::LParen | TokenKind::LBrace | TokenKind::LBracket => nested += 1,
            TokenKind::RParen | TokenKind::RBrace | TokenKind::RBracket => nested -= 1,
            TokenKind::Comma if nested == 0 => start = open + 1 + offset + 1,
            _ => {}
        }
    }
    (start, idx)
}

/// `(parameter name, bound target)` of the binding argument spanning
/// `range` — the `param = target …` shape, either half absent while the
/// argument is still being typed.
fn argument_parts(sig: &[Token], range: (usize, usize)) -> (Option<String>, Option<String>) {
    let (start, end) = range;
    let slice = &sig[start..end.min(sig.len())];
    let eq = slice.iter().position(|t| matches!(t.kind, TokenKind::Eq));
    let param = slice.first().and_then(ident).map(str::to_string);
    let target = eq
        .and_then(|e| slice.get(e + 1))
        .and_then(ident)
        .map(str::to_string);
    (param, target)
}

/// True when the cursor sits on the destination side of the map pair it is
/// in — i.e. an arrow has been written since the pair began.
fn past_map_arrow(sig: &[Token], idx: usize) -> bool {
    let mut i = idx;
    while let Some(prev) = i.checked_sub(1) {
        i = prev;
        match sig[i].kind {
            TokenKind::Arrow | TokenKind::FatArrow => return true,
            TokenKind::Comma | TokenKind::LBrace => return false,
            _ => {}
        }
    }
    false
}

/// The index just after the statement boundary before `idx` (`;`, `{` or
/// `}`), i.e. where the statement the cursor is in begins.
fn statement_start(sig: &[Token], idx: usize) -> usize {
    let mut i = idx;
    while let Some(prev) = i.checked_sub(1) {
        if matches!(
            sig[prev].kind,
            TokenKind::Semi | TokenKind::LBrace | TokenKind::RBrace
        ) {
            return i;
        }
        i = prev;
    }
    0
}

/// Classifies `pos` in `sig`. `None` means "no context matched" — the
/// caller offers nothing rather than guessing, which is what keeps a
/// wrong-context list from ever appearing.
pub(crate) fn classify(sig: &[Token], pos: Pos) -> Option<Cursor> {
    let (replace_span, idx) = prefix_anchor(sig, pos);
    let stack = frames(sig, idx);
    let namespaces: Vec<String> = stack
        .iter()
        .filter_map(|f| match f {
            Frame::Namespace(n) => Some(n.clone()),
            _ => None,
        })
        .collect();
    let world_frame = stack.iter().rev().find_map(|f| match f {
        Frame::World { name, machine } => Some((name.clone(), *machine)),
        _ => None,
    });
    let world = world_frame.as_ref().map(|(name, machine)| {
        if *machine {
            "main".to_string()
        } else if namespaces.is_empty() {
            name.clone()
        } else {
            format!("{}::{name}", namespaces.join("::"))
        }
    });
    let innermost = stack.last();

    let context = classify_context(sig, idx, &stack, innermost)?;
    Some(Cursor {
        context,
        namespaces,
        world,
        replace_span,
    })
}

fn classify_context(
    sig: &[Token],
    idx: usize,
    stack: &[Frame],
    innermost: Option<&Frame>,
) -> Option<Context> {
    let prev = idx.checked_sub(1).map(|j| &sig[j]);

    // An alphabet body holds glyph literals and ranges only — nothing to
    // complete, and offering the world/item keywords there would be wrong.
    if matches!(innermost, Some(Frame::Alphabet)) {
        return None;
    }

    // A `with map { … }` body: which side of the pair, and against which
    // tape. The two answers come from different worlds, which is why the
    // context carries both halves of the binding argument.
    if matches!(innermost, Some(Frame::Map)) {
        let site = call_site(sig, idx)?;
        let (param, target) = argument_parts(sig, current_argument(sig, site.open, idx));
        return Some(if past_map_arrow(sig, idx) {
            Context::MapDst {
                target: Some(site.target),
                param,
            }
        } else {
            Context::MapSrc { host_tape: target }
        });
    }

    // The alphabet slot of `tape NAME: ▮`, in a machine declaration or a
    // signature parameter alike.
    if matches!(prev.map(|t| &t.kind), Some(TokenKind::Colon))
        && let Some(name_idx) = idx.checked_sub(2)
        && ident(&sig[name_idx]).is_some()
        && name_idx
            .checked_sub(1)
            .is_some_and(|k| is_kw(&sig[k], "tape"))
    {
        return Some(Context::AlphabetRef);
    }

    // A `use …;` statement anywhere at item level.
    let stmt = statement_start(sig, idx);
    if sig.get(stmt).is_some_and(|t| is_kw(t, "use")) && stmt < idx {
        return Some(Context::UsePath);
    }

    // Inside a bracketed vector: the comma count is the tape position.
    if let Some((open, index)) = open_bracket(sig, idx) {
        return Some(Context::VectorCell {
            kind: vector_kind(sig, open),
            index,
        });
    }

    // Inside a call/graft/bind argument list.
    if let Some(site) = call_site(sig, idx) {
        return Some(match prev.map(|t| &t.kind) {
            Some(TokenKind::Eq) => {
                let (param, _) = argument_parts(sig, current_argument(sig, site.open, idx));
                Context::BindingValue { param }
            }
            Some(TokenKind::LParen) | Some(TokenKind::Comma) => Context::BindingName {
                target: Some(site.target),
            },
            _ => return None,
        });
    }

    // The target slot right after the construct keyword.
    if let Some(prev) = prev {
        match ident(prev) {
            Some("call") => return Some(Context::Target(CallKind::Call)),
            Some("graft") => return Some(Context::Target(CallKind::Graft)),
            Some("bind") => return Some(Context::Target(CallKind::Bind)),
            Some("goto") => return Some(Context::GotoTarget),
            Some("then") => return Some(Context::Continuation),
            _ => {}
        }
        // The action half of a rule: right after the arrow, or after a
        // completed `write […]` / `move […]` vector.
        if matches!(prev.kind, TokenKind::Arrow)
            || (matches!(prev.kind, TokenKind::RBracket) && after_action_vector(sig, idx))
        {
            return Some(Context::ActionStart);
        }
    }

    // An item boundary: which keywords depend on the enclosing block.
    let at_boundary = prev.is_none_or(|t| {
        matches!(
            t.kind,
            TokenKind::Semi | TokenKind::LBrace | TokenKind::RBrace
        )
    });
    if !at_boundary {
        return None;
    }
    match innermost {
        None | Some(Frame::Namespace(_)) => Some(Context::TopLevelItem),
        Some(Frame::World { machine, .. }) => Some(Context::WorldItem { machine: *machine }),
        // Inside a state body a boundary opens a rule, which starts with a
        // pattern bracket — no name or keyword to offer.
        Some(Frame::State) => None,
        _ => {
            let _ = stack;
            None
        }
    }
}

/// True when the `]` just before the cursor closed a `write` / `move`
/// vector (as opposed to a rule's leading pattern) — the action half
/// continues after those two, but a pattern's `]` is followed by `->`.
fn after_action_vector(sig: &[Token], idx: usize) -> bool {
    let mut depth = 0i32;
    let mut i = idx - 1;
    loop {
        match sig[i].kind {
            TokenKind::RBracket => depth += 1,
            TokenKind::LBracket => {
                depth -= 1;
                if depth == 0 {
                    break;
                }
            }
            _ => {}
        }
        match i.checked_sub(1) {
            Some(prev) => i = prev,
            None => return false,
        }
    }
    matches!(
        i.checked_sub(1).and_then(|j| ident(&sig[j])),
        Some("write") | Some("move")
    )
}
