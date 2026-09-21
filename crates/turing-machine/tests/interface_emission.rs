//! Pins the compiled stdlib object across the `Declarations` refactor that
//! widened `ExternalContracts` into an owned table with provenance
//! (docs/tmt/language.md (declarations)), and carries the
//! interface-emission tests that refactor exists for
//! (docs/formats.md (routine interfaces)): every compiled world's `.param`
//! lines, the `writes=` suffix's effective-set/preserves semantics, exported
//! alphabets reaching `Interface.alphabets`, and the text round trip codegen's
//! glyph spelling depends on.

use mtc_core::formats::crc32::crc32;
use mtc_core::formats::object::{ObjectFile, RoutineInterface, SymbolDef};
use mtc_turing_machine::asm::assemble;
use mtc_turing_machine::compiler::{CompileOptions, Declarations, compile};
use mtc_turing_machine::optimizer::OptLevel;
use mtc_turing_machine::stdlib;

/// A byte-level fingerprint (length, CRC-32) — cheap to inline and exact
/// enough to catch any change to the serialized object, without committing
/// a multi-kilobyte byte array to the test source.
fn fingerprint(bytes: &[u8]) -> (usize, u32) {
    (bytes.len(), crc32(bytes))
}

/// The code blobs alone, length-prefixed and concatenated — a fingerprint
/// that moves iff codegen's CODE output changes, independent of the
/// interface section this task adds beside it.
fn blobs_bytes(o: &ObjectFile) -> Vec<u8> {
    let mut v = Vec::new();
    for b in &o.blobs {
        v.extend_from_slice(&(b.len() as u32).to_le_bytes());
        v.extend_from_slice(b);
    }
    v
}

/// The [`RoutineInterface`] for the routine/machine named `name`, found by
/// its symbol's blob index — `Interface.routines` is parallel to `blobs`,
/// like `signatures`.
fn routine_interface<'a>(object: &'a ObjectFile, name: &str) -> &'a RoutineInterface {
    let symbol = object
        .symbols
        .iter()
        .find(|s| s.name == name)
        .unwrap_or_else(|| panic!("no symbol named `{name}` in {:?}", object.symbols));
    let blob = match symbol.def {
        SymbolDef::Defined { blob } | SymbolDef::Local { blob } => blob,
        SymbolDef::External => panic!("`{name}` is external, not defined in this object"),
    };
    &object
        .interface
        .as_ref()
        .unwrap_or_else(|| panic!("object carries no interface section"))
        .routines[blob as usize]
}

/// The compiled stdlib object's CODE never moves — the whole-object pin
/// below is deliberately allowed to move in THIS task, because the object
/// now carries an interface section beside its code (docs/formats.md
/// (routine interfaces)); this is the negative control that keeps that
/// expected delta from hiding a real codegen regression. Mutation: any
/// change to codegen's instruction output, independent of the interface
/// section.
#[test]
fn the_stdlib_code_blobs_are_byte_identical_to_before_the_interface_section() {
    let o0 = compile(
        stdlib::SOURCE,
        CompileOptions {
            opt_level: OptLevel::O0,
            ..CompileOptions::default()
        },
    )
    .expect("the embedded stdlib compiles at -O0")
    .object;
    let o1 = compile(
        stdlib::SOURCE,
        CompileOptions {
            opt_level: OptLevel::O1,
            strip_debugger: true,
            ..CompileOptions::default()
        },
    )
    .expect("the embedded stdlib compiles at the release preset")
    .object;

    // Captured from HEAD eba68db (pre-Task-3, before codegen emitted any
    // `.param` line) — the code blobs alone, so an interface-section-only
    // change to the whole object cannot move this.
    assert_eq!(
        fingerprint(&blobs_bytes(&o0)),
        (1740, 1487017377),
        "the -O0 stdlib object's CODE blobs moved"
    );
    assert_eq!(
        fingerprint(&blobs_bytes(&o1)),
        (1680, 1286225706),
        "the -O1 (release preset) stdlib object's CODE blobs moved"
    );
}

/// The compiled stdlib object's whole-object fingerprint. Re-pinned here a
/// second time: the stdlib's 12 exported graphs now carry `.graph <name>,
/// <digest>` lines and reach `Interface.graphs` (docs/formats.md (routine
/// interfaces)), so the object legitimately GROWS again — -O0 moves 7243 →
/// 7825 bytes, -O1 (release preset) 7183 → 7765 bytes, both a 582-byte
/// interface-section addition and nothing else. The code-blob-only pin
/// above is the negative control proving the CODE itself did not move
/// alongside it: whole-object byte identity is expected to move whenever
/// the interface section's own content changes (it moved once already,
/// when every compiled world first started carrying `.param` lines), and
/// this is the one place that expected delta is asserted instead of a
/// straight equality against the pre-change tree. Mutation this catches:
/// anything in the stdlib's own compile path (opt pipeline, codegen, the
/// `externals` table it is built with) producing different bytes than
/// before.
#[test]
fn the_stdlib_object_is_byte_identical_at_both_opt_levels() {
    let o0 = compile(
        stdlib::SOURCE,
        CompileOptions {
            opt_level: OptLevel::O0,
            ..CompileOptions::default()
        },
    )
    .expect("the embedded stdlib compiles at -O0")
    .object
    .to_bytes();
    let o1 = compile(
        stdlib::SOURCE,
        CompileOptions {
            opt_level: OptLevel::O1,
            strip_debugger: true,
            ..CompileOptions::default()
        },
    )
    .expect("the embedded stdlib compiles at the release preset")
    .object
    .to_bytes();

    assert_eq!(
        fingerprint(&o0),
        (7825, 3034593110),
        "the -O0 stdlib object's bytes moved"
    );
    assert_eq!(
        fingerprint(&o1),
        (7765, 1710262935),
        "the -O1 (release preset) stdlib object's bytes moved"
    );
}

/// `Declarations::none()` and `Declarations::stdlib()` must not silently
/// converge on the same module count — the two constructors that replaced
/// `ExternalContracts::None`/`::Stdlib`. Mutation: making `none()` return
/// the stdlib module (e.g. delegating to `stdlib()` instead of
/// `Self::default()`).
#[test]
fn declarations_none_and_stdlib_differ_in_module_count() {
    assert_eq!(Declarations::none().len(), 0);
    assert_eq!(Declarations::stdlib().len(), 1);
}

/// `CompileOptions::default()` must keep believing the stdlib's declared
/// write contracts, exactly as `ExternalContracts::default() == Stdlib` did
/// before this task introduced `Declarations`, whose OWN default
/// (`Declarations::default() == Declarations::none()`) is empty. Mutation:
/// deriving `Default` for `CompileOptions` instead of hand-writing it,
/// which would let `externals` fall back to `Declarations::default()` and
/// silently stop believing the stdlib by default.
#[test]
fn compile_options_default_still_believes_the_stdlib() {
    assert_eq!(
        CompileOptions::default().externals.len(),
        Declarations::stdlib().len()
    );
}

// -- .param emission (docs/formats.md (routine interfaces)) ----------------

/// Mutation: emitting glyphs in alphabet declaration order rather than band
/// order. The two tapes draw from DIFFERENT alphabets on purpose — a
/// transposition or a declaration-order slip is invisible on a single
/// shared alphabet but shows up on the second tape's assertion here.
#[test]
fn a_compiled_routine_carries_its_parameter_names_and_glyphs() {
    let src = "\
alphabet ctl { '_', '0', '1' }
alphabet data { '_', 'x', 'y' }
export routine twoTape(tape c: ctl, tape d: data) {
  entry state s { [*, *] -> return; }
}
machine {
  tape m: ctl;
  entry state go { [*] -> stop; }
}
";
    let object = compile(src, CompileOptions::default())
        .expect("compiles")
        .object;
    let routine = routine_interface(&object, "twoTape");
    assert_eq!(routine.params, vec!["c".to_string(), "d".to_string()]);
    assert_eq!(
        routine.glyphs,
        vec![
            vec!["_".to_string(), "0".to_string(), "1".to_string()],
            vec!["_".to_string(), "x".to_string(), "y".to_string()],
        ]
    );
}

/// The wire has no spelling for "no restriction declared" — an absent
/// `writes=` decodes as "writes nothing" (docs/formats.md (routine
/// interfaces)) — so the ONLY thing that legitimately empties `writes[0]`
/// is the tape actually writing nothing, never the mere absence of a
/// `writes`/`preserves` clause: `byNeither` below writes nothing in its
/// body (a bare `return`), so its INFERRED footprint is independently
/// empty, and that — not the absent clause — is why no suffix prints. The
/// pair with [`the_writes_suffix_lists_the_effective_set`] and with
/// [`an_uncontracted_routine_publishes_its_inferred_write_set`], which
/// pins the other half: an uncontracted tape that DOES write something
/// still publishes it. Mutation: falling back to the whole alphabet for
/// an uncontracted tape (what a naive "no clause -> unrestricted" reading
/// of `compiler::declared_effective` would do) instead of its inferred
/// footprint — `byNeither`'s decoded `writes` would then be the whole
/// alphabet instead of empty, and this assertion goes red.
#[test]
fn the_writes_suffix_is_absent_only_when_the_routine_writes_nothing() {
    let src = "\
alphabet bits { '_', '0', '1' }
export routine byNeither(tape a: bits) {
  entry state s { [*] -> return; }
}
machine {
  tape m: bits;
  entry state go { [*] -> stop; }
}
";
    let object = compile(src, CompileOptions::default())
        .expect("compiles")
        .object;
    let routine = routine_interface(&object, "byNeither");
    assert!(
        routine.writes[0].is_empty(),
        "a tape that writes nothing must print no `writes=` suffix, \
         decoding to an empty set: got {:?}",
        routine.writes[0]
    );
}

/// A tape declaring NEITHER `writes` nor `preserves`, whose body
/// unconditionally writes one glyph of a three-glyph alphabet: the object
/// must publish exactly that glyph, never an empty set — an uncontracted
/// routine still has to describe what it actually writes, because the
/// wire's only spelling for "empty" means "writes nothing"
/// (docs/formats.md (routine interfaces)). The inferred set comes from
/// the same sound-upper-bound footprint analysis `check_contracts` runs
/// to validate a DECLARED contract (`footprint::infer_resolved_with`),
/// applied here in the absence of one. Mutation: leaving `IrTape.writes`
/// as `None`/empty for an uncontracted tape instead of filling it from
/// inference — the object would then claim `byInference` writes nothing,
/// which is false.
#[test]
fn an_uncontracted_routine_publishes_its_inferred_write_set() {
    let src = "\
alphabet bits { '_', '0', '1' }
export routine byInference(tape a: bits) {
  entry state s { [*] -> write ['1'] return; }
}
machine {
  tape m: bits;
  entry state go { [*] -> stop; }
}
";
    let object = compile(src, CompileOptions::default())
        .expect("compiles")
        .object;
    let routine = routine_interface(&object, "byInference");
    assert_eq!(routine.writes[0], vec!["1".to_string()]);
}

/// The pair with [`the_writes_suffix_is_absent_only_when_the_routine_writes_nothing`].
#[test]
fn the_writes_suffix_lists_the_effective_set() {
    let src = "\
alphabet bits { '_', '0', '1' }
export routine byWrites(tape a: bits writes { '1' }) {
  entry state s { [*] -> return; }
}
machine {
  tape m: bits;
  entry state go { [*] -> stop; }
}
";
    let object = compile(src, CompileOptions::default())
        .expect("compiles")
        .object;
    let routine = routine_interface(&object, "byWrites");
    assert_eq!(routine.writes[0], vec!["1".to_string()]);
}

/// The `std::…::invertNumber` shape (`preserves { '_' }`, no `writes`
/// clause). Mutation: filling `IrTape.writes` from the raw `writes` clause
/// instead of `compiler::declared_effective` — this tape would then print
/// no suffix at all, publishing permission to write the preserved blank the
/// source forbids. Nothing else in this suite catches that.
#[test]
fn a_preserves_only_clause_prints_the_alphabet_minus_the_preserved_glyphs() {
    let src = "\
alphabet bits { '_', '0', '1' }
export routine byPreserves(tape a: bits preserves { '_' }) {
  entry state s { [*] -> return; }
}
machine {
  tape m: bits;
  entry state go { [*] -> stop; }
}
";
    let object = compile(src, CompileOptions::default())
        .expect("compiles")
        .object;
    let routine = routine_interface(&object, "byPreserves");
    assert_eq!(routine.writes[0], vec!["0".to_string(), "1".to_string()]);
}

// -- exported alphabets (docs/formats.md (routine interfaces)) -------------

/// Mutation: the alphabet-fill dropping the `exported` filter, or naming
/// the wrong alphabet. `bits` is `export`ed; `hidden` is not; both are
/// declared so a naive "copy every alphabet" implementation would pass
/// this test, which is why [`a_local_alphabet_does_not`] exists beside it.
#[test]
fn an_exported_alphabet_reaches_the_interface_section() {
    let src = "\
export alphabet bits { '_', '0', '1' }
alphabet hidden { '_', 'x' }
export routine mark(tape a: bits) {
  entry state s { [*] -> return; }
}
machine {
  tape m: bits;
  entry state go { [*] -> stop; }
}
";
    let object = compile(src, CompileOptions::default())
        .expect("compiles")
        .object;
    let iface = object
        .interface
        .as_ref()
        .expect("the object carries an interface section");
    assert!(
        iface.alphabets.iter().any(|a| a.name == "bits"
            && a.glyphs == vec!["_".to_string(), "0".to_string(), "1".to_string()]),
        "exported alphabet `bits` missing from Interface.alphabets: {:?}",
        iface.alphabets
    );
}

/// Mutation: dropping the `exported` filter — the local alphabet `hidden`
/// would then leak into `Interface.alphabets` too.
#[test]
fn a_local_alphabet_does_not() {
    let src = "\
export alphabet bits { '_', '0', '1' }
alphabet hidden { '_', 'x' }
export routine mark(tape a: bits) {
  entry state s { [*] -> return; }
}
machine {
  tape m: bits;
  entry state go { [*] -> stop; }
}
";
    let object = compile(src, CompileOptions::default())
        .expect("compiles")
        .object;
    let iface = object
        .interface
        .as_ref()
        .expect("the object carries an interface section");
    assert!(
        !iface.alphabets.iter().any(|a| a.name == "hidden"),
        "local (non-exported) alphabet `hidden` leaked into \
         Interface.alphabets: {:?}",
        iface.alphabets
    );
}

// -- the text round trip (docs/formats.md (glyph literals and glyph lists))

/// `compile -S`, then assemble that text independently, then byte-compare
/// the two objects — the round-trip gate. No alphabet here is `export`ed,
/// so the known text-expressibility exception for exported/imported
/// alphabets (docs/formats.md (routine interfaces)) never enters into it:
/// this test isolates `.param` glyph-list spelling alone. Mutation: any
/// `.param` spelling the assembler cannot read back.
#[test]
fn emitted_assembly_reassembles_to_the_same_object() {
    let src = "\
alphabet bits { '_', '0', '1' }
export routine byWrites(tape a: bits writes { '1' }) {
  entry state s { [*] -> return; }
}
machine {
  tape m: bits;
  entry state go { [*] -> stop; }
}
";
    let output = compile(src, CompileOptions::default()).expect("compiles");
    let reassembled = assemble(&output.tma, false).expect("the emitted `.tma` reassembles");
    assert_eq!(
        output.object.to_bytes(),
        reassembled.to_bytes(),
        "compiled object and independently reassembled `.tma` diverged"
    );
}

/// The same round trip over an alphabet carrying every shape a `.tmc`
/// alphabet can hold in one label: a multi-character label (`'ab'`), a
/// multi-scalar ZWJ cluster (the family emoji sequence — the same fixture
/// `lexer.rs::glyph_zwj_sequence_is_one_glyph` pins), a bare multi-digit
/// numeric label (`10`, `11`, `12` from a numeric range) and an escaped
/// quote glyph. No alphabet here is `export`ed, for the same reason as
/// above. Mutation: re-narrowing the `.tma` glyph literal back to one
/// character (Task 2b's change); this test goes red at the reassemble
/// step, which is exactly where the old plan would have written a fourth
/// text-expressibility exception instead of widening the notation.
#[test]
fn every_glyph_a_tmc_alphabet_can_hold_round_trips() {
    let zwj = "👨\u{200d}👩\u{200d}👧";
    let src = format!(
        "\
alphabet w {{ '_', 'ab', '{zwj}', '\\'', 10..12 }}
export routine label(tape a: w) {{
  entry state s {{ [*] -> return; }}
}}
machine {{
  tape m: w;
  entry state go {{ [*] -> stop; }}
}}
"
    );
    let output = compile(&src, CompileOptions::default()).expect("compiles");
    let routine = routine_interface(&output.object, "label");
    assert_eq!(
        routine.glyphs[0],
        vec![
            "_".to_string(),
            "ab".to_string(),
            zwj.to_string(),
            "'".to_string(),
            "10".to_string(),
            "11".to_string(),
            "12".to_string(),
        ]
    );
    let reassembled = assemble(&output.tma, false).expect("the emitted `.tma` reassembles");
    assert_eq!(
        output.object.to_bytes(),
        reassembled.to_bytes(),
        "compiled object and independently reassembled `.tma` diverged"
    );
}
