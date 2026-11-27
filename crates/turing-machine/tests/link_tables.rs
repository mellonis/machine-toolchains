//! A plain table reference whose hole happens to sit five bytes after a
//! `call.m` opcode byte links as the plain table it is. The linker tells a
//! table fixup's kind from the code bytes: the table-reference opcode right
//! before the hole marks a plain table, and a framed-call opcode five bytes
//! back marks a frame half only when that first test fails (docs/core.md
//! (relaxation)).
//!
//! The collision here is the natural one: a state's `mtc T; djmp D` pair
//! where T's table offset is 0x13 — the `call.m` opcode — so the byte five
//! back from the `djmp` hole is T's offset. Reading D as a frame descriptor
//! would refuse it for a "physical tape" past the machine arity, on a
//! program that makes no call at all.

use mtc_core::formats::object::ObjectFile;
use mtc_core::linker::LinkOptions;
use mtc_turing_machine::arch::opcodes::{CALL_M, DJMP, MTC};
use mtc_turing_machine::asm::link;
use mtc_turing_machine::compiler::{CompileOptions, compile};
use mtc_turing_machine::optimizer::OptLevel;

/// `pad` unconditional states push the two table-bearing states' code
/// past 0x300, so the first target in `t`'s dispatch table has a second
/// byte of 3 — a "physical tape" at the three-tape machine's arity. They
/// emit no tables, so the table offsets stay 0, 9, 19 (= 0x13): `s`'s
/// match and dispatch tables, then `t`'s match table. `control` adds a
/// third row to `s`, moving `t`'s match table off 0x13.
fn source(pad: usize, control: bool) -> String {
    let mut s = String::from(
        "alphabet ab { '_', 'a', 'b' }\n\nmachine {\n  tape x: ab;\n  tape y: ab;\n  tape z: ab;\n\n",
    );
    for i in 0..pad {
        let kw = if i == 0 { "entry state" } else { "state" };
        let w = if i % 2 == 0 { "'a'" } else { "'b'" };
        s += &format!(
            "  {kw} p{i} {{ [*, *, *] -> write [{w}, -, -] move [>, ., .] goto p{}; }}\n",
            i + 1
        );
    }
    s += &format!("  state p{pad} {{ [*, *, *] -> move [<, ., .] goto s; }}\n");
    s += "  state s {\n";
    if control {
        s += "    ['_', '_', '_'] -> halt;\n";
    }
    s += "\
    ['a', 'a', 'a'] -> goto t;
    ['b', 'b', 'b'] -> stop;
  }
  state t {
    ['a', 'a', 'a'] -> write ['b', 'b', 'b'] goto s;
    ['b', 'b', 'b'] -> stop;
  }
}
";
    s
}

fn object(src: &str, level: OptLevel) -> ObjectFile {
    compile(
        src,
        CompileOptions {
            opt_level: level,
            ..Default::default()
        },
    )
    .unwrap_or_else(|e| panic!("the fixture must compile: {e}"))
    .object
}

/// Whether some `mtc` whose table offset is the `call.m` opcode is
/// immediately followed by a `djmp` — the byte pattern that puts a
/// framed-call opcode five bytes before a plain table hole.
fn has_collision(obj: &ObjectFile) -> bool {
    let pattern = [MTC, CALL_M, 0, 0, 0, DJMP];
    obj.blobs
        .iter()
        .any(|b| b.windows(pattern.len()).any(|w| w == pattern))
}

#[test]
fn a_call_m_opcode_byte_before_a_dispatch_hole_is_not_a_frame() {
    for level in [OptLevel::O0, OptLevel::O1] {
        let obj = object(&source(120, false), level);
        // A drift in codegen or layout that loses the collision fails here
        // instead of passing without discriminating.
        assert!(
            has_collision(&obj),
            "{level:?}: the fixture no longer places a call.m opcode byte five bytes before a djmp hole"
        );
        let out = link(&[obj], &[], LinkOptions::default())
            .unwrap_or_else(|e| panic!("{level:?}: a call-free program must link: {e}"));
        assert_eq!(out.executable.profile, 0, "{level:?}: a frameless image");
    }
}

#[test]
fn the_control_without_the_collision_links() {
    for level in [OptLevel::O0, OptLevel::O1] {
        let obj = object(&source(120, true), level);
        assert!(
            !has_collision(&obj),
            "{level:?}: the control has no collision"
        );
        link(&[obj], &[], LinkOptions::default())
            .unwrap_or_else(|e| panic!("{level:?}: the control must link: {e}"));
    }
}
