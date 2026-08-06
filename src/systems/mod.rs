//! Complete systems built on the 68000 core and the [`crate::peripherals`]
//! chips: implement [`crate::Bus`] for a real memory map and interrupt
//! wiring. One submodule per system, each behind its own Cargo feature (see
//! [`atari_st`]) — the 68000 core alone remains usable without pulling in or
//! compiling any code specific to a particular system.

#[cfg(feature = "atari-st")]
pub mod atari_st;

#[cfg(feature = "next")]
pub mod next;
