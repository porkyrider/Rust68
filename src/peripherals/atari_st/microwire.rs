//! Microwire — serial interface to the external LMC1992 mixer (STE), which
//! downstream controls master volume, left/right volume, bass/treble
//! balance, and the mixing mode, on the **final output signal** (PSG
//! *and* DMA sound mixed together) — not a register of the DMA Sound chip
//! itself, but a separate circuit driven by the same two serial registers
//! (`$FF8922` DATA, `$FF8924` MASK).
//!
//! Master volume, left/right volume AND the bass/treble filter are
//! modeled ([`Self::filter_left`]/[`Self::filter_right`]); the mixing mode
//! has no audible effect with a single host output source — it alone
//! remains a refinement not requested at this stage.
//!
//! Command word decoding: reproduces the LMC1992 algorithm as verified in
//! Hatari (`dmaSnd.c`, `DmaSnd_InterruptHandler_Microwire`) — MSB-first
//! serial transmission, a 2-bit `10` address prefix followed by a 3-bit
//! command selector then a value, the whole thing picked out by keeping
//! only the DATA bits whose corresponding MASK bit is 1. We skip the real
//! serial timing (16 shifts at 1 MHz) and decode instantly as soon as
//! MASK and DATA have each been written in full: no functional
//! consequence, since the software loops rereading DATA until it drops
//! back to zero anyway (already immediately the case, see the dedicated
//! comment in `AtariSt::read8`) before continuing.
//!
//! ## Bass/treble filter (Hatari-style)
//! [`Self::filter_left`]/[`Self::filter_right`] apply gain AND the
//! bass/treble filter in a single pass per sample — faithfully reproduced
//! from Hatari (`dmaSnd.c`, `DmaSnd_Bass_Shelf`/`DmaSnd_Treble_Shelf`/
//! `DmaSnd_Set_Tone_Level`/`DmaSnd_IIRfilterL`/`DmaSnd_IIRfilterR`): two
//! first-order shelf filters — one for bass (corner at 118.2763 Hz), one
//! for treble (corner at 8438.756 Hz), values measured on the real
//! LMC1992 chip — algebraically combined into a single biquad (2nd order)
//! filter, applied in direct form II. 13 gain steps (-12dB to +12dB in
//! 2dB steps) precomputed once for Rust68's FIXED output rate (44100 Hz,
//! see `AUDIO_SAMPLE_RATE` in `atari_st_sdl2.rs`) — unlike Hatari, which
//! recomputes these tables if the host output rate changes at runtime,
//! Rust68 only ever has one possible rate.
//!
//! The gain (volume) is an EXTERNAL parameter of
//! `filter_left`/`filter_right` (not read internally from
//! [`Self::left_gain`]/[`Self::right_gain`]): the caller remains free to
//! smooth volume changes over time (avoiding a "zipper noise" click on an
//! abrupt change) before feeding it into the filter — exactly what
//! `atari_st_sdl2.rs` does. This filter applies downstream of the
//! PSG+DMA Sound mix (as in Hatari, `DmaSnd_Apply_LMC`, called after
//! mixing and resampling to the host rate), not per individual source.

/// `(int)(powf(10.0, dB/20.0) * 65536.0 + 0.5)`, 2dB steps — master volume
/// table (6 bits), taken as-is from Hatari (`dmaSnd.c`,
/// `LMC1992_Master_Volume_Table`): whatever the command, `65535`
/// represents unity gain (0dB, no attenuation).
const MASTER_VOLUME_TABLE: [u16; 64] = [
    7, 8, 10, 13, 16, 21, 26, 33, 41, 52, // -80dB
    66, 83, 104, 131, 165, 207, 261, 328, 414, 521, // -60dB
    655, 825, 1039, 1308, 1646, 2072, 2609, 3285, 4135, 5206, // -40dB
    6554, 8250, 10387, 13076, 16462, 20724, 26090, 32846, 41350, 52057, // -20dB
    65535, 65535, 65535, 65535, 65535, 65535, 65535, 65535, 65535, 65535, // 0dB
    65535, 65535, 65535, 65535, 65535, 65535, 65535, 65535, 65535, 65535, // 0dB
    65535, 65535, 65535, 65535, // 0dB
];

/// Left/right volume table (5 bits) — taken from Hatari
/// (`LMC1992_LeftRight_Volume_Table`).
const LEFT_RIGHT_VOLUME_TABLE: [u16; 32] = [
    655, 825, 1039, 1308, 1646, 2072, 2609, 3285, 4135, 5206, // -40dB
    6554, 8250, 10387, 13076, 16462, 20724, 26090, 32846, 41350, 52057, // -20dB
    65535, 65535, 65535, 65535, 65535, 65535, 65535, 65535, 65535, 65535, // 0dB
    65535, 65535, // 0dB
];

/// Number of precomputed bass/treble gain steps (-12dB to +12dB in 2dB
/// steps) — taken from Hatari (`dmaSnd.c`, `TONE_STEPS`).
const TONE_STEPS: usize = 13;

/// Maps the raw 4-bit value of a bass/treble command (0-15) onto index
/// 0-12 in the precomputed tables — taken as-is from Hatari
/// (`LMC1992_Bass_Treble_Table`, `dmaSnd.c`).
const BASS_TREBLE_INDEX: [u8; 16] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 12, 12, 12];

/// Bass corner frequency (Hz) — real LMC1992, measured value (see Hatari,
/// `dmaSnd.c`, `DmaSnd_Init_Bass_and_Treble_Tables`).
const BASS_CORNER_HZ: f32 = 118.2763;
/// Treble corner frequency (Hz) — likewise.
const TREBLE_CORNER_HZ: f32 = 8438.756;
/// Rust68's FIXED output rate (see `AUDIO_SAMPLE_RATE` in
/// `atari_st_sdl2.rs`) — the tables below are computed once for this
/// frequency (Hatari recomputes them if its output frequency changes,
/// which never happens here).
const OUTPUT_RATE_HZ: f32 = 44100.0;

/// Coefficients of a first-order "shelf" filter — see
/// [`bass_shelf`]/[`treble_shelf`].
#[derive(Debug, Clone, Copy, Default)]
struct FirstOrderShelf {
    a1: f32,
    b0: f32,
    b1: f32,
}

/// Coefficients of the BASS shelf filter for a linear gain `g` (not in
/// dB) at corner frequency `fc`, sampled at `fs` — taken as-is from Hatari
/// (`DmaSnd_Bass_Shelf`, `dmaSnd.c`).
fn bass_shelf(g: f32, fc: f32, fs: f32) -> FirstOrderShelf {
    let tan_fc = (std::f32::consts::PI * fc / fs).tan();
    let a1 = if g < 1.0 { (tan_fc - g) / (tan_fc + g) } else { (tan_fc - 1.0) / (tan_fc + 1.0) };
    let b0 = (1.0 + a1) * (g - 1.0) / 2.0 + 1.0;
    let b1 = (1.0 + a1) * (g - 1.0) / 2.0 + a1;
    FirstOrderShelf { a1, b0, b1 }
}

/// Equivalent of [`bass_shelf`] for TREBLE — taken as-is from Hatari
/// (`DmaSnd_Treble_Shelf`, `dmaSnd.c`).
fn treble_shelf(g: f32, fc: f32, fs: f32) -> FirstOrderShelf {
    let tan_fc = (std::f32::consts::PI * fc / fs).tan();
    let a1 = if g < 1.0 { (g * tan_fc - 1.0) / (g * tan_fc + 1.0) } else { (tan_fc - 1.0) / (tan_fc + 1.0) };
    let b0 = 1.0 + (1.0 - a1) * (g - 1.0) / 2.0;
    let b1 = a1 + (a1 - 1.0) * (g - 1.0) / 2.0;
    FirstOrderShelf { a1, b0, b1 }
}

/// Precomputes the 13 bass gain steps (+12dB to -12dB, in 2dB steps) —
/// taken from Hatari's bass loop
/// (`DmaSnd_Init_Bass_and_Treble_Tables`).
fn build_bass_table() -> [FirstOrderShelf; TONE_STEPS] {
    // Index 12 = +12dB (max boost), index 0 = -12dB (max cut) — direction
    // set by Hatari (C loop `for(n=TONE_STEPS; n--; ...)`, which fills
    // n=TONE_STEPS-1 FIRST with dB=+12 before any decrement), not
    // arbitrary: it's also the direction expected by `BASS_TREBLE_INDEX`
    // (increasing raw command value -> increasing index -> more bass), so
    // the order here must match exactly.
    let mut table = [FirstOrderShelf::default(); TONE_STEPS];
    let mut db = 12.0f32;
    for entry in table.iter_mut().rev() {
        let g = 10f32.powf(db / 20.0);
        *entry = bass_shelf(g, BASS_CORNER_HZ, OUTPUT_RATE_HZ);
        db -= 2.0;
    }
    table
}

/// Precomputes the 13 treble gain steps — taken from Hatari's treble
/// loop, including capping the corner frequency at 80% of Nyquist if
/// needed (no effect at 44100 Hz: 8438.756 Hz < 0.4×44100=17640 Hz — kept
/// to stay faithful to the complete formula).
fn build_treble_table() -> [FirstOrderShelf; TONE_STEPS] {
    let nyquist_80pct = 0.5 * 0.8 * OUTPUT_RATE_HZ;
    let (fc, db_step) = if TREBLE_CORNER_HZ > nyquist_80pct {
        (nyquist_80pct, 2.0 * nyquist_80pct / TREBLE_CORNER_HZ)
    } else {
        (TREBLE_CORNER_HZ, 2.0)
    };
    // Same direction as `build_bass_table` (index 12 = +12dB) — see its
    // comment.
    let mut table = [FirstOrderShelf::default(); TONE_STEPS];
    let mut db = db_step * (TONE_STEPS as f32 - 1.0) / 2.0;
    for entry in table.iter_mut().rev() {
        let g = 10f32.powf(db / 20.0);
        *entry = treble_shelf(g, fc, OUTPUT_RATE_HZ);
        db -= db_step;
    }
    table
}

/// Full state of the Microwire/LMC1992 circuit.
#[derive(Debug, Clone)]
pub struct Microwire {
    mask: u16,
    data: u16,
    master_volume: u16,
    left_volume: u16,
    right_volume: u16,
    /// Raw value (0-15) of the last bass command received — 6 = flat/0dB
    /// by default, like Hatari (`DmaSnd_Reset`): there's no dedicated
    /// reset signal for the Microwire on real silicon, but a neutral
    /// setting is a reasonable starting point before any command.
    bass: u8,
    /// See [`Self::bass`], for treble.
    treble: u8,
    bass_table: [FirstOrderShelf; TONE_STEPS],
    treble_table: [FirstOrderShelf; TONE_STEPS],
    /// Coefficients of the COMBINED biquad filter (bass+treble
    /// algebraically multiplied into a single second-order filter) —
    /// `[a1, a2, b0, b1, b2]`, see [`Self::set_tone_level`].
    coef: [f32; 5],
    /// State (2 last intermediate samples) of the IIR filter, one per
    /// channel — see [`Self::filter_left`]/[`Self::filter_right`].
    iir_left: [f32; 2],
    iir_right: [f32; 2],
}

impl Microwire {
    pub fn new() -> Self {
        // Full volume by default (real silicon: no reset signal on the
        // Microwire itself, but TOS systematically programs a reasonable
        // volume very early at boot; starting attenuated would leave the
        // sound completely silent until any command has been sent).
        let mut mw = Microwire {
            mask: 0,
            data: 0,
            master_volume: 65535,
            left_volume: 65535,
            right_volume: 65535,
            bass: 6,
            treble: 6,
            bass_table: build_bass_table(),
            treble_table: build_treble_table(),
            coef: [0.0; 5],
            iir_left: [0.0; 2],
            iir_right: [0.0; 2],
        };
        mw.set_tone_level();
        mw
    }

    pub fn write_mask_high(&mut self, value: u8) {
        self.mask = (self.mask & 0x00FF) | ((value as u16) << 8);
    }

    pub fn write_mask_low(&mut self, value: u8) {
        self.mask = (self.mask & 0xFF00) | value as u16;
    }

    pub fn write_data_high(&mut self, value: u8) {
        self.data = (self.data & 0x00FF) | ((value as u16) << 8);
    }

    /// DATA low byte: last of the 4 bytes written in the real sequence
    /// (MASK high/low then DATA high/low) — decodes the command here.
    pub fn write_data_low(&mut self, value: u8) {
        self.data = (self.data & 0xFF00) | value as u16;
        self.decode();
    }

    fn decode(&mut self) {
        let mut i: i32 = 15;
        while i >= 0 {
            if self.mask & (1 << i) == 0 {
                i -= 1;
                continue;
            }
            let mut cmd: u16 = 0;
            let mut cmd_len: u32 = 0;
            while i >= 0 && self.mask & (1 << i) != 0 {
                cmd <<= 1;
                cmd_len += 1;
                if self.data & (1 << i) != 0 {
                    cmd |= 1;
                }
                i -= 1;
            }
            if cmd_len >= 11 && (cmd >> (cmd_len - 2)) & 0x3 == 0x2 {
                self.apply(cmd);
                return;
            }
            // Invalid command (wrong address prefix or too short): keep
            // scanning the rest of the mask, like real silicon (see the
            // module doc).
        }
    }

    fn apply(&mut self, cmd: u16) {
        match (cmd >> 6) & 0x7 {
            1 => {
                self.bass = (cmd & 0xF) as u8;
                self.set_tone_level();
            }
            2 => {
                self.treble = (cmd & 0xF) as u8;
                self.set_tone_level();
            }
            3 => self.master_volume = MASTER_VOLUME_TABLE[(cmd & 0x3F) as usize],
            4 => self.right_volume = LEFT_RIGHT_VOLUME_TABLE[(cmd & 0x1F) as usize],
            5 => self.left_volume = LEFT_RIGHT_VOLUME_TABLE[(cmd & 0x1F) as usize],
            // Mixing (0): not modeled, see the module doc.
            _ => {}
        }
    }

    /// Recombines the precomputed bass/treble tables into a single biquad
    /// filter ([`Self::coef`]), based on the current settings —
    /// Hatari-style (`DmaSnd_Set_Tone_Level`): a simple algebraic
    /// expansion of the product of the two first-order transfer functions
    /// (no `tan`/`pow`, unlike the table construction itself) — can be
    /// called on every bass/treble command received without worrying
    /// about cost.
    fn set_tone_level(&mut self) {
        let treb = self.treble_table[BASS_TREBLE_INDEX[(self.treble & 0xF) as usize] as usize];
        let bass = self.bass_table[BASS_TREBLE_INDEX[(self.bass & 0xF) as usize] as usize];
        self.coef[0] = treb.a1 + bass.a1;
        self.coef[1] = treb.a1 * bass.a1;
        self.coef[2] = treb.b0 * bass.b0;
        self.coef[3] = treb.b0 * bass.b1 + treb.b1 * bass.b0;
        self.coef[4] = treb.b1 * bass.b1;
    }

    /// Applies gain + the bass/treble filter to the LEFT channel for one
    /// sample — see the module doc (`filter_left`/`filter_right`).
    pub fn filter_left(&mut self, xn: f32, gain: f32) -> f32 {
        Self::iir_step(&self.coef, &mut self.iir_left, xn, gain)
    }

    /// Equivalent of [`Self::filter_left`] for the RIGHT channel —
    /// independent state ([`Self::iir_right`]), see the module doc about
    /// the factory diagnostic cartridge's distinct stereo tones.
    pub fn filter_right(&mut self, xn: f32, gain: f32) -> f32 {
        Self::iir_step(&self.coef, &mut self.iir_right, xn, gain)
    }

    /// One direct-form-II biquad filter step — taken as-is from Hatari
    /// (`DmaSnd_IIRfilterL`/`DmaSnd_IIRfilterR`): the gain applies at the
    /// filter's INPUT (same pass), not downstream.
    fn iir_step(coef: &[f32; 5], state: &mut [f32; 2], xn: f32, gain: f32) -> f32 {
        let a = gain * xn - coef[0] * state[0] - coef[1] * state[1];
        let yn = coef[2] * a + coef[3] * state[0] + coef[4] * state[1];
        state[1] = state[0];
        state[0] = a;
        yn
    }

    /// Gain (0.0-1.0) of the left channel: left volume × master volume —
    /// applied separately from the right channel ([`Self::right_gain`])
    /// since the STE factory diagnostic cartridge (test "Stereo 1 kHz/500
    /// Hz tones") deliberately programs different, changing left/right
    /// volumes to make 2 tones audible at distinct levels. External
    /// diagnostic / smoothing target — see [`Self::filter_left`] for the
    /// actual application (gain supplied as a parameter, not read back
    /// here).
    pub fn left_gain(&self) -> f32 {
        self.left_volume as f32 / 65535.0 * (self.master_volume as f32 / 65535.0)
    }

    /// Gain (0.0-1.0) of the right channel — see [`Self::left_gain`].
    pub fn right_gain(&self) -> f32 {
        self.right_volume as f32 / 65535.0 * (self.master_volume as f32 / 65535.0)
    }
}

impl Default for Microwire {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn send_command(mw: &mut Microwire, cmd11: u16) {
        mw.write_mask_high(0x07);
        mw.write_mask_low(0xFF);
        mw.write_data_high(((cmd11 >> 8) & 0xFF) as u8);
        mw.write_data_low((cmd11 & 0xFF) as u8);
    }

    #[test]
    fn full_volume_by_default() {
        let mw = Microwire::new();
        assert!((mw.left_gain() - 1.0).abs() < 0.001);
        assert!((mw.right_gain() - 1.0).abs() < 0.001);
    }

    #[test]
    fn master_volume_command_attenuates_both_channels() {
        let mut mw = Microwire::new();
        // Type=3 (master volume) << 6, value=0 (most attenuated index,
        // -80dB); address prefix "10" already included via bit10=1/bit9=0.
        send_command(&mut mw, 0x400 | (3 << 6));
        assert!(mw.left_gain() < 0.01, "left_gain={} should be heavily attenuated", mw.left_gain());
        assert!(mw.right_gain() < 0.01, "right_gain={} should be heavily attenuated", mw.right_gain());
    }

    #[test]
    fn master_volume_command_full_scale_is_unity_gain() {
        let mut mw = Microwire::new();
        send_command(&mut mw, 0x400 | (3 << 6) | 0x3F);
        assert!((mw.left_gain() - 1.0).abs() < 0.001);
        assert!((mw.right_gain() - 1.0).abs() < 0.001);
    }

    #[test]
    fn left_right_volume_command_is_independent() {
        let mut mw = Microwire::new();
        // Right volume at minimum, left unchanged (full): the two
        // channels must remain DISTINCT, not averaged together — this is
        // precisely what the cartridge's "Stereo 1 kHz/500 Hz tones" test
        // verifies (2 tones at different volumes).
        send_command(&mut mw, 0x400 | (4 << 6));
        assert!((mw.left_gain() - 1.0).abs() < 0.001, "left unchanged");
        assert!(mw.right_gain() < 0.01, "right should be heavily attenuated");
    }

    #[test]
    fn command_with_wrong_prefix_is_ignored() {
        let mut mw = Microwire::new();
        // bit10=0: invalid prefix (should be "10"), the command must not
        // be applied.
        send_command(&mut mw, 3 << 6);
        assert!((mw.left_gain() - 1.0).abs() < 0.001, "invalid command must change nothing");
    }

    // --- Bass/treble filter (Hatari-style) ------------------------------

    #[test]
    fn default_setting_is_transparent() {
        // Bass=treble=6 (0dB/flat, default setting): each first-order
        // filter has a transfer function EXACTLY equal to 1 (numerator ==
        // denominator, `b0=1, b1=a1` — see `bass_shelf`/`treble_shelf` for
        // g=1.0), so the combined biquad too, NOT only in steady state:
        // the filter must behave as a pure gain from the very first
        // sample, including transiently.
        let mut mw = Microwire::new();
        for &xn in &[1000.0, -500.0, 0.0, 12345.0, -8888.0] {
            let yn = mw.filter_left(xn, 2.0);
            assert!((yn - xn * 2.0).abs() < 0.01, "expected {} (pure gain), got {yn}", xn * 2.0);
        }
    }

    #[test]
    fn max_bass_amplifies_a_dc_signal_in_steady_state() {
        // A BASS shelf essentially acts on DC (0 Hz): a constant signal,
        // in steady state, must converge to input × linear_gain(bass dB)
        // — nearly independent of the treble setting (HIGH-frequency
        // shelf, transparent at DC). +12dB ~ linear gain ×3.98.
        let mut mw = Microwire::new();
        mw.write_mask_high(0x07);
        mw.write_mask_low(0xFF);
        // Type=1 (bass) << 6, value=12 (max index, +12dB).
        mw.write_data_high((((0x400 | (1 << 6) | 12) >> 8) & 0xFF) as u8);
        mw.write_data_low(((0x400 | (1 << 6) | 12)) as u8);

        let mut yn = 0.0;
        for _ in 0..500 {
            yn = mw.filter_left(1000.0, 1.0);
        }
        assert!((yn - 3981.0).abs() < 50.0, "expected steady state ~3981 (+12dB), got {yn}");
    }

    #[test]
    fn min_bass_attenuates_a_dc_signal_in_steady_state() {
        let mut mw = Microwire::new();
        mw.write_mask_high(0x07);
        mw.write_mask_low(0xFF);
        // Type=1 (bass) << 6, value=0 (min index, -12dB).
        mw.write_data_high((((0x400 | (1 << 6)) >> 8) & 0xFF) as u8);
        mw.write_data_low((0x400 | (1 << 6)) as u8);

        let mut yn = 0.0;
        for _ in 0..500 {
            yn = mw.filter_left(1000.0, 1.0);
        }
        assert!((yn - 251.0).abs() < 20.0, "expected steady state ~251 (-12dB), got {yn}");
    }

    #[test]
    fn left_and_right_channels_have_independent_state() {
        // Since the filter is recursive (IIR), a channel fed with DC must
        // NEVER have its state influenced by the other channel, even when
        // that one is fed very different values.
        let mut mw = Microwire::new();
        mw.filter_left(10000.0, 1.0);
        mw.filter_left(10000.0, 1.0);
        let right_untouched = mw.filter_right(0.0, 1.0);
        assert!(right_untouched.abs() < 0.01, "right channel not affected by left channel activity");
    }
}
