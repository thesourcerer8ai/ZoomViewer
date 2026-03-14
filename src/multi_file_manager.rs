//! Multi-file support for managing multiple concurrent NAND dump files
//!
//! Coordinates multiple dump files with independent state, caching, and worker pools.
//! Each dump file has its own cache directory, worker pool, and viewport state.

use crate::cache_manager::CacheManager;
use crate::file_loader::FileLoader;
use crate::task_queue::TaskQueue;
use crate::types::{FileMetadata, Viewport};
use crate::worker_pool::WorkerPool;
use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::Arc;

/// Unique identifier for a dump file session
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DumpId(u64);

impl DumpId {
    /// Create a new dump ID
    pub fn new(id: u64) -> Self {
        DumpId(id)
    }
}

/// Per-file state for a single dump
pub struct DumpFileState {
    /// Unique identifier for this dump
    pub id: DumpId,
    /// File metadata
    pub metadata: FileMetadata,
    /// Cache manager for this dump
    pub cache: Arc<CacheManager>,
    /// Task queue for this dump
    pub task_queue: Arc<TaskQueue>,
    /// File loader for this dump
    pub file_loader: Arc<Mutex<FileLoader>>,
    /// Worker pool for this dump
    pub worker_pool: Option<WorkerPool>,
    /// Viewport state for this dump
    pub viewport: Viewport,
}

/// Multi-file manager coordinating multiple dump files
pub struct MultiFileManager {
    /// Map of dump IDs to their state
    dumps: Arc<Mutex<HashMap<DumpId, Arc<Mutex<DumpFileState>>>>>,
    /// Next dump ID to assign
    next_id: Arc<Mutex<u64>>,
    /// Base cache directory
    cache_dir: String,
}

impl MultiFileManager {
    /// Create a new multi-file manager
    ///
    /// # Arguments
    /// * `cache_dir` - Base cache directory for all dumps
    pub fn new(cache_dir: String) -> Self {
        MultiFileManager {
            dumps: Arc::new(Mutex::new(HashMap::new())),
            next_id: Arc::new(Mutex::new(1)),
            cache_dir,
        }
    }

    /// Open a new dump file and register it with the manager
    ///
    /// # Arguments
    /// * `metadata` - File metadata for the dump
    ///
    /// # Returns
    /// The DumpId for the newly opened dump
    pub fn open_dump(&self, metadata: FileMetadata) -> crate::error::Result<DumpId> {
        // Generate unique ID
        let mut next_id = self.next_id.lock();
        let id = DumpId::new(*next_id);
        *next_id += 1;
        drop(next_id);

        // Extract filename from path for cache isolation
        let filename = std::path::Path::new(&metadata.path)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("dump")
            .to_string();

        // Create per-file cache
        let cache = Arc::new(CacheManager::new(&self.cache_dir, filename)?);

        // Create per-file task queue
        let task_queue = Arc::new(TaskQueue::new());

        // Create per-file file loader
        let file_loader = Arc::new(Mutex::new(FileLoader::new(
            &metadata.path,
            metadata.page_length,
            metadata.block_size,
        )?));

        // Create initial viewport at upper left corner
        let viewport = Viewport::new(0, 0.0, 0.0, 1024, 768);

        // Create dump state
        let state = DumpFileState {
            id,
            metadata: metadata.clone(),
            cache,
            task_queue,
            file_loader,
            worker_pool: None,
            viewport,
        };

        // Register dump
        let mut dumps = self.dumps.lock();
        dumps.insert(id, Arc::new(Mutex::new(state)));

        log::info!("Opened dump file: id={:?}, path={}", id, metadata.path);

        Ok(id)
    }

    /// Get the state for a specific dump
    pub fn get_dump(&self, id: DumpId) -> crate::error::Result<Arc<Mutex<DumpFileState>>> {
        let dumps = self.dumps.lock();
        dumps
            .get(&id)
            .cloned()
            .ok_or_else(|| crate::error::Error::NotFound(format!("Dump {:?} not found", id)))
    }

    /// Close a dump file and clean up its resources
    pub fn close_dump(&self, id: DumpId) -> crate::error::Result<()> {
        let mut dumps = self.dumps.lock();
        if dumps.remove(&id).is_some() {
            log::info!("Closed dump file: id={:?}", id);
            Ok(())
        } else {
            Err(crate::error::Error::NotFound(format!("Dump {:?} not found", id)))
        }
    }

    /// Get list of all open dump IDs
    pub fn list_open_dumps(&self) -> Vec<DumpId> {
        let dumps = self.dumps.lock();
        dumps.keys().copied().collect()
    }

    /// Update viewport state for a dump
    pub fn update_viewport(&self, id: DumpId, viewport: Viewport) -> crate::error::Result<()> {
        let dump = self.get_dump(id)?;
        let mut state = dump.lock();
        state.viewport = viewport;
        Ok(())
    }

    /// Get viewport state for a dump
    pub fn get_viewport(&self, id: DumpId) -> crate::error::Result<Viewport> {
        let dump = self.get_dump(id)?;
        let state = dump.lock();
        Ok(state.viewport.clone())
    }

    /// Start the worker pool for a dump
    ///
    /// Creates and starts worker threads for the specified dump.
    /// Each dump has its own isolated worker pool.
    pub fn start_worker_pool(&self, id: DumpId) -> crate::error::Result<()> {
        let dump = self.get_dump(id)?;
        let mut state = dump.lock();

        // Create worker pool if not already created
        if state.worker_pool.is_none() {
            let mut pool = WorkerPool::new(
                (*state.task_queue).clone(),
                (*state.cache).clone(),
                Arc::clone(&state.file_loader),
                state.metadata.clone(),
            );

            // Start the worker pool
            pool.start(
                (*state.task_queue).clone(),
                (*state.cache).clone(),
                Arc::clone(&state.file_loader),
                state.metadata.clone(),
            );

            state.worker_pool = Some(pool);
            log::info!("Started worker pool for dump: id={:?}", id);
        }

        Ok(())
    }

    /// Shutdown the worker pool for a dump
    ///
    /// Gracefully shuts down the worker threads for the specified dump.
    pub fn shutdown_worker_pool(&self, id: DumpId) -> crate::error::Result<()> {
        let dump = self.get_dump(id)?;
        let mut state = dump.lock();

        if let Some(pool) = state.worker_pool.take() {
            pool.shutdown();
            log::info!("Shutdown worker pool for dump: id={:?}", id);
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_multi_file_manager_creation() {
        let manager = MultiFileManager::new(".cache".to_string());
        assert_eq!(manager.list_open_dumps().len(), 0);
    }

    #[test]
    fn test_open_dump() {
        use tempfile::NamedTempFile;
        use std::io::{Write, Seek};

        let temp_dir = TempDir::new().unwrap();
        let manager = MultiFileManager::new(temp_dir.path().join(".cache").to_string_lossy().to_string());

        // Create a sparse test file (51 GB)
        let mut test_file = NamedTempFile::new().unwrap();
        test_file.write_all(&[0xAA; 10240]).unwrap();
        test_file.seek(std::io::SeekFrom::Start(51 * 1024 * 1024 * 1024 - 1)).unwrap();
        test_file.write_all(&[0xAA]).unwrap();
        test_file.flush().unwrap();

        let metadata = FileMetadata::new(
            test_file.path().to_string_lossy().to_string(),
            51 * 1024 * 1024 * 1024,
            512,
            64,
        );
        let result = manager.open_dump(metadata);

        assert!(result.is_ok());
        let _id = result.unwrap();
        assert_eq!(manager.list_open_dumps().len(), 1);
    }

    #[test]
    fn test_close_dump() {
        use tempfile::NamedTempFile;
        use std::io::{Write, Seek};

        let temp_dir = TempDir::new().unwrap();
        let manager = MultiFileManager::new(temp_dir.path().join(".cache").to_string_lossy().to_string());

        // Create a sparse test file (51 GB)
        let mut test_file = NamedTempFile::new().unwrap();
        test_file.write_all(&[0xAA; 10240]).unwrap();
        test_file.seek(std::io::SeekFrom::Start(51 * 1024 * 1024 * 1024 - 1)).unwrap();
        test_file.write_all(&[0xAA]).unwrap();
        test_file.flush().unwrap();

        let metadata = FileMetadata::new(
            test_file.path().to_string_lossy().to_string(),
            51 * 1024 * 1024 * 1024,
            512,
            64,
        );
        let id = manager.open_dump(metadata).unwrap();

        assert_eq!(manager.list_open_dumps().len(), 1);

        let result = manager.close_dump(id);
        assert!(result.is_ok());
        assert_eq!(manager.list_open_dumps().len(), 0);
    }

    #[test]
    fn test_multiple_dumps() {
        use tempfile::NamedTempFile;
        use std::io::{Write, Seek};

        let temp_dir = TempDir::new().unwrap();
        let manager = MultiFileManager::new(temp_dir.path().join(".cache").to_string_lossy().to_string());

        // Create two sparse test files
        let mut test_file1 = NamedTempFile::new().unwrap();
        test_file1.write_all(&[0xAA; 10240]).unwrap();
        test_file1.seek(std::io::SeekFrom::Start(51 * 1024 * 1024 * 1024 - 1)).unwrap();
        test_file1.write_all(&[0xAA]).unwrap();
        test_file1.flush().unwrap();

        let mut test_file2 = NamedTempFile::new().unwrap();
        test_file2.write_all(&[0xBB; 10240]).unwrap();
        test_file2.seek(std::io::SeekFrom::Start(100 * 1024 * 1024 * 1024 - 1)).unwrap();
        test_file2.write_all(&[0xBB]).unwrap();
        test_file2.flush().unwrap();

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

        let id1 = manager.open_dump(metadata1).unwrap();
        let id2 = manager.open_dump(metadata2).unwrap();

        assert_eq!(manager.list_open_dumps().len(), 2);
        assert_ne!(id1, id2);
    }

    #[test]
    fn test_viewport_preservation() {
        use tempfile::NamedTempFile;
        use std::io::{Write, Seek};

        let temp_dir = TempDir::new().unwrap();
        let manager = MultiFileManager::new(temp_dir.path().join(".cache").to_string_lossy().to_string());

        // Create a sparse test file (51 GB)
        let mut test_file = NamedTempFile::new().unwrap();
        test_file.write_all(&[0xAA; 10240]).unwrap();
        test_file.seek(std::io::SeekFrom::Start(51 * 1024 * 1024 * 1024 - 1)).unwrap();
        test_file.write_all(&[0xAA]).unwrap();
        test_file.flush().unwrap();

        let metadata = FileMetadata::new(
            test_file.path().to_string_lossy().to_string(),
            51 * 1024 * 1024 * 1024,
            512,
            64,
        );
        let id = manager.open_dump(metadata).unwrap();

        // Update viewport
        let new_viewport = Viewport::new(2, 512.0, 512.0, 1024, 768);
        manager.update_viewport(id, new_viewport.clone()).unwrap();

        // Retrieve and verify
        let retrieved = manager.get_viewport(id).unwrap();
        assert_eq!(retrieved.level, 2);
        assert_eq!(retrieved.center_x, 512.0);
        assert_eq!(retrieved.center_y, 512.0);
    }
}
