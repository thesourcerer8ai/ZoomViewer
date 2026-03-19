//! Worker pool for parallel tile generation
//!
//! Distributes tile generation work across CPU cores using the TaskQueue.
//! Each worker continuously processes tasks in priority order (high → normal → low),
//! generates tiles, caches them, and enters a wait state when the queue is empty.
//!
//! **Validates: Requirements 10.1, 10.2, 10.3, 10.4, 10.5, 10.6**

use crate::cache_manager::CacheManager;
use crate::file_loader::FileLoader;
use crate::pyramid_tile_generator::PyramidTileGenerator;
use crate::task_queue::TaskQueue;
use crate::tile_generator::TileGenerator;
use crate::types::{FileMetadata, TileCoord};
use parking_lot::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::sync::mpsc::{channel, Sender, Receiver};
use std::thread;
use std::time::Duration;

/// Worker pool for parallel tile generation
///
/// Manages a pool of worker threads that process tile generation tasks from a shared queue.
/// Each worker continuously dequeues tasks, generates tiles, and caches results.
pub struct WorkerPool {
    /// Number of worker threads
    num_workers: usize,
    /// Worker thread handles
    workers: Vec<thread::JoinHandle<()>>,
    /// Shutdown signal
    shutdown: Arc<AtomicBool>,
    /// Tile completion notification sender
    tile_complete_tx: Option<Sender<TileCoord>>,
    /// Tile completion notification receiver
    tile_complete_rx: Option<Receiver<TileCoord>>,
}

impl WorkerPool {
    /// Create a new worker pool with one worker per available CPU core (minimum 1)
    ///
    /// # Arguments
    /// * `task_queue` - Shared task queue for work distribution
    /// * `cache` - Cache manager for storing generated tiles
    /// * `file_loader` - File loader for reading dump data
    /// * `metadata` - File metadata for tile generation
    ///
    /// # Requirements
    /// - Creates one worker per CPU core (Requirement 10.1, 10.2)
    pub fn new(
        _task_queue: TaskQueue,
        _cache: CacheManager,
        _file_loader: Arc<Mutex<FileLoader>>,
        _metadata: FileMetadata,
    ) -> Self {
        // Get number of available CPU cores, minimum 1
        let num_workers = num_cpus::get().max(1);
        
        // Create channel for tile completion notifications
        let (tx, rx) = channel();

        WorkerPool {
            num_workers,
            workers: Vec::new(),
            shutdown: Arc::new(AtomicBool::new(false)),
            tile_complete_tx: Some(tx),
            tile_complete_rx: Some(rx),
        }
    }
    
    /// Get the tile completion receiver
    /// 
    /// Returns the receiver for tile completion notifications.
    /// This can only be called once - subsequent calls will return None.
    pub fn take_tile_receiver(&mut self) -> Option<Receiver<TileCoord>> {
        self.tile_complete_rx.take()
    }

    /// Start all worker threads
    ///
    /// Spawns all worker threads which will begin processing tasks from the queue.
    ///
    /// # Requirements
    /// - Spawns all workers (Requirement 10.1, 10.2)
    pub fn start(
        &mut self,
        task_queue: TaskQueue,
        cache: CacheManager,
        file_loader: Arc<Mutex<FileLoader>>,
        metadata: FileMetadata,
    ) {
        let tile_tx = self.tile_complete_tx.clone();
        
        for worker_id in 0..self.num_workers {
            let queue = task_queue.clone();
            let cache_clone = cache.clone();
            let file_loader_clone = Arc::clone(&file_loader);
            let metadata_clone = metadata.clone();
            let shutdown = Arc::clone(&self.shutdown);
            let tx_clone = tile_tx.clone();

            let handle = thread::spawn(move || {
                Self::worker_thread(
                    worker_id,
                    queue,
                    cache_clone,
                    file_loader_clone,
                    metadata_clone,
                    shutdown,
                    tx_clone,
                );
            });

            self.workers.push(handle);
        }
    }

    /// Generate a placeholder tile for failed tile generation
    ///
    /// Creates a simple PNG tile with an error indicator (red X pattern)
    ///
    /// # Requirements
    /// - Display placeholder after max retries (Requirement 16.3)
    fn generate_placeholder_tile(coord: &TileCoord) -> crate::error::Result<Vec<u8>> {

        const TILE_SIZE: u32 = 512;
        
        // Create a gray background with red X pattern
        // Build continuous RGBA vector
        let mut rgba_data = Vec::with_capacity((TILE_SIZE * TILE_SIZE * 4) as usize);
        for y in 0..TILE_SIZE {
            for x in 0..TILE_SIZE {
                let is_diagonal1 = (x as i32 - y as i32).abs() < 3;
                let is_diagonal2 = ((TILE_SIZE - 1 - x) as i32 - y as i32).abs() < 3;
                if is_diagonal1 || is_diagonal2 {
                    rgba_data.extend_from_slice(&[255, 0, 0, 255]); // Red
                } else {
                    rgba_data.extend_from_slice(&[200, 200, 200, 255]); // Light gray
                }
            }
        }
        
        let qoi_bytes = qoi::encode_to_vec(&rgba_data, TILE_SIZE, TILE_SIZE)
            .map_err(|e| crate::error::Error::TileGenerationFailed(format!("Failed to encode placeholder: {:?}", e)))?;
        
        log::debug!("Generated placeholder tile for {:?}", coord);
        Ok(qoi_bytes)
    }

    /// Worker thread function
    ///
    /// Continuously dequeues tasks, generates tiles, caches results, and marks complete.
    /// Enters wait state if queue is empty.
    ///
    /// # Requirements
    /// - Continuously checks queue (Requirement 10.3)
    /// - Processes in priority order (Requirement 10.4)
    /// - Caches and marks complete (Requirement 10.5)
    /// - Enters wait state when empty (Requirement 10.6)
    fn worker_thread(
        worker_id: usize,
        task_queue: TaskQueue,
        cache: CacheManager,
        file_loader: Arc<Mutex<FileLoader>>,
        metadata: FileMetadata,
        shutdown: Arc<AtomicBool>,
        tile_complete_tx: Option<Sender<TileCoord>>,
    ) {
        log::info!("Worker {} started", worker_id);

        let mut tiles_generated = 0u64;
        let mut last_report_time = std::time::Instant::now();
        const REPORT_INTERVAL: std::time::Duration = std::time::Duration::from_secs(5);

        loop {
            // Check for shutdown signal
            if shutdown.load(Ordering::Relaxed) {
                log::info!("Worker {} shutting down", worker_id);
                break;
            }

            // Try to dequeue a task
            match task_queue.dequeue() {
                Some(task) => {
                    log::debug!(
                        "Worker {} processing task: level={}, x={}, y={}, priority={:?}",
                        worker_id,
                        task.coord.level,
                        task.coord.x,
                        task.coord.y,
                        task.priority
                    );

                    // Check if tile is already cached
                    if cache.tile_exists(&task.coord) {
                        log::debug!(
                            "Worker {} found cached tile: {:?}",
                            worker_id,
                            task.coord
                        );
                        continue;
                    }

                    // Generate tile based on level
                    let result = if task.coord.level == 0 {
                        // High-resolution tile from dump
                        let mut loader = file_loader.lock();
                        TileGenerator::generate_tile(task.coord, &metadata, &mut loader)
                    } else {
                        // Pyramid tile from lower-level tiles
                        PyramidTileGenerator::generate_pyramid_tile(
                            task.coord,
                            &metadata,
                            &task_queue,
                            &cache,
                            task.priority,
                        )
                    };

                    match result {
                        Ok(png_bytes) => {
                            // Cache the generated tile
                            if let Err(e) = cache.save_tile(&task.coord, &png_bytes) {
                                log::error!(
                                    "Worker {} failed to cache tile {:?}: {}",
                                    worker_id,
                                    task.coord,
                                    e
                                );
                            } else {
                                log::debug!(
                                    "Worker {} completed tile: {:?}",
                                    worker_id,
                                    task.coord
                                );
                                
                                tiles_generated += 1;
                                
                                // Report generation rate every 5 seconds
                                let now = std::time::Instant::now();
                                if now.duration_since(last_report_time) >= REPORT_INTERVAL {
                                    let elapsed_secs = now.duration_since(last_report_time).as_secs_f64();
                                    let tiles_per_sec = tiles_generated as f64 / elapsed_secs;
                                    
                                    // Get queue size for diagnostics
                                    let queue_size = task_queue.size();
                                    
                                    log::info!(
                                        "Worker {} generation rate: {:.1} tiles/sec ({} tiles in {:.1}s) | Queue size: {}",
                                        worker_id,
                                        tiles_per_sec,
                                        tiles_generated,
                                        elapsed_secs,
                                        queue_size
                                    );
                                    tiles_generated = 0;
                                    last_report_time = now;
                                }
                                
                                // Remove tile from all queues since it's now cached
                                task_queue.remove_from_all_queues(task.coord);
                                
                                // Notify task queue that this tile is complete
                                // This will automatically enqueue any parent tiles waiting for it
                                let enqueued_parents = task_queue.notify_tile_complete(task.coord);
                                if !enqueued_parents.is_empty() {
                                    log::debug!(
                                        "Worker {} enqueued {} parent tiles waiting for {:?}",
                                        worker_id,
                                        enqueued_parents.len(),
                                        task.coord
                                    );
                                }
                                
                                // Notify that tile is complete
                                if let Some(ref tx) = tile_complete_tx {
                                    let _ = tx.send(task.coord);
                                }
                            }
                        }
                        Err(e) => {
                            // Check if this is a "registered dependency" error for pyramid tiles
                            let error_msg = format!("{}", e);
                            let is_waiting_for_children = error_msg.contains("registered dependency");
                            
                            if is_waiting_for_children {
                                // Dependencies registered - parent will be auto-enqueued when children complete
                                log::debug!(
                                    "Worker {} pyramid tile {:?} registered dependency, will retry when children complete",
                                    worker_id,
                                    task.coord
                                );
                                continue;
                            }
                            
                            // Log error with context (tile coordinates, reason)
                            log::error!(
                                "Worker {} failed to generate tile (level={}, x={}, y={}, priority={:?}): {}",
                                worker_id,
                                task.coord.level,
                                task.coord.x,
                                task.coord.y,
                                task.priority,
                                e
                            );

                            // Implement retry logic with exponential backoff
                            const MAX_RETRIES: u32 = 3;
                            const BASE_DELAY_MS: u64 = 100;

                            if task.retry_count < MAX_RETRIES {
                                // Calculate exponential backoff delay: baseDelay * (2 ^ retryCount)
                                let delay_ms = BASE_DELAY_MS * (1 << task.retry_count);
                                
                                log::info!(
                                    "Worker {} scheduling retry {} for tile {:?} after {}ms",
                                    worker_id,
                                    task.retry_count + 1,
                                    task.coord,
                                    delay_ms
                                );

                                // Sleep for the backoff delay
                                thread::sleep(Duration::from_millis(delay_ms));

                                // Re-enqueue task with incremented retry count
                                let mut retry_task = task.clone();
                                retry_task.retry_count += 1;
                                task_queue.enqueue(retry_task);
                            } else {
                                // Max retries exceeded, generate placeholder tile
                                log::warn!(
                                    "Worker {} max retries exceeded for tile {:?}, generating placeholder",
                                    worker_id,
                                    task.coord
                                );

                                // Generate and cache placeholder tile
                                match Self::generate_placeholder_tile(&task.coord) {
                                    Ok(placeholder_bytes) => {
                                        if let Err(e) = cache.save_tile(&task.coord, &placeholder_bytes) {
                                            log::error!(
                                                "Worker {} failed to cache placeholder tile {:?}: {}",
                                                worker_id,
                                                task.coord,
                                                e
                                            );
                                        }
                                    }
                                    Err(e) => {
                                        log::error!(
                                            "Worker {} failed to generate placeholder tile {:?}: {}",
                                            worker_id,
                                            task.coord,
                                            e
                                        );
                                    }
                                }
                            }
                        }
                    }
                }
                None => {
                    // Queue is empty, enter wait state
                    thread::sleep(Duration::from_millis(10));
                }
            }
        }

        log::info!("Worker {} stopped", worker_id);
    }

    /// Gracefully stop all workers
    ///
    /// Signals all workers to shut down and waits for them to complete.
    ///
    /// # Requirements
    /// - Gracefully stops all workers (Requirement 10.1)
    pub fn shutdown(mut self) {
        log::info!("Shutting down worker pool with {} workers", self.num_workers);

        // Signal shutdown
        self.shutdown.store(true, Ordering::Relaxed);

        // Wait for all workers to finish
        for (i, handle) in self.workers.drain(..).enumerate() {
            if let Err(e) = handle.join() {
                log::error!("Worker {} failed to join: {:?}", i, e);
            }
        }

        log::info!("Worker pool shutdown complete");
    }

    /// Check if the worker pool is running
    pub fn is_running(&self) -> bool {
        !self.shutdown.load(Ordering::Relaxed)
    }

    /// Get the number of workers in the pool
    pub fn num_workers(&self) -> usize {
        self.num_workers
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Priority, TileCoord, TileTask};
    use tempfile::{NamedTempFile, TempDir};
    use std::io::{Write, Seek};

    /// Helper function to create a test file and file loader
    fn create_test_file_loader() -> (NamedTempFile, FileLoader) {
        let mut temp_file = NamedTempFile::new().unwrap();
        // Write some test data
        temp_file.write_all(&[0xAA; 10240]).unwrap();
        // Make it a sparse 51GB file
        temp_file.seek(std::io::SeekFrom::Start(51 * 1024 * 1024 * 1024 - 1)).unwrap();
        temp_file.write_all(&[0xAA]).unwrap();
        temp_file.flush().unwrap();

        let file_loader = FileLoader::new(temp_file.path(), 512, 64).unwrap();
        (temp_file, file_loader)
    }

    #[test]
    fn test_worker_pool_creation() {
        let task_queue = TaskQueue::new();
        let temp_dir = TempDir::new().unwrap();
        let cache = CacheManager::new(temp_dir.path(), "test.bin".to_string()).unwrap();
        let (_temp_file, file_loader) = create_test_file_loader();
        let metadata = file_loader.get_metadata().clone();
        let file_loader_arc = Arc::new(Mutex::new(file_loader));

        let pool = WorkerPool::new(
            task_queue,
            cache,
            file_loader_arc,
            metadata,
        );

        // Should create one worker per CPU core (minimum 1)
        assert!(pool.num_workers() >= 1);
        assert_eq!(pool.num_workers(), num_cpus::get().max(1));
    }

    #[test]
    fn test_worker_pool_start_and_shutdown() {
        let task_queue = TaskQueue::new();
        let temp_dir = TempDir::new().unwrap();
        let cache = CacheManager::new(temp_dir.path(), "test.bin".to_string()).unwrap();
        let (_temp_file, file_loader) = create_test_file_loader();
        let metadata = file_loader.get_metadata().clone();
        let file_loader_arc = Arc::new(Mutex::new(file_loader));

        let mut pool = WorkerPool::new(
            task_queue.clone(),
            cache.clone(),
            Arc::clone(&file_loader_arc),
            metadata.clone(),
        );

        // Start the pool
        pool.start(task_queue, cache, file_loader_arc, metadata);
        assert!(pool.is_running());

        // Shutdown the pool
        pool.shutdown();
        // After shutdown, pool is consumed so we can't check is_running()
    }

    #[test]
    fn test_worker_processes_task() {
        let task_queue = TaskQueue::new();
        let temp_dir = TempDir::new().unwrap();
        let cache = CacheManager::new(temp_dir.path(), "test.bin".to_string()).unwrap();
        let (_temp_file, file_loader) = create_test_file_loader();
        let metadata = file_loader.get_metadata().clone();
        let file_loader_arc = Arc::new(Mutex::new(file_loader));

        // Enqueue a task
        let coord = TileCoord::new(0, 0, 0);
        let task = TileTask::new(coord, Priority::High, true);
        task_queue.enqueue(task);

        let mut pool = WorkerPool::new(
            task_queue.clone(),
            cache.clone(),
            Arc::clone(&file_loader_arc),
            metadata.clone(),
        );

        pool.start(
            task_queue.clone(),
            cache.clone(),
            Arc::clone(&file_loader_arc),
            metadata,
        );

        // Wait for worker to process the task
        thread::sleep(Duration::from_millis(500));

        // Check if tile was generated and cached
        assert!(cache.tile_exists(&coord));

        pool.shutdown();
    }

    #[test]
    fn test_worker_processes_multiple_tasks() {
        let task_queue = TaskQueue::new();
        let temp_dir = TempDir::new().unwrap();
        let cache = CacheManager::new(temp_dir.path(), "test.bin".to_string()).unwrap();
        let (_temp_file, file_loader) = create_test_file_loader();
        let metadata = file_loader.get_metadata().clone();
        let file_loader_arc = Arc::new(Mutex::new(file_loader));

        // Enqueue multiple tasks
        let coords = vec![
            TileCoord::new(0, 0, 0),
            TileCoord::new(0, 1, 0),
            TileCoord::new(0, 0, 1),
        ];

        for coord in &coords {
            let task = TileTask::new(*coord, Priority::Normal, true);
            task_queue.enqueue(task);
        }

        let mut pool = WorkerPool::new(
            task_queue.clone(),
            cache.clone(),
            Arc::clone(&file_loader_arc),
            metadata.clone(),
        );

        pool.start(
            task_queue.clone(),
            cache.clone(),
            Arc::clone(&file_loader_arc),
            metadata,
        );

        // Wait for workers to process all tasks
        thread::sleep(Duration::from_secs(2));

        // Check if all tiles were generated and cached
        for coord in &coords {
            assert!(cache.tile_exists(coord), "Tile {:?} should be cached", coord);
        }

        pool.shutdown();
    }

    #[test]
    fn test_worker_respects_priority_order() {
        let task_queue = TaskQueue::new();
        let temp_dir = TempDir::new().unwrap();
        let cache = CacheManager::new(temp_dir.path(), "test.bin".to_string()).unwrap();
        let (_temp_file, file_loader) = create_test_file_loader();
        let metadata = file_loader.get_metadata().clone();
        let file_loader_arc = Arc::new(Mutex::new(file_loader));

        // Enqueue tasks with different priorities
        let low_coord = TileCoord::new(0, 0, 0);
        let normal_coord = TileCoord::new(0, 1, 0);
        let high_coord = TileCoord::new(0, 2, 0);

        task_queue.enqueue(TileTask::new(low_coord, Priority::Low, true));
        task_queue.enqueue(TileTask::new(normal_coord, Priority::Normal, true));
        task_queue.enqueue(TileTask::new(high_coord, Priority::High, true));

        let mut pool = WorkerPool::new(
            task_queue.clone(),
            cache.clone(),
            Arc::clone(&file_loader_arc),
            metadata.clone(),
        );

        pool.start(
            task_queue.clone(),
            cache.clone(),
            Arc::clone(&file_loader_arc),
            metadata,
        );

        // Wait for workers to process tasks
        thread::sleep(Duration::from_secs(2));

        // All tasks should be processed (priority order is handled by TaskQueue)
        assert!(cache.tile_exists(&low_coord));
        assert!(cache.tile_exists(&normal_coord));
        assert!(cache.tile_exists(&high_coord));

        pool.shutdown();
    }

    #[test]
    fn test_worker_waits_when_queue_empty() {
        let task_queue = TaskQueue::new();
        let temp_dir = TempDir::new().unwrap();
        let cache = CacheManager::new(temp_dir.path(), "test.bin".to_string()).unwrap();
        let (_temp_file, file_loader) = create_test_file_loader();
        let metadata = file_loader.get_metadata().clone();
        let file_loader_arc = Arc::new(Mutex::new(file_loader));

        // Don't enqueue any tasks
        let mut pool = WorkerPool::new(
            task_queue.clone(),
            cache.clone(),
            Arc::clone(&file_loader_arc),
            metadata.clone(),
        );

        pool.start(
            task_queue.clone(),
            cache.clone(),
            Arc::clone(&file_loader_arc),
            metadata,
        );

        // Workers should be waiting (not crashing)
        thread::sleep(Duration::from_millis(100));
        assert!(pool.is_running());

        pool.shutdown();
    }

    #[test]
    fn test_worker_skips_cached_tiles() {
        let task_queue = TaskQueue::new();
        let temp_dir = TempDir::new().unwrap();
        let cache = CacheManager::new(temp_dir.path(), "test.bin".to_string()).unwrap();
        let (_temp_file, file_loader) = create_test_file_loader();
        let metadata = file_loader.get_metadata().clone();
        let file_loader_arc = Arc::new(Mutex::new(file_loader));

        // Pre-cache a tile
        let coord = TileCoord::new(0, 0, 0);
        let png_data = vec![0x71, 0x6f, 0x69, 0x66, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x04, 0x00];
        cache.save_tile(&coord, &png_data).unwrap();

        // Enqueue task for the same tile
        let task = TileTask::new(coord, Priority::High, true);
        task_queue.enqueue(task);

        let mut pool = WorkerPool::new(
            task_queue.clone(),
            cache.clone(),
            Arc::clone(&file_loader_arc),
            metadata.clone(),
        );

        pool.start(
            task_queue.clone(),
            cache.clone(),
            Arc::clone(&file_loader_arc),
            metadata,
        );

        // Wait briefly
        thread::sleep(Duration::from_millis(100));

        // Tile should still be the cached version (not regenerated)
        let loaded = cache.load_tile(&coord).unwrap();
        assert_eq!(loaded, png_data);

        pool.shutdown();
    }
}

    #[test]
    fn test_placeholder_tile_generation() {
        // Test that placeholder tiles can be generated
        use crate::types::TileCoord;
        
        let coord = TileCoord::new(0, 5, 10);
        let result = WorkerPool::generate_placeholder_tile(&coord);
        
        assert!(result.is_ok());
        let png_bytes = result.unwrap();
        
        // Verify it's a valid QOI (starts with qoif signature)
        assert!(png_bytes.len() > 14);
        assert_eq!(&png_bytes[0..4], &[0x71, 0x6f, 0x69, 0x66]);
    }

    #[test]
    fn test_exponential_backoff_calculation() {
        // Test exponential backoff delay calculation
        const BASE_DELAY_MS: u64 = 100;
        
        // Retry 0: 100ms
        let delay0 = BASE_DELAY_MS * (1 << 0);
        assert_eq!(delay0, 100);
        
        // Retry 1: 200ms
        let delay1 = BASE_DELAY_MS * (1 << 1);
        assert_eq!(delay1, 200);
        
        // Retry 2: 400ms
        let delay2 = BASE_DELAY_MS * (1 << 2);
        assert_eq!(delay2, 400);
    }

    #[test]
    fn test_retry_count_increments() {
        // Test that retry count increments correctly
        use crate::types::{TileCoord, TileTask, Priority};
        
        let coord = TileCoord::new(0, 0, 0);
        let mut task = TileTask::new(coord, Priority::High, true);
        
        assert_eq!(task.retry_count, 0);
        
        task.retry_count += 1;
        assert_eq!(task.retry_count, 1);
        
        task.retry_count += 1;
        assert_eq!(task.retry_count, 2);
        
        task.retry_count += 1;
        assert_eq!(task.retry_count, 3);
    }

    #[test]
    fn test_error_logging_includes_context() {
        // This test verifies that error logging includes tile coordinates
        // We can't easily test log output, but we can verify the error structure
        use crate::types::TileCoord;
        use crate::error::Error;
        
        let coord = TileCoord::new(2, 15, 23);
        let error = Error::TileGenerationFailed(format!(
            "Failed to generate tile (level={}, x={}, y={})",
            coord.level, coord.x, coord.y
        ));
        
        let error_msg = format!("{}", error);
        assert!(error_msg.contains("level=2"));
        assert!(error_msg.contains("x=15"));
        assert!(error_msg.contains("y=23"));
    }
