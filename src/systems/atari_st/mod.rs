//! Board Atari ST : mapping mémoire réel + câblage MFP/GLUE → IPL.
//!
//! Implémente [`crate::Bus`] pour un ST/STE minimal : RAM installée à
//! `0x000000`, ROM TOS à `0xFC0000`, MFP 68901 aux adresses impaires
//! `0xFFFA01`-`0xFFFA2F`. Sur silicium réel, le MMU répond toujours
//! /DTACK dans tout l'espace d'adressage "RAM ST" (4 Mo, voir
//! [`ST_RAM_ADDRESS_SPACE`]) — même au-delà de la RAM physiquement
//! installée, l'accès ne déclenche **jamais** de bus error dans cette
//! plage (contrairement au vrai "trou" entre 4 Mo et le début de la zone
//! d'E/S, `0xFF8000`, qui lui déclenche un bus error via
//! [`crate::Bus::take_bus_fault`] — mécanisme que de nombreux
//! programmes/démos utilisent pour détecter la RAM installée une fois le
//! TOS démarré). Au-delà de la RAM réellement installée mais dans les
//! 4 Mo, un accès "flotte" : modélisé ici par une valeur fixe non
//! stockée (jamais ce qui vient d'être écrit), plutôt que la valeur
//! réelle (capacité résiduelle du bus selon le dernier cycle, non
//! reproductible dans un émulateur déterministe) — c'est justement cette
//! absence de persistance, pas une histoire d'adressage replié, que le
//! TOS observe pour sa propre détection de RAM au tout début du boot
//! froid (écrire un motif, le relire, conclure "pas de RAM ici" si ça ne
//! correspond pas).
//!
//! Câblage d'interruption réel ST/STE, par priorité décroissante :
//! MFP → IPL6, VBL (GLUE) → IPL4, HBL (GLUE) → IPL2. Les deux ACIA
//! (clavier + MIDI) ne génèrent pas d'IPL directement : leurs sorties IRQ
//! sont OR câblées sur `GPIP4` du MFP (câblage réel ST/STE). Le WD1772
//! (`/INTRQ`) est câblé sur `GPIP5` (câblage réel ST/STE).
//!
//! Le Shifter (vidéo) est piloté par le rythme HBL/VBL du GLUE : `tick`
//! détecte les changements de `Glue::current_line`/`frame_count` et
//! déclenche `Shifter::render_scanline`/`start_frame` en conséquence,
//! accumulant l'image dans [`AtariSt::framebuffer`].
//!
//! ## Limitations connues (v1)
//! - RAM/ROM/MFP/ACIA×2/YM2149/Shifter sont réellement mappés. Le reste de
//!   la zone d'E/S (`0xFF8000`-`0xFFFFFF` : FDC/DMA…) et le port cartouche
//!   (`0xFA0000`-`0xFBFFFF`) répondent `0xFF` en lecture et ignorent les
//!   écritures — chip select réel mais périphérique pas encore émulé (ou,
//!   pour la cartouche, absente), plutôt qu'un bus error qui casserait
//!   tout polling de statut par le logiciel ou la sonde de cartouche que
//!   fait la ROM TOS au boot (`cmpi.l #$fa52235f,$fa0000`).
//! - `rom_base` vaut `DEFAULT_ROM_BASE` (`0xFC0000`, TOS <= 1.04) par
//!   défaut ; [`AtariSt::set_rom_base`] permet de le changer à `0xE00000`
//!   pour les TOS >= 1.06 (256 Ko, ne tiendrait de toute façon pas entre
//!   `0xFC0000` et `IO_BASE`). Pas de mirroring simultané aux deux adresses.
//! - Pas de modèle de contention DRAM/vidéo (`is_contended` reste à
//!   `false`) : le Shifter est maintenant implémenté mais son accès
//!   mémoire n'est pas (encore) modélisé comme volant des cycles bus au
//!   CPU.
//! - Les adresses paires adjacentes à un registre MFP (ex: `0xFFFA00`,
//!   normalement flottantes sur un vrai bus 8 bits) retombent dans le
//!   stub d'E/S générique plutôt que de modéliser précisément le
//!   comportement de décodage UDS/LDS.
//! - `AtariSt::tick` doit être appelé explicitement par l'appelant après
//!   chaque `Cpu::step` (ce crate ne fait pas progresser les
//!   périphériques tout seul) — voir l'exemple sur `tick`.
//! - Registres DMA/WD1772 simplifiés : sélecteur de registre FDC
//!   (`DMA_MODE`, bits 0-1) modélisé, mais pas le registre de nombre de
//!   secteurs ni la sélection FDC/HDC réels du contrôleur DMA ST — notre
//!   modèle de transfert "instantané par secteur" n'en a pas besoin
//!   fonctionnellement (voir `peripherals::atari_st::wd1772` pour le détail).

pub mod model;

use crate::peripherals::atari_st::acia::{self, Acia};
use crate::peripherals::atari_st::blitter::{self, Blitter};
use crate::peripherals::atari_st::dma_sound::{self, DmaSound};
use crate::peripherals::atari_st::microwire::Microwire;
use crate::peripherals::atari_st::glue::{Glue, VideoMode};
use crate::peripherals::atari_st::ikbd::Ikbd;
use crate::peripherals::atari_st::mfp::Mfp;
use crate::peripherals::atari_st::shifter::{self, Shifter};
use crate::peripherals::atari_st::wd1772::{self, DmaChannel, FloppyDisk, SECTOR_SIZE, Wd1772};
use crate::peripherals::atari_st::ym2149::{self, Ym2149};
use crate::{ADDR_MASK, Bus};

/// Adresse du premier registre MFP (`GPIP`), sur ST/STE réel.
pub const MFP_BASE: u32 = 0xFFFA01;
/// Nombre de registres logiques du MFP (voir `peripherals::atari_st::mfp::reg`).
const MFP_REG_COUNT: u32 = 24;
/// Adresse du dernier registre MFP (`UDR`).
pub const MFP_END: u32 = MFP_BASE + (MFP_REG_COUNT - 1) * 2;

/// ACIA clavier : registre de contrôle/statut, sur ST/STE réel.
pub const ACIA_KEYBOARD_CONTROL: u32 = 0xFFFC00;
/// ACIA clavier : registre de données.
pub const ACIA_KEYBOARD_DATA: u32 = 0xFFFC02;
/// ACIA MIDI : registre de contrôle/statut.
pub const ACIA_MIDI_CONTROL: u32 = 0xFFFC04;
/// ACIA MIDI : registre de données.
pub const ACIA_MIDI_DATA: u32 = 0xFFFC06;

/// YM2149 : registre sélecteur (écriture = choix du registre, lecture =
/// registre actuellement sélectionné), sur ST/STE réel.
pub const YM2149_SELECT: u32 = 0xFF8800;
/// YM2149 : registre de données du registre actuellement sélectionné.
pub const YM2149_DATA: u32 = 0xFF8802;

/// WD1772 : registre multiplexé (Commande/Statut/Piste/Secteur/Donnée
/// selon le sélecteur `DMA_MODE`), sur ST/STE réel.
pub const FDC_DATA: u32 = 0xFF8604;
/// Contrôleur DMA : sélecteur de registre FDC — registre 16 bits sur
/// silicium réel (bits 0, 9-15 inutilisés ; bits 1-2 = sélecteur de
/// registre FDC A1-A0, voir [`AtariSt::write16`]), confirmé par Hatari
/// (`fdc.c` : `FDC_reg = (FDC_DMA.Mode & 0x6) >> 1`). PAS bits 0-1 comme un
/// registre 8 bits classique — un accès mot (l'usage réel de TOS) place le
/// sélecteur dans l'octet BAS du mot, ce que seule une prise en charge
/// explicite de l'accès 16 bits complet peut voir (un `write8` naïf décomposé
/// octet par octet, comme le reste du bus, ne verrait que l'octet haut,
/// toujours nul pour ces petites valeurs — le sélecteur resterait alors
/// bloqué à 0 en permanence quoi que TOS écrive).
pub const DMA_MODE: u32 = 0xFF8606;
/// Compteur d'adresse DMA, octet haut.
pub const DMA_ADDR_HIGH: u32 = 0xFF8609;
/// Compteur d'adresse DMA, octet médian.
pub const DMA_ADDR_MID: u32 = 0xFF860B;
/// Compteur d'adresse DMA, octet bas.
pub const DMA_ADDR_LOW: u32 = 0xFF860D;

/// Base des registres du Blitter, sur STE réel.
pub const BLITTER_BASE: u32 = 0xFF8A00;

/// Début de la zone d'E/S général (ACIA, PSG, FDC, Shifter…) sur ST/STE.
pub const IO_BASE: u32 = 0xFF8000;
/// Fin de l'espace d'adressage (24 bits).
pub const IO_END: u32 = 0x00FF_FFFF;

/// Port cartouche (ROM externe, ex: cartouches jeu). Contrairement au
/// "trou" physique au-dessus de la RAM installée (qui déclenche un bus
/// error, voir [`crate::Bus::take_bus_fault`]), une cartouche absente
/// répond en lecture flottante (`0xFF`) SANS bus error — le boot ROM TOS
/// sonde justement cette zone (`cmpi.l #$fa52235f,$fa0000` : signature de
/// cartouche) pour détecter une cartouche sans jamais planter s'il n'y en
/// a pas.
pub const CARTRIDGE_BASE: u32 = 0xFA0000;
pub const CARTRIDGE_END: u32 = 0xFBFFFF;

/// Lignes visibles par trame (200, en basse/moyenne résolution, quel que
/// soit PAL/NTSC — voir [`AtariSt::tick`] pour l'usage vis-à-vis du Timer B
/// du MFP).
const VISIBLE_LINES: usize = 200;

/// DMA sound + Microwire (STE), non implémenté : simple stub qui répond
/// `0x00` en lecture (pas `0xFF` comme le reste de la zone d'E/S non
/// émulée) et ignore les écritures. Nécessaire car le TOS >= 1.62 (STE)
/// écrit puis relit le registre Microwire (`$FF8922`) en boucle en
/// attendant qu'il retombe à zéro (fin de décalage série) au tout début du
/// boot — avec la réponse générique `0xFF` (bits toujours à 1), cette
/// attente ne se termine jamais. Pas une émulation réelle du DMA
/// sound/LMC1992 : juste de quoi ne pas bloquer indéfiniment un TOS STE
/// qui sonde cette zone.
pub const STE_DMA_SOUND_BASE: u32 = 0xFF8900;
pub const STE_DMA_SOUND_END: u32 = 0xFF893F;
/// Registre Microwire DATA (interface série vers le mixeur LMC1992) — voir
/// le commentaire sur son cas particulier en lecture dans [`AtariSt::read8`].
const STE_MICROWIRE_DATA: u32 = 0xFF8922;
const STE_MICROWIRE_DATA1: u32 = 0xFF8923;
/// Registre Microwire MASK (masque de décalage série) — voir
/// [`crate::peripherals::atari_st::microwire::Microwire`].
const STE_MICROWIRE_MASK: u32 = 0xFF8924;
const STE_MICROWIRE_MASK1: u32 = 0xFF8925;

/// Adresse de base usuelle de la ROM TOS (192 Ko, TOS 1.x/2.x).
pub const DEFAULT_ROM_BASE: u32 = 0xFC0000;

/// Registre de configuration mémoire (MMU), sur ST/STE réel. Écrire ici
/// désactive l'overlay ROM à l'adresse 0 (voir [`AtariSt::overlay`]) — le
/// TOS le fait très tôt au boot, juste après avoir sondé le cookie de
/// warmstart, pour reprendre normalement le contrôle des adresses basses
/// une fois son propre code d'amorçage terminé.
///
/// Bits 3-2 = taille logique annoncée de la banque 0, bits 1-0 = banque 1
/// (`00`=128 Ko, `01`=512 Ko, `10`=2 Mo, `11`=réservé) — confirmé par le
/// code source de Hatari (`stMemory.c`, `STMemory_MMU_ConfToBank`). Le TOS
/// l'écrit lui-même pendant la détection de RAM au boot froid ; pilote
/// réellement le mirroring d'adresse intra-banque (voir
/// [`AtariSt::translate_ram_addr`]) sur STE, pas seulement le timing de
/// rafraîchissement DRAM.
pub const MEMORY_CONF: u32 = 0xFF8001;

/// Taille de l'espace d'adressage "RAM ST" sur ST/STE réel (4 Mo, deux
/// banques MMU de 2 Mo chacune), par opposition au vrai "trou" au-delà,
/// avant `IO_BASE` — voir la doc de [`AtariSt::in_floating_st_ram`] pour le
/// détail de ce qui déclenche (ou pas) un bus error dans cette plage selon
/// le type d'accès (CPU direct vs DMA).
const ST_RAM_ADDRESS_SPACE: u32 = 4 * 1024 * 1024;

/// Cycles CPU accordés au CPU entre deux tranches de blit non-HOG (16 mots,
/// voir `Blitter::execute`), c'est-à-dire le temps que le CPU a pour
/// s'exécuter en parallèle avant que le Blitter ne reprenne la main —
/// PAS le temps que le Blitter lui-même met à traiter sa tranche (aucun
/// rapport direct avec `WORDS_PER_SLICE`, qui approxime plutôt les 64
/// accès bus que le silicium réel accorde au Blitter à chaque tour).
/// Utilisé pour cadencer la reprise autonome du Blitter dans
/// [`AtariSt::tick`] au même rythme que le vrai matériel, plutôt qu'une
/// tranche entière par instruction CPU (bien trop rapide).
///
/// Valeur reprise de Hatari (`src/blitter.c`, `Blitter_Start`, mode non
/// "cycle exact" — celui qui correspond à notre propre modèle, sans
/// comptage d'accès bus individuel) : le commentaire y est explicite —
/// "In non cycle exact mode, the blitter will have 64 bus accesses and the
/// cpu will run during 64*4 = 256 cpu cycles" — implémenté via
/// `CycInt_AddRelativeInterrupt(BLITTER_NONHOG_BUS_CPU*4, ...)` avec
/// `BLITTER_NONHOG_BUS_CPU = 64`, donc bien 256, pas 64 (bug de calibration
/// corrigé ici : une valeur de 64 laissait le Blitter reprendre la main
/// 4× plus souvent que sur le matériel de référence).
const BLITTER_SLICE_CYCLES: u32 = 256;

/// Broche GPIP du MFP câblée sur le signal "MONO DETECT" du connecteur
/// moniteur, sur ST/STE réel : un moniteur monochrome met ce signal à la
/// masse (broche lue à 0), tandis qu'un moniteur couleur (ou l'absence de
/// moniteur) laisse une résistance de tirage le maintenir à l'état haut
/// (broche lue à 1). Le TOS lit cette broche très tôt au boot pour choisir
/// entre le mode haute résolution monochrome (640×400 N&B) et les modes
/// couleur (320×200/640×200) — sans ce câblage, la broche resterait à son
/// état par défaut (0), et le TOS conclurait à tort à un moniteur
/// monochrome. Ce board modélise un moniteur couleur : la broche est donc
/// maintenue à 1 en permanence, y compris après un `/RESET` logiciel
/// (`Bus::reset_bus`) puisque le signal reflète un branchement physique
/// externe, pas un état interne du MFP que `/RESET` réinitialiserait.
const GPIP_MONO_DETECT: u8 = 7;

/// Board Atari ST minimal : RAM + ROM + MFP 68901 + GLUE (HBL/VBL).
pub struct AtariSt {
    ram: Vec<u8>,
    rom: Vec<u8>,
    rom_base: u32,
    /// Image ROM de cartouche (port `$FA0000`), le cas échéant — vide par
    /// défaut (emplacement libre, lectures renvoyant `0xFF` sans bus error,
    /// voir [`Self::read8`]). Voir [`Self::load_cartridge`].
    cartridge: Vec<u8>,
    /// `RUST68_TRACE_IRQ=1` lu une seule fois à la construction (pas à
    /// chaque appel — voir l'historique du fichier pour la régression de
    /// performance que ça a provoquée en étant vérifié à chaque IACK).
    trace_irq: bool,
    /// Cycles CPU accumulés depuis la dernière tranche de blit non-HOG
    /// traitée (voir [`Self::tick`]) — au-delà de [`BLITTER_SLICE_CYCLES`],
    /// une nouvelle tranche est autorisée. Sans ce throttling, un blit
    /// repris à CHAQUE tick() (donc à chaque instruction CPU) se termine en
    /// une poignée d'instructions au lieu du temps réel que prend le
    /// silicium (256 cycles CPU laissés au CPU entre deux tranches en mode
    /// partagé — voir la doc de [`BLITTER_SLICE_CYCLES`] pour la source).
    blitter_slice_cycle_acc: u32,
    /// Registres DMA son STE (`$FF8900`-`$FF893F`) — pas de véritable
    /// émulation audio DMA (le son STE reste silencieux), mais un simple
    /// stockage octet par octet fidèle en lecture/écriture. Sans cela (un
    /// stub renvoyant toujours `0x00` en lecture, écritures ignorées), tout
    /// logiciel testant la présence du DMA son par écriture-puis-relecture
    /// (technique standard de détection matérielle, utilisée notamment par
    /// les cartouches de diagnostic) boucle indéfiniment en attendant une
    /// valeur qui ne revient jamais.
    ste_dma_sound: [u8; (STE_DMA_SOUND_END - STE_DMA_SOUND_BASE + 1) as usize],
    /// Contrôleur DMA Sound (STE) : lecture d'échantillons PCM 8 bits en
    /// RAM. Champ public pour la génération audio par l'appelant (voir
    /// [`DmaSound::next_sample`]) — câblé aux registres `$FF8901`-`$FF8921`
    /// (offsets [`dma_sound::reg`]) dans `read8`/`write8`, le reste de la
    /// plage `STE_DMA_SOUND_BASE..=STE_DMA_SOUND_END` (dont le Microwire)
    /// restant un stockage générique séparé (voir `ste_dma_sound` juste
    /// au-dessus).
    pub dma_sound: DmaSound,
    /// Circuit Microwire/LMC1992 (STE) : volume maître et gauche/droite en
    /// aval du mélange PSG+DMA — voir [`Microwire::gain`], à appliquer par
    /// l'appelant sur l'échantillon de sortie final (pas un registre de
    /// `dma_sound`, câblé séparément dans `read8`/`write8` sur les mêmes
    /// adresses `$FF8922`/`$FF8924`).
    pub microwire: Microwire,
    /// Puce MFP 68901, câblée sur IPL6 (voir `Bus::irq_level`). Champ
    /// public : l'appelant a besoin d'y injecter des événements externes
    /// (`set_gpip_input`, `push_rx_byte`…). Faire progresser ses timers
    /// passe par [`Self::tick`], pas directement par `Mfp::tick`.
    pub mfp: Mfp,
    /// Puce GLUE (timing HBL/VBL), câblée sur IPL2/IPL4. Champ public :
    /// utile en lecture pour synchroniser un rendu vidéo externe sur
    /// `current_line()`/`frame_count()`.
    pub glue: Glue,
    /// ACIA clavier. Champ public : injecter les octets reçus du
    /// contrôleur clavier via `push_rx_byte`, lire les commandes envoyées
    /// par le programme via `take_tx_byte`.
    pub acia_keyboard: Acia,
    /// Contrôleur IKBD (HD6301) : traduit les événements clavier/souris
    /// venant de l'hôte en octets protocole IKBD et gère les commandes
    /// envoyées par le programme (reset, mode souris…). Câblé sur
    /// `acia_keyboard` par [`Self::tick`] — voir [`ikbd::Ikbd`]. Champ
    /// public : injecter les événements hôte via `key_make`/`key_break`/
    /// `mouse_move`.
    pub ikbd: Ikbd,
    /// ACIA MIDI (in/out).
    pub acia_midi: Acia,
    /// PSG YM2149 (son + ports d'E/S). Champ public : lire les niveaux de
    /// sortie audio via `channel_level`, injecter les entrées des ports
    /// A/B (joystick/souris/lecteur, câblage non interprété par ce board).
    pub ym2149: Ym2149,
    /// Shifter (vidéo). Champ public surtout pour la lecture directe des
    /// registres si besoin ; en pratique l'image se lit via
    /// [`Self::framebuffer`], déjà rendue.
    pub shifter: Shifter,
    /// Image de la trame en cours de construction : une ligne par entrée
    /// (indexée comme `Glue::current_line`), mise à jour au rythme HBL par
    /// [`Self::tick`]. Contient l'image de la trame précédente jusqu'à ce
    /// que la ligne correspondante de la trame courante soit rendue.
    pub framebuffer: Vec<Vec<(u8, u8, u8)>>,
    /// Compteur monotone (jamais remis à zéro, contrairement à
    /// `Glue::current_line` qui boucle) : nécessaire pour détecter le
    /// passage d'une trame complète (313 lignes en PAL) sans le confondre
    /// avec "aucune ligne écoulée" quand `current_line` revient à 0.
    last_absolute_line: u64,
    /// Dernier `Glue::vbl_edge_count()` observé — détecte le front VBL
    /// (transition ligne visible -> blanking, PAS le bouclage de trame) pour
    /// déclencher `Shifter::start_frame`. Voir la doc de
    /// `Glue::vbl_edge_count` pour pourquoi VBL, spécifiquement, et pas
    /// `frame_count`.
    last_vbl_edge: u64,
    /// PC de l'instruction CPU en cours d'exécution, mis à jour par
    /// l'appelant juste avant `Cpu::step` — uniquement pour le diagnostic
    /// (`RUST68_TRACE_BLITTER`, identifier la routine ROM qui arme un blit).
    pub last_pc: u32,
    /// WD1772 (contrôleur de disquette). Champ public : câbler `/INTRQ`
    /// n'est pas nécessaire à la main, `Self::tick` s'en charge (relayé
    /// sur `GPIP5` du MFP).
    pub wd1772: Wd1772,
    /// Disque inséré dans le lecteur A, s'il y en a un. Champ public :
    /// insérer/éjecter directement (`st.floppy_a = Some(Box::new(RawDiskImage::new(...)))`).
    /// Type objet (`dyn FloppyDisk`) plutôt qu'un format concret : accepte
    /// aussi bien `RawDiskImage` (`.st`) que `peripherals::atari_st::stx::StxImage`
    /// (`.stx`) sans que ce board ait à connaître le format du fichier.
    pub floppy_a: Option<Box<dyn FloppyDisk>>,
    dma_register_select: u8,
    dma_address: u32,
    /// Bit 4 de `DMA_MODE` : bascule `FDC_DATA` entre accès aux registres du
    /// WD1772 (0) et écriture/lecture du compteur de secteurs DMA (1) — un
    /// mécanisme séparé, PAS un registre du contrôleur de disquette lui-même
    /// (confirmé par Hatari, `fdc.c` : "Set DMA sector count if ff8606 bit
    /// 4 == 1"). Voir `dma_sector_count`.
    dma_sector_count_mode: bool,
    /// Nombre de secteurs restant à transférer, programmé via `FDC_DATA` en
    /// mode compteur de secteurs (voir `dma_sector_count_mode`) — `None`
    /// tant qu'aucun compteur n'a jamais été programmé (simplification :
    /// sur silicium réel il vaut 0 après reset, ce qui bloquerait tout
    /// transfert tant que le logiciel ne l'a pas armé ; en pratique TOS le
    /// fait toujours avant la moindre commande Type II, donc traiter
    /// "jamais programmé" comme "illimité" est sans conséquence pratique et
    /// évite de faire échouer un transfert délibérément déclenché "à la
    /// main", sans ce préambule, par un test/outil). Limite RÉELLEMENT le
    /// nombre de secteurs transférés côté DMA, indépendamment de ce que le
    /// WD1772 lui-même ferait naturellement (qui continuerait de trouver
    /// les secteurs suivants sur la piste) — sans cette limite, une lecture
    /// multi-secteurs (bit M) déborde sur la RAM bien au-delà de ce que le
    /// logiciel attend dès que la piste physique a plus de secteurs que ce
    /// qui a été demandé (le cas des pistes protégées non standard).
    dma_sector_count: Option<u16>,
    /// Blitter (STE). Champ public surtout pour la lecture directe des
    /// registres ; le déclenchement se fait en écrivant le bit BUSY/START
    /// du registre de contrôle (voir `Bus::write8` sur `BLITTER_BASE +
    /// blitter::reg::CONTROL`).
    pub blitter: Blitter,
    /// Overlay ROM à l'adresse 0 (câblage matériel réel ST/STE) : tant que
    /// vrai, les LECTURES dans `0x000000..OVERLAY_SIZE` renvoient le
    /// contenu de la ROM (pas de la RAM sous-jacente), tandis que les
    /// ÉCRITURES continuent d'aller en RAM normalement — exactement le
    /// comportement réel (la ROM est en lecture seule de toute façon).
    /// Actif par défaut à la création (et après un `/RESET`), désactivé
    /// par la première écriture dans [`MEMORY_CONF`] (le TOS le fait très
    /// tôt au boot). Sans cet overlay : (1) le vecteur de reset (SSP/PC lus
    /// à `0x000000`/`0x000004`) ne serait pas satisfait par de la RAM
    /// neuve (zéros), et (2) la technique standard de détection de RAM du
    /// TOS — zéroter le vecteur de bus error à `0x000008` puis sonder
    /// au-delà de la RAM installée, ce qui fait rebondir l'exécution sur
    /// l'adresse 0 à chaque échec — ne retomberait pas sur du code ROM
    /// valide (le `bra.s` d'en-tête TOS) mais sur de la RAM à zéro,
    /// dégénérant en exécution de code arbitraire. L'overlay ne couvre
    /// volontairement qu'une petite fenêtre ([`OVERLAY_SIZE`], pas toute la
    /// ROM) : au-delà, des adresses basses comme les variables système
    /// `memvalid`/`phystop` (`$420`, `$42E`…) doivent rester de la vraie
    /// RAM, sans quoi leur vérification par le TOS n'aurait aucun sens
    /// (une variable système censée persister à travers un redémarrage à
    /// chaud ne peut pas être en lecture seule dans la ROM).
    overlay: bool,
    /// Copie de la dernière valeur écrite dans [`MEMORY_CONF`] (registre
    /// MMU). Pilote désormais réellement le mirroring d'adresse intra-banque
    /// sur STE (voir [`Self::translate_ram_addr`]) — pas seulement une
    /// relecture logicielle passive.
    memory_conf: u8,
    /// Taille réelle peuplée de chaque banque RAM (voir
    /// [`Self::ram_bank_sizes`]) — `None` si `self.ram.len()` ne correspond
    /// à aucune configuration de banque STE standard (voir sa doc), auquel
    /// cas [`Self::translate_ram_addr`] ne traduit rien (mappage direct
    /// inchangé, comme avant l'introduction du mirroring MMU).
    ram_bank_sizes: Option<(u32, u32)>,
    /// Force la valeur logicielle de [`MEMORY_CONF`] à rester fixée à cette
    /// valeur, quoi que le logiciel y écrive — voir
    /// [`Self::pin_memory_conf`].
    memory_conf_pin: Option<u8>,
    /// Vrai si le Blitter est physiquement présent sur cette machine. De
    /// série sur STE/Mega STE ; absent sur 520ST/1040ST (le Mega ST avait
    /// juste un support de puce, pas toujours peuplé — voir
    /// [`crate::systems::atari_st::model`]). Quand faux, la zone
    /// `BLITTER_BASE` retombe dans le stub d'E/S générique (`0xFF` en
    /// lecture, écritures ignorées) au lieu de répondre — un vrai
    /// programme qui sonde la présence du Blitter avant de s'en servir
    /// doit voir la même chose que sur un ST sans Blitter.
    blitter_present: bool,
    bus_fault: Option<(u32, bool)>,
}

/// Taille de la fenêtre d'overlay ROM à l'adresse 0 (voir [`AtariSt::overlay`]).
/// Couvre largement l'en-tête TOS et le tout début du code d'amorçage
/// (`os_entry`/`os_version`/`os_reseth`/`os_beg`/… puis les premières
/// instructions réelles), sans empiéter sur les variables système basses
/// (`memvalid` etc. commencent à `$420`).
const OVERLAY_SIZE: u32 = 0x200;

/// Taille de la zone en permanence mappée sur la ROM à l'adresse 0 (les
/// vecteurs reset SSP/PC), indépendamment de l'état de [`AtariSt::overlay`].
///
/// Documentée noir sur blanc dans le manuel technique Atari (Mega Service
/// Manual, carte mémoire RAM) : "Note: the first 8 bytes of ROM are mapped
/// into addresses 0-7. These are reset vectors which the 68000 uses on
/// start-up." — une caractéristique permanente du plan mémoire, distincte du
/// plus large overlay désactivable par `MEMORY_CONF` (qui, lui, couvre
/// `0x200` octets et sert uniquement à amorcer le tout début du boot). Écrire
/// dans cette zone doit déclencher un vrai bus error (Glue : "asserts Bus
/// Error if... writing to ROM"), pas être silencieusement ignoré comme les
/// autres écritures en ROM — confirmé par la cartouche de diagnostic usine
/// STe (test "I7 Bus error not detected", qui écrit délibérément à l'adresse
/// 0 après avoir installé son propre gestionnaire de bus error, pour
/// vérifier que le matériel réagit). N'entre pas en conflit avec la technique
/// standard de détection RAM du TOS (qui cible l'adresse 8, hors de cette
/// zone), ni avec aucune variable système basse (`memvalid` etc. commencent
/// à `$420`).
const RESET_VECTOR_ROM_SIZE: u32 = 8;

/// Canal DMA reliant le WD1772 à la RAM du board à l'adresse DMA courante
/// (voir `peripherals::atari_st::wd1772::DmaChannel`) : le WD1772 ne connaît pas la
/// RAM, seulement ce canal.
///
/// Fait aussi respecter `dma_sector_count` (voir sa doc) : au-delà du
/// nombre de secteurs programmé, les octets sont silencieusement perdus
/// (lecture : RAM inchangée : écriture : `0` renvoyé au WD1772) plutôt que
/// transférés — comportement du vrai contrôleur DMA, indépendant de ce que
/// le WD1772 continuerait de faire tout seul.
struct RamDmaChannel<'a> {
    ram: &'a mut [u8],
    address: &'a mut u32,
    sector_count: &'a mut Option<u16>,
    bytes_in_sector: u32,
}

impl<'a> RamDmaChannel<'a> {
    fn transfer_allowed(&self) -> bool {
        !matches!(self.sector_count, Some(0))
    }

    fn advance(&mut self) {
        self.bytes_in_sector += 1;
        if self.bytes_in_sector >= SECTOR_SIZE as u32 {
            self.bytes_in_sector = 0;
            if let Some(count) = self.sector_count.as_mut() {
                *count = count.saturating_sub(1);
            }
        }
    }
}

/// Vue `Bus` d'une tranche de RAM, pour donner au Blitter (qui prend un
/// `Bus` générique) accès à la RAM du board sans emprunt réflexif de
/// `AtariSt` tout entier.
///
/// Doit aussi voir la ROM : le Blitter lit fréquemment ses données source
/// (masques d'icône, motifs) directement en ROM (`src_addr` dans la plage
/// `rom_base..`). Un `RamBus` ne connaissant que `ram` renvoyait `0xFF` pour
/// toute lecture ROM (adresse hors de `ram`, bien au-delà de sa taille
/// installée) au lieu du contenu réel — corruption silencieuse et
/// systématique de tout blit lisant sa source en ROM (masques d'icône lors
/// d'une sélection, motifs de menu), invisible aux tests qui rejouent le
/// Blitter via un bus HashMap avec ROM embarquée à la main plutôt que via ce
/// `RamBus` précis.
struct RamBus<'a> {
    ram: &'a mut [u8],
    rom: &'a [u8],
    rom_base: u32,
}

impl<'a> Bus for RamBus<'a> {
    fn read8(&mut self, addr: u32) -> u8 {
        if let Some(&b) = self.ram.get(addr as usize) {
            return b;
        }
        if addr >= self.rom_base && addr - self.rom_base < self.rom.len() as u32 {
            return self.rom[(addr - self.rom_base) as usize];
        }
        if AtariSt::in_floating_st_ram(addr) {
            return 0x00;
        }
        if std::env::var("RUST68_TRACE_RAMBUS_FALLBACK").is_ok() {
            eprintln!("[rambus] lecture hors RAM/ROM/flottante : addr={addr:#08x} -> 0xFF");
        }
        0xFF
    }

    fn write8(&mut self, addr: u32, value: u8) {
        if let Some(slot) = self.ram.get_mut(addr as usize) {
            *slot = value;
        }
        // ROM et au-delà : écriture ignorée (lecture seule / flottante),
        // même logique que `AtariSt::write8`.
    }
}

impl<'a> DmaChannel for RamDmaChannel<'a> {
    fn pull(&mut self) -> u8 {
        let byte = if self.transfer_allowed() {
            self.ram.get(*self.address as usize).copied().unwrap_or(0)
        } else {
            0
        };
        *self.address = self.address.wrapping_add(1);
        self.advance();
        byte
    }

    fn push(&mut self, byte: u8) {
        if self.transfer_allowed() {
            if let Some(slot) = self.ram.get_mut(*self.address as usize) {
                *slot = byte;
            }
        }
        *self.address = self.address.wrapping_add(1);
        self.advance();
    }
}

impl AtariSt {
    /// Accès en lecture à la RAM installée — uniquement pour le diagnostic
    /// (instantané complet déclenché par `RUST68_RAM_DUMP_KEY`, voir le
    /// binaire SDL2).
    pub fn ram(&self) -> &[u8] {
        &self.ram
    }

    /// Génère l'échantillon DMA Sound (STE) suivant, à `host_rate_hz` (voir
    /// [`DmaSound::next_sample`]) — emprunte `self.ram` et `self.dma_sound`
    /// séparément pour l'appelant (un binaire externe ne peut pas le faire
    /// lui-même : `ram()` emprunte tout `&self`, incompatible avec un
    /// emprunt simultané `&mut self.dma_sound`).
    ///
    /// Relaie aussi chaque front XSINT (fin de trame DMA, voir
    /// `DmaSound::take_xsint_pulses`) vers `Mfp::pulse_ta` : câblage
    /// matériel réel (XSINT sur l'entrée de comptage d'événements du Timer
    /// A), sans quoi un logiciel qui compte les bouclages de trame via ce
    /// timer (dont la cartouche de diagnostic usine STe, test Audio) ne
    /// verrait jamais l'interruption et retomberait sur un mécanisme de
    /// secours bien plus court que la vraie durée de lecture.
    pub fn next_dma_sample(&mut self, host_rate_hz: u32) -> (i8, i8) {
        let sample = self.dma_sound.next_sample(&self.ram, host_rate_hz);
        for _ in 0..self.dma_sound.take_xsint_pulses() {
            self.mfp.pulse_ta();
        }
        sample
    }

    /// Taille de la ROM chargée — pour construire un puits de traçage (voir
    /// [`Self::describe_addr_static`]) sans emprunter `AtariSt` lui-même.
    pub fn rom_len(&self) -> usize {
        self.rom.len()
    }

    /// Vrai si le modèle simulé possède un Blitter — même usage que
    /// [`Self::rom_len`].
    pub fn blitter_present(&self) -> bool {
        self.blitter_present
    }

    /// Crée un board avec `ram_size` octets de RAM installée à `0x000000`,
    /// `rom` (typiquement un dump TOS) mappée à `DEFAULT_ROM_BASE`, et le
    /// GLUE cadencé en PAL 50 Hz (le cas le plus courant — voir
    /// [`VideoMode`] pour du NTSC).
    pub fn new(ram_size: usize, rom: Vec<u8>) -> Self {
        let mut mfp = Mfp::new();
        mfp.set_gpip_input(GPIP_MONO_DETECT, true); // moniteur couleur (voir la constante)
        // État de repos réel de GPIP4/GPIP5 (`/IRQ` ACIA, `/INTRQ` WD1772,
        // actifs bas, tirés au niveau haut par défaut — voir `Self::tick`) :
        // sans cette initialisation, l'état interne par défaut du MFP (0)
        // masquerait la toute première transition vers "interruption active"
        // (elle calculerait aussi 0, donc aucun front détecté).
        mfp.set_gpip_input(4, true);
        mfp.set_gpip_input(5, true);
        AtariSt {
            ram: vec![0; ram_size],
            rom,
            rom_base: DEFAULT_ROM_BASE,
            cartridge: Vec::new(),
            trace_irq: std::env::var("RUST68_TRACE_IRQ").is_ok(),
            blitter_slice_cycle_acc: 0,
            ste_dma_sound: [0; (STE_DMA_SOUND_END - STE_DMA_SOUND_BASE + 1) as usize],
            dma_sound: DmaSound::new(),
            microwire: Microwire::new(),
            mfp,
            glue: Glue::new(VideoMode::Pal50),
            acia_keyboard: Acia::new(),
            ikbd: Ikbd::new(),
            acia_midi: Acia::new(),
            ym2149: Ym2149::new(),
            shifter: Shifter::new(),
            framebuffer: Vec::new(),
            last_absolute_line: 0,
            last_vbl_edge: 0,
            last_pc: 0,
            wd1772: Wd1772::new(),
            floppy_a: None,
            dma_register_select: 0,
            dma_address: 0,
            dma_sector_count_mode: false,
            dma_sector_count: None,
            blitter: Blitter::new(),
            overlay: true,
            memory_conf: 0,
            ram_bank_sizes: Self::ram_bank_sizes(ram_size),
            memory_conf_pin: None,
            blitter_present: true,
            bus_fault: None,
        }
    }

    /// Construit un board à partir d'un modèle connu de la gamme ST/STE
    /// (voir [`model`]) : RAM et présence du Blitter réglées d'après le
    /// modèle, `rom` fourni séparément (la version de TOS installée n'est
    /// pas une propriété du modèle — n'importe quel TOS compatible peut
    /// être flashé dans une machine réelle). La base ROM (`0xFC0000` vs
    /// `0xE00000`) reste à régler séparément via [`Self::set_rom_base`]
    /// une fois la version de TOS connue (voir `os_version` dans l'en-tête
    /// TOS, indépendant du modèle de machine).
    pub fn from_model(profile: model::MachineProfile, rom: Vec<u8>) -> Self {
        let mut st = Self::new(profile.ram_size, rom);
        st.blitter_present = profile.has_blitter;
        st.shifter.set_ste_palette(profile.ste_palette);
        st
    }

    /// Change l'adresse de base de la ROM après construction. Utile pour
    /// les TOS >= 1.06, mappés à `0xE00000` sur ST/STE réel plutôt qu'à
    /// [`DEFAULT_ROM_BASE`] (`0xFC0000`, valable pour TOS <= 1.04) — la
    /// taille de 256 Ko de ces TOS plus récents ne tiendrait de toute façon
    /// pas entre `0xFC0000` et le début de la zone d'E/S (`0xFF8000`).
    pub fn set_rom_base(&mut self, base: u32) {
        self.rom_base = base;
    }

    /// Change le mode vidéo (PAL 50 Hz par défaut à la construction, voir
    /// [`Self::new`]) — remplace le GLUE, donc à appeler juste après la
    /// construction/avant le premier `reset`, pas en cours d'émulation (sans
    /// quoi le compteur de ligne/trame en cours serait perdu).
    pub fn set_video_mode(&mut self, mode: VideoMode) {
        self.glue = Glue::new(mode);
    }

    /// Insère une image ROM de cartouche, mappée en lecture seule à partir
    /// de `CARTRIDGE_BASE` (`$FA0000`) — port cartouche ST/STE réel, utilisé
    /// notamment par les cartouches de diagnostic matériel. `data` doit déjà
    /// être au format natif 68000 (mots big-endian) ; voir `atari_st_sdl2`
    /// pour l'entrelacement de deux images HGH/LOW séparées (ROMs 8 bits
    /// jumelées, format EPROM courant pour ces cartouches).
    pub fn load_cartridge(&mut self, data: Vec<u8>) {
        self.cartridge = data;
    }

    /// Vrai si une cartouche a été chargée (voir [`Self::load_cartridge`]).
    /// Utile pour savoir si le raccourci "redémarrage à chaud" du TOS (voir
    /// [`Self::pin_memory_conf`]) a lieu d'être : une cartouche de
    /// diagnostic fait sa propre initialisation matérielle complète et ne
    /// passe pas par le boot TOS normal, donc pas par ce raccourci — y
    /// figer `MEMORY_CONF` casserait sa propre détection RAM (qui a
    /// justement besoin d'écrire ce registre librement pour observer le
    /// mirroring d'adresse et se corriger elle-même).
    pub fn has_cartridge(&self) -> bool {
        !self.cartridge.is_empty()
    }

    /// Fait progresser les périphériques (MFP + GLUE + YM2149) de
    /// `cpu_cycles` cycles CPU, relaie l'IRQ combinée des deux ACIA sur
    /// `GPIP4` du MFP (OR câblé, câblage réel ST/STE), et déclenche le
    /// rendu vidéo (`Shifter`) au rythme HBL/VBL du GLUE. À appeler par
    /// l'appelant après chaque `Cpu::step` :
    ///
    /// ```
    /// use rust68::{Cpu, systems::atari_st::AtariSt};
    ///
    /// let mut st = AtariSt::new(0x1000, vec![]);
    /// let mut cpu = Cpu::new();
    /// cpu.reset(&mut st);
    /// let cycles = cpu.step(&mut st).unwrap();
    /// st.tick(cycles);
    /// ```
    pub fn tick(&mut self, cpu_cycles: u32) {
        // Fait avancer un blit non-HOG en pause (voir `Blitter::execute`,
        // tranches de 16 mots) indépendamment de toute écriture CPU du
        // registre CONTROL. Sur silicium réel, le Blitter progresse de
        // façon autonome (cycles de bus partagés avec le CPU au rythme du
        // matériel), pas seulement quand le logiciel réécrit CONTROL —
        // notre modèle antérieur ne reprenait le blit QUE sur une écriture
        // de CONTROL avec le bit BUSY posé, ce qui fonctionnait par
        // coïncidence avec la boucle `TAS.B` de TOS (qui EST une écriture)
        // mais bloquait indéfiniment tout logiciel scrutant BUSY par simple
        // lecture (`BTST.B`, sans réécriture) — confirmé en pratique avec
        // la cartouche de diagnostic usine STe (test G2 "endmask", blit
        // large de 40 mots dépassant une tranche, jamais relancé).
        if self.blitter_present && self.blitter.busy() {
            self.blitter_slice_cycle_acc += cpu_cycles;
            if self.blitter_slice_cycle_acc >= BLITTER_SLICE_CYCLES {
                self.blitter_slice_cycle_acc -= BLITTER_SLICE_CYCLES;
                let mut ram_bus = RamBus {
                    ram: &mut self.ram,
                    rom: &self.rom,
                    rom_base: self.rom_base,
                };
                self.blitter.execute(&mut ram_bus);
            }
        } else {
            self.blitter_slice_cycle_acc = 0;
        }
        self.mfp.tick(cpu_cycles);
        self.glue.tick(cpu_cycles);
        self.ym2149.tick(cpu_cycles);
        self.ikbd.tick(cpu_cycles);
        // Commandes envoyées par le programme (reset, mode souris…) : les
        // relayer de l'émission de l'ACIA vers l'IKBD, qui les interprète.
        while let Some(byte) = self.acia_keyboard.take_tx_byte() {
            self.ikbd.receive_cmd(byte);
        }
        // Ne pousse l'octet suivant dans l'ACIA que si le précédent a bien
        // été consommé par le programme (RDRF retombé) — lecture du
        // registre de statut, sans effet de bord (contrairement à une
        // lecture du registre de données, qui acquitte RDRF).
        //
        // Sur silicium réel, `/IRQ` de l'ACIA remonte réellement à 1 (le
        // temps de l'intervalle série entre deux octets) avant de retomber
        // pour l'octet suivant — un vrai front à chaque fois. Ici, pousser
        // l'octet suivant dans le même tick() que celui où RDRF vient de
        // retomber masque cette remontée : GPIP4 resterait à 0 en continu
        // entre deux octets d'une même rafale (ex. les 3 octets d'une trame
        // souris), et `Mfp::set_gpip_input` — à juste titre edge-triggered —
        // ne verrait alors jamais de front pour les octets suivants, les
        // laissant bloqués jusqu'à ce qu'un évènement sans rapport (une
        // autre écriture de registre MFP) déclenche un front incident. Bug
        // exact déjà isolé et corrigé dans le projet compagnon Stay (voir
        // `Bus::read_acia_ikbd_data`) : forcer explicitement le relâchement
        // (niveau haut) avant de réarmer RDRF pour l'octet suivant, dans le
        // même tick, pour garantir un vrai front montant-puis-descendant.
        if self.acia_keyboard.read(acia::reg::CONTROL_STATUS) & 0x01 == 0 {
            if let Some(byte) = self.ikbd.pop_tx() {
                self.mfp.set_gpip_input(4, true);
                self.acia_keyboard.push_rx_byte(byte);
            }
        }
        // `/IRQ` (ACIA) et `/INTRQ` (WD1772) sont des signaux matériels réels
        // actifs bas (asserted = niveau logique 0, comme leur nom l'indique)
        // câblés directement sur GPIP4/GPIP5 — sans inverseur, GPIP doit
        // donc lire 0 quand l'interruption est active, 1 au repos. Un TOS
        // réel sonde parfois ce niveau brut directement (pas seulement via
        // le canal d'interruption edge-triggered du MFP) : au boot, par
        // exemple, la détection du nombre de lecteurs de disquette attend
        // que GPIP5 passe à 0 après une commande WD1772, avec un timeout —
        // sans cette inversion, le bit ne descend jamais à 0 et le TOS
        // conclut à tort qu'aucun lecteur n'est présent (`_nflops` reste à
        // 0, aucune icône A: sur le bureau).
        // Fait progresser une commande WD1772 en cours (voir la doc de
        // `wd1772::Wd1772::tick` — vitesse de pas, latence de rotation,
        // débit de transfert réels, plutôt que l'exécution instantanée
        // d'une version précédente qui rendait toute la disquette bien
        // trop rapide). Câblage face/lecteur : voir `floppy_drive_select`,
        // relu ici aussi (pas seulement à l'écriture de commande) car une
        // commande multi-secteurs peut s'étaler sur plusieurs `tick()`.
        {
            let (drive_a_selected, side) = self.floppy_drive_select();
            self.wd1772.side = side;
            let disk = if drive_a_selected { self.floppy_a.as_deref_mut() } else { None };
            let mut channel = RamDmaChannel {
                ram: &mut self.ram,
                address: &mut self.dma_address,
                sector_count: &mut self.dma_sector_count,
                bytes_in_sector: 0,
            };
            self.wd1772.tick(cpu_cycles, disk, &mut channel);
        }

        let acia_irq = self.acia_keyboard.irq_requested() || self.acia_midi.irq_requested();
        self.mfp.set_gpip_input(4, !acia_irq);
        self.mfp.set_gpip_input(5, !self.wd1772.interrupt_requested());

        // Recharge le compteur vidéo du Shifter au front VBL (transition
        // ligne visible -> blanking, voir `Glue::vbl_edge_count`), PAS au
        // bouclage complet de la trame (`Glue::frame_count`) : sur silicium
        // réel, la base est rechargée dès le début du blanking vertical,
        // qui précède la ligne 0 de la trame suivante de tout le reste du
        // blanking (~113 lignes en PAL) — pas dans le même souffle. Utiliser
        // `frame_count` ici rendait la ligne visible 0 de la trame suivante
        // déjà rendue (et son pulse Timer B déjà émis, voir plus bas) dans
        // le MÊME appel `tick()` que celui où VBL vient tout juste de
        // s'armer, ne laissant absolument aucune fenêtre au logiciel pour
        // prendre l'interruption VBL avant que cette ligne ne soit déjà
        // consommée — confirmé nécessaire par la cartouche de diagnostic
        // usine STe (test "T4 Video Counter in Memory Controller").
        let vbl_edge_now = self.glue.vbl_edge_count();
        if vbl_edge_now != self.last_vbl_edge {
            self.last_vbl_edge = vbl_edge_now;
            self.shifter.start_frame();
        }
        let lines_per_frame = self.glue.lines_per_frame() as u64;
        // Compteur absolu (jamais remis à zéro) pour ne pas confondre "une
        // trame entière vient de s'écouler" avec "aucune ligne écoulée"
        // quand current_line() revient à 0 en bouclant.
        let absolute_line_now =
            self.glue.frame_count() * lines_per_frame + self.glue.current_line() as u64;
        // Borne défensive : ne rattrape jamais plus d'une trame complète en
        // un seul tick (cas normal : 0 ou 1 ligne, tick() étant appelé après
        // chaque instruction, bien plus fréquemment qu'une ligne = 512 cycles).
        let mut guard = 0u64;
        while self.last_absolute_line < absolute_line_now && guard < lines_per_frame {
            self.last_absolute_line += 1;
            let idx = (self.last_absolute_line % lines_per_frame) as usize;
            // Câblage matériel réel ST/STE : le Shifter ne fetch (et donc
            // n'avance son compteur vidéo) QUE pendant les lignes visibles
            // (200, quel que soit PAL/NTSC) — pas pendant le blanking
            // vertical. Idem pour l'entrée externe TBI du Timer B du MFP,
            // reliée au signal de balayage actif (DE), pas au HBL brut :
            // elle ne pulse que sur ces mêmes lignes visibles. C'est
            // exactement ce que le boot TOS exploite pour détecter qu'il
            // vient d'entrer en VBL : il programme le Timer B en mode
            // event-count puis attend que la valeur cesse de changer (~615
            // lectures stables), ce qui n'arrive jamais tant qu'on reste
            // dans la zone visible.
            if idx < VISIBLE_LINES {
                let row = self.shifter.render_scanline(&self.ram);
                if idx >= self.framebuffer.len() {
                    self.framebuffer.resize(idx + 1, Vec::new());
                }
                self.framebuffer[idx] = row;
                self.mfp.pulse_tb();
            }
            guard += 1;
        }
        if guard >= lines_per_frame && self.last_absolute_line < absolute_line_now && std::env::var("RUST68_TRACE_VECTORS").is_ok() {
            eprintln!(
                "[trace] tick() : rattrapage vidéo tronqué par la garde (retard restant : {} lignes)",
                absolute_line_now - self.last_absolute_line
            );
        }
    }

    fn mfp_offset(addr: u32) -> Option<u8> {
        if addr >= MFP_BASE && addr <= MFP_END && (addr - MFP_BASE) % 2 == 0 {
            Some(((addr - MFP_BASE) / 2) as u8)
        } else {
            None
        }
    }

    fn in_rom(&self, addr: u32) -> bool {
        addr >= self.rom_base && addr - self.rom_base < self.rom.len() as u32
    }

    /// Vrai si `addr` (déjà connue hors de la RAM installée, c'est-à-dire
    /// `addr >= self.ram.len()`) tombe dans l'espace d'adressage "RAM ST"
    /// fixe de 4 Mo (voir [`ST_RAM_ADDRESS_SPACE`]) — où un accès ne
    /// déclenche **jamais** de bus error sur silicium réel, même sans RAM
    /// physique à cette adresse précise, contrairement au vrai "trou"
    /// au-delà, avant `IO_BASE`.
    ///
    /// Confirmé par le code source de Hatari (`stMemory.c`) : cette zone
    /// est mappée sur `VoidMem_bank`, dont les lectures renvoient une valeur
    /// fixe (`nonexistingdata()` = 0) et les écritures sont silencieusement
    /// ignorées (`dummy_get`/`dummy_put`), sans jamais lever de bus error —
    /// le vrai "trou" (`BusErrMem_bank` chez Hatari) ne commence, lui,
    /// qu'à 4 Mo.
    ///
    /// Historique : deux tentatives antérieures de mirroring d'adresse par
    /// banque MMU (pour, en plus, satisfaire une cartouche de diagnostic
    /// usine dont l'heuristique rapide de taille RAM conclut "2 Mo" au lieu
    /// de 1 Mo pour un 1040STE) avaient été essayées et abandonnées — l'une
    /// faisait conclure au TOS 4 Mo au lieu d'1 Mo, l'autre (un vrai bus
    /// error ici, retenté puis annulé dans cette même session) provoquait un
    /// double bus fault (SP encore dérivé de l'en-tête ROM — "os_entry", pas
    /// un vrai SSP — au moment du tout premier accès hors RAM, avant que le
    /// TOS n'ait eu la moindre chance d'installer le sien).
    ///
    /// Vérifié directement contre Hatari (capture d'écran à l'appui) que le
    /// mirroring EST bien nécessaire — Hatari affiche correctement "1M RAM"
    /// pour ce même TOS/cartouche/1040STE, pas "2M". Le vrai mécanisme (voir
    /// [`Self::translate_ram_addr`]) est plus étroit que les tentatives
    /// précédentes : un mirroring purement INTRA-banque, piloté par
    /// [`MEMORY_CONF`], qui devient l'identité dès que ce registre reflète
    /// la RAM réellement installée (le cas normal, une fois le TOS booté) —
    /// donc sans le risque des tentatives précédentes (ni bus error, ni
    /// mirroring global hors de propos).
    fn in_floating_st_ram(addr: u32) -> bool {
        addr < ST_RAM_ADDRESS_SPACE
    }

    /// Taille réellement peuplée de chaque banque RAM STE pour une RAM
    /// totale de `ram_len` octets — reproduction exacte de la table de
    /// `STMemory_RAM_SetBankSize` (Hatari, `stMemory.c`), seules
    /// configurations standard sur silicium réel (banques par paires de
    /// 128/512/2048 Ko). `None` si `ram_len` ne correspond à aucune d'elles
    /// (auquel cas [`Self::translate_ram_addr`] ne traduit rien).
    fn ram_bank_sizes(ram_len: usize) -> Option<(u32, u32)> {
        const KB: usize = 1024;
        Some(match ram_len / KB {
            128 => (128 * 1024, 0),
            256 => (128 * 1024, 128 * 1024),
            512 => (512 * 1024, 0),
            640 => (512 * 1024, 128 * 1024),
            1024 => (512 * 1024, 512 * 1024),
            2048 => (2048 * 1024, 0),
            2176 => (2048 * 1024, 128 * 1024),
            2560 => (2048 * 1024, 512 * 1024),
            4096 => (2048 * 1024, 2048 * 1024),
            _ => return None,
        })
    }

    /// Valeur de [`MEMORY_CONF`] correspondant à une RAM totale de
    /// `ram_len` octets CORRECTEMENT configurée (bits 3-2 = banque 0,
    /// bits 1-0 = banque 1) — même table que [`Self::ram_bank_sizes`],
    /// exprimée en code MEMCONF plutôt qu'en taille de banque. `None` si
    /// `ram_len` ne correspond à aucune configuration standard.
    ///
    /// À utiliser pour pré-remplir `MEMORY_CONF` avant un démarrage à chaud
    /// (voir `atari_st_sdl2`) : le raccourci "redémarrage à chaud" saute
    /// précisément le code TOS qui configurerait normalement ce registre
    /// (même raisonnement que pour `memvalid`/`phystop`) — sans ce
    /// pré-remplissage, [`Self::translate_ram_addr`] verrait `MEMORY_CONF`
    /// resté à sa valeur de reset (`0`, soit 128 Ko + 128 Ko) et rendrait
    /// inaccessible (flottante) toute la RAM au-delà de 256 Ko.
    pub fn expected_memory_conf(ram_len: usize) -> Option<u8> {
        const KB: usize = 1024;
        Some(match ram_len / KB {
            128 => (0 << 2) | 0,
            256 => (0 << 2) | 0,
            512 => (1 << 2) | 0,
            640 => (1 << 2) | 0,
            1024 => (1 << 2) | 1,
            2048 => (2 << 2) | 0,
            2176 => (2 << 2) | 0,
            2560 => (2 << 2) | 1,
            4096 => (2 << 2) | 2,
            _ => return None,
        })
    }

    /// Fige la valeur logicielle de [`MEMORY_CONF`] à `value` — toute
    /// écriture ultérieure du CPU dans ce registre (`write8`) est acceptée
    /// (l'overlay se désactive normalement) mais n'affecte plus la valeur
    /// mémorisée, qui reste `value`. `None` (défaut) : comportement normal,
    /// le CPU contrôle entièrement ce registre.
    ///
    /// À utiliser avec le raccourci "redémarrage à chaud" (voir
    /// `atari_st_sdl2`, à côté de son pré-remplissage équivalent de
    /// `memvalid`/`phystop`) : contrairement à ces derniers (simplement LUS
    /// par le TOS pour décider chaud/froid), le TOS écrit
    /// INCONDITIONNELLEMENT `MEMORY_CONF=0` très tôt au boot (avant même de
    /// consulter `memvalid`), qui n'est normalement corrigé qu'à la toute
    /// fin de l'algorithme de détection RAM — algorithme que le raccourci
    /// saute justement. Un simple pré-remplissage ponctuel se fait donc
    /// aussitôt écraser ; le figer ici le fait survivre à cette écriture
    /// intermédiaire, exactement comme si la détection avait réellement eu
    /// lieu et avait conclu la bonne valeur.
    pub fn pin_memory_conf(&mut self, value: u8) {
        self.memory_conf_pin = Some(value);
    }

    /// Décode un champ 2 bits de [`MEMORY_CONF`] en taille de banque
    /// logique (`00`=128 Ko, `01`=512 Ko, `10`=2 Mo, `11`=réservé/invalide,
    /// traité comme absent) — reproduction de `STMemory_MMU_Size` (Hatari).
    fn mmu_bank_size_from_code(code: u8) -> u32 {
        match code & 0x3 {
            0 => 128 * 1024,
            1 => 512 * 1024,
            2 => 2048 * 1024,
            _ => 0,
        }
    }

    /// Traduit une adresse logique CPU en offset physique dans `self.ram` —
    /// `None` si l'adresse tombe hors de la RAM installée (doit alors
    /// retomber dans [`Self::in_floating_st_ram`]).
    ///
    /// Sur STE/Mega STE avec une configuration de banques standard (voir
    /// [`Self::ram_bank_sizes`]), reproduit le mirroring d'adresse
    /// intra-banque du MMU/MCU (`STMemory_MMU_Translate_Addr_STE`, Hatari) :
    /// [`MEMORY_CONF`] (bits 3-2 = taille logique banque 0, bits 1-0 =
    /// banque 1) attribue à chaque banque une taille que le logiciel croit
    /// vraie ; si elle dépasse la taille RÉELLEMENT peuplée, les adresses
    /// au-delà de la taille réelle mais dans la taille logique "bouclent"
    /// (adressage DRAM incomplet : certaines lignes de colonne/rangée ne
    /// sont simplement pas câblées pour une puce plus petite que
    /// l'emplacement prévu). Démontré chez Hatari : la formule se réduit
    /// systématiquement à `addr_logique & (taille_réelle - 1)`,
    /// indépendamment de la taille logique précise (seul son ordre de
    /// grandeur, via le dispatch de banque ci-dessous, importe) — d'où
    /// l'implémentation simplifiée. Devient l'identité dès que
    /// `MEMORY_CONF` reflète la RAM réellement installée (le cas normal, une
    /// fois le TOS booté) : aucun changement de comportement hors de la
    /// fenêtre de démarrage où la configuration est encore incorrecte/par
    /// défaut.
    ///
    /// Sur ST/Mega ST (`!self.blitter_present`) ou pour une taille de RAM
    /// non standard (`ram_bank_sizes` = `None`) : mappage direct, comme
    /// avant l'introduction de ce mirroring — la formule STF (non-STE)
    /// diffère (réordonnancement différent des bits colonne/rangée) et
    /// n'est pas reproduite ici faute de besoin démontré.
    fn translate_ram_addr(&self, addr: u32) -> Option<usize> {
        let Some((ram_b0, ram_b1)) = self.ram_bank_sizes.filter(|_| self.blitter_present) else {
            return if (addr as usize) < self.ram.len() { Some(addr as usize) } else { None };
        };
        let mmu_b0 = Self::mmu_bank_size_from_code(self.memory_conf >> 2);
        let mmu_b1 = Self::mmu_bank_size_from_code(self.memory_conf);
        if addr < mmu_b0 {
            if ram_b0 == 0 {
                return None;
            }
            Some((addr & (ram_b0 - 1)) as usize)
        } else if addr < mmu_b0.saturating_add(mmu_b1) {
            if ram_b1 == 0 {
                return None;
            }
            let off = (addr - mmu_b0) & (ram_b1 - 1);
            Some((ram_b0 + off) as usize)
        } else {
            None
        }
    }

    fn is_shifter_addr(addr: u32) -> bool {
        matches!(
            addr,
            shifter::addr::VIDEO_BASE_HIGH
                | shifter::addr::VIDEO_BASE_MID
                | shifter::addr::VIDEO_BASE_LOW
                | shifter::addr::VIDEO_COUNTER_HIGH
                | shifter::addr::VIDEO_COUNTER_MID
                | shifter::addr::VIDEO_COUNTER_LOW
                | shifter::addr::RESOLUTION
        ) || (shifter::addr::PALETTE_BASE..shifter::addr::PALETTE_BASE + 32).contains(&addr)
    }

    fn is_blitter_addr(&self, addr: u32) -> bool {
        self.blitter_present && (BLITTER_BASE..BLITTER_BASE + blitter::reg::END).contains(&addr)
    }

    /// Décode les lignes de sélection lecteur/face du connecteur disquette,
    /// portées par le port A du YM2149 (voir la doc de
    /// [`ym2149::Ym2149::port_a_output`]) — renvoie `(lecteur A sélectionné,
    /// face)`. Sans ce câblage, `self.wd1772.side` resterait toujours à sa
    /// valeur par défaut (0) quoi que TOS programme, rendant illisible tout
    /// contenu situé sur la face 1 d'une disquette double face (le cas de
    /// pratiquement tout logiciel ST réel au format `.st` 720 Ko).
    fn floppy_drive_select(&self) -> (bool, u8) {
        let port_a = self.ym2149.port_a_output();
        let drive_a_selected = port_a & 0x02 == 0;
        let side = !port_a & 0x01;
        (drive_a_selected, side)
    }

    /// Vrai si `off` (offset relatif à [`STE_DMA_SOUND_BASE`]) correspond à
    /// un registre géré par [`dma_sound::DmaSound`] (voir son module
    /// [`dma_sound::reg`]) plutôt qu'au stockage générique
    /// (`self.ste_dma_sound`) — le Microwire (`$FF8922`/`$FF8923`, offsets
    /// `0x22`/`0x23`) reste volontairement hors de cette liste, géré à part
    /// (voir la doc sur `STE_MICROWIRE_DATA`).
    fn is_dma_sound_reg(off: u32) -> bool {
        use dma_sound::reg;
        matches!(
            off,
            reg::CONTROL_LOW
                | reg::FRAME_START_HIGH
                | reg::FRAME_START_MID
                | reg::FRAME_START_LOW
                | reg::FRAME_COUNT_HIGH
                | reg::FRAME_COUNT_MID
                | reg::FRAME_COUNT_LOW
                | reg::FRAME_END_HIGH
                | reg::FRAME_END_MID
                | reg::FRAME_END_LOW
                | reg::SOUND_MODE
        )
    }

    /// Étiquette de composant pour `addr`, pour [`crate::trace::FileTraceSink`]
    /// (voir `RUST68_TRACE_ALL`) — reproduit fidèlement l'ordre de décision
    /// de [`Bus::read8`] ci-dessous (même priorité entre RAM/ROM et
    /// périphériques), afin que l'étiquette corresponde toujours à ce que
    /// l'accès a *réellement* touché, pas à une classification approximative
    /// indépendante. N'observe pas `self.overlay` (état vrai seulement
    /// pendant les toutes premières instructions du boot froid) : voir
    /// [`Self::describe_addr_static`], dont ceci n'est qu'un raccourci —
    /// l'étiquette "ram" est renvoyée à la place pendant cette courte
    /// fenêtre, sans conséquence puisque c'est purement descriptif (le vrai
    /// dispatch dans `read8`/`write8` reste, lui, inchangé).
    pub fn describe_addr(&self, addr: u32) -> &'static str {
        Self::describe_addr_static(self.ram.len(), self.rom_base, self.rom.len(), self.blitter_present, addr)
    }

    /// Version sans `&self` de [`Self::describe_addr`] — pour les appelants
    /// (comme le puits de traçage `RUST68_TRACE_ALL`, voir
    /// `bin/atari_st_sdl2.rs`) qui ne peuvent pas emprunter `AtariSt` tout en
    /// l'enveloppant simultanément dans un [`crate::TracingBus`] mutable.
    /// Les paramètres capturent tout ce dont la classification a besoin,
    /// figé à la construction (jamais modifié ensuite).
    pub fn describe_addr_static(
        ram_len: usize,
        rom_base: u32,
        rom_len: usize,
        blitter_present: bool,
        addr: u32,
    ) -> &'static str {
        let addr = addr & ADDR_MASK;
        if (addr as usize) < ram_len {
            return "ram";
        }
        if Self::in_floating_st_ram(addr) {
            return "floating";
        }
        if Self::mfp_offset(addr).is_some() {
            return "mfp";
        }
        match addr {
            ACIA_KEYBOARD_CONTROL | ACIA_KEYBOARD_DATA => return "acia-keyboard",
            ACIA_MIDI_CONTROL | ACIA_MIDI_DATA => return "acia-midi",
            YM2149_SELECT | YM2149_DATA => return "ym2149",
            _ if Self::is_shifter_addr(addr) => return "shifter",
            FDC_DATA | DMA_MODE | DMA_ADDR_HIGH | DMA_ADDR_MID | DMA_ADDR_LOW => return "wd1772-dma",
            _ if blitter_present && (BLITTER_BASE..BLITTER_BASE + blitter::reg::END).contains(&addr) => {
                return "blitter";
            }
            _ if (STE_DMA_SOUND_BASE..=STE_DMA_SOUND_END).contains(&addr) => return "ste-dma-sound",
            _ => {}
        }
        if addr >= rom_base && addr - rom_base < rom_len as u32 {
            return "rom";
        }
        if (IO_BASE..=IO_END).contains(&addr) {
            return "io-non-implemente";
        }
        if (CARTRIDGE_BASE..=CARTRIDGE_END).contains(&addr) {
            return "cartouche";
        }
        "fault"
    }
}

impl Bus for AtariSt {
    fn read8(&mut self, addr: u32) -> u8 {
        let addr = addr & ADDR_MASK;
        if self.overlay && addr < OVERLAY_SIZE && (addr as usize) < self.rom.len() {
            return self.rom[addr as usize];
        }
        if let Some(phys) = self.translate_ram_addr(addr) {
            return self.ram[phys];
        }
        if Self::in_floating_st_ram(addr) {
            // Au-delà de la RAM installée mais dans l'espace "RAM ST" (4 Mo) :
            // jamais de bus error sur silicium réel (voir la doc du module),
            // valeur fixe non stockée (jamais ce qui vient d'être écrit) —
            // confirmé par le code source de Hatari (`stMemory.c`,
            // `VoidMem_bank`/`dummy_get`) : cette zone renvoie une valeur
            // fixe sans jamais fauter, contrairement au vrai "trou" au-delà
            // de 4 Mo (`BusErrMem_bank` chez Hatari, avant `IO_BASE` chez
            // nous). Ne PAS confondre avec un défaut d'aliasing de banque
            // MMU (qui, lui, existe réellement chez Hatari mais ne
            // s'applique qu'à l'intérieur d'une banque physiquement
            // peuplée — non modélisé ici, voir la doc de
            // `in_floating_st_ram`).
            return 0x00;
        }
        if let Some(off) = Self::mfp_offset(addr) {
            return self.mfp.read(off);
        }
        match addr {
            ACIA_KEYBOARD_CONTROL => return self.acia_keyboard.read(acia::reg::CONTROL_STATUS),
            ACIA_KEYBOARD_DATA => {
                let v = self.acia_keyboard.read(acia::reg::DATA);
                if std::env::var("RUST68_TRACE_IKBD").is_ok() {
                    eprintln!("[ikbd] lecture ACIA_KEYBOARD_DATA -> {v:#04x}");
                }
                return v;
            }
            ACIA_MIDI_CONTROL => return self.acia_midi.read(acia::reg::CONTROL_STATUS),
            ACIA_MIDI_DATA => return self.acia_midi.read(acia::reg::DATA),
            YM2149_SELECT => return self.ym2149.read(ym2149::bus_offset::SELECT),
            YM2149_DATA => return self.ym2149.read(ym2149::bus_offset::DATA),
            _ if Self::is_shifter_addr(addr) => return self.shifter.read(addr),
            FDC_DATA => return self.wd1772.read(self.dma_register_select),
            DMA_MODE => return self.dma_register_select,
            DMA_ADDR_HIGH => return (self.dma_address >> 16) as u8,
            DMA_ADDR_MID => return (self.dma_address >> 8) as u8,
            DMA_ADDR_LOW => return self.dma_address as u8,
            _ if self.is_blitter_addr(addr) => return self.blitter.read(addr - BLITTER_BASE),
            // Microwire DATA (`$FF8922`/`$FF8923`) : toujours 0 en lecture,
            // quoi qu'on y ait écrit — simule un décalage série toujours déjà
            // terminé (silicium réel : ce registre se vide progressivement
            // pendant le décalage ; sans émuler le vrai timing série, le
            // logiciel qui écrit puis boucle en l'attendant à zéro doit
            // trouver zéro immédiatement, pas boucler indéfiniment). Le
            // registre MASK (`$FF8924`) et le reste de la plage restent un
            // stockage lecture/écriture fidèle normal.
            STE_MICROWIRE_DATA | STE_MICROWIRE_DATA1 => return 0x00,
            _ if (STE_DMA_SOUND_BASE..=STE_DMA_SOUND_END).contains(&addr) => {
                let off = addr - STE_DMA_SOUND_BASE;
                if Self::is_dma_sound_reg(off) {
                    return self.dma_sound.read(off);
                }
                return self.ste_dma_sound[off as usize];
            }
            _ => {}
        }
        if self.in_rom(addr) {
            return self.rom[(addr - self.rom_base) as usize];
        }
        if (CARTRIDGE_BASE..=CARTRIDGE_END).contains(&addr) {
            let off = (addr - CARTRIDGE_BASE) as usize;
            if off < self.cartridge.len() {
                return self.cartridge[off];
            }
            return 0xFF;
        }
        if (IO_BASE..=IO_END).contains(&addr) {
            return 0xFF;
        }
        if std::env::var("RUST68_TRACE_VECTORS").is_ok() {
            eprintln!("[trace] bus fault en lecture : addr={addr:#x}");
        }
        self.bus_fault = Some((addr, false));
        0xFF
    }

    // Registres de palette Shifter (`$FF8240`-`$FF825E`) : sur le silicium
    // réel (confirmé par Hatari, `Video_ColorReg_WriteWord`), un accès `.W`
    // ou `.L` du CPU écrit le mot normalement, mais un accès `.B` ISOLÉ
    // duplique l'octet écrit dans les deux moitiés du mot avant masquage
    // (voir la doc de [`shifter::Shifter::write`]). Le `write8` par défaut
    // ne voit jamais qu'un octet à la fois et ne peut donc pas distinguer
    // ces deux cas — cette surcharge de `write16` intercepte les VRAIS
    // accès mot pour cette plage précise et les route vers
    // `write_palette_word`, qui n'applique pas la duplication.
    fn write16(&mut self, addr: u32, value: u16) {
        let masked = addr & ADDR_MASK;
        if (shifter::addr::PALETTE_BASE..shifter::addr::PALETTE_BASE + 32).contains(&masked) {
            self.shifter.write_palette_word(masked, value);
            return;
        }
        // Registres Blitter 16 bits (SRC_X_INC/SRC_Y_INC/ENDMASK1-3/
        // DST_X_INC/DST_Y_INC/X_COUNT/Y_COUNT) : sur le silicium réel, un
        // accès `.B` ISOLÉ à l'un de ces registres est ignoré (confirmé par
        // Hatari, `Blitter_CheckAccess_Byte`) — seul un accès `.W`/`.L`
        // complet est honoré. Le `write8` par défaut ne voit jamais qu'un
        // octet à la fois et ne peut donc pas faire cette distinction :
        // cette surcharge intercepte les VRAIS accès mot pour ces
        // registres précis et les route vers `Blitter::write_word` (voir sa
        // doc) plutôt que vers la composition octet par octet.
        if self.blitter_present && (BLITTER_BASE..BLITTER_BASE + blitter::reg::END).contains(&masked) {
            let reg_offset = masked - BLITTER_BASE;
            if reg_offset < 0x20
                || matches!(
                    reg_offset,
                    blitter::reg::SRC_X_INC
                        | blitter::reg::SRC_Y_INC
                        | blitter::reg::ENDMASK_1
                        | blitter::reg::ENDMASK_2
                        | blitter::reg::ENDMASK_3
                        | blitter::reg::DST_X_INC
                        | blitter::reg::DST_Y_INC
                        | blitter::reg::X_COUNT
                        | blitter::reg::Y_COUNT
                )
            {
                self.blitter.write_word(reg_offset, value);
                return;
            }
        }
        // `DMA_MODE` ($FF8606) : registre 16 bits réel (voir sa doc) — le
        // sélecteur de registre FDC vit dans l'octet BAS du mot. TOS y
        // accède quasi systématiquement en mot complet ; une composition
        // octet par octet naïve (comme la voie générique ci-dessous) ne
        // verrait que l'octet HAUT (toujours nul pour ces petites valeurs),
        // laissant le sélecteur bloqué à 0 en permanence — d'où la
        // sélection FDC entièrement cassée que cette interception corrige.
        if masked == DMA_MODE {
            // Délègue à `write8` (pas de logique dupliquée ici) : évite que
            // les deux chemins divergent, comme cela avait déjà causé un
            // oubli du bit 4 (mode compteur de secteurs DMA) la première
            // fois que ce cas a été traité séparément ici.
            self.write8(DMA_MODE, value as u8);
            return;
        }
        // `FDC_DATA` ($FF8604) : registre 16 bits réel lui aussi (même
        // remarque que `DMA_MODE` ci-dessus) — l'octet du registre WD1772
        // réellement sélectionné (commande/statut/piste/secteur/donnée)
        // vit dans l'octet BAS du mot, confirmé par Hatari (`fdc.c`,
        // `FDC_DiskController_WriteWord` : `IoMem_ReadByte(0xff8605)`).
        // TOS y accède quasi systématiquement en mot complet ; sans cette
        // interception, la composition octet par octet générique ne
        // verrait que l'octet HAUT (toujours nul), et TOUTE commande/
        // valeur de piste/secteur/donnée écrite ainsi serait perdue —
        // rendant toute lecture de disquette impossible.
        if masked == FDC_DATA {
            self.write8(FDC_DATA, value as u8);
            return;
        }
        self.write8(addr, (value >> 8) as u8);
        self.write8(addr.wrapping_add(1), value as u8);
    }

    /// Voir la doc de [`Self::write16`] sur `FDC_DATA` — même interception
    /// symétrique en lecture (l'octet WD1772 réel vit dans l'octet BAS du
    /// mot, la composition générique par défaut le mettrait dans l'octet
    /// HAUT).
    fn read16(&mut self, addr: u32) -> u16 {
        let masked = addr & ADDR_MASK;
        if masked == FDC_DATA {
            return self.read8(FDC_DATA) as u16;
        }
        let hi = self.read8(addr) as u16;
        let lo = self.read8(addr.wrapping_add(1)) as u16;
        (hi << 8) | lo
    }

    fn write32(&mut self, addr: u32, value: u32) {
        let masked = addr & ADDR_MASK;
        // SRC_ADDR/DST_ADDR du Blitter : registres 32 bits (24 bits
        // significatifs), même principe que `write16` ci-dessus — seul un
        // accès `.L` complet est honoré sur le silicium réel.
        if self.blitter_present && (BLITTER_BASE..BLITTER_BASE + blitter::reg::END).contains(&masked) {
            let reg_offset = masked - BLITTER_BASE;
            if reg_offset == blitter::reg::SRC_ADDR || reg_offset == blitter::reg::DST_ADDR {
                self.blitter.write_long(reg_offset, value);
                return;
            }
        }
        self.write16(addr, (value >> 16) as u16);
        self.write16(addr.wrapping_add(2), value as u16);
    }

    fn write8(&mut self, addr: u32, value: u8) {
        let addr = addr & ADDR_MASK;
        if addr < 16 && std::env::var("RUST68_TRACE_VECTORS").is_ok() {
            eprintln!("[trace] écriture vecteur bas : addr={addr:#x} value={value:#04x} overlay={}", self.overlay);
        }
        // Gardé par `!self.rom.is_empty()` : cette protection permanente
        // suppose une vraie ROM contenant les vecteurs reset à l'origine du
        // mirroring (voir la doc de la constante). Plusieurs tests
        // d'intégration construisent un `AtariSt::new(_, vec![])` (ROM vide)
        // comme banc d'essai CPU/bus nu, et écrivent directement en RAM basse
        // — que ce soit pour poser leur propre vecteur de reset
        // (`cpu_prend_une_interruption_mfp_bout_en_bout`) ou comme contenu
        // vidéo ordinaire à l'adresse 0, base vidéo par défaut
        // (`tick_rend_une_ligne_video_dans_le_framebuffer`) : sans vraie ROM,
        // ce mirroring matériel n'a pas de sens et ne doit pas s'appliquer.
        if !self.rom.is_empty() && addr < RESET_VECTOR_ROM_SIZE {
            self.bus_fault = Some((addr, true));
            return;
        }
        if let Some(phys) = self.translate_ram_addr(addr) {
            self.ram[phys] = value;
            return;
        }
        if Self::in_floating_st_ram(addr) {
            // Au-delà de la RAM installée mais dans l'espace "RAM ST" (4 Mo) :
            // écriture "flottante", jamais persistée — voir la doc
            // equivalente dans `read8`.
            return;
        }
        if let Some(off) = Self::mfp_offset(addr) {
            self.mfp.write(off, value);
            return;
        }
        match addr {
            MEMORY_CONF => {
                self.memory_conf = self.memory_conf_pin.unwrap_or(value);
                if std::env::var("RUST68_TRACE_VECTORS").is_ok() {
                    eprintln!(
                        "[trace] MEMORY_CONF écrit : overlay désactivé (value={value:#04x}, mémorisée={:#04x})",
                        self.memory_conf
                    );
                }
                self.overlay = false;
                return;
            }
            ACIA_KEYBOARD_CONTROL => {
                self.acia_keyboard.write(acia::reg::CONTROL_STATUS, value);
                return;
            }
            ACIA_KEYBOARD_DATA => {
                self.acia_keyboard.write(acia::reg::DATA, value);
                return;
            }
            ACIA_MIDI_CONTROL => {
                self.acia_midi.write(acia::reg::CONTROL_STATUS, value);
                return;
            }
            ACIA_MIDI_DATA => {
                self.acia_midi.write(acia::reg::DATA, value);
                return;
            }
            YM2149_SELECT => {
                self.ym2149.write(ym2149::bus_offset::SELECT, value);
                return;
            }
            YM2149_DATA => {
                self.ym2149.write(ym2149::bus_offset::DATA, value);
                return;
            }
            _ if Self::is_shifter_addr(addr) => {
                self.shifter.write(addr, value);
                return;
            }
            FDC_DATA => {
                if self.dma_sector_count_mode {
                    // Bit 4 de DMA_MODE posé : ce n'est PAS un registre du
                    // WD1772, voir la doc de `dma_sector_count`.
                    self.dma_sector_count = Some(value as u16);
                    return;
                }
                if self.dma_register_select == wd1772::reg::COMMAND_STATUS {
                    let (drive_a_selected, side) = self.floppy_drive_select();
                    self.wd1772.side = side;
                    let disk = if drive_a_selected { self.floppy_a.as_deref_mut() } else { None };
                    // Ne fait plus que DÉMARRER la commande (positionne
                    // BUSY) : c'est `AtariSt::tick` qui la fait progresser
                    // et la termine réellement, voir la doc de
                    // `Wd1772::execute_command`/`Wd1772::tick`.
                    self.wd1772.execute_command(value, disk);
                } else {
                    self.wd1772.write_simple_register(self.dma_register_select, value);
                }
                return;
            }
            DMA_MODE => {
                // Bits 1-2 (A1-A0), pas 0-1 — voir la doc de `DMA_MODE`.
                self.dma_register_select = (value & 0x6) >> 1;
                self.dma_sector_count_mode = value & 0x10 != 0;
                return;
            }
            DMA_ADDR_HIGH => {
                self.dma_address = (self.dma_address & 0x00FFFF) | ((value as u32) << 16);
                return;
            }
            DMA_ADDR_MID => {
                self.dma_address = (self.dma_address & 0xFF00FF) | ((value as u32) << 8);
                return;
            }
            DMA_ADDR_LOW => {
                self.dma_address = (self.dma_address & 0xFFFF00) | value as u32;
                return;
            }
            _ if self.blitter_present && addr == BLITTER_BASE + blitter::reg::CONTROL => {
                self.blitter.write(blitter::reg::CONTROL, value);
                // Bit BUSY/START (bit 7) posé : déclenche le blit dans son
                // intégralité (modèle synchrone, voir peripherals::atari_st::blitter).
                if value & 0x80 != 0 {
                    if std::env::var("RUST68_TRACE_BLITTER").is_ok() {
                        let word = |a, b| ((self.blitter.read(a) as u32) << 8) | self.blitter.read(b) as u32;
                        let long = |a: u32| {
                            ((self.blitter.read(a) as u32) << 24)
                                | ((self.blitter.read(a + 1) as u32) << 16)
                                | ((self.blitter.read(a + 2) as u32) << 8)
                                | self.blitter.read(a + 3) as u32
                        };
                        let halftone_table: Vec<String> = (0..16)
                            .map(|i| {
                                format!(
                                    "{:04x}",
                                    word(
                                        blitter::reg::HALFTONE_BASE + i * 2,
                                        blitter::reg::HALFTONE_BASE + i * 2 + 1
                                    )
                                )
                            })
                            .collect();
                        eprintln!(
                            "[trace] blit : pc={:#08x} src={:#08x} dst={:#08x} x={} y={} hop={} op={:#03x} skew={:#04x} control={:#04x} endmask1={:#06x} endmask2={:#06x} endmask3={:#06x} src_xinc={} src_yinc={} dst_xinc={} dst_yinc={} halftone=[{}]",
                            self.last_pc,
                            long(blitter::reg::SRC_ADDR),
                            long(blitter::reg::DST_ADDR),
                            word(blitter::reg::X_COUNT, blitter::reg::X_COUNT1),
                            word(blitter::reg::Y_COUNT, blitter::reg::Y_COUNT1),
                            self.blitter.read(blitter::reg::HOP),
                            self.blitter.read(blitter::reg::OP),
                            self.blitter.read(blitter::reg::SKEW),
                            value,
                            word(blitter::reg::ENDMASK_1, blitter::reg::ENDMASK_11),
                            word(blitter::reg::ENDMASK_2, blitter::reg::ENDMASK_21),
                            word(blitter::reg::ENDMASK_3, blitter::reg::ENDMASK_31),
                            word(blitter::reg::SRC_X_INC, blitter::reg::SRC_X_INC1) as i16,
                            word(blitter::reg::SRC_Y_INC, blitter::reg::SRC_Y_INC1) as i16,
                            word(blitter::reg::DST_X_INC, blitter::reg::DST_X_INC1) as i16,
                            word(blitter::reg::DST_Y_INC, blitter::reg::DST_Y_INC1) as i16,
                            halftone_table.join(","),
                        );
                    }
                    // `RUST68_TRACE_BLITTER_MEM=1` : dump le contenu réel de
                    // la destination (tous les mots de la première ligne,
                    // espacés de DST_X_INC) avant/après `execute()`, pour
                    // vérifier directement le résultat écrit par le Blitter
                    // plutôt que de le déduire à la main depuis les
                    // paramètres seuls.
                    let trace_mem = std::env::var("RUST68_TRACE_BLITTER_MEM").is_ok();
                    let mem_dst_addr = ((self.blitter.read(blitter::reg::DST_ADDR) as u32) << 24)
                        | ((self.blitter.read(blitter::reg::DST_ADDR1) as u32) << 16)
                        | ((self.blitter.read(blitter::reg::DST_ADDR2) as u32) << 8)
                        | self.blitter.read(blitter::reg::DST_ADDR3) as u32;
                    let mem_x_count = ((self.blitter.read(blitter::reg::X_COUNT) as u32) << 8)
                        | self.blitter.read(blitter::reg::X_COUNT1) as u32;
                    let mem_dst_xinc = (((self.blitter.read(blitter::reg::DST_X_INC) as u16) << 8)
                        | self.blitter.read(blitter::reg::DST_X_INC1) as u16) as i16;
                    if trace_mem {
                        let before: Vec<String> = (0..mem_x_count.max(1).min(20))
                            .map(|i| {
                                let a = mem_dst_addr.wrapping_add((i as i32 * mem_dst_xinc as i32) as u32);
                                format!("{:04x}", self.read16(a))
                            })
                            .collect();
                        eprintln!("[blitmem] AVANT dst={mem_dst_addr:#08x} : [{}]", before.join(","));
                    }
                    let mut ram_bus = RamBus {
                        ram: &mut self.ram,
                        rom: &self.rom,
                        rom_base: self.rom_base,
                    };
                    self.blitter.execute(&mut ram_bus);
                    if trace_mem {
                        let after: Vec<String> = (0..mem_x_count.max(1).min(20))
                            .map(|i| {
                                let a = mem_dst_addr.wrapping_add((i as i32 * mem_dst_xinc as i32) as u32);
                                format!("{:04x}", self.read16(a))
                            })
                            .collect();
                        eprintln!("[blitmem] APRES dst={mem_dst_addr:#08x} : [{}]", after.join(","));
                    }
                }
                return;
            }
            _ if self.is_blitter_addr(addr) => {
                self.blitter.write(addr - BLITTER_BASE, value);
                return;
            }
            STE_MICROWIRE_MASK => {
                self.microwire.write_mask_high(value);
                self.ste_dma_sound[(addr - STE_DMA_SOUND_BASE) as usize] = value;
                return;
            }
            STE_MICROWIRE_MASK1 => {
                self.microwire.write_mask_low(value);
                self.ste_dma_sound[(addr - STE_DMA_SOUND_BASE) as usize] = value;
                return;
            }
            STE_MICROWIRE_DATA => {
                self.microwire.write_data_high(value);
                return;
            }
            STE_MICROWIRE_DATA1 => {
                self.microwire.write_data_low(value);
                return;
            }
            _ if (STE_DMA_SOUND_BASE..=STE_DMA_SOUND_END).contains(&addr) => {
                let off = addr - STE_DMA_SOUND_BASE;
                if Self::is_dma_sound_reg(off) {
                    self.dma_sound.write(off, value);
                } else {
                    self.ste_dma_sound[off as usize] = value;
                }
                return;
            }
            _ => {}
        }
        if self.in_rom(addr) {
            return; // ROM : écriture ignorée (lecture seule sur silicium réel)
        }
        if (IO_BASE..=IO_END).contains(&addr) || (CARTRIDGE_BASE..=CARTRIDGE_END).contains(&addr) {
            return; // périphérique/cartouche non émulé : écriture ignorée
        }
        if std::env::var("RUST68_TRACE_VECTORS").is_ok() {
            eprintln!("[trace] bus fault en écriture : addr={addr:#x} value={value:#04x}");
        }
        self.bus_fault = Some((addr, true));
    }

    fn reset_bus(&mut self) {
        // L'instruction RESET génère /RESET vers les périphériques externes.
        // Le GLUE n'est PAS réinitialisé : sur silicium réel, le timing
        // vidéo continue de tourner indépendamment d'un /RESET CPU (le
        // moniteur reste synchronisé).
        self.mfp = Mfp::new();
        self.mfp.set_gpip_input(GPIP_MONO_DETECT, true);
        self.mfp.set_gpip_input(4, true);
        self.mfp.set_gpip_input(5, true);
        self.acia_keyboard = Acia::new();
        self.ikbd = Ikbd::new();
        self.acia_midi = Acia::new();
        self.ym2149 = Ym2149::new();
        self.dma_sound = DmaSound::new();
        // `self.microwire` volontairement PAS réinitialisé ici : le vrai
        // circuit Microwire/LMC1992 n'a pas de signal de reset câblé,
        // confirmé par Hatari (`dmaSnd.c` : « Microwire has no reset
        // signal, it will keep its values on warm reset »).
        // `Shifter::reset` (pas `Shifter::new()`) : préserve `ste_palette`,
        // une caractéristique du silicium (voir sa doc) que RESET ne doit
        // pas effacer.
        self.shifter.reset();
        self.wd1772 = Wd1772::new();
        self.dma_register_select = 0;
        self.dma_address = 0;
        self.dma_sector_count_mode = false;
        self.dma_sector_count = None;
        self.blitter = Blitter::new();
        self.overlay = true;
        self.memory_conf = 0;
        // Le disque inséré (floppy_a), lui, n'est pas éjecté par /RESET :
        // c'est un support physique, pas un état de la puce.
        // Le GLUE n'est pas réinitialisé (voir ci-dessus) : resynchroniser
        // juste le suivi de ligne/trame sur sa position courante pour ne
        // pas déclencher un rattrapage massif au prochain tick().
        self.last_vbl_edge = self.glue.vbl_edge_count();
        self.last_absolute_line =
            self.glue.frame_count() * self.glue.lines_per_frame() as u64 + self.glue.current_line() as u64;
    }

    fn take_bus_fault(&mut self) -> Option<(u32, bool)> {
        self.bus_fault.take()
    }

    fn has_pending_bus_fault(&self) -> bool {
        self.bus_fault.is_some()
    }

    fn irq_level(&self) -> u8 {
        // Câblage matériel ST/STE, par priorité décroissante :
        // MFP (IPL6) > VBL (IPL4) > HBL (IPL2).
        if self.mfp.interrupt_requested() {
            6
        } else if self.glue.vbl_pending() {
            4
        } else if self.glue.hbl_pending() {
            2
        } else {
            0
        }
    }

    fn irq_ack(&mut self, level: u8) -> u8 {
        if self.trace_irq {
            eprintln!("[irq] niveau={level} pc={:#08x}", self.last_pc);
        }
        match level {
            6 => self.mfp.iack(),
            4 => {
                self.glue.ack_vbl();
                24 + 4 // autovecteur niveau 4
            }
            2 => {
                self.glue.ack_hbl();
                24 + 2 // autovecteur niveau 2
            }
            _ => 24 + level,
        }
    }
}
