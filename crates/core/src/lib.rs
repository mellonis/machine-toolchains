//! Arch-agnostic machine-toolchains core: container formats, VM core,
//! linker, assembler/disassembler frameworks, tape devices.

#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

#[cfg(feature = "std")]
pub mod asm;
#[cfg(feature = "std")]
pub mod dap;
#[cfg(feature = "std")]
pub mod diagnostics;
#[cfg(feature = "std")]
pub mod formats;
#[cfg(feature = "std")]
pub mod framing;
#[cfg(feature = "std")]
pub mod linemap;
#[cfg(feature = "std")]
pub mod linker;
#[cfg(feature = "std")]
pub mod lsp;
#[cfg(feature = "std")]
pub mod source_path;
#[cfg(feature = "std")]
pub mod syntax;
pub mod vm;
