//! Motorola MC68901 MFP (Multi-Function Peripheral).
//!
//! Atari ST's I/O chip (and many other 68k systems): 8 general-purpose
//! I/O lines (GPIP), 4 timers (A/B/C/D), a 16-channel interrupt
//! controller, and an RS-232 USART.
//!
//! This module models the chip **alone**, independently of any system
//! wiring: it is up to the caller (the Atari ST "board", not yet
//! implemented in this crate) to map [`Mfp::read`]/[`Mfp::write`] into
//! its [`crate::Bus`], to wire [`Mfp::iack`] to `Bus::irq_ack`, and to
//! connect [`Mfp::interrupt_requested`] to generating the IPL level
//! (the MFP is wired to IPL6 on real ST/STE, but that choice belongs
//! to the board, not the chip).
//!
//! ## Known limitations (v1)
//! - The USART is a "per-byte" model (no start/stop/parity bits nor
//!   real baud rate generation): `push_rx_byte`/`take_tx_byte`
//!   simulate reception/transmission at the byte level — a deliberate
//!   choice, RS232 is not in scope (no tested software depends on it
//!   at the bit level).
//! - `tick()` assumes a fixed 8 MHz CPU clock (ST/STE) to convert CPU
//!   cycles into real MFP clock cycles (2.4576 MHz) — a deliberate
//!   choice consistent with the rest of the board (`model::MachineProfile`
//!   documents `cpu_hz` as informative only for the same reason).
//! - If several timer periods elapse in a single `tick()` call (rare:
//!   `tick_delay_timer` already handles all decrements actually due,
//!   including a possible full wraparound of the 8-bit counter), the
//!   channel only arms once — consistent with real silicon, where
//!   `IPR` is a simple bit and not an occurrence counter: a second
//!   expiry before the first is acknowledged cannot be distinguished
//!   in hardware anyway.

/// Register offsets in the chip's address space (÷2: on real Atari
/// ST, the MFP is mapped to odd addresses 0xFFFA01, 0xFFFA03… every 2
/// bytes — this offset is the logical index, up to the caller to
/// convert it to/from the real bus address).
pub mod reg {
    pub const GPIP: u8 = 0;
    pub const AER: u8 = 1;
    pub const DDR: u8 = 2;
    pub const IERA: u8 = 3;
    pub const IERB: u8 = 4;
    pub const IPRA: u8 = 5;
    pub const IPRB: u8 = 6;
    pub const ISRA: u8 = 7;
    pub const ISRB: u8 = 8;
    pub const IMRA: u8 = 9;
    pub const IMRB: u8 = 10;
    pub const VR: u8 = 11;
    pub const TACR: u8 = 12;
    pub const TBCR: u8 = 13;
    pub const TCDCR: u8 = 14;
    pub const TADR: u8 = 15;
    pub const TBDR: u8 = 16;
    pub const TCDR: u8 = 17;
    pub const TDDR: u8 = 18;
    pub const SCR: u8 = 19;
    pub const UCR: u8 = 20;
    pub const RSR: u8 = 21;
    pub const TSR: u8 = 22;
    pub const UDR: u8 = 23;
}

/// Interrupt channel numbers (0-15), fixed by the silicon — table from
/// the MC68901 datasheet. Channels 8-15 live in the "A" registers
/// (IERA/IPRA/ISRA/IMRA), channels 0-7 in the "B" registers.
pub mod channel {
    pub const GPIP0: u8 = 0;
    pub const GPIP1: u8 = 1;
    pub const GPIP2: u8 = 2;
    pub const GPIP3: u8 = 3;
    pub const TIMER_D: u8 = 4;
    pub const TIMER_C: u8 = 5;
    pub const GPIP4: u8 = 6;
    pub const GPIP5: u8 = 7;
    pub const TIMER_B: u8 = 8;
    pub const TX_ERROR: u8 = 9;
    pub const TX_EMPTY: u8 = 10;
    pub const RX_ERROR: u8 = 11;
    pub const RX_FULL: u8 = 12;
    pub const TIMER_A: u8 = 13;
    pub const GPIP6: u8 = 14;
    pub const GPIP7: u8 = 15;
}

/// Table of prescaler divisors in "delay" mode (values 1-7 of the
/// prescale field of the timer control registers; index 0 = timer
/// stopped).
const PRESCALE: [u32; 8] = [0, 4, 10, 16, 50, 64, 100, 200];

/// MFP clock / CPU clock ratio for an ST/STE (CPU at 8 MHz, MFP at
/// 2.4576 MHz): 2,457,600 / 8,000,000 = 192/625, reduced to integers
/// for exact accumulation without floating-point drift (see `tick`).
const MFP_CLOCK_NUM: u32 = 192;
const MFP_CLOCK_DEN: u32 = 625;

#[derive(Debug, Clone)]
struct Timer {
    /// Raw control register (TACR/TBCR: bits 0-3; TCDCR splits its two
    /// timers across bits 6-4 and 2-0 — the field stored here is
    /// always already aligned to bits 3-0 by `Mfp::write`).
    control: u8,
    /// Data register (reload value; 0 is treated as 256).
    data: u8,
    /// Current countdown value (what a read of the data register
    /// returns while the timer is running).
    counter: u8,
    /// Fractional MFP cycle accumulator for delay mode (see
    /// `MFP_CLOCK_NUM`/`DEN`) — Timer A/B only, which also have an
    /// event-count mode driven by `pulse()` rather than `tick()`.
    prescale_acc: u32,
}

impl Timer {
    fn new() -> Self {
        Timer {
            control: 0,
            data: 0,
            counter: 0,
            prescale_acc: 0,
        }
    }

    fn prescale_divisor(&self) -> u32 {
        PRESCALE[(self.control & 0x7) as usize]
    }

    fn running(&self) -> bool {
        // 0 = stopped. 1-7 = delay mode (bits 0-2 = divisor). 8 (bit 3
        // alone, Timer A/B only) = event-count mode, decremented by
        // `pulse()` and not by the divisor — so `!= 0` covers both,
        // not just `& 0x7`.
        self.control != 0
    }

    /// "Event count" mode (Timer A/B only): bit 3 of the control
    /// register, decrements on `pulse()` rather than on the MFP
    /// clock.
    fn event_count_mode(&self) -> bool {
        self.control & 0x8 != 0
    }

    fn reload(&mut self) {
        self.counter = self.data;
        self.prescale_acc = 0;
    }

    /// Decrements the counter by one step; returns `true` if it just
    /// fired (and reloads from `data` in that case).
    ///
    /// Firing happens on the `N`-th decrement after a reload to
    /// `data = N` (not the `N+1`-th): the counter, read while running,
    /// is `N` right after the reload (never `N-1`), so it is already
    /// counting the first of the `N` intervals at that point — firing
    /// must happen on the decrement that would take it below 1, not
    /// on an extra decrement once it reaches 0. The previous version
    /// tested `counter == 0` on entry (instead of `<= 1`), which added
    /// one extra MFP step to the very first interval following each
    /// write to the control register (TACR/TBCR/TCDCR) — a discrepancy
    /// confirmed by the STe factory diagnostic cartridge (test "T0 MFP
    /// timer": Timer A/B/C/D at ÷200, reload 4, whose software wait
    /// window is too tight to tolerate this extra step).
    ///
    /// The test must be a strict equality `== 1`, not `<= 1`: if
    /// `counter` is already 0 on entry (counter never loaded — a real
    /// documented case, "data=0 means 256", or a counter left at its
    /// previous value because TACR/TBCR was written before
    /// TADR/TBDR, which the diagnostic cartridge actually does to arm
    /// DMA sound via XSINT), real silicon does NOT fire right away: it
    /// wraps 0→255 (8-bit register) and only fires after 256 more
    /// decrements — confirmed in Hatari (`mfp.c`, `MFP_TimerA_EventCount`:
    /// `if (TA_MAINCOUNTER == 1) { ... fire ... } else { TA_MAINCOUNTER--; }`,
    /// with the explicit comment about the expected 0→255 wraparound).
    /// With `<= 1`, a counter left at 0 fired instantly on the very
    /// first pulse — the cause of the diagnostic cartridge's DMA audio
    /// test appearing "skipped"/too fast.
    fn decrement(&mut self) -> bool {
        if self.counter == 1 {
            self.counter = self.data;
            true
        } else {
            self.counter = self.counter.wrapping_sub(1);
            false
        }
    }
}

/// Full state of an MC68901 chip.
#[derive(Debug, Clone)]
pub struct Mfp {
    gpip_in: u8,
    gpip_out: u8,
    aer: u8,
    ddr: u8,
    ier: u16,
    ipr: u16,
    isr: u16,
    imr: u16,
    vr: u8,
    ta: Timer,
    tb: Timer,
    tc: Timer,
    td: Timer,
    scr: u8,
    ucr: u8,
    rsr: u8,
    tsr: u8,
    udr: u8,
    rx_queue: std::collections::VecDeque<u8>,
    tx_queue: std::collections::VecDeque<u8>,
}

impl Default for Mfp {
    fn default() -> Self {
        Self::new()
    }
}

impl Mfp {
    /// State after a hardware reset: all registers at zero, except
    /// TSR bit 7 ("Buffer Empty") which starts at 1 — the transmit
    /// buffer really is empty on reset (no byte pending), so it is
    /// available to accept a new one. Confirmed necessary by the STe
    /// factory diagnostic cartridge, whose character output routine
    /// (RS232, used as the main text console) waits for this bit to
    /// be 1 before writing the very first byte into UDR — with TSR at
    /// 0 on reset, no write could ever take place (the only mechanism
    /// that sets this bit is precisely a write to UDR), permanently
    /// blocking the very first displayed character.
    pub fn new() -> Self {
        Mfp {
            gpip_in: 0,
            gpip_out: 0,
            aer: 0,
            ddr: 0,
            ier: 0,
            ipr: 0,
            isr: 0,
            imr: 0,
            vr: 0,
            ta: Timer::new(),
            tb: Timer::new(),
            tc: Timer::new(),
            td: Timer::new(),
            scr: 0,
            ucr: 0,
            rsr: 0,
            tsr: TSR_BUFFER_EMPTY,
            udr: 0,
            rx_queue: std::collections::VecDeque::new(),
            tx_queue: std::collections::VecDeque::new(),
        }
    }

    // --- Registers -------------------------------------------------------

    /// Reads the logical register `offset` (see [`reg`]).
    pub fn read(&mut self, offset: u8) -> u8 {
        match offset {
            reg::GPIP => (self.gpip_out & self.ddr) | (self.gpip_in & !self.ddr),
            reg::AER => self.aer,
            reg::DDR => self.ddr,
            reg::IERA => (self.ier >> 8) as u8,
            reg::IERB => self.ier as u8,
            reg::IPRA => (self.ipr >> 8) as u8,
            reg::IPRB => self.ipr as u8,
            reg::ISRA => (self.isr >> 8) as u8,
            reg::ISRB => self.isr as u8,
            reg::IMRA => (self.imr >> 8) as u8,
            reg::IMRB => self.imr as u8,
            reg::VR => self.vr,
            reg::TACR => self.ta.control,
            reg::TBCR => self.tb.control,
            reg::TCDCR => (self.tc.control << 4) | self.td.control,
            // Reading the data register of a RUNNING timer returns the
            // current countdown value, not the written reload value
            // (real MC68901 behavior, used by TOS/GEM to read a clock
            // without stopping it).
            reg::TADR => {
                if self.ta.running() {
                    self.ta.counter
                } else {
                    self.ta.data
                }
            }
            reg::TBDR => {
                if self.tb.running() {
                    self.tb.counter
                } else {
                    self.tb.data
                }
            }
            reg::TCDR => {
                if self.tc.running() {
                    self.tc.counter
                } else {
                    self.tc.data
                }
            }
            reg::TDDR => {
                if self.td.running() {
                    self.td.counter
                } else {
                    self.td.data
                }
            }
            reg::SCR => self.scr,
            reg::UCR => self.ucr,
            reg::RSR => self.rsr,
            reg::TSR => self.tsr,
            reg::UDR => {
                // Reading UDR returns the currently latched byte,
                // clears "buffer full", then latches the next pending
                // byte if there is one (for the following read).
                let value = self.udr;
                self.rsr &= !RSR_BUFFER_FULL;
                self.maybe_start_next_rx();
                value
            }
            _ => 0,
        }
    }

    /// Writes the logical register `offset` (see [`reg`]).
    pub fn write(&mut self, offset: u8, value: u8) {
        match offset {
            reg::GPIP => self.gpip_out = value,
            reg::AER => self.aer = value,
            reg::DDR => self.ddr = value,
            reg::IERA => self.ier = (self.ier & 0x00FF) | ((value as u16) << 8),
            reg::IERB => self.ier = (self.ier & 0xFF00) | value as u16,
            // IPR/ISR can only be cleared by a software write (writing
            // 1 to a bit has no effect; only writing 0 clears it —
            // documented MC68901 behavior, so that acknowledging one
            // source cannot accidentally arm another).
            reg::IPRA => self.ipr &= 0x00FF | ((value as u16) << 8),
            reg::IPRB => self.ipr &= 0xFF00 | value as u16,
            reg::ISRA => self.isr &= 0x00FF | ((value as u16) << 8),
            reg::ISRB => self.isr &= 0xFF00 | value as u16,
            reg::IMRA => self.imr = (self.imr & 0x00FF) | ((value as u16) << 8),
            reg::IMRB => self.imr = (self.imr & 0xFF00) | value as u16,
            reg::VR => {
                // Transition of bit S (bit 3) from 1 (SEI, "software
                // end-of-interrupt") to 0 (automatic EOI): the silicon
                // clears ISRA/ISRB IN BULK — confirmed against Hatari
                // (`MFP_VectorReg_WriteByte`, `mfp.c`). See the doc of
                // `Self::iack` for the exact meaning of bit S (INVERTED
                // relative to a superficial reading of the datasheet —
                // bit set = SEI, NOT "auto").
                if self.vr & 0x08 != 0 && value & 0x08 == 0 {
                    self.isr = 0;
                }
                self.vr = value;
            }
            reg::TACR => {
                self.ta.control = value & 0x0F;
                self.ta.reload();
            }
            reg::TBCR => {
                self.tb.control = value & 0x0F;
                self.tb.reload();
            }
            reg::TCDCR => {
                self.tc.control = (value >> 4) & 0x07;
                self.td.control = value & 0x07;
                self.tc.reload();
                self.td.reload();
            }
            reg::TADR => {
                self.ta.data = value;
                if !self.ta.running() {
                    self.ta.counter = value;
                }
            }
            reg::TBDR => {
                self.tb.data = value;
                if !self.tb.running() {
                    self.tb.counter = value;
                }
            }
            reg::TCDR => {
                self.tc.data = value;
                if !self.tc.running() {
                    self.tc.counter = value;
                }
            }
            reg::TDDR => {
                self.td.data = value;
                if !self.td.running() {
                    self.td.counter = value;
                }
            }
            reg::SCR => self.scr = value,
            reg::UCR => self.ucr = value,
            reg::RSR => self.rsr = value,
            // Bits 7 (Buffer Empty) and 6 (Underrun Error) are
            // read-only hardware status bits on the real MC68901, not
            // affected by a software write — only control bits 0-5
            // (Transmitter Enable, Break, End Of Transmission, Auto
            // Turnaround) are. A full register replacement here used
            // to clear Buffer Empty as early as the USART's standard
            // initialization sequence (writing TSR=0x01 to enable the
            // transmitter), permanently locking out every following
            // byte: no transmission could ever take place again,
            // since only a write to UDR resets Buffer Empty, and no
            // write to UDR can take place while Buffer Empty is zero.
            // Confirmed by the STe factory diagnostic cartridge, whose
            // entire text output goes through this RS232 port.
            reg::TSR => self.tsr = (self.tsr & 0xC0) | (value & 0x3F),
            reg::UDR => {
                self.udr = value;
                self.tx_queue.push_back(value);
                // Simplified model: transmission is instantaneous (no
                // simulated baud rate), TSR stays "buffer empty"
                // immediately.
                self.tsr |= TSR_BUFFER_EMPTY;
                self.request(channel::TX_EMPTY);
            }
            _ => {}
        }
    }

    // --- Timers ------------------------------------------------------------

    /// Advances the timers in "delay" mode by `cpu_cycles` CPU cycles
    /// (ST/STE clock at 8 MHz — see the `MFP_CLOCK_NUM/DEN` constant).
    /// Timers A/B in event-count mode are NOT advanced here: see
    /// [`Self::pulse_ta`]/[`Self::pulse_tb`].
    pub fn tick(&mut self, cpu_cycles: u32) {
        self.tick_delay_timer(cpu_cycles, TimerId::A);
        self.tick_delay_timer(cpu_cycles, TimerId::B);
        self.tick_delay_timer(cpu_cycles, TimerId::C);
        self.tick_delay_timer(cpu_cycles, TimerId::D);
    }

    fn tick_delay_timer(&mut self, cpu_cycles: u32, id: TimerId) {
        let (timer, chan) = self.timer_and_channel_mut(id);
        if !timer.running() {
            return;
        }
        if id == TimerId::A || id == TimerId::B {
            if timer.event_count_mode() {
                return; // advanced by pulse_ta/pulse_tb, not by the clock
            }
        }
        let div = timer.prescale_divisor();
        if div == 0 {
            return;
        }
        timer.prescale_acc += cpu_cycles * MFP_CLOCK_NUM;
        let mfp_ticks = timer.prescale_acc / (MFP_CLOCK_DEN * div);
        timer.prescale_acc %= MFP_CLOCK_DEN * div;
        let mut fired = false;
        for _ in 0..mfp_ticks {
            if timer.decrement() {
                fired = true;
            }
        }
        if fired {
            self.request(chan);
        }
    }

    fn timer_and_channel_mut(&mut self, id: TimerId) -> (&mut Timer, u8) {
        match id {
            TimerId::A => (&mut self.ta, channel::TIMER_A),
            TimerId::B => (&mut self.tb, channel::TIMER_B),
            TimerId::C => (&mut self.tc, channel::TIMER_C),
            TimerId::D => (&mut self.td, channel::TIMER_D),
        }
    }

    /// Signals an edge on the TAI input (Timer A in event-count mode).
    pub fn pulse_ta(&mut self) {
        if self.ta.running() && self.ta.event_count_mode() && self.ta.decrement() {
            self.request(channel::TIMER_A);
        }
    }

    /// Signals an edge on the TBI input (Timer B in event-count mode).
    pub fn pulse_tb(&mut self) {
        if self.tb.running() && self.tb.event_count_mode() && self.tb.decrement() {
            self.request(channel::TIMER_B);
        }
    }

    // --- GPIP / interrupts --------------------------------------------------

    /// Applies a logic level to a GPIP pin (0-7) configured as an
    /// input (`DDR` bit clear) and triggers an interrupt if the
    /// observed edge matches the direction programmed in `AER` (1 =
    /// rising edge).
    pub fn set_gpip_input(&mut self, pin: u8, level: bool) {
        debug_assert!(pin < 8);
        let mask = 1u8 << pin;
        if self.ddr & mask != 0 {
            return; // pin configured as output: no edge detection
        }
        let was = self.gpip_in & mask != 0;
        if was == level {
            return; // no edge
        }
        let rising_wanted = self.aer & mask != 0;
        if level == rising_wanted {
            // GPIP0-3 → channels 0-3; GPIP4-7 → channels 6,7,14,15
            // (datasheet table, cf. the `channel` module).
            let chan = match pin {
                0 => channel::GPIP0,
                1 => channel::GPIP1,
                2 => channel::GPIP2,
                3 => channel::GPIP3,
                4 => channel::GPIP4,
                5 => channel::GPIP5,
                6 => channel::GPIP6,
                7 => channel::GPIP7,
                _ => unreachable!(),
            };
            self.request(chan);
        }
        if level {
            self.gpip_in |= mask;
        } else {
            self.gpip_in &= !mask;
        }
    }

    /// Arms the "pending" bit of a channel (IPR) if it is enabled (IER).
    fn request(&mut self, chan: u8) {
        if std::env::var("RUST68_TRACE_MFP_REQUEST").is_ok() {
            eprintln!("[mfp] request chan={chan} ier={:#06x} armed={}", self.ier, self.ier & (1u16 << chan) != 0);
        }
        let mask = 1u16 << chan;
        if self.ier & mask != 0 {
            self.ipr |= mask;
        }
    }

    /// True if at least one eligible channel (see
    /// [`Self::highest_priority_pending`]) is requesting service — this
    /// is the signal the board must relay to `Bus::irq_level` (wired
    /// to IPL6 on real ST/STE).
    pub fn interrupt_requested(&self) -> bool {
        self.highest_priority_pending().is_some()
    }

    /// The highest-priority active channel (15 = highest priority,
    /// datasheet table) among those pending+enabled+unmasked, **provided
    /// that no STRICTLY higher-priority channel is already "in
    /// service"** (ISR) — nested priority resolution consistent with
    /// the real MC68901: an ISR of lower priority being serviced is
    /// preemptable by a new higher-priority channel, but blocks (for
    /// the duration of its own servicing) any channel of equal or
    /// lower priority, which remains pending without ever generating
    /// an IPL request as long as the higher channel remains
    /// in-service. Confirmed against Hatari (`mfp.c`,
    /// `MFP_InterruptRequest`/`MFP_CheckPendingInterrupts`: the mask of
    /// "higher-priority channels" applied to ISR before allowing a
    /// request) — our channel numbering (0-15, `channel` above)
    /// already encodes the full priority order in a single integer,
    /// so "channels of higher priority than channel N" is simply
    /// "index bits > N" on `isr`, with no need to distinguish A/B
    /// separately.
    fn highest_priority_pending(&self) -> Option<u8> {
        let mut higher_mask: u16 = 0; // no channel of priority > 15
        for chan in (0..=15u8).rev() {
            let bit = 1u16 << chan;
            if self.ipr & self.imr & bit != 0 && self.isr & higher_mask == 0 {
                return Some(chan);
            }
            higher_mask |= bit;
        }
        None
    }

    /// Interrupt acknowledge cycle (IACK): computes the vector for the
    /// highest-priority active channel, clears its pending bit, and
    /// arms (or not, see below) its "in service" bit depending on bit
    /// S (bit 3) of VR.
    ///
    /// **Meaning of bit S — counter-intuitive, take care**: bit set
    /// (1) = **SEI** ("software end-of-interrupt"): ISR is armed on
    /// IACK and stays set until explicitly cleared by software
    /// (writing 0 into ISRA/ISRB) — this is the mode used by the STe
    /// factory diagnostic cartridge (test "T0 MFP timer", VR=0x48, bit
    /// S set), whose shared interrupt handler (a single routine for
    /// all 4 Timer A/B/C/D vectors) determines which timer fired by
    /// reading precisely these ISR bits before clearing them itself.
    /// Bit clear (0) = **automatic** EOI: the silicon arms THEN clears
    /// ISR in the SAME IACK cycle, before the CPU has even fetched the
    /// vector — ISR is therefore never observable as set for that
    /// channel in this mode. Confirmed against Hatari (`mfp.c`,
    /// `MFP_ProcessIACK`: `if (VR & 0x08) ISR|=Bit; else ISR&=~Bit;` —
    /// bit set = SEI).
    ///
    /// An earlier reading of this code assumed the opposite (bit set =
    /// "auto", never armed) then, when fixing it following the
    /// cartridge test above, moved to "always armed regardless of the
    /// bit" instead of correctly conditioning on the TRUE meaning of
    /// the bit — an automatic-EOI channel (bit S=0) would then have
    /// its ISR remain stuck set forever (nothing ever clears it in
    /// that mode, unlike SEI mode where software handles it), which,
    /// combined with the nested priority resolution above (a set ISR
    /// blocks any channel of equal/lower priority), could block ALL
    /// lower-priority MFP channels indefinitely after the very first
    /// live automatic-EOI channel — reproduced in practice by sound
    /// that never stops and a mouse that no longer responds correctly
    /// as soon as such a channel fires (e.g. GEM, Desktop > Info).
    ///
    /// Returns the full vector: bits 7-4 = `VR[7:4]` (base programmed
    /// by software), bits 3-0 = channel number (0-15 — VR's own bit 3
    /// is NOT part of this: it is bit S above, a separate control, not
    /// a high-order bit of the vector — an easy confusion since its
    /// bit position coincides with the high bit of the channel field).
    pub fn iack(&mut self) -> u8 {
        let Some(chan) = self.highest_priority_pending() else {
            // Spurious interrupt (withdrawn before the IACK): standard
            // 68000 spurious vector (24), as VPA would produce without
            // an MFP.
            return 24;
        };
        let mask = 1u16 << chan;
        self.ipr &= !mask;
        if self.vr & 0x08 != 0 {
            self.isr |= mask; // SEI: stays set until cleared by software
        } else {
            self.isr &= !mask; // automatic EOI: armed THEN cleared in the same cycle
        }
        (self.vr & 0xF0) | chan
    }

    /// Manually acknowledges a channel in "software end-of-interrupt"
    /// mode (software write of 0 into ISR, to be done from the handler
    /// before the RTE — cf. [`Self::write`] on `reg::ISRA`/`ISRB`,
    /// exposed here as a direct helper for the board).
    pub fn end_of_interrupt(&mut self, chan: u8) {
        self.isr &= !(1u16 << chan);
    }

    // --- USART ---------------------------------------------------------------

    /// Injects a received byte (byte-level RS-232 simulation, cf. the
    /// module's limitations). If there is not already a reception in
    /// progress, immediately arms "buffer full" and the RX_FULL
    /// channel.
    pub fn push_rx_byte(&mut self, byte: u8) {
        self.rx_queue.push_back(byte);
        self.maybe_start_next_rx();
    }

    fn maybe_start_next_rx(&mut self) {
        if self.rsr & RSR_BUFFER_FULL == 0 {
            if let Some(b) = self.rx_queue.pop_front() {
                self.udr = b;
                self.rsr |= RSR_BUFFER_FULL;
                self.request(channel::RX_FULL);
            }
        }
    }

    /// Removes the next byte transmitted by the program (written into
    /// UDR with transmit direction), if there is one.
    pub fn take_tx_byte(&mut self) -> Option<u8> {
        self.tx_queue.pop_front()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TimerId {
    A,
    B,
    C,
    D,
}

const RSR_BUFFER_FULL: u8 = 1 << 7;
const TSR_BUFFER_EMPTY: u8 = 1 << 7;
