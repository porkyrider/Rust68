//! Microwire — interface série vers le mixeur externe LMC1992 (STE), qui
//! contrôle en aval le volume maître, le volume gauche/droite, la balance
//! graves/aigus et le mode de mixage, sur le **signal de sortie final**
//! (PSG *et* son DMA mélangés) — pas un registre de la puce DMA Sound
//! elle-même, mais un circuit séparé piloté par les mêmes deux registres
//! série (`$FF8922` DATA, `$FF8924` MASK).
//!
//! Seuls le volume maître et le volume gauche/droite sont modélisés ici
//! (facteur de gain appliqué en aval) : graves/aigus nécessiteraient un
//! filtre IIR pour un rendu fidèle et le mode de mixage n'a pas d'effet
//! audible avec une seule source de sortie hôte — raffinements non
//! demandés à ce stade (même limitation assumée que le filtrage de
//! tonalité dans `dma_sound.rs`).
//!
//! Décodage du mot de commande : reproduit l'algorithme du LMC1992 tel que
//! vérifié dans Hatari (`dmaSnd.c`, `DmaSnd_InterruptHandler_Microwire`) —
//! transmission série MSB-first, un préfixe d'adresse `10` (2 bits) suivi
//! d'un sélecteur de commande (3 bits) puis d'une valeur, le tout repéré en
//! ne gardant que les bits de DATA dont le bit MASK correspondant est à 1.
//! On saute la temporisation série réelle (16 décalages à 1 MHz) et on
//! décode instantanément dès que MASK et DATA ont chacun été écrits en
//! entier : sans conséquence fonctionnelle, puisque le logiciel de toute
//! façon boucle en relisant DATA jusqu'à ce qu'il retombe à zéro (déjà le
//! cas immédiatement, voir le commentaire dédié dans
//! `AtariSt::read8`) avant de continuer.

/// `(int)(powf(10.0, dB/20.0) * 65536.0 + 0.5)`, pas de 2dB — table du
/// volume maître (6 bits), reprise telle quelle de Hatari (`dmaSnd.c`,
/// `LMC1992_Master_Volume_Table`) : whatever la commande, `65535` représente
/// un gain unité (0dB, pas d'atténuation).
const MASTER_VOLUME_TABLE: [u16; 64] = [
    7, 8, 10, 13, 16, 21, 26, 33, 41, 52, // -80dB
    66, 83, 104, 131, 165, 207, 261, 328, 414, 521, // -60dB
    655, 825, 1039, 1308, 1646, 2072, 2609, 3285, 4135, 5206, // -40dB
    6554, 8250, 10387, 13076, 16462, 20724, 26090, 32846, 41350, 52057, // -20dB
    65535, 65535, 65535, 65535, 65535, 65535, 65535, 65535, 65535, 65535, // 0dB
    65535, 65535, 65535, 65535, 65535, 65535, 65535, 65535, 65535, 65535, // 0dB
    65535, 65535, 65535, 65535, // 0dB
];

/// Table du volume gauche/droite (5 bits) — reprise de Hatari
/// (`LMC1992_LeftRight_Volume_Table`).
const LEFT_RIGHT_VOLUME_TABLE: [u16; 32] = [
    655, 825, 1039, 1308, 1646, 2072, 2609, 3285, 4135, 5206, // -40dB
    6554, 8250, 10387, 13076, 16462, 20724, 26090, 32846, 41350, 52057, // -20dB
    65535, 65535, 65535, 65535, 65535, 65535, 65535, 65535, 65535, 65535, // 0dB
    65535, 65535, // 0dB
];

/// État complet du circuit Microwire/LMC1992.
#[derive(Debug, Clone)]
pub struct Microwire {
    mask: u16,
    data: u16,
    master_volume: u16,
    left_volume: u16,
    right_volume: u16,
}

impl Microwire {
    pub fn new() -> Self {
        // Volume plein par défaut (silicium réel : pas de signal de reset
        // sur le Microwire lui-même, mais TOS programme systématiquement un
        // volume raisonnable très tôt au boot ; démarrer atténué rendrait le
        // son totalement silencieux tant qu'aucune commande n'a encore été
        // envoyée).
        Microwire { mask: 0, data: 0, master_volume: 65535, left_volume: 65535, right_volume: 65535 }
    }

    pub fn write_mask_high(&mut self, value: u8) {
        self.mask = (self.mask & 0x00FF) | ((value as u16) << 8);
    }

    pub fn write_mask_low(&mut self, value: u8) {
        self.mask = (self.mask & 0xFF00) | value as u16;
    }

    pub fn write_data_high(&mut self, value: u8) {
        self.data = (self.data & 0x00FF) | ((value as u16) << 8);
    }

    /// Octet bas de DATA : dernier des 4 octets écrits dans la séquence
    /// réelle (MASK haut/bas puis DATA haut/bas) — décode la commande ici.
    pub fn write_data_low(&mut self, value: u8) {
        self.data = (self.data & 0xFF00) | value as u16;
        self.decode();
    }

    fn decode(&mut self) {
        let mut i: i32 = 15;
        while i >= 0 {
            if self.mask & (1 << i) == 0 {
                i -= 1;
                continue;
            }
            let mut cmd: u16 = 0;
            let mut cmd_len: u32 = 0;
            while i >= 0 && self.mask & (1 << i) != 0 {
                cmd <<= 1;
                cmd_len += 1;
                if self.data & (1 << i) != 0 {
                    cmd |= 1;
                }
                i -= 1;
            }
            if cmd_len >= 11 && (cmd >> (cmd_len - 2)) & 0x3 == 0x2 {
                self.apply(cmd);
                return;
            }
            // Commande invalide (mauvais préfixe d'adresse ou trop courte) :
            // on continue de scruter le reste du masque, comme le silicium
            // réel (voir la doc du module).
        }
    }

    fn apply(&mut self, cmd: u16) {
        match (cmd >> 6) & 0x7 {
            3 => self.master_volume = MASTER_VOLUME_TABLE[(cmd & 0x3F) as usize],
            4 => self.right_volume = LEFT_RIGHT_VOLUME_TABLE[(cmd & 0x1F) as usize],
            5 => self.left_volume = LEFT_RIGHT_VOLUME_TABLE[(cmd & 0x1F) as usize],
            // Mixage (0) et graves/aigus (1/2) : non modélisés, voir la doc
            // du module.
            _ => {}
        }
    }

    /// Gain (0.0-1.0) du canal gauche : volume gauche × volume maître —
    /// appliqué séparément du canal droit ([`Self::right_gain`]) puisque la
    /// cartouche de diagnostic usine STe (test "Stereo 1 kHz/500 Hz tones")
    /// programme délibérément des volumes gauche/droite différents et
    /// changeants pour faire entendre 2 tons à des niveaux distincts.
    pub fn left_gain(&self) -> f32 {
        self.left_volume as f32 / 65535.0 * (self.master_volume as f32 / 65535.0)
    }

    /// Gain (0.0-1.0) du canal droit — voir [`Self::left_gain`].
    pub fn right_gain(&self) -> f32 {
        self.right_volume as f32 / 65535.0 * (self.master_volume as f32 / 65535.0)
    }
}

impl Default for Microwire {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn send_command(mw: &mut Microwire, cmd11: u16) {
        mw.write_mask_high(0x07);
        mw.write_mask_low(0xFF);
        mw.write_data_high(((cmd11 >> 8) & 0xFF) as u8);
        mw.write_data_low((cmd11 & 0xFF) as u8);
    }

    #[test]
    fn volume_plein_par_defaut() {
        let mw = Microwire::new();
        assert!((mw.left_gain() - 1.0).abs() < 0.001);
        assert!((mw.right_gain() - 1.0).abs() < 0.001);
    }

    #[test]
    fn commande_volume_maitre_attenue_les_deux_canaux() {
        let mut mw = Microwire::new();
        // Type=3 (master volume) << 6, valeur=0 (index le plus atténué,
        // -80dB) ; préfixe d'adresse "10" déjà inclus via le bit10=1/bit9=0.
        send_command(&mut mw, 0x400 | (3 << 6));
        assert!(mw.left_gain() < 0.01, "left_gain={} devrait etre tres attenue", mw.left_gain());
        assert!(mw.right_gain() < 0.01, "right_gain={} devrait etre tres attenue", mw.right_gain());
    }

    #[test]
    fn commande_volume_maitre_pleine_echelle_gain_unite() {
        let mut mw = Microwire::new();
        send_command(&mut mw, 0x400 | (3 << 6) | 0x3F);
        assert!((mw.left_gain() - 1.0).abs() < 0.001);
        assert!((mw.right_gain() - 1.0).abs() < 0.001);
    }

    #[test]
    fn commande_volume_gauche_droite_independante() {
        let mut mw = Microwire::new();
        // Volume droit au minimum, gauche inchangé (plein) : les deux
        // canaux doivent rester DISTINCTS, pas moyennés ensemble — c'est
        // précisément ce que le test "Stereo 1 kHz/500 Hz tones" de la
        // cartouche vérifie (2 tons à des volumes différents).
        send_command(&mut mw, 0x400 | (4 << 6));
        assert!((mw.left_gain() - 1.0).abs() < 0.001, "gauche inchangé");
        assert!(mw.right_gain() < 0.01, "droit doit etre tres attenue");
    }

    #[test]
    fn commande_avec_mauvais_prefixe_est_ignoree() {
        let mut mw = Microwire::new();
        // bit10=0 : préfixe invalide (devrait être "10"), la commande ne
        // doit pas être appliquée.
        send_command(&mut mw, 3 << 6);
        assert!((mw.left_gain() - 1.0).abs() < 0.001, "commande invalide ne doit rien changer");
    }
}
