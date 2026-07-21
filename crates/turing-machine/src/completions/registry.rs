//! The single-source-of-truth CLI-surface registry that drives shell
//! completion (`tmt completions <shell>`). Every field here must trace to
//! a flag or positional the hand-rolled parser (`cli::Args`,
//! `cli/build.rs`, `cli/inspect.rs`, `cli/run.rs`, `cli/lint.rs`,
//! `cli/fmt.rs`) actually accepts — the drift guard in
//! `crates/turing-machine/tests/completions_registry.rs` probes the real
//! parser with every entry so this can't silently rot as subcommands and
//! flags change.
//!
//! The shape mirrors the PM-1 `pmt` registry
//! (`crates/post-machine/src/completions/registry.rs`); the TM-1 surface
//! differs in what it registers, not in how it describes it.

/// One (sub)command's completable surface: its dotted path from the
/// root, its flags, and its positional argument shape.
#[derive(Debug, Clone)]
pub struct CommandSpec {
    /// Dotted path from the root, e.g. `["compile"]` or
    /// `["tape", "show"]`. The empty path is the bare `tmt` invocation.
    pub path: Vec<String>,
    pub flags: Vec<FlagSpec>,
    pub positional: Positional,
}

/// One flag a command accepts, and how a shell should offer it.
#[derive(Debug, Clone)]
pub struct FlagSpec {
    /// The literal token the parser matches (`-o`, `--emit-ir`), or — for
    /// [`FlagKind::SuffixFamily`] — the shared prefix (`--fno-`).
    pub name: String,
    /// A short completion-menu blurb (not a copy of the CLI's own usage
    /// prose — shell-completion descriptions are conventionally terser).
    pub help: String,
    pub kind: FlagKind,
    /// May appear more than once on one command line (`-L`, `-l`,
    /// `--allow`). A `SuffixFamily` is inherently repeatable independent
    /// of this field.
    pub repeatable: bool,
    /// Flags sharing a group are mutually exclusive (`-O0`/`-O1`;
    /// `tape set`'s `-o`/`--in-place`). The parser enforces this only
    /// where it has an explicit runtime check, but a completion script
    /// can still steer the user away from the clash.
    pub exclusive_group: Option<String>,
    /// Set when a flag is meaningful only alongside another. No TM-1
    /// flag needs it today (PM-1's `pmt lint --force` requiring `--fix`
    /// is the shape it exists for, and `tmt lint` has no `--fix`), but
    /// the field is kept so the two registries stay readable side by
    /// side.
    pub requires: Option<String>,
}

impl FlagSpec {
    fn boolean(name: &str, help: &str) -> Self {
        FlagSpec {
            name: name.to_string(),
            help: help.to_string(),
            kind: FlagKind::Boolean,
            repeatable: false,
            exclusive_group: None,
            requires: None,
        }
    }

    fn value(name: &str, help: &str, hint: ValueHint) -> Self {
        FlagSpec {
            name: name.to_string(),
            help: help.to_string(),
            kind: FlagKind::Value(hint),
            repeatable: false,
            exclusive_group: None,
            requires: None,
        }
    }

    fn optional_equals(name: &str, help: &str, hint: ValueHint) -> Self {
        FlagSpec {
            name: name.to_string(),
            help: help.to_string(),
            kind: FlagKind::OptionalEqualsValue(hint),
            repeatable: false,
            exclusive_group: None,
            requires: None,
        }
    }

    fn suffix_family(prefix: &str, help: &str, choices: Vec<String>) -> Self {
        FlagSpec {
            name: prefix.to_string(),
            help: help.to_string(),
            kind: FlagKind::SuffixFamily(choices),
            repeatable: true,
            exclusive_group: None,
            requires: None,
        }
    }

    fn repeatable(mut self) -> Self {
        self.repeatable = true;
        self
    }

    fn exclusive(mut self, group: &str) -> Self {
        self.exclusive_group = Some(group.to_string());
        self
    }
}

#[derive(Debug, Clone)]
pub enum FlagKind {
    /// No value: `-g`, `--trace`.
    Boolean,
    /// `--name value` or `--name=value` — `cli::Args::value` accepts both
    /// forms uniformly for every ordinary value flag.
    Value(ValueHint),
    /// Bare `--name` OR `--name=value` — never `--name value` as two
    /// tokens. `--emit-ir[=STAGE]` is the only current example:
    /// `cli/build.rs::take_emit_ir` checks for the bare flag and for a
    /// `--emit-ir=` prefix, but never calls `Args::value`, so a
    /// space-separated stage is left behind as a stray positional.
    OptionalEqualsValue(ValueHint),
    /// A flag *family* sharing a prefix, one full flag per choice
    /// (`--fno-inline`, `--fno-dce`, …) rather than a `name=value` pair.
    /// `--fno-<pass>` is the only current example.
    SuffixFamily(Vec<String>),
}

#[derive(Debug, Clone)]
pub enum ValueHint {
    /// Free text a shell cannot usefully complete (a numeric budget, a
    /// glyph pattern, a world name, a library name).
    Text,
    /// A fixed, enumerable set of values (`--call-mech`, `--lang`).
    Choices(Vec<String>),
    /// A filesystem path. Empty `extensions` means "any file" — used for
    /// output paths, which name a file that does not exist yet and so
    /// cannot be filtered by what is on disk.
    File(FileHint),
    /// A directory, not a file (`link`'s `-L`).
    Directory,
}

#[derive(Debug, Clone, Default)]
pub struct FileHint {
    /// Glob suffixes without the leading `*.` (`"tmc"`, `"tmx.map"`).
    pub extensions: Vec<String>,
    /// Whether a directory is ALSO a valid completion alongside a
    /// matching file — `tmt lint`/`tmt fmt` positionals and their
    /// `--exclude` set this, since they walk directories too.
    pub dirs: bool,
}

/// One literal completable word plus its completion-menu description
/// (`_describe`-shaped: e.g. a subcommand name and its one-line gloss).
#[derive(Debug, Clone)]
pub struct Choice {
    pub value: String,
    pub help: String,
}

fn choice(value: &str, help: &str) -> Choice {
    Choice {
        value: value.to_string(),
        help: help.to_string(),
    }
}

#[derive(Debug, Clone)]
pub enum Positional {
    None,
    /// Exactly one, of the given shape.
    One(PositionalHint),
    /// One or more of the given shape (`link`'s `INPUT.tmo...`).
    OneOrMore(PositionalHint),
}

#[derive(Debug, Clone)]
pub enum PositionalHint {
    File(FileHint),
    /// A fixed set of literal words with descriptions (the root's
    /// subcommand name, a group's sub-subcommand name, or
    /// `tmt completions <shell>`'s shell name).
    Choices(Vec<Choice>),
    /// Free text with no completion.
    Text,
}

#[derive(Debug, Clone)]
pub struct Registry {
    pub commands: Vec<CommandSpec>,
}

fn ext(extensions: &[&str]) -> FileHint {
    FileHint {
        extensions: extensions.iter().map(|s| s.to_string()).collect(),
        dirs: false,
    }
}

fn any_file() -> FileHint {
    FileHint::default()
}

fn strings(words: &[&str]) -> Vec<String> {
    words.iter().map(|s| s.to_string()).collect()
}

/// The optimizer's own pass-name list, as owned strings. `--fno-<pass>`
/// and `--emit-ir=after:<pass>` both read it rather than a retyped copy —
/// that list is what the drift guard checks these against.
fn pass_names() -> Vec<String> {
    crate::optimizer::pass_names()
        .into_iter()
        .map(|pass| pass.to_string())
        .collect()
}

/// `--emit-ir=STAGE`'s resolvable stages, matching `cli/build.rs`'s own
/// `stage_is_known` predicate: the pipeline bookends plus one
/// `after:<pass>` per registered pass.
fn emit_ir_choices() -> Vec<String> {
    let mut choices = vec!["lowered".to_string(), "final".to_string()];
    choices.extend(pass_names().into_iter().map(|pass| format!("after:{pass}")));
    choices
}

fn compile_spec() -> CommandSpec {
    CommandSpec {
        path: strings(&["compile"]),
        positional: Positional::One(PositionalHint::File(ext(&["tmc"]))),
        flags: vec![
            FlagSpec::boolean("-g", "record debug info (labels + .tmc lines)"),
            FlagSpec::boolean("-O0", "optimization level O0 (default)").exclusive("opt-level"),
            FlagSpec::boolean("-O1", "optimization level O1 (full pass pipeline)")
                .exclusive("opt-level"),
            FlagSpec::boolean("--strip-debugger", "drop `brk` at codegen"),
            FlagSpec::boolean("--debug", "preset: -g -O0"),
            FlagSpec::boolean("--release", "preset: -O1 --strip-debugger"),
            FlagSpec::boolean("-S", "emit the generated .tma instead of an object"),
            FlagSpec::boolean(
                "--stamped-asm",
                "emit raw stamped .tma (skip .rept re-detection)",
            ),
            FlagSpec::optional_equals(
                "--emit-ir",
                "write the world IR JSON next to the output",
                ValueHint::Choices(emit_ir_choices()),
            ),
            FlagSpec::suffix_family(
                "--fno-",
                "disable one optimizer pass (repeatable)",
                pass_names(),
            ),
            // `outline` is the one default-OFF pass, so it appears on this
            // command TWICE with opposite senses, and both spellings are
            // real: `--foutline` turns it on, and `--fno-outline` — which
            // the `--fno-` family renders because `outline` is a
            // registered pass name like any other — keeps it off. The
            // family always mirrors the pass list exactly rather than
            // second-guessing which passes default on.
            FlagSpec::boolean(
                "--foutline",
                "enable the default-off `outline` pass (-O1 only)",
            ),
            FlagSpec::boolean("-Werror", "treat warnings as errors"),
            FlagSpec::boolean("-v", "render the compile report (passes, rounds)"),
            FlagSpec::value("-o", "output path", ValueHint::File(any_file())),
            FlagSpec::boolean("--help", "show subcommand help"),
        ],
    }
}

fn asm_spec() -> CommandSpec {
    CommandSpec {
        path: strings(&["asm"]),
        positional: Positional::One(PositionalHint::File(ext(&["tma"]))),
        flags: vec![
            FlagSpec::boolean("-g", "record the label/line debug section"),
            FlagSpec::value("-o", "output path", ValueHint::File(any_file())),
            FlagSpec::boolean("--help", "show subcommand help"),
        ],
    }
}

fn link_spec() -> CommandSpec {
    CommandSpec {
        path: strings(&["link"]),
        positional: Positional::OneOrMore(PositionalHint::File(ext(&["tmo"]))),
        flags: vec![
            FlagSpec::boolean("--no-relax", "keep every call site in far form"),
            FlagSpec::value(
                "--entry",
                "program entry symbol (default main)",
                ValueHint::Text,
            ),
            // The one TM-1 value flag with a closed value set the parser
            // itself validates (`cli/build.rs::parse_call_mech` rejects
            // anything else), so the completion offers exactly those three
            // words rather than free text.
            FlagSpec::value(
                "--call-mech",
                "bound-call lowering (default hybrid)",
                ValueHint::Choices(strings(&["mono", "frames", "hybrid"])),
            ),
            FlagSpec::boolean("--nostdlib", "do not link the embedded standard library"),
            FlagSpec::value(
                "-L",
                "add a library search directory (repeatable, in order)",
                ValueHint::Directory,
            )
            .repeatable(),
            FlagSpec::value(
                "-l",
                "link NAME.tmo from the search path (repeatable)",
                ValueHint::Text,
            )
            .repeatable(),
            FlagSpec::boolean(
                "-v",
                "render the link report (dropped functions, relaxation)",
            ),
            FlagSpec::value("-o", "output path", ValueHint::File(any_file())),
            FlagSpec::boolean("--help", "show subcommand help"),
        ],
    }
}

/// A `.tmc`/`.tma`-filtered `FileHint` that ALSO accepts directories:
/// `lint` and `fmt` both walk directories recursively for `*.tmc` and
/// `*.tma`.
fn source_or_dir() -> FileHint {
    FileHint {
        extensions: strings(&["tmc", "tma"]),
        dirs: true,
    }
}

fn lint_spec() -> CommandSpec {
    CommandSpec {
        path: strings(&["lint"]),
        positional: Positional::OneOrMore(PositionalHint::File(source_or_dir())),
        flags: vec![
            FlagSpec::value(
                "--exclude",
                "skip a path (repeatable)",
                ValueHint::File(source_or_dir()),
            )
            .repeatable(),
            FlagSpec::value("--allow", "allow a lint code (repeatable)", ValueHint::Text)
                .repeatable(),
            // The `.tmc` family's addition over `pmt lint`: opt-in rules
            // are off unless named here. There is deliberately no
            // `--fix`/`--force` pair — no TM-1 rule emits a
            // machine-applicable fix.
            FlagSpec::value(
                "--warn",
                "enable an opt-in lint code (repeatable)",
                ValueHint::Text,
            )
            .repeatable(),
            FlagSpec::boolean("--no-config", "ignore tmt.json project files"),
            FlagSpec::boolean("--help", "show subcommand help"),
        ],
    }
}

/// Mirrors [`lint_spec`]'s positional shape (`.tmc`/`.tma` files or dirs,
/// `--exclude` repeatable), plus `--check` and `--lang`. The `-` stdin
/// form isn't a registry entry — it's a single bare token, not a
/// completable path shape.
fn fmt_spec() -> CommandSpec {
    CommandSpec {
        path: strings(&["fmt"]),
        positional: Positional::OneOrMore(PositionalHint::File(source_or_dir())),
        flags: vec![
            FlagSpec::value(
                "--exclude",
                "skip a path (repeatable)",
                ValueHint::File(source_or_dir()),
            )
            .repeatable(),
            FlagSpec::boolean(
                "--check",
                "report without writing; exit 1 if any would change",
            ),
            FlagSpec::value(
                "--lang",
                "stdin language (tmc or tma)",
                ValueHint::Choices(strings(&["tmc", "tma"])),
            ),
            FlagSpec::boolean("--help", "show subcommand help"),
        ],
    }
}

fn dis_spec() -> CommandSpec {
    CommandSpec {
        path: strings(&["dis"]),
        positional: Positional::One(PositionalHint::File(ext(&["tmo", "tmx"]))),
        flags: vec![
            FlagSpec::boolean(
                "--listing",
                "print the debugger code view (not reassembleable)",
            ),
            FlagSpec::value(
                "--map",
                "explicit .tmx.map sidecar",
                ValueHint::File(ext(&["tmx.map"])),
            ),
            FlagSpec::boolean("--help", "show subcommand help"),
        ],
    }
}

/// `tape new` takes no positional at all — the template is sized from
/// `--from`. Note the absence of a `--help` flag on every `tape`/`ir`
/// child: those parsers never consume one, so `positionals()` would
/// reject it as an unknown flag (the group's own bare invocation prints
/// the usage instead).
fn tape_new_spec() -> CommandSpec {
    CommandSpec {
        path: strings(&["tape", "new"]),
        positional: Positional::None,
        flags: vec![
            FlagSpec::value(
                "--from",
                "executable to size the blank template to",
                ValueHint::File(ext(&["tmx"])),
            ),
            FlagSpec::value(
                "-o",
                "output path (default blank.tmt)",
                ValueHint::File(ext(&["tmt"])),
            ),
        ],
    }
}

fn tape_set_spec() -> CommandSpec {
    CommandSpec {
        path: strings(&["tape", "set"]),
        positional: Positional::One(PositionalHint::File(ext(&["tmt"]))),
        flags: vec![
            FlagSpec::value(
                "-o",
                "output path (clone target)",
                ValueHint::File(ext(&["tmt"])),
            )
            .exclusive("set-output"),
            FlagSpec::boolean("--in-place", "write back over the input").exclusive("set-output"),
            FlagSpec::value("--tape", "tape index to edit (default 0)", ValueHint::Text),
            FlagSpec::value(
                "--cells",
                "glyph pattern for the tape's cells",
                ValueHint::Text,
            ),
            FlagSpec::value("--origin", "leftmost cell's coordinate", ValueHint::Text),
            FlagSpec::value("--head", "head position", ValueHint::Text),
        ],
    }
}

fn tape_show_spec() -> CommandSpec {
    CommandSpec {
        path: strings(&["tape", "show"]),
        positional: Positional::One(PositionalHint::File(ext(&["tmt"]))),
        flags: vec![],
    }
}

fn run_spec() -> CommandSpec {
    CommandSpec {
        path: strings(&["run"]),
        positional: Positional::One(PositionalHint::File(ext(&["tmx"]))),
        flags: vec![
            // One tape flag, not PM-1's `--tape-block`/`--tape` pair: a
            // TM-1 image runs a whole band, and the band always comes
            // from an MT snapshot (there is no inline glyph-pattern
            // form to be mutually exclusive with).
            FlagSpec::value(
                "--tape",
                "load the initial tape band from an MT snapshot",
                ValueHint::File(ext(&["tmt"])),
            ),
            FlagSpec::value(
                "--max-steps",
                "step budget (default 10000000)",
                ValueHint::Text,
            ),
            FlagSpec::boolean("--no-step-limit", "remove the step budget"),
            FlagSpec::value("--max-tacts", "tact budget", ValueHint::Text),
            FlagSpec::boolean("--trace", "stream per-instruction listing lines to stderr"),
            FlagSpec::boolean("-v", "no extra effect yet (stats always print)"),
            FlagSpec::boolean("--help", "show subcommand help"),
        ],
    }
}

fn ir_graph_spec() -> CommandSpec {
    CommandSpec {
        path: strings(&["ir", "graph"]),
        positional: Positional::One(PositionalHint::File(ext(&["ir.json"]))),
        flags: vec![FlagSpec::value(
            "--function",
            "restrict output to one world",
            ValueHint::Text,
        )],
    }
}

fn lsp_spec() -> CommandSpec {
    CommandSpec {
        path: strings(&["lsp"]),
        positional: Positional::None,
        flags: vec![FlagSpec::boolean("--help", "show subcommand help")],
    }
}

fn completions_spec() -> CommandSpec {
    CommandSpec {
        path: strings(&["completions"]),
        positional: Positional::One(PositionalHint::Choices(vec![
            choice("zsh", "Z shell completion script"),
            choice("bash", "Bash completion script (not yet implemented)"),
            choice("fish", "fish completion script (not yet implemented)"),
        ])),
        flags: vec![FlagSpec::boolean("--help", "show subcommand help")],
    }
}

/// Short one-line glosses for the top-level subcommand list, reusing the
/// wording from the CLI's own top-level `--help` text (`cli/mod.rs`'s
/// `USAGE` constant) rather than re-authoring a third copy.
fn top_level_help(name: &str) -> &'static str {
    match name {
        "compile" => ".tmc source -> .tmo object (-S for .tma, --emit-ir for world IR JSON)",
        "asm" => ".tma assembly -> .tmo object",
        "link" => ".tmo objects -> .tmx executable (+ .tmx.map sidecar)",
        "dis" => "disassemble a .tmo or .tmx (--listing for the address view)",
        "run" => "execute a .tmx on a multi-tape .tmt block",
        "tape" => "new/set/show .tmt tape-block snapshots",
        "ir" => "render --emit-ir JSON (ir graph -> Mermaid)",
        "lint" => "hygiene findings over .tmc and .tma sources",
        "fmt" => "canonical formatting for .tmc and .tma sources",
        "lsp" => "run the LSP server on stdio",
        "completions" => "emit a shell completion script (zsh; bash/fish follow-on)",
        _ => "",
    }
}

fn group_child_help(path: &[String]) -> &'static str {
    match (
        path.first().map(String::as_str),
        path.get(1).map(String::as_str),
    ) {
        (Some("tape"), Some("new")) => "write a blank .tmt template sized to an executable",
        (Some("tape"), Some("set")) => "clone a .tmt tape-block snapshot with edits",
        (Some("tape"), Some("show")) => "render a .tmt tape-block snapshot",
        (Some("ir"), Some("graph")) => "render --emit-ir JSON as a Mermaid flowchart",
        _ => "",
    }
}

/// The root `tmt` invocation: global flags plus the subcommand-name
/// positional, the latter derived from the other entries (not
/// hand-duplicated) so it can't drift from them on its own.
fn root_spec(commands: &[CommandSpec]) -> CommandSpec {
    let mut top_level_names = Vec::new();
    for command in commands {
        if let Some(first) = command.path.first()
            && !top_level_names.contains(first)
        {
            top_level_names.push(first.clone());
        }
    }
    let choices = top_level_names
        .iter()
        .map(|name| choice(name, top_level_help(name)))
        .collect();
    CommandSpec {
        path: Vec::new(),
        positional: Positional::One(PositionalHint::Choices(choices)),
        flags: vec![
            FlagSpec::boolean("--help", "show top-level help"),
            FlagSpec::boolean("-h", "show top-level help"),
            FlagSpec::boolean("--version", "print the tmt version"),
        ],
    }
}

/// The registry describing the real, currently-dispatched `tmt` surface:
/// ten top-level subcommands (`compile`/`asm`/`link`/`dis`/`run`/`tape`/
/// `ir`/`lint`/`fmt`/`lsp`, `tape` and `ir` nested) plus `completions`
/// itself. Absent, permanently: `tape build`, which is PM-1-only
/// glyph-pattern sugar (`cli/inspect.rs` says why TM-1 has no analogue).
pub fn registry() -> Registry {
    let commands = vec![
        compile_spec(),
        asm_spec(),
        link_spec(),
        dis_spec(),
        run_spec(),
        tape_new_spec(),
        tape_set_spec(),
        tape_show_spec(),
        ir_graph_spec(),
        lint_spec(),
        fmt_spec(),
        lsp_spec(),
        completions_spec(),
    ];
    let root = root_spec(&commands);

    let mut all = vec![root];
    all.extend(commands);
    Registry { commands: all }
}

/// For a group's sub-subcommand choice list (`tape` -> new/set/show,
/// `ir` -> graph): every `CommandSpec` whose path is `[group, _]`.
pub fn group_children<'a>(registry: &'a Registry, group: &str) -> Vec<&'a CommandSpec> {
    registry
        .commands
        .iter()
        .filter(|c| c.path.len() == 2 && c.path[0] == group)
        .collect()
}

/// A group child's one-line gloss, for the group's own `_describe` list.
pub fn child_help(command: &CommandSpec) -> &'static str {
    group_child_help(&command.path)
}

/// Every literal token the parser will recognize for this flag: used by
/// completion renderers to emit literal candidates, and by the drift
/// guard to probe the real parser with each one.
pub fn expand(flag: &FlagSpec) -> Vec<String> {
    match &flag.kind {
        FlagKind::Boolean | FlagKind::Value(_) | FlagKind::OptionalEqualsValue(_) => {
            vec![flag.name.clone()]
        }
        FlagKind::SuffixFamily(choices) => choices
            .iter()
            .map(|choice| format!("{}{choice}", flag.name))
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn find(reg: &Registry, path: &[&str]) -> CommandSpec {
        let target: Vec<String> = path.iter().map(|s| s.to_string()).collect();
        reg.commands
            .iter()
            .find(|c| c.path == target)
            .unwrap_or_else(|| panic!("no registry entry for {path:?}"))
            .clone()
    }

    #[test]
    fn root_subcommand_choices_are_derived_not_duplicated() {
        let registry = registry();
        let root = registry
            .commands
            .iter()
            .find(|c| c.path.is_empty())
            .expect("root entry");
        let Positional::One(PositionalHint::Choices(names)) = &root.positional else {
            panic!("root positional should be a fixed choice list");
        };
        let names: Vec<&str> = names.iter().map(|c| c.value.as_str()).collect();
        assert_eq!(
            names,
            vec![
                "compile",
                "asm",
                "link",
                "dis",
                "run",
                "tape",
                "ir",
                "lint",
                "fmt",
                "lsp",
                "completions"
            ]
        );
    }

    #[test]
    fn expand_suffix_family_produces_one_flag_per_choice() {
        let flag = FlagSpec::suffix_family(
            "--fno-",
            "disable one optimizer pass (repeatable)",
            vec!["inline".into(), "dce".into()],
        );
        assert_eq!(expand(&flag), vec!["--fno-inline", "--fno-dce"]);
    }

    /// `outline` is default-OFF, and both of its spellings are real
    /// flags with opposite senses: the `--fno-` family carries it
    /// (because it is a registered pass), and `--foutline` is a separate
    /// boolean that turns it on. Neither may swallow the other.
    #[test]
    fn outline_appears_both_as_a_fno_choice_and_as_its_own_enable_flag() {
        let reg = registry();
        let compile = find(&reg, &["compile"]);

        let fno = compile
            .flags
            .iter()
            .find(|f| f.name == "--fno-")
            .expect("--fno- family");
        assert!(
            expand(fno).contains(&"--fno-outline".to_string()),
            "the --fno- family mirrors pass_names(), which includes outline"
        );

        let foutline = compile
            .flags
            .iter()
            .find(|f| f.name == "--foutline")
            .expect("--foutline should be its own boolean flag");
        assert!(matches!(foutline.kind, FlagKind::Boolean));
    }

    /// `link`'s `--call-mech` is the one TM-1 value flag whose value set
    /// the parser itself closes over (`parse_call_mech`), so it must be
    /// registered as `Choices`, not free text.
    #[test]
    fn call_mech_registers_its_closed_value_set() {
        let reg = registry();
        let link = find(&reg, &["link"]);
        let call_mech = link
            .flags
            .iter()
            .find(|f| f.name == "--call-mech")
            .expect("link should register --call-mech");
        let FlagKind::Value(ValueHint::Choices(choices)) = &call_mech.kind else {
            panic!("--call-mech should be a Value(Choices(..))");
        };
        assert_eq!(choices, &strings(&["mono", "frames", "hybrid"]));
    }

    /// `lint`'s harder cases: a repeatable positional that also accepts
    /// directories, repeatable value-taking flags, and the `--warn`
    /// opt-in switch that PM-1's `pmt lint` has no analogue for.
    #[test]
    fn lint_positional_and_flags_carry_dirs_and_repeatable() {
        let reg = registry();
        let lint = find(&reg, &["lint"]);

        let Positional::OneOrMore(PositionalHint::File(hint)) = &lint.positional else {
            panic!("lint positional should be one-or-more files/dirs");
        };
        assert!(hint.dirs, "lint positionals also accept directories");

        for name in ["--exclude", "--allow", "--warn"] {
            let flag = lint
                .flags
                .iter()
                .find(|f| f.name == name)
                .unwrap_or_else(|| panic!("lint should register {name}"));
            assert!(flag.repeatable, "{name} is repeatable");
        }
        assert!(
            lint.flags.iter().all(|f| f.name != "--fix"),
            "tmt lint has no --fix (no TM-1 rule emits a fix)"
        );
    }
}
