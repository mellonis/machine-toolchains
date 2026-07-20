//! tail-call (state-graph form): a call in tail position — a rule whose
//! transition is `call target … then return` — becomes a `TailCall` (codegen
//! emits `jmp @<target>`, a relocated external jump) instead of `call` + a
//! resume that immediately returns. The callee's own `return` pops the frame
//! the ORIGINAL caller pushed, so the intermediate stack slot and return trip
//! are saved. Part of the `-O1` pipeline (optimizer/mod.rs).
//!
//! Two guards, both load-bearing:
//!
//! * **Routine worlds only.** A machine world's terminators are `stp`/`hlt`; it
//!   cannot carry a `return` at all (the front end rejects `return` outside a
//!   routine — crate::compiler `ReturnOutsideRoutine`), so no `call … then
//!   return` can ever appear there. The `kind == Routine` gate is therefore
//!   structural, matching the `.pmc` pass's "never in `main`" rule (whose
//!   return is `stp`, which a callee `ret` would underflow).
//!
//! * **Bindless calls only.** A BOUND call (`binding` non-empty) rides the
//!   frames stack discipline: the paired `call`/`ret` pushes and restores the
//!   frame register FR (docs/tmt/isa.md (the frames execution profile)).
//!   Tail-transferring with a bare `jmp` would skip that push, so the
//!   callee's `ret` would restore the WRONG FR and desync the stack. Bound
//!   tail calls are excluded in this phase; only a bindless `call` (an empty
//!   binding — a plain call the linker resolves) is safe to turn into a jump.
//!
//! The `debugger` flag is NOT motion here: the rewrite changes the SAME rule's
//! transition and leaves its `brk` in place, so the pause point stays at this
//! exact code head — codegen still emits `brk` before the `jmp`. No brk-barrier
//! interaction, so a debugger-bearing tail call rewrites like any other.

use crate::ir::{IrThen, IrTransition, IrWorld, IrWorldKind};

pub fn run(w: &mut IrWorld) -> u32 {
    // Machine worlds cannot hold a `return` (structural — see the module doc),
    // so there is nothing to convert; only routines qualify.
    if w.kind != IrWorldKind::Routine {
        return 0;
    }
    let mut changes = 0u32;
    for st in &mut w.states {
        for r in &mut st.rules {
            // A bindless `call target … then return`: rewrite in place to a
            // `TailCall`. Bound calls (non-empty binding) are excluded.
            let is_tail = matches!(
                &r.transition,
                IrTransition::CallThen { binding, then, .. }
                    if binding.is_empty() && matches!(then, IrThen::Return)
            );
            if is_tail {
                let IrTransition::CallThen { target, .. } = &r.transition else {
                    unreachable!("just matched a bindless CallThen with a return then");
                };
                r.transition = IrTransition::TailCall {
                    target: target.clone(),
                };
                changes += 1;
            }
        }
    }
    changes
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compiler::analyze;
    use crate::expand::expand;
    use crate::ir::{IrProgram, lower, validate_world};

    fn ir_of(src: &str) -> IrProgram {
        let a = analyze(src).unwrap_or_else(|e| panic!("analyze: {e}"));
        let ex = expand(&a.resolved).unwrap_or_else(|e| panic!("expand: {e}"));
        lower(&ex, &a.resolved)
            .unwrap_or_else(|e| panic!("lower: {e}"))
            .0
    }

    fn world<'a>(ir: &'a IrProgram, name: &str) -> &'a IrWorld {
        ir.worlds.iter().find(|w| w.name == name).expect("world")
    }

    #[test]
    fn bindless_call_then_return_becomes_a_tail_call() {
        // `caller::go` bindlessly calls an EXTERNAL routine (`lib::ext`, no
        // in-unit signature ⇒ empty binding) and immediately returns — the
        // canonical tail position. It rewrites to a TailCall to `lib::ext`. (An
        // in-unit call to a tape-bearing routine would REQUIRE binding args and
        // so never be bindless — hence the external target.)
        let mut ir = ir_of(
            "alphabet ab { '_', 'a' }
use lib::ext;
routine caller(tape t: ab) {
  entry state go { [*] -> call ext() then return; }
}
machine { tape t: ab; entry state m { [*] -> stop; } }",
        );
        let idx = ir
            .worlds
            .iter()
            .position(|w| w.name == "caller")
            .expect("caller world");
        let changes = run(&mut ir.worlds[idx]);
        assert_eq!(changes, 1, "the one bindless tail call converts");
        let caller = world(&ir, "caller");
        assert_eq!(
            caller.states[caller.entry as usize].rules[0].transition,
            IrTransition::TailCall {
                target: "lib::ext".into()
            }
        );
        validate_world(caller).unwrap();
    }

    #[test]
    fn a_debugger_bearing_tail_call_still_converts_keeping_the_brk() {
        // The rewrite is not motion — the `brk` stays on the same rule, so the
        // pause point is preserved. The transition still becomes a TailCall.
        let mut ir = ir_of(
            "alphabet ab { '_', 'a' }
use lib::ext;
routine caller(tape t: ab) {
  entry state go { [*] -> debugger call ext() then return; }
}
machine { tape t: ab; entry state m { [*] -> stop; } }",
        );
        let idx = ir.worlds.iter().position(|w| w.name == "caller").unwrap();
        assert_eq!(run(&mut ir.worlds[idx]), 1);
        let caller = world(&ir, "caller");
        let rule = &caller.states[caller.entry as usize].rules[0];
        assert!(rule.debugger, "the brk survives the rewrite");
        assert_eq!(
            rule.transition,
            IrTransition::TailCall {
                target: "lib::ext".into()
            }
        );
        validate_world(caller).unwrap();
    }

    #[test]
    fn a_bound_call_is_left_alone() {
        // A bound tail call carries a binding, so the frames FR discipline
        // forbids tail-transferring it — the CallThen stays untouched. `helper`
        // is defined in-unit, so `t = t` binds its tape (a non-empty binding).
        let mut ir = ir_of(
            "alphabet ab { '_', 'a', 'b' }
routine helper(tape t: ab) { entry state s { [*] -> return; } }
routine caller(tape t: ab) {
  entry state go { [*] -> call helper(t = t with map { 'a'->'b' }) then return; }
}
machine { tape t: ab; entry state m { [*] -> stop; } }",
        );
        let idx = ir.worlds.iter().position(|w| w.name == "caller").unwrap();
        assert_eq!(run(&mut ir.worlds[idx]), 0, "a bound call does not convert");
        let caller = world(&ir, "caller");
        assert!(matches!(
            caller.states[caller.entry as usize].rules[0].transition,
            IrTransition::CallThen { .. }
        ));
    }

    #[test]
    fn a_call_that_does_not_return_is_left_alone() {
        // `then goto` / `then stop` are not tail positions — only `then
        // return` is. Here the (bindless external) call resumes at a local state.
        let mut ir = ir_of(
            "alphabet ab { '_', 'a' }
use lib::ext;
routine caller(tape t: ab) {
  entry state go   { [*] -> call ext() then after; }
  state after      { [*] -> return; }
}
machine { tape t: ab; entry state m { [*] -> stop; } }",
        );
        let idx = ir.worlds.iter().position(|w| w.name == "caller").unwrap();
        assert_eq!(
            run(&mut ir.worlds[idx]),
            0,
            "a resuming call does not convert"
        );
    }

    #[test]
    fn a_machine_world_is_never_touched() {
        // A machine cannot carry `return` (the front end rejects it), so it can
        // hold no tail call; the pass returns 0 on the machine world regardless
        // of its (bindless external) calls.
        let mut ir = ir_of(
            "alphabet ab { '_', 'a' }
use lib::ext;
machine {
  tape t: ab;
  entry state m { [*] -> call ext() then done; }
  state done { [*] -> stop; }
}",
        );
        let m = ir
            .worlds
            .iter_mut()
            .find(|w| w.kind == IrWorldKind::Machine)
            .expect("the machine world");
        assert_eq!(run(m), 0, "the machine world is structurally exempt");
    }
}
