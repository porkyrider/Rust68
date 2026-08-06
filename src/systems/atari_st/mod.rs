//! Atari ST board: real memory map + MFP/GLUE → IPL wiring.
//!
//! Implements [`crate::Bus`] for a minimal ST/STE: installed RAM at
//! `0x000000`, TOS ROM at `0xFC0000`, MFP 68901 at odd addresses
//! `0xFFFA01`-`0xFFFA2F`. On real silicon, the MMU always responds with
//! /DTACK across the whole "ST RAM" address space (4 MB, see
//! [`ST_RAM_ADDRESS_SPACE`]) — even beyond the physically installed RAM,
//! an access in this range **never** triggers a bus error (unlike the
//! real "hole" between 4 MB and the start of the I/O area, `0xFF8000`,
//! which does trigger a bus error via [`crate::Bus::take_bus_fault`] —
//! a mechanism many programs/demos use to detect installed RAM once TOS
//! has started). Beyond the actually installed RAM but within the 4 MB,
//! an access "floats": modeled here as a fixed, non-stored value (never
//! what was just written), rather than the real value (residual bus
//! capacitance from the last cycle, not reproducible in a deterministic
//! emulator) — it is precisely this lack of persistence, not some
//! address-folding story, that TOS observes for its own RAM detection at
//! the very start of a cold boot (write a pattern, read it back, conclude
//! "no RAM here" if it doesn't match).
//!
//! Real ST/STE interrupt wiring, in decreasing priority order:
//! MFP → IPL6, VBL (GLUE) → IPL4, HBL (GLUE) → IPL2. The two ACIAs
//! (keyboard + MIDI) do not generate an IPL directly: their IRQ outputs
//! are OR-wired onto the MFP's `GPIP4` (real ST/STE wiring). The WD1772
//! (`/INTRQ`) is wired onto `GPIP5` (real ST/STE wiring).
//!
//! The Shifter (video) is driven by the GLUE's HBL/VBL rhythm: `tick`
//! detects changes in `Glue::current_line`/`frame_count` and triggers
//! `Shifter::render_scanline`/`start_frame` accordingly, accumulating
//! the image in [`AtariSt::framebuffer`].
//!
//! ## Known limitations (v1)
//! - RAM/ROM/MFP/ACIA×2/YM2149/Shifter are actually mapped. The rest of
//!   the I/O area (`0xFF8000`-`0xFFFFFF`: FDC/DMA…) and the cartridge port
//!   (`0xFA0000`-`0xFBFFFF`) respond `0xFF` on reads and ignore writes —
//!   a real chip select but a peripheral not yet emulated (or, for the
//!   cartridge, absent), rather than a bus error that would break any
//!   status polling done by software or the cartridge probe the TOS ROM
//!   performs at boot (`cmpi.l #$fa52235f,$fa0000`).
//! - `rom_base` defaults to `DEFAULT_ROM_BASE` (`0xFC0000`, TOS <= 1.04);
//!   [`AtariSt::set_rom_base`] allows changing it to `0xE00000` for
//!   TOS >= 1.06 (256 KB, which wouldn't fit between `0xFC0000` and
//!   `IO_BASE` anyway). No simultaneous mirroring at both addresses.
//! - No DRAM/video contention model (`is_contended` stays `false`): the
//!   Shifter is now implemented but its memory access is not (yet)
//!   modeled as stealing bus cycles from the CPU.
//! - Even addresses adjacent to an MFP register (e.g. `0xFFFA00`,
//!   normally floating on a real 8-bit bus) fall through to the generic
//!   I/O stub instead of precisely modeling UDS/LDS decoding behavior.
//! - `AtariSt::tick` must be called explicitly by the caller after each
//!   `Cpu::step` (this crate does not advance the peripherals on its
//!   own) — see the example on `tick`.
//! - Simplified DMA/WD1772 registers: the FDC register selector
//!   (`DMA_MODE`, bits 0-1) is modeled, but not the real sector-count
//!   register or the real FDC/HDC selection of the ST DMA controller —
//!   our "instantaneous per-sector" transfer model doesn't functionally
//!   need it (see `peripherals::atari_st::wd1772` for details).

pub mod model;

use crate::peripherals::atari_st::acia::{self, Acia};
use crate::peripherals::atari_st::blitter::{self, Blitter};
use crate::peripherals::atari_st::dma_sound::{self, DmaSound};
use crate::peripherals::atari_st::microwire::Microwire;
use crate::peripherals::atari_st::glue::{Glue, VideoMode};
use crate::peripherals::atari_st::ikbd::Ikbd;
use crate::peripherals::atari_st::mfp::Mfp;
use crate::peripherals::atari_st::shifter::{self, Shifter};
use crate::peripherals::atari_st::wd1772::{self, DmaChannel, FloppyDisk, SECTOR_SIZE, Wd1772};
use crate::peripherals::atari_st::ym2149::{self, Ym2149};
use crate::{ADDR_MASK, Bus};

/// Address of the first MFP register (`GPIP`), on real ST/STE.
pub const MFP_BASE: u32 = 0xFFFA01;
/// Number of logical MFP registers (see `peripherals::atari_st::mfp::reg`).
const MFP_REG_COUNT: u32 = 24;
/// Address of the last MFP register (`UDR`).
pub const MFP_END: u32 = MFP_BASE + (MFP_REG_COUNT - 1) * 2;

/// Keyboard ACIA: control/status register, on real ST/STE.
pub const ACIA_KEYBOARD_CONTROL: u32 = 0xFFFC00;
/// Keyboard ACIA: data register.
pub const ACIA_KEYBOARD_DATA: u32 = 0xFFFC02;
/// MIDI ACIA: control/status register.
pub const ACIA_MIDI_CONTROL: u32 = 0xFFFC04;
/// MIDI ACIA: data register.
pub const ACIA_MIDI_DATA: u32 = 0xFFFC06;

/// YM2149: selector register (write = register choice, read = currently
/// selected register), on real ST/STE.
pub const YM2149_SELECT: u32 = 0xFF8800;
/// YM2149: data register of the currently selected register.
pub const YM2149_DATA: u32 = 0xFF8802;

/// WD1772: multiplexed register (Command/Status/Track/Sector/Data
/// depending on the `DMA_MODE` selector), on real ST/STE.
pub const FDC_DATA: u32 = 0xFF8604;
/// DMA controller: FDC register selector — a 16-bit register on real
/// silicon (bit 0, bits 9-15 unused; bits 1-2 = FDC register selector
/// A1-A0, see [`AtariSt::write16`]), confirmed by Hatari
/// (`fdc.c`: `FDC_reg = (FDC_DMA.Mode & 0x6) >> 1`). NOT bits 0-1 as a
/// classic 8-bit register would suggest — a word access (TOS's real
/// usage) places the selector in the LOW byte of the word, which only
/// explicit support for a full 16-bit access can see (a naive `write8`
/// decomposed byte by byte, like the rest of the bus, would only see the
/// high byte, always zero for these small values — the selector would
/// then stay permanently stuck at 0 no matter what TOS writes).
pub const DMA_MODE: u32 = 0xFF8606;
/// DMA address counter, high byte.
pub const DMA_ADDR_HIGH: u32 = 0xFF8609;
/// DMA address counter, middle byte.
pub const DMA_ADDR_MID: u32 = 0xFF860B;
/// DMA address counter, low byte.
pub const DMA_ADDR_LOW: u32 = 0xFF860D;

/// GLUE synchronization register (bit 1 = external 50/60Hz selection)
/// — see [`crate::peripherals::atari_st::glue::Glue::write_sync`] for the
/// STE vertical border removal that its cycle-position triggers.
pub const GLUE_SYNC: u32 = 0xFF820A;

/// Base of the Blitter registers, on real STE.
pub const BLITTER_BASE: u32 = 0xFF8A00;

/// Start of the general I/O area (ACIA, PSG, FDC, Shifter…) on ST/STE.
pub const IO_BASE: u32 = 0xFF8000;
/// End of the address space (24 bits).
pub const IO_END: u32 = 0x00FF_FFFF;

/// Cartridge port (external ROM, e.g. game cartridges). Unlike the
/// physical "hole" above installed RAM (which triggers a bus error, see
/// [`crate::Bus::take_bus_fault`]), an absent cartridge responds with a
/// floating read (`0xFF`) WITHOUT a bus error — the TOS boot ROM probes
/// precisely this area (`cmpi.l #$fa52235f,$fa0000`: cartridge signature)
/// to detect a cartridge without ever crashing if there isn't one.
pub const CARTRIDGE_BASE: u32 = 0xFA0000;
pub const CARTRIDGE_END: u32 = 0xFBFFFF;

/// DMA sound (STE) — see [`crate::peripherals::atari_st::dma_sound::DmaSound`]
/// (register-accurate, 8-bit PCM mono/stereo playback at the 4 hardware
/// frequencies, simplified: no 8-byte FIFO nor low-pass filter, see its
/// module doc). The Microwire/LMC1992 (`$FF8920`/`22`/`24`) is handled
/// separately, see [`crate::peripherals::atari_st::microwire::Microwire`].
/// Below `$FF8922` (Microwire DATA register), a `0x00` response (not
/// `0xFF` like the rest of the non-emulated I/O area) remains necessary
/// for addresses not mapped to a real register of these two peripherals:
/// TOS >= 1.62 (STE) writes then reads back the Microwire DATA register
/// in a loop, waiting for it to fall back to zero (end of serial shift)
/// at the very start of boot — with the generic `0xFF` response (bits
/// always 1), this wait would never end (see the comment on this special
/// case in [`AtariSt::read8`]).
pub const STE_DMA_SOUND_BASE: u32 = 0xFF8900;
pub const STE_DMA_SOUND_END: u32 = 0xFF893F;
/// Microwire DATA register (serial interface to the LMC1992 mixer) — see
/// the comment on its special read case in [`AtariSt::read8`].
const STE_MICROWIRE_DATA: u32 = 0xFF8922;
const STE_MICROWIRE_DATA1: u32 = 0xFF8923;
/// Microwire MASK register (serial shift mask) — see
/// [`crate::peripherals::atari_st::microwire::Microwire`].
const STE_MICROWIRE_MASK: u32 = 0xFF8924;
const STE_MICROWIRE_MASK1: u32 = 0xFF8925;

/// Usual base address of the TOS ROM (192 KB, TOS 1.x/2.x).
pub const DEFAULT_ROM_BASE: u32 = 0xFC0000;

/// Memory configuration register (MMU), on real ST/STE. Writing here
/// disables the ROM overlay at address 0 (see [`AtariSt::overlay`]) — TOS
/// does this very early at boot, right after probing the warmstart
/// cookie, to normally regain control of the low addresses once its own
/// bootstrap code has finished.
///
/// Bits 3-2 = announced logical size of bank 0, bits 1-0 = bank 1
/// (`00`=128 KB, `01`=512 KB, `10`=2 MB, `11`=reserved) — confirmed by
/// Hatari's source code (`stMemory.c`, `STMemory_MMU_ConfToBank`). TOS
/// writes it itself during RAM detection at cold boot; it actually
/// drives intra-bank address mirroring (see
/// [`AtariSt::translate_ram_addr`]) on STE, not just DRAM refresh timing.
pub const MEMORY_CONF: u32 = 0xFF8001;

/// Size of the "ST RAM" address space on real ST/STE (4 MB, two MMU
/// banks of 2 MB each), as opposed to the real "hole" beyond it, before
/// `IO_BASE` — see the doc of [`AtariSt::in_floating_st_ram`] for the
/// details of what triggers (or doesn't) a bus error in this range
/// depending on the access type (direct CPU vs DMA).
const ST_RAM_ADDRESS_SPACE: u32 = 4 * 1024 * 1024;

/// CPU cycles granted to the CPU between two non-HOG blit slices (64 bus
/// accesses, see `Blitter::execute`/`BUS_ACCESSES_PER_SLICE`), i.e. the
/// time the CPU has to run in parallel before the Blitter takes back
/// control — NOT the time the Blitter itself takes to process its slice.
/// Used to pace the Blitter's autonomous resumption in [`AtariSt::tick`]
/// at the same rate as real hardware, rather than a whole slice per CPU
/// instruction (far too fast).
///
/// Value taken from Hatari (`src/blitter.c`, `Blitter_Start`, non "cycle
/// exact" mode — the one matching our own model, without individual bus
/// access counting): the comment there is explicit —
/// "In non cycle exact mode, the blitter will have 64 bus accesses and the
/// cpu will run during 64*4 = 256 cpu cycles" — implemented via
/// `CycInt_AddRelativeInterrupt(BLITTER_NONHOG_BUS_CPU*4, ...)` with
/// `BLITTER_NONHOG_BUS_CPU = 64`, so indeed 256, not 64 (calibration bug
/// fixed here: a value of 64 let the Blitter take back control 4x more
/// often than on the reference hardware).
const BLITTER_SLICE_CYCLES: u32 = 256;

/// MFP GPIP pin wired to the "MONO DETECT" signal of the monitor
/// connector, on real ST/STE: a monochrome monitor pulls this signal to
/// ground (pin read as 0), while a color monitor (or the absence of a
/// monitor) leaves a pull-up resistor holding it high (pin read as 1).
/// TOS reads this pin very early at boot to choose between monochrome
/// high resolution mode (640×400 B&W) and the color modes (320×200/
/// 640×200) — without this wiring, the pin would stay at its default
/// state (0), and TOS would wrongly conclude a monochrome monitor is
/// connected. This board models a color monitor: the pin is therefore
/// held at 1 permanently, including after a software `/RESET`
/// (`Bus::reset_bus`) since the signal reflects an external physical
/// connection, not internal MFP state that `/RESET` would reset.
const GPIP_MONO_DETECT: u8 = 7;

/// Minimal Atari ST board: RAM + ROM + MFP 68901 + GLUE (HBL/VBL).
pub struct AtariSt {
    ram: Vec<u8>,
    rom: Vec<u8>,
    rom_base: u32,
    /// Cartridge ROM image (port `$FA0000`), if any — empty by default
    /// (free slot, reads return `0xFF` with no bus error, see
    /// [`Self::read8`]). See [`Self::load_cartridge`].
    cartridge: Vec<u8>,
    /// `RUST68_TRACE_IRQ=1` read only once at construction (not on every
    /// call — see the file history for the performance regression this
    /// caused when checked on every IACK).
    trace_irq: bool,
    /// CPU cycles accumulated since the last non-HOG blit slice processed
    /// (see [`Self::tick`]) — past [`BLITTER_SLICE_CYCLES`], a new slice
    /// is allowed. Without this throttling, a blit resumed on EVERY
    /// tick() (i.e. every CPU instruction) finishes in a handful of
    /// instructions instead of the real time silicon takes (256 CPU
    /// cycles left to the CPU between two slices in shared mode — see the
    /// doc of [`BLITTER_SLICE_CYCLES`] for the source).
    blitter_slice_cycle_acc: u32,
    /// STE DMA sound registers (`$FF8900`-`$FF893F`) — no real DMA audio
    /// emulation (STE sound stays silent), but a simple, faithful
    /// byte-by-byte read/write storage. Without this (a stub always
    /// returning `0x00` on read, writes ignored), any software testing
    /// for DMA sound presence via write-then-read-back (a standard
    /// hardware detection technique, used notably by diagnostic
    /// cartridges) loops indefinitely waiting for a value that never
    /// comes back.
    ste_dma_sound: [u8; (STE_DMA_SOUND_END - STE_DMA_SOUND_BASE + 1) as usize],
    /// DMA Sound controller (STE): reads 8-bit PCM samples from RAM.
    /// Public field for audio generation by the caller (see
    /// [`DmaSound::next_sample`]) — wired to registers `$FF8901`-`$FF8921`
    /// (offsets [`dma_sound::reg`]) in `read8`/`write8`, the rest of the
    /// `STE_DMA_SOUND_BASE..=STE_DMA_SOUND_END` range (including the
    /// Microwire) remaining a separate generic storage (see
    /// `ste_dma_sound` just above).
    pub dma_sound: DmaSound,
    /// Microwire/LMC1992 circuit (STE): master and left/right volume
    /// downstream of the PSG+DMA mix — see [`Microwire::gain`], to be
    /// applied by the caller to the final output sample (not a
    /// `dma_sound` register, wired separately in `read8`/`write8` at the
    /// same addresses `$FF8922`/`$FF8924`).
    pub microwire: Microwire,
    /// MFP 68901 chip, wired onto IPL6 (see `Bus::irq_level`). Public
    /// field: the caller needs to inject external events into it
    /// (`set_gpip_input`, `push_rx_byte`…). Advancing its timers goes
    /// through [`Self::tick`], not directly through `Mfp::tick`.
    pub mfp: Mfp,
    /// GLUE chip (HBL/VBL timing), wired onto IPL2/IPL4. Public field:
    /// useful to read in order to synchronize external video rendering
    /// on `current_line()`/`frame_count()`.
    pub glue: Glue,
    /// Keyboard ACIA. Public field: inject bytes received from the
    /// keyboard controller via `push_rx_byte`, read commands sent by the
    /// program via `take_tx_byte`.
    pub acia_keyboard: Acia,
    /// IKBD controller (HD6301): translates keyboard/mouse events coming
    /// from the host into IKBD protocol bytes and handles commands sent
    /// by the program (reset, mouse mode…). Wired to `acia_keyboard` by
    /// [`Self::tick`] — see [`ikbd::Ikbd`]. Public field: inject host
    /// events via `key_make`/`key_break`/`mouse_move`.
    pub ikbd: Ikbd,
    /// MIDI ACIA (in/out).
    pub acia_midi: Acia,
    /// YM2149 PSG (sound + I/O ports). Public field: read audio output
    /// levels via `channel_level`, inject port A/B inputs
    /// (joystick/mouse/drive, wiring not interpreted by this board).
    pub ym2149: Ym2149,
    /// Shifter (video). Public field mainly for direct register reads if
    /// needed; in practice the image is read via [`Self::framebuffer`],
    /// already rendered.
    pub shifter: Shifter,
    /// Image of the frame currently being built: one line per entry
    /// (indexed like `Glue::current_line`), updated at the HBL rate by
    /// [`Self::tick`]. Contains the previous frame's image until the
    /// corresponding line of the current frame has been rendered.
    pub framebuffer: Vec<Vec<(u8, u8, u8)>>,
    /// Monotonic counter (never reset, unlike `Glue::current_line` which
    /// wraps around): needed to detect that a complete frame has just
    /// elapsed (313 lines in PAL) without confusing it with "no line
    /// elapsed" when `current_line` wraps back to 0.
    last_absolute_line: u64,
    /// Last observed `Glue::vbl_edge_count()` — detects the VBL edge
    /// (visible-line -> blanking transition, NOT the frame wraparound) to
    /// trigger `Shifter::start_frame`. See the doc of
    /// `Glue::vbl_edge_count` for why VBL specifically, and not
    /// `frame_count`.
    last_vbl_edge: u64,
    /// PC of the CPU instruction currently executing, updated by the
    /// caller just before `Cpu::step` — only for diagnostics
    /// (`RUST68_TRACE_BLITTER`, identify the ROM routine arming a blit).
    pub last_pc: u32,
    /// WD1772 (floppy disk controller). Public field: wiring `/INTRQ` by
    /// hand isn't necessary, `Self::tick` handles it (relayed onto the
    /// MFP's `GPIP5`).
    pub wd1772: Wd1772,
    /// Disk inserted in drive A, if any. Public field: insert/eject
    /// directly (`st.floppy_a = Some(Box::new(RawDiskImage::new(...)))`).
    /// A trait object (`dyn FloppyDisk`) rather than a concrete format:
    /// accepts `RawDiskImage` (`.st`) as well as
    /// `peripherals::atari_st::stx::StxImage` (`.stx`) without this board
    /// needing to know the file format.
    pub floppy_a: Option<Box<dyn FloppyDisk>>,
    dma_register_select: u8,
    dma_address: u32,
    /// Bit 4 of `DMA_MODE`: toggles `FDC_DATA` between access to the
    /// WD1772's registers (0) and writing/reading the DMA sector count
    /// (1) — a separate mechanism, NOT a register of the floppy disk
    /// controller itself (confirmed by Hatari, `fdc.c`: "Set DMA sector
    /// count if ff8606 bit 4 == 1"). See `dma_sector_count`.
    dma_sector_count_mode: bool,
    /// Number of sectors remaining to transfer, programmed via
    /// `FDC_DATA` in sector-count mode (see `dma_sector_count_mode`) —
    /// `None` as long as no count has ever been programmed
    /// (simplification: on real silicon it is 0 after reset, which would
    /// block any transfer until software arms it; in practice TOS always
    /// does so before the slightest Type II command, so treating "never
    /// programmed" as "unlimited" has no practical consequence and avoids
    /// making a transfer deliberately triggered "by hand", without this
    /// preamble, by a test/tool fail). REALLY limits the number of
    /// sectors transferred on the DMA side, independently of what the
    /// WD1772 itself would naturally do (which would keep finding the
    /// following sectors on the track) — without this limit, a
    /// multi-sector read (bit M) overflows into RAM far beyond what
    /// software expects as soon as the physical track has more sectors
    /// than what was requested (the case of non-standard protected
    /// tracks).
    dma_sector_count: Option<u16>,
    /// Blitter (STE). Public field mainly for direct register reads;
    /// triggering happens by writing the BUSY/START bit of the control
    /// register (see `Bus::write8` on `BLITTER_BASE +
    /// blitter::reg::CONTROL`).
    pub blitter: Blitter,
    /// ROM overlay at address 0 (real ST/STE hardware wiring): as long as
    /// true, READS in `0x000000..OVERLAY_SIZE` return the ROM's content
    /// (not the underlying RAM), while WRITES still go to RAM normally —
    /// exactly the real behavior (the ROM is read-only anyway). Active by
    /// default at creation (and after a `/RESET`), disabled by the first
    /// write to [`MEMORY_CONF`] (TOS does this very early at boot).
    /// Without this overlay: (1) the reset vector (SSP/PC read at
    /// `0x000000`/`0x000004`) would not be satisfied by fresh RAM
    /// (zeros), and (2) TOS's standard RAM detection technique — zeroing
    /// the bus error vector at `0x000008` then probing beyond installed
    /// RAM, which bounces execution back to address 0 on each failure —
    /// would not land on valid ROM code (the TOS header's `bra.s`) but on
    /// zeroed RAM, degenerating into arbitrary code execution. The
    /// overlay deliberately covers only a small window ([`OVERLAY_SIZE`],
    /// not the whole ROM): beyond it, low addresses such as the system
    /// variables `memvalid`/`phystop` (`$420`, `$42E`…) must remain real
    /// RAM, without which TOS's check of them would make no sense (a
    /// system variable meant to persist across a warm restart cannot be
    /// read-only in ROM).
    overlay: bool,
    /// Copy of the last value written to [`MEMORY_CONF`] (MMU register).
    /// Now actually drives intra-bank address mirroring on STE (see
    /// [`Self::translate_ram_addr`]) — not just a passive software
    /// readback.
    memory_conf: u8,
    /// Actually populated size of each RAM bank (see
    /// [`Self::ram_bank_sizes`]) — `None` if `self.ram.len()` doesn't
    /// match any standard STE bank configuration (see its doc), in which
    /// case [`Self::translate_ram_addr`] doesn't translate anything
    /// (direct mapping unchanged, as before the introduction of MMU
    /// mirroring).
    ram_bank_sizes: Option<(u32, u32)>,
    /// Forces the software value of [`MEMORY_CONF`] to stay pinned to
    /// this value, no matter what software writes to it — see
    /// [`Self::pin_memory_conf`].
    memory_conf_pin: Option<u8>,
    /// True if the Blitter is physically present on this machine.
    /// Standard on STE/Mega STE; absent on 520ST/1040ST (the Mega ST just
    /// had a chip socket, not always populated — see
    /// [`crate::systems::atari_st::model`]). When false, the
    /// `BLITTER_BASE` area falls through to the generic I/O stub (`0xFF`
    /// on read, writes ignored) instead of responding — a real program
    /// probing for Blitter presence before using it must see the same
    /// thing as on an ST without a Blitter.
    blitter_present: bool,
    bus_fault: Option<(u32, bool)>,
}

/// Size of the ROM overlay window at address 0 (see [`AtariSt::overlay`]).
/// Broadly covers the TOS header and the very start of the bootstrap code
/// (`os_entry`/`os_version`/`os_reseth`/`os_beg`/… then the first real
/// instructions), without encroaching on the low system variables
/// (`memvalid` etc. start at `$420`).
const OVERLAY_SIZE: u32 = 0x200;

/// Size of the area permanently mapped onto ROM at address 0 (the SSP/PC
/// reset vectors), independently of the state of [`AtariSt::overlay`].
///
/// Documented in black and white in the Atari technical manual (Mega
/// Service Manual, RAM memory map): "Note: the first 8 bytes of ROM are
/// mapped into addresses 0-7. These are reset vectors which the 68000
/// uses on start-up." — a permanent characteristic of the memory map,
/// distinct from the larger overlay that can be disabled via
/// `MEMORY_CONF` (which covers `0x200` bytes and only serves to bootstrap
/// the very start of boot). Writing into this area must trigger a real
/// bus error (Glue: "asserts Bus Error if... writing to ROM"), not be
/// silently ignored like other ROM writes — confirmed by the STe factory
/// diagnostic cartridge (test "I7 Bus error not detected", which
/// deliberately writes to address 0 after installing its own bus error
/// handler, to verify the hardware reacts). Doesn't conflict with TOS's
/// standard RAM detection technique (which targets address 8, outside
/// this area), nor with any low system variable (`memvalid` etc. start
/// at `$420`).
const RESET_VECTOR_ROM_SIZE: u32 = 8;

/// DMA channel connecting the WD1772 to the board's RAM at the current
/// DMA address (see `peripherals::atari_st::wd1772::DmaChannel`): the
/// WD1772 doesn't know about RAM, only this channel.
///
/// Also enforces `dma_sector_count` (see its doc): beyond the programmed
/// sector count, bytes are silently dropped (read: RAM unchanged; write:
/// `0` returned to the WD1772) rather than transferred — behavior of the
/// real DMA controller, independent of what the WD1772 would keep doing
/// on its own.
struct RamDmaChannel<'a> {
    ram: &'a mut [u8],
    address: &'a mut u32,
    sector_count: &'a mut Option<u16>,
    bytes_in_sector: u32,
}

impl<'a> RamDmaChannel<'a> {
    fn transfer_allowed(&self) -> bool {
        !matches!(self.sector_count, Some(0))
    }

    fn advance(&mut self) {
        self.bytes_in_sector += 1;
        if self.bytes_in_sector >= SECTOR_SIZE as u32 {
            self.bytes_in_sector = 0;
            if let Some(count) = self.sector_count.as_mut() {
                *count = count.saturating_sub(1);
            }
        }
    }
}

/// `Bus` view over a slice of RAM, to give the Blitter (which takes a
/// generic `Bus`) access to the board's RAM without a reflexive borrow of
/// the whole `AtariSt`.
///
/// Must also see the ROM: the Blitter frequently reads its source data
/// (icon masks, patterns) directly from ROM (`src_addr` in the
/// `rom_base..` range). A `RamBus` that only knew about `ram` used to
/// return `0xFF` for any ROM read (address outside `ram`, well beyond its
/// installed size) instead of the real content — silent and systematic
/// corruption of any blit reading its source from ROM (icon masks during
/// a selection, menu patterns), invisible to tests that replay the
/// Blitter via a HashMap bus with a hand-embedded ROM rather than through
/// this precise `RamBus`.
struct RamBus<'a> {
    ram: &'a mut [u8],
    rom: &'a [u8],
    rom_base: u32,
}

impl<'a> Bus for RamBus<'a> {
    fn read8(&mut self, addr: u32) -> u8 {
        if let Some(&b) = self.ram.get(addr as usize) {
            return b;
        }
        if addr >= self.rom_base && addr - self.rom_base < self.rom.len() as u32 {
            return self.rom[(addr - self.rom_base) as usize];
        }
        if AtariSt::in_floating_st_ram(addr) {
            return 0x00;
        }
        if std::env::var("RUST68_TRACE_RAMBUS_FALLBACK").is_ok() {
            eprintln!("[rambus] read outside RAM/ROM/floating: addr={addr:#08x} -> 0xFF");
        }
        0xFF
    }

    fn write8(&mut self, addr: u32, value: u8) {
        if let Some(slot) = self.ram.get_mut(addr as usize) {
            *slot = value;
        }
        // ROM and beyond: write ignored (read-only / floating), same
        // logic as `AtariSt::write8`.
    }
}

impl<'a> DmaChannel for RamDmaChannel<'a> {
    fn pull(&mut self) -> u8 {
        let byte = if self.transfer_allowed() {
            self.ram.get(*self.address as usize).copied().unwrap_or(0)
        } else {
            0
        };
        *self.address = self.address.wrapping_add(1);
        self.advance();
        byte
    }

    fn push(&mut self, byte: u8) {
        if self.transfer_allowed() {
            if let Some(slot) = self.ram.get_mut(*self.address as usize) {
                *slot = byte;
            }
        }
        *self.address = self.address.wrapping_add(1);
        self.advance();
    }
}

impl AtariSt {
    /// Read access to installed RAM — only for diagnostics (full snapshot
    /// triggered by `RUST68_RAM_DUMP_KEY`, see the SDL2 binary).
    pub fn ram(&self) -> &[u8] {
        &self.ram
    }

    /// Generates the next DMA Sound (STE) sample, at `host_rate_hz` (see
    /// [`DmaSound::next_sample`]) — borrows `self.ram` and
    /// `self.dma_sound` separately for the caller (an external binary
    /// cannot do this itself: `ram()` borrows all of `&self`, incompatible
    /// with a simultaneous `&mut self.dma_sound` borrow).
    ///
    /// Also relays each XSINT edge (end of DMA frame, see
    /// `DmaSound::take_xsint_pulses`) to `Mfp::pulse_ta`: real hardware
    /// wiring (XSINT on Timer A's event-counting input), without which
    /// software that counts frame wraparounds via this timer (including
    /// the STe factory diagnostic cartridge, Audio test) would never see
    /// the interrupt and would fall back to a much shorter fallback
    /// mechanism than the real playback duration.
    pub fn next_dma_sample(&mut self, host_rate_hz: u32) -> (i8, i8) {
        let sample = self.dma_sound.next_sample(&self.ram, host_rate_hz);
        for _ in 0..self.dma_sound.take_xsint_pulses() {
            self.mfp.pulse_ta();
        }
        sample
    }

    /// Size of the loaded ROM — to build a trace sink (see
    /// [`Self::describe_addr_static`]) without borrowing `AtariSt` itself.
    pub fn rom_len(&self) -> usize {
        self.rom.len()
    }

    /// True if the simulated model has a Blitter — same usage as
    /// [`Self::rom_len`].
    pub fn blitter_present(&self) -> bool {
        self.blitter_present
    }

    /// Creates a board with `ram_size` bytes of RAM installed at
    /// `0x000000`, `rom` (typically a TOS dump) mapped at
    /// `DEFAULT_ROM_BASE`, and the GLUE clocked at PAL 50 Hz (the most
    /// common case — see [`VideoMode`] for NTSC).
    pub fn new(ram_size: usize, rom: Vec<u8>) -> Self {
        let mut mfp = Mfp::new();
        mfp.set_gpip_input(GPIP_MONO_DETECT, true); // color monitor (see the constant)
        // Real idle state of GPIP4/GPIP5 (ACIA `/IRQ`, WD1772 `/INTRQ`,
        // active low, pulled high by default — see `Self::tick`): without
        // this initialization, the MFP's default internal state (0) would
        // mask the very first transition to "interrupt active" (it would
        // also compute 0, so no edge detected).
        mfp.set_gpip_input(4, true);
        mfp.set_gpip_input(5, true);
        AtariSt {
            ram: vec![0; ram_size],
            rom,
            rom_base: DEFAULT_ROM_BASE,
            cartridge: Vec::new(),
            trace_irq: std::env::var("RUST68_TRACE_IRQ").is_ok(),
            blitter_slice_cycle_acc: 0,
            ste_dma_sound: [0; (STE_DMA_SOUND_END - STE_DMA_SOUND_BASE + 1) as usize],
            dma_sound: DmaSound::new(),
            microwire: Microwire::new(),
            mfp,
            glue: Glue::new(VideoMode::Pal50),
            acia_keyboard: Acia::new(),
            ikbd: Ikbd::new(),
            acia_midi: Acia::new(),
            ym2149: Ym2149::new(),
            shifter: Shifter::new(),
            framebuffer: Vec::new(),
            last_absolute_line: 0,
            last_vbl_edge: 0,
            last_pc: 0,
            wd1772: Wd1772::new(),
            floppy_a: None,
            dma_register_select: 0,
            dma_address: 0,
            dma_sector_count_mode: false,
            dma_sector_count: None,
            blitter: Blitter::new(),
            overlay: true,
            memory_conf: 0,
            ram_bank_sizes: Self::ram_bank_sizes(ram_size),
            memory_conf_pin: None,
            blitter_present: true,
            bus_fault: None,
        }
    }

    /// Builds a board from a known model in the ST/STE lineup (see
    /// [`model`]): RAM and Blitter presence set according to the model,
    /// `rom` provided separately (the installed TOS version is not a
    /// property of the model — any compatible TOS can be flashed into a
    /// real machine). The ROM base (`0xFC0000` vs `0xE00000`) still needs
    /// to be set separately via [`Self::set_rom_base`] once the TOS
    /// version is known (see `os_version` in the TOS header, independent
    /// of the machine model).
    pub fn from_model(profile: model::MachineProfile, rom: Vec<u8>) -> Self {
        let mut st = Self::new(profile.ram_size, rom);
        st.blitter_present = profile.has_blitter;
        st.shifter.set_ste_palette(profile.ste_palette);
        st
    }

    /// Changes the ROM base address after construction. Useful for
    /// TOS >= 1.06, mapped at `0xE00000` on real ST/STE rather than at
    /// [`DEFAULT_ROM_BASE`] (`0xFC0000`, valid for TOS <= 1.04) — the
    /// 256 KB size of these more recent TOS versions wouldn't fit between
    /// `0xFC0000` and the start of the I/O area (`0xFF8000`) anyway.
    pub fn set_rom_base(&mut self, base: u32) {
        self.rom_base = base;
    }

    /// Changes the video mode (PAL 50 Hz by default at construction, see
    /// [`Self::new`]) — replaces the GLUE, so it must be called right
    /// after construction/before the first `reset`, not during ongoing
    /// emulation (otherwise the current line/frame counter would be
    /// lost).
    pub fn set_video_mode(&mut self, mode: VideoMode) {
        self.glue = Glue::new(mode);
    }

    /// Inserts a cartridge ROM image, mapped read-only starting at
    /// `CARTRIDGE_BASE` (`$FA0000`) — real ST/STE cartridge port, notably
    /// used by hardware diagnostic cartridges. `data` must already be in
    /// native 68000 format (big-endian words); see `atari_st_sdl2` for
    /// interleaving two separate HIGH/LOW images (paired 8-bit ROMs, a
    /// common EPROM format for these cartridges).
    pub fn load_cartridge(&mut self, data: Vec<u8>) {
        self.cartridge = data;
    }

    /// True if a cartridge has been loaded (see [`Self::load_cartridge`]).
    /// Useful to know whether TOS's "warm restart" shortcut (see
    /// [`Self::pin_memory_conf`]) is relevant: a diagnostic cartridge does
    /// its own complete hardware initialization and doesn't go through
    /// the normal TOS boot, so not through this shortcut either — pinning
    /// `MEMORY_CONF` there would break its own RAM detection (which
    /// precisely needs to write this register freely to observe address
    /// mirroring and self-correct).
    pub fn has_cartridge(&self) -> bool {
        !self.cartridge.is_empty()
    }

    /// Advances the peripherals (MFP + GLUE + YM2149) by
    /// `cpu_cycles` CPU cycles, relays the combined IRQ of the two ACIAs onto
    /// the MFP's `GPIP4` (wired OR, real ST/STE wiring), and triggers
    /// video rendering (`Shifter`) at the GLUE's HBL/VBL rate. To be called by
    /// the caller after every `Cpu::step`:
    ///
    /// ```
    /// use rust68::{Cpu, systems::atari_st::AtariSt};
    ///
    /// let mut st = AtariSt::new(0x1000, vec![]);
    /// let mut cpu = Cpu::new();
    /// cpu.reset(&mut st);
    /// let cycles = cpu.step(&mut st).unwrap();
    /// st.tick(cycles);
    /// ```
    pub fn tick(&mut self, cpu_cycles: u32) {
        // Advances a paused non-HOG blit (see `Blitter::execute`, 16-word
        // slices) independently of any CPU write to the CONTROL register.
        // On real silicon, the Blitter progresses autonomously (bus
        // cycles shared with the CPU at the hardware's rhythm), not only
        // when software rewrites CONTROL — our earlier model only resumed
        // the blit on a write to CONTROL with the BUSY bit set, which
        // worked by coincidence with TOS's `TAS.B` loop (which IS a
        // write) but blocked indefinitely any software polling BUSY via
        // a plain read (`BTST.B`, without a rewrite) — confirmed in
        // practice with the STe factory diagnostic cartridge (test G2
        // "endmask", a wide 40-word blit exceeding a single slice, never
        // resumed).
        if self.blitter_present && self.blitter.busy() {
            self.blitter_slice_cycle_acc += cpu_cycles;
            if self.blitter_slice_cycle_acc >= BLITTER_SLICE_CYCLES {
                self.blitter_slice_cycle_acc -= BLITTER_SLICE_CYCLES;
                if std::env::var("RUST68_TRACE_BLIT_REGS").is_ok() {
                    eprintln!(
                        "[blit-reg] pc={:#010x} tick triggers execute() acc_before_update={}",
                        blitter::DEBUG_LAST_PC.load(std::sync::atomic::Ordering::Relaxed),
                        self.blitter_slice_cycle_acc + BLITTER_SLICE_CYCLES,
                    );
                }
                let mut ram_bus = RamBus {
                    ram: &mut self.ram,
                    rom: &self.rom,
                    rom_base: self.rom_base,
                };
                self.blitter.execute(&mut ram_bus);
            }
        } else {
            self.blitter_slice_cycle_acc = 0;
        }
        self.mfp.tick(cpu_cycles);
        self.glue.tick(cpu_cycles);
        self.ym2149.tick(cpu_cycles);
        self.ikbd.tick(cpu_cycles);
        // Commands sent by the program (reset, mouse mode…): relay them
        // from the ACIA's transmit side to the IKBD, which interprets
        // them.
        while let Some(byte) = self.acia_keyboard.take_tx_byte() {
            self.ikbd.receive_cmd(byte);
        }
        // Only push the next byte into the ACIA if the previous one has
        // actually been consumed by the program (RDRF cleared) — read of
        // the status register, without side effects (unlike a read of the
        // data register, which acknowledges RDRF).
        //
        // On real silicon, the ACIA's `/IRQ` genuinely rises back to 1
        // (for the duration of the serial gap between two bytes) before
        // falling again for the next byte — a real edge every time. Here,
        // pushing the next byte within the same tick() in which RDRF has
        // just cleared masks this rise: GPIP4 would stay at 0
        // continuously between two bytes of the same burst (e.g. the 3
        // bytes of a mouse packet), and `Mfp::set_gpip_input` — rightly
        // edge-triggered — would then never see an edge for the following
        // bytes, leaving them stuck until an unrelated event (another MFP
        // register write) triggers an incidental edge. Exact bug already
        // isolated and fixed in the companion project Stay (see
        // `Bus::read_acia_ikbd_data`): explicitly force the release (high
        // level) before rearming RDRF for the next byte, within the same
        // tick, to guarantee a genuine rising-then-falling edge.
        if self.acia_keyboard.read(acia::reg::CONTROL_STATUS) & 0x01 == 0 {
            if let Some(byte) = self.ikbd.pop_tx() {
                self.mfp.set_gpip_input(4, true);
                self.acia_keyboard.push_rx_byte(byte);
            }
        }
        // `/IRQ` (ACIA) and `/INTRQ` (WD1772) are real hardware signals,
        // active low (asserted = logic level 0, as their name indicates),
        // wired directly onto GPIP4/GPIP5 — with no inverter, GPIP must
        // therefore read 0 when the interrupt is active, 1 at rest. A
        // real TOS sometimes probes this raw level directly (not only via
        // the MFP's edge-triggered interrupt channel): at boot, for
        // example, floppy drive count detection waits for GPIP5 to go to
        // 0 after a WD1772 command, with a timeout — without this
        // inversion, the bit never goes down to 0 and TOS wrongly
        // concludes no drive is present (`_nflops` stays at 0, no A:
        // icon on the desktop).
        // Advances an in-progress WD1772 command (see the doc of
        // `wd1772::Wd1772::tick` — real seek speed, rotational latency,
        // transfer rate, rather than the instantaneous execution of an
        // earlier version that made the whole floppy drive far too fast).
        // Drive/side wiring: see `floppy_drive_select`, re-read here too
        // (not only on command write) because a multi-sector command can
        // span several `tick()` calls.
        {
            let (drive_a_selected, side) = self.floppy_drive_select();
            self.wd1772.side = side;
            let disk = if drive_a_selected { self.floppy_a.as_deref_mut() } else { None };
            let mut channel = RamDmaChannel {
                ram: &mut self.ram,
                address: &mut self.dma_address,
                sector_count: &mut self.dma_sector_count,
                bytes_in_sector: 0,
            };
            self.wd1772.tick(cpu_cycles, disk, &mut channel);
        }

        let acia_irq = self.acia_keyboard.irq_requested() || self.acia_midi.irq_requested();
        self.mfp.set_gpip_input(4, !acia_irq);
        self.mfp.set_gpip_input(5, !self.wd1772.interrupt_requested());

        // Reloads the Shifter's video counter on the VBL edge (visible
        // line -> blanking transition, see `Glue::vbl_edge_count`), NOT
        // on the complete frame wraparound (`Glue::frame_count`): on real
        // silicon, the base is reloaded right at the start of vertical
        // blanking, which precedes line 0 of the next frame by the whole
        // rest of blanking (~113 lines in PAL) — not in the same breath.
        // Using `frame_count` here would have visible line 0 of the next
        // frame already rendered (and its Timer B pulse already emitted,
        // see below) within the SAME `tick()` call as the one where VBL
        // has just armed, leaving absolutely no window for software to
        // take the VBL interrupt before this line has already been
        // consumed — confirmed necessary by the STe factory diagnostic
        // cartridge (test "T4 Video Counter in Memory Controller").
        let vbl_edge_now = self.glue.vbl_edge_count();
        if vbl_edge_now != self.last_vbl_edge {
            self.last_vbl_edge = vbl_edge_now;
            self.shifter.start_frame();
        }
        let lines_per_frame = self.glue.lines_per_frame() as u64;
        // Absolute counter (never reset) so as not to confuse "a whole
        // frame has just elapsed" with "no line elapsed" when
        // current_line() wraps back to 0.
        let absolute_line_now =
            self.glue.frame_count() * lines_per_frame + self.glue.current_line() as u64;
        // Defensive bound: never catch up more than one whole frame in a
        // single tick (normal case: 0 or 1 line, tick() being called
        // after each instruction, far more often than one line = 512
        // cycles).
        let mut guard = 0u64;
        while self.last_absolute_line < absolute_line_now && guard < lines_per_frame {
            self.last_absolute_line += 1;
            let absolute_line = (self.last_absolute_line % lines_per_frame) as u32;
            // Real ST/STE hardware wiring: the Shifter only fetches (and
            // thus only advances its video counter) DURING displayed
            // lines (`Glue::display_index`, nominal 200-line window,
            // potentially extended by an STE top/bottom border removal —
            // see `peripherals::atari_st::shifter`) — not during vertical
            // blanking. Likewise for the MFP Timer B's external TBI
            // input, wired to the active display-enable signal (DE), not
            // to the raw HBL: it only pulses on these same displayed
            // lines. This is exactly what the TOS boot exploits to detect
            // it has just entered VBL: it programs Timer B in event-count
            // mode then waits for the value to stop changing (~615 stable
            // reads), which never happens as long as we stay within the
            // displayed area.
            if let Some(idx) = self.glue.display_index(absolute_line).map(|i| i as usize) {
                let row = self.shifter.render_scanline(&self.ram);
                if idx >= self.framebuffer.len() {
                    self.framebuffer.resize(idx + 1, Vec::new());
                }
                self.framebuffer[idx] = row;
                self.mfp.pulse_tb();
            }
            guard += 1;
        }
        if guard >= lines_per_frame && self.last_absolute_line < absolute_line_now && std::env::var("RUST68_TRACE_VECTORS").is_ok() {
            eprintln!(
                "[trace] tick(): video catch-up truncated by the guard (remaining lag: {} lines)",
                absolute_line_now - self.last_absolute_line
            );
        }
    }

    fn mfp_offset(addr: u32) -> Option<u8> {
        if addr >= MFP_BASE && addr <= MFP_END && (addr - MFP_BASE) % 2 == 0 {
            Some(((addr - MFP_BASE) / 2) as u8)
        } else {
            None
        }
    }

    fn in_rom(&self, addr: u32) -> bool {
        addr >= self.rom_base && addr - self.rom_base < self.rom.len() as u32
    }

    /// True if `addr` (already known to be outside installed RAM, i.e.
    /// `addr >= self.ram.len()`) falls within the fixed 4 MB "ST RAM"
    /// address space (see [`ST_RAM_ADDRESS_SPACE`]) — where an access
    /// **never** triggers a bus error on real silicon, even without
    /// physical RAM at that precise address, unlike the real "hole"
    /// beyond it, before `IO_BASE`.
    ///
    /// Confirmed by Hatari's source code (`stMemory.c`): this area is
    /// mapped onto `VoidMem_bank`, whose reads return a fixed value
    /// (`nonexistingdata()` = 0) and whose writes are silently ignored
    /// (`dummy_get`/`dummy_put`), never raising a bus error — the real
    /// "hole" (`BusErrMem_bank` in Hatari) only starts at 4 MB.
    ///
    /// History: two earlier attempts at MMU per-bank address mirroring
    /// (in order to also satisfy a factory diagnostic cartridge whose
    /// quick RAM-size heuristic concludes "2 MB" instead of 1 MB for a
    /// 1040STE) were tried and abandoned — one made TOS conclude 4 MB
    /// instead of 1 MB, the other (a real bus error here, retried then
    /// reverted within this same session) caused a double bus fault (SP
    /// still derived from the ROM header — "os_entry", not a real SSP —
    /// at the moment of the very first out-of-RAM access, before TOS had
    /// had the slightest chance to install its own).
    ///
    /// Directly verified against Hatari (with a screenshot as evidence)
    /// that mirroring IS indeed necessary — Hatari correctly displays
    /// "1M RAM" for this same TOS/cartridge/1040STE, not "2M". The real
    /// mechanism (see [`Self::translate_ram_addr`]) is narrower than the
    /// earlier attempts: a purely INTRA-bank mirroring, driven by
    /// [`MEMORY_CONF`], which becomes the identity as soon as this
    /// register reflects the actually installed RAM (the normal case,
    /// once TOS has booted) — so without the risk of the earlier
    /// attempts (neither a bus error, nor an irrelevant global
    /// mirroring).
    fn in_floating_st_ram(addr: u32) -> bool {
        addr < ST_RAM_ADDRESS_SPACE
    }

    /// Actually populated size of each STE RAM bank for a total RAM of
    /// `ram_len` bytes — exact reproduction of the
    /// `STMemory_RAM_SetBankSize` table (Hatari, `stMemory.c`), the only
    /// standard configurations on real silicon (bank pairs of
    /// 128/512/2048 KB). `None` if `ram_len` doesn't match any of them
    /// (in which case [`Self::translate_ram_addr`] doesn't translate
    /// anything).
    fn ram_bank_sizes(ram_len: usize) -> Option<(u32, u32)> {
        const KB: usize = 1024;
        Some(match ram_len / KB {
            128 => (128 * 1024, 0),
            256 => (128 * 1024, 128 * 1024),
            512 => (512 * 1024, 0),
            640 => (512 * 1024, 128 * 1024),
            1024 => (512 * 1024, 512 * 1024),
            2048 => (2048 * 1024, 0),
            2176 => (2048 * 1024, 128 * 1024),
            2560 => (2048 * 1024, 512 * 1024),
            4096 => (2048 * 1024, 2048 * 1024),
            _ => return None,
        })
    }

    /// Value of [`MEMORY_CONF`] corresponding to a total RAM of
    /// `ram_len` bytes CORRECTLY configured (bits 3-2 = bank 0, bits
    /// 1-0 = bank 1) — same table as [`Self::ram_bank_sizes`], expressed
    /// as a MEMCONF code rather than as a bank size. `None` if `ram_len`
    /// doesn't match any standard configuration.
    ///
    /// To be used to pre-fill `MEMORY_CONF` before a warm start (see
    /// `atari_st_sdl2`): the "warm restart" shortcut precisely skips the
    /// TOS code that would normally configure this register (same
    /// reasoning as for `memvalid`/`phystop`) — without this pre-fill,
    /// [`Self::translate_ram_addr`] would see `MEMORY_CONF` stuck at its
    /// reset value (`0`, i.e. 128 KB + 128 KB) and would make all RAM
    /// beyond 256 KB inaccessible (floating).
    pub fn expected_memory_conf(ram_len: usize) -> Option<u8> {
        const KB: usize = 1024;
        Some(match ram_len / KB {
            128 => (0 << 2) | 0,
            256 => (0 << 2) | 0,
            512 => (1 << 2) | 0,
            640 => (1 << 2) | 0,
            1024 => (1 << 2) | 1,
            2048 => (2 << 2) | 0,
            2176 => (2 << 2) | 0,
            2560 => (2 << 2) | 1,
            4096 => (2 << 2) | 2,
            _ => return None,
        })
    }

    /// Pins the software value of [`MEMORY_CONF`] to `value` — any
    /// subsequent CPU write to this register (`write8`) is accepted (the
    /// overlay disables normally) but no longer affects the stored value,
    /// which stays `value`. `None` (default): normal behavior, the CPU
    /// fully controls this register.
    ///
    /// To be used with the "warm restart" shortcut (see `atari_st_sdl2`,
    /// alongside its equivalent pre-fill of `memvalid`/`phystop`): unlike
    /// those (simply READ by TOS to decide warm/cold), TOS
    /// UNCONDITIONALLY writes `MEMORY_CONF=0` very early at boot (even
    /// before consulting `memvalid`), which is normally only corrected at
    /// the very end of the RAM detection algorithm — the exact algorithm
    /// the shortcut skips. A one-off pre-fill therefore gets immediately
    /// overwritten; pinning it here makes it survive this intermediate
    /// write, exactly as if detection had actually taken place and had
    /// concluded the correct value.
    pub fn pin_memory_conf(&mut self, value: u8) {
        self.memory_conf_pin = Some(value);
    }

    /// Decodes a 2-bit field of [`MEMORY_CONF`] into a logical bank size
    /// (`00`=128 KB, `01`=512 KB, `10`=2 MB, `11`=reserved/invalid,
    /// treated as absent) — reproduction of `STMemory_MMU_Size` (Hatari).
    fn mmu_bank_size_from_code(code: u8) -> u32 {
        match code & 0x3 {
            0 => 128 * 1024,
            1 => 512 * 1024,
            2 => 2048 * 1024,
            _ => 0,
        }
    }

    /// Translates a logical CPU address into a physical offset within
    /// `self.ram` — `None` if the address falls outside installed RAM (it
    /// must then fall through to [`Self::in_floating_st_ram`]).
    ///
    /// On STE/Mega STE with a standard bank configuration (see
    /// [`Self::ram_bank_sizes`]), reproduces the MMU/MCU's intra-bank
    /// address mirroring (`STMemory_MMU_Translate_Addr_STE`, Hatari):
    /// [`MEMORY_CONF`] (bits 3-2 = bank 0's logical size, bits 1-0 =
    /// bank 1) assigns each bank a size that software believes is real;
    /// if it exceeds the ACTUALLY populated size, addresses beyond the
    /// real size but within the logical size "wrap around" (incomplete
    /// DRAM addressing: certain column/row lines simply aren't wired for
    /// a chip smaller than the slot it's meant for). Demonstrated in
    /// Hatari: the formula systematically reduces to
    /// `logical_addr & (real_size - 1)`, independently of the precise
    /// logical size (only its order of magnitude, via the bank dispatch
    /// below, matters) — hence the simplified implementation. Becomes the
    /// identity as soon as `MEMORY_CONF` reflects the actually installed
    /// RAM (the normal case, once TOS has booted): no change in behavior
    /// outside the startup window where the configuration is still
    /// incorrect/default.
    ///
    /// On ST/Mega ST (`!self.blitter_present`) or for a non-standard RAM
    /// size (`ram_bank_sizes` = `None`): direct mapping, as before the
    /// introduction of this mirroring — the STF (non-STE) formula differs
    /// (different reordering of column/row bits) and isn't reproduced
    /// here for lack of a demonstrated need.
    fn translate_ram_addr(&self, addr: u32) -> Option<usize> {
        let Some((ram_b0, ram_b1)) = self.ram_bank_sizes.filter(|_| self.blitter_present) else {
            return if (addr as usize) < self.ram.len() { Some(addr as usize) } else { None };
        };
        let mmu_b0 = Self::mmu_bank_size_from_code(self.memory_conf >> 2);
        let mmu_b1 = Self::mmu_bank_size_from_code(self.memory_conf);
        if addr < mmu_b0 {
            if ram_b0 == 0 {
                return None;
            }
            Some((addr & (ram_b0 - 1)) as usize)
        } else if addr < mmu_b0.saturating_add(mmu_b1) {
            if ram_b1 == 0 {
                return None;
            }
            let off = (addr - mmu_b0) & (ram_b1 - 1);
            Some((ram_b0 + off) as usize)
        } else {
            None
        }
    }

    fn is_shifter_addr(addr: u32) -> bool {
        matches!(
            addr,
            shifter::addr::VIDEO_BASE_HIGH
                | shifter::addr::VIDEO_BASE_MID
                | shifter::addr::VIDEO_BASE_LOW
                | shifter::addr::VIDEO_COUNTER_HIGH
                | shifter::addr::VIDEO_COUNTER_MID
                | shifter::addr::VIDEO_COUNTER_LOW
                | shifter::addr::RESOLUTION
                | shifter::addr::HSCROLL_NO_PREFETCH
                | shifter::addr::HSCROLL_PREFETCH
                | shifter::addr::LINE_WIDTH
        ) || (shifter::addr::PALETTE_BASE..shifter::addr::PALETTE_BASE + 32).contains(&addr)
    }

    fn is_blitter_addr(&self, addr: u32) -> bool {
        self.blitter_present && (BLITTER_BASE..BLITTER_BASE + blitter::reg::END).contains(&addr)
    }

    /// Decodes the drive/side selection lines of the floppy connector,
    /// carried by the YM2149's port A (see the doc of
    /// [`ym2149::Ym2149::port_a_output`]) — returns `(drive A selected,
    /// side)`. Without this wiring, `self.wd1772.side` would always stay
    /// at its default value (0) no matter what TOS programs, making
    /// unreadable any content located on side 1 of a double-sided floppy
    /// (the case of practically all real ST software in the 720 KB `.st`
    /// format).
    fn floppy_drive_select(&self) -> (bool, u8) {
        let port_a = self.ym2149.port_a_output();
        let drive_a_selected = port_a & 0x02 == 0;
        let side = !port_a & 0x01;
        (drive_a_selected, side)
    }

    /// True if `off` (offset relative to [`STE_DMA_SOUND_BASE`])
    /// corresponds to a register handled by [`dma_sound::DmaSound`] (see
    /// its [`dma_sound::reg`] module) rather than the generic storage
    /// (`self.ste_dma_sound`) — the Microwire (`$FF8922`/`$FF8923`,
    /// offsets `0x22`/`0x23`) is deliberately kept out of this list,
    /// handled separately (see the doc on `STE_MICROWIRE_DATA`).
    fn is_dma_sound_reg(off: u32) -> bool {
        use dma_sound::reg;
        matches!(
            off,
            reg::CONTROL_LOW
                | reg::FRAME_START_HIGH
                | reg::FRAME_START_MID
                | reg::FRAME_START_LOW
                | reg::FRAME_COUNT_HIGH
                | reg::FRAME_COUNT_MID
                | reg::FRAME_COUNT_LOW
                | reg::FRAME_END_HIGH
                | reg::FRAME_END_MID
                | reg::FRAME_END_LOW
                | reg::SOUND_MODE
        )
    }

    /// Component label for `addr`, for [`crate::trace::FileTraceSink`]
    /// (see `RUST68_TRACE_ALL`) — faithfully reproduces the decision
    /// order of [`Bus::read8`] below (same priority between RAM/ROM and
    /// peripherals), so that the label always matches what the access
    /// *actually* touched, not an independent, approximate
    /// classification. Doesn't observe `self.overlay` (true state only
    /// during the very first instructions of cold boot): see
    /// [`Self::describe_addr_static`], of which this is only a shortcut —
    /// the "ram" label is returned instead during this short window,
    /// without consequence since it is purely descriptive (the real
    /// dispatch in `read8`/`write8` remains unchanged).
    pub fn describe_addr(&self, addr: u32) -> &'static str {
        Self::describe_addr_static(self.ram.len(), self.rom_base, self.rom.len(), self.blitter_present, addr)
    }

    /// `&self`-less version of [`Self::describe_addr`] — for callers
    /// (such as the `RUST68_TRACE_ALL` trace sink, see
    /// `bin/atari_st_sdl2.rs`) that cannot borrow `AtariSt` while
    /// simultaneously wrapping it in a mutable [`crate::TracingBus`]. The
    /// parameters capture everything the classification needs, fixed at
    /// construction (never modified afterwards).
    pub fn describe_addr_static(
        ram_len: usize,
        rom_base: u32,
        rom_len: usize,
        blitter_present: bool,
        addr: u32,
    ) -> &'static str {
        let addr = addr & ADDR_MASK;
        if (addr as usize) < ram_len {
            return "ram";
        }
        if Self::in_floating_st_ram(addr) {
            return "floating";
        }
        if Self::mfp_offset(addr).is_some() {
            return "mfp";
        }
        match addr {
            ACIA_KEYBOARD_CONTROL | ACIA_KEYBOARD_DATA => return "acia-keyboard",
            ACIA_MIDI_CONTROL | ACIA_MIDI_DATA => return "acia-midi",
            YM2149_SELECT | YM2149_DATA => return "ym2149",
            GLUE_SYNC => return "glue",
            _ if Self::is_shifter_addr(addr) => return "shifter",
            FDC_DATA | DMA_MODE | DMA_ADDR_HIGH | DMA_ADDR_MID | DMA_ADDR_LOW => return "wd1772-dma",
            _ if blitter_present && (BLITTER_BASE..BLITTER_BASE + blitter::reg::END).contains(&addr) => {
                return "blitter";
            }
            _ if (STE_DMA_SOUND_BASE..=STE_DMA_SOUND_END).contains(&addr) => return "ste-dma-sound",
            _ => {}
        }
        if addr >= rom_base && addr - rom_base < rom_len as u32 {
            return "rom";
        }
        if (IO_BASE..=IO_END).contains(&addr) {
            return "io-non-implemente";
        }
        if (CARTRIDGE_BASE..=CARTRIDGE_END).contains(&addr) {
            return "cartouche";
        }
        "fault"
    }
}

impl Bus for AtariSt {
    fn read8(&mut self, addr: u32) -> u8 {
        let addr = addr & ADDR_MASK;
        if self.overlay && addr < OVERLAY_SIZE && (addr as usize) < self.rom.len() {
            return self.rom[addr as usize];
        }
        if let Some(phys) = self.translate_ram_addr(addr) {
            return self.ram[phys];
        }
        if Self::in_floating_st_ram(addr) {
            // Beyond installed RAM but within the "ST RAM" space (4 MB):
            // never a bus error on real silicon (see the module doc), a
            // fixed, non-stored value (never what was just written) —
            // confirmed by Hatari's source code (`stMemory.c`,
            // `VoidMem_bank`/`dummy_get`): this area returns a fixed
            // value without ever faulting, unlike the real "hole" beyond
            // 4 MB (`BusErrMem_bank` in Hatari, before `IO_BASE` for us).
            // Do NOT confuse with an MMU bank aliasing quirk (which does
            // genuinely exist in Hatari but only applies inside a
            // physically populated bank — not modeled here, see the doc
            // of `in_floating_st_ram`).
            return 0x00;
        }
        if let Some(off) = Self::mfp_offset(addr) {
            return self.mfp.read(off);
        }
        match addr {
            ACIA_KEYBOARD_CONTROL => return self.acia_keyboard.read(acia::reg::CONTROL_STATUS),
            ACIA_KEYBOARD_DATA => {
                let v = self.acia_keyboard.read(acia::reg::DATA);
                if std::env::var("RUST68_TRACE_IKBD").is_ok() {
                    eprintln!("[ikbd] read ACIA_KEYBOARD_DATA -> {v:#04x}");
                }
                return v;
            }
            ACIA_MIDI_CONTROL => return self.acia_midi.read(acia::reg::CONTROL_STATUS),
            ACIA_MIDI_DATA => return self.acia_midi.read(acia::reg::DATA),
            YM2149_SELECT => return self.ym2149.read(ym2149::bus_offset::SELECT),
            YM2149_DATA => return self.ym2149.read(ym2149::bus_offset::DATA),
            GLUE_SYNC => return self.glue.read_sync(),
            _ if Self::is_shifter_addr(addr) => return self.shifter.read(addr),
            FDC_DATA => return self.wd1772.read(self.dma_register_select),
            DMA_MODE => return self.dma_register_select,
            DMA_ADDR_HIGH => return (self.dma_address >> 16) as u8,
            DMA_ADDR_MID => return (self.dma_address >> 8) as u8,
            DMA_ADDR_LOW => return self.dma_address as u8,
            _ if self.is_blitter_addr(addr) => return self.blitter.read(addr - BLITTER_BASE),
            // Microwire DATA (`$FF8922`/`$FF8923`): always 0 on read, no
            // matter what was written to it — simulates a serial shift
            // that is always already finished (real silicon: this
            // register progressively empties during the shift; without
            // emulating the real serial timing, software that writes then
            // loops waiting for it to reach zero must find zero
            // immediately, not loop indefinitely). The MASK register
            // (`$FF8924`) and the rest of the range remain normal,
            // faithful read/write storage.
            STE_MICROWIRE_DATA | STE_MICROWIRE_DATA1 => return 0x00,
            _ if (STE_DMA_SOUND_BASE..=STE_DMA_SOUND_END).contains(&addr) => {
                let off = addr - STE_DMA_SOUND_BASE;
                if Self::is_dma_sound_reg(off) {
                    return self.dma_sound.read(off);
                }
                return self.ste_dma_sound[off as usize];
            }
            _ => {}
        }
        if self.in_rom(addr) {
            return self.rom[(addr - self.rom_base) as usize];
        }
        if (CARTRIDGE_BASE..=CARTRIDGE_END).contains(&addr) {
            let off = (addr - CARTRIDGE_BASE) as usize;
            if off < self.cartridge.len() {
                return self.cartridge[off];
            }
            return 0xFF;
        }
        if (IO_BASE..=IO_END).contains(&addr) {
            return 0xFF;
        }
        if std::env::var("RUST68_TRACE_VECTORS").is_ok() {
            eprintln!("[trace] read bus fault: addr={addr:#x}");
        }
        self.bus_fault = Some((addr, false));
        0xFF
    }

    // Shifter palette registers (`$FF8240`-`$FF825E`): on real silicon
    // (confirmed by Hatari, `Video_ColorReg_WriteWord`), a `.W` or `.L`
    // CPU access writes the word normally, but an ISOLATED `.B` access
    // duplicates the written byte into both halves of the word before
    // masking (see the doc of [`shifter::Shifter::write`]). The default
    // `write8` only ever sees one byte at a time and therefore cannot
    // distinguish these two cases — this `write16` override intercepts
    // REAL word accesses for this precise range and routes them to
    // `write_palette_word`, which doesn't apply the duplication.
    fn write16(&mut self, addr: u32, value: u16) {
        let masked = addr & ADDR_MASK;
        if (shifter::addr::PALETTE_BASE..shifter::addr::PALETTE_BASE + 32).contains(&masked) {
            self.shifter.write_palette_word(masked, value);
            return;
        }
        // 16-bit Blitter registers (SRC_X_INC/SRC_Y_INC/ENDMASK1-3/
        // DST_X_INC/DST_Y_INC/X_COUNT/Y_COUNT): on real silicon, an
        // ISOLATED `.B` access to one of these registers is ignored
        // (confirmed by Hatari, `Blitter_CheckAccess_Byte`) — only a
        // complete `.W`/`.L` access is honored. The default `write8`
        // only ever sees one byte at a time and therefore cannot make
        // this distinction: this override intercepts REAL word accesses
        // for these precise registers and routes them to
        // `Blitter::write_word` (see its doc) rather than to the
        // byte-by-byte composition.
        if self.blitter_present && (BLITTER_BASE..BLITTER_BASE + blitter::reg::END).contains(&masked) {
            let reg_offset = masked - BLITTER_BASE;
            if reg_offset < 0x20
                || matches!(
                    reg_offset,
                    blitter::reg::SRC_X_INC
                        | blitter::reg::SRC_Y_INC
                        | blitter::reg::ENDMASK_1
                        | blitter::reg::ENDMASK_2
                        | blitter::reg::ENDMASK_3
                        | blitter::reg::DST_X_INC
                        | blitter::reg::DST_Y_INC
                        | blitter::reg::X_COUNT
                        | blitter::reg::Y_COUNT
                )
            {
                self.blitter.write_word(reg_offset, value);
                return;
            }
            // CONTROL+SKEW ($FF8A3C/$FF8A3D): TOS regularly writes them in
            // a single `.W` access (e.g. TOS 1.62, `$E11746`, `MOVE.W
            // D7,(A5)`, D7 packing both bytes to arm a blit with the
            // correct SKEW right from the start). The generic
            // byte-by-byte composition below would write CONTROL (HIGH
            // byte) BEFORE SKEW (LOW byte) — but `write8` triggers
            // `execute()` *synchronously and immediately* as soon as
            // CONTROL is written with the BUSY bit set (see below): the
            // Blitter would then start the blit with the OLD SKEW, an
            // instant before the new one is set by this same `.W` access
            // — confirmed by direct comparison with a real Hatari (trace
            // `RUST68_HATARI_TRACE`) on a GEM menu restoration blit: all
            // 4 planes must share the same SKEW (armed once by TOS before
            // the loop), but only the first one came out with SKEW=0 on
            // the Rust68 side — exactly the plane whose blit starts
            // during this same `.W` access. Writing SKEW first (no side
            // effects) then delegating to `write8` for CONTROL (normal,
            // unchanged path) fixes the ordering without duplicating the
            // trigger logic.
            if reg_offset == blitter::reg::CONTROL {
                self.blitter.write(blitter::reg::SKEW, value as u8);
                self.write8(addr, (value >> 8) as u8);
                return;
            }
        }
        // `DMA_MODE` ($FF8606): a real 16-bit register (see its doc) —
        // the FDC register selector lives in the LOW byte of the word.
        // TOS accesses it almost exclusively as a full word; a naive
        // byte-by-byte composition (like the generic path below) would
        // only see the HIGH byte (always zero for these small values),
        // leaving the selector permanently stuck at 0 — hence the
        // completely broken FDC selection that this interception fixes.
        if masked == DMA_MODE {
            // Delegates to `write8` (no duplicated logic here): avoids
            // the two paths diverging, as had already caused bit 4 (DMA
            // sector-count mode) to be forgotten the first time this case
            // was handled separately here.
            self.write8(DMA_MODE, value as u8);
            return;
        }
        // `FDC_DATA` ($FF8604): also a real 16-bit register (same remark
        // as `DMA_MODE` above) — the byte of the actually selected
        // WD1772 register (command/status/track/sector/data) lives in
        // the LOW byte of the word, confirmed by Hatari (`fdc.c`,
        // `FDC_DiskController_WriteWord`: `IoMem_ReadByte(0xff8605)`).
        // TOS accesses it almost exclusively as a full word; without this
        // interception, the generic byte-by-byte composition would only
        // see the HIGH byte (always zero), and EVERY command/track/
        // sector/data value written this way would be lost — making any
        // floppy read impossible.
        if masked == FDC_DATA {
            self.write8(FDC_DATA, value as u8);
            return;
        }
        self.write8(addr, (value >> 8) as u8);
        self.write8(addr.wrapping_add(1), value as u8);
    }

    /// See the doc of [`Self::write16`] on `FDC_DATA` — same symmetric
    /// interception on read (the real WD1772 byte lives in the LOW byte
    /// of the word, the default generic composition would put it in the
    /// HIGH byte).
    fn read16(&mut self, addr: u32) -> u16 {
        let masked = addr & ADDR_MASK;
        if masked == FDC_DATA {
            return self.read8(FDC_DATA) as u16;
        }
        let hi = self.read8(addr) as u16;
        let lo = self.read8(addr.wrapping_add(1)) as u16;
        (hi << 8) | lo
    }

    fn write32(&mut self, addr: u32, value: u32) {
        let masked = addr & ADDR_MASK;
        // Blitter's SRC_ADDR/DST_ADDR: 32-bit registers (24 significant
        // bits), same principle as `write16` above — only a complete
        // `.L` access is honored on real silicon.
        if self.blitter_present && (BLITTER_BASE..BLITTER_BASE + blitter::reg::END).contains(&masked) {
            let reg_offset = masked - BLITTER_BASE;
            if reg_offset == blitter::reg::SRC_ADDR || reg_offset == blitter::reg::DST_ADDR {
                self.blitter.write_long(reg_offset, value);
                return;
            }
        }
        self.write16(addr, (value >> 16) as u16);
        self.write16(addr.wrapping_add(2), value as u16);
    }

    fn write8(&mut self, addr: u32, value: u8) {
        let addr = addr & ADDR_MASK;
        if addr < 16 && std::env::var("RUST68_TRACE_VECTORS").is_ok() {
            eprintln!("[trace] low vector write: addr={addr:#x} value={value:#04x} overlay={}", self.overlay);
        }
        // Guarded by `!self.rom.is_empty()`: this permanent protection
        // assumes a real ROM containing the reset vectors that this
        // mirroring is based on (see the constant's doc). Several
        // integration tests build an `AtariSt::new(_, vec![])` (empty
        // ROM) as a bare CPU/bus test rig, and write directly to low RAM
        // — whether to set their own reset vector
        // (`cpu_prend_une_interruption_mfp_bout_en_bout`) or as plain
        // video content at address 0, the default video base
        // (`tick_rend_une_ligne_video_dans_le_framebuffer`): without a
        // real ROM, this hardware mirroring makes no sense and must not
        // apply.
        if !self.rom.is_empty() && addr < RESET_VECTOR_ROM_SIZE {
            self.bus_fault = Some((addr, true));
            return;
        }
        if let Some(phys) = self.translate_ram_addr(addr) {
            self.ram[phys] = value;
            return;
        }
        if Self::in_floating_st_ram(addr) {
            // Beyond installed RAM but within the "ST RAM" space (4 MB):
            // "floating" write, never persisted — see the equivalent doc
            // in `read8`.
            return;
        }
        if let Some(off) = Self::mfp_offset(addr) {
            self.mfp.write(off, value);
            return;
        }
        match addr {
            MEMORY_CONF => {
                self.memory_conf = self.memory_conf_pin.unwrap_or(value);
                if std::env::var("RUST68_TRACE_VECTORS").is_ok() {
                    eprintln!(
                        "[trace] MEMORY_CONF written: overlay disabled (value={value:#04x}, stored={:#04x})",
                        self.memory_conf
                    );
                }
                self.overlay = false;
                return;
            }
            ACIA_KEYBOARD_CONTROL => {
                self.acia_keyboard.write(acia::reg::CONTROL_STATUS, value);
                return;
            }
            ACIA_KEYBOARD_DATA => {
                self.acia_keyboard.write(acia::reg::DATA, value);
                return;
            }
            ACIA_MIDI_CONTROL => {
                self.acia_midi.write(acia::reg::CONTROL_STATUS, value);
                return;
            }
            ACIA_MIDI_DATA => {
                self.acia_midi.write(acia::reg::DATA, value);
                return;
            }
            YM2149_SELECT => {
                self.ym2149.write(ym2149::bus_offset::SELECT, value);
                return;
            }
            YM2149_DATA => {
                self.ym2149.write(ym2149::bus_offset::DATA, value);
                return;
            }
            // GLUE synchronization register ($FF820A): a SINGLE hardware
            // register, but two distinct effects modeled in two separate
            // components — a switch to 60Hz well-placed in cycle terms
            // near the top/bottom of the displayed window removes the
            // corresponding VERTICAL border for the current frame
            // (`Glue::write_sync`), while a switch well-placed near the
            // end of the line (`RIGHT_OFF`) or very early in the line
            // (nudges `LEFT_PLUS_2`/`RIGHT_MINUS_2`) removes/adjusts the
            // HORIZONTAL border of the current line (`Shifter::write_sync`,
            // see its module doc) — both need the cycle position within
            // the line, known only to the board.
            GLUE_SYNC => {
                self.glue.write_sync(value);
                self.shifter.write_sync(value, self.glue.cycles_in_line());
                return;
            }
            // STE fine scrolling ($FF8264/$FF8265) and line width
            // ($FF820F): unlike other Shifter registers, their effect
            // depends on the CYCLE of the write within the current line —
            // a write before the start of active display (or during a
            // line outside the visible area, top/bottom border: nothing
            // to protect) applies to the CURRENT line, a later write is
            // deferred to the next line (`pending_*` on the `Shifter`
            // side). Cycle-exact thresholds identical to Hatari
            // (`video.c`, `Video_HorScroll_Write`/`Video_LineWidth_WriteByte`):
            // `line_start_cycle()` (56 PAL/52 NTSC) for scrolling,
            // `line_end_cycle()` (376 PAL/372 NTSC, end of active
            // display) for line width — different threshold because
            // LineWidth is added to the address at the moment active
            // display ends, not at its start.
            shifter::addr::HSCROLL_NO_PREFETCH | shifter::addr::HSCROLL_PREFETCH => {
                let visible = self.glue.display_line().is_some();
                let apply_now = !visible || self.glue.cycles_in_line() <= self.glue.line_start_cycle();
                let prefetch = addr == shifter::addr::HSCROLL_PREFETCH;
                self.shifter.write_hscroll(value, prefetch, apply_now);
                return;
            }
            shifter::addr::LINE_WIDTH => {
                let visible = self.glue.display_line().is_some();
                let apply_now = !visible || self.glue.cycles_in_line() <= self.glue.line_end_cycle();
                self.shifter.write_line_width(value, apply_now);
                return;
            }
            // Resolution ($FF8260): same principle as above, the
            // cycle position within the line determines whether a brief
            // switch to high resolution/back triggers the STE left-border
            // removal trick (`LEFT_OFF_2_STE`, see `Shifter`'s
            // module doc) — only the board knows this position.
            shifter::addr::RESOLUTION => {
                self.shifter.write_resolution(value, self.glue.cycles_in_line());
                return;
            }
            _ if Self::is_shifter_addr(addr) => {
                self.shifter.write(addr, value);
                return;
            }
            FDC_DATA => {
                if self.dma_sector_count_mode {
                    // DMA_MODE bit 4 set: this is NOT a WD1772
                    // register, see `dma_sector_count`'s doc.
                    self.dma_sector_count = Some(value as u16);
                    return;
                }
                if self.dma_register_select == wd1772::reg::COMMAND_STATUS {
                    let (drive_a_selected, side) = self.floppy_drive_select();
                    self.wd1772.side = side;
                    let disk = if drive_a_selected { self.floppy_a.as_deref_mut() } else { None };
                    // Only STARTS the command now (sets
                    // BUSY): it's `AtariSt::tick` that advances it
                    // and actually completes it, see the doc of
                    // `Wd1772::execute_command`/`Wd1772::tick`.
                    self.wd1772.execute_command(value, disk);
                } else {
                    self.wd1772.write_simple_register(self.dma_register_select, value);
                }
                return;
            }
            DMA_MODE => {
                // Bits 1-2 (A1-A0), not 0-1 — see `DMA_MODE`'s doc.
                self.dma_register_select = (value & 0x6) >> 1;
                self.dma_sector_count_mode = value & 0x10 != 0;
                return;
            }
            DMA_ADDR_HIGH => {
                self.dma_address = (self.dma_address & 0x00FFFF) | ((value as u32) << 16);
                return;
            }
            DMA_ADDR_MID => {
                self.dma_address = (self.dma_address & 0xFF00FF) | ((value as u32) << 8);
                return;
            }
            DMA_ADDR_LOW => {
                self.dma_address = (self.dma_address & 0xFFFF00) | value as u32;
                return;
            }
            _ if self.blitter_present && addr == BLITTER_BASE + blitter::reg::CONTROL => {
                self.blitter.write(blitter::reg::CONTROL, value);
                // BUSY/START bit (bit 7) set: triggers the blit in its
                // entirety (synchronous model, see peripherals::atari_st::blitter).
                if value & 0x80 != 0 {
                    if std::env::var("RUST68_TRACE_BLITTER").is_ok() {
                        let word = |a, b| ((self.blitter.read(a) as u32) << 8) | self.blitter.read(b) as u32;
                        let long = |a: u32| {
                            ((self.blitter.read(a) as u32) << 24)
                                | ((self.blitter.read(a + 1) as u32) << 16)
                                | ((self.blitter.read(a + 2) as u32) << 8)
                                | self.blitter.read(a + 3) as u32
                        };
                        let halftone_table: Vec<String> = (0..16)
                            .map(|i| {
                                format!(
                                    "{:04x}",
                                    word(
                                        blitter::reg::HALFTONE_BASE + i * 2,
                                        blitter::reg::HALFTONE_BASE + i * 2 + 1
                                    )
                                )
                            })
                            .collect();
                        eprintln!(
                            "[trace] blit : pc={:#08x} src={:#08x} dst={:#08x} x={} y={} hop={} op={:#03x} skew={:#04x} control={:#04x} endmask1={:#06x} endmask2={:#06x} endmask3={:#06x} src_xinc={} src_yinc={} dst_xinc={} dst_yinc={} halftone=[{}]",
                            self.last_pc,
                            long(blitter::reg::SRC_ADDR),
                            long(blitter::reg::DST_ADDR),
                            word(blitter::reg::X_COUNT, blitter::reg::X_COUNT1),
                            word(blitter::reg::Y_COUNT, blitter::reg::Y_COUNT1),
                            self.blitter.read(blitter::reg::HOP),
                            self.blitter.read(blitter::reg::OP),
                            self.blitter.read(blitter::reg::SKEW),
                            value,
                            word(blitter::reg::ENDMASK_1, blitter::reg::ENDMASK_11),
                            word(blitter::reg::ENDMASK_2, blitter::reg::ENDMASK_21),
                            word(blitter::reg::ENDMASK_3, blitter::reg::ENDMASK_31),
                            word(blitter::reg::SRC_X_INC, blitter::reg::SRC_X_INC1) as i16,
                            word(blitter::reg::SRC_Y_INC, blitter::reg::SRC_Y_INC1) as i16,
                            word(blitter::reg::DST_X_INC, blitter::reg::DST_X_INC1) as i16,
                            word(blitter::reg::DST_Y_INC, blitter::reg::DST_Y_INC1) as i16,
                            halftone_table.join(","),
                        );
                    }
                    // `RUST68_TRACE_BLITTER_MEM=1`: dumps the actual
                    // destination content (every word of the first line,
                    // spaced by DST_X_INC) before/after `execute()`, to
                    // directly check the result written by the Blitter
                    // rather than deducing it by hand from the
                    // parameters alone.
                    let trace_mem = std::env::var("RUST68_TRACE_BLITTER_MEM").is_ok();
                    let mem_dst_addr = ((self.blitter.read(blitter::reg::DST_ADDR) as u32) << 24)
                        | ((self.blitter.read(blitter::reg::DST_ADDR1) as u32) << 16)
                        | ((self.blitter.read(blitter::reg::DST_ADDR2) as u32) << 8)
                        | self.blitter.read(blitter::reg::DST_ADDR3) as u32;
                    let mem_x_count = ((self.blitter.read(blitter::reg::X_COUNT) as u32) << 8)
                        | self.blitter.read(blitter::reg::X_COUNT1) as u32;
                    let mem_dst_xinc = (((self.blitter.read(blitter::reg::DST_X_INC) as u16) << 8)
                        | self.blitter.read(blitter::reg::DST_X_INC1) as u16) as i16;
                    if trace_mem {
                        let before: Vec<String> = (0..mem_x_count.max(1).min(20))
                            .map(|i| {
                                let a = mem_dst_addr.wrapping_add((i as i32 * mem_dst_xinc as i32) as u32);
                                format!("{:04x}", self.read16(a))
                            })
                            .collect();
                        eprintln!("[blitmem] AVANT dst={mem_dst_addr:#08x} : [{}]", before.join(","));
                    }
                    let mut ram_bus = RamBus {
                        ram: &mut self.ram,
                        rom: &self.rom,
                        rom_base: self.rom_base,
                    };
                    self.blitter.execute(&mut ram_bus);
                    if trace_mem {
                        let after: Vec<String> = (0..mem_x_count.max(1).min(20))
                            .map(|i| {
                                let a = mem_dst_addr.wrapping_add((i as i32 * mem_dst_xinc as i32) as u32);
                                format!("{:04x}", self.read16(a))
                            })
                            .collect();
                        eprintln!("[blitmem] APRES dst={mem_dst_addr:#08x} : [{}]", after.join(","));
                    }
                }
                return;
            }
            _ if self.is_blitter_addr(addr) => {
                self.blitter.write(addr - BLITTER_BASE, value);
                return;
            }
            STE_MICROWIRE_MASK => {
                self.microwire.write_mask_high(value);
                self.ste_dma_sound[(addr - STE_DMA_SOUND_BASE) as usize] = value;
                return;
            }
            STE_MICROWIRE_MASK1 => {
                self.microwire.write_mask_low(value);
                self.ste_dma_sound[(addr - STE_DMA_SOUND_BASE) as usize] = value;
                return;
            }
            STE_MICROWIRE_DATA => {
                self.microwire.write_data_high(value);
                return;
            }
            STE_MICROWIRE_DATA1 => {
                self.microwire.write_data_low(value);
                return;
            }
            _ if (STE_DMA_SOUND_BASE..=STE_DMA_SOUND_END).contains(&addr) => {
                let off = addr - STE_DMA_SOUND_BASE;
                if Self::is_dma_sound_reg(off) {
                    self.dma_sound.write(off, value);
                } else {
                    self.ste_dma_sound[off as usize] = value;
                }
                return;
            }
            _ => {}
        }
        if self.in_rom(addr) {
            return; // ROM: write ignored (read-only on real silicon)
        }
        if (IO_BASE..=IO_END).contains(&addr) || (CARTRIDGE_BASE..=CARTRIDGE_END).contains(&addr) {
            return; // unemulated peripheral/cartridge: write ignored
        }
        if std::env::var("RUST68_TRACE_VECTORS").is_ok() {
            eprintln!("[trace] bus fault on write: addr={addr:#x} value={value:#04x}");
        }
        self.bus_fault = Some((addr, true));
    }

    fn reset_bus(&mut self) {
        // The RESET instruction generates /RESET towards the external peripherals.
        // The GLUE is NOT reset: on real silicon, the video
        // timing keeps running independently of a CPU /RESET (the
        // monitor stays synchronized).
        self.mfp = Mfp::new();
        self.mfp.set_gpip_input(GPIP_MONO_DETECT, true);
        self.mfp.set_gpip_input(4, true);
        self.mfp.set_gpip_input(5, true);
        self.acia_keyboard = Acia::new();
        self.ikbd = Ikbd::new();
        self.acia_midi = Acia::new();
        self.ym2149 = Ym2149::new();
        self.dma_sound = DmaSound::new();
        // `self.microwire` deliberately NOT reset here: the real
        // Microwire/LMC1992 circuit has no wired reset signal,
        // confirmed by Hatari (`dmaSnd.c`: "Microwire has no reset
        // signal, it will keep its values on warm reset").
        // `Shifter::reset` (not `Shifter::new()`): preserves `ste_palette`,
        // a silicon characteristic (see its doc) that RESET must
        // not erase.
        self.shifter.reset();
        self.wd1772 = Wd1772::new();
        self.dma_register_select = 0;
        self.dma_address = 0;
        self.dma_sector_count_mode = false;
        self.dma_sector_count = None;
        self.blitter = Blitter::new();
        self.overlay = true;
        self.memory_conf = 0;
        // The inserted disk (floppy_a) itself is not ejected by /RESET:
        // it's a physical medium, not chip state.
        // The GLUE is not reset (see above): just resynchronize
        // line/frame tracking on its current position so as not to
        // trigger a massive catch-up on the next tick().
        self.last_vbl_edge = self.glue.vbl_edge_count();
        self.last_absolute_line =
            self.glue.frame_count() * self.glue.lines_per_frame() as u64 + self.glue.current_line() as u64;
    }

    fn take_bus_fault(&mut self) -> Option<(u32, bool)> {
        self.bus_fault.take()
    }

    fn has_pending_bus_fault(&self) -> bool {
        self.bus_fault.is_some()
    }

    fn irq_level(&self) -> u8 {
        // Real ST/STE hardware wiring, by decreasing priority:
        // MFP (IPL6) > VBL (IPL4) > HBL (IPL2).
        if self.mfp.interrupt_requested() {
            6
        } else if self.glue.vbl_pending() {
            4
        } else if self.glue.hbl_pending() {
            2
        } else {
            0
        }
    }

    fn irq_ack(&mut self, level: u8) -> u8 {
        if self.trace_irq {
            eprintln!("[irq] niveau={level} pc={:#08x}", self.last_pc);
        }
        match level {
            6 => self.mfp.iack(),
            4 => {
                self.glue.ack_vbl();
                24 + 4 // autovecteur niveau 4
            }
            2 => {
                self.glue.ack_hbl();
                24 + 2 // autovecteur niveau 2
            }
            _ => 24 + level,
        }
    }
}
