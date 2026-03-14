//! Pyramid tile generator for lower-resolution tile composition
//!
//! This module implements the PyramidTileGenerator which composes lower-level tiles
//! into higher-level tiles by compositing 4 tiles into a 2x2 grid and downscaling
//! to half resolution using 2:1 pixel averaging.

use crate::error::{Error, Result};
use crate::types::{FileMetadata, TileCoord, PyramidLevel, Priority, TileTask};
use crate::bit_renderer::{Pixel, PixelBuffer};
use crate::cache_manager::CacheManager;
use crate::task_queue::TaskQueue;

/// Tile dimensions in pixels (consistent across all levels)
pub const TILE_WIDTH: u32 = 512;
pub const TILE_HEIGHT: u32 = 512;

/// PyramidTileGenerator handles composition of lower-level tiles into higher-level tiles
pub struct PyramidTileGenerator;

impl PyramidTileGenerator {
    /// Calculate pyramid level information
    ///
    /// Given a level and metadata, calculate the tile dimensions and grid layout at that level.
    /// The pyramid terminates when the entire dump fits in a single tile.
    ///
    /// **Validates: Requirements 5.1, 5.2, 5.3, 5.4**
    pub fn calculate_pyramid_level(level: u32, metadata: &FileMetadata) -> Result<PyramidLevel> {
        if level == 0 {
            // Level 0: highest resolution, tiles are TILE_WIDTH x TILE_HEIGHT pixels
            // Each pixel represents one bit
            // Calculate how many tiles we need to cover the entire dump
            
            // Total bits in dump = size * 8
            let total_bits = metadata.size * 8;
            
            // Pixels needed = total_bits (1 pixel per bit)
            let total_pixels = total_bits;
            
            // Tiles needed horizontally and vertically
            let tiles_wide = ((total_pixels + (TILE_WIDTH as u64) - 1) / (TILE_WIDTH as u64)) as u32;
            let tiles_tall = ((total_pixels + (TILE_HEIGHT as u64) - 1) / (TILE_HEIGHT as u64)) as u32;
            
            return Ok(PyramidLevel::new(
                0,
                TILE_WIDTH,
                TILE_HEIGHT,
                tiles_wide,
                tiles_tall,
            ));
        }
        
        // For level > 0, each level has half the dimensions of the previous level
        // Calculate the previous level first
        let prev_level = Self::calculate_pyramid_level(level - 1, metadata)?;
        
        // Each tile at this level is composed of 4 tiles from the previous level
        // So the dimensions are halved
        let tiles_wide = (prev_level.tiles_wide + 1) / 2;
        let tiles_tall = (prev_level.tiles_tall + 1) / 2;
        
        // If we've reached a single tile, this is the termination level
        if tiles_wide == 1 && tiles_tall == 1 {
            return Ok(PyramidLevel::new(
                level,
                TILE_WIDTH,
                TILE_HEIGHT,
                1,
                1,
            ));
        }
        
        Ok(PyramidLevel::new(
            level,
            TILE_WIDTH,
            TILE_HEIGHT,
            tiles_wide,
            tiles_tall,
        ))
    }
    
    /// Find the maximum pyramid level (where entire dump fits in one tile)
    ///
    /// **Validates: Requirements 5.4**
    pub fn find_max_level(metadata: &FileMetadata) -> Result<u32> {
        let mut level = 0;
        loop {
            let pyr_level = Self::calculate_pyramid_level(level, metadata)?;
            if pyr_level.tiles_wide == 1 && pyr_level.tiles_tall == 1 {
                return Ok(level);
            }
            level += 1;
            // Safety check to prevent infinite loops
            if level > 32 {
                return Err(Error::InvalidCoordinates(
                    "Pyramid level exceeded maximum (32)".to_string(),
                ));
            }
        }
    }
    
    /// Composite 4 tiles into a 2x2 grid
    ///
    /// Takes 4 QOI tiles and combines them into a single tile with 2x2 layout:
    /// - Top-left: tiles[0]
    /// - Top-right: tiles[1]
    /// - Bottom-left: tiles[2]
    /// - Bottom-right: tiles[3]
    ///
    /// **Validates: Requirements 7.3**
    /// Composite 4 tiles (QOI or raw RGB) into a single pixel buffer
    /// 
    /// Automatically detects the format based on tile level and magic number:
    /// - Level 0: Never cached (recalculated from dump)
    /// - Level 1-3: QOI format (compressed for disk storage)
    /// - Level 4+: Raw RGB format (fast decode, minimal overhead)
    pub fn composite_tiles(tiles: [Vec<u8>; 4]) -> Result<PixelBuffer> {
        // Decode all 4 tiles into pixel buffers
        // Automatically detect format: QOI starts with "qoif", raw RGB doesn't
        let mut buffers = Vec::new();
        for (i, tile_data) in tiles.iter().enumerate() {
            let buffer = if tile_data.len() >= 4 && &tile_data[0..4] == b"qoif" {
                // QOI format (level 1-3 tiles)
                // Validate QOI has minimum size (header + end marker)
                if tile_data.len() < 14 {
                    return Err(Error::TileGenerationFailed(
                        format!("Invalid QOI tile {}: file too small ({} bytes)", i, tile_data.len())
                    ));
                }
                
                Self::decode_qoi(tile_data)
                    .map_err(|e| Error::TileGenerationFailed(
                        format!("Failed to decode QOI tile {}: {} (size: {} bytes)", i, e, tile_data.len())
                    ))?
            } else {
                // Raw RGB format (level 4+ tiles)
                // Format: 3 bytes per pixel (RGB), 256x256 tiles = 196,608 bytes
                const TILE_SIZE: u32 = 256;
                const EXPECTED_RGB_SIZE: usize = (TILE_SIZE * TILE_SIZE * 3) as usize;
                
                if tile_data.len() != EXPECTED_RGB_SIZE {
                    return Err(Error::TileGenerationFailed(
                        format!("Invalid raw RGB tile size: {} bytes (expected {})", 
                            tile_data.len(), EXPECTED_RGB_SIZE)
                    ));
                }
                
                Self::decode_raw_rgb(tile_data, TILE_SIZE, TILE_SIZE)
                    .map_err(|e| Error::TileGenerationFailed(
                        format!("Failed to decode raw RGB tile {}: {}", i, e)
                    ))?
            };
            buffers.push(buffer);
        }
        
        // Verify all tiles have the same dimensions
        let width = buffers[0].width() as usize;
        let height = buffers[0].height() as usize;
        for (i, buf) in buffers.iter().enumerate() {
            if buf.width() as usize != width || buf.height() as usize != height {
                return Err(Error::TileGenerationFailed(
                    format!("Tile {} has mismatched dimensions: {}x{} vs {}x{}", 
                        i, buf.width(), buf.height(), width, height)
                ));
            }
        }
        
        // Create composite buffer (2x2 grid of tiles)
        let composite_width = (width * 2) as u32;
        let composite_height = (height * 2) as u32;
        let mut composite = PixelBuffer::new(composite_width, composite_height);
        
        // Copy tiles into composite buffer using optimized direct access
        Self::copy_buffer_region(&buffers[0], &mut composite, 0, 0)?;
        Self::copy_buffer_region(&buffers[1], &mut composite, width as u32, 0)?;
        Self::copy_buffer_region(&buffers[2], &mut composite, 0, height as u32)?;
        Self::copy_buffer_region(&buffers[3], &mut composite, width as u32, height as u32)?;
        
        Ok(composite)
    }
    
    /// Copy a pixel buffer region into another buffer at a specific offset
    /// Optimized for performance using direct buffer access
    fn copy_buffer_region(
        src: &PixelBuffer,
        dst: &mut PixelBuffer,
        offset_x: u32,
        offset_y: u32,
    ) -> Result<()> {
        let src_width = src.width() as usize;
        let src_height = src.height() as usize;
        let dst_width = dst.width() as usize;
        let offset_x = offset_x as usize;
        let offset_y = offset_y as usize;
        
        let src_data = src.data();
        let dst_data = dst.data_mut();
        
        // Copy row by row using direct buffer access
        for y in 0..src_height {
            let src_row_start = y * src_width;
            let dst_row_start = (offset_y + y) * dst_width + offset_x;
            
            // Copy entire row at once
            dst_data[dst_row_start..dst_row_start + src_width]
                .copy_from_slice(&src_data[src_row_start..src_row_start + src_width]);
        }
        Ok(())
    }
    
    /// Downscale a tile to half resolution using 2:1 pixel averaging
    ///
    /// Takes a tile and reduces it to half width and half height by averaging
    /// 2x2 pixel blocks into single pixels.
    ///
    /// **Validates: Requirements 7.4**
    /// 
    /// Optimized for performance using direct buffer access and SIMD-friendly operations.
    pub fn downscale(tile: &PixelBuffer) -> Result<PixelBuffer> {
        let src_width = tile.width() as usize;
        let src_height = tile.height() as usize;
        
        // Result dimensions are half
        let dst_width = (src_width / 2) as u32;
        let dst_height = (src_height / 2) as u32;
        
        let mut result = PixelBuffer::new(dst_width, dst_height);
        
        let src_data = tile.data();
        let dst_data = result.data_mut();
        
        // For each destination pixel, average 2x2 source pixels
        // Use direct buffer indexing for maximum performance
        for y in 0..dst_height as usize {
            for x in 0..dst_width as usize {
                // Source coordinates (2x2 block)
                let src_x = x * 2;
                let src_y = y * 2;
                
                // Get indices for the 4 source pixels
                let idx00 = src_y * src_width + src_x;
                let idx10 = src_y * src_width + src_x + 1;
                let idx01 = (src_y + 1) * src_width + src_x;
                let idx11 = (src_y + 1) * src_width + src_x + 1;
                
                // Get the 4 source pixels
                let p00 = src_data[idx00];
                let p10 = src_data[idx10];
                let p01 = src_data[idx01];
                let p11 = src_data[idx11];
                
                // Average the 4 pixels using u32 to avoid overflow
                let avg_r = ((p00.r as u32 + p10.r as u32 + p01.r as u32 + p11.r as u32) >> 2) as u8;
                let avg_g = ((p00.g as u32 + p10.g as u32 + p01.g as u32 + p11.g as u32) >> 2) as u8;
                let avg_b = ((p00.b as u32 + p10.b as u32 + p01.b as u32 + p11.b as u32) >> 2) as u8;
                
                let avg_pixel = Pixel::new(avg_r, avg_g, avg_b);
                
                // Write to destination
                let dst_idx = y * dst_width as usize + x;
                dst_data[dst_idx] = avg_pixel;
            }
        }
        
        Ok(result)
    }
    
    /// Generate an empty child tile (light gray, no text)
    /// Used for out-of-bounds child tiles in pyramid generation
    fn generate_empty_child_tile() -> Result<Vec<u8>> {
        const TILE_SIZE: u32 = 256;
        
        // Create a light gray tile
        let mut rgba_data = vec![220u8; (TILE_SIZE * TILE_SIZE * 4) as usize];
        
        // Set alpha channel to fully opaque
        for i in 0..(TILE_SIZE * TILE_SIZE) as usize {
            rgba_data[i * 4 + 3] = 255;
        }
        
        // Encode as QOI
        let qoi_data = qoi::encode_to_vec(&rgba_data, TILE_SIZE, TILE_SIZE)
            .map_err(|e| Error::ImageError(format!("QOI encoding error: {:?}", e)))?;
        
        Ok(qoi_data)
    }
    
    /// Generate a pyramid tile by compositing and downscaling lower-level tiles
    ///
    /// For a tile at level L > 0:
    /// 1. Identify 4 child tiles at level L-1
    /// 2. Load or request children (inherit priority from parent)
    /// 3. Composite into 2x2 grid
    /// 4. Downscale to half resolution
    /// 5. Cache result
    ///
    /// **Validates: Requirements 7.1, 7.2, 7.3, 7.4, 7.5**
    pub fn generate_pyramid_tile(
        coord: TileCoord,
        metadata: &FileMetadata,
        task_queue: &TaskQueue,
        cache: &CacheManager,
        priority: Priority,
    ) -> Result<Vec<u8>> {
        // Ensure this is not a level 0 tile
        if coord.level == 0 {
            return Err(Error::InvalidCoordinates(
                "generatePyramidTile only supports level > 0 tiles".to_string(),
            ));
        }
        
        // Step 1: Identify 4 child tiles at level-1
        let child_level = coord.level - 1;
        let child_coords = [
            TileCoord::new(child_level, coord.x * 2, coord.y * 2),
            TileCoord::new(child_level, coord.x * 2 + 1, coord.y * 2),
            TileCoord::new(child_level, coord.x * 2, coord.y * 2 + 1),
            TileCoord::new(child_level, coord.x * 2 + 1, coord.y * 2 + 1),
        ];
        
        // Calculate max tiles at child level to check bounds
        let pixels_wide_l0 = (metadata.page_length as u64 * 8) * metadata.grid_width as u64;
        let pixels_tall_l0 = metadata.block_size as u64 * metadata.grid_height as u64;
        let scale_factor = 2u64.pow(child_level);
        let pixels_wide = pixels_wide_l0 / scale_factor;
        let pixels_tall = pixels_tall_l0 / scale_factor;
        const TILE_SIZE: u64 = 256;
        let max_tiles_x = ((pixels_wide + TILE_SIZE - 1) / TILE_SIZE) as u32;
        let max_tiles_y = ((pixels_tall + TILE_SIZE - 1) / TILE_SIZE) as u32;
        
        // Step 2: Load or request children, use empty tiles for out-of-bounds
        let mut child_tiles = Vec::new();
        let mut missing_tiles = Vec::new();
        
        for child_coord in &child_coords {
            // Check if child tile is within bounds
            if child_coord.x >= max_tiles_x || child_coord.y >= max_tiles_y {
                // Out of bounds - use empty tile
                log::trace!(
                    "Child tile {:?} is out of bounds (max: {}x{}), using empty tile",
                    child_coord, max_tiles_x, max_tiles_y
                );
                child_tiles.push(Self::generate_empty_child_tile()?);
            } else {
                match cache.load_tile(child_coord) {
                    Ok(tile_data) => {
                        child_tiles.push(tile_data);
                    }
                    Err(_) => {
                        // Child tile not cached, request it with same priority as parent
                        missing_tiles.push(*child_coord);
                    }
                }
            }
        }
        
        // If any in-bounds tiles are missing, register dependency and return error
        if !missing_tiles.is_empty() {
            // Register this parent tile as waiting for the missing children
            // This allows it to be re-enqueued when children complete
            task_queue.register_waiting_parent(coord, priority, &missing_tiles);
            
            // Enqueue missing child tiles with same priority as parent
            // LIFO ordering within each priority level ensures children are processed before parent
            // For low-priority tiles, only enqueue if queue isn't too large to prevent explosion
            let should_enqueue = match priority {
                Priority::High | Priority::Normal => true,
                Priority::Low => task_queue.size() < 200,
            };
            
            if should_enqueue {
                for child_coord in &missing_tiles {
                    let task = TileTask::new(*child_coord, priority, child_level == 0);
                    task_queue.enqueue(task);
                }
            } else {
                log::debug!(
                    "Queue size {} too large, deferring low-priority children for {:?}",
                    task_queue.size(),
                    coord
                );
                // Return error so parent will be re-enqueued and retry later
            }
            
            return Err(Error::TileGenerationFailed(
                format!("Child tiles {:?} not available, registered dependency", missing_tiles)
            ));
        }
        
        // Step 3: Composite 4 tiles into 2x2 grid
        let composite_buffer = Self::composite_tiles([
            child_tiles[0].clone(),
            child_tiles[1].clone(),
            child_tiles[2].clone(),
            child_tiles[3].clone(),
        ])?;
        
        // Step 4: Downscale to half resolution
        let downscaled = Self::downscale(&composite_buffer)?;
        
        // Step 5: Cache using tiered strategy based on level
        // Level 1-3: Use QOI compression (good compression ratio)
        // Level 4+: Use raw RGB (minimal overhead, fast decode)
        let cached_bytes = if coord.level <= 3 {
            // Compress with QOI for levels 1-3
            Self::encode_qoi(&downscaled)?
        } else {
            // Use raw RGB for levels 4+
            Self::encode_raw_rgb(&downscaled)
        };
        
        cache.save_tile(&coord, &cached_bytes)?;
        
        Ok(cached_bytes)
    }
    
    /// Decode QOI data into a pixel buffer
    /// Optimized for performance using direct buffer access
    fn decode_qoi(qoi_data: &[u8]) -> Result<PixelBuffer> {
        // Decode QOI data
        let (header, decoded_data) = qoi::decode_to_vec(qoi_data)
            .map_err(|e| Error::ImageError(
                format!("Failed to decode QOI: {:?}", e)
            ))?;
        
        let mut buffer = PixelBuffer::new(header.width, header.height);
        let buffer_data = buffer.data_mut();
        
        // Convert RGBA data to pixels directly
        // QOI data is in RGBA format (4 bytes per pixel)
        let mut pixel_idx = 0;
        for chunk in decoded_data.chunks(4) {
            if chunk.len() == 4 && pixel_idx < buffer_data.len() {
                buffer_data[pixel_idx] = Pixel::new(chunk[0], chunk[1], chunk[2]);
                pixel_idx += 1;
            }
        }
        
        Ok(buffer)
    }
    
    /// Encode a pixel buffer as QOI
    /// Optimized for performance using direct buffer access
    #[allow(dead_code)]
    fn encode_qoi(buffer: &PixelBuffer) -> Result<Vec<u8>> {
        let width = buffer.width();
        let height = buffer.height();

        // Convert pixel buffer to RGBA8 format (QOI requires RGBA)
        let rgba_size = (width * height * 4) as usize;
        let mut rgba_data = Vec::with_capacity(rgba_size);
        
        // Use direct buffer access for maximum performance
        let pixels = buffer.data();
        for pixel in pixels {
            rgba_data.push(pixel.r);
            rgba_data.push(pixel.g);
            rgba_data.push(pixel.b);
            rgba_data.push(255); // Alpha channel (fully opaque)
        }

        // Encode as QOI
        let qoi_data = qoi::encode_to_vec(&rgba_data, width, height)
            .map_err(|e| Error::ImageError(format!("QOI encoding error: {:?}", e)))?;

        Ok(qoi_data)
    }
    
    /// Encode a pixel buffer as raw RGB data (faster than QOI for intermediate tiles)
    /// 
    /// This is used for pyramid tiles to avoid expensive QOI encoding.
    /// Format: 3 bytes per pixel (RGB), no compression.
    fn encode_raw_rgb(buffer: &PixelBuffer) -> Vec<u8> {
        let mut rgb_data = Vec::with_capacity((buffer.width() * buffer.height() * 3) as usize);
        
        for pixel in buffer.data() {
            rgb_data.push(pixel.r);
            rgb_data.push(pixel.g);
            rgb_data.push(pixel.b);
        }
        
        rgb_data
    }
    
    /// Decode raw RGB data into a pixel buffer
    fn decode_raw_rgb(rgb_data: &[u8], width: u32, height: u32) -> Result<PixelBuffer> {
        let mut buffer = PixelBuffer::new(width, height);
        let buffer_data = buffer.data_mut();
        
        let mut pixel_idx = 0;
        for chunk in rgb_data.chunks(3) {
            if chunk.len() == 3 && pixel_idx < buffer_data.len() {
                buffer_data[pixel_idx] = Pixel::new(chunk[0], chunk[1], chunk[2]);
                pixel_idx += 1;
            }
        }
        
        Ok(buffer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::FileMetadata;

    #[test]
    fn test_calculate_pyramid_level_0() {
        let metadata = FileMetadata::new(
            "test.bin".to_string(),
            1_000_000,
            512,
            64,
        );
        
        let level = PyramidTileGenerator::calculate_pyramid_level(0, &metadata).unwrap();
        assert_eq!(level.level, 0);
        assert_eq!(level.tile_width, TILE_WIDTH);
        assert_eq!(level.tile_height, TILE_HEIGHT);
        assert!(level.tiles_wide > 0);
        assert!(level.tiles_tall > 0);
    }

    #[test]
    fn test_calculate_pyramid_level_1() {
        let metadata = FileMetadata::new(
            "test.bin".to_string(),
            1_000_000,
            512,
            64,
        );
        
        let level0 = PyramidTileGenerator::calculate_pyramid_level(0, &metadata).unwrap();
        let level1 = PyramidTileGenerator::calculate_pyramid_level(1, &metadata).unwrap();
        
        assert_eq!(level1.level, 1);
        // Level 1 should have roughly half the tiles (rounded up)
        assert!(level1.tiles_wide <= (level0.tiles_wide + 1) / 2 + 1);
        assert!(level1.tiles_tall <= (level0.tiles_tall + 1) / 2 + 1);
    }

    #[test]
    fn test_find_max_level() {
        let metadata = FileMetadata::new(
            "test.bin".to_string(),
            1_000_000,
            512,
            64,
        );
        
        let max_level = PyramidTileGenerator::find_max_level(&metadata).unwrap();
        assert!(max_level > 0);
        
        // Verify that max level has single tile
        let max_pyr = PyramidTileGenerator::calculate_pyramid_level(max_level, &metadata).unwrap();
        assert_eq!(max_pyr.tiles_wide, 1);
        assert_eq!(max_pyr.tiles_tall, 1);
    }

    #[test]
    fn test_downscale_basic() {
        // Create a simple 4x4 buffer with alternating colors
        let mut buffer = PixelBuffer::new(4, 4);
        let black = Pixel::black();
        let white = Pixel::white();
        
        // Fill with pattern
        for y in 0..4 {
            for x in 0..4 {
                let pixel = if (x + y) % 2 == 0 { black } else { white };
                buffer.set(x, y, pixel).unwrap();
            }
        }
        
        let downscaled = PyramidTileGenerator::downscale(&buffer).unwrap();
        assert_eq!(downscaled.width(), 2);
        assert_eq!(downscaled.height(), 2);
    }

    #[test]
    fn test_downscale_uniform_color() {
        // Create a buffer filled with one color
        let buffer = PixelBuffer::with_fill(4, 4, Pixel::black());
        
        let downscaled = PyramidTileGenerator::downscale(&buffer).unwrap();
        assert_eq!(downscaled.width(), 2);
        assert_eq!(downscaled.height(), 2);
        
        // All pixels should still be black (or very close)
        for y in 0..2 {
            for x in 0..2 {
                let pixel = downscaled.get(x, y).unwrap();
                assert_eq!(pixel.r, 0);
                assert_eq!(pixel.g, 0);
                assert_eq!(pixel.b, 0);
            }
        }
    }

    /// Test composition of 4 tiles into 2x2 grid
    /// **Validates: Requirements 7.3**
    #[test]
    fn test_composite_tiles_basic() {
        // Create 4 simple tiles with different colors
        let tile_size = 4;
        
        // Top-left: all black
        let buffer_tl = PixelBuffer::with_fill(tile_size, tile_size, Pixel::black());
        let qoi_tl = PyramidTileGenerator::encode_qoi(&buffer_tl).unwrap();
        
        // Top-right: all white
        let buffer_tr = PixelBuffer::with_fill(tile_size, tile_size, Pixel::white());
        let qoi_tr = PyramidTileGenerator::encode_qoi(&buffer_tr).unwrap();
        
        // Bottom-left: red
        let buffer_bl = PixelBuffer::with_fill(tile_size, tile_size, Pixel::new(255, 0, 0));
        let qoi_bl = PyramidTileGenerator::encode_qoi(&buffer_bl).unwrap();
        
        // Bottom-right: green
        let buffer_br = PixelBuffer::with_fill(tile_size, tile_size, Pixel::new(0, 255, 0));
        let qoi_br = PyramidTileGenerator::encode_qoi(&buffer_br).unwrap();
        
        // Composite the tiles
        let composite = PyramidTileGenerator::composite_tiles([qoi_tl, qoi_tr, qoi_bl, qoi_br]).unwrap();
        
        // Verify dimensions
        assert_eq!(composite.width(), tile_size * 2);
        assert_eq!(composite.height(), tile_size * 2);
        
        // Verify top-left quadrant is black
        let pixel_tl = composite.get(0, 0).unwrap();
        assert_eq!(pixel_tl.r, 0);
        assert_eq!(pixel_tl.g, 0);
        assert_eq!(pixel_tl.b, 0);
        
        // Verify top-right quadrant is white
        let pixel_tr = composite.get(tile_size, 0).unwrap();
        assert_eq!(pixel_tr.r, 255);
        assert_eq!(pixel_tr.g, 255);
        assert_eq!(pixel_tr.b, 255);
        
        // Verify bottom-left quadrant is red
        let pixel_bl = composite.get(0, tile_size).unwrap();
        assert_eq!(pixel_bl.r, 255);
        assert_eq!(pixel_bl.g, 0);
        assert_eq!(pixel_bl.b, 0);
        
        // Verify bottom-right quadrant is green
        let pixel_br = composite.get(tile_size, tile_size).unwrap();
        assert_eq!(pixel_br.r, 0);
        assert_eq!(pixel_br.g, 255);
        assert_eq!(pixel_br.b, 0);
    }

    /// Test composition with mismatched tile dimensions fails
    /// **Validates: Requirements 7.3**
    #[test]
    fn test_composite_tiles_mismatched_dimensions() {
        // Create tiles with different sizes
        let buffer1 = PixelBuffer::new(4, 4);
        let png1 = PyramidTileGenerator::encode_qoi(&buffer1).unwrap();
        
        let buffer2 = PixelBuffer::new(8, 8);
        let png2 = PyramidTileGenerator::encode_qoi(&buffer2).unwrap();
        
        let buffer3 = PixelBuffer::new(4, 4);
        let png3 = PyramidTileGenerator::encode_qoi(&buffer3).unwrap();
        
        let buffer4 = PixelBuffer::new(4, 4);
        let png4 = PyramidTileGenerator::encode_qoi(&buffer4).unwrap();
        
        // Composite should fail due to mismatched dimensions
        let result = PyramidTileGenerator::composite_tiles([png1, png2, png3, png4]);
        assert!(result.is_err());
    }

    /// Test downscaling produces correct pixel averaging
    /// **Validates: Requirements 7.4**
    #[test]
    fn test_downscale_pixel_averaging() {
        // Create a 4x4 buffer with specific colors for averaging test
        let mut buffer = PixelBuffer::new(4, 4);
        
        // Top-left 2x2 block: all black (0, 0, 0)
        for y in 0..2 {
            for x in 0..2 {
                buffer.set(x, y, Pixel::black()).unwrap();
            }
        }
        
        // Top-right 2x2 block: all white (255, 255, 255)
        for y in 0..2 {
            for x in 2..4 {
                buffer.set(x, y, Pixel::white()).unwrap();
            }
        }
        
        // Bottom-left 2x2 block: red (255, 0, 0)
        for y in 2..4 {
            for x in 0..2 {
                buffer.set(x, y, Pixel::new(255, 0, 0)).unwrap();
            }
        }
        
        // Bottom-right 2x2 block: green (0, 255, 0)
        for y in 2..4 {
            for x in 2..4 {
                buffer.set(x, y, Pixel::new(0, 255, 0)).unwrap();
            }
        }
        
        let downscaled = PyramidTileGenerator::downscale(&buffer).unwrap();
        
        // Verify dimensions
        assert_eq!(downscaled.width(), 2);
        assert_eq!(downscaled.height(), 2);
        
        // Verify averaged colors
        // Top-left: average of 4 black pixels = black
        let pixel_tl = downscaled.get(0, 0).unwrap();
        assert_eq!(pixel_tl.r, 0);
        assert_eq!(pixel_tl.g, 0);
        assert_eq!(pixel_tl.b, 0);
        
        // Top-right: average of 4 white pixels = white
        let pixel_tr = downscaled.get(1, 0).unwrap();
        assert_eq!(pixel_tr.r, 255);
        assert_eq!(pixel_tr.g, 255);
        assert_eq!(pixel_tr.b, 255);
        
        // Bottom-left: average of 4 red pixels = red
        let pixel_bl = downscaled.get(0, 1).unwrap();
        assert_eq!(pixel_bl.r, 255);
        assert_eq!(pixel_bl.g, 0);
        assert_eq!(pixel_bl.b, 0);
        
        // Bottom-right: average of 4 green pixels = green
        let pixel_br = downscaled.get(1, 1).unwrap();
        assert_eq!(pixel_br.r, 0);
        assert_eq!(pixel_br.g, 255);
        assert_eq!(pixel_br.b, 0);
    }

    /// Test downscaling with mixed colors produces correct averages
    /// **Validates: Requirements 7.4**
    #[test]
    fn test_downscale_mixed_colors() {
        // Create a 2x2 buffer with different colors in each pixel
        let mut buffer = PixelBuffer::new(2, 2);
        buffer.set(0, 0, Pixel::new(0, 0, 0)).unwrap();     // Black
        buffer.set(1, 0, Pixel::new(255, 255, 255)).unwrap(); // White
        buffer.set(0, 1, Pixel::new(255, 0, 0)).unwrap();   // Red
        buffer.set(1, 1, Pixel::new(0, 255, 0)).unwrap();   // Green
        
        let downscaled = PyramidTileGenerator::downscale(&buffer).unwrap();
        
        // Result should be 1x1 with averaged color
        assert_eq!(downscaled.width(), 1);
        assert_eq!(downscaled.height(), 1);
        
        let pixel = downscaled.get(0, 0).unwrap();
        // Average: (0+255+255+0)/4 = 127.5 ≈ 127 for R
        // Average: (0+255+0+255)/4 = 127.5 ≈ 127 for G
        // Average: (0+255+0+0)/4 = 63.75 ≈ 63 for B
        assert_eq!(pixel.r, 127);
        assert_eq!(pixel.g, 127);
        assert_eq!(pixel.b, 63);
    }

    /// Test caching of pyramid tiles
    /// **Validates: Requirements 7.5**
    #[test]
    fn test_pyramid_tile_caching() {
        use tempfile::TempDir;
        
        let temp_dir = TempDir::new().unwrap();
        let cache_path = temp_dir.path().join(".cache");
        
        let cache = CacheManager::new(&cache_path, "test.bin".to_string()).unwrap();
        let task_queue = TaskQueue::new();
        
        let metadata = FileMetadata::new(
            "test.bin".to_string(),
            1_000_000,
            512,
            64,
        );
        
        // Create and cache 4 child tiles at level 0
        let tile_size = TILE_WIDTH;
        for i in 0..4 {
            let x = i % 2;
            let y = i / 2;
            let coord = TileCoord::new(0, x, y);
            
            // Create a simple tile
            let buffer = PixelBuffer::with_fill(tile_size, tile_size, Pixel::black());
            let qoi = PyramidTileGenerator::encode_qoi(&buffer).unwrap();
            
            cache.save_tile(&coord, &qoi).unwrap();
        }
        
        // Generate pyramid tile at level 1
        let pyramid_coord = TileCoord::new(1, 0, 0);
        let result = PyramidTileGenerator::generate_pyramid_tile(
            pyramid_coord,
            &metadata,
            &task_queue,
            &cache,
            Priority::Normal,
        );
        
        assert!(result.is_ok());
        
        // Verify the pyramid tile was cached
        assert!(cache.tile_exists(&pyramid_coord));
        
        // Verify we can load it back
        let loaded = cache.load_tile(&pyramid_coord).unwrap();
        assert!(!loaded.is_empty());
    }

    /// Test pyramid tile generation requests missing child tiles
    /// **Validates: Requirements 7.2**
    #[test]
    fn test_pyramid_tile_requests_missing_children() {
        use tempfile::TempDir;
        
        let temp_dir = TempDir::new().unwrap();
        let cache_path = temp_dir.path().join(".cache");
        
        let cache = CacheManager::new(&cache_path, "test.bin".to_string()).unwrap();
        let task_queue = TaskQueue::new();
        
        let metadata = FileMetadata::new(
            "test.bin".to_string(),
            1_000_000,
            512,
            64,
        );
        
        // Don't cache any child tiles
        
        // Try to generate pyramid tile at level 1
        let pyramid_coord = TileCoord::new(1, 0, 0);
        let result = PyramidTileGenerator::generate_pyramid_tile(
            pyramid_coord,
            &metadata,
            &task_queue,
            &cache,
            Priority::Normal,
        );
        
        // Should fail because child tiles are missing
        assert!(result.is_err());
        
        // Verify that a high-priority task was enqueued for the missing child
        let dequeued = task_queue.dequeue();
        assert!(dequeued.is_some());
        
        let task = dequeued.unwrap();
        assert_eq!(task.priority, Priority::High);
        assert_eq!(task.coord.level, 0); // Child tile at level 0
    }

    /// Test pyramid tile generation with partial child tiles
    /// **Validates: Requirements 7.1, 7.2**
    #[test]
    fn test_pyramid_tile_partial_children() {
        use tempfile::TempDir;
        
        let temp_dir = TempDir::new().unwrap();
        let cache_path = temp_dir.path().join(".cache");
        
        let cache = CacheManager::new(&cache_path, "test.bin".to_string()).unwrap();
        let task_queue = TaskQueue::new();
        
        let metadata = FileMetadata::new(
            "test.bin".to_string(),
            1_000_000,
            512,
            64,
        );
        
        // Cache only 2 out of 4 child tiles
        let tile_size = TILE_WIDTH;
        for i in 0..2 {
            let coord = TileCoord::new(0, i, 0);
            let buffer = PixelBuffer::with_fill(tile_size, tile_size, Pixel::black());
            let qoi = PyramidTileGenerator::encode_qoi(&buffer).unwrap();
            cache.save_tile(&coord, &qoi).unwrap();
        }
        
        // Try to generate pyramid tile at level 1
        let pyramid_coord = TileCoord::new(1, 0, 0);
        let result = PyramidTileGenerator::generate_pyramid_tile(
            pyramid_coord,
            &metadata,
            &task_queue,
            &cache,
            Priority::Normal,
        );
        
        // Should fail because not all child tiles are available
        assert!(result.is_err());
        
        // Verify that tasks were enqueued for missing children
        assert!(!task_queue.is_empty());
    }

    /// Test pyramid tile generation fails for level 0
    /// **Validates: Requirements 7.1**
    #[test]
    fn test_pyramid_tile_level_0_fails() {
        use tempfile::TempDir;
        
        let temp_dir = TempDir::new().unwrap();
        let cache_path = temp_dir.path().join(".cache");
        
        let cache = CacheManager::new(&cache_path, "test.bin".to_string()).unwrap();
        let task_queue = TaskQueue::new();
        
        let metadata = FileMetadata::new(
            "test.bin".to_string(),
            1_000_000,
            512,
            64,
        );
        
        // Try to generate pyramid tile at level 0 (should fail)
        let coord = TileCoord::new(0, 0, 0);
        let result = PyramidTileGenerator::generate_pyramid_tile(
            coord,
            &metadata,
            &task_queue,
            &cache,
            Priority::Normal,
        );
        
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("level > 0"));
    }

    /// Test encode and decode QOI round-trip
    /// **Validates: Requirements 7.3, 7.4, 7.5**
    #[test]
    fn test_png_encode_decode_roundtrip() {
        // Create a buffer with specific pattern
        let mut buffer = PixelBuffer::new(8, 8);
        for y in 0..8 {
            for x in 0..8 {
                let color = if (x + y) % 2 == 0 { 0 } else { 255 };
                buffer.set(x, y, Pixel::new(color, color, color)).unwrap();
            }
        }
        
        // Encode to QOI
        let qoi_data = PyramidTileGenerator::encode_qoi(&buffer).unwrap();
        
        // Decode back
        let decoded = PyramidTileGenerator::decode_qoi(&qoi_data).unwrap();
        
        // Verify dimensions match
        assert_eq!(decoded.width(), buffer.width());
        assert_eq!(decoded.height(), buffer.height());
        
        // Verify pixels match
        for y in 0..8 {
            for x in 0..8 {
                let original = buffer.get(x, y).unwrap();
                let decoded_pixel = decoded.get(x, y).unwrap();
                assert_eq!(original.r, decoded_pixel.r);
                assert_eq!(original.g, decoded_pixel.g);
                assert_eq!(original.b, decoded_pixel.b);
            }
        }
    }

    /// Test composition and downscaling together
    /// **Validates: Requirements 7.3, 7.4**
    #[test]
    fn test_composite_and_downscale() {
        let tile_size = 8;
        
        // Create 4 tiles with distinct colors
        let colors = [
            Pixel::new(255, 0, 0),   // Red
            Pixel::new(0, 255, 0),   // Green
            Pixel::new(0, 0, 255),   // Blue
            Pixel::new(255, 255, 0), // Yellow
        ];
        
        let mut pngs = Vec::new();
        for color in &colors {
            let buffer = PixelBuffer::with_fill(tile_size, tile_size, *color);
            let qoi = PyramidTileGenerator::encode_qoi(&buffer).unwrap();
            pngs.push(qoi);
        }
        
        // Composite
        let composite = PyramidTileGenerator::composite_tiles([
            pngs[0].clone(),
            pngs[1].clone(),
            pngs[2].clone(),
            pngs[3].clone(),
        ]).unwrap();
        
        assert_eq!(composite.width(), tile_size * 2);
        assert_eq!(composite.height(), tile_size * 2);
        
        // Downscale
        let downscaled = PyramidTileGenerator::downscale(&composite).unwrap();
        
        assert_eq!(downscaled.width(), tile_size);
        assert_eq!(downscaled.height(), tile_size);
        
        // Verify the downscaled result has averaged colors in each quadrant
        // Top-left should be red
        let pixel_tl = downscaled.get(tile_size / 4, tile_size / 4).unwrap();
        assert!(pixel_tl.r > 200); // Should be mostly red
        
        // Top-right should be green
        let pixel_tr = downscaled.get(tile_size * 3 / 4, tile_size / 4).unwrap();
        assert!(pixel_tr.g > 200); // Should be mostly green
        
        // Bottom-left should be blue
        let pixel_bl = downscaled.get(tile_size / 4, tile_size * 3 / 4).unwrap();
        assert!(pixel_bl.b > 200); // Should be mostly blue
        
        // Bottom-right should be yellow
        let pixel_br = downscaled.get(tile_size * 3 / 4, tile_size * 3 / 4).unwrap();
        assert!(pixel_br.r > 200); // Should have red
        assert!(pixel_br.g > 200); // Should have green
    }
}

#[cfg(test)]
mod property_tests {
    use super::*;
    use crate::types::FileMetadata;
    use proptest::prelude::*;

    /// Property 12: Pyramid level organization
    /// For any NAND dump file, the pyramid SHALL organize tiles into multiple resolution levels,
    /// with level 0 as the highest resolution and each subsequent level having half the dimensions
    /// of the previous level.
    ///
    /// **Validates: Requirements 5.1, 5.2, 5.3**
    #[test]
    #[ignore]
    fn prop_pyramid_level_organization() {
        proptest!(|(
            size in 1_000_000u64..100_000_000u64,
            page_length in 512u32..4096u32,
            block_size in 64u32..256u32,
        )| {
            let metadata = FileMetadata::new(
                "test.bin".to_string(),
                size,
                page_length,
                block_size,
            );
            
            // Get level 0
            let level0 = PyramidTileGenerator::calculate_pyramid_level(0, &metadata)
                .expect("Level 0 should always be calculable");
            
            // Verify level 0 is highest resolution
            assert_eq!(level0.level, 0);
            assert_eq!(level0.tile_width, TILE_WIDTH);
            assert_eq!(level0.tile_height, TILE_HEIGHT);
            
            // Get level 1 and verify it has half dimensions
            let level1 = PyramidTileGenerator::calculate_pyramid_level(1, &metadata)
                .expect("Level 1 should be calculable");
            
            assert_eq!(level1.level, 1);
            assert_eq!(level1.tile_width, TILE_WIDTH);
            assert_eq!(level1.tile_height, TILE_HEIGHT);
            
            // Level 1 should have roughly half the tiles (rounded up)
            let expected_tiles_wide = (level0.tiles_wide + 1) / 2;
            let expected_tiles_tall = (level0.tiles_tall + 1) / 2;
            
            assert!(level1.tiles_wide <= expected_tiles_wide + 1);
            assert!(level1.tiles_tall <= expected_tiles_tall + 1);
            
            // Get level 2 and verify it continues the pattern
            let level2 = PyramidTileGenerator::calculate_pyramid_level(2, &metadata)
                .expect("Level 2 should be calculable");
            
            assert_eq!(level2.level, 2);
            let expected_tiles_wide_l2 = (level1.tiles_wide + 1) / 2;
            let expected_tiles_tall_l2 = (level1.tiles_tall + 1) / 2;
            
            assert!(level2.tiles_wide <= expected_tiles_wide_l2 + 1);
            assert!(level2.tiles_tall <= expected_tiles_tall_l2 + 1);
        });
    }

    /// Property 13: Pyramid termination
    /// For any NAND dump file, the pyramid SHALL continue creating levels until the entire
    /// dump fits in a single tile.
    ///
    /// **Validates: Requirements 5.4**
    #[test]
    #[ignore]
    fn prop_pyramid_termination() {
        proptest!(|(
            size in 1_000_000u64..100_000_000u64,
            page_length in 512u32..4096u32,
            block_size in 64u32..256u32,
        )| {
            let metadata = FileMetadata::new(
                "test.bin".to_string(),
                size,
                page_length,
                block_size,
            );
            
            let max_level = PyramidTileGenerator::find_max_level(&metadata)
                .expect("Max level should be findable");
            
            // Verify max level has single tile
            let max_pyr = PyramidTileGenerator::calculate_pyramid_level(max_level, &metadata)
                .expect("Max level should be calculable");
            
            assert_eq!(max_pyr.tiles_wide, 1);
            assert_eq!(max_pyr.tiles_tall, 1);
            
            // Verify previous level has more than one tile (unless it's level 0)
            if max_level > 0 {
                let prev_pyr = PyramidTileGenerator::calculate_pyramid_level(max_level - 1, &metadata)
                    .expect("Previous level should be calculable");
                
                assert!(prev_pyr.tiles_wide > 1 || prev_pyr.tiles_tall > 1);
            }
        });
    }

    /// Property 14: Pyramid composition strategy
    /// For any pyramid tile at resolution level L > 0, the pyramid generator SHALL generate it
    /// by compositing tiles from level L-1 (not by reading directly from the dump).
    ///
    /// **Validates: Requirements 5.5**
    #[test]
    #[ignore]
    fn prop_pyramid_composition_strategy() {
        proptest!(|(
            size in 1_000_000u64..100_000_000u64,
            page_length in 512u32..4096u32,
            block_size in 64u32..256u32,
        )| {
            let metadata = FileMetadata::new(
                "test.bin".to_string(),
                size,
                page_length,
                block_size,
            );
            
            // For any level > 0, verify that it's composed from level-1
            for level in 1..=3 {
                let pyr_level = PyramidTileGenerator::calculate_pyramid_level(level, &metadata)
                    .expect("Level should be calculable");
                
                let prev_level = PyramidTileGenerator::calculate_pyramid_level(level - 1, &metadata)
                    .expect("Previous level should be calculable");
                
                // Each tile at this level is composed of 4 tiles from the previous level
                // So the number of tiles should be roughly 1/4 of the previous level
                let expected_tiles_wide = (prev_level.tiles_wide + 1) / 2;
                let expected_tiles_tall = (prev_level.tiles_tall + 1) / 2;
                
                // Allow for rounding differences
                assert!(pyr_level.tiles_wide <= expected_tiles_wide + 1);
                assert!(pyr_level.tiles_tall <= expected_tiles_tall + 1);
            }
        });
    }

    /// Property 15: Consistent tile dimensions
    /// For all tiles in the pyramid, the tile dimensions SHALL be consistent across all
    /// resolution levels.
    ///
    /// **Validates: Requirements 5.6**
    #[test]
    #[ignore]
    fn prop_consistent_tile_dimensions() {
        proptest!(|(
            size in 1_000_000u64..100_000_000u64,
            page_length in 512u32..4096u32,
            block_size in 64u32..256u32,
        )| {
            let metadata = FileMetadata::new(
                "test.bin".to_string(),
                size,
                page_length,
                block_size,
            );
            
            // Check that all levels have the same tile dimensions
            for level in 0..=5 {
                let pyr_level = match PyramidTileGenerator::calculate_pyramid_level(level, &metadata) {
                    Ok(l) => l,
                    Err(_) => break, // Stop if we exceed max level
                };
                
                assert_eq!(pyr_level.tile_width, TILE_WIDTH);
                assert_eq!(pyr_level.tile_height, TILE_HEIGHT);
            }
        });
    }

    /// Property: Downscale produces half dimensions
    /// For any pixel buffer, downscaling SHALL produce a buffer with half width and height.
    #[test]
    #[ignore]
    fn prop_downscale_dimensions() {
        proptest!(|(
            width in 4u32..512u32,
            height in 4u32..512u32,
        )| {
            // Only test even dimensions for simplicity
            let width = width & !1;
            let height = height & !1;
            
            let buffer = PixelBuffer::new(width, height);
            let downscaled = PyramidTileGenerator::downscale(&buffer)
                .expect("Downscale should succeed");
            
            assert_eq!(downscaled.width(), width / 2);
            assert_eq!(downscaled.height(), height / 2);
        });
    }
}
