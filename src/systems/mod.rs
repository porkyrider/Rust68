//! Systèmes complets bâtis sur le cœur 68000 et les puces de
//! [`crate::peripherals`] : implémentent [`crate::Bus`] pour un mapping
//! mémoire et un câblage d'interruption réels. Un sous-module par système,
//! chacun derrière sa propre feature Cargo (voir [`atari_st`]) — le cœur
//! 68000 seul reste utilisable sans rapatrier ni compiler de code
//! spécifique à un système particulier.

#[cfg(feature = "atari-st")]
pub mod atari_st;
