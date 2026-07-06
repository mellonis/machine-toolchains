//! One file per lint rule (docs/lint.md). Each rule exposes
//! `pub(crate) fn check(&LintContext, &mut Vec<Diagnostic>)` and is
//! registered in `super::RULES` under its defect-named code.
