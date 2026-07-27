//! # Rust68
//!
//! Émulateur de la série Motorola 68000 (68k) en Rust.
//!
//! L'objectif à terme est de couvrir toute la famille 68000 et ses variantes.
//! Cette première version cible le **MC68000** d'origine, celui de l'Atari ST
//! et de l'Amiga.
//!
//! ## Architecture
//!
//! - [`Cpu`] modélise l'état du processeur (registres, SR/CCR, PC).
//! - Le CPU ne contient pas de mémoire : tous les accès passent par un [`Bus`]
//!   que l'appelant implémente pour son système.
//! - [`Cpu::reset`] amorce le CPU depuis le vecteur de reset, [`Cpu::step`]
//!   exécute une instruction.
//!
//! ## Exemple
//!
//! ```
//! use rust68::{Cpu, Bus, FlatBus};
//!
//! let mut bus = FlatBus::new();
//! // Vecteur de reset : SSP = 0x1000, PC = 0x0400.
//! bus.write32(0x0000, 0x0000_1000);
//! bus.write32(0x0004, 0x0000_0400);
//! // Un NOP à l'adresse de départ.
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

pub use addressing::{Operand, Size};
pub use bus::{Bus, TimedBus};
pub use cpu::{ADDR_MASK, Cpu, ccr, sr};
pub use execute::StepError;

/// Bus mémoire plat de 16 Mo (tout l'espace d'adressage du 68000).
///
/// Utile pour les tests et le prototypage : toute adresse est de la RAM
/// ordinaire. Les systèmes réels (Atari, Amiga) fourniront leur propre [`Bus`].
pub struct FlatBus {
    ram: Vec<u8>,
}

impl FlatBus {
    /// Crée un bus de 16 Mo initialisé à zéro.
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
