//! Core data structures for the NAND Flash Viewer

use crate::block_arranger::BlockArranger;
use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};

/// File metadata for a NAND dump
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileMetadata {
    /// Path to the dump file
    pub path: String,
    /// Total file size in bytes
    pub size: u64,
    /// Bytes per page
    pub page_length: u32,
    /// Pages per block
    pub block_size: u32,
    /// Total number of pages in the dump
    pub total_pages: u64,
    /// Total number of blocks in the dump
    pub total_blocks: u64,
    /// Number of blocks per row in 4:3 grid layout
    pub grid_width: u32,
    /// Number of rows of blocks in 4:3 grid layout
    pub grid_height: u32,
}

impl FileMetadata {
    /// Create new file metadata with calculated derived values
    pub fn new(path: String, size: u64, page_length: u32, block_size: u32) -> Self {
        let total_pages = size / (page_length as u64);
        let total_blocks = (total_pages + (block_size as u64) - 1) / (block_size as u64);
        
        // Calculate grid dimensions for 4:3 aspect ratio
        let (grid_width, grid_height) = BlockArranger::calculate_grid_dimensions(
            total_blocks,
            block_size,
            page_length,
        );
        
        FileMetadata {
            path,
            size,
            page_length,
            block_size,
            total_pages,
            total_blocks,
            grid_width,
            grid_height,
        }
    }
}

/// Tile coordinates in the pyramid
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TileCoord {
    /// Resolution level (0 = highest resolution)
    pub level: u32,
    /// Tile column
    pub x: u32,
    /// Tile row
    pub y: u32,
}

impl TileCoord {
    /// Create a new tile coordinate
    pub fn new(level: u32, x: u32, y: u32) -> Self {
        TileCoord { level, x, y }
    }
}

/// Pyramid level information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PyramidLevel {
    /// Resolution level
    pub level: u32,
    /// Tile width in pixels
    pub tile_width: u32,
    /// Tile height in pixels
    pub tile_height: u32,
    /// Number of tiles horizontally
    pub tiles_wide: u32,
    /// Number of tiles vertically
    pub tiles_tall: u32,
    /// Total number of tiles at this level
    pub total_tiles: u64,
}

impl PyramidLevel {
    /// Create a new pyramid level
    pub fn new(level: u32, tile_width: u32, tile_height: u32, tiles_wide: u32, tiles_tall: u32) -> Self {
        let total_tiles = (tiles_wide as u64) * (tiles_tall as u64);
        PyramidLevel {
            level,
            tile_width,
            tile_height,
            tiles_wide,
            tiles_tall,
            total_tiles,
        }
    }
}

/// Priority level for tile generation tasks
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Priority {
    /// Low priority - far from viewport
    Low = 0,
    /// Normal priority - adjacent to viewport
    Normal = 1,
    /// High priority - in viewport
    High = 2,
}

/// Tile generation task
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TileTask {
    /// Tile coordinates
    pub coord: TileCoord,
    /// Task priority
    pub priority: Priority,
    /// Number of retry attempts
    pub retry_count: u32,
    /// Timestamp when task was created (seconds since UNIX_EPOCH)
    pub created_at: u64,
    /// Whether this is a high-resolution tile (level 0)
    pub is_high_resolution: bool,
}

impl TileTask {
    /// Create a new tile task
    pub fn new(coord: TileCoord, priority: Priority, is_high_resolution: bool) -> Self {
        let created_at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        
        TileTask {
            coord,
            priority,
            retry_count: 0,
            created_at,
            is_high_resolution,
        }
    }
}

/// Viewport state
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Viewport {
    /// Current zoom level
    pub level: u32,
    /// Center X coordinate in level coordinate space (pixels)
    pub center_x: f64,
    /// Center Y coordinate in level coordinate space (pixels)
    pub center_y: f64,
    /// Screen width in pixels
    pub width_pixels: u32,
    /// Screen height in pixels
    pub height_pixels: u32,
    /// Tiles currently visible in viewport
    pub visible_tiles: Vec<TileCoord>,
    /// Tiles adjacent to viewport (predictive loading)
    pub adjacent_tiles: Vec<TileCoord>,
}

impl Viewport {
    /// Create a new viewport
    pub fn new(level: u32, center_x: f64, center_y: f64, width_pixels: u32, height_pixels: u32) -> Self {
        Viewport {
            level,
            center_x,
            center_y,
            width_pixels,
            height_pixels,
            visible_tiles: Vec::new(),
            adjacent_tiles: Vec::new(),
        }
    }
}

/// Byte range fragment from dump file
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Fragment {
    /// Start byte offset (inclusive)
    pub start_byte: u64,
    /// End byte offset (exclusive)
    pub end_byte: u64,
}

impl Fragment {
    /// Create a new fragment
    pub fn new(start_byte: u64, end_byte: u64) -> Self {
        Fragment { start_byte, end_byte }
    }
    
    /// Get the length of this fragment
    pub fn length(&self) -> u64 {
        self.end_byte - self.start_byte
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_file_metadata_creation() {
        let metadata = FileMetadata::new(
            "test.bin".to_string(),
            1_000_000,
            512,
            64,
        );
        
        assert_eq!(metadata.path, "test.bin");
        assert_eq!(metadata.size, 1_000_000);
        assert_eq!(metadata.page_length, 512);
        assert_eq!(metadata.block_size, 64);
        assert!(metadata.total_pages > 0);
        assert!(metadata.total_blocks > 0);
        assert!(metadata.grid_width > 0);
        assert!(metadata.grid_height > 0);
    }

    #[test]
    fn test_tile_coord_creation() {
        let coord = TileCoord::new(0, 5, 10);
        assert_eq!(coord.level, 0);
        assert_eq!(coord.x, 5);
        assert_eq!(coord.y, 10);
    }

    #[test]
    fn test_priority_ordering() {
        assert!(Priority::High > Priority::Normal);
        assert!(Priority::Normal > Priority::Low);
    }

    #[test]
    fn test_fragment_length() {
        let frag = Fragment::new(100, 200);
        assert_eq!(frag.length(), 100);
    }

    #[test]
    fn test_viewport_creation() {
        let vp = Viewport::new(0, 512.0, 512.0, 1024, 768);
        assert_eq!(vp.level, 0);
        assert_eq!(vp.center_x, 512.0);
        assert_eq!(vp.center_y, 512.0);
        assert_eq!(vp.width_pixels, 1024);
        assert_eq!(vp.height_pixels, 768);
    }
}
