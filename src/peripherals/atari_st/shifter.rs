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
//!   `Bus::is_contended`/`TimedBus`, pas ici).
//!
//! ## Limitations connues (v1)
//! - Les registres de compteur vidéo (`$FF8205`/`07`/`09`) acceptent
//!   toujours l'écriture logicielle (comportement STE) ; sur ST d'origine,
//!   ils sont normalement lecture seule.
//! - Pas de défilement fin horizontal/vertical (registres STE dédiés), pas
//!   de synchronisation 50/60 Hz distincte de celle déjà portée par le
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
        }
    }

    /// Active/désactive le format de palette étendu STE (12 bits) — à
    /// appeler juste après construction d'après [`crate::systems::atari_st::model::MachineProfile::ste_palette`].
    pub fn set_ste_palette(&mut self, on: bool) {
        self.ste_palette = on;
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
    /// `ram` doit couvrir au moins `video_counter + bytes_per_line()` —
    /// renvoie une ligne de zéros (noir) si ce n'est pas le cas, plutôt que
    /// de paniquer (fin de RAM installée, ou base vidéo pointée au-delà par
    /// un logiciel de test). Le compteur, lui, avance TOUJOURS (avant même
    /// de savoir si la lecture réussira) : sur silicium réel, le compteur
    /// d'adresse du Shifter est un simple générateur d'adresses, indépendant
    /// de la présence physique de RAM à cette adresse précise — seul le
    /// contenu lu en dépend, pas l'avancement du compteur. Ne PAS avancer ici
    /// bloquait le compteur indéfiniment (donc toute lecture logicielle de
    /// `VIDEO_COUNTER_*`) dès que la base vidéo dépassait la RAM installée —
    /// confirmé nécessaire par la cartouche de diagnostic usine STe (test
    /// "T4 Video Counter in Memory Controller", qui déplace délibérément la
    /// base vidéo jusqu'à la toute fin de la RAM installée pour vérifier que
    /// le compteur continue de progresser correctement).
    pub fn render_scanline(&mut self, ram: &[u8]) -> Vec<(u8, u8, u8)> {
        let bytes_per_line = self.resolution.bytes_per_line();
        let start = self.video_counter as usize;
        self.video_counter += bytes_per_line as u32;
        let Some(line) = ram.get(start..start + bytes_per_line) else {
            return vec![(0, 0, 0); self.resolution.width()];
        };

        match self.resolution {
            Resolution::High => self.render_mono(line),
            _ => self.render_planar(line),
        }
    }

    fn render_planar(&self, line: &[u8]) -> Vec<(u8, u8, u8)> {
        let planes = self.resolution.planes();
        let bytes_per_group = planes * 2;
        let mut pixels = Vec::with_capacity(self.resolution.width());
        for group in line.chunks_exact(bytes_per_group) {
            let words: Vec<u16> = group
                .chunks_exact(2)
                .map(|b| u16::from_be_bytes([b[0], b[1]]))
                .collect();
            for bit in (0..16).rev() {
                let mut color_index = 0u8;
                for (plane, &word) in words.iter().enumerate() {
                    color_index |= (((word >> bit) & 1) as u8) << plane;
                }
                pixels.push(self.color_to_rgb(self.palette[color_index as usize]));
            }
        }
        pixels
    }

    fn render_mono(&self, line: &[u8]) -> Vec<(u8, u8, u8)> {
        let mut pixels = Vec::with_capacity(self.resolution.width());
        for &byte in line {
            for bit in (0..8).rev() {
                let set = (byte >> bit) & 1 != 0;
                // Convention : bit à 1 = noir, à 0 = blanc (cf. limitations).
                pixels.push(if set { (0, 0, 0) } else { (255, 255, 255) });
            }
        }
        pixels
    }

    /// Convertit un mot de palette brut en RGB 24 bits, par réplication de
    /// bits (0..2^n-1 -> 0..255 exactement, pas juste une mise à l'échelle
    /// approximative) — 3 bits/composante en ST (`$RGB` sur 9 bits), 4
    /// bits/composante en STE (`$RGB` sur 12 bits, nibbles contigus, voir
    /// [`Self::ste_palette`]).
    fn color_to_rgb(&self, word: u16) -> (u8, u8, u8) {
        if self.ste_palette {
            let r4 = ((word >> 8) & 0x0F) as u8;
            let g4 = ((word >> 4) & 0x0F) as u8;
            let b4 = (word & 0x0F) as u8;
            (expand4(r4), expand4(g4), expand4(b4))
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
