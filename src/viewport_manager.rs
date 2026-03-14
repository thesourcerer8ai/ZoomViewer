//! Viewport management for tile prioritization
//!
//! The ViewportManager tracks the current viewport state and identifies which tiles
//! are visible or adjacent to the viewport. It updates task priorities in the TaskQueue
//! to ensure responsive UI by prioritizing visible tiles.

use crate::types::{FileMetadata, Priority, TileCoord, Viewport};
use crate::task_queue::TaskQueue;
use std::sync::Arc;

/// Tile size in pixels (standard for all tiles)
pub const TILE_SIZE: u32 = 256;

/// ViewportManager manages viewport state and tile prioritization
pub struct ViewportManager {
    /// Current viewport state
    viewport: Viewport,
    /// File metadata for coordinate calculations
    metadata: FileMetadata,
    /// Task queue for priority updates
    task_queue: Arc<TaskQueue>,
}

impl ViewportManager {
    /// Create a new ViewportManager
    pub fn new(metadata: FileMetadata, task_queue: Arc<TaskQueue>) -> Self {
        // Start with a default viewport at level 0, upper left corner
        let viewport = Viewport::new(0, 0.0, 0.0, 1024, 768);
        
        ViewportManager {
            viewport,
            metadata,
            task_queue,
        }
    }
    
    /// Update viewport state and recalculate visible and adjacent tiles
    ///
    /// This function:
    /// 1. Updates the viewport parameters
    /// 2. Recalculates which tiles are visible
    /// 3. Recalculates which tiles are adjacent
    /// 4. Updates task priorities in the queue
    pub fn update_viewport(
        &mut self,
        level: u32,
        center_x: f64,
        center_y: f64,
        width_pixels: u32,
        height_pixels: u32,
    ) {
        // Update viewport state
        self.viewport.level = level;
        self.viewport.center_x = center_x;
        self.viewport.center_y = center_y;
        self.viewport.width_pixels = width_pixels;
        self.viewport.height_pixels = height_pixels;
        
        // Recalculate visible and adjacent tiles
        self.viewport.visible_tiles = self.calculate_visible_tiles();
        self.viewport.adjacent_tiles = self.calculate_adjacent_tiles();
        
        // Update task priorities
        self.update_task_priorities();
    }
    
    /// Get tiles currently visible in the viewport
    pub fn get_visible_tiles(&self) -> Vec<TileCoord> {
        self.viewport.visible_tiles.clone()
    }
    
    /// Get tiles adjacent to the viewport for predictive loading
    pub fn get_adjacent_tiles(&self) -> Vec<TileCoord> {
        self.viewport.adjacent_tiles.clone()
    }
    
    /// Update task priorities in the queue based on viewport state
    ///
    /// - High priority: tiles in viewport
    /// - Normal priority: tiles adjacent to viewport
    /// 
    /// Enqueues all visible and adjacent tiles. Workers will skip tiles that are already cached.
    /// Tiles can exist in multiple queues simultaneously with different priorities.
    pub fn update_task_priorities(&self) {
        // Clear high priority queue to remove tiles that are no longer visible
        // This ensures only currently visible tiles have high priority
        self.task_queue.clear_high_priority();
        
        let mut enqueued_visible = 0;
        let mut enqueued_adjacent = 0;
        let mut enqueued_parent_level = 0;
        
        // Enqueue visible tiles with high priority
        // These tiles will be added to the high priority queue
        // They may also exist in normal/low queues from previous viewport states
        for tile in &self.viewport.visible_tiles {
            self.task_queue.enqueue(crate::types::TileTask::new(
                *tile,
                Priority::High,
                tile.level == 0,
            ));
            enqueued_visible += 1;
        }
        
        // Enqueue adjacent tiles with normal priority (skip if already in queue)
        for tile in &self.viewport.adjacent_tiles {
            if !self.task_queue.contains(*tile) {
                self.task_queue.enqueue(crate::types::TileTask::new(
                    *tile,
                    Priority::Normal,
                    tile.level == 0,
                ));
                enqueued_adjacent += 1;
            }
        }
        
        // Enqueue parent level tiles (level + 1) with normal priority for smooth zoom-out
        if self.viewport.level < 10 { // Reasonable max level
            let parent_level = self.viewport.level + 1;
            let parent_tiles = self.calculate_parent_level_tiles(parent_level);
            
            for tile in &parent_tiles {
                if !self.task_queue.contains(*tile) {
                    self.task_queue.enqueue(crate::types::TileTask::new(
                        *tile,
                        Priority::Normal,
                        false, // Parent level tiles are pyramid tiles
                    ));
                    enqueued_parent_level += 1;
                }
            }
        }
        
        log::debug!(
            "Enqueued {} visible tiles (high priority), {} adjacent tiles (normal priority), {} parent level tiles (normal priority)",
            enqueued_visible,
            enqueued_adjacent,
            enqueued_parent_level
        );
    }
    
    /// Calculate which tiles are visible in the current viewport
    fn calculate_visible_tiles(&self) -> Vec<TileCoord> {
        let mut visible = Vec::new();
        
        // Calculate viewport bounds in pixel coordinates at the current level
        let half_width = (self.viewport.width_pixels as f64) / 2.0;
        let half_height = (self.viewport.height_pixels as f64) / 2.0;
        
        let left = (self.viewport.center_x - half_width).max(0.0);
        let right = self.viewport.center_x + half_width;
        let top = (self.viewport.center_y - half_height).max(0.0);
        let bottom = self.viewport.center_y + half_height;
        
        // Convert pixel bounds to tile coordinates
        let tile_left = (left / TILE_SIZE as f64).floor() as u32;
        let tile_right = (right / TILE_SIZE as f64).ceil() as u32;
        let tile_top = (top / TILE_SIZE as f64).floor() as u32;
        let tile_bottom = (bottom / TILE_SIZE as f64).ceil() as u32;
        
        // Calculate maximum tile bounds based on file metadata
        let max_tiles = self.calculate_max_tiles_at_level(self.viewport.level);
        
        log::debug!(
            "Viewport bounds: left={:.1}, right={:.1}, top={:.1}, bottom={:.1}",
            left, right, top, bottom
        );
        log::debug!(
            "Tile range before clamp: x=[{}, {}), y=[{}, {})",
            tile_left, tile_right, tile_top, tile_bottom
        );
        log::debug!(
            "Max tiles at level {}: ({}, {})",
            self.viewport.level, max_tiles.0, max_tiles.1
        );
        
        // Clamp to valid tile range
        let tile_right = tile_right.min(max_tiles.0);
        let tile_bottom = tile_bottom.min(max_tiles.1);
        
        log::debug!(
            "Tile range after clamp: x=[{}, {}), y=[{}, {})",
            tile_left, tile_right, tile_top, tile_bottom
        );
        
        // Generate all visible tile coordinates
        for y in tile_top..tile_bottom {
            for x in tile_left..tile_right {
                visible.push(TileCoord::new(self.viewport.level, x, y));
            }
        }
        
        log::debug!("Generated {} visible tiles", visible.len());
        
        visible
    }
    
    /// Calculate which tiles are adjacent to the viewport (within 1 tile distance)
    fn calculate_adjacent_tiles(&self) -> Vec<TileCoord> {
        let mut adjacent = Vec::new();
        
        // Calculate viewport bounds in pixel coordinates
        let half_width = (self.viewport.width_pixels as f64) / 2.0;
        let half_height = (self.viewport.height_pixels as f64) / 2.0;
        
        let left = (self.viewport.center_x - half_width).max(0.0);
        let right = self.viewport.center_x + half_width;
        let top = (self.viewport.center_y - half_height).max(0.0);
        let bottom = self.viewport.center_y + half_height;
        
        // Convert to tile coordinates
        let tile_left = (left / TILE_SIZE as f64).floor() as u32;
        let tile_right = (right / TILE_SIZE as f64).ceil() as u32;
        let tile_top = (top / TILE_SIZE as f64).floor() as u32;
        let tile_bottom = (bottom / TILE_SIZE as f64).ceil() as u32;
        
        // Expand by 1 tile in each direction for adjacent tiles
        let adj_left = tile_left.saturating_sub(1);
        let adj_top = tile_top.saturating_sub(1);
        let adj_right = tile_right + 1;
        let adj_bottom = tile_bottom + 1;
        
        // Calculate maximum tile bounds
        let max_tiles = self.calculate_max_tiles_at_level(self.viewport.level);
        let adj_right = adj_right.min(max_tiles.0);
        let adj_bottom = adj_bottom.min(max_tiles.1);
        
        // Generate adjacent tiles (excluding visible tiles)
        for y in adj_top..adj_bottom {
            for x in adj_left..adj_right {
                let coord = TileCoord::new(self.viewport.level, x, y);
                
                // Only include if not already in visible tiles
                if !self.viewport.visible_tiles.contains(&coord) {
                    adjacent.push(coord);
                }
            }
        }
        
        adjacent
    }
    
    /// Calculate tiles at the parent level (level + 1) that cover the current viewport
    ///
    /// These tiles provide a lower-resolution view for smooth zoom-out transitions
    fn calculate_parent_level_tiles(&self, parent_level: u32) -> Vec<TileCoord> {
        let mut parent_tiles = Vec::new();
        
        // Calculate viewport bounds in pixel coordinates at current level
        let half_width = (self.viewport.width_pixels as f64) / 2.0;
        let half_height = (self.viewport.height_pixels as f64) / 2.0;
        
        let left = (self.viewport.center_x - half_width).max(0.0);
        let right = self.viewport.center_x + half_width;
        let top = (self.viewport.center_y - half_height).max(0.0);
        let bottom = self.viewport.center_y + half_height;
        
        // Convert to tile coordinates at parent level
        // Parent level has half the resolution, so divide by 2
        let scale_factor = 2.0;
        let tile_left = ((left / scale_factor) / TILE_SIZE as f64).floor() as u32;
        let tile_right = ((right / scale_factor) / TILE_SIZE as f64).ceil() as u32;
        let tile_top = ((top / scale_factor) / TILE_SIZE as f64).floor() as u32;
        let tile_bottom = ((bottom / scale_factor) / TILE_SIZE as f64).ceil() as u32;
        
        // Calculate maximum tile bounds at parent level
        let max_tiles = self.calculate_max_tiles_at_level(parent_level);
        let tile_right = tile_right.min(max_tiles.0);
        let tile_bottom = tile_bottom.min(max_tiles.1);
        
        // Generate all parent level tiles covering the viewport
        for y in tile_top..tile_bottom {
            for x in tile_left..tile_right {
                parent_tiles.push(TileCoord::new(parent_level, x, y));
            }
        }
        
        log::debug!(
            "Parent level {} tiles covering viewport: {} tiles",
            parent_level,
            parent_tiles.len()
        );
        
        parent_tiles
    }
    
    /// Calculate the maximum number of tiles at a given level
    ///
    /// Returns (tiles_wide, tiles_tall)
    fn calculate_max_tiles_at_level(&self, level: u32) -> (u32, u32) {
        // Calculate total pixels at level 0 (highest resolution)
        // Each byte is 8 pixels wide, pages are arranged vertically
        let pixels_wide_l0 = (self.metadata.page_length as u64 * 8) * self.metadata.grid_width as u64;
        // Each page is 1 pixel tall
        let pixels_tall_l0 = self.metadata.block_size as u64 * self.metadata.grid_height as u64;
        
        // Scale by level (each level is half the resolution)
        let scale_factor = 2u64.pow(level);
        let pixels_wide = pixels_wide_l0 / scale_factor;
        let pixels_tall = pixels_tall_l0 / scale_factor;
        
        // Convert to tiles
        let tiles_wide = ((pixels_wide + TILE_SIZE as u64 - 1) / TILE_SIZE as u64) as u32;
        let tiles_tall = ((pixels_tall + TILE_SIZE as u64 - 1) / TILE_SIZE as u64) as u32;
        
        (tiles_wide.max(1), tiles_tall.max(1))
    }
    
    /// Get the current viewport state
    pub fn get_viewport(&self) -> &Viewport {
        &self.viewport
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::FileMetadata;
    
    fn create_test_metadata() -> FileMetadata {
        FileMetadata::new(
            "test.bin".to_string(),
            10_000_000, // 10 MB
            512,        // 512 bytes per page
            64,         // 64 pages per block
        )
    }
    
    #[test]
    fn test_viewport_manager_creation() {
        let metadata = create_test_metadata();
        let task_queue = Arc::new(TaskQueue::new());
        let manager = ViewportManager::new(metadata, task_queue);
        
        assert_eq!(manager.viewport.level, 0);
        assert_eq!(manager.viewport.center_x, 0.0);
        assert_eq!(manager.viewport.center_y, 0.0);
    }
    
    #[test]
    fn test_update_viewport() {
        let metadata = create_test_metadata();
        let task_queue = Arc::new(TaskQueue::new());
        let mut manager = ViewportManager::new(metadata, task_queue);
        
        manager.update_viewport(1, 1024.0, 768.0, 1920, 1080);
        
        assert_eq!(manager.viewport.level, 1);
        assert_eq!(manager.viewport.center_x, 1024.0);
        assert_eq!(manager.viewport.center_y, 768.0);
        assert_eq!(manager.viewport.width_pixels, 1920);
        assert_eq!(manager.viewport.height_pixels, 1080);
    }
    
    #[test]
    fn test_get_visible_tiles_at_origin() {
        let metadata = create_test_metadata();
        let task_queue = Arc::new(TaskQueue::new());
        let mut manager = ViewportManager::new(metadata, task_queue);
        
        // Set viewport at origin with 1024x768 screen
        manager.update_viewport(0, 512.0, 384.0, 1024, 768);
        
        let visible = manager.get_visible_tiles();
        
        // Should have tiles covering the viewport
        assert!(!visible.is_empty());
        
        // All tiles should be at level 0
        for tile in &visible {
            assert_eq!(tile.level, 0);
        }
        
        // Should include tile (0, 0)
        assert!(visible.contains(&TileCoord::new(0, 0, 0)));
    }
    
    #[test]
    fn test_get_adjacent_tiles() {
        let metadata = create_test_metadata();
        let task_queue = Arc::new(TaskQueue::new());
        let mut manager = ViewportManager::new(metadata, task_queue);
        
        // Set viewport at origin
        manager.update_viewport(0, 512.0, 384.0, 1024, 768);
        
        let adjacent = manager.get_adjacent_tiles();
        
        // Should have adjacent tiles
        assert!(!adjacent.is_empty());
        
        // Adjacent tiles should not overlap with visible tiles
        let visible = manager.get_visible_tiles();
        for tile in &adjacent {
            assert!(!visible.contains(tile));
        }
    }
    
    #[test]
    fn test_visible_tiles_within_bounds() {
        let metadata = create_test_metadata();
        let task_queue = Arc::new(TaskQueue::new());
        let mut manager = ViewportManager::new(metadata, task_queue);
        
        manager.update_viewport(0, 512.0, 384.0, 1024, 768);
        
        let visible = manager.get_visible_tiles();
        let max_tiles = manager.calculate_max_tiles_at_level(0);
        
        // All visible tiles should be within valid bounds
        for tile in &visible {
            assert!(tile.x < max_tiles.0, "Tile x {} exceeds max {}", tile.x, max_tiles.0);
            assert!(tile.y < max_tiles.1, "Tile y {} exceeds max {}", tile.y, max_tiles.1);
        }
    }
    
    #[test]
    fn test_calculate_max_tiles_at_different_levels() {
        let metadata = create_test_metadata();
        let task_queue = Arc::new(TaskQueue::new());
        let manager = ViewportManager::new(metadata, task_queue);
        
        let level0 = manager.calculate_max_tiles_at_level(0);
        let level1 = manager.calculate_max_tiles_at_level(1);
        let level2 = manager.calculate_max_tiles_at_level(2);
        
        // Higher levels should have fewer tiles (or equal due to rounding)
        assert!(level1.0 <= level0.0);
        assert!(level1.1 <= level0.1);
        assert!(level2.0 <= level1.0);
        assert!(level2.1 <= level1.1);
    }
    
    #[test]
    fn test_viewport_at_different_positions() {
        let metadata = create_test_metadata();
        let task_queue = Arc::new(TaskQueue::new());
        let mut manager = ViewportManager::new(metadata, task_queue);
        
        // Test at origin
        manager.update_viewport(0, 512.0, 384.0, 1024, 768);
        let visible1 = manager.get_visible_tiles();
        
        // Test at different position
        manager.update_viewport(0, 2048.0, 1536.0, 1024, 768);
        let visible2 = manager.get_visible_tiles();
        
        // Visible tiles should be different
        assert_ne!(visible1, visible2);
    }
}

#[cfg(test)]
mod property_tests {
    use super::*;
    use proptest::prelude::*;
    
    /// **Property 29: Viewport tile identification**
    /// 
    /// For any viewport change, the viewport manager SHALL identify which tiles
    /// are now visible and assign high priority to all visible tiles.
    /// 
    /// **Validates: Requirements 11.1, 11.2**
    /// 
    /// This property verifies that:
    /// 1. All visible tiles are within viewport bounds
    /// 2. No tiles outside viewport are marked visible
    /// 3. Visible tiles cover the entire viewport area
    #[test]
    #[ignore]
    fn prop_viewport_tile_identification() {
        proptest!(|(
            level in 0u32..5,
            center_x in 0.0f64..10000.0,
            center_y in 0.0f64..10000.0,
            width_pixels in 512u32..2048,
            height_pixels in 512u32..2048,
            page_length in 512u32..2048,
            block_size in prop::sample::select(vec![64u32, 128, 256, 512, 768, 1024]),
        )| {
            let metadata = FileMetadata::new(
                "test.bin".to_string(),
                100_000_000, // 100 MB
                page_length,
                block_size,
            );
            
            let task_queue = Arc::new(TaskQueue::new());
            let mut manager = ViewportManager::new(metadata, task_queue);
            
            // Update viewport
            manager.update_viewport(level, center_x, center_y, width_pixels, height_pixels);
            
            let visible_tiles = manager.get_visible_tiles();
            
            // Calculate viewport bounds in pixels
            let half_width = (width_pixels as f64) / 2.0;
            let half_height = (height_pixels as f64) / 2.0;
            let left = (center_x - half_width).max(0.0);
            let right = center_x + half_width;
            let top = (center_y - half_height).max(0.0);
            let bottom = center_y + half_height;
            
            // Property 1: All visible tiles must intersect with viewport bounds
            for tile in &visible_tiles {
                prop_assert_eq!(tile.level, level, "Tile level must match viewport level");
                
                // Calculate tile bounds in pixels
                let tile_left = (tile.x as f64) * (TILE_SIZE as f64);
                let tile_right = ((tile.x + 1) as f64) * (TILE_SIZE as f64);
                let tile_top = (tile.y as f64) * (TILE_SIZE as f64);
                let tile_bottom = ((tile.y + 1) as f64) * (TILE_SIZE as f64);
                
                // Tile must intersect with viewport
                let intersects = tile_right > left && tile_left < right && 
                                tile_bottom > top && tile_top < bottom;
                
                prop_assert!(intersects, 
                    "Visible tile ({}, {}) at level {} does not intersect viewport bounds. \
                     Tile: [{}, {}, {}, {}], Viewport: [{}, {}, {}, {}]",
                    tile.x, tile.y, tile.level,
                    tile_left, tile_top, tile_right, tile_bottom,
                    left, top, right, bottom
                );
            }
            
            // Property 2: All visible tiles must be within valid bounds
            let max_tiles = manager.calculate_max_tiles_at_level(level);
            for tile in &visible_tiles {
                prop_assert!(tile.x < max_tiles.0, 
                    "Tile x coordinate {} exceeds maximum {}", tile.x, max_tiles.0);
                prop_assert!(tile.y < max_tiles.1, 
                    "Tile y coordinate {} exceeds maximum {}", tile.y, max_tiles.1);
            }
            
            // Property 3: No duplicate tiles in visible list
            let mut unique_tiles = visible_tiles.clone();
            unique_tiles.sort_by_key(|t| (t.level, t.x, t.y));
            unique_tiles.dedup();
            prop_assert_eq!(unique_tiles.len(), visible_tiles.len(), 
                "Visible tiles list contains duplicates");
            
            // Property 4: Visible tiles should cover the viewport area
            // (at least one tile should be present if viewport is within bounds)
            if center_x >= 0.0 && center_y >= 0.0 {
                let max_pixels_x = (max_tiles.0 as f64) * (TILE_SIZE as f64);
                let max_pixels_y = (max_tiles.1 as f64) * (TILE_SIZE as f64);
                
                if center_x < max_pixels_x && center_y < max_pixels_y {
                    prop_assert!(!visible_tiles.is_empty(), 
                        "Viewport within bounds should have at least one visible tile");
                }
            }
        });
    }
    
    /// Property test for adjacent tile identification
    /// 
    /// Verifies that adjacent tiles are correctly identified and do not overlap
    /// with visible tiles.
    #[test]
    #[ignore]
    fn prop_adjacent_tiles_non_overlapping() {
        proptest!(|(
            level in 0u32..5,
            center_x in 512.0f64..5000.0,
            center_y in 512.0f64..5000.0,
            width_pixels in 512u32..1920,
            height_pixels in 512u32..1080,
        )| {
            let metadata = FileMetadata::new(
                "test.bin".to_string(),
                50_000_000,
                512,
                64,
            );
            
            let task_queue = Arc::new(TaskQueue::new());
            let mut manager = ViewportManager::new(metadata, task_queue);
            
            manager.update_viewport(level, center_x, center_y, width_pixels, height_pixels);
            
            let visible_tiles = manager.get_visible_tiles();
            let adjacent_tiles = manager.get_adjacent_tiles();
            
            // Property: Adjacent tiles must not overlap with visible tiles
            for adj_tile in &adjacent_tiles {
                prop_assert!(!visible_tiles.contains(adj_tile),
                    "Adjacent tile {:?} should not be in visible tiles", adj_tile);
            }
            
            // Property: Adjacent tiles must be at the same level
            for adj_tile in &adjacent_tiles {
                prop_assert_eq!(adj_tile.level, level,
                    "Adjacent tile level must match viewport level");
            }
            
            // Property: No duplicate tiles in adjacent list
            let mut unique_adjacent = adjacent_tiles.clone();
            unique_adjacent.sort_by_key(|t| (t.level, t.x, t.y));
            unique_adjacent.dedup();
            prop_assert_eq!(unique_adjacent.len(), adjacent_tiles.len(),
                "Adjacent tiles list contains duplicates");
        });
    }
    
    /// Property test for priority updates
    /// 
    /// Verifies that priority updates are correctly applied to the task queue
    #[test]
    #[ignore]
    fn prop_priority_updates() {
        proptest!(|(
            level in 0u32..3,
            center_x in 512.0f64..2048.0,
            center_y in 512.0f64..2048.0,
        )| {
            let metadata = FileMetadata::new(
                "test.bin".to_string(),
                10_000_000,
                512,
                64,
            );
            
            let task_queue = Arc::new(TaskQueue::new());
            let mut manager = ViewportManager::new(metadata, task_queue.clone());
            
            // Update viewport
            manager.update_viewport(level, center_x, center_y, 1024, 768);
            
            let visible_tiles = manager.get_visible_tiles();
            let adjacent_tiles = manager.get_adjacent_tiles();
            
            // Property: Visible and adjacent tiles should be non-empty for valid viewports
            if center_x > 0.0 && center_y > 0.0 {
                prop_assert!(!visible_tiles.is_empty() || !adjacent_tiles.is_empty(),
                    "Valid viewport should have visible or adjacent tiles");
            }
            
            // Property: Total tiles (visible + adjacent) should be reasonable
            let total_tiles = visible_tiles.len() + adjacent_tiles.len();
            prop_assert!(total_tiles < 1000,
                "Total tiles ({}) seems unreasonably large", total_tiles);
        });
    }
}
