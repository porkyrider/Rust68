//! Motorola MC6850 ACIA (Asynchronous Communications Interface Adapter).
//!
//! L'Atari ST embarque **deux** puces ACIA identiques : une pour le
//! clavier (relié au contrôleur HD6301 du clavier via une liaison série),
//! une pour le port MIDI (in/out). Les deux partagent le même modèle de
//! puce — c'est [`crate::systems::atari_st::AtariSt`] qui instancie deux
//! [`Acia`] séparées et relie leurs sorties IRQ (OR câblé) sur `GPIP4` du
//! MFP (voir `peripherals::mfp::channel::GPIP4`, câblage réel ST/STE).
//!
//! ## Limitations connues (v1)
//! - Modèle "au byte" comme `peripherals::mfp` : pas de bit start/stop/
//!   parité ni de baud rate réel — seuls les registres et les flags de
//!   statut (RDRF/TDRE/OVRN/FE) sont modélisés fidèlement.
//! - `DCD`/`CTS` (détection porteuse / clear-to-send) sont câblés
//!   toujours actifs (pas d'erreur, toujours prêt à émettre) : aucune
//!   simulation de ligne de handshake externe.
//! - Pas de simulation de bit de parité (`PE` reste toujours à 0).

/// Offsets des deux registres logiques (÷2 par rapport à l'espacement réel
/// sur le bus — voir `systems::atari_st` pour le mapping d'adresses).
pub mod reg {
    /// Écriture : registre de contrôle. Lecture : registre de statut.
    pub const CONTROL_STATUS: u8 = 0;
    /// Écriture : registre d'émission. Lecture : registre de réception.
    pub const DATA: u8 = 1;
}

/// État d'une puce MC6850 ACIA.
#[derive(Debug, Clone)]
pub struct Acia {
    control: u8,
    rdrf: bool,
    tdre: bool,
    overrun: bool,
    framing_error: bool,
    rx_data: u8,
    tx_queue: std::collections::VecDeque<u8>,
}

impl Default for Acia {
    fn default() -> Self {
        Self::new()
    }
}

impl Acia {
    /// État après reset matériel : TDRE actif (émetteur prêt), tout le
    /// reste à zéro/faux — comportement documenté du MC6850.
    pub fn new() -> Self {
        Acia {
            control: 0,
            rdrf: false,
            tdre: true,
            overrun: false,
            framing_error: false,
            rx_data: 0,
            tx_queue: std::collections::VecDeque::new(),
        }
    }

    fn receive_interrupt_enabled(&self) -> bool {
        self.control & 0x80 != 0
    }

    /// Bits 6-5 du registre de contrôle : `01` = RTS bas + interruption
    /// d'émission activée (les 3 autres combinaisons désactivent TIE).
    fn transmit_interrupt_enabled(&self) -> bool {
        (self.control >> 5) & 0x03 == 0b01
    }

    /// Lit le registre logique `offset` (voir [`reg`]).
    pub fn read(&mut self, offset: u8) -> u8 {
        match offset {
            reg::CONTROL_STATUS => self.status_byte(),
            reg::DATA => {
                let value = self.rx_data;
                // Lire la donnée efface RDRF et OVRN (comportement réel du
                // MC6850 : la lecture acquitte les deux à la fois).
                self.rdrf = false;
                self.overrun = false;
                value
            }
            _ => 0,
        }
    }

    fn status_byte(&self) -> u8 {
        let irq = (self.rdrf && self.receive_interrupt_enabled())
            || (self.tdre && self.transmit_interrupt_enabled());
        (self.rdrf as u8)
            | ((self.tdre as u8) << 1)
            // bit 2 (DCD) et bit 3 (CTS) : toujours 0 (pas de handshake externe simulé)
            | ((self.framing_error as u8) << 4)
            | ((self.overrun as u8) << 5)
            // bit 6 (PE) : toujours 0 (pas de simulation de parité)
            | ((irq as u8) << 7)
    }

    /// Écrit le registre logique `offset` (voir [`reg`]).
    pub fn write(&mut self, offset: u8, value: u8) {
        match offset {
            reg::CONTROL_STATUS => {
                self.control = value;
                // Bits 0-1 = 11 : Master Reset (indépendant du reste du
                // registre de contrôle, qui est quand même mis à jour).
                if value & 0x03 == 0x03 {
                    self.rdrf = false;
                    self.tdre = true;
                    self.overrun = false;
                    self.framing_error = false;
                }
            }
            reg::DATA => {
                self.tx_queue.push_back(value);
                // Modèle "au byte" instantané (cf. limitations) : TDRE
                // reste/redevient immédiatement actif.
                self.tdre = true;
            }
            _ => {}
        }
    }

    /// Injecte un octet reçu (simulation au niveau octet, cf.
    /// limitations). Si un octet précédent n'a pas encore été lu (RDRF
    /// déjà actif), le nouvel octet est **perdu** et `OVRN` s'arme — le
    /// MC6850 n'a pas de FIFO de réception, un seul registre.
    pub fn push_rx_byte(&mut self, byte: u8) {
        if self.rdrf {
            self.overrun = true;
            return;
        }
        self.rx_data = byte;
        self.rdrf = true;
    }

    /// Retire le prochain octet transmis par le programme, s'il y en a
    /// un.
    pub fn take_tx_byte(&mut self) -> Option<u8> {
        self.tx_queue.pop_front()
    }

    /// Vrai si cette puce demande une interruption (à combiner en OR avec
    /// l'autre ACIA par le board — les deux partagent la même broche
    /// GPIP du MFP sur ST/STE réel).
    pub fn irq_requested(&self) -> bool {
        (self.rdrf && self.receive_interrupt_enabled())
            || (self.tdre && self.transmit_interrupt_enabled())
    }
}
