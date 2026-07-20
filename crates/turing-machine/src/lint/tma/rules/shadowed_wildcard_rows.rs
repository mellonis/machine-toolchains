//! `shadowed-wildcard-rows`: a match-table row whose pattern is covered by an
//! EARLIER row in the same dispatch band — it can never match, so it is dead.
//!
//! # The cover model (shared, mirrored locally)
//!
//! This is the assembly-level twin of the `.tmc` `dead-rule` lint and the
//! optimizer's `dead_rows` pass: the SAME same-band cover model, applied to a
//! different cell vocabulary. Row `W` covers row `R` cell-wise iff, at every
//! tape position, `W`'s cell is a wildcard or the exact index `R` has there;
//! then every input `R` matches, `W` matches too, so `R` never fires. As with
//! `dead-rule` and `dead_rows`, cover reasoning is order-aware and therefore
//! sound only WITHIN a band: codegen (and the assembler's row emission) puts
//! exact rows first (sorted, pairwise disjoint), then the partial and
//! catch-all rows in source order, so an earlier source row shadows a later
//! one it covers only when both share a band. The `Exact` band is vacuous
//! (disjoint exacts never cover each other; the assembler rejects a
//! non-disjoint pair outright), and the catch-all band holds at most one row
//! (the assembler rejects a second all-wildcard row), so a reported shadow is
//! in practice always a partial row covered by an earlier partial. The
//! predicate is copied locally rather than
//! shared because the three sites speak three cell types — `.tmc` glyph-label
//! sets (`lint::patterns`), IR cells (`optimizer::dead_rows`), and the raw
//! asm index/wildcard cells here — with no common representation; the model,
//! not the code, is what is shared.
//!
//! # What it sees
//!
//! Consecutive `.row` directives form one match table (a labeled row opens a
//! new table; unlabeled rows extend it — mirroring the assembler's grouping);
//! `.rept` bodies are scanned as their own tables. A row with a cell that is
//! not a plain wildcard or decimal index — a `.rept` substitution template
//! `{…}` inside a body — is opaque and takes no part in cover reasoning
//! (never covers, never reported). The lint runs behind the assemble fatal
//! gate, so every top-level `.row` is a well-formed match row by the time it
//! is reached.

use mtc_core::asm::cst::{AsmItem, AsmItemKind, TableDirectiveCst, TableDirectiveKind};
use mtc_core::diagnostics::{Diagnostic, Span};

use crate::lint::tma::TmaLintContext;

/// One parsed match cell: any symbol, or exactly this symbol index.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Cell {
    Wildcard,
    Index(u32),
}

/// A row's dispatch band. `Exact` = wildcard-free, `CatchAll` = all-wildcard,
/// `Partial` = a mix (mirrors codegen's row classification).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Band {
    Exact,
    Partial,
    CatchAll,
}

fn band(cells: &[Cell]) -> Band {
    if cells.iter().all(|c| matches!(c, Cell::Index(_))) {
        Band::Exact
    } else if cells.iter().all(|c| matches!(c, Cell::Wildcard)) {
        Band::CatchAll
    } else {
        Band::Partial
    }
}

/// Whether `w` covers `r` cell-wise (every input `r` matches, `w` matches
/// too). Differing arities never cover.
fn covers(w: &[Cell], r: &[Cell]) -> bool {
    w.len() == r.len()
        && w.iter().zip(r).all(|(wc, rc)| match (wc, rc) {
            (Cell::Wildcard, _) => true,
            (Cell::Index(a), Cell::Index(b)) => a == b,
            (Cell::Index(_), Cell::Wildcard) => false,
        })
}

/// A row's parsed cells (`None` when opaque — a `.rept` template cell) and
/// the directive's span (what a finding points at).
struct Row {
    cells: Option<Vec<Cell>>,
    span: Span,
}

/// Parse a `.row`'s single verbatim `[..]` operand into cells; `None` if any
/// cell is neither `*` nor a decimal index (a substitution template).
fn parse_row(td: &TableDirectiveCst) -> Row {
    let cells = td.operands.first().and_then(|op| {
        let inner = op.text.trim().strip_prefix('[')?.strip_suffix(']')?;
        inner
            .split(',')
            .map(|raw| {
                let s = raw.trim();
                if s == "*" {
                    Some(Cell::Wildcard)
                } else if !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()) {
                    s.parse::<u32>().ok().map(Cell::Index)
                } else {
                    None
                }
            })
            .collect::<Option<Vec<Cell>>>()
    });
    Row {
        cells,
        span: td.span,
    }
}

pub(crate) fn check(ctx: &TmaLintContext, out: &mut Vec<Diagnostic>) {
    scan(&ctx.cst.items, out);
}

/// Walk a run of items, grouping consecutive `.row` directives into tables
/// and descending into `.rept` bodies (their rows form their own tables).
fn scan(items: &[AsmItem], out: &mut Vec<Diagnostic>) {
    let mut table: Vec<Row> = Vec::new();
    for item in items {
        match &item.kind {
            AsmItemKind::TableDirective(td) if td.kind == TableDirectiveKind::Row => {
                // A labeled row opens a new table (the assembler names a table
                // by its first row's label); flush the previous one first.
                if !td.labels.is_empty() {
                    report(&table, out);
                    table.clear();
                }
                table.push(parse_row(td));
            }
            // Comments and blank runs are trivia — they never break a table.
            AsmItemKind::Comment(_) => {}
            AsmItemKind::Rept(rept) => {
                report(&table, out);
                table.clear();
                scan(&rept.body, out);
            }
            // Anything structural (a dispatch/frame directive, a `.section`, a
            // `.func`, an instruction line) ends the current table.
            _ => {
                report(&table, out);
                table.clear();
            }
        }
    }
    report(&table, out);
}

/// Flag every row an earlier same-band row covers.
fn report(table: &[Row], out: &mut Vec<Diagnostic>) {
    for (k, row) in table.iter().enumerate() {
        let Some(rk) = &row.cells else { continue };
        let bk = band(rk);
        if bk == Band::Exact {
            continue; // disjoint exacts never cover (assembler-enforced)
        }
        let coverer = (0..k).find(|&j| {
            table[j]
                .cells
                .as_ref()
                .is_some_and(|rj| band(rj) == bk && covers(rj, rk))
        });
        if let Some(j) = coverer {
            out.push(Diagnostic {
                code: "shadowed-wildcard-rows",
                span: row.span,
                message: format!(
                    "this row can never match — the earlier row at line {} in the same match table already covers it",
                    table[j].span.start.line
                ),
                fix: None,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::lint::tma::lint_tma;

    fn findings(src: &str) -> Vec<String> {
        lint_tma(src, &[])
            .unwrap()
            .into_iter()
            .filter(|d| d.code == "shadowed-wildcard-rows")
            .map(|d| format!("{}:{}", d.span.start.line, d.message))
            .collect()
    }

    #[test]
    fn a_partial_row_shadowed_by_an_earlier_partial_fires() {
        // `[1,*,*]` (row 0, partial) covers `[1,2,*]` (row 1, partial) — same
        // band, earlier → row 1 is dead. `[*,*,*]` catch-all survives.
        let src = "\
.routine main, tapes=3, alpha=(3, 3, 3)
.section tables
T0: .row [1, *, *]
    .row [1, 2, *]
    .row [*, *, *]
.section code
.func main
        rd
        mtc T0
        stp
";
        let f = findings(src);
        assert_eq!(f.len(), 1, "{f:?}");
        assert!(f[0].starts_with("4:"), "points at the shadowed row: {f:?}");
    }

    #[test]
    fn a_wider_partial_shadows_a_narrower_partial_two_cells_in() {
        // `[*,1,*]` (row 0) covers `[3,1,*]` (row 1) — cell 1 agrees, cells 0
        // and 2 are the coverer's wildcards. Both partial, row 0 earlier.
        // (A catch-all cannot be shadowed: the assembler forbids two
        // all-wildcard rows, and only a catch-all covers a catch-all.)
        let src = "\
.routine main, tapes=3, alpha=(4, 2, 2)
.section tables
T0: .row [*, 1, *]
    .row [3, 1, *]
    .row [*, *, *]
.section code
.func main
        rd
        mtc T0
        stp
";
        let f = findings(src);
        assert_eq!(f.len(), 1, "{f:?}");
        assert!(f[0].starts_with("4:"), "{f:?}");
    }

    #[test]
    fn distinct_partials_do_not_shadow() {
        // `[1,*,*,*]`..`[8,*,*,*]` differ in cell 0 — none covers another.
        let src = "\
.routine main, tapes=4, alpha=(9, 2, 2, 2)
.section tables
T0: .row [1, *, *, *]
    .row [2, *, *, *]
    .row [*, *, *, *]
.section code
.func main
        rd
        mtc T0
        stp
";
        assert!(findings(src).is_empty(), "{:?}", findings(src));
    }

    #[test]
    fn a_partial_and_a_catch_all_are_different_bands() {
        // `[*,0]` (partial) and `[*,*]` (catch-all) — cell-wise the catch-all
        // covers, but they are different bands, so no shadow (the brainfuck
        // Tzero shape).
        let src = "\
.routine main, tapes=2, alpha=(2, 2)
.section tables
T0: .row [*, 0]
    .row [*, *]
.section code
.func main
        rd
        mtc T0
        stp
";
        assert!(findings(src).is_empty(), "{:?}", findings(src));
    }

    #[test]
    fn a_labeled_row_starts_a_new_table_so_no_cross_table_shadow() {
        // Two tables back to back, each `[1,*]` then `[*,*]`. Row 0 of T1 must
        // not be compared against T0's rows — merging them would flag T1's
        // `[1,*]` (covered by T0's `[1,*]`) and its second `[*,*]`. Both
        // tables are referenced so the file assembles.
        let src = "\
.routine main, tapes=2, alpha=(2, 2)
.section tables
T0: .row [1, *]
    .row [*, *]
T1: .row [1, *]
    .row [*, *]
.section code
.func main
        rd
        mtc T0
        mtc T1
        stp
";
        assert!(findings(src).is_empty(), "{:?}", findings(src));
    }

    #[test]
    fn a_rept_template_row_is_opaque_and_never_flagged() {
        // `.rept` bodies with `{v}` cells cannot be classified — they take no
        // part in cover reasoning (the brainfuck Tinc shape: one templated
        // row per iteration, distinct after expansion).
        let src = "\
.routine main, tapes=2, alpha=(2, 128)
.section tables
.rept v, 0, 126
T0: .row [*, {v}]
.endr
.section code
.func main
        rd
        mtc T0
        stp
";
        assert!(findings(src).is_empty(), "{:?}", findings(src));
    }
}
