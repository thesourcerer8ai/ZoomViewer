//! Cache management for hierarchical tile storage
//!
//! Manages a hierarchical cache directory structure:
//! .cache/{dump_filename}/{level}/{block_y}/{block_x}.qoi

use crate::error::{Error, Result};
use crate::types::TileCoord;
use std::fs;
use std::path::{Path, PathBuf};

/// Manages hierarchical cache directory structure for tiles
#[derive(Debug, Clone)]
pub struct CacheManager {
    /// Base cache directory path
    cache_dir: PathBuf,
    /// Dump filename (used in cache path)
    dump_filename: String,
}

impl CacheManager {
    /// Create a new CacheManager and ensure .cache directory exists
    ///
    /// # Arguments
    /// * `cache_dir` - Base cache directory path (typically ".cache")
    /// * `dump_filename` - Name of the dump file (used for cache organization)
    ///
    /// # Requirements
    /// - Creates .cache directory if it doesn't exist (Requirement 8.1, 19.1)
    pub fn new<P: AsRef<Path>>(cache_dir: P, dump_filename: String) -> Result<Self> {
        let cache_path = cache_dir.as_ref().to_path_buf();
        
        // Create .cache directory if needed
        if !cache_path.exists() {
            fs::create_dir_all(&cache_path)
                .map_err(|e| Error::CacheError(format!("Failed to create cache directory: {}", e)))?;
        }
        
        Ok(CacheManager {
            cache_dir: cache_path,
            dump_filename,
        })
    }
    
    /// Get the full path for a tile in the cache
    ///
    /// Returns: .cache/{dump_filename}/{level}/{block_y}/{block_x}.qoi
    ///
    /// # Requirements
    /// - Returns hierarchical path structure (Requirement 8.2, 19.2, 19.3)
    pub fn get_tile_path(&self, coord: &TileCoord) -> PathBuf {
        self.cache_dir
            .join(&self.dump_filename)
            .join(coord.level.to_string())
            .join(coord.y.to_string())
            .join(format!("{}.qoi", coord.x))
    }
    
    /// Check if a tile exists in the cache
    ///
    /// # Requirements
    /// - Checks if tile file exists (Requirement 8.3)
    pub fn tile_exists(&self, coord: &TileCoord) -> bool {
        self.get_tile_path(coord).exists()
    }
    
    /// Load a QOI tile from cache
    ///
    /// # Requirements
    /// - Loads QOI from cache (Requirement 8.4)
    /// - Validates QOI integrity
    pub fn load_tile(&self, coord: &TileCoord) -> Result<Vec<u8>> {
        let path = self.get_tile_path(coord);
        
        // Read the file
        let data = fs::read(&path)
            .map_err(|e| Error::CacheError(format!("Failed to read tile from cache: {}", e)))?;
        
        // Validate QOI integrity by checking QOI signature
        if data.len() < 14 {
            return Err(Error::CacheError("Invalid QOI: file too small".to_string()));
        }
        
        // QOI files start with magic bytes: "qoif" (0x71 0x6F 0x69 0x66)
        const QOI_SIGNATURE: &[u8] = b"qoif";
        if &data[0..4] != QOI_SIGNATURE {
            return Err(Error::CacheError("Invalid QOI: incorrect signature".to_string()));
        }
        
        Ok(data)
    }
    
    /// Save a QOI tile to cache with atomic writes
    ///
    /// Uses atomic writes (write to temp file, then rename) to prevent
    /// incomplete/corrupted tiles from being cached.
    ///
    /// # Requirements
    /// - Saves QOI to cache (Requirement 8.1, 8.2)
    /// - Creates intermediate directories (Requirement 19.4)
    pub fn save_tile(&self, coord: &TileCoord, qoi_data: &[u8]) -> Result<()> {
        let path = self.get_tile_path(coord);
        
        // Create all intermediate directories
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| Error::CacheError(format!("Failed to create cache directories: {}", e)))?;
        }
        
        // Write to a temporary file first
        let temp_path = path.with_extension("tmp");
        fs::write(&temp_path, qoi_data)
            .map_err(|e| Error::CacheError(format!("Failed to write temporary tile file: {}", e)))?;
        
        // Atomically rename temp file to final location
        // This ensures we never have incomplete/corrupted tiles in the cache
        fs::rename(&temp_path, &path)
            .map_err(|e| Error::CacheError(format!("Failed to finalize tile cache: {}", e)))?;
        
        Ok(())
    }
    
    /// Invalidate the entire cache for this dump
    ///
    /// # Requirements
    /// - Supports cache cleanup (Requirement 19.5)
    pub fn invalidate_cache(&self) -> Result<()> {
        let dump_cache_dir = self.cache_dir.join(&self.dump_filename);
        
        if dump_cache_dir.exists() {
            fs::remove_dir_all(&dump_cache_dir)
                .map_err(|e| Error::CacheError(format!("Failed to invalidate cache: {}", e)))?;
        }
        
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    use proptest::prelude::*;

    // Helper function to create valid QOI data for testing
    fn create_test_qoi_data() -> Vec<u8> {
        let mut qoi_data = b"qoif".to_vec();
        qoi_data.extend_from_slice(&[0x00, 0x00, 0x00, 0x10]); // width (16)
        qoi_data.extend_from_slice(&[0x00, 0x00, 0x00, 0x10]); // height (16)
        qoi_data.push(4); // channels (RGBA)
        qoi_data.push(1); // colorspace (sRGB with linear alpha)
        qoi_data
    }

    #[test]
    fn test_cache_manager_creation() {
        let temp_dir = TempDir::new().unwrap();
        let cache_path = temp_dir.path().join(".cache");
        
        let manager = CacheManager::new(&cache_path, "test.bin".to_string()).unwrap();
        
        assert!(cache_path.exists());
        assert_eq!(manager.dump_filename, "test.bin");
    }

    #[test]
    fn test_get_tile_path() {
        let temp_dir = TempDir::new().unwrap();
        let cache_path = temp_dir.path().join(".cache");
        
        let manager = CacheManager::new(&cache_path, "test.bin".to_string()).unwrap();
        let coord = TileCoord::new(0, 5, 10);
        
        let path = manager.get_tile_path(&coord);
        
        // Verify path structure: .cache/test.bin/0/10/5.qoi
        assert!(path.to_string_lossy().contains("test.bin"));
        assert!(path.to_string_lossy().contains("0"));
        assert!(path.to_string_lossy().contains("10"));
        assert!(path.to_string_lossy().contains("5.qoi"));
    }

    #[test]
    fn test_tile_exists_false() {
        let temp_dir = TempDir::new().unwrap();
        let cache_path = temp_dir.path().join(".cache");
        
        let manager = CacheManager::new(&cache_path, "test.bin".to_string()).unwrap();
        let coord = TileCoord::new(0, 5, 10);
        
        assert!(!manager.tile_exists(&coord));
    }

    #[test]
    fn test_save_and_load_tile() {
        let temp_dir = TempDir::new().unwrap();
        let cache_path = temp_dir.path().join(".cache");
        
        let manager = CacheManager::new(&cache_path, "test.bin".to_string()).unwrap();
        let coord = TileCoord::new(0, 5, 10);
        
        // Create a valid QOI signature (qoif magic bytes)
        let mut qoi_data = b"qoif".to_vec();
        qoi_data.extend_from_slice(&[0x00, 0x00, 0x00, 0x10]); // width (16)
        qoi_data.extend_from_slice(&[0x00, 0x00, 0x00, 0x10]); // height (16)
        qoi_data.push(4); // channels (RGBA)
        qoi_data.push(1); // colorspace (sRGB with linear alpha)
        
        // Save the tile
        manager.save_tile(&coord, &qoi_data).unwrap();
        
        // Verify it exists
        assert!(manager.tile_exists(&coord));
        
        // Load and verify
        let loaded = manager.load_tile(&coord).unwrap();
        assert_eq!(loaded, qoi_data);
    }

    #[test]
    fn test_load_invalid_qoi() {
        let temp_dir = TempDir::new().unwrap();
        let cache_path = temp_dir.path().join(".cache");
        
        let manager = CacheManager::new(&cache_path, "test.bin".to_string()).unwrap();
        let coord = TileCoord::new(0, 5, 10);
        
        // Save invalid QOI data
        let invalid_data = vec![0xFF, 0xFF, 0xFF];
        manager.save_tile(&coord, &invalid_data).unwrap();
        
        // Try to load - should fail validation
        let result = manager.load_tile(&coord);
        assert!(result.is_err());
    }

    #[test]
    fn test_save_creates_directories() {
        let temp_dir = TempDir::new().unwrap();
        let cache_path = temp_dir.path().join(".cache");
        
        let manager = CacheManager::new(&cache_path, "test.bin".to_string()).unwrap();
        let coord = TileCoord::new(5, 100, 200);
        
        // Create valid QOI data
        let mut qoi_data = create_test_qoi_data();
        qoi_data.extend_from_slice(&[0x00, 0x00, 0x00, 0x0D]);
        
        // Save should create all intermediate directories
        manager.save_tile(&coord, &qoi_data).unwrap();
        
        let path = manager.get_tile_path(&coord);
        assert!(path.exists());
        
        // Verify directory structure
        assert!(path.parent().unwrap().exists());
    }

    #[test]
    fn test_invalidate_cache() {
        let temp_dir = TempDir::new().unwrap();
        let cache_path = temp_dir.path().join(".cache");
        
        let manager = CacheManager::new(&cache_path, "test.bin".to_string()).unwrap();
        let coord = TileCoord::new(0, 5, 10);
        
        // Create valid QOI data
        let mut qoi_data = create_test_qoi_data();
        qoi_data.extend_from_slice(&[0x00, 0x00, 0x00, 0x0D]);
        
        // Save a tile
        manager.save_tile(&coord, &qoi_data).unwrap();
        assert!(manager.tile_exists(&coord));
        
        // Invalidate cache
        manager.invalidate_cache().unwrap();
        
        // Verify tile no longer exists
        assert!(!manager.tile_exists(&coord));
    }

    #[test]
    fn test_multiple_tiles_different_levels() {
        let temp_dir = TempDir::new().unwrap();
        let cache_path = temp_dir.path().join(".cache");
        
        let manager = CacheManager::new(&cache_path, "test.bin".to_string()).unwrap();
        
        // Create valid QOI data
        let mut qoi_data = create_test_qoi_data();
        qoi_data.extend_from_slice(&[0x00, 0x00, 0x00, 0x0D]);
        
        // Save tiles at different levels
        let coord1 = TileCoord::new(0, 5, 10);
        let coord2 = TileCoord::new(1, 2, 5);
        let coord3 = TileCoord::new(2, 1, 2);
        
        manager.save_tile(&coord1, &qoi_data).unwrap();
        manager.save_tile(&coord2, &qoi_data).unwrap();
        manager.save_tile(&coord3, &qoi_data).unwrap();
        
        // Verify all exist
        assert!(manager.tile_exists(&coord1));
        assert!(manager.tile_exists(&coord2));
        assert!(manager.tile_exists(&coord3));
        
        // Verify paths are different
        let path1 = manager.get_tile_path(&coord1);
        let path2 = manager.get_tile_path(&coord2);
        let path3 = manager.get_tile_path(&coord3);
        
        assert_ne!(path1, path2);
        assert_ne!(path2, path3);
        assert_ne!(path1, path3);
    }

    #[test]
    fn test_cache_miss_load_fails() {
        let temp_dir = TempDir::new().unwrap();
        let cache_path = temp_dir.path().join(".cache");
        
        let manager = CacheManager::new(&cache_path, "test.bin".to_string()).unwrap();
        let coord = TileCoord::new(0, 5, 10);
        
        // Tile doesn't exist - should be a cache miss
        assert!(!manager.tile_exists(&coord));
        
        // Trying to load non-existent tile should fail
        let result = manager.load_tile(&coord);
        assert!(result.is_err());
    }

    #[test]
    fn test_cache_hit_after_save() {
        let temp_dir = TempDir::new().unwrap();
        let cache_path = temp_dir.path().join(".cache");
        
        let manager = CacheManager::new(&cache_path, "test.bin".to_string()).unwrap();
        let coord = TileCoord::new(0, 5, 10);
        
        // Create valid QOI data
        let mut qoi_data = create_test_qoi_data();
        qoi_data.extend_from_slice(&[0x00, 0x00, 0x00, 0x0D]);
        
        // Initial state: cache miss
        assert!(!manager.tile_exists(&coord));
        
        // Save the tile
        manager.save_tile(&coord, &qoi_data).unwrap();
        
        // Now it's a cache hit
        assert!(manager.tile_exists(&coord));
        
        // Load should succeed
        let loaded = manager.load_tile(&coord).unwrap();
        assert_eq!(loaded, qoi_data);
    }

    #[test]
    fn test_multiple_cache_operations() {
        let temp_dir = TempDir::new().unwrap();
        let cache_path = temp_dir.path().join(".cache");
        
        let manager = CacheManager::new(&cache_path, "test.bin".to_string()).unwrap();
        
        // Create valid QOI data
        let mut qoi_data1 = vec![0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
        qoi_data1.extend_from_slice(&[0x01, 0x02, 0x03, 0x04]);
        
        let mut qoi_data2 = vec![0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
        qoi_data2.extend_from_slice(&[0x05, 0x06, 0x07, 0x08]);
        
        let coord1 = TileCoord::new(0, 1, 2);
        let coord2 = TileCoord::new(0, 3, 4);
        
        // Save first tile
        manager.save_tile(&coord1, &qoi_data1).unwrap();
        assert!(manager.tile_exists(&coord1));
        assert!(!manager.tile_exists(&coord2));
        
        // Save second tile
        manager.save_tile(&coord2, &qoi_data2).unwrap();
        assert!(manager.tile_exists(&coord1));
        assert!(manager.tile_exists(&coord2));
        
        // Load both and verify
        let loaded1 = manager.load_tile(&coord1).unwrap();
        let loaded2 = manager.load_tile(&coord2).unwrap();
        
        assert_eq!(loaded1, qoi_data1);
        assert_eq!(loaded2, qoi_data2);
    }

    #[test]
    fn test_invalidate_empty_cache() {
        let temp_dir = TempDir::new().unwrap();
        let cache_path = temp_dir.path().join(".cache");
        
        let manager = CacheManager::new(&cache_path, "test.bin".to_string()).unwrap();
        
        // Invalidating empty cache should succeed without error
        let result = manager.invalidate_cache();
        assert!(result.is_ok());
    }

    #[test]
    fn test_cache_cleanup_removes_all_tiles() {
        let temp_dir = TempDir::new().unwrap();
        let cache_path = temp_dir.path().join(".cache");
        
        let manager = CacheManager::new(&cache_path, "test.bin".to_string()).unwrap();
        
        // Create valid QOI data
        let mut qoi_data = create_test_qoi_data();
        qoi_data.extend_from_slice(&[0x00, 0x00, 0x00, 0x0D]);
        
        // Save multiple tiles at different levels
        let tiles = vec![
            TileCoord::new(0, 1, 2),
            TileCoord::new(0, 3, 4),
            TileCoord::new(1, 5, 6),
            TileCoord::new(2, 7, 8),
        ];
        
        for coord in &tiles {
            manager.save_tile(coord, &qoi_data).unwrap();
            assert!(manager.tile_exists(coord));
        }
        
        // Cleanup cache
        manager.invalidate_cache().unwrap();
        
        // All tiles should be gone
        for coord in &tiles {
            assert!(!manager.tile_exists(coord));
        }
    }

    #[test]
    fn test_directory_creation_nested() {
        let temp_dir = TempDir::new().unwrap();
        let cache_path = temp_dir.path().join(".cache");
        
        let manager = CacheManager::new(&cache_path, "test.bin".to_string()).unwrap();
        
        // Use deep coordinates to test nested directory creation
        let coord = TileCoord::new(10, 999, 888);
        
        // Create valid QOI data
        let mut qoi_data = create_test_qoi_data();
        qoi_data.extend_from_slice(&[0x00, 0x00, 0x00, 0x0D]);
        
        // Save should create all nested directories
        manager.save_tile(&coord, &qoi_data).unwrap();
        
        let path = manager.get_tile_path(&coord);
        assert!(path.exists());
        
        // Verify the full directory structure exists
        let dump_dir = cache_path.join("test.bin");
        let level_dir = dump_dir.join("10");
        let y_dir = level_dir.join("888");
        
        assert!(dump_dir.exists());
        assert!(level_dir.exists());
        assert!(y_dir.exists());
    }

    #[test]
    fn test_load_corrupted_qoi_too_small() {
        let temp_dir = TempDir::new().unwrap();
        let cache_path = temp_dir.path().join(".cache");
        
        let manager = CacheManager::new(&cache_path, "test.bin".to_string()).unwrap();
        let coord = TileCoord::new(0, 5, 10);
        
        // Save QOI data that's too small (less than 8 bytes)
        let invalid_data = vec![0x89, 0x50, 0x4E];
        manager.save_tile(&coord, &invalid_data).unwrap();
        
        // Try to load - should fail validation
        let result = manager.load_tile(&coord);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("too small"));
    }

    // Property-Based Tests

    /// Property 18: Cache lookup
    /// 
    /// For any tile request, the cache manager SHALL check if the tile exists in the cache
    /// before generating, and if it exists, SHALL load it instead of regenerating.
    ///
    /// **Validates: Requirements 8.3, 8.4**
    #[test]
    #[ignore]
    fn prop_cache_consistency() {
        proptest!(|(
            level in 0u32..10,
            x in 0u32..100,
            y in 0u32..100,
            data_size in 8usize..1024,
        )| {
            let temp_dir = TempDir::new().unwrap();
            let cache_path = temp_dir.path().join(".cache");
            
            let manager = CacheManager::new(&cache_path, "test.bin".to_string()).unwrap();
            let coord = TileCoord::new(level, x, y);
            
            // Before saving, tile should not exist
            prop_assert!(!manager.tile_exists(&coord));
            
            // Create valid QOI data with random content
            let mut qoi_data = create_test_qoi_data();
            // Add random data after QOI signature
            for i in 0..data_size {
                qoi_data.push((i % 256) as u8);
            }
            
            // Save the tile
            manager.save_tile(&coord, &qoi_data).unwrap();
            
            // After saving, tile should exist (cache lookup)
            prop_assert!(manager.tile_exists(&coord));
            
            // Load the tile from cache
            let loaded = manager.load_tile(&coord).unwrap();
            
            // Loaded tile should match saved tile exactly
            prop_assert_eq!(loaded, qoi_data);
            
            // Verify the tile path is correct
            let path = manager.get_tile_path(&coord);
            prop_assert!(path.exists());
            prop_assert!(path.to_string_lossy().contains(&level.to_string()));
            prop_assert!(path.to_string_lossy().contains(&y.to_string()));
            let expected_filename = format!("{}.qoi", x);
            prop_assert!(path.to_string_lossy().contains(&expected_filename));
        });
    }
}
