//! The `pmt` command-line tool: a thin renderer over the library API.
//! Libraries never print; every byte of terminal output originates here.

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
pmt — Post-machine toolchain

USAGE: pmt <SUBCOMMAND> [ARGS]

SUBCOMMANDS:
  compile      .pmc source -> .pmo object (-S for .pma, --emit-ir for CFG JSON)
  asm          .pma assembly -> .pmo object
  link         .pmo objects -> .pmx executable (+ .pmx.map sidecar)
  build        compile+link driver: .pmc/.pma/.pmo inputs or manifest targets
  lint         lint .pmc/.pma sources (hygiene findings; docs/pmt/lint.md)
  fmt          format .pmc/.pma sources in place (--check to preview; -)
  dis          disassemble a .pmo or .pmx (--listing for the address view)
  run          execute a .pmx on a tape
  tape-block   build/new/set/show .pmt tape-block snapshots
  ir           render --emit-ir JSON (ir graph -> Mermaid)
  lsp          run the LSP server on stdio
  completions  emit a shell completion script (zsh; bash/fish follow-on)

Run `pmt <SUBCOMMAND> --help` for details. `pmt --version` prints the version.
";

pub fn execute(args: &[String]) -> Result<CliOutput, String> {
    execute_with(args, &mut std::io::stderr().lock())
}

/// Writer seam: `--trace` (`docs/pmt/cli.md`) streams into `trace_out` live.
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
        Some("build") => driver::build(&args[1..]),
        Some("lint") => lint::lint(&args[1..]),
        Some("fmt") => fmt::fmt(&args[1..]),
        Some("dis") => inspect::dis(&args[1..]),
        Some("tape-block") => inspect::tape_block(&args[1..]),
        Some("ir") => inspect::ir(&args[1..]),
        Some("run") => run::run(&args[1..], trace_out),
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
    /// Resolve `Auto` against the alphabet actually in play. PM-1's fixed pair
    /// is single-character, so a PM tape can never be read two ways and stays
    /// dense; a foreign block with wider glyphs gets borders
    /// (docs/pmt/cli.md (tape-block show)).
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

/// Split repeatable `KEY=VALUE` edit flags into pairs, preserving order.
/// The key is everything before the FIRST `=`, so a value may contain `=`.
/// A key repeated within one flag is an error rather than last-wins: silently
/// dropping an edit the author wrote is worse than making them look
/// (docs/pmt/cli.md (tape-block edit flags)).
pub(crate) fn parse_keyed(flag: &str, values: &[String]) -> Result<Vec<(String, String)>, String> {
    let mut out: Vec<(String, String)> = Vec::new();
    for raw in values {
        let Some((key, value)) = raw.split_once('=') else {
            return Err(format!("{flag} `{raw}`: expected KEY=VALUE"));
        };
        if key.is_empty() {
            return Err(format!("{flag} `{raw}`: empty tape key"));
        }
        if out.iter().any(|(k, _)| k == key) {
            return Err(format!("{flag}: tape `{key}` given twice"));
        }
        out.push((key.to_string(), value.to_string()));
    }
    Ok(out)
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
            alphabet: None,
        };
        let alphabet: Vec<String> = vec![" ".into(), "*".into()];
        let rendered = render_tape(&snapshot, &alphabet, Delimit::Auto);
        assert_eq!(rendered, "origin 0, head 2\n|* *|\n   ^\n");
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

    fn owned(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn parse_keyed_splits_at_the_first_equals() {
        let got = parse_keyed("--cells", &owned(&["0='a','b'", "main='c'"])).unwrap();
        assert_eq!(
            got,
            vec![
                ("0".to_string(), "'a','b'".to_string()),
                ("main".to_string(), "'c'".to_string()),
            ]
        );
    }

    #[test]
    fn parse_keyed_allows_equals_inside_the_value() {
        let got = parse_keyed("--cells", &owned(&["0='='"])).unwrap();
        assert_eq!(got, vec![("0".to_string(), "'='".to_string())]);
    }

    #[test]
    fn parse_keyed_allows_an_empty_value() {
        let got = parse_keyed("--cells", &owned(&["1="])).unwrap();
        assert_eq!(got, vec![("1".to_string(), String::new())]);
    }

    #[test]
    fn parse_keyed_rejects_a_missing_equals() {
        let err = parse_keyed("--cells", &owned(&["0"])).unwrap_err();
        assert!(err.contains("--cells"), "got: {err}");
        assert!(err.contains("KEY="), "got: {err}");
    }

    #[test]
    fn parse_keyed_rejects_an_empty_key() {
        assert!(parse_keyed("--cells", &owned(&["='a'"])).is_err());
    }

    #[test]
    fn parse_keyed_rejects_a_repeated_key() {
        let err = parse_keyed("--cells", &owned(&["0='a'", "0='b'"])).unwrap_err();
        assert!(err.contains("twice"), "got: {err}");
    }
}
