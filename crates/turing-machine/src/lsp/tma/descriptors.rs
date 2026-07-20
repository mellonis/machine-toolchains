//! Frame-descriptor field checks, CST-tier: the `.frame` / `.map` / `.exits`
//! defects the assembler itself rejects, re-derived from the parsed CST so the
//! editor can show every one of them at once.
//!
//! # Why the service repeats checks the assembler already makes
//!
//! Lowering stops at its first offending descriptor and, worse, never runs at
//! all when something unrelated earlier in the file refuses to assemble — so a
//! file with a stray mnemonic in its code section shows an editor NOTHING about
//! the three broken descriptors above it. Reading the descriptors off the total
//! CST removes both limits: the checks are independent of the fatal gate and
//! independent of each other.
//!
//! These are deliberately NOT new findings. Every rule below mirrors one the
//! assembler enforces, and each publishes under the assembler's own `bad-frame`
//! code carrying the assembler's own wording, so a user never sees the editor
//! object to something `tmt asm` would accept. The one finding that duplicates
//! the published fatal is dropped where the channels merge, so no defect is
//! reported twice. Defects the assembler tolerates (a map clause that repeats a
//! source symbol, say, where the last pair silently wins) are out of scope here:
//! flagging those is a lint rule's job, on both surfaces at once, not a
//! service-only opinion.
//!
//! # How a descriptor is delimited
//!
//! A labeled `.frame` header opens a descriptor; `.map` and `.exits` lines
//! continue it; anything else — another `.frame`, a `.func`, a `.section`, an
//! instruction line — closes it. Comments are trivia and pass through without
//! closing anything. This is the same grouping discipline the `.tma` lint rules
//! use to read descriptors, and it matches the assembler's own open-frame
//! state.

use mtc_core::asm::cst::{AsmItemKind, FrameDirectiveCst, FrameMapCst, FramePairCst};
use mtc_core::diagnostics::{Diagnostic, Span};

use super::FlatItem;

/// The assembler's own code for every descriptor defect. Sharing it keeps a
/// CST-tier finding indistinguishable from the fatal it anticipates.
const CODE: &str = "bad-frame";

/// TM-1 drives at most sixteen tapes, so a frame projects at most sixteen.
const MAX_TAPES: usize = 16;

fn finding(span: Span, message: String) -> Diagnostic {
    Diagnostic {
        span,
        code: CODE,
        message,
        fix: None,
    }
}

/// The descriptor currently being accumulated: its arity and the `.map` tape
/// indices and `.exits` lines seen so far.
struct OpenFrame {
    arity: usize,
    seen_maps: Vec<(u32, Span)>,
    exits_lines: Vec<Span>,
}

pub(super) fn check(flat: &[FlatItem]) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    let mut open: Option<OpenFrame> = None;

    for entry in flat {
        match &entry.item.kind {
            // Trivia: neither opens nor closes a descriptor.
            AsmItemKind::Comment(_) => {}
            AsmItemKind::FrameDirective(FrameDirectiveCst::Header(h)) => {
                open = Some(OpenFrame {
                    arity: h.tapes.len(),
                    seen_maps: Vec::new(),
                    exits_lines: Vec::new(),
                });
                if h.tapes.is_empty() || h.tapes.len() > MAX_TAPES {
                    out.push(finding(
                        h.tapes_span,
                        "frame `tapes` list must have 1..=16 entries".to_string(),
                    ));
                }
                for &phys in &h.tapes {
                    if phys > u32::from(u8::MAX) {
                        out.push(finding(
                            h.tapes_span,
                            "physical tape index exceeds 255".to_string(),
                        ));
                        break;
                    }
                }
            }
            AsmItemKind::FrameDirective(FrameDirectiveCst::Map(m)) => {
                let Some(frame) = open.as_mut() else {
                    out.push(finding(
                        m.span,
                        "`.map` has no preceding `.frame`".to_string(),
                    ));
                    continue;
                };
                check_map(m, frame, &mut out);
            }
            AsmItemKind::FrameDirective(FrameDirectiveCst::Exits(e)) => {
                let Some(frame) = open.as_mut() else {
                    out.push(finding(
                        e.span,
                        "`.exits` has no preceding `.frame`".to_string(),
                    ));
                    continue;
                };
                if !frame.exits_lines.is_empty() {
                    out.push(finding(
                        e.span,
                        "`.exits` may appear at most once per frame".to_string(),
                    ));
                }
                frame.exits_lines.push(e.span);
            }
            // Anything else closes the open descriptor.
            _ => open = None,
        }
    }
    out
}

/// One `.map` line against its open descriptor: the tape index must name a
/// virtual tape the frame has, must not repeat, and each map clause must obey
/// the blank pin and the read-direction-only rule for one-way pairs.
fn check_map(m: &FrameMapCst, frame: &mut OpenFrame, out: &mut Vec<Diagnostic>) {
    if (m.k as usize) >= frame.arity {
        out.push(finding(
            m.k_span,
            format!("`.map` tape {} is >= the frame arity {}", m.k, frame.arity),
        ));
    }
    if frame.seen_maps.iter().any(|(k, _)| *k == m.k) {
        out.push(finding(m.k_span, format!("duplicate `.map {}`", m.k)));
    }
    frame.seen_maps.push((m.k, m.k_span));

    if let (Some(pairs), Some(span)) = (&m.rmap, m.rmap_span) {
        check_pairs(pairs, span, out);
    }
    if let (Some(pairs), Some(span)) = (&m.wmap, m.wmap_span) {
        // The one-way (`=>`) spelling is read-direction only: such a pair is
        // excluded from write-back, so it is legal in `rmap` and meaningless
        // in `wmap`.
        if pairs.iter().any(|p| p.one_way) {
            out.push(finding(
                span,
                "one-way pairs (`=>`) are read-direction only; wmap pairs use `->`".to_string(),
            ));
        }
        check_pairs(pairs, span, out);
    }
}

/// The pair-level rules shared by both map directions: index 0 is pinned to
/// identity (blank reads and writes as blank), and no index or value may exceed
/// the wire encoding's ceiling. A non-blank index MAY map onto 0 — folding a
/// symbol onto blank is legal authoring, and whether a fold is sound is the
/// composition engine's question, not this surface's.
fn check_pairs(pairs: &[FramePairCst], span: Span, out: &mut Vec<Diagnostic>) {
    for p in pairs {
        if p.from > 0xFFFE || p.to > 0xFFFE {
            out.push(finding(
                span,
                "frame map index/value exceeds 0xFFFE".to_string(),
            ));
            return;
        }
        if p.from == 0 && p.to != 0 {
            out.push(finding(
                span,
                "frame map unpins blank: 0 must map to 0".to_string(),
            ));
            return;
        }
    }
}
