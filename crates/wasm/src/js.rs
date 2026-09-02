//! Rust ↔ JS value plumbing for the boundary: plain objects out, options
//! objects in. The only file besides `lib.rs` that names `js_sys`.

use js_sys::{Array, Object, Reflect, Uint8Array};
use wasm_bindgen::{JsCast, JsValue};

use crate::inner::diagnostics::{Diag, Severity};
use crate::inner::listing::Row;
use crate::inner::program::{SourceLoc, TapeLayout};
use crate::inner::session::{Cause, Event, Finished, Limits, OutcomeInfo, Seed, Snapshot, Stats};
use crate::inner::tapeblock::{TapeBlock, TapeBlockInput, TapeInput};

pub fn obj() -> Object {
    Object::new()
}

pub fn set(o: &Object, key: &str, value: impl Into<JsValue>) {
    // Reflect::set fails only on a frozen or non-object target; ours are fresh.
    Reflect::set(o, &JsValue::from_str(key), &value.into()).expect("fresh object is writable");
}

pub fn strings(items: &[String]) -> Array {
    items.iter().map(|s| JsValue::from_str(s)).collect()
}

pub fn u32s(items: &[u32]) -> Array {
    items.iter().map(|&n| JsValue::from_f64(n as f64)).collect()
}

pub fn diag(d: &Diag) -> JsValue {
    let o = obj();
    set(&o, "code", d.code.as_str());
    set(
        &o,
        "severity",
        match d.severity {
            Severity::Error => "error",
            Severity::Warning => "warning",
        },
    );
    set(&o, "from", d.from);
    set(&o, "to", d.to);
    set(&o, "message", d.message.as_str());
    if let Some(f) = &d.fix {
        let fix = obj();
        set(&fix, "description", f.description.as_str());
        set(
            &fix,
            "applicability",
            if f.machine_applicable {
                "machineApplicable"
            } else {
                "maybeIncorrect"
            },
        );
        let edits: Array = f
            .edits
            .iter()
            .map(|e| {
                let eo = obj();
                set(&eo, "from", e.from);
                set(&eo, "to", e.to);
                set(&eo, "replacement", e.replacement.as_str());
                JsValue::from(eo)
            })
            .collect();
        set(&fix, "edits", edits);
        set(&o, "fix", fix);
    }
    o.into()
}

pub fn diags(ds: &[Diag]) -> JsValue {
    ds.iter().map(diag).collect::<Array>().into()
}

pub fn layout(t: &TapeLayout) -> JsValue {
    let o = obj();
    set(&o, "name", t.name.as_str());
    set(&o, "glyphs", strings(&t.glyphs));
    o.into()
}

pub fn row(r: &Row) -> JsValue {
    let o = obj();
    set(&o, "addr", r.addr);
    set(&o, "bytes", r.bytes.as_str());
    set(&o, "mnemonic", r.mnemonic.as_str());
    set(&o, "operand", r.operand.as_str());
    set(
        &o,
        "function",
        r.function
            .as_deref()
            .map(JsValue::from_str)
            .unwrap_or(JsValue::NULL),
    );
    set(
        &o,
        "label",
        r.label
            .as_deref()
            .map(JsValue::from_str)
            .unwrap_or(JsValue::NULL),
    );
    o.into()
}

pub fn source_loc(l: &SourceLoc) -> JsValue {
    let o = obj();
    set(&o, "function", l.function.as_str());
    set(
        &o,
        "line",
        l.line.map(JsValue::from).unwrap_or(JsValue::NULL),
    );
    o.into()
}

pub fn stats(s: &Stats) -> JsValue {
    let o = obj();
    set(&o, "steps", s.steps as f64);
    set(&o, "coreTacts", s.core_tacts as f64);
    set(&o, "stallTacts", s.stall_tacts as f64);
    set(&o, "totalTacts", s.total_tacts as f64);
    o.into()
}

pub fn finished(f: &Finished) -> JsValue {
    let o = obj();
    let outcome = obj();
    match &f.outcome {
        OutcomeInfo::Stopped => set(&outcome, "kind", "stopped"),
        OutcomeInfo::Halted => set(&outcome, "kind", "halted"),
        OutcomeInfo::Trapped(t) => {
            set(&outcome, "kind", "trapped");
            let trap = obj();
            set(&trap, "kind", t.kind);
            set(
                &trap,
                "at",
                t.at.map(JsValue::from).unwrap_or(JsValue::UNDEFINED),
            );
            set(&trap, "detail", t.detail.as_str());
            set(&outcome, "trap", trap);
        }
    }
    set(&o, "outcome", outcome);
    set(&o, "stats", stats(&f.stats));
    set(&o, "ip", f.ip);
    set(&o, "stack", u32s(&f.stack));
    o.into()
}

pub fn event(e: &Event) -> JsValue {
    let o = obj();
    match e {
        Event::DeviceWait => set(&o, "kind", "deviceWait"),
        Event::BudgetSpent => set(&o, "kind", "budgetSpent"),
        Event::Paused(c) => {
            set(&o, "kind", "paused");
            match c {
                Cause::Step => set(&o, "cause", "step"),
                Cause::Brk => set(&o, "cause", "brk"),
                Cause::Manual => set(&o, "cause", "manual"),
                Cause::Breakpoint(a) => {
                    let bp = obj();
                    set(&bp, "breakpoint", *a);
                    set(&o, "cause", bp);
                }
                Cause::Trap(t) => {
                    let tr = obj();
                    set(&tr, "trap", t.kind);
                    set(&o, "cause", tr);
                }
            }
        }
        Event::Finished(f) => {
            set(&o, "kind", "finished");
            set(&o, "result", finished(f));
        }
    }
    o.into()
}

pub fn snapshot(s: &Snapshot) -> JsValue {
    let o = obj();
    set(&o, "band", s.band);
    set(&o, "name", s.name.as_str());
    set(&o, "glyphs", strings(&s.glyphs));
    set(&o, "origin", s.origin as f64);
    set(&o, "cells", Uint8Array::from(s.cells.as_slice()));
    set(&o, "head", s.head as f64);
    o.into()
}

pub fn tape_block(b: &TapeBlock) -> JsValue {
    let o = obj();
    set(&o, "alphabet", strings(&b.alphabet));
    let tapes: Array = b
        .tapes
        .iter()
        .map(|t| {
            let to = obj();
            set(&to, "origin", t.origin as f64);
            set(&to, "cells", Uint8Array::from(t.cells.as_slice()));
            set(&to, "head", t.head as f64);
            set(&to, "glyphs", strings(&t.glyphs));
            JsValue::from(to)
        })
        .collect();
    set(&o, "tapes", tapes);
    o.into()
}

pub fn seed(s: &Seed) -> JsValue {
    let o = obj();
    set(&o, "cells", Uint8Array::from(s.cells.as_slice()));
    set(&o, "head", s.head as f64);
    set(&o, "origin", s.origin as f64);
    o.into()
}

// ---- readers -----------------------------------------------------------

fn field(v: &JsValue, key: &str) -> Option<JsValue> {
    if v.is_undefined() || v.is_null() {
        return None;
    }
    Reflect::get(v, &JsValue::from_str(key))
        .ok()
        .filter(|x| !x.is_undefined() && !x.is_null())
}

pub fn string_list(v: &JsValue, key: &str) -> Vec<String> {
    field(v, key)
        .map(|arr| {
            Array::from(&arr)
                .iter()
                .filter_map(|x| x.as_string())
                .collect()
        })
        .unwrap_or_default()
}

/// `None` when the key is absent (or null); an error when it is present
/// but not an array of strings.
fn opt_string_list(v: &JsValue, key: &str, who: &str) -> Result<Option<Vec<String>>, String> {
    match field(v, key) {
        None => Ok(None),
        Some(arr) if Array::is_array(&arr) => Array::from(&arr)
            .iter()
            .map(|x| {
                x.as_string()
                    .ok_or_else(|| format!("{who}: `{key}` must be an array of strings"))
            })
            .collect::<Result<Vec<_>, _>>()
            .map(Some),
        Some(_) => Err(format!("{who}: `{key}` must be an array of strings")),
    }
}

pub fn number(v: &JsValue, key: &str) -> Option<f64> {
    field(v, key).and_then(|x| x.as_f64())
}

/// `cells` as a `Uint8Array` or a number array.
fn cells(v: &JsValue, who: &str) -> Result<Vec<u8>, String> {
    let cells_val = field(v, "cells").ok_or_else(|| format!("{who}: missing `cells`"))?;
    if cells_val.is_instance_of::<Uint8Array>() {
        Ok(Uint8Array::new(&cells_val).to_vec())
    } else if Array::is_array(&cells_val) {
        Array::from(&cells_val)
            .iter()
            .map(|x| {
                x.as_f64()
                    .map(|n| n as u8)
                    .ok_or_else(|| format!("{who}: non-numeric cell"))
            })
            .collect()
    } else {
        Err(format!(
            "{who}: `cells` must be a Uint8Array or a number array"
        ))
    }
}

/// `{ alphabet?, tapes: [{ cells, head?, origin?, glyphs? }] }` — the
/// decoded `TapeBlock` shape is accepted as it is, `glyphs` included.
pub fn tape_block_input(v: &JsValue) -> Result<TapeBlockInput, String> {
    if v.is_undefined() || v.is_null() {
        return Err("a tape block must be an object with `tapes`".to_string());
    }
    let alphabet = opt_string_list(v, "alphabet", "tape block")?;
    let tapes_val = field(v, "tapes").ok_or_else(|| "tape block: missing `tapes`".to_string())?;
    if !Array::is_array(&tapes_val) {
        return Err("tape block: `tapes` must be an array".to_string());
    }
    let tapes = Array::from(&tapes_val)
        .iter()
        .enumerate()
        .map(|(i, t)| {
            let who = format!("tape {i}");
            Ok(TapeInput {
                cells: cells(&t, &who)?,
                head: number(&t, "head").map(|n| n as i64).unwrap_or(0),
                origin: number(&t, "origin").map(|n| n as i64).unwrap_or(0),
                glyphs: opt_string_list(&t, "glyphs", &who)?,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok(TapeBlockInput { alphabet, tapes })
}

fn limit_field(v: &JsValue, key: &str) -> Result<Option<u64>, String> {
    match number(v, key) {
        None => Ok(None),
        Some(n) if !n.is_finite() || n < 0.0 => Err(format!("`{key}` must be a finite number ≥ 0")),
        Some(n) => Ok(Some(n as u64)),
    }
}

pub fn limits(v: &JsValue) -> Result<Limits, String> {
    Ok(Limits {
        max_steps: limit_field(v, "maxSteps")?,
        max_tacts: limit_field(v, "maxTacts")?,
    })
}

/// `seeds` is `undefined`, or an array of `{ cells, head?, origin? }` where
/// `cells` is a `Uint8Array` or a number array.
pub fn seeds(v: &JsValue) -> Result<Vec<Seed>, String> {
    if v.is_undefined() || v.is_null() {
        return Ok(Vec::new());
    }
    if !Array::is_array(v) {
        return Err("`seeds` must be an array of { cells, head?, origin? }".to_string());
    }
    Array::from(v)
        .iter()
        .enumerate()
        .map(|(i, s)| {
            Ok(Seed {
                cells: cells(&s, &format!("seed {i}"))?,
                head: number(&s, "head").map(|n| n as i64).unwrap_or(0),
                origin: number(&s, "origin").map(|n| n as i64).unwrap_or(0),
            })
        })
        .collect()
}
