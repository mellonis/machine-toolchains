//! The sans-I/O processor core (docs/isa.md): a pure transition function
//! from bus responses to bus requests. Owns registers and the in-flight
//! instruction; performs no I/O; knows no opcodes (that's the Arch).

use super::arch::{Arch, MicroOp, Operand, OperandKind};
use super::bus::{BusRequest, BusResponse, CoreEvent};
use super::frame::{FrameDescriptor, FrameStep, FrameWalk};
use super::table::{DispatchWalk, MatchWalk};
use super::trap::{RaisedTrapKind, Trap};

enum Phase {
    FetchOpcode,
    FetchOperand {
        opcode: u8,
        kind: OperandKind,
        buf: Vec<u8>,
    },
    Execute {
        ops: std::collections::VecDeque<MicroOp>,
        pending: Pending,
    },
    StepAck,
    Done,
}

/// What the in-flight bus request was for.
enum Pending {
    None,
    Move,
    Write,
    /// `vdev` is the VIRTUAL device the read was issued on — the settle
    /// side translates the physical symbol back through its rmap when a
    /// frame is active (and is a plain pass-through otherwise).
    Latch {
        match_index: u32,
        vdev: u8,
    },
    ReadSlot {
        slot: u8,
        vdev: u8,
    },
    EntCheck {
        target: u32,
    },
    Push {
        target: u32,
    },
    Pop,
    Match(MatchWalk),
    Dispatch(DispatchWalk),
    /// A framed call's entry-marker verification (as `EntCheck`, keeping
    /// the frame offset to activate on a successful push).
    EntCheckFrame {
        target: u32,
        frame: u32,
    },
    /// A framed call's return-address push: on Ok the FR pair pushes and
    /// the descriptor load begins.
    PushFrame {
        target: u32,
        frame: u32,
    },
    /// An in-flight frame-descriptor load; `then_ip` is where execution
    /// continues once the cache is filled.
    FrameLoad {
        walk: FrameWalk,
        then_ip: u32,
    },
    /// A paired return-stack pop (frames profile): `exit_to` is the retx
    /// exit address (the popped return address is discarded), or `None`
    /// for a plain ret returning to the popped address.
    PopPair {
        exit_to: Option<u32>,
    },
}

pub struct Core<'a> {
    arch: &'a dyn Arch,
    ip: u32,
    instr_start: u32,
    mr: u32,
    tr: [u32; 16],
    tr_len: u8,
    phase: Phase,
    brk_pending: bool,
    /// Number of physical tape devices visible to `ReadAll` under the
    /// identity frame.
    device_count: u8,
    /// Whether the frames execution profile is active. Off (the base
    /// profile), `Call`/`Ret` behave exactly as always and the frame
    /// instructions trap `ProfileViolation`.
    frames_enabled: bool,
    /// The frame register: 0 = identity (no translation); a non-zero
    /// value is the active descriptor's table-section offset + 1.
    fr: u32,
    /// Decoded descriptor for the active frame (`Some` iff FR != 0 and
    /// the descriptor load completed).
    frame_cache: Option<FrameDescriptor>,
    /// The FR half of the (return address, FR) pair stack. A
    /// core-internal parallel stack synced with the bus stack: pushed
    /// only when the bus acknowledges a push, popped only when the bus
    /// yields a value — so it never drifts from the driver's stack.
    fr_stack: Vec<u32>,
}

impl<'a> Core<'a> {
    pub fn new(arch: &'a dyn Arch, entry: u32) -> Self {
        Self {
            arch,
            ip: entry,
            instr_start: entry,
            mr: 0,
            tr: [0; 16],
            tr_len: 0,
            phase: Phase::FetchOpcode,
            brk_pending: false,
            device_count: 1,
            frames_enabled: false,
            fr: 0,
            frame_cache: None,
            fr_stack: Vec::new(),
        }
    }

    /// Builder: how many physical tape devices the machine exposes
    /// (default 1). Only `ReadAll` under the identity frame consumes it.
    pub fn with_device_count(mut self, n: u8) -> Self {
        self.device_count = n;
        self
    }

    /// Builder: enable the frames execution profile — `Call`/`Ret` keep
    /// the FR pair stack in step, and `CallFrame`/`RetX` become legal.
    pub fn with_frames(mut self) -> Self {
        self.frames_enabled = true;
        self
    }

    pub fn ip(&self) -> u32 {
        self.ip
    }

    /// The frame register (0 = identity frame).
    pub fn fr(&self) -> u32 {
        self.fr
    }

    /// Address of the instruction the core is executing (or last worked
    /// on) — the faulting address on traps, unlike `ip()` which has
    /// advanced past fetched bytes.
    pub fn instr_start(&self) -> u32 {
        self.instr_start
    }

    pub fn mf(&self) -> bool {
        self.mr != 0
    }

    /// The driver latches initial MF from the tape before the first resume.
    pub fn set_mf(&mut self, mf: bool) {
        self.mr = u32::from(mf);
    }

    /// The match register (docs/isa.md (registers)): 0 = no row matched.
    /// MF is formally `MR != 0`; 1-bit-flag architectures only ever write 0/1 here.
    pub fn mr(&self) -> u32 {
        self.mr
    }

    pub fn set_mr(&mut self, mr: u32) {
        self.mr = mr;
    }

    /// The tuple register: symbols latched by `Read` micro-ops this
    /// instruction sequence. `MatchTable` compares rows against this prefix.
    pub fn tr(&self) -> &[u32] {
        &self.tr[..usize::from(self.tr_len)]
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
            Phase::Execute { ops, pending } => {
                self.phase = Phase::Execute { ops, pending };
                self.step_execute(resp)
            }
            Phase::StepAck => self.start(),
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
            OperandKind::RelI8 | OperandKind::Imm8 => buf.len() == 1,
            OperandKind::RelI32 | OperandKind::TableRef => buf.len() == 4,
            OperandKind::FramedCall => buf.len() == 8,
            OperandKind::SymbolVec | OperandKind::MoveVec => byte & 0x80 != 0,
        };
        if !complete {
            self.phase = Phase::FetchOperand { opcode, kind, buf };
            return self.code_read();
        }
        let operand = match kind {
            OperandKind::None => Operand::None,
            OperandKind::RelI8 => Operand::I8(buf[0] as i8),
            OperandKind::RelI32 => Operand::I32(i32::from_le_bytes(buf[..4].try_into().unwrap())),
            OperandKind::TableRef => {
                Operand::Table(u32::from_le_bytes(buf[..4].try_into().unwrap()))
            }
            OperandKind::Imm8 => Operand::Imm(buf[0]),
            // Displacement (bytes 0..4) then frame table offset (4..8).
            OperandKind::FramedCall => Operand::FramedCall {
                rel: i32::from_le_bytes(buf[..4].try_into().unwrap()),
                table: u32::from_le_bytes(buf[4..8].try_into().unwrap()),
            },
            // MoveVec shares SymbolVec's compact walk AND its decoded
            // shape: both fetch to `Operand::Symbols`, so an arch's
            // lowerings handle every vector operand uniformly — the two
            // kinds differ only in assembly vocabulary and rendering,
            // which never reach the core.
            OperandKind::SymbolVec | OperandKind::MoveVec => {
                Operand::Symbols(buf.iter().map(|b| u32::from(b & 0x7F)).collect())
            }
        };
        self.finish_fetch(opcode, operand)
    }

    fn finish_fetch(&mut self, opcode: u8, operand: Operand) -> CoreEvent {
        match self.arch.lower(opcode, &operand) {
            Ok(ops) => {
                self.phase = Phase::Execute {
                    ops: ops.into(),
                    pending: Pending::None,
                };
                self.step_execute(BusResponse::Ok)
            }
            Err(_) => self.trap(Trap::BadOperand {
                at: self.instr_start,
            }),
        }
    }

    fn step_execute(&mut self, resp: BusResponse) -> CoreEvent {
        let Phase::Execute { mut ops, pending } = std::mem::replace(&mut self.phase, Phase::Done)
        else {
            unreachable!("step_execute outside Execute phase");
        };

        // 1. Settle the in-flight request, if any.
        // A response of the wrong type is a driver protocol violation; it is rendered as CodeOutOfBounds — drivers must conform.
        match pending {
            Pending::None => {}
            Pending::Move | Pending::Write => match resp {
                BusResponse::Ok => {}
                BusResponse::Fault(fault) => return self.trap(Trap::Device { fault }),
                _ => return self.trap(Trap::CodeOutOfBounds { at: self.ip }),
            },
            Pending::Latch { match_index, vdev } => match resp {
                BusResponse::Symbol(s) => match self.virt_symbol(vdev, s) {
                    Ok(v) => self.mr = u32::from(v == match_index),
                    Err(trap) => return self.trap(trap),
                },
                BusResponse::Fault(fault) => return self.trap(Trap::Device { fault }),
                _ => return self.trap(Trap::CodeOutOfBounds { at: self.ip }),
            },
            Pending::ReadSlot { slot, vdev } => match resp {
                BusResponse::Symbol(s) => match self.virt_symbol(vdev, s) {
                    Ok(v) => {
                        self.tr[usize::from(slot)] = v;
                        self.tr_len = self.tr_len.max(slot + 1);
                    }
                    Err(trap) => return self.trap(trap),
                },
                BusResponse::Fault(fault) => return self.trap(Trap::Device { fault }),
                _ => return self.trap(Trap::CodeOutOfBounds { at: self.ip }),
            },
            Pending::EntCheck { target } => match resp {
                BusResponse::Byte(b) if self.arch.is_entry_marker(b) => {
                    self.phase = Phase::Execute {
                        ops,
                        pending: Pending::Push { target },
                    };
                    return CoreEvent::Request(BusRequest::StackPush { value: self.ip });
                }
                BusResponse::Byte(_) | BusResponse::OutOfCode => {
                    return self.trap(Trap::CallTargetNotEntry { target });
                }
                _ => return self.trap(Trap::CodeOutOfBounds { at: self.ip }),
            },
            Pending::Push { target } => match resp {
                BusResponse::Ok => {
                    if self.frames_enabled {
                        // Uniform pair discipline: a plain call pushes
                        // the (unchanged) FR beside the return address.
                        self.fr_stack.push(self.fr);
                    }
                    self.ip = target;
                }
                BusResponse::StackFull => return self.trap(Trap::StackOverflow),
                _ => return self.trap(Trap::CodeOutOfBounds { at: self.ip }),
            },
            Pending::Pop => match resp {
                BusResponse::Value(v) => self.ip = v,
                BusResponse::StackEmpty => return self.trap(Trap::StackUnderflow),
                _ => return self.trap(Trap::CodeOutOfBounds { at: self.ip }),
            },
            Pending::EntCheckFrame { target, frame } => match resp {
                BusResponse::Byte(b) if self.arch.is_entry_marker(b) => {
                    self.phase = Phase::Execute {
                        ops,
                        pending: Pending::PushFrame { target, frame },
                    };
                    return CoreEvent::Request(BusRequest::StackPush { value: self.ip });
                }
                BusResponse::Byte(_) | BusResponse::OutOfCode => {
                    return self.trap(Trap::CallTargetNotEntry { target });
                }
                _ => return self.trap(Trap::CodeOutOfBounds { at: self.ip }),
            },
            Pending::PushFrame { target, frame } => match resp {
                BusResponse::Ok => {
                    // Sync discipline: FR mutations ride only successful
                    // bus responses — a refused push leaves FR untouched.
                    self.fr_stack.push(self.fr);
                    self.fr = frame + 1;
                    return self.start_frame_load(ops, frame, target);
                }
                BusResponse::StackFull => return self.trap(Trap::StackOverflow),
                _ => return self.trap(Trap::CodeOutOfBounds { at: self.ip }),
            },
            Pending::FrameLoad { mut walk, then_ip } => match resp {
                BusResponse::Byte(b) => match walk.feed(Some(b)) {
                    FrameStep::NeedByte(addr) => {
                        self.phase = Phase::Execute {
                            ops,
                            pending: Pending::FrameLoad { walk, then_ip },
                        };
                        return CoreEvent::Request(BusRequest::FrameRead { addr });
                    }
                    FrameStep::Done(desc) => {
                        self.frame_cache = Some(desc);
                        self.ip = then_ip;
                    }
                    FrameStep::Malformed => {
                        return self.trap(Trap::TableOutOfBounds {
                            at: self.instr_start,
                        });
                    }
                },
                BusResponse::OutOfTable => {
                    return self.trap(Trap::TableOutOfBounds {
                        at: self.instr_start,
                    });
                }
                _ => return self.trap(Trap::CodeOutOfBounds { at: self.ip }),
            },
            Pending::PopPair { exit_to } => match resp {
                BusResponse::Value(v) => {
                    // A conforming driver keeps the two stacks paired —
                    // both sides push and pop only together.
                    let restored = self.fr_stack.pop().unwrap_or(0);
                    let then_ip = exit_to.unwrap_or(v);
                    // retx always reloads a non-identity restored frame;
                    // a plain ret skips the reload when the frame did
                    // not change under the call.
                    let reload = restored != 0 && (exit_to.is_some() || restored != self.fr);
                    if restored == 0 {
                        self.frame_cache = None;
                    }
                    self.fr = restored;
                    if reload {
                        return self.start_frame_load(ops, restored - 1, then_ip);
                    }
                    self.ip = then_ip;
                }
                BusResponse::StackEmpty => return self.trap(Trap::StackUnderflow),
                _ => return self.trap(Trap::CodeOutOfBounds { at: self.ip }),
            },
            Pending::Match(mut walk) => match resp {
                BusResponse::Byte(b) => {
                    match walk.feed(Some(b), &self.tr[..usize::from(self.tr_len)]) {
                        crate::vm::table::WalkStep::NeedByte(addr) => {
                            self.phase = Phase::Execute {
                                ops,
                                pending: Pending::Match(walk),
                            };
                            return CoreEvent::Request(BusRequest::TableRead { addr });
                        }
                        crate::vm::table::WalkStep::Done(mr) => self.mr = mr,
                        crate::vm::table::WalkStep::Malformed => {
                            return self.trap(Trap::BadOperand {
                                at: self.instr_start,
                            });
                        }
                    }
                }
                BusResponse::OutOfTable => {
                    return self.trap(Trap::TableOutOfBounds {
                        at: self.instr_start,
                    });
                }
                _ => return self.trap(Trap::CodeOutOfBounds { at: self.ip }),
            },
            Pending::Dispatch(mut walk) => match resp {
                BusResponse::Byte(b) => match walk.feed(Some(b)) {
                    crate::vm::table::DispatchStep::NeedByte(addr) => {
                        self.phase = Phase::Execute {
                            ops,
                            pending: Pending::Dispatch(walk),
                        };
                        return CoreEvent::Request(BusRequest::TableRead { addr });
                    }
                    crate::vm::table::DispatchStep::Done(target) => self.ip = target,
                    crate::vm::table::DispatchStep::OutOfRange => {
                        return self.trap(Trap::DispatchOutOfRange {
                            at: self.instr_start,
                        });
                    }
                },
                BusResponse::OutOfTable => {
                    return self.trap(Trap::TableOutOfBounds {
                        at: self.instr_start,
                    });
                }
                _ => return self.trap(Trap::CodeOutOfBounds { at: self.ip }),
            },
        }

        // 2. Issue the next micro-op.
        while let Some(op) = ops.pop_front() {
            let (request, pending) = match op {
                MicroOp::Nop => continue,
                MicroOp::Brk => {
                    self.brk_pending = true;
                    continue;
                }
                MicroOp::Stop => {
                    self.phase = Phase::Done;
                    return CoreEvent::Stopped;
                }
                MicroOp::Halt => {
                    self.phase = Phase::Done;
                    return CoreEvent::Halted;
                }
                MicroOp::Raise { kind } => {
                    let at = self.instr_start;
                    return self.trap(match kind {
                        RaisedTrapKind::UnmappedRead => Trap::UnmappedRead { at },
                        RaisedTrapKind::UnmappedWrite => Trap::UnmappedWrite { at },
                    });
                }
                MicroOp::MoveLeft { dev } => match self.phys_dev(dev) {
                    Ok(dev) => (BusRequest::DeviceMoveLeft { dev }, Pending::Move),
                    Err(trap) => return self.trap(trap),
                },
                MicroOp::MoveRight { dev } => match self.phys_dev(dev) {
                    Ok(dev) => (BusRequest::DeviceMoveRight { dev }, Pending::Move),
                    Err(trap) => return self.trap(trap),
                },
                MicroOp::Write { dev, index } => {
                    // Both halves translate at issue: the device index
                    // and (through the frame's wmap) the symbol.
                    let translated = self
                        .phys_dev(dev)
                        .and_then(|p| Ok((p, self.phys_symbol(dev, index)?)));
                    match translated {
                        Ok((dev, index)) => {
                            (BusRequest::DeviceWrite { dev, index }, Pending::Write)
                        }
                        Err(trap) => return self.trap(trap),
                    }
                }
                MicroOp::LatchMatch(match_index) => match self.phys_dev(0) {
                    Ok(dev) => (
                        BusRequest::DeviceRead { dev },
                        Pending::Latch {
                            match_index,
                            vdev: 0,
                        },
                    ),
                    Err(trap) => return self.trap(trap),
                },
                MicroOp::Read { dev, slot } => {
                    if slot >= 16 {
                        return self.trap(Trap::BadOperand {
                            at: self.instr_start,
                        });
                    }
                    match self.phys_dev(dev) {
                        Ok(phys) => (
                            BusRequest::DeviceRead { dev: phys },
                            Pending::ReadSlot { slot, vdev: dev },
                        ),
                        Err(trap) => return self.trap(trap),
                    }
                }
                MicroOp::JumpRel(off) => {
                    match self.jump_target(off) {
                        Ok(t) => self.ip = t,
                        Err(trap) => return self.trap(trap),
                    }
                    continue;
                }
                MicroOp::JumpRelIf { off, when_match } => {
                    if (self.mr != 0) == when_match {
                        match self.jump_target(off) {
                            Ok(t) => self.ip = t,
                            Err(trap) => return self.trap(trap),
                        }
                    }
                    continue;
                }
                MicroOp::Call(off) => match self.jump_target(off) {
                    Ok(target) => (
                        BusRequest::CodeRead { addr: target },
                        Pending::EntCheck { target },
                    ),
                    Err(trap) => return self.trap(trap),
                },
                MicroOp::Ret => {
                    if self.frames_enabled {
                        // Pair discipline: pop the FR half too, restore
                        // it, and reload the descriptor if it changed.
                        (BusRequest::StackPop, Pending::PopPair { exit_to: None })
                    } else {
                        (BusRequest::StackPop, Pending::Pop)
                    }
                }
                MicroOp::MatchTable { table } => {
                    let mut walk = MatchWalk::new(table);
                    match walk.feed(None, self.tr()) {
                        crate::vm::table::WalkStep::NeedByte(addr) => {
                            (BusRequest::TableRead { addr }, Pending::Match(walk))
                        }
                        _ => {
                            return self.trap(Trap::BadOperand {
                                at: self.instr_start,
                            });
                        }
                    }
                }
                MicroOp::ReadAll => {
                    // Width: the active frame's arity, or every physical
                    // tape under the identity frame. Expanding into plain
                    // `Read` micro-ops reuses their translation, fault,
                    // and settle paths verbatim.
                    let width = match self.active_frame() {
                        Some(desc) => desc.entries.len() as u8,
                        None => self.device_count,
                    };
                    self.tr_len = 0;
                    for i in (0..width).rev() {
                        ops.push_front(MicroOp::Read { dev: i, slot: i });
                    }
                    continue;
                }
                MicroOp::CallFrame { .. } | MicroOp::RetX { .. } if !self.frames_enabled => {
                    // Base profile: the frame instructions are outside
                    // the execution profile.
                    return self.trap(Trap::ProfileViolation {
                        at: self.instr_start,
                    });
                }
                MicroOp::CallFrame { rel, frame } => match self.jump_target(rel) {
                    Ok(target) => (
                        BusRequest::CodeRead { addr: target },
                        Pending::EntCheckFrame { target, frame },
                    ),
                    Err(trap) => return self.trap(trap),
                },
                MicroOp::RetX { k } => {
                    // The exit address is read from the CURRENT cache
                    // before anything pops — the exit vector belongs to
                    // the frame being left.
                    let Some(&exit) = self
                        .active_frame()
                        .and_then(|desc| desc.exits.get(usize::from(k)))
                    else {
                        return self.trap(Trap::ExitOutOfRange {
                            at: self.instr_start,
                        });
                    };
                    (
                        BusRequest::StackPop,
                        Pending::PopPair {
                            exit_to: Some(exit),
                        },
                    )
                }
                MicroOp::DispatchJump { table } => {
                    if self.mr == 0 {
                        return self.trap(Trap::NoTransition {
                            at: self.instr_start,
                        });
                    }
                    let mut walk = DispatchWalk::new(table, self.mr);
                    match walk.feed(None) {
                        crate::vm::table::DispatchStep::NeedByte(addr) => {
                            (BusRequest::TableRead { addr }, Pending::Dispatch(walk))
                        }
                        _ => {
                            return self.trap(Trap::BadOperand {
                                at: self.instr_start,
                            });
                        }
                    }
                }
            };
            self.phase = Phase::Execute { ops, pending };
            return CoreEvent::Request(request);
        }

        // 3. Instruction retired.
        self.phase = Phase::StepAck;
        if std::mem::take(&mut self.brk_pending) {
            CoreEvent::Break
        } else {
            CoreEvent::Step
        }
    }

    /// The active frame's descriptor, `None` under the identity frame.
    /// Invariant: a non-zero FR always has a decoded descriptor — FR
    /// only becomes non-zero together with a completed descriptor load,
    /// and a failed load traps before anything else executes.
    fn active_frame(&self) -> Option<&FrameDescriptor> {
        if self.fr == 0 {
            None
        } else {
            Some(
                self.frame_cache
                    .as_ref()
                    .expect("non-zero FR always has a decoded descriptor"),
            )
        }
    }

    /// Translate a virtual device index to the physical one. Identity
    /// frame: a pass-through. Active frame: the index must be inside the
    /// frame's arity.
    fn phys_dev(&self, dev: u8) -> Result<u8, Trap> {
        match self.active_frame() {
            None => Ok(dev),
            Some(desc) => match desc.entries.get(usize::from(dev)) {
                Some(entry) => Ok(entry.phys),
                None => Err(Trap::BadOperand {
                    at: self.instr_start,
                }),
            },
        }
    }

    /// Read settle: physical symbol → virtual through the frame's rmap
    /// (identity when the map is empty; a hole or an index past the map
    /// is an unmapped read).
    fn virt_symbol(&self, vdev: u8, s: u32) -> Result<u32, Trap> {
        let Some(desc) = self.active_frame() else {
            return Ok(s);
        };
        let rmap = &desc.entries[usize::from(vdev)].rmap;
        if rmap.is_empty() {
            return Ok(s);
        }
        match rmap.get(s as usize) {
            Some(&v) if v != 0xFFFF => Ok(u32::from(v)),
            _ => Err(Trap::UnmappedRead {
                at: self.instr_start,
            }),
        }
    }

    /// Write issue: virtual symbol → physical through the frame's wmap
    /// (identity when the map is empty; a hole or an index past the map
    /// is an unmapped write).
    fn phys_symbol(&self, vdev: u8, v: u32) -> Result<u32, Trap> {
        let Some(desc) = self.active_frame() else {
            return Ok(v);
        };
        let wmap = &desc.entries[usize::from(vdev)].wmap;
        if wmap.is_empty() {
            return Ok(v);
        }
        match wmap.get(v as usize) {
            Some(&p) if p != 0xFFFF => Ok(u32::from(p)),
            _ => Err(Trap::UnmappedWrite {
                at: self.instr_start,
            }),
        }
    }

    /// Begin a descriptor load: FR is already set; the walk fills the
    /// cache and execution continues at `then_ip` once it completes.
    fn start_frame_load(
        &mut self,
        ops: std::collections::VecDeque<MicroOp>,
        offset: u32,
        then_ip: u32,
    ) -> CoreEvent {
        let mut walk = FrameWalk::new(offset);
        let FrameStep::NeedByte(addr) = walk.feed(None) else {
            unreachable!("a fresh frame walk always needs its first byte");
        };
        self.phase = Phase::Execute {
            ops,
            pending: Pending::FrameLoad { walk, then_ip },
        };
        CoreEvent::Request(BusRequest::FrameRead { addr })
    }

    /// Operands are relative to the END of the instruction (docs/isa.md);
    /// at execute time `self.ip` == instr_end (fetch advanced it).
    fn jump_target(&self, off: i32) -> Result<u32, Trap> {
        let target = i64::from(self.ip) + i64::from(off);
        u32::try_from(target).map_err(|_| Trap::CodeOutOfBounds {
            at: self.instr_start,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vm::arch::test_arch::TestArch;
    use crate::vm::bus::{BusRequest as Rq, BusResponse as Rs, CoreEvent as Ev};
    use crate::vm::trap::Trap;

    /// Drive the core with a scripted byte image, servicing `CodeRead`
    /// requests only. Returns the first non-`CodeRead` event (it does not
    /// panic — a request for anything else during fetch is simply handed
    /// back to the caller) and the addresses the core fetched.
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
        // 0x01 = nop (no operand): its one micro-op needs no device
        // interaction, so fetch completes straight to Step.
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
        // 0x81 = payload 1 with high bit (last element).
        // Fetch completes and execution begins immediately, so the
        // observable outcome is the Write micro-op's device request (index 1,
        // pinning the 7-bit payload decode) rather than a bare Step.
        let (ev, fetched) = run_fetch(&[0x07, 0x81], 0);
        assert_eq!(ev, Ev::Request(Rq::DeviceWrite { dev: 0, index: 1 }));
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

    /// Tape micro-ops carry their device index through to the bus.
    #[test]
    fn tape_micro_ops_are_device_indexed() {
        // 0x14 = test-arch "move left on dev 1"
        let (ev, _) = run_fetch(&[0x14], 0);
        assert_eq!(ev, Ev::Request(Rq::DeviceMoveLeft { dev: 1 }));
    }

    /// Read{dev, slot} latches device symbols into the TR bank.
    #[test]
    fn read_latches_into_tr() {
        // 0x10 = test-arch "read dev0→slot0, dev1→slot1".
        let arch = TestArch;
        let mut core = Core::new(&arch, 0);
        let mut ev = core.start();
        // Serve CodeRead(0) → 0x10, then two DeviceReads with symbols 7 and 9.
        ev = match ev {
            Ev::Request(Rq::CodeRead { addr: 0 }) => core.resume(Rs::Byte(0x10)),
            other => panic!("unexpected: {other:?}"),
        };
        ev = match ev {
            Ev::Request(Rq::DeviceRead { dev: 0 }) => core.resume(Rs::Symbol(7)),
            other => panic!("unexpected: {other:?}"),
        };
        ev = match ev {
            Ev::Request(Rq::DeviceRead { dev: 1 }) => core.resume(Rs::Symbol(9)),
            other => panic!("unexpected: {other:?}"),
        };
        assert_eq!(ev, Ev::Step);
        assert_eq!(core.tr(), &[7, 9]);
    }

    /// The Raise micro-op traps with the instruction's own address.
    #[test]
    fn raise_micro_op_traps_typed() {
        // 0x15 = test-arch "raise unmapped-read".
        let (ev, _) = run_fetch(&[0x15], 0);
        assert_eq!(ev, Ev::Trapped(Trap::UnmappedRead { at: 0 }));
    }

    /// Full scripted driver: code image + tiny stack + fake device log.
    /// Returns (final event, request log, mf).
    fn run_full(
        code: &[u8],
        entry: u32,
        stack_cap: usize,
        device_symbols: &[u32], // successive DeviceRead answers
        max_steps: usize,
    ) -> (Ev, Vec<Rq>, bool) {
        let arch = TestArch;
        let mut core = Core::new(&arch, entry);
        let mut log = Vec::new();
        let mut stack: Vec<u32> = Vec::new();
        let mut reads = device_symbols.iter().copied();
        let mut steps = 0;
        let mut ev = core.start();
        loop {
            match ev {
                Ev::Request(rq) => {
                    log.push(rq);
                    let resp = match rq {
                        Rq::CodeRead { addr } => match code.get(addr as usize) {
                            Some(&b) => Rs::Byte(b),
                            None => Rs::OutOfCode,
                        },
                        Rq::StackPush { value } => {
                            if stack.len() == stack_cap {
                                Rs::StackFull
                            } else {
                                stack.push(value);
                                Rs::Ok
                            }
                        }
                        Rq::StackPop => match stack.pop() {
                            Some(v) => Rs::Value(v),
                            None => Rs::StackEmpty,
                        },
                        Rq::DeviceRead { .. } => Rs::Symbol(reads.next().unwrap_or(0)),
                        Rq::DeviceMoveLeft { .. }
                        | Rq::DeviceMoveRight { .. }
                        | Rq::DeviceWrite { .. } => Rs::Ok,
                        Rq::TableRead { .. } | Rq::FrameRead { .. } => Rs::OutOfTable,
                    };
                    ev = core.resume(resp);
                }
                Ev::Step | Ev::Break => {
                    steps += 1;
                    if steps >= max_steps {
                        return (ev, log, core.mf());
                    }
                    ev = core.resume(Rs::Ok);
                }
                terminal => return (terminal, log, core.mf()),
            }
        }
    }

    #[test]
    fn move_write_latch_sequence_and_mf() {
        // right (move+latch reads 1 → mf=true), wr 0 (+latch reads 0 → mf=false), stop
        let code = [0x06, 0x07, 0x80, 0x02];
        let (ev, log, mf) = run_full(&code, 0, 4, &[1, 0], 100);
        assert_eq!(ev, Ev::Stopped);
        assert!(!mf);
        assert_eq!(
            log,
            vec![
                Rq::CodeRead { addr: 0 },
                Rq::DeviceMoveRight { dev: 0 },
                Rq::DeviceRead { dev: 0 },
                Rq::CodeRead { addr: 1 },
                Rq::CodeRead { addr: 2 },
                Rq::DeviceWrite { dev: 0, index: 0 },
                Rq::DeviceRead { dev: 0 },
                Rq::CodeRead { addr: 3 },
            ]
        );
    }

    #[test]
    fn conditional_jump_taken_and_untaken() {
        // 0x09 jm rel32: at entry mf=false (reset default) → falls
        // through to stop at 5. The taken case below uses a separate
        // program that first latches mf=true.
        let fall = [0x09, 0x01, 0x00, 0x00, 0x00, 0x02, 0x02];
        let (ev, log, _) = run_full(&fall, 0, 4, &[], 100);
        assert_eq!(ev, Ev::Stopped);
        assert_eq!(*log.last().unwrap(), Rq::CodeRead { addr: 5 }); // fell through

        // taken: set mf via a latch first — right reads 1 → mf=true, then jm +1
        // layout: [0]=0x06 right, [1..6]=jm +1, [6]=halt (skipped), [7]=stop
        let taken = [0x06, 0x09, 0x01, 0x00, 0x00, 0x00, 0x03, 0x02];
        let (ev2, log2, _) = run_full(&taken, 0, 4, &[1], 100);
        assert_eq!(ev2, Ev::Stopped); // jumped over the halt at 6 to stop at 7
        assert!(log2.contains(&Rq::CodeRead { addr: 7 }));
        assert!(!log2.contains(&Rq::CodeRead { addr: 6 })); // the halt at 6 was skipped
    }

    #[test]
    fn unconditional_jump_targets_end_relative() {
        // jmp rel8 at 0: instr_end = 2; off = +1 → target 3 (skip halt at 2)
        let code = [0x08, 0x01, 0x03, 0x02];
        let (ev, log, _) = run_full(&code, 0, 4, &[], 100);
        assert_eq!(ev, Ev::Stopped);
        assert!(log.contains(&Rq::CodeRead { addr: 3 }));
    }

    #[test]
    fn negative_jump_target_traps() {
        let code = [0x08, 0x80]; // jmp rel8 -128 from instr_end 2 → -126
        let (ev, _, _) = run_full(&code, 0, 4, &[], 100);
        // Pinned to `at: 0` (instr_start): this is only reachable if the
        // operand byte 0x80 was actually sign-extended to -128 and the jump
        // trapped immediately at the Call/JumpRel site. A u8 misread (off =
        // +128) would instead land ip at 130 and only trap later, off-the-end,
        // at a different address — `matches!(.., { .. })` would miss that bug.
        assert_eq!(ev, Ev::Trapped(Trap::CodeOutOfBounds { at: 0 }));
    }

    #[test]
    fn call_checks_entry_pushes_and_jumps_ret_returns() {
        // [0..5] call +1 → target 6 must hold 0x0E (entry) — instr_end 5, off 1
        // [5] stop  [6] 0x0E entry  [7] ret
        let code = [0x0A, 0x01, 0x00, 0x00, 0x00, 0x02, 0x0E, 0x0B];
        let (ev, log, _) = run_full(&code, 0, 4, &[], 100);
        assert_eq!(ev, Ev::Stopped);
        let call_check = Rq::CodeRead { addr: 6 }; // ent verification read
        let push = Rq::StackPush { value: 5 };
        let pos_check = log.iter().position(|r| *r == call_check).unwrap();
        let pos_push = log.iter().position(|r| *r == push).unwrap();
        assert!(pos_check < pos_push, "ent verified before push");
        assert!(log.contains(&Rq::StackPop));
        assert!(log.contains(&Rq::CodeRead { addr: 5 })); // returned to stop
    }

    #[test]
    fn call_to_non_entry_traps() {
        let code = [0x0A, 0x01, 0x00, 0x00, 0x00, 0x02, 0x01]; // target 6 = nop, not entry
        let (ev, _, _) = run_full(&code, 0, 4, &[], 100);
        assert_eq!(ev, Ev::Trapped(Trap::CallTargetNotEntry { target: 6 }));
    }

    #[test]
    fn call_past_image_is_not_entry() {
        // call +10 from instr_end 5 → target 15, beyond the 7-byte image
        let code = [0x0A, 0x0A, 0x00, 0x00, 0x00, 0x02, 0x0E];
        let (ev, _, _) = run_full(&code, 0, 4, &[], 100);
        assert_eq!(ev, Ev::Trapped(Trap::CallTargetNotEntry { target: 15 }));
    }

    #[test]
    fn stack_overflow_and_underflow_trap() {
        // capacity 0 stack: call overflows
        let code = [0x0A, 0x01, 0x00, 0x00, 0x00, 0x02, 0x0E, 0x0B];
        let (ev, _, _) = run_full(&code, 0, 0, &[], 100);
        assert_eq!(ev, Ev::Trapped(Trap::StackOverflow));
        // bare ret underflows
        let (ev2, _, _) = run_full(&[0x0B], 0, 4, &[], 100);
        assert_eq!(ev2, Ev::Trapped(Trap::StackUnderflow));
    }

    #[test]
    fn device_fault_becomes_trap() {
        let arch = TestArch;
        let mut core = Core::new(&arch, 0);
        core.start();
        // feed: opcode 0x07 (wr), operand 0x82 → Write(2) request → Fault
        core.resume(Rs::Byte(0x07));
        let ev = core.resume(Rs::Byte(0x82));
        let Ev::Request(Rq::DeviceWrite { index: 2, .. }) = ev else {
            panic!("expected write request, got {ev:?}");
        };
        let ev = core.resume(Rs::Fault(
            crate::vm::trap::DeviceFault::IndexOutsideAlphabet { index: 2 },
        ));
        assert!(matches!(
            ev,
            Ev::Trapped(Trap::Device {
                fault: crate::vm::trap::DeviceFault::IndexOutsideAlphabet { index: 2 }
            })
        ));
    }

    #[test]
    fn halt_and_brk_nop() {
        let (ev, _, _) = run_full(&[0x03], 0, 4, &[], 100);
        assert_eq!(ev, Ev::Halted);
        let (ev2, _, _) = run_full(&[0x04, 0x01, 0x02], 0, 4, &[], 100);
        assert_eq!(ev2, Ev::Stopped); // brk and nop are no-ops without a debugger
    }

    #[test]
    fn brk_retires_as_break_event_and_resume_continues_normally() {
        // brk; nop; stop — cap the step budget at exactly the brk's
        // retirement to observe its own event distinctly from Step.
        let (ev, _, _) = run_full(&[0x04, 0x01, 0x02], 0, 4, &[], 1);
        assert_eq!(ev, Ev::Break); // not Ev::Step
        // Resuming past it (as `halt_and_brk_nop` does with a larger
        // budget) reaches Stopped normally — the ack path doesn't care
        // which retirement event preceded it.
    }

    /// MR is the general register; MF is its boolean view (spec: MF ≡ MR≠0).
    #[test]
    fn mr_generalizes_mf() {
        let arch = TestArch;
        let mut core = Core::new(&arch, 0);
        assert_eq!(core.mr(), 0);
        assert!(!core.mf());
        core.set_mr(5);
        assert!(core.mf());
        core.set_mf(true);
        assert_eq!(core.mr(), 1);
        core.set_mf(false);
        assert_eq!(core.mr(), 0);
    }

    /// Serve CodeRead from `code`, TableRead from `tables`, DeviceRead from
    /// a symbol queue; resume past inter-instruction Steps and return the
    /// first terminal event (Stopped / Halted / Trapped).
    fn run_with_tables(code: &[u8], tables: &[u8], symbols: &[u32]) -> Ev {
        let arch = TestArch;
        let mut core = Core::new(&arch, 0);
        let mut reads = symbols.iter().copied();
        let mut ev = core.start();
        loop {
            ev = match ev {
                Ev::Request(Rq::CodeRead { addr }) => core.resume(match code.get(addr as usize) {
                    Some(&b) => Rs::Byte(b),
                    None => Rs::OutOfCode,
                }),
                Ev::Request(Rq::TableRead { addr }) => {
                    core.resume(match tables.get(addr as usize) {
                        Some(&b) => Rs::Byte(b),
                        None => Rs::OutOfTable,
                    })
                }
                Ev::Request(Rq::DeviceRead { .. }) => {
                    core.resume(Rs::Symbol(reads.next().expect("device script exhausted")))
                }
                Ev::Step | Ev::Break => core.resume(Rs::Ok),
                other => return other,
            };
        }
    }

    /// Match table at 0 (width 2, rows [1,2] and [1,*]), dispatch table at 7
    /// (2 entries: code addresses 11 and 12).
    fn table_blob() -> Vec<u8> {
        let mut t = vec![2, 2, 0, 1, 2, 1, 0x7F];
        t.extend([2u8, 0]);
        t.extend(11u32.to_le_bytes()); // MR=1 → stp at code addr 11
        t.extend(12u32.to_le_bytes()); // MR=2 → hlt at code addr 12
        t
    }

    /// 0x10 rd(dev0→tr0, dev1→tr1); 0x11 mtc @0; 0x12 djmp @7; stp; hlt.
    fn table_code() -> Vec<u8> {
        let mut c = vec![0x10, 0x11];
        c.extend(0i32.to_le_bytes());
        c.push(0x12);
        c.extend(7i32.to_le_bytes());
        c.push(0x02); // stp — code addr 11
        c.push(0x03); // hlt — code addr 12
        c
    }

    /// rd; mtc; djmp — the canonical conditional-state shape end to end.
    #[test]
    fn match_then_dispatch_selects_target() {
        // [1,2] matches row 1 → MR=1 → dispatch to stp.
        assert_eq!(
            run_with_tables(&table_code(), &table_blob(), &[1, 2]),
            Ev::Stopped
        );
        // [1,9] falls to the wildcard row → MR=2 → dispatch to hlt.
        assert_eq!(
            run_with_tables(&table_code(), &table_blob(), &[1, 9]),
            Ev::Halted
        );
    }

    #[test]
    fn dispatch_on_no_match_traps_no_transition() {
        // [5,5] matches nothing (no catch-all) → MR=0 → djmp (at addr 6) traps.
        assert_eq!(
            run_with_tables(&table_code(), &table_blob(), &[5, 5]),
            Ev::Trapped(Trap::NoTransition { at: 6 })
        );
    }

    #[test]
    fn table_read_past_section_traps() {
        // Truncated blob: header parses (width 2, 2 rows), first row byte
        // (addr 3) is out of table → the mtc at addr 1 faults.
        assert_eq!(
            run_with_tables(&table_code(), &table_blob()[..3], &[1, 2]),
            Ev::Trapped(Trap::TableOutOfBounds { at: 1 })
        );
    }

    #[test]
    fn match_table_malformed_width_traps_bad_operand() {
        // The width byte (17) exceeds the 16-position ceiling, so MatchWalk
        // yields Malformed on the first table byte; the MatchTable settle arm
        // maps it to BadOperand pinned at the mtc's own address (1). The TR
        // fill [1,2] is never compared — the header is rejected before any
        // row byte is read.
        let blob = [17u8, 1, 0]; // width 17, row_count 1 (LE) — never reached
        assert_eq!(
            run_with_tables(&table_code(), &blob, &[1, 2]),
            Ev::Trapped(Trap::BadOperand { at: 1 })
        );
    }

    #[test]
    fn dispatch_mr_past_entry_count_traps_out_of_range() {
        // [1,9] takes the wildcard row → MR=2, but the dispatch table declares
        // only one entry (count=1); MR > count makes DispatchWalk yield
        // OutOfRange → DispatchOutOfRange at the djmp's address (6).
        let mut blob = vec![2u8, 2, 0, 1, 2, 1, 0x7F]; // match table at 0
        blob.extend([1u8, 0]); // dispatch count = 1 (LE)
        blob.extend(11u32.to_le_bytes()); // the lone entry (would serve MR=1)
        assert_eq!(
            run_with_tables(&table_code(), &blob, &[1, 9]),
            Ev::Trapped(Trap::DispatchOutOfRange { at: 6 })
        );
    }

    #[test]
    fn dispatch_entry_read_past_table_traps_out_of_bounds() {
        // [1,2] matches row 1 → MR=1 (in range), but the blob ends right after
        // the dispatch count — the entry TableRead runs off the table and
        // returns OutOfTable → TableOutOfBounds at the djmp's address (6).
        let mut blob = vec![2u8, 2, 0, 1, 2, 1, 0x7F]; // match table at 0
        blob.extend([1u8, 0]); // dispatch count = 1, then truncated (no entry bytes)
        assert_eq!(
            run_with_tables(&table_code(), &blob, &[1, 2]),
            Ev::Trapped(Trap::TableOutOfBounds { at: 6 })
        );
    }

    // ----- the frames execution profile -----
    //
    // Fake encodings: 0x18 read-all; 0x19/0x1C callframe rel32 with the
    // frame offset hard-wired per opcode (0 / FRAME2_OFFSET); 0x1A/0x1B/
    // 0x1D retx with k hard-wired (0/1/2).

    use crate::vm::arch::test_arch::FRAME2_OFFSET;
    use crate::vm::frame::test_support::descriptor_bytes;

    /// Scripted driver for frames tests: code + tables (serving TableRead
    /// AND FrameRead) + per-device symbol scripts + a bounded stack.
    /// Records every request and FR at each instruction retirement.
    struct FramesRig {
        code: Vec<u8>,
        tables: Vec<u8>,
        /// Per-device FIFO of `DeviceRead` answers.
        reads: Vec<std::collections::VecDeque<u32>>,
        stack_cap: usize,
    }

    impl FramesRig {
        fn reads_of(scripts: &[&[u32]]) -> Vec<std::collections::VecDeque<u32>> {
            scripts
                .iter()
                .map(|s| s.iter().copied().collect())
                .collect()
        }

        /// Drive to the first terminal event; return (event, request log,
        /// FR at each Step/Break).
        fn run(&mut self, core: &mut Core) -> (Ev, Vec<Rq>, Vec<u32>) {
            let mut log = Vec::new();
            let mut fr_trace = Vec::new();
            let mut stack: Vec<u32> = Vec::new();
            let mut ev = core.start();
            for _ in 0..10_000 {
                match ev {
                    Ev::Request(rq) => {
                        log.push(rq);
                        let resp = match rq {
                            Rq::CodeRead { addr } => match self.code.get(addr as usize) {
                                Some(&b) => Rs::Byte(b),
                                None => Rs::OutOfCode,
                            },
                            Rq::StackPush { value } => {
                                if stack.len() == self.stack_cap {
                                    Rs::StackFull
                                } else {
                                    stack.push(value);
                                    Rs::Ok
                                }
                            }
                            Rq::StackPop => match stack.pop() {
                                Some(v) => Rs::Value(v),
                                None => Rs::StackEmpty,
                            },
                            Rq::DeviceRead { dev } => Rs::Symbol(
                                self.reads
                                    .get_mut(dev as usize)
                                    .and_then(|q| q.pop_front())
                                    .expect("device script exhausted"),
                            ),
                            Rq::DeviceMoveLeft { .. }
                            | Rq::DeviceMoveRight { .. }
                            | Rq::DeviceWrite { .. } => Rs::Ok,
                            Rq::TableRead { addr } | Rq::FrameRead { addr } => {
                                match self.tables.get(addr as usize) {
                                    Some(&b) => Rs::Byte(b),
                                    None => Rs::OutOfTable,
                                }
                            }
                        };
                        ev = core.resume(resp);
                    }
                    Ev::Step | Ev::Break => {
                        fr_trace.push(core.fr());
                        ev = core.resume(Rs::Ok);
                    }
                    terminal => return (terminal, log, fr_trace),
                }
            }
            panic!("run did not terminate");
        }
    }

    /// [0..5] framed call rel +1 → 6, [5] hlt (return-address canary:
    /// retx must NOT land here), [6] ent, body at 7+.
    fn framed_program(call_op: u8, body: &[u8]) -> Vec<u8> {
        let mut c = vec![call_op];
        c.extend(1i32.to_le_bytes());
        c.push(0x03); // hlt at 5 — the pushed return address
        c.push(0x0E); // ent at 6
        c.extend(body);
        c
    }

    fn frame_reads(log: &[Rq]) -> Vec<u32> {
        log.iter()
            .filter_map(|r| match r {
                Rq::FrameRead { addr } => Some(*addr),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn framed_call_activates_translates_and_retx_exits() {
        // Descriptor at 0: arity 1, virtual 0 → phys 1, swap maps, one
        // exit at the stp. Body: read-all, retx#0.
        let arch = TestArch;
        // body: [7] read-all, [8] retx#0; exit at 9: stp
        let code = {
            let mut c = framed_program(0x19, &[0x18, 0x1A]);
            c.push(0x02); // stp at 9 — exits[0]
            c
        };
        let tables = descriptor_bytes(&[(1, &[1, 0], &[1, 0])], &[9]);
        let desc_len = tables.len() as u32; // 20
        let mut rig = FramesRig {
            code,
            tables,
            reads: FramesRig::reads_of(&[&[], &[0]]), // dev1 serves phys 0
            stack_cap: 4,
        };
        let mut core = Core::new(&arch, 0).with_frames();
        let (ev, log, fr_trace) = rig.run(&mut core);
        assert_eq!(ev, Ev::Stopped);
        // The read went to the PHYSICAL device 1, never to virtual 0.
        assert!(log.contains(&Rq::DeviceRead { dev: 1 }));
        assert!(!log.contains(&Rq::DeviceRead { dev: 0 }));
        // rmap[phys 0] = virtual 1 landed in TR.
        assert_eq!(core.tr(), &[1]);
        // Descriptor bytes were fetched once, sequentially, as FrameReads.
        assert_eq!(frame_reads(&log), (0..desc_len).collect::<Vec<u32>>());
        // FR: active (=1) through the framed body, restored to 0 by retx.
        assert_eq!(fr_trace, vec![1, 1, 1, 0]);
        assert_eq!(core.fr(), 0);
    }

    #[test]
    fn latch_translates_the_device_and_the_read_symbol() {
        // Swap rmap: the latch's read returns phys 1 → virtual 0, which
        // does NOT equal the match index 1 → mf false. Untranslated, the
        // raw 1 would have matched — pinning the settle-side translation.
        let arch = TestArch;
        let code = {
            let mut c = framed_program(0x19, &[0x06, 0x1A]); // right+latch, retx#0
            c.push(0x02); // stp at 9
            c
        };
        let tables = descriptor_bytes(&[(1, &[1, 0], &[1, 0])], &[9]);
        let mut rig = FramesRig {
            code,
            tables,
            reads: FramesRig::reads_of(&[&[], &[1]]),
            stack_cap: 4,
        };
        let mut core = Core::new(&arch, 0).with_frames();
        let (ev, log, _) = rig.run(&mut core);
        assert_eq!(ev, Ev::Stopped);
        // The move was translated to the physical device too.
        assert!(log.contains(&Rq::DeviceMoveRight { dev: 1 }));
        assert!(!core.mf());
    }

    #[test]
    fn write_translates_symbol_through_wmap() {
        // wmap[virtual 0] = phys 3; rmap identity (the trailing latch
        // read passes through).
        let arch = TestArch;
        let code = {
            let mut c = framed_program(0x19, &[0x07, 0x80, 0x1A]); // wr v=0, retx#0
            c.push(0x02); // stp at 10
            c
        };
        let tables = descriptor_bytes(&[(1, &[], &[3])], &[10]);
        let mut rig = FramesRig {
            code,
            tables,
            reads: FramesRig::reads_of(&[&[], &[3]]),
            stack_cap: 4,
        };
        let mut core = Core::new(&arch, 0).with_frames();
        let (ev, log, _) = rig.run(&mut core);
        assert_eq!(ev, Ev::Stopped);
        assert!(log.contains(&Rq::DeviceWrite { dev: 1, index: 3 }));
    }

    #[test]
    fn write_hole_and_out_of_range_trap_unmapped_write() {
        let arch = TestArch;
        // A hole under the written symbol.
        let code = framed_program(0x19, &[0x07, 0x80]); // wr v=0 at 7
        let mut rig = FramesRig {
            code,
            tables: descriptor_bytes(&[(1, &[], &[0xFFFF])], &[9]),
            reads: FramesRig::reads_of(&[&[], &[]]),
            stack_cap: 4,
        };
        let mut core = Core::new(&arch, 0).with_frames();
        let (ev, log, _) = rig.run(&mut core);
        assert_eq!(ev, Ev::Trapped(Trap::UnmappedWrite { at: 7 }));
        // The trap fires at issue — no write request ever reaches the bus.
        assert!(!log.iter().any(|r| matches!(r, Rq::DeviceWrite { .. })));

        // Virtual symbol past the map's end.
        let code = framed_program(0x19, &[0x07, 0x81]); // wr v=1
        let mut rig = FramesRig {
            code,
            tables: descriptor_bytes(&[(1, &[], &[3])], &[9]),
            reads: FramesRig::reads_of(&[&[], &[]]),
            stack_cap: 4,
        };
        let mut core = Core::new(&arch, 0).with_frames();
        let (ev, _, _) = rig.run(&mut core);
        assert_eq!(ev, Ev::Trapped(Trap::UnmappedWrite { at: 7 }));
    }

    #[test]
    fn read_hole_and_out_of_range_trap_unmapped_read() {
        let arch = TestArch;
        // A hole under the physical symbol read back.
        let code = framed_program(0x19, &[0x17]); // read at 7
        let mut rig = FramesRig {
            code,
            tables: descriptor_bytes(&[(1, &[0xFFFF], &[])], &[9]),
            reads: FramesRig::reads_of(&[&[], &[0]]),
            stack_cap: 4,
        };
        let mut core = Core::new(&arch, 0).with_frames();
        let (ev, _, _) = rig.run(&mut core);
        assert_eq!(ev, Ev::Trapped(Trap::UnmappedRead { at: 7 }));

        // Physical symbol past the map's end.
        let code = framed_program(0x19, &[0x17]);
        let mut rig = FramesRig {
            code,
            tables: descriptor_bytes(&[(1, &[1], &[])], &[9]),
            reads: FramesRig::reads_of(&[&[], &[5]]),
            stack_cap: 4,
        };
        let mut core = Core::new(&arch, 0).with_frames();
        let (ev, _, _) = rig.run(&mut core);
        assert_eq!(ev, Ev::Trapped(Trap::UnmappedRead { at: 7 }));
    }

    #[test]
    fn identity_maps_pass_symbols_unchanged() {
        // Empty rmap/wmap = identity: only the device index translates.
        let arch = TestArch;
        let code = {
            let mut c = framed_program(0x19, &[0x17, 0x07, 0x87, 0x1A]);
            c.push(0x02); // stp at 11
            c
        };
        let tables = descriptor_bytes(&[(1, &[], &[])], &[11]);
        let mut rig = FramesRig {
            code,
            tables,
            reads: FramesRig::reads_of(&[&[], &[9, 7]]),
            stack_cap: 4,
        };
        let mut core = Core::new(&arch, 0).with_frames();
        let (ev, log, _) = rig.run(&mut core);
        assert_eq!(ev, Ev::Stopped);
        assert_eq!(core.tr(), &[9]);
        assert!(log.contains(&Rq::DeviceWrite { dev: 1, index: 7 }));
    }

    #[test]
    fn virtual_device_past_arity_traps_bad_operand() {
        // Arity-1 frame; 0x14 moves on virtual device 1 → out of frame.
        let arch = TestArch;
        let code = framed_program(0x19, &[0x14]);
        let mut rig = FramesRig {
            code,
            tables: descriptor_bytes(&[(1, &[], &[])], &[9]),
            reads: FramesRig::reads_of(&[]),
            stack_cap: 4,
        };
        let mut core = Core::new(&arch, 0).with_frames();
        let (ev, _, _) = rig.run(&mut core);
        assert_eq!(ev, Ev::Trapped(Trap::BadOperand { at: 7 }));
    }

    #[test]
    fn read_all_identity_reads_device_count_tapes() {
        // No frames involved at all: read-all under the identity frame
        // reads every physical tape in order.
        let arch = TestArch;
        let mut rig = FramesRig {
            code: vec![0x18, 0x02],
            tables: Vec::new(),
            reads: FramesRig::reads_of(&[&[4], &[5], &[6]]),
            stack_cap: 4,
        };
        let mut core = Core::new(&arch, 0).with_device_count(3);
        let (ev, log, _) = rig.run(&mut core);
        assert_eq!(ev, Ev::Stopped);
        assert_eq!(core.tr(), &[4, 5, 6]);
        let reads: Vec<Rq> = log
            .iter()
            .filter(|r| matches!(r, Rq::DeviceRead { .. }))
            .copied()
            .collect();
        assert_eq!(
            reads,
            vec![
                Rq::DeviceRead { dev: 0 },
                Rq::DeviceRead { dev: 1 },
                Rq::DeviceRead { dev: 2 },
            ]
        );
    }

    #[test]
    fn read_all_framed_narrows_tr_to_arity() {
        // Identity read-all first (3 tapes), then a framed one (arity 2):
        // TR must narrow to the frame's width, reading through the maps.
        let arch = TestArch;
        // [0] read-all; [1..6] callframe +1 → 7; [6] hlt; [7] ent;
        // [8] read-all; [9] retx#0; [10] stp (exits[0])
        let mut code = vec![0x18];
        code.extend(framed_program(0x19, &[0x18, 0x1A]));
        code.push(0x02); // stp at 10
        let tables = descriptor_bytes(&[(2, &[], &[]), (0, &[5, 6], &[])], &[10]);
        let mut rig = FramesRig {
            code,
            tables,
            // dev0: identity read 4, then framed virt1 raw 1 → rmap[1]=6
            // dev2: identity read 3, then framed virt0 (identity) 8
            reads: FramesRig::reads_of(&[&[4, 1], &[5], &[3, 8]]),
            stack_cap: 4,
        };
        let mut core = Core::new(&arch, 0).with_device_count(3).with_frames();
        let (ev, _, _) = rig.run(&mut core);
        assert_eq!(ev, Ev::Stopped);
        assert_eq!(core.tr(), &[8, 6]); // width 2, both through their maps
    }

    #[test]
    fn nested_plain_call_keeps_the_frame_without_reload() {
        // Inside the framed body, a plain call/ret pair: the frame stays
        // active after the return (pair discipline pushed the SAME FR),
        // and no descriptor reload happens (restored == current).
        let arch = TestArch;
        // [7..12] call +2 → 14; [12] 0x17 read (after return); [13] retx#0;
        // [14] ent(sub); [15] ret; exit at 16: stp
        let mut body = vec![0x0A];
        body.extend(2i32.to_le_bytes());
        body.extend([0x17, 0x1A, 0x0E, 0x0B]);
        let code = {
            let mut c = framed_program(0x19, &body);
            c.push(0x02); // stp at 16
            c
        };
        let tables = descriptor_bytes(&[(1, &[], &[])], &[16]);
        let desc_len = tables.len() as u32; // 12
        let mut rig = FramesRig {
            code,
            tables,
            reads: FramesRig::reads_of(&[&[], &[7]]),
            stack_cap: 4,
        };
        let mut core = Core::new(&arch, 0).with_frames();
        let (ev, log, fr_trace) = rig.run(&mut core);
        assert_eq!(ev, Ev::Stopped);
        // The read AFTER the nested return still routes to phys dev 1.
        assert_eq!(core.tr(), &[7]);
        assert!(log.contains(&Rq::DeviceRead { dev: 1 }));
        // One descriptor load total — the plain ret did not reload.
        assert_eq!(frame_reads(&log), (0..desc_len).collect::<Vec<u32>>());
        // callframe, ent, call, ent(sub), ret, read, retx
        assert_eq!(fr_trace, vec![1, 1, 1, 1, 1, 1, 0]);
    }

    #[test]
    fn nested_framed_call_reloads_the_outer_frame_on_retx() {
        // Frame A (offset 0, phys 1) calls frame B (offset 64, phys 2);
        // B's retx must restore AND reload A: the next read routes to
        // A's physical device again.
        let arch = TestArch;
        // [7..12] callframe2 +1 → 13; [12] hlt (B's return canary);
        // [13] ent; [14] 0x17 read (in B); [15] retx#0 → B exits[0]=16;
        // [16] 0x17 read (back in A); [17] retx#0 → A exits[0]=18; [18] stp
        let mut body = vec![0x1C];
        body.extend(1i32.to_le_bytes());
        body.extend([0x03, 0x0E, 0x17, 0x1A, 0x17, 0x1A]);
        let code = {
            let mut c = framed_program(0x19, &body);
            c.push(0x02); // stp at 18
            c
        };
        let desc_a = descriptor_bytes(&[(1, &[], &[])], &[18]);
        let desc_b = descriptor_bytes(&[(2, &[], &[])], &[16]);
        let (len_a, len_b) = (desc_a.len() as u32, desc_b.len() as u32);
        let mut tables = desc_a;
        tables.resize(FRAME2_OFFSET as usize, 0xEE);
        tables.extend(desc_b);
        let mut rig = FramesRig {
            code,
            tables,
            reads: FramesRig::reads_of(&[&[], &[4], &[2]]),
            stack_cap: 4,
        };
        let mut core = Core::new(&arch, 0).with_frames();
        let (ev, log, fr_trace) = rig.run(&mut core);
        assert_eq!(ev, Ev::Stopped);
        // Read order: B's read on phys 2, then (after reload) A's on phys 1.
        let reads: Vec<Rq> = log
            .iter()
            .filter(|r| matches!(r, Rq::DeviceRead { .. }))
            .copied()
            .collect();
        assert_eq!(
            reads,
            vec![Rq::DeviceRead { dev: 2 }, Rq::DeviceRead { dev: 1 }]
        );
        assert_eq!(core.tr(), &[4]); // A's read overwrote slot 0 last
        // Loads: A, then B, then A again (the reload).
        let mut expected: Vec<u32> = (0..len_a).collect();
        expected.extend(FRAME2_OFFSET..FRAME2_OFFSET + len_b);
        expected.extend(0..len_a);
        assert_eq!(frame_reads(&log), expected);
        // callframeA, entA, callframeB, entB, readB, retxB, readA, retxA
        let fr_b = FRAME2_OFFSET + 1;
        assert_eq!(fr_trace, vec![1, 1, fr_b, fr_b, fr_b, 1, 1, 0]);
    }

    #[test]
    fn plain_ret_after_a_framed_call_restores_identity() {
        // A framed body that returns with a PLAIN ret: the pair still
        // pops, FR restores to 0 (cache dropped, nothing reloaded), and
        // execution resumes at the pushed return address.
        let arch = TestArch;
        let mut code = framed_program(0x19, &[0x0B]); // body: plain ret at 7
        code[5] = 0x02; // the return address (5) holds stp — the happy path
        let tables = descriptor_bytes(&[(1, &[], &[])], &[9]);
        let desc_len = tables.len() as u32;
        let mut rig = FramesRig {
            code,
            tables,
            reads: FramesRig::reads_of(&[]),
            stack_cap: 4,
        };
        let mut core = Core::new(&arch, 0).with_frames();
        let (ev, log, _) = rig.run(&mut core);
        assert_eq!(ev, Ev::Stopped);
        assert_eq!(core.fr(), 0);
        // No reload on restoring the identity frame.
        assert_eq!(frame_reads(&log), (0..desc_len).collect::<Vec<u32>>());
    }

    #[test]
    fn retx_exit_index_out_of_range_traps() {
        // One exit; retx#1 names exit 1 → out of range, pinned at retx.
        let arch = TestArch;
        let code = framed_program(0x19, &[0x1B]); // retx#1 at 7
        let mut rig = FramesRig {
            code,
            tables: descriptor_bytes(&[(1, &[], &[])], &[9]),
            reads: FramesRig::reads_of(&[]),
            stack_cap: 4,
        };
        let mut core = Core::new(&arch, 0).with_frames();
        let (ev, _, _) = rig.run(&mut core);
        assert_eq!(ev, Ev::Trapped(Trap::ExitOutOfRange { at: 7 }));
    }

    #[test]
    fn retx_at_identity_traps_exit_out_of_range() {
        // Frames enabled but no frame active: retx has no exit vector.
        let arch = TestArch;
        let mut rig = FramesRig {
            code: vec![0x1A],
            tables: Vec::new(),
            reads: FramesRig::reads_of(&[]),
            stack_cap: 4,
        };
        let mut core = Core::new(&arch, 0).with_frames();
        let (ev, _, _) = rig.run(&mut core);
        assert_eq!(ev, Ev::Trapped(Trap::ExitOutOfRange { at: 0 }));
    }

    #[test]
    fn frame_instructions_trap_profile_violation_on_base_profile() {
        // Without with_frames(), callframe and retx are outside the
        // profile — pinned as typed traps at the instruction address.
        let arch = TestArch;
        for code in [framed_program(0x19, &[]), vec![0x1A]] {
            let mut rig = FramesRig {
                code,
                tables: descriptor_bytes(&[(1, &[], &[])], &[9]),
                reads: FramesRig::reads_of(&[]),
                stack_cap: 4,
            };
            let mut core = Core::new(&arch, 0);
            let (ev, _, _) = rig.run(&mut core);
            assert_eq!(ev, Ev::Trapped(Trap::ProfileViolation { at: 0 }));
        }
    }

    #[test]
    fn stack_overflow_on_framed_call_leaves_fr_in_sync() {
        let arch = TestArch;
        // Capacity 0: the first framed call overflows; FR must stay 0.
        let mut rig = FramesRig {
            code: framed_program(0x19, &[]),
            tables: descriptor_bytes(&[(1, &[], &[])], &[9]),
            reads: FramesRig::reads_of(&[]),
            stack_cap: 0,
        };
        let mut core = Core::new(&arch, 0).with_frames();
        let (ev, log, _) = rig.run(&mut core);
        assert_eq!(ev, Ev::Trapped(Trap::StackOverflow));
        assert_eq!(core.fr(), 0);
        assert!(frame_reads(&log).is_empty()); // no load without a push

        // Capacity 1: frame A activates, the nested framed call
        // overflows — FR must still be A's, not B's.
        let mut body = vec![0x1C];
        body.extend(1i32.to_le_bytes());
        body.extend([0x03, 0x0E]);
        let desc_a = descriptor_bytes(&[(1, &[], &[])], &[18]);
        let mut tables = desc_a;
        tables.resize(FRAME2_OFFSET as usize, 0xEE);
        tables.extend(descriptor_bytes(&[(2, &[], &[])], &[16]));
        let mut rig = FramesRig {
            code: framed_program(0x19, &body),
            tables,
            reads: FramesRig::reads_of(&[]),
            stack_cap: 1,
        };
        let mut core = Core::new(&arch, 0).with_frames();
        let (ev, _, _) = rig.run(&mut core);
        assert_eq!(ev, Ev::Trapped(Trap::StackOverflow));
        assert_eq!(core.fr(), 1);
    }

    #[test]
    fn truncated_or_malformed_descriptor_traps_table_out_of_bounds() {
        let arch = TestArch;
        let full = descriptor_bytes(&[(1, &[1, 0], &[])], &[9]);
        let seventeen: Vec<(u8, &[u16], &[u16])> = (0..17).map(|i| (i, &[][..], &[][..])).collect();
        let cases: Vec<Vec<u8>> = vec![
            full[..5].to_vec(),                 // truncated mid-descriptor
            descriptor_bytes(&[], &[9]),        // arity 0
            descriptor_bytes(&seventeen, &[9]), // arity 17
        ];
        for tables in cases {
            let mut rig = FramesRig {
                code: framed_program(0x19, &[]),
                tables,
                reads: FramesRig::reads_of(&[]),
                stack_cap: 4,
            };
            let mut core = Core::new(&arch, 0).with_frames();
            let (ev, _, _) = rig.run(&mut core);
            assert_eq!(ev, Ev::Trapped(Trap::TableOutOfBounds { at: 0 }));
        }
    }
}
