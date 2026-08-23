//! The `.tmc` green-syntax layer over the core framework
//! (docs/core.md (syntax trees)): the kind space and the
//! source-layout pass for now, with green emission and typed views
//! following in later tasks of the same migration.

mod kinds;
mod layout;

pub use kinds::{TmcKind, kind_name};
pub use layout::{SigLayout, layout};
