//! Reader for the `.msa` format (Magic Shadow Archiver): a simple
//! per-track compressed container for a raw `.st` image — no protection
//! metadata (unlike `.stx`), just the sector bytes.
//!
//! Verified against the format's public documentation and cross-checked
//! with Hatari's reference implementation (`src/floppies/msa.c`,
//! `MSA_UnCompress`): same header guards (ending track ≤ 86,
//! sectors/track ≤ 56, sides ≤ 1), same defensive truncation of an RLE run
//! length that would overflow the track rather than panicking.
//!
//! ## Layout
//! - Header (10 bytes, big-endian): `id: u16` (must be `$0E0F`),
//!   `sectors_per_track: u16`, `sides: u16` (0 or 1 — the REAL number of
//!   sides = this value + 1), `starting_track: u16`, `ending_track: u16`
//!   (both 0-indexed).
//! - One block per track, in increasing track/side order, alternating
//!   sides (track 0/side 0, track 0/side 1, track 1/side 0, ...):
//!   `data_length: u16` followed by `data_length` bytes.
//!   - If `data_length == 512 * sectors_per_track`, the track is stored
//!     raw (no compression): copy directly.
//!   - Otherwise, RLE: any byte other than `$E5` is copied as-is; `$E5`
//!     introduces a compressed run `$E5 <byte> <length:u16 big-endian>`
//!     (length copies of the byte — including for a lone `$E5` on the
//!     source disk, encoded as `$E5 $E5 $0001`).
//!
//! Fully decompresses in memory into the standard linear `.st` layout,
//! then delegates to [`RawDiskImage`] — no reading logic of its own in
//! this module, just decompression pre-processing.
//!
//! ## Limitations
//! - `starting_track != 0` (extremely rare in practice — a real `.msa`
//!   image almost always covers the disk starting from track 0): tracks
//!   before `starting_track` are absent from the source image and remain
//!   zero in the result rather than failing the whole load.
//! - Read-only: this module doesn't recompress anything (see
//!   [`FloppyDisk::write_protected`] on `RawDiskImage`, left at its
//!   default `false` — a real `.msa` floppy disk isn't write-protected by
//!   the format itself, unlike `.stx`).

use super::wd1772::RawDiskImage;

/// Error while parsing/decompressing a `.msa` image.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MsaError {
    /// The header doesn't start with the `$0E0F` signature, or an
    /// announced value is out of plausible bounds (same guards as
    /// Hatari: ending track > 86, sectors/track > 56, sides > 1, starting
    /// track > ending track).
    BadHeader,
    /// The file is shorter than what its own fields announce (truncated
    /// header, track length exceeding the remaining data, or an RLE run
    /// truncated mid-sequence).
    Truncated,
}

const HEADER_SIZE: usize = 10;
const SECTOR_SIZE: usize = 512;
const RLE_MARKER: u8 = 0xE5;

/// Decompresses a `.msa` image already loaded in memory into an equivalent
/// raw `.st` image, exposed via [`RawDiskImage`].
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

/// Decompresses an RLE track into `output` (already sized to the exact
/// expected size, `512 * sectors_per_track`).
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
            // A corrupted image may announce a length that overflows the
            // track — truncate rather than panic, like Hatari
            // (`MSA_UnCompress`: "Illegal run length -> corrupted disk
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
    fn rejects_invalid_signature() {
        let mut data = header(9, 0, 0, 0);
        data[0] = 0; // break the $0E0F ID
        assert!(matches!(parse(&data), Err(MsaError::BadHeader)));
    }

    #[test]
    fn rejects_out_of_bounds_geometry() {
        assert!(matches!(parse(&header(9, 0, 0, 87)), Err(MsaError::BadHeader))); // ending track > 86
        assert!(matches!(parse(&header(9, 2, 0, 0)), Err(MsaError::BadHeader))); // sides > 1
        assert!(matches!(parse(&header(9, 0, 5, 2)), Err(MsaError::BadHeader))); // start > end
    }

    #[test]
    fn uncompressed_track_is_copied_as_is() {
        let mut data = header(1, 0, 0, 0); // 1 track, 1 side, 1 sector/track
        let mut payload = [0u8; SECTOR_SIZE];
        for (i, b) in payload.iter_mut().enumerate() {
            *b = (i % 256) as u8;
        }
        data.extend_from_slice(&(SECTOR_SIZE as u16).to_be_bytes()); // data_length == uncompressed size
        data.extend_from_slice(&payload);

        let image = parse(&data).expect("valid parsing");
        assert_eq!(image.num_tracks(), 1);
        assert_eq!(image.num_sides(), 1);
        assert_eq!(image.sectors_per_track(), 1);
        assert_eq!(image.read_sector(0, 0, 1).unwrap(), payload);
    }

    #[test]
    fn decompresses_a_simple_rle_run() {
        let mut data = header(1, 0, 0, 0);
        // 6 bytes of $AA (RLE), then the rest of the sector as literal $00
        // via a second RLE run (511-6 = 506 bytes of $00).
        let mut compressed = Vec::new();
        compressed.extend_from_slice(&[RLE_MARKER, 0xAA, 0, 6]);
        compressed.extend_from_slice(&[RLE_MARKER, 0x00, (506u16 >> 8) as u8, 506u16 as u8]);
        data.extend_from_slice(&(compressed.len() as u16).to_be_bytes());
        data.extend_from_slice(&compressed);

        let image = parse(&data).expect("valid parsing");
        let sector = image.read_sector(0, 0, 1).unwrap();
        assert_eq!(&sector[0..6], &[0xAA; 6]);
        assert_eq!(&sector[6..], &[0x00; 506]);
    }

    #[test]
    fn lone_e5_byte_is_encoded_as_a_length_one_run() {
        let mut data = header(1, 0, 0, 0);
        let mut compressed = Vec::new();
        compressed.extend_from_slice(&[RLE_MARKER, RLE_MARKER, 0, 1]); // a single real $E5
        compressed.extend_from_slice(&[RLE_MARKER, 0x00, (511u16 >> 8) as u8, 511u16 as u8]);
        data.extend_from_slice(&(compressed.len() as u16).to_be_bytes());
        data.extend_from_slice(&compressed);

        let image = parse(&data).expect("valid parsing");
        let sector = image.read_sector(0, 0, 1).unwrap();
        assert_eq!(sector[0], 0xE5);
        assert_eq!(&sector[1..], &[0x00; 511]);
    }

    #[test]
    fn overflowing_rle_length_is_truncated_not_panicked() {
        let mut data = header(1, 0, 0, 0);
        let mut compressed = Vec::new();
        // Announces 2000 bytes of $AA while the track is only 512 bytes —
        // must be truncated to 512, not panic.
        compressed.extend_from_slice(&[RLE_MARKER, 0xAA, (2000u16 >> 8) as u8, 2000u16 as u8]);
        data.extend_from_slice(&(compressed.len() as u16).to_be_bytes());
        data.extend_from_slice(&compressed);

        let image = parse(&data).expect("valid parsing");
        let sector = image.read_sector(0, 0, 1).unwrap();
        assert_eq!(&sector[..], &[0xAA; SECTOR_SIZE]);
    }

    #[test]
    fn truncated_file_fails_cleanly() {
        let mut data = header(9, 1, 0, 1); // announces 2 tracks x 2 sides but no track data
        data.truncate(HEADER_SIZE);
        assert!(matches!(parse(&data), Err(MsaError::Truncated)));
    }

    #[test]
    fn double_sided_geometry_two_tracks() {
        let mut data = header(1, 1, 0, 1); // 2 tracks, 2 sides, 1 sector/track
        for marker in [0x11u8, 0x22, 0x33, 0x44] {
            let payload = [marker; SECTOR_SIZE];
            data.extend_from_slice(&(SECTOR_SIZE as u16).to_be_bytes());
            data.extend_from_slice(&payload);
        }
        let image = parse(&data).expect("valid parsing");
        assert_eq!(image.num_tracks(), 2);
        assert_eq!(image.num_sides(), 2);
        // Order: track0/side0, track0/side1, track1/side0, track1/side1.
        assert_eq!(image.read_sector(0, 0, 1).unwrap()[0], 0x11);
        assert_eq!(image.read_sector(0, 1, 1).unwrap()[0], 0x22);
        assert_eq!(image.read_sector(1, 0, 1).unwrap()[0], 0x33);
        assert_eq!(image.read_sector(1, 1, 1).unwrap()[0], 0x44);
    }
}
