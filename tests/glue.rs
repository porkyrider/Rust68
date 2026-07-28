//! Tests unitaires du GLUE (`rust68::peripherals::glue`).

use rust68::peripherals::glue::{Glue, VideoMode};

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
fn vbl_arme_a_la_fin_de_trame_et_la_ligne_boucle() {
    let mut glue = Glue::new(VideoMode::Pal50);
    // 312 lignes complètes : pas encore de VBL (313 lignes/trame en PAL).
    glue.tick(512 * 312);
    assert_eq!(glue.current_line(), 312);
    assert!(!glue.vbl_pending());
    assert_eq!(glue.frame_count(), 0);

    // La 313e ligne complète la trame.
    glue.tick(512);
    assert!(glue.vbl_pending());
    assert!(glue.hbl_pending(), "la fin de trame est aussi une fin de ligne (HBL)");
    assert_eq!(glue.current_line(), 0, "la ligne boucle à 0 en début de trame suivante");
    assert_eq!(glue.frame_count(), 1);

    glue.ack_vbl();
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
