//! Tile iterator for generating low-priority background tiles
//!
//! Iterates through all tiles in the pyramid, starting from the bottom (level 0)
//! and working upward. This provides a controlled way to generate background tiles
//! without flooding the queue.

use crate::types::{FileMetadata, TileCoord};

/// Iterator for generating tiles in a controlled order
///
/// Generates tiles bottom-up through the pyramid, starting at level 0.
/// Only stores the current position, avoiding queue explosion.
pub struct TileIterator {
    _metadata: FileMetadata,
    current_level: u32,
    current_x: u32,
    current_y: u32,
    max_level: u32,
    max_tiles_x: Vec<u32>,
    max_tiles_y: Vec<u32>,
}

impl TileIterator {
    /// Create a new tile iterator
    /// 
    /// Starts at Level 1 (not Level 0) because:
    /// - Level 0 tiles are not cached on disk (recalculated from dump on demand)
    /// - Level 1+ tiles are cached and need background generation
    pub fn new(metadata: FileMetadata) -> Self {
        // Calculate max tiles at each level
        let pixels_wide_l0 = (metadata.page_length as u64 * 8) * metadata.grid_width as u64;
        let pixels_tall_l0 = metadata.block_size as u64 * metadata.grid_height as u64;
        
        const TILE_SIZE: u64 = 256;
        
        // Calculate how many levels we need
        let mut max_level = 0u32;
        let mut pixels_wide = pixels_wide_l0;
        let mut pixels_tall = pixels_tall_l0;
        
        while pixels_wide > TILE_SIZE || pixels_tall > TILE_SIZE {
            max_level += 1;
            pixels_wide /= 2;
            pixels_tall /= 2;
        }
        
        // Pre-calculate max tiles for each level
        let mut max_tiles_x = Vec::new();
        let mut max_tiles_y = Vec::new();
        
        for level in 0..=max_level {
            let scale = 2u64.pow(level);
            let w = pixels_wide_l0 / scale;
            let h = pixels_tall_l0 / scale;
            let tiles_x = ((w + TILE_SIZE - 1) / TILE_SIZE) as u32;
            let tiles_y = ((h + TILE_SIZE - 1) / TILE_SIZE) as u32;
            max_tiles_x.push(tiles_x.max(1));
            max_tiles_y.push(tiles_y.max(1));
        }
        
        TileIterator {
            _metadata: metadata,
            current_level: 1,  // Start at Level 1 (Level 0 not cached)
            current_x: 0,
            current_y: 0,
            max_level,
            max_tiles_x,
            max_tiles_y,
        }
    }
    
    /// Get the next tile coordinate
    ///
    /// Returns None when all tiles have been iterated
    pub fn next(&mut self) -> Option<TileCoord> {
        if self.current_level > self.max_level {
            return None;
        }
        
        let max_x = self.max_tiles_x[self.current_level as usize];
        let max_y = self.max_tiles_y[self.current_level as usize];
        
        // Return current tile
        let coord = TileCoord::new(self.current_level, self.current_x, self.current_y);
        
        // Advance to next position
        self.current_x += 1;
        if self.current_x >= max_x {
            self.current_x = 0;
            self.current_y += 1;
            if self.current_y >= max_y {
                self.current_y = 0;
                self.current_level += 1;
            }
        }
        
        Some(coord)
    }
    
    /// Reset iterator to the beginning (Level 1)
    pub fn reset(&mut self) {
        self.current_level = 1;  // Start at Level 1 (Level 0 not cached)
        self.current_x = 0;
        self.current_y = 0;
    }
    
    /// Get current position for debugging
    pub fn current_position(&self) -> (u32, u32, u32) {
        (self.current_level, self.current_x, self.current_y)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    fn create_test_metadata() -> FileMetadata {
        FileMetadata::new(
            "test.bin".to_string(),
            10_000_000, // 10 MB
            512,        // 512 bytes per page
            64,         // 64 pages per block
        )
    }
    
    #[test]
    fn test_iterator_creation() {
        let metadata = create_test_metadata();
        let iterator = TileIterator::new(metadata);
        
        assert_eq!(iterator.current_level, 1);  // Starts at Level 1
        assert_eq!(iterator.current_x, 0);
        assert_eq!(iterator.current_y, 0);
    }
    
    #[test]
    fn test_iterator_next() {
        let metadata = create_test_metadata();
        let mut iterator = TileIterator::new(metadata);
        
        // Get first tile (should be Level 1, x=0, y=0)
        let tile1 = iterator.next();
        assert!(tile1.is_some());
        let coord1 = tile1.unwrap();
        assert_eq!(coord1.level, 1);  // Level 1, not 0
        assert_eq!(coord1.x, 0);
        assert_eq!(coord1.y, 0);
        
        // Get second tile
        let tile2 = iterator.next();
        assert!(tile2.is_some());
        let coord2 = tile2.unwrap();
        assert_eq!(coord2.level, 1);
        assert_eq!(coord2.x, 1);
        assert_eq!(coord2.y, 0);
    }
    
    #[test]
    fn test_iterator_reset() {
        let metadata = create_test_metadata();
        let mut iterator = TileIterator::new(metadata);
        
        // Advance iterator
        let _ = iterator.next();
        let _ = iterator.next();
        
        // Reset
        iterator.reset();
        
        assert_eq!(iterator.current_level, 1);  // Back to Level 1
        assert_eq!(iterator.current_x, 0);
        assert_eq!(iterator.current_y, 0);
    }
}
