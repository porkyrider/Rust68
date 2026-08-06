//! Lexicon of NeXT models targeted eventually — same principle as
//! `systems::atari_st::model` (a specific real machine, not a randomly
//! chosen RAM size), but here purely declarative for now: see the doc of
//! [`super`] for what is missing before a [`NextModel`] can actually boot
//! anything.

/// A known model in the NeXT lineup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NextModel {
    /// 1988: the original "Cube", 68030 + 68882 FPU + external 68851 PMMU.
    Cube,
    /// 1990: pizza-box case, same 68030 core as the Cube.
    Station,
    /// 1991: Cube with "Turbo" board, 68040 (integrated MMU/FPU).
    CubeTurbo,
    /// 1991: Station with "Turbo" board, 68040.
    StationTurbo,
}

impl NextModel {
    /// Corresponding 68k core variant — the only characteristic wired up
    /// for now (see the doc of [`super`]). `M68010` is an intermediate
    /// milestone: neither 68030 (integrated PMMU) nor 68040 (integrated
    /// MMU+FPU) are implemented yet in [`crate::cpu`], so no variant can
    /// today faithfully represent a real NeXT — this method exists so that
    /// future wiring (RAM, ROM, ...) already has a consistent hook point
    /// with `systems::atari_st::model`.
    pub fn cpu_type(self) -> crate::CpuType {
        crate::CpuType::M68010
    }
}
