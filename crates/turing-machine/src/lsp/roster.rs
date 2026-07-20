//! The name roster: everything the completion contexts need to look up by
//! name, distilled out of one resolved module (docs/lsp.md (completions)).
//!
//! Distilled rather than borrowed for one reason — it is the service's
//! sanctioned staleness exception. Positions always come from the CURRENT
//! token stream, but a document mid-edit usually does not resolve, and a
//! completion list that empties out the moment a bracket is unbalanced is
//! useless. The roster therefore survives a failed re-analysis, so only
//! NAMES and GLYPHS can ever be one edit old; nothing positional is
//! retained.

use std::collections::HashMap;

use crate::compiler::{Resolved, WorldKind, full_name};
use crate::parser::{AlphabetElem, Program, SymLit};

/// One symbol of a resolved alphabet: the label the compiler compares on,
/// plus how it must be SPELLED to be that symbol again in source.
///
/// The two differ, and the difference matters: a resolved alphabet stores
/// labels only, so `'0'` (a glyph) and `0` (a number) both arrive as the
/// string `0`. Completing the wrong one produces a symbol the alphabet
/// does not contain. The numeric flag is recovered from the alphabet's own
/// source elements — the numeric singles and numeric ranges are exactly
/// the labels that spell bare — so no range needs re-expanding here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GlyphEntry {
    pub(crate) label: String,
    numeric: bool,
}

impl GlyphEntry {
    /// The source spelling: a bare decimal for a numeric symbol, a
    /// single-quoted literal otherwise (only `'` and `\` need escaping).
    pub(crate) fn spelling(&self) -> String {
        if self.numeric {
            self.label.clone()
        } else {
            format!(
                "'{}'",
                self.label.replace('\\', "\\\\").replace('\'', "\\'")
            )
        }
    }
}

/// One world's addressable names, keyed in [`Roster::worlds`] by the
/// world's mangled name (`main` for the machine, `ns::name` otherwise).
#[derive(Debug, Clone)]
pub(crate) struct WorldRoster {
    pub(crate) kind: WorldKind,
    /// Tapes in VECTOR-POSITION order: `(tape name, mangled alphabet)`.
    /// Position `i` is what a pattern/write cell at index `i` draws from —
    /// the whole reason a cell can offer the right glyphs.
    pub(crate) tapes: Vec<(String, String)>,
    /// State names declared in this world.
    pub(crate) states: Vec<String>,
    /// State-parameter names (routine/graph signatures) — also legal
    /// `goto` targets.
    pub(crate) state_params: Vec<String>,
    /// Graft instance names (`… as NAME`) — addressable like states.
    pub(crate) grafts: Vec<String>,
    /// Bind instance names — legal `call` targets.
    pub(crate) binds: Vec<String>,
}

impl WorldRoster {
    /// Every name a `goto` / continuation may address in this world.
    pub(crate) fn transition_targets(&self) -> Vec<String> {
        let mut out = self.states.clone();
        out.extend(self.state_params.iter().cloned());
        out.extend(self.grafts.iter().cloned());
        out
    }

    /// The mangled alphabet the tape at vector position `index` draws
    /// from, or `None` when the world has no such position.
    pub(crate) fn alphabet_at(&self, index: usize) -> Option<&str> {
        self.tapes.get(index).map(|(_, a)| a.as_str())
    }

    /// The mangled alphabet of the tape parameter named `param`, for
    /// resolving the CALLEE side of a binding map.
    pub(crate) fn alphabet_of_param(&self, param: &str) -> Option<&str> {
        self.tapes
            .iter()
            .find(|(name, _)| name == param)
            .map(|(_, a)| a.as_str())
    }
}

#[derive(Debug, Clone, Default)]
pub(crate) struct Roster {
    /// Mangled alphabet name → its symbols in position order.
    pub(crate) alphabets: HashMap<String, Vec<GlyphEntry>>,
    /// Mangled world name → its names.
    pub(crate) worlds: HashMap<String, WorldRoster>,
    /// Mangled names of `routine` worlds (call / bind targets).
    pub(crate) routines: Vec<String>,
    /// Mangled names of `graph` worlds (graft targets).
    pub(crate) graphs: Vec<String>,
    /// `use` bindings: the bare name a path binds → its full path.
    pub(crate) imports: Vec<(String, String)>,
}

impl Roster {
    pub(crate) fn build(resolved: &Resolved, program: Option<&Program>) -> Roster {
        let numeric = numeric_labels(program);
        let mut roster = Roster {
            alphabets: resolved
                .alphabets
                .iter()
                .map(|(name, a)| {
                    let bare = numeric.get(name);
                    let entries = a
                        .glyphs
                        .iter()
                        .map(|label| GlyphEntry {
                            label: label.clone(),
                            numeric: bare.is_some_and(|set| set.contains(label)),
                        })
                        .collect();
                    (name.clone(), entries)
                })
                .collect(),
            ..Roster::default()
        };
        for world in &resolved.worlds {
            match world.kind {
                WorldKind::Routine => roster.routines.push(world.name.clone()),
                WorldKind::Graph => roster.graphs.push(world.name.clone()),
                WorldKind::Machine => {}
            }
            roster.worlds.insert(
                world.name.clone(),
                WorldRoster {
                    kind: world.kind,
                    tapes: world
                        .tapes
                        .iter()
                        .map(|t| (t.name.clone(), t.alphabet.clone()))
                        .collect(),
                    states: world.states.iter().map(|s| s.name.clone()).collect(),
                    state_params: world.state_params.clone(),
                    grafts: world
                        .grafts
                        .iter()
                        .filter_map(|g| g.as_name.clone())
                        .collect(),
                    binds: world.binds.iter().map(|b| b.name.clone()).collect(),
                },
            );
        }
        roster.routines.sort();
        roster.graphs.sort();
        if let Some(program) = program {
            roster.imports = program
                .imports
                .iter()
                .map(|i| (i.binding().to_string(), i.full_path()))
                .collect();
        }
        roster
    }

    /// The symbols of a mangled alphabet, in position order.
    pub(crate) fn glyphs(&self, mangled: &str) -> Option<&[GlyphEntry]> {
        self.alphabets.get(mangled).map(Vec::as_slice)
    }

    /// Resolves a name AS WRITTEN in source to the mangled world it names:
    /// an exact hit first, then the `use`-bound spelling, then a
    /// same-namespace sibling of `scope` (the innermost enclosing
    /// namespace path). Mirrors what the resolver itself accepts, which is
    /// what makes a completed name a name the compiler will also take.
    pub(crate) fn resolve_world(&self, written: &str, scope: &[String]) -> Option<&WorldRoster> {
        if let Some(world) = self.worlds.get(written) {
            return Some(world);
        }
        if let Some((_, full)) = self.imports.iter().find(|(binding, _)| binding == written)
            && let Some(world) = self.worlds.get(full.as_str())
        {
            return Some(world);
        }
        for depth in (0..scope.len()).rev() {
            let qualified = format!("{}::{written}", scope[..=depth].join("::"));
            if let Some(world) = self.worlds.get(&qualified) {
                return Some(world);
            }
        }
        None
    }

    /// Alphabet names offered in an alphabet position: every mangled name,
    /// plus the bare spellings `use` made available.
    pub(crate) fn alphabet_names(&self) -> Vec<String> {
        let mut out: Vec<String> = self.alphabets.keys().cloned().collect();
        for (binding, full) in &self.imports {
            if self.alphabets.contains_key(full.as_str()) && !out.contains(binding) {
                out.push(binding.clone());
            }
        }
        out.sort();
        out
    }

    /// Routine names offered in a call/bind target position: mangled names
    /// plus the bare spellings `use` made available.
    pub(crate) fn routine_names(&self) -> Vec<String> {
        self.with_import_aliases(&self.routines)
    }

    /// Graph names offered in a graft target position.
    pub(crate) fn graph_names(&self) -> Vec<String> {
        self.with_import_aliases(&self.graphs)
    }

    /// True when the roster knows an alphabet by that mangled name.
    pub(crate) fn has_alphabet(&self, mangled: &str) -> bool {
        self.alphabets.contains_key(mangled)
    }

    fn with_import_aliases(&self, mangled: &[String]) -> Vec<String> {
        let mut out = mangled.to_vec();
        for (binding, full) in &self.imports {
            if mangled.iter().any(|m| m == full) && !out.contains(binding) {
                out.push(binding.clone());
            }
        }
        out.sort();
        out
    }
}

/// Per mangled alphabet, the labels its source declared NUMERICALLY — the
/// ones that spell as bare decimals rather than quoted glyphs. Numeric
/// ranges contribute every value in the range without expanding any glyph
/// range, since a glyph range's labels spell quoted either way.
fn numeric_labels(program: Option<&Program>) -> HashMap<String, std::collections::HashSet<String>> {
    let mut out: HashMap<String, std::collections::HashSet<String>> = HashMap::new();
    let Some(program) = program else {
        return out;
    };
    for alphabet in &program.alphabets {
        let mut labels = std::collections::HashSet::new();
        for elem in &alphabet.elems {
            match elem {
                AlphabetElem::Single(SymLit::Number { value, .. }) => {
                    labels.insert(value.to_string());
                }
                AlphabetElem::Range {
                    lo: SymLit::Number { value: lo, .. },
                    hi: SymLit::Number { value: hi, .. },
                    ..
                } => {
                    for v in *lo..=*hi {
                        labels.insert(v.to_string());
                    }
                }
                _ => {}
            }
        }
        if !labels.is_empty() {
            out.insert(full_name(&alphabet.ns, &alphabet.name), labels);
        }
    }
    out
}
