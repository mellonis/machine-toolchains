//! Lexical path identity for map-sidecar source provenance
//! (docs/formats.md (map sidecar)). The build drivers store each
//! function's source path relative to the sidecar's own directory so a
//! build tree stays relocatable and the sidecar carries no machine
//! paths; a DAP adapter resolves those entries back against the sidecar
//! directory it loaded the map from. Both directions are **lexical
//! only**: paths are joined and `.`/`..`-folded as strings, never
//! resolved through the filesystem — the same identity policy the LSP
//! cross-file overlay documents (docs/lsp.md (known caveats)), so a
//! symlinked tree can present one file under two names.

use std::path::{Component, Path, PathBuf};

/// Joins `p` onto `base` when relative (`base` is the caller's anchor —
/// typically the current directory, or the sidecar's directory) and
/// folds `.` and `..` components lexically. A `..` that would climb
/// past the root is dropped rather than kept, matching how a filesystem
/// resolves `/..`. An absolute `p` is folded but not re-anchored.
pub fn lexical_absolute(base: &Path, p: &Path) -> PathBuf {
    let joined = if p.is_absolute() {
        p.to_path_buf()
    } else {
        base.join(p)
    };
    let mut out = PathBuf::new();
    for component in joined.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                // Pop a normal component; keep a leading/stacked `..`
                // only when there is nothing to pop AND we are not
                // anchored at a root (a rootless base can legitimately
                // begin with `..`).
                if matches!(out.components().next_back(), Some(Component::Normal(_))) {
                    out.pop();
                } else if !matches!(
                    out.components().next_back(),
                    Some(Component::RootDir | Component::Prefix(_))
                ) {
                    out.push(Component::ParentDir);
                }
            }
            other => out.push(other),
        }
    }
    out
}

/// The lexical relative path from `base_dir` to `target` — both already
/// absolute and folded (see [`lexical_absolute`]). `None` when the two
/// share no common root (e.g. different Windows drive prefixes), in
/// which case the caller falls back to storing `target` absolute.
pub fn relative_to(base_dir: &Path, target: &Path) -> Option<PathBuf> {
    let mut base = base_dir.components().peekable();
    let mut targ = target.components().peekable();
    // Strip the shared prefix. The first divergence with NO shared
    // component at all (differing `Prefix`/`RootDir`) means no relative
    // path exists.
    let mut shared = 0usize;
    while let (Some(b), Some(t)) = (base.peek(), targ.peek()) {
        if b != t {
            break;
        }
        base.next();
        targ.next();
        shared += 1;
    }
    if shared == 0 {
        return None;
    }
    let mut out = PathBuf::new();
    for _ in base {
        out.push(Component::ParentDir);
    }
    for t in targ {
        out.push(t);
    }
    if out.as_os_str().is_empty() {
        out.push(Component::CurDir);
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absolute_input_is_folded_not_reanchored() {
        assert_eq!(
            lexical_absolute(Path::new("/anchor"), Path::new("/a/b/../c/./d")),
            PathBuf::from("/a/c/d")
        );
    }

    #[test]
    fn relative_input_joins_the_base() {
        assert_eq!(
            lexical_absolute(Path::new("/work/out"), Path::new("../src/main.pmc")),
            PathBuf::from("/work/src/main.pmc")
        );
    }

    #[test]
    fn parent_past_root_is_dropped() {
        assert_eq!(
            lexical_absolute(Path::new("/"), Path::new("../../x")),
            PathBuf::from("/x")
        );
    }

    #[test]
    fn rootless_base_keeps_leading_parents() {
        assert_eq!(
            lexical_absolute(Path::new(".."), Path::new("x/../y")),
            PathBuf::from("../y")
        );
    }

    #[test]
    fn relative_to_sibling_directory() {
        assert_eq!(
            relative_to(Path::new("/work/out"), Path::new("/work/src/main.pmc")),
            Some(PathBuf::from("../src/main.pmc"))
        );
    }

    #[test]
    fn relative_to_same_directory() {
        assert_eq!(
            relative_to(Path::new("/work"), Path::new("/work/main.pmc")),
            Some(PathBuf::from("main.pmc"))
        );
    }

    #[test]
    fn relative_to_the_directory_itself_is_dot() {
        assert_eq!(
            relative_to(Path::new("/work"), Path::new("/work")),
            Some(PathBuf::from("."))
        );
    }

    #[test]
    fn no_common_root_is_none() {
        // Rootless vs rooted share not even the root component.
        assert_eq!(relative_to(Path::new("rel/base"), Path::new("/abs")), None);
    }

    #[test]
    fn round_trip_recovers_the_source() {
        let map_dir = Path::new("/proj/build");
        let source = Path::new("/proj/src/a.pmc");
        let stored = relative_to(map_dir, source).unwrap();
        assert_eq!(lexical_absolute(map_dir, &stored), source);
    }
}
