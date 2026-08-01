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
use crate::peripherals::atari_st::glue::{Glue, VideoMode};
use crate::peripherals::atari_st::ikbd::Ikbd;
use crate::peripherals::atari_st::mfp::Mfp;
use crate::peripherals::atari_st::shifter::{self, Shifter};
use crate::peripherals::atari_st::wd1772::{self, DmaChannel, FloppyDisk, Wd1772};
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
/// Contrôleur DMA : sélecteur de registre FDC (bits 0-1, voir
/// limitations — modèle simplifié).
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

/// Adresse de base usuelle de la ROM TOS (192 Ko, TOS 1.x/2.x).
pub const DEFAULT_ROM_BASE: u32 = 0xFC0000;

/// Registre de configuration mémoire (MMU), sur ST/STE réel. Écrire ici
/// désactive l'overlay ROM à l'adresse 0 (voir [`AtariSt::overlay`]) — le
/// TOS le fait très tôt au boot, juste après avoir sondé le cookie de
/// warmstart, pour reprendre normalement le contrôle des adresses basses
/// une fois son propre code d'amorçage terminé.
///
/// Bits 1-0 = taille annoncée de la banque 0, bits 3-2 = banque 1
/// (`00`=128 Ko, `01`=512 Ko, `10`=2 Mo, `11`=réservé). Le TOS l'écrit
/// lui-même pendant la détection de RAM au boot froid, mais ce champ ne
/// pilote **pas** l'accès mémoire (voir [`ST_RAM_ADDRESS_SPACE`]) : sur
/// silicium réel il ajuste surtout le timing de rafraîchissement DRAM
/// (différent selon la densité des puces), pas le décodage d'adresse
/// lui-même. On le stocke simplement pour une relecture logicielle
/// cohérente.
pub const MEMORY_CONF: u32 = 0xFF8001;

/// Taille de l'espace d'adressage "RAM ST" sur ST/STE réel (4 Mo, deux
/// banques MMU de 2 Mo chacune) : le MMU y répond toujours /DTACK, même
/// sans RAM physique à l'adresse précise accédée — jamais de bus error
/// dans cette plage (voir [`AtariSt::in_floating_st_ram`]), contrairement
/// au vrai "trou" au-delà, avant `IO_BASE`. Confirmé par la communauté
/// Atari (ex : accéder à de la RAM ST non peuplée renvoie des données
/// résiduelles du dernier cycle de bus, pas un bus error).
const ST_RAM_ADDRESS_SPACE: u32 = 4 * 1024 * 1024;

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
    last_frame: u64,
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
    /// MMU) : purement pour une relecture logicielle cohérente, ne pilote
    /// pas l'accès mémoire (voir sa doc et [`ST_RAM_ADDRESS_SPACE`]).
    memory_conf: u8,
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

/// Canal DMA reliant le WD1772 à la RAM du board à l'adresse DMA courante
/// (voir `peripherals::atari_st::wd1772::DmaChannel`) : le WD1772 ne connaît pas la
/// RAM, seulement ce canal.
struct RamDmaChannel<'a> {
    ram: &'a mut [u8],
    address: &'a mut u32,
}

/// Vue `Bus` d'une tranche de RAM, pour donner au Blitter (qui prend un
/// `Bus` générique) accès à la RAM du board sans emprunt réflexif de
/// `AtariSt` tout entier.
struct RamBus<'a> {
    ram: &'a mut [u8],
}

impl<'a> Bus for RamBus<'a> {
    fn read8(&mut self, addr: u32) -> u8 {
        self.ram.get(addr as usize).copied().unwrap_or(0xFF)
    }

    fn write8(&mut self, addr: u32, value: u8) {
        if let Some(slot) = self.ram.get_mut(addr as usize) {
            *slot = value;
        }
    }
}

impl<'a> DmaChannel for RamDmaChannel<'a> {
    fn pull(&mut self) -> u8 {
        let byte = self.ram.get(*self.address as usize).copied().unwrap_or(0);
        *self.address = self.address.wrapping_add(1);
        byte
    }

    fn push(&mut self, byte: u8) {
        if let Some(slot) = self.ram.get_mut(*self.address as usize) {
            *slot = byte;
        }
        *self.address = self.address.wrapping_add(1);
    }
}

impl AtariSt {
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
            mfp,
            glue: Glue::new(VideoMode::Pal50),
            acia_keyboard: Acia::new(),
            ikbd: Ikbd::new(),
            acia_midi: Acia::new(),
            ym2149: Ym2149::new(),
            shifter: Shifter::new(),
            framebuffer: Vec::new(),
            last_absolute_line: 0,
            last_frame: 0,
            wd1772: Wd1772::new(),
            floppy_a: None,
            dma_register_select: 0,
            dma_address: 0,
            blitter: Blitter::new(),
            overlay: true,
            memory_conf: 0,
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
        let acia_irq = self.acia_keyboard.irq_requested() || self.acia_midi.irq_requested();
        self.mfp.set_gpip_input(4, !acia_irq);
        self.mfp.set_gpip_input(5, !self.wd1772.interrupt_requested());

        let frame_now = self.glue.frame_count();
        if frame_now != self.last_frame {
            self.last_frame = frame_now;
            self.shifter.start_frame();
        }
        let lines_per_frame = self.glue.lines_per_frame() as u64;
        // Compteur absolu (jamais remis à zéro) pour ne pas confondre "une
        // trame entière vient de s'écouler" avec "aucune ligne écoulée"
        // quand current_line() revient à 0 en bouclant.
        let absolute_line_now = frame_now * lines_per_frame + self.glue.current_line() as u64;
        // Borne défensive : ne rattrape jamais plus d'une trame complète en
        // un seul tick (cas normal : 0 ou 1 ligne, tick() étant appelé après
        // chaque instruction, bien plus fréquemment qu'une ligne = 512 cycles).
        let mut guard = 0u64;
        while self.last_absolute_line < absolute_line_now && guard < lines_per_frame {
            self.last_absolute_line += 1;
            let row = self.shifter.render_scanline(&self.ram);
            let idx = (self.last_absolute_line % lines_per_frame) as usize;
            if idx >= self.framebuffer.len() {
                self.framebuffer.resize(idx + 1, Vec::new());
            }
            self.framebuffer[idx] = row;
            // Câblage matériel réel ST/STE : l'entrée externe TBI du Timer B
            // du MFP est reliée au signal de balayage actif (DE), pas au
            // HBL brut — elle ne pulse donc que pendant les lignes visibles
            // (200 lignes, quel que soit PAL/NTSC), pas pendant le
            // blanking vertical. C'est exactement ce que le boot TOS
            // exploite pour détecter qu'il vient d'entrer en VBL : il
            // programme le Timer B en mode event-count puis attend que la
            // valeur cesse de changer (~615 lectures stables), ce qui
            // n'arrive jamais tant qu'on reste dans la zone visible.
            if idx < VISIBLE_LINES {
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
    /// physique à cette adresse précise (le MMU répond /DTACK dans toute
    /// cette plage, contrairement au vrai "trou" au-delà, avant `IO_BASE`).
    fn in_floating_st_ram(addr: u32) -> bool {
        addr < ST_RAM_ADDRESS_SPACE
    }

    fn is_shifter_addr(addr: u32) -> bool {
        matches!(
            addr,
            shifter::addr::VIDEO_BASE_HIGH
                | shifter::addr::VIDEO_BASE_MID
                | shifter::addr::VIDEO_COUNTER_HIGH
                | shifter::addr::VIDEO_COUNTER_MID
                | shifter::addr::VIDEO_COUNTER_LOW
                | shifter::addr::RESOLUTION
        ) || (shifter::addr::PALETTE_BASE..shifter::addr::PALETTE_BASE + 32).contains(&addr)
    }

    fn is_blitter_addr(&self, addr: u32) -> bool {
        self.blitter_present && (BLITTER_BASE..BLITTER_BASE + blitter::reg::END).contains(&addr)
    }
}

impl Bus for AtariSt {
    fn read8(&mut self, addr: u32) -> u8 {
        let addr = addr & ADDR_MASK;
        if self.overlay && addr < OVERLAY_SIZE && (addr as usize) < self.rom.len() {
            return self.rom[addr as usize];
        }
        if (addr as usize) < self.ram.len() {
            return self.ram[addr as usize];
        }
        if Self::in_floating_st_ram(addr) {
            // Au-delà de la RAM installée mais dans l'espace "RAM ST" (4 Mo) :
            // jamais de bus error sur silicium réel (voir la doc du module),
            // valeur fixe non stockée (jamais ce qui vient d'être écrit).
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
            _ if (STE_DMA_SOUND_BASE..=STE_DMA_SOUND_END).contains(&addr) => return 0x00,
            _ => {}
        }
        if self.in_rom(addr) {
            return self.rom[(addr - self.rom_base) as usize];
        }
        if (IO_BASE..=IO_END).contains(&addr) || (CARTRIDGE_BASE..=CARTRIDGE_END).contains(&addr) {
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
        self.write8(addr, (value >> 8) as u8);
        self.write8(addr.wrapping_add(1), value as u8);
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
        if (addr as usize) < self.ram.len() {
            self.ram[addr as usize] = value;
            return;
        }
        if Self::in_floating_st_ram(addr) {
            // Au-delà de la RAM installée mais dans l'espace "RAM ST" (4 Mo) :
            // écriture "flottante", jamais persistée (voir la doc du module).
            return;
        }
        if let Some(off) = Self::mfp_offset(addr) {
            self.mfp.write(off, value);
            return;
        }
        match addr {
            MEMORY_CONF => {
                self.memory_conf = value;
                if std::env::var("RUST68_TRACE_VECTORS").is_ok() {
                    eprintln!("[trace] MEMORY_CONF écrit : overlay désactivé (value={value:#04x})");
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
                if self.dma_register_select == wd1772::reg::COMMAND_STATUS {
                    let mut channel = RamDmaChannel {
                        ram: &mut self.ram,
                        address: &mut self.dma_address,
                    };
                    self.wd1772
                        .execute_command(value, self.floppy_a.as_deref_mut(), &mut channel);
                } else {
                    self.wd1772.write_simple_register(self.dma_register_select, value);
                }
                return;
            }
            DMA_MODE => {
                self.dma_register_select = value & 0x03;
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
                            "[trace] blit : src={:#08x} dst={:#08x} x={} y={} hop={} op={:#03x} skew={:#04x} control={:#04x} endmask1={:#06x} endmask2={:#06x} endmask3={:#06x} src_xinc={} src_yinc={} dst_xinc={} dst_yinc={} halftone=[{}]",
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
                    let mut ram_bus = RamBus { ram: &mut self.ram };
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
        // `Shifter::reset` (pas `Shifter::new()`) : préserve `ste_palette`,
        // une caractéristique du silicium (voir sa doc) que RESET ne doit
        // pas effacer.
        self.shifter.reset();
        self.wd1772 = Wd1772::new();
        self.dma_register_select = 0;
        self.dma_address = 0;
        self.blitter = Blitter::new();
        self.overlay = true;
        self.memory_conf = 0;
        // Le disque inséré (floppy_a), lui, n'est pas éjecté par /RESET :
        // c'est un support physique, pas un état de la puce.
        // Le GLUE n'est pas réinitialisé (voir ci-dessus) : resynchroniser
        // juste le suivi de ligne/trame sur sa position courante pour ne
        // pas déclencher un rattrapage massif au prochain tick().
        self.last_frame = self.glue.frame_count();
        self.last_absolute_line =
            self.last_frame * self.glue.lines_per_frame() as u64 + self.glue.current_line() as u64;
    }

    fn take_bus_fault(&mut self) -> Option<(u32, bool)> {
        self.bus_fault.take()
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
