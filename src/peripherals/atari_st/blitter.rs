//! Blitter — coprocesseur de transfert de blocs (BitBlt) de l'Atari STE.
//!
//! Combine un mot source (optionnellement décalé bit à bit via `skew` pour
//! aligner des images non alignées sur un mot), un motif de demi-teinte
//! (halftone), et le contenu destination actuel, via une fonction booléenne
//! programmable (`OP`, une des 16 fonctions à 2 entrées), avec masquage de
//! bord de ligne (`ENDMASK1/2/3`) et parcours par incréments X/Y.
//!
//! Ce module modélise la puce **seule** : [`Blitter::execute`] prend un
//! `Bus` (pour lire/écrire la RAM aux adresses source/destination) et
//! exécute le blit dans son intégralité en un seul appel (modèle
//! synchrone "instantané", comme le WD1772 — `BUSY` n'est jamais
//! observable par polling). C'est au board de mapper
//! [`Blitter::read`]/[`Blitter::write`] dans son `Bus` et de déclencher
//! `execute` quand le bit START du registre de contrôle est écrit.
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
//! - Pas de vol de cycles bus au CPU modélisé (mode "hog"/"steal" du bit
//!   de contrôle) : le blit s'exécute intégralement de façon synchrone,
//!   indépendamment de ce bit.
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
        }
    }

    /// Vrai si le bit BUSY du registre de contrôle est actif. Dans ce
    /// modèle synchrone, toujours faux juste après [`Self::execute`] (voir
    /// limitations du module) : exposé surtout pour cohérence avec le
    /// registre réel.
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
            reg::SRC_ADDR3 => self.src_addr = (self.src_addr & 0xFFFF_FF00) | value as u32,
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
            reg::DST_ADDR3 => self.dst_addr = (self.dst_addr & 0xFFFF_FF00) | value as u32,
            reg::X_COUNT => self.x_count = (self.x_count & 0x00FF) | ((value as u32) << 8),
            reg::X_COUNT1 => self.x_count = (self.x_count & 0xFF00) | value as u32,
            reg::Y_COUNT => {
                self.y_count = (self.y_count & 0x00FF) | ((value as u32) << 8);
                self.armed = true;
            }
            reg::Y_COUNT1 => {
                self.y_count = (self.y_count & 0xFF00) | value as u32;
                self.armed = true;
            }
            reg::HOP => self.hop = value & 0x03,
            reg::OP => self.op = value & 0x0F,
            reg::SKEW => self.skew = value,
            reg::CONTROL => self.write_control(value),
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
        if value & CONTROL_BUSY != 0 && !self.armed {
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
    fn skewed_source(&self, previous: u16, current: u16) -> u16 {
        let skew = (self.skew & 0x0F) as u32;
        let combined: u32 = if self.src_x_inc < 0 {
            ((current as u32) << 16) | previous as u32
        } else {
            ((previous as u32) << 16) | current as u32
        };
        (combined >> skew) as u16
    }

    /// Exécute le blit dans son intégralité (modèle synchrone, voir
    /// limitations du module), en utilisant `bus` pour lire/écrire la RAM
    /// aux adresses source/destination courantes. Met à jour les
    /// registres d'adresse source/destination et le compteur Y à zéro en
    /// fin d'exécution ; efface le bit BUSY.
    ///
    /// `FXSR` (lecture d'amorçage en début de ligne) et `NFSR` (dernière
    /// lecture source de la ligne omise) sont honorés d'après le bit du
    /// registre [`reg::SKEW`] plutôt que déduits de `skew != 0`. En mode
    /// `SMUDGE`, le mot de demi-teinte utilisé pour chaque mot vient des 4
    /// bits bas du mot source décalé (`skewed_source`) plutôt que du
    /// numéro de ligne courant ; sinon, le numéro de ligne (bits 0-3 de
    /// [`reg::CONTROL`]) avance ou recule à la fin de chaque ligne selon
    /// le signe de `DST_Y_INC`.
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
        if !self.armed {
            return;
        }
        self.control |= CONTROL_BUSY;

        // X_COUNT/Y_COUNT : le manuel Blitter officiel documente 0 comme
        // désignant 65536 — mais cette conversion a désormais lieu à
        // l'ÉCRITURE du registre (voir `write_word_count`), pas ici. Deux
        // tentatives précédentes de l'appliquer directement ici (sur un
        // champ 16 bits, via `.max(1)`) confondaient "0 explicitement écrit
        // `y_count == 0` fait légitimement 0 tour de boucle externe (blit
        // sans effet). `x_count` reste borné à 1 (`.max(1)`) uniquement pour
        // éviter un dépassement arithmétique dans `(x_count - 1)` plus bas
        // (avance de fin de ligne), pas pour lui donner une signification
        // particulière.
        let x_count = self.x_count.max(1);
        let y_count = self.y_count;
        let smudge = self.control & 0x20 != 0;
        let fxsr = self.skew & 0x80 != 0;
        let nfsr = self.skew & 0x40 != 0;

        // `need_src` (repris de Hatari, `Blitter_Step`) : le pointeur
        // source n'avance en fin de ligne QUE si l'opération lit
        // effectivement la source — c'est-à-dire si OP n'est pas l'une des
        // 4 fonctions logiques qui ignorent la source (0x0/0x5/0xA/0xF :
        // constante 0, "destination", "NOT destination", constante 1) ET
        // si HOP produit une valeur dépendant de la source (HOP=2/3, ou
        // HOP=1 seulement en mode SMUDGE, qui lit la source pour choisir la
        // demi-teinte).
        let lop_needs_src = !matches!(self.op, 0x00 | 0x05 | 0x0A | 0x0F);
        let hop_needs_src = (self.hop & 0x02) != 0 || (self.hop == 1 && smudge);
        let need_src = lop_needs_src && hop_needs_src;

        for _ in 0..y_count {
            let halftone_line = self.control & 0x0F;
            let mut src = self.src_addr;
            let mut dst = self.dst_addr;
            // FXSR (lecture d'amorçage) : sur le silicium réel (confirmé
            // par Hatari, `Blitter_ProcessWord`), cette lecture a lieu à
            // l'adresse COURANTE (`src`, celle configurée par le logiciel
            // comme point de départ de la ligne), PUIS `src` avance de
            // SRC_X_INC avant même la première lecture "normale" du mot 0 —
            // le mot 0 est donc réellement lu à `src+SRC_X_INC`, pas à
            // `src`. Une version précédente lisait l'amorçage à
            // `src-SRC_X_INC` en laissant `src` inchangé pour le mot 0,
            // inversant l'ordre réel et décalant d'un mot entier TOUTE la
            // ligne dès que FXSR est actif — cohérent avec la corruption
            // toujours observée dans la barre de menu (texte/icônes GEM,
            // qui posent FXSR dès que SKEW≠0).
            let mut previous_source = if fxsr {
                let primed = bus.read16(src & crate::ADDR_MASK);
                src = src.wrapping_add(self.src_x_inc as i32 as u32);
                primed
            } else {
                0
            };

            for word_index in 0..x_count {
                let is_last_word = word_index == x_count - 1;
                // NFSR (No Final Source Read) : sur le dernier mot de la
                // ligne, aucune nouvelle lecture bus n'a lieu — le registre
                // tampon source conserve la dernière valeur qui y a été
                // chargée. Une précédente version forçait cette valeur à 0,
                // ce qui était faux (confirmé par Hatari, référence de
                // l'émulateur : `Blitter_SourceFetch(true)` réutilise
                // `bus_word`, la dernière valeur lue, au lieu de lire la
                // mémoire ou d'utiliser 0) : `previous_source` contient déjà
                // exactement cette dernière valeur lue (amorcée par FXSR ou
                // par le mot précédent de la ligne).
                let current_source = if is_last_word && nfsr {
                    previous_source
                } else {
                    bus.read16(src & crate::ADDR_MASK)
                };
                let source = self.skewed_source(previous_source, current_source);
                previous_source = current_source;

                let halftone_word = if smudge {
                    self.halftone[(source & 0x0F) as usize]
                } else {
                    self.halftone[halftone_line as usize]
                };

                let hop_result = self.apply_hop(source, halftone_word);
                let dest_current = bus.read16(dst & crate::ADDR_MASK);
                let mut result = self.apply_op(hop_result, dest_current);

                // Cas particulier d'une ligne d'un seul mot (`x_count ==
                // 1`) : d'après le manuel Blitter officiel, ENDMASK_1 est
                // utilisé seul (pas de combinaison avec ENDMASK_3, qui est
                // simplement ignoré) — "In the case of a one word line
                // ENDMASK 1 is used." C'est au logiciel appelant de
                // précalculer la valeur combinée souhaitée et de l'écrire
                // dans ENDMASK_1 avant de déclencher le blit ; une
                // précédente tentative de combiner ENDMASK_1 et ENDMASK_3
                // par ET ici (en lisant la doc comme "les deux masques
                // fusionnent") mettait silencieusement à zéro des blits
                // valides dès qu'ENDMASK_3 valait 0 — un cas très fréquent
                // en pratique (observé sur les petits blits du curseur
                // souris), confirmant que cette lecture était erronée.
                let mask = if word_index == 0 {
                    self.endmask[0]
                } else if is_last_word {
                    self.endmask[2]
                } else {
                    self.endmask[1]
                };
                result = (result & mask) | (dest_current & !mask);

                bus.write16(dst & crate::ADDR_MASK, result);

                src = src.wrapping_add(self.src_x_inc as i32 as u32);
                dst = dst.wrapping_add(self.dst_x_inc as i32 as u32);
            }

            // Avance de fin de ligne : sur le silicium réel (confirmé par
            // Hatari, `Blitter_Step`), le pointeur n'est incrémenté de
            // X_INC qu'entre les mots ; pour le DERNIER mot de la ligne,
            // c'est Y_INC qui est appliqué À LA PLACE de X_INC (pas en
            // plus) — le logiciel appelant précalcule donc Y_INC en
            // tenant déjà compte des (X_COUNT-1) pas de X_INC déjà
            // parcourus. Le code précédent ajoutait Y_INC seul à l'adresse
            // de DÉBUT de ligne, perdant entièrement la contribution des
            // (X_COUNT-1) pas de X_INC — correct par accident pour les
            // lignes d'un seul mot (X_COUNT=1, ex. curseur souris) mais
            // faux dès que X_COUNT>1 (texte/icônes GEM), cohérent avec la
            // corruption observée qui épargnait le curseur mais touchait
            // le texte/les icônes.
            let src_x_steps = (x_count - 1) as i32 * self.src_x_inc as i32;
            let dst_x_steps = (x_count - 1) as i32 * self.dst_x_inc as i32;
            if need_src {
                // FXSR consomme un pas SRC_X_INC supplémentaire (voir plus
                // haut) qui doit se retrouver dans l'adresse de fin de
                // ligne, en plus des (x_count-1) pas normaux.
                let fxsr_step = if fxsr { self.src_x_inc as i32 } else { 0 };
                self.src_addr = self
                    .src_addr
                    .wrapping_add(src_x_steps as u32)
                    .wrapping_add(fxsr_step as u32)
                    .wrapping_add(self.src_y_inc as i32 as u32);
            }
            self.dst_addr = self
                .dst_addr
                .wrapping_add(dst_x_steps as u32)
                .wrapping_add(self.dst_y_inc as i32 as u32);

            let next_line = if self.dst_y_inc < 0 {
                halftone_line.wrapping_sub(1) & 0x0F
            } else {
                (halftone_line + 1) & 0x0F
            };
            self.control = (self.control & 0xF0) | next_line;
        }

        // NOTE : ne PAS remettre `self.y_count`/`self.x_count` (registres
        // VISIBLES) à zéro ici — voir le commentaire de
        // [`Self::write_control`] pour la distinction entre ce registre
        // (qui doit revenir à sa valeur initiale, documentée) et
        // `self.armed` (qui, lui, passe à faux ci-dessous et empêche tout
        // redémarrage tant que le logiciel n'a pas explicitement réécrit
        // Y_COUNT).
        self.control &= !CONTROL_BUSY;
        self.armed = false;
    }
}
