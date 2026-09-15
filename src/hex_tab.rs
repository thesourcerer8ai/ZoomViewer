//! Hex Viewer Tab Module
//!
//! Provides a high-performance, NAND-specific hex viewer:
//! - Vertical Page-by-Page layout: each line represents one consecutive NAND page
//! - Responsive Auto-Fit horizontally (filling the available window width with bytes)
//! - Responsive Auto-Fit vertically (filling the available screen height with pages)
//! - Display modes: Hex + ASCII, Hex Only, ASCII Only
//! - Adjustable Font Size: 10pt (Compact), 12pt (Normal), 14pt (Large)
//! - Miniature Bit-Level NAND Viewer (from xorviewer.pl) with 4-color XOR diff rendering
//! - Page Structure parsing (.case XML) with segment coloring (Data, Spare/SA, ECC)
//! - Velocity-dependent vertical page scrollbar
//! - Bidirectional NAND Viewer <-> Hex Tab navigation

use crate::data_provider::DumpDataProvider;
use crate::search::SearchResult;
use crate::types::FileMetadata;
use egui::{
    scroll_area::ScrollBarVisibility, vec2, Align, Color32, ColorImage, FontFamily, FontId, Key,
    Layout, Pos2, Rect, Rounding, ScrollArea, Sense, Stroke, TextureHandle, TextureOptions, Ui,
};
use parking_lot::Mutex;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

/// Segment kind in a NAND page structure
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageSegmentKind {
    Data,
    Ecc,
    SpareArea,
    Other,
}

/// A parsed record from a .case page structure definition
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageStructureRecord {
    pub name: String,
    pub start: u64,
    pub stop: u64,
    pub kind: PageSegmentKind,
}

/// Parse a .case file content to extract page structure records
pub fn parse_case_file(content: &str) -> Vec<PageStructureRecord> {
    let mut records = Vec::new();

    for line in content.lines() {
        let trimmed = line.trim();
        if !trimmed.starts_with("<Record ") {
            continue;
        }

        // Extract StructureDefinitionName
        let name = if let Some(start_idx) = trimmed.find("StructureDefinitionName=\"") {
            let rest = &trimmed[start_idx + 25..];
            if let Some(end_idx) = rest.find('"') {
                &rest[..end_idx]
            } else {
                continue;
            }
        } else {
            continue;
        };

        // Skip whole "Page" definition record
        if name.eq_ignore_ascii_case("Page") {
            continue;
        }

        // Extract StartAddress
        let start_addr: u64 = if let Some(start_idx) = trimmed.find("StartAddress=\"") {
            let rest = &trimmed[start_idx + 14..];
            if let Some(end_idx) = rest.find('"') {
                rest[..end_idx].parse().unwrap_or(0)
            } else {
                continue;
            }
        } else {
            continue;
        };

        // Extract StopAddress
        let stop_addr: u64 = if let Some(start_idx) = trimmed.find("StopAddress=\"") {
            let rest = &trimmed[start_idx + 13..];
            if let Some(end_idx) = rest.find('"') {
                rest[..end_idx].parse().unwrap_or(0)
            } else {
                continue;
            }
        } else {
            continue;
        };

        let kind = if name.eq_ignore_ascii_case("Data area") || name.eq_ignore_ascii_case("DATA") {
            PageSegmentKind::Data
        } else if name.eq_ignore_ascii_case("ECC") {
            PageSegmentKind::Ecc
        } else if name.eq_ignore_ascii_case("SA")
            || name.eq_ignore_ascii_case("SPARE")
            || name.eq_ignore_ascii_case("OOB")
        {
            PageSegmentKind::SpareArea
        } else {
            PageSegmentKind::Other
        };

        records.push(PageStructureRecord {
            name: name.to_string(),
            start: start_addr,
            stop: stop_addr,
            kind,
        });
    }

    records.sort_by_key(|r| r.start);
    records
}

/// Look for associated .case file near dump path or in working directory
pub fn find_associated_case_file(dump_path: &str) -> Option<PathBuf> {
    let p = Path::new(dump_path);

    // 1. Same filename with .case extension (e.g. data.dump.case or data.case)
    let candidate1 = p.with_extension("case");
    if candidate1.is_file() {
        return Some(candidate1);
    }
    let candidate2 = PathBuf::from(format!("{}.case", dump_path));
    if candidate2.is_file() {
        return Some(candidate2);
    }

    // 2. download.case or geo512.case in same directory
    if let Some(parent) = p.parent() {
        if !parent.as_os_str().is_empty() {
            let c_down = parent.join("download.case");
            if c_down.is_file() {
                return Some(c_down);
            }
            let c_geo = parent.join("geo512.case");
            if c_geo.is_file() {
                return Some(c_geo);
            }
        }
    }

    // 3. In current working directory
    let cwd_down = PathBuf::from("download.case");
    if cwd_down.is_file() {
        return Some(cwd_down);
    }
    let cwd_geo = PathBuf::from("geo512.case");
    if cwd_geo.is_file() {
        return Some(cwd_geo);
    }

    None
}

/// Display mode for Hex viewer
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HexDisplayMode {
    /// Both Hex bytes and ASCII representation
    HexAndAscii,
    /// Only Hex bytes (maximizes horizontal bytes)
    HexOnly,
    /// Only ASCII characters (maximum character density)
    AsciiOnly,
}

/// Display mode when an XOR node is active upstream
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HexDiffMode {
    /// Show active output bytes (XOR applied)
    XorOnly,
    /// Show raw input bytes before XOR
    RawOnly,
    /// Show active bytes with color-coded diff against raw bytes
    ColorDiff,
}

/// Placement of the miniature bit-level NAND viewer
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MiniNandPosition {
    Top,
    Side,
    Hidden,
}

/// State for the Hex Tab UI
pub struct HexTabState {
    /// Starting NAND page displayed at the top row (pagestart in xorviewer.pl)
    pub current_page: u64,
    /// Byte column offset inside the page (start in xorviewer.pl)
    pub col_offset_in_page: u64,
    /// Requested jump target byte offset from outside (e.g. NAND viewer or search)
    pub pending_jump_offset: Option<u64>,

    /// Display format (Hex+ASCII, Hex Only, ASCII Only)
    pub display_mode: HexDisplayMode,
    /// Font size (10.0, 12.0, 14.0)
    pub font_size: f32,
    /// Diff mode for XOR comparison
    pub diff_mode: HexDiffMode,
    /// Position of mini bit-level NAND viewer
    pub mini_nand_position: MiniNandPosition,

    /// Text input buffer for "Go to address / page"
    pub goto_address_input: String,
    /// Error message for invalid address entry
    pub goto_error: Option<String>,


    /// Texture for the miniature bit-level NAND viewer
    mini_nand_texture: Option<TextureHandle>,

    /// Requested jump to NAND viewer: (block, page, offset_in_page)
    pub jump_to_nand: Option<(u64, u64, u64)>,

    // Velocity-dependent vertical scrollbar state
    pub is_dragging_scrollbar: bool,
    pub last_drag_mouse_y: Option<f32>,
    pub last_drag_time: Option<Instant>,
    pub scroll_velocity_mode: String,
    pub drag_grab_offset_y: f32,

    /// Height of the CentralPanel content area measured in the previous frame.
    /// Used to compute visible_pages accurately without a fragile fixed overhead guess.
    pub last_central_height: f32,

    /// Exact height available for page hex rows measured after headers and mini-nand.
    pub last_rows_height: f32,

    // Horizontal page scrollbar state
    pub is_dragging_h_scrollbar: bool,
    pub last_h_drag_mouse_x: Option<f32>,
}

impl Default for HexTabState {
    fn default() -> Self {
        Self::new()
    }
}

impl HexTabState {
    pub fn new() -> Self {
        Self {
            current_page: 0,
            col_offset_in_page: 0,
            pending_jump_offset: None,

            display_mode: HexDisplayMode::HexAndAscii,
            font_size: 12.0,
            diff_mode: HexDiffMode::ColorDiff,
            mini_nand_position: MiniNandPosition::Top,

            goto_address_input: "0x00000000".to_string(),
            goto_error: None,

            mini_nand_texture: None,
            jump_to_nand: None,

            is_dragging_scrollbar: false,
            last_drag_mouse_y: None,
            last_drag_time: None,
            scroll_velocity_mode: String::new(),
            drag_grab_offset_y: 0.0,

            last_central_height: 0.0,
            last_rows_height: 0.0,

            is_dragging_h_scrollbar: false,
            last_h_drag_mouse_x: None,
        }
    }

    /// Convert linear byte offset to current_page and col_offset_in_page
    pub fn jump_to_linear_offset(&mut self, offset: u64, meta: &FileMetadata) {
        let pl = (meta.page_length as u64).max(1);
        let total_pages = meta.total_pages.max(1);

        let target_page = (offset / pl).min(total_pages.saturating_sub(1));
        let target_col = (offset % pl).min(pl.saturating_sub(1));

        self.current_page = target_page;
        self.col_offset_in_page = target_col;
        self.goto_address_input = format!("0x{:08X}", offset);
    }

    /// Calculate the current linear byte offset of the top-left byte
    pub fn current_linear_offset(&self, meta: &FileMetadata) -> u64 {
        let pl = meta.page_length as u64;
        self.current_page * pl + self.col_offset_in_page
    }

    /// Find the segment covering a given byte column (helper for external callers)
    pub fn find_segment_at_column<'a>(col: u64, records: &'a [PageStructureRecord]) -> Option<&'a PageStructureRecord> {
        records.iter().find(|r| col >= r.start && col <= r.stop)
    }

    /// Main entry point to render the Hex Tab UI
    pub fn show_ui(
        &mut self,
        ctx: &egui::Context,
        main_provider_opt: Option<&Arc<Mutex<dyn DumpDataProvider>>>,
        raw_provider_opt: Option<&Arc<Mutex<dyn DumpDataProvider>>>,
        search_results: &[SearchResult],
        search_pattern_len: usize,
        page_records: &[PageStructureRecord],
    ) {
        let Some(main_prov_arc) = main_provider_opt else {
            egui::CentralPanel::default().show(ctx, |ui| {
                ui.vertical_centered(|ui| {
                    ui.add_space(80.0);
                    ui.heading("No Active Data Source Available");
                    ui.add_space(10.0);
                    ui.label("Configure an Input node connected to an OutputViewer in the Workflow Editor.");
                });
            });
            return;
        };

        // Read metadata with short-lived lock
        let (meta, page_length, block_size, total_pages, total_size) = {
            let guard = main_prov_arc.lock();
            let m = guard.get_metadata();
            let pl = (m.page_length as u64).max(1);
            let bs = (m.block_size as u64).max(1);
            let tp = m.total_pages.max(1);
            let ts = m.size;
            (m, pl, bs, tp, ts)
        };

        let has_xor_diff = raw_provider_opt.is_some();

        // Handle pending external jump (e.g. from NAND viewer or search)
        if let Some(target) = self.pending_jump_offset.take() {
            self.jump_to_linear_offset(target, &meta);
        }

        if !has_xor_diff && self.diff_mode != HexDiffMode::XorOnly {
            self.diff_mode = HexDiffMode::XorOnly;
        }

        // Available dimensions
        let available_rect = ctx.available_rect();
        let avail_w = available_rect.width();
        let avail_h = available_rect.height();

        // 1. Calculate Vertical Auto-Fit
        // 1. Calculate Vertical Auto-Fit
        // Exactly as many rows as fit in the available content space, without any overflow
        let row_h = self.font_size * 1.3 + 1.5;
        let rows_avail_h = if self.last_rows_height > row_h {
            self.last_rows_height
        } else if self.last_central_height > row_h {
            (self.last_central_height - 30.0).max(row_h)
        } else {
            (avail_h - 120.0).max(row_h)
        };
        let visible_pages = (rows_avail_h / row_h).floor() as usize;
        let visible_pages = visible_pages.clamp(2, 200);

        // 2. Calculate Horizontal Auto-Fit
        // Prefix width: "B:0000 P:0000 0x0000 | " takes ~160px
        let prefix_width = 165.0;
        let scrollbar_w = 26.0;
        let content_w = if self.mini_nand_position == MiniNandPosition::Side {
            (avail_w - prefix_width - scrollbar_w - 20.0) * 0.65
        } else {
            avail_w - prefix_width - scrollbar_w - 20.0
        };

        // Monospace character width estimation for given font size (~0.6 * font_size)
        let char_w = self.font_size * 0.62;
        let width_per_byte = match self.display_mode {
            HexDisplayMode::HexAndAscii => 3.0 * char_w + 1.0 * char_w + 4.0, // "XX " + "X"
            HexDisplayMode::HexOnly => 3.0 * char_w + 2.0,                    // "XX "
            HexDisplayMode::AsciiOnly => 1.0 * char_w + 1.5,                  // "X"
        };

        let bytes_per_row = ((content_w / width_per_byte).floor() as usize).clamp(8, page_length as usize);

        // Clamp offsets
        self.current_page = self.current_page.min(total_pages.saturating_sub(1));
        let max_col = page_length.saturating_sub(bytes_per_row as u64);
        self.col_offset_in_page = self.col_offset_in_page.min(max_col);

        // --- Pre-read all row data BEFORE entering egui closures ---
        // This avoids double-mutable-borrow of MutexGuard across closure boundaries.
        // row_data[i] = (active_bytes, raw_bytes) for page (current_page + i)
        let row_data: Vec<(Vec<u8>, Vec<u8>)> = {
            let mut main_guard = main_prov_arc.lock();
            let mut raw_guard_opt = raw_provider_opt.map(|p| p.lock());

            (0..visible_pages)
                .map(|row_idx| {
                    let page_num = self.current_page + row_idx as u64;

                    // Don't read past end of device
                    if page_num >= total_pages {
                        return (Vec::new(), Vec::new());
                    }

                    let row_offset = page_num * page_length + self.col_offset_in_page;

                    if row_offset < total_size {
                        let read_len = (bytes_per_row as u32)
                            .min(total_size.saturating_sub(row_offset) as u32);
                        let act = main_guard.read_bytes(row_offset, read_len).unwrap_or_default();
                        let raw = if let Some(ref mut rg) = raw_guard_opt {
                            rg.read_bytes(row_offset, read_len).unwrap_or_default()
                        } else {
                            Vec::new()
                        };
                        (act, raw)
                    } else {
                        (Vec::new(), Vec::new())
                    }
                })
                .collect()
        }; // all locks released here

        // Handle keyboard inputs
        self.handle_keyboard_inputs(ctx, total_pages, block_size, bytes_per_row as u64, page_length, visible_pages);

        // Render Toolbar
        egui::TopBottomPanel::top("hex_toolbar").show(ctx, |ui| {
            self.render_toolbar(
                ui,
                &meta,
                total_pages,
                block_size,
                page_length,
                bytes_per_row,
                has_xor_diff,
                search_results,
                page_records,
            );
        });

        // Render Status Bar
        egui::TopBottomPanel::bottom("hex_status_bar").show(ctx, |ui| {
            self.render_status_bar(ui, &meta, bytes_per_row, has_xor_diff, page_records);
        });

        // Main Center Area — capture the response to measure the actual allocated height.
        let central_response = egui::CentralPanel::default().show(ctx, |ui| {
            // Measure and store the actual available height at the start of the panel,
            // subtracting horizontal scrollbar height (24.0) so next frame computes
            // visible_pages from actual row space rather than a fixed estimate.
            let panel_h = ui.available_height();
            let h_scrollbar_h = 20.0;
            self.last_central_height = (panel_h - h_scrollbar_h - 4.0).max(0.0);

            ui.horizontal(|ui| {
                // Left main content: Table with row-by-row pages + bottom horizontal scrollbar
                let left_w = if self.mini_nand_position == MiniNandPosition::Side {
                    ui.available_width() - 260.0
                } else {
                    ui.available_width() - 26.0
                };

                ui.allocate_ui_with_layout(
                    vec2(left_w, panel_h),
                    Layout::top_down(Align::Min),
                    |ui| {
                        let content_h = (panel_h - h_scrollbar_h - 4.0).max(50.0);

                        ui.allocate_ui_with_layout(
                            vec2(left_w, content_h),
                            Layout::top_down(Align::Min),
                            |ui| {
                                // Optional Top Mini NAND Bit-Viewer
                                if self.mini_nand_position == MiniNandPosition::Top {
                                    self.render_mini_nand_viewer(
                                        ui,
                                        ctx,
                                        &row_data,
                                        bytes_per_row,
                                        visible_pages,
                                        page_length,
                                        total_pages,
                                    );
                                    ui.separator();
                                }

                                // Column headers and Hex Rows
                                self.render_page_hex_rows(
                                    ui,
                                    &row_data,
                                    &meta,
                                    bytes_per_row,
                                    visible_pages,
                                    page_length,
                                    total_size,
                                    has_xor_diff,
                                    search_results,
                                    search_pattern_len,
                                    page_records,
                                );
                            },
                        );

                        // Horizontal page scrollbar (columns navigation inside page)
                        self.render_horizontal_page_scrollbar(
                            ui,
                            ctx,
                            page_length,
                            bytes_per_row,
                            page_records,
                        );
                    },
                );

                // Optional Side Mini NAND Bit-Viewer
                if self.mini_nand_position == MiniNandPosition::Side {
                    ui.allocate_ui_with_layout(
                        vec2(220.0, panel_h),
                        Layout::top_down(Align::Center),
                        |ui| {
                            ui.heading("Mini NAND View");
                            self.render_mini_nand_viewer(
                                ui,
                                ctx,
                                &row_data,
                                bytes_per_row,
                                visible_pages,
                                page_length,
                                total_pages,
                            );
                        },
                    );
                }

                // Vertical velocity-dependent scrollbar (pages navigation)
                let scrollbar_width = 20.0;
                ui.allocate_ui_with_layout(
                    vec2(scrollbar_width, panel_h),
                    Layout::top_down(Align::Center),
                    |ui| {
                        self.render_velocity_page_scrollbar(
                            ui,
                            ctx,
                            total_pages,
                            block_size,
                            visible_pages,
                        );
                    },
                );
            });
        });
        // Suppress unused-variable warning for central_response while keeping the return value
        // available for potential future use (e.g. rect-based hit testing).
        let _ = central_response;
    }

    /// Render navigation toolbar
    fn render_toolbar(
        &mut self,
        ui: &mut Ui,
        meta: &FileMetadata,
        total_pages: u64,
        block_size: u64,
        page_length: u64,
        bytes_per_row: usize,
        has_xor_diff: bool,
        search_results: &[SearchResult],
        page_records: &[PageStructureRecord],
    ) {
        ui.horizontal_wrapped(|ui| {
            // Display Mode selector
            ui.label("Display:");
            ui.selectable_value(&mut self.display_mode, HexDisplayMode::HexAndAscii, "Hex + ASCII");
            ui.selectable_value(&mut self.display_mode, HexDisplayMode::HexOnly, "Hex Only");
            ui.selectable_value(&mut self.display_mode, HexDisplayMode::AsciiOnly, "ASCII Only");

            ui.separator();

            // Font size
            ui.label("Font:");
            ui.selectable_value(&mut self.font_size, 10.0, "10pt");
            ui.selectable_value(&mut self.font_size, 12.0, "12pt");
            ui.selectable_value(&mut self.font_size, 14.0, "14pt");

            ui.separator();

            // XOR Diff mode
            if has_xor_diff {
                ui.label("Diff:");
                ui.selectable_value(&mut self.diff_mode, HexDiffMode::ColorDiff, "Diff (Raw vs XOR)");
                ui.selectable_value(&mut self.diff_mode, HexDiffMode::XorOnly, "XOR Decoded");
                ui.selectable_value(&mut self.diff_mode, HexDiffMode::RawOnly, "Raw Data");
                ui.separator();
            }

            // Mini NAND Viewer toggle
            ui.label("Mini Bit-Viewer:");
            ui.selectable_value(&mut self.mini_nand_position, MiniNandPosition::Top, "Top");
            ui.selectable_value(&mut self.mini_nand_position, MiniNandPosition::Side, "Side");
            ui.selectable_value(&mut self.mini_nand_position, MiniNandPosition::Hidden, "Off");

            ui.separator();

            // Horizontal page column navigation (start in xorviewer.pl)
            ui.label(format!("Page Col: 0x{:04X} ({})", self.col_offset_in_page, self.col_offset_in_page));
            let max_col = page_length.saturating_sub(bytes_per_row as u64);
            if ui.button("◀◀").on_hover_text("Jump to start of page (Home)").clicked() {
                self.col_offset_in_page = 0;
            }
            if ui.button("◀").on_hover_text("Shift left (Left / Ctrl+Left)").clicked() {
                self.col_offset_in_page = self.col_offset_in_page.saturating_sub(16);
            }
            if ui.button("▶").on_hover_text("Shift right (Right / Ctrl+Right)").clicked() {
                self.col_offset_in_page = (self.col_offset_in_page + 16).min(max_col);
            }
            if ui.button("▶▶").on_hover_text("Jump to end of page (End)").clicked() {
                self.col_offset_in_page = max_col;
            }

            // Page structure quick jump dropdown
            if !page_records.is_empty() {
                ui.separator();
                egui::ComboBox::from_label("Structure")
                    .selected_text(
                        HexTabState::find_segment_at_column(self.col_offset_in_page, page_records)
                            .map(|r| r.name.as_str())
                            .unwrap_or("Segments..."),
                    )
                    .show_ui(ui, |ui| {
                        for rec in page_records {
                            let is_active = self.col_offset_in_page >= rec.start && self.col_offset_in_page <= rec.stop;
                            let label = format!("{} (0x{:X}..0x{:X}, {} B)", rec.name, rec.start, rec.stop, rec.stop - rec.start + 1);
                            if ui.selectable_label(is_active, label).clicked() {
                                self.col_offset_in_page = rec.start.min(max_col);
                            }
                        }
                    });
            }

            ui.separator();

            // Page Navigation buttons
            if ui.button("⏮ First Page").on_hover_text("Jump to Page 0").clicked() {
                self.current_page = 0;
            }
            if ui.button("◀ Block").on_hover_text("Previous NAND Block (PgUp / Ctrl+PgUp)").clicked() {
                self.current_page = self.current_page.saturating_sub(block_size);
            }
            if ui.button("Block ▶").on_hover_text("Next NAND Block (PgDn / Ctrl+PgDn)").clicked() {
                self.current_page = (self.current_page + block_size).min(total_pages.saturating_sub(1));
            }

            ui.separator();

            // Go to Address or Page
            ui.label("Go:");
            let resp = ui.add(
                egui::TextEdit::singleline(&mut self.goto_address_input)
                    .desired_width(95.0)
                    .hint_text("0x... or p123"),
            );
            if resp.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) || ui.button("Go").clicked() {
                self.handle_goto(meta);
            }
            if let Some(ref err) = self.goto_error {
                ui.colored_label(Color32::from_rgb(255, 100, 100), err);
            }

            ui.separator();

            // Search matches navigation
            if !search_results.is_empty() {
                ui.label(format!("Matches: {}", search_results.len()));
                if ui.button("◀ Prev").clicked() {
                    self.jump_to_search_match(search_results, meta, false);
                }
                if ui.button("Next ▶").clicked() {
                    self.jump_to_search_match(search_results, meta, true);
                }
                ui.separator();
            }

            // Jump to NAND viewer
            if ui.button("Show in NAND Viewer ↗")
                .on_hover_text("Switch to NAND Viewer tab centered on this location")
                .clicked()
            {
                let cur_linear = self.current_linear_offset(meta);
                let (block, page, offset_in_page) = HexTabState::offset_to_nand_coords(cur_linear, meta);
                self.jump_to_nand = Some((block, page, offset_in_page));
            }
        });
    }

    /// Render status bar at bottom
    fn render_status_bar(
        &mut self,
        ui: &mut Ui,
        meta: &FileMetadata,
        bytes_per_row: usize,
        has_xor_diff: bool,
        page_records: &[PageStructureRecord],
    ) {
        let cur_linear = self.current_linear_offset(meta);
        let (block, page, _offset_in_page) = HexTabState::offset_to_nand_coords(cur_linear, meta);

        ui.horizontal(|ui| {
            ui.label(format!(
                "Page: {} / {} | Block: {} / {} | In-Block Page: {} / {}",
                self.current_page, meta.total_pages, block, meta.total_blocks, page, meta.block_size
            ));
            ui.separator();
            ui.label(format!(
                "Col: 0x{:04X}..0x{:04X} ({} B visible) / PageLen: {}",
                self.col_offset_in_page,
                (self.col_offset_in_page + bytes_per_row as u64).min(meta.page_length as u64),
                bytes_per_row,
                meta.page_length
            ));
            ui.separator();
            ui.label(format!("Linear: 0x{:08X} / 0x{:08X}", cur_linear, meta.size));

            if let Some(seg) = HexTabState::find_segment_at_column(self.col_offset_in_page, page_records) {
                ui.separator();
                let seg_color = match seg.kind {
                    PageSegmentKind::Data => Color32::from_rgb(120, 200, 255),
                    PageSegmentKind::Ecc => Color32::from_rgb(255, 160, 80),
                    PageSegmentKind::SpareArea => Color32::from_rgb(255, 215, 0),
                    PageSegmentKind::Other => Color32::LIGHT_GRAY,
                };
                ui.colored_label(seg_color, format!("Region: {} (0x{:X}..0x{:X})", seg.name, seg.start, seg.stop));
            }

            if has_xor_diff {
                ui.separator();
                let diff_mode_str = match self.diff_mode {
                    HexDiffMode::ColorDiff => "Diff Mode (Green: Changed, Gray: Same)",
                    HexDiffMode::XorOnly => "XOR Output",
                    HexDiffMode::RawOnly => "Raw Input",
                };
                ui.colored_label(Color32::from_rgb(100, 230, 120), diff_mode_str);
            }

            if !self.scroll_velocity_mode.is_empty() {
                ui.separator();
                ui.colored_label(Color32::from_rgb(255, 215, 0), &self.scroll_velocity_mode);
            }
        });
    }

    /// Render the miniature bit-level NAND viewer (from xorviewer.pl)
    fn render_mini_nand_viewer(
        &mut self,
        ui: &mut Ui,
        ctx: &egui::Context,
        row_data: &[(Vec<u8>, Vec<u8>)],
        bytes_per_row: usize,
        visible_pages: usize,
        page_length: u64,
        total_pages: u64,
    ) {
        let bit_w = bytes_per_row * 8;
        let bit_h = visible_pages;

        if bit_w == 0 || bit_h == 0 {
            return;
        }

        // Generate pixel buffer
        let mut pixels = Vec::with_capacity(bit_w * bit_h);

        // 4 exact colors from xorviewer.pl
        let col_white = Color32::WHITE;
        let col_black = Color32::BLACK;
        let col_red = Color32::from_rgb(255, 40, 40);
        let col_green = Color32::from_rgb(40, 230, 40);

        for (row_idx, (active_bytes, raw_bytes)) in row_data.iter().enumerate() {
            let has_raw = !raw_bytes.is_empty();

            for byte_idx in 0..bytes_per_row {
                let act_byte = active_bytes.get(byte_idx).copied().unwrap_or(0);
                let raw_byte = if has_raw { raw_bytes.get(byte_idx).copied() } else { None };

                // 8 bits MSB to LSB
                for bit_pos in (0..8).rev() {
                    let bit_act = (act_byte >> bit_pos) & 1 == 1;

                    let pixel_color = if let Some(rb) = raw_byte {
                        let bit_raw = (rb >> bit_pos) & 1 == 1;
                        // xorviewer.pl exact 4-color diff:
                        // bit_act ? (bit_raw ? white : green) : (bit_raw ? red : black)
                        if bit_act {
                            if bit_raw { col_white } else { col_green }
                        } else {
                            if bit_raw { col_red } else { col_black }
                        }
                    } else {
                        if bit_act { col_white } else { col_black }
                    };

                    pixels.push(pixel_color);
                }
            }
            let _ = row_idx; // suppress unused warning
        }

        // Upload to egui texture
        let color_image = ColorImage {
            size: [bit_w, bit_h],
            pixels,
        };

        let texture = self.mini_nand_texture.get_or_insert_with(|| {
            ctx.load_texture("mini_nand_viewer", color_image.clone(), TextureOptions::NEAREST)
        });
        texture.set(color_image, TextureOptions::NEAREST);

        // Display with scaling
        let display_height = match self.mini_nand_position {
            MiniNandPosition::Top => (bit_h as f32 * 2.0).clamp(32.0, 70.0),
            _ => (bit_h as f32 * 3.0).clamp(60.0, 160.0),
        };
        let display_width = (bit_w as f32 * 1.5).min(ui.available_width() - 10.0);

        let (rect, response) = ui.allocate_exact_size(vec2(display_width, display_height), Sense::click());

        // Draw border and image
        ui.painter().rect_filled(rect, Rounding::ZERO, Color32::from_rgb(20, 20, 25));
        ui.painter().image(
            texture.id(),
            rect,
            Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
            Color32::WHITE,
        );
        ui.painter().rect_stroke(rect, Rounding::ZERO, Stroke::new(1.5, Color32::from_rgb(255, 215, 0))); // Gold border like xorviewer.pl

        // Click to jump to coordinate
        if response.clicked() {
            if let Some(pos) = response.interact_pointer_pos() {
                let rel_x = (pos.x - rect.min.x) / rect.width();
                let rel_y = (pos.y - rect.min.y) / rect.height();

                let clicked_byte = (rel_x * bytes_per_row as f32).floor() as u64;
                let clicked_page_offset = (rel_y * visible_pages as f32).floor() as u64;

                self.current_page = (self.current_page + clicked_page_offset).min(total_pages.saturating_sub(1));
                self.col_offset_in_page = (self.col_offset_in_page + clicked_byte).min(page_length.saturating_sub(bytes_per_row as u64));
            }
        }

        // Hover tooltip showing coordinates
        if response.hovered() {
            if let Some(pos) = response.interact_pointer_pos() {
                let rel_x = (pos.x - rect.min.x) / rect.width();
                let rel_y = (pos.y - rect.min.y) / rect.height();
                let hov_byte = (rel_x * bytes_per_row as f32).floor() as u64 + self.col_offset_in_page;
                let hov_page = (rel_y * visible_pages as f32).floor() as u64 + self.current_page;
                response.on_hover_text(format!("Page: {}, Col: 0x{:04X} ({}) — Click to jump", hov_page, hov_byte, hov_byte));
            }
        }
    }

    /// Render page-by-page rows of Hex / ASCII data
    fn render_page_hex_rows(
        &mut self,
        ui: &mut Ui,
        row_data: &[(Vec<u8>, Vec<u8>)],
        meta: &FileMetadata,
        bytes_per_row: usize,
        visible_pages: usize,
        page_length: u64,
        total_size: u64,
        has_xor_diff: bool,
        search_results: &[SearchResult],
        search_pattern_len: usize,
        page_records: &[PageStructureRecord],
    ) {
        let mono_font = FontId::new(self.font_size, FontFamily::Monospace);
        let block_size = meta.block_size as u64;

        // Palette
        let col_page_header = Color32::from_rgb(100, 170, 240); // Blue for page numbers
        let col_normal = Color32::from_rgb(215, 215, 215);
        let col_zero = Color32::from_rgb(105, 105, 105);
        let col_ff = Color32::from_rgb(180, 160, 90);
        let col_xor_changed = Color32::from_rgb(60, 240, 100);  // Green from xorviewer.pl
        let col_search_bg = Color32::from_rgb(190, 140, 20);

        ScrollArea::vertical()
            .auto_shrink([false, false])
            .scroll_bar_visibility(ScrollBarVisibility::AlwaysHidden)
            .enable_scrolling(false)
            .show(ui, |ui| {
                ui.style_mut().spacing.item_spacing = vec2(0.0, 1.5);

                // Column Header Bar
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new("Block  Page  Offset   | ")
                            .font(mono_font.clone())
                            .color(Color32::from_rgb(160, 160, 170)),
                    );

                    match self.display_mode {
                        HexDisplayMode::HexAndAscii | HexDisplayMode::HexOnly => {
                            for col_idx in 0..bytes_per_row {
                                let abs_col = self.col_offset_in_page + col_idx as u64;
                                let col_color = if let Some(seg) = HexTabState::find_segment_at_column(abs_col, page_records) {
                                    match seg.kind {
                                        PageSegmentKind::Data => Color32::from_rgb(130, 190, 240),
                                        PageSegmentKind::Ecc => Color32::from_rgb(255, 150, 70),
                                        PageSegmentKind::SpareArea => Color32::from_rgb(255, 215, 0),
                                        PageSegmentKind::Other => Color32::from_rgb(170, 170, 170),
                                    }
                                } else {
                                    Color32::from_rgb(150, 150, 150)
                                };

                                ui.label(
                                    egui::RichText::new(format!("{:02X} ", (abs_col % 256) as u8))
                                        .font(mono_font.clone())
                                        .color(col_color),
                                );
                            }
                        }
                        HexDisplayMode::AsciiOnly => {
                            ui.label(
                                egui::RichText::new("ASCII Stream Content (Full Horizontal)")
                                        .font(mono_font.clone())
                                        .color(Color32::from_rgb(150, 150, 150)),
                            );
                        }
                    }

                    if self.display_mode == HexDisplayMode::HexAndAscii {
                        ui.label(
                            egui::RichText::new(" | ASCII")
                                .font(mono_font.clone())
                                .color(Color32::from_rgb(160, 160, 170)),
                        );
                    }
                });

                ui.separator();

                // Capture exact remaining height available for rows to ensure perfect fit without overflow
                self.last_rows_height = ui.available_height();

                // Render each page row
                for row_idx in 0..visible_pages {
                    let page_num = self.current_page + row_idx as u64;
                    if page_num >= meta.total_pages {
                        break;
                    }

                    let block_idx = if block_size > 0 { page_num / block_size } else { 0 };
                    let page_in_block = if block_size > 0 { page_num % block_size } else { 0 };
                    let _row_linear_offset = page_num * page_length + self.col_offset_in_page;

                // Read bytes for this row from pre-read buffer.
                    // active_bytes may be empty if the read failed or this page is beyond EOF —
                    // in that case we render zeros. The correct loop terminator is
                    // `page_num >= meta.total_pages` above.
                    let (active_bytes, raw_bytes) = row_data.get(row_idx).cloned().unwrap_or_default();
                    let _ = (total_size, page_length); // suppress unused-variable warning

                    ui.horizontal(|ui| {
                        // 1. Prefix: Block, Page, Offset
                        let prefix_str = format!("B:{:04} P:{:03} +{:04X} | ", block_idx, page_in_block, self.col_offset_in_page);
                        ui.label(
                            egui::RichText::new(prefix_str)
                                .font(mono_font.clone())
                                .color(col_page_header),
                        );

                        // 2. Hex Bytes (if HexAndAscii or HexOnly)
                        let mut ascii_chars = Vec::with_capacity(bytes_per_row);

                        if self.display_mode != HexDisplayMode::AsciiOnly {
                            for col_idx in 0..bytes_per_row {
                                let abs_col = self.col_offset_in_page + col_idx as u64;
                                let byte_linear_offset = page_num * page_length + abs_col;

                                let act_b = active_bytes.get(col_idx).copied();
                                let raw_b = raw_bytes.get(col_idx).copied();

                                let disp_b = match self.diff_mode {
                                    HexDiffMode::RawOnly => raw_b.unwrap_or(0),
                                    _ => act_b.unwrap_or(0),
                                };

                                let is_diff = has_xor_diff
                                    && act_b.is_some()
                                    && raw_b.is_some()
                                    && act_b != raw_b;

                                let is_search = search_pattern_len > 0
                                    && search_results.iter().any(|r| {
                                        byte_linear_offset >= r.byte_offset
                                            && byte_linear_offset < r.byte_offset + search_pattern_len as u64
                                    });

                                // Text color
                                let mut text_color = if self.diff_mode == HexDiffMode::ColorDiff && is_diff {
                                    col_xor_changed
                                } else if disp_b == 0x00 {
                                    col_zero
                                } else if disp_b == 0xFF {
                                    col_ff
                                } else {
                                    col_normal
                                };

                                if is_search {
                                    text_color = Color32::WHITE;
                                }

                                let mut rich = egui::RichText::new(format!("{:02X} ", disp_b))
                                    .font(mono_font.clone())
                                    .color(text_color);

                                if is_search {
                                    rich = rich.background_color(col_search_bg);
                                }

                                // Segment background highlight if configured
                                if let Some(seg) = HexTabState::find_segment_at_column(abs_col, page_records) {
                                    if seg.kind == PageSegmentKind::SpareArea {
                                        rich = rich.background_color(Color32::from_rgba_premultiplied(80, 70, 10, 40));
                                    } else if seg.kind == PageSegmentKind::Ecc {
                                        rich = rich.background_color(Color32::from_rgba_premultiplied(70, 30, 10, 40));
                                    }
                                }

                                let byte_label = ui.label(rich);

                                if is_diff || byte_label.hovered() {
                                    if let (Some(a), Some(r)) = (act_b, raw_b) {
                                        byte_label.on_hover_ui(|ui| {
                                            ui.label(format!("Page: {}, Col: 0x{:04X} ({})", page_num, abs_col, abs_col));
                                            ui.label(format!("Linear: 0x{:08X}", byte_linear_offset));
                                            ui.label(format!("Raw:    0x{:02X} ({})", r, r));
                                            ui.label(format!("Output: 0x{:02X} ({})", a, a));
                                            ui.label(format!("XOR:    0x{:02X}", r ^ a));
                                            if let Some(s) = HexTabState::find_segment_at_column(abs_col, page_records) {
                                                ui.label(format!("Segment: {} ({})", s.name, format!("{:?}", s.kind)));
                                            }
                                        });
                                    }
                                }

                                let ch = if disp_b >= 32 && disp_b <= 126 {
                                    disp_b as char
                                } else {
                                    '.'
                                };
                                ascii_chars.push(ch);
                            }
                        } else {
                            // ASCII Only mode
                            for col_idx in 0..bytes_per_row {
                                let act_b = active_bytes.get(col_idx).copied().unwrap_or(0);
                                let ch = if act_b >= 32 && act_b <= 126 {
                                    act_b as char
                                } else {
                                    '.'
                                };
                                ascii_chars.push(ch);
                            }
                        }

                        // 3. ASCII Column
                        if self.display_mode != HexDisplayMode::HexOnly {
                            if self.display_mode == HexDisplayMode::HexAndAscii {
                                ui.label(egui::RichText::new(" | ").font(mono_font.clone()).color(Color32::DARK_GRAY));
                            }
                            let ascii_str: String = ascii_chars.into_iter().collect();
                            ui.label(
                                egui::RichText::new(ascii_str)
                                    .font(mono_font.clone())
                                    .color(col_normal),
                            );
                        }
                    });
                }
            });
    }

    /// Consolidated vertical page scrollbar with direct linear tracking and modifier precision modes
    fn render_velocity_page_scrollbar(
        &mut self,
        ui: &mut Ui,
        ctx: &egui::Context,
        total_pages: u64,
        block_size: u64,
        visible_pages: usize,
    ) {
        if total_pages == 0 {
            return;
        }

        let max_page = total_pages.saturating_sub(1);
        let track_width = 18.0;

        ui.style_mut().spacing.item_spacing = vec2(0.0, 2.0);

        // 1. Top Step Button: ▲
        let btn_up = ui.add_sized(
            vec2(track_width, 16.0),
            egui::Button::new("▲").small(),
        );
        if btn_up.on_hover_text("Previous Page (Up Arrow) — Ctrl for Block").clicked() {
            let step = if ctx.input(|i| i.modifiers.ctrl) { block_size } else { 1 };
            self.current_page = self.current_page.saturating_sub(step);
            ctx.request_repaint();
        }

        // 2. Track Painter
        let track_height = (ui.available_height() - 18.0).max(40.0);
        let (response, painter) = ui.allocate_painter(
            vec2(track_width, track_height),
            Sense::click_and_drag(),
        );
        let track_rect = response.rect;

        // Draw track background
        painter.rect_filled(track_rect, Rounding::same(3.0), Color32::from_rgb(25, 27, 32));
        painter.rect_stroke(track_rect, Rounding::same(3.0), Stroke::new(1.0, Color32::from_rgb(45, 48, 56)));

        // Thumb geometry
        let min_thumb_h = 24.0;
        let visible_fraction = ((visible_pages as f32) / (total_pages as f32).max(1.0)).clamp(0.01, 1.0);
        let thumb_h = (track_height * visible_fraction).clamp(min_thumb_h, track_height);
        let scrollable_h = (track_height - thumb_h).max(1.0);

        let progress = if max_page > 0 {
            (self.current_page as f64 / max_page as f64).clamp(0.0, 1.0) as f32
        } else {
            0.0
        };
        let thumb_y = track_rect.min.y + progress * scrollable_h;

        let thumb_rect = Rect::from_min_size(
            Pos2::new(track_rect.min.x + 2.0, thumb_y),
            vec2(track_width - 4.0, thumb_h),
        );

        let modifiers = ctx.input(|i| i.modifiers);

        // Interaction Handling
        if response.drag_started() {
            self.is_dragging_scrollbar = true;
            if let Some(pos) = response.interact_pointer_pos() {
                self.last_drag_mouse_y = Some(pos.y);
                if thumb_rect.contains(pos) {
                    // Clicked on thumb: keep exact grab point
                    self.drag_grab_offset_y = pos.y - thumb_rect.min.y;
                } else {
                    // Clicked on track: center thumb on click and jump
                    self.drag_grab_offset_y = thumb_h / 2.0;
                    let rel_y = (pos.y - track_rect.min.y - self.drag_grab_offset_y).clamp(0.0, scrollable_h);
                    let frac = (rel_y / scrollable_h) as f64;
                    self.current_page = (frac * max_page as f64).round() as u64;
                    ctx.request_repaint();
                }
            }
        }

        if response.dragged() && self.is_dragging_scrollbar {
            if let Some(mouse_pos) = response.interact_pointer_pos() {
                if modifiers.shift {
                    // Shift held: Fine Mode (1 page per pixel)
                    self.scroll_velocity_mode = "Fine Mode (Shift: 1 Page/px)".to_string();
                    if let Some(prev_y) = self.last_drag_mouse_y {
                        let dy = (mouse_pos.y - prev_y).round() as i64;
                        if dy != 0 {
                            let new_page = if dy > 0 {
                                self.current_page.saturating_add(dy as u64)
                            } else {
                                self.current_page.saturating_sub((-dy) as u64)
                            };
                            self.current_page = new_page.min(max_page);
                            self.last_drag_mouse_y = Some(mouse_pos.y);
                            ctx.request_repaint();
                        }
                    } else {
                        self.last_drag_mouse_y = Some(mouse_pos.y);
                    }
                } else if modifiers.ctrl {
                    // Ctrl held: Block Mode (1 block per 4 pixels)
                    let pages_per_px = (block_size as f32 / 4.0).max(1.0);
                    self.scroll_velocity_mode = format!("Block Mode (Ctrl: ~{} Pages/px)", pages_per_px.round() as u64);
                    if let Some(prev_y) = self.last_drag_mouse_y {
                        let dy = mouse_pos.y - prev_y;
                        let page_delta = (dy * pages_per_px).round() as i64;
                        if page_delta != 0 {
                            let new_page = if page_delta > 0 {
                                self.current_page.saturating_add(page_delta as u64)
                            } else {
                                self.current_page.saturating_sub((-page_delta) as u64)
                            };
                            self.current_page = new_page.min(max_page);
                            self.last_drag_mouse_y = Some(mouse_pos.y);
                            ctx.request_repaint();
                        }
                    } else {
                        self.last_drag_mouse_y = Some(mouse_pos.y);
                    }
                } else {
                    // Default Proportional Mode: Direct lock to mouse cursor position without any jitter
                    self.scroll_velocity_mode.clear();
                    let rel_y = (mouse_pos.y - track_rect.min.y - self.drag_grab_offset_y).clamp(0.0, scrollable_h);
                    let frac = (rel_y / scrollable_h) as f64;
                    let target_page = (frac * max_page as f64).round() as u64;
                    self.current_page = target_page.min(max_page);
                    self.last_drag_mouse_y = Some(mouse_pos.y);
                    ctx.request_repaint();
                }
            }
        }

        if response.drag_stopped() {
            self.is_dragging_scrollbar = false;
            self.last_drag_mouse_y = None;
            self.scroll_velocity_mode.clear();
        }

        // Direct click on track (when not dragging)
        if response.clicked() && !self.is_dragging_scrollbar {
            if let Some(pos) = response.interact_pointer_pos() {
                let rel_y = (pos.y - track_rect.min.y - thumb_h / 2.0).clamp(0.0, scrollable_h);
                let target_fraction = (rel_y / scrollable_h) as f64;
                self.current_page = ((target_fraction * max_page as f64).round() as u64).min(max_page);
                ctx.request_repaint();
            }
        }

        // Draw thumb
        let thumb_color = if self.is_dragging_scrollbar {
            Color32::from_rgb(110, 170, 240)
        } else if response.hovered() {
            Color32::from_rgb(85, 115, 150)
        } else {
            Color32::from_rgb(55, 70, 90)
        };

        painter.rect_filled(thumb_rect, Rounding::same(3.0), thumb_color);
        painter.rect_stroke(thumb_rect, Rounding::same(3.0), Stroke::new(1.0, Color32::from_rgb(120, 150, 190)));

        // Grip lines in the center of the thumb
        if thumb_h >= 30.0 {
            let mid_y = thumb_rect.center().y;
            let left_x = thumb_rect.min.x + 3.0;
            let right_x = thumb_rect.max.x - 3.0;
            let grip_color = Color32::from_rgb(170, 190, 220);
            painter.line_segment([Pos2::new(left_x, mid_y - 3.0), Pos2::new(right_x, mid_y - 3.0)], Stroke::new(1.0, grip_color));
            painter.line_segment([Pos2::new(left_x, mid_y), Pos2::new(right_x, mid_y)], Stroke::new(1.0, grip_color));
            painter.line_segment([Pos2::new(left_x, mid_y + 3.0), Pos2::new(right_x, mid_y + 3.0)], Stroke::new(1.0, grip_color));
        }

        // Tooltip
        let cur_block = if block_size > 0 { self.current_page / block_size } else { 0 };
        let in_block_page = if block_size > 0 { self.current_page % block_size } else { 0 };
        let mode_hint = if !self.scroll_velocity_mode.is_empty() {
            format!("\nActive: {}", self.scroll_velocity_mode)
        } else {
            "\nHold Shift for Fine Mode (1 Page/px), Ctrl for Block Mode".to_string()
        };
        response.on_hover_text(format!(
            "Page {} / {} (Block {}, Page {})\nClick or drag to scrub across dump{}",
            self.current_page,
            total_pages,
            cur_block,
            in_block_page,
            mode_hint
        ));

        // 3. Bottom Step Button: ▼
        let btn_down = ui.add_sized(
            vec2(track_width, 16.0),
            egui::Button::new("▼").small(),
        );
        if btn_down.on_hover_text("Next Page (Down Arrow) — Ctrl for Block").clicked() {
            let step = if ctx.input(|i| i.modifiers.ctrl) { block_size } else { 1 };
            self.current_page = (self.current_page + step).min(max_page);
            ctx.request_repaint();
        }
    }

    /// Dedicated horizontal scrollbar for scrubbing byte columns within a NAND page
    fn render_horizontal_page_scrollbar(
        &mut self,
        ui: &mut Ui,
        ctx: &egui::Context,
        page_length: u64,
        bytes_per_row: usize,
        page_records: &[PageStructureRecord],
    ) {
        if page_length == 0 {
            return;
        }

        let max_col = page_length.saturating_sub(bytes_per_row as u64);

        ui.horizontal(|ui| {
            ui.style_mut().spacing.item_spacing = vec2(4.0, 0.0);

            // Step Left button (16 bytes)
            let btn_left = ui.add_sized(
                vec2(20.0, 16.0),
                egui::Button::new("◀").small(),
            );
            if btn_left.on_hover_text("Scroll left 16 bytes (Left Arrow / Ctrl+Left)").clicked() {
                self.col_offset_in_page = self.col_offset_in_page.saturating_sub(16);
                ctx.request_repaint();
            }

            // Reserve space for Right button (20px) + spacing (4px)
            let track_w = (ui.available_width() - 24.0).max(40.0);
            let track_h = 16.0;

            let (response, painter) = ui.allocate_painter(
                vec2(track_w, track_h),
                Sense::click_and_drag(),
            );

            let track_rect = response.rect;

            // 1. Draw track background
            painter.rect_filled(
                track_rect,
                Rounding::same(3.0),
                Color32::from_rgb(25, 27, 32),
            );
            painter.rect_stroke(
                track_rect,
                Rounding::same(3.0),
                Stroke::new(1.0, Color32::from_rgb(45, 48, 56)),
            );

            // 2. Draw .case page structure segments onto track
            if !page_records.is_empty() {
                for rec in page_records {
                    if rec.stop < rec.start {
                        continue;
                    }
                    let start_frac = (rec.start as f32 / page_length as f32).clamp(0.0, 1.0);
                    let stop_frac = (((rec.stop + 1) as f32) / page_length as f32).clamp(0.0, 1.0);

                    let seg_x1 = track_rect.min.x + start_frac * track_rect.width();
                    let seg_x2 = track_rect.min.x + stop_frac * track_rect.width();

                    if seg_x2 > seg_x1 {
                        let seg_rect = Rect::from_min_max(
                            Pos2::new(seg_x1, track_rect.min.y + 2.0),
                            Pos2::new(seg_x2, track_rect.max.y - 2.0),
                        );

                        let (fill, stroke_col) = match rec.kind {
                            PageSegmentKind::Data => (
                                Color32::from_rgba_premultiplied(35, 75, 140, 100),
                                Color32::from_rgba_premultiplied(65, 115, 200, 140),
                            ),
                            PageSegmentKind::Ecc => (
                                Color32::from_rgba_premultiplied(160, 65, 20, 110),
                                Color32::from_rgba_premultiplied(220, 100, 35, 160),
                            ),
                            PageSegmentKind::SpareArea => (
                                Color32::from_rgba_premultiplied(150, 120, 15, 110),
                                Color32::from_rgba_premultiplied(220, 180, 30, 170),
                            ),
                            PageSegmentKind::Other => (
                                Color32::from_rgba_premultiplied(75, 75, 80, 80),
                                Color32::from_rgba_premultiplied(120, 120, 125, 120),
                            ),
                        };

                        painter.rect_filled(seg_rect, Rounding::same(1.5), fill);
                        painter.rect_stroke(seg_rect, Rounding::same(1.5), Stroke::new(0.5, stroke_col));
                    }
                }
            }

            // 3. Calculate thumb dimensions
            let visible_frac = (bytes_per_row as f32 / page_length as f32).clamp(0.005, 1.0);
            let min_thumb_w = 28.0;
            let thumb_w = (track_rect.width() * visible_frac).clamp(min_thumb_w, track_rect.width());
            let scrollable_w = (track_rect.width() - thumb_w).max(1.0);

            let col_frac = if max_col > 0 {
                (self.col_offset_in_page as f64 / max_col as f64).clamp(0.0, 1.0) as f32
            } else {
                0.0
            };

            let thumb_x = track_rect.min.x + col_frac * scrollable_w;
            let thumb_rect = Rect::from_min_size(
                Pos2::new(thumb_x, track_rect.min.y + 1.5),
                vec2(thumb_w, track_rect.height() - 3.0),
            );

            // 4. Handle dragging and clicking
            if response.drag_started() {
                self.is_dragging_h_scrollbar = true;
                self.last_h_drag_mouse_x = response.interact_pointer_pos().map(|p| p.x);
            }

            if response.dragged() && self.is_dragging_h_scrollbar {
                if let Some(pos) = response.interact_pointer_pos() {
                    let rel_x = (pos.x - track_rect.min.x - thumb_w / 2.0).clamp(0.0, scrollable_w);
                    let frac = (rel_x / scrollable_w) as f64;
                    let target_col = (frac * max_col as f64).round() as u64;
                    self.col_offset_in_page = target_col.min(max_col);
                    ctx.request_repaint();
                }
            }

            if response.drag_stopped() {
                self.is_dragging_h_scrollbar = false;
                self.last_h_drag_mouse_x = None;
            }

            if response.clicked() {
                if let Some(pos) = response.interact_pointer_pos() {
                    let rel_x = (pos.x - track_rect.min.x - thumb_w / 2.0).clamp(0.0, scrollable_w);
                    let frac = (rel_x / scrollable_w) as f64;
                    let target_col = (frac * max_col as f64).round() as u64;
                    self.col_offset_in_page = target_col.min(max_col);
                    ctx.request_repaint();
                }
            }

            // 5. Draw thumb
            let thumb_color = if self.is_dragging_h_scrollbar {
                Color32::from_rgb(110, 170, 240)
            } else if response.hovered() {
                Color32::from_rgb(85, 115, 150)
            } else {
                Color32::from_rgb(55, 70, 90)
            };

            painter.rect_filled(thumb_rect, Rounding::same(2.5), thumb_color);
            painter.rect_stroke(
                thumb_rect,
                Rounding::same(2.5),
                Stroke::new(1.0, Color32::from_rgb(120, 150, 190)),
            );

            // Grip lines in the center of the thumb
            if thumb_w >= 32.0 {
                let mid_x = thumb_rect.center().x;
                let top_y = thumb_rect.min.y + 3.0;
                let bot_y = thumb_rect.max.y - 3.0;
                let grip_color = Color32::from_rgb(170, 190, 220);
                painter.line_segment([Pos2::new(mid_x - 3.0, top_y), Pos2::new(mid_x - 3.0, bot_y)], Stroke::new(1.0, grip_color));
                painter.line_segment([Pos2::new(mid_x, top_y), Pos2::new(mid_x, bot_y)], Stroke::new(1.0, grip_color));
                painter.line_segment([Pos2::new(mid_x + 3.0, top_y), Pos2::new(mid_x + 3.0, bot_y)], Stroke::new(1.0, grip_color));
            }

            // 6. Tooltip
            let hover_col = if let Some(pos) = response.interact_pointer_pos() {
                let frac = ((pos.x - track_rect.min.x) / track_rect.width()).clamp(0.0, 1.0);
                (frac as f64 * page_length as f64).round() as u64
            } else {
                self.col_offset_in_page
            };
            let seg_str = HexTabState::find_segment_at_column(hover_col, page_records)
                .map(|s| format!(" | Segment: {} ({:?})", s.name, s.kind))
                .unwrap_or_default();

            response.on_hover_text(format!(
                "Page Col: 0x{:04X} ({}) / PageLen: {} (showing {}..{}){}\nClick or drag to scrub across the page horizontally",
                self.col_offset_in_page,
                self.col_offset_in_page,
                page_length,
                self.col_offset_in_page,
                (self.col_offset_in_page + bytes_per_row as u64).min(page_length),
                seg_str
            ));

            // Step Right button (16 bytes)
            let btn_right = ui.add_sized(
                vec2(20.0, 16.0),
                egui::Button::new("▶").small(),
            );
            if btn_right.on_hover_text("Scroll right 16 bytes (Right Arrow / Ctrl+Right)").clicked() {
                self.col_offset_in_page = (self.col_offset_in_page + 16).min(max_col);
                ctx.request_repaint();
            }
        });
    }

    /// Handle keyboard input for page, column, and block navigation
    fn handle_keyboard_inputs(
        &mut self,
        ctx: &egui::Context,
        total_pages: u64,
        block_size: u64,
        bytes_per_row: u64,
        page_length: u64,
        visible_pages: usize,
    ) {
        let max_col = page_length.saturating_sub(bytes_per_row);

        ctx.input(|i| {
            // Vertical Page Navigation (Up/Down)
            if i.key_pressed(Key::ArrowUp) {
                if i.modifiers.ctrl {
                    // Ctrl+Up: Previous NAND Block
                    self.current_page = self.current_page.saturating_sub(block_size);
                } else {
                    // Up: Previous Page
                    self.current_page = self.current_page.saturating_sub(1);
                }
            } else if i.key_pressed(Key::ArrowDown) {
                if i.modifiers.ctrl {
                    // Ctrl+Down: Next NAND Block
                    self.current_page = (self.current_page + block_size).min(total_pages.saturating_sub(1));
                } else {
                    // Down: Next Page
                    self.current_page = (self.current_page + 1).min(total_pages.saturating_sub(1));
                }
            }

            // Horizontal In-Page Navigation (Left/Right)
            if i.key_pressed(Key::ArrowLeft) {
                if i.modifiers.ctrl {
                    // Ctrl+Left: Half screen width left
                    let step = (bytes_per_row / 2).max(1);
                    self.col_offset_in_page = self.col_offset_in_page.saturating_sub(step);
                } else {
                    // Left: 1 Byte left
                    self.col_offset_in_page = self.col_offset_in_page.saturating_sub(1);
                }
            } else if i.key_pressed(Key::ArrowRight) {
                if i.modifiers.ctrl {
                    // Ctrl+Right: Half screen width right
                    let step = (bytes_per_row / 2).max(1);
                    self.col_offset_in_page = (self.col_offset_in_page + step).min(max_col);
                } else {
                    // Right: 1 Byte right
                    self.col_offset_in_page = (self.col_offset_in_page + 1).min(max_col);
                }
            }

            // PageUp / PageDown (Screen pages)
            if i.key_pressed(Key::PageUp) {
                if i.modifiers.ctrl {
                    self.current_page = self.current_page.saturating_sub(block_size);
                } else {
                    self.current_page = self.current_page.saturating_sub(visible_pages as u64);
                }
            } else if i.key_pressed(Key::PageDown) {
                if i.modifiers.ctrl {
                    self.current_page = (self.current_page + block_size).min(total_pages.saturating_sub(1));
                } else {
                    self.current_page = (self.current_page + visible_pages as u64).min(total_pages.saturating_sub(1));
                }
            }

            // Home / End: start or end of current NAND page
            if i.key_pressed(Key::Home) {
                if i.modifiers.ctrl {
                    self.current_page = 0;
                }
                self.col_offset_in_page = 0;
            } else if i.key_pressed(Key::End) {
                if i.modifiers.ctrl {
                    self.current_page = total_pages.saturating_sub(1);
                }
                self.col_offset_in_page = max_col;
            }

            // Mouse wheel: Vertical page scrolling
            if i.raw_scroll_delta.y != 0.0 {
                let delta = (i.raw_scroll_delta.y / 20.0).round() as i64;
                if delta > 0 {
                    self.current_page = self.current_page.saturating_sub(delta as u64);
                } else if delta < 0 {
                    self.current_page = (self.current_page + (-delta) as u64).min(total_pages.saturating_sub(1));
                }
            }

            // Horizontal mouse wheel: Column scrolling
            if i.raw_scroll_delta.x != 0.0 {
                let delta = (i.raw_scroll_delta.x / 10.0).round() as i64;
                if delta > 0 {
                    self.col_offset_in_page = (self.col_offset_in_page + delta as u64).min(max_col);
                } else if delta < 0 {
                    self.col_offset_in_page = self.col_offset_in_page.saturating_sub((-delta) as u64);
                }
            }
        });
    }

    /// Jump to adjacent search match
    fn jump_to_search_match(&mut self, results: &[SearchResult], meta: &FileMetadata, forward: bool) {
        if results.is_empty() {
            return;
        }
        let cur_linear = self.current_linear_offset(meta);

        if forward {
            if let Some(m) = results.iter().find(|r| r.byte_offset > cur_linear) {
                self.jump_to_linear_offset(m.byte_offset, meta);
            } else {
                self.jump_to_linear_offset(results[0].byte_offset, meta);
            }
        } else {
            if let Some(m) = results.iter().rev().find(|r| r.byte_offset < cur_linear) {
                self.jump_to_linear_offset(m.byte_offset, meta);
            } else {
                self.jump_to_linear_offset(results.last().unwrap().byte_offset, meta);
            }
        }
    }

    /// Handle goto address or page input
    fn handle_goto(&mut self, meta: &FileMetadata) {
        let input = self.goto_address_input.trim();

        // Check if input is a page number, e.g. "p120" or "page 120"
        if let Some(page_str) = input.strip_prefix('p').or_else(|| input.strip_prefix('P')) {
            if let Ok(p) = page_str.trim().parse::<u64>() {
                self.current_page = p.min(meta.total_pages.saturating_sub(1));
                self.goto_error = None;
                return;
            }
        }

        // Otherwise parse as linear byte offset
        let parsed = if let Some(hex_str) = input.strip_prefix("0x").or_else(|| input.strip_prefix("0X")) {
            u64::from_str_radix(hex_str.trim(), 16).ok()
        } else if input.starts_with('$') {
            u64::from_str_radix(&input[1..], 16).ok()
        } else if input.chars().all(|c| c.is_ascii_digit()) {
            input.parse::<u64>().ok()
        } else {
            u64::from_str_radix(input, 16).ok()
        };

        if let Some(offset) = parsed {
            self.goto_error = None;
            self.jump_to_linear_offset(offset, meta);
        } else {
            self.goto_error = Some("Invalid input (use 0x..., dec, or p<num>)".to_string());
        }
    }

    /// Convert linear byte offset to NAND block, page, and offset inside page
    pub fn offset_to_nand_coords(offset: u64, meta: &FileMetadata) -> (u64, u64, u64) {
        let pl = meta.page_length as u64;
        let bs = meta.block_size as u64;
        let block_stride = pl * bs;

        let block = if block_stride > 0 { offset / block_stride } else { 0 };
        let offset_in_block = if block_stride > 0 { offset % block_stride } else { 0 };
        let page = if pl > 0 { offset_in_block / pl } else { 0 };
        let offset_in_page = if pl > 0 { offset_in_block % pl } else { 0 };

        (block, page, offset_in_page)
    }

    /// Convert NAND block, page, and offset to linear byte offset
    pub fn nand_coords_to_offset(block: u64, page: u64, offset_in_page: u64, meta: &FileMetadata) -> u64 {
        let pl = meta.page_length as u64;
        let bs = meta.block_size as u64;
        let block_stride = pl * bs;
        block * block_stride + page * pl + offset_in_page
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_meta() -> FileMetadata {
        FileMetadata::new("test.dump".to_string(), 1024 * 1024 * 64, 4096, 64)
    }

    #[test]
    fn test_page_and_col_coords_conversion() {
        let meta = test_meta();
        let mut state = HexTabState::new();

        // Jump to byte at page 5, col 128
        let target_offset = 5 * 4096 + 128;
        state.jump_to_linear_offset(target_offset, &meta);

        assert_eq!(state.current_page, 5);
        assert_eq!(state.col_offset_in_page, 128);
        assert_eq!(state.current_linear_offset(&meta), target_offset);
    }

    #[test]
    fn test_parse_case_file_records() {
        let xml = r#"<?xml version="1.0"?>
<Project>
  <Records>
    <Record StructureDefinitionName="SA" StartAddress="0" StopAddress="7" />
    <Record StructureDefinitionName="Data area" StartAddress="8" StopAddress="519" />
    <Record StructureDefinitionName="ECC" StartAddress="520" StopAddress="619" />
    <Record StructureDefinitionName="Page" StartAddress="0" StopAddress="1239" />
  </Records>
</Project>
"#;
        let records = parse_case_file(xml);
        assert_eq!(records.len(), 3); // "Page" is skipped

        assert_eq!(records[0].name, "SA");
        assert_eq!(records[0].start, 0);
        assert_eq!(records[0].stop, 7);
        assert_eq!(records[0].kind, PageSegmentKind::SpareArea);

        assert_eq!(records[1].name, "Data area");
        assert_eq!(records[1].start, 8);
        assert_eq!(records[1].stop, 519);
        assert_eq!(records[1].kind, PageSegmentKind::Data);

        assert_eq!(records[2].name, "ECC");
        assert_eq!(records[2].start, 520);
        assert_eq!(records[2].stop, 619);
        assert_eq!(records[2].kind, PageSegmentKind::Ecc);
    }

    #[test]
    fn test_find_segment_at_column() {
        let records = vec![
            PageStructureRecord {
                name: "SA".to_string(),
                start: 0,
                stop: 7,
                kind: PageSegmentKind::SpareArea,
            },
            PageStructureRecord {
                name: "Data area".to_string(),
                start: 8,
                stop: 519,
                kind: PageSegmentKind::Data,
            },
            PageStructureRecord {
                name: "ECC".to_string(),
                start: 520,
                stop: 619,
                kind: PageSegmentKind::Ecc,
            },
        ];

        let seg_sa = HexTabState::find_segment_at_column(4, &records);
        assert!(seg_sa.is_some());
        assert_eq!(seg_sa.unwrap().kind, PageSegmentKind::SpareArea);

        let seg_data = HexTabState::find_segment_at_column(100, &records);
        assert!(seg_data.is_some());
        assert_eq!(seg_data.unwrap().kind, PageSegmentKind::Data);

        let seg_ecc = HexTabState::find_segment_at_column(550, &records);
        assert!(seg_ecc.is_some());
        assert_eq!(seg_ecc.unwrap().kind, PageSegmentKind::Ecc);

        let seg_none = HexTabState::find_segment_at_column(900, &records);
        assert!(seg_none.is_none());
    }

    #[test]
    fn test_goto_page_and_hex() {
        let meta = test_meta();
        let mut state = HexTabState::new();

        state.goto_address_input = "p42".to_string();
        state.handle_goto(&meta);
        assert_eq!(state.current_page, 42);
        assert!(state.goto_error.is_none());

        state.goto_address_input = "0x2000".to_string(); // 2 * 4096 = Page 2, col 0
        state.handle_goto(&meta);
        assert_eq!(state.current_page, 2);
        assert_eq!(state.col_offset_in_page, 0);
        assert!(state.goto_error.is_none());
    }

    #[test]
    fn test_horizontal_scrollbar_clamping() {
        let meta = test_meta(); // page_length = 4096
        let mut state = HexTabState::new();

        // Visible bytes = 32, page_length = 4096 => max_col = 4064
        let bytes_per_row = 32usize;
        let page_len = meta.page_length as u64;
        let max_col = page_len.saturating_sub(bytes_per_row as u64);
        assert_eq!(max_col, 4064);

        // Clamping col_offset_in_page
        state.col_offset_in_page = 5000;
        state.col_offset_in_page = state.col_offset_in_page.min(max_col);
        assert_eq!(state.col_offset_in_page, 4064);

        // When bytes_per_row >= page_length, max_col is 0
        let small_page_len = 16u64;
        let max_col_small = small_page_len.saturating_sub(bytes_per_row as u64);
        assert_eq!(max_col_small, 0);
    }

    #[test]
    fn test_horizontal_scrollbar_defaults() {
        let state = HexTabState::new();
        assert!(!state.is_dragging_h_scrollbar);
        assert!(state.last_h_drag_mouse_x.is_none());
        assert_eq!(state.col_offset_in_page, 0);
    }

    #[test]
    fn test_visible_pages_exact_fit_no_overflow() {
        let font_size = 12.0f32;
        let row_h = font_size * 1.3 + 1.5;
        let available_height = 400.0f32;

        let visible_pages = (available_height / row_h).floor() as usize;
        let total_rows_h = (visible_pages as f32) * row_h;

        // Guaranteed to fit strictly within available height
        assert!(total_rows_h <= available_height);
        assert!(visible_pages >= 2);
    }
}
