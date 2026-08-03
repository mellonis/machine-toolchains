//! The `tmt` command-line tool: a thin renderer over the library API.
//! Libraries never print; every byte of terminal output originates here.
//! The sibling of `crates/post-machine/src/cli/mod.rs` — the TM-1 front
//! (compile / asm / link / dis / run / tape / ir), mirroring `pmt`'s shapes
//! with `.tmc`/`.tma`/`.tmo`/`.tmx`/`.tmt` extensions.
//!
//! `CliOutput` and the hand-rolled `Args` scanner below are copied
//! verbatim-adapted from the PM-1 `pmt` CLI. Hoisting the shared shell
//! (`CliOutput`, `Args`, `render_tape`) into `mtc-core` is a later-phase
//! decision — until a third tool exists, two near-identical copies read
//! more plainly than a premature abstraction.

// `pub(crate)`: the LSP's overlay-vs-linker faithfulness test
// (`lsp/overlay.rs`) reaches `build::find_library` directly — the module
// path itself must be nameable from outside `cli`, not just the function.
pub(crate) mod build;
mod completions;
mod driver;
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
tmt — Turing-machine toolchain (TM-1)

USAGE: tmt <SUBCOMMAND> [ARGS]

SUBCOMMANDS:
  compile      .tmc source -> .tmo object (-S for .tma, --emit-ir for world IR JSON)
  asm          .tma assembly -> .tmo object
  link         .tmo objects -> .tmx executable (+ .tmx.map sidecar)
  build        compile+link driver: .tmc/.tma/.tmo inputs or manifest targets
  dis          disassemble a .tmo or .tmx (--listing for the address view)
  run          execute a .tmx on a multi-tape .tmt block
  tape-block   new/set/show .tmt tape-block snapshots
  ir           render --emit-ir JSON (ir graph -> Mermaid)
  lint         hygiene findings over .tmc and .tma sources
  fmt          canonical formatting for .tmc and .tma sources
  lsp          run the LSP server for .tmc and .tma on stdio
  completions  emit a shell completion script (zsh; bash/fish follow-on)

Run `tmt <SUBCOMMAND> --help` for details. `tmt --version` prints the version.
";

pub fn execute(args: &[String]) -> Result<CliOutput, String> {
    execute_with(args, &mut std::io::stderr().lock())
}

/// Writer seam: `--trace` streams into `trace_out` live. The bin path
/// passes stderr; tests pass a `Vec<u8>` and assert on it.
pub fn execute_with(
    args: &[String],
    trace_out: &mut dyn std::io::Write,
) -> Result<CliOutput, String> {
    match args.first().map(String::as_str) {
        None | Some("--help") | Some("-h") => Ok(CliOutput::ok(USAGE.into(), String::new())),
        // Line order mirrors `pmt --version`: tool / language / dialect.
        Some("--version") => Ok(CliOutput::ok(
            format!(
                "tmt {}\ntmc language {}\ntma dialect (tm-1) {}\n",
                env!("CARGO_PKG_VERSION"),
                crate::parser::TMC_LANG_VERSION,
                crate::asm::TM1_TMA_DIALECT_VERSION
            ),
            String::new(),
        )),
        Some("compile") => build::compile(&args[1..]),
        Some("asm") => build::asm(&args[1..]),
        Some("link") => build::link(&args[1..]),
        Some("build") => driver::build(&args[1..]),
        Some("dis") => inspect::dis(&args[1..]),
        Some("tape-block") => inspect::tape_block(&args[1..]),
        Some("ir") => inspect::ir(&args[1..]),
        Some("run") => run::run(&args[1..], trace_out),
        Some("lint") => lint::lint(&args[1..]),
        Some("fmt") => fmt::fmt(&args[1..]),
        Some("lsp") => lsp::lsp(&args[1..]),
        Some("completions") => completions::completions(&args[1..]),
        Some(other) => Err(format!("unknown subcommand `{other}`\n\n{USAGE}")),
    }
}

/// Cell-delimiting policy for [`render_tape`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Delimit {
    /// Dense when every glyph is one character, separated otherwise.
    Auto,
    /// Never separate. Ambiguous with multi-character glyphs, by request.
    Dense,
    /// Always separate.
    Separated,
}

impl Delimit {
    /// Resolve `Auto` against the alphabet actually in play. A single-character
    /// alphabet can never be read two ways, so it stays dense and legible;
    /// anything wider needs the borders to be unambiguous
    /// (docs/tmt/cli.md (tape-block show)).
    fn separates(self, alphabet: &[String]) -> bool {
        match self {
            Self::Dense => false,
            Self::Separated => true,
            Self::Auto => alphabet.iter().any(|g| g.chars().count() != 1),
        }
    }
}

/// Render one tape with its glyphs: the span line plus a caret line under the
/// head. Glyph 0 is blank by convention.
pub(crate) fn render_tape(
    snapshot: &TapeSnapshot,
    alphabet: &[String],
    delimit: Delimit,
) -> String {
    let separated = delimit.separates(alphabet);
    let glyph = |index: u8| -> &str {
        alphabet
            .get(usize::from(index))
            .map(String::as_str)
            .unwrap_or("?")
    };
    let mut cells_line = String::new();
    let mut caret_line = String::new();
    for (i, &cell) in snapshot.cells.iter().enumerate() {
        if separated && i > 0 {
            cells_line.push('|');
            caret_line.push(' ');
        }
        let g = glyph(cell);
        let here = snapshot.origin + i as i64 == snapshot.head;
        cells_line.push_str(g);
        let width = g.chars().count().max(1);
        caret_line.push_str(&if here { "^" } else { " " }.repeat(width));
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
        // cells {1, 0, 1}, head 2, glyphs "0"/"1": a single `|` border at
        // each end and the caret under the last cell (the head).
        let snapshot = TapeSnapshot {
            origin: 0,
            cells: vec![1, 0, 1],
            head: 2,
            alphabet: None,
        };
        let alphabet: Vec<String> = vec!["0".into(), "1".into()];
        let rendered = render_tape(&snapshot, &alphabet, Delimit::Auto);
        assert_eq!(rendered, "origin 0, head 2\n|101|\n   ^\n");
    }

    #[test]
    fn render_tape_stays_dense_for_single_character_alphabets() {
        let snapshot = TapeSnapshot {
            origin: 0,
            cells: vec![1, 0, 1],
            head: 1,
            alphabet: None,
        };
        let alphabet = vec!["_".to_string(), "*".to_string()];
        let text = render_tape(&snapshot, &alphabet, Delimit::Auto);
        assert!(text.contains("|*_*|"), "got:\n{text}");
    }

    #[test]
    fn render_tape_separates_when_a_glyph_is_multi_character() {
        let snapshot = TapeSnapshot {
            origin: 0,
            cells: vec![0, 1, 1],
            head: 0,
            alphabet: None,
        };
        let alphabet = vec!["0".to_string(), "11".to_string()];
        let text = render_tape(&snapshot, &alphabet, Delimit::Auto);
        assert!(text.contains("|0|11|11|"), "got:\n{text}");
    }

    #[test]
    fn render_tape_honours_forced_modes() {
        let snapshot = TapeSnapshot {
            origin: 0,
            cells: vec![0, 1],
            head: 0,
            alphabet: None,
        };
        let single = vec!["a".to_string(), "b".to_string()];
        assert!(render_tape(&snapshot, &single, Delimit::Separated).contains("|a|b|"));

        let multi = vec!["0".to_string(), "11".to_string()];
        assert!(render_tape(&snapshot, &multi, Delimit::Dense).contains("|011|"));
    }

    #[test]
    fn the_caret_tracks_the_head_through_separators() {
        let snapshot = TapeSnapshot {
            origin: 0,
            cells: vec![0, 1],
            head: 1,
            alphabet: None,
        };
        let alphabet = vec!["0".to_string(), "11".to_string()];
        let text = render_tape(&snapshot, &alphabet, Delimit::Auto);
        let mut lines = text.lines().skip(1); // past the "origin …, head …" line
        let cells = lines.next().unwrap();
        let caret = lines.next().unwrap();
        // Carets sit under the head cell's glyph, not under a separator.
        let start = cells.find("11").unwrap();
        assert_eq!(
            caret.trim_end().len(),
            start + 2,
            "cells: {cells}\ncaret: {caret}"
        );
        assert!(caret.trim_start().chars().all(|c| c == '^'));
    }
}
