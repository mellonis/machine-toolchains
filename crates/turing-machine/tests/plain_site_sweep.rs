//! An `#[ignore]`d measurement instrument, not a correctness test: it
//! assembles or compiles every shipped program standalone, resolves each
//! plain call site's callee itself (the linker's own first-wins order —
//! unit then stdlib — without running the real linker), and reports what
//! the planned plain-site size check WOULD say, so the check's blast
//! radius is known before it becomes an error. Run it with
//! `cargo test -p mtc-turing-machine --test plain_site_sweep -- --ignored --nocapture`.

use std::fs;
use std::path::{Path, PathBuf};

use mtc_core::formats::object::{ObjectFile, RoutineSig};
use mtc_turing_machine::asm::{assemble, tm1_syntax};
use mtc_turing_machine::compiler::{CompileOptions, compile};

/// Every `.tmc` and `.tma` the repository ships, recursively, under the
/// three roots that hold real programs.
fn corpus() -> Vec<PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap();
    let mut out = Vec::new();
    for dir in [
        root.join("docs/examples"),
        root.join("crates/turing-machine/tests/golden"),
        root.join("crates/turing-machine/src/stdlib"),
    ] {
        collect(&dir, &mut out);
    }
    out.sort();
    out
}

fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            collect(&p, out);
        } else if matches!(
            p.extension().and_then(|s| s.to_str()),
            Some("tmc") | Some("tma")
        ) {
            out.push(p);
        }
    }
}

/// Every plain (relocation) call site's caller and callee signatures.
///
/// **The callee is looked up ACROSS the unit and the stdlib**, not just
/// inside the object: every `call std::…` in the shipped corpus is an
/// EXTERNAL symbol, so a lookup confined to one object would report
/// nothing about the exact sites this sweep exists to measure. `extra`
/// is the stdlib object (and, for a multi-unit target, its siblings),
/// searched after the object itself — the linker's own first-wins order.
/// Reported, never asserted.
fn report(path: &Path, obj: &ObjectFile, extra: &[&ObjectFile], framed_call_opcode: Option<u8>) {
    let Some(sigs) = obj.signatures.as_ref() else {
        println!("{}: no signatures (nothing to compare)", path.display());
        return;
    };
    let mut resolved = 0usize;
    let mut unresolved = 0usize;
    for reloc in &obj.relocations {
        // A framed call's displacement half is emitted as a relocation
        // shaped exactly like a plain call's (docs/core.md (framed calls));
        // the opcode byte sits one before the operand hole. Skip it, the
        // same way a bound site with an explicit map is skipped — it goes
        // through the composition algebra, not the plain-site check.
        if reloc.offset > 0
            && framed_call_opcode == Some(obj.blobs[reloc.blob as usize][reloc.offset as usize - 1])
        {
            continue;
        }
        let caller: &RoutineSig = &sigs[reloc.blob as usize];
        let name = &obj.symbols[reloc.symbol as usize].name;
        let find = |o: &ObjectFile| -> Option<RoutineSig> {
            let blob = o.symbols.iter().find_map(|s| match s.def {
                mtc_core::formats::object::SymbolDef::Defined { blob }
                | mtc_core::formats::object::SymbolDef::Local { blob }
                    if s.name == *name =>
                {
                    Some(blob)
                }
                _ => None,
            })?;
            o.signatures.as_ref()?.get(blob as usize).cloned()
        };
        let Some(callee) = find(obj).or_else(|| extra.iter().find_map(|o| find(o))) else {
            unresolved += 1;
            println!(
                "UNRESOLVED       {}: blob {} -> `{name}` (no signature in the unit or \
                 the stdlib)",
                path.display(),
                reloc.blob
            );
            continue;
        };
        resolved += 1;
        let callee = &callee;
        let wider_tapes = callee.arity > caller.arity;
        let wider_alpha = callee
            .cardinalities
            .iter()
            .zip(&caller.cardinalities)
            .any(|(c, k)| c > k);
        let narrower_alpha = callee
            .cardinalities
            .iter()
            .zip(&caller.cardinalities)
            .any(|(c, k)| c < k);
        if wider_tapes || wider_alpha {
            println!(
                "ERROR-WOULD-FIRE {}: blob {} -> `{name}` caller {:?} callee {:?}",
                path.display(),
                reloc.blob,
                caller.cardinalities,
                callee.cardinalities
            );
        } else if narrower_alpha {
            println!(
                "WARN-WOULD-FIRE  {}: blob {} -> `{name}` caller {:?} callee {:?}",
                path.display(),
                reloc.blob,
                caller.cardinalities,
                callee.cardinalities
            );
        }
    }
    println!(
        "{}: {resolved} call site(s) compared, {unresolved} unresolved",
        path.display()
    );
}

#[test]
#[ignore = "measurement instrument; run explicitly with --ignored --nocapture"]
fn sweep_the_shipped_corpus() {
    // The embedded stdlib is where every `call std::…` in the corpus
    // resolves, so it is part of the comparison, not a separate concern.
    let stdlib = mtc_turing_machine::stdlib::object().clone();
    let framed_call_opcode = tm1_syntax().framed_call_opcode();
    let mut compiled: Vec<(PathBuf, ObjectFile)> = Vec::new();
    for path in corpus() {
        let src = fs::read_to_string(&path).expect("readable");
        let obj = if path.extension().and_then(|s| s.to_str()) == Some("tma") {
            match assemble(&src, false) {
                Ok(o) => o,
                Err(e) => {
                    println!("{}: does not assemble standalone ({e})", path.display());
                    continue;
                }
            }
        } else {
            match compile(&src, CompileOptions::default()) {
                Ok(out) => out.object,
                Err(e) => {
                    println!("{}: does not compile standalone ({e:?})", path.display());
                    continue;
                }
            }
        };
        compiled.push((path, obj));
    }
    // A multi-unit target's sibling sources resolve each other, so every
    // compiled unit is a candidate callee holder for every other one —
    // over-broad by design, since a false RESOLUTION here only widens what
    // the sweep reports, and a missed one hides a finding.
    for (path, obj) in &compiled {
        let mut extra: Vec<&ObjectFile> = compiled
            .iter()
            .filter(|(p, _)| p != path)
            .map(|(_, o)| o)
            .collect();
        extra.push(&stdlib);
        report(path, obj, &extra, framed_call_opcode);
    }
    println!("--- sweep complete ---");
}
