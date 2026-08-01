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

// Écriture par mot/long complet — reflète le vrai chemin d'accès `.W`/`.L`
// du CPU (voir `Blitter::write_word`/`Blitter::write_long` : un accès `.B`
// isolé sur ces registres est ignoré sur le silicium réel, donc composer la
// valeur via deux/quatre `bl.write()` octet par octet n'aurait plus aucun
// effet depuis ce changement).
fn write_word(bl: &mut Blitter, offset: u32, value: u16) {
    bl.write_word(offset, value);
}

fn write_long(bl: &mut Blitter, offset: u32, value: u32) {
    bl.write_long(offset, value);
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
    bl.write(reg::OP, 0x3); // copie hop_result vers la destination
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

/// Table de vérité OP vérifiée par résolution directe du système
/// d'équations posé par le manuel Blitter officiel (User Manual for the
/// Atari ST Bit-Block Transfer Processor, archive.org — recoupé avec
/// BLITTER.TXT, mêmes valeurs) : OP=1 "source AND destination", OP=2
/// "source AND NOT destination", OP=4 "NOT source AND destination", OP=8
/// "NOT source AND NOT destination" ne sont simultanément satisfaisables
/// qu'avec l'index inversé `3 - ((s<<1)|d)` — pas l'index direct
/// `(s<<1)|d` qu'utilisait le code avant correction (bug confirmé : cet
/// index direct donnait par ex. OP=3="NOT source" et OP=0xA="destination
/// inchangée" au lieu de "source" et "NOT destination").
#[test]
fn op_0x3_est_source_op_0xa_est_not_destination() {
    let mut bl = Blitter::new();
    bl.write(reg::HOP, 2); // source seule (pas de demi-teinte)
    write_word(&mut bl, reg::SRC_X_INC, 2);
    write_word(&mut bl, reg::DST_X_INC, 2);
    write_word(&mut bl, reg::X_COUNT, 1);
    write_word(&mut bl, reg::Y_COUNT, 1);
    write_word(&mut bl, reg::ENDMASK_1, 0xFFFF);
    write_word(&mut bl, reg::ENDMASK_2, 0xFFFF);
    write_word(&mut bl, reg::ENDMASK_3, 0xFFFF);

    // OP=0x3 : source, indépendant de la destination.
    bl.write(reg::OP, 0x3);
    let mut bus = FlatBus::new();
    bus.write16(0x1000, 0b1010_1010_1010_1010);
    bus.write16(0x2000, 0b1111_0000_1111_0000);
    write_long(&mut bl, reg::SRC_ADDR, 0x1000);
    write_long(&mut bl, reg::DST_ADDR, 0x2000);
    bl.execute(&mut bus);
    assert_eq!(bus.read16(0x2000), 0b1010_1010_1010_1010, "OP=0x3 copie la source telle quelle");

    // OP=0xA : NOT(destination), indépendant de la source.
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
    assert_eq!(bus.read16(0x2000), !0x1234u16, "OP=0xA inverse la destination, indépendamment de la source");
}

/// Vérifie les 16 valeurs d'OP en une seule fois (un seul bit source/dest
/// à la fois, pour lire directement la table de vérité), contre le manuel
/// Blitter officiel — verrouille le bug d'indexation corrigé (voir
/// `apply_op`) pour de bon.
#[test]
fn table_de_verite_op_complete_seize_valeurs() {
    // (règle, table [((0,0)),(0,1),(1,0),(1,1)] pour (source,destination))
    let regles: [(u8, [bool; 4]); 16] = [
        (0x0, [false, false, false, false]), // all zeros
        (0x1, [false, false, false, true]),  // source AND destination
        (0x2, [false, false, true, false]),  // source AND NOT destination
        (0x3, [false, false, true, true]),   // source
        (0x4, [false, true, false, false]),  // NOT source AND destination
        (0x5, [false, true, false, true]),   // destination
        (0x6, [false, true, true, false]),   // source XOR destination
        (0x7, [false, true, true, true]),    // source OR destination
        (0x8, [true, false, false, false]),  // NOT source AND NOT destination
        (0x9, [true, false, false, true]),   // NOT(source XOR destination)
        (0xA, [true, false, true, false]),   // NOT destination
        (0xB, [true, false, true, true]),    // source OR NOT destination
        (0xC, [true, true, false, false]),   // NOT source
        (0xD, [true, true, false, true]),    // NOT source OR destination
        (0xE, [true, true, true, false]),    // NOT source OR NOT destination
        (0xF, [true, true, true, true]),     // all ones
    ];

    for (op, table) in regles {
        for (i, &(s, d)) in [(0u16, 0u16), (0, 1), (1, 0), (1, 1)].iter().enumerate() {
            let mut bl = Blitter::new();
            bl.write(reg::HOP, 2); // source seule
            bl.write(reg::OP, op);
            write_word(&mut bl, reg::SRC_X_INC, 2);
            write_word(&mut bl, reg::DST_X_INC, 2);
            write_word(&mut bl, reg::X_COUNT, 1);
            write_word(&mut bl, reg::Y_COUNT, 1);
            write_word(&mut bl, reg::ENDMASK_1, 0xFFFF);
            write_word(&mut bl, reg::ENDMASK_2, 0xFFFF);
            write_word(&mut bl, reg::ENDMASK_3, 0xFFFF);
            let mut bus = FlatBus::new();
            bus.write16(0x1000, s * 0xFFFF);
            bus.write16(0x2000, d * 0xFFFF);
            write_long(&mut bl, reg::SRC_ADDR, 0x1000);
            write_long(&mut bl, reg::DST_ADDR, 0x2000);
            bl.execute(&mut bus);
            let expected = if table[i] { 0xFFFFu16 } else { 0x0000 };
            assert_eq!(
                bus.read16(0x2000),
                expected,
                "OP={op:#x} avec (source={s},destination={d}) doit donner {expected:#06x}"
            );
        }
    }
}

#[test]
fn endmask_masque_le_premier_et_dernier_mot_de_chaque_ligne() {
    let mut bl = Blitter::new();
    bl.write(reg::HOP, 2); // source seule
    bl.write(reg::OP, 0x3); // remplace par la source (copie pure)
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

/// D'après le manuel Blitter officiel : "In the case of a one word line
/// ENDMASK 1 is used" — ENDMASK_3 est simplement IGNORÉ pour une ligne
/// d'un seul mot, pas combiné par ET avec ENDMASK_1 (une correction
/// antérieure avait cru le contraire — "les deux masques fusionnent" —
/// ce qui mettait silencieusement à zéro des blits par ailleurs valides
/// dès qu'ENDMASK_3 valait 0, un cas très fréquent en pratique observé
/// sur les petits blits du curseur souris).
#[test]
fn ligne_d_un_seul_mot_utilise_endmask1_seul_endmask3_ignore() {
    let mut bl = Blitter::new();
    bl.write(reg::HOP, 2); // source seule
    bl.write(reg::OP, 0x3); // remplace par la source (copie pure)
    write_word(&mut bl, reg::SRC_X_INC, 2);
    write_word(&mut bl, reg::DST_X_INC, 2);
    write_word(&mut bl, reg::X_COUNT, 1);
    write_word(&mut bl, reg::Y_COUNT, 1);
    write_word(&mut bl, reg::ENDMASK_1, 0xFF00); // seul masque qui doit compter
    write_word(&mut bl, reg::ENDMASK_2, 0xFFFF); // jamais utilisé ici (pas de mot du milieu)
    write_word(&mut bl, reg::ENDMASK_3, 0x0000); // doit être ignoré, pas combiné

    let mut bus = FlatBus::new();
    bus.write16(0x1000, 0xFFFF);
    bus.write16(0x2000, 0x0000);
    write_long(&mut bl, reg::SRC_ADDR, 0x1000);
    write_long(&mut bl, reg::DST_ADDR, 0x2000);

    bl.execute(&mut bus);

    assert_eq!(
        bus.read16(0x2000),
        0xFF00,
        "ENDMASK_1 seul doit s'appliquer ; ENDMASK_3=0 ne doit pas tout annuler"
    );
}

#[test]
fn parcours_y_avance_via_les_increments_y() {
    let mut bl = Blitter::new();
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x3); // copie
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
    bl.write(reg::OP, 0x3); // copie du résultat HOP
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

/// Le manuel officiel documente que X_COUNT/Y_COUNT décomptent en interne
/// pendant l'exécution puis reviennent à leur valeur INITIALE une fois le
/// blit terminé — jamais à zéro (0 désignant 65536, un appelant qui
/// enchaîne des blits sans réécrire Y_COUNT à chaque fois, en s'appuyant
/// sur cette persistance documentée, verrait sinon un 0 fallacieux =
/// 65536 lignes). Confirmé buggé avant correction : le code remettait
/// `y_count` à 0 explicitement en fin d'exécution.
#[test]
fn busy_efface_et_y_count_retrouve_sa_valeur_initiale_apres_execute() {
    let mut bl = Blitter::new();
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x3);
    write_word(&mut bl, reg::X_COUNT, 1);
    write_word(&mut bl, reg::Y_COUNT, 5);
    write_word(&mut bl, reg::ENDMASK_1, 0xFFFF);
    write_word(&mut bl, reg::ENDMASK_2, 0xFFFF);
    write_word(&mut bl, reg::ENDMASK_3, 0xFFFF);
    bl.write(reg::CONTROL, 1 << 7); // BUSY posé "à la main" pour le test

    let mut bus = FlatBus::new();
    bl.execute(&mut bus);

    assert!(!bl.busy(), "BUSY doit être effacé après execute()");
    assert_eq!(bl.read(reg::Y_COUNT), 0, "octet haut de Y_COUNT=5");
    assert_eq!(bl.read(reg::Y_COUNT + 1), 5, "Y_COUNT doit retrouver sa valeur initiale (5), pas 0");
}

#[test]
fn fxsr_amorce_le_registre_tampon_avant_la_premiere_lecture() {
    // D'après le datasheet : FXSR (bit 7 de SKEW) déclenche une lecture
    // source supplémentaire en tout début de ligne, pour amorcer le
    // "registre tampon" utilisé par le décalage skew. Sans FXSR, ce
    // tampon part de zéro.
    let mut bl = Blitter::new();
    bl.write(reg::HOP, 2); // source seule
    bl.write(reg::OP, 0x3); // copie
    bl.write(reg::SKEW, 0x84); // FXSR=1, skew=4
    write_word(&mut bl, reg::SRC_X_INC, 2);
    write_word(&mut bl, reg::DST_X_INC, 2);
    write_word(&mut bl, reg::X_COUNT, 1);
    write_word(&mut bl, reg::Y_COUNT, 1);
    write_word(&mut bl, reg::ENDMASK_1, 0xFFFF);
    write_word(&mut bl, reg::ENDMASK_2, 0xFFFF);
    write_word(&mut bl, reg::ENDMASK_3, 0xFFFF);

    let mut bus = FlatBus::new();
    // FXSR lit à SRC_ADDR courant (0x1000) puis avance SRC_ADDR de
    // SRC_X_INC AVANT la lecture normale du mot 0, qui a donc lieu à
    // 0x1000+2=0x1002 (confirmé par Hatari, `Blitter_ProcessWord` :
    // `src_addr += src_x_incr` a lieu entre la lecture FXSR et la
    // lecture normale du premier mot).
    bus.write16(0x1000, 0x000F); // mot "précédent" (lu par FXSR) : bits bas = 1111
    bus.write16(0x1002, 0x0000); // mot source courant (mot 0, réellement lu à SRC_ADDR+SRC_X_INC)
    write_long(&mut bl, reg::SRC_ADDR, 0x1000);
    write_long(&mut bl, reg::DST_ADDR, 0x2000);

    bl.execute(&mut bus);

    assert_eq!(
        bus.read16(0x2000),
        0xF000,
        "FXSR=1 : les 4 bits bas du mot précédent remontent en tête du résultat décalé"
    );

    // Même essai sans FXSR : le tampon d'amorçage part de zéro, le mot
    // précédent en mémoire n'a donc plus d'effet.
    let mut bl = Blitter::new();
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x3);
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
    // pas effectuée — le registre tampon source garde la dernière valeur
    // qui y a été chargée (confirmé par Hatari : `Blitter_SourceFetch(true)`
    // réutilise `bus_word` au lieu de relire la mémoire). Ici FXSR=0, donc
    // le tampon initial vaut 0 : le résultat est 0 non pas parce que NFSR
    // force un zéro, mais parce que c'est la valeur (par ailleurs nulle) déjà
    // présente dans le tampon. Voir le test suivant pour un tampon non nul.
    let mut bl = Blitter::new();
    bl.write(reg::HOP, 2); // source seule
    bl.write(reg::OP, 0x3); // copie
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

    assert_eq!(bus.read16(0x2000), 0x0000, "NFSR=1, FXSR=0 : tampon initial nul réutilisé");
}

#[test]
fn nfsr_reutilise_le_dernier_mot_source_lu_et_non_zero() {
    // Avec FXSR=1 ET NFSR=1, le tampon source est amorcé par la lecture
    // FXSR (à SRC_ADDR courant, 0x1000) puis, comme X_COUNT=1 fait du seul
    // mot de la ligne le dernier, NFSR empêche toute nouvelle lecture bus
    // (qui aurait eu lieu à 0x1000+SRC_X_INC=0x1002 sinon) : c'est donc la
    // valeur amorcée par FXSR qui doit apparaître en sortie, PAS zéro. Une
    // version précédente forçait la source du dernier mot à 0, ce qui
    // aurait donné 0x0000 ici au lieu de 0xABCD — bug réel identifié en
    // comparant avec Hatari (`bus_word` réutilisé, pas remis à zéro).
    let mut bl = Blitter::new();
    bl.write(reg::HOP, 2); // source seule
    bl.write(reg::OP, 0x3); // copie
    bl.write(reg::SKEW, 0xC0); // FXSR=1, NFSR=1, skew=0
    write_word(&mut bl, reg::SRC_X_INC, 2);
    write_word(&mut bl, reg::DST_X_INC, 2);
    write_word(&mut bl, reg::X_COUNT, 1);
    write_word(&mut bl, reg::Y_COUNT, 1);
    write_word(&mut bl, reg::ENDMASK_1, 0xFFFF);
    write_word(&mut bl, reg::ENDMASK_2, 0xFFFF);
    write_word(&mut bl, reg::ENDMASK_3, 0xFFFF);

    let mut bus = FlatBus::new();
    bus.write16(0x1000, 0xABCD); // lu par FXSR (à SRC_ADDR courant)
    bus.write16(0x1002, 0x1111); // ne doit PAS être lu (NFSR)
    write_long(&mut bl, reg::SRC_ADDR, 0x1000);
    write_long(&mut bl, reg::DST_ADDR, 0x2000);

    bl.execute(&mut bus);

    assert_eq!(
        bus.read16(0x2000),
        0xABCD,
        "NFSR=1 : réutilise le mot amorcé par FXSR, pas zéro"
    );
}

#[test]
fn smudge_choisit_la_demi_teinte_via_les_bits_bas_de_la_source() {
    // SMUDGE (bit 5 de CONTROL) : le mot de demi-teinte utilisé pour
    // chaque mot vient des 4 bits bas du mot source décalé, pas du
    // numéro de ligne courant — donc potentiellement différent à chaque
    // mot d'une même ligne (contrairement au mode normal).
    let mut bl = Blitter::new();
    bl.write(reg::HOP, 1); // demi-teinte seule
    bl.write(reg::OP, 0x3); // copie du résultat HOP
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
    bl.write(reg::OP, 0x3); // copie
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
    bl.write(reg::OP, 0x3); // copie
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

/// Vérifie la combinaison décalée contre l'exemple chiffré concret de
/// BLIT_FAQ.TXT (SKEW=3 : "reads out bits 18..3" d'un tampon [précédent
/// (haut, bits 16-31)][courant (bas, bits 0-15)]) — verrouille le bug
/// (ordre des mots ET sens du décalage inversés) trouvé et corrigé cette
/// session dans `skewed_source`.
#[test]
fn skew_non_nul_combine_precedent_et_courant_dans_le_bon_ordre() {
    let mut bl = Blitter::new();
    bl.write(reg::HOP, 2); // source seule
    bl.write(reg::OP, 0x3); // copie
    bl.write(reg::SKEW, 0x83); // FXSR posé (bit 7) + skew=3

    write_word(&mut bl, reg::SRC_X_INC, 2);
    write_word(&mut bl, reg::DST_X_INC, 2);
    write_word(&mut bl, reg::X_COUNT, 1);
    write_word(&mut bl, reg::Y_COUNT, 1);
    write_word(&mut bl, reg::ENDMASK_1, 0xFFFF);
    write_word(&mut bl, reg::ENDMASK_2, 0xFFFF);
    write_word(&mut bl, reg::ENDMASK_3, 0xFFFF);

    let mut bus = FlatBus::new();
    // FXSR lit à SRC_ADDR courant (0x1000) puis SRC_ADDR avance de
    // SRC_X_INC avant la lecture normale du mot 0, qui a donc lieu à
    // 0x1000+2=0x1002.
    bus.write16(0x1000, 0x0005); // mot "précédent" (lu par FXSR) : bits bas = 101
    bus.write16(0x1002, 0x0000); // mot "courant" (mot 0)
    write_long(&mut bl, reg::SRC_ADDR, 0x1000);
    write_long(&mut bl, reg::DST_ADDR, 0x2000);

    bl.execute(&mut bus);

    // Attendu : bits bas (101) du mot précédent remontés en tête (décalés
    // de 16-3=13), combinés aux bits hauts (tous à 0 ici) du mot courant
    // décalé à droite de 3 → 0b101_0000000000000 = 0xA000.
    assert_eq!(bus.read16(0x2000), 0xA000, "combinaison decalée précédent/courant dans le bon ordre");
}

/// Même exemple chiffré que le test précédent, mais avec SRC_X_INC négatif
/// (parcours décroissant, blit "miroir") : d'après Hatari
/// (`Blitter_SourceShift`/`Blitter_SourceFetch`), l'ordre des deux moitiés
/// du tampon 32 bits s'INVERSE dans ce cas — le mot COURANT (nouvellement
/// lu) occupe la moitié HAUTE et le mot PRÉCÉDENT la moitié BASSE (au lieu
/// de l'inverse pour un parcours croissant). On place donc le motif "101"
/// dans le mot COURANT (pas le précédent) pour obtenir le même résultat
/// 0xA000 — une version qui ne tiendrait pas compte de la direction (comme
/// avant cette correction) donnerait 0x0000 ici, puisqu'elle chercherait
/// le motif dans le mauvais mot.
#[test]
fn skew_non_nul_inverse_ordre_de_combinaison_si_src_x_inc_negatif() {
    let mut bl = Blitter::new();
    bl.write(reg::HOP, 2); // source seule
    bl.write(reg::OP, 0x3); // copie
    bl.write(reg::SKEW, 0x83); // FXSR posé (bit 7) + skew=3

    write_word(&mut bl, reg::SRC_X_INC, 0xFFFE); // -2 : parcours décroissant
    write_word(&mut bl, reg::DST_X_INC, 2);
    write_word(&mut bl, reg::X_COUNT, 1);
    write_word(&mut bl, reg::Y_COUNT, 1);
    write_word(&mut bl, reg::ENDMASK_1, 0xFFFF);
    write_word(&mut bl, reg::ENDMASK_2, 0xFFFF);
    write_word(&mut bl, reg::ENDMASK_3, 0xFFFF);

    let mut bus = FlatBus::new();
    // FXSR lit à SRC_ADDR courant (0x1000) puis SRC_ADDR avance de
    // SRC_X_INC (-2) avant la lecture normale du mot 0, qui a donc lieu à
    // 0x1000-2=0x0FFE.
    bus.write16(0x1000, 0x0000); // mot "précédent" (lu par FXSR)
    bus.write16(0x0FFE, 0x0005); // mot "courant" (mot 0) : bits bas = 101
    write_long(&mut bl, reg::SRC_ADDR, 0x1000);
    write_long(&mut bl, reg::DST_ADDR, 0x2000);

    bl.execute(&mut bus);

    assert_eq!(
        bus.read16(0x2000),
        0xA000,
        "parcours décroissant : le motif doit être cherché dans le mot COURANT, pas le précédent"
    );
}

/// Avance d'adresse en fin de ligne pour un blit multi-mots/multi-lignes
/// avec X_INC non nul. Sur le silicium réel (confirmé par Hatari,
/// `Blitter_Step` : le pointeur avance de X_INC entre les mots, mais le
/// DERNIER mot de la ligne avance de Y_INC À LA PLACE de X_INC), Y_INC doit
/// donc déjà tenir compte des (X_COUNT-1) pas de X_INC parcourus dans la
/// ligne. Un bug précédent ajoutait Y_INC seul à l'adresse de DÉBUT de
/// ligne, perdant la contribution de (X_COUNT-1)*X_INC — invisible sur les
/// blits d'un seul mot (curseur souris) mais corrompant tout blit
/// multi-mots (texte/icônes GEM), ce qui correspond exactement aux
/// symptômes observés en direct par l'utilisateur.
#[test]
fn avance_fin_de_ligne_tient_compte_de_x_count_moins_un_fois_x_inc() {
    let mut bl = Blitter::new();
    bl.write(reg::HOP, 2); // source seule
    bl.write(reg::OP, 0x3); // copie
    bl.write(reg::SKEW, 0x00); // pas de décalage/FXSR/NFSR

    write_word(&mut bl, reg::SRC_X_INC, 2);
    write_word(&mut bl, reg::DST_X_INC, 2);
    // Largeur de ligne réelle (mémoire) = 20 octets ; X_COUNT=3 mots
    // parcourus par pas de 2 → Y_INC doit valoir 20 - (3-1)*2 = 16 pour
    // que la ligne suivante démarre au bon endroit.
    write_word(&mut bl, reg::SRC_Y_INC, 16);
    write_word(&mut bl, reg::DST_Y_INC, 16);
    write_word(&mut bl, reg::X_COUNT, 3);
    write_word(&mut bl, reg::Y_COUNT, 2);
    write_word(&mut bl, reg::ENDMASK_1, 0xFFFF);
    write_word(&mut bl, reg::ENDMASK_2, 0xFFFF);
    write_word(&mut bl, reg::ENDMASK_3, 0xFFFF);

    let mut bus = FlatBus::new();
    // Ligne 0 (source à 0x1000, largeur réelle 20 octets)
    bus.write16(0x1000, 0x1111);
    bus.write16(0x1002, 0x2222);
    bus.write16(0x1004, 0x3333);
    // Ligne 1 (source à 0x1000+20=0x1014)
    bus.write16(0x1014, 0x4444);
    bus.write16(0x1016, 0x5555);
    bus.write16(0x1018, 0x6666);
    write_long(&mut bl, reg::SRC_ADDR, 0x1000);
    write_long(&mut bl, reg::DST_ADDR, 0x2000);

    bl.execute(&mut bus);

    assert_eq!(bus.read16(0x2000), 0x1111, "ligne 0, mot 0");
    assert_eq!(bus.read16(0x2002), 0x2222, "ligne 0, mot 1");
    assert_eq!(bus.read16(0x2004), 0x3333, "ligne 0, mot 2");
    assert_eq!(bus.read16(0x2014), 0x4444, "ligne 1 (0x2000+20), mot 0");
    assert_eq!(bus.read16(0x2016), 0x5555, "ligne 1, mot 1");
    assert_eq!(bus.read16(0x2018), 0x6666, "ligne 1, mot 2");
}

/// X_COUNT=0 écrit par MOT complet : le manuel Blitter officiel et Hatari
/// documentent cette valeur comme désignant 65536, mais trois tentatives
/// indépendantes d'implémenter cette règle ont toutes aggravé nettement la
/// corruption observée en pratique (voir le commentaire de
/// `Blitter::write_word`) — revenu à la valeur écrite telle quelle (bornée à
/// 1 mot minimum par `execute`, pas 65536) en attendant de localiser la
/// vraie cause amont.
#[test]
fn x_count_zero_ecrit_par_mot_reste_borne_a_un_seul_mot() {
    let mut bl = Blitter::new();
    bl.write(reg::HOP, 0); // tout à 1, résultat indépendant de la source
    bl.write(reg::OP, 0x3); // copie du résultat HOP (donc 0xFFFF partout)
    write_word(&mut bl, reg::X_COUNT, 0);
    write_word(&mut bl, reg::Y_COUNT, 1);
    write_word(&mut bl, reg::SRC_X_INC, 2);
    write_word(&mut bl, reg::DST_X_INC, 2);
    write_word(&mut bl, reg::ENDMASK_1, 0xFFFF);
    write_word(&mut bl, reg::ENDMASK_2, 0xFFFF);
    write_word(&mut bl, reg::ENDMASK_3, 0xFFFF);
    write_long(&mut bl, reg::SRC_ADDR, 0x1000);
    write_long(&mut bl, reg::DST_ADDR, 0x2000);

    assert_eq!(bl.read(reg::X_COUNT), 0);
    assert_eq!(bl.read(reg::X_COUNT1), 0);

    let mut bus = FlatBus::new();
    bus.write16(0x2002, 0x4242); // second mot : ne doit pas être modifié
    bl.execute(&mut bus);

    assert_eq!(bus.read16(0x2000), 0xFFFF, "premier (et seul) mot traité");
    assert_eq!(bus.read16(0x2002), 0x4242, "pas de second mot (donc pas 65536)");
}

/// Écriture d'un ou deux octets ISOLÉS dans X_COUNT : sur le silicium réel,
/// un accès `.B` isolé à ce registre est ignoré (voir `Blitter::write`) —
/// x_count reste donc à sa valeur précédente (0, jamais touché). `execute`
/// borne ensuite x_count à un minimum de 1 (voir son commentaire : évite un
/// dépassement arithmétique dans le calcul d'avance de fin de ligne) — donc
/// UN SEUL mot doit être traité, ni zéro ni 65536.
#[test]
fn x_count_zero_ecrit_par_octet_isole_ne_declenche_pas_la_conversion() {
    let mut bl = Blitter::new();
    bl.write(reg::HOP, 0);
    bl.write(reg::OP, 0x3);
    bl.write(reg::X_COUNT, 0x00);
    bl.write(reg::X_COUNT1, 0x00);
    write_word(&mut bl, reg::Y_COUNT, 1);
    write_word(&mut bl, reg::SRC_X_INC, 2);
    write_word(&mut bl, reg::DST_X_INC, 2);
    write_word(&mut bl, reg::ENDMASK_1, 0xFFFF);
    write_word(&mut bl, reg::ENDMASK_2, 0xFFFF);
    write_word(&mut bl, reg::ENDMASK_3, 0xFFFF);
    write_long(&mut bl, reg::SRC_ADDR, 0x1000);
    write_long(&mut bl, reg::DST_ADDR, 0x2000);

    let mut bus = FlatBus::new();
    bus.write16(0x2002, 0x4242); // second mot : ne doit pas être modifié
    bl.execute(&mut bus);

    assert_eq!(bus.read16(0x2000), 0xFFFF, "x_count=0 -> borné à 1 seul mot traité");
    assert_eq!(
        bus.read16(0x2002),
        0x4242,
        "un seul mot traité : pas de second mot (donc pas 65536)"
    );
}

/// Reproduit la boucle de relance du mode non-HOG utilisée par TOS
/// (`TAS.B` sur CONTROL en boucle jusqu'à ce que BUSY retombe) : une fois
/// le blit terminé, reposer le bit BUSY SANS avoir réécrit Y_COUNT ne doit
/// PAS redéclencher une exécution complète — le manuel Blitter officiel
/// documente explicitement que "the flag will remain clear... the BLiTTER
/// won't be restarted" tant que Y_COUNT n'a pas été explicitement réarmé.
/// Un bug réel (confirmé par comparaison directe Blitter activé/désactivé
/// sur un cas concret) faisait ré-exécuter tout le blit à chaque tentative
/// de redémarrage, écrivant du contenu erroné depuis des adresses déjà
/// avancées par le tour précédent.
#[test]
fn control_busy_repose_sans_reecrire_y_count_ne_redeclenche_pas() {
    let mut bl = Blitter::new();
    bl.write(reg::HOP, 2); // source seule
    bl.write(reg::OP, 0x3); // copie
    write_word(&mut bl, reg::SRC_X_INC, 2);
    write_word(&mut bl, reg::DST_X_INC, 2);
    write_word(&mut bl, reg::X_COUNT, 1);
    write_word(&mut bl, reg::Y_COUNT, 1);
    write_word(&mut bl, reg::ENDMASK_1, 0xFFFF);
    write_word(&mut bl, reg::ENDMASK_2, 0xFFFF);
    write_word(&mut bl, reg::ENDMASK_3, 0xFFFF);
    write_long(&mut bl, reg::SRC_ADDR, 0x1000);
    write_long(&mut bl, reg::DST_ADDR, 0x2000);

    let mut bus = FlatBus::new();
    bus.write16(0x1000, 0x1234);
    bl.execute(&mut bus);
    assert_eq!(bus.read16(0x2000), 0x1234, "premier blit : copie normale");

    // SRC_ADDR/DST_ADDR ont avancé (Y_INC par défaut = 0, donc inchangés
    // ici, mais le point est de vérifier qu'AUCUNE deuxième copie n'a
    // lieu) : on modifie la source pour détecter un second passage.
    // `bl.write(CONTROL,..)` puis `bl.execute(..)` reproduit ce que fait le
    // board (`AtariSt`) sur écriture réelle de CONTROL : lui seul déclenche
    // `execute()`, `Blitter::write` seul ne le fait pas.
    bus.write16(0x1000, 0x9999);
    bl.write(reg::CONTROL, 0x80); // TAS.B repose BUSY sans réécrire Y_COUNT
    bl.execute(&mut bus);
    assert_eq!(
        bus.read16(0x2000),
        0x1234,
        "reposer BUSY sans réarmer Y_COUNT ne doit PAS redéclencher le blit"
    );
    assert!(!bl.busy(), "BUSY doit rester lisible à 0, pas rester \"collé\" à 1");

    // Réarmement explicite (réécriture de Y_COUNT) : un nouveau
    // déclenchement DOIT alors fonctionner normalement.
    write_word(&mut bl, reg::Y_COUNT, 1);
    bl.write(reg::CONTROL, 0x80);
    bl.execute(&mut bus);
    assert_eq!(
        bus.read16(0x2000),
        0x9999,
        "après réarmement explicite de Y_COUNT, un nouveau blit doit s'exécuter"
    );
}
