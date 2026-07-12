//! One file per assembly lint rule (docs/lint.md). Each rule exposes
//! `pub(crate) fn check(&AsmLintContext, &mut Vec<Diagnostic>)` and is
//! registered in `super::RULES` under its defect-named code.

pub(crate) mod unreachable_code;
pub(crate) mod unused_label;
