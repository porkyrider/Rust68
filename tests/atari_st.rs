#![cfg(feature = "atari-st")]
//! Tests du board Atari ST (`rust68::systems::atari_st::AtariSt`).

use rust68::peripherals::atari_st::acia;
use rust68::peripherals::atari_st::blitter::reg as blitter_reg;
use rust68::peripherals::atari_st::mfp::{channel, reg};
use rust68::peripherals::atari_st::shifter::addr as shifter_addr;
use rust68::peripherals::atari_st::wd1772::{self, RawDiskImage, SECTOR_SIZE};
use rust68::peripherals::atari_st::ym2149;
use rust68::systems::atari_st::{
    ACIA_KEYBOARD_CONTROL, ACIA_KEYBOARD_DATA, ACIA_MIDI_CONTROL, ACIA_MIDI_DATA, AtariSt,
    BLITTER_BASE, DEFAULT_ROM_BASE, DMA_ADDR_HIGH, DMA_ADDR_LOW, DMA_ADDR_MID, DMA_MODE, FDC_DATA,
    IO_BASE, MEMORY_CONF, MFP_BASE, STE_DMA_SOUND_BASE, YM2149_DATA, YM2149_SELECT, model,
};
use rust68::{Bus, Cpu};

#[test]
fn gpip7_indique_moniteur_couleur_meme_apres_reset() {
    // GPIP7 = signal "MONO DETECT" du connecteur moniteur sur ST/STE réel :
    // à la masse (0) pour un moniteur monochrome, tiré à 1 sinon. Sans ce
    // câblage, le TOS lirait 0 par défaut et forcerait à tort le mode haute
    // résolution monochrome (320×200 couleur attendu par défaut ici). Le
    // signal reflète un branchement physique externe, pas un état MFP
    // réinitialisable : il doit rester à 1 même après un `/RESET` logiciel.
    let mut st = AtariSt::new(0x1000, vec![]);
    assert_eq!(st.read8(MFP_BASE) & 0x80, 0x80, "GPIP7 = 1 (moniteur couleur)");
    st.reset_bus();
    assert_eq!(
        st.read8(MFP_BASE) & 0x80,
        0x80,
        "GPIP7 reste à 1 après /RESET"
    );
}

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
fn from_model_regle_ram_et_presence_du_blitter() {
    use rust68::systems::atari_st::model::AtariModel;

    // 1040STE : Blitter de série, 1 Mo de RAM (l'exemple donné pour ce lexique).
    // Écriture par MOT complet : un accès `.B` isolé sur ce registre est
    // ignoré sur le silicium réel (voir `Blitter::write`).
    let mut ste = AtariSt::from_model(AtariModel::Ste1040.profile(), vec![]);
    ste.write16(BLITTER_BASE + blitter_reg::HALFTONE_BASE, 0x4200);
    assert_eq!(
        ste.read8(BLITTER_BASE + blitter_reg::HALFTONE_BASE),
        0x42,
        "Blitter de série sur 1040STE : registre réellement lu/écrit"
    );
    assert_eq!(ste.take_bus_fault(), None);

    // 520ST : pas de Blitter de série — la zone retombe dans le stub d'E/S
    // générique (0xFF, écritures ignorées), comme un périphérique absent.
    let mut st = AtariSt::from_model(AtariModel::St520.profile(), vec![]);
    assert_eq!(st.read8(BLITTER_BASE), 0xFF, "pas de Blitter : stub d'E/S générique");
    assert_eq!(st.take_bus_fault(), None, "chip select réel : pas de bus error");
    st.write8(BLITTER_BASE + blitter_reg::CONTROL, 0x80); // ne doit pas déclencher de blit
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
    let mut st = AtariSt::new(0x1000, vec![]); // RAM minuscule
    // Au-delà de l'espace d'adressage "RAM ST" fixe de 4 Mo (voir
    // `acces_flottant_au_dela_de_la_ram_dans_les_4_mo` ci-dessous) : un
    // vrai trou, sans aucun chip select.
    let addr = 0x0050_0000;
    let value = st.read8(addr);
    assert_eq!(value, 0xFF);
    assert_eq!(st.take_bus_fault(), Some((addr, false)));
    // Le fault doit être consommé (remis à None) par take_bus_fault.
    assert_eq!(st.take_bus_fault(), None);
}

#[test]
fn acces_flottant_au_dela_de_la_ram_dans_les_4_mo() {
    // Sur silicium réel, le MMU répond toujours /DTACK dans tout l'espace
    // d'adressage "RAM ST" (4 Mo) — jamais de bus error au-delà de la RAM
    // installée tant qu'on reste dans cette plage, même sans RAM physique
    // à l'adresse précise. Un accès y "flotte" : contrairement à de la
    // vraie RAM, une lecture ne renvoie PAS ce qui vient d'être écrit —
    // c'est justement ce que le TOS observe (pas un repliement d'adresse)
    // pour sa détection de RAM au tout début du boot froid. Confirmé par le
    // code source de Hatari (`stMemory.c`, `VoidMem_bank`) — voir la doc de
    // `AtariSt::in_floating_st_ram` pour l'historique des tentatives de
    // bus error/mirroring essayées et abandonnées ici.
    let mut st = AtariSt::new(0x1000, vec![]); // 4 Ko de RAM réelle

    st.write8(0x0010_0000, 0x42); // bien au-delà des 4 Ko réels, mais < 4 Mo
    assert_eq!(
        st.read8(0x0010_0000),
        0x00,
        "accès flottant : ne renvoie jamais ce qui vient d'être écrit"
    );
    assert_eq!(st.take_bus_fault(), None, "dans les 4 Mo : jamais de bus error");

    // Juste en dessous de la limite de 4 Mo : toujours flottant.
    assert_eq!(st.read8(ST_RAM_ADDRESS_SPACE_TEST - 1), 0x00);
    assert_eq!(st.take_bus_fault(), None);
}

/// Copie locale de la limite de 4 Mo (constante privée du module) pour
/// garder le test lisible sans exposer publiquement une valeur qui n'a
/// pas vocation à être une API stable.
const ST_RAM_ADDRESS_SPACE_TEST: u32 = 4 * 1024 * 1024;

#[test]
fn ecriture_dans_les_8_premiers_octets_declenche_bus_error_avec_vraie_rom() {
    // Documenté noir sur blanc par Atari (Mega Service Manual, carte
    // mémoire RAM) : "the first 8 bytes of ROM are mapped into addresses
    // 0-7. These are reset vectors which the 68000 uses on start-up" — un
    // mirroring permanent, distinct de l'overlay désactivable par
    // MEMORY_CONF. Écrire dedans doit déclencher un vrai bus error (Glue :
    // "asserts Bus Error if... writing to ROM"), pas être silencieusement
    // ignoré. Confirmé nécessaire par la cartouche de diagnostic usine STe
    // (test "I7 Bus error not detected").
    let rom = vec![0u8; 0x1000];
    let mut st = AtariSt::new(0x1000, rom);

    st.write8(0x0000, 0x42);
    assert_eq!(st.take_bus_fault(), Some((0x0000, true)));

    st.write8(0x0007, 0x42);
    assert_eq!(st.take_bus_fault(), Some((0x0007, true)));

    // À partir de l'adresse 8 : RAM normale, aucun bus error (la lecture y
    // voit encore l'overlay ROM tant qu'il est actif — voir `AtariSt::overlay`
    // — mais l'écriture, elle, aboutit bien en RAM sans faute).
    st.write8(0x0008, 0x42);
    assert_eq!(st.take_bus_fault(), None);
}

#[test]
fn ecriture_dans_les_8_premiers_octets_sans_vraie_rom_ne_faute_pas() {
    // Sans ROM réelle (banc d'essai nu), ce mirroring matériel n'a pas de
    // sens : plusieurs tests d'intégration écrivent directement en RAM
    // basse (vecteur de reset de test, contenu vidéo à l'adresse 0...).
    let mut st = AtariSt::new(0x1000, vec![]);
    st.write8(0x0000, 0x42);
    assert_eq!(st.take_bus_fault(), None);
    assert_eq!(st.read8(0x0000), 0x42);
}

#[test]
fn peripherique_non_emule_repond_neutre_sans_fault() {
    let mut st = AtariSt::new(0x1000, vec![]);
    let addr = IO_BASE + 0x950; // zone encore libre, au-delà du stub DMA sound/Microwire
    assert_eq!(st.read8(addr), 0xFF);
    assert_eq!(st.take_bus_fault(), None, "chip select réel : pas de bus error");
    st.write8(addr, 0x12); // ne doit pas paniquer, simplement ignoré
}

#[test]
fn stub_dma_sound_microwire_repond_zero_pas_0xff() {
    // Contrairement au reste de la zone d'E/S non émulée (0xFF, voir le
    // test précédent) : un TOS STE écrit puis relit le registre Microwire
    // en boucle en attendant qu'il retombe à zéro (fin de décalage série).
    // Avec 0xFF (bits toujours à 1), cette attente ne se termine jamais.
    let mut st = AtariSt::new(0x1000, vec![]);
    st.write8(STE_DMA_SOUND_BASE + 0x22, 0x34);
    assert_eq!(st.read8(STE_DMA_SOUND_BASE + 0x22), 0x00);
    assert_eq!(st.take_bus_fault(), None);
}

#[test]
fn dma_sound_lit_les_echantillons_depuis_la_ram_via_le_bus_complet() {
    // Câblage bout-en-bout (pas juste le module `DmaSound` isolé) : les
    // registres écrits via le bus `AtariSt` doivent piloter une lecture
    // réelle dans SA PROPRE RAM (voir `AtariSt::next_dma_sample`).
    let mut st = AtariSt::new(0x1000, vec![]);
    st.write8(0x100, 0x11);
    st.write8(0x101, 0x22);

    st.write8(STE_DMA_SOUND_BASE + 0x07, 0x00); // frame start low (base 0x100 déjà à 0)
    st.write8(STE_DMA_SOUND_BASE + 0x05, 0x01); // frame start mid -> début = 0x100
    st.write8(STE_DMA_SOUND_BASE + 0x13, 0x02); // frame end low
    st.write8(STE_DMA_SOUND_BASE + 0x11, 0x01); // frame end mid -> fin = 0x102
    st.write8(STE_DMA_SOUND_BASE + 0x21, 0x83); // mono, 50066 Hz
    st.write8(STE_DMA_SOUND_BASE + 0x01, 0x01); // PLAY (sans boucle)

    assert_eq!(st.next_dma_sample(50066), (0x11, 0x11));
    assert_eq!(st.next_dma_sample(50066), (0x22, 0x22));
    // Fin de trame atteinte, pas de boucle : silence ensuite.
    assert_eq!(st.next_dma_sample(50066), (0, 0));
}

#[test]
fn dma_sound_boucle_pulse_le_timer_a_via_xsint() {
    // Câblage matériel réel : XSINT (fin de trame DMA, y compris en
    // boucle) est relié à l'entrée de comptage d'événements du Timer A du
    // MFP — sans ce relais, un logiciel qui compte les bouclages via ce
    // timer (la cartouche de diagnostic usine STe, test Audio) n'obtient
    // jamais son interruption. Reproduit précisément ce scénario : Timer A
    // en mode comptage d'événements (bit3 posé), armé pour interrompre
    // après 3 bouclages.
    let mut st = AtariSt::new(0x1000, vec![]);
    st.write8(0x100, 0x11);
    st.write8(0x101, 0x22);

    // Trame d'un seul mot (2 octets — les adresses de trame sont alignées
    // mot sur silicium réel, bit0 câblé à masse, voir `DmaSound::write`),
    // boucle activée : les 2 octets consommés font boucler à chaque fois.
    st.write8(STE_DMA_SOUND_BASE + 0x07, 0x00);
    st.write8(STE_DMA_SOUND_BASE + 0x05, 0x01); // début = 0x100
    st.write8(STE_DMA_SOUND_BASE + 0x13, 0x02);
    st.write8(STE_DMA_SOUND_BASE + 0x11, 0x01); // fin = 0x102
    st.write8(STE_DMA_SOUND_BASE + 0x21, 0x83); // mono, 50066 Hz
    st.write8(STE_DMA_SOUND_BASE + 0x01, 0x03); // PLAY + LOOP

    // La donnée doit être écrite AVANT le contrôle : `reload()` (déclenché
    // par l'écriture de TACR) recharge le compteur depuis la donnée.
    st.mfp.write(reg::TADR, 3); // 3 pulses avant déclenchement
    st.mfp.write(reg::TACR, 0x08); // bit3 = mode comptage d'événements
    st.mfp.write(reg::IERA, 1 << (channel::TIMER_A - 8));
    st.mfp.write(reg::IMRA, 1 << (channel::TIMER_A - 8));

    // Mono, 2 octets/trame : 2 appels consomment une trame entière (1 front
    // XSINT). 3 bouclages nécessitent donc 6 appels.
    for _ in 0..5 {
        st.next_dma_sample(50066);
        assert!(!st.mfp.interrupt_requested(), "pas encore 3 bouclages");
    }
    st.next_dma_sample(50066);
    assert!(st.mfp.interrupt_requested(), "3 bouclages : Timer A doit avoir déclenché");
}

#[test]
fn microwire_volume_maitre_cablee_bout_en_bout_via_le_bus() {
    // Câblage bout-en-bout (pas juste le module `Microwire` isolé) : les
    // registres MASK ($FF8924) puis DATA ($FF8922) écrits via le bus
    // `AtariSt` doivent piloter `st.microwire.left_gain()`/`right_gain()` —
    // c'est ainsi que la cartouche de diagnostic usine STe change de volume
    // entre ses différents tests de tonalité (masque toujours `$7FF`,
    // donnée = commande | `$400`).
    let mut st = AtariSt::new(0x1000, vec![]);
    assert!((st.microwire.left_gain() - 1.0).abs() < 0.001, "volume plein par défaut");

    // Type=3 (volume maître) << 6, valeur=0 : index le plus atténué (-80dB).
    let cmd: u16 = 0x400 | (3 << 6);
    st.write8(0xFF8924, 0x07); // MASK haut
    st.write8(0xFF8925, 0xFF); // MASK bas -> 0x7FF
    st.write8(0xFF8922, (cmd >> 8) as u8); // DATA haut
    st.write8(0xFF8923, (cmd & 0xFF) as u8); // DATA bas -> décode ici

    assert!(st.microwire.left_gain() < 0.01, "commande via le bus doit fortement atténuer");
    assert!(st.microwire.right_gain() < 0.01, "les deux canaux sont atténués par le volume maître");

    // DATA se relit toujours à 0 immédiatement (silicium simulé "décalage
    // déjà terminé", voir la doc de `read8`).
    assert_eq!(st.read8(0xFF8922), 0x00);
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
fn reset_bus_preserve_la_palette_etendue_ste() {
    // `ste_palette` est une caractéristique du silicium (quel Shifter est
    // physiquement dans la machine), pas un registre — RESET (que le TOS
    // exécute lui-même tôt au démarrage, voir `AtariSt::reset_bus`) ne doit
    // pas la réinitialiser. Un bug précédent remplaçait tout le Shifter par
    // `Shifter::new()` (toujours `ste_palette=false`), ce qui repassait
    // silencieusement toute machine STE en palette ST (9 bits) dès la
    // première instruction RESET du TOS — reproduit ici en écrivant une
    // valeur 12 bits ($0FFF, blanc) et en vérifiant qu'elle n'est PAS
    // tronquée au masque ST ($0777) après reset.
    let profile = model::AtariModel::Ste1040.profile();
    let mut st = AtariSt::from_model(profile, vec![]);
    st.reset_bus();
    st.write16(shifter_addr::PALETTE_BASE, 0x0FFF);
    assert_eq!(
        st.shifter.palette_raw()[0],
        0x0FFF,
        "palette STE (12 bits) doit survivre à reset_bus, pas retomber au masque ST (9 bits)"
    );
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
    // `/IRQ` de l'ACIA est un signal réel actif bas câblé sans inverseur :
    // GPIP4 passe de 1 (repos) à 0 quand l'IRQ s'active, un front DESCENDANT
    // (AER=0, la valeur par défaut — pas besoin de l'écrire explicitement,
    // gardé ici pour la lisibilité du test).
    st.mfp.write(reg::AER, 0);
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
    assert_eq!(st.shifter.resolution(), rust68::peripherals::atari_st::shifter::Resolution::Low);
}

/// Test d'intégration bout-en-bout : écrit un motif connu en RAM vidéo,
/// avance le board d'une ligne complète (512 cycles CPU, PAL), et vérifie
/// que le framebuffer contient les pixels attendus.
#[test]
fn tick_rend_une_ligne_video_dans_le_framebuffer() {
    let mut st = AtariSt::new(0x10000, vec![]);
    // Base vidéo = 0x000000 (par défaut), résolution basse.
    st.write8(shifter_addr::RESOLUTION, 0b00);
    // Palette couleur 1 = blanc. Écriture par mot (accès `.W` normal du
    // CPU) : deux `write8` isolés dupliqueraient chacun leur octet dans
    // les deux moitiés du registre (comportement matériel réel, voir
    // `Shifter::write`) et ne donneraient PAS le résultat voulu en général.
    let c1 = shifter_addr::PALETTE_BASE + 2;
    st.write16(c1, 0x0777);
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
    st.tick(512 * 313); // une trame PAL complète (313 lignes, dont 200 visibles)
    // Seules les 200 lignes visibles sont rendues (voir `Glue::visible_lines`
    // et `AtariSt::tick`) : le Shifter ne fetch/avance son compteur vidéo
    // que là, pas pendant les ~113 lignes de blanking vertical, exactement
    // comme sur silicium réel.
    assert_eq!(st.framebuffer.len(), 200);
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
        rust68::peripherals::atari_st::shifter::Resolution::Low,
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

/// Programme le port A du YM2149 pour sélectionner le lecteur A, face 0
/// (voir `AtariSt::floppy_drive_select`) — sans ça, le port A garde sa
/// valeur par défaut après reset (0xFF, "aucun lecteur sélectionné"),
/// exactement comme sur silicium réel avant que TOS ne le programme.
fn select_drive_a_face_0(st: &mut AtariSt) {
    st.write8(YM2149_SELECT, ym2149::reg::IO_PORT_A);
    st.write8(YM2149_DATA, 0b1111_1101); // bit1=0 (lecteur A), bit0=1 (face 0)
}

/// Fait progresser l'émulation jusqu'à la fin de la commande WD1772 en
/// cours (voir la doc de `wd1772::Wd1772::tick` — les commandes ne sont
/// plus synchrones) : par blocs de 50 000 cycles, jusqu'à 3 millions de
/// cycles au total (large marge même pour une commande Type II complète —
/// chargement de tête + latence de rotation + transfert).
fn run_disk_to_completion(st: &mut AtariSt) {
    let mut total = 0u32;
    while st.wd1772.busy() && total < 6_000_000 {
        st.tick(50_000);
        total += 50_000;
    }
    assert!(!st.wd1772.busy(), "la commande n'a pas terminé dans la marge de cycles prévue");
}

#[test]
fn fdc_registres_multiplexes_via_dma_mode() {
    let mut st = AtariSt::new(0x1000, vec![]);
    st.write8(DMA_MODE, wd1772::reg::TRACK << 1);
    st.write8(FDC_DATA, 42);
    assert_eq!(st.wd1772.read(wd1772::reg::TRACK), 42);
    assert_eq!(st.read8(FDC_DATA), 42);
}

#[test]
fn dma_mode_sur_16_bits_decode_le_selecteur_dans_les_bits_1_2() {
    // Bug réel corrigé : `DMA_MODE` ($FF8606) est un registre 16 bits sur
    // silicium réel, avec le sélecteur de registre FDC dans les bits 1-2 de
    // l'octet bas (confirmé par Hatari, `fdc.c` :
    // `FDC_reg = (FDC_DMA.Mode & 0x6) >> 1`) — PAS bits 0-1 d'un `write8`
    // isolé. TOS y accède quasi systématiquement en écriture MOT complète
    // (`move.w`, comme ici via `write16`), jamais en `write8` isolé : une
    // version antérieure qui décomposait naïvement cet accès octet par
    // octet ne voyait jamais que l'octet haut (toujours nul), laissant le
    // sélecteur bloqué à COMMAND_STATUS en permanence quoi que TOS écrive —
    // rendant impossible toute sélection réelle de Piste/Secteur/Donnée, et
    // donc toute lecture de disquette.
    let mut st = AtariSt::new(0x1000, vec![]);

    // bits1-2 = 01 (TRACK) ; valeur pleine telle qu'un vrai TOS l'écrit.
    st.write16(DMA_MODE, 0x0082);
    st.write8(FDC_DATA, 7);
    assert_eq!(st.wd1772.read(wd1772::reg::TRACK), 7, "bits1-2=01 -> TRACK");

    // bits1-2 = 10 (SECTOR)
    st.write16(DMA_MODE, 0x0084);
    st.write8(FDC_DATA, 8);
    assert_eq!(st.wd1772.read(wd1772::reg::SECTOR), 8, "bits1-2=10 -> SECTOR");

    // bits1-2 = 11 (DATA)
    st.write16(DMA_MODE, 0x0086);
    st.write8(FDC_DATA, 9);
    assert_eq!(st.wd1772.read(wd1772::reg::DATA), 9, "bits1-2=11 -> DATA");
}

#[test]
fn fdc_data_sur_16_bits_atteint_bien_le_wd1772() {
    // Bug réel corrigé : `FDC_DATA` ($FF8604) est lui aussi un registre 16
    // bits sur silicium réel, avec l'octet WD1772 réel dans l'octet BAS du
    // mot (confirmé par Hatari, `fdc.c` :
    // `FDC_DiskController_WriteWord`/`...ReadWord` lisent/écrivent
    // `0xff8605`) — PAS l'octet haut qu'une composition octet par octet
    // générique verrait. TOS y accède quasi systématiquement en mot
    // complet : sans cette interception, toute commande ou valeur de
    // piste/secteur/donnée écrite ainsi serait silencieusement perdue
    // (seul l'octet haut, toujours nul, atteindrait le WD1772).
    let mut st = AtariSt::new(0x1000, vec![]);
    st.write16(DMA_MODE, 0x0082); // TRACK (bits1-2=01)
    st.write16(FDC_DATA, 0x0055); // écriture MOT, comme le fait vraiment TOS
    assert_eq!(st.wd1772.read(wd1772::reg::TRACK), 0x55, "octet bas du mot attendu, pas l'octet haut");

    // Lecture symétrique : l'octet réel doit apparaître dans l'octet bas du
    // mot lu, pas l'octet haut.
    assert_eq!(st.read16(FDC_DATA), 0x0055, "octet WD1772 attendu dans l'octet bas du mot lu");
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
    st.floppy_a = Some(Box::new(disque_de_test()));
    select_drive_a_face_0(&mut st);

    st.write8(DMA_ADDR_HIGH, 0x00);
    st.write8(DMA_ADDR_MID, 0x10);
    st.write8(DMA_ADDR_LOW, 0x00); // adresse DMA = 0x1000

    st.write8(DMA_MODE, wd1772::reg::SECTOR << 1);
    st.write8(FDC_DATA, 1); // secteur 1

    st.write8(DMA_MODE, wd1772::reg::COMMAND_STATUS << 1);
    st.write8(FDC_DATA, 0b1000_0000); // Read Sector, m=0
    run_disk_to_completion(&mut st);

    assert_eq!(st.read8(0x1000), 0xAB, "le motif du secteur 1 doit être en RAM");
    assert!(st.wd1772.interrupt_requested());
}

#[test]
fn compteur_secteurs_dma_limite_le_transfert_multi_secteurs() {
    // Bug réel corrigé : le bit 4 de `DMA_MODE` bascule `FDC_DATA` vers un
    // compteur de secteurs DMA séparé (pas un registre du WD1772, voir la
    // doc de `AtariSt::dma_sector_count`) qui limite RÉELLEMENT le nombre
    // de secteurs transférés en lecture multiple (bit M) — indépendamment
    // de ce que le WD1772 continuerait de faire tout seul (il trouve
    // toujours les secteurs suivants tant qu'ils existent sur la piste).
    // Sans cette limite, un GEMDOS qui ne demande que quelques secteurs (le
    // répertoire racine, par exemple) voit son transfert déborder sur la
    // RAM bien au-delà de ce qu'il attend, dès que la piste physique en a
    // davantage — le cas des pistes protégées non standard, confirmé sur
    // une vraie image commerciale (`Rick_Dangerous.stx`, piste 1 : 12
    // secteurs physiques réels contre 10 attendus par le BPB du disque).
    let mut data = vec![0u8; 9 * SECTOR_SIZE];
    for s in 0..9usize {
        data[s * SECTOR_SIZE] = 0x10 + s as u8; // motif reconnaissable par secteur
    }
    let disque = RawDiskImage::new(data, 80, 1, 9);

    let mut st = AtariSt::new(0x4000, vec![]);
    st.floppy_a = Some(Box::new(disque));
    select_drive_a_face_0(&mut st);

    // Sentinelle en RAM avant le transfert, pour détecter un débordement.
    for i in 0..(9 * SECTOR_SIZE as u32) {
        st.write8(0x1000 + i, 0xEE);
    }

    st.write8(DMA_ADDR_HIGH, 0x00);
    st.write8(DMA_ADDR_MID, 0x10);
    st.write8(DMA_ADDR_LOW, 0x00);

    st.write8(DMA_MODE, wd1772::reg::SECTOR << 1);
    st.write8(FDC_DATA, 1); // secteur de départ = 1

    // Programme le compteur de secteurs DMA à 2 (bit4 de DMA_MODE posé).
    st.write8(DMA_MODE, 0x10);
    st.write8(FDC_DATA, 2);

    // Read Sector MULTIPLE (m=1, bit4 de la commande) : bit4 de DMA_MODE
    // retombé à 0, donc FDC_DATA redevient le registre commande/statut.
    st.write8(DMA_MODE, wd1772::reg::COMMAND_STATUS << 1);
    st.write8(FDC_DATA, 0b1001_0000);
    run_disk_to_completion(&mut st);

    assert_eq!(st.read8(0x1000), 0x10, "secteur 1 doit être transféré");
    assert_eq!(st.read8(0x1000 + SECTOR_SIZE as u32), 0x11, "secteur 2 doit être transféré");
    assert_eq!(
        st.read8(0x1000 + 2 * SECTOR_SIZE as u32),
        0xEE,
        "le compteur de secteurs DMA doit arrêter le transfert avant le 3e secteur"
    );
}

#[test]
fn face_disquette_cablee_depuis_le_port_a_du_ym2149() {
    // Cœur du bug corrigé : `wd1772.side` restait auparavant toujours à 0
    // quoi que le port A du YM2149 indique, rendant illisible tout contenu
    // situé sur la face 1 d'une disquette double face (le cas de
    // pratiquement tout logiciel ST réel au format `.st` 720 Ko).
    let mut data = vec![0u8; 2 * 9 * SECTOR_SIZE]; // 2 faces, 9 secteurs, piste 0
    data[0] = 0xAA; // piste 0, face 0, secteur 1
    data[9 * SECTOR_SIZE] = 0xBB; // piste 0, face 1, secteur 1
    let disque = RawDiskImage::new(data, 80, 2, 9);

    let mut st = AtariSt::new(0x2000, vec![]);
    st.floppy_a = Some(Box::new(disque));
    st.write8(DMA_ADDR_HIGH, 0x00);
    st.write8(DMA_ADDR_MID, 0x10);
    st.write8(DMA_ADDR_LOW, 0x00); // adresse DMA = 0x1000
    st.write8(DMA_MODE, wd1772::reg::SECTOR << 1);
    st.write8(FDC_DATA, 1); // secteur 1

    // Lecteur A, face 0 (bit0=1) : doit lire le motif de la face 0.
    st.write8(YM2149_SELECT, ym2149::reg::IO_PORT_A);
    st.write8(YM2149_DATA, 0b1111_1101);
    st.write8(DMA_MODE, wd1772::reg::COMMAND_STATUS << 1);
    st.write8(FDC_DATA, 0b1000_0000); // Read Sector, m=0
    run_disk_to_completion(&mut st);
    assert_eq!(st.read8(0x1000), 0xAA, "face 0 attendue");

    // Lecteur A, face 1 (bit0=0) : doit maintenant lire le motif de la face 1.
    st.write8(DMA_ADDR_MID, 0x10); // ré-armer l'adresse DMA (avancée par la lecture précédente)
    st.write8(DMA_ADDR_LOW, 0x00);
    st.write8(YM2149_SELECT, ym2149::reg::IO_PORT_A);
    st.write8(YM2149_DATA, 0b1111_1100);
    st.write8(DMA_MODE, wd1772::reg::COMMAND_STATUS << 1);
    st.write8(FDC_DATA, 0b1000_0000); // Read Sector, m=0
    run_disk_to_completion(&mut st);
    assert_eq!(st.read8(0x1000), 0xBB, "face 1 attendue après changement du port A");
}

#[test]
fn lecteur_non_selectionne_repond_not_ready() {
    // Si le port A désélectionne le lecteur A (bit1=1) — par exemple parce
    // que le port A n'a pas encore été programmé par TOS (valeur par défaut
    // après reset, voir `Ym2149::new`) — le FDC ne doit pas silencieusement
    // utiliser `floppy_a` quand même : il doit répondre NOT_READY, comme un
    // vrai contrôleur qui ne voit aucun lecteur actif sur le bus.
    let mut st = AtariSt::new(0x1000, vec![]);
    st.floppy_a = Some(Box::new(disque_de_test()));
    // Port A jamais programmé : reste à sa valeur par défaut (0xFF).

    st.write8(DMA_MODE, wd1772::reg::COMMAND_STATUS << 1);
    st.write8(FDC_DATA, 0b1000_0000); // Read Sector, m=0

    assert_eq!(
        st.wd1772.read(wd1772::reg::COMMAND_STATUS) & wd1772::status::NOT_READY,
        wd1772::status::NOT_READY,
        "aucun lecteur sélectionné -> NOT_READY, pas une lecture silencieuse de floppy_a"
    );
}

#[test]
fn write_sector_bout_en_bout_via_dma() {
    let mut st = AtariSt::new(0x2000, vec![]);
    st.floppy_a = Some(Box::new(disque_de_test()));
    select_drive_a_face_0(&mut st);

    // Prépare 512 octets à 0x55 en RAM à partir de 0x1000.
    for i in 0..SECTOR_SIZE as u32 {
        st.write8(0x1000 + i, 0x55);
    }
    st.write8(DMA_ADDR_HIGH, 0x00);
    st.write8(DMA_ADDR_MID, 0x10);
    st.write8(DMA_ADDR_LOW, 0x00);

    st.write8(DMA_MODE, wd1772::reg::SECTOR << 1);
    st.write8(FDC_DATA, 2); // secteur 2

    st.write8(DMA_MODE, wd1772::reg::COMMAND_STATUS << 1);
    st.write8(FDC_DATA, 0b1010_0000); // Write Sector, m=0
    run_disk_to_completion(&mut st);

    let secteur_2 = st.floppy_a.as_ref().unwrap().read_sector(0, 0, 2).unwrap();
    assert!(secteur_2.iter().all(|&b| b == 0x55));
}

#[test]
fn irq_wd1772_relayee_via_gpip5_du_mfp() {
    let mut st = AtariSt::new(0x1000, vec![]);
    st.floppy_a = Some(Box::new(disque_de_test()));
    st.mfp.write(reg::DDR, 0x00);
    // `/INTRQ` du WD1772 est un signal réel actif bas câblé sans inverseur :
    // GPIP5 passe de 1 (repos) à 0 quand /INTRQ s'active, un front DESCENDANT
    // (AER=0, la valeur par défaut — gardé explicite ici pour la lisibilité).
    st.mfp.write(reg::AER, 0);
    st.mfp.write(reg::IERB, 1 << channel::GPIP5);
    st.mfp.write(reg::IMRB, 1 << channel::GPIP5);

    assert_eq!(st.irq_level(), 0);
    st.write8(DMA_MODE, wd1772::reg::COMMAND_STATUS << 1);
    st.write8(FDC_DATA, 0b0000_0000); // Restore

    st.tick(4); // relaie /INTRQ vers GPIP5
    assert_eq!(st.irq_level(), 6, "l'IRQ WD1772 doit remonter jusqu'au MFP (IPL6)");
}

#[test]
fn reset_bus_reinitialise_le_wd1772() {
    let mut st = AtariSt::new(0x1000, vec![]);
    st.floppy_a = Some(Box::new(disque_de_test()));
    st.write8(DMA_MODE, wd1772::reg::COMMAND_STATUS << 1);
    st.write8(FDC_DATA, 0b0000_0000); // Restore -> lève /INTRQ
    assert!(st.wd1772.interrupt_requested());

    st.reset_bus();

    assert!(!st.wd1772.interrupt_requested(), "reset_bus doit réinitialiser le WD1772");
}

// Écriture par mot/long complet (accès `.W`/`.L` réel du CPU) : sur le
// silicium réel, la plupart des registres Blitter ignorent un accès `.B`
// isolé (voir `Blitter::write`) — composer la valeur via des `write8`
// séparés n'aurait donc plus aucun effet depuis ce changement.
fn write_blitter_word(st: &mut AtariSt, offset: u32, value: u16) {
    st.write16(BLITTER_BASE + offset, value);
}

fn write_blitter_long(st: &mut AtariSt, offset: u32, value: u32) {
    st.write32(BLITTER_BASE + offset, value);
}

#[test]
fn blitter_registres_mappes_a_ff8a00() {
    let mut st = AtariSt::new(0x1000, vec![]);
    st.write8(BLITTER_BASE + blitter_reg::HOP, 0b10);
    assert_eq!(st.read8(BLITTER_BASE + blitter_reg::HOP), 0b10);
    assert_eq!(st.blitter.read(blitter_reg::HOP), 0b10);
}

/// Test d'intégration bout-en-bout : déclenche un blit (copie pure) via
/// l'écriture du bit BUSY du registre de contrôle, et vérifie que la RAM
/// du board a bien été modifiée.
#[test]
fn ecrire_control_declenche_le_blit_bout_en_bout() {
    let mut st = AtariSt::new(0x4000, vec![]);
    st.write8(BLITTER_BASE + blitter_reg::HOP, 2); // source seule
    st.write8(BLITTER_BASE + blitter_reg::OP, 0x3); // copie (source)
    write_blitter_word(&mut st, blitter_reg::SRC_X_INC, 2);
    write_blitter_word(&mut st, blitter_reg::DST_X_INC, 2);
    write_blitter_word(&mut st, blitter_reg::X_COUNT, 1);
    write_blitter_word(&mut st, blitter_reg::Y_COUNT, 1);
    write_blitter_word(&mut st, blitter_reg::ENDMASK_1, 0xFFFF);
    write_blitter_word(&mut st, blitter_reg::ENDMASK_2, 0xFFFF);
    write_blitter_word(&mut st, blitter_reg::ENDMASK_3, 0xFFFF);
    write_blitter_long(&mut st, blitter_reg::SRC_ADDR, 0x1000);
    write_blitter_long(&mut st, blitter_reg::DST_ADDR, 0x2000);
    st.write16(0x1000, 0x1234);

    st.write8(BLITTER_BASE + blitter_reg::CONTROL, 0x80); // BUSY/START

    assert_eq!(st.read16(0x2000), 0x1234, "le blit doit avoir copié la source en RAM");
    assert!(!st.blitter.busy(), "BUSY doit être effacé après exécution");
}

#[test]
fn reset_bus_reinitialise_le_blitter() {
    let mut st = AtariSt::new(0x1000, vec![]);
    st.write8(BLITTER_BASE + blitter_reg::OP, 0xC);
    st.reset_bus();
    assert_eq!(st.blitter.read(blitter_reg::OP), 0, "reset_bus doit réinitialiser le Blitter");
}

#[test]
fn mirroring_mmu_intra_banque_ste_quand_memconf_surestime_la_ram_reelle() {
    // 1040STE réel : deux banques de 512 Ko chacune (1 Mo total). Au reset à
    // froid, MEMCONF vaut 0 (banques "128 Ko" par défaut, voir
    // `AtariSt::translate_ram_addr`) — le logiciel de détection RAM écrit
    // ensuite une estimation candidate (ex: "2 Mo" par banque) AVANT de la
    // corriger ; c'est justement PENDANT cette fenêtre que le mirroring
    // matériel doit se manifester, pour que la correction puisse avoir
    // lieu. Reproduit le bug réel trouvé via la cartouche de diagnostic
    // usine STe (annonçait "2M RAM" au lieu d'1 Mo) et vérifié directement
    // contre Hatari (qui, lui, affiche "1M RAM" correctement).
    let mut st = AtariSt::from_model(model::AtariModel::Ste1040.profile(), vec![]);
    assert!(st.blitter_present(), "le 1040STE a un Blitter (STE)");

    // MEMCONF = 0x0A : banque 0 = 2 Mo (bits 3-2 = 10), banque 1 = 2 Mo
    // (bits 1-0 = 10) — l'estimation "trop grande" que prend la cartouche
    // avant de la corriger.
    st.write8(MEMORY_CONF, 0x0A);

    // Écrit un motif au tout début de la banque 0 réelle.
    st.write32(0x10, 0xA5A51234);
    // Adresse au-delà de la banque 0 RÉELLE (512 Ko) mais toujours dans sa
    // taille LOGIQUE actuelle (2 Mo) : doit boucler sur la même RAM
    // physique (mirroring), pas lire une valeur indépendante.
    assert_eq!(
        st.read32(0x10 + 0x80000),
        0xA5A51234,
        "adresse au-delà de la banque réelle (512 Ko) mais dans sa taille logique (2 Mo) : doit boucler"
    );

    // Une fois MEMCONF corrigé à la vraie configuration (512 Ko + 512 Ko,
    // code 0x05), le mirroring disparaît : les deux adresses redeviennent
    // indépendantes (identité, comportement normal une fois le TOS booté).
    st.write8(MEMORY_CONF, 0x05);
    st.write32(0x10 + 0x80000, 0xDEADBEEF);
    assert_eq!(
        st.read32(0x10),
        0xA5A51234,
        "MEMCONF correct : plus de mirroring, la banque 1 est indépendante"
    );
    assert_eq!(st.read32(0x10 + 0x80000), 0xDEADBEEF);
}

/// Configure un blit trivial (20 mots, 1 par ligne — `X_COUNT=1`,
/// `Y_COUNT=20`), sans dépendance à la source (`OP=0x0F` : sortie toujours
/// à 1, donc `HOP=0` ignore la source) — seule la progression (tranchage,
/// `BUSY`) nous intéresse ici, pas le contenu écrit. `DST_ADDR` doit
/// pointer dans la RAM allouée par l'appelant.
fn setup_blit_20_mots(st: &mut AtariSt, dst_addr: u32, control: u8) {
    st.write8(BLITTER_BASE + blitter_reg::HOP, 0);
    st.write8(BLITTER_BASE + blitter_reg::OP, 0x0F);
    st.write16(BLITTER_BASE + blitter_reg::DST_X_INC, 0);
    st.write16(BLITTER_BASE + blitter_reg::DST_Y_INC, 2);
    st.write16(BLITTER_BASE + blitter_reg::X_COUNT, 1);
    st.write16(BLITTER_BASE + blitter_reg::Y_COUNT, 20);
    st.write16(BLITTER_BASE + blitter_reg::ENDMASK_1, 0xFFFF);
    st.write16(BLITTER_BASE + blitter_reg::ENDMASK_2, 0xFFFF);
    st.write16(BLITTER_BASE + blitter_reg::ENDMASK_3, 0xFFFF);
    st.write32(BLITTER_BASE + blitter_reg::DST_ADDR, dst_addr);
    // Déclenche : la toute première tranche s'exécute de façon synchrone,
    // dès cette écriture (voir `AtariSt::write8`, branche CONTROL).
    st.write8(BLITTER_BASE + blitter_reg::CONTROL, control);
}

fn blit_y_count(st: &mut AtariSt) -> u16 {
    ((st.read8(BLITTER_BASE + blitter_reg::Y_COUNT) as u16) << 8)
        | st.read8(BLITTER_BASE + blitter_reg::Y_COUNT1) as u16
}

#[test]
fn blit_non_hog_progresse_par_tranches_de_16_mots_au_rythme_du_cpu() {
    // Limitation historique corrigée (voir la doc du module blitter) : le
    // Blitter en mode partagé (non-HOG) doit rendre la main au CPU toutes
    // les 16 mots (`WORDS_PER_SLICE`), pas exécuter le blit entier d'un
    // coup — et la reprise doit être pilotée par `AtariSt::tick` au rythme
    // réel du CPU (`BLITTER_SLICE_CYCLES` = 256 cycles entre deux
    // tranches, calibré contre Hatari `src/blitter.c`), pas par une
    // simple réécriture logicielle de CONTROL.
    let mut st = AtariSt::new(0x1_0000, vec![]);
    setup_blit_20_mots(&mut st, 0x2000, 0x80); // BUSY=1, HOG=0

    // Première tranche déjà traitée par l'écriture CONTROL elle-même :
    // 16 des 20 mots (20 lignes, 1 mot/ligne) sont faits, 4 restent.
    assert_eq!(blit_y_count(&mut st), 4, "16 lignes traitées par la première tranche");
    assert!(st.blitter.busy(), "blit pas fini : BUSY doit rester observable par polling");

    // Moins de 256 cycles écoulés : aucune tranche supplémentaire.
    st.tick(255);
    assert_eq!(blit_y_count(&mut st), 4, "pas encore assez de cycles CPU pour une nouvelle tranche");
    assert!(st.blitter.busy());

    // Le cycle 256e déclenche la tranche suivante, qui termine le blit
    // (il ne reste que 4 mots, sous le budget de 16).
    st.tick(1);
    assert_eq!(blit_y_count(&mut st), 0, "blit terminé après la deuxième tranche");
    assert!(!st.blitter.busy(), "BUSY retombe une fois le blit réellement fini");
}

#[test]
fn blit_hog_se_termine_en_une_seule_tranche() {
    // À l'inverse : en mode HOG (bit 6 de CONTROL), le silicium réel garde
    // le bus jusqu'à la fin complète, sans jamais le rendre au CPU — donc
    // un seul appel (celui déclenché par l'écriture CONTROL) doit suffire,
    // quelle que soit la taille du blit.
    let mut st = AtariSt::new(0x1_0000, vec![]);
    setup_blit_20_mots(&mut st, 0x2000, 0xC0); // BUSY=1, HOG=1

    assert_eq!(blit_y_count(&mut st), 0, "mode HOG : tout traité en un seul appel");
    assert!(!st.blitter.busy());
}
