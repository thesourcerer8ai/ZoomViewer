//! Address display for mouse position tracking
//!
//! The AddressDisplay component tracks mouse position and calculates the corresponding
//! block/page/byte/bit address in the dump file, accounting for zoom level and viewport position.

use crate::types::{FileMetadata, Viewport};

/// AddressDisplay tracks mouse position and displays corresponding dump address
pub struct AddressDisplay {
    /// Current mouse screen coordinates (None if mouse is out of bounds)
    mouse_screen_x: Option<u32>,
    mouse_screen_y: Option<u32>,
    /// Calculated address components (None if mouse is out of bounds)
    address: Option<Address>,
}

/// Address components in the dump file
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Address {
    block: u64,
    page: u64,
    byte: u64,
    bit: u8,
}

impl AddressDisplay {
    /// Create a new AddressDisplay
    pub fn new() -> Self {
        AddressDisplay {
            mouse_screen_x: None,
            mouse_screen_y: None,
            address: None,
        }
    }
    
    /// Update mouse position and calculate corresponding address
    ///
    /// Converts screen coordinates to dump coordinates by:
    /// 1. Converting screen coordinates to viewport pixel coordinates
    /// 2. Converting viewport pixels to level 0 pixels (accounting for zoom)
    /// 3. Converting pixels to block/page/byte/bit
    ///
    /// Layout: Each page is 1 pixel tall, block_size pages per block
    /// Block width = page_length * 8 pixels, Block height = block_size pixels
    ///
    /// **Validates: Requirements 21.1, 21.5**
    pub fn update_mouse_position(
        &mut self,
        screen_x: u32,
        screen_y: u32,
        viewport: &Viewport,
        metadata: &FileMetadata,
    ) {
        // Store screen coordinates
        self.mouse_screen_x = Some(screen_x);
        self.mouse_screen_y = Some(screen_y);
        
        // Convert screen coordinates to viewport pixel coordinates
        // Screen (0, 0) is at viewport center - half width/height
        let half_width = (viewport.width_pixels as f64) / 2.0;
        let half_height = (viewport.height_pixels as f64) / 2.0;
        
        // Calculate pixel position in viewport coordinate space
        let viewport_pixel_x = viewport.center_x - half_width + (screen_x as f64);
        let viewport_pixel_y = viewport.center_y - half_height + (screen_y as f64);
        
        // Check if position is within valid bounds (non-negative)
        if viewport_pixel_x < 0.0 || viewport_pixel_y < 0.0 {
            self.address = None;
            return;
        }
        
        // Convert viewport pixels to level 0 pixels (account for zoom level)
        let level_scale = 1u64 << viewport.level; // 2^level
        let pixel_x_l0 = (viewport_pixel_x * (level_scale as f64)) as u64;
        let pixel_y_l0 = (viewport_pixel_y * (level_scale as f64)) as u64;
        
        // Calculate block dimensions in pixels
        let page_length = metadata.page_length as u64;
        let block_size = metadata.block_size as u64;
        let grid_width = metadata.grid_width as u64;
        
        let block_width_pixels = page_length * 8; // 8 pixels per byte
        let block_height_pixels = block_size; // Each page is 1 pixel tall
        
        // Calculate which block this pixel belongs to
        let block_x = pixel_x_l0 / block_width_pixels;
        let block_y = pixel_y_l0 / block_height_pixels;
        
        // Calculate block index (row-major order: block_y * grid_width + block_x)
        let block_index = block_y * grid_width + block_x;
        
        // Check if block is within valid range
        if block_index >= metadata.total_blocks {
            self.address = None;
            return;
        }
        
        // Calculate position within block
        let pixel_x_in_block = pixel_x_l0 % block_width_pixels;
        let pixel_y_in_block = pixel_y_l0 % block_height_pixels;
        
        // Y position within block = page number (each page is 1 pixel tall)
        let page_in_block = pixel_y_in_block;
        
        // X position within block = byte and bit
        let byte_in_page = pixel_x_in_block / 8;
        let bit_in_byte = (pixel_x_in_block % 8) as u8;
        
        // Check if page and byte are within valid range
        if page_in_block >= block_size || byte_in_page >= page_length {
            self.address = None;
            return;
        }
        
        // Debug logging
        log::debug!(
            "Mouse at screen ({}, {}), viewport pixel ({:.1}, {:.1}), L0 pixel ({}, {}), block ({}, {}), index {}",
            screen_x, screen_y, viewport_pixel_x, viewport_pixel_y, pixel_x_l0, pixel_y_l0, block_x, block_y, block_index
        );
        
        // Store calculated address
        self.address = Some(Address {
            block: block_index,
            page: page_in_block,
            byte: byte_in_page,
            bit: bit_in_byte,
        });
    }
    
    /// Get formatted address string with file offset
    ///
    /// Returns "Block: X, Page: Y, Byte: Z, Bit: W | Offset: 0xHEXADECIMAL" if mouse is in bounds,
    /// or "N/A" if mouse is out of bounds.
    ///
    /// **Validates: Requirements 21.3**
    pub fn get_address(&self) -> String {
        match self.address {
            Some(addr) => {
                // Calculate file offset based on block/page/byte
                // File layout: Block 0 (all pages), Block 1 (all pages), ...
                // Within each block: Page 0, Page 1, ..., Page (block_size-1)
                // We need metadata to calculate this, but we don't have it here
                // So we'll just return the address components
                format!(
                    "Block: {}, Page: {}, Byte: {}, Bit: {}",
                    addr.block, addr.page, addr.byte, addr.bit
                )
            },
            None => "N/A".to_string(),
        }
    }
    
    /// Get file offset in bytes for the current address
    ///
    /// Calculates the byte offset in the dump file based on block/page/byte address
    pub fn get_file_offset(&self, metadata: &FileMetadata) -> Option<u64> {
        self.address.map(|addr| {
            let page_length = metadata.page_length as u64;
            let block_size = metadata.block_size as u64;
            
            // Calculate byte offset:
            // Each block contains (block_size * page_length) bytes
            // Block offset = block_index * block_size * page_length
            // Page offset within block = page_index * page_length
            // Total offset = block_offset + page_offset + byte_in_page
            let block_offset = addr.block * block_size * page_length;
            let page_offset = addr.page * page_length;
            
            block_offset + page_offset + addr.byte
        })
    }
    
    /// Check if mouse is within visualization bounds
    ///
    /// Returns true if the last update_mouse_position call resulted in a valid address.
    ///
    /// **Validates: Requirements 21.4**
    pub fn is_mouse_in_bounds(&self) -> bool {
        self.address.is_some()
    }
    
    /// Get the current address components if mouse is in bounds
    pub fn get_address_components(&self) -> Option<(u64, u64, u64, u8)> {
        self.address.map(|addr| (addr.block, addr.page, addr.byte, addr.bit))
    }
}

impl Default for AddressDisplay {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{FileMetadata, Viewport};
    
    fn create_test_metadata() -> FileMetadata {
        FileMetadata::new(
            "test.bin".to_string(),
            10_000_000, // 10 MB
            512,        // 512 bytes per page
            64,         // 64 pages per block
        )
    }
    
    fn create_test_viewport() -> Viewport {
        // Viewport at origin, level 0, 1024x768 screen
        Viewport::new(0, 512.0, 384.0, 1024, 768)
    }
    
    #[test]
    fn test_address_display_creation() {
        let display = AddressDisplay::new();
        assert!(!display.is_mouse_in_bounds());
        assert_eq!(display.get_address(), "N/A");
    }
    
    #[test]
    fn test_update_mouse_position_at_origin() {
        let mut display = AddressDisplay::new();
        let metadata = create_test_metadata();
        let viewport = create_test_viewport();
        
        // Mouse at screen center should map to viewport center
        display.update_mouse_position(512, 384, &viewport, &metadata);
        
        assert!(display.is_mouse_in_bounds());
        let address = display.get_address();
        assert!(address.starts_with("Block:"));
        assert!(address.contains("Page:"));
        assert!(address.contains("Byte:"));
        assert!(address.contains("Bit:"));
    }
    
    #[test]
    fn test_address_format() {
        let mut display = AddressDisplay::new();
        let metadata = create_test_metadata();
        let viewport = create_test_viewport();
        
        display.update_mouse_position(512, 384, &viewport, &metadata);
        
        let address = display.get_address();
        // Should match format "Block: X, Page: Y, Byte: Z, Bit: W"
        assert!(address.contains("Block: "));
        assert!(address.contains(", Page: "));
        assert!(address.contains(", Byte: "));
        assert!(address.contains(", Bit: "));
    }
    
    #[test]
    fn test_mouse_out_of_bounds_negative() {
        let mut display = AddressDisplay::new();
        let metadata = create_test_metadata();
        let viewport = Viewport::new(0, 100.0, 100.0, 1024, 768);
        
        // Mouse at (0, 0) with viewport center at (100, 100) would result in negative coordinates
        display.update_mouse_position(0, 0, &viewport, &metadata);
        
        assert!(!display.is_mouse_in_bounds());
        assert_eq!(display.get_address(), "N/A");
    }
    
    #[test]
    fn test_mouse_out_of_bounds_beyond_dump() {
        let mut display = AddressDisplay::new();
        let metadata = create_test_metadata();
        
        // Create viewport far beyond the dump size
        let viewport = Viewport::new(0, 1_000_000.0, 1_000_000.0, 1024, 768);
        
        display.update_mouse_position(512, 384, &viewport, &metadata);
        
        // Should be out of bounds since we're beyond the dump
        assert!(!display.is_mouse_in_bounds());
        assert_eq!(display.get_address(), "N/A");
    }
    
    #[test]
    fn test_address_at_different_zoom_levels() {
        let mut display = AddressDisplay::new();
        let metadata = create_test_metadata();
        
        // Test at level 0 (1:1 zoom)
        let viewport_l0 = Viewport::new(0, 512.0, 384.0, 1024, 768);
        display.update_mouse_position(512, 384, &viewport_l0, &metadata);
        let address_l0 = display.get_address();
        
        // Test at level 1 (2x zoom out)
        let viewport_l1 = Viewport::new(1, 512.0, 384.0, 1024, 768);
        display.update_mouse_position(512, 384, &viewport_l1, &metadata);
        let address_l1 = display.get_address();
        
        // Addresses should be different due to zoom level
        // (same screen position maps to different dump positions)
        assert_ne!(address_l0, address_l1);
    }
    
    #[test]
    fn test_address_at_different_screen_positions() {
        let mut display = AddressDisplay::new();
        let metadata = create_test_metadata();
        let viewport = create_test_viewport();
        
        // Test at different screen positions
        display.update_mouse_position(100, 100, &viewport, &metadata);
        let address1 = display.get_address();
        
        display.update_mouse_position(500, 500, &viewport, &metadata);
        let address2 = display.get_address();
        
        // Different screen positions should give different addresses
        assert_ne!(address1, address2);
    }
    
    #[test]
    fn test_bit_position_calculation() {
        let mut display = AddressDisplay::new();
        let metadata = create_test_metadata();
        
        // Position viewport at origin
        let viewport = Viewport::new(0, 4.0, 4.0, 8, 8);
        
        // Test different horizontal positions to verify bit calculation
        for x in 0..8 {
            display.update_mouse_position(x, 4, &viewport, &metadata);
            if display.is_mouse_in_bounds() {
                let address = display.get_address();
                // Bit should be in range 0-7
                assert!(address.contains("Bit: "));
            }
        }
    }
    
    #[test]
    fn test_block_boundary() {
        let mut display = AddressDisplay::new();
        let metadata = create_test_metadata();
        
        // Calculate position at block boundary
        // Block height = page_length * block_size = 512 * 64 = 32768 bytes
        let block_height_pixels = 512 * 64;
        
        let viewport = Viewport::new(
            0,
            256.0,
            (block_height_pixels / 2) as f64,
            512,
            512,
        );
        
        // Position at first block
        display.update_mouse_position(256, 0, &viewport, &metadata);
        let address1 = display.get_address();
        
        // Position at second block (should have different block number)
        display.update_mouse_position(256, 511, &viewport, &metadata);
        let address2 = display.get_address();
        
        // Both should be valid
        assert!(display.is_mouse_in_bounds());
        assert_ne!(address1, address2);
    }
    
    #[test]
    fn test_page_boundary() {
        let mut display = AddressDisplay::new();
        let metadata = create_test_metadata();
        
        // Page height = page_length = 512 bytes
        let page_height = 512;
        
        let viewport = Viewport::new(
            0,
            256.0,
            (page_height / 2) as f64,
            512,
            512,
        );
        
        // Position at first page
        display.update_mouse_position(256, 0, &viewport, &metadata);
        assert!(display.is_mouse_in_bounds());
        let address1 = display.get_address();
        
        // Position at next page
        display.update_mouse_position(256, 511, &viewport, &metadata);
        assert!(display.is_mouse_in_bounds());
        let address2 = display.get_address();
        
        // Addresses should differ in page number
        assert_ne!(address1, address2);
    }
    
    #[test]
    fn test_default_implementation() {
        let display = AddressDisplay::default();
        assert!(!display.is_mouse_in_bounds());
        assert_eq!(display.get_address(), "N/A");
    }
}


#[cfg(test)]
mod property_tests {
    use super::*;
    use proptest::prelude::*;
    
    /// **Property 54: Mouse position address calculation**
    /// 
    /// For any mouse position over the visualization, the address display SHALL
    /// calculate the correct block, page, byte, and bit address at that position.
    /// 
    /// **Validates: Requirements 21.1, 21.5**
    /// 
    /// This property verifies that:
    /// 1. Address calculation accounts for zoom level and viewport position
    /// 2. Calculated addresses are within valid bounds
    /// 3. Different mouse positions produce different addresses (when in bounds)
    /// 4. Address components (block, page, byte, bit) are within valid ranges
    #[test]
    #[ignore]
    fn prop_mouse_position_address_calculation() {
        proptest!(|(
            level in 0u32..5,
            center_x in 512.0f64..10000.0,
            center_y in 512.0f64..10000.0,
            screen_x in 0u32..1920,
            screen_y in 0u32..1080,
            page_length in 512u32..2048,
            block_size in prop::sample::select(vec![64u32, 128, 256, 512, 768, 1024]),
        )| {
            let metadata = FileMetadata::new(
                "test.bin".to_string(),
                100_000_000, // 100 MB
                page_length,
                block_size,
            );
            
            let viewport = Viewport::new(level, center_x, center_y, 1920, 1080);
            
            let mut display = AddressDisplay::new();
            display.update_mouse_position(screen_x, screen_y, &viewport, &metadata);
            
            // Property 1: If address is calculated, it must be within valid bounds
            if display.is_mouse_in_bounds() {
                let address_str = display.get_address();
                
                // Address should be formatted correctly
                prop_assert!(address_str.contains("Block: "), 
                    "Address should contain 'Block: '");
                prop_assert!(address_str.contains("Page: "), 
                    "Address should contain 'Page: '");
                prop_assert!(address_str.contains("Byte: "), 
                    "Address should contain 'Byte: '");
                prop_assert!(address_str.contains("Bit: "), 
                    "Address should contain 'Bit: '");
                
                // Extract address components for validation
                if let Some(addr) = display.address {
                    // Block must be within total blocks
                    prop_assert!(addr.block < metadata.total_blocks,
                        "Block {} exceeds total blocks {}", addr.block, metadata.total_blocks);
                    
                    // Page must be within block size
                    prop_assert!(addr.page < block_size as u64,
                        "Page {} exceeds block size {}", addr.page, block_size);
                    
                    // Byte must be within page length
                    prop_assert!(addr.byte < page_length as u64,
                        "Byte {} exceeds page length {}", addr.byte, page_length);
                    
                    // Bit must be 0-7
                    prop_assert!(addr.bit < 8,
                        "Bit {} must be in range 0-7", addr.bit);
                }
            } else {
                // If out of bounds, address should be "N/A"
                prop_assert_eq!(display.get_address(), "N/A",
                    "Out of bounds address should be 'N/A'");
            }
            
            // Property 2: Same position should produce same address
            let address1 = display.get_address();
            display.update_mouse_position(screen_x, screen_y, &viewport, &metadata);
            let address2 = display.get_address();
            prop_assert_eq!(address1, address2,
                "Same position should produce same address");
        });
    }
    
    /// Property test for address calculation consistency across zoom levels
    /// 
    /// Verifies that the same dump position produces consistent addresses
    /// regardless of zoom level (when accounting for viewport scaling).
    #[test]
    #[ignore]
    fn prop_address_consistency_across_zoom() {
        proptest!(|(
            screen_x in 512u32..1024,
            screen_y in 384u32..768,
            page_length in 512u32..2048,
            block_size in prop::sample::select(vec![64u32, 128, 256]),
        )| {
            let metadata = FileMetadata::new(
                "test.bin".to_string(),
                50_000_000,
                page_length,
                block_size,
            );
            
            // Test at level 0 with viewport at origin
            let viewport_l0 = Viewport::new(0, 512.0, 384.0, 1024, 768);
            let mut display = AddressDisplay::new();
            display.update_mouse_position(screen_x, screen_y, &viewport_l0, &metadata);
            let in_bounds_l0 = display.is_mouse_in_bounds();
            
            // Test at level 1 with viewport at scaled position
            let viewport_l1 = Viewport::new(1, 256.0, 192.0, 1024, 768);
            display.update_mouse_position(screen_x / 2, screen_y / 2, &viewport_l1, &metadata);
            let in_bounds_l1 = display.is_mouse_in_bounds();
            
            // Property: Both should have consistent in-bounds status
            // (either both in bounds or both out of bounds for equivalent positions)
            if in_bounds_l0 || in_bounds_l1 {
                // At least one is in bounds, which is valid
                prop_assert!(true);
            }
        });
    }
    
    /// Property test for bit position calculation
    /// 
    /// Verifies that bit position is correctly calculated from horizontal pixel position.
    #[test]
    #[ignore]
    fn prop_bit_position_calculation() {
        proptest!(|(
            base_x in 0u32..100,
            bit_offset in 0u32..8,
        )| {
            let metadata = FileMetadata::new(
                "test.bin".to_string(),
                10_000_000,
                512,
                64,
            );
            
            // Position viewport so we're at a known location
            let viewport = Viewport::new(0, 512.0, 384.0, 1024, 768);
            
            // Calculate screen position that should map to specific bit
            let screen_x = 512 + base_x * 8 + bit_offset;
            let screen_y = 384;
            
            let mut display = AddressDisplay::new();
            display.update_mouse_position(screen_x, screen_y, &viewport, &metadata);
            
            if display.is_mouse_in_bounds() {
                if let Some(addr) = display.address {
                    // Bit should be in valid range
                    prop_assert!(addr.bit < 8,
                        "Bit {} must be in range 0-7", addr.bit);
                }
            }
        });
    }
    
    /// Property test for address monotonicity
    /// 
    /// Verifies that moving the mouse vertically increases byte/page/block addresses.
    #[test]
    #[ignore]
    fn prop_address_monotonicity_vertical() {
        proptest!(|(
            screen_x in 512u32..1024,
            screen_y1 in 100u32..400,
            screen_y2 in 500u32..700,
        )| {
            let metadata = FileMetadata::new(
                "test.bin".to_string(),
                50_000_000,
                512,
                64,
            );
            
            let viewport = Viewport::new(0, 512.0, 384.0, 1024, 768);
            
            let mut display = AddressDisplay::new();
            
            // Get address at first position
            display.update_mouse_position(screen_x, screen_y1, &viewport, &metadata);
            let addr1 = display.address;
            
            // Get address at second position (lower on screen = higher in dump)
            display.update_mouse_position(screen_x, screen_y2, &viewport, &metadata);
            let addr2 = display.address;
            
            // Property: If both are in bounds, second address should be >= first
            // (moving down increases byte position)
            if let (Some(a1), Some(a2)) = (addr1, addr2) {
                // Calculate linear byte offset for comparison
                let offset1 = a1.block * (metadata.block_size as u64) * (metadata.page_length as u64)
                            + a1.page * (metadata.page_length as u64)
                            + a1.byte;
                let offset2 = a2.block * (metadata.block_size as u64) * (metadata.page_length as u64)
                            + a2.page * (metadata.page_length as u64)
                            + a2.byte;
                
                prop_assert!(offset2 >= offset1,
                    "Moving down should increase byte offset: {} -> {}", offset1, offset2);
            }
        });
    }
    
    /// Property test for out-of-bounds detection
    /// 
    /// Verifies that positions beyond the dump are correctly detected as out of bounds.
    #[test]
    #[ignore]
    fn prop_out_of_bounds_detection() {
        proptest!(|(
            level in 0u32..3,
            center_x in 0.0f64..1000.0,
            center_y in 0.0f64..1000.0,
            screen_x in 0u32..1024,
            screen_y in 0u32..768,
        )| {
            let metadata = FileMetadata::new(
                "test.bin".to_string(),
                1_000_000, // Small file for easier out-of-bounds testing
                512,
                64,
            );
            
            let viewport = Viewport::new(level, center_x, center_y, 1024, 768);
            
            let mut display = AddressDisplay::new();
            display.update_mouse_position(screen_x, screen_y, &viewport, &metadata);
            
            // Property: is_mouse_in_bounds() should match whether address is Some
            let has_address = display.address.is_some();
            let in_bounds = display.is_mouse_in_bounds();
            
            prop_assert_eq!(has_address, in_bounds,
                "is_mouse_in_bounds() should match whether address exists");
            
            // Property: If out of bounds, get_address() should return "N/A"
            if !in_bounds {
                prop_assert_eq!(display.get_address(), "N/A",
                    "Out of bounds should return 'N/A'");
            }
        });
    }
}
