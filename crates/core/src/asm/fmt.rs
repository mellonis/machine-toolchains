//! Canonical-grid printer for assembly text (docs/formats.md (assembly
//! text)): label col 0, mnemonic col 8, operand col 16 are fixed;
//! trailing comment col 32 is a FLOOR, not fixed — it aligns per group
//! at `max(32, widest code width in the group + 1)` (see
//! [`comment_columns`]). Zero token changes — whitespace/newlines only.
//!
//! Pure CST walk (mirrors the `.pmc` printer's discipline, `crates/
//! post-machine/src/fmt/mod.rs`, but is far simpler: assembly text is
//! already line-oriented, so there is no indentation nesting). Every
//! field lands on a fixed column, or on its group's comment column, or,
//! failing that, one space past wherever the previous field ended. The
//! one exception is the three unbounded lists — `.targets`, `.exits`,
//! `.map` — which DO wrap: each runs onto further physical lines when
//! its code would exceed [`LINE_WIDTH_LIMIT`] columns, continuation
//! lines aligned under the list's first element (see
//! [`wrap_operand_list`]). This gets every element under the budget for
//! `.targets`/`.exits`, whose elements are individual names — but NOT
//! for `.map`, whose wrap point is between its `<k>`/`rmap=(…)`/`wmap=(…)`
//! clauses rather than inside a clause's own pair list (see
//! [`render_frame_directive`]): a single clause wider than the budget on
//! its own still prints as one over-budget line. The budget bounds code
//! only; a trailing comment still lands at its group's uncapped column
//! same as every other line here, so a wrapped list's commented last
//! line can still run past it. Columns below are 0-based (tab-stop
//! convention, matching `disassembler.rs`'s `grid_line` and
//! docs/formats.md); the CST's
//! `Span`/`Pos::col` fields are 1-based, so a 0-based target of 32 is the
//! 1-based column 33 a `TrailingComment.col` would report at the floor
//! (a wider group's column shifts that report accordingly).

use super::cst::{
    AsmCst, AsmItemKind, FrameDirectiveCst, FrameMapCst, FramePairCst, FuncCst, LabelCst, LineCst,
    OperandToken, ReptCst, RoutineDirectiveCst, SectionCst, TableDirectiveCst, TableDirectiveKind,
    TrailingComment, VolatileCst, parse_asm_cst_with,
};
use super::syntax::AsmCaps;
use super::{AsmError, AsmErrorKind};

const TOP_COL: usize = 0;
const MNEMONIC_COL: usize = 8;
const OPERAND_COL: usize = 16;
const COMMENT_COL: usize = 32;

/// The code-line width budget a `.targets`/`.exits`/`.map` list wraps
/// against (docs/formats.md (assembly text)) — the same 80-column limit
/// `line-too-long` reports past. Only the code is measured; a trailing
/// comment still lands at its group's uncapped column (see
/// [`comment_columns`]), so this constant plays no part in comment
/// placement and does not by itself guarantee a wrapped line's full text
/// — comment included — stays under it.
const LINE_WIDTH_LIMIT: usize = 80;

/// A label field (name + `:`) of this many chars or fewer leaves a
/// mandatory `>= 1` space before [`MNEMONIC_COL`] and stays on the same
/// physical line as whatever follows; an 8-char field would touch the
/// mnemonic column, so it — and any longer field — moves to its own
/// line instead.
const MAX_INLINE_LABEL_FIELD: usize = 7;

/// `.pma` source → canonical grid text, classic dialect (no opt-in
/// surface). Thin wrapper over [`format_asm_with`] at
/// [`AsmCaps::default`] — byte-identical to the pre-caps printer, since
/// sections, table directives, and `.rept` blocks never shape under the
/// default caps.
pub fn format_asm(source: &str) -> Result<String, AsmError> {
    format_asm_with(source, AsmCaps::default())
}

/// `.pma` source → canonical grid text under `caps` (the dialect's
/// opt-in surface). Err = the structural gate: the file contains a Raw
/// (non-assembly) line — a disassembly-listing row, a stray `<name>`,
/// `A: 5`, and the like; nothing else refuses (an unknown mnemonic still
/// formats — this layer has no semantic gate, only the CST's structural
/// one). Thin renderer: never prints.
///
/// The opt-in nodes normalize to the same column grid as ordinary lines,
/// with ONE exception: a `.rept` block's BODY prints VERBATIM from source
/// (macros as written) — see [`render_rept`] — because a body item's CST
/// shaping is intentionally imperfect for substitution templates
/// (`Linc{v}: nop` shapes labelless), and grid-printing it would corrupt
/// its text.
///
/// Two phases: [`render_pieces`] measures every item into a [`Piece`]
/// (code and held-back comment; every comment is held back, including an
/// own-line one, which measures with empty `code`), then this loop
/// emits them, padding each held-back comment to its own target column.
///
/// The target-column scan below chooses a column per piece from
/// [`comment_columns`]'s per-group result — passed straight through for a
/// trailing comment, or through [`own_line_comment_col`] for an own-line
/// one, which additionally decides between that group column and column 0
/// — as a `Vec<usize>` alongside `pieces` rather than folding the choice
/// into [`render_pieces`] itself: `render_pieces` (the measure phase) must
/// not know about columns at all.
pub fn format_asm_with(source: &str, caps: AsmCaps) -> Result<String, AsmError> {
    let cst = parse_asm_cst_with(source, caps);
    if let Some(raw) = cst.items.iter().find_map(|item| match &item.kind {
        AsmItemKind::Raw(r) => Some(r),
        _ => None,
    }) {
        return Err(AsmError {
            span: raw.span,
            kind: AsmErrorKind::RawLine,
        });
    }

    let pieces = render_pieces(&cst, source);
    let group_cols = comment_columns(&pieces);
    let comment_cols: Vec<usize> = pieces
        .iter()
        .enumerate()
        .map(|(i, p)| match p.kind {
            PieceKind::Comment => own_line_comment_col(&pieces, i, group_cols[i]),
            _ => group_cols[i],
        })
        .collect();

    let mut out = String::new();
    for (i, p) in pieces.iter().enumerate() {
        // Blank-line runs already collapsed to one bool by the CST
        // (`blank_before`); item 0 is guaranteed `false` by construction
        // (no leading file blanks), so this also gives "no leading
        // blanks" for free — the `i > 0` guard is defensive, matching
        // the `.pmc` printer's convention.
        if i > 0 && p.blank_before {
            out.push('\n');
        }
        let mut line = p.code.clone();
        if let Some(hc) = &p.header_comment {
            // Only a `.rept` piece ever sets this, and its header line
            // is always followed by at least the `.endr` line, so `code`
            // always has an interior `\n` here. The final `line.trim_end()`
            // below only reaches the LAST physical line, so this interior
            // one is trimmed explicitly after the comment is appended —
            // the same reason [`render_rept`] used to trim it inline.
            let split_at = line
                .find('\n')
                .expect("a piece with a header_comment always has a header line above its body");
            let mut header_line = line[..split_at].to_string();
            let mut hcol = header_line.chars().count();
            pad_to(&mut header_line, &mut hcol, comment_cols[i]);
            header_line.push_str(hc);
            line = format!("{}\n{}", header_line.trim_end(), &line[split_at + 1..]);
        }
        let mut col = line.rsplit('\n').next().unwrap_or("").chars().count();
        if let Some(c) = &p.comment {
            // `pad_to`'s "already at/past the stop → one separating
            // space" branch is right for a real code line that runs
            // into its comment column, but wrong for a standalone
            // own-line comment with no code before it: landing exactly
            // on column 0 there means zero characters precede the
            // comment, not one space. Building the indent from scratch
            // when `line` is empty (which — given no [`render_X`]
            // producer ever leaves `code` ending in `\n`, so an empty
            // last-line implies `code` is entirely empty — only ever
            // happens for that case) sidesteps the ambiguity; for every
            // non-empty `line` this is byte-identical to `pad_to`.
            if line.is_empty() {
                line.push_str(&" ".repeat(comment_cols[i]));
            } else {
                pad_to(&mut line, &mut col, comment_cols[i]);
            }
            line.push_str(c);
        }
        out.push_str(line.trim_end());
        out.push('\n');
    }
    Ok(out)
}

/// What a printed item is, for the group scan in [`comment_columns`].
#[derive(PartialEq)]
enum PieceKind {
    /// A code line that may carry a trailing comment.
    Line,
    /// An own-line comment.
    Comment,
    /// `.section` / `.func` / `.routine` — a structural item.
    Structural,
    /// A `.rept` block; its body prints verbatim.
    Rept,
}

/// One item's rendered code, with its trailing comment held back so a
/// later pass can choose the column. An own-line comment holds its text
/// here too, with empty `code` — there is no code line for it to trail,
/// but it still goes through the same held-back-comment shape so a
/// later pass can choose its column the same way it chooses every
/// other's (today, [`format_asm_with`]'s per-item column scan).
struct Piece {
    code: String,
    comment: Option<String>,
    /// A SECOND held-back trailing comment, anchored to `code`'s FIRST
    /// line rather than its last. Only [`render_rept`] ever sets this —
    /// a `.rept` block's header carries its own trailing comment on a
    /// physical line that is not the piece's last, so it cannot share
    /// `comment`'s slot — but it still goes through the same per-group
    /// column [`comment`] does ([`comment_columns`] does not
    /// distinguish the two), rather than a fixed column of its own.
    header_comment: Option<String>,
    /// Tells an own-line comment apart from a code line's trailing one,
    /// a structural directive, and a `.rept` block — consumed by
    /// [`comment_columns`]'s group scan and, through
    /// [`continues_a_trailing_comment`], by [`own_line_comment_col`].
    kind: PieceKind,
    blank_before: bool,
}

/// One Piece per CST item, comments held back for a later column choice.
/// `blank_before` mirrors the single-pass loop this replaced: items are
/// walked once, in order, and a blank-line run before item `i` sets it.
fn render_pieces(cst: &AsmCst, source: &str) -> Vec<Piece> {
    cst.items
        .iter()
        .enumerate()
        .map(|(i, item)| {
            let mut p = match &item.kind {
                AsmItemKind::Comment(c) => Piece {
                    code: String::new(),
                    comment: Some(c.text.clone()),
                    header_comment: None,
                    kind: PieceKind::Comment,
                    blank_before: false,
                },
                AsmItemKind::Func(f) => render_func(f),
                AsmItemKind::Line(l) => render_line(l),
                AsmItemKind::Raw(_) => unreachable!("the structural gate already refused"),
                AsmItemKind::Section(s) => render_section(s),
                AsmItemKind::TableDirective(d) => render_table_directive(d),
                AsmItemKind::Rept(r) => render_rept(r, source),
                AsmItemKind::RoutineDirective(r) => render_routine(r),
                AsmItemKind::FrameDirective(d) => render_frame_directive(d),
                AsmItemKind::Volatile(v) => render_volatile(v),
            };
            p.blank_before = i > 0 && item.blank_before;
            p
        })
        .collect()
}

/// `.func name [local] [; comment]` — a column-0 directive.
fn render_func(f: &FuncCst) -> Piece {
    let mut line = String::from(".func ");
    line.push_str(&f.name);
    if f.local {
        line.push_str(" local");
    }
    Piece {
        code: line,
        comment: f.trailing.as_ref().map(|tc| tc.text.clone()),
        header_comment: None,
        kind: PieceKind::Structural,
        blank_before: false,
    }
}

/// `.volatile [; comment]` — a column-0 directive, printed like `.func`:
/// it belongs to the block structure (it names its `.func`'s build column),
/// not to the instruction grid.
fn render_volatile(v: &VolatileCst) -> Piece {
    Piece {
        code: String::from(".volatile"),
        comment: v.trailing.as_ref().map(|tc| tc.text.clone()),
        header_comment: None,
        kind: PieceKind::Structural,
        blank_before: false,
    }
}

/// Does the own-line comment at `i` continue a trailing comment above it
/// (docs/formats.md (assembly text)), or open a new structural block? A
/// blank line above breaks the continuation.
fn continues_a_trailing_comment(pieces: &[Piece], i: usize) -> bool {
    if pieces[i].blank_before {
        return false;
    }
    let mut j = i;
    while j > 0 {
        j -= 1;
        match pieces[j].kind {
            PieceKind::Comment if !pieces[j].blank_before => continue,
            PieceKind::Line => return pieces[j].comment.is_some(),
            _ => return false,
        }
    }
    false
}

/// Own-line comment column (docs/formats.md (assembly text)). Two cases:
/// a run continuing the trailing comment above it prints at that group's
/// comment column; everything else is structural and prints at column 0.
///
/// Column 8 is the mnemonic column — where statements live. A comment is
/// not a statement, so it is never placed there.
fn own_line_comment_col(pieces: &[Piece], i: usize, group_col: usize) -> usize {
    if continues_a_trailing_comment(pieces, i) {
        group_col
    } else {
        TOP_COL
    }
}

/// Per-group trailing-comment column (docs/formats.md (assembly text)):
/// `max(COMMENT_COL, widest code width in the group + 1)`. COMMENT_COL is
/// a floor, so a group only ever widens — which is what keeps output
/// unchanged for dialects whose operands all fit before it.
///
/// A group ends at a blank line, an own-line comment at column 0, a
/// structural directive, or a `.rept` block. A `.rept` body prints
/// verbatim rather than through this grid, so it contributes no width:
/// aligning a group to a member that never joins it would be incoherent.
///
/// Width comes from carrying a comment, not from a piece's kind: any
/// piece with one — an instruction line, but equally a commented
/// `.section`/`.func`/`.routine`/`.rept` — contributes the width of the
/// code IT trails. Excluding a kind here would let it be padded out to
/// a column its own width never justified, stranding it ragged against
/// the rest of its group — the exact defect group alignment exists to
/// remove. A piece with no comment contributes no width regardless of
/// kind. A `.rept` piece can contribute width from TWO lines at once —
/// its header line, if [`Piece::header_comment`] holds one, and its
/// last line (`.endr`), if [`Piece::comment`] holds one — since the two
/// are independently-anchored comments that still land at this one
/// group column together; whichever line is wider is what the group
/// must be at least that wide to hold cleanly.
///
/// The column is NOT capped by any line limit; `line-too-long` reports an
/// overlong result. The `.tmc` printer makes the same call.
///
/// Reuses [`continues_a_trailing_comment`] for the own-line-comment half
/// of the `ends` test rather than a second predicate: [`own_line_comment_col`]
/// decides the same question when it picks between a group column and
/// column 0, so the two must never disagree about where a group ends.
fn comment_columns(pieces: &[Piece]) -> Vec<usize> {
    let mut cols = vec![COMMENT_COL; pieces.len()];
    let mut start = 0;
    for i in 0..=pieces.len() {
        let ends = i == pieces.len()
            || pieces[i].blank_before
            || matches!(pieces[i].kind, PieceKind::Structural | PieceKind::Rept)
            || (pieces[i].kind == PieceKind::Comment && !continues_a_trailing_comment(pieces, i));
        if !ends {
            continue;
        }
        let widest = (start..i)
            .map(|k| {
                let p = &pieces[k];
                let trailing_width = if p.comment.is_some() {
                    p.code.rsplit('\n').next().unwrap_or("").chars().count()
                } else {
                    0
                };
                let header_width = if p.header_comment.is_some() {
                    p.code.split('\n').next().unwrap_or("").chars().count()
                } else {
                    0
                };
                trailing_width.max(header_width)
            })
            .max()
            .unwrap_or(0);
        let col = COMMENT_COL.max(widest + 1);
        for c in cols.iter_mut().take(i).skip(start) {
            *c = col;
        }
        start = i;
    }
    cols
}

/// One `label* [word operands] [; comment]` line. Non-last labels
/// always get their own line (position rule); the last label shares
/// its rule with a solo label — own line at 8+ chars, otherwise inline
/// with whatever follows on the same physical line. When the line ends
/// up with nothing to print after an own-line label (a long, bare,
/// label-only line), the empty continuation is dropped rather than
/// leaving a blank line behind.
///
/// A long label with NO instruction (a label-only line) is the one
/// case where "own line" must NOT split the physical line further: if
/// it carries a trailing comment, that comment stays on the label's
/// own line (padded to its group's comment column — [`COMMENT_COL`] as
/// a floor — or one space past the field when the field itself runs
/// past that stop) rather than moving to a bare continuation line. A
/// bare continuation reparses as an OWN-LINE
/// comment (no label on that physical line) whose own-line predicate
/// (`continues_a_trailing_comment`) would then find the label's `Line`
/// piece above it carrying no trailing comment of its own — the
/// comment having moved off onto its own line — so the printer would
/// re-indent it to [`TOP_COL`] on a second pass — an idempotence
/// violation (`format(format(x)) != format(x)`). Keeping
/// the comment on the label's line reparses to the identical
/// label-with-trailing-comment shape, so pass 1 is already a fixed
/// point. This only applies when `instr` is `None`: when an
/// instruction follows, it owns the continuation line and the label
/// line has nothing else to carry.
fn render_line(line: &LineCst) -> Piece {
    let instr = line
        .instr
        .as_ref()
        .map(|i| (i.word.as_str(), i.operands.as_slice()));
    // An ordinary instruction's operand list is never one of the three
    // unbounded lists, so it never wraps.
    render_fields(&line.labels, instr, &line.trailing, false)
}

/// The shared `label* [word operands] [; comment]` grid printer, driving
/// [`render_line`], [`render_table_directive`], and
/// [`render_frame_directive`] — a table or frame directive is the same
/// shape with a mandatory directive word standing in for the mnemonic.
/// `instr` is `None` only for a label-only Line; a table or frame
/// directive always passes `Some`, so the long-label-only-line-with-
/// trailing-comment idempotency guard (the `instr.is_none()` branch
/// below) can never fire for one. The trailing comment is held back into
/// `Piece::comment` rather than padded here — [`format_asm_with`]'s emit
/// loop pads every such piece to its target column.
///
/// `wrap` opts into [`wrap_operand_list`] for the three unbounded lists
/// (`.targets`, `.exits`, `.map`) — every other caller passes `false`,
/// since their operand lists are bounded and never reach the width where
/// a break would earn its keep (docs/formats.md (assembly text)).
fn render_fields(
    labels: &[LabelCst],
    instr: Option<(&str, &[OperandToken])>,
    trailing: &Option<TrailingComment>,
    wrap: bool,
) -> Piece {
    let mut out = String::new();
    let n = labels.len();
    for label in &labels[..n.saturating_sub(1)] {
        out.push_str(&label.name);
        out.push_str(":\n");
    }

    let mut cur = String::new();
    if let Some(last) = labels.last() {
        let field = format!("{}:", last.name);
        let fits_inline = field.chars().count() <= MAX_INLINE_LABEL_FIELD;
        if fits_inline || instr.is_none() {
            cur.push_str(&field);
        } else {
            out.push_str(&field);
            out.push('\n');
        }
    }
    let mut col = cur.chars().count();

    if let Some((word, operands)) = instr {
        pad_to(&mut cur, &mut col, MNEMONIC_COL);
        cur.push_str(word);
        col += word.chars().count();

        let operand_text = join_operands(operands);
        if !operand_text.is_empty() {
            // `col`'s final value would only feed a trailing-comment pad
            // — held back into `Piece::comment` now, not computed here —
            // so this branch's last write to it is intentionally dropped.
            pad_to(&mut cur, &mut col, OPERAND_COL);
            if wrap && col + operand_text.chars().count() > LINE_WIDTH_LIMIT {
                let texts: Vec<&str> = operands.iter().map(|o| o.text.as_str()).collect();
                cur.push_str(&wrap_operand_list(&texts, col));
            } else {
                cur.push_str(&operand_text);
            }
        }
    }

    out.push_str(cur.trim_end());
    Piece {
        code: out,
        comment: trailing.as_ref().map(|tc| tc.text.clone()),
        header_comment: None,
        kind: PieceKind::Line,
        blank_before: false,
    }
}

/// `.routine name, tapes=N, alpha=(c1, c2, …)` — a column-0 directive
/// like `.func`, reconstructed from the parsed fields. The CST's
/// structurally-exact gate admits only canonically spelled values, so
/// the reconstruction changes no token's text; interior spacing
/// normalizes to the `, ` convention (whitespace-only, per this
/// printer's contract).
fn render_routine(r: &RoutineDirectiveCst) -> Piece {
    let alpha = r
        .alpha
        .iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(", ");
    Piece {
        code: format!(".routine {}, tapes={}, alpha=({})", r.name, r.tapes, alpha),
        comment: r.trailing.as_ref().map(|tc| tc.text.clone()),
        header_comment: None,
        kind: PieceKind::Structural,
        blank_before: false,
    }
}

/// `.section NAME` — a column-0 region marker, printed like `.func`
/// (single space before the name, trailing comment held back for
/// [`format_asm_with`]'s emit loop to pad to its group's comment column,
/// [`COMMENT_COL`] as a floor — like any other piece's).
fn render_section(s: &SectionCst) -> Piece {
    Piece {
        code: format!(".section {}", s.name),
        comment: s.trailing.as_ref().map(|tc| tc.text.clone()),
        header_comment: None,
        kind: PieceKind::Structural,
        blank_before: false,
    }
}

/// `.row [..]` / `.targets L1, ..` / `.target L` — the same
/// label/word/operands grid as an instruction line, with the directive
/// keyword standing in for the mnemonic. Operands print verbatim from
/// their CST tokens (a `.row` keeps its whole bracketed vector as one
/// token; `.targets` comma-joins its names), so interior spelling
/// survives. `.targets` is the one unbounded list of the three
/// (`.row`/`.target` are bounded by tape count and by taking a single
/// operand), so it is the only kind here that wraps.
fn render_table_directive(d: &TableDirectiveCst) -> Piece {
    let word = match d.kind {
        TableDirectiveKind::Row => ".row",
        TableDirectiveKind::Targets => ".targets",
        TableDirectiveKind::Target => ".target",
    };
    let wrap = matches!(d.kind, TableDirectiveKind::Targets);
    render_fields(
        &d.labels,
        Some((word, d.operands.as_slice())),
        &d.trailing,
        wrap,
    )
}

/// `.frame`/`.map`/`.exits` — the frame-descriptor directive family
/// (docs/formats.md (frame descriptors)), each printed on the same
/// label/word/operands grid as a table directive: `.frame` carries the
/// descriptor label; `.map`/`.exits` are unlabeled. The operand text is
/// reconstructed from the parsed fields (canonically spelled, so no token
/// text changes — mirrors [`render_routine`]); `->`/`=>` survive exactly
/// as authored (the CST records the one-way bit). `.exits`, like
/// `.targets`, grows with the program and wraps; `.frame`'s `tapes=(…)`
/// is bounded by tape count and never does. `.map` wraps too — its
/// unbounded part is the symbol-mapping pairs, which is why
/// [`frame_map_operands`] keeps `rmap=(…)`/`wmap=(…)` as separate
/// elements rather than one composed string: a trailing comma continues
/// a `.map` list wherever it falls (docs/formats.md (assembly text)), so
/// a break between clauses reparses exactly as one inside a clause would
/// — this printer takes the simpler of the two and keeps each
/// `rmap=(…)`/`wmap=(…)` group whole rather than splitting its pairs.
fn render_frame_directive(d: &FrameDirectiveCst) -> Piece {
    match d {
        FrameDirectiveCst::Header(h) => {
            let alpha = h
                .tapes
                .iter()
                .map(u32::to_string)
                .collect::<Vec<_>>()
                .join(", ");
            let operand = [OperandToken {
                text: format!("tapes=({alpha})"),
                span: h.span,
            }];
            render_fields(
                std::slice::from_ref(&h.label),
                Some((".frame", &operand)),
                &h.trailing,
                false,
            )
        }
        FrameDirectiveCst::Map(m) => {
            let operands = frame_map_operands(m);
            render_fields(&[], Some((".map", &operands)), &m.trailing, true)
        }
        FrameDirectiveCst::Exits(e) => {
            render_fields(&[], Some((".exits", &e.targets)), &e.trailing, true)
        }
    }
}

/// `<k>[, rmap=(…)][, wmap=(…)]` — the `.map` operand list, one element
/// per top-level clause rather than one composed string (see
/// [`render_frame_directive`] for why).
fn frame_map_operands(m: &FrameMapCst) -> Vec<OperandToken> {
    let mut operands = vec![OperandToken {
        text: m.k.to_string(),
        span: m.k_span,
    }];
    if let Some(pairs) = &m.rmap {
        operands.push(OperandToken {
            text: format!("rmap=({})", frame_pairs_text(pairs)),
            span: m.rmap_span.unwrap_or(m.span),
        });
    }
    if let Some(pairs) = &m.wmap {
        operands.push(OperandToken {
            text: format!("wmap=({})", frame_pairs_text(pairs)),
            span: m.wmap_span.unwrap_or(m.span),
        });
    }
    operands
}

/// A `(..)` pair list as `<from>-><to>` / `<from>=><to>`, comma-joined.
fn frame_pairs_text(pairs: &[FramePairCst]) -> String {
    pairs
        .iter()
        .map(|p| {
            let arrow = if p.one_way { "=>" } else { "->" };
            format!("{}{arrow}{}", p.from, p.to)
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// `.rept v, lo, hi` … `.endr`. The header and terminator normalize to
/// the grid (column-0 directives, like `.func`); the BODY prints VERBATIM
/// — every physical source line strictly between the header line and the
/// `.endr` line, exactly as written (macros as written), with only
/// trailing whitespace trimmed to honor the whole-file no-trailing-space
/// invariant. Body items are NOT re-shaped through the grid: a
/// substitution template such as `Linc{v}: nop` shapes labelless (the
/// `{` breaks the word), and grid-printing it would corrupt its spacing
/// and text. Recovering the body by physical-line range (`endr_span`
/// bounds it) also preserves body comments and blank lines, which carry
/// no line number of their own on a Comment item.
///
/// The block has two independently-anchored trailing comments (header
/// and `.endr`), and both are held back for the emit loop to pad — the
/// header's into [`Piece::header_comment`] (it is not on `code`'s last
/// line, so it cannot share [`Piece::comment`]'s slot), `.endr`'s into
/// [`Piece::comment`] like every other piece's. Both are padded to the
/// SAME per-group column [`comment_columns`] computes for this piece —
/// a `.rept` block is one group member, not two — and that computation
/// checks BOTH lines' own widths (not just `.endr`'s, the piece's last
/// line) precisely so the shared column is wide enough to hold each of
/// them without either overflowing it: the header is no longer pinned
/// to a fixed column of its own, and it is bounded by its own width
/// like everything else that carries a comment.
fn render_rept(r: &ReptCst, source: &str) -> Piece {
    // Header: reconstructed from the parsed bounds and normalized; its
    // trailing comment is held back rather than padded here.
    let header = format!(".rept {}, {}, {}", r.var, r.lo, r.hi);
    let mut code = String::new();
    code.push_str(header.trim_end());
    code.push('\n');

    // Body: source lines (1-based) in (header_line, endr_line), verbatim.
    let lines: Vec<&str> = source.lines().collect();
    let body_start = r.span.start.line as usize + 1;
    let body_end = r.endr_span.start.line as usize;
    for n in body_start..body_end {
        if let Some(text) = lines.get(n - 1) {
            code.push_str(text.trim_end());
            code.push('\n');
        }
    }

    // Terminator: `.endr`; its trailing comment is held back below.
    code.push_str(".endr");

    Piece {
        code,
        comment: r.endr_trailing.as_ref().map(|tc| tc.text.clone()),
        header_comment: r.trailing.as_ref().map(|tc| tc.text.clone()),
        kind: PieceKind::Rept,
        blank_before: false,
    }
}

/// Operand text verbatim from the CST's `OperandToken`s (never
/// retokenized/rewritten — leading zeros, sign, spelling all survive),
/// comma-joined (docs/formats.md (assembly text)). A trailing `[..]`
/// operand is the one exception: a declarative binding call
/// (`call name [binding]`) space-separates the target from the bracket,
/// so a leading operand before a final bracket is joined with a space,
/// not a comma (docs/formats.md (bound calls)).
fn join_operands(operands: &[OperandToken]) -> String {
    if let [lead @ .., last] = operands
        && !lead.is_empty()
        && last.text.starts_with('[')
    {
        let head = lead
            .iter()
            .map(|o| o.text.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        return format!("{head} {}", last.text);
    }
    operands
        .iter()
        .map(|o| o.text.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

/// Packs `texts` onto as few physical lines as fit within
/// [`LINE_WIDTH_LIMIT`] (docs/formats.md (assembly text)) — the
/// `.targets`/`.exits`/`.map` wrapping [`render_fields`] falls into once
/// the plain [`join_operands`] line would run past the budget. `start_col`
/// is the 0-based column the caller's line has already reached — where
/// the first element is about to land — and doubles as the indent every
/// continuation line reuses, so the whole list reads as one column of
/// elements under the first one. Greedy: an element joins the current
/// line whenever it (plus its separating comma, plus the one space that
/// would precede it) still fits; otherwise it starts a fresh line. A
/// break always falls right after a comma — never before one — so every
/// wrapped line but the last ends in `,`, which is exactly what continues
/// a `.targets`/`.exits`/`.map` list back into one logical item on the
/// next parse (docs/formats.md (assembly text)): reformatting the
/// wrapped output reproduces it unchanged.
///
/// A trailing EMPTY operand — the shape a `.targets aa,` / `bb,` join
/// with nothing after it leaves behind (docs/formats.md (assembly
/// text)) — carries no text of its own to place. It is skipped outright
/// rather than packed: the comma already printed after the real operand
/// before it is the whole story, and packing it would either strand a
/// bare continuation line holding nothing but a comma or leave a stray
/// trailing space before the line's own trim. Skipping it here still
/// gives the preceding real operand its comma, because the separator
/// decision below keys on this element's INDEX in `texts`, not on
/// whether it prints anything.
///
/// Takes plain strings rather than the CST's `OperandToken`s so
/// `disassembler.rs` — a sibling module with no CST and no source
/// positions to attach — can wrap its own emitted lists through this
/// exact packing instead of a second implementation of it. A `Span`
/// asserts where text came from in a source file; synthesized
/// disassembly has no such position, so fabricating one to reach this
/// function would be a lie in the type.
pub(super) fn wrap_operand_list(texts: &[&str], start_col: usize) -> String {
    let mut out = String::new();
    let mut col = start_col;
    for (i, text) in texts.iter().enumerate() {
        let is_last = i + 1 == texts.len();
        if text.is_empty() && is_last {
            continue;
        }
        let sep = if is_last { "" } else { "," };
        let piece_len = text.chars().count() + sep.len();
        if out.is_empty() {
            // The very first element lands right where the caller's
            // line already stands — no separator, no wrap decision.
        } else if col + 1 + piece_len > LINE_WIDTH_LIMIT {
            out.push('\n');
            out.push_str(&" ".repeat(start_col));
            col = start_col;
        } else {
            out.push(' ');
            col += 1;
        }
        out.push_str(text);
        out.push_str(sep);
        col += piece_len;
    }
    out
}

/// Advances `cur`/`col` to `target`: pads with spaces when there is
/// room, or inserts exactly one separating space when the cursor has
/// already reached or passed it (docs/formats.md (assembly text): "a
/// single space when a field overflows its stop").
fn pad_to(cur: &mut String, col: &mut usize, target: usize) {
    if *col < target {
        cur.push_str(&" ".repeat(target - *col));
        *col = target;
    } else {
        cur.push(' ');
        *col += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::asm::assembler::assemble;
    use crate::asm::disassembler::disassemble_object;
    use crate::asm::lexer::{AsmTokenKind, lex_line};
    use crate::asm::syntax::AsmCaps;
    use crate::asm::syntax::fixture::test_syntax;
    use crate::asm::syntax::{ArchSyntax, Flow, SyntaxEntry};
    use crate::diagnostics::Span;
    use crate::vm::OperandKind;

    // The `.pma` example from docs/formats.md (assembly text) — the
    // SAME constant `cst.rs`'s own doc-example test pins, reproduced
    // here as a `const` so this module's tests don't reach across a
    // sibling module's private `#[cfg(test)]` items.
    const DOC_EXAMPLE: &str = "\
.func goToEnd                   ; emits ent, defines symbol
L1:     rgt
        jm      L1              ; assembler picks jm.s automatically
        lft
        ret

.func main
        call    goToEnd         ; width decided at link time
        rgt
        wr      1               ; mark
        stp
";

    // -- Case 1: the docs example reprints byte-identically -----------

    #[test]
    fn case1_doc_example_is_a_fixed_point() {
        assert_eq!(format_asm(DOC_EXAMPLE).unwrap(), DOC_EXAMPLE);
    }

    // -- Case 2: scrambled whitespace formats TO the canonical text ---

    #[test]
    fn case2_scrambled_whitespace_formats_to_canonical() {
        // Same program, whitespace mangled: tight/loose spacing, tabs,
        // no grid alignment at all, and a spaced colon (`L1 :`) that
        // must normalize to `L1:` (a whitespace-only change — the
        // colon is a separate token either way, so the label name is
        // untouched).
        let scrambled = "\
.func goToEnd ; emits ent, defines symbol
L1 :  rgt
 jm L1 ; assembler picks jm.s automatically
lft
\tret

.func main
call goToEnd ; width decided at link time
rgt
   wr 1 ; mark
stp
";
        assert_eq!(format_asm(scrambled).unwrap(), DOC_EXAMPLE);
    }

    // -- Grid-stop overflow: single space, not padding to the next stop.
    // None of the 11 enumerated fixtures exercise this branch of
    // `pad_to` (every mnemonic/operand in them is short) — the brief's
    // "Printing rules (each is a test)" still covers it, so it gets its
    // own tests here.

    #[test]
    fn overflow_mnemonic_gets_one_space_before_operand() {
        // "verylongmnem" (12 chars) starts at col 8, ends at col 20 —
        // past OPERAND_COL (16) — so the operand gets exactly one
        // separating space instead of padding back to col 16.
        let src = ".func f\n        verylongmnem 1\n";
        assert_eq!(format_asm(src).unwrap(), src);
    }

    #[test]
    fn overflow_boundary_mnemonic_exactly_at_operand_col() {
        // "abcdefgh" (8 chars) starts at col 8, ends EXACTLY at col 16
        // (OPERAND_COL) — `pad_to`'s `<` is strict, so landing exactly
        // on the stop still takes the overflow (one-space) branch, not
        // the padding branch. Mnemonic-side mirror of the label's
        // 7-vs-8 boundary test above.
        let src = ".func f\n        abcdefgh 1\n";
        assert_eq!(format_asm(src).unwrap(), src);
    }

    #[test]
    fn overflow_operand_gets_one_space_before_trailing_comment() {
        // mnemonic "wr" ends at col 10, pads to col 16, then a 26-char
        // operand runs to col 42 — past COMMENT_COL (32) — so the
        // trailing comment gets one space, not padding back to 32.
        let operand = "a".repeat(26);
        let src = format!(".func f\n        wr      {operand} ; c\n");
        assert_eq!(format_asm(&src).unwrap(), src);
    }

    #[test]
    fn overflow_shapes_are_idempotent() {
        for src in [
            ".func f\n        verylongmnem 1\n".to_string(),
            ".func f\n        abcdefgh 1\n".to_string(),
            format!(".func f\n        wr      {} ; c\n", "a".repeat(26)),
        ] {
            let once = format_asm(&src).unwrap();
            let twice = format_asm(&once).unwrap();
            assert_eq!(twice, once, "not idempotent for {src:?}");
        }
    }

    // -- Case 3: a long label goes on its own line ---------------------

    #[test]
    fn case3_long_label_own_line_instruction_follows() {
        let src = ".func f\nverylongname:  nop\n";
        let expected = ".func f\nverylongname:\n        nop\n";
        assert_eq!(format_asm(src).unwrap(), expected);
    }

    #[test]
    fn case3_eight_char_field_boundary_is_own_line() {
        // "abcdefg:" is exactly 8 chars (7-letter name + colon) — the
        // brief's own stated boundary: 8+ is own-line, 7 or fewer
        // stays inline.
        let src = ".func f\nabcdefg: nop\n";
        let expected = ".func f\nabcdefg:\n        nop\n";
        assert_eq!(format_asm(src).unwrap(), expected);
    }

    #[test]
    fn case3_seven_char_field_stays_inline() {
        // "abcdef:" is 7 chars — the largest field that still fits.
        let src = ".func f\nabcdef: nop\n";
        let expected = ".func f\nabcdef: nop\n";
        assert_eq!(format_asm(src).unwrap(), expected);
    }

    // -- Case 4: multi-label lines -------------------------------------

    #[test]
    fn case4_non_last_label_own_line_last_stays_inline() {
        let src = ".func f\nA: B: nop\n";
        let expected = ".func f\nA:\nB:      nop\n";
        assert_eq!(format_asm(src).unwrap(), expected);
    }

    #[test]
    fn case4_multi_label_with_a_long_last_label() {
        // Every label goes own-line here: A: is non-last (position
        // rule), verylongname: is last but too long (length rule).
        let src = ".func f\nA: verylongname: nop\n";
        let expected = ".func f\nA:\nverylongname:\n        nop\n";
        assert_eq!(format_asm(src).unwrap(), expected);
    }

    #[test]
    fn label_only_line_short_label_no_trailing() {
        let src = ".func f\nL1:\n        nop\n";
        assert_eq!(format_asm(src).unwrap(), src);
    }

    #[test]
    fn label_only_line_with_trailing_comment() {
        // "A:" (2 chars) padded straight to COMMENT_COL (32).
        let src = format!(
            ".func f\nA:{}; c\n        nop\n",
            " ".repeat(COMMENT_COL - 2)
        );
        assert_eq!(format_asm(&src).unwrap(), src);
    }

    #[test]
    fn label_only_line_long_label_with_trailing_comment() {
        let src = ".func f\nverylongname:\n";
        let expected = ".func f\nverylongname:\n";
        assert_eq!(format_asm(src).unwrap(), expected);

        // Same, but with a trailing comment: the comment stays on the
        // LABEL's own line (padded from the field's end to col 32),
        // not on a bare continuation line. A bare continuation would
        // reparse as an own-line comment (no label on that physical
        // line, and no trailing comment left on the label's own line to
        // continue) and get re-indented to TOP_COL on a second pass —
        // an idempotence violation. See `case5_idempotent_over_every_fixture`'s
        // `"verylongname: ; note\n"` fixture for the pinned round-trip.
        let src_c = ".func f\nverylongname: ; note\n";
        let expected_c = format!(
            ".func f\nverylongname:{}; note\n",
            " ".repeat(COMMENT_COL - "verylongname:".chars().count())
        );
        assert_eq!(format_asm(src_c).unwrap(), expected_c);
    }

    // -- Case 5 + 6 fixtures (shared) ----------------------------------

    fn fixtures() -> Vec<&'static str> {
        vec![
            DOC_EXAMPLE,
            ".func f\nverylongname:  nop\n",
            ".func f\nA: B: nop\n",
            ".func f\nA: verylongname: nop\n",
            ".func f\n        wr      007, -1  ; leading zero survives\n",
            ".func f\n        bogus   1, 2\n", // unknown mnemonic
            ".func f local\n        nop\n\n\n\n.func g\n        ret\n", // blank-run collapse
            "; preamble\n.func f\n        nop\n; between f and g\n.func g\n        ret\n; trailing\n",
            ".func f\n        nop\n        ; inside f\n        ret\n",
            ".func f\n        verylongmnem 1\n", // mnemonic overflows into the operand column
            ".func f\n        abcdefgh 1\n",     // mnemonic ends exactly at the operand column
            ".func f\nverylongname: ; note\n",   // long label-only line with a trailing comment
            ".func f\nverylongname: short: nop ; note\n", // multi-label variant: long non-last label + trailing comment on the short last label's instruction line
        ]
    }

    // -- Case 5: idempotence over every fixture ------------------------

    #[test]
    fn case5_idempotent_over_every_fixture() {
        for src in fixtures() {
            let once = format_asm(src).unwrap();
            let twice = format_asm(&once).unwrap();
            assert_eq!(twice, once, "not idempotent for {src:?}");
        }
    }

    // -- Case 6: zero token changes ------------------------------------

    /// Flattens every physical line's tokens (in source order) into one
    /// `Vec<AsmTokenKind>`. `AsmTokenKind` carries no position — only
    /// kind + text (`Word`/`Number`/`Comment` hold their spelling) — so
    /// comparing the flattened sequences both drops columns AND
    /// compares comments by text, in one step. Input and output can
    /// have different LINE counts (a long label splits one source line
    /// into two printed lines) so this compares the flattened token
    /// stream, not line-by-line.
    fn flat_kinds(source: &str) -> Vec<AsmTokenKind> {
        source
            .lines()
            .enumerate()
            .flat_map(|(i, line)| lex_line(line, i as u32 + 1, AsmCaps::default()))
            .map(|t| t.kind)
            .collect()
    }

    #[test]
    fn case6_zero_token_changes_over_every_fixture() {
        for src in fixtures() {
            let out = format_asm(src).unwrap();
            assert_eq!(
                flat_kinds(src),
                flat_kinds(&out),
                "token stream changed for {src:?}\n---\n{out}"
            );
        }
    }

    #[test]
    fn case6_flatten_sanity_check_on_a_relabeled_line() {
        // Direct check on the case-4 shape: `A: B: nop` -> `A:\nB:      nop\n`
        // must flatten to the identical token-kind sequence.
        let input = "A: B: nop";
        let output = "A:\nB:      nop";
        assert_eq!(flat_kinds(input), flat_kinds(output));
    }

    // -- Case 7: leading-zero / signed operands survive verbatim -------

    #[test]
    fn case7_leading_zero_and_signed_operands_survive() {
        let src = ".func f\n        wr      007, -1\n";
        assert_eq!(format_asm(src).unwrap(), src);
    }

    // -- Case 8: the structural gate -----------------------------------

    #[test]
    fn case8_listing_shaped_line_is_a_raw_line_error() {
        let src = ".func f\n  0004:  21 05 00 00 00  call    0x0005 <goToEnd>\n";
        let err = format_asm(src).unwrap_err();
        assert_eq!(err.kind, AsmErrorKind::RawLine);
        assert_eq!(err.span, Span::new(2, 3, 2, 50));
    }

    #[test]
    fn case8_nothing_formats_when_any_line_is_raw() {
        // The gate is whole-file: even a perfectly good line elsewhere
        // does not partially format.
        let src = ".func f\n        nop\n<stray>\n";
        assert!(format_asm(src).is_err());
    }

    // -- Case 9: unknown mnemonics still format ------------------------

    #[test]
    fn case9_unknown_mnemonic_still_formats() {
        let src = ".func f\n        bogus   1, 2\n";
        assert_eq!(format_asm(src).unwrap(), src);
    }

    // -- Case 10: blank-run collapse + final newline -------------------

    #[test]
    fn case10_blank_run_collapses_to_one_and_ends_in_one_newline() {
        let src = ".func f local\n        nop\n\n\n\n.func g\n        ret\n";
        let expected = ".func f local\n        nop\n\n.func g\n        ret\n";
        let out = format_asm(src).unwrap();
        assert_eq!(out, expected);
        assert!(out.ends_with('\n') && !out.ends_with("\n\n"));
    }

    #[test]
    fn case10_no_leading_blank_lines() {
        let src = "\n\n.func f\n        nop\n";
        let out = format_asm(src).unwrap();
        assert!(!out.starts_with('\n'));
        assert_eq!(out, ".func f\n        nop\n");
    }

    #[test]
    fn case10_crlf_normalizes_to_lf() {
        let src = ".func f\r\n        nop\r\n";
        let out = format_asm(src).unwrap();
        assert_eq!(out, ".func f\n        nop\n");
        assert!(!out.contains('\r'));
    }

    #[test]
    fn empty_source_formats_to_empty() {
        assert_eq!(format_asm("").unwrap(), "");
    }

    #[test]
    fn blank_only_file_formats_to_empty() {
        // Real blank/whitespace-only lines, not the empty string: every
        // line tokenizes to nothing, so `parse_asm_cst` never produces an
        // item and the print loop never runs — same end result as `""`,
        // pinned here because it goes through a different code path
        // (repeated `pending_blank` folding in `parse_asm_cst`, not the
        // zero-line case).
        let src = "\n\n   \n\t\n";
        let out = format_asm(src).unwrap();
        assert_eq!(out, "");
        assert_eq!(format_asm(&out).unwrap(), out);
    }

    // -- Trailing whitespace on lines (item 4: pinned, not a renderer
    // change — the printer rebuilds every line from CST tokens using the
    // canonical column constants, so whitespace after the last real
    // token on a physical line was never captured into any token in the
    // first place and cannot survive into the output).

    #[test]
    fn trailing_whitespace_on_every_line_shape_formats_clean_and_idempotent() {
        // Same content as `case3_seven_char_field_stays_inline`'s and
        // `case1_doc_example_is_a_fixed_point`'s fixtures, but every
        // physical line — the `.func` header, a plain instruction line,
        // and an own-line comment — carries trailing spaces or a tab.
        // `abcdef: nop` above carries no trailing comment, so the
        // own-line comment is structural — column 0, not MNEMONIC_COL.
        let src = ".func f  \nabcdef: nop\t\n        ; note   \n        stop  \n";
        let expected = ".func f\nabcdef: nop\n; note\n        stop\n";
        let once = format_asm(src).unwrap();
        assert_eq!(once, expected);
        assert!(
            once.lines().all(|l| l == l.trim_end()),
            "trailing whitespace survived: {once:?}"
        );
        let twice = format_asm(&once).unwrap();
        assert_eq!(twice, once);
    }

    #[test]
    fn trailing_whitespace_after_a_trailing_comment_is_stripped() {
        // The comment token itself captures everything from `;` to the
        // end of the physical line (lexer.rs), so trailing whitespace
        // AFTER the comment text is part of the comment token's text —
        // `format_asm_with`'s final `line.trim_end()` per piece is what
        // drops it, not the lexer failing to capture it.
        let src = ".func f\n        wr      1 ; c   \n";
        let expected = ".func f\n        wr      1               ; c\n";
        assert_eq!(format_asm(src).unwrap(), expected);
    }

    // -- Comment-only file (item 5): no `Line` piece anywhere, so no
    // comment can continue a trailing comment above it — every own-line
    // comment is TOP_COL regardless of its original indentation
    // (`continues_a_trailing_comment` never finds a `Line` to return
    // true from) — pinned separately from the function-body/preamble
    // comment-placement tests above, none of which cover a file with
    // zero functions.

    #[test]
    fn comment_only_file_prints_every_comment_at_top_level_col_0() {
        let src = "; first\n    ; indented, but still top-level\n; last\n";
        let expected = "; first\n; indented, but still top-level\n; last\n";
        let once = format_asm(src).unwrap();
        assert_eq!(once, expected);
        let twice = format_asm(&once).unwrap();
        assert_eq!(twice, once);
    }

    // -- Own-line comment placement (unpinned by the 11 enumerated
    // cases, but part of the printing-rule list — a judgment call
    // documented on `own_line_comment_col`) --------------------------

    #[test]
    fn preamble_comment_before_the_first_func_is_col_0() {
        let src = "; preamble\n.func f\n        nop\n";
        assert_eq!(format_asm(src).unwrap(), src);
    }

    #[test]
    fn comment_inside_a_function_body_is_col_0() {
        // `nop` above carries no trailing comment, so this own-line
        // comment is structural, not a continuation — column 0.
        let src = ".func f\n        nop\n        ; note\n        ret\n";
        let expected = ".func f\n        nop\n; note\n        ret\n";
        assert_eq!(format_asm(src).unwrap(), expected);
    }

    #[test]
    fn comment_leading_into_the_next_func_is_col_0() {
        let src = ".func f\n        nop\n; about g\n.func g\n        ret\n";
        assert_eq!(format_asm(src).unwrap(), src);
    }

    #[test]
    fn trailing_comment_after_the_last_function_is_col_0() {
        // No upcoming `.func`, but that no longer matters — `nop`
        // above carries no trailing comment for this one to continue, so
        // it is structural, not attached to the body above it.
        let src = ".func f\n        nop\n        ; done\n";
        let expected = ".func f\n        nop\n; done\n";
        assert_eq!(format_asm(src).unwrap(), expected);
    }

    #[test]
    fn a_body_comment_prints_at_column_zero() {
        // MNEMONIC_COL leaves comment placement. A comment on its own
        // line inside a .func body is structural, not attached, because the
        // line above it carries no trailing comment to continue.
        let src = ".func f\n        nop\n        ; note\n        ret\n";
        let expected = ".func f\n        nop\n; note\n        ret\n";
        assert_eq!(format_asm(src).unwrap(), expected);
    }

    #[test]
    fn a_comment_run_continues_the_line_above_it() {
        // The line above carries a trailing comment, so the run is a
        // continuation and prints at that group's comment column.
        let src = ".func f\n        nop     ; first\n; continued\n        ret\n";
        let expected = format!(
            ".func f\n        nop{pad}; first\n{cont}; continued\n        ret\n",
            pad = " ".repeat(COMMENT_COL - "        nop".len()),
            cont = " ".repeat(COMMENT_COL),
        );
        assert_eq!(format_asm(src).unwrap(), expected);
    }

    // -- Case 11: `grid_line`'s long-label rule (unit test lives in
    // `disassembler.rs`'s own test module, next to `grid_line`; see
    // `grid_line_long_label_own_line` there) — this module only
    // exercises the effect through `format_asm`/`disassemble_object`
    // (below), since fmt has no direct call into `grid_line`.

    // -- Self-canonical: format_asm(dis x) == dis x --------------------

    #[test]
    fn self_canonical_over_disassembled_objects() {
        let syntax = test_syntax();
        let programs = [
            ".func f\nL0001:  nop\n        jmp.s   L0001\n        wr      1\n        call    g\n        stop\n",
            "\
.func f
START:  nop
        jmp     START
        wr      1, 2
        call    g
        call    missing
        stop
.func g
        wr      0
        ret
",
        ];
        for src in programs {
            let obj = assemble(&syntax, 0x7E, src, false).unwrap();
            let dis = disassemble_object(&syntax, &obj);
            assert_eq!(
                format_asm(&dis).as_deref(),
                Ok(dis.as_str()),
                "disassembly is not already canonical:\n{dis}"
            );
        }
    }

    // -- Task 5: sections, table directives, and `.rept` blocks --------
    // All exercise the opt-in surface, so they format under caps-on.

    fn caps_all() -> AsmCaps {
        AsmCaps {
            tables: true,
            rept: true,
            vectors: true,
            volatile: false,
        }
    }

    #[test]
    fn section_and_table_directives_normalize_to_the_grid() {
        // `.section` is a column-0 directive (single space, like `.func`);
        // `.row`/`.targets` are label + word + operands on the same grid
        // as an instruction line. A `.row`'s bracketed vector survives as
        // one verbatim operand; `.targets` comma-joins its names.
        let src = "\
.section    tables
T0:   .row  [1, 2]
      .row [1, *]
D0:.targets  A,B
.section code
";
        let expected = "\
.section tables
T0:     .row    [1, 2]
        .row    [1, *]
D0:     .targets A, B
.section code
";
        assert_eq!(format_asm_with(src, caps_all()).unwrap(), expected);
    }

    #[test]
    fn rept_header_and_endr_normalize_but_the_body_prints_verbatim() {
        // Header reconstructed + normalized (`.rept v,0,1` → grid spacing);
        // body line kept AS WRITTEN — odd interior spacing and its comment
        // survive, only trailing whitespace trimmed; `.endr` keeps its own
        // trailing comment, padded to the comment column.
        let src = ".rept v,0,1\n   Linc{v}:    nop      ; step   \n.endr  ; done\n";
        let expected = format!(
            ".rept v, 0, 1\n   Linc{{v}}:    nop      ; step\n.endr{}; done\n",
            " ".repeat(COMMENT_COL - ".endr".len())
        );
        assert_eq!(format_asm_with(src, caps_all()).unwrap(), expected);
    }

    #[test]
    fn rept_body_preserves_comment_and_blank_lines() {
        // Comment items carry no line number, so the body is recovered by
        // physical-line range (bounded by `endr_span`), not by walking
        // body items — a comment-only line and a blank line both survive.
        let src = ".rept v, 0, 0\n        ; a note\n\n        nop\n.endr\n";
        let out = format_asm_with(src, caps_all()).unwrap();
        assert_eq!(out, src);
        assert_eq!(format_asm_with(&out, caps_all()).unwrap(), out);
    }

    #[test]
    fn idempotent_over_all_mechanisms() {
        // Sections, grid-normalized table directives, a `.rept` block with
        // a verbatim (template) body, and a vector operand all in one file:
        // format(format(x)) == format(x).
        let src = "\
.section tables
Tm:     .row    [5, 6]
        .row    [*, *]
.rept v, 1, 2
Tr{v}:  .targets loop
.endr
.section code
.func main
        wr      [1, -, 2]
        tmatch  Tm
        stp
loop:   nop
";
        let once = format_asm_with(src, caps_all()).unwrap();
        let twice = format_asm_with(&once, caps_all()).unwrap();
        assert_eq!(twice, once, "not idempotent:\n{once}");
    }

    #[test]
    fn routine_directive_normalizes_and_is_idempotent() {
        // Tight interior spacing normalizes to the `, ` convention
        // (whitespace-only — the structurally-exact gate admits only
        // canonically spelled values, so no token text changes); the
        // 36-char directive overflows COMMENT_COL, so the trailing
        // comment gets the single overflow space.
        let src = ".routine main,tapes=2,alpha=(3,5) ; sig\n";
        let once = format_asm_with(src, caps_all()).unwrap();
        assert_eq!(once, ".routine main, tapes=2, alpha=(3, 5) ; sig\n");
        assert_eq!(format_asm_with(&once, caps_all()).unwrap(), once);
    }

    #[test]
    fn routine_directive_already_canonical_is_verbatim() {
        let src = ".routine main, tapes=2, alpha=(3, 5)\n.func main\n        stp\n";
        assert_eq!(format_asm_with(src, caps_all()).unwrap(), src);
    }

    #[test]
    fn frame_directives_normalize_to_the_grid_and_are_idempotent() {
        // `.frame` carries the descriptor label (label/word/operands grid);
        // `.map`/`.exits` are unlabeled. Tight spacing normalizes to the
        // `, ` / grid convention; `->`/`=>` survive exactly as authored.
        let src = "\
.section tables
F0:.frame  tapes=(3,0)
   .map 0,rmap=(2->1, 4=>0),wmap=(1->2)
   .exits Lodd,Leven
.section code
";
        let expected = "\
.section tables
F0:     .frame  tapes=(3, 0)
        .map    0, rmap=(2->1, 4=>0), wmap=(1->2)
        .exits  Lodd, Leven
.section code
";
        assert_eq!(format_asm_with(src, caps_all()).unwrap(), expected);
        // Idempotent.
        assert_eq!(format_asm_with(expected, caps_all()).unwrap(), expected);
    }

    #[test]
    fn default_caps_still_refuse_the_new_directive_lines() {
        // Under default caps `.section`/`.row` never shape as their nodes;
        // a `.row` line degrades to a Line with a Junk bracket → a Raw
        // node → the structural gate. This pins that PM-1 fmt is
        // unaffected by the opt-in surface.
        assert!(format_asm(".section tables\nT0: .row [1, 2]\n").is_err());
    }

    // -- List wrapping: `.targets`/`.exits`/`.map` ----------------------
    // A trailing comma continues one of these three lists onto the next
    // physical line (docs/formats.md (assembly text)); this is the write
    // side of that rule — a list the printer would otherwise emit past
    // the line-width budget wraps instead, using exactly that comma.

    #[test]
    fn a_long_targets_list_wraps_at_eighty_columns() {
        let names: Vec<String> = (0..40).map(|i| format!("label_number_{i:02}")).collect();
        let src = format!(".section tables\nD0:     .targets {}\n", names.join(", "));
        let out = format_asm_with(
            &src,
            AsmCaps {
                tables: true,
                ..AsmCaps::default()
            },
        )
        .unwrap();
        assert!(
            out.lines().all(|l| l.chars().count() <= 80),
            "every line fits: {:?}",
            out.lines().map(|l| l.chars().count()).max()
        );
        assert!(out.lines().count() > 2, "the list actually wrapped");
    }

    #[test]
    fn a_wrapped_targets_list_reparses_and_reformats_to_a_fixed_point() {
        // format(format(x)) == format(x): the wrapped output's own
        // trailing commas continue back into ONE logical `.targets` item
        // on the next parse (the CST's list-continuation fold), and the
        // printer must recompute the identical wrap from it.
        let names: Vec<String> = (0..40).map(|i| format!("label_number_{i:02}")).collect();
        let src = format!(".section tables\nD0:     .targets {}\n", names.join(", "));
        let once = format_asm_with(&src, caps_all()).unwrap();
        assert!(once.lines().count() > 2, "the fixture must actually wrap");
        let twice = format_asm_with(&once, caps_all()).unwrap();
        assert_eq!(twice, once, "not idempotent:\n{once}");
        // And the fold really did rejoin the wrapped lines into one item
        // — otherwise this would pass even if the wrap merely happened
        // to reproduce itself by coincidence rather than by the fold.
        let cst = parse_asm_cst_with(&once, caps_all());
        let targets = cst
            .items
            .iter()
            .filter(|i| matches!(i.kind, AsmItemKind::TableDirective(_)))
            .count();
        assert_eq!(
            targets, 1,
            "the wrapped lines must fold back into one directive"
        );
    }

    #[test]
    fn a_long_exits_list_wraps_at_eighty_columns() {
        let names: Vec<String> = (0..30).map(|i| format!("exit_label_{i:02}")).collect();
        let src = format!(
            ".section tables\nF0:     .frame tapes=(0)\n        .exits {}\n.section code\n",
            names.join(", ")
        );
        let out = format_asm_with(&src, caps_all()).unwrap();
        assert!(
            out.lines().all(|l| l.chars().count() <= 80),
            "every line fits: {:?}",
            out.lines().map(|l| l.chars().count()).max()
        );
        // `.section tables` + `.frame` + `.section code` never move; only
        // a genuinely wrapped `.exits` explains more than 4 lines total.
        assert!(out.lines().count() > 4, "the list actually wrapped:\n{out}");
        let twice = format_asm_with(&out, caps_all()).unwrap();
        assert_eq!(twice, out, "not idempotent:\n{out}");
    }

    #[test]
    fn a_long_map_clause_list_wraps_between_clauses() {
        // `.map`'s wrap point is between its `<k>`, `rmap=(…)`, `wmap=(…)`
        // clauses, not inside a clause's own pair list — see
        // `render_frame_directive`'s doc for why. Each clause here stays
        // short on its own; only the sum of the three earns the wrap.
        let src = "\
.section tables
F0:     .frame  tapes=(0, 1)
        .map    0, rmap=(1->2, 3->4, 5->6, 7->8), wmap=(1->2, 3->4, 5->6, 7->8, 9->10)
.section code
";
        let out = format_asm_with(src, caps_all()).unwrap();
        assert!(
            out.lines().all(|l| l.chars().count() <= 80),
            "every line fits: {:?}",
            out.lines().map(|l| l.chars().count()).max()
        );
        assert!(
            out.lines().count() > src.lines().count(),
            "the .map list actually wrapped:\n{out}"
        );
        // Regression guard: the split must fall BETWEEN clauses, never
        // inside one. Today the printer cannot do otherwise —
        // `frame_map_operands` hands `render_fields` three atomic
        // elements, so there is nowhere mid-`rmap=(…)` to break — but
        // the shared CST fold is genuinely nesting-blind (a flat "is the
        // line's last token a comma" check, no paren awareness), so a
        // future change that flattened pairs into operands could move
        // the split point silently. Every continuation line must open a
        // new clause.
        let map_lines: Vec<&str> = out
            .lines()
            .skip_while(|l| !l.contains(".map"))
            .take_while(|l| !l.trim_start().starts_with(".section"))
            .collect();
        assert!(
            map_lines.len() > 1,
            "the .map item itself must span more than one line:\n{out}"
        );
        for line in &map_lines[1..] {
            let t = line.trim_start();
            assert!(
                t.starts_with("rmap=")
                    || t.starts_with("wmap=")
                    || t.chars().next().is_some_and(|c| c.is_ascii_digit()),
                "a .map continuation line must open a new clause, not \
                 split inside one: {line:?}\n{out}"
            );
        }
        // The wrap must still be a fixed point, folding back into one
        // `.map` item rather than degrading into separate lines.
        let twice = format_asm_with(&out, caps_all()).unwrap();
        assert_eq!(twice, out, "not idempotent:\n{out}");
        let cst = parse_asm_cst_with(&out, caps_all());
        let maps = cst
            .items
            .iter()
            .filter(|i| {
                matches!(
                    i.kind,
                    AsmItemKind::FrameDirective(FrameDirectiveCst::Map(_))
                )
            })
            .count();
        assert_eq!(maps, 1, "the wrapped lines must fold back into one .map");
    }

    #[test]
    fn a_dangling_trailing_comma_on_one_line_keeps_the_bare_comma() {
        // `.targets aa, bb,` on ONE physical line — short enough that
        // wrapping never triggers — already carries a trailing EMPTY
        // operand (`operand_region`'s general rule for any trailing
        // comma, continuation or not). This pins the untouched
        // `join_operands` path's decision as the baseline the wrapped
        // path (below) must match: a bare dangling comma, no stray
        // space.
        let src = ".section tables\nD0:     .targets aa, bb,\n";
        assert_eq!(format_asm_with(src, caps_all()).unwrap(), src);
    }

    #[test]
    fn a_wrapped_list_with_a_trailing_empty_operand_keeps_a_bare_dangling_comma() {
        // The shape a `.targets aa,` / `bb,` continuation with NOTHING
        // after it leaves behind (docs/formats.md (assembly text)): the
        // operand list ends in one empty entry. Forcing this shape at
        // width proves `wrap_operand_list` does not turn that empty tail
        // into a stray `, ` or a bare continuation line holding nothing
        // but a comma — it must match the one-line baseline above.
        let names: Vec<String> = (0..40).map(|i| format!("label_number_{i:02}")).collect();
        let src = format!(".section tables\nD0:     .targets {},\n", names.join(", "));
        let out = format_asm_with(&src, caps_all()).unwrap();
        assert!(
            out.lines().all(|l| l.chars().count() <= 80),
            "every line fits: {out:?}"
        );
        assert!(
            out.lines()
                .all(|l| !l.ends_with(' ') && !l.trim().is_empty()),
            "no wrapped line is blank or carries trailing whitespace:\n{out}"
        );
        assert!(
            out.trim_end().ends_with(','),
            "the empty tail's dangling comma survives:\n{out}"
        );
        let twice = format_asm_with(&out, caps_all()).unwrap();
        assert_eq!(twice, out, "not idempotent:\n{out}");
        let cst = parse_asm_cst_with(&out, caps_all());
        let targets: Vec<_> = cst
            .items
            .iter()
            .filter_map(|i| match &i.kind {
                AsmItemKind::TableDirective(d) => Some(d),
                _ => None,
            })
            .collect();
        assert_eq!(targets.len(), 1, "the fold must still see one directive");
        assert_eq!(
            targets[0].operands.last().map(|o| o.text.as_str()),
            Some(""),
            "the trailing empty operand survives the round trip"
        );
    }

    #[test]
    fn a_wrapped_list_still_carries_its_trailing_comment_on_the_last_line() {
        // The wrap decision reads only the code (`join_operands`'s
        // length), so a trailing comment cannot feed back into where the
        // list breaks — but the comment itself still has to land
        // somewhere once the list is multi-line. `comment_columns` reads
        // a piece's width from `p.code`'s LAST line
        // (`rsplit('\n').next()`), so the comment must pad relative to
        // the wrapped list's final line, not its first.
        let names: Vec<String> = (0..40).map(|i| format!("label_number_{i:02}")).collect();
        let src = format!(
            ".section tables\nD0:     .targets {} ; the dispatch table\n",
            names.join(", ")
        );
        let out = format_asm_with(&src, caps_all()).unwrap();
        assert!(
            out.lines().all(|l| l.chars().count() <= 80),
            "every code line fits: {out:?}"
        );
        let commented: Vec<&str> = out.lines().filter(|l| l.contains(';')).collect();
        assert_eq!(
            commented.len(),
            1,
            "exactly one physical line carries the comment:\n{out}"
        );
        let targets_lines: Vec<&str> = out
            .lines()
            .skip_while(|l| !l.contains(".targets"))
            .collect();
        assert_eq!(
            commented[0],
            *targets_lines.last().unwrap(),
            "the comment lands on the directive's LAST wrapped line:\n{out}"
        );
        assert!(
            targets_lines.len() > 1,
            "the fixture must actually wrap:\n{out}"
        );
        // Fixed point, and the reparse still folds to one directive whose
        // trailing comment survived the round trip.
        let twice = format_asm_with(&out, caps_all()).unwrap();
        assert_eq!(twice, out, "not idempotent:\n{out}");
        let cst = parse_asm_cst_with(&out, caps_all());
        let targets: Vec<_> = cst
            .items
            .iter()
            .filter_map(|i| match &i.kind {
                AsmItemKind::TableDirective(d) => Some(d),
                _ => None,
            })
            .collect();
        assert_eq!(targets.len(), 1, "the fold must still see one directive");
        assert_eq!(
            targets[0].trailing.as_ref().map(|tc| tc.text.as_str()),
            Some("; the dispatch table")
        );
    }

    // -- List wrapping: assembler round trip ----------------------------
    // Everything above proves the wrapped TEXT is well-shaped; none of it
    // proves the assembler actually accepts it. This closes that gap: the
    // grammar's continuation fold is a flat "does the line end in a
    // comma" check with no semantic awareness of what a valid
    // `.targets`/`.exits`/`.map` list looks like, so a wrap emitted in a
    // shape the grammar declines is exactly the defect this printer
    // could introduce without a test ever calling `assemble`.

    /// Neutral fake dialect (per-file local helper, mirroring
    /// `assembler.rs`'s and `disassembler.rs`'s own `fake_syntax`, since
    /// there is no shared test-support module): `tdispatch` references a
    /// `.targets` table (TableRef, Stop — a dispatch has no static
    /// successor); `fcall` is a framed call (FramedCall, Call flow) for
    /// `.frame`/`.map`/`.exits`; plus `nop`/`stp`.
    fn fake_syntax() -> ArchSyntax {
        use Flow::{Call, FallThrough as FT, Stop};
        ArchSyntax {
            entries: vec![
                SyntaxEntry {
                    opcode: 0x01,
                    mnemonic: "nop",
                    operand: OperandKind::None,
                    flow: FT,
                },
                SyntaxEntry {
                    opcode: 0x02,
                    mnemonic: "stp",
                    operand: OperandKind::None,
                    flow: Stop,
                },
                SyntaxEntry {
                    opcode: 0x12,
                    mnemonic: "tdispatch",
                    operand: OperandKind::TableRef,
                    flow: Stop,
                },
                SyntaxEntry {
                    opcode: 0x14,
                    mnemonic: "fcall",
                    operand: OperandKind::FramedCall,
                    flow: Call,
                },
                SyntaxEntry {
                    opcode: 0x0E,
                    mnemonic: "ent",
                    operand: OperandKind::None,
                    flow: FT,
                },
            ],
            relax_pairs: vec![],
            entry_opcode: 0x0E,
            break_opcode: None,
            trap_opcode: None,
            caps: caps_all(),
        }
    }

    fn asm_fake(src: &str) -> crate::formats::object::ObjectFile {
        assemble(&fake_syntax(), 0x7E, src, false).unwrap()
    }

    #[test]
    fn a_wrapped_targets_list_assembles_to_the_same_object_as_unwrapped() {
        let names: Vec<String> = (0..40).map(|i| format!("A{i:02}")).collect();
        let code: String = names.iter().map(|n| format!("{n}: stp\n")).collect();
        let src = format!(
            ".section tables\nD0: .targets {}\n.section code\n.func main\n    tdispatch D0\n{code}",
            names.join(", ")
        );
        let wrapped = format_asm_with(&src, caps_all()).unwrap();
        assert!(
            wrapped.lines().count() > src.lines().count(),
            "the fixture must actually wrap:\n{wrapped}"
        );
        assert_eq!(
            asm_fake(&wrapped),
            asm_fake(&src),
            "wrapped .targets must assemble to the identical object"
        );
    }

    #[test]
    fn a_wrapped_exits_list_assembles_to_the_same_object_as_unwrapped() {
        let names: Vec<String> = (0..30).map(|i| format!("E{i:02}")).collect();
        let code: String = names.iter().map(|n| format!("{n}: stp\n")).collect();
        let src = format!(
            ".section tables\nF0: .frame tapes=(0)\n    .exits {}\n.section code\n.func main\n    fcall helper, F0\n{code}.func helper\n    stp\n",
            names.join(", ")
        );
        let wrapped = format_asm_with(&src, caps_all()).unwrap();
        assert!(
            wrapped.lines().count() > src.lines().count(),
            "the fixture must actually wrap:\n{wrapped}"
        );
        assert_eq!(
            asm_fake(&wrapped),
            asm_fake(&src),
            "wrapped .exits must assemble to the identical object"
        );
    }

    #[test]
    fn a_wrapped_map_clause_list_assembles_to_the_same_object_as_unwrapped() {
        let src = "\
.section tables
F0: .frame tapes=(0, 1)
    .map 0, rmap=(1->2, 3->4, 5->6, 7->8), wmap=(1->2, 3->4, 5->6, 7->8, 9->10)
.section code
.func main
    fcall helper, F0
    stp
.func helper
    stp
";
        let wrapped = format_asm_with(src, caps_all()).unwrap();
        assert!(
            wrapped.lines().count() > src.lines().count(),
            "the fixture must actually wrap:\n{wrapped}"
        );
        assert_eq!(
            asm_fake(&wrapped),
            asm_fake(src),
            "wrapped .map must assemble to the identical object"
        );
    }

    // -- Task 3: group-wide trailing-comment column -------------------

    #[test]
    fn a_group_widens_past_the_floor_for_its_widest_member() {
        // column = max(COMMENT_COL, widest code width in group + 1).
        // "        .targets aaaaaaaaaaaaaaaaaaaaaaaaa" is 42 chars: 8
        // (indent) + 8 (".targets", which lands EXACTLY on OPERAND_COL) +
        // 1 (the boundary-overflow separator `pad_to` gives a field that
        // lands exactly on a stop — the same rule `overflow_boundary_
        // mnemonic_exactly_at_operand_col` pins) + 25 (the operand). So
        // the group aligns at 43 and the narrow line follows it there.
        let src =
            ".func f\n        nop     ; short\n        .targets aaaaaaaaaaaaaaaaaaaaaaaaa ; wide\n";
        let out = format_asm_with(
            src,
            AsmCaps {
                tables: true,
                ..AsmCaps::default()
            },
        )
        .unwrap();
        let cols: Vec<usize> = out
            .lines()
            .filter(|l| l.contains(';'))
            .map(|l| l.find(';').unwrap())
            .collect();
        assert_eq!(cols, vec![43, 43], "both members align at the group column");
    }

    #[test]
    fn a_blank_line_starts_a_new_group() {
        // The `.targets` line's group column is 43, not 42 — see the
        // width breakdown in `a_group_widens_past_the_floor_for_its_
        // widest_member` above.
        let src =
            ".func f\n        nop     ; a\n\n        .targets aaaaaaaaaaaaaaaaaaaaaaaaa ; b\n";
        let out = format_asm_with(
            src,
            AsmCaps {
                tables: true,
                ..AsmCaps::default()
            },
        )
        .unwrap();
        let cols: Vec<usize> = out
            .lines()
            .filter(|l| l.contains(';'))
            .map(|l| l.find(';').unwrap())
            .collect();
        assert_eq!(
            cols,
            vec![32, 43],
            "the blank line splits them into two groups"
        );
    }

    #[test]
    fn an_uncommented_line_contributes_no_width() {
        let src = ".func f\n        nop     ; a\n        .targets aaaaaaaaaaaaaaaaaaaaaaaaa\n        ret     ; b\n";
        let out = format_asm_with(
            src,
            AsmCaps {
                tables: true,
                ..AsmCaps::default()
            },
        )
        .unwrap();
        let cols: Vec<usize> = out
            .lines()
            .filter(|l| l.contains(';'))
            .map(|l| l.find(';').unwrap())
            .collect();
        assert_eq!(
            cols,
            vec![32, 32],
            "the long uncommented line does not widen the group"
        );
    }

    #[test]
    fn the_group_column_is_never_capped_by_line_width() {
        // Unbounded — a group's column is never capped by the 80-column
        // limit. `line-too-long` (the arch-agnostic assembly rule) is
        // what reports an overlong result here. Asserting merely that
        // SOME line in the output exceeds 80 columns pins nothing: this
        // fixture's `.targets` line is already 87 characters of CODE
        // before any comment, so that would hold even under a fixed
        // COMMENT_COL. The real claim is that the NARROW `nop` line's
        // comment column follows the group past 80 rather than staying
        // at the 32-column floor — checked directly below.
        let wide = "a".repeat(70);
        let src = format!(".func f\n        nop     ; a\n        .targets {wide} ; b\n");
        let out = format_asm_with(
            &src,
            AsmCaps {
                tables: true,
                ..AsmCaps::default()
            },
        )
        .unwrap();
        let narrow_col = out
            .lines()
            .find(|l| l.contains("nop"))
            .unwrap()
            .find(';')
            .unwrap();
        assert_eq!(
            narrow_col, 88,
            "the narrow line's comment column is pulled past the 80-column \
             limit by its wide group-mate, rather than capping at the floor"
        );
    }

    #[test]
    fn a_rept_block_ends_a_group_so_width_does_not_leak_across_it() {
        // Isolation, not just "contributes no width": a `.rept` block
        // must also stop a WIDE line on its far side from dragging a
        // NARROW line on its near side into the same group. Reverting
        // the `PieceKind::Rept` arm from `comment_columns`'s `ends`
        // match merges all three pieces into one group and widens
        // "short"'s column to match "wide"'s — this assertion then
        // fails (verified by hand: reverting that arm changes the
        // first element of `cols` from 32 to 57).
        let wide = "a".repeat(40);
        let src = format!(
            ".func f\n        nop     ; short\n.rept v, 0, 0\n        nop\n.endr\n        wr      {wide} ; wide\n"
        );
        let out = format_asm_with(
            &src,
            AsmCaps {
                rept: true,
                ..AsmCaps::default()
            },
        )
        .unwrap();
        let cols: Vec<usize> = out
            .lines()
            .filter(|l| l.contains(';'))
            .map(|l| l.find(';').unwrap())
            .collect();
        assert_eq!(
            cols,
            vec![32, 57],
            "the .rept block keeps the narrow line at the floor, isolated \
             from the wide line on its far side"
        );
    }

    #[test]
    fn a_standalone_own_line_comment_ends_a_group_so_width_does_not_leak_across_it() {
        // Same isolation property as the `.rept` test above, for the
        // other `ends` boundary: a non-continuing own-line comment (the
        // "structural" case — nothing above it, an uncommented `ret`
        // here, carries a trailing comment to continue) must stop a wide
        // line after it from dragging a narrow line before it into the
        // same group. Reverting the `PieceKind::Comment` arm from
        // `comment_columns`'s `ends` match merges all three pieces (plus
        // the uncommented line and the comment itself) into one group
        // and widens "short"'s column to match "wide"'s — verified by
        // hand: reverting that arm changes `short_col` from 32 to 57.
        let wide = "a".repeat(40);
        let src = format!(
            ".func f\n        nop     ; short\n        ret\n; standalone\n        wr      {wide} ; wide\n"
        );
        let out = format_asm(&src).unwrap();
        let short_col = out
            .lines()
            .find(|l| l.contains("short"))
            .unwrap()
            .find(';')
            .unwrap();
        let standalone_col = out
            .lines()
            .find(|l| l.contains("standalone"))
            .unwrap()
            .find(';')
            .unwrap();
        let wide_col = out
            .lines()
            .find(|l| l.contains("wide"))
            .unwrap()
            .find(';')
            .unwrap();
        assert_eq!(short_col, 32, "the narrow line stays at the floor");
        assert_eq!(standalone_col, 0, "a non-continuing comment is structural");
        assert_eq!(wide_col, 57, "the wide line widens only its own group");
    }

    // -- Whole-branch review corrections ----------------------

    #[test]
    fn a_commented_structural_directive_widens_its_group() {
        // A `.section`/`.func`/`.routine` line that carries a
        // trailing comment must count toward its group's width like any
        // other commented piece, or it strands itself ragged against its
        // own group's narrower members.
        let src = ".func aFunctionWithAnExtremelyLongNameHere ; what it does\n        rd      ; read\n        stp     ; done\n";
        let out = format_asm(src).unwrap();
        let cols: Vec<usize> = out
            .lines()
            .filter(|l| l.contains(';'))
            .map(|l| l.find(';').unwrap())
            .collect();
        assert!(
            cols.windows(2).all(|w| w[0] == w[1]),
            "all three share one column, got {cols:?}"
        );
    }

    #[test]
    fn a_rept_header_comment_shares_its_group_column() {
        // The `.rept` header's trailing comment must go through the
        // same group-column mechanism as `.endr`'s and every other
        // comment, not a fixed column of its own. A wide line sharing
        // the block's group forces that group past the floor; the
        // header, the `.endr`, and the wide line must all land together.
        let wide = "a".repeat(40);
        let src = format!(
            ".rept v, 0, 0 ; header\n        nop\n.endr ; footer\n        wr      {wide} ; wide\n"
        );
        let out = format_asm_with(
            &src,
            AsmCaps {
                rept: true,
                ..AsmCaps::default()
            },
        )
        .unwrap();
        let cols: Vec<usize> = out
            .lines()
            .filter(|l| l.contains(';'))
            .map(|l| l.find(';').unwrap())
            .collect();
        assert!(
            cols.windows(2).all(|w| w[0] == w[1]),
            "header, footer, and the wide line share one column, got {cols:?}"
        );
    }

    #[test]
    fn a_rept_header_alone_widens_its_group() {
        // The header's OWN code width must feed the
        // group's column even with no other wide member in the group —
        // `comment_columns` samples a piece's LAST line by default,
        // which for a `.rept` piece is always `.endr`; without also
        // checking the header line, a long header would get a target
        // column too narrow for its own code and fall back to a single
        // overflow space, landing past the column the SHORT `stp` line
        // shares the group with — misaligned even though both are
        // nominally at "the group's comment column".
        let long_var = "aVeryLongVariableNameThatIsQuiteWide";
        let src = format!(
            ".rept {long_var}, 0, 0 ; header\n        nop\n.endr ; footer\n        stp     ; short\n"
        );
        let out = format_asm_with(
            &src,
            AsmCaps {
                rept: true,
                ..AsmCaps::default()
            },
        )
        .unwrap();
        let cols: Vec<usize> = out
            .lines()
            .filter(|l| l.contains(';'))
            .map(|l| l.find(';').unwrap())
            .collect();
        assert!(
            cols.windows(2).all(|w| w[0] == w[1]),
            "header, footer, and the short line all share one column, got {cols:?}"
        );
    }

    #[test]
    fn a_rept_header_alone_widens_its_group_when_endr_has_no_comment() {
        // Same property as the test above, with `.endr` carrying no
        // comment of its own: before this fix a `.rept` piece with
        // `comment == None` was excluded from the width scan entirely
        // (the scan only ever looked at `Piece::comment`), so nothing
        // bounded the header even though `header_comment` carries one.
        let long_var = "aVeryLongVariableNameThatIsQuiteWide";
        let src = format!(
            ".rept {long_var}, 0, 0 ; header\n        nop\n.endr\n        stp     ; short\n"
        );
        let out = format_asm_with(
            &src,
            AsmCaps {
                rept: true,
                ..AsmCaps::default()
            },
        )
        .unwrap();
        let header_col = out
            .lines()
            .find(|l| l.contains("header"))
            .unwrap()
            .find(';')
            .unwrap();
        let short_col = out
            .lines()
            .find(|l| l.contains("short"))
            .unwrap()
            .find(';')
            .unwrap();
        assert_eq!(
            header_col, short_col,
            "the short line must widen to match the header's own width, \
             got header={header_col} short={short_col}"
        );
    }
}
