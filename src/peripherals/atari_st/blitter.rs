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
//! ## Limitations connues (v1) — à prendre avec prudence
//! - **`skew` (décalage bit à bit du mot source)** : implémenté d'après ma
//!   meilleure compréhension du mécanisme documenté (combiner le mot
//!   précédent et le mot courant, extraire une fenêtre de 16 bits décalée
//!   de `skew` positions), mais **non vérifié contre une référence
//!   matérielle réelle ou un émulateur existant** — aucune suite de test
//!   équivalente à TomHarte n'existe pour le Blitter. À valider avant de
//!   s'appuyer dessus pour un défilement fin précis.
//! - `NFSR`/`FXSR` (contrôle fin de quand faire une lecture source
//!   supplémentaire d'amorçage) non modélisés distinctement : une lecture
//!   d'amorçage est toujours effectuée en début de ligne si `skew != 0`.
//! - Mode "dessin de ligne" (line number utilisé pour du tracé de
//!   polygone plutôt que du blit rectangulaire) non implémenté : seul le
//!   mode blit rectangulaire standard est couvert.
//! - Pas de vol de cycles bus au CPU modélisé (mode "hog"/"steal" du bit
//!   de contrôle) : le blit s'exécute intégralement de façon synchrone,
//!   indépendamment de ce bit.
//! - Halftone : le mot utilisé pour chaque ligne est `halftone[line % 16]`,
//!   `line` incrémentant à chaque ligne Y traitée — comportement usuel
//!   documenté pour un remplissage à motif, non vérifié indépendamment.

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
    /// Bits 0-3 = décalage (skew), bit 6 = FXSR, bit 7 = NFSR.
    pub const SKEW: u32 = 0x3C;
    /// Bit 7 = BUSY, bit 6 = HOG, bit 4 = SMUDGE.
    pub const CONTROL: u32 = 0x3D;
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
    control: u8,
    /// Compteur de ligne pour la sélection du mot de demi-teinte (voir
    /// limitations : cycle 0..16 à chaque ligne Y traitée).
    halftone_line: u8,
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
            halftone_line: 0,
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
    /// source et le mot de demi-teinte courant selon la table standard
    /// (0=zéro, 1=demi-teinte seule, 2=source seule, 3=source ET demi-teinte).
    fn apply_hop(&self, source: u16, halftone: u16) -> u16 {
        match self.hop & 0x3 {
            0 => 0,
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
    pub fn execute(&mut self, bus: &mut impl crate::Bus) {
        self.control |= CONTROL_BUSY;

        let x_count = self.x_count.max(1);
        let y_count = self.y_count;

        for _ in 0..y_count {
            let halftone_word = self.halftone[(self.halftone_line % 16) as usize];
            let mut src = self.src_addr;
            let mut dst = self.dst_addr;
            let mut previous_source = if self.skew & 0x0F != 0 {
                bus.read16(src.wrapping_sub(self.src_x_inc as u32) & crate::ADDR_MASK)
            } else {
                0
            };

            for word_index in 0..x_count {
                let current_source = bus.read16(src & crate::ADDR_MASK);
                let source = self.skewed_source(previous_source, current_source);
                previous_source = current_source;

                let hop_result = self.apply_hop(source, halftone_word);
                let dest_current = bus.read16(dst & crate::ADDR_MASK);
                let mut result = self.apply_op(hop_result, dest_current);

                let mask = if word_index == 0 {
                    self.endmask[0]
                } else if word_index == x_count - 1 {
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
            self.halftone_line = self.halftone_line.wrapping_add(1);
        }

        self.y_count = 0;
        self.control &= !CONTROL_BUSY;
    }
}
