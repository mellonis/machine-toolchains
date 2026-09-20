//! Named symbol maps (docs/tmt/language.md (named maps)): a top-level
//! `map NAME: SRC -> DST { pairs }` declaration, exportable/importable
//! alongside `export alphabet`, used at a `call`/`bind`/`graft` site as
//! `with map NAME` beside the inline `with map { … }` form.
//!
//! **The central claim** (`a_named_map_declares_and_is_used_by_name`): a
//! name is a SPELLING, not a semantics — a site naming a declared map
//! compiles to the exact same binding the same site would carry with the
//! pairs written inline.
//!
//! **The four declaration checks**, run ONCE at the declaration
//! (`compiler::resolve_all_maps` -> `expand::check_named_map_decl`): every
//! pair's glyphs resolve in their own alphabet
//! (`map-symbol-not-in-alphabet`), the blank stays pinned
//! (`map-blank-pin`), the map is injective on equal-cardinality alphabets
//! (`map-not-injective`), and — the one check with no inline-form analog
//! — it is CLOSED on unequal-cardinality alphabets: every non-blank source
//! symbol must be named explicitly (`map-not-closed`). The first three
//! reuse the exact codes a graft's own inline map already raises
//! (`expand::build_tapemap`); only the fourth is new (step 1 of this
//! task's brief, recorded in the task report).
//!
//! **Two site checks** (`compiler::expand_named_maps_in_args`): the caller
//! tape's alphabet must be the map's declared SOURCE
//! (`named-map-source-mismatch`), the callee parameter's alphabet must be
//! its declared TARGET (`named-map-target-mismatch`).

use std::path::{Path, PathBuf};

use mtc_turing_machine::cli::{CliOutput, execute};
use mtc_turing_machine::compiler::{CompileErrorKind, CompileOptions, compile};

/// A fresh, per-call fixture directory under `CARGO_TARGET_TMPDIR`, named
/// uniquely by process id + an atomic counter — copied verbatim from
/// `tests/mode_equivalence.rs::scratch` (temp paths must not collide
/// across concurrent `cargo test`/`cargo nextest` processes sharing one
/// `CARGO_TARGET_TMPDIR`).
fn scratch(name: &str) -> PathBuf {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("{name}-{}-{n}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn write(dir: &Path, name: &str, content: &str) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, content).unwrap();
    path
}

fn args(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| s.to_string()).collect()
}

// ---------------------------------------------------------------------------
// The central claim: a name is a spelling, not a semantics.
// ---------------------------------------------------------------------------

/// The spec's own headline shape (task brief), concretely compilable: a
/// `bind` and a direct `call` both binding through the same named map. Not
/// injective — `'^'` and `'$'` both collapse onto `bits`'s blank — which is
/// legal because `wide` (5) and `bits` (3) are UNEQUAL cardinalities, where
/// injectivity is never required; it is the closed-on-unequal case (every
/// one of `wide`'s 4 non-blank symbols is named).
const HEADLINE_MAP_DECL: &str =
    "map wideToBits: wide -> bits { '^' => '_', '$' => '_', '0' -> '0', '1' -> '1' }";
const HEADLINE_INLINE_MAP: &str = "with map { '^' => '_', '$' => '_', '0' -> '0', '1' -> '1' }";

fn headline_src(named: bool) -> String {
    let map_use = if named {
        "with map wideToBits".to_string()
    } else {
        HEADLINE_INLINE_MAP.to_string()
    };
    let decl_line = if named {
        format!("{HEADLINE_MAP_DECL}\n")
    } else {
        String::new()
    };
    format!(
        "\
alphabet wide {{ '_', '^', '$', '0', '1' }}
alphabet bits {{ '_', '0', '1' }}
{decl_line}
routine plusOne(tape num: bits) {{
  entry state s {{ [*] -> return; }}
}}

routine invert(tape num: bits) {{
  entry state s {{ [*] -> return; }}
}}

machine {{
  tape data: wide;
  bind plusOne(num = data {map_use}) as inc;
  entry state go {{ [*] -> call invert(num = data {map_use}) then t; }}
  state t {{ [*] -> stop; }}
}}
"
    )
}

/// **The central claim.** Mutation: expanding a named reference to a
/// DIFFERENT pair order than the declaration's own (e.g. reversing
/// `decl.pairs` before cloning in `compiler::expand_named_maps_in_args`)
/// changes which physical symbol each callee row matches at codegen — the
/// `.tma` text and the assembled object both diverge, since a bound call's
/// pairs print (and assemble) in the order `IrTapeBinding::pairs` carries
/// them. Verified by hand for this task: reversing that clone reds this
/// test (`assert_eq!` on `.tma`), restored afterward — recorded in the
/// task report, not re-asserted here (a hand-applied source mutation is not
/// itself a fixture).
#[test]
fn a_named_map_declares_and_is_used_by_name() {
    let inline = compile(&headline_src(false), CompileOptions::default())
        .unwrap_or_else(|e| panic!("inline form: {e}"));
    let named = compile(&headline_src(true), CompileOptions::default())
        .unwrap_or_else(|e| panic!("named form: {e}"));
    assert_eq!(
        named.tma, inline.tma,
        "a name is a spelling, not a semantics — the generated assembly must match byte for byte"
    );
    assert_eq!(
        named.object.to_bytes(),
        inline.object.to_bytes(),
        "the assembled object must match byte for byte"
    );
}

// ---------------------------------------------------------------------------
// The four declaration checks — one firing / near-miss pair each.
// ---------------------------------------------------------------------------

fn err(src: &str) -> CompileErrorKind {
    compile(src, CompileOptions::default())
        .expect_err("expected a compile error")
        .kind
}

fn compiles(src: &str) {
    compile(src, CompileOptions::default()).unwrap_or_else(|e| panic!("expected success: {e}"));
}

/// Check 1 — every pair's `src`/`dst` resolves in its own alphabet.
/// Equal-cardinality alphabets keep this fixture from also tripping the
/// injectivity or closed checks, isolating the one under test.
#[test]
fn a_pair_naming_a_glyph_outside_its_alphabet_is_refused() {
    let src = "\
alphabet a2 { '_', 'x' }
alphabet b2 { '_', 'y' }
map bad: a2 -> b2 { 'Z' -> 'y' }
";
    assert!(
        matches!(err(src), CompileErrorKind::MapSymbolNotInAlphabet(g) if g == "Z"),
        "{:?}",
        err(src)
    );
}

#[test]
fn the_matching_glyph_pair_compiles() {
    compiles(
        "\
alphabet a2 { '_', 'x' }
alphabet b2 { '_', 'y' }
map ok: a2 -> b2 { 'x' -> 'y' }
",
    );
}

/// Check 2 — the blank (index 0) stays pinned.
#[test]
fn a_pair_that_moves_the_blank_off_itself_is_refused() {
    let src = "\
alphabet a2 { '_', 'x' }
alphabet b2 { '_', 'y' }
map bad: a2 -> b2 { '_' -> 'y' }
";
    assert_eq!(err(src), CompileErrorKind::MapBlankPin);
}

#[test]
fn the_matching_pair_leaving_blank_pinned_compiles() {
    compiles(
        "\
alphabet a2 { '_', 'x' }
alphabet b2 { '_', 'y' }
map ok: a2 -> b2 { 'x' -> 'y' }
",
    );
}

/// Check 3 — injective on EQUAL cardinalities: two `src`s colliding on one
/// `dst`. `a3`/`b3` are both 3 symbols; `x` is EXPLICITLY mapped onto `q`,
/// and `y` — left UNLISTED — identity-completes onto its own index, which
/// is ALSO `q`. (Two pairs written to name the SAME target directly, e.g.
/// `'x' -> 'p', 'y' -> 'p'`, is caught earlier as a write-back `MapConflict`
/// — a different, more fundamental check than identity-completion
/// injectivity; this fixture isolates the latter by leaving `y` unlisted.)
#[test]
fn two_sources_colliding_on_one_target_is_refused_on_equal_cardinalities() {
    let src = "\
alphabet a3 { '_', 'x', 'y' }
alphabet b3 { '_', 'p', 'q' }
map bad: a3 -> b3 { 'x' -> 'q' }
";
    assert!(
        matches!(err(src), CompileErrorKind::MapNotInjective { symbol } if symbol == "q"),
        "{:?}",
        err(src)
    );
}

#[test]
fn a_single_pair_that_identity_completes_injectively_compiles() {
    // `y` (unlisted) identity-completes to `q` — distinct from `p`, so the
    // completed map `{_->_, x->p, y->q}` is a bijection.
    compiles(
        "\
alphabet a3 { '_', 'x', 'y' }
alphabet b3 { '_', 'p', 'q' }
map ok: a3 -> b3 { 'x' -> 'p' }
",
    );
}

/// Check 4 — closed on UNEQUAL cardinalities: every non-blank source must
/// be named explicitly. `wide` (5) and `bits` (3) differ; this fixture
/// drops `'1'`, the one symbol `HEADLINE_MAP_DECL` (which names all four)
/// does not.
#[test]
fn an_unmapped_source_is_refused_on_unequal_cardinalities() {
    let src = "\
alphabet wide { '_', '^', '$', '0', '1' }
alphabet bits { '_', '0', '1' }
map bad: wide -> bits { '^' => '_', '$' => '_', '0' -> '0' }
";
    assert!(
        matches!(err(src), CompileErrorKind::MapNotClosed(g) if g == "1"),
        "{:?}",
        err(src)
    );
}

/// The near miss is the headline declaration itself: `HEADLINE_MAP_DECL`
/// names `wide`'s all four non-blank symbols (`'^'`, `'$'`, `'0'`, `'1'`)
/// even though two of them collapse onto the same target — closed, not
/// injective, which unequal cardinalities never require (the brief's own
/// warning: this shape is NOT the injectivity fixture).
#[test]
fn the_headline_map_names_every_non_blank_source_and_compiles() {
    compiles(&format!(
        "alphabet wide {{ '_', '^', '$', '0', '1' }}\n\
         alphabet bits {{ '_', '0', '1' }}\n\
         {HEADLINE_MAP_DECL}\n"
    ));
}

// ---------------------------------------------------------------------------
// The two site checks.
// ---------------------------------------------------------------------------

fn site_src(caller_alphabet: &str) -> String {
    format!(
        "\
alphabet wide {{ '_', '^', '$', '0', '1' }}
alphabet bits {{ '_', '0', '1' }}
alphabet other {{ '_', 'Q' }}
{HEADLINE_MAP_DECL}

routine plusOne(tape num: bits) {{
  entry state s {{ [*] -> return; }}
}}

machine {{
  tape data: {caller_alphabet};
  entry state go {{ [*] -> call plusOne(num = data with map wideToBits) then stop; }}
}}
"
    )
}

/// Mutation: checking only the TARGET side (the callee parameter's
/// alphabet against the map's declared DST) — this site's callee side is
/// correct (`plusOne`'s `num` is over `bits`, the map's own declared
/// target), so a check that verified only that side would let this
/// compile; only the CALLER side (`data: other`, not `wide`) is wrong.
#[test]
fn a_site_whose_caller_alphabet_is_not_the_maps_source_is_refused() {
    let src = site_src("other");
    assert!(
        matches!(
            err(&src),
            CompileErrorKind::NamedMapSourceMismatch { map, expected, found }
                if map == "wideToBits" && expected == "wide" && found == "other"
        ),
        "{:?}",
        err(&src)
    );
}

#[test]
fn the_matching_site_compiles() {
    compiles(&site_src("wide"));
}

/// The other half of the site check: the callee parameter's alphabet must
/// be the map's declared TARGET. `oddOut`'s own tape is over `other`
/// (neither `bits` nor `wide`), so a correct caller side (`data: wide`)
/// still refuses.
#[test]
fn a_site_whose_callee_alphabet_is_not_the_maps_target_is_refused() {
    let src = "\
alphabet wide { '_', '^', '$', '0', '1' }
alphabet bits { '_', '0', '1' }
alphabet other { '_', 'Q' }
map wideToBits: wide -> bits { '^' => '_', '$' => '_', '0' -> '0', '1' -> '1' }

routine oddOut(tape num: other) {
  entry state s { [*] -> return; }
}

machine {
  tape data: wide;
  entry state go { [*] -> call oddOut(num = data with map wideToBits) then stop; }
}
";
    assert!(
        matches!(
            err(src),
            CompileErrorKind::NamedMapTargetMismatch { map, expected, found }
                if map == "wideToBits" && expected == "bits" && found == "other"
        ),
        "{:?}",
        err(src)
    );
}

// ---------------------------------------------------------------------------
// Cross-unit: an exported map reaches the header, and an imported one is
// usable at a site.
// ---------------------------------------------------------------------------

const LIB_TMC: &str = "\
namespace mylib {
  export alphabet wide { '_', '^', '$', '0', '1' }
  export alphabet bits { '_', '0', '1' }
  export map wideToBits: wide -> bits { '^' => '_', '$' => '_', '0' -> '0', '1' -> '1' }
}
";

fn run_interface(path: &Path) -> CliOutput {
    let out = execute(&args(&["interface", path.to_str().unwrap()]))
        .unwrap_or_else(|e| panic!("interface {}: {e}", path.display()));
    assert_eq!(out.code, 0, "interface {}: {}", path.display(), out.stderr);
    out
}

#[test]
fn an_exported_map_reaches_the_header() {
    let dir = scratch("named_maps_header");
    let path = write(&dir, "lib.tmc", LIB_TMC);
    let out = run_interface(&path);
    assert!(
        out.stdout.contains(
            "export map wideToBits: wide -> bits { '^' => '_', '$' => '_', '0' -> '0', '1' -> '1' }"
        ),
        "{}",
        out.stdout
    );

    // The printed header re-parses under the strict declarations-only
    // reader: feed it back through `interface` itself (which reads either
    // a `.tmc` or a header the same way — `crate::header::from_source`
    // runs `ReadMode::DeclarationsOnly` when the extension is `.tmh`).
    let header_path = write(&dir, "lib.tmh", &out.stdout);
    let round = run_interface(&header_path);
    assert_eq!(
        round.stdout, out.stdout,
        "the header round-trips byte for byte"
    );
}

#[test]
fn an_imported_map_is_usable_at_a_site() {
    let dir = scratch("named_maps_import");
    let lib_path = write(&dir, "lib.tmc", LIB_TMC);
    let lib_header = run_interface(&lib_path).stdout;
    let header_path = write(&dir, "lib.tmh", &lib_header);

    let consumer = "\
use mylib::wide, mylib::bits, mylib::wideToBits;

routine plusOne(tape num: bits) {
  entry state s { [*] -> return; }
}

machine {
  tape data: wide;
  entry state go { [*] -> call plusOne(num = data with map wideToBits) then stop; }
}
";
    let consumer_path = write(&dir, "consumer.tmc", consumer);
    let out_path = dir.join("consumer.tmo");
    let argv = args(&[
        "compile",
        consumer_path.to_str().unwrap(),
        "--extern",
        header_path.to_str().unwrap(),
        "-o",
    ]);
    let mut argv = argv;
    argv.push(out_path.to_str().unwrap().to_string());
    let out = execute(&argv).unwrap_or_else(|e| panic!("compile consumer: {e}"));
    assert_eq!(out.code, 0, "{}", out.stderr);
}
