//! Yamaha YM2149 (PSG — Programmable Sound Generator), register-for-register
//! compatible with the General Instrument AY-3-8910.
//!
//! On the Atari ST: 3 square-wave channels (A/B/C), a shared noise
//! generator, an envelope generator, and two 8-bit I/O ports (port A
//! among other things drives floppy drive/side selection, the
//! Centronics strobe, and the joystick/mouse select lines — board-specific
//! wiring, not modeled here, cf. limitations).
//!
//! This module models the chip **alone**: registers, tone/noise/envelope
//! generators, per-channel digital output levels. It is up to the board
//! to map [`Ym2149::read`]/[`Ym2149::write`] into its `Bus` (select
//! register at `0xFF8800`, data register at `0xFF8802` on real ST) and
//! to convert [`Ym2149::channel_level`] into real audio samples according
//! to its own output pipeline.
//!
//! ## Non-linear mixing of the 3 channels (Hatari-style)
//! [`mix_channels_model`] combines 3 channel levels (0-31) into a
//! single sample, modeling the chip's real DAC as three adjustable
//! pull-down resistors in parallel across a fixed load resistor
//! (voltage divider) — NOT a simple sum of the 3 levels: real silicon
//! is not physically a linear adder (combining 2-3 channels at full
//! amplitude clearly saturates well below 3x a single channel).
//! Formula and constants taken as-is from Hatari (`sound.c`,
//! `YM2149_BuildModelVolumeTable`, model attributed to David
//! Savinkoff, analysis of real measurements by Paulo Simoes and
//! Benjamin Gerard) — see the function's doc for details.
//!
//! ## Known limitations (v1)
//! - No conversion to PCM samples beyond the 3-channel mixing above:
//!   no downstream analog filtering simulated (see `Microwire` instead
//!   for the LMC1992 bass/treble filter, downstream of the PSG+DMA
//!   Sound mix).
//! - Meaning of port A/B bits (drive selection, joystick, Centronics...)
//!   not interpreted: these are raw 8-bit registers, up to the board
//!   to give them meaning.
//! - Envelope shape table and noise generator polynomial: behavior
//!   publicly documented by the original manufacturer (General
//!   Instrument, AY-3-8910 datasheet, table of the 10 envelope shapes
//!   and 17-bit LFSR) — reproduced here independently, not borrowed
//!   from an existing emulator.

/// Index of the 16 registers (selected via [`Ym2149::write`] on
/// [`REG_SELECT`], data then read/written on [`REG_DATA`]).
pub mod reg {
    pub const TONE_A_FINE: u8 = 0;
    pub const TONE_A_COARSE: u8 = 1;
    pub const TONE_B_FINE: u8 = 2;
    pub const TONE_B_COARSE: u8 = 3;
    pub const TONE_C_FINE: u8 = 4;
    pub const TONE_C_COARSE: u8 = 5;
    pub const NOISE_PERIOD: u8 = 6;
    pub const MIXER: u8 = 7;
    pub const AMPLITUDE_A: u8 = 8;
    pub const AMPLITUDE_B: u8 = 9;
    pub const AMPLITUDE_C: u8 = 10;
    pub const ENVELOPE_FINE: u8 = 11;
    pub const ENVELOPE_COARSE: u8 = 12;
    pub const ENVELOPE_SHAPE: u8 = 13;
    pub const IO_PORT_A: u8 = 14;
    pub const IO_PORT_B: u8 = 15;
}

/// Number of significant bits of each register (the rest is masked
/// off on write — standard silicon behavior, which has no flip-flop
/// for unwired bits).
const REG_WIDTH_MASK: [u8; 16] = [
    0xFF, 0x0F, // tone A fine/coarse
    0xFF, 0x0F, // tone B
    0xFF, 0x0F, // tone C
    0x1F, // noise period (5 bits)
    0xFF, // mixer (all bits used)
    0x1F, 0x1F, 0x1F, // amplitudes A/B/C (bit4 = envelope mode, bits0-3 = level)
    0xFF, 0xFF, // envelope period fine/coarse
    0x0F, // envelope shape
    0xFF, 0xFF, // I/O port A/B
];

/// Register offset "select" (logical offset 0) and "data" (logical
/// offset 1) in the chip's address space — on real ST, select at
/// `0xFF8800`, data at `0xFF8802`.
pub mod bus_offset {
    pub const SELECT: u8 = 0;
    pub const DATA: u8 = 1;
}

#[derive(Debug, Clone, Default)]
struct ToneGenerator {
    counter: u16,
    output: bool,
}

impl ToneGenerator {
    /// Advances by one chip cycle (already divided down from the CPU
    /// cycle by the caller). A disabled tone (period 0, a real
    /// documented case: treated as 1) still toggles at the minimum
    /// rate rather than freezing.
    ///
    /// The silicon's internal counter advances at clock/8 (not at the
    /// chip frequency itself): toggling every `period` chip cycles
    /// would produce a sound 8x too high-pitched. Confirmed by
    /// cross-checking the AY-3-8910/YM2149 datasheet and the Hatari
    /// reference implementation (`sound.c`, `ToneA_count`/`ToneA_per`
    /// incremented once per tick at 250 kHz for a chip clocked at 2 MHz).
    fn tick(&mut self, period_reg: u16) {
        let period = period_reg.max(1) * 8;
        self.counter += 1;
        if self.counter >= period {
            self.counter = 0;
            self.output = !self.output;
        }
    }
}

#[derive(Debug, Clone, Default)]
struct NoiseGenerator {
    counter: u16,
    /// 17-bit LFSR (bit 0 = current output). Standard AY-3-8910
    /// polynomial: feedback on XOR of bits 0 and 3.
    lfsr: u32,
}

impl NoiseGenerator {
    fn new() -> Self {
        NoiseGenerator {
            counter: 0,
            lfsr: 1, // non-zero initial state (an LFSR stuck at 0 would never move)
        }
    }

    /// The silicon's noise counter advances at half the rate of the
    /// tone/envelope counter (clock/16 instead of clock/8) — verified
    /// in Hatari (`sound.c`, `YM2149_Freq_div_2`: `Noise_count` only
    /// advances one cycle out of two relative to `ToneX_count`).
    fn tick(&mut self, period_reg: u8) {
        let period = period_reg.max(1) as u16 * 16;
        self.counter += 1;
        if self.counter >= period {
            self.counter = 0;
            let feedback = (self.lfsr ^ (self.lfsr >> 3)) & 1;
            self.lfsr = (self.lfsr >> 1) | (feedback << 16);
        }
    }

    fn output(&self) -> bool {
        self.lfsr & 1 != 0
    }
}

/// Envelope shape (`ENVELOPE_SHAPE` register, 4 bits): standard
/// AY-3-8910 datasheet table (Continue/Attack/Alternate/Hold),
/// reconstructed here from the manufacturer's public description
/// (not from emulator code).
#[derive(Debug, Clone, Copy)]
struct EnvelopeShape {
    continue_: bool,
    attack: bool,
    alternate: bool,
    hold: bool,
}

impl EnvelopeShape {
    fn from_bits(bits: u8) -> Self {
        EnvelopeShape {
            hold: bits & 0x1 != 0,
            alternate: bits & 0x2 != 0,
            attack: bits & 0x4 != 0,
            continue_: bits & 0x8 != 0,
        }
    }
}

#[derive(Debug, Clone)]
struct EnvelopeGenerator {
    counter: u32,
    /// Position in the 0..31 ramp (5 bits of resolution, twice that
    /// of the channels — standard documented chip behavior).
    step: u8,
    /// Direction of the current ramp (true = rising).
    rising: bool,
    /// True once a non-Continue cycle has reached its end and the
    /// level must stay frozen.
    finished: bool,
    shape: EnvelopeShape,
}

impl EnvelopeGenerator {
    fn new() -> Self {
        EnvelopeGenerator {
            counter: 0,
            step: 0,
            rising: false,
            finished: false,
            shape: EnvelopeShape::from_bits(0),
        }
    }

    /// Writing the shape register always restarts the envelope from
    /// the beginning (documented real silicon behavior).
    fn restart(&mut self, shape_bits: u8) {
        self.shape = EnvelopeShape::from_bits(shape_bits);
        self.counter = 0;
        self.step = 0;
        self.rising = self.shape.attack;
        self.finished = false;
    }

    /// The envelope counter advances at the same rate as the tone
    /// counter (clock/8) — verified in Hatari (`sound.c`,
    /// `Env_count`/`Env_per` incremented in the same 250 kHz loop as
    /// `ToneX_count`). The doubled resolution (32 steps instead of 16)
    /// comes solely from the ramp's step count, not from a different
    /// clock divider.
    fn tick(&mut self, period_reg: u16) {
        if self.finished {
            return;
        }
        let period = period_reg.max(1) as u32 * 8;
        self.counter += 1;
        if self.counter < period {
            return;
        }
        self.counter = 0;
        self.step += 1;
        if self.step > 31 {
            self.step = 0;
            if !self.shape.continue_ {
                // Single ramp: freezes at the final level (datasheet
                // table — Hold only has an effect if Continue=1).
                self.finished = true;
                self.step = if self.rising { 31 } else { 0 };
                return;
            }
            if self.shape.alternate {
                self.rising = !self.rising;
            }
            if self.shape.hold {
                self.finished = true;
                self.step = if self.rising { 31 } else { 0 };
            }
        }
    }

    /// Current level 0-31.
    fn level(&self) -> u8 {
        if self.rising {
            self.step
        } else {
            31 - self.step
        }
    }
}

/// Full state of a YM2149 chip.
#[derive(Debug, Clone)]
pub struct Ym2149 {
    selected: u8,
    regs: [u8; 16],
    /// Fractional CPU cycle accumulator: the chip is clocked at CPU/4
    /// (2 MHz for an ST/STE CPU at 8 MHz).
    cpu_cycle_acc: u32,
    tone: [ToneGenerator; 3],
    noise: NoiseGenerator,
    envelope: EnvelopeGenerator,
    /// Input levels of ports A/B for bits configured as inputs (DDR
    /// via `MIXER` bits 6-7) — injected by the caller, cf.
    /// `set_port_a_input`/`set_port_b_input`.
    port_a_in: u8,
    port_b_in: u8,
    /// Sum of each channel's output level, accumulated on EVERY chip
    /// cycle (not just sampled occasionally) since the last
    /// [`Self::take_averaged_levels`] — see its doc: needed to avoid
    /// aliasing when converting to an audio sample rate (44.1 kHz)
    /// much slower than the chip clock (2 MHz, up to ~45 possible
    /// toggles between two output samples for a high-pitched tone).
    level_accum: [u32; 3],
    level_accum_count: u32,
}

impl Default for Ym2149 {
    fn default() -> Self {
        Self::new()
    }
}

impl Ym2149 {
    pub fn new() -> Self {
        let mut regs = [0u8; 16];
        // Port A: floppy drive/side select lines (bits 0-2, see
        // `port_a_output`) pulled up by real pull-up resistors as long
        // as TOS has not programmed the port — "no drive selected +
        // side 0" (0xFF), confirmed by Hatari (`psg.c`: explicit
        // comment about the post-reset state). Without this, before
        // TOS programs this port very early at boot, the side read by
        // default would be side 1 (bit0=0) instead of 0.
        regs[reg::IO_PORT_A as usize] = 0xFF;
        Ym2149 {
            selected: 0,
            regs,
            cpu_cycle_acc: 0,
            tone: Default::default(),
            noise: NoiseGenerator::new(),
            envelope: EnvelopeGenerator::new(),
            port_a_in: 0,
            port_b_in: 0,
            level_accum: [0; 3],
            level_accum_count: 0,
        }
    }

    /// Reads the chip bus at logical offset `offset` (see
    /// [`bus_offset`]): `SELECT` returns the currently selected
    /// register number, `DATA` returns its content (ports A/B combine
    /// the latched output value with the input level according to the
    /// direction programmed in `MIXER`).
    pub fn read(&mut self, offset: u8) -> u8 {
        match offset {
            bus_offset::SELECT => self.selected,
            bus_offset::DATA => match self.selected {
                reg::IO_PORT_A => self.read_port(reg::IO_PORT_A, 6, self.port_a_in),
                reg::IO_PORT_B => self.read_port(reg::IO_PORT_B, 7, self.port_b_in),
                r if r < 16 => self.regs[r as usize],
                _ => 0xFF,
            },
            _ => 0xFF,
        }
    }

    fn read_port(&self, reg_idx: u8, dir_bit: u8, input: u8) -> u8 {
        let is_output = self.regs[reg::MIXER as usize] & (1 << dir_bit) != 0;
        if is_output {
            self.regs[reg_idx as usize]
        } else {
            input
        }
    }

    /// Writes the chip bus at logical offset `offset` (see
    /// [`bus_offset`]).
    pub fn write(&mut self, offset: u8, value: u8) {
        match offset {
            bus_offset::SELECT => self.selected = value & 0x0F,
            bus_offset::DATA => {
                let idx = self.selected as usize;
                if idx >= 16 {
                    return;
                }
                let masked = value & REG_WIDTH_MASK[idx];
                self.regs[idx] = masked;
                if idx == reg::ENVELOPE_SHAPE as usize {
                    self.envelope.restart(masked);
                }
            }
            _ => {}
        }
    }

    /// Applies a level to port A for bits configured as inputs (DDR =
    /// 0 on the corresponding bits of `MIXER`).
    pub fn set_port_a_input(&mut self, value: u8) {
        self.port_a_in = value;
    }

    /// RAW output register of port A (internal latch, not what a bus
    /// read-back would see via [`Self::read`] — that depends on the
    /// DDR direction programmed in `MIXER`). On real ST/STE, this
    /// register directly drives (regardless of the direction read by
    /// the CPU) the floppy connector's drive/side select lines: bit0
    /// (inverted) = side (0 after negation = side 1, 1 = side 0),
    /// bit1 = 0 → drive A selected, bit2 = 0 → drive B — confirmed by
    /// Hatari (`psg.c`/`fdc.c`, `FDC_SetDriveSide`). Up to the board to
    /// decode these bits (see `AtariSt::tick`/`FDC_DATA` write).
    pub fn port_a_output(&self) -> u8 {
        self.regs[reg::IO_PORT_A as usize]
    }

    /// Applies a level to port B for bits configured as inputs.
    pub fn set_port_b_input(&mut self, value: u8) {
        self.port_b_in = value;
    }

    fn tone_period(&self, fine: u8, coarse: u8) -> u16 {
        ((self.regs[coarse as usize] as u16) << 8) | self.regs[fine as usize] as u16
    }

    /// Advances the generators by `cpu_cycles` CPU cycles (ST/STE
    /// clock at 8 MHz; the chip itself runs at CPU/4).
    pub fn tick(&mut self, cpu_cycles: u32) {
        self.cpu_cycle_acc += cpu_cycles;
        let chip_cycles = self.cpu_cycle_acc / 4;
        self.cpu_cycle_acc %= 4;

        let tone_a = self.tone_period(reg::TONE_A_FINE, reg::TONE_A_COARSE);
        let tone_b = self.tone_period(reg::TONE_B_FINE, reg::TONE_B_COARSE);
        let tone_c = self.tone_period(reg::TONE_C_FINE, reg::TONE_C_COARSE);
        let noise_period = self.regs[reg::NOISE_PERIOD as usize];
        let envelope_period = self.tone_period(reg::ENVELOPE_FINE, reg::ENVELOPE_COARSE);

        for _ in 0..chip_cycles {
            self.tone[0].tick(tone_a);
            self.tone[1].tick(tone_b);
            self.tone[2].tick(tone_c);
            self.noise.tick(noise_period);
            self.envelope.tick(envelope_period);
            // Accumulates the level for THIS precise chip cycle (not
            // just the final state after the loop) — see the doc of
            // `level_accum`/`take_averaged_levels`.
            for ch in 0..3 {
                self.level_accum[ch] += self.channel_level(ch) as u32;
            }
            self.level_accum_count += 1;
        }
    }

    /// Time-average of each channel's level since the last call (or
    /// since creation/reset if never called), then resets the
    /// accumulator to zero for the next period — to be called once per
    /// produced audio output sample (no more often), otherwise the
    /// average would only cover a fraction of the real period.
    ///
    /// Unlike [`Self::channel_level`] (a point-in-time sample of the
    /// instantaneous state), this integrates ALL tone/noise toggles
    /// that occurred between two output samples — without this, a
    /// tone whose chip-level period is shorter than the audio sampling
    /// period (44.1 kHz vs 2 MHz: up to ~45 possible toggles per
    /// output sample) produces aliasing that sounds like grainy
    /// static instead of a clean tone — confirmed by an actual user
    /// recording showing a staircase waveform instead of a clean
    /// square signal.
    pub fn take_averaged_levels(&mut self) -> [f32; 3] {
        let count = self.level_accum_count;
        let levels = if count == 0 {
            // No chip cycles elapsed since the last call (audio output
            // sampled faster than the chip clock, an edge case): falls
            // back to the instantaneous state rather than a division
            // by zero.
            [self.channel_level(0) as f32, self.channel_level(1) as f32, self.channel_level(2) as f32]
        } else {
            let mut out = [0.0f32; 3];
            for ch in 0..3 {
                out[ch] = self.level_accum[ch] as f32 / count as f32;
            }
            out
        };
        self.level_accum = [0; 3];
        self.level_accum_count = 0;
        levels
    }

    /// Digital output level 0-31 of channel `channel` (0=A, 1=B, 2=C)
    /// at the current instant: combines tone/noise gating (`MIXER`)
    /// and the fixed amplitude or envelope level. A real
    /// AY-3-8910/YM2149 chip produces a 4-bit level (0-15) in fixed
    /// mode but 5-bit (0-31) in envelope mode; here we directly return
    /// the final level already scaled to 0-31 (fixed x2) for a single
    /// consistent output scale between the two modes.
    pub fn channel_level(&self, channel: usize) -> u8 {
        let (tone_bit, noise_bit, amplitude_reg) = match channel {
            0 => (0, 3, reg::AMPLITUDE_A),
            1 => (1, 4, reg::AMPLITUDE_B),
            2 => (2, 5, reg::AMPLITUDE_C),
            _ => panic!("invalid YM2149 channel: {channel}"),
        };
        let mixer = self.regs[reg::MIXER as usize];
        let tone_enabled = mixer & (1 << tone_bit) == 0;
        let noise_enabled = mixer & (1 << noise_bit) == 0;
        let tone_active = !tone_enabled || self.tone[channel].output;
        let noise_active = !noise_enabled || self.noise.output();
        if !(tone_active && noise_active) {
            return 0;
        }
        let amplitude = self.regs[amplitude_reg as usize];
        if amplitude & 0x10 != 0 {
            self.envelope.level()
        } else {
            VOLUME_4_TO_5[(amplitude & 0x0F) as usize]
        }
    }
}

/// Conversion of fixed 4-bit volume (amplitude register) to 5-bit
/// scale (0-31, same scale as the envelope) — as MEASURED on real
/// silicon (Hatari, `sound.c`, `YmVolume4to5`), NOT a simple x2:
/// `volume5 = volume4*2+1`, except 0 and 1 which stay 0 and 1 (so that
/// 0 stays exactly 0 and 15 correctly becomes 31, at both ends). Used
/// by [`Ym2149::channel_level`]; the envelope, meanwhile, already
/// natively spans 0-31 in steps of 1 (see [`Envelope::level`]) and
/// does not need this conversion.
const VOLUME_4_TO_5: [u8; 16] = [0, 1, 5, 7, 9, 11, 13, 15, 17, 19, 21, 23, 25, 27, 29, 31];

/// Table of the 32 per-channel "conductances" of the non-linear
/// 3-channel mixing model, Hatari-style
/// (`YM2149_BuildModelVolumeTable`, `sound.c`) — built once (recursive
/// computation starting from level 31, see
/// [`build_conductance_table`]).
fn conductance_table() -> &'static [f64; 32] {
    static TABLE: std::sync::OnceLock<[f64; 32]> = std::sync::OnceLock::new();
    TABLE.get_or_init(build_conductance_table)
}

/// Builds the conductance table — see [`conductance_table`]. Physical
/// model of the real DAC: each channel is seen as an adjustable
/// pull-down resistor (0-31), level 31 corresponding to the highest
/// conductance (lowest resistance). `FOURTH2` (fourth root of two)
/// and `WARP` are taken as-is from Hatari, where `WARP` is documented
/// as "measured at 1.65932 from 46602" (an empirical result, not
/// analytically derived).
fn build_conductance_table() -> [f64; 32] {
    const FOURTH2: f64 = 1.19;
    const WARP: f64 = 1.666666666666666667;
    let mut conductance = 2.0 / 3.0 / (1.0 - 1.0 / WARP) - 2.0 / 3.0; // = 1.0
    let mut table = [0.0f64; 32];
    for i in (1..=31).rev() {
        table[i] = conductance / 2.0;
        conductance = 1.0 / (1.0 - 1.0 / FOURTH2 / (1.0 / conductance + 1.0)) - 1.0;
    }
    table[0] = 1.0e-8; // avoids a division by zero (total silence)
    table
}

/// Linearly interpolated conductance for a FRACTIONAL level
/// (0.0-31.0) — [`Ym2149::take_averaged_levels`] returns a time
/// average (anti-aliasing), not an integer 0-31 level as an
/// instantaneous Hatari-style sample would; interpolating between the
/// 2 nearest table entries is a reasonable adaptation of this
/// discrete model to a continuous input (the gap between two adjacent
/// levels of the real DAC is small anyway, ~1.19x in amplitude).
fn conductance_at(table: &[f64; 32], level: f32) -> f64 {
    let level = level.clamp(0.0, 31.0);
    let lo = level.floor() as usize;
    let hi = (lo + 1).min(31);
    let frac = (level - lo as f32) as f64;
    table[lo] * (1.0 - frac) + table[hi] * frac
}

/// Combines 3 channel levels (0-31, see [`Ym2149::channel_level`]/
/// [`Ym2149::take_averaged_levels`]) into a single NON-LINEAR output
/// sample, Hatari-style (`YM2149_BuildModelVolumeTable`,
/// `YM_MODEL_MIXING`) — see the module doc. Returns a value in
/// `0.0..=65535.0` (0 = silence, 65535 = all 3 channels at maximum
/// simultaneously); to be centered/scaled by the caller according to
/// its own output pipeline (see `atari_st_sdl2.rs::mix_sample`).
///
/// Formula taken as-is from Hatari (`sound.c`, comment attributed to
/// David Savinkoff): `(MaxVol*WARP) / (1.0 +
/// 1.0/(conductance_i+conductance_j+conductance_k))` — a voltage
/// divider between the fixed load resistor (normalized to 1.0) and
/// the 3 adjustable pull-down resistors in parallel.
pub fn mix_channels_model(levels: [f32; 3]) -> f32 {
    const MAX_VOL: f64 = 65535.0;
    const WARP: f64 = 1.666666666666666667;
    let table = conductance_table();
    let sum =
        conductance_at(table, levels[0]) + conductance_at(table, levels[1]) + conductance_at(table, levels[2]);
    ((MAX_VOL * WARP) / (1.0 + 1.0 / sum)) as f32
}
