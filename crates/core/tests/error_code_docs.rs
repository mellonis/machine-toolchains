//! Drift guard for `docs/core.md (error codes)`: the page publishes the
//! assembler framework's full error-code catalog — the inventory the
//! stability promise ("safe to match in scripts and editor
//! integrations") depends on — and a published inventory without a
//! check rots silently. This file set-compares the page's table against
//! `AsmErrorKind::CODES`, the registry `code()` itself reads, so a code
//! added, renamed, or dropped in source fails here until the page
//! follows (and vice versa).

use mtc_core::asm::AsmErrorKind;

fn doc() -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/core.md");
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

/// `(code, capability)` per table row: the first two `|`-delimited cells,
/// the code stripped of its backticks.
fn table_rows(lines: &[&str]) -> Vec<(String, String)> {
    lines
        .iter()
        .filter(|l| l.starts_with("| `"))
        .map(|l| {
            let mut cells = l.split('|').skip(1).map(str::trim);
            let code = cells
                .next()
                .expect("row has a code cell")
                .trim_matches('`')
                .to_string();
            let capability = cells.next().expect("row has a capability cell").to_string();
            (code, capability)
        })
        .collect()
}

#[test]
fn the_published_catalog_lists_exactly_the_registry_codes() {
    let doc = doc();
    let rows = table_rows(&section(&doc, "### Error codes"));
    let mut documented: Vec<&str> = rows.iter().map(|(c, _)| c.as_str()).collect();
    documented.sort_unstable();
    let mut registry: Vec<&str> = AsmErrorKind::CODES.to_vec();
    registry.sort_unstable();
    assert_eq!(
        documented, registry,
        "docs/core.md (error codes) and AsmErrorKind::CODES disagree — \
         update the page's table to list exactly the registry's codes"
    );
}

#[test]
fn every_capability_cell_names_a_real_assembler_capability() {
    // The capability column is the page's per-dialect reachability key:
    // `—` = reachable in every dialect, otherwise an `AsmCaps` field name.
    let doc = doc();
    let rows = table_rows(&section(&doc, "### Error codes"));
    assert!(!rows.is_empty(), "the catalog table should have rows");
    for (code, capability) in &rows {
        assert!(
            matches!(capability.as_str(), "—" | "tables" | "rept" | "vectors"),
            "row `{code}` names capability `{capability}`, which is not \
             `—` or an AsmCaps field"
        );
    }
}
