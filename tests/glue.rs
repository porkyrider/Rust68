#![cfg(feature = "atari-st")]
//! Tests unitaires du GLUE (`rust68::peripherals::atari_st::glue`).

use rust68::peripherals::atari_st::glue::{Glue, VideoMode};

#[test]
fn pas_de_hbl_avant_la_fin_de_ligne() {
    let mut glue = Glue::new(VideoMode::Pal50);
    glue.tick(511); // 1 cycle avant la fin de ligne (512 cycles PAL)
    assert!(!glue.hbl_pending());
    assert_eq!(glue.current_line(), 0);
}

#[test]
fn hbl_arme_a_la_fin_de_chaque_ligne() {
    let mut glue = Glue::new(VideoMode::Pal50);
    glue.tick(512);
    assert!(glue.hbl_pending());
    assert_eq!(glue.current_line(), 1);

    glue.ack_hbl();
    assert!(!glue.hbl_pending());

    glue.tick(512);
    assert!(glue.hbl_pending());
    assert_eq!(glue.current_line(), 2);
}

#[test]
fn vbl_arme_a_la_transition_visible_blanking_pas_au_bouclage() {
    // Sur silicium réel, VBL survient au DÉBUT du blanking vertical (juste
    // après la dernière ligne visible), pas au bouclage complet de la trame
    // (`line` revenant à 0) — tout le reste du blanking (~113 lignes en
    // PAL) s'écoule ENSUITE, avant que la ligne 0 de la trame suivante ne
    // soit affichée. Confondre les deux ferait rendre cette ligne 0 dans le
    // même souffle que l'armement de VBL, sans laisser au logiciel la
    // moindre chance de prendre l'interruption avant qu'elle ne soit déjà
    // consommée.
    let mut glue = Glue::new(VideoMode::Pal50);
    // 199 lignes complètes (dernière ligne visible, 0..199 en PAL) : pas
    // encore de VBL.
    glue.tick(512 * 199);
    assert_eq!(glue.current_line(), 199);
    assert!(!glue.vbl_pending());
    assert_eq!(glue.frame_count(), 0);

    // La 200e ligne (transition vers le blanking vertical) déclenche VBL.
    glue.tick(512);
    assert!(glue.vbl_pending());
    assert!(glue.hbl_pending(), "toute fin de ligne est aussi une fin de ligne (HBL)");
    assert_eq!(glue.current_line(), 200);
    assert_eq!(glue.frame_count(), 0, "la trame ne bascule qu'au bouclage complet (313 lignes)");

    glue.ack_vbl();
    assert!(!glue.vbl_pending());

    // Le reste du blanking (113 lignes) fait boucler la ligne et avancer
    // frame_count(), sans réarmer VBL (déjà consommé plus haut).
    glue.tick(512 * 113);
    assert_eq!(glue.current_line(), 0, "la ligne boucle à 0 en début de trame suivante");
    assert_eq!(glue.frame_count(), 1);
    assert!(!glue.vbl_pending());
}

#[test]
fn ntsc_utilise_des_constantes_differentes() {
    let mut pal = Glue::new(VideoMode::Pal50);
    let mut ntsc = Glue::new(VideoMode::Ntsc60);
    pal.tick(508);
    ntsc.tick(508);
    assert!(!pal.hbl_pending(), "508 cycles < 512 (PAL) : pas encore de HBL");
    assert!(ntsc.hbl_pending(), "508 cycles = exactement une ligne NTSC");
}

#[test]
fn plusieurs_lignes_en_un_seul_tick() {
    let mut glue = Glue::new(VideoMode::Pal50);
    glue.tick(512 * 5 + 100); // 5 lignes complètes + reliquat
    assert_eq!(glue.current_line(), 5);
    assert!(glue.hbl_pending());
}
