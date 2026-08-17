//! Address ↔ source-line resolution over the link-time map sidecar
//! (docs/formats.md (map sidecar)). [`LineIndex`] is built once from a
//! parsed [`crate::linker::MapFile`] and answers the two queries a DAP
//! adapter needs on every stop and every `setBreakpoints` request: "what
//! function/line/file is this address in" ([`LineIndex::resolve`]) and
//! "what address should a breakpoint on this line plant at"
//! ([`LineIndex::address_for_line`]). Both are sorted-vector binary
//! searches — no hash maps, since a `MapFile`'s function and line tables
//! are small and read-only once loaded. Source provenance rides each
//! function record verbatim (the sidecar's own relative-path policy —
//! docs/formats.md (map sidecar)); this index never touches paths, it
//! only carries and filters by the raw strings. The degradation this
//! produces without `-g` debug info is documented user-facing at
//! docs/dap.md (stepping granularity, breakpoints and stepping).

use crate::linker::MapFile;

/// One function's address range and (offset, line) table, owned and
/// sorted for binary search.
#[derive(Debug, Clone)]
struct FunctionEntry {
    name: String,
    /// Absolute code offset of the function's `ent`.
    start: u32,
    /// Exclusive end offset.
    end: u32,
    /// `(absolute code offset, source line)`, sorted by offset ascending.
    lines: Vec<(u32, u32)>,
    /// The map record's source provenance, verbatim
    /// ([`crate::linker::MapFunction::source`]); `None` for a function
    /// linked without it.
    source: Option<String>,
}

/// What an address resolves to: the containing function, the mapped
/// source line (if `-g` line info covers the address), and the
/// function's source-file provenance (if the sidecar carries it) —
/// verbatim as stored, typically relative to the sidecar's directory
/// (docs/formats.md (map sidecar)).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SourceLoc<'a> {
    pub function: &'a str,
    pub line: Option<u32>,
    /// The function's FIRST mapped line, regardless of `addr` — `None`
    /// only when the function has no line entries at all. The anchor for
    /// consumers that must render a sourced position for an address
    /// BEFORE the first mapped instruction (linker-synthesized preludes):
    /// the native-debugger prologue convention points such addresses at
    /// the function's opening line rather than dropping the source.
    /// `line` itself stays `None` there deliberately — stepping compares
    /// raw (function, line) pairs and must see the unmapped prefix as
    /// its own position.
    pub function_first_line: Option<u32>,
    pub source: Option<&'a str>,
}

/// Address ↔ source-line index over a linked executable's map sidecar
/// (docs/formats.md (map sidecar)). Immutable once built; cheap to query
/// repeatedly (a `DebugSession` step or a `setBreakpoints` request each
/// do one lookup).
#[derive(Debug, Clone)]
pub struct LineIndex {
    /// Functions sorted by `start`, non-overlapping — supports
    /// [`LineIndex::resolve`]'s containment search.
    functions: Vec<FunctionEntry>,
    /// Every function's `(line, offset, function index)` triples merged
    /// into one table, sorted by `(line, offset)` ascending — supports
    /// [`LineIndex::address_for_line`]'s at-or-after search. Sorting the
    /// tie-break on `offset` too means that when a source line maps to
    /// more than one code offset (e.g. a loop body revisited by
    /// codegen), the first entry at that line is already the lowest
    /// address, matching the "first address" breakpoint-planting rule.
    /// The function index (into the sorted `functions`) carries each
    /// entry's source provenance for the per-file filter.
    by_line: Vec<(u32, u32, u32)>,
}

impl LineIndex {
    /// Builds the index from a parsed map sidecar. Functions without
    /// `-g` debug info carry an empty `lines` table — see
    /// [`LineIndex::resolve`]'s empty-table case.
    pub fn new(map: &MapFile) -> LineIndex {
        let mut functions: Vec<FunctionEntry> = map
            .functions
            .iter()
            .map(|f| {
                let mut lines = f.lines.clone();
                lines.sort_unstable_by_key(|&(offset, _)| offset);
                FunctionEntry {
                    name: f.name.clone(),
                    start: f.start,
                    end: f.end,
                    lines,
                    source: f.source.clone(),
                }
            })
            .collect();
        functions.sort_unstable_by_key(|f| f.start);

        let mut by_line: Vec<(u32, u32, u32)> = functions
            .iter()
            .enumerate()
            .flat_map(|(fi, f)| {
                f.lines
                    .iter()
                    .map(move |&(offset, line)| (line, offset, fi as u32))
            })
            .collect();
        by_line.sort_unstable();

        LineIndex { functions, by_line }
    }

    /// True when any function record carries source provenance — the
    /// adapter's switch between per-file breakpoint filtering and the
    /// legacy global line table (docs/dap.md (breakpoints and stepping)).
    pub fn has_sources(&self) -> bool {
        self.functions.iter().any(|f| f.source.is_some())
    }

    /// The containing function, its mapped source line — the largest
    /// mapped offset `<= addr` — and its source provenance, if `addr`
    /// falls inside any function's `[start, end)` range. A function
    /// whose `lines` table is empty (no `-g` debug info) resolves with
    /// `line: None`: the function is still found, but no line is known.
    pub fn resolve(&self, addr: u32) -> Option<SourceLoc<'_>> {
        let idx = self
            .functions
            .partition_point(|f| f.start <= addr)
            .checked_sub(1)?;
        let f = &self.functions[idx];
        if addr >= f.end {
            return None;
        }

        let line_idx = f.lines.partition_point(|&(offset, _)| offset <= addr);
        let line = line_idx.checked_sub(1).map(|i| f.lines[i].1);
        Some(SourceLoc {
            function: f.name.as_str(),
            line,
            function_first_line: f.lines.first().map(|&(_, line)| line),
            source: f.source.as_deref(),
        })
    }

    /// The first address mapped at-or-after `line` — the
    /// breakpoint-planting rule: a breakpoint requested on an unmapped
    /// line (a blank line, a comment, a line that compiled to nothing)
    /// plants at the next mapped line instead. `None` means `line` is
    /// past every mapped line considered: the breakpoint is
    /// unverifiable.
    ///
    /// `source` is the per-file dimension (docs/dap.md (breakpoints and
    /// stepping)): `Some(s)` considers only entries whose function
    /// carries exactly that provenance string, so identical line numbers
    /// across compilation units stop colliding; `None` searches every
    /// function — the pre-provenance behavior, which an adapter keeps
    /// for a sidecar with no provenance at all.
    ///
    /// Selection is by **line**, not by address: among the qualifying
    /// `(line, offset)` entries, this picks the smallest `line >=
    /// line`, then (only to break a tie between two entries that share
    /// that same line) the smallest `offset`. Function layout in a
    /// linked image is emission order, not source order, so a
    /// lower-addressed function can carry higher source lines than a
    /// later one — picking by address instead could plant a line-7
    /// breakpoint at a line-40 offset just because that offset happens
    /// to sort first.
    pub fn address_for_line(&self, line: u32, source: Option<&str>) -> Option<u32> {
        let start = self.by_line.partition_point(|&(l, _, _)| l < line);
        self.by_line[start..]
            .iter()
            .find(|&&(_, _, fi)| match source {
                None => true,
                Some(s) => self.functions[fi as usize].source.as_deref() == Some(s),
            })
            .map(|&(_, offset, _)| offset)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::linker::MapFunction;

    fn func(
        name: &str,
        start: u32,
        end: u32,
        lines: Vec<(u32, u32)>,
        source: Option<&str>,
    ) -> MapFunction {
        MapFunction {
            name: name.to_string(),
            start,
            end,
            labels: vec![],
            lines,
            source: source.map(str::to_string),
        }
    }

    fn sample_map() -> MapFile {
        MapFile {
            arch: 1,
            functions: vec![
                func("main", 0, 10, vec![(0, 1), (2, 3), (5, 5)], None),
                func("helper", 20, 30, vec![(20, 10), (25, 12)], None),
                func("no_debug", 30, 35, vec![], None),
                // first mapped offset (37) is after `start` (35) —
                // an unmapped prologue prefix.
                func("prologue", 35, 40, vec![(37, 20)], None),
            ],
            bindings: vec![],
        }
    }

    /// Two units whose line tables overlap — the collision the per-file
    /// dimension exists to split (docs/dap.md (breakpoints and
    /// stepping)).
    fn two_unit_map() -> MapFile {
        MapFile {
            arch: 1,
            functions: vec![
                func("main", 0, 10, vec![(0, 1), (4, 5)], Some("a.pmc")),
                func("helper", 10, 20, vec![(10, 2), (15, 5)], Some("b.pmc")),
                func("blob", 20, 25, vec![(20, 3)], None),
            ],
            bindings: vec![],
        }
    }

    #[test]
    fn resolve_at_exact_offset() {
        let idx = LineIndex::new(&sample_map());
        assert_eq!(
            idx.resolve(5),
            Some(SourceLoc {
                function: "main",
                line: Some(5),
                function_first_line: Some(1),
                source: None
            })
        );
        assert_eq!(
            idx.resolve(25),
            Some(SourceLoc {
                function: "helper",
                line: Some(12),
                function_first_line: Some(10),
                source: None
            })
        );
    }

    #[test]
    fn function_first_line_is_carried_regardless_of_addr() {
        let idx = LineIndex::new(&sample_map());
        // `prologue`'s unmapped prefix: no line for the address itself,
        // but the function's first mapped line is available as the
        // prologue-convention anchor.
        let pre = idx.resolve(35).unwrap();
        assert_eq!(pre.line, None);
        assert_eq!(pre.function_first_line, Some(20));
        // A function with no line entries at all carries neither.
        let bare = idx.resolve(32).unwrap();
        assert_eq!(bare.line, None);
        assert_eq!(bare.function_first_line, None);
    }

    #[test]
    fn resolve_between_offsets() {
        let idx = LineIndex::new(&sample_map());
        // offset 3 falls between the offset-2/line-3 and offset-5/line-5
        // entries — the largest mapped offset <= 3 is 2, so line 3 wins.
        assert_eq!(idx.resolve(3).unwrap().line, Some(3));
        // offset 9 falls after the last entry (offset 5, line 5) but
        // still inside `main`'s [0, 10) range.
        assert_eq!(idx.resolve(9).unwrap().line, Some(5));
    }

    #[test]
    fn resolve_outside_any_function() {
        let idx = LineIndex::new(&sample_map());
        // in the gap between `main`'s end (10) and `helper`'s start (20)
        assert_eq!(idx.resolve(15), None);
        // `end` is exclusive — the boundary itself is not in `main`
        assert_eq!(idx.resolve(10), None);
        // past every function
        assert_eq!(idx.resolve(100), None);
    }

    #[test]
    fn resolve_before_first_mapped_offset_in_function() {
        let idx = LineIndex::new(&sample_map());
        // `prologue` spans [35, 40) but its first `lines` entry starts
        // at offset 37 — the two leading offsets have no mapped line
        // yet, even though they're inside the function.
        assert_eq!(idx.resolve(35).unwrap().line, None);
        assert_eq!(idx.resolve(36).unwrap().line, None);
        assert_eq!(idx.resolve(37).unwrap().line, Some(20));
    }

    #[test]
    fn resolve_empty_lines_table() {
        let idx = LineIndex::new(&sample_map());
        let loc = idx.resolve(32).unwrap();
        assert_eq!((loc.function, loc.line), ("no_debug", None));
    }

    #[test]
    fn resolve_carries_source_provenance() {
        let idx = LineIndex::new(&two_unit_map());
        assert_eq!(idx.resolve(4).unwrap().source, Some("a.pmc"));
        assert_eq!(idx.resolve(15).unwrap().source, Some("b.pmc"));
        assert_eq!(idx.resolve(22).unwrap().source, None);
    }

    #[test]
    fn has_sources_reports_any_provenance() {
        assert!(!LineIndex::new(&sample_map()).has_sources());
        assert!(LineIndex::new(&two_unit_map()).has_sources());
    }

    #[test]
    fn address_for_line_exact() {
        let idx = LineIndex::new(&sample_map());
        assert_eq!(idx.address_for_line(1, None), Some(0));
        assert_eq!(idx.address_for_line(10, None), Some(20));
        assert_eq!(idx.address_for_line(12, None), Some(25));
    }

    #[test]
    fn address_for_line_gap_next_mapped_line_wins() {
        let idx = LineIndex::new(&sample_map());
        // line 2 is unmapped; the next mapped line is 3, at offset 2.
        assert_eq!(idx.address_for_line(2, None), Some(2));
        // line 4 is unmapped; the next mapped line is 5, at offset 5.
        assert_eq!(idx.address_for_line(4, None), Some(5));
        // line 6 is unmapped and so are 7/8/9; the next mapped line
        // crosses into `helper` at line 10, offset 20.
        assert_eq!(idx.address_for_line(6, None), Some(20));
    }

    #[test]
    fn address_for_line_past_the_end() {
        let idx = LineIndex::new(&sample_map());
        // the highest mapped line across every function is 20
        // (`prologue`'s only entry); nothing maps at or after 21.
        assert_eq!(idx.address_for_line(21, None), None);
    }

    #[test]
    fn address_for_line_filters_by_source() {
        let idx = LineIndex::new(&two_unit_map());
        // Line 5 exists in BOTH units: offset 4 in a.pmc, offset 15 in
        // b.pmc. The global search answers the lowest offset; the
        // filtered ones split the collision.
        assert_eq!(idx.address_for_line(5, None), Some(4));
        assert_eq!(idx.address_for_line(5, Some("a.pmc")), Some(4));
        assert_eq!(idx.address_for_line(5, Some("b.pmc")), Some(15));
        // Line 4 is unmapped in both: the at-or-after rule stays within
        // the requested file (a.pmc's next mapped line is 5 at offset
        // 4; b.pmc's is 5 at offset 15).
        assert_eq!(idx.address_for_line(4, Some("a.pmc")), Some(4));
        assert_eq!(idx.address_for_line(4, Some("b.pmc")), Some(15));
        // Line 1 exists only in a.pmc — b.pmc's search skips past it to
        // its own first mapped line (2, at offset 10).
        assert_eq!(idx.address_for_line(1, Some("b.pmc")), Some(10));
        // A file the map never names matches nothing.
        assert_eq!(idx.address_for_line(1, Some("c.pmc")), None);
        // A provenance-less function is reachable only by the global
        // search — never by any file's filter.
        assert_eq!(idx.address_for_line(3, None), Some(20));
        assert_eq!(idx.address_for_line(3, Some("a.pmc")), Some(4));
    }

    #[test]
    fn address_for_line_selects_by_line_not_by_address() {
        // A linked image lays out functions by emission order, not
        // source order, so a lower-addressed function can carry higher
        // source lines than a later one. Here offset 0 maps to line
        // 100, and the later offset 10 maps to line 5 — address order
        // and line order disagree. Picking by minimal *address* among
        // qualifying entries would answer both queries below with
        // offset 0 (line 100 >= both targets); the breakpoint-planting
        // rule instead must pick by minimal *line*, landing on offset
        // 10 (line 5) in both cases.
        let map = MapFile {
            arch: 1,
            functions: vec![
                func("low_addr_high_line", 0, 5, vec![(0, 100)], None),
                func("high_addr_low_line", 10, 15, vec![(10, 5)], None),
            ],
            bindings: vec![],
        };
        let idx = LineIndex::new(&map);
        assert_eq!(idx.address_for_line(1, None), Some(10));
        assert_eq!(idx.address_for_line(5, None), Some(10));
    }
}
