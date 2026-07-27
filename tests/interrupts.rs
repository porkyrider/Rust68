//! Tests unitaires du mécanisme d'interruption IPL (Cpu::take_interrupt).
//!
//! Aucune suite TomHarte ne couvre les interruptions : ce sont des
//! événements externes au flot d'opcodes, pas des instructions. Ce fichier
//! sert donc de filet de sécurité dédié, sur le même principe que
//! `tests/instructions.rs`.

use rust68::{Bus, Cpu, FlatBus, sr};

/// Bus de test : RAM plate + niveau IPL et vecteur IACK pilotables.
struct IrqBus {
    inner: FlatBus,
    level: u8,
    vector_override: Option<u8>,
}

impl IrqBus {
    fn new() -> Self {
        IrqBus {
            inner: FlatBus::new(),
            level: 0,
            vector_override: None,
        }
    }
}

impl Bus for IrqBus {
    fn read8(&mut self, addr: u32) -> u8 {
        self.inner.read8(addr)
    }
    fn write8(&mut self, addr: u32, value: u8) {
        self.inner.write8(addr, value)
    }
    fn irq_level(&self) -> u8 {
        self.level
    }
    fn irq_ack(&mut self, level: u8) -> u8 {
        self.vector_override.unwrap_or(24 + level)
    }
}

/// Construit un CPU + bus, place un vecteur de reset (SSP=0x2000, PC=0x0400)
/// et un NOP à 0x0400, comme `tests/instructions.rs::setup`.
fn setup(words: &[u16]) -> (Cpu, IrqBus) {
    let mut bus = IrqBus::new();
    bus.write32(0x0000, 0x0000_2000);
    bus.write32(0x0004, 0x0000_0400);
    let mut addr = 0x0400;
    for &w in words {
        bus.write16(addr, w);
        addr += 2;
    }
    let mut cpu = Cpu::new();
    cpu.reset(&mut bus);
    (cpu, bus)
}

#[test]
fn interruption_masquee_si_niveau_insuffisant() {
    let (mut cpu, mut bus) = setup(&[0x4E71]); // NOP
    cpu.sr &= !sr::IPL_MASK; // masque = 0, mais on demande niveau... 0 aussi
    bus.level = 0; // aucune demande
    let cycles = cpu.step(&mut bus).unwrap();
    assert_eq!(cycles, 4, "NOP normal, aucune interruption à niveau 0");
    assert_eq!(cpu.pc, 0x0402);

    // Masque à 3, demande de niveau 2 (inférieur ou égal) : pas pris.
    let (mut cpu, mut bus) = setup(&[0x4E71]);
    cpu.sr = (cpu.sr & !sr::IPL_MASK) | (3 << 8);
    bus.level = 2;
    let cycles = cpu.step(&mut bus).unwrap();
    assert_eq!(cycles, 4, "niveau <= masque courant : pas d'interruption");
    assert_eq!(cpu.pc, 0x0402);
}

#[test]
fn interruption_autovector_prise_et_masque_releve() {
    let (mut cpu, mut bus) = setup(&[0x4E71]); // NOP jamais exécuté
    cpu.sr &= !sr::IPL_MASK; // masque = 0
    bus.write32(0x0068, 0x0000_0800); // vecteur autovector niveau 2 = 24+2=26, adresse 26*4=0x68
    bus.level = 2;
    let pc_avant = cpu.pc;

    let cycles = cpu.step(&mut bus).unwrap();

    assert_eq!(cycles, 44);
    assert_eq!(cpu.pc, 0x0800, "saut au handler du vecteur 26");
    assert_eq!(
        (cpu.sr & sr::IPL_MASK) >> 8,
        2,
        "le masque IPL doit être relevé au niveau accepté"
    );
    // Frame standard 6 octets : SP-6 = SR sauvegardé, SP-2 (longword) = PC de retour.
    assert_eq!(bus.read32(cpu.sp().wrapping_add(2)), pc_avant);
}

#[test]
fn interruption_meme_niveau_que_masque_non_reprise() {
    let (mut cpu, mut bus) = setup(&[0x4E71]);
    cpu.sr = (cpu.sr & !sr::IPL_MASK) | (2 << 8); // masque déjà à 2
    bus.level = 2; // même niveau : ne doit pas re-déclencher
    let cycles = cpu.step(&mut bus).unwrap();
    assert_eq!(cycles, 4, "niveau == masque courant : pas d'interruption");
    assert_eq!(cpu.pc, 0x0402);
}

#[test]
fn interruption_niveau7_toujours_prise() {
    // reset() met le masque IPL à 7 par défaut : seul le niveau 7 (NMI)
    // doit encore pouvoir se déclencher.
    let (mut cpu, mut bus) = setup(&[0x4E71]);
    assert_eq!((cpu.sr & sr::IPL_MASK) >> 8, 7);
    bus.write32(0x007C, 0x0000_0900); // vecteur 31 (24+7), adresse 31*4=0x7C
    bus.level = 7;

    let cycles = cpu.step(&mut bus).unwrap();

    assert_eq!(cycles, 44);
    assert_eq!(cpu.pc, 0x0900);
    assert_eq!((cpu.sr & sr::IPL_MASK) >> 8, 7);
}

#[test]
fn interruption_vectorisee_utilise_le_vecteur_fournit_par_le_peripherique() {
    let (mut cpu, mut bus) = setup(&[0x4E71]);
    cpu.sr &= !sr::IPL_MASK;
    bus.vector_override = Some(0x40); // ex: MFP fournit son propre vecteur
    bus.write32(0x0100, 0x0000_0A00); // 0x40 * 4 = 0x100
    bus.level = 5;

    cpu.step(&mut bus).unwrap();

    assert_eq!(cpu.pc, 0x0A00);
}

#[test]
fn interruption_bascule_user_vers_superviseur() {
    let (mut cpu, mut bus) = setup(&[0x4E71]);
    cpu.usp = 0x3000;
    cpu.set_supervisor(false);
    assert!(!cpu.supervisor());
    assert_eq!(cpu.sp(), 0x3000);

    cpu.sr &= !sr::IPL_MASK;
    bus.write32(0x0068, 0x0000_0800); // vecteur 26
    bus.level = 2;

    cpu.step(&mut bus).unwrap();

    assert!(cpu.supervisor(), "l'interruption doit passer en superviseur");
    assert_eq!(cpu.usp, 0x3000, "l'USP utilisateur doit être préservé");
    assert_eq!(cpu.sp(), 0x2000 - 6, "le frame est empilé sur la pile superviseur");
}
