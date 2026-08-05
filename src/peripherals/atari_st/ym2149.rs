//! Yamaha YM2149 (PSG — Programmable Sound Generator), compatible registre
//! à registre avec le General Instrument AY-3-8910.
//!
//! Sur Atari ST : 3 canaux de son carré (A/B/C), un générateur de bruit
//! partagé, un générateur d'enveloppe, et deux ports d'E/S 8 bits (le port
//! A pilote entre autres la sélection de face/lecteur disquette, le strobe
//! Centronics, et les lignes de sélection joystick/souris — câblage
//! spécifique au board, pas modélisé ici, cf. limitations).
//!
//! Ce module modélise la puce **seule** : registres, générateurs de tonalité/
//! bruit/enveloppe, niveaux de sortie numériques par canal. C'est au board
//! de mapper [`Ym2149::read`]/[`Ym2149::write`] dans son `Bus` (registre
//! sélecteur à `0xFF8800`, registre de données à `0xFF8802` sur ST réel) et
//! de convertir [`Ym2149::channel_level`] en échantillons audio réels selon
//! son propre pipeline de sortie.
//!
//! ## Limitations connues (v1)
//! - Pas de conversion en échantillons PCM : seuls les niveaux numériques
//!   0-31 par canal sont exposés (mélange tonalité/bruit/enveloppe déjà
//!   fait, mais pas de courbe DAC ni de filtrage analogique simulés).
//! - Signification des bits des ports A/B (sélection lecteur, joystick,
//!   Centronics...) non interprétée : ce sont des registres 8 bits bruts,
//!   à charge du board de leur donner un sens.
//! - Table des formes d'enveloppe et polynôme du générateur de bruit :
//!   comportement documenté publiquement par le fabricant d'origine
//!   (General Instrument, datasheet AY-3-8910, table des 10 formes
//!   d'enveloppe et LFSR 17 bits) — reproduits ici indépendamment, pas
//!   empruntés à un émulateur existant.

/// Index des 16 registres (sélectionnés via [`Ym2149::write`] sur
/// [`REG_SELECT`], données lues/écrites ensuite sur [`REG_DATA`]).
pub mod reg {
    pub const TONE_A_FINE: u8 = 0;
    pub const TONE_A_COARSE: u8 = 1;
    pub const TONE_B_FINE: u8 = 2;
    pub const TONE_B_COARSE: u8 = 3;
    pub const TONE_C_FINE: u8 = 4;
    pub const TONE_C_COARSE: u8 = 5;
    pub const NOISE_PERIOD: u8 = 6;
    pub const MIXER: u8 = 7;
    pub const AMPLITUDE_A: u8 = 8;
    pub const AMPLITUDE_B: u8 = 9;
    pub const AMPLITUDE_C: u8 = 10;
    pub const ENVELOPE_FINE: u8 = 11;
    pub const ENVELOPE_COARSE: u8 = 12;
    pub const ENVELOPE_SHAPE: u8 = 13;
    pub const IO_PORT_A: u8 = 14;
    pub const IO_PORT_B: u8 = 15;
}

/// Nombre de bits significatifs de chaque registre (le reste est masqué à
/// l'écriture — comportement standard du silicium, qui n'a pas de bascule
/// pour les bits non câblés).
const REG_WIDTH_MASK: [u8; 16] = [
    0xFF, 0x0F, // tone A fine/coarse
    0xFF, 0x0F, // tone B
    0xFF, 0x0F, // tone C
    0x1F, // noise period (5 bits)
    0xFF, // mixer (tous les bits utilisés)
    0x1F, 0x1F, 0x1F, // amplitudes A/B/C (bit4 = mode enveloppe, bits0-3 = niveau)
    0xFF, 0xFF, // envelope period fine/coarse
    0x0F, // envelope shape
    0xFF, 0xFF, // I/O port A/B
];

/// Registre offset "select" (offset logique 0) et "data" (offset logique 1)
/// dans l'espace d'adressage de la puce — sur ST réel, sélecteur à
/// `0xFF8800`, données à `0xFF8802`.
pub mod bus_offset {
    pub const SELECT: u8 = 0;
    pub const DATA: u8 = 1;
}

#[derive(Debug, Clone, Default)]
struct ToneGenerator {
    counter: u16,
    output: bool,
}

impl ToneGenerator {
    /// Avance d'un cycle puce (déjà divisé du cycle CPU par l'appelant).
    /// Une tonalité désactivée (period 0, cas réel documenté : traité comme
    /// 1) bascule quand même au rythme minimal plutôt que de geler.
    ///
    /// Le compteur interne du silicium avance à clock/8 (pas à la fréquence
    /// puce elle-même) : basculer tous les `period` cycles puce donnerait un
    /// son 8× trop aigu. Confirmé par recoupement datasheet AY-3-8910/YM2149
    /// et par l'implémentation de référence Hatari (`sound.c`,
    /// `ToneA_count`/`ToneA_per` incrémentés une fois par tick à 250 kHz pour
    /// une puce cadencée à 2 MHz).
    fn tick(&mut self, period_reg: u16) {
        let period = period_reg.max(1) * 8;
        self.counter += 1;
        if self.counter >= period {
            self.counter = 0;
            self.output = !self.output;
        }
    }
}

#[derive(Debug, Clone, Default)]
struct NoiseGenerator {
    counter: u16,
    /// LFSR 17 bits (bit 0 = sortie courante). Polynôme standard
    /// AY-3-8910 : rebouclage sur XOR des bits 0 et 3.
    lfsr: u32,
}

impl NoiseGenerator {
    fn new() -> Self {
        NoiseGenerator {
            counter: 0,
            lfsr: 1, // état initial non nul (un LFSR bloqué à 0 ne bougerait jamais)
        }
    }

    /// Le compteur de bruit du silicium avance deux fois moins vite que celui
    /// de tonalité/enveloppe (clock/16 au lieu de clock/8) — vérifié dans
    /// Hatari (`sound.c`, `YM2149_Freq_div_2` : `Noise_count` n'avance qu'un
    /// cycle sur deux par rapport à `ToneX_count`).
    fn tick(&mut self, period_reg: u8) {
        let period = period_reg.max(1) as u16 * 16;
        self.counter += 1;
        if self.counter >= period {
            self.counter = 0;
            let feedback = (self.lfsr ^ (self.lfsr >> 3)) & 1;
            self.lfsr = (self.lfsr >> 1) | (feedback << 16);
        }
    }

    fn output(&self) -> bool {
        self.lfsr & 1 != 0
    }
}

/// Forme d'enveloppe (registre `ENVELOPE_SHAPE`, 4 bits) : table standard du
/// datasheet AY-3-8910 (Continue/Attack/Alternate/Hold), reconstruite ici
/// depuis la description publique du fabricant (pas du code d'émulateur).
#[derive(Debug, Clone, Copy)]
struct EnvelopeShape {
    continue_: bool,
    attack: bool,
    alternate: bool,
    hold: bool,
}

impl EnvelopeShape {
    fn from_bits(bits: u8) -> Self {
        EnvelopeShape {
            hold: bits & 0x1 != 0,
            alternate: bits & 0x2 != 0,
            attack: bits & 0x4 != 0,
            continue_: bits & 0x8 != 0,
        }
    }
}

#[derive(Debug, Clone)]
struct EnvelopeGenerator {
    counter: u32,
    /// Position dans la rampe 0..31 (5 bits de résolution, deux fois celle
    /// des canaux — comportement standard documenté du chip).
    step: u8,
    /// Sens de la rampe courante (true = montant).
    rising: bool,
    /// Vrai une fois qu'un cycle non-Continue a atteint sa fin et que le
    /// niveau doit rester figé.
    finished: bool,
    shape: EnvelopeShape,
}

impl EnvelopeGenerator {
    fn new() -> Self {
        EnvelopeGenerator {
            counter: 0,
            step: 0,
            rising: false,
            finished: false,
            shape: EnvelopeShape::from_bits(0),
        }
    }

    /// Écrire le registre de forme relance toujours l'enveloppe depuis le
    /// début (comportement documenté du silicium réel).
    fn restart(&mut self, shape_bits: u8) {
        self.shape = EnvelopeShape::from_bits(shape_bits);
        self.counter = 0;
        self.step = 0;
        self.rising = self.shape.attack;
        self.finished = false;
    }

    /// Le compteur d'enveloppe avance au même rythme que celui de tonalité
    /// (clock/8) — vérifié dans Hatari (`sound.c`, `Env_count`/`Env_per`
    /// incrémentés dans la même boucle à 250 kHz que `ToneX_count`). La
    /// résolution doublée (32 pas au lieu de 16) vient uniquement du nombre
    /// de pas de la rampe, pas d'un diviseur d'horloge différent.
    fn tick(&mut self, period_reg: u16) {
        if self.finished {
            return;
        }
        let period = period_reg.max(1) as u32 * 8;
        self.counter += 1;
        if self.counter < period {
            return;
        }
        self.counter = 0;
        self.step += 1;
        if self.step > 31 {
            self.step = 0;
            if !self.shape.continue_ {
                // Une seule rampe : se fige au niveau final (table du
                // datasheet — Hold n'a d'effet que si Continue=1).
                self.finished = true;
                self.step = if self.rising { 31 } else { 0 };
                return;
            }
            if self.shape.alternate {
                self.rising = !self.rising;
            }
            if self.shape.hold {
                self.finished = true;
                self.step = if self.rising { 31 } else { 0 };
            }
        }
    }

    /// Niveau courant 0-31.
    fn level(&self) -> u8 {
        if self.rising {
            self.step
        } else {
            31 - self.step
        }
    }
}

/// État complet d'une puce YM2149.
#[derive(Debug, Clone)]
pub struct Ym2149 {
    selected: u8,
    regs: [u8; 16],
    /// Accumulateur de cycles CPU fractionnaires : la puce est cadencée à
    /// CPU/4 (2 MHz pour un CPU ST/STE à 8 MHz).
    cpu_cycle_acc: u32,
    tone: [ToneGenerator; 3],
    noise: NoiseGenerator,
    envelope: EnvelopeGenerator,
    /// Niveaux d'entrée des ports A/B pour les bits configurés en entrée
    /// (DDR via `MIXER` bits 6-7) — injectés par l'appelant, cf.
    /// `set_port_a_input`/`set_port_b_input`.
    port_a_in: u8,
    port_b_in: u8,
    /// Somme des niveaux de sortie de chaque canal, accumulée À CHAQUE
    /// cycle puce (pas juste échantillonnée ponctuellement) depuis le
    /// dernier [`Self::take_averaged_levels`] — voir sa doc : nécessaire
    /// pour éviter l'aliasing quand on convertit vers un taux
    /// d'échantillonnage audio (44,1 kHz) bien plus lent que l'horloge
    /// puce (2 MHz, jusqu'à ~45 bascules possibles entre deux échantillons
    /// de sortie pour une tonalité aiguë).
    level_accum: [u32; 3],
    level_accum_count: u32,
}

impl Default for Ym2149 {
    fn default() -> Self {
        Self::new()
    }
}

impl Ym2149 {
    pub fn new() -> Self {
        let mut regs = [0u8; 16];
        // Port A : lignes de sélection lecteur/face du lecteur de
        // disquette (bits 0-2, voir `port_a_output`) tirées au niveau haut
        // par des résistances de rappel réelles tant que TOS n'a pas
        // programmé le port — "aucun lecteur sélectionné + face 0" (0xFF),
        // confirmé par Hatari (`psg.c` : commentaire explicite sur l'état
        // après reset). Sans ça, avant que TOS ne programme ce port très
        // tôt au boot, la face lue par défaut serait la face 1 (bit0=0)
        // plutôt que 0.
        regs[reg::IO_PORT_A as usize] = 0xFF;
        Ym2149 {
            selected: 0,
            regs,
            cpu_cycle_acc: 0,
            tone: Default::default(),
            noise: NoiseGenerator::new(),
            envelope: EnvelopeGenerator::new(),
            port_a_in: 0,
            port_b_in: 0,
            level_accum: [0; 3],
            level_accum_count: 0,
        }
    }

    /// Lit le bus de la puce à l'offset logique `offset` (voir
    /// [`bus_offset`]) : `SELECT` renvoie le numéro de registre
    /// actuellement sélectionné, `DATA` renvoie son contenu (les ports A/B
    /// combinent la valeur latchée en sortie avec le niveau d'entrée selon
    /// la direction programmée dans `MIXER`).
    pub fn read(&mut self, offset: u8) -> u8 {
        match offset {
            bus_offset::SELECT => self.selected,
            bus_offset::DATA => match self.selected {
                reg::IO_PORT_A => self.read_port(reg::IO_PORT_A, 6, self.port_a_in),
                reg::IO_PORT_B => self.read_port(reg::IO_PORT_B, 7, self.port_b_in),
                r if r < 16 => self.regs[r as usize],
                _ => 0xFF,
            },
            _ => 0xFF,
        }
    }

    fn read_port(&self, reg_idx: u8, dir_bit: u8, input: u8) -> u8 {
        let is_output = self.regs[reg::MIXER as usize] & (1 << dir_bit) != 0;
        if is_output {
            self.regs[reg_idx as usize]
        } else {
            input
        }
    }

    /// Écrit le bus de la puce à l'offset logique `offset` (voir
    /// [`bus_offset`]).
    pub fn write(&mut self, offset: u8, value: u8) {
        match offset {
            bus_offset::SELECT => self.selected = value & 0x0F,
            bus_offset::DATA => {
                let idx = self.selected as usize;
                if idx >= 16 {
                    return;
                }
                let masked = value & REG_WIDTH_MASK[idx];
                self.regs[idx] = masked;
                if idx == reg::ENVELOPE_SHAPE as usize {
                    self.envelope.restart(masked);
                }
            }
            _ => {}
        }
    }

    /// Applique un niveau au port A pour les bits configurés en entrée
    /// (DDR = 0 sur les bits correspondants de `MIXER`).
    pub fn set_port_a_input(&mut self, value: u8) {
        self.port_a_in = value;
    }

    /// Registre de sortie BRUT du port A (bascule interne, pas ce qu'une
    /// relecture bus verrait via [`Self::read`] — celle-ci dépend de la
    /// direction DDR programmée dans `MIXER`). Sur ST/STE réel, ce registre
    /// pilote directement (indépendamment de la direction lue par le CPU)
    /// les lignes de sélection lecteur/face du connecteur disquette :
    /// bit0 (inversé) = face (0 après négation = face 1, 1 = face 0),
    /// bit1 = 0 → lecteur A sélectionné, bit2 = 0 → lecteur B — confirmé
    /// par Hatari (`psg.c`/`fdc.c`, `FDC_SetDriveSide`). À charge du board
    /// de décoder ces bits (voir `AtariSt::tick`/écriture `FDC_DATA`).
    pub fn port_a_output(&self) -> u8 {
        self.regs[reg::IO_PORT_A as usize]
    }

    /// Applique un niveau au port B pour les bits configurés en entrée.
    pub fn set_port_b_input(&mut self, value: u8) {
        self.port_b_in = value;
    }

    fn tone_period(&self, fine: u8, coarse: u8) -> u16 {
        ((self.regs[coarse as usize] as u16) << 8) | self.regs[fine as usize] as u16
    }

    /// Avance les générateurs de `cpu_cycles` cycles CPU (horloge ST/STE à
    /// 8 MHz ; la puce elle-même tourne à CPU/4).
    pub fn tick(&mut self, cpu_cycles: u32) {
        self.cpu_cycle_acc += cpu_cycles;
        let chip_cycles = self.cpu_cycle_acc / 4;
        self.cpu_cycle_acc %= 4;

        let tone_a = self.tone_period(reg::TONE_A_FINE, reg::TONE_A_COARSE);
        let tone_b = self.tone_period(reg::TONE_B_FINE, reg::TONE_B_COARSE);
        let tone_c = self.tone_period(reg::TONE_C_FINE, reg::TONE_C_COARSE);
        let noise_period = self.regs[reg::NOISE_PERIOD as usize];
        let envelope_period = self.tone_period(reg::ENVELOPE_FINE, reg::ENVELOPE_COARSE);

        for _ in 0..chip_cycles {
            self.tone[0].tick(tone_a);
            self.tone[1].tick(tone_b);
            self.tone[2].tick(tone_c);
            self.noise.tick(noise_period);
            self.envelope.tick(envelope_period);
            // Accumule le niveau de CE cycle puce précis (pas seulement
            // l'état final après la boucle) — voir la doc de
            // `level_accum`/`take_averaged_levels`.
            for ch in 0..3 {
                self.level_accum[ch] += self.channel_level(ch) as u32;
            }
            self.level_accum_count += 1;
        }
    }

    /// Moyenne temporelle du niveau de chaque canal depuis le dernier appel
    /// (ou depuis la création/le reset si jamais appelé), puis remet
    /// l'accumulateur à zéro pour la prochaine période — à appeler une fois
    /// par échantillon de sortie audio produit (pas plus souvent), sans quoi
    /// la moyenne ne porterait que sur une fraction de la période réelle.
    ///
    /// Contrairement à [`Self::channel_level`] (un échantillonnage ponctuel
    /// de l'état instantané), ceci intègre TOUTES les bascules
    /// tonalité/bruit survenues entre deux échantillons de sortie — sans ça,
    /// une tonalité dont la période puce est plus courte que la période
    /// d'échantillonnage audio (44,1 kHz vs 2 MHz : jusqu'à ~45 bascules
    /// possibles par échantillon de sortie) produit un repliement de
    /// spectre (aliasing) qui sonne comme un parasite granuleux au lieu
    /// d'une tonalité propre — confirmé par un enregistrement réel de
    /// l'utilisateur montrant une forme d'onde en escalier au lieu d'un
    /// signal carré net.
    pub fn take_averaged_levels(&mut self) -> [f32; 3] {
        let count = self.level_accum_count;
        let levels = if count == 0 {
            // Aucun cycle puce écoulé depuis le dernier appel (sortie audio
            // échantillonnée plus vite que l'horloge puce, cas limite) :
            // retombe sur l'état instantané plutôt qu'une division par zéro.
            [self.channel_level(0) as f32, self.channel_level(1) as f32, self.channel_level(2) as f32]
        } else {
            let mut out = [0.0f32; 3];
            for ch in 0..3 {
                out[ch] = self.level_accum[ch] as f32 / count as f32;
            }
            out
        };
        self.level_accum = [0; 3];
        self.level_accum_count = 0;
        levels
    }

    /// Niveau numérique de sortie 0-31 du canal `channel` (0=A, 1=B, 2=C) à
    /// l'instant courant : combine le portillonnage tonalité/bruit
    /// (`MIXER`) et le niveau d'amplitude fixe ou d'enveloppe. Une puce
    /// AY-3-8910/YM2149 réelle produit un niveau 4 bits (0-15) en mode fixe
    /// mais 5 bits (0-31) en mode enveloppe ; on renvoie ici directement le
    /// niveau final déjà à l'échelle 0-31 (fixe ×2) pour une seule échelle
    /// de sortie cohérente entre les deux modes.
    pub fn channel_level(&self, channel: usize) -> u8 {
        let (tone_bit, noise_bit, amplitude_reg) = match channel {
            0 => (0, 3, reg::AMPLITUDE_A),
            1 => (1, 4, reg::AMPLITUDE_B),
            2 => (2, 5, reg::AMPLITUDE_C),
            _ => panic!("canal YM2149 invalide : {channel}"),
        };
        let mixer = self.regs[reg::MIXER as usize];
        let tone_enabled = mixer & (1 << tone_bit) == 0;
        let noise_enabled = mixer & (1 << noise_bit) == 0;
        let tone_active = !tone_enabled || self.tone[channel].output;
        let noise_active = !noise_enabled || self.noise.output();
        if !(tone_active && noise_active) {
            return 0;
        }
        let amplitude = self.regs[amplitude_reg as usize];
        if amplitude & 0x10 != 0 {
            self.envelope.level()
        } else {
            (amplitude & 0x0F) * 2
        }
    }
}
