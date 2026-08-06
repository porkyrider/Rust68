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
    // (`line` revenant à 0) — tout le reste du blanking s'écoule ENSUITE,
    // avant que la ligne visible 0 de la trame suivante ne soit affichée.
    // Confondre les deux ferait rendre cette ligne 0 dans le même souffle
    // que l'armement de VBL, sans laisser au logiciel la moindre chance de
    // prendre l'interruption avant qu'elle ne soit déjà consommée.
    //
    // Numérotation absolue Hatari (`VIDEO_START_HBL_50HZ`=63,
    // `VIDEO_END_HBL_50HZ`=263) : un vrai blanking HAUT de 63 lignes
    // précède la première ligne affichée (nécessaire pour la suppression
    // de bordure haute STE, voir `peripherals::atari_st::shifter`), donc
    // les 200 lignes affichées vont de la ligne absolue 63 à 262 inclus,
    // pas 0 à 199 — seule la RÉPARTITION du blanking change (63 avant +
    // 50 après au lieu de 0 avant + 113 après), pas son total (113).
    let mut glue = Glue::new(VideoMode::Pal50);
    assert_eq!(glue.display_line(), None, "encore dans le blanking haut au tout début");

    // 262 lignes complètes (dernière ligne affichée, absolue 261, PAL) :
    // pas encore de VBL.
    glue.tick(512 * 262);
    assert_eq!(glue.current_line(), 262);
    assert_eq!(glue.display_line(), Some(199), "dernière des 200 lignes affichées");
    assert!(!glue.vbl_pending());
    assert_eq!(glue.frame_count(), 0);

    // La ligne absolue 263 (transition vers le blanking vertical bas)
    // déclenche VBL.
    glue.tick(512);
    assert!(glue.vbl_pending());
    assert!(glue.hbl_pending(), "toute fin de ligne est aussi une fin de ligne (HBL)");
    assert_eq!(glue.current_line(), 263);
    assert_eq!(glue.display_line(), None, "entrée dans le blanking bas");
    assert_eq!(glue.frame_count(), 0, "la trame ne bascule qu'au bouclage complet (313 lignes)");

    glue.ack_vbl();
    assert!(!glue.vbl_pending());

    // Le reste du blanking bas (50 lignes) fait boucler la ligne et avancer
    // frame_count(), sans réarmer VBL (déjà consommé plus haut) — puis le
    // blanking haut de la trame suivante (63 lignes) doit s'écouler avant
    // qu'une ligne affichée ne réapparaisse.
    glue.tick(512 * 50);
    assert_eq!(glue.current_line(), 0, "la ligne boucle à 0 en début de trame suivante");
    assert_eq!(glue.frame_count(), 1);
    assert!(!glue.vbl_pending());
    assert_eq!(glue.display_line(), None, "blanking haut de la nouvelle trame");

    glue.tick(512 * 63);
    assert_eq!(glue.display_line(), Some(0), "première ligne affichée de la nouvelle trame");
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

#[test]
fn write_sync_vers_60hz_tot_dans_le_blanking_haut_supprime_la_bordure_haute() {
    // $FF820A, bit1=0 = sélection 60Hz. Survenant encore dans le blanking
    // haut nominal (avant la ligne 63 PAL) et à un cycle assez précoce
    // dans la ligne, ce switch tire le début de la fenêtre affichée à la
    // position de départ NOMINALE du mode 60Hz (34) — révélant les lignes
    // 34..63, normalement en blanking (voir Hatari, `video.c`,
    // `Video_Update_Glue_State`).
    let mut glue = Glue::new(VideoMode::Pal50);
    assert_eq!(glue.display_line(), None, "encore en bordure avant l'écriture");

    glue.write_sync(0x00); // bit1=0 : 60Hz
    assert_eq!(glue.read_sync(), 0x00);

    // Les lignes 34..63 sont maintenant affichées (avant : bordure).
    glue.tick(512 * 34);
    assert_eq!(
        glue.display_line(),
        Some(0),
        "ligne 34 : premier pixel de la bordure haute supprimée"
    );
    glue.tick(512 * 29); // atteint la ligne 63, position nominale normale
    assert_eq!(
        glue.display_line(),
        Some(29),
        "ligne 63 : jonction avec la fenêtre nominale (63-34=29 lignes révélées)"
    );
}

#[test]
fn write_sync_vers_60hz_en_fin_d_affichage_supprime_la_bordure_basse() {
    // Même registre, mais le switch survient cette fois sur l'avant-
    // dernière/dernière ligne affichée nominale (262 en PAL, juste avant
    // la transition VBL à 263) : étend la fenêtre de
    // `VIDEO_HEIGHT_BOTTOM_50HZ` (47) lignes supplémentaires au lieu de
    // déplacer son début.
    let mut glue = Glue::new(VideoMode::Pal50);
    glue.tick(512 * 262); // dernière ligne affichée nominale (262, PAL)
    assert_eq!(glue.display_line(), Some(199));

    glue.write_sync(0x00); // bit1=0 : 60Hz, tôt dans cette ligne (cycles_in_line=0)

    // Sans la suppression, VBL se déclencherait à la ligne 263 (juste
    // après) : elle reste ici en attente jusqu'à la ligne 310 (263+47).
    glue.tick(512); // ligne 263 : bordure basse supprimée, doit rester affichée
    assert_eq!(glue.display_line(), Some(200), "bordure basse supprimée : encore affichée");
    assert!(!glue.vbl_pending(), "VBL différé tant que la bordure basse reste supprimée");

    glue.tick(512 * 46); // jusqu'à la ligne 309 (dernière ligne étendue)
    assert_eq!(glue.display_line(), Some(246));
    assert!(!glue.vbl_pending());

    glue.tick(512); // ligne 310 : fin de la fenêtre étendue, VBL désormais armé
    assert!(glue.vbl_pending());
    assert_eq!(glue.display_line(), None);
}

#[test]
fn write_sync_vers_50hz_ou_hors_fenetre_de_cycle_n_a_aucun_effet() {
    // Un switch VERS 50Hz (bit1=1) ne doit jamais étendre la fenêtre — ni
    // un switch vers 60Hz survenant trop tard dans la ligne (au-delà du
    // seuil de "gating", voir la doc de `Glue::write_sync`).
    let mut glue = Glue::new(VideoMode::Pal50);
    glue.write_sync(0x02); // bit1=1 : 50Hz, aucun effet possible
    glue.tick(512 * 34);
    assert_eq!(glue.display_line(), None, "50Hz : la bordure haute n'a pas dû être supprimée");

    let mut glue2 = Glue::new(VideoMode::Pal50);
    glue2.tick(510); // au-delà du seuil de cycle (504), toujours ligne 0 (< 512)
    assert_eq!(glue2.current_line(), 0);
    glue2.write_sync(0x00);
    glue2.tick(512 * 34 - 510);
    assert_eq!(
        glue2.display_line(),
        None,
        "switch trop tardif dans la ligne : aucun effet"
    );
}
