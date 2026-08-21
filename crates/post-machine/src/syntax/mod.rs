//! The `.pmc` green-syntax layer over the core framework
//! (docs/core.md (syntax tree)): the kind space, the source-layout
//! pass, green emission for the existing parser, and typed views over
//! the resulting tree.

mod emit;
mod extract;
mod kinds;
mod layout;
mod views;

pub use emit::GreenSink;
pub use extract::extract_program;
// `pub(crate)`, not `pub`: `extract_statement` itself is `pub(crate)` (only
// the `.pmc` language service, inside this crate, needs one statement's
// item internals — see its own doc comment) — `syntax` is a `pub mod`
// (crate::syntax::extract_program is used from this crate's own
// integration tests), so re-exporting a `pub(crate)` item any wider than
// `pub(crate)` here would be private-in-public.
pub(crate) use extract::extract_statement;
pub use kinds::{PmcKind, kind_name};
pub use layout::{SigLayout, layout};
pub use views::{
    DocRunView, FileView, FnHeader, FunctionView, ItemView, LabelView, NamespaceView,
    StatementView, TopView, UseDeclView, UsePathView,
};
