//! The `pmt` command-line tool: a thin renderer over the library API.
//! Libraries never print; every byte of terminal output originates here.

mod build;
mod completions;
mod fmt;
mod inspect;
mod lint;
mod lsp;
mod run;

use mtc_core::formats::tapeblock::TapeSnapshot;

#[derive(Debug)]
pub struct CliOutput {
    pub stdout: String,
    pub stderr: String,
    pub code: u8,
}

impl CliOutput {
    pub(crate) fn ok(stdout: String, stderr: String) -> Self {
        Self {
            stdout,
            stderr,
            code: 0,
        }
    }
}

const USAGE: &str = "\
pmt — Post-machine toolchain

USAGE: pmt <SUBCOMMAND> [ARGS]

SUBCOMMANDS:
  compile      .pmc source -> .pmo object (-S for .pma, --emit-ir for CFG JSON)
  asm          .pma assembly -> .pmo object
  link         .pmo objects -> .pmx executable (+ .pmx.map sidecar)
  lint         lint .pmc/.pma sources (hygiene findings; docs/lint.md)
  fmt          format .pmc/.pma sources in place (--check to preview; -)
  dis          disassemble a .pmo or .pmx (--listing for the address view)
  run          execute a .pmx on a tape
  tape         build/show .pmt tape-block snapshots
  ir           render --emit-ir JSON (ir graph -> Mermaid)
  lsp          run the LSP server on stdio
  completions  emit a shell completion script (zsh; bash/fish follow-on)

Run `pmt <SUBCOMMAND> --help` for details. `pmt --version` prints the version.
";

pub fn execute(args: &[String]) -> Result<CliOutput, String> {
    execute_with(args, &mut std::io::stderr().lock())
}

/// Writer seam: `--trace` (`docs/cli.md`) streams into `trace_out` live.
/// The bin path passes stderr; tests pass a `Vec<u8>` and assert on it.
pub fn execute_with(
    args: &[String],
    trace_out: &mut dyn std::io::Write,
) -> Result<CliOutput, String> {
    match args.first().map(String::as_str) {
        None | Some("--help") | Some("-h") => Ok(CliOutput::ok(USAGE.into(), String::new())),
        Some("--version") => Ok(CliOutput::ok(
            format!(
                "pmt {}\npmc language {}\npma dialect (pm-1) {}\n",
                env!("CARGO_PKG_VERSION"),
                crate::parser::PMC_LANG_VERSION,
                crate::asm::PM1_PMA_DIALECT_VERSION
            ),
            String::new(),
        )),
        Some("compile") => build::compile(&args[1..]),
        Some("asm") => build::asm(&args[1..]),
        Some("link") => build::link(&args[1..]),
        Some("lint") => lint::lint(&args[1..]),
        Some("fmt") => fmt::fmt(&args[1..]),
        Some("dis") => inspect::dis(&args[1..]),
        Some("tape") => inspect::tape(&args[1..]),
        Some("ir") => inspect::ir(&args[1..]),
        Some("run") => run::run(&args[1..], trace_out),
        Some("lsp") => lsp::lsp(&args[1..]),
        Some("completions") => completions::completions(&args[1..]),
        Some(other) => Err(format!("unknown subcommand `{other}`\n\n{USAGE}")),
    }
}

/// Render one tape with its glyphs: the dense span line plus a caret
/// line under the head. Glyph 0 is blank by convention.
pub(crate) fn render_tape(snapshot: &TapeSnapshot, alphabet: &[String]) -> String {
    let glyph = |index: u8| -> &str {
        alphabet
            .get(usize::from(index))
            .map(String::as_str)
            .unwrap_or("?")
    };
    let mut cells_line = String::new();
    let mut caret_line = String::new();
    for (i, &cell) in snapshot.cells.iter().enumerate() {
        let g = glyph(cell);
        let here = snapshot.origin + i as i64 == snapshot.head;
        cells_line.push_str(g);
        caret_line.push_str(&if here {
            "^".repeat(g.chars().count().max(1))
        } else {
            " ".repeat(g.chars().count().max(1))
        });
    }
    format!(
        "origin {}, head {}\n|{}|\n {}\n",
        snapshot.origin,
        snapshot.head,
        cells_line,
        caret_line.trim_end()
    )
}

/// Minimal flag scanner: flags may appear anywhere; `--name value` and
/// `--name=value` are both accepted; remaining tokens are positionals.
pub(crate) struct Args {
    tokens: Vec<Option<String>>,
}

impl Args {
    pub(crate) fn new(args: &[String]) -> Self {
        Self {
            tokens: args.iter().cloned().map(Some).collect(),
        }
    }

    /// Consume a boolean flag; true if present (first occurrence).
    pub(crate) fn flag(&mut self, name: &str) -> bool {
        for slot in &mut self.tokens {
            if slot.as_deref() == Some(name) {
                *slot = None;
                return true;
            }
        }
        false
    }

    /// Consume `name value` or `name=value`.
    pub(crate) fn value(&mut self, name: &str) -> Result<Option<String>, String> {
        for i in 0..self.tokens.len() {
            let Some(tok) = self.tokens[i].as_deref() else {
                continue;
            };
            if tok == name {
                self.tokens[i] = None;
                let next = self.tokens.get_mut(i + 1).and_then(Option::take);
                return next
                    .ok_or_else(|| format!("{name} needs a value"))
                    .map(Some);
            }
            if let Some(rest) = tok.strip_prefix(&format!("{name}=")) {
                let value = rest.to_string();
                self.tokens[i] = None;
                return Ok(Some(value));
            }
        }
        Ok(None)
    }

    /// Consume every occurrence of a repeatable `name value` flag.
    pub(crate) fn values(&mut self, name: &str) -> Result<Vec<String>, String> {
        let mut out = Vec::new();
        while let Some(v) = self.value(name)? {
            out.push(v);
        }
        Ok(out)
    }

    /// Everything left must be positional (no dashed tokens).
    pub(crate) fn positionals(self) -> Result<Vec<String>, String> {
        let mut out = Vec::new();
        for tok in self.tokens.into_iter().flatten() {
            if tok.starts_with('-') && tok != "-" {
                return Err(format!("unknown flag `{tok}`"));
            }
            out.push(tok);
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_tape_draws_a_single_bordered_span_with_a_caret() {
        // marks {0, 2}, head 2, glyphs " "/"*": the head sits on the last
        // cell — a single `|` border at each end only (per-cell borders
        // would misalign the caret math).
        let snapshot = TapeSnapshot {
            origin: 0,
            cells: vec![1, 0, 1],
            head: 2,
        };
        let alphabet: Vec<String> = vec![" ".into(), "*".into()];
        let rendered = render_tape(&snapshot, &alphabet);
        assert_eq!(rendered, "origin 0, head 2\n|* *|\n   ^\n");
    }
}
