//! NAND Flash Viewer - Main entry point

use nand_flash_viewer::{
    logging, AppWindow, CacheManager, FileDialog, FileLoader, TaskQueue, WorkerPool,
};
use parking_lot::Mutex;
use std::sync::Arc;
use std::env;
use std::io::{self, Write};

fn main() {
    // Initialize logging
    logging::init();

    log::info!("NAND Flash Viewer starting");
    
    // Initialize FLTK application
    let app = fltk::app::App::default();

    // Get file path from command line or prompt user
    let file_path = if let Some(path) = env::args().nth(1) {
        path
    } else {
        // Prompt user for file path
        print!("Enter path to NAND dump file: ");
        io::stdout().flush().unwrap();
        
        let mut input = String::new();
        io::stdin().read_line(&mut input).unwrap();
        input.trim().to_string()
    };

    if file_path.is_empty() {
        eprintln!("Error: No file path provided");
        eprintln!("Usage: nand-flash-viewer <path-to-dump-file>");
        std::process::exit(1);
    }

    // Create file dialog to open a dump file
    let cache_dir = ".cache";
    let file_dialog = FileDialog::new(cache_dir);

    // Open the file
    match file_dialog.open_file(&file_path) {
        Ok(metadata) => {
            log::info!(
                "Opened file: {}, size: {} bytes, page_length: {}, block_size: {}",
                metadata.path,
                metadata.size,
                metadata.page_length,
                metadata.block_size
            );
            
            // Calculate and log grid dimensions
            log::info!(
                "Grid dimensions: {} columns x {} rows",
                metadata.grid_width,
                metadata.grid_height
            );
            
            // Calculate total visualization size in pixels
            // Width: page_length bytes × 8 pixels/byte × grid_width
            let bytes_per_block_width = metadata.page_length * 8; // 8 pixels per byte
            let total_width_pixels = (metadata.grid_width as u64) * (bytes_per_block_width as u64);
            
            // Height: block_size pages × grid_height (each page is 1 pixel tall)
            let bytes_per_block_height = metadata.block_size; // Each page is 1 pixel tall
            let total_height_pixels = (metadata.grid_height as u64) * (bytes_per_block_height as u64);
            
            log::info!(
                "Total visualization size: {} x {} pixels ({:.2} megapixels)",
                total_width_pixels,
                total_height_pixels,
                (total_width_pixels * total_height_pixels) as f64 / 1_000_000.0
            );
            
            // Calculate and log aspect ratio
            let aspect_ratio = total_width_pixels as f64 / total_height_pixels as f64;
            log::info!(
                "Aspect ratio: {:.2}:1 (target: 1.33:1 for 4:3)",
                aspect_ratio
            );
            
            log::info!(
                "Block size in pixels: {} x {} pixels per block",
                bytes_per_block_width,
                bytes_per_block_height
            );
            
            // Calculate pyramid level 0 tile dimensions
            const TILE_SIZE: u64 = 256;
            
            log::info!(
                "Tile size: {} x {} pixels",
                TILE_SIZE,
                TILE_SIZE
            );
            
            let tiles_wide = (total_width_pixels + TILE_SIZE - 1) / TILE_SIZE;
            let tiles_tall = (total_height_pixels + TILE_SIZE - 1) / TILE_SIZE;
            let total_tiles = tiles_wide * tiles_tall;
            
            log::info!(
                "Pyramid level 0: {} x {} tiles ({} total), size: {} x {} pixels",
                tiles_wide,
                tiles_tall,
                total_tiles,
                total_width_pixels,
                total_height_pixels
            );
            
            // Calculate and display all pyramid levels
            let mut level = 1;
            let mut level_width = total_width_pixels;
            let mut level_height = total_height_pixels;
            
            while level_width > TILE_SIZE || level_height > TILE_SIZE {
                // Each level is half the size of the previous level
                level_width = (level_width + 1) / 2;
                level_height = (level_height + 1) / 2;
                
                let level_tiles_wide = (level_width + TILE_SIZE - 1) / TILE_SIZE;
                let level_tiles_tall = (level_height + TILE_SIZE - 1) / TILE_SIZE;
                let level_total_tiles = level_tiles_wide * level_tiles_tall;
                
                log::info!(
                    "Pyramid level {}: {} x {} tiles ({} total), size: {} x {} pixels",
                    level,
                    level_tiles_wide,
                    level_tiles_tall,
                    level_total_tiles,
                    level_width,
                    level_height
                );
                
                level += 1;
            }
            
            log::info!("Total pyramid levels: {}", level);

            // Open file loader
            match FileLoader::new(&metadata.path, metadata.page_length, metadata.block_size) {
                Ok(file_loader) => {
                    // Create task queue
                    let task_queue = TaskQueue::new();
                    
                    // Initialize low-priority iterator for background tile generation
                    // This generates tiles bottom-up through the pyramid without queue explosion
                    task_queue.init_low_priority_iterator(metadata.clone());

                    // Extract filename from path for cache directory
                    let filename = std::path::Path::new(&metadata.path)
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("dump")
                        .to_string();

                    // Create cache manager
                    let cache = match CacheManager::new(cache_dir, filename) {
                        Ok(c) => Arc::new(c),
                        Err(e) => {
                            log::error!("Failed to create cache manager: {}", e);
                            return;
                        }
                    };

                    // Create worker pool
                    let file_loader_arc = Arc::new(Mutex::new(file_loader));
                    let mut worker_pool = WorkerPool::new(
                        task_queue.clone(),
                        (*cache).clone(),
                        file_loader_arc.clone(),
                        metadata.clone(),
                    );
                    
                    // Get tile completion receiver before starting workers
                    let tile_rx = worker_pool.take_tile_receiver();

                    // Start workers
                    worker_pool.start(
                        task_queue.clone(),
                        (*cache).clone(),
                        Arc::new(Mutex::new(FileLoader::new(
                            &metadata.path,
                            metadata.page_length,
                            metadata.block_size,
                        ).unwrap())),
                        metadata.clone(),
                    );

                    log::info!("Worker pool started with {} workers", worker_pool.num_workers());

                    // Create main application window
                    let _app_window = AppWindow::new(metadata.clone(), Arc::new(task_queue.clone()), cache.clone(), tile_rx);

                    log::info!("NAND Flash Viewer initialized and ready");
                    
                    // Run FLTK event loop to keep window open
                    app.run().unwrap();
                    
                    // Cleanup: shutdown worker pool when window closes
                    worker_pool.shutdown();
                }
                Err(e) => {
                    log::error!("Failed to open file: {}", e);
                }
            }
        }
        Err(e) => {
            log::error!("Failed to open file dialog: {}", e);
        }
    }
}



