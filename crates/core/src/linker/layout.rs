//! Layout, call relaxation, and `MX` code emission (spec §9).
//!
//! Each reached function's ORIGINAL blob is decoded exactly once into a
//! `Piece` list. A monotone shrink fixpoint then decides which far calls
//! become short (jump widths never change — only calls can shrink).
//! Finally every function is re-emitted from that SAME original decode,
//! through an offset map built fresh from the converged widths: calls are
//! patched against the final function bases, and jumps are re-encoded at
//! their original width with the offset recomputed through the map (the
//! shrink-only invariant guarantees it still fits).

use std::collections::HashMap;

use super::resolve::FuncRef;
use super::{LinkError, MapFunction};
use crate::asm::decode::{self, Body, DecodedOperand};
use crate::asm::{ArchSyntax, Flow};

/// One classified piece of a function's ORIGINAL blob (offsets are
/// blob-relative, i.e. relative to that function's own `ent`).
enum Piece {
    Verbatim {
        orig: u32,
        bytes: Vec<u8>,
    },
    Jump {
        orig: u32,
        opcode: u8,
        /// Operand width in bytes: 1 (`RelI8`) or 4 (`RelI32`).
        width: u8,
        orig_target: u32,
    },
    /// `orig` is the CALL OPCODE's address; the hole is `orig + 1`.
    CallSite {
        orig: u32,
        callee: usize,
    },
}

impl Piece {
    fn orig(&self) -> u32 {
        match self {
            Piece::Verbatim { orig, .. }
            | Piece::Jump { orig, .. }
            | Piece::CallSite { orig, .. } => *orig,
        }
    }
}

pub(super) struct Built {
    pub code: Vec<u8>,
    pub functions: Vec<MapFunction>,
    pub relaxed_calls: u32,
    pub far_calls: u32,
}

/// Decode `f`'s original blob into a `Piece` list. Decode failure or a
/// call instruction with no matching hole in `f.calls` → `MalformedBlob`.
fn classify(syntax: &ArchSyntax, f: &FuncRef) -> Result<Vec<Piece>, LinkError> {
    let blob = f.blob;
    let call_holes: HashMap<u32, usize> = f.calls.iter().copied().collect();
    let decoded = decode::decode_stream(syntax, blob, 0, blob.len() as u32);

    let mut pieces = Vec::with_capacity(decoded.len());
    for d in decoded {
        let addr = d.addr;
        let len = d.len;
        match d.body {
            Body::Raw(_) => {
                return Err(LinkError::MalformedBlob {
                    symbol: f.name.to_string(),
                    at: addr,
                });
            }
            Body::Instr { mnemonic, operand } => {
                let entry = syntax
                    .by_mnemonic(mnemonic)
                    .expect("mnemonic came from a successful decode against this syntax");
                match (entry.flow, operand) {
                    (Flow::Jump | Flow::Branch, DecodedOperand::RelTarget(orig_target)) => {
                        pieces.push(Piece::Jump {
                            orig: addr,
                            opcode: entry.opcode,
                            width: (len - 1) as u8,
                            orig_target,
                        });
                    }
                    (Flow::Call, DecodedOperand::RelTarget(_)) => {
                        let hole = addr + 1;
                        let Some(&callee) = call_holes.get(&hole) else {
                            return Err(LinkError::MalformedBlob {
                                symbol: f.name.to_string(),
                                at: hole,
                            });
                        };
                        pieces.push(Piece::CallSite { orig: addr, callee });
                    }
                    _ => {
                        pieces.push(Piece::Verbatim {
                            orig: addr,
                            bytes: blob[addr as usize..(addr + len) as usize].to_vec(),
                        });
                    }
                }
            }
        }
    }
    Ok(pieces)
}

fn piece_size(piece: &Piece, is_short: bool) -> u32 {
    match piece {
        Piece::Verbatim { bytes, .. } => bytes.len() as u32,
        Piece::Jump { width, .. } => 1 + u32::from(*width),
        Piece::CallSite { .. } => {
            if is_short {
                2
            } else {
                5
            }
        }
    }
}

/// Full relayout from scratch under the current width vector: per-function
/// sizes, prefix-sum bases (main — `functions[0]` — at 0), and each
/// piece's blob-relative offset within its own function.
fn layout_pass(functions: &[Vec<Piece>], is_short: &[Vec<bool>]) -> (Vec<u32>, Vec<Vec<u32>>) {
    let mut sizes = Vec::with_capacity(functions.len());
    let mut offsets = Vec::with_capacity(functions.len());
    for (pieces, shorts) in functions.iter().zip(is_short) {
        let mut off = 0u32;
        let mut piece_offsets = Vec::with_capacity(pieces.len());
        for (piece, &short) in pieces.iter().zip(shorts) {
            piece_offsets.push(off);
            off += piece_size(piece, short);
        }
        offsets.push(piece_offsets);
        sizes.push(off);
    }
    let mut bases = Vec::with_capacity(functions.len());
    let mut base = 0u32;
    for &size in &sizes {
        bases.push(base);
        base += size;
    }
    (bases, offsets)
}

pub(super) fn build(
    syntax: &ArchSyntax,
    order: &[FuncRef],
    relax: bool,
) -> Result<Built, LinkError> {
    let functions: Vec<Vec<Piece>> = order
        .iter()
        .map(|f| classify(syntax, f))
        .collect::<Result<_, _>>()?;

    // Width vector: every call site starts FAR. Only calls whose opcode has
    // a short partner are ever candidates for the fixpoint.
    let mut is_short: Vec<Vec<bool>> = functions.iter().map(|p| vec![false; p.len()]).collect();
    let has_short_partner: Vec<Vec<bool>> = functions
        .iter()
        .zip(order)
        .map(|(pieces, f)| {
            pieces
                .iter()
                .map(|p| match p {
                    Piece::CallSite { orig, .. } => {
                        syntax.short_of(f.blob[*orig as usize]).is_some()
                    }
                    _ => false,
                })
                .collect()
        })
        .collect();

    if relax {
        loop {
            let (bases, offsets) = layout_pass(&functions, &is_short);
            let mut grew = false;
            for (fi, pieces) in functions.iter().enumerate() {
                for (pi, piece) in pieces.iter().enumerate() {
                    let Piece::CallSite { callee, .. } = piece else {
                        continue;
                    };
                    if is_short[fi][pi] || !has_short_partner[fi][pi] {
                        continue;
                    }
                    let instr_end = bases[fi] + offsets[fi][pi] + 5;
                    let off = i64::from(bases[*callee]) - i64::from(instr_end);
                    if i8::try_from(off).is_ok() {
                        is_short[fi][pi] = true;
                        grew = true;
                    }
                }
            }
            if !grew {
                break;
            }
        }
    }

    // Final, converged layout — the ONLY one used for emission.
    let (bases, offsets) = layout_pass(&functions, &is_short);

    let mut code = Vec::new();
    let mut map_functions = Vec::with_capacity(order.len());
    let mut relaxed_calls = 0u32;
    let mut far_calls = 0u32;

    for (fi, f) in order.iter().enumerate() {
        let pieces = &functions[fi];
        let base = bases[fi];
        let piece_offsets = &offsets[fi];
        debug_assert_eq!(code.len() as u32, base);

        // orig blob offset -> new offset within THIS function; needed to
        // resolve jump targets (arbitrary earlier/later instruction
        // boundaries) and to remap debug labels/lines.
        let orig_to_new: HashMap<u32, u32> = pieces
            .iter()
            .enumerate()
            .map(|(pi, piece)| (piece.orig(), piece_offsets[pi]))
            .collect();

        for (pi, piece) in pieces.iter().enumerate() {
            match piece {
                Piece::Verbatim { bytes, .. } => code.extend_from_slice(bytes),
                Piece::Jump {
                    opcode,
                    width,
                    orig_target,
                    ..
                } => {
                    let new_target = base + orig_to_new[orig_target];
                    let new_end = base + piece_offsets[pi] + 1 + u32::from(*width);
                    let off = i64::from(new_target) - i64::from(new_end);
                    code.push(*opcode);
                    match *width {
                        1 => {
                            debug_assert!(
                                i8::try_from(off).is_ok(),
                                "shrink-only invariant: jump no longer fits its original width"
                            );
                            code.push((off as i8) as u8);
                        }
                        4 => {
                            let off32 = i32::try_from(off).expect("jump offset fits i32");
                            code.extend(off32.to_le_bytes());
                        }
                        _ => unreachable!("jump operand width is always 1 or 4"),
                    }
                }
                Piece::CallSite { orig, callee } => {
                    let far_opcode = f.blob[*orig as usize];
                    let new_start = piece_offsets[pi];
                    if is_short[fi][pi] {
                        let short_opcode = syntax
                            .short_of(far_opcode)
                            .expect("marked short only when a short partner exists");
                        let new_end = base + new_start + 2;
                        let off = i64::from(bases[*callee]) - i64::from(new_end);
                        let off8 = i8::try_from(off)
                            .expect("relaxation fixpoint guarantees short calls fit i8");
                        code.push(short_opcode);
                        code.push(off8 as u8);
                        relaxed_calls += 1;
                    } else {
                        let new_end = base + new_start + 5;
                        let off = i64::from(bases[*callee]) - i64::from(new_end);
                        let off32 = i32::try_from(off).expect("call offset fits i32");
                        code.push(far_opcode);
                        code.extend(off32.to_le_bytes());
                        far_calls += 1;
                    }
                }
            }
        }

        let end = code.len() as u32;

        let (labels, lines) = match f.debug {
            Some(debug) => {
                let labels = debug
                    .labels
                    .iter()
                    .map(|(name, off)| (name.clone(), base + orig_to_new[off]))
                    .collect();
                let lines = debug
                    .lines
                    .iter()
                    .map(|(off, line)| (base + orig_to_new[off], *line))
                    .collect();
                (labels, lines)
            }
            None => (Vec::new(), Vec::new()),
        };

        map_functions.push(MapFunction {
            name: f.name.to_string(),
            start: base,
            end,
            labels,
            lines,
        });
    }

    Ok(Built {
        code,
        functions: map_functions,
        relaxed_calls,
        far_calls,
    })
}

#[cfg(test)]
mod tests {
    use super::super::{LinkOptions, link};
    use crate::asm::assemble;
    use crate::asm::syntax::fixture::test_syntax;
    use crate::asm::{Flow, RelaxPair, SyntaxEntry};
    use crate::vm::OperandKind;

    /// Fixture + a short call (0x31) so relaxation has a target form.
    fn syntax_with_short_call() -> crate::asm::ArchSyntax {
        let mut s = test_syntax();
        s.entries.push(SyntaxEntry {
            opcode: 0x31,
            mnemonic: "call.s",
            operand: OperandKind::RelI8,
            flow: Flow::Call,
        });
        s.relax_pairs.push(RelaxPair {
            far: 0x21,
            short: 0x31,
        });
        s
    }

    const TWO_FUNCS: &str = "\
.func main
        call    go
        stop
.func go
        nop
        ret
";

    #[test]
    fn links_two_functions_with_relaxed_call() {
        let syntax = syntax_with_short_call();
        let obj = assemble(&syntax, 0x7E, TWO_FUNCS, false).unwrap();
        let out = link(&syntax, &[obj], &[], LinkOptions::default()).unwrap();
        // main: [0E][31 off][02] = 4 bytes; go at 4: [0E][01][0B].
        // call.s at 1, end 3, target 4 → off +1.
        assert_eq!(
            out.executable.code,
            vec![0x0E, 0x31, 0x01, 0x02, 0x0E, 0x01, 0x0B]
        );
        assert_eq!(out.executable.entry, 0);
        assert_eq!(out.map.functions.len(), 2);
        assert_eq!(
            (
                out.map.functions[0].name.as_str(),
                out.map.functions[0].start,
                out.map.functions[0].end
            ),
            ("main", 0, 4)
        );
        assert_eq!(
            (
                out.map.functions[1].name.as_str(),
                out.map.functions[1].start,
                out.map.functions[1].end
            ),
            ("go", 4, 7)
        );
    }

    #[test]
    fn no_relax_keeps_far_calls() {
        let syntax = syntax_with_short_call();
        let obj = assemble(&syntax, 0x7E, TWO_FUNCS, false).unwrap();
        let out = link(&syntax, &[obj], &[], LinkOptions { relax: false }).unwrap();
        // main: [0E][21 off32][02] = 7 bytes; go at 7; call end 6 → off +1.
        assert_eq!(
            out.executable.code,
            vec![0x0E, 0x21, 0x01, 0x00, 0x00, 0x00, 0x02, 0x0E, 0x01, 0x0B]
        );
    }

    #[test]
    fn jump_spanning_a_shrunk_call_is_repatched() {
        // THE approved-design case: a backward jump over a call site.
        // L: nop ; call go ; jmp L ; stop  — the jmp crosses the call hole.
        let src = "\
.func main
L:      nop
        call    go
        jmp     L
        stop
.func go
        ret
";
        let syntax = syntax_with_short_call();
        let obj = assemble(&syntax, 0x7E, src, false).unwrap();
        // Original blob: [0E][01][21 hole][30 off][02]: jmp.s at 7..9, end 9,
        // target 1 → orig off = -8.
        assert_eq!(obj.blobs[0][7], 0x30);
        assert_eq!(obj.blobs[0][8] as i8, -8);
        let out = link(&syntax, &[obj], &[], LinkOptions::default()).unwrap();
        // After shrink: [0E][01][31 off][30 off'][02] = 7 bytes; go at 7.
        // call.s at 2, end 4, target 7 → +3. jmp.s at 4..6, end 6, target 1 → -5.
        assert_eq!(
            out.executable.code,
            vec![0x0E, 0x01, 0x31, 0x03, 0x30, 0xFB, 0x02, 0x0E, 0x0B]
        );
    }

    #[test]
    fn debug_offsets_are_remapped() {
        let src = "\
.func main
        call    go
X:      stop
.func go
        ret
";
        let syntax = syntax_with_short_call();
        let obj = assemble(&syntax, 0x7E, src, true).unwrap();
        // Original: X at blob offset 6 (after ent + 5-byte call).
        let out = link(&syntax, &[obj], &[], LinkOptions::default()).unwrap();
        // Relaxed: call.s is 2 bytes → X moves to absolute 3.
        assert_eq!(out.map.functions[0].labels, vec![("X".to_string(), 3)]);
        assert!(!out.map.functions[0].lines.is_empty());
    }

    #[test]
    fn far_call_when_out_of_short_range() {
        // Pad main so the callee lands beyond +127 from the call site.
        let mut src = String::from(".func main\n        call    go\n");
        for _ in 0..150 {
            src.push_str("        nop\n");
        }
        src.push_str("        stop\n.func go\n        ret\n");
        let syntax = syntax_with_short_call();
        let obj = assemble(&syntax, 0x7E, &src, false).unwrap();
        let out = link(&syntax, &[obj], &[], LinkOptions::default()).unwrap();
        assert_eq!(out.executable.code[1], 0x21, "call must stay far");
    }
}
