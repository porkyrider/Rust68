//! Puces du système Atari ST/STE : MFP 68901, GLUE, ACIA (clavier/MIDI),
//! YM2149 (PSG), Shifter (vidéo), WD1772 (disquette), Blitter (STE).
//!
//! Ce module ne fait pas partie du CPU MC68000 lui-même : il rassemble les
//! puces d'un système particulier, que l'appelant câble dans son
//! implémentation de [`crate::Bus`] (mapping mémoire, génération d'IPL…) —
//! voir [`crate::systems::atari_st`]. Compilé uniquement avec la feature
//! Cargo `atari-st` (activée par défaut), pour que le cœur 68000 seul
//! reste utilisable sans rapatrier ni compiler de code spécifique à un
//! système particulier.

pub mod acia;
pub mod blitter;
pub mod glue;
pub mod mfp;
pub mod shifter;
pub mod wd1772;
pub mod ym2149;
