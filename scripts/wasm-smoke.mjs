#!/usr/bin/env node
// End-to-end over the BUILT bundle (not the Rust crate): load the web-target
// glue from bytes, then for both languages check → format → build → run,
// verifying manifest checksums, the line table, and a size ceiling; then
// the assembly languages.
//
//   node scripts/wasm-smoke.mjs target/wasm-bundle/dist
import { createHash } from "node:crypto";
import { readFileSync, statSync } from "node:fs";
import { gzipSync } from "node:zlib";
import { join } from "node:path";
import { pathToFileURL } from "node:url";

const dist = process.argv[2];
if (!dist) { console.error("usage: wasm-smoke.mjs <dist dir>"); process.exit(2); }

let failures = 0;
function check(cond, msg) {
  if (cond) { console.log(`ok   ${msg}`); } else { failures++; console.log(`FAIL ${msg}`); }
}
function eq(a, b, msg) { check(JSON.stringify(a) === JSON.stringify(b), `${msg} (${JSON.stringify(a)} vs ${JSON.stringify(b)})`); }

// --- manifest and checksums --------------------------------------------
const manifest = JSON.parse(readFileSync(join(dist, "manifest.json"), "utf8"));
for (const [file, sha] of Object.entries(manifest.files)) {
  const actual = createHash("sha256").update(readFileSync(join(dist, file))).digest("hex");
  eq(actual, sha, `checksum ${file}`);
}
check(/^\d+\.\d+\.\d+(-[0-9A-Za-z.]+)?$/.test(manifest.crate_version), `crate_version ${manifest.crate_version}`);

// --- load --------------------------------------------------------------
const glue = await import(pathToFileURL(join(dist, "mtc_wasm.js")).href);
const wasmBytes = readFileSync(join(dist, "mtc_wasm_bg.wasm"));
await glue.default({ module_or_path: wasmBytes });
const { Toolchain } = glue;

const PMC_INC = "main() {\n    1: right(2);\n    2: check(1, 3);\n    3: mark(4);\n    4: left(5);\n    5: check(4, 6);\n    6: right(!);\n}\n";
const PMC_UNUSED_LABEL = "namespace api {\nhelper() {\n5: right;\n}\n}\nmain() { @api::helper(); }\n";
const TMC_REPLACE_B = "alphabet ab { '_', 'a', 'b' }\n\nmachine {\n  tape main: ab;\n\n  entry state scan {\n    ['b'] -> write ['a'] move [>] goto scan;\n    ['a'] ->             move [>] goto scan;\n    ['_'] -> stop;\n  }\n}\n";
const TMC_UNUSED_ALPHABET = "alphabet ab { '_', 'a', 'b' }\nalphabet spare { '_', 'x' }\n\nmachine {\n  tape main: ab;\n\n  entry state scan {\n    ['b'] -> write ['a'] move [>] goto scan;\n    ['a'] ->             move [>] goto scan;\n    ['_'] -> stop;\n  }\n}\n";
// Only 'a' has a rule; seeding 'b' traps on entry with no applicable transition.
const TMC_NO_TRANSITION = "alphabet ab { '_', 'a', 'b' }\n\nmachine {\n  tape main: ab;\n\n  entry state scan {\n    ['a'] -> move [>] goto scan;\n  }\n}\n";

function runToEnd(session) {
  for (;;) {
    const ev = session.pump();
    if (ev.kind === "finished") return ev.result;
    if (ev.kind !== "budgetSpent") throw new Error(`unexpected ${JSON.stringify(ev)}`);
  }
}

// --- check / format ----------------------------------------------------
check(Toolchain.check("pmc", PMC_UNUSED_LABEL).some(d => d.code === "unused-label"), "pmc check finds unused-label");
check(Toolchain.check("tmc", TMC_UNUSED_ALPHABET).some(d => d.code === "unused-alphabet"), "tmc check finds unused-alphabet");
const fatal = Toolchain.check("pmc", "main() { nope");
eq(fatal.length, 1, "pmc fatal is one diagnostic"); eq(fatal[0]?.severity, "error", "…of severity error");
for (const [lang, src] of [["pmc", PMC_UNUSED_LABEL], ["tmc", TMC_UNUSED_ALPHABET]]) {
  const once = Toolchain.format(lang, src);
  check(once.ok, `${lang} format ok`);
  const twice = Toolchain.format(lang, once.text);
  eq(twice.text, once.text, `${lang} format idempotent`);
}
let threw = false;
try { Toolchain.check("cobol", "x"); } catch { threw = true; }
check(threw, "unknown lang throws");

// --- build / run: pmc --------------------------------------------------
{
  const r = Toolchain.build("pmc", PMC_INC, { optLevel: 1 });
  check(r.ok, "pmc builds");
  const p = r.program;
  eq(p.tapes(), [{ name: "tape", glyphs: [" ", "*"] }], "pmc tape layout");
  check(p.listing().length > 0 && p.listing()[0].addr === 0, "pmc listing starts at 0");
  check(p.disassembly().includes("main"), "pmc disassembly names main");
  check(p.bytes().length > 0, "pmc MX bytes");
  check(JSON.parse(p.mapJson()).functions.length > 0, "pmc map json");
  const s = p.session([{ cells: [1, 1, 1], head: 0 }]);
  const result = runToEnd(s);
  eq(result.outcome.kind, "stopped", "pmc stopped");
  const snap = s.snapshot(0);
  eq(Array.from(snap.cells.slice(0, 4)), [1, 1, 1, 1], "pmc final tape");
  eq(snap.head, 0, "pmc head back on the first mark");
  s.stop();
  let stopped = false; try { s.pump(); } catch { stopped = true; } check(stopped, "pmc use after stop throws");
  p.free();
}

// --- build / run: tmc --------------------------------------------------
{
  const r = Toolchain.build("tmc", TMC_REPLACE_B, { optLevel: 0 });
  check(r.ok, "tmc builds");
  const p = r.program;
  eq(p.tapes(), [{ name: "main", glyphs: ["_", "a", "b"] }], "tmc tape layout");
  const line = 8; // the ['a'] rule
  const addr = p.addressForLine(line);
  check(addr !== undefined && addr !== null, `tmc line ${line} has an address`);
  eq(p.lineOf(addr)?.line, line, "tmc lineOf(addressForLine(n)) is n");
  const s = p.session([{ cells: new Uint8Array([2, 2, 2]) }], { maxSteps: 1000 });
  s.pause();
  eq(s.pump().kind, "paused", "tmc manual pause fires");
  check(
    typeof s.ip === "number" && typeof s.mf === "boolean" &&
    typeof s.fr === "number" && typeof s.depth === "number",
    "getters have their types",
  );
  eq(s.stack(), [], "empty stack at start");
  const bpAddr = p.listing()[1].addr;
  s.addBreakpoint(bpAddr);
  const ev = s.pump();
  eq(ev.kind, "paused", "tmc breakpoint pause fires");
  eq(ev.cause?.breakpoint, bpAddr, "…at the planted address");
  s.removeBreakpoint(bpAddr);
  const result = runToEnd(s);
  eq(result.outcome.kind, "stopped", "tmc stopped");
  const snap = s.snapshot(0);
  eq(Array.from(snap.cells.slice(0, 3)), [1, 1, 1], "tmc final tape");
  eq(snap.head, 3, "tmc head on the first blank");
  check(s.snapshots().length === 1, "snapshots() has one band");
  check(s.finished()?.outcome.kind === "stopped", "finished() repeats the result");
  const stats = s.stop();
  check(stats.steps > 0, "tmc stats carry steps");
  let threwOnFree = false;
  try { s.free(); } catch { threwOnFree = true; }
  check(!threwOnFree, "free after stop does not throw");
  p.free();
}
{
  const r = Toolchain.build("tmc", TMC_REPLACE_B);
  const s = r.program.session([{ cells: [2, 2, 2, 2, 2, 2, 2, 2] }], { maxSteps: 2 });
  const result = runToEnd(s);
  eq(result.outcome.kind, "trapped", "tmc step limit traps");
  eq(result.outcome.trap?.kind, "step-limit", "…as step-limit");
  r.program.free();
}
{
  const r = Toolchain.build("tmc", TMC_NO_TRANSITION);
  const s = r.program.session([{ cells: [2] }]);
  const result = runToEnd(s);
  eq(result.outcome.kind, "trapped", "tmc no-transition traps");
  eq(result.outcome.trap?.kind, "no-transition", "…as no-transition");
  check(typeof result.outcome.trap?.at === "number", "no-transition carries at");
  r.program.free();
}
{
  const r = Toolchain.build("tmc", "alphabet a { '_' }\nmachine {");
  eq(r.ok, false, "tmc fatal is not ok");
  eq(r.diagnostics.length, 1, "…with one diagnostic");
}

// --- pump budget / limits / seed validation (A2/A3/A4) ------------------
{
  const r = Toolchain.build("tmc", TMC_REPLACE_B);
  const s = r.program.session([{ cells: [2, 2, 2] }]);
  let threwOnZero = false;
  try { s.pump(0); } catch { threwOnZero = true; }
  check(threwOnZero, "pump(0) throws instead of spinning forever");
  const ev = s.pump(2 ** 32);
  eq(ev.kind, "finished", "pump(2**32) runs the whole program instead of truncating to 0");
  r.program.free();
}
{
  const r = Toolchain.build("tmc", TMC_REPLACE_B);
  let threw = false;
  try { r.program.session([{ cells: [2, 2, 2] }], { maxSteps: -1 }); } catch { threw = true; }
  check(threw, "negative maxSteps throws instead of saturating to an instant trap");
  r.program.free();
}
{
  const r = Toolchain.build("tmc", TMC_REPLACE_B);
  let threw = false;
  try { r.program.session([{ cells: {} }]); } catch { threw = true; }
  check(threw, "a non-array `cells` throws instead of seeding a blank band");
  r.program.free();
}

// --- assembly (#113) ---------------------------------------------------
{
  // `pmt compile -S` of PMC_INC: right to the blank, mark, left to the blank, right, stop.
  const PMA_INC = ".func main\nL1:\n        rgt\n        jm      L1\n        wr      1\nL4:\n        lft\n        jm      L4\n        rgt\n        stp\n";
  const r = Toolchain.build("pma", PMA_INC, undefined);
  check(r.ok, "pma builds");
  const p = r.program;
  eq(p.tapes(), [{ name: "tape", glyphs: [" ", "*"] }], "pma tape layout");
  const addr = p.addressForLine(3);
  check(addr != null, "pma line 3 (`rgt`) has an address");
  eq(p.lineOf(addr), { file: "user", function: "main", line: 3 }, "pma lineOf is the physical line");
  const s = p.session([{ cells: [1, 1, 1] }], undefined);
  const result = runToEnd(s);
  eq(result.outcome.kind, "stopped", "pma stopped");
  eq(Array.from(s.snapshot(0).cells.slice(0, 4)), [1, 1, 1, 1], "pma final tape");
  s.stop();
  p.free();

  const bad = Toolchain.build("pma", ".func main\n        bogus\n", undefined);
  eq(bad.ok, false, "pma refusal is not ok");
  eq(bad.diagnostics[0]?.code, "unknown-mnemonic", "…with the assembler's code");
  check(Toolchain.check("pma", ".func main\nL1:\n        stp\n", undefined).some(d => d.code === "unused-label"), "pma check runs the asm lint");
  eq(Toolchain.format("pma", ".func main\n  L1:  rgt\n jm L1\n").text, ".func main\nL1:     rgt\n        jm      L1\n", "pma format is the canonical grid");

  // A compiled program's disassembly reassembles to the same image.
  const compiled = Toolchain.build("tmc", TMC_REPLACE_B, undefined).program;
  const again = Toolchain.build("tma", compiled.disassembly(), undefined);
  check(again.ok, "tma builds from disassembly");
  eq(Array.from(again.program.bytes()), Array.from(compiled.bytes()), "…to the same image");
  eq(again.program.tapes(), [{ name: "tape0", glyphs: ["0", "1", "2"] }], "tma bands are image-labelled");
  again.program.free();
  compiled.free();
}

// --- size ceiling ------------------------------------------------------
const raw = statSync(join(dist, "mtc_wasm_bg.wasm")).size;
const gz = gzipSync(wasmBytes, { level: 9 }).length;
console.log(`size raw=${raw} gzip=${gz}`);
check(gz < 1_000_000, "gzipped wasm under the 1 MB ceiling");

if (failures) { console.error(`${failures} check(s) failed`); process.exit(1); }
console.log("smoke: all checks passed");
