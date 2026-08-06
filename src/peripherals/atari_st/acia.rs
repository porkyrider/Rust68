//! Motorola MC6850 ACIA (Asynchronous Communications Interface Adapter).
//!
//! The Atari ST has **two** identical ACIA chips: one for the keyboard
//! (connected to the keyboard's HD6301 controller via a serial link), one
//! for the MIDI port (in/out). Both share the same chip model —
//! [`crate::systems::atari_st::AtariSt`] instantiates two separate
//! [`Acia`] instances and wires their IRQ outputs (wired OR) to `GPIP4` of
//! the MFP (see `peripherals::mfp::channel::GPIP4`, real ST/STE wiring).
//!
//! ## Known limitations (v1)
//! - "Byte-level" model like `peripherals::mfp`: no start/stop/parity bit
//!   or real baud rate — only the registers and status flags
//!   (RDRF/TDRE/OVRN/FE) are modeled faithfully.
//! - `DCD`/`CTS` (carrier detect / clear-to-send) are wired always active
//!   (no error, always ready to transmit): no external handshake line
//!   simulation.
//! - No parity bit simulation (`PE` always stays 0).

/// Offsets of the two logical registers (÷2 relative to the real spacing
/// on the bus — see `systems::atari_st` for the address mapping).
pub mod reg {
    /// Write: control register. Read: status register.
    pub const CONTROL_STATUS: u8 = 0;
    /// Write: transmit register. Read: receive register.
    pub const DATA: u8 = 1;
}

/// State of an MC6850 ACIA chip.
#[derive(Debug, Clone)]
pub struct Acia {
    control: u8,
    rdrf: bool,
    tdre: bool,
    overrun: bool,
    framing_error: bool,
    rx_data: u8,
    tx_queue: std::collections::VecDeque<u8>,
}

impl Default for Acia {
    fn default() -> Self {
        Self::new()
    }
}

impl Acia {
    /// State after a hardware reset: TDRE active (transmitter ready),
    /// everything else zero/false — documented MC6850 behavior.
    pub fn new() -> Self {
        Acia {
            control: 0,
            rdrf: false,
            tdre: true,
            overrun: false,
            framing_error: false,
            rx_data: 0,
            tx_queue: std::collections::VecDeque::new(),
        }
    }

    fn receive_interrupt_enabled(&self) -> bool {
        self.control & 0x80 != 0
    }

    /// Bits 6-5 of the control register: `01` = RTS low + transmit
    /// interrupt enabled (the 3 other combinations disable TIE).
    fn transmit_interrupt_enabled(&self) -> bool {
        (self.control >> 5) & 0x03 == 0b01
    }

    /// Reads the logical register `offset` (see [`reg`]).
    pub fn read(&mut self, offset: u8) -> u8 {
        match offset {
            reg::CONTROL_STATUS => self.status_byte(),
            reg::DATA => {
                let value = self.rx_data;
                // Reading the data clears RDRF and OVRN (real MC6850
                // behavior: the read acknowledges both at once).
                self.rdrf = false;
                self.overrun = false;
                value
            }
            _ => 0,
        }
    }

    fn status_byte(&self) -> u8 {
        let irq = (self.rdrf && self.receive_interrupt_enabled())
            || (self.tdre && self.transmit_interrupt_enabled());
        (self.rdrf as u8)
            | ((self.tdre as u8) << 1)
            // bit 2 (DCD) and bit 3 (CTS): always 0 (no external handshake simulated)
            | ((self.framing_error as u8) << 4)
            | ((self.overrun as u8) << 5)
            // bit 6 (PE): always 0 (no parity simulation)
            | ((irq as u8) << 7)
    }

    /// Writes the logical register `offset` (see [`reg`]).
    pub fn write(&mut self, offset: u8, value: u8) {
        match offset {
            reg::CONTROL_STATUS => {
                self.control = value;
                // Bits 0-1 = 11: Master Reset (independent of the rest of
                // the control register, which is still updated).
                if value & 0x03 == 0x03 {
                    self.rdrf = false;
                    self.tdre = true;
                    self.overrun = false;
                    self.framing_error = false;
                }
            }
            reg::DATA => {
                self.tx_queue.push_back(value);
                // Instant "byte-level" model (see limitations): TDRE
                // remains/immediately becomes active again.
                self.tdre = true;
            }
            _ => {}
        }
    }

    /// Injects a received byte (byte-level simulation, see limitations).
    /// If a previous byte hasn't been read yet (RDRF already active), the
    /// new byte is **lost** and `OVRN` gets set — the MC6850 has no
    /// receive FIFO, just a single register.
    pub fn push_rx_byte(&mut self, byte: u8) {
        if self.rdrf {
            self.overrun = true;
            return;
        }
        self.rx_data = byte;
        self.rdrf = true;
    }

    /// Removes the next byte transmitted by the program, if there is one.
    pub fn take_tx_byte(&mut self) -> Option<u8> {
        self.tx_queue.pop_front()
    }

    /// True if this chip is requesting an interrupt (to be OR-combined
    /// with the other ACIA by the board — both share the same MFP GPIP
    /// pin on real ST/STE hardware).
    pub fn irq_requested(&self) -> bool {
        (self.rdrf && self.receive_interrupt_enabled())
            || (self.tdre && self.transmit_interrupt_enabled())
    }
}
