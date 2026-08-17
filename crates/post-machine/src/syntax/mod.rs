//! The `.pmc` green-syntax layer over the core framework
//! (docs/core.md (syntax tree)): the kind space, the source-layout
//! pass, and green emission for the existing parser. Views arrive in
//! a later migration step.

mod kinds;

pub use kinds::{PmcKind, kind_name};
