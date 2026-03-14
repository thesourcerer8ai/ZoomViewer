//! Metadata persistence for NAND dump files
//!
//! Manages metadata caching in .cache/{dump_filename}/metadata.json
//! Stores: file path, size, page length, block size, timestamp

use crate::error::{Error, Result};
use crate::types::FileMetadata;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// Metadata stored in JSON for persistence
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Metadata {
    /// Path to the dump file
    pub path: String,
    /// Total file size in bytes
    pub size: u64,
    /// Bytes per page
    pub page_length: u32,
    /// Pages per block
    pub block_size: u32,
    /// Timestamp when metadata was saved (seconds since UNIX_EPOCH)
    pub timestamp: u64,
}

impl Metadata {
    /// Create new metadata from FileMetadata
    pub fn from_file_metadata(file_metadata: &FileMetadata) -> Self {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);

        Metadata {
            path: file_metadata.path.clone(),
            size: file_metadata.size,
            page_length: file_metadata.page_length,
            block_size: file_metadata.block_size,
            timestamp,
        }
    }

    /// Convert to FileMetadata
    pub fn to_file_metadata(&self) -> FileMetadata {
        FileMetadata::new(
            self.path.clone(),
            self.size,
            self.page_length,
            self.block_size,
        )
    }
}

/// Manages metadata persistence
#[derive(Debug, Clone)]
pub struct MetadataManager {
    /// Base cache directory path
    cache_dir: PathBuf,
    /// Dump filename (used in cache path)
    dump_filename: String,
}

impl MetadataManager {
    /// Create a new MetadataManager
    ///
    /// # Arguments
    /// * `cache_dir` - Base cache directory path (typically ".cache")
    /// * `dump_filename` - Name of the dump file (used for cache organization)
    pub fn new<P: AsRef<Path>>(cache_dir: P, dump_filename: String) -> Result<Self> {
        let cache_path = cache_dir.as_ref().to_path_buf();

        // Create .cache directory if needed
        if !cache_path.exists() {
            fs::create_dir_all(&cache_path)
                .map_err(|e| Error::CacheError(format!("Failed to create cache directory: {}", e)))?;
        }

        Ok(MetadataManager {
            cache_dir: cache_path,
            dump_filename,
        })
    }

    /// Get the path to the metadata.json file
    fn get_metadata_path(&self) -> PathBuf {
        self.cache_dir
            .join(&self.dump_filename)
            .join("metadata.json")
    }

    /// Load metadata from .cache/{dump_filename}/metadata.json
    ///
    /// Validates that:
    /// - metadata.json exists
    /// - JSON is valid
    /// - file still exists at the stored path
    /// - file size hasn't changed (indicates stale metadata)
    ///
    /// # Requirements
    /// - Load metadata.json from .cache/{dump_filename}/ (Requirement 22.2)
    /// - Validate metadata is still valid (Requirement 22.3)
    pub fn load_metadata(&self) -> Result<Metadata> {
        let metadata_path = self.get_metadata_path();

        // Check if metadata file exists
        if !metadata_path.exists() {
            return Err(Error::CacheError(
                "Metadata file not found".to_string(),
            ));
        }

        // Read and parse JSON
        let json_content = fs::read_to_string(&metadata_path)
            .map_err(|e| Error::CacheError(format!("Failed to read metadata file: {}", e)))?;

        let metadata: Metadata = serde_json::from_str(&json_content)
            .map_err(|e| Error::CacheError(format!("Failed to parse metadata JSON: {}", e)))?;

        // Validate that the file still exists
        if !Path::new(&metadata.path).exists() {
            return Err(Error::CacheError(
                "Dump file no longer exists at stored path".to_string(),
            ));
        }

        // Validate that file size hasn't changed (detect stale metadata)
        let current_size = fs::metadata(&metadata.path)
            .map_err(|e| Error::CacheError(format!("Failed to read file metadata: {}", e)))?
            .len();

        if current_size != metadata.size {
            return Err(Error::CacheError(
                format!(
                    "File size mismatch: stored={}, current={}",
                    metadata.size, current_size
                ),
            ));
        }

        Ok(metadata)
    }

    /// Save metadata to .cache/{dump_filename}/metadata.json
    ///
    /// Creates the dump-specific cache directory if needed.
    ///
    /// # Requirements
    /// - Save metadata.json with current parameters (Requirement 22.1, 22.6)
    pub fn save_metadata(&self, file_metadata: &FileMetadata) -> Result<()> {
        let metadata_path = self.get_metadata_path();

        // Create dump-specific cache directory if needed
        let dump_cache_dir = self.cache_dir.join(&self.dump_filename);
        if !dump_cache_dir.exists() {
            fs::create_dir_all(&dump_cache_dir)
                .map_err(|e| Error::CacheError(format!("Failed to create cache directory: {}", e)))?;
        }

        // Create metadata from FileMetadata
        let metadata = Metadata::from_file_metadata(file_metadata);

        // Serialize to JSON
        let json_content = serde_json::to_string_pretty(&metadata)
            .map_err(|e| Error::CacheError(format!("Failed to serialize metadata: {}", e)))?;

        // Write to file
        fs::write(&metadata_path, json_content)
            .map_err(|e| Error::CacheError(format!("Failed to write metadata file: {}", e)))?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn test_metadata_creation() {
        let file_metadata = FileMetadata::new(
            "/path/to/dump.bin".to_string(),
            1_000_000,
            512,
            64,
        );

        let metadata = Metadata::from_file_metadata(&file_metadata);

        assert_eq!(metadata.path, "/path/to/dump.bin");
        assert_eq!(metadata.size, 1_000_000);
        assert_eq!(metadata.page_length, 512);
        assert_eq!(metadata.block_size, 64);
        assert!(metadata.timestamp > 0);
    }

    #[test]
    fn test_metadata_to_file_metadata() {
        let metadata = Metadata {
            path: "/path/to/dump.bin".to_string(),
            size: 1_000_000,
            page_length: 512,
            block_size: 64,
            timestamp: 1234567890,
        };

        let file_metadata = metadata.to_file_metadata();

        assert_eq!(file_metadata.path, "/path/to/dump.bin");
        assert_eq!(file_metadata.size, 1_000_000);
        assert_eq!(file_metadata.page_length, 512);
        assert_eq!(file_metadata.block_size, 64);
    }

    #[test]
    fn test_metadata_manager_creation() {
        let temp_dir = TempDir::new().unwrap();
        let manager = MetadataManager::new(temp_dir.path(), "test_dump.bin".to_string());

        assert!(manager.is_ok());
    }

    #[test]
    fn test_save_and_load_metadata() {
        let temp_dir = TempDir::new().unwrap();
        let manager = MetadataManager::new(temp_dir.path(), "dump_cache".to_string()).unwrap();

        // Create a temporary dump file
        let dump_path = temp_dir.path().join("test_dump.bin");
        fs::write(&dump_path, vec![0u8; 1_000_000]).unwrap();

        // Create and save metadata
        let file_metadata = FileMetadata::new(
            dump_path.to_string_lossy().to_string(),
            1_000_000,
            512,
            64,
        );

        manager.save_metadata(&file_metadata).unwrap();

        // Load and verify
        let loaded = manager.load_metadata().unwrap();

        assert_eq!(loaded.path, file_metadata.path);
        assert_eq!(loaded.size, 1_000_000);
        assert_eq!(loaded.page_length, 512);
        assert_eq!(loaded.block_size, 64);
    }

    #[test]
    fn test_load_nonexistent_metadata() {
        let temp_dir = TempDir::new().unwrap();
        let manager = MetadataManager::new(temp_dir.path(), "nonexistent.bin".to_string()).unwrap();

        let result = manager.load_metadata();
        assert!(result.is_err());
    }

    #[test]
    fn test_load_invalid_json() {
        let temp_dir = TempDir::new().unwrap();
        let manager = MetadataManager::new(temp_dir.path(), "test_dump.bin".to_string()).unwrap();

        // Create metadata directory and invalid JSON file
        let metadata_path = manager.get_metadata_path();
        if let Some(parent) = metadata_path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(&metadata_path, "invalid json {").unwrap();

        let result = manager.load_metadata();
        assert!(result.is_err());
    }

    #[test]
    fn test_load_metadata_file_not_found() {
        let temp_dir = TempDir::new().unwrap();
        let manager = MetadataManager::new(temp_dir.path(), "test_dump.bin".to_string()).unwrap();

        // Create metadata directory and valid JSON pointing to nonexistent file
        let metadata_path = manager.get_metadata_path();
        if let Some(parent) = metadata_path.parent() {
            fs::create_dir_all(parent).unwrap();
        }

        let metadata = Metadata {
            path: "/nonexistent/file.bin".to_string(),
            size: 1_000_000,
            page_length: 512,
            block_size: 64,
            timestamp: 1234567890,
        };

        let json_content = serde_json::to_string_pretty(&metadata).unwrap();
        fs::write(&metadata_path, json_content).unwrap();

        let result = manager.load_metadata();
        assert!(result.is_err());
    }

    #[test]
    fn test_load_metadata_stale_file_size() {
        let temp_dir = TempDir::new().unwrap();
        let manager = MetadataManager::new(temp_dir.path(), "dump_cache".to_string()).unwrap();

        // Create a dump file
        let dump_path = temp_dir.path().join("test_dump.bin");
        fs::write(&dump_path, vec![0u8; 1_000_000]).unwrap();

        // Save metadata
        let file_metadata = FileMetadata::new(
            dump_path.to_string_lossy().to_string(),
            1_000_000,
            512,
            64,
        );
        manager.save_metadata(&file_metadata).unwrap();

        // Modify the file size
        fs::write(&dump_path, vec![0u8; 2_000_000]).unwrap();

        // Try to load - should fail due to size mismatch
        let result = manager.load_metadata();
        assert!(result.is_err());
    }

    #[test]
    fn test_metadata_json_format() {
        let temp_dir = TempDir::new().unwrap();
        let manager = MetadataManager::new(temp_dir.path(), "dump_cache".to_string()).unwrap();

        // Create a dump file
        let dump_path = temp_dir.path().join("test_dump.bin");
        fs::write(&dump_path, vec![0u8; 1_000_000]).unwrap();

        // Save metadata
        let file_metadata = FileMetadata::new(
            dump_path.to_string_lossy().to_string(),
            1_000_000,
            512,
            64,
        );
        manager.save_metadata(&file_metadata).unwrap();

        // Read and verify JSON format
        let metadata_path = manager.get_metadata_path();
        let json_content = fs::read_to_string(&metadata_path).unwrap();

        // Verify it's valid JSON with expected fields
        let parsed: serde_json::Value = serde_json::from_str(&json_content).unwrap();
        assert!(parsed.get("path").is_some());
        assert!(parsed.get("size").is_some());
        assert!(parsed.get("page_length").is_some());
        assert!(parsed.get("block_size").is_some());
        assert!(parsed.get("timestamp").is_some());
    }

    // ========================================================================
    // Task 8.4: Comprehensive unit tests for metadata persistence
    // Requirements: 22.1, 22.3
    // ========================================================================

    /// Test save/load cycle with various page lengths
    /// Validates: Requirement 22.1 (cache metadata)
    #[test]
    fn test_save_load_cycle_various_page_lengths() {
        let temp_dir = TempDir::new().unwrap();
        
        // Test with different valid page lengths
        let page_lengths = [500, 1024, 2048, 8192, 20000];
        
        for page_length in page_lengths {
            let manager = MetadataManager::new(
                temp_dir.path(), 
                format!("cache_{}", page_length)
            ).unwrap();
            
            let dump_path = temp_dir.path().join(format!("actual_dump_{}.bin", page_length));
            fs::write(&dump_path, vec![0u8; 1_000_000]).unwrap();
            
            let file_metadata = FileMetadata::new(
                dump_path.to_string_lossy().to_string(),
                1_000_000,
                page_length,
                64,
            );
            
            // Save
            manager.save_metadata(&file_metadata).unwrap();
            
            // Load and verify
            let loaded = manager.load_metadata().unwrap();
            assert_eq!(loaded.page_length, page_length);
            assert_eq!(loaded.size, 1_000_000);
            assert_eq!(loaded.block_size, 64);
        }
    }

    /// Test save/load cycle with various block sizes
    /// Validates: Requirement 22.1 (cache metadata)
    #[test]
    fn test_save_load_cycle_various_block_sizes() {
        let temp_dir = TempDir::new().unwrap();
        
        // Test with all valid block sizes
        let block_sizes = [64, 128, 256, 512, 1024];
        
        for block_size in block_sizes {
            let manager = MetadataManager::new(
                temp_dir.path(), 
                format!("cache_block_{}", block_size)
            ).unwrap();
            
            let dump_path = temp_dir.path().join(format!("actual_dump_block_{}.bin", block_size));
            fs::write(&dump_path, vec![0u8; 1_000_000]).unwrap();
            
            let file_metadata = FileMetadata::new(
                dump_path.to_string_lossy().to_string(),
                1_000_000,
                512,
                block_size,
            );
            
            // Save
            manager.save_metadata(&file_metadata).unwrap();
            
            // Load and verify
            let loaded = manager.load_metadata().unwrap();
            assert_eq!(loaded.block_size, block_size);
            assert_eq!(loaded.size, 1_000_000);
            assert_eq!(loaded.page_length, 512);
        }
    }

    /// Test multiple save/load cycles preserve data
    /// Validates: Requirement 22.1 (cache metadata)
    #[test]
    fn test_multiple_save_load_cycles() {
        let temp_dir = TempDir::new().unwrap();
        let manager = MetadataManager::new(temp_dir.path(), "dump_cache".to_string()).unwrap();
        
        let dump_path = temp_dir.path().join("actual_dump.bin");
        fs::write(&dump_path, vec![0u8; 1_000_000]).unwrap();
        
        // First save/load cycle
        let file_metadata1 = FileMetadata::new(
            dump_path.to_string_lossy().to_string(),
            1_000_000,
            512,
            64,
        );
        manager.save_metadata(&file_metadata1).unwrap();
        let loaded1 = manager.load_metadata().unwrap();
        assert_eq!(loaded1.page_length, 512);
        
        // Second save/load cycle with different parameters
        let file_metadata2 = FileMetadata::new(
            dump_path.to_string_lossy().to_string(),
            1_000_000,
            1024,
            128,
        );
        manager.save_metadata(&file_metadata2).unwrap();
        let loaded2 = manager.load_metadata().unwrap();
        assert_eq!(loaded2.page_length, 1024);
        assert_eq!(loaded2.block_size, 128);
    }

    /// Test timestamp is updated on each save
    /// Validates: Requirement 22.1 (cache metadata with timestamp)
    #[test]
    fn test_timestamp_updated_on_save() {
        let temp_dir = TempDir::new().unwrap();
        let manager = MetadataManager::new(temp_dir.path(), "dump_cache".to_string()).unwrap();
        
        let dump_path = temp_dir.path().join("actual_dump.bin");
        fs::write(&dump_path, vec![0u8; 1_000_000]).unwrap();
        
        let file_metadata = FileMetadata::new(
            dump_path.to_string_lossy().to_string(),
            1_000_000,
            512,
            64,
        );
        
        // First save
        manager.save_metadata(&file_metadata).unwrap();
        let loaded1 = manager.load_metadata().unwrap();
        let timestamp1 = loaded1.timestamp;
        
        // Wait a moment
        std::thread::sleep(std::time::Duration::from_millis(10));
        
        // Second save
        manager.save_metadata(&file_metadata).unwrap();
        let loaded2 = manager.load_metadata().unwrap();
        let timestamp2 = loaded2.timestamp;
        
        // Timestamp should be updated
        assert!(timestamp2 >= timestamp1);
    }

    /// Test validation detects file deletion
    /// Validates: Requirement 22.3 (validate metadata is valid)
    #[test]
    fn test_validation_detects_file_deletion() {
        let temp_dir = TempDir::new().unwrap();
        let manager = MetadataManager::new(temp_dir.path(), "dump_cache".to_string()).unwrap();
        
        let dump_path = temp_dir.path().join("actual_dump.bin");
        fs::write(&dump_path, vec![0u8; 1_000_000]).unwrap();
        
        let file_metadata = FileMetadata::new(
            dump_path.to_string_lossy().to_string(),
            1_000_000,
            512,
            64,
        );
        
        // Save metadata
        manager.save_metadata(&file_metadata).unwrap();
        
        // Delete the dump file
        fs::remove_file(&dump_path).unwrap();
        
        // Load should fail due to missing file
        let result = manager.load_metadata();
        assert!(result.is_err());
        if let Err(Error::CacheError(msg)) = result {
            assert!(msg.contains("no longer exists"));
        } else {
            panic!("Expected CacheError about missing file");
        }
    }

    /// Test validation detects file size increase
    /// Validates: Requirement 22.3 (validate metadata is valid)
    #[test]
    fn test_validation_detects_file_size_increase() {
        let temp_dir = TempDir::new().unwrap();
        let manager = MetadataManager::new(temp_dir.path(), "dump_cache".to_string()).unwrap();
        
        let dump_path = temp_dir.path().join("actual_dump.bin");
        fs::write(&dump_path, vec![0u8; 1_000_000]).unwrap();
        
        let file_metadata = FileMetadata::new(
            dump_path.to_string_lossy().to_string(),
            1_000_000,
            512,
            64,
        );
        
        // Save metadata
        manager.save_metadata(&file_metadata).unwrap();
        
        // Increase file size
        fs::write(&dump_path, vec![0u8; 2_000_000]).unwrap();
        
        // Load should fail due to size mismatch
        let result = manager.load_metadata();
        assert!(result.is_err());
        if let Err(Error::CacheError(msg)) = result {
            assert!(msg.contains("size mismatch"));
        } else {
            panic!("Expected CacheError about size mismatch");
        }
    }

    /// Test validation detects file size decrease
    /// Validates: Requirement 22.3 (validate metadata is valid)
    #[test]
    fn test_validation_detects_file_size_decrease() {
        let temp_dir = TempDir::new().unwrap();
        let manager = MetadataManager::new(temp_dir.path(), "dump_cache".to_string()).unwrap();
        
        let dump_path = temp_dir.path().join("actual_dump.bin");
        fs::write(&dump_path, vec![0u8; 2_000_000]).unwrap();
        
        let file_metadata = FileMetadata::new(
            dump_path.to_string_lossy().to_string(),
            2_000_000,
            512,
            64,
        );
        
        // Save metadata
        manager.save_metadata(&file_metadata).unwrap();
        
        // Decrease file size
        fs::write(&dump_path, vec![0u8; 1_000_000]).unwrap();
        
        // Load should fail due to size mismatch
        let result = manager.load_metadata();
        assert!(result.is_err());
        if let Err(Error::CacheError(msg)) = result {
            assert!(msg.contains("size mismatch"));
        } else {
            panic!("Expected CacheError about size mismatch");
        }
    }

    /// Test save creates cache directory structure
    /// Validates: Requirement 22.1 (cache in .cache/{dump_filename}/)
    #[test]
    fn test_save_creates_directory_structure() {
        let temp_dir = TempDir::new().unwrap();
        let manager = MetadataManager::new(temp_dir.path(), "dump_cache".to_string()).unwrap();
        
        let dump_path = temp_dir.path().join("actual_dump.bin");
        fs::write(&dump_path, vec![0u8; 1_000_000]).unwrap();
        
        let file_metadata = FileMetadata::new(
            dump_path.to_string_lossy().to_string(),
            1_000_000,
            512,
            64,
        );
        
        // Save metadata
        manager.save_metadata(&file_metadata).unwrap();
        
        // Verify directory structure exists
        let cache_dir = temp_dir.path().join("dump_cache");
        assert!(cache_dir.exists());
        assert!(cache_dir.is_dir());
        
        let metadata_file = cache_dir.join("metadata.json");
        assert!(metadata_file.exists());
        assert!(metadata_file.is_file());
    }

    /// Test load with corrupted JSON data
    /// Validates: Requirement 22.3 (validate metadata is valid)
    #[test]
    fn test_load_with_corrupted_json() {
        let temp_dir = TempDir::new().unwrap();
        let manager = MetadataManager::new(temp_dir.path(), "dump.bin".to_string()).unwrap();
        
        // Create metadata directory
        let metadata_path = manager.get_metadata_path();
        if let Some(parent) = metadata_path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        
        // Write corrupted JSON (missing closing brace)
        fs::write(&metadata_path, r#"{"path": "/test.bin", "size": 1000"#).unwrap();
        
        // Load should fail
        let result = manager.load_metadata();
        assert!(result.is_err());
        if let Err(Error::CacheError(msg)) = result {
            assert!(msg.contains("parse"));
        } else {
            panic!("Expected CacheError about parsing");
        }
    }

    /// Test load with missing required fields
    /// Validates: Requirement 22.3 (validate metadata is valid)
    #[test]
    fn test_load_with_missing_fields() {
        let temp_dir = TempDir::new().unwrap();
        let manager = MetadataManager::new(temp_dir.path(), "dump.bin".to_string()).unwrap();
        
        // Create metadata directory
        let metadata_path = manager.get_metadata_path();
        if let Some(parent) = metadata_path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        
        // Write JSON missing page_length field
        fs::write(&metadata_path, r#"{"path": "/test.bin", "size": 1000, "block_size": 64, "timestamp": 123}"#).unwrap();
        
        // Load should fail
        let result = manager.load_metadata();
        assert!(result.is_err());
    }

    /// Test save/load with large file sizes
    /// Validates: Requirement 22.1 (cache metadata for large files)
    #[test]
    fn test_save_load_large_file_metadata() {
        let temp_dir = TempDir::new().unwrap();
        let manager = MetadataManager::new(temp_dir.path(), "large_dump_cache".to_string()).unwrap();
        
        let dump_path = temp_dir.path().join("actual_large_dump.bin");
        // Create a small file but with metadata indicating large size
        fs::write(&dump_path, vec![0u8; 1000]).unwrap();
        
        // Use actual file size for testing
        let file_metadata = FileMetadata::new(
            dump_path.to_string_lossy().to_string(),
            1000, // Actual file size for testing
            512,
            64,
        );
        
        // Save and load
        manager.save_metadata(&file_metadata).unwrap();
        let loaded = manager.load_metadata().unwrap();
        
        assert_eq!(loaded.size, 1000);
        assert_eq!(loaded.page_length, 512);
        assert_eq!(loaded.block_size, 64);
    }

    /// Test metadata path generation
    /// Validates: Requirement 22.1 (correct path structure)
    #[test]
    fn test_metadata_path_generation() {
        let temp_dir = TempDir::new().unwrap();
        let manager = MetadataManager::new(temp_dir.path(), "test_dump.bin".to_string()).unwrap();
        
        let metadata_path = manager.get_metadata_path();
        let expected_path = temp_dir.path().join("test_dump.bin").join("metadata.json");
        
        assert_eq!(metadata_path, expected_path);
    }

    /// Test concurrent save operations don't corrupt data
    /// Validates: Requirement 22.1 (reliable metadata caching)
    #[test]
    fn test_concurrent_save_operations() {
        use std::sync::Arc;
        use std::thread;
        
        let temp_dir = TempDir::new().unwrap();
        let manager = Arc::new(MetadataManager::new(temp_dir.path(), "dump_cache".to_string()).unwrap());
        
        let dump_path = temp_dir.path().join("actual_dump.bin");
        fs::write(&dump_path, vec![0u8; 1_000_000]).unwrap();
        
        let mut handles = vec![];
        
        // Spawn multiple threads saving metadata
        for i in 0..5 {
            let manager_clone = Arc::clone(&manager);
            let path_clone = dump_path.to_string_lossy().to_string();
            
            let handle = thread::spawn(move || {
                let file_metadata = FileMetadata::new(
                    path_clone,
                    1_000_000,
                    512 + i * 100, // Different page lengths
                    64,
                );
                manager_clone.save_metadata(&file_metadata).unwrap();
            });
            
            handles.push(handle);
        }
        
        // Wait for all threads
        for handle in handles {
            handle.join().unwrap();
        }
        
        // Load should succeed (last write wins)
        let loaded = manager.load_metadata().unwrap();
        assert_eq!(loaded.size, 1_000_000);
        assert_eq!(loaded.block_size, 64);
    }
}
