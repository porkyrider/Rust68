//! CPU memory/bus interface.
//!
//! The 68000 has no knowledge of the physical memory layout: it issues
//! accesses on its bus, and the system (Atari, Amiga, test harness…) decides
//! what those addresses cover (RAM, ROM, peripheral registers…).
//!
//! The caller implements [`Bus`] for its system. Only [`Bus::read8`] and
//! [`Bus::write8`] are mandatory; 16- and 32-bit accesses are derived in
//! **big-endian** (the 68000's native ordering) and can be overridden for
//! better performance or to model specific behavior.

/// Memory bus as seen by the 68000 CPU.
///
/// The 68000 has a 24-bit (16 MB) address space. Addresses passed here are
/// already truncated by the CPU to 24 significant bits.
pub trait Bus {
    /// Reads a byte at address `addr`.
    fn read8(&mut self, addr: u32) -> u8;

    /// Writes a byte `value` at address `addr`.
    fn write8(&mut self, addr: u32, value: u8);

    /// Reads a big-endian word (16 bits) at address `addr`.
    fn read16(&mut self, addr: u32) -> u16 {
        let hi = self.read8(addr) as u16;
        let lo = self.read8(addr.wrapping_add(1)) as u16;
        (hi << 8) | lo
    }

    /// Reads a big-endian longword (32 bits) at address `addr`.
    fn read32(&mut self, addr: u32) -> u32 {
        let hi = self.read16(addr) as u32;
        let lo = self.read16(addr.wrapping_add(2)) as u32;
        (hi << 16) | lo
    }

    /// Writes a big-endian word (16 bits) at address `addr`.
    fn write16(&mut self, addr: u32, value: u16) {
        self.write8(addr, (value >> 8) as u8);
        self.write8(addr.wrapping_add(1), value as u8);
    }

    /// Writes a big-endian longword (32 bits) at address `addr`.
    fn write32(&mut self, addr: u32, value: u32) {
        self.write16(addr, (value >> 16) as u16);
        self.write16(addr.wrapping_add(2), value as u16);
    }

    /// Called when the CPU executes the RESET instruction (opcode 0x4E70).
    /// On the 68000, this instruction asserts the /RESET signal to external
    /// peripherals for 124 cycles. Default implementation is a no-op;
    /// override to propagate the reset to peripherals.
    fn reset_bus(&mut self) {}

    /// True if `addr` is subject to DRAM/video bus contention (the
    /// Shifter/video chip shares the RAM bus with the CPU and steals cycles
    /// from it in a periodic pattern — modeled by the CPU as rounding up to
    /// 4 cycles, cf. `Cpu::step`). Default: no contention (ROM, peripheral
    /// registers, or a system without a contention model). Atari ST/STE
    /// implementations must return `true` for addresses in populated RAM.
    fn is_contended(&self, addr: u32) -> bool {
        let _ = addr;
        false
    }

    /// Checked by [`crate::cpu::Cpu::step`] right after each bus transaction
    /// (instruction fetch, then again after execution) to know whether the
    /// most recent access touched an address with no chip select at all
    /// (the physical "hole" between the top of installed RAM and the start
    /// of ROM on a real ST/STE). If `Some((fault_addr, is_write))` is
    /// returned, the CPU immediately triggers a bus error (vector 2)
    /// instead of continuing normally, and the flag must be consumed (reset
    /// to `None`) by this call.
    ///
    /// Default implementation: never a bus error ("always available"
    /// mapping — the historical behavior of all existing `Bus`
    /// implementations, notably the ProcessorTests test harnesses, which
    /// have no notion of limited RAM).
    fn take_bus_fault(&mut self) -> Option<(u32, bool)> {
        None
    }

    /// Like [`Self::take_bus_fault`], but without consuming the flag — for
    /// multi-access instructions (MOVEM) that must stop at the FIRST
    /// faulting access (real silicon: the CPU aborts immediately, it does
    /// not keep trying the remaining registers in the list) while leaving
    /// [`Self::take_bus_fault`] intact so that `Cpu::step`'s generic
    /// post-instruction check triggers the exception normally, with the
    /// address of the FIRST faulting access rather than the last.
    ///
    /// Default implementation: no fault pending (consistent with
    /// [`Self::take_bus_fault`]'s default).
    fn has_pending_bus_fault(&self) -> bool {
        false
    }

    /// Interrupt level (IPL2-0) currently requested by peripherals to the
    /// CPU: 0 = no request, 1-6 = normal level, 7 = non-maskable (NMI).
    /// Checked by [`crate::cpu::Cpu::step`] before fetching each
    /// instruction: the CPU takes the interrupt if `level` is strictly
    /// greater than the SR's current IPL mask (or always for level 7).
    /// Default implementation: no request (ProcessorTests test harnesses,
    /// systems without an interrupting peripheral).
    fn irq_level(&self) -> u8 {
        0
    }

    /// Interrupt acknowledge (IACK) cycle for the level `level` the CPU
    /// just accepted: the peripheral returns the vector number (0-255) to
    /// use for building the handler address. Default implementation:
    /// autovector (24 + level), the most common case on Atari ST (GLUE
    /// HBL/VBL). A vectored peripheral (MFP 68901) must override this
    /// method to return its own programmed vector for that level.
    fn irq_ack(&mut self, level: u8) -> u8 {
        24 + level
    }
}

/// Event sink for [`TracingBus`] — decoupled from the output format (file,
/// in-memory channel, etc.) so that `TracingBus` stays generic and testable
/// without I/O.
pub trait TraceSink {
    /// `pc`: address of the current CPU instruction (supplied by the
    /// caller, see [`TracingBus::pc`]). `size`: 1/2/4 bytes. `value`: the
    /// value read/written, right-aligned (no shift based on `size`).
    fn on_read(&mut self, pc: u32, addr: u32, size: u8, value: u32);
    fn on_write(&mut self, pc: u32, addr: u32, size: u8, value: u32);
}

/// Interposed bus that logs every real bus transaction (exact access size —
/// .B/.W/.L — as issued by the CPU or any other generic caller on
/// `impl Bus`) to a [`TraceSink`], transparently to the code it passes
/// through (like [`TimedBus`] for timing).
///
/// Unlike ad hoc tracing scattered across individual `Bus` implementations
/// (which can only see one byte at a time as soon as the caller uses the
/// trait's default `read16`/`write16` implementations, or which has to
/// duplicate the tracing logic in every system-specific
/// `read16`/`write16`/`read32`/`write32` override), this decorator captures
/// the transaction as issued by the caller, at its real size, at a single
/// point — then delegates to `inner`, which can split it or short-circuit
/// it as it sees fit without ever needing to know about tracing.
///
/// `sink: None` is the fast path when tracing is disabled (a single
/// `Option::is_none` check per access, no allocation or formatting) —
/// allows the bus to always be wrapped in the main loop without duplicated
/// code for the "no tracing" case.
pub struct TracingBus<'b, B: Bus> {
    pub inner: &'b mut B,
    pub sink: Option<&'b mut dyn TraceSink>,
    /// PC of the current instruction — to be updated by the caller before
    /// each `Cpu::step` (the CPU does not expose its current PC to the `Bus`).
    pub pc: u32,
}

impl<'b, B: Bus> Bus for TracingBus<'b, B> {
    fn read8(&mut self, addr: u32) -> u8 {
        let v = self.inner.read8(addr);
        if let Some(sink) = self.sink.as_deref_mut() {
            sink.on_read(self.pc, addr, 1, v as u32);
        }
        v
    }

    fn write8(&mut self, addr: u32, value: u8) {
        if let Some(sink) = self.sink.as_deref_mut() {
            sink.on_write(self.pc, addr, 1, value as u32);
        }
        self.inner.write8(addr, value);
    }

    fn read16(&mut self, addr: u32) -> u16 {
        let v = self.inner.read16(addr);
        if let Some(sink) = self.sink.as_deref_mut() {
            sink.on_read(self.pc, addr, 2, v as u32);
        }
        v
    }

    fn write16(&mut self, addr: u32, value: u16) {
        if let Some(sink) = self.sink.as_deref_mut() {
            sink.on_write(self.pc, addr, 2, value as u32);
        }
        self.inner.write16(addr, value);
    }

    fn read32(&mut self, addr: u32) -> u32 {
        let v = self.inner.read32(addr);
        if let Some(sink) = self.sink.as_deref_mut() {
            sink.on_read(self.pc, addr, 4, v);
        }
        v
    }

    fn write32(&mut self, addr: u32, value: u32) {
        if let Some(sink) = self.sink.as_deref_mut() {
            sink.on_write(self.pc, addr, 4, value);
        }
        self.inner.write32(addr, value);
    }

    fn reset_bus(&mut self) {
        self.inner.reset_bus();
    }

    fn is_contended(&self, addr: u32) -> bool {
        self.inner.is_contended(addr)
    }

    fn take_bus_fault(&mut self) -> Option<(u32, bool)> {
        self.inner.take_bus_fault()
    }

    fn has_pending_bus_fault(&self) -> bool {
        self.inner.has_pending_bus_fault()
    }

    fn irq_level(&self) -> u8 {
        self.inner.irq_level()
    }

    fn irq_ack(&mut self, level: u8) -> u8 {
        self.inner.irq_ack(level)
    }
}

/// Interposed bus that applies the DRAM/video wait-state model (Steem
/// style: `RAM_ACCESS_WS`) to every real bus transaction, transparently to
/// all CPU code (addressing.rs / execute.rs) since they are generic over
/// `impl Bus` and have no idea they are going through this wrapper.
///
/// `pos` is the current position on the 4-cycle grid — initialized from
/// `Cpu.cycles` by `Cpu::step` before each instruction and never reset
/// between instructions, to stay in phase with the continuous video clock.
/// Each transaction nominally consumes 4 cycles; if `is_contended(addr)`
/// and the position isn't already aligned to 4, the gap is lost (rounded up
/// to the next multiple of 4) before the transaction — exactly the Steem
/// mechanism.
pub struct TimedBus<'b, B: Bus> {
    pub inner: &'b mut B,
    pub pos: u64,
    pub access_count: u32,
}

impl<'b, B: Bus> TimedBus<'b, B> {
    fn charge(&mut self, addr: u32) {
        if self.inner.is_contended(addr) {
            let rem = self.pos & 3;
            if rem != 0 {
                self.pos += 4 - rem;
            }
        }
        self.access_count += 1;
        self.pos += 4;
    }
}

impl<'b, B: Bus> Bus for TimedBus<'b, B> {
    fn read8(&mut self, addr: u32) -> u8 {
        self.charge(addr);
        self.inner.read8(addr)
    }

    fn write8(&mut self, addr: u32, value: u8) {
        self.charge(addr);
        self.inner.write8(addr, value);
    }

    fn read16(&mut self, addr: u32) -> u16 {
        self.charge(addr);
        self.inner.read16(addr)
    }

    fn write16(&mut self, addr: u32, value: u16) {
        self.charge(addr);
        self.inner.write16(addr, value);
    }

    fn read32(&mut self, addr: u32) -> u32 {
        // Real 16-bit bus: a long access = two independent word bus cycles,
        // each with its own grid alignment (not a single atomic 32-bit
        // access).
        let hi = self.read16(addr) as u32;
        let lo = self.read16(addr.wrapping_add(2)) as u32;
        (hi << 16) | lo
    }

    fn write32(&mut self, addr: u32, value: u32) {
        self.write16(addr, (value >> 16) as u16);
        self.write16(addr.wrapping_add(2), value as u16);
    }

    fn reset_bus(&mut self) {
        self.inner.reset_bus();
    }

    fn is_contended(&self, addr: u32) -> bool {
        self.inner.is_contended(addr)
    }

    fn take_bus_fault(&mut self) -> Option<(u32, bool)> {
        self.inner.take_bus_fault()
    }

    fn has_pending_bus_fault(&self) -> bool {
        self.inner.has_pending_bus_fault()
    }

    fn irq_level(&self) -> u8 {
        self.inner.irq_level()
    }

    fn irq_ack(&mut self, level: u8) -> u8 {
        self.inner.irq_ack(level)
    }
}
