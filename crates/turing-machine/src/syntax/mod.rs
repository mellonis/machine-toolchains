//! The `.tmc` green-syntax layer over the core framework
//! (docs/core.md (syntax trees)): the kind space, the source-layout
//! pass, and the green-tree sink for now, with typed views following
//! in later tasks of the same migration.

mod emit;
mod kinds;
mod layout;

pub use emit::GreenSink;
pub use kinds::{TmcKind, kind_name};
pub use layout::{SigLayout, layout};
// `pub(crate)`, not `pub`: `token_kind` itself is `pub(crate)` (only the
// parser's `bump()` needs it), so re-exporting it any wider than
// `pub(crate)` here would be private-in-public.
pub(crate) use kinds::token_kind;
