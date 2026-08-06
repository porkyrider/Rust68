//! Peripherals for host systems built on the 68000 core, one submodule per
//! system (see [`atari_st`]) — each compiled only if the corresponding
//! Cargo feature is enabled, so that the 68000 core alone remains usable
//! without pulling in or compiling code specific to a particular system.

#[cfg(feature = "atari-st")]
pub mod atari_st;
