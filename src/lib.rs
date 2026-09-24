//! NAND Flash Viewer - High-performance visualization for large NAND dump files
//!
//! This library provides efficient tile-based rendering of NAND flash dumps
//! using an image pyramid algorithm for responsive zoom/pan operations.

pub mod types;
pub mod error;
pub mod logging;
pub mod coordinate_parser;
pub mod bit_renderer;
pub mod byte_arranger;
pub mod block_arranger;
pub mod file_loader;
pub mod cache_manager;
pub mod metadata_manager;
pub mod file_dialog;
pub mod task_queue;
pub mod tile_generator;
pub mod pyramid_tile_generator;
pub mod tile_iterator;
pub mod worker_pool;
pub mod viewport_manager;
pub mod zoom_controller;
pub mod pan_controller;
pub mod address_display;
pub mod viewport_renderer;
pub mod app_window;
pub mod multi_file_manager;
pub mod window_manager;
pub mod window_state;

#[cfg(test)]
mod multi_file_tests;

#[cfg(test)]
mod integration_tests;

pub use error::{Error, Result};
pub use types::*;
pub use coordinate_parser::CoordinateParser;
pub use bit_renderer::{BitRenderer, Pixel, PixelBuffer};
pub use byte_arranger::ByteArranger;
pub use block_arranger::BlockArranger;
pub use file_loader::FileLoader;
pub use cache_manager::CacheManager;
pub use metadata_manager::{MetadataManager, Metadata};
pub use file_dialog::FileDialog;
pub use task_queue::TaskQueue;
pub use tile_generator::TileGenerator;
pub use pyramid_tile_generator::PyramidTileGenerator;
pub use tile_iterator::TileIterator;
pub use worker_pool::WorkerPool;
pub use viewport_manager::ViewportManager;
pub use zoom_controller::ZoomController;
pub use pan_controller::PanController;
pub use address_display::AddressDisplay;
pub use viewport_renderer::ViewportRenderer;
pub use app_window::AppWindow;
pub use multi_file_manager::{MultiFileManager, DumpId, DumpFileState};
pub use window_manager::{WindowManager, WindowId, WindowState};
pub use window_state::{WindowState as WindowGeometry, WindowStateManager};
pub mod workflow;
pub mod data_provider;
pub mod search;
pub mod search_tab;
pub mod hex_tab;
pub mod fuse_node;
pub mod page_structure_tab;

pub use data_provider::{DumpDataProvider, FileDataProvider, SearchFilteredDataProvider, XorDataProvider};
pub use search::{SearchResult, SearchOptions, SearchMode, search_provider, search_provider_with_sender, export_results_to_file};
pub use search_tab::SearchTabState;
pub use hex_tab::{HexTabState, HexDiffMode};
pub use fuse_node::{ActiveFuseMount, InodeKind, parse_inode, default_mount_dir_for_node};
pub use page_structure_tab::PageStructureTabState;

pub use workflow::{WorkflowEditorState, WorkflowNode, WorkflowNodeKind};
