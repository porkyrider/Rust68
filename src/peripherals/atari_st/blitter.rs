//! Blitter — coprocesseur de transfert de blocs (BitBlt) de l'Atari STE.
//!
//! Combine un mot source (optionnellement décalé bit à bit via `skew` pour
//! aligner des images non alignées sur un mot), un motif de demi-teinte
//! (halftone), et le contenu destination actuel, via une fonction booléenne
//! programmable (`OP`, une des 16 fonctions à 2 entrées), avec masquage de
//! bord de ligne (`ENDMASK1/2/3`) et parcours par incréments X/Y.
//!
//! Ce module modélise la puce **seule** : [`Blitter::execute`] prend un
//! `Bus` (pour lire/écrire la RAM aux adresses source/destination) et fait
//! progresser le blit — en mode HOG (bit 6 de CONTROL posé), il s'exécute
//! intégralement en un seul appel ; en mode partagé (non-HOG, le cas le
//! plus courant en pratique), un seul appel ne traite qu'une tranche de 16
//! mots avant de rendre la main (voir [`Self::execute`] et sa doc pour le
//! détail), `BUSY` restant posé entre deux tranches — donc bien observable
//! par polling dans ce mode, contrairement à une version antérieure de ce
//! module entièrement synchrone. C'est au board de mapper
//! [`Blitter::read`]/[`Blitter::write`] dans son `Bus`, de déclencher un
//! premier appel à `execute` quand le bit START du registre de contrôle
//! est écrit, ET de rappeler `execute` périodiquement pour faire progresser
//! un blit non-HOG en pause — voir `systems::atari_st::AtariSt::tick`, qui
//! le fait de façon autonome au rythme du CPU (accès bus partagés avec le
//! CPU, comme sur silicium réel), plutôt que de dépendre uniquement d'une
//! réécriture logicielle de CONTROL (ce qui fonctionnait par coïncidence
//! avec la boucle `TAS.B` de TOS mais pas avec un simple `BTST.B` de
//! scrutation).
//!
//! Registres et sémantique des bits `FXSR`/`NFSR`/`SMUDGE`/HOP/numéro de
//! ligne de demi-teinte croisés contre plusieurs sources indépendantes :
//! le datasheet `BLITTER.TXT` (info-coach.fr), le `BLIT_FAQ.TXT`
//! (dépôt `ggnkua/Atari_ST_Sources`) et le code source de Hatari
//! (`src/blitter.c`), qui se recoupent — voir le détail par item
//! ci-dessous. Contrairement à l'Amiga, le Blitter Atari STE **n'a pas**
//! de mode "tracé de ligne" pour du dessin de polygone : le champ "numéro
//! de ligne" du registre CONTROL ne sert qu'à sélectionner/pré-positionner
//! le mot de demi-teinte courant (modélisé ci-dessous).
//!
//! ## Limitations connues (v1) — à prendre avec prudence
//! - Le vol de cycles bus au CPU (mode "hog"/"steal") EST modélisé (voir
//!   ci-dessus et `systems::atari_st::AtariSt::tick`/`BLITTER_SLICE_CYCLES`),
//!   par une tranche de 64 accès bus RÉELS entre deux passages de bus
//!   (`BUS_ACCESSES_PER_SLICE` dans [`Self::execute`] — lecture source,
//!   lecture destination et écriture destination chacune comptée
//!   séparément, pas un nombre de mots traités), reprise directement du
//!   source de Hatari (`src/blitter.c`, `BLITTER_NONHOG_BUS_BLITTER`).
//!   Reste néanmoins une approximation par rapport au mode "cycle exact"
//!   de Hatari, qui entrelace ces accès AU MILIEU de l'exécution des
//!   instructions CPU (plutôt qu'entre deux instructions complètes comme
//!   ici) et reproduit un cas de bug documenté du silicium réel où le
//!   Blitter s'arrête parfois à 63 accès au lieu de 64 — non modélisé ici.
//! - Aucune suite de test équivalente à TomHarte n'existe pour le
//!   Blitter : la logique est vérifiée par recoupement documentaire
//!   (datasheet, BLIT_FAQ.TXT, code source de Hatari) plutôt que contre
//!   des vecteurs de test matériel.

/// Offsets des registres dans l'espace propre de la puce (à additionner à
/// l'adresse de base du board, `0xFF8A00` sur STE réel).
pub mod reg {
    /// 16 mots de motif de demi-teinte, offsets `0x00`, `0x02`, … `0x1E`.
    pub const HALFTONE_BASE: u32 = 0x00;
    pub const SRC_X_INC: u32 = 0x20;
    pub const SRC_X_INC1: u32 = 0x21;
    pub const SRC_Y_INC: u32 = 0x22;
    pub const SRC_Y_INC1: u32 = 0x23;
    /// Adresse source (32 bits, seuls les 24 bits bas sont significatifs).
    pub const SRC_ADDR: u32 = 0x24;
    pub const SRC_ADDR1: u32 = 0x25;
    pub const SRC_ADDR2: u32 = 0x26;
    pub const SRC_ADDR3: u32 = 0x27;
    pub const ENDMASK_1: u32 = 0x28;
    pub const ENDMASK_11: u32 = 0x29;
    pub const ENDMASK_2: u32 = 0x2A;
    pub const ENDMASK_21: u32 = 0x2B;
    pub const ENDMASK_3: u32 = 0x2C;
    pub const ENDMASK_31: u32 = 0x2D;
    pub const DST_X_INC: u32 = 0x2E;
    pub const DST_X_INC1: u32 = 0x2F;
    pub const DST_Y_INC: u32 = 0x30;
    pub const DST_Y_INC1: u32 = 0x31;
    /// Adresse destination (32 bits, seuls les 24 bits bas sont significatifs).
    pub const DST_ADDR: u32 = 0x32;
    pub const DST_ADDR1: u32 = 0x33;
    pub const DST_ADDR2: u32 = 0x34;
    pub const DST_ADDR3: u32 = 0x35;
    pub const X_COUNT: u32 = 0x36;
    pub const X_COUNT1: u32 = 0x37;
    pub const Y_COUNT: u32 = 0x38;
    pub const Y_COUNT1: u32 = 0x39;
    pub const HOP: u32 = 0x3A;
    pub const OP: u32 = 0x3B;
    /// Bit 7 = BUSY (écriture : start/stop du blit ; lecture : busy/idle),
    /// bit 6 = HOG, bit 5 = SMUDGE, bits 3-0 = numéro de ligne de
    /// demi-teinte courant — lisible/inscriptible directement (pas un
    /// compteur interne caché : le logiciel peut le pré-positionner).
    pub const CONTROL: u32 = 0x3C;
    /// Bit 7 = FXSR (Force eXtra Source Read), bit 6 = NFSR (No Final
    /// Source Read), bits 3-0 = décalage (skew, nombre de bits de
    /// décalage à droite).
    pub const SKEW: u32 = 0x3D;
    /// Fin de l'espace registre (exclusif).
    pub const END: u32 = 0x3E;
}

const CONTROL_BUSY: u8 = 1 << 7;
const CONTROL_HOG: u8 = 1 << 6;

/// PC courant du CPU, pour diagnostic uniquement (`RUST68_TRACE_BLIT_REGS`) —
/// mis à jour par l'appelant (ex. `examples/rd_menu_ca6a.rs`) juste avant
/// chaque `cpu.step()`, lu par les traces d'écriture de registre ci-dessous
/// pour identifier l'instruction ROM responsable sans changer la signature
/// publique de `write`/`write_word`.
pub static DEBUG_LAST_PC: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

#[derive(Debug, Clone)]
pub struct Blitter {
    halftone: [u16; 16],
    src_x_inc: i16,
    src_y_inc: i16,
    src_addr: u32,
    endmask: [u16; 3],
    dst_x_inc: i16,
    dst_y_inc: i16,
    dst_addr: u32,
    /// Mots par ligne. Stocké sur 32 bits (pas 16) pour pouvoir représenter
    /// la valeur 65536 — voir [`Self::write_word_count`].
    x_count: u32,
    /// Lignes par bloc. Même remarque que [`Self::x_count`].
    y_count: u32,
    hop: u8,
    op: u8,
    skew: u8,
    /// Bit 7 = BUSY, bit 6 = HOG, bit 5 = SMUDGE, bits 3-0 = numéro de
    /// ligne de demi-teinte courant (voir [`reg::CONTROL`]).
    control: u8,
    /// "Armement" du Blitter — voir [`Self::execute`] pour le bug réel que
    /// ce champ corrige.
    armed: bool,
    /// Vrai entre le début et la fin RÉELLE (Y_COUNT atteignant 0) d'un
    /// blit — c'est-à-dire tant qu'il reste du travail, y compris entre
    /// deux tranches en mode non-HOG (voir [`Self::execute`]). Distinct de
    /// `armed` : `armed` autorise un DÉMARRAGE (ou redémarrage explicite,
    /// Y_COUNT vient d'être réécrit), `mid_blit` autorise la POURSUITE d'un
    /// blit déjà commencé sans qu'aucun registre n'ait besoin d'être
    /// réécrit entre les tranches.
    mid_blit: bool,
    /// Registre à décalage source 32 bits, persistant entre les tranches
    /// (voir [`Self::execute`]).
    buffer: u32,
    /// Dernier mot effectivement lu sur le bus source — réutilisé par NFSR
    /// (voir [`Self::execute`]).
    bus_word: u16,
    /// Vrai si la lecture d'amorçage FXSR de la ligne EN COURS a déjà eu
    /// lieu (remis à faux à chaque nouvelle ligne).
    have_fxsr: bool,
    /// Vrai si la prochaine lecture source doit être omise (NFSR,
    /// positionné dynamiquement quand X_COUNT atteint 2).
    nfsr_dynamic: bool,
    /// Largeur (en mots) de la ligne en cours de blit — capturée une seule
    /// fois au véritable début du blit (contrairement à `x_count`, qui
    /// décompte réellement et se réinitialise à cette valeur à chaque fin
    /// de ligne).
    x_count_reset: u32,
}

impl Default for Blitter {
    fn default() -> Self {
        Self::new()
    }
}

impl Blitter {
    pub fn new() -> Self {
        Blitter {
            halftone: [0; 16],
            src_x_inc: 0,
            src_y_inc: 0,
            src_addr: 0,
            endmask: [0xFFFF; 3],
            dst_x_inc: 0,
            dst_y_inc: 0,
            dst_addr: 0,
            x_count: 0,
            y_count: 0,
            hop: 0,
            op: 0,
            skew: 0,
            control: 0,
            armed: false,
            mid_blit: false,
            buffer: 0,
            bus_word: 0,
            have_fxsr: false,
            nfsr_dynamic: false,
            x_count_reset: 0,
        }
    }

    /// Vrai si le bit BUSY du registre de contrôle est actif. En mode HOG,
    /// toujours faux juste après [`Self::execute`] (le blit s'y termine en
    /// un seul appel). En mode non-HOG, peut rester vrai entre deux
    /// tranches d'un blit encore en cours — voir la doc du module — donc
    /// bien observable par polling dans ce mode, contrairement à un modèle
    /// entièrement synchrone. Consulté par
    /// `systems::atari_st::AtariSt::tick` pour savoir s'il faut continuer à
    /// faire progresser le blit.
    pub fn busy(&self) -> bool {
        self.control & CONTROL_BUSY != 0
    }

    /// Lit le registre à l'offset `addr` (voir [`reg`]).
    pub fn read(&self, addr: u32) -> u8 {
        match addr {
            a if a < 0x20 && a % 2 == 0 => (self.halftone[(a / 2) as usize] >> 8) as u8,
            a if a < 0x20 => self.halftone[(a / 2) as usize] as u8,
            reg::SRC_X_INC => (self.src_x_inc >> 8) as u8,
            reg::SRC_X_INC1 => self.src_x_inc as u8,
            reg::SRC_Y_INC => (self.src_y_inc >> 8) as u8,
            reg::SRC_Y_INC1 => self.src_y_inc as u8,
            reg::SRC_ADDR => (self.src_addr >> 24) as u8,
            reg::SRC_ADDR1 => (self.src_addr >> 16) as u8,
            reg::SRC_ADDR2 => (self.src_addr >> 8) as u8,
            reg::SRC_ADDR3 => self.src_addr as u8,
            reg::ENDMASK_1 => (self.endmask[0] >> 8) as u8,
            reg::ENDMASK_11 => self.endmask[0] as u8,
            reg::ENDMASK_2 => (self.endmask[1] >> 8) as u8,
            reg::ENDMASK_21 => self.endmask[1] as u8,
            reg::ENDMASK_3 => (self.endmask[2] >> 8) as u8,
            reg::ENDMASK_31 => self.endmask[2] as u8,
            reg::DST_X_INC => (self.dst_x_inc >> 8) as u8,
            reg::DST_X_INC1 => self.dst_x_inc as u8,
            reg::DST_Y_INC => (self.dst_y_inc >> 8) as u8,
            reg::DST_Y_INC1 => self.dst_y_inc as u8,
            reg::DST_ADDR => (self.dst_addr >> 24) as u8,
            reg::DST_ADDR1 => (self.dst_addr >> 16) as u8,
            reg::DST_ADDR2 => (self.dst_addr >> 8) as u8,
            reg::DST_ADDR3 => self.dst_addr as u8,
            // Le registre matériel reste 16 bits : relire après une écriture
            // qui a converti 0 en 65536 en interne (voir
            // `write_word_count`) redonne 0, pas 65536 — confirmé par
            // Hatari (`Blitter_WordsPerLine_ReadWord`, masque `& 0xFFFF`).
            reg::X_COUNT => ((self.x_count & 0xFFFF) >> 8) as u8,
            reg::X_COUNT1 => (self.x_count & 0xFF) as u8,
            reg::Y_COUNT => ((self.y_count & 0xFFFF) >> 8) as u8,
            reg::Y_COUNT1 => (self.y_count & 0xFF) as u8,
            reg::HOP => self.hop,
            reg::OP => self.op,
            reg::SKEW => self.skew,
            reg::CONTROL => self.control,
            _ => 0xFF,
        }
    }

    /// Écrit le registre à l'offset `addr`.
    ///
    /// Note sur l'accès `.B` isolé : le manuel Blitter officiel et Hatari
    /// (`Blitter_CheckAccess_Byte`) documentent que la plupart de ces
    /// registres IGNORENT un accès `.B` isolé sur le silicium réel (seul un
    /// accès `.W`/`.L` complet est honoré). Une tentative d'implémenter
    /// fidèlement cette règle ici a provoqué un plantage direct au premier
    /// déclenchement d'un blit sur ce TOS/cette démo précis — signe que le
    /// logiciel réel s'appuie bel et bien, quelque part, sur un accès `.B`
    /// pour composer un registre, contrairement à ce que documente Hatari
    /// pour le matériel de référence qu'il émule. Composition octet par
    /// octet conservée ci-dessous en attendant de comprendre cet écart.
    pub fn write(&mut self, addr: u32, value: u8) {
        match addr {
            a if a < 0x20 && a % 2 == 0 => {
                let w = &mut self.halftone[(a / 2) as usize];
                *w = (*w & 0x00FF) | ((value as u16) << 8);
            }
            a if a < 0x20 => {
                let w = &mut self.halftone[(a / 2) as usize];
                *w = (*w & 0xFF00) | value as u16;
            }
            reg::SRC_X_INC => {
                self.src_x_inc =
                    ((((self.src_x_inc as u16) & 0x00FF) | ((value as u16) << 8)) & 0xFFFE) as i16
            }
            reg::SRC_X_INC1 => {
                self.src_x_inc =
                    ((((self.src_x_inc as u16) & 0xFF00) | value as u16) & 0xFFFE) as i16
            }
            reg::SRC_Y_INC => {
                self.src_y_inc =
                    ((((self.src_y_inc as u16) & 0x00FF) | ((value as u16) << 8)) & 0xFFFE) as i16
            }
            reg::SRC_Y_INC1 => {
                self.src_y_inc =
                    ((((self.src_y_inc as u16) & 0xFF00) | value as u16) & 0xFFFE) as i16
            }
            reg::SRC_ADDR => self.src_addr = (self.src_addr & 0x00FF_FFFF) | ((value as u32) << 24),
            reg::SRC_ADDR1 => {
                self.src_addr = (self.src_addr & 0xFF00_FFFF) | ((value as u32) << 16)
            }
            reg::SRC_ADDR2 => self.src_addr = (self.src_addr & 0xFFFF_00FF) | ((value as u32) << 8),
            // Bit 0 forcé à zéro (registre câblé mot uniquement sur le
            // silicium réel — même contrainte que `write_long` applique
            // déjà à l'écriture `.L` complète, et que les registres
            // d'incrément appliquent déjà à leur propre octet bas
            // ci-dessous) : sans ce masquage, une écriture octet isolée du
            // bas de SRC_ADDR pouvait laisser une adresse impaire, désalignant
            // la lecture des mots et mélangeant les plans de bits entrelacés.
            reg::SRC_ADDR3 => self.src_addr = (self.src_addr & 0xFFFF_FF00) | (value as u32 & 0xFE),
            reg::ENDMASK_1 => self.endmask[0] = (self.endmask[0] & 0x00FF) | ((value as u16) << 8),
            reg::ENDMASK_11 => self.endmask[0] = (self.endmask[0] & 0xFF00) | value as u16,
            reg::ENDMASK_2 => self.endmask[1] = (self.endmask[1] & 0x00FF) | ((value as u16) << 8),
            reg::ENDMASK_21 => self.endmask[1] = (self.endmask[1] & 0xFF00) | value as u16,
            reg::ENDMASK_3 => self.endmask[2] = (self.endmask[2] & 0x00FF) | ((value as u16) << 8),
            reg::ENDMASK_31 => self.endmask[2] = (self.endmask[2] & 0xFF00) | value as u16,
            reg::DST_X_INC => {
                self.dst_x_inc =
                    ((((self.dst_x_inc as u16) & 0x00FF) | ((value as u16) << 8)) & 0xFFFE) as i16
            }
            reg::DST_X_INC1 => {
                self.dst_x_inc =
                    ((((self.dst_x_inc as u16) & 0xFF00) | value as u16) & 0xFFFE) as i16
            }
            reg::DST_Y_INC => {
                self.dst_y_inc =
                    ((((self.dst_y_inc as u16) & 0x00FF) | ((value as u16) << 8)) & 0xFFFE) as i16
            }
            reg::DST_Y_INC1 => {
                self.dst_y_inc =
                    ((((self.dst_y_inc as u16) & 0xFF00) | value as u16) & 0xFFFE) as i16
            }
            reg::DST_ADDR => self.dst_addr = (self.dst_addr & 0x00FF_FFFF) | ((value as u32) << 24),
            reg::DST_ADDR1 => {
                self.dst_addr = (self.dst_addr & 0xFF00_FFFF) | ((value as u32) << 16)
            }
            reg::DST_ADDR2 => self.dst_addr = (self.dst_addr & 0xFFFF_00FF) | ((value as u32) << 8),
            // Bit 0 forcé à zéro — voir le commentaire équivalent sur
            // `SRC_ADDR3` juste au-dessus.
            reg::DST_ADDR3 => self.dst_addr = (self.dst_addr & 0xFFFF_FF00) | (value as u32 & 0xFE),
            reg::X_COUNT => self.x_count = (self.x_count & 0x00FF) | ((value as u32) << 8),
            reg::X_COUNT1 => self.x_count = (self.x_count & 0xFF00) | value as u32,
            reg::Y_COUNT => {
                if std::env::var("RUST68_TRACE_BLIT_REGS").is_ok() {
                    eprintln!("[blit-reg] pc={:#010x} write Y_COUNT(hi)={value:#04x} (skew_actuel={:#04x} dst_addr_actuel={:#010x})", DEBUG_LAST_PC.load(std::sync::atomic::Ordering::Relaxed), self.skew, self.dst_addr);
                }
                self.y_count = (self.y_count & 0x00FF) | ((value as u32) << 8);
                self.armed = true;
            }
            reg::Y_COUNT1 => {
                if std::env::var("RUST68_TRACE_BLIT_REGS").is_ok() {
                    eprintln!("[blit-reg] pc={:#010x} write Y_COUNT(lo)={value:#04x} (skew_actuel={:#04x} dst_addr_actuel={:#010x})", DEBUG_LAST_PC.load(std::sync::atomic::Ordering::Relaxed), self.skew, self.dst_addr);
                }
                self.y_count = (self.y_count & 0xFF00) | value as u32;
                self.armed = true;
            }
            reg::HOP => self.hop = value & 0x03,
            reg::OP => self.op = value & 0x0F,
            reg::SKEW => {
                if std::env::var("RUST68_TRACE_BLIT_REGS").is_ok() {
                    eprintln!("[blit-reg] pc={:#010x} write SKEW={value:#04x}", DEBUG_LAST_PC.load(std::sync::atomic::Ordering::Relaxed));
                }
                self.skew = value;
            }
            reg::CONTROL => {
                if std::env::var("RUST68_TRACE_BLIT_REGS").is_ok() {
                    eprintln!("[blit-reg] pc={:#010x} write CONTROL={value:#04x} (skew_actuel={:#04x})", DEBUG_LAST_PC.load(std::sync::atomic::Ordering::Relaxed), self.skew);
                }
                self.write_control(value);
            }
            _ => {}
        }
    }

    /// Écrit un registre 16 bits par mot complet — chemin emprunté par le
    /// board pour tout accès `.W` réel du CPU sur SRC_X_INC/SRC_Y_INC/
    /// ENDMASK1-3/DST_X_INC/DST_Y_INC/X_COUNT/Y_COUNT (voir la doc de
    /// [`Self::write`] : un accès `.B` isolé sur ces registres est ignoré
    /// sur le silicium réel, seul cet accès mot complet est honoré).
    ///
    /// X_COUNT/Y_COUNT : le manuel Blitter officiel et Hatari documentent 0
    /// comme désignant 65536 — mais TROIS tentatives indépendantes
    /// d'implémenter cette règle (deux directement dans `execute` sur un
    /// champ 16 bits, puis une à l'écriture avec un stockage 32 bits
    /// correctement dimensionné) ont chacune aggravé nettement la
    /// corruption observée en pratique sur ce TOS/ce cas d'usage précis —
    /// revenu à la valeur écrite telle quelle (sans conversion) en
    /// attendant de localiser la vraie cause amont.
    pub fn write_word(&mut self, addr: u32, value: u16) {
        match addr {
            a if a < 0x20 && a % 2 == 0 => self.halftone[(a / 2) as usize] = value,
            reg::SRC_X_INC => self.src_x_inc = (value & 0xFFFE) as i16,
            reg::SRC_Y_INC => self.src_y_inc = (value & 0xFFFE) as i16,
            reg::DST_X_INC => self.dst_x_inc = (value & 0xFFFE) as i16,
            reg::DST_Y_INC => self.dst_y_inc = (value & 0xFFFE) as i16,
            reg::ENDMASK_1 => self.endmask[0] = value,
            reg::ENDMASK_2 => self.endmask[1] = value,
            reg::ENDMASK_3 => self.endmask[2] = value,
            reg::X_COUNT => self.x_count = value as u32,
            reg::Y_COUNT => {
                if std::env::var("RUST68_TRACE_BLIT_REGS").is_ok() {
                    eprintln!("[blit-reg] pc={:#010x} write_word Y_COUNT={value:#06x} (skew_actuel={:#04x} dst_addr_actuel={:#010x})", DEBUG_LAST_PC.load(std::sync::atomic::Ordering::Relaxed), self.skew, self.dst_addr);
                }
                self.y_count = value as u32;
                self.armed = true;
            }
            _ => {}
        }
    }

    /// Écrit le registre CONTROL en tenant compte de l'"armement" du
    /// Blitter — voir [`Self::execute`] pour le détail du bug réel que
    /// cette logique corrige (redémarrages accidentels via `TAS.B` dans la
    /// boucle de relance du mode non-HOG).
    ///
    /// Sur le silicium réel (manuel Blitter officiel, section sur le mode
    /// partagé CPU/Blitter) : "If the BUSY flag is reset when the Y_Count
    /// is zero, the flag will remain clear indicating BLiTTER completion
    /// and the BLiTTER won't be restarted." — tant que le logiciel n'a pas
    /// explicitement réécrit Y_COUNT depuis la dernière exécution complète,
    /// toute tentative de poser le bit BUSY (y compris via `TAS.B`, utilisé
    /// par TOS pour relancer le Blitter tranche par tranche en mode
    /// partagé) est sans effet — le bit reste lisible à 0. Sans cette
    /// protection, chaque itération de la boucle de relance ré-exécutait
    /// un blit COMPLET depuis les adresses déjà avancées par le tour
    /// précédent, écrivant du contenu erroné bien au-delà de la zone
    /// prévue — cause réelle, une fois isolée par comparaison directe
    /// Blitter activé/désactivé, de la corruption visuelle observée
    /// (motif "déjà là" avant même l'application d'un blit de teinte, par
    /// exemple).
    fn write_control(&mut self, value: u8) {
        if value & CONTROL_BUSY != 0 && !self.armed && !self.mid_blit {
            self.control = (self.control & CONTROL_BUSY) | (value & !CONTROL_BUSY);
        } else {
            self.control = value;
        }
    }

    /// Écrit SRC_ADDR ou DST_ADDR par mot long complet (32 bits, seuls les
    /// 24 bits bas sont significatifs) — même principe que
    /// [`Self::write_word`] pour les registres 16 bits : un accès `.B` ou
    /// `.W` isolé sur ces registres est ignoré sur le silicium réel, seul
    /// un accès `.L` complet est honoré.
    pub fn write_long(&mut self, addr: u32, value: u32) {
        if std::env::var("RUST68_TRACE_BLIT_REGS").is_ok()
            && matches!(addr, reg::SRC_ADDR | reg::DST_ADDR)
        {
            eprintln!(
                "[blit-reg] pc={:#010x} write_long {}={:#010x} control_actuel={:#04x} busy={} mid_blit={} armed={}",
                DEBUG_LAST_PC.load(std::sync::atomic::Ordering::Relaxed),
                if addr == reg::SRC_ADDR { "SRC_ADDR" } else { "DST_ADDR" },
                value, self.control, self.busy(), self.mid_blit, self.armed,
            );
        }
        match addr {
            reg::SRC_ADDR => self.src_addr = value & 0x00FF_FFFE,
            reg::DST_ADDR => self.dst_addr = value & 0x00FF_FFFE,
            _ => {}
        }
    }

    /// Applique la fonction de demi-teinte (`HOP`, 2 bits) : combine le mot
    /// source et le mot de demi-teinte courant selon la table standard du
    /// datasheet (0=tous à 1, 1=demi-teinte seule, 2=source seule,
    /// 3=source ET demi-teinte).
    fn apply_hop(&self, source: u16, halftone: u16) -> u16 {
        match self.hop & 0x3 {
            0 => 0xFFFF,
            1 => halftone,
            2 => source,
            3 => source & halftone,
            _ => unreachable!(),
        }
    }

    /// Applique la fonction booléenne programmable (`OP`, 4 bits) : pour
    /// chaque position de bit, l'index `3 - ((s<<1)|d)` (c'est-à-dire
    /// `(NOT s << 1) | NOT d`) sélectionne le bit de sortie dans la table
    /// de vérité de 4 bits.
    ///
    /// Convention vérifiée par résolution directe du système d'équations
    /// posé par le manuel Blitter officiel (`User Manual for the Atari ST
    /// Bit-Block Transfer Processor`, archive.org, recoupé avec
    /// `BLITTER.TXT` — les deux donnent la même table) : OP=1 "source AND
    /// destination", OP=2 "source AND NOT destination", OP=4 "NOT source
    /// AND destination", OP=8 "NOT source AND NOT destination" ne sont
    /// simultanément satisfaisables qu'avec cet index inversé — l'index
    /// direct `(s<<1)|d` (convention Amiga/X11 "naturelle", utilisée par
    /// erreur ici auparavant) donne par ex. OP=3 = "NOT source" au lieu de
    /// "source", et OP=7 = NON(s ET d) au lieu de "source OR destination"
    /// — une confusion qui affecte le rendu de tout blit n'utilisant pas
    /// une des 4 fonctions symétriques (0x0/0x5/0xA/0xF).
    fn apply_op(&self, s_word: u16, d_word: u16) -> u16 {
        let mut result = 0u16;
        for bit in 0..16 {
            let s = (s_word >> bit) & 1;
            let d = (d_word >> bit) & 1;
            let index = 3 - ((s << 1) | d);
            let out = (self.op as u16 >> index) & 1;
            result |= out << bit;
        }
        result
    }

    /// Décale le mot source courant de `skew` bits en le combinant avec le
    /// mot précédent.
    ///
    /// Formule vérifiée contre un exemple chiffré concret de BLIT_FAQ.TXT
    /// (dépôt `ggnkua/Atari_ST_Sources`) : pour `SKEW=3` et un parcours X
    /// croissant, le Blitter "reads out bits 18..3" d'un tampon 32 bits où
    /// le mot COURANT occupe les bits 0-15 (bas) et le mot PRÉCÉDENT
    /// (recopié par le Blitter dans le tampon haut après chaque écriture)
    /// occupe les bits 16-31 (haut) — soit `((precedent as u32) << 16 |
    /// courant as u32) >> skew`, tronqué à 16 bits.
    ///
    /// Direction du parcours (confirmé par Hatari, `Blitter_SourceShift`/
    /// `Blitter_SourceFetch`) : ce tampon 32 bits est un registre à décalage
    /// alimenté DIFFÉREMMENT selon le signe de `SRC_X_INC`. Pour un parcours
    /// croissant (X_INC ≥ 0), le mot nouvellement lu va dans la moitié
    /// BASSE et l'ancien contenu remonte en HAUT (l'ordre "precedent:haut,
    /// courant:bas" ci-dessus). Pour un parcours DÉCROISSANT (X_INC < 0,
    /// blit "miroir"), c'est l'INVERSE : le mot nouvellement lu va dans la
    /// moitié HAUTE et l'ancien contenu (décalé) se retrouve en BAS — soit
    /// "courant:haut, precedent:bas". Le décalage à droite de `skew` reste
    /// identique dans les deux cas (même registre matériel), mais comme
    /// l'ordre des moitiés est inversé, le résultat diffère. Cette
    /// dépendance à la direction n'était pas modélisée dans une version
    /// précédente (les deux moitiés étaient toujours dans l'ordre "parcours
    /// croissant"). Un premier essai de ce correctif avait coïncidé avec
    /// l'apparition d'un bruit RVB massif en test live — mais la vraie
    /// cause s'est révélée être un bug d'armement du Blitter sans rapport
    /// (redémarrages accidentels via `TAS.B` dans la boucle de relance du
    /// mode non-HOG, voir [`Self::write_control`]), qui ré-exécutait des
    /// blits entiers depuis des adresses déjà avancées — une fois ce bug
    /// corrigé séparément, ce correctif de direction a pu être réappliqué
    /// sans regression.
    /// Décale le registre à décalage source 32 bits (`buffer`, voir
    /// [`Self::execute`]) : pour un parcours croissant (`src_x_inc >= 0`),
    /// l'ancien contenu remonte en HAUT (place nette en BAS pour le
    /// prochain mot lu) ; pour un parcours décroissant, c'est l'inverse.
    /// Traduction directe de `Blitter_SourceShift` (Hatari,
    /// `src/blitter.c`).
    fn shift_buffer(buffer: &mut u32, src_x_inc: i16) {
        if src_x_inc < 0 {
            *buffer >>= 16;
        } else {
            *buffer <<= 16;
        }
    }

    /// Charge `word` dans la moitié du registre à décalage source que
    /// [`Self::shift_buffer`] vient de libérer. Traduction directe de
    /// `Blitter_SourceFetch` (Hatari, `src/blitter.c`).
    fn fetch_buffer(buffer: &mut u32, src_x_inc: i16, word: u16) {
        if src_x_inc < 0 {
            *buffer |= (word as u32) << 16;
        } else {
            *buffer |= word as u32;
        }
    }

    /// Exécute le blit dans son intégralité (modèle synchrone, voir
    /// limitations du module), en utilisant `bus` pour lire/écrire la RAM
    /// aux adresses source/destination courantes. Met à jour les
    /// registres d'adresse source/destination en fin d'exécution ; efface
    /// le bit BUSY.
    ///
    /// Traite le blit **mot par mot** (pas ligne par ligne avec une
    /// formule d'avance d'adresse précalculée) — traduction directe de la
    /// machine à états de Hatari (`Blitter_ProcessWord`,
    /// `Blitter_SourceShift`/`Blitter_SourceFetch`), afin de reproduire
    /// fidèlement le registre à décalage source 32 bits (`buffer`
    /// ci-dessous) : sur le silicium réel, ce registre **persiste sur la
    /// durée ENTIÈRE du blit** (toutes les lignes), jamais remis à zéro
    /// entre deux lignes — seuls un décalage puis une lecture le
    /// modifient, à chaque mot lu (y compris l'amorçage FXSR). Une version
    /// précédente, structurée ligne par ligne avec une formule d'avance
    /// d'adresse batch, réinitialisait le mot "précédent" à 0 au début de
    /// CHAQUE ligne (sauf FXSR) et gérait NFSR comme un cas particulier
    /// local plutôt que la véritable suppression de lecture/avance
    /// qu'implique le silicium réel — confirmé faux par un test
    /// différentiel comparant exhaustivement notre sortie à un portage
    /// direct de `Blitter_ProcessWord` (`tests/blitter_hatari_diff.rs`) :
    /// pour un blit multi-lignes en direction négative avec SKEW=0 et
    /// X_COUNT=1, l'ancienne version produisait 0 au lieu du mot de la
    /// ligne précédente à chaque nouvelle ligne ; avec NFSR actif, elle
    /// divergeait aussi de l'avance d'adresse source réelle (qui saute
    /// entièrement l'avance de fin de ligne quand la dernière lecture est
    /// omise).
    ///
    /// **Armement** (voir [`Self::write_control`]) : ne fait RIEN si
    /// [`Self::armed`] est faux — c'est-à-dire si le logiciel n'a pas
    /// explicitement réécrit Y_COUNT depuis la dernière exécution complète.
    /// C'est ce qui empêche un déclenchement CONTROL accidentel (par ex.
    /// `TAS.B` dans la boucle de relance du mode non-HOG, qui pose
    /// physiquement le bit BUSY à chaque itération) de ré-exécuter tout le
    /// blit depuis des adresses déjà avancées par un tour précédent — voir
    /// le commentaire détaillé de [`Self::write_control`] pour le bug réel
    /// que ceci corrige.
    pub fn execute(&mut self, bus: &mut impl crate::Bus) {
        if self.armed {
            // Véritable démarrage (Y_COUNT vient d'être réécrit) : (ré)initialise
            // tout l'état de progression persistant. `x_count_reset` reste
            // borné à 1 (`.max(1)`) uniquement pour éviter une boucle
            // infinie plus bas, pas pour lui donner une signification
            // particulière.
            self.x_count = self.x_count.max(1);
            self.x_count_reset = self.x_count;
            // NE PAS remettre `self.buffer` à zéro ici : sur le silicium
            // réel (confirmé par Hatari ET Steem SSE — aucune des deux
            // références ne réinitialise jamais `BlitterVars.buffer`/
            // `Blitter.SrcBuffer` au démarrage d'un blit, uniquement via
            // les décalages/lectures normaux), le registre à décalage
            // source persiste sur la durée de vie de la puce, y compris
            // entre deux blits LOGIQUEMENT SÉPARÉS (Y_COUNT réécrit entre
            // les deux). TOS dessine typiquement une icône ou un glyphe
            // colonne par colonne via une SUITE de petits blits adjacents
            // (X_COUNT=1, SKEW non nul) qui s'appuient sur ce
            // chaînage pour reconstituer correctement les pixels à cheval
            // sur une frontière de mot — une remise à zéro systématique ici
            // clive alors chaque nouvelle colonne, cohérent avec la
            // corruption RVB observée sur les icônes glissées et le texte
            // des menus (uniquement les blits utilisant réellement la
            // source, jamais les remplissages/inversions purs qui n'en ont
            // pas besoin).
            self.bus_word = 0;
            self.have_fxsr = false;
            self.nfsr_dynamic = false;
            self.armed = false;
            self.mid_blit = true;
            self.control |= CONTROL_BUSY;
            if std::env::var("RUST68_TRACE_BLIT_START").is_ok() {
                eprintln!(
                    "[blit-start] pc={:#010x} dst_addr={:#010x} src_addr={:#010x} x_count={} y_count={} dst_x_inc={} dst_y_inc={} src_x_inc={} src_y_inc={} hop={} op={:#04x} skew={:#04x} control={:#04x} endmask={:#06x},{:#06x},{:#06x} buffer_avant={:#010x}",
                    DEBUG_LAST_PC.load(std::sync::atomic::Ordering::Relaxed),
                    self.dst_addr, self.src_addr, self.x_count, self.y_count,
                    self.dst_x_inc, self.dst_y_inc, self.src_x_inc, self.src_y_inc,
                    self.hop, self.op, self.skew, self.control,
                    self.endmask[0], self.endmask[1], self.endmask[2], self.buffer,
                );
            }
        } else if !self.mid_blit {
            return;
        }

        let x_count_reset = self.x_count_reset;
        let smudge = self.control & 0x20 != 0;
        let fxsr_reg = self.skew & 0x80 != 0;
        let nfsr_reg = self.skew & 0x40 != 0;
        let skew = (self.skew & 0x0F) as u32;
        // Mode HOG (bit 6 de CONTROL) : le Blitter garde le bus jusqu'à la
        // fin complète du blit, sans jamais rendre la main au CPU. En mode
        // non-HOG, le silicium réel ne traite qu'un nombre BORNÉ d'accès
        // bus RÉELS (lecture OU écriture, chacun compté séparément — pas un
        // nombre de MOTS traités) avant de rendre le bus — le logiciel doit
        // reposer BUSY (typiquement via `TAS.B`, qui relit/rendosse au
        // passage le numéro de ligne demi-teinte déjà avancé par le
        // matériel) pour faire progresser la suite. Confirmé nécessaire
        // par trace réelle Hatari (`--trace blitter`) sur ce cas précis :
        // une longue série d'écritures CONTROL avec BUSY posé et un numéro
        // de ligne qui progresse entre chaque écriture (pas une seule
        // écriture qui termine tout d'un coup), avec des centaines de
        // cycles d'instructions CPU authentiques entre deux écritures —
        // c'est-à-dire du VRAI travail CPU entrelacé avec la progression
        // du blit. Une version précédente exécutait tout le blit
        // instantanément dès le premier déclenchement CONTROL, empêchant
        // ce travail CPU entrelacé de s'exécuter dans le bon ordre relatif
        // à la progression réelle du blit — cohérent avec la corruption
        // observée dans le rendu de menus GEM (TOS 1.62/STE) qui persistait
        // malgré une vérification exhaustive, par ailleurs correcte, de
        // l'arithmétique interne du Blitter (table OP, HOP, skew, endmask,
        // avance d'adresse — voir `tests/blitter_hatari_diff.rs`).
        //
        // Le seuil de 64 accès bus (`BUS_ACCESSES_PER_SLICE` ci-dessous) et
        // le fait qu'il compte des accès réels et non des mots viennent
        // directement du source de Hatari (`src/blitter.c`,
        // `BLITTER_NONHOG_BUS_BLITTER`, avec un commentaire notant qu'un
        // silicium réel peut occasionnellement s'arrêter à 63 au lieu de 64
        // — cas de bug non reproduit ici, comme l'irrégularité de
        // rafraîchissement RAM déjà documentée ailleurs). Une version
        // précédente comptait 16 MOTS traités par tranche (pas d'accès bus)
        // — un mot "normal" avec lecture source ET écriture destination vaut
        // 3 accès bus réels (lecture source, lecture destination pour la
        // combinaison OP, écriture destination), donc l'ancienne tranche de
        // 16 mots représentait en réalité 32 à 48 accès bus selon le blit
        // (jamais 64) : la fréquence de rendu de main au CPU était
        // systématiquement trop élevée, cohérent avec un blit qui progresse
        // plus lentement (en cycles CPU réels) que sur silicium réel/Hatari.
        let hog = self.control & CONTROL_HOG != 0;
        const BUS_ACCESSES_PER_SLICE: u32 = 64;
        let mut bus_accesses_this_slice: u32 = 0;

        // `need_src` (repris de Hatari, `Blitter_Step`) : le pointeur
        // source n'avance QUE si l'opération lit effectivement la source —
        // c'est-à-dire si OP n'est pas l'une des 4 fonctions logiques qui
        // ignorent la source (0x0/0x5/0xA/0xF : constante 0, "destination",
        // "NOT destination", constante 1) ET si HOP produit une valeur
        // dépendant de la source (HOP=2/3, ou HOP=1 seulement en mode
        // SMUDGE, qui lit la source pour choisir la demi-teinte).
        let lop_needs_src = !matches!(self.op, 0x00 | 0x05 | 0x0A | 0x0F);
        let hop_needs_src = (self.hop & 0x02) != 0 || (self.hop == 1 && smudge);
        let need_src = lop_needs_src && hop_needs_src;

        let trace_words = std::env::var("RUST68_TRACE_BLITTER_WORDS").is_ok();
        let trace_slices = std::env::var("RUST68_TRACE_BLITTER_SLICES").is_ok();

        if trace_slices {
            eprintln!(
                "[slice] entree hog={hog} y_count={} x_count={} mid_blit_deja={}",
                self.y_count, self.x_count, self.mid_blit,
            );
        }
        while self.y_count > 0 {
            if !hog && bus_accesses_this_slice >= BUS_ACCESSES_PER_SLICE {
                if trace_slices {
                    eprintln!(
                        "[slice] pause budget epuise, y_count_restant={} x_count_restant={}",
                        self.y_count, self.x_count,
                    );
                }
                // Tranche épuisée : on rend la main au CPU sans effacer
                // BUSY — le prochain déclenchement CONTROL (typiquement
                // `TAS.B` dans la boucle de relance logicielle) reprendra
                // exactement où on s'est arrêté, via `mid_blit`.
                return;
            }

            let x_count = self.x_count;
            let first_word = x_count == x_count_reset;
            if first_word {
                self.nfsr_dynamic = false;
            }

            // Cas particulier d'une ligne d'un seul mot (`x_count_reset ==
            // 1`) : d'après le manuel Blitter officiel, ENDMASK_1 est
            // utilisé seul (pas de combinaison avec ENDMASK_3, qui est
            // simplement ignoré) — "In the case of a one word line
            // ENDMASK 1 is used."
            let mask = if first_word || x_count_reset == 1 {
                self.endmask[0]
            } else if x_count == 1 {
                self.endmask[2]
            } else {
                self.endmask[1]
            };

            // FXSR (lecture d'amorçage, une fois en début de ligne) : sur
            // le silicium réel, cette lecture a lieu à l'adresse COURANTE,
            // PUIS `src_addr` avance de SRC_X_INC avant même la première
            // lecture "normale" du mot 0 — le mot 0 est donc réellement lu
            // à `src_addr+SRC_X_INC`, pas à `src_addr`.
            if fxsr_reg && !self.have_fxsr && need_src {
                Self::shift_buffer(&mut self.buffer, self.src_x_inc);
                let w = bus.read16(self.src_addr & crate::ADDR_MASK);
                bus_accesses_this_slice += 1;
                self.bus_word = w;
                Self::fetch_buffer(&mut self.buffer, self.src_x_inc, w);
                self.src_addr = self.src_addr.wrapping_add(self.src_x_inc as i32 as u32);
                self.have_fxsr = true;
            }

            // Lecture source normale — omise si NFSR est actif ET que ce
            // mot est celui identifié comme "dernier" par le mécanisme
            // dynamique ci-dessous (`nfsr_dynamic`, posé quand X_COUNT==2
            // était vrai au mot précédent).
            let mut fetch_src = false;
            if need_src && !self.nfsr_dynamic {
                Self::shift_buffer(&mut self.buffer, self.src_x_inc);
                let w = bus.read16(self.src_addr & crate::ADDR_MASK);
                bus_accesses_this_slice += 1;
                self.bus_word = w;
                Self::fetch_buffer(&mut self.buffer, self.src_x_inc, w);
                fetch_src = true;
            }

            // Cas particulier NFSR : le silicium réel effectue un
            // décalage+relecture (réutilisant le dernier mot bus lu) avant
            // ET après le traitement du DERNIER mot de CHAQUE ligne — pas
            // seulement les lignes d'un seul mot. Confirmé dans le vrai
            // source Hatari (`Blitter_ProcessWord`, blitter.c) : la
            // condition y est `BlitterVars.nfsr && BlitterRegs.x_count ==
            // 1`, où `BlitterRegs.x_count` est le compteur COURANT (pas la
            // valeur initiale `x_count_reset`) — donc vrai à la fin de
            // chaque ligne, quelle que soit sa largeur.
            let weird_single_word_nfsr = nfsr_reg && x_count == 1;
            if weird_single_word_nfsr {
                Self::shift_buffer(&mut self.buffer, self.src_x_inc);
                Self::fetch_buffer(&mut self.buffer, self.src_x_inc, self.bus_word);
            }

            let source = (self.buffer >> skew) as u16;
            let halftone_line = self.control & 0x0F;
            let halftone_word = if smudge {
                self.halftone[(source & 0x0F) as usize]
            } else {
                self.halftone[halftone_line as usize]
            };

            let hop_result = self.apply_hop(source, halftone_word);
            let dest_current = bus.read16(self.dst_addr & crate::ADDR_MASK);
            bus_accesses_this_slice += 1;
            let mut result = self.apply_op(hop_result, dest_current);
            result = (result & mask) | (dest_current & !mask);

            if trace_words
                && ((source != 0x0000 && source != 0xFFFF)
                    || (self.hop == 1 && self.halftone[0] != self.halftone[1]))
            {
                eprintln!(
                    "[bw] src={:#08x} dst={:#08x} x_count={x_count} fxsr={fxsr_reg} nfsr={nfsr_reg} buffer={:#010x} skewed={source:#06x} halftone_line={halftone_line} halftone={halftone_word:#06x} hop={hop_result:#06x} dest_avant={dest_current:#06x} mask={mask:#06x} ecrit={result:#06x}",
                    self.src_addr & crate::ADDR_MASK,
                    self.dst_addr & crate::ADDR_MASK,
                    self.buffer,
                );
            }

            bus.write16(self.dst_addr & crate::ADDR_MASK, result);
            bus_accesses_this_slice += 1;

            if weird_single_word_nfsr {
                Self::shift_buffer(&mut self.buffer, self.src_x_inc);
                Self::fetch_buffer(&mut self.buffer, self.src_x_inc, self.bus_word);
            }

            // Le mot qui vient d'être traité est celui où X_COUNT==2 :
            // c'est le mot qui PRÉCÈDE le dernier de la ligne. Si NFSR est
            // actif, la lecture source du PROCHAIN mot (le dernier) doit
            // être omise — posé ici pour le prochain tour de boucle.
            if x_count == 2 && nfsr_reg {
                self.nfsr_dynamic = true;
            }

            // Avance de l'adresse source : uniquement si une lecture a eu
            // lieu ce mot-ci. Sur le DERNIER mot de la ligne (ou si la
            // lecture suivante sera omise par NFSR), l'avance utilise
            // SRC_Y_INC au lieu de SRC_X_INC — le logiciel appelant
            // configure donc SRC_Y_INC en connaissance de cause.
            if fetch_src {
                if x_count == 1 || self.nfsr_dynamic {
                    self.src_addr = self.src_addr.wrapping_add(self.src_y_inc as i32 as u32);
                } else {
                    self.src_addr = self.src_addr.wrapping_add(self.src_x_inc as i32 as u32);
                }
            }

            if x_count == 1 {
                // Fin de ligne : DST_Y_INC remplace DST_X_INC (pas en
                // plus) — le logiciel appelant précalcule donc Y_INC en
                // tenant déjà compte des (X_COUNT-1) pas de X_INC déjà
                // parcourus.
                self.have_fxsr = false;
                self.y_count -= 1;
                self.x_count = x_count_reset;
                self.dst_addr = self.dst_addr.wrapping_add(self.dst_y_inc as i32 as u32);
                let next_line = if self.dst_y_inc >= 0 {
                    (halftone_line + 1) & 0x0F
                } else {
                    halftone_line.wrapping_sub(1) & 0x0F
                };
                self.control = (self.control & 0xF0) | next_line;
            } else {
                self.x_count -= 1;
                self.dst_addr = self.dst_addr.wrapping_add(self.dst_x_inc as i32 as u32);
            }
        }

        // NOTE : ne PAS remettre `self.y_count`/`self.x_count` (registres
        // VISIBLES) à zéro ici — voir le commentaire de
        // [`Self::write_control`] pour la distinction entre ce registre
        // (qui doit revenir à sa valeur initiale, documentée) et
        // `self.armed` (qui, lui, passe à faux ci-dessous et empêche tout
        // redémarrage tant que le logiciel n'a pas explicitement réécrit
        // Y_COUNT).
        //
        // Le bit HOG (bit 6) est également effacé en fin de blit, pas
        // seulement BUSY — confirmé par Steem SSE (`blitter.cpp`,
        // `Blitter_Start_Line`, commentaire "hog bit also reset
        // (BLTBENCH.TOS)", citant un comportement observé sur silicium réel
        // via l'outil de test BLTBENCH.TOS). Une version précédente ne
        // touchait pas ce bit, laissant HOG visible à `1` après un blit en
        // mode HOG même une fois terminé — un logiciel qui relit CONTROL
        // pour décider de son comportement pour le blit SUIVANT (dans une
        // longue séquence d'appels, comme le dessin d'un menu GEM) pouvait
        // donc prendre un chemin différent de celui du matériel réel.
        self.control &= !(CONTROL_BUSY | CONTROL_HOG);
        self.armed = false;
        self.mid_blit = false;
    }
}
