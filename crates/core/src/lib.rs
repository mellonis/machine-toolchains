//! Arch-agnostic machine-toolchains core: container formats, VM core,
//! linker, assembler/disassembler frameworks, tape devices.

#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

#[cfg(feature = "std")]
pub mod asm;
#[cfg(feature = "std")]
pub mod diagnostics;
#[cfg(feature = "std")]
pub mod formats;
#[cfg(feature = "std")]
pub mod framing;
#[cfg(feature = "std")]
pub mod linker;
#[cfg(feature = "std")]
pub mod lsp;
pub mod vm;
