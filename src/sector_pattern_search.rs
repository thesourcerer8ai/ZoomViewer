//! Sector Pattern Search
//!
//! Searches every 512-byte DATA sub-sector for one of 9 precomputed 6-byte
//! probe signatures, covering meaningful XOR combinations of three active base
//! NAND sector types:
//!
//!   |Block  — LBA sector (bytes 80–509 are ASCII 'x' = 0x78)
//!   P00000  — ECC sector (32 × 16-byte "P00000…" chunks)
//!   Fill77  — blank sector, all 0x77
//!
//! Fill00 (all 0x00) and FillFF (all 0xFF) singles are intentionally excluded
//! because they produce too many false positives on real NAND dumps.
//! Fill00 XOR pairs are also omitted (XOR with zero is the identity).
//!
//! Because all three active patterns produce fixed, predictable bytes in the
//! x-fill region (offsets 80–496, 16-byte aligned), XORing any two patterns
//! yields the same 6-byte sequence at every probe position.  These sequences
//! are precomputed once; the hot loop is pure 6-byte equality comparisons.
//!
//! A sector matches as soon as ANY single probe matches; the search skips to
//! the next sector immediately, reporting only the sector's start offset.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::Sender;

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

use crate::data_provider::DumpDataProvider;
use crate::pattern_healing::DataSegment;
use crate::search::{export_results_to_file, SearchResult};

// ─────────────────────────────────────────────────────────────────────────────
// Constants
// ─────────────────────────────────────────────────────────────────────────────

pub const SECTOR_SIZE: usize = 512;
pub const PROBE_LEN: usize = 6;

/// 16-byte-aligned probe offsets inside the x-fill region (80–496 inclusive).
/// 27 positions: 80, 96, 112, …, 496.
pub const PROBE_OFFSETS: &[usize] = &[
     80,  96, 112, 128, 144, 160, 176, 192, 208, 224, 240, 256,
    272, 288, 304, 320, 336, 352, 368, 384, 400, 416, 432, 448,
    464, 480, 496,
];

// ─────────────────────────────────────────────────────────────────────────────
// Base pattern bytes at probe positions (same at every probe offset)
//
//  |Block  → 0x78 ('x'), all 6 bytes
//  P00000  → 0x50 0x30 0x30 0x30 0x30 0x30  ("P00000", first 6 of each chunk)
//  Fill00  → 0x00 × 6
//  Fill77  → 0x77 × 6
//  FillFF  → 0xFF × 6
// ─────────────────────────────────────────────────────────────────────────────

const BLOCK:  [u8; 6] = [0x78, 0x78, 0x78, 0x78, 0x78, 0x78];
const P00000: [u8; 6] = [0x50, 0x30, 0x30, 0x30, 0x30, 0x30];
//const F00:    [u8; 6] = [0x00, 0x00, 0x00, 0x00, 0x00, 0x00];
const F77:    [u8; 6] = [0x77, 0x77, 0x77, 0x77, 0x77, 0x77];
const FFF:    [u8; 6] = [0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF];

const fn xor6(a: [u8; 6], b: [u8; 6]) -> [u8; 6] {
    [
        a[0] ^ b[0],
        a[1] ^ b[1],
        a[2] ^ b[2],
        a[3] ^ b[3],
        a[4] ^ b[4],
        a[5] ^ b[5],
    ]
}

// ─────────────────────────────────────────────────────────────────────────────
// SectorTarget
// ─────────────────────────────────────────────────────────────────────────────

/// One precomputed search target: the 6 bytes expected at every probe offset
/// inside a matching sector.
#[derive(Debug, Clone)]
pub struct SectorTarget {
    pub name:  &'static str,
    pub probe: [u8; 6],
}

/// All 9 non-degenerate targets.
///
/// Fill00 and FillFF singles are excluded (too many false positives on real
/// NAND dumps).  Fill00 XOR pairs are also omitted because XOR with zero is
/// the identity — they would just duplicate the other singles.
pub fn all_targets() -> Vec<SectorTarget> {
    vec![
        // 3 singles (Fill 0x00 and Fill 0xFF disabled — false positives)
        SectorTarget { name: "|Block",           probe: BLOCK              },
        SectorTarget { name: "P00000",           probe: P00000             },
        //SectorTarget { name: "Fill 0x00",        probe: F00                },
        SectorTarget { name: "Fill 0x77",        probe: F77                },
        //SectorTarget { name: "Fill 0xFF",        probe: FFF                },
        // 6 meaningful XOR pairs (excluding Fill00 pairs = identity)
        SectorTarget { name: "|Block XOR P00000",        probe: xor6(BLOCK, P00000) },
        SectorTarget { name: "|Block XOR Fill 0x77",     probe: xor6(BLOCK, F77)   },
        SectorTarget { name: "|Block XOR Fill 0xFF",     probe: xor6(BLOCK, FFF)   },
        SectorTarget { name: "P00000 XOR Fill 0x77",     probe: xor6(P00000, F77)  },
        SectorTarget { name: "P00000 XOR Fill 0xFF",     probe: xor6(P00000, FFF)  },
        SectorTarget { name: "Fill 0x77 XOR Fill 0xFF",  probe: xor6(F77, FFF)     },
    ]
}

// ─────────────────────────────────────────────────────────────────────────────
// Match result
// ─────────────────────────────────────────────────────────────────────────────

/// A single matching 512-byte sector found during a sector pattern search.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SectorPatternMatch {
    /// 1-based index.
    pub index: usize,
    /// Absolute byte offset of the 512-byte sector's start in the stream.
    pub byte_offset: u64,
    /// Block index.
    pub block: u64,
    /// Page index within the block.
    pub page: u64,
    /// Byte offset of the sector start within the page.
    pub offset_in_page: u64,
    /// Hex representation of the 6 matched probe bytes.
    pub preview_hex: String,
    /// Name of the matching target (e.g. "|Block XOR P00000").
    pub target_name: String,
}

// ─────────────────────────────────────────────────────────────────────────────
// Search engine
// ─────────────────────────────────────────────────────────────────────────────

/// Search every 512-byte DATA sub-sector in `provider` for any of the 11
/// precomputed targets.
///
/// # Arguments
/// * `provider`       – Upstream data provider.
/// * `data_segments`  – DATA byte ranges within each page (from PageStructureTabState).
///                      If empty and page_length == 512, the whole page is one sector.
/// * `cancel_flag`    – Set to `true` to abort the search.
/// * `progress_bytes` – Updated with the number of bytes scanned so far.
/// * `max_matches`    – Stop after this many matches (0 = unlimited).
/// * `match_tx`       – Optional channel for live streaming of matches.
pub fn search_sector_patterns(
    provider:       Arc<Mutex<dyn DumpDataProvider>>,
    data_segments:  Vec<DataSegment>,
    cancel_flag:    Arc<AtomicBool>,
    progress_bytes: Arc<AtomicU64>,
    max_matches:    usize,
    match_tx:       Option<Sender<SectorPatternMatch>>,
) -> Result<Vec<SectorPatternMatch>, String> {
    let targets = all_targets();

    let (page_length, total_size, block_stride) = {
        let g = provider.lock();
        let m = g.get_metadata();
        let bs = m.block_size as u64;
        let pl = m.page_length as u64;
        (pl, m.size, bs * pl)
    };

    if page_length == 0 {
        return Err("Provider has zero page_length".to_string());
    }

    // If no DATA segments were provided but each page is exactly 512 bytes,
    // treat the whole page as one DATA area (same fallback as PatternHealing).
    let effective_segs: Vec<DataSegment> = if data_segments.is_empty() && page_length == 512 {
        vec![DataSegment { start: 0, end: 512 }]
    } else {
        data_segments
    };

    if effective_segs.is_empty() {
        return Err(
            "No DATA segments defined. Please configure the Page Structure tab.".to_string(),
        );
    }

    let mut results: Vec<SectorPatternMatch> = Vec::new();
    let total_pages = total_size / page_length;

    // Read pages one at a time.  256 KB chunk reads balance lock contention
    // and memory use; we still need full page alignment for segment iteration.
    let chunk_pages: u64 = ((256 * 1024) / page_length).max(1);

    let mut page_idx: u64 = 0;
    while page_idx < total_pages {
        if cancel_flag.load(Ordering::Relaxed) {
            break;
        }

        let batch = chunk_pages.min(total_pages - page_idx);
        let read_offset = page_idx * page_length;
        let read_len = (batch * page_length) as u32;

        let chunk = provider
            .lock()
            .read_bytes(read_offset, read_len)
            .map_err(|e| format!("Read failed at offset {}: {}", read_offset, e))?;

        // Walk pages within the chunk.
        for local_page in 0..batch {
            if cancel_flag.load(Ordering::Relaxed) {
                break;
            }

            let global_page = page_idx + local_page;
            let page_start_in_chunk = (local_page * page_length) as usize;
            let page_end_in_chunk = page_start_in_chunk + page_length as usize;
            if page_end_in_chunk > chunk.len() {
                break;
            }
            let page_bytes = &chunk[page_start_in_chunk..page_end_in_chunk];

            // Walk DATA segments within the page.
            for seg in &effective_segs {
                let seg_start = seg.start as usize;
                let seg_end   = seg.end   as usize;
                if seg_end > page_length as usize { continue; }

                // Walk 512-byte sub-sectors within the DATA segment.
                let mut off = seg_start;
                while off + SECTOR_SIZE <= seg_end {
                    let sector = &page_bytes[off..off + SECTOR_SIZE];

                    // Check all probe positions until the first match.
                    'sector: for &probe_off in PROBE_OFFSETS {
                        if probe_off + PROBE_LEN > SECTOR_SIZE { break; }
                        let window = &sector[probe_off..probe_off + PROBE_LEN];

                        for target in &targets {
                            if window == target.probe {
                                // Hit — record the sector's absolute start offset.
                                let abs_sector_start =
                                    (global_page * page_length) + off as u64;

                                let block = if block_stride > 0 {
                                    abs_sector_start / block_stride
                                } else {
                                    0
                                };
                                let offset_in_page = off as u64;

                                // Build a hex preview of the 6 matched bytes.
                                let preview_hex = window
                                    .iter()
                                    .map(|b| format!("{:02X}", b))
                                    .collect::<Vec<_>>()
                                    .join(" ");

                                let m = SectorPatternMatch {
                                    index:          results.len() + 1,
                                    byte_offset:    abs_sector_start,
                                    block,
                                    page:           global_page,
                                    offset_in_page,
                                    preview_hex,
                                    target_name:    target.name.to_string(),
                                };

                                if let Some(ref tx) = match_tx {
                                    let _ = tx.send(m.clone());
                                }
                                results.push(m);

                                if max_matches > 0 && results.len() >= max_matches {
                                    progress_bytes.store(
                                        (global_page + 1) * page_length,
                                        Ordering::Relaxed,
                                    );
                                    return Ok(results);
                                }

                                break 'sector; // skip remaining probes for this sector
                            }
                        }
                    }

                    off += SECTOR_SIZE;
                }
            }
        }

        page_idx += batch;
        progress_bytes.store(page_idx * page_length, Ordering::Relaxed);
    }

    progress_bytes.store(total_size, Ordering::Relaxed);
    Ok(results)
}

// ─────────────────────────────────────────────────────────────────────────────
// Log writer — same format as the Search tab
// ─────────────────────────────────────────────────────────────────────────────

/// Write `matches` to `path` using the same format as the Search tab.
///
/// The file format is detected from the path extension:
/// * `.csv` → CSV with header row
/// * anything else → fixed-width text table
///
/// The "Preview" / `AsciiPreview` column contains the target name
/// (e.g. `|Block XOR P00000`) rather than ASCII text, which is
/// more useful for structural sector search results.
pub fn write_log_file<P: AsRef<Path>>(
    path:        P,
    matches:     &[SectorPatternMatch],
    page_length: u32,
    block_size:  u32,
) -> std::io::Result<()> {
    let is_csv = path
        .as_ref()
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.eq_ignore_ascii_case("csv"))
        .unwrap_or(false);

    // Convert SectorPatternMatch → SearchResult so we can reuse
    // export_results_to_file from search.rs.
    let converted: Vec<SearchResult> = matches
        .iter()
        .map(|m| SearchResult {
            index:          m.index,
            byte_offset:    m.byte_offset,
            block:          m.block,
            page:           m.page,
            offset_in_page: m.offset_in_page,
            preview_hex:    m.preview_hex.clone(),
            // "Preview" column shows the matched target name.
            preview_ascii:  m.target_name.clone(),
        })
        .collect();

    export_results_to_file(&converted, path, is_csv, page_length, block_size)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probe_vectors_correct() {
        // |Block single: all 0x78
        let t = all_targets();
        let block_t = t.iter().find(|t| t.name == "|Block").unwrap();
        assert_eq!(block_t.probe, [0x78; 6]);

        // P00000 single: P 0 0 0 0 0
        let p_t = t.iter().find(|t| t.name == "P00000").unwrap();
        assert_eq!(p_t.probe, [0x50, 0x30, 0x30, 0x30, 0x30, 0x30]);

        // |Block XOR P00000: 0x78^0x50 = 0x28, then 0x78^0x30 = 0x48 × 5
        let xor_t = t.iter().find(|t| t.name == "|Block XOR P00000").unwrap();
        assert_eq!(xor_t.probe, [0x28, 0x48, 0x48, 0x48, 0x48, 0x48]);

        // |Block XOR Fill 0x77: 0x78^0x77 = 0x0F × 6
        let xor77 = t.iter().find(|t| t.name == "|Block XOR Fill 0x77").unwrap();
        assert_eq!(xor77.probe, [0x0F; 6]);

        // Fill 0x77 XOR Fill 0xFF: 0x77^0xFF = 0x88 × 6
        let x77ff = t.iter().find(|t| t.name == "Fill 0x77 XOR Fill 0xFF").unwrap();
        assert_eq!(x77ff.probe, [0x88; 6]);
    }

    #[test]
    fn probe_offsets_count() {
        // Expect exactly 27 positions: (496-80)/16 + 1 = 27
        assert_eq!(PROBE_OFFSETS.len(), 27);
        assert_eq!(PROBE_OFFSETS[0], 80);
        assert_eq!(*PROBE_OFFSETS.last().unwrap(), 496);
    }

    #[test]
    fn all_targets_count() {
        // 3 singles (Fill 0x00 and Fill 0xFF omitted — too many false positives)
        // + 6 XOR pairs = 9
        assert_eq!(all_targets().len(), 9);
    }

    #[test]
    fn no_duplicate_probes() {
        // Every target must have a unique probe vector so matches are unambiguous.
        let targets = all_targets();
        let mut seen: Vec<[u8; 6]> = Vec::new();
        for t in &targets {
            assert!(
                !seen.contains(&t.probe),
                "Duplicate probe for target '{}'",
                t.name
            );
            seen.push(t.probe);
        }
    }
}
