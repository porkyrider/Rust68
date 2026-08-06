//! Western Digital WD1772 — the Atari ST's floppy disk controller (FDC).
//!
//! Models the chip alone: 4 registers (Command/Status, Track, Sector,
//! Data) multiplexed via the A1-A0 lines, and the command set for
//! Type I (Restore/Seek/Step/Step-In/Step-Out), Type II (Read/Write
//! Sector) and Type IV (Force Interrupt), publicly documented by
//! Western Digital (WD1770/WD1772 datasheet, a standard command format
//! reused in countless independent technical references).
//!
//! The disk itself is abstracted through the [`FloppyDisk`] trait — this
//! module doesn't know how to read any particular file format (`.st`,
//! `.stx`…), it's up to the caller to provide an implementation (see
//! [`RawDiskImage`] for the raw `.st` format, linear sector layout).
//!
//! Type II data transfer (sector read/write) goes through
//! [`DmaChannel`], which the board implements to give access to its RAM at
//! the current DMA address — the WD1772 doesn't know about RAM, only the
//! disk and this channel.
//!
//! ## Real timing
//! Unlike a previous version (fully synchronous, `BUSY` never observable —
//! real software polling the status waiting for a command to finish then
//! ran much faster than on real silicon), [`Self::execute_command`] now
//! only STARTS the command (sets `BUSY`); it's [`Self::tick`], to be
//! called by the board on every clock advance (see
//! `systems::atari_st::AtariSt::tick`), that advances the real delay and
//! finishes the command (sets the final status, raises `/INTRQ`) once it
//! has elapsed. Constants verified against Hatari (`fdc.c`), the reference
//! used throughout this project for this kind of hardware value — see the
//! [`timing`] module.
//!
//! Simplifications assumed (see also the limitations below):
//! - Rotational latency before a target sector arrives under the head:
//!   [`Wd1772::cycles_to_target_sector`] follows an angular position of
//!   the disk (`rotation_phase`) that keeps advancing continuously at a
//!   constant speed while the motor is spinning, and computes the REAL
//!   delay until the targeted sector by assuming sectors are evenly
//!   spaced on the track (not a bit-exact MFM stream simulation like
//!   Hatari does, but more than a simple fixed average: a sequential
//!   sector-by-sector read, the most common case for the GEMDOS floppy
//!   driver, reaches a neighboring sector much faster than a randomly
//!   picked sector on the track, just like on real silicon).
//!   `timing::AVG_ROTATIONAL_LATENCY_CYCLES` is now only used as a
//!   fallback if the disk is absent at the time of the calculation.
//! - Head load (15 ms): conditioned on the `E` bit (bit 2) of the
//!   Type II/III command (see `COMMAND_BIT_HEAD_LOAD`), just like on real
//!   silicon/Hatari (`FDC_COMMAND_BIT_HEAD_LOAD`) — NOT a delay always
//!   applied as a previous version of this module assumed (bug fixed:
//!   verified that TOS chains its sector-by-sector reads with `E=0`,
//!   relying precisely on the absence of this delay; applying it anyway
//!   misaligned the search for the next sector's real angular position,
//!   ~217 ms/sector measured instead of the ~22 ms/sector expected on a
//!   9-sector track).
//! - Consecutive sectors of a multiple read/write (bit M): no new seek
//!   latency between two successive sectors (assumes standard formatting,
//!   contiguous sectors on the track) — the angular position still keeps
//!   advancing normally during the transfer, so a following command
//!   resumes from a consistent track position.
//!
//! ## Known limitations (v1)
//! - Type III (Read Address/Read Track/Write Track-Format) not
//!   implemented: command ignored, just sets `LOST_DATA`/`RECORD_NOT_FOUND`
//!   depending on the case to signal failure instead of crashing
//!   (immediate, no realistic delay simulated since it isn't implemented
//!   anyway).
//! - No verification (bit V) nor real CRC: a Seek command with V=1
//!   always succeeds (no simulated track ID read), the CRC error never
//!   triggers.
//! - The `TR00` signal (physical track 0 sensor) isn't modeled
//!   separately: the Track register is authoritative.
//!
//! ## Mechanical sound ([`SoundEvent`])
//! Each voice (spin-up/motor/click/seek, see
//! `peripherals::atari_st::drive_sound::DriveSound`) is MONOPHONIC: a
//! new trigger restarts the voice from the beginning instead of layering
//! it on top of the instance still playing (no real polyphony).
//! Limitations that follow from this:
//! - A single step (Step/StepIn/StepOut) triggers ONLY the click, never
//!   the seek rumble — unlike a multi-track Seek/Restore — even in a
//!   tight burst (copy-protection re-reading track by track): first tried
//!   faithful to Steem SSE (rumble on any movement, see the history), but
//!   found in practice to sound like a continuous blur rather than a
//!   train of distinct clicks with the WAV samples used here (see
//!   `queue_step_sound`).
//! - The click (`drive_click.wav`) is truncated to 60 ms on load (see
//!   `atari_st_sdl2.rs::load_drive_sound`) for the same reason: its
//!   natural resonance tail (~200 ms) would otherwise remain audible
//!   during a burst of tightly spaced steps (6-7 ms apart at the fastest
//!   step rate), each new click abruptly cutting off a tail still
//!   ringing.
//! - Only one drive (A) has sound — see `AtariSt::floppy_a`: drive B
//!   isn't modeled at all (always `NOT_READY`).

/// Real timing constants (the WD1772's reference clock is 8 MHz — the
/// same as the ST/STE CPU, which lets these delays be expressed directly
/// in CPU cycles without conversion), verified against Hatari (`fdc.c`).
pub mod timing {
    /// Minimum delay before a Type I command produces an observable
    /// effect (~0.09 ms, measured by Hatari on a real 520 STF) — avoids
    /// finishing instantly a command that has nothing to do (e.g. a Seek
    /// to the track already current).
    pub const TYPE_I_MIN_CYCLES: u32 = 90 * 8;

    /// Step rate (CPU cycles/track at 8 MHz) according to bits r1-r0
    /// (bits 0-1) of a Type I command — specific to the WD1772 (NOT the
    /// WD1770, which has a different table for the same bit encoding):
    /// 6, 12, 2, 3 ms.
    pub const STEP_RATE_CYCLES: [u32; 4] = [6 * 8_000, 12 * 8_000, 2 * 8_000, 3 * 8_000];

    /// Head load delay before a Type II (15 ms) — see the simplifications
    /// assumed in the module doc.
    pub const HEAD_LOAD_CYCLES: u32 = 15 * 8_000;

    /// Average rotational latency (half a revolution at 300 rpm) —
    /// fallback used only when the disk is no longer accessible at the
    /// time of computing the target sector position (see
    /// [`super::Wd1772::cycles_to_target_sector`], which follows the
    /// disk's real angular position in the normal case).
    pub const AVG_ROTATIONAL_LATENCY_CYCLES: u32 = 100 * 8_000;

    /// Transfer time of a 512-byte sector at 250 kbit/s (4 µs per bit,
    /// 8 bits per byte -> 256 CPU cycles at 8 MHz per byte).
    pub const CYCLES_PER_BYTE: u32 = 256;
    pub const SECTOR_TRANSFER_CYCLES: u32 = CYCLES_PER_BYTE * super::SECTOR_SIZE as u32;
}

/// Registers accessible via the A1-A0 lines (multiplexed by the DMA
/// controller on a real ST, see `systems::atari_st`).
pub mod reg {
    /// Write: command register. Read: status register.
    pub const COMMAND_STATUS: u8 = 0;
    pub const TRACK: u8 = 1;
    pub const SECTOR: u8 = 2;
    pub const DATA: u8 = 3;
}

/// Standard sector size (`.st`/GEMDOS format).
pub const SECTOR_SIZE: usize = 512;

/// Abstract view of a disk as accessed by the FDC: addressed by
/// track/side/sector, not by byte — this is the WD1772's native format
/// (it doesn't know about the image file's physical layout).
pub trait FloppyDisk {
    fn num_tracks(&self) -> u8;
    fn num_sides(&self) -> u8;
    fn sectors_per_track(&self) -> u8;
    fn write_protected(&self) -> bool;
    /// `None` if the track/side/sector doesn't exist on this disk.
    fn read_sector(&self, track: u8, side: u8, sector: u8) -> Option<[u8; SECTOR_SIZE]>;
    /// Silently ignored if the track/side/sector doesn't exist.
    fn write_sector(&mut self, track: u8, side: u8, sector: u8, data: &[u8; SECTOR_SIZE]);

    /// REAL number of sectors on the given track/side — by default
    /// identical to [`Self::sectors_per_track`] (uniform formats like
    /// `.st`, a single number for the whole disk), but can differ from one
    /// track to another for a format with per-track metadata (`.stx`:
    /// [`Self::sectors_per_track`] only returns a global maximum there,
    /// not necessarily the real count of THE targeted track — see
    /// `StxImage`). Used by [`Wd1772::cycles_to_target_sector`]: a wrong
    /// count progressively misaligns (sector after sector) the computed
    /// seek latency relative to the target sector's real angular
    /// position.
    fn sectors_on_track(&self, _track: u8, _side: u8) -> u8 {
        self.sectors_per_track()
    }

    /// True if this sector carries a deliberate CRC error in its ID
    /// (a common protection technique: a sector formatted with a
    /// knowingly wrong CRC, which a "clean" copy would recompute
    /// correctly and thus lose — see `StxImage`, where this is carried by
    /// `.stx` metadata). False by default (formats without this notion,
    /// like `.st`). Consulted after a successful read to set the
    /// [`status::CRC_ERROR`] bit — the data transfer happens anyway (the
    /// real FDC transfers what it decoded even when the CRC doesn't
    /// match, it just flags the anomaly).
    fn sector_has_crc_error(&self, _track: u8, _side: u8, _sector: u8) -> bool {
        false
    }

    /// REAL position (in bits, from the index pulse) of this sector's ID
    /// field on the original physical track, if known — carried by `.stx`
    /// metadata (`bit_position` of each SDR, see the `stx` module doc), as
    /// used by Hatari (`FDC_NextSectorID_FdcCycles_STX`, `BitPosition`).
    /// `None` by default (formats without this info, like `.st`):
    /// [`Wd1772::cycles_to_target_sector`] then falls back to uniform
    /// spacing estimated from [`Self::sectors_on_track`] — an
    /// approximation that can misalign the computed seek latency on a
    /// track with non-standard formatting (more sectors packed closer
    /// together than a normal track, a common protection technique — cf.
    /// `Rick_Dangerous.stx`, tracks with 10 sectors instead of the usual
    /// 9: the uniform-spacing assumption produced a wait of about a full
    /// revolution between two consecutive sectors instead of the real
    /// tenth of a revolution, measured ~2x slower than Hatari on a large
    /// sequential transfer).
    fn sector_bit_position(&self, _track: u8, _side: u8, _sector: u8) -> Option<u32> {
        None
    }
}

/// Raw disk image in `.st` format: a linear block of 512-byte sectors, in
/// track-by-track then side-by-side order
/// (`index = (track * sides + side) * sectors_per_track + sector`) — the
/// most common `.st` image format. Doesn't handle `.stx` (per-sector
/// protection metadata, out of scope for this module).
#[derive(Debug, Clone)]
pub struct RawDiskImage {
    data: Vec<u8>,
    tracks: u8,
    sides: u8,
    sectors_per_track: u8,
    write_protected: bool,
}

impl RawDiskImage {
    /// Builds an image from an already loaded buffer (exact size
    /// `tracks * sides * sectors_per_track * 512` expected).
    pub fn new(data: Vec<u8>, tracks: u8, sides: u8, sectors_per_track: u8) -> Self {
        RawDiskImage {
            data,
            tracks,
            sides,
            sectors_per_track,
            write_protected: false,
        }
    }

    pub fn set_write_protected(&mut self, protected: bool) {
        self.write_protected = protected;
    }

    fn offset(&self, track: u8, side: u8, sector: u8) -> Option<usize> {
        if track >= self.tracks || side >= self.sides || sector == 0 || sector > self.sectors_per_track
        {
            return None;
        }
        let index = (track as usize * self.sides as usize + side as usize)
            * self.sectors_per_track as usize
            + (sector - 1) as usize;
        let offset = index * SECTOR_SIZE;
        if offset + SECTOR_SIZE > self.data.len() {
            None
        } else {
            Some(offset)
        }
    }
}

impl FloppyDisk for RawDiskImage {
    fn num_tracks(&self) -> u8 {
        self.tracks
    }
    fn num_sides(&self) -> u8 {
        self.sides
    }
    fn sectors_per_track(&self) -> u8 {
        self.sectors_per_track
    }
    fn write_protected(&self) -> bool {
        self.write_protected
    }

    fn read_sector(&self, track: u8, side: u8, sector: u8) -> Option<[u8; SECTOR_SIZE]> {
        let offset = self.offset(track, side, sector)?;
        let mut buf = [0u8; SECTOR_SIZE];
        buf.copy_from_slice(&self.data[offset..offset + SECTOR_SIZE]);
        Some(buf)
    }

    fn write_sector(&mut self, track: u8, side: u8, sector: u8, data: &[u8; SECTOR_SIZE]) {
        if let Some(offset) = self.offset(track, side, sector) {
            self.data[offset..offset + SECTOR_SIZE].copy_from_slice(data);
        }
    }
}

/// Channel giving access to the board's RAM at the current DMA address,
/// for a Type II transfer. The WD1772 calls `pull`/`push` once per sector
/// byte; it's up to the board to advance its own DMA address counter on
/// each call (the WD1772 doesn't know about it).
pub trait DmaChannel {
    /// Reads the next byte from RAM (for Write Sector).
    fn pull(&mut self) -> u8;
    /// Writes the next byte to RAM (for Read Sector).
    fn push(&mut self, byte: u8);
}

/// Status register bits, common to both command families (their bit-level
/// meaning differs between Type I and Type II/III, documented at each
/// command's level).
pub mod status {
    pub const BUSY: u8 = 1 << 0;
    pub const INDEX_OR_DRQ: u8 = 1 << 1;
    pub const TRACK00_OR_LOST_DATA: u8 = 1 << 2;
    pub const CRC_ERROR: u8 = 1 << 3;
    pub const SEEK_ERROR_OR_RECORD_NOT_FOUND: u8 = 1 << 4;
    pub const HEAD_LOADED_OR_RECORD_TYPE: u8 = 1 << 5;
    pub const WRITE_PROTECT: u8 = 1 << 6;
    pub const NOT_READY: u8 = 1 << 7;
}

/// Trigger for the drive's mechanical sound — approach borrowed from the
/// companion project Stay (`stay-fdc`/`stay-sound`, itself modeled on
/// Steem SSE's `floppy_drive.cpp`): the WD1772 pushes events onto
/// [`Wd1772::sound_events`] on every relevant mechanical activity change,
/// without knowing anything about audio rendering (no dependency on a
/// sound module here) — it's up to the caller (the board, then the SDL2
/// binary) to consume them via [`Wd1772::take_sound_events`] to drive a
/// dedicated sample mixer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SoundEvent {
    MotorOn,
    MotorOff,
    /// Type I on a single track (Step/StepIn/StepOut, or a Seek/Restore
    /// that only moves by one track).
    StepClick,
    /// Type I on multiple tracks: looping rumble until the corresponding
    /// `SeekEnd`.
    SeekStart,
    SeekEnd,
}

/// Number of revolutions (at 300 rpm, ~200 ms each) the motor keeps
/// spinning after the end of the last command before stopping (and
/// emitting [`SoundEvent::MotorOff`]) — value borrowed from Stay/Steem SSE
/// (`MOTOR_OFF_PULSES`/`floppy_drive.cpp`), a real ST drive not cutting
/// its motor instantly after a single access.
const MOTOR_OFF_INDEX_PULSES: u32 = 9;
/// CPU cycles (8 MHz) for one disk revolution at 300 rpm — see
/// [`timing::AVG_ROTATIONAL_LATENCY_CYCLES`] for the calculation detail
/// (here a full revolution, not a half average latency).
const CYCLES_PER_REVOLUTION: u32 = 200 * 8_000;
/// Grace period before stopping the seek rumble (`SeekEnd`) after the end
/// of a Type I command that had triggered it — rather than cutting it
/// immediately on each individual command. Follows Steem SSE's real
/// behavior (`floppy_drive.cpp`, `SoundStep`/`SoundCheckIrq`): the seek
/// rumble accompanies ANY head movement, including a single-track
/// Step/StepIn/StepOut, and Steem only stops it if the track hasn't
/// changed since the last check — a burst of separate, closely spaced
/// Step commands (e.g. copy-protection re-reading track by track, ~6-12
/// ms apart depending on the programmed step rate) must therefore sound
/// like ONE continuous rumble, not a succession of cutoffs. 20 ms
/// comfortably covers the gap between two consecutive Step commands even
/// at the slowest step rate (12 ms), while staying well shorter than
/// `MOTOR_OFF_INDEX_PULSES` so it stops promptly once activity has
/// actually ended.
const SEEK_GRACE_CYCLES: u32 = 20 * 8_000;

/// Bit `E` (bit 2) of a Type II/III command: `0` = don't wait for head
/// load (already loaded), `1` = wait for [`timing::HEAD_LOAD_CYCLES`] —
/// see `Wd1772::start_type2`. Verified against Hatari (`fdc.h`,
/// `FDC_COMMAND_BIT_HEAD_LOAD`): the polarity is counter-intuitive (0 =
/// NO delay), not to be confused with the `V` bit (verify) of Type I
/// commands, which shares the same position but a different semantics.
const COMMAND_BIT_HEAD_LOAD: u8 = 1 << 2;

/// Current step of a command in progress (see the module doc on timing) —
/// `Idle` if no command is in flight.
#[derive(Debug, Clone, Copy)]
enum Phase {
    Idle,
    /// Type I: still physical steps to perform. `step_rate` (step speed,
    /// reused for each following step) is distinct from `cycles_left`
    /// (time remaining before the NEXT step).
    Stepping { remaining_steps: u16, cycles_left: u32, step_rate: u32, inward: bool, update_track: bool },
    /// Type II: head load before searching for the target sector.
    HeadLoad { command: u8, cycles_left: u32 },
    /// Type II: rotational latency before `self.sector` arrives under the
    /// head.
    Searching { command: u8, cycles_left: u32 },
    /// Type II: transfer of sector `self.sector` in progress.
    Transferring { command: u8, cycles_left: u32 },
}

/// Full state of a WD1772 controller.
#[derive(Debug, Clone)]
pub struct Wd1772 {
    status: u8,
    track: u8,
    sector: u8,
    data: u8,
    /// Current side (external signal, wired by the board — on a real ST,
    /// a bit of the YM2149's port A).
    pub side: u8,
    /// Last step direction used by Step (without an explicit u/d):
    /// `true` = towards higher tracks (Step-In), `false` = towards 0 (Step-Out).
    last_step_in: bool,
    /// `Some(vector)` if an interrupt (`/INTRQ`) is pending.
    intrq: bool,
    phase: Phase,
    /// Mechanical sound — see [`SoundEvent`]/[`Self::take_sound_events`].
    sound_events: Vec<SoundEvent>,
    /// True as long as the motor is considered spinning (independent of
    /// `BUSY`/`phase`: the motor keeps spinning for a while after the end
    /// of a command, see `motor_spin_down`).
    motor_on: bool,
    /// Cycles remaining before the motor stops (see `MOTOR_OFF_INDEX_PULSES`)
    /// once the last command has finished — `None` as long as no stop is
    /// scheduled (motor already off, or a command still in progress).
    motor_spin_down: Option<u32>,
    /// True as long as a seek rumble (`SeekStart`) is in progress, to know
    /// whether the end of a Type I command must emit the corresponding
    /// `SeekEnd`.
    seek_sound_active: bool,
    /// Cycles remaining before the seek rumble stops (see
    /// `SEEK_GRACE_CYCLES`) — `None` as long as no stop is scheduled
    /// (rumble already stopped, or a command still in progress).
    seek_spin_down: Option<u32>,
    /// Current angular position of the disk (CPU cycles elapsed since the
    /// last pass through the start of the track, `0..CYCLES_PER_REVOLUTION`)
    /// — see [`Self::cycles_to_target_sector`]. Only advances while the
    /// motor is spinning (`motor_on`); reset on every motor restart
    /// (rising edge, see `set_motor_on`) from `total_cycles` rather than a
    /// fixed value: on real silicon, the rotation phase at restart is in
    /// practice unpredictable, and a fixed reset would systematically
    /// align sector 1 right after the restart point — penalizing every
    /// time (nearly a full revolution of waiting) the very common
    /// "Restore then read boot sector" case instead of varying like on
    /// real silicon.
    rotation_phase: u32,
    /// Total CPU cycles seen by this controller since its creation (a
    /// monotonic wall clock, never reset — even with the motor off): only
    /// used as a deterministic-but-variable seed for `rotation_phase` on
    /// motor restart, avoiding introducing a dependency on a real random
    /// number generator for this minor detail.
    total_cycles: u64,
}

impl Default for Wd1772 {
    fn default() -> Self {
        Self::new()
    }
}

impl Wd1772 {
    pub fn new() -> Self {
        Wd1772 {
            status: 0,
            track: 0,
            sector: 0,
            data: 0,
            side: 0,
            last_step_in: true,
            intrq: false,
            phase: Phase::Idle,
            sound_events: Vec::new(),
            motor_on: false,
            motor_spin_down: None,
            seek_sound_active: false,
            seek_spin_down: None,
            rotation_phase: 0,
            total_cycles: 0,
        }
    }

    /// Empties and returns the queue of mechanical sound events
    /// accumulated since the last call — to be called by the board on
    /// every audio frame to drive a dedicated sample mixer (see
    /// [`SoundEvent`]).
    pub fn take_sound_events(&mut self) -> Vec<SoundEvent> {
        std::mem::take(&mut self.sound_events)
    }

    /// Schedules the next motor trigger (`SoundEvent::MotorOn` on the
    /// rising edge only — several consecutive commands while the motor is
    /// already spinning must not replay the spin-up sample every time)
    /// and cancels a scheduled stop (`motor_spin_down`) since the motor is
    /// being reused.
    fn set_motor_on(&mut self) {
        if !self.motor_on {
            self.sound_events.push(SoundEvent::MotorOn);
            self.rotation_phase = (self.total_cycles % CYCLES_PER_REVOLUTION as u64) as u32;
        }
        self.motor_on = true;
        self.motor_spin_down = None;
    }

    /// Schedules the motor to stop after [`MOTOR_OFF_INDEX_PULSES`]
    /// revolutions — called at the end of EVERY command (success or
    /// failure), not just the last access of a session: a following
    /// command before the deadline simply cancels this delay via
    /// `set_motor_on`.
    fn schedule_motor_off(&mut self) {
        if self.motor_on {
            self.motor_spin_down = Some(MOTOR_OFF_INDEX_PULSES * CYCLES_PER_REVOLUTION);
        }
    }

    /// Emits the sound appropriate for a Type I movement of `distance`
    /// tracks (0 = already on target, nothing to play; 1 = a sharp click,
    /// accompanied by the seek rumble like on Steem SSE (see
    /// `SEEK_GRACE_CYCLES`); more = just the rumble, until the end of the
    /// movement).
    fn queue_step_sound(&mut self, distance: u16) {
        match distance {
            0 => {}
            // A single step triggers ONLY the click, not the seek rumble —
            // unlike Steem SSE, whose `SoundStep` does both on every
            // Step-family command with real movement (see this file's
            // history). Empirical finding after testing under real
            // conditions: with the WAV samples used here, layering the
            // rumble on EVERY single step makes a burst of separate Step
            // commands (copy-protection re-reading track by track) sound
            // like a continuous blur rather than a distinct, regular train
            // of clicks — preferred by the user on trial. See
            // `ensure_seek_bed_active`, still used for the `_` case (a
            // real multi-track Seek).
            1 => self.sound_events.push(SoundEvent::StepClick),
            _ => self.ensure_seek_bed_active(),
        }
    }

    /// Starts the seek rumble if it isn't already running (no new
    /// `SeekStart` — so no audible restart of the loop — if a previous
    /// command had already triggered it) and cancels any scheduled stop
    /// (`seek_spin_down`): a burst of closely spaced Step commands must
    /// extend the same rumble, not restart it.
    fn ensure_seek_bed_active(&mut self) {
        if !self.seek_sound_active {
            self.sound_events.push(SoundEvent::SeekStart);
            self.seek_sound_active = true;
        }
        self.seek_spin_down = None;
    }

    fn end_seek_sound_if_active(&mut self) {
        if self.seek_sound_active {
            self.sound_events.push(SoundEvent::SeekEnd);
            self.seek_sound_active = false;
        }
        self.seek_spin_down = None;
    }

    /// Schedules the seek rumble to stop after [`SEEK_GRACE_CYCLES`]
    /// rather than stopping it immediately at the end of EVERY Type I
    /// command — see this constant's doc.
    fn schedule_seek_off(&mut self) {
        if self.seek_sound_active {
            self.seek_spin_down = Some(SEEK_GRACE_CYCLES);
        }
    }

    /// Reads logical register `r` (see [`reg`]). Reading the status
    /// acknowledges `/INTRQ` (real WD1772 behavior).
    pub fn read(&mut self, r: u8) -> u8 {
        match r {
            reg::COMMAND_STATUS => {
                self.intrq = false;
                self.status
            }
            reg::TRACK => self.track,
            reg::SECTOR => self.sector,
            reg::DATA => self.data,
            _ => 0xFF,
        }
    }

    /// Writes logical register `r`. Writing to `COMMAND_STATUS` executes
    /// the command — but only the simple registers (Track/Sector/Data) are
    /// handled here; the board must call [`Self::execute_command`]
    /// separately for that case (it needs access to the disk that this
    /// method doesn't have).
    pub fn write_simple_register(&mut self, r: u8, value: u8) {
        match r {
            reg::TRACK => self.track = value,
            reg::SECTOR => self.sector = value,
            reg::DATA => self.data = value,
            _ => {}
        }
    }

    /// True if `/INTRQ` is currently active (to be relayed by the board to
    /// its own interrupt mechanism — host-system-specific wiring, not
    /// modeled here).
    pub fn interrupt_requested(&self) -> bool {
        self.intrq
    }

    /// True as long as a command is in progress (see the module doc on
    /// timing) — observable by software polling the status, unlike a
    /// previous fully synchronous version.
    pub fn busy(&self) -> bool {
        self.status & status::BUSY != 0
    }

    /// Starts command `command` (the value written to `COMMAND_STATUS`).
    /// Sets `BUSY` and schedules the real timing (see the module doc);
    /// it's [`Self::tick`] that finishes the command once the delay has
    /// elapsed. `disk` is `None` to simulate a drive with no disk
    /// (`NOT_READY`, immediate).
    pub fn execute_command<D: FloppyDisk + ?Sized>(&mut self, command: u8, disk: Option<&mut D>) {
        self.intrq = false;
        if command & 0xF0 == 0xD0 {
            // Type IV: Force Interrupt — immediately interrupts any
            // command in progress and generates /INTRQ if one of the
            // I0-I3 conditions is selected. Always immediate on real
            // silicon (this is precisely the mechanism for not having to
            // wait for a command in progress to finish).
            self.phase = Phase::Idle;
            self.status &= !status::BUSY;
            if command & 0x0F != 0 {
                self.intrq = true;
            }
            // An ongoing seek rumble must not stay stuck looping if Force
            // Interrupt cuts the command before its normal end.
            self.end_seek_sound_if_active();
            self.schedule_motor_off();
            return;
        }

        let Some(disk) = disk else {
            self.phase = Phase::Idle;
            self.status = status::NOT_READY;
            self.intrq = true;
            return;
        };

        self.status = status::BUSY;

        if command & 0x80 == 0 {
            self.start_type1(command);
        } else if command & 0xC0 == 0x80 {
            self.start_type2(command, disk);
        } else {
            // Type III (Read Address/Read Track/Write Track): not
            // implemented (cf. limitations) — signals failure immediately
            // rather than simulating a realistic delay for a command we
            // don't actually execute.
            self.phase = Phase::Idle;
            self.status = status::SEEK_ERROR_OR_RECORD_NOT_FOUND | self.write_protect_bit(disk);
            self.intrq = true;
        }
    }

    /// Advances the timing of a command in progress by `cycles` CPU
    /// cycles — to be called by the board on every clock advance (see the
    /// module doc). Also advances the scheduled motor stop (see
    /// `motor_spin_down`) even without a command in flight (`Phase::Idle`):
    /// the motor keeps spinning for a while after the last command,
    /// independently of `BUSY`.
    pub fn tick<D: FloppyDisk + ?Sized>(
        &mut self,
        mut cycles: u32,
        mut disk: Option<&mut D>,
        dma: &mut impl DmaChannel,
    ) {
        let elapsed = cycles;
        self.total_cycles += elapsed as u64;
        if let Some(remaining) = self.motor_spin_down.as_mut() {
            if cycles >= *remaining {
                self.motor_spin_down = None;
                self.motor_on = false;
                self.sound_events.push(SoundEvent::MotorOff);
            } else {
                *remaining -= cycles;
            }
        }
        if let Some(remaining) = self.seek_spin_down.as_mut() {
            if cycles >= *remaining {
                self.seek_spin_down = None;
                self.end_seek_sound_if_active();
            } else {
                *remaining -= cycles;
            }
        }
        // Disk angular position: advances exactly once per call, by the
        // REAL number of elapsed cycles (independently of what the
        // `Phase` state machine below is doing) — a real disk keeps
        // spinning at constant speed, whether or not the FDC is busy
        // searching for a sector — see `cycles_to_target_sector`.
        if self.motor_on {
            self.rotation_phase =
                ((self.rotation_phase as u64 + elapsed as u64) % CYCLES_PER_REVOLUTION as u64) as u32;
        }
        // Loop: a single `cycles` advance can cross several steps (e.g.
        // several steps of a long-distance Seek in a single `tick` if the
        // CPU executes a long instruction) — each iteration consumes
        // cycles until exhaustion or until returning to `Idle`.
        while cycles > 0 {
            match &mut self.phase {
                Phase::Idle => return,
                Phase::Stepping { remaining_steps, cycles_left, step_rate, inward, update_track } => {
                    if cycles < *cycles_left {
                        *cycles_left -= cycles;
                        return;
                    }
                    cycles -= *cycles_left;
                    let (mut remaining_steps, step_rate, inward, update_track) =
                        (*remaining_steps, *step_rate, *inward, *update_track);
                    if remaining_steps > 0 {
                        // A physical step has just completed.
                        if update_track {
                            self.track =
                                if inward { self.track.saturating_add(1) } else { self.track.saturating_sub(1) };
                        }
                        remaining_steps -= 1;
                    }
                    if remaining_steps == 0 {
                        self.phase = Phase::Idle;
                        self.finish_type1(disk.as_deref());
                    } else {
                        // Re-arms for the next step, at the same speed.
                        self.phase =
                            Phase::Stepping { remaining_steps, cycles_left: step_rate, step_rate, inward, update_track };
                    }
                }
                Phase::HeadLoad { command, cycles_left } => {
                    if cycles < *cycles_left {
                        *cycles_left -= cycles;
                        return;
                    }
                    cycles -= *cycles_left;
                    let command = *command;
                    let wait = disk
                        .as_deref()
                        .map(|d| self.cycles_to_target_sector(d))
                        .unwrap_or(timing::AVG_ROTATIONAL_LATENCY_CYCLES);
                    self.phase = Phase::Searching { command, cycles_left: wait };
                }
                Phase::Searching { command, cycles_left } => {
                    if cycles < *cycles_left {
                        *cycles_left -= cycles;
                        return;
                    }
                    cycles -= *cycles_left;
                    let command = *command;
                    if let Some(d) = disk.as_deref() {
                        if d.read_sector(self.track, self.side, self.sector).is_none() && command & 0x20 == 0 {
                            // Read only: a nonexistent sector is detected
                            // right at the search stage (no ID found), no
                            // need to wait for the transfer time.
                            self.phase = Phase::Idle;
                            self.status = status::SEEK_ERROR_OR_RECORD_NOT_FOUND;
                            self.intrq = true;
                            self.schedule_motor_off();
                            continue;
                        }
                    }
                    self.phase = Phase::Transferring { command, cycles_left: timing::SECTOR_TRANSFER_CYCLES };
                }
                Phase::Transferring { command, cycles_left } => {
                    if cycles < *cycles_left {
                        *cycles_left -= cycles;
                        return;
                    }
                    cycles -= *cycles_left;
                    let command = *command;
                    self.finish_sector_transfer(command, disk.as_deref_mut(), dma);
                }
            }
        }
    }

    fn write_protect_bit<D: FloppyDisk + ?Sized>(&self, disk: &D) -> u8 {
        if disk.write_protected() {
            status::WRITE_PROTECT
        } else {
            0
        }
    }

    /// CPU cycles until sector `self.sector` arrives under the head,
    /// assuming `sectors_per_track` sectors evenly spaced on the track
    /// (standard formatting, the same assumption as the already-documented
    /// "no new search between consecutive sectors of a multiple transfer")
    /// — based on the disk's real angular position (`rotation_phase`)
    /// rather than a fixed average, as Hatari does
    /// (`FDC_NextSectorID_FdcCycles_*`): a sector that has just passed
    /// under the head, or close to the last access in the track's
    /// physical order, therefore arrives much faster than a randomly
    /// picked sector — exactly what the old fixed average was missing for
    /// sequential sector-by-sector reads (the most common case for the
    /// GEMDOS floppy driver), which it penalized uniformly at the worst
    /// case.
    fn cycles_to_target_sector<D: FloppyDisk + ?Sized>(&self, disk: &D) -> u32 {
        // Real position captured on the original medium (`.stx`:
        // `bit_position` per sector, see the doc of
        // `FloppyDisk::sector_bit_position`) — takes priority over the
        // uniform-spacing estimate below, which wrongly assumes that any
        // track divides the full revolution into equal portions (false
        // for non-standard formatting, e.g. an "extra sector" — measured
        // ~2x slower than Hatari on `Rick_Dangerous.stx` before this fix:
        // each consecutive sector of a 10-sector track was waiting ~1 full
        // revolution instead of the real 1/10th of a revolution).
        if let Some(bit_pos) = disk.sector_bit_position(self.track, self.side, self.sector) {
            let target_position = bit_pos * (timing::CYCLES_PER_BYTE / 8);
            return (target_position + CYCLES_PER_REVOLUTION
                - self.rotation_phase % CYCLES_PER_REVOLUTION)
                % CYCLES_PER_REVOLUTION;
        }
        let sectors_per_track = disk.sectors_on_track(self.track, self.side);
        if sectors_per_track == 0 {
            return timing::AVG_ROTATIONAL_LATENCY_CYCLES;
        }
        let sector_period = CYCLES_PER_REVOLUTION / sectors_per_track as u32;
        // Sectors numbered starting from 1 on real silicon.
        let target_position =
            (self.sector.saturating_sub(1) as u32 % sectors_per_track as u32) * sector_period;
        (target_position + CYCLES_PER_REVOLUTION - self.rotation_phase % CYCLES_PER_REVOLUTION)
            % CYCLES_PER_REVOLUTION
    }

    fn start_type1(&mut self, command: u8) {
        let step_rate = timing::STEP_RATE_CYCLES[(command & 0x03) as usize];
        let (remaining_steps, inward, update_track): (u16, bool, bool) = match command >> 4 {
            0b0000 => {
                // Restore: brings the head back to track 0, one step at a
                // time from the current position (at least 1 step, like
                // on real silicon which always checks at least once).
                (self.track.max(1) as u16, false, true)
            }
            0b0001 => {
                // Seek: the target is in the Data register.
                let target = self.data;
                let inward = target > self.track;
                self.last_step_in = inward;
                let steps = (target as i16 - self.track as i16).unsigned_abs();
                (steps, inward, true)
            }
            0b0010 | 0b0011 => (1, self.last_step_in, command & 0x10 != 0),
            0b0100 | 0b0101 => {
                self.last_step_in = true;
                (1, true, command & 0x10 != 0)
            }
            0b0110 | 0b0111 => {
                self.last_step_in = false;
                (1, false, command & 0x10 != 0)
            }
            _ => (0, false, false),
        };
        self.set_motor_on();
        self.queue_step_sound(remaining_steps);
        let cycles_left = if remaining_steps == 0 { timing::TYPE_I_MIN_CYCLES } else { step_rate };
        self.phase = Phase::Stepping { remaining_steps, cycles_left, step_rate, inward, update_track };
    }

    fn finish_type1<D: FloppyDisk + ?Sized>(&mut self, disk: Option<&D>) {
        self.status = 0;
        if self.track == 0 {
            self.status |= status::TRACK00_OR_LOST_DATA;
        }
        if let Some(disk) = disk {
            self.status |= self.write_protect_bit(disk);
        }
        self.intrq = true;
        self.schedule_seek_off();
        self.schedule_motor_off();
    }

    fn start_type2<D: FloppyDisk + ?Sized>(&mut self, command: u8, disk: &mut D) {
        let is_write = command & 0x20 != 0;
        if is_write && disk.write_protected() {
            // Detected immediately (hardware WPRT line), no need to wait
            // for head load to signal it.
            self.phase = Phase::Idle;
            self.status = status::WRITE_PROTECT;
            self.intrq = true;
            self.schedule_motor_off();
            return;
        }
        self.set_motor_on();
        if command & COMMAND_BIT_HEAD_LOAD != 0 {
            self.phase = Phase::HeadLoad { command, cycles_left: timing::HEAD_LOAD_CYCLES };
        } else {
            // Bit E=0: the software explicitly requests SKIPPING the head
            // load delay (real silicon: the head is already loaded,
            // typically several consecutive reads on the same track
            // without a Seek in between) — straight to searching.
            // Empirically verified essential: without this bit, the
            // GEMDOS floppy driver (which chains simple Read Sector
            // commands with E=0, NOT the multiple bit) wrongly incurs a
            // renewed head delay on every sector, which misaligns the
            // search for the next sector's real angular position and
            // makes it wait almost a full revolution every time —
            // measured ~217 ms/sector instead of the ~22 ms/sector
            // expected on a 9-sector track.
            let wait = self.cycles_to_target_sector(disk);
            self.phase = Phase::Searching { command, cycles_left: wait };
        }
    }

    /// Finishes the transfer of the current sector (once its transfer
    /// time has elapsed): performs the actual read/write, then either
    /// chains onto the next sector (bit M) without a new search
    /// (contiguous sectors, see the module doc) or finishes the command.
    fn finish_sector_transfer<D: FloppyDisk + ?Sized>(
        &mut self,
        command: u8,
        disk: Option<&mut D>,
        dma: &mut impl DmaChannel,
    ) {
        let is_write = command & 0x20 != 0;
        let multiple = command & 0x10 != 0;
        let Some(disk) = disk else {
            self.phase = Phase::Idle;
            self.status = status::NOT_READY;
            self.intrq = true;
            self.schedule_motor_off();
            return;
        };

        let mut crc_error = false;
        if is_write {
            let mut buf = [0u8; SECTOR_SIZE];
            for b in buf.iter_mut() {
                *b = dma.pull();
            }
            disk.write_sector(self.track, self.side, self.sector, &buf);
        } else {
            match disk.read_sector(self.track, self.side, self.sector) {
                Some(buf) => {
                    for &b in buf.iter() {
                        dma.push(b);
                    }
                    // Transferred anyway (the real FDC transfers what it
                    // decoded even when the CRC doesn't match), but
                    // flagged in the final status below — and, like the
                    // real WD1772, this interrupts a multi-sector read
                    // (bit M) instead of chaining onto the next one.
                    crc_error = disk.sector_has_crc_error(self.track, self.side, self.sector);
                }
                None => {
                    self.phase = Phase::Idle;
                    self.status = status::SEEK_ERROR_OR_RECORD_NOT_FOUND;
                    self.intrq = true;
                    self.schedule_motor_off();
                    return;
                }
            }
        }

        if multiple && !crc_error && self.sector < disk.sectors_on_track(self.track, self.side) {
            self.sector += 1;
            self.phase = Phase::Transferring { command, cycles_left: timing::SECTOR_TRANSFER_CYCLES };
        } else {
            self.phase = Phase::Idle;
            self.status = self.write_protect_bit(disk);
            if crc_error {
                self.status |= status::CRC_ERROR;
            }
            self.intrq = true;
            self.schedule_motor_off();
        }
    }
}
