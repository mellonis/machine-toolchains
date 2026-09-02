//! Arch registries with a `'static` lifetime.
//!
//! `Machine<'a>` and `AsyncSession<'a>` borrow the registry they were
//! loaded from, and a wasm-bindgen class cannot carry a lifetime — so the
//! registry has to outlive everything, i.e. be `'static`. A single global
//! does not work: `Tm1::new(tape_count)` is per program. Instead one
//! registry per tape count is leaked on first use and cached; the cache is
//! bounded by the `u8` tape count, so the leak is at most 255 small boxes
//! per process, and a browser page is one process.

use std::cell::RefCell;
use std::collections::HashMap;

use mtc_core::vm::ArchRegistry;
use mtc_post_machine::arch::Pm1;
use mtc_turing_machine::arch::Tm1;

thread_local! {
    static REGISTRIES: RefCell<HashMap<u8, &'static ArchRegistry>> = RefCell::new(HashMap::new());
}

/// The registry holding PM-1 and TM-1 (the latter sized for `tape_count`
/// tapes). The same reference comes back for the same count.
pub fn registry_for(tape_count: u8) -> &'static ArchRegistry {
    REGISTRIES.with(|cache| {
        *cache.borrow_mut().entry(tape_count).or_insert_with(|| {
            let mut registry = ArchRegistry::new();
            registry.register(Box::new(Pm1));
            registry.register(Box::new(Tm1::new(tape_count)));
            Box::leak(Box::new(registry))
        })
    })
}
