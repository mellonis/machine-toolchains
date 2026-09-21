//! `tmt interface`: the fifteenth subcommand, a thin renderer over
//! `crate::header` (docs/tmt/cli.md (interface)). Mirrors `dis`'s shape in
//! `cli/inspect.rs` — sniff the input, dispatch on the container kind,
//! print the result.

use std::fs;
use std::path::Path;

use mtc_core::formats::object::ObjectFile;
use mtc_core::formats::{ARCH_TM1, ContainerKind, sniff};
use mtc_core::vm::LoadError;

use super::{Args, CliOutput};

pub(super) const INTERFACE_USAGE: &str = "\
USAGE: tmt interface INPUT [-o OUT.tmh] [FLAGS]

INPUT is told apart by its container magic, never by its extension: a
.tmc source or a compiled .tmo object. A .tmh extension (case-insensitive)
additionally selects declarations-only reading of a text INPUT: a
`machine` block or a routine body is rejected, and a bodiless routine
signature is required instead. Prints the unit's exported
declarations — alphabets and routine signatures with their EFFECTIVE
write contracts either way; neither arm ever prints `volatile` (it
leaves no trace past source and is never checked at a call site). From
source the header is complete: it also carries exported graph bodies in
full and every `?` doc line. From an object it carries signatures and
alphabets only — no graph body, no map, no doc line, since none of those
exist on the wire. Without -o the header goes to stdout.

FLAGS (text INPUT only — rejected on a .tmo object, which carries no
external references of its own left to resolve):
  --extern FILE      read FILE's declarations (.tmh strict, .tmc lenient;
                     repeatable, in command-line order)
  --nostdlib         do not read the embedded standard library's declarations
";

pub(super) fn interface(raw: &[String]) -> Result<CliOutput, String> {
    let mut args = Args::new(raw);
    if args.help() {
        return Ok(CliOutput::ok(INTERFACE_USAGE.into(), String::new()));
    }
    let explicit_out = args.value("-o")?;
    // Read BEFORE the input, exactly as `tmt compile` does (`cli/build.rs`
    // (compile)): a broken `--extern` file's error then names ITS OWN
    // path, never the primary input's.
    let extern_paths = args.values("--extern")?;
    let nostdlib = args.flag("--nostdlib");
    let inputs = args.positionals()?;
    let [input] = inputs.as_slice() else {
        return Err(format!(
            "interface takes exactly one input\n\n{INTERFACE_USAGE}"
        ));
    };
    let path = Path::new(input);
    let bytes = fs::read(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;

    let text = match sniff(&bytes) {
        Some(ContainerKind::Object) => {
            // An object's own header is read straight off its interface
            // section — nothing left to resolve against another unit's
            // declarations, so `--extern`/`--nostdlib` are a usage error
            // here rather than silently ignored (docs/tmt/cli.md
            // (interface)).
            if !extern_paths.is_empty() || nostdlib {
                return Err(format!(
                    "{}: --extern/--nostdlib apply only to a text INPUT — a .tmo object's \
                     header is read straight off its own interface section, with no \
                     declarations left to resolve",
                    path.display()
                ));
            }
            let obj = ObjectFile::from_bytes(&bytes).map_err(|e| e.to_string())?;
            if obj.arch != ARCH_TM1 {
                return Err(LoadError::UnknownArch(obj.arch).to_string());
            }
            crate::header::from_object(&obj).map_err(|e| format!("{}: {e}", path.display()))?
        }
        Some(_) => {
            return Err(format!(
                "{}: not a .tmc source or a .tmo object",
                path.display()
            ));
        }
        None => {
            let source = String::from_utf8(bytes).map_err(|_| {
                format!("{}: not a .tmo object and not UTF-8 source", path.display())
            })?;
            // A `.tmh` extension (case-insensitive, matching the repo's
            // standing extension-routing rule) selects declarations-only
            // reading (docs/tmt/language.md (headers)) — text has no
            // container magic to sniff a header from a full source by, so
            // this is the one place the extension itself IS the signal.
            let is_header = path
                .extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| e.eq_ignore_ascii_case("tmh"));
            // The same declarations table `tmt compile` builds from
            // `--extern`/`--nostdlib` (`cli/build.rs::read_externals`): a
            // library whose exported routine, graph or map reaches
            // another unit's alphabet needs it to header at all
            // (docs/tmt/cli.md (interface)).
            let externals = super::build::read_externals(&extern_paths, nostdlib)?;
            let text = if is_header {
                crate::header::from_declarations(&source, &externals)
            } else {
                crate::header::from_source(&source, &externals)
            };
            text.map_err(|e| {
                format!(
                    "{}:{}:{}: error: {} [{}]",
                    path.display(),
                    e.span.start.line,
                    e.span.start.col,
                    e.kind,
                    e.kind.code()
                )
            })?
        }
    };

    if let Some(out) = explicit_out {
        fs::write(&out, &text).map_err(|e| format!("cannot write {out}: {e}"))?;
        Ok(CliOutput::ok(String::new(), String::new()))
    } else {
        Ok(CliOutput::ok(text, String::new()))
    }
}
