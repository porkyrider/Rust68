//! Lexique des modèles de la gamme Atari ST/STE/Mega ST/Mega STE.
//!
//! Principe général (valable pour n'importe quelle machine qu'on émule,
//! pas seulement celle-ci) : émuler UNE machine réelle précise, ce n'est
//! pas choisir une taille de RAM au hasard puis espérer que ça marche —
//! c'est un ensemble de caractéristiques qui vont ensemble (vitesse CPU,
//! RAM d'origine, ROM/BIOS attendue, options matérielles comme le
//! Blitter ici). Ce module rassemble ces caractéristiques pour la gamme
//! ST/STE sous une forme consultable ([`AtariModel::profile`]), plutôt que
//! de les laisser éparpillées en constantes magiques dans le binaire de
//! démonstration.
//!
//! ## Ce qui est modélisé, et ce qui ne l'est pas (encore)
//! - `ram_size` et `has_blitter` sont pleinement pris en compte par
//!   [`crate::systems::atari_st::AtariSt::from_model`] (RAM installée,
//!   présence effective du Blitter sur le bus).
//! - `cpu_hz` est **informatif seulement** pour l'instant : le rythme
//!   MFP (`peripherals::atari_st::mfp`, ratio horloge fixe 192/625) et le
//!   pacing audio du binaire `atari_st_sdl2` supposent tous les deux un
//!   CPU à 8 MHz. Choisir un modèle Mega STE ne fait donc PAS tourner
//!   l'émulation à 16 MHz — le champ existe pour documenter la vraie
//!   caractéristique matérielle sans prétendre à une précision qu'on n'a
//!   pas encore implémentée (plutôt que de l'omettre en silence).
//! - `tos_version` est une **suggestion** (le TOS d'origine du modèle,
//!   informatif) : la base ROM réelle (`0xFC0000` vs `0xE00000`) dépend de
//!   la version de TOS effectivement chargée, pas du modèle de machine —
//!   voir `os_version` dans l'en-tête TOS et
//!   [`crate::systems::atari_st::AtariSt::set_rom_base`], déjà auto-détecté
//!   indépendamment de ce lexique. Un ST réel peut très bien tourner avec
//!   un TOS plus récent que celui d'origine (mise à jour EPROM courante).
//!
//! ## Sources
//! Caractéristiques croisées depuis plusieurs références publiques
//! (Wikipedia "Atari MEGA STE", old-computers.com, atari-wiki.com,
//! atari-forum.com) — voir le commentaire de chaque variante pour le
//! détail contesté (Mega ST : le Blitter était sur un support PLCC, pas
//! toujours peuplé en usine).

/// Un modèle connu de la gamme Atari ST/STE/Mega ST/Mega STE.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AtariModel {
    /// 1985 : premier modèle, 512 Ko, TOS 1.00/1.02, pas de Blitter.
    St520,
    /// 1986 : 1 Mo, TOS 1.02/1.04, pas de Blitter.
    St1040,
    /// 1987 : boîtier bureautique + clavier séparé, 1/2/4 Mo, TOS 1.02/1.04.
    /// Support PLCC pour le Blitter présent sur toutes les cartes mères,
    /// mais pas systématiquement peuplé en usine sur les premières séries
    /// — modélisé ici comme présent (cas le plus courant sur les machines
    /// qui ont survécu/sont émulées), à ajuster via
    /// [`MachineProfile`]/[`crate::systems::atari_st::AtariSt::from_model`]
    /// suivi d'une désactivation manuelle si besoin d'un Mega ST sans
    /// Blitter précis.
    MegaSt,
    /// 1989 : 512 Ko, TOS 1.06/1.62, Blitter et palette 4096 couleurs de
    /// série.
    Ste520,
    /// 1989 : 1 Mo, TOS 1.62 (1.06 sur les tout premiers exemplaires),
    /// Blitter de série.
    Ste1040,
    /// 1991 : boîtier TT, 1/2/4 Mo, TOS 2.05/2.06, Blitter de série, CPU
    /// commutable logiciellement 8/16 MHz avec cache 16 Ko (16 MHz non
    /// modélisé, voir la doc de module).
    MegaSte,
}

/// Caractéristiques d'un modèle — voir la doc de module pour ce qui est
/// effectivement pris en compte par l'émulation aujourd'hui.
#[derive(Debug, Clone, Copy)]
pub struct MachineProfile {
    pub name: &'static str,
    /// Fréquence CPU d'origine, en Hz (informatif, voir doc de module).
    pub cpu_hz: u32,
    /// RAM installée d'origine, en octets.
    pub ram_size: usize,
    /// Version de TOS d'origine (informatif, voir doc de module).
    pub tos_version: &'static str,
    /// Blitter présent de série sur ce modèle.
    pub has_blitter: bool,
}

impl AtariModel {
    /// Renvoie les caractéristiques du modèle.
    pub fn profile(self) -> MachineProfile {
        match self {
            AtariModel::St520 => MachineProfile {
                name: "Atari 520ST",
                cpu_hz: 8_000_000,
                ram_size: 512 * 1024,
                tos_version: "1.02",
                has_blitter: false,
            },
            AtariModel::St1040 => MachineProfile {
                name: "Atari 1040ST",
                cpu_hz: 8_000_000,
                ram_size: 1024 * 1024,
                tos_version: "1.04",
                has_blitter: false,
            },
            AtariModel::MegaSt => MachineProfile {
                name: "Atari Mega ST",
                cpu_hz: 8_000_000,
                ram_size: 1024 * 1024,
                tos_version: "1.04",
                has_blitter: true,
            },
            AtariModel::Ste520 => MachineProfile {
                name: "Atari 520STE",
                cpu_hz: 8_000_000,
                ram_size: 512 * 1024,
                tos_version: "1.62",
                has_blitter: true,
            },
            AtariModel::Ste1040 => MachineProfile {
                name: "Atari 1040STE",
                cpu_hz: 8_000_000,
                ram_size: 1024 * 1024,
                tos_version: "1.62",
                has_blitter: true,
            },
            AtariModel::MegaSte => MachineProfile {
                name: "Atari Mega STE",
                cpu_hz: 8_000_000, // 16 MHz disponible sur le matériel réel, non modélisé
                ram_size: 4 * 1024 * 1024,
                tos_version: "2.06",
                has_blitter: true,
            },
        }
    }

    /// Cherche un modèle par nom insensible à la casse, acceptant les
    /// formes usuelles (`"1040ste"`, `"1040STE"`, `"mega-ste"`, `"megaste"`…).
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
    fn parse_accepte_plusieurs_formes() {
        assert_eq!(AtariModel::parse("1040ste"), Some(AtariModel::Ste1040));
        assert_eq!(AtariModel::parse("1040STE"), Some(AtariModel::Ste1040));
        assert_eq!(AtariModel::parse("Mega-STE"), Some(AtariModel::MegaSte));
        assert_eq!(AtariModel::parse("megaste"), Some(AtariModel::MegaSte));
        assert_eq!(AtariModel::parse("inconnu"), None);
    }

    #[test]
    fn profil_1040ste_correspond_a_l_exemple_courant() {
        // 1 Mo de RAM, Blitter présent, TOS 1.62 — l'exemple donné pour
        // justifier ce lexique.
        let p = AtariModel::Ste1040.profile();
        assert_eq!(p.ram_size, 1024 * 1024);
        assert!(p.has_blitter);
        assert_eq!(p.tos_version, "1.62");
    }

    #[test]
    fn st520_et_st1040_n_ont_pas_de_blitter() {
        assert!(!AtariModel::St520.profile().has_blitter);
        assert!(!AtariModel::St1040.profile().has_blitter);
    }
}
