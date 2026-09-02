//! The standard library the browser links: its text is the embedded
//! source, its object differs from the release object only in the side
//! table, and its addresses resolve to lines of that text under the `std`
//! file — with the user's own lines kept apart by provenance.

use mtc_core::linker::LinkOptions;
use mtc_wasm::inner::program::{Program, STD_SOURCE, SourceFile, USER_SOURCE, build};
use mtc_wasm::inner::session::{Event, Limits, OutcomeInfo, Seed, Session};
use mtc_wasm::inner::{Arch, Lang, stdlib};

/// Calls into the PM-1 library and marks past the section it walked.
const PMC_STD: &str = "main() {\n    1: @std::goToEnd();\n    2: right(3);\n    3: mark(!);\n}\n";
/// Calls into the TM-1 library: plus one on a bare binary number.
const TMC_STD: &str = "alphabet bits { '_', '0', '1' }\nmachine {\n  tape num: bits;\n  entry state start { [*] -> call std::binaryNumbersBare::plusOne() then done; }\n  state done { [*] -> stop; }\n}\n";

fn std_function<'a>(program: &'a Program, name: &str) -> &'a mtc_core::linker::MapFunction {
    program
        .map
        .functions
        .iter()
        .find(|f| f.name == name)
        .unwrap_or_else(|| panic!("`{name}` is linked in"))
}

/// 1-based line numbers of the routine's declaration and of the next
/// declaration after it in the embedded text, so the pin follows the
/// library when it is edited.
fn declaration_range(source: &str, needle: &str) -> (u32, u32) {
    let lines: Vec<&str> = source.lines().collect();
    let start = lines
        .iter()
        .position(|l| l.contains(needle))
        .unwrap_or_else(|| panic!("`{needle}` in the stdlib source"));
    let end = lines[start + 1..]
        .iter()
        .position(|l| l.contains("export "))
        .map(|p| start + 1 + p)
        .unwrap_or(lines.len());
    (start as u32 + 1, end as u32 + 1)
}

/// 1-based lines of a namespace's opening and of the next namespace's.
fn namespace_range(source: &str, opener: &str) -> (u32, u32) {
    let lines: Vec<&str> = source.lines().collect();
    let start = lines
        .iter()
        .position(|l| l.contains(opener))
        .unwrap_or_else(|| panic!("`{opener}` in the stdlib source"));
    let end = lines[start + 1..]
        .iter()
        .position(|l| l.trim_start().starts_with("namespace "))
        .map(|p| start + 1 + p)
        .unwrap_or(lines.len());
    (start as u32 + 1, end as u32 + 1)
}

#[test]
fn source_is_the_embedded_text() {
    assert_eq!(stdlib::source(Arch::Pm1), mtc_post_machine::stdlib::SOURCE);
    assert_eq!(
        stdlib::source(Arch::Tm1),
        mtc_turing_machine::stdlib::SOURCE
    );
    assert!(stdlib::source(Arch::Pm1).contains("export goToEnd()"));
    assert!(stdlib::source(Arch::Tm1).contains("namespace binaryNumbersBare"));
}

/// The browser-linked image is the image the CLI would write: linking
/// the same unit against the release-preset object gives the same code.
#[test]
fn debug_stdlib_links_to_the_same_code_as_the_release_stdlib() {
    use mtc_post_machine::compiler::{CompileOptions, compile};
    let (browser, _) = build(Lang::Pmc, PMC_STD, 1).unwrap();
    let object = compile(PMC_STD, CompileOptions::default()).unwrap().object;
    let cli = mtc_post_machine::asm::link(
        &[object],
        &[mtc_post_machine::stdlib::object().clone()],
        LinkOptions::default(),
    )
    .unwrap();
    assert_eq!(browser.exe.code, cli.executable.code);
    assert!(
        stdlib::object(Arch::Pm1).debug.is_some()
            && mtc_post_machine::stdlib::object().debug.is_none(),
        "the browser's copy carries the side table the release copy lacks"
    );
    assert_eq!(
        stdlib::object(Arch::Pm1).blobs,
        mtc_post_machine::stdlib::object().blobs,
        "and nothing else differs in the code"
    );

    use mtc_turing_machine::compiler::{CompileOptions as TmOptions, compile as tm_compile};
    let (browser, _) = build(Lang::Tmc, TMC_STD, 1).unwrap();
    let object = tm_compile(TMC_STD, TmOptions::default()).unwrap().object;
    let cli = mtc_turing_machine::asm::link(
        &[object],
        &[mtc_turing_machine::stdlib::object().clone()],
        LinkOptions::default(),
    )
    .unwrap();
    assert_eq!(browser.exe.code, cli.executable.code);
    assert_eq!(
        browser.exe.to_bytes(),
        cli.executable.to_bytes(),
        "the whole image"
    );
}

#[test]
fn a_stdlib_address_resolves_to_a_line_of_the_stdlib_source() {
    let (program, _) = build(Lang::Pmc, PMC_STD, 1).unwrap();
    let go_to_end = std_function(&program, "std::goToEnd");
    assert_eq!(go_to_end.source.as_deref(), Some(STD_SOURCE));
    assert!(
        !go_to_end.lines.is_empty(),
        "the line table reached the map"
    );
    let (from, to) = declaration_range(stdlib::source(Arch::Pm1), "export goToEnd()");
    for &(offset, _) in &go_to_end.lines {
        let loc = program.line_of(offset).unwrap();
        assert_eq!(loc.file, SourceFile::Std);
        assert_eq!(loc.function, "std::goToEnd");
        let line = loc.line.expect("mapped");
        assert!(
            (from..to).contains(&line),
            "line {line} lies in goToEnd's declaration {from}..{to}"
        );
    }
    let main = std_function(&program, "main");
    assert_eq!(main.source.as_deref(), Some(USER_SOURCE));
    assert_eq!(program.line_of(main.start).unwrap().file, SourceFile::User);

    let (program, _) = build(Lang::Tmc, TMC_STD, 1).unwrap();
    let plus_one = program
        .map
        .functions
        .iter()
        .find(|f| f.name == "std::binaryNumbersBare::plusOne")
        .expect("the routine is linked in");
    assert_eq!(plus_one.source.as_deref(), Some(STD_SOURCE));
    assert!(
        !plus_one.lines.is_empty(),
        "the TM-1 line table reached the map"
    );
    // A `.tmc` routine grafts a graph, so its rows map to the graph's
    // lines — anywhere inside the namespace, not the routine's own header.
    let (from, to) = namespace_range(stdlib::source(Arch::Tm1), "namespace binaryNumbersBare {");
    for &(offset, _) in &plus_one.lines {
        let loc = program.line_of(offset).unwrap();
        assert_eq!(loc.file, SourceFile::Std);
        assert_eq!(loc.function, "std::binaryNumbersBare::plusOne");
        let line = loc.line.expect("mapped");
        assert!(
            (from..to).contains(&line),
            "line {line} lies in binaryNumbersBare {from}..{to}"
        );
    }
    // The entry byte before the first mapped row is the function's, with
    // no line of its own.
    let entry = program.line_of(plus_one.start).unwrap();
    assert_eq!((entry.file, entry.line), (SourceFile::Std, None));
}

#[test]
fn address_for_line_keeps_the_two_files_apart() {
    let (program, _) = build(Lang::Pmc, PMC_STD, 1).unwrap();
    let go_to_end = std_function(&program, "std::goToEnd");
    let main = std_function(&program, "main");
    let std_line = program.line_of(go_to_end.lines[0].0).unwrap().line.unwrap();

    let user_addr = program.address_for_line(2, SourceFile::User).unwrap();
    assert!((main.start..main.end).contains(&user_addr));
    let std_addr = program.address_for_line(std_line, SourceFile::Std).unwrap();
    assert!((go_to_end.start..go_to_end.end).contains(&std_addr));
    // The same number asked of the other file never crosses over.
    if let Some(a) = program.address_for_line(std_line, SourceFile::User) {
        assert!(!(go_to_end.start..go_to_end.end).contains(&a));
    }
    if let Some(a) = program.address_for_line(2, SourceFile::Std) {
        assert!(!(main.start..main.end).contains(&a));
    }
    assert_eq!(SourceFile::parse("std"), Some(SourceFile::Std));
    assert_eq!(SourceFile::parse("user"), Some(SourceFile::User));
    assert_eq!(SourceFile::parse("main.pmc"), None);
}

#[test]
fn the_program_still_runs_with_the_debug_stdlib() {
    let (program, _) = build(Lang::Pmc, PMC_STD, 1).unwrap();
    let mut s = Session::new(
        &program,
        &[Seed {
            cells: vec![1, 1],
            head: 0,
            origin: 0,
        }],
        Limits::default(),
    )
    .unwrap();
    match s.pump(None).unwrap() {
        Event::Finished(f) => assert!(matches!(f.outcome, OutcomeInfo::Stopped)),
        other => panic!("{other:?}"),
    }
    let snap = s.snapshot(0).unwrap();
    assert_eq!(&snap.cells[..3], &[1, 1, 1]);
}

#[test]
fn map_json_carries_the_provenance_strings() {
    let (program, _) = build(Lang::Pmc, PMC_STD, 1).unwrap();
    let json = program.map_json();
    assert!(
        json.contains("\"source\": \"user\"") || json.contains("\"source\":\"user\""),
        "{json}"
    );
    assert!(
        json.contains("\"source\": \"std\"") || json.contains("\"source\":\"std\""),
        "{json}"
    );
}
