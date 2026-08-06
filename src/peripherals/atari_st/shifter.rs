//! Shifter — puce vidéo de l'Atari ST/STE.
//!
//! Lit la RAM vidéo ligne par ligne (au rythme du HBL généré par
//! [`crate::peripherals::glue::Glue`]) et convertit les bits mémoire en
//! pixels RGB selon la résolution programmée : basse (320×200, 4 plans,
//! 16 couleurs), moyenne (640×200, 2 plans, 4 couleurs), haute (640×400,
//! 1 plan, monochrome — écran monochrome uniquement).
//!
//! Ce module modélise la puce **seule**, indépendamment de tout pipeline
//! d'affichage réel : [`Shifter::render_scanline`] prend une tranche de
//! RAM et renvoie des pixels RGB 24 bits, prêts à être posés dans n'importe
//! quel framebuffer hôte. C'est au board de :
//! - mapper [`Shifter::read`]/[`Shifter::write`] dans son `Bus`,
//! - appeler [`Shifter::write_palette_word`] pour tout accès `.W`/`.L` du
//!   CPU touchant un registre de palette (`$FF8240`-`$FF825E`) — contrairement
//!   à [`Shifter::write`], qui suppose systématiquement un accès `.B` isolé
//!   et duplique l'octet écrit dans les deux moitiés du mot, comportement
//!   réel du silicium mais faux pour une écriture `.W`/`.L` normale (voir la
//!   doc de [`Shifter::write`]),
//! - appeler [`Shifter::set_ste_palette`] juste après construction d'après le
//!   modèle de machine ([`crate::systems::atari_st::model::MachineProfile::ste_palette`]),
//! - appeler [`Shifter::start_frame`] à chaque VBL (recharge le compteur
//!   vidéo depuis l'adresse de base),
//! - appeler [`Shifter::render_scanline`] à chaque HBL en lui passant sa
//!   RAM (le Shifter est un second maître de bus, indépendant du CPU —
//!   d'où la contention DRAM/vidéo modélisée génériquement par
//!   `Bus::is_contended`/`TimedBus`, pas ici),
//! - router `$FF8264`/`$FF8265` (défilement fin STE) et `$FF820F`
//!   (largeur de ligne STE) vers [`Shifter::write_hscroll`]/
//!   [`Shifter::write_line_width`] en calculant lui-même `apply_now`
//!   d'après `Glue::cycles_in_line`/`line_start_cycle`/`line_end_cycle` —
//!   seul le board connaît la position en cycles dans la ligne, voir la
//!   doc de ces deux méthodes pour le "gating" cycle-exact exact.
//!
//! ## Défilement fin STE (cycle-exact, façon Hatari)
//! `$FF8264`/`$FF8265`/`$FF820F` sont modélisés fidèlement, y compris le
//! "gating" cycle-exact des écritures (immédiat sur la ligne en cours de
//! balayage si écrit avant le seuil pertinent, différé à la ligne suivante
//! sinon — voir [`Shifter::write_hscroll`]/[`Shifter::write_line_width`]).
//! Hatari lui-même ne fait PAS de rendu pixel-par-pixel en temps réel : il
//! horodate les écritures de registre pendant la ligne pour décider où
//! elles s'appliquent, puis convertit la ligne entière d'un coup à HBL —
//! exactement le modèle déjà en place ici ([`Shifter::render_scanline`]
//! appelé une fois par ligne), pas de refonte du pipeline nécessaire.
//!
//! Hors périmètre, délibérément (bugs/effets de démomaking spécifiques,
//! sans rapport avec le mécanisme de défilement lui-même — touchent la
//! détection de changement de résolution en cours de ligne et le suivi de
//! bordure par ligne, aucun état correspondant ici) :
//! - Le bug/trick "`$FF8265` puis `$FF8264`=0" (ligne à 336 pixels, utilisé
//!   par des démos précises comme Pacemaker/Obsession/Skulls).
//! - Variantes de timing selon révision de carte mère STF (WS1-WS4) — sans
//!   objet, seule une machine STE (un jeu de constantes fixe) est visée.
//!
//! ## Bordure horizontale / overscan STE (`$FF8260`/`$FF820A`, façon Hatari)
//! Modélisé via [`border`] : un `border_mask` accumulé PENDANT le balayage
//! de la ligne courante (écritures `$FF8260`/`$FF820A` routées par le board
//! vers [`Shifter::write_resolution`]/[`Shifter::write_sync`] avec la
//! position en cycles), consommé et remis à zéro en fin de
//! [`Shifter::render_scanline`] — même schéma que
//! `pending_h_scroll`/`pending_line_width`, mais sans report à la ligne
//! suivante (les tricks modélisés ici s'appliquent tous à la ligne où
//! l'écriture survient). Toute la logique de détection vit dans Hatari
//! (`video.c`) dans **une seule** fonction, `Video_Update_Glue_State`,
//! appelée à la fois pour `$FF8260` (`Video_WriteToGlueRes`) et `$FF820A`
//! (`Video_Sync_WriteByte`) — reproduit ici en deux méthodes séparées côté
//! `Shifter` par simplicité, mais les CONSTANTES DE CYCLE et les EFFETS EN
//! OCTETS (`BORDERBYTES_*`) sont repris tels quels de Hatari
//! (`video.h`/`video.c`, valeurs machine STE).
//!
//! Mécanismes modélisés d'après Hatari (v1, périmètre STE uniquement — la
//! variante ST d'origine du trick `LEFT_OFF`, qui nécessite un
//! "stabilisateur" moyenne résolution, n'est pas modélisée) :
//! - `LEFT_OFF_2_STE` : un bref passage en haute résolution très tôt dans
//!   la ligne (`cycles_in_line <= 4`) puis un retour en basse/moyenne
//!   résolution PILE au cycle 4 révèle +20 octets de bordure gauche
//!   (variante STE courte, sans stabilisateur — `video.c`, commentaire
//!   "no stabilizer needed").
//! - `RIGHT_OFF` : un switch `$FF820A` vers 60Hz dans la fenêtre de cycle
//!   `]372, 376]` (jonction entre les fins de ligne 60Hz/50Hz nominales)
//!   révèle +44 octets de bordure droite.
//! - `LEFT_PLUS_2`/`RIGHT_MINUS_2` : nudge ±2 octets (switch `$FF820A` vers
//!   60Hz très tôt dans la ligne, avant `cycles_in_line <= 36`). Chaque
//!   mécanisme (y compris `RIGHT_OFF`) a sa PROPRE fenêtre d'annulation sur
//!   un retour au 50Hz — reprise telle quelle de Hatari
//!   (`Video_Update_Glue_State`), voir la doc de
//!   [`Shifter::write_sync`] : `LEFT_PLUS_2` seulement si
//!   `cycles_in_line <= 52`, `RIGHT_MINUS_2`/`RIGHT_OFF` seulement si
//!   `cycles_in_line <= 376` (pour `RIGHT_OFF`, cette fenêtre est en
//!   pratique très étroite, quasiment confondue avec sa fenêtre de
//!   déclenchement).
//! - `STOP_MIDDLE` : switch hi-res en milieu de ligne (`]4, 164]`,
//!   `$FF8260`) raccourcit la ligne de 106 octets — annulable par un
//!   retour en basse/moyenne résolution tant que `cycles_in_line <= 164`
//!   (avant le "point de non-retour").
//! - `RIGHT_OFF_FULL` : switch hi-res plus tardif (`]164, 376]`,
//!   `$FF8260`) supprime TOTALEMENT la bordure droite (+44+22 octets) ET
//!   force `LEFT_OFF_2_STE` dès le DÉBUT de la ligne SUIVANTE (cascade
//!   inter-lignes, voir [`Shifter::pending_left_off_next_line`]) — une
//!   fois déclenché, pas annulable dans la même ligne (simplification :
//!   Hatari a une fenêtre d'annulation via `$FF820A`, non modélisée).
//!
//! ## Bordure gauche moyenne résolution / défilement matériel 4 bits
//! (façon Steem SSE, PAS Hatari)
//! `OVERSCAN_MED_RES`/`FOUR_BIT_SCROLL` (voir [`border`] et
//! [`Shifter::detect_med_res_tricks`]) affinent l'alignement d'une bordure
//! gauche déjà révélée par `LEFT_OFF_2_STE` (précondition : ne s'appliquent
//! QUE si `LEFT_OFF_2_STE` est déjà actif sur la ligne). Contrairement aux
//! mécanismes ci-dessus (qui réagissent à un SEUIL DE CYCLE fixe à chaque
//! écriture), ce sont des tricks à multiples changements de résolution
//! (`$FF8260` haute→moyenne→basse) dont Hatari lui-même modélise la
//! détection par une pile de cas particuliers nommés d'après les démos qui
//! les ont révélés ("No Cooper", "PYM", …) plutôt qu'une règle générale —
//! jugé trop fragile à porter tel quel (voir `ETAT.md`, section Shifter).
//! Steem SSE (autre émulateur ST/STE open source) modélise le MÊME trick
//! avec une approche différente et plus générale : au lieu de seuils fixes,
//! il mesure le nombre de cycles RÉELLEMENT écoulés entre changements de
//! résolution successifs dans la ligne, via un historique des écritures
//! `$FF8260` (`TGlue::CheckSideOverscan`/`PreviousShiftModeChange`/
//! `NextShiftModeChange`/`CycleOfLastChangeToShiftMode`, `glue.cpp`) — cette
//! formule est reprise ici (`Shifter::resolution_write_history`), les
//! fenêtres de cycle (`MEDRES_OA`/`_OB`/`_OC`) étant cross-vérifiées contre
//! celles de Hatari (convergence confirmée, voir `ETAT.md`).
//!
//! **Limitations documentées** (approximations volontaires par rapport aux
//! sources, faute de pouvoir vérifier certains détails internes sans accès
//! à une capture matérielle réelle) :
//! - Les constantes `BORDERBYTES_*` sont calibrées pour la basse résolution
//!   (4 plans, cohérent avec `BORDERBYTES_NORMAL`=160=`Resolution::Low.bytes_per_line()`
//!   dans Hatari) et appliquées TELLES QUELLES quelle que soit la
//!   résolution effective (pas de mise à l'échelle par nombre de plans) —
//!   l'usage réel de ces tricks cible presque exclusivement la basse
//!   résolution, donc cette approximation n'a pas d'effet dans les cas
//!   documentés/connus.
//! - Interaction avec le défilement fin STE (`$FF8264`/`$FF8265`) : les
//!   segments de bordure gauche/droite révélés sont rendus INDÉPENDAMMENT
//!   du décalage de défilement fin (pas de `scroll_src_bit` appliqué dans
//!   la bordure) — seul le segment central (largeur nominale) applique le
//!   défilement. Combiner les deux effets simultanément (comme le font de
//!   vraies démos STE "overscan + scroll") n'est donc pas fidèle si les
//!   DEUX sont actifs sur la même ligne.
//! - `RIGHT_MINUS_2` sans `RIGHT_OFF` simultané sur la même ligne n'est pas
//!   modélisé comme un raccourcissement du segment central (seulement comme
//!   une réduction de l'extension `RIGHT_OFF`, plafonnée à 0) — écart
//!   mineur, effet visuel ±2 pixels au pire.
//! - Annulation de `STOP_MIDDLE` via un retour de résolution (`$FF8260`),
//!   pas via `$FF820A` comme le fait réellement Hatari dans ce cas précis
//!   (déviation documentée, choix de conception).
//! - [`Shifter::detect_med_res_tricks`] ne reproduit PAS le garde-fou
//!   `!ShiftModeChangeAtCycle(...)` du code Steem original (fonction non
//!   entièrement comprise, effet supposé marginal — écart documenté, pas
//!   silencieux) ; ne reproduit pas non plus `HblPixelShift` (décalage fin
//!   au niveau du pixel, appliqué via le registre `hscroll` chez Steem pour
//!   deux valeurs de cycle précises) — Steem lui-même qualifie cette
//!   dernière valeur de "peut-être venue de l'auteur de la démo", donc pas
//!   davantage vérifiée que le reste.
//!
//! ## Limitations connues (v1)
//! - Les registres de compteur vidéo (`$FF8205`/`07`/`09`) acceptent
//!   toujours l'écriture logicielle (comportement STE) ; sur ST d'origine,
//!   ils sont normalement lecture seule.
//! - Pas de synchronisation 50/60 Hz distincte de celle déjà portée par le
//!   GLUE.
//! - Convention de polarité en mode haute résolution (bit à 1 = noir, à 0 =
//!   blanc) : c'est la convention la plus communément documentée, mais pas
//!   vérifiée contre une capture d'écran matérielle réelle — à confirmer si
//!   l'image apparaît inversée en pratique.

/// Résolution vidéo programmée (registre `$FF8260`, bits 0-1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resolution {
    /// 320×200, 4 plans (16 couleurs).
    Low,
    /// 640×200, 2 plans (4 couleurs).
    Medium,
    /// 640×400, 1 plan monochrome (écran monochrome uniquement).
    High,
}

impl Resolution {
    fn from_bits(bits: u8) -> Self {
        match bits & 0b11 {
            0b00 => Resolution::Low,
            0b01 => Resolution::Medium,
            _ => Resolution::High,
        }
    }

    fn to_bits(self) -> u8 {
        match self {
            Resolution::Low => 0b00,
            Resolution::Medium => 0b01,
            Resolution::High => 0b10,
        }
    }

    /// Largeur en pixels affichés.
    pub fn width(self) -> usize {
        match self {
            Resolution::Low => 320,
            Resolution::Medium | Resolution::High => 640,
        }
    }

    /// Nombre de plans de bits.
    fn planes(self) -> usize {
        match self {
            Resolution::Low => 4,
            Resolution::Medium => 2,
            Resolution::High => 1,
        }
    }

    /// Nombre d'octets de RAM vidéo consommés par ligne affichée.
    pub fn bytes_per_line(self) -> usize {
        self.width() / 8 * self.planes()
    }
}

/// Adresses des registres sur ST/STE réel.
pub mod addr {
    /// Adresse de base vidéo, octet haut (bits 23-16, 256 octets d'alignement).
    pub const VIDEO_BASE_HIGH: u32 = 0xFF8201;
    /// Adresse de base vidéo, octet médian (bits 15-8).
    pub const VIDEO_BASE_MID: u32 = 0xFF8203;
    /// Adresse de base vidéo, octet bas (bits 7-0) — STE uniquement (permet
    /// un alignement plus fin que les 256 octets du ST d'origine, qui n'a
    /// pas ce registre). Absent du match précédent : une lecture y
    /// retombait sur le `0xFF` par défaut de fin de bus plutôt que la vraie
    /// valeur du registre — trouvé en retraçant un DoubleFault tardif de
    /// Rick_Dangerous.stx jusqu'à du code qui reconstruit l'adresse de
    /// base vidéo à partir de ces trois registres pour calculer une cible
    /// de saut : `0xFF` au lieu de `0x00` y rendait l'adresse finale
    /// impaire et désalignée.
    pub const VIDEO_BASE_LOW: u32 = 0xFF820D;
    /// Compteur vidéo courant, octet haut.
    pub const VIDEO_COUNTER_HIGH: u32 = 0xFF8205;
    /// Compteur vidéo courant, octet médian.
    pub const VIDEO_COUNTER_MID: u32 = 0xFF8207;
    /// Compteur vidéo courant, octet bas.
    pub const VIDEO_COUNTER_LOW: u32 = 0xFF8209;
    /// Premier registre de palette (16 mots consécutifs, un par couleur).
    pub const PALETTE_BASE: u32 = 0xFF8240;
    /// Registre de résolution.
    pub const RESOLUTION: u32 = 0xFF8260;
    /// Défilement fin horizontal STE, SANS préchargement (voir
    /// [`Shifter::write_hscroll`]) — décalage 0-15 pixels dans les 4 bits
    /// bas, mais aucun mot supplémentaire n'est lu : les 16 premiers
    /// pixels de la ligne sortent noirs.
    pub const HSCROLL_NO_PREFETCH: u32 = 0xFF8264;
    /// Défilement fin horizontal STE, AVEC préchargement — même décalage,
    /// mais un groupe de 16 pixels de plus est lu par ligne pour remplir
    /// le bord droit sans perte visible. Registre standard, quasi
    /// systématiquement utilisé par les jeux/démos STE.
    pub const HSCROLL_PREFETCH: u32 = 0xFF8265;
    /// Largeur de ligne STE (octets supplémentaires, en mots moins un,
    /// ajoutés à l'avance d'adresse en fin de ligne — écran virtuel plus
    /// large que l'affichage).
    pub const LINE_WIDTH: u32 = 0xFF820F;
}

/// Bits et constantes de la suppression de bordure horizontale STE, voir la
/// doc de module. Les noms de bits ci-dessous reprennent, pour les
/// mécanismes hérités de Hatari, ceux de `video.c` (`BORDERMASK_*`) pour
/// rester croisable avec son code ; [`OVERSCAN_MED_RES`]/[`FOUR_BIT_SCROLL`]
/// (façon Steem SSE, pas Hatari) n'ont pas cette contrainte de
/// cross-référence (les deux projets n'utilisent d'ailleurs pas la même
/// valeur pour ce dernier).
pub mod border {
    /// Bordure gauche supprimée, variante STE courte (+20 octets) — voir
    /// [`super::Shifter::write_resolution`].
    pub const LEFT_OFF_2_STE: u32 = 0x0200;
    /// Bit intermédiaire : haute résolution armée tôt dans la ligne, en
    /// attente de confirmation par un retour en basse/moyenne résolution
    /// pile au cycle [`HDE_ON_HI`] — ne produit AUCUN effet de rendu par
    /// lui-même (seule la conversion en [`LEFT_OFF_2_STE`] en produit un),
    /// voir [`super::Shifter::write_resolution`].
    pub(super) const LEFT_OFF_PENDING: u32 = 0x0001;
    /// Nudge +2 octets de bordure gauche (ligne commençant plus tôt en
    /// 60Hz) — voir [`super::Shifter::write_sync`].
    pub const LEFT_PLUS_2: u32 = 0x0002;
    /// Ligne raccourcie (-106 octets), switch haute résolution en milieu de
    /// ligne — voir [`super::Shifter::write_resolution`].
    pub const STOP_MIDDLE: u32 = 0x0004;
    /// Bordure droite supprimée (+44 octets) — voir
    /// [`super::Shifter::write_sync`]/[`super::Shifter::write_resolution`]
    /// (deux déclencheurs possibles, voir [`RIGHT_OFF_FULL`]).
    pub const RIGHT_OFF: u32 = 0x0010;
    /// Suppression TOTALE de la bordure droite (+22 octets, EN PLUS de
    /// [`RIGHT_OFF`] déjà comptés), qui force aussi [`LEFT_OFF_2_STE`] sur
    /// la ligne SUIVANTE (cascade inter-lignes) — voir
    /// [`super::Shifter::write_resolution`].
    pub const RIGHT_OFF_FULL: u32 = 0x0020;
    /// Nudge -2 octets de bordure droite (ligne finissant plus tôt en
    /// 60Hz) — voir [`super::Shifter::write_sync`].
    pub const RIGHT_MINUS_2: u32 = 0x0008;
    /// Bordure gauche moyenne résolution affinée, façon Steem SSE
    /// (`TRICK_OVERSCAN_MED_RES`, même valeur que le `BORDERMASK_OVERSCAN_
    /// MED_RES` de Hatari) — voir
    /// [`super::Shifter::detect_med_res_tricks`].
    pub const OVERSCAN_MED_RES: u32 = 0x0040;
    /// Défilement matériel 4 bits façon Steem SSE (`TRICK_4BIT_SCROLL`,
    /// valeur Rust68 arbitraire — Hatari n'a pas d'équivalent au bit près,
    /// voir sa propre `BORDERMASK_LEFT_OFF_MED`, non reprise ici) — voir
    /// [`super::Shifter::detect_med_res_tricks`].
    pub const FOUR_BIT_SCROLL: u32 = 0x0800;

    /// Position en cycles où un switch haute résolution arme
    /// [`LEFT_OFF_PENDING`] (`video.h`, `HDE_On_Hi`, valeur STE).
    pub(super) const HDE_ON_HI: u32 = 4;
    /// Fenêtre haute (inclusive) de détection [`STOP_MIDDLE`] — au-delà,
    /// c'est [`RIGHT_OFF_FULL`] qui se déclenche à la place (`video.h`,
    /// `HDE_Off_Hi`, valeur STE).
    pub(super) const HDE_OFF_HI: u32 = 164;
    /// Fenêtre basse (exclusive) de détection [`RIGHT_OFF`] (`video.h`,
    /// `HDE_Off_Low_60`, valeur STE).
    pub(super) const HDE_OFF_LOW_60: u32 = 372;
    /// Seuil d'annulation de [`LEFT_PLUS_2`] sur un retour au 50Hz
    /// (`video.h`, `HDE_On_Low_60`, valeur STE) — voir
    /// [`super::Shifter::write_sync`].
    pub(super) const HDE_ON_LOW_60: u32 = 52;
    /// Fenêtre haute (inclusive) de détection [`RIGHT_OFF`] (`video.h`,
    /// `HDE_Off_Low_50`, valeur STE).
    pub(super) const HDE_OFF_LOW_50: u32 = 376;
    /// Seuil de cycle sous lequel un switch 60Hz est encore assez tôt dans
    /// la ligne pour être un nudge [`LEFT_PLUS_2`]/[`RIGHT_MINUS_2`]
    /// (`video.h`, `Preload_Start_Low_60`, valeur STE).
    pub(super) const PRELOAD_START_LOW_60: u32 = 36;
    /// Fenêtre basse (exclusive) de détection [`OVERSCAN_MED_RES`] (Steem
    /// SSE, `glue.cpp`, `ScanlineTiming[MEDRES_OA]`) — cross-vérifiée
    /// contre Hatari (`LINE_LEFT_MED_CYCLE_1`, même valeur), voir la doc de
    /// module.
    pub(super) const MEDRES_OA: u32 = 24;
    /// Fenêtre haute (inclusive) de détection [`OVERSCAN_MED_RES`]/
    /// [`FOUR_BIT_SCROLL`] (Steem SSE, `ScanlineTiming[MEDRES_OB]`).
    pub(super) const MEDRES_OB: u32 = 48;
    /// Fenêtre basse (inclusive) de détection [`FOUR_BIT_SCROLL`] (Steem
    /// SSE, `ScanlineTiming[MEDRES_OC]`) — cross-vérifiée contre Hatari
    /// (`LINE_LEFT_STAB_LOW`, même valeur).
    pub(super) const MEDRES_OC: u32 = 16;

    /// Effet en octets de [`LEFT_OFF_2_STE`] (`video.h`,
    /// `BORDERBYTES_LEFT_2_STE`).
    pub const BYTES_LEFT_OFF_2_STE: u32 = 20;
    /// Effet en octets de [`RIGHT_OFF`] (`video.h`, `BORDERBYTES_RIGHT`).
    pub const BYTES_RIGHT_OFF: u32 = 44;
    /// Effet en octets des nudges [`LEFT_PLUS_2`]/[`RIGHT_MINUS_2`]
    /// (`video.h`, dérivé de la différence 4 cycles = 2 octets entre les
    /// seuils 50Hz/60Hz).
    pub const BYTES_NUDGE: u32 = 2;
    /// Effet en octets (réduction) de [`STOP_MIDDLE`] (`video.h`,
    /// `BORDERBYTES_NORMAL`(160) - `BORDERBYTES_MIDDLE`(54) = 106, ou
    /// directement le "-106" documenté dans Hatari).
    pub const BYTES_STOP_MIDDLE: u32 = 106;
    /// Effet en octets de [`RIGHT_OFF_FULL`], EN PLUS de
    /// [`BYTES_RIGHT_OFF`] (`video.h`, `BORDERBYTES_RIGHT_FULL`).
    pub const BYTES_RIGHT_OFF_FULL: u32 = 22;
}

/// État complet d'un Shifter.
#[derive(Debug, Clone)]
pub struct Shifter {
    video_base: u32,
    video_counter: u32,
    resolution: Resolution,
    /// 16 couleurs. Format brut selon [`Self::ste_palette`] : ST (9 bits,
    /// 8-10=R/4-6=G/0-2=B, 3 bits chacun) ou STE (12 bits, 8-11=R/4-7=G/0-3=B,
    /// 4 bits chacun, nibbles contigus).
    palette: [u16; 16],
    /// Palette étendue STE (12 bits/4 par composante) au lieu du format ST
    /// d'origine (9 bits/3 par composante) — voir [`Self::set_ste_palette`].
    ste_palette: bool,
    /// Décalage de défilement fin EFFECTIF courant (0-15) — voir
    /// [`Self::write_hscroll`]. Registre ordinaire (pas une caractéristique
    /// du silicium comme `ste_palette`), remis à zéro par [`Self::reset`].
    h_scroll_count: u8,
    /// `true` si le décalage effectif courant vient de `$FF8265` (avec
    /// préchargement), `false` si de `$FF8264` (sans) — voir
    /// [`Self::write_hscroll`].
    h_scroll_prefetch: bool,
    /// Largeur de ligne EFFECTIVE courante (`$FF820F` brut) — voir
    /// [`Self::write_line_width`].
    line_width: u8,
    /// Décalage/préchargement en attente, appliqué au COMMIT de fin de
    /// [`Self::render_scanline`] (donc effectif à partir de la ligne
    /// suivante) — voir [`Self::write_hscroll`].
    pending_h_scroll: Option<(u8, bool)>,
    /// Largeur de ligne en attente, même mécanisme que
    /// [`Self::pending_h_scroll`] — voir [`Self::write_line_width`].
    pending_line_width: Option<u8>,
    /// Masque de bordure accumulé PENDANT le balayage de la ligne courante
    /// (bits [`border`]) — voir [`Self::write_resolution`]/
    /// [`Self::write_sync`], consommé et remis à zéro (ou réamorcé par
    /// [`Self::pending_left_off_next_line`]) en fin de
    /// [`Self::render_scanline`].
    border_mask: u32,
    /// `true` si un [`border::RIGHT_OFF_FULL`] déclenché sur la ligne
    /// courante doit forcer [`border::LEFT_OFF_2_STE`] au DÉBUT de la ligne
    /// SUIVANTE (cascade inter-lignes, façon Hatari qui écrit directement
    /// dans `ShifterLines[HblCounterVideo+1]`) — voir
    /// [`Self::write_resolution`], consommé en fin de
    /// [`Self::render_scanline`].
    pending_left_off_next_line: bool,
    /// Historique (cycle, valeur brute 0-2) des écritures `$FF8260` de la
    /// ligne courante — nécessaire à [`Self::detect_med_res_tricks`] (façon
    /// Steem SSE, voir la doc de module), qui mesure le nombre de cycles
    /// RÉELLEMENT écoulés entre changements de résolution successifs,
    /// contrairement aux autres mécanismes ci-dessus qui réagissent à un
    /// seuil fixe à chaque écriture. Alimenté par [`Self::write_resolution`],
    /// remis à zéro en fin de [`Self::render_scanline`].
    resolution_write_history: Vec<(u32, u8)>,
    /// Décalage en octets (signé) de la position de lecture de la bordure
    /// gauche, calculé par [`Self::detect_med_res_tricks`] — voir la doc de
    /// module. Distinct de [`Self::border_left_bytes`] : n'affecte QUE la
    /// position de lecture (`start`, façon `SHIFT_SDP` de Steem), pas la
    /// largeur affichée ni l'avance du compteur vidéo.
    med_res_byte_shift: i32,
}

impl Default for Shifter {
    fn default() -> Self {
        Self::new()
    }
}

impl Shifter {
    pub fn new() -> Self {
        Shifter {
            video_base: 0,
            video_counter: 0,
            resolution: Resolution::Low,
            palette: [0; 16],
            ste_palette: false,
            h_scroll_count: 0,
            h_scroll_prefetch: false,
            line_width: 0,
            pending_h_scroll: None,
            pending_line_width: None,
            border_mask: 0,
            pending_left_off_next_line: false,
            resolution_write_history: Vec::new(),
            med_res_byte_shift: 0,
        }
    }

    /// Active/désactive le format de palette étendu STE (12 bits) — à
    /// appeler juste après construction d'après [`crate::systems::atari_st::model::MachineProfile::ste_palette`].
    pub fn set_ste_palette(&mut self, on: bool) {
        self.ste_palette = on;
    }

    /// Écrit `$FF8264` (`prefetch=false`) ou `$FF8265` (`prefetch=true`) —
    /// défilement fin horizontal STE, 0-15 pixels. `apply_now` décide si la
    /// valeur s'applique à la ligne EN COURS de balayage ou est différée à
    /// la suivante (voir [`Self::render_scanline`]) : calculé par
    /// l'appelant (le board, seul à connaître la position en cycles dans
    /// la ligne via `Glue::cycles_in_line`/`line_start_cycle` — reproduit le
    /// seuil de "gating" de Hatari, `video.c`, `Video_HorScroll_Write`).
    pub fn write_hscroll(&mut self, value: u8, prefetch: bool, apply_now: bool) {
        let value = value & 0x0F;
        if apply_now {
            self.h_scroll_count = value;
            self.h_scroll_prefetch = prefetch;
            self.pending_h_scroll = None;
        } else {
            self.pending_h_scroll = Some((value, prefetch));
        }
    }

    /// Écrit `$FF820F` (largeur de ligne STE) — même mécanisme
    /// immédiat/différé que [`Self::write_hscroll`], mais gaté par
    /// l'appelant sur `Glue::line_end_cycle` (fin d'affichage actif), pas
    /// `line_start_cycle` (voir `video.c`, `Video_LineWidth_WriteByte`).
    pub fn write_line_width(&mut self, value: u8, apply_now: bool) {
        if apply_now {
            self.line_width = value;
            self.pending_line_width = None;
        } else {
            self.pending_line_width = Some(value);
        }
    }

    /// Écrit `$FF8260` (résolution) avec connaissance de la position en
    /// cycles dans la ligne — à appeler par le board à la place d'un
    /// [`Self::write`] direct, pour détecter les tricks de bordure
    /// horizontale déclenchés par un passage en haute résolution, façon
    /// Hatari `Video_Update_Glue_State` (voir la doc de module) :
    /// - `cycles_in_line <= 4` : arme [`border::LEFT_OFF_PENDING`] ; un
    ///   retour en basse/moyenne résolution PILE au cycle 4 le convertit en
    ///   [`border::LEFT_OFF_2_STE`] (+20 octets de bordure gauche révélés).
    ///   Tout autre retour en basse/moyenne résolution annule la tentative.
    /// - `4 < cycles_in_line <= 164` : arme [`border::STOP_MIDDLE`] (ligne
    ///   raccourcie de 106 octets) directement, pas de confirmation
    ///   nécessaire. Annulable par un retour en basse/moyenne résolution
    ///   tant que `cycles_in_line <= 164` (avant le "point de non-retour" —
    ///   passé ce cycle, la ligne a déjà été coupée, un retour de
    ///   résolution n'y change plus rien).
    /// - `164 < cycles_in_line <= 376` : arme [`border::RIGHT_OFF`] +
    ///   [`border::RIGHT_OFF_FULL`] (bordure droite TOTALEMENT supprimée,
    ///   +44+22 octets) ET force [`border::LEFT_OFF_2_STE`] au DÉBUT de la
    ///   ligne SUIVANTE (cascade, voir [`Self::pending_left_off_next_line`]) —
    ///   une fois déclenché, ce mécanisme n'est PAS annulable par un retour
    ///   en basse/moyenne résolution plus tard dans la même ligne
    ///   (simplification documentée : Hatari a une fenêtre d'annulation via
    ///   `$FF820A`, pas modélisée ici, voir la doc de module).
    pub fn write_resolution(&mut self, value: u8, cycles_in_line: u32) {
        let new_res = Resolution::from_bits(value);
        // Historique pour `detect_med_res_tricks` (façon Steem SSE, voir la
        // doc de module) — indépendant de la logique de `border_mask`
        // ci-dessous, qui reste fidèle à Hatari.
        self.resolution_write_history.push((cycles_in_line, value & 0b11));
        if new_res == Resolution::High {
            if cycles_in_line <= border::HDE_ON_HI {
                self.border_mask |= border::LEFT_OFF_PENDING;
            } else if cycles_in_line <= border::HDE_OFF_HI {
                self.border_mask |= border::STOP_MIDDLE;
                self.border_mask &= !border::RIGHT_MINUS_2;
            } else if cycles_in_line <= border::HDE_OFF_LOW_50 {
                self.border_mask |= border::RIGHT_OFF | border::RIGHT_OFF_FULL;
                self.pending_left_off_next_line = true;
            }
        } else if cycles_in_line == border::HDE_ON_HI && self.border_mask & border::LEFT_OFF_PENDING != 0 {
            self.border_mask = (self.border_mask & !border::LEFT_OFF_PENDING) | border::LEFT_OFF_2_STE;
        } else {
            self.border_mask &= !border::LEFT_OFF_PENDING;
            if cycles_in_line <= border::HDE_OFF_HI {
                self.border_mask &= !border::STOP_MIDDLE;
            }
        }
        self.resolution = new_res;
    }

    /// Écrit `$FF820A` (registre de synchronisation GLUE) côté Shifter —
    /// distinct de [`crate::peripherals::atari_st::glue::Glue::write_sync`]
    /// (overscan VERTICAL) : ici, les effets HORIZONTAUX
    /// ([`border::RIGHT_OFF`], nudges [`border::LEFT_PLUS_2`]/
    /// [`border::RIGHT_MINUS_2`]), façon Hatari `Video_Update_Glue_State`
    /// (voir la doc de module). Le registre est physiquement UNIQUE : le
    /// board doit appeler LES DEUX méthodes (`Glue::write_sync` et
    /// celle-ci) sur chaque écriture `$FF820A`.
    ///
    /// Un retour au 50Hz N'ANNULE PAS tout d'un bloc : chaque mécanisme a sa
    /// PROPRE fenêtre d'annulation, reprise de Hatari
    /// (`Video_Update_Glue_State`) : [`border::LEFT_PLUS_2`] seulement si
    /// `cycles_in_line <= HDE_On_Low_60`(52) ; [`border::RIGHT_MINUS_2`] et
    /// [`border::RIGHT_OFF`] seulement si `cycles_in_line <=
    /// HDE_Off_Low_50`(376) — pour `RIGHT_OFF`, cette fenêtre est en
    /// pratique très étroite : le bit n'est justement armé QUE dans
    /// `]HDE_Off_Low_60(372), HDE_Off_Low_50(376)]`, donc seule une seconde
    /// écriture `$FF820A` survenant quelques cycles après le trigger, dans
    /// la MÊME fenêtre, peut encore l'annuler — passé cycle 376, l'effet
    /// est acquis pour le reste de la ligne (comportement confirmé contre
    /// `video.c`, pas une approximation).
    pub fn write_sync(&mut self, value: u8, cycles_in_line: u32) {
        let selecting_60hz = value & 0x02 == 0;
        if selecting_60hz {
            if cycles_in_line > border::HDE_OFF_LOW_60 && cycles_in_line <= border::HDE_OFF_LOW_50 {
                self.border_mask |= border::RIGHT_OFF;
            }
            if cycles_in_line <= border::PRELOAD_START_LOW_60 {
                self.border_mask |= border::LEFT_PLUS_2 | border::RIGHT_MINUS_2;
            }
        } else {
            if cycles_in_line <= border::HDE_ON_LOW_60 {
                self.border_mask &= !border::LEFT_PLUS_2;
            }
            if cycles_in_line <= border::HDE_OFF_LOW_50 {
                self.border_mask &= !(border::RIGHT_MINUS_2 | border::RIGHT_OFF);
            }
        }
    }

    /// Masque de bordure accumulé pour la ligne EN COURS de balayage (bits
    /// [`border`]) — diagnostic externe/tests.
    pub fn border_mask(&self) -> u32 {
        self.border_mask
    }

    /// Analyse [`Self::resolution_write_history`] pour détecter
    /// [`border::OVERSCAN_MED_RES`]/[`border::FOUR_BIT_SCROLL`] — façon
    /// Steem SSE (`TGlue::CheckSideOverscan`, `glue.cpp`), PAS Hatari, voir
    /// la doc de module. Appelé une seule fois par ligne, au début de
    /// [`Self::render_scanline`] (Steem analyse l'historique complet de la
    /// ligne a posteriori, pas au fil des écritures comme les autres
    /// mécanismes de ce module).
    ///
    /// Précondition (confirmée dans le code Steem : `!left_border`) :
    /// ces deux tricks affinent une bordure gauche déjà révélée par
    /// [`border::LEFT_OFF_2_STE`], ils ne la déclenchent pas eux-mêmes — ne
    /// fait rien si elle n'est pas déjà active.
    ///
    /// `OVERSCAN_MED_RES` : le dernier passage en moyenne résolution
    /// (valeur `0b01`) dans la fenêtre `]24, 48]`, précédé d'un AUTRE
    /// changement de résolution, décale la position de lecture de
    /// `-(((cycles_in_low_res / 2) % 8) / 2)` octets, `cycles_in_low_res`
    /// étant le nombre de cycles entre ce changement précédent et le
    /// passage en moyenne résolution.
    ///
    /// `FOUR_BIT_SCROLL` : même déclencheur (dernier passage en moyenne
    /// résolution), fenêtre `[16, 48]`, mais regarde EN PLUS le changement
    /// SUIVANT (retour en basse résolution) pour mesurer le temps passé en
    /// moyenne résolution (`cycles_in_med_res`) ; décale la position de
    /// lecture de `8 - cycles_in_med_res/2 + cycles_in_low_res/4` octets.
    /// Les deux effets se cumulent si les deux tricks se déclenchent sur la
    /// même ligne (comme chez Steem, où les deux appellent `SHIFT_SDP` sans
    /// s'exclure).
    fn detect_med_res_tricks(&mut self) {
        if self.border_mask & border::LEFT_OFF_2_STE == 0 {
            return;
        }
        let history = &self.resolution_write_history;

        if let Some(r1) = Self::cycle_of_last_change_to(history, 0b01) {
            if r1 > border::MEDRES_OA && r1 <= border::MEDRES_OB {
                if let Some(r0) = Self::previous_change(history, r1) {
                    let cycles_in_low_res = (r1 - r0) as i32;
                    self.med_res_byte_shift -= ((cycles_in_low_res / 2) % 8) / 2;
                    self.border_mask |= border::OVERSCAN_MED_RES;
                }
            }
        }

        if let Some(r1) = Self::cycle_of_last_change_to(history, 0b01) {
            if r1 >= border::MEDRES_OC && r1 <= border::MEDRES_OB {
                if let Some(r0_next) = Self::next_change(history, r1) {
                    if r0_next <= border::MEDRES_OB {
                        let cycles_in_med_res = (r0_next - r1) as i32;
                        let cycles_in_low_res = Self::previous_change(history, r1)
                            .map(|r0_prev| (r1 - r0_prev) as i32)
                            .unwrap_or(0);
                        self.med_res_byte_shift += 8 - cycles_in_med_res / 2 + cycles_in_low_res / 4;
                        self.border_mask |= border::FOUR_BIT_SCROLL;
                    }
                }
            }
        }
    }

    /// Cycle du DERNIER changement de résolution vers `mode` (`0b00`-`0b10`)
    /// dans l'historique — façon Steem, `CycleOfLastChangeToShiftMode`.
    /// `history` doit être trié par cycle croissant (garanti par
    /// [`Self::write_resolution`], qui ne fait qu'ajouter en fin).
    fn cycle_of_last_change_to(history: &[(u32, u8)], mode: u8) -> Option<u32> {
        history.iter().rev().find(|&&(_, m)| m == mode).map(|&(c, _)| c)
    }

    /// Cycle du changement STRICTEMENT AVANT `cycle`, tout mode confondu —
    /// façon Steem, `PreviousShiftModeChange`.
    fn previous_change(history: &[(u32, u8)], cycle: u32) -> Option<u32> {
        history.iter().rev().map(|&(c, _)| c).find(|&c| c < cycle)
    }

    /// Cycle du changement STRICTEMENT APRÈS `cycle`, tout mode confondu —
    /// façon Steem, `NextShiftModeChange`.
    fn next_change(history: &[(u32, u8)], cycle: u32) -> Option<u32> {
        history.iter().map(|&(c, _)| c).find(|&c| c > cycle)
    }

    /// Octets de bordure gauche à révéler pour la ligne en cours, d'après
    /// [`Self::border_mask`] — voir [`Self::render_scanline`].
    fn border_left_bytes(&self) -> u32 {
        let mut bytes = 0;
        if self.border_mask & border::LEFT_OFF_2_STE != 0 {
            bytes += border::BYTES_LEFT_OFF_2_STE;
        }
        if self.border_mask & border::LEFT_PLUS_2 != 0 {
            bytes += border::BYTES_NUDGE;
        }
        bytes
    }

    /// Octets de bordure droite à révéler pour la ligne en cours, d'après
    /// [`Self::border_mask`] — voir [`Self::render_scanline`].
    fn border_right_bytes(&self) -> u32 {
        let mut bytes = 0;
        if self.border_mask & border::RIGHT_OFF != 0 {
            bytes += border::BYTES_RIGHT_OFF;
        }
        if self.border_mask & border::RIGHT_OFF_FULL != 0 {
            bytes += border::BYTES_RIGHT_OFF_FULL;
        }
        let minus_2 = if self.border_mask & border::RIGHT_MINUS_2 != 0 { border::BYTES_NUDGE } else { 0 };
        bytes.saturating_sub(minus_2)
    }

    /// Octets à RETRANCHER du segment central pour la ligne en cours,
    /// d'après [`Self::border_mask`] — voir [`Self::render_scanline`].
    fn border_center_reduction_bytes(&self) -> u32 {
        if self.border_mask & border::STOP_MIDDLE != 0 { border::BYTES_STOP_MIDDLE } else { 0 }
    }

    /// Décalage de défilement fin effectif courant (0-15) — diagnostic
    /// externe/tests.
    pub fn h_scroll_count(&self) -> u8 {
        self.h_scroll_count
    }

    /// `true` si le décalage effectif courant vient de `$FF8265` (avec
    /// préchargement) — diagnostic externe/tests.
    pub fn h_scroll_prefetch(&self) -> bool {
        self.h_scroll_prefetch
    }

    /// Largeur de ligne effective courante (`$FF820F` brut) — diagnostic
    /// externe/tests.
    pub fn line_width(&self) -> u8 {
        self.line_width
    }

    /// Réinitialise l'état interne (registres/compteur/palette) suite à un
    /// `/RESET` matériel, en préservant [`Self::ste_palette`] : contrairement
    /// aux registres, la présence de la palette étendue STE est une
    /// caractéristique du silicium (quel Shifter est physiquement présent
    /// dans la machine), pas un état remis à zéro par une impulsion RESET —
    /// remplacer l'instance entière par `Shifter::new()` (comme le faisait
    /// une version précédente de `AtariSt::reset_bus`) perdait ce réglage à
    /// chaque exécution de l'instruction RESET (le TOS en exécute une au
    /// démarrage), ce qui repassait silencieusement toute la session en
    /// format de palette ST (9 bits) sur une machine STE.
    pub fn reset(&mut self) {
        let ste_palette = self.ste_palette;
        *self = Self::new();
        self.ste_palette = ste_palette;
    }

    fn palette_mask(&self) -> u16 {
        if self.ste_palette { 0x0FFF } else { 0x0777 }
    }

    pub fn resolution(&self) -> Resolution {
        self.resolution
    }

    /// Les 16 mots de palette bruts (voir [`Self::color_to_rgb`] pour leur
    /// décodage) — utile pour du diagnostic externe (voir `RUST68_DEBUG`
    /// dans `atari_st_sdl2`).
    pub fn palette_raw(&self) -> &[u16; 16] {
        &self.palette
    }

    /// Adresse de base vidéo courante (registre, voir [`addr::VIDEO_BASE_HIGH`]/
    /// [`addr::VIDEO_BASE_MID`]) — diagnostic externe.
    pub fn video_base(&self) -> u32 {
        self.video_base
    }

    /// Compteur vidéo courant — diagnostic externe.
    pub fn video_counter(&self) -> u32 {
        self.video_counter
    }

    /// Lit le registre à l'adresse bus `addr` (voir [`addr`]).
    pub fn read(&self, bus_addr: u32) -> u8 {
        match bus_addr {
            addr::VIDEO_BASE_HIGH => (self.video_base >> 16) as u8,
            addr::VIDEO_BASE_MID => (self.video_base >> 8) as u8,
            addr::VIDEO_BASE_LOW => self.video_base as u8,
            addr::VIDEO_COUNTER_HIGH => (self.video_counter >> 16) as u8,
            addr::VIDEO_COUNTER_MID => (self.video_counter >> 8) as u8,
            addr::VIDEO_COUNTER_LOW => self.video_counter as u8,
            addr::RESOLUTION => self.resolution.to_bits(),
            _ if (addr::PALETTE_BASE..addr::PALETTE_BASE + 32).contains(&bus_addr) => {
                let color = ((bus_addr - addr::PALETTE_BASE) / 2) as usize;
                let word = self.palette[color];
                if (bus_addr - addr::PALETTE_BASE) % 2 == 0 {
                    (word >> 8) as u8
                } else {
                    word as u8
                }
            }
            _ => 0xFF,
        }
    }

    /// Écrit le registre à l'adresse bus `addr` (voir [`addr`]).
    pub fn write(&mut self, bus_addr: u32, value: u8) {
        match bus_addr {
            addr::VIDEO_BASE_HIGH => {
                self.video_base = (self.video_base & 0x00FFFF) | ((value as u32) << 16)
            }
            addr::VIDEO_BASE_MID => {
                self.video_base = (self.video_base & 0xFF00FF) | ((value as u32) << 8)
            }
            addr::VIDEO_BASE_LOW => self.video_base = (self.video_base & 0xFFFF00) | value as u32,
            addr::VIDEO_COUNTER_HIGH => {
                self.video_counter = (self.video_counter & 0x00FFFF) | ((value as u32) << 16)
            }
            addr::VIDEO_COUNTER_MID => {
                self.video_counter = (self.video_counter & 0xFF00FF) | ((value as u32) << 8)
            }
            addr::VIDEO_COUNTER_LOW => {
                self.video_counter = (self.video_counter & 0xFFFF00) | value as u32
            }
            addr::RESOLUTION => self.resolution = Resolution::from_bits(value),
            // Écriture d'un SEUL octet dans un registre de palette : sur le
            // silicium réel (confirmé par Hatari, `Video_ColorReg_WriteWord`,
            // commentaire donnant `move.b #7,$ff8240 -> couleur 0 = $707`),
            // l'octet écrit est dupliqué dans LES DEUX moitiés du mot AVANT
            // masquage — l'autre moitié n'est PAS préservée. Une version
            // précédente ne touchait que la moitié réellement écrite en
            // conservant l'autre : correct pour une écriture .W/.L (voir
            // [`Self::write_palette_word`], utilisé par le board pour ce
            // cas), mais faux pour une instruction .B isolée, qui écrase
            // alors la moitié non écrite avec une valeur non liée (ex. bits
            // rouges corrompus par un reliquat d'une autre couleur) — cohérent
            // avec un curseur/texte qui vire occasionnellement au rouge/jaune
            // au lieu de rester noir.
            _ if (addr::PALETTE_BASE..addr::PALETTE_BASE + 32).contains(&bus_addr) => {
                let color = ((bus_addr - addr::PALETTE_BASE) / 2) as usize;
                let duplicated = ((value as u16) << 8) | value as u16;
                self.palette[color] = duplicated & self.palette_mask();
                if std::env::var("RUST68_TRACE_PALETTE").is_ok() {
                    eprintln!(
                        "[palette] .B addr={bus_addr:#08x} color={color} value={value:#04x} -> {:#06x}",
                        self.palette[color]
                    );
                }
            }
            _ => {}
        }
    }

    /// Écrit un mot complet (16 bits) dans un registre de palette — chemin
    /// utilisé par le board pour les accès `.W`/`.L` du CPU (contrairement à
    /// [`Self::write`], qui ne voit qu'un octet à la fois et suppose donc
    /// systématiquement un accès `.B` isolé ; voir la duplication d'octet
    /// documentée là-bas). `bus_addr` doit être l'adresse paire du registre.
    pub fn write_palette_word(&mut self, bus_addr: u32, value: u16) {
        let color = ((bus_addr - addr::PALETTE_BASE) / 2) as usize;
        self.palette[color] = value & self.palette_mask();
        if std::env::var("RUST68_TRACE_PALETTE").is_ok() {
            eprintln!(
                "[palette] .W addr={bus_addr:#08x} color={color} value={value:#06x} -> {:#06x}",
                self.palette[color]
            );
        }
    }

    /// Recharge le compteur vidéo depuis l'adresse de base — à appeler par
    /// le board à chaque VBL.
    pub fn start_frame(&mut self) {
        self.video_counter = self.video_base;
    }

    /// Lit la prochaine ligne de RAM vidéo à partir du compteur courant,
    /// la convertit en pixels RGB 24 bits selon la résolution programmée,
    /// et avance le compteur du nombre d'octets consommés. À appeler par
    /// le board à chaque HBL.
    ///
    /// `ram` doit couvrir au moins `video_counter` + le nombre d'octets
    /// réellement consommés par cette ligne (`resolution.bytes_per_line()`,
    /// plus un groupe de 16 pixels si le préchargement STE est actif, voir
    /// [`Self::write_hscroll`]) — renvoie une ligne de zéros (noir) si ce
    /// n'est pas le cas, plutôt que de paniquer (fin de RAM installée, ou
    /// base vidéo pointée au-delà par un logiciel de test). Le compteur,
    /// lui, avance TOUJOURS (avant même de savoir si la lecture réussira) :
    /// sur silicium réel, le compteur d'adresse du Shifter est un simple
    /// générateur d'adresses, indépendant de la présence physique de RAM à
    /// cette adresse précise — seul le contenu lu en dépend, pas
    /// l'avancement du compteur. Ne PAS avancer ici bloquait le compteur
    /// indéfiniment (donc toute lecture logicielle de `VIDEO_COUNTER_*`)
    /// dès que la base vidéo dépassait la RAM installée — confirmé
    /// nécessaire par la cartouche de diagnostic usine STe (test "T4 Video
    /// Counter in Memory Controller", qui déplace délibérément la base
    /// vidéo jusqu'à la toute fin de la RAM installée pour vérifier que le
    /// compteur continue de progresser correctement).
    pub fn render_scanline(&mut self, ram: &[u8]) -> Vec<(u8, u8, u8)> {
        // Doit tourner AVANT le calcul de `left_bytes`/`start` ci-dessous :
        // façon Steem, l'historique complet de la ligne est analysé une
        // fois, pas au fil des écritures (voir la doc de
        // `detect_med_res_tricks`).
        self.detect_med_res_tricks();
        let planes = self.resolution.planes();
        let bytes_per_group = planes * 2;
        // Préchargement STE actif ($FF8265, décalage non nul) : un groupe
        // de 16 pixels de PLUS est réellement lu (et consommé sur le
        // compteur d'adresse) par ligne pour remplir le bord droit — voir
        // la doc de [`Self::write_hscroll`] et [`Self::scroll_src_bit`].
        let extra_group = self.h_scroll_count > 0 && self.h_scroll_prefetch;
        // `STOP_MIDDLE` (voir [`border`]) raccourcit le segment central
        // (ligne interrompue par un switch haute résolution en cours de
        // route) — retranché AVANT d'ajouter le préchargement, comme
        // Hatari (`BORDERBYTES_NORMAL - BORDERBYTES_STOP_MIDDLE`).
        let center_bytes = (self.resolution.bytes_per_line())
            .saturating_sub(self.border_center_reduction_bytes() as usize)
            + if extra_group { bytes_per_group } else { 0 };
        // Bordure gauche/droite ($FF8260/$FF820A, voir [`border`] et la doc
        // de module) : octets supplémentaires révélés de part et d'autre du
        // segment central, en plus du défilement fin ci-dessus.
        let left_bytes = self.border_left_bytes() as usize;
        let right_bytes = self.border_right_bytes() as usize;
        let bytes_this_line = left_bytes + center_bytes + right_bytes;
        // La bordure gauche est lue AVANT la position nominale du compteur
        // (les octets qu'elle révèle "précèdent" le début d'affichage
        // habituel) ; le compteur, lui, avance comme si seule l'extension
        // DROITE avait eu lieu — modèle simplifié, non vérifié contre le
        // détail exact de la divergence `pVideoRaster`/`nVideoCounter` de
        // Hatari (voir les limitations documentées en tête de module).
        // `med_res_byte_shift` (façon Steem, `SHIFT_SDP`) affine ENCORE
        // cette position de lecture sans changer la largeur affichée ni
        // l'avance du compteur — voir `detect_med_res_tricks`.
        let start = (self.video_counter as i64 - left_bytes as i64 + self.med_res_byte_shift as i64).max(0) as usize;
        // `LineWidth` ($FF820F) : octets supplémentaires ajoutés à l'avance
        // d'adresse en FIN de ligne (écran virtuel plus large que
        // l'affichage), en plus des octets réellement affichés/préchargés
        // ci-dessus — voir Hatari, `video.c`, `pVideoRaster += LineWidth*2`.
        self.video_counter += (center_bytes + right_bytes + self.line_width as usize * 2) as u32;
        // Largeur (en pixels) du segment CENTRAL à produire : normalement
        // `resolution.width()`, réduite par `STOP_MIDDLE` — indépendante du
        // préchargement de défilement fin (qui ajoute des octets LUS, pas
        // affichés, voir la doc de [`Self::render_planar`]).
        let center_width = self.resolution.bytes_per_line().saturating_sub(self.border_center_reduction_bytes() as usize)
            * 8
            / planes.max(1);

        let pixels = match ram.get(start..start + bytes_this_line) {
            Some(line) => {
                let (left, rest) = line.split_at(left_bytes);
                let (center, right) = rest.split_at(center_bytes);
                match self.resolution {
                    Resolution::High => self.render_mono_full(left, center, right, center_width),
                    _ => self.render_planar_full(left, center, right, planes, center_width),
                }
            }
            None => {
                let width = center_width + (left_bytes + right_bytes) * 8 / planes.max(1);
                vec![(0, 0, 0); width]
            }
        };

        // Commit des valeurs de défilement/largeur en attente (écritures
        // survenues trop tard dans CETTE ligne pour s'y appliquer, voir le
        // "gating" cycle-exact côté board) : effectives à partir de la
        // PROCHAINE ligne — reproduit le mécanisme `New*` de Hatari.
        if let Some((count, prefetch)) = self.pending_h_scroll.take() {
            self.h_scroll_count = count;
            self.h_scroll_prefetch = prefetch;
        }
        if let Some(width) = self.pending_line_width.take() {
            self.line_width = width;
        }
        // Le masque de bordure ne couvre que la ligne qu'il vient de servir
        // à rendre — remis à zéro (ou réamorcé avec `LEFT_OFF_2_STE` si
        // `RIGHT_OFF_FULL` vient de forcer la cascade sur CETTE ligne
        // suivante, voir `write_resolution`), ré-accumulé ensuite par les
        // écritures de la ligne suivante.
        self.border_mask = if self.pending_left_off_next_line { border::LEFT_OFF_2_STE } else { 0 };
        self.pending_left_off_next_line = false;
        self.resolution_write_history.clear();
        self.med_res_byte_shift = 0;

        pixels
    }

    /// Position bit source dans `line` pour le pixel de sortie `x`, en
    /// tenant compte du défilement fin STE effectif — voir la doc de
    /// [`Self::write_hscroll`]. `None` si ce pixel doit sortir en index de
    /// couleur 0 sans lecture (bord gauche noirci en mode sans
    /// préchargement, le registre à décalage matériel n'ayant pas encore
    /// eu le temps de se charger — `$FF8264`, voir Hatari `video.c`,
    /// commentaire sur `HWScrollPrefetch=0`).
    fn scroll_src_bit(&self, x: usize) -> Option<usize> {
        let scroll = self.h_scroll_count as usize;
        if scroll == 0 {
            return Some(x); // passthrough : comportement historique inchangé
        }
        if self.h_scroll_prefetch {
            Some(x + scroll) // tampon étendu d'un groupe, voir render_scanline
        } else if x < 16 {
            None // registre à décalage pas encore chargé : noir
        } else {
            Some((x - 16) + scroll) // tampon normal, aucun octet en plus
        }
    }

    /// Index de couleur (0-15) au bit source `src_bit` de `line`, décodage
    /// planaire word-interleaved standard — factorisé pour être réutilisé à
    /// la fois par [`Self::render_planar`] (segment central, avec
    /// défilement fin via [`Self::scroll_src_bit`]) et
    /// [`Self::render_planar_full`] (segments de bordure gauche/droite,
    /// adressage direct sans défilement).
    fn planar_color_index(line: &[u8], planes: usize, src_bit: usize) -> u8 {
        let group = src_bit / 16;
        let bit_in_group = 15 - (src_bit % 16);
        let mut color_index = 0u8;
        for plane in 0..planes {
            let byte_offset = group * planes * 2 + plane * 2;
            if let (Some(&hi), Some(&lo)) = (line.get(byte_offset), line.get(byte_offset + 1)) {
                let word = u16::from_be_bytes([hi, lo]);
                color_index |= (((word >> bit_in_group) & 1) as u8) << plane;
            }
        }
        color_index
    }

    /// Pixel RGB au bit source `src_bit` de `line` en mode haute résolution
    /// monochrome — factorisé de la même façon que
    /// [`Self::planar_color_index`], voir [`Self::render_mono`]/
    /// [`Self::render_mono_full`].
    fn mono_pixel(line: &[u8], src_bit: usize) -> (u8, u8, u8) {
        let group = src_bit / 16;
        let bit_in_group = 15 - (src_bit % 16);
        let byte_offset = group * 2;
        let set = match (line.get(byte_offset), line.get(byte_offset + 1)) {
            (Some(&hi), Some(&lo)) => (u16::from_be_bytes([hi, lo]) >> bit_in_group) & 1 != 0,
            _ => false,
        };
        // Convention : bit à 1 = noir, à 0 = blanc (cf. limitations).
        if set { (0, 0, 0) } else { (255, 255, 255) }
    }

    /// `width` : nombre de pixels à produire pour ce segment — NORMALEMENT
    /// [`Resolution::width`], mais réduit par [`Self::render_scanline`]
    /// quand [`border::STOP_MIDDLE`] raccourcit la ligne (le défilement fin
    /// ne change PAS `width`, seulement l'adressage source via
    /// [`Self::scroll_src_bit`] — voir la doc de [`Self::render_scanline`]).
    fn render_planar(&self, line: &[u8], width: usize) -> Vec<(u8, u8, u8)> {
        let planes = self.resolution.planes();
        let mut pixels = Vec::with_capacity(width);
        for x in 0..width {
            let Some(src_bit) = self.scroll_src_bit(x) else {
                pixels.push(self.color_to_rgb(self.palette[0]));
                continue;
            };
            let color_index = Self::planar_color_index(line, planes, src_bit);
            pixels.push(self.color_to_rgb(self.palette[color_index as usize]));
        }
        pixels
    }

    /// Voir la doc de [`Self::render_planar`] pour `width`.
    fn render_mono(&self, line: &[u8], width: usize) -> Vec<(u8, u8, u8)> {
        let mut pixels = Vec::with_capacity(width);
        for x in 0..width {
            let Some(src_bit) = self.scroll_src_bit(x) else {
                pixels.push((255, 255, 255)); // "noir de bord" mono = bit 0 = blanc, voir la convention ci-dessous
                continue;
            };
            pixels.push(Self::mono_pixel(line, src_bit));
        }
        pixels
    }

    /// Rend une ligne planaire complète, bordure gauche/droite comprise
    /// (voir [`border`] et la doc de module) : les segments de bordure sont
    /// décodés directement (adressage séquentiel, sans défilement fin), le
    /// segment central via [`Self::render_planar`] (défilement fin
    /// applicable). `left`/`center`/`right` sont des sous-tranches
    /// disjointes de la ligne RAM lue par [`Self::render_scanline`],
    /// `center_width` le nombre de pixels à produire pour le segment
    /// central (voir la doc de [`Self::render_planar`]).
    fn render_planar_full(
        &self,
        left: &[u8],
        center: &[u8],
        right: &[u8],
        planes: usize,
        center_width: usize,
    ) -> Vec<(u8, u8, u8)> {
        let mut pixels = Vec::with_capacity((left.len() + right.len()) * 8 / planes.max(1) + center_width);
        pixels.extend(self.render_planar_border_segment(left, planes));
        pixels.extend(self.render_planar(center, center_width));
        pixels.extend(self.render_planar_border_segment(right, planes));
        pixels
    }

    /// Décode un segment de bordure (gauche ou droite) : adressage direct
    /// séquentiel, sans défilement fin (voir la limitation documentée dans
    /// la doc de module).
    fn render_planar_border_segment(&self, segment: &[u8], planes: usize) -> Vec<(u8, u8, u8)> {
        (0..segment.len() * 8 / planes.max(1))
            .map(|x| {
                let color_index = Self::planar_color_index(segment, planes, x);
                self.color_to_rgb(self.palette[color_index as usize])
            })
            .collect()
    }

    /// Équivalent de [`Self::render_planar_full`] pour la haute résolution
    /// monochrome.
    fn render_mono_full(&self, left: &[u8], center: &[u8], right: &[u8], center_width: usize) -> Vec<(u8, u8, u8)> {
        let mut pixels = Vec::with_capacity(left.len() * 8 + right.len() * 8 + center_width);
        for x in 0..left.len() * 8 {
            pixels.push(Self::mono_pixel(left, x));
        }
        pixels.extend(self.render_mono(center, center_width));
        for x in 0..right.len() * 8 {
            pixels.push(Self::mono_pixel(right, x));
        }
        pixels
    }

    /// Convertit un mot de palette brut en RGB 24 bits, par réplication de
    /// bits (0..2^n-1 -> 0..255 exactement, pas juste une mise à l'échelle
    /// approximative) — 3 bits/composante en ST (`$RGB` sur 9 bits), 4
    /// bits/composante en STE (`$RGB` sur 12 bits, nibbles contigus mais
    /// PAS dans l'ordre naïf, voir [`ste_nibble_to_intensity`] — bug réel
    /// trouvé et corrigé ici, voir sa doc).
    fn color_to_rgb(&self, word: u16) -> (u8, u8, u8) {
        if self.ste_palette {
            let r4 = ((word >> 8) & 0x0F) as u8;
            let g4 = ((word >> 4) & 0x0F) as u8;
            let b4 = (word & 0x0F) as u8;
            (expand4(ste_nibble_to_intensity(r4)), expand4(ste_nibble_to_intensity(g4)), expand4(ste_nibble_to_intensity(b4)))
        } else {
            let r3 = ((word >> 8) & 0x07) as u8;
            let g3 = ((word >> 4) & 0x07) as u8;
            let b3 = (word & 0x07) as u8;
            (expand3(r3), expand3(g3), expand3(b3))
        }
    }
}

/// Réplique une valeur 3 bits (0-7) sur 8 bits (0-255) : `v<<5 | v<<2 | v>>1`
/// donne exactement 0 et 255 aux extrémités et une progression régulière
/// entre les deux, plutôt qu'une simple multiplication approximative.
fn expand3(v: u8) -> u8 {
    (v << 5) | (v << 2) | (v >> 1)
}

/// Réplique une valeur 4 bits (0-15) sur 8 bits (0-255) : `v<<4 | v` donne
/// exactement 0 et 255 aux extrémités et une progression régulière.
fn expand4(v: u8) -> u8 {
    (v << 4) | v
}

/// Réordonne un nibble de palette STE brut (bits 3-0 d'un registre couleur)
/// en la véritable intensité 4 bits qu'il représente — **bug réel corrigé
/// ici** : le bit 3 du nibble n'est PAS le bit de poids fort d'une valeur 4
/// bits normale, c'est un bit de précision fine AJOUTÉ EN BAS par le
/// matériel STE, pour rester compatible avec le format 3 bits du ST
/// d'origine (bits 2-0, qui occupent les MÊMES positions bus que sur ST).
/// La vraie intensité est donc `(bits2-0 << 1) | bit3`, pas le nibble lu
/// tel quel. Confirmé contre Hatari (`conv_st.c`,
/// `ConvST_SetupRGBTable` : `rr = ((r & 0x7) << 1) | ((r & 0x8) >> 3)`).
///
/// Avant ce correctif, un nibble `0b0111` (le maximum "façon ST", ce
/// qu'écrirait naturellement un jeu STE voulant une composante à pleine
/// intensité 3 bits) était lu comme la valeur 4 bits `7` et donnait un RGB
/// à ~47 % (119/255) au lieu de ~93 % (238/255, un cran sous le vrai
/// maximum 15/15) — un assombrissement systématique et sévère de toute
/// palette STE construite ainsi, cohérent avec des couleurs de jeu
/// rapportées "très sombres".
fn ste_nibble_to_intensity(nibble: u8) -> u8 {
    let low3 = nibble & 0x07;
    let fine_bit = (nibble >> 3) & 0x01;
    (low3 << 1) | fine_bit
}
