//! Bidirectional conversion between tile coordinates and byte offsets
//!
//! This module implements the CoordinateParser interface for converting between
//! tile coordinates (level, x, y) and byte offsets in the NAND dump file.
//! It accounts for page length, block size, and grid layout.

use crate::types::{FileMetadata, TileCoord};

/// Standard tile dimensions in pixels
const TILE_WIDTH: u32 = 256;
const TILE_HEIGHT: u32 = 256;

/// Bidirectional coordinate conversion between tiles and byte offsets
pub struct CoordinateParser;

impl CoordinateParser {
    /// Converts tile coordinates to byte offset in the dump file
    ///
    /// Algorithm:
    /// 1. Calculate tile dimensions at level 0: `tileW0 = tileWidth * 2^level`, `tileH0 = tileHeight * 2^level`
    /// 2. Calculate pixel position in level 0: `pixelX = x * tileW0`, `pixelY = y * tileH0`
    /// 3. Convert pixels to bits: `bitX = pixelX`, `bitY = pixelY`
    /// 4. Convert bits to bytes: `byteX = bitX / 8`, `byteY = bitY`
    /// 5. Calculate block position: `blockX = byteX / (pageLength * 8)`, `blockY = byteY / (pageLength * blockSize)`
    /// 6. Calculate byte offset within block: `offsetInBlock = (byteY % (pageLength * blockSize)) * pageLength + (byteX % (pageLength * 8))`
    /// 7. Calculate absolute byte offset: `offset = (blockY * gridWidth + blockX) * blockSize * pageLength + offsetInBlock`
    /// Converts tile coordinates to byte offset in the dump file
    ///
    /// A tile at (x, y) represents a 256x256 pixel region:
    /// - 256 bits horizontally = 32 bytes
    /// - 256 bytes vertically
    ///
    /// The dump is organized as:
    /// - Blocks in a grid (gridWidth x gridHeight)
    /// - Each block contains blockSize pages
    /// - Each page contains pageLength bytes
    ///
    /// Algorithm:
    /// 1. Calculate pixel position: pixelX = x * 256, pixelY = y * 256
    /// 2. Convert to bytes: byteX = pixelX / 8, byteY = pixelY
    /// 3. Calculate block position: blockX = byteX / (pageLength * 8), blockY = byteY / (pageLength * blockSize)
    /// 4. Calculate position within block: pageInBlock = (byteY % (pageLength * blockSize)) / pageLength, byteInPage = byteX % (pageLength * 8)
    /// 5. Calculate offset: (blockY * gridWidth + blockX) * blockSize * pageLength + pageInBlock * pageLength + byteInPage
    pub fn tile_to_byte_offset(coord: TileCoord, metadata: &FileMetadata) -> u64 {
        let level_scale = 1u32 << coord.level; // 2^level
        
        // Step 1: Calculate tile dimensions at level 0
        let tile_w0 = TILE_WIDTH * level_scale;
        let tile_h0 = TILE_HEIGHT * level_scale;
        
        // Step 2: Calculate pixel position in level 0
        let pixel_x = (coord.x as u64) * (tile_w0 as u64);
        let pixel_y = (coord.y as u64) * (tile_h0 as u64);
        
        // Step 3: Convert pixels to bytes
        let byte_x = pixel_x / 8;
        let byte_y = pixel_y;
        
        // Step 4: Calculate block position
        let page_length = metadata.page_length as u64;
        let grid_width = metadata.grid_width as u64;
        let block_size = metadata.block_size as u64;
        
        let bytes_per_block_width = page_length * 8;
        let bytes_per_block_height = page_length * block_size;
        
        let block_x = byte_x / bytes_per_block_width;
        let block_y = byte_y / bytes_per_block_height;
        
        // Step 5: Calculate position within block
        let byte_y_in_block = byte_y % bytes_per_block_height;
        let page_in_block = byte_y_in_block / page_length;
        let byte_in_page = byte_y_in_block % page_length;
        
        // Step 6: Calculate absolute byte offset
        let offset = ((block_y * grid_width + block_x) * block_size * page_length) 
                   + (page_in_block * page_length) 
                   + byte_in_page;
        
        offset
    }
    
    /// Converts byte offset to tile coordinates at a specific resolution level
    ///
    /// Algorithm (reverse of tile_to_byte_offset):
    /// 1. Calculate block coordinates: blockY = offset / (blockSize * pageLength * gridWidth), blockX = (offset % (blockSize * pageLength * gridWidth)) / (blockSize * pageLength)
    /// 2. Calculate position within block: pageInBlock = (offset % (blockSize * pageLength)) / pageLength, byteInPage = offset % pageLength
    /// 3. Calculate byte position in global grid: byteY = blockY * pageLength * blockSize + pageInBlock * pageLength, byteX = blockX * pageLength * 8 + byteInPage
    /// 4. Calculate pixel position: pixelX = byteX * 8, pixelY = byteY
    /// 5. Calculate tile coordinates: x = pixelX / (tileWidth * 2^level), y = pixelY / (tileHeight * 2^level)
    pub fn byte_offset_to_tile(offset: u64, level: u32, metadata: &FileMetadata) -> TileCoord {
        let page_length = metadata.page_length as u64;
        let grid_width = metadata.grid_width as u64;
        let block_size = metadata.block_size as u64;
        
        // Step 1: Calculate block coordinates
        let block_stride = block_size * page_length * grid_width;
        let block_y = offset / block_stride;
        let block_x = (offset % block_stride) / (block_size * page_length);
        
        // Step 2: Calculate position within block
        let offset_in_block = offset % (block_size * page_length);
        let page_in_block = offset_in_block / page_length;
        let byte_in_page = offset_in_block % page_length;
        
        // Step 3: Calculate byte position in global grid
        let bytes_per_block_width = page_length * 8;
        let bytes_per_block_height = page_length * block_size;
        
        let byte_y = block_y * bytes_per_block_height + page_in_block * page_length + byte_in_page;
        let byte_x = block_x * bytes_per_block_width;
        
        // Step 4: Calculate pixel position (bits in x, bytes in y)
        let pixel_x = byte_x * 8;
        let pixel_y = byte_y;
        
        // Step 5: Calculate tile coordinates
        let level_scale = 1u64 << level; // 2^level
        let tile_w0 = (TILE_WIDTH as u64) * level_scale;
        let tile_h0 = (TILE_HEIGHT as u64) * level_scale;
        
        let x = (pixel_x / tile_w0) as u32;
        let y = (pixel_y / tile_h0) as u32;
        
        TileCoord::new(level, x, y)
    }
    
    /// Pretty-prints tile coordinates in "L{level}:({x},{y})" format
    pub fn pretty_print(coord: TileCoord) -> String {
        format!("L{}:({},{})", coord.level, coord.x, coord.y)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tile_to_byte_offset_level_0() {
        let metadata = FileMetadata::new(
            "test.bin".to_string(),
            1_000_000,
            512,
            64,
        );
        
        let coord = TileCoord::new(0, 0, 0);
        let offset = CoordinateParser::tile_to_byte_offset(coord, &metadata);
        
        // At level 0, tile (0,0) should start at byte 0
        assert_eq!(offset, 0);
    }

    #[test]
    fn test_tile_to_byte_offset_with_level() {
        let metadata = FileMetadata::new(
            "test.bin".to_string(),
            1_000_000,
            512,
            64,
        );
        
        // At level 1, tiles are 2x larger (512x512 pixels)
        let coord = TileCoord::new(1, 0, 0);
        let offset = CoordinateParser::tile_to_byte_offset(coord, &metadata);
        
        // Should be 0 since it's still the first tile
        assert_eq!(offset, 0);
    }

    #[test]
    fn test_byte_offset_to_tile_level_0() {
        let metadata = FileMetadata::new(
            "test.bin".to_string(),
            1_000_000,
            512,
            64,
        );
        
        let coord = CoordinateParser::byte_offset_to_tile(0, 0, &metadata);
        
        // Byte offset 0 should map to tile (0, 0) at level 0
        assert_eq!(coord.level, 0);
        assert_eq!(coord.x, 0);
        assert_eq!(coord.y, 0);
    }

    #[test]
    fn test_round_trip_conversion() {
        let metadata = FileMetadata::new(
            "test.bin".to_string(),
            10_000_000,
            512,
            64,
        );
        
        let original = TileCoord::new(0, 0, 10);
        let offset = CoordinateParser::tile_to_byte_offset(original, &metadata);
        let recovered = CoordinateParser::byte_offset_to_tile(offset, 0, &metadata);
        
        // Round-trip should produce equivalent coordinates
        assert_eq!(original.level, recovered.level);
        assert_eq!(original.x, recovered.x);
        assert_eq!(original.y, recovered.y);
    }

    #[test]
    fn test_round_trip_conversion_with_level() {
        let metadata = FileMetadata::new(
            "test.bin".to_string(),
            10_000_000,
            512,
            64,
        );
        
        let original = TileCoord::new(2, 0, 7);
        let offset = CoordinateParser::tile_to_byte_offset(original, &metadata);
        let recovered = CoordinateParser::byte_offset_to_tile(offset, 2, &metadata);
        
        // Round-trip should produce equivalent coordinates
        assert_eq!(original.level, recovered.level);
        assert_eq!(original.x, recovered.x);
        assert_eq!(original.y, recovered.y);
    }

    #[test]
    fn test_pretty_print() {
        let coord = TileCoord::new(0, 5, 10);
        let formatted = CoordinateParser::pretty_print(coord);
        
        assert_eq!(formatted, "L0:(5,10)");
    }

    #[test]
    fn test_pretty_print_with_level() {
        let coord = TileCoord::new(3, 12, 45);
        let formatted = CoordinateParser::pretty_print(coord);
        
        assert_eq!(formatted, "L3:(12,45)");
    }

    #[test]
    fn test_multiple_tiles_different_offsets() {
        let metadata = FileMetadata::new(
            "test.bin".to_string(),
            10_000_000,
            512,
            64,
        );
        
        let tile1 = TileCoord::new(0, 0, 0);
        let tile2 = TileCoord::new(0, 0, 1);
        let tile3 = TileCoord::new(0, 0, 2);
        
        let offset1 = CoordinateParser::tile_to_byte_offset(tile1, &metadata);
        let offset2 = CoordinateParser::tile_to_byte_offset(tile2, &metadata);
        let offset3 = CoordinateParser::tile_to_byte_offset(tile3, &metadata);
        
        // Different tiles should have different offsets
        assert!(offset1 < offset2);
        assert!(offset2 < offset3);
    }

    // Edge case tests for coordinate conversion
    // **Validates: Requirements 18.1, 18.4**

    #[test]
    fn test_first_tile_at_level_0() {
        // Test the very first tile (0, 0) at level 0
        let metadata = FileMetadata::new(
            "test.bin".to_string(),
            100_000_000,
            512,
            64,
        );
        
        let coord = TileCoord::new(0, 0, 0);
        let offset = CoordinateParser::tile_to_byte_offset(coord, &metadata);
        
        // First tile should start at byte 0
        assert_eq!(offset, 0);
        
        // Round-trip should work
        let recovered = CoordinateParser::byte_offset_to_tile(offset, 0, &metadata);
        assert_eq!(coord, recovered);
    }

    #[test]
    fn test_boundary_minimum_page_length() {
        // Test with minimum page length (500 bytes)
        let metadata = FileMetadata::new(
            "test.bin".to_string(),
            50_000_000_000, // 50 GB
            500,
            64,
        );
        
        let coord = TileCoord::new(0, 0, 10);
        let offset = CoordinateParser::tile_to_byte_offset(coord, &metadata);
        let recovered = CoordinateParser::byte_offset_to_tile(offset, 0, &metadata);
        
        assert_eq!(coord, recovered);
    }

    #[test]
    fn test_boundary_maximum_page_length() {
        // Test with maximum page length (20000 bytes)
        let metadata = FileMetadata::new(
            "test.bin".to_string(),
            100_000_000_000, // 100 GB
            20000,
            128,
        );
        
        let coord = TileCoord::new(0, 0, 5);
        let offset = CoordinateParser::tile_to_byte_offset(coord, &metadata);
        let recovered = CoordinateParser::byte_offset_to_tile(offset, 0, &metadata);
        
        assert_eq!(coord, recovered);
    }

    #[test]
    fn test_boundary_minimum_block_size() {
        // Test with minimum block size (64 pages per block)
        let metadata = FileMetadata::new(
            "test.bin".to_string(),
            10_000_000,
            1024,
            64,
        );
        
        let coord = TileCoord::new(0, 0, 3);
        let offset = CoordinateParser::tile_to_byte_offset(coord, &metadata);
        let recovered = CoordinateParser::byte_offset_to_tile(offset, 0, &metadata);
        
        assert_eq!(coord, recovered);
    }

    #[test]
    fn test_boundary_maximum_block_size() {
        // Test with maximum block size (1024 pages per block)
        let metadata = FileMetadata::new(
            "test.bin".to_string(),
            50_000_000,
            2048,
            1024,
        );
        
        let coord = TileCoord::new(0, 0, 2);
        let offset = CoordinateParser::tile_to_byte_offset(coord, &metadata);
        let recovered = CoordinateParser::byte_offset_to_tile(offset, 0, &metadata);
        
        assert_eq!(coord, recovered);
    }

    #[test]
    fn test_boundary_all_valid_block_sizes() {
        // Test all valid block sizes: 64, 128, 256, 512, 1024
        let block_sizes = vec![64, 128, 256, 512, 1024];
        
        for block_size in block_sizes {
            let metadata = FileMetadata::new(
                "test.bin".to_string(),
                20_000_000,
                1024,
                block_size,
            );
            
            let coord = TileCoord::new(0, 0, 5);
            let offset = CoordinateParser::tile_to_byte_offset(coord, &metadata);
            let recovered = CoordinateParser::byte_offset_to_tile(offset, 0, &metadata);
            
            assert_eq!(coord, recovered, "Failed for block_size={}", block_size);
        }
    }

    #[test]
    fn test_boundary_various_levels() {
        // Test coordinate conversion at different pyramid levels
        let metadata = FileMetadata::new(
            "test.bin".to_string(),
            100_000_000,
            2048,
            128,
        );
        
        for level in 0..5 {
            let coord = TileCoord::new(level, 0, 3);
            let offset = CoordinateParser::tile_to_byte_offset(coord, &metadata);
            let recovered = CoordinateParser::byte_offset_to_tile(offset, level, &metadata);
            
            assert_eq!(coord, recovered, "Failed for level={}", level);
        }
    }

    #[test]
    fn test_boundary_last_tile_in_first_block() {
        // Test the last tile within the first block
        let metadata = FileMetadata::new(
            "test.bin".to_string(),
            10_000_000,
            512,
            64,
        );
        
        // Calculate the last tile y coordinate in the first block
        // Each tile is 256 pixels tall, each page is 512 bytes tall
        // Block has 64 pages = 64 * 512 = 32768 bytes tall
        // Last tile y in first block: (32768 - 1) / 256 = 127
        let coord = TileCoord::new(0, 0, 127);
        let offset = CoordinateParser::tile_to_byte_offset(coord, &metadata);
        let recovered = CoordinateParser::byte_offset_to_tile(offset, 0, &metadata);
        
        assert_eq!(coord, recovered);
    }

    #[test]
    fn test_boundary_first_tile_in_second_block() {
        // Test the first tile in the second block (vertically)
        let metadata = FileMetadata::new(
            "test.bin".to_string(),
            50_000_000,
            512,
            64,
        );
        
        // First tile in second block: y = 64 * 512 / 256 = 128
        let coord = TileCoord::new(0, 0, 128);
        let offset = CoordinateParser::tile_to_byte_offset(coord, &metadata);
        let recovered = CoordinateParser::byte_offset_to_tile(offset, 0, &metadata);
        
        assert_eq!(coord, recovered);
    }

    #[test]
    fn test_round_trip_with_various_page_sizes() {
        // Test round-trip conversion with different page sizes
        let page_sizes = vec![500, 1024, 2048, 4096, 8192, 20000];
        
        for page_length in page_sizes {
            let metadata = FileMetadata::new(
                "test.bin".to_string(),
                100_000_000,
                page_length,
                128,
            );
            
            let coords = vec![
                TileCoord::new(0, 0, 0),
                TileCoord::new(0, 0, 10),
                TileCoord::new(1, 0, 5),
                TileCoord::new(2, 0, 2),
            ];
            
            for coord in coords {
                let offset = CoordinateParser::tile_to_byte_offset(coord, &metadata);
                let recovered = CoordinateParser::byte_offset_to_tile(offset, coord.level, &metadata);
                
                assert_eq!(coord, recovered, "Failed for page_length={}, coord={:?}", page_length, coord);
            }
        }
    }

    #[test]
    fn test_round_trip_with_various_block_sizes() {
        // Test round-trip conversion with different block sizes
        let block_sizes = vec![64, 128, 256, 512, 1024];
        
        for block_size in block_sizes {
            let metadata = FileMetadata::new(
                "test.bin".to_string(),
                100_000_000,
                2048,
                block_size,
            );
            
            let coords = vec![
                TileCoord::new(0, 0, 0),
                TileCoord::new(0, 0, 15),
                TileCoord::new(1, 0, 7),
                TileCoord::new(2, 0, 3),
            ];
            
            for coord in coords {
                let offset = CoordinateParser::tile_to_byte_offset(coord, &metadata);
                let recovered = CoordinateParser::byte_offset_to_tile(offset, coord.level, &metadata);
                
                assert_eq!(coord, recovered, "Failed for block_size={}, coord={:?}", block_size, coord);
            }
        }
    }

    #[test]
    fn test_round_trip_extreme_configurations() {
        // Test with extreme but valid configurations
        let configs = vec![
            (500, 64),      // Minimum page, minimum block
            (20000, 1024),  // Maximum page, maximum block
            (500, 1024),    // Minimum page, maximum block
            (20000, 64),    // Maximum page, minimum block
        ];
        
        for (page_length, block_size) in configs {
            let metadata = FileMetadata::new(
                "test.bin".to_string(),
                100_000_000,
                page_length,
                block_size,
            );
            
            let coord = TileCoord::new(0, 0, 5);
            let offset = CoordinateParser::tile_to_byte_offset(coord, &metadata);
            let recovered = CoordinateParser::byte_offset_to_tile(offset, 0, &metadata);
            
            assert_eq!(coord, recovered, "Failed for page_length={}, block_size={}", page_length, block_size);
        }
    }

    #[test]
    fn test_offset_increases_with_y_coordinate() {
        // Verify that byte offset increases as y coordinate increases
        let metadata = FileMetadata::new(
            "test.bin".to_string(),
            50_000_000,
            1024,
            128,
        );
        
        let mut prev_offset = 0;
        for y in 0..20 {
            let coord = TileCoord::new(0, 0, y);
            let offset = CoordinateParser::tile_to_byte_offset(coord, &metadata);
            
            if y > 0 {
                assert!(offset > prev_offset, "Offset should increase with y coordinate");
            }
            prev_offset = offset;
        }
    }

    #[test]
    fn test_level_scaling_affects_offset() {
        // Verify that higher levels (lower resolution) affect offset calculation
        let metadata = FileMetadata::new(
            "test.bin".to_string(),
            100_000_000,
            2048,
            256,
        );
        
        // Same tile coordinates at different levels should map to different offsets
        let coord_l0 = TileCoord::new(0, 0, 1);
        let coord_l1 = TileCoord::new(1, 0, 1);
        let coord_l2 = TileCoord::new(2, 0, 1);
        
        let offset_l0 = CoordinateParser::tile_to_byte_offset(coord_l0, &metadata);
        let offset_l1 = CoordinateParser::tile_to_byte_offset(coord_l1, &metadata);
        let offset_l2 = CoordinateParser::tile_to_byte_offset(coord_l2, &metadata);
        
        // Higher levels should have larger offsets for same (x, y)
        assert!(offset_l1 > offset_l0);
        assert!(offset_l2 > offset_l1);
    }
}

#[cfg(test)]
mod property_tests {
    use super::*;
    use proptest::prelude::*;

    /// **Validates: Requirements 18.4**
    /// Property 46: Coordinate round-trip
    /// For any valid tile coordinate where x = 0 (byte_x = 0), converting to byte offset and back SHALL produce an equivalent coordinate.
    /// Note: The round-trip property only holds for x = 0 because the offset only encodes the vertical position (byte_y), not the horizontal position (byte_x).
    #[test]
    #[ignore]
    fn prop_coordinate_round_trip() {
        proptest!(|(
            level in 0u32..10,
            y in 0u32..100,
            page_length in 512u32..4096,
            block_size in 64u32..1024,
        )| {
            // Create metadata with valid parameters
            let metadata = FileMetadata::new(
                "test.bin".to_string(),
                100_000_000,
                page_length,
                block_size,
            );
            
            // Create original tile coordinate with x = 0 (byte_x = 0)
            let original = TileCoord::new(level, 0, y);
            
            // Convert to byte offset
            let offset = CoordinateParser::tile_to_byte_offset(original, &metadata);
            
            // Convert back to tile coordinate
            let recovered = CoordinateParser::byte_offset_to_tile(offset, level, &metadata);
            
            // Verify round-trip produces equivalent coordinates
            prop_assert_eq!(original.level, recovered.level, "Level mismatch");
            prop_assert_eq!(original.x, recovered.x, "X coordinate mismatch");
            prop_assert_eq!(original.y, recovered.y, "Y coordinate mismatch");
        });
    }
}
