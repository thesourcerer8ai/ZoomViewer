//! Search Tab Module
//!
//! Provides an interactive search GUI for NAND dump streams, supporting ASCII/HEX searches,
//! progress monitoring, cancellation, result navigation, and file export.

use crate::data_provider::DumpDataProvider;
use crate::search::{export_results_to_file, search_provider_with_sender, SearchMode, SearchOptions, SearchResult};
use crate::FileMetadata;
use parking_lot::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver};
use std::sync::Arc;
use std::thread;

/// State for the Search Tab UI
pub struct SearchTabState {
    /// Configured search options
    pub options: SearchOptions,
    /// Whether a search is currently running in the background
    pub is_searching: bool,
    /// Cancellation flag passed to background thread
    pub cancel_flag: Arc<AtomicBool>,
    /// Scanned bytes progress
    pub progress_bytes: Arc<AtomicU64>,
    /// Total bytes to scan
    pub total_bytes: u64,
    /// Status message for the user
    pub status_message: String,
    /// List of found search results
    pub results: Vec<SearchResult>,
    /// Index of currently selected search result
    pub selected_result_idx: Option<usize>,
    /// Target file path for exporting results
    pub export_path: String,
    /// Requested jump target: (block, page, offset_in_page)
    pub jump_target: Option<(u64, u64, u64)>,
    /// Metadata for opening results in browser
    pub target_metadata: Option<FileMetadata>,
    /// Channel receiver for live streaming of found matches
    match_rx: Option<Receiver<SearchResult>>,
    /// Channel receiver for search completion
    done_rx: Option<Receiver<Result<usize, String>>>,
}

impl Default for SearchTabState {
    fn default() -> Self {
        Self::new()
    }
}

impl SearchTabState {
    pub fn new() -> Self {
        Self {
            options: SearchOptions {
                pattern: "|Block".to_string(),
                mode: SearchMode::Ascii,
                case_sensitive: false,
                max_matches: 1000,
            },
            is_searching: false,
            cancel_flag: Arc::new(AtomicBool::new(false)),
            progress_bytes: Arc::new(AtomicU64::new(0)),
            total_bytes: 0,
            status_message: "Ready to search.".to_string(),
            results: Vec::new(),
            selected_result_idx: None,
            export_path: "search_results.txt".to_string(),
            jump_target: None,
            target_metadata: None,
            match_rx: None,
            done_rx: None,
        }
    }

    /// Check for incoming live matches and completion status
    pub fn poll_results(&mut self) -> bool {
        let mut updated = false;

        // 1. Drain newly found matches from background search
        if let Some(ref rx) = self.match_rx {
            while let Ok(m) = rx.try_recv() {
                self.results.push(m);
                updated = true;
            }
        }

        // 2. Check if background search completed or errored
        if let Some(ref rx) = self.done_rx {
            if let Ok(res) = rx.try_recv() {
                self.is_searching = false;
                self.match_rx = None;
                self.done_rx = None;
                updated = true;
                match res {
                    Ok(_) => {
                        let was_cancelled = self.cancel_flag.load(Ordering::Relaxed);
                        if was_cancelled {
                            self.status_message = format!("Search stopped. Found {} matches.", self.results.len());
                        } else {
                            self.status_message = format!("Search completed! Found {} matches.", self.results.len());
                        }
                    }
                    Err(e) => {
                        self.status_message = format!("Search error: {}", e);
                    }
                }
            }
        }

        if self.is_searching && updated {
            self.status_message = format!("Searching in progress... ({} matches found)", self.results.len());
        }

        updated
    }

    /// Start a new search on the given provider
    pub fn start_search(&mut self, provider: Arc<Mutex<dyn DumpDataProvider>>) {
        if self.options.pattern.trim().is_empty() {
            self.status_message = "Search pattern cannot be empty!".to_string();
            return;
        }

        let meta = provider.lock().get_metadata();
        self.target_metadata = Some(meta.clone());
        self.total_bytes = meta.size;
        self.progress_bytes = Arc::new(AtomicU64::new(0));
        self.cancel_flag = Arc::new(AtomicBool::new(false));
        self.is_searching = true;
        self.results.clear();
        self.selected_result_idx = None;
        self.status_message = "Searching in progress...".to_string();

        let (match_tx, match_rx) = channel();
        let (done_tx, done_rx) = channel();
        self.match_rx = Some(match_rx);
        self.done_rx = Some(done_rx);

        let options = self.options.clone();
        let cancel = self.cancel_flag.clone();
        let progress = self.progress_bytes.clone();

        thread::spawn(move || {
            let res = search_provider_with_sender(provider, &options, cancel, progress, Some(match_tx));
            match res {
                Ok(results) => {
                    let _ = done_tx.send(Ok(results.len()));
                }
                Err(e) => {
                    let _ = done_tx.send(Err(e));
                }
            }
        });
    }

    /// Cancel the active search
    pub fn stop_search(&mut self) {
        if self.is_searching {
            self.cancel_flag.store(true, Ordering::Relaxed);
            self.status_message = format!("Stopping search... ({} matches found so far)", self.results.len());
        }
    }

    /// Save current results to file
    pub fn export_results(&mut self) {
        if self.results.is_empty() {
            self.status_message = "No results to export!".to_string();
            return;
        }

        let is_csv = self.export_path.ends_with(".csv");
        match export_results_to_file(&self.results, &self.export_path, is_csv, self.target_metadata.clone().unwrap().page_length, self.target_metadata.clone().unwrap().block_size) {
            Ok(_) => {
                self.status_message = format!(
                    "Successfully exported {} results to '{}'!",
                    self.results.len(),
                    self.export_path
                );
            }
            Err(e) => {
                self.status_message = format!("Failed to export results: {}", e);
            }
        }
    }

    /// Render Search Tab egui UI
    pub fn show_ui(
        &mut self,
        ctx: &egui::Context,
        target_name: &str,
        provider_opt: Option<Arc<Mutex<dyn DumpDataProvider>>>,
    ) {
        self.poll_results();

        if self.is_searching {
            ctx.request_repaint();
        }

        if let Some(ref prov) = provider_opt {
            self.target_metadata = Some(prov.lock().get_metadata());
        }

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("NAND Dump Pattern Search");
            ui.add_space(4.0);

            // Target Node Status
            ui.group(|ui| {
                ui.horizontal(|ui| {
                    ui.label("🎯 Search Target Node:");
                    ui.strong(target_name);
                    if provider_opt.is_none() {
                        ui.colored_label(egui::Color32::RED, "(No valid provider connected!)");
                    } else {
                        ui.colored_label(egui::Color32::GREEN, "● Connected");
                    }
                });
            });

            ui.add_space(6.0);

            // Search Configuration Panel
            ui.group(|ui| {
                ui.horizontal(|ui| {
                    ui.label("Search Pattern:");
                    let text_edit = ui.text_edit_singleline(&mut self.options.pattern);
                    if text_edit.lost_focus() && ctx.input(|i| i.key_pressed(egui::Key::Enter)) {
                        if !self.is_searching {
                            if let Some(ref prov) = provider_opt {
                                self.start_search(prov.clone());
                            }
                        }
                    }
                });

                ui.horizontal(|ui| {
                    ui.label("Mode:");
                    ui.radio_value(&mut self.options.mode, SearchMode::Ascii, "ASCII Text");
                    ui.radio_value(&mut self.options.mode, SearchMode::Hex, "HEX Bytes (e.g. AA BB 00)");

                    if self.options.mode == SearchMode::Ascii {
                        ui.separator();
                        ui.checkbox(&mut self.options.case_sensitive, "Case Sensitive");
                    }

                    ui.separator();
                    ui.label("Max Matches:");
                    ui.add(egui::DragValue::new(&mut self.options.max_matches).clamp_range(1..=100_000));
                });

                ui.horizontal(|ui| {
                    if !self.is_searching {
                        let can_search = provider_opt.is_some() && !self.options.pattern.trim().is_empty();
                        if ui.add_enabled(can_search, egui::Button::new("🔍 Start Search")).clicked() {
                            if let Some(ref prov) = provider_opt {
                                self.start_search(prov.clone());
                            }
                        }
                    } else {
                        if ui.button("⏹ Stop Search").clicked() {
                            self.stop_search();
                        }
                    }

                    ui.separator();
                    ui.label(&self.status_message);
                });

                // Progress Bar
                if self.is_searching || self.total_bytes > 0 {
                    let progress = self.progress_bytes.load(Ordering::Relaxed);
                    let ratio = if self.total_bytes > 0 {
                        (progress as f32 / self.total_bytes as f32).clamp(0.0, 1.0)
                    } else {
                        0.0
                    };
                    let progress_mb = progress as f64 / (1024.0 * 1024.0);
                    let total_mb = self.total_bytes as f64 / (1024.0 * 1024.0);
                    let percent = (ratio * 100.0) as u32;

                    ui.add_space(4.0);
                    ui.add(
                        egui::ProgressBar::new(ratio)
                            .text(format!("{:.1} MB / {:.1} MB ({}%)", progress_mb, total_mb, percent))
                            .animate(self.is_searching),
                    );
                }
            });

            ui.add_space(6.0);

            // Export & Match Count Toolbar
            ui.horizontal(|ui| {
                ui.strong(format!("Results: {} matches", self.results.len()));

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("💾 Save Results").clicked() {
                        self.export_results();
                    }
                    ui.text_edit_singleline(&mut self.export_path);
                    ui.label("Export file:");
                });
            });

            ui.separator();

            // Results Table
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    egui::Grid::new("search_results_grid")
                        .striped(true)
                        .spacing([12.0, 6.0])
                        .show(ui, |ui| {
                            // Header
                            ui.strong("#");
                            ui.strong("Block #");
                            ui.strong("Page #");
                            ui.strong("Offset in Page");
                            ui.strong("Hex Preview");
                            ui.strong("ASCII Preview");
                            ui.strong("Actions");
                            ui.end_row();

                            let mut jump_to = None;
                            let mut open_browser_for = None;

                            for (i, r) in self.results.iter().enumerate() {
                                let is_selected = self.selected_result_idx == Some(i);

                                if is_selected {
                                    ui.colored_label(egui::Color32::GOLD, format!("{}", r.index));
                                } else {
                                    ui.label(format!("{}", r.index));
                                }

                                ui.label(format!("{}", r.block));
                                ui.label(format!("{}", r.page));
                                ui.label(format!("0x{:04X} ({})", r.offset_in_page, r.offset_in_page));
                                ui.monospace(&r.preview_hex);
                                ui.monospace(&r.preview_ascii);

                                ui.horizontal(|ui| {
                                    if ui.button("Jump ↗").on_hover_text("Navigate to this location in ZoomViewer").clicked() {
                                        jump_to = Some((i, r.block, r.page, r.offset_in_page));
                                    }
                                    if ui.button("🌐 Browser").on_hover_text("Open this location in browser (xorviewer.pl)").clicked() {
                                        open_browser_for = Some((r.block, r.page, r.offset_in_page));
                                    }
                                });

                                ui.end_row();
                            }

                            if let Some((idx, block, page, offset)) = jump_to {
                                self.selected_result_idx = Some(idx);
                                self.jump_target = Some((block, page, offset));
                            }

                            if let Some((block, page, offset)) = open_browser_for {
                                if let Some(ref meta) = self.target_metadata {
                                    if let Some(url) = crate::app_window::open_xorviewer_in_browser(meta, block, page, offset) {
                                        self.status_message = format!("Opened in browser: {}", url);
                                    } else {
                                        self.status_message = "Cannot open browser: dump file path is empty.".to_string();
                                    }
                                } else {
                                    self.status_message = "Cannot open browser: no provider metadata available.".to_string();
                                }
                            }
                        });
                });
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_search_tab_default_pattern() {
        let state = SearchTabState::new();
        assert_eq!(state.options.pattern, "|Block");
    }
}
