//! Background Search Engine for NAND dump providers
//!
//! Provides fast, chunked pattern searching (ASCII and HEX) across dynamic
//! DumpDataProviders with boundary overlap, progress reporting, cancellation,
//! and result exporting.

use crate::data_provider::DumpDataProvider;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::fs::File;
use std::io::Write;
use std::path::Path;
use std::sync::mpsc::Sender;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

/// A single match found in the dump
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchResult {
    /// 1-based match index
    pub index: usize,
    /// Absolute byte offset in the dump
    pub byte_offset: u64,
    /// Block index
    pub block: u64,
    /// Page index within block
    pub page: u64,
    /// Byte offset within page
    pub offset_in_page: u64,
    /// Hex preview of snippet around match
    pub preview_hex: String,
    /// Printable ASCII preview of snippet around match
    pub preview_ascii: String,
}

/// Search format mode
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SearchMode {
    Ascii,
    Hex,
}

/// Search configuration options
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchOptions {
    pub pattern: String,
    pub mode: SearchMode,
    pub case_sensitive: bool,
    pub max_matches: usize,
}

impl Default for SearchOptions {
    fn default() -> Self {
        Self {
            pattern: String::new(),
            mode: SearchMode::Ascii,
            case_sensitive: true,
            max_matches: 1000,
        }
    }
}

/// Parse search pattern into raw search bytes
pub fn parse_pattern(options: &SearchOptions) -> Result<Vec<u8>, String> {
    let trimmed = options.pattern.trim();
    if trimmed.is_empty() {
        return Err("Search pattern is empty".to_string());
    }

    match options.mode {
        SearchMode::Ascii => {
            let bytes = if options.case_sensitive {
                trimmed.as_bytes().to_vec()
            } else {
                trimmed.to_lowercase().into_bytes()
            };
            Ok(bytes)
        }
        SearchMode::Hex => {
            // Strip spaces, dashes, colons, and '0x' prefixes
            let cleaned: String = trimmed
                .chars()
                .filter(|c| !c.is_whitespace() && *c != '-' && *c != ':' && *c != ',')
                .collect();
            let hex_str = cleaned.strip_prefix("0x").unwrap_or(&cleaned);

            if hex_str.len() % 2 != 0 {
                return Err("Hex pattern must contain an even number of characters".to_string());
            }

            let mut bytes = Vec::with_capacity(hex_str.len() / 2);
            for i in (0..hex_str.len()).step_by(2) {
                let byte_str = &hex_str[i..i + 2];
                let byte = u8::from_str_radix(byte_str, 16)
                    .map_err(|_| format!("Invalid hex byte: '{}'", byte_str))?;
                bytes.push(byte);
            }

            if bytes.is_empty() {
                return Err("Hex pattern produced 0 bytes".to_string());
            }

            Ok(bytes)
        }
    }
}

/// Helper to generate ASCII and Hex previews for a slice of data
fn generate_previews(snippet: &[u8]) -> (String, String) {
    let mut hex = String::with_capacity(snippet.len() * 3);
    let mut ascii = String::with_capacity(snippet.len());

    for (i, &b) in snippet.iter().enumerate() {
        if i > 0 {
            hex.push(' ');
        }
        hex.push_str(&format!("{:02X}", b));

        if b.is_ascii_graphic() || b == b' ' {
            ascii.push(b as char);
        } else {
            ascii.push('.');
        }
    }

    (hex, ascii)
}

/// Search across a DumpDataProvider with a 2-buffer overlap window.
///
/// Supports cancellation and live progress updates.
pub fn search_provider(
    provider: Arc<Mutex<dyn DumpDataProvider>>,
    options: &SearchOptions,
    cancel_flag: Arc<AtomicBool>,
    progress_bytes: Arc<AtomicU64>,
) -> Result<Vec<SearchResult>, String> {
    search_provider_with_sender(provider, options, cancel_flag, progress_bytes, None)
}

/// Search across a DumpDataProvider with live streaming of matches as they are found.
pub fn search_provider_with_sender(
    provider: Arc<Mutex<dyn DumpDataProvider>>,
    options: &SearchOptions,
    cancel_flag: Arc<AtomicBool>,
    progress_bytes: Arc<AtomicU64>,
    match_sender: Option<Sender<SearchResult>>,
) -> Result<Vec<SearchResult>, String> {
    let pattern_bytes = parse_pattern(options)?;
    let pat_len = pattern_bytes.len();
    let is_case_insensitive = options.mode == SearchMode::Ascii && !options.case_sensitive;

    let meta = provider.lock().get_metadata();
    let total_size = meta.size;
    let page_length = meta.page_length as u64;
    let block_size = meta.block_size as u64;
    let block_stride = block_size * page_length;

    let chunk_size: usize = 256 * 1024; // 256 KB chunks
    let overlap: usize = pat_len.saturating_sub(1);

    let mut results = Vec::new();
    let mut offset: u64 = 0;
    let mut prev_tail: Vec<u8> = Vec::new();

    while offset < total_size {
        if cancel_flag.load(Ordering::Relaxed) {
            log::info!("Search cancelled at offset {}", offset);
            break;
        }

        let to_read = ((total_size - offset).min(chunk_size as u64)) as u32;
        let mut chunk = provider
            .lock()
            .read_bytes(offset, to_read)
            .map_err(|e| format!("Read failed at offset {}: {}", offset, e))?;

        // Combine prev_tail + chunk to handle matches straddling boundary
        let combined = if prev_tail.is_empty() {
            chunk
        } else {
            let mut c = prev_tail.clone();
            c.append(&mut chunk);
            c
        };

        let combined_start_offset = offset.saturating_sub(prev_tail.len() as u64);

        // Search within `combined`
        let search_limit = combined.len().saturating_sub(pat_len - 1);
        for i in 0..search_limit {
            if cancel_flag.load(Ordering::Relaxed) {
                break;
            }

            let slice = &combined[i..i + pat_len];
            let is_match = if is_case_insensitive {
                slice.iter().zip(&pattern_bytes).all(|(a, b)| a.to_ascii_lowercase() == *b)
            } else {
                slice == pattern_bytes.as_slice()
            };

            if is_match {
                let match_abs_offset = combined_start_offset + (i as u64);

                // Avoid duplicating matches found in overlap
                if let Some(last) = results.last() {
                    let last_res: &SearchResult = last;
                    if last_res.byte_offset == match_abs_offset {
                        continue;
                    }
                }

                // Compute block, page, and offset inside page
                let block = if block_stride > 0 { match_abs_offset / block_stride } else { 0 };
                let offset_in_block = if block_stride > 0 { match_abs_offset % block_stride } else { 0 };
                let page = if page_length > 0 { offset_in_block / page_length } else { 0 };
                let offset_in_page = if page_length > 0 { offset_in_block % page_length } else { 0 };

                // Extract preview snippet (up to 16 bytes starting at match)
                let preview_len = pat_len.max(16).min(combined.len() - i);
                let snippet = &combined[i..i + preview_len];
                let (preview_hex, preview_ascii) = generate_previews(snippet);

                let match_res = SearchResult {
                    index: results.len() + 1,
                    byte_offset: match_abs_offset,
                    block,
                    page,
                    offset_in_page,
                    preview_hex,
                    preview_ascii,
                };

                // Stream match to receiver immediately if sender is provided
                if let Some(ref sender) = match_sender {
                    let _ = sender.send(match_res.clone());
                }

                results.push(match_res);

                if results.len() >= options.max_matches {
                    break;
                }
            }
        }

        if results.len() >= options.max_matches {
            break;
        }

        // Keep tail for next iteration's overlap
        if overlap > 0 && combined.len() >= overlap {
            prev_tail = combined[combined.len() - overlap..].to_vec();
        } else {
            prev_tail.clear();
        }

        offset += to_read as u64;
        progress_bytes.store(offset, Ordering::Relaxed);
    }

    progress_bytes.store(total_size, Ordering::Relaxed);
    Ok(results)
}

/// Export search results to a file (CSV or plain formatted TXT)
pub fn export_results_to_file<P: AsRef<Path>>(
    results: &[SearchResult],
    path: P,
    is_csv: bool,
    page_length: u32, 
    block_size: u32,
) -> std::io::Result<()> {
    let mut file = File::create(path)?;

    if is_csv {
        writeln!(file, "Index,ByteOffset,Block,Page,OffsetInPage,HexPreview,AsciiPreview")?;
        for r in results {
            writeln!(
                file,
                "{},{},{},{},{},\"{}\",\"{}\"",
                r.index, r.byte_offset, r.block, r.page, r.offset_in_page, r.preview_hex, r.preview_ascii
            )?;
        }
    } else {
        writeln!(file, "--- Search Results ({}) --- Pagesize {} --- Blocksize {} ---", results.len(), page_length, block_size)?;
        writeln!(
            file,
            "{:<6} | {:<12} | {:<8} | {:<8} | {:<11} | {:<47} | Preview",
            "Index", "Byte Offset", "Block #", "Page #", "Page Offset", "Hex Preview"
        )?;
        writeln!(file, "{:-<100}", "")?;
        for r in results {
            writeln!(
                file,
                "{:<6} | {:<12} | {:<8} | {:<8} | {:<11} | {:<47} | {}",
                r.index, r.byte_offset, r.block, r.page, r.offset_in_page, r.preview_hex, r.preview_ascii
            )?;
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data_provider::FileDataProvider;
    use crate::file_loader::FileLoader;
    use tempfile::NamedTempFile;

    fn create_test_provider(data: &[u8]) -> Arc<Mutex<dyn DumpDataProvider>> {
        let mut temp = NamedTempFile::new().unwrap();
        temp.write_all(data).unwrap();
        temp.flush().unwrap();
        let loader = FileLoader::new(temp.path(), 512, 64).unwrap();
        Arc::new(Mutex::new(FileDataProvider::new(loader)))
    }

    #[test]
    fn test_ascii_search() {
        let mut data = vec![0u8; 1024];
        data[100..104].copy_from_slice(b"KIR0");
        let provider = create_test_provider(&data);

        let options = SearchOptions {
            pattern: "KIR0".to_string(),
            mode: SearchMode::Ascii,
            case_sensitive: true,
            max_matches: 10,
        };

        let cancel = Arc::new(AtomicBool::new(false));
        let progress = Arc::new(AtomicU64::new(0));

        let res = search_provider(provider, &options, cancel, progress).unwrap();
        assert_eq!(res.len(), 1);
        assert_eq!(res[0].byte_offset, 100);
        assert_eq!(res[0].block, 0);
        assert_eq!(res[0].page, 0);
        assert_eq!(res[0].offset_in_page, 100);
    }

    #[test]
    fn test_hex_search() {
        let mut data = vec![0u8; 1024];
        data[600..604].copy_from_slice(&[0xDE, 0xAD, 0xBE, 0xEF]);
        let provider = create_test_provider(&data);

        let options = SearchOptions {
            pattern: "DEADBEEF".to_string(),
            mode: SearchMode::Hex,
            case_sensitive: false,
            max_matches: 10,
        };

        let cancel = Arc::new(AtomicBool::new(false));
        let progress = Arc::new(AtomicU64::new(0));

        let res = search_provider(provider, &options, cancel, progress).unwrap();
        assert_eq!(res.len(), 1);
        assert_eq!(res[0].byte_offset, 600);
        // Offset 600 with page_size 512: page = 1, offset_in_page = 88
        assert_eq!(res[0].page, 1);
        assert_eq!(res[0].offset_in_page, 88);
    }

    #[test]
    fn test_search_cancellation() {
        let data = vec![0u8; 1024];
        let provider = create_test_provider(&data);

        let options = SearchOptions {
            pattern: "ABCD".to_string(),
            mode: SearchMode::Ascii,
            case_sensitive: true,
            max_matches: 10,
        };

        let cancel = Arc::new(AtomicBool::new(true)); // Pre-cancelled
        let progress = Arc::new(AtomicU64::new(0));

        let res = search_provider(provider, &options, cancel, progress).unwrap();
        assert_eq!(res.len(), 0);
    }

    #[test]
    fn test_export_results() {
        let page_length = 56122;
        let block_size = 384;
        let results = vec![SearchResult {
            index: 1,
            byte_offset: 512,
            block: 0,
            page: 1,
            offset_in_page: 0,
            preview_hex: "55 AA".to_string(),
            preview_ascii: "U.".to_string(),
        }];

        let temp_csv = NamedTempFile::new().unwrap();
        assert!(export_results_to_file(&results, temp_csv.path(), true, page_length, block_size).is_ok());

        let temp_txt = NamedTempFile::new().unwrap();
        assert!(export_results_to_file(&results, temp_txt.path(), false, page_length, block_size).is_ok());
    }

    #[test]
    fn test_search_provider_with_sender_streaming() {
        let mut data = vec![0u8; 2048];
        data[100..104].copy_from_slice(b"FIND");
        data[500..504].copy_from_slice(b"FIND");
        data[1500..1504].copy_from_slice(b"FIND");
        let provider = create_test_provider(&data);

        let options = SearchOptions {
            pattern: "FIND".to_string(),
            mode: SearchMode::Ascii,
            case_sensitive: true,
            max_matches: 10,
        };

        let cancel = Arc::new(AtomicBool::new(false));
        let progress = Arc::new(AtomicU64::new(0));
        let (tx, rx) = std::sync::mpsc::channel();

        let res = search_provider_with_sender(provider, &options, cancel, progress, Some(tx)).unwrap();
        assert_eq!(res.len(), 3);

        // Verify matches were streamed into rx in real-time
        let mut streamed = Vec::new();
        while let Ok(m) = rx.try_recv() {
            streamed.push(m);
        }
        assert_eq!(streamed.len(), 3);
        assert_eq!(streamed[0].byte_offset, 100);
        assert_eq!(streamed[1].byte_offset, 500);
        assert_eq!(streamed[2].byte_offset, 1500);
    }
}
