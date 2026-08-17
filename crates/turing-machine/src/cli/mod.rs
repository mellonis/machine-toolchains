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
mod dap;
// `pub(crate)`: `crate::dap::TmDapAdapter`'s target-mode `launch` calls
// `driver::build_target_for_launch` directly — the DAP seam this crate's
// `dap` module carves the manifest build path down to (docs/tmt/cli.md
// (build)), not a duplicate of it.
pub(crate) mod driver;
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
  ir           render --emit-ir JSON (ir graph -> Mermaid, ir footprints -> write sets)
  lint         hygiene findings over .tmc and .tma sources
  fmt          canonical formatting for .tmc and .tma sources
  lsp          run the LSP server for .tmc and .tma on stdio
  dap          run the DAP debug-adapter server on stdio
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
        Some("dap") => dap::dap(&args[1..]),
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

/// Render one tape with its glyphs: the head line plus the span. Glyph 0 is
/// blank by convention.
///
/// The head line names the glyph under the head rather than marking it with a
/// caret line beneath the span. A caret has to be padded from column zero out
/// to the head, so a head resting far from the origin costs a line as long as
/// the span itself — on a megacell tape that one line doubles the output and
/// carries a single character of information
/// (docs/tmt/cli.md (tape-block show)).
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
    for (i, &cell) in snapshot.cells.iter().enumerate() {
        if separated && i > 0 {
            cells_line.push('|');
        }
        cells_line.push_str(glyph(cell));
    }
    // The span is a window, not the whole tape: a head outside it rests on
    // blank, which is glyph 0 by convention.
    let under_head = usize::try_from(snapshot.head - snapshot.origin)
        .ok()
        .and_then(|i| snapshot.cells.get(i).copied())
        .unwrap_or(0);
    format!(
        "origin {}, head {} reads '{}'\n|{}|\n",
        snapshot.origin,
        snapshot.head,
        glyph(under_head),
        cells_line,
    )
}

/// Split repeatable `KEY=VALUE` edit flags into pairs, preserving order.
/// The key is everything before the FIRST `=`, so a value may contain `=`.
/// A key repeated within one flag is an error rather than last-wins: silently
/// dropping an edit the author wrote is worse than making them look
/// (docs/tmt/cli.md (tape-block edit flags)).
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

    /// Consume `--help`, then `-h` — `-h` is a strict alias of `--help`
    /// everywhere the latter is recognized (top level, subcommands, nested
    /// actions): both are consumed unconditionally rather than short-
    /// circuited, so an invocation that (redundantly) writes both still
    /// leaves neither token to trip `positionals`' unknown-flag check.
    pub(crate) fn help(&mut self) -> bool {
        let long = self.flag("--help");
        let short = self.flag("-h");
        long || short
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
    fn render_tape_draws_a_single_bordered_span_and_names_the_head_glyph() {
        // cells {1, 0, 1}, head 2, glyphs "0"/"1": a single `|` border at
        // each end, and the head's glyph named on the head line.
        let snapshot = TapeSnapshot {
            origin: 0,
            cells: vec![1, 0, 1],
            head: 2,
            alphabet: None,
        };
        let alphabet: Vec<String> = vec!["0".into(), "1".into()];
        let rendered = render_tape(&snapshot, &alphabet, Delimit::Auto);
        assert_eq!(rendered, "origin 0, head 2 reads '1'\n|101|\n");
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
    fn the_head_glyph_is_named_whole_even_when_multi_character() {
        // The head sits on a two-character glyph reached past a separator:
        // the whole glyph is named, not a slice of it.
        let snapshot = TapeSnapshot {
            origin: 0,
            cells: vec![0, 1],
            head: 1,
            alphabet: None,
        };
        let alphabet = vec!["0".to_string(), "11".to_string()];
        let text = render_tape(&snapshot, &alphabet, Delimit::Auto);
        assert_eq!(text, "origin 0, head 1 reads '11'\n|0|11|\n");
    }

    #[test]
    fn the_head_glyph_is_read_through_a_non_zero_origin() {
        // head 4 on a span starting at 3 is the SECOND cell, not the fifth:
        // the glyph is indexed from the origin, not from zero.
        let snapshot = TapeSnapshot {
            origin: 3,
            cells: vec![0, 1, 0],
            head: 4,
            alphabet: None,
        };
        let alphabet = vec!["_".to_string(), "*".to_string()];
        let text = render_tape(&snapshot, &alphabet, Delimit::Auto);
        assert_eq!(text, "origin 3, head 4 reads '*'\n|_*_|\n");
    }

    #[test]
    fn a_head_outside_the_span_reads_blank() {
        // The span is a window on an unbounded tape; off either end the tape
        // is blank, which is glyph 0. Below the origin exercises the negative
        // offset, past the end exercises the length bound.
        let alphabet = vec!["_".to_string(), "*".to_string()];
        for head in [-7, 99] {
            let snapshot = TapeSnapshot {
                origin: 0,
                cells: vec![1, 1],
                head,
                alphabet: None,
            };
            let text = render_tape(&snapshot, &alphabet, Delimit::Auto);
            assert_eq!(text, format!("origin 0, head {head} reads '_'\n|**|\n"));
        }
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
