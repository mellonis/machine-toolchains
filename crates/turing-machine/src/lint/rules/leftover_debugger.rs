//! `leftover-debugger`: a `debugger` marker left on a rule. It lowers to a
//! `brk` (docs/core.md (debug break)); an un-stripped `brk` is an optimizer
//! observability barrier, so shipping one also pessimizes `-O1` output.
//! Report-only — a `debugger` sits inside a rule action, so there is no
//! whole-line deletion a source-level fix could offer safely.

use mtc_core::diagnostics::Diagnostic;

use crate::lint::LintContext;

pub(crate) fn check(ctx: &LintContext, out: &mut Vec<Diagnostic>) {
    for world in &ctx.resolved.worlds {
        for state in &world.states {
            for rule in &state.rules {
                if rule.debugger {
                    out.push(Diagnostic {
                        code: "leftover-debugger",
                        span: rule.span,
                        message: "leftover 'debugger' marker".to_string(),
                        fix: None,
                    });
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::lint::{LintOptions, lint};

    const SRC: &str = "\
alphabet bit { '_', '1' }
machine {
  tape t: bit;
  entry state s {
    ['1'] -> debugger goto s;
    ['_'] -> stop;
  }
}
";

    #[test]
    fn a_debugger_marker_fires() {
        let report = lint(SRC, LintOptions::default()).unwrap();
        let d: Vec<_> = report
            .diagnostics
            .iter()
            .filter(|d| d.code == "leftover-debugger")
            .collect();
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].message, "leftover 'debugger' marker");
        assert!(d[0].fix.is_none());
    }

    #[test]
    fn allow_suppresses_the_finding() {
        let report = lint(
            SRC,
            LintOptions {
                allow: vec!["leftover-debugger".to_string()],
                warn: Vec::new(),
            },
        )
        .unwrap();
        assert!(
            report
                .diagnostics
                .iter()
                .all(|d| d.code != "leftover-debugger")
        );
    }

    #[test]
    fn a_clean_program_is_quiet() {
        let clean = "\
alphabet bit { '_', '1' }
machine {
  tape t: bit;
  entry state s { [*] -> stop; }
}
";
        let report = lint(clean, LintOptions::default()).unwrap();
        assert!(
            report
                .diagnostics
                .iter()
                .all(|d| d.code != "leftover-debugger")
        );
    }
}
