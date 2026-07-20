//! The single-source-of-truth CLI-surface registry that drives shell
//! completion (`pmt completions <shell>`, docs/pmt/cli.md (pmt completions)).
//! Every field here must trace to a flag or positional the hand-rolled
//! parser (`cli::Args`, `cli/build.rs`, `cli/inspect.rs`, `cli/run.rs`)
//! actually accepts — the drift guard in
//! `crates/post-machine/tests/completions_registry.rs` probes the real
//! parser with every entry so this can't silently rot as subcommands and
//! flags change.

/// One (sub)command's completable surface: its dotted path from the
/// root, its flags, and its positional argument shape.
#[derive(Debug, Clone)]
pub struct CommandSpec {
    /// Dotted path from the root, e.g. `["compile"]` or
    /// `["tape", "build"]`. The empty path is the bare `pmt` invocation.
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
    /// A short completion-menu blurb (not a copy of docs/pmt/cli.md's prose —
    /// shell-completion descriptions are conventionally terser).
    pub help: String,
    pub kind: FlagKind,
    /// May appear more than once on one command line (`-L`, `-l`). A
    /// `SuffixFamily` is inherently repeatable independent of this field.
    pub repeatable: bool,
    /// Flags sharing a group are mutually exclusive (`-O0`/`-O1`;
    /// `--tape-block`/`--tape`). The parser itself does not enforce this
    /// (whichever is scanned last wins, or there's an explicit runtime
    /// check — docs/pmt/cli.md) but a completion script can still steer the
    /// user away from the clash.
    pub exclusive_group: Option<String>,
    /// Set when a flag is meaningful only alongside another — e.g.
    /// `pmt lint --fix --force`, where `--force` requires `--fix`, and
    /// `pmt run --head` requiring `--tape`.
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

    fn requires(mut self, flag: &str) -> Self {
        self.requires = Some(flag.to_string());
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
    /// glyph pattern, a `move,read,write` triple, a library name).
    Text,
    /// A fixed, enumerable set of values.
    Choices(Vec<String>),
    /// A filesystem path. Empty `extensions` means "any file" — used for
    /// output paths (the design doc explains the read-vs-write split).
    File(FileHint),
    /// A directory, not a file (`link`'s `-L`).
    Directory,
}

#[derive(Debug, Clone, Default)]
pub struct FileHint {
    /// Glob suffixes without the leading `*.` (`"pmc"`, `"pmx.map"`).
    pub extensions: Vec<String>,
    /// Whether a directory is ALSO a valid completion alongside a
    /// matching file — `pmt lint PATH...`/`--exclude` set this since
    /// they also accept directories.
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
    /// One or more of the given shape (`link`'s `INPUT.pmo...`).
    OneOrMore(PositionalHint),
}

#[derive(Debug, Clone)]
pub enum PositionalHint {
    File(FileHint),
    /// A fixed set of literal words with descriptions (the root's
    /// subcommand name, a group's sub-subcommand name, or
    /// `pmt completions <shell>`'s shell name).
    Choices(Vec<Choice>),
    /// Free text with no completion (`tape build`'s glyph pattern).
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

/// `--fno-<pass>` and `--emit-ir=after:<pass>` both read the optimizer's
/// own pass-name list (docs/pmt/language.md (optimization)) rather than a
/// retyped copy — that list is what the drift guard checks this against.
fn emit_ir_choices() -> Vec<String> {
    let mut choices = vec!["lowered".to_string(), "final".to_string()];
    choices.extend(
        crate::optimizer::pass_names()
            .into_iter()
            .map(|pass| format!("after:{pass}")),
    );
    choices
}

fn compile_spec() -> CommandSpec {
    CommandSpec {
        path: strings(&["compile"]),
        positional: Positional::One(PositionalHint::File(ext(&["pmc"]))),
        flags: vec![
            FlagSpec::boolean("-g", "record debug info (labels + .pmc lines)"),
            FlagSpec::boolean("-O0", "optimization level O0 (default)").exclusive("opt-level"),
            FlagSpec::boolean("-O1", "optimization level O1 (full pass pipeline)")
                .exclusive("opt-level"),
            FlagSpec::boolean("--strip-debugger", "drop `brk` at codegen"),
            FlagSpec::boolean("--debug", "preset: -g -O0"),
            FlagSpec::boolean("--release", "preset: -O1 --strip-debugger"),
            FlagSpec::boolean("-S", "emit the generated .pma instead of an object"),
            FlagSpec::optional_equals(
                "--emit-ir",
                "write the CFG IR JSON next to the output",
                ValueHint::Choices(emit_ir_choices()),
            ),
            FlagSpec::suffix_family(
                "--fno-",
                "disable one optimizer pass (repeatable)",
                crate::optimizer::pass_names()
                    .iter()
                    .map(|p| p.to_string())
                    .collect(),
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
        positional: Positional::One(PositionalHint::File(ext(&["pma"]))),
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
        positional: Positional::OneOrMore(PositionalHint::File(ext(&["pmo"]))),
        flags: vec![
            FlagSpec::boolean("--no-relax", "keep every symbol site in far form"),
            FlagSpec::boolean("--nostdlib", "do not link the built-in std"),
            FlagSpec::value(
                "-L",
                "add a library search directory (repeatable, in order)",
                ValueHint::Directory,
            )
            .repeatable(),
            FlagSpec::value(
                "-l",
                "link NAME.pmo from the search path (repeatable)",
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

/// A `.pmc`/`.pma`-filtered `FileHint` that ALSO accepts directories
/// (`lint` and `fmt` both walk directories recursively for `*.pmc` and
/// `*.pma`, docs/pmt/lint.md / docs/pmt/cli.md).
fn source_or_dir() -> FileHint {
    FileHint {
        extensions: strings(&["pmc", "pma"]),
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
            FlagSpec::boolean("--fix", "apply fixes"),
            FlagSpec::boolean("--force", "overwrite without confirmation").requires("--fix"),
            FlagSpec::boolean("--no-config", "ignore pmt.json project files"),
            FlagSpec::boolean("--help", "show subcommand help"),
        ],
    }
}

/// Mirrors [`lint_spec`]: same positional shape (`.pmc`/`.pma` files or
/// dirs, `--exclude` repeatable), plus `--check` and `--lang` (docs/pmt/cli.md
/// (pmt fmt)). The `-` stdin form isn't a registry entry — it's a single
/// bare token, not a completable path shape.
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
                "stdin language (pmc or pma)",
                ValueHint::Choices(strings(&["pmc", "pma"])),
            ),
            FlagSpec::boolean("--help", "show subcommand help"),
        ],
    }
}

fn dis_spec() -> CommandSpec {
    CommandSpec {
        path: strings(&["dis"]),
        positional: Positional::One(PositionalHint::File(ext(&["pmo", "pmx"]))),
        flags: vec![
            FlagSpec::boolean(
                "--listing",
                "print the debugger code view (not reassembleable)",
            ),
            FlagSpec::value(
                "--map",
                "explicit .pmx.map sidecar",
                ValueHint::File(ext(&["pmx.map"])),
            ),
            FlagSpec::boolean("--help", "show subcommand help"),
        ],
    }
}

fn tape_build_spec() -> CommandSpec {
    CommandSpec {
        path: strings(&["tape", "build"]),
        positional: Positional::One(PositionalHint::Text),
        flags: vec![
            FlagSpec::value(
                "--head",
                "initial head position (default 0)",
                ValueHint::Text,
            ),
            FlagSpec::value(
                "-o",
                "output path (default tape.pmt)",
                ValueHint::File(any_file()),
            ),
        ],
    }
}

fn tape_new_spec() -> CommandSpec {
    CommandSpec {
        path: strings(&["tape", "new"]),
        positional: Positional::None,
        flags: vec![
            FlagSpec::value(
                "--from",
                "executable to size the blank template to",
                ValueHint::File(ext(&["pmx"])),
            ),
            FlagSpec::value(
                "-o",
                "output path (default blank.pmt)",
                ValueHint::File(ext(&["pmt"])),
            ),
        ],
    }
}

fn tape_set_spec() -> CommandSpec {
    CommandSpec {
        path: strings(&["tape", "set"]),
        positional: Positional::One(PositionalHint::File(ext(&["pmt"]))),
        flags: vec![
            FlagSpec::value(
                "-o",
                "output path (clone target)",
                ValueHint::File(ext(&["pmt"])),
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
        positional: Positional::One(PositionalHint::File(ext(&["pmt"]))),
        flags: vec![],
    }
}

fn run_spec() -> CommandSpec {
    CommandSpec {
        path: strings(&["run"]),
        positional: Positional::One(PositionalHint::File(ext(&["pmx"]))),
        flags: vec![
            FlagSpec::value(
                "--tape-block",
                "load the initial tape from a snapshot",
                ValueHint::File(ext(&["pmt"])),
            )
            .exclusive("tape-source"),
            FlagSpec::value("--tape", "build the initial tape inline", ValueHint::Text)
                .exclusive("tape-source"),
            FlagSpec::value(
                "--head",
                "head position for --tape (default 0)",
                ValueHint::Text,
            )
            .requires("--tape"),
            FlagSpec::value(
                "--save-tape-block",
                "write the final tape as a snapshot",
                ValueHint::File(any_file()),
            ),
            FlagSpec::value(
                "--max-steps",
                "step budget (default 10000000)",
                ValueHint::Text,
            ),
            FlagSpec::boolean("--no-step-limit", "remove the step budget"),
            FlagSpec::value("--max-tacts", "tact budget", ValueHint::Text),
            FlagSpec::boolean("--strict-cells", "trap on double-mark/double-unmark"),
            FlagSpec::value(
                "--tact-profile",
                "device costs move,read,write (default 1,1,1)",
                ValueHint::Text,
            ),
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
            "restrict output to one function",
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
/// `USAGE` constant / docs/pmt/cli.md's opening block) rather than
/// re-authoring a third copy.
fn top_level_help(name: &str) -> &'static str {
    match name {
        "compile" => ".pmc source -> .pmo object (-S for .pma, --emit-ir for CFG JSON)",
        "asm" => ".pma assembly -> .pmo object",
        "link" => ".pmo objects -> .pmx executable (+ .pmx.map sidecar)",
        "lint" => "lint .pmc/.pma sources (hygiene findings; docs/pmt/lint.md)",
        "fmt" => "format .pmc/.pma sources in place (--check to preview; -)",
        "dis" => "disassemble a .pmo or .pmx (--listing for the address view)",
        "run" => "execute a .pmx on a tape",
        "tape" => "build/new/set/show .pmt tape-block snapshots",
        "ir" => "render --emit-ir JSON (ir graph -> Mermaid)",
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
        (Some("tape"), Some("build")) => "write a .pmt tape-block snapshot from a glyph pattern",
        (Some("tape"), Some("new")) => "write a blank .pmt template sized to an executable",
        (Some("tape"), Some("set")) => "clone a .pmt tape-block snapshot with edits",
        (Some("tape"), Some("show")) => "render a .pmt tape-block snapshot",
        (Some("ir"), Some("graph")) => "render --emit-ir JSON as a Mermaid flowchart",
        _ => "",
    }
}

/// The root `pmt` invocation: global flags plus the subcommand-name
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
            FlagSpec::boolean("--version", "print the pmt version"),
        ],
    }
}

/// The registry describing master's real, currently-dispatched CLI
/// surface: 11 top-level subcommands (`compile`/`asm`/`link`/`lint`/
/// `fmt`/`dis`/`tape`/`run`/`ir`/`lsp`, `tape` and `ir` nested) plus
/// `completions` itself. `build` (issue-tracked) is deliberately absent
/// — see the design doc for the entry it'll need.
pub fn registry() -> Registry {
    let commands = vec![
        compile_spec(),
        asm_spec(),
        link_spec(),
        lint_spec(),
        fmt_spec(),
        dis_spec(),
        tape_build_spec(),
        tape_new_spec(),
        tape_set_spec(),
        tape_show_spec(),
        run_spec(),
        ir_graph_spec(),
        lsp_spec(),
        completions_spec(),
    ];
    let root = root_spec(&commands);

    let mut all = vec![root];
    all.extend(commands);
    Registry { commands: all }
}

/// For a group's sub-subcommand choice list (`tape` -> build/show,
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
                "lint",
                "fmt",
                "dis",
                "tape",
                "run",
                "ir",
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

    /// `lint`'s harder cases: a repeatable positional that also accepts
    /// directories, a repeatable value-taking `--exclude`, and `--force`
    /// gated on `--fix` — the shape the design doc's sketch proved ahead
    /// of `lint` landing as an active entry (see git history), now
    /// checked directly against the real registered `CommandSpec`.
    #[test]
    fn lint_positional_and_flags_carry_dirs_repeatable_and_requires() {
        let reg = registry();
        let lint = reg
            .commands
            .iter()
            .find(|c| c.path == vec!["lint".to_string()])
            .expect("lint should be registered");

        let Positional::OneOrMore(PositionalHint::File(hint)) = &lint.positional else {
            panic!("lint positional should be one-or-more files/dirs");
        };
        assert!(hint.dirs, "lint positionals also accept directories");

        let exclude = lint.flags.iter().find(|f| f.name == "--exclude").unwrap();
        assert!(exclude.repeatable, "--exclude is repeatable");
        let allow = lint.flags.iter().find(|f| f.name == "--allow").unwrap();
        assert!(allow.repeatable, "--allow is repeatable");
        let force = lint.flags.iter().find(|f| f.name == "--force").unwrap();
        assert_eq!(force.requires.as_deref(), Some("--fix"));
    }
}
