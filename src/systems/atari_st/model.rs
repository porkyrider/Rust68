//! Lexicon of models in the Atari ST/STE/Mega ST/Mega STE lineup.
//!
//! General principle (valid for any machine being emulated, not just this
//! one): emulating ONE specific real machine is not about picking a random
//! RAM size and hoping it works — it's a set of characteristics that go
//! together (CPU speed, original RAM, expected ROM/BIOS, hardware options
//! such as the Blitter here). This module gathers these characteristics for
//! the ST/STE lineup in a queryable form ([`AtariModel::profile`]), rather
//! than leaving them scattered as magic constants throughout the demo
//! binary.
//!
//! ## What is modeled, and what isn't (yet)
//! - `ram_size` and `has_blitter` are fully taken into account by
//!   [`crate::systems::atari_st::AtariSt::from_model`] (installed RAM,
//!   effective presence of the Blitter on the bus).
//! - `cpu_hz` is **informational only** for now: the MFP pacing
//!   (`peripherals::atari_st::mfp`, fixed 192/625 clock ratio) and the
//!   audio pacing of the `atari_st_sdl2` binary both assume an 8 MHz CPU.
//!   Choosing a Mega STE model therefore does NOT run the emulation at
//!   16 MHz — the field exists to document the real hardware
//!   characteristic without claiming a precision that hasn't been
//!   implemented yet (rather than silently omitting it).
//! - `tos_version` is a **suggestion** (the model's original TOS,
//!   informational): the real ROM base (`0xFC0000` vs `0xE00000`) depends
//!   on the version of TOS actually loaded, not on the machine model —
//!   see `os_version` in the TOS header and
//!   [`crate::systems::atari_st::AtariSt::set_rom_base`], already
//!   auto-detected independently of this lexicon. A real ST can very well
//!   run with a TOS more recent than its original one (a common EPROM
//!   upgrade).
//!
//! ## Sources
//! Characteristics cross-checked from several public references (Wikipedia
//! "Atari MEGA STE", old-computers.com, atari-wiki.com, atari-forum.com) —
//! see each variant's comment for disputed details (Mega ST: the Blitter
//! was on a PLCC socket, not always populated at the factory).

/// A known model in the Atari ST/STE/Mega ST/Mega STE lineup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AtariModel {
    /// 1985: first model, 512 KB, TOS 1.00/1.02, no Blitter.
    St520,
    /// 1986: 1 MB, TOS 1.02/1.04, no Blitter.
    St1040,
    /// 1987: desktop case + separate keyboard, 1/2/4 MB, TOS 1.02/1.04.
    /// A PLCC socket for the Blitter is present on all motherboards, but
    /// not always populated at the factory on early batches — modeled here
    /// as present (the most common case among surviving/emulated
    /// machines), adjustable via
    /// [`MachineProfile`]/[`crate::systems::atari_st::AtariSt::from_model`]
    /// followed by a manual disable if a precise Blitter-less Mega ST is
    /// needed.
    MegaSt,
    /// 1989: 512 KB, TOS 1.06/1.62, Blitter and 4096-color palette as
    /// standard.
    Ste520,
    /// 1989: 1 MB, TOS 1.62 (1.06 on the very first units), Blitter as
    /// standard.
    Ste1040,
    /// 1991: TT case, 1/2/4 MB, TOS 2.05/2.06, Blitter as standard, CPU
    /// software-switchable 8/16 MHz with 16 KB cache (16 MHz not modeled,
    /// see module doc).
    MegaSte,
}

/// Characteristics of a model — see the module doc for what is actually
/// taken into account by the emulation today.
#[derive(Debug, Clone, Copy)]
pub struct MachineProfile {
    pub name: &'static str,
    /// Original CPU frequency, in Hz (informational, see module doc).
    pub cpu_hz: u32,
    /// Original installed RAM, in bytes.
    pub ram_size: usize,
    /// Original TOS version (informational, see module doc).
    pub tos_version: &'static str,
    /// Blitter present as standard on this model.
    pub has_blitter: bool,
    /// STE extended Shifter palette (12 bits, 4 bits per component) instead
    /// of the original ST format (9 bits, 3 bits per component).
    pub ste_palette: bool,
}

impl AtariModel {
    /// Returns the model's characteristics.
    pub fn profile(self) -> MachineProfile {
        match self {
            AtariModel::St520 => MachineProfile {
                name: "Atari 520ST",
                cpu_hz: 8_000_000,
                ram_size: 512 * 1024,
                tos_version: "1.02",
                has_blitter: false,
                ste_palette: false,
            },
            AtariModel::St1040 => MachineProfile {
                name: "Atari 1040ST",
                cpu_hz: 8_000_000,
                ram_size: 1024 * 1024,
                tos_version: "1.04",
                has_blitter: false,
                ste_palette: false,
            },
            AtariModel::MegaSt => MachineProfile {
                name: "Atari Mega ST",
                cpu_hz: 8_000_000,
                ram_size: 1024 * 1024,
                tos_version: "1.04",
                has_blitter: true,
                ste_palette: false,
            },
            AtariModel::Ste520 => MachineProfile {
                name: "Atari 520STE",
                cpu_hz: 8_000_000,
                ram_size: 512 * 1024,
                tos_version: "1.62",
                has_blitter: true,
                ste_palette: true,
            },
            AtariModel::Ste1040 => MachineProfile {
                name: "Atari 1040STE",
                cpu_hz: 8_000_000,
                ram_size: 1024 * 1024,
                tos_version: "1.62",
                has_blitter: true,
                ste_palette: true,
            },
            AtariModel::MegaSte => MachineProfile {
                name: "Atari Mega STE",
                cpu_hz: 8_000_000, // 16 MHz available on real hardware, not modeled
                ram_size: 4 * 1024 * 1024,
                tos_version: "2.06",
                has_blitter: true,
                ste_palette: true,
            },
        }
    }

    /// Looks up a model by case-insensitive name, accepting common forms
    /// (`"1040ste"`, `"1040STE"`, `"mega-ste"`, `"megaste"`…).
    pub fn parse(name: &str) -> Option<Self> {
        let normalized: String = name
            .chars()
            .filter(|c| c.is_alphanumeric())
            .collect::<String>()
            .to_lowercase();
        Some(match normalized.as_str() {
            "520st" => AtariModel::St520,
            "1040st" => AtariModel::St1040,
            "megast" => AtariModel::MegaSt,
            "520ste" => AtariModel::Ste520,
            "1040ste" => AtariModel::Ste1040,
            "megaste" => AtariModel::MegaSte,
            _ => return None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_accepts_several_forms() {
        assert_eq!(AtariModel::parse("1040ste"), Some(AtariModel::Ste1040));
        assert_eq!(AtariModel::parse("1040STE"), Some(AtariModel::Ste1040));
        assert_eq!(AtariModel::parse("Mega-STE"), Some(AtariModel::MegaSte));
        assert_eq!(AtariModel::parse("megaste"), Some(AtariModel::MegaSte));
        assert_eq!(AtariModel::parse("inconnu"), None);
    }

    #[test]
    fn profile_1040ste_matches_common_example() {
        // 1 MB of RAM, Blitter present, TOS 1.62 — the example given to
        // justify this lexicon.
        let p = AtariModel::Ste1040.profile();
        assert_eq!(p.ram_size, 1024 * 1024);
        assert!(p.has_blitter);
        assert_eq!(p.tos_version, "1.62");
    }

    #[test]
    fn st520_and_st1040_have_no_blitter() {
        assert!(!AtariModel::St520.profile().has_blitter);
        assert!(!AtariModel::St1040.profile().has_blitter);
    }
}
