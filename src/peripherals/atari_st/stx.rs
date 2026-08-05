//! Lecteur minimal de disquettes au format `.stx` (Pasti).
//!
//! Écrit initialement sans spécification officielle (disposition déduite
//! empiriquement), puis corrigé et recoupé après consultation de la
//! documentation publique du format (Pasti/STX, info-coach.fr et
//! atari.8bitchip.info) suite à un bug réel découvert sur une image
//! protégée réelle (voir la doc de `sector_data_ref` ci-dessous : le
//! secteur 1 piste 0 de `Rick_Dangerous.stx`, un jeu commercial connu,
//! ressortait entièrement à zéro). Les bits "fuzzy" eux-mêmes sont
//! maintenant simulés (voir [`StxImage::read_sector`], 2026-08-04 — trouvé
//! en retraçant un DoubleFault tardif de `Rick_Dangerous.stx` jusqu'à cette
//! zone jamais réécrite). `bit_position` (position réelle du champ ID sur
//! la piste physique) est exposé via [`FloppyDisk::sector_bit_position`]
//! depuis 2026-08-05 — trouvé en retraçant un facteur ~2x de lenteur face à
//! Hatari sur un gros transfert séquentiel : sans cette donnée, la latence
//! de recherche était estimée par un espacement uniforme entre secteurs,
//! faux sur une piste au formatage non standard (secteurs plus nombreux et
//! rapprochés qu'une piste normale, technique de protection courante).
//! `read_time` et l'image de piste brute complète restent ignorés — voir
//! les limitations plus bas.
//!
//! ## Disposition
//! - En-tête fichier (16 octets) : magique `"RSY\0"`, version u16 LE,
//!   outil u16 LE, réservé u16 LE, nombre de pistes u8, révision u8,
//!   réservé u32.
//! - Un enregistrement de piste (TDR, 16 octets) par piste, les
//!   enregistrements se suivant séquentiellement (chaque TDR commence à
//!   `position précédente + block_size précédent`) :
//!   `block_size: u32 LE`, `fuzzy_size: u32 LE`, `sector_count: u16 LE`,
//!   `flags: u16 LE`, `track_length: u16 LE`, `track_number: u8` (bits
//!   0-6 = numéro de piste, bit 7 = face), `track_type: u8`.
//! - `sector_count` enregistrements de secteur (SDR, 16 octets) suivent
//!   immédiatement le TDR : `data_offset: u32 LE`, `bit_position: u16 LE`,
//!   `read_time: u16 LE`, `track: u8`, `head: u8`, `sector: u8` (le
//!   numéro logique, potentiellement non séquentiel — entrelacement du
//!   secteur), `size_code: u8` (taille = `128 << size_code`), `crc1: u8`,
//!   `crc2: u8`, `fdc_status: u8`, `reserved: u8`.
//! - Juste après la table de SDR, un bloc de `fuzzy_size` octets (masque de
//!   bits instables pour les secteurs à protection par "fuzzy bits") — peut
//!   être absent (`fuzzy_size == 0`). `data_offset` de chaque SDR est
//!   relatif à la fin de CE bloc (fin de la table de SDR si absent), PAS au
//!   début du TDR ni au début du fichier — voir [`StxImage::parse`]
//!   (`sector_data_ref`). Une éventuelle image de piste brute (capture
//!   WD1772 complète, présente quand le bit 6 de `flags` est posé) peut
//!   suivre le bloc fuzzy avant les données de secteur proprement dites,
//!   mais `data_offset` la traverse déjà tout seul : inutile de la localiser
//!   séparément pour extraire un secteur `size_code == 2`.
//!
//! ## Limitations (lecteur minimal)
//! - Seuls les secteurs `size_code == 2` (512 octets, taille standard
//!   GEMDOS/ST) sont extraits ; les autres tailles sont ignorées.
//! - Une charge utile de secteur qui déborderait de la fin du fichier
//!   (image tronquée) est silencieusement ignorée (le secteur n'est pas
//!   ajouté) plutôt que de faire échouer tout le parsing — seul un TDR ou
//!   un SDR incomplet est traité comme une erreur fatale ([`StxError::Truncated`]).
//! - Les pistes sans secteurs discrets (capture de flux brut pour
//!   protection avancée, `sector_count == 0`) ne fournissent aucune
//!   donnée : `read_sector` renverra `None`.
//! - `sectors_per_track()` renvoie le maximum observé sur l'ensemble du
//!   disque ; certaines pistes réelles en ont moins (piste de protection
//!   avec un nombre de secteurs différent de la norme) — une lecture
//!   multi-secteurs (bit M du WD1772) peut donc échouer prématurément sur
//!   ces pistes-là plutôt que de s'arrêter proprement en fin de piste.
//! - Lecture seule : `write_sector` est ignoré (pas de ré-écriture du
//!   fichier `.stx`, voir [`StxImage::write_protected`]).

use super::wd1772::{FloppyDisk, SECTOR_SIZE};

struct StxSector {
    sector: u8,
    data: [u8; SECTOR_SIZE],
    /// Position réelle (en bits, depuis l'impulsion d'index) du champ ID de
    /// ce secteur sur la piste physique d'origine — capturée telle quelle
    /// par l'outil ayant produit l'image `.stx`, voir
    /// [`FloppyDisk::sector_bit_position`].
    bit_position: u16,
    /// Masque de bits "fuzzy" (protection par bits physiquement instables) :
    /// un octet de masque par octet de `data`, bit à 1 = position stable
    /// (garder le bit réel de `data`), bit à 0 = position instable (chaque
    /// lecture doit renvoyer une valeur imprévisible sur ce bit-là, comme sur
    /// le média magnétique réel). `None` pour un secteur ordinaire (immense
    /// majorité). Voir [`StxImage::read_sector`] pour l'application.
    fuzzy_mask: Option<[u8; SECTOR_SIZE]>,
    /// Bit 3 (`STX_SECTOR_FLAG_CRC`) de `fdc_status` : CRC d'ID délibérément
    /// faux (technique de protection — souvent posé EN MÊME TEMPS que
    /// `fuzzy_mask` sur le même secteur, cf. `Rick_Dangerous.stx` pistes
    /// 0-4, secteurs 11-12). Voir [`FloppyDisk::sector_has_crc_error`].
    has_crc_error: bool,
}

struct StxTrack {
    track: u8,
    side: u8,
    sectors: Vec<StxSector>,
}

/// Erreur de parsing d'une image `.stx`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StxError {
    /// L'en-tête ne commence pas par la signature `"RSY\0"`.
    BadMagic,
    /// Le fichier est plus court que ce que ses propres champs annoncent.
    Truncated,
}

/// Image disque `.stx` chargée en mémoire, exposée comme [`FloppyDisk`].
pub struct StxImage {
    tracks: Vec<StxTrack>,
    num_tracks: u8,
    num_sides: u8,
    sectors_per_track: u8,
    /// État d'un petit xorshift64 pour simuler les bits "fuzzy" (voir
    /// [`Self::read_sector`]) — `Cell` car `FloppyDisk::read_sector` prend
    /// `&self`, pas `&mut self` (une vraie disquette ne se "modifie" pas en
    /// la lisant), mais chaque lecture doit tout de même faire progresser
    /// l'état pour varier d'un appel à l'autre.
    fuzzy_rng: std::cell::Cell<u64>,
}

impl StxImage {
    /// Parse une image `.stx` déjà chargée en mémoire.
    pub fn parse(data: &[u8]) -> Result<Self, StxError> {
        if data.len() < 16 || &data[0..4] != b"RSY\0" {
            return Err(StxError::BadMagic);
        }
        let track_count = data[10] as usize;

        let mut pos = 16usize;
        let mut tracks = Vec::with_capacity(track_count);
        let mut max_track = 0u8;
        let mut max_side = 0u8;
        let mut max_sectors = 0u8;

        for _ in 0..track_count {
            if pos + 16 > data.len() {
                return Err(StxError::Truncated);
            }
            let block_size = u32::from_le_bytes(data[pos..pos + 4].try_into().unwrap()) as usize;
            // Taille (en octets) du bloc de bits "fuzzy" (secteurs à
            // protection par bits instables) inséré juste après la table de
            // SDR — jamais lu auparavant, alors que `data_offset` (ci-
            // dessous) est relatif à la fin de CE bloc, pas au début du TDR.
            // Confirmé empiriquement sur une vraie image protégée
            // (Rick_Dangerous.stx, piste 0) : sans ce décalage, le secteur
            // extrait tombait en plein milieu du bloc fuzzy/de l'image de
            // piste brute, donnant des données fausses (zéros) plutôt que
            // le vrai contenu du secteur.
            let fuzzy_size = u32::from_le_bytes(data[pos + 4..pos + 8].try_into().unwrap()) as usize;
            let sector_count =
                u16::from_le_bytes(data[pos + 8..pos + 10].try_into().unwrap()) as usize;
            let track_number = data[pos + 14];
            let track = track_number & 0x7F;
            let side = (track_number >> 7) & 1;

            // Référence pour `data_offset` de chaque SDR : fin de la table
            // de SDR (TDR + 16 octets par secteur) + bloc fuzzy — PAS le
            // début du TDR. `data_offset` inclut lui-même le franchissement
            // de l'éventuelle image de piste brute (track image) qui peut
            // suivre le bloc fuzzy : pas besoin de la localiser séparément.
            let sector_data_ref = pos + 16 + sector_count * 16 + fuzzy_size;
            // Début du bloc fuzzy lui-même (avant qu'il ne soit sauté par
            // `sector_data_ref` ci-dessus) : chaque secteur marqué "fuzzy"
            // (bit 7 de `fdc_status`) y consomme ses `SECTOR_SIZE` octets de
            // masque SÉQUENTIELLEMENT, dans l'ordre de la table de SDR —
            // vérifié contre Hatari (`src/floppies/stx.c`,
            // `pStxTrack->pFuzzyData` avancé de `SectorSize` uniquement pour
            // les secteurs `STX_SECTOR_FLAG_FUZZY`).
            let fuzzy_block_start = pos + 16 + sector_count * 16;
            let mut fuzzy_cursor = fuzzy_block_start;

            let mut sectors = Vec::with_capacity(sector_count);
            let mut sdr_pos = pos + 16;
            for _ in 0..sector_count {
                if sdr_pos + 16 > data.len() {
                    return Err(StxError::Truncated);
                }
                let data_offset =
                    u32::from_le_bytes(data[sdr_pos..sdr_pos + 4].try_into().unwrap()) as usize;
                let bit_position = u16::from_le_bytes(data[sdr_pos + 4..sdr_pos + 6].try_into().unwrap());
                let sector = data[sdr_pos + 10];
                let size_code = data[sdr_pos + 11];
                let fdc_status = data[sdr_pos + 14];
                // Bit 7 = STX_SECTOR_FLAG_FUZZY (protection par bits
                // physiquement instables — voir la doc de `fuzzy_mask`).
                let is_fuzzy = fdc_status & 0x80 != 0;
                let has_crc_error = fdc_status & 0x08 != 0;
                let fuzzy_mask = if is_fuzzy && fuzzy_cursor + SECTOR_SIZE <= data.len() {
                    let mut mask = [0u8; SECTOR_SIZE];
                    mask.copy_from_slice(&data[fuzzy_cursor..fuzzy_cursor + SECTOR_SIZE]);
                    Some(mask)
                } else {
                    None
                };
                if is_fuzzy {
                    fuzzy_cursor += SECTOR_SIZE;
                }
                if size_code == 2 {
                    let abs = sector_data_ref + data_offset;
                    if abs + SECTOR_SIZE <= data.len() {
                        let mut buf = [0u8; SECTOR_SIZE];
                        buf.copy_from_slice(&data[abs..abs + SECTOR_SIZE]);
                        sectors.push(StxSector { sector, data: buf, bit_position, fuzzy_mask, has_crc_error });
                    }
                }
                sdr_pos += 16;
            }

            max_track = max_track.max(track);
            max_side = max_side.max(side);
            max_sectors = max_sectors.max(sectors.len() as u8);
            tracks.push(StxTrack {
                track,
                side,
                sectors,
            });

            if block_size == 0 {
                break;
            }
            pos += block_size;
        }

        Ok(StxImage {
            tracks,
            num_tracks: max_track + 1,
            num_sides: max_side + 1,
            sectors_per_track: max_sectors,
            // Graine fixe : le déterminisme n'a aucune importance ici (le
            // jeu ne peut pas prédire ces octets de toute façon), et une
            // graine fixe garde les runs reproductibles pour le débogage.
            fuzzy_rng: std::cell::Cell::new(0x9E3779B97F4A7C15),
        })
    }

    fn find_sector(&self, track: u8, side: u8, sector: u8) -> Option<&StxSector> {
        self.tracks
            .iter()
            .find(|t| t.track == track && t.side == side)
            .and_then(|t| t.sectors.iter().find(|s| s.sector == sector))
    }

    /// Un octet "aléatoire" de plus (xorshift64*, une seule instance par
    /// image partagée entre tous les secteurs fuzzy — pas besoin d'un état
    /// par secteur, seule la variation d'une lecture à l'autre compte).
    fn next_random_byte(&self) -> u8 {
        let mut x = self.fuzzy_rng.get();
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.fuzzy_rng.set(x);
        (x >> 24) as u8
    }
}

impl FloppyDisk for StxImage {
    fn num_tracks(&self) -> u8 {
        self.num_tracks
    }
    fn num_sides(&self) -> u8 {
        self.num_sides
    }
    fn sectors_per_track(&self) -> u8 {
        self.sectors_per_track
    }
    fn write_protected(&self) -> bool {
        true
    }

    /// Pour un secteur "fuzzy" (protection par bits physiquement instables,
    /// voir la doc de [`StxSector::fuzzy_mask`]), combine l'octet réel avec
    /// du bruit sur les positions instables — comme le vrai média (chaque
    /// lecture y donne un résultat différent) et comme Hatari
    /// (`src/floppies/stx.c` : `Byte = (Byte & FuzzyData[i]) |
    /// (Hatari_rand() & ~FuzzyData[i])`). Sans ceci, une vérification de
    /// protection qui compare deux lectures du même secteur et s'attend à
    /// ce qu'elles DIFFÈRENT (preuve qu'il s'agit d'un média physique et
    /// non d'une copie numérique parfaite) verrait toujours deux lectures
    /// identiques ici — trouvé en creusant pourquoi `Rick_Dangerous.stx`
    /// finissait par exécuter de la mémoire non initialisée comme du code,
    /// bien après le chargement (piste 0, qui a un bloc fuzzy, vérifié
    /// absente de toute variation avant ce correctif).
    fn read_sector(&self, track: u8, side: u8, sector: u8) -> Option<[u8; SECTOR_SIZE]> {
        let sec = self.find_sector(track, side, sector)?;
        let mut buf = sec.data;
        if let Some(mask) = &sec.fuzzy_mask {
            for i in 0..SECTOR_SIZE {
                let noise = self.next_random_byte();
                buf[i] = (buf[i] & mask[i]) | (noise & !mask[i]);
            }
        }
        Some(buf)
    }

    fn write_sector(&mut self, _track: u8, _side: u8, _sector: u8, _data: &[u8; SECTOR_SIZE]) {}

    /// Compte RÉEL de secteurs de la piste/face visée (celui de son propre
    /// TDR), pas le maximum global renvoyé par `sectors_per_track()` — une
    /// image `.stx` protégée n'a typiquement PAS le même nombre de secteurs
    /// sur toutes les pistes (voir la doc du module), donc utiliser le
    /// maximum global désaligne progressivement le calcul de latence de
    /// rotation d'une piste à l'autre.
    fn sectors_on_track(&self, track: u8, side: u8) -> u8 {
        self.tracks
            .iter()
            .find(|t| t.track == track && t.side == side)
            .map(|t| t.sectors.len() as u8)
            .unwrap_or(self.sectors_per_track)
    }

    fn sector_has_crc_error(&self, track: u8, side: u8, sector: u8) -> bool {
        self.find_sector(track, side, sector)
            .is_some_and(|s| s.has_crc_error)
    }

    fn sector_bit_position(&self, track: u8, side: u8, sector: u8) -> Option<u32> {
        self.find_sector(track, side, sector).map(|s| s.bit_position as u32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn le16(v: u16) -> [u8; 2] {
        v.to_le_bytes()
    }
    fn le32(v: u32) -> [u8; 4] {
        v.to_le_bytes()
    }

    /// Construit une image `.stx` synthétique à une piste, un secteur, pour
    /// tester le parsing sans dépendre d'un fichier réel.
    fn build_minimal_stx(sector_payload: &[u8; SECTOR_SIZE]) -> Vec<u8> {
        build_stx_with_fuzzy(sector_payload, 0)
    }

    /// Comme [`build_minimal_stx`], avec un bloc `fuzzy_size` (potentiellement
    /// non nul) inséré entre la table de SDR et la charge utile du secteur —
    /// voir [`StxImage::parse`] (`sector_data_ref`) : `data_offset` doit être
    /// relatif à la fin de CE bloc, pas au début du TDR.
    fn build_stx_with_fuzzy(sector_payload: &[u8; SECTOR_SIZE], fuzzy_size: u32) -> Vec<u8> {
        let mut file = Vec::new();
        // En-tête.
        file.extend_from_slice(b"RSY\0");
        file.extend_from_slice(&le16(3)); // version
        file.extend_from_slice(&le16(1)); // tool
        file.extend_from_slice(&le16(0)); // reserved
        file.push(1); // track_count
        file.push(2); // revision
        file.extend_from_slice(&[0u8; 4]); // reserved

        // `data_offset` relatif à la fin de la table de SDR + bloc fuzzy
        // (voir la doc du module) : ici, le secteur suit immédiatement ce
        // bloc, donc `data_offset = 0`.
        let sdr_data_offset: u32 = 0;
        let track_pos = file.len();
        let block_size = 16 + 16 + fuzzy_size + SECTOR_SIZE as u32;

        // TDR.
        file.extend_from_slice(&le32(block_size));
        file.extend_from_slice(&le32(fuzzy_size));
        file.extend_from_slice(&le16(1)); // sector_count
        file.extend_from_slice(&le16(0x61)); // flags
        file.extend_from_slice(&le16(6261)); // track_length
        file.push(0); // track_number (piste 0, face 0)
        file.push(0); // track_type

        // SDR.
        file.extend_from_slice(&le32(sdr_data_offset));
        file.extend_from_slice(&le16(0)); // bit_position
        file.extend_from_slice(&le16(0)); // read_time
        file.push(0); // track
        file.push(0); // head
        file.push(1); // sector (1-indexé)
        file.push(2); // size_code = 512 octets
        file.extend_from_slice(&[0u8; 4]); // crc1, crc2, fdc_status, reserved

        assert_eq!(file.len(), track_pos + 16 + 16);
        file.extend_from_slice(&vec![0xFFu8; fuzzy_size as usize]);
        file.extend_from_slice(sector_payload);
        file
    }

    /// Comme [`build_minimal_stx`], mais avec un `bit_position` choisi pour
    /// le secteur — teste que ce champ (jusqu'ici toujours à 0 dans les
    /// autres constructeurs de ce module) est bien extrait et exposé via
    /// [`FloppyDisk::sector_bit_position`].
    fn build_minimal_stx_with_bit_position(sector_payload: &[u8; SECTOR_SIZE], bit_position: u16) -> Vec<u8> {
        let mut file = Vec::new();
        file.extend_from_slice(b"RSY\0");
        file.extend_from_slice(&le16(3));
        file.extend_from_slice(&le16(1));
        file.extend_from_slice(&le16(0));
        file.push(1);
        file.push(2);
        file.extend_from_slice(&[0u8; 4]);

        let sdr_data_offset: u32 = 0;
        let track_pos = file.len();
        let block_size = 16 + 16 + SECTOR_SIZE as u32;

        file.extend_from_slice(&le32(block_size));
        file.extend_from_slice(&le32(0)); // fuzzy_size
        file.extend_from_slice(&le16(1)); // sector_count
        file.extend_from_slice(&le16(0x61)); // flags
        file.extend_from_slice(&le16(6261)); // track_length
        file.push(0); // track_number
        file.push(0); // track_type

        file.extend_from_slice(&le32(sdr_data_offset));
        file.extend_from_slice(&le16(bit_position));
        file.extend_from_slice(&le16(0)); // read_time
        file.push(0); // track
        file.push(0); // head
        file.push(1); // sector
        file.push(2); // size_code = 512 octets
        file.extend_from_slice(&[0u8; 4]);

        assert_eq!(file.len(), track_pos + 16 + 16);
        file.extend_from_slice(sector_payload);
        file
    }

    #[test]
    fn expose_la_position_reelle_du_secteur_via_bit_position() {
        let payload = [0u8; SECTOR_SIZE];
        let file = build_minimal_stx_with_bit_position(&payload, 12345);
        let image = StxImage::parse(&file).expect("parsing valide");

        assert_eq!(image.sector_bit_position(0, 0, 1), Some(12345));
        // Secteur/piste inexistants : pas de position connue.
        assert_eq!(image.sector_bit_position(0, 0, 2), None);
        assert_eq!(image.sector_bit_position(1, 0, 1), None);
    }

    #[test]
    fn rejette_signature_invalide() {
        let data = vec![0u8; 32];
        let is_bad_magic = matches!(StxImage::parse(&data), Err(StxError::BadMagic));
        assert!(is_bad_magic);
    }

    #[test]
    fn extrait_le_secteur_unique() {
        let mut payload = [0u8; SECTOR_SIZE];
        for (i, b) in payload.iter_mut().enumerate() {
            *b = (i % 256) as u8;
        }
        let file = build_minimal_stx(&payload);
        let image = StxImage::parse(&file).expect("parsing valide");

        assert_eq!(image.num_tracks(), 1);
        assert_eq!(image.num_sides(), 1);
        assert_eq!(image.sectors_per_track(), 1);
        assert!(image.write_protected());

        let sector = image.read_sector(0, 0, 1).expect("secteur present");
        assert_eq!(sector, payload);
    }

    #[test]
    fn extrait_le_secteur_avec_bloc_fuzzy_non_nul() {
        // Bug réel corrigé : sur une piste avec `fuzzy_size != 0` (secteurs
        // à protection par bits instables — le cas de `Rick_Dangerous.stx`,
        // un jeu commercial réel dont le secteur de boot piste 0 ressortait
        // entièrement à zéro avant ce correctif), `data_offset` doit être
        // compté depuis la fin du bloc fuzzy, pas depuis le début du TDR.
        let mut payload = [0u8; SECTOR_SIZE];
        for (i, b) in payload.iter_mut().enumerate() {
            *b = (i % 256) as u8;
        }
        let file = build_stx_with_fuzzy(&payload, 1024);
        let image = StxImage::parse(&file).expect("parsing valide");

        let sector = image.read_sector(0, 0, 1).expect("secteur present");
        assert_eq!(sector, payload, "le bloc fuzzy de 1024 octets doit être sauté, pas confondu avec le secteur");
    }

    /// Comme [`build_stx_with_fuzzy`], mais marque le secteur unique comme
    /// "fuzzy" (bit 7 de `fdc_status`) et écrit `mask` (exactement
    /// `SECTOR_SIZE` octets, comme Hatari) comme bloc fuzzy au lieu d'un
    /// simple remplissage `0xFF`.
    fn build_stx_with_fuzzy_flag(sector_payload: &[u8; SECTOR_SIZE], mask: &[u8; SECTOR_SIZE]) -> Vec<u8> {
        build_stx_with_status(sector_payload, mask, 0x80)
    }

    fn build_stx_with_status(sector_payload: &[u8; SECTOR_SIZE], mask: &[u8; SECTOR_SIZE], fdc_status: u8) -> Vec<u8> {
        let mut file = Vec::new();
        file.extend_from_slice(b"RSY\0");
        file.extend_from_slice(&le16(3));
        file.extend_from_slice(&le16(1));
        file.extend_from_slice(&le16(0));
        file.push(1);
        file.push(2);
        file.extend_from_slice(&[0u8; 4]);

        let sdr_data_offset: u32 = 0;
        let track_pos = file.len();
        let fuzzy_size = SECTOR_SIZE as u32;
        let block_size = 16 + 16 + fuzzy_size + SECTOR_SIZE as u32;

        file.extend_from_slice(&le32(block_size));
        file.extend_from_slice(&le32(fuzzy_size));
        file.extend_from_slice(&le16(1));
        file.extend_from_slice(&le16(0x61));
        file.extend_from_slice(&le16(6261));
        file.push(0);
        file.push(0);

        file.extend_from_slice(&le32(sdr_data_offset));
        file.extend_from_slice(&le16(0));
        file.extend_from_slice(&le16(0));
        file.push(0);
        file.push(0);
        file.push(1);
        file.push(2);
        file.push(0); // crc1
        file.push(0); // crc2
        file.push(fdc_status);
        file.push(0); // reserved

        assert_eq!(file.len(), track_pos + 16 + 16);
        file.extend_from_slice(mask);
        file.extend_from_slice(sector_payload);
        file
    }

    #[test]
    fn secteur_fuzzy_varie_sur_les_positions_instables_et_stabilise_les_autres() {
        // Comme Hatari (`src/floppies/stx.c` : `Byte = (Byte & FuzzyData[i])
        // | (Hatari_rand() & ~FuzzyData[i])`) — trouvé en retraçant un
        // DoubleFault tardif de Rick_Dangerous.stx jusqu'à une vérification
        // de protection qui doit voir deux lectures DIFFÉRENTES du même
        // secteur "fuzzy" pour ne pas se croire face à une copie parfaite.
        let payload = [0x42u8; SECTOR_SIZE];
        let mut mask = [0xFFu8; SECTOR_SIZE]; // tout stable...
        mask[0] = 0x00; // ...sauf l'octet 0, entièrement instable.
        let file = build_stx_with_fuzzy_flag(&payload, &mask);
        let image = StxImage::parse(&file).expect("parsing valide");

        let mut byte0_values = std::collections::HashSet::new();
        for _ in 0..50 {
            let sector = image.read_sector(0, 0, 1).expect("secteur present");
            // Positions stables : toujours la vraie valeur, sur les 50 lectures.
            assert_eq!(&sector[1..], &payload[1..], "les octets stables ne doivent jamais varier");
            byte0_values.insert(sector[0]);
        }
        assert!(
            byte0_values.len() > 1,
            "l'octet fuzzy doit varier d'une lecture à l'autre (obtenu : {byte0_values:?})"
        );
    }

    #[test]
    fn secteur_marque_crc_error_le_signale_sans_affecter_les_octets() {
        // Bit 3 (STX_SECTOR_FLAG_CRC) sans bit 7 (fuzzy) : technique de
        // protection distincte (CRC d'ID délibérément faux) — souvent posée
        // EN MÊME TEMPS que fuzzy sur le même secteur en pratique
        // (Rick_Dangerous.stx pistes 0-4 secteurs 11-12, fdc_status=0x88),
        // mais testée seule ici pour isoler l'effet de chaque bit.
        let mut payload = [0u8; SECTOR_SIZE];
        for (i, b) in payload.iter_mut().enumerate() {
            *b = (i % 256) as u8;
        }
        let mask = [0xFFu8; SECTOR_SIZE];
        let file = build_stx_with_status(&payload, &mask, 0x08);
        let image = StxImage::parse(&file).expect("parsing valide");

        assert!(image.sector_has_crc_error(0, 0, 1));
        assert!(!image.sector_has_crc_error(0, 0, 2), "secteur inexistant : jamais d'erreur CRC");
        let sector = image.read_sector(0, 0, 1).expect("secteur present");
        assert_eq!(sector, payload, "l'erreur CRC ne doit pas altérer les octets transférés");
    }

    #[test]
    fn secteur_non_fuzzy_reste_identique_a_chaque_lecture() {
        let mut payload = [0u8; SECTOR_SIZE];
        for (i, b) in payload.iter_mut().enumerate() {
            *b = (i % 256) as u8;
        }
        let file = build_minimal_stx(&payload);
        let image = StxImage::parse(&file).expect("parsing valide");

        for _ in 0..10 {
            let sector = image.read_sector(0, 0, 1).expect("secteur present");
            assert_eq!(sector, payload, "un secteur ordinaire ne doit jamais varier");
        }
    }

    #[test]
    fn secteur_absent_renvoie_none() {
        let payload = [0u8; SECTOR_SIZE];
        let file = build_minimal_stx(&payload);
        let image = StxImage::parse(&file).expect("parsing valide");

        assert!(image.read_sector(0, 0, 2).is_none());
        assert!(image.read_sector(1, 0, 1).is_none());
        assert!(image.read_sector(0, 1, 1).is_none());
    }

    #[test]
    fn fichier_tronque_est_rejete() {
        let payload = [0u8; SECTOR_SIZE];
        let file = build_minimal_stx(&payload);
        // Coupe en plein milieu du TDR de la seule piste : ni le TDR ni le
        // SDR qui suit ne tiennent dans le fichier tronqué.
        let truncated = &file[..20];
        let is_truncated = matches!(StxImage::parse(truncated), Err(StxError::Truncated));
        assert!(is_truncated);
    }
}
