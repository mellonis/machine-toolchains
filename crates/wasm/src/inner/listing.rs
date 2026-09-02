//! The debugger code view as data: one row per instruction, function and
//! label names from the map, jump targets resolved to `function` or
//! `function.label` the way the text listing does.

use mtc_core::asm::listing_parts;
use mtc_core::linker::MapFile;

use super::Lang;
use super::program::Program;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub addr: u32,
    /// Space-separated hex bytes, one pair per byte.
    pub bytes: String,
    pub mnemonic: String,
    pub operand: String,
    /// The function whose range contains `addr`.
    pub function: Option<String>,
    /// A label sitting exactly at `addr` (a function start is its name).
    pub label: Option<String>,
}

fn function_at(map: &MapFile, addr: u32) -> Option<&str> {
    map.functions
        .iter()
        .find(|f| f.start <= addr && addr < f.end)
        .map(|f| f.name.as_str())
}

fn label_at(map: &MapFile, addr: u32) -> Option<String> {
    for f in &map.functions {
        if f.start == addr {
            return Some(f.name.clone());
        }
        if let Some((label, _)) = f.labels.iter().find(|(_, a)| *a == addr) {
            return Some(format!("{}.{label}", f.name));
        }
    }
    None
}

pub fn rows(program: &Program) -> Vec<Row> {
    let syntax = match program.lang {
        Lang::Pmc => mtc_post_machine::asm::pm1_syntax(),
        Lang::Tmc => mtc_turing_machine::asm::tm1_syntax(),
    };
    let map = &program.map;
    let code = &program.exe.code;
    let resolve = |target: u32| label_at(map, target);
    let mut out = Vec::new();
    let mut addr = 0u32;
    while (addr as usize) < code.len() {
        let parts = listing_parts(&syntax, code, addr, &resolve);
        // A decoder that cannot advance (an undecodable trailing byte) still
        // gets a row, and the walk moves by one so it always terminates.
        let len = parts.len.max(1);
        out.push(Row {
            addr,
            bytes: parts.bytes_hex,
            mnemonic: parts.mnemonic,
            operand: parts.operand,
            function: function_at(map, addr).map(str::to_string),
            label: label_at(map, addr),
        });
        addr += len;
    }
    out
}
