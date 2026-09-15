//! Everything the compiler knows about units other than its own
//! (docs/tmt/language.md (declarations)): the embedded standard library
//! today, headers, sibling sources and library objects later. Owned,
//! because a header is read per invocation — the `&'static` shape the
//! embedded stdlib's `OnceLock` allowed is not available to a per-compile
//! source, so the stdlib's [`Resolved`] is cloned in here rather than
//! borrowed, keeping ONE type for both cases.

use std::path::PathBuf;

use crate::compiler::Resolved;

/// Where one declaration module came from. Read by the diagnostics that
/// must tell "no such name" from "its declarations were not given"
/// (docs/tmt/cli.md (error codes)) and by the graft-digest check
/// (docs/formats.md (routine interfaces)).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Origin {
    /// The embedded standard library, read implicitly unless `--nostdlib`.
    Stdlib,
    /// A `--extern FILE` given on the command line.
    Extern(PathBuf),
    /// Another source of the same build target.
    Sibling(PathBuf),
    /// The interface section of a library object on the `-L` path.
    Library(String),
}

/// Everything the compiler knows about units other than its own
/// (docs/tmt/language.md (declarations)): a callee found in one of these
/// modules contributes its DECLARED effective write set to the footprint
/// inference, projected through the binding; a callee found nowhere
/// contributes the whole alphabet.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Declarations {
    modules: Vec<(Origin, Resolved)>,
}

impl Declarations {
    /// No external declarations known — what the embedded stdlib's own
    /// analysis passes to avoid consulting its own `OnceLock` while it is
    /// being built (`stdlib::object()`, `stdlib::analysis()`).
    pub fn none() -> Self {
        Self::default()
    }

    /// The embedded standard library, cloned in from its process-wide
    /// cache. The implicit default a compile believes unless `--nostdlib`.
    pub fn stdlib() -> Self {
        let mut decls = Self::none();
        decls.push(Origin::Stdlib, crate::stdlib::resolved().clone());
        decls
    }

    /// Add one declaration module.
    pub(crate) fn push(&mut self, origin: Origin, resolved: Resolved) {
        self.modules.push((origin, resolved));
    }

    /// The borrowed view every consumer reads — the exact shape
    /// `ExternalContracts::modules()` returned before this type existed.
    pub(crate) fn modules(&self) -> Vec<&Resolved> {
        self.modules.iter().map(|(_, resolved)| resolved).collect()
    }

    /// Which module a resolved external came from, by identity: two
    /// modules may carry equal declarations from different origins, so
    /// this compares the borrow's address rather than its value. No
    /// production consumer yet — added now, with provenance, so a
    /// diagnostic distinguishing "no such name" from "declared but not
    /// given" and a digest check naming a library (docs/tmt/language.md
    /// (declarations), docs/formats.md (routine interfaces)) do not have
    /// to retrofit this table later.
    #[allow(dead_code)]
    pub(crate) fn origin_of(&self, resolved: &Resolved) -> Option<&Origin> {
        self.modules
            .iter()
            .find(|(_, candidate)| std::ptr::eq(candidate, resolved))
            .map(|(origin, _)| origin)
    }

    /// How many declaration modules this carries — the public shape of
    /// `modules().len()` for callers outside the crate, which cannot name
    /// the crate-private [`Resolved`] type.
    pub fn len(&self) -> usize {
        self.modules.len()
    }

    /// Whether this carries no declaration modules at all.
    pub fn is_empty(&self) -> bool {
        self.modules.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Mutation: `none()` returning a non-empty table. `stdlib()` is proven
    // non-empty separately by `declarations_none_and_stdlib_differ_in_
    // module_count` in `tests/interface_emission.rs`; this unit test keeps
    // the crate-private surface (`push`/`modules`/`origin_of`) exercised
    // without leaking `Resolved` across the crate boundary.
    #[test]
    fn none_carries_no_modules() {
        let decls = Declarations::none();
        assert!(decls.is_empty());
        assert_eq!(decls.len(), 0);
        assert!(decls.modules().is_empty());
    }

    // Mutation: `push` failing to record either the origin or the module
    // itself, or `origin_of` matching by value instead of by identity (two
    // pushed modules with equal contents would then be indistinguishable).
    #[test]
    fn origin_of_finds_the_pushed_module_by_identity() {
        let mut decls = Declarations::none();
        let first = crate::stdlib::resolved().clone();
        let second = crate::stdlib::resolved().clone();
        decls.push(Origin::Extern(PathBuf::from("a.tmh")), first);
        decls.push(Origin::Sibling(PathBuf::from("b.tmc")), second);
        assert_eq!(decls.len(), 2);
        let modules = decls.modules();
        assert_eq!(
            decls.origin_of(modules[0]),
            Some(&Origin::Extern(PathBuf::from("a.tmh")))
        );
        assert_eq!(
            decls.origin_of(modules[1]),
            Some(&Origin::Sibling(PathBuf::from("b.tmc")))
        );
    }
}
