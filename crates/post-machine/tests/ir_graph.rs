//! `pmt ir graph`'s three views over one function: the edge-for-edge raw
//! view, the merged view and the per-block-pair shape view
//! (docs/pmt/cli.md (pmt ir)).

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};

use mtc_post_machine::cli::execute;
use mtc_post_machine::ir::{IrBlock, IrFunction, IrOp, IrProgram, IrTerm};
use mtc_post_machine::ir_graph::GraphView;

fn block(id: u32, labels: Vec<u32>, ops: Vec<IrOp>, term: IrTerm) -> IrBlock {
    IrBlock {
        id,
        labels,
        line: 1,
        ops,
        term,
        term_line: 1,
    }
}

/// Every node and edge shape the renderer draws: labels, each op kind,
/// the three block-ending terminators, a fall-through, a goto, a check
/// with distinct arms and one whose arms agree.
fn every_shape() -> IrFunction {
    IrFunction {
        name: "main".into(),
        line: 1,
        local: false,
        blocks: vec![
            block(
                0,
                vec![1, 2],
                vec![
                    IrOp::Lft { line: 1 },
                    IrOp::Rgt { line: 1 },
                    IrOp::Wr { index: 1, line: 1 },
                    IrOp::WrLft { index: 0, line: 1 },
                    IrOp::WrRgt { index: 1, line: 1 },
                    IrOp::Brk { line: 1 },
                    IrOp::Call {
                        name: "std::f".into(),
                        line: 1,
                    },
                ],
                IrTerm::FallThrough { to: 1 },
            ),
            block(
                1,
                vec![],
                vec![],
                IrTerm::Check {
                    marked: 2,
                    blank: 3,
                },
            ),
            block(
                2,
                vec![3],
                vec![],
                IrTerm::Check {
                    marked: 4,
                    blank: 4,
                },
            ),
            block(3, vec![], vec![], IrTerm::Goto { to: 5 }),
            block(4, vec![], vec![], IrTerm::Return),
            block(5, vec![], vec![], IrTerm::TailCall { name: "g".into() }),
            block(6, vec![], vec![], IrTerm::Halt),
        ],
    }
}

/// The raw view is today's rendering, byte for byte.
///
/// Mutation: change any edge format in the renderer (write `-->|MF|` as
/// `-->|mf|`, say) and the literal no longer matches.
#[test]
fn the_raw_view_is_pinned_byte_for_byte() {
    let expected = r#"flowchart TD
    B0["1:<br/>2:<br/>lft<br/>rgt<br/>wr 1<br/>wrl 0<br/>wrr 1<br/>brk<br/>call @std::f"]
    B1["(empty)"]
    B2["3:"]
    B3["(empty)"]
    B4["ret"]
    B5["jmp @g"]
    B6["hlt"]
    B0 --> B1
    B1 -->|MF| B2
    B1 -->|!MF| B3
    B2 -->|MF| B4
    B2 -->|!MF| B4
    B3 -->|goto| B5
"#;
    assert_eq!(every_shape().to_mermaid(), expected);
}

const NODES: &str = r#"flowchart TD
    B0["1:<br/>2:<br/>lft<br/>rgt<br/>wr 1<br/>wrl 0<br/>wrr 1<br/>brk<br/>call @std::f"]
    B1["(empty)"]
    B2["3:"]
    B3["(empty)"]
    B4["ret"]
    B5["jmp @g"]
    B6["hlt"]
"#;

/// The merged view differs from the raw one only where a check's two arms
/// reach one block: that pair is one edge, labelled with both readings of
/// the match flag as a set.
///
/// Mutation: merge nothing (render the check pair as two edges).
#[test]
fn the_merged_view_folds_a_check_whose_arms_agree() {
    let expected = format!(
        "{NODES}{}",
        r#"    B0 --> B1
    B1 -->|MF| B2
    B1 -->|!MF| B3
    B2 -->|"{MF,!MF}"| B4
    B3 -->|goto| B5
"#
    );
    assert_eq!(every_shape().to_mermaid_view(GraphView::Merged), expected);
    assert_eq!(
        every_shape().to_mermaid_view(GraphView::default()),
        expected,
        "merged is the default view"
    );
}

/// The shape view draws one edge per pair of blocks, labelled with the
/// number of raw edges it stands for.
///
/// Mutation: label the agreeing check pair `1`.
#[test]
fn the_shape_view_counts_edges_per_block_pair() {
    let expected = format!(
        "{NODES}{}",
        r#"    B0 -->|"1"| B1
    B1 -->|"1"| B2
    B1 -->|"1"| B3
    B2 -->|"2"| B4
    B3 -->|"1"| B5
"#
    );
    assert_eq!(every_shape().to_mermaid_view(GraphView::Shape), expected);
}

fn args(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| s.to_string()).collect()
}

fn scratch(file: &str, text: &str) -> String {
    static N: AtomicU32 = AtomicU32::new(0);
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!(
        "ir_graph_{}_{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join(file);
    fs::write(&path, text).unwrap();
    path.to_str().unwrap().to_string()
}

/// `pmt ir graph` picks the view by flag on either input kind — the view
/// flags choose how to draw, not what to compile, so a `.ir.json` takes
/// them as readily as a `.pmc` — and refuses both flags at once.
///
/// Mutation: route `--raw` through the `.pmc`-only flag check; the
/// `.ir.json` arm errors.
#[test]
fn the_cli_picks_the_view_by_flag() {
    let f = every_shape();
    let json = scratch(
        "f.ir.json",
        &IrProgram {
            version: mtc_post_machine::ir::IR_VERSION,
            functions: vec![f.clone()],
        }
        .to_json(),
    );
    let run = |input: &str, extra: &[&str]| {
        let mut a = args(&["ir", "graph", input]);
        a.extend(args(extra));
        execute(&a)
    };
    let with = |view| format!("%% main\n{}\n", f.to_mermaid_view(view));
    assert_eq!(run(&json, &[]).unwrap().stdout, with(GraphView::Merged));
    assert_eq!(run(&json, &["--raw"]).unwrap().stdout, with(GraphView::Raw));
    assert_eq!(
        run(&json, &["--shape"]).unwrap().stdout,
        with(GraphView::Shape)
    );
    let err = run(&json, &["--raw", "--shape"]).unwrap_err();
    assert!(err.contains("mutually exclusive"), "{err}");

    // `1: check(2, 2); 2: right(!);` — at -O0 the check keeps both arms,
    // which reach one block.
    let pmc = scratch(
        "same.pmc",
        "main() {\n    1: check(2, 2);\n    2: right(!);\n}\n",
    );
    let merged = run(&pmc, &[]).unwrap().stdout;
    assert!(merged.contains(r#"-->|"{MF,!MF}"|"#), "{merged}");
    let raw = run(&pmc, &["--raw", "-O0"]).unwrap().stdout;
    assert!(raw.contains("-->|MF|") && raw.contains("-->|!MF|"), "{raw}");
}
