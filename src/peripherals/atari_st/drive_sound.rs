//! Bruitage mécanique du lecteur de disquette (moteur, pas, recherche) —
//! porté du projet compagnon Stay (`stay_sound::DriveSound`, lui-même
//! calqué sur Steem SSE `floppy_drive.cpp` : `SoundVBL`/`SoundCheckCommand`/
//! `SoundStep`) : un échantillon de démarrage joue une fois quand le moteur
//! s'allume, un bourdonnement en boucle joue tant qu'il tourne, une commande
//! Step sur une seule piste joue un clic bref, et un Seek/Restore
//! multi-piste joue un bourdonnement de recherche en boucle pour toute la
//! durée du déplacement plutôt qu'un clic par piste.
//!
//! Ce module ne sait rien du WD1772 ni du disque : c'est à l'appelant de
//! consommer [`crate::peripherals::atari_st::wd1772::Wd1772::take_sound_events`]
//! et d'appeler la méthode [`DriveSound`] correspondante à chaque
//! évènement (voir `bin/atari_st_sdl2.rs`).
//!
//! Les échantillons eux-mêmes sont chargés séparément (voir [`wav`]) et
//! injectés via [`DriveSound::set_sample`] — ce module n'a aucun accès au
//! système de fichiers, et reste silencieux (une voix jamais remplie ne se
//! déclenche simplement jamais) si un slot n'est jamais rempli : un jeu
//! d'échantillons absent ou non livré dégrade proprement plutôt que de
//! faire échouer quoi que ce soit.

use super::wd1772::SoundEvent;

/// Parseur WAV minimal (RIFF/PCM), juste assez pour charger les fichiers
/// mono 8 ou 16 bits non compressés du jeu `3rdparty/DriveSound` de Steem
/// SSE (le même que celui utilisé par Stay).
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
                WavError::NotWav => write!(f, "pas un fichier RIFF/WAVE"),
                WavError::MissingFmt => write!(f, "bloc 'fmt ' manquant"),
                WavError::MissingData => write!(f, "bloc 'data' manquant"),
                WavError::UnsupportedFormat(tag) => write!(f, "format non pris en charge (PCM seulement), tag={tag}"),
                WavError::UnsupportedBitDepth(bits) => write!(f, "profondeur non prise en charge (8 ou 16 bits), obtenu {bits}"),
                WavError::UnsupportedChannels(ch) => write!(f, "nombre de canaux non pris en charge (mono seulement), obtenu {ch}"),
                WavError::Truncated => write!(f, "fichier tronqué"),
            }
        }
    }

    /// Parse un WAV mono PCM 8 ou 16 bits. Renvoie (échantillons en i16,
    /// fréquence d'échantillonnage). Le PCM 8 bits est non signé dans le
    /// format WAV (0..255, silence = 128) et est remis à l'échelle i16
    /// signée ici pour que l'appelant n'ait jamais à se soucier de la
    /// profondeur d'origine.
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

            // Les blocs sont alignés sur un mot : saute l'octet de bourrage
            // si `chunk_len` est impair.
            pos = body_end + (chunk_len & 1);
        }

        let (format_tag, channels, sample_rate, bits_per_sample) = fmt.ok_or(WavError::MissingFmt)?;
        let pcm = pcm.ok_or(WavError::MissingData)?;

        // 1 = PCM, 0xFFFE = WAVE_FORMAT_EXTENSIBLE (toujours du PCM en
        // pratique pour les fichiers mono simples ciblés ici).
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

    /// Rééchantillonnage au plus proche voisin d'un échantillon de sortie
    /// depuis `data`, selon la position de lecture courante de cette voix —
    /// un bourdonnement/clic mécanique n'a besoin de rien de plus fin.
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

/// Mélangeur de bruitage mécanique du lecteur — voir la doc du module.
pub struct DriveSound {
    samples: [Sample; NUM_SLOTS],
    voices: [Voice; NUM_SLOTS],
    output_rate: u32,
    /// Atténuation globale pour que ça reste une ambiance mécanique de
    /// fond plutôt que de rivaliser avec la musique PSG/DMA — Stay/Steem
    /// SSE atténue fortement de la même façon par rapport à la pleine
    /// échelle (`floppy_drive.cpp`, `SoundVolume`).
    volume_shift: u32,
}

impl DriveSound {
    pub fn new(output_rate: u32) -> Self {
        Self {
            samples: [Sample::default(), Sample::default(), Sample::default(), Sample::default()],
            voices: [Voice::new(), Voice::new(), Voice::new(), Voice::new()],
            output_rate,
            volume_shift: 2, // ÷4
        }
    }

    pub fn set_sample(&mut self, slot: Slot, data: Vec<i16>, native_rate: u32) {
        self.samples[slot as usize] = Sample { data, native_rate };
    }

    fn rate_ratio(&self, slot: Slot) -> f64 {
        let native = self.samples[slot as usize].native_rate.max(1);
        native as f64 / self.output_rate as f64
    }

    // ── Déclencheurs, appelés par l'appelant qui draine la file
    // `Wd1772::take_sound_events` ────────────────────────────────────────

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
        self.step_click(); // Steem/Stay : un dernier clic plus doux termine un seek.
    }

    /// Applique un lot d'évènements (voir
    /// [`crate::peripherals::atari_st::wd1772::Wd1772::take_sound_events`])
    /// aux voix correspondantes.
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

    /// Mélange `n` échantillons mono de bruitage additivement dans `out`
    /// (marge i32, comme pour le mélange PSG+DMA — l'appelant écrête vers
    /// i16 après avoir additionné toutes les sources).
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
