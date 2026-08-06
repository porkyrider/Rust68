//! Minimal reader for `.stx` (Pasti) floppy disk images.
//!
//! Initially written without an official specification (layout inferred
//! empirically), then fixed and cross-checked after consulting the
//! format's public documentation (Pasti/STX, info-coach.fr and
//! atari.8bitchip.info) following a real bug found on a real protected
//! image (see the doc of `sector_data_ref` below: sector 1, track 0 of
//! `Rick_Dangerous.stx`, a well-known commercial game, came out entirely
//! zeroed). The "fuzzy" bits themselves are now simulated (see
//! [`StxImage::read_sector`], 2026-08-04 — found by tracing a late
//! DoubleFault in `Rick_Dangerous.stx` back to this never-rewritten
//! area). `bit_position` (the real position of the ID field on the
//! physical track) has been exposed via [`FloppyDisk::sector_bit_position`]
//! since 2026-08-05 — found by tracing a ~2x slowdown factor compared to
//! Hatari on a large sequential transfer: without this data, the seek
//! latency was estimated from uniform spacing between sectors, which is
//! wrong on a track with non-standard formatting (more sectors packed
//! closer together than a normal track, a common protection technique).
//! `read_time` and the full raw track image remain ignored — see the
//! limitations below.
//!
//! ## Layout
//! - File header (16 bytes): magic `"RSY\0"`, version u16 LE,
//!   tool u16 LE, reserved u16 LE, track count u8, revision u8,
//!   reserved u32.
//! - One track record (TDR, 16 bytes) per track, the records following
//!   one another sequentially (each TDR starts at
//!   `previous position + previous block_size`):
//!   `block_size: u32 LE`, `fuzzy_size: u32 LE`, `sector_count: u16 LE`,
//!   `flags: u16 LE`, `track_length: u16 LE`, `track_number: u8` (bits
//!   0-6 = track number, bit 7 = side), `track_type: u8`.
//! - `sector_count` sector records (SDR, 16 bytes) immediately follow
//!   the TDR: `data_offset: u32 LE`, `bit_position: u16 LE`,
//!   `read_time: u16 LE`, `track: u8`, `head: u8`, `sector: u8` (the
//!   logical number, potentially non-sequential — sector
//!   interleaving), `size_code: u8` (size = `128 << size_code`), `crc1: u8`,
//!   `crc2: u8`, `fdc_status: u8`, `reserved: u8`.
//! - Right after the SDR table, a block of `fuzzy_size` bytes (mask of
//!   unstable bits for sectors protected with "fuzzy bits") — may be
//!   absent (`fuzzy_size == 0`). Each SDR's `data_offset` is relative to
//!   the end of THIS block (end of the SDR table if absent), NOT to the
//!   start of the TDR nor the start of the file — see [`StxImage::parse`]
//!   (`sector_data_ref`). An optional raw track image (full WD1772
//!   capture, present when bit 6 of `flags` is set) may follow the fuzzy
//!   block before the actual sector data, but `data_offset` already
//!   skips over it on its own: no need to locate it separately to extract
//!   a `size_code == 2` sector.
//!
//! ## Limitations (minimal reader)
//! - Only `size_code == 2` sectors (512 bytes, standard GEMDOS/ST size)
//!   are extracted; other sizes are ignored.
//! - A sector payload that would overflow past the end of the file
//!   (truncated image) is silently ignored (the sector is not added)
//!   rather than failing the whole parse — only an incomplete TDR or
//!   SDR is treated as a fatal error ([`StxError::Truncated`]).
//! - Tracks without discrete sectors (raw stream capture for advanced
//!   protection, `sector_count == 0`) provide no data at all:
//!   `read_sector` will return `None`.
//! - `sectors_per_track()` returns the maximum observed across the whole
//!   disk; some real tracks have fewer (a protection track with a sector
//!   count different from the norm) — a multi-sector read (WD1772 bit M)
//!   may therefore fail prematurely on those tracks instead of stopping
//!   cleanly at the end of the track.
//! - Read-only: `write_sector` is ignored (no rewriting of the `.stx`
//!   file, see [`StxImage::write_protected`]).

use super::wd1772::{FloppyDisk, SECTOR_SIZE};

struct StxSector {
    sector: u8,
    data: [u8; SECTOR_SIZE],
    /// Real position (in bits, from the index pulse) of this sector's ID
    /// field on the original physical track — captured as-is by the tool
    /// that produced the `.stx` image, see
    /// [`FloppyDisk::sector_bit_position`].
    bit_position: u16,
    /// "Fuzzy" bit mask (protection using physically unstable bits): one
    /// mask byte per byte of `data`, bit set to 1 = stable position (keep
    /// the real bit from `data`), bit set to 0 = unstable position (each
    /// read must return an unpredictable value on that bit, just like the
    /// real magnetic medium). `None` for an ordinary sector (the vast
    /// majority). See [`StxImage::read_sector`] for how this is applied.
    fuzzy_mask: Option<[u8; SECTOR_SIZE]>,
    /// Bit 3 (`STX_SECTOR_FLAG_CRC`) of `fdc_status`: deliberately wrong ID
    /// CRC (a protection technique — often set AT THE SAME TIME as
    /// `fuzzy_mask` on the same sector, cf. `Rick_Dangerous.stx` tracks
    /// 0-4, sectors 11-12). See [`FloppyDisk::sector_has_crc_error`].
    has_crc_error: bool,
}

struct StxTrack {
    track: u8,
    side: u8,
    sectors: Vec<StxSector>,
}

/// Error while parsing a `.stx` image.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StxError {
    /// The header does not start with the `"RSY\0"` signature.
    BadMagic,
    /// The file is shorter than what its own fields announce.
    Truncated,
}

/// `.stx` disk image loaded in memory, exposed as [`FloppyDisk`].
pub struct StxImage {
    tracks: Vec<StxTrack>,
    num_tracks: u8,
    num_sides: u8,
    sectors_per_track: u8,
    /// State of a small xorshift64 used to simulate "fuzzy" bits (see
    /// [`Self::read_sector`]) — `Cell` because `FloppyDisk::read_sector`
    /// takes `&self`, not `&mut self` (a real floppy disk isn't "modified"
    /// by reading it), but each read still needs to advance the state so
    /// it varies from one call to the next.
    fuzzy_rng: std::cell::Cell<u64>,
}

impl StxImage {
    /// Parses a `.stx` image already loaded in memory.
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
            // Size (in bytes) of the "fuzzy" bits block (sectors protected
            // with unstable bits) inserted right after the SDR table —
            // never read before, even though `data_offset` (below) is
            // relative to the end of THIS block, not to the start of the
            // TDR. Confirmed empirically on a real protected image
            // (Rick_Dangerous.stx, track 0): without this offset, the
            // extracted sector landed right in the middle of the fuzzy
            // block/raw track image, producing bogus data (zeros) instead
            // of the real sector content.
            let fuzzy_size = u32::from_le_bytes(data[pos + 4..pos + 8].try_into().unwrap()) as usize;
            let sector_count =
                u16::from_le_bytes(data[pos + 8..pos + 10].try_into().unwrap()) as usize;
            let track_number = data[pos + 14];
            let track = track_number & 0x7F;
            let side = (track_number >> 7) & 1;

            // Reference for each SDR's `data_offset`: end of the SDR table
            // (TDR + 16 bytes per sector) + fuzzy block — NOT the start of
            // the TDR. `data_offset` itself already accounts for crossing
            // over the optional raw track image that may follow the fuzzy
            // block: no need to locate it separately.
            let sector_data_ref = pos + 16 + sector_count * 16 + fuzzy_size;
            // Start of the fuzzy block itself (before it gets skipped by
            // `sector_data_ref` above): each sector marked "fuzzy" (bit 7 of
            // `fdc_status`) consumes its `SECTOR_SIZE` mask bytes from it
            // SEQUENTIALLY, in the order of the SDR table — verified
            // against Hatari (`src/floppies/stx.c`,
            // `pStxTrack->pFuzzyData` advanced by `SectorSize` only for
            // `STX_SECTOR_FLAG_FUZZY` sectors).
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
                // Bit 7 = STX_SECTOR_FLAG_FUZZY (protection using
                // physically unstable bits — see the doc of `fuzzy_mask`).
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
            // Fixed seed: determinism doesn't matter here at all (the game
            // can't predict these bytes anyway), and a fixed seed keeps
            // runs reproducible for debugging.
            fuzzy_rng: std::cell::Cell::new(0x9E3779B97F4A7C15),
        })
    }

    fn find_sector(&self, track: u8, side: u8, sector: u8) -> Option<&StxSector> {
        self.tracks
            .iter()
            .find(|t| t.track == track && t.side == side)
            .and_then(|t| t.sectors.iter().find(|s| s.sector == sector))
    }

    /// One more "random" byte (xorshift64*, a single instance per image
    /// shared across all fuzzy sectors — no need for per-sector state,
    /// only variation from one read to the next matters).
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

    /// For a "fuzzy" sector (protection using physically unstable bits,
    /// see the doc of [`StxSector::fuzzy_mask`]), combines the real byte
    /// with noise on the unstable positions — like the real medium (each
    /// read gives a different result there) and like Hatari
    /// (`src/floppies/stx.c`: `Byte = (Byte & FuzzyData[i]) |
    /// (Hatari_rand() & ~FuzzyData[i])`). Without this, a protection check
    /// that compares two reads of the same sector and expects them to
    /// DIFFER (proof that it's a physical medium and not a perfect digital
    /// copy) would always see two identical reads here — found while
    /// digging into why `Rick_Dangerous.stx` eventually executed
    /// uninitialized memory as code, well after loading (track 0, which
    /// has a fuzzy block, verified to show no variation at all before this
    /// fix).
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

    /// REAL sector count of the targeted track/side (from its own TDR),
    /// not the global maximum returned by `sectors_per_track()` — a
    /// protected `.stx` image typically does NOT have the same number of
    /// sectors on every track (see the module doc), so using the global
    /// maximum would progressively misalign the rotational latency
    /// calculation from one track to the next.
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

    /// Builds a synthetic single-track, single-sector `.stx` image, to test
    /// parsing without depending on a real file.
    fn build_minimal_stx(sector_payload: &[u8; SECTOR_SIZE]) -> Vec<u8> {
        build_stx_with_fuzzy(sector_payload, 0)
    }

    /// Like [`build_minimal_stx`], with a `fuzzy_size` block (potentially
    /// non-zero) inserted between the SDR table and the sector payload —
    /// see [`StxImage::parse`] (`sector_data_ref`): `data_offset` must be
    /// relative to the end of THIS block, not to the start of the TDR.
    fn build_stx_with_fuzzy(sector_payload: &[u8; SECTOR_SIZE], fuzzy_size: u32) -> Vec<u8> {
        let mut file = Vec::new();
        // Header.
        file.extend_from_slice(b"RSY\0");
        file.extend_from_slice(&le16(3)); // version
        file.extend_from_slice(&le16(1)); // tool
        file.extend_from_slice(&le16(0)); // reserved
        file.push(1); // track_count
        file.push(2); // revision
        file.extend_from_slice(&[0u8; 4]); // reserved

        // `data_offset` relative to the end of the SDR table + fuzzy block
        // (see the module doc): here, the sector immediately follows that
        // block, so `data_offset = 0`.
        let sdr_data_offset: u32 = 0;
        let track_pos = file.len();
        let block_size = 16 + 16 + fuzzy_size + SECTOR_SIZE as u32;

        // TDR.
        file.extend_from_slice(&le32(block_size));
        file.extend_from_slice(&le32(fuzzy_size));
        file.extend_from_slice(&le16(1)); // sector_count
        file.extend_from_slice(&le16(0x61)); // flags
        file.extend_from_slice(&le16(6261)); // track_length
        file.push(0); // track_number (track 0, side 0)
        file.push(0); // track_type

        // SDR.
        file.extend_from_slice(&le32(sdr_data_offset));
        file.extend_from_slice(&le16(0)); // bit_position
        file.extend_from_slice(&le16(0)); // read_time
        file.push(0); // track
        file.push(0); // head
        file.push(1); // sector (1-indexed)
        file.push(2); // size_code = 512 bytes
        file.extend_from_slice(&[0u8; 4]); // crc1, crc2, fdc_status, reserved

        assert_eq!(file.len(), track_pos + 16 + 16);
        file.extend_from_slice(&vec![0xFFu8; fuzzy_size as usize]);
        file.extend_from_slice(sector_payload);
        file
    }

    /// Like [`build_minimal_stx`], but with a chosen `bit_position` for the
    /// sector — tests that this field (always 0 so far in the other
    /// builders of this module) is properly extracted and exposed via
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
    fn exposes_real_sector_position_via_bit_position() {
        let payload = [0u8; SECTOR_SIZE];
        let file = build_minimal_stx_with_bit_position(&payload, 12345);
        let image = StxImage::parse(&file).expect("valid parsing");

        assert_eq!(image.sector_bit_position(0, 0, 1), Some(12345));
        // Nonexistent sector/track: no known position.
        assert_eq!(image.sector_bit_position(0, 0, 2), None);
        assert_eq!(image.sector_bit_position(1, 0, 1), None);
    }

    #[test]
    fn rejects_invalid_signature() {
        let data = vec![0u8; 32];
        let is_bad_magic = matches!(StxImage::parse(&data), Err(StxError::BadMagic));
        assert!(is_bad_magic);
    }

    #[test]
    fn extracts_the_single_sector() {
        let mut payload = [0u8; SECTOR_SIZE];
        for (i, b) in payload.iter_mut().enumerate() {
            *b = (i % 256) as u8;
        }
        let file = build_minimal_stx(&payload);
        let image = StxImage::parse(&file).expect("valid parsing");

        assert_eq!(image.num_tracks(), 1);
        assert_eq!(image.num_sides(), 1);
        assert_eq!(image.sectors_per_track(), 1);
        assert!(image.write_protected());

        let sector = image.read_sector(0, 0, 1).expect("sector present");
        assert_eq!(sector, payload);
    }

    #[test]
    fn extracts_sector_with_nonzero_fuzzy_block() {
        // Real bug fixed: on a track with `fuzzy_size != 0` (sectors
        // protected with unstable bits — the case of `Rick_Dangerous.stx`,
        // a real commercial game whose track 0 boot sector came out
        // entirely zeroed before this fix), `data_offset` must be counted
        // from the end of the fuzzy block, not from the start of the TDR.
        let mut payload = [0u8; SECTOR_SIZE];
        for (i, b) in payload.iter_mut().enumerate() {
            *b = (i % 256) as u8;
        }
        let file = build_stx_with_fuzzy(&payload, 1024);
        let image = StxImage::parse(&file).expect("valid parsing");

        let sector = image.read_sector(0, 0, 1).expect("sector present");
        assert_eq!(sector, payload, "the 1024-byte fuzzy block must be skipped, not confused with the sector");
    }

    /// Like [`build_stx_with_fuzzy`], but marks the single sector as
    /// "fuzzy" (bit 7 of `fdc_status`) and writes `mask` (exactly
    /// `SECTOR_SIZE` bytes, like Hatari) as the fuzzy block instead of a
    /// plain `0xFF` fill.
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
    fn fuzzy_sector_varies_on_unstable_positions_and_stays_stable_on_others() {
        // Like Hatari (`src/floppies/stx.c`: `Byte = (Byte & FuzzyData[i])
        // | (Hatari_rand() & ~FuzzyData[i])`) — found by tracing a late
        // DoubleFault in Rick_Dangerous.stx back to a protection check that
        // must see two DIFFERENT reads of the same "fuzzy" sector so as not
        // to believe it's facing a perfect copy.
        let payload = [0x42u8; SECTOR_SIZE];
        let mut mask = [0xFFu8; SECTOR_SIZE]; // everything stable...
        mask[0] = 0x00; // ...except byte 0, entirely unstable.
        let file = build_stx_with_fuzzy_flag(&payload, &mask);
        let image = StxImage::parse(&file).expect("valid parsing");

        let mut byte0_values = std::collections::HashSet::new();
        for _ in 0..50 {
            let sector = image.read_sector(0, 0, 1).expect("sector present");
            // Stable positions: always the real value, across all 50 reads.
            assert_eq!(&sector[1..], &payload[1..], "stable bytes must never vary");
            byte0_values.insert(sector[0]);
        }
        assert!(
            byte0_values.len() > 1,
            "the fuzzy byte must vary from one read to the next (got: {byte0_values:?})"
        );
    }

    #[test]
    fn sector_marked_crc_error_reports_it_without_affecting_bytes() {
        // Bit 3 (STX_SECTOR_FLAG_CRC) without bit 7 (fuzzy): a distinct
        // protection technique (deliberately wrong ID CRC) — often set AT
        // THE SAME TIME as fuzzy on the same sector in practice
        // (Rick_Dangerous.stx tracks 0-4 sectors 11-12, fdc_status=0x88),
        // but tested alone here to isolate the effect of each bit.
        let mut payload = [0u8; SECTOR_SIZE];
        for (i, b) in payload.iter_mut().enumerate() {
            *b = (i % 256) as u8;
        }
        let mask = [0xFFu8; SECTOR_SIZE];
        let file = build_stx_with_status(&payload, &mask, 0x08);
        let image = StxImage::parse(&file).expect("valid parsing");

        assert!(image.sector_has_crc_error(0, 0, 1));
        assert!(!image.sector_has_crc_error(0, 0, 2), "nonexistent sector: never a CRC error");
        let sector = image.read_sector(0, 0, 1).expect("sector present");
        assert_eq!(sector, payload, "the CRC error must not alter the transferred bytes");
    }

    #[test]
    fn non_fuzzy_sector_stays_identical_on_every_read() {
        let mut payload = [0u8; SECTOR_SIZE];
        for (i, b) in payload.iter_mut().enumerate() {
            *b = (i % 256) as u8;
        }
        let file = build_minimal_stx(&payload);
        let image = StxImage::parse(&file).expect("valid parsing");

        for _ in 0..10 {
            let sector = image.read_sector(0, 0, 1).expect("sector present");
            assert_eq!(sector, payload, "an ordinary sector must never vary");
        }
    }

    #[test]
    fn missing_sector_returns_none() {
        let payload = [0u8; SECTOR_SIZE];
        let file = build_minimal_stx(&payload);
        let image = StxImage::parse(&file).expect("valid parsing");

        assert!(image.read_sector(0, 0, 2).is_none());
        assert!(image.read_sector(1, 0, 1).is_none());
        assert!(image.read_sector(0, 1, 1).is_none());
    }

    #[test]
    fn truncated_file_is_rejected() {
        let payload = [0u8; SECTOR_SIZE];
        let file = build_minimal_stx(&payload);
        // Cuts right in the middle of the sole track's TDR: neither the TDR
        // nor the following SDR fit in the truncated file.
        let truncated = &file[..20];
        let is_truncated = matches!(StxImage::parse(truncated), Err(StxError::Truncated));
        assert!(is_truncated);
    }
}
