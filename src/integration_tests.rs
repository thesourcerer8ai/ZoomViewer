//! Integration tests for NAND Flash Viewer
//!
//! These tests verify end-to-end workflows and component interactions.

#[cfg(test)]
mod tests {
    use crate::*;
    use std::sync::Arc;
    use tempfile::{TempDir, NamedTempFile};
    use std::io::{Write, Seek};

    /// Helper to create a test dump file
    fn create_test_dump(size_bytes: u64) -> NamedTempFile {
        let mut file = NamedTempFile::new().unwrap();
        
        // Write some test data at the beginning
        let test_data: Vec<u8> = (0..10240).map(|i| (i % 256) as u8).collect();
        file.write_all(&test_data).unwrap();
        
        // Seek to create a sparse file of the desired size
        if size_bytes > 10240 {
            file.seek(std::io::SeekFrom::Start(size_bytes - 1)).unwrap();
            file.write_all(&[0xFF]).unwrap();
        }
        
        file.flush().unwrap();
        file
    }

    /// Test 24.1: File load → metadata detection → tile generation
    /// 
    /// Validates: Requirements 1.1, 1.2, 1.3, 1.4, 1.6
    #[test]
    fn test_file_load_to_tile_generation() {
        // Create a test dump file (1 MB)
        let temp_file = create_test_dump(1_000_000);
        
        // Load the file with metadata
        let file_loader = FileLoader::new(temp_file.path(), 2048, 64).unwrap();
        let metadata = file_loader.get_metadata().clone();
        
        // Verify metadata was stored correctly
        assert_eq!(metadata.page_length, 2048);
        assert_eq!(metadata.block_size, 64);
        assert!(metadata.size >= 1_000_000);
        
        // Generate a tile at level 0
        let coord = TileCoord::new(0, 0, 0);
        let mut file_loader_mut = file_loader;
        let tile_data = TileGenerator::generate_tile(coord, &metadata, &mut file_loader_mut);
        
        // Verify tile was generated successfully
        assert!(tile_data.is_ok(), "Tile generation should succeed");
        let tile_bytes = tile_data.unwrap();
        
        // Verify it's valid QOI data
        assert!(tile_bytes.len() > 14, "QOI should have header + data");
        assert_eq!(&tile_bytes[0..4], b"qoif", "Should be valid QOI");
    }

    /// Test 24.2: Viewport change → priority update → tile generation
    /// 
    /// Validates: Requirements 11.1, 11.2, 11.3, 11.4, 11.5
    #[test]
    fn test_viewport_driven_loading() {
        let temp_file = create_test_dump(10_000_000);
        let file_loader = FileLoader::new(temp_file.path(), 2048, 128).unwrap();
        let metadata = file_loader.get_metadata().clone();
        
        // Create task queue
        let task_queue = Arc::new(TaskQueue::new());
        
        // Create viewport manager
        let mut viewport_manager = ViewportManager::new(metadata.clone(), task_queue.clone());
        
        // Update viewport to a specific position
        viewport_manager.update_viewport(0, 512.0, 384.0, 1024, 768);
        
        // Get visible tiles
        let visible_tiles = viewport_manager.get_visible_tiles();
        assert!(!visible_tiles.is_empty(), "Should have visible tiles");
        
        // Get adjacent tiles
        let adjacent_tiles = viewport_manager.get_adjacent_tiles();
        assert!(!adjacent_tiles.is_empty(), "Should have adjacent tiles");
        
        // Enqueue tiles with priorities
        for tile in &visible_tiles {
            task_queue.enqueue(TileTask::new(*tile, Priority::High, tile.level == 0));
        }
        
        for tile in &adjacent_tiles {
            task_queue.enqueue(TileTask::new(*tile, Priority::Normal, tile.level == 0));
        }
        
        // Verify high priority tiles are dequeued first
        let first_task = task_queue.dequeue();
        assert!(first_task.is_some());
        assert_eq!(first_task.unwrap().priority, Priority::High);
    }

    /// Test 24.3: Zoom/pan → viewport update → tile requests
    /// 
    /// Validates: Requirements 12.1, 12.2, 12.3, 12.5, 13.1, 13.2
    #[test]
    fn test_zoom_pan_workflow() {
        let temp_file = create_test_dump(10_000_000);
        let file_loader = FileLoader::new(temp_file.path(), 2048, 128).unwrap();
        let metadata = file_loader.get_metadata().clone();
        
        // Create task queue and viewport manager
        let task_queue = Arc::new(TaskQueue::new());
        let mut viewport_manager = ViewportManager::new(metadata.clone(), task_queue.clone());
        
        // Initial viewport at level 0
        viewport_manager.update_viewport(0, 512.0, 384.0, 1024, 768);
        let initial_tiles = viewport_manager.get_visible_tiles();
        assert!(!initial_tiles.is_empty(), "Should have initial tiles");
        
        // Simulate zoom in by moving to a higher level (more detail)
        // In a real implementation, zoom controller would handle this
        viewport_manager.update_viewport(1, 512.0, 384.0, 1024, 768);
        let zoomed_tiles = viewport_manager.get_visible_tiles();
        assert!(!zoomed_tiles.is_empty(), "Should have tiles after zoom");
        
        // Simulate pan by changing center coordinates
        viewport_manager.update_viewport(1, 1024.0, 768.0, 1024, 768);
        let panned_tiles = viewport_manager.get_visible_tiles();
        assert!(!panned_tiles.is_empty(), "Should have tiles after pan");
        
        // Verify tiles changed after pan
        assert_ne!(zoomed_tiles, panned_tiles, "Tiles should change after pan");
    }

    /// Test 24.4: Cache hit → tile display
    /// 
    /// Validates: Requirements 8.3, 8.4
    #[test]
    fn test_cache_hit_workflow() {
        let temp_dir = TempDir::new().unwrap();
        let cache_dir = temp_dir.path().join(".cache");
        
        // Create cache manager
        let cache = Arc::new(CacheManager::new(&cache_dir, "test.bin".to_string()).unwrap());
        
        // Create a test tile
        let coord = TileCoord::new(0, 5, 10);
        let test_tile_data = b"qoif\x00\x00\x00\x10\x00\x00\x00\x10\x04\x01test_data".to_vec();
        
        // Save to cache
        cache.save_tile(&coord, &test_tile_data).unwrap();
        
        // Verify tile exists
        assert!(cache.tile_exists(&coord), "Tile should exist in cache");
        
        // Load from cache
        let loaded_data = cache.load_tile(&coord).unwrap();
        assert_eq!(loaded_data, test_tile_data, "Loaded data should match saved data");
    }

    /// Test 24.5: Cache miss → generation → caching
    /// 
    /// Validates: Requirements 6.3, 8.1, 8.2
    #[test]
    fn test_cache_miss_workflow() {
        let temp_dir = TempDir::new().unwrap();
        let cache_dir = temp_dir.path().join(".cache");
        let temp_file = create_test_dump(1_000_000);
        
        // Create cache manager
        let cache = Arc::new(CacheManager::new(&cache_dir, "test.bin".to_string()).unwrap());
        
        // Load file
        let mut file_loader = FileLoader::new(temp_file.path(), 2048, 64).unwrap();
        let metadata = file_loader.get_metadata().clone();
        
        let coord = TileCoord::new(0, 0, 0);
        
        // Verify tile doesn't exist yet
        assert!(!cache.tile_exists(&coord), "Tile should not exist initially");
        
        // Generate tile
        let tile_data = TileGenerator::generate_tile(coord, &metadata, &mut file_loader).unwrap();
        
        // Cache the tile
        cache.save_tile(&coord, &tile_data).unwrap();
        
        // Verify tile now exists
        assert!(cache.tile_exists(&coord), "Tile should exist after caching");
        
        // Load and verify
        let loaded_data = cache.load_tile(&coord).unwrap();
        assert_eq!(loaded_data, tile_data, "Cached tile should match generated tile");
    }

    /// Test 24.6: Multiple workers → concurrent generation
    /// 
    /// Validates: Requirements 10.1, 10.2, 10.3, 10.4, 10.5
    #[test]
    fn test_concurrent_tile_generation() {
        let temp_dir = TempDir::new().unwrap();
        let cache_dir = temp_dir.path().join(".cache");
        let temp_file = create_test_dump(10_000_000);
        
        // Create components
        let cache = Arc::new(CacheManager::new(&cache_dir, "test.bin".to_string()).unwrap());
        let file_loader = Arc::new(parking_lot::Mutex::new(
            FileLoader::new(temp_file.path(), 2048, 128).unwrap()
        ));
        let metadata = file_loader.lock().get_metadata().clone();
        let task_queue = Arc::new(TaskQueue::new());
        
        // Enqueue multiple tiles
        for i in 0..5 {
            task_queue.enqueue(TileTask::new(
                TileCoord::new(0, i, 0),
                Priority::Normal,
                true, // is_high_resolution
            ));
        }
        
        // Create worker pool
        let mut worker_pool = WorkerPool::new(
            (*task_queue).clone(),
            (*cache).clone(),
            file_loader.clone(),
            metadata.clone(),
        );
        
        // Start workers
        worker_pool.start(
            (*task_queue).clone(),
            (*cache).clone(),
            file_loader.clone(),
            metadata.clone(),
        );
        
        // Wait a bit for workers to process
        std::thread::sleep(std::time::Duration::from_secs(2));
        
        // Shutdown workers
        worker_pool.shutdown();
        
        // Verify some tiles were generated and cached
        let mut cached_count = 0;
        for i in 0..5 {
            if cache.tile_exists(&TileCoord::new(0, i, 0)) {
                cached_count += 1;
            }
        }
        
        assert!(cached_count > 0, "At least some tiles should be cached");
    }

    /// Test 24.7: End-to-end workflow
    /// 
    /// Validates: Multiple requirements across all components
    #[test]
    fn test_end_to_end_workflow() {
        let temp_dir = TempDir::new().unwrap();
        let cache_dir = temp_dir.path().join(".cache");
        let temp_file = create_test_dump(5_000_000);
        
        // Step 1: Load file with metadata
        let file_loader = Arc::new(parking_lot::Mutex::new(
            FileLoader::new(temp_file.path(), 2048, 128).unwrap()
        ));
        let metadata = file_loader.lock().get_metadata().clone();
        
        // Step 2: Create cache
        let cache = Arc::new(CacheManager::new(&cache_dir, "test.bin".to_string()).unwrap());
        
        // Step 3: Create task queue
        let task_queue = Arc::new(TaskQueue::new());
        
        // Step 4: Create viewport manager
        let mut viewport_manager = ViewportManager::new(metadata.clone(), task_queue.clone());
        viewport_manager.update_viewport(0, 512.0, 384.0, 1024, 768);
        
        // Step 5: Enqueue visible tiles
        let visible_tiles = viewport_manager.get_visible_tiles();
        for tile in visible_tiles {
            task_queue.enqueue(TileTask::new(tile, Priority::High, tile.level == 0));
        }
        
        // Step 6: Create and start worker pool
        let mut worker_pool = WorkerPool::new(
            (*task_queue).clone(),
            (*cache).clone(),
            file_loader.clone(),
            metadata.clone(),
        );
        worker_pool.start(
            (*task_queue).clone(),
            (*cache).clone(),
            file_loader.clone(),
            metadata.clone(),
        );
        
        // Step 7: Wait for processing
        std::thread::sleep(std::time::Duration::from_secs(2));
        
        // Step 8: Verify tiles were generated
        let mut generated_count = 0;
        for tile in viewport_manager.get_visible_tiles() {
            if cache.tile_exists(&tile) {
                generated_count += 1;
            }
        }
        
        // Step 9: Cleanup
        worker_pool.shutdown();
        
        // Verify workflow succeeded
        assert!(generated_count > 0, "End-to-end workflow should generate tiles");
    }
}
