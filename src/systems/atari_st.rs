//! Board Atari ST : mapping mémoire réel + câblage MFP/GLUE → IPL.
//!
//! Implémente [`crate::Bus`] pour un ST/STE minimal : RAM installée à
//! `0x000000`, ROM TOS à `0xFC0000`, MFP 68901 aux adresses impaires
//! `0xFFFA01`-`0xFFFA2F`. Le "trou" physique entre le haut de la RAM
//! installée et le début de la zone d'E/S (`0xFF8000`) déclenche un bus
//! error via [`crate::Bus::take_bus_fault`] — c'est le mécanisme que de
//! nombreux programmes/démos utilisent pour détecter la RAM installée.
//!
//! Câblage d'interruption réel ST/STE, par priorité décroissante :
//! MFP → IPL6, VBL (GLUE) → IPL4, HBL (GLUE) → IPL2. Les deux ACIA
//! (clavier + MIDI) ne génèrent pas d'IPL directement : leurs sorties IRQ
//! sont OR câblées sur `GPIP4` du MFP (câblage réel ST/STE).
//!
//! ## Limitations connues (v1)
//! - RAM/ROM/MFP/ACIA×2/YM2149 sont réellement mappés. Le reste de la zone
//!   d'E/S (`0xFF8000`-`0xFFFFFF` : FDC/DMA, Shifter…) répond `0xFF` en
//!   lecture et ignore les écritures — chip select réel mais périphérique
//!   pas encore émulé, plutôt qu'un bus error qui casserait tout polling
//!   de statut par le logiciel.
//! - Pas de miroir ROM à `0xE00000` (modèles 130ST très anciens).
//! - Pas de modèle de contention DRAM/vidéo (`is_contended` reste à
//!   `false` : nécessite le Shifter, pas encore implémenté).
//! - Les adresses paires adjacentes à un registre MFP (ex: `0xFFFA00`,
//!   normalement flottantes sur un vrai bus 8 bits) retombent dans le
//!   stub d'E/S générique plutôt que de modéliser précisément le
//!   comportement de décodage UDS/LDS.
//! - `AtariSt::tick` doit être appelé explicitement par l'appelant après
//!   chaque `Cpu::step` (ce crate ne fait pas progresser les
//!   périphériques tout seul) — voir l'exemple sur `tick`.

use crate::peripherals::acia::{self, Acia};
use crate::peripherals::glue::{Glue, VideoMode};
use crate::peripherals::mfp::Mfp;
use crate::peripherals::ym2149::{self, Ym2149};
use crate::{ADDR_MASK, Bus};

/// Adresse du premier registre MFP (`GPIP`), sur ST/STE réel.
pub const MFP_BASE: u32 = 0xFFFA01;
/// Nombre de registres logiques du MFP (voir `peripherals::mfp::reg`).
const MFP_REG_COUNT: u32 = 24;
/// Adresse du dernier registre MFP (`UDR`).
pub const MFP_END: u32 = MFP_BASE + (MFP_REG_COUNT - 1) * 2;

/// ACIA clavier : registre de contrôle/statut, sur ST/STE réel.
pub const ACIA_KEYBOARD_CONTROL: u32 = 0xFFFC00;
/// ACIA clavier : registre de données.
pub const ACIA_KEYBOARD_DATA: u32 = 0xFFFC02;
/// ACIA MIDI : registre de contrôle/statut.
pub const ACIA_MIDI_CONTROL: u32 = 0xFFFC04;
/// ACIA MIDI : registre de données.
pub const ACIA_MIDI_DATA: u32 = 0xFFFC06;

/// YM2149 : registre sélecteur (écriture = choix du registre, lecture =
/// registre actuellement sélectionné), sur ST/STE réel.
pub const YM2149_SELECT: u32 = 0xFF8800;
/// YM2149 : registre de données du registre actuellement sélectionné.
pub const YM2149_DATA: u32 = 0xFF8802;

/// Début de la zone d'E/S général (ACIA, PSG, FDC, Shifter…) sur ST/STE.
pub const IO_BASE: u32 = 0xFF8000;
/// Fin de l'espace d'adressage (24 bits).
pub const IO_END: u32 = 0x00FF_FFFF;

/// Adresse de base usuelle de la ROM TOS (192 Ko, TOS 1.x/2.x).
pub const DEFAULT_ROM_BASE: u32 = 0xFC0000;

/// Board Atari ST minimal : RAM + ROM + MFP 68901 + GLUE (HBL/VBL).
pub struct AtariSt {
    ram: Vec<u8>,
    rom: Vec<u8>,
    rom_base: u32,
    /// Puce MFP 68901, câblée sur IPL6 (voir `Bus::irq_level`). Champ
    /// public : l'appelant a besoin d'y injecter des événements externes
    /// (`set_gpip_input`, `push_rx_byte`…). Faire progresser ses timers
    /// passe par [`Self::tick`], pas directement par `Mfp::tick`.
    pub mfp: Mfp,
    /// Puce GLUE (timing HBL/VBL), câblée sur IPL2/IPL4. Champ public :
    /// utile en lecture pour synchroniser un rendu vidéo externe sur
    /// `current_line()`/`frame_count()`.
    pub glue: Glue,
    /// ACIA clavier. Champ public : injecter les octets reçus du
    /// contrôleur clavier via `push_rx_byte`, lire les commandes envoyées
    /// par le programme via `take_tx_byte`.
    pub acia_keyboard: Acia,
    /// ACIA MIDI (in/out).
    pub acia_midi: Acia,
    /// PSG YM2149 (son + ports d'E/S). Champ public : lire les niveaux de
    /// sortie audio via `channel_level`, injecter les entrées des ports
    /// A/B (joystick/souris/lecteur, câblage non interprété par ce board).
    pub ym2149: Ym2149,
    bus_fault: Option<(u32, bool)>,
}

impl AtariSt {
    /// Crée un board avec `ram_size` octets de RAM installée à `0x000000`,
    /// `rom` (typiquement un dump TOS) mappée à `DEFAULT_ROM_BASE`, et le
    /// GLUE cadencé en PAL 50 Hz (le cas le plus courant — voir
    /// [`VideoMode`] pour du NTSC).
    pub fn new(ram_size: usize, rom: Vec<u8>) -> Self {
        AtariSt {
            ram: vec![0; ram_size],
            rom,
            rom_base: DEFAULT_ROM_BASE,
            mfp: Mfp::new(),
            glue: Glue::new(VideoMode::Pal50),
            acia_keyboard: Acia::new(),
            acia_midi: Acia::new(),
            ym2149: Ym2149::new(),
            bus_fault: None,
        }
    }

    /// Fait progresser les périphériques (MFP + GLUE + YM2149) de
    /// `cpu_cycles` cycles CPU, et relaie l'IRQ combinée des deux ACIA sur
    /// `GPIP4` du MFP (OR câblé, câblage réel ST/STE). À appeler par
    /// l'appelant après chaque `Cpu::step` :
    ///
    /// ```
    /// use rust68::{Cpu, systems::atari_st::AtariSt};
    ///
    /// let mut st = AtariSt::new(0x1000, vec![]);
    /// let mut cpu = Cpu::new();
    /// cpu.reset(&mut st);
    /// let cycles = cpu.step(&mut st).unwrap();
    /// st.tick(cycles);
    /// ```
    pub fn tick(&mut self, cpu_cycles: u32) {
        self.mfp.tick(cpu_cycles);
        self.glue.tick(cpu_cycles);
        self.ym2149.tick(cpu_cycles);
        let acia_irq = self.acia_keyboard.irq_requested() || self.acia_midi.irq_requested();
        self.mfp.set_gpip_input(4, acia_irq);
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
        match addr {
            ACIA_KEYBOARD_CONTROL => return self.acia_keyboard.read(acia::reg::CONTROL_STATUS),
            ACIA_KEYBOARD_DATA => return self.acia_keyboard.read(acia::reg::DATA),
            ACIA_MIDI_CONTROL => return self.acia_midi.read(acia::reg::CONTROL_STATUS),
            ACIA_MIDI_DATA => return self.acia_midi.read(acia::reg::DATA),
            YM2149_SELECT => return self.ym2149.read(ym2149::bus_offset::SELECT),
            YM2149_DATA => return self.ym2149.read(ym2149::bus_offset::DATA),
            _ => {}
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
        match addr {
            ACIA_KEYBOARD_CONTROL => {
                self.acia_keyboard.write(acia::reg::CONTROL_STATUS, value);
                return;
            }
            ACIA_KEYBOARD_DATA => {
                self.acia_keyboard.write(acia::reg::DATA, value);
                return;
            }
            ACIA_MIDI_CONTROL => {
                self.acia_midi.write(acia::reg::CONTROL_STATUS, value);
                return;
            }
            ACIA_MIDI_DATA => {
                self.acia_midi.write(acia::reg::DATA, value);
                return;
            }
            YM2149_SELECT => {
                self.ym2149.write(ym2149::bus_offset::SELECT, value);
                return;
            }
            YM2149_DATA => {
                self.ym2149.write(ym2149::bus_offset::DATA, value);
                return;
            }
            _ => {}
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
        // Le GLUE n'est PAS réinitialisé : sur silicium réel, le timing
        // vidéo continue de tourner indépendamment d'un /RESET CPU (le
        // moniteur reste synchronisé).
        self.mfp = Mfp::new();
        self.acia_keyboard = Acia::new();
        self.acia_midi = Acia::new();
        self.ym2149 = Ym2149::new();
    }

    fn take_bus_fault(&mut self) -> Option<(u32, bool)> {
        self.bus_fault.take()
    }

    fn irq_level(&self) -> u8 {
        // Câblage matériel ST/STE, par priorité décroissante :
        // MFP (IPL6) > VBL (IPL4) > HBL (IPL2).
        if self.mfp.interrupt_requested() {
            6
        } else if self.glue.vbl_pending() {
            4
        } else if self.glue.hbl_pending() {
            2
        } else {
            0
        }
    }

    fn irq_ack(&mut self, level: u8) -> u8 {
        match level {
            6 => self.mfp.iack(),
            4 => {
                self.glue.ack_vbl();
                24 + 4 // autovecteur niveau 4
            }
            2 => {
                self.glue.ack_hbl();
                24 + 2 // autovecteur niveau 2
            }
            _ => 24 + level,
        }
    }
}
