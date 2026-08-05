//! Lecteur du format `.msa` (Magic Shadow Archiver) : un simple conteneur
//! compressé piste par piste pour une image `.st` brute — pas de métadonnées
//! de protection (contrairement à `.stx`), juste les octets de secteurs.
//!
//! Vérifié contre la documentation publique du format et recoupé avec
//! l'implémentation de référence de Hatari (`src/floppies/msa.c`,
//! `MSA_UnCompress`) : mêmes garde-fous sur l'en-tête (piste de fin ≤ 86,
//! secteurs/piste ≤ 56, faces ≤ 1), même troncature défensive d'une
//! longueur de séquence RLE qui déborderait la piste plutôt que de planter.
//!
//! ## Disposition
//! - En-tête (10 octets, gros-boutien) : `id: u16` (doit valoir `$0E0F`),
//!   `sectors_per_track: u16`, `sides: u16` (0 ou 1 — nombre RÉEL de faces
//!   = cette valeur + 1), `starting_track: u16`, `ending_track: u16`
//!   (toutes deux 0-indexées).
//! - Un bloc par piste, dans l'ordre piste/face croissant en alternant les
//!   faces (piste 0/face 0, piste 0/face 1, piste 1/face 0, ...) :
//!   `data_length: u16` puis `data_length` octets.
//!   - Si `data_length == 512 * sectors_per_track`, la piste est stockée
//!     brute (pas de compression) : copier directement.
//!   - Sinon, RLE : tout octet différent de `$E5` se copie tel quel ;
//!     `$E5` annonce une séquence compressée `$E5 <octet> <longueur:u16
//!     gros-boutien>` (longueur copies de l'octet — y compris pour un
//!     `$E5` isolé sur le disque d'origine, encodé `$E5 $E5 $0001`).
//!
//! Décompresse entièrement en mémoire vers la disposition `.st` linéaire
//! standard puis délègue à [`RawDiskImage`] — aucune logique de lecture
//! propre à ce module, juste un pré-traitement de décompression.
//!
//! ## Limitations
//! - `starting_track != 0` (rarissime en pratique — une image `.msa` réelle
//!   couvre presque toujours le disque depuis la piste 0) : les pistes
//!   avant `starting_track` sont absentes de l'image source et restent à
//!   zéro dans le résultat plutôt que d'échouer tout le chargement.
//! - Lecture seule : ce module ne recompresse rien (voir
//!   [`FloppyDisk::write_protected`] sur `RawDiskImage`, laissé à son
//!   défaut `false` — une vraie disquette `.msa` n'est pas protégée en
//!   écriture par le format lui-même, contrairement à `.stx`).

use super::wd1772::RawDiskImage;

/// Erreur de parsing/décompression d'une image `.msa`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MsaError {
    /// L'en-tête ne commence pas par la signature `$0E0F`, ou une valeur
    /// annoncée est hors des bornes plausibles (mêmes garde-fous que
    /// Hatari : piste de fin > 86, secteurs/piste > 56, faces > 1, piste de
    /// début > piste de fin).
    BadHeader,
    /// Le fichier est plus court que ce que ses propres champs annoncent
    /// (en-tête tronqué, longueur de piste dépassant les données
    /// restantes, ou séquence RLE tronquée en plein milieu).
    Truncated,
}

const HEADER_SIZE: usize = 10;
const SECTOR_SIZE: usize = 512;
const RLE_MARKER: u8 = 0xE5;

/// Décompresse une image `.msa` déjà chargée en mémoire en une image `.st`
/// brute équivalente, exposée via [`RawDiskImage`].
pub fn parse(data: &[u8]) -> Result<RawDiskImage, MsaError> {
    if data.len() < HEADER_SIZE {
        return Err(MsaError::Truncated);
    }
    let id = u16::from_be_bytes([data[0], data[1]]);
    let sectors_per_track = u16::from_be_bytes([data[2], data[3]]);
    let sides_field = u16::from_be_bytes([data[4], data[5]]);
    let starting_track = u16::from_be_bytes([data[6], data[7]]);
    let ending_track = u16::from_be_bytes([data[8], data[9]]);

    if id != 0x0E0F
        || ending_track > 86
        || starting_track > ending_track
        || sectors_per_track > 56
        || sides_field > 1
    {
        return Err(MsaError::BadHeader);
    }

    let sides = sides_field + 1;
    let bytes_per_track = SECTOR_SIZE * sectors_per_track as usize;
    let total_tracks = ending_track as usize + 1;

    let mut out = vec![0u8; total_tracks * sides as usize * bytes_per_track];
    let mut pos = HEADER_SIZE;
    let mut out_pos = starting_track as usize * sides as usize * bytes_per_track;

    for _ in starting_track..=ending_track {
        for _ in 0..sides {
            if pos + 2 > data.len() {
                return Err(MsaError::Truncated);
            }
            let data_length = u16::from_be_bytes([data[pos], data[pos + 1]]) as usize;
            pos += 2;
            if pos + data_length > data.len() {
                return Err(MsaError::Truncated);
            }
            let track_data = &data[pos..pos + data_length];
            pos += data_length;

            let out_track = &mut out[out_pos..out_pos + bytes_per_track];
            if data_length == bytes_per_track {
                out_track.copy_from_slice(track_data);
            } else {
                decompress_track(track_data, out_track)?;
            }
            out_pos += bytes_per_track;
        }
    }

    Ok(RawDiskImage::new(out, total_tracks as u8, sides as u8, sectors_per_track as u8))
}

/// Décompresse une piste RLE dans `output` (déjà dimensionné à la taille
/// exacte attendue, `512 * sectors_per_track`).
fn decompress_track(input: &[u8], output: &mut [u8]) -> Result<(), MsaError> {
    let mut in_pos = 0usize;
    let mut out_pos = 0usize;
    let target = output.len();
    while out_pos < target {
        if in_pos >= input.len() {
            return Err(MsaError::Truncated);
        }
        let byte = input[in_pos];
        in_pos += 1;
        if byte != RLE_MARKER {
            output[out_pos] = byte;
            out_pos += 1;
        } else {
            if in_pos + 3 > input.len() {
                return Err(MsaError::Truncated);
            }
            let value = input[in_pos];
            let run_len = u16::from_be_bytes([input[in_pos + 1], input[in_pos + 2]]) as usize;
            in_pos += 3;
            // Une image corrompue peut annoncer une longueur qui déborde de
            // la piste — tronquer plutôt que planter, comme Hatari
            // (`MSA_UnCompress` : "Illegal run length -> corrupted disk
            // image?").
            let run_len = run_len.min(target - out_pos);
            output[out_pos..out_pos + run_len].fill(value);
            out_pos += run_len;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::peripherals::atari_st::wd1772::FloppyDisk;

    fn header(sectors_per_track: u16, sides: u16, starting_track: u16, ending_track: u16) -> Vec<u8> {
        let mut h = Vec::with_capacity(HEADER_SIZE);
        h.extend_from_slice(&0x0E0Fu16.to_be_bytes());
        h.extend_from_slice(&sectors_per_track.to_be_bytes());
        h.extend_from_slice(&sides.to_be_bytes());
        h.extend_from_slice(&starting_track.to_be_bytes());
        h.extend_from_slice(&ending_track.to_be_bytes());
        h
    }

    #[test]
    fn rejette_signature_invalide() {
        let mut data = header(9, 0, 0, 0);
        data[0] = 0; // casse l'ID $0E0F
        assert!(matches!(parse(&data), Err(MsaError::BadHeader)));
    }

    #[test]
    fn rejette_geometrie_hors_bornes() {
        assert!(matches!(parse(&header(9, 0, 0, 87)), Err(MsaError::BadHeader))); // piste de fin > 86
        assert!(matches!(parse(&header(9, 2, 0, 0)), Err(MsaError::BadHeader))); // faces > 1
        assert!(matches!(parse(&header(9, 0, 5, 2)), Err(MsaError::BadHeader))); // début > fin
    }

    #[test]
    fn piste_non_compressee_copiee_telle_quelle() {
        let mut data = header(1, 0, 0, 0); // 1 piste, 1 face, 1 secteur/piste
        let mut payload = [0u8; SECTOR_SIZE];
        for (i, b) in payload.iter_mut().enumerate() {
            *b = (i % 256) as u8;
        }
        data.extend_from_slice(&(SECTOR_SIZE as u16).to_be_bytes()); // data_length == taille non compressée
        data.extend_from_slice(&payload);

        let image = parse(&data).expect("parsing valide");
        assert_eq!(image.num_tracks(), 1);
        assert_eq!(image.num_sides(), 1);
        assert_eq!(image.sectors_per_track(), 1);
        assert_eq!(image.read_sector(0, 0, 1).unwrap(), payload);
    }

    #[test]
    fn decompresse_une_sequence_rle_simple() {
        let mut data = header(1, 0, 0, 0);
        // 6 octets $AA (RLE), puis le reste du secteur en $00 littéraux via
        // une seconde séquence RLE (511-6 = 506 octets de $00).
        let mut compressed = Vec::new();
        compressed.extend_from_slice(&[RLE_MARKER, 0xAA, 0, 6]);
        compressed.extend_from_slice(&[RLE_MARKER, 0x00, (506u16 >> 8) as u8, 506u16 as u8]);
        data.extend_from_slice(&(compressed.len() as u16).to_be_bytes());
        data.extend_from_slice(&compressed);

        let image = parse(&data).expect("parsing valide");
        let sector = image.read_sector(0, 0, 1).unwrap();
        assert_eq!(&sector[0..6], &[0xAA; 6]);
        assert_eq!(&sector[6..], &[0x00; 506]);
    }

    #[test]
    fn octet_e5_isole_encode_comme_une_sequence_de_longueur_un() {
        let mut data = header(1, 0, 0, 0);
        let mut compressed = Vec::new();
        compressed.extend_from_slice(&[RLE_MARKER, RLE_MARKER, 0, 1]); // un seul $E5 réel
        compressed.extend_from_slice(&[RLE_MARKER, 0x00, (511u16 >> 8) as u8, 511u16 as u8]);
        data.extend_from_slice(&(compressed.len() as u16).to_be_bytes());
        data.extend_from_slice(&compressed);

        let image = parse(&data).expect("parsing valide");
        let sector = image.read_sector(0, 0, 1).unwrap();
        assert_eq!(sector[0], 0xE5);
        assert_eq!(&sector[1..], &[0x00; 511]);
    }

    #[test]
    fn longueur_rle_debordante_est_tronquee_pas_paniquee() {
        let mut data = header(1, 0, 0, 0);
        let mut compressed = Vec::new();
        // Annonce 2000 octets de $AA alors que la piste ne fait que 512
        // octets — doit être tronqué à 512, pas planter.
        compressed.extend_from_slice(&[RLE_MARKER, 0xAA, (2000u16 >> 8) as u8, 2000u16 as u8]);
        data.extend_from_slice(&(compressed.len() as u16).to_be_bytes());
        data.extend_from_slice(&compressed);

        let image = parse(&data).expect("parsing valide");
        let sector = image.read_sector(0, 0, 1).unwrap();
        assert_eq!(&sector[..], &[0xAA; SECTOR_SIZE]);
    }

    #[test]
    fn fichier_tronque_echoue_proprement() {
        let mut data = header(9, 1, 0, 1); // annonce 2 pistes x 2 faces mais aucune donnée de piste
        data.truncate(HEADER_SIZE);
        assert!(matches!(parse(&data), Err(MsaError::Truncated)));
    }

    #[test]
    fn geometrie_double_face_deux_pistes() {
        let mut data = header(1, 1, 0, 1); // 2 pistes, 2 faces, 1 secteur/piste
        for marker in [0x11u8, 0x22, 0x33, 0x44] {
            let payload = [marker; SECTOR_SIZE];
            data.extend_from_slice(&(SECTOR_SIZE as u16).to_be_bytes());
            data.extend_from_slice(&payload);
        }
        let image = parse(&data).expect("parsing valide");
        assert_eq!(image.num_tracks(), 2);
        assert_eq!(image.num_sides(), 2);
        // Ordre piste0/face0, piste0/face1, piste1/face0, piste1/face1.
        assert_eq!(image.read_sector(0, 0, 1).unwrap()[0], 0x11);
        assert_eq!(image.read_sector(0, 1, 1).unwrap()[0], 0x22);
        assert_eq!(image.read_sector(1, 0, 1).unwrap()[0], 0x33);
        assert_eq!(image.read_sector(1, 1, 1).unwrap()[0], 0x44);
    }
}
