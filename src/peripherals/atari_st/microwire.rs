//! Microwire — interface série vers le mixeur externe LMC1992 (STE), qui
//! contrôle en aval le volume maître, le volume gauche/droite, la balance
//! graves/aigus et le mode de mixage, sur le **signal de sortie final**
//! (PSG *et* son DMA mélangés) — pas un registre de la puce DMA Sound
//! elle-même, mais un circuit séparé piloté par les mêmes deux registres
//! série (`$FF8922` DATA, `$FF8924` MASK).
//!
//! Le volume maître, le volume gauche/droite ET le filtre graves/aigus sont
//! modélisés ([`Self::filter_left`]/[`Self::filter_right`]) ; le mode de
//! mixage n'a pas d'effet audible avec une seule source de sortie hôte —
//! seul celui-ci reste un raffinement non demandé à ce stade.
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
//!
//! ## Filtre graves/aigus (façon Hatari)
//! [`Self::filter_left`]/[`Self::filter_right`] appliquent gain ET filtre
//! graves/aigus en une seule passe par échantillon — reproduit fidèlement
//! de Hatari (`dmaSnd.c`, `DmaSnd_Bass_Shelf`/`DmaSnd_Treble_Shelf`/
//! `DmaSnd_Set_Tone_Level`/`DmaSnd_IIRfilterL`/`DmaSnd_IIRfilterR`) : deux
//! filtres "plateau" (shelf) du premier ordre — un pour les graves (coude à
//! 118.2763 Hz), un pour les aigus (coude à 8438.756 Hz), valeurs mesurées
//! sur le vrai circuit LMC1992 — combinés algébriquement en un seul filtre
//! biquad (2nd ordre), appliqué en forme directe II. 13 paliers de gain
//! (-12dB à +12dB par pas de 2dB) précalculés une fois pour la fréquence de
//! sortie FIXE de Rust68 (44100 Hz, voir `AUDIO_SAMPLE_RATE` dans
//! `atari_st_sdl2.rs`) — contrairement à Hatari, qui recalcule ces tables
//! si la fréquence de sortie hôte change en cours de route, Rust68 n'en a
//! qu'une seule possible.
//!
//! Le gain (volume) est un paramètre EXTERNE de `filter_left`/`filter_right`
//! (pas lu depuis [`Self::left_gain`]/[`Self::right_gain`] en interne) :
//! l'appelant reste libre de lisser les variations de volume dans le temps
//! (évite un "clic" de zipper noise sur un changement brutal) avant de
//! l'injecter dans le filtre — exactement ce que fait `atari_st_sdl2.rs`.
//! Ce filtre s'applique en aval du mixage PSG+DMA Sound (comme chez
//! Hatari, `DmaSnd_Apply_LMC`, appelé après le mixage et le rééchantillonnage
//! vers la fréquence hôte), pas par source individuelle.

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

/// Nombre de paliers de gain graves/aigus précalculés (-12dB à +12dB par
/// pas de 2dB) — reprise de Hatari (`dmaSnd.c`, `TONE_STEPS`).
const TONE_STEPS: usize = 13;

/// Mappe la valeur brute 4 bits d'une commande graves/aigus (0-15) sur
/// l'indice 0-12 dans les tables précalculées — reprise telle quelle de
/// Hatari (`LMC1992_Bass_Treble_Table`, `dmaSnd.c`).
const BASS_TREBLE_INDEX: [u8; 16] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 12, 12, 12];

/// Fréquence de coude des graves (Hz) — LMC1992 réel, valeur mesurée (voir
/// Hatari, `dmaSnd.c`, `DmaSnd_Init_Bass_and_Treble_Tables`).
const BASS_CORNER_HZ: f32 = 118.2763;
/// Fréquence de coude des aigus (Hz) — idem.
const TREBLE_CORNER_HZ: f32 = 8438.756;
/// Fréquence de sortie Rust68, FIXE (voir `AUDIO_SAMPLE_RATE` dans
/// `atari_st_sdl2.rs`) — les tables ci-dessous sont calculées une seule
/// fois pour cette fréquence (Hatari les recalcule si sa fréquence de
/// sortie change, ce qui n'arrive jamais ici).
const OUTPUT_RATE_HZ: f32 = 44100.0;

/// Coefficients d'un filtre "plateau" (shelf) du premier ordre — voir
/// [`bass_shelf`]/[`treble_shelf`].
#[derive(Debug, Clone, Copy, Default)]
struct FirstOrderShelf {
    a1: f32,
    b0: f32,
    b1: f32,
}

/// Coefficients du filtre plateau des GRAVES pour un gain linéaire `g`
/// (pas en dB) à la fréquence de coude `fc`, échantillonné à `fs` — reprise
/// telle quelle de Hatari (`DmaSnd_Bass_Shelf`, `dmaSnd.c`).
fn bass_shelf(g: f32, fc: f32, fs: f32) -> FirstOrderShelf {
    let tan_fc = (std::f32::consts::PI * fc / fs).tan();
    let a1 = if g < 1.0 { (tan_fc - g) / (tan_fc + g) } else { (tan_fc - 1.0) / (tan_fc + 1.0) };
    let b0 = (1.0 + a1) * (g - 1.0) / 2.0 + 1.0;
    let b1 = (1.0 + a1) * (g - 1.0) / 2.0 + a1;
    FirstOrderShelf { a1, b0, b1 }
}

/// Équivalent de [`bass_shelf`] pour les AIGUS — reprise telle quelle de
/// Hatari (`DmaSnd_Treble_Shelf`, `dmaSnd.c`).
fn treble_shelf(g: f32, fc: f32, fs: f32) -> FirstOrderShelf {
    let tan_fc = (std::f32::consts::PI * fc / fs).tan();
    let a1 = if g < 1.0 { (g * tan_fc - 1.0) / (g * tan_fc + 1.0) } else { (tan_fc - 1.0) / (tan_fc + 1.0) };
    let b0 = 1.0 + (1.0 - a1) * (g - 1.0) / 2.0;
    let b1 = a1 + (a1 - 1.0) * (g - 1.0) / 2.0;
    FirstOrderShelf { a1, b0, b1 }
}

/// Précalcule les 13 paliers de gain des graves (+12dB à -12dB, pas de
/// 2dB) — reprise de la boucle graves de Hatari
/// (`DmaSnd_Init_Bass_and_Treble_Tables`).
fn build_bass_table() -> [FirstOrderShelf; TONE_STEPS] {
    // Indice 12 = +12dB (boost max), indice 0 = -12dB (coupe max) — sens
    // fixé par Hatari (boucle C `for(n=TONE_STEPS; n--; ...)`, qui remplit
    // n=TONE_STEPS-1 EN PREMIER avec dB=+12 avant tout décrément), pas
    // arbitraire : c'est aussi le sens attendu par `BASS_TREBLE_INDEX`
    // (valeur brute de commande croissante -> indice croissant -> plus de
    // graves), donc l'ordre ici doit correspondre exactement.
    let mut table = [FirstOrderShelf::default(); TONE_STEPS];
    let mut db = 12.0f32;
    for entry in table.iter_mut().rev() {
        let g = 10f32.powf(db / 20.0);
        *entry = bass_shelf(g, BASS_CORNER_HZ, OUTPUT_RATE_HZ);
        db -= 2.0;
    }
    table
}

/// Précalcule les 13 paliers de gain des aigus — reprise de la boucle
/// aigus de Hatari, y compris le plafonnement de la fréquence de coude à
/// 80% de Nyquist si nécessaire (sans effet à 44100 Hz : 8438.756 Hz <
/// 0.4×44100=17640 Hz — gardé pour rester fidèle à la formule complète).
fn build_treble_table() -> [FirstOrderShelf; TONE_STEPS] {
    let nyquist_80pct = 0.5 * 0.8 * OUTPUT_RATE_HZ;
    let (fc, db_step) = if TREBLE_CORNER_HZ > nyquist_80pct {
        (nyquist_80pct, 2.0 * nyquist_80pct / TREBLE_CORNER_HZ)
    } else {
        (TREBLE_CORNER_HZ, 2.0)
    };
    // Même sens que `build_bass_table` (indice 12 = +12dB) — voir son
    // commentaire.
    let mut table = [FirstOrderShelf::default(); TONE_STEPS];
    let mut db = db_step * (TONE_STEPS as f32 - 1.0) / 2.0;
    for entry in table.iter_mut().rev() {
        let g = 10f32.powf(db / 20.0);
        *entry = treble_shelf(g, fc, OUTPUT_RATE_HZ);
        db -= db_step;
    }
    table
}

/// État complet du circuit Microwire/LMC1992.
#[derive(Debug, Clone)]
pub struct Microwire {
    mask: u16,
    data: u16,
    master_volume: u16,
    left_volume: u16,
    right_volume: u16,
    /// Valeur brute (0-15) de la dernière commande graves reçue — 6 =
    /// plat/0dB par défaut, comme Hatari (`DmaSnd_Reset`) : pas de signal
    /// de reset propre au Microwire sur silicium réel, mais un réglage
    /// neutre est un point de départ raisonnable avant toute commande.
    bass: u8,
    /// Voir [`Self::bass`], pour les aigus.
    treble: u8,
    bass_table: [FirstOrderShelf; TONE_STEPS],
    treble_table: [FirstOrderShelf; TONE_STEPS],
    /// Coefficients du filtre biquad COMBINÉ (graves+aigus multipliés
    /// algébriquement en un seul filtre du second ordre) —
    /// `[a1, a2, b0, b1, b2]`, voir [`Self::set_tone_level`].
    coef: [f32; 5],
    /// État (2 derniers échantillons intermédiaires) du filtre IIR, un par
    /// canal — voir [`Self::filter_left`]/[`Self::filter_right`].
    iir_left: [f32; 2],
    iir_right: [f32; 2],
}

impl Microwire {
    pub fn new() -> Self {
        // Volume plein par défaut (silicium réel : pas de signal de reset
        // sur le Microwire lui-même, mais TOS programme systématiquement un
        // volume raisonnable très tôt au boot ; démarrer atténué rendrait le
        // son totalement silencieux tant qu'aucune commande n'a encore été
        // envoyée).
        let mut mw = Microwire {
            mask: 0,
            data: 0,
            master_volume: 65535,
            left_volume: 65535,
            right_volume: 65535,
            bass: 6,
            treble: 6,
            bass_table: build_bass_table(),
            treble_table: build_treble_table(),
            coef: [0.0; 5],
            iir_left: [0.0; 2],
            iir_right: [0.0; 2],
        };
        mw.set_tone_level();
        mw
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
            1 => {
                self.bass = (cmd & 0xF) as u8;
                self.set_tone_level();
            }
            2 => {
                self.treble = (cmd & 0xF) as u8;
                self.set_tone_level();
            }
            3 => self.master_volume = MASTER_VOLUME_TABLE[(cmd & 0x3F) as usize],
            4 => self.right_volume = LEFT_RIGHT_VOLUME_TABLE[(cmd & 0x1F) as usize],
            5 => self.left_volume = LEFT_RIGHT_VOLUME_TABLE[(cmd & 0x1F) as usize],
            // Mixage (0) : non modélisé, voir la doc du module.
            _ => {}
        }
    }

    /// Recombine les tables graves/aigus précalculées en un seul filtre
    /// biquad ([`Self::coef`]), d'après les réglages courants — façon
    /// Hatari (`DmaSnd_Set_Tone_Level`) : simple développement algébrique
    /// du produit des deux fonctions de transfert du premier ordre (pas de
    /// `tan`/`pow`, contrairement à la construction des tables elle-même) —
    /// peut être appelé à chaque commande graves/aigus reçue sans souci de
    /// coût.
    fn set_tone_level(&mut self) {
        let treb = self.treble_table[BASS_TREBLE_INDEX[(self.treble & 0xF) as usize] as usize];
        let bass = self.bass_table[BASS_TREBLE_INDEX[(self.bass & 0xF) as usize] as usize];
        self.coef[0] = treb.a1 + bass.a1;
        self.coef[1] = treb.a1 * bass.a1;
        self.coef[2] = treb.b0 * bass.b0;
        self.coef[3] = treb.b0 * bass.b1 + treb.b1 * bass.b0;
        self.coef[4] = treb.b1 * bass.b1;
    }

    /// Applique gain + filtre graves/aigus au canal GAUCHE pour un
    /// échantillon — voir la doc de module (`filter_left`/`filter_right`).
    pub fn filter_left(&mut self, xn: f32, gain: f32) -> f32 {
        Self::iir_step(&self.coef, &mut self.iir_left, xn, gain)
    }

    /// Équivalent de [`Self::filter_left`] pour le canal DROIT — état
    /// indépendant ([`Self::iir_right`]), voir la doc de module sur les
    /// tons stéréo distincts de la cartouche de diagnostic usine.
    pub fn filter_right(&mut self, xn: f32, gain: f32) -> f32 {
        Self::iir_step(&self.coef, &mut self.iir_right, xn, gain)
    }

    /// Un pas de filtre biquad forme directe II — reprise telle quelle de
    /// Hatari (`DmaSnd_IIRfilterL`/`DmaSnd_IIRfilterR`) : le gain
    /// s'applique à l'ENTRÉE du filtre (même passe), pas en aval.
    fn iir_step(coef: &[f32; 5], state: &mut [f32; 2], xn: f32, gain: f32) -> f32 {
        let a = gain * xn - coef[0] * state[0] - coef[1] * state[1];
        let yn = coef[2] * a + coef[3] * state[0] + coef[4] * state[1];
        state[1] = state[0];
        state[0] = a;
        yn
    }

    /// Gain (0.0-1.0) du canal gauche : volume gauche × volume maître —
    /// appliqué séparément du canal droit ([`Self::right_gain`]) puisque la
    /// cartouche de diagnostic usine STe (test "Stereo 1 kHz/500 Hz tones")
    /// programme délibérément des volumes gauche/droite différents et
    /// changeants pour faire entendre 2 tons à des niveaux distincts.
    /// Diagnostic externe / cible de lissage — voir [`Self::filter_left`]
    /// pour l'application réelle (gain fourni en paramètre, pas relu ici).
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

    // --- Filtre graves/aigus (façon Hatari) -----------------------------

    #[test]
    fn reglage_par_defaut_est_transparent() {
        // Graves=aigus=6 (0dB/plat, réglage par défaut) : chaque filtre du
        // premier ordre a une fonction de transfert EXACTEMENT égale à 1
        // (numérateur == dénominateur, `b0=1, b1=a1` — voir `bass_shelf`/
        // `treble_shelf` pour g=1.0), donc le biquad combiné aussi, PAS
        // seulement en régime établi : le filtre doit se comporter comme un
        // gain pur dès le premier échantillon, y compris en transitoire.
        let mut mw = Microwire::new();
        for &xn in &[1000.0, -500.0, 0.0, 12345.0, -8888.0] {
            let yn = mw.filter_left(xn, 2.0);
            assert!((yn - xn * 2.0).abs() < 0.01, "attendu {} (gain pur), obtenu {yn}", xn * 2.0);
        }
    }

    #[test]
    fn graves_a_fond_amplifie_un_signal_continu_en_regime_etabli() {
        // Un shelf de GRAVES agit essentiellement sur le continu (0 Hz) :
        // un signal constant, en régime établi, doit converger vers
        // entrée × gain_linéaire(dB des graves) — quasi indépendant du
        // réglage des aigus (shelf HAUTES fréquences, transparent en
        // continu). +12dB ~ gain linéaire ×3.98.
        let mut mw = Microwire::new();
        mw.write_mask_high(0x07);
        mw.write_mask_low(0xFF);
        // Type=1 (graves) << 6, valeur=12 (index max, +12dB).
        mw.write_data_high((((0x400 | (1 << 6) | 12) >> 8) & 0xFF) as u8);
        mw.write_data_low(((0x400 | (1 << 6) | 12)) as u8);

        let mut yn = 0.0;
        for _ in 0..500 {
            yn = mw.filter_left(1000.0, 1.0);
        }
        assert!((yn - 3981.0).abs() < 50.0, "régime établi attendu ~3981 (+12dB), obtenu {yn}");
    }

    #[test]
    fn graves_au_minimum_attenue_un_signal_continu_en_regime_etabli() {
        let mut mw = Microwire::new();
        mw.write_mask_high(0x07);
        mw.write_mask_low(0xFF);
        // Type=1 (graves) << 6, valeur=0 (index min, -12dB).
        mw.write_data_high((((0x400 | (1 << 6)) >> 8) & 0xFF) as u8);
        mw.write_data_low((0x400 | (1 << 6)) as u8);

        let mut yn = 0.0;
        for _ in 0..500 {
            yn = mw.filter_left(1000.0, 1.0);
        }
        assert!((yn - 251.0).abs() < 20.0, "régime établi attendu ~251 (-12dB), obtenu {yn}");
    }

    #[test]
    fn canaux_gauche_et_droit_ont_un_etat_independant() {
        // Le filtre étant récursif (IIR), un canal alimenté en continu ne
        // doit JAMAIS voir son état influencé par l'autre canal, même
        // alimenté avec des valeurs très différentes.
        let mut mw = Microwire::new();
        mw.filter_left(10000.0, 1.0);
        mw.filter_left(10000.0, 1.0);
        let right_untouched = mw.filter_right(0.0, 1.0);
        assert!(right_untouched.abs() < 0.01, "canal droit non affecté par l'activité du canal gauche");
    }
}
