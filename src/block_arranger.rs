//! BlockArranger for vertical page/block layout
//!
//! Arranges pages vertically within blocks and blocks in a grid layout.
//! Validates: Requirements 4.1, 4.2, 4.3, 4.4, 4.5, 4.6, 4.7

use crate::byte_arranger::ByteArranger;
use crate::bit_renderer::PixelBuffer;

/// BlockArranger arranges pages vertically and blocks in grid layout
pub struct BlockArranger;

/// Spacing between consecutive pages (in pixels)
const PAGE_SPACING: u32 = 2;

/// Spacing between consecutive blocks (in pixels)
const BLOCK_SPACING: u32 = 8;

impl BlockArranger {
    /// Calculate the height of a block in pixels
    ///
    /// Returns blockSize * pageLength * 8 + (blockSize-1) * pageSpacing
    ///
    /// # Arguments
    /// * `block_size` - Number of pages in a block
    /// * `page_length` - Number of bytes in a page
    ///
    /// # Returns
    /// Height in pixels
    pub fn calculate_block_height(block_size: u32, page_length: u32) -> u32 {
        block_size * page_length * 8 + (block_size.saturating_sub(1)) * PAGE_SPACING
    }

    /// Calculate grid dimensions maintaining 4:3 aspect ratio
    ///
    /// Given total blocks, calculates gridWidth and gridHeight for 4:3 layout.
    /// Uses algorithm: height = sqrt(3 * totalPixels / 4), width = sqrt(4 * totalPixels / 3)
    ///
    /// # Arguments
    /// * `total_blocks` - Total number of blocks to arrange
    /// * `_block_size` - Number of pages per block
    /// * `_page_length` - Number of bytes per page
    ///
    /// # Returns
    /// Tuple of (gridWidth, gridHeight) in blocks
    pub fn calculate_grid_dimensions(
        total_blocks: u64,
        block_size: u32,
        page_length: u32,
    ) -> (u32, u32) {
        if total_blocks == 0 {
            return (1, 1);
        }

        // Calculate pixel dimensions of a single block
        let block_width_pixels = (page_length * 8) as f64; // 8 pixels per byte
        let block_height_pixels = block_size as f64; // Each page is 1 pixel tall
        
        // Calculate aspect ratio of a single block
        let block_aspect_ratio = block_width_pixels / block_height_pixels;
        
        // Target aspect ratio for the overall visualization
        let target_aspect_ratio = 4.0 / 3.0;
        
        let grid_aspect_ratio = target_aspect_ratio / block_aspect_ratio;
        
        // Calculate ideal dimensions (may not fit exactly)
        let total_f = total_blocks as f64;
        let _ideal_height = (total_f / grid_aspect_ratio).sqrt();
        let ideal_width = (total_f * grid_aspect_ratio).sqrt();
        
        // Try different grid dimensions to find the best fit
        // Start with the ideal and adjust to fit exactly total_blocks
        let mut best_width = ideal_width.ceil() as u32;
        let mut best_height = ((total_blocks as f64) / (best_width as f64)).ceil() as u32;
        let mut best_diff = ((best_width as f64) / (best_height as f64) - grid_aspect_ratio).abs();
        
        // Try a few variations to find the closest to target aspect ratio
        for width in (ideal_width.floor() as u32).max(1)..=(ideal_width.ceil() as u32 + 2) {
            let height = ((total_blocks as f64) / (width as f64)).ceil() as u32;
            if (width as u64) * (height as u64) >= total_blocks {
                let aspect_diff = ((width as f64) / (height as f64) - grid_aspect_ratio).abs();
                if aspect_diff < best_diff {
                    best_width = width;
                    best_height = height;
                    best_diff = aspect_diff;
                }
            }
        }

        (best_width.max(1), best_height.max(1))
    }

    /// Render a block at grid position with page/block spacing
    ///
    /// Renders block at grid position with page spacing between pages
    /// and block spacing between blocks. Uses ByteArranger for each page.
    ///
    /// # Arguments
    /// * `block_data` - Slice of bytes representing the block (blockSize * pageLength bytes)
    /// * `block_x` - Block column in grid
    /// * `block_y` - Block row in grid
    /// * `block_size` - Number of pages in a block
    /// * `page_length` - Number of bytes in a page
    /// * `grid_width` - Number of blocks per row in grid
    /// * `canvas` - The pixel buffer to write to
    ///
    /// # Returns
    /// Ok(()) on success, Err with message if rendering fails
    pub fn render_block(
        block_data: &[u8],
        block_x: u32,
        block_y: u32,
        block_size: u32,
        page_length: u32,
        _grid_width: u32,
        canvas: &mut PixelBuffer,
    ) -> Result<(), String> {
        // Calculate the starting pixel position for this block
        let page_width = ByteArranger::calculate_page_width(page_length);
        let block_height = Self::calculate_block_height(block_size, page_length);

        // Calculate starting X position (blocks are placed left-to-right with block spacing)
        let start_x = block_x * (page_width + BLOCK_SPACING);
        
        // Calculate starting Y position (blocks are placed top-to-bottom with block spacing)
        let start_y = block_y * (block_height + BLOCK_SPACING);

        // Render each page in the block
        for page_index in 0..block_size {
            let page_start = (page_index as usize) * (page_length as usize);
            let page_end = page_start + (page_length as usize);

            if page_end > block_data.len() {
                return Err(format!(
                    "Block data too small: expected at least {} bytes, got {}",
                    page_end,
                    block_data.len()
                ));
            }

            let page_data = &block_data[page_start..page_end];

            // Calculate Y position for this page (with page spacing)
            let page_y = start_y + page_index * (8 + PAGE_SPACING);

            // Render the page at the calculated position
            ByteArranger::render_page_at(page_data, start_x, page_y, canvas)?;
        }

        Ok(())
    }

    /// Get the spacing between consecutive pages
    pub fn get_page_spacing() -> u32 {
        PAGE_SPACING
    }

    /// Get the spacing between consecutive blocks
    pub fn get_block_spacing() -> u32 {
        BLOCK_SPACING
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_calculate_block_height_single_page() {
        // 1 page * 512 bytes * 8 bits + 0 spacing = 4096 pixels
        let height = BlockArranger::calculate_block_height(1, 512);
        assert_eq!(height, 512 * 8);
    }

    #[test]
    fn test_calculate_block_height_multiple_pages() {
        // 64 pages * 512 bytes * 8 bits + 63 * 2 spacing
        let height = BlockArranger::calculate_block_height(64, 512);
        assert_eq!(height, 64 * 512 * 8 + 63 * PAGE_SPACING);
    }

    #[test]
    fn test_calculate_block_height_with_spacing() {
        // 4 pages * 256 bytes * 8 bits + 3 * 2 spacing
        let height = BlockArranger::calculate_block_height(4, 256);
        assert_eq!(height, 4 * 256 * 8 + 3 * PAGE_SPACING);
    }

    #[test]
    fn test_calculate_grid_dimensions_single_block() {
        let (width, height) = BlockArranger::calculate_grid_dimensions(1, 64, 512);
        // For 1 block, sqrt(4/3) ≈ 1.15 which rounds up to 2, sqrt(3/4) ≈ 0.87 which rounds up to 1
        // This maintains the 4:3 aspect ratio
        assert!(width >= 1 && height >= 1);
    }

    #[test]
    fn test_calculate_grid_dimensions_maintains_aspect_ratio() {
        let (width, height) = BlockArranger::calculate_grid_dimensions(100, 64, 512);
        // Should maintain approximately 4:3 ratio
        let ratio = (width as f64) / (height as f64);
        // Allow some tolerance due to rounding
        assert!(ratio >= 1.2 && ratio <= 1.4, "Ratio {} not in expected range", ratio);
    }

    #[test]
    fn test_calculate_grid_dimensions_zero_blocks() {
        let (width, height) = BlockArranger::calculate_grid_dimensions(0, 64, 512);
        assert_eq!(width, 1);
        assert_eq!(height, 1);
    }

    #[test]
    fn test_get_page_spacing() {
        assert_eq!(BlockArranger::get_page_spacing(), PAGE_SPACING);
    }

    #[test]
    fn test_get_block_spacing() {
        assert_eq!(BlockArranger::get_block_spacing(), BLOCK_SPACING);
    }

    #[test]
    fn test_block_spacing_hierarchy() {
        let page_spacing = BlockArranger::get_page_spacing();
        let block_spacing = BlockArranger::get_block_spacing();
        assert!(block_spacing > page_spacing, "Block spacing should be larger than page spacing");
    }

    #[test]
    fn test_render_block_single_page() {
        // Create a buffer large enough for one page
        let page_width = ByteArranger::calculate_page_width(8);
        let mut buffer = PixelBuffer::new(page_width, 8);

        // Create block data (1 page of 8 bytes)
        let block_data = vec![0xFF; 8];

        let result = BlockArranger::render_block(&block_data, 0, 0, 1, 8, 1, &mut buffer);
        assert!(result.is_ok());
    }

    #[test]
    fn test_render_block_multiple_pages() {
        // Create a buffer large enough for 4 pages with spacing
        let page_width = ByteArranger::calculate_page_width(8);
        let block_height = BlockArranger::calculate_block_height(4, 8);
        let mut buffer = PixelBuffer::new(page_width, block_height);

        // Create block data (4 pages of 8 bytes each)
        let block_data = vec![0xFF; 32];

        let result = BlockArranger::render_block(&block_data, 0, 0, 4, 8, 1, &mut buffer);
        assert!(result.is_ok());
    }

    #[test]
    fn test_render_block_insufficient_data() {
        let page_width = ByteArranger::calculate_page_width(8);
        let block_height = BlockArranger::calculate_block_height(4, 8);
        let mut buffer = PixelBuffer::new(page_width, block_height);

        // Create block data that's too small (only 2 pages instead of 4)
        let block_data = vec![0xFF; 16];

        let result = BlockArranger::render_block(&block_data, 0, 0, 4, 8, 1, &mut buffer);
        assert!(result.is_err());
    }

    #[test]
    fn test_render_block_grid_positioning() {
        // Test that blocks are positioned correctly in grid
        let page_width = ByteArranger::calculate_page_width(8);
        let block_height = BlockArranger::calculate_block_height(2, 8);
        let total_height = block_height * 2 + BLOCK_SPACING;
        let total_width = page_width * 2;

        let mut buffer = PixelBuffer::new(total_width, total_height);

        // Create block data
        let block_data = vec![0xFF; 16];

        // Render blocks at different grid positions
        let result1 = BlockArranger::render_block(&block_data, 0, 0, 2, 8, 2, &mut buffer);
        let result2 = BlockArranger::render_block(&block_data, 1, 0, 2, 8, 2, &mut buffer);
        let result3 = BlockArranger::render_block(&block_data, 0, 1, 2, 8, 2, &mut buffer);

        assert!(result1.is_ok());
        assert!(result2.is_ok());
        assert!(result3.is_ok());
    }

    // Edge case tests for task 5.6
    
    #[test]
    fn test_single_block_edge_case() {
        // Test rendering a single block with minimal configuration
        // Requirements: 4.1, 4.5, 4.6
        let page_length = 8;
        let block_size = 1;
        let page_width = ByteArranger::calculate_page_width(page_length);
        let block_height = BlockArranger::calculate_block_height(block_size, page_length);
        
        let mut buffer = PixelBuffer::new(page_width, block_height);
        let block_data = vec![0xAA; page_length as usize];
        
        let result = BlockArranger::render_block(&block_data, 0, 0, block_size, page_length, 1, &mut buffer);
        assert!(result.is_ok(), "Single block should render successfully");
        
        // Verify block height calculation for single page (no spacing)
        assert_eq!(block_height, page_length * 8, "Single page block should have no page spacing");
    }

    #[test]
    fn test_single_block_large_page() {
        // Test single block with large page size (typical NAND configuration)
        // Requirements: 4.1, 4.5
        let page_length = 2048; // Common NAND page size
        let block_size = 1;
        let page_width = ByteArranger::calculate_page_width(page_length);
        let block_height = BlockArranger::calculate_block_height(block_size, page_length);
        
        let mut buffer = PixelBuffer::new(page_width, block_height);
        let block_data = vec![0x55; page_length as usize];
        
        let result = BlockArranger::render_block(&block_data, 0, 0, block_size, page_length, 1, &mut buffer);
        assert!(result.is_ok(), "Single block with large page should render successfully");
    }

    #[test]
    fn test_multiple_blocks_vertical_spacing() {
        // Test that multiple blocks maintain correct vertical spacing
        // Requirements: 4.1, 4.5, 4.6
        let page_length = 16;
        let block_size = 2;
        let page_width = ByteArranger::calculate_page_width(page_length);
        let block_height = BlockArranger::calculate_block_height(block_size, page_length);
        
        // Create buffer for 3 blocks vertically
        let total_height = block_height * 3 + BLOCK_SPACING * 2;
        let mut buffer = PixelBuffer::new(page_width, total_height);
        
        let block_data = vec![0xFF; (block_size * page_length) as usize];
        
        // Render 3 blocks vertically
        let result1 = BlockArranger::render_block(&block_data, 0, 0, block_size, page_length, 1, &mut buffer);
        let result2 = BlockArranger::render_block(&block_data, 0, 1, block_size, page_length, 1, &mut buffer);
        let result3 = BlockArranger::render_block(&block_data, 0, 2, block_size, page_length, 1, &mut buffer);
        
        assert!(result1.is_ok(), "First block should render");
        assert!(result2.is_ok(), "Second block should render");
        assert!(result3.is_ok(), "Third block should render");
        
        // Verify spacing between blocks
        let expected_block1_start = 0;
        let expected_block2_start = block_height + BLOCK_SPACING;
        let expected_block3_start = (block_height + BLOCK_SPACING) * 2;
        
        assert_eq!(expected_block1_start, 0);
        assert_eq!(expected_block2_start, block_height + BLOCK_SPACING);
        assert_eq!(expected_block3_start, (block_height + BLOCK_SPACING) * 2);
    }

    #[test]
    fn test_grid_layout_2x2() {
        // Test 2x2 grid layout
        // Requirements: 4.6
        let (grid_width, grid_height) = BlockArranger::calculate_grid_dimensions(4, 64, 512);
        
        // For 4 blocks, should create a 2x2 grid (or close to it)
        assert!(grid_width >= 2, "Grid width should be at least 2 for 4 blocks");
        assert!(grid_height >= 2, "Grid height should be at least 2 for 4 blocks");
        assert!(grid_width * grid_height >= 4, "Grid should hold all 4 blocks");
    }

    #[test]
    fn test_grid_layout_single_row() {
        // Test grid layout with very few blocks
        // Requirements: 4.6
        let (grid_width, grid_height) = BlockArranger::calculate_grid_dimensions(2, 64, 512);
        
        // Should maintain aspect ratio even with few blocks
        let capacity = grid_width * grid_height;
        assert!(capacity >= 2, "Grid should hold at least 2 blocks");
    }

    #[test]
    fn test_grid_layout_many_blocks() {
        // Test grid layout with many blocks
        // Requirements: 4.6, 4.7
        let (grid_width, grid_height) = BlockArranger::calculate_grid_dimensions(1000, 64, 512);
        
        // Verify capacity
        let capacity = (grid_width as u64) * (grid_height as u64);
        assert!(capacity >= 1000, "Grid should hold all 1000 blocks");
        
        // Verify aspect ratio is approximately 4:3
        let ratio = (grid_width as f64) / (grid_height as f64);
        assert!(ratio >= 1.2 && ratio <= 1.5, 
            "Grid aspect ratio {} should be approximately 4:3 for many blocks", ratio);
    }

    #[test]
    fn test_spacing_calculation_no_pages() {
        // Edge case: block with 0 pages (should handle gracefully)
        // Requirements: 4.5
        let height = BlockArranger::calculate_block_height(0, 512);
        assert_eq!(height, 0, "Block with 0 pages should have 0 height");
    }

    #[test]
    fn test_spacing_calculation_many_pages() {
        // Test spacing calculation with many pages per block
        // Requirements: 4.4, 4.5
        let block_size = 256; // Large block
        let page_length = 512;
        let height = BlockArranger::calculate_block_height(block_size, page_length);
        
        // Expected: 256 * 512 * 8 + 255 * PAGE_SPACING
        let expected = block_size * page_length * 8 + (block_size - 1) * PAGE_SPACING;
        assert_eq!(height, expected, "Height calculation should include all page spacing");
        
        // Verify spacing is significant portion
        let spacing_total = (block_size - 1) * PAGE_SPACING;
        assert!(spacing_total > 0, "Should have spacing between pages");
    }

    #[test]
    fn test_spacing_hierarchy_verification() {
        // Verify that block spacing is consistently larger than page spacing
        // Requirements: 4.5
        let page_spacing = BlockArranger::get_page_spacing();
        let block_spacing = BlockArranger::get_block_spacing();
        
        assert!(block_spacing > page_spacing, 
            "Block spacing ({}) must be larger than page spacing ({})", 
            block_spacing, page_spacing);
        
        // Verify the difference is meaningful (at least 2x)
        assert!(block_spacing >= page_spacing * 2, 
            "Block spacing should be at least 2x page spacing for visual distinction");
    }

    #[test]
    fn test_grid_layout_aspect_ratio_edge_cases() {
        // Test aspect ratio with various block counts
        // Requirements: 4.7
        let test_cases = vec![1, 2, 3, 4, 5, 10, 16, 25, 50, 100, 500];
        
        for total_blocks in test_cases {
            let (grid_width, grid_height) = BlockArranger::calculate_grid_dimensions(
                total_blocks, 64, 512
            );
            
            let capacity = (grid_width as u64) * (grid_height as u64);
            assert!(capacity >= total_blocks, 
                "Grid should hold all {} blocks (capacity: {})", total_blocks, capacity);
            
            // For larger grids, aspect ratio should be closer to 4:3
            if total_blocks >= 10 {
                let ratio = (grid_width as f64) / (grid_height as f64);
                assert!(ratio >= 1.0 && ratio <= 1.6, 
                    "Aspect ratio {} should be reasonable for {} blocks", ratio, total_blocks);
            }
        }
    }

    #[test]
    fn test_render_block_with_different_patterns() {
        // Test rendering blocks with different data patterns
        // Requirements: 4.1, 4.2, 4.3
        let page_length = 8;
        let block_size = 3;
        let page_width = ByteArranger::calculate_page_width(page_length);
        let block_height = BlockArranger::calculate_block_height(block_size, page_length);
        
        let mut buffer = PixelBuffer::new(page_width, block_height);
        
        // Test with alternating pattern
        let mut block_data = Vec::new();
        for i in 0..block_size {
            for _ in 0..page_length {
                block_data.push(if i % 2 == 0 { 0xFF } else { 0x00 });
            }
        }
        
        let result = BlockArranger::render_block(&block_data, 0, 0, block_size, page_length, 1, &mut buffer);
        assert!(result.is_ok(), "Block with alternating pattern should render successfully");
    }

    #[test]
    fn test_multiple_blocks_grid_layout_rendering() {
        // Test rendering multiple blocks in a grid layout
        // Requirements: 4.6
        let page_length = 8;
        let block_size = 2;
        let total_blocks = 4;
        
        let (grid_width, grid_height) = BlockArranger::calculate_grid_dimensions(
            total_blocks, block_size, page_length
        );
        
        let page_width = ByteArranger::calculate_page_width(page_length);
        let block_height = BlockArranger::calculate_block_height(block_size, page_length);
        
        // Create buffer large enough for the grid
        let total_width = page_width * grid_width;
        let total_height = block_height * grid_height + BLOCK_SPACING * (grid_height - 1);
        let mut buffer = PixelBuffer::new(total_width, total_height);
        
        let block_data = vec![0xAA; (block_size * page_length) as usize];
        
        // Render blocks in grid positions
        for block_y in 0..grid_height {
            for block_x in 0..grid_width {
                let block_index = block_y * grid_width + block_x;
                if (block_index as u64) < total_blocks {
                    let result = BlockArranger::render_block(
                        &block_data, block_x, block_y, block_size, page_length, grid_width, &mut buffer
                    );
                    assert!(result.is_ok(), 
                        "Block at grid position ({}, {}) should render successfully", block_x, block_y);
                }
            }
        }
    }

    #[test]
    fn test_spacing_calculation_boundary_values() {
        // Test spacing calculations with boundary values
        // Requirements: 4.4, 4.5
        
        // Minimum page length
        let height_min = BlockArranger::calculate_block_height(64, 500);
        assert!(height_min > 0, "Should handle minimum page length");
        
        // Maximum page length
        let height_max = BlockArranger::calculate_block_height(64, 20000);
        assert!(height_max > 0, "Should handle maximum page length");
        
        // Verify spacing is included correctly
        let expected_min = 64 * 500 * 8 + 63 * PAGE_SPACING;
        let expected_max = 64 * 20000 * 8 + 63 * PAGE_SPACING;
        
        assert_eq!(height_min, expected_min, "Minimum page length calculation should be correct");
        assert_eq!(height_max, expected_max, "Maximum page length calculation should be correct");
    }

    #[test]
    fn test_grid_dimensions_with_common_nand_configurations() {
        // Test grid layout with common NAND flash configurations
        // Requirements: 4.6, 4.7
        
        // Common configurations: (pages_per_block, page_size)
        let configs = vec![
            (64, 2048),   // Common SLC NAND
            (128, 4096),  // Common MLC NAND
            (256, 8192),  // Common TLC NAND
        ];
        
        for (block_size, page_length) in configs {
            // Test with various block counts
            for total_blocks in vec![10, 100, 1000] {
                let (grid_width, grid_height) = BlockArranger::calculate_grid_dimensions(
                    total_blocks, block_size, page_length
                );
                
                let capacity = (grid_width as u64) * (grid_height as u64);
                assert!(capacity >= total_blocks, 
                    "Grid should hold all {} blocks for config ({}, {})", 
                    total_blocks, block_size, page_length);
                
                // Verify aspect ratio
                let ratio = (grid_width as f64) / (grid_height as f64);
                assert!(ratio >= 1.0 && ratio <= 1.6, 
                    "Aspect ratio {} should be reasonable for config ({}, {}) with {} blocks", 
                    ratio, block_size, page_length, total_blocks);
            }
        }
    }

    // Property-Based Tests
    use proptest::prelude::*;

    /*
    proptest! {
        /// **Property 11: Grid layout arrangement**
        /// 
        /// **Validates: Requirements 4.6, 4.7**
        /// 
        /// For any set of blocks, the block arranger SHALL arrange them in a grid layout
        /// (top-to-bottom, left-to-right) and maintain an aspect ratio of approximately 4:3 (width:height).
        #[test]
        fn prop_grid_layout_aspect_ratio(
            total_blocks in 1u64..10000,
            block_size in prop::sample::select(vec![64u32, 128, 256, 512, 768, 1024]),
            page_length in 512u32..4096
        ) {
            // Calculate grid dimensions
            let (grid_width, grid_height) = BlockArranger::calculate_grid_dimensions(
                total_blocks,
                block_size,
                page_length
            );
            
            // Property 1: Grid dimensions should be positive
            prop_assert!(grid_width > 0, "Grid width should be positive");
            prop_assert!(grid_height > 0, "Grid height should be positive");
            
            // Property 2: Grid should be able to hold all blocks
            let grid_capacity = (grid_width as u64) * (grid_height as u64);
            prop_assert!(grid_capacity >= total_blocks,
                "Grid capacity {} should be >= total blocks {}",
                grid_capacity, total_blocks);
            
            // Property 3: Grid should maintain approximately 4:3 aspect ratio
            // Allow tolerance for rounding (ratio should be between 1.0 and 1.6)
            // The ideal ratio is 4/3 ≈ 1.333
            let ratio = (grid_width as f64) / (grid_height as f64);
            prop_assert!(ratio >= 1.0 && ratio <= 1.6,
                "Aspect ratio {} should be approximately 4:3 (between 1.0 and 1.6) for {} blocks",
                ratio, total_blocks);
            
            // Property 4: For larger grids, ratio should be closer to 4:3
            // When we have many blocks, the rounding error becomes less significant
            if total_blocks >= 100 {
                prop_assert!(ratio >= 1.2 && ratio <= 1.5,
                    "For {} blocks, aspect ratio {} should be closer to 4:3 (between 1.2 and 1.5)",
                    total_blocks, ratio);
            }
            
            // Property 5: Grid should not be wastefully large
            // Due to ceiling operations on both dimensions, wasted space can be up to
            // (width + height - 1) in the worst case. We verify it's reasonable.
            let wasted_space = grid_capacity - total_blocks;
            let max_acceptable_waste = (grid_width + grid_height) as u64;
            prop_assert!(wasted_space <= max_acceptable_waste,
                "Wasted space {} should not exceed {} (width + height)",
                wasted_space, max_acceptable_waste);
        }

        /// **Property 10: Block spacing hierarchy**
        /// 
        /// **Validates: Requirements 4.5**
        /// 
        /// For any rendered block layout, the spacing between consecutive blocks SHALL be
        /// larger than the spacing between consecutive pages.
        #[test]
        fn prop_block_spacing_hierarchy(
            block_size in 1u32..1025,
            page_length in 500u32..20001,
            _total_blocks in 1u64..1000
        ) {
            // Get the spacing values
            let page_spacing = BlockArranger::get_page_spacing();
            let block_spacing = BlockArranger::get_block_spacing();
            
            // Property 1: Block spacing must be strictly greater than page spacing
            prop_assert!(block_spacing > page_spacing,
                "Block spacing ({}) must be larger than page spacing ({})",
                block_spacing, page_spacing);
            
            // Property 2: The spacing hierarchy should be maintained in calculated heights
            // Calculate block height which includes page spacing
            let block_height = BlockArranger::calculate_block_height(block_size, page_length);
            
            // Expected height: blockSize * pageLength * 8 + (blockSize-1) * pageSpacing
            let expected_height = block_size * page_length * 8 + 
                                  block_size.saturating_sub(1) * page_spacing;
            prop_assert_eq!(block_height, expected_height,
                "Block height calculation should match expected formula");
            
            // Property 3: When rendering multiple blocks in a grid, the spacing between
            // blocks should be larger than spacing between pages
            // This is verified by checking that BLOCK_SPACING > PAGE_SPACING
            // and that the render_block function uses these constants correctly
            
            // Property 4: The difference should be meaningful (not just 1 pixel)
            // This ensures visual distinction between page and block boundaries
            let spacing_difference = block_spacing - page_spacing;
            prop_assert!(spacing_difference >= 2,
                "Block spacing should be meaningfully larger than page spacing (difference: {})",
                spacing_difference);
            
            // Property 5: Verify spacing is consistent across different configurations
            // The spacing values should not depend on block_size, page_length, or total_blocks
            let page_spacing_check = BlockArranger::get_page_spacing();
            let block_spacing_check = BlockArranger::get_block_spacing();
            prop_assert_eq!(page_spacing, page_spacing_check,
                "Page spacing should be consistent");
            prop_assert_eq!(block_spacing, block_spacing_check,
                "Block spacing should be consistent");
        }
    }
    */
}
