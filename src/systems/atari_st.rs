//! Board Atari ST : mapping mémoire réel + câblage MFP/GLUE → IPL.
//!
//! Implémente [`crate::Bus`] pour un ST/STE minimal : RAM installée à
//! `0x000000`, ROM TOS à `0xFC0000`, MFP 68901 aux adresses impaires
//! `0xFFFA01`-`0xFFFA2F`. Le "trou" physique entre le haut de la RAM
//! installée et le début de la zone d'E/S (`0xFF8000`) déclenche un bus
//! error via [`crate::Bus::take_bus_fault`] — c'est le mécanisme que de
//! nombreux programmes/démos utilisent pour détecter la RAM installée.
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

use crate::peripherals::atari_st::acia::{self, Acia};
use crate::peripherals::atari_st::blitter::{self, Blitter};
use crate::peripherals::atari_st::glue::{Glue, VideoMode};
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
pub const MEMORY_CONF: u32 = 0xFF8001;

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
            bus_fault: None,
        }
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

    fn is_blitter_addr(addr: u32) -> bool {
        (BLITTER_BASE..BLITTER_BASE + blitter::reg::END).contains(&addr)
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
        if let Some(off) = Self::mfp_offset(addr) {
            return self.mfp.read(off);
        }
        match addr {
            ACIA_KEYBOARD_CONTROL => return self.acia_keyboard.read(acia::reg::CONTROL_STATUS),
            ACIA_KEYBOARD_DATA => return self.acia_keyboard.read(acia::reg::DATA),
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
            _ if Self::is_blitter_addr(addr) => return self.blitter.read(addr - BLITTER_BASE),
            _ if (STE_DMA_SOUND_BASE..=STE_DMA_SOUND_END).contains(&addr) => return 0x00,
            _ => {}
        }
        if self.in_rom(addr) {
            return self.rom[(addr - self.rom_base) as usize];
        }
        if (IO_BASE..=IO_END).contains(&addr) || (CARTRIDGE_BASE..=CARTRIDGE_END).contains(&addr) {
            return 0xFF;
        }
        self.bus_fault = Some((addr, false));
        0xFF
    }

    fn write8(&mut self, addr: u32, value: u8) {
        let addr = addr & ADDR_MASK;
        if (addr as usize) < self.ram.len() {
            self.ram[addr as usize] = value;
            return;
        }
        if let Some(off) = Self::mfp_offset(addr) {
            self.mfp.write(off, value);
            return;
        }
        match addr {
            MEMORY_CONF => {
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
            _ if addr == BLITTER_BASE + blitter::reg::CONTROL => {
                self.blitter.write(blitter::reg::CONTROL, value);
                // Bit BUSY/START (bit 7) posé : déclenche le blit dans son
                // intégralité (modèle synchrone, voir peripherals::atari_st::blitter).
                if value & 0x80 != 0 {
                    let mut ram_bus = RamBus { ram: &mut self.ram };
                    self.blitter.execute(&mut ram_bus);
                }
                return;
            }
            _ if Self::is_blitter_addr(addr) => {
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
        self.acia_midi = Acia::new();
        self.ym2149 = Ym2149::new();
        self.shifter = Shifter::new();
        self.wd1772 = Wd1772::new();
        self.dma_register_select = 0;
        self.dma_address = 0;
        self.blitter = Blitter::new();
        self.overlay = true;
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
