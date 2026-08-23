//! The `.tmc` green-syntax layer over the core framework
//! (docs/core.md (syntax trees)): the kind space for now, with green
//! emission and typed views following in later tasks of the same
//! migration.

mod kinds;

pub use kinds::{TmcKind, kind_name};
