//! Mechanical floppy drive noise (motor, step, seek) — ported from the
//! companion Stay project (`stay_sound::DriveSound`, itself modeled on
//! Steem SSE's `floppy_drive.cpp`: `SoundVBL`/`SoundCheckCommand`/
//! `SoundStep`): a startup sample plays once when the motor turns on, a
//! looping hum plays while it's running, a Step command on a single track
//! plays a short click, and a multi-track Seek/Restore plays a looping
//! seek hum for the entire duration of the move rather than one click per
//! track.
//!
//! This module knows nothing about the WD1772 or the disk: it's up to the
//! caller to consume
//! [`crate::peripherals::atari_st::wd1772::Wd1772::take_sound_events`]
//! and call the corresponding [`DriveSound`] method for each event (see
//! `bin/atari_st_sdl2.rs`).
//!
//! The samples themselves are loaded separately (see [`wav`]) and
//! injected via [`DriveSound::set_sample`] — this module has no
//! filesystem access, and stays silent (a voice that's never filled
//! simply never triggers) if a slot is never filled: a missing or
//! undelivered sample set degrades gracefully rather than causing
//! anything to fail.

use super::wd1772::SoundEvent;

/// Minimal WAV parser (RIFF/PCM), just enough to load the uncompressed
/// mono 8 or 16-bit files from Steem SSE's `3rdparty/DriveSound` set (the
/// same one used by Stay).
pub mod wav {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum WavError {
        NotWav,
        MissingFmt,
        MissingData,
        UnsupportedFormat(u16),
        UnsupportedBitDepth(u16),
        UnsupportedChannels(u16),
        Truncated,
    }

    impl std::fmt::Display for WavError {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            match self {
                WavError::NotWav => write!(f, "not a RIFF/WAVE file"),
                WavError::MissingFmt => write!(f, "missing 'fmt ' chunk"),
                WavError::MissingData => write!(f, "missing 'data' chunk"),
                WavError::UnsupportedFormat(tag) => write!(f, "unsupported format (PCM only), tag={tag}"),
                WavError::UnsupportedBitDepth(bits) => write!(f, "unsupported bit depth (8 or 16 bits only), got {bits}"),
                WavError::UnsupportedChannels(ch) => write!(f, "unsupported channel count (mono only), got {ch}"),
                WavError::Truncated => write!(f, "truncated file"),
            }
        }
    }

    /// Parses a mono PCM 8 or 16-bit WAV. Returns (samples as i16, sample
    /// rate). 8-bit PCM is unsigned in the WAV format (0..255, silence =
    /// 128) and is rescaled to signed i16 here so the caller never has to
    /// worry about the original bit depth.
    pub fn load_wav_mono_i16(data: &[u8]) -> Result<(Vec<i16>, u32), WavError> {
        if data.len() < 12 || &data[0..4] != b"RIFF" || &data[8..12] != b"WAVE" {
            return Err(WavError::NotWav);
        }

        let mut pos = 12usize;
        let mut fmt: Option<(u16, u16, u32, u16)> = None;
        let mut pcm: Option<&[u8]> = None;

        while pos + 8 <= data.len() {
            let chunk_id = &data[pos..pos + 4];
            let chunk_len =
                u32::from_le_bytes([data[pos + 4], data[pos + 5], data[pos + 6], data[pos + 7]]) as usize;
            let body_start = pos + 8;
            let body_end = body_start.checked_add(chunk_len).ok_or(WavError::Truncated)?;
            if body_end > data.len() {
                return Err(WavError::Truncated);
            }
            let body = &data[body_start..body_end];

            match chunk_id {
                b"fmt " => {
                    if body.len() < 16 {
                        return Err(WavError::Truncated);
                    }
                    let format_tag = u16::from_le_bytes([body[0], body[1]]);
                    let channels = u16::from_le_bytes([body[2], body[3]]);
                    let sample_rate = u32::from_le_bytes([body[4], body[5], body[6], body[7]]);
                    let bits_per_sample = u16::from_le_bytes([body[14], body[15]]);
                    fmt = Some((format_tag, channels, sample_rate, bits_per_sample));
                }
                b"data" => pcm = Some(body),
                _ => {}
            }

            // Chunks are word-aligned: skip the padding byte if
            // `chunk_len` is odd.
            pos = body_end + (chunk_len & 1);
        }

        let (format_tag, channels, sample_rate, bits_per_sample) = fmt.ok_or(WavError::MissingFmt)?;
        let pcm = pcm.ok_or(WavError::MissingData)?;

        // 1 = PCM, 0xFFFE = WAVE_FORMAT_EXTENSIBLE (always PCM in practice
        // for the simple mono files targeted here).
        if format_tag != 1 && format_tag != 0xFFFE {
            return Err(WavError::UnsupportedFormat(format_tag));
        }
        if channels != 1 {
            return Err(WavError::UnsupportedChannels(channels));
        }

        let samples = match bits_per_sample {
            8 => pcm.iter().map(|&b| ((b as i16) - 128) * 256).collect(),
            16 => pcm.chunks_exact(2).map(|c| i16::from_le_bytes([c[0], c[1]])).collect(),
            other => return Err(WavError::UnsupportedBitDepth(other)),
        };

        Ok((samples, sample_rate))
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Slot {
    Start = 0,
    Motor = 1,
    Step = 2,
    Seek = 3,
}
const NUM_SLOTS: usize = 4;

#[derive(Default)]
struct Sample {
    data: Vec<i16>,
    native_rate: u32,
}

struct Voice {
    active: bool,
    looping: bool,
    pos: f64,
    rate_ratio: f64, // native_rate / output_rate
}

impl Voice {
    fn new() -> Self {
        Self { active: false, looping: false, pos: 0.0, rate_ratio: 1.0 }
    }

    fn start(&mut self, looping: bool, rate_ratio: f64) {
        self.active = true;
        self.looping = looping;
        self.pos = 0.0;
        self.rate_ratio = rate_ratio;
    }

    fn stop(&mut self) {
        self.active = false;
    }

    /// Nearest-neighbor resampling of an output sample from `data`,
    /// according to this voice's current playback position — a mechanical
    /// hum/click doesn't need anything finer.
    fn next(&mut self, data: &[i16]) -> i32 {
        if !self.active || data.is_empty() {
            return 0;
        }
        let idx = self.pos as usize;
        if idx >= data.len() {
            if self.looping {
                self.pos -= data.len() as f64;
            } else {
                self.active = false;
                return 0;
            }
        }
        let idx = (self.pos as usize).min(data.len() - 1);
        let v = data[idx];
        self.pos += self.rate_ratio;
        v as i32
    }
}

/// Drive mechanical noise mixer — see the module doc.
pub struct DriveSound {
    samples: [Sample; NUM_SLOTS],
    voices: [Voice; NUM_SLOTS],
    output_rate: u32,
    /// Overall attenuation so it stays a background mechanical ambience
    /// rather than competing with the PSG/DMA music — Stay/Steem SSE
    /// attenuates heavily the same way relative to full scale
    /// (`floppy_drive.cpp`, `SoundVolume`).
    volume_shift: u32,
}

impl DriveSound {
    pub fn new(output_rate: u32) -> Self {
        Self {
            samples: [Sample::default(), Sample::default(), Sample::default(), Sample::default()],
            voices: [Voice::new(), Voice::new(), Voice::new(), Voice::new()],
            output_rate,
            volume_shift: 2, // /4
        }
    }

    pub fn set_sample(&mut self, slot: Slot, data: Vec<i16>, native_rate: u32) {
        self.samples[slot as usize] = Sample { data, native_rate };
    }

    fn rate_ratio(&self, slot: Slot) -> f64 {
        let native = self.samples[slot as usize].native_rate.max(1);
        native as f64 / self.output_rate as f64
    }

    // ── Triggers, called by the caller draining the
    // `Wd1772::take_sound_events` queue ─────────────────────────────────

    fn motor_on(&mut self) {
        let r = self.rate_ratio(Slot::Start);
        self.voices[Slot::Start as usize].start(false, r);
        let r = self.rate_ratio(Slot::Motor);
        self.voices[Slot::Motor as usize].start(true, r);
    }

    fn motor_off(&mut self) {
        self.voices[Slot::Motor as usize].stop();
    }

    fn step_click(&mut self) {
        let r = self.rate_ratio(Slot::Step);
        self.voices[Slot::Step as usize].start(false, r);
    }

    fn seek_start(&mut self) {
        let r = self.rate_ratio(Slot::Seek);
        self.voices[Slot::Seek as usize].start(true, r);
    }

    fn seek_end(&mut self) {
        self.voices[Slot::Seek as usize].stop();
        self.step_click(); // Steem/Stay: one final, softer click ends a seek.
    }

    /// Applies a batch of events (see
    /// [`crate::peripherals::atari_st::wd1772::Wd1772::take_sound_events`])
    /// to the corresponding voices.
    pub fn handle_events(&mut self, events: &[SoundEvent]) {
        for event in events {
            match event {
                SoundEvent::MotorOn => self.motor_on(),
                SoundEvent::MotorOff => self.motor_off(),
                SoundEvent::StepClick => self.step_click(),
                SoundEvent::SeekStart => self.seek_start(),
                SoundEvent::SeekEnd => self.seek_end(),
            }
        }
    }

    /// Additively mixes `n` mono noise samples into `out` (i32 headroom,
    /// as for the PSG+DMA mix — the caller clips to i16 after summing all
    /// sources).
    pub fn mix_into(&mut self, out: &mut [i32]) {
        for slot_idx in 0..NUM_SLOTS {
            let data = &self.samples[slot_idx].data;
            if data.is_empty() {
                continue;
            }
            let voice = &mut self.voices[slot_idx];
            for o in out.iter_mut() {
                *o += voice.next(data) >> self.volume_shift;
            }
        }
    }
}
