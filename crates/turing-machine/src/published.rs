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

use std::collections::BTreeMap;

use mtc_core::diagnostics::Span;

use crate::compiler::{
    CompileError, CompileErrorKind, ParamKind, Provenance, ReadMode, Resolved, ResolvedAlphabet,
    ResolvedMapDecl, ResolvedSet, ResolvedWorld, WorldKind, clause_glyphs, declared_effective,
    full_name, symset_glyphs,
};
use crate::declarations::Declarations;
use crate::parser::Program;

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

/// One declaration a module carries, as the lookups a consumer uses see
/// it: its qualified name, whether the module PUBLISHES it (exports it),
/// and its form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct VisibleForm {
    pub name: String,
    pub published: bool,
    pub form: PublishedForm,
}

/// Every alphabet, set, map, routine and graph `module` carries, exported
/// or not, sorted by name — exactly what the cross-unit lookups
/// (`compiler::external_decls` and the per-kind lookups over it,
/// `footprint::find_external`, `Declarations::routine`) can return from
/// the module: none of them reads the `exported` flag, so a header's
/// printed private graph, a leniently read source's private routine and
/// an alphabet a module merely copied in from another unit are all
/// visible to a consumer when their module comes first in the table. A
/// machine is never a declaration. A module with `from_object` set yields
/// `State { name: None }` for every state parameter.
pub(crate) fn visible_forms(module: &Resolved) -> Vec<VisibleForm> {
    let mut forms = Vec::new();
    let mut push = |name: &str, published: bool, form: PublishedForm| {
        forms.push(VisibleForm {
            name: name.to_string(),
            published,
            form,
        });
    };
    for a in module.alphabets.values() {
        push(&a.name, a.exported, alphabet_form(a));
    }
    for s in module.sets.values() {
        push(&s.name, s.exported, set_form(s));
    }
    for m in module.maps.values() {
        push(&m.name, m.exported, map_form(m, module));
    }
    for w in module
        .worlds
        .iter()
        .filter(|w| w.kind != WorldKind::Machine)
    {
        let form = match w.kind {
            WorldKind::Graph => graph_form(w),
            _ => routine_form(w, module),
        };
        push(&w.name, w.exported, form);
    }
    forms.sort_by(|a, b| a.name.cmp(&b.name));
    forms
}

/// Every EXPORTED alphabet, set, map, routine and graph `module` declares,
/// keyed by qualified name — the published subset of [`visible_forms`],
/// which is what the shipped-corpus guard below compares between a unit's
/// source and its generated header.
#[cfg(test)]
pub(crate) fn module_forms(module: &Resolved) -> BTreeMap<String, PublishedForm> {
    visible_forms(module)
        .into_iter()
        .filter(|v| v.published)
        .map(|v| (v.name, v.form))
        .collect()
}

/// THE rule for two readings of one name taken from two different units
/// (docs/tmt/language.md (declarations)): they conflict when at least one
/// of them is published and their forms disagree. A private declaration
/// takes part because the lookups a consumer uses see a declaration
/// whatever its export status, so a private name beside a published one
/// of another form would be picked by table order; two private
/// declarations of one name are each their own unit's business and are
/// never compared.
pub(crate) fn readings_conflict(
    a_published: bool,
    a: &PublishedForm,
    b_published: bool,
    b: &PublishedForm,
) -> bool {
    (a_published || b_published) && !a.agrees(b)
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

/// The unit's own definitions — every alphabet, set, map, routine and
/// graph `program` declares, exported or not — each with its name span,
/// whether the unit exports it, and its form, settled exactly as a
/// declarations reading of it
/// would be (`header::settle_declarations`). With [`Provenance::Object`]
/// every state parameter's name is `None`.
pub(crate) fn own_forms(
    program: &Program,
    resolved: &Resolved,
    externals: &Declarations,
    mode: ReadMode,
    provenance: Provenance,
) -> BTreeMap<String, (Span, bool, PublishedForm)> {
    let mut settled = resolved.clone();
    settled.from_object = provenance == Provenance::Object;
    crate::header::settle_declarations(program, &mut settled, externals, mode);
    let mut forms = BTreeMap::new();
    for a in &program.alphabets {
        let name = full_name(&a.ns, &a.name);
        if let Some(r) = settled.alphabets.get(&name) {
            forms.insert(name, (a.name_span, a.exported, alphabet_form(r)));
        }
    }
    for s in &program.sets {
        let name = full_name(&s.ns, &s.name);
        if let Some(r) = settled.sets.get(&name) {
            forms.insert(name, (s.name_span, s.exported, set_form(r)));
        }
    }
    for m in &program.maps {
        let name = full_name(&m.ns, &m.name);
        if let Some(r) = settled.maps.get(&name) {
            forms.insert(name, (m.name_span, m.exported, map_form(r, &settled)));
        }
    }
    for (name, span, exported, graph) in program
        .routines
        .iter()
        .map(|r| (full_name(&r.ns, &r.name), r.name_span, r.exported, false))
        .chain(
            program
                .graphs
                .iter()
                .map(|g| (full_name(&g.ns, &g.name), g.name_span, g.exported, true)),
        )
    {
        let Some(world) = settled.worlds.iter().find(|w| w.name == name) else {
            continue;
        };
        let form = if graph {
            graph_form(world)
        } else {
            routine_form(world, &settled)
        };
        forms.insert(name, (span, exported, form));
    }
    forms
}

/// Whether `module` declares `name` under any kind — a cheap superset of
/// "carries a form of it", used to skip the settle cost when nothing could
/// clash.
fn declares(module: &Resolved, name: &str) -> bool {
    module.alphabets.contains_key(name)
        || module.sets.contains_key(name)
        || module.maps.contains_key(name)
        || module.worlds.iter().any(|w| w.name == name)
}

/// The unit's own definitions against what the table carries under the
/// same names (docs/tmt/language.md (declarations)), by the one rule for
/// two readings of a name ([`readings_conflict`]): a name ANY table module
/// declares differently is refused at the unit's definition when either
/// side publishes it — the unit's private definition against a published
/// declaration, and the unit's published definition against a declaration
/// a module carries privately, since a consumer of both would be handed
/// whichever came first. Two private ones are not compared. `agrees` is
/// not transitive, so every reading is tested; the origin named is the
/// first disagreeing one in table order. Two table modules disagreeing
/// with each other is [`Declarations::check_conflicts`]' business.
pub(crate) fn check_own_against_table(
    program: &Program,
    resolved: &Resolved,
    externals: &Declarations,
    mode: ReadMode,
    provenance: Provenance,
) -> Result<(), CompileError> {
    let own_names: Vec<String> = program
        .alphabets
        .iter()
        .map(|a| full_name(&a.ns, &a.name))
        .chain(program.sets.iter().map(|s| full_name(&s.ns, &s.name)))
        .chain(program.maps.iter().map(|m| full_name(&m.ns, &m.name)))
        .chain(program.routines.iter().map(|r| full_name(&r.ns, &r.name)))
        .chain(program.graphs.iter().map(|g| full_name(&g.ns, &g.name)))
        .collect();
    // The cheap gate: the editor's keystroke path analyzes a unit whose
    // names the table almost never declares, and must not pay the settle.
    let candidates: Vec<_> = externals
        .entries()
        .filter(|(_, m)| own_names.iter().any(|n| declares(m, n)))
        .map(|(origin, m)| (origin, visible_forms(m)))
        .collect();
    if candidates.is_empty() {
        return Ok(());
    }
    for (name, (span, published, own)) in own_forms(program, resolved, externals, mode, provenance)
    {
        let clash = candidates.iter().find_map(|(origin, forms)| {
            forms
                .iter()
                .any(|v| v.name == name && readings_conflict(published, &own, v.published, &v.form))
                .then_some(origin)
        });
        if let Some(origin) = clash {
            return Err(CompileError {
                span,
                kind: CompileErrorKind::ConflictingDeclaration {
                    name,
                    first: "this unit".to_string(),
                    second: origin.describe(),
                },
            });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::declarations::{Declarations, Origin};

    use crate::compiler::{CompileErrorKind as Kind, analyze_with};
    use std::path::{Path, PathBuf};

    const L1: &str = "namespace lib {\n  export alphabet ab { '_', 'a' }\n  export routine r(tape t: ab writes { 'a' });\n}\n";
    const L2: &str = "namespace lib {\n  export alphabet ab { '_', 'a' }\n  export routine r(tape t: ab writes { 'a' }, state done);\n}\n";
    const C8: &str = "use lib::ab;\nuse lib::r;\nmachine {\n  tape t: ab;\n  entry state s { [*] -> call r(t = t) then stop; }\n}\n";
    /// The same caller with an argument-less (transparent) call, which is
    /// never checked by name: the pure merge property, free of which
    /// reading a named list would be checked against.
    const C8_TRANSPARENT: &str = "use lib::ab;\nuse lib::r;\nmachine {\n  tape t: ab;\n  entry state s { [*] -> call r() then stop; }\n}\n";
    /// A valid named caller against the text readings' parameter names.
    const C8_NAMED: &str = "use lib::ab;\nuse lib::r;\nmachine {\n  tape t: ab;\n  entry state s { [*] -> call r(t = t, done = stop) then stop; }\n}\n";
    const OWN: &str = "namespace lib {\n  export alphabet ab { '_', 'a' }\n  export routine r(tape t: ab writes { 'a' }, state done) {\n    entry state s { [*] -> goto done; }\n  }\n}\nmachine {\n  tape t: lib::ab;\n  entry state s { [*] -> call lib::r(t = t, done = stop) then stop; }\n}\n";
    const OWN_FIN: &str = "namespace lib {\n  export alphabet ab { '_', 'a' }\n  export routine r(tape t: ab writes { 'a' }, state fin) {\n    entry state s { [*] -> goto fin; }\n  }\n}\nmachine {\n  tape t: lib::ab;\n  entry state s { [*] -> call lib::r(t = t, fin = stop) then stop; }\n}\n";
    const C_OWN: &str = "use lib::ab;\nuse lib::r;\nmachine {\n  tape t: ab;\n  entry state s { [*] -> call r(t = t, done = stop) then stop; }\n}\n";

    fn table(modules: &[(&str, &str)]) -> Declarations {
        let mut decls = Declarations::none();
        for (path, text) in modules {
            let resolved = crate::header::read_extern(Path::new(path), text, &Declarations::none())
                .expect("reads");
            decls.push(Origin::Extern(PathBuf::from(path)), resolved);
        }
        decls
    }

    /// Mutation: drop `Declarations::check_conflicts` from `resolve_program`.
    #[test]
    fn two_table_modules_publishing_one_name_differently_conflict() {
        let err = analyze_with(C8, &table(&[("l1.tmh", L1), ("l2.tmh", L2)])).unwrap_err();
        assert_eq!(err.span, Span::point(0, 0));
        assert_eq!(
            err.kind,
            Kind::ConflictingDeclaration {
                name: "lib::r".into(),
                first: "`l1.tmh`".into(),
                second: "`l2.tmh`".into(),
            }
        );
    }

    /// Mutation: compare modules by origin instead of by form.
    #[test]
    fn two_table_modules_publishing_one_name_identically_merge() {
        assert!(analyze_with(C8, &table(&[("l1.tmh", L1), ("l1b.tmh", L1)])).is_ok());
    }

    /// A name of another kind is another declaration. Mutation: compare
    /// only forms of the same kind.
    #[test]
    fn an_own_name_of_another_kind_conflicts() {
        let src = "namespace lib {\n  alphabet r { '_', 'a' }\n}\nmachine {\n  tape t: lib::r;\n  entry state s { [*] -> stop; }\n}\n";
        let err = analyze_with(src, &table(&[("l1.tmh", L1)])).unwrap_err();
        assert!(
            matches!(&err.kind, Kind::ConflictingDeclaration { name, first, .. }
                if name == "lib::r" && first == "this unit"),
            "{err:?}"
        );
        assert_eq!(err.span.start.line, 2);
    }

    fn own_object() -> mtc_core::formats::object::ObjectFile {
        crate::compiler::compile(
            OWN,
            crate::compiler::CompileOptions {
                externals: Declarations::none(),
                ..Default::default()
            },
        )
        .expect("OWN compiles alone")
        .object
    }

    /// The object reading names its exit `exit0`, the source reading
    /// `done`; they are one unit and must merge. The TEXT reading goes
    /// first so the consumer's `done = …` binds against a named parameter.
    /// Mutation: compare forms by derived `==` — they conflict on the
    /// state name.
    #[test]
    fn an_object_reading_and_a_source_reading_of_one_unit_merge() {
        let mut decls = table(&[("own.tmc", OWN)]);
        let from_object =
            crate::header::declarations_from_object(&own_object(), &Declarations::none())
                .expect("the object reads");
        decls.push(Origin::Library("own".into()), from_object);
        assert!(analyze_with(C_OWN, &decls).is_ok());
    }

    /// The object read's OWN-definition check, against a table that
    /// already holds the source reading. Mutation: `own_forms` ignoring
    /// `Provenance`.
    #[test]
    fn an_object_read_beside_its_source_reading_is_not_a_conflict() {
        let context = table(&[("own.tmc", OWN)]);
        assert!(crate::header::declarations_from_object(&own_object(), &context).is_ok());
    }

    /// Two TEXT readings disagree on a state name: a real difference (a
    /// caller binds by name). Mutation: treat every state name as optional
    /// (count only).
    #[test]
    fn two_text_readings_differing_in_a_state_name_conflict() {
        let err =
            analyze_with(C8, &table(&[("own.tmc", OWN), ("own_fin.tmc", OWN_FIN)])).unwrap_err();
        assert_eq!(
            err.kind,
            Kind::ConflictingDeclaration {
                name: "lib::r".into(),
                first: "`own.tmc`".into(),
                second: "`own_fin.tmc`".into(),
            }
        );
    }

    fn object_reading() -> Resolved {
        crate::header::declarations_from_object(&own_object(), &Declarations::none())
            .expect("the object reads")
    }

    fn text_reading(path: &str, text: &str) -> Resolved {
        crate::header::read_extern(Path::new(path), text, &Declarations::none()).expect("reads")
    }

    /// Three readings of `lib::r` — an object's (nameless state
    /// parameters), a text one naming it `done`, a text one naming it
    /// `fin` — conflict wherever the object reading sits in the table:
    /// `agrees` is not transitive, so every reading must be tested against
    /// every other. Mutation: compare each reading only against the first
    /// one seen (`Declarations::check_conflicts`).
    #[test]
    fn three_readings_conflict_whatever_the_position_of_the_object_reading() {
        let others = [("own.tmc", OWN), ("own_fin.tmc", OWN_FIN)];
        for position in 0..3 {
            let mut decls = Declarations::none();
            let mut texts = others.iter();
            for i in 0..3 {
                if i == position {
                    decls.push(Origin::Library("own".into()), object_reading());
                } else {
                    let (path, text) = texts.next().unwrap();
                    decls.push(
                        Origin::Extern(PathBuf::from(path)),
                        text_reading(path, text),
                    );
                }
            }
            let err = analyze_with(C8, &decls).unwrap_err();
            assert!(
                matches!(&err.kind, Kind::ConflictingDeclaration { name, .. } if name == "lib::r"),
                "object at {position}: {err:?}"
            );
        }
    }

    /// The near miss: the object reading and two text readings that agree
    /// with each other merge, wherever the object sits. Mutation: treat
    /// the object reading's nameless parameters as disagreeing.
    #[test]
    fn an_object_reading_and_two_agreeing_text_readings_merge_in_any_position() {
        let others = [("own.tmc", OWN), ("own_copy.tmc", OWN)];
        for position in 0..3 {
            let mut decls = Declarations::none();
            let mut texts = others.iter();
            for i in 0..3 {
                if i == position {
                    decls.push(Origin::Library("own".into()), object_reading());
                } else {
                    let (path, text) = texts.next().unwrap();
                    decls.push(
                        Origin::Extern(PathBuf::from(path)),
                        text_reading(path, text),
                    );
                }
            }
            assert!(
                analyze_with(C8_TRANSPARENT, &decls).is_ok(),
                "object at {position}"
            );
            assert!(
                analyze_with(C8_NAMED, &decls).is_ok(),
                "named caller, object at {position}"
            );
        }
    }

    /// The table in `object` position `position` among three readings of
    /// `lib::r`: the object's and two agreeing text ones.
    fn three_readings(position: usize) -> Declarations {
        let others = [("own.tmc", OWN), ("own_copy.tmc", OWN)];
        let mut decls = Declarations::none();
        let mut texts = others.iter();
        for i in 0..3 {
            if i == position {
                decls.push(Origin::Library("own".into()), object_reading());
            } else {
                let (path, text) = texts.next().unwrap();
                decls.push(
                    Origin::Extern(PathBuf::from(path)),
                    text_reading(path, text),
                );
            }
        }
        decls
    }

    /// A caller naming the text readings' state parameter compiles whatever
    /// the position of the object reading: the argument check reads the
    /// text reading, never the object's minted names. Mutation: take the
    /// first reading regardless of provenance (`declarations::first_reading`
    /// ignoring `from_object`) — position 0 reports `unknown-arg`.
    #[test]
    fn a_named_caller_compiles_whatever_the_position_of_the_object_reading() {
        for position in 0..3 {
            let decls = three_readings(position);
            if let Err(e) = analyze_with(C8_NAMED, &decls) {
                panic!("object at {position}: {e:?}");
            }
        }
    }

    /// With only the object reading, the minted name is the one on offer
    /// and an authored name is not invented. Mutation: drop the object
    /// fallback in `first_reading` (the minted-name caller then compiles
    /// as an unchecked external and the authored-name one is not refused).
    #[test]
    fn an_object_reading_alone_offers_only_its_minted_names() {
        let mut decls = Declarations::none();
        decls.push(Origin::Library("own".into()), object_reading());
        let minted = C8_NAMED.replace("done", "exit0");
        if let Err(e) = analyze_with(&minted, &decls) {
            panic!("minted name refused: {e:?}");
        }
        let err = analyze_with(C8_NAMED, &decls).unwrap_err();
        assert!(
            matches!(&err.kind, Kind::UnknownArg(n) if n == "done"),
            "{err:?}"
        );
    }

    /// The IR lowering reads the same reading as the check: the compiled
    /// object is identical whichever position the object reading takes.
    /// Mutation: revert the rule in `Declarations::routine` only — with the
    /// object reading first, lowering sees the minted name and refuses
    /// `done`.
    #[test]
    fn the_lowering_reads_the_same_reading_whatever_the_order() {
        let compile_at = |position: usize| {
            crate::compiler::compile(
                C8_NAMED,
                crate::compiler::CompileOptions {
                    externals: three_readings(position),
                    ..Default::default()
                },
            )
            .unwrap_or_else(|e| panic!("object at {position}: {e:?}"))
            .object
        };
        let first = format!("{:?}", compile_at(0));
        assert_eq!(first, format!("{:?}", compile_at(1)));
        assert_eq!(first, format!("{:?}", compile_at(2)));
    }

    /// The own-definition check tests the unit against EVERY reading of
    /// the name: the object reading (nameless) agrees with a unit naming
    /// its state `fin`, the text reading naming it `done` does not.
    /// Mutation: compare only against the first reading of the name
    /// (`check_own_against_table`).
    #[test]
    fn an_own_definition_is_tested_against_every_reading_of_its_name() {
        let mut decls = Declarations::none();
        decls.push(Origin::Library("own".into()), object_reading());
        decls.push(
            Origin::Extern(PathBuf::from("own.tmc")),
            text_reading("own.tmc", OWN),
        );
        let err = analyze_with(OWN_FIN, &decls).unwrap_err();
        assert!(
            matches!(&err.kind, Kind::ConflictingDeclaration { first, second, .. }
                if first == "this unit" && second == "`own.tmc`"),
            "{err:?}"
        );
    }

    /// A PRIVATE own routine, graph or set is a definition of its name
    /// too, compared with what the table publishes. Mutation: filter the
    /// unit's routines (resp. graphs, sets) to the exported ones in
    /// `own_forms`.
    #[test]
    fn a_private_own_routine_graph_or_set_conflicts_with_a_published_name() {
        let machine = "machine {\n  tape t: lib::ab;\n  entry state s { [*] -> stop; }\n}\n";
        let bodies = [
            "routine r(tape t: lib::ab writes { 'a' }, state done) {\n    entry state s { [*] -> goto done; }\n  }",
            "graph r(tape t: lib::ab, state done) {\n    entry state s { [*] -> goto done; }\n  }",
            "set r { 'a' }",
        ];
        for body in bodies {
            let src = format!("use lib::ab;\nnamespace lib {{\n  {body}\n}}\n{machine}");
            let err = analyze_with(&src, &table(&[("l1.tmh", L1)])).unwrap_err();
            assert!(
                matches!(&err.kind, Kind::ConflictingDeclaration { name, first, .. }
                    if name == "lib::r" && first == "this unit"),
                "{body}: {err:?}"
            );
        }
    }

    /// A header printing a PRIVATE `graph lib::walker` (the body its
    /// exported `outerA` nests) that writes `'y'`.
    const LA: &str = "namespace lib {\n  export alphabet marks { '_', 'x', 'y' }\n  graph walker(tape t: marks writes { 'y' }, state done) {\n    entry state w {\n      ['x'] -> write ['y'] goto done;\n      [*]   -> goto done;\n    }\n  }\n  export graph outerA(tape a: marks writes { 'y' }, state done) {\n    entry graft walker(t = a, done = done) as inner;\n  }\n}\n";
    /// A header exporting a ROUTINE under the same name.
    const LR: &str = "namespace lib {\n  export alphabet marks { '_', 'x', 'y' }\n  export routine walker(tape t: marks writes {});\n}\n";
    /// A header exporting a GRAPH of another body under the same name.
    const LB: &str = "namespace lib {\n  export alphabet marks { '_', 'x', 'y' }\n  export graph walker(tape t: marks writes { 'x' }, state done) {\n    entry state w {\n      ['y'] -> write ['x'] goto done;\n      [*]   -> goto done;\n    }\n  }\n}\n";
    /// Another library whose PRIVATE `lib::walker` has another body, and
    /// which publishes nothing under that name.
    const LZ: &str = "namespace lib {\n  export alphabet marks { '_', 'x', 'y' }\n  graph walker(tape t: marks writes { 'x' }, state done) {\n    entry state w {\n      ['y'] -> write ['x'] goto done;\n      [*]   -> goto done;\n    }\n  }\n  export graph outerZ(tape a: marks writes { 'x' }, state done) {\n    entry graft walker(t = a, done = done) as inner;\n  }\n}\n";
    /// A source whose PRIVATE `routine lib::r` no header prints; it
    /// writes `'y'`.
    const X_PRIVATE_R: &str = "namespace lib {\n  export alphabet marks { '_', 'x', 'y' }\n  routine r(tape t: marks) {\n    entry state s { [*] -> write ['y'] return; }\n  }\n  export routine other(tape t: marks) {\n    entry state s { [*] -> call r(t = t) then d; }\n    state d { [*] -> return; }\n  }\n}\n";
    /// A unit whose private `r` writes `'x'` instead (and whose export has
    /// another name, so only `lib::r` is contested).
    const X2_PRIVATE_R: &str = "namespace lib {\n  export alphabet marks { '_', 'x', 'y' }\n  routine r(tape t: marks) {\n    entry state s { [*] -> write ['x'] return; }\n  }\n  export routine another(tape t: marks) {\n    entry state s { [*] -> call r(t = t) then d; }\n    state d { [*] -> return; }\n  }\n}\n";
    /// Headers publishing `lib::r` — with another write set than
    /// [`X_PRIVATE_R`]'s, and with the same one.
    const H_R_QUIET: &str = "namespace lib {\n  export alphabet marks { '_', 'x', 'y' }\n  export routine r(tape t: marks writes {});\n}\n";
    const H_R_SAME: &str = "namespace lib {\n  export alphabet marks { '_', 'x', 'y' }\n  export routine r(tape t: marks writes { 'y' });\n}\n";
    const CALLS_WALKER: &str = "use lib::marks;\nroutine quiet(tape t: marks writes {}) {\n  entry state s { [*] -> call lib::walker(t = t) then d; }\n  state d { [*] -> return; }\n}\nmachine {\n  tape t: marks;\n  entry state go { [*] -> call quiet(t = t) then fin; }\n  state fin { [*] -> stop; }\n}\n";
    const GRAFTS_WALKER: &str = "use lib::marks;\nmachine {\n  tape t: marks;\n  entry graft lib::walker(t = t, done = fin) as r;\n  state fin { [*] -> stop; }\n}\n";
    const CALLS_R: &str = "use lib::marks;\nroutine loud(tape t: marks) {\n  entry state s { [*] -> call lib::r(t = t) then d; }\n  state d { [*] -> return; }\n}\nmachine {\n  tape t: marks;\n  entry state go { [*] -> call loud(t = t) then fin; }\n  state fin { [*] -> stop; }\n}\n";
    const GRAFTS_OUTER_A: &str = "use lib::marks;\nuse lib::outerA;\nmachine {\n  tape t: marks;\n  entry graft outerA(a = t, done = fin) as r;\n  state fin { [*] -> stop; }\n}\n";

    /// `caller` against the two modules in BOTH table orders: refused as
    /// a conflict over `name` each time, naming the two origins in table
    /// order.
    fn assert_conflict_in_both_orders(caller: &str, a: (&str, &str), b: (&str, &str), name: &str) {
        for (first, second) in [(a, b), (b, a)] {
            let err = analyze_with(caller, &table(&[first, second])).unwrap_err();
            assert_eq!(
                err.kind,
                Kind::ConflictingDeclaration {
                    name: name.into(),
                    first: format!("`{}`", first.0),
                    second: format!("`{}`", second.0),
                },
                "[{}, {}]",
                first.0,
                second.0
            );
        }
    }

    /// A declaration one module carries without exporting it is what a
    /// consumer's lookup would return when that module comes first, so it
    /// is compared with what the other modules publish: a private graph
    /// beside a published routine, beside a published graph of another
    /// body, and a private routine no header prints beside a published
    /// routine of another form — each refused in both table orders.
    /// Mutation: compare exported forms only (`readings_conflict`
    /// requiring BOTH readings to be published) — every
    /// pair here then compiles or misreports by order.
    #[test]
    fn a_private_declaration_beside_a_published_one_of_another_form_conflicts_in_both_orders() {
        assert_conflict_in_both_orders(CALLS_WALKER, ("la.tmh", LA), ("lr.tmh", LR), "lib::walker");
        assert_conflict_in_both_orders(
            GRAFTS_WALKER,
            ("la.tmh", LA),
            ("lb.tmh", LB),
            "lib::walker",
        );
        assert_conflict_in_both_orders(
            CALLS_R,
            ("x.tmc", X_PRIVATE_R),
            ("h.tmh", H_R_QUIET),
            "lib::r",
        );
    }

    /// The near miss: a private declaration beside a published one of the
    /// SAME form merges, in both orders — a private routine beside a
    /// header publishing the set it really writes. (No such pair exists
    /// for a graph: its digest covers its `export` keyword, so a private
    /// graph and an exported one never have one form.) Mutation: refuse
    /// any private declaration that shares a name with a published one,
    /// whatever its form.
    #[test]
    fn a_private_declaration_beside_a_published_one_of_the_same_form_merges() {
        let (a, b) = (("x.tmc", X_PRIVATE_R), ("h.tmh", H_R_SAME));
        for order in [[a, b], [b, a]] {
            if let Err(e) = analyze_with(CALLS_R, &table(&order)) {
                panic!("[{}, {}]: {e:?}", order[0].0, order[1].0);
            }
        }
    }

    /// Two modules' PRIVATE declarations of one name, with nothing
    /// published under it, are not compared with each other: a consumer
    /// of one module's export compiles in both orders. Mutation: compare
    /// every pair of readings whatever their export status.
    #[test]
    fn two_private_declarations_of_one_name_with_nothing_published_do_not_conflict() {
        for order in [
            [("la.tmh", LA), ("lz.tmh", LZ)],
            [("lz.tmh", LZ), ("la.tmh", LA)],
        ] {
            let out = crate::compiler::compile(
                GRAFTS_OUTER_A,
                crate::compiler::CompileOptions {
                    externals: table(&order),
                    ..Default::default()
                },
            );
            if let Err(e) = out {
                panic!("[{}, {}]: {e:?}", order[0].0, order[1].0);
            }
        }
    }

    /// A published reading EQUAL to an earlier private one is still a
    /// published reading: a third module's private declaration of another
    /// form conflicts with it. Mutation: retain a reading only when no
    /// equal FORM was seen, whatever its export status
    /// (`Declarations::check_conflicts`) — the published reading is then
    /// dropped as a duplicate of the private one and the third module is
    /// compared private against private.
    #[test]
    fn a_published_reading_equal_to_an_earlier_private_one_is_still_compared() {
        let decls = table(&[
            ("x.tmc", X_PRIVATE_R),
            ("h.tmh", H_R_SAME),
            ("x2.tmc", X2_PRIVATE_R),
        ]);
        let err = analyze_with(CALLS_R, &decls).unwrap_err();
        assert_eq!(
            err.kind,
            Kind::ConflictingDeclaration {
                name: "lib::r".into(),
                first: "`h.tmh`".into(),
                second: "`x2.tmc`".into(),
            }
        );
    }

    /// The own-definition check is symmetric with the table's: a unit's
    /// PUBLISHED definition is compared with a declaration a table module
    /// carries privately, as its private definition is with a published
    /// one. With one module in the table the table's own pair check does
    /// not run, so this is the own-definition check alone. Mutation:
    /// compare the unit's definitions with the table's EXPORTED forms
    /// only (`check_own_against_table`).
    #[test]
    fn an_own_published_definition_conflicts_with_a_private_declaration_of_the_table() {
        let own = "namespace lib {\n  export alphabet marks { '_', 'x', 'y' }\n  export routine walker(tape t: marks writes {}) {\n    entry state s { [*] -> return; }\n  }\n}\n";
        let err = analyze_with(own, &table(&[("la.tmh", LA)]))
            .map(|_| ())
            .expect_err("the unit publishes `lib::walker` as another form");
        assert_eq!(
            err.kind,
            Kind::ConflictingDeclaration {
                name: "lib::walker".into(),
                first: "this unit".into(),
                second: "`la.tmh`".into(),
            }
        );
        assert_eq!(err.span.start.line, 3);
    }

    /// The near miss: the unit's PRIVATE graph beside a table module's
    /// private graph of another body is not a conflict. Mutation: compare
    /// the unit's definitions with every declaration a module carries,
    /// whatever either side's export status.
    #[test]
    fn an_own_private_definition_beside_a_private_declaration_of_the_table_is_legal() {
        let own = "namespace lib {\n  export alphabet marks { '_', 'x', 'y' }\n  graph walker(tape t: marks, state done) {\n    entry state w {\n      ['y'] -> write ['x'] goto done;\n      [*]   -> goto done;\n    }\n  }\n  export graph outerZ(tape a: marks, state done) {\n    entry graft walker(t = a, done = done) as inner;\n  }\n}\n";
        if let Err(e) = analyze_with(own, &table(&[("la.tmh", LA)])) {
            panic!("{e:?}");
        }
    }

    /// Doc lines are not part of a declaration: the same header with and
    /// without a `?` doc line merges. `PublishedForm` has no docs field,
    /// so no one-line mutation of it exists; this guards the comparison
    /// against any change that compares text-derived data (the module's
    /// docs table, the raw header text), which makes it fail.
    #[test]
    fn a_doc_line_is_not_part_of_a_declaration() {
        let documented = L1.replace(
            "  export routine r",
            "  ? Documented here.\n  export routine r",
        );
        assert!(analyze_with(C8, &table(&[("l1.tmh", L1), ("l1doc.tmh", &documented)])).is_ok());
    }

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
        let mut skipped: Vec<String> = Vec::new();
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
                skipped.push(path.file_name().unwrap().to_string_lossy().into_owned());
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
        eprintln!("published-form corpus: {units} units, {forms} forms, skipped {skipped:?}");
        // A unit whose header fails to print is named, never silently
        // dropped: the list must be exactly the units that legitimately
        // have no header to compare.
        let expected_skipped: Vec<String> = Vec::new();
        assert_eq!(
            skipped, expected_skipped,
            "units skipped for want of a header"
        );
        assert!(units >= 14, "only {units} shipped units were read");
        // The corpus exports 48 declarations today (std 42,
        // named_map_foreign_alphabets 4, a5 1, a6 1); the floor leaves room
        // for small corpus edits.
        assert!(forms >= 40, "only {forms} forms were compared");
    }
}
