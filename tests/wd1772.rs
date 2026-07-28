//! Tests unitaires du WD1772 (`rust68::peripherals::wd1772`).

use rust68::peripherals::wd1772::{DmaChannel, FloppyDisk, RawDiskImage, SECTOR_SIZE, Wd1772, reg, status};

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

#[test]
fn restore_ramene_en_piste_zero_et_leve_intrq() {
    let mut fdc = Wd1772::new();
    fdc.write_simple_register(reg::TRACK, 42);
    let mut disk = disk_1_track_1_side_9_sectors();
    let mut dma = TestDma::new();
    fdc.execute_command(0b0000_0000, Some(&mut disk), &mut dma);

    assert_eq!(fdc.read(reg::TRACK), 0);
    assert!(fdc.interrupt_requested());
    let status = fdc.read(reg::COMMAND_STATUS);
    assert_eq!(status & status::TRACK00_OR_LOST_DATA, status::TRACK00_OR_LOST_DATA);
    assert_eq!(status & rust68::peripherals::wd1772::status::BUSY, 0, "plus BUSY une fois la commande terminée");
}

#[test]
fn lire_le_statut_acquitte_intrq() {
    let mut fdc = Wd1772::new();
    let mut disk = disk_1_track_1_side_9_sectors();
    let mut dma = TestDma::new();
    fdc.execute_command(0b0000_0000, Some(&mut disk), &mut dma);
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
    fdc.execute_command(0b0001_0000, Some(&mut disk), &mut dma); // Seek
    assert_eq!(fdc.read(reg::TRACK), 5);
}

#[test]
fn step_in_et_step_out_avec_mise_a_jour_piste() {
    let mut fdc = Wd1772::new();
    let mut disk = disk_1_track_1_side_9_sectors();
    let mut dma = TestDma::new();

    // Step-In avec u=1 (bit4) : piste doit augmenter.
    fdc.execute_command(0b0101_0000, Some(&mut disk), &mut dma);
    assert_eq!(fdc.read(reg::TRACK), 1);
    fdc.execute_command(0b0101_0000, Some(&mut disk), &mut dma);
    assert_eq!(fdc.read(reg::TRACK), 2);

    // Step-Out avec u=1 : piste doit redescendre.
    fdc.execute_command(0b0111_0000, Some(&mut disk), &mut dma);
    assert_eq!(fdc.read(reg::TRACK), 1);
}

#[test]
fn step_sans_u_ne_modifie_pas_le_registre_piste() {
    let mut fdc = Wd1772::new();
    let mut disk = disk_1_track_1_side_9_sectors();
    let mut dma = TestDma::new();
    fdc.execute_command(0b0101_0000, Some(&mut disk), &mut dma); // Step-In, u=1 -> piste=1
    assert_eq!(fdc.read(reg::TRACK), 1);
    fdc.execute_command(0b0010_0000, Some(&mut disk), &mut dma); // Step (rejoue in), u=0
    assert_eq!(fdc.read(reg::TRACK), 1, "sans u, le registre piste ne change pas");
}

#[test]
fn read_sector_transfere_le_secteur_demande_via_dma() {
    let mut fdc = Wd1772::new();
    let mut disk = disk_1_track_1_side_9_sectors();
    let mut dma = TestDma::new();
    fdc.write_simple_register(reg::SECTOR, 3);
    fdc.execute_command(0b1000_0000, Some(&mut disk), &mut dma); // Read Sector, m=0

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
    fdc.execute_command(0b1010_0000, Some(&mut disk), &mut dma); // Write Sector, m=0

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
    fdc.execute_command(0b1010_0000, Some(&mut disk), &mut dma);

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
    fdc.execute_command(0b1001_0000, Some(&mut disk), &mut dma); // Read Sector, m=1

    assert_eq!(dma.written.len(), SECTOR_SIZE * 9, "doit avoir lu les 9 secteurs de la piste");
    for s in 0..9 {
        assert_eq!(dma.written[s * SECTOR_SIZE], (s + 1) as u8);
    }
}

#[test]
fn secteur_inexistant_leve_record_not_found() {
    let mut fdc = Wd1772::new();
    let mut disk = disk_1_track_1_side_9_sectors();
    let mut dma = TestDma::new();
    fdc.write_simple_register(reg::SECTOR, 99); // n'existe pas (9 secteurs max)
    fdc.execute_command(0b1000_0000, Some(&mut disk), &mut dma);

    assert_eq!(
        fdc.read(reg::COMMAND_STATUS) & status::SEEK_ERROR_OR_RECORD_NOT_FOUND,
        status::SEEK_ERROR_OR_RECORD_NOT_FOUND
    );
}

#[test]
fn sans_disque_leve_not_ready() {
    let mut fdc = Wd1772::new();
    let mut dma = TestDma::new();
    fdc.execute_command(0b1000_0000, None::<&mut RawDiskImage>, &mut dma);
    assert!(fdc.interrupt_requested(), "vérifié avant lecture du statut (qui acquitte /INTRQ)");
    assert_eq!(fdc.read(reg::COMMAND_STATUS) & status::NOT_READY, status::NOT_READY);
}

#[test]
fn force_interrupt_efface_busy() {
    let mut fdc = Wd1772::new();
    let mut disk = disk_1_track_1_side_9_sectors();
    let mut dma = TestDma::new();
    fdc.execute_command(0b1101_0001, Some(&mut disk), &mut dma); // Type IV, I0=1
    assert!(fdc.interrupt_requested(), "vérifié avant lecture du statut (qui acquitte /INTRQ)");
    assert_eq!(fdc.read(reg::COMMAND_STATUS) & status::BUSY, 0);
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
