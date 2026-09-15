//! Drift guard for `docs/tmt/cli.md`'s error-code inventory: the page
//! promises the bracketed `[CODE]` suffixes are stable identifiers safe
//! to match in scripts, and publishes the compile-error catalog — a
//! published inventory without a check rots silently. This file
//! set-compares the page's table against `CompileErrorKind::CODES`, the
//! registry `code()` itself reads, so a code added, renamed, or dropped
//! in source fails here until the page follows (and vice versa). The
//! assembly namespace is shared framework territory: its catalog lives
//! once on `docs/core.md (error codes)` (guarded from the core crate),
//! and this page must point there rather than fork a copy.

use mtc_turing_machine::CompileErrorKind;

fn doc() -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/tmt/cli.md");
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()))
}

/// The lines of the section opened by `heading`, up to the next heading
/// of any level.
fn section<'a>(doc: &'a str, heading: &str) -> Vec<&'a str> {
    let mut lines = doc.lines();
    for line in lines.by_ref() {
        if line.trim_end() == heading {
            break;
        }
    }
    lines.take_while(|l| !l.starts_with('#')).collect()
}

/// The backticked code in each table row's first cell.
fn table_codes(lines: &[&str]) -> Vec<String> {
    lines
        .iter()
        .filter(|l| l.starts_with("| `"))
        .map(|l| {
            l.split('|')
                .nth(1)
                .expect("row has a code cell")
                .trim()
                .trim_matches('`')
                .to_string()
        })
        .collect()
}

#[test]
fn the_published_compile_catalog_lists_exactly_the_registry_codes() {
    let doc = doc();
    let mut documented = table_codes(&section(&doc, "### Compile errors"));
    documented.sort_unstable();
    let mut registry: Vec<String> = CompileErrorKind::CODES
        .iter()
        .map(|c| c.to_string())
        .collect();
    registry.sort_unstable();
    assert_eq!(
        documented, registry,
        "docs/tmt/cli.md (compile errors) and CompileErrorKind::CODES \
         disagree — update the page's table to list exactly the \
         registry's codes"
    );
}

/// The published link-warning catalog on the CLI page lists exactly the
/// linker's registry — the same two-way set-compare the compile-error
/// catalog gets.
///
/// Mutation it catches: add a code to `DIAGNOSTIC_CODES` without a row
/// here (or leave a row behind after retiring a code) and the sorted
/// vectors differ.
#[test]
fn the_published_link_warning_catalog_lists_exactly_the_registry_codes() {
    let doc = doc();
    let mut published = table_codes(&section(&doc, "### Link warnings"));
    published.sort();
    assert!(
        !published.is_empty(),
        "no `### Link warnings` table in docs/tmt/cli.md"
    );
    let mut registry: Vec<String> = mtc_core::linker::DIAGNOSTIC_CODES
        .iter()
        .map(|(c, _)| (*c).to_string())
        .collect();
    registry.sort();
    assert_eq!(published, registry, "docs/tmt/cli.md (### Link warnings)");
}

#[test]
fn the_assembly_section_points_at_the_shared_catalog() {
    let doc = doc();
    let lines = section(&doc, "## `tmt asm`");
    assert!(
        lines
            .iter()
            .any(|l| l.contains("docs/core.md (error codes)")),
        "docs/tmt/cli.md (tmt asm) should cite the shared catalog at \
         docs/core.md (error codes)"
    );
}
