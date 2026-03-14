//! Viewport renderer - composites tiles into the viewport for display

use crate::{CacheManager, TileCoord, Viewport};
use image::RgbaImage;
use std::sync::{Arc, Mutex};
use std::collections::HashMap;

/// Renders tiles from cache into a viewport image
pub struct ViewportRenderer {
    /// Cache manager for loading tiles
    cache: Arc<CacheManager>,
    /// Tile width in pixels
    tile_width: u32,
    /// Tile height in pixels
    tile_height: u32,
    /// Cache of decoded tile images (TileCoord -> RgbaImage)
    decoded_tile_cache: Arc<Mutex<HashMap<TileCoord, Arc<RgbaImage>>>>,
}

impl ViewportRenderer {
    /// Create a new viewport renderer
    pub fn new(cache: Arc<CacheManager>, tile_width: u32, tile_height: u32) -> Self {
        ViewportRenderer {
            cache,
            tile_width,
            tile_height,
            decoded_tile_cache: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Render viewport to an RGBA image
    /// Returns an image with missing tiles shown as gray placeholders
    pub fn render_viewport(&self, viewport: &Viewport, _blend_level: Option<u32>, _blend_factor: f64) -> RgbaImage {
        let mut image = RgbaImage::new(viewport.width_pixels, viewport.height_pixels);

        // Fill with light gray background
        for pixel in image.pixels_mut() {
            *pixel = image::Rgba([200, 200, 200, 255]);
        }

        // Render visible tiles
        for tile_coord in &viewport.visible_tiles {
            self.render_tile(&mut image, viewport, *tile_coord);
        }

        // Render adjacent tiles
        for tile_coord in &viewport.adjacent_tiles {
            self.render_tile(&mut image, viewport, *tile_coord);
        }

        image
    }

    /// Render a single tile into the viewport image
    fn render_tile(&self, image: &mut RgbaImage, viewport: &Viewport, coord: TileCoord) {
        // Calculate tile position in level coordinate space
        let tile_x_pixels = (coord.x as f64) * (self.tile_width as f64);
        let tile_y_pixels = (coord.y as f64) * (self.tile_height as f64);

        // Calculate viewport bounds in level coordinate space
        let viewport_left = viewport.center_x - (viewport.width_pixels as f64) / 2.0;
        let viewport_top = viewport.center_y - (viewport.height_pixels as f64) / 2.0;
        let viewport_right = viewport_left + (viewport.width_pixels as f64);
        let viewport_bottom = viewport_top + (viewport.height_pixels as f64);

        // Calculate tile bounds in level coordinate space
        let tile_right = tile_x_pixels + (self.tile_width as f64);
        let tile_bottom = tile_y_pixels + (self.tile_height as f64);

        // Check if tile is visible
        if tile_right < viewport_left
            || tile_x_pixels > viewport_right
            || tile_bottom < viewport_top
            || tile_y_pixels > viewport_bottom
        {
            return; // Tile is outside viewport
        }

        // Calculate intersection of tile and viewport
        let intersect_left = tile_x_pixels.max(viewport_left);
        let intersect_top = tile_y_pixels.max(viewport_top);
        let intersect_right = tile_right.min(viewport_right);
        let intersect_bottom = tile_bottom.min(viewport_bottom);

        // Convert to screen coordinates
        let screen_x = ((intersect_left - viewport_left) as u32).min(viewport.width_pixels);
        let screen_y = ((intersect_top - viewport_top) as u32).min(viewport.height_pixels);
        let screen_width = ((intersect_right - intersect_left) as u32)
            .min(viewport.width_pixels - screen_x);
        let screen_height = ((intersect_bottom - intersect_top) as u32)
            .min(viewport.height_pixels - screen_y);

        // Try to get decoded tile from cache first
        let tile_rgba = {
            let cache = self.decoded_tile_cache.lock().unwrap();
            cache.get(&coord).cloned()
        };

        let tile_rgba = if let Some(cached_tile) = tile_rgba {
            cached_tile
        } else {
            // Load and decode tile
            match self.cache.load_tile(&coord) {
                Ok(tile_data) => {
                    // Decode QOI/PNG
                    match image::load_from_memory(&tile_data) {
                        Ok(tile_img) => {
                            let decoded = Arc::new(tile_img.to_rgba8());
                            
                            // Cache the decoded tile
                            {
                                let mut cache = self.decoded_tile_cache.lock().unwrap();
                                cache.insert(coord, decoded.clone());
                            }
                            
                            decoded
                        }
                        Err(_) => {
                            // Failed to decode, show placeholder
                            self.draw_placeholder(image, screen_x, screen_y, screen_width, screen_height, coord);
                            return;
                        }
                    }
                }
                Err(_) => {
                    // Tile not in cache, show placeholder
                    self.draw_placeholder(image, screen_x, screen_y, screen_width, screen_height, coord);
                    return;
                }
            }
        };

        // Calculate source region within tile
        let src_x = ((intersect_left - tile_x_pixels) as u32).min(self.tile_width);
        let src_y = ((intersect_top - tile_y_pixels) as u32).min(self.tile_height);

        // Copy tile region to viewport
        for y in 0..screen_height {
            for x in 0..screen_width {
                let src_px = src_x + x;
                let src_py = src_y + y;

                if src_px < self.tile_width && src_py < self.tile_height {
                    let dst_x = screen_x + x;
                    let dst_y = screen_y + y;

                    if dst_x < viewport.width_pixels && dst_y < viewport.height_pixels {
                        if let Some(src_pixel) = tile_rgba.get_pixel_checked(src_px, src_py) {
                            image.put_pixel(dst_x, dst_y, *src_pixel);
                        }
                    }
                }
            }
        }
    }

    /// Draw a placeholder tile (loading/missing indicator) with coordinates
    fn draw_placeholder(&self, image: &mut RgbaImage, x: u32, y: u32, width: u32, height: u32, coord: TileCoord) {
        // Draw light gray with border
        for py in y..y.saturating_add(height).min(image.height()) {
            for px in x..x.saturating_add(width).min(image.width()) {
                if px < image.width() && py < image.height() {
                    // Border in darker gray
                    if px == x || px == x + width - 1 || py == y || py == y + height - 1 {
                        image.put_pixel(px, py, image::Rgba([100, 100, 100, 255]));
                    } else {
                        image.put_pixel(px, py, image::Rgba([180, 180, 180, 255]));
                    }
                }
            }
        }
        
        // Draw coordinate text if there's enough space
        if width >= 60 && height >= 30 {
            use imageproc::drawing::draw_text_mut;
            use rusttype::{Font, Scale};
            
            // Use DejaVu Sans Mono font (embedded)
            let font_data: &[u8] = include_bytes!("../assets/ChakraPetchMono-Medium.otf");
            if let Some(font) = Font::try_from_bytes(font_data) {
                let scale = Scale::uniform(14.0);
                let text = format!("L{}:({},{})", coord.level, coord.x, coord.y);
                
                // Position text near top-left of placeholder
                let text_x = (x + 8).min(image.width().saturating_sub(1));
                let text_y = (y + 8).min(image.height().saturating_sub(1));
                
                draw_text_mut(
                    image,
                    image::Rgba([50, 50, 50, 255]), // Dark gray text
                    text_x as i32,
                    text_y as i32,
                    scale,
                    &font,
                    &text,
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::FileMetadata;
    use std::path::PathBuf;
    use tempfile::TempDir;

    fn create_test_cache() -> (TempDir, Arc<CacheManager>) {
        let temp_dir = TempDir::new().unwrap();
        let cache = CacheManager::new(temp_dir.path(), "test.bin".to_string()).unwrap();
        (temp_dir, Arc::new(cache))
    }

    #[test]
    fn test_viewport_renderer_creation() {
        let (_temp, cache) = create_test_cache();
        let renderer = ViewportRenderer::new(cache, 256, 256);
        assert_eq!(renderer.tile_width, 256);
        assert_eq!(renderer.tile_height, 256);
    }

    #[test]
    fn test_render_viewport_empty() {
        let (_temp, cache) = create_test_cache();
        let renderer = ViewportRenderer::new(cache, 256, 256);

        let viewport = Viewport::new(0, 512.0, 512.0, 1024, 768);
        let image = renderer.render_viewport(&viewport, None, 0.0);

        assert_eq!(image.width(), 1024);
        assert_eq!(image.height(), 768);
    }

    #[test]
    fn test_render_viewport_with_placeholder() {
        let (_temp, cache) = create_test_cache();
        let renderer = ViewportRenderer::new(cache, 256, 256);

        let mut viewport = Viewport::new(0, 512.0, 512.0, 1024, 768);
        viewport.visible_tiles = vec![TileCoord::new(0, 0, 0)];

        let image = renderer.render_viewport(&viewport, None, 0.0);

        assert_eq!(image.width(), 1024);
        assert_eq!(image.height(), 768);
        // Should have placeholder tiles (gray)
    }
}
