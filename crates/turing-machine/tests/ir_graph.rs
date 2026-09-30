//! `tmt ir graph`'s three views over one world: the row-for-row raw view,
//! the lossless merged view, and the per-state-pair shape view
//! (docs/tmt/cli.md (tmt ir)).

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};

use proptest::prelude::*;

use mtc_turing_machine::cli::execute;
use mtc_turing_machine::compiler::{CompileOptions, compile};
use mtc_turing_machine::ir::{
    IrCell, IrDispatch, IrMove, IrProgram, IrRule, IrState, IrTape, IrThen, IrTransition, IrWorld,
    IrWorldKind, IrWrite,
};
use mtc_turing_machine::ir_graph::{GraphView, MergedRow, ReadCell, merge_rows, merged_label};
use mtc_turing_machine::optimizer::OptLevel;

fn tape(name: &str, card: u32) -> IrTape {
    IrTape {
        name: name.into(),
        alphabet: "al".into(),
        cardinality: card,
        volatile: false,
        glyphs: Vec::new(),
        writes: None,
        enters: None,
        leaves: None,
        opaque: false,
    }
}

fn rule(pattern: Vec<IrCell>, transition: IrTransition) -> IrRule {
    IrRule {
        pattern,
        write: None,
        moves: None,
        debugger: false,
        transition,
        synthesized: false,
        direct: false,
        line: 1,
    }
}

fn ix(i: u32) -> IrCell {
    IrCell::Index { index: i }
}

const ANY: IrCell = IrCell::Wildcard;

fn world(arity: u32, states: Vec<Vec<IrRule>>) -> IrWorld {
    IrWorld {
        name: "w".into(),
        kind: IrWorldKind::Routine,
        arity,
        tapes: (0..arity).map(|i| tape(&format!("t{i}"), 4)).collect(),
        entry: 0,
        states: states
            .into_iter()
            .enumerate()
            .map(|(id, rules)| IrState {
                id: id as u32,
                name: format!("s{id}"),
                line: 1,
                rules,
                dispatch: IrDispatch::Table,
            })
            .collect(),
        local: true,
        line: 1,
        exits: 2,
        returns: true,
    }
}

/// One world exercising every edge shape the renderer draws: a plain
/// goto with a write and a move, a `debugger` row, a call with two exits
/// and each kind of `then`, a tail call, the three returns and both
/// terminators, and the three trap kinds.
fn every_shape() -> IrWorld {
    let call = |then: Option<IrThen>| IrTransition::CallThen {
        target: "ns::r".into(),
        binding: Vec::new(),
        exits: vec![1, 0],
        then,
    };
    let mut wrote = rule(vec![ix(1), ANY], IrTransition::Goto { state: 1 });
    wrote.write = Some(vec![IrWrite::Index { index: 2 }, IrWrite::Keep]);
    wrote.moves = Some(vec![IrMove::Right, IrMove::Stay]);
    let mut brk = rule(vec![ix(2), ix(3)], IrTransition::Goto { state: 0 });
    brk.debugger = true;
    brk.moves = Some(vec![IrMove::Left, IrMove::Left]);
    let trap = |t: IrTransition| {
        let mut r = rule(vec![ix(0), ix(0)], t);
        r.synthesized = true;
        r
    };
    world(
        2,
        vec![
            vec![
                wrote,
                brk,
                rule(vec![ix(0), ix(1)], call(Some(IrThen::Goto { state: 1 }))),
                rule(vec![ix(0), ix(2)], call(Some(IrThen::Return))),
                rule(
                    vec![ix(0), ix(3)],
                    call(Some(IrThen::ReturnExit { exit: 1 })),
                ),
                rule(vec![ix(3), ix(0)], call(Some(IrThen::Stop))),
                rule(vec![ix(3), ix(1)], call(Some(IrThen::Halt))),
                rule(vec![ix(3), ix(2)], call(None)),
                rule(
                    vec![ANY, ANY],
                    IrTransition::TailCall {
                        target: "ns::t".into(),
                    },
                ),
            ],
            vec![
                rule(vec![ix(1), ix(1)], IrTransition::Return),
                rule(vec![ix(1), ix(2)], IrTransition::ReturnExit { exit: 0 }),
                rule(vec![ix(1), ix(3)], IrTransition::Stop),
                rule(vec![ix(2), ix(1)], IrTransition::Halt),
                trap(IrTransition::TrapRead),
                trap(IrTransition::TrapWrite),
                trap(IrTransition::TrapContract),
            ],
        ],
    )
}

/// The raw view is today's row-for-row rendering, byte for byte.
///
/// Mutation: change any edge format in the renderer (drop the space in
/// `call ns::r exit #0`, say) and the literal no longer matches.
#[test]
fn the_raw_view_is_pinned_byte_for_byte() {
    let expected = r#"flowchart TD
    S0["s0"]
    S1["s1"]
    T_ret(("ret"))
    T_ret1(("ret #1"))
    T_stp(("stp"))
    T_hlt(("hlt"))
    T_tail(("tail"))
    T_ret0(("ret #0"))
    T_trap0(("trap #0"))
    T_trap1(("trap #1"))
    T_trap2(("trap #2"))
    S0 -->|"[1,*] w[2,-] m[>,.]"| S1
    S0 -->|"brk [2,3] m[<,<]"| S0
    S0 -->|"[0,1] call ns::r exit #0"| S1
    S0 -->|"[0,1] call ns::r exit #1"| S0
    S0 -->|"[0,1] call ns::r"| S1
    S0 -->|"[0,2] call ns::r exit #0"| S1
    S0 -->|"[0,2] call ns::r exit #1"| S0
    S0 -->|"[0,2] call ns::r"| T_ret
    S0 -->|"[0,3] call ns::r exit #0"| S1
    S0 -->|"[0,3] call ns::r exit #1"| S0
    S0 -->|"[0,3] call ns::r"| T_ret1
    S0 -->|"[3,0] call ns::r exit #0"| S1
    S0 -->|"[3,0] call ns::r exit #1"| S0
    S0 -->|"[3,0] call ns::r"| T_stp
    S0 -->|"[3,1] call ns::r exit #0"| S1
    S0 -->|"[3,1] call ns::r exit #1"| S0
    S0 -->|"[3,1] call ns::r"| T_hlt
    S0 -->|"[3,2] call ns::r exit #0"| S1
    S0 -->|"[3,2] call ns::r exit #1"| S0
    S0 -->|"[*,*] tail ns::t"| T_tail
    S1 -->|"[1,1]"| T_ret
    S1 -->|"[1,2]"| T_ret0
    S1 -->|"[1,3]"| T_stp
    S1 -->|"[2,1]"| T_hlt
    S1 -->|"[0,0]"| T_trap0
    S1 -->|"[0,0]"| T_trap1
    S1 -->|"[0,0]"| T_trap2
"#;
    assert_eq!(every_shape().to_mermaid(), expected);
}

/// A world whose rows do merge: two call rows that differ only in their
/// first cell (the call draws its two exits and its `then`, all three
/// merged), and two wildcard rows with the same write, with an exact row
/// between them that does not stand in their way.
fn merging_world() -> IrWorld {
    let call = || IrTransition::CallThen {
        target: "ns::r".into(),
        binding: Vec::new(),
        exits: vec![1, 0],
        then: Some(IrThen::Goto { state: 1 }),
    };
    let write = |cell: IrCell| {
        let mut r = rule(vec![cell, ANY], IrTransition::Goto { state: 1 });
        r.write = Some(vec![IrWrite::Index { index: 1 }, IrWrite::Keep]);
        r
    };
    world(
        2,
        vec![
            vec![
                rule(vec![ix(1), ix(0)], call()),
                write(ix(3)),
                rule(vec![ix(2), ix(0)], call()),
                write(ix(0)),
            ],
            vec![rule(vec![ANY, ANY], IrTransition::Stop)],
        ],
    )
}

/// The merged view, derived by hand from the rule: the call rows merge
/// into `{1,2}` in their first cell and every arrow the call draws carries
/// the merged label; the two writing rows merge into `{0,3}`, drawn where
/// the first of them stood.
///
/// Mutation: draw a merged row at its LAST raw row's position — the call
/// arrows then follow the `w[1,-]` arrow and the literal no longer
/// matches.
#[test]
fn the_merged_view_draws_each_merged_row_once() {
    let expected = r#"flowchart TD
    S0["s0"]
    S1["s1"]
    T_stp(("stp"))
    S0 -->|"[{1,2},0] call ns::r exit #0"| S1
    S0 -->|"[{1,2},0] call ns::r exit #1"| S0
    S0 -->|"[{1,2},0] call ns::r"| S1
    S0 -->|"[{0,3},*] w[1,-]"| S1
    S1 -->|"[*,*]"| T_stp
"#;
    assert_eq!(merging_world().to_mermaid_view(GraphView::Merged), expected);
}

/// The shape view counts ROWS per pair of states, not arrows: a call row
/// reaching `S1` through both its `then` and an exit counts once.
///
/// Mutation: count arrows instead of distinct rows and `S0 → S1` reads 6.
#[test]
fn the_shape_view_counts_rows_per_state_pair() {
    let expected = r#"flowchart TD
    S0["s0"]
    S1["s1"]
    T_stp(("stp"))
    S0 -->|"4"| S1
    S0 -->|"2"| S0
    S1 -->|"1"| T_stp
"#;
    assert_eq!(merging_world().to_mermaid_view(GraphView::Shape), expected);
}

/// The default view is the merged one, and `to_mermaid` stays the raw one.
///
/// Mutation: make `Raw` the `GraphView` default.
#[test]
fn merged_is_the_default_view() {
    let w = merging_world();
    assert_eq!(
        w.to_mermaid_view(GraphView::default()),
        w.to_mermaid_view(GraphView::Merged)
    );
    assert_eq!(w.to_mermaid(), w.to_mermaid_view(GraphView::Raw));
    assert_ne!(w.to_mermaid(), w.to_mermaid_view(GraphView::Merged));
}

// ---------------------------------------------------------------------------
// Losslessness.
// ---------------------------------------------------------------------------

/// A row's identity for the multiset comparison: everything a raw arrow
/// renders from, pattern included.
fn row_key(
    pattern: &[IrCell],
    write: &Option<Vec<IrWrite>>,
    moves: &Option<Vec<IrMove>>,
    debugger: bool,
    transition: &IrTransition,
) -> String {
    format!("{pattern:?} {write:?} {moves:?} {debugger} {transition:?}")
}

fn partial(p: &[IrCell]) -> bool {
    p.contains(&IrCell::Wildcard) && p.iter().any(|c| *c != IrCell::Wildcard)
}

fn overlap(a: &[IrCell], b: &[IrCell]) -> bool {
    a.iter()
        .zip(b)
        .all(|(x, y)| *x == IrCell::Wildcard || *y == IrCell::Wildcard || x == y)
}

/// Everything the merged view promises about one state's rules, checked
/// against the raw rules themselves:
///
/// 0. every read cell of a merged row is `*` or a set (ascending,
///    distinct, non-empty);
/// 1. expanding every merged row gives back the raw rows, as a multiset;
/// 2. each merged row's recorded raw rows are exactly the ones it expands
///    to, and together they cover every raw row once;
/// 3. merged rows come in the order of their first raw row;
/// 4. two overlapping wildcard-bearing raw rows keep their relative order;
/// 5. the result is a fixpoint — no two merged rows could merge again.
fn check_state(rules: &[IrRule]) -> Result<(), String> {
    let merged: Vec<MergedRow> = merge_rows(rules);
    let mut raw: BTreeMap<String, usize> = BTreeMap::new();
    for r in rules {
        *raw.entry(row_key(
            &r.pattern,
            &r.write,
            &r.moves,
            r.debugger,
            &r.transition,
        ))
        .or_default() += 1;
    }
    let mut expanded: BTreeMap<String, usize> = BTreeMap::new();
    let mut arrow_of = vec![usize::MAX; rules.len()];
    for (k, m) in merged.iter().enumerate() {
        for cell in &m.pattern {
            if let ReadCell::Set(s) = cell
                && (s.is_empty() || s.windows(2).any(|w| w[0] >= w[1]))
            {
                return Err(format!(
                    "merged row {k} holds a cell that is not a set: {s:?}"
                ));
            }
        }
        let patterns = m.expand();
        for p in &patterns {
            *expanded
                .entry(row_key(p, &m.write, &m.moves, m.debugger, &m.transition))
                .or_default() += 1;
        }
        let mut own: Vec<Vec<IrCell>> = m.rows.iter().map(|&i| rules[i].pattern.clone()).collect();
        let mut back = patterns.clone();
        own.sort_by_key(|p| format!("{p:?}"));
        back.sort_by_key(|p| format!("{p:?}"));
        if own != back {
            return Err(format!(
                "merged row {k} expands to {back:?}, recorded {own:?}"
            ));
        }
        for &i in &m.rows {
            if arrow_of[i] != usize::MAX {
                return Err(format!("raw row {i} is in two merged rows"));
            }
            arrow_of[i] = k;
        }
    }
    if raw != expanded {
        return Err(format!(
            "expand(merged) != raw:\n{expanded:#?}\nvs\n{raw:#?}"
        ));
    }
    if arrow_of.contains(&usize::MAX) {
        return Err("a raw row is in no merged row".into());
    }
    for pair in merged.windows(2) {
        if pair[0].rows[0] >= pair[1].rows[0] {
            return Err("merged rows out of first-row order".into());
        }
    }
    for p in 0..rules.len() {
        for q in p + 1..rules.len() {
            let (a, b) = (&rules[p].pattern, &rules[q].pattern);
            if partial(a) && partial(b) && overlap(a, b) && arrow_of[p] > arrow_of[q] {
                return Err(format!("overlapping rows {p} and {q} drawn out of order"));
            }
        }
    }
    // A fixpoint: no two merged rows could still merge.
    let is_partial = |m: &MergedRow| {
        m.pattern.contains(&ReadCell::Any) && m.pattern.iter().any(|c| *c != ReadCell::Any)
    };
    let overlaps = |x: &MergedRow, y: &MergedRow| {
        x.pattern.iter().zip(&y.pattern).all(|(a, b)| match (a, b) {
            (ReadCell::Set(s), ReadCell::Set(t)) => s.iter().any(|v| t.contains(v)),
            _ => true,
        })
    };
    for x in 0..merged.len() {
        for y in x + 1..merged.len() {
            let (mx, my) = (&merged[x], &merged[y]);
            if (&mx.write, &mx.moves, mx.debugger, &mx.transition)
                != (&my.write, &my.moves, my.debugger, &my.transition)
            {
                continue;
            }
            let differing: Vec<usize> = (0..mx.pattern.len())
                .filter(|&c| mx.pattern[c] != my.pattern[c])
                .collect();
            let [c] = differing.as_slice() else { continue };
            let (ReadCell::Set(s), ReadCell::Set(t)) = (&mx.pattern[*c], &my.pattern[*c]) else {
                continue;
            };
            let blocked = is_partial(my)
                && merged[x + 1..y]
                    .iter()
                    .any(|z| is_partial(z) && overlaps(z, my));
            if !blocked && s.iter().all(|v| !t.contains(v)) {
                return Err(format!("merged rows {x} and {y} could still merge"));
            }
        }
    }
    Ok(())
}

/// The `[1,2]`/`[2,1]` trap: two rows that differ in two cells stay two
/// arrows, since a per-cell union would invent `[1,1]` and `[2,2]`.
///
/// Mutation: let a bucket ignore a second cell as well (merge rows that
/// differ in two cells) and one arrow `[{1,2},{1,2}]` comes out.
#[test]
fn rows_differing_in_two_cells_never_merge() {
    let rules = vec![
        rule(vec![ix(1), ix(2)], IrTransition::Stop),
        rule(vec![ix(2), ix(1)], IrTransition::Stop),
    ];
    let labels: Vec<String> = merge_rows(&rules).iter().map(merged_label).collect();
    assert_eq!(labels, ["[1,2]", "[2,1]"]);
    check_state(&rules).unwrap();
}

/// The full square does merge, one cell at a time, to `[{1,2},{1,2}]`:
/// merged rows merge again.
///
/// Mutation: only ever merge in the first cell; two arrows remain.
#[test]
fn merged_rows_merge_again() {
    let rules = vec![
        rule(vec![ix(1), ix(1)], IrTransition::Stop),
        rule(vec![ix(2), ix(1)], IrTransition::Stop),
        rule(vec![ix(1), ix(2)], IrTransition::Stop),
        rule(vec![ix(2), ix(2)], IrTransition::Stop),
    ];
    let labels: Vec<String> = merge_rows(&rules).iter().map(merged_label).collect();
    assert_eq!(labels, ["[{1,2},{1,2}]"]);
    check_state(&rules).unwrap();
}

/// Merging runs to a fixpoint, not one sweep over the cells: the rows
/// `[1,*,0]` and `[2,*,0]` cannot merge at first, because `[2,1,*]` sits
/// between them and overlaps the second; once that row has merged upward
/// into `[2,2,*]` (a merge in the SECOND cell, later in the same sweep),
/// nothing stands between them any more and the next sweep merges them.
///
/// Mutation: stop after one sweep; three arrows remain.
#[test]
fn merging_repeats_until_nothing_changes() {
    let (k, k2) = (
        || IrTransition::Goto { state: 0 },
        || IrTransition::Goto { state: 1 },
    );
    let rules = vec![
        rule(vec![ix(2), ix(2), ANY], k2()),
        rule(vec![ix(1), ANY, ix(0)], k()),
        rule(vec![ix(2), ix(1), ANY], k2()),
        rule(vec![ix(2), ANY, ix(0)], k()),
    ];
    let labels: Vec<String> = merge_rows(&rules).iter().map(merged_label).collect();
    assert_eq!(labels, ["[2,{1,2},*]", "[{1,2},*,0]"]);
    check_state(&rules).unwrap();
}

/// Rows merge only under one transition: the same call bound two ways
/// stays two arrows even though the two labels read alike.
///
/// Mutation: key rows on the rendered label instead of the transition.
#[test]
fn rows_with_different_bindings_never_merge() {
    use mtc_turing_machine::ir::IrTapeBinding;
    let call = |caller_tape: u32| IrTransition::CallThen {
        target: "ns::r".into(),
        binding: vec![IrTapeBinding {
            caller_tape,
            pairs: Vec::new(),
            param: None,
            map_written: false,
            open: false,
        }],
        exits: Vec::new(),
        then: Some(IrThen::Stop),
    };
    let rules = vec![rule(vec![ix(1)], call(0)), rule(vec![ix(2)], call(1))];
    assert_eq!(merge_rows(&rules).len(), 2);
}

/// Random states: a few tapes over small alphabets, rows drawn from a few
/// actions, so merges, near-merges, overlaps and exact duplicates are all
/// common.
fn arb_state() -> impl Strategy<Value = Vec<IrRule>> {
    (1usize..=4, 2u32..=4).prop_flat_map(|(arity, card)| {
        let cell = prop_oneof![
            1 => Just(IrCell::Wildcard),
            3 => (0..card).prop_map(|index| IrCell::Index { index }),
        ];
        let row = (
            prop::collection::vec(cell, arity),
            0u32..3,
            any::<bool>(),
            0u32..2,
        )
            .prop_map(|(pattern, target, debugger, write)| {
                let mut r = rule(pattern, IrTransition::Goto { state: target });
                r.debugger = debugger && target == 0;
                if write == 1 {
                    r.write = Some(vec![IrWrite::Keep; r.pattern.len()]);
                }
                r
            });
        prop::collection::vec(row, 0..24)
    })
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(2000))]

    /// Mutations that redden it: merge rows differing in two cells;
    /// union a `*` cell with a set; drop the disjointness requirement
    /// (exact duplicate rows collapse into one); drop the order guard.
    #[test]
    fn merging_is_lossless_on_generated_states(rules in arb_state()) {
        if let Err(e) = check_state(&rules) {
            prop_assert!(false, "{e}\nrules: {rules:#?}");
        }
    }
}

/// Every `.tmc` the crate ships, compiled at both levels: every state of
/// every world satisfies the same checks.
#[test]
fn merging_is_lossless_on_the_shipped_corpus() {
    let mut files: Vec<PathBuf> = Vec::new();
    for dir in ["tests/golden", "src/stdlib", "../../docs/examples"] {
        for entry in fs::read_dir(dir).expect("corpus directory") {
            let path = entry.expect("entry").path();
            if path.is_dir() {
                for sub in fs::read_dir(&path).expect("example directory") {
                    files.push(sub.expect("entry").path());
                }
            } else {
                files.push(path);
            }
        }
    }
    files.retain(|p| p.extension().is_some_and(|e| e == "tmc"));
    let mut states = 0;
    let mut merged_any = false;
    for path in &files {
        let src = fs::read_to_string(path).expect("readable");
        for opt_level in [OptLevel::O0, OptLevel::O1] {
            let out = compile(
                &src,
                CompileOptions {
                    opt_level,
                    ..Default::default()
                },
            )
            .unwrap_or_else(|e| panic!("{}: {e:?}", path.display()));
            for w in &out.ir.worlds {
                for s in &w.states {
                    check_state(&s.rules).unwrap_or_else(|e| {
                        panic!("{} {} {}: {e}", path.display(), w.name, s.name)
                    });
                    merged_any |= merge_rows(&s.rules).len() < s.rules.len();
                    states += 1;
                }
            }
        }
    }
    assert!(files.len() >= 10, "corpus walk found {} files", files.len());
    assert!(
        states > 100 && merged_any,
        "{states} states, merged: {merged_any}"
    );
}

// ---------------------------------------------------------------------------
// The notation, held to the table in docs/formats.md (graph label notation).
// ---------------------------------------------------------------------------

/// A raw label as the table writes it — `[p] w[w] m[m]` — as a rule bound
/// for state 0.
fn parse_raw(label: &str) -> IrRule {
    let mut parts = label.split(' ');
    let pat = parts.next().unwrap();
    let pattern = pat
        .trim_start_matches('[')
        .trim_end_matches(']')
        .split(',')
        .map(|c| match c {
            "*" => IrCell::Wildcard,
            n => IrCell::Index {
                index: n.parse().unwrap(),
            },
        })
        .collect();
    let mut r = rule(pattern, IrTransition::Goto { state: 0 });
    for part in parts {
        let body = &part[2..part.len() - 1];
        if part.starts_with("w[") {
            r.write = Some(
                body.split(',')
                    .map(|c| match c {
                        "-" => IrWrite::Keep,
                        n => IrWrite::Index {
                            index: n.parse().unwrap(),
                        },
                    })
                    .collect(),
            );
        } else if part.starts_with("m[") {
            r.moves = Some(
                body.split(',')
                    .map(|c| match c {
                        "<" => IrMove::Left,
                        ">" => IrMove::Right,
                        _ => IrMove::Stay,
                    })
                    .collect(),
            );
        } else {
            panic!("unexpected label part `{part}`");
        }
    }
    r
}

/// The code spans of one table cell.
fn spans(cell: &str) -> Vec<String> {
    cell.split('`')
        .enumerate()
        .filter(|(i, _)| i % 2 == 1)
        .map(|(_, s)| s.to_string())
        .collect()
}

fn notation_table() -> Vec<(Vec<String>, Vec<String>)> {
    let doc = fs::read_to_string("../../docs/formats.md").expect("docs/formats.md");
    let section = doc
        .split("### Graph label notation")
        .nth(1)
        .expect("the notation section");
    let mut rows = Vec::new();
    for line in section.lines() {
        let Some(line) = line.strip_prefix("| `") else {
            continue;
        };
        let full = format!("`{line}");
        let cols: Vec<&str> = full.split(" | ").collect();
        assert_eq!(cols.len(), 2, "a golden row has two columns: {line}");
        rows.push((spans(cols[0]), spans(cols[1])));
    }
    rows
}

/// Every golden case on the page is what the renderer produces, and the
/// raw column is the raw view's own text.
///
/// Mutation: write a two-value run as `1–2`, or compress two identical
/// cells — the table's `{1,2}` / `[*,*]` rows fail.
#[test]
fn the_documented_notation_table_is_what_the_renderer_writes() {
    let table = notation_table();
    assert!(table.len() >= 16, "found {} golden rows", table.len());
    for (raw, merged) in &table {
        let rules: Vec<IrRule> = raw.iter().map(|l| parse_raw(l)).collect();
        let w = world(rules[0].pattern.len() as u32, vec![rules.clone()]);
        let raw_view = w.to_mermaid_view(GraphView::Raw);
        for l in raw {
            assert!(
                raw_view.contains(&format!("\"{l}\"")),
                "{l} not in\n{raw_view}"
            );
        }
        check_state(&rules).unwrap();
        let got: Vec<String> = merge_rows(&rules).iter().map(merged_label).collect();
        assert_eq!(&got, merged, "rows {raw:?}");
    }
}

// ---------------------------------------------------------------------------
// The CLI surface.
// ---------------------------------------------------------------------------

fn args(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| s.to_string()).collect()
}

fn scratch_ir(program: &IrProgram) -> String {
    static N: AtomicU32 = AtomicU32::new(0);
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!(
        "ir_graph_{}_{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join("w.ir.json");
    fs::write(&path, program.to_json()).unwrap();
    path.to_str().unwrap().to_string()
}

/// `tmt ir graph` prints the merged view by default, the raw one under
/// `--raw`, the shape one under `--shape`, and refuses both flags at once.
///
/// Mutation: ignore `--raw` in the CLI; the raw text no longer comes out.
#[test]
fn the_cli_picks_the_view_by_flag() {
    let w = merging_world();
    let path = scratch_ir(&IrProgram {
        version: mtc_turing_machine::ir::TM_IR_VERSION,
        worlds: vec![w.clone()],
        entry_world: None,
    });
    let run = |extra: &[&str]| {
        let mut a = args(&["ir", "graph", &path]);
        a.extend(args(extra));
        execute(&a)
    };
    let with = |view| format!("%% w\n{}\n", w.to_mermaid_view(view));
    assert_eq!(run(&[]).unwrap().stdout, with(GraphView::Merged));
    assert_eq!(run(&["--raw"]).unwrap().stdout, with(GraphView::Raw));
    assert_eq!(run(&["--shape"]).unwrap().stdout, with(GraphView::Shape));
    let err = run(&["--raw", "--shape"]).unwrap_err();
    assert!(err.contains("mutually exclusive"), "{err}");
}
