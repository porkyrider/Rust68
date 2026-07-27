//! Board Atari ST : mapping mémoire réel + câblage MFP → IPL6.
//!
//! Implémente [`crate::Bus`] pour un ST/STE minimal : RAM installée à
//! `0x000000`, ROM TOS à `0xFC0000`, MFP 68901 aux adresses impaires
//! `0xFFFA01`-`0xFFFA2F`. Le "trou" physique entre le haut de la RAM
//! installée et le début de la zone d'E/S (`0xFF8000`) déclenche un bus
//! error via [`crate::Bus::take_bus_fault`] — c'est le mécanisme que de
//! nombreux programmes/démos utilisent pour détecter la RAM installée.
//!
//! ## Limitations connues (v1)
//! - Seuls RAM/ROM/MFP sont réellement mappés. Le reste de la zone d'E/S
//!   (`0xFF8000`-`0xFFFFFF` : ACIA, PSG/YM2149, FDC/DMA, Shifter…) répond
//!   `0xFF` en lecture et ignore les écritures — chip select réel mais
//!   périphérique pas encore émulé, plutôt qu'un bus error qui casserait
//!   tout polling de statut par le logiciel.
//! - Pas de miroir ROM à `0xE00000` (modèles 130ST très anciens).
//! - Pas de modèle de contention DRAM/vidéo (`is_contended` reste à
//!   `false` : nécessite le Shifter, pas encore implémenté).
//! - Les adresses paires adjacentes à un registre MFP (ex: `0xFFFA00`,
//!   normalement flottantes sur un vrai bus 8 bits) retombent dans le
//!   stub d'E/S générique plutôt que de modéliser précisément le
//!   comportement de décodage UDS/LDS.

use crate::peripherals::mfp::Mfp;
use crate::{ADDR_MASK, Bus};

/// Adresse du premier registre MFP (`GPIP`), sur ST/STE réel.
pub const MFP_BASE: u32 = 0xFFFA01;
/// Nombre de registres logiques du MFP (voir `peripherals::mfp::reg`).
const MFP_REG_COUNT: u32 = 24;
/// Adresse du dernier registre MFP (`UDR`).
pub const MFP_END: u32 = MFP_BASE + (MFP_REG_COUNT - 1) * 2;

/// Début de la zone d'E/S général (ACIA, PSG, FDC, Shifter…) sur ST/STE.
pub const IO_BASE: u32 = 0xFF8000;
/// Fin de l'espace d'adressage (24 bits).
pub const IO_END: u32 = 0x00FF_FFFF;

/// Adresse de base usuelle de la ROM TOS (192 Ko, TOS 1.x/2.x).
pub const DEFAULT_ROM_BASE: u32 = 0xFC0000;

/// Board Atari ST minimal : RAM + ROM + MFP 68901.
pub struct AtariSt {
    ram: Vec<u8>,
    rom: Vec<u8>,
    rom_base: u32,
    /// Puce MFP 68901, câblée sur IPL6 (voir `Bus::irq_level`). Champ
    /// public : l'appelant a besoin d'y injecter des événements externes
    /// (`set_gpip_input`, `push_rx_byte`…) et de faire progresser ses
    /// timers via `tick()`.
    pub mfp: Mfp,
    bus_fault: Option<(u32, bool)>,
}

impl AtariSt {
    /// Crée un board avec `ram_size` octets de RAM installée à `0x000000`
    /// et `rom` (typiquement un dump TOS) mappée à `DEFAULT_ROM_BASE`.
    pub fn new(ram_size: usize, rom: Vec<u8>) -> Self {
        AtariSt {
            ram: vec![0; ram_size],
            rom,
            rom_base: DEFAULT_ROM_BASE,
            mfp: Mfp::new(),
            bus_fault: None,
        }
    }

    fn mfp_offset(addr: u32) -> Option<u8> {
        if addr >= MFP_BASE && addr <= MFP_END && (addr - MFP_BASE) % 2 == 0 {
            Some(((addr - MFP_BASE) / 2) as u8)
        } else {
            None
        }
    }

    fn in_rom(&self, addr: u32) -> bool {
        addr >= self.rom_base && addr - self.rom_base < self.rom.len() as u32
    }
}

impl Bus for AtariSt {
    fn read8(&mut self, addr: u32) -> u8 {
        let addr = addr & ADDR_MASK;
        if (addr as usize) < self.ram.len() {
            return self.ram[addr as usize];
        }
        if let Some(off) = Self::mfp_offset(addr) {
            return self.mfp.read(off);
        }
        if self.in_rom(addr) {
            return self.rom[(addr - self.rom_base) as usize];
        }
        if (IO_BASE..=IO_END).contains(&addr) {
            return 0xFF;
        }
        self.bus_fault = Some((addr, false));
        0xFF
    }

    fn write8(&mut self, addr: u32, value: u8) {
        let addr = addr & ADDR_MASK;
        if (addr as usize) < self.ram.len() {
            self.ram[addr as usize] = value;
            return;
        }
        if let Some(off) = Self::mfp_offset(addr) {
            self.mfp.write(off, value);
            return;
        }
        if self.in_rom(addr) {
            return; // ROM : écriture ignorée (lecture seule sur silicium réel)
        }
        if (IO_BASE..=IO_END).contains(&addr) {
            return; // périphérique non émulé : écriture ignorée
        }
        self.bus_fault = Some((addr, true));
    }

    fn reset_bus(&mut self) {
        // L'instruction RESET génère /RESET vers les périphériques externes.
        self.mfp = Mfp::new();
    }

    fn take_bus_fault(&mut self) -> Option<(u32, bool)> {
        self.bus_fault.take()
    }

    fn irq_level(&self) -> u8 {
        // Câblage matériel ST/STE : sortie IRQ du MFP sur IPL6 du CPU.
        if self.mfp.interrupt_requested() { 6 } else { 0 }
    }

    fn irq_ack(&mut self, level: u8) -> u8 {
        if level == 6 {
            self.mfp.iack()
        } else {
            24 + level
        }
    }
}
