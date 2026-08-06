#![cfg(feature = "atari-st")]
//! Unit tests for the YM2149 (`rust68::peripherals::atari_st::ym2149`).

use rust68::peripherals::atari_st::ym2149::{Ym2149, bus_offset, mix_channels_model, reg};

fn select(chip: &mut Ym2149, r: u8) {
    chip.write(bus_offset::SELECT, r);
}

fn write_reg(chip: &mut Ym2149, r: u8, value: u8) {
    select(chip, r);
    chip.write(bus_offset::DATA, value);
}

fn read_reg(chip: &mut Ym2149, r: u8) -> u8 {
    select(chip, r);
    chip.read(bus_offset::DATA)
}

#[test]
fn registers_masked_according_to_their_real_width() {
    let mut chip = Ym2149::new();
    write_reg(&mut chip, reg::TONE_A_COARSE, 0xFF);
    assert_eq!(read_reg(&mut chip, reg::TONE_A_COARSE), 0x0F, "coarse tone: 4 bits");

    write_reg(&mut chip, reg::NOISE_PERIOD, 0xFF);
    assert_eq!(read_reg(&mut chip, reg::NOISE_PERIOD), 0x1F, "noise period: 5 bits");

    write_reg(&mut chip, reg::ENVELOPE_SHAPE, 0xFF);
    assert_eq!(read_reg(&mut chip, reg::ENVELOPE_SHAPE), 0x0F, "envelope shape: 4 bits");

    write_reg(&mut chip, reg::TONE_A_FINE, 0xFF);
    assert_eq!(read_reg(&mut chip, reg::TONE_A_FINE), 0xFF, "fine tone: full 8 bits");
}

#[test]
fn register_selector_is_readable() {
    let mut chip = Ym2149::new();
    select(&mut chip, 5);
    assert_eq!(chip.read(bus_offset::SELECT), 5);
}

#[test]
fn tone_toggles_at_the_programmed_rate() {
    let mut chip = Ym2149::new();
    // Period = 1: toggles every chip cycle (= 4 CPU cycles).
    write_reg(&mut chip, reg::TONE_A_FINE, 1);
    write_reg(&mut chip, reg::TONE_A_COARSE, 0);
    // Mixer: tone A enabled (bit0=0), noise A disabled (bit3=1).
    write_reg(&mut chip, reg::MIXER, 0b0000_1000);
    write_reg(&mut chip, reg::AMPLITUDE_A, 0x0F); // fixed max volume

    let mut levels = Vec::new();
    for _ in 0..6 {
        // The silicon's internal counter runs at clock/8: with
        // period=1, the toggle occurs every 8 chip cycles.
        for _ in 0..8 {
            chip.tick(4); // 1 chip cycle
        }
        levels.push(chip.channel_level(0));
    }
    // Minimum period (1): toggles after 8 chip cycles, then
    // alternates 31 (VOLUME_4_TO_5[0x0F], not a simple x2 — see
    // `Ym2149::channel_level`) / 0 every 8 subsequent cycles.
    assert_eq!(levels, vec![31, 0, 31, 0, 31, 0]);
}

#[test]
fn take_averaged_levels_averages_the_intermediate_toggles() {
    // Anti-aliasing: `channel_level` is a point-in-time sample of the
    // instantaneous state; if many toggles have occurred since the last
    // call (a high-pitched tone ticked in one large block of CPU cycles,
    // as the SDL2 binary does once per instruction), `channel_level`
    // alone would only see the final state, not the actual signal
    // average — hence the existence of `take_averaged_levels`.
    let mut chip = Ym2149::new();
    write_reg(&mut chip, reg::TONE_A_FINE, 1); // toggles every 8 chip cycles
    write_reg(&mut chip, reg::TONE_A_COARSE, 0);
    write_reg(&mut chip, reg::MIXER, 0b0000_1000); // tone A active, noise A off
    write_reg(&mut chip, reg::AMPLITUDE_A, 0x0F); // level 31 when "high" (VOLUME_4_TO_5[0x0F])

    // One single large tick covering a WHOLE number of full periods (80
    // chip cycles = 320 CPU cycles = 5 x 16 cycles, period=1 toggling
    // every 8 chip cycles): the average must be exactly 50% of the
    // max level (31), not just the point-in-time final state after the loop.
    chip.tick(320);
    let levels = chip.take_averaged_levels();
    assert!((levels[0] - 15.5).abs() < 0.01, "expected average = 15.5 (50% of 31), got {}", levels[0]);

    // The accumulator is reset to zero: a call immediately after without
    // a new tick() must not reuse the previous average.
    let levels_after_reset = chip.take_averaged_levels();
    assert_eq!(levels_after_reset[0], chip.channel_level(0) as f32, "empty accumulator -> falls back to the instantaneous state");
}

#[test]
fn mixer_cuts_tone_or_noise_according_to_the_bits() {
    let mut chip = Ym2149::new();
    write_reg(&mut chip, reg::AMPLITUDE_A, 0x0F);
    // Both tone and noise disabled (bits 0 and 3 set to 1): the channel
    // must stay at full level (the internal "gate" is "open" = 1).
    write_reg(&mut chip, reg::MIXER, 0b0000_1001);
    chip.tick(100);
    assert_eq!(chip.channel_level(0), 31, "tone+noise cut: gate always open");
}

#[test]
fn noise_produces_a_non_trivial_sequence() {
    let mut chip = Ym2149::new();
    write_reg(&mut chip, reg::NOISE_PERIOD, 1);
    write_reg(&mut chip, reg::MIXER, 0b0000_0001); // tone A cut, noise A active
    write_reg(&mut chip, reg::AMPLITUDE_A, 0x0F);

    let mut seen_zero = false;
    let mut seen_nonzero = false;
    for _ in 0..64 {
        chip.tick(4);
        if chip.channel_level(0) == 0 {
            seen_zero = true;
        } else {
            seen_nonzero = true;
        }
    }
    assert!(seen_zero && seen_nonzero, "the LFSR must produce both levels");
}

/// Mixer with both tone AND noise disabled (gates always open) to
/// isolate channel A's amplitude/envelope level in tests.
const MIXER_GATES_OPEN_A: u8 = 0b0000_1001;

#[test]
fn envelope_mode_activated_via_amplitude_bit4() {
    let mut chip = Ym2149::new();
    write_reg(&mut chip, reg::MIXER, MIXER_GATES_OPEN_A);
    write_reg(&mut chip, reg::ENVELOPE_FINE, 1);
    write_reg(&mut chip, reg::ENVELOPE_COARSE, 0);
    // Shape "attack only, continue=1, alternate=0, hold=0" -> rising sawtooth
    write_reg(&mut chip, reg::ENVELOPE_SHAPE, 0b1100);
    write_reg(&mut chip, reg::AMPLITUDE_A, 0x10); // bit4 = envelope mode

    // Right after the reset (writing the shape register), the level must
    // be at the minimum of the rising ramp.
    assert_eq!(chip.channel_level(0), 0);
}

#[test]
fn envelope_attack_without_continue_freezes_at_31_after_one_ramp() {
    let mut chip = Ym2149::new();
    write_reg(&mut chip, reg::MIXER, MIXER_GATES_OPEN_A);
    write_reg(&mut chip, reg::ENVELOPE_FINE, 1);
    write_reg(&mut chip, reg::ENVELOPE_COARSE, 0);
    // continue=0, attack=1: a single rising ramp then freezes at 31 (max).
    write_reg(&mut chip, reg::ENVELOPE_SHAPE, 0b0100);
    write_reg(&mut chip, reg::AMPLITUDE_A, 0x10);

    // 32 steps to complete the ramp (period=1 -> 8 chip cycles per
    // step, the envelope counter running at clock/8 like the tone
    // counter, each chip cycle = 4 CPU cycles).
    for _ in 0..(32 * 8) {
        chip.tick(4);
    }
    // The envelope has its own 0-31 scale (double the resolution of the
    // fixed 0-15 amplitude), returned as-is by channel_level in envelope
    // mode (no x2 here, unlike fixed amplitude mode).
    assert_eq!(chip.channel_level(0), 31, "frozen at the top of the ramp");

    // Advancing further must no longer change anything.
    chip.tick(400);
    assert_eq!(chip.channel_level(0), 31);
}

#[test]
fn writing_the_shape_register_restarts_the_envelope() {
    let mut chip = Ym2149::new();
    write_reg(&mut chip, reg::MIXER, MIXER_GATES_OPEN_A);
    write_reg(&mut chip, reg::ENVELOPE_FINE, 1);
    write_reg(&mut chip, reg::ENVELOPE_COARSE, 0);
    write_reg(&mut chip, reg::ENVELOPE_SHAPE, 0b0100); // attack
    for _ in 0..10 {
        chip.tick(4);
    }
    // Rewriting (same value or not) must restart from zero.
    write_reg(&mut chip, reg::ENVELOPE_SHAPE, 0b0100);
    write_reg(&mut chip, reg::AMPLITUDE_A, 0x10);
    assert_eq!(chip.channel_level(0), 0, "the envelope must have restarted from zero");
}

#[test]
fn port_a_toggles_between_input_and_output_according_to_ddr() {
    let mut chip = Ym2149::new();
    // Port A DDR = input (bit6 of MIXER = 0).
    write_reg(&mut chip, reg::MIXER, 0);
    chip.set_port_a_input(0x42);
    assert_eq!(read_reg(&mut chip, reg::IO_PORT_A), 0x42);

    // Switches to output (bit6 = 1): the read now reflects the written
    // latch, not the external input.
    write_reg(&mut chip, reg::MIXER, 0b0100_0000);
    write_reg(&mut chip, reg::IO_PORT_A, 0x99);
    assert_eq!(read_reg(&mut chip, reg::IO_PORT_A), 0x99);
}

// --- Non-linear 3-channel mixing (Hatari-style) --------------------------

#[test]
fn total_silence_gives_a_near_zero_level() {
    assert!(mix_channels_model([0.0, 0.0, 0.0]) < 0.01);
}

#[test]
fn three_channels_at_maximum_give_the_maximum_level() {
    let level = mix_channels_model([31.0, 31.0, 31.0]);
    assert!((level - 65535.0).abs() < 1.0, "expected ~65535, got {level}");
}

#[test]
fn mixing_saturates_instead_of_summing_linearly() {
    // Central property of the non-linear model: combining 3 channels at
    // full amplitude must NOT give 3x the level of a single channel
    // (the real DAC saturates) — a plain linear sum would give exactly
    // 3x by construction.
    let one_channel = mix_channels_model([31.0, 0.0, 0.0]);
    let three_channels = mix_channels_model([31.0, 31.0, 31.0]);
    assert!(
        three_channels < one_channel * 3.0,
        "3 channels ({three_channels}) should saturate below 3x 1 channel ({})",
        one_channel * 3.0
    );
    // Still strictly increasing (more active channels = louder),
    // just not proportionally.
    assert!(three_channels > one_channel);
}

#[test]
fn mixing_increases_with_a_channels_level() {
    let low = mix_channels_model([5.0, 0.0, 0.0]);
    let high = mix_channels_model([25.0, 0.0, 0.0]);
    assert!(high > low, "a higher channel level must give a louder output");
}

#[test]
fn fractional_interpolation_stays_between_neighboring_integer_levels() {
    // `take_averaged_levels` returns FRACTIONAL levels (time average) —
    // the model must remain monotonic/bounded for an intermediate value,
    // not just for integers.
    let at_10 = mix_channels_model([10.0, 0.0, 0.0]);
    let at_10_5 = mix_channels_model([10.5, 0.0, 0.0]);
    let at_11 = mix_channels_model([11.0, 0.0, 0.0]);
    assert!(at_10 < at_10_5 && at_10_5 < at_11, "10={at_10} 10.5={at_10_5} 11={at_11}");
}
