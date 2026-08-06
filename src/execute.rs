//! MC68000 instruction decoding and execution.

use crate::addressing::{Operand, Size};
use crate::bus::Bus;
use crate::cpu::{ADDR_MASK, Cpu, CpuType, ccr, sr};

/// Unhandled execution error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepError {
    /// Opcode matching no instruction implemented by the `op_line_*`
    /// handlers. Returned internally by [`Cpu::execute`]; [`Cpu::step`]
    /// does NOT let it propagate to the caller — it dispatches the
    /// illegal instruction exception (vector 4) instead, just like real
    /// silicon (there is no hardware distinction between "reserved" and
    /// "not yet implemented by this project") — see the comment at the
    /// interception site in `Cpu::step`. This variant therefore stays
    /// visible only in internal code, never observable via the public API.
    Unimplemented(u16),
    /// Effective address encoding structurally invalid for an otherwise
    /// recognized instruction (e.g. out-of-range addressing mode, reserved
    /// size). Handled the same way as [`Self::Unimplemented`] in
    /// [`Cpu::step`]: dispatches vector 4 rather than propagating to the
    /// caller — same reasoning (no hardware distinction between the two
    /// cases on real silicon).
    IllegalAddressing,
    /// Word/long access to an odd address: address error (vector 3).
    /// Fields: (odd_address, is_write, pc_at_time_of_access)
    AddressError(u32, bool, u32),
    /// Double bus fault: a bus/address error occurred while pushing the
    /// frame of a previous bus/address error (typically a stack pointer
    /// outside any mapped region). On real 68000 hardware (and on Hatari,
    /// verified) the CPU then halts permanently — see
    /// [`crate::cpu::Cpu::halted`]. Returned repeatedly by [`Cpu::step`]
    /// as long as this field stays true (only a hardware `Cpu::reset`
    /// clears it).
    DoubleFault,
}

/// Converts a read address error into a StepError.
#[inline(always)]
fn ae_read(r: Result<u32, (u32, u32)>) -> Result<u32, StepError> {
    r.map_err(|(addr, pc)| StepError::AddressError(addr, false, pc))
}

/// Converts a write address error into a StepError.
#[inline(always)]
fn ae_write(r: Result<(), (u32, u32)>) -> Result<(), StepError> {
    r.map_err(|(addr, pc)| StepError::AddressError(addr, true, pc))
}

/// Cycle-exact cost of the DIVU microcode core: the 68000 computes the
/// quotient via a bit-by-bit non-restoring binary division, whose cost
/// depends on the `dividend`/`divisor` values (not just the addressing
/// mode). This algorithm was publicly documented by Jorge Cwik ("Pasti")
/// in a June 2005 Atari-Forum post (thread "68000 DIVU/DIVS cycle
/// accurate timing") describing the 15-bit loop's operation and its
/// per-branch costs. The implementation below is an independent rewrite
/// of that technical description (not a translation of an existing
/// emulator's code), whose constants were re-derived and verified by
/// exhaustive search against the 371 register-direct cases of the
/// TomHarte conformance suite (0 mismatches), before extending to all
/// 2500 DIVU cases (all addressing modes).
///
/// Does not cover the zero-divisor case (handled separately by the
/// caller). The final `- 4` compensates for the `+ 4` opcode-fetch cost
/// that the caller systematically adds (`op_line_8`, a convention shared
/// across this whole file): it is not a constant of the algorithm itself,
/// just the cut point chosen here between the "microcode core" and the
/// "fetch".
fn divu_core_cycles(dividend: u32, divisor: u16) -> u32 {
    let divisor = divisor as u32;
    // Overflow detected before the loop: the quotient would not fit in
    // 16 bits.
    if (dividend >> 16) >= divisor {
        return 5 * 2 - 4;
    }
    let shifted_divisor = divisor << 16;
    let mut remainder = dividend;
    let mut microcycles: i32 = 38;
    for _ in 0..15 {
        let before_shift = remainder;
        remainder <<= 1;
        if (before_shift as i32) < 0 {
            // The high-order bit was set before this shift: the
            // non-restoring subtraction still applies here (no extra
            // comparison cost, unlike the branch below).
            remainder = remainder.wrapping_sub(shifted_divisor);
        } else {
            // Actual comparison: 2 base cycles, 1 of which is refunded if
            // the subtraction actually succeeds (borrow).
            microcycles += 2;
            if remainder >= shifted_divisor {
                remainder = remainder.wrapping_sub(shifted_divisor);
                microcycles -= 1;
            }
        }
    }
    (microcycles * 2 - 4) as u32
}

/// Cycle-exact cost of the DIVS microcode core. Same public source as
/// [`divu_core_cycles`] (Jorge Cwik, Atari-Forum 2005), an independent
/// rewrite verified by exhaustive search against the 177 non-overflowing
/// register-direct TomHarte cases (0 mismatches) as well as the 4 sign
/// combinations of the overflow case, before extending to all 2500 DIVS
/// cases.
fn divs_core_cycles(dividend: i32, divisor: i16) -> u32 {
    let mut microcycles: i32 = 6;
    if dividend < 0 {
        microcycles += 1;
    }
    let abs_dividend = dividend.unsigned_abs();
    let abs_divisor = (divisor as i32).unsigned_abs();
    if (abs_dividend >> 16) >= abs_divisor {
        // Absolute overflow: cost depends only on the dividend's sign
        // (16 or 18 real cycles), the quotient loop never runs.
        return ((microcycles + 2) * 2 - 4) as u32;
    }
    microcycles += 55;
    if divisor >= 0 {
        if dividend >= 0 {
            microcycles -= 1;
        } else {
            microcycles += 1;
        }
    }
    // Counts the 15 high-order bits of the absolute quotient that are 0
    // (each costs 1 extra cycle) — the division itself has already been
    // performed directly, this count is only used for timing.
    let mut abs_quotient = (abs_dividend / abs_divisor) as u16;
    for _ in 0..15 {
        if abs_quotient & 0x8000 == 0 {
            microcycles += 1;
        }
        abs_quotient <<= 1;
    }
    (microcycles * 2 - 4) as u32
}

/// Base cost of MOVE's destination write, by mode/size (Yacht.txt lines
/// 338-553 of the STAY repository: `<ea>,Dn` / `<ea>,(An)` / ... tables by
/// destination mode). Added to the SOURCE's `ea_extra_cycles` (captured by
/// op_move before resolving the destination) — the two terms are
/// independent and not redundant (one reads, the other writes).
fn move_dst_base(dst_mode: u16, dst_reg: u16, size: Size) -> u32 {
    let long = size == Size::Long;
    match dst_mode {
        0b000 | 0b001 => 4, // Dn, An (MOVEA)
        0b010 | 0b011 | 0b100 => {
            if long {
                12
            } else {
                8
            }
        } // (An), (An)+, -(An)
        0b101 => {
            if long {
                16
            } else {
                12
            }
        } // (d16,An)
        0b110 => {
            if long {
                18
            } else {
                14
            }
        } // (d8,An,Xn)
        0b111 => match dst_reg {
            0b000 => {
                if long {
                    16
                } else {
                    12
                }
            } // (xxx).W
            0b001 => {
                if long {
                    20
                } else {
                    16
                }
            } // (xxx).L
            _ => {
                if long {
                    12
                } else {
                    8
                }
            } // not valid as a MOVE dst — safe fallback
        },
        _ => {
            if long {
                12
            } else {
                8
            }
        }
    }
}

/// Cost shared by OR/AND and the generic ADD/SUB form (Yacht.txt lines
/// 1064-1105 and 1258-1299 of the STAY repository — same microcode family,
/// same shape). `to_ea`: direction Dn,<ea> (true) or <ea>,Dn (false).
/// `ea_is_reg` (relevant only if `!to_ea`): source is Dn/An, no memory EA
/// cost.
fn logic_op_cost(to_ea: bool, ea_is_reg: bool, size: Size, ea_extra: u32) -> u32 {
    let long = size == Size::Long;
    if to_ea {
        if long { 12 + ea_extra } else { 8 + ea_extra }
    } else if long {
        // ea_is_reg also covers #imm (see call sites): its base (8) always
        // gets + ea_extra added, which is 0 for Dn/An so unchanged for them.
        if ea_is_reg { 8 + ea_extra } else { 6 + ea_extra }
    } else {
        // ea_extra is 0 for Dn/An, so "4" and "4+ea_extra" already coincide
        // for them: no need to distinguish ea_is_reg here anymore (the old
        // bare "4" branch broke #imm, which needs its ea_extra=4/8).
        4 + ea_extra
    }
}

/// Cost shared by CLR/NEG/NEGX/NOT (Yacht.txt lines 564-586 of the STAY
/// repository): read-modify-write on Dn or in memory.
fn rmw_cost(is_dn: bool, size: Size, ea_extra: u32) -> u32 {
    let long = size == Size::Long;
    if is_dn {
        if long { 6 } else { 4 }
    } else if long {
        12 + ea_extra
    } else {
        8 + ea_extra
    }
}

/// JSR cost by mode (Yacht.txt lines 806-814) — a dedicated table, does not
/// follow the generic `ea_extra_cycles` pattern.
fn jsr_cost(mode: u16, reg: u16) -> u32 {
    match mode {
        0b101 => 18, // (d16,An)
        0b110 => 22, // (d8,An,Xn)
        0b111 => match reg {
            0b000 => 18, // (xxx).W
            0b001 => 20, // (xxx).L
            0b010 => 18, // (d16,PC)
            0b011 => 22, // (d8,PC,Xn)
            _ => 16,
        },
        _ => 16, // (An)
    }
}

/// JMP cost by mode (Yacht.txt lines 817-825).
fn jmp_cost(mode: u16, reg: u16) -> u32 {
    match mode {
        0b101 => 10,
        0b110 => 14,
        0b111 => match reg {
            0b000 => 10,
            0b001 => 12,
            0b010 => 10,
            0b011 => 14,
            _ => 8,
        },
        _ => 8,
    }
}

/// PEA/LEA cost by mode (Yacht.txt lines 634-644 / 794-803) — same shape
/// for both instructions, offset by a constant (PEA = LEA + 8).
fn pea_lea_cost(mode: u16, reg: u16, is_pea: bool) -> u32 {
    let lea = match mode {
        0b101 => 8,  // (d16,An)
        0b110 => 12, // (d8,An,Xn)
        0b111 => match reg {
            0b000 => 8,  // (xxx).W
            0b001 => 12, // (xxx).L
            0b010 => 8,  // (d16,PC)
            0b011 => 12, // (d8,PC,Xn)
            _ => 4,
        },
        _ => 4, // (An)
    };
    if is_pea { lea + 8 } else { lea }
}

/// Base MOVEM cost, by mode/direction (Yacht.txt lines 654-696 of the STAY
/// repository) — independent of size (word/long only change the per-register
/// cost, `4` or `8`, added separately based on the number of registers in
/// the mask). `-(An)` (mode 0b100, R→M only) shares `(An)`'s base.
fn movem_base(mode: u16, reg: u16, to_regs: bool) -> u32 {
    if to_regs {
        match mode {
            0b010 | 0b011 => 12, // (An), (An)+
            0b101 => 16,         // (d16,An)
            0b110 => 18,         // (d8,An,Xn)
            0b111 => match reg {
                0b001 => 20,      // (xxx).L
                0b011 => 18,      // (d8,PC,Xn) — same cost as (d8,An,Xn)
                _ => 16,          // (xxx).W, (d16,PC)
            },
            _ => 12,
        }
    } else {
        match mode {
            0b010 | 0b100 => 8, // (An), -(An)
            0b101 => 12,        // (d16,An)
            0b110 => 14,        // (d8,An,Xn)
            0b111 => {
                if reg == 0b001 {
                    16
                } else {
                    12
                }
            } // (xxx).L / (xxx).W
            _ => 8,
        }
    }
}

impl Cpu {
    /// Reads memory at `addr` (unmasked) with address error detection.
    /// Returns `Err((fault_addr, pc))` for a word/long access to an odd
    /// address. For a **long** access, the reported fault address is
    /// `addr + 2` (the 68000 reports the error on the second word cycle
    /// of the long transfer).
    fn read_mem_checked(
        &self,
        bus: &mut impl Bus,
        addr: u32,
        size: Size,
    ) -> Result<u32, (u32, u32)> {
        if size != Size::Byte && addr & 1 != 0 {
            let fault = if size == Size::Long {
                addr.wrapping_add(2)
            } else {
                addr
            };
            return Err((fault, self.ea_frame_pc));
        }
        Ok(match size {
            Size::Byte => bus.read8(addr & ADDR_MASK) as u32,
            Size::Word => bus.read16(addr & ADDR_MASK) as u32,
            Size::Long => bus.read32(addr & ADDR_MASK),
        })
    }

    /// Checks supervisor mode. If in user mode, triggers the privilege
    /// violation exception (vector 8) and returns Some(cycles).
    /// Returns None if in supervisor mode (normal execution).
    fn check_privilege(&mut self, bus: &mut impl Bus) -> Option<u32> {
        if !self.supervisor() {
            let pc_push = self.pc.wrapping_sub(2); // opcode_addr
            self.take_exception(bus, 8, pc_push);
            Some(34)
        } else {
            None
        }
    }

    /// Reads `Dn` or `An` depending on `is_addr` — used by MOVEC (68010+),
    /// whose source/destination general register is selected by an A/D bit
    /// in the extension word rather than by `resolve_ea`'s usual mode/reg
    /// field.
    fn d_or_a(&self, is_addr: bool, reg: usize) -> u32 {
        if is_addr { self.a[reg] } else { self.d[reg] }
    }

    /// Writes `value` into `Dn` or `An` depending on `is_addr` — see
    /// [`Self::d_or_a`].
    fn set_d_or_a(&mut self, is_addr: bool, reg: usize, value: u32) {
        if is_addr {
            self.a[reg] = value;
        } else {
            self.d[reg] = value;
        }
    }

    pub fn step(&mut self, bus: &mut impl Bus) -> Result<u32, StepError> {
        // Double bus fault (see `Cpu::halted`): the real 68000 stays frozen
        // until an external hardware /RESET, it never resumes on its own —
        // even just re-evaluating interrupts would already be doing too
        // much.
        if self.halted {
            return Err(StepError::DoubleFault);
        }
        // Interrupts are recognized at the boundary between two
        // instructions (not while one is executing): checked even before
        // fetching the next opcode, outside any TimedBus since an IACK
        // cycle is not subject to normal DRAM/video wait-states.
        if let Some(cycles) = self.take_interrupt(bus) {
            self.cycles = self.cycles.wrapping_add(cycles as u64);
            return Ok(cycles);
        }
        // T bit at the very start of THIS instruction (before execution) —
        // see its use at the end of this function, doc of
        // `Cpu::trace_pending`.
        let trace_armed_before = self.sr & sr::T != 0;
        self.pending_address_error = None;
        self.fault_prefix = 4;
        // Wraps the bus to apply DRAM/video wait-states (Steem
        // RAM_ACCESS_WS) to every real transaction. `addressing.rs` and the
        // opcode handlers are generic over `impl Bus` and don't need to
        // know they're going through this wrapper — the interception is
        // therefore transparent, no bus-access call site needs to be
        // modified. `pos` picks up from `self.cycles`: the 4-cycle grid
        // stays in phase with the continuous clock across instructions,
        // not reset to zero on every step().
        let mut timed = crate::bus::TimedBus {
            inner: bus,
            pos: self.cycles,
            access_count: 0,
        };
        let opcode = self.fetch_word(&mut timed);
        self.current_ir = opcode;
        // PC right after fetching the opcode (= opcode_addr + 2), before any
        // extension words. This is the PC to save in the frame if an
        // instruction-fetch AE occurs.
        let pc_after_opcode = self.pc;

        // Did the opcode fetch itself touch an address with no chip select
        // (the physical "hole")? Typical case: a program that jumped into
        // an unmapped region (e.g. corrupted stack, or our crash watchdog
        // that sees the PC leave every valid range).
        if let Some((fault_addr, is_write)) = timed.take_bus_fault() {
            self.take_bus_error_full(
                &mut timed,
                fault_addr,
                is_write,
                Some(pc_after_opcode.wrapping_sub(2)),
                true,
            );
            return Ok(self.finalize_cycles(&timed, 50));
        }

        let result = self.execute(&mut timed, opcode);
        match result {
            Ok(cycles) => {
                // Yacht.txt "Group 0 : Address error = 50(4/7)" is the cost of
                // the exception dispatch ALONE (vector fetch + frame push +
                // first handler fetch) — it does not replace whatever the
                // faulting instruction had already cost to reach this point
                // (e.g. RTE fully executes at its normal 20 cycles, THEN the
                // address error fires while fetching the next instruction
                // from the bad popped PC: total = 20 + 50 = 70, matching
                // ProcessorTests). Confirmed against conformance data: RTE
                // 20+50=70, RTS 16+50=66, RTR 20+50=70 all match exactly.
                //
                // pending_address_error is set by BSR/BRA to override the
                // frame's PC. For JMP/RTS, we just detect self.pc & 1 and
                // use pc_after_opcode.
                if let Some((fault_addr, is_write, explicit_pc)) = self.pending_address_error.take()
                {
                    self.take_address_error_full(
                        &mut timed,
                        fault_addr,
                        is_write,
                        Some(explicit_pc),
                        true,
                    );
                    return Ok(self.finalize_cycles(&timed, cycles + 50));
                }
                // After execution, if PC is odd → address error on
                // instruction fetch (e.g. JMP/RTS jumping to an odd
                // address)
                if self.pc & 1 != 0 {
                    let fault_addr = self.pc;
                    // PC in the frame = address after fetching the current
                    // opcode, NOT the target
                    self.take_address_error_full(
                        &mut timed,
                        fault_addr,
                        false,
                        Some(pc_after_opcode),
                        true,
                    );
                    return Ok(self.finalize_cycles(&timed, cycles + 50));
                }
                // Did a data read/write during execution touch the
                // physical "hole" (beyond installed RAM, before the ROM)?
                // This is the mechanism countless programs/demos use to
                // detect installed RAM: they install their own handler at
                // vector $8, then scan memory until the bus faults —
                // without this real exception, the scan continues forever
                // (or to some arbitrary bound) and ends up overwriting its
                // own stack with the data it writes.
                if let Some((fault_addr, is_write)) = timed.take_bus_fault() {
                    self.take_bus_error_full(&mut timed, fault_addr, is_write, None, false);
                    return Ok(self.finalize_cycles(&timed, cycles + 50));
                }
                // Trace (SR's T bit): if the instruction completed
                // normally (none of the cases above, nor a software
                // exception internal to the instruction itself — TRAP,
                // CHK, divide by zero, ILLEGAL, Line-A/F, privilege
                // violation — since take_exception already clears T when
                // entering ITS OWN frame, so self.sr.T can only still be
                // set here if the instruction truly completed normally),
                // the 68000 triggers exception vector 9 before the next
                // instruction — BUT NOT if it was THIS instruction itself
                // that just set T (MOVE/ANDI/ORI/EORI to SR, RTE popping
                // an SR with T=1): real silicon then delays by one extra
                // instruction, exactly like the IPL mask (cf.
                // `Cpu::sr_write_pending_delay`) — it is the SAME
                // low-level hardware mechanism (the updated SR, T included,
                // only becomes fully effective on the next instruction
                // cycle). Cross-checked on 2026-08-04 by step-by-step
                // comparison execution under Hatari (breakpoints + register
                // dumps) on `Rick_Dangerous.stx`: a TOS routine sets T via
                // `ORI #$a71f,SR` and then executes YET ANOTHER instruction
                // before the trace fires (it uses this to drive, instruction
                // by instruction, a decryption loop) — the earlier
                // hypothesis of "right at the end of THIS instruction"
                // (unverified at the time) caused a double bus fault that
                // Hatari never has.
                //
                // The TomHarte suite deliberately captures the effect of a
                // SINGLE instruction without chaining into the trace even
                // when T=1 on entry (e.g. NOP with T=1: final.sr ==
                // initial.sr, no frame pushed): `step` therefore does NOT
                // take the exception itself, it just sets `trace_pending`
                // to true — it is up to the caller (a real emulation loop,
                // not the conformance harness) to call
                // `take_trace_exception` if it wants the real effect.
                self.trace_pending = trace_armed_before && (self.sr & sr::T != 0);
                Ok(self.finalize_cycles(&timed, cycles))
            }
            Err(StepError::AddressError(fault_addr, is_write, pc_at_fault)) => {
                self.take_address_error_at(&mut timed, fault_addr, is_write, Some(pc_at_fault));
                // Fault while reading/writing the operand right after EA
                // resolution (the common case: ae_read(ea.read(...))? /
                // ae_write(ea.write(...))? immediately following resolve_ea).
                // Cost = fault_prefix + ea_extra + 50 (Yacht.txt "Group 0:
                // Address error"). Three prefixes, all calibrated against
                // ProcessorTests — see `Cpu::fault_prefix` doc:
                //  - 4: simple source-operand read (DIVU/DIVS divisor, <ea>,Dn,
                //    TST, CMP...). E.g. (d8,An,Xn)=4+10+50=64, -(An)=4+6+50=60.
                //  - 0: RMW re-read of the OLD destination for a two-operand
                //    Dn,<ea> (OR/AND/EOR/ADD/SUB). E.g. (An)=0+8+50=58.
                //  - 8: immediate-to-memory RMW family sharing op_line_0
                //    (ORI/ANDI/SUBI/ADDI/EORI). E.g. -(An)=8+6+50=64.
                // Instructions whose fault happens somewhere other than "right
                // after resolve_ea" (e.g. UNLINK reading straight from An) have
                // their own different prefix cost and are not covered here —
                // tracked separately.
                let table_cost = self.fault_prefix + self.ea_extra_cycles + 50;
                Ok(self.finalize_cycles(&timed, table_cost))
            }
            // Opcode matching no implemented instruction. On real silicon,
            // ANY bit pattern that doesn't match a valid instruction
            // (whether officially "reserved" like 0x4AFC, or simply a
            // pattern this project hasn't implemented yet) triggers the
            // illegal instruction exception (vector 4) — there is no
            // hardware distinction between the two cases. Verified against
            // Hatari (its disassembler, `w w <addr> <word>` then
            // `d <addr>`, explicitly classifies 0x4545 as "illegal"):
            // `Rick_Dangerous.stx` deliberately executes a reserved opcode
            // in-game, likely an anti-piracy/anti-debugger trick of the
            // same kind as the "supervisor via ILLEGAL" trigger already
            // encountered (0x4AFC) — without this dispatch, any program
            // relying on this technique stops the emulator dead instead of
            // continuing normally as on real hardware. The TomHarte suite
            // still catches any real regression on an actually valid
            // opcode (it checks the precise effect of every covered
            // opcode, not just the absence of a crash).
            // `IllegalAddressing`: same principle as `Unimplemented`
            // above, but for an otherwise recognized opcode whose encoded
            // addressing mode is structurally invalid for THIS instruction
            // (e.g. ORI's reserved size "11" combined with an EA that
            // doesn't match any of the special CCR/SR forms — 0x00E0,
            // encountered in-game in Rick_Dangerous.stx and verified
            // "illegal" the same way against Hatari). Same handling:
            // vector 4, no distinction from the case above on real
            // silicon.
            Err(StepError::Unimplemented(_)) | Err(StepError::IllegalAddressing) => {
                let pc_push = pc_after_opcode;
                self.take_exception(&mut timed, 4, pc_push);
                Ok(self.finalize_cycles(&timed, 34))
            }
            Err(e) => Err(e),
        }
    }

    /// Combines the bus cycles actually observed during the instruction
    /// (transactions × 4 + inserted wait-states, already accumulated in
    /// `timed.pos`) with the purely internal part of the table cost
    /// computed by the opcode handler (`table_cost` minus the 4 cycles per
    /// transaction already counted on the bus side, to avoid double
    /// counting). Advances `self.cycles` and returns the total actually
    /// consumed.
    fn finalize_cycles<B: Bus>(&mut self, timed: &crate::bus::TimedBus<B>, table_cost: u32) -> u32 {
        let bus_cycles = timed.pos - self.cycles;
        let internal = (table_cost as u64).saturating_sub(4 * timed.access_count as u64);
        let total = bus_cycles + internal;
        self.cycles = self.cycles.wrapping_add(total);
        total as u32
    }

    fn execute(&mut self, bus: &mut impl Bus, opcode: u16) -> Result<u32, StepError> {
        match opcode >> 12 {
            0b0000 => self.op_line_0(bus, opcode), // ORI, ANDI, SUBI, ADDI, EORI, CMPI, BTST/BCHG/BCLR/BSET
            0b0001..=0b0011 => self.op_move(bus, opcode),
            0b0100 => self.op_line_4(bus, opcode),
            0b0101 => self.op_line_5(bus, opcode), // ADDQ, SUBQ, Scc, DBcc
            0b0110 => self.op_branch(bus, opcode),
            0b0111 => self.op_moveq(opcode),
            0b1000 => self.op_line_8(bus, opcode), // DIVU, DIVS, OR
            0b1001 => self.op_sub(bus, opcode),
            0b1011 => self.op_line_b(bus, opcode), // CMP, CMPA, CMPM, EOR
            0b1100 => self.op_line_c(bus, opcode), // MULU, MULS, AND, EXG, ABCD
            0b1101 => self.op_add(bus, opcode),
            0b1110 => self.op_line_e(bus, opcode), // ASL/ASR/LSL/LSR/ROL/ROR/ROXL/ROXR
            0b1010 => {
                // Line A: exception vector 10
                let pc_push = self.pc.wrapping_sub(2); // opcode_addr (for replay)
                self.take_exception(bus, 10, pc_push);
                Ok(34)
            }
            0b1111 => {
                // Line F: exception vector 11
                let pc_push = self.pc.wrapping_sub(2);
                self.take_exception(bus, 11, pc_push);
                Ok(34)
            }
            _ => Err(StepError::Unimplemented(opcode)),
        }
    }

    // =========================================================================
    // Line 0000: immediate operations + bit manipulation
    // =========================================================================

    fn op_line_0(&mut self, bus: &mut impl Bus, opcode: u16) -> Result<u32, StepError> {
        let mode = (opcode >> 3) & 0b111;
        let reg = opcode & 0b111;

        // MOVEP: 0000 ddd 1 0z 001 rrr (bit 8=1, mode=001, bit7=dir, bit6=size)
        if opcode & 0x0100 != 0 && mode == 0b001 {
            return self.op_movep(bus, opcode);
        }

        // MOVES (68010+ only): 0000 1110 ss mmm rrr — fixed high byte
        // 0x0E, disjoint from the MOVEP/BTST family above/below (which all
        // require bit8=1, whereas 0x0E00 has it at 0). On a real MC68000
        // this opcode doesn't exist: "illegal instruction" exception
        // (vector 4), the same CPU-detection probe as MOVEC/RTD/MOVE from
        // CCR.
        if opcode & 0xFF00 == 0x0E00 {
            if self.cpu_type == CpuType::M68000 {
                let pc_push = self.pc.wrapping_sub(2);
                self.take_exception(bus, 4, pc_push);
                return Ok(34);
            }
            return self.op_moves(bus, opcode, mode, reg);
        }

        // BTST/BCHG/BCLR/BSET Dn,<ea>: 0000 rrr 1 tt mmm rrr
        // Yacht.txt lines 226-312: mem cost = base+ea_extra; register cost
        // depends on the bit number (< 16 or >= 16) and differs for BCLR.
        if opcode & 0x0100 != 0 && (opcode >> 8) & 0b111 != 0b100 {
            let bit_reg = ((opcode >> 9) & 0b111) as usize;
            let op = (opcode >> 6) & 0b11;
            let is_mem = mode != 0b000;
            let sz = if is_mem { Size::Byte } else { Size::Long };
            let ea = self
                .resolve_ea(bus, mode, reg, sz)
                .ok_or(StepError::IllegalAddressing)?;
            let ea_extra = self.ea_extra_cycles;
            let val = ae_read(ea.read(self, bus, sz))?;
            let modulus = if is_mem { 8 } else { 32 };
            let bit = self.d[bit_reg] as u32 % modulus;
            let mask = 1u32 << bit;
            self.set_flag(ccr::Z, val & mask == 0);
            let result = match op {
                0b00 => {
                    // BTST — no write. #imm-as-target case (rare but valid:
                    // BTST Dn,#imm reads the immediate as a byte to test):
                    // +2 relative to the generic EA table — calibrated and
                    // verified exact against ProcessorTests.
                    let imm_bonus = if mode == 0b111 && reg == 0b100 { 2 } else { 0 };
                    return Ok(if is_mem {
                        4 + ea_extra + imm_bonus
                    } else {
                        6
                    });
                }
                0b01 => val ^ mask,  // BCHG
                0b10 => val & !mask, // BCLR
                0b11 => val | mask,  // BSET
                _ => unreachable!(),
            };
            ae_write(ea.write(self, bus, sz, result))?;
            let reg_cost = if op == 0b10 {
                // BCLR
                if bit < 16 { 8 } else { 10 }
            } else {
                // BCHG / BSET
                if bit < 16 { 6 } else { 8 }
            };
            return Ok(if is_mem { 8 + ea_extra } else { reg_cost });
        }

        // BTST/BCHG/BCLR/BSET #imm,<ea>: 0000 1000 xx mmm rrr (bits 11-9 = 100)
        if (opcode >> 9) & 0b111 == 0b100 {
            let op = (opcode >> 6) & 0b11;
            let bit_num = self.fetch_word(bus) as u32 & 0xFF;
            let is_mem = mode != 0b000;
            let sz = if is_mem { Size::Byte } else { Size::Long };
            let ea = self
                .resolve_ea(bus, mode, reg, sz)
                .ok_or(StepError::IllegalAddressing)?;
            let ea_extra = self.ea_extra_cycles;
            let val = ae_read(ea.read(self, bus, sz))?;
            let modulus = if is_mem { 8 } else { 32 };
            let bit = bit_num % modulus;
            let mask = 1u32 << bit;
            self.set_flag(ccr::Z, val & mask == 0);
            let result = match op {
                0b00 => {
                    // BTST
                    return Ok(if is_mem { 8 + ea_extra } else { 10 });
                }
                0b01 => val ^ mask,  // BCHG
                0b10 => val & !mask, // BCLR
                0b11 => val | mask,  // BSET
                _ => unreachable!(),
            };
            ae_write(ea.write(self, bus, sz, result))?;
            let reg_cost = if op == 0b10 {
                // BCLR
                if bit < 16 { 12 } else { 14 }
            } else {
                // BCHG / BSET
                if bit < 16 { 10 } else { 12 }
            };
            return Ok(if is_mem { 12 + ea_extra } else { reg_cost });
        }

        // ORI/ANDI/EORI to CCR/SR : opcodes fixes
        const CCR_VALID: u16 = 0x001F;
        match opcode {
            0x003C => {
                let imm = self.fetch_word(bus);
                self.write_sr((self.sr | (imm & CCR_VALID)) & 0xA71F);
                return Ok(20);
            } // ORI to CCR
            0x007C => {
                if let Some(c) = self.check_privilege(bus) {
                    return Ok(c);
                }
                let imm = self.fetch_word(bus);
                self.write_sr((self.sr | imm) & 0xA71F);
                return Ok(20);
            } // ORI to SR
            0x023C => {
                let imm = self.fetch_word(bus);
                self.write_sr((self.sr & ((imm & CCR_VALID) | !CCR_VALID)) & 0xA71F);
                return Ok(20);
            } // ANDI to CCR
            0x027C => {
                if let Some(c) = self.check_privilege(bus) {
                    return Ok(c);
                }
                let imm = self.fetch_word(bus);
                self.write_sr((self.sr & imm) & 0xA71F);
                return Ok(20);
            } // ANDI to SR
            0x0A3C => {
                let imm = self.fetch_word(bus);
                self.write_sr((self.sr ^ (imm & CCR_VALID)) & 0xA71F);
                return Ok(20);
            } // EORI to CCR
            0x0A7C => {
                if let Some(c) = self.check_privilege(bus) {
                    return Ok(c);
                }
                let imm = self.fetch_word(bus);
                self.write_sr((self.sr ^ imm) & 0xA71F);
                return Ok(20);
            } // EORI to SR
            _ => {}
        }

        // ORI/ANDI/SUBI/ADDI/EORI/CMPI
        let op = (opcode >> 9) & 0b111;
        let size = Size::from_bits(opcode >> 6).ok_or(StepError::IllegalAddressing)?;
        let imm = match size {
            Size::Byte => self.fetch_word(bus) as u8 as u32,
            Size::Word => self.fetch_word(bus) as u32,
            Size::Long => self.fetch_long(bus),
        };

        let ea = self
            .resolve_ea(bus, mode, reg, size)
            .ok_or(StepError::IllegalAddressing)?;
        let ea_extra = self.ea_extra_cycles;
        let is_dn = mode == 0b000;
        // All opcodes in this group (including CMPI, read-only) fetch an
        // immediate before resolving the EA: the fault therefore has the
        // same fixed prefix of 8 (opcode fetch + immediate fetch),
        // regardless of size, whether there's a rewrite or not. Confirmed
        // via ProcessorTests: CMP.l "#,-(An)" (CMPI.l grouped in the CMP.l
        // file) follows "8+ea_extra+50", just as EORI.w/EORI.l both follow
        // the same formula.
        self.fault_prefix = 8;
        let val = ae_read(ea.read(self, bus, size))?;
        let long = size == Size::Long;

        match op {
            0b000 | 0b001 | 0b010 | 0b011 | 0b101 => {
                // ORI/ANDI/SUBI/ADDI/EORI
                let r = match op {
                    0b000 => {
                        let r = val | imm;
                        self.set_logic_flags(r, size);
                        r
                    }
                    0b001 => {
                        let r = val & imm;
                        self.set_logic_flags(r, size);
                        r
                    }
                    0b010 => self.sub_with_flags(val, imm, size),
                    0b011 => self.add_with_flags(val, imm, size),
                    0b101 => {
                        let r = val ^ imm;
                        self.set_logic_flags(r, size);
                        r
                    }
                    _ => unreachable!(),
                };
                ae_write(ea.write(self, bus, size, r))?;
                Ok(if long {
                    if is_dn { 16 } else { 20 + ea_extra }
                } else if is_dn {
                    8
                } else {
                    12 + ea_extra
                })
            }
            0b110 => {
                // CMPI — read-only
                self.cmp_flags(val, imm, size);
                Ok(if long {
                    if is_dn { 14 } else { 12 + ea_extra }
                } else {
                    8 + ea_extra // uniform, including Dn (ea_extra=0 then)
                })
            }
            _ => Err(StepError::Unimplemented(opcode)),
        }
    }

    // =========================================================================
    // MOVE / MOVEA
    // =========================================================================

    // ABCD: BCD addition with X
    fn op_abcd(&mut self, bus: &mut impl Bus, opcode: u16) -> Result<u32, StepError> {
        let dst_reg = ((opcode >> 9) & 0b111) as usize;
        let src_reg = (opcode & 0b111) as usize;
        let mem_mode = opcode & 0x0008 != 0;
        let x = if self.flag(ccr::X) { 1u32 } else { 0 };

        let (src_val, dst_val, dst_op) = if mem_mode {
            // A7 stays word-aligned: decrement by 2 even in byte mode.
            let s_step = if src_reg == 7 { 2 } else { 1 };
            self.a[src_reg] = self.a[src_reg].wrapping_sub(s_step);
            let src_addr = self.a[src_reg] & ADDR_MASK;
            let d_step = if dst_reg == 7 { 2 } else { 1 };
            self.a[dst_reg] = self.a[dst_reg].wrapping_sub(d_step);
            let dst_addr = self.a[dst_reg] & ADDR_MASK;
            let s = bus.read8(src_addr) as u32;
            let d = bus.read8(dst_addr) as u32;
            (s, d, Operand::Memory(dst_addr))
        } else {
            (
                self.d[src_reg] & 0xFF,
                self.d[dst_reg] & 0xFF,
                Operand::DataReg(dst_reg),
            )
        };

        // 68000 BCD algorithm (MAME behavior)
        let src = src_val;
        let dst = dst_val;

        // Low nibble: correction if the sum > 9
        let lo = (src & 0x0F) + (dst & 0x0F) + x;
        let lo_correct = if lo > 9 { 6u32 } else { 0 };

        // High nibble: with carry from the low nibble
        let hi = (src >> 4) + (dst >> 4) + (lo + lo_correct) / 16;
        let hi_correct = if hi > 9 { 6u32 } else { 0 };

        // Complete result
        let raw_sum = src + dst + x;
        let corrected = raw_sum + lo_correct + (hi_correct << 4);
        let result = corrected & 0xFF;
        let carry = corrected >= 0x100;

        self.set_flag(ccr::C, carry);
        self.set_flag(ccr::X, carry);
        // V (68000 silicon): bit 7 flipping from 0 to 1 during the decimal correction.
        let raw_byte = raw_sum & 0xFF;
        self.set_flag(ccr::V, (!raw_byte & result) & 0x80 != 0);
        if result != 0 {
            self.set_flag(ccr::Z, false);
        }
        self.set_flag(ccr::N, result & 0x80 != 0);

        ae_write(dst_op.write(self, bus, Size::Byte, result))?;
        Ok(if mem_mode { 18 } else { 6 })
    }

    // SBCD: BCD subtraction with X
    fn op_sbcd(&mut self, bus: &mut impl Bus, opcode: u16) -> Result<u32, StepError> {
        let dst_reg = ((opcode >> 9) & 0b111) as usize;
        let src_reg = (opcode & 0b111) as usize;
        let mem_mode = opcode & 0x0008 != 0;
        let x = if self.flag(ccr::X) { 1u32 } else { 0 };

        let (src_val, dst_val, dst_op) = if mem_mode {
            // A7 stays word-aligned: decrement by 2 even in byte mode.
            let s_step = if src_reg == 7 { 2 } else { 1 };
            self.a[src_reg] = self.a[src_reg].wrapping_sub(s_step);
            let src_addr = self.a[src_reg] & ADDR_MASK;
            let d_step = if dst_reg == 7 { 2 } else { 1 };
            self.a[dst_reg] = self.a[dst_reg].wrapping_sub(d_step);
            let dst_addr = self.a[dst_reg] & ADDR_MASK;
            let s = bus.read8(src_addr) as u32;
            let d = bus.read8(dst_addr) as u32;
            (s, d, Operand::Memory(dst_addr))
        } else {
            (
                self.d[src_reg] & 0xFF,
                self.d[dst_reg] & 0xFF,
                Operand::DataReg(dst_reg),
            )
        };

        // BCD subtraction: dst - src - X
        let dst = dst_val as i32;
        let src = src_val as i32;
        let xi = x as i32;

        let lo = (dst & 0x0F) - (src & 0x0F) - xi;
        let lo_borrow = lo < 0;
        let lo_correct = if lo_borrow { 6i32 } else { 0 };

        let hi = (dst >> 4) - (src >> 4) - (if lo_borrow { 1 } else { 0 });
        let hi_borrow = hi < 0;
        let hi_correct = if hi_borrow { 6i32 } else { 0 };

        let raw = (dst - src - xi + 0x100) & 0xFF;
        let corrected_raw = raw - lo_correct - (hi_correct << 4);
        let corrected = corrected_raw & 0xFF;
        let result = corrected as u32;
        // The silicon detects the borrow on the value *before* the 0xFF
        // mask: with invalid BCD nibbles (>9), `hi_borrow` (computed
        // nibble by nibble) can stay false even though the decimal
        // correction still pushes the result below zero (e.g. dst=0xb2,
        // src=0xad → hi=0, but corrected_raw=-1). TomHarte shows the real
        // C/X also follows this second case, hence the OR of both
        // conditions.
        let actual_borrow = hi_borrow || corrected_raw < 0;

        self.set_flag(ccr::C, actual_borrow);
        self.set_flag(ccr::X, actual_borrow);
        // V (68000 silicon): bit 7 flipping from 1 to 0 during the decimal correction.
        self.set_flag(ccr::V, (raw & !corrected) & 0x80 != 0);
        if result != 0 {
            self.set_flag(ccr::Z, false);
        }
        self.set_flag(ccr::N, result & 0x80 != 0);
        ae_write(dst_op.write(self, bus, Size::Byte, result))?;
        Ok(if mem_mode { 18 } else { 6 })
    }

    // NBCD: negate BCD (0 - dst - X)
    fn op_nbcd(&mut self, bus: &mut impl Bus, mode: u16, reg: u16) -> Result<u32, StepError> {
        let x = if self.flag(ccr::X) { 1u32 } else { 0 };
        let ea = self
            .resolve_ea(bus, mode, reg, Size::Byte)
            .ok_or(StepError::IllegalAddressing)?;
        let ea_extra = self.ea_extra_cycles;
        let is_dn = mode == 0b000;
        let dst = ae_read(ea.read(self, bus, Size::Byte))?;

        // NBCD = 0 - dst - X (BCD)
        let d = dst as i32;
        let xi = x as i32;

        let lo = -(d & 0x0F) - xi;
        let lo_borrow = lo < 0;
        let lo_correct = if lo_borrow { 6i32 } else { 0 };

        let hi = -(d >> 4) - (if lo_borrow { 1 } else { 0 });
        let hi_borrow = hi < 0;
        let hi_correct = if hi_borrow { 6i32 } else { 0 };

        let raw = (0 - d - xi + 0x100) & 0xFF;
        let corrected = (raw - lo_correct - (hi_correct << 4)) & 0xFF;
        let result = corrected as u32;
        let actual_borrow = hi_borrow;

        self.set_flag(ccr::C, actual_borrow);
        self.set_flag(ccr::X, actual_borrow);
        // V (68000 silicon): bit 7 flipping from 1 to 0 during the decimal correction.
        self.set_flag(ccr::V, (raw & !corrected) & 0x80 != 0);
        if result != 0 {
            self.set_flag(ccr::Z, false);
        }
        self.set_flag(ccr::N, result & 0x80 != 0);

        ae_write(ea.write(self, bus, Size::Byte, result))?;
        Ok(if is_dn { 6 } else { 8 + ea_extra })
    }

    // MOVEP: 0000 ddd 1 0z 001 rrr (line 0, dispatched before MOVE)
    // Encoded in op_line_0 for z=0 (word) and z=1 (long), d/r from registers
    fn op_movep(&mut self, bus: &mut impl Bus, opcode: u16) -> Result<u32, StepError> {
        let dreg = ((opcode >> 9) & 0b111) as usize;
        let areg = (opcode & 0b111) as usize;
        let to_mem = opcode & 0x0080 != 0; // bit 7: 0=mem→Dn, 1=Dn→mem
        let long = opcode & 0x0040 != 0; // bit 6: 0=word, 1=long
        let disp = self.fetch_word(bus) as i16 as i32;
        let base = (self.a[areg] as i32).wrapping_add(disp) as u32 & ADDR_MASK;

        if to_mem {
            if long {
                bus.write8(base, (self.d[dreg] >> 24) as u8);
                bus.write8(base + 2, (self.d[dreg] >> 16) as u8);
                bus.write8(base + 4, (self.d[dreg] >> 8) as u8);
                bus.write8(base + 6, self.d[dreg] as u8);
            } else {
                bus.write8(base, (self.d[dreg] >> 8) as u8);
                bus.write8(base + 2, self.d[dreg] as u8);
            }
        } else if long {
            let b0 = bus.read8(base) as u32;
            let b1 = bus.read8(base + 2) as u32;
            let b2 = bus.read8(base + 4) as u32;
            let b3 = bus.read8(base + 6) as u32;
            self.d[dreg] = (b0 << 24) | (b1 << 16) | (b2 << 8) | b3;
        } else {
            let b0 = bus.read8(base) as u32;
            let b1 = bus.read8(base + 2) as u32;
            let word = (b0 << 8) | b1;
            self.d[dreg] = (self.d[dreg] & 0xFFFF_0000) | word;
        }
        Ok(if long { 24 } else { 16 })
    }

    /// MOVES (68010+): `0000 1110 ss mmm rrr` + extension word (bit 15 =
    /// direction, 1=`Rn,<ea>` with DFC / 0=`<ea>,Rn` with SFC; bit 11 =
    /// A/D; bits 14-12 = general register). Privileged.
    ///
    /// Deliberate and documented simplification: `sfc`/`dfc` are accepted
    /// and stored (see MOVEC) but do not actually alter the accessed
    /// address space — the current `Bus` trait has no notion of function
    /// code. Functionally equivalent to MOVE for a single address space,
    /// exact as long as no multi-space MMU exists (to revisit with the
    /// NeXT PMMU). Cycle cost not calibrated (no SingleStepTests vectors
    /// available for the 68010): a plausible value taken from the same
    /// order of magnitude as a memory↔register MOVE, NOT verified against
    /// a hardware reference.
    fn op_moves(&mut self, bus: &mut impl Bus, opcode: u16, mode: u16, reg: u16) -> Result<u32, StepError> {
        if let Some(c) = self.check_privilege(bus) {
            return Ok(c);
        }
        let size = Size::from_bits(opcode >> 6).ok_or(StepError::IllegalAddressing)?;
        let ext = self.fetch_word(bus);
        let to_memory = ext & 0x8000 != 0; // bit 15: 1 = Rn -> <ea>, 0 = <ea> -> Rn
        let is_addr = ext & 0x0800 != 0; // bit 11: A/D
        let greg = ((ext >> 12) & 0b111) as usize;
        let greg_operand = if is_addr { Operand::AddrReg(greg) } else { Operand::DataReg(greg) };
        let ea = self
            .resolve_ea(bus, mode, reg, size)
            .ok_or(StepError::IllegalAddressing)?;
        let ea_extra = self.ea_extra_cycles;
        if !matches!(ea, Operand::Memory(_)) {
            return Err(StepError::IllegalAddressing);
        }
        if to_memory {
            let val = ae_read(greg_operand.read(self, bus, size))?;
            ae_write(ea.write(self, bus, size, val))?;
        } else {
            let val = ae_read(ea.read(self, bus, size))?;
            ae_write(greg_operand.write(self, bus, size, val))?;
        }
        Ok(16 + ea_extra)
    }

    fn op_move(&mut self, bus: &mut impl Bus, opcode: u16) -> Result<u32, StepError> {
        let size = match opcode >> 12 {
            0b0001 => Size::Byte,
            0b0011 => Size::Word,
            0b0010 => Size::Long,
            _ => unreachable!(),
        };
        // PC right after fetching the opcode (before any src/dst extension
        // word). Used to compute frame_pc for MOVE.l write AEs.
        let pc_after_opcode = self.pc;
        let src_mode = (opcode >> 3) & 0b111;
        let src_reg = opcode & 0b111;
        let src = self
            .resolve_ea(bus, src_mode, src_reg, size)
            .ok_or(StepError::IllegalAddressing)?;
        let value = ae_read(src.read(self, bus, size))?;
        // Captured before the destination's resolve_ea overwrites
        // ea_extra_cycles: only the SOURCE's EA-computation cost is added
        // (the dst write cost is covered by move_dst_base, derived from
        // Yacht.txt, not the generic term).
        let src_extra = self.ea_extra_cycles;

        let dst_reg = (opcode >> 9) & 0b111;
        let dst_mode = (opcode >> 6) & 0b111;
        let dst = self
            .resolve_ea(bus, dst_mode, dst_reg, size)
            .ok_or(StepError::IllegalAddressing)?;

        // For a -(An) destination, ea_frame_pc has already accounted for
        // the predecrement's prefetch. Operand::write adds +2 (prefetch
        // before the write cycle), which would double-count it. We cancel
        // that +2 before the write so it gets re-added and reaches the
        // correct value.
        if dst_mode == 0b100 {
            self.ea_frame_pc = self.ea_frame_pc.wrapping_sub(2);
        }

        let sr_before_flags = self.sr;
        if dst_mode != 0b001 {
            self.set_logic_flags(value, size);
        }

        // For MOVE.w/b with dst -(An), the 68000 does a Prefetch() before
        // the write cycle. This Prefetch advances the pipeline: the IR in
        // the write-AE frame = the next word in the stream, not the
        // opcode. That word = bus.read16(self.pc) at this point (after
        // consuming the extension words).
        if dst_mode == 0b100 && size != Size::Long {
            self.write_ae_ir = Some(bus.read16(self.pc & crate::cpu::ADDR_MASK));
        }

        // MOVE.l -(An): the CLK writes the LSW first (at An-2), then the
        // MSW (at An-4). fault_addr = the full 32-bit address (before the
        // 24-bit mask).
        let write_result = if size == Size::Long && dst_mode == 0b100 {
            // After resolve_ea, An has already been decremented by 4.
            // An_current = An_initial - 4; LSW addr (32-bit) = An_initial - 2 = An_current + 2
            let an_full = self.a[dst_reg as usize];
            let lsw_addr_full = an_full.wrapping_add(2);
            let msw_addr_full = an_full;
            let lsw_addr = lsw_addr_full & crate::cpu::ADDR_MASK;
            let msw_addr = msw_addr_full & crate::cpu::ADDR_MASK;
            let frame_pc_ae = self.ea_frame_pc.wrapping_add(2);
            if lsw_addr & 1 != 0 {
                Err((lsw_addr_full, frame_pc_ae))
            } else {
                bus.write16(lsw_addr, value as u16);
                if msw_addr & 1 != 0 {
                    Err((msw_addr_full, frame_pc_ae))
                } else {
                    bus.write16(msw_addr, (value >> 16) as u16);
                    Ok(())
                }
            }
        } else {
            dst.write(self, bus, size, value)
        };
        if let Err((fault_addr, _)) = write_result {
            // Write AE: the CCR bits saved in the frame depend on dst_mode
            // and size. TomHarte analysis (267 Dn/An src cases, 0 fail):
            //   MOVE.w: mask=0x10 (X alone preserved) | N<<3 | Z<<2 — universal for all modes
            //   MOVE.l: mask varies by dst_mode:
            //     dm=0,2,3 (Dn, (An), (An)+) → SR unchanged
            //     dm=4,7   (-(An), abs)       → mask=0x10 | NZ
            //     dm=5,6   ((d16,An),(d8,Xn)) → mask=0x13 (X+V+C) | NZ
            let sr_for_frame = if dst_mode == 0b001 {
                // MOVEA: no flag update, SR always unchanged
                sr_before_flags
            } else if size == Size::Long {
                // imm (sm=7,sr=4) behaves like Dn/An: already in the pipeline
                let src_is_reg = src_mode <= 0b001 || (src_mode == 0b111 && src_reg == 0b100);
                // For dm=2,3 with src=memory, or sm=3+(An)+ src with dst=abs.l: flags on the LSW.
                // Otherwise: flags on the full 32-bit value.
                let is_dst_abs_long = dst_mode == 0b111 && dst_reg == 0b001;
                let use_lsw = !src_is_reg
                    && (dst_mode == 0b010
                        || dst_mode == 0b011
                        || (src_mode == 0b011 && is_dst_abs_long));
                let (n, z) = if use_lsw {
                    let lsw = value & 0xFFFF;
                    ((lsw >> 15) & 1, if lsw == 0 { 1u32 } else { 0 })
                } else {
                    ((value >> 31) & 1, if value == 0 { 1u32 } else { 0 })
                };
                match dst_mode {
                    // (An) and (An)+: unchanged if src=Dn/An, X+NZ(LSW) if src=memory
                    0b010 | 0b011 => {
                        if src_is_reg {
                            sr_before_flags
                        } else {
                            (sr_before_flags & 0xFFE0)
                                | (sr_before_flags & 0x0010)
                                | ((n as u16) << 3)
                                | ((z as u16) << 2)
                        }
                    }
                    // (d16,An), (d8,Xn): X+V+C if src=Dn/An, X alone if src=memory
                    0b101 | 0b110 => {
                        let preserve = if src_is_reg { 0x0013u16 } else { 0x0010u16 };
                        (sr_before_flags & 0xFFE0)
                            | (sr_before_flags & preserve)
                            | ((n as u16) << 3)
                            | ((z as u16) << 2)
                    }
                    // -(An), abs → X alone preserved (all sources)
                    _ => {
                        (sr_before_flags & 0xFFE0)
                            | (sr_before_flags & 0x0010)
                            | ((n as u16) << 3)
                            | ((z as u16) << 2)
                    }
                }
            } else {
                // MOVE.b and MOVE.w: X alone preserved for all modes
                let msb = size.msb();
                let n: u16 = if value & msb != 0 { 1 } else { 0 };
                let z: u16 = if value & size.mask() == 0 { 1 } else { 0 };
                (sr_before_flags & 0xFFE0) | (sr_before_flags & 0x0010) | (n << 3) | (z << 2)
            };

            // For (An)+ MOVE.w/b: the post-increment must be undone on a
            // write AE. For MOVE.l to an odd address, the post-inc is
            // never committed (see resolve_ea mode 3).
            if dst_mode == 0b011 && size != Size::Long {
                let step = if dst_reg == 7 && size == Size::Byte {
                    2
                } else {
                    size.bytes()
                };
                self.a[dst_reg as usize] = self.a[dst_reg as usize].wrapping_sub(step);
            }
            // For -(An) MOVE.l: undo the dst predecrement on a write AE.
            // Done before take_address_error_full because if dst=A7, the
            // rollback adjusts the SP base.
            if dst_mode == 0b100 && size == Size::Long {
                self.a[dst_reg as usize] = self.a[dst_reg as usize].wrapping_add(size.bytes());
            }

            // For a MOVE write AE, frame_pc depends on the number of SRC
            // extension words (bus pipeline cycle). Rule:
            // frame_pc = pc_after_opcode + 2 + 2 × nb_ext_src_words
            //   nb_ext_src = 0 if src mode has no extension (Dn/An/indirect without disp)
            //               = 1 if src has 1 ext word (d16,An / d8,An,Xn / abs.w / PC-rel / imm b/w)
            //               = 2 if src has 2 ext words (abs.l / imm.l)
            // Exception: src=Dn/An (mode 0,1) with dst=abs.l (dm=7, dr=1) → +2 extra
            // Same rule for MOVE.l, MOVE.w, MOVE.b
            let nb_src_ext: u32 = match src_mode {
                0b101 | 0b110 => 1, // d16,An or d8,An,Xn
                0b111 => match src_reg {
                    0b000 | 0b010 | 0b011 => 1, // abs.w, d16,PC, d8,PC,Xn
                    0b001 => 2,                 // abs.l
                    0b100 => {
                        if size == Size::Long {
                            2
                        } else {
                            1
                        }
                    } // imm.l = 2, imm.b/w = 1
                    _ => 0,
                },
                _ => 0, // Dn, An, (An), (An)+, -(An)
            };
            let is_src_reg = src_mode <= 0b001;
            let is_dst_abs_long = dst_mode == 0b111 && dst_reg == 0b001;
            let extra: u32 = if is_src_reg && is_dst_abs_long { 2 } else { 0 };
            let frame_pc = pc_after_opcode.wrapping_add(2 + nb_src_ext * 2 + extra);

            self.sr = sr_for_frame;
            self.take_address_error_full(bus, fault_addr, true, Some(frame_pc), false);
            // Cost = cost already spent reading the source (paid in full,
            // the read succeeded) + dst write cost reduced to its Word
            // variant (only a single 16-bit transfer is attempted before
            // the odd address is detected — the high half of a Long is
            // never paid) + 50 (Group 0 dispatch). -(An) adds +4: an extra
            // prefetch before the write cycle (cf. write_ae_ir above), not
            // covered by the normal pipeline once the exception fires.
            let predec_extra = if dst_mode == 0b100 { 4 } else { 0 };
            // (xxx).L: an extra -4 if the source required a memory read
            // (not direct Dn/An) — the overlap of fetching the 2 address
            // extension words with the source read cycle changes the
            // timing of the failed write. Found and verified by exhaustive
            // search against the 27 TomHarte dst=(xxx).L address-error
            // cases (0 mismatches): no correction if src is Dn/An, -4
            // otherwise.
            let abs_long_mem_src_extra = if is_dst_abs_long && !is_src_reg { -4i32 } else { 0 };
            let dst_ae_cost = (src_extra as i32
                + move_dst_base(dst_mode, dst_reg, Size::Word) as i32
                + predec_extra as i32
                + abs_long_mem_src_extra
                + 50) as u32;
            return Ok(dst_ae_cost);
        }
        Ok(src_extra + move_dst_base(dst_mode, dst_reg, size))
    }

    // =========================================================================
    // Line 0100: miscellaneous
    // =========================================================================

    fn op_line_4(&mut self, bus: &mut impl Bus, opcode: u16) -> Result<u32, StepError> {
        let mode = (opcode >> 3) & 0b111;
        let reg = opcode & 0b111;

        match opcode {
            0x4E71 => return Ok(4), // NOP
            0x4E70 => {
                // RESET (privileged) — asserts /RESET on the bus (124 cycles)
                if let Some(c) = self.check_privilege(bus) {
                    return Ok(c);
                }
                bus.reset_bus();
                return Ok(132);
            }
            0x4E75 => return self.op_rts(bus),
            0x4E74 => {
                // RTD (68010+ only): like RTS, but then pops an immediate
                // displacement from the stack — frees the call parameters
                // without a separate ADD on the caller's side. Not
                // privileged. On a real MC68000 this opcode doesn't exist:
                // "illegal instruction" exception (vector 4), the same
                // CPU-detection probe as MOVEC/MOVE from CCR above/below.
                if self.cpu_type == CpuType::M68000 {
                    let pc_push = self.pc.wrapping_sub(2);
                    self.take_exception(bus, 4, pc_push);
                    return Ok(34);
                }
                return self.op_rtd(bus);
            }
            0x4E73 => {
                // RTE (privileged)
                if let Some(c) = self.check_privilege(bus) {
                    return Ok(c);
                }
                return self.op_rte(bus);
            }
            0x4E77 => return self.op_rtr(bus),
            0x4AFC => {
                // ILLEGAL: exception vector 4, pushes opcode_addr
                let pc_push = self.pc.wrapping_sub(2);
                self.take_exception(bus, 4, pc_push);
                return Ok(34);
            }
            0x4E7A | 0x4E7B => {
                // MOVEC (68010+ only): on a real MC68000, this opcode
                // doesn't exist — "illegal instruction" exception
                // (vector 4), like ILLEGAL above. This is exactly what
                // software (TOS included, to detect the CPU type at boot)
                // deliberately exploits: install a temporary handler at
                // vector 4, then execute MOVEC to trigger it on a 68000.
                if self.cpu_type == CpuType::M68000 {
                    let pc_push = self.pc.wrapping_sub(2);
                    self.take_exception(bus, 4, pc_push);
                    return Ok(34);
                }
                if let Some(c) = self.check_privilege(bus) {
                    return Ok(c);
                }
                // to_control: true = general register → control register
                // (0x4E7B), false = the reverse (0x4E7A).
                let to_control = opcode == 0x4E7B;
                let ext = self.fetch_word(bus);
                let is_addr = ext & 0x8000 != 0; // bit 15: A/D
                let greg = ((ext >> 12) & 0b111) as usize;
                let rc = ext & 0x0FFF;
                // 68010 subset: SFC/DFC/USP/VBR. Other selectors
                // (CACR/CAAR 68020+...) remain "illegal instruction", as a
                // reserved Rc would on real silicon.
                // `greg_val` (the source general register's value, if
                // to_control) is computed BEFORE any mutable reference to
                // a control register, so as not to keep that reference
                // alive across a new call to `self...` (disjoint borrows
                // aren't possible across a method call).
                let greg_val = self.d_or_a(is_addr, greg);
                match rc {
                    0x000 => {
                        if to_control {
                            self.sfc = (greg_val & 0b111) as u8;
                        } else {
                            let v = self.sfc as u32;
                            self.set_d_or_a(is_addr, greg, v);
                        }
                    }
                    0x001 => {
                        if to_control {
                            self.dfc = (greg_val & 0b111) as u8;
                        } else {
                            let v = self.dfc as u32;
                            self.set_d_or_a(is_addr, greg, v);
                        }
                    }
                    0x800 => {
                        if to_control {
                            self.usp = greg_val;
                        } else {
                            self.set_d_or_a(is_addr, greg, self.usp);
                        }
                    }
                    0x801 => {
                        if to_control {
                            self.vbr = greg_val;
                        } else {
                            self.set_d_or_a(is_addr, greg, self.vbr);
                        }
                    }
                    _ => {
                        let pc_push = self.pc.wrapping_sub(2);
                        self.take_exception(bus, 4, pc_push);
                        return Ok(34);
                    }
                }
                return Ok(12);
            }
            0x4E76 => {
                // TRAPV: exception vector 7 if V=1
                if self.flag(ccr::V) {
                    let pc_push = self.pc; // opcode_addr + 2
                    self.take_exception(bus, 7, pc_push);
                    return Ok(34);
                }
                return Ok(4);
            }
            _ => {}
        }

        // TRAP: 0100 1110 0100 vvvv (vector 32+v)
        if opcode & 0xFFF0 == 0x4E40 {
            let v = (opcode & 0xF) as u32;
            let pc_push = self.pc; // PC after opcode fetch = opcode_addr + 2
            self.take_exception(bus, 32 + v, pc_push);
            return Ok(34);
        }
        // LINK: 0100 1110 0101 0rrr
        if opcode & 0xFFF8 == 0x4E50 {
            return self.op_link(bus, reg as usize);
        }
        // UNLK: 0100 1110 0101 1rrr
        if opcode & 0xFFF8 == 0x4E58 {
            return self.op_unlk(bus, reg as usize);
        }
        // MOVE to USP: 0100 1110 0110 0rrr (privileged)
        if opcode & 0xFFF8 == 0x4E60 {
            if let Some(c) = self.check_privilege(bus) {
                return Ok(c);
            }
            self.usp = self.a[reg as usize];
            return Ok(4);
        }
        // MOVE from USP: 0100 1110 0110 1rrr (privileged)
        if opcode & 0xFFF8 == 0x4E68 {
            if let Some(c) = self.check_privilege(bus) {
                return Ok(c);
            }
            self.a[reg as usize] = self.usp;
            return Ok(4);
        }
        // STOP: 0100 1110 0111 0010 (privileged)
        if opcode == 0x4E72 {
            if let Some(c) = self.check_privilege(bus) {
                return Ok(c);
            }
            let new_sr = self.fetch_word(bus);
            self.pc = self.pc.wrapping_sub(4);
            self.write_sr(new_sr & 0xA71F);
            return Ok(4);
        }
        // JSR: 0100 1110 10 mmm rrr
        if opcode & 0b1111_1111_1100_0000 == 0b0100_1110_1000_0000 {
            return self.op_jsr(bus, mode, reg);
        }
        // JMP: 0100 1110 11 mmm rrr
        if opcode & 0b1111_1111_1100_0000 == 0b0100_1110_1100_0000 {
            return self.op_jmp(bus, mode, reg);
        }
        // EXTB.L (68020+ only): 0100 1001 1100 0 rrr — directly extends a
        // byte to a longword (the 68000/68010 only have EXT.W and EXT.L,
        // no direct byte→long form). MUST precede LEA
        // below: LEA's wider mask (0100 aaa 111 mmm rrr, "aaa" wildcard)
        // accidentally matches this same bit pattern, reading it as
        // "LEA D0,A4" — an invalid LEA encoding (direct Dn mode isn't a
        // memory address) that would otherwise fail with
        // `IllegalAddressing` before even reaching this check (found by
        // running `tests/cpu68020.rs`: EXTB.L was indeed returning that
        // error with the check placed after CHK/EXT, further down).
        if opcode & 0b1111_1111_1111_1000 == 0b0100_1001_1100_0000 {
            if self.cpu_type != CpuType::M68020 {
                let pc_push = self.pc.wrapping_sub(2);
                self.take_exception(bus, 4, pc_push);
                return Ok(34);
            }
            let r = reg as usize;
            self.d[r] = self.d[r] as u8 as i8 as i32 as u32;
            self.set_logic_flags(self.d[r], Size::Long);
            return Ok(4);
        }
        // LEA: 0100 aaa 111 mmm rrr
        if opcode & 0b1111_0001_1100_0000 == 0b0100_0001_1100_0000 {
            return self.op_lea(bus, opcode);
        }
        // SWAP: 0100 1000 0100 0rrr — must precede PEA (same 0xffc0 mask)
        if opcode & 0b1111_1111_1111_1000 == 0b0100_1000_0100_0000 {
            let r = reg as usize;
            self.d[r] = (self.d[r] >> 16) | (self.d[r] << 16);
            self.set_logic_flags(self.d[r], Size::Long);
            return Ok(4);
        }
        // PEA: 0100 1000 01 mmm rrr
        if opcode & 0b1111_1111_1100_0000 == 0b0100_1000_0100_0000 {
            return self.op_pea(bus, mode, reg);
        }
        // CHK: 0100 rrr 110 mmm rrr
        if opcode & 0b1111_0001_1100_0000 == 0b0100_0001_1000_0000 {
            let dn_reg = ((opcode >> 9) & 0b111) as usize;
            let ea = self
                .resolve_ea(bus, mode, reg, Size::Word)
                .ok_or(StepError::IllegalAddressing)?;
            let ea_extra = self.ea_extra_cycles;
            let upper = ae_read(ea.read(self, bus, Size::Word))? as i16;
            let dn = (self.d[dn_reg] & 0xFFFF) as i16;
            if dn < 0 {
                // Clear N,V,Z,C then set N
                self.sr &= !0x000F;
                self.set_flag(ccr::N, true);
                let pc_push = self.pc; // opcode_addr + 2 (after opcode fetch + EA)
                self.take_exception(bus, 6, pc_push);
                // The cost of the "dn<0" trap depends on the result of the
                // internal dn-upper subtraction (like a CMP), NOT on the
                // true mathematical comparison: when dn is very negative
                // and upper very positive, dn-upper overflows the signed
                // 16-bit range and its sign bit (after carry/truncation)
                // can differ from the "true" sign of dn-upper. Found and
                // verified by exhaustive search against the 181 TomHarte
                // "dn<0" cases (0 mismatches): 40 cycles if (dn-upper),
                // truncated to signed 16 bits, is <= 0; 38 otherwise. An
                // earlier attempt at "always 38" made the score worse
                // (388 → 402 failures) for lack of identifying this rule.
                let diff = dn.wrapping_sub(upper);
                return Ok((if diff <= 0 { 40 } else { 38 }) + ea_extra);
            } else if dn > upper {
                self.sr &= !0x000F;
                let pc_push = self.pc;
                self.take_exception(bus, 6, pc_push);
                return Ok(38 + ea_extra);
            }
            // In range: clear N,V,Z,C (MAME behavior)
            self.sr &= !0x000F;
            return Ok(10 + ea_extra);
        }
        // MULU.L/MULS.L (68020+ only): 0100 1100 00 mmm rrr + extension
        // word. Unlike the 16×16→32 MULU/MULS above (`op_line_c`), the .L
        // form is 32×32→32 (truncated result, overflow check) or
        // 32×32→64 (Dh:Dl pair, never overflows) depending on the
        // extension word: bits14-12=Dh, bit11=signed, bit10=64-bit result
        // (pair) if set otherwise 32-bit (Dl alone), bits2-0=Dl. Cycle
        // costs not calibrated (no 68020 conformance vectors available):
        // the published Motorola value taken as-is, not verified against
        // a hardware reference.
        if opcode & 0b1111_1111_1100_0000 == 0b0100_1100_0000_0000 {
            if self.cpu_type != CpuType::M68020 {
                let pc_push = self.pc.wrapping_sub(2);
                self.take_exception(bus, 4, pc_push);
                return Ok(34);
            }
            let src = self
                .resolve_ea(bus, mode, reg, Size::Long)
                .ok_or(StepError::IllegalAddressing)?;
            let ea_extra = self.ea_extra_cycles;
            let src_val = ae_read(src.read(self, bus, Size::Long))?;
            let ext = self.fetch_word(bus);
            let dh = ((ext >> 12) & 0b111) as usize;
            let signed = ext & 0x0800 != 0;
            let result_64 = ext & 0x0400 != 0;
            let dl = (ext & 0b111) as usize;
            self.set_flag(ccr::C, false);
            if signed {
                let product = (self.d[dl] as i32 as i64) * (src_val as i32 as i64);
                if result_64 {
                    self.d[dh] = (product >> 32) as u32;
                    self.d[dl] = product as u32;
                    self.set_flag(ccr::N, product < 0);
                    self.set_flag(ccr::Z, product == 0);
                    self.set_flag(ccr::V, false);
                } else {
                    let truncated = product as i32;
                    self.d[dl] = truncated as u32;
                    self.set_flag(ccr::N, truncated < 0);
                    self.set_flag(ccr::Z, truncated == 0);
                    self.set_flag(ccr::V, product != truncated as i64);
                }
            } else {
                let product = (self.d[dl] as u64) * (src_val as u64);
                if result_64 {
                    self.d[dh] = (product >> 32) as u32;
                    self.d[dl] = product as u32;
                    self.set_flag(ccr::N, product & 0x8000_0000_0000_0000 != 0);
                    self.set_flag(ccr::Z, product == 0);
                    self.set_flag(ccr::V, false);
                } else {
                    let truncated = product as u32;
                    self.d[dl] = truncated;
                    self.set_flag(ccr::N, truncated & 0x8000_0000 != 0);
                    self.set_flag(ccr::Z, truncated == 0);
                    self.set_flag(ccr::V, product != truncated as u64);
                }
            }
            return Ok(if result_64 { 44 } else { 42 } + ea_extra);
        }
        // DIVU.L/DIVS.L (68020+ only): 0100 1100 01 mmm rrr + extension
        // word. bits14-12=Dr (remainder), bit11=signed, bit10=64-bit
        // dividend (Dr:Dq pair, Dr=high order) if set otherwise 32-bit
        // dividend (Dq alone, Dr still gets the remainder), bits2-0=Dq
        // (quotient). Cycle costs not calibrated — same caveats as
        // MULU.L/MULS.L.
        if opcode & 0b1111_1111_1100_0000 == 0b0100_1100_0100_0000 {
            if self.cpu_type != CpuType::M68020 {
                let pc_push = self.pc.wrapping_sub(2);
                self.take_exception(bus, 4, pc_push);
                return Ok(34);
            }
            let src = self
                .resolve_ea(bus, mode, reg, Size::Long)
                .ok_or(StepError::IllegalAddressing)?;
            let ea_extra = self.ea_extra_cycles;
            let divisor = ae_read(src.read(self, bus, Size::Long))?;
            if divisor == 0 {
                // See the DIVU/DIVS doc (op_line_8): post-instruction trap.
                self.take_exception(bus, 5, self.pc);
                return Ok(ea_extra + 10);
            }
            let ext = self.fetch_word(bus);
            let dr = ((ext >> 12) & 0b111) as usize;
            let signed = ext & 0x0800 != 0;
            let dividend_64 = ext & 0x0400 != 0;
            let dq = (ext & 0b111) as usize;
            self.set_flag(ccr::C, false);
            if signed {
                let dividend: i64 = if dividend_64 {
                    ((self.d[dr] as i64) << 32) | (self.d[dq] as u32 as i64)
                } else {
                    self.d[dq] as i32 as i64
                };
                let divisor = divisor as i32 as i64;
                let quotient = dividend / divisor;
                let remainder = dividend % divisor;
                let overflow = quotient > i32::MAX as i64 || quotient < i32::MIN as i64;
                if !overflow {
                    self.d[dr] = remainder as u32;
                    self.d[dq] = quotient as u32;
                    self.set_flag(ccr::N, quotient < 0);
                    self.set_flag(ccr::Z, quotient == 0);
                }
                self.set_flag(ccr::V, overflow);
            } else {
                let dividend: u64 = if dividend_64 {
                    ((self.d[dr] as u64) << 32) | (self.d[dq] as u64)
                } else {
                    self.d[dq] as u64
                };
                let divisor = divisor as u64;
                let quotient = dividend / divisor;
                let remainder = dividend % divisor;
                let overflow = quotient > u32::MAX as u64;
                if !overflow {
                    self.d[dr] = remainder as u32;
                    self.d[dq] = quotient as u32;
                    self.set_flag(ccr::N, quotient & 0x8000_0000 != 0);
                    self.set_flag(ccr::Z, quotient == 0);
                }
                self.set_flag(ccr::V, overflow);
            }
            return Ok(if dividend_64 { 84 } else { 44 } + ea_extra);
        }
        // EXT: 0100 1000 1 sz 000 rrr — must precede MOVEM (same bit range)
        if opcode & 0b1111_1111_1011_1000 == 0b0100_1000_1000_0000 {
            let to_long = opcode & 0x0040 != 0;
            let r = reg as usize;
            if to_long {
                self.d[r] = self.d[r] as u16 as i16 as i32 as u32;
                self.set_logic_flags(self.d[r], Size::Long);
            } else {
                let word = (self.d[r] as u8 as i8 as i16 as u16) as u32;
                self.d[r] = (self.d[r] & 0xFFFF_0000) | word;
                self.set_logic_flags(word, Size::Word);
            }
            return Ok(4);
        }
        // MOVEM: 0100 1 d00 1sz mmm rrr (d=0: regs→mem, d=1: mem→regs)
        if opcode & 0b1111_1011_1000_0000 == 0b0100_1000_1000_0000 {
            return self.op_movem(bus, opcode);
        }
        // MOVE from SR: 0100 0000 11 mmm rrr
        if opcode & 0b1111_1111_1100_0000 == 0b0100_0000_1100_0000 {
            let sr = self.sr;
            let ea = self
                .resolve_ea(bus, mode, reg, Size::Word)
                .ok_or(StepError::IllegalAddressing)?;
            let ea_extra = self.ea_extra_cycles;
            let is_dn = mode == 0b000;
            // Dummy read before write (68000 RMW): AE reported as a read error.
            ae_read(ea.read(self, bus, Size::Word))?;
            ae_write(ea.write(self, bus, Size::Word, sr as u32))?;
            return Ok(if is_dn { 6 } else { 8 + ea_extra });
        }
        // MOVE to CCR: 0100 0100 11 mmm rrr
        if opcode & 0b1111_1111_1100_0000 == 0b0100_0100_1100_0000 {
            let ea = self
                .resolve_ea(bus, mode, reg, Size::Word)
                .ok_or(StepError::IllegalAddressing)?;
            let ea_extra = self.ea_extra_cycles;
            let val = ae_read(ea.read(self, bus, Size::Word))?;
            self.write_sr(((self.sr & 0xFF00) | (val as u16 & 0x001F)) & 0xA71F);
            return Ok(12 + ea_extra);
        }
        // MOVE to SR: 0100 0110 11 mmm rrr (privileged)
        if opcode & 0b1111_1111_1100_0000 == 0b0100_0110_1100_0000 {
            if let Some(c) = self.check_privilege(bus) {
                return Ok(c);
            }
            let ea = self
                .resolve_ea(bus, mode, reg, Size::Word)
                .ok_or(StepError::IllegalAddressing)?;
            let ea_extra = self.ea_extra_cycles;
            let val = ae_read(ea.read(self, bus, Size::Word))?;
            self.write_sr(val as u16 & 0xA71F);
            return Ok(12 + ea_extra);
        }
        // MOVE from CCR: 0100 0010 11 mmm rrr (68010+ only) — reuses the
        // SS=11 value, undefined for CLR on a real 68000 (CLR only knows
        // byte/word/long = 00/01/10). Must precede the CLR test below (its
        // wider mask, 0xFF00, would otherwise also match this range and
        // wrongly attempt an invalid `Size::from_bits`). On a real
        // MC68000 this opcode doesn't exist: "illegal instruction"
        // exception (vector 4), like MOVEC below (the same kind of
        // CPU-detection probe used by software, TOS included).
        if opcode & 0b1111_1111_1100_0000 == 0b0100_0010_1100_0000 {
            if self.cpu_type == CpuType::M68000 {
                let pc_push = self.pc.wrapping_sub(2);
                self.take_exception(bus, 4, pc_push);
                return Ok(34);
            }
            // Not privileged (unlike MOVE from SR): reads the CCR (SR's
            // low byte, 5 significant bits), high byte 0 — same RMW
            // pattern as MOVE from SR above.
            let ccr = (self.sr & 0x00FF) as u32;
            let ea = self
                .resolve_ea(bus, mode, reg, Size::Word)
                .ok_or(StepError::IllegalAddressing)?;
            let ea_extra = self.ea_extra_cycles;
            let is_dn = mode == 0b000;
            ae_read(ea.read(self, bus, Size::Word))?;
            ae_write(ea.write(self, bus, Size::Word, ccr))?;
            return Ok(if is_dn { 6 } else { 8 + ea_extra });
        }
        // CLR: 0100 0010 SS mmm rrr
        if opcode & 0b1111_1111_0000_0000 == 0b0100_0010_0000_0000 {
            let size = Size::from_bits(opcode >> 6).ok_or(StepError::IllegalAddressing)?;
            let dst = self
                .resolve_ea(bus, mode, reg, size)
                .ok_or(StepError::IllegalAddressing)?;
            let ea_extra = self.ea_extra_cycles;
            let is_dn = mode == 0b000;
            // The 68000 performs a dummy read before the write (RMW): an
            // odd address triggers a *read* error, not a write error.
            ae_read(dst.read(self, bus, size))?;
            ae_write(dst.write(self, bus, size, 0))?;
            self.set_flag(ccr::N, false);
            self.set_flag(ccr::Z, true);
            self.set_flag(ccr::V, false);
            self.set_flag(ccr::C, false);
            return Ok(rmw_cost(is_dn, size, ea_extra));
        }
        // TAS: 0100 1010 11 mmm rrr — must precede TST (same high byte)
        if opcode & 0b1111_1111_1100_0000 == 0b0100_1010_1100_0000 {
            let ea = self
                .resolve_ea(bus, mode, reg, Size::Byte)
                .ok_or(StepError::IllegalAddressing)?;
            let ea_extra = self.ea_extra_cycles;
            let is_dn = mode == 0b000;
            let val = ae_read(ea.read(self, bus, Size::Byte))?;
            self.set_flag(ccr::N, val & 0x80 != 0);
            self.set_flag(ccr::Z, val == 0);
            self.set_flag(ccr::V, false);
            self.set_flag(ccr::C, false);
            ae_write(ea.write(self, bus, Size::Byte, val | 0x80))?;
            return Ok(if is_dn { 4 } else { 10 + ea_extra });
        }
        // TST: 0100 1010 SS mmm rrr
        if opcode & 0b1111_1111_0000_0000 == 0b0100_1010_0000_0000 {
            let size = Size::from_bits(opcode >> 6).ok_or(StepError::IllegalAddressing)?;
            let ea = self
                .resolve_ea(bus, mode, reg, size)
                .ok_or(StepError::IllegalAddressing)?;
            let ea_extra = self.ea_extra_cycles;
            let val = ae_read(ea.read(self, bus, size))?;
            self.set_logic_flags(val, size);
            return Ok(4 + ea_extra);
        }
        // NEG: 0100 0100 SS mmm rrr (careful: bits 10-8 = 010)
        if opcode & 0b1111_1111_0000_0000 == 0b0100_0100_0000_0000 {
            let size = Size::from_bits(opcode >> 6).ok_or(StepError::IllegalAddressing)?;
            let ea = self
                .resolve_ea(bus, mode, reg, size)
                .ok_or(StepError::IllegalAddressing)?;
            let ea_extra = self.ea_extra_cycles;
            let is_dn = mode == 0b000;
            let val = ae_read(ea.read(self, bus, size))?;
            let result = self.sub_with_flags(0, val, size);
            // X = C for NEG
            let c = self.flag(ccr::C);
            self.set_flag(ccr::X, c);
            ae_write(ea.write(self, bus, size, result))?;
            return Ok(rmw_cost(is_dn, size, ea_extra));
        }
        // NEGX: 0100 0000 SS mmm rrr
        if opcode & 0b1111_1111_0000_0000 == 0b0100_0000_0000_0000 {
            let size = Size::from_bits(opcode >> 6).ok_or(StepError::IllegalAddressing)?;
            let ea = self
                .resolve_ea(bus, mode, reg, size)
                .ok_or(StepError::IllegalAddressing)?;
            let ea_extra = self.ea_extra_cycles;
            let is_dn = mode == 0b000;
            let val = ae_read(ea.read(self, bus, size))?;
            let x = if self.flag(ccr::X) { 1u32 } else { 0 };
            let result = self.subx_with_flags(0, val, x, size);
            ae_write(ea.write(self, bus, size, result))?;
            return Ok(rmw_cost(is_dn, size, ea_extra));
        }
        // NOT: 0100 0110 SS mmm rrr
        if opcode & 0b1111_1111_0000_0000 == 0b0100_0110_0000_0000 {
            let size = Size::from_bits(opcode >> 6).ok_or(StepError::IllegalAddressing)?;
            let ea = self
                .resolve_ea(bus, mode, reg, size)
                .ok_or(StepError::IllegalAddressing)?;
            let ea_extra = self.ea_extra_cycles;
            let is_dn = mode == 0b000;
            let val = ae_read(ea.read(self, bus, size))?;
            let result = !val & size.mask();
            ae_write(ea.write(self, bus, size, result))?;
            self.set_logic_flags(result, size);
            return Ok(rmw_cost(is_dn, size, ea_extra));
        }
        // NBCD: 0100 1000 00 mmm rrr
        if opcode & 0b1111_1111_1100_0000 == 0b0100_1000_0000_0000 {
            return self.op_nbcd(bus, mode, reg);
        }

        Err(StepError::Unimplemented(opcode))
    }

    fn op_jsr(&mut self, bus: &mut impl Bus, mode: u16, reg: u16) -> Result<u32, StepError> {
        let ea = self
            .resolve_ea(bus, mode, reg, Size::Long)
            .ok_or(StepError::IllegalAddressing)?;
        let addr = match ea {
            Operand::Memory(a) => a,
            _ => return Err(StepError::IllegalAddressing),
        };
        // Odd target: address error on the target fetch (FC = program),
        // BEFORE the push. SP is not decremented; the frame's PC is
        // self.pc. We go through pending_address_error so the frame
        // carries the program FC. Partial cost before the fault =
        // jsr_cost(mode,reg) minus the push that never executes (2 words
        // = 8 cycles) — calibrated exact against ProcessorTests:
        // (An)=16-8=8, (d16,An)=18-8=10, (d8,An,Xn)=22-8=14.
        if addr & 1 != 0 {
            self.pending_address_error = Some((addr, false, self.pc));
            return Ok(jsr_cost(mode, reg).saturating_sub(8));
        }
        let ret = self.pc;
        self.set_sp(self.sp().wrapping_sub(4));
        bus.write32(self.sp() & ADDR_MASK, ret);
        self.pc = addr;
        Ok(jsr_cost(mode, reg))
    }

    fn op_jmp(&mut self, bus: &mut impl Bus, mode: u16, reg: u16) -> Result<u32, StepError> {
        let ea = self
            .resolve_ea(bus, mode, reg, Size::Long)
            .ok_or(StepError::IllegalAddressing)?;
        let addr = match ea {
            Operand::Memory(a) => a,
            _ => return Err(StepError::IllegalAddressing),
        };
        self.pc = addr;
        Ok(jmp_cost(mode, reg))
    }

    fn op_rts(&mut self, bus: &mut impl Bus) -> Result<u32, StepError> {
        let addr = bus.read32(self.sp() & ADDR_MASK);
        self.set_sp(self.sp().wrapping_add(4));
        self.pc = addr;
        Ok(16)
    }

    /// RTD (68010+): `0100 1110 0111 0100` followed by a signed 16-bit
    /// displacement — pops PC like RTS, then adds the displacement to SP.
    fn op_rtd(&mut self, bus: &mut impl Bus) -> Result<u32, StepError> {
        let addr = bus.read32(self.sp() & ADDR_MASK);
        self.set_sp(self.sp().wrapping_add(4));
        let disp = self.fetch_word(bus) as i16 as i32;
        self.set_sp((self.sp() as i32).wrapping_add(disp) as u32);
        self.pc = addr;
        Ok(16)
    }

    fn op_rte(&mut self, bus: &mut impl Bus) -> Result<u32, StepError> {
        let new_sr = bus.read16(self.sp() & ADDR_MASK);
        self.set_sp(self.sp().wrapping_add(2));
        let new_pc = bus.read32(self.sp() & ADDR_MASK);
        self.set_sp(self.sp().wrapping_add(4));
        self.write_sr(new_sr & 0xA71F);
        self.pc = new_pc;
        Ok(20)
    }

    fn op_rtr(&mut self, bus: &mut impl Bus) -> Result<u32, StepError> {
        let new_ccr = bus.read16(self.sp() & ADDR_MASK);
        self.set_sp(self.sp().wrapping_add(2));
        let new_pc = bus.read32(self.sp() & ADDR_MASK);
        self.set_sp(self.sp().wrapping_add(4));
        self.sr = ((self.sr & 0xFF00) | (new_ccr & 0x001F)) & 0xA71F;
        self.pc = new_pc;
        Ok(20)
    }

    fn op_link(&mut self, bus: &mut impl Bus, reg: usize) -> Result<u32, StepError> {
        let disp = self.fetch_word(bus) as i16 as i32;
        let saved_an = self.a[reg]; // save An before any modification (LINK A7 case)
        self.set_sp(self.sp().wrapping_sub(4));
        bus.write32(self.sp() & ADDR_MASK, saved_an);
        self.a[reg] = self.sp();
        self.set_sp((self.sp() as i32).wrapping_add(disp) as u32);
        Ok(16)
    }

    fn op_unlk(&mut self, bus: &mut impl Bus, reg: usize) -> Result<u32, StepError> {
        let frame = self.a[reg];
        // UNLK performs an extra prefetch before the stack access: the
        // exception frame's PC is advanced by 2 (our_pc+4 instead of +2).
        //
        // No resolve_ea here (direct access via the register): otherwise
        // ea_extra_cycles would stay at the stale value from the previous
        // instruction. Address-error cost calibrated against
        // ProcessorTests: opcode(4) + extra prefetch(4) + 50 = 58 for
        // "UNLINK An", confirmed exact.
        if frame & 1 != 0 {
            self.ea_extra_cycles = 0;
            self.fault_prefix = 8;
            return Err(StepError::AddressError(
                frame,
                false,
                self.pc.wrapping_add(2),
            ));
        }
        let saved = ae_read(Operand::Memory(frame).read(self, bus, Size::Long))?;
        self.set_sp(frame.wrapping_add(4));
        self.a[reg] = saved;
        Ok(12)
    }

    fn op_pea(&mut self, bus: &mut impl Bus, mode: u16, reg: u16) -> Result<u32, StepError> {
        let ea = self
            .resolve_ea(bus, mode, reg, Size::Long)
            .ok_or(StepError::IllegalAddressing)?;
        let addr = match ea {
            Operand::Memory(a) => a,
            _ => return Err(StepError::IllegalAddressing),
        };
        self.set_sp(self.sp().wrapping_sub(4));
        bus.write32(self.sp() & ADDR_MASK, addr);
        Ok(pea_lea_cost(mode, reg, true))
    }

    fn op_lea(&mut self, bus: &mut impl Bus, opcode: u16) -> Result<u32, StepError> {
        let areg = ((opcode >> 9) & 0b111) as usize;
        let mode = (opcode >> 3) & 0b111;
        let reg = opcode & 0b111;
        let ea = self
            .resolve_ea(bus, mode, reg, Size::Long)
            .ok_or(StepError::IllegalAddressing)?;
        self.a[areg] = match ea {
            Operand::Memory(a) => a,
            _ => return Err(StepError::IllegalAddressing),
        };
        Ok(pea_lea_cost(mode, reg, false))
    }

    fn op_movem(&mut self, bus: &mut impl Bus, opcode: u16) -> Result<u32, StepError> {
        let to_regs = opcode & 0x0400 != 0; // bit 10: 0=regs→mem, 1=mem→regs
        let size = if opcode & 0x0040 != 0 {
            Size::Long
        } else {
            Size::Word
        };
        let mode = (opcode >> 3) & 0b111;
        let reg = opcode & 0b111;
        let mask = self.fetch_word(bus);

        if to_regs {
            // Memory → registers (post-increment mode possible)
            // Save A[reg]: resolve_ea applies a post-increment for (An)+;
            // on an address error, the instruction is aborted and the
            // register must stay unchanged.
            let saved_areg = self.a[reg as usize];
            let ea = self
                .resolve_ea(bus, mode, reg, size)
                .ok_or(StepError::IllegalAddressing)?;
            // MOVEM fetches an extra register-mask word before resolving the
            // EA; resolve_ea's size-based default (Long=0/Word=4) doesn't
            // know about it. +4 confirmed exact against ProcessorTests for
            // (An)/(An)+: Long=4+ea_extra+50, Word=8+ea_extra+50.
            self.fault_prefix += 4;
            let base = match ea {
                Operand::Memory(a) => a,
                _ => return Err(StepError::IllegalAddressing),
            };
            let frame_pc = self.pc.wrapping_add(2);
            // MOVEM accesses memory word by word. An odd address triggers
            // an address error reported at the base address (NOT base+2
            // as for a single long access). We therefore check the base's
            // alignment.
            if base & 1 != 0 {
                self.a[reg as usize] = saved_areg;
                return Err(StepError::AddressError(base, false, frame_pc));
            }
            let mut addr = base;
            let mut new_d = self.d;
            let mut new_a = self.a;
            for i in 0..16usize {
                if mask & (1 << i) != 0 {
                    let val = if size == Size::Long {
                        bus.read32(addr & ADDR_MASK)
                    } else {
                        bus.read16(addr & ADDR_MASK) as i16 as i32 as u32
                    };
                    // Real silicon: the CPU aborts on the FIRST faulting
                    // access, it does not continue with the remaining
                    // registers (otherwise `Cpu::step`'s generic
                    // post-instruction check would only see the LAST
                    // failure, not the first, and the instruction would
                    // behave as if it had fully succeeded). Do NOT consume
                    // the flag here (`has_pending_bus_fault`, not
                    // `take_bus_fault`): that generic check still needs to
                    // see it to trigger the exception.
                    if bus.has_pending_bus_fault() {
                        break;
                    }
                    if i < 8 {
                        new_d[i] = val;
                    } else {
                        new_a[i - 8] = val;
                    }
                    addr = addr.wrapping_add(size.bytes());
                }
            }
            self.d = new_d;
            self.a = new_a;
            // For (An)+, update the address register
            if mode == 0b011 {
                self.a[reg as usize] = addr;
            }
        } else {
            // Registers → memory
            let predec = mode == 0b100;
            let frame_pc = self.pc.wrapping_add(2);

            if predec {
                // For -(An), the decrement is handled manually without
                // going through resolve_ea (resolve_ea would perform a
                // spurious first decrement). Inverted mask: bit 0 = A7,
                // bit 7 = A0, bit 8 = D7, bit 15 = D0.
                let mut addr = self.a[reg as usize];
                // The AE occurs on the first address written. In long
                // mode, the 68000 writes the low word first (at ea+2), so
                // the faulting address is (base - 4) + 2. In word mode,
                // it's (base - 2). EA not updated.
                for i in 0..16usize {
                    if mask & (1 << i) != 0 {
                        addr = addr.wrapping_sub(size.bytes());
                        if addr & 1 != 0 {
                            let fault = if size == Size::Long {
                                addr.wrapping_add(2)
                            } else {
                                addr
                            };
                            // Manual decrement, no resolve_ea:
                            // ea_extra_cycles would otherwise stay at a
                            // stale value. Calibrated against
                            // ProcessorTests — 2 cycles below the generic
                            // "-(An)" table (10/6), because MOVEM does not
                            // pay the decrement idle twice when it's
                            // already included elsewhere.
                            self.ea_extra_cycles = if size == Size::Long { 8 } else { 4 };
                            self.fault_prefix = if size == Size::Long { 4 } else { 8 };
                            return Err(StepError::AddressError(fault, true, frame_pc));
                        }
                        break;
                    }
                }
                let mut addr = self.a[reg as usize];
                for i in 0..16usize {
                    if mask & (1 << i) != 0 {
                        let val = if i < 8 { self.a[7 - i] } else { self.d[15 - i] };
                        addr = addr.wrapping_sub(size.bytes());
                        if size == Size::Long {
                            bus.write32(addr & ADDR_MASK, val);
                        } else {
                            bus.write16(addr & ADDR_MASK, val as u16);
                        }
                        // See the equivalent comment on the mem→registers
                        // side: aborts on the first faulting access rather
                        // than continuing with the remaining registers.
                        if bus.has_pending_bus_fault() {
                            break;
                        }
                    }
                }
                self.a[reg as usize] = addr;
            } else {
                let ea = self
                    .resolve_ea(bus, mode, reg, size)
                    .ok_or(StepError::IllegalAddressing)?;
                // See the equivalent comment on the mem→registers side: +4
                // for the mask word already read before resolve_ea.
                self.fault_prefix += 4;
                let base = match ea {
                    Operand::Memory(a) => a,
                    _ => return Err(StepError::IllegalAddressing),
                };
                let frame_pc = self.pc.wrapping_add(2);
                if base & 1 != 0 {
                    return Err(StepError::AddressError(base, true, frame_pc));
                }
                let mut addr = base;
                for i in 0..16usize {
                    if mask & (1 << i) != 0 {
                        let val = if i < 8 { self.d[i] } else { self.a[i - 8] };
                        if size == Size::Long {
                            bus.write32(addr & ADDR_MASK, val);
                        } else {
                            bus.write16(addr & ADDR_MASK, val as u16);
                        }
                        // See the equivalent comment on the mem→registers
                        // side: aborts on the first faulting access rather
                        // than continuing with the remaining registers.
                        if bus.has_pending_bus_fault() {
                            break;
                        }
                        addr = addr.wrapping_add(size.bytes());
                    }
                }
            }
        }
        let m = mask.count_ones();
        let per_reg = if size == Size::Long { 8 } else { 4 };
        Ok(movem_base(mode, reg, to_regs) + per_reg * m)
    }

    // =========================================================================
    // Line 0101: ADDQ, SUBQ, Scc, DBcc
    // =========================================================================

    fn op_line_5(&mut self, bus: &mut impl Bus, opcode: u16) -> Result<u32, StepError> {
        let mode = (opcode >> 3) & 0b111;
        let reg = opcode & 0b111;

        // DBcc: 0101 cccc 1100 1rrr
        // Yacht.txt lines 952-966: 10 if the loop continues (branch taken),
        // 12 if the condition becomes true (exit), 14 if the counter expires (exit).
        if opcode & 0b1111_0000_1111_1000 == 0b0101_0000_1100_1000 {
            let cc = (opcode >> 8) & 0b1111;
            let r = reg as usize;
            if !self.test_condition(cc) {
                let base = self.pc; // PC before the displacement word
                let disp = self.fetch_word(bus) as i16 as i32;
                // PC of the next instruction (after the displacement) = frame_pc on an AE.
                let next_pc = self.pc;
                let word = (self.d[r] & 0xFFFF) as u16;
                let decremented = word.wrapping_sub(1);
                if decremented != 0xFFFF {
                    let target = (base as i32).wrapping_add(disp) as u32;
                    if target & 1 != 0 {
                        // Odd target: address error on the fetch (program
                        // FC). The instruction is aborted: the counter
                        // decrement is NOT committed.
                        self.pc = target;
                        self.pending_address_error = Some((target, false, next_pc));
                    } else {
                        self.d[r] = (self.d[r] & 0xFFFF_0000) | (decremented as u32);
                        self.pc = target;
                    }
                    return Ok(10); // loop continues
                } else {
                    self.d[r] = (self.d[r] & 0xFFFF_0000) | (decremented as u32);
                    return Ok(14); // counter expired
                }
            } else {
                self.fetch_word(bus); // consume the displacement
                return Ok(12); // condition became true
            }
        }

        // Scc: 0101 cccc 11 mmm rrr
        if opcode & 0b0000_0000_1100_0000 == 0b0000_0000_1100_0000 {
            let cc = (opcode >> 8) & 0b1111;
            let taken = self.test_condition(cc);
            let is_dn = mode == 0b000;
            let ea = self
                .resolve_ea(bus, mode, reg, Size::Byte)
                .ok_or(StepError::IllegalAddressing)?;
            let ea_extra = self.ea_extra_cycles;
            ae_write(ea.write(self, bus, Size::Byte, if taken { 0xFF } else { 0x00 }))?;
            return Ok(if is_dn {
                if taken { 6 } else { 4 }
            } else {
                8 + ea_extra
            });
        }

        // ADDQ / SUBQ
        let is_sub = opcode & 0x0100 != 0;
        let imm_bits = (opcode >> 9) & 0b111;
        let imm = if imm_bits == 0 { 8u32 } else { imm_bits as u32 };

        // For an address register, no flags, size is always Long.
        // Yacht.txt lines 943-950: real cost is 8, regardless of the
        // instruction's size (the M68000UM lists 4 for B/W, but Yacht
        // documents the confirmed erratum on real silicon).
        if mode == 0b001 {
            let r = reg as usize;
            if is_sub {
                self.a[r] = self.a[r].wrapping_sub(imm);
            } else {
                self.a[r] = self.a[r].wrapping_add(imm);
            }
            return Ok(8);
        }

        let size = Size::from_bits(opcode >> 6).ok_or(StepError::IllegalAddressing)?;
        let ea = self
            .resolve_ea(bus, mode, reg, size)
            .ok_or(StepError::IllegalAddressing)?;
        let ea_extra = self.ea_extra_cycles;
        let is_dn = mode == 0b000;
        let val = ae_read(ea.read(self, bus, size))?;
        let result = if is_sub {
            self.sub_with_flags(val, imm, size)
        } else {
            self.add_with_flags(val, imm, size)
        };
        ae_write(ea.write(self, bus, size, result))?;
        let long = size == Size::Long;
        Ok(if is_dn {
            if long { 8 } else { 4 }
        } else if long {
            12 + ea_extra
        } else {
            8 + ea_extra
        })
    }

    // =========================================================================
    // Line 0110: BRA / BSR / Bcc
    // =========================================================================

    fn op_branch(&mut self, bus: &mut impl Bus, opcode: u16) -> Result<u32, StepError> {
        let condition = (opcode >> 8) & 0b1111;
        let byte_disp = opcode as i8;
        let base = self.pc;
        let disp = if byte_disp == 0 {
            self.fetch_word(bus) as i16 as i32
        } else {
            byte_disp as i32
        };
        let target = (base as i32).wrapping_add(disp) as u32;

        match condition {
            0b0000 => {
                // BRA to an odd address: frame_pc = pc after opcode fetch (= opcode_addr+2)
                let next_pc = base;
                self.pc = target;
                if target & 1 != 0 {
                    self.pending_address_error = Some((target, false, next_pc));
                }
                Ok(10)
            }
            0b0001 => {
                let ret = self.pc;
                let new_sp = self.sp().wrapping_sub(4);
                self.set_sp(new_sp);
                if new_sp & 1 != 0 {
                    return Err(StepError::AddressError(new_sp, true, self.pc));
                }
                bus.write32(self.sp() & ADDR_MASK, ret);
                // BSR to an odd address: frame_pc = target (the target address)
                self.pc = target;
                if target & 1 != 0 {
                    self.pending_address_error = Some((target, false, target));
                }
                Ok(18) // Yacht.txt lines 1005-1032: the stack push wasn't counted
            }
            _ => {
                // Bcc: Yacht.txt lines 1005-1032. .B form (nonzero
                // displacement): taken=10, not taken=8. .W form (extension
                // word): taken=10, not taken=12.
                let taken = self.test_condition(condition);
                if taken {
                    self.pc = target;
                }
                Ok(if taken {
                    10
                } else if byte_disp != 0 {
                    8
                } else {
                    12
                })
            }
        }
    }

    // =========================================================================
    // MOVEQ
    // =========================================================================

    fn op_moveq(&mut self, opcode: u16) -> Result<u32, StepError> {
        if opcode & 0x0100 != 0 {
            return Err(StepError::Unimplemented(opcode));
        }
        let reg = ((opcode >> 9) & 0b111) as usize;
        let value = opcode as i8 as i32 as u32;
        self.d[reg] = value;
        self.set_logic_flags(value, Size::Long);
        Ok(4)
    }

    // =========================================================================
    // Line 1000: OR, DIVU, DIVS, SBCD
    // =========================================================================

    fn op_line_8(&mut self, bus: &mut impl Bus, opcode: u16) -> Result<u32, StepError> {
        let reg = ((opcode >> 9) & 0b111) as usize;
        let mode = (opcode >> 3) & 0b111;
        let ea_reg = opcode & 0b111;
        let size_bits = (opcode >> 6) & 0b111;

        // DIVU: opmode 011
        if size_bits == 0b011 {
            let src = self
                .resolve_ea(bus, mode, ea_reg, Size::Word)
                .ok_or(StepError::IllegalAddressing)?;
            let ea_extra = self.ea_extra_cycles;
            let divisor = ae_read(src.read(self, bus, Size::Word))?;
            if divisor == 0 {
                // Divide-by-zero (vector 5) is a post-instruction trap, not a
                // Group 0 fault: the frame's saved PC is the address of the
                // NEXT instruction (self.pc, already advanced past the
                // opcode and any EA extension words), not the divide
                // instruction itself — RTE must not re-execute the DIVU.
                self.take_exception(bus, 5, self.pc);
                return Ok(ea_extra + 10);
            }
            let dividend = self.d[reg];
            let quotient = dividend / divisor;
            let remainder = dividend % divisor;
            let div_cycles = divu_core_cycles(dividend, divisor as u16);
            if quotient > 0xFFFF {
                // DIVU overflow (68000 silicon): N=1, Z=0, V=1, C=0, X preserved.
                self.set_flag(ccr::N, true);
                self.set_flag(ccr::Z, false);
                self.set_flag(ccr::V, true);
                self.set_flag(ccr::C, false);
            } else {
                self.d[reg] = (remainder << 16) | (quotient & 0xFFFF);
                self.set_flag(ccr::N, quotient & 0x8000 != 0);
                self.set_flag(ccr::Z, quotient == 0);
                self.set_flag(ccr::V, false);
                self.set_flag(ccr::C, false);
            }
            // DIVU does not modify X
            return Ok(div_cycles + 4 + ea_extra);
        }
        // DIVS : opmode 111
        if size_bits == 0b111 {
            let src = self
                .resolve_ea(bus, mode, ea_reg, Size::Word)
                .ok_or(StepError::IllegalAddressing)?;
            let ea_extra = self.ea_extra_cycles;
            let divisor = ae_read(src.read(self, bus, Size::Word))? as u16 as i16 as i32;
            if divisor == 0 {
                // See the DIVU branch above: post-instruction trap, frame PC
                // must be the next instruction, not this DIVS.
                self.take_exception(bus, 5, self.pc);
                return Ok(ea_extra + 10);
            }
            let dividend = self.d[reg] as i32;
            let quotient = dividend / divisor;
            let remainder = dividend % divisor;
            let div_cycles = divs_core_cycles(dividend, divisor as i16);
            if quotient > 0x7FFF || quotient < -0x8000 {
                // DIVS overflow (68000 silicon): N=1, Z=0, V=1, C=0, X preserved.
                self.set_flag(ccr::N, true);
                self.set_flag(ccr::Z, false);
                self.set_flag(ccr::V, true);
                self.set_flag(ccr::C, false);
            } else {
                let q = quotient as u16 as u32;
                let r = (remainder as u16 as u32) << 16;
                self.d[reg] = r | q;
                self.set_flag(ccr::N, quotient < 0);
                self.set_flag(ccr::Z, quotient == 0);
                self.set_flag(ccr::V, false);
                self.set_flag(ccr::C, false);
            }
            // DIVS does not modify X
            return Ok(div_cycles + 4 + ea_extra);
        }
        // SBCD : 1000 rrr 10000 mrrr
        if opcode & 0b1111_0001_1111_0000 == 0b1000_0001_0000_0000 {
            return self.op_sbcd(bus, opcode);
        }

        // OR : 1000 rrr d SS mmm rrr
        let size = Size::from_bits(size_bits).ok_or(StepError::IllegalAddressing)?;
        let to_ea = opcode & 0x0100 != 0;
        let ea = self
            .resolve_ea(bus, mode, ea_reg, size)
            .ok_or(StepError::IllegalAddressing)?;
        let ea_extra = self.ea_extra_cycles;
        let ea_is_reg = mode <= 0b001 || (mode == 0b111 && ea_reg == 0b100);
        if to_ea {
            let a = self.d[reg] & size.mask();
            let b = ae_read(ea.read(self, bus, size))?;
            let r = a | b;
            ae_write(ea.write(self, bus, size, r))?;
            self.set_logic_flags(r, size);
        } else {
            let a = ae_read(ea.read(self, bus, size))?;
            let b = self.d[reg] & size.mask();
            let r = a | b;
            ae_write(Operand::DataReg(reg).write(self, bus, size, r))?;
            self.set_logic_flags(r, size);
        }
        Ok(logic_op_cost(to_ea, ea_is_reg, size, ea_extra))
    }

    // =========================================================================
    // Ligne 1001 : SUB / SUBA / SUBX
    // =========================================================================

    fn op_sub(&mut self, bus: &mut impl Bus, opcode: u16) -> Result<u32, StepError> {
        let reg = ((opcode >> 9) & 0b111) as usize;
        let mode = (opcode >> 3) & 0b111;
        let ea_reg = opcode & 0b111;
        let size_bits = (opcode >> 6) & 0b111;

        // SUBX : bit 8 = 1, bits5:4 = 00, size_bits ∉ {3,7} (distingue de SUBA)
        if opcode & 0x0100 != 0 && (opcode >> 4) & 0b11 == 0b00 && size_bits != 3 && size_bits != 7
        {
            let src_reg = ea_reg as usize;
            let size = Size::from_bits(size_bits).ok_or(StepError::IllegalAddressing)?;
            let mem_mode = opcode & 0x0008 != 0;
            let x = if self.flag(ccr::X) { 1u32 } else { 0 };

            let (src_val, dst_val, dst_op) = if mem_mode {
                // SUBX -(An),-(An) performs an extra prefetch: frame_pc = pc+2.
                self.ea_frame_pc = self.pc.wrapping_add(2);
                let saved_src = self.a[src_reg];
                let saved_dst = self.a[reg];
                // Hardware order: decrement src → read src → decrement dst → read dst.
                let step = if src_reg == 7 && size == Size::Byte {
                    2
                } else {
                    size.bytes()
                };
                self.a[src_reg] = self.a[src_reg].wrapping_sub(step);
                let src_addr = self.a[src_reg];
                let s = match self.read_mem_checked(bus, src_addr, size) {
                    Ok(v) => v,
                    Err((addr, pc)) => {
                        if size == Size::Long {
                            self.a[src_reg] = saved_src;
                        }
                        // See ADDX (identical structure): fault on src,
                        // generic ea_extra="-(An)"; zero prefix in Long, +4
                        // in Word (same size adjustment as everywhere else).
                        self.ea_extra_cycles = if size == Size::Long { 10 } else { 6 };
                        self.fault_prefix = if size == Size::Long { 0 } else { 4 };
                        return Err(StepError::AddressError(addr, false, pc));
                    }
                };
                let dstep = if reg == 7 && size == Size::Byte {
                    2
                } else {
                    size.bytes()
                };
                self.a[reg] = self.a[reg].wrapping_sub(dstep);
                let dst_addr = self.a[reg];
                let d = match self.read_mem_checked(bus, dst_addr, size) {
                    Ok(v) => v,
                    Err((addr, pc)) => {
                        if size == Size::Long {
                            // AE on dst (long): only the dst decrement is undone.
                            // The src decrement stays committed (even if src=A7/SSP).
                            self.a[reg] = saved_dst;
                        }
                        // Fault on dst: src already read successfully, its reduced
                        // cost serves as a prefix (cf. ADDX). Exhaustively verified against
                        // TomHarte's 2500 SUBX.w cases: prefix 8 in every
                        // case (Word or Long, src=A7 or not) — "-(An),-(An)" fault on dst
                        // = 64 systematically, not just when src=A7.
                        self.ea_extra_cycles = if size == Size::Long { 10 } else { 6 };
                        self.fault_prefix = 8;
                        return Err(StepError::AddressError(addr, false, pc));
                    }
                };
                (s, d, Operand::Memory(dst_addr & ADDR_MASK))
            } else {
                (
                    self.d[src_reg] & size.mask(),
                    self.d[reg] & size.mask(),
                    Operand::DataReg(reg),
                )
            };
            let result = self.subx_with_flags(dst_val, src_val, x, size);
            ae_write(dst_op.write(self, bus, size, result))?;
            let long = size == Size::Long;
            return Ok(if mem_mode {
                if long { 30 } else { 18 }
            } else if long {
                8
            } else {
                4
            });
        }

        // SUBA : opmode 011 ou 111
        if size_bits == 0b011 || size_bits == 0b111 {
            let size = if size_bits == 0b011 {
                Size::Word
            } else {
                Size::Long
            };
            let src = self
                .resolve_ea(bus, mode, ea_reg, size)
                .ok_or(StepError::IllegalAddressing)?;
            let ea_extra = self.ea_extra_cycles;
            // #imm follows the same base as Dn/An (8+ea_extra, ea_extra=0 for those
            // so unchanged) rather than the real memory modes' "6+ea_extra"
            // — calibrated and verified exact against ProcessorTests
            // (SUBA.l/ADDA.l "#,An" expected 16, not 14).
            let src_is_reg_or_imm = mode <= 0b001 || (mode == 0b111 && ea_reg == 0b100);
            let value = size.sign_extend(ae_read(src.read(self, bus, size))?);
            self.a[reg] = self.a[reg].wrapping_sub(value);
            return Ok(if size == Size::Long {
                if src_is_reg_or_imm {
                    8 + ea_extra
                } else {
                    6 + ea_extra
                }
            } else {
                8 + ea_extra
            });
        }

        let size = Size::from_bits(size_bits).ok_or(StepError::IllegalAddressing)?;
        let to_ea = opcode & 0x0100 != 0;
        let ea = self
            .resolve_ea(bus, mode, ea_reg, size)
            .ok_or(StepError::IllegalAddressing)?;
        let ea_extra = self.ea_extra_cycles;
        let ea_is_reg = mode <= 0b001 || (mode == 0b111 && ea_reg == 0b100);
        if to_ea {
            let a = ae_read(ea.read(self, bus, size))?;
            let b = self.d[reg] & size.mask();
            let result = self.sub_with_flags(a, b, size);
            ae_write(ea.write(self, bus, size, result))?;
        } else {
            let a = self.d[reg] & size.mask();
            let b = ae_read(ea.read(self, bus, size))?;
            let result = self.sub_with_flags(a, b, size);
            ae_write(Operand::DataReg(reg).write(self, bus, size, result))?;
        }
        Ok(logic_op_cost(to_ea, ea_is_reg, size, ea_extra))
    }

    // =========================================================================
    // Ligne 1011 : CMP / CMPA / CMPM / EOR
    // =========================================================================

    fn op_line_b(&mut self, bus: &mut impl Bus, opcode: u16) -> Result<u32, StepError> {
        let reg = ((opcode >> 9) & 0b111) as usize;
        let mode = (opcode >> 3) & 0b111;
        let ea_reg = opcode & 0b111;
        let size_bits = (opcode >> 6) & 0b111;

        // CMPA : opmode 011 ou 111
        if size_bits == 0b011 || size_bits == 0b111 {
            let size = if size_bits == 0b011 {
                Size::Word
            } else {
                Size::Long
            };
            let src = self
                .resolve_ea(bus, mode, ea_reg, size)
                .ok_or(StepError::IllegalAddressing)?;
            let ea_extra = self.ea_extra_cycles;
            let value = size.sign_extend(ae_read(src.read(self, bus, size))?);
            self.cmp_flags(self.a[reg], value, Size::Long);
            return Ok(6 + ea_extra);
        }

        // EOR : bit 8 = 1, destination ≠ An
        if opcode & 0x0100 != 0 {
            // CMPM : 1011 rrr 1 SS 001 rrr
            if mode == 0b001 {
                let size = Size::from_bits(size_bits).ok_or(StepError::IllegalAddressing)?;
                let src_r = ea_reg as usize;
                let dst_r = reg;
                let src_step = if src_r == 7 && size == Size::Byte {
                    2
                } else {
                    size.bytes()
                };
                let dst_step = if dst_r == 7 && size == Size::Byte {
                    2
                } else {
                    size.bytes()
                };
                // frame_pc for CMPM: extra prefetch, pc+2 after opcode fetch
                self.ea_frame_pc = self.pc.wrapping_add(2);
                self.ea_is_pc_relative = false;
                let src_addr = self.a[src_r];

                // AE on src for odd word/long access: partial increment +2.
                // For .b, an odd address is legal → always commit_src.
                let commit_src = if size != Size::Byte && src_addr & 1 != 0 {
                    2
                } else {
                    src_step
                };
                self.a[src_r] = self.a[src_r].wrapping_add(commit_src);
                // No resolve_ea ((An)+ handled manually): ea_extra_cycles/
                // fault_prefix would otherwise be stale. Calibrated against
                // ProcessorTests, same scheme as ADDX/SUBX: fault on src
                // (first read) = generic ea_extra "(An)+", zero prefix
                // in Long / +4 in Word.
                self.ea_extra_cycles = if size == Size::Long { 8 } else { 4 };
                self.fault_prefix = if size == Size::Long { 0 } else { 4 };
                let s = ae_read(Operand::Memory(src_addr).read(self, bus, size))?;

                let dst_addr = self.a[dst_r];
                // AE on dst only for odd word/long access.
                // For .b, an odd address is legal → always commit_dst.
                let dst_ae = size != Size::Byte && dst_addr & 1 != 0;
                let commit_dst = if dst_ae { 0 } else { dst_step };
                self.a[dst_r] = self.a[dst_r].wrapping_add(commit_dst);
                // Fault on dst (second read): src already read, its reduced cost
                // serves as prefix = 8, identical in Word and Long here (unlike
                // ADDX/SUBX which need +4 in Word) — calibrated and verified
                // exact against ProcessorTests.
                self.ea_extra_cycles = if size == Size::Long { 8 } else { 4 };
                self.fault_prefix = 8;
                let d = ae_read(Operand::Memory(dst_addr).read(self, bus, size))?;

                self.cmp_flags(d, s, size);
                return Ok(if size == Size::Long { 20 } else { 12 });
            }
            // EOR Dn,<ea>
            let size = Size::from_bits(size_bits).ok_or(StepError::IllegalAddressing)?;
            let ea = self
                .resolve_ea(bus, mode, ea_reg, size)
                .ok_or(StepError::IllegalAddressing)?;
            let ea_extra = self.ea_extra_cycles;
            let ea_is_reg = mode <= 0b001 || (mode == 0b111 && ea_reg == 0b100);
            let a = self.d[reg] & size.mask();
            let b = ae_read(ea.read(self, bus, size))?;
            let r = a ^ b;
            ae_write(ea.write(self, bus, size, r))?;
            self.set_logic_flags(r, size);
            let long = size == Size::Long;
            return Ok(if ea_is_reg {
                if long { 8 } else { 4 }
            } else if long {
                12 + ea_extra
            } else {
                8 + ea_extra
            });
        }

        // CMP
        let size = Size::from_bits(size_bits).ok_or(StepError::IllegalAddressing)?;
        let ea = self
            .resolve_ea(bus, mode, ea_reg, size)
            .ok_or(StepError::IllegalAddressing)?;
        let ea_extra = self.ea_extra_cycles;
        let src = ae_read(ea.read(self, bus, size))?;
        let dst = self.d[reg] & size.mask();
        self.cmp_flags(dst, src, size);
        let base = if size == Size::Long { 6 } else { 4 };
        Ok(base + ea_extra)
    }

    // =========================================================================
    // Ligne 1100 : AND / MULU / MULS / EXG / ABCD
    // =========================================================================

    fn op_line_c(&mut self, bus: &mut impl Bus, opcode: u16) -> Result<u32, StepError> {
        let reg = ((opcode >> 9) & 0b111) as usize;
        let mode = (opcode >> 3) & 0b111;
        let ea_reg = opcode & 0b111;
        let size_bits = (opcode >> 6) & 0b111;

        // MULU: opmode 011
        // Yacht.txt 1211-1234: exact cost = 38 + 2×(bits set to 1 in the source word) + ea_extra.
        if size_bits == 0b011 {
            let src = self
                .resolve_ea(bus, mode, ea_reg, Size::Word)
                .ok_or(StepError::IllegalAddressing)?;
            let ea_extra = self.ea_extra_cycles;
            let val = ae_read(src.read(self, bus, Size::Word))?;
            let result = (self.d[reg] & 0xFFFF) * (val & 0xFFFF);
            self.d[reg] = result;
            self.set_flag(ccr::N, result & 0x8000_0000 != 0);
            self.set_flag(ccr::Z, result == 0);
            self.set_flag(ccr::V, false);
            self.set_flag(ccr::C, false);
            let m = (val as u16).count_ones();
            return Ok(38 + 2 * m + ea_extra);
        }
        // MULS: opmode 111
        // Yacht.txt 1211-1234: m = number of adjacent-bit transitions (01/10)
        // within the 16 bits of the source — same cost as MULU otherwise.
        if size_bits == 0b111 {
            let src = self
                .resolve_ea(bus, mode, ea_reg, Size::Word)
                .ok_or(StepError::IllegalAddressing)?;
            let ea_extra = self.ea_extra_cycles;
            let src_word = ae_read(src.read(self, bus, Size::Word))?;
            let val = src_word as u16 as i16 as i32;
            let result = ((self.d[reg] & 0xFFFF) as u16 as i16 as i32) * val;
            self.d[reg] = result as u32;
            self.set_flag(ccr::N, result < 0);
            self.set_flag(ccr::Z, result == 0);
            self.set_flag(ccr::V, false);
            self.set_flag(ccr::C, false);
            // Booth's algorithm: bit[-1] (just below the LSB) is 0. m = number of
            // 01/10 transitions between bit[i] and bit[i-1] for i=0..15 (bit0 is
            // compared against this implicit bit[-1]=0, hence the count via src^(src<<1),
            // masked to 16 bits to ignore the shift's overflow).
            // Calibrated and verified exact against ProcessorTests
            // (src=$CBE0 → m=5, src=$9633 → m=9).
            let src16 = src_word as u16 as u32;
            let m = ((src16 ^ (src16 << 1)) & 0xFFFF).count_ones();
            return Ok(38 + 2 * m + ea_extra);
        }

        // EXG / ABCD : bit 8 = 1
        if opcode & 0x0100 != 0 {
            let op = (opcode >> 3) & 0b11111;
            match op {
                0b01000 => {
                    // EXG Dx,Dy
                    self.d.swap(reg, ea_reg as usize);
                    return Ok(6);
                }
                0b01001 => {
                    // EXG Ax,Ay
                    self.a.swap(reg, ea_reg as usize);
                    return Ok(6);
                }
                0b10001 => {
                    // EXG Dx,Ay
                    let tmp = self.d[reg];
                    self.d[reg] = self.a[ea_reg as usize];
                    self.a[ea_reg as usize] = tmp;
                    return Ok(6);
                }
                _ => {}
            }
            // ABCD : 1100 rrr 10000 mrrr (op bits 7-3 = 0000m)
            if op <= 0b00001 {
                return self.op_abcd(bus, opcode);
            }
        }

        // AND : 1100 rrr d SS mmm rrr
        let size = Size::from_bits(size_bits).ok_or(StepError::IllegalAddressing)?;
        let to_ea = opcode & 0x0100 != 0;
        let ea = self
            .resolve_ea(bus, mode, ea_reg, size)
            .ok_or(StepError::IllegalAddressing)?;
        let ea_extra = self.ea_extra_cycles;
        let ea_is_reg = mode <= 0b001 || (mode == 0b111 && ea_reg == 0b100);
        if to_ea {
            let a = self.d[reg] & size.mask();
            let b = ae_read(ea.read(self, bus, size))?;
            let r = a & b;
            ae_write(ea.write(self, bus, size, r))?;
            self.set_logic_flags(r, size);
        } else {
            let a = ae_read(ea.read(self, bus, size))?;
            let b = self.d[reg] & size.mask();
            let r = a & b;
            ae_write(Operand::DataReg(reg).write(self, bus, size, r))?;
            self.set_logic_flags(r, size);
        }
        Ok(logic_op_cost(to_ea, ea_is_reg, size, ea_extra))
    }

    // =========================================================================
    // Ligne 1101 : ADD / ADDA / ADDX
    // =========================================================================

    fn op_add(&mut self, bus: &mut impl Bus, opcode: u16) -> Result<u32, StepError> {
        let reg = ((opcode >> 9) & 0b111) as usize;
        let mode = (opcode >> 3) & 0b111;
        let ea_reg = opcode & 0b111;
        let size_bits = (opcode >> 6) & 0b111;

        if opcode & 0x0100 != 0 && (opcode >> 4) & 0b11 == 0b00 && size_bits != 3 && size_bits != 7
        {
            return self.op_addx(bus, opcode);
        }
        if size_bits == 0b011 || size_bits == 0b111 {
            let size = if size_bits == 0b011 {
                Size::Word
            } else {
                Size::Long
            };
            let src = self
                .resolve_ea(bus, mode, ea_reg, size)
                .ok_or(StepError::IllegalAddressing)?;
            let ea_extra = self.ea_extra_cycles;
            // Voir SUBA (formule identique) : #imm suit la base Dn/An.
            let src_is_reg_or_imm = mode <= 0b001 || (mode == 0b111 && ea_reg == 0b100);
            let value = size.sign_extend(ae_read(src.read(self, bus, size))?);
            self.a[reg] = self.a[reg].wrapping_add(value);
            return Ok(if size == Size::Long {
                if src_is_reg_or_imm {
                    8 + ea_extra
                } else {
                    6 + ea_extra
                }
            } else {
                8 + ea_extra
            });
        }
        let size = Size::from_bits(size_bits).ok_or(StepError::IllegalAddressing)?;
        let to_ea = opcode & 0x0100 != 0;
        let ea = self
            .resolve_ea(bus, mode, ea_reg, size)
            .ok_or(StepError::IllegalAddressing)?;
        let ea_extra = self.ea_extra_cycles;
        let ea_is_reg = mode <= 0b001 || (mode == 0b111 && ea_reg == 0b100);
        if to_ea {
            let a = self.d[reg] & size.mask();
            let b = ae_read(ea.read(self, bus, size))?;
            let result = self.add_with_flags(a, b, size);
            ae_write(ea.write(self, bus, size, result))?;
        } else {
            let a = ae_read(ea.read(self, bus, size))?;
            let b = self.d[reg] & size.mask();
            let result = self.add_with_flags(a, b, size);
            ae_write(Operand::DataReg(reg).write(self, bus, size, result))?;
        }
        Ok(logic_op_cost(to_ea, ea_is_reg, size, ea_extra))
    }

    fn op_addx(&mut self, bus: &mut impl Bus, opcode: u16) -> Result<u32, StepError> {
        let dst_reg = ((opcode >> 9) & 0b111) as usize;
        let src_reg = (opcode & 0b111) as usize;
        let size = Size::from_bits(opcode >> 6).ok_or(StepError::IllegalAddressing)?;
        let mem_mode = opcode & 0x0008 != 0;
        let x = if self.flag(ccr::X) { 1u32 } else { 0 };

        let (src_val, dst_val, dst_op) = if mem_mode {
            // ADDX -(An),-(An) performs an extra prefetch: frame_pc = pc+2.
            self.ea_frame_pc = self.pc.wrapping_add(2);
            let saved_src = self.a[src_reg];
            let saved_dst = self.a[dst_reg];
            // Hardware order: decrement src → read src → decrement dst → read dst.
            let step = if src_reg == 7 && size == Size::Byte {
                2
            } else {
                size.bytes()
            };
            self.a[src_reg] = self.a[src_reg].wrapping_sub(step);
            let src_addr = self.a[src_reg];
            let s = match self.read_mem_checked(bus, src_addr, size) {
                Ok(v) => v,
                Err((addr, pc)) => {
                    // AE on the src read: on a long access, both decrements
                    // are undone; on a word, the src decrement is kept.
                    if size == Size::Long {
                        self.a[src_reg] = saved_src;
                    }
                    // No resolve_ea (manual decrements): costs calibrated
                    // against ProcessorTests. Fault on the src read (first
                    // of the two): generic ea_extra="-(An)" (10/6); zero prefix
                    // in Long, +4 in Word.
                    self.ea_extra_cycles = if size == Size::Long { 10 } else { 6 };
                    self.fault_prefix = if size == Size::Long { 0 } else { 4 };
                    return Err(StepError::AddressError(addr, false, pc));
                }
            };
            let dstep = if dst_reg == 7 && size == Size::Byte {
                2
            } else {
                size.bytes()
            };
            self.a[dst_reg] = self.a[dst_reg].wrapping_sub(dstep);
            let dst_addr = self.a[dst_reg];
            let d = match self.read_mem_checked(bus, dst_addr, size) {
                Ok(v) => v,
                Err((addr, pc)) => {
                    if size == Size::Long {
                        // AE on dst (long): only the dst decrement is undone.
                        // The src decrement stays committed (even if src=A7/SSP).
                        self.a[dst_reg] = saved_dst;
                    }
                    // Fault on the dst read (second): the src read already
                    // succeeded, its "-(An)" cost counts as a prefix (reduced by 2,
                    // as for MOVEM — the decrement idle isn't repaid).
                    // Exhaustively verified against TomHarte's 2500 ADDX.w
                    // cases: prefix 8 in every case (Word or Long, src=A7
                    // or not); ea_extra_cycles carries the "-(An)" cost of the dst which,
                    // itself, faults.
                    self.ea_extra_cycles = if size == Size::Long { 10 } else { 6 };
                    self.fault_prefix = 8;
                    return Err(StepError::AddressError(addr, false, pc));
                }
            };
            (s, d, Operand::Memory(dst_addr & ADDR_MASK))
        } else {
            (
                self.d[src_reg] & size.mask(),
                self.d[dst_reg] & size.mask(),
                Operand::DataReg(dst_reg),
            )
        };
        let result = self.addx_with_flags(src_val, dst_val, x, size);
        ae_write(dst_op.write(self, bus, size, result))?;
        let long = size == Size::Long;
        Ok(if mem_mode {
            if long { 30 } else { 18 }
        } else if long {
            8
        } else {
            4
        })
    }

    // =========================================================================
    // Line 1110: shifts and rotations
    // =========================================================================

    fn op_line_e(&mut self, bus: &mut impl Bus, opcode: u16) -> Result<u32, StepError> {
        // Memory shift (1 bit only): 1110 tt d 11 EA
        // bit 8 = direction (1=left, 0=right); bits 10:9 = type (AS/LS/ROX/RO)
        if opcode & 0b1111_0000_1100_0000 == 0b1110_0000_1100_0000 {
            let dir = (opcode >> 8) & 1; // 0=right, 1=left
            let shift_type = (opcode >> 9) & 0b11;
            let mode = (opcode >> 3) & 0b111;
            let reg = opcode & 0b111;
            let ea = self
                .resolve_ea(bus, mode, reg, Size::Word)
                .ok_or(StepError::IllegalAddressing)?;
            let ea_extra = self.ea_extra_cycles;
            let val = ae_read(ea.read(self, bus, Size::Word))?;
            let result = self.do_shift(val, 1, dir != 0, shift_type, Size::Word);
            ae_write(ea.write(self, bus, Size::Word, result))?;
            return Ok(8 + ea_extra);
        }

        // Register shift: 1110 ccc d SS i tt rrr
        let dir = (opcode >> 8) & 1;
        let size = Size::from_bits(opcode >> 6).ok_or(StepError::IllegalAddressing)?;
        let count_reg = opcode & 0x0020 != 0; // bit 5: 0=immediate, 1=Dn
        let shift_type = (opcode >> 3) & 0b11;
        let dst_reg = (opcode & 0b111) as usize;
        let count_raw = ((opcode >> 9) & 0b111) as u32;

        let count = if count_reg {
            self.d[count_raw as usize] % 64
        } else {
            if count_raw == 0 { 8 } else { count_raw }
        };

        let val = self.d[dst_reg] & size.mask();
        let result = self.do_shift(val, count, dir != 0, shift_type, size);
        ae_write(Operand::DataReg(dst_reg).write(self, bus, size, result))?;
        let base = if size == Size::Long { 8 } else { 6 };
        Ok(base + 2 * count)
    }

    /// Performs a shift/rotation and sets the flags.
    ///
    /// `shift_type`: 00=AS, 01=LS, 10=ROX, 11=RO
    fn do_shift(&mut self, val: u32, count: u32, left: bool, shift_type: u16, size: Size) -> u32 {
        let mask = size.mask();
        let msb = size.msb();
        let bits = size.bytes() * 8;
        let val = val & mask;

        self.set_flag(ccr::C, false);
        self.set_flag(ccr::V, false);

        let result = if count == 0 {
            // No shift: C unchanged, X unchanged (except ROX where C = X)
            if shift_type == 0b10 {
                self.set_flag(ccr::C, self.flag(ccr::X));
            }
            val
        } else {
            match shift_type {
                0b00 => {
                    // ASL/ASR
                    if left {
                        let result = if count >= bits {
                            0
                        } else {
                            (val << count) & mask
                        };
                        let last_out = if count > bits {
                            false
                        } else if count == bits {
                            val & 1 != 0
                        } else {
                            (val >> (bits - count)) & 1 != 0
                        };
                        self.set_flag(ccr::C, last_out);
                        self.set_flag(ccr::X, last_out);
                        // V = 1 if a sign bit changed: original or intermediate val
                        let v = if count >= bits {
                            val != 0 // everything shifted out, sign became 0, overflow if val≠0
                        } else {
                            // Check whether the sign varied over count steps
                            let mask_hi = if count >= bits {
                                mask
                            } else {
                                (mask >> (bits - count - 1)) << (bits - count - 1)
                            };
                            let _ = mask_hi;
                            // Simple approach: does a bit exist in [val.msb..val.msb+count] that differs from the initial sign
                            let orig_sign = val & msb != 0;
                            (0..count).any(|i| ((val << i) & mask & msb != 0) != orig_sign)
                                || (result & msb != 0) != orig_sign
                        };
                        self.set_flag(ccr::V, v);
                        result
                    } else {
                        let sign_bit = val & msb != 0;
                        let sign_fill = if sign_bit { mask } else { 0 };
                        let result = if count >= bits {
                            sign_fill
                        } else {
                            // Arithmetic right shift : sign-extend
                            let signed = (val as i32) << (32 - bits);
                            ((signed >> count) as u32 >> (32 - bits)) & mask
                        };
                        let last_out = if count > bits {
                            sign_bit
                        } else if count == bits {
                            val & msb != 0
                        } else {
                            (val >> (count - 1)) & 1 != 0
                        };
                        self.set_flag(ccr::C, last_out);
                        self.set_flag(ccr::X, last_out);
                        result
                    }
                }
                0b01 => {
                    // LSL/LSR
                    if left {
                        let result = if count >= bits {
                            0
                        } else {
                            val.wrapping_shl(count) & mask
                        };
                        let last_out = if count <= bits {
                            (val >> (bits - count)) & 1 != 0
                        } else {
                            false
                        };
                        self.set_flag(ccr::C, last_out);
                        self.set_flag(ccr::X, last_out);
                        result
                    } else {
                        let result = if count >= bits { 0 } else { val >> count };
                        let last_out = if count <= bits {
                            (val >> (count - 1)) & 1 != 0
                        } else {
                            false
                        };
                        self.set_flag(ccr::C, last_out);
                        self.set_flag(ccr::X, last_out);
                        result
                    }
                }
                0b10 => {
                    // ROXL/ROXR (rotation with X)
                    let effective = count % (bits + 1);
                    if effective == 0 {
                        self.set_flag(ccr::C, self.flag(ccr::X));
                        val
                    } else if left {
                        let x_bit: u64 = if self.flag(ccr::X) { 1 } else { 0 };
                        let wide = (val as u64) | (x_bit << bits);
                        let n = bits + 1;
                        let rotated =
                            ((wide << effective) | (wide >> (n - effective))) & ((1u64 << n) - 1);
                        let new_x = (rotated >> bits) & 1 != 0;
                        self.set_flag(ccr::C, new_x);
                        self.set_flag(ccr::X, new_x);
                        rotated as u32 & mask
                    } else {
                        let x_bit: u64 = if self.flag(ccr::X) { 1 } else { 0 };
                        let wide = (val as u64) | (x_bit << bits);
                        let n = bits + 1;
                        let rotated =
                            ((wide >> effective) | (wide << (n - effective))) & ((1u64 << n) - 1);
                        let new_x = (rotated >> bits) & 1 != 0;
                        self.set_flag(ccr::C, new_x);
                        self.set_flag(ccr::X, new_x);
                        rotated as u32 & mask
                    }
                }
                0b11 => {
                    // ROL/ROR
                    let effective = count % bits;
                    if effective == 0 {
                        let last = if left { val & 1 != 0 } else { val & msb != 0 };
                        self.set_flag(ccr::C, last);
                        val
                    } else if left {
                        let result = ((val << effective) | (val >> (bits - effective))) & mask;
                        self.set_flag(ccr::C, result & 1 != 0);
                        result
                    } else {
                        let result = ((val >> effective) | (val << (bits - effective))) & mask;
                        self.set_flag(ccr::C, result & msb != 0);
                        result
                    }
                }
                _ => unreachable!(),
            }
        };

        self.set_flag(ccr::N, result & msb != 0);
        self.set_flag(ccr::Z, result == 0);
        result
    }

    // =========================================================================
    // Flag computation
    // =========================================================================

    pub(crate) fn set_logic_flags(&mut self, value: u32, size: Size) {
        let v = value & size.mask();
        self.set_flag(ccr::N, v & size.msb() != 0);
        self.set_flag(ccr::Z, v == 0);
        self.set_flag(ccr::V, false);
        self.set_flag(ccr::C, false);
    }

    fn add_with_flags(&mut self, a: u32, b: u32, size: Size) -> u32 {
        let mask = size.mask();
        let msb = size.msb();
        let a = a & mask;
        let b = b & mask;
        let sum64 = (a as u64) + (b as u64);
        let sum = (sum64 as u32) & mask;

        let carry = sum64 > mask as u64;
        let overflow = ((a ^ sum) & (b ^ sum) & msb) != 0;

        self.set_flag(ccr::N, sum & msb != 0);
        self.set_flag(ccr::Z, sum == 0);
        self.set_flag(ccr::V, overflow);
        self.set_flag(ccr::C, carry);
        self.set_flag(ccr::X, carry);
        sum
    }

    fn sub_with_flags(&mut self, a: u32, b: u32, size: Size) -> u32 {
        let mask = size.mask();
        let msb = size.msb();
        let a = a & mask;
        let b = b & mask;
        let diff64 = (a as u64).wrapping_sub(b as u64);
        let diff = (diff64 as u32) & mask;

        let borrow = b > a;
        let overflow = ((a ^ b) & (a ^ diff) & msb) != 0;

        self.set_flag(ccr::N, diff & msb != 0);
        self.set_flag(ccr::Z, diff == 0);
        self.set_flag(ccr::V, overflow);
        self.set_flag(ccr::C, borrow);
        self.set_flag(ccr::X, borrow);
        diff
    }

    fn addx_with_flags(&mut self, a: u32, b: u32, x: u32, size: Size) -> u32 {
        let mask = size.mask();
        let msb = size.msb();
        let a = a & mask;
        let b = b & mask;
        let sum64 = (a as u64) + (b as u64) + (x as u64);
        let sum = (sum64 as u32) & mask;

        let carry = sum64 > mask as u64;
        let overflow = ((a ^ sum) & (b ^ sum) & msb) != 0;

        self.set_flag(ccr::N, sum & msb != 0);
        if sum != 0 {
            self.set_flag(ccr::Z, false);
        }
        self.set_flag(ccr::V, overflow);
        self.set_flag(ccr::C, carry);
        self.set_flag(ccr::X, carry);
        sum
    }

    /// Subtraction for CMP: sets N Z V C but **not X**.
    fn cmp_flags(&mut self, a: u32, b: u32, size: Size) {
        let saved_x = self.flag(ccr::X);
        self.sub_with_flags(a, b, size);
        self.set_flag(ccr::X, saved_x);
    }

    fn subx_with_flags(&mut self, a: u32, b: u32, x: u32, size: Size) -> u32 {
        let mask = size.mask();
        let msb = size.msb();
        let a = a & mask;
        let b = b & mask;
        let diff64 = (a as u64).wrapping_sub(b as u64).wrapping_sub(x as u64);
        let diff = (diff64 as u32) & mask;

        let borrow = (b as u64) + (x as u64) > (a as u64);
        let overflow = ((a ^ b) & (a ^ diff) & msb) != 0;

        self.set_flag(ccr::N, diff & msb != 0);
        if diff != 0 {
            self.set_flag(ccr::Z, false);
        }
        self.set_flag(ccr::V, overflow);
        self.set_flag(ccr::C, borrow);
        self.set_flag(ccr::X, borrow);
        diff
    }

    pub fn test_condition(&self, cc: u16) -> bool {
        let n = self.flag(ccr::N);
        let z = self.flag(ccr::Z);
        let v = self.flag(ccr::V);
        let c = self.flag(ccr::C);
        match cc {
            0b0000 => true,
            0b0001 => false,
            0b0010 => !c && !z,
            0b0011 => c || z,
            0b0100 => !c,
            0b0101 => c,
            0b0110 => !z,
            0b0111 => z,
            0b1000 => !v,
            0b1001 => v,
            0b1010 => !n,
            0b1011 => n,
            0b1100 => n == v,
            0b1101 => n != v,
            0b1110 => !z && (n == v),
            0b1111 => z || (n != v),
            _ => unreachable!(),
        }
    }
}
