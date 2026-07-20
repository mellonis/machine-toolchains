//! The `.tmc` lint rules (docs/tmt/lint.md (the `.tmc` rules)). One module per
//! rule; each exposes `pub(crate) fn check(&LintContext, &mut
//! Vec<Diagnostic>)`. The rule table lives in the parent module.

pub(crate) mod binding_product_threshold;
pub(crate) mod dead_rule;
pub(crate) mod deprecated_call;
pub(crate) mod leftover_debugger;
pub(crate) mod redundant_identity_pairs;
pub(crate) mod state_may_trap;
pub(crate) mod unused_binding;
pub(crate) mod unused_graft_instance;
pub(crate) mod unused_graph;
pub(crate) mod unused_import;
pub(crate) mod unused_routine;
pub(crate) mod writes_through_collapse;
