//! The arch registry, with a `'static` lifetime.
//!
//! `Machine<'a>` and `AsyncSession<'a>` borrow the registry they were
//! loaded from, and a wasm-bindgen class cannot carry a lifetime — so the
//! registry has to outlive everything, i.e. be `'static`. One instance
//! serves every program: `Tm1::new`'s own doc
//! (`crates/turing-machine/src/arch/mod.rs`) says the arch is
//! width-agnostic and the constructor only validates the declared tape
//! count as a sanity check, retaining nothing — so a single `Tm1` handles
//! every tape count. The registry is built once and leaked on first use.
//! It lives in a `thread_local!`, not a plain `static`, because `Arch`
//! carries no `Send + Sync` bound and a `static OnceLock` cannot hold it;
//! a wasm page is single-threaded, so in practice this is one instance
//! per process.

use std::cell::OnceCell;

use mtc_core::vm::ArchRegistry;
use mtc_post_machine::arch::Pm1;
use mtc_turing_machine::arch::Tm1;

thread_local! {
    static REGISTRY: OnceCell<&'static ArchRegistry> = const { OnceCell::new() };
}

/// The registry holding PM-1 and TM-1. The same reference comes back on
/// every call.
pub fn registry() -> &'static ArchRegistry {
    REGISTRY.with(|cell| {
        *cell.get_or_init(|| {
            let mut registry = ArchRegistry::new();
            registry.register(Box::new(Pm1));
            registry.register(Box::new(Tm1::new(1)));
            Box::leak(Box::new(registry))
        })
    })
}
