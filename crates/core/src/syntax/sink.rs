//! The language-agnostic green-emission pair every toolchain parser
//! drives (docs/core.md (syntax trees)): a source-layout pass that
//! reconstructs verbatim per-token text and the trivia between tokens,
//! and a [`GreenSink`] that feeds a [`TreeBuilder`] from that schedule.
//! Both were byte-identical twins in the two arch crates before being
//! hoisted here; kinds stay an opaque per-language space (`K: Copy +
//! Into<SyntaxKind>`), and everything language-specific — which token
//! kinds are trivia, how a token's end is derived, what whitespace's
//! kind is — arrives through [`LayoutToken`] facts the crate-side
//! adapter builds from its own lexer's output. Core interprets none of
//! it, proven by the fake-kind tests below (the same discipline the VM
//! core keeps with its fake arch).

use std::rc::Rc;

use super::builder::{Checkpoint, TreeBuilder};
use super::green::{GreenNode, SyntaxKind};

/// One significant token's verbatim text plus the trivia pieces
/// (whitespace runs and comments) between the previous significant
/// token and this one, in source order. The concatenation of every
/// entry's pieces is the source, byte for byte — the green tree's
/// lossless law starts here.
pub struct SigLayout<K> {
    /// Verbatim source text of this significant token ("" for Eof).
    pub text: String,
    /// Trivia pieces before this token, in source order.
    pub trivia_before: Vec<(K, String)>,
}

/// How a token's end byte is derived from its start — the one fact the
/// generic pass cannot know per kind.
#[derive(Clone, Copy)]
pub enum EndRule {
    /// Start + this many source CHARACTERS (the ordinary rule; the
    /// count is the lexer's own `len`).
    Chars(u32),
    /// The token runs to the end of its source line (doc/attention
    /// lines, whose payload is normalized so no char count exists).
    ToLineEnd,
    /// The token is empty at its start position (Eof).
    AtStart,
}

/// Whether a token lands in the significant-entry stream or in the
/// trivia schedule, and — for trivia — under which kind it reprints.
#[derive(Clone, Copy)]
pub enum TokenClass<K> {
    Significant,
    Trivia(K),
}

/// One token's layout facts, built by the crate-side adapter from its
/// own lexer's token. Positions are the lexer's 1-based line and
/// 1-based char-counted column, trusted; ends are derived per
/// [`EndRule`] and validated by the invariant that everything between
/// two tokens is whitespace.
pub struct LayoutToken<K> {
    pub line: u32,
    pub col: u32,
    pub end: EndRule,
    pub class: TokenClass<K>,
}

/// Byte offset of each (line, col) token start, computed by one pass
/// over the source tracking 1-based line and char column.
fn start_offsets<K>(source: &str, tokens: &[LayoutToken<K>]) -> Vec<usize> {
    let mut offsets = Vec::with_capacity(tokens.len());
    let mut ti = 0;
    let mut line: u32 = 1;
    let mut col: u32 = 1;
    for (byte, ch) in source.char_indices() {
        while ti < tokens.len() && tokens[ti].line == line && tokens[ti].col == col {
            offsets.push(byte);
            ti += 1;
        }
        if ch == '\n' {
            line += 1;
            col = 1;
        } else {
            col += 1;
        }
    }
    // Eof (and any token starting exactly at end-of-text). Only an
    // empty-at-start token may still be unplaced here — anything else
    // claims characters the source does not have.
    while ti < tokens.len() {
        assert!(
            matches!(tokens[ti].end, EndRule::AtStart),
            "unplaced non-empty token at {}:{}",
            tokens[ti].line,
            tokens[ti].col
        );
        offsets.push(source.len());
        ti += 1;
    }
    offsets
}

/// End byte of a token per its [`EndRule`].
fn end_offset<K>(source: &str, token: &LayoutToken<K>, start: usize) -> usize {
    match token.end {
        EndRule::ToLineEnd => source[start..]
            .find('\n')
            .map(|nl| start + nl)
            .unwrap_or(source.len()),
        EndRule::AtStart => start,
        EndRule::Chars(len) => {
            let mut it = source[start..].char_indices();
            for _ in 0..len {
                it.next();
            }
            it.next().map(|(o, _)| start + o).unwrap_or(source.len())
        }
    }
}

/// The layout pass: verbatim per-token text and the trivia between
/// tokens, reconstructed from the token facts plus the source text.
/// `whitespace` is the kind an inter-token gap reprints under. Panics
/// (by design — these are lexer-contract violations, not user errors)
/// when a gap between tokens holds non-whitespace, when trivia dangles
/// after the last significant token, or when the pieces do not cover
/// the source exactly.
pub fn layout<K: Copy>(
    source: &str,
    tokens: &[LayoutToken<K>],
    whitespace: K,
) -> Vec<SigLayout<K>> {
    let starts = start_offsets(source, tokens);
    let mut entries = Vec::new();
    let mut pending: Vec<(K, String)> = Vec::new();
    let mut cursor = 0usize;
    for (i, t) in tokens.iter().enumerate() {
        let start = starts[i];
        let gap = &source[cursor..start];
        assert!(
            gap.chars().all(char::is_whitespace),
            "non-whitespace between tokens at byte {cursor}: {gap:?}"
        );
        if !gap.is_empty() {
            pending.push((whitespace, gap.to_string()));
        }
        let end = end_offset(source, t, start);
        let text = &source[start..end];
        cursor = end;
        match t.class {
            TokenClass::Trivia(kind) => {
                pending.push((kind, text.to_string()));
            }
            TokenClass::Significant => {
                entries.push(SigLayout {
                    text: text.to_string(),
                    trivia_before: std::mem::take(&mut pending),
                });
            }
        }
    }
    assert!(
        pending.is_empty(),
        "trivia after the last significant token"
    );
    assert_eq!(cursor, source.len(), "source tail not covered");
    entries
}

/// A [`TreeBuilder`] fed from a [`SigLayout`] schedule. The parser
/// stays the single owner of grammar decisions — the sink only mirrors
/// token consumption and node boundaries, so the green tree and the
/// parser's errors can never disagree.
pub struct GreenSink<K> {
    builder: TreeBuilder,
    entries: Vec<SigLayout<K>>,
    /// First significant-token index whose trivia is not yet emitted.
    flushed_upto: usize,
    /// Currently open nodes — see [`open_depth`](Self::open_depth).
    open: usize,
}

impl<K: Copy + Into<SyntaxKind>> GreenSink<K> {
    pub fn new(entries: Vec<SigLayout<K>>) -> GreenSink<K> {
        GreenSink {
            builder: TreeBuilder::new(),
            entries,
            flushed_upto: 0,
            open: 0,
        }
    }

    /// Emit `trivia_before[pos]` into the currently open node, once.
    /// Idempotent — a later call at the same or an already-flushed
    /// `pos` is a no-op, so callers needn't track whether some other
    /// helper already flushed this position.
    pub fn flush(&mut self, pos: usize) {
        if self.flushed_upto > pos {
            return;
        }
        debug_assert_eq!(self.flushed_upto, pos, "trivia flushed out of order");
        for (kind, text) in std::mem::take(&mut self.entries[pos].trivia_before) {
            self.builder.token(kind.into(), text);
        }
        self.flushed_upto = pos + 1;
    }

    /// Flush, then emit significant token `pos` verbatim.
    pub fn token(&mut self, pos: usize, kind: K) {
        self.flush(pos);
        let text = std::mem::take(&mut self.entries[pos].text);
        debug_assert!(!text.is_empty(), "significant token {pos} emitted twice");
        self.builder.token(kind.into(), text);
    }

    pub fn start(&mut self, kind: K) {
        self.builder.start_node(kind.into());
        self.open += 1;
    }

    pub fn finish(&mut self) {
        debug_assert!(self.open > 0, "finish without a matching start");
        self.builder.finish_node();
        self.open -= 1;
    }

    pub fn checkpoint(&self) -> Checkpoint {
        self.builder.checkpoint()
    }

    pub fn start_at(&mut self, cp: Checkpoint, kind: K) {
        self.builder.start_node_at(cp, kind.into());
        self.open += 1;
    }

    /// How many nodes are currently open — the recovery wrapper records
    /// this at a loop seam and unwinds back to it with
    /// [`finish_to`](Self::finish_to) when an item parse fails midway.
    pub fn open_depth(&self) -> usize {
        self.open
    }

    /// Close open nodes until exactly `depth` remain. The partially
    /// built nodes close as-is — their children stay in the tree (the
    /// lossless law is untouched); the caller then retro-wraps the
    /// whole region into its error node via a checkpoint taken at the
    /// seam.
    pub fn finish_to(&mut self, depth: usize) {
        debug_assert!(depth <= self.open, "finish_to past the open depth");
        while self.open > depth {
            self.finish();
        }
    }

    /// Emit the trailing trivia (the Eof entry's schedule) and close.
    /// Call with the root node still open — this flushes the tail,
    /// finishes the root, and closes the build.
    pub fn finish_tree(mut self, pos_after_last: usize) -> Rc<GreenNode> {
        self.flush(pos_after_last);
        self.builder.finish_node();
        self.builder.finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syntax::{SyntaxNode, debug_dump};

    /// A crate-private fake kind space — the neutrality proof: nothing
    /// here knows any real language's kinds.
    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    enum FakeKind {
        Root = 0,
        Wrap = 1,
        Ident = 2,
        Whitespace = 3,
        Comment = 4,
    }

    impl From<FakeKind> for SyntaxKind {
        fn from(k: FakeKind) -> SyntaxKind {
            SyntaxKind(k as u16)
        }
    }

    fn kind_name(kind: SyntaxKind) -> String {
        match kind.0 {
            0 => "ROOT",
            1 => "WRAP",
            2 => "IDENT",
            3 => "WHITESPACE",
            4 => "COMMENT",
            _ => "?",
        }
        .to_string()
    }

    /// Direct unit exercise of the sink alone: `IDENT@0..1 "a"` with
    /// one space of leading trivia, checkpoint-wrapped retroactively.
    /// Also covers `flush`'s idempotence directly: `flush(0)` runs
    /// first, then `token(0, ..)` flushes the SAME position again — a
    /// no-op, proven by the dump showing the leading space once.
    #[test]
    fn sink_builds_a_minimal_tree_with_checkpoint_wrap() {
        let entries = vec![
            SigLayout {
                text: "a".to_string(),
                trivia_before: vec![(FakeKind::Whitespace, " ".to_string())],
            },
            SigLayout {
                text: String::new(),
                trivia_before: vec![],
            },
        ];
        let mut sink = GreenSink::new(entries);
        sink.start(FakeKind::Root);
        let cp = sink.checkpoint();
        sink.flush(0);
        sink.start_at(cp, FakeKind::Wrap);
        sink.token(0, FakeKind::Ident);
        sink.finish(); // Wrap
        let tree = sink.finish_tree(1);
        let root = SyntaxNode::new_root(tree);
        assert_eq!(root.text(), " a");
        let dump = debug_dump(&root, &kind_name);
        assert_eq!(
            dump,
            "ROOT@0..2\n  WRAP@0..2\n    WHITESPACE@0..1 \" \"\n    IDENT@1..2 \"a\"\n"
        );
    }

    /// Depth tracking for error recovery: a failed parse may leave
    /// nodes open; `finish_to` closes back to a recorded depth so the
    /// recovery wrapper can retro-wrap the region into an error node.
    #[test]
    fn finish_to_unwinds_to_a_recorded_depth() {
        let entries = vec![
            SigLayout {
                text: "a".to_string(),
                trivia_before: vec![],
            },
            SigLayout {
                text: "b".to_string(),
                trivia_before: vec![],
            },
            SigLayout {
                text: String::new(),
                trivia_before: vec![],
            },
        ];
        let mut sink = GreenSink::new(entries);
        sink.start(FakeKind::Root);
        let depth = sink.open_depth();
        assert_eq!(depth, 1);
        let cp = sink.checkpoint();
        // A "failed parse": two nodes left open with a token inside.
        sink.start(FakeKind::Wrap);
        sink.token(0, FakeKind::Ident);
        sink.start(FakeKind::Wrap);
        assert_eq!(sink.open_depth(), 3);
        // Recovery: close back to the loop's depth, wrap the region.
        sink.finish_to(depth);
        assert_eq!(sink.open_depth(), depth);
        sink.start_at(cp, FakeKind::Wrap);
        sink.token(1, FakeKind::Ident);
        sink.finish();
        let root = SyntaxNode::new_root(sink.finish_tree(2));
        assert_eq!(root.text(), "ab");
        // One wrapping node under ROOT holding the partial region.
        assert_eq!(root.children().count(), 1);
        let wrap = root.children().next().expect("the recovery wrap");
        assert_eq!(wrap.text(), "ab");
    }

    /// `token` refuses to re-emit an already-taken significant position
    /// — the guard that catches schedule/index drift in a parser.
    #[test]
    #[cfg(debug_assertions)] // the guard is a debug_assert; release strips it
    #[should_panic(expected = "emitted twice")]
    fn a_token_emitted_twice_is_caught() {
        let entries = vec![SigLayout {
            text: "a".to_string(),
            trivia_before: vec![],
        }];
        let mut sink = GreenSink::new(entries);
        sink.start(FakeKind::Root);
        sink.token(0, FakeKind::Ident);
        sink.token(0, FakeKind::Ident);
    }

    fn fact(
        line: u32,
        col: u32,
        end: EndRule,
        class: TokenClass<FakeKind>,
    ) -> LayoutToken<FakeKind> {
        LayoutToken {
            line,
            col,
            end,
            class,
        }
    }

    /// The foundation invariant: trivia + token texts concatenate back
    /// to the source, byte for byte — with a trivia token, a multibyte
    /// gap, and a to-line-end token in play.
    #[test]
    fn layout_concat_reproduces_the_source() {
        let src = "aa µ\n#to-eol\nbb";
        let tokens = vec![
            fact(1, 1, EndRule::Chars(2), TokenClass::Significant), // aa
            fact(
                1,
                4,
                EndRule::Chars(1),
                TokenClass::Trivia(FakeKind::Comment),
            ), // µ
            fact(2, 1, EndRule::ToLineEnd, TokenClass::Significant), // #to-eol
            fact(3, 1, EndRule::Chars(2), TokenClass::Significant), // bb
            fact(3, 3, EndRule::AtStart, TokenClass::Significant),  // eof
        ];
        let entries = layout(src, &tokens, FakeKind::Whitespace);
        let mut out = String::new();
        for e in &entries {
            for (_, t) in &e.trivia_before {
                out.push_str(t);
            }
            out.push_str(&e.text);
        }
        assert_eq!(out, src, "layout is not lossless");
        assert_eq!(entries[1].text, "#to-eol");
        assert_eq!(
            entries[1].trivia_before,
            vec![
                (FakeKind::Whitespace, " ".to_string()),
                (FakeKind::Comment, "µ".to_string()),
                (FakeKind::Whitespace, "\n".to_string()),
            ]
        );
    }

    /// The Eof entry carries the trailing trivia; with none, its
    /// schedule is empty and the concatenation law still holds.
    #[test]
    fn eof_entry_carries_trailing_trivia() {
        let src = "a\n";
        let tokens = vec![
            fact(1, 1, EndRule::Chars(1), TokenClass::Significant),
            fact(2, 1, EndRule::AtStart, TokenClass::Significant),
        ];
        let entries = layout(src, &tokens, FakeKind::Whitespace);
        let eof = entries.last().expect("eof entry");
        assert_eq!(eof.text, "");
        assert_eq!(
            eof.trivia_before,
            vec![(FakeKind::Whitespace, "\n".to_string())]
        );
    }

    /// A non-whitespace gap between tokens is a lexer-contract
    /// violation and panics rather than silently landing in trivia.
    #[test]
    #[should_panic(expected = "non-whitespace between tokens")]
    fn a_non_whitespace_gap_is_refused() {
        let src = "a!b";
        let tokens = vec![
            fact(1, 1, EndRule::Chars(1), TokenClass::Significant),
            fact(1, 3, EndRule::Chars(1), TokenClass::Significant),
            fact(1, 4, EndRule::AtStart, TokenClass::Significant),
        ];
        layout(src, &tokens, FakeKind::Whitespace);
    }
}
