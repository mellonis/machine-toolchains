//! The `.pmc` green-syntax layer over the core framework
//! (docs/core.md (syntax tree)): the kind space, the source-layout
//! pass, and green emission for the existing parser. Views arrive in
//! a later migration step.

mod emit;
mod kinds;
mod layout;

pub use emit::GreenSink;
pub use kinds::{PmcKind, kind_name};
pub use layout::{SigLayout, layout};
