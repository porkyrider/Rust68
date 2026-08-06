//! GLUE ("General Logic Unit") — Atari ST.
//!
//! Custom chip that generates, among other functions, the system's two
//! autovectored video interrupts: HBL (horizontal blank, end of each
//! scanline) on **IPL2**, and VBL (vertical blank, end of frame) on
//! **IPL4**. This is what paces the display — TOS uses VBL for its vbl
//! queue (scrolling, per-line palette changes via HBL, periodic keyboard
//! reads…).
//!
//! Like [`crate::peripherals::mfp`], this module models the timing signal
//! **only**: it's up to the board ([`crate::systems::atari_st::AtariSt`])
//! to wire [`Glue::hbl_pending`]/[`Glue::vbl_pending`] to `Bus::irq_level`
//! (IPL2/IPL4, priority below the MFP on IPL6) and
//! [`Glue::ack_hbl`]/[`Glue::ack_vbl`] to `Bus::irq_ack`.
//!
//! ## Known limitations (v1)
//! - HBL/VBL timing only: GLUE also actually handles part of memory/bus
//!   decoding in reality (role shared with the MMU), not modeled here.
//! - Timing constants (cycles/line, lines/frame): usual values quoted by
//!   the emulation community (Hatari/WinSTon), not verified against a
//!   formal hardware reference (no test suite equivalent to TomHarte
//!   exists for this component).
//! - Line numbering aligned with Hatari's: `current_line()` is an
//!   ABSOLUTE position within the frame (0..LINES_PER_FRAME), including a
//!   real top blanking period (`VideoMode::frame_start_line()`, 63 in
//!   PAL/34 in NTSC) before the nominal visible window. `display_line()`
//!   gives the index into the framebuffer (`None` during blanking/border).
//! - Vertical overscan (top/bottom): `write_sync`/`read_sync` model the
//!   `$FF820A` register — a 50/60Hz switch occurring in the right cycle
//!   window near the top or bottom of the visible window extends it for
//!   the current frame (`display_start`/`display_end`), Hatari-style
//!   vertical overscan. Simplification: once triggered for a frame, the
//!   extension is never cancelled by a later write that would revert it
//!   (Hatari handles a few finer cancellation cases, not modeled here).
//!   Horizontal overscan (left/right border, `$FF8260`) isn't handled by
//!   `Glue` but by `Shifter` (see its own module).

/// Video mode: determines the HBL/VBL pace. The ST/STE runs at 8 MHz CPU
/// regardless of mode; only the number of cycles per line/lines per frame
/// changes depending on the broadcast standard.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VideoMode {
    /// 50 Hz, 313 lines/frame, 512 CPU cycles/line (the most common in
    /// Europe — usual Hatari/WinSTon values).
    Pal50,
    /// 60 Hz, 263 lines/frame, 508 CPU cycles/line.
    Ntsc60,
}

impl VideoMode {
    fn cycles_per_line(self) -> u32 {
        match self {
            VideoMode::Pal50 => 512,
            VideoMode::Ntsc60 => 508,
        }
    }

    fn lines_per_frame(self) -> u32 {
        match self {
            VideoMode::Pal50 => 313,
            VideoMode::Ntsc60 => 263,
        }
    }

    /// Active-display-start (DE) cycle within the line — the "gating"
    /// threshold for `$FF8264`/`$FF8265` writes (see the doc of
    /// [`Glue::line_start_cycle`]): Hatari values (`video.h`,
    /// `LINE_START_CYCLE_50`/`_60`).
    fn line_start_cycle(self) -> u32 {
        match self {
            VideoMode::Pal50 => 56,
            VideoMode::Ntsc60 => 52,
        }
    }

    /// Active-display-end (DE) cycle within the line — the "gating"
    /// threshold for the `$FF820F` write (see the doc of
    /// [`Glue::line_end_cycle`]): Hatari values (`video.h`,
    /// `LINE_END_CYCLE_50`/`_60`).
    fn line_end_cycle(self) -> u32 {
        match self {
            VideoMode::Pal50 => 376,
            VideoMode::Ntsc60 => 372,
        }
    }

    /// Number of lines actually displayed (identical PAL/NTSC — only the
    /// duration of the vertical blanking that follows differs). See the
    /// doc of [`Glue::vbl_edge_count`] for why this boundary, not the full
    /// wraparound of [`Self::lines_per_frame`], is what triggers VBL on
    /// real silicon.
    fn visible_lines(self) -> u32 {
        200
    }

    /// First ABSOLUTE line (0..lines per frame) where active display
    /// normally starts — Hatari values (`video.h`,
    /// `VIDEO_START_HBL_50HZ`/`_60HZ`): a real TOP vertical blanking
    /// period of 63 lines (PAL) precedes the first visible line, not just
    /// the BOTTOM end-of-frame blanking already modeled. Needed for STE
    /// top border removal (see `peripherals::atari_st::shifter`): without
    /// this period "before" the normally visible line, there's simply no
    /// line to reveal. The corresponding bottom blanking is deduced from
    /// `lines_per_frame() - frame_start_line() - visible_lines()` (50 in
    /// PAL, 63-313+200... see [`Glue::display_line`]) — the SUM of the two
    /// blanking periods (113 in PAL) remains the one already modeled
    /// before this rework, only their top/bottom split changes.
    fn frame_start_line(self) -> u32 {
        match self {
            VideoMode::Pal50 => 63,
            VideoMode::Ntsc60 => 34,
        }
    }

    /// Extra lines revealed by a successful bottom border removal —
    /// Hatari values (`video.h`, `VIDEO_HEIGHT_BOTTOM_50HZ`/`_60HZ`). See
    /// [`Glue::write_sync`].
    fn bottom_border_extra_lines(self) -> u32 {
        match self {
            VideoMode::Pal50 => 47,
            VideoMode::Ntsc60 => 26,
        }
    }
}

/// Cycle limit, within the line, for a 50/60Hz switch (`$FF820A`) near the
/// top or bottom of the displayable window to trigger a border removal —
/// Hatari value (`video.h`, `LINE_REMOVE_TOP_CYCLE`/
/// `LINE_REMOVE_BOTTOM_CYCLE`, ~504 STF/500 STE; a single value kept here,
/// as the 4-cycle gap between silicon variants isn't verifiable without a
/// hardware reference, see the module limitations). Reused as-is for PAL
/// and NTSC for lack of a distinct NTSC value found during research.
const LINE_REMOVE_BORDER_CYCLE: u32 = 504;

/// State of the HBL/VBL timing generator.
#[derive(Debug, Clone)]
pub struct Glue {
    mode: VideoMode,
    cycles_in_line: u32,
    line: u32,
    frame: u64,
    hbl_pending: bool,
    vbl_pending: bool,
    vbl_edges: u64,
    /// [start, end) window of ABSOLUTE lines displayed for the CURRENT
    /// frame — defaults to
    /// `frame_start_line()..frame_start_line()+visible_lines()`, reset to
    /// this default on each new frame (see [`Self::tick`]). Adjustable by
    /// [`Self::write_sync`] (STE top/bottom border removal) for the
    /// current frame.
    display_start: u32,
    display_end: u32,
    /// Raw `$FF820A` register (bit 1 = external 50/60Hz select) — see
    /// [`Self::write_sync`].
    sync: u8,
}

impl Glue {
    pub fn new(mode: VideoMode) -> Self {
        let display_start = mode.frame_start_line();
        let display_end = display_start + mode.visible_lines();
        Glue {
            mode,
            cycles_in_line: 0,
            line: 0,
            frame: 0,
            hbl_pending: false,
            vbl_pending: false,
            vbl_edges: 0,
            display_start,
            display_end,
            sync: 0,
        }
    }

    /// Advances the generator by `cpu_cycles` CPU cycles, arming HBL at
    /// each end of line and VBL at the visible-line -> vertical-blanking
    /// transition (see the doc of [`Self::vbl_edge_count`]) — NOT at the
    /// full frame wraparound (`line` resetting to 0), which stays reserved
    /// for [`Self::frame_count`]. The displayable window
    /// (`display_start`/`_end`) is reset to its nominal value on each new
    /// frame — any top/bottom border removal only applies to the frame
    /// where it was triggered, never subsequent ones (real silicon
    /// behavior, confirmed by Hatari: recomputed on each VBL).
    pub fn tick(&mut self, cpu_cycles: u32) {
        self.cycles_in_line += cpu_cycles;
        let per_line = self.mode.cycles_per_line();
        while self.cycles_in_line >= per_line {
            self.cycles_in_line -= per_line;
            self.hbl_pending = true;
            self.line += 1;
            if self.line == self.display_end {
                self.vbl_pending = true;
                self.vbl_edges += 1;
            }
            if self.line >= self.mode.lines_per_frame() {
                self.line = 0;
                self.frame += 1;
                self.display_start = self.mode.frame_start_line();
                self.display_end = self.display_start + self.mode.visible_lines();
            }
        }
    }

    /// DISPLAYED line index (0..visible lines of the current frame) if
    /// the current absolute scanline ([`Self::current_line`]) is within
    /// the displayable window — `None` otherwise (top or bottom
    /// blanking). It's this index, not [`Self::current_line`] directly,
    /// that must be used to index `AtariSt::framebuffer`: the displayable
    /// window can start before the nominal position (top border removal)
    /// or end after it (bottom border), see
    /// `peripherals::atari_st::shifter`.
    pub fn display_line(&self) -> Option<u32> {
        self.display_index(self.line)
    }

    /// Like [`Self::display_line`], but for an arbitrary ABSOLUTE
    /// scanline rather than necessarily the current one — useful to
    /// `AtariSt::tick` for its occasional catch-up of several lines in a
    /// single call (see the doc of the `display_start`/`display_end`
    /// field): only exact if all the caught-up lines belong to the same
    /// frame as the one whose displayable window is currently stored here
    /// — the normal case in real usage (a `tick()` never covers more than
    /// a handful of CPU cycles, far less than a whole frame), not
    /// guaranteed for a test `tick()` deliberately covering several
    /// frames at once.
    pub fn display_index(&self, absolute_line: u32) -> Option<u32> {
        if absolute_line >= self.display_start && absolute_line < self.display_end {
            Some(absolute_line - self.display_start)
        } else {
            None
        }
    }

    /// True if an HBL is pending acknowledgment (IPL2).
    pub fn hbl_pending(&self) -> bool {
        self.hbl_pending
    }

    /// True if a VBL is pending acknowledgment (IPL4).
    pub fn vbl_pending(&self) -> bool {
        self.vbl_pending
    }

    /// Acknowledges the pending HBL (to be called from `Bus::irq_ack` for
    /// level 2).
    pub fn ack_hbl(&mut self) {
        self.hbl_pending = false;
    }

    /// Acknowledges the pending VBL (to be called from `Bus::irq_ack` for
    /// level 4).
    pub fn ack_vbl(&mut self) {
        self.vbl_pending = false;
    }

    /// Current scanline (0..lines per frame).
    pub fn current_line(&self) -> u32 {
        self.line
    }

    /// Number of complete frames elapsed since creation/last reset —
    /// useful for pacing an external video renderer.
    pub fn frame_count(&self) -> u64 {
        self.frame
    }

    /// Number of VBL edges that occurred since creation/last reset — at
    /// the visible-line -> vertical-blanking transition (see the doc of
    /// [`Self::tick`]), NOT at the full frame wraparound.
    ///
    /// Distinct from [`Self::frame_count`] (which only advances at the
    /// full wraparound): on real silicon, VBL occurs at the START of
    /// vertical blanking (right after the last visible line), with the
    /// rest of the blanking period (~113 lines in PAL) elapsing
    /// AFTERWARDS before line 0 of the next frame is displayed. The board
    /// ([`crate::systems::atari_st::AtariSt::tick`]) must detect this
    /// change (not that of `frame_count`) to reload the Shifter's video
    /// counter from its base — otherwise it would only restart at the
    /// wraparound to 0, in the SAME `tick()` call that would already
    /// render visible line 0 of the next frame, leaving the software no
    /// chance at all to take the VBL interrupt before that line has
    /// already been rendered — confirmed necessary by the STE factory
    /// diagnostic cartridge (test "T4 Video Counter in Memory Controller",
    /// which systematically failed by exactly one video line on its very
    /// first read after the base was reprogrammed).
    pub fn vbl_edge_count(&self) -> u64 {
        self.vbl_edges
    }

    /// Number of lines per frame in the current video mode — useful for
    /// detecting `current_line()` wraparound from one tick to the next.
    pub fn lines_per_frame(&self) -> u32 {
        self.mode.lines_per_frame()
    }

    /// Position in CPU cycles within the current scanline (0..cycles/line
    /// of the current mode) — needed for cycle-exact "gating" of STE
    /// Shifter register writes (`$FF8264`/`$FF8265`/`$FF820F`, see
    /// `peripherals::atari_st::shifter`): a write before the start of
    /// active display of the current line applies immediately, after it
    /// is deferred to the next line — exactly Hatari's `New*`/staging
    /// mechanism (`video.c`,
    /// `Video_HorScroll_Write`/`Video_LineWidth_WriteByte`).
    pub fn cycles_in_line(&self) -> u32 {
        self.cycles_in_line
    }

    /// Active-display-start (DE) cycle within the line, in the current
    /// video mode — see [`Self::cycles_in_line`].
    pub fn line_start_cycle(&self) -> u32 {
        self.mode.line_start_cycle()
    }

    /// Active-display-end (DE) cycle within the line, in the current
    /// video mode — see [`Self::cycles_in_line`].
    pub fn line_end_cycle(&self) -> u32 {
        self.mode.line_end_cycle()
    }

    /// Raw `$FF820A` register (bit 1 = external 50/60Hz select).
    pub fn read_sync(&self) -> u8 {
        self.sync
    }

    /// Writes `$FF820A` (bit 1 = external 50/60Hz select) — along the
    /// way, detects STE top/bottom border removal: a switch TO 60Hz
    /// occurring at a precise cycle near the top or bottom of the nominal
    /// displayable window extends it for THE CURRENT FRAME (see Hatari,
    /// `video.c`, `Video_Update_Glue_State`). Only a switch to 60Hz can
    /// enlarge the window (the machine's nominal mode remains
    /// [`Self::mode`], never changed by this write itself — only the
    /// window displayed for this particular frame is):
    /// - **Top border**: the switch occurs while still in the nominal top
    ///   blanking (`self.line < frame_start_line()`) — the window then
    ///   starts at the NOMINAL start position of 60Hz mode (34), revealing
    ///   the lines in between.
    /// - **Bottom border**: the switch occurs on the second-to-last or
    ///   last nominal displayed line — the window extends by
    ///   [`VideoMode::bottom_border_extra_lines`] extra lines.
    ///
    /// Simplification assumed relative to Hatari: no CANCELLATION
    /// mechanism if a later switch reverts the decision (once triggered
    /// for this frame, the extension stays in effect until the next
    /// frame) — captures the common case (a single well-placed switch per
    /// frame), not the most exotic multi-write sequences.
    pub fn write_sync(&mut self, value: u8) {
        self.sync = value;
        let selecting_60hz = value & 0x02 == 0;
        if !selecting_60hz || self.cycles_in_line > LINE_REMOVE_BORDER_CYCLE {
            return;
        }
        let nominal_start = self.mode.frame_start_line();
        if self.line < nominal_start {
            let alt_start = VideoMode::Ntsc60.frame_start_line();
            if alt_start < self.display_start {
                self.display_start = alt_start;
            }
            return;
        }
        let nominal_end = nominal_start + self.mode.visible_lines();
        if self.line + 1 >= nominal_end && self.line < nominal_end {
            let extended = nominal_end + self.mode.bottom_border_extra_lines();
            self.display_end = self.display_end.max(extended);
        }
    }
}
