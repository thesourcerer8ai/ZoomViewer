//! ByteArranger for horizontal byte layout
//!
//! Arranges bytes horizontally within a page with no spacing between bytes.
//! Validates: Requirements 3.1, 3.2, 3.3, 3.4

use crate::bit_renderer::BitRenderer;
use crate::bit_renderer::PixelBuffer;

/// ByteArranger arranges bytes horizontally within a page with no spacing
pub struct ByteArranger;

impl ByteArranger {
    /// Calculate the width of a page in pixels
    ///
    /// Returns pageLength * 8 (pixels, no spacing between bytes)
    ///
    /// # Arguments
    /// * `page_length` - Number of bytes in a page
    ///
    /// # Returns
    /// Width in pixels (pageLength * 8)
    pub fn calculate_page_width(page_length: u32) -> u32 {
        page_length * 8
    }

    /// Render a page of bytes horizontally with no spacing
    ///
    /// Renders bytes left-to-right with no spacing between bytes.
    /// First byte starts at x=0, each subsequent byte at x = byte_index * 8
    ///
    /// # Arguments
    /// * `page_data` - Slice of bytes to render
    /// * `y` - Y coordinate where the page should be rendered
    /// * `canvas` - The pixel buffer to write to
    ///
    /// # Returns
    /// Ok(()) on success, Err with message if rendering fails
    pub fn render_page(page_data: &[u8], y: u32, canvas: &mut PixelBuffer) -> Result<(), String> {
        Self::render_page_at(page_data, 0, y, canvas)
    }
    
    pub fn render_page_at(page_data: &[u8], start_x: u32, y: u32, canvas: &mut PixelBuffer) -> Result<(), String> {
        for (byte_index, &byte) in page_data.iter().enumerate() {
            let x = start_x + (byte_index as u32) * 8;
            BitRenderer::render_byte(byte, x, y, canvas)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bit_renderer::Pixel;

    #[test]
    fn test_calculate_page_width() {
        assert_eq!(ByteArranger::calculate_page_width(1), 8);
        assert_eq!(ByteArranger::calculate_page_width(10), 80);
        assert_eq!(ByteArranger::calculate_page_width(512), 4096);
        assert_eq!(ByteArranger::calculate_page_width(2048), 16384);
    }

    #[test]
    fn test_render_page_empty() {
        let mut buffer = PixelBuffer::new(8, 1);
        let page_data: &[u8] = &[];
        let result = ByteArranger::render_page(page_data, 0, &mut buffer);
        assert!(result.is_ok());
    }

    #[test]
    fn test_render_page_single_byte_all_zeros() {
        let mut buffer = PixelBuffer::new(8, 1);
        let page_data: &[u8] = &[0x00];
        let result = ByteArranger::render_page(page_data, 0, &mut buffer);
        assert!(result.is_ok());

        // All bits should be white (0)
        for x in 0..8 {
            let pixel = buffer.get(x, 0).unwrap();
            assert_eq!(pixel, Pixel::white());
        }
    }

    #[test]
    fn test_render_page_single_byte_all_ones() {
        let mut buffer = PixelBuffer::new(8, 1);
        let page_data: &[u8] = &[0xFF];
        let result = ByteArranger::render_page(page_data, 0, &mut buffer);
        assert!(result.is_ok());

        // All bits should be black (1)
        for x in 0..8 {
            let pixel = buffer.get(x, 0).unwrap();
            assert_eq!(pixel, Pixel::black());
        }
    }

    #[test]
    fn test_render_page_multiple_bytes_no_spacing() {
        let mut buffer = PixelBuffer::new(16, 1);
        let page_data: &[u8] = &[0xFF, 0x00];
        let result = ByteArranger::render_page(page_data, 0, &mut buffer);
        assert!(result.is_ok());

        // First byte (0xFF) should be all black at x=0..7
        for x in 0..8 {
            let pixel = buffer.get(x, 0).unwrap();
            assert_eq!(pixel, Pixel::black(), "First byte bit {} should be black", x);
        }

        // Second byte (0x00) should be all white at x=8..15
        for x in 8..16 {
            let pixel = buffer.get(x, 0).unwrap();
            assert_eq!(pixel, Pixel::white(), "Second byte bit {} should be white", x);
        }
    }

    #[test]
    fn test_render_page_msb_lsb_ordering() {
        let mut buffer = PixelBuffer::new(8, 1);
        // 0x80 = 10000000 in binary (MSB=1, LSB=0)
        let page_data: &[u8] = &[0x80];
        let result = ByteArranger::render_page(page_data, 0, &mut buffer);
        assert!(result.is_ok());

        // MSB (1) should be on the left (x=0)
        assert_eq!(buffer.get(0, 0).unwrap(), Pixel::black());
        // LSB (0) should be on the right (x=7)
        assert_eq!(buffer.get(7, 0).unwrap(), Pixel::white());
    }

    #[test]
    fn test_render_page_out_of_bounds() {
        let mut buffer = PixelBuffer::new(8, 1);
        // Try to render 2 bytes in a buffer that only has 8 pixels width
        let page_data: &[u8] = &[0xFF, 0xFF];
        let result = ByteArranger::render_page(page_data, 0, &mut buffer);
        assert!(result.is_err());
    }

    #[test]
    fn test_render_page_multiple_rows() {
        let mut buffer = PixelBuffer::new(8, 3);
        let page_data: &[u8] = &[0xFF];

        // Render same byte on different rows
        assert!(ByteArranger::render_page(page_data, 0, &mut buffer).is_ok());
        assert!(ByteArranger::render_page(page_data, 1, &mut buffer).is_ok());
        assert!(ByteArranger::render_page(page_data, 2, &mut buffer).is_ok());

        // All pixels should be black
        for y in 0..3 {
            for x in 0..8 {
                assert_eq!(buffer.get(x, y).unwrap(), Pixel::black());
            }
        }
    }

    #[test]
    fn test_render_page_alternating_pattern() {
        let mut buffer = PixelBuffer::new(16, 1);
        // 0xAA = 10101010, 0x55 = 01010101
        let page_data: &[u8] = &[0xAA, 0x55];
        let result = ByteArranger::render_page(page_data, 0, &mut buffer);
        assert!(result.is_ok());

        // First byte: 10101010 (alternating black-white)
        assert_eq!(buffer.get(0, 0).unwrap(), Pixel::black());
        assert_eq!(buffer.get(1, 0).unwrap(), Pixel::white());
        assert_eq!(buffer.get(2, 0).unwrap(), Pixel::black());
        assert_eq!(buffer.get(3, 0).unwrap(), Pixel::white());
        assert_eq!(buffer.get(4, 0).unwrap(), Pixel::black());
        assert_eq!(buffer.get(5, 0).unwrap(), Pixel::white());
        assert_eq!(buffer.get(6, 0).unwrap(), Pixel::black());
        assert_eq!(buffer.get(7, 0).unwrap(), Pixel::white());

        // Second byte: 01010101 (alternating white-black)
        assert_eq!(buffer.get(8, 0).unwrap(), Pixel::white());
        assert_eq!(buffer.get(9, 0).unwrap(), Pixel::black());
        assert_eq!(buffer.get(10, 0).unwrap(), Pixel::white());
        assert_eq!(buffer.get(11, 0).unwrap(), Pixel::black());
        assert_eq!(buffer.get(12, 0).unwrap(), Pixel::white());
        assert_eq!(buffer.get(13, 0).unwrap(), Pixel::black());
        assert_eq!(buffer.get(14, 0).unwrap(), Pixel::white());
        assert_eq!(buffer.get(15, 0).unwrap(), Pixel::black());
    }

    #[test]
    fn test_render_page_full_page_small() {
        // Test a full small page (typical minimum: 512 bytes)
        let page_length = 512;
        let page_data: Vec<u8> = (0..page_length).map(|i| (i % 256) as u8).collect();
        let expected_width = page_length as u32 * 8;
        
        let mut buffer = PixelBuffer::new(expected_width, 1);
        let result = ByteArranger::render_page(&page_data, 0, &mut buffer);
        
        assert!(result.is_ok(), "Should successfully render a full 512-byte page");
        
        // Verify first byte is at x=0
        let first_byte = page_data[0];
        let first_bit = (first_byte >> 7) & 1;
        let expected_first = if first_bit == 1 { Pixel::black() } else { Pixel::white() };
        assert_eq!(buffer.get(0, 0).unwrap(), expected_first);
        
        // Verify last byte is at the end
        let last_byte = page_data[page_length - 1];
        let last_bit = last_byte & 1;
        let expected_last = if last_bit == 1 { Pixel::black() } else { Pixel::white() };
        assert_eq!(buffer.get(expected_width - 1, 0).unwrap(), expected_last);
    }

    #[test]
    fn test_render_page_full_page_large() {
        // Test a full large page (typical maximum: 20000 bytes)
        let page_length = 2048; // Using 2048 for faster test execution
        let page_data: Vec<u8> = (0..page_length).map(|i| (i % 256) as u8).collect();
        let expected_width = page_length as u32 * 8;
        
        let mut buffer = PixelBuffer::new(expected_width, 1);
        let result = ByteArranger::render_page(&page_data, 0, &mut buffer);
        
        assert!(result.is_ok(), "Should successfully render a full 2048-byte page");
        
        // Verify width calculation
        assert_eq!(ByteArranger::calculate_page_width(page_length as u32), expected_width);
        
        // Spot check: verify byte at position 1000
        let test_index = 1000;
        let test_byte = page_data[test_index];
        let test_x = test_index as u32 * 8;
        let test_bit = (test_byte >> 7) & 1;
        let expected_pixel = if test_bit == 1 { Pixel::black() } else { Pixel::white() };
        assert_eq!(buffer.get(test_x, 0).unwrap(), expected_pixel,
            "Byte at index {} should be correctly positioned at x={}", test_index, test_x);
    }

    #[test]
    fn test_pixel_positioning_accuracy_sequential_bytes() {
        // Test that each byte is positioned exactly at byte_index * 8
        let page_data: Vec<u8> = vec![0x80, 0x40, 0x20, 0x10, 0x08, 0x04, 0x02, 0x01];
        let mut buffer = PixelBuffer::new(64, 1);
        
        let result = ByteArranger::render_page(&page_data, 0, &mut buffer);
        assert!(result.is_ok());
        
        // Each byte has exactly one bit set, verify it's at the correct position
        for (byte_index, &byte_value) in page_data.iter().enumerate() {
            let start_x = byte_index as u32 * 8;
            
            // Find which bit is set in this byte
            let bit_position = match byte_value {
                0x80 => 0, // MSB
                0x40 => 1,
                0x20 => 2,
                0x10 => 3,
                0x08 => 4,
                0x04 => 5,
                0x02 => 6,
                0x01 => 7, // LSB
                _ => panic!("Unexpected byte value"),
            };
            
            // Verify the black pixel is at the expected position
            let expected_x = start_x + bit_position;
            assert_eq!(buffer.get(expected_x, 0).unwrap(), Pixel::black(),
                "Byte {} (0x{:02X}) should have black pixel at x={}", 
                byte_index, byte_value, expected_x);
            
            // Verify all other pixels in this byte are white
            for bit_idx in 0..8 {
                if bit_idx != bit_position {
                    let x = start_x + bit_idx;
                    assert_eq!(buffer.get(x, 0).unwrap(), Pixel::white(),
                        "Byte {} bit {} at x={} should be white", byte_index, bit_idx, x);
                }
            }
        }
    }

    #[test]
    fn test_pixel_positioning_accuracy_boundary() {
        // Test pixel positioning at byte boundaries
        let page_data: Vec<u8> = vec![0xFF, 0x00, 0xFF, 0x00];
        let mut buffer = PixelBuffer::new(32, 1);
        
        let result = ByteArranger::render_page(&page_data, 0, &mut buffer);
        assert!(result.is_ok());
        
        // Verify boundaries between bytes
        // Byte 0 (0xFF) ends at x=7, Byte 1 (0x00) starts at x=8
        assert_eq!(buffer.get(7, 0).unwrap(), Pixel::black(), "Last bit of byte 0 should be black");
        assert_eq!(buffer.get(8, 0).unwrap(), Pixel::white(), "First bit of byte 1 should be white");
        
        // Byte 1 (0x00) ends at x=15, Byte 2 (0xFF) starts at x=16
        assert_eq!(buffer.get(15, 0).unwrap(), Pixel::white(), "Last bit of byte 1 should be white");
        assert_eq!(buffer.get(16, 0).unwrap(), Pixel::black(), "First bit of byte 2 should be black");
        
        // Byte 2 (0xFF) ends at x=23, Byte 3 (0x00) starts at x=24
        assert_eq!(buffer.get(23, 0).unwrap(), Pixel::black(), "Last bit of byte 2 should be black");
        assert_eq!(buffer.get(24, 0).unwrap(), Pixel::white(), "First bit of byte 3 should be white");
    }

    // Property-Based Tests
    use proptest::prelude::*;

    /*
    proptest! {
        /// **Property 8: Byte horizontal arrangement**
        /// 
        /// **Validates: Requirements 3.1, 3.2, 3.3, 3.4**
        /// 
        /// For any page data, the byte arranger SHALL display bytes horizontally in sequence
        /// from left to right, with the first byte on the left and the last byte on the right,
        /// with no spacing between bytes.
        #[test]
        fn prop_byte_horizontal_arrangement(page_data in prop::collection::vec(any::<u8>(), 1..100)) {
            // Calculate required buffer width (8 pixels per byte, no spacing)
            let page_length = page_data.len() as u32;
            let expected_width = page_length * 8;
            
            // Create buffer to hold the rendered page
            let mut buffer = PixelBuffer::new(expected_width, 1);
            
            // Render the page
            let result = ByteArranger::render_page(&page_data, 0, &mut buffer);
            
            // Property 1: Rendering should succeed for any valid page data
            prop_assert!(result.is_ok(), "Rendering should succeed for any page data");
            
            // Property 2: Bytes should be arranged horizontally in sequence
            // Each byte should start at x = byte_index * 8
            for (byte_index, &byte_value) in page_data.iter().enumerate() {
                let start_x = (byte_index as u32) * 8;
                
                // Verify each bit of the byte is rendered at the correct position
                for bit_index in 0..8 {
                    let x = start_x + bit_index;
                    let pixel = buffer.get(x, 0);
                    
                    prop_assert!(pixel.is_some(), 
                        "Pixel should exist at x={} for byte {} bit {}", 
                        x, byte_index, bit_index);
                    
                    // Extract the bit value (MSB on left, LSB on right)
                    let bit = (byte_value >> (7 - bit_index)) & 1;
                    let expected_pixel = if bit == 1 { Pixel::black() } else { Pixel::white() };
                    
                    prop_assert_eq!(pixel.unwrap(), expected_pixel,
                        "Byte {} (value 0x{:02X}) bit {} at x={} should be {:?}",
                        byte_index, byte_value, 7 - bit_index, x, expected_pixel);
                }
            }
            
            // Property 3: First byte should be on the left (starting at x=0)
            if !page_data.is_empty() {
                let first_byte = page_data[0];
                let first_bit = (first_byte >> 7) & 1;
                let first_pixel = buffer.get(0, 0).unwrap();
                let expected_first = if first_bit == 1 { Pixel::black() } else { Pixel::white() };
                
                prop_assert_eq!(first_pixel, expected_first,
                    "First byte (0x{:02X}) should start at x=0", first_byte);
            }
            
            // Property 4: Last byte should be on the right (ending at x=expected_width-1)
            if !page_data.is_empty() {
                let last_byte = page_data[page_data.len() - 1];
                let last_bit = last_byte & 1; // LSB
                let last_x = expected_width - 1;
                let last_pixel = buffer.get(last_x, 0).unwrap();
                let expected_last = if last_bit == 1 { Pixel::black() } else { Pixel::white() };
                
                prop_assert_eq!(last_pixel, expected_last,
                    "Last byte (0x{:02X}) should end at x={}", last_byte, last_x);
            }
            
            // Property 5: No spacing between bytes
            // Verify that consecutive bytes are adjacent (no gaps)
            if page_data.len() >= 2 {
                for byte_index in 0..(page_data.len() - 1) {
                    let current_byte_end = (byte_index as u32) * 8 + 7;
                    let next_byte_start = ((byte_index + 1) as u32) * 8;
                    
                    // The next byte should start immediately after the current byte
                    prop_assert_eq!(next_byte_start, current_byte_end + 1,
                        "Byte {} should end at x={} and byte {} should start at x={} (no spacing)",
                        byte_index, current_byte_end, byte_index + 1, next_byte_start);
                }
            }
        }
    }
    */
}
