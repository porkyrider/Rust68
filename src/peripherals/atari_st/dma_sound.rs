//! DMA Sound (STE) — plays signed 8-bit PCM samples from RAM, at one of 4
//! hardware sample rates (6258/12517/25033/50066 Hz), mono or stereo.
//!
//! Reproduces Hatari's functional behavior (`dmaSnd.c`), simplified: no 8-
//! byte FIFO — only frame counter advancement, end-of-frame loop/stop, and
//! frequency conversion to the host output rate are modeled. The low-pass
//! LMC1992 filter (bass/treble) IS modeled, but on the `Microwire` side
//! (see its module doc), downstream of the PSG+DMA Sound mix — not here,
//! which only supplies raw PCM samples.
//!
//! **8-byte FIFO, deliberately not modeled**: on real silicon, RAM is read
//! in 8-byte blocks once per HBL (not continuously at the sample-rate
//! pace) — Hatari models this faithfully (`DmaSnd_FIFO_Refill`/
//! `DmaSnd_FIFO_PullByte`, `dmaSnd.c`) because this HBL granularity has a
//! real audible effect on the rare pieces of software that rewrite the
//! sample buffer WHILE it's playing (Hatari names "Mental Hangover" and
//! "Power Up Plus" specifically): depending on whether RAM is re-read at
//! the HBL rate or at the exact moment each sample is consumed (what this
//! module does), the live write becomes audible with a different offset
//! (up to ~1 HBL). A real but narrow effect (no known case in this
//! project today); Steem SSE itself doesn't model a FIFO for STE DMA
//! Sound either (its own "FIFO" is that of the floppy/ACSI DMA
//! controller, a different peripheral) — to be reconsidered if a concrete
//! game/demo reveals an issue, not anticipated without a known case.
//!
//! ## Registers (addresses relative to `STE_DMA_SOUND_BASE` = `0xFF8900`)
//! See [`reg`]. The Microwire (`$FF8920`/`$FF8922`/`$FF8924`) is a
//! separate peripheral (LMC1992), handled elsewhere (see
//! `AtariSt::STE_MICROWIRE_DATA`) — not here.

/// Register offsets, relative to `STE_DMA_SOUND_BASE`.
pub mod reg {
    /// `$FF8900`/`$FF8901`: DMA control (word, only the low byte actually
    /// matters on real silicon — bit0=play in progress, bit1=loop).
    pub const CONTROL_LOW: u32 = 0x01;
    pub const FRAME_START_HIGH: u32 = 0x03;
    pub const FRAME_START_MID: u32 = 0x05;
    pub const FRAME_START_LOW: u32 = 0x07;
    pub const FRAME_COUNT_HIGH: u32 = 0x09;
    pub const FRAME_COUNT_MID: u32 = 0x0B;
    pub const FRAME_COUNT_LOW: u32 = 0x0D;
    pub const FRAME_END_HIGH: u32 = 0x0F;
    pub const FRAME_END_MID: u32 = 0x11;
    pub const FRAME_END_LOW: u32 = 0x13;
    /// `$FF8921`: sound mode (bit7 = mono if set, otherwise stereo;
    /// bits1-0 = frequency, see [`super::SAMPLE_RATES_HZ`]).
    pub const SOUND_MODE: u32 = 0x21;
}

const CTRL_PLAY: u8 = 0x01;
const CTRL_LOOP: u8 = 0x02;
const MODE_MONO: u8 = 0x80;

/// Hardware sample rates (Hz), indexed by bits 1-0 of [`reg::SOUND_MODE`]
/// — 8,010,613 Hz / 160, divided by 8/4/2/1. Confirmed by the Atari
/// community (see also Hatari's `DmaSndSampleRates`, `dmaSnd.c`).
const SAMPLE_RATES_HZ: [u32; 4] = [6258, 12517, 25033, 50066];

/// Full state of the DMA Sound (STE) controller.
#[derive(Debug, Clone)]
pub struct DmaSound {
    control: u8,
    sound_mode: u8,
    frame_start: u32,
    frame_end: u32,
    /// Current playback address — advances with each byte consumed,
    /// independently of the audio generation rate (see
    /// [`Self::next_sample`]).
    frame_counter: u32,
    /// Frequency conversion accumulator, 32.32 fixed-point format (like
    /// Hatari): `(dma_frequency << 32) / host_frequency` added for each
    /// output sample generated; its integer part indicates how many new
    /// bytes to consume before the next output sample.
    freq_acc: u64,
    /// Last byte read for each channel (held between two accumulator
    /// advances — this is what's actually mixed into the output).
    held_left: i8,
    held_right: i8,
    /// Forces an immediate read on the very next [`Self::next_sample`],
    /// without waiting for the frequency accumulator to cross a step —
    /// otherwise, when the DMA frequency is lower than the host output
    /// frequency (e.g. 6258 Hz to 44100 Hz), the very first output
    /// samples would stay at 0 (silence) instead of the real first byte,
    /// until the accumulator reaches its first step.
    just_started: bool,
    /// Number of XSINT edges that occurred since the last
    /// [`Self::take_xsint_pulses`] — a real hardware signal wired to the
    /// MFP's Timer A event-counting input (see the doc of
    /// [`Self::end_of_frame`]), used by some software (including the STE
    /// factory diagnostic cartridge's Audio test) to count frame loops
    /// rather than polling a register directly.
    xsint_pulses: u32,
}

impl DmaSound {
    pub fn new() -> Self {
        DmaSound {
            control: 0,
            sound_mode: 0,
            frame_start: 0,
            frame_end: 0,
            frame_counter: 0,
            freq_acc: 0,
            held_left: 0,
            held_right: 0,
            just_started: false,
            xsint_pulses: 0,
        }
    }

    /// Removes and returns the number of XSINT edges accumulated since
    /// the last call — to be called once per board `tick()` to relay each
    /// one to `Mfp::pulse_ta()` (see the doc of [`Self::end_of_frame`]).
    pub fn take_xsint_pulses(&mut self) -> u32 {
        std::mem::take(&mut self.xsint_pulses)
    }

    fn mono(&self) -> bool {
        self.sound_mode & MODE_MONO != 0
    }

    fn sample_rate_hz(&self) -> u32 {
        SAMPLE_RATES_HZ[(self.sound_mode & 0x3) as usize]
    }

    pub fn playing(&self) -> bool {
        self.control & CTRL_PLAY != 0
    }

    /// Reads the logical register `offset` (see [`reg`]).
    pub fn read(&self, offset: u32) -> u8 {
        match offset {
            reg::CONTROL_LOW => self.control,
            reg::FRAME_COUNT_HIGH => (self.frame_counter >> 16) as u8,
            reg::FRAME_COUNT_MID => (self.frame_counter >> 8) as u8,
            reg::FRAME_COUNT_LOW => self.frame_counter as u8,
            reg::SOUND_MODE => self.sound_mode,
            _ => 0,
        }
    }

    /// Writes the logical register `offset` (see [`reg`]).
    pub fn write(&mut self, offset: u32, value: u8) {
        match offset {
            reg::CONTROL_LOW => {
                let was_playing = self.playing();
                self.control = value & (CTRL_PLAY | CTRL_LOOP);
                if !was_playing && self.playing() {
                    self.start_new_frame();
                }
            }
            reg::FRAME_START_HIGH => self.frame_start = (self.frame_start & 0x00FFFF) | ((value as u32) << 16),
            reg::FRAME_START_MID => self.frame_start = (self.frame_start & 0xFF00FF) | ((value as u32) << 8),
            // Bit0 wired to ground: frame addresses always word-aligned,
            // like the Blitter's address registers (see its doc).
            reg::FRAME_START_LOW => self.frame_start = (self.frame_start & 0xFFFF00) | (value as u32 & 0xFE),
            reg::FRAME_END_HIGH => self.frame_end = (self.frame_end & 0x00FFFF) | ((value as u32) << 16),
            reg::FRAME_END_MID => self.frame_end = (self.frame_end & 0xFF00FF) | ((value as u32) << 8),
            reg::FRAME_END_LOW => self.frame_end = (self.frame_end & 0xFFFF00) | (value as u32 & 0xFE),
            reg::SOUND_MODE => self.sound_mode = value,
            _ => {}
        }
    }

    /// Starts a new frame: copies start/end into the playback counter. If
    /// start == end and looping is disabled, stops immediately (behavior
    /// verified on real silicon, see Hatari's `DmaSnd_StartNewFrame`).
    fn start_new_frame(&mut self) {
        self.frame_counter = self.frame_start;
        if self.frame_start == self.frame_end && self.control & CTRL_LOOP == 0 {
            self.control &= !CTRL_PLAY;
        } else {
            self.just_started = true;
        }
    }

    /// End of frame reached during playback: loops (new frame) or stops,
    /// depending on the repeat bit. Always counts as an XSINT edge (see
    /// [`Self::take_xsint_pulses`]), even when looping: on real silicon,
    /// XSINT toggles briefly at EVERY end of frame, whether playback then
    /// continues or stops.
    fn end_of_frame(&mut self) {
        self.xsint_pulses += 1;
        if self.control & CTRL_LOOP != 0 {
            self.start_new_frame();
        } else {
            self.control &= !CTRL_PLAY;
        }
    }

    /// Consumes a byte at `self.frame_counter` in `ram`, advances the
    /// counter, and handles end of frame. Returns 0 (silence) if playback
    /// is stopped or the address is outside installed RAM (silent, never
    /// a bus error — DMA access, see the doc of
    /// `AtariSt::in_floating_st_ram`).
    fn pull_byte(&mut self, ram: &[u8]) -> i8 {
        if !self.playing() {
            return 0;
        }
        let byte = ram.get(self.frame_counter as usize).copied().unwrap_or(0) as i8;
        self.frame_counter = self.frame_counter.wrapping_add(1);
        if self.frame_counter == self.frame_end {
            self.end_of_frame();
        }
        byte
    }

    /// Generates the next stereo sample (L, R), already at `host_rate_hz`
    /// (32.32 fixed-point frequency conversion from the current DMA
    /// frequency, see [`Self::freq_acc`]) — to be called once per audio
    /// output sample, regardless of playback state (silence if stopped).
    pub fn next_sample(&mut self, ram: &[u8], host_rate_hz: u32) -> (i8, i8) {
        if !self.playing() {
            self.held_left = 0;
            self.held_right = 0;
            return (0, 0);
        }
        let ratio = ((self.sample_rate_hz() as u64) << 32) / host_rate_hz as u64;
        self.freq_acc += ratio;
        let mut steps = (self.freq_acc >> 32) as u32;
        self.freq_acc &= 0xFFFF_FFFF;
        if self.just_started {
            self.just_started = false;
            steps = steps.max(1);
        }
        for _ in 0..steps {
            if !self.playing() {
                break;
            }
            if self.mono() {
                let m = self.pull_byte(ram);
                self.held_left = m;
                self.held_right = m;
            } else {
                self.held_left = self.pull_byte(ram);
                self.held_right = self.pull_byte(ram);
            }
        }
        (self.held_left, self.held_right)
    }
}

impl Default for DmaSound {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stops_immediately_when_start_equals_end_without_loop() {
        let mut dma = DmaSound::new();
        dma.write(reg::FRAME_START_HIGH, 0x00);
        dma.write(reg::FRAME_START_MID, 0x10);
        dma.write(reg::FRAME_START_LOW, 0x00);
        dma.write(reg::FRAME_END_HIGH, 0x00);
        dma.write(reg::FRAME_END_MID, 0x10);
        dma.write(reg::FRAME_END_LOW, 0x00);
        dma.write(reg::CONTROL_LOW, 0x01); // PLAY, no loop
        assert!(!dma.playing(), "start == end without loop: immediate stop");
    }

    #[test]
    fn reads_mono_samples_and_loops() {
        let ram = vec![0x10, 0x20, 0x30, 0x40, 0x00, 0x00, 0x00, 0x00];
        let mut dma = DmaSound::new();
        dma.write(reg::FRAME_START_LOW, 0x00);
        dma.write(reg::FRAME_END_LOW, 0x04); // 4 bytes: 0x10,0x20,0x30,0x40
        dma.write(reg::SOUND_MODE, 0x83); // mono, 50066 Hz (bits1-0=11)
        dma.write(reg::CONTROL_LOW, 0x03); // PLAY + LOOP

        // host_rate == dma_rate: one byte consumed per output sample.
        let (l0, r0) = dma.next_sample(&ram, 50066);
        assert_eq!((l0, r0), (0x10, 0x10));
        let (l1, _) = dma.next_sample(&ram, 50066);
        assert_eq!(l1, 0x20);
        let (l2, _) = dma.next_sample(&ram, 50066);
        assert_eq!(l2, 0x30);
        let (l3, _) = dma.next_sample(&ram, 50066);
        assert_eq!(l3, 0x40);
        // End of frame reached after the 4th byte -> loops, restarts at 0x10.
        let (l4, _) = dma.next_sample(&ram, 50066);
        assert_eq!(l4, 0x10);
        assert!(dma.playing(), "loop active: must never stop");
    }

    #[test]
    fn stops_at_end_of_frame_without_loop() {
        let ram = vec![0x11, 0x22];
        let mut dma = DmaSound::new();
        dma.write(reg::FRAME_START_LOW, 0x00);
        dma.write(reg::FRAME_END_LOW, 0x02);
        dma.write(reg::SOUND_MODE, 0x83); // mono, 50066 Hz
        dma.write(reg::CONTROL_LOW, 0x01); // PLAY, no loop

        assert_eq!(dma.next_sample(&ram, 50066).0, 0x11);
        assert_eq!(dma.next_sample(&ram, 50066).0, 0x22);
        assert!(!dma.playing(), "end of frame without loop: stops");
        assert_eq!(dma.next_sample(&ram, 50066), (0, 0), "silence once stopped");
    }
}
