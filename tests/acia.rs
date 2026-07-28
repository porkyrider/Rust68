//! Tests unitaires du MC6850 ACIA (`rust68::peripherals::acia`).

use rust68::peripherals::acia::{Acia, reg};

#[test]
fn etat_initial_emetteur_pret_recepteur_vide() {
    let mut acia = Acia::new();
    let status = acia.read(reg::CONTROL_STATUS);
    assert_eq!(status & 0x01, 0, "RDRF doit être à 0 au reset");
    assert_eq!(status & 0x02, 0x02, "TDRE doit être à 1 au reset (émetteur prêt)");
}

#[test]
fn reception_arme_rdrf_et_lecture_l_efface() {
    let mut acia = Acia::new();
    acia.push_rx_byte(0x41);
    assert_eq!(acia.read(reg::CONTROL_STATUS) & 0x01, 0x01, "RDRF armé");
    assert_eq!(acia.read(reg::DATA), 0x41);
    assert_eq!(acia.read(reg::CONTROL_STATUS) & 0x01, 0, "RDRF effacé par la lecture");
}

#[test]
fn overrun_si_octet_non_lu_avant_le_suivant() {
    let mut acia = Acia::new();
    acia.push_rx_byte(0x41);
    acia.push_rx_byte(0x42); // non lu : doit être perdu, pas mis en file
    let status = acia.read(reg::CONTROL_STATUS);
    assert_eq!(status & 0x20, 0x20, "OVRN doit s'armer");
    assert_eq!(acia.read(reg::DATA), 0x41, "le premier octet reste disponible");

    // La lecture efface OVRN en même temps que RDRF.
    let status_apres = acia.read(reg::CONTROL_STATUS);
    assert_eq!(status_apres & 0x20, 0, "OVRN effacé par la lecture");
}

#[test]
fn transmission_au_niveau_octet() {
    let mut acia = Acia::new();
    acia.write(reg::DATA, 0x55);
    assert_eq!(acia.read(reg::CONTROL_STATUS) & 0x02, 0x02, "TDRE reste actif");
    assert_eq!(acia.take_tx_byte(), Some(0x55));
    assert_eq!(acia.take_tx_byte(), None);
}

#[test]
fn interruption_reception_soumise_a_rie() {
    let mut acia = Acia::new();
    acia.push_rx_byte(0x10);
    assert!(!acia.irq_requested(), "RIE non activé par défaut : pas d'IRQ");

    acia.write(reg::CONTROL_STATUS, 0x80); // RIE seul (bit7), reste à 0
    acia.push_rx_byte(0x10);
    assert!(acia.irq_requested(), "RIE actif + RDRF : IRQ demandée");
}

#[test]
fn interruption_emission_soumise_a_tie() {
    let mut acia = Acia::new();
    // TDRE est actif dès le reset, mais TIE (bits 6-5 = 01) ne l'est pas.
    assert!(!acia.irq_requested());

    acia.write(reg::CONTROL_STATUS, 0b0010_0000); // bits6-5 = 01 : TIE actif
    assert!(acia.irq_requested(), "TIE actif + TDRE : IRQ demandée");

    acia.write(reg::CONTROL_STATUS, 0b0100_0000); // bits6-5 = 10 : TIE inactif
    assert!(!acia.irq_requested());
}

#[test]
fn master_reset_efface_les_flags_de_statut() {
    let mut acia = Acia::new();
    acia.push_rx_byte(0x10);
    acia.push_rx_byte(0x20); // overrun
    assert_ne!(acia.read(reg::CONTROL_STATUS) & 0x21, 0);

    acia.write(reg::CONTROL_STATUS, 0x03); // bits0-1 = 11 : master reset
    let status = acia.read(reg::CONTROL_STATUS);
    assert_eq!(status & 0x01, 0, "RDRF effacé par master reset");
    assert_eq!(status & 0x20, 0, "OVRN effacé par master reset");
    assert_eq!(status & 0x02, 0x02, "TDRE réarmé par master reset");
}
