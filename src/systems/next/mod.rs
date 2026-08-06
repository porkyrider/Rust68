//! Scaffolding for a future NeXT system — **not a functional system**.
//!
//! Unlike `systems::atari_st`, which was able to rely throughout on
//! Hatari/Steem SSE (local, cycle-accurate C sources, used as a correction
//! reference at every step), no NeXT ROM nor reference emulator (e.g.
//! Previous) is available on this machine at the time of writing this
//! module. Building a `Bus` wired to a real NeXT motherboard (memory map,
//! 68030/68040 PMMU, SCSI, Ethernet, sound, MO drive...) with nothing to
//! verify against would be an exercise in guesswork, not emulation — so this
//! module is limited to structure only, pending a dedicated session with
//! those references in hand.
//!
//! What already exists on the CPU core side that this system will consume:
//! the 68010 subset (`CpuType::M68010`, see `crate::cpu`) — a first step
//! towards 68020/68030/68040, which a real NeXT requires depending on the
//! model (see [`model::NextModel`]).

pub mod model;

use crate::Cpu;

/// State of a NeXT system — for now, only the CPU core. No [`crate::Bus`]
/// implementation, no RAM/ROM, no disk image loading: see the module doc.
pub struct Next {
    pub cpu: Cpu,
}

impl Next {
    pub fn new(model: model::NextModel) -> Self {
        let mut cpu = Cpu::new();
        cpu.cpu_type = model.cpu_type();
        Next { cpu }
    }
}
