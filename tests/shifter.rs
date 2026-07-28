//! Tests unitaires du Shifter (`rust68::peripherals::shifter`).

use rust68::peripherals::shifter::{Resolution, Shifter, addr};

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
    let mut sh = Shifter::new();
    let addr_color3 = addr::PALETTE_BASE + 3 * 2;
    sh.write(addr_color3, 0x07); // octet haut : R=7
    sh.write(addr_color3 + 1, 0x77); // octet bas : G=7, B=7
    assert_eq!(sh.read(addr_color3), 0x07);
    assert_eq!(sh.read(addr_color3 + 1), 0x77);
}

#[test]
fn rendu_basse_resolution_un_groupe_16_pixels() {
    let mut sh = Shifter::new();
    // Palette : couleur 0 = noir, couleur 1 = blanc.
    sh.write(addr::PALETTE_BASE, 0x00);
    sh.write(addr::PALETTE_BASE + 1, 0x00);
    let c1 = addr::PALETTE_BASE + 1 * 2;
    sh.write(c1, 0x07);
    sh.write(c1 + 1, 0x77);
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
fn rendu_moyenne_resolution_deux_plans() {
    let mut sh = Shifter::new();
    for (i, val) in [(0u32, 0x000), (1, 0x700), (2, 0x070), (3, 0x777)] {
        let a = addr::PALETTE_BASE + i * 2;
        sh.write(a, (val >> 8) as u8);
        sh.write(a + 1, val as u8);
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
fn ram_insuffisante_renvoie_une_ligne_noire_sans_avancer() {
    let mut sh = Shifter::new();
    sh.write(addr::RESOLUTION, 0b00);
    let ram = vec![0u8; 10]; // bien moins que 160 octets requis
    let pixels = sh.render_scanline(&ram);
    assert!(pixels.iter().all(|&p| p == (0, 0, 0)));
    assert_eq!(sh.read(addr::VIDEO_COUNTER_LOW), 0, "compteur inchangé");
}
