//! Address ↔ source-line resolution over the link-time map sidecar
//! (docs/formats.md (map sidecar)). [`LineIndex`] is built once from a
//! parsed [`crate::linker::MapFile`] and answers the two queries a DAP
//! adapter needs on every stop and every `setBreakpoints` request: "what
//! function/line is this address in" ([`LineIndex::resolve`]) and "what
//! address should a breakpoint on this line plant at"
//! ([`LineIndex::address_for_line`]). Both are sorted-vector binary
//! searches — no hash maps, since a `MapFile`'s function and line tables
//! are small and read-only once loaded. The degradation this produces
//! without `-g` debug info is documented user-facing at docs/dap.md
//! (stepping granularity, breakpoints and stepping).

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
    /// Every function's `(line, offset)` pairs merged into one table,
    /// sorted by `(line, offset)` ascending — supports
    /// [`LineIndex::address_for_line`]'s at-or-after search. Sorting the
    /// tie-break on `offset` too means that when a source line maps to
    /// more than one code offset (e.g. a loop body revisited by
    /// codegen), the first entry at that line is already the lowest
    /// address, matching the "first address" breakpoint-planting rule.
    by_line: Vec<(u32, u32)>,
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
                }
            })
            .collect();
        functions.sort_unstable_by_key(|f| f.start);

        let mut by_line: Vec<(u32, u32)> = functions
            .iter()
            .flat_map(|f| f.lines.iter().map(|&(offset, line)| (line, offset)))
            .collect();
        by_line.sort_unstable();

        LineIndex { functions, by_line }
    }

    /// The containing function and its mapped source line — the largest
    /// mapped offset `<= addr` — if `addr` falls inside any function's
    /// `[start, end)` range. A function whose `lines` table is empty (no
    /// `-g` debug info) resolves to `(name, None)`: the function is
    /// still found, but no line is known.
    pub fn resolve(&self, addr: u32) -> Option<(&str, Option<u32>)> {
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
        Some((f.name.as_str(), line))
    }

    /// The first address mapped at-or-after `line`, across every
    /// function — the breakpoint-planting rule: a breakpoint requested
    /// on an unmapped line (a blank line, a comment, a line that
    /// compiled to nothing) plants at the next mapped line instead.
    /// `None` means `line` is past every mapped line in the map: the
    /// breakpoint is unverifiable.
    ///
    /// Selection is by **line**, not by address: among every `(line,
    /// offset)` entry in the map, this picks the smallest `line >=
    /// line`, then (only to break a tie between two entries that share
    /// that same line) the smallest `offset`. Function layout in a
    /// linked image is emission order, not source order, so a
    /// lower-addressed function can carry higher source lines than a
    /// later one — picking by address instead could plant a line-7
    /// breakpoint at a line-40 offset just because that offset happens
    /// to sort first.
    pub fn address_for_line(&self, line: u32) -> Option<u32> {
        let idx = self.by_line.partition_point(|&(l, _)| l < line);
        self.by_line.get(idx).map(|&(_, offset)| offset)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::linker::MapFunction;

    fn sample_map() -> MapFile {
        MapFile {
            arch: 1,
            functions: vec![
                MapFunction {
                    name: "main".to_string(),
                    start: 0,
                    end: 10,
                    labels: vec![],
                    lines: vec![(0, 1), (2, 3), (5, 5)],
                },
                MapFunction {
                    name: "helper".to_string(),
                    start: 20,
                    end: 30,
                    labels: vec![],
                    lines: vec![(20, 10), (25, 12)],
                },
                MapFunction {
                    name: "no_debug".to_string(),
                    start: 30,
                    end: 35,
                    labels: vec![],
                    lines: vec![],
                },
                MapFunction {
                    name: "prologue".to_string(),
                    start: 35,
                    end: 40,
                    labels: vec![],
                    // first mapped offset (37) is after `start` (35) —
                    // an unmapped prologue prefix.
                    lines: vec![(37, 20)],
                },
            ],
            bindings: vec![],
        }
    }

    #[test]
    fn resolve_at_exact_offset() {
        let idx = LineIndex::new(&sample_map());
        assert_eq!(idx.resolve(5), Some(("main", Some(5))));
        assert_eq!(idx.resolve(25), Some(("helper", Some(12))));
    }

    #[test]
    fn resolve_between_offsets() {
        let idx = LineIndex::new(&sample_map());
        // offset 3 falls between the offset-2/line-3 and offset-5/line-5
        // entries — the largest mapped offset <= 3 is 2, so line 3 wins.
        assert_eq!(idx.resolve(3), Some(("main", Some(3))));
        // offset 9 falls after the last entry (offset 5, line 5) but
        // still inside `main`'s [0, 10) range.
        assert_eq!(idx.resolve(9), Some(("main", Some(5))));
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
        assert_eq!(idx.resolve(35), Some(("prologue", None)));
        assert_eq!(idx.resolve(36), Some(("prologue", None)));
        assert_eq!(idx.resolve(37), Some(("prologue", Some(20))));
    }

    #[test]
    fn resolve_empty_lines_table() {
        let idx = LineIndex::new(&sample_map());
        assert_eq!(idx.resolve(32), Some(("no_debug", None)));
    }

    #[test]
    fn address_for_line_exact() {
        let idx = LineIndex::new(&sample_map());
        assert_eq!(idx.address_for_line(1), Some(0));
        assert_eq!(idx.address_for_line(10), Some(20));
        assert_eq!(idx.address_for_line(12), Some(25));
    }

    #[test]
    fn address_for_line_gap_next_mapped_line_wins() {
        let idx = LineIndex::new(&sample_map());
        // line 2 is unmapped; the next mapped line is 3, at offset 2.
        assert_eq!(idx.address_for_line(2), Some(2));
        // line 4 is unmapped; the next mapped line is 5, at offset 5.
        assert_eq!(idx.address_for_line(4), Some(5));
        // line 6 is unmapped and so are 7/8/9; the next mapped line
        // crosses into `helper` at line 10, offset 20.
        assert_eq!(idx.address_for_line(6), Some(20));
    }

    #[test]
    fn address_for_line_past_the_end() {
        let idx = LineIndex::new(&sample_map());
        // the highest mapped line across every function is 20
        // (`prologue`'s only entry); nothing maps at or after 21.
        assert_eq!(idx.address_for_line(21), None);
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
                MapFunction {
                    name: "low_addr_high_line".to_string(),
                    start: 0,
                    end: 5,
                    labels: vec![],
                    lines: vec![(0, 100)],
                },
                MapFunction {
                    name: "high_addr_low_line".to_string(),
                    start: 10,
                    end: 15,
                    labels: vec![],
                    lines: vec![(10, 5)],
                },
            ],
            bindings: vec![],
        };
        let idx = LineIndex::new(&map);
        assert_eq!(idx.address_for_line(1), Some(10));
        assert_eq!(idx.address_for_line(5), Some(10));
    }
}
