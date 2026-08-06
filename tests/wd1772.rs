#![cfg(feature = "atari-st")]
//! Unit tests for the WD1772 (`rust68::peripherals::atari_st::wd1772`).

use rust68::peripherals::atari_st::wd1772::{
    DmaChannel, FloppyDisk, RawDiskImage, SECTOR_SIZE, SoundEvent, Wd1772, reg, status,
};

/// Test DMA channel: two buffers, one for reads (RAM -> disk),
/// one for writes (disk -> RAM), each with its own cursor.
struct TestDma {
    to_write: Vec<u8>, // what the "software" prepared for a disk write
    read_cursor: usize,
    written: Vec<u8>, // what the WD1772 produced during a disk read
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
    // Recognizable pattern: each sector starts with its own number.
    for s in 0..9u8 {
        data[s as usize * SECTOR_SIZE] = s + 1;
    }
    RawDiskImage::new(data, 1, 1, 9)
}

/// Advances `fdc` until the current command finishes (see the docs on
/// `Wd1772::tick` — commands are no longer synchronous): in blocks of
/// 50,000 cycles, up to 30 million cycles total (a wide margin even for
/// a Restore from the farthest track at the slowest step rate).
fn run_to_completion<D: FloppyDisk + ?Sized>(fdc: &mut Wd1772, mut disk: Option<&mut D>, dma: &mut TestDma) {
    let mut total = 0u32;
    while fdc.busy() && total < 30_000_000 {
        fdc.tick(50_000, disk.as_deref_mut(), dma);
        total += 50_000;
    }
    assert!(!fdc.busy(), "the command did not finish within the expected cycle margin");
}

/// Like [`run_to_completion`], but in steps of 1000 cycles (more precise)
/// and returns the total number of cycles consumed — used to compare
/// durations between two commands (see the rotational latency tests).
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
    assert!(!fdc.busy(), "the command did not finish within the expected cycle margin");
    total
}

#[test]
fn restore_returns_to_track_zero_and_raises_intrq() {
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
    assert_eq!(status & rust68::peripherals::atari_st::wd1772::status::BUSY, 0, "no longer BUSY once the command is finished");
}

#[test]
fn reading_status_acknowledges_intrq() {
    let mut fdc = Wd1772::new();
    let mut disk = disk_1_track_1_side_9_sectors();
    let mut dma = TestDma::new();
    fdc.execute_command(0b0000_0000, Some(&mut disk));
    run_to_completion(&mut fdc, Some(&mut disk), &mut dma);
    assert!(fdc.interrupt_requested());
    fdc.read(reg::COMMAND_STATUS);
    assert!(!fdc.interrupt_requested(), "reading the status must acknowledge /INTRQ");
}

#[test]
fn seek_moves_to_target_track() {
    let mut fdc = Wd1772::new();
    let mut disk = disk_1_track_1_side_9_sectors();
    let mut dma = TestDma::new();
    fdc.write_simple_register(reg::DATA, 5);
    fdc.execute_command(0b0001_0000, Some(&mut disk)); // Seek
    run_to_completion(&mut fdc, Some(&mut disk), &mut dma);
    assert_eq!(fdc.read(reg::TRACK), 5);
}

#[test]
fn step_in_and_step_out_update_track() {
    let mut fdc = Wd1772::new();
    let mut disk = disk_1_track_1_side_9_sectors();
    let mut dma = TestDma::new();

    // Step-In with u=1 (bit4): track must increase.
    fdc.execute_command(0b0101_0000, Some(&mut disk));
    run_to_completion(&mut fdc, Some(&mut disk), &mut dma);
    assert_eq!(fdc.read(reg::TRACK), 1);
    fdc.execute_command(0b0101_0000, Some(&mut disk));
    run_to_completion(&mut fdc, Some(&mut disk), &mut dma);
    assert_eq!(fdc.read(reg::TRACK), 2);

    // Step-Out with u=1: track must decrease.
    fdc.execute_command(0b0111_0000, Some(&mut disk));
    run_to_completion(&mut fdc, Some(&mut disk), &mut dma);
    assert_eq!(fdc.read(reg::TRACK), 1);
}

#[test]
fn step_without_u_does_not_modify_track_register() {
    let mut fdc = Wd1772::new();
    let mut disk = disk_1_track_1_side_9_sectors();
    let mut dma = TestDma::new();
    fdc.execute_command(0b0101_0000, Some(&mut disk)); // Step-In, u=1 -> track=1
    run_to_completion(&mut fdc, Some(&mut disk), &mut dma);
    assert_eq!(fdc.read(reg::TRACK), 1);
    fdc.execute_command(0b0010_0000, Some(&mut disk)); // Step (replays in), u=0
    run_to_completion(&mut fdc, Some(&mut disk), &mut dma);
    assert_eq!(fdc.read(reg::TRACK), 1, "without u, the track register does not change");
}

#[test]
fn read_sector_transfers_requested_sector_via_dma() {
    let mut fdc = Wd1772::new();
    let mut disk = disk_1_track_1_side_9_sectors();
    let mut dma = TestDma::new();
    fdc.write_simple_register(reg::SECTOR, 3);
    fdc.execute_command(0b1000_0000, Some(&mut disk)); // Read Sector, m=0
    run_to_completion(&mut fdc, Some(&mut disk), &mut dma);

    assert_eq!(dma.written.len(), SECTOR_SIZE);
    assert_eq!(dma.written[0], 3, "sector 3 starts with byte 3 (test pattern)");
    assert!(fdc.interrupt_requested());
}

#[test]
fn write_sector_writes_to_disk_via_dma() {
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
fn write_sector_refuses_when_disk_is_write_protected() {
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
    assert_eq!(unchanged, original, "the write must not have taken place");
}

#[test]
fn read_sector_multiple_reads_several_consecutive_sectors() {
    let mut fdc = Wd1772::new();
    let mut disk = disk_1_track_1_side_9_sectors();
    let mut dma = TestDma::new();
    fdc.write_simple_register(reg::SECTOR, 1);
    fdc.execute_command(0b1001_0000, Some(&mut disk)); // Read Sector, m=1
    run_to_completion(&mut fdc, Some(&mut disk), &mut dma);

    assert_eq!(dma.written.len(), SECTOR_SIZE * 9, "must have read all 9 sectors of the track");
    for s in 0..9 {
        assert_eq!(dma.written[s * SECTOR_SIZE], (s + 1) as u8);
    }
}

/// A disk with two tracks of different sizes (10 and 12 sectors) — like
/// a protected `.stx` image where `sectors_per_track()` (the global
/// maximum, here 12, track 1) does not reflect the actual count of
/// each individual track (see the docs on `FloppyDisk::sectors_on_track`).
/// Used to check that `Wd1772::finish_sector_transfer` correctly bounds
/// the continuation of a multiple read (bit M) to the ACTUAL count of the
/// target track, not to that global maximum.
struct HeterogeneousTrackDisk;

impl FloppyDisk for HeterogeneousTrackDisk {
    fn num_tracks(&self) -> u8 {
        2
    }
    fn num_sides(&self) -> u8 {
        1
    }
    fn sectors_per_track(&self) -> u8 {
        12 // global maximum (track 1), NOT the actual count of track 0
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
fn multiple_read_stops_at_actual_track_count_not_global_maximum() {
    let mut fdc = Wd1772::new();
    let mut disk = HeterogeneousTrackDisk;
    let mut dma = TestDma::new();
    fdc.write_simple_register(reg::SECTOR, 1);
    fdc.execute_command(0b1001_0000, Some(&mut disk)); // Read Sector, m=1, track 0 (10 actual sectors)
    run_to_completion(&mut fdc, Some(&mut disk), &mut dma);

    assert_eq!(
        dma.written.len(),
        SECTOR_SIZE * 10,
        "must stop at the 10 actual sectors of the track, not continue up to the global maximum (12)"
    );
    assert_eq!(
        fdc.read(reg::COMMAND_STATUS) & status::SEEK_ERROR_OR_RECORD_NOT_FOUND,
        0,
        "a normal end of track must not be reported as an error"
    );
}

#[test]
fn nonexistent_sector_raises_record_not_found() {
    let mut fdc = Wd1772::new();
    let mut disk = disk_1_track_1_side_9_sectors();
    let mut dma = TestDma::new();
    fdc.write_simple_register(reg::SECTOR, 99); // does not exist (9 sectors max)
    fdc.execute_command(0b1000_0000, Some(&mut disk));
    run_to_completion(&mut fdc, Some(&mut disk), &mut dma);

    assert_eq!(
        fdc.read(reg::COMMAND_STATUS) & status::SEEK_ERROR_OR_RECORD_NOT_FOUND,
        status::SEEK_ERROR_OR_RECORD_NOT_FOUND
    );
}

#[test]
fn without_disk_raises_not_ready() {
    let mut fdc = Wd1772::new();
    // Immediate (no tick needed): `NOT_READY` is a hardware line read
    // right away, not a simulated delay for a command that couldn't
    // execute anyway.
    fdc.execute_command(0b1000_0000, None::<&mut RawDiskImage>);
    assert!(fdc.interrupt_requested(), "checked before reading the status (which acknowledges /INTRQ)");
    assert_eq!(fdc.read(reg::COMMAND_STATUS) & status::NOT_READY, status::NOT_READY);
}

#[test]
fn force_interrupt_clears_busy() {
    let mut fdc = Wd1772::new();
    let mut disk = disk_1_track_1_side_9_sectors();
    fdc.execute_command(0b1101_0001, Some(&mut disk)); // Type IV, I0=1
    assert!(fdc.interrupt_requested(), "checked before reading the status (which acknowledges /INTRQ)");
    assert_eq!(fdc.read(reg::COMMAND_STATUS) & status::BUSY, 0);
}

#[test]
fn force_interrupt_interrupts_a_command_in_progress() {
    // Type IV must be immediate even if a Type I/II command is currently
    // executing (BUSY) — this is precisely the intended mechanism to
    // avoid having to wait for a long command to finish.
    let mut fdc = Wd1772::new();
    let mut disk = disk_1_track_1_side_9_sectors();
    fdc.write_simple_register(reg::DATA, 40);
    fdc.execute_command(0b0001_0000, Some(&mut disk)); // Seek(40), several tens of ms
    assert!(fdc.busy(), "the command must be in progress, not finished instantly");

    fdc.execute_command(0b1101_0001, Some(&mut disk)); // Force Interrupt, I0=1
    assert!(!fdc.busy(), "Force Interrupt must interrupt immediately");
    assert!(fdc.interrupt_requested());
}

#[test]
fn type1_command_takes_a_real_nonzero_amount_of_time() {
    // Core of the fix: a command must no longer finish before even the
    // first `tick()` — otherwise software polling BUSY would never see
    // it, and floppy disk emulation would be far too fast compared to
    // real hardware.
    let mut fdc = Wd1772::new();
    let mut disk = disk_1_track_1_side_9_sectors();
    fdc.write_simple_register(reg::DATA, 10);
    fdc.execute_command(0b0001_0000, Some(&mut disk)); // Seek(10)
    assert!(fdc.busy(), "BUSY must be observable right after issuing the command");
}

#[test]
fn single_step_sound_triggers_only_the_click() {
    // A single-track move (Step-In) plays only a sharp click, NOT the
    // seek hum — even if a burst of separate, closely spaced Step
    // commands follows (copy-protection re-reading track by track, a
    // real case observed on Rick_Dangerous.stx): empirically established
    // after testing under real conditions, layering the hum on TOP OF
    // EVERY isolated step makes such a burst sound like a continuous
    // blur instead of a distinct, regular train of clicks (see the docs
    // on `Wd1772::queue_step_sound`).
    let mut fdc = Wd1772::new();
    let mut disk = disk_1_track_1_side_9_sectors();
    let mut dma = TestDma::new();
    fdc.execute_command(0b0101_0000, Some(&mut disk)); // Step-In #1, u=1
    let events = fdc.take_sound_events();
    assert_eq!(events, vec![SoundEvent::MotorOn, SoundEvent::StepClick]);
    run_to_completion(&mut fdc, Some(&mut disk), &mut dma);
    let _ = fdc.take_sound_events();

    fdc.execute_command(0b0101_0000, Some(&mut disk)); // Step-In #2, closely spaced
    let events = fdc.take_sound_events();
    assert_eq!(events, vec![SoundEvent::StepClick], "still no hum, even in a burst");
}

#[test]
fn multi_track_seek_sound_hums_until_the_end() {
    let mut fdc = Wd1772::new();
    let mut disk = disk_1_track_1_side_9_sectors();
    let mut dma = TestDma::new();
    fdc.write_simple_register(reg::DATA, 10); // target 10 tracks away -> several steps
    fdc.execute_command(0b0001_0000, Some(&mut disk)); // Seek
    let events = fdc.take_sound_events();
    assert_eq!(events, vec![SoundEvent::MotorOn, SoundEvent::SeekStart]);

    run_to_completion(&mut fdc, Some(&mut disk), &mut dma);
    let events = fdc.take_sound_events();
    assert!(!events.contains(&SoundEvent::SeekEnd), "not yet, the margin after the end of the seek has not elapsed");

    fdc.tick(200_000 /* > SEEK_GRACE_CYCLES (20 ms = 160 000 cycles) */, Some(&mut disk), &mut dma);
    let events = fdc.take_sound_events();
    assert!(events.contains(&SoundEvent::SeekEnd), "the hum must stop once the margin after the end of the seek has passed");
}

#[test]
fn motor_stays_on_after_a_command_then_stops() {
    // Core behavior taken from Stay/Steem SSE: the motor does not switch
    // off instantly at the end of a command (unlike BUSY, which drops
    // immediately) — it keeps spinning a few more revolutions, which
    // software chaining several accesses quickly should not hear as a
    // restart every time.
    let mut fdc = Wd1772::new();
    let mut disk = disk_1_track_1_side_9_sectors();
    let mut dma = TestDma::new();
    fdc.write_simple_register(reg::SECTOR, 1);
    fdc.execute_command(0b1000_0000, Some(&mut disk)); // Read Sector
    run_to_completion(&mut fdc, Some(&mut disk), &mut dma);
    assert!(!fdc.busy(), "the command itself must be finished");
    let events = fdc.take_sound_events();
    assert!(!events.contains(&SoundEvent::MotorOff), "the motor must not switch off right away");

    // An immediately following command must NOT replay MotorOn (the
    // motor was already spinning).
    fdc.execute_command(0b1000_0000, Some(&mut disk));
    let events = fdc.take_sound_events();
    assert!(!events.contains(&SoundEvent::MotorOn), "no new MotorOn while the motor was already spinning");
    run_to_completion(&mut fdc, Some(&mut disk), &mut dma);
    let _ = fdc.take_sound_events();

    // Let it run long enough without a new command: the motor must
    // eventually stop (9 revolutions at 300 rpm ~ 1.8 s).
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
    assert!(saw_motor_off, "the motor must eventually stop after a period of inactivity");
}

#[test]
fn sector_by_sector_read_with_e_bit_zero_skips_head_load() {
    // Real bug fixed: TOS/GEMDOS chains its sector-by-sector reads with
    // the `E` bit (head load) at 0 — verified by tracing a real floppy
    // disk (Rick_Dangerous.stx) — precisely to SKIP this delay between
    // two consecutive reads on the same track. Applying it anyway (bug
    // of the old implementation) misaligned the search for the real
    // angular position of the next sector every time, forcing a wait of
    // about a full revolution (~217 ms measured in practice) per sector
    // instead of following the actual disk rotation. Reading ALL the
    // sectors of a track one by one (9 separate Read Sector commands,
    // as TOS really does) must therefore take about ONE disk revolution
    // in total, not nine.
    let mut fdc = Wd1772::new();
    let mut disk = disk_1_track_1_side_9_sectors(); // 9 sectors/track
    let mut dma = TestDma::new();

    let mut total_cycles = 0u32;
    for sector in 1..=9u8 {
        fdc.write_simple_register(reg::SECTOR, sector);
        fdc.execute_command(0b1000_0000, Some(&mut disk)); // Read Sector, m=0, E=0
        total_cycles += run_to_completion_cycles(&mut fdc, Some(&mut disk), &mut dma);
    }

    // One disk revolution at 300 rpm = 200 ms = 1,600,000 CPU cycles (8 MHz).
    const CYCLES_PER_REVOLUTION: u32 = 200 * 8_000;
    assert!(
        total_cycles < CYCLES_PER_REVOLUTION * 3 / 2,
        "9 sequential reads should not exceed ~1.5 disk revolutions: {total_cycles} cycles \
         (the old bug, which reloaded the head for every sector, took ~9x longer)"
    );
}

#[test]
fn raw_disk_image_round_trip_and_bounds() {
    let mut disk = RawDiskImage::new(vec![0u8; 2 * 9 * SECTOR_SIZE], 2, 1, 9);
    let mut sector = [0u8; SECTOR_SIZE];
    sector[0] = 0x42;
    disk.write_sector(1, 0, 5, &sector);
    assert_eq!(disk.read_sector(1, 0, 5).unwrap()[0], 0x42);

    assert!(disk.read_sector(2, 0, 1).is_none(), "track out of bounds");
    assert!(disk.read_sector(0, 0, 0).is_none(), "sector 0 does not exist (1-based)");
    assert!(disk.read_sector(0, 0, 10).is_none(), "sector out of bounds (9 max)");
}
