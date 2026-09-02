//! The layer under the JS boundary: plain Rust, natively testable.

pub mod diagnostics;
pub mod positions;
pub mod registry;

/// Which source language a call is about. The public library APIs of the
/// two toolchains are symmetric for everything the binding exposes, so one
/// class family with a `lang` parameter serves both.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    Pmc,
    Tmc,
}

impl Lang {
    pub fn parse(s: &str) -> Option<Lang> {
        match s {
            "pmc" => Some(Lang::Pmc),
            "tmc" => Some(Lang::Tmc),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Lang::Pmc => "pmc",
            Lang::Tmc => "tmc",
        }
    }
}
