//! Périphériques des systèmes hôtes bâtis sur le cœur 68000, un
//! sous-module par système (voir [`atari_st`]) — chacun compilé uniquement
//! si la feature Cargo correspondante est activée, pour que le cœur 68000
//! seul reste utilisable sans rapatrier ni compiler de code spécifique à
//! un système particulier.

#[cfg(feature = "atari-st")]
pub mod atari_st;
