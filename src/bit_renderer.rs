//! BitRenderer for converting bits to pixels
//!
//! Converts individual bits to pixels with the following mapping:
//! - bit 1 → black pixel (0x000000)
//! - bit 0 → white pixel (0xFFFFFF)
//!
//! Bits are rendered horizontally with MSB on the left and LSB on the right.

/// A single pixel represented as RGB values
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pixel {
    /// Red component (0-255)
    pub r: u8,
    /// Green component (0-255)
    pub g: u8,
    /// Blue component (0-255)
    pub b: u8,
}

impl Pixel {
    /// Create a new pixel with RGB values
    pub fn new(r: u8, g: u8, b: u8) -> Self {
        Pixel { r, g, b }
    }

    /// Create a black pixel (0x000000)
    pub fn black() -> Self {
        Pixel { r: 0, g: 0, b: 0 }
    }

    /// Create a white pixel (0xFFFFFF)
    pub fn white() -> Self {
        Pixel { r: 255, g: 255, b: 255 }
    }
}

/// A 2D buffer of pixels
#[derive(Debug, Clone)]
pub struct PixelBuffer {
    /// Pixel data stored in row-major order (y * width + x)
    data: Vec<Pixel>,
    /// Width in pixels
    width: u32,
    /// Height in pixels
    height: u32,
}

impl PixelBuffer {
    /// Create a new pixel buffer with the given dimensions
    pub fn new(width: u32, height: u32) -> Self {
        let size = (width as usize) * (height as usize);
        PixelBuffer {
            data: vec![Pixel::white(); size],
            width,
            height,
        }
    }

    /// Create a new pixel buffer filled with a specific pixel
    pub fn with_fill(width: u32, height: u32, fill: Pixel) -> Self {
        let size = (width as usize) * (height as usize);
        PixelBuffer {
            data: vec![fill; size],
            width,
            height,
        }
    }

    /// Get the width of the buffer
    pub fn width(&self) -> u32 {
        self.width
    }

    /// Get the height of the buffer
    pub fn height(&self) -> u32 {
        self.height
    }

    /// Get a pixel at the given coordinates
    pub fn get(&self, x: u32, y: u32) -> Option<Pixel> {
        if x >= self.width || y >= self.height {
            return None;
        }
        let index = (y as usize) * (self.width as usize) + (x as usize);
        Some(self.data[index])
    }

    /// Set a pixel at the given coordinates
    pub fn set(&mut self, x: u32, y: u32, pixel: Pixel) -> Result<(), String> {
        if x >= self.width || y >= self.height {
            return Err(format!(
                "Pixel coordinates ({}, {}) out of bounds for buffer {}x{}",
                x, y, self.width, self.height
            ));
        }
        let index = (y as usize) * (self.width as usize) + (x as usize);
        self.data[index] = pixel;
        Ok(())
    }

    /// Get mutable access to the underlying pixel data
    pub fn data_mut(&mut self) -> &mut [Pixel] {
        &mut self.data
    }

    /// Get immutable access to the underlying pixel data
    pub fn data(&self) -> &[Pixel] {
        &self.data
    }
}

/// BitRenderer converts bits to pixels
pub struct BitRenderer;

impl BitRenderer {
    /// Render a single bit as a pixel
    ///
    /// - bit 1 → black pixel (0x000000)
    /// - bit 0 → white pixel (0xFFFFFF)
    pub fn render_bit(bit: u8) -> Pixel {
        match bit {
            0 => Pixel::white(),
            _ => Pixel::black(),
        }
    }

    /// Render a byte as 8 pixels horizontally
    ///
    /// Bits are arranged from MSB (left) to LSB (right).
    /// Each bit becomes one pixel.
    ///
    /// # Arguments
    /// * `byte` - The byte to render
    /// * `start_x` - Starting X coordinate in the buffer
    /// * `y` - Y coordinate in the buffer
    /// * `buffer` - The pixel buffer to write to
    ///
    /// # Returns
    /// Ok(()) on success, Err with message if coordinates are out of bounds
    pub fn render_byte(byte: u8, start_x: u32, y: u32, buffer: &mut PixelBuffer) -> Result<(), String> {
        // Render bits from MSB (bit 7) to LSB (bit 0)
        // MSB is on the left (start_x), LSB is on the right (start_x + 7)
        for bit_index in 0..8 {
            // Extract bit from MSB to LSB
            let bit = (byte >> (7 - bit_index)) & 1;
            let pixel = Self::render_bit(bit);
            let x = start_x + bit_index as u32;
            buffer.set(x, y, pixel)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn test_pixel_creation() {
        let pixel = Pixel::new(255, 128, 64);
        assert_eq!(pixel.r, 255);
        assert_eq!(pixel.g, 128);
        assert_eq!(pixel.b, 64);
    }

    #[test]
    fn test_pixel_black() {
        let black = Pixel::black();
        assert_eq!(black.r, 0);
        assert_eq!(black.g, 0);
        assert_eq!(black.b, 0);
    }

    #[test]
    fn test_pixel_white() {
        let white = Pixel::white();
        assert_eq!(white.r, 255);
        assert_eq!(white.g, 255);
        assert_eq!(white.b, 255);
    }

    #[test]
    fn test_pixel_buffer_creation() {
        let buffer = PixelBuffer::new(10, 20);
        assert_eq!(buffer.width(), 10);
        assert_eq!(buffer.height(), 20);
        // All pixels should be white by default
        assert_eq!(buffer.get(0, 0), Some(Pixel::white()));
        assert_eq!(buffer.get(9, 19), Some(Pixel::white()));
    }

    #[test]
    fn test_pixel_buffer_with_fill() {
        let buffer = PixelBuffer::with_fill(5, 5, Pixel::black());
        assert_eq!(buffer.get(0, 0), Some(Pixel::black()));
        assert_eq!(buffer.get(4, 4), Some(Pixel::black()));
    }

    #[test]
    fn test_pixel_buffer_set_and_get() {
        let mut buffer = PixelBuffer::new(10, 10);
        let pixel = Pixel::new(100, 150, 200);
        buffer.set(5, 5, pixel).unwrap();
        assert_eq!(buffer.get(5, 5), Some(pixel));
    }

    #[test]
    fn test_pixel_buffer_out_of_bounds() {
        let mut buffer = PixelBuffer::new(10, 10);
        let pixel = Pixel::black();
        assert!(buffer.set(10, 5, pixel).is_err());
        assert!(buffer.set(5, 10, pixel).is_err());
        assert!(buffer.get(10, 5).is_none());
        assert!(buffer.get(5, 10).is_none());
    }

    #[test]
    fn test_render_bit_zero() {
        let pixel = BitRenderer::render_bit(0);
        assert_eq!(pixel, Pixel::white());
    }

    #[test]
    fn test_render_bit_one() {
        let pixel = BitRenderer::render_bit(1);
        assert_eq!(pixel, Pixel::black());
    }

    #[test]
    fn test_render_byte_all_zeros() {
        let mut buffer = PixelBuffer::new(8, 1);
        BitRenderer::render_byte(0x00, 0, 0, &mut buffer).unwrap();
        
        // All pixels should be white
        for x in 0..8 {
            assert_eq!(buffer.get(x, 0), Some(Pixel::white()));
        }
    }

    #[test]
    fn test_render_byte_all_ones() {
        let mut buffer = PixelBuffer::new(8, 1);
        BitRenderer::render_byte(0xFF, 0, 0, &mut buffer).unwrap();
        
        // All pixels should be black
        for x in 0..8 {
            assert_eq!(buffer.get(x, 0), Some(Pixel::black()));
        }
    }

    #[test]
    fn test_render_byte_alternating_pattern() {
        let mut buffer = PixelBuffer::new(8, 1);
        // 0xAA = 10101010 in binary
        BitRenderer::render_byte(0xAA, 0, 0, &mut buffer).unwrap();
        
        // Pattern should be: black, white, black, white, black, white, black, white
        assert_eq!(buffer.get(0, 0), Some(Pixel::black()));  // MSB = 1
        assert_eq!(buffer.get(1, 0), Some(Pixel::white())); // 0
        assert_eq!(buffer.get(2, 0), Some(Pixel::black()));  // 1
        assert_eq!(buffer.get(3, 0), Some(Pixel::white())); // 0
        assert_eq!(buffer.get(4, 0), Some(Pixel::black()));  // 1
        assert_eq!(buffer.get(5, 0), Some(Pixel::white())); // 0
        assert_eq!(buffer.get(6, 0), Some(Pixel::black()));  // 1
        assert_eq!(buffer.get(7, 0), Some(Pixel::white())); // LSB = 0
    }

    #[test]
    fn test_render_byte_reverse_pattern() {
        let mut buffer = PixelBuffer::new(8, 1);
        // 0x55 = 01010101 in binary
        BitRenderer::render_byte(0x55, 0, 0, &mut buffer).unwrap();
        
        // Pattern should be: white, black, white, black, white, black, white, black
        assert_eq!(buffer.get(0, 0), Some(Pixel::white())); // MSB = 0
        assert_eq!(buffer.get(1, 0), Some(Pixel::black()));  // 1
        assert_eq!(buffer.get(2, 0), Some(Pixel::white())); // 0
        assert_eq!(buffer.get(3, 0), Some(Pixel::black()));  // 1
        assert_eq!(buffer.get(4, 0), Some(Pixel::white())); // 0
        assert_eq!(buffer.get(5, 0), Some(Pixel::black()));  // 1
        assert_eq!(buffer.get(6, 0), Some(Pixel::white())); // 0
        assert_eq!(buffer.get(7, 0), Some(Pixel::black()));  // LSB = 1
    }

    #[test]
    fn test_render_byte_msb_only() {
        let mut buffer = PixelBuffer::new(8, 1);
        // 0x80 = 10000000 in binary (only MSB set)
        BitRenderer::render_byte(0x80, 0, 0, &mut buffer).unwrap();
        
        assert_eq!(buffer.get(0, 0), Some(Pixel::black()));  // MSB = 1
        for x in 1..8 {
            assert_eq!(buffer.get(x, 0), Some(Pixel::white())); // Rest = 0
        }
    }

    #[test]
    fn test_render_byte_lsb_only() {
        let mut buffer = PixelBuffer::new(8, 1);
        // 0x01 = 00000001 in binary (only LSB set)
        BitRenderer::render_byte(0x01, 0, 0, &mut buffer).unwrap();
        
        for x in 0..7 {
            assert_eq!(buffer.get(x, 0), Some(Pixel::white())); // First 7 = 0
        }
        assert_eq!(buffer.get(7, 0), Some(Pixel::black()));  // LSB = 1
    }

    #[test]
    fn test_render_byte_with_offset() {
        let mut buffer = PixelBuffer::new(16, 1);
        // Render 0xFF at offset 4
        BitRenderer::render_byte(0xFF, 4, 0, &mut buffer).unwrap();
        
        // First 4 pixels should be white (default)
        for x in 0..4 {
            assert_eq!(buffer.get(x, 0), Some(Pixel::white()));
        }
        // Next 8 pixels should be black
        for x in 4..12 {
            assert_eq!(buffer.get(x, 0), Some(Pixel::black()));
        }
        // Last 4 pixels should be white (default)
        for x in 12..16 {
            assert_eq!(buffer.get(x, 0), Some(Pixel::white()));
        }
    }

    #[test]
    fn test_render_byte_out_of_bounds() {
        let mut buffer = PixelBuffer::new(8, 1);
        // Try to render at x=5, which would go to x=12 (out of bounds)
        let result = BitRenderer::render_byte(0xFF, 5, 0, &mut buffer);
        assert!(result.is_err());
    }

    #[test]
    fn test_render_byte_multiple_rows() {
        let mut buffer = PixelBuffer::new(8, 3);
        
        // Render different bytes on different rows
        BitRenderer::render_byte(0xFF, 0, 0, &mut buffer).unwrap();
        BitRenderer::render_byte(0x00, 0, 1, &mut buffer).unwrap();
        BitRenderer::render_byte(0xAA, 0, 2, &mut buffer).unwrap();
        
        // Row 0: all black
        for x in 0..8 {
            assert_eq!(buffer.get(x, 0), Some(Pixel::black()));
        }
        
        // Row 1: all white
        for x in 0..8 {
            assert_eq!(buffer.get(x, 1), Some(Pixel::white()));
        }
        
        // Row 2: alternating pattern
        assert_eq!(buffer.get(0, 2), Some(Pixel::black()));
        assert_eq!(buffer.get(1, 2), Some(Pixel::white()));
    }

    // Property-Based Tests

    /*
    proptest! {
        /// **Property 7: Bit-to-pixel rendering**
        /// 
        /// **Validates: Requirements 2.1, 2.2, 2.3, 2.4, 2.5**
        /// 
        /// For any byte value, the bit renderer SHALL produce exactly 8 pixels (one per bit),
        /// with bit value 1 rendered as black and bit value 0 as white, in LSB-first order
        /// (MSB on left, LSB on right).
        #[test]
        fn prop_bit_to_pixel_rendering(byte: u8) {
            // Create a buffer large enough to hold 8 pixels
            let mut buffer = PixelBuffer::new(8, 1);
            
            // Render the byte
            let result = BitRenderer::render_byte(byte, 0, 0, &mut buffer);
            
            // Property 1: Rendering should succeed
            prop_assert!(result.is_ok(), "Rendering should succeed for any byte value");
            
            // Property 2: Exactly 8 pixels should be produced
            // (verified by checking all 8 positions)
            for bit_index in 0..8 {
                let pixel = buffer.get(bit_index, 0);
                prop_assert!(pixel.is_some(), "Pixel at position {} should exist", bit_index);
            }
            
            // Property 3: Each bit should be rendered correctly
            // MSB (bit 7) is on the left (x=0), LSB (bit 0) is on the right (x=7)
            for bit_index in 0..8 {
                // Extract bit from MSB to LSB
                let bit = (byte >> (7 - bit_index)) & 1;
                let pixel = buffer.get(bit_index as u32, 0).unwrap();
                
                // Property 4: bit value 1 → black pixel
                // Property 5: bit value 0 → white pixel
                if bit == 1 {
                    prop_assert_eq!(pixel, Pixel::black(), 
                        "Bit {} (value 1) at position {} should be black", 
                        7 - bit_index, bit_index);
                } else {
                    prop_assert_eq!(pixel, Pixel::white(), 
                        "Bit {} (value 0) at position {} should be white", 
                        7 - bit_index, bit_index);
                }
            }
            
            // Property 6: Verify MSB-to-LSB ordering (MSB on left, LSB on right)
            let msb = (byte >> 7) & 1;
            let lsb = byte & 1;
            
            let leftmost_pixel = buffer.get(0, 0).unwrap();
            let rightmost_pixel = buffer.get(7, 0).unwrap();
            
            if msb == 1 {
                prop_assert_eq!(leftmost_pixel, Pixel::black(), 
                    "MSB (bit 7) should be on the left (x=0)");
            } else {
                prop_assert_eq!(leftmost_pixel, Pixel::white(), 
                    "MSB (bit 7) should be on the left (x=0)");
            }
            
            if lsb == 1 {
                prop_assert_eq!(rightmost_pixel, Pixel::black(), 
                    "LSB (bit 0) should be on the right (x=7)");
            } else {
                prop_assert_eq!(rightmost_pixel, Pixel::white(), 
                    "LSB (bit 0) should be on the right (x=7)");
            }
        }
    }
    */
}
