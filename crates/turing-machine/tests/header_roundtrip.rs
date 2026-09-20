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

/// A routine over a NON-exported alphabet is legal, and both arms must
/// render it: the source arm as a plain `alphabet` (no `export`), the
/// object arm as a synthesized `<routine>__<param>` declaration — never
/// as an error. Mutation: erroring on the unmatched glyph list on the
/// object arm (this task's own prior design) — the object arm would fail
/// to render `touch`'s header at all instead of exiting 0 with a
/// synthesized `alphabet touch__t { … }`.
#[test]
fn a_routine_over_a_local_alphabet_renders_on_both_arms() {
    const LOCAL_ALPHABET_FIXTURE: &str = "\
alphabet localBits { '_', '0', '1' }

export routine touch(tape t: localBits writes { '1' }) {
  entry state s { [*] -> write ['1'] return; }
}
";
    let dir = scratch("header_local_alphabet");
    let src_path = dir.join("touch.tmc");
    std::fs::write(&src_path, LOCAL_ALPHABET_FIXTURE).unwrap();

    let source_out = run_interface(&src_path);
    assert!(
        source_out
            .stdout
            .contains("alphabet localBits { '_', '0', '1' }")
            && !source_out.stdout.contains("export alphabet localBits"),
        "the source arm must print the referenced local alphabet WITHOUT \
         `export`: {}",
        source_out.stdout
    );
    assert!(
        source_out
            .stdout
            .contains("export routine touch(tape t: localBits writes { '1' });"),
        "{}",
        source_out.stdout
    );

    let object = compile(
        LOCAL_ALPHABET_FIXTURE,
        CompileOptions {
            opt_level: OptLevel::O0,
            ..CompileOptions::default()
        },
    )
    .unwrap_or_else(|e| panic!("compile LOCAL_ALPHABET_FIXTURE: {e}"))
    .object;
    let obj_path = dir.join("touch.tmo");
    std::fs::write(&obj_path, object.to_bytes()).unwrap();

    // The object arm must exit 0 — never error for want of an alphabet
    // name — and synthesize a deterministic `<routine>__<param>` name.
    let object_out = run_interface(&obj_path);
    assert!(
        object_out
            .stdout
            .contains("alphabet touch__t { '_', '0', '1' }"),
        "{}",
        object_out.stdout
    );
    assert!(
        object_out
            .stdout
            .contains("export routine touch(tape t: touch__t writes { '1' });"),
        "{}",
        object_out.stdout
    );
}

/// When a tape's glyph list DOES match one of the object's own exported
/// alphabets by content, the object arm must reuse that alphabet's real
/// name rather than synthesizing one — `plusOne`'s `num` tape draws from
/// the exported `bits` alphabet, so its rendered signature must say
/// `bits`, not a synthesized `plusOne__num`. Mutation: always
/// synthesizing regardless of a content match — the object arm would
/// then label the tape `plusOne__num` and this assertion goes red.
#[test]
fn the_object_arm_prefers_the_exported_name_when_the_content_matches() {
    const EXPORTED_MATCH_FIXTURE: &str = "\
export alphabet bits { '_', '0', '1' }

export routine plusOne(tape num: bits writes { '0', '1' }) {
  entry state s { [*] -> write ['0'] return; }
}
";
    let dir = scratch("header_exported_name_preferred");
    let object = compile(
        EXPORTED_MATCH_FIXTURE,
        CompileOptions {
            opt_level: OptLevel::O0,
            ..CompileOptions::default()
        },
    )
    .unwrap_or_else(|e| panic!("compile EXPORTED_MATCH_FIXTURE: {e}"))
    .object;
    let obj_path = dir.join("plus_one.tmo");
    std::fs::write(&obj_path, object.to_bytes()).unwrap();

    let out = run_interface(&obj_path);
    assert!(
        out.stdout
            .contains("export routine plusOne(tape num: bits writes { '0', '1' });"),
        "{}",
        out.stdout
    );
    assert!(
        !out.stdout.contains("plusOne__num"),
        "synthesized a name despite a content match: {}",
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
///
/// EVERY stdlib routine line agrees byte-for-byte between the two arms —
/// no tolerance, not even for the two volatile namespaces
/// (`binaryNumbersVolatile`, `binaryNumbersBareVolatile`), which import
/// their representation alphabet from a SIBLING namespace via an explicit
/// `use std::binaryNumbers::symbols;` (or its bare twin). Rule (2) of
/// `resolve_object_alphabet` (docs/tmt/cli.md (interface)) is exactly
/// what closes this: `binaryNumbers::symbols` is EXPORTED, so a tape
/// whose glyph list matches it, in a SIBLING namespace of `binaryNumbers`
/// itself, now resolves through the same "another namespace of this same
/// object" tier the source arm's own `use`-printing rule reaches through
/// its "target lives in another unit" clause — the two arms are deciding
/// the SAME question (is this alphabet reachable unqualified, or does it
/// need a `use` line?) from different data, and now answer it the same
/// way. Mutation: reverting `resolve_object_alphabet`'s rule (2) to a
/// same-or-enclosing-namespace-only match (the OLD behavior) — the
/// object arm would synthesize a private name for the two volatile
/// namespaces again, and this whole-document comparison would fail on
/// exactly those routines.
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
    for (name, source_line) in &source_routines {
        let object_line = &object_routines[name];
        assert_eq!(source_line, object_line, "`{name}`: the two arms disagree");
    }

    // The two arms' `use` lines agree too — both print exactly the same
    // set, in the same namespaces, now that the object arm's rule (2)
    // reaches the same sibling-namespace case the source arm's
    // "target lives in another unit" clause does. (The DOC-LINE
    // difference — `?` comments the object arm never carries, since the
    // wire has no doc-line field — is pre-existing and out of scope
    // here, which is why this compares `use` lines specifically rather
    // than the whole rendered document.)
    fn use_lines(text: &str) -> Vec<&str> {
        text.lines()
            .filter(|l| l.trim_start().starts_with("use "))
            .collect::<Vec<_>>()
    }
    assert_eq!(
        use_lines(&source_out.stdout),
        use_lines(&object_out.stdout),
        "source arm:\n{}\nobject arm:\n{}",
        source_out.stdout,
        object_out.stdout
    );
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

/// A tape with NEITHER `writes` nor `preserves` must publish the SAME
/// write set on both arms: the compiler's own INFERRED set for that tape,
/// never the whole alphabet. `touchA`'s body writes exactly one glyph
/// (`'a'`) of a three-glyph alphabet unconditionally. Mutation: the source
/// arm calling `compiler::declared_effective` directly instead of
/// `compiler::published_writes` — with no clause written,
/// `declared_effective` falls back to the WHOLE alphabet, so the source
/// arm would print `writes { '_', 'a', 'b' }` while the object arm (fixed
/// in the prior round) still prints the correctly inferred `writes { 'a' }`,
/// and this test goes red on the mismatch.
#[test]
fn the_two_arms_agree_on_an_uncontracted_routine() {
    const UNCONTRACTED_FIXTURE: &str = "\
export alphabet tri { '_', 'a', 'b' }

export routine touchA(tape t: tri) {
  entry state s { [*] -> write ['a'] return; }
}
";
    let dir = scratch("header_uncontracted_agree");
    let src_path = dir.join("touch_a.tmc");
    std::fs::write(&src_path, UNCONTRACTED_FIXTURE).unwrap();
    let source_out = run_interface(&src_path);
    assert!(
        source_out
            .stdout
            .contains("export routine touchA(tape t: tri writes { 'a' });"),
        "source arm did not publish the inferred write set: {}",
        source_out.stdout
    );

    let object = compile(
        UNCONTRACTED_FIXTURE,
        CompileOptions {
            opt_level: OptLevel::O0,
            ..CompileOptions::default()
        },
    )
    .unwrap_or_else(|e| panic!("compile UNCONTRACTED_FIXTURE: {e}"))
    .object;
    let obj_path = dir.join("touch_a.tmo");
    std::fs::write(&obj_path, object.to_bytes()).unwrap();
    let object_out = run_interface(&obj_path);

    assert_eq!(
        source_out.stdout, object_out.stdout,
        "the two arms disagree on an uncontracted routine's write set"
    );
}

/// The object arm must skip the entry world: a `machine` block is never a
/// callee, so it has no interface entry to read and no declaration to
/// render — printing it would falsely claim `main` "writes nothing".
/// Mutation: printing every `SymbolDef::Defined` symbol including the one
/// named `main` — the object-arm stdout would then contain a spurious
/// `export routine main(...)` line naming the machine world.
#[test]
fn a_unit_with_a_machine_block_renders_identically_on_both_arms() {
    const MACHINE_FIXTURE: &str = "\
export alphabet bits { '_', '0', '1' }

export routine plusOne(tape num: bits writes { '0', '1' }) {
  entry state inc {
    ['1'] -> write ['0'] move [<] goto inc;
    [*]   -> write ['1'] return;
  }
}

machine {
  tape num: bits;

  entry state s {
    [*] -> call plusOne(num = num) then stop;
  }
}
";
    let dir = scratch("header_machine_block");
    let src_path = dir.join("with_machine.tmc");
    std::fs::write(&src_path, MACHINE_FIXTURE).unwrap();
    let source_out = run_interface(&src_path);
    assert!(
        !source_out.stdout.contains("main"),
        "the source arm must never mention `main`: {}",
        source_out.stdout
    );

    let object = compile(
        MACHINE_FIXTURE,
        CompileOptions {
            opt_level: OptLevel::O0,
            ..CompileOptions::default()
        },
    )
    .unwrap_or_else(|e| panic!("compile MACHINE_FIXTURE: {e}"))
    .object;
    let obj_path = dir.join("with_machine.tmo");
    std::fs::write(&obj_path, object.to_bytes()).unwrap();
    let object_out = run_interface(&obj_path);

    assert!(
        !object_out.stdout.contains("main"),
        "the object arm printed the entry world: {}",
        object_out.stdout
    );
    assert_eq!(
        source_out.stdout, object_out.stdout,
        "a unit with a machine block must render identically on both arms \
         for its exported routine"
    );
    assert!(
        object_out
            .stdout
            .contains("export routine plusOne(tape num: bits writes { '0', '1' });"),
        "{}",
        object_out.stdout
    );
}

/// A content match in a SIBLING namespace — reachable only through an
/// explicit `use`, never unqualified — names the tape by a `use
/// <qualified name>;` line in the routine's OWN namespace plus the short
/// name, not a synthesized private declaration: rule (2) of
/// `resolve_object_alphabet`. `nsB::plusOne`'s tape draws from a LOCAL
/// (unexported) alphabet whose content is byte-identical to `nsA::bits`,
/// the object's only exported alphabet with that content, in a SIBLING
/// namespace (`nsA` is not an ancestor of `nsB`, so rule (1) — reachable
/// unqualified — does not match it). Mutation: synthesizing instead of
/// matching across namespaces (the OLD behavior, before this rule had
/// tiers) — the `use nsA::bits;` line and the `bits`-named reference
/// would both disappear, replaced by a synthesized
/// `nsB_plusOne__num` declaration and reference.
#[test]
fn the_object_arm_names_a_sibling_namespaces_alphabet_by_use() {
    const CROSS_NAMESPACE_FIXTURE: &str = "\
namespace nsA {
  export alphabet bits { '_', '0', '1' }
}

namespace nsB {
  alphabet localBits { '_', '0', '1' }

  export routine plusOne(tape num: localBits writes { '0', '1' }) {
    entry state s { [*] -> write ['0'] return; }
  }
}
";
    let dir = scratch("header_cross_namespace");
    let object = compile(
        CROSS_NAMESPACE_FIXTURE,
        CompileOptions {
            opt_level: OptLevel::O0,
            ..CompileOptions::default()
        },
    )
    .unwrap_or_else(|e| panic!("compile CROSS_NAMESPACE_FIXTURE: {e}"))
    .object;
    let obj_path = dir.join("cross_ns.tmo");
    std::fs::write(&obj_path, object.to_bytes()).unwrap();

    let out = run_interface(&obj_path);
    assert!(
        out.stdout.contains("use nsA::bits;"),
        "no `use` line for the sibling-namespace match: {}",
        out.stdout
    );
    assert!(
        out.stdout
            .contains("export routine plusOne(tape num: bits writes { '0', '1' });"),
        "{}",
        out.stdout
    );
    assert!(
        !out.stdout.contains("__num"),
        "synthesized a private alphabet instead of matching the sibling namespace: {}",
        out.stdout
    );
}

/// An alphabet this object IMPORTED from ANOTHER unit
/// (`Interface::imports`) — rule (3) of `resolve_object_alphabet`: a `use
/// <qualified name>;` line and the short name, the same shape rule (2)
/// gives a same-object cross-namespace match, sourced from the wire's
/// import record instead. `caller.tmc` never declares `bits` itself —
/// only `--extern lib.tmh` supplies it — so a match here can only have
/// come from `Interface::imports`, never from `Interface::alphabets`.
/// Mutation: ignoring `Interface::imports` entirely (rule (3) deleted) —
/// the tape would fall through to rule (4) and synthesize a private name
/// instead of `use lib::bits;` + `bits`.
#[test]
fn the_object_arm_names_an_imported_alphabet_by_use() {
    const LIB_HEADER: &str = "\
namespace lib {
  export alphabet bits { '_', '0', '1' }
}
";
    const CALLER: &str = "\
use lib::bits;

export routine touch(tape t: bits writes {}) {
  entry state s { [*] -> return; }
}
";
    let dir = scratch("header_object_import");
    let header_path = dir.join("lib.tmh");
    std::fs::write(&header_path, LIB_HEADER).unwrap();
    let caller_path = dir.join("caller.tmc");
    std::fs::write(&caller_path, CALLER).unwrap();
    let obj_path = dir.join("caller.tmo");

    let compile_out = execute(&args(&[
        "compile",
        caller_path.to_str().unwrap(),
        "--extern",
        header_path.to_str().unwrap(),
        "-o",
        obj_path.to_str().unwrap(),
    ]))
    .unwrap_or_else(|e| panic!("compile: {e}"));
    assert_eq!(compile_out.code, 0, "{}", compile_out.stderr);

    let out = run_interface(&obj_path);
    assert!(
        out.stdout.contains("use lib::bits;"),
        "no `use` line for the imported alphabet: {}",
        out.stdout
    );
    assert!(
        out.stdout
            .contains("export routine touch(tape t: bits writes {});"),
        "{}",
        out.stdout
    );
    assert!(
        !out.stdout.contains("alphabet bits {"),
        "printed a local declaration for a cross-unit import: {}",
        out.stdout
    );
}

/// A short-name COLLISION inside one namespace — `nsA::bits` and
/// `nsC::bits` are two DIFFERENT full paths that would both want the
/// short name `bits` inside `nsB` (neither is reachable unqualified from
/// `nsB`, so both are rule-(2) candidates) — refuses the SECOND claim
/// rather than printing an ambiguous `use`, falling back to rule (4) for
/// it. `touch`'s two tape parameters are processed in signature order, so
/// `a` (matching `nsA::bits`) claims `bits` first and `c` (matching
/// `nsC::bits`) loses the race. Mutation: claiming BOTH under `bits`
/// regardless of the collision — the header would then declare `use
/// nsC::bits;` too, alongside `use nsA::bits;`, and `c`'s signature would
/// ALSO read `bits`, ambiguously naming two different alphabets in one
/// scope.
#[test]
fn a_short_name_clash_falls_back_to_a_synthesized_name() {
    const CLASH_FIXTURE: &str = "\
namespace nsA {
  export alphabet bits { '_', '0', '1' }
}

namespace nsC {
  export alphabet bits { '_', 'x', 'y' }
}

namespace nsB {
  alphabet likeA { '_', '0', '1' }
  alphabet likeC { '_', 'x', 'y' }

  export routine touch(tape a: likeA writes {}, tape c: likeC writes {}) {
    entry state s { [*, *] -> return; }
  }
}
";
    let dir = scratch("header_object_clash");
    let object = compile(
        CLASH_FIXTURE,
        CompileOptions {
            opt_level: OptLevel::O0,
            ..CompileOptions::default()
        },
    )
    .unwrap_or_else(|e| panic!("compile CLASH_FIXTURE: {e}"))
    .object;
    let obj_path = dir.join("clash.tmo");
    std::fs::write(&obj_path, object.to_bytes()).unwrap();

    let out = run_interface(&obj_path);
    assert!(
        out.stdout.contains("use nsA::bits;"),
        "the first (winning) claim's `use` line is missing: {}",
        out.stdout
    );
    assert!(
        !out.stdout.contains("use nsC::bits;"),
        "a colliding `use` line was printed: {}",
        out.stdout
    );
    assert!(
        out.stdout.contains("nsB_touch__c"),
        "the losing claim did not fall back to a synthesized name: {}",
        out.stdout
    );
    assert!(
        out.stdout.contains(
            "export routine touch(tape a: bits writes {}, tape c: nsB_touch__c writes {});"
        ),
        "{}",
        out.stdout
    );
}

/// A namespaced routine over a TOP-LEVEL (enclosing-namespace) exported
/// alphabet must still match it on the object arm — enclosing scopes,
/// unlike siblings, ARE reachable unqualified in source, so the
/// namespace-scoping fix above must not narrow matching down to
/// exact-namespace-only. Mutation: requiring exact namespace equality
/// (`ns == alphabet_ns` instead of `ns.starts_with(alphabet_ns)`) — the
/// top-level `bits` alphabet would then no longer match `mylib::touch`'s
/// tape, and the object arm would synthesize `mylib_touch__num` instead of
/// reusing `bits`.
#[test]
fn an_object_arm_routine_matches_an_alphabet_in_an_enclosing_namespace() {
    const ENCLOSING_FIXTURE: &str = "\
export alphabet bits { '_', '0', '1' }

namespace mylib {
  export routine touch(tape num: bits writes { '0', '1' }) {
    entry state s { [*] -> write ['0'] return; }
  }
}
";
    let dir = scratch("header_enclosing_namespace");
    let object = compile(
        ENCLOSING_FIXTURE,
        CompileOptions {
            opt_level: OptLevel::O0,
            ..CompileOptions::default()
        },
    )
    .unwrap_or_else(|e| panic!("compile ENCLOSING_FIXTURE: {e}"))
    .object;
    let obj_path = dir.join("enclosing.tmo");
    std::fs::write(&obj_path, object.to_bytes()).unwrap();

    let out = run_interface(&obj_path);
    assert!(
        out.stdout
            .contains("export routine touch(tape num: bits writes { '0', '1' });"),
        "did not match the enclosing namespace's exported alphabet: {}",
        out.stdout
    );
    assert!(
        !out.stdout.contains("mylib_touch__num"),
        "synthesized a name despite an enclosing-namespace content match: {}",
        out.stdout
    );
}

/// `?` doc lines print VERBATIM, line-for-line — one output line per
/// written source line, never joined into one paragraph-wide line — on
/// the source arm; the object arm carries no doc line at all (no field on
/// the wire). Mutation: printing `Doc::paragraphs` (the space-joined
/// form) instead of `Doc::paragraph_lines` — the two written lines would
/// collapse into one `? First line of doc. Second line of doc.` line.
#[test]
fn doc_lines_print_verbatim_on_the_source_arm_and_not_on_the_object_arm() {
    const DOC_FIXTURE: &str = "\
export alphabet bits { '_', '0', '1' }

? First line of doc.
? Second line of doc.
export routine plusOne(tape num: bits writes { '0', '1' }) {
  entry state s { [*] -> write ['0'] return; }
}
";
    let dir = scratch("header_doc_verbatim");
    let src_path = dir.join("doc.tmc");
    std::fs::write(&src_path, DOC_FIXTURE).unwrap();
    let source_out = run_interface(&src_path);
    assert!(
        source_out
            .stdout
            .contains("? First line of doc.\n? Second line of doc.\n"),
        "doc lines were not printed verbatim, line-for-line: {}",
        source_out.stdout
    );
    assert!(
        !source_out
            .stdout
            .contains("First line of doc. Second line of doc."),
        "doc lines were paragraph-joined: {}",
        source_out.stdout
    );

    let object = compile(
        DOC_FIXTURE,
        CompileOptions {
            opt_level: OptLevel::O0,
            ..CompileOptions::default()
        },
    )
    .unwrap_or_else(|e| panic!("compile DOC_FIXTURE: {e}"))
    .object;
    let obj_path = dir.join("doc.tmo");
    std::fs::write(&obj_path, object.to_bytes()).unwrap();
    let object_out = run_interface(&obj_path);
    // A non-vacuous positive check first: the object arm must still render
    // the routine's own signature line (so the absence of `?` below proves
    // "no doc line", not merely "empty/broken output").
    assert!(
        object_out
            .stdout
            .contains("export routine plusOne(tape num: bits writes { '0', '1' });"),
        "{}",
        object_out.stdout
    );
    assert!(
        !object_out.stdout.contains('?'),
        "the object arm printed a doc line, which has no wire field: {}",
        object_out.stdout
    );
}

/// The declaration-only signature shape (`;` in place of a `{ … }` body,
/// docs/tmt/language.md (headers)) closes the `tmt interface`
/// generator/reader loop: the header `tmt interface` renders for a
/// `.tmc` source reparses back to the identical text through the
/// declarations-only reader. Mutation: reverting the `.tmh`-extension
/// dispatch in `cli/interface.rs` (so `interface` always reads
/// full-program mode) — `run_interface` on `mylib.tmh` would then fail
/// to parse the bodiless `plusOne` signature at all, instead of
/// reproducing it.
#[test]
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

    // Re-running `interface` over the header itself, through the
    // declarations-only reader (`.tmh` extension dispatch), must
    // reproduce it unchanged.
    let reparsed = run_interface(&header_path);
    assert_eq!(reparsed.stdout, header);
}

/// The real embedded standard-library source, not a small fixture — the
/// case that first surfaced this printer defect (docs/tmt/cli.md
/// (interface)): `binaryNumbersVolatile` and `binaryNumbersBareVolatile`
/// each import their representation alphabet from a SIBLING namespace via
/// an explicit `use`, unqualified in every one of their routines' tape
/// signatures. Mutation: dropping the `use`-line pass entirely — the
/// reparse below fails `unresolved-alphabet` at
/// `binaryNumbersVolatile::goToNumber`'s tape signature.
#[test]
fn the_real_stdlib_source_reparses_as_a_header() {
    let dir = scratch("header_stdlib_reparses");
    let src_path = dir.join("std.tmc");
    std::fs::write(&src_path, stdlib::SOURCE).unwrap();
    let header = run_interface(&src_path).stdout;

    let header_path = dir.join("std.tmh");
    std::fs::write(&header_path, &header).unwrap();

    let reparsed = run_interface(&header_path);
    assert_eq!(
        reparsed.stdout, header,
        "the stdlib's own header did not reparse to itself"
    );
}

/// A namespace importing another's exported alphabet by `use`, named
/// unqualified in a tape signature: the header must reprint that `use`
/// line, or the alphabet name in the reprinted signature is unresolvable
/// when the header is read back through the strict declarations-only
/// reader (docs/tmt/cli.md (interface)). Mutation: dropping the
/// `use`-line pass — the reparse below fails `unresolved-alphabet`
/// instead of reproducing the header, the same failure the real stdlib
/// showed before this fix.
#[test]
fn a_header_prints_the_use_lines_its_declarations_need() {
    const USE_ALPHABET_FIXTURE: &str = "\
namespace producer {
  export alphabet bits { '_', '0', '1' }
}

namespace consumer {
  use producer::bits;

  export routine touch(tape t: bits writes { '1' }) {
    entry state s { [*] -> write ['1'] return; }
  }
}
";
    let dir = scratch("header_use_needed");
    let src_path = dir.join("use_needed.tmc");
    std::fs::write(&src_path, USE_ALPHABET_FIXTURE).unwrap();
    let source_out = run_interface(&src_path);

    assert!(
        source_out.stdout.contains("use producer::bits;"),
        "the printer dropped the needed `use` line: {}",
        source_out.stdout
    );
    assert!(
        source_out
            .stdout
            .contains("export routine touch(tape t: bits writes { '1' });"),
        "{}",
        source_out.stdout
    );

    let header_path = dir.join("use_needed.tmh");
    std::fs::write(&header_path, &source_out.stdout).unwrap();
    let reparsed = run_interface(&header_path);
    assert_eq!(
        reparsed.stdout, source_out.stdout,
        "the printed header did not reparse to itself"
    );
}

/// A `use` naming an alphabet declared in ANOTHER unit — resolved through
/// the compile's declarations table (the embedded standard library, no
/// `--extern` needed here), never through this unit's own declarations —
/// must still print, or the reprinted signature's bare `symbols` cannot
/// resolve when the header is read back (docs/tmt/cli.md (interface)).
/// This is the concrete shape cross-unit alphabet resolution made
/// reachable: before it existed, `use std::binaryNumbers::symbols; tape
/// t: symbols` failed to COMPILE at all (`unresolved-alphabet`), so `tmt
/// interface` could never even reach the printer with it. Mutation: the
/// OLD source-arm rule (`printed_full_names` alone, with no "target
/// lives in another unit" clause) — `symbols` is never printed by THIS
/// unit, so the `use` line would be dropped and the reparse below would
/// fail `unresolved-alphabet` on `symbols`, the same failure class the
/// real stdlib's volatile twins showed before their own fix.
#[test]
fn a_header_keeps_the_use_of_an_alphabet_from_another_unit() {
    const CROSS_UNIT_FIXTURE: &str = "\
use std::binaryNumbers::symbols;

export routine touch(tape t: symbols writes {}) {
  entry state s { [*] -> return; }
}
";
    let dir = scratch("header_cross_unit_use");
    let src_path = dir.join("caller.tmc");
    std::fs::write(&src_path, CROSS_UNIT_FIXTURE).unwrap();
    let source_out = run_interface(&src_path);

    assert!(
        source_out
            .stdout
            .contains("use std::binaryNumbers::symbols;"),
        "the printer dropped the cross-unit `use` line: {}",
        source_out.stdout
    );
    assert!(
        source_out
            .stdout
            .contains("export routine touch(tape t: symbols writes {});"),
        "{}",
        source_out.stdout
    );

    let header_path = dir.join("caller.tmh");
    std::fs::write(&header_path, &source_out.stdout).unwrap();
    let reparsed = run_interface(&header_path);
    assert_eq!(
        reparsed.stdout, source_out.stdout,
        "the printed header did not reparse to itself"
    );
}

/// A NON-exported map named inside an EXPORTED graph body's binding
/// still needs its own declaration in the header, or the printed
/// `with map NAME` reference cannot resolve when the header is read
/// back — the same "referenced, printed even if not itself exported"
/// rule an alphabet already gets. Mutation: dropping the
/// referenced-map path (printing only EXPORTED maps) — the header would
/// then carry `with map collapse` inside `outer`'s printed body with no
/// `collapse` declaration anywhere in it, and the reparse below would
/// fail `unknown map \`collapse\` [undefined-map]`.
#[test]
fn a_locally_referenced_map_prints_unexported_and_the_header_reparses() {
    const PRIVATE_MAP_FIXTURE: &str = "\
export alphabet host5 { '_', '^', '$', '0', '1' }
export alphabet bare3 { '_', '0', '1' }
map collapse: host5 -> bare3 { '^' => '_', '$' => '_', '0' -> '0', '1' -> '1' }

export graph inner(tape v: bare3, state done) {
  entry state s { [*] -> done; }
}

export graph outer(tape t: host5, state done) {
  entry graft inner(v = t with map collapse, done = done) as z;
}
";
    let dir = scratch("header_private_map_referenced");
    let src_path = dir.join("privmap.tmc");
    std::fs::write(&src_path, PRIVATE_MAP_FIXTURE).unwrap();
    let source_out = run_interface(&src_path);

    assert!(
        source_out.stdout.contains(
            "map collapse: host5 -> bare3 { '^' => '_', '$' => '_', '0' -> '0', '1' -> '1' }"
        ),
        "the private but referenced map's own declaration is missing: {}",
        source_out.stdout
    );
    assert!(
        !source_out.stdout.contains("export map collapse"),
        "a non-exported map must not print `export`: {}",
        source_out.stdout
    );
    assert!(
        source_out.stdout.contains("with map collapse"),
        "{}",
        source_out.stdout
    );

    let header_path = dir.join("privmap.tmh");
    std::fs::write(&header_path, &source_out.stdout).unwrap();
    let reparsed = run_interface(&header_path);
    assert_eq!(
        reparsed.stdout, source_out.stdout,
        "the printed header did not reparse to itself"
    );
}

/// A map imported from a sibling namespace (`use producer::collapse;`)
/// and named inside an EXPORTED graph body needs its `use` line kept —
/// the same rule an imported alphabet already gets
/// (`a_header_keeps_the_use_of_an_alphabet_from_another_unit`). The
/// import's TARGET (`producer::collapse`) is itself printed (it is
/// exported), so this also exercises the "printed-or-external" half of
/// the `use`-line rule on a map for the first time.
#[test]
fn an_imported_map_referenced_in_a_graph_body_keeps_its_use_line() {
    const IMPORTED_MAP_FIXTURE: &str = "\
namespace producer {
  export alphabet host5 { '_', '^', '$', '0', '1' }
  export alphabet bare3 { '_', '0', '1' }
  export map collapse: host5 -> bare3 { '^' => '_', '$' => '_', '0' -> '0', '1' -> '1' }
}

namespace consumer {
  use producer::host5, producer::bare3, producer::collapse;

  export graph inner(tape v: bare3, state done) {
    entry state s { [*] -> done; }
  }

  export graph outer(tape t: host5, state done) {
    entry graft inner(v = t with map collapse, done = done) as z;
  }
}
";
    let dir = scratch("header_imported_map_referenced");
    let src_path = dir.join("impmap.tmc");
    std::fs::write(&src_path, IMPORTED_MAP_FIXTURE).unwrap();
    let source_out = run_interface(&src_path);

    assert!(
        source_out.stdout.contains("use producer::collapse;"),
        "the printer dropped the needed `use` line for the imported map: {}",
        source_out.stdout
    );
    // `producer::collapse` is itself exported IN THIS SAME UNIT, so its own
    // declaration prints too (under `producer`) — the "printed" half of the
    // printed-or-external rule, not the "external" half; a genuinely
    // cross-unit import is `a_header_keeps_the_use_of_an_alphabet_from_
    // another_unit`'s own shape, already covered for alphabets.
    assert!(
        source_out
            .stdout
            .contains("export map collapse: host5 -> bare3"),
        "{}",
        source_out.stdout
    );

    let header_path = dir.join("impmap.tmh");
    std::fs::write(&header_path, &source_out.stdout).unwrap();
    let reparsed = run_interface(&header_path);
    assert_eq!(
        reparsed.stdout, source_out.stdout,
        "the printed header did not reparse to itself"
    );
}

/// The near miss: an import whose target is NEVER printed (a
/// non-exported alphabet nothing exported references) must be dropped,
/// not reprinted — `producer::secret` is private and nothing exported
/// reaches it, so `producer` never appears in the header at all.
/// Mutation: printing every import regardless of whether its target is
/// printed — the header would then carry `use producer::secret;` naming
/// a namespace that never appears anywhere else in the output. Caught
/// here by CONTENT, not by a reparse failure: VERIFIED by hand that
/// feeding that hand-mutated text back through `tmt interface` still
/// exits 0 — resolving an unreferenced, `::`-absolute import path is
/// `unused-import`, a lint finding, never fatal
/// (docs/tmt/language.md (namespaces, visibility, and imports): "An
/// import nothing references is a lint finding"). The positive
/// assertion — the `use` line, and `producer` itself, are simply absent
/// — is therefore the test.
#[test]
fn a_use_of_something_the_header_does_not_print_is_dropped() {
    const UNUSED_IMPORT_FIXTURE: &str = "\
namespace producer {
  alphabet secret { '_', 'x' }
}

namespace consumer {
  use producer::secret;

  export alphabet bits { '_', '0', '1' }

  export routine touch(tape t: bits writes { '1' }) {
    entry state s { [*] -> write ['1'] return; }
  }
}
";
    let dir = scratch("header_use_dropped");
    let src_path = dir.join("use_dropped.tmc");
    std::fs::write(&src_path, UNUSED_IMPORT_FIXTURE).unwrap();
    let out = run_interface(&src_path);

    assert!(
        !out.stdout.contains("use producer::secret"),
        "an import whose target is never printed leaked into the header: {}",
        out.stdout
    );
    assert!(
        !out.stdout.contains("producer"),
        "`producer` has nothing exported or referenced, so it must not \
         appear in the header at all: {}",
        out.stdout
    );
    assert!(
        out.stdout
            .contains("export routine touch(tape t: bits writes { '1' });"),
        "{}",
        out.stdout
    );
}

/// A `0.2` bodiless routine signature (`;` in place of a `{ … }` body)
/// still carries its `writes` contract when read in declarations-only
/// mode. Mutation: skipping the signature when no body follows (e.g. a
/// reader that treats a WORLD-less REUSE as an empty declaration) — the
/// contract text below would then be absent from the output.
#[test]
fn a_bodiless_signature_parses_and_carries_its_contracts() {
    let dir = scratch("header_bodiless_signature");
    let path = dir.join("mylib.tmh");
    std::fs::write(
        &path,
        "export alphabet bits { '_', '0', '1' }\n\
         export routine plusOne(tape num: bits writes { '0', '1' });\n",
    )
    .unwrap();

    let out = run_interface(&path);
    assert!(
        out.stdout
            .contains("export routine plusOne(tape num: bits writes { '0', '1' });"),
        "{}",
        out.stdout
    );
}

/// One exported alphabet, a namespace, a `?` doc line, and a bodiless
/// routine signature — accepted by the declarations-only reader with no
/// `machine` block present. Paired with `a_header_may_not_declare_a_machine`
/// below.
#[test]
fn a_header_without_a_machine_is_fine() {
    let dir = scratch("header_no_machine");
    let path = dir.join("mylib.tmh");
    std::fs::write(
        &path,
        "export alphabet bits { '_', '0', '1' }\n\
         namespace mylib {\n\
         ? adds one to a bits tape\n\
         export routine plusOne(tape num: bits writes { '0', '1' });\n\
         }\n",
    )
    .unwrap();

    let out = run_interface(&path);
    assert!(
        out.stdout
            .contains("export routine plusOne(tape num: bits writes { '0', '1' });"),
        "{}",
        out.stdout
    );
}

/// A `machine { … }` block is rejected in declarations-only reading — a
/// header carries no program entry point (a `machine` is never a callee,
/// so it has nothing a header would state). Mutation: dropping the
/// rejection in `check_declarations_shape` — `execute` would then return
/// `Ok` instead of `Err`, and `expect_err` below would panic.
#[test]
fn a_header_may_not_declare_a_machine() {
    let dir = scratch("header_machine_rejected");
    let path = dir.join("with_machine.tmh");
    std::fs::write(
        &path,
        "export alphabet bits { '_', '0', '1' }\n\
         machine {\n\
         tape num: bits;\n\
         entry state s { [*] -> stop; }\n\
         }\n",
    )
    .unwrap();

    let err = execute(&args(&["interface", path.to_str().unwrap()]))
        .expect_err("a `machine` block must be rejected in declarations-only reading");
    assert!(err.contains("machine-in-declarations"), "{err}");
}

/// A routine WITH a body is rejected in declarations-only reading — a
/// header states a callable signature only. Paired with
/// `a_header_may_carry_a_graph_body` below: together the pair catches the
/// easy over-correction of banning every body regardless of carrier,
/// which would also break graphs (a graph's only form is its source, so
/// it cannot state one bodiless). Mutation: banning every body — this
/// test stays green (a routine body is still rejected), but its sibling
/// goes red.
#[test]
fn a_header_may_not_carry_a_routine_body() {
    let dir = scratch("header_routine_body_rejected");
    let path = dir.join("routine_body.tmh");
    std::fs::write(
        &path,
        "export alphabet bits { '_', '0', '1' }\n\
         export routine plusOne(tape num: bits writes { '0', '1' }) {\n\
         entry state s { [*] -> write ['1'] return; }\n\
         }\n",
    )
    .unwrap();

    let err = execute(&args(&["interface", path.to_str().unwrap()]))
        .expect_err("a routine WITH a body must be rejected in declarations-only reading");
    assert!(err.contains("routine-body-in-declarations"), "{err}");
}

/// A graph WITH a body is LEGAL in declarations-only reading — a graph's
/// only form is its source, so a header cannot state one any other way.
/// This is the fixture that catches the over-correction its sibling
/// above cannot: banning every body in declarations-only mode would
/// reject this too. Mutation: banning every body regardless of carrier —
/// this test goes red (`execute` returns `Err` instead of the graph's
/// rendered body).
#[test]
fn a_header_may_carry_a_graph_body() {
    let dir = scratch("header_graph_body_allowed");
    let path = dir.join("graph_body.tmh");
    std::fs::write(
        &path,
        "export alphabet bits { '_', '0', '1' }\n\
         export graph g(tape t: bits, state d) {\n\
         entry state s { [*] -> goto d; }\n\
         }\n",
    )
    .unwrap();

    let out = run_interface(&path);
    assert!(
        out.stdout
            .contains("export graph g(tape t: bits writes {}, state d) {"),
        "{}",
        out.stdout
    );
    assert!(out.stdout.contains("entry state s {"), "{}", out.stdout);
}

/// The mode flag is real, not cosmetic: a namespaced bodiless routine,
/// compiled as a PROGRAM (`compiler::compile`'s default
/// `ReadMode::Program`), still requires a body — the declarations-only
/// alternative does not leak into ordinary compilation. Mutation: making
/// the `;` alternative unconditional (letting a `ReadMode::Program` world
/// through the same declarations-only exemption `check_entry` grants a
/// header's routine) — `compile` would then return `Ok` for a routine
/// with no rules at all.
#[test]
fn a_bodiless_signature_in_a_tmc_program_is_an_error() {
    const NAMESPACED_BODILESS_ROUTINE: &str = "\
export alphabet bits { '_', '0', '1' }
namespace mylib {
  ? adds one to a bits tape
  export routine plusOne(tape num: bits writes { '0', '1' });
}
";
    let err = compile(
        NAMESPACED_BODILESS_ROUTINE,
        CompileOptions {
            opt_level: OptLevel::O0,
            ..CompileOptions::default()
        },
    )
    .expect_err("a bodiless routine must still fail to compile as a program");
    assert_eq!(err.kind.code(), "entry-count", "{err}");
    assert_eq!(err.span.start.line, 4);
}

/// A routine with `state` parameters, for the two arms below: the source
/// declares two exits and the compiled object carries their COUNT.
const STATE_PARAM_FIXTURE: &str = "\
export alphabet bits { '_', '0', '1' }

export routine pick(tape n: bits, state hit, state miss) {
  entry state s {
    ['_'] -> goto hit;
    [*]   -> goto miss;
  }
}
";

/// The source arm has the declaration in front of it, so it prints each
/// `state` parameter by its real name, in signature order. The printed
/// header re-parses through the strict declarations-only reader.
///
/// Mutation: dropping `state` parameters from `signature_text`; the
/// printed signature loses them and the assertion goes red.
#[test]
fn the_source_arm_prints_state_parameters_by_name() {
    let dir = scratch("header_source_state_params");
    let path = dir.join("pick.tmc");
    std::fs::write(&path, STATE_PARAM_FIXTURE).unwrap();

    let out = run_interface(&path);
    assert!(
        out.stdout.contains(
            "export routine pick(tape n: bits writes {}, state hit, state miss) noreturn;"
        ),
        "{}",
        out.stdout
    );

    let header_path = dir.join("pick.tmh");
    std::fs::write(&header_path, &out.stdout).unwrap();
    assert_eq!(run_interface(&header_path).stdout, out.stdout);
}

/// The object arm has only the exit COUNT — a `state` parameter's name
/// is compile-time material the wire never carries (docs/formats.md
/// (routine interfaces)) — so it prints the exits positionally, one
/// `state exit<k>` per declared exit, and that header re-parses too.
///
/// Mutation: printing the tape parameters alone; a caller reading the
/// object's header would see a routine that takes no exits at all.
#[test]
fn the_object_arm_prints_state_parameters_positionally() {
    let dir = scratch("header_object_state_params");
    let object = compile(
        STATE_PARAM_FIXTURE,
        CompileOptions {
            opt_level: OptLevel::O0,
            ..CompileOptions::default()
        },
    )
    .unwrap_or_else(|e| panic!("compile STATE_PARAM_FIXTURE: {e}"))
    .object;
    let obj_path = dir.join("pick.tmo");
    std::fs::write(&obj_path, object.to_bytes()).unwrap();

    let out = run_interface(&obj_path);
    assert!(
        out.stdout.contains(
            "export routine pick(tape n: bits writes {}, state exit0, state exit1) noreturn;"
        ),
        "{}",
        out.stdout
    );

    let header_path = dir.join("pick.tmh");
    std::fs::write(&header_path, &out.stdout).unwrap();
    assert_eq!(run_interface(&header_path).stdout, out.stdout);
}

/// The object arm's positional exit names are freshened against the tape
/// parameters printed beside them: a routine whose tape is literally
/// named `exit0` would otherwise produce a signature naming one parameter
/// twice, which the strict reader rejects as `duplicate-param` — a header
/// printed at exit 0 that cannot be read back.
///
/// Mutation: print `state exit{k}` unconditionally; the re-parse below
/// fails.
#[test]
fn the_object_arms_exit_names_never_collide_with_a_tape_parameter() {
    const CLASH: &str = "\
export alphabet bits { '_', '0', '1' }

export routine pick(tape exit0: bits, state hit) {
  entry state s { [*] -> goto hit; }
}
";
    let dir = scratch("header_object_exit_clash");
    let object = compile(
        CLASH,
        CompileOptions {
            opt_level: OptLevel::O0,
            ..CompileOptions::default()
        },
    )
    .unwrap_or_else(|e| panic!("compile CLASH: {e}"))
    .object;
    let obj_path = dir.join("pick.tmo");
    std::fs::write(&obj_path, object.to_bytes()).unwrap();

    let out = run_interface(&obj_path);
    assert!(
        out.stdout
            .contains("export routine pick(tape exit0: bits writes {}, state exit0_1) noreturn;"),
        "{}",
        out.stdout
    );

    let header_path = dir.join("pick.tmh");
    std::fs::write(&header_path, &out.stdout).unwrap();
    assert_eq!(run_interface(&header_path).stdout, out.stdout);
}

/// `TMC_LANG_VERSION` moved `0.1` → `0.2` in this task — the first task
/// in the binding arc's phase 3a to change the `.tmc` grammar (the
/// bodiless-signature alternative). Pre-1.0, `N` bumps on ANY grammar
/// change; there is no patch digit. Mutation: leaving it at `0.1`.
#[test]
fn the_language_version_is_two() {
    assert_eq!(mtc_turing_machine::TMC_LANG_VERSION, "0.2");
}
