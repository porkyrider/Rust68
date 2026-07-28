//! Périphériques des systèmes hôtes (Atari ST, …) bâtis sur le cœur 68000.
//!
//! Ce module ne fait pas partie du CPU MC68000 lui-même : il rassemble les
//! puces d'un système particulier, que l'appelant câble dans son
//! implémentation de [`crate::Bus`] (mapping mémoire, génération d'IPL…).

pub mod acia;
pub mod glue;
pub mod mfp;
pub mod shifter;
pub mod ym2149;
