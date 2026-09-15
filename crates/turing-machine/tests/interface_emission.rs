//! Pins the compiled stdlib object across the `Declarations` refactor that
//! widens `ExternalContracts` into an owned table with provenance
//! (docs/tmt/language.md (declarations)) — a no-behaviour-change task. Task
//! 3 fills this file out with the interface-emission tests the refactor
//! exists to carry.

use mtc_core::formats::crc32::crc32;
use mtc_turing_machine::compiler::{CompileOptions, Declarations, compile};
use mtc_turing_machine::optimizer::OptLevel;
use mtc_turing_machine::stdlib;

/// A byte-level fingerprint (length, CRC-32) — cheap to inline and exact
/// enough to catch any change to the serialized object, without committing
/// a multi-kilobyte byte array to the test source.
fn fingerprint(bytes: &[u8]) -> (usize, u32) {
    (bytes.len(), crc32(bytes))
}

/// The compiled stdlib object must not move a byte across the
/// `Declarations` refactor, at either opt level — captured from the
/// pre-refactor tree (HEAD ae6e58b) before any production code changed.
/// Mutation this catches: anything in the stdlib's own compile path (opt
/// pipeline, codegen, the `externals` table it is built with) producing
/// different bytes than before.
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
        (6103, 0x4d43769b),
        "the -O0 stdlib object's bytes moved"
    );
    assert_eq!(
        fingerprint(&o1),
        (6043, 0x8d0eb94f),
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
