//! Core of the Motorola 68000 CPU.
//!
//! Primarily targets the original **MC68000** (Atari ST, Amiga 500…),
//! since extended to a subset of the **68010** (see [`CpuType`]) — a first
//! step towards 68020/68030/68040 with a view to emulating NeXT computers.
//! The 68000 remains the default behavior ([`Cpu::new`]): no behavioral
//! difference for a caller that never touches `cpu_type`.

use crate::bus::Bus;

/// 68000 addressing mask: 24-bit (16 MB) address bus.
pub const ADDR_MASK: u32 = 0x00FF_FFFF;

/// Condition Code Register (CCR, low byte of SR) bits.
pub mod ccr {
    /// Carry.
    pub const C: u16 = 1 << 0;
    /// Overflow — signed overflow.
    pub const V: u16 = 1 << 1;
    /// Zero — result is zero.
    pub const Z: u16 = 1 << 2;
    /// Negative — sign bit of the result.
    pub const N: u16 = 1 << 3;
    /// Extend — extended carry (multi-precision arithmetic).
    pub const X: u16 = 1 << 4;
}

/// Status Register (SR) system byte bits.
pub mod sr {
    /// Supervisor — supervisor mode (vs. user).
    pub const S: u16 = 1 << 13;
    /// Trace — single-step execution.
    pub const T: u16 = 1 << 15;
    /// Interrupt priority level mask (IPL, bits 8-10).
    pub const IPL_MASK: u16 = 0b111 << 8;
}

/// Index of the stack pointer within the address register bank (A7).
const SP: usize = 7;

/// 68k core variant emulated by a given [`Cpu`].
///
/// Chosen at runtime (not a Cargo feature) — a single binary can run a
/// 68000 system and a 68010+ system depending on the value of
/// [`Cpu::cpu_type`], on the same model as `AtariModel`/`MachineProfile`
/// (see `systems::atari_st::model`). Currently limited to 68000/68010/
/// 68020 (subset, see `addressing::resolve_indexed_full` and
/// `execute::op_line_4` for what's covered): 68030/68040 will be added here
/// over time, only once actually implemented (no untested "stub" variant).
///
/// Variants are declared in order of increasing capability: `PartialOrd`/
/// `Ord` enable gates like `cpu_type >= CpuType::M68020` that remain valid
/// as-is when a further variant is added.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum CpuType {
    M68000,
    M68010,
    M68020,
}

/// Full state of a 68000 core.
///
/// The CPU holds no memory: all accesses go through a [`Bus`] supplied by
/// the caller to the methods that need it.
#[derive(Debug, Clone)]
pub struct Cpu {
    /// Data registers D0-D7.
    pub d: [u32; 8],
    /// Address registers A0-A7; A7 is the **active** stack pointer.
    pub a: [u32; 8],
    /// Program counter.
    pub pc: u32,
    /// Status Register (system byte + CCR).
    pub sr: u16,
    /// User stack pointer, saved while in supervisor mode.
    pub usp: u32,
    /// Supervisor stack pointer, saved while in user mode.
    pub ssp: u32,
    /// Cycles consumed since the last counter reset.
    pub cycles: u64,
    /// Prefetch queue (68000 instruction pipeline: 2 words max).
    /// Words are consumed FIFO before any bus read during a fetch.
    pub prefetch: [u16; 2],
    /// Number of valid words in `prefetch` (0, 1, or 2).
    pub prefetch_len: usize,
    /// Opcode of the current instruction (Instruction Register, used for exception frames).
    pub current_ir: u16,
    /// Faulting address if an instruction fetch hit an odd address.
    /// `Some((fault_addr, is_write, pc_at_fault))` triggers an address error.
    pub pending_address_error: Option<(u32, bool, u32)>,
    /// PC to record in the address-error frame for a data access via the
    /// last resolved EA. Depends on the addressing mode (cf. resolve_ea).
    pub ea_frame_pc: u32,
    /// True if the last resolved EA is a PC-relative mode ((d16,PC) or (d8,PC,Xn)).
    /// These modes access program space (FC=2/6), not data space (FC=1/5).
    pub ea_is_pc_relative: bool,
    /// IR to use in a write AE's frame when it differs from current_ir.
    /// For MOVE.w/b with dst -(An), the 68000 has already advanced its pipeline before the write:
    /// the IR in the frame is the next word in the program stream (bus.read16(pc)),
    /// not the current opcode.
    pub write_ae_ir: Option<u16>,
    /// Extra effective address calculation cycles for the last resolved EA
    /// (depends on the addressing mode and size — cf. resolve_ea).
    /// Each instruction handler adds it to its base cost after calling
    /// resolve_ea, on the same principle as ea_frame_pc/ea_is_pc_relative.
    pub ea_extra_cycles: u32,
    /// Prefix (in cycles) to add before `ea_extra + 50` if the immediately
    /// following `ae_read` triggers an address error. Calibrated case by
    /// case against ProcessorTests (all three forms coexist, no single
    /// general rule):
    ///   - 4 (default): plain read of a source operand (DIVU/DIVS, `<ea>,Dn`,
    ///     TST, CMP...) — the prefix is just the opcode fetch.
    ///   - 0: RMW re-read of the destination value for a two-operand
    ///     register+memory `Dn,<ea>` (`OR/AND/EOR/ADD/SUB Dn,<ea>`).
    ///   - 8: RMW re-read in the immediate-to-memory family
    ///     (`ORI/ANDI/SUBI/ADDI/EORI`, sharing `op_line_0`) — the prefix
    ///     includes the immediate fetch in addition to the opcode.
    /// Reset to 4 at the start of each `step()`.
    pub fault_prefix: u32,
    /// Diagnostic log of exceptions taken (vector, pushed PC, faulting
    /// address [group 0 only, 0 otherwise], write?, handler address read
    /// from the vector table [`bus.read32(vector*4)`], cycles) — circular
    /// buffer (see `EXCEPTION_LOG_CAP`), pure record with no effect on
    /// execution. handler_pc allows distinguishing a vector still pointing
    /// at the ROM's default handler from one patched by the running
    /// program (handler in low RAM). Used by external diagnostic harnesses
    /// (crates/app/examples/*) to retrace a sequence of exceptions without
    /// instrumenting every call site individually.
    pub exception_log: std::collections::VecDeque<(u8, u32, u32, bool, u32, u64)>,
    /// True if the last instruction completed normally (no internal
    /// exception) with the SR's T bit set — BUT only if it was already set
    /// BEFORE this instruction: if the instruction itself just set it
    /// (MOVE/ANDI/ORI/EORI to SR, RTE), real 68000 hardware still executes
    /// one more instruction before tracing takes effect (same hardware
    /// mechanism as the IPL mask delay, see
    /// [`Cpu::sr_write_pending_delay`]) — empirically verified against
    /// Hatari on 2026-08-04 (see `Cpu::step`).
    /// `step` does NOT take the trace itself (the TomHarte conformance
    /// suite deliberately captures the effect of a single instruction
    /// without chaining into the trace, even when T=1 on entry): it is up
    /// to the caller to check this field after each `step` and call
    /// [`Cpu::take_trace_exception`] if it wants the real effect, before
    /// the next `step` (to preserve real silicon's trace-before-interrupt
    /// priority).
    pub trace_pending: bool,
    /// True right after an instruction that modified the SR (and thus
    /// potentially the IPL mask) — `MOVE`/`ANDI`/`ORI`/`EORI to SR`, `RTE`,
    /// `STOP`. Documented (MC68000 User Manual, CPU Space cycle): the IPL
    /// lines are only re-evaluated on the 3rd falling clock edge of an
    /// instruction's last bus cycle, which delays recognition of a
    /// just-lowered IPL mask by exactly one instruction — a program that
    /// unmasks an interrupt is thus guaranteed at least one instruction
    /// before it can actually trigger. `take_interrupt` consumes this flag
    /// (resets it to false) on every call, whether an interrupt is taken or
    /// not.
    pub sr_write_pending_delay: bool,
    /// True if a double bus fault has been detected: a bus/address error
    /// occurred while the CPU was already pushing the frame of a previous
    /// bus/address error (or reading its vector), typically because the
    /// active stack pointer itself points into an unmapped area. On real
    /// 68000 silicon, the CPU then halts permanently (HALT): only an
    /// external hardware `/RESET` can bring it back — it does not "bounce"
    /// indefinitely on the vector.
    ///
    /// Verified against Hatari (`src/cpu/newcpu.c`, `Exception()`:
    /// `if ((m68k_areg(regs,7) & 1) || exception_in_exception < 0)
    /// cpu_halt(CPU_HALT_DOUBLE_FAULT);`): it's the same immediate halt,
    /// not a bounce — a theory once entertained on the strength of CLK's
    /// code (github.com/TomHarte/CLK), which bounces indefinitely for
    /// never having coded this case (a TODO acknowledged by its author),
    /// but CLK is not the authoritative reference here, Hatari is
    /// (`Rick_Dangerous.stx` boots under Hatari, confirmed with no
    /// bus/address error anywhere in its whole startup via `--trace
    /// cpu_exception` — the divergence was therefore upstream, in our own
    /// emulation, not in this mechanism).
    /// [`Cpu::step`] returns [`crate::execute::StepError::DoubleFault`] as
    /// long as this field stays true, rather than continuing to execute
    /// instructions in an undefined state.
    pub halted: bool,
    /// Emulated 68k variant — see [`CpuType`]. Determines in particular the
    /// shape of exception frames (format word on 68010+) and the
    /// availability of MOVEC/MOVES/RTD.
    pub cpu_type: CpuType,
    /// Vector Base Register (68010+ only): offsets the exception vector
    /// table by `vbr` bytes instead of fixing it at address 0 — see
    /// [`Cpu::take_exception`]/[`Cpu::take_interrupt`]. Defaults to 0,
    /// which exactly reproduces the 68000's fixed addressing even when
    /// this field is read unconditionally on `cpu_type` (see those
    /// methods). Not affected by [`Cpu::reset`]: on real silicon, the
    /// reset vector itself is always read at physical address 0, since VBR
    /// is not yet initialized at that point.
    pub vbr: u32,
    /// Source Function Code (68010+, MOVEC/MOVES). 3 significant bits.
    pub sfc: u8,
    /// Destination Function Code (68010+, MOVEC/MOVES). 3 significant bits.
    pub dfc: u8,
}

/// Max size of `Cpu::exception_log` — enough to cover several frames of
/// normal TRAP/interrupt activity without growing unbounded over a long run.
pub const EXCEPTION_LOG_CAP: usize = 4096;

impl Default for Cpu {
    fn default() -> Self {
        Self::new()
    }
}

impl Cpu {
    /// Creates a CPU in a neutral state (before `reset`).
    pub fn new() -> Self {
        Cpu {
            d: [0xFFFF_FFFF; 8],
            a: [0xFFFF_FFFF; 8],
            pc: 0,
            sr: sr::S,
            usp: 0,
            ssp: 0,
            cycles: 0,
            prefetch: [0; 2],
            prefetch_len: 0,
            current_ir: 0,
            pending_address_error: None,
            ea_frame_pc: 0,
            ea_is_pc_relative: false,
            write_ae_ir: None,
            ea_extra_cycles: 0,
            fault_prefix: 4,
            exception_log: std::collections::VecDeque::new(),
            trace_pending: false,
            sr_write_pending_delay: false,
            halted: false,
            cpu_type: CpuType::M68000,
            vbr: 0,
            sfc: 0,
            dfc: 0,
        }
    }

    /// Records an entry in `exception_log`, purging the oldest ones beyond
    /// `EXCEPTION_LOG_CAP`.
    fn log_exception(&mut self, vector: u32, pc: u32, fault_addr: u32, is_write: bool, handler_pc: u32) {
        if self.exception_log.len() >= EXCEPTION_LOG_CAP {
            self.exception_log.pop_front();
        }
        self.exception_log.push_back((vector as u8, pc, fault_addr, is_write, handler_pc, self.cycles));
    }

    /// Loads words into the prefetch queue (used by test harnesses).
    ///
    /// Words are supplied in reading order (first word = next to be consumed).
    pub fn load_prefetch(&mut self, words: &[u16]) {
        let n = words.len().min(2);
        self.prefetch_len = n;
        for i in 0..n {
            self.prefetch[i] = words[i];
        }
    }

    /// Whether the CPU is in supervisor mode.
    #[inline]
    pub fn supervisor(&self) -> bool {
        self.sr & sr::S != 0
    }

    /// Active stack pointer (A7).
    #[inline]
    pub fn sp(&self) -> u32 {
        self.a[SP]
    }

    /// Sets the active stack pointer (A7).
    #[inline]
    pub fn set_sp(&mut self, value: u32) {
        self.a[SP] = value;
    }

    /// Toggles the supervisor bit, swapping the A7 stack pointers.
    ///
    /// On the 68000, USP and SSP are two distinct physical registers; only
    /// one of them is exposed via A7 depending on the current mode. This
    /// method handles the swap so that `self.a[7]` always reflects the
    /// stack for the correct mode.
    pub fn set_supervisor(&mut self, supervisor: bool) {
        if supervisor == self.supervisor() {
            return;
        }
        if supervisor {
            // user -> supervisor: save the current USP, restore the SSP.
            self.usp = self.a[SP];
            self.a[SP] = self.ssp;
            self.sr |= sr::S;
        } else {
            // supervisor -> user: save the current SSP, restore the USP.
            self.ssp = self.a[SP];
            self.a[SP] = self.usp;
            self.sr &= !sr::S;
        }
    }

    /// Performs a hardware **reset**.
    ///
    /// The 68000 loads the initial SSP from address `0x000000` and the
    /// initial PC from `0x000004` (the first two longwords of the reset
    /// vector), enters supervisor mode, disables trace, IPL = 7.
    pub fn reset(&mut self, bus: &mut impl Bus) {
        self.sr = sr::S | sr::IPL_MASK; // supervisor, IPL=7, trace off
        let ssp = bus.read32(0x0000_0000);
        let pc = bus.read32(0x0000_0004);
        self.ssp = ssp;
        self.a[SP] = self.ssp;
        self.pc = pc;
        self.cycles = 0;
        self.halted = false;
    }

    // --- Exception mechanism ----------------------------------------------

    /// Triggers an exception: pushes SR + PC onto the supervisor stack,
    /// enters supervisor mode, disables trace, jumps to the vector.
    ///
    /// `vector` is the vector number (0-255). Vector address =
    /// `vbr + vector*4` (on the 68000, `vbr` is always 0: fixed addressing
    /// identical to before).
    ///
    /// "Short" frame: on the 68000, SR (word) + PC (longword), 6 bytes. On
    /// 68010+, a 3rd format word is appended after the PC — format nibble 0
    /// (standard frame) followed by the 12-bit vector number
    /// (`(vector << 2) & 0x0FFF`) — see the M68000 Family Programmer's
    /// Reference Manual, 68010 addendum. Explicit gate on `cpu_type` rather
    /// than a no-op reduction like for `vbr`: this extra word is genuinely
    /// 2 more bytes of stack than the 68000 ever wrote.
    pub fn take_exception(&mut self, bus: &mut impl Bus, vector: u32, pc_to_push: u32) {
        // Switch to supervisor mode without changing the CCR flags
        let saved_sr = self.sr;
        if !self.supervisor() {
            self.usp = self.a[SP];
            self.a[SP] = self.ssp;
        }
        self.sr = (saved_sr | sr::S) & !sr::T;
        self.sr &= 0xA71F; // mask reserved bits

        let is_68010 = self.cpu_type == CpuType::M68010;
        // Push SR then PC (format: SR word, PC longword), plus the format
        // word on 68010+.
        let frame_size = if is_68010 { 8 } else { 6 };
        let sp = self.a[SP].wrapping_sub(frame_size);
        self.a[SP] = sp;
        bus.write16(sp & ADDR_MASK, saved_sr);
        bus.write32((sp + 2) & ADDR_MASK, pc_to_push);
        if is_68010 {
            let format_word = (vector << 2) & 0x0FFF; // format 0 (high nibble) + vector offset
            bus.write16((sp + 6) & ADDR_MASK, format_word as u16);
        }
        // A bus/address error here (stack outside any mapped area) is a
        // double bus fault — see `Cpu::halted`'s doc. Verified against
        // Hatari (`src/cpu/newcpu.c`, `Exception()`): it's exactly the same
        // behavior there too (`cpu_halt(CPU_HALT_DOUBLE_FAULT)`, immediate
        // halt, NOT a bounce/retry) — Hatari only differs from us upstream,
        // in the memory model ("floating" RAM below 4 MB that almost never
        // triggers this case in practice), not in the reaction once the
        // case is reached.
        if bus.take_bus_fault().is_some() {
            self.halted = true;
            return;
        }

        // Read the vector's address
        let vec_addr = self.vbr.wrapping_add(vector * 4) & ADDR_MASK;
        let new_pc = bus.read32(vec_addr);
        if bus.take_bus_fault().is_some() {
            self.halted = true;
            return;
        }
        self.log_exception(vector, pc_to_push, 0, false, new_pc);
        // TomHarte convention: final.pc = m_au = new_pc + 4.
        // Our model: cpu.pc + 4 = final.pc → cpu.pc = new_pc.
        self.pc = new_pc;
    }

    /// Triggers an address error exception (vector 3).
    ///
    /// 14-byte frame:
    ///   SP+0..1  : access_info = (IR & 0xFFE0) | (R/W << 4) | FC
    ///   SP+2     : 0x00
    ///   SP+3..5  : fault_addr (odd address, 24 bits)
    ///   SP+6..7  : IR (opcode)
    ///   SP+8..9  : saved SR
    ///   SP+10..13: pipeline PC at the time of the access
    pub fn take_address_error(&mut self, bus: &mut impl Bus, fault_addr: u32, is_write: bool) {
        self.take_address_error_at(bus, fault_addr, is_write, None)
    }

    pub fn take_address_error_at(
        &mut self,
        bus: &mut impl Bus,
        fault_addr: u32,
        is_write: bool,
        explicit_pc: Option<u32>,
    ) {
        self.take_address_error_full(bus, fault_addr, is_write, explicit_pc, false)
    }

    pub fn take_address_error_full(
        &mut self,
        bus: &mut impl Bus,
        fault_addr: u32,
        is_write: bool,
        explicit_pc: Option<u32>,
        is_instruction_fetch: bool,
    ) {
        self.take_group0_exception(bus, 3, fault_addr, is_write, explicit_pc, is_instruction_fetch)
    }

    /// Triggers a bus error exception (vector 2): an access (read or write)
    /// to an address with no chip select behind it — the physical "hole"
    /// between the top of installed RAM and the start of ROM on a real
    /// ST/STE. Same frame format as the address error (14 bytes), only the
    /// vector differs (2 instead of 3). This is the mechanism many
    /// programs/demos use to detect the amount of installed RAM: they
    /// install their own handler at vector $8, then write to increasing
    /// addresses until they crash the bus.
    pub fn take_bus_error_full(
        &mut self,
        bus: &mut impl Bus,
        fault_addr: u32,
        is_write: bool,
        explicit_pc: Option<u32>,
        is_instruction_fetch: bool,
    ) {
        self.take_group0_exception(bus, 2, fault_addr, is_write, explicit_pc, is_instruction_fetch)
    }

    fn take_group0_exception(
        &mut self,
        bus: &mut impl Bus,
        vector: u32,
        fault_addr: u32,
        is_write: bool,
        explicit_pc: Option<u32>,
        is_instruction_fetch: bool,
    ) {
        // 68010+: the bus/address error frame changes shape (the "long"
        // frame of 29 words, format $8 — captures the CPU's complete
        // internal state to allow the faulting instruction to be restarted
        // after correction by the handler, the basis of demand paging with
        // an MMU). Not implemented: no 68010+ system in this project yet
        // triggers a bus/address error (no MMU), so a loud failure rather
        // than a silently wrong 68000 frame — to be revisited when PMMU
        // (NeXT) work justifies it.
        assert!(
            self.cpu_type == CpuType::M68000,
            "68010+ bus/address error frame (long format) not implemented"
        );
        let saved_sr = self.sr;
        // On real 68000 hardware, the pipeline performs a prefetch before
        // any write cycle, advancing the PC by 2 more than for read
        // cycles.
        let pc_at_access = explicit_pc.unwrap_or_else(|| {
            if is_write {
                self.pc.wrapping_add(2)
            } else {
                self.pc
            }
        });
        let ir = if is_write {
            self.write_ae_ir.unwrap_or(self.current_ir)
        } else {
            self.current_ir
        };
        self.write_ae_ir = None;

        if !self.supervisor() {
            self.usp = self.a[7];
            self.a[7] = self.ssp;
        }
        self.sr = (saved_sr | sr::S) & !sr::T;
        self.sr &= 0xA71F;

        // FC : 1=user data, 2=user program, 5=supervisor data, 6=supervisor program
        // PC-relative modes ((d16,PC),(d8,PC,Xn)) access program space.
        let supervisor = saved_sr & sr::S != 0;
        let is_program = is_instruction_fetch || self.ea_is_pc_relative;
        let fc: u16 = match (supervisor, is_program) {
            (false, false) => 1, // user data
            (false, true) => 2,  // user program
            (true, false) => 5,  // supervisor data
            (true, true) => 6,   // supervisor program
        };
        let rw_bit: u16 = if is_write { 0 } else { 1 };
        let access_info = (ir & 0xFFE0) | (rw_bit << 4) | fc;

        let sp = self.a[7].wrapping_sub(14);
        self.a[7] = sp;

        bus.write16(sp & ADDR_MASK, access_info);
        // The faulting address is stored as 32 bits (+2..+5), MSB included.
        bus.write32(sp.wrapping_add(2) & ADDR_MASK, fault_addr);
        bus.write16(sp.wrapping_add(6) & ADDR_MASK, ir);
        bus.write16(sp.wrapping_add(8) & ADDR_MASK, saved_sr);
        bus.write32(sp.wrapping_add(10) & ADDR_MASK, pc_at_access);
        // A bus/address error while pushing THIS frame (stack outside any
        // mapped area) is a double bus fault — see `Cpu::halted`'s doc:
        // verified against Hatari (`src/cpu/newcpu.c`, `Exception()`,
        // `m68k_areg(regs,7) & 1 || exception_in_exception < 0` →
        // `cpu_halt(CPU_HALT_DOUBLE_FAULT)`), it's an immediate halt there
        // too, never a bounce.
        if bus.take_bus_fault().is_some() {
            self.halted = true;
            return;
        }

        let new_pc = bus.read32(self.vbr.wrapping_add(vector * 4) & ADDR_MASK);
        if bus.take_bus_fault().is_some() {
            self.halted = true;
            return;
        }
        self.log_exception(vector, pc_at_access, fault_addr, is_write, new_pc);
        self.pc = new_pc;
    }

    /// Checks whether a peripheral is requesting an interrupt at a level
    /// higher than the current IPL mask (`SR` bits 8-10) and, if so, takes
    /// it. Called by [`crate::execute`]`::Cpu::step` before fetching each
    /// instruction.
    ///
    /// Taking an interrupt: saves SR+PC (standard 6-byte frame — NOT the
    /// 14-byte Group 0/1 frame of address/bus errors), enters supervisor
    /// mode, disables trace, raises the IPL mask to the accepted level (so
    /// as not to re-trigger until it drops), then performs the acknowledge
    /// cycle ([`Bus::irq_ack`]) to obtain the vector and jumps to the
    /// handler. Returns `Some(cycles)` if an interrupt was taken (nothing
    /// else executes this step), `None` otherwise.
    ///
    /// Level 7 is the 68000's only non-maskable interrupt: it is always
    /// taken regardless of the current mask. Levels 1-6 are only taken if
    /// strictly greater than the mask (an interrupt at the same level or
    /// lower does not trigger until the mask has dropped).
    ///
    /// Approximate cost (44 cycles, the usual figure cited for interrupt
    /// exception processing): no TomHarte suite covers interrupts (they
    /// are external events, not opcodes) — to be calibrated later against
    /// a hardware reference.
    pub fn take_interrupt(&mut self, bus: &mut impl Bus) -> Option<u32> {
        // Consumed on every call (one instruction = one call), whether an
        // interrupt is taken or not — see the field's doc.
        let delay = self.sr_write_pending_delay;
        self.sr_write_pending_delay = false;

        let level = bus.irq_level() & 0x7;
        if level == 0 {
            return None;
        }
        if delay {
            return None;
        }
        let current_mask = ((self.sr & sr::IPL_MASK) >> 8) as u8;
        if level != 7 && level <= current_mask {
            return None;
        }

        let saved_sr = self.sr;
        if !self.supervisor() {
            self.usp = self.a[SP];
            self.a[SP] = self.ssp;
        }
        self.sr = (saved_sr | sr::S) & !sr::T;
        self.sr = (self.sr & !sr::IPL_MASK) | ((level as u16) << 8);
        self.sr &= 0xA71F;

        // Short frame: see `Cpu::take_exception`'s doc for the 68010+
        // format word (identical here, just duplicated since taking an
        // interrupt has its own push sequence — the vector, needed for the
        // format word, is only known after the acknowledge cycle).
        let is_68010 = self.cpu_type == CpuType::M68010;
        let frame_size = if is_68010 { 8 } else { 6 };
        let sp = self.a[SP].wrapping_sub(frame_size);
        self.a[SP] = sp;
        bus.write16(sp & ADDR_MASK, saved_sr);
        bus.write32(sp.wrapping_add(2) & ADDR_MASK, self.pc);

        let vector = bus.irq_ack(level) as u32;
        if is_68010 {
            let format_word = (vector << 2) & 0x0FFF;
            bus.write16(sp.wrapping_add(6) & ADDR_MASK, format_word as u16);
        }
        let vec_addr = self.vbr.wrapping_add(vector * 4) & ADDR_MASK;
        let new_pc = bus.read32(vec_addr);
        self.log_exception(vector, self.pc, 0, false, new_pc);
        self.pc = new_pc;
        Some(44)
    }

    /// Takes the trace exception (vector 9) if [`Self::trace_pending`] is
    /// true — see its doc for the expected call contract. Standard 6-byte
    /// frame (SR+PC), like interrupts. Returns `Some(34)` (usual exception
    /// dispatch cost, cf. TRAP/TRAPV/ILLEGAL) if the trace was taken,
    /// `None` otherwise.
    pub fn take_trace_exception(&mut self, bus: &mut impl Bus) -> Option<u32> {
        if !self.trace_pending {
            return None;
        }
        self.trace_pending = false;
        let pc_push = self.pc;
        self.take_exception(bus, 9, pc_push);
        self.cycles = self.cycles.wrapping_add(34);
        Some(34)
    }

    // --- Grouped access to the CCR byte ------------------------------------

    /// Returns the low byte of the SR (the CCR flags: X N Z V C).
    #[inline]
    pub fn ccr(&self) -> u8 {
        self.sr as u8
    }

    /// Writes the SR, handling the USP/SSP switch if the S bit changes.
    pub fn write_sr(&mut self, new_sr: u16) {
        self.sr_write_pending_delay = true;
        let old_super = self.supervisor();
        self.sr = new_sr;
        let new_super = self.supervisor();
        if old_super && !new_super {
            self.ssp = self.a[7];
            self.a[7] = self.usp;
        } else if !old_super && new_super {
            self.usp = self.a[7];
            self.a[7] = self.ssp;
        }
    }

    /// Sets a CCR flag to `set`.
    #[inline]
    pub fn set_flag(&mut self, flag: u16, set: bool) {
        if set {
            self.sr |= flag;
        } else {
            self.sr &= !flag;
        }
    }

    /// Returns the state of a flag (CCR or system bit).
    #[inline]
    pub fn flag(&self, flag: u16) -> bool {
        self.sr & flag != 0
    }

    // --- Reading the instruction stream ------------------------------------

    /// Reads the 16-bit word pointed to by the PC and advances the PC by 2.
    ///
    /// The internal PC is 32 bits (the 68000 only masks on bus accesses).
    /// If the prefetch queue holds words, the first one is consumed
    /// (without a bus access). This lets test harnesses inject the
    /// hardware pipeline without overwriting data memory.
    pub fn fetch_word(&mut self, bus: &mut impl Bus) -> u16 {
        let addr = self.pc;
        self.pc = self.pc.wrapping_add(2);
        if self.prefetch_len > 0 {
            let word = self.prefetch[0];
            self.prefetch[0] = self.prefetch[1];
            self.prefetch_len -= 1;
            word
        } else {
            // Odd-address detection on instruction fetch (FC = program)
            if addr & 1 != 0 && self.pending_address_error.is_none() {
                self.pending_address_error = Some((addr, false, self.pc));
            }
            bus.read16(addr & ADDR_MASK)
        }
    }

    /// Reads the 32-bit longword pointed to by the PC and advances the PC by 4.
    pub fn fetch_long(&mut self, bus: &mut impl Bus) -> u32 {
        let hi = self.fetch_word(bus) as u32;
        let lo = self.fetch_word(bus) as u32;
        (hi << 16) | lo
    }
}
