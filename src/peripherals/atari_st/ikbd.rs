//! Émulation du contrôleur IKBD (HD6301) clavier/souris/joystick.
//!
//! Sur ST/STE réel, le HD6301 dialogue avec le CPU via l'ACIA clavier
//! (`AtariSt::acia_keyboard`) à 7812,5 bauds. Protocole (sortant, IKBD →
//! CPU) :
//! - Frappe clavier : `0xNN` (scancode 0x01-0x72)
//! - Relâchement : `0x80 | 0xNN`
//! - Mouvement souris relatif (mode par défaut, `$08`) : `0xF8|boutons,
//!   dx, dy` — envoyé automatiquement à chaque mouvement.
//! - Position souris absolue (mode `$09`, voir [`Ikbd::mouse_mode_absolute`]) :
//!   `0xF7, boutons, xmsb, xlsb, ymsb, ylsb` — **PAS envoyé automatiquement
//!   sur mouvement** (comportement réel du silicium, confirmé contre
//!   Hatari) : seulement en réponse à `$0D`, ou sur appui/relâchement de
//!   bouton si `$07` l'a demandé.
//!
//! Commandes (entrant, CPU → IKBD via l'émission de l'ACIA) : `0x80 0x01`
//! (reset), `0x07` (action souris), `0x08`/`0x09` (mode relatif/absolu),
//! `0x0D` (interroge la position absolue), `0x0E` (charge la position
//! interne), etc. — voir [`Ikbd::receive_cmd`].
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
    /// `true` si la souris est en mode ABSOLU (`$09`), `false` = relatif
    /// (`$08`, par défaut) — voir la doc de [`Self::mouse_move`].
    /// **Bug réel corrigé** : `$09` était reconnu (bon nombre d'octets de
    /// paramètre consommés) mais totalement ignoré — la souris restait en
    /// mode relatif pour toujours, envoyant des paquets `0xF8` que GEM,
    /// une fois basculé en mode absolu pour une boîte de dialogue modale
    /// (ex: Bureau > Informations), n'attend plus du tout — désynchronisant
    /// son analyseur de flux série et produisant un mouvement de curseur
    /// en apparence "tourné" (les octets `dx`/`dy` mal réinterprétés) tant
    /// que la boîte de dialogue reste ouverte. Confirmé contre Hatari
    /// (`ikbd.c`, `IKBD_Cmd_AbsMouseMode`/`IKBD_SendAutoKeyboardCommands` :
    /// en mode absolu, silicium réel n'envoie JAMAIS de paquet automatique
    /// sur mouvement — seulement sur interrogation `$0D`, ou sur
    /// appui/relâchement de bouton si `$07` l'a demandé).
    mouse_mode_absolute: bool,
    /// Bornes courantes (`$09`, MSB en premier) du mode absolu — aussi
    /// utilisées comme bornes de blocage de [`Self::mouse_x`]/`mouse_y` en
    /// PERMANENCE (silicium réel : une seule position interne suivie en
    /// continu, bornée par ces limites, quel que soit le mode de rapport
    /// actif — voir Hatari, `IKBD_UpdateInternalMousePosition`). Valeurs
    /// par défaut inchangées par rapport au comportement historique
    /// (639/399) tant qu'aucune commande `$09` n'a encore été reçue.
    abs_max_x: u16,
    abs_max_y: u16,
    /// Dernier octet de paramètre de la commande `$07` ("mouse action") —
    /// bits 0-1 : rapporter la position absolue sur appui/relâchement de
    /// bouton (seul mécanisme de rapport AUTOMATIQUE en mode absolu, voir
    /// [`Self::mouse_mode_absolute`]).
    mouse_action: u8,

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
            mouse_mode_absolute: false,
            abs_max_x: 639,
            abs_max_y: 399,
            mouse_action: 0,
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
                    self.mouse_mode_absolute = false;
                    self.reset_pending_cycles = Some(IKBD_RESET_CYCLES);
                }
            }
            // Action souris ($07) : bits 0-1 = rapporter la position
            // absolue sur appui/relâchement de bouton (SEUL mécanisme de
            // rapport automatique en mode absolu, voir
            // `Self::mouse_mode_absolute`) — reste sans effet en mode
            // relatif (déjà rapporté sur chaque mouvement).
            0x07 => self.mouse_action = self.cmd_buf.get(1).copied().unwrap_or(0),
            // Mode relatif ($08) : pas de paramètre.
            0x08 => self.mouse_mode_absolute = false,
            // Mode absolu ($09) : bornes MaxX/MaxY, MSB en premier — voir
            // la doc de `Self::mouse_mode_absolute`. Ne touche PAS
            // `mouse_x`/`mouse_y` eux-mêmes (silicium réel : le bornage ne
            // s'applique qu'au PROCHAIN mouvement, pas rétroactivement).
            0x09 => {
                self.mouse_mode_absolute = true;
                self.abs_max_x = ((self.cmd_buf[1] as u16) << 8) | self.cmd_buf[2] as u16;
                self.abs_max_y = ((self.cmd_buf[3] as u16) << 8) | self.cmd_buf[4] as u16;
            }
            // Interroge la position absolue → 0xF7 + boutons + x(2) + y(2).
            0x0D => {
                let bytes = self.abs_report_bytes();
                self.tx_queue.extend(bytes);
            }
            // Charge la position interne ($0E) : octet de remplissage +
            // X(2)/Y(2), MSB en premier — GEM s'en sert typiquement juste
            // après $09 pour recentrer le curseur dans les bornes de la
            // boîte de dialogue avant que l'utilisateur ne bouge la souris.
            0x0E => {
                let x = ((self.cmd_buf[2] as u16) << 8) | self.cmd_buf[3] as u16;
                let y = ((self.cmd_buf[4] as u16) << 8) | self.cmd_buf[5] as u16;
                self.mouse_x = x as i32;
                self.mouse_y = y as i32;
            }
            _ => {}
        }
        self.cmd_buf.clear();
    }

    /// Les 6 octets du rapport de position absolue (`0xF7` + boutons + x(2)
    /// + y(2), MSB en premier) — partagé entre la réponse à `$0D` et le
    /// rapport automatique sur bouton en mode absolu (voir
    /// [`Self::mouse_move`]).
    fn abs_report_bytes(&self) -> [u8; 6] {
        let x = self.mouse_x as u16;
        let y = self.mouse_y as u16;
        [0xF7, self.mouse_buttons, (x >> 8) as u8, x as u8, (y >> 8) as u8, y as u8]
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
        // Position interne suivie en PERMANENCE, bornée par `abs_max_x`/`_y`
        // — silicium réel, quel que soit le mode de rapport actif (voir la
        // doc de `Self::abs_max_x`).
        self.mouse_x = (self.mouse_x + dx as i32).clamp(0, self.abs_max_x as i32);
        self.mouse_y = (self.mouse_y + eff_dy as i32).clamp(0, self.abs_max_y as i32);
        if self.mouse_mode_absolute {
            // Mode absolu : AUCUN rapport automatique sur mouvement
            // (silicium réel) — seulement sur appui/relâchement de bouton
            // si `$07` l'a demandé (bits 0-1), le reste passe par une
            // interrogation `$0D` explicite. Voir la doc de
            // `Self::mouse_mode_absolute`.
            if buttons_changed && self.mouse_action & 0x03 != 0 {
                let bytes = self.abs_report_bytes();
                for b in bytes {
                    self.push_output(b);
                }
            }
            return;
        }
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

    // --- Mode absolu ($09), bug réel corrigé ------------------------------

    fn send_cmd(ikbd: &mut Ikbd, bytes: &[u8]) {
        for &b in bytes {
            ikbd.receive_cmd(b);
        }
    }

    #[test]
    fn mode_absolu_n_envoie_aucun_paquet_automatique_sur_mouvement() {
        // Cœur du bug corrigé : silicium réel, en mode absolu, n'envoie
        // JAMAIS de paquet automatique sur mouvement (ni `0xF8` relatif, ni
        // `0xF7` absolu) — seulement sur interrogation `$0D` ou sur
        // bouton si `$07` l'a demandé. Une régression qui réintroduirait
        // un envoi automatique ici recréerait exactement le bug GEM
        // (curseur "tourné" pendant qu'une boîte de dialogue modale est
        // ouverte).
        let mut ikbd = Ikbd::new();
        ikbd.tick(IKBD_RESET_CYCLES);
        drain(&mut ikbd);
        send_cmd(&mut ikbd, &[0x09, 0x03, 0x1F, 0x01, 0x8F]); // max_x=0x031F, max_y=0x018F
        drain(&mut ikbd); // la commande elle-même ne répond rien

        ikbd.mouse_move(5, -3, 0b01);
        assert!(drain(&mut ikbd).is_empty(), "aucun paquet automatique en mode absolu");
    }

    #[test]
    fn mode_absolu_suit_quand_meme_la_position_interrogeable_via_0x0d() {
        let mut ikbd = Ikbd::new();
        send_cmd(&mut ikbd, &[0x09, 0x03, 0x1F, 0x01, 0x8F]);
        drain(&mut ikbd);

        ikbd.mouse_move(10, 20, 0);
        ikbd.mouse_move(5, 5, 0);
        drain(&mut ikbd);

        ikbd.receive_cmd(0x0D);
        assert_eq!(drain(&mut ikbd), vec![0xF7, 0, 0x00, 15, 0x00, 25]);
    }

    #[test]
    fn mode_absolu_borne_la_position_a_max_x_max_y() {
        let mut ikbd = Ikbd::new();
        send_cmd(&mut ikbd, &[0x09, 0x00, 0x0A, 0x00, 0x05]); // max_x=10, max_y=5
        drain(&mut ikbd);

        ikbd.mouse_move(100, 100, 0);
        drain(&mut ikbd);
        ikbd.receive_cmd(0x0D);
        assert_eq!(drain(&mut ikbd), vec![0xF7, 0, 0x00, 10, 0x00, 5], "bornée à max_x/max_y, pas 639/399");
    }

    #[test]
    fn mode_absolu_rapporte_automatiquement_sur_bouton_si_0x07_le_demande() {
        let mut ikbd = Ikbd::new();
        ikbd.tick(IKBD_RESET_CYCLES); // le rapport automatique passe par push_output, gaté pendant un reset
        send_cmd(&mut ikbd, &[0x09, 0x03, 0x1F, 0x01, 0x8F]);
        send_cmd(&mut ikbd, &[0x07, 0x03]); // action souris : bits 0-1 posés
        drain(&mut ikbd);

        ikbd.mouse_move(0, 0, 0b01); // changement de bouton, pas de mouvement
        assert_eq!(drain(&mut ikbd), vec![0xF7, 0b01, 0x00, 0, 0x00, 0]);
    }

    #[test]
    fn mode_absolu_sans_action_0x07_ne_rapporte_rien_sur_bouton() {
        let mut ikbd = Ikbd::new();
        ikbd.tick(IKBD_RESET_CYCLES);
        send_cmd(&mut ikbd, &[0x09, 0x03, 0x1F, 0x01, 0x8F]); // pas de $07
        drain(&mut ikbd);

        ikbd.mouse_move(0, 0, 0b01);
        assert!(drain(&mut ikbd).is_empty());
    }

    #[test]
    fn retour_en_mode_relatif_via_0x08_restaure_les_paquets_automatiques() {
        let mut ikbd = Ikbd::new();
        ikbd.tick(IKBD_RESET_CYCLES);
        send_cmd(&mut ikbd, &[0x09, 0x03, 0x1F, 0x01, 0x8F]);
        drain(&mut ikbd);
        ikbd.receive_cmd(0x08); // retour au mode relatif
        drain(&mut ikbd);

        ikbd.mouse_move(5, -3, 0b01);
        assert_eq!(drain(&mut ikbd), vec![0xF9, 5, (-3i8) as u8], "mode relatif restauré");
    }

    #[test]
    fn commande_0x0e_charge_directement_la_position_interne() {
        let mut ikbd = Ikbd::new();
        send_cmd(&mut ikbd, &[0x0E, 0x00, 0x00, 0x64, 0x00, 0x32]); // x=100, y=50
        drain(&mut ikbd);
        ikbd.receive_cmd(0x0D);
        assert_eq!(drain(&mut ikbd), vec![0xF7, 0, 0x00, 100, 0x00, 50]);
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
