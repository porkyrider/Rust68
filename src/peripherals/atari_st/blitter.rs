//! Blitter — the Atari STE's block transfer coprocessor (BitBlt).
//!
//! Combines a source word (optionally shifted bit by bit via `skew` to
//! align images not aligned on a word boundary), a halftone pattern, and
//! the current destination content, via a programmable boolean function
//! (`OP`, one of the 16 two-input functions), with line-edge masking
//! (`ENDMASK1/2/3`) and X/Y increment-based traversal.
//!
//! This module models the chip **alone**: [`Blitter::execute`] takes a
//! `Bus` (to read/write RAM at the source/destination addresses) and
//! advances the blit — in HOG mode (bit 6 of CONTROL set), it executes
//! entirely in a single call; in shared mode (non-HOG, the most common
//! case in practice), a single call only processes a 16-word slice before
//! yielding back (see [`Self::execute`] and its doc for details), with
//! `BUSY` staying set between two slices — so well observable by polling
//! in this mode, unlike a previous, fully synchronous version of this
//! module. It's up to the board to map [`Blitter::read`]/[`Blitter::write`]
//! into its `Bus`, to trigger a first call to `execute` when the START bit
//! of the control register is written, AND to call `execute` back
//! periodically to advance a paused non-HOG blit — see
//! `systems::atari_st::AtariSt::tick`, which does this autonomously at the
//! CPU's pace (bus accesses shared with the CPU, like on real silicon),
//! rather than relying solely on a software rewrite of CONTROL (which
//! happened to work with TOS's `TAS.B` loop but not with a simple polling
//! `BTST.B`).
//!
//! Registers and the semantics of the `FXSR`/`NFSR`/`SMUDGE`/HOP/halftone
//! line number bits cross-checked against several independent sources:
//! the `BLITTER.TXT` datasheet (info-coach.fr), `BLIT_FAQ.TXT`
//! (the `ggnkua/Atari_ST_Sources` repo), and Hatari's source code
//! (`src/blitter.c`), which all agree — see the per-item detail below.
//! Unlike the Amiga, the Atari STE Blitter **does not** have a
//! "line-draw" mode for polygon drawing: the "line number" field of the
//! CONTROL register is only used to select/pre-position the current
//! halftone word (modeled below).
//!
//! ## Known limitations (v1) — take with caution
//! - Bus cycle stealing from the CPU ("hog"/"steal" mode) IS modeled (see
//!   above and `systems::atari_st::AtariSt::tick`/`BLITTER_SLICE_CYCLES`),
//!   via a slice of 64 REAL bus accesses between two bus handoffs
//!   (`BUS_ACCESSES_PER_SLICE` in [`Self::execute`] — source read,
//!   destination read and destination write each counted separately, not
//!   a number of words processed), taken directly from Hatari's source
//!   (`src/blitter.c`, `BLITTER_NONHOG_BUS_BLITTER`). Still remains an
//!   approximation compared to Hatari's "cycle exact" mode, which
//!   interleaves these accesses IN THE MIDDLE of CPU instruction execution
//!   (rather than between whole instructions as here) and reproduces a
//!   documented real-silicon bug case where the Blitter sometimes stops
//!   at 63 accesses instead of 64 — not modeled here.
//! - No TomHarte-equivalent test suite exists for the Blitter: the logic
//!   is verified by cross-referencing documentation (datasheet,
//!   BLIT_FAQ.TXT, Hatari's source code) rather than against hardware
//!   test vectors.

/// Register offsets in the chip's own address space (to be added to the
/// board's base address, `0xFF8A00` on a real STE).
pub mod reg {
    /// 16 halftone pattern words, offsets `0x00`, `0x02`, … `0x1E`.
    pub const HALFTONE_BASE: u32 = 0x00;
    pub const SRC_X_INC: u32 = 0x20;
    pub const SRC_X_INC1: u32 = 0x21;
    pub const SRC_Y_INC: u32 = 0x22;
    pub const SRC_Y_INC1: u32 = 0x23;
    /// Source address (32 bits, only the low 24 bits are significant).
    pub const SRC_ADDR: u32 = 0x24;
    pub const SRC_ADDR1: u32 = 0x25;
    pub const SRC_ADDR2: u32 = 0x26;
    pub const SRC_ADDR3: u32 = 0x27;
    pub const ENDMASK_1: u32 = 0x28;
    pub const ENDMASK_11: u32 = 0x29;
    pub const ENDMASK_2: u32 = 0x2A;
    pub const ENDMASK_21: u32 = 0x2B;
    pub const ENDMASK_3: u32 = 0x2C;
    pub const ENDMASK_31: u32 = 0x2D;
    pub const DST_X_INC: u32 = 0x2E;
    pub const DST_X_INC1: u32 = 0x2F;
    pub const DST_Y_INC: u32 = 0x30;
    pub const DST_Y_INC1: u32 = 0x31;
    /// Destination address (32 bits, only the low 24 bits are significant).
    pub const DST_ADDR: u32 = 0x32;
    pub const DST_ADDR1: u32 = 0x33;
    pub const DST_ADDR2: u32 = 0x34;
    pub const DST_ADDR3: u32 = 0x35;
    pub const X_COUNT: u32 = 0x36;
    pub const X_COUNT1: u32 = 0x37;
    pub const Y_COUNT: u32 = 0x38;
    pub const Y_COUNT1: u32 = 0x39;
    pub const HOP: u32 = 0x3A;
    pub const OP: u32 = 0x3B;
    /// Bit 7 = BUSY (write: start/stop the blit; read: busy/idle),
    /// bit 6 = HOG, bit 5 = SMUDGE, bits 3-0 = current halftone line
    /// number — directly readable/writable (not a hidden internal
    /// counter: software can pre-position it).
    pub const CONTROL: u32 = 0x3C;
    /// Bit 7 = FXSR (Force eXtra Source Read), bit 6 = NFSR (No Final
    /// Source Read), bits 3-0 = skew (number of right-shift bits).
    pub const SKEW: u32 = 0x3D;
    /// End of the register space (exclusive).
    pub const END: u32 = 0x3E;
}

const CONTROL_BUSY: u8 = 1 << 7;
const CONTROL_HOG: u8 = 1 << 6;

/// Current CPU PC, for diagnostics only (`RUST68_TRACE_BLIT_REGS`) —
/// updated by the caller (e.g. `examples/rd_menu_ca6a.rs`) right before
/// each `cpu.step()`, read by the register write traces below to identify
/// the responsible ROM instruction without changing the public signature
/// of `write`/`write_word`.
pub static DEBUG_LAST_PC: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

#[derive(Debug, Clone)]
pub struct Blitter {
    halftone: [u16; 16],
    src_x_inc: i16,
    src_y_inc: i16,
    src_addr: u32,
    endmask: [u16; 3],
    dst_x_inc: i16,
    dst_y_inc: i16,
    dst_addr: u32,
    /// Words per line. Stored as 32 bits (not 16) so it can represent the
    /// value 65536 — see [`Self::write_word_count`].
    x_count: u32,
    /// Lines per block. Same remark as [`Self::x_count`].
    y_count: u32,
    hop: u8,
    op: u8,
    skew: u8,
    /// Bit 7 = BUSY, bit 6 = HOG, bit 5 = SMUDGE, bits 3-0 = current
    /// halftone line number (see [`reg::CONTROL`]).
    control: u8,
    /// Blitter "armed" state — see [`Self::execute`] for the real bug
    /// this field fixes.
    armed: bool,
    /// True between the start and the REAL end (Y_COUNT reaching 0) of a
    /// blit — i.e. as long as work remains, including between two slices
    /// in non-HOG mode (see [`Self::execute`]). Distinct from `armed`:
    /// `armed` authorizes a START (or explicit restart, Y_COUNT was just
    /// rewritten), `mid_blit` authorizes CONTINUING a blit already started
    /// without any register needing to be rewritten between slices.
    mid_blit: bool,
    /// 32-bit source shift register, persistent between slices (see
    /// [`Self::execute`]).
    buffer: u32,
    /// Last word actually read from the source bus — reused by NFSR (see
    /// [`Self::execute`]).
    bus_word: u16,
    /// True if the FXSR priming read of the CURRENT line has already
    /// happened (reset to false on every new line).
    have_fxsr: bool,
    /// True if the next source read must be skipped (NFSR, set
    /// dynamically when X_COUNT reaches 2).
    nfsr_dynamic: bool,
    /// Width (in words) of the line currently being blitted — captured
    /// once at the true start of the blit (unlike `x_count`, which
    /// actually counts down and resets to this value at the end of each
    /// line).
    x_count_reset: u32,
}

impl Default for Blitter {
    fn default() -> Self {
        Self::new()
    }
}

impl Blitter {
    pub fn new() -> Self {
        Blitter {
            halftone: [0; 16],
            src_x_inc: 0,
            src_y_inc: 0,
            src_addr: 0,
            endmask: [0xFFFF; 3],
            dst_x_inc: 0,
            dst_y_inc: 0,
            dst_addr: 0,
            x_count: 0,
            y_count: 0,
            hop: 0,
            op: 0,
            skew: 0,
            control: 0,
            armed: false,
            mid_blit: false,
            buffer: 0,
            bus_word: 0,
            have_fxsr: false,
            nfsr_dynamic: false,
            x_count_reset: 0,
        }
    }

    /// True if the CONTROL register's BUSY bit is active. In HOG mode,
    /// always false right after [`Self::execute`] (the blit finishes
    /// there in a single call). In non-HOG mode, can stay true between
    /// two slices of a blit still in progress — see the module doc — so
    /// well observable by polling in this mode, unlike a fully
    /// synchronous model. Consulted by
    /// `systems::atari_st::AtariSt::tick` to know whether to keep
    /// advancing the blit.
    pub fn busy(&self) -> bool {
        self.control & CONTROL_BUSY != 0
    }

    /// Reads the register at offset `addr` (see [`reg`]).
    pub fn read(&self, addr: u32) -> u8 {
        match addr {
            a if a < 0x20 && a % 2 == 0 => (self.halftone[(a / 2) as usize] >> 8) as u8,
            a if a < 0x20 => self.halftone[(a / 2) as usize] as u8,
            reg::SRC_X_INC => (self.src_x_inc >> 8) as u8,
            reg::SRC_X_INC1 => self.src_x_inc as u8,
            reg::SRC_Y_INC => (self.src_y_inc >> 8) as u8,
            reg::SRC_Y_INC1 => self.src_y_inc as u8,
            reg::SRC_ADDR => (self.src_addr >> 24) as u8,
            reg::SRC_ADDR1 => (self.src_addr >> 16) as u8,
            reg::SRC_ADDR2 => (self.src_addr >> 8) as u8,
            reg::SRC_ADDR3 => self.src_addr as u8,
            reg::ENDMASK_1 => (self.endmask[0] >> 8) as u8,
            reg::ENDMASK_11 => self.endmask[0] as u8,
            reg::ENDMASK_2 => (self.endmask[1] >> 8) as u8,
            reg::ENDMASK_21 => self.endmask[1] as u8,
            reg::ENDMASK_3 => (self.endmask[2] >> 8) as u8,
            reg::ENDMASK_31 => self.endmask[2] as u8,
            reg::DST_X_INC => (self.dst_x_inc >> 8) as u8,
            reg::DST_X_INC1 => self.dst_x_inc as u8,
            reg::DST_Y_INC => (self.dst_y_inc >> 8) as u8,
            reg::DST_Y_INC1 => self.dst_y_inc as u8,
            reg::DST_ADDR => (self.dst_addr >> 24) as u8,
            reg::DST_ADDR1 => (self.dst_addr >> 16) as u8,
            reg::DST_ADDR2 => (self.dst_addr >> 8) as u8,
            reg::DST_ADDR3 => self.dst_addr as u8,
            // The hardware register stays 16 bits: reading back after a
            // write that converted 0 to 65536 internally (see
            // `write_word_count`) gives back 0, not 65536 — confirmed by
            // Hatari (`Blitter_WordsPerLine_ReadWord`, `& 0xFFFF` mask).
            reg::X_COUNT => ((self.x_count & 0xFFFF) >> 8) as u8,
            reg::X_COUNT1 => (self.x_count & 0xFF) as u8,
            reg::Y_COUNT => ((self.y_count & 0xFFFF) >> 8) as u8,
            reg::Y_COUNT1 => (self.y_count & 0xFF) as u8,
            reg::HOP => self.hop,
            reg::OP => self.op,
            reg::SKEW => self.skew,
            reg::CONTROL => self.control,
            _ => 0xFF,
        }
    }

    /// Writes the register at offset `addr`.
    ///
    /// Note on isolated `.B` access: the official Blitter manual and
    /// Hatari (`Blitter_CheckAccess_Byte`) document that most of these
    /// registers IGNORE an isolated `.B` access on real silicon (only a
    /// full `.W`/`.L` access is honored). An attempt to faithfully
    /// implement this rule here caused an immediate crash on the first
    /// blit triggered by this exact TOS/demo — a sign that the real
    /// software does in fact rely, somewhere, on a `.B` access to compose
    /// a register, contrary to what Hatari documents for the reference
    /// hardware it emulates. Byte-by-byte composition kept below until
    /// this discrepancy is understood.
    pub fn write(&mut self, addr: u32, value: u8) {
        match addr {
            a if a < 0x20 && a % 2 == 0 => {
                let w = &mut self.halftone[(a / 2) as usize];
                *w = (*w & 0x00FF) | ((value as u16) << 8);
            }
            a if a < 0x20 => {
                let w = &mut self.halftone[(a / 2) as usize];
                *w = (*w & 0xFF00) | value as u16;
            }
            reg::SRC_X_INC => {
                self.src_x_inc =
                    ((((self.src_x_inc as u16) & 0x00FF) | ((value as u16) << 8)) & 0xFFFE) as i16
            }
            reg::SRC_X_INC1 => {
                self.src_x_inc =
                    ((((self.src_x_inc as u16) & 0xFF00) | value as u16) & 0xFFFE) as i16
            }
            reg::SRC_Y_INC => {
                self.src_y_inc =
                    ((((self.src_y_inc as u16) & 0x00FF) | ((value as u16) << 8)) & 0xFFFE) as i16
            }
            reg::SRC_Y_INC1 => {
                self.src_y_inc =
                    ((((self.src_y_inc as u16) & 0xFF00) | value as u16) & 0xFFFE) as i16
            }
            reg::SRC_ADDR => self.src_addr = (self.src_addr & 0x00FF_FFFF) | ((value as u32) << 24),
            reg::SRC_ADDR1 => {
                self.src_addr = (self.src_addr & 0xFF00_FFFF) | ((value as u32) << 16)
            }
            reg::SRC_ADDR2 => self.src_addr = (self.src_addr & 0xFFFF_00FF) | ((value as u32) << 8),
            // Bit 0 forced to zero (a word-only wired register on real
            // silicon — the same constraint `write_long` already applies
            // to the full `.L` write, and that the increment registers
            // already apply to their own low byte below): without this
            // masking, an isolated low-byte write to SRC_ADDR could leave
            // an odd address, misaligning word reads and mixing up
            // interleaved bitplanes.
            reg::SRC_ADDR3 => self.src_addr = (self.src_addr & 0xFFFF_FF00) | (value as u32 & 0xFE),
            reg::ENDMASK_1 => self.endmask[0] = (self.endmask[0] & 0x00FF) | ((value as u16) << 8),
            reg::ENDMASK_11 => self.endmask[0] = (self.endmask[0] & 0xFF00) | value as u16,
            reg::ENDMASK_2 => self.endmask[1] = (self.endmask[1] & 0x00FF) | ((value as u16) << 8),
            reg::ENDMASK_21 => self.endmask[1] = (self.endmask[1] & 0xFF00) | value as u16,
            reg::ENDMASK_3 => self.endmask[2] = (self.endmask[2] & 0x00FF) | ((value as u16) << 8),
            reg::ENDMASK_31 => self.endmask[2] = (self.endmask[2] & 0xFF00) | value as u16,
            reg::DST_X_INC => {
                self.dst_x_inc =
                    ((((self.dst_x_inc as u16) & 0x00FF) | ((value as u16) << 8)) & 0xFFFE) as i16
            }
            reg::DST_X_INC1 => {
                self.dst_x_inc =
                    ((((self.dst_x_inc as u16) & 0xFF00) | value as u16) & 0xFFFE) as i16
            }
            reg::DST_Y_INC => {
                self.dst_y_inc =
                    ((((self.dst_y_inc as u16) & 0x00FF) | ((value as u16) << 8)) & 0xFFFE) as i16
            }
            reg::DST_Y_INC1 => {
                self.dst_y_inc =
                    ((((self.dst_y_inc as u16) & 0xFF00) | value as u16) & 0xFFFE) as i16
            }
            reg::DST_ADDR => self.dst_addr = (self.dst_addr & 0x00FF_FFFF) | ((value as u32) << 24),
            reg::DST_ADDR1 => {
                self.dst_addr = (self.dst_addr & 0xFF00_FFFF) | ((value as u32) << 16)
            }
            reg::DST_ADDR2 => self.dst_addr = (self.dst_addr & 0xFFFF_00FF) | ((value as u32) << 8),
            // Bit 0 forced to zero — see the equivalent comment on
            // `SRC_ADDR3` just above.
            reg::DST_ADDR3 => self.dst_addr = (self.dst_addr & 0xFFFF_FF00) | (value as u32 & 0xFE),
            reg::X_COUNT => self.x_count = (self.x_count & 0x00FF) | ((value as u32) << 8),
            reg::X_COUNT1 => self.x_count = (self.x_count & 0xFF00) | value as u32,
            reg::Y_COUNT => {
                if std::env::var("RUST68_TRACE_BLIT_REGS").is_ok() {
                    eprintln!("[blit-reg] pc={:#010x} write Y_COUNT(hi)={value:#04x} (current_skew={:#04x} current_dst_addr={:#010x})", DEBUG_LAST_PC.load(std::sync::atomic::Ordering::Relaxed), self.skew, self.dst_addr);
                }
                self.y_count = (self.y_count & 0x00FF) | ((value as u32) << 8);
                self.armed = true;
            }
            reg::Y_COUNT1 => {
                if std::env::var("RUST68_TRACE_BLIT_REGS").is_ok() {
                    eprintln!("[blit-reg] pc={:#010x} write Y_COUNT(lo)={value:#04x} (current_skew={:#04x} current_dst_addr={:#010x})", DEBUG_LAST_PC.load(std::sync::atomic::Ordering::Relaxed), self.skew, self.dst_addr);
                }
                self.y_count = (self.y_count & 0xFF00) | value as u32;
                self.armed = true;
            }
            reg::HOP => self.hop = value & 0x03,
            reg::OP => self.op = value & 0x0F,
            reg::SKEW => {
                if std::env::var("RUST68_TRACE_BLIT_REGS").is_ok() {
                    eprintln!("[blit-reg] pc={:#010x} write SKEW={value:#04x}", DEBUG_LAST_PC.load(std::sync::atomic::Ordering::Relaxed));
                }
                self.skew = value;
            }
            reg::CONTROL => {
                if std::env::var("RUST68_TRACE_BLIT_REGS").is_ok() {
                    eprintln!("[blit-reg] pc={:#010x} write CONTROL={value:#04x} (current_skew={:#04x})", DEBUG_LAST_PC.load(std::sync::atomic::Ordering::Relaxed), self.skew);
                }
                self.write_control(value);
            }
            _ => {}
        }
    }

    /// Writes a 16-bit register as a full word — the path taken by the
    /// board for any real CPU `.W` access on SRC_X_INC/SRC_Y_INC/
    /// ENDMASK1-3/DST_X_INC/DST_Y_INC/X_COUNT/Y_COUNT (see [`Self::write`]'s
    /// doc: an isolated `.B` access on these registers is ignored
    /// on real silicon, only this full-word access is honored).
    ///
    /// X_COUNT/Y_COUNT: the official Blitter manual and Hatari document 0
    /// as meaning 65536 — but THREE independent attempts at
    /// implementing this rule (two directly in `execute` on a
    /// 16-bit field, then one at write time with correctly-sized
    /// 32-bit storage) each markedly worsened the
    /// corruption observed in practice on this specific TOS/use case —
    /// reverted to storing the written value as-is (no conversion) while
    /// waiting to locate the real upstream cause.
    pub fn write_word(&mut self, addr: u32, value: u16) {
        match addr {
            a if a < 0x20 && a % 2 == 0 => self.halftone[(a / 2) as usize] = value,
            reg::SRC_X_INC => self.src_x_inc = (value & 0xFFFE) as i16,
            reg::SRC_Y_INC => self.src_y_inc = (value & 0xFFFE) as i16,
            reg::DST_X_INC => self.dst_x_inc = (value & 0xFFFE) as i16,
            reg::DST_Y_INC => self.dst_y_inc = (value & 0xFFFE) as i16,
            reg::ENDMASK_1 => self.endmask[0] = value,
            reg::ENDMASK_2 => self.endmask[1] = value,
            reg::ENDMASK_3 => self.endmask[2] = value,
            reg::X_COUNT => self.x_count = value as u32,
            reg::Y_COUNT => {
                if std::env::var("RUST68_TRACE_BLIT_REGS").is_ok() {
                    eprintln!("[blit-reg] pc={:#010x} write_word Y_COUNT={value:#06x} (current_skew={:#04x} current_dst_addr={:#010x})", DEBUG_LAST_PC.load(std::sync::atomic::Ordering::Relaxed), self.skew, self.dst_addr);
                }
                self.y_count = value as u32;
                self.armed = true;
            }
            _ => {}
        }
    }

    /// Writes the CONTROL register while accounting for the Blitter's
    /// "arming" — see [`Self::execute`] for details on the real bug that
    /// this logic fixes (accidental restarts via `TAS.B` in the
    /// non-HOG mode's resume loop).
    ///
    /// On real silicon (official Blitter manual, section on shared
    /// CPU/Blitter mode): "If the BUSY flag is reset when the Y_Count
    /// is zero, the flag will remain clear indicating BLiTTER completion
    /// and the BLiTTER won't be restarted." — as long as the software hasn't
    /// explicitly rewritten Y_COUNT since the last complete execution,
    /// any attempt to set the BUSY bit (including via `TAS.B`, used
    /// by TOS to resume the Blitter slice by slice in shared
    /// mode) has no effect — the bit stays readable as 0. Without this
    /// protection, each iteration of the resume loop re-executed
    /// a COMPLETE blit from the addresses already advanced by the previous
    /// round, writing incorrect content well beyond the intended
    /// area — the real cause, once isolated by direct comparison with the
    /// Blitter enabled/disabled, of the visual corruption observed
    /// (a pattern "already there" even before a tint blit is applied, for
    /// example).
    fn write_control(&mut self, value: u8) {
        if value & CONTROL_BUSY != 0 && !self.armed && !self.mid_blit {
            self.control = (self.control & CONTROL_BUSY) | (value & !CONTROL_BUSY);
        } else {
            self.control = value;
        }
    }

    /// Writes SRC_ADDR or DST_ADDR as a full longword (32 bits, only the
    /// low 24 bits are significant) — same principle as
    /// [`Self::write_word`] for the 16-bit registers: an isolated `.B` or
    /// `.W` access on these registers is ignored on real silicon, only
    /// a full `.L` access is honored.
    pub fn write_long(&mut self, addr: u32, value: u32) {
        if std::env::var("RUST68_TRACE_BLIT_REGS").is_ok()
            && matches!(addr, reg::SRC_ADDR | reg::DST_ADDR)
        {
            eprintln!(
                "[blit-reg] pc={:#010x} write_long {}={:#010x} current_control={:#04x} busy={} mid_blit={} armed={}",
                DEBUG_LAST_PC.load(std::sync::atomic::Ordering::Relaxed),
                if addr == reg::SRC_ADDR { "SRC_ADDR" } else { "DST_ADDR" },
                value, self.control, self.busy(), self.mid_blit, self.armed,
            );
        }
        match addr {
            reg::SRC_ADDR => self.src_addr = value & 0x00FF_FFFE,
            reg::DST_ADDR => self.dst_addr = value & 0x00FF_FFFE,
            _ => {}
        }
    }

    /// Applies the halftone function (`HOP`, 2 bits): combines the source
    /// word and the current halftone word according to the datasheet's
    /// standard table (0=all set to 1, 1=halftone only, 2=source only,
    /// 3=source AND halftone).
    fn apply_hop(&self, source: u16, halftone: u16) -> u16 {
        match self.hop & 0x3 {
            0 => 0xFFFF,
            1 => halftone,
            2 => source,
            3 => source & halftone,
            _ => unreachable!(),
        }
    }

    /// Applies the programmable boolean function (`OP`, 4 bits): for
    /// each bit position, the index `3 - ((s<<1)|d)` (i.e.
    /// `(NOT s << 1) | NOT d`) selects the output bit in the 4-bit
    /// truth table.
    ///
    /// Convention verified by directly solving the system of equations
    /// posed by the official Blitter manual (`User Manual for the Atari ST
    /// Bit-Block Transfer Processor`, archive.org, cross-checked against
    /// `BLITTER.TXT` — both give the same table): OP=1 "source AND
    /// destination", OP=2 "source AND NOT destination", OP=4 "NOT source
    /// AND destination", OP=8 "NOT source AND NOT destination" are only
    /// simultaneously satisfiable with this inverted index — the
    /// direct index `(s<<1)|d` ("natural" Amiga/X11 convention, mistakenly
    /// used here before) gives e.g. OP=3 = "NOT source" instead of
    /// "source", and OP=7 = NOT(s AND d) instead of "source OR destination"
    /// — a mix-up that affects the rendering of any blit not using
    /// one of the 4 symmetric functions (0x0/0x5/0xA/0xF).
    fn apply_op(&self, s_word: u16, d_word: u16) -> u16 {
        let mut result = 0u16;
        for bit in 0..16 {
            let s = (s_word >> bit) & 1;
            let d = (d_word >> bit) & 1;
            let index = 3 - ((s << 1) | d);
            let out = (self.op as u16 >> index) & 1;
            result |= out << bit;
        }
        result
    }

    /// Shifts the current source word by `skew` bits, combining it with the
    /// previous word.
    ///
    /// Formula verified against a concrete worked example from BLIT_FAQ.TXT
    /// (`ggnkua/Atari_ST_Sources` repo): for `SKEW=3` and increasing X
    /// traversal, the Blitter "reads out bits 18..3" of a 32-bit buffer where
    /// the CURRENT word occupies bits 0-15 (low) and the PREVIOUS word
    /// (copied by the Blitter into the high buffer after each write)
    /// occupies bits 16-31 (high) — i.e. `((previous as u32) << 16 |
    /// current as u32) >> skew`, truncated to 16 bits.
    ///
    /// Traversal direction (confirmed by Hatari, `Blitter_SourceShift`/
    /// `Blitter_SourceFetch`): this 32-bit buffer is a shift register
    /// fed DIFFERENTLY depending on the sign of `SRC_X_INC`. For increasing
    /// traversal (X_INC ≥ 0), the newly-read word goes into the LOW
    /// half and the old content moves up to HIGH (the "previous:high,
    /// current:low" order above). For DECREASING traversal (X_INC < 0,
    /// "mirror" blit), it's the OPPOSITE: the newly-read word goes into the
    /// HIGH half and the old (shifted) content ends up in LOW — i.e.
    /// "current:high, previous:low". The right shift by `skew` stays
    /// identical in both cases (same hardware register), but since
    /// the order of the halves is inverted, the result differs. This
    /// direction dependency was not modeled in a previous version
    /// (both halves were always in "increasing traversal" order). A first
    /// attempt at this fix coincided with the appearance of massive
    /// RGB noise in a live test — but the real cause turned out to be
    /// an unrelated Blitter-arming bug (accidental restarts via `TAS.B` in
    /// the non-HOG mode's resume loop, see [`Self::write_control`]), which
    /// re-executed entire blits from addresses already advanced — once that
    /// bug was fixed separately, this direction fix could be reapplied
    /// without regression.
    /// Shifts the 32-bit source shift register (`buffer`, see
    /// [`Self::execute`]): for increasing traversal (`src_x_inc >= 0`),
    /// the old content moves up to HIGH (clearing room at LOW for the
    /// next word read); for decreasing traversal, it's the opposite.
    /// Direct translation of `Blitter_SourceShift` (Hatari,
    /// `src/blitter.c`).
    fn shift_buffer(buffer: &mut u32, src_x_inc: i16) {
        if src_x_inc < 0 {
            *buffer >>= 16;
        } else {
            *buffer <<= 16;
        }
    }

    /// Loads `word` into the half of the source shift register that
    /// [`Self::shift_buffer`] just freed up. Direct translation of
    /// `Blitter_SourceFetch` (Hatari, `src/blitter.c`).
    fn fetch_buffer(buffer: &mut u32, src_x_inc: i16, word: u16) {
        if src_x_inc < 0 {
            *buffer |= (word as u32) << 16;
        } else {
            *buffer |= word as u32;
        }
    }

    /// Executes the blit in its entirety (synchronous model, see the
    /// module's limitations), using `bus` to read/write RAM at the
    /// current source/destination addresses. Updates the
    /// source/destination address registers at the end of execution; clears
    /// the BUSY bit.
    ///
    /// Processes the blit **word by word** (not line by line with a
    /// precomputed address-advance formula) — direct translation of Hatari's
    /// state machine (`Blitter_ProcessWord`,
    /// `Blitter_SourceShift`/`Blitter_SourceFetch`), in order to faithfully
    /// reproduce the 32-bit source shift register (`buffer`
    /// below): on real silicon, this register **persists for the
    /// ENTIRE duration of the blit** (all lines), never cleared
    /// between two lines — only a shift followed by a read
    /// modify it, on every word read (including the FXSR priming). A
    /// previous version, structured line by line with a batch address-advance
    /// formula, reset the "previous" word to 0 at the start of
    /// EVERY line (except FXSR) and handled NFSR as a local special
    /// case rather than the true suppression of the read/advance
    /// that real silicon implies — confirmed wrong by a differential
    /// test exhaustively comparing our output against a direct port
    /// of `Blitter_ProcessWord` (`tests/blitter_hatari_diff.rs`):
    /// for a multi-line blit in negative direction with SKEW=0 and
    /// X_COUNT=1, the old version produced 0 instead of the previous
    /// line's word on every new line; with NFSR active, it also
    /// diverged from the real source address advance (which entirely skips
    /// the end-of-line advance when the last read is omitted).
    ///
    /// **Arming** (see [`Self::write_control`]): does NOTHING if
    /// [`Self::armed`] is false — i.e. if the software hasn't
    /// explicitly rewritten Y_COUNT since the last complete execution.
    /// This is what prevents an accidental CONTROL trigger (e.g.
    /// `TAS.B` in the non-HOG mode's resume loop, which re-sets the BUSY bit
    /// physically each iteration) from re-executing the entire
    /// blit from addresses already advanced by a previous round — see
    /// [`Self::write_control`]'s detailed comment for the real bug
    /// that this fixes.
    pub fn execute(&mut self, bus: &mut impl crate::Bus) {
        if self.armed {
            // True start (Y_COUNT was just rewritten): (re)initializes
            // all persistent progress state. `x_count_reset` stays
            // clamped to 1 (`.max(1)`) only to avoid an infinite
            // loop below, not to give it any particular
            // meaning.
            self.x_count = self.x_count.max(1);
            self.x_count_reset = self.x_count;
            // Do NOT reset `self.buffer` to zero here: on real
            // silicon (confirmed by both Hatari AND Steem SSE — neither
            // reference ever resets `BlitterVars.buffer`/
            // `Blitter.SrcBuffer` when a blit starts, only via
            // normal shifts/reads), the source shift register
            // persists for the chip's whole lifetime, including
            // between two LOGICALLY SEPARATE blits (Y_COUNT rewritten between
            // the two). TOS typically draws an icon or glyph
            // column by column via a SEQUENCE of small adjacent blits
            // (X_COUNT=1, nonzero SKEW) that rely on this
            // chaining to correctly reconstruct pixels straddling
            // a word boundary — a systematic reset here then
            // splits every new column, consistent with the
            // RGB corruption observed on dragged icons and menu
            // text (only for blits that actually use the
            // source, never pure fills/inversions which don't
            // need it).
            self.bus_word = 0;
            self.have_fxsr = false;
            self.nfsr_dynamic = false;
            self.armed = false;
            self.mid_blit = true;
            self.control |= CONTROL_BUSY;
            if std::env::var("RUST68_TRACE_BLIT_START").is_ok() {
                eprintln!(
                    "[blit-start] pc={:#010x} dst_addr={:#010x} src_addr={:#010x} x_count={} y_count={} dst_x_inc={} dst_y_inc={} src_x_inc={} src_y_inc={} hop={} op={:#04x} skew={:#04x} control={:#04x} endmask={:#06x},{:#06x},{:#06x} buffer_before={:#010x}",
                    DEBUG_LAST_PC.load(std::sync::atomic::Ordering::Relaxed),
                    self.dst_addr, self.src_addr, self.x_count, self.y_count,
                    self.dst_x_inc, self.dst_y_inc, self.src_x_inc, self.src_y_inc,
                    self.hop, self.op, self.skew, self.control,
                    self.endmask[0], self.endmask[1], self.endmask[2], self.buffer,
                );
            }
        } else if !self.mid_blit {
            return;
        }

        let x_count_reset = self.x_count_reset;
        let smudge = self.control & 0x20 != 0;
        let fxsr_reg = self.skew & 0x80 != 0;
        let nfsr_reg = self.skew & 0x40 != 0;
        let skew = (self.skew & 0x0F) as u32;
        // HOG mode (CONTROL bit 6): the Blitter keeps the bus until the
        // blit is completely finished, never handing control back to the CPU. In
        // non-HOG mode, real silicon only processes a BOUNDED number of
        // REAL bus accesses (read OR write, each counted separately — not a
        // number of WORDS processed) before releasing the bus — the software must
        // re-set BUSY (typically via `TAS.B`, which incidentally re-reads/re-applies
        // the halftone line number already advanced by the
        // hardware) to move progress forward. Confirmed necessary
        // by a real Hatari trace (`--trace blitter`) on this exact case:
        // a long series of CONTROL writes with BUSY set and a line
        // number that progresses between each write (not a single
        // write that finishes everything at once), with hundreds of
        // authentic CPU instruction cycles between two writes —
        // i.e. REAL CPU work interleaved with the blit's
        // progress. A previous version executed the entire blit
        // instantly on the first CONTROL trigger, preventing
        // this interleaved CPU work from executing in the correct order relative
        // to the blit's actual progress — consistent with the corruption
        // observed in GEM menu rendering (TOS 1.62/STE) that persisted
        // despite an exhaustive, otherwise correct, verification of
        // the Blitter's internal arithmetic (OP table, HOP, skew, endmask,
        // address advance — see `tests/blitter_hatari_diff.rs`).
        //
        // The 64-bus-access threshold (`BUS_ACCESSES_PER_SLICE` below) and
        // the fact that it counts real accesses rather than words come
        // directly from Hatari's source (`src/blitter.c`,
        // `BLITTER_NONHOG_BUS_BLITTER`, with a comment noting that real
        // silicon can occasionally stop at 63 instead of 64
        // — a bug case not reproduced here, like the RAM refresh
        // irregularity already documented elsewhere). A previous version
        // counted 16 WORDS processed per slice (not bus accesses)
        // — a "normal" word with both a source read AND a destination write costs
        // 3 real bus accesses (source read, destination read for the
        // OP combination, destination write), so the old 16-word
        // slice actually represented 32 to 48 bus accesses depending on the blit
        // (never 64): the frequency of handing control back to the CPU was
        // systematically too high, consistent with a blit that progresses
        // more slowly (in real CPU cycles) than on real silicon/Hatari.
        let hog = self.control & CONTROL_HOG != 0;
        const BUS_ACCESSES_PER_SLICE: u32 = 64;
        let mut bus_accesses_this_slice: u32 = 0;

        // `need_src` (taken from Hatari, `Blitter_Step`): the source
        // pointer advances ONLY if the operation actually reads the source —
        // i.e. if OP is not one of the 4 logical functions that
        // ignore the source (0x0/0x5/0xA/0xF: constant 0, "destination",
        // "NOT destination", constant 1) AND if HOP produces a value
        // dependent on the source (HOP=2/3, or HOP=1 only in
        // SMUDGE mode, which reads the source to pick the halftone).
        let lop_needs_src = !matches!(self.op, 0x00 | 0x05 | 0x0A | 0x0F);
        let hop_needs_src = (self.hop & 0x02) != 0 || (self.hop == 1 && smudge);
        let need_src = lop_needs_src && hop_needs_src;

        let trace_words = std::env::var("RUST68_TRACE_BLITTER_WORDS").is_ok();
        let trace_slices = std::env::var("RUST68_TRACE_BLITTER_SLICES").is_ok();

        if trace_slices {
            eprintln!(
                "[slice] entry hog={hog} y_count={} x_count={} mid_blit_already={}",
                self.y_count, self.x_count, self.mid_blit,
            );
        }
        while self.y_count > 0 {
            if !hog && bus_accesses_this_slice >= BUS_ACCESSES_PER_SLICE {
                if trace_slices {
                    eprintln!(
                        "[slice] pause budget exhausted, y_count_remaining={} x_count_remaining={}",
                        self.y_count, self.x_count,
                    );
                }
                // Slice exhausted: hand control back to the CPU without clearing
                // BUSY — the next CONTROL trigger (typically
                // `TAS.B` in the software resume loop) will resume
                // exactly where we stopped, via `mid_blit`.
                return;
            }

            let x_count = self.x_count;
            let first_word = x_count == x_count_reset;
            if first_word {
                self.nfsr_dynamic = false;
            }

            // Special case of a single-word line (`x_count_reset ==
            // 1`): per the official Blitter manual, ENDMASK_1 is
            // used alone (no combination with ENDMASK_3, which is
            // simply ignored) — "In the case of a one word line
            // ENDMASK 1 is used."
            let mask = if first_word || x_count_reset == 1 {
                self.endmask[0]
            } else if x_count == 1 {
                self.endmask[2]
            } else {
                self.endmask[1]
            };

            // FXSR (priming read, once at the start of a line): on
            // real silicon, this read happens at the CURRENT address,
            // THEN `src_addr` advances by SRC_X_INC even before the first
            // "normal" read of word 0 — word 0 is thus actually read
            // at `src_addr+SRC_X_INC`, not at `src_addr`.
            if fxsr_reg && !self.have_fxsr && need_src {
                Self::shift_buffer(&mut self.buffer, self.src_x_inc);
                let w = bus.read16(self.src_addr & crate::ADDR_MASK);
                bus_accesses_this_slice += 1;
                self.bus_word = w;
                Self::fetch_buffer(&mut self.buffer, self.src_x_inc, w);
                self.src_addr = self.src_addr.wrapping_add(self.src_x_inc as i32 as u32);
                self.have_fxsr = true;
            }

            // Normal source read — omitted if NFSR is active AND this
            // word is the one identified as "last" by the
            // dynamic mechanism below (`nfsr_dynamic`, set when X_COUNT==2
            // was true on the previous word).
            let mut fetch_src = false;
            if need_src && !self.nfsr_dynamic {
                Self::shift_buffer(&mut self.buffer, self.src_x_inc);
                let w = bus.read16(self.src_addr & crate::ADDR_MASK);
                bus_accesses_this_slice += 1;
                self.bus_word = w;
                Self::fetch_buffer(&mut self.buffer, self.src_x_inc, w);
                fetch_src = true;
            }

            // Special NFSR case: real silicon performs a
            // shift+reread (reusing the last bus word read) both BEFORE
            // AND after processing the LAST word of EVERY line — not
            // only single-word lines. Confirmed in the real
            // Hatari source (`Blitter_ProcessWord`, blitter.c): the
            // condition there is `BlitterVars.nfsr && BlitterRegs.x_count ==
            // 1`, where `BlitterRegs.x_count` is the CURRENT counter (not the
            // initial value `x_count_reset`) — so true at the end of
            // every line, regardless of its width.
            let weird_single_word_nfsr = nfsr_reg && x_count == 1;
            if weird_single_word_nfsr {
                Self::shift_buffer(&mut self.buffer, self.src_x_inc);
                Self::fetch_buffer(&mut self.buffer, self.src_x_inc, self.bus_word);
            }

            let source = (self.buffer >> skew) as u16;
            let halftone_line = self.control & 0x0F;
            let halftone_word = if smudge {
                self.halftone[(source & 0x0F) as usize]
            } else {
                self.halftone[halftone_line as usize]
            };

            let hop_result = self.apply_hop(source, halftone_word);
            let dest_current = bus.read16(self.dst_addr & crate::ADDR_MASK);
            bus_accesses_this_slice += 1;
            let mut result = self.apply_op(hop_result, dest_current);
            result = (result & mask) | (dest_current & !mask);

            if trace_words
                && ((source != 0x0000 && source != 0xFFFF)
                    || (self.hop == 1 && self.halftone[0] != self.halftone[1]))
            {
                eprintln!(
                    "[bw] src={:#08x} dst={:#08x} x_count={x_count} fxsr={fxsr_reg} nfsr={nfsr_reg} buffer={:#010x} skewed={source:#06x} halftone_line={halftone_line} halftone={halftone_word:#06x} hop={hop_result:#06x} dest_before={dest_current:#06x} mask={mask:#06x} written={result:#06x}",
                    self.src_addr & crate::ADDR_MASK,
                    self.dst_addr & crate::ADDR_MASK,
                    self.buffer,
                );
            }

            bus.write16(self.dst_addr & crate::ADDR_MASK, result);
            bus_accesses_this_slice += 1;

            if weird_single_word_nfsr {
                Self::shift_buffer(&mut self.buffer, self.src_x_inc);
                Self::fetch_buffer(&mut self.buffer, self.src_x_inc, self.bus_word);
            }

            // The word that was just processed is the one where X_COUNT==2:
            // it's the word that PRECEDES the last one on the line. If NFSR is
            // active, the source read for the NEXT word (the last one) must
            // be omitted — set here for the next loop iteration.
            if x_count == 2 && nfsr_reg {
                self.nfsr_dynamic = true;
            }

            // Source address advance: only if a read happened on this word.
            // On the LAST word of the line (or if the next read will be
            // omitted by NFSR), the advance uses SRC_Y_INC instead of
            // SRC_X_INC — the calling software therefore
            // configures SRC_Y_INC accordingly.
            if fetch_src {
                if x_count == 1 || self.nfsr_dynamic {
                    self.src_addr = self.src_addr.wrapping_add(self.src_y_inc as i32 as u32);
                } else {
                    self.src_addr = self.src_addr.wrapping_add(self.src_x_inc as i32 as u32);
                }
            }

            if x_count == 1 {
                // End of line: DST_Y_INC replaces DST_X_INC (not in
                // addition) — the calling software therefore precomputes Y_INC
                // already accounting for the (X_COUNT-1) X_INC steps already
                // traversed.
                self.have_fxsr = false;
                self.y_count -= 1;
                self.x_count = x_count_reset;
                self.dst_addr = self.dst_addr.wrapping_add(self.dst_y_inc as i32 as u32);
                let next_line = if self.dst_y_inc >= 0 {
                    (halftone_line + 1) & 0x0F
                } else {
                    halftone_line.wrapping_sub(1) & 0x0F
                };
                self.control = (self.control & 0xF0) | next_line;
            } else {
                self.x_count -= 1;
                self.dst_addr = self.dst_addr.wrapping_add(self.dst_x_inc as i32 as u32);
            }
        }

        // NOTE: do NOT reset `self.y_count`/`self.x_count` (VISIBLE
        // registers) to zero here — see [`Self::write_control`]'s
        // comment for the distinction between this register
        // (which must return to its documented initial value) and
        // `self.armed` (which does go false below and prevents any
        // restart until the software has explicitly rewritten
        // Y_COUNT).
        //
        // The HOG bit (bit 6) is also cleared at the end of the blit, not
        // just BUSY — confirmed by Steem SSE (`blitter.cpp`,
        // `Blitter_Start_Line`, comment "hog bit also reset
        // (BLTBENCH.TOS)", citing behavior observed on real silicon
        // via the BLTBENCH.TOS test tool). A previous version didn't
        // touch this bit, leaving HOG visible as `1` after a blit in
        // HOG mode even once finished — software that re-reads CONTROL
        // to decide its behavior for the NEXT blit (in a
        // long sequence of calls, like drawing a GEM menu) could
        // therefore take a different path than real hardware would.
        self.control &= !(CONTROL_BUSY | CONTROL_HOG);
        self.armed = false;
        self.mid_blit = false;
    }
}
