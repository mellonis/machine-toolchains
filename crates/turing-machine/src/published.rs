//! The published form of a declaration (docs/tmt/language.md
//! (declarations)): what another unit sees of an exported alphabet, set,
//! map, routine or graph, and so what two readings of one name must agree
//! on. One comparison, [`PublishedForm::agrees`], serves every place that
//! asks whether two readings are the same declaration.
//!
//! The form deliberately leaves out what a caller cannot rely on:
//! `volatile` and `opaque` are not published, `never writes` is folded
//! into the effective write set, doc lines and comments are not compared,
//! and `noreturn` is a fact rather than a span. A tape is compared by its
//! glyph list, never its alphabet's name — a reading taken from a compiled
//! object synthesizes alphabet names, and a call binds by index anyway.

// The conflict checks that compare two readings of one name consume these
// forms; until they land only the tests below do.
#![cfg_attr(not(test), allow(dead_code))]

use std::collections::BTreeMap;

use crate::compiler::{
    ParamKind, Resolved, ResolvedAlphabet, ResolvedMapDecl, ResolvedSet, ResolvedWorld, WorldKind,
    clause_glyphs, declared_effective, symset_glyphs,
};

/// A declaration as another unit sees it (docs/tmt/language.md
/// (declarations)): what two readings of one name must agree on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PublishedForm {
    Alphabet {
        glyphs: Vec<String>,
    },
    Set {
        members: Vec<String>,
    },
    /// Source and target alphabets as glyph lists, pairs as the header
    /// prints them.
    Map {
        src: Vec<String>,
        dst: Vec<String>,
        pairs: String,
    },
    Routine {
        params: Vec<ParamForm>,
        noreturn: bool,
    },
    Graph {
        digest: Option<u32>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ParamForm {
    Tape {
        name: String,
        glyphs: Vec<String>,
        writes: Vec<String>,
        enters: Option<Vec<String>>,
        leaves: Option<Vec<String>>,
    },
    /// `None` for a reading taken from a compiled object, whose
    /// interface carries no state-parameter names (the object arm of
    /// `tmt interface` mints `exit0`, `exit1`, … for them).
    State { name: Option<String> },
}

impl PublishedForm {
    /// THE comparison of two published forms — the only one the corpus
    /// guard and the conflict checks use. Equality, except that two state
    /// parameters agree by position alone when either name is `None` (a
    /// routine's parameter lists must still have the same length and agree
    /// element by element). Never `==` between forms of two readings.
    pub(crate) fn agrees(&self, other: &Self) -> bool {
        match (self, other) {
            (
                PublishedForm::Routine {
                    params: a,
                    noreturn: na,
                },
                PublishedForm::Routine {
                    params: b,
                    noreturn: nb,
                },
            ) => na == nb && a.len() == b.len() && a.iter().zip(b).all(|(x, y)| x.agrees(y)),
            _ => self == other,
        }
    }
}

impl ParamForm {
    fn agrees(&self, other: &Self) -> bool {
        match (self, other) {
            (ParamForm::State { name: Some(a) }, ParamForm::State { name: Some(b) }) => a == b,
            (ParamForm::State { .. }, ParamForm::State { .. }) => true,
            _ => self == other,
        }
    }
}

/// Every EXPORTED alphabet, set, map, routine and graph `module` declares,
/// keyed by qualified name. A module with `from_object` set yields
/// `State { name: None }` for every state parameter.
pub(crate) fn module_forms(module: &Resolved) -> BTreeMap<String, PublishedForm> {
    let mut forms = BTreeMap::new();
    for a in module.alphabets.values().filter(|a| a.exported) {
        forms.insert(a.name.clone(), alphabet_form(a));
    }
    for s in module.sets.values().filter(|s| s.exported) {
        forms.insert(s.name.clone(), set_form(s));
    }
    for m in module.maps.values().filter(|m| m.exported) {
        forms.insert(m.name.clone(), map_form(m, module));
    }
    for w in module
        .worlds
        .iter()
        .filter(|w| w.exported && w.kind != WorldKind::Machine)
    {
        let form = match w.kind {
            WorldKind::Graph => graph_form(w),
            _ => routine_form(w, module),
        };
        forms.insert(w.name.clone(), form);
    }
    forms
}

pub(crate) fn alphabet_form(a: &ResolvedAlphabet) -> PublishedForm {
    PublishedForm::Alphabet {
        glyphs: a.glyphs.clone(),
    }
}

pub(crate) fn set_form(s: &ResolvedSet) -> PublishedForm {
    PublishedForm::Set {
        members: s.glyphs.clone(),
    }
}

/// An alphabet's glyph list by name; when the module does not carry it
/// (this runs on every compile, the editor's included, so it must not
/// panic), the name itself stands in and the form compares by name.
fn glyphs_of(module: &Resolved, alphabet: &str) -> Vec<String> {
    module
        .alphabets
        .get(alphabet)
        .map(|a| a.glyphs.clone())
        .unwrap_or_else(|| vec![alphabet.to_string()])
}

pub(crate) fn map_form(m: &ResolvedMapDecl, module: &Resolved) -> PublishedForm {
    PublishedForm::Map {
        src: glyphs_of(module, &m.src),
        dst: glyphs_of(module, &m.dst),
        pairs: crate::header::map_pair_list_text(&m.pairs),
    }
}

pub(crate) fn routine_form(w: &ResolvedWorld, module: &Resolved) -> PublishedForm {
    let mut tapes = w.tapes.iter();
    let mut states = w.state_params.iter();
    let mut params = Vec::new();
    for kind in &w.param_kinds {
        match kind {
            ParamKind::Tape => {
                let Some(tape) = tapes.next() else { continue };
                let glyphs = glyphs_of(module, &tape.alphabet);
                params.push(ParamForm::Tape {
                    name: tape.name.clone(),
                    writes: symset_glyphs(declared_effective(tape), &glyphs),
                    enters: clause_glyphs(tape.enters, &glyphs),
                    leaves: clause_glyphs(tape.leaves, &glyphs),
                    glyphs,
                });
            }
            ParamKind::State => {
                let Some(name) = states.next() else { continue };
                params.push(ParamForm::State {
                    name: (!module.from_object).then(|| name.clone()),
                });
            }
        }
    }
    PublishedForm::Routine {
        params,
        noreturn: w.declared_noreturn.is_some(),
    }
}

pub(crate) fn graph_form(w: &ResolvedWorld) -> PublishedForm {
    PublishedForm::Graph { digest: w.digest }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::declarations::{Declarations, Origin};

    fn analyzed(source: &str) -> Resolved {
        crate::compiler::analyze(source)
            .unwrap_or_else(|e| panic!("the fixture analyzes: {:?}", e.kind))
            .resolved
    }

    fn routine_in(source: &str, name: &str) -> PublishedForm {
        let module = analyzed(source);
        let world = module
            .worlds
            .iter()
            .find(|w| w.name == name)
            .expect("the routine is in the module");
        routine_form(world, &module)
    }

    /// Two modules declaring the same routine over alphabets of different
    /// names but equal glyphs publish equal forms. Mutation: compare
    /// `tape.alphabet` (the alphabet's name) in place of its glyphs.
    #[test]
    fn a_tape_is_compared_by_its_glyphs_not_its_alphabet_name() {
        let a = routine_in(
            "namespace lib {\n  export alphabet ab { '_', 'a' }\n  export routine r(tape t: ab writes { 'a' }) {\n    entry state s { [*] -> return; }\n  }\n}\n",
            "lib::r",
        );
        let b = routine_in(
            "namespace lib {\n  export alphabet other { '_', 'a' }\n  export routine r(tape t: other writes { 'a' }) {\n    entry state s { [*] -> return; }\n  }\n}\n",
            "lib::r",
        );
        assert!(a.agrees(&b), "{a:?} against {b:?}");
        assert_eq!(a, b);
    }

    /// An alphabet that reached a module by a cross-unit reference is
    /// published by its own unit only; the near miss is the module's own
    /// exported alphabet, which is published. Mutation: select alphabets
    /// without the `exported` flag.
    #[test]
    fn an_alphabet_copied_from_another_module_is_not_published() {
        let lib = crate::header::read_extern(
            std::path::Path::new("lib.tmh"),
            "namespace lib {\n  export alphabet marks { '_', 'x' }\n}\n",
            &Declarations::none(),
        )
        .expect("the header reads");
        let mut externals = Declarations::none();
        externals.push(Origin::Library("lib".to_string()), lib);
        let consumer = crate::compiler::analyze_with(
            "namespace app {\n  export alphabet own { '_', 'o' }\n  export routine r(tape t: lib::marks) {\n    entry state s { [*] -> return; }\n  }\n}\n",
            &externals,
        )
        .expect("the consumer analyzes")
        .resolved;
        assert!(
            consumer.alphabets.contains_key("lib::marks"),
            "the fixture must copy the foreign alphabet in"
        );
        let forms = module_forms(&consumer);
        assert!(!forms.contains_key("lib::marks"), "{forms:?}");
        assert!(forms.contains_key("app::own"), "{forms:?}");
        assert!(forms.contains_key("app::r"), "{forms:?}");
    }

    fn tape(name: &str) -> ParamForm {
        ParamForm::Tape {
            name: name.to_string(),
            glyphs: vec!["_".to_string()],
            writes: vec![],
            enters: None,
            leaves: None,
        }
    }

    fn state(name: Option<&str>) -> ParamForm {
        ParamForm::State {
            name: name.map(str::to_string),
        }
    }

    fn routine(params: Vec<ParamForm>) -> PublishedForm {
        PublishedForm::Routine {
            params,
            noreturn: false,
        }
    }

    /// State parameters agree by name when both are named, and by position
    /// when either is unnamed — but never across a different count.
    /// Mutation: derive the comparison (`==`) — the first pair then
    /// disagrees; or ignore length — the third pair then agrees.
    #[test]
    fn state_parameters_agree_by_name_or_by_position_when_one_is_unnamed() {
        let named = routine(vec![tape("t"), state(Some("done"))]);
        let unnamed = routine(vec![tape("t"), state(None)]);
        assert!(named.agrees(&unnamed));
        assert!(unnamed.agrees(&named));
        assert!(!routine(vec![state(Some("done"))]).agrees(&routine(vec![state(Some("fin"))])));
        assert!(
            !routine(vec![state(Some("done"))]).agrees(&routine(vec![state(None), state(None)]))
        );
    }

    /// For every unit the repository ships, reading its source for its
    /// declarations and reading its generated header yield equal published
    /// forms — the precondition of the one-declaration rule, which would
    /// otherwise refuse a unit compiled beside its own up-to-date header.
    /// Mutation: skip the writes step of `header::settle_declarations` —
    /// `tests/golden/a5_call_across_alphabets.tmc`'s uncontracted
    /// `mylib::plusOne` then differs (whole alphabet against its inferred
    /// set).
    #[test]
    fn every_shipped_unit_publishes_the_same_forms_from_source_and_header() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let mut sources = Vec::new();
        for dir in ["tests/golden", "src/stdlib", "../../docs/examples"] {
            let Ok(entries) = std::fs::read_dir(root.join(dir)) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                let paths: Vec<_> = if path.is_dir() {
                    std::fs::read_dir(&path)
                        .unwrap()
                        .flatten()
                        .map(|e| e.path())
                        .collect()
                } else {
                    vec![path]
                };
                sources.extend(
                    paths
                        .into_iter()
                        .filter(|p| p.extension().is_some_and(|e| e == "tmc")),
                );
            }
        }
        sources.sort();
        let (mut units, mut forms) = (0usize, 0usize);
        for path in &sources {
            let text = std::fs::read_to_string(path).unwrap();
            // The embedded stdlib's own source reads against nothing; every
            // other unit against the embedded stdlib, as `tmt interface` does.
            let context = if path.ends_with("src/stdlib/std.tmc") {
                Declarations::none()
            } else {
                Declarations::stdlib()
            };
            let Ok(header) = crate::header::from_source(&text, &context) else {
                continue;
            };
            let from_source =
                crate::header::read_extern(std::path::Path::new("unit.tmc"), &text, &context)
                    .unwrap_or_else(|e| panic!("{}: {e:?}", path.display()));
            let from_header =
                crate::header::read_extern(std::path::Path::new("unit.tmh"), &header, &context)
                    .unwrap_or_else(|e| panic!("{}: header: {e:?}", path.display()));
            // Both readings are TEXT, so every state name is `Some` and
            // `agrees` is plain equality here; it is used anyway, as the
            // one comparison.
            let (a, b) = (module_forms(&from_source), module_forms(&from_header));
            assert_eq!(
                a.keys().collect::<Vec<_>>(),
                b.keys().collect::<Vec<_>>(),
                "{}: the two readings publish different names",
                path.display()
            );
            for (name, form) in &a {
                assert!(
                    form.agrees(&b[name]),
                    "{}: `{name}` differs between the source and its header:\n{form:?}\n{:?}",
                    path.display(),
                    b[name]
                );
            }
            // A second, independent count of what the unit exports, from
            // its AST — so an empty or truncated `module_forms` cannot pass.
            let program = crate::compiler::analyze_with(&text, &context)
                .unwrap_or_else(|e| panic!("{}: {e:?}", path.display()))
                .program;
            let exported = program.alphabets.iter().filter(|x| x.exported).count()
                + program.sets.iter().filter(|x| x.exported).count()
                + program.maps.iter().filter(|x| x.exported).count()
                + program.routines.iter().filter(|x| x.exported).count()
                + program.graphs.iter().filter(|x| x.exported).count();
            assert_eq!(
                a.len(),
                exported,
                "{}: published forms against exported declarations",
                path.display()
            );
            units += 1;
            forms += a.len();
        }
        eprintln!("published-form corpus: {units} units, {forms} forms");
        assert!(units >= 14, "only {units} shipped units were read");
        // The corpus exports 48 declarations today (std 42,
        // named_map_foreign_alphabets 4, a5 1, a6 1); the floor leaves room
        // for small corpus edits.
        assert!(forms >= 40, "only {forms} forms were compared");
    }
}
