//! # Rust68
//!
//! Motorola 68000 series (68k) emulator written in Rust.
//!
//! The eventual goal is to cover the whole 68000 family and its variants.
//! This first version targets the original **MC68000**, the one found in
//! the Atari ST and the Amiga.
//!
//! ## Architecture
//!
//! - [`Cpu`] models the processor state (registers, SR/CCR, PC).
//! - The CPU holds no memory: all accesses go through a [`Bus`] that the
//!   caller implements for its system.
//! - [`Cpu::reset`] boots the CPU from the reset vector, [`Cpu::step`]
//!   executes one instruction.
//!
//! ## Example
//!
//! ```
//! use rust68::{Cpu, Bus, FlatBus};
//!
//! let mut bus = FlatBus::new();
//! // Reset vector: SSP = 0x1000, PC = 0x0400.
//! bus.write32(0x0000, 0x0000_1000);
//! bus.write32(0x0004, 0x0000_0400);
//! // A NOP at the starting address.
//! bus.write16(0x0400, 0x4E71);
//!
//! let mut cpu = Cpu::new();
//! cpu.reset(&mut bus);
//! assert_eq!(cpu.pc, 0x0400);
//!
//! cpu.step(&mut bus).unwrap();
//! assert_eq!(cpu.pc, 0x0402);
//! ```

mod addressing;
mod bus;
mod cpu;
mod execute;
pub mod peripherals;
pub mod systems;
pub mod trace;

pub use addressing::{Operand, Size};
pub use bus::{Bus, TimedBus, TraceSink, TracingBus};
pub use cpu::{ADDR_MASK, Cpu, CpuType, ccr, sr};
pub use execute::StepError;

/// Flat 16 MB memory bus (the 68000's entire address space).
///
/// Useful for testing and prototyping: every address is plain RAM. Real
/// systems (Atari, Amiga) will provide their own [`Bus`].
pub struct FlatBus {
    ram: Vec<u8>,
}

impl FlatBus {
    /// Creates a 16 MB bus initialized to zero.
    pub fn new() -> Self {
        FlatBus {
            ram: vec![0; 0x0100_0000],
        }
    }
}

impl Default for FlatBus {
    fn default() -> Self {
        Self::new()
    }
}

impl Bus for FlatBus {
    fn read8(&mut self, addr: u32) -> u8 {
        self.ram[(addr & ADDR_MASK) as usize]
    }

    fn write8(&mut self, addr: u32, value: u8) {
        self.ram[(addr & ADDR_MASK) as usize] = value;
    }
}
