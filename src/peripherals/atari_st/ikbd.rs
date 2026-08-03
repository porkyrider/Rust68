//! Émulation du contrôleur IKBD (HD6301) clavier/souris/joystick.
//!
//! Sur ST/STE réel, le HD6301 dialogue avec le CPU via l'ACIA clavier
//! (`AtariSt::acia_keyboard`) à 7812,5 bauds. Protocole (sortant, IKBD →
//! CPU) :
//! - Frappe clavier : `0xNN` (scancode 0x01-0x72)
//! - Relâchement : `0x80 | 0xNN`
//! - Mouvement souris relatif : `0xF8|boutons, dx, dy`
//!
//! Commandes (entrant, CPU → IKBD via l'émission de l'ACIA) : `0x80 0x01`
//! (reset), `0x08`-`0x0F` (modes souris), `0x0D` (interroge la position
//! absolue), etc. — voir [`Ikbd::receive_cmd`].
//!
//! Ce module ne modélise pas le joystick (Rust68 n'a pas de frontend
//! manette actuellement) : les commandes qui s'y rapportent sont
//! reconnues et consomment le bon nombre d'octets de paramètres (pour ne
//! pas désynchroniser le flux de commandes suivant), mais n'ont aucun
//! effet.

use std::collections::VecDeque;

/// Cycles CPU (68000 à 8 MHz) entre un reset (mise sous tension ou
/// commande `0x80 0x01`) et l'arrivée effective de la réponse
/// d'autotest `0xF1` de l'IKBD.
///
/// Livrer `0xF1` de façon synchrone (immédiatement au reset) fait
/// arriver l'octet avant que le TOS ait fini de configurer IERB/IMRB du
/// MFP : l'interruption ACIA correspondante, en attente mais encore
/// masquée à cet instant, est alors silencieusement effacée par
/// l'écriture ultérieure (normale) du TOS dans IERB qui active le canal
/// ACIA. Comme l'octet n'est de ce fait jamais lu, `RDRF` de l'ACIA
/// reste plein en permanence, bloquant tout octet suivant (clavier ET
/// souris) derrière lui pour toujours — plus aucune interruption IKBD
/// ne peut plus jamais arriver.
///
/// Valeur choisie empiriquement (voir le projet compagnon Stay, qui a
/// isolé et corrigé exactement cette régression) : suffisamment grande
/// pour retomber après que le TOS ait terminé la configuration
/// IERB/IMRB de son initialisation clavier/souris.
const IKBD_RESET_CYCLES: u32 = 5_000_000;

/// État complet d'un IKBD HD6301 émulé.
pub struct Ikbd {
    /// File de sortie : octets en attente de livraison à l'ACIA (RX, côté CPU).
    tx_queue: VecDeque<u8>,
    /// Tampon de commande entrante (CPU → IKBD via l'émission de l'ACIA).
    cmd_buf: Vec<u8>,
    /// Nombre d'octets de paramètre encore attendus avant d'exécuter la commande en cours.
    cmd_remaining: usize,

    // État souris.
    mouse_x: i32,
    mouse_y: i32,
    mouse_buttons: u8,
    /// Sens de l'axe Y : 1 = origine en haut (vers le bas = positif), -1 = origine en bas.
    y_axis: i8,

    /// Cycles restants avant la livraison d'une réponse `0xF1` de reset en
    /// attente (voir [`IKBD_RESET_CYCLES`]). `None` = aucun reset en cours.
    reset_pending_cycles: Option<u32>,
    /// Octets clavier/souris survenus pendant un reset en cours (voir
    /// [`Self::reset_pending_cycles`]), mis en attente pour être livrés
    /// juste après `0xF1` plutôt qu'avant. Sur le vrai HD6301, le
    /// contrôleur exécute son autotest et ne scanne pas le clavier durant
    /// ce délai ; livrer un scancode avant `0xF1` fait croire au logiciel
    /// hôte (ex. la cartouche de diagnostic, test K1) que le clavier ne
    /// répond pas correctement, et le fait basculer en mode RS232 de
    /// secours ("clavier HS").
    pending_during_reset: VecDeque<u8>,
}

impl Ikbd {
    pub fn new() -> Self {
        Ikbd {
            tx_queue: VecDeque::new(),
            cmd_buf: Vec::new(),
            cmd_remaining: 0,
            mouse_x: 0,
            mouse_y: 0,
            mouse_buttons: 0,
            y_axis: 1,
            // Autotest de mise sous tension : différé comme un reset logiciel
            // (voir la doc de IKBD_RESET_CYCLES), pas disponible dès le cycle 0.
            reset_pending_cycles: Some(IKBD_RESET_CYCLES),
            pending_during_reset: VecDeque::new(),
        }
    }

    /// Fait progresser le délai de réponse au reset, s'il y en a un en
    /// cours. À appeler une fois par tick de bus avec le nombre de cycles
    /// écoulés, avant [`Self::pop_tx`].
    pub fn tick(&mut self, cycles: u32) {
        if let Some(remaining) = self.reset_pending_cycles {
            if cycles >= remaining {
                self.reset_pending_cycles = None;
                self.tx_queue.push_back(0xF1);
                self.tx_queue.extend(self.pending_during_reset.drain(..));
            } else {
                self.reset_pending_cycles = Some(remaining - cycles);
            }
        }
    }

    /// Retire le prochain octet à injecter dans l'ACIA (RX), s'il y en a un.
    pub fn pop_tx(&mut self) -> Option<u8> {
        self.tx_queue.pop_front()
    }

    /// Reçoit un octet de commande envoyé par le CPU (via l'émission de
    /// l'ACIA clavier).
    pub fn receive_cmd(&mut self, byte: u8) {
        if self.cmd_remaining > 0 {
            self.cmd_buf.push(byte);
            self.cmd_remaining -= 1;
            if self.cmd_remaining == 0 {
                self.execute_cmd();
            }
            return;
        }

        self.cmd_buf.clear();
        self.cmd_buf.push(byte);

        match byte {
            0x80 => self.cmd_remaining = 1, // reset : attend le paramètre 0x01
            0x07 => self.cmd_remaining = 1, // action des boutons souris
            0x08 => {}                       // mode souris relatif (pas de paramètre)
            0x09 => self.cmd_remaining = 4, // mode souris absolu
            0x0A => self.cmd_remaining = 2, // touches clavier pour la souris
            0x0B => self.cmd_remaining = 2, // seuil souris
            0x0C => self.cmd_remaining = 2, // échelle souris
            0x0D => {}                       // interroge la position absolue (répond directement)
            0x0E => self.cmd_remaining = 5, // règle la position interne
            0x0F => self.y_axis = -1,       // Y=0 en bas
            0x10 => self.y_axis = 1,        // Y=0 en haut
            0x11 => {}                       // démarre la transmission clavier
            0x12 => {}                       // souris désactivée
            0x13 => {}                       // arrête la transmission clavier
            // 0x14-0x1A : commandes joystick — non modélisées (pas de
            // frontend manette), mais le compte d'octets de paramètres
            // doit rester correct pour ne pas désynchroniser le flux.
            0x14 | 0x15 | 0x16 | 0x18 | 0x1A => {}
            0x17 => self.cmd_remaining = 1,
            0x19 => self.cmd_remaining = 6,
            0x1B => self.cmd_remaining = 6, // règle l'horloge
            0x1C => {}                       // lit l'horloge
            0x20 => self.cmd_remaining = 3, // charge en mémoire
            0x21 => self.cmd_remaining = 2, // lit la mémoire
            0x22 => self.cmd_remaining = 2, // exécute
            _ => {}
        }

        if self.cmd_remaining == 0 {
            self.execute_cmd();
        }
    }

    fn execute_cmd(&mut self) {
        match self.cmd_buf[0] {
            0x80 => {
                // Reset logiciel : commande 0x80 + paramètre 0x01.
                if self.cmd_buf.get(1) == Some(&0x01) {
                    self.mouse_buttons = 0;
                    self.y_axis = 1;
                    self.reset_pending_cycles = Some(IKBD_RESET_CYCLES);
                }
            }
            0x0D => {
                // Interroge la position absolue → 0xF7 + boutons + x(2) + y(2).
                let x = self.mouse_x as u16;
                let y = self.mouse_y as u16;
                self.tx_queue.push_back(0xF7);
                self.tx_queue.push_back(self.mouse_buttons);
                self.tx_queue.push_back((x >> 8) as u8);
                self.tx_queue.push_back(x as u8);
                self.tx_queue.push_back((y >> 8) as u8);
                self.tx_queue.push_back(y as u8);
            }
            _ => {}
        }
        self.cmd_buf.clear();
    }

    // ── Événements venant de l'hôte ─────────────────────────────────────

    /// Signale l'appui d'une touche (make). `scancode` est le scancode IKBD Atari.
    pub fn key_make(&mut self, scancode: u8) {
        self.push_output(scancode);
    }

    /// Signale le relâchement d'une touche (break). Code = `0x80 | make`.
    pub fn key_break(&mut self, scancode: u8) {
        self.push_output(0x80 | scancode);
    }

    /// Route un octet clavier/souris vers `tx_queue`, ou vers
    /// `pending_during_reset` si un reset est en cours (voir la doc de ce
    /// champ) pour qu'il n'arrive jamais avant le `0xF1` d'autotest.
    fn push_output(&mut self, byte: u8) {
        if self.reset_pending_cycles.is_some() {
            self.pending_during_reset.push_back(byte);
        } else {
            self.tx_queue.push_back(byte);
        }
    }

    /// Signale un mouvement relatif de la souris et l'état des boutons.
    pub fn mouse_move(&mut self, dx: i8, dy: i8, buttons: u8) {
        let buttons_changed = buttons != self.mouse_buttons;
        self.mouse_buttons = buttons;
        let eff_dy = if self.y_axis < 0 { dy.wrapping_neg() } else { dy };
        self.mouse_x = (self.mouse_x + dx as i32).clamp(0, 639);
        self.mouse_y = (self.mouse_y + eff_dy as i32).clamp(0, 399);
        if dx == 0 && dy == 0 && !buttons_changed {
            return;
        }
        self.push_output(0xF8 | (buttons & 0x03));
        self.push_output(dx as u8);
        self.push_output(eff_dy as u8);
    }
}

impl Default for Ikbd {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn drain(ikbd: &mut Ikbd) -> Vec<u8> {
        let mut out = Vec::new();
        while let Some(b) = ikbd.pop_tx() {
            out.push(b);
        }
        out
    }

    #[test]
    fn reset_response_est_differee_pas_immediate() {
        let mut ikbd = Ikbd::new();
        ikbd.tick(IKBD_RESET_CYCLES - 1);
        assert!(drain(&mut ikbd).is_empty(), "0xF1 ne doit pas arriver avant le délai complet");
        ikbd.tick(1);
        assert_eq!(drain(&mut ikbd), vec![0xF1]);
    }

    #[test]
    fn touche_pressee_pendant_un_reset_arrive_apres_0xf1_pas_avant() {
        // Reproduit le scénario cartouche de diagnostic (test K1) : une
        // touche pressée pendant la fenêtre de reset ne doit jamais
        // devancer le 0xF1 d'autotest, sous peine de faire croire au test
        // que le clavier ne répond pas (bascule RS232 "clavier HS").
        let mut ikbd = Ikbd::new();
        ikbd.tick(IKBD_RESET_CYCLES / 2);
        ikbd.key_make(0x1E); // touche 'A' pressée en plein milieu du reset
        assert!(
            drain(&mut ikbd).is_empty(),
            "le scancode ne doit pas être livré avant la fin du reset"
        );
        ikbd.tick(IKBD_RESET_CYCLES / 2);
        assert_eq!(drain(&mut ikbd), vec![0xF1, 0x1E]);
    }

    #[test]
    fn commande_reset_relance_le_delai() {
        let mut ikbd = Ikbd::new();
        ikbd.tick(IKBD_RESET_CYCLES);
        drain(&mut ikbd);

        ikbd.receive_cmd(0x80);
        ikbd.receive_cmd(0x01);
        assert!(drain(&mut ikbd).is_empty(), "0xF1 doit de nouveau être différé après un reset logiciel");
        ikbd.tick(IKBD_RESET_CYCLES);
        assert_eq!(drain(&mut ikbd), vec![0xF1]);
    }

    #[test]
    fn paquet_mouvement_relatif_format_standard() {
        let mut ikbd = Ikbd::new();
        ikbd.tick(IKBD_RESET_CYCLES);
        drain(&mut ikbd);
        ikbd.mouse_move(5, -3, 0b01);
        assert_eq!(drain(&mut ikbd), vec![0xF9, 5, (-3i8) as u8]);
    }

    #[test]
    fn aucun_paquet_si_rien_ne_change() {
        let mut ikbd = Ikbd::new();
        ikbd.tick(IKBD_RESET_CYCLES);
        drain(&mut ikbd);
        ikbd.mouse_move(0, 0, 0);
        assert!(drain(&mut ikbd).is_empty());
    }

    #[test]
    fn axe_y_inverse_par_commande_0x0f() {
        let mut ikbd = Ikbd::new();
        ikbd.tick(IKBD_RESET_CYCLES);
        drain(&mut ikbd);
        ikbd.receive_cmd(0x0F); // Y=0 en bas
        ikbd.mouse_move(0, 10, 0);
        assert_eq!(drain(&mut ikbd), vec![0xF8, 0, (-10i8) as u8]);
    }

    #[test]
    fn interrogation_position_absolue_0x0d() {
        let mut ikbd = Ikbd::new();
        ikbd.mouse_move(100, 50, 0b11);
        drain(&mut ikbd);
        ikbd.receive_cmd(0x0D);
        assert_eq!(drain(&mut ikbd), vec![0xF7, 0b11, 0x00, 100, 0x00, 50]);
    }

    #[test]
    fn commande_joystick_avec_parametres_ne_desynchronise_pas_la_suite() {
        let mut ikbd = Ikbd::new();
        ikbd.tick(IKBD_RESET_CYCLES);
        drain(&mut ikbd);
        // 0x19 attend 6 octets de paramètres (curseur joystick) — non
        // modélisé, mais doit bien être absorbé pour que la commande
        // suivante (interrogation position absolue) soit lue correctement.
        ikbd.receive_cmd(0x19);
        for b in 0..6 {
            ikbd.receive_cmd(b);
        }
        ikbd.mouse_move(1, 1, 0);
        drain(&mut ikbd);
        ikbd.receive_cmd(0x0D);
        assert_eq!(drain(&mut ikbd), vec![0xF7, 0, 0x00, 1, 0x00, 1]);
    }
}
