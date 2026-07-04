//! The sans-I/O processor core (spec §4): a pure transition function
//! from bus responses to bus requests. Owns registers and the in-flight
//! instruction; performs no I/O; knows no opcodes (that's the Arch).

use super::arch::{Arch, MicroOp, Operand, OperandKind};
use super::bus::{BusRequest, BusResponse, CoreEvent};
use super::trap::Trap;

enum Phase {
    FetchOpcode,
    FetchOperand {
        opcode: u8,
        kind: OperandKind,
        buf: Vec<u8>,
    },
    /// Task 4 stub: lowering is stored, then Step is emitted without
    /// executing. Task 5 replaces this with real micro-op execution.
    Retire {
        #[allow(dead_code)] // consumed starting Task 5
        ops: Vec<MicroOp>,
    },
    Done,
}

pub struct Core<'a> {
    arch: &'a dyn Arch,
    ip: u32,
    instr_start: u32,
    mf: bool,
    phase: Phase,
}

impl<'a> Core<'a> {
    pub fn new(arch: &'a dyn Arch, entry: u32) -> Self {
        Self {
            arch,
            ip: entry,
            instr_start: entry,
            mf: false,
            phase: Phase::FetchOpcode,
        }
    }

    pub fn ip(&self) -> u32 {
        self.ip
    }

    pub fn mf(&self) -> bool {
        self.mf
    }

    /// The driver latches initial MF from the tape before the first resume.
    pub fn set_mf(&mut self, mf: bool) {
        self.mf = mf;
    }

    pub fn start(&mut self) -> CoreEvent {
        self.instr_start = self.ip;
        self.phase = Phase::FetchOpcode;
        self.code_read()
    }

    fn code_read(&self) -> CoreEvent {
        CoreEvent::Request(BusRequest::CodeRead { addr: self.ip })
    }

    fn trap(&mut self, trap: Trap) -> CoreEvent {
        self.phase = Phase::Done;
        CoreEvent::Trapped(trap)
    }

    pub fn resume(&mut self, resp: BusResponse) -> CoreEvent {
        match std::mem::replace(&mut self.phase, Phase::Done) {
            Phase::FetchOpcode => self.on_opcode(resp),
            Phase::FetchOperand { opcode, kind, buf } => {
                self.on_operand_byte(opcode, kind, buf, resp)
            }
            Phase::Retire { .. } => {
                // Task 4 stub: driver acknowledged Step with Ok — fetch next.
                self.start()
            }
            Phase::Done => self.trap(Trap::CodeOutOfBounds { at: self.ip }),
        }
    }

    fn on_opcode(&mut self, resp: BusResponse) -> CoreEvent {
        let byte = match resp {
            BusResponse::Byte(b) => b,
            BusResponse::OutOfCode => return self.trap(Trap::CodeOutOfBounds { at: self.ip }),
            _ => return self.trap(Trap::CodeOutOfBounds { at: self.ip }),
        };
        let Some(kind) = self.arch.operand_kind(byte) else {
            return self.trap(Trap::InvalidOpcode {
                opcode: byte,
                at: self.ip,
            });
        };
        self.ip += 1;
        match kind {
            OperandKind::None => self.finish_fetch(byte, Operand::None),
            _ => {
                self.phase = Phase::FetchOperand {
                    opcode: byte,
                    kind,
                    buf: Vec::new(),
                };
                self.code_read()
            }
        }
    }

    fn on_operand_byte(
        &mut self,
        opcode: u8,
        kind: OperandKind,
        mut buf: Vec<u8>,
        resp: BusResponse,
    ) -> CoreEvent {
        let byte = match resp {
            BusResponse::Byte(b) => b,
            _ => return self.trap(Trap::CodeOutOfBounds { at: self.ip }),
        };
        buf.push(byte);
        self.ip += 1;
        let complete = match kind {
            OperandKind::None => true, // unreachable by construction
            OperandKind::RelI8 => buf.len() == 1,
            OperandKind::RelI32 => buf.len() == 4,
            OperandKind::SymbolVec => byte & 0x80 != 0,
        };
        if !complete {
            self.phase = Phase::FetchOperand { opcode, kind, buf };
            return self.code_read();
        }
        let operand = match kind {
            OperandKind::None => Operand::None,
            OperandKind::RelI8 => Operand::I8(buf[0] as i8),
            OperandKind::RelI32 => Operand::I32(i32::from_le_bytes(buf[..4].try_into().unwrap())),
            OperandKind::SymbolVec => {
                Operand::Symbols(buf.iter().map(|b| u32::from(b & 0x7F)).collect())
            }
        };
        self.finish_fetch(opcode, operand)
    }

    fn finish_fetch(&mut self, opcode: u8, operand: Operand) -> CoreEvent {
        match self.arch.lower(opcode, &operand) {
            Ok(ops) => {
                // Task 4 stub: store lowering, retire immediately.
                self.phase = Phase::Retire { ops };
                CoreEvent::Step
            }
            Err(_) => self.trap(Trap::BadOperand {
                at: self.instr_start,
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vm::arch::test_arch::TestArch;
    use crate::vm::bus::{BusRequest as Rq, BusResponse as Rs, CoreEvent as Ev};
    use crate::vm::trap::Trap;

    /// Drive the core with a scripted byte image; panics if the core asks
    /// for anything but code during fetch. Returns the first non-Request
    /// event and the addresses the core fetched.
    fn run_fetch(code: &[u8], entry: u32) -> (Ev, Vec<u32>) {
        let arch = TestArch;
        let mut core = Core::new(&arch, entry);
        let mut fetched = Vec::new();
        let mut ev = core.start();
        loop {
            match ev {
                Ev::Request(Rq::CodeRead { addr }) => {
                    fetched.push(addr);
                    let resp = match code.get(addr as usize) {
                        Some(&b) => Rs::Byte(b),
                        None => Rs::OutOfCode,
                    };
                    ev = core.resume(resp);
                }
                other => return (other, fetched),
            }
        }
    }

    #[test]
    fn fetches_single_byte_instruction() {
        // 0x01 = nop (no operand) — Task 4 stub yields Step after fetch
        let (ev, fetched) = run_fetch(&[0x01], 0);
        assert_eq!(ev, Ev::Step);
        assert_eq!(fetched, vec![0]);
    }

    #[test]
    fn fetches_rel8_operand() {
        // 0x08 = jmp rel8; operand byte follows
        let (ev, fetched) = run_fetch(&[0x08, 0x05], 0);
        assert_eq!(ev, Ev::Step);
        assert_eq!(fetched, vec![0, 1]);
    }

    #[test]
    fn fetches_rel32_operand_little_endian() {
        let (ev, fetched) = run_fetch(&[0x09, 0x01, 0x00, 0x00, 0x00], 0);
        assert_eq!(ev, Ev::Step);
        assert_eq!(fetched, vec![0, 1, 2, 3, 4]);
    }

    #[test]
    fn fetches_symbol_vec_until_high_bit() {
        // 0x07 = wr(vec); TestArch requires exactly one element;
        // 0x81 = payload 1 with high bit (last element)
        let (ev, fetched) = run_fetch(&[0x07, 0x81], 0);
        assert_eq!(ev, Ev::Step);
        assert_eq!(fetched, vec![0, 1]);
    }

    #[test]
    fn multi_element_symbol_vec_is_collected_then_arch_rejects() {
        // 0x01 (no high bit, more follow) then 0x82 (last) → 2 elements;
        // TestArch's wr wants exactly 1 → BadOperand
        let (ev, fetched) = run_fetch(&[0x07, 0x01, 0x82], 0);
        assert!(matches!(ev, Ev::Trapped(Trap::BadOperand { .. })));
        assert_eq!(fetched, vec![0, 1, 2]);
    }

    #[test]
    fn invalid_opcode_traps_with_location() {
        let (ev, _) = run_fetch(&[0x55], 0);
        assert_eq!(
            ev,
            Ev::Trapped(Trap::InvalidOpcode {
                opcode: 0x55,
                at: 0
            })
        );
    }

    #[test]
    fn out_of_code_during_fetch_traps() {
        let (ev, _) = run_fetch(&[], 0);
        assert!(matches!(ev, Ev::Trapped(Trap::CodeOutOfBounds { at: 0 })));
        // operand runs off the end:
        let (ev2, _) = run_fetch(&[0x09, 0x01], 0);
        assert!(matches!(ev2, Ev::Trapped(Trap::CodeOutOfBounds { .. })));
    }

    #[test]
    fn entry_offset_is_respected() {
        let (_, fetched) = run_fetch(&[0x00, 0x00, 0x01], 2);
        assert_eq!(fetched, vec![2]);
    }
}
