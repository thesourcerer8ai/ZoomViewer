//! Custom egui Node Graph Canvas & Workflow Editor Module
//!
//! Provides a single-window embedded node graph editor using egui + fltk-egui.

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::Receiver;
use parking_lot::Mutex;
use crate::data_provider::{DumpDataProvider, FileDataProvider, SearchContextDataProvider, SearchFilteredDataProvider, XorDataProvider};
use crate::file_loader::FileLoader;
use crate::types::FileMetadata;
use crate::pattern_healing::{DataSegment, PatternHealingDataProvider};
use crate::page_structure_tab::{PageStructureTabState, SegmentKind};
pub use crate::file_dialog::FileDialog;

/// Status of node execution
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum NodeExecutionStatus {
    Idle,
    Running,
    Completed,
    Error(String),
}

/// Output mode for the PatternSearch workflow node
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum SearchOutputMode {
    /// Emit one output page per unique *source page* that contains at least one match.
    /// This is the original behavior.
    #[default]
    AllMatchingPages,
    /// Emit one output page per *individual match*, sized
    /// `bytes_before + pattern_len + bytes_after`.  The match always starts at
    /// offset `bytes_before` inside the page; regions that fall outside the
    /// source are zero-padded so every page is the same size and every match
    /// is aligned at the same position.
    OnePagePerResult,
}

/// Available types of workflow processing nodes
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum WorkflowNodeKind {
    Input {
        file_path: String,
        page_length: u32,
        block_size: u32,
    },
    XorTransform {
        pattern_hex: String,
    },
    PatternSearch {
        search_pattern: String,
        is_hex: bool,
        case_sensitive: bool,
        max_matches: usize,
        /// How search results are mapped to output pages.
        #[serde(default)]
        output_mode: SearchOutputMode,
        /// Bytes of source context to include *before* each match (Mode B only).
        #[serde(default)]
        bytes_before: u64,
        /// Bytes of source context to include *after* each match (Mode B only).
        #[serde(default)]
        bytes_after: u64,
        #[serde(default)]
        results: Vec<crate::search::SearchResult>,
        /// Byte offset at which the *next* search run should start.
        /// Set to the byte just past the last result's offset after a limit-capped
        /// run, so the user can increase the limit and continue without rescanning.
        /// `None` means "start from the beginning".
        #[serde(default)]
        resume_from_offset: Option<u64>,
    },
    BlockArranger {
        grid_width: u32,
        grid_height: u32,
        stripe_width: u32,
    },
    OutputViewer {
        name: String,
    },
    FileExport {
        export_path: String,
        auto_export: bool,
    },
    FuseMount {
        mount_path: String,
        auto_mount: bool,
    },
    Concatenate,
    /// Modulo page filter — emits every Nth page starting at the Mth page (0-indexed).
    /// For example N=3, M=0 yields pages 0, 3, 6, 9, …
    ModuloPageFilter {
        /// Keep every Nth page (step size). Must be ≥ 1.
        modulo: u64,
        /// Index of the first page to keep (0-indexed offset within each period).
        offset: u64,
    },
    /// NAND reader node — connected to a physical NAND controller over serial.
    /// Carries the device settings so the workflow can show its configuration
    /// and link to the NAND Reader tab.
    NandReader {
        /// Serial device path, e.g. "/dev/ttyUSB0".
        device_path: String,
        /// Dump output filename.
        dump_filename: String,
        /// Page size in bytes.
        page_size: u32,
        /// Pages per block.
        pages_per_block: u32,
        /// Number of blocks.
        num_blocks: u32,
    },
    /// Block selector — forwards only the blocks whose indices match the
    /// user-specified `blocks` spec (single numbers, comma-separated lists,
    /// and/or ranges like `3-200`).  Blocks are emitted contiguously in
    /// ascending order.
    ///
    /// When `blocks` is empty and the node is connected to an upstream node,
    /// it is automatically filled with `"0-<MAXBLOCK>"`.
    BlockSelector {
        /// Human-readable block spec, e.g. `"0-199"`, `"0,5,10-20"`, `"0xFF"`.
        /// Empty means "pass through everything" (filled on first connection).
        blocks: String,
    },
    /// Automated pattern healing node.
    ///
    /// Reads every 512-byte DATA sub-block from the upstream provider (using
    /// the DATA segments from the shared `PageStructureTabState`) and attempts
    /// to heal it via:
    ///  • Fill detection (>90 % of 0x00 / 0xFF / 0x77)
    ///  • LBA / `|Block#` sector regeneration with majority-voted address
    ///  • P00000 ECC pattern regeneration with the special bit taken from the
    ///    raw upstream data
    ///
    /// All non-DATA bytes (ECC, SA, …) are passed through unchanged.
    PatternHealing,
    /// Block permutation node — enumerates every non-empty subset of the
    /// upstream blocks and emits them as a single concatenated stream.
    ///
    /// For N source blocks the output contains `2^N - 1` blocks total.
    ///
    /// **Unordered** (`ordered = false`): subsets are grouped by size, so all
    /// singletons come first, then all pairs, then all triples, …
    /// Example (N=4): `0 1 2 3 | 0 1  0 2  0 3  1 2  1 3  2 3 | …`
    ///
    /// **Ordered** (`ordered = true`): subsets are emitted in strict
    /// lexicographic order, grouping by leading element first.
    /// Example (N=4): `0  0 1  0 1 2  0 1 2 3  0 1 3  0 2  …`
    BlockPermutation {
        /// `true` → lexicographic / ordered mode; `false` → size-grouped mode.
        ordered: bool,
    },
}

/// Data structure for a node in the workflow graph
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowNode {
    pub id: usize,
    pub name: String,
    pub pos: [f32; 2],
    pub kind: WorkflowNodeKind,
    pub status: NodeExecutionStatus,
    pub output_log: String,
    /// Whether the node window is collapsed (title bar only).
    /// Defaults to `false` so existing workflow files load without the field.
    #[serde(default)]
    pub collapsed: bool,
}

impl WorkflowNode {
    pub fn new_input(id: usize, pos: [f32; 2], path: &str, page_length: u32, block_size: u32) -> Self {
        Self {
            id,
            name: "NAND Dump Input".to_string(),
            pos,
            kind: WorkflowNodeKind::Input {
                file_path: path.to_string(),
                page_length,
                block_size,
            },
            status: NodeExecutionStatus::Idle,
            output_log: "No execution history".to_string(),
            collapsed: false,
        }
    }

    pub fn new_xor(id: usize, pos: [f32; 2]) -> Self {
        Self {
            id,
            name: "XOR Transformation".to_string(),
            pos,
            kind: WorkflowNodeKind::XorTransform {
                pattern_hex: String::new(),
            },
            status: NodeExecutionStatus::Idle,
            output_log: "No execution history".to_string(),
            collapsed: false,
        }
    }

    pub fn new_pattern_search(id: usize, pos: [f32; 2]) -> Self {
        Self {
            id,
            name: "Search Node".to_string(),
            pos,
            kind: WorkflowNodeKind::PatternSearch {
                search_pattern: "|Block".to_string(),
                is_hex: false,
                case_sensitive: true,
                max_matches: 100,
                output_mode: SearchOutputMode::AllMatchingPages,
                bytes_before: 0,
                bytes_after: 0,
                results: Vec::new(),
                resume_from_offset: None,
            },
            status: NodeExecutionStatus::Idle,
            output_log: "No execution history".to_string(),
            collapsed: false,
        }
    }

    pub fn new_block_arranger(id: usize, pos: [f32; 2]) -> Self {
        Self {
            id,
            name: "Block Arranger".to_string(),
            pos,
            kind: WorkflowNodeKind::BlockArranger {
                grid_width: 16,
                grid_height: 16,
                stripe_width: 512,
            },
            status: NodeExecutionStatus::Idle,
            output_log: "No execution history".to_string(),
            collapsed: false,
        }
    }

    pub fn new_output(id: usize, pos: [f32; 2], name: &str) -> Self {
        Self {
            id,
            name: format!("Output: {}", name),
            pos,
            kind: WorkflowNodeKind::OutputViewer {
                name: name.to_string(),
            },
            status: NodeExecutionStatus::Idle,
            output_log: "Connected to ZoomViewer".to_string(),
            collapsed: false,
        }
    }

    pub fn new_export(id: usize, pos: [f32; 2]) -> Self {
        Self {
            id,
            name: "Dump Export".to_string(),
            pos,
            kind: WorkflowNodeKind::FileExport {
                export_path: "exported_dump.bin".to_string(),
                auto_export: false,
            },
            status: NodeExecutionStatus::Idle,
            output_log: "Ready to export".to_string(),
            collapsed: false,
        }
    }

    pub fn new_fuse_mount(id: usize, pos: [f32; 2]) -> Self {
        let mount_path = crate::fuse_node::default_mount_dir_for_node(id)
            .to_string_lossy()
            .to_string();
        Self {
            id,
            name: "FUSE Mount".to_string(),
            pos,
            kind: WorkflowNodeKind::FuseMount {
                mount_path,
                auto_mount: false,
            },
            status: NodeExecutionStatus::Idle,
            output_log: "Ready to mount virtual filesystem".to_string(),
            collapsed: false,
        }
    }

    pub fn new_concatenate(id: usize, pos: [f32; 2]) -> Self {
        Self {
            id,
            name: "Concatenate".to_string(),
            pos,
            kind: WorkflowNodeKind::Concatenate,
            status: NodeExecutionStatus::Idle,
            output_log: "No execution history".to_string(),
            collapsed: false,
        }
    }

    pub fn new_modulo_page_filter(id: usize, pos: [f32; 2]) -> Self {
        Self {
            id,
            name: "Modulo Page Filter".to_string(),
            pos,
            kind: WorkflowNodeKind::ModuloPageFilter {
                modulo: 3,
                offset: 0,
            },
            status: NodeExecutionStatus::Idle,
            output_log: "No execution history".to_string(),
            collapsed: false,
        }
    }

    pub fn new_nand_reader(id: usize, pos: [f32; 2]) -> Self {
        // Try to inherit last-used settings from the persisted NandReaderSettings
        let saved = crate::nand_reader_tab::NandReaderSettings::load();
        Self {
            id,
            name: "NAND Reader".to_string(),
            pos,
            kind: WorkflowNodeKind::NandReader {
                device_path: saved.device_path,
                dump_filename: saved.dump_filename,
                page_size: saved.page_size,
                pages_per_block: saved.pages_per_block,
                num_blocks: saved.num_blocks,
            },
            status: NodeExecutionStatus::Idle,
            output_log: "Connect the NAND Reader tab to start.".to_string(),
            collapsed: false,
        }
    }

    pub fn new_pattern_healing(id: usize, pos: [f32; 2]) -> Self {
        Self {
            id,
            name: "Pattern Healing".to_string(),
            pos,
            kind: WorkflowNodeKind::PatternHealing,
            status: NodeExecutionStatus::Idle,
            output_log: "Connect an upstream node to start healing.".to_string(),
            collapsed: false,
        }
    }

    pub fn new_block_selector(id: usize, pos: [f32; 2]) -> Self {
        Self {
            id,
            name: "Block Selector".to_string(),
            pos,
            kind: WorkflowNodeKind::BlockSelector {
                blocks: String::new(),
            },
            status: NodeExecutionStatus::Idle,
            output_log: "Connect an upstream node and set the block range.".to_string(),
            collapsed: false,
        }
    }

    pub fn new_block_permutation(id: usize, pos: [f32; 2]) -> Self {
        Self {
            id,
            name: "Block Permutation".to_string(),
            pos,
            kind: WorkflowNodeKind::BlockPermutation { ordered: false },
            status: NodeExecutionStatus::Idle,
            output_log: "Connect an upstream node to enumerate block subsets.".to_string(),
            collapsed: false,
        }
    }

    pub fn execute(&mut self) {
        self.status = NodeExecutionStatus::Running;
        let start_time = std::time::Instant::now();

        match &self.kind {
            WorkflowNodeKind::Input {
                file_path,
                page_length,
                block_size,
            } => {
                let exists = Path::new(file_path).exists();
                self.status = NodeExecutionStatus::Completed;
                self.output_log = format!(
                    "Input loaded!\nFile: {}\nExists: {}\nPage length: {} B, Block size: {} pages",
                    file_path, exists, page_length, block_size
                );
            }
            WorkflowNodeKind::XorTransform { pattern_hex } => {
                self.status = NodeExecutionStatus::Completed;
                self.output_log = format!(
                    "XOR Transform applied!\nPattern: {}\nElapsed: {:.2}ms",
                    pattern_hex,
                    start_time.elapsed().as_secs_f64() * 1000.0
                );
            }
            WorkflowNodeKind::PatternSearch {
                search_pattern,
                is_hex,
                max_matches,
                ..
            } => {
                self.status = NodeExecutionStatus::Completed;
                self.output_log = format!(
                    "Pattern Search Completed!\nPattern: '{}' (Hex: {})\nMatches found: 0 (limit {})",
                    search_pattern, is_hex, max_matches
                );
            }
            WorkflowNodeKind::BlockArranger {
                grid_width,
                grid_height,
                stripe_width,
            } => {
                self.status = NodeExecutionStatus::Completed;
                self.output_log = format!(
                    "Block Map Created!\nGrid: {}x{}\nStripe Width: {} bytes",
                    grid_width, grid_height, stripe_width
                );
            }
            WorkflowNodeKind::OutputViewer { name } => {
                self.status = NodeExecutionStatus::Completed;
                self.output_log = format!(
                    "Output Viewer '{}' ready and connected to ZoomViewer.",
                    name
                );
            }
            WorkflowNodeKind::FileExport { export_path, auto_export } => {
                self.status = NodeExecutionStatus::Completed;
                self.output_log = format!(
                    "Workflow Output ready!\nExport target: {}\nAuto-export: {}",
                    export_path, auto_export
                );
            }
            WorkflowNodeKind::FuseMount { mount_path, auto_mount } => {
                self.status = NodeExecutionStatus::Completed;
                self.output_log = format!(
                    "FUSE Mount node ready!\nMount target: {}\nAuto-mount: {}",
                    mount_path, auto_mount
                );
            }
            WorkflowNodeKind::Concatenate => {
                self.status = NodeExecutionStatus::Completed;
                self.output_log = "Concatenate node ready.\nConnect 2 or more inputs.".to_string();
            }
            WorkflowNodeKind::ModuloPageFilter { modulo, offset } => {
                self.status = NodeExecutionStatus::Completed;
                self.output_log = format!(
                    "Modulo Page Filter ready.\nKeeping every {}th page starting at offset {}.",
                    modulo, offset
                );
            }
            WorkflowNodeKind::NandReader { device_path, dump_filename, .. } => {
                self.status = NodeExecutionStatus::Idle;
                self.output_log = format!(
                    "NAND Reader node.\nDevice: {}\nDump file: {}\nUse the 'NAND Reader' tab to connect and dump.",
                    device_path, dump_filename
                );
            }
            WorkflowNodeKind::PatternHealing => {
                self.status = NodeExecutionStatus::Completed;
                self.output_log = "Pattern Healing node ready.\nConnect an upstream node and view output to start healing.".to_string();
            }
            WorkflowNodeKind::BlockSelector { blocks } => {
                self.status = NodeExecutionStatus::Completed;
                let spec = if blocks.trim().is_empty() {
                    "all blocks (connect upstream to auto-fill range)".to_string()
                } else {
                    format!("blocks: {}", blocks)
                };
                self.output_log = format!("Block Selector ready.\nSelected: {}", spec);
            }
            WorkflowNodeKind::BlockPermutation { ordered } => {
                self.status = NodeExecutionStatus::Completed;
                let mode = if *ordered { "ordered (lexicographic)" } else { "unordered (size-grouped)" };
                self.output_log = format!(
                    "Block Permutation ready.\nMode: {}\nConnect an upstream node to enumerate all subsets.\n⚠ Output size = 2^N − 1 blocks.",
                    mode
                );
            }
        }
    }
}

/// Helper to parse a hex string (e.g. "FF", "0x77", "AA BB CC") into bytes
pub fn parse_hex_bytes(s: &str) -> Vec<u8> {
    let clean: String = s.chars().filter(|c| c.is_ascii_hexdigit()).collect();
    let mut bytes = Vec::new();
    let mut chars = clean.chars();
    while let (Some(c1), Some(c2)) = (chars.next(), chars.next()) {
        if let Ok(b) = u8::from_str_radix(&format!("{}{}", c1, c2), 16) {
            bytes.push(b);
        }
    }
    bytes
}

/// Parse a block-selection spec string into a sorted, deduplicated list of block indices.
///
/// Accepts:
/// * Single numbers in decimal or hex (`0`, `5`, `0xFF`)
/// * Comma-separated lists (`0,5,10`)
/// * Ranges (`3-200`, `0x10-0x20`)
/// * Any combination: `"0,3-10,0xFF"`
///
/// `max_block` is the inclusive upper bound (used to clamp open-ended specs and
/// to validate that requested indices exist).  Pass `u64::MAX` to skip clamping.
///
/// Returns `None` if the string is empty or contains no parseable tokens.
pub fn parse_block_spec(spec: &str, max_block: u64) -> Option<Vec<u64>> {
    if spec.trim().is_empty() {
        return None;
    }
    let mut result: Vec<u64> = Vec::new();
    for token in spec.split(',') {
        let token = token.trim();
        if token.is_empty() {
            continue;
        }
        if let Some(dash_pos) = find_range_dash(token) {
            let start_str = token[..dash_pos].trim();
            let end_str = token[dash_pos + 1..].trim();
            if let (Some(start), Some(end)) = (parse_dec_or_hex(start_str), parse_dec_or_hex(end_str)) {
                let end_clamped = end.min(max_block);
                if start <= end_clamped {
                    for b in start..=end_clamped {
                        result.push(b);
                    }
                }
            }
        } else if let Some(val) = parse_dec_or_hex(token) {
            if val <= max_block {
                result.push(val);
            }
        }
    }
    if result.is_empty() {
        return None;
    }
    result.sort_unstable();
    result.dedup();
    Some(result)
}

/// Find the position of the `-` separator in a range token like `"3-200"` or `"0x10-0x20"`.
/// We skip any leading `-` that is part of a hex prefix, and avoid treating the `-` in `0x…`
/// as a range separator by only searching after the first digit group.
fn find_range_dash(token: &str) -> Option<usize> {
    let bytes = token.as_bytes();
    // We want the first `-` that is NOT immediately after `0x`/`0X` and NOT at position 0.
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] == b'-' && i > 0 {
            return Some(i);
        }
        i += 1;
    }
    None
}

/// Parse a string that is either a plain decimal integer or a hex value prefixed
/// with "0x" / "0X". Returns `None` if neither format parses successfully.
fn parse_dec_or_hex(s: &str) -> Option<u64> {
    let trimmed = s.trim();
    if let Some(hex) = trimmed.strip_prefix("0x").or_else(|| trimmed.strip_prefix("0X")) {
        u64::from_str_radix(hex, 16).ok()
    } else {
        trimmed.parse::<u64>().ok()
    }
}

/// A `DragValue` for a `u32` field that:
/// * Displays the current value as `0xHEX (dec)` in the widget.
/// * Accepts typed input in both decimal (`4096`) and hex (`0x1000`) form.
/// * Still supports click-and-drag to increment/decrement.
fn hex_drag_u32(value: &mut u32, speed: f64) -> egui::DragValue<'_> {
    egui::DragValue::new(value)
        .speed(speed)
        .custom_formatter(|n, _| format!("0x{:X} ({})", n as u64, n as u64))
        .custom_parser(|s| parse_dec_or_hex(s).map(|v| v as f64))
}

/// A `DragValue` for a `u64` field that:
/// * Displays the current value as `0xHEX (dec)`.
/// * Accepts typed input in both decimal and hex (`0x…`) form.
fn hex_drag_u64(value: &mut u64, speed: f64) -> egui::DragValue<'_> {
    egui::DragValue::new(value)
        .speed(speed)
        .custom_formatter(|n, _| format!("0x{:X} ({})", n as u64, n as u64))
        .custom_parser(|s| parse_dec_or_hex(s).map(|v| v as f64))
}

/// A `DragValue` for a `usize` field with hex display and hex/decimal input.
fn hex_drag_usize(value: &mut usize, speed: f64) -> egui::DragValue<'_> {
    egui::DragValue::new(value)
        .speed(speed)
        .custom_formatter(|n, _| format!("0x{:X} ({})", n as u64, n as u64))
        .custom_parser(|s| parse_dec_or_hex(s).map(|v| v as f64))
}

// ──────────────────────────────────────────────────────────────────────────────
// Metadata helpers for Input nodes
// ──────────────────────────────────────────────────────────────────────────────

#[derive(serde::Deserialize)]
#[allow(dead_code)]
struct CachedMetadataSimple {
    #[serde(default)]
    pub path: String,
    #[serde(default)]
    pub size: u64,
    pub page_length: u32,
    pub block_size: u32,
}

/// Try to load page_length and block_size from `.cache/<filename>/metadata.json` or `.metadata.json`.
/// Returns `Some((page_length, block_size))` on a valid cache hit, `None` otherwise.
pub fn load_node_metadata(file_path: &str) -> Option<(u32, u32)> {
    if file_path.trim().is_empty() {
        return None;
    }

    log::debug!("load_node_metadata()");

    let path = Path::new(file_path);
    let filename = path.file_name()?.to_str()?.to_string();

    // Check candidate cache directories:
    // 1. Current working directory .cache
    // 2. Parent directory of the dump file .cache (if dump file is in another dir)
    let mut candidate_cache_dirs = Vec::new();
    if let Ok(cwd) = std::env::current_dir() {
        candidate_cache_dirs.push(cwd.join(".cache"));
    }
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            candidate_cache_dirs.push(parent.join(".cache"));
        }
    }

    // Candidate filenames to look for in cache:
    // e.g. "sdcard0.dump", or if it ends with .xor, also try the base dump name
    let mut candidate_dump_names = vec![filename.clone()];
    if let Some(stripped) = filename.strip_suffix(".xor") {
        candidate_dump_names.push(stripped.to_string());
    }

    for cache_dir in &candidate_cache_dirs {
        for dump_name in &candidate_dump_names {
            let dump_cache_folder = cache_dir.join(dump_name);
            for json_filename in &["metadata.json", ".metadata.json"] {
                let meta_file = dump_cache_folder.join(json_filename);
                if meta_file.is_file() {
                    // Read and deserialize directly to avoid strict file stat/path checks
                    if let Ok(content) = fs::read_to_string(&meta_file) {
                        if let Ok(meta) = serde_json::from_str::<CachedMetadataSimple>(&content) {
                            if meta.page_length > 0 && meta.block_size > 0 {
                                log::debug!(
                                    "Loaded cached metadata for {}: page={} block={}",
                                    filename, meta.page_length, meta.block_size
                                );
                                return Some((meta.page_length, meta.block_size));
                            }
                        }
                    }
                }
            }
        }
    }

    None
}

/// Save page_length and block_size to `.cache/<filename>/metadata.json`.
/// Errors are logged at warn level and silently ignored.
pub fn save_node_metadata(file_path: &str, page_length: u32, block_size: u32) {
    if file_path.trim().is_empty() {
        return;
    }
    let path = Path::new(file_path);
    let filename = match path.file_name().and_then(|n| n.to_str()) {
        Some(n) => n.to_string(),
        None => return,
    };
    let size = std::fs::metadata(file_path).map(|m| m.len()).unwrap_or(0);

    let cache_dir = match std::env::current_dir() {
        Ok(d) => d.join(".cache"),
        Err(e) => {
            log::warn!("save_node_metadata: cannot get cwd: {}", e);
            return;
        }
    };
    let dump_cache_dir = cache_dir.join(&filename);
    if let Err(e) = fs::create_dir_all(&dump_cache_dir) {
        log::warn!("save_node_metadata: create_dir_all failed: {}", e);
        return;
    }

    let fm = FileMetadata::new(file_path.to_string(), size, page_length, block_size);
    let meta = crate::metadata_manager::Metadata::from_file_metadata(&fm);
    if let Ok(json_content) = serde_json::to_string_pretty(&meta) {
        let meta_file = dump_cache_dir.join("metadata.json");
        if let Err(e) = fs::write(&meta_file, json_content) {
            log::warn!("save_node_metadata: write failed for '{}': {}", meta_file.display(), e);
        } else {
            log::info!("Saved metadata for {}: page={} block={}", filename, page_length, block_size);
        }
    }
}

/// Look for an associated `.xor` file for the given dump file.
/// Following Variant A (exact append): checks for `<filename>.xor` in the same directory.
/// Only returns Some if the file exists and has size > 0.
pub fn find_associated_xor_file<P: AsRef<Path>>(dump_path: P) -> Option<PathBuf> {
    let p = dump_path.as_ref();
    let file_name = p.file_name()?.to_str()?;
    if file_name.is_empty() {
        return None;
    }
    let xor_file_name = format!("{}.xor", file_name);
    let xor_path = match p.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent.join(xor_file_name),
        _ => PathBuf::from(xor_file_name),
    };

    if xor_path.is_file() {
        if let Ok(meta) = std::fs::metadata(&xor_path) {
            if meta.len() > 0 {
                return Some(xor_path);
            }
        }
    }
    None
}

/// Representation of a connection wire between nodes
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeConnection {
    pub from_node: usize,
    pub to_node: usize,
}

/// Active background search process state
pub struct ActiveSearchState {
    pub cancel_flag: Arc<AtomicBool>,
    pub progress_bytes: Arc<AtomicU64>,
    pub total_bytes: u64,
    pub rx: Receiver<Result<Vec<crate::search::SearchResult>, String>>,
    /// Receives individual matches as they are found (for live count display).
    pub match_rx: std::sync::mpsc::Receiver<crate::search::SearchResult>,
    /// Count of matches found so far in this run (updated by poll_active_searches).
    pub live_match_count: usize,
    /// Number of results already stored on the node before this run started.
    pub prior_result_count: usize,
}

impl std::fmt::Debug for ActiveSearchState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ActiveSearchState")
            .field("total_bytes", &self.total_bytes)
            .finish()
    }
}

/// Workflow editor graph state and execution manager
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowEditorState {
    pub next_node_id: usize,
    pub nodes: Vec<WorkflowNode>,
    pub connections: Vec<NodeConnection>,
    pub selected_node: Option<usize>,
    pub connecting_from: Option<usize>,
    pub status_message: String,
    /// Tracks which Input node (by id) currently has its file browser open (not persisted)
    #[serde(skip, default)]
    pub file_browser_open: HashMap<usize, bool>,
    /// Current browsed directory per Input node (not persisted)
    #[serde(skip, default)]
    pub file_browser_path: HashMap<usize, PathBuf>,
    /// Toggle to show all files in file browser instead of only binary dump extensions (not persisted)
    #[serde(skip, default)]
    pub file_browser_show_all: HashMap<usize, bool>,
    /// Optional pending XOR suggestion: (Input node id, associated xor path) (not persisted)
    #[serde(skip, default)]
    pub pending_xor_offer: Option<(usize, PathBuf)>,
    /// Active background searches per node_id: (cancel_flag, progress_bytes, total_bytes, rx) (not persisted)
    #[serde(skip, default)]
    pub active_searches: Arc<Mutex<HashMap<usize, ActiveSearchState>>>,
    /// Active FUSE filesystem mounts per node_id (not persisted)
    #[serde(skip, default)]
    pub active_fuse_mounts: Arc<Mutex<HashMap<usize, crate::fuse_node::ActiveFuseMount>>>,
    /// Rendered rect of each node from the previous frame, used to draw
    /// connection wires from actual node edges instead of fixed offsets (not persisted)
    #[serde(skip, default)]
    pub node_rects: HashMap<usize, egui::Rect>,
    /// Shared page structure state — used by PatternHealing nodes to know which
    /// byte ranges within each page are DATA segments (not persisted)
    #[serde(skip, default)]
    pub page_structure: Option<Arc<std::sync::Mutex<PageStructureTabState>>>,
    /// Cached healing statistics per node_id (not persisted)
    #[serde(skip, default)]
    pub healing_stats_cache: HashMap<usize, crate::pattern_healing::HealingStats>,
    /// Live shared healing stats arcs, keyed by node_id.
    /// The PatternHealingDataProvider writes into these; the UI reads from them.
    #[serde(skip, default)]
    pub healing_stats: Arc<Mutex<HashMap<usize, Arc<Mutex<crate::pattern_healing::HealingStats>>>>>,
}
impl Default for WorkflowEditorState {
    fn default() -> Self {
        Self::new(None)
    }
}

impl WorkflowEditorState {
    pub fn new(default_dump_path: Option<&str>) -> Self {
        Self::new_with_geometry(default_dump_path, None, None)
    }

    pub fn new_with_geometry(
        default_dump_path: Option<&str>,
        page_length: Option<u32>,
        block_size: Option<u32>,
    ) -> Self {
        let dump_path = default_dump_path.unwrap_or("dump.bin");
        let (pl, bs) = match (page_length, block_size) {
            (Some(p), Some(b)) => (p, b),
            _ => default_dump_path
                .and_then(load_node_metadata)
                .unwrap_or((4096, 64)),
        };

        // Check if an associated .xor file exists in the same directory (Variante A)
        if let Some(dump_str) = default_dump_path {
            if let Some(xor_path) = find_associated_xor_file(dump_str) {
                let xor_path_str = xor_path.to_string_lossy().to_string();
                // Check cache for xor file first, otherwise inherit from dump file
                let (xor_pl, xor_bs) = load_node_metadata(&xor_path_str).unwrap_or((pl, bs));

                let n1 = WorkflowNode::new_input(1, [100.0, 150.0], dump_str, pl, bs);
                let n2 = WorkflowNode::new_input(2, [100.0, 320.0], &xor_path_str, xor_pl, xor_bs);
                let n3 = WorkflowNode::new_xor(3, [360.0, 235.0]);
                let n4 = WorkflowNode::new_output(4, [620.0, 235.0], "ZoomViewer");

                let connections = vec![
                    NodeConnection { from_node: 1, to_node: 3 },
                    NodeConnection { from_node: 2, to_node: 3 },
                    NodeConnection { from_node: 3, to_node: 4 },
                ];

                return Self {
                    next_node_id: 5,
                    nodes: vec![n1, n2, n3, n4],
                    connections,
                    selected_node: Some(4), // Select the OutputViewer by default
                    connecting_from: None,
                    status_message: format!(
                        "Workflow initialized with dump and matching XOR pattern: {}",
                        xor_path.display()
                    ),
                    file_browser_open: HashMap::new(),
                    file_browser_path: HashMap::new(),
                    file_browser_show_all: HashMap::new(),
                    pending_xor_offer: None,
                    active_searches: Arc::new(Mutex::new(HashMap::new())),
                    active_fuse_mounts: Arc::new(Mutex::new(HashMap::new())),
                    node_rects: HashMap::new(),
                    page_structure: None,
                    healing_stats_cache: HashMap::new(),
                    healing_stats: Arc::new(Mutex::new(HashMap::new())),
                };
            }
        }

        let n1 = WorkflowNode::new_input(1, [100.0, 200.0], dump_path, pl, bs);
        let n2 = WorkflowNode::new_output(2, [420.0, 200.0], "ZoomViewer");

        let connections = vec![
            NodeConnection { from_node: 1, to_node: 2 },
        ];

        Self {
            next_node_id: 3,
            nodes: vec![n1, n2],
            connections,
            selected_node: Some(2), // Select the OutputViewer by default
            connecting_from: None,
            status_message: "Workflow Graph Editor ready. OutputViewer node selected.".to_string(),
            file_browser_open: HashMap::new(),
            file_browser_path: HashMap::new(),
            file_browser_show_all: HashMap::new(),
            pending_xor_offer: None,
            active_searches: Arc::new(Mutex::new(HashMap::new())),
            active_fuse_mounts: Arc::new(Mutex::new(HashMap::new())),
            node_rects: HashMap::new(),
            page_structure: None,
            healing_stats_cache: HashMap::new(),
            healing_stats: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Converts a single-input node connection into a 2-input XOR pipeline.
    pub fn insert_xor_for_input(&mut self, input_node_id: usize, xor_path: &Path) {
        let (pos, pl, bs) = if let Some(n) = self.nodes.iter().find(|n| n.id == input_node_id) {
            if let WorkflowNodeKind::Input { page_length, block_size, .. } = n.kind {
                (n.pos, page_length, block_size)
            } else {
                ([100.0, 200.0], 4096, 64)
            }
        } else {
            return;
        };

        let xor_path_str = xor_path.to_string_lossy().to_string();
        log::debug!("insert_xor_for_input()");
        let (xor_pl, xor_bs) = load_node_metadata(&xor_path_str).unwrap_or((pl, bs));

        let xor_input_id = self.next_node_id;
        let xor_transform_id = self.next_node_id + 1;
        self.next_node_id += 2;

        let xor_input_pos = [pos[0], pos[1] + 160.0];
        let xor_trans_pos = [pos[0] + 250.0, pos[1] + 80.0];

        let n_xor_in = WorkflowNode::new_input(xor_input_id, xor_input_pos, &xor_path_str, xor_pl, xor_bs);
        let n_xor_tr = WorkflowNode::new_xor(xor_transform_id, xor_trans_pos);

        self.nodes.push(n_xor_in);
        self.nodes.push(n_xor_tr);

        // Re-route outgoing connections from input_node_id to xor_transform_id
        for conn in &mut self.connections {
            if conn.from_node == input_node_id {
                conn.from_node = xor_transform_id;
            }
        }

        // Connect input_node_id -> xor_transform_id (primary)
        self.connections.push(NodeConnection {
            from_node: input_node_id,
            to_node: xor_transform_id,
        });

        // Connect xor_input_id -> xor_transform_id (secondary)
        self.connections.push(NodeConnection {
            from_node: xor_input_id,
            to_node: xor_transform_id,
        });

        self.status_message = format!("Added XOR pipeline with {}", xor_path.display());
    }

    pub fn add_node(&mut self, kind: WorkflowNodeKind) {
        let id = self.next_node_id;
        self.next_node_id += 1;
        let pos = [300.0 + (id as f32 * 20.0), 200.0 + (id as f32 * 10.0)];

        let node = match kind {
            WorkflowNodeKind::Input { file_path, page_length, block_size } => {
                log::debug!("add_node()");
                let (pl, bs) = load_node_metadata(&file_path).unwrap_or((page_length, block_size));
                WorkflowNode::new_input(id, pos, &file_path, pl, bs)
            }
            WorkflowNodeKind::XorTransform { .. } => WorkflowNode::new_xor(id, pos),
            WorkflowNodeKind::PatternSearch { .. } => WorkflowNode::new_pattern_search(id, pos),
            WorkflowNodeKind::BlockArranger { .. } => WorkflowNode::new_block_arranger(id, pos),
            WorkflowNodeKind::OutputViewer { .. } => WorkflowNode::new_output(id, pos, "Viewer"),
            WorkflowNodeKind::FileExport { .. } => WorkflowNode::new_export(id, pos),
            WorkflowNodeKind::FuseMount { .. } => WorkflowNode::new_fuse_mount(id, pos),
            WorkflowNodeKind::Concatenate => WorkflowNode::new_concatenate(id, pos),
            WorkflowNodeKind::ModuloPageFilter { .. } => WorkflowNode::new_modulo_page_filter(id, pos),
            WorkflowNodeKind::NandReader { .. } => WorkflowNode::new_nand_reader(id, pos),
            WorkflowNodeKind::PatternHealing => WorkflowNode::new_pattern_healing(id, pos),
            WorkflowNodeKind::BlockSelector { .. } => WorkflowNode::new_block_selector(id, pos),
            WorkflowNodeKind::BlockPermutation { .. } => WorkflowNode::new_block_permutation(id, pos),
        };

        self.nodes.push(node);
        self.selected_node = Some(id);
        self.status_message = format!("Added node #{}", id);
    }

    /// Find the currently selected OutputViewer node ID, or the first OutputViewer in the graph
    pub fn get_active_output_node_id(&self) -> Option<usize> {
        if let Some(sel) = self.selected_node {
            if let Some(node) = self.nodes.iter().find(|n| n.id == sel) {
                if matches!(node.kind, WorkflowNodeKind::OutputViewer { .. }) {
                    return Some(sel);
                }
            }
        }
        // Fallback to first OutputViewer found in nodes
        self.nodes.iter().find(|n| matches!(n.kind, WorkflowNodeKind::OutputViewer { .. })).map(|n| n.id)
    }

    /// Walk upstream from the active OutputViewer and return the `page_length` of
    /// the first `Input` node found in the pipeline. Returns `None` when no pipeline
    /// is active or no Input node exists upstream.
    pub fn active_page_length(&self) -> Option<u32> {
        let output_id = self.get_active_output_node_id()?;
        let mut current_id = output_id;
        let mut visited = HashSet::new();
        loop {
            if !visited.insert(current_id) {
                break; // cycle guard
            }
            if let Some(node) = self.nodes.iter().find(|n| n.id == current_id) {
                if let WorkflowNodeKind::Input { page_length, .. } = &node.kind {
                    if *page_length > 0 {
                        return Some(*page_length);
                    }
                }
            }
            // Walk one step upstream
            if let Some(conn) = self.connections.iter().find(|c| c.to_node == current_id) {
                current_id = conn.from_node;
            } else {
                break;
            }
        }
        None
    }

    /// Recursively builds a dynamic `DumpDataProvider` pipeline rooted at `target_node_id`.
    pub fn build_data_provider(&self, target_node_id: usize) -> Result<Arc<Mutex<dyn DumpDataProvider>>, String> {
        // `ancestors` tracks the nodes on the *current* path from the root down to here.
        // This correctly detects cycles (A→B→A) while allowing the same node to be
        // reached via independent branches (diamond-shaped DAGs like A→B→D and A→C→D).
        let mut ancestors = HashSet::new();
        self.build_data_provider_internal(target_node_id, &mut ancestors)
    }

    fn build_data_provider_internal(
        &self,
        target_node_id: usize,
        ancestors: &mut HashSet<usize>,
    ) -> Result<Arc<Mutex<dyn DumpDataProvider>>, String> {
        if !ancestors.insert(target_node_id) {
            return Err(format!("Cycle detected in workflow graph at node #{}", target_node_id));
        }

        let node = self.nodes.iter().find(|n| n.id == target_node_id)
            .ok_or_else(|| format!("Node #{} not found in workflow", target_node_id))?;

        let result = match &node.kind {
            WorkflowNodeKind::Input { file_path, page_length, block_size } => {
                let loader = FileLoader::new(file_path, *page_length, *block_size)
                    .map_err(|e| format!("Failed to open input file '{}': {}", file_path, e))?;
                let provider = FileDataProvider::new(loader);
                Ok(Arc::new(Mutex::new(provider)) as Arc<Mutex<dyn DumpDataProvider>>)
            }
            WorkflowNodeKind::XorTransform { pattern_hex } => {
                let incoming: Vec<usize> = self.connections.iter()
                    .filter(|c| c.to_node == target_node_id)
                    .map(|c| c.from_node)
                    .collect();

                let static_pattern = parse_hex_bytes(pattern_hex);

                if incoming.is_empty() {
                    return Err(format!("XorTransform node #{} has no input connections", target_node_id));
                }

                let primary = self.build_data_provider_internal(incoming[0], ancestors)?;
                let mut secondaries = Vec::new();
                for &from_id in incoming.iter().skip(1).take(2) {
                    secondaries.push(self.build_data_provider_internal(from_id, ancestors)?);
                }

                let xor_provider = XorDataProvider::new(primary, secondaries, static_pattern);
                Ok(Arc::new(Mutex::new(xor_provider)) as Arc<Mutex<dyn DumpDataProvider>>)
            }
            WorkflowNodeKind::OutputViewer { .. } => {
                let incoming = self.connections.iter()
                    .find(|c| c.to_node == target_node_id)
                    .ok_or_else(|| format!("OutputViewer node #{} has no input connected", target_node_id))?;
                self.build_data_provider_internal(incoming.from_node, ancestors)
            }
            WorkflowNodeKind::PatternSearch {
                search_pattern,
                is_hex,
                case_sensitive,
                results,
                output_mode,
                bytes_before,
                bytes_after,
                ..
            } => {
                let incoming = self.connections.iter()
                    .find(|c| c.to_node == target_node_id)
                    .ok_or_else(|| format!("Search node #{} has no input connected", target_node_id))?;
                let upstream_id = incoming.from_node;

                // Clone everything we need before the recursive call.
                let search_pattern = search_pattern.clone();
                let output_mode = *output_mode;
                let bytes_before = *bytes_before;
                let bytes_after = *bytes_after;
                let is_hex = *is_hex;
                let case_sensitive = *case_sensitive;
                let results: Vec<crate::search::SearchResult> = results.clone();

                let upstream = self.build_data_provider_internal(upstream_id, ancestors)?;

                match output_mode {
                    SearchOutputMode::AllMatchingPages => {
                        let page_len = upstream.lock().get_metadata().page_length as u64;
                        let mut matching_pages: Vec<u64> = if page_len > 0 {
                            results.iter().map(|r| r.byte_offset / page_len).collect()
                        } else {
                            Vec::new()
                        };
                        matching_pages.sort_unstable();
                        matching_pages.dedup();

                        let filtered = SearchFilteredDataProvider::new(upstream, matching_pages, search_pattern);
                        Ok(Arc::new(Mutex::new(filtered)) as Arc<Mutex<dyn DumpDataProvider>>)
                    }
                    SearchOutputMode::OnePagePerResult => {
                        let mode = if is_hex {
                            crate::search::SearchMode::Hex
                        } else {
                            crate::search::SearchMode::Ascii
                        };
                        let pat_opts = crate::search::SearchOptions {
                            pattern: search_pattern.clone(),
                            mode,
                            case_sensitive,
                            max_matches: 1,
                        };
                        let pattern_len = crate::search::parse_pattern(&pat_opts)
                            .map(|b| b.len() as u64)
                            .unwrap_or(1);

                        let match_offsets: Vec<u64> =
                            results.iter().map(|r| r.byte_offset).collect();

                        let ctx = SearchContextDataProvider::new(
                            upstream,
                            match_offsets,
                            bytes_before,
                            bytes_after,
                            pattern_len,
                            search_pattern,
                        );
                        Ok(Arc::new(Mutex::new(ctx)) as Arc<Mutex<dyn DumpDataProvider>>)
                    }
                }
            }
            WorkflowNodeKind::BlockArranger { .. } => {
                let incoming = self.connections.iter()
                    .find(|c| c.to_node == target_node_id)
                    .ok_or_else(|| format!("BlockArranger node #{} has no input connected", target_node_id))?;
                self.build_data_provider_internal(incoming.from_node, ancestors)
            }
            WorkflowNodeKind::FileExport { .. } => {
                let incoming = self.connections.iter()
                    .find(|c| c.to_node == target_node_id)
                    .ok_or_else(|| format!("FileExport node #{} has no input connected", target_node_id))?;
                self.build_data_provider_internal(incoming.from_node, ancestors)
            }
            WorkflowNodeKind::FuseMount { .. } => {
                let incoming = self.connections.iter()
                    .find(|c| c.to_node == target_node_id)
                    .ok_or_else(|| format!("FuseMount node #{} has no input connected", target_node_id))?;
                self.build_data_provider_internal(incoming.from_node, ancestors)
            }
            WorkflowNodeKind::Concatenate => {
                let incoming: Vec<usize> = self.connections.iter()
                    .filter(|c| c.to_node == target_node_id)
                    .map(|c| c.from_node)
                    .collect();

                if incoming.is_empty() {
                    return Err(format!("Concatenate node #{} has no input connections", target_node_id));
                }

                let mut providers = Vec::with_capacity(incoming.len());
                for &from_id in &incoming {
                    providers.push(self.build_data_provider_internal(from_id, ancestors)?);
                }

                let concat_provider = crate::data_provider::ConcatenateDataProvider::new(providers);
                Ok(Arc::new(Mutex::new(concat_provider)) as Arc<Mutex<dyn DumpDataProvider>>)
            }
            WorkflowNodeKind::ModuloPageFilter { modulo, offset } => {
                let incoming = self.connections.iter()
                    .find(|c| c.to_node == target_node_id)
                    .ok_or_else(|| format!("ModuloPageFilter node #{} has no input connected", target_node_id))?;
                let upstream = self.build_data_provider_internal(incoming.from_node, ancestors)?;
                let modulo = *modulo;
                let offset = *offset;
                let provider = crate::data_provider::ModuloPageFilterDataProvider::new(upstream, modulo, offset);
                Ok(Arc::new(Mutex::new(provider)) as Arc<Mutex<dyn DumpDataProvider>>)
            }
            WorkflowNodeKind::NandReader { dump_filename, page_size, pages_per_block, .. } => {
                // The NAND reader node exposes the dump file it wrote as a read-only provider.
                // page_size is the page length; pages_per_block is used as the block_size.
                if dump_filename.trim().is_empty() {
                    return Err(format!("NandReader node #{} has no dump filename set", target_node_id));
                }
                let loader = FileLoader::new(dump_filename, *page_size, *pages_per_block)
                    .map_err(|e| format!("Failed to open dump file '{}': {}", dump_filename, e))?;
                let provider = FileDataProvider::new(loader);
                Ok(Arc::new(Mutex::new(provider)) as Arc<Mutex<dyn DumpDataProvider>>)
            }
            WorkflowNodeKind::PatternHealing => {
                let incoming = self.connections.iter()
                    .find(|c| c.to_node == target_node_id)
                    .ok_or_else(|| format!("PatternHealing node #{} has no input connected", target_node_id))?;
                let upstream = self.build_data_provider_internal(incoming.from_node, ancestors)?;

                // Build the DataSegment list from the shared PageStructureTabState.
                // If no page structure is set, or it has no DATA segments, pass an empty
                // list — the provider will fall back to whole-page healing for 512-byte pages.
                let data_segments: Vec<DataSegment> = if let Some(ps_arc) = &self.page_structure {
                    let ps = ps_arc.lock().unwrap();
                    let page_len = upstream.lock().get_metadata().page_length as u32;
                    let mut segs = Vec::new();
                    let mut offset: u32 = 0;
                    for seg in &ps.segments {
                        let seg_end = offset + seg.size;
                        if seg.kind == SegmentKind::Data && seg.size >= 512 {
                            // Only include segments that are large enough for at least one 512-byte block
                            // and lie within the page boundary.
                            let clamped_end = seg_end.min(page_len);
                            if clamped_end > offset {
                                segs.push(DataSegment { start: offset, end: clamped_end });
                            }
                        }
                        offset = seg_end;
                    }
                    segs
                } else {
                    Vec::new()
                };

                let mut provider = PatternHealingDataProvider::new(upstream, data_segments);

                // Wire up shared stats so the UI can read them without rebuilding the provider.
                let stats_arc = {
                    let mut map = self.healing_stats.lock();
                    map.entry(target_node_id)
                        .or_insert_with(|| Arc::new(Mutex::new(crate::pattern_healing::HealingStats::default())))
                        .clone()
                };
                provider.stats = stats_arc;

                Ok(Arc::new(Mutex::new(provider)) as Arc<Mutex<dyn DumpDataProvider>>)
            }
            WorkflowNodeKind::BlockSelector { blocks } => {
                let incoming = self.connections.iter()
                    .find(|c| c.to_node == target_node_id)
                    .ok_or_else(|| format!("BlockSelector node #{} has no input connected", target_node_id))?;
                let upstream = self.build_data_provider_internal(incoming.from_node, ancestors)?;

                // Determine the total block count from the upstream provider.
                let total_blocks = {
                    let guard = upstream.lock();
                    let m = guard.get_metadata();
                    m.total_blocks
                };

                let max_block = total_blocks.saturating_sub(1);

                // If spec is empty, pass through all blocks.
                let spec = if blocks.trim().is_empty() {
                    format!("0-{}", max_block)
                } else {
                    blocks.clone()
                };

                let selected = parse_block_spec(&spec, max_block)
                    .unwrap_or_else(|| (0..=max_block).collect());

                let provider = crate::data_provider::BlockSelectorDataProvider::new(upstream, selected, spec);
                Ok(Arc::new(Mutex::new(provider)) as Arc<Mutex<dyn DumpDataProvider>>)
            }
            WorkflowNodeKind::BlockPermutation { ordered } => {
                let incoming = self.connections.iter()
                    .find(|c| c.to_node == target_node_id)
                    .ok_or_else(|| format!("BlockPermutation node #{} has no input connected", target_node_id))?;
                let upstream = self.build_data_provider_internal(incoming.from_node, ancestors)?;
                let provider = crate::data_provider::BlockPermutationDataProvider::new(upstream, *ordered);
                Ok(Arc::new(Mutex::new(provider)) as Arc<Mutex<dyn DumpDataProvider>>)
            }
        };

        // Backtrack: remove this node from the ancestor path so sibling branches
        // (diamond-shaped DAGs) can visit the same node without a false cycle error.
        ancestors.remove(&target_node_id);
        result
    }

    /// Build data provider for the active OutputViewer node
    pub fn build_active_output_provider(&self) -> Result<Arc<Mutex<dyn DumpDataProvider>>, String> {
        let node_id = self.get_active_output_node_id()
            .ok_or_else(|| "No OutputViewer node found in workflow".to_string())?;
        self.build_data_provider(node_id)
    }

    /// Build data provider for search: target selected node, or active output viewer
    pub fn build_search_provider(&self) -> Result<(usize, String, Arc<Mutex<dyn DumpDataProvider>>), String> {
        let node_id = if let Some(sel) = self.selected_node {
            sel
        } else if let Some(out_id) = self.get_active_output_node_id() {
            out_id
        } else {
            return Err("No node selected or available for search".to_string());
        };

        let node_name = self.nodes.iter().find(|n| n.id == node_id)
            .map(|n| n.name.clone())
            .unwrap_or_else(|| format!("Node #{}", node_id));

        let provider = self.build_data_provider(node_id)?;
        Ok((node_id, node_name, provider))
    }

    /// Build data providers for the Hex Viewer.
    /// Walk upstream from the active OutputViewer and find the first PatternSearch node.
    ///
    /// Returns `Some((virtual_results, pattern_len))` where `virtual_results` contains
    /// `SearchResult` values with **output-stream byte offsets** (i.e. offsets into the
    /// virtual provider that the hex viewer is actually reading):
    ///
    /// * `AllMatchingPages` – offsets are the real source offsets (the filtered provider
    ///   maps source pages 1-to-1, so offsets are identical to the stored results).
    /// * `OnePagePerResult` – each output page has size `bytes_before + pat_len +
    ///   bytes_after`, and the match always starts at `bytes_before` inside that page.
    ///   Virtual offset for result `i` = `i * page_size + bytes_before`.
    ///
    /// Returns `None` when there is no active output or no upstream PatternSearch node
    /// with results.
    pub fn get_upstream_search_results(&self) -> Option<(Vec<crate::search::SearchResult>, usize)> {
        let output_id = self.get_active_output_node_id()?;

        // Walk upstream until we find a PatternSearch node
        let mut current_id = output_id;
        let mut visited = std::collections::HashSet::new();

        let search_node = loop {
            if !visited.insert(current_id) {
                return None; // cycle guard
            }
            let node = self.nodes.iter().find(|n| n.id == current_id)?;
            if matches!(node.kind, WorkflowNodeKind::PatternSearch { .. }) {
                break node;
            }
            // Follow the single upstream connection
            let incoming = self.connections.iter().find(|c| c.to_node == current_id)?;
            current_id = incoming.from_node;
        };

        let (search_pattern, is_hex, case_sensitive, results, output_mode, bytes_before, bytes_after) =
            match &search_node.kind {
                WorkflowNodeKind::PatternSearch {
                    search_pattern,
                    is_hex,
                    case_sensitive,
                    results,
                    output_mode,
                    bytes_before,
                    bytes_after,
                    ..
                } => (search_pattern, *is_hex, *case_sensitive, results, *output_mode, *bytes_before, *bytes_after),
                _ => return None,
            };

        if results.is_empty() {
            return None;
        }

        // Determine pattern byte length
        let mode = if is_hex {
            crate::search::SearchMode::Hex
        } else {
            crate::search::SearchMode::Ascii
        };
        let pat_opts = crate::search::SearchOptions {
            pattern: search_pattern.clone(),
            mode,
            case_sensitive,
            max_matches: 1,
        };
        let pattern_len = crate::search::parse_pattern(&pat_opts)
            .map(|b| b.len())
            .unwrap_or(1);

        let virtual_results = match output_mode {
            SearchOutputMode::AllMatchingPages => {
                // The SearchFilteredDataProvider re-maps pages sequentially but keeps
                // each page at the same *intra-page* offset.  A result at source byte
                // offset `src_off` sits in source page `src_page = src_off / page_len`,
                // and the filtered view assigns that page the index equal to its
                // position in the deduplicated sorted matching_pages list.
                //
                // Build the same dedup-sorted page list the provider uses, find each
                // result's filtered page index, then reconstruct the virtual offset.
                let page_len = {
                    // Peek at the upstream provider's page length via the stored
                    // node connections (no need to rebuild the whole provider).
                    // Walk upstream from the search node to the first Input node.
                    let mut up_id = current_id; // current_id == search_node.id here
                    let mut pl: Option<u64> = None;
                    let mut v2 = std::collections::HashSet::new();
                    loop {
                        if !v2.insert(up_id) { break; }
                        if let Some(inc) = self.connections.iter().find(|c| c.to_node == up_id) {
                            up_id = inc.from_node;
                            if let Some(n) = self.nodes.iter().find(|n| n.id == up_id) {
                                if let WorkflowNodeKind::Input { page_length, .. } = n.kind {
                                    pl = Some(page_length as u64);
                                    break;
                                }
                            }
                        } else {
                            break;
                        }
                    }
                    pl.unwrap_or(1)
                };

                // Build deduplicated, sorted matching page list (same as SearchFilteredDataProvider)
                let mut src_pages: Vec<u64> = results.iter().map(|r| r.byte_offset / page_len).collect();
                src_pages.sort_unstable();
                src_pages.dedup();

                results.iter().map(|r| {
                    let src_page = r.byte_offset / page_len;
                    let in_page = r.byte_offset % page_len;
                    // Binary search for this page in the filtered list
                    let filtered_page = src_pages.partition_point(|&p| p < src_page) as u64;
                    let virtual_offset = filtered_page * page_len + in_page;
                    crate::search::SearchResult {
                        byte_offset: virtual_offset,
                        ..r.clone()
                    }
                }).collect()
            }

            SearchOutputMode::OnePagePerResult => {
                // Page size in the virtual stream
                let page_size = bytes_before + pattern_len as u64 + bytes_after;
                results.iter().enumerate().map(|(i, r)| {
                    let virtual_offset = i as u64 * page_size + bytes_before;
                    crate::search::SearchResult {
                        byte_offset: virtual_offset,
                        ..r.clone()
                    }
                }).collect()
            }
        };

        Some((virtual_results, pattern_len))
    }

    /// Returns (active_output_provider, Option<raw_primary_input_provider>).
    /// When an XorTransform node is present in the active output pipeline,
    /// the raw primary input provider is also returned to allow raw vs XOR diff visualization.
    pub fn build_hex_providers(
        &self,
    ) -> Result<(Arc<Mutex<dyn DumpDataProvider>>, Option<Arc<Mutex<dyn DumpDataProvider>>>), String> {
        let output_id = self.get_active_output_node_id()
            .ok_or_else(|| "No OutputViewer node found in workflow".to_string())?;
        let main_provider = self.build_data_provider(output_id)?;

        // Traverse upstream from active output looking for an XorTransform node
        let mut raw_provider = None;
        let mut current_id = output_id;
        let mut visited = HashSet::new();

        while let Some(incoming) = self.connections.iter().find(|c| c.to_node == current_id) {
            if !visited.insert(current_id) {
                break;
            }
            let from_node = incoming.from_node;
            if let Some(node) = self.nodes.iter().find(|n| n.id == from_node) {
                if matches!(node.kind, WorkflowNodeKind::XorTransform { .. }) {
                    // Found XOR transform! Get its primary input (first incoming connection)
                    let xor_incoming: Vec<usize> = self.connections.iter()
                        .filter(|c| c.to_node == from_node)
                        .map(|c| c.from_node)
                        .collect();
                    if let Some(&first_in) = xor_incoming.first() {
                        if let Ok(raw) = self.build_data_provider(first_in) {
                            raw_provider = Some(raw);
                        }
                    }
                    break;
                }
            }
            current_id = from_node;
        }

        Ok((main_provider, raw_provider))
    }

    /// Write pipeline output to disk for a FileExport node
    pub fn export_node_to_file(&self, export_node_id: usize) -> Result<u64, String> {
        let node = self.nodes.iter().find(|n| n.id == export_node_id)
            .ok_or_else(|| format!("Node #{} not found", export_node_id))?;
        if let WorkflowNodeKind::FileExport { export_path, .. } = &node.kind {
            let provider = self.build_data_provider(export_node_id)?;
            let meta = provider.lock().get_metadata();
            let mut file = fs::File::create(export_path)
                .map_err(|e| format!("Failed to create output file '{}': {}", export_path, e))?;
            let chunk_size = 512 * 1024;
            let mut offset = 0;
            let mut written = 0;
            while offset < meta.size {
                let to_read = ((meta.size - offset).min(chunk_size as u64)) as u32;
                let chunk = provider.lock().read_bytes(offset, to_read)
                    .map_err(|e| format!("Read failed during export: {}", e))?;
                file.write_all(&chunk)
                    .map_err(|e| format!("Write failed during export: {}", e))?;
                offset += chunk.len() as u64;
                written += chunk.len() as u64;
            }
            file.flush().map_err(|e| format!("Flush failed: {}", e))?;
            Ok(written)
        } else {
            Err(format!("Node #{} is not a FileExport node", export_node_id))
        }
    }

    pub fn save_to_file<P: AsRef<Path>>(&self, path: P) -> Result<(), String> {
        let json = serde_json::to_string_pretty(self)
            .map_err(|e| format!("Failed to serialize workflow: {}", e))?;
        fs::write(path, json).map_err(|e| format!("Failed to write file: {}", e))?;
        Ok(())
    }

    pub fn load_from_file<P: AsRef<Path>>(&mut self, path: P) -> Result<(), String> {
        let content = fs::read_to_string(path)
            .map_err(|e| format!("Failed to read file: {}", e))?;
        let mut loaded: WorkflowEditorState = serde_json::from_str(&content)
            .map_err(|e| format!("Failed to parse JSON: {}", e))?;

        // Auto-update Input nodes from cached metadata
        for node in &mut loaded.nodes {
            if let WorkflowNodeKind::Input { file_path, page_length, block_size } = &mut node.kind {
                log::debug!("load_from_file()");
                if let Some((pl, bs)) = load_node_metadata(file_path) {
                    *page_length = pl;
                    *block_size = bs;
                }
            }
        }

        *self = loaded;
        self.status_message = "Workflow successfully loaded.".to_string();
        Ok(())
    }

    /// Execute search on a PatternSearch node using its connected input
    pub fn execute_search_node(&mut self, node_id: usize) -> Result<(), String> {
        let (incoming_id, search_pattern, is_hex, case_sensitive, max_matches) = {
            let node = self.nodes.iter().find(|n| n.id == node_id)
                .ok_or_else(|| format!("Node #{} not found", node_id))?;
            let incoming = self.connections.iter().find(|c| c.to_node == node_id)
                .ok_or_else(|| format!("Search node #{} has no input connected", node_id))?;
            match &node.kind {
                WorkflowNodeKind::PatternSearch { search_pattern, is_hex, case_sensitive, max_matches, .. } => {
                    (incoming.from_node, search_pattern.clone(), *is_hex, *case_sensitive, *max_matches)
                }
                _ => return Err(format!("Node #{} is not a PatternSearch node", node_id)),
            }
        };

        if search_pattern.trim().is_empty() {
            return Err("Search pattern is empty".to_string());
        }

        let upstream_provider = self.build_data_provider(incoming_id)?;
        let mode = if is_hex { crate::search::SearchMode::Hex } else { crate::search::SearchMode::Ascii };
        let options = crate::search::SearchOptions {
            pattern: search_pattern.clone(),
            mode,
            case_sensitive,
            max_matches,
        };

        let cancel_flag = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let progress = Arc::new(std::sync::atomic::AtomicU64::new(0));

        let results = crate::search::search_provider(upstream_provider.clone(), &options, cancel_flag, progress)?;

        let page_len = upstream_provider.lock().get_metadata().page_length as u64;
        let unique_pages = if page_len > 0 {
            let mut pages: Vec<u64> = results.iter().map(|r| r.byte_offset / page_len).collect();
            pages.sort_unstable();
            pages.dedup();
            pages.len()
        } else {
            0
        };

        let match_count = results.len();
        if let Some(node) = self.nodes.iter_mut().find(|n| n.id == node_id) {
            node.status = NodeExecutionStatus::Completed;
            node.output_log = format!(
                "Pattern Search Completed!\nPattern: '{}' (Hex: {})\nMatches found: {} across {} pages (limit {})",
                search_pattern, is_hex, match_count, unique_pages, max_matches
            );
            if let WorkflowNodeKind::PatternSearch { results: node_res, .. } = &mut node.kind {
                *node_res = results;
            }
        }

        self.status_message = format!("Search node #{} found {} matches across {} pages", node_id, match_count, unique_pages);
        Ok(())
    }

    /// Start search on a PatternSearch node in a background thread (non-blocking)
    pub fn start_search_node(&mut self, node_id: usize) -> Result<(), String> {
        let (incoming_id, search_pattern, is_hex, case_sensitive, max_matches, resume_offset, prior_count) = {
            let node = self.nodes.iter().find(|n| n.id == node_id)
                .ok_or_else(|| format!("Node #{} not found", node_id))?;
            let incoming = self.connections.iter().find(|c| c.to_node == node_id)
                .ok_or_else(|| format!("Search node #{} has no input connected", node_id))?;
            match &node.kind {
                WorkflowNodeKind::PatternSearch {
                    search_pattern, is_hex, case_sensitive, max_matches,
                    results, resume_from_offset, ..
                } => {
                    (
                        incoming.from_node,
                        search_pattern.clone(),
                        *is_hex,
                        *case_sensitive,
                        *max_matches,
                        *resume_from_offset,
                        results.len(),
                    )
                }
                _ => return Err(format!("Node #{} is not a PatternSearch node", node_id)),
            }
        };

        if search_pattern.trim().is_empty() {
            return Err("Search pattern is empty".to_string());
        }

        // Cancel previous search on this node if any was running
        self.cancel_search_node(node_id);

        let upstream_provider = self.build_data_provider(incoming_id)?;
        let total_bytes = upstream_provider.lock().get_metadata().size;
        let mode = if is_hex { crate::search::SearchMode::Hex } else { crate::search::SearchMode::Ascii };

        // Clamp max_matches for this run: we only need (max_matches - prior_count) more.
        let remaining = max_matches.saturating_sub(prior_count);
        let options = crate::search::SearchOptions {
            pattern: search_pattern.clone(),
            mode,
            case_sensitive,
            max_matches: remaining,
        };

        let start_offset = resume_offset.unwrap_or(0);

        let cancel_flag = Arc::new(AtomicBool::new(false));
        let progress_bytes = Arc::new(AtomicU64::new(0));
        let (tx, rx) = std::sync::mpsc::channel();
        let (match_tx, match_rx) = std::sync::mpsc::channel::<crate::search::SearchResult>();

        let cancel_thread = cancel_flag.clone();
        let prog_thread = progress_bytes.clone();
        let prov_thread = upstream_provider.clone();
        let index_offset = prior_count;

        std::thread::spawn(move || {
            let res = crate::search::search_provider_from(
                prov_thread,
                &options,
                cancel_thread,
                prog_thread,
                start_offset,
                index_offset,
                Some(match_tx),
            );
            let _ = tx.send(res);
        });

        self.active_searches.lock().insert(
            node_id,
            ActiveSearchState {
                cancel_flag,
                progress_bytes,
                total_bytes,
                rx,
                match_rx,
                live_match_count: 0,
                prior_result_count: prior_count,
            },
        );

        if let Some(node) = self.nodes.iter_mut().find(|n| n.id == node_id) {
            node.status = NodeExecutionStatus::Running;
            let resume_msg = if resume_offset.is_some() {
                format!(" (continuing from {} existing results)", prior_count)
            } else {
                String::new()
            };
            node.output_log = format!("Search started{}…  Pattern: '{}'", resume_msg, search_pattern);
        }
        self.status_message = format!("Search started for '{}' on node #{}…", search_pattern, node_id);
        Ok(())
    }

    /// Check if a background search is running for node_id
    pub fn is_node_searching(&self, node_id: usize) -> bool {
        self.active_searches.lock().contains_key(&node_id)
    }

    /// Cancel any background search running on node_id
    pub fn cancel_search_node(&mut self, node_id: usize) {
        if let Some(search) = self.active_searches.lock().get(&node_id) {
            search.cancel_flag.store(true, Ordering::Relaxed);
        }
    }

    pub fn cancel_all_searches(&mut self) {
        let searches = self.active_searches.lock();
        for search in searches.values() {
            search.cancel_flag.store(true, Ordering::Relaxed);
        }
        let count = searches.len();
        drop(searches);
        if count > 0 {
            self.status_message = format!("Stopping {} active search(es)...", count);
        } else {
            self.status_message = "No active searches to stop.".to_string();
        }
    }

    /// Poll completed background searches and update nodes
    pub fn poll_active_searches(&mut self, ctx: &egui::Context) {        let mut completed = Vec::new();
        let mut any_running = false;

        {
            let mut searches = self.active_searches.lock();
            for (&node_id, search) in searches.iter_mut() {
                any_running = true;
                // Drain live match stream to update the counter in real time.
                while let Ok(_m) = search.match_rx.try_recv() {
                    search.live_match_count += 1;
                }
                if let Ok(res) = search.rx.try_recv() {
                    // Capture the scan offset and cancelled flag at the moment the
                    // thread finished, so we can set resume_from_offset correctly.
                    let scan_offset = search.progress_bytes.load(Ordering::Relaxed);
                    let was_cancelled = search.cancel_flag.load(Ordering::Relaxed);
                    completed.push((node_id, res, search.prior_result_count, scan_offset, was_cancelled));
                }
            }
        }

        if any_running {
            ctx.request_repaint();
        }

        for (node_id, res, _prior_count, scan_offset, was_cancelled) in completed {
            self.active_searches.lock().remove(&node_id);
            let page_len = match self.connections.iter().find(|c| c.to_node == node_id) {
                Some(c) => self.build_data_provider(c.from_node).map(|p| p.lock().get_metadata().page_length as u64).unwrap_or(512),
                None => 512,
            };
            if let Some(node) = self.nodes.iter_mut().find(|n| n.id == node_id) {
                match res {
                    Ok(new_results) => {
                        let hit_limit;
                        let match_count;

                        if let WorkflowNodeKind::PatternSearch {
                            results: node_res,
                            resume_from_offset,
                            max_matches,
                            ..
                        } = &mut node.kind {
                            // Append new results to existing ones.
                            let last_offset = new_results.last().map(|r| r.byte_offset);
                            node_res.extend(new_results);
                            match_count = node_res.len();
                            hit_limit = match_count >= *max_matches;

                            // Determine where to resume from:
                            //  • Limit hit → byte after the last match offset
                            //  • Cancelled  → the scan offset where the thread stopped
                            //  • Reached end → no resume point
                            *resume_from_offset = if hit_limit {
                                last_offset.map(|o| o + 1)
                            } else if was_cancelled {
                                Some(scan_offset)
                            } else {
                                None
                            };
                        } else {
                            match_count = 0;
                            hit_limit = false;
                        }

                        let unique_pages = if page_len > 0 {
                            if let WorkflowNodeKind::PatternSearch { results: node_res, .. } = &node.kind {
                                let mut pages: Vec<u64> = node_res.iter().map(|r| r.byte_offset / page_len).collect();
                                pages.sort_unstable();
                                pages.dedup();
                                pages.len()
                            } else { 0 }
                        } else { 0 };

                        node.status = NodeExecutionStatus::Completed;
                        let limit_note = if was_cancelled {
                            "\n⏹ Search stopped — click ▶ Continue to resume from here."
                        } else if hit_limit {
                            "\n⚠ Limit reached — increase limit and click ▶ Continue to find more."
                        } else {
                            "\n✔ Search reached end of data."
                        };
                        node.output_log = format!(
                            "Pattern Search Completed!\nMatches found: {} across {} pages{}",
                            match_count, unique_pages, limit_note
                        );
                        self.status_message = format!(
                            "Search on node #{} found {} matches in {} pages",
                            node_id, match_count, unique_pages
                        );
                    }
                    Err(e) => {
                        node.status = NodeExecutionStatus::Error(e.clone());
                        node.output_log = format!("Search failed: {}", e);
                        self.status_message = format!("Search failed on node #{}: {}", node_id, e);
                    }
                }
            }
        }
    }

    pub fn run_workflow(&mut self) {
        self.status_message = "Running workflow execution...".to_string();
        let node_ids: Vec<usize> = self.nodes.iter().map(|n| n.id).collect();
        for id in node_ids {
            let kind_tag = self.nodes.iter().find(|n| n.id == id).map(|n| {
                match &n.kind {
                    WorkflowNodeKind::PatternSearch { .. } => "search",
                    WorkflowNodeKind::FileExport { .. } => "export",
                    WorkflowNodeKind::FuseMount { auto_mount, .. } => {
                        if *auto_mount {
                            "fuse_mount"
                        } else {
                            "other"
                        }
                    }
                    _ => "other",
                }
            });
            match kind_tag.as_deref() {
                Some("search") => {
                    if let Err(e) = self.start_search_node(id) {
                        if let Some(node) = self.nodes.iter_mut().find(|n| n.id == id) {
                            node.status = NodeExecutionStatus::Error(e.clone());
                            node.output_log = format!("Search execution failed: {}", e);
                        }
                    }
                }
                Some("export") => {
                    self.execute_export_node(id);
                }
                Some("fuse_mount") => {
                    let _ = self.mount_fuse_node(id);
                }
                _ => {
                    if let Some(node) = self.nodes.iter_mut().find(|n| n.id == id) {
                        node.execute();
                    }
                }
            }
        }
        self.status_message = "Workflow execution completed!".to_string();
    }

    pub fn run_selected_node(&mut self) {
        if let Some(selected_id) = self.selected_node {
            let kind_tag = self.nodes.iter().find(|n| n.id == selected_id).map(|n| {
                match &n.kind {
                    WorkflowNodeKind::PatternSearch { .. } => "search",
                    WorkflowNodeKind::FileExport { .. } => "export",
                    WorkflowNodeKind::FuseMount { .. } => "fuse_mount",
                    _ => "other",
                }
            });
            match kind_tag.as_deref() {
                Some("search") => {
                    if let Err(e) = self.start_search_node(selected_id) {
                        self.status_message = format!("Search execution failed: {}", e);
                        if let Some(node) = self.nodes.iter_mut().find(|n| n.id == selected_id) {
                            node.status = NodeExecutionStatus::Error(e.clone());
                            node.output_log = format!("Search execution failed: {}", e);
                        }
                    }
                }
                Some("export") => {
                    self.execute_export_node(selected_id);
                }
                Some("fuse_mount") => {
                    let _ = self.mount_fuse_node(selected_id);
                }
                _ => {
                    if let Some(node) = self.nodes.iter_mut().find(|n| n.id == selected_id) {
                        let name = node.name.clone();
                        node.execute();
                        self.status_message = format!("Executed node #{} ('{}')", selected_id, name);
                    }
                }
            }
        } else {
            self.status_message = "No node selected to execute!".to_string();
        }
    }

    /// Execute the actual file export for a FileExport node and update its status/log.
    pub fn execute_export_node(&mut self, node_id: usize) {
        match self.export_node_to_file(node_id) {
            Ok(written) => {
                let export_path = self.nodes.iter()
                    .find(|n| n.id == node_id)
                    .and_then(|n| if let WorkflowNodeKind::FileExport { export_path, .. } = &n.kind { Some(export_path.clone()) } else { None })
                    .unwrap_or_default();
                let mb = written as f64 / (1024.0 * 1024.0);
                let msg = format!("Export complete: {:.2} MB written to '{}'", mb, export_path);
                self.status_message = msg.clone();
                if let Some(node) = self.nodes.iter_mut().find(|n| n.id == node_id) {
                    node.status = NodeExecutionStatus::Completed;
                    node.output_log = msg;
                }
            }
            Err(e) => {
                self.status_message = format!("Export failed on node #{}: {}", node_id, e);
                if let Some(node) = self.nodes.iter_mut().find(|n| n.id == node_id) {
                    node.status = NodeExecutionStatus::Error(e.clone());
                    node.output_log = format!("Export failed: {}", e);
                }
            }
        }
    }

    /// Checks if a FuseMount node is currently active and mounted
    pub fn is_fuse_node_mounted(&self, node_id: usize) -> bool {
        let mounts = self.active_fuse_mounts.lock();
        if let Some(mount) = mounts.get(&node_id) {
            mount.is_mounted.load(Ordering::SeqCst)
        } else {
            false
        }
    }

    /// Mounts the virtual FUSE filesystem for a FuseMount node
    pub fn mount_fuse_node(&mut self, node_id: usize) -> Result<PathBuf, String> {
        let node = self.nodes.iter().find(|n| n.id == node_id)
            .ok_or_else(|| format!("Node #{} not found", node_id))?;
        let mount_path_str = match &node.kind {
            WorkflowNodeKind::FuseMount { mount_path, .. } => mount_path.clone(),
            _ => return Err(format!("Node #{} is not a FuseMount node", node_id)),
        };

        if mount_path_str.trim().is_empty() {
            return Err("Mount path cannot be empty".to_string());
        }

        let mount_path = PathBuf::from(&mount_path_str);
        let provider = self.build_data_provider(node_id)?;

        let active_mount = crate::fuse_node::ActiveFuseMount::mount(provider, &mount_path)?;
        self.active_fuse_mounts.lock().insert(node_id, active_mount);
        Ok(mount_path)
    }

    /// Unmounts the virtual FUSE filesystem for a FuseMount node
    pub fn unmount_fuse_node(&mut self, node_id: usize) {
        if let Some(mut mount) = self.active_fuse_mounts.lock().remove(&node_id) {
            mount.unmount();
        }
    }

    /// Render workflow menu bar, toolbar and interactive node graph UI
    pub fn show_ui(&mut self, ctx: &egui::Context) {
        self.poll_active_searches(ctx);

        let mut action_insert_xor: Option<(usize, PathBuf)> = None;
        let mut action_dismiss_xor = false;
        let mut action_run_search: Option<usize> = None;
        let mut action_cancel_search: Option<usize> = None;
        let mut action_export_node: Option<usize> = None;
        let mut action_mount_fuse: Option<usize> = None;
        let mut action_unmount_fuse: Option<usize> = None;

        egui::TopBottomPanel::top("workflow_top_bar").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.heading("Workflow Editor");
                ui.separator();

                if ui.button("💾 Save").clicked() {
                    if let Err(e) = self.save_to_file("workflow.json") {
                        self.status_message = format!("Save failed: {}", e);
                    } else {
                        self.status_message = "Saved to workflow.json!".to_string();
                    }
                }
                if ui.button("📂 Load").clicked() {
                    if let Err(e) = self.load_from_file("workflow.json") {
                        self.status_message = format!("Load failed: {}", e);
                    }
                }
                ui.separator();
                if ui.button("▶ Run All").clicked() {
                    self.run_workflow();
                }
                if ui.button("⚡ Run Selected").clicked() {
                    self.run_selected_node();
                }
                let has_active = !self.active_searches.lock().is_empty();
                if ui.add_enabled(has_active, egui::Button::new("⏹ Stop All")).clicked() {
                    self.cancel_all_searches();
                }
                ui.separator();
                ui.menu_button("➕ Add Node", |ui| {
                    if ui.button("NAND Dump Input").clicked() {
                        self.add_node(WorkflowNodeKind::Input {
                            file_path: "dump.bin".to_string(),
                            page_length: 4096,
                            block_size: 64,
                        });
                        ui.close_menu();
                    }
                    if ui.button("XOR Transform").clicked() {
                        self.add_node(WorkflowNodeKind::XorTransform {
                            pattern_hex: String::new(),
                        });
                        ui.close_menu();
                    }
                    if ui.button("Pattern Search").clicked() {
                        self.add_node(WorkflowNodeKind::PatternSearch {
                            search_pattern: "|Block".to_string(),
                            is_hex: false,
                            case_sensitive: true,
                            max_matches: 100,
                            output_mode: SearchOutputMode::AllMatchingPages,
                            bytes_before: 0,
                            bytes_after: 0,
                            results: Vec::new(),
                            resume_from_offset: None,
                        });
                        ui.close_menu();
                    }
                    if ui.button("Block Arranger").clicked() {
                        self.add_node(WorkflowNodeKind::BlockArranger {
                            grid_width: 16,
                            grid_height: 16,
                            stripe_width: 512,
                        });
                        ui.close_menu();
                    }
                    if ui.button("Output Viewer").clicked() {
                        self.add_node(WorkflowNodeKind::OutputViewer {
                            name: "Viewer".to_string(),
                        });
                        ui.close_menu();
                    }
                    if ui.button("File Export").clicked() {
                        self.add_node(WorkflowNodeKind::FileExport {
                            export_path: "output.bin".to_string(),
                            auto_export: false,
                        });
                        ui.close_menu();
                    }
                    if ui.button("FUSE Mount").clicked() {
                        let mount_path = crate::fuse_node::default_mount_dir_for_node(self.next_node_id)
                            .to_string_lossy()
                            .to_string();
                        self.add_node(WorkflowNodeKind::FuseMount {
                            mount_path,
                            auto_mount: false,
                        });
                        ui.close_menu();
                    }
                    if ui.button("Concatenate").clicked() {
                        self.add_node(WorkflowNodeKind::Concatenate);
                        ui.close_menu();
                    }
                    if ui.button("Modulo Page Filter").clicked() {
                        self.add_node(WorkflowNodeKind::ModuloPageFilter {
                            modulo: 3,
                            offset: 0,
                        });
                        ui.close_menu();
                    }
                    if ui.button("NAND Reader").clicked() {
                        self.add_node(WorkflowNodeKind::NandReader {
                            device_path: String::new(),
                            dump_filename: "01_01.dump".to_string(),
                            page_size: 4096,
                            pages_per_block: 64,
                            num_blocks: 1024,
                        });
                        ui.close_menu();
                    }
                    if ui.button("Pattern Healing").clicked() {
                        self.add_node(WorkflowNodeKind::PatternHealing);
                        ui.close_menu();
                    }
                    if ui.button("Block Selector").clicked() {
                        self.add_node(WorkflowNodeKind::BlockSelector {
                            blocks: String::new(),
                        });
                        ui.close_menu();
                    }
                    if ui.button("Block Permutation").clicked() {
                        self.add_node(WorkflowNodeKind::BlockPermutation { ordered: false });
                        ui.close_menu();
                    }
                });
            });

            // Pending XOR offer banner
            if let Some((offered_node_id, ref xor_path)) = self.pending_xor_offer.clone() {
                ui.separator();
                ui.horizontal(|ui| {
                    let xor_name = xor_path.file_name().and_then(|n| n.to_str()).unwrap_or("xor file");
                    ui.colored_label(
                        egui::Color32::from_rgb(100, 220, 255),
                        format!("⚡ Matching XOR pattern found: {}", xor_name),
                    );
                    if ui.button("➕ Insert XOR Transform").clicked() {
                        action_insert_xor = Some((offered_node_id, xor_path.clone()));
                    }
                    if ui.button("Dismiss").clicked() {
                        action_dismiss_xor = true;
                    }
                });
            }

            ui.separator();
            ui.label(&self.status_message);
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            let painter = ui.painter();

            // Draw connection wires using last-frame node rects so wires connect
            // from the right-center of the source node to the left-center of the
            // target node, rather than a fixed offset from the top-left corner.
            for conn in &self.connections {
                let from_pos = self.nodes.iter().find(|n| n.id == conn.from_node).map(|n| {
                    if let Some(rect) = self.node_rects.get(&n.id) {
                        egui::pos2(rect.max.x, rect.center().y)
                    } else {
                        egui::pos2(n.pos[0] + 200.0, n.pos[1] + 30.0)
                    }
                });
                let to_pos = self.nodes.iter().find(|n| n.id == conn.to_node).map(|n| {
                    if let Some(rect) = self.node_rects.get(&n.id) {
                        egui::pos2(rect.min.x, rect.center().y)
                    } else {
                        egui::pos2(n.pos[0], n.pos[1] + 30.0)
                    }
                });

                if let (Some(p1), Some(p2)) = (from_pos, to_pos) {
                    let dx = (p2.x - p1.x).abs().max(80.0) * 0.5;
                    let cp1 = p1 + egui::vec2(dx, 0.0);
                    let cp2 = p2 - egui::vec2(dx, 0.0);
                    let cubic = egui::epaint::CubicBezierShape::from_points_stroke(
                        [p1, cp1, cp2, p2],
                        false,
                        egui::Color32::TRANSPARENT,
                        egui::Stroke::new(2.5, egui::Color32::from_rgb(100, 180, 255)),
                    );
                    painter.add(cubic);
                }
            }

            // Draw active connection wire during dragging/connection mode
            if let Some(from_id) = self.connecting_from {
                if let Some(from_node) = self.nodes.iter().find(|n| n.id == from_id) {
                    let p1 = if let Some(rect) = self.node_rects.get(&from_node.id) {
                        egui::pos2(rect.max.x, rect.center().y)
                    } else {
                        egui::pos2(from_node.pos[0] + 200.0, from_node.pos[1] + 30.0)
                    };
                    if let Some(pointer_pos) = ui.input(|i| i.pointer.hover_pos()) {
                        let dx = (pointer_pos.x - p1.x).abs().max(80.0) * 0.5;
                        let cp1 = p1 + egui::vec2(dx, 0.0);
                        let cp2 = pointer_pos - egui::vec2(dx, 0.0);
                        let cubic = egui::epaint::CubicBezierShape::from_points_stroke(
                            [p1, cp1, cp2, pointer_pos],
                            false,
                            egui::Color32::TRANSPARENT,
                            egui::Stroke::new(2.0, egui::Color32::YELLOW),
                        );
                        painter.add(cubic);
                    }
                }
            }

            // Render interactive nodes
            let mut delete_id = None;
            let mut connect_target = None;

            // Pre-calculate which nodes are already connected into an XOR node to avoid borrowing self during loop
            let xor_node_ids: std::collections::HashSet<usize> = self.nodes.iter()
                .filter(|n| matches!(n.kind, WorkflowNodeKind::XorTransform { .. }))
                .map(|n| n.id)
                .collect();
            let nodes_connected_to_xor: std::collections::HashSet<usize> = self.connections.iter()
                .filter(|c| xor_node_ids.contains(&c.to_node))
                .map(|c| c.from_node)
                .collect();

            // Pre-collect active-search progress info to avoid borrowing self inside the node window closure (fixes E0499)
            // Maps node_id -> (progress_bytes, total_bytes, live_match_count, prior_result_count)
            let active_search_info: HashMap<usize, (u64, u64, usize, usize)> = {
                let searches = self.active_searches.lock();
                searches.iter()
                    .map(|(&id, s)| (id, (
                        s.progress_bytes.load(Ordering::Relaxed),
                        s.total_bytes,
                        s.live_match_count,
                        s.prior_result_count,
                    )))
                    .collect()
            };

            // Pre-collect active FUSE mounts info to avoid borrowing self inside node closure
            let active_fuse_mounted_ids: std::collections::HashSet<usize> = {
                let mounts = self.active_fuse_mounts.lock();
                mounts.iter()
                    .filter_map(|(&id, m)| if m.is_mounted.load(Ordering::SeqCst) { Some(id) } else { None })
                    .collect()
            };

            // Pre-compute page counts for ModuloPageFilter nodes to avoid re-borrowing self
            // inside the node window closure. Maps node_id -> (output_pages, total_source_pages).
            let modulo_page_counts: HashMap<usize, (u64, u64)> = self.nodes.iter()
                .filter_map(|n| {
                    if let WorkflowNodeKind::ModuloPageFilter { modulo: _, offset: _ } = n.kind {
                        let provider = self.build_data_provider(n.id).ok()?;
                        let guard = provider.lock();
                        let meta = guard.get_metadata();
                        let out_pages = if meta.page_length > 0 { meta.size / meta.page_length as u64 } else { 0 };
                        // total source pages: walk upstream
                        let total_src = self.connections.iter()
                            .find(|c| c.to_node == n.id)
                            .and_then(|c| self.build_data_provider(c.from_node).ok())
                            .map(|p| {
                                let g = p.lock();
                                let m = g.get_metadata();
                                if m.page_length > 0 { m.size / m.page_length as u64 } else { 0 }
                            })
                            .unwrap_or(0);
                        Some((n.id, (out_pages, total_src)))
                    } else {
                        None
                    }
                })
                .collect();

            // Pre-compute block counts for BlockSelector nodes.
            // Maps node_id -> (selected_blocks, total_source_blocks).
            let block_selector_counts: HashMap<usize, (u64, u64)> = self.nodes.iter()
                .filter_map(|n| {
                    if let WorkflowNodeKind::BlockSelector { .. } = n.kind {
                        // total source blocks from upstream
                        let total_src = self.connections.iter()
                            .find(|c| c.to_node == n.id)
                            .and_then(|c| self.build_data_provider(c.from_node).ok())
                            .map(|p| p.lock().get_metadata().total_blocks)
                            .unwrap_or(0);
                        // selected block count from built provider
                        let selected = self.build_data_provider(n.id).ok()
                            .map(|p| p.lock().get_metadata().total_blocks)
                            .unwrap_or(0);
                        Some((n.id, (selected, total_src)))
                    } else {
                        None
                    }
                })
                .collect();

            for node in &mut self.nodes {
                let node_id = node.id;
                let mut pos = egui::pos2(node.pos[0], node.pos[1]);

                let is_selected = self.selected_node == Some(node_id);
                let stroke_color = if self.connecting_from == Some(node_id) {
                    egui::Color32::GREEN
                } else if is_selected {
                    egui::Color32::YELLOW
                } else {
                    egui::Color32::GRAY
                };

                let window_title = match &node.status {
                    NodeExecutionStatus::Idle => format!("⏸ #{} - {}", node.id, node.name),
                    NodeExecutionStatus::Running => format!("⏳ #{} - {}", node.id, node.name),
                    NodeExecutionStatus::Completed => format!("✅ #{} - {}", node.id, node.name),
                    NodeExecutionStatus::Error(_) => format!("❌ #{} - {}", node.id, node.name),
                };

                let win_res = egui::Window::new(&window_title)
                    .current_pos(pos)
                    .default_size([200.0, 140.0])
                    .default_open(!node.collapsed)
                    .frame(egui::Frame::window(ui.style()).stroke(egui::Stroke::new(2.0, stroke_color)))
                    .show(ctx, |ui| {
                        ui.horizontal(|ui| {
                            if ui.button("Select").clicked() {
                                self.selected_node = Some(node_id);
                            }
                            if ui.button("🗑").clicked() {
                                delete_id = Some(node_id);
                            }
                            // Pin connecting controls
                            if let Some(src_id) = self.connecting_from {
                                if src_id != node_id {
                                    if ui.button("📥 In").clicked() {
                                        connect_target = Some(node_id);
                                    }
                                } else {
                                    if ui.button("❌ Cancel").clicked() {
                                        self.connecting_from = None;
                                    }
                                }
                            } else {
                                if ui.button("Out ➔").clicked() {
                                    self.connecting_from = Some(node_id);
                                    self.status_message = format!("Connecting from Node #{}... Click 'In' on target node.", node_id);
                                }
                            }
                        });

                        ui.separator();

                        match &mut node.kind {
                            WorkflowNodeKind::Input { file_path, page_length, block_size } => {
                                ui.label("File Path:");
                                let path_edit = ui.text_edit_singleline(file_path);
                                if path_edit.lost_focus() && path_edit.changed() {
                                    // Auto-load page/block from cache when path changes
                                    log::debug!("Auto-load page/block from cache when path changes");
                                    if let Some((pl, bs)) = load_node_metadata(file_path) {
                                        *page_length = pl;
                                        *block_size = bs;
                                        self.status_message = format!(
                                            "Loaded metadata from cache X: page={} block={}",
                                            pl, bs
                                        );
                                    }
                                }
                                // Browse button
                                if ui.button("📂 Browse").clicked() {
                                    let start_dir = if !file_path.trim().is_empty() {
                                        let p = Path::new(file_path.trim());
                                        if p.is_dir() {
                                            p.to_path_buf()
                                        } else if let Some(parent) = p.parent() {
                                            if !parent.as_os_str().is_empty() && parent.is_dir() {
                                                parent.to_path_buf()
                                            } else {
                                                std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
                                            }
                                        } else {
                                            std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
                                        }
                                    } else {
                                        std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
                                    };
                                    let start_dir = std::fs::canonicalize(&start_dir).unwrap_or(start_dir);
                                    self.file_browser_path.insert(node_id, start_dir);
                                    self.file_browser_open.insert(node_id, true);
                                }
                                ui.separator();
                                ui.horizontal(|ui| {
                                    ui.label("Page:");
                                    let page_drag = ui.add(hex_drag_u32(page_length, 64.0));
                                    ui.label("Block:");
                                    let block_drag = ui.add(hex_drag_u32(block_size, 16.0));
                                    if (page_drag.lost_focus() && page_drag.changed())
                                        || (block_drag.lost_focus() && block_drag.changed())
                                    {
                                        save_node_metadata(file_path, *page_length, *block_size);
                                        self.status_message = format!(
                                            "Saved metadata: page={} block={}",
                                            *page_length, *block_size
                                        );
                                    }
                                });

                                // Auto-check cached metadata on display
                                /*
                                log::debug!("Auto-check cached metadata on display");
                                if let Some((cached_pl, cached_bs)) = load_node_metadata(file_path) {
                                    if *page_length != cached_pl || *block_size != cached_bs {
                                        // If currently at default values (4096 / 64), auto-update immediately
                                        if *page_length == 4096 && *block_size == 64 {
                                            *page_length = cached_pl;
                                            *block_size = cached_bs;
                                        } else {
                                            ui.horizontal(|ui| {
                                                ui.colored_label(egui::Color32::from_rgb(180, 220, 100), format!("Cache: {} / {}", cached_pl, cached_bs));
                                                if ui.small_button("⟳ Reload").clicked() {
                                                    *page_length = cached_pl;
                                                    *block_size = cached_bs;
                                                    self.status_message = format!("Loaded metadata from cache Y: page={} block={}", cached_pl, cached_bs);
                                                }
                                            });
                                        }
                                    }
                                }*/

                                // Auto-detect associated XOR file
                                if let Some(xor_path) = find_associated_xor_file(file_path.as_str()) {
                                    let xor_name = xor_path.file_name().and_then(|n| n.to_str()).unwrap_or("xor file");
                                    let already_connected = nodes_connected_to_xor.contains(&node_id);
                                    if !already_connected {
                                        ui.add_space(2.0);
                                        ui.colored_label(egui::Color32::from_rgb(100, 220, 255), format!("⚡ XOR found: {}", xor_name));
                                        if ui.button("➕ Insert XOR Transform").clicked() {
                                            action_insert_xor = Some((node_id, xor_path.clone()));
                                        }
                                    }
                                }
                            }
                            WorkflowNodeKind::XorTransform { pattern_hex } => {
                                ui.label("XOR Hex Pattern:");
                                ui.text_edit_singleline(pattern_hex);
                            }
                            WorkflowNodeKind::PatternSearch { search_pattern, is_hex, case_sensitive, max_matches, output_mode, bytes_before, bytes_after, results, resume_from_offset } => {
                                ui.label("Pattern:");
                                ui.text_edit_singleline(search_pattern);
                                ui.checkbox(is_hex, "Hex mode");
                                ui.checkbox(case_sensitive, "Case sensitive");
                                ui.horizontal(|ui| {
                                    ui.label("Limit:");
                                    ui.add(hex_drag_usize(max_matches, 1.0));
                                });

                                ui.add_space(4.0);
                                ui.separator();
                                ui.label("Output mode:");
                                ui.horizontal(|ui| {
                                    ui.radio_value(output_mode, SearchOutputMode::AllMatchingPages, "Pages with matches");
                                    ui.radio_value(output_mode, SearchOutputMode::OnePagePerResult, "One page per result");
                                });

                                if *output_mode == SearchOutputMode::OnePagePerResult {
                                    ui.add_space(2.0);
                                    ui.horizontal(|ui| {
                                        ui.label("Bytes before:");
                                        ui.add(
                                            hex_drag_u64(bytes_before, 16.0)
                                                .clamp_range(0u64..=1_000_000u64),
                                        );
                                    });
                                    ui.horizontal(|ui| {
                                        ui.label("Bytes after: ");
                                        ui.add(
                                            hex_drag_u64(bytes_after, 16.0)
                                                .clamp_range(0u64..=1_000_000u64),
                                        );
                                    });
                                }
                                ui.add_space(4.0);
                                if let Some(&(prog_bytes, tot_bytes, live_count, prior_count)) = active_search_info.get(&node_id) {
                                    // ── Running ──────────────────────────────────────────
                                    let ratio = if tot_bytes > 0 {
                                        (prog_bytes as f32 / tot_bytes as f32).clamp(0.0, 1.0)
                                    } else {
                                        0.0
                                    };
                                    let prog_mb = prog_bytes as f64 / (1024.0 * 1024.0);
                                    let tot_mb = tot_bytes as f64 / (1024.0 * 1024.0);
                                    ui.add(egui::ProgressBar::new(ratio)
                                        .text(format!("{:.1}% ({:.1} / {:.1} MB)", ratio * 100.0, prog_mb, tot_mb))
                                        .animate(true));
                                    ui.colored_label(
                                        egui::Color32::from_rgb(100, 210, 255),
                                        format!("⟳ {} matches found so far…", prior_count + live_count),
                                    );
                                    if ui.button("⏹ Stop Search").clicked() {
                                        action_cancel_search = Some(node_id);
                                    }
                                } else {
                                    // ── Idle / Completed ────────────────────────────────
                                    if results.is_empty() {
                                        ui.colored_label(
                                            egui::Color32::from_rgb(180, 180, 180),
                                            "No results yet — click ▶ Start Search",
                                        );
                                    } else {
                                        let mut pages: Vec<u64> = results.iter().map(|r| r.page).collect();
                                        pages.sort_unstable();
                                        pages.dedup();
                                        let limit_hit = resume_from_offset.is_some();
                                        let label_color = if limit_hit {
                                            egui::Color32::from_rgb(255, 200, 80) // amber = limit hit
                                        } else {
                                            egui::Color32::from_rgb(100, 230, 140) // green = complete
                                        };
                                        let status_icon = if limit_hit { "⚠" } else { "✔" };
                                        ui.colored_label(
                                            label_color,
                                            format!("{} {} matches ({} pages)", status_icon, results.len(), pages.len()),
                                        );
                                        if limit_hit {
                                            ui.colored_label(
                                                egui::Color32::from_rgb(255, 200, 80),
                                                "Limit reached — increase limit or click ▶ Continue.",
                                            );
                                        }
                                    }

                                    ui.add_space(2.0);
                                    let can_continue = resume_from_offset.is_some();
                                    ui.horizontal(|ui| {
                                        // Always show a fresh-start button.
                                        if ui.button("🔍 Start Search").on_hover_text("Clear existing results and search from the beginning").clicked() {
                                            // Clear existing results and resume point before starting.
                                            *results = Vec::new();
                                            *resume_from_offset = None;
                                            action_run_search = Some(node_id);
                                        }
                                        // Show Continue only when there's a resume point.
                                        if can_continue {
                                            if ui.button("▶ Continue").on_hover_text("Keep existing results and continue searching from where it was stopped or the limit was reached").clicked() {
                                                action_run_search = Some(node_id);
                                            }
                                        }
                                    });
                                }
                            }
                            WorkflowNodeKind::BlockArranger { grid_width, grid_height, stripe_width } => {
                                ui.horizontal(|ui| {
                                    ui.label("Cols:");
                                    ui.add(hex_drag_u32(grid_width, 1.0));
                                    ui.label("Rows:");
                                    ui.add(hex_drag_u32(grid_height, 1.0));
                                });
                                ui.horizontal(|ui| {
                                    ui.label("Stripe:");
                                    ui.add(hex_drag_u32(stripe_width, 128.0));
                                });
                            }
                            WorkflowNodeKind::OutputViewer { name } => {
                                ui.label("Viewer name:");
                                ui.text_edit_singleline(name);
                            }
                            WorkflowNodeKind::FileExport { export_path, auto_export } => {
                                ui.label("Export Target:");
                                ui.text_edit_singleline(export_path);
                                ui.checkbox(auto_export, "Auto Export on Run");
                                ui.add_space(4.0);
                                if ui.button("💾 Export Now").on_hover_text("Write pipeline output to the target file immediately").clicked() {
                                    action_export_node = Some(node_id);
                                }
                            }
                            WorkflowNodeKind::FuseMount { mount_path, auto_mount } => {
                                ui.label("Mount Point:");
                                ui.text_edit_singleline(mount_path);
                                ui.checkbox(auto_mount, "Auto Mount on Run");
                                ui.add_space(4.0);

                                let is_mounted = active_fuse_mounted_ids.contains(&node_id);
                                if is_mounted {
                                    ui.colored_label(egui::Color32::from_rgb(80, 220, 120), "● Mounted");
                                    ui.horizontal(|ui| {
                                        if ui.button("⏹ Unmount").on_hover_text("Unmount virtual filesystem").clicked() {
                                            action_unmount_fuse = Some(node_id);
                                        }
                                        #[cfg(unix)]
                                        if ui.button("📂 Open").on_hover_text("Open mount folder in file manager").clicked() {
                                            let _ = std::process::Command::new("xdg-open").arg(&*mount_path).spawn();
                                        }
                                    });
                                    ui.label(egui::RichText::new("Exposed: dump.bin, meta.json, blocks/, pages/").italics().weak().size(11.0));
                                } else {
                                    ui.colored_label(egui::Color32::from_rgb(200, 200, 200), "○ Not Mounted");
                                    if ui.button("▶ Mount FUSE").on_hover_text("Mount live pipeline as virtual filesystem").clicked() {
                                        action_mount_fuse = Some(node_id);
                                    }
                                }
                            }
                            WorkflowNodeKind::Concatenate => {
                                // Count how many inputs are connected to this node
                                let input_count = self.connections.iter()
                                    .filter(|c| c.to_node == node_id)
                                    .count();
                                if input_count == 0 {
                                    ui.colored_label(
                                        egui::Color32::from_rgb(255, 180, 60),
                                        "⚠ No inputs connected yet.",
                                    );
                                    ui.label("Connect 2 or more nodes using 'Out ➔' / '📥 In'.");
                                } else if input_count == 1 {
                                    ui.colored_label(
                                        egui::Color32::from_rgb(255, 200, 80),
                                        format!("⚠ {} input connected — need at least 2.", input_count),
                                    );
                                } else {
                                    ui.colored_label(
                                        egui::Color32::from_rgb(100, 230, 140),
                                        format!("✔ {} inputs connected", input_count),
                                    );
                                }
                                ui.add_space(2.0);
                                ui.label(egui::RichText::new(
                                    "Inputs are concatenated in connection order.\nShorter inputs are zero-padded to the longest."
                                ).italics().weak().size(11.0));
                            }
                            WorkflowNodeKind::ModuloPageFilter { modulo, offset } => {
                                ui.horizontal(|ui| {
                                    ui.label("Every");
                                    ui.add(
                                        hex_drag_u64(modulo, 1.0)
                                            .clamp_range(1u64..=100_000u64),
                                    );
                                    ui.label("th page, starting at offset");
                                    ui.add(
                                        hex_drag_u64(offset, 1.0)
                                            .clamp_range(0u64..=100_000u64),
                                    );
                                });
                                ui.add_space(2.0);
                                if let Some(&(out_pages, total)) = modulo_page_counts.get(&node_id) {
                                    ui.colored_label(
                                        egui::Color32::from_rgb(100, 230, 140),
                                        format!("→ {} of {} input pages selected", out_pages, total),
                                    );
                                }
                                ui.add_space(2.0);
                                ui.label(egui::RichText::new(
                                    "Keeps pages at indices: offset, offset+N, offset+2N, …"
                                ).italics().weak().size(11.0));
                            }
                            WorkflowNodeKind::NandReader {
                                device_path,
                                dump_filename,
                                page_size,
                                pages_per_block,
                                num_blocks,
                            } => {
                                // Read-only summary — editing is done in the NAND Reader tab.
                                ui.horizontal(|ui| {
                                    ui.label("Device:");
                                    if device_path.is_empty() {
                                        ui.colored_label(
                                            egui::Color32::from_rgb(255, 180, 60),
                                            "not set — configure in NAND Reader tab",
                                        );
                                    } else {
                                        ui.monospace(device_path.as_str());
                                    }
                                });
                                ui.horizontal(|ui| {
                                    ui.label("Dump file:");
                                    ui.monospace(dump_filename.as_str());
                                });
                                ui.horizontal(|ui| {
                                    ui.label("Page size:");
                                    ui.monospace(format!("{} B", page_size));
                                    ui.label("  Pages/block:");
                                    ui.monospace(format!("{}", pages_per_block));
                                    ui.label("  Blocks:");
                                    ui.monospace(format!("{}", num_blocks));
                                });
                                let total_mb = *page_size as f64
                                    * *pages_per_block as f64
                                    * *num_blocks as f64
                                    / (1024.0 * 1024.0);
                                ui.colored_label(
                                    egui::Color32::from_rgb(160, 200, 255),
                                    format!("Total: {:.1} MB", total_mb),
                                );
                                ui.add_space(2.0);
                                ui.label(
                                    egui::RichText::new(
                                        "Switch to the 'NAND Reader' tab to connect,\nscan chips and start a dump.",
                                    )
                                    .italics()
                                    .weak()
                                    .size(11.0),
                                );
                            }
                            WorkflowNodeKind::PatternHealing => {
                                // Show current page structure segments (read-only summary)
                                let has_structure = self.page_structure.as_ref().map(|ps| {
                                    let g = ps.lock().unwrap();
                                    !g.segments.is_empty()
                                }).unwrap_or(false);

                                if has_structure {
                                    if let Some(ps_arc) = &self.page_structure {
                                        let ps = ps_arc.lock().unwrap();
                                        let data_segs: Vec<_> = ps.segments.iter()
                                            .filter(|s| s.kind == crate::page_structure_tab::SegmentKind::Data)
                                            .collect();
                                        if data_segs.is_empty() {
                                            ui.colored_label(
                                                egui::Color32::from_rgb(255, 180, 60),
                                                "⚠ No DATA segments defined in Page Structure tab.",
                                            );
                                        } else {
                                            ui.colored_label(
                                                egui::Color32::from_rgb(100, 230, 140),
                                                format!("✔ {} DATA segment(s) from page structure", data_segs.len()),
                                            );
                                        }
                                        drop(ps);
                                    }
                                } else {
                                    ui.colored_label(
                                        egui::Color32::from_rgb(255, 180, 60),
                                        "⚠ No page structure set — will heal whole 512-byte pages.",
                                    );
                                    ui.label(
                                        egui::RichText::new("Define segments in the 'Page Structure' tab.")
                                            .italics().weak().size(11.0),
                                    );
                                }

                                ui.add_space(4.0);

                                // Show live healing statistics
                                let live_stats: Option<crate::pattern_healing::HealingStats> = {
                                    let map = self.healing_stats.lock();
                                    map.get(&node_id).map(|a| a.lock().clone())
                                };
                                if let Some(stats) = live_stats {
                                    ui.separator();
                                    ui.label(egui::RichText::new("Healing Statistics").strong());
                                    ui.add_space(2.0);
                                    egui::Grid::new(format!("healing_stats_{}", node_id))
                                        .num_columns(2)
                                        .spacing([8.0, 2.0])
                                        .striped(true)
                                        .show(ui, |ui| {
                                            ui.label("DATA areas:");
                                            ui.label(format!("{}", stats.total_data_areas));
                                            ui.end_row();
                                            ui.label("Fill 0x00:");
                                            ui.colored_label(egui::Color32::from_rgb(100, 200, 255), format!("{}", stats.healed_fill_00));
                                            ui.end_row();
                                            ui.label("Fill 0xFF:");
                                            ui.colored_label(egui::Color32::from_rgb(100, 200, 255), format!("{}", stats.healed_fill_ff));
                                            ui.end_row();
                                            ui.label("Fill 0x77:");
                                            ui.colored_label(egui::Color32::from_rgb(100, 200, 255), format!("{}", stats.healed_fill_77));
                                            ui.end_row();
                                            ui.label("LBA |Block#:");
                                            ui.colored_label(egui::Color32::from_rgb(100, 230, 140), format!("{}", stats.healed_lba));
                                            ui.end_row();
                                            ui.label("P00000 ECC:");
                                            ui.colored_label(egui::Color32::from_rgb(100, 230, 140), format!("{}", stats.healed_p00000));
                                            ui.end_row();
                                            ui.label("Untouched:");
                                            ui.label(format!("{}", stats.untouched));
                                            ui.end_row();
                                            ui.label(egui::RichText::new("Total healed:").strong());
                                            ui.colored_label(
                                                egui::Color32::from_rgb(80, 220, 120),
                                                format!("{}", stats.total_healed()),
                                            );
                                            ui.end_row();
                                        });
                                } else {
                                    ui.label(
                                        egui::RichText::new("Statistics update as pages are read.")
                                            .italics().weak().size(11.0),
                                    );
                                }

                                ui.add_space(2.0);
                                ui.label(
                                    egui::RichText::new(
                                        "Healing runs on-the-fly as pages are requested.\n\
                                         Non-DATA bytes (ECC, SA) are passed through unchanged.",
                                    )
                                    .italics().weak().size(11.0),
                                );
                            }
                            WorkflowNodeKind::BlockSelector { blocks } => {
                                ui.label("Blocks:");
                                let resp = ui.text_edit_singleline(blocks);

                                // Parse feedback
                                if blocks.trim().is_empty() {
                                    ui.colored_label(
                                        egui::Color32::from_rgb(255, 200, 80),
                                        "⚠ Empty — will pass all blocks (auto-filled on connect).",
                                    );
                                } else {
                                    // Quick parse to check validity (use u64::MAX as a loose upper bound)
                                    let _ = resp; // keep borrow happy
                                    let total_src = block_selector_counts.get(&node_id)
                                        .map(|&(_, t)| t)
                                        .unwrap_or(u64::MAX);
                                    let max_bound = if total_src == 0 { u64::MAX } else { total_src.saturating_sub(1) };
                                    match parse_block_spec(blocks, max_bound) {
                                        Some(_) => {
                                            if let Some(&(sel, total)) = block_selector_counts.get(&node_id) {
                                                ui.colored_label(
                                                    egui::Color32::from_rgb(100, 230, 140),
                                                    format!("✔ {} of {} source blocks selected", sel, total),
                                                );
                                            } else {
                                                ui.colored_label(
                                                    egui::Color32::from_rgb(100, 230, 140),
                                                    "✔ Valid spec (connect upstream to see counts)",
                                                );
                                            }
                                        }
                                        None => {
                                            ui.colored_label(
                                                egui::Color32::from_rgb(220, 80, 80),
                                                "✘ Could not parse — use e.g. \"0-199\" or \"0,5,10-20\"",
                                            );
                                        }
                                    }
                                }

                                ui.add_space(2.0);
                                ui.label(
                                    egui::RichText::new(
                                        "Accepts: single numbers (hex/dec), ranges (3-200),\n\
                                         comma-separated lists, or any combination.\n\
                                         Example: \"0,5,10-20,0xFF\"",
                                    )
                                    .italics().weak().size(11.0),
                                );
                            }
                            WorkflowNodeKind::BlockPermutation { ordered } => {
                                ui.checkbox(ordered, "Ordered (lexicographic)");

                                ui.add_space(4.0);
                                if *ordered {
                                    ui.label(
                                        egui::RichText::new(
                                            "Subsets in strict lexicographic order:\n\
                                             0  0,1  0,1,2  0,1,2,3  0,1,3  0,2  …",
                                        )
                                        .italics().weak().size(11.0),
                                    );
                                } else {
                                    ui.label(
                                        egui::RichText::new(
                                            "Subsets grouped by size (singletons first):\n\
                                             0  1  2  3 | 0,1  0,2  0,3  1,2  … | …",
                                        )
                                        .italics().weak().size(11.0),
                                    );
                                }

                                ui.add_space(4.0);
                                ui.colored_label(
                                    egui::Color32::from_rgb(255, 180, 60),
                                    "⚠ Output = 2^N − 1 blocks. Use only with small N.",
                                );
                            }
                        }

                        ui.separator();
                        ui.collapsing("Log Output", |ui| {
                            ui.label(&node.output_log);
                        });
                    });

                if let Some(win_res) = win_res {
                    pos = win_res.response.rect.min;
                    self.node_rects.insert(node_id, win_res.response.rect);
                    // Read back the collapsed state from egui's internal CollapsingState.
                    // The window stores it under area_id.with("collapsing") where area_id = Id::new(title).
                    let collapsing_id = egui::Id::new(&window_title).with("collapsing");
                    let is_open = egui::collapsing_header::CollapsingState::load_with_default_open(
                        ctx, collapsing_id, !node.collapsed,
                    ).is_open();
                    node.collapsed = !is_open;
                }

                node.pos = [pos.x, pos.y];
            }

            // ──────────────────────────────────────────────────────────────────────────
            // File browser windows (one per Input node that has browser open)
            // ──────────────────────────────────────────────────────────────────────────
            let open_browsers: Vec<usize> = self
                .file_browser_open
                .iter()
                .filter_map(|(&id, &open)| if open { Some(id) } else { None })
                .collect();

            // Collect file-selection results outside the borrow
            let mut selected_file: Option<(usize, PathBuf)> = None;
            let mut closed_browser: Option<usize> = None;

            for browser_node_id in open_browsers {
                let mut current_dir = self
                    .file_browser_path
                    .get(&browser_node_id)
                    .cloned()
                    .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));

                // Ensure current_dir exists and is a valid directory; otherwise fallback to CWD
                if current_dir.as_os_str().is_empty() || !current_dir.is_dir() {
                    current_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
                    current_dir = std::fs::canonicalize(&current_dir).unwrap_or(current_dir);
                    self.file_browser_path.insert(browser_node_id, current_dir.clone());
                }

                let window_title = format!("📂 Select File — Node #{}", browser_node_id);
                let mut is_open = true;
                egui::Window::new(window_title)
                    .open(&mut is_open)
                    .default_size([480.0, 360.0])
                    .resizable(true)
                    .show(ctx, |ui| {
                        // Current path bar
                        ui.horizontal(|ui| {
                            ui.label("📁");
                            ui.monospace(current_dir.display().to_string());
                        });
                        // Navigation and filter buttons
                        ui.horizontal(|ui| {
                            if ui.button("⬆ Up").clicked() {
                                if let Some(parent) = current_dir.parent() {
                                    if !parent.as_os_str().is_empty() && parent.is_dir() {
                                        self.file_browser_path.insert(browser_node_id, parent.to_path_buf());
                                    }
                                }
                            }
                            if ui.button("🏠 CWD").clicked() {
                                if let Ok(cwd) = std::env::current_dir() {
                                    let canon = std::fs::canonicalize(&cwd).unwrap_or(cwd);
                                    self.file_browser_path.insert(browser_node_id, canon);
                                }
                            }
                            let show_all = self.file_browser_show_all.entry(browser_node_id).or_insert(false);
                            ui.checkbox(show_all, "Show all files");
                        });
                        ui.separator();

                        let show_all = *self.file_browser_show_all.get(&browser_node_id).unwrap_or(&false);
                        // Read directory entries
                        let entries: Vec<PathBuf> = match std::fs::read_dir(&current_dir) {
                            Ok(rd) => {
                                let mut dirs: Vec<PathBuf> = Vec::new();
                                let mut files: Vec<PathBuf> = Vec::new();
                                for entry in rd.flatten() {
                                    let p = entry.path();
                                    if p.is_dir() {
                                        dirs.push(p);
                                    } else if show_all {
                                        files.push(p);
                                    } else {
                                        // Show common dump/binary extensions
                                        let ext = p.extension()
                                            .and_then(|e| e.to_str())
                                            .unwrap_or("")
                                            .to_lowercase();
                                        if matches!(
                                            ext.as_str(),
                                            "dump" | "dmp" | "bin" | "img" | "raw" | "dd" | "nand" | "xor" | "dat" | "out" | "decoded"
                                        ) {
                                            files.push(p);
                                        }
                                    }
                                }
                                dirs.sort();
                                files.sort();
                                dirs.into_iter().chain(files).collect()
                            }
                            Err(e) => {
                                ui.colored_label(egui::Color32::RED, format!("Cannot read dir: {}", e));
                                if ui.button("🔄 Reset to Working Directory").clicked() {
                                    if let Ok(cwd) = std::env::current_dir() {
                                        let canon = std::fs::canonicalize(&cwd).unwrap_or(cwd);
                                        self.file_browser_path.insert(browser_node_id, canon);
                                    }
                                }
                                Vec::new()
                            }
                        };

                        egui::ScrollArea::vertical().show(ui, |ui| {
                            for entry_path in entries {
                                let is_dir = entry_path.is_dir();
                                let name = entry_path
                                    .file_name()
                                    .and_then(|n| n.to_str())
                                    .unwrap_or("?");
                                let label = if is_dir {
                                    format!("🗁  {}/", name)
                                } else {
                                    format!("📄  {}", name)
                                };
                                let response = ui.selectable_label(false, label);
                                if response.double_clicked() {
                                    if is_dir {
                                        self.file_browser_path.insert(browser_node_id, entry_path.clone());
                                    } else {
                                        selected_file = Some((browser_node_id, entry_path.clone()));
                                        closed_browser = Some(browser_node_id);
                                    }
                                }
                            }
                        });

                        ui.separator();
                        if ui.button("✖ Cancel").clicked() {
                            closed_browser = Some(browser_node_id);
                        }
                    });

                // User closed the window via the X button
                if !is_open {
                    closed_browser = Some(browser_node_id);
                }
            }

            // Apply file selection result
            if let Some((target_node_id, chosen_path)) = selected_file {
                let path_str = chosen_path.to_string_lossy().to_string();
                // Update the matching Input node's file_path
                for node in &mut self.nodes {
                    if node.id == target_node_id {
                        if let WorkflowNodeKind::Input { file_path, page_length, block_size } = &mut node.kind {
                            *file_path = path_str.clone();
                            log::debug!("Auto-load metadata for selected file");
                            if let Some((pl, bs)) = load_node_metadata(&path_str) {
                                *page_length = pl;
                                *block_size = bs;
                                self.status_message = format!(
                                    "Loaded metadata from cache Z: page={} block={}", pl, bs
                                );
                            } else {
                                self.status_message = format!("Selected: {}", path_str);
                            }
                        }
                        break;
                    }
                }
                if let Some(xor_path) = find_associated_xor_file(&chosen_path) {
                    self.pending_xor_offer = Some((target_node_id, xor_path));
                }
            }
            if let Some(id) = closed_browser {
                self.file_browser_open.insert(id, false);
            }

            // Apply target connection
            if let (Some(src_id), Some(tgt_id)) = (self.connecting_from, connect_target) {
                // Prevent duplicate connections
                if !self.connections.iter().any(|c| c.from_node == src_id && c.to_node == tgt_id) {
                    self.connections.push(NodeConnection { from_node: src_id, to_node: tgt_id });
                    self.status_message = format!("Connected Node #{} ➔ Node #{}", src_id, tgt_id);

                    // Auto-fill the blocks spec for a BlockSelector node whose field is still empty.
                    // Check first (immutable borrow), then mutate (mutable borrow).
                    let should_autofill = self.nodes.iter().any(|n| {
                        n.id == tgt_id && matches!(&n.kind, WorkflowNodeKind::BlockSelector { blocks } if blocks.trim().is_empty())
                    });
                    if should_autofill {
                        // Build the upstream provider before borrowing nodes mutably.
                        let total_blocks = self.build_data_provider(src_id)
                            .ok()
                            .map(|p| p.lock().get_metadata().total_blocks)
                            .unwrap_or(0);
                        if total_blocks > 0 {
                            if let Some(tgt_node) = self.nodes.iter_mut().find(|n| n.id == tgt_id) {
                                if let WorkflowNodeKind::BlockSelector { blocks } = &mut tgt_node.kind {
                                    *blocks = format!("0-{}", total_blocks - 1);
                                    self.status_message = format!(
                                        "Connected Node #{} ➔ Node #{} — auto-filled blocks: 0-{}",
                                        src_id, tgt_id, total_blocks - 1
                                    );
                                }
                            }
                        }
                    }
                }
                self.connecting_from = None;
            }

            // Handle deletion
            if let Some(del_id) = delete_id {
                self.unmount_fuse_node(del_id);
                self.nodes.retain(|n| n.id != del_id);
                self.connections.retain(|c| c.from_node != del_id && c.to_node != del_id);
                if self.selected_node == Some(del_id) {
                    self.selected_node = None;
                }
                self.status_message = format!("Deleted node #{}", del_id);
            }
        });

        if let Some((node_id, xor_path)) = action_insert_xor {
            self.insert_xor_for_input(node_id, &xor_path);
            self.pending_xor_offer = None;
        }
        if action_dismiss_xor {
            self.pending_xor_offer = None;
        }
        if let Some(s_node_id) = action_run_search {
            if let Err(e) = self.start_search_node(s_node_id) {
                self.status_message = format!("Search failed: {}", e);
            }
        }
        if let Some(c_node_id) = action_cancel_search {
            self.cancel_search_node(c_node_id);
            self.status_message = format!("Cancelling search on node #{}...", c_node_id);
        }
        if let Some(e_node_id) = action_export_node {
            self.execute_export_node(e_node_id);
        }
        if let Some(m_node_id) = action_mount_fuse {
            match self.mount_fuse_node(m_node_id) {
                Ok(path) => {
                    let msg = format!("FUSE filesystem mounted at '{}'", path.display());
                    self.status_message = msg.clone();
                    if let Some(node) = self.nodes.iter_mut().find(|n| n.id == m_node_id) {
                        node.status = NodeExecutionStatus::Completed;
                        node.output_log = msg;
                    }
                }
                Err(e) => {
                    self.status_message = format!("FUSE mount failed on node #{}: {}", m_node_id, e);
                    if let Some(node) = self.nodes.iter_mut().find(|n| n.id == m_node_id) {
                        node.status = NodeExecutionStatus::Error(e.clone());
                        node.output_log = format!("Mount failed: {}", e);
                    }
                }
            }
        }
        if let Some(u_node_id) = action_unmount_fuse {
            self.unmount_fuse_node(u_node_id);
            let msg = format!("FUSE node #{} unmounted", u_node_id);
            self.status_message = msg.clone();
            if let Some(node) = self.nodes.iter_mut().find(|n| n.id == u_node_id) {
                node.status = NodeExecutionStatus::Idle;
                node.output_log = msg;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;

    #[test]
    fn test_workflow_state_initialization() {
        let state = WorkflowEditorState::new(Some("test_dump.bin"));
        assert_eq!(state.nodes.len(), 2);
        assert_eq!(state.connections.len(), 1);
    }

    #[test]
    fn test_workflow_without_output_viewer() {
        let mut state = WorkflowEditorState::new(None);
        state.nodes.retain(|n| !matches!(n.kind, WorkflowNodeKind::OutputViewer { .. }));
        assert_eq!(state.get_active_output_node_id(), None);
        let res = state.build_active_output_provider();
        match res {
            Err(e) => assert_eq!(e, "No OutputViewer node found in workflow"),
            Ok(_) => panic!("expected Err"),
        }
    }

    #[test]
    fn test_workflow_add_nodes() {
        let mut state = WorkflowEditorState::new(None);
        let count_before = state.nodes.len();

        state.add_node(WorkflowNodeKind::PatternSearch {
            search_pattern: "AA BB CC".to_string(),
            is_hex: true,
            case_sensitive: false,
            max_matches: 50,
            output_mode: SearchOutputMode::AllMatchingPages,
            bytes_before: 0,
            bytes_after: 0,
            results: Vec::new(),
        });

        assert_eq!(state.nodes.len(), count_before + 1);
        assert!(state.selected_node.is_some());
    }

    #[test]
    fn test_workflow_execution() {
        let mut state = WorkflowEditorState::new(Some("dummy.dump"));
        state.run_workflow();
        assert!(state.status_message.contains("completed"));
    }

    #[test]
    fn test_workflow_serialization() {
        let state = WorkflowEditorState::new(Some("sample.bin"));
        let temp_file = NamedTempFile::new().unwrap();
        let path = temp_file.path();

        let save_result = state.save_to_file(path);
        assert!(save_result.is_ok());

        let mut loaded_state = WorkflowEditorState::new(None);
        let load_result = loaded_state.load_from_file(path);
        assert!(load_result.is_ok());
        assert_eq!(loaded_state.nodes.len(), 2);
    }

    #[test]
    fn test_workflow_build_pipeline_and_xor() {
        use std::io::Write;
        let mut file1 = NamedTempFile::new().unwrap();
        let mut file2 = NamedTempFile::new().unwrap();

        // 1 block = 64 pages * 512 bytes = 32768 bytes
        file1.write_all(&vec![0xAA; 32768]).unwrap();
        file1.flush().unwrap();
        file2.write_all(&vec![0x55; 32768]).unwrap();
        file2.flush().unwrap();

        let path1 = file1.path().to_str().unwrap();
        let path2 = file2.path().to_str().unwrap();

        let state = WorkflowEditorState {
            next_node_id: 10,
            nodes: vec![
                WorkflowNode::new_input(1, [0.0, 0.0], path1, 512, 64),
                WorkflowNode::new_input(2, [0.0, 50.0], path2, 512, 64),
                WorkflowNode::new_xor(3, [100.0, 25.0]),
                WorkflowNode::new_output(4, [200.0, 25.0], "Viewer"),
            ],
            connections: vec![
                NodeConnection { from_node: 1, to_node: 3 },
                NodeConnection { from_node: 2, to_node: 3 },
                NodeConnection { from_node: 3, to_node: 4 },
            ],
            selected_node: Some(4),
            connecting_from: None,
            status_message: String::new(),
            file_browser_open: HashMap::new(),
            file_browser_path: HashMap::new(),
            file_browser_show_all: HashMap::new(),
            pending_xor_offer: None,
            active_searches: Arc::new(Mutex::new(HashMap::new())),
            active_fuse_mounts: Arc::new(Mutex::new(HashMap::new())),
            node_rects: HashMap::new(),
            page_structure: None,
            healing_stats_cache: HashMap::new(),
            healing_stats: Arc::new(Mutex::new(HashMap::new())),
        };

        // Build data provider for OutputViewer node 4
        let provider = state.build_data_provider(4).expect("Should build data provider");
        let mut guard = provider.lock();
        let bytes = guard.read_bytes(0, 16).unwrap();
        assert_eq!(bytes.len(), 16);
        // 0xAA ^ 0x55 == 0xFF
        assert!(bytes.iter().all(|&b| b == 0xFF));

        let identity = guard.cache_identity();
        assert!(identity.starts_with("xor_"));
    }

    #[test]
    fn test_workflow_cycle_detection() {
        let state = WorkflowEditorState {
            next_node_id: 5,
            nodes: vec![
                WorkflowNode::new_xor(1, [0.0, 0.0]),
                WorkflowNode::new_xor(2, [100.0, 0.0]),
            ],
            connections: vec![
                NodeConnection { from_node: 1, to_node: 2 },
                NodeConnection { from_node: 2, to_node: 1 },
            ],
            selected_node: Some(1),
            connecting_from: None,
            status_message: String::new(),
            file_browser_open: HashMap::new(),
            file_browser_path: HashMap::new(),
            file_browser_show_all: HashMap::new(),
            pending_xor_offer: None,
            active_searches: Arc::new(Mutex::new(HashMap::new())),
            active_fuse_mounts: Arc::new(Mutex::new(HashMap::new())),
            node_rects: HashMap::new(),
            page_structure: None,
            healing_stats_cache: HashMap::new(),
            healing_stats: Arc::new(Mutex::new(HashMap::new())),
        };

        match state.build_data_provider(1) {
            Err(e) => assert!(e.contains("Cycle detected")),
            Ok(_) => panic!("Expected cycle detection error"),
        }
    }

    #[test]
    fn test_workflow_export_node() {
        use std::io::Write;
        let mut file1 = NamedTempFile::new().unwrap();
        let mut dummy = vec![0x00; 32768];
        dummy[0] = 0x11;
        dummy[1] = 0x22;
        dummy[2] = 0x33;
        dummy[3] = 0x44;
        file1.write_all(&dummy).unwrap();
        file1.flush().unwrap();

        let out_file = NamedTempFile::new().unwrap();
        let out_path = out_file.path().to_str().unwrap().to_string();

        let state = WorkflowEditorState {
            next_node_id: 5,
            nodes: vec![
                WorkflowNode::new_input(1, [0.0, 0.0], file1.path().to_str().unwrap(), 512, 64),
                WorkflowNode {
                    id: 2,
                    name: "Export".to_string(),
                    pos: [100.0, 0.0],
                    kind: WorkflowNodeKind::FileExport {
                        export_path: out_path.clone(),
                        auto_export: false,
                    },
                    status: NodeExecutionStatus::Idle,
                    output_log: String::new(),
                    collapsed: false,
                },
            ],
            connections: vec![
                NodeConnection { from_node: 1, to_node: 2 },
            ],
            selected_node: Some(2),
            connecting_from: None,
            status_message: String::new(),
            file_browser_open: HashMap::new(),
            file_browser_path: HashMap::new(),
            file_browser_show_all: HashMap::new(),
            pending_xor_offer: None,
            active_searches: Arc::new(Mutex::new(HashMap::new())),
            active_fuse_mounts: Arc::new(Mutex::new(HashMap::new())),
            node_rects: HashMap::new(),
            page_structure: None,
            healing_stats_cache: HashMap::new(),
            healing_stats: Arc::new(Mutex::new(HashMap::new())),
        };

        let exported = state.export_node_to_file(2).expect("Export should succeed");
        assert_eq!(exported, 32768);
        let read_back = fs::read(&out_path).unwrap();
        assert_eq!(read_back[0..4], [0x11, 0x22, 0x33, 0x44]);
    }

    #[test]
    fn test_find_associated_xor_file() {
        let temp_dir = tempfile::tempdir().unwrap();
        let dump_path = temp_dir.path().join("my_image.dump");
        let xor_path = temp_dir.path().join("my_image.dump.xor");

        // Write dump file
        fs::write(&dump_path, b"DUMP_DATA_CONTENT").unwrap();

        // No xor file yet -> should return None
        assert_eq!(find_associated_xor_file(&dump_path), None);

        // Create empty xor file (0 bytes) -> should still return None
        fs::write(&xor_path, b"").unwrap();
        assert_eq!(find_associated_xor_file(&dump_path), None);

        // Write non-empty xor file -> should return Some(xor_path)
        fs::write(&xor_path, b"XOR_PATTERN").unwrap();
        let found = find_associated_xor_file(&dump_path);
        assert!(found.is_some());
        assert_eq!(found.unwrap(), xor_path);
    }

    #[test]
    fn test_workflow_auto_detect_xor_creates_4_nodes() {
        use std::io::Write;
        let temp_dir = tempfile::tempdir().unwrap();
        let dump_path = temp_dir.path().join("flash.dump");
        let xor_path = temp_dir.path().join("flash.dump.xor");

        // 32768 bytes = 64 pages * 512 bytes
        let mut dump_file = fs::File::create(&dump_path).unwrap();
        dump_file.write_all(&vec![0xAA; 32768]).unwrap();
        dump_file.flush().unwrap();

        let mut xor_file = fs::File::create(&xor_path).unwrap();
        xor_file.write_all(&vec![0x55; 32768]).unwrap();
        xor_file.flush().unwrap();

        // Initialize state with dump file path
        let dump_path_str = dump_path.to_str().unwrap();
        let state = WorkflowEditorState::new_with_geometry(Some(dump_path_str), Some(512), Some(64));

        // Must automatically create 4 nodes: Input 1, Input 2, XOR 3, OutputViewer 4
        assert_eq!(state.nodes.len(), 4);
        assert_eq!(state.connections.len(), 3);

        // Verify node kinds
        assert!(matches!(state.nodes[0].kind, WorkflowNodeKind::Input { .. }));
        assert!(matches!(state.nodes[1].kind, WorkflowNodeKind::Input { .. }));
        assert!(matches!(state.nodes[2].kind, WorkflowNodeKind::XorTransform { .. }));
        assert!(matches!(state.nodes[3].kind, WorkflowNodeKind::OutputViewer { .. }));

        // Verify active output provider builds properly and produces XOR-transformed data
        let provider = state.build_active_output_provider().expect("Active provider should build");
        let mut guard = provider.lock();
        let bytes = guard.read_bytes(0, 16).unwrap();
        assert_eq!(bytes.len(), 16);
        // 0xAA ^ 0x55 == 0xFF
        assert!(bytes.iter().all(|&b| b == 0xFF));
    }

    #[test]
    fn test_workflow_insert_xor_for_input() {
        let temp_dir = tempfile::tempdir().unwrap();
        let dump_path = temp_dir.path().join("data.dump");
        let xor_path = temp_dir.path().join("data.dump.xor");

        fs::write(&dump_path, &vec![0xCC; 32768]).unwrap();
        fs::write(&xor_path, &vec![0x33; 32768]).unwrap();

        // Start with normal 2-node graph (without auto-detection)
        let mut state = WorkflowEditorState {
            next_node_id: 3,
            nodes: vec![
                WorkflowNode::new_input(1, [100.0, 200.0], dump_path.to_str().unwrap(), 512, 64),
                WorkflowNode::new_output(2, [420.0, 200.0], "ZoomViewer"),
            ],
            connections: vec![
                NodeConnection { from_node: 1, to_node: 2 },
            ],
            selected_node: Some(2),
            connecting_from: None,
            status_message: String::new(),
            file_browser_open: HashMap::new(),
            file_browser_path: HashMap::new(),
            file_browser_show_all: HashMap::new(),
            pending_xor_offer: None,
            active_searches: Arc::new(Mutex::new(HashMap::new())),
            active_fuse_mounts: Arc::new(Mutex::new(HashMap::new())),
            node_rects: HashMap::new(),
            page_structure: None,
            healing_stats_cache: HashMap::new(),
            healing_stats: Arc::new(Mutex::new(HashMap::new())),
        };

        // Insert XOR
        state.insert_xor_for_input(1, &xor_path);

        assert_eq!(state.nodes.len(), 4);
        assert_eq!(state.connections.len(), 3);

        let provider = state.build_active_output_provider().expect("Should build XOR active provider");
        let mut guard = provider.lock();
        let bytes = guard.read_bytes(0, 16).unwrap();
        // 0xCC ^ 0x33 == 0xFF
        assert!(bytes.iter().all(|&b| b == 0xFF));
    }

    #[test]
    fn test_load_node_metadata_sdcard0() {
        let meta = load_node_metadata("sdcard0.dump");
        assert_eq!(meta, Some((55296, 384)));
    }

    #[test]
    fn test_load_from_file_updates_input_nodes_from_metadata() {
        let temp_dir = tempfile::tempdir().unwrap();
        let wf_path = temp_dir.path().join("test_wf.json");

        // Write a workflow JSON with default page 4096 / block 64
        let raw_json = r#"{
            "next_node_id": 3,
            "nodes": [
                {
                    "id": 1,
                    "name": "NAND Dump Input",
                    "pos": [100.0, 200.0],
                    "kind": {
                        "Input": {
                            "file_path": "sdcard0.dump",
                            "page_length": 4096,
                            "block_size": 64
                        }
                    },
                    "status": "Idle",
                    "output_log": ""
                }
            ],
            "connections": [],
            "selected_node": 1,
            "connecting_from": null,
            "status_message": ""
        }"#;
        fs::write(&wf_path, raw_json).unwrap();

        let mut state = WorkflowEditorState::new(None);
        state.load_from_file(&wf_path).expect("Load workflow should succeed");

        // The node's page_length and block_size must be automatically updated to the metadata.json values!
        if let WorkflowNodeKind::Input { page_length, block_size, .. } = state.nodes[0].kind {
            assert_eq!(page_length, 55296);
            assert_eq!(block_size, 384);
        } else {
            panic!("Expected Input node");
        }
    }

    #[test]
    fn test_build_hex_providers_with_and_without_xor() {
        let temp_dir = tempfile::tempdir().unwrap();
        let dump_path = temp_dir.path().join("hex_test.dump");
        let xor_path = temp_dir.path().join("hex_test.dump.xor");

        fs::write(&dump_path, &vec![0xAA; 32768]).unwrap();
        fs::write(&xor_path, &vec![0x55; 32768]).unwrap();

        // 1. Without XOR (Input -> Output)
        let state_simple = WorkflowEditorState {
            next_node_id: 3,
            nodes: vec![
                WorkflowNode::new_input(1, [100.0, 200.0], dump_path.to_str().unwrap(), 512, 64),
                WorkflowNode::new_output(2, [400.0, 200.0], "ZoomViewer"),
            ],
            connections: vec![NodeConnection { from_node: 1, to_node: 2 }],
            selected_node: Some(2),
            connecting_from: None,
            status_message: String::new(),
            file_browser_open: HashMap::new(),
            file_browser_path: HashMap::new(),
            file_browser_show_all: HashMap::new(),
            pending_xor_offer: None,
            active_searches: Arc::new(Mutex::new(HashMap::new())),
            active_fuse_mounts: Arc::new(Mutex::new(HashMap::new())),
            node_rects: HashMap::new(),
            page_structure: None,
            healing_stats_cache: HashMap::new(),
            healing_stats: Arc::new(Mutex::new(HashMap::new())),
        };

        let (main_p, raw_p) = state_simple.build_hex_providers().unwrap();
        assert!(raw_p.is_none());
        assert_eq!(main_p.lock().read_bytes(0, 4).unwrap(), vec![0xAA, 0xAA, 0xAA, 0xAA]);

        // 2. With XOR (Input1 + Input2 -> XOR -> Output)
        let mut state_xor = state_simple;
        state_xor.insert_xor_for_input(1, &xor_path);

        let (xor_main_p, raw_opt) = state_xor.build_hex_providers().unwrap();
        assert!(raw_opt.is_some());
        let raw_p = raw_opt.unwrap();

        // Raw provider has original bytes 0xAA
        assert_eq!(raw_p.lock().read_bytes(0, 4).unwrap(), vec![0xAA, 0xAA, 0xAA, 0xAA]);
        // XOR output has 0xAA ^ 0x55 = 0xFF
        assert_eq!(xor_main_p.lock().read_bytes(0, 4).unwrap(), vec![0xFF, 0xFF, 0xFF, 0xFF]);
    }

    #[test]
    fn test_file_browser_start_dir_resolution() {
        let temp_dir = tempfile::tempdir().unwrap();
        let file_in_temp = temp_dir.path().join("subfile.bin");
        fs::write(&file_in_temp, b"test").unwrap();

        // Helper replicating the start_dir resolution logic
        let resolve_dir = |file_path: &str| -> PathBuf {
            let start_dir = if !file_path.trim().is_empty() {
                let p = Path::new(file_path.trim());
                if p.is_dir() {
                    p.to_path_buf()
                } else if let Some(parent) = p.parent() {
                    if !parent.as_os_str().is_empty() && parent.is_dir() {
                        parent.to_path_buf()
                    } else {
                        std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
                    }
                } else {
                    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
                }
            } else {
                std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
            };
            std::fs::canonicalize(&start_dir).unwrap_or(start_dir)
        };

        // 1. Bare filename like "dump.bin" should NOT resolve to empty path ""
        let d1 = resolve_dir("dump.bin");
        assert!(!d1.as_os_str().is_empty(), "Resolved dir must not be empty string");
        assert!(d1.is_dir(), "Resolved dir must be an existing directory");

        // 2. Empty string should resolve to existing CWD
        let d2 = resolve_dir("");
        assert!(d2.is_dir(), "Empty file path must resolve to existing directory");

        // 3. Non-existent path should resolve to existing CWD
        let d3 = resolve_dir("/non/existent/dir/dump.bin");
        assert!(d3.is_dir(), "Non-existent path must resolve to existing directory");

        // 4. File in temp dir should resolve to that temp dir
        let d4 = resolve_dir(file_in_temp.to_str().unwrap());
        let expected_temp = std::fs::canonicalize(temp_dir.path()).unwrap();
        assert_eq!(d4, expected_temp);
    }

    #[test]
    fn test_workflow_search_node_filtering() {
        let temp_dir = tempfile::tempdir().unwrap();
        let dump_path = temp_dir.path().join("search_filter_test.dump");

        // Create 4 pages of 512 bytes each
        let mut data = vec![0u8; 2048];
        // Page 0 (0..512): "EMPTY PAGE 0"
        data[0..12].copy_from_slice(b"EMPTY PAGE 0");
        // Page 1 (512..1024): contains "|Block data 1"
        data[512..525].copy_from_slice(b"|Block data 1");
        // Page 2 (1024..1536): "EMPTY PAGE 2"
        data[1024..1036].copy_from_slice(b"EMPTY PAGE 2");
        // Page 3 (1536..2048): contains "|Block data 3"
        data[1536..1549].copy_from_slice(b"|Block data 3");

        fs::write(&dump_path, &data).unwrap();

        // Construct graph: Node 1 (Input) -> Node 2 (Search) -> Node 3 (OutputViewer)
        let n1 = WorkflowNode::new_input(1, [100.0, 100.0], dump_path.to_str().unwrap(), 512, 64);
        let n2 = WorkflowNode::new_pattern_search(2, [300.0, 100.0]);
        let n3 = WorkflowNode::new_output(3, [500.0, 100.0], "ZoomViewer");

        let mut state = WorkflowEditorState {
            next_node_id: 4,
            nodes: vec![n1, n2, n3],
            connections: vec![
                NodeConnection { from_node: 1, to_node: 2 },
                NodeConnection { from_node: 2, to_node: 3 },
            ],
            selected_node: Some(3),
            connecting_from: None,
            status_message: String::new(),
            file_browser_open: HashMap::new(),
            file_browser_path: HashMap::new(),
            file_browser_show_all: HashMap::new(),
            pending_xor_offer: None,
            active_searches: Arc::new(Mutex::new(HashMap::new())),
            active_fuse_mounts: Arc::new(Mutex::new(HashMap::new())),
            node_rects: HashMap::new(),
            page_structure: None,
            healing_stats_cache: HashMap::new(),
            healing_stats: Arc::new(Mutex::new(HashMap::new())),
        };

        // 1. Before executing search: filtered view has 0 matching pages (suppresses all pages)
        let initial_provider = state.build_active_output_provider().expect("Build should succeed");
        assert_eq!(initial_provider.lock().get_metadata().size, 0);
        assert_eq!(initial_provider.lock().get_metadata().total_pages, 0);

        // 2. Execute search node (default pattern is "|Block")
        state.execute_search_node(2).expect("Search node execution should succeed");

        // Verify matches found in node
        if let WorkflowNodeKind::PatternSearch { results, .. } = &state.nodes[1].kind {
            assert_eq!(results.len(), 2);
            assert_eq!(results[0].byte_offset, 512);
            assert_eq!(results[1].byte_offset, 1536);
        } else {
            panic!("Expected PatternSearch node");
        }

        // 3. Build active output provider after search
        let filtered_provider = state.build_active_output_provider().expect("Build after search should succeed");
        let meta = filtered_provider.lock().get_metadata();
        // Only 2 pages matched out of 4 -> total size must be 2 * 512 = 1024
        assert_eq!(meta.size, 1024);
        assert_eq!(meta.total_pages, 2);
        assert_eq!(meta.page_length, 512);

        // Verify Page 0 of filtered output corresponds to original Page 1
        let p0_bytes = filtered_provider.lock().read_bytes(0, 13).unwrap();
        assert_eq!(&p0_bytes, b"|Block data 1");

        // Verify Page 1 of filtered output corresponds to original Page 3
        let p1_bytes = filtered_provider.lock().read_bytes(512, 13).unwrap();
        assert_eq!(&p1_bytes, b"|Block data 3");
    }

    #[test]
    fn test_fuse_mount_node_lifecycle_and_provider() {
        let mut tmp = NamedTempFile::new().unwrap();
        let payload = vec![0xABu8; 32768];
        tmp.write_all(&payload).unwrap();
        tmp.flush().unwrap();
        let tmp_path = tmp.path().to_str().unwrap().to_string();

        let mut state = WorkflowEditorState::new(None);
        state.nodes.clear();
        state.connections.clear();

        // 1. Input Node
        state.add_node(WorkflowNodeKind::Input {
            file_path: tmp_path,
            page_length: 512,
            block_size: 64,
        });
        let input_id = state.nodes[0].id;

        // 2. FuseMount Node
        state.add_node(WorkflowNodeKind::FuseMount {
            mount_path: "/tmp/test_fuse_mount".to_string(),
            auto_mount: false,
        });
        let fuse_id = state.nodes[1].id;

        // Connect Input -> FuseMount
        state.connections.push(NodeConnection {
            from_node: input_id,
            to_node: fuse_id,
        });

        // Verify provider can be built from FuseMount node
        let provider = state.build_data_provider(fuse_id).expect("Should build data provider through FuseMount");
        let meta = provider.lock().get_metadata();
        assert_eq!(meta.size, 32768);
        assert_eq!(meta.page_length, 512);
        assert_eq!(meta.block_size, 64);
        assert_eq!(meta.total_pages, 64);

        let bytes = provider.lock().read_bytes(0, 4).unwrap();
        assert_eq!(bytes, vec![0xAB, 0xAB, 0xAB, 0xAB]);

        // Verify mount status checks
        assert!(!state.is_fuse_node_mounted(fuse_id));

        // Test serde serialization roundtrip
        let json = serde_json::to_string(&state).expect("Workflow with FuseMount should serialize");
        let deserialized: WorkflowEditorState = serde_json::from_str(&json).expect("Workflow should deserialize");
        assert_eq!(deserialized.nodes.len(), 2);
        assert!(matches!(deserialized.nodes[1].kind, WorkflowNodeKind::FuseMount { .. }));
    }
}
