//! NAND Flash Viewer - Main entry point

use nand_flash_viewer::{
    logging, AppWindow, CacheManager, DumpDataProvider, FileDataProvider, FileDialog, FileLoader,
    FileMetadata, TaskQueue, WorkerPool, WorkflowEditorState,
};
use parking_lot::Mutex;
use std::env;
use std::io::{self, Write};
use std::sync::Arc;

fn main() {
    // Initialize logging
    logging::init();

    log::info!("NAND Flash Viewer starting");

    // Initialize FLTK application
    let app = fltk::app::App::default();

    // Get file path from command line, or auto-load workflow.json if present, or prompt user
    let file_path = match env::args().nth(1).filter(|p| !p.trim().is_empty()) {
        Some(path) => path,
        None if std::path::Path::new("workflow.json").is_file() => {
            log::info!("No file path provided; found 'workflow.json' in current directory, loading automatically");
            println!("No file path provided; found 'workflow.json' in current directory, loading automatically.");
            "workflow.json".to_string()
        }
        None => {
            // Prompt user for file path
            print!("Enter path to NAND dump file or workflow (.json): ");
            io::stdout().flush().unwrap();

            let mut input = String::new();
            io::stdin().read_line(&mut input).unwrap();
            input.trim().to_string()
        }
    };

    if file_path.is_empty() {
        eprintln!("Error: No file path provided");
        eprintln!("Usage: nand-flash-viewer <path-to-dump-file-or-workflow.json>");
        std::process::exit(1);
    }

    let cache_dir = ".cache";

    // Detect if file is a workflow file (.json) or a raw dump
    let (metadata, workflow, provider_opt): (
        FileMetadata,
        WorkflowEditorState,
        Option<Arc<Mutex<dyn DumpDataProvider>>>,
    ) = if file_path.ends_with(".json") {
        log::info!("Loading workflow from file: {}", file_path);
        let mut wf = WorkflowEditorState::new(None);
        if let Err(e) = wf.load_from_file(&file_path) {
            log::error!("Failed to load workflow file '{}': {}", file_path, e);
            eprintln!("Failed to load workflow file: {}", e);
            std::process::exit(1);
        }
        let prov_opt = match wf.build_active_output_provider() {
            Ok(p) => Some(p),
            Err(e) => {
                log::warn!("Workflow has no valid active output provider: {} — NAND Viewer tab will be disabled until an OutputViewer node is added.", e);
                None
            }
        };
        // Use a placeholder metadata when no provider is available yet
        let meta = if let Some(ref p) = prov_opt {
            p.lock().get_metadata()
        } else {
            FileMetadata::default()
        };
        (meta, wf, prov_opt)
    } else {
        let file_dialog = FileDialog::new(cache_dir);
        let meta = match file_dialog.open_file(&file_path) {
            Ok(m) => m,
            Err(e) => {
                log::error!("Failed to open file: {}", e);
                eprintln!("Failed to open file: {}", e);
                std::process::exit(1);
            }
        };
        let wf = WorkflowEditorState::new_with_geometry(
            Some(&file_path),
            Some(meta.page_length),
            Some(meta.block_size),
        );
        let prov_opt: Option<Arc<Mutex<dyn DumpDataProvider>>> = match wf.build_active_output_provider() {
            Ok(p) => Some(p),
            Err(e) => {
                log::warn!("Workflow output error: {}. Falling back to raw file loader.", e);
                let loader = match FileLoader::new(&meta.path, meta.page_length, meta.block_size) {
                    Ok(l) => l,
                    Err(e) => {
                        log::error!("Failed to open file loader: {}", e);
                        eprintln!("Failed to open file loader: {}", e);
                        std::process::exit(1);
                    }
                };
                let p: Arc<Mutex<dyn DumpDataProvider>> =
                    Arc::new(Mutex::new(FileDataProvider::new(loader)));
                Some(p)
            }
        };
        let active_meta = if let Some(ref p) = prov_opt {
            p.lock().get_metadata()
        } else {
            meta
        };
        (active_meta, wf, prov_opt)
    };

    if provider_opt.is_some() {
        log::info!(
            "Data stream opened: {}, size: {} bytes, page_length: {}, block_size: {}",
            metadata.path,
            metadata.size,
            metadata.page_length,
            metadata.block_size
        );
    } else {
        log::info!("No active data stream opened yet (waiting for OutputViewer node in workflow)");
    }

    // Calculate grid dimensions
    let bytes_per_block_width = metadata.page_length * 8;
    let total_width_pixels = (metadata.grid_width as u64) * (bytes_per_block_width as u64);
    let bytes_per_block_height = metadata.block_size;
    let total_height_pixels = (metadata.grid_height as u64) * (bytes_per_block_height as u64);

    if provider_opt.is_some() {
        log::info!(
            "Total visualization size: {} x {} pixels ({:.2} megapixels)",
            total_width_pixels,
            total_height_pixels,
            (total_width_pixels * total_height_pixels) as f64 / 1_000_000.0
        );
    }

    // Create task queue
    let task_queue = TaskQueue::new();
    task_queue.init_low_priority_iterator(metadata.clone());

    // Identity for cache directory
    let cache_identity = provider_opt
        .as_ref()
        .map(|p| p.lock().cache_identity())
        .unwrap_or_else(|| "no_output".to_string());
    let cache = match CacheManager::new(cache_dir, cache_identity) {
        Ok(c) => Arc::new(c),
        Err(e) => {
            log::error!("Failed to create cache manager: {}", e);
            return;
        }
    };

    // Create and start worker pool if provider is available
    let (worker_pool, tile_rx) = if let Some(ref prov) = provider_opt {
        let mut wp = WorkerPool::new(
            task_queue.clone(),
            (*cache).clone(),
            prov.clone(),
            metadata.clone(),
        );
        let rx = wp.take_tile_receiver();
        wp.start(
            task_queue.clone(),
            (*cache).clone(),
            prov.clone(),
            metadata.clone(),
        );
        log::info!("Worker pool started with {} workers", wp.num_workers());
        (Some(wp), rx)
    } else {
        (None, None)
    };

    // Create main application window
    let _app_window = AppWindow::new(
        metadata.clone(),
        Arc::new(task_queue.clone()),
        cache.clone(),
        tile_rx,
        provider_opt.clone(),
        Some(workflow),
    );

    log::info!("NAND Flash Viewer initialized and ready");

    // Run FLTK event loop
    app.run().unwrap();

    // Cleanup
    if let Some(wp) = worker_pool {
        wp.shutdown();
    }
}
