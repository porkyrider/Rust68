//! Atari ST/STE system chips: MFP 68901, GLUE, ACIA (keyboard/MIDI),
//! YM2149 (PSG), Shifter (video), WD1772 (floppy disk), Blitter (STE),
//! `.stx` reader.
//!
//! This module is not part of the MC68000 CPU itself: it gathers the chips
//! of a particular system, which the caller wires into its
//! [`crate::Bus`] implementation (memory map, IPL generation…) — see
//! [`crate::systems::atari_st`]. Compiled only with the `atari-st` Cargo
//! feature (none enabled by default), so that the 68000 core alone remains
//! usable without pulling in or compiling code specific to a particular
//! system.

pub mod acia;
pub mod blitter;
pub mod dma_sound;
pub mod drive_sound;
pub mod glue;
pub mod ikbd;
pub mod mfp;
pub mod microwire;
pub mod msa;
pub mod shifter;
pub mod stx;
pub mod wd1772;
pub mod ym2149;
