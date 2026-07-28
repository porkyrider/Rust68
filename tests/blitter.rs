#![cfg(feature = "atari-st")]
//! Tests unitaires du Blitter (`rust68::peripherals::atari_st::blitter`).
//!
//! Aucune suite équivalente à TomHarte n'existe pour ce périphérique :
//! ces tests valident la logique interne implémentée (table de vérité OP,
//! HOP, endmask, parcours X/Y), et pour `skew` spécifiquement, le
//! comportement *tel qu'implémenté* plutôt qu'une référence matérielle
//! vérifiée (voir les limitations documentées dans le module).

use rust68::peripherals::atari_st::blitter::{Blitter, reg};
use rust68::{Bus, FlatBus};

fn write_word(bl: &mut Blitter, offset: u32, value: u16) {
    bl.write(offset, (value >> 8) as u8);
    bl.write(offset + 1, value as u8);
}

fn write_long(bl: &mut Blitter, offset: u32, value: u32) {
    bl.write(offset, (value >> 24) as u8);
    bl.write(offset + 1, (value >> 16) as u8);
    bl.write(offset + 2, (value >> 8) as u8);
    bl.write(offset + 3, value as u8);
}

#[test]
fn registres_16_et_32_bits_round_trip() {
    let mut bl = Blitter::new();
    write_word(&mut bl, reg::SRC_X_INC, 0xFFFE); // -2 en i16
    write_word(&mut bl, reg::X_COUNT, 10);
    write_long(&mut bl, reg::SRC_ADDR, 0x001234);
    write_word(&mut bl, reg::HALFTONE_BASE + 4, 0xABCD); // halftone[2]

    assert_eq!(bl.read(reg::SRC_X_INC), 0xFF);
    assert_eq!(bl.read(reg::SRC_X_INC + 1), 0xFE);
    assert_eq!(
        (bl.read(reg::X_COUNT) as u16) << 8 | bl.read(reg::X_COUNT + 1) as u16,
        10
    );
    assert_eq!(
        (bl.read(reg::SRC_ADDR3) as u32)
            | ((bl.read(reg::SRC_ADDR2) as u32) << 8)
            | ((bl.read(reg::SRC_ADDR1) as u32) << 16)
            | ((bl.read(reg::SRC_ADDR) as u32) << 24),
        0x001234
    );
    assert_eq!(
        (bl.read(reg::HALFTONE_BASE + 4) as u16) << 8 | bl.read(reg::HALFTONE_BASE + 5) as u16,
        0xABCD
    );
}

#[test]
fn hop_zero_ignore_source_et_demi_teinte_op_toujours_un() {
    let mut bl = Blitter::new();
    bl.write(reg::HOP, 0);
    bl.write(reg::OP, 0x0F); // OP = toujours 1, pour isoler l'effet de HOP
    write_word(&mut bl, reg::SRC_X_INC, 2);
    write_word(&mut bl, reg::DST_X_INC, 2);
    write_word(&mut bl, reg::X_COUNT, 1);
    write_word(&mut bl, reg::Y_COUNT, 1);
    write_word(&mut bl, reg::ENDMASK_1, 0xFFFF);
    write_word(&mut bl, reg::ENDMASK_2, 0xFFFF);
    write_word(&mut bl, reg::ENDMASK_3, 0xFFFF);

    let mut bus = FlatBus::new();
    bus.write16(0x1000, 0xFFFF); // source : tout à 1
    bus.write16(0x2000, 0x0000); // dest : tout à 0
    write_long(&mut bl, reg::SRC_ADDR, 0x1000);
    write_long(&mut bl, reg::DST_ADDR, 0x2000);

    bl.execute(&mut bus);

    // OP=0xF -> sortie toujours 1 quel que soit s/d : le résultat doit
    // donc être 0xFFFF indépendamment de HOP.
    assert_eq!(bus.read16(0x2000), 0xFFFF);
}

#[test]
fn hop_zero_vaut_tous_a_un_pas_zero() {
    // D'après le datasheet BLITTER.TXT (info-coach.fr) et le BLIT_FAQ.TXT
    // (ggnkua/Atari_ST_Sources) : la table HOP est 0=tous à 1, 1=demi-teinte,
    // 2=source, 3=source ET demi-teinte — HOP=0 ne met donc PAS le résultat
    // à zéro. On utilise OP=0xC (copie de hop_result) pour observer
    // directement l'effet de HOP.
    let mut bl = Blitter::new();
    bl.write(reg::HOP, 0);
    bl.write(reg::OP, 0xC); // copie hop_result vers la destination
    write_word(&mut bl, reg::SRC_X_INC, 2);
    write_word(&mut bl, reg::DST_X_INC, 2);
    write_word(&mut bl, reg::X_COUNT, 1);
    write_word(&mut bl, reg::Y_COUNT, 1);
    write_word(&mut bl, reg::ENDMASK_1, 0xFFFF);
    write_word(&mut bl, reg::ENDMASK_2, 0xFFFF);
    write_word(&mut bl, reg::ENDMASK_3, 0xFFFF);

    let mut bus = FlatBus::new();
    bus.write16(0x1000, 0x0000); // source : tout à 0 (ne doit pas influer)
    bus.write16(0x2000, 0x5555); // dest : peu importe, remplacée par hop_result
    write_long(&mut bl, reg::SRC_ADDR, 0x1000);
    write_long(&mut bl, reg::DST_ADDR, 0x2000);

    bl.execute(&mut bus);

    assert_eq!(bus.read16(0x2000), 0xFFFF, "HOP=0 -> tous les bits à 1");
}

#[test]
fn op_0x3_est_not_source_op_0xa_est_inchange() {
    let mut bl = Blitter::new();
    bl.write(reg::HOP, 2); // source seule (pas de demi-teinte)
    write_word(&mut bl, reg::SRC_X_INC, 2);
    write_word(&mut bl, reg::DST_X_INC, 2);
    write_word(&mut bl, reg::X_COUNT, 1);
    write_word(&mut bl, reg::Y_COUNT, 1);
    write_word(&mut bl, reg::ENDMASK_1, 0xFFFF);
    write_word(&mut bl, reg::ENDMASK_2, 0xFFFF);
    write_word(&mut bl, reg::ENDMASK_3, 0xFFFF);

    // OP=0x3 : NOT(source), indépendant de la destination.
    bl.write(reg::OP, 0x3);
    let mut bus = FlatBus::new();
    bus.write16(0x1000, 0b1010_1010_1010_1010);
    bus.write16(0x2000, 0b1111_0000_1111_0000);
    write_long(&mut bl, reg::SRC_ADDR, 0x1000);
    write_long(&mut bl, reg::DST_ADDR, 0x2000);
    bl.execute(&mut bus);
    assert_eq!(bus.read16(0x2000), !0b1010_1010_1010_1010u16);

    // OP=0xA : destination inchangée, indépendant de la source.
    let mut bl = Blitter::new();
    bl.write(reg::HOP, 2);
    write_word(&mut bl, reg::SRC_X_INC, 2);
    write_word(&mut bl, reg::DST_X_INC, 2);
    write_word(&mut bl, reg::X_COUNT, 1);
    write_word(&mut bl, reg::Y_COUNT, 1);
    write_word(&mut bl, reg::ENDMASK_1, 0xFFFF);
    write_word(&mut bl, reg::ENDMASK_2, 0xFFFF);
    write_word(&mut bl, reg::ENDMASK_3, 0xFFFF);
    bl.write(reg::OP, 0xA);
    let mut bus = FlatBus::new();
    bus.write16(0x1000, 0xFFFF);
    bus.write16(0x2000, 0x1234);
    write_long(&mut bl, reg::SRC_ADDR, 0x1000);
    write_long(&mut bl, reg::DST_ADDR, 0x2000);
    bl.execute(&mut bus);
    assert_eq!(bus.read16(0x2000), 0x1234, "OP=0xA laisse la destination inchangée");
}

#[test]
fn endmask_masque_le_premier_et_dernier_mot_de_chaque_ligne() {
    let mut bl = Blitter::new();
    bl.write(reg::HOP, 2); // source seule
    bl.write(reg::OP, 0xC); // remplace par la source (copie pure)
    write_word(&mut bl, reg::SRC_X_INC, 2);
    write_word(&mut bl, reg::DST_X_INC, 2);
    write_word(&mut bl, reg::X_COUNT, 3);
    write_word(&mut bl, reg::Y_COUNT, 1);
    write_word(&mut bl, reg::ENDMASK_1, 0x00FF); // premier mot : seul l'octet bas passe
    write_word(&mut bl, reg::ENDMASK_2, 0xFFFF); // mot du milieu : tout passe
    write_word(&mut bl, reg::ENDMASK_3, 0xFF00); // dernier mot : seul l'octet haut passe

    let mut bus = FlatBus::new();
    for i in 0..3 {
        bus.write16(0x1000 + i * 2, 0xFFFF);
        bus.write16(0x2000 + i * 2, 0x0000);
    }
    write_long(&mut bl, reg::SRC_ADDR, 0x1000);
    write_long(&mut bl, reg::DST_ADDR, 0x2000);

    bl.execute(&mut bus);

    assert_eq!(bus.read16(0x2000), 0x00FF, "premier mot masqué par ENDMASK1");
    assert_eq!(bus.read16(0x2002), 0xFFFF, "mot du milieu masqué par ENDMASK2");
    assert_eq!(bus.read16(0x2004), 0xFF00, "dernier mot masqué par ENDMASK3");
}

#[test]
fn parcours_y_avance_via_les_increments_y() {
    let mut bl = Blitter::new();
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0xC); // copie
    write_word(&mut bl, reg::SRC_X_INC, 2);
    write_word(&mut bl, reg::DST_X_INC, 2);
    write_word(&mut bl, reg::SRC_Y_INC, 0); // pas d'avancée Y côté source : relit la même ligne
    write_word(&mut bl, reg::DST_Y_INC, 4); // saute une ligne de 2 mots côté dest
    write_word(&mut bl, reg::X_COUNT, 1);
    write_word(&mut bl, reg::Y_COUNT, 2);
    write_word(&mut bl, reg::ENDMASK_1, 0xFFFF);
    write_word(&mut bl, reg::ENDMASK_2, 0xFFFF);
    write_word(&mut bl, reg::ENDMASK_3, 0xFFFF);

    let mut bus = FlatBus::new();
    bus.write16(0x1000, 0x4242);
    write_long(&mut bl, reg::SRC_ADDR, 0x1000);
    write_long(&mut bl, reg::DST_ADDR, 0x2000);

    bl.execute(&mut bus);

    assert_eq!(bus.read16(0x2000), 0x4242, "ligne 0");
    assert_eq!(bus.read16(0x2004), 0x4242, "ligne 1, après DST_Y_INC");
}

#[test]
fn halftone_cycle_par_ligne() {
    let mut bl = Blitter::new();
    bl.write(reg::HOP, 1); // demi-teinte seule
    bl.write(reg::OP, 0xC); // copie du résultat HOP
    write_word(&mut bl, reg::HALFTONE_BASE, 0x1111);
    write_word(&mut bl, reg::HALFTONE_BASE + 2, 0x2222);
    write_word(&mut bl, reg::SRC_X_INC, 2);
    write_word(&mut bl, reg::DST_X_INC, 2);
    write_word(&mut bl, reg::SRC_Y_INC, 0);
    write_word(&mut bl, reg::DST_Y_INC, 4);
    write_word(&mut bl, reg::X_COUNT, 1);
    write_word(&mut bl, reg::Y_COUNT, 2);
    write_word(&mut bl, reg::ENDMASK_1, 0xFFFF);
    write_word(&mut bl, reg::ENDMASK_2, 0xFFFF);
    write_word(&mut bl, reg::ENDMASK_3, 0xFFFF);

    let mut bus = FlatBus::new();
    write_long(&mut bl, reg::SRC_ADDR, 0x1000);
    write_long(&mut bl, reg::DST_ADDR, 0x2000);
    bl.execute(&mut bus);

    assert_eq!(bus.read16(0x2000), 0x1111, "ligne 0 utilise halftone[0]");
    assert_eq!(bus.read16(0x2004), 0x2222, "ligne 1 utilise halftone[1]");
}

#[test]
fn busy_efface_et_y_count_remis_a_zero_apres_execute() {
    let mut bl = Blitter::new();
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0xC);
    write_word(&mut bl, reg::X_COUNT, 1);
    write_word(&mut bl, reg::Y_COUNT, 1);
    write_word(&mut bl, reg::ENDMASK_1, 0xFFFF);
    write_word(&mut bl, reg::ENDMASK_2, 0xFFFF);
    write_word(&mut bl, reg::ENDMASK_3, 0xFFFF);
    bl.write(reg::CONTROL, 1 << 7); // BUSY posé "à la main" pour le test

    let mut bus = FlatBus::new();
    bl.execute(&mut bus);

    assert!(!bl.busy(), "BUSY doit être effacé après execute()");
    assert_eq!(bl.read(reg::Y_COUNT), 0);
    assert_eq!(bl.read(reg::Y_COUNT + 1), 0);
}

#[test]
fn fxsr_amorce_le_registre_tampon_avant_la_premiere_lecture() {
    // D'après le datasheet : FXSR (bit 7 de SKEW) déclenche une lecture
    // source supplémentaire en tout début de ligne, pour amorcer le
    // "registre tampon" utilisé par le décalage skew. Sans FXSR, ce
    // tampon part de zéro.
    let mut bl = Blitter::new();
    bl.write(reg::HOP, 2); // source seule
    bl.write(reg::OP, 0xC); // copie
    bl.write(reg::SKEW, 0x84); // FXSR=1, skew=4
    write_word(&mut bl, reg::SRC_X_INC, 2);
    write_word(&mut bl, reg::DST_X_INC, 2);
    write_word(&mut bl, reg::X_COUNT, 1);
    write_word(&mut bl, reg::Y_COUNT, 1);
    write_word(&mut bl, reg::ENDMASK_1, 0xFFFF);
    write_word(&mut bl, reg::ENDMASK_2, 0xFFFF);
    write_word(&mut bl, reg::ENDMASK_3, 0xFFFF);

    let mut bus = FlatBus::new();
    bus.write16(0x0FFE, 0xF000); // mot "précédent" (avant la source)
    bus.write16(0x1000, 0x0000); // mot source courant
    write_long(&mut bl, reg::SRC_ADDR, 0x1000);
    write_long(&mut bl, reg::DST_ADDR, 0x2000);

    bl.execute(&mut bus);

    assert_eq!(
        bus.read16(0x2000),
        0x000F,
        "FXSR=1 : les 4 bits hauts du mot précédent apparaissent dans le résultat décalé"
    );

    // Même essai sans FXSR : le tampon d'amorçage part de zéro, le mot
    // précédent en mémoire n'a donc plus d'effet.
    let mut bl = Blitter::new();
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0xC);
    bl.write(reg::SKEW, 0x04); // FXSR=0, skew=4
    write_word(&mut bl, reg::SRC_X_INC, 2);
    write_word(&mut bl, reg::DST_X_INC, 2);
    write_word(&mut bl, reg::X_COUNT, 1);
    write_word(&mut bl, reg::Y_COUNT, 1);
    write_word(&mut bl, reg::ENDMASK_1, 0xFFFF);
    write_word(&mut bl, reg::ENDMASK_2, 0xFFFF);
    write_word(&mut bl, reg::ENDMASK_3, 0xFFFF);
    let mut bus = FlatBus::new();
    bus.write16(0x0FFE, 0xF000);
    bus.write16(0x1000, 0x0000);
    write_long(&mut bl, reg::SRC_ADDR, 0x1000);
    write_long(&mut bl, reg::DST_ADDR, 0x2000);
    bl.execute(&mut bus);
    assert_eq!(bus.read16(0x2000), 0x0000, "FXSR=0 : pas d'amorçage, tampon initial à zéro");
}

#[test]
fn nfsr_supprime_la_derniere_lecture_source_de_la_ligne() {
    // NFSR (bit 6 de SKEW) : la dernière lecture source d'une ligne n'est
    // pas effectuée. Avec skew=0 (source utilisée telle quelle), le mot
    // en mémoire au dernier index doit donc être ignoré au profit de zéro.
    let mut bl = Blitter::new();
    bl.write(reg::HOP, 2); // source seule
    bl.write(reg::OP, 0xC); // copie
    bl.write(reg::SKEW, 0x40); // NFSR=1, skew=0
    write_word(&mut bl, reg::SRC_X_INC, 2);
    write_word(&mut bl, reg::DST_X_INC, 2);
    write_word(&mut bl, reg::X_COUNT, 1); // un seul mot : c'est aussi le dernier
    write_word(&mut bl, reg::Y_COUNT, 1);
    write_word(&mut bl, reg::ENDMASK_1, 0xFFFF);
    write_word(&mut bl, reg::ENDMASK_2, 0xFFFF);
    write_word(&mut bl, reg::ENDMASK_3, 0xFFFF);

    let mut bus = FlatBus::new();
    bus.write16(0x1000, 0xFFFF); // ne doit pas être lu
    write_long(&mut bl, reg::SRC_ADDR, 0x1000);
    write_long(&mut bl, reg::DST_ADDR, 0x2000);

    bl.execute(&mut bus);

    assert_eq!(bus.read16(0x2000), 0x0000, "NFSR=1 : dernière lecture source omise");
}

#[test]
fn smudge_choisit_la_demi_teinte_via_les_bits_bas_de_la_source() {
    // SMUDGE (bit 5 de CONTROL) : le mot de demi-teinte utilisé pour
    // chaque mot vient des 4 bits bas du mot source décalé, pas du
    // numéro de ligne courant — donc potentiellement différent à chaque
    // mot d'une même ligne (contrairement au mode normal).
    let mut bl = Blitter::new();
    bl.write(reg::HOP, 1); // demi-teinte seule
    bl.write(reg::OP, 0xC); // copie du résultat HOP
    bl.write(reg::CONTROL, 0x20); // SMUDGE=1, numéro de ligne=0
    write_word(&mut bl, reg::HALFTONE_BASE + 2 * 3, 0x3333); // halftone[3]
    write_word(&mut bl, reg::HALFTONE_BASE + 2 * 7, 0x7777); // halftone[7]
    write_word(&mut bl, reg::SRC_X_INC, 2);
    write_word(&mut bl, reg::DST_X_INC, 2);
    write_word(&mut bl, reg::X_COUNT, 2);
    write_word(&mut bl, reg::Y_COUNT, 1);
    write_word(&mut bl, reg::ENDMASK_1, 0xFFFF);
    write_word(&mut bl, reg::ENDMASK_2, 0xFFFF);
    write_word(&mut bl, reg::ENDMASK_3, 0xFFFF);

    let mut bus = FlatBus::new();
    bus.write16(0x1000, 0x0003); // nibble bas = 3
    bus.write16(0x1002, 0x0007); // nibble bas = 7
    write_long(&mut bl, reg::SRC_ADDR, 0x1000);
    write_long(&mut bl, reg::DST_ADDR, 0x2000);

    bl.execute(&mut bus);

    assert_eq!(bus.read16(0x2000), 0x3333, "mot 0 : nibble source 3 -> halftone[3]");
    assert_eq!(bus.read16(0x2002), 0x7777, "mot 1 : nibble source 7 -> halftone[7], même ligne");
}

#[test]
fn numero_de_ligne_demi_teinte_lisible_et_reglable_via_control() {
    // Le numéro de ligne de demi-teinte est exposé directement par les
    // bits 0-3 de CONTROL (lisible/inscriptible), pas un compteur caché.
    // Sa direction d'avancement suit le signe de DST_Y_INC.
    let mut bl = Blitter::new();
    bl.write(reg::HOP, 1); // demi-teinte seule
    bl.write(reg::OP, 0xC); // copie
    write_word(&mut bl, reg::HALFTONE_BASE + 2 * 5, 0x5555); // halftone[5]
    write_word(&mut bl, reg::HALFTONE_BASE + 2 * 4, 0x4444); // halftone[4]

    bl.write(reg::CONTROL, 5); // pré-positionne le numéro de ligne à 5
    assert_eq!(bl.read(reg::CONTROL) & 0x0F, 5, "numéro de ligne relu tel qu'écrit");

    write_word(&mut bl, reg::SRC_X_INC, 2);
    write_word(&mut bl, reg::DST_X_INC, 2);
    write_word(&mut bl, reg::SRC_Y_INC, 0);
    write_word(&mut bl, reg::DST_Y_INC, 0xFFFC); // -4 en i16
    write_word(&mut bl, reg::X_COUNT, 1);
    write_word(&mut bl, reg::Y_COUNT, 2);
    write_word(&mut bl, reg::ENDMASK_1, 0xFFFF);
    write_word(&mut bl, reg::ENDMASK_2, 0xFFFF);
    write_word(&mut bl, reg::ENDMASK_3, 0xFFFF);

    let mut bus = FlatBus::new();
    write_long(&mut bl, reg::SRC_ADDR, 0x1000);
    write_long(&mut bl, reg::DST_ADDR, 0x2000);
    bl.execute(&mut bus);

    assert_eq!(bus.read16(0x2000), 0x5555, "ligne 0 : numéro pré-positionné à 5");
    assert_eq!(
        bus.read16(0x1FFC),
        0x4444,
        "ligne 1 : numéro décrémenté à 4 (DST_Y_INC négatif), dst = 0x2000-4"
    );
    assert_eq!(
        bl.read(reg::CONTROL) & 0x0F,
        3,
        "numéro de ligne final = 5-2 après 2 lignes décroissantes"
    );
}

#[test]
fn skew_zero_ne_modifie_pas_le_mot_source() {
    // skew=0 doit toujours renvoyer le mot courant tel quel, quel que soit
    // le mot précédent — c'est la partie de `skew` dont on est certain.
    let mut bl = Blitter::new();
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0xC); // copie
    bl.write(reg::SKEW, 0);
    write_word(&mut bl, reg::SRC_X_INC, 2);
    write_word(&mut bl, reg::DST_X_INC, 2);
    write_word(&mut bl, reg::X_COUNT, 1);
    write_word(&mut bl, reg::Y_COUNT, 1);
    write_word(&mut bl, reg::ENDMASK_1, 0xFFFF);
    write_word(&mut bl, reg::ENDMASK_2, 0xFFFF);
    write_word(&mut bl, reg::ENDMASK_3, 0xFFFF);

    let mut bus = FlatBus::new();
    bus.write16(0x0FFE, 0xAAAA); // mot juste avant la source (ne doit pas influer)
    bus.write16(0x1000, 0x1234);
    write_long(&mut bl, reg::SRC_ADDR, 0x1000);
    write_long(&mut bl, reg::DST_ADDR, 0x2000);

    bl.execute(&mut bus);

    assert_eq!(bus.read16(0x2000), 0x1234, "skew=0 : mot source inchangé");
}
