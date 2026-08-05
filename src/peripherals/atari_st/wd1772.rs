//! Western Digital WD1772 — contrôleur de disquette (FDC) de l'Atari ST.
//!
//! Modélise la puce seule : 4 registres (Commande/Statut, Piste, Secteur,
//! Donnée) multiplexés par les lignes A1-A0, et le jeu de commandes
//! Type I (Restore/Seek/Step/Step-In/Step-Out), Type II (Read/Write
//! Sector) et Type IV (Force Interrupt), documenté publiquement par
//! Western Digital (datasheet WD1770/WD1772, format de commande standard
//! repris dans d'innombrables références techniques indépendantes).
//!
//! Le disque lui-même est abstrait via le trait [`FloppyDisk`] — ce module
//! ne sait pas lire un format de fichier particulier (`.st`, `.stx`…),
//! c'est à l'appelant de fournir une implémentation (voir [`RawDiskImage`]
//! pour le format `.st` brut, secteur linéaire).
//!
//! Le transfert de données Type II (lecture/écriture secteur) passe par
//! [`DmaChannel`], que le board implémente pour donner accès à sa RAM à
//! l'adresse DMA courante — le WD1772 ne connaît pas la RAM, seulement le
//! disque et ce canal.
//!
//! ## Temporisation réelle
//! Contrairement à une version précédente (entièrement synchrone, `BUSY`
//! jamais observable — un logiciel réel qui attend la fin d'une commande
//! en pollant le statut tournait alors bien plus vite que sur silicium
//! réel), [`Self::execute_command`] ne fait plus que DÉMARRER la commande
//! (positionne `BUSY`) ; c'est [`Self::tick`], à appeler par le board à
//! chaque avancée d'horloge (voir `systems::atari_st::AtariSt::tick`), qui
//! fait progresser le délai réel et termine la commande (positionne le
//! statut final, lève `/INTRQ`) une fois écoulé. Constantes vérifiées
//! contre Hatari (`fdc.c`), la référence utilisée dans tout ce projet pour
//! ce type de valeur matérielle — voir le module [`timing`].
//!
//! Simplifications assumées (voir aussi les limitations plus bas) :
//! - Latence de rotation avant qu'un secteur cible arrive sous la tête :
//!   [`Wd1772::cycles_to_target_sector`] suit une position angulaire du
//!   disque (`rotation_phase`) qui avance en continu à vitesse constante
//!   tant que le moteur tourne, et calcule le délai RÉEL jusqu'au secteur
//!   visé en supposant les secteurs également espacés sur la piste (pas un
//!   suivi bit-exact du flux MFM comme le fait Hatari, mais plus qu'une
//!   simple moyenne fixe : une lecture séquentielle secteur par secteur,
//!   le cas le plus courant du pilote disquette de GEMDOS, retrouve un
//!   secteur voisin bien plus vite qu'un secteur pris au hasard sur la
//!   piste, comme sur silicium réel). `timing::AVG_ROTATIONAL_LATENCY_CYCLES`
//!   ne sert plus que de repli si le disque est absent au moment du calcul.
//! - Chargement de tête (15 ms) : conditionné au bit `E` (bit 2) de la
//!   commande Type II/III (voir `COMMAND_BIT_HEAD_LOAD`), comme sur
//!   silicium réel/Hatari (`FDC_COMMAND_BIT_HEAD_LOAD`) — PAS un délai
//!   toujours appliqué comme le supposait une version précédente de ce
//!   module (bug corrigé : vérifié que TOS enchaîne ses lectures secteur
//!   par secteur avec `E=0`, comptant justement sur l'absence de ce délai ;
//!   l'appliquer quand même désalignait la recherche de la position
//!   angulaire réelle du secteur suivant, ~217 ms/secteur mesurés au lieu
//!   des ~22 ms/secteur attendus sur une piste à 9 secteurs).
//! - Secteurs consécutifs d'une lecture/écriture multiple (bit M) : pas de
//!   nouvelle latence de recherche entre deux secteurs successifs
//!   (hypothèse d'un formatage standard, secteurs contigus sur la piste) —
//!   la position angulaire continue malgré tout d'avancer normalement
//!   pendant le transfert, donc une commande suivante repart d'un point de
//!   piste cohérent.
//!
//! ## Limitations connues (v1)
//! - Type III (Read Address/Read Track/Write Track-Format) non implémenté :
//!   commande ignorée, positionne juste `LOST_DATA`/`RECORD_NOT_FOUND`
//!   selon le cas pour signaler l'échec plutôt que de planter (immédiat,
//!   pas de délai réaliste simulé puisque non implémenté de toute façon).
//! - Pas de vérification (bit V) ni de CRC réel : une commande Seek avec
//!   V=1 réussit toujours (pas de lecture d'ID de piste simulée), l'erreur
//!   CRC ne se déclenche jamais.
//! - Le signal `TR00` (capteur physique piste 0) n'est pas modélisé
//!   séparément : le registre Piste fait foi.
//!
//! ## Bruitage mécanique ([`SoundEvent`])
//! Chaque voix (démarrage/moteur/clic/recherche, voir
//! `peripherals::atari_st::drive_sound::DriveSound`) est MONOPHONE : un
//! nouveau déclenchement redémarre la voix depuis le début plutôt que de la
//! superposer à l'instance encore en train de jouer (pas de vraie
//! polyphonie). Limitations qui en découlent :
//! - Un pas isolé (Step/StepIn/StepOut) ne déclenche QUE le clic, jamais le
//!   bourdonnement de recherche — contrairement au Seek/Restore multi-piste
//!   — même en rafale rapprochée (protection anti-copie relisant piste par
//!   piste) : essayé fidèle à Steem SSE (bourdonnement sur tout mouvement,
//!   voir l'historique), mais constaté à l'usage comme sonnant comme un
//!   magma continu plutôt qu'un train de clics distincts avec les
//!   échantillons WAV utilisés ici (voir `queue_step_sound`).
//! - Le clic (`drive_click.wav`) est tronqué à 60 ms au chargement (voir
//!   `atari_st_sdl2.rs::load_drive_sound`) pour la même raison : sa traîne
//!   de résonance naturelle (~200 ms) reste sinon audible pendant une
//!   rafale de pas rapprochés (6-7 ms d'écart à la vitesse de pas la plus
//!   rapide), chaque nouveau clic coupant net une traîne encore forte.
//! - Un seul lecteur (A) a du bruitage — voir `AtariSt::floppy_a` : le
//!   lecteur B n'est pas modélisé du tout (toujours `NOT_READY`).

/// Constantes de temporisation réelles (horloge de référence 8 MHz du
/// WD1772 — celle du CPU ST/STE, ce qui permet d'exprimer directement ces
/// délais en cycles CPU sans conversion), vérifiées contre Hatari
/// (`fdc.c`).
pub mod timing {
    /// Délai minimal avant qu'une commande Type I ne produise un effet
    /// observable (~0.09 ms, mesuré par Hatari sur un 520 STF réel) — évite
    /// de terminer instantanément une commande qui n'a rien à faire (ex.
    /// Seek vers la piste déjà courante).
    pub const TYPE_I_MIN_CYCLES: u32 = 90 * 8;

    /// Vitesse de pas (cycles CPU/piste à 8 MHz) selon les bits r1-r0
    /// (bits 0-1) d'une commande Type I — spécifique au WD1772 (PAS le
    /// WD1770, qui a une table différente pour le même encodage de bits) :
    /// 6, 12, 2, 3 ms.
    pub const STEP_RATE_CYCLES: [u32; 4] = [6 * 8_000, 12 * 8_000, 2 * 8_000, 3 * 8_000];

    /// Délai de chargement de tête avant un Type II (15 ms) — voir les
    /// simplifications assumées dans la doc du module.
    pub const HEAD_LOAD_CYCLES: u32 = 15 * 8_000;

    /// Latence de rotation moyenne (demi-tour à 300 tr/min) — repli utilisé
    /// uniquement quand le disque n'est plus accessible au moment de
    /// calculer la position du secteur cible (voir
    /// [`super::Wd1772::cycles_to_target_sector`], qui suit la position
    /// angulaire réelle du disque dans le cas normal).
    pub const AVG_ROTATIONAL_LATENCY_CYCLES: u32 = 100 * 8_000;

    /// Temps de transfert d'un secteur de 512 octets à 250 kbit/s (4 µs par
    /// bit, 8 bits par octet -> 256 cycles CPU à 8 MHz par octet).
    pub const CYCLES_PER_BYTE: u32 = 256;
    pub const SECTOR_TRANSFER_CYCLES: u32 = CYCLES_PER_BYTE * super::SECTOR_SIZE as u32;
}

/// Registres accessibles via les lignes A1-A0 (multiplexées par le
/// contrôleur DMA sur ST réel, voir `systems::atari_st`).
pub mod reg {
    /// Écriture : registre de commande. Lecture : registre de statut.
    pub const COMMAND_STATUS: u8 = 0;
    pub const TRACK: u8 = 1;
    pub const SECTOR: u8 = 2;
    pub const DATA: u8 = 3;
}

/// Taille standard d'un secteur (format `.st`/GEMDOS).
pub const SECTOR_SIZE: usize = 512;

/// Vue abstraite d'un disque tel qu'accédé par le FDC : adressage par
/// piste/face/secteur, pas par octet — c'est le format natif du WD1772
/// (il ne connaît pas la disposition physique du fichier image).
pub trait FloppyDisk {
    fn num_tracks(&self) -> u8;
    fn num_sides(&self) -> u8;
    fn sectors_per_track(&self) -> u8;
    fn write_protected(&self) -> bool;
    /// `None` si la piste/face/secteur n'existe pas sur ce disque.
    fn read_sector(&self, track: u8, side: u8, sector: u8) -> Option<[u8; SECTOR_SIZE]>;
    /// Ignoré silencieusement si la piste/face/secteur n'existe pas.
    fn write_sector(&mut self, track: u8, side: u8, sector: u8, data: &[u8; SECTOR_SIZE]);

    /// Nombre RÉEL de secteurs sur la piste/face donnée — par défaut
    /// identique à [`Self::sectors_per_track`] (formats uniformes comme
    /// `.st`, un seul nombre pour tout le disque), mais peut différer d'une
    /// piste à l'autre pour un format à métadonnées par piste (`.stx` :
    /// [`Self::sectors_per_track`] n'y renvoie qu'un maximum global, pas
    /// forcément le compte réel de LA piste visée — voir `StxImage`).
    /// Utilisé par [`Wd1772::cycles_to_target_sector`] : un mauvais compte
    /// désaligne progressivement (secteur après secteur) la latence de
    /// recherche calculée par rapport à la position angulaire réelle du
    /// secteur cible.
    fn sectors_on_track(&self, _track: u8, _side: u8) -> u8 {
        self.sectors_per_track()
    }

    /// Vrai si ce secteur porte une erreur CRC délibérée dans son ID
    /// (technique de protection courante : un secteur formaté avec un CRC
    /// sciemment faux, qu'une copie "propre" recalculerait correctement et
    /// perdrait donc — voir `StxImage`, où c'est porté par les métadonnées
    /// `.stx`). Faux par défaut (formats sans cette notion, comme `.st`).
    /// Consulté après une lecture réussie pour positionner le bit
    /// [`status::CRC_ERROR`] — le transfert de données a lieu quand même
    /// (le FDC réel transfère ce qu'il a décodé même quand le CRC ne
    /// correspond pas, il se contente de signaler l'anomalie).
    fn sector_has_crc_error(&self, _track: u8, _side: u8, _sector: u8) -> bool {
        false
    }

    /// Position RÉELLE (en bits, depuis l'impulsion d'index) du champ ID de
    /// ce secteur sur la piste physique d'origine, si connue — portée par
    /// les métadonnées `.stx` (`bit_position` de chaque SDR, voir la doc du
    /// module `stx`), comme utilisé par Hatari
    /// (`FDC_NextSectorID_FdcCycles_STX`, `BitPosition`). `None` par défaut
    /// (formats sans cette info, comme `.st`) : [`Wd1772::cycles_to_target_sector`]
    /// retombe alors sur un espacement uniforme estimé à partir de
    /// [`Self::sectors_on_track`] — une approximation qui peut désaligner la
    /// latence de recherche calculée sur une piste au formatage non standard
    /// (secteurs plus nombreux/rapprochés qu'une piste normale, technique de
    /// protection courante — cf. `Rick_Dangerous.stx`, pistes à 10 secteurs
    /// au lieu des 9 habituels : l'hypothèse d'espacement uniforme donnait
    /// une attente d'environ un tour complet entre deux secteurs
    /// consécutifs au lieu du dixième de tour réel, mesuré ~2x plus lent
    /// qu'Hatari sur un gros transfert séquentiel).
    fn sector_bit_position(&self, _track: u8, _side: u8, _sector: u8) -> Option<u32> {
        None
    }
}

/// Image disque brute au format `.st` : un bloc linéaire de secteurs de
/// 512 octets, dans l'ordre piste par piste puis face par face
/// (`index = (track * sides + side) * sectors_per_track + sector`) — le
/// format d'image `.st` le plus courant. Ne gère pas `.stx` (métadonnées
/// de protection par secteur, hors de portée de ce module).
#[derive(Debug, Clone)]
pub struct RawDiskImage {
    data: Vec<u8>,
    tracks: u8,
    sides: u8,
    sectors_per_track: u8,
    write_protected: bool,
}

impl RawDiskImage {
    /// Construit une image à partir d'un buffer déjà chargé (taille exacte
    /// `tracks * sides * sectors_per_track * 512` attendue).
    pub fn new(data: Vec<u8>, tracks: u8, sides: u8, sectors_per_track: u8) -> Self {
        RawDiskImage {
            data,
            tracks,
            sides,
            sectors_per_track,
            write_protected: false,
        }
    }

    pub fn set_write_protected(&mut self, protected: bool) {
        self.write_protected = protected;
    }

    fn offset(&self, track: u8, side: u8, sector: u8) -> Option<usize> {
        if track >= self.tracks || side >= self.sides || sector == 0 || sector > self.sectors_per_track
        {
            return None;
        }
        let index = (track as usize * self.sides as usize + side as usize)
            * self.sectors_per_track as usize
            + (sector - 1) as usize;
        let offset = index * SECTOR_SIZE;
        if offset + SECTOR_SIZE > self.data.len() {
            None
        } else {
            Some(offset)
        }
    }
}

impl FloppyDisk for RawDiskImage {
    fn num_tracks(&self) -> u8 {
        self.tracks
    }
    fn num_sides(&self) -> u8 {
        self.sides
    }
    fn sectors_per_track(&self) -> u8 {
        self.sectors_per_track
    }
    fn write_protected(&self) -> bool {
        self.write_protected
    }

    fn read_sector(&self, track: u8, side: u8, sector: u8) -> Option<[u8; SECTOR_SIZE]> {
        let offset = self.offset(track, side, sector)?;
        let mut buf = [0u8; SECTOR_SIZE];
        buf.copy_from_slice(&self.data[offset..offset + SECTOR_SIZE]);
        Some(buf)
    }

    fn write_sector(&mut self, track: u8, side: u8, sector: u8, data: &[u8; SECTOR_SIZE]) {
        if let Some(offset) = self.offset(track, side, sector) {
            self.data[offset..offset + SECTOR_SIZE].copy_from_slice(data);
        }
    }
}

/// Canal donnant accès à la RAM du board à l'adresse DMA courante, pour un
/// transfert Type II. Le WD1772 appelle `pull`/`push` une fois par octet
/// de secteur ; c'est au board de faire avancer son propre compteur
/// d'adresse DMA à chaque appel (le WD1772 ne le connaît pas).
pub trait DmaChannel {
    /// Lit l'octet suivant depuis la RAM (pour Write Sector).
    fn pull(&mut self) -> u8;
    /// Écrit l'octet suivant en RAM (pour Read Sector).
    fn push(&mut self, byte: u8);
}

/// Bits du registre de statut, communs aux deux familles de commandes
/// (leur signification bit à bit diffère entre Type I et Type II/III,
/// documenté au niveau de chaque commande).
pub mod status {
    pub const BUSY: u8 = 1 << 0;
    pub const INDEX_OR_DRQ: u8 = 1 << 1;
    pub const TRACK00_OR_LOST_DATA: u8 = 1 << 2;
    pub const CRC_ERROR: u8 = 1 << 3;
    pub const SEEK_ERROR_OR_RECORD_NOT_FOUND: u8 = 1 << 4;
    pub const HEAD_LOADED_OR_RECORD_TYPE: u8 = 1 << 5;
    pub const WRITE_PROTECT: u8 = 1 << 6;
    pub const NOT_READY: u8 = 1 << 7;
}

/// Déclencheur de bruitage mécanique du lecteur — approche reprise du
/// projet compagnon Stay (`stay-fdc`/`stay-sound`, lui-même calqué sur
/// Steem SSE `floppy_drive.cpp`) : le WD1772 empile des évènements dans
/// [`Wd1772::sound_events`] à chaque changement d'activité mécanique
/// pertinent, sans connaître le rendu audio (aucune dépendance vers un
/// module de son ici) — c'est à l'appelant (le board, puis le binaire
/// SDL2) de les consommer via [`Wd1772::take_sound_events`] pour piloter
/// un mélangeur d'échantillons dédié.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SoundEvent {
    MotorOn,
    MotorOff,
    /// Type I sur une seule piste (Step/StepIn/StepOut, ou un Seek/Restore
    /// qui ne déplace que d'une piste).
    StepClick,
    /// Type I sur plusieurs pistes : bourdonnement en boucle jusqu'au
    /// `SeekEnd` correspondant.
    SeekStart,
    SeekEnd,
}

/// Nombre de tours (à 300 tr/min, ~200 ms chacun) que le moteur continue de
/// tourner après la fin de la dernière commande avant de s'arrêter (et
/// d'émettre [`SoundEvent::MotorOff`]) — valeur reprise de Stay/Steem SSE
/// (`MOTOR_OFF_PULSES`/`floppy_drive.cpp`), un vrai lecteur ST ne coupant
/// pas son moteur instantanément après un seul accès.
const MOTOR_OFF_INDEX_PULSES: u32 = 9;
/// Cycles CPU (8 MHz) d'un tour de disque à 300 tr/min — voir
/// [`timing::AVG_ROTATIONAL_LATENCY_CYCLES`] pour le détail du calcul (ici
/// un tour complet, pas une demi-latence moyenne).
const CYCLES_PER_REVOLUTION: u32 = 200 * 8_000;
/// Marge avant d'arrêter le bourdonnement de recherche (`SeekEnd`) après la
/// fin d'une commande Type I qui l'avait déclenché — plutôt que de couper
/// immédiatement à chaque commande individuelle. Reprend le comportement
/// réel de Steem SSE (`floppy_drive.cpp`, `SoundStep`/`SoundCheckIrq`) : le
/// bourdonnement de recherche accompagne TOUT déplacement de tête, y
/// compris un Step/StepIn/StepOut d'une seule piste, et Steem ne l'arrête
/// que si la piste n'a plus changé depuis la dernière vérification — une
/// rafale de commandes Step séparées rapprochées (ex. protection anti-copie
/// relisant piste par piste, ~6-12 ms d'écart selon la vitesse de pas
/// programmée) doit donc sonner comme UN SEUL bourdonnement continu, pas
/// une succession de coupures. 20 ms couvre confortablement l'écart entre
/// deux commandes Step consécutives même à la vitesse de pas la plus lente
/// (12 ms), tout en restant bien plus court que `MOTOR_OFF_INDEX_PULSES`
/// pour s'arrêter promptement une fois l'activité réellement terminée.
const SEEK_GRACE_CYCLES: u32 = 20 * 8_000;

/// Bit `E` (bit 2) d'une commande Type II/III : `0` = ne pas attendre le
/// chargement de tête (déjà chargée), `1` = attendre
/// [`timing::HEAD_LOAD_CYCLES`] — voir `Wd1772::start_type2`. Vérifié
/// contre Hatari (`fdc.h`, `FDC_COMMAND_BIT_HEAD_LOAD`) : la polarité est
/// contre-intuitive (0 = PAS de délai), à ne pas confondre avec le bit `V`
/// (vérification) des commandes Type I qui partage la même position mais
/// une sémantique différente.
const COMMAND_BIT_HEAD_LOAD: u8 = 1 << 2;

/// Étape courante d'une commande en cours d'exécution (voir la doc du
/// module sur la temporisation) — `Idle` si aucune commande n'est en vol.
#[derive(Debug, Clone, Copy)]
enum Phase {
    Idle,
    /// Type I : encore des pas physiques à faire. `step_rate` (vitesse de
    /// pas, réutilisée à chaque pas suivant) est distincte de `cycles_left`
    /// (temps restant avant le PROCHAIN pas).
    Stepping { remaining_steps: u16, cycles_left: u32, step_rate: u32, inward: bool, update_track: bool },
    /// Type II : chargement de tête avant de rechercher le secteur cible.
    HeadLoad { command: u8, cycles_left: u32 },
    /// Type II : latence de rotation avant que `self.sector` arrive sous la
    /// tête.
    Searching { command: u8, cycles_left: u32 },
    /// Type II : transfert du secteur `self.sector` en cours.
    Transferring { command: u8, cycles_left: u32 },
}

/// État complet d'un contrôleur WD1772.
#[derive(Debug, Clone)]
pub struct Wd1772 {
    status: u8,
    track: u8,
    sector: u8,
    data: u8,
    /// Face courante (signal externe, câblée par le board — sur ST réel,
    /// bit du port A du YM2149).
    pub side: u8,
    /// Dernière direction de pas utilisée par Step (sans u/d explicite) :
    /// `true` = vers les pistes hautes (Step-In), `false` = vers 0 (Step-Out).
    last_step_in: bool,
    /// `Some(vecteur)` si une interruption (`/INTRQ`) est en attente.
    intrq: bool,
    phase: Phase,
    /// Bruitage mécanique — voir [`SoundEvent`]/[`Self::take_sound_events`].
    sound_events: Vec<SoundEvent>,
    /// Vrai tant que le moteur est considéré comme lancé (indépendant de
    /// `BUSY`/`phase` : le moteur continue de tourner un moment après la
    /// fin d'une commande, voir `motor_spin_down`).
    motor_on: bool,
    /// Cycles restants avant l'arrêt du moteur (voir `MOTOR_OFF_INDEX_PULSES`)
    /// une fois la dernière commande terminée — `None` tant qu'aucun arrêt
    /// n'est programmé (moteur déjà éteint, ou commande encore en cours).
    motor_spin_down: Option<u32>,
    /// Vrai tant qu'un bourdonnement de recherche (`SeekStart`) est en
    /// cours, pour savoir si la fin de commande Type I doit émettre le
    /// `SeekEnd` correspondant.
    seek_sound_active: bool,
    /// Cycles restants avant l'arrêt du bourdonnement de recherche (voir
    /// `SEEK_GRACE_CYCLES`) — `None` tant qu'aucun arrêt n'est programmé
    /// (bourdonnement déjà arrêté, ou commande encore en cours).
    seek_spin_down: Option<u32>,
    /// Position angulaire courante du disque (cycles CPU écoulés depuis le
    /// dernier passage par le début de la piste, `0..CYCLES_PER_REVOLUTION`)
    /// — voir [`Self::cycles_to_target_sector`]. Ne progresse que moteur
    /// lancé (`motor_on`) ; réinitialisée à chaque redémarrage moteur (front
    /// montant, voir `set_motor_on`) à partir de `total_cycles` plutôt qu'à
    /// une valeur fixe : sur silicium réel, la phase de rotation au
    /// redémarrage est en pratique imprévisible, et une remise à zéro fixe
    /// alignerait systématiquement le secteur 1 juste après le point de
    /// redémarrage — pénalisant à chaque fois (quasi un tour complet
    /// d'attente) le cas très courant "Restore puis lecture du secteur de
    /// boot" plutôt que de varier comme sur silicium réel.
    rotation_phase: u32,
    /// Cycles CPU totaux vus par ce contrôleur depuis sa création (horloge
    /// murale monotone, jamais remise à zéro — y compris moteur éteint) :
    /// sert uniquement de graine déterministe-mais-variable pour
    /// `rotation_phase` au redémarrage moteur, en évitant d'introduire une
    /// dépendance vers un vrai générateur aléatoire pour ce détail mineur.
    total_cycles: u64,
}

impl Default for Wd1772 {
    fn default() -> Self {
        Self::new()
    }
}

impl Wd1772 {
    pub fn new() -> Self {
        Wd1772 {
            status: 0,
            track: 0,
            sector: 0,
            data: 0,
            side: 0,
            last_step_in: true,
            intrq: false,
            phase: Phase::Idle,
            sound_events: Vec::new(),
            motor_on: false,
            motor_spin_down: None,
            seek_sound_active: false,
            seek_spin_down: None,
            rotation_phase: 0,
            total_cycles: 0,
        }
    }

    /// Vide et renvoie la file d'évènements de bruitage mécanique accumulés
    /// depuis le dernier appel — à appeler par le board à chaque trame
    /// audio pour piloter un mélangeur d'échantillons dédié (voir
    /// [`SoundEvent`]).
    pub fn take_sound_events(&mut self) -> Vec<SoundEvent> {
        std::mem::take(&mut self.sound_events)
    }

    /// Programme le prochain déclenchement du moteur (`SoundEvent::MotorOn`
    /// sur le front montant seulement — plusieurs commandes consécutives
    /// pendant que le moteur tourne déjà ne doivent pas rejouer
    /// l'échantillon de démarrage à chaque fois) et annule un arrêt
    /// programmé (`motor_spin_down`) puisque le moteur est réutilisé.
    fn set_motor_on(&mut self) {
        if !self.motor_on {
            self.sound_events.push(SoundEvent::MotorOn);
            self.rotation_phase = (self.total_cycles % CYCLES_PER_REVOLUTION as u64) as u32;
        }
        self.motor_on = true;
        self.motor_spin_down = None;
    }

    /// Programme l'arrêt du moteur après [`MOTOR_OFF_INDEX_PULSES`] tours —
    /// appelé à la fin de CHAQUE commande (succès ou échec), pas seulement
    /// au dernier accès d'une session : une commande suivante avant
    /// l'échéance annule simplement ce délai via `set_motor_on`.
    fn schedule_motor_off(&mut self) {
        if self.motor_on {
            self.motor_spin_down = Some(MOTOR_OFF_INDEX_PULSES * CYCLES_PER_REVOLUTION);
        }
    }

    /// Émet le bruitage adapté à un déplacement Type I de `distance` pistes
    /// (0 = déjà sur la cible, rien à jouer ; 1 = un clic sec, accompagné du
    /// bourdonnement de recherche comme sur Steem SSE (voir
    /// `SEEK_GRACE_CYCLES`) ; plus = juste le bourdonnement, jusqu'à la fin
    /// du déplacement).
    fn queue_step_sound(&mut self, distance: u16) {
        match distance {
            0 => {}
            // Un pas isolé ne déclenche QUE le clic, pas le bourdonnement de
            // recherche — contrairement à Steem SSE, dont `SoundStep` fait
            // les deux à chaque commande Step-family avec mouvement réel
            // (voir l'historique de ce fichier). Constat empirique après
            // essai en conditions réelles : avec les échantillons WAV
            // utilisés ici, superposer le bourdonnement à CHAQUE pas isolé
            // fait sonner une rafale de commandes Step séparées (protection
            // anti-copie relisant piste par piste) comme un magma continu
            // plutôt qu'un train de clics distincts et réguliers — préféré
            // par l'utilisateur à l'essai. Voir `ensure_seek_bed_active`,
            // toujours utilisée pour le cas `_` (vrai Seek multi-piste).
            1 => self.sound_events.push(SoundEvent::StepClick),
            _ => self.ensure_seek_bed_active(),
        }
    }

    /// Démarre le bourdonnement de recherche s'il ne tourne pas déjà (pas de
    /// nouveau `SeekStart` — donc pas de redémarrage audible de la boucle —
    /// si une commande précédente l'avait déjà déclenché) et annule tout
    /// arrêt programmé (`seek_spin_down`) : une rafale de commandes Step
    /// rapprochées doit prolonger le même bourdonnement, pas le redémarrer.
    fn ensure_seek_bed_active(&mut self) {
        if !self.seek_sound_active {
            self.sound_events.push(SoundEvent::SeekStart);
            self.seek_sound_active = true;
        }
        self.seek_spin_down = None;
    }

    fn end_seek_sound_if_active(&mut self) {
        if self.seek_sound_active {
            self.sound_events.push(SoundEvent::SeekEnd);
            self.seek_sound_active = false;
        }
        self.seek_spin_down = None;
    }

    /// Programme l'arrêt du bourdonnement de recherche après
    /// [`SEEK_GRACE_CYCLES`] plutôt que de l'arrêter immédiatement à la fin
    /// de CHAQUE commande Type I — voir la doc de cette constante.
    fn schedule_seek_off(&mut self) {
        if self.seek_sound_active {
            self.seek_spin_down = Some(SEEK_GRACE_CYCLES);
        }
    }

    /// Lit le registre logique `r` (voir [`reg`]). Lire le statut acquitte
    /// `/INTRQ` (comportement réel du WD1772).
    pub fn read(&mut self, r: u8) -> u8 {
        match r {
            reg::COMMAND_STATUS => {
                self.intrq = false;
                self.status
            }
            reg::TRACK => self.track,
            reg::SECTOR => self.sector,
            reg::DATA => self.data,
            _ => 0xFF,
        }
    }

    /// Écrit le registre logique `r`. Écrire dans `COMMAND_STATUS` exécute
    /// la commande — mais seulement les registres simples (Piste/Secteur/
    /// Donnée) sont gérés ici ; le board doit appeler
    /// [`Self::execute_command`] séparément pour ce cas (il a besoin d'un
    /// accès au disque que cette méthode n'a pas).
    pub fn write_simple_register(&mut self, r: u8, value: u8) {
        match r {
            reg::TRACK => self.track = value,
            reg::SECTOR => self.sector = value,
            reg::DATA => self.data = value,
            _ => {}
        }
    }

    /// Vrai si `/INTRQ` est actuellement actif (à relayer par le board vers
    /// son mécanisme d'interruption — câblage spécifique au système hôte,
    /// pas modélisé ici).
    pub fn interrupt_requested(&self) -> bool {
        self.intrq
    }

    /// Vrai tant qu'une commande est en cours (voir la doc du module sur la
    /// temporisation) — observable par un logiciel qui poll le statut,
    /// contrairement à une version précédente entièrement synchrone.
    pub fn busy(&self) -> bool {
        self.status & status::BUSY != 0
    }

    /// Démarre la commande `command` (valeur écrite dans `COMMAND_STATUS`).
    /// Positionne `BUSY` et programme la temporisation réelle (voir la doc
    /// du module) ; c'est [`Self::tick`] qui termine la commande une fois
    /// le délai écoulé. `disk` est `None` pour simuler un lecteur sans
    /// disque (`NOT_READY`, immédiat).
    pub fn execute_command<D: FloppyDisk + ?Sized>(&mut self, command: u8, disk: Option<&mut D>) {
        self.intrq = false;
        if command & 0xF0 == 0xD0 {
            // Type IV : Force Interrupt — interrompt immédiatement toute
            // commande en cours et génère /INTRQ si une des conditions
            // I0-I3 est sélectionnée. Toujours immédiat sur silicium réel
            // (c'est justement le mécanisme pour ne pas attendre qu'une
            // commande en cours se termine).
            self.phase = Phase::Idle;
            self.status &= !status::BUSY;
            if command & 0x0F != 0 {
                self.intrq = true;
            }
            // Un bourdonnement de recherche en cours ne doit pas rester
            // bloqué en boucle si Force Interrupt coupe la commande avant
            // sa fin normale.
            self.end_seek_sound_if_active();
            self.schedule_motor_off();
            return;
        }

        let Some(disk) = disk else {
            self.phase = Phase::Idle;
            self.status = status::NOT_READY;
            self.intrq = true;
            return;
        };

        self.status = status::BUSY;

        if command & 0x80 == 0 {
            self.start_type1(command);
        } else if command & 0xC0 == 0x80 {
            self.start_type2(command, disk);
        } else {
            // Type III (Read Address/Read Track/Write Track) : non
            // implémenté (cf. limitations) — signale l'échec immédiatement
            // plutôt que de simuler un délai réaliste pour une commande
            // qu'on n'exécute pas vraiment.
            self.phase = Phase::Idle;
            self.status = status::SEEK_ERROR_OR_RECORD_NOT_FOUND | self.write_protect_bit(disk);
            self.intrq = true;
        }
    }

    /// Fait avancer la temporisation d'une commande en cours de `cycles`
    /// cycles CPU — à appeler par le board à chaque avancée d'horloge (voir
    /// la doc du module). Fait aussi avancer l'arrêt programmé du moteur
    /// (voir `motor_spin_down`) même sans commande en vol (`Phase::Idle`) :
    /// le moteur continue de tourner un moment après la dernière commande,
    /// indépendamment de `BUSY`.
    pub fn tick<D: FloppyDisk + ?Sized>(
        &mut self,
        mut cycles: u32,
        mut disk: Option<&mut D>,
        dma: &mut impl DmaChannel,
    ) {
        let elapsed = cycles;
        self.total_cycles += elapsed as u64;
        if let Some(remaining) = self.motor_spin_down.as_mut() {
            if cycles >= *remaining {
                self.motor_spin_down = None;
                self.motor_on = false;
                self.sound_events.push(SoundEvent::MotorOff);
            } else {
                *remaining -= cycles;
            }
        }
        if let Some(remaining) = self.seek_spin_down.as_mut() {
            if cycles >= *remaining {
                self.seek_spin_down = None;
                self.end_seek_sound_if_active();
            } else {
                *remaining -= cycles;
            }
        }
        // Position angulaire du disque : avance une seule fois par appel,
        // du nombre RÉEL de cycles écoulés (indépendamment de ce que fait
        // la machine d'état `Phase` ci-dessous) — un disque réel continue
        // de tourner à vitesse constante, que le FDC soit en train de
        // chercher un secteur ou pas — voir `cycles_to_target_sector`.
        if self.motor_on {
            self.rotation_phase =
                ((self.rotation_phase as u64 + elapsed as u64) % CYCLES_PER_REVOLUTION as u64) as u32;
        }
        // Boucle : une seule avancée de `cycles` peut faire franchir
        // plusieurs étapes (ex. plusieurs pas d'un Seek longue distance en
        // un seul `tick` si le CPU exécute une instruction longue) — chaque
        // itération consomme les cycles jusqu'à épuisement ou jusqu'à
        // revenir à `Idle`.
        while cycles > 0 {
            match &mut self.phase {
                Phase::Idle => return,
                Phase::Stepping { remaining_steps, cycles_left, step_rate, inward, update_track } => {
                    if cycles < *cycles_left {
                        *cycles_left -= cycles;
                        return;
                    }
                    cycles -= *cycles_left;
                    let (mut remaining_steps, step_rate, inward, update_track) =
                        (*remaining_steps, *step_rate, *inward, *update_track);
                    if remaining_steps > 0 {
                        // Un pas physique vient de s'achever.
                        if update_track {
                            self.track =
                                if inward { self.track.saturating_add(1) } else { self.track.saturating_sub(1) };
                        }
                        remaining_steps -= 1;
                    }
                    if remaining_steps == 0 {
                        self.phase = Phase::Idle;
                        self.finish_type1(disk.as_deref());
                    } else {
                        // Rearme pour le pas suivant, à la même vitesse.
                        self.phase =
                            Phase::Stepping { remaining_steps, cycles_left: step_rate, step_rate, inward, update_track };
                    }
                }
                Phase::HeadLoad { command, cycles_left } => {
                    if cycles < *cycles_left {
                        *cycles_left -= cycles;
                        return;
                    }
                    cycles -= *cycles_left;
                    let command = *command;
                    let wait = disk
                        .as_deref()
                        .map(|d| self.cycles_to_target_sector(d))
                        .unwrap_or(timing::AVG_ROTATIONAL_LATENCY_CYCLES);
                    self.phase = Phase::Searching { command, cycles_left: wait };
                }
                Phase::Searching { command, cycles_left } => {
                    if cycles < *cycles_left {
                        *cycles_left -= cycles;
                        return;
                    }
                    cycles -= *cycles_left;
                    let command = *command;
                    if let Some(d) = disk.as_deref() {
                        if d.read_sector(self.track, self.side, self.sector).is_none() && command & 0x20 == 0 {
                            // Lecture seulement : un secteur inexistant se
                            // détecte dès la recherche (pas d'ID trouvé),
                            // pas besoin d'attendre le temps de transfert.
                            self.phase = Phase::Idle;
                            self.status = status::SEEK_ERROR_OR_RECORD_NOT_FOUND;
                            self.intrq = true;
                            self.schedule_motor_off();
                            continue;
                        }
                    }
                    self.phase = Phase::Transferring { command, cycles_left: timing::SECTOR_TRANSFER_CYCLES };
                }
                Phase::Transferring { command, cycles_left } => {
                    if cycles < *cycles_left {
                        *cycles_left -= cycles;
                        return;
                    }
                    cycles -= *cycles_left;
                    let command = *command;
                    self.finish_sector_transfer(command, disk.as_deref_mut(), dma);
                }
            }
        }
    }

    fn write_protect_bit<D: FloppyDisk + ?Sized>(&self, disk: &D) -> u8 {
        if disk.write_protected() {
            status::WRITE_PROTECT
        } else {
            0
        }
    }

    /// Cycles CPU jusqu'à ce que le secteur `self.sector` arrive sous la
    /// tête, en supposant `sectors_per_track` secteurs également espacés
    /// sur la piste (formatage standard, même hypothèse que le "pas de
    /// nouvelle recherche entre secteurs consécutifs d'un transfert
    /// multiple" déjà documenté) — se base sur la position angulaire réelle
    /// du disque (`rotation_phase`) plutôt qu'une moyenne fixe, comme le
    /// fait Hatari (`FDC_NextSectorID_FdcCycles_*`) : un secteur qui vient
    /// justement de passer sous la tête, ou proche du dernier accès dans
    /// l'ordre physique de la piste, arrive donc bien plus vite qu'un
    /// secteur pris au hasard — exactement ce qui manquait à l'ancienne
    /// moyenne fixe pour des lectures séquentielles secteur par secteur
    /// (le cas le plus courant du pilote disquette de GEMDOS), qu'elle
    /// pénalisait uniformément au pire cas.
    fn cycles_to_target_sector<D: FloppyDisk + ?Sized>(&self, disk: &D) -> u32 {
        // Position réelle capturée sur le média d'origine (`.stx` :
        // `bit_position` par secteur, voir la doc de
        // `FloppyDisk::sector_bit_position`) — prioritaire sur l'estimation
        // par espacement uniforme ci-dessous, qui suppose à tort que
        // n'importe quelle piste divise le tour complet en portions égales
        // (faux pour un formatage non standard, ex. "secteur en plus" —
        // mesuré ~2x plus lent qu'Hatari sur `Rick_Dangerous.stx` avant ce
        // correctif : chaque secteur consécutif d'une piste à 10 secteurs
        // attendait ~1 tour complet au lieu du 1/10e de tour réel).
        if let Some(bit_pos) = disk.sector_bit_position(self.track, self.side, self.sector) {
            let target_position = bit_pos * (timing::CYCLES_PER_BYTE / 8);
            return (target_position + CYCLES_PER_REVOLUTION
                - self.rotation_phase % CYCLES_PER_REVOLUTION)
                % CYCLES_PER_REVOLUTION;
        }
        let sectors_per_track = disk.sectors_on_track(self.track, self.side);
        if sectors_per_track == 0 {
            return timing::AVG_ROTATIONAL_LATENCY_CYCLES;
        }
        let sector_period = CYCLES_PER_REVOLUTION / sectors_per_track as u32;
        // Secteurs numérotés à partir de 1 sur silicium réel.
        let target_position =
            (self.sector.saturating_sub(1) as u32 % sectors_per_track as u32) * sector_period;
        (target_position + CYCLES_PER_REVOLUTION - self.rotation_phase % CYCLES_PER_REVOLUTION)
            % CYCLES_PER_REVOLUTION
    }

    fn start_type1(&mut self, command: u8) {
        let step_rate = timing::STEP_RATE_CYCLES[(command & 0x03) as usize];
        let (remaining_steps, inward, update_track): (u16, bool, bool) = match command >> 4 {
            0b0000 => {
                // Restore : ramène la tête en piste 0, un pas à la fois
                // depuis la position actuelle (au moins 1 pas, comme sur
                // silicium réel qui vérifie toujours au moins une fois).
                (self.track.max(1) as u16, false, true)
            }
            0b0001 => {
                // Seek : la cible est dans le registre Donnée.
                let target = self.data;
                let inward = target > self.track;
                self.last_step_in = inward;
                let steps = (target as i16 - self.track as i16).unsigned_abs();
                (steps, inward, true)
            }
            0b0010 | 0b0011 => (1, self.last_step_in, command & 0x10 != 0),
            0b0100 | 0b0101 => {
                self.last_step_in = true;
                (1, true, command & 0x10 != 0)
            }
            0b0110 | 0b0111 => {
                self.last_step_in = false;
                (1, false, command & 0x10 != 0)
            }
            _ => (0, false, false),
        };
        self.set_motor_on();
        self.queue_step_sound(remaining_steps);
        let cycles_left = if remaining_steps == 0 { timing::TYPE_I_MIN_CYCLES } else { step_rate };
        self.phase = Phase::Stepping { remaining_steps, cycles_left, step_rate, inward, update_track };
    }

    fn finish_type1<D: FloppyDisk + ?Sized>(&mut self, disk: Option<&D>) {
        self.status = 0;
        if self.track == 0 {
            self.status |= status::TRACK00_OR_LOST_DATA;
        }
        if let Some(disk) = disk {
            self.status |= self.write_protect_bit(disk);
        }
        self.intrq = true;
        self.schedule_seek_off();
        self.schedule_motor_off();
    }

    fn start_type2<D: FloppyDisk + ?Sized>(&mut self, command: u8, disk: &mut D) {
        let is_write = command & 0x20 != 0;
        if is_write && disk.write_protected() {
            // Détecté immédiatement (ligne matérielle WPRT), pas besoin
            // d'attendre le chargement de tête pour le signaler.
            self.phase = Phase::Idle;
            self.status = status::WRITE_PROTECT;
            self.intrq = true;
            self.schedule_motor_off();
            return;
        }
        self.set_motor_on();
        if command & COMMAND_BIT_HEAD_LOAD != 0 {
            self.phase = Phase::HeadLoad { command, cycles_left: timing::HEAD_LOAD_CYCLES };
        } else {
            // Bit E=0 : le logiciel demande explicitement de SAUTER le
            // délai de chargement de tête (silicium réel : la tête est
            // déjà chargée, typiquement plusieurs lectures consécutives
            // sur la même piste sans Seek entre les deux) — directement en
            // recherche. Vérifié empiriquement essentiel : sans ce bit, le
            // pilote disquette GEMDOS (qui enchaîne des Read Sector
            // simples avec E=0, PAS le bit multiple) subit à chaque
            // secteur un délai de tête reconduit à tort, qui désaligne la
            // recherche de la position angulaire réelle du secteur suivant
            // et fait attendre un tour quasi complet à chaque fois — measuré
            // ~217 ms/secteur au lieu de ~22 ms/secteur attendus sur une
            // piste à 9 secteurs.
            let wait = self.cycles_to_target_sector(disk);
            self.phase = Phase::Searching { command, cycles_left: wait };
        }
    }

    /// Termine le transfert du secteur courant (une fois son temps de
    /// transfert écoulé) : effectue la lecture/écriture réelle, puis
    /// enchaîne sur le secteur suivant (bit M) sans nouvelle recherche
    /// (secteurs contigus, voir la doc du module) ou termine la commande.
    fn finish_sector_transfer<D: FloppyDisk + ?Sized>(
        &mut self,
        command: u8,
        disk: Option<&mut D>,
        dma: &mut impl DmaChannel,
    ) {
        let is_write = command & 0x20 != 0;
        let multiple = command & 0x10 != 0;
        let Some(disk) = disk else {
            self.phase = Phase::Idle;
            self.status = status::NOT_READY;
            self.intrq = true;
            self.schedule_motor_off();
            return;
        };

        let mut crc_error = false;
        if is_write {
            let mut buf = [0u8; SECTOR_SIZE];
            for b in buf.iter_mut() {
                *b = dma.pull();
            }
            disk.write_sector(self.track, self.side, self.sector, &buf);
        } else {
            match disk.read_sector(self.track, self.side, self.sector) {
                Some(buf) => {
                    for &b in buf.iter() {
                        dma.push(b);
                    }
                    // Transféré quand même (le FDC réel transfère ce qu'il a
                    // décodé même quand le CRC ne correspond pas), mais
                    // signalé dans le status final ci-dessous — et, comme le
                    // WD1772 réel, ça interrompt une lecture multi-secteurs
                    // (bit M) au lieu d'enchaîner sur le suivant.
                    crc_error = disk.sector_has_crc_error(self.track, self.side, self.sector);
                }
                None => {
                    self.phase = Phase::Idle;
                    self.status = status::SEEK_ERROR_OR_RECORD_NOT_FOUND;
                    self.intrq = true;
                    self.schedule_motor_off();
                    return;
                }
            }
        }

        if multiple && !crc_error && self.sector < disk.sectors_on_track(self.track, self.side) {
            self.sector += 1;
            self.phase = Phase::Transferring { command, cycles_left: timing::SECTOR_TRANSFER_CYCLES };
        } else {
            self.phase = Phase::Idle;
            self.status = self.write_protect_bit(disk);
            if crc_error {
                self.status |= status::CRC_ERROR;
            }
            self.intrq = true;
            self.schedule_motor_off();
        }
    }
}
