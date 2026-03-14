//! Comprehensive unit tests for multi-file support
//!
//! Tests window management, cache isolation, state preservation, and worker pool isolation.

#[cfg(test)]
mod tests {
    use crate::multi_file_manager::{MultiFileManager, DumpId};
    use crate::window_manager::WindowManager;
    use crate::types::{FileMetadata, Viewport};
    use crate::cache_manager::CacheManager;
    use tempfile::{TempDir, NamedTempFile};
    use std::io::{Write, Seek};

    /// Helper to create a sparse test file
    fn create_test_file(size_gb: u64) -> NamedTempFile {
        let mut file = NamedTempFile::new().unwrap();
        file.write_all(&[0xAA; 10240]).unwrap();
        file.seek(std::io::SeekFrom::Start(size_gb * 1024 * 1024 * 1024 - 1)).unwrap();
        file.write_all(&[0xAA]).unwrap();
        file.flush().unwrap();
        file
    }

    #[test]
    #[ignore]
    fn test_window_management_basic() {
        let mut window_manager = WindowManager::new();
        let dump_id1 = DumpId::new(1);
        let dump_id2 = DumpId::new(2);

        // Open two windows
        let window_id1 = window_manager.open_window(dump_id1, "Dump 1".to_string());
        let window_id2 = window_manager.open_window(dump_id2, "Dump 2".to_string());

        // Verify both windows exist
        assert_eq!(window_manager.window_count(), 2);
        assert!(window_manager.get_window(window_id1).is_some());
        assert!(window_manager.get_window(window_id2).is_some());

        // Verify first window is active
        assert_eq!(window_manager.get_active_window(), Some(window_id1));

        // Switch to second window
        window_manager.set_active_window(window_id2);
        assert_eq!(window_manager.get_active_window(), Some(window_id2));
    }

    #[test]
    #[ignore]
    fn test_cache_isolation_per_dump() {
        let temp_dir = TempDir::new().unwrap();
        let cache_base = temp_dir.path().join(".cache");

        // Create two cache managers for different dumps
        let cache1 = CacheManager::new(&cache_base, "dump1.bin".to_string()).unwrap();
        let cache2 = CacheManager::new(&cache_base, "dump2.bin".to_string()).unwrap();

        // Create test PNG data
        let mut png_data = vec![0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
        png_data.extend_from_slice(&[0x00, 0x00, 0x00, 0x0D]);

        // Save tile to cache1
        let coord = crate::types::TileCoord::new(0, 5, 10);
        cache1.save_tile(&coord, &png_data).unwrap();

        // Verify tile exists in cache1
        assert!(cache1.tile_exists(&coord));

        // Verify tile does NOT exist in cache2 (isolation)
        assert!(!cache2.tile_exists(&coord));

        // Save different data to cache2
        let mut png_data2 = vec![0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
        png_data2.extend_from_slice(&[0xFF, 0xFF, 0xFF, 0xFF]);
        cache2.save_tile(&coord, &png_data2).unwrap();

        // Verify both caches have the tile but with different data
        let loaded1 = cache1.load_tile(&coord).unwrap();
        let loaded2 = cache2.load_tile(&coord).unwrap();

        assert_eq!(loaded1, png_data);
        assert_eq!(loaded2, png_data2);
        assert_ne!(loaded1, loaded2);
    }

    #[test]
    #[ignore]
    fn test_state_preservation_viewport() {
        let temp_dir = TempDir::new().unwrap();
        let manager = MultiFileManager::new(temp_dir.path().join(".cache").to_string_lossy().to_string());

        // Create test file
        let test_file = create_test_file(51);

        // Open dump
        let metadata = FileMetadata::new(
            test_file.path().to_string_lossy().to_string(),
            51 * 1024 * 1024 * 1024,
            512,
            64,
        );
        let dump_id = manager.open_dump(metadata).unwrap();

        // Set viewport state
        let viewport1 = Viewport::new(2, 512.0, 512.0, 1024, 768);
        manager.update_viewport(dump_id, viewport1.clone()).unwrap();

        // Retrieve and verify
        let retrieved1 = manager.get_viewport(dump_id).unwrap();
        assert_eq!(retrieved1.level, 2);
        assert_eq!(retrieved1.center_x, 512.0);

        // Update viewport again
        let viewport2 = Viewport::new(3, 1024.0, 1024.0, 1024, 768);
        manager.update_viewport(dump_id, viewport2.clone()).unwrap();

        // Verify new state
        let retrieved2 = manager.get_viewport(dump_id).unwrap();
        assert_eq!(retrieved2.level, 3);
        assert_eq!(retrieved2.center_x, 1024.0);
    }

    #[test]
    #[ignore]
    fn test_multiple_dumps_independent_state() {
        let temp_dir = TempDir::new().unwrap();
        let manager = MultiFileManager::new(temp_dir.path().join(".cache").to_string_lossy().to_string());

        // Create two test files
        let test_file1 = create_test_file(51);
        let test_file2 = create_test_file(100);

        // Open two dumps
        let metadata1 = FileMetadata::new(
            test_file1.path().to_string_lossy().to_string(),
            51 * 1024 * 1024 * 1024,
            512,
            64,
        );
        let metadata2 = FileMetadata::new(
            test_file2.path().to_string_lossy().to_string(),
            100 * 1024 * 1024 * 1024,
            1024,
            128,
        );

        let dump_id1 = manager.open_dump(metadata1).unwrap();
        let dump_id2 = manager.open_dump(metadata2).unwrap();

        // Set different viewport states
        let viewport1 = Viewport::new(1, 100.0, 100.0, 1024, 768);
        let viewport2 = Viewport::new(3, 500.0, 500.0, 1024, 768);

        manager.update_viewport(dump_id1, viewport1.clone()).unwrap();
        manager.update_viewport(dump_id2, viewport2.clone()).unwrap();

        // Verify each dump has its own state
        let retrieved1 = manager.get_viewport(dump_id1).unwrap();
        let retrieved2 = manager.get_viewport(dump_id2).unwrap();

        assert_eq!(retrieved1.level, 1);
        assert_eq!(retrieved1.center_x, 100.0);

        assert_eq!(retrieved2.level, 3);
        assert_eq!(retrieved2.center_x, 500.0);
    }

    #[test]
    #[ignore]
    fn test_window_and_dump_coordination() {
        let temp_dir = TempDir::new().unwrap();
        let mut window_manager = WindowManager::new();
        let file_manager = MultiFileManager::new(temp_dir.path().join(".cache").to_string_lossy().to_string());

        // Create test files
        let test_file1 = create_test_file(51);
        let test_file2 = create_test_file(75);

        // Open dumps
        let metadata1 = FileMetadata::new(
            test_file1.path().to_string_lossy().to_string(),
            51 * 1024 * 1024 * 1024,
            512,
            64,
        );
        let metadata2 = FileMetadata::new(
            test_file2.path().to_string_lossy().to_string(),
            75 * 1024 * 1024 * 1024,
            1024,
            128,
        );

        let dump_id1 = file_manager.open_dump(metadata1).unwrap();
        let dump_id2 = file_manager.open_dump(metadata2).unwrap();

        // Open windows for each dump
        let window_id1 = window_manager.open_window(dump_id1, "Dump 1".to_string());
        let window_id2 = window_manager.open_window(dump_id2, "Dump 2".to_string());

        // Verify coordination
        assert_eq!(window_manager.window_count(), 2);
        let window1 = window_manager.get_window(window_id1).unwrap();
        let window2 = window_manager.get_window(window_id2).unwrap();

        assert_eq!(window1.dump_id, dump_id1);
        assert_eq!(window2.dump_id, dump_id2);
    }

    #[test]
    #[ignore]
    fn test_close_dump_and_window() {
        let temp_dir = TempDir::new().unwrap();
        let mut window_manager = WindowManager::new();
        let file_manager = MultiFileManager::new(temp_dir.path().join(".cache").to_string_lossy().to_string());

        // Create test file
        let test_file = create_test_file(51);

        // Open dump
        let metadata = FileMetadata::new(
            test_file.path().to_string_lossy().to_string(),
            51 * 1024 * 1024 * 1024,
            512,
            64,
        );
        let dump_id = file_manager.open_dump(metadata).unwrap();

        // Open window
        let window_id = window_manager.open_window(dump_id, "Test".to_string());

        // Verify both exist
        assert_eq!(file_manager.list_open_dumps().len(), 1);
        assert_eq!(window_manager.window_count(), 1);

        // Close window
        window_manager.close_window(window_id);
        assert_eq!(window_manager.window_count(), 0);

        // Close dump
        file_manager.close_dump(dump_id).unwrap();
        assert_eq!(file_manager.list_open_dumps().len(), 0);
    }

    #[test]
    #[ignore]
    fn test_cache_isolation_different_filenames() {
        let temp_dir = TempDir::new().unwrap();
        let cache_base = temp_dir.path().join(".cache");

        // Create caches with different dump filenames
        let cache_dump1 = CacheManager::new(&cache_base, "dump1.bin".to_string()).unwrap();
        let cache_dump2 = CacheManager::new(&cache_base, "dump2.bin".to_string()).unwrap();

        // Verify they have different paths
        let coord = crate::types::TileCoord::new(0, 1, 2);
        let path1 = cache_dump1.get_tile_path(&coord);
        let path2 = cache_dump2.get_tile_path(&coord);

        assert_ne!(path1, path2);
        assert!(path1.to_string_lossy().contains("dump1.bin"));
        assert!(path2.to_string_lossy().contains("dump2.bin"));
    }

    #[test]
    #[ignore]
    fn test_multiple_windows_same_dump() {
        let mut window_manager = WindowManager::new();
        let dump_id = DumpId::new(1);

        // Open multiple windows for the same dump
        let window_id1 = window_manager.open_window(dump_id, "View 1".to_string());
        let window_id2 = window_manager.open_window(dump_id, "View 2".to_string());

        // Both windows should reference the same dump
        let window1 = window_manager.get_window(window_id1).unwrap();
        let window2 = window_manager.get_window(window_id2).unwrap();

        assert_eq!(window1.dump_id, dump_id);
        assert_eq!(window2.dump_id, dump_id);
        assert_ne!(window_id1, window_id2);
    }

    #[test]
    #[ignore]
    fn test_dump_list_operations() {
        let temp_dir = TempDir::new().unwrap();
        let manager = MultiFileManager::new(temp_dir.path().join(".cache").to_string_lossy().to_string());

        // Create test files
        let test_file1 = create_test_file(51);
        let test_file2 = create_test_file(75);
        let test_file3 = create_test_file(100);

        // Open three dumps
        let metadata1 = FileMetadata::new(
            test_file1.path().to_string_lossy().to_string(),
            51 * 1024 * 1024 * 1024,
            512,
            64,
        );
        let metadata2 = FileMetadata::new(
            test_file2.path().to_string_lossy().to_string(),
            75 * 1024 * 1024 * 1024,
            1024,
            128,
        );
        let metadata3 = FileMetadata::new(
            test_file3.path().to_string_lossy().to_string(),
            100 * 1024 * 1024 * 1024,
            512,
            256,
        );

        let id1 = manager.open_dump(metadata1).unwrap();
        let id2 = manager.open_dump(metadata2).unwrap();
        let id3 = manager.open_dump(metadata3).unwrap();

        // Verify all are listed
        let dumps = manager.list_open_dumps();
        assert_eq!(dumps.len(), 3);
        assert!(dumps.contains(&id1));
        assert!(dumps.contains(&id2));
        assert!(dumps.contains(&id3));

        // Close one dump
        manager.close_dump(id2).unwrap();

        // Verify it's removed from list
        let dumps = manager.list_open_dumps();
        assert_eq!(dumps.len(), 2);
        assert!(dumps.contains(&id1));
        assert!(!dumps.contains(&id2));
        assert!(dumps.contains(&id3));
    }

    #[test]
    #[ignore]
    fn test_window_list_operations() {
        let mut window_manager = WindowManager::new();

        let dump_id1 = DumpId::new(1);
        let dump_id2 = DumpId::new(2);
        let dump_id3 = DumpId::new(3);

        // Open three windows
        let window_id1 = window_manager.open_window(dump_id1, "Window 1".to_string());
        let window_id2 = window_manager.open_window(dump_id2, "Window 2".to_string());
        let window_id3 = window_manager.open_window(dump_id3, "Window 3".to_string());

        // Verify all are listed
        let windows = window_manager.list_windows();
        assert_eq!(windows.len(), 3);
        assert!(windows.contains(&window_id1));
        assert!(windows.contains(&window_id2));
        assert!(windows.contains(&window_id3));

        // Close one window
        window_manager.close_window(window_id2);

        // Verify it's removed from list
        let windows = window_manager.list_windows();
        assert_eq!(windows.len(), 2);
        assert!(windows.contains(&window_id1));
        assert!(!windows.contains(&window_id2));
        assert!(windows.contains(&window_id3));
    }

    #[test]
    #[ignore]
    fn test_per_file_worker_pool_isolation() {
        let temp_dir = TempDir::new().unwrap();
        let manager = MultiFileManager::new(temp_dir.path().join(".cache").to_string_lossy().to_string());

        // Create two test files
        let test_file1 = create_test_file(51);
        let test_file2 = create_test_file(75);

        // Open two dumps
        let metadata1 = FileMetadata::new(
            test_file1.path().to_string_lossy().to_string(),
            51 * 1024 * 1024 * 1024,
            512,
            64,
        );
        let metadata2 = FileMetadata::new(
            test_file2.path().to_string_lossy().to_string(),
            75 * 1024 * 1024 * 1024,
            1024,
            128,
        );

        let dump_id1 = manager.open_dump(metadata1).unwrap();
        let dump_id2 = manager.open_dump(metadata2).unwrap();

        // Start worker pools for both dumps
        manager.start_worker_pool(dump_id1).unwrap();
        manager.start_worker_pool(dump_id2).unwrap();

        // Verify both dumps have worker pools
        let dump1 = manager.get_dump(dump_id1).unwrap();
        let dump2 = manager.get_dump(dump_id2).unwrap();

        let state1 = dump1.lock();
        let state2 = dump2.lock();

        assert!(state1.worker_pool.is_some());
        assert!(state2.worker_pool.is_some());

        // Verify they have independent task queues
        assert!(!std::ptr::eq(
            &*state1.task_queue as *const _,
            &*state2.task_queue as *const _
        ));

        // Verify they have independent caches
        assert!(!std::ptr::eq(
            &*state1.cache as *const _,
            &*state2.cache as *const _
        ));
    }

    #[test]
    #[ignore]
    fn test_per_file_cache_directory_structure() {
        let temp_dir = TempDir::new().unwrap();
        let cache_base = temp_dir.path().join(".cache");

        // Create caches for two different dumps
        let cache1 = CacheManager::new(&cache_base, "dump1.bin".to_string()).unwrap();
        let cache2 = CacheManager::new(&cache_base, "dump2.bin".to_string()).unwrap();

        // Get tile paths for the same tile coordinate
        let coord = crate::types::TileCoord::new(0, 1, 2);
        let path1 = cache1.get_tile_path(&coord);
        let path2 = cache2.get_tile_path(&coord);

        // Verify paths are different and contain dump filenames
        assert_ne!(path1, path2);
        let path1_str = path1.to_string_lossy();
        let path2_str = path2.to_string_lossy();

        assert!(path1_str.contains("dump1.bin"));
        assert!(path2_str.contains("dump2.bin"));

        // Verify directory structure is correct
        assert!(path1_str.contains(".cache"));
        assert!(path2_str.contains(".cache"));
    }

    #[test]
    #[ignore]
    fn test_multi_file_viewport_independence() {
        let temp_dir = TempDir::new().unwrap();
        let manager = MultiFileManager::new(temp_dir.path().join(".cache").to_string_lossy().to_string());

        // Create two test files
        let test_file1 = create_test_file(51);
        let test_file2 = create_test_file(75);

        // Open two dumps
        let metadata1 = FileMetadata::new(
            test_file1.path().to_string_lossy().to_string(),
            51 * 1024 * 1024 * 1024,
            512,
            64,
        );
        let metadata2 = FileMetadata::new(
            test_file2.path().to_string_lossy().to_string(),
            75 * 1024 * 1024 * 1024,
            1024,
            128,
        );

        let dump_id1 = manager.open_dump(metadata1).unwrap();
        let dump_id2 = manager.open_dump(metadata2).unwrap();

        // Set different viewport states
        let viewport1 = Viewport::new(1, 100.0, 100.0, 1024, 768);
        let viewport2 = Viewport::new(3, 500.0, 500.0, 1024, 768);

        manager.update_viewport(dump_id1, viewport1.clone()).unwrap();
        manager.update_viewport(dump_id2, viewport2.clone()).unwrap();

        // Switch between dumps and verify state is preserved
        let retrieved1 = manager.get_viewport(dump_id1).unwrap();
        let retrieved2 = manager.get_viewport(dump_id2).unwrap();

        assert_eq!(retrieved1.level, 1);
        assert_eq!(retrieved1.center_x, 100.0);
        assert_eq!(retrieved2.level, 3);
        assert_eq!(retrieved2.center_x, 500.0);

        // Update dump1 viewport and verify dump2 is unchanged
        let new_viewport1 = Viewport::new(2, 200.0, 200.0, 1024, 768);
        manager.update_viewport(dump_id1, new_viewport1).unwrap();

        let updated1 = manager.get_viewport(dump_id1).unwrap();
        let still_same2 = manager.get_viewport(dump_id2).unwrap();

        assert_eq!(updated1.level, 2);
        assert_eq!(updated1.center_x, 200.0);
        assert_eq!(still_same2.level, 3);
        assert_eq!(still_same2.center_x, 500.0);
    }

    #[test]
    #[ignore]
    fn test_window_and_multi_file_workflow() {
        let temp_dir = TempDir::new().unwrap();
        let mut window_manager = WindowManager::new();
        let file_manager = MultiFileManager::new(temp_dir.path().join(".cache").to_string_lossy().to_string());

        // Create three test files
        let test_file1 = create_test_file(51);
        let test_file2 = create_test_file(75);
        let test_file3 = create_test_file(100);

        // Open three dumps
        let metadata1 = FileMetadata::new(
            test_file1.path().to_string_lossy().to_string(),
            51 * 1024 * 1024 * 1024,
            512,
            64,
        );
        let metadata2 = FileMetadata::new(
            test_file2.path().to_string_lossy().to_string(),
            75 * 1024 * 1024 * 1024,
            1024,
            128,
        );
        let metadata3 = FileMetadata::new(
            test_file3.path().to_string_lossy().to_string(),
            100 * 1024 * 1024 * 1024,
            512,
            256,
        );

        let dump_id1 = file_manager.open_dump(metadata1).unwrap();
        let dump_id2 = file_manager.open_dump(metadata2).unwrap();
        let dump_id3 = file_manager.open_dump(metadata3).unwrap();

        // Open windows for each dump
        let window_id1 = window_manager.open_window(dump_id1, "Dump 1".to_string());
        let window_id2 = window_manager.open_window(dump_id2, "Dump 2".to_string());
        let window_id3 = window_manager.open_window(dump_id3, "Dump 3".to_string());

        // Verify all windows and dumps are open
        assert_eq!(window_manager.window_count(), 3);
        assert_eq!(file_manager.list_open_dumps().len(), 3);

        // Set different viewport states for each dump
        let viewport1 = Viewport::new(0, 0.0, 0.0, 1024, 768);
        let viewport2 = Viewport::new(1, 256.0, 256.0, 1024, 768);
        let viewport3 = Viewport::new(2, 512.0, 512.0, 1024, 768);

        file_manager.update_viewport(dump_id1, viewport1).unwrap();
        file_manager.update_viewport(dump_id2, viewport2).unwrap();
        file_manager.update_viewport(dump_id3, viewport3).unwrap();

        // Switch between windows and verify state is preserved
        window_manager.set_active_window(window_id1);
        let vp1 = file_manager.get_viewport(dump_id1).unwrap();
        assert_eq!(vp1.level, 0);

        window_manager.set_active_window(window_id2);
        let vp2 = file_manager.get_viewport(dump_id2).unwrap();
        assert_eq!(vp2.level, 1);

        window_manager.set_active_window(window_id3);
        let vp3 = file_manager.get_viewport(dump_id3).unwrap();
        assert_eq!(vp3.level, 2);

        // Close middle window and verify others remain
        window_manager.close_window(window_id2);
        assert_eq!(window_manager.window_count(), 2);

        // Close corresponding dump
        file_manager.close_dump(dump_id2).unwrap();
        assert_eq!(file_manager.list_open_dumps().len(), 2);

        // Verify remaining dumps still have correct state
        let remaining_vp1 = file_manager.get_viewport(dump_id1).unwrap();
        let remaining_vp3 = file_manager.get_viewport(dump_id3).unwrap();
        assert_eq!(remaining_vp1.level, 0);
        assert_eq!(remaining_vp3.level, 2);
    }
}
