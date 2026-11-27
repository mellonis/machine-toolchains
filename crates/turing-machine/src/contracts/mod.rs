//! Compiler-planted checks for the things a `.tmc` signature DECLARES about
//! itself, as opposed to the things the compiler can prove or disprove from
//! a body alone.
//!
//! A static check refuses a program whose own rules contradict a
//! declaration; a planted check is what a declaration the body neither
//! proves nor disproves reduces to — a few synthesized states in a DEBUG
//! build that stop the machine where the declaration turns out to be false
//! (docs/tmt/language.md (head-position clauses)). Both halves exist for
//! every declaration kind that has one: the static halves live with the
//! rest of resolution in `compiler.rs`, and the planted ones live here,
//! each a rewrite over the lowered IR.

pub(crate) mod head;
