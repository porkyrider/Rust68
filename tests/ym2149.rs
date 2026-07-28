#![cfg(feature = "atari-st")]
//! Tests unitaires du YM2149 (`rust68::peripherals::atari_st::ym2149`).

use rust68::peripherals::atari_st::ym2149::{Ym2149, bus_offset, reg};

fn select(chip: &mut Ym2149, r: u8) {
    chip.write(bus_offset::SELECT, r);
}

fn write_reg(chip: &mut Ym2149, r: u8, value: u8) {
    select(chip, r);
    chip.write(bus_offset::DATA, value);
}

fn read_reg(chip: &mut Ym2149, r: u8) -> u8 {
    select(chip, r);
    chip.read(bus_offset::DATA)
}

#[test]
fn registres_masques_selon_leur_largeur_reelle() {
    let mut chip = Ym2149::new();
    write_reg(&mut chip, reg::TONE_A_COARSE, 0xFF);
    assert_eq!(read_reg(&mut chip, reg::TONE_A_COARSE), 0x0F, "coarse tone : 4 bits");

    write_reg(&mut chip, reg::NOISE_PERIOD, 0xFF);
    assert_eq!(read_reg(&mut chip, reg::NOISE_PERIOD), 0x1F, "période de bruit : 5 bits");

    write_reg(&mut chip, reg::ENVELOPE_SHAPE, 0xFF);
    assert_eq!(read_reg(&mut chip, reg::ENVELOPE_SHAPE), 0x0F, "forme d'enveloppe : 4 bits");

    write_reg(&mut chip, reg::TONE_A_FINE, 0xFF);
    assert_eq!(read_reg(&mut chip, reg::TONE_A_FINE), 0xFF, "fine tone : 8 bits complets");
}

#[test]
fn selecteur_de_registre_lisible() {
    let mut chip = Ym2149::new();
    select(&mut chip, 5);
    assert_eq!(chip.read(bus_offset::SELECT), 5);
}

#[test]
fn tonalite_bascule_au_rythme_programme() {
    let mut chip = Ym2149::new();
    // Période = 1 : bascule à chaque cycle puce (= 4 cycles CPU).
    write_reg(&mut chip, reg::TONE_A_FINE, 1);
    write_reg(&mut chip, reg::TONE_A_COARSE, 0);
    // Mixer : tonalité A activée (bit0=0), bruit A désactivé (bit3=1).
    write_reg(&mut chip, reg::MIXER, 0b0000_1000);
    write_reg(&mut chip, reg::AMPLITUDE_A, 0x0F); // volume max fixe

    let mut levels = Vec::new();
    for _ in 0..6 {
        chip.tick(4); // 1 cycle puce
        levels.push(chip.channel_level(0));
    }
    // Période minimale (1) : bascule dès le premier cycle puce, puis
    // alterne 30 (0x0F*2) / 0 à chaque cycle suivant.
    assert_eq!(levels, vec![30, 0, 30, 0, 30, 0]);
}

#[test]
fn mixer_coupe_tonalite_ou_bruit_selon_les_bits() {
    let mut chip = Ym2149::new();
    write_reg(&mut chip, reg::AMPLITUDE_A, 0x0F);
    // Tonalité et bruit tous deux désactivés (bits 0 et 3 à 1) : le canal
    // doit rester au niveau plein (portillon "activé" = 1 en interne).
    write_reg(&mut chip, reg::MIXER, 0b0000_1001);
    chip.tick(100);
    assert_eq!(chip.channel_level(0), 30, "tone+bruit coupés : porte toujours ouverte");
}

#[test]
fn bruit_produit_une_sequence_non_triviale() {
    let mut chip = Ym2149::new();
    write_reg(&mut chip, reg::NOISE_PERIOD, 1);
    write_reg(&mut chip, reg::MIXER, 0b0000_0001); // tone A coupée, bruit A actif
    write_reg(&mut chip, reg::AMPLITUDE_A, 0x0F);

    let mut seen_zero = false;
    let mut seen_nonzero = false;
    for _ in 0..64 {
        chip.tick(4);
        if chip.channel_level(0) == 0 {
            seen_zero = true;
        } else {
            seen_nonzero = true;
        }
    }
    assert!(seen_zero && seen_nonzero, "le LFSR doit produire les deux niveaux");
}

/// Mixer avec tonalité ET bruit désactivés (portes toujours ouvertes) pour
/// isoler le niveau d'amplitude/enveloppe du canal A dans les tests.
const MIXER_GATES_OPEN_A: u8 = 0b0000_1001;

#[test]
fn enveloppe_mode_active_sur_bit4_amplitude() {
    let mut chip = Ym2149::new();
    write_reg(&mut chip, reg::MIXER, MIXER_GATES_OPEN_A);
    write_reg(&mut chip, reg::ENVELOPE_FINE, 1);
    write_reg(&mut chip, reg::ENVELOPE_COARSE, 0);
    // Forme "attack seul, continue=1, alternate=0, hold=0" -> dents de scie montantes
    write_reg(&mut chip, reg::ENVELOPE_SHAPE, 0b1100);
    write_reg(&mut chip, reg::AMPLITUDE_A, 0x10); // bit4 = mode enveloppe

    // Juste après le reset (écriture du registre de forme), le niveau doit
    // être au minimum de la rampe montante.
    assert_eq!(chip.channel_level(0), 0);
}

#[test]
fn enveloppe_attack_sans_continue_se_fige_a_31_apres_une_rampe() {
    let mut chip = Ym2149::new();
    write_reg(&mut chip, reg::MIXER, MIXER_GATES_OPEN_A);
    write_reg(&mut chip, reg::ENVELOPE_FINE, 1);
    write_reg(&mut chip, reg::ENVELOPE_COARSE, 0);
    // continue=0, attack=1 : une seule rampe montante puis fige à 31 (max).
    write_reg(&mut chip, reg::ENVELOPE_SHAPE, 0b0100);
    write_reg(&mut chip, reg::AMPLITUDE_A, 0x10);

    // 32 paliers pour compléter la rampe (période=1 cycle puce = 4 cycles CPU).
    for _ in 0..32 {
        chip.tick(4);
    }
    // L'enveloppe a sa propre échelle 0-31 (résolution double de l'ampli
    // fixe 0-15), renvoyée telle quelle par channel_level en mode
    // enveloppe (pas de ×2 ici, contrairement au mode ampli fixe).
    assert_eq!(chip.channel_level(0), 31, "figé au maximum de la rampe");

    // Continuer à avancer ne doit plus rien changer.
    chip.tick(400);
    assert_eq!(chip.channel_level(0), 31);
}

#[test]
fn ecrire_le_registre_de_forme_relance_l_enveloppe() {
    let mut chip = Ym2149::new();
    write_reg(&mut chip, reg::MIXER, MIXER_GATES_OPEN_A);
    write_reg(&mut chip, reg::ENVELOPE_FINE, 1);
    write_reg(&mut chip, reg::ENVELOPE_COARSE, 0);
    write_reg(&mut chip, reg::ENVELOPE_SHAPE, 0b0100); // attack
    for _ in 0..10 {
        chip.tick(4);
    }
    // Réécrire (même valeur ou non) doit repartir de zéro.
    write_reg(&mut chip, reg::ENVELOPE_SHAPE, 0b0100);
    write_reg(&mut chip, reg::AMPLITUDE_A, 0x10);
    assert_eq!(chip.channel_level(0), 0, "l'enveloppe doit être repartie de zéro");
}

#[test]
fn port_a_bascule_entre_entree_et_sortie_selon_ddr() {
    let mut chip = Ym2149::new();
    // DDR port A = entrée (bit6 de MIXER = 0).
    write_reg(&mut chip, reg::MIXER, 0);
    chip.set_port_a_input(0x42);
    assert_eq!(read_reg(&mut chip, reg::IO_PORT_A), 0x42);

    // Bascule en sortie (bit6 = 1) : la lecture reflète maintenant le latch
    // écrit, pas l'entrée externe.
    write_reg(&mut chip, reg::MIXER, 0b0100_0000);
    write_reg(&mut chip, reg::IO_PORT_A, 0x99);
    assert_eq!(read_reg(&mut chip, reg::IO_PORT_A), 0x99);
}
