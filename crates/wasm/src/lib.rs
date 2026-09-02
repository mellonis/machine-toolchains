//! Browser binding for the machine toolchains. `inner/` is plain Rust over
//! the three crates' public APIs and is what the native tests exercise;
//! this file and `js.rs` are the wasm-bindgen layer over it: three classes,
//! plain JS objects for every data type, and the TypeScript declarations of
//! those objects. Reference: `docs/wasm.md (the object model)`.

#[doc(hidden)]
pub mod inner;
mod js;

use wasm_bindgen::prelude::*;

use inner::Lang;
use inner::diagnostics::{CheckError, CheckOptions};
use inner::session::SessionError;

#[wasm_bindgen(typescript_custom_section)]
const TYPES: &str = r#"
export type Lang = "pmc" | "tmc";
export interface CheckOptions { allow?: string[]; warn?: string[] }
export interface BuildOptions { optLevel?: 0 | 1 }
export type FormatResult = { ok: true; text: string } | { ok: false; error: Diagnostic };
export type BuildResult =
  | { ok: true; program: Program; diagnostics: Diagnostic[] }
  | { ok: false; diagnostics: Diagnostic[] };
export interface TapeLayout { name: string; glyphs: string[] }
export interface ListingRow { addr: number; bytes: string; mnemonic: string; operand: string;
                              function: string | null; label: string | null }
export interface SourceLoc { function: string; line: number | null }
export interface Seed { cells: Uint8Array | number[]; head?: number; origin?: number }
export interface Limits { maxSteps?: number; maxTacts?: number }
export type PumpEvent =
  | { kind: "deviceWait" }
  | { kind: "budgetSpent" }
  | { kind: "paused"; cause: "step" | "brk" | "manual" | { breakpoint: number } | { trap: string } }
  | { kind: "finished"; result: RunResult };
export interface RunResult { outcome: Outcome; stats: RunStats; ip: number; stack: number[] }
export type Outcome = { kind: "stopped" } | { kind: "halted" } | { kind: "trapped"; trap: TrapInfo };
export interface TrapInfo { kind: string; at?: number; detail: string }
export interface RunStats { steps: number; coreTacts: number; stallTacts: number; totalTacts: number }
export interface TapeSnapshot { band: number; name: string; glyphs: string[];
                                origin: number; cells: Uint8Array; head: number }
export interface Diagnostic { code: string; severity: "error" | "warning";
                              from: number; to: number; message: string; fix?: Fix }
export interface Fix { description: string; applicability: "machineApplicable" | "maybeIncorrect";
                       edits: Edit[] }
export interface Edit { from: number; to: number; replacement: string }
"#;

fn lang(s: &str) -> Result<Lang, JsError> {
    Lang::parse(s)
        .ok_or_else(|| JsError::new(&format!("unknown lang `{s}`; expected \"pmc\" or \"tmc\"")))
}

fn session_err(e: SessionError) -> JsError {
    JsError::new(&match e {
        SessionError::Stopped => "session already stopped".to_string(),
        SessionError::TooManySeeds { given, bands } => format!("{given} seeds for {bands} band(s)"),
        SessionError::BadSeed { band, index, width } => {
            format!("band {band}: cell index {index} outside its alphabet of {width}")
        }
        SessionError::NoSuchBand(b) => format!("no band {b}"),
        SessionError::Load(m) => format!("load failed: {m}"),
    })
}

/// Stateless entry points: the lint channel, the formatter, the build.
#[wasm_bindgen]
pub struct Toolchain;

#[wasm_bindgen]
impl Toolchain {
    #[wasm_bindgen(unchecked_return_type = "Diagnostic[]")]
    pub fn check(
        lang_name: &str,
        source: &str,
        #[wasm_bindgen(unchecked_param_type = "CheckOptions | undefined")] opts: JsValue,
    ) -> Result<JsValue, JsError> {
        let options = CheckOptions {
            allow: js::string_list(&opts, "allow"),
            warn: js::string_list(&opts, "warn"),
        };
        match inner::diagnostics::check(lang(lang_name)?, source, &options) {
            Ok(ds) => Ok(js::diags(&ds)),
            Err(CheckError::UnknownAllowCode(c)) => {
                Err(JsError::new(&format!("unknown lint rule `{c}`")))
            }
        }
    }

    #[wasm_bindgen(unchecked_return_type = "FormatResult")]
    pub fn format(lang_name: &str, source: &str) -> Result<JsValue, JsError> {
        let o = js::obj();
        match inner::diagnostics::format(lang(lang_name)?, source) {
            Ok(text) => {
                js::set(&o, "ok", true);
                js::set(&o, "text", text.as_str());
            }
            Err(d) => {
                js::set(&o, "ok", false);
                js::set(&o, "error", js::diag(&d));
            }
        }
        Ok(o.into())
    }

    #[wasm_bindgen(unchecked_return_type = "BuildResult")]
    pub fn build(
        lang_name: &str,
        source: &str,
        #[wasm_bindgen(unchecked_param_type = "BuildOptions | undefined")] opts: JsValue,
    ) -> Result<JsValue, JsError> {
        let opt_level: u8 = match js::number(&opts, "optLevel") {
            Some(0.0) => 0,
            _ => 1,
        };
        let o = js::obj();
        match inner::program::build(lang(lang_name)?, source, opt_level) {
            Ok((program, warnings)) => {
                js::set(&o, "ok", true);
                js::set(&o, "program", Program { inner: program });
                js::set(&o, "diagnostics", js::diags(&warnings));
            }
            Err(d) => {
                js::set(&o, "ok", false);
                js::set(&o, "diagnostics", js::diags(&[d]));
            }
        }
        Ok(o.into())
    }
}

/// A linked program: the executable, its map, and everything the browser
/// asks of them.
#[wasm_bindgen]
pub struct Program {
    inner: inner::program::Program,
}

#[wasm_bindgen]
impl Program {
    #[wasm_bindgen(unchecked_return_type = "TapeLayout[]")]
    pub fn tapes(&self) -> JsValue {
        self.inner
            .tapes()
            .iter()
            .map(js::layout)
            .collect::<js_sys::Array>()
            .into()
    }

    #[wasm_bindgen(unchecked_return_type = "ListingRow[]")]
    pub fn listing(&self) -> JsValue {
        inner::listing::rows(&self.inner)
            .iter()
            .map(js::row)
            .collect::<js_sys::Array>()
            .into()
    }

    #[wasm_bindgen(js_name = lineOf, unchecked_return_type = "SourceLoc | null")]
    pub fn line_of(&self, addr: u32) -> JsValue {
        self.inner
            .line_of(addr)
            .map(|l| js::source_loc(&l))
            .unwrap_or(JsValue::NULL)
    }

    #[wasm_bindgen(js_name = addressForLine)]
    pub fn address_for_line(&self, line: u32) -> Option<u32> {
        self.inner.address_for_line(line)
    }

    pub fn disassembly(&self) -> String {
        self.inner.disassembly()
    }

    pub fn bytes(&self) -> Vec<u8> {
        self.inner.bytes()
    }

    #[wasm_bindgen(js_name = mapJson)]
    pub fn map_json(&self) -> String {
        self.inner.map_json()
    }

    pub fn session(
        &self,
        #[wasm_bindgen(unchecked_param_type = "Seed[] | undefined")] seeds: JsValue,
        #[wasm_bindgen(unchecked_param_type = "Limits | undefined")] limits: JsValue,
    ) -> Result<Session, JsError> {
        let seeds = js::seeds(&seeds).map_err(|m| JsError::new(&m))?;
        let limits = js::limits(&limits).map_err(|m| JsError::new(&m))?;
        let inner =
            inner::session::Session::new(&self.inner, &seeds, limits).map_err(session_err)?;
        Ok(Session { inner })
    }
}

/// A pumped run. The embedder owns the loop: every call to `pump` retires
/// instructions until a budget runs out, a pause fires, or the program ends.
#[wasm_bindgen]
pub struct Session {
    inner: inner::session::Session,
}

#[wasm_bindgen]
impl Session {
    #[wasm_bindgen(unchecked_return_type = "PumpEvent")]
    pub fn pump(&mut self, budget: Option<f64>) -> Result<JsValue, JsError> {
        let budget = match budget {
            None => None,
            Some(n) if !n.is_finite() || n < 1.0 => {
                return Err(JsError::new(
                    "budget must be a finite number ≥ 1, or undefined to run to the next pause",
                ));
            }
            Some(n) => Some(n.min(u64::MAX as f64) as u64),
        };
        self.inner
            .pump(budget)
            .map(|e| js::event(&e))
            .map_err(session_err)
    }

    pub fn pause(&mut self) -> Result<(), JsError> {
        self.inner.pause().map_err(session_err)
    }

    #[wasm_bindgen(js_name = addBreakpoint)]
    pub fn add_breakpoint(&mut self, addr: u32) -> Result<(), JsError> {
        self.inner.add_breakpoint(addr).map_err(session_err)
    }

    #[wasm_bindgen(js_name = removeBreakpoint)]
    pub fn remove_breakpoint(&mut self, addr: u32) -> Result<(), JsError> {
        self.inner.remove_breakpoint(addr).map_err(session_err)
    }

    #[wasm_bindgen(unchecked_return_type = "TapeSnapshot")]
    pub fn snapshot(&self, band: u32) -> Result<JsValue, JsError> {
        self.inner
            .snapshot(band)
            .map(|s| js::snapshot(&s))
            .map_err(session_err)
    }

    #[wasm_bindgen(unchecked_return_type = "TapeSnapshot[]")]
    pub fn snapshots(&self) -> Result<JsValue, JsError> {
        self.inner
            .snapshots()
            .map(|v| v.iter().map(js::snapshot).collect::<js_sys::Array>().into())
            .map_err(session_err)
    }

    #[wasm_bindgen(getter)]
    pub fn ip(&self) -> Result<u32, JsError> {
        self.inner.ip().map_err(session_err)
    }

    #[wasm_bindgen(getter)]
    pub fn mf(&self) -> Result<bool, JsError> {
        self.inner.mf().map_err(session_err)
    }

    #[wasm_bindgen(getter)]
    pub fn fr(&self) -> Result<u32, JsError> {
        self.inner.fr().map_err(session_err)
    }

    #[wasm_bindgen(getter)]
    pub fn depth(&self) -> Result<u32, JsError> {
        self.inner.depth().map(|d| d as u32).map_err(session_err)
    }

    #[wasm_bindgen(unchecked_return_type = "number[]")]
    pub fn stack(&self) -> Result<JsValue, JsError> {
        self.inner
            .stack()
            .map(|s| js::u32s(&s).into())
            .map_err(session_err)
    }

    #[wasm_bindgen(unchecked_return_type = "RunStats")]
    pub fn stats(&self) -> Result<JsValue, JsError> {
        self.inner
            .stats()
            .map(|s| js::stats(&s))
            .map_err(session_err)
    }

    #[wasm_bindgen(unchecked_return_type = "RunResult | null")]
    pub fn finished(&self) -> Result<JsValue, JsError> {
        self.inner
            .finished()
            .map(|f| f.map(|f| js::finished(&f)).unwrap_or(JsValue::NULL))
            .map_err(session_err)
    }

    /// Ends the run and returns its statistics; every later call throws.
    #[wasm_bindgen(unchecked_return_type = "RunStats")]
    pub fn stop(&mut self) -> Result<JsValue, JsError> {
        self.inner
            .stop()
            .map(|s| js::stats(&s))
            .map_err(session_err)
    }
}
