//! The one rule for what a table fixup references (docs/core.md (relaxation)).
//!
//! A table fixup is an untyped `(hole, table_offset)` pair: the object
//! format records no kind, and a concatenated table blob is not
//! self-describing. The kind is inferred from the code bytes around the
//! hole, the same way the disassembler infers it — and every place the
//! linker needs it asks here, so the rule cannot drift between them.

use crate::asm::{ArchSyntax, Flow};
use crate::vm::OperandKind;

/// Which table a fixup hole references. A plain `TableRef` operand sits
/// one byte after its opcode (`Match` if the opcode falls through,
/// `Dispatch` if it transfers); a `FramedCall`'s frame half sits five
/// bytes after its opcode (`Frame`).
#[derive(Clone, Copy)]
pub(super) enum RefKind {
    Match,
    Dispatch,
    Frame,
}

/// The `(opcode offset, RefKind)` for a fixup hole, or `None` when neither
/// a `TableRef` opcode precedes the hole by one byte nor a `FramedCall`
/// opcode precedes it by five (a malformed fixup).
///
/// The one-byte test runs FIRST and wins: the byte five back from a plain
/// table hole is whatever precedes its opcode — often the last byte of an
/// earlier operand, such as the low byte of another table offset — and can
/// equal the framed-call opcode by coincidence. The reverse collision
/// needs only that no `TableRef` opcode is `0x00`: the linker classifies
/// blobs before layout patches them, while a framed call's displacement
/// half is still the zero placeholder the assembler (or the linker's own
/// rewriting) wrote, so the byte before a frame half is always `0x00`.
pub(super) fn ref_kind(syntax: &ArchSyntax, blob: &[u8], hole: u32) -> Option<(u32, RefKind)> {
    debug_assert!(
        syntax
            .by_opcode(0x00)
            .is_none_or(|e| e.operand != OperandKind::TableRef),
        "a table-reference opcode of 0x00 would read a frame half as a plain table"
    );
    if let Some(op) = hole
        .checked_sub(1)
        .and_then(|p| blob.get(p as usize))
        .copied()
        && let Some(entry) = syntax.by_opcode(op)
        && entry.operand == OperandKind::TableRef
    {
        let kind = if entry.flow == Flow::FallThrough {
            RefKind::Match
        } else {
            RefKind::Dispatch
        };
        return Some((hole - 1, kind));
    }
    if let Some(op) = hole
        .checked_sub(5)
        .and_then(|p| blob.get(p as usize))
        .copied()
        && let Some(entry) = syntax.by_opcode(op)
        && entry.operand == OperandKind::FramedCall
    {
        return Some((hole - 5, RefKind::Frame));
    }
    None
}
