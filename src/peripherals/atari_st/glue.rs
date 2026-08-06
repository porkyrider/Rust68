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
//! - Numérotation de ligne alignée sur celle de Hatari : `current_line()`
//!   est une position ABSOLUE dans la trame (0..LINES_PER_FRAME), incluant
//!   un vrai blanking haut (`VideoMode::frame_start_line()`, 63 en PAL/34
//!   en NTSC) avant la fenêtre visible nominale. `display_line()` donne
//!   l'index dans le framebuffer (`None` en blanking/bordure).
//! - Overscan vertical (haut/bas) : `write_sync`/`read_sync` modélisent le
//!   registre `$FF820A` — un switch 50/60Hz survenant dans la bonne fenêtre
//!   de cycle près du haut ou du bas de la fenêtre visible étend celle-ci
//!   pour la trame en cours (`display_start`/`display_end`), à la manière
//!   du overscan vertical façon Hatari. Simplification : une fois
//!   déclenchée pour une trame, l'extension n'est jamais annulée par une
//!   écriture ultérieure qui reviendrait en arrière (Hatari gère quelques
//!   cas d'annulation plus fins, non modélisés ici). L'overscan horizontal
//!   (bordure gauche/droite, `$FF8260`) n'est pas géré par `Glue` mais par
//!   `Shifter` (voir son propre module).

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

    /// Cycle de début d'affichage actif (DE) dans la ligne — seuil de
    /// "gating" des écritures `$FF8264`/`$FF8265` (voir la doc de
    /// [`Glue::line_start_cycle`]) : valeurs Hatari (`video.h`,
    /// `LINE_START_CYCLE_50`/`_60`).
    fn line_start_cycle(self) -> u32 {
        match self {
            VideoMode::Pal50 => 56,
            VideoMode::Ntsc60 => 52,
        }
    }

    /// Cycle de fin d'affichage actif (DE) dans la ligne — seuil de
    /// "gating" de l'écriture `$FF820F` (voir la doc de
    /// [`Glue::line_end_cycle`]) : valeurs Hatari (`video.h`,
    /// `LINE_END_CYCLE_50`/`_60`).
    fn line_end_cycle(self) -> u32 {
        match self {
            VideoMode::Pal50 => 376,
            VideoMode::Ntsc60 => 372,
        }
    }

    /// Nombre de lignes réellement affichées (identique PAL/NTSC — seule la
    /// durée du blanking vertical qui suit diffère). Voir la doc de
    /// [`Glue::vbl_edge_count`] pour pourquoi cette frontière, pas le
    /// bouclage complet de [`Self::lines_per_frame`], est ce qui déclenche
    /// VBL sur silicium réel.
    fn visible_lines(self) -> u32 {
        200
    }

    /// Première ligne ABSOLUE (0..lignes par trame) où l'affichage actif
    /// démarre normalement — valeurs Hatari (`video.h`,
    /// `VIDEO_START_HBL_50HZ`/`_60HZ`) : un vrai blanking vertical HAUT de
    /// 63 lignes (PAL) précède la première ligne visible, pas juste le
    /// blanking BAS de fin de trame déjà modélisé. Nécessaire pour la
    /// suppression de bordure haute STE (voir
    /// `peripherals::atari_st::shifter`) : sans cette période "avant" la
    /// ligne normalement visible, il n'y a tout simplement aucune ligne à
    /// révéler. Le blanking bas correspondant se déduit de
    /// `lines_per_frame() - frame_start_line() - visible_lines()` (50 en
    /// PAL, 63-313+200... voir [`Glue::display_line`]) — la SOMME des deux
    /// blancs (113 en PAL) reste celle déjà modélisée avant cette
    /// refonte, seule leur répartition haut/bas change.
    fn frame_start_line(self) -> u32 {
        match self {
            VideoMode::Pal50 => 63,
            VideoMode::Ntsc60 => 34,
        }
    }

    /// Lignes supplémentaires révélées par une suppression de bordure
    /// basse réussie — valeurs Hatari (`video.h`, `VIDEO_HEIGHT_BOTTOM_50HZ`/
    /// `_60HZ`). Voir [`Glue::write_sync`].
    fn bottom_border_extra_lines(self) -> u32 {
        match self {
            VideoMode::Pal50 => 47,
            VideoMode::Ntsc60 => 26,
        }
    }
}

/// Cycle limite, dans la ligne, pour qu'un switch 50/60Hz (`$FF820A`)
/// proche du haut ou du bas de la fenêtre affichable déclenche une
/// suppression de bordure — valeur Hatari (`video.h`,
/// `LINE_REMOVE_TOP_CYCLE`/`LINE_REMOVE_BOTTOM_CYCLE`, ~504 STF/500 STE ;
/// une seule valeur retenue ici, l'écart de 4 cycles entre variantes de
/// silicium n'étant pas vérifiable sans référence matérielle, voir les
/// limitations de module). Réutilisée telle quelle pour PAL et NTSC faute
/// de valeur NTSC distincte trouvée en recherche.
const LINE_REMOVE_BORDER_CYCLE: u32 = 504;

/// État du générateur de timing HBL/VBL.
#[derive(Debug, Clone)]
pub struct Glue {
    mode: VideoMode,
    cycles_in_line: u32,
    line: u32,
    frame: u64,
    hbl_pending: bool,
    vbl_pending: bool,
    vbl_edges: u64,
    /// Fenêtre [début, fin) de lignes ABSOLUES affichées pour la trame EN
    /// COURS — par défaut `frame_start_line()..frame_start_line()+visible_lines()`,
    /// remise à ce défaut à chaque nouvelle trame (voir [`Self::tick`]).
    /// Ajustable par [`Self::write_sync`] (suppression de bordure
    /// haute/basse STE) pour la trame en cours.
    display_start: u32,
    display_end: u32,
    /// Registre `$FF820A` brut (bit 1 = sélection 50/60Hz externe) — voir
    /// [`Self::write_sync`].
    sync: u8,
}

impl Glue {
    pub fn new(mode: VideoMode) -> Self {
        let display_start = mode.frame_start_line();
        let display_end = display_start + mode.visible_lines();
        Glue {
            mode,
            cycles_in_line: 0,
            line: 0,
            frame: 0,
            hbl_pending: false,
            vbl_pending: false,
            vbl_edges: 0,
            display_start,
            display_end,
            sync: 0,
        }
    }

    /// Avance le générateur de `cpu_cycles` cycles CPU, armant HBL à chaque
    /// fin de ligne et VBL à la transition ligne visible -> blanking
    /// vertical (voir la doc de [`Self::vbl_edge_count`]) — PAS au bouclage
    /// complet de la trame (`line` reprenant à 0), qui reste réservé à
    /// [`Self::frame_count`]. La fenêtre affichable (`display_start`/`_end`)
    /// est remise à sa valeur nominale à chaque nouvelle trame — toute
    /// suppression de bordure haute/basse ne vaut que pour la trame où elle
    /// a été déclenchée, jamais les suivantes (comportement réel du
    /// silicium, confirmé par Hatari : recalculé à chaque VBL).
    pub fn tick(&mut self, cpu_cycles: u32) {
        self.cycles_in_line += cpu_cycles;
        let per_line = self.mode.cycles_per_line();
        while self.cycles_in_line >= per_line {
            self.cycles_in_line -= per_line;
            self.hbl_pending = true;
            self.line += 1;
            if self.line == self.display_end {
                self.vbl_pending = true;
                self.vbl_edges += 1;
            }
            if self.line >= self.mode.lines_per_frame() {
                self.line = 0;
                self.frame += 1;
                self.display_start = self.mode.frame_start_line();
                self.display_end = self.display_start + self.mode.visible_lines();
            }
        }
    }

    /// Index de ligne AFFICHÉE (0..lignes visibles de la trame courante) si
    /// la ligne balayée absolue courante ([`Self::current_line`]) est dans
    /// la fenêtre affichable — `None` sinon (blanking haut ou bas). C'est
    /// cet index, pas [`Self::current_line`] directement, qu'il faut
    /// utiliser pour indexer `AtariSt::framebuffer` : la fenêtre affichable
    /// peut démarrer avant la position nominale (suppression de bordure
    /// haute) ou se terminer après (bordure basse), voir
    /// `peripherals::atari_st::shifter`.
    pub fn display_line(&self) -> Option<u32> {
        self.display_index(self.line)
    }

    /// Comme [`Self::display_line`], mais pour une ligne balayée ABSOLUE
    /// arbitraire plutôt que forcément la courante — utile à
    /// `AtariSt::tick` pour son rattrapage éventuel de plusieurs lignes en
    /// un seul appel (voir la doc du champ `display_start`/`display_end`) :
    /// n'est exact que si toutes les lignes rattrapées appartiennent à la
    /// même trame que celle dont la fenêtre affichable est actuellement
    /// mémorisée ici — cas normal en usage réel (un `tick()` ne couvre
    /// jamais qu'une poignée de cycles CPU, largement moins qu'une trame
    /// entière), pas garanti pour un `tick()` de test couvrant délibérément
    /// plusieurs trames d'un coup.
    pub fn display_index(&self, absolute_line: u32) -> Option<u32> {
        if absolute_line >= self.display_start && absolute_line < self.display_end {
            Some(absolute_line - self.display_start)
        } else {
            None
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

    /// Nombre de fronts VBL survenus depuis la création/le dernier reset —
    /// à la transition ligne visible -> blanking vertical (voir la doc de
    /// [`Self::tick`]), PAS au bouclage complet de la trame.
    ///
    /// Distinct de [`Self::frame_count`] (qui, lui, avance seulement au
    /// bouclage complet) : sur silicium réel, VBL survient au DÉBUT du
    /// blanking vertical (juste après la dernière ligne visible), avec tout
    /// le reste du blanking (~113 lignes en PAL) qui s'écoule ENSUITE avant
    /// que la ligne 0 de la trame suivante ne soit affichée. Le board
    /// ([`crate::systems::atari_st::AtariSt::tick`]) doit détecter ce
    /// changement (pas celui de `frame_count`) pour recharger le compteur
    /// vidéo du Shifter depuis sa base — sinon celui-ci ne redémarrerait
    /// qu'au bouclage 0, dans le MÊME appel `tick()` qui rendrait déjà la
    /// ligne visible 0 de la trame suivante, sans laisser au logiciel la
    /// moindre chance de prendre l'interruption VBL avant que cette ligne
    /// ne soit déjà rendue — confirmé nécessaire par la cartouche de
    /// diagnostic usine STe (test "T4 Video Counter in Memory Controller",
    /// qui échouait systématiquement d'exactement une ligne vidéo à sa toute
    /// première lecture après reprogrammation de la base).
    pub fn vbl_edge_count(&self) -> u64 {
        self.vbl_edges
    }

    /// Nombre de lignes par trame dans le mode vidéo courant — utile pour
    /// détecter le bouclage de `current_line()` d'un tick à l'autre.
    pub fn lines_per_frame(&self) -> u32 {
        self.mode.lines_per_frame()
    }

    /// Position en cycles CPU dans la ligne balayée courante (0..cycles/ligne
    /// du mode courant) — nécessaire pour le "gating" cycle-exact des
    /// écritures de registres Shifter STE (`$FF8264`/`$FF8265`/`$FF820F`,
    /// voir `peripherals::atari_st::shifter`) : une écriture avant le début
    /// d'affichage actif de la ligne courante s'y applique immédiatement,
    /// après elle est différée à la ligne suivante — exactement le
    /// mécanisme `New*`/staging de Hatari (`video.c`,
    /// `Video_HorScroll_Write`/`Video_LineWidth_WriteByte`).
    pub fn cycles_in_line(&self) -> u32 {
        self.cycles_in_line
    }

    /// Cycle de début d'affichage actif (DE) dans la ligne, dans le mode
    /// vidéo courant — voir [`Self::cycles_in_line`].
    pub fn line_start_cycle(&self) -> u32 {
        self.mode.line_start_cycle()
    }

    /// Cycle de fin d'affichage actif (DE) dans la ligne, dans le mode
    /// vidéo courant — voir [`Self::cycles_in_line`].
    pub fn line_end_cycle(&self) -> u32 {
        self.mode.line_end_cycle()
    }

    /// Registre `$FF820A` brut (bit 1 = sélection 50/60Hz externe).
    pub fn read_sync(&self) -> u8 {
        self.sync
    }

    /// Écrit `$FF820A` (bit 1 = sélection 50/60Hz externe) — au passage,
    /// détecte la suppression de bordure haute/basse STE : un switch VERS
    /// 60Hz survenant à un cycle précis près du haut ou du bas de la
    /// fenêtre affichable nominale étend celle-ci pour LA TRAME EN COURS
    /// (voir Hatari, `video.c`, `Video_Update_Glue_State`). Seul un switch
    /// vers 60Hz peut agrandir la fenêtre (le mode nominal de la machine
    /// reste [`Self::mode`], jamais changé par cette écriture elle-même —
    /// seule la fenêtre affichée pour cette trame précise l'est) :
    /// - **Bordure haute** : le switch survient encore dans le blanking
    ///   haut nominal (`self.line < frame_start_line()`) — la fenêtre
    ///   démarre alors à la position de départ NOMINALE du mode 60Hz (34),
    ///   révélant les lignes entre les deux.
    /// - **Bordure basse** : le switch survient sur l'avant-dernière ou la
    ///   dernière ligne affichée nominale — la fenêtre s'étend de
    ///   [`VideoMode::bottom_border_extra_lines`] lignes supplémentaires.
    ///
    /// Simplification assumée par rapport à Hatari : pas de mécanisme
    /// d'ANNULATION si un switch ultérieur revient sur la décision (une
    /// fois déclenchée pour cette trame, l'extension reste acquise jusqu'à
    /// la prochaine trame) — capture le cas courant (un seul switch bien
    /// placé par trame), pas les séquences multi-écritures les plus
    /// exotiques.
    pub fn write_sync(&mut self, value: u8) {
        self.sync = value;
        let selecting_60hz = value & 0x02 == 0;
        if !selecting_60hz || self.cycles_in_line > LINE_REMOVE_BORDER_CYCLE {
            return;
        }
        let nominal_start = self.mode.frame_start_line();
        if self.line < nominal_start {
            let alt_start = VideoMode::Ntsc60.frame_start_line();
            if alt_start < self.display_start {
                self.display_start = alt_start;
            }
            return;
        }
        let nominal_end = nominal_start + self.mode.visible_lines();
        if self.line + 1 >= nominal_end && self.line < nominal_end {
            let extended = nominal_end + self.mode.bottom_border_extra_lines();
            self.display_end = self.display_end.max(extended);
        }
    }
}
