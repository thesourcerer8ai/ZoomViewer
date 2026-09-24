//! DumpDataProvider trait and implementations for dynamic, on-demand data pipeline.
//!
//! Replaces direct file reading in ZoomViewer with a pull-based stream:
//! - FileDataProvider: reads chunks dynamically from disk on demand.
//! - XorDataProvider: dynamically evaluates XOR across 1, 2, or 3 inputs + static patterns,
//!   applying scope-based pattern repetition (page, block, or linear) with 0x00 padding.

use crate::error::Result;
use crate::file_loader::FileLoader;
use crate::types::{FileMetadata, Fragment};
use parking_lot::Mutex;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

/// Trait for any source providing NAND dump bytes on demand.
pub trait DumpDataProvider: Send + Sync {
    /// Read `length` bytes starting at `offset`.
    fn read_bytes(&mut self, offset: u64, length: u32) -> Result<Vec<u8>>;

    /// Read multiple fragments and concatenate results.
    fn read_fragments(&mut self, fragments: Vec<Fragment>) -> Result<Vec<u8>> {
        if fragments.is_empty() {
            return Ok(Vec::new());
        }
        let mut sorted = fragments;
        sorted.sort_by_key(|f| f.start_byte);
        let merged = FileLoader::merge_contiguous_fragments(&sorted);
        let mut result = Vec::new();
        for f in merged {
            let len = f.length() as u32;
            let bytes = self.read_bytes(f.start_byte, len)?;
            result.extend_from_slice(&bytes);
        }
        Ok(result)
    }

    /// Retrieve metadata for this provider.
    fn get_metadata(&self) -> FileMetadata;

    /// Stable unique cache identifier for tile caching.
    fn cache_identity(&self) -> String;
}

// Implement DumpDataProvider directly for FileLoader for backward compatibility
impl DumpDataProvider for FileLoader {
    fn read_bytes(&mut self, offset: u64, length: u32) -> Result<Vec<u8>> {
        self.read_bytes(offset, length)
    }

    fn read_fragments(&mut self, fragments: Vec<Fragment>) -> Result<Vec<u8>> {
        self.read_fragments(fragments)
    }

    fn get_metadata(&self) -> FileMetadata {
        self.get_metadata()
    }

    fn cache_identity(&self) -> String {
        let meta = self.get_metadata();
        let mut hasher = DefaultHasher::new();
        meta.path.hash(&mut hasher);
        meta.size.hash(&mut hasher);
        meta.page_length.hash(&mut hasher);
        meta.block_size.hash(&mut hasher);
        format!("{:016x}", hasher.finish())
    }
}

/// A provider that wraps a FileLoader.
pub struct FileDataProvider {
    loader: FileLoader,
    identity: String,
}

impl FileDataProvider {
    pub fn new(loader: FileLoader) -> Self {
        let meta = loader.get_metadata();
        let mut hasher = DefaultHasher::new();
        "FILE_INPUT".hash(&mut hasher);
        meta.path.hash(&mut hasher);
        meta.size.hash(&mut hasher);
        meta.page_length.hash(&mut hasher);
        meta.block_size.hash(&mut hasher);
        let identity = format!("{:016x}", hasher.finish());
        Self { loader, identity }
    }
}

impl DumpDataProvider for FileDataProvider {
    fn read_bytes(&mut self, offset: u64, length: u32) -> Result<Vec<u8>> {
        self.loader.read_bytes(offset, length)
    }

    fn read_fragments(&mut self, fragments: Vec<Fragment>) -> Result<Vec<u8>> {
        self.loader.read_fragments(fragments)
    }

    fn get_metadata(&self) -> FileMetadata {
        self.loader.get_metadata()
    }

    fn cache_identity(&self) -> String {
        self.identity.clone()
    }
}

/// A provider that evaluates XOR on-the-fly between a primary input,
/// up to 2 secondary inputs, and an optional static pattern.
pub struct XorDataProvider {
    primary: Arc<Mutex<dyn DumpDataProvider>>,
    secondaries: Vec<Arc<Mutex<dyn DumpDataProvider>>>,
    static_pattern: Vec<u8>,
    metadata: FileMetadata,
    identity: String,
}

impl XorDataProvider {
    /// Create a new XorDataProvider.
    ///
    /// # Arguments
    /// * `primary` - The main input provider (determines page_length and block_size)
    /// * `secondaries` - 0, 1, or 2 additional input providers
    /// * `static_pattern` - Optional static byte pattern (e.g. `[0xFF]` or `[0x77]`)
    pub fn new(
        primary: Arc<Mutex<dyn DumpDataProvider>>,
        secondaries: Vec<Arc<Mutex<dyn DumpDataProvider>>>,
        static_pattern: Vec<u8>,
    ) -> Self {
        let primary_meta = primary.lock().get_metadata();
        let page_length = primary_meta.page_length;
        let block_size = primary_meta.block_size;

        // Overall size is the maximum of all input sizes
        let mut max_size = primary_meta.size;
        for sec in &secondaries {
            let s_meta = sec.lock().get_metadata();
            if s_meta.size > max_size {
                max_size = s_meta.size;
            }
        }

        let mut hasher = DefaultHasher::new();
        "XOR_NODE".hash(&mut hasher);
        primary.lock().cache_identity().hash(&mut hasher);
        for sec in &secondaries {
            sec.lock().cache_identity().hash(&mut hasher);
        }
        static_pattern.hash(&mut hasher);
        page_length.hash(&mut hasher);
        block_size.hash(&mut hasher);
        max_size.hash(&mut hasher);
        let identity = format!("xor_{:016x}", hasher.finish());

        let metadata = FileMetadata::new(
            format!("xor://{}", identity),
            max_size,
            page_length,
            block_size,
        );

        Self {
            primary,
            secondaries,
            static_pattern,
            metadata,
            identity,
        }
    }

    /// Bulk-apply XOR from a secondary provider into `buffer` for bytes `[offset, offset+len)`.
    ///
    /// Processes the range in segments aligned to the secondary's repeat period (page or block),
    /// issuing one `read_bytes` call per contiguous active segment instead of one per byte.
    fn apply_secondary_xor(
        buffer: &mut [u8],
        sec: &mut dyn DumpDataProvider,
        sec_size: u64,
        offset: u64,
        page_length: u64,
        block_bytes: u64,
    ) -> Result<()> {
        if sec_size == 0 {
            return Ok(());
        }

        let length = buffer.len() as u64;

        if sec_size <= page_length {
            // Rule 1: repeats every `page_length` bytes; only first `sec_size` bytes are active
            let period = page_length;
            let mut i = 0u64;
            while i < length {
                let pos = offset + i;
                let off_in_period = pos % period;
                if off_in_period < sec_size {
                    let active_remaining = (sec_size - off_in_period).min(length - i);
                    let sec_bytes = sec.read_bytes(off_in_period, active_remaining as u32)?;
                    let buf_slice = &mut buffer[i as usize..(i + active_remaining) as usize];
                    for (b, s) in buf_slice.iter_mut().zip(sec_bytes.iter()) {
                        *b ^= s;
                    }
                    i += active_remaining;
                } else {
                    // Zero-padded region: skip to next period boundary
                    let skip = (period - off_in_period).min(length - i);
                    i += skip;
                }
            }
        } else if sec_size <= block_bytes {
            // Rule 2: repeats every `block_bytes`; only first `sec_size` bytes are active
            let period = block_bytes;
            let mut i = 0u64;
            while i < length {
                let pos = offset + i;
                let off_in_period = pos % period;
                if off_in_period < sec_size {
                    let active_remaining = (sec_size - off_in_period).min(length - i);
                    let sec_bytes = sec.read_bytes(off_in_period, active_remaining as u32)?;
                    let buf_slice = &mut buffer[i as usize..(i + active_remaining) as usize];
                    for (b, s) in buf_slice.iter_mut().zip(sec_bytes.iter()) {
                        *b ^= s;
                    }
                    i += active_remaining;
                } else {
                    let skip = (period - off_in_period).min(length - i);
                    i += skip;
                }
            }
        } else {
            // Rule 3: linear, one-to-one; bytes beyond sec_size are XOR with 0x00 (no-op)
            if offset < sec_size {
                let readable = (sec_size - offset).min(length);
                let sec_bytes = sec.read_bytes(offset, readable as u32)?;
                let buf_slice = &mut buffer[..readable as usize];
                for (b, s) in buf_slice.iter_mut().zip(sec_bytes.iter()) {
                    *b ^= s;
                }
            }
        }

        Ok(())
    }
}

impl DumpDataProvider for XorDataProvider {
    fn read_bytes(&mut self, offset: u64, length: u32) -> Result<Vec<u8>> {
        if length == 0 {
            return Ok(Vec::new());
        }

        let primary_size = self.primary.lock().get_metadata().size;
        let mut buffer = vec![0u8; length as usize];

        // 1. Read primary input bytes
        if offset < primary_size {
            let to_read = ((primary_size - offset).min(length as u64)) as u32;
            let p_bytes = self.primary.lock().read_bytes(offset, to_read)?;
            buffer[..p_bytes.len()].copy_from_slice(&p_bytes);
        }

        let page_len = self.metadata.page_length as u64;
        let block_bytes = (self.metadata.block_size as u64) * page_len;

        // 2. Apply each secondary input using bulk segment-aligned reads
        for sec_arc in &self.secondaries {
            let mut sec = sec_arc.lock();
            let sec_size = sec.get_metadata().size;
            Self::apply_secondary_xor(&mut buffer, &mut *sec, sec_size, offset, page_len, block_bytes)?;
        }

        // 3. Apply static pattern if present
        if !self.static_pattern.is_empty() {
            let pat_len = self.static_pattern.len() as u64;
            for i in 0..(length as usize) {
                let pos = offset + (i as u64);
                let pat_byte = self.static_pattern[(pos % pat_len) as usize];
                buffer[i] ^= pat_byte;
            }
        }

        Ok(buffer)
    }

    fn get_metadata(&self) -> FileMetadata {
        self.metadata.clone()
    }

    fn cache_identity(&self) -> String {
        self.identity.clone()
    }
}

/// A provider that concatenates 2 or more input providers sequentially.
///
/// Each input is zero-padded to the length of the longest input (measured in pages),
/// so the final stream is `max_pages * page_length * num_inputs` bytes.
/// The `page_length` and `block_size` are taken from the first (primary) input.
/// If inputs have different `page_length` values, the primary's value is used and
/// shorter-paged inputs are read at their native granularity within the common page window.
pub struct ConcatenateDataProvider {
    inputs: Vec<Arc<Mutex<dyn DumpDataProvider>>>,
    /// Individual sizes of each input (used for zero-padding)
    input_sizes: Vec<u64>,
    /// The maximum number of pages across all inputs (used for padding alignment)
    max_pages: u64,
    metadata: FileMetadata,
    identity: String,
}

impl ConcatenateDataProvider {
    /// Create a new ConcatenateDataProvider from at least one input.
    ///
    /// # Arguments
    /// * `inputs` – Two or more providers to concatenate. The first provider's
    ///              `page_length` and `block_size` are used for the combined metadata.
    ///
    /// # Panics
    /// Panics if `inputs` is empty.
    pub fn new(inputs: Vec<Arc<Mutex<dyn DumpDataProvider>>>) -> Self {
        assert!(!inputs.is_empty(), "ConcatenateDataProvider requires at least one input");

        let primary_meta = inputs[0].lock().get_metadata();
        let page_length = primary_meta.page_length;
        let block_size = primary_meta.block_size;

        // Collect per-input sizes and compute max page count
        let input_sizes: Vec<u64> = inputs.iter()
            .map(|p| p.lock().get_metadata().size)
            .collect();

        let max_pages: u64 = input_sizes.iter()
            .map(|&sz| (sz + page_length as u64 - 1) / page_length as u64)
            .max()
            .unwrap_or(0);

        // Total concatenated size = each input padded to max_pages * page_length
        let padded_input_size = max_pages * page_length as u64;
        let total_size = padded_input_size * inputs.len() as u64;

        // Build a stable cache identity
        let mut hasher = DefaultHasher::new();
        "CONCATENATE_NODE".hash(&mut hasher);
        for inp in &inputs {
            inp.lock().cache_identity().hash(&mut hasher);
        }
        page_length.hash(&mut hasher);
        block_size.hash(&mut hasher);
        total_size.hash(&mut hasher);
        let identity = format!("concat_{:016x}", hasher.finish());

        let metadata = FileMetadata::new(
            format!("concat://{}", identity),
            total_size,
            page_length,
            block_size,
        );

        Self {
            inputs,
            input_sizes,
            max_pages,
            metadata,
            identity,
        }
    }
}

impl DumpDataProvider for ConcatenateDataProvider {
    fn read_bytes(&mut self, offset: u64, length: u32) -> Result<Vec<u8>> {
        if length == 0 {
            return Ok(Vec::new());
        }

        let total_size = self.metadata.size;
        if offset >= total_size {
            return Ok(vec![0u8; length as usize]);
        }

        let page_len = self.metadata.page_length as u64;
        let padded_input_size = self.max_pages * page_len;
        let mut buffer = vec![0u8; length as usize];

        // Clamp readable range to valid data
        let readable_end = (offset + length as u64).min(total_size);

        // Determine which input slots overlap with [offset, readable_end)
        let first_input = (offset / padded_input_size) as usize;
        let last_input = ((readable_end.saturating_sub(1)) / padded_input_size) as usize;

        for input_idx in first_input..=last_input {
            if input_idx >= self.inputs.len() {
                break;
            }

            let slot_start = input_idx as u64 * padded_input_size;
            let slot_end = slot_start + padded_input_size;

            // Segment within this slot that overlaps with the requested range
            let seg_start = offset.max(slot_start);
            let seg_end = readable_end.min(slot_end);
            if seg_end <= seg_start {
                continue;
            }

            let in_slot_offset = seg_start - slot_start; // offset within this input's padded region
            let seg_len = (seg_end - seg_start) as u32;
            let input_size = self.input_sizes[input_idx];

            // How much of this segment actually has real data?
            if in_slot_offset < input_size {
                let readable = ((input_size - in_slot_offset).min(seg_len as u64)) as u32;
                let bytes = self.inputs[input_idx].lock().read_bytes(in_slot_offset, readable)?;

                let dest_start = (seg_start - offset) as usize;
                let copy_len = bytes.len().min(readable as usize);
                buffer[dest_start..dest_start + copy_len].copy_from_slice(&bytes[..copy_len]);
                // Remainder of seg (in_slot_offset + readable .. seg_end) stays 0x00 (already zeroed)
            }
            // If in_slot_offset >= input_size the whole segment is padding — already 0x00
        }

        Ok(buffer)
    }

    fn get_metadata(&self) -> FileMetadata {
        self.metadata.clone()
    }

    fn cache_identity(&self) -> String {
        self.identity.clone()
    }
}

/// A provider that outputs a filtered sequential view containing only the pages that have search results.
/// All other pages without search results in them are suppressed.
pub struct SearchFilteredDataProvider {
    source: Arc<Mutex<dyn DumpDataProvider>>,
    matching_pages: Vec<u64>,
    search_pattern: String,
    metadata: FileMetadata,
    identity: String,
}

impl SearchFilteredDataProvider {
    /// Create a new SearchFilteredDataProvider.
    ///
    /// # Arguments
    /// * `source` - Upstream data provider (determines page_length and block_size)
    /// * `matching_pages` - Deduplicated and sorted list of original absolute page indices
    /// * `search_pattern` - The search pattern used to generate these matches
    pub fn new(
        source: Arc<Mutex<dyn DumpDataProvider>>,
        matching_pages: Vec<u64>,
        search_pattern: String,
    ) -> Self {
        let (page_length, block_size, src_identity) = {
            let guard = source.lock();
            let m = guard.get_metadata();
            (m.page_length, m.block_size, guard.cache_identity())
        };

        let total_pages = matching_pages.len() as u64;
        let total_size = total_pages * (page_length as u64);

        let mut hasher = DefaultHasher::new();
        "SEARCH_FILTERED".hash(&mut hasher);
        src_identity.hash(&mut hasher);
        search_pattern.hash(&mut hasher);
        matching_pages.hash(&mut hasher);
        page_length.hash(&mut hasher);
        block_size.hash(&mut hasher);
        total_size.hash(&mut hasher);
        let identity = format!("filtered_{:016x}", hasher.finish());

        let metadata = FileMetadata::new(
            format!("search://filtered/{}", identity),
            total_size,
            page_length,
            block_size,
        );

        Self {
            source,
            matching_pages,
            search_pattern,
            metadata,
            identity,
        }
    }

    /// Slice of original matching page indices
    pub fn matching_pages(&self) -> &[u64] {
        &self.matching_pages
    }

    /// Search pattern used to produce this filtered view
    pub fn search_pattern(&self) -> &str {
        &self.search_pattern
    }

    /// Translates a filtered page index to the original dump page index
    pub fn original_page(&self, filtered_page: u64) -> Option<u64> {
        self.matching_pages.get(filtered_page as usize).copied()
    }
}

impl DumpDataProvider for SearchFilteredDataProvider {
    fn read_bytes(&mut self, offset: u64, length: u32) -> Result<Vec<u8>> {
        if length == 0 || offset >= self.metadata.size {
            return Ok(vec![0u8; length as usize]);
        }

        let total_size = self.metadata.size;
        let page_len = self.metadata.page_length as u64;
        let mut buffer = vec![0u8; length as usize];

        // Valid readable range within the filtered stream
        let readable_end = (offset + length as u64).min(total_size);
        let start_fp = offset / page_len;
        let end_fp = (readable_end - 1) / page_len;

        for fp in start_fp..=end_fp {
            if (fp as usize) >= self.matching_pages.len() {
                break;
            }
            let orig_page = self.matching_pages[fp as usize];
            let filtered_page_start_byte = fp * page_len;
            let filtered_page_end_byte = (fp + 1) * page_len;

            // Determine overlapping segment within this page
            let seg_start = offset.max(filtered_page_start_byte);
            let seg_end = readable_end.min(filtered_page_end_byte);
            if seg_end <= seg_start {
                continue;
            }

            let in_page_offset = seg_start - filtered_page_start_byte;
            let seg_len = (seg_end - seg_start) as u32;

            let orig_offset = orig_page * page_len + in_page_offset;
            let page_bytes = self.source.lock().read_bytes(orig_offset, seg_len)?;

            let dest_start = (seg_start - offset) as usize;
            let dest_end = dest_start + (page_bytes.len().min(seg_len as usize));
            buffer[dest_start..dest_end].copy_from_slice(&page_bytes[..dest_end - dest_start]);
        }

        Ok(buffer)
    }

    fn get_metadata(&self) -> FileMetadata {
        self.metadata.clone()
    }

    fn cache_identity(&self) -> String {
        self.identity.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    fn create_test_file(data: &[u8]) -> (NamedTempFile, FileLoader) {
        let mut temp = NamedTempFile::new().unwrap();
        temp.write_all(data).unwrap();
        temp.flush().unwrap();
        let loader = FileLoader::new(temp.path(), 512, 64).unwrap();
        (temp, loader)
    }

    #[test]
    fn test_file_data_provider() {
        let test_data = vec![1, 2, 3, 4, 5];
        let (_temp, loader) = create_test_file(&test_data);
        let mut provider = FileDataProvider::new(loader);
        let read = provider.read_bytes(0, 5).unwrap();
        assert_eq!(read, test_data);
        assert!(!provider.cache_identity().is_empty());
    }

    #[test]
    fn test_xor_rule1_page_pattern() {
        // Page size = 512 bytes. Shorter file = 4 bytes.
        // Rule 1: shorter file <= page size -> applied to start of each page, rest 0x00
        let p_data = vec![0x00u8; 1024]; // 2 pages
        let (_temp_p, p_loader) = create_test_file(&p_data);
        let primary = Arc::new(Mutex::new(FileDataProvider::new(p_loader)));

        let sec_data = vec![0xAA, 0xBB, 0xCC, 0xDD];
        let (_temp_s, s_loader) = create_test_file(&sec_data);
        let sec = Arc::new(Mutex::new(FileDataProvider::new(s_loader)));

        let mut xor_provider = XorDataProvider::new(primary, vec![sec], Vec::new());

        // Check first page: bytes 0..4 should be AA, BB, CC, DD; byte 4..512 should be 0x00
        let page1 = xor_provider.read_bytes(0, 8).unwrap();
        assert_eq!(page1, vec![0xAA, 0xBB, 0xCC, 0xDD, 0x00, 0x00, 0x00, 0x00]);

        // Check second page (offset 512): bytes 512..516 should be AA, BB, CC, DD
        let page2 = xor_provider.read_bytes(512, 8).unwrap();
        assert_eq!(page2, vec![0xAA, 0xBB, 0xCC, 0xDD, 0x00, 0x00, 0x00, 0x00]);
    }

    #[test]
    fn test_xor_rule2_block_pattern() {
        // Page size = 512 bytes, block_size = 2 pages -> block = 1024 bytes.
        // Shorter file = 600 bytes (> page size, <= block size).
        // Rule 2: applied to start of each block, rest of block 0x00.
        let p_data = vec![0x00u8; 2048]; // 2 blocks (4 pages)
        let mut temp_p = NamedTempFile::new().unwrap();
        temp_p.write_all(&p_data).unwrap();
        let p_loader = FileLoader::new(temp_p.path(), 512, 64).unwrap();
        let primary = Arc::new(Mutex::new(FileDataProvider::new(p_loader)));

        let mut sec_data = vec![0x55u8; 600];
        sec_data[0] = 0x99;
        let mut temp_s = NamedTempFile::new().unwrap();
        temp_s.write_all(&sec_data).unwrap();
        let s_loader = FileLoader::new(temp_s.path(), 512, 64).unwrap();
        let sec = Arc::new(Mutex::new(FileDataProvider::new(s_loader)));

        let mut xor_provider = XorDataProvider::new(primary, vec![sec], Vec::new());

        // At block 0 start (offset 0):
        let read0 = xor_provider.read_bytes(0, 2).unwrap();
        assert_eq!(read0, vec![0x99, 0x55]);

        // At offset 599: 0x55; at offset 600: 0x00 (padded with 0x00)
        let read600 = xor_provider.read_bytes(599, 2).unwrap();
        assert_eq!(read600, vec![0x55, 0x00]);
    }

    #[test]
    fn test_xor_with_static_pattern() {
        let p_data = vec![0x00; 16];
        let (_temp_p, p_loader) = create_test_file(&p_data);
        let primary = Arc::new(Mutex::new(FileDataProvider::new(p_loader)));

        let mut xor_provider = XorDataProvider::new(primary, vec![], vec![0xFF, 0x77]);
        let read = xor_provider.read_bytes(0, 4).unwrap();
        assert_eq!(read, vec![0xFF, 0x77, 0xFF, 0x77]);
    }

    #[test]
    fn test_search_filtered_data_provider() {
        // Create 4 pages of 512 bytes each:
        // Page 0: 0x00
        // Page 1: 0x11
        // Page 2: 0x22
        // Page 3: 0x33
        let mut data = vec![0u8; 2048];
        data[512..1024].fill(0x11);
        data[1024..1536].fill(0x22);
        data[1536..2048].fill(0x33);

        let (_temp, loader) = create_test_file(&data);
        let primary = Arc::new(Mutex::new(FileDataProvider::new(loader)));

        // Filter: only page 1 and page 3 contain matches
        let matching_pages = vec![1, 3];
        let mut filtered = SearchFilteredDataProvider::new(primary, matching_pages, "|Block".to_string());

        // Check metadata: 2 pages * 512 = 1024 bytes
        let meta = filtered.get_metadata();
        assert_eq!(meta.size, 1024);
        assert_eq!(meta.page_length, 512);
        assert_eq!(meta.total_pages, 2);

        // Page mappings
        assert_eq!(filtered.original_page(0), Some(1));
        assert_eq!(filtered.original_page(1), Some(3));
        assert_eq!(filtered.original_page(2), None);

        // Read page 0 of filtered (corresponds to page 1 of upstream = 0x11)
        let read_p0 = filtered.read_bytes(0, 512).unwrap();
        assert_eq!(read_p0.len(), 512);
        assert!(read_p0.iter().all(|&b| b == 0x11));

        // Read page 1 of filtered (corresponds to page 3 of upstream = 0x33)
        let read_p1 = filtered.read_bytes(512, 512).unwrap();
        assert_eq!(read_p1.len(), 512);
        assert!(read_p1.iter().all(|&b| b == 0x33));

        // Straddled read across filtered page 0 and page 1 (last 4 bytes of p0 and first 4 bytes of p1)
        let read_straddle = filtered.read_bytes(508, 8).unwrap();
        assert_eq!(read_straddle, vec![0x11, 0x11, 0x11, 0x11, 0x33, 0x33, 0x33, 0x33]);

        // Reading beyond EOF returns 0s
        let read_past = filtered.read_bytes(1020, 10).unwrap();
        assert_eq!(&read_past[..4], &[0x33, 0x33, 0x33, 0x33]);
        assert_eq!(&read_past[4..], &[0x00, 0x00, 0x00, 0x00, 0x00, 0x00]);
    }

    #[test]
    fn test_search_filtered_data_provider_empty() {
        let data = vec![0xAAu8; 1024];
        let (_temp, loader) = create_test_file(&data);
        let primary = Arc::new(Mutex::new(FileDataProvider::new(loader)));

        let mut filtered = SearchFilteredDataProvider::new(primary, vec![], "|Block".to_string());
        let meta = filtered.get_metadata();
        assert_eq!(meta.size, 0);
        assert_eq!(meta.total_pages, 0);

        let read = filtered.read_bytes(0, 100).unwrap();
        assert_eq!(read, vec![0u8; 100]);
    }
}
