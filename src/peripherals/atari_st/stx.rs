//! Lecteur minimal de disquettes au format `.stx` (Pasti).
//!
//! Aucune spécification officielle du format `.stx` n'a été consultée pour
//! écrire ce module : la disposition des champs ci-dessous a été déduite
//! empiriquement par inspection directe d'images réelles (recoupement de
//! plusieurs pistes/secteurs pour vérifier la cohérence interne — écart de
//! 512 octets entre `data_offset` de secteurs consécutifs, charge utile
//! tenant dans `block_size` avec la marge attendue pour `fuzzy_size`, etc.).
//! Seuls les champs nécessaires à extraire le contenu brut des secteurs
//! sont exploités ; toute la métadonnée de protection (bits `fuzzy`,
//! `flags`, `bit_position`, `read_time`, piste brute…) est ignorée.
//!
//! ## Disposition observée
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
//! - `data_offset` est **relatif au début du TDR de la piste** (et non au
//!   début du fichier, ni à la fin du tableau de SDR) : c'est le fait qui
//!   a demandé le plus de vérification empirique (trois hypothèses
//!   testées, cette dernière est celle qui reste cohérente sur toutes les
//!   pistes échantillonnées).
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
            let sector_count =
                u16::from_le_bytes(data[pos + 8..pos + 10].try_into().unwrap()) as usize;
            let track_number = data[pos + 14];
            let track = track_number & 0x7F;
            let side = (track_number >> 7) & 1;

            let mut sectors = Vec::with_capacity(sector_count);
            let mut sdr_pos = pos + 16;
            for _ in 0..sector_count {
                if sdr_pos + 16 > data.len() {
                    return Err(StxError::Truncated);
                }
                let data_offset =
                    u32::from_le_bytes(data[sdr_pos..sdr_pos + 4].try_into().unwrap()) as usize;
                let sector = data[sdr_pos + 10];
                let size_code = data[sdr_pos + 11];
                if size_code == 2 {
                    let abs = pos + data_offset;
                    if abs + SECTOR_SIZE <= data.len() {
                        let mut buf = [0u8; SECTOR_SIZE];
                        buf.copy_from_slice(&data[abs..abs + SECTOR_SIZE]);
                        sectors.push(StxSector { sector, data: buf });
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
        })
    }

    fn find_sector(&self, track: u8, side: u8, sector: u8) -> Option<&[u8; SECTOR_SIZE]> {
        self.tracks
            .iter()
            .find(|t| t.track == track && t.side == side)
            .and_then(|t| t.sectors.iter().find(|s| s.sector == sector))
            .map(|s| &s.data)
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

    fn read_sector(&self, track: u8, side: u8, sector: u8) -> Option<[u8; SECTOR_SIZE]> {
        self.find_sector(track, side, sector).copied()
    }

    fn write_sector(&mut self, _track: u8, _side: u8, _sector: u8, _data: &[u8; SECTOR_SIZE]) {}
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
        let mut file = Vec::new();
        // En-tête.
        file.extend_from_slice(b"RSY\0");
        file.extend_from_slice(&le16(3)); // version
        file.extend_from_slice(&le16(1)); // tool
        file.extend_from_slice(&le16(0)); // reserved
        file.push(1); // track_count
        file.push(2); // revision
        file.extend_from_slice(&[0u8; 4]); // reserved

        let sdr_data_offset: u32 = 16 + 16; // relatif au TDR : TDR(16) + SDR(16)
        let track_pos = file.len();
        let block_size = 16 + 16 + SECTOR_SIZE as u32;

        // TDR.
        file.extend_from_slice(&le32(block_size));
        file.extend_from_slice(&le32(0)); // fuzzy_size
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
        file.extend_from_slice(sector_payload);
        file
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
