//! `retx-exit-bounds`: a `retx #k` whose `k` is at or past the exit count of
//! the frame active when it runs — the return always traps (exit-out-of-range,
//! docs/formats.md (framed calls, traps, and multi-exit returns)).
//!
//! # Which frame governs a `retx`
//!
//! A `retx #k` returns through exit `k` of the ACTIVE frame's exit vector, and
//! the active frame is the one a `call.m <target>, <frame>` installs for the
//! duration of the call. So the frame governing a `retx` inside function `F`
//! is the descriptor named by a `call.m F, <frame>` — the callee, not the
//! caller (the exit labels themselves live caller-side, but the exit COUNT is
//! the descriptor's, and it bounds the callee's returns). The rule maps each
//! `retx`'s owning function to the frames in-file `call.m`s bind to it,
//! resolves those descriptors' exit counts, and flags a `k` that is out of
//! range.
//!
//! # In-file resolution only
//!
//! Resolution is in-file: a routine reached only from another translation
//! unit's `call.m` has no visible descriptor here, so its `retx`es are SKIPPED
//! silently (cross-file exit binding is a linker concern). When a routine is
//! bound by in-file `call.m`s to more than one DISTINCT frame descriptor, the
//! governing exit count is context-dependent and the rule stays silent on that
//! routine (soundness over completeness — no false positive). The common
//! hand-authored case is one descriptor per callee, resolved exactly. The
//! lint runs behind the assemble fatal gate, so `k`, the frame descriptors,
//! and the `call.m` operands are all well-formed by the time it is reached.
//!
//! # `.rept` bodies
//!
//! `call.m` bindings inside a `.rept` body are collected too, so a routine
//! bound to one frame at top level and another inside a `.rept` reads as
//! multiply-bound (ambiguous) rather than as a single unambiguous binding. A
//! templated frame operand such as `F{v}` resolves to no in-file descriptor
//! and so counts as an unknown-arity binding, which the ambiguity discipline
//! above turns into a silent skip — the sound direction. A `retx` inside a
//! `.rept` body is itself not scanned: a completeness-only limitation that
//! can only miss a finding, never produce a wrong one.

use std::collections::BTreeSet;

use mtc_core::asm::cst::{AsmItem, AsmItemKind, FrameDirectiveCst, OperandToken};
use mtc_core::diagnostics::{Diagnostic, Span};

use crate::lint::tma::TmaLintContext;

/// A `retx` occurrence: its owning function, the requested exit `k`, and the
/// instruction's span.
struct Retx {
    owner: String,
    k: usize,
    span: Span,
}

/// Push the open frame group's `(label, exit count)` to `out` and clear it.
fn close_frame(open: &mut Option<(String, usize)>, out: &mut Vec<(String, usize)>) {
    if let Some(frame) = open.take() {
        out.push(frame);
    }
}

/// Parse a `#<n>` immediate operand's numeric value.
fn parse_imm(text: &str) -> Option<usize> {
    text.trim().strip_prefix('#').and_then(|n| n.parse().ok())
}

/// Record a `call.m`'s `(callee, frame label)` binding, if both operands are
/// present.
fn push_call(operands: &[OperandToken], calls: &mut Vec<(String, String)>) {
    if let (Some(target), Some(frame)) = (operands.first(), operands.get(1)) {
        calls.push((
            target.text.trim().to_string(),
            frame.text.trim().to_string(),
        ));
    }
}

/// Collect `call.m` bindings inside a `.rept` body — a routine bound to one
/// frame at top level and another inside a `.rept` is correctly seen as
/// multiply-bound. Only `call.m` is scanned here: a `retx` inside a `.rept`
/// body is deliberately not (see the module doc — a completeness-only limit,
/// never a wrong finding). Nested `.rept` cannot occur in a shaped body (the
/// CST degrades it), but recursion keeps the scan total.
fn collect_rept_calls(items: &[AsmItem], calls: &mut Vec<(String, String)>) {
    for item in items {
        match &item.kind {
            AsmItemKind::Line(line) => {
                if let Some(instr) = &line.instr
                    && instr.word == "call.m"
                {
                    push_call(&instr.operands, calls);
                }
            }
            AsmItemKind::Rept(rept) => collect_rept_calls(&rept.body, calls),
            _ => {}
        }
    }
}

pub(crate) fn check(ctx: &TmaLintContext, out: &mut Vec<Diagnostic>) {
    let mut frame_exits: Vec<(String, usize)> = Vec::new();
    let mut calls: Vec<(String, String)> = Vec::new(); // (callee, frame label)
    let mut retxs: Vec<Retx> = Vec::new();

    let mut current_func: Option<&str> = None;
    // The frame-descriptor group being accumulated: (label, exit count). It is
    // extended by `.map`/`.exits` continuation lines and closed (pushed to
    // `frame_exits`) by anything else — a `.frame` header, a `.func`, an
    // instruction line, a `.section`. Comments are trivia and pass through.
    let mut open_frame: Option<(String, usize)> = None;

    for item in &ctx.cst.items {
        match &item.kind {
            AsmItemKind::Comment(_) => {}
            AsmItemKind::FrameDirective(FrameDirectiveCst::Map(_)) => {} // continues the group
            AsmItemKind::FrameDirective(FrameDirectiveCst::Exits(e)) => {
                if let Some(frame) = &mut open_frame {
                    frame.1 = e.targets.len();
                }
            }
            AsmItemKind::FrameDirective(FrameDirectiveCst::Header(h)) => {
                close_frame(&mut open_frame, &mut frame_exits);
                open_frame = Some((h.label.name.clone(), 0));
            }
            AsmItemKind::Func(f) => {
                close_frame(&mut open_frame, &mut frame_exits);
                current_func = Some(&f.name);
            }
            AsmItemKind::Line(line) => {
                close_frame(&mut open_frame, &mut frame_exits);
                let Some(instr) = &line.instr else { continue };
                match instr.word.as_str() {
                    "call.m" => push_call(&instr.operands, &mut calls),
                    "retx" => {
                        if let (Some(func), Some(op)) = (current_func, instr.operands.first())
                            && let Some(k) = parse_imm(&op.text)
                        {
                            retxs.push(Retx {
                                owner: func.to_string(),
                                k,
                                span: instr.word_span,
                            });
                        }
                    }
                    _ => {}
                }
            }
            // A `.rept` is structural (it closes an open frame group), but its
            // body still carries `call.m` bindings the ambiguity check needs.
            AsmItemKind::Rept(rept) => {
                close_frame(&mut open_frame, &mut frame_exits);
                collect_rept_calls(&rept.body, &mut calls);
            }
            _ => close_frame(&mut open_frame, &mut frame_exits),
        }
    }
    close_frame(&mut open_frame, &mut frame_exits);

    for retx in &retxs {
        // The distinct in-file frame labels bound to this routine.
        let labels: BTreeSet<&str> = calls
            .iter()
            .filter(|(callee, _)| *callee == retx.owner)
            .map(|(_, frame)| frame.as_str())
            .collect();
        // Each governing descriptor's exit count, `None` when the descriptor
        // is not in this file — a cross-file frame, or a `.rept` substitution
        // template like `F{v}`. An unresolved label is a binding of UNKNOWN
        // arity, so it must not be dropped: dropping it would leave a lone
        // resolvable sibling looking unambiguous and invent a finding.
        let counts: Vec<Option<usize>> = labels
            .iter()
            .map(|label| {
                frame_exits
                    .iter()
                    .find(|(name, _)| name == label)
                    .map(|(_, count)| *count)
            })
            .collect();
        // Exactly one governing descriptor, and its exit count known → an
        // unambiguous bound. Zero (cross-file), several distinct descriptors,
        // or an unknown-arity binding all leave the count context-dependent.
        if counts.len() != 1 {
            continue;
        }
        let Some(count) = counts[0] else { continue };
        if retx.k >= count {
            out.push(Diagnostic {
                code: "retx-exit-bounds",
                span: retx.span,
                message: format!(
                    "retx #{} is out of range — the governing frame declares {} exit(s) (valid #0..#{}), so this return always traps",
                    retx.k,
                    count,
                    count.saturating_sub(1)
                ),
                fix: None,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::lint::tma::lint_tma;

    fn findings(src: &str) -> Vec<String> {
        lint_tma(src, &[])
            .unwrap()
            .into_iter()
            .filter(|d| d.code == "retx-exit-bounds")
            .map(|d| format!("{}:{}", d.span.start.line, d.message))
            .collect()
    }

    /// A two-exit frame `Fh` installed on `helper` by a `call.m helper, Fh`.
    /// `helper`'s `retx #k` is bounded by Fh's two exits (valid #0, #1).
    const MILESTONE: &str = "\
.routine main, tapes=2, alpha=(2, 2)
.routine helper, tapes=2, alpha=(2, 2)
.section tables
Fh: .frame tapes=(1, 0)
    .exits done, other
.section code
.func main
        call.m helper, Fh
done:   stp
other:  hlt
.func helper
        wr   [1, -]
        retx #K
";

    #[test]
    fn a_retx_past_the_exit_vector_fires() {
        // `retx #2` under a 2-exit frame always traps.
        let f = findings(&MILESTONE.replace("#K", "#2"));
        assert_eq!(f.len(), 1, "{f:?}");
        assert!(f[0].contains("out of range"), "{f:?}");
    }

    #[test]
    fn a_retx_at_the_exit_count_fires() {
        // Off-by-one: #1 is the last valid exit for a 2-exit frame, so #2 is
        // the first bad one — but two exits means valid #0, #1; `retx #2` is
        // covered above. Here confirm the boundary the OTHER way: a 1-exit
        // frame makes `retx #1` out of range.
        let one_exit = MILESTONE.replace(".exits done, other", ".exits done");
        let f = findings(&one_exit.replace("#K", "#1"));
        assert_eq!(f.len(), 1, "{f:?}");
    }

    #[test]
    fn a_retx_in_range_is_silent() {
        // `retx #1` under a 2-exit frame is valid.
        assert!(
            findings(&MILESTONE.replace("#K", "#1")).is_empty(),
            "{:?}",
            findings(&MILESTONE.replace("#K", "#1"))
        );
        // `retx #0` too.
        assert!(findings(&MILESTONE.replace("#K", "#0")).is_empty());
    }

    #[test]
    fn a_retx_with_no_in_file_frame_is_skipped() {
        // `helper` is never the target of an in-file `call.m` — its governing
        // frame is cross-file, so the rule stays silent even on `retx #9`.
        let src = "\
.routine helper, tapes=2, alpha=(2, 2)
.section code
.func helper
        wr   [1, -]
        retx #9
";
        assert!(findings(src).is_empty(), "{:?}", findings(src));
    }

    #[test]
    fn ambiguous_multi_frame_binding_is_skipped() {
        // `helper` is bound by two distinct in-file frames with different
        // exit counts (F1: 1 exit, F2: 3). The governing count is
        // context-dependent, so a `retx #2` (bad under F1, fine under F2) is
        // NOT flagged.
        let src = "\
.routine main, tapes=2, alpha=(2, 2)
.routine helper, tapes=2, alpha=(2, 2)
.section tables
F1: .frame tapes=(1, 0)
    .exits a1
F2: .frame tapes=(1, 0)
    .exits b0, b1, b2
.section code
.func main
        call.m helper, F1
        call.m helper, F2
a1:     stp
b0:     stp
b1:     stp
b2:     hlt
.func helper
        wr   [1, -]
        retx #2
";
        assert!(findings(src).is_empty(), "{:?}", findings(src));
    }

    #[test]
    fn a_call_m_binding_inside_a_rept_body_is_seen() {
        // `helper` is bound to F1 (1 exit) at top level AND to F2 (3 exits)
        // from inside a `.rept` body. Seeing only the top-level F1 would
        // misread the routine as unambiguously one-exit and wrongly flag
        // `retx #2`; the `.rept` binding makes it multiply-bound, so the
        // governing count is context-dependent → silent.
        let src = "\
.routine main, tapes=2, alpha=(2, 2)
.routine helper, tapes=2, alpha=(2, 2)
.section tables
F1: .frame tapes=(1, 0)
    .exits a1
F2: .frame tapes=(1, 0)
    .exits b0, b1, b2
.section code
.func main
        call.m helper, F1
        .rept v, 0, 0
        call.m helper, F2
        .endr
a1:     stp
b0:     stp
b1:     stp
b2:     hlt
.func helper
        wr   [1, -]
        retx #2
";
        assert!(findings(src).is_empty(), "{:?}", findings(src));
    }
}
