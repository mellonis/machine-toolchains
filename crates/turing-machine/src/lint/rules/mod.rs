//! The `.tmc` lint rules (docs/lint.md, once that page lands; substance in
//! prose here until then). One module per rule; each exposes `pub(crate) fn
//! check(&LintContext, &mut Vec<Diagnostic>)`. The rule table lives in the
//! parent module.

pub(crate) mod leftover_debugger;
pub(crate) mod unused_import;
