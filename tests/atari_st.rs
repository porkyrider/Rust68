//! Tests du board Atari ST (`rust68::systems::atari_st::AtariSt`).

use rust68::peripherals::acia;
use rust68::peripherals::mfp::{channel, reg};
use rust68::peripherals::shifter::addr as shifter_addr;
use rust68::peripherals::wd1772::{self, FloppyDisk, RawDiskImage, SECTOR_SIZE};
use rust68::peripherals::ym2149;
use rust68::systems::atari_st::{
    ACIA_KEYBOARD_CONTROL, ACIA_KEYBOARD_DATA, ACIA_MIDI_CONTROL, ACIA_MIDI_DATA, AtariSt,
    DEFAULT_ROM_BASE, DMA_ADDR_HIGH, DMA_ADDR_LOW, DMA_ADDR_MID, DMA_MODE, FDC_DATA, IO_BASE,
    MFP_BASE, YM2149_DATA, YM2149_SELECT,
};
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
    let addr = IO_BASE + 0x900; // ex: zone Blitter (STE), pas encore modélisée
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

#[test]
fn priorite_mfp_vbl_hbl() {
    let mut st = AtariSt::new(0x1000, vec![]);
    assert_eq!(st.irq_level(), 0);

    // Une ligne complète (512 cycles PAL) : HBL seul en attente.
    st.tick(512);
    assert_eq!(st.irq_level(), 2, "HBL seul en attente : IPL2");

    // Une trame complète (313 lignes) : VBL doit dominer HBL.
    st.tick(512 * 312);
    assert_eq!(st.irq_level(), 4, "VBL présent : IPL4 domine HBL (IPL2)");

    // Le MFP domine tout le reste.
    st.mfp.write(reg::DDR, 0x00);
    st.mfp.write(reg::AER, 0x01);
    st.mfp.write(reg::IERB, 1 << channel::GPIP0);
    st.mfp.write(reg::IMRB, 1 << channel::GPIP0);
    st.mfp.set_gpip_input(0, true);
    assert_eq!(st.irq_level(), 6, "MFP présent : IPL6 domine VBL/HBL");

    // Acquitter dans l'ordre de priorité.
    st.irq_ack(6);
    assert_eq!(st.irq_level(), 4, "MFP acquitté : VBL redevient visible");
    let vbl_vector = st.irq_ack(4);
    assert_eq!(vbl_vector, 28, "autovecteur niveau 4 = 24+4");
    assert_eq!(st.irq_level(), 2, "VBL acquitté : HBL redevient visible");
    let hbl_vector = st.irq_ack(2);
    assert_eq!(hbl_vector, 26, "autovecteur niveau 2 = 24+2");
    assert_eq!(st.irq_level(), 0);
}

#[test]
fn tick_fait_progresser_mfp_et_glue_ensemble() {
    let mut st = AtariSt::new(0x1000, vec![]);
    st.mfp.write(reg::IERA, 1 << (channel::TIMER_A - 8));
    st.mfp.write(reg::IMRA, 1 << (channel::TIMER_A - 8));
    st.mfp.write(reg::TADR, 200);
    st.mfp.write(reg::TACR, 7); // ÷200, la période la plus lente

    for _ in 0..600 {
        st.tick(512); // 600 lignes : largement > 1 trame et > 1 période timer
    }

    assert!(st.glue.frame_count() >= 1, "le GLUE doit avoir avancé");
    assert!(
        st.mfp.read(reg::IPRA) & (1 << (channel::TIMER_A - 8)) != 0,
        "le MFP doit avoir avancé aussi"
    );
}

#[test]
fn reset_bus_ne_touche_pas_le_glue() {
    let mut st = AtariSt::new(0x1000, vec![]);
    st.tick(512 * 313); // 1 trame complète : VBL en attente
    assert!(st.glue.vbl_pending());
    st.reset_bus();
    assert!(
        st.glue.vbl_pending(),
        "le timing vidéo continue indépendamment d'un /RESET CPU"
    );
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

#[test]
fn acia_mappees_aux_bonnes_adresses() {
    let mut st = AtariSt::new(0x1000, vec![]);
    st.write8(ACIA_KEYBOARD_DATA, 0x99); // devrait aller à l'ACIA clavier, pas MIDI
    assert_eq!(st.acia_keyboard.take_tx_byte(), Some(0x99));
    assert_eq!(st.acia_midi.take_tx_byte(), None);

    st.write8(ACIA_MIDI_DATA, 0x77);
    assert_eq!(st.acia_midi.take_tx_byte(), Some(0x77));
    assert_eq!(st.acia_keyboard.take_tx_byte(), None);

    assert_eq!(
        st.read8(ACIA_KEYBOARD_CONTROL),
        st.acia_keyboard.read(acia::reg::CONTROL_STATUS)
    );
    assert_eq!(
        st.read8(ACIA_MIDI_CONTROL),
        st.acia_midi.read(acia::reg::CONTROL_STATUS)
    );
}

#[test]
fn irq_acia_relayee_via_gpip4_du_mfp() {
    let mut st = AtariSt::new(0x1000, vec![]);
    st.mfp.write(reg::DDR, 0x00); // GPIP4 en entrée
    st.mfp.write(reg::AER, 1 << 4); // front montant
    st.mfp.write(reg::IERB, 1 << channel::GPIP4);
    st.mfp.write(reg::IMRB, 1 << channel::GPIP4);

    assert_eq!(st.irq_level(), 0);

    // Activer RIE sur l'ACIA clavier puis recevoir un octet : IRQ demandée.
    st.acia_keyboard.write(acia::reg::CONTROL_STATUS, 0x80);
    st.acia_keyboard.push_rx_byte(0x41);
    assert!(st.acia_keyboard.irq_requested());

    st.tick(4); // fait progresser le câblage GPIP4 (voir AtariSt::tick)
    assert_eq!(st.irq_level(), 6, "l'IRQ ACIA doit remonter jusqu'au MFP (IPL6)");
}

#[test]
fn reset_bus_reinitialise_les_acia() {
    let mut st = AtariSt::new(0x1000, vec![]);
    st.acia_keyboard.write(acia::reg::CONTROL_STATUS, 0x80);
    st.acia_keyboard.push_rx_byte(0x41);
    assert!(st.acia_keyboard.irq_requested());

    st.reset_bus();

    assert!(!st.acia_keyboard.irq_requested(), "reset_bus doit réinitialiser les ACIA");
}

#[test]
fn ym2149_mappe_a_ff8800_ff8802() {
    let mut st = AtariSt::new(0x1000, vec![]);
    st.write8(YM2149_SELECT, ym2149::reg::AMPLITUDE_A);
    st.write8(YM2149_DATA, 0x0F);
    assert_eq!(st.read8(YM2149_SELECT), ym2149::reg::AMPLITUDE_A);
    assert_eq!(st.read8(YM2149_DATA), 0x0F);
    assert_eq!(
        st.ym2149.channel_level(0),
        0,
        "sans activer de porte tonalité/bruit dans MIXER, le canal reste coupé"
    );
}

#[test]
fn tick_fait_progresser_le_ym2149() {
    let mut st = AtariSt::new(0x1000, vec![]);
    st.write8(YM2149_SELECT, ym2149::reg::MIXER);
    st.write8(YM2149_DATA, 0b0000_1001); // portes tonalité+bruit A ouvertes
    st.write8(YM2149_SELECT, ym2149::reg::AMPLITUDE_A);
    st.write8(YM2149_DATA, 0x0F);

    st.tick(4);
    assert_eq!(st.ym2149.channel_level(0), 30, "le YM2149 doit avoir été cadencé par tick()");
}

#[test]
fn reset_bus_reinitialise_le_ym2149() {
    let mut st = AtariSt::new(0x1000, vec![]);
    st.write8(YM2149_SELECT, ym2149::reg::AMPLITUDE_A);
    st.write8(YM2149_DATA, 0x0F);
    st.reset_bus();
    st.write8(YM2149_SELECT, ym2149::reg::AMPLITUDE_A);
    assert_eq!(st.read8(YM2149_DATA), 0, "reset_bus doit réinitialiser le YM2149");
}

#[test]
fn shifter_registres_mappes_correctement() {
    let mut st = AtariSt::new(0x1000, vec![]);
    st.write8(shifter_addr::VIDEO_BASE_HIGH, 0x00);
    st.write8(shifter_addr::VIDEO_BASE_MID, 0x10); // base vidéo = 0x001000
    st.write8(shifter_addr::RESOLUTION, 0b00);
    assert_eq!(st.read8(shifter_addr::VIDEO_BASE_MID), 0x10);
    assert_eq!(st.shifter.resolution(), rust68::peripherals::shifter::Resolution::Low);
}

/// Test d'intégration bout-en-bout : écrit un motif connu en RAM vidéo,
/// avance le board d'une ligne complète (512 cycles CPU, PAL), et vérifie
/// que le framebuffer contient les pixels attendus.
#[test]
fn tick_rend_une_ligne_video_dans_le_framebuffer() {
    let mut st = AtariSt::new(0x10000, vec![]);
    // Base vidéo = 0x000000 (par défaut), résolution basse.
    st.write8(shifter_addr::RESOLUTION, 0b00);
    // Palette couleur 1 = blanc.
    let c1 = shifter_addr::PALETTE_BASE + 2;
    st.write8(c1, 0x07);
    st.write8(c1 + 1, 0x77);
    // Plan 0, premier mot = 0x8000 (pixel 0 posé -> couleur 1).
    st.write8(0x0000, 0x80);
    st.write8(0x0001, 0x00);

    st.tick(512); // une ligne PAL complète

    assert!(!st.framebuffer.is_empty(), "une ligne doit avoir été rendue");
    let line0 = &st.framebuffer[st.glue.current_line() as usize];
    assert_eq!(line0.len(), 320);
    assert_eq!(line0[0], (255, 255, 255), "pixel 0 -> couleur 1 (blanc)");
    assert_eq!(line0[1], (0, 0, 0), "pixel 1 -> couleur 0 (noir)");
}

#[test]
fn tick_rend_une_trame_complete() {
    let mut st = AtariSt::new(0x1_0000, vec![]);
    st.write8(shifter_addr::RESOLUTION, 0b00);
    st.tick(512 * 313); // une trame PAL complète (313 lignes)
    assert_eq!(st.framebuffer.len(), 313);
    assert!(
        st.framebuffer.iter().all(|line| line.len() == 320),
        "chaque ligne rendue doit avoir la largeur de la résolution basse"
    );
}

#[test]
fn reset_bus_reinitialise_le_shifter_et_resynchronise_le_suivi() {
    let mut st = AtariSt::new(0x1000, vec![]);
    st.write8(shifter_addr::RESOLUTION, 0b01); // moyenne résolution
    st.tick(512 * 5);
    st.reset_bus();
    assert_eq!(
        st.shifter.resolution(),
        rust68::peripherals::shifter::Resolution::Low,
        "reset_bus doit réinitialiser le Shifter"
    );
    // Pas de rattrapage massif au tick suivant : un seul tick court ne doit
    // rendre au plus qu'une ligne.
    let lignes_avant = st.framebuffer.len();
    st.tick(4);
    assert!(st.framebuffer.len() <= lignes_avant + 1);
}

fn disque_de_test() -> RawDiskImage {
    let mut data = vec![0u8; 9 * SECTOR_SIZE];
    data[0] = 0xAB; // secteur 1, piste 0, face 0 : motif reconnaissable
    RawDiskImage::new(data, 80, 1, 9)
}

#[test]
fn fdc_registres_multiplexes_via_dma_mode() {
    let mut st = AtariSt::new(0x1000, vec![]);
    st.write8(DMA_MODE, wd1772::reg::TRACK);
    st.write8(FDC_DATA, 42);
    assert_eq!(st.wd1772.read(wd1772::reg::TRACK), 42);
    assert_eq!(st.read8(FDC_DATA), 42);
}

#[test]
fn dma_compteur_adresse_round_trip() {
    let mut st = AtariSt::new(0x1000, vec![]);
    st.write8(DMA_ADDR_HIGH, 0x00);
    st.write8(DMA_ADDR_MID, 0x02);
    st.write8(DMA_ADDR_LOW, 0x10);
    assert_eq!(st.read8(DMA_ADDR_HIGH), 0x00);
    assert_eq!(st.read8(DMA_ADDR_MID), 0x02);
    assert_eq!(st.read8(DMA_ADDR_LOW), 0x10);
}

/// Test d'intégration bout-en-bout : insère un disque, positionne le
/// secteur et l'adresse DMA, déclenche Read Sector via l'écriture du
/// registre de commande, et vérifie que la RAM a reçu le secteur.
#[test]
fn read_sector_bout_en_bout_via_dma() {
    let mut st = AtariSt::new(0x2000, vec![]);
    st.floppy_a = Some(disque_de_test());

    st.write8(DMA_ADDR_HIGH, 0x00);
    st.write8(DMA_ADDR_MID, 0x10);
    st.write8(DMA_ADDR_LOW, 0x00); // adresse DMA = 0x1000

    st.write8(DMA_MODE, wd1772::reg::SECTOR);
    st.write8(FDC_DATA, 1); // secteur 1

    st.write8(DMA_MODE, wd1772::reg::COMMAND_STATUS);
    st.write8(FDC_DATA, 0b1000_0000); // Read Sector, m=0

    assert_eq!(st.read8(0x1000), 0xAB, "le motif du secteur 1 doit être en RAM");
    assert!(st.wd1772.interrupt_requested());
}

#[test]
fn write_sector_bout_en_bout_via_dma() {
    let mut st = AtariSt::new(0x2000, vec![]);
    st.floppy_a = Some(disque_de_test());

    // Prépare 512 octets à 0x55 en RAM à partir de 0x1000.
    for i in 0..SECTOR_SIZE as u32 {
        st.write8(0x1000 + i, 0x55);
    }
    st.write8(DMA_ADDR_HIGH, 0x00);
    st.write8(DMA_ADDR_MID, 0x10);
    st.write8(DMA_ADDR_LOW, 0x00);

    st.write8(DMA_MODE, wd1772::reg::SECTOR);
    st.write8(FDC_DATA, 2); // secteur 2

    st.write8(DMA_MODE, wd1772::reg::COMMAND_STATUS);
    st.write8(FDC_DATA, 0b1010_0000); // Write Sector, m=0

    let secteur_2 = st.floppy_a.as_ref().unwrap().read_sector(0, 0, 2).unwrap();
    assert!(secteur_2.iter().all(|&b| b == 0x55));
}

#[test]
fn irq_wd1772_relayee_via_gpip5_du_mfp() {
    let mut st = AtariSt::new(0x1000, vec![]);
    st.floppy_a = Some(disque_de_test());
    st.mfp.write(reg::DDR, 0x00);
    st.mfp.write(reg::AER, 1 << 5); // front montant
    st.mfp.write(reg::IERB, 1 << channel::GPIP5);
    st.mfp.write(reg::IMRB, 1 << channel::GPIP5);

    assert_eq!(st.irq_level(), 0);
    st.write8(DMA_MODE, wd1772::reg::COMMAND_STATUS);
    st.write8(FDC_DATA, 0b0000_0000); // Restore

    st.tick(4); // relaie /INTRQ vers GPIP5
    assert_eq!(st.irq_level(), 6, "l'IRQ WD1772 doit remonter jusqu'au MFP (IPL6)");
}

#[test]
fn reset_bus_reinitialise_le_wd1772() {
    let mut st = AtariSt::new(0x1000, vec![]);
    st.floppy_a = Some(disque_de_test());
    st.write8(DMA_MODE, wd1772::reg::COMMAND_STATUS);
    st.write8(FDC_DATA, 0b0000_0000); // Restore -> lève /INTRQ
    assert!(st.wd1772.interrupt_requested());

    st.reset_bus();

    assert!(!st.wd1772.interrupt_requested(), "reset_bus doit réinitialiser le WD1772");
}
