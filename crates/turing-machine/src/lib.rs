//! TM-1: everything specific to the multi-tape Turing architecture, built
//! on the arch-agnostic mtc-core VM. The sibling of the PM-1 crate: where
//! PM-1 drives a single two-symbol tape, TM-1 drives up to sixteen tapes,
//! each with its own alphabet, and dispatches transitions through the
//! shared match/dispatch table engine.

pub mod arch;
