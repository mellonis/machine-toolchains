//! Shared diagnostic primitives: positions, spans, and structured
//! findings with optional machine-applicable fixes. Arch-agnostic by
//! contract — no architecture may leak in. Producers live in the arch
//! crates (the `.pmc` compiler and lint layer today); renderers live in
//! their CLIs (docs/cli.md (thin-renderer rule)).

/// 1-based line and column; columns count characters, not bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Pos {
    pub line: u32,
    pub col: u32,
}

/// Half-open range: `start` inclusive, `end` exclusive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Span {
    pub start: Pos,
    pub end: Pos,
}

impl Span {
    pub fn new(start_line: u32, start_col: u32, end_line: u32, end_col: u32) -> Span {
        Span {
            start: Pos {
                line: start_line,
                col: start_col,
            },
            end: Pos {
                line: end_line,
                col: end_col,
            },
        }
    }

    /// A single-column span at one position.
    pub fn point(line: u32, col: u32) -> Span {
        Span::new(line, col, line, col + 1)
    }
}

/// Confidence tier of a fix (the rustc suggestion model): plain `--fix`
/// applies only `MachineApplicable`; `MaybeIncorrect` needs `--force`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Applicability {
    MachineApplicable,
    MaybeIncorrect,
}

/// One text edit; an empty `replacement` deletes the span.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edit {
    pub span: Span,
    pub replacement: String,
}

/// A machine-applicable remedy; `edits` apply atomically.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fix {
    pub description: String,
    pub applicability: Applicability,
    pub edits: Vec<Edit>,
}

/// One structured finding. The code is a stable kebab-case rule id
/// (`"unused-label"`); rendering prefixes (`warning:` / `lint:`) are a
/// property of the producing channel, not a field here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub code: &'static str,
    pub span: Span,
    pub message: String,
    pub fix: Option<Fix>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn positions_and_spans_order_by_start() {
        let a = Span::new(1, 5, 1, 8);
        let b = Span::new(2, 1, 2, 2);
        let c = Span::new(1, 9, 1, 10);
        let mut spans = vec![b, c, a];
        spans.sort();
        assert_eq!(spans, vec![a, c, b]);
        assert!(Pos { line: 1, col: 9 } < Pos { line: 2, col: 1 });
    }

    #[test]
    fn point_spans_are_one_column_wide() {
        let p = Span::point(3, 7);
        assert_eq!(p.start, Pos { line: 3, col: 7 });
        assert_eq!(p.end, Pos { line: 3, col: 8 });
    }

    #[test]
    fn a_diagnostic_carries_its_optional_fix() {
        let d = Diagnostic {
            code: "unused-label",
            span: Span::new(12, 3, 12, 5),
            message: "label 5 is never referenced (function 'f')".into(),
            fix: Some(Fix {
                description: "remove the label prefix `5:`".into(),
                applicability: Applicability::MaybeIncorrect,
                edits: vec![Edit {
                    span: Span::new(12, 3, 12, 5),
                    replacement: String::new(),
                }],
            }),
        };
        assert_eq!(d.fix.as_ref().unwrap().edits[0].replacement, "");
        assert_eq!(d.fix.unwrap().applicability, Applicability::MaybeIncorrect);
    }
}
