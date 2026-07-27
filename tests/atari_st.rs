//! Tests du board Atari ST (`rust68::systems::atari_st::AtariSt`).

use rust68::peripherals::mfp::{channel, reg};
use rust68::systems::atari_st::{AtariSt, DEFAULT_ROM_BASE, IO_BASE, MFP_BASE};
use rust68::{Bus, Cpu};

#[test]
fn ram_lecture_ecriture() {
    let mut st = AtariSt::new(0x1000, vec![]);
    st.write8(0x100, 0x42);
    assert_eq!(st.read8(0x100), 0x42);
}

#[test]
fn rom_lecture_seule() {
    let mut rom = vec![0u8; 0x100];
    rom[0] = 0xAB;
    let mut st = AtariSt::new(0x1000, rom);
    assert_eq!(st.read8(DEFAULT_ROM_BASE), 0xAB);
    st.write8(DEFAULT_ROM_BASE, 0xFF); // doit être ignoré
    assert_eq!(st.read8(DEFAULT_ROM_BASE), 0xAB);
}

#[test]
fn mfp_mappe_aux_adresses_impaires() {
    let mut st = AtariSt::new(0x1000, vec![]);
    // reg::VR est le registre logique 11 -> adresse MFP_BASE + 11*2.
    let addr = MFP_BASE + (reg::VR as u32) * 2;
    st.write8(addr, 0x48);
    assert_eq!(st.mfp.read(reg::VR), 0x48);
    assert_eq!(st.read8(addr), 0x48);
}

#[test]
fn trou_physique_declenche_bus_fault() {
    let mut st = AtariSt::new(0x1000, vec![]); // RAM minuscule : tout le reste est un trou
    let addr = 0x0000_2000; // au-delà de la RAM installée, avant IO_BASE
    let value = st.read8(addr);
    assert_eq!(value, 0xFF);
    assert_eq!(st.take_bus_fault(), Some((addr, false)));
    // Le fault doit être consommé (remis à None) par take_bus_fault.
    assert_eq!(st.take_bus_fault(), None);
}

#[test]
fn peripherique_non_emule_repond_neutre_sans_fault() {
    let mut st = AtariSt::new(0x1000, vec![]);
    let addr = IO_BASE + 0x800; // ex: zone PSG, pas modélisée
    assert_eq!(st.read8(addr), 0xFF);
    assert_eq!(st.take_bus_fault(), None, "chip select réel : pas de bus error");
    st.write8(addr, 0x12); // ne doit pas paniquer, simplement ignoré
}

#[test]
fn irq_level_cablee_sur_ipl6_via_le_mfp() {
    let mut st = AtariSt::new(0x1000, vec![]);
    assert_eq!(st.irq_level(), 0, "aucune interruption MFP en attente");

    st.mfp.write(reg::DDR, 0x00);
    st.mfp.write(reg::AER, 0x01);
    st.mfp.write(reg::IERB, 1 << channel::GPIP0);
    st.mfp.write(reg::IMRB, 1 << channel::GPIP0);
    st.mfp.set_gpip_input(0, true);

    assert_eq!(st.irq_level(), 6, "MFP câblé sur IPL6");
    let vector = st.irq_ack(6);
    assert_eq!(vector & 0x07, channel::GPIP0);
    assert_eq!(st.irq_level(), 0, "IACK a effacé le pending MFP");
}

#[test]
fn reset_bus_reinitialise_le_mfp() {
    let mut st = AtariSt::new(0x1000, vec![]);
    st.mfp.write(reg::IERA, 0xFF);
    st.reset_bus();
    assert_eq!(st.mfp.read(reg::IERA), 0, "reset_bus doit réinitialiser le MFP");
}

/// Test d'intégration bout-en-bout : un CPU réel prend une interruption
/// générée par le MFP via le board, à travers tout le mécanisme
/// Cpu::step -> Bus::irq_level -> Bus::irq_ack -> Mfp::iack.
#[test]
fn cpu_prend_une_interruption_mfp_bout_en_bout() {
    let mut st = AtariSt::new(0x1_0000, vec![]);
    // Vecteur de reset : SSP=0x2000, PC=0x0400.
    st.write32(0x0000, 0x0000_2000);
    st.write32(0x0004, 0x0000_0400);
    st.write16(0x0400, 0x4E71); // NOP, jamais exécuté si l'IRQ est prise avant

    let mut cpu = Cpu::new();
    cpu.reset(&mut st);
    cpu.sr &= !rust68::sr::IPL_MASK; // masque IPL = 0 : rien ne bloque l'interruption

    st.mfp.write(reg::DDR, 0x00);
    st.mfp.write(reg::AER, 0x01);
    st.mfp.write(reg::IERB, 1 << channel::GPIP0);
    st.mfp.write(reg::IMRB, 1 << channel::GPIP0);
    st.mfp.write(reg::VR, 0x40); // vecteur de base 0x40, canal 0 -> vecteur 0x40
    st.write32(0x0040 * 4, 0x0000_0800); // handler à 0x0800
    st.mfp.set_gpip_input(0, true);

    let pc_avant = cpu.pc;
    let cycles = cpu.step(&mut st).unwrap();

    assert_eq!(cycles, 44);
    assert_eq!(cpu.pc, 0x0800, "le CPU doit avoir sauté au handler MFP");
    assert_eq!((cpu.sr & rust68::sr::IPL_MASK) >> 8, 6, "masque IPL relevé à 6");
    assert_eq!(
        st.read32(cpu.sp().wrapping_add(2)),
        pc_avant,
        "le PC de retour empilé doit être celui d'avant l'interruption"
    );
}
