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
//! ## Limitations connues (v1)
//! - Type III (Read Address/Read Track/Write Track-Format) non implémenté :
//!   commande ignorée, positionne juste `LOST_DATA`/`RECORD_NOT_FOUND`
//!   selon le cas pour signaler l'échec plutôt que de planter.
//! - Pas de vérification (bit V) ni de CRC réel : une commande Seek avec
//!   V=1 réussit toujours (pas de lecture d'ID de piste simulée), l'erreur
//!   CRC ne se déclenche jamais.
//! - Pas de temporisation réelle (vitesse de pas, délai de 15 ms, latence
//!   de rotation) : les commandes s'exécutent intégralement de façon
//!   synchrone dès l'écriture du registre de commande — `BUSY` n'est donc
//!   jamais observable à `true` par un polling logiciel (simplification du
//!   même type que le modèle "au byte" du MFP/ACIA, ici "au secteur").
//! - Le signal `TR00` (capteur physique piste 0) n'est pas modélisé
//!   séparément : le registre Piste fait foi.

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
    /// accès au disque et au canal DMA que cette méthode n'a pas).
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

    /// Exécute la commande `command` (valeur écrite dans `COMMAND_STATUS`)
    /// de façon synchrone : toutes les commandes se terminent avant que
    /// cette fonction ne rende la main (voir limitations du module). `disk`
    /// est `None` pour simuler un lecteur sans disque (`NOT_READY`).
    pub fn execute_command<D: FloppyDisk>(
        &mut self,
        command: u8,
        disk: Option<&mut D>,
        dma: &mut impl DmaChannel,
    ) {
        self.intrq = false;
        if command & 0xF0 == 0xD0 {
            // Type IV : Force Interrupt — termine toute commande en cours
            // (déjà fait, notre modèle est synchrone) et génère /INTRQ si
            // une des conditions I0-I3 est sélectionnée.
            self.status &= !status::BUSY;
            if command & 0x0F != 0 {
                self.intrq = true;
            }
            return;
        }

        let Some(disk) = disk else {
            self.status = status::NOT_READY;
            self.intrq = true;
            return;
        };

        if command & 0x80 == 0 {
            self.execute_type1(command, disk);
        } else if command & 0xC0 == 0x80 {
            self.execute_type2(command, disk, dma);
        } else {
            // Type III (Read Address/Read Track/Write Track) : non
            // implémenté (cf. limitations) — signale l'échec plutôt que
            // d'exécuter silencieusement autre chose.
            self.status = status::BUSY;
            self.status = status::SEEK_ERROR_OR_RECORD_NOT_FOUND | self.write_protect_bit(disk);
            self.intrq = true;
        }
    }

    fn write_protect_bit<D: FloppyDisk>(&self, disk: &D) -> u8 {
        if disk.write_protected() {
            status::WRITE_PROTECT
        } else {
            0
        }
    }

    fn execute_type1<D: FloppyDisk>(&mut self, command: u8, disk: &D) {
        self.status = status::BUSY;
        match command >> 4 {
            0b0000 => {
                // Restore : ramène la tête en piste 0.
                self.track = 0;
            }
            0b0001 => {
                // Seek : la cible est dans le registre Donnée.
                self.last_step_in = self.data > self.track;
                self.track = self.data;
            }
            0b0010 | 0b0011 => {
                // Step : rejoue la dernière direction utilisée.
                self.step(self.last_step_in, command & 0x10 != 0);
            }
            0b0100 | 0b0101 => {
                self.last_step_in = true;
                self.step(true, command & 0x10 != 0);
            }
            0b0110 | 0b0111 => {
                self.last_step_in = false;
                self.step(false, command & 0x10 != 0);
            }
            _ => {}
        }
        let _ = disk; // réservé pour une future vérification (bit V) réelle
        self.status = 0;
        if self.track == 0 {
            self.status |= status::TRACK00_OR_LOST_DATA;
        }
        self.status |= self.write_protect_bit(disk);
        self.intrq = true;
    }

    fn step(&mut self, inward: bool, update_track: bool) {
        let next = if inward {
            self.track.saturating_add(1)
        } else {
            self.track.saturating_sub(1)
        };
        if update_track {
            self.track = next;
        }
    }

    fn execute_type2<D: FloppyDisk>(&mut self, command: u8, disk: &mut D, dma: &mut impl DmaChannel) {
        let is_write = command & 0x20 != 0;
        let multiple = command & 0x10 != 0;
        self.status = status::BUSY;

        if is_write && disk.write_protected() {
            self.status = status::WRITE_PROTECT;
            self.intrq = true;
            return;
        }

        loop {
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
                    }
                    None => {
                        self.status = status::SEEK_ERROR_OR_RECORD_NOT_FOUND;
                        self.intrq = true;
                        return;
                    }
                }
            }
            if !multiple || self.sector >= disk.sectors_per_track() {
                break;
            }
            self.sector += 1;
        }
        self.status = self.write_protect_bit(disk);
        self.intrq = true;
    }
}
