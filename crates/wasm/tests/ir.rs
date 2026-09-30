//! The IR channel: `ir` is the document `compile --emit-ir=STAGE` writes,
//! `ir_graph` the text `ir graph` prints, split at its `%% name` headers —
//! both held byte for byte to the CLIs themselves.

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};

use mtc_wasm::inner::Lang;
use mtc_wasm::inner::ir::{GraphView, IrError, IrStage, ir, ir_graph};

/// `scan`'s two moving rows share an action, so the merged view folds
/// them; `hop` only forwards, which the `-O1` pipeline threads away, so
/// the lowered and final stages differ there.
const TMC: &str = "alphabet ab { '_', 'a', 'b' }\n\nmachine {\n  tape main: ab;\n\n  entry state scan {\n    ['a'] -> move [>] goto scan;\n    ['b'] -> move [>] goto scan;\n    ['_'] -> goto hop;\n  }\n  state hop { [*] -> goto done; }\n  state done { [*] -> stop; }\n}\n";
/// A check whose arms agree (`2: check(3, 3)`) — the one parallel pair a
/// CFG can carry, folded to a goto at `-O1`.
const PMC: &str = "main() {\n    1: right(2);\n    2: check(3, 3);\n    3: mark(!);\n}\n";

fn scratch() -> PathBuf {
    static N: AtomicU32 = AtomicU32::new(0);
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!(
        "wasm_ir_{}_{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn args(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| s.to_string()).collect()
}

/// What the CLI writes for `compile -O{level} --emit-ir={stage}`, and what
/// `ir graph [flag]` then prints over it.
fn cli(lang: Lang, level: u8, stage: &str, view_flag: Option<&str>) -> (String, String) {
    let dir = scratch();
    let (ext, obj) = match lang {
        Lang::Pmc => ("pmc", "p.pmo"),
        _ => ("tmc", "p.tmo"),
    };
    let src = dir.join(format!("p.{ext}"));
    fs::write(&src, if lang == Lang::Pmc { PMC } else { TMC }).unwrap();
    let obj = dir.join(obj);
    let compile = args(&[
        "compile",
        &format!("-O{level}"),
        &format!("--emit-ir={stage}"),
        "-o",
        obj.to_str().unwrap(),
        src.to_str().unwrap(),
    ]);
    let json = dir.join("p.ir.json");
    let mut graph = args(&["ir", "graph", json.to_str().unwrap()]);
    graph.extend(view_flag.map(str::to_string));
    let run = |a: &[String]| match lang {
        Lang::Pmc => mtc_post_machine::cli::execute(a).map(|o| o.stdout),
        _ => mtc_turing_machine::cli::execute(a).map(|o| o.stdout),
    };
    run(&compile).unwrap();
    let doc = fs::read_to_string(&json).unwrap();
    (doc, run(&graph).unwrap())
}

/// Mutation: hand back the final IR for `lowered` (or the reverse); the
/// `-O1` comparisons fail for the stage the CLI does not match.
#[test]
fn ir_is_the_document_emit_ir_writes() {
    for lang in [Lang::Pmc, Lang::Tmc] {
        for level in [0, 1] {
            for (stage, name) in [(IrStage::Lowered, "lowered"), (IrStage::Final, "final")] {
                let (doc, _) = cli(lang, level, name, None);
                assert_eq!(
                    ir(
                        lang,
                        if lang == Lang::Pmc { PMC } else { TMC },
                        level,
                        stage
                    )
                    .unwrap(),
                    doc,
                    "{lang:?} -O{level} {name}"
                );
            }
        }
    }
    // The stage matters where the optimizer ran, and the default is the
    // lowered one.
    assert_ne!(
        ir(Lang::Tmc, TMC, 1, IrStage::Lowered).unwrap(),
        ir(Lang::Tmc, TMC, 1, IrStage::Final).unwrap()
    );
    assert_eq!(IrStage::default(), IrStage::Lowered);
}

/// Mutation: render every entry in the raw view; the merged and shape
/// comparisons fail.
#[test]
fn ir_graph_is_what_ir_graph_prints_split_by_world() {
    for lang in [Lang::Pmc, Lang::Tmc] {
        let src = if lang == Lang::Pmc { PMC } else { TMC };
        for (view, flag) in [
            (GraphView::Merged, None),
            (GraphView::Shape, Some("--shape")),
            (GraphView::Raw, Some("--raw")),
        ] {
            let (_, printed) = cli(lang, 0, "lowered", flag);
            let graphs = ir_graph(lang, src, 0, IrStage::Lowered, view).unwrap();
            assert!(!graphs.is_empty());
            let joined: String = graphs
                .iter()
                .map(|(name, mermaid)| format!("%% {name}\n{mermaid}\n"))
                .collect();
            assert_eq!(joined, printed, "{lang:?} {view:?}");
        }
    }
    // The views really differ on these programs, and merged is the default.
    let tm = |v| ir_graph(Lang::Tmc, TMC, 0, IrStage::Lowered, v).unwrap();
    assert_ne!(tm(GraphView::Merged), tm(GraphView::Raw));
    assert!(tm(GraphView::Merged)[0].1.contains("[{1,2}] m[>]"));
    let pm = ir_graph(Lang::Pmc, PMC, 0, IrStage::Lowered, GraphView::default()).unwrap();
    assert!(pm[0].1.contains("{MF,!MF}"), "{}", pm[0].1);
}

/// Mutation: let an assembly language through to the compiler.
#[test]
fn assembly_languages_have_no_ir() {
    for lang in [Lang::Pma, Lang::Tma] {
        assert_eq!(ir(lang, "", 0, IrStage::Lowered), Err(IrError::NoIr(lang)));
        assert_eq!(
            ir_graph(lang, "", 0, IrStage::Lowered, GraphView::Merged),
            Err(IrError::NoIr(lang))
        );
    }
}

/// A source that does not compile is its fatal diagnostic.
///
/// Mutation: report a compile failure as an empty document.
#[test]
fn a_broken_source_is_its_fatal_diagnostic() {
    let Err(IrError::Compile(d)) = ir(Lang::Tmc, "machine {", 0, IrStage::Final) else {
        panic!("a broken .tmc has no IR");
    };
    assert!(!d.code.is_empty() && !d.message.is_empty(), "{d:?}");
    let Err(IrError::Compile(_)) = ir(Lang::Pmc, "main() { nope", 0, IrStage::Final) else {
        panic!("a broken .pmc has no IR");
    };
}

/// The option spellings the JS side accepts, and nothing else.
///
/// Mutation: accept `after:inline` as a stage.
#[test]
fn stage_and_view_names() {
    assert_eq!(IrStage::parse("lowered"), Some(IrStage::Lowered));
    assert_eq!(IrStage::parse("final"), Some(IrStage::Final));
    assert_eq!(IrStage::parse("after:inline"), None);
    assert_eq!(GraphView::parse("merged"), Some(GraphView::Merged));
    assert_eq!(GraphView::parse("shape"), Some(GraphView::Shape));
    assert_eq!(GraphView::parse("raw"), Some(GraphView::Raw));
    assert_eq!(GraphView::parse("dense"), None);
}
