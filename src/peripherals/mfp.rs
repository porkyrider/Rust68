//! Motorola MC68901 MFP (Multi-Function Peripheral).
//!
//! Puce d'E/S de l'Atari ST (et de bien d'autres systèmes 68k) : 8 lignes
//! d'E/S générales (GPIP), 4 timers (A/B/C/D), un contrôleur d'interruption
//! à 16 canaux, et un USART RS-232.
//!
//! Ce module modélise la puce **seule**, indépendamment de tout câblage
//! système : c'est à l'appelant (le "board" Atari ST, pas encore implémenté
//! dans ce crate) de mapper [`Mfp::read`]/[`Mfp::write`] dans son
//! [`crate::Bus`], de brancher [`Mfp::iack`] sur `Bus::irq_ack`, et de
//! relier [`Mfp::interrupt_requested`] à la génération du niveau IPL
//! (le MFP est câblé sur IPL6 sur ST/STE réel, mais ce choix appartient au
//! board, pas à la puce).
//!
//! ## Limitations connues (v1)
//! - Pas de résolution de priorité imbriquée entre canaux : le canal actif
//!   de plus haute priorité masque simplement tous les autres tant qu'il
//!   n'est pas acquitté, conformément au mode le plus courant, mais sans
//!   modéliser l'interruption d'un ISR de priorité inférieure par un
//!   nouveau canal de priorité supérieure en cours de service.
//! - L'USART est un modèle "au byte" (pas de bit start/stop/parité ni de
//!   génération de baud rate réelle) : `push_rx_byte`/`take_tx_byte`
//!   simulent la réception/émission au niveau octet.
//! - `tick()` suppose une horloge CPU fixe à 8 MHz (ST/STE) pour convertir
//!   les cycles CPU en cycles d'horloge MFP réelle (2.4576 MHz).

/// Offsets des registres dans l'espace d'adressage de la puce (÷2 : sur
/// Atari ST réel, le MFP est mappé aux adresses impaires 0xFFFA01,
/// 0xFFFA03… tous les 2 octets — cet offset est l'index logique, à
/// l'appelant de le convertir depuis/vers l'adresse bus réelle).
pub mod reg {
    pub const GPIP: u8 = 0;
    pub const AER: u8 = 1;
    pub const DDR: u8 = 2;
    pub const IERA: u8 = 3;
    pub const IERB: u8 = 4;
    pub const IPRA: u8 = 5;
    pub const IPRB: u8 = 6;
    pub const ISRA: u8 = 7;
    pub const ISRB: u8 = 8;
    pub const IMRA: u8 = 9;
    pub const IMRB: u8 = 10;
    pub const VR: u8 = 11;
    pub const TACR: u8 = 12;
    pub const TBCR: u8 = 13;
    pub const TCDCR: u8 = 14;
    pub const TADR: u8 = 15;
    pub const TBDR: u8 = 16;
    pub const TCDR: u8 = 17;
    pub const TDDR: u8 = 18;
    pub const SCR: u8 = 19;
    pub const UCR: u8 = 20;
    pub const RSR: u8 = 21;
    pub const TSR: u8 = 22;
    pub const UDR: u8 = 23;
}

/// Numéros de canal d'interruption (0-15), fixés par le silicium — table du
/// datasheet MC68901. Les canaux 8-15 vivent dans les registres "A"
/// (IERA/IPRA/ISRA/IMRA), les canaux 0-7 dans les registres "B".
pub mod channel {
    pub const GPIP0: u8 = 0;
    pub const GPIP1: u8 = 1;
    pub const GPIP2: u8 = 2;
    pub const GPIP3: u8 = 3;
    pub const TIMER_D: u8 = 4;
    pub const TIMER_C: u8 = 5;
    pub const GPIP4: u8 = 6;
    pub const GPIP5: u8 = 7;
    pub const TIMER_B: u8 = 8;
    pub const TX_ERROR: u8 = 9;
    pub const TX_EMPTY: u8 = 10;
    pub const RX_ERROR: u8 = 11;
    pub const RX_FULL: u8 = 12;
    pub const TIMER_A: u8 = 13;
    pub const GPIP6: u8 = 14;
    pub const GPIP7: u8 = 15;
}

/// Table des diviseurs de prescaler en mode "delay" (valeurs 1-7 du champ
/// prescale des registres de contrôle timer ; l'index 0 = timer arrêté).
const PRESCALE: [u32; 8] = [0, 4, 10, 16, 50, 64, 100, 200];

/// Ratio horloge MFP / horloge CPU pour un ST/STE (CPU à 8 MHz, MFP à
/// 2.4576 MHz) : 2 457 600 / 8 000 000 = 192/625, réduit en entiers pour
/// accumulation exacte sans dérive flottante (voir `tick`).
const MFP_CLOCK_NUM: u32 = 192;
const MFP_CLOCK_DEN: u32 = 625;

#[derive(Debug, Clone)]
struct Timer {
    /// Registre de contrôle brut (TACR/TBCR : bits 0-3 ; TCDCR découpe ses
    /// deux timers sur bits 6-4 et 2-0 — le champ stocké ici est toujours
    /// déjà aligné sur bits 3-0 par `Mfp::write`).
    control: u8,
    /// Registre de données (valeur de rechargement ; 0 est traité comme 256).
    data: u8,
    /// Valeur courante du compte à rebours (ce que renvoie une lecture du
    /// registre de données pendant que le timer tourne).
    counter: u8,
    /// Accumulateur de cycles MFP fractionnaires pour le mode delay (voir
    /// `MFP_CLOCK_NUM`/`DEN`) — uniquement pour Timer A/B, qui ont aussi un
    /// mode event-count piloté par `pulse()` plutôt que `tick()`.
    prescale_acc: u32,
}

impl Timer {
    fn new() -> Self {
        Timer {
            control: 0,
            data: 0,
            counter: 0,
            prescale_acc: 0,
        }
    }

    fn prescale_divisor(&self) -> u32 {
        PRESCALE[(self.control & 0x7) as usize]
    }

    fn running(&self) -> bool {
        // 0 = arrêté. 1-7 = mode delay (bits 0-2 = diviseur). 8 (bit 3 seul,
        // Timer A/B uniquement) = mode event-count, décompté par `pulse()`
        // et non par le diviseur — donc `!= 0` couvre les deux, pas
        // seulement `& 0x7`.
        self.control != 0
    }

    /// Mode "event count" (Timer A/B uniquement) : bit 3 du registre de
    /// contrôle, décompte sur `pulse()` plutôt que sur l'horloge MFP.
    fn event_count_mode(&self) -> bool {
        self.control & 0x8 != 0
    }

    fn reload(&mut self) {
        self.counter = self.data;
        self.prescale_acc = 0;
    }

    /// Décrémente le compteur d'un cran ; renvoie `true` s'il vient de
    /// passer par zéro (et se recharge alors depuis `data`, 0 valant 256).
    fn decrement(&mut self) -> bool {
        if self.counter == 0 {
            self.counter = if self.data == 0 { 255 } else { self.data - 1 };
            true
        } else {
            self.counter -= 1;
            false
        }
    }
}

/// État complet d'une puce MC68901.
#[derive(Debug, Clone)]
pub struct Mfp {
    gpip_in: u8,
    gpip_out: u8,
    aer: u8,
    ddr: u8,
    ier: u16,
    ipr: u16,
    isr: u16,
    imr: u16,
    vr: u8,
    ta: Timer,
    tb: Timer,
    tc: Timer,
    td: Timer,
    scr: u8,
    ucr: u8,
    rsr: u8,
    tsr: u8,
    udr: u8,
    rx_queue: std::collections::VecDeque<u8>,
    tx_queue: std::collections::VecDeque<u8>,
}

impl Default for Mfp {
    fn default() -> Self {
        Self::new()
    }
}

impl Mfp {
    /// État après reset matériel : tous les registres à zéro (comportement
    /// du MC68901 documenté au reset, RSR/TSR compris).
    pub fn new() -> Self {
        Mfp {
            gpip_in: 0,
            gpip_out: 0,
            aer: 0,
            ddr: 0,
            ier: 0,
            ipr: 0,
            isr: 0,
            imr: 0,
            vr: 0,
            ta: Timer::new(),
            tb: Timer::new(),
            tc: Timer::new(),
            td: Timer::new(),
            scr: 0,
            ucr: 0,
            rsr: 0,
            tsr: 0,
            udr: 0,
            rx_queue: std::collections::VecDeque::new(),
            tx_queue: std::collections::VecDeque::new(),
        }
    }

    // --- Registres -----------------------------------------------------

    /// Lit le registre logique `offset` (voir [`reg`]).
    pub fn read(&mut self, offset: u8) -> u8 {
        match offset {
            reg::GPIP => (self.gpip_out & self.ddr) | (self.gpip_in & !self.ddr),
            reg::AER => self.aer,
            reg::DDR => self.ddr,
            reg::IERA => (self.ier >> 8) as u8,
            reg::IERB => self.ier as u8,
            reg::IPRA => (self.ipr >> 8) as u8,
            reg::IPRB => self.ipr as u8,
            reg::ISRA => (self.isr >> 8) as u8,
            reg::ISRB => self.isr as u8,
            reg::IMRA => (self.imr >> 8) as u8,
            reg::IMRB => self.imr as u8,
            reg::VR => self.vr,
            reg::TACR => self.ta.control,
            reg::TBCR => self.tb.control,
            reg::TCDCR => (self.tc.control << 4) | self.td.control,
            // Lire le registre de données d'un timer EN MARCHE renvoie le
            // compte à rebours courant, pas la valeur de rechargement écrite
            // (comportement réel du MC68901, utilisé par TOS/GEM pour lire
            // une horloge sans l'arrêter).
            reg::TADR => {
                if self.ta.running() {
                    self.ta.counter
                } else {
                    self.ta.data
                }
            }
            reg::TBDR => {
                if self.tb.running() {
                    self.tb.counter
                } else {
                    self.tb.data
                }
            }
            reg::TCDR => {
                if self.tc.running() {
                    self.tc.counter
                } else {
                    self.tc.data
                }
            }
            reg::TDDR => {
                if self.td.running() {
                    self.td.counter
                } else {
                    self.td.data
                }
            }
            reg::SCR => self.scr,
            reg::UCR => self.ucr,
            reg::RSR => self.rsr,
            reg::TSR => self.tsr,
            reg::UDR => {
                // Lire UDR renvoie l'octet actuellement latché, efface
                // "buffer full", puis latche le prochain octet en attente
                // s'il y en a un (pour la lecture suivante).
                let value = self.udr;
                self.rsr &= !RSR_BUFFER_FULL;
                self.maybe_start_next_rx();
                value
            }
            _ => 0,
        }
    }

    /// Écrit le registre logique `offset` (voir [`reg`]).
    pub fn write(&mut self, offset: u8, value: u8) {
        match offset {
            reg::GPIP => self.gpip_out = value,
            reg::AER => self.aer = value,
            reg::DDR => self.ddr = value,
            reg::IERA => self.ier = (self.ier & 0x00FF) | ((value as u16) << 8),
            reg::IERB => self.ier = (self.ier & 0xFF00) | value as u16,
            // IPR/ISR ne peuvent qu'être effacés par écriture logicielle
            // (écrire 1 sur un bit n'a aucun effet ; seul écrire 0 efface —
            // comportement documenté du MC68901, pour acquitter une source
            // sans risquer d'en armer une autre par erreur).
            reg::IPRA => self.ipr &= 0x00FF | ((value as u16) << 8),
            reg::IPRB => self.ipr &= 0xFF00 | value as u16,
            reg::ISRA => self.isr &= 0x00FF | ((value as u16) << 8),
            reg::ISRB => self.isr &= 0xFF00 | value as u16,
            reg::IMRA => self.imr = (self.imr & 0x00FF) | ((value as u16) << 8),
            reg::IMRB => self.imr = (self.imr & 0xFF00) | value as u16,
            reg::VR => self.vr = value,
            reg::TACR => {
                self.ta.control = value & 0x0F;
                self.ta.reload();
            }
            reg::TBCR => {
                self.tb.control = value & 0x0F;
                self.tb.reload();
            }
            reg::TCDCR => {
                self.tc.control = (value >> 4) & 0x07;
                self.td.control = value & 0x07;
                self.tc.reload();
                self.td.reload();
            }
            reg::TADR => {
                self.ta.data = value;
                if !self.ta.running() {
                    self.ta.counter = value;
                }
            }
            reg::TBDR => {
                self.tb.data = value;
                if !self.tb.running() {
                    self.tb.counter = value;
                }
            }
            reg::TCDR => {
                self.tc.data = value;
                if !self.tc.running() {
                    self.tc.counter = value;
                }
            }
            reg::TDDR => {
                self.td.data = value;
                if !self.td.running() {
                    self.td.counter = value;
                }
            }
            reg::SCR => self.scr = value,
            reg::UCR => self.ucr = value,
            reg::RSR => self.rsr = value,
            reg::TSR => self.tsr = value,
            reg::UDR => {
                self.udr = value;
                self.tx_queue.push_back(value);
                // Modèle simplifié : la transmission est instantanée (pas de
                // baud rate simulé), TSR reste "buffer empty" immédiatement.
                self.tsr |= TSR_BUFFER_EMPTY;
                self.request(channel::TX_EMPTY);
            }
            _ => {}
        }
    }

    // --- Timers ----------------------------------------------------------

    /// Avance les timers en mode "delay" de `cpu_cycles` cycles CPU (horloge
    /// ST/STE à 8 MHz — voir la constante `MFP_CLOCK_NUM/DEN`). Les timers
    /// A/B en mode event-count ne sont PAS avancés ici : voir [`Self::pulse_ta`]/
    /// [`Self::pulse_tb`].
    pub fn tick(&mut self, cpu_cycles: u32) {
        self.tick_delay_timer(cpu_cycles, TimerId::A);
        self.tick_delay_timer(cpu_cycles, TimerId::B);
        self.tick_delay_timer(cpu_cycles, TimerId::C);
        self.tick_delay_timer(cpu_cycles, TimerId::D);
    }

    fn tick_delay_timer(&mut self, cpu_cycles: u32, id: TimerId) {
        let (timer, chan) = self.timer_and_channel_mut(id);
        if !timer.running() {
            return;
        }
        if id == TimerId::A || id == TimerId::B {
            if timer.event_count_mode() {
                return; // avancé par pulse_ta/pulse_tb, pas par l'horloge
            }
        }
        let div = timer.prescale_divisor();
        if div == 0 {
            return;
        }
        timer.prescale_acc += cpu_cycles * MFP_CLOCK_NUM;
        let mfp_ticks = timer.prescale_acc / (MFP_CLOCK_DEN * div);
        timer.prescale_acc %= MFP_CLOCK_DEN * div;
        let mut fired = false;
        for _ in 0..mfp_ticks {
            if timer.decrement() {
                fired = true;
            }
        }
        if fired {
            self.request(chan);
        }
    }

    fn timer_and_channel_mut(&mut self, id: TimerId) -> (&mut Timer, u8) {
        match id {
            TimerId::A => (&mut self.ta, channel::TIMER_A),
            TimerId::B => (&mut self.tb, channel::TIMER_B),
            TimerId::C => (&mut self.tc, channel::TIMER_C),
            TimerId::D => (&mut self.td, channel::TIMER_D),
        }
    }

    /// Signale un front sur l'entrée TAI (Timer A en mode event-count).
    pub fn pulse_ta(&mut self) {
        if self.ta.running() && self.ta.event_count_mode() && self.ta.decrement() {
            self.request(channel::TIMER_A);
        }
    }

    /// Signale un front sur l'entrée TBI (Timer B en mode event-count).
    pub fn pulse_tb(&mut self) {
        if self.tb.running() && self.tb.event_count_mode() && self.tb.decrement() {
            self.request(channel::TIMER_B);
        }
    }

    // --- GPIP / interruptions --------------------------------------------

    /// Applique un niveau logique à une broche GPIP (0-7) configurée en
    /// entrée (`DDR` bit clair) et déclenche une interruption si le front
    /// observé correspond au sens programmé dans `AER` (1 = front montant).
    pub fn set_gpip_input(&mut self, pin: u8, level: bool) {
        debug_assert!(pin < 8);
        let mask = 1u8 << pin;
        if self.ddr & mask != 0 {
            return; // broche configurée en sortie : pas de détection de front
        }
        let was = self.gpip_in & mask != 0;
        if was == level {
            return; // pas de front
        }
        let rising_wanted = self.aer & mask != 0;
        if level == rising_wanted {
            // GPIP0-3 → canaux 0-3 ; GPIP4-7 → canaux 6,7,14,15 (table du
            // datasheet, cf. module `channel`).
            let chan = match pin {
                0 => channel::GPIP0,
                1 => channel::GPIP1,
                2 => channel::GPIP2,
                3 => channel::GPIP3,
                4 => channel::GPIP4,
                5 => channel::GPIP5,
                6 => channel::GPIP6,
                7 => channel::GPIP7,
                _ => unreachable!(),
            };
            self.request(chan);
        }
        if level {
            self.gpip_in |= mask;
        } else {
            self.gpip_in &= !mask;
        }
    }

    /// Arme le bit "pending" d'un canal (IPR) s'il est activé (IER).
    fn request(&mut self, chan: u8) {
        let mask = 1u16 << chan;
        if self.ier & mask != 0 {
            self.ipr |= mask;
        }
    }

    /// Vrai si au moins un canal pending+enabled+non masqué demande service
    /// — c'est ce signal que le board doit relayer vers `Bus::irq_level`
    /// (câblé sur IPL6 sur ST/STE réel).
    pub fn interrupt_requested(&self) -> bool {
        self.ipr & self.imr != 0
    }

    /// Canal actif de plus haute priorité (15 = le plus prioritaire, table
    /// du datasheet), parmi ceux pending+enabled+non masqués.
    fn highest_priority_pending(&self) -> Option<u8> {
        let active = self.ipr & self.imr;
        if active == 0 {
            None
        } else {
            Some(15 - active.leading_zeros() as u8)
        }
    }

    /// Cycle d'acquittement d'interruption (IACK) : calcule le vecteur pour
    /// le canal actif de plus haute priorité, efface son bit pending, et
    /// arme son bit "in service" — sauf en mode "automatic end-of-interrupt"
    /// (bit S du VR, bit 3), où ISR n'est jamais posé (le canal reste libre
    /// de se re-déclencher immédiatement, ce mode ne bloque pas les
    /// interruptions de même priorité).
    ///
    /// Renvoie le vecteur complet : bits 7-3 = `VR[7:3]` (programmé par le
    /// logiciel), bits 2-0 = numéro de canal.
    pub fn iack(&mut self) -> u8 {
        let Some(chan) = self.highest_priority_pending() else {
            // Interruption fantôme (retirée avant l'IACK) : vecteur spurious
            // standard du 68000 (24), comme le ferait VPA sans MFP.
            return 24;
        };
        let mask = 1u16 << chan;
        self.ipr &= !mask;
        const AUTO_EOI: u8 = 0x08;
        if self.vr & AUTO_EOI == 0 {
            self.isr |= mask;
        }
        (self.vr & 0xF8) | chan
    }

    /// Acquitte manuellement un canal en mode "software end-of-interrupt"
    /// (écriture logicielle de 0 dans ISR, à faire depuis le handler avant
    /// le RTE — cf. [`Self::write`] sur `reg::ISRA`/`ISRB`, exposé ici comme
    /// helper direct pour le board).
    pub fn end_of_interrupt(&mut self, chan: u8) {
        self.isr &= !(1u16 << chan);
    }

    // --- USART -------------------------------------------------------------

    /// Injecte un octet reçu (simulation RS-232 au niveau octet, cf.
    /// limitations du module). S'il n'y a pas déjà une réception en cours,
    /// arme immédiatement "buffer full" et le canal RX_FULL.
    pub fn push_rx_byte(&mut self, byte: u8) {
        self.rx_queue.push_back(byte);
        self.maybe_start_next_rx();
    }

    fn maybe_start_next_rx(&mut self) {
        if self.rsr & RSR_BUFFER_FULL == 0 {
            if let Some(b) = self.rx_queue.pop_front() {
                self.udr = b;
                self.rsr |= RSR_BUFFER_FULL;
                self.request(channel::RX_FULL);
            }
        }
    }

    /// Retire le prochain octet transmis par le programme (écrit dans UDR
    /// avec direction émission), s'il y en a un.
    pub fn take_tx_byte(&mut self) -> Option<u8> {
        self.tx_queue.pop_front()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TimerId {
    A,
    B,
    C,
    D,
}

const RSR_BUFFER_FULL: u8 = 1 << 7;
const TSR_BUFFER_EMPTY: u8 = 1 << 7;
