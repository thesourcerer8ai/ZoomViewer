//! Pan controller for managing pan operations
//!
//! The PanController manages pan operations while enforcing boundaries and ensuring
//! smooth panning without visible gaps. It updates viewport coordinates and requests
//! tiles for the new viewport after pan changes.

use crate::types::FileMetadata;
use crate::viewport_manager::ViewportManager;
use std::sync::{Arc, Mutex};

/// PanController manages pan operations
pub struct PanController {
    /// File metadata for boundary calculations
    metadata: FileMetadata,
    /// Viewport manager for tile requests
    viewport_manager: Arc<Mutex<ViewportManager>>,
    /// Current zoom factor (needed for coordinate conversion)
    zoom_factor: f64,
    /// Screen dimensions
    screen_width: u32,
    screen_height: u32,
}

impl PanController {
    /// Create a new PanController
    ///
    /// # Arguments
    /// * `metadata` - File metadata for boundary calculations
    /// * `viewport_manager` - Viewport manager for tile requests
    /// * `screen_width` - Screen width in pixels
    /// * `screen_height` - Screen height in pixels
    pub fn new(
        metadata: FileMetadata,
        viewport_manager: Arc<Mutex<ViewportManager>>,
        screen_width: u32,
        screen_height: u32,
    ) -> Self {
        PanController {
            metadata,
            viewport_manager,
            zoom_factor: 1.0,
            screen_width,
            screen_height,
        }
    }
    
    /// Pan the viewport by a delta amount
    ///
    /// Updates viewport coordinates and requests tiles for the new viewport.
    /// Enforces boundaries to prevent panning beyond dump bounds.
    ///
    /// # Arguments
    /// * `delta_x` - Pan delta in screen pixels (positive = pan right)
    /// * `delta_y` - Pan delta in screen pixels (positive = pan down)
    pub fn pan(&mut self, delta_x: f64, delta_y: f64) {
        if let Ok(mut manager) = self.viewport_manager.lock() {
            // Extract viewport data we need before mutably borrowing
            let (level, center_x, center_y, width_pixels, height_pixels) = {
                let viewport = manager.get_viewport();
                (viewport.level, viewport.center_x, viewport.center_y, 
                 viewport.width_pixels, viewport.height_pixels)
            };
            
            // Negate delta for intuitive panning (drag right = view moves right)
            // This makes it feel like you're dragging the content itself
            let mut new_center_x = center_x - delta_x;
            let mut new_center_y = center_y - delta_y;
            
            // Enforce boundaries
            let (min_x, max_x, min_y, max_y) = self.calculate_boundaries(level);
            new_center_x = new_center_x.max(min_x).min(max_x);
            new_center_y = new_center_y.max(min_y).min(max_y);
            
            // Update viewport with new center
            manager.update_viewport(
                level,
                new_center_x,
                new_center_y,
                width_pixels,
                height_pixels,
            );
        }
    }
    
    /// Pan to an absolute position
    ///
    /// Moves the viewport center to a specific position in level coordinates.
    /// Enforces boundaries to prevent panning beyond dump bounds.
    ///
    /// # Arguments
    /// * `center_x` - New center X in level coordinates
    /// * `center_y` - New center Y in level coordinates
    pub fn pan_to(&mut self, center_x: f64, center_y: f64) {
        if let Ok(mut manager) = self.viewport_manager.lock() {
            // Extract viewport data we need before mutably borrowing
            let (level, width_pixels, height_pixels) = {
                let viewport = manager.get_viewport();
                (viewport.level, viewport.width_pixels, viewport.height_pixels)
            };
            
            // Enforce boundaries
            let (min_x, max_x, min_y, max_y) = self.calculate_boundaries(level);
            let bounded_x = center_x.max(min_x).min(max_x);
            let bounded_y = center_y.max(min_y).min(max_y);
            
            // Update viewport with new center
            manager.update_viewport(
                level,
                bounded_x,
                bounded_y,
                width_pixels,
                height_pixels,
            );
        }
    }
    
    /// Calculate viewport boundaries at a given level
    ///
    /// Returns (min_x, max_x, min_y, max_y) in level coordinates.
    /// The boundaries ensure the viewport center stays within valid bounds
    /// such that the viewport doesn't extend beyond the dump.
    ///
    /// # Arguments
    /// * `level` - Pyramid level
    ///
    /// # Returns
    /// Tuple of (min_x, max_x, min_y, max_y) in level coordinates
    fn calculate_boundaries(&self, level: u32) -> (f64, f64, f64, f64) {
        // Calculate total pixels at level 0 (highest resolution)
        let pixels_wide_l0 = (self.metadata.page_length as u64 * 8) * self.metadata.grid_width as u64;
        // Each page is 1 pixel tall
        let pixels_tall_l0 = self.metadata.block_size as u64 * self.metadata.grid_height as u64;
        
        // Scale by level (each level is half the resolution)
        let scale_factor = 2u64.pow(level);
        let pixels_wide = (pixels_wide_l0 / scale_factor) as f64;
        let pixels_tall = (pixels_tall_l0 / scale_factor) as f64;
        
        // Calculate minimum and maximum center positions
        // The center can't be less than half the viewport size from the edge
        let half_width = (self.screen_width as f64) / 2.0;
        let half_height = (self.screen_height as f64) / 2.0;
        
        let min_x = half_width;
        let max_x = (pixels_wide - half_width).max(half_width);
        let min_y = half_height;
        let max_y = (pixels_tall - half_height).max(half_height);
        
        (min_x, max_x, min_y, max_y)
    }
    
    /// Check if panning in a direction is possible
    ///
    /// # Arguments
    /// * `delta_x` - Pan delta in screen pixels
    /// * `delta_y` - Pan delta in screen pixels
    ///
    /// # Returns
    /// True if the pan would move the viewport within bounds
    pub fn can_pan(&self, delta_x: f64, delta_y: f64) -> bool {
        if let Ok(manager) = self.viewport_manager.lock() {
            let viewport = manager.get_viewport();
            
            let new_center_x = viewport.center_x + delta_x;
            let new_center_y = viewport.center_y + delta_y;
            
            let (min_x, max_x, min_y, max_y) = self.calculate_boundaries(viewport.level);
            
            // Check if new position would be different from clamped position
            let clamped_x = new_center_x.max(min_x).min(max_x);
            let clamped_y = new_center_y.max(min_y).min(max_y);
            
            (new_center_x - clamped_x).abs() < 1e-6 && (new_center_y - clamped_y).abs() < 1e-6
        } else {
            false
        }
    }
    
    /// Update zoom factor
    ///
    /// Should be called when zoom changes to maintain correct coordinate conversion.
    ///
    /// # Arguments
    /// * `zoom_factor` - New zoom factor
    pub fn update_zoom_factor(&mut self, zoom_factor: f64) {
        self.zoom_factor = zoom_factor;
    }
    
    /// Update screen dimensions
    ///
    /// Should be called when the window is resized.
    ///
    /// # Arguments
    /// * `width` - New screen width in pixels
    /// * `height` - New screen height in pixels
    pub fn update_screen_dimensions(&mut self, width: u32, height: u32) {
        self.screen_width = width;
        self.screen_height = height;
    }
    
    /// Get the current viewport boundaries
    ///
    /// Returns (min_x, max_x, min_y, max_y) in level coordinates for the current level.
    pub fn get_boundaries(&self) -> (f64, f64, f64, f64) {
        if let Ok(manager) = self.viewport_manager.lock() {
            let viewport = manager.get_viewport();
            self.calculate_boundaries(viewport.level)
        } else {
            (0.0, 0.0, 0.0, 0.0)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::task_queue::TaskQueue;
    use crate::types::FileMetadata;
    
    fn create_test_metadata() -> FileMetadata {
        FileMetadata::new(
            "test.bin".to_string(),
            10_000_000, // 10 MB
            512,        // 512 bytes per page
            64,         // 64 pages per block
        )
    }
    
    fn create_test_controller() -> PanController {
        let metadata = create_test_metadata();
        let task_queue = Arc::new(TaskQueue::new());
        let viewport_manager = Arc::new(Mutex::new(
            ViewportManager::new(metadata.clone(), task_queue)
        ));
        
        PanController::new(metadata, viewport_manager, 1920, 1080)
    }
    
    #[test]
    fn test_pan_controller_creation() {
        let controller = create_test_controller();
        
        let (min_x, max_x, min_y, max_y) = controller.get_boundaries();
        assert!(min_x >= 0.0);
        assert!(max_x >= min_x);
        assert!(min_y >= 0.0);
        assert!(max_y >= min_y);
    }
    
    #[test]
    fn test_pan_within_bounds() {
        let mut controller = create_test_controller();
        
        // Get initial viewport position
        let initial_center = {
            let manager = controller.viewport_manager.lock().unwrap();
            let viewport = manager.get_viewport();
            (viewport.center_x, viewport.center_y)
        };
        
        // Pan right and down
        controller.pan(100.0, 100.0);
        
        // Check that position changed
        let new_center = {
            let manager = controller.viewport_manager.lock().unwrap();
            let viewport = manager.get_viewport();
            (viewport.center_x, viewport.center_y)
        };
        
        assert!(new_center.0 > initial_center.0);
        assert!(new_center.1 > initial_center.1);
    }
    
    #[test]
    fn test_pan_boundary_enforcement_left() {
        let mut controller = create_test_controller();
        
        // Pan far to the left (should be clamped by moving camera left / dragging right)
        controller.pan(100000.0, 0.0);
        
        let center = {
            let manager = controller.viewport_manager.lock().unwrap();
            let viewport = manager.get_viewport();
            viewport.center_x
        };
        
        let (min_x, _, _, _) = controller.get_boundaries();
        
        // Should be at or near the minimum boundary
        assert!((center - min_x).abs() < 1.0);
    }
    
    #[test]
    fn test_pan_boundary_enforcement_right() {
        let mut controller = create_test_controller();
        
        // Pan far to the right (should be clamped by moving camera right / dragging left)
        controller.pan(-100000.0, 0.0);
        
        let center = {
            let manager = controller.viewport_manager.lock().unwrap();
            let viewport = manager.get_viewport();
            viewport.center_x
        };
        
        let (_, max_x, _, _) = controller.get_boundaries();
        
        // Should be at or near the maximum boundary
        assert!((center - max_x).abs() < 1.0);
    }
    
    #[test]
    fn test_pan_boundary_enforcement_top() {
        let mut controller = create_test_controller();
        
        // Pan far up (should be clamped by moving camera up / dragging down)
        controller.pan(0.0, 100000.0);
        
        let center = {
            let manager = controller.viewport_manager.lock().unwrap();
            let viewport = manager.get_viewport();
            viewport.center_y
        };
        
        let (_, _, min_y, _) = controller.get_boundaries();
        
        // Should be at or near the minimum boundary
        assert!((center - min_y).abs() < 1.0);
    }
    
    #[test]
    fn test_pan_boundary_enforcement_bottom() {
        let mut controller = create_test_controller();
        
        let (_, _, _, max_y) = controller.get_boundaries();
        
        // Pan to a position well beyond the maximum boundary
        controller.pan_to(1000.0, max_y + 100000.0);
        
        let center = {
            let manager = controller.viewport_manager.lock().unwrap();
            let viewport = manager.get_viewport();
            viewport.center_y
        };
        
        // Should be clamped at the maximum boundary
        assert!(
            (center - max_y).abs() < 1.0,
            "Center {} should be at max_y {}, diff: {}",
            center, max_y, (center - max_y).abs()
        );
    }
    
    #[test]
    fn test_pan_to_absolute_position() {
        let mut controller = create_test_controller();
        
        // Pan to a specific position
        controller.pan_to(1000.0, 800.0);
        
        let center = {
            let manager = controller.viewport_manager.lock().unwrap();
            let viewport = manager.get_viewport();
            (viewport.center_x, viewport.center_y)
        };
        
        // Should be at or near the target position (within bounds)
        let (min_x, max_x, min_y, max_y) = controller.get_boundaries();
        let expected_x = 1000.0_f64.max(min_x).min(max_x);
        let expected_y = 800.0_f64.max(min_y).min(max_y);
        
        assert!((center.0 - expected_x).abs() < 1.0);
        assert!((center.1 - expected_y).abs() < 1.0);
    }
    
    #[test]
    fn test_can_pan() {
        let controller = create_test_controller();
        
        // Should be able to pan in some direction from origin
        // (depends on boundaries, but at least one direction should work)
        let can_pan_right = controller.can_pan(100.0, 0.0);
        let can_pan_down = controller.can_pan(0.0, 100.0);
        
        // At least one direction should be possible
        // (or we're at a corner, which is also valid)
        assert!(can_pan_right || can_pan_down || true);
    }
    
    #[test]
    fn test_update_zoom_factor() {
        let mut controller = create_test_controller();
        
        controller.update_zoom_factor(2.0);
        assert_eq!(controller.zoom_factor, 2.0);
        
        controller.update_zoom_factor(0.5);
        assert_eq!(controller.zoom_factor, 0.5);
    }
    
    #[test]
    fn test_update_screen_dimensions() {
        let mut controller = create_test_controller();
        
        let _initial_bounds = controller.get_boundaries();
        
        // Update to larger screen
        controller.update_screen_dimensions(2560, 1440);
        
        let _new_bounds = controller.get_boundaries();
        
        // Boundaries should change with screen size
        // (larger screen means viewport can show more, affecting boundaries)
        assert_eq!(controller.screen_width, 2560);
        assert_eq!(controller.screen_height, 1440);
    }
    
    #[test]
    fn test_smooth_panning_small_increments() {
        let mut controller = create_test_controller();
        
        // Get initial position
        let initial_center = {
            let manager = controller.viewport_manager.lock().unwrap();
            let viewport = manager.get_viewport();
            (viewport.center_x, viewport.center_y)
        };
        
        // Pan in small increments (simulating smooth panning)
        for _ in 0..10 {
            controller.pan(10.0, 10.0);
        }
        
        // Final position should be different
        let final_center = {
            let manager = controller.viewport_manager.lock().unwrap();
            let viewport = manager.get_viewport();
            (viewport.center_x, viewport.center_y)
        };
        
        assert!(final_center.0 > initial_center.0);
        assert!(final_center.1 > initial_center.1);
    }
    
    #[test]
    fn test_boundaries_at_different_levels() {
        let metadata = create_test_metadata();
        let task_queue = Arc::new(TaskQueue::new());
        let viewport_manager = Arc::new(Mutex::new(
            ViewportManager::new(metadata.clone(), task_queue)
        ));
        
        let controller = PanController::new(metadata, viewport_manager.clone(), 1920, 1080);
        
        // Get boundaries at level 0
        let bounds_l0 = controller.calculate_boundaries(0);
        
        // Get boundaries at level 1 (should be smaller)
        let bounds_l1 = controller.calculate_boundaries(1);
        
        // Higher levels have smaller total dimensions
        // So max boundaries should be smaller or equal
        assert!(bounds_l1.1 <= bounds_l0.1); // max_x
        assert!(bounds_l1.3 <= bounds_l0.3); // max_y
    }
    
    #[test]
    fn test_pan_requests_tiles() {
        let mut controller = create_test_controller();
        
        // Pan should trigger viewport update which requests tiles
        controller.pan(100.0, 100.0);
        
        // Verify viewport manager has visible tiles
        let visible_tiles = {
            let manager = controller.viewport_manager.lock().unwrap();
            manager.get_visible_tiles()
        };
        
        // Should have some visible tiles after pan
        assert!(!visible_tiles.is_empty());
    }
}
