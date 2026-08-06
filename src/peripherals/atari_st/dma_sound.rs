//! DMA Sound (STE) — lecture d'échantillons PCM 8 bits signés depuis la RAM,
//! au rythme d'une des 4 fréquences matérielles (6258/12517/25033/50066 Hz),
//! en mono ou stéréo.
//!
//! Reproduit le comportement fonctionnel de Hatari (`dmaSnd.c`), simplifié :
//! pas de FIFO 8 octets — seuls l'avancement du compteur de trame, la
//! boucle/arrêt de fin de trame, et la conversion de fréquence vers le
//! taux de sortie hôte sont modélisés. Le filtre passe-bas/LMC1992
//! (bass/treble) EST modélisé, mais côté `Microwire` (voir sa doc de
//! module), en aval du mixage PSG+DMA Sound — pas ici, qui ne fournit que
//! les échantillons PCM bruts.
//!
//! **FIFO 8 octets, délibérément pas modélisé** : sur silicium réel, la
//! RAM est lue par blocs de 8 octets une fois par HBL (pas en continu au
//! rythme de la fréquence d'échantillonnage) — Hatari le modélise
//! fidèlement (`DmaSnd_FIFO_Refill`/`DmaSnd_FIFO_PullByte`, `dmaSnd.c`) car
//! cette granularité HBL a un effet audible réel sur les rares logiciels
//! qui réécrivent le tampon d'échantillons PENDANT la lecture (Hatari cite
//! nommément "Mental Hangover" et "Power Up Plus") : selon que la RAM est
//! relue au rythme HBL ou au moment exact de chaque échantillon consommé
//! (ce que fait ce module), l'écriture en direct devient audible avec un
//! décalage différent (jusqu'à ~1 HBL). Effet réel mais étroit (aucun cas
//! connu dans ce projet aujourd'hui) ; Steem SSE lui-même ne modélise pas
//! de FIFO pour le DMA Sound STE (son "FIFO" à lui est celui du contrôleur
//! DMA disquette/ACSI, un périphérique différent) — à reconsidérer si un
//! jeu/démo concret révèle un problème, pas anticipé sans cas connu.
//!
//! ## Registres (adresses relatives à `STE_DMA_SOUND_BASE` = `0xFF8900`)
//! Voir [`reg`]. Le Microwire (`$FF8920`/`$FF8922`/`$FF8924`) est un
//! périphérique séparé (LMC1992), géré ailleurs (voir
//! `AtariSt::STE_MICROWIRE_DATA`) — pas ici.

/// Offsets des registres, relatifs à `STE_DMA_SOUND_BASE`.
pub mod reg {
    /// `$FF8900`/`$FF8901` : contrôle DMA (mot, seul l'octet bas compte
    /// réellement sur silicium réel — bit0=lecture en cours, bit1=boucle).
    pub const CONTROL_LOW: u32 = 0x01;
    pub const FRAME_START_HIGH: u32 = 0x03;
    pub const FRAME_START_MID: u32 = 0x05;
    pub const FRAME_START_LOW: u32 = 0x07;
    pub const FRAME_COUNT_HIGH: u32 = 0x09;
    pub const FRAME_COUNT_MID: u32 = 0x0B;
    pub const FRAME_COUNT_LOW: u32 = 0x0D;
    pub const FRAME_END_HIGH: u32 = 0x0F;
    pub const FRAME_END_MID: u32 = 0x11;
    pub const FRAME_END_LOW: u32 = 0x13;
    /// `$FF8921` : mode son (bit7 = mono si posé, sinon stéréo ; bits1-0 =
    /// fréquence, voir [`super::SAMPLE_RATES_HZ`]).
    pub const SOUND_MODE: u32 = 0x21;
}

const CTRL_PLAY: u8 = 0x01;
const CTRL_LOOP: u8 = 0x02;
const MODE_MONO: u8 = 0x80;

/// Fréquences d'échantillonnage matérielles (Hz), indexées par les bits 1-0
/// de [`reg::SOUND_MODE`] — 8 010 613 Hz / 160, divisé par 8/4/2/1.
/// Confirmé par la communauté Atari (voir aussi `DmaSndSampleRates` chez
/// Hatari, `dmaSnd.c`).
const SAMPLE_RATES_HZ: [u32; 4] = [6258, 12517, 25033, 50066];

/// État complet du contrôleur DMA Sound (STE).
#[derive(Debug, Clone)]
pub struct DmaSound {
    control: u8,
    sound_mode: u8,
    frame_start: u32,
    frame_end: u32,
    /// Adresse courante de lecture — avance à chaque octet consommé,
    /// indépendamment du rythme de génération audio (voir
    /// [`Self::next_sample`]).
    frame_counter: u32,
    /// Accumulateur de conversion de fréquence, format fixe 32.32 (comme
    /// Hatari) : `(fréquence_dma << 32) / fréquence_hôte` ajouté à chaque
    /// échantillon de sortie généré ; sa partie entière indique combien de
    /// nouveaux octets consommer avant le prochain échantillon de sortie.
    freq_acc: u64,
    /// Dernier octet lu pour chaque canal (tenu entre deux avancées de
    /// l'accumulateur — c'est ce qui est effectivement mixé en sortie).
    held_left: i8,
    held_right: i8,
    /// Force une lecture immédiate au tout prochain [`Self::next_sample`],
    /// sans attendre que l'accumulateur de fréquence ait franchi un cran —
    /// sinon, quand la fréquence DMA est inférieure à celle de sortie hôte
    /// (ex: 6258 Hz vers 44100 Hz), les tout premiers échantillons de
    /// sortie resteraient à 0 (silence) au lieu du premier octet réel, le
    /// temps que l'accumulateur atteigne son premier cran.
    just_started: bool,
    /// Nombre de fronts XSINT survenus depuis le dernier
    /// [`Self::take_xsint_pulses`] — signal matériel réel câblé sur
    /// l'entrée de comptage d'événements du Timer A du MFP (voir la doc de
    /// [`Self::end_of_frame`]), utilisé par certains logiciels (dont la
    /// cartouche de diagnostic usine STe, test Audio) pour compter les
    /// bouclages de trame plutôt que de sonder un registre directement.
    xsint_pulses: u32,
}

impl DmaSound {
    pub fn new() -> Self {
        DmaSound {
            control: 0,
            sound_mode: 0,
            frame_start: 0,
            frame_end: 0,
            frame_counter: 0,
            freq_acc: 0,
            held_left: 0,
            held_right: 0,
            just_started: false,
            xsint_pulses: 0,
        }
    }

    /// Retire et renvoie le nombre de fronts XSINT accumulés depuis le
    /// dernier appel — à appeler une fois par `tick()` du board pour
    /// relayer chacun vers `Mfp::pulse_ta()` (voir la doc de
    /// [`Self::end_of_frame`]).
    pub fn take_xsint_pulses(&mut self) -> u32 {
        std::mem::take(&mut self.xsint_pulses)
    }

    fn mono(&self) -> bool {
        self.sound_mode & MODE_MONO != 0
    }

    fn sample_rate_hz(&self) -> u32 {
        SAMPLE_RATES_HZ[(self.sound_mode & 0x3) as usize]
    }

    pub fn playing(&self) -> bool {
        self.control & CTRL_PLAY != 0
    }

    /// Lit le registre logique `offset` (voir [`reg`]).
    pub fn read(&self, offset: u32) -> u8 {
        match offset {
            reg::CONTROL_LOW => self.control,
            reg::FRAME_COUNT_HIGH => (self.frame_counter >> 16) as u8,
            reg::FRAME_COUNT_MID => (self.frame_counter >> 8) as u8,
            reg::FRAME_COUNT_LOW => self.frame_counter as u8,
            reg::SOUND_MODE => self.sound_mode,
            _ => 0,
        }
    }

    /// Écrit le registre logique `offset` (voir [`reg`]).
    pub fn write(&mut self, offset: u32, value: u8) {
        match offset {
            reg::CONTROL_LOW => {
                let was_playing = self.playing();
                self.control = value & (CTRL_PLAY | CTRL_LOOP);
                if !was_playing && self.playing() {
                    self.start_new_frame();
                }
            }
            reg::FRAME_START_HIGH => self.frame_start = (self.frame_start & 0x00FFFF) | ((value as u32) << 16),
            reg::FRAME_START_MID => self.frame_start = (self.frame_start & 0xFF00FF) | ((value as u32) << 8),
            // Bit0 câblé à masse : adresses de trame toujours alignées mot,
            // comme les registres d'adresse du Blitter (voir sa doc).
            reg::FRAME_START_LOW => self.frame_start = (self.frame_start & 0xFFFF00) | (value as u32 & 0xFE),
            reg::FRAME_END_HIGH => self.frame_end = (self.frame_end & 0x00FFFF) | ((value as u32) << 16),
            reg::FRAME_END_MID => self.frame_end = (self.frame_end & 0xFF00FF) | ((value as u32) << 8),
            reg::FRAME_END_LOW => self.frame_end = (self.frame_end & 0xFFFF00) | (value as u32 & 0xFE),
            reg::SOUND_MODE => self.sound_mode = value,
            _ => {}
        }
    }

    /// Démarre une nouvelle trame : recopie début/fin dans le compteur de
    /// lecture. Si début == fin et boucle désactivée, arrête immédiatement
    /// (comportement vérifié sur silicium réel, voir Hatari
    /// `DmaSnd_StartNewFrame`).
    fn start_new_frame(&mut self) {
        self.frame_counter = self.frame_start;
        if self.frame_start == self.frame_end && self.control & CTRL_LOOP == 0 {
            self.control &= !CTRL_PLAY;
        } else {
            self.just_started = true;
        }
    }

    /// Fin de trame atteinte pendant la lecture : boucle (nouvelle trame) ou
    /// arrête, selon le bit de répétition. Compte toujours comme un front
    /// XSINT (voir [`Self::take_xsint_pulses`]), même en boucle : sur
    /// silicium réel, XSINT bascule brièvement à CHAQUE fin de trame, que la
    /// lecture continue ou s'arrête ensuite.
    fn end_of_frame(&mut self) {
        self.xsint_pulses += 1;
        if self.control & CTRL_LOOP != 0 {
            self.start_new_frame();
        } else {
            self.control &= !CTRL_PLAY;
        }
    }

    /// Consomme un octet à `self.frame_counter` dans `ram`, avance le
    /// compteur, et gère la fin de trame. Renvoie 0 (silence) si la lecture
    /// est arrêtée ou l'adresse hors de la RAM installée (silencieux,
    /// jamais de bus error — accès DMA, voir la doc de
    /// `AtariSt::in_floating_st_ram`).
    fn pull_byte(&mut self, ram: &[u8]) -> i8 {
        if !self.playing() {
            return 0;
        }
        let byte = ram.get(self.frame_counter as usize).copied().unwrap_or(0) as i8;
        self.frame_counter = self.frame_counter.wrapping_add(1);
        if self.frame_counter == self.frame_end {
            self.end_of_frame();
        }
        byte
    }

    /// Génère l'échantillon stéréo suivant (L, R), déjà à `host_rate_hz`
    /// (conversion de fréquence 32.32 fixe depuis la fréquence DMA en
    /// cours, voir [`Self::freq_acc`]) — à appeler une fois par échantillon
    /// de sortie audio, quel que soit l'état de lecture (silence si
    /// arrêté).
    pub fn next_sample(&mut self, ram: &[u8], host_rate_hz: u32) -> (i8, i8) {
        if !self.playing() {
            self.held_left = 0;
            self.held_right = 0;
            return (0, 0);
        }
        let ratio = ((self.sample_rate_hz() as u64) << 32) / host_rate_hz as u64;
        self.freq_acc += ratio;
        let mut steps = (self.freq_acc >> 32) as u32;
        self.freq_acc &= 0xFFFF_FFFF;
        if self.just_started {
            self.just_started = false;
            steps = steps.max(1);
        }
        for _ in 0..steps {
            if !self.playing() {
                break;
            }
            if self.mono() {
                let m = self.pull_byte(ram);
                self.held_left = m;
                self.held_right = m;
            } else {
                self.held_left = self.pull_byte(ram);
                self.held_right = self.pull_byte(ram);
            }
        }
        (self.held_left, self.held_right)
    }
}

impl Default for DmaSound {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn demarre_arrete_immediatement_si_debut_egale_fin_sans_boucle() {
        let mut dma = DmaSound::new();
        dma.write(reg::FRAME_START_HIGH, 0x00);
        dma.write(reg::FRAME_START_MID, 0x10);
        dma.write(reg::FRAME_START_LOW, 0x00);
        dma.write(reg::FRAME_END_HIGH, 0x00);
        dma.write(reg::FRAME_END_MID, 0x10);
        dma.write(reg::FRAME_END_LOW, 0x00);
        dma.write(reg::CONTROL_LOW, 0x01); // PLAY, pas de boucle
        assert!(!dma.playing(), "début == fin sans boucle : arrêt immédiat");
    }

    #[test]
    fn lit_des_echantillons_mono_et_boucle() {
        let ram = vec![0x10, 0x20, 0x30, 0x40, 0x00, 0x00, 0x00, 0x00];
        let mut dma = DmaSound::new();
        dma.write(reg::FRAME_START_LOW, 0x00);
        dma.write(reg::FRAME_END_LOW, 0x04); // 4 octets : 0x10,0x20,0x30,0x40
        dma.write(reg::SOUND_MODE, 0x83); // mono, 50066 Hz (bits1-0=11)
        dma.write(reg::CONTROL_LOW, 0x03); // PLAY + LOOP

        // host_rate == dma_rate : un octet consommé par échantillon de sortie.
        let (l0, r0) = dma.next_sample(&ram, 50066);
        assert_eq!((l0, r0), (0x10, 0x10));
        let (l1, _) = dma.next_sample(&ram, 50066);
        assert_eq!(l1, 0x20);
        let (l2, _) = dma.next_sample(&ram, 50066);
        assert_eq!(l2, 0x30);
        let (l3, _) = dma.next_sample(&ram, 50066);
        assert_eq!(l3, 0x40);
        // Fin de trame atteinte après le 4e octet -> boucle, repart à 0x10.
        let (l4, _) = dma.next_sample(&ram, 50066);
        assert_eq!(l4, 0x10);
        assert!(dma.playing(), "boucle active : ne doit jamais s'arrêter");
    }

    #[test]
    fn arrete_a_la_fin_de_trame_sans_boucle() {
        let ram = vec![0x11, 0x22];
        let mut dma = DmaSound::new();
        dma.write(reg::FRAME_START_LOW, 0x00);
        dma.write(reg::FRAME_END_LOW, 0x02);
        dma.write(reg::SOUND_MODE, 0x83); // mono, 50066 Hz
        dma.write(reg::CONTROL_LOW, 0x01); // PLAY, pas de boucle

        assert_eq!(dma.next_sample(&ram, 50066).0, 0x11);
        assert_eq!(dma.next_sample(&ram, 50066).0, 0x22);
        assert!(!dma.playing(), "fin de trame sans boucle : arrêt");
        assert_eq!(dma.next_sample(&ram, 50066), (0, 0), "silence une fois arrêté");
    }
}
