//! One file per lint rule (docs/lint.md). Each rule exposes
//! `pub(crate) fn check(&LintContext, &mut Vec<Diagnostic>)` and is
//! registered in `super::RULES` under its defect-named code.

pub(crate) mod confusable_names;
pub(crate) mod identical_check_arms;
pub(crate) mod leading_zeros;
pub(crate) mod leftover_debugger;
pub(crate) mod line_too_long;
pub(crate) mod namespaced_main;
pub(crate) mod non_camel_case;
pub(crate) mod redundant_jump;
pub(crate) mod shadowed_import;
pub(crate) mod unused_label;
