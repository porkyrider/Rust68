//! GLUE (« General Logic Unit ») — Atari ST.
//!
//! Puce custom qui génère, entre autres fonctions, les deux interruptions
//! vidéo autovectorisées du système : HBL (horizontal blank, fin de chaque
//! ligne balayée) sur **IPL2**, et VBL (vertical blank, fin de trame) sur
//! **IPL4**. C'est ce qui rythme l'affichage — TOS utilise VBL pour sa
//! file d'attente vbl (défilement, changement de palette par ligne via
//! HBL, lecture clavier périodique…).
//!
//! Comme [`crate::peripherals::mfp`], ce module modélise le signal de
//! timing **seul** : c'est au board ([`crate::systems::atari_st::AtariSt`])
//! de brancher [`Glue::hbl_pending`]/[`Glue::vbl_pending`] sur
//! `Bus::irq_level` (IPL2/IPL4, priorité en dessous du MFP sur IPL6) et
//! [`Glue::ack_hbl`]/[`Glue::ack_vbl`] sur `Bus::irq_ack`.
//!
//! ## Limitations connues (v1)
//! - Uniquement le timing HBL/VBL : GLUE gère aussi en réalité une partie
//!   du décodage mémoire/bus (rôle partagé avec la MMU), non modélisé ici.
//! - Constantes de timing (cycles/ligne, lignes/trame) : valeurs usuelles
//!   citées par la communauté d'émulation (Hatari/WinSTon), pas vérifiées
//!   contre une référence matérielle formelle (aucune suite de test
//!   équivalente à TomHarte n'existe pour ce composant).
//! - Pas de distinction fine entre lignes visibles / overscan (juste un
//!   compteur de ligne linéaire 0..LINES_PER_FRAME).

/// Mode vidéo : détermine le rythme HBL/VBL. Le ST/STE fonctionne à 8 MHz
/// CPU quel que soit le mode ; seul le nombre de cycles par ligne/lignes
/// par trame change selon la norme de diffusion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VideoMode {
    /// 50 Hz, 313 lignes/trame, 512 cycles CPU/ligne (le plus courant en
    /// Europe — valeurs usuelles Hatari/WinSTon).
    Pal50,
    /// 60 Hz, 263 lignes/trame, 508 cycles CPU/ligne.
    Ntsc60,
}

impl VideoMode {
    fn cycles_per_line(self) -> u32 {
        match self {
            VideoMode::Pal50 => 512,
            VideoMode::Ntsc60 => 508,
        }
    }

    fn lines_per_frame(self) -> u32 {
        match self {
            VideoMode::Pal50 => 313,
            VideoMode::Ntsc60 => 263,
        }
    }
}

/// État du générateur de timing HBL/VBL.
#[derive(Debug, Clone)]
pub struct Glue {
    mode: VideoMode,
    cycles_in_line: u32,
    line: u32,
    frame: u64,
    hbl_pending: bool,
    vbl_pending: bool,
}

impl Glue {
    pub fn new(mode: VideoMode) -> Self {
        Glue {
            mode,
            cycles_in_line: 0,
            line: 0,
            frame: 0,
            hbl_pending: false,
            vbl_pending: false,
        }
    }

    /// Avance le générateur de `cpu_cycles` cycles CPU, armant HBL à
    /// chaque fin de ligne et VBL à chaque fin de trame.
    pub fn tick(&mut self, cpu_cycles: u32) {
        self.cycles_in_line += cpu_cycles;
        let per_line = self.mode.cycles_per_line();
        while self.cycles_in_line >= per_line {
            self.cycles_in_line -= per_line;
            self.hbl_pending = true;
            self.line += 1;
            if self.line >= self.mode.lines_per_frame() {
                self.line = 0;
                self.frame += 1;
                self.vbl_pending = true;
            }
        }
    }

    /// Vrai si un HBL est en attente d'acquittement (IPL2).
    pub fn hbl_pending(&self) -> bool {
        self.hbl_pending
    }

    /// Vrai si un VBL est en attente d'acquittement (IPL4).
    pub fn vbl_pending(&self) -> bool {
        self.vbl_pending
    }

    /// Acquitte le HBL en cours (à appeler depuis `Bus::irq_ack` pour le
    /// niveau 2).
    pub fn ack_hbl(&mut self) {
        self.hbl_pending = false;
    }

    /// Acquitte le VBL en cours (à appeler depuis `Bus::irq_ack` pour le
    /// niveau 4).
    pub fn ack_vbl(&mut self) {
        self.vbl_pending = false;
    }

    /// Ligne balayée courante (0..lignes par trame).
    pub fn current_line(&self) -> u32 {
        self.line
    }

    /// Nombre de trames complètes écoulées depuis la création/le dernier
    /// reset — utile pour cadencer un rendu vidéo externe.
    pub fn frame_count(&self) -> u64 {
        self.frame
    }
}
