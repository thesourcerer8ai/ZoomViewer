//! Pattern Healing Data Provider
//!
//! Operates on each 512-byte DATA area in every page from the upstream provider
//! and attempts to heal damaged/erased sectors using several algorithms:
//!
//! 1. **Fill healing**: >90% 0x00, 0xFF, or 0x77 → fill the whole 512 bytes.
//! 2. **LBA healing**: >300 'x' ASCII characters in bytes 100–509 → extract the
//!    LBA address three ways (decimal, hex, byte-offset), majority-vote, and
//!    regenerate the full `|Block#` sector.
//! 3. **P00000 ECC healing**: multiple `P00000` occurrences → split into 16-byte
//!    chunks, majority-vote the `pattern` and `patternpos` fields, regenerate the
//!    32× repeated pattern, then overwrite the single special bit from the
//!    upstream raw data.
//!
//! All other DATA areas and all non-DATA segments (ECC, SA, …) are passed through
//! unchanged. The output has the same page size, block size and total size as the
//! upstream provider.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use parking_lot::Mutex;

use crate::data_provider::DumpDataProvider;
use crate::error::Result;
use crate::types::FileMetadata;

// ─────────────────────────────────────────────────────────────────────────────
// Public statistics
// ─────────────────────────────────────────────────────────────────────────────

/// Counts of each healing action across all pages processed so far.
#[derive(Debug, Clone, Default)]
pub struct HealingStats {
    pub total_data_areas:   u64,
    pub healed_fill_00:     u64,
    pub healed_fill_ff:     u64,
    pub healed_fill_77:     u64,
    pub healed_lba:         u64,
    pub healed_p00000:      u64,
    pub untouched:          u64,
}

impl HealingStats {
    pub fn total_healed(&self) -> u64 {
        self.healed_fill_00
            + self.healed_fill_ff
            + self.healed_fill_77
            + self.healed_lba
            + self.healed_p00000
    }

    /// Human-readable summary suitable for a UI log panel.
    pub fn summary(&self) -> String {
        format!(
            "DATA areas processed : {}\n\
             Healed (fill 0x00)   : {}\n\
             Healed (fill 0xFF)   : {}\n\
             Healed (fill 0x77)   : {}\n\
             Healed (LBA |Block#) : {}\n\
             Healed (P00000 ECC)  : {}\n\
             Untouched            : {}\n\
             Total healed         : {}",
            self.total_data_areas,
            self.healed_fill_00,
            self.healed_fill_ff,
            self.healed_fill_77,
            self.healed_lba,
            self.healed_p00000,
            self.untouched,
            self.total_healed(),
        )
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// DATA-segment descriptor
// ─────────────────────────────────────────────────────────────────────────────

/// Byte range of one DATA segment within a page (half-open: `[start, end)`).
#[derive(Debug, Clone, Copy)]
pub struct DataSegment {
    pub start: u32,
    pub end:   u32,   // exclusive
}

impl DataSegment {
    pub fn len(self) -> u32 { self.end - self.start }
}

// ─────────────────────────────────────────────────────────────────────────────
// PatternHealingDataProvider
// ─────────────────────────────────────────────────────────────────────────────

pub struct PatternHealingDataProvider {
    upstream:      Arc<Mutex<dyn DumpDataProvider>>,
    /// DATA byte ranges within each page, derived from the page structure.
    data_segments: Vec<DataSegment>,
    metadata:      FileMetadata,
    identity:      String,
    /// Accumulated healing statistics (updated lazily as pages are read).
    pub stats:     Arc<Mutex<HealingStats>>,
}

impl PatternHealingDataProvider {
    /// Create a new `PatternHealingDataProvider`.
    ///
    /// # Arguments
    /// * `upstream`      – The provider to read raw pages from.
    /// * `data_segments` – Ordered DATA byte ranges within each page.
    ///   If empty *and* the upstream page length is exactly 512, the whole
    ///   page is treated as one DATA area (fallback mode).
    pub fn new(
        upstream: Arc<Mutex<dyn DumpDataProvider>>,
        data_segments: Vec<DataSegment>,
    ) -> Self {
        let (page_length, block_size, total_size, up_identity) = {
            let g = upstream.lock();
            let m = g.get_metadata();
            (m.page_length, m.block_size, m.size, g.cache_identity())
        };

        // If no DATA segments were provided but each page is exactly 512 bytes,
        // treat the whole page as one DATA segment.
        let effective_segs: Vec<DataSegment> = if data_segments.is_empty() && page_length == 512 {
            vec![DataSegment { start: 0, end: 512 }]
        } else {
            data_segments
        };

        let mut hasher = DefaultHasher::new();
        "PATTERN_HEALING".hash(&mut hasher);
        up_identity.hash(&mut hasher);
        for s in &effective_segs {
            s.start.hash(&mut hasher);
            s.end.hash(&mut hasher);
        }
        let identity = format!("healing_{:016x}", hasher.finish());

        let metadata = FileMetadata::new(
            format!("healing://{}", identity),
            total_size,
            page_length,
            block_size,
        );

        Self {
            upstream,
            data_segments: effective_segs,
            metadata,
            identity,
            stats: Arc::new(Mutex::new(HealingStats::default())),
        }
    }

    /// Clone of the current stats (cheap snapshot).
    pub fn stats_snapshot(&self) -> HealingStats {
        self.stats.lock().clone()
    }

    // ── Core: heal one 512-byte DATA area ────────────────────────────────────

    /// Given the raw 512-byte `data` slice and the same bytes from the upstream
    /// provider (needed to extract the special ECC bit), return a healed version
    /// and update `stats`. `data` is already a copy so we can mutate it freely.
    fn heal_512(data: &mut [u8; 512], raw_upstream: &[u8; 512], stats: &mut HealingStats) {
        stats.total_data_areas += 1;

        // ── 1. Fill detection ─────────────────────────────────────────────────
        if let Some(byte) = Self::detect_fill(data) {
            let fill = [byte; 512];
            data.copy_from_slice(&fill);
            match byte {
                0x00 => stats.healed_fill_00 += 1,
                0xFF => stats.healed_fill_ff += 1,
                0x77 => stats.healed_fill_77 += 1,
                _    => { /* shouldn't happen */ }
            }
            return;
        }

        // ── 2. LBA / |Block# healing ──────────────────────────────────────────
        if Self::looks_like_lba_sector(data) {
            if let Some(lba) = Self::extract_lba_majority(data) {
                Self::regenerate_lba_sector(data, lba);
                stats.healed_lba += 1;
                return;
            }
        }

        // ── 3. P00000 ECC pattern healing ─────────────────────────────────────
        if Self::looks_like_p00000_sector(data) {
            if let Some((pattern, patternpos)) = Self::majority_vote_p00000(data) {
                Self::regenerate_p00000_sector(data, raw_upstream, pattern, patternpos);
                stats.healed_p00000 += 1;
                return;
            }
        }

        stats.untouched += 1;
    }

    // ── Fill detection ────────────────────────────────────────────────────────

    /// Returns `Some(byte)` if >90% of the 512 bytes equal one of {0x00, 0xFF, 0x77}.
    fn detect_fill(data: &[u8; 512]) -> Option<u8> {
        let threshold = (512.0 * 0.90) as usize; // 460
        for candidate in [0x00u8, 0xFFu8, 0x77u8] {
            let count = data.iter().filter(|&&b| b == candidate).count();
            if count >= threshold {
                return Some(candidate);
            }
        }
        None
    }

    // ── LBA healing ───────────────────────────────────────────────────────────

    /// Returns `true` when the slice looks like an LBA / `|Block#` sector:
    /// more than 300 `x` (ASCII 0x78) characters in byte range [100, 510).
    fn looks_like_lba_sector(data: &[u8; 512]) -> bool {
        let x_count = data[100..510].iter().filter(|&&b| b == b'x').count();
        x_count > 300
    }

    /// Attempt to fix up to 3 bit errors in a decimal ASCII string by forcing
    /// each nibble into the range 0x30..=0x39. Mirrors `fixdecimal()` in
    /// `dumpextractrelevant.pl`.
    fn fix_decimal(bytes: &[u8]) -> Option<Vec<u8>> {
        let non_digit = bytes.iter().filter(|&&b| !(0x30..=0x39).contains(&b)).count();
        if non_digit == 0 {
            return Some(bytes.to_vec()); // already clean
        }
        if non_digit <= 3 {
            // Replace each non-digit nibble: keep upper nibble = 0x3_, lower nibble unchanged.
            let fixed: Vec<u8> = bytes.iter()
                .map(|&b| if (0x30..=0x39).contains(&b) { b } else { (b & 0x0F) | 0x30 })
                .collect();
            Some(fixed)
        } else {
            None // too many errors
        }
    }

    /// Parse decimal bytes (possibly with up to 3 correctable errors) → u64.
    fn parse_decimal_bytes(bytes: &[u8]) -> Option<u64> {
        let fixed = Self::fix_decimal(bytes)?;
        let s = std::str::from_utf8(&fixed).ok()?;
        s.parse::<u64>().ok()
    }

    /// Parse hex ASCII bytes → u64 (case-insensitive, 0–9, a–f, A–F only).
    fn parse_hex_bytes_to_u64(bytes: &[u8]) -> Option<u64> {
        let all_hex = bytes.iter().all(|&b| {
            (b'0'..=b'9').contains(&b)
                || (b'a'..=b'f').contains(&b)
                || (b'A'..=b'F').contains(&b)
        });
        if !all_hex { return None; }
        let s = std::str::from_utf8(bytes).ok()?;
        u64::from_str_radix(s, 16).ok()
    }

    /// Extract the LBA three ways and apply majority voting.
    ///
    /// Sector format (from `initpattern.pl`):
    /// ```text
    /// |Block#<D12> (0x<H8>) Byte: <B20> Pos: <P10> MB\n***xxx…x\n\x00
    ///  0      7    19 21   30   37   57  63   73   84
    /// ```
    /// Offsets (0-indexed within the 512-byte DATA area):
    ///   * `lba_decimal` : bytes  7..19  (12 decimal digits)
    ///   * `lba_hex`     : bytes 21..29  (8 hex digits, upper-case)
    ///   * `lba_byte`    : bytes 37..57  (20 decimal digits = LBA * 512)
    fn extract_lba_majority(data: &[u8; 512]) -> Option<u64> {
        // Verify leading "|Block#"
        if &data[0..7] != b"|Block#" {
            return None;
        }

        // Method D — 12 decimal digits at offset 7
        let lba_d: Option<u64> = Self::parse_decimal_bytes(&data[7..19]);

        // Method H — 8 hex digits at offset 21 (after " (0x")
        // Verify the " (0x" separator exists at offsets 19..23.
        let lba_h: Option<u64> = if &data[19..23] == b" (0x" {
            Self::parse_hex_bytes_to_u64(&data[23..31])
        } else {
            None
        };

        // Method B — 20 decimal digits at offset 37 represent LBA * 512
        // Verify ") Byte: " at offsets 31..38 (allow minor corruption).
        let lba_b: Option<u64> = Self::parse_decimal_bytes(&data[37..57])
            .map(|byte_addr| byte_addr / 512);

        // Majority voting: accept a value only if at least 2 out of 3 agree.
        let lba = match (lba_d, lba_h, lba_b) {
            (Some(d), Some(h), _) if d == h => Some(d),
            (Some(d), _, Some(b)) if d == b => Some(d),
            (_, Some(h), Some(b)) if h == b => Some(h),
            (Some(d), None, None) => Some(d), // only one source, accept it
            (None, Some(h), None) => Some(h),
            (None, None, Some(b)) => Some(b),
            _ => None,
        };

        lba
    }

    /// Regenerate the full 512-byte `|Block#` sector for the given LBA.
    ///
    /// Format from `initpattern.pl`:
    /// ```perl
    /// sprintf("|Block#%012d (0x%08X) Byte: %020d Pos: %10d MB\n***", lba, lba, lba*512, lba>>11)
    /// ```
    /// Then `x` fill to byte 509, then `\n\x00`.
    fn regenerate_lba_sector(data: &mut [u8; 512], lba: u64) {
        let pos_mb = lba >> 11; // lba * 512 / (1024*1024) = lba / 2048 = lba >> 11
        let header = format!(
            "|Block#{:012} (0x{:08X}) Byte: {:020} Pos: {:10} MB\n***",
            lba, lba, lba * 512, pos_mb,
        );
        let header_bytes = header.as_bytes();
        let header_len = header_bytes.len();

        // header_len should be exactly 75 bytes. Clamp defensively.
        let copy_len = header_len.min(510);
        data[..copy_len].copy_from_slice(&header_bytes[..copy_len]);

        // Fill bytes [header_len..509] with 'x', then '\n', then '\x00'
        if copy_len < 510 {
            for b in &mut data[copy_len..510] {
                *b = b'x';
            }
        }
        data[510] = b'\n';
        data[511] = 0x00;
    }

    // ── P00000 healing ────────────────────────────────────────────────────────

    /// Returns `true` when the 512-byte area looks like a P00000 ECC sector:
    /// it must contain at least 3 occurrences of the byte sequence `P00000`
    /// (0x50 0x30 0x30 0x30 0x30 0x30) which is the start of every 16-byte chunk
    /// whose `pattern` field begins with `0x0000_0` (very common for low-numbered
    /// ECC patterns).
    fn looks_like_p00000_sector(data: &[u8; 512]) -> bool {
        let needle = b"P00000";
        let mut count = 0usize;
        let mut i = 0;
        while i + needle.len() <= data.len() {
            if data[i..i + needle.len()] == *needle {
                count += 1;
                if count >= 3 {
                    return true;
                }
                i += needle.len();
            } else {
                i += 1;
            }
        }
        false
    }

    /// Parse one 16-byte ECC chunk: `P<11 uppercase hex><4 uppercase hex>`.
    ///
    /// Returns `(pattern, patternpos)` or `None` on parse failure.
    fn parse_p00000_chunk(chunk: &[u8]) -> Option<(u64, u64)> {
        if chunk.len() < 16 { return None; }
        if chunk[0] != b'P' { return None; }
        let pattern    = Self::parse_hex_bytes_to_u64(&chunk[1..12])?;
        let patternpos = Self::parse_hex_bytes_to_u64(&chunk[12..16])?;
        Some((pattern, patternpos))
    }

    /// Split the 512-byte area into 32 × 16-byte chunks, try to decode each,
    /// and return the majority-voted `(pattern, patternpos)`.
    fn majority_vote_p00000(data: &[u8; 512]) -> Option<(u64, u64)> {
        use std::collections::HashMap;

        let mut votes_pattern:    HashMap<u64, usize> = HashMap::new();
        let mut votes_patternpos: HashMap<u64, usize> = HashMap::new();
        let mut decoded = 0usize;

        for chunk_idx in 0..32 {
            let start = chunk_idx * 16;
            if let Some((pat, pos)) = Self::parse_p00000_chunk(&data[start..start + 16]) {
                *votes_pattern.entry(pat).or_insert(0) += 1;
                *votes_patternpos.entry(pos).or_insert(0) += 1;
                decoded += 1;
            }
        }

        if decoded < 3 {
            return None; // Not enough readable chunks
        }

        // Pick the most-voted value for each field
        let pattern    = votes_pattern.into_iter().max_by_key(|&(_, c)| c)?.0;
        let patternpos = votes_patternpos.into_iter().max_by_key(|&(_, c)| c)?.0;

        Some((pattern, patternpos))
    }

    /// Regenerate the 512-byte ECC sector from `pattern` and `patternpos`,
    /// then overwrite the single "special bit" with the corresponding bit
    /// from `raw_upstream` (the unmodified bytes read from the upstream provider
    /// for this same DATA area).
    ///
    /// Algorithm from `initpattern.pl`:
    ///
    /// ```ignore
    /// sector = "P<pattern:011X><patternpos:04X>" repeated 32 times
    /// bittargetbyte = (pattern >> 3) & 0x1FF        // byte index 0..511
    /// bittargetbit  =  pattern & 7                   // bit  index 0..7
    /// sector[bittargetbyte] bit bittargetbit =
    ///     raw_upstream[bittargetbyte] bit bittargetbit
    /// ```
    fn regenerate_p00000_sector(
        data:         &mut [u8; 512],
        raw_upstream: &[u8; 512],
        pattern:      u64,
        patternpos:   u64,
    ) {
        // Build the repeated 16-byte chunk.
        let chunk = format!("P{:011X}{:04X}", pattern, patternpos);
        let chunk_bytes = chunk.as_bytes();

        // Fill all 32 chunks (32 × 16 = 512 bytes).
        for i in 0..32 {
            let start = i * 16;
            // chunk_bytes is exactly 16 bytes; copy defensively
            let copy_len = chunk_bytes.len().min(16);
            data[start..start + copy_len].copy_from_slice(&chunk_bytes[..copy_len]);
        }

        // Overwrite the special bit from the upstream raw data.
        let bittargetbyte = ((pattern >> 3) & 0x1FF) as usize;  // 0..511
        let bittargetbit  = (pattern & 7) as u32;                // 0..7
        let mask: u8 = 1 << bittargetbit;

        // Take the bit value from the original (potentially damaged) upstream
        // data and transplant it into the regenerated sector.
        let raw_bit = raw_upstream[bittargetbyte] & mask;
        data[bittargetbyte] = (data[bittargetbyte] & !mask) | raw_bit;
    }

    // ── Page-level healing ────────────────────────────────────────────────────

    /// Heal all DATA segments within a single page buffer of `page_len` bytes.
    ///
    /// `page_buf` is modified in-place. Non-DATA bytes are left unchanged.
    fn heal_page(
        page_buf:  &mut Vec<u8>,
        raw_page:  &[u8],
        data_segs: &[DataSegment],
        page_len:  usize,
        stats:     &mut HealingStats,
    ) {
        for seg in data_segs {
            let seg_start = seg.start as usize;
            let seg_end   = seg.end   as usize;

            // Iterate over 512-byte sub-blocks within this DATA segment.
            let mut off = seg_start;
            while off + 512 <= seg_end && off + 512 <= page_len {
                // Copy 512 bytes from page_buf into a fixed-size array so we can
                // call heal_512 (which requires [u8; 512]).
                let mut block = [0u8; 512];
                block.copy_from_slice(&page_buf[off..off + 512]);

                let mut raw_block = [0u8; 512];
                if off + 512 <= raw_page.len() {
                    raw_block.copy_from_slice(&raw_page[off..off + 512]);
                }

                Self::heal_512(&mut block, &raw_block, stats);
                page_buf[off..off + 512].copy_from_slice(&block);

                off += 512;
            }
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// DumpDataProvider impl
// ─────────────────────────────────────────────────────────────────────────────

impl DumpDataProvider for PatternHealingDataProvider {
    fn read_bytes(&mut self, offset: u64, length: u32) -> Result<Vec<u8>> {
        if length == 0 {
            return Ok(Vec::new());
        }

        // Read the raw bytes from upstream first.
        let raw = self.upstream.lock().read_bytes(offset, length)?;

        let page_len = self.metadata.page_length as usize;
        if page_len == 0 || self.data_segments.is_empty() {
            // Nothing to heal; pass through.
            return Ok(raw);
        }

        let total_size = self.metadata.size;

        // Work on a mutable copy.
        let mut buf = raw.clone();

        // Determine which pages overlap with the requested byte range.
        let start_page = offset / page_len as u64;
        let end_byte   = (offset + length as u64).min(total_size);
        let end_page   = if end_byte == 0 { 0 } else { (end_byte - 1) / page_len as u64 };

        let mut local_stats = HealingStats::default();

        for page_idx in start_page..=end_page {
            let page_byte_start = page_idx * page_len as u64;
            let page_byte_end   = page_byte_start + page_len as u64;

            // Overlap of this page with the requested [offset, offset+length) window.
            let overlap_start = offset.max(page_byte_start);
            let overlap_end   = end_byte.min(page_byte_end);
            if overlap_end <= overlap_start {
                continue;
            }

            // We can only heal a page if we have its complete bytes.
            // If the request only covers part of this page we still need the
            // full page, so we fetch it separately from upstream.
            let full_page_raw: Vec<u8> = if overlap_start == page_byte_start
                && overlap_end == page_byte_end
                && (overlap_start - offset) as usize + page_len <= buf.len()
            {
                // The full page is already inside `buf` — extract it.
                let buf_start = (overlap_start - offset) as usize;
                buf[buf_start..buf_start + page_len].to_vec()
            } else {
                // Need to fetch the full page from upstream.
                self.upstream.lock().read_bytes(page_byte_start, page_len as u32)?
            };

            // Clone for healing (we need raw for the P00000 bit extraction).
            let mut healed_page = full_page_raw.clone();

            Self::heal_page(
                &mut healed_page,
                &full_page_raw,
                &self.data_segments,
                page_len,
                &mut local_stats,
            );

            // Write the healed bytes back into `buf` for the overlapping segment.
            let buf_write_start = (overlap_start - offset) as usize;
            let in_page_start   = (overlap_start - page_byte_start) as usize;
            let seg_len         = (overlap_end - overlap_start) as usize;

            buf[buf_write_start..buf_write_start + seg_len]
                .copy_from_slice(&healed_page[in_page_start..in_page_start + seg_len]);
        }

        // Merge local stats into the shared counter.
        {
            let mut s = self.stats.lock();
            s.total_data_areas += local_stats.total_data_areas;
            s.healed_fill_00   += local_stats.healed_fill_00;
            s.healed_fill_ff   += local_stats.healed_fill_ff;
            s.healed_fill_77   += local_stats.healed_fill_77;
            s.healed_lba       += local_stats.healed_lba;
            s.healed_p00000    += local_stats.healed_p00000;
            s.untouched        += local_stats.untouched;
        }

        Ok(buf)
    }

    fn get_metadata(&self) -> FileMetadata {
        self.metadata.clone()
    }

    fn cache_identity(&self) -> String {
        self.identity.clone()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Unit tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data_provider::FileDataProvider;
    use crate::file_loader::FileLoader;
    use std::io::Write;
    use tempfile::NamedTempFile;

    fn make_provider(data: Vec<u8>, page_len: u32) -> Arc<Mutex<dyn DumpDataProvider>> {
        let mut tmp = NamedTempFile::new().unwrap();
        tmp.write_all(&data).unwrap();
        tmp.flush().unwrap();
        let loader = FileLoader::new(tmp.path(), page_len, 64).unwrap();
        Arc::new(Mutex::new(FileDataProvider::new(loader)))
    }

    // ── fill detection ────────────────────────────────────────────────────────

    #[test]
    fn test_detect_fill_00() {
        let mut d = [0x00u8; 512];
        assert_eq!(PatternHealingDataProvider::detect_fill(&d), Some(0x00));
        // Sprinkle 20 non-zero bytes — still >90%
        for i in 0..20 { d[i] = 0xAB; }
        assert_eq!(PatternHealingDataProvider::detect_fill(&d), Some(0x00));
        // 60 non-zero → <90%, should be None
        for i in 0..60 { d[i] = 0xAB; }
        assert_eq!(PatternHealingDataProvider::detect_fill(&d), None);
    }

    #[test]
    fn test_detect_fill_ff() {
        let d = [0xFFu8; 512];
        assert_eq!(PatternHealingDataProvider::detect_fill(&d), Some(0xFF));
    }

    #[test]
    fn test_detect_fill_77() {
        let d = [0x77u8; 512];
        assert_eq!(PatternHealingDataProvider::detect_fill(&d), Some(0x77));
    }

    // ── fix_decimal ───────────────────────────────────────────────────────────

    #[test]
    fn test_fix_decimal_clean() {
        let r = PatternHealingDataProvider::fix_decimal(b"012345678901");
        assert_eq!(r, Some(b"012345678901".to_vec()));
    }

    #[test]
    fn test_fix_decimal_one_error() {
        // 'v' (0x76) → lower nibble 6, upper forced to 0x30 → '6'
        let r = PatternHealingDataProvider::fix_decimal(b"012345v789");
        assert!(r.is_some());
        let fixed = r.unwrap();
        // All bytes must be valid digit ASCII
        assert!(fixed.iter().all(|&b| (0x30..=0x39).contains(&b)));
    }

    #[test]
    fn test_fix_decimal_too_many_errors() {
        let r = PatternHealingDataProvider::fix_decimal(b"xxxxxxxxxxxx"); // 12 errors
        assert_eq!(r, None);
    }

    // ── LBA extraction ────────────────────────────────────────────────────────

    fn make_lba_sector(lba: u64) -> [u8; 512] {
        let mut data = [b'x'; 512];
        let header = format!(
            "|Block#{:012} (0x{:08X}) Byte: {:020} Pos: {:10} MB\n***",
            lba, lba, lba * 512, lba >> 11,
        );
        let hb = header.as_bytes();
        let copy = hb.len().min(510);
        data[..copy].copy_from_slice(&hb[..copy]);
        data[510] = b'\n';
        data[511] = 0x00;
        data
    }

    #[test]
    fn test_looks_like_lba_sector() {
        let data = make_lba_sector(12345);
        assert!(PatternHealingDataProvider::looks_like_lba_sector(&data));
    }

    #[test]
    fn test_extract_lba_majority_clean() {
        let data = make_lba_sector(99999);
        let lba = PatternHealingDataProvider::extract_lba_majority(&data);
        assert_eq!(lba, Some(99999));
    }

    #[test]
    fn test_lba_regenerate_roundtrip() {
        let lba = 1_000_000u64;
        let original = make_lba_sector(lba);
        // Corrupt the decimal field slightly
        let mut corrupted = original;
        corrupted[8] = b'?';  // one digit error
        let extracted = PatternHealingDataProvider::extract_lba_majority(&corrupted);
        // Hex and byte fields still clean → majority gives correct LBA
        assert_eq!(extracted, Some(lba));
    }

    #[test]
    fn test_regenerate_lba_sector() {
        let lba = 42u64;
        let expected = make_lba_sector(lba);
        let mut data = [0u8; 512];
        PatternHealingDataProvider::regenerate_lba_sector(&mut data, lba);
        assert_eq!(data, expected);
    }

    // ── P00000 healing ────────────────────────────────────────────────────────

    fn make_p00000_sector(pattern: u64, patternpos: u64) -> [u8; 512] {
        let chunk = format!("P{:011X}{:04X}", pattern, patternpos);
        let cb = chunk.as_bytes();
        let mut data = [0u8; 512];
        for i in 0..32 {
            data[i * 16..i * 16 + 16].copy_from_slice(cb);
        }
        data
    }

    #[test]
    fn test_looks_like_p00000_sector() {
        let d = make_p00000_sector(0, 1);
        assert!(PatternHealingDataProvider::looks_like_p00000_sector(&d));
    }

    #[test]
    fn test_majority_vote_p00000_clean() {
        let data = make_p00000_sector(0xABC, 0x0001);
        let result = PatternHealingDataProvider::majority_vote_p00000(&data);
        assert_eq!(result, Some((0xABC, 0x0001)));
    }

    #[test]
    fn test_majority_vote_p00000_with_noise() {
        let mut data = make_p00000_sector(0x1FF, 0x0001);
        // Corrupt 4 chunks completely with garbage
        for i in 0..4 {
            let start = i * 16;
            data[start..start + 16].fill(0xCC);
        }
        let result = PatternHealingDataProvider::majority_vote_p00000(&data);
        assert_eq!(result, Some((0x1FF, 0x0001)));
    }

    #[test]
    fn test_p00000_bit_preservation() {
        let pattern: u64 = 7; // bittargetbyte = (7>>3)&0x1FF = 0, bittargetbit = 7
        let patternpos: u64 = 1;

        let mut sector = make_p00000_sector(pattern, patternpos);

        // Create upstream data where byte 0 bit 7 is set
        let mut raw = sector;
        raw[0] |= 0x80; // set bit 7

        PatternHealingDataProvider::regenerate_p00000_sector(
            &mut sector, &raw, pattern, patternpos
        );

        // After regeneration, bit 7 of byte 0 must match raw_upstream
        assert_eq!(sector[0] & 0x80, raw[0] & 0x80);
    }

    #[test]
    fn test_p00000_bit_zero_preserved() {
        let pattern: u64 = 7;
        let patternpos: u64 = 1;

        let mut sector = make_p00000_sector(pattern, patternpos);

        // Create upstream data where byte 0 bit 7 is clear
        let mut raw = sector;
        raw[0] &= !0x80;

        PatternHealingDataProvider::regenerate_p00000_sector(
            &mut sector, &raw, pattern, patternpos
        );

        assert_eq!(sector[0] & 0x80, 0x00);
    }

    // ── Integration: provider passthrough ─────────────────────────────────────

    #[test]
    fn test_provider_fill_healing_00() {
        // Page = 512 bytes; all 0x00 → healed to all 0x00 (same bytes, but counted)
        let data = vec![0x00u8; 512];
        let upstream = make_provider(data, 512);
        let segs = vec![DataSegment { start: 0, end: 512 }];
        let mut provider = PatternHealingDataProvider::new(upstream, segs);

        let bytes = provider.read_bytes(0, 512).unwrap();
        assert_eq!(bytes, vec![0x00u8; 512]);

        let stats = provider.stats_snapshot();
        assert_eq!(stats.healed_fill_00, 1);
    }

    #[test]
    fn test_provider_lba_healing() {
        let lba = 55u64;
        let sector = make_lba_sector(lba);
        // Corrupt the decimal field
        let mut corrupted = sector.to_vec();
        corrupted[10] = b'?';

        let upstream = make_provider(corrupted, 512);
        let segs = vec![DataSegment { start: 0, end: 512 }];
        let mut provider = PatternHealingDataProvider::new(upstream, segs);

        let healed = provider.read_bytes(0, 512).unwrap();
        let expected = make_lba_sector(lba);
        assert_eq!(&healed[..7], b"|Block#");
        // Regenerated sector should match expected up to the header
        assert_eq!(&healed[..31], &expected[..31]);

        let stats = provider.stats_snapshot();
        assert_eq!(stats.healed_lba, 1);
    }

    #[test]
    fn test_provider_non_data_bytes_untouched() {
        // Page = 1024 bytes: bytes 0..512 are DATA (fill 0x00), bytes 512..1024 are ECC (0xAB)
        let mut data = vec![0x00u8; 1024];
        for b in &mut data[512..] { *b = 0xAB; }

        let upstream = make_provider(data.clone(), 1024);
        let segs = vec![DataSegment { start: 0, end: 512 }];
        let mut provider = PatternHealingDataProvider::new(upstream, segs);

        let result = provider.read_bytes(0, 1024).unwrap();
        // DATA part healed (fill 0x00 → same)
        assert!(result[..512].iter().all(|&b| b == 0x00));
        // ECC part unchanged
        assert!(result[512..].iter().all(|&b| b == 0xAB));
    }

    #[test]
    fn test_provider_p00000_healing() {
        let pattern: u64 = 100;
        let patternpos: u64 = 1;
        let sector = make_p00000_sector(pattern, patternpos);
        let upstream = make_provider(sector.to_vec(), 512);
        let segs = vec![DataSegment { start: 0, end: 512 }];
        let mut provider = PatternHealingDataProvider::new(upstream, segs);

        let result = provider.read_bytes(0, 512).unwrap();
        // Starts with "P"
        assert_eq!(result[0], b'P');

        let stats = provider.stats_snapshot();
        assert_eq!(stats.healed_p00000, 1);
    }

    #[test]
    fn test_provider_metadata_matches_upstream() {
        let data = vec![0xAAu8; 4096];
        let upstream = make_provider(data, 512);
        let segs = vec![DataSegment { start: 0, end: 512 }];
        let provider = PatternHealingDataProvider::new(upstream, segs);

        let meta = provider.get_metadata();
        assert_eq!(meta.size, 4096);
        assert_eq!(meta.page_length, 512);
        assert_eq!(meta.total_pages, 8);
    }

    #[test]
    fn test_fallback_whole_page_when_no_segments() {
        // No data segments + 512-byte pages → fallback to treating whole page as DATA
        let data = vec![0xFFu8; 512];
        let upstream = make_provider(data, 512);
        let mut provider = PatternHealingDataProvider::new(upstream, vec![]);

        let result = provider.read_bytes(0, 512).unwrap();
        assert_eq!(result, vec![0xFFu8; 512]);

        let stats = provider.stats_snapshot();
        assert_eq!(stats.healed_fill_ff, 1);
    }
}
