//! `unused-import`: a `use` whose binding resolves to nothing referenced.
//! The compiler already raises this on the compile channel (`tmt compile`);
//! this rule RE-EXPOSES those findings on the lint channel so a `tmt lint`
//! run and the shared allow-list cover them too (the pmt convention that
//! hygiene warnings answer to `lint.allow`). It re-emits analyze's own
//! `unused-import` diagnostics verbatim rather than recomputing usage — one
//! detection, two channels.

use mtc_core::diagnostics::Diagnostic;

use crate::lint::LintContext;

pub(crate) fn check(ctx: &LintContext, out: &mut Vec<Diagnostic>) {
    for d in ctx.diagnostics {
        if d.code == "unused-import" {
            out.push(d.clone());
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::lint::{LintOptions, lint};

    // `helper` is imported but never referenced anywhere in the machine.
    const SRC: &str = "\
use lib::helper;
alphabet b { '_', '0' }
machine {
  tape t: b;
  entry state s { [*] -> stop; }
}
";

    #[test]
    fn an_unreferenced_import_is_re_exposed_on_the_lint_channel() {
        let report = lint(SRC, LintOptions::default()).unwrap();
        let d: Vec<_> = report
            .diagnostics
            .iter()
            .filter(|d| d.code == "unused-import")
            .collect();
        assert_eq!(d.len(), 1, "{:?}", report.diagnostics);
        assert!(d[0].message.contains("helper"));
    }

    #[test]
    fn allow_suppresses_the_finding() {
        let report = lint(
            SRC,
            LintOptions {
                allow: vec!["unused-import".to_string()],
                warn: Vec::new(),
            },
        )
        .unwrap();
        assert!(
            report
                .diagnostics
                .iter()
                .all(|d| d.code != "unused-import")
        );
    }
}
