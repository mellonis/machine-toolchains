//! Green emission for the `.pmc` parser — core's language-agnostic
//! [`GreenSink`] instantiated over this crate's kind space
//! (docs/core.md (syntax trees)). The parser stays the single owner of
//! grammar decisions — the sink only mirrors token consumption and
//! node boundaries, so the green tree and the parser's errors can
//! never disagree. The sink's own mechanics are unit-tested in core
//! against a fake kind space; nothing `.pmc`-specific remains here.

use super::kinds::PmcKind;

/// The `.pmc` green sink — core's, with this crate's kind space.
pub type GreenSink = mtc_core::syntax::GreenSink<PmcKind>;
