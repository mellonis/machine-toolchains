//! End-to-end tests for `tmt interface`, the fifteenth subcommand
//! (docs/tmt/cli.md (interface)): the two-arm header printer over
//! `crate::header`, driven only through the public CLI — the printer
//! itself is crate-private, so every assertion here goes through
//! `mtc_turing_machine::cli::execute`, exactly as a real invocation would.

use std::collections::BTreeMap;

use mtc_turing_machine::cli::execute;
use mtc_turing_machine::compiler::{CompileOptions, compile};
use mtc_turing_machine::optimizer::OptLevel;
use mtc_turing_machine::stdlib;

fn args(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| s.to_string()).collect()
}

/// A fresh, per-call fixture directory under `CARGO_TARGET_TMPDIR`, named
/// uniquely by process id + an atomic counter — copied verbatim from
/// `tests/mode_equivalence.rs::scratch` (docs/superpowers plan's "temp
/// paths in tests" rule): a literal, non-unique directory name is shared
/// by every concurrently running `cargo test` invocation that targets the
/// same `CARGO_TARGET_TMPDIR`, so two such invocations racing on the
/// identical path can interleave their non-atomic `fs::write`s.
fn scratch(name: &str) -> std::path::PathBuf {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("{name}-{}-{n}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A local (non-exported) alphabet and routine alongside an exported
/// alphabet, an exported routine, and an exported graph the routine
/// grafts — the shape `the_source_arm_prints_every_exported_declaration`
/// and `the_object_arm_prints_signatures_and_alphabets_but_no_graphs`
/// both compile and render.
const DECL_FIXTURE: &str = "\
alphabet localAlpha { '_', 'x' }
export alphabet bits { '_', '0', '1' }

routine localHelper(tape t: localAlpha) {
  entry state s { [*] -> return; }
}

export graph plusOneGraph(tape num: bits writes { '0', '1' }, state done) {
  entry state inc {
    ['1'] -> write ['0'] move [<] goto inc;
    [*]   -> write ['1'] goto done;
  }
}

export routine plusOne(tape num: bits writes { '0', '1' }) {
  entry graft plusOneGraph(num = num, done = return);
}
";

fn run_interface(path: &std::path::Path) -> mtc_turing_machine::cli::CliOutput {
    let out = execute(&args(&["interface", path.to_str().unwrap()]))
        .unwrap_or_else(|e| panic!("interface {}: {e}", path.display()));
    assert_eq!(out.code, 0, "interface {}: {}", path.display(), out.stderr);
    out
}

/// Mutation: printing local declarations too (dropping the `exported`
/// filter on any of `Program::alphabets` / `routines` / `graphs`); the
/// negative half — the assertions that `localAlpha`/`localHelper` are
/// absent — goes red.
#[test]
fn the_source_arm_prints_every_exported_declaration() {
    let dir = scratch("header_source_every_decl");
    let path = dir.join("decl.tmc");
    std::fs::write(&path, DECL_FIXTURE).unwrap();

    let out = run_interface(&path);

    assert!(
        out.stdout
            .contains("export alphabet bits { '_', '0', '1' }"),
        "{}",
        out.stdout
    );
    assert!(
        out.stdout
            .contains("export routine plusOne(tape num: bits writes { '0', '1' });"),
        "{}",
        out.stdout
    );
    assert!(
        out.stdout.contains(
            "export graph plusOneGraph(tape num: bits writes { '0', '1' }, state done) {"
        ),
        "{}",
        out.stdout
    );
    assert!(
        out.stdout.contains("entry state inc {"),
        "graph body missing: {}",
        out.stdout
    );
    assert!(
        !out.stdout.contains("localAlpha"),
        "local alphabet leaked: {}",
        out.stdout
    );
    assert!(
        !out.stdout.contains("localHelper"),
        "local routine leaked: {}",
        out.stdout
    );
}

/// Mutation: falling through to the source arm on an object input; the
/// graph reappears (`export graph`/the graph's own state body would show
/// up in output that should carry signatures and alphabets only).
#[test]
fn the_object_arm_prints_signatures_and_alphabets_but_no_graphs() {
    let dir = scratch("header_object_no_graphs");
    let object = compile(
        DECL_FIXTURE,
        CompileOptions {
            opt_level: OptLevel::O0,
            ..CompileOptions::default()
        },
    )
    .unwrap_or_else(|e| panic!("compile DECL_FIXTURE: {e}"))
    .object;
    let obj_path = dir.join("decl.tmo");
    std::fs::write(&obj_path, object.to_bytes()).unwrap();

    let out = run_interface(&obj_path);

    assert!(
        out.stdout
            .contains("export alphabet bits { '_', '0', '1' }"),
        "{}",
        out.stdout
    );
    assert!(
        out.stdout
            .contains("export routine plusOne(tape num: bits writes { '0', '1' });"),
        "{}",
        out.stdout
    );
    assert!(
        !out.stdout.contains("graph"),
        "a graph leaked into the object arm: {}",
        out.stdout
    );
    assert!(
        !out.stdout.contains("localAlpha") && !out.stdout.contains("localHelper"),
        "a non-exported declaration leaked: {}",
        out.stdout
    );
}

/// Mutation: dispatching on the input's EXTENSION instead of its
/// container magic — pins the repo's standing `sniff()`-not-extension
/// rule (the `tape-block new --from` precedent). A `.tmo` renamed to
/// `.tmc` must still run the object arm: were the mutation applied, the
/// buggy code would try to parse these raw object bytes as `.tmc` source
/// and fail (or, on a byte sequence that happens to be valid UTF-8,
/// produce a parse error long before any header text), never emitting
/// this clean object-arm signature line.
#[test]
fn sniff_not_extension_decides_the_arm() {
    let dir = scratch("header_sniff_not_extension");
    let object = compile(
        DECL_FIXTURE,
        CompileOptions {
            opt_level: OptLevel::O0,
            ..CompileOptions::default()
        },
    )
    .unwrap_or_else(|e| panic!("compile DECL_FIXTURE: {e}"))
    .object;
    // The extension says `.tmc` (source); the magic says otherwise.
    let renamed = dir.join("decl.tmc");
    std::fs::write(&renamed, object.to_bytes()).unwrap();

    let out = run_interface(&renamed);

    assert!(
        out.stdout
            .contains("export routine plusOne(tape num: bits writes { '0', '1' });"),
        "did not run the object arm: {}",
        out.stdout
    );
    assert!(
        !out.stdout.contains("graph"),
        "an object rendered as source (graph body present): {}",
        out.stdout
    );
}

/// Mutation: any printer path that echoes source text rather than
/// rendering from the resolved module (e.g. slicing a signature's own
/// span instead of reconstructing it from `Signature`/`ResolvedTape`) —
/// the two renders below would then differ on comment/whitespace-only
/// input. This is the digest's precondition: a later graph-digest
/// computation over this printer's output requires exactly this
/// property.
#[test]
fn the_printer_is_insensitive_to_comments_and_whitespace() {
    const COMPACT: &str = "\
export alphabet bits { '_', '0', '1' }

namespace mylib {
  export routine plusOne(tape num: bits writes { '0', '1' }) {
    entry state inc {
      ['1'] -> write ['0'] move [<] goto inc;
      [*] -> write ['1'] return;
    }
  }
}
";
    const SPREAD_OUT: &str = "\
// a leading comment
export   alphabet   bits   {
  '_', // the blank
  '0',
  '1'
}


namespace mylib { // trailing comment

  export routine plusOne(
    tape num: bits writes { '0', '1' } // contract
  ) {
    entry state inc {
      ['1'] -> write ['0'] move [<] goto inc; // carry
      [*]   ->             write ['1']       return;
    }
  }

}
";

    let dir = scratch("header_whitespace_insensitive");
    let compact = dir.join("compact.tmc");
    std::fs::write(&compact, COMPACT).unwrap();
    let spread_out = dir.join("spread_out.tmc");
    std::fs::write(&spread_out, SPREAD_OUT).unwrap();

    let a = run_interface(&compact);
    let b = run_interface(&spread_out);

    assert_eq!(a.stdout, b.stdout);
    assert!(
        a.stdout
            .contains("export routine plusOne(tape num: bits writes { '0', '1' });"),
        "{}",
        a.stdout
    );
}

/// Extract `qualified::routine::name -> "export routine …;"` from one
/// rendered header: a small brace-depth walk that tracks the CURRENT
/// namespace path (pushed by a `namespace NAME {` line, popped by a bare
/// `}`) while treating every other line ending in `{` — a graph's or a
/// state's own opening brace — as an anonymous scope that still pushes
/// and pops correctly without contributing to the namespace path. Good
/// enough for text this printer itself produced; not a general `.tmc`
/// parser.
fn qualified_routines(header: &str) -> BTreeMap<String, String> {
    let mut stack: Vec<Option<String>> = Vec::new();
    let mut out = BTreeMap::new();
    for raw in header.lines() {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Some(name) = trimmed
            .strip_prefix("namespace ")
            .and_then(|s| s.strip_suffix(" {"))
        {
            stack.push(Some(name.to_string()));
            continue;
        }
        if trimmed == "}" {
            stack.pop();
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("export routine ") {
            let name = rest.split('(').next().unwrap_or(rest);
            let ns: Vec<&str> = stack.iter().filter_map(|s| s.as_deref()).collect();
            let qualified = if ns.is_empty() {
                name.to_string()
            } else {
                format!("{}::{name}", ns.join("::"))
            };
            out.insert(qualified, trimmed.to_string());
            continue;
        }
        if trimmed.ends_with('{') {
            stack.push(None);
        }
    }
    out
}

/// Mutation: printing `preserves` from the source arm — `invertNumber`
/// (`std::binaryNumbersBare::invertNumber` and its volatile twin, each
/// declaring `preserves { '_' }` with no `writes` clause) diverges,
/// because the object arm has no `preserves` to print back: it only ever
/// carries the EFFECTIVE set. This pins the F1 ruling
/// (docs/formats.md (routine interfaces)) at the surface where it is
/// observable. VERIFIED RED by hand: printing `preserves`'s raw elements
/// instead of the effective set on the source arm's tape signature made
/// this test fail on exactly the two `invertNumber` entries, restored
/// afterward (see the task report).
#[test]
fn the_two_arms_agree_on_every_stdlib_routine() {
    let dir = scratch("header_two_arms_stdlib");
    let src_path = dir.join("std.tmc");
    std::fs::write(&src_path, stdlib::SOURCE).unwrap();
    let source_out = run_interface(&src_path);

    let object = compile(
        stdlib::SOURCE,
        CompileOptions {
            opt_level: OptLevel::O0,
            ..CompileOptions::default()
        },
    )
    .unwrap_or_else(|e| panic!("compile stdlib::SOURCE: {e}"))
    .object;
    let obj_path = dir.join("std.tmo");
    std::fs::write(&obj_path, object.to_bytes()).unwrap();
    let object_out = run_interface(&obj_path);

    let source_routines = qualified_routines(&source_out.stdout);
    let object_routines = qualified_routines(&object_out.stdout);

    assert!(
        !source_routines.is_empty(),
        "no routines parsed out of the source-arm header: {}",
        source_out.stdout
    );
    assert_eq!(
        source_routines.len(),
        object_routines.len(),
        "source arm: {:?}\nobject arm: {:?}",
        source_routines.keys().collect::<Vec<_>>(),
        object_routines.keys().collect::<Vec<_>>()
    );
    assert_eq!(source_routines, object_routines);
}

/// Mutation: ignoring `-o` and printing to stdout regardless — the
/// target file would stay empty/absent while stdout still carried the
/// text, which this test's `stdout.is_empty()` and file-content
/// assertions together catch.
#[test]
fn the_o_flag_writes_the_header_to_a_file_instead_of_stdout() {
    let dir = scratch("header_o_flag");
    let src_path = dir.join("mylib.tmc");
    std::fs::write(
        &src_path,
        "export alphabet bits { '_', '0', '1' }\n\
         export routine plusOne(tape num: bits writes { '0', '1' }) { entry state s { [*] -> return; } }\n",
    )
    .unwrap();
    let out_path = dir.join("mylib.tmh");

    let out = execute(&args(&[
        "interface",
        src_path.to_str().unwrap(),
        "-o",
        out_path.to_str().unwrap(),
    ]))
    .unwrap();
    assert_eq!(out.code, 0, "{}", out.stderr);
    assert!(
        out.stdout.is_empty(),
        "text went to stdout as well as the file: {}",
        out.stdout
    );

    let written = std::fs::read_to_string(&out_path).unwrap();
    assert!(
        written.contains("export routine plusOne(tape num: bits writes { '0', '1' });"),
        "{written}"
    );
}

/// Grammar delta for the reader Task 5 lands — a `.tmh` carrying the
/// declaration-only signature shape `parse_reuse` cannot accept yet
/// (`expected '{' to open the body, found ';'`). Un-ignored by that
/// task's own reader (the declarations-only reader), which is why this
/// stays a marker rather than a real assertion for now.
#[test]
#[ignore = "un-ignored by the declarations-only header reader that accepts this shape"]
fn interface_output_reparses_as_a_header() {
    let dir = scratch("header_reparses");
    let src_path = dir.join("mylib.tmc");
    std::fs::write(
        &src_path,
        "export alphabet bits { '_', '0', '1' }\n\
         export routine plusOne(tape num: bits writes { '0', '1' }) { entry state s { [*] -> return; } }\n",
    )
    .unwrap();
    let header = run_interface(&src_path).stdout;

    let header_path = dir.join("mylib.tmh");
    std::fs::write(&header_path, &header).unwrap();

    // Placeholder for the reader this task hands off to: re-running
    // `interface` over the header itself should reproduce it unchanged.
    let reparsed = run_interface(&header_path);
    assert_eq!(reparsed.stdout, header);
}
