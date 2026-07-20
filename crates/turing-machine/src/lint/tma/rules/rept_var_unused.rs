//! `rept-var-unused`: a `.rept v, lo, hi` … `.endr` block whose loop variable
//! is never substituted in the body — every iteration expands identically, so
//! the loop is a hygiene smell (a copy-paste count masquerading as a macro).
//!
//! # What counts as a use
//!
//! `.rept` substitution only touches `{expr}` markers; text outside braces is
//! copied verbatim (docs/formats.md (the `.rept` macro)). So the variable is
//! "used" iff it appears as an identifier inside some `{…}` in the body — a
//! bare `v` in a comment or a mnemonic is not a use. To stay on the safe side
//! of a false positive, the scan is conservative: it reads the block's raw
//! body source (comments included) and flags only when NO `{…}` anywhere
//! mentions the variable as a whole-word identifier. The lint runs behind the
//! assemble fatal gate, so every `{…}` here already evaluated cleanly (a
//! `{unknownvar}` would have refused the file), and the block is balanced.

use mtc_core::asm::cst::{AsmItemKind, ReptCst};
use mtc_core::diagnostics::Diagnostic;

use crate::lint::tma::TmaLintContext;

pub(crate) fn check(ctx: &TmaLintContext, out: &mut Vec<Diagnostic>) {
    for item in &ctx.cst.items {
        if let AsmItemKind::Rept(rept) = &item.kind
            && !body_uses_var(ctx.source, rept)
        {
            out.push(Diagnostic {
                code: "rept-var-unused",
                span: rept.span,
                message: format!(
                    "the `.rept` loop variable `{}` is never used in the body — every iteration expands identically",
                    rept.var
                ),
                fix: None,
            });
        }
    }
}

/// True when some `{…}` in the block's raw body source mentions `rept.var` as
/// a whole-word identifier. The body is the physical lines strictly between
/// the `.rept` header and its `.endr` (the CST records both spans).
fn body_uses_var(source: &str, rept: &ReptCst) -> bool {
    let lines: Vec<&str> = source.lines().collect();
    // 1-based line numbers → 0-based indices. The body is header_line+1
    // through endr_line-1, i.e. 0-based `header_line ..= endr_line-2`.
    let header = rept.span.start.line as usize;
    let endr = rept.endr_span.start.line as usize;
    let hi = endr.saturating_sub(1).min(lines.len());
    let lo = header.min(hi);
    let body = lines[lo..hi].join("\n");
    fragments_mention(&body, &rept.var)
}

/// Scan every `{…}` fragment for a whole-word occurrence of `var`.
fn fragments_mention(text: &str, var: &str) -> bool {
    let mut rest = text;
    while let Some(open) = rest.find('{') {
        let after = &rest[open + 1..];
        let Some(close) = after.find('}') else {
            break; // unbalanced (unreachable past the assemble gate)
        };
        if mentions_identifier(&after[..close], var) {
            return true;
        }
        rest = &after[close + 1..];
    }
    false
}

/// True when `var` appears in `fragment` as a whole identifier token
/// (`[alpha_][alnum_]*`, the substitution grammar's identifier rule), not as
/// a substring of a longer name.
fn mentions_identifier(fragment: &str, var: &str) -> bool {
    let bytes = fragment.as_bytes();
    let is_word = |b: u8| b.is_ascii_alphanumeric() || b == b'_';
    let mut i = 0;
    while i < bytes.len() {
        if is_word(bytes[i]) && (i == 0 || !is_word(bytes[i - 1])) {
            let start = i;
            while i < bytes.len() && is_word(bytes[i]) {
                i += 1;
            }
            if &fragment[start..i] == var {
                return true;
            }
        } else {
            i += 1;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use crate::lint::tma::lint_tma;

    fn findings(src: &str) -> Vec<String> {
        lint_tma(src, &[])
            .unwrap()
            .into_iter()
            .filter(|d| d.code == "rept-var-unused")
            .map(|d| format!("{}:{}", d.span.start.line, d.message))
            .collect()
    }

    #[test]
    fn a_rept_that_never_uses_its_var_fires() {
        // The body is `nop` × 3 — `v` never appears in a `{…}`.
        let src = "\
.routine main, tapes=2, alpha=(2, 2)
.section code
.func main
.rept v, 0, 2
        nop
.endr
        stp
";
        let f = findings(src);
        assert_eq!(f.len(), 1, "{f:?}");
        assert!(f[0].starts_with("4:"), "points at the .rept header: {f:?}");
        assert!(f[0].contains("`v`"), "{f:?}");
    }

    #[test]
    fn a_var_used_in_a_label_is_a_use() {
        // `Ln{v}:` substitutes the variable into the label — a use.
        let src = "\
.routine main, tapes=2, alpha=(2, 2)
.section code
.func main
.rept v, 0, 2
Ln{v}:  nop
.endr
        stp
";
        assert!(findings(src).is_empty(), "{:?}", findings(src));
    }

    #[test]
    fn a_var_used_in_a_row_expression_is_a_use() {
        // The brainfuck Tinc shape: `{v}` in the match row's operand.
        let src = "\
.routine main, tapes=2, alpha=(2, 128)
.section tables
.rept v, 0, 126
T0: .row [*, {v}]
.endr
.section code
.func main
        rd
        mtc T0
        stp
";
        assert!(findings(src).is_empty(), "{:?}", findings(src));
    }

    #[test]
    fn a_var_used_only_in_an_arithmetic_expression_is_a_use() {
        // `{v+1}` mentions `v` inside an expression — still a use.
        let src = "\
.routine main, tapes=2, alpha=(2, 130)
.section tables
.rept v, 0, 125
T0: .row [*, {v+1}]
.endr
.section code
.func main
        rd
        mtc T0
        stp
";
        assert!(findings(src).is_empty(), "{:?}", findings(src));
    }

    #[test]
    fn a_var_name_only_in_a_comment_is_not_a_use() {
        // `; step v` mentions `v` in prose, never in a `{…}` — the body still
        // expands identically, so the rule fires.
        let src = "\
.routine main, tapes=2, alpha=(2, 2)
.section code
.func main
.rept v, 0, 2
        nop             ; step v
.endr
        stp
";
        let f = findings(src);
        assert_eq!(f.len(), 1, "a bare `v` in a comment is not a substitution use: {f:?}");
    }
}
