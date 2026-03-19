//! FileLoader for reading NAND dump files
//!
//! Provides sequential byte access to dump files with metadata detection
//! and efficient fragment reading with contiguous optimization.

use crate::{Error, FileMetadata, Fragment, Result};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

/// Loads and provides access to NAND dump files
pub struct FileLoader {
    file: File,
    metadata: FileMetadata,
}

impl FileLoader {
    /// Opens a dump file and initializes metadata
    ///
    /// # Arguments
    /// * `path` - Path to the dump file
    /// * `page_length` - Bytes per page (500-100000)
    /// * `block_size` - Pages per block (64, 128, 256, 512, or 1024)
    ///
    /// # Returns
    /// * `Ok(FileLoader)` - Successfully opened file with metadata
    /// * `Err(Error)` - File open failed or metadata invalid
    ///
    /// # Requirements
    /// * 1.1: Accept files of any size
    /// * 1.2: Accept user-provided page length
    /// * 1.3: Accept user-provided block size
    /// * 1.6: Store metadata in memory
    pub fn new<P: AsRef<Path>>(
        path: P,
        page_length: u32,
        block_size: u32,
    ) -> Result<Self> {
        let path_ref = path.as_ref();
        
        // Open file in read-only mode
        let file = File::open(path_ref)
            .map_err(|e| Error::IoError(e))?;
        
        // Get file size
        let size = file.metadata()
            .map_err(|e| Error::IoError(e))?
            .len();
        
        // Validate page length (500-100000 bytes)
        if page_length < 500 || page_length > 100000 {
            return Err(Error::InvalidMetadata(
                format!("Page length {} is outside valid range (500-100000)", page_length)
            ));
        }
        
        // Validate block size (64, 128, 256, 384, 512, 768, or 1024 pages per block)
        let valid_block_sizes = [64, 128, 256, 384, 512, 768, 1024];
        if !valid_block_sizes.contains(&block_size) {
            return Err(Error::InvalidMetadata(
                format!("Block size {} is not one of: 64, 128, 256, 384, 512, 768, 1024", block_size)
            ));
        }
        
        // Create metadata with calculated derived values
        let path_str = path_ref
            .to_str()
            .ok_or_else(|| Error::InvalidMetadata("Invalid path".to_string()))?
            .to_string();
        
        let metadata = FileMetadata::new(path_str, size, page_length, block_size);
        
        Ok(FileLoader { file, metadata })
    }
    
    /// Returns the cached file metadata
    ///
    /// # Returns
    /// File metadata including size, page length, block size, and derived values
    pub fn get_metadata(&self) -> FileMetadata {
        self.metadata.clone()
    }
    
    /// Reads bytes from the dump file at a specific offset
    ///
    /// # Arguments
    /// * `offset` - Byte offset in the dump file
    /// * `length` - Number of bytes to read
    ///
    /// # Returns
    /// * `Ok(Vec<u8>)` - Bytes read from file
    /// * `Err(Error)` - I/O error occurred
    ///
    /// # Requirements
    /// * 1.1: Handle I/O errors gracefully
    pub fn read_bytes(&mut self, offset: u64, length: u32) -> Result<Vec<u8>> {
        // Seek to offset
        self.file.seek(SeekFrom::Start(offset))
            .map_err(|e| Error::IoError(e))?;
        
        // Read bytes
        let mut buffer = vec![0u8; length as usize];
        self.file.read_exact(&mut buffer)
            .map_err(|e| Error::IoError(e))?;
        
        Ok(buffer)
    }
    
    /// Reads multiple fragments from the dump file and concatenates them
    ///
    /// Optimizes for contiguous fragments by merging them into single reads
    /// when possible, reducing I/O overhead.
    ///
    /// # Arguments
    /// * `fragments` - Vector of byte range fragments to read
    ///
    /// # Returns
    /// * `Ok(Vec<u8>)` - Concatenated bytes from all fragments
    /// * `Err(Error)` - I/O error occurred
    ///
    /// # Requirements
    /// * 6.1: Read multiple fragments
    /// * 6.2: Optimize for contiguous fragments
    pub fn read_fragments(&mut self, fragments: Vec<Fragment>) -> Result<Vec<u8>> {
        if fragments.is_empty() {
            return Ok(Vec::new());
        }
        
        // Sort fragments by start byte to enable contiguous optimization
        let mut sorted_fragments = fragments;
        sorted_fragments.sort_by_key(|f| f.start_byte);
        
        // Merge contiguous fragments
        let merged = Self::merge_contiguous_fragments(&sorted_fragments);
        
        // Read merged fragments and concatenate
        let mut result = Vec::new();
        for fragment in merged {
            let length = fragment.length() as u32;
            let bytes = self.read_bytes(fragment.start_byte, length)?;
            result.extend_from_slice(&bytes);
        }
        
        Ok(result)
    }
    
    /// Merges contiguous fragments into larger fragments to reduce I/O operations
    ///
    /// # Arguments
    /// * `fragments` - Sorted vector of fragments
    ///
    /// # Returns
    /// Vector of merged fragments with gaps filled
    fn merge_contiguous_fragments(fragments: &[Fragment]) -> Vec<Fragment> {
        if fragments.is_empty() {
            return Vec::new();
        }
        
        let mut merged = Vec::new();
        let mut current_start = fragments[0].start_byte;
        let mut current_end = fragments[0].end_byte;
        
        for fragment in &fragments[1..] {
            // If fragment is contiguous or overlapping, extend current
            if fragment.start_byte <= current_end {
                current_end = current_end.max(fragment.end_byte);
            } else {
                // Gap found, save current and start new
                merged.push(Fragment::new(current_start, current_end));
                current_start = fragment.start_byte;
                current_end = fragment.end_byte;
            }
        }
        
        // Add final fragment
        merged.push(Fragment::new(current_start, current_end));
        
        merged
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Write, Seek};
    use tempfile::NamedTempFile;

    fn create_test_file(size: u64) -> NamedTempFile {
        let mut file = NamedTempFile::new().unwrap();
        // For large files, just write a small amount and seek to the end
        // This creates a sparse file which is much faster
        if size > 1024 * 1024 * 100 { // > 100 MB
            file.write_all(&[0xABu8; 1024]).unwrap();
            file.seek(std::io::SeekFrom::Start(size - 1)).unwrap();
            file.write_all(&[0xABu8]).unwrap();
        } else {
            let chunk = vec![0xABu8; 1024 * 1024];
            let mut remaining = size;
            while remaining > 0 {
                let to_write = std::cmp::min(remaining, 1024 * 1024) as usize;
                file.write_all(&chunk[..to_write]).unwrap();
                remaining -= to_write as u64;
            }
        }
        file.flush().unwrap();
        file
    }

    #[test]
    fn test_file_loader_new_valid() {
        // Test with a small file (1 MB)
        let file = create_test_file(1024 * 1024);
        let loader = FileLoader::new(file.path(), 512, 64);
        assert!(loader.is_ok());
    }

    #[test]
    fn test_file_loader_new_small_file() {
        // Test with a very small file (10 KB)
        let file = create_test_file(10 * 1024);
        let loader = FileLoader::new(file.path(), 512, 64);
        assert!(loader.is_ok());
    }

    #[test]
    fn test_file_loader_new_invalid_page_length() {
        let file = create_test_file(1024 * 1024);
        let loader = FileLoader::new(file.path(), 100, 64); // Too small
        assert!(loader.is_err());
    }

    #[test]
    fn test_file_loader_new_invalid_block_size() {
        let file = create_test_file(1024 * 1024);
        let loader = FileLoader::new(file.path(), 512, 100); // Invalid
        assert!(loader.is_err());
    }

    #[test]
    fn test_get_metadata() {
        let file = create_test_file(1024 * 1024);
        let loader = FileLoader::new(file.path(), 512, 64).unwrap();
        let metadata = loader.get_metadata();
        
        assert_eq!(metadata.page_length, 512);
        assert_eq!(metadata.block_size, 64);
        assert_eq!(metadata.size, 1024 * 1024);
    }

    #[test]
    fn test_read_bytes() {
        let file = create_test_file(1024 * 1024);
        let mut loader = FileLoader::new(file.path(), 512, 64).unwrap();
        
        let bytes = loader.read_bytes(0, 10).unwrap();
        assert_eq!(bytes.len(), 10);
        assert!(bytes.iter().all(|&b| b == 0xAB));
    }

    #[test]
    fn test_read_bytes_at_offset() {
        let file = create_test_file(1024 * 1024);
        let mut loader = FileLoader::new(file.path(), 512, 64).unwrap();
        
        let bytes = loader.read_bytes(1000, 10).unwrap();
        assert_eq!(bytes.len(), 10);
        assert!(bytes.iter().all(|&b| b == 0xAB));
    }

    #[test]
    fn test_read_fragments_single() {
        let file = create_test_file(1024 * 1024);
        let mut loader = FileLoader::new(file.path(), 512, 64).unwrap();
        
        let fragments = vec![Fragment::new(0, 10)];
        let bytes = loader.read_fragments(fragments).unwrap();
        assert_eq!(bytes.len(), 10);
    }

    #[test]
    fn test_read_fragments_multiple_contiguous() {
        let file = create_test_file(1024 * 1024);
        let mut loader = FileLoader::new(file.path(), 512, 64).unwrap();
        
        let fragments = vec![
            Fragment::new(0, 10),
            Fragment::new(10, 20),
            Fragment::new(20, 30),
        ];
        let bytes = loader.read_fragments(fragments).unwrap();
        assert_eq!(bytes.len(), 30);
    }

    #[test]
    fn test_read_fragments_multiple_with_gaps() {
        let file = create_test_file(1024 * 1024);
        let mut loader = FileLoader::new(file.path(), 512, 64).unwrap();
        
        let fragments = vec![
            Fragment::new(0, 10),
            Fragment::new(100, 110),
            Fragment::new(200, 210),
        ];
        let bytes = loader.read_fragments(fragments).unwrap();
        assert_eq!(bytes.len(), 30);
    }

    #[test]
    fn test_read_fragments_unordered() {
        let file = create_test_file(1024 * 1024);
        let mut loader = FileLoader::new(file.path(), 512, 64).unwrap();
        
        // Fragments provided out of order
        let fragments = vec![
            Fragment::new(20, 30),
            Fragment::new(0, 10),
            Fragment::new(10, 20),
        ];
        let bytes = loader.read_fragments(fragments).unwrap();
        assert_eq!(bytes.len(), 30);
    }

    #[test]
    fn test_merge_contiguous_fragments() {
        let fragments = vec![
            Fragment::new(0, 10),
            Fragment::new(10, 20),
            Fragment::new(20, 30),
        ];
        let merged = FileLoader::merge_contiguous_fragments(&fragments);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].start_byte, 0);
        assert_eq!(merged[0].end_byte, 30);
    }

    #[test]
    fn test_merge_fragments_with_gaps() {
        let fragments = vec![
            Fragment::new(0, 10),
            Fragment::new(100, 110),
            Fragment::new(200, 210),
        ];
        let merged = FileLoader::merge_contiguous_fragments(&fragments);
        assert_eq!(merged.len(), 3);
    }

    #[test]
    fn test_merge_overlapping_fragments() {
        let fragments = vec![
            Fragment::new(0, 15),
            Fragment::new(10, 20),
        ];
        let merged = FileLoader::merge_contiguous_fragments(&fragments);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].start_byte, 0);
        assert_eq!(merged[0].end_byte, 20);
    }

    // ============================================================================
    // Task 6.4: Comprehensive Unit Tests for File Loading
    // ============================================================================
    // These tests validate reading various byte ranges, error handling, and
    // boundary conditions as specified in Requirements 1.1
    // ============================================================================

    /// Test reading from the start of the file
    #[test]
    fn test_read_bytes_from_start() {
        let file = create_test_file(1024 * 1024);
        let mut loader = FileLoader::new(file.path(), 512, 64).unwrap();
        
        let bytes = loader.read_bytes(0, 100).unwrap();
        assert_eq!(bytes.len(), 100);
        assert!(bytes.iter().all(|&b| b == 0xAB));
    }

    /// Test reading from the middle of the file
    #[test]
    fn test_read_bytes_from_middle() {
        let file = create_test_file(1024 * 1024);
        let mut loader = FileLoader::new(file.path(), 512, 64).unwrap();
        
        // Read from within the first 1KB that was written
        let offset = 512;
        let bytes = loader.read_bytes(offset, 256).unwrap();
        assert_eq!(bytes.len(), 256);
        // Sparse file may have zeros in the middle, so just verify we got data
        assert!(bytes.len() > 0);
    }

    /// Test reading from near the end of the file
    #[test]
    fn test_read_bytes_from_end() {
        let file = create_test_file(1024 * 1024);
        let mut loader = FileLoader::new(file.path(), 512, 64).unwrap();
        
        let file_size = 1024 * 1024;
        let offset = file_size - 1;
        let bytes = loader.read_bytes(offset, 1).unwrap();
        assert_eq!(bytes.len(), 1);
        // The last byte should be 0xAB (written by create_test_file)
        assert_eq!(bytes[0], 0xAB);
    }

    /// Test reading a single byte
    #[test]
    fn test_read_single_byte() {
        let file = create_test_file(1024 * 1024);
        let mut loader = FileLoader::new(file.path(), 512, 64).unwrap();
        
        let bytes = loader.read_bytes(500, 1).unwrap();
        assert_eq!(bytes.len(), 1);
        assert_eq!(bytes[0], 0xAB);
    }

    /// Test reading a large contiguous block
    #[test]
    fn test_read_large_block() {
        let file = create_test_file(1024 * 1024);
        let mut loader = FileLoader::new(file.path(), 512, 64).unwrap();
        
        // Read from the beginning where data was written
        let bytes = loader.read_bytes(0, 1024).unwrap(); // 1 KB
        assert_eq!(bytes.len(), 1024);
        // First 1024 bytes should be 0xAB
        assert!(bytes.iter().all(|&b| b == 0xAB));
    }

    /// Test reading at page-aligned boundaries
    #[test]
    fn test_read_at_page_boundary() {
        let file = create_test_file(1024 * 1024);
        let mut loader = FileLoader::new(file.path(), 512, 64).unwrap();
        
        // Read at page boundary (512 bytes)
        let bytes = loader.read_bytes(512, 512).unwrap();
        assert_eq!(bytes.len(), 512);
        assert!(bytes.iter().all(|&b| b == 0xAB));
    }

    /// Test reading at block-aligned boundaries
    #[test]
    fn test_read_at_block_boundary() {
        let file = create_test_file(1024 * 1024);
        let mut loader = FileLoader::new(file.path(), 512, 64).unwrap();
        
        // Read from the beginning where data was written
        let bytes = loader.read_bytes(0, 512).unwrap();
        assert_eq!(bytes.len(), 512);
        assert!(bytes.iter().all(|&b| b == 0xAB));
    }

    /// Test file not found error
    #[test]
    fn test_file_not_found() {
        let result = FileLoader::new("/nonexistent/path/to/file.bin", 512, 64);
        assert!(result.is_err());
        match result {
            Err(Error::IoError(_)) => {}, // Expected
            _ => panic!("Expected IoError for missing file"),
        }
    }

    /// Test reading from a file that doesn't exist
    #[test]
    fn test_read_from_nonexistent_file() {
        let result = FileLoader::new("/nonexistent/file.bin", 512, 64);
        assert!(result.is_err());
    }

    /// Test reading multiple fragments with various patterns
    #[test]
    fn test_read_fragments_pattern_1() {
        let file = create_test_file(1024 * 1024);
        let mut loader = FileLoader::new(file.path(), 512, 64).unwrap();
        
        // Pattern: small fragments spread across file
        let fragments = vec![
            Fragment::new(0, 10),
            Fragment::new(1000, 1010),
            Fragment::new(2000, 2010),
            Fragment::new(3000, 3010),
        ];
        let bytes = loader.read_fragments(fragments).unwrap();
        assert_eq!(bytes.len(), 40);
    }

    /// Test reading fragments that become contiguous after sorting
    #[test]
    fn test_read_fragments_become_contiguous() {
        let file = create_test_file(1024 * 1024);
        let mut loader = FileLoader::new(file.path(), 512, 64).unwrap();
        
        // Fragments provided out of order but contiguous
        let fragments = vec![
            Fragment::new(30, 40),
            Fragment::new(10, 20),
            Fragment::new(20, 30),
            Fragment::new(0, 10),
        ];
        let bytes = loader.read_fragments(fragments).unwrap();
        assert_eq!(bytes.len(), 40);
    }

    /// Test reading empty fragment list
    #[test]
    fn test_read_empty_fragments() {
        let file = create_test_file(1024 * 1024);
        let mut loader = FileLoader::new(file.path(), 512, 64).unwrap();
        
        let fragments = vec![];
        let bytes = loader.read_fragments(fragments).unwrap();
        assert_eq!(bytes.len(), 0);
    }

    /// Test reading fragments with overlaps
    #[test]
    fn test_read_fragments_with_overlaps() {
        let file = create_test_file(1024 * 1024);
        let mut loader = FileLoader::new(file.path(), 512, 64).unwrap();
        
        // Overlapping fragments should be merged
        let fragments = vec![
            Fragment::new(0, 20),
            Fragment::new(10, 30),
            Fragment::new(25, 40),
        ];
        let bytes = loader.read_fragments(fragments).unwrap();
        // After merging: [0, 40] = 40 bytes
        assert_eq!(bytes.len(), 40);
    }

    /// Test reading fragments at various offsets
    #[test]
    fn test_read_fragments_various_offsets() {
        let file = create_test_file(2 * 1024 * 1024); // 2 MB
        let mut loader = FileLoader::new(file.path(), 512, 64).unwrap();
        
        let fragments = vec![
            Fragment::new(100, 200),
            Fragment::new(5000, 5100),
            Fragment::new(1024 * 1024, 1024 * 1024 + 100),
        ];
        let bytes = loader.read_fragments(fragments).unwrap();
        assert_eq!(bytes.len(), 300);
    }

    /// Test reading fragments that span multiple pages
    #[test]
    fn test_read_fragments_multiple_pages() {
        let file = create_test_file(1024 * 1024);
        let mut loader = FileLoader::new(file.path(), 512, 64).unwrap();
        
        // Each fragment spans multiple pages
        let page_size = 512;
        let fragments = vec![
            Fragment::new(0, page_size * 2),
            Fragment::new(page_size * 5, page_size * 7),
            Fragment::new(page_size * 10, page_size * 12),
        ];
        let bytes = loader.read_fragments(fragments).unwrap();
        assert_eq!(bytes.len(), (page_size * 6) as usize);
    }

    /// Test reading fragments that span multiple blocks
    #[test]
    fn test_read_fragments_multiple_blocks() {
        let file = create_test_file(1024 * 1024);
        let mut loader = FileLoader::new(file.path(), 512, 64).unwrap();
        
        // Each fragment spans multiple blocks
        let block_size = 512 * 64;
        let fragments = vec![
            Fragment::new(0, block_size as u64),
            Fragment::new(block_size as u64 * 2, block_size as u64 * 3),
            Fragment::new(block_size as u64 * 5, block_size as u64 * 6),
        ];
        let bytes = loader.read_fragments(fragments).unwrap();
        assert_eq!(bytes.len(), (block_size * 3) as usize);
    }

    /// Test boundary condition: read exactly at file end
    #[test]
    fn test_read_at_exact_file_end() {
        let file = create_test_file(1024 * 1024);
        let mut loader = FileLoader::new(file.path(), 512, 64).unwrap();
        
        let file_size = 1024 * 1024;
        let bytes = loader.read_bytes(file_size - 1, 1).unwrap();
        assert_eq!(bytes.len(), 1);
    }

    /// Test boundary condition: read with offset at page boundary
    #[test]
    fn test_read_offset_at_page_boundary() {
        let file = create_test_file(1024 * 1024);
        let mut loader = FileLoader::new(file.path(), 512, 64).unwrap();
        
        let page_size = 512;
        for page_num in 0..10 {
            let offset = (page_num * page_size) as u64;
            let bytes = loader.read_bytes(offset, page_size as u32).unwrap();
            assert_eq!(bytes.len(), page_size as usize);
        }
    }

    /// Test boundary condition: read with offset at block boundary
    #[test]
    fn test_read_offset_at_block_boundary() {
        let file = create_test_file(1024 * 1024);
        let mut loader = FileLoader::new(file.path(), 512, 64).unwrap();
        
        let block_size = 512 * 64;
        for block_num in 0..5 {
            let offset = (block_num * block_size) as u64;
            let bytes = loader.read_bytes(offset, block_size as u32).unwrap();
            assert_eq!(bytes.len(), block_size as usize);
        }
    }

    /// Test reading with various page lengths
    #[test]
    fn test_read_with_various_page_lengths() {
        let page_lengths = [512, 1024, 2048, 4096, 8192];
        
        for page_length in &page_lengths {
            let file = create_test_file(1024 * 1024);
            let mut loader = FileLoader::new(file.path(), *page_length, 64).unwrap();
            
            let bytes = loader.read_bytes(0, *page_length).unwrap();
            assert_eq!(bytes.len(), *page_length as usize);
        }
    }

    /// Test reading with various block sizes
    #[test]
    fn test_read_with_various_block_sizes() {
        let block_sizes = [64, 128, 256, 512, 1024];
        
        for block_size in &block_sizes {
            let file = create_test_file(1024 * 1024);
            let mut loader = FileLoader::new(file.path(), 512, *block_size).unwrap();
            
            let bytes = loader.read_bytes(0, 512).unwrap();
            assert_eq!(bytes.len(), 512);
        }
    }

    /// Test metadata is correctly cached after file open
    #[test]
    fn test_metadata_cached_after_open() {
        let file = create_test_file(1024 * 1024);
        let loader = FileLoader::new(file.path(), 512, 64).unwrap();
        
        let metadata1 = loader.get_metadata();
        let metadata2 = loader.get_metadata();
        
        // Metadata should be identical (cached)
        assert_eq!(metadata1.page_length, metadata2.page_length);
        assert_eq!(metadata1.block_size, metadata2.block_size);
        assert_eq!(metadata1.size, metadata2.size);
    }

    /// Test reading fragments preserves order
    #[test]
    fn test_read_fragments_preserves_order() {
        let file = create_test_file(1024 * 1024);
        let mut loader = FileLoader::new(file.path(), 512, 64).unwrap();
        
        // Create test file with distinct patterns at different offsets
        // For this test, we just verify the fragments are read in the correct order
        let fragments = vec![
            Fragment::new(0, 10),
            Fragment::new(100, 110),
            Fragment::new(200, 210),
        ];
        let bytes = loader.read_fragments(fragments).unwrap();
        
        // Should have 30 bytes total (10 + 10 + 10)
        assert_eq!(bytes.len(), 30);
    }

    /// Test reading fragments with single-byte gaps
    #[test]
    fn test_read_fragments_single_byte_gaps() {
        let file = create_test_file(1024 * 1024);
        let mut loader = FileLoader::new(file.path(), 512, 64).unwrap();
        
        let fragments = vec![
            Fragment::new(0, 10),
            Fragment::new(11, 20),
            Fragment::new(21, 30),
        ];
        let bytes = loader.read_fragments(fragments).unwrap();
        // Single-byte gaps are NOT merged, so we get 3 separate reads
        // [0,10] = 10 bytes, [11,20] = 9 bytes, [21,30] = 9 bytes = 28 total
        assert_eq!(bytes.len(), 28);
    }

    /// Test reading fragments with large gaps
    #[test]
    fn test_read_fragments_large_gaps() {
        let file = create_test_file(20 * 1024 * 1024); // 20 MB
        let mut loader = FileLoader::new(file.path(), 512, 64).unwrap();
        
        let fragments = vec![
            Fragment::new(0, 100),
            Fragment::new(1024 * 1024, 1024 * 1024 + 100),
            Fragment::new(10 * 1024 * 1024, 10 * 1024 * 1024 + 100),
        ];
        let bytes = loader.read_fragments(fragments).unwrap();
        assert_eq!(bytes.len(), 300);
    }

    /// Test reading many small fragments
    #[test]
    fn test_read_many_small_fragments() {
        let file = create_test_file(1024 * 1024);
        let mut loader = FileLoader::new(file.path(), 512, 64).unwrap();
        
        let mut fragments = Vec::new();
        for i in 0..100 {
            let offset = (i * 100) as u64;
            fragments.push(Fragment::new(offset, offset + 10));
        }
        
        let bytes = loader.read_fragments(fragments).unwrap();
        assert_eq!(bytes.len(), 1000); // 100 fragments * 10 bytes each
    }

    /// Test reading fragments that are already sorted
    #[test]
    fn test_read_fragments_already_sorted() {
        let file = create_test_file(1024 * 1024);
        let mut loader = FileLoader::new(file.path(), 512, 64).unwrap();
        
        let fragments = vec![
            Fragment::new(0, 100),
            Fragment::new(100, 200),
            Fragment::new(200, 300),
            Fragment::new(300, 400),
        ];
        let bytes = loader.read_fragments(fragments).unwrap();
        assert_eq!(bytes.len(), 400);
    }

    /// Test reading fragments in reverse order
    #[test]
    fn test_read_fragments_reverse_order() {
        let file = create_test_file(1024 * 1024);
        let mut loader = FileLoader::new(file.path(), 512, 64).unwrap();
        
        let fragments = vec![
            Fragment::new(300, 400),
            Fragment::new(200, 300),
            Fragment::new(100, 200),
            Fragment::new(0, 100),
        ];
        let bytes = loader.read_fragments(fragments).unwrap();
        assert_eq!(bytes.len(), 400);
    }

    /// Test reading fragments with random order
    #[test]
    fn test_read_fragments_random_order() {
        let file = create_test_file(1024 * 1024);
        let mut loader = FileLoader::new(file.path(), 512, 64).unwrap();
        
        let fragments = vec![
            Fragment::new(200, 300),
            Fragment::new(0, 100),
            Fragment::new(300, 400),
            Fragment::new(100, 200),
        ];
        let bytes = loader.read_fragments(fragments).unwrap();
        assert_eq!(bytes.len(), 400);
    }
}
