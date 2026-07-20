//! The `.tma` lint rules: the three TM-1 assembly defects core's
//! arch-agnostic rules cannot see (docs/tmt/lint.md (the `.tma` additions)).
//! Each rule is a `pub(crate) fn check` over [`super::TmaLintContext`].

pub(crate) mod rept_var_unused;
pub(crate) mod retx_exit_bounds;
pub(crate) mod shadowed_wildcard_rows;
