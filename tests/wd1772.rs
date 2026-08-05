#![cfg(feature = "atari-st")]
//! Tests unitaires du WD1772 (`rust68::peripherals::atari_st::wd1772`).

use rust68::peripherals::atari_st::wd1772::{
    DmaChannel, FloppyDisk, RawDiskImage, SECTOR_SIZE, SoundEvent, Wd1772, reg, status,
};

/// Canal DMA de test : deux buffers, un pour la lecture (RAM -> disque),
/// un pour l'écriture (disque -> RAM), chacun avec son propre curseur.
struct TestDma {
    to_write: Vec<u8>, // ce que le "logiciel" a préparé pour une écriture disque
    read_cursor: usize,
    written: Vec<u8>, // ce que le WD1772 a produit lors d'une lecture disque
}

impl TestDma {
    fn new() -> Self {
        TestDma { to_write: Vec::new(), read_cursor: 0, written: Vec::new() }
    }
}

impl DmaChannel for TestDma {
    fn pull(&mut self) -> u8 {
        let b = self.to_write.get(self.read_cursor).copied().unwrap_or(0);
        self.read_cursor += 1;
        b
    }
    fn push(&mut self, byte: u8) {
        self.written.push(byte);
    }
}

fn disk_1_track_1_side_9_sectors() -> RawDiskImage {
    let mut data = vec![0u8; 9 * SECTOR_SIZE];
    // Motif reconnaissable : chaque secteur commence par son propre numéro.
    for s in 0..9u8 {
        data[s as usize * SECTOR_SIZE] = s + 1;
    }
    RawDiskImage::new(data, 1, 1, 9)
}

/// Fait progresser `fdc` jusqu'à la fin de la commande en cours (voir la
/// doc de `Wd1772::tick` — les commandes ne sont plus synchrones) : par
/// blocs de 50 000 cycles, jusqu'à 30 millions de cycles au total (large
/// marge même pour un Restore depuis la piste la plus lointaine à la
/// vitesse de pas la plus lente).
fn run_to_completion<D: FloppyDisk + ?Sized>(fdc: &mut Wd1772, mut disk: Option<&mut D>, dma: &mut TestDma) {
    let mut total = 0u32;
    while fdc.busy() && total < 30_000_000 {
        fdc.tick(50_000, disk.as_deref_mut(), dma);
        total += 50_000;
    }
    assert!(!fdc.busy(), "la commande n'a pas terminé dans la marge de cycles prévue");
}

/// Comme [`run_to_completion`], mais par pas de 1000 cycles (plus précis)
/// et renvoie le nombre total de cycles consommés — pour comparer des
/// durées entre deux commandes (voir les tests de latence de rotation).
fn run_to_completion_cycles<D: FloppyDisk + ?Sized>(
    fdc: &mut Wd1772,
    mut disk: Option<&mut D>,
    dma: &mut TestDma,
) -> u32 {
    let mut total = 0u32;
    while fdc.busy() && total < 30_000_000 {
        fdc.tick(1_000, disk.as_deref_mut(), dma);
        total += 1_000;
    }
    assert!(!fdc.busy(), "la commande n'a pas terminé dans la marge de cycles prévue");
    total
}

#[test]
fn restore_ramene_en_piste_zero_et_leve_intrq() {
    let mut fdc = Wd1772::new();
    fdc.write_simple_register(reg::TRACK, 42);
    let mut disk = disk_1_track_1_side_9_sectors();
    let mut dma = TestDma::new();
    fdc.execute_command(0b0000_0000, Some(&mut disk));
    run_to_completion(&mut fdc, Some(&mut disk), &mut dma);

    assert_eq!(fdc.read(reg::TRACK), 0);
    assert!(fdc.interrupt_requested());
    let status = fdc.read(reg::COMMAND_STATUS);
    assert_eq!(status & status::TRACK00_OR_LOST_DATA, status::TRACK00_OR_LOST_DATA);
    assert_eq!(status & rust68::peripherals::atari_st::wd1772::status::BUSY, 0, "plus BUSY une fois la commande terminée");
}

#[test]
fn lire_le_statut_acquitte_intrq() {
    let mut fdc = Wd1772::new();
    let mut disk = disk_1_track_1_side_9_sectors();
    let mut dma = TestDma::new();
    fdc.execute_command(0b0000_0000, Some(&mut disk));
    run_to_completion(&mut fdc, Some(&mut disk), &mut dma);
    assert!(fdc.interrupt_requested());
    fdc.read(reg::COMMAND_STATUS);
    assert!(!fdc.interrupt_requested(), "lire le statut doit acquitter /INTRQ");
}

#[test]
fn seek_deplace_vers_la_piste_cible() {
    let mut fdc = Wd1772::new();
    let mut disk = disk_1_track_1_side_9_sectors();
    let mut dma = TestDma::new();
    fdc.write_simple_register(reg::DATA, 5);
    fdc.execute_command(0b0001_0000, Some(&mut disk)); // Seek
    run_to_completion(&mut fdc, Some(&mut disk), &mut dma);
    assert_eq!(fdc.read(reg::TRACK), 5);
}

#[test]
fn step_in_et_step_out_avec_mise_a_jour_piste() {
    let mut fdc = Wd1772::new();
    let mut disk = disk_1_track_1_side_9_sectors();
    let mut dma = TestDma::new();

    // Step-In avec u=1 (bit4) : piste doit augmenter.
    fdc.execute_command(0b0101_0000, Some(&mut disk));
    run_to_completion(&mut fdc, Some(&mut disk), &mut dma);
    assert_eq!(fdc.read(reg::TRACK), 1);
    fdc.execute_command(0b0101_0000, Some(&mut disk));
    run_to_completion(&mut fdc, Some(&mut disk), &mut dma);
    assert_eq!(fdc.read(reg::TRACK), 2);

    // Step-Out avec u=1 : piste doit redescendre.
    fdc.execute_command(0b0111_0000, Some(&mut disk));
    run_to_completion(&mut fdc, Some(&mut disk), &mut dma);
    assert_eq!(fdc.read(reg::TRACK), 1);
}

#[test]
fn step_sans_u_ne_modifie_pas_le_registre_piste() {
    let mut fdc = Wd1772::new();
    let mut disk = disk_1_track_1_side_9_sectors();
    let mut dma = TestDma::new();
    fdc.execute_command(0b0101_0000, Some(&mut disk)); // Step-In, u=1 -> piste=1
    run_to_completion(&mut fdc, Some(&mut disk), &mut dma);
    assert_eq!(fdc.read(reg::TRACK), 1);
    fdc.execute_command(0b0010_0000, Some(&mut disk)); // Step (rejoue in), u=0
    run_to_completion(&mut fdc, Some(&mut disk), &mut dma);
    assert_eq!(fdc.read(reg::TRACK), 1, "sans u, le registre piste ne change pas");
}

#[test]
fn read_sector_transfere_le_secteur_demande_via_dma() {
    let mut fdc = Wd1772::new();
    let mut disk = disk_1_track_1_side_9_sectors();
    let mut dma = TestDma::new();
    fdc.write_simple_register(reg::SECTOR, 3);
    fdc.execute_command(0b1000_0000, Some(&mut disk)); // Read Sector, m=0
    run_to_completion(&mut fdc, Some(&mut disk), &mut dma);

    assert_eq!(dma.written.len(), SECTOR_SIZE);
    assert_eq!(dma.written[0], 3, "secteur 3 commence par l'octet 3 (motif de test)");
    assert!(fdc.interrupt_requested());
}

#[test]
fn write_sector_ecrit_sur_le_disque_via_dma() {
    let mut fdc = Wd1772::new();
    let mut disk = disk_1_track_1_side_9_sectors();
    let mut dma = TestDma::new();
    dma.to_write = vec![0xAA; SECTOR_SIZE];
    fdc.write_simple_register(reg::SECTOR, 2);
    fdc.execute_command(0b1010_0000, Some(&mut disk)); // Write Sector, m=0
    run_to_completion(&mut fdc, Some(&mut disk), &mut dma);

    let readback = disk.read_sector(0, 0, 2).unwrap();
    assert!(readback.iter().all(|&b| b == 0xAA));
}

#[test]
fn write_sector_refuse_si_disque_protege_en_ecriture() {
    let mut fdc = Wd1772::new();
    let mut disk = disk_1_track_1_side_9_sectors();
    let original = disk.read_sector(0, 0, 2).unwrap();
    disk.set_write_protected(true);
    let mut dma = TestDma::new();
    dma.to_write = vec![0xAA; SECTOR_SIZE];
    fdc.write_simple_register(reg::SECTOR, 2);
    fdc.execute_command(0b1010_0000, Some(&mut disk));
    run_to_completion(&mut fdc, Some(&mut disk), &mut dma);

    assert_eq!(fdc.read(reg::COMMAND_STATUS) & status::WRITE_PROTECT, status::WRITE_PROTECT);
    let unchanged = disk.read_sector(0, 0, 2).unwrap();
    assert_eq!(unchanged, original, "l'écriture ne doit pas avoir eu lieu");
}

#[test]
fn read_sector_multiple_lit_plusieurs_secteurs_consecutifs() {
    let mut fdc = Wd1772::new();
    let mut disk = disk_1_track_1_side_9_sectors();
    let mut dma = TestDma::new();
    fdc.write_simple_register(reg::SECTOR, 1);
    fdc.execute_command(0b1001_0000, Some(&mut disk)); // Read Sector, m=1
    run_to_completion(&mut fdc, Some(&mut disk), &mut dma);

    assert_eq!(dma.written.len(), SECTOR_SIZE * 9, "doit avoir lu les 9 secteurs de la piste");
    for s in 0..9 {
        assert_eq!(dma.written[s * SECTOR_SIZE], (s + 1) as u8);
    }
}

/// Disque à deux pistes de tailles différentes (10 et 12 secteurs) — comme
/// une image `.stx` protégée où `sectors_per_track()` (le maximum global,
/// ici 12, piste 1) ne reflète pas le compte réel de chaque piste
/// individuelle (voir la doc de `FloppyDisk::sectors_on_track`). Sert à
/// vérifier que `Wd1772::finish_sector_transfer` borne bien la continuation
/// d'une lecture multiple (bit M) sur le compte RÉEL de la piste visée, pas
/// sur ce maximum global.
struct DisqueParPisteHeterogene;

impl FloppyDisk for DisqueParPisteHeterogene {
    fn num_tracks(&self) -> u8 {
        2
    }
    fn num_sides(&self) -> u8 {
        1
    }
    fn sectors_per_track(&self) -> u8 {
        12 // maximum global (piste 1), PAS le compte réel de la piste 0
    }
    fn write_protected(&self) -> bool {
        true
    }
    fn read_sector(&self, track: u8, _side: u8, sector: u8) -> Option<[u8; SECTOR_SIZE]> {
        let real_count = self.sectors_on_track(track, 0);
        if sector == 0 || sector > real_count {
            return None;
        }
        let mut buf = [0u8; SECTOR_SIZE];
        buf[0] = sector;
        Some(buf)
    }
    fn write_sector(&mut self, _track: u8, _side: u8, _sector: u8, _data: &[u8; SECTOR_SIZE]) {}
    fn sectors_on_track(&self, track: u8, _side: u8) -> u8 {
        if track == 0 { 10 } else { 12 }
    }
}

#[test]
fn lecture_multiple_s_arrete_au_compte_reel_de_la_piste_pas_au_maximum_global() {
    let mut fdc = Wd1772::new();
    let mut disk = DisqueParPisteHeterogene;
    let mut dma = TestDma::new();
    fdc.write_simple_register(reg::SECTOR, 1);
    fdc.execute_command(0b1001_0000, Some(&mut disk)); // Read Sector, m=1, piste 0 (10 secteurs réels)
    run_to_completion(&mut fdc, Some(&mut disk), &mut dma);

    assert_eq!(
        dma.written.len(),
        SECTOR_SIZE * 10,
        "doit s'arrêter aux 10 secteurs réels de la piste, pas continuer jusqu'au maximum global (12)"
    );
    assert_eq!(
        fdc.read(reg::COMMAND_STATUS) & status::SEEK_ERROR_OR_RECORD_NOT_FOUND,
        0,
        "une fin normale de piste ne doit pas être signalée comme une erreur"
    );
}

#[test]
fn secteur_inexistant_leve_record_not_found() {
    let mut fdc = Wd1772::new();
    let mut disk = disk_1_track_1_side_9_sectors();
    let mut dma = TestDma::new();
    fdc.write_simple_register(reg::SECTOR, 99); // n'existe pas (9 secteurs max)
    fdc.execute_command(0b1000_0000, Some(&mut disk));
    run_to_completion(&mut fdc, Some(&mut disk), &mut dma);

    assert_eq!(
        fdc.read(reg::COMMAND_STATUS) & status::SEEK_ERROR_OR_RECORD_NOT_FOUND,
        status::SEEK_ERROR_OR_RECORD_NOT_FOUND
    );
}

#[test]
fn sans_disque_leve_not_ready() {
    let mut fdc = Wd1772::new();
    // Immédiat (pas de tick nécessaire) : `NOT_READY` est une ligne
    // matérielle lue tout de suite, pas un délai simulé pour une commande
    // qui de toute façon ne peut pas s'exécuter.
    fdc.execute_command(0b1000_0000, None::<&mut RawDiskImage>);
    assert!(fdc.interrupt_requested(), "vérifié avant lecture du statut (qui acquitte /INTRQ)");
    assert_eq!(fdc.read(reg::COMMAND_STATUS) & status::NOT_READY, status::NOT_READY);
}

#[test]
fn force_interrupt_efface_busy() {
    let mut fdc = Wd1772::new();
    let mut disk = disk_1_track_1_side_9_sectors();
    fdc.execute_command(0b1101_0001, Some(&mut disk)); // Type IV, I0=1
    assert!(fdc.interrupt_requested(), "vérifié avant lecture du statut (qui acquitte /INTRQ)");
    assert_eq!(fdc.read(reg::COMMAND_STATUS) & status::BUSY, 0);
}

#[test]
fn force_interrupt_interrompt_une_commande_en_cours() {
    // Type IV doit être immédiat même si une commande Type I/II est en
    // cours d'exécution (BUSY) — c'est justement le mécanisme prévu pour ne
    // pas avoir à attendre la fin d'une commande longue.
    let mut fdc = Wd1772::new();
    let mut disk = disk_1_track_1_side_9_sectors();
    fdc.write_simple_register(reg::DATA, 40);
    fdc.execute_command(0b0001_0000, Some(&mut disk)); // Seek(40), plusieurs dizaines de ms
    assert!(fdc.busy(), "la commande doit être en cours, pas terminée instantanément");

    fdc.execute_command(0b1101_0001, Some(&mut disk)); // Force Interrupt, I0=1
    assert!(!fdc.busy(), "Force Interrupt doit interrompre immédiatement");
    assert!(fdc.interrupt_requested());
}

#[test]
fn commande_type1_prend_un_temps_reel_non_nul() {
    // Cœur du correctif : une commande ne doit plus se terminer avant même
    // le premier `tick()` — sans quoi un logiciel qui poll BUSY ne le
    // verrait jamais, et l'émulation de la disquette serait bien trop
    // rapide par rapport au vrai matériel.
    let mut fdc = Wd1772::new();
    let mut disk = disk_1_track_1_side_9_sectors();
    fdc.write_simple_register(reg::DATA, 10);
    fdc.execute_command(0b0001_0000, Some(&mut disk)); // Seek(10)
    assert!(fdc.busy(), "BUSY doit être observable juste après l'émission de la commande");
}

#[test]
fn bruitage_step_simple_ne_declenche_que_le_clic() {
    // Un déplacement d'une seule piste (Step-In) ne joue qu'un clic sec,
    // PAS le bourdonnement de recherche — même si une rafale de commandes
    // Step séparées et rapprochées se produit ensuite (protection
    // anti-copie relisant piste par piste, cas réel observé sur
    // Rick_Dangerous.stx) : constat empirique après essai en conditions
    // réelles, superposer le bourdonnement à CHAQUE pas isolé fait sonner
    // une telle rafale comme un magma continu plutôt qu'un train de clics
    // distincts et réguliers (voir la doc de `Wd1772::queue_step_sound`).
    let mut fdc = Wd1772::new();
    let mut disk = disk_1_track_1_side_9_sectors();
    let mut dma = TestDma::new();
    fdc.execute_command(0b0101_0000, Some(&mut disk)); // Step-In #1, u=1
    let events = fdc.take_sound_events();
    assert_eq!(events, vec![SoundEvent::MotorOn, SoundEvent::StepClick]);
    run_to_completion(&mut fdc, Some(&mut disk), &mut dma);
    let _ = fdc.take_sound_events();

    fdc.execute_command(0b0101_0000, Some(&mut disk)); // Step-In #2, rapproché
    let events = fdc.take_sound_events();
    assert_eq!(events, vec![SoundEvent::StepClick], "toujours pas de bourdonnement, même en rafale");
}

#[test]
fn bruitage_seek_multi_piste_bourdonne_jusqu_a_la_fin() {
    let mut fdc = Wd1772::new();
    let mut disk = disk_1_track_1_side_9_sectors();
    let mut dma = TestDma::new();
    fdc.write_simple_register(reg::DATA, 10); // cible à 10 pistes -> plusieurs pas
    fdc.execute_command(0b0001_0000, Some(&mut disk)); // Seek
    let events = fdc.take_sound_events();
    assert_eq!(events, vec![SoundEvent::MotorOn, SoundEvent::SeekStart]);

    run_to_completion(&mut fdc, Some(&mut disk), &mut dma);
    let events = fdc.take_sound_events();
    assert!(!events.contains(&SoundEvent::SeekEnd), "pas encore, la marge après la fin du seek n'est pas écoulée");

    fdc.tick(200_000 /* > SEEK_GRACE_CYCLES (20 ms = 160 000 cycles) */, Some(&mut disk), &mut dma);
    let events = fdc.take_sound_events();
    assert!(events.contains(&SoundEvent::SeekEnd), "le bourdonnement doit s'arrêter passé la marge après la fin du seek");
}

#[test]
fn moteur_reste_allume_apres_une_commande_puis_s_arrete() {
    // Cœur du comportement repris de Stay/Steem SSE : le moteur ne s'éteint
    // pas instantanément à la fin d'une commande (contrairement à BUSY,
    // qui lui retombe tout de suite) — il continue de tourner quelques
    // tours de plus, ce qu'un logiciel qui enchaîne vite plusieurs accès
    // ne devrait pas entendre comme un redémarrage à chaque fois.
    let mut fdc = Wd1772::new();
    let mut disk = disk_1_track_1_side_9_sectors();
    let mut dma = TestDma::new();
    fdc.write_simple_register(reg::SECTOR, 1);
    fdc.execute_command(0b1000_0000, Some(&mut disk)); // Read Sector
    run_to_completion(&mut fdc, Some(&mut disk), &mut dma);
    assert!(!fdc.busy(), "la commande elle-même doit être terminée");
    let events = fdc.take_sound_events();
    assert!(!events.contains(&SoundEvent::MotorOff), "le moteur ne doit pas s'éteindre tout de suite");

    // Une commande immédiatement suivante ne doit PAS rejouer MotorOn (le
    // moteur tournait toujours).
    fdc.execute_command(0b1000_0000, Some(&mut disk));
    let events = fdc.take_sound_events();
    assert!(!events.contains(&SoundEvent::MotorOn), "pas de nouveau MotorOn tant que le moteur tournait déjà");
    run_to_completion(&mut fdc, Some(&mut disk), &mut dma);
    let _ = fdc.take_sound_events();

    // Laisser tourner assez longtemps sans nouvelle commande : le moteur
    // doit finir par s'arrêter (9 tours à 300 tr/min ~ 1,8 s).
    let mut total = 0u32;
    let mut saw_motor_off = false;
    while total < 20_000_000 {
        fdc.tick(50_000, Some(&mut disk), &mut dma);
        total += 50_000;
        if fdc.take_sound_events().contains(&SoundEvent::MotorOff) {
            saw_motor_off = true;
            break;
        }
    }
    assert!(saw_motor_off, "le moteur doit finir par s'arrêter après une période d'inactivité");
}

#[test]
fn lecture_secteur_par_secteur_avec_bit_e_a_zero_saute_le_chargement_de_tete() {
    // Bug réel corrigé : TOS/GEMDOS enchaîne ses lectures secteur par
    // secteur avec le bit `E` (chargement de tête) à 0 — vérifié par trace
    // sur une vraie disquette (Rick_Dangerous.stx) — pour justement SAUTER
    // ce délai entre deux lectures consécutives sur la même piste.
    // L'appliquer quand même (bug de l'ancienne implémentation) désalignait
    // la recherche de la position angulaire réelle du secteur suivant à
    // chaque fois, forçant une attente d'environ un tour complet par
    // secteur (~217 ms mesurés en pratique) au lieu de suivre la rotation
    // réelle du disque. Lire TOUS les secteurs d'une piste un par un (9
    // commandes Read Sector séparées, comme le fait vraiment TOS) doit donc
    // prendre environ UN tour de disque au total, pas neuf.
    let mut fdc = Wd1772::new();
    let mut disk = disk_1_track_1_side_9_sectors(); // 9 secteurs/piste
    let mut dma = TestDma::new();

    let mut total_cycles = 0u32;
    for sector in 1..=9u8 {
        fdc.write_simple_register(reg::SECTOR, sector);
        fdc.execute_command(0b1000_0000, Some(&mut disk)); // Read Sector, m=0, E=0
        total_cycles += run_to_completion_cycles(&mut fdc, Some(&mut disk), &mut dma);
    }

    // Un tour de disque à 300 tr/min = 200 ms = 1 600 000 cycles CPU (8 MHz).
    const CYCLES_PER_REVOLUTION: u32 = 200 * 8_000;
    assert!(
        total_cycles < CYCLES_PER_REVOLUTION * 3 / 2,
        "9 lectures séquentielles ne devraient pas dépasser ~1,5 tour de disque : {total_cycles} cycles \
         (l'ancien bug, qui rechargeait la tête à chaque secteur, en prenait ~9x plus)"
    );
}

#[test]
fn raw_disk_image_round_trip_et_bornes() {
    let mut disk = RawDiskImage::new(vec![0u8; 2 * 9 * SECTOR_SIZE], 2, 1, 9);
    let mut sector = [0u8; SECTOR_SIZE];
    sector[0] = 0x42;
    disk.write_sector(1, 0, 5, &sector);
    assert_eq!(disk.read_sector(1, 0, 5).unwrap()[0], 0x42);

    assert!(disk.read_sector(2, 0, 1).is_none(), "piste hors bornes");
    assert!(disk.read_sector(0, 0, 0).is_none(), "secteur 0 n'existe pas (1-based)");
    assert!(disk.read_sector(0, 0, 10).is_none(), "secteur hors bornes (9 max)");
}
