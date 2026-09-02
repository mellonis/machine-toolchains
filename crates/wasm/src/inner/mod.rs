//! The layer under the JS boundary: plain Rust, natively testable.

pub mod diagnostics;
pub mod listing;
pub mod positions;
pub mod program;
pub mod registry;
pub mod session;
pub mod tapeblock;

/// The two architectures. Everything downstream of a build — the
/// program, its sessions, the listing, the registry — keys on this alone;
/// which text the program came from is a front-end matter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Arch {
    Pm1,
    Tm1,
}

/// Which language a call is about: an architecture crossed with a kind,
/// source (`.pmc`/`.tmc`) or assembly (`.pma`/`.tma`). The public library
/// APIs of the two toolchains are symmetric for everything the binding
/// exposes, so one class family with a `lang` parameter serves all four.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    Pmc,
    Tmc,
    Pma,
    Tma,
}

impl Lang {
    pub const ALL: [Lang; 4] = [Lang::Pmc, Lang::Tmc, Lang::Pma, Lang::Tma];

    pub fn parse(s: &str) -> Option<Lang> {
        match s {
            "pmc" => Some(Lang::Pmc),
            "tmc" => Some(Lang::Tmc),
            "pma" => Some(Lang::Pma),
            "tma" => Some(Lang::Tma),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Lang::Pmc => "pmc",
            Lang::Tmc => "tmc",
            Lang::Pma => "pma",
            Lang::Tma => "tma",
        }
    }

    pub fn arch(self) -> Arch {
        match self {
            Lang::Pmc | Lang::Pma => Arch::Pm1,
            Lang::Tmc | Lang::Tma => Arch::Tm1,
        }
    }

    /// Assembly text, taken by the assembler rather than the compiler.
    pub fn is_asm(self) -> bool {
        matches!(self, Lang::Pma | Lang::Tma)
    }
}
