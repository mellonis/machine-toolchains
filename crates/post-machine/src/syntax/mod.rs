//! The `.pmc` green-syntax layer over the core framework
//! (docs/core.md (syntax tree)): the kind space, the source-layout
//! pass, green emission for the existing parser, and typed views over
//! the resulting tree.

mod emit;
mod kinds;
mod layout;
mod views;

pub use emit::GreenSink;
pub use kinds::{PmcKind, kind_name};
pub use layout::{SigLayout, layout};
pub use views::{
    DocRunView, FileView, FnHeader, FunctionView, ItemView, LabelView, NamespaceView,
    StatementView, TopView, UseDeclView, UsePathView,
};
