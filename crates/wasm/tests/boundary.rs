//! The layer under wasm-bindgen is plain Rust: nothing in `src/inner/`
//! may name `wasm_bindgen` or `js_sys`, so it stays testable natively and
//! the JS boundary stays in `lib.rs` + `js.rs`, where a reader expects it.

use std::fs;
use std::path::Path;

#[test]
fn inner_module_never_names_the_js_boundary() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/inner");
    let mut checked = 0;
    for entry in fs::read_dir(&dir).expect("src/inner exists") {
        let path = entry.expect("entry").path();
        if path.extension().is_some_and(|e| e == "rs") {
            let text = fs::read_to_string(&path).expect("readable");
            for needle in ["wasm_bindgen", "js_sys"] {
                assert!(
                    !text.contains(needle),
                    "{} names `{needle}`; the JS boundary belongs in lib.rs/js.rs",
                    path.display()
                );
            }
            checked += 1;
        }
    }
    assert!(
        checked >= 2,
        "expected the inner modules under {}",
        dir.display()
    );
}

#[test]
fn registry_serves_both_arches_per_tape_count() {
    use mtc_wasm::inner::registry::registry_for;
    let r1 = registry_for(1);
    assert!(r1.get(0x01).is_some(), "PM-1 registered");
    assert!(r1.get(0x02).is_some(), "TM-1 registered");
    assert!(r1.get(0x7F).is_none(), "the fake test arch is not");
    let again = registry_for(1);
    assert!(
        std::ptr::eq(r1, again),
        "one registry per tape count, cached"
    );
    let r4 = registry_for(4);
    assert!(
        !std::ptr::eq(r1, r4),
        "a different tape count is a different registry"
    );
}
