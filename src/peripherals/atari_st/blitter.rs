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
//! - **`skew` (décalage bit à bit du mot source)** : la formule de
//!   combinaison (mot précédent + mot courant, fenêtre de 16 bits décalée
//!   de `skew` positions) suppose un parcours X croissant ; Hatari inverse
//!   l'ordre de combinaison quand `SRC_X_INC` est négatif (blit
//!   "miroir"/décroissant), ce qui n'est **pas** modélisé ici — non
//!   vérifié indépendamment, aucune suite de test équivalente à TomHarte
//!   n'existe pour le Blitter.
//! - Pas de vol de cycles bus au CPU modélisé (mode "hog"/"steal" du bit
//!   de contrôle) : le blit s'exécute intégralement de façon synchrone,
//!   indépendamment de ce bit.

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
    x_count: u16,
    y_count: u16,
    hop: u8,
    op: u8,
    skew: u8,
    /// Bit 7 = BUSY, bit 6 = HOG, bit 5 = SMUDGE, bits 3-0 = numéro de
    /// ligne de demi-teinte courant (voir [`reg::CONTROL`]).
    control: u8,
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
            reg::X_COUNT => (self.x_count >> 8) as u8,
            reg::X_COUNT1 => self.x_count as u8,
            reg::Y_COUNT => (self.y_count >> 8) as u8,
            reg::Y_COUNT1 => self.y_count as u8,
            reg::HOP => self.hop,
            reg::OP => self.op,
            reg::SKEW => self.skew,
            reg::CONTROL => self.control,
            _ => 0xFF,
        }
    }

    /// Écrit le registre à l'offset `addr`.
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
                self.src_x_inc = (((self.src_x_inc as u16) & 0x00FF) | ((value as u16) << 8)) as i16
            }
            reg::SRC_X_INC1 => {
                self.src_x_inc = (((self.src_x_inc as u16) & 0xFF00) | value as u16) as i16
            }
            reg::SRC_Y_INC => {
                self.src_y_inc = (((self.src_y_inc as u16) & 0x00FF) | ((value as u16) << 8)) as i16
            }
            reg::SRC_Y_INC1 => {
                self.src_y_inc = (((self.src_y_inc as u16) & 0xFF00) | value as u16) as i16
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
                self.dst_x_inc = (((self.dst_x_inc as u16) & 0x00FF) | ((value as u16) << 8)) as i16
            }
            reg::DST_X_INC1 => {
                self.dst_x_inc = (((self.dst_x_inc as u16) & 0xFF00) | value as u16) as i16
            }
            reg::DST_Y_INC => {
                self.dst_y_inc = (((self.dst_y_inc as u16) & 0x00FF) | ((value as u16) << 8)) as i16
            }
            reg::DST_Y_INC1 => {
                self.dst_y_inc = (((self.dst_y_inc as u16) & 0xFF00) | value as u16) as i16
            }
            reg::DST_ADDR => self.dst_addr = (self.dst_addr & 0x00FF_FFFF) | ((value as u32) << 24),
            reg::DST_ADDR1 => {
                self.dst_addr = (self.dst_addr & 0xFF00_FFFF) | ((value as u32) << 16)
            }
            reg::DST_ADDR2 => self.dst_addr = (self.dst_addr & 0xFFFF_00FF) | ((value as u32) << 8),
            reg::DST_ADDR3 => self.dst_addr = (self.dst_addr & 0xFFFF_FF00) | value as u32,
            reg::X_COUNT => self.x_count = (self.x_count & 0x00FF) | ((value as u16) << 8),
            reg::X_COUNT1 => self.x_count = (self.x_count & 0xFF00) | value as u16,
            reg::Y_COUNT => self.y_count = (self.y_count & 0x00FF) | ((value as u16) << 8),
            reg::Y_COUNT1 => self.y_count = (self.y_count & 0xFF00) | value as u16,
            reg::HOP => self.hop = value & 0x03,
            reg::OP => self.op = value & 0x0F,
            reg::SKEW => self.skew = value,
            reg::CONTROL => self.control = value,
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
    /// chaque position de bit, l'index `(s<<1)|d` sélectionne le bit de
    /// sortie dans la table de vérité de 4 bits — convention standard
    /// partagée par de nombreuses puces "raster op" (Amiga, X11…), pas
    /// spécifique à l'Atari.
    fn apply_op(&self, s_word: u16, d_word: u16) -> u16 {
        let mut result = 0u16;
        for bit in 0..16 {
            let s = (s_word >> bit) & 1;
            let d = (d_word >> bit) & 1;
            let index = (s << 1) | d;
            let out = (self.op as u16 >> index) & 1;
            result |= out << bit;
        }
        result
    }

    /// Décale le mot source courant de `skew` bits en le combinant avec le
    /// mot précédent (voir limitations du module).
    fn skewed_source(&self, previous: u16, current: u16) -> u16 {
        let skew = (self.skew & 0x0F) as u32;
        if skew == 0 {
            return current;
        }
        let combined = ((current as u32) << 16) | previous as u32;
        (combined >> (16 - skew)) as u16
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
    pub fn execute(&mut self, bus: &mut impl crate::Bus) {
        self.control |= CONTROL_BUSY;

        let x_count = self.x_count.max(1);
        let y_count = self.y_count;
        let smudge = self.control & 0x20 != 0;
        let fxsr = self.skew & 0x80 != 0;
        let nfsr = self.skew & 0x40 != 0;

        for _ in 0..y_count {
            let halftone_line = self.control & 0x0F;
            let mut src = self.src_addr;
            let mut dst = self.dst_addr;
            let mut previous_source = if fxsr {
                bus.read16(src.wrapping_sub(self.src_x_inc as u32) & crate::ADDR_MASK)
            } else {
                0
            };

            for word_index in 0..x_count {
                let is_last_word = word_index == x_count - 1;
                let current_source = if is_last_word && nfsr {
                    0
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

            self.src_addr = self.src_addr.wrapping_add(self.src_y_inc as i32 as u32);
            self.dst_addr = self.dst_addr.wrapping_add(self.dst_y_inc as i32 as u32);

            let next_line = if self.dst_y_inc < 0 {
                halftone_line.wrapping_sub(1) & 0x0F
            } else {
                (halftone_line + 1) & 0x0F
            };
            self.control = (self.control & 0xF0) | next_line;
        }

        self.y_count = 0;
        self.control &= !CONTROL_BUSY;
    }
}
