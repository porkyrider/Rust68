#![cfg(feature = "atari-st")]
//! Tests unitaires du Shifter (`rust68::peripherals::atari_st::shifter`).

use rust68::peripherals::atari_st::shifter::{Resolution, Shifter, addr, border};

#[test]
fn adresse_de_base_video_haut_milieu() {
    let mut sh = Shifter::new();
    sh.write(addr::VIDEO_BASE_HIGH, 0x12);
    sh.write(addr::VIDEO_BASE_MID, 0x34);
    assert_eq!(sh.read(addr::VIDEO_BASE_HIGH), 0x12);
    assert_eq!(sh.read(addr::VIDEO_BASE_MID), 0x34);

    sh.start_frame();
    assert_eq!(sh.read(addr::VIDEO_COUNTER_HIGH), 0x12);
    assert_eq!(sh.read(addr::VIDEO_COUNTER_MID), 0x34);
    assert_eq!(sh.read(addr::VIDEO_COUNTER_LOW), 0x00);
}

#[test]
fn resolution_lue_et_ecrite() {
    let mut sh = Shifter::new();
    assert_eq!(sh.resolution(), Resolution::Low);
    sh.write(addr::RESOLUTION, 0b01);
    assert_eq!(sh.resolution(), Resolution::Medium);
    assert_eq!(sh.read(addr::RESOLUTION), 0b01);
    sh.write(addr::RESOLUTION, 0b10);
    assert_eq!(sh.resolution(), Resolution::High);
}

#[test]
fn palette_round_trip() {
    // Écriture par mot (`.W`/`.L`, chemin normal du board — voir
    // `write_palette_word`) : les deux octets sont pris tels quels, pas de
    // duplication (contrairement à `write`, réservé aux accès `.B` isolés,
    // voir `octet_isole_duplique_dans_les_deux_moities`).
    let mut sh = Shifter::new();
    let addr_color3 = addr::PALETTE_BASE + 3 * 2;
    sh.write_palette_word(addr_color3, 0x0777); // R=7, G=7, B=7
    assert_eq!(sh.read(addr_color3), 0x07);
    assert_eq!(sh.read(addr_color3 + 1), 0x77);
}

#[test]
fn octet_isole_duplique_dans_les_deux_moities() {
    // Comportement matériel réel documenté (Hatari, `Video_ColorReg_WriteWord`) :
    // un accès `.B` isolé sur un registre de palette duplique l'octet écrit
    // dans les DEUX moitiés du mot avant masquage — l'autre moitié n'est
    // pas préservée. Reproduit l'exemple donné en commentaire côté Hatari :
    //   move.w #0,$ff8240      -> couleur 0 = $000
    //   move.b #7,$ff8240      -> couleur 0 = $707
    //   move.b #$55,$ff8241    -> couleur 0 = $555
    let mut sh = Shifter::new();
    sh.write_palette_word(addr::PALETTE_BASE, 0x0000);
    assert_eq!(sh.palette_raw()[0], 0x000);

    sh.write(addr::PALETTE_BASE, 0x07); // .B sur l'octet haut
    assert_eq!(sh.palette_raw()[0], 0x707, ".B haut duplique 0x07 dans les 2 octets");

    sh.write(addr::PALETTE_BASE + 1, 0x55); // .B sur l'octet bas
    assert_eq!(sh.palette_raw()[0], 0x555, ".B bas duplique 0x55 dans les 2 octets");
}

#[test]
fn rendu_basse_resolution_un_groupe_16_pixels() {
    let mut sh = Shifter::new();
    // Palette : couleur 0 = noir, couleur 1 = blanc.
    sh.write_palette_word(addr::PALETTE_BASE, 0x0000);
    let c1 = addr::PALETTE_BASE + 1 * 2;
    sh.write_palette_word(c1, 0x0777);
    sh.write(addr::RESOLUTION, 0b00); // basse résolution, 4 plans

    // Plan 0 = 0x8000 (bit15 posé), plans 1-3 = 0 : le pixel 0 doit avoir
    // l'index de couleur 1 (bit0 du plan0 posé), les 15 autres l'index 0.
    // Ligne complète requise (160 octets en basse résolution) ; seul le
    // premier groupe de 16 pixels est non nul.
    let mut ram = vec![0u8; 160];
    ram[0] = 0x80;
    let pixels = sh.render_scanline(&ram);

    assert_eq!(pixels.len(), 320, "basse résolution : 320 pixels par ligne");
    assert_eq!(pixels[0], (255, 255, 255), "pixel 0 -> couleur 1 (blanc)");
    for p in &pixels[1..16] {
        assert_eq!(*p, (0, 0, 0), "pixels 1-15 -> couleur 0 (noir)");
    }
}

#[test]
fn palette_ste_reordonne_le_bit_de_precision_fine_avant_conversion_rgb() {
    // Bug réel : le bit 3 d'un nibble de palette STE n'est PAS le bit de
    // poids fort d'une valeur 4 bits normale — c'est un bit de précision
    // fine ajouté EN BAS par le matériel, les bits 2-0 restant le nibble
    // ST d'origine (mêmes positions bus). Vraie intensité = (bits2-0<<1)|bit3,
    // confirmé contre Hatari (`conv_st.c`, `ConvST_SetupRGBTable`).
    let mut sh = Shifter::new();
    sh.set_ste_palette(true);
    sh.write(addr::RESOLUTION, 0b00);

    // 0x0777 : nibble bas 3 bits partout à 7, bit de précision fine à 0 —
    // exactement ce qu'écrirait un jeu STE voulant une composante "au
    // maximum façon ST 3 bits". Avant le correctif, lu comme la valeur 4
    // bits brute 7 -> RGB (119,119,119) au lieu du (238,238,238) réel (un
    // cran sous le vrai blanc 15/15) — un assombrissement de moitié.
    let c1 = addr::PALETTE_BASE + 1 * 2;
    sh.write_palette_word(c1, 0x0777);
    let mut ram = vec![0u8; 160];
    ram[0] = 0x80; // pixel 0 -> index de couleur 1 (plan 0 seul)
    let pixels = sh.render_scanline(&ram);
    assert_eq!(
        pixels[0],
        (238, 238, 238),
        "0x777 en palette STE : (bits2-0=7,bit3=0) -> intensité réelle 14/15, pas 7/15"
    );

    // 0x0FFF : nibble complet à 1111 (bits2-0=7 ET bit de précision fine=1)
    // -> intensité réelle 15/15, vrai blanc. `start_frame()` recharge le
    // compteur vidéo (sinon la ligne suivante lirait au-delà des 160
    // octets de `ram`, hors limites — voir `render_scanline`).
    sh.write_palette_word(c1, 0x0FFF);
    sh.start_frame();
    let pixels = sh.render_scanline(&ram);
    assert_eq!(pixels[0], (255, 255, 255), "0xFFF : intensité réelle 15/15");

    // 0x0888 : SEUL le bit de précision fine est posé (bits2-0=0) ->
    // intensité réelle 1/15, presque noir — pas 8/15 (gris moyen) comme le
    // lirait une interprétation naïve du nibble.
    sh.write_palette_word(c1, 0x0888);
    sh.start_frame();
    let pixels = sh.render_scanline(&ram);
    assert_eq!(pixels[0], (17, 17, 17), "0x888 : intensité réelle 1/15, presque noir");
}

#[test]
fn rendu_moyenne_resolution_deux_plans() {
    let mut sh = Shifter::new();
    for (i, val) in [(0u32, 0x000), (1, 0x700), (2, 0x070), (3, 0x777)] {
        let a = addr::PALETTE_BASE + i * 2;
        sh.write_palette_word(a, val as u16);
    }
    sh.write(addr::RESOLUTION, 0b01); // moyenne résolution, 2 plans

    // Plan0 = 0x8000 (pixel0 posé), Plan1 = 0x4000 (pixel1 posé). Ligne
    // complète requise (160 octets en moyenne résolution).
    let mut ram = vec![0u8; 160];
    ram[0..4].copy_from_slice(&[0x80, 0x00, 0x40, 0x00]);
    let pixels = sh.render_scanline(&ram);

    assert_eq!(pixels.len(), 640, "moyenne résolution : 640 pixels par ligne");
    assert_eq!(pixels[0], (255, 0, 0), "pixel0 : plan0 seul -> couleur 1 (rouge)");
    assert_eq!(pixels[1], (0, 255, 0), "pixel1 : plan1 seul -> couleur 2 (vert)");
    assert_eq!(pixels[2], (0, 0, 0), "pixel2 : aucun plan -> couleur 0 (noir)");
}

#[test]
fn rendu_haute_resolution_monochrome() {
    let mut sh = Shifter::new();
    sh.write(addr::RESOLUTION, 0b10);
    let mut ram = vec![0u8; 80]; // 640/8 = 80 octets/ligne en haute résolution
    ram[0] = 0b1000_0000;
    let pixels = sh.render_scanline(&ram);
    assert_eq!(pixels.len(), 640, "haute résolution : 640 pixels par ligne");
    assert_eq!(pixels[0], (0, 0, 0), "bit posé -> noir");
    assert_eq!(pixels[1], (255, 255, 255), "bit clair -> blanc");
}

#[test]
fn compteur_video_avance_du_nombre_d_octets_consommes() {
    let mut sh = Shifter::new();
    sh.write(addr::RESOLUTION, 0b00); // basse résolution : 160 octets/ligne
    let ram = vec![0u8; 1000];
    sh.render_scanline(&ram);
    assert_eq!(sh.read(addr::VIDEO_COUNTER_HIGH), 0);
    assert_eq!(sh.read(addr::VIDEO_COUNTER_MID), 0);
    assert_eq!(sh.read(addr::VIDEO_COUNTER_LOW), 160);
}

#[test]
fn ram_insuffisante_renvoie_une_ligne_noire_mais_avance_quand_meme() {
    // Le compteur d'adresse du Shifter est un simple générateur, indépendant
    // de la présence physique de RAM à cette adresse (voir la doc de
    // `render_scanline`) : seul le CONTENU affiché dépend de la RAM
    // disponible, pas l'avancement du compteur.
    let mut sh = Shifter::new();
    sh.write(addr::RESOLUTION, 0b00);
    let ram = vec![0u8; 10]; // bien moins que 160 octets requis
    let pixels = sh.render_scanline(&ram);
    assert!(pixels.iter().all(|&p| p == (0, 0, 0)));
    assert_eq!(sh.read(addr::VIDEO_COUNTER_LOW), 160, "le compteur avance quand même");
}

// --- Défilement fin STE (`write_hscroll`/`write_line_width`) -------------
//
// Basse résolution : 4 plans, 8 octets (4 mots)/groupe de 16 pixels, 160
// octets/ligne (20 groupes). Tous les tests ci-dessous n'utilisent que le
// plan 0 (les 3 autres restent à zéro) : le flux de bits source du plan 0
// EST alors directement l'index de couleur (0 ou 1), ce qui simplifie la
// vérification pixel par pixel.

#[test]
fn defilement_avec_prechargement_lit_un_groupe_de_plus_et_decale() {
    // $FF8265 (avec préchargement) : un groupe de 16 pixels EN PLUS est lu
    // (168 octets au lieu de 160) pour remplir le bord droit sans perte —
    // voir la doc de `Shifter::write_hscroll`. Plan 0 : groupe 0 = tout à
    // 1, groupe "supplémentaire" (index 20) = tout à 0. Avec un décalage
    // de 1 bit, le pixel de sortie x lit le bit source x+1 : les pixels
    // 0..14 restent dans le groupe 0 (tout à 1, couleur 1), le pixel 15
    // atteint le premier bit du groupe supplémentaire (tout à 0, couleur 0).
    let mut sh = Shifter::new();
    sh.write(addr::RESOLUTION, 0b00);
    sh.write_palette_word(addr::PALETTE_BASE, 0x0000); // couleur 0 = noir
    sh.write_palette_word(addr::PALETTE_BASE + 1 * 2, 0x0777); // couleur 1 = blanc
    sh.write_hscroll(1, true, true); // apply_now=true : géré directement ici, pas de gating à tester

    let mut ram = vec![0u8; 160 + 8]; // 160 (ligne normale) + 8 (groupe supplémentaire)
    ram[0] = 0xFF;
    ram[1] = 0xFF; // plan 0, groupe 0 = 0xFFFF
    // groupe supplémentaire (octets 160-167) : plan 0 déjà à 0 par défaut.
    let pixels = sh.render_scanline(&ram);

    assert_eq!(pixels.len(), 320);
    for (x, &p) in pixels.iter().take(15).enumerate() {
        assert_eq!(p, (255, 255, 255), "pixel {x} : dans le groupe 0 (tout à 1) décalé -> couleur 1");
    }
    assert_eq!(pixels[15], (0, 0, 0), "pixel 15 : atteint le groupe supplémentaire (tout à 0) -> couleur 0");

    // Le groupe supplémentaire est réellement CONSOMMÉ sur le compteur
    // d'adresse (168 octets, pas 160).
    assert_eq!(sh.read(addr::VIDEO_COUNTER_LOW), 168);
}

#[test]
fn defilement_sans_prechargement_noircit_les_16_premiers_pixels() {
    // $FF8264 (sans préchargement) : aucun octet en plus n'est lu — les 16
    // premiers pixels de sortie sortent noirs (registre à décalage matériel
    // pas encore chargé), le reste vient du tampon NORMAL (160 octets),
    // décalé. Avec un décalage de 1 bit : pixel 16 lit le bit source 1 du
    // groupe 0 (tout à 1 ici) -> couleur 1.
    let mut sh = Shifter::new();
    sh.write(addr::RESOLUTION, 0b00);
    sh.write_palette_word(addr::PALETTE_BASE, 0x0000);
    sh.write_palette_word(addr::PALETTE_BASE + 1 * 2, 0x0777);
    sh.write_hscroll(1, false, true); // false = $FF8264, sans préchargement

    let mut ram = vec![0u8; 160]; // AUCUN octet en plus nécessaire
    ram[0] = 0xFF;
    ram[1] = 0xFF; // plan 0, groupe 0 = 0xFFFF
    let pixels = sh.render_scanline(&ram);

    for (x, &p) in pixels.iter().take(16).enumerate() {
        assert_eq!(p, (0, 0, 0), "pixel {x} : bord noirci (registre pas encore chargé)");
    }
    assert_eq!(pixels[16], (255, 255, 255), "pixel 16 : premier pixel réel, décalé depuis le groupe 0");

    // Aucun octet supplémentaire consommé (160, pas 168).
    assert_eq!(sh.read(addr::VIDEO_COUNTER_LOW), 160);
}

#[test]
fn decalage_nul_reproduit_exactement_le_rendu_historique() {
    // Garde-fou de non-régression : `h_scroll_count == 0` (l'état par
    // défaut, jamais modifié par un jeu qui n'utilise pas le défilement)
    // DOIT produire un résultat identique à avant ce chantier, quel que
    // soit le registre utilisé pour le poser à zéro.
    let mut sh = Shifter::new();
    sh.write(addr::RESOLUTION, 0b00);
    sh.write_palette_word(addr::PALETTE_BASE, 0x0000);
    sh.write_palette_word(addr::PALETTE_BASE + 1 * 2, 0x0777);
    sh.write_hscroll(0, true, true); // décalage nul, via $FF8265

    let mut ram = vec![0u8; 160];
    ram[0] = 0x80; // pixel 0 -> couleur 1 (voir `rendu_basse_resolution_un_groupe_16_pixels`)
    let pixels = sh.render_scanline(&ram);

    assert_eq!(pixels[0], (255, 255, 255));
    for p in &pixels[1..16] {
        assert_eq!(*p, (0, 0, 0));
    }
    assert_eq!(sh.read(addr::VIDEO_COUNTER_LOW), 160, "aucun octet en plus quand le décalage est nul");
}

#[test]
fn largeur_de_ligne_ajoute_des_octets_a_l_avance_d_adresse() {
    let mut sh = Shifter::new();
    sh.write(addr::RESOLUTION, 0b00);
    sh.write_line_width(3, true); // +3 mots = +6 octets par ligne

    let ram = vec![0u8; 200];
    sh.render_scanline(&ram);
    assert_eq!(
        sh.read(addr::VIDEO_COUNTER_LOW),
        160 + 6,
        "avance = octets de ligne normaux + LineWidth*2"
    );
}

#[test]
fn defilement_et_largeur_de_ligne_se_cumulent_sur_l_avance_d_adresse() {
    let mut sh = Shifter::new();
    sh.write(addr::RESOLUTION, 0b00);
    sh.write_hscroll(1, true, true); // +8 octets (groupe supplémentaire préchargé)
    sh.write_line_width(3, true); // +6 octets

    let ram = vec![0u8; 200];
    sh.render_scanline(&ram);
    assert_eq!(sh.read(addr::VIDEO_COUNTER_LOW), 160 + 8 + 6);
}

#[test]
fn ecriture_en_attente_ne_s_applique_qu_a_la_ligne_suivante() {
    // `apply_now=false` : la valeur ne doit PAS affecter la ligne en cours
    // de rendu, seulement la suivante — reproduit le mécanisme `New*` de
    // Hatari (le "gating" cycle-exact lui-même, calculé par l'appelant, est
    // testé côté board dans tests/atari_st.rs).
    let mut sh = Shifter::new();
    sh.write(addr::RESOLUTION, 0b00);
    sh.write_palette_word(addr::PALETTE_BASE, 0x0000);
    sh.write_palette_word(addr::PALETTE_BASE + 1 * 2, 0x0777);

    let mut ram = vec![0u8; 160 + 8];
    ram[0] = 0xFF;
    ram[1] = 0xFF;

    sh.write_hscroll(1, true, false); // en attente, PAS appliqué à cette ligne
    assert_eq!(sh.h_scroll_count(), 0, "valeur effective inchangée avant le commit");
    let pixels_ligne_1 = sh.render_scanline(&ram);
    assert_eq!(pixels_ligne_1[0], (255, 255, 255), "ligne 1 : toujours décalage nul (pixel 0 = groupe 0 tout à 1)");
    assert_eq!(sh.read(addr::VIDEO_COUNTER_LOW), 160, "ligne 1 : aucun octet en plus, décalage pas encore effectif");

    // Le commit a eu lieu à la fin du rendu de la ligne 1 : la ligne 2 voit
    // maintenant le décalage.
    assert_eq!(sh.h_scroll_count(), 1, "valeur effective mise à jour après le commit");
    sh.write(addr::VIDEO_COUNTER_LOW, 0); // reprend au même endroit en RAM pour comparer
    let pixels_ligne_2 = sh.render_scanline(&ram);
    assert_eq!(pixels_ligne_2[15], (0, 0, 0), "ligne 2 : décalage désormais effectif (voir test préchargement)");
}

// --- Bordure horizontale / overscan STE (`write_resolution`/`write_sync`,
// voir le module `border`) ------------------------------------------------

#[test]
fn write_resolution_arme_puis_confirme_left_off_2_ste() {
    // Hi-res très tôt (cycle <= 4) arme la tentative ; un retour en
    // basse/moyenne résolution PILE au cycle 4 la confirme en
    // `LEFT_OFF_2_STE` (voir la doc de `write_resolution`).
    let mut sh = Shifter::new();
    sh.write_resolution(0b10, 4);
    assert_eq!(
        sh.border_mask() & border::LEFT_OFF_2_STE,
        0,
        "tentative armée mais pas encore confirmée : aucun effet de rendu"
    );
    sh.write_resolution(0b00, 4);
    assert_eq!(sh.border_mask() & border::LEFT_OFF_2_STE, border::LEFT_OFF_2_STE);
    assert_eq!(sh.resolution(), Resolution::Low, "la résolution finale reste bien celle demandée");
}

#[test]
fn write_resolution_hors_fenetre_n_arme_rien() {
    let mut sh = Shifter::new();
    sh.write_resolution(0b10, 10); // cycle 10 > HDE_On_Hi (4) : trop tard
    sh.write_resolution(0b00, 4);
    assert_eq!(sh.border_mask(), 0);
}

#[test]
fn write_resolution_retour_hors_cycle_4_annule_la_tentative() {
    let mut sh = Shifter::new();
    sh.write_resolution(0b10, 2); // arme
    sh.write_resolution(0b00, 6); // retour, mais pas pile au cycle 4 : annulé
    assert_eq!(sh.border_mask(), 0);
}

#[test]
fn left_off_2_ste_revele_20_octets_de_bordure_gauche() {
    // Moyenne résolution (2 plans, 4 octets/groupe de 16 pixels) plutôt que
    // basse résolution : BYTES_LEFT_OFF_2_STE (20 octets) ne tombe pas sur
    // une frontière de groupe en basse résolution (8 octets/groupe), ce qui
    // compliquerait la vérification pixel par pixel sans rien ajouter à ce
    // que ce test cherche à couvrir (le mécanisme de détection + le
    // placement du segment, pas l'arrondi de groupe).
    let mut sh = Shifter::new();
    sh.write_palette_word(addr::PALETTE_BASE, 0x0000); // noir
    sh.write_palette_word(addr::PALETTE_BASE + 2, 0x0777); // blanc (index 1)
    sh.write(addr::RESOLUTION, 0b01); // moyenne résolution
    sh.write(addr::VIDEO_COUNTER_LOW, 100);

    sh.write_resolution(0b10, 4);
    sh.write_resolution(0b01, 4);
    assert_eq!(sh.border_mask() & border::LEFT_OFF_2_STE, border::LEFT_OFF_2_STE);

    let mut ram = vec![0u8; 260];
    ram[80] = 0x80; // bordure gauche, pixel 0 -> couleur 1 (compteur - 20 = 80)
    ram[100] = 0x80; // segment central (compteur nominal = 100), pixel 0 -> couleur 1
    let pixels = sh.render_scanline(&ram);

    assert_eq!(pixels.len(), 720, "80 px de bordure gauche (20 octets, 2 plans) + 640 px nominaux");
    assert_eq!(pixels[0], (255, 255, 255), "premier pixel de bordure gauche révélée");
    for p in &pixels[1..80] {
        assert_eq!(*p, (0, 0, 0), "reste de la bordure gauche : noir (RAM à zéro)");
    }
    assert_eq!(pixels[80], (255, 255, 255), "premier pixel du segment central (compteur nominal)");

    // Le compteur avance comme si seule une extension DROITE avait eu lieu
    // (ici aucune) : +160 (largeur nominale moyenne résolution), PAS +180
    // (qui compterait aussi les 20 octets de bordure gauche "empruntés"
    // avant la position nominale) — voir la doc de `render_scanline`.
    assert_eq!(sh.read(addr::VIDEO_COUNTER_MID), 1);
    assert_eq!(sh.read(addr::VIDEO_COUNTER_LOW), 4); // 100+160 = 260 = 0x104
    assert_eq!(sh.border_mask(), 0, "masque remis à zéro après le rendu de la ligne");
}

#[test]
fn right_off_revele_44_octets_de_bordure_droite() {
    let mut sh = Shifter::new();
    sh.write_palette_word(addr::PALETTE_BASE, 0x0000);
    sh.write_palette_word(addr::PALETTE_BASE + 2, 0x0777);
    sh.write(addr::RESOLUTION, 0b00); // basse résolution, 160 octets/ligne nominal

    // RIGHT_OFF : switch $FF820A vers 60Hz dans la fenêtre ]372,376].
    sh.write_sync(0x00, 374);
    assert_eq!(sh.border_mask() & border::RIGHT_OFF, border::RIGHT_OFF);

    let mut ram = vec![0u8; 160 + 44];
    ram[160] = 0x80; // premier octet du segment droit -> pixel 0 du segment -> couleur 1
    let pixels = sh.render_scanline(&ram);

    assert_eq!(pixels.len(), 320 + 88, "320 px nominaux + 88 px de bordure droite (44 octets, 4 plans)");
    assert_eq!(pixels[320], (255, 255, 255), "premier pixel de la bordure droite révélée");
    assert_eq!(sh.read(addr::VIDEO_COUNTER_LOW), 204, "160 (nominal) + 44 (RIGHT_OFF)");
}

#[test]
fn right_off_hors_fenetre_n_a_aucun_effet() {
    let mut sh = Shifter::new();
    sh.write(addr::RESOLUTION, 0b00);
    sh.write_sync(0x00, 100); // bien avant la fenêtre ]372,376]
    assert_eq!(sh.border_mask(), 0);
    sh.write_sync(0x00, 400); // bien après
    assert_eq!(sh.border_mask(), 0);
}

#[test]
fn nudge_60hz_precoce_ajoute_2_octets_a_gauche() {
    // Le nudge `RIGHT_MINUS_2` (-2, saturé à 0 sans `RIGHT_OFF` actif en
    // même temps) n'a pas d'effet visible ici — limitation documentée dans
    // la doc de module (pas de raccourcissement du segment central).
    let mut sh = Shifter::new();
    sh.write(addr::RESOLUTION, 0b00);
    sh.write_sync(0x00, 10); // cycle 10 <= 36 (Preload_Start_Low_60)
    assert_eq!(sh.border_mask() & border::LEFT_PLUS_2, border::LEFT_PLUS_2);
    assert_eq!(sh.border_mask() & border::RIGHT_MINUS_2, border::RIGHT_MINUS_2);

    sh.write(addr::VIDEO_COUNTER_LOW, 50);
    let ram = vec![0u8; 250];
    let pixels = sh.render_scanline(&ram);
    assert_eq!(pixels.len(), 320 + 4, "320 px nominaux + 4 px de nudge gauche (2 octets, 4 plans)");
    assert_eq!(sh.read(addr::VIDEO_COUNTER_LOW), 210, "50 + 160 : le nudge gauche n'avance pas le compteur");
}

#[test]
fn retour_50hz_annule_les_effets_horizontaux_en_cours() {
    let mut sh = Shifter::new();
    sh.write_sync(0x00, 10); // arme les nudges
    assert_ne!(sh.border_mask(), 0);
    sh.write_sync(0x02, 20); // bit1=1 : retour 50Hz
    assert_eq!(sh.border_mask(), 0);
}

// --- Fenêtres d'annulation par mécanisme (façon Hatari,
// `Video_Update_Glue_State`), voir la doc de `write_sync` -----------------

#[test]
fn retour_50hz_apres_52_cycles_n_annule_plus_le_nudge_gauche_mais_annule_le_droit() {
    // `LEFT_PLUS_2` n'est annulable que jusqu'au cycle 52 (`HDE_On_Low_60`),
    // `RIGHT_MINUS_2` jusqu'au cycle 376 (`HDE_Off_Low_50`) — deux fenêtres
    // DISTINCTES, pas une annulation globale.
    let mut sh = Shifter::new();
    sh.write_sync(0x00, 10); // arme les deux nudges
    sh.write_sync(0x02, 60); // retour 50Hz à cycle 60 : > 52, <= 376
    assert_eq!(
        sh.border_mask() & border::LEFT_PLUS_2,
        border::LEFT_PLUS_2,
        "nudge gauche déjà verrouillé (cycle 60 > 52) : pas annulé"
    );
    assert_eq!(
        sh.border_mask() & border::RIGHT_MINUS_2,
        0,
        "nudge droit encore annulable (cycle 60 <= 376)"
    );
}

// --- STOP_MIDDLE et RIGHT_OFF_FULL (Phase 3) ------------------------------

#[test]
fn stop_middle_raccourcit_la_ligne_de_106_octets() {
    let mut sh = Shifter::new();
    sh.write(addr::RESOLUTION, 0b00); // basse résolution : 160 octets/ligne nominal

    // Hi-res en milieu de ligne (cycle 100, dans `]4, 164]`) : STOP_MIDDLE,
    // pas LEFT_OFF_PENDING (cycle > 4).
    sh.write_resolution(0b10, 100);
    assert_eq!(sh.border_mask() & border::STOP_MIDDLE, border::STOP_MIDDLE);
    // La résolution reste HAUTE (pas de retour en basse/moyenne résolution
    // avant la fin de la ligne) : c'est le cas réaliste — la ligne rend
    // effectivement en haute résolution, raccourcie.
    assert_eq!(sh.resolution(), Resolution::High);

    let ram = vec![0xFFu8; 200];
    let pixels = sh.render_scanline(&ram);

    // Haute résolution nominale : 80 octets/ligne (640/8). -106 saturé à 0
    // (`saturating_sub`) : la ligne raccourcie devient vide en pratique à
    // cette résolution (80 < 106) — voir la limitation documentée sur
    // `BORDERBYTES_*` non mis à l'échelle par plan.
    assert_eq!(pixels.len(), 0);
}

#[test]
fn stop_middle_en_basse_resolution_ne_va_pas_sous_zero() {
    // Combiné à un défilement fin actif : le préchargement ajoute des
    // octets LUS (pour l'adressage source), PAS des pixels AFFICHÉS (même
    // principe que sans `STOP_MIDDLE`, voir `defilement_avec_prechargement_
    // lit_un_groupe_de_plus_et_decale`) — la largeur affichée du segment
    // central dépend uniquement de la réduction `STOP_MIDDLE`, pas du
    // préchargement.
    let mut sh = Shifter::new();
    sh.write(addr::RESOLUTION, 0b00);
    sh.write_hscroll(1, true, true); // préchargement actif : +8 octets LUS, 0 affiché
    sh.write_resolution(0b10, 100); // STOP_MIDDLE
    sh.write_resolution(0b00, 200); // retour en basse résolution, APRÈS le point de non-retour (164)
    assert_eq!(sh.border_mask() & border::STOP_MIDDLE, border::STOP_MIDDLE, "cycle 200 > 164 : pas annulé");

    let ram = vec![0u8; 200];
    let pixels = sh.render_scanline(&ram);
    // 160 (basse rés.) - 106 (StopMiddle) = 54 octets affichés = 108 pixels
    // (basse résolution, 4 plans) ; le préchargement n'y ajoute rien.
    assert_eq!(pixels.len(), 108);
}

#[test]
fn stop_middle_annule_par_un_retour_en_basse_resolution_au_cycle_4() {
    let mut sh = Shifter::new();
    sh.write(addr::RESOLUTION, 0b00);
    sh.write_resolution(0b10, 100); // arme STOP_MIDDLE
    sh.write_resolution(0b00, 4); // retour pile au cycle 4 : annule (voir write_resolution)
    assert_eq!(sh.border_mask() & border::STOP_MIDDLE, 0);
}

#[test]
fn right_off_full_ajoute_66_octets_et_force_left_off_sur_la_ligne_suivante() {
    let mut sh = Shifter::new();
    sh.write_palette_word(addr::PALETTE_BASE, 0x0000);
    sh.write_palette_word(addr::PALETTE_BASE + 2, 0x0777);
    sh.write(addr::RESOLUTION, 0b00); // basse résolution

    // Hi-res après la fenêtre STOP_MIDDLE (>164), dans `]164, 376]` :
    // RIGHT_OFF + RIGHT_OFF_FULL directement (pas besoin de $FF820A).
    sh.write_resolution(0b10, 200);
    assert_eq!(sh.border_mask() & border::RIGHT_OFF, border::RIGHT_OFF);
    assert_eq!(sh.border_mask() & border::RIGHT_OFF_FULL, border::RIGHT_OFF_FULL);
    sh.write_resolution(0b00, 300); // revient en basse résolution pour le rendu (résolution finale)

    let mut ram = vec![0u8; 300];
    ram[160] = 0x80; // premier octet du segment droit -> couleur 1
    let pixels = sh.render_scanline(&ram);
    // 320 px nominaux + 44+22=66 octets = 132 px de bordure droite.
    assert_eq!(pixels.len(), 320 + 132);
    assert_eq!(pixels[320], (255, 255, 255));

    // Cascade : la ligne SUIVANTE démarre directement avec LEFT_OFF_2_STE
    // armé (20 octets de bordure gauche), sans dance de confirmation.
    assert_eq!(
        sh.border_mask() & border::LEFT_OFF_2_STE,
        border::LEFT_OFF_2_STE,
        "cascade : bordure gauche forcée dès le début de la ligne suivante"
    );

    sh.write(addr::VIDEO_COUNTER_LOW, 100);
    let mut ram2 = vec![0u8; 260]; // start(80) + bordure gauche(20) + centre(160) = 260
    ram2[80] = 0x80; // bordure gauche de la ligne 2 (compteur-20=80)
    let pixels2 = sh.render_scanline(&ram2);
    assert_eq!(pixels2.len(), 320 + 40, "40 px de bordure gauche (20 octets, 4 plans)");
    assert_eq!(pixels2[0], (255, 255, 255), "bordure gauche de la ligne 2, forcée par la cascade");
}

#[test]
fn right_off_full_hors_fenetre_n_arme_rien() {
    let mut sh = Shifter::new();
    sh.write(addr::RESOLUTION, 0b00);
    sh.write_resolution(0b10, 400); // bien après la fenêtre `]164, 376]`
    assert_eq!(sh.border_mask() & (border::RIGHT_OFF | border::RIGHT_OFF_FULL), 0);
}

#[test]
fn right_off_reste_annulable_seulement_dans_sa_fenetre_de_declenchement() {
    // La fenêtre d'annulation de `RIGHT_OFF` (`cycles_in_line <= 376`) est
    // quasiment confondue avec sa fenêtre de déclenchement (`]372, 376]`) —
    // une seconde écriture juste après (cycle 375) l'annule encore, mais
    // une écriture nettement plus tardive (cycle 400, la ligne suivante
    // aurait déjà dépassé le point de non-retour) ne le peut plus.
    let mut sh = Shifter::new();
    sh.write_sync(0x00, 374); // arme RIGHT_OFF
    sh.write_sync(0x02, 375); // 50Hz, encore dans la fenêtre d'annulation
    assert_eq!(sh.border_mask() & border::RIGHT_OFF, 0, "annulé : cycle 375 <= 376");

    let mut sh2 = Shifter::new();
    sh2.write_sync(0x00, 374); // arme RIGHT_OFF
    sh2.write_sync(0x02, 400); // 50Hz, bien après la fenêtre d'annulation
    assert_eq!(
        sh2.border_mask() & border::RIGHT_OFF,
        border::RIGHT_OFF,
        "verrouillé pour le reste de la ligne : cycle 400 > 376"
    );
}

// --- OVERSCAN_MED_RES et FOUR_BIT_SCROLL (Phase 4, façon Steem SSE) -------

#[test]
fn med_res_tricks_ignores_si_left_off_2_ste_pas_actif() {
    // Précondition confirmée dans le code Steem (`!left_border`) : ces deux
    // tricks affinent une bordure gauche déjà révélée, ils ne la
    // déclenchent pas eux-mêmes.
    let mut sh = Shifter::new();
    sh.write(addr::RESOLUTION, 0b00);
    sh.write_resolution(0b01, 30); // dans la fenêtre, mais pas de LEFT_OFF_2_STE au préalable
    let ram = vec![0u8; 200];
    let _ = sh.render_scanline(&ram);
    assert_eq!(sh.border_mask() & (border::OVERSCAN_MED_RES | border::FOUR_BIT_SCROLL), 0);
}

#[test]
fn four_bit_scroll_decale_la_lecture_sans_changer_la_largeur() {
    // Isolé de OVERSCAN_MED_RES : r1=16 n'est PAS dans sa fenêtre
    // (`]24,48]`, 16 n'est pas > 24) mais EST dans celle de FOUR_BIT_SCROLL
    // (`[16,48]`).
    let mut sh = Shifter::new();
    sh.write_palette_word(addr::PALETTE_BASE, 0x0000);
    sh.write_palette_word(addr::PALETTE_BASE + 2, 0x0777);
    sh.write(addr::RESOLUTION, 0b00); // basse résolution
    sh.write(addr::VIDEO_COUNTER_LOW, 100);

    sh.write_resolution(0b10, 4); // arme LEFT_OFF_PENDING
    sh.write_resolution(0b00, 4); // confirme LEFT_OFF_2_STE
    sh.write_resolution(0b01, 16); // moyenne résolution à cycle 16 (r1)
    sh.write_resolution(0b00, 30); // retour basse résolution à cycle 30 (r0_next)

    assert_eq!(sh.resolution(), Resolution::Low);

    let mut ram = vec![0u8; 270]; // start(84) + bordure gauche(20) + centre(160) = 264
    // cycles_in_med_res=30-16=14, cycles_in_low_res=16-4=12,
    // shift_in_bytes=8-14/2+12/4=8-7+3=4 ; start=100-20(LEFT_OFF_2_STE)+4=84
    // (`SHIFT_SDP` façon Steem AJOUTE le décalage à la position de lecture).
    ram[84] = 0x80;
    ram[80] = 0x80; // position SANS le décalage (compteur-20) : ne doit PAS être ce qui est lu
    let pixels = sh.render_scanline(&ram);

    // `border_mask()` n'est vérifiable qu'AVANT `render_scanline` pour les
    // mécanismes armés écriture par écriture (voir les autres tests) — ici
    // OVERSCAN_MED_RES/FOUR_BIT_SCROLL ne sont calculés qu'À L'INTÉRIEUR de
    // `render_scanline` (analyse de tout l'historique de la ligne, façon
    // Steem) puis remis à zéro avant son retour : seul l'EFFET (contenu des
    // pixels) est observable de l'extérieur, vérifié ci-dessous.
    assert_eq!(pixels.len(), 320 + 40, "largeur inchangée : seul LEFT_OFF_2_STE ajoute des pixels");
    assert_eq!(pixels[0], (255, 255, 255), "lu depuis la position décalée par FOUR_BIT_SCROLL (84), pas 80");
}

#[test]
fn overscan_med_res_decale_la_lecture_sans_changer_la_largeur() {
    // Isolé de FOUR_BIT_SCROLL : aucune écriture après le passage en
    // moyenne résolution (pas de "changement suivant" à mesurer), la ligne
    // reste rendue en moyenne résolution.
    let mut sh = Shifter::new();
    sh.write_palette_word(addr::PALETTE_BASE, 0x0000);
    sh.write_palette_word(addr::PALETTE_BASE + 2, 0x0777);
    sh.write(addr::RESOLUTION, 0b00);
    sh.write(addr::VIDEO_COUNTER_LOW, 100);

    sh.write_resolution(0b10, 4); // arme LEFT_OFF_PENDING
    sh.write_resolution(0b00, 4); // confirme LEFT_OFF_2_STE
    sh.write_resolution(0b01, 30); // moyenne résolution à cycle 30 (r1), dans ]24,48]

    assert_eq!(sh.resolution(), Resolution::Medium);

    let mut ram = vec![0u8; 260];
    // cycles_in_low_res=30-4=26, shift=-((26/2)%8)/2=-((13%8)/2)=-2 ;
    // start=100-20(LEFT_OFF_2_STE)+(-2)=78.
    ram[78] = 0x80;
    let pixels = sh.render_scanline(&ram);

    // Voir le commentaire équivalent dans
    // `four_bit_scroll_decale_la_lecture_sans_changer_la_largeur` sur
    // pourquoi `border_mask()` n'est pas vérifiable ici — seul l'effet
    // (contenu des pixels) l'est.
    // Moyenne résolution (2 plans) : 20 octets de bordure gauche = 80 px.
    assert_eq!(pixels.len(), 80 + 640, "largeur inchangée par le décalage lui-même");
    assert_eq!(pixels[0], (255, 255, 255), "lu depuis la position décalée par OVERSCAN_MED_RES");
}

#[test]
fn resolution_write_history_et_med_res_byte_shift_remis_a_zero_chaque_ligne() {
    let mut sh = Shifter::new();
    sh.write(addr::RESOLUTION, 0b00);
    sh.write(addr::VIDEO_COUNTER_LOW, 100);
    sh.write_resolution(0b10, 4);
    sh.write_resolution(0b00, 4);
    sh.write_resolution(0b01, 30); // arme OVERSCAN_MED_RES pour cette ligne (moyenne résolution)

    let ram = vec![0u8; 260];
    let pixels1 = sh.render_scanline(&ram);
    assert_eq!(pixels1.len(), 80 + 640, "ligne 1 : bordure gauche LEFT_OFF_2_STE encore active");

    // Ligne suivante, aucune nouvelle écriture $FF8260 : LEFT_OFF_2_STE
    // n'est PAS reconduit (aucune cascade `RIGHT_OFF_FULL` ici) — ni lui ni
    // OVERSCAN_MED_RES/FOUR_BIT_SCROLL (qui en dépendent, voir
    // `detect_med_res_tricks`) ne survivent à la ligne suivante.
    let ram2 = vec![0u8; 300];
    let pixels2 = sh.render_scanline(&ram2);
    assert_eq!(pixels2.len(), 640, "ligne 2 : largeur nominale, plus de bordure gauche");
}
