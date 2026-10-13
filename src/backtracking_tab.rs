//! Backtracking Tab — shows the provenance chain of a clicked hex byte.

use crate::data_provider::BacktrackAddress;

/// Role of a backtrack entry in the provenance chain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BacktrackRole {
    Leaf,         // Input file node
    Intermediate, // Transform node between leaf and origin
    Origin,       // The node whose byte was clicked
}

/// A single row in the backtracking results table.
#[derive(Debug, Clone)]
pub struct BacktrackEntry {
    pub node_id: usize,
    pub node_name: String,
    pub file_path: Option<String>,
    pub absolute_offset: u64,
    pub block: u64,
    pub page_in_block: u64,
    pub offset_in_page: u64,
    pub byte_value: u8,
    pub role: BacktrackRole,
}

/// State for the Backtracking Results Tab UI.
pub struct BacktrackingTabState {
    /// The provenance entries (sorted: Leaf first, Intermediate, Origin last).
    pub entries: Vec<BacktrackEntry>,
    /// Node ID that was clicked in the Hex Tab.
    pub source_node_id: Option<usize>,
    /// Byte offset within that node's stream that was clicked.
    pub source_offset: Option<u64>,
    /// Requested navigation: (node_id, absolute_offset_in_that_node).
    /// Consumed by the app_window timer loop.
    pub jump_to_hex: Option<(usize, u64)>,
}

impl Default for BacktrackingTabState {
    fn default() -> Self {
        Self::new()
    }
}

impl BacktrackingTabState {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            source_node_id: None,
            source_offset: None,
            jump_to_hex: None,
        }
    }

    /// Populate the entries from raw BacktrackAddress results.
    /// `origin_node_id` is the node whose byte was clicked (set is_origin=true on it).
    /// Roles: is_leaf → Leaf; node_id==origin_node_id && !is_leaf → Origin; else Intermediate.
    /// Sort order: Leaf entries first, then Intermediate, then Origin at the bottom.
    pub fn set_results(
        &mut self,
        addresses: Vec<BacktrackAddress>,
        origin_node_id: usize,
        origin_offset: u64,
    ) {
        self.source_node_id = Some(origin_node_id);
        self.source_offset = Some(origin_offset);

        let mut entries: Vec<BacktrackEntry> = addresses
            .into_iter()
            .map(|addr| {
                let role = if addr.is_leaf {
                    BacktrackRole::Leaf
                } else if addr.node_id == origin_node_id {
                    BacktrackRole::Origin
                } else {
                    BacktrackRole::Intermediate
                };
                BacktrackEntry {
                    node_id: addr.node_id,
                    node_name: addr.node_name,
                    file_path: addr.file_path,
                    absolute_offset: addr.absolute_offset,
                    block: addr.block,
                    page_in_block: addr.page_in_block,
                    offset_in_page: addr.offset_in_page,
                    byte_value: addr.byte_value,
                    role,
                }
            })
            .collect();

        // Sort: Leaf first, Intermediate middle, Origin last
        entries.sort_by_key(|e| match e.role {
            BacktrackRole::Leaf => 0u8,
            BacktrackRole::Intermediate => 1,
            BacktrackRole::Origin => 2,
        });

        self.entries = entries;
    }

    /// Render the Backtracking Tab egui UI.
    pub fn show_ui(&mut self, ctx: &egui::Context) {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("Byte Provenance / Backtracking");
            ui.add_space(4.0);

            if self.entries.is_empty() {
                ui.add_space(80.0);
                ui.vertical_centered(|ui| {
                    ui.label("Click a byte in the Hex tab to trace its origin.");
                });
                return;
            }

            // Show source info
            if let (Some(nid), Some(off)) = (self.source_node_id, self.source_offset) {
                ui.label(format!("Clicked byte: Node #{}, offset 0x{:08X}", nid, off));
                ui.add_space(4.0);
            }

            ui.separator();

            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    egui::Grid::new("backtrack_results_grid")
                        .striped(true)
                        .spacing([12.0, 6.0])
                        .show(ui, |ui| {
                            // Header row
                            ui.strong("Role");
                            ui.strong("Node");
                            ui.strong("File");
                            ui.strong("Block");
                            ui.strong("Page");
                            ui.strong("Offset");
                            ui.strong("Value");
                            ui.strong("Action");
                            ui.end_row();

                            let mut jump_request = None;

                            for entry in &self.entries {
                                let (role_icon, role_color) = match entry.role {
                                    BacktrackRole::Leaf => (
                                        "🌿 Leaf",
                                        egui::Color32::from_rgb(100, 210, 100),
                                    ),
                                    BacktrackRole::Intermediate => (
                                        "⚙ Intermediate",
                                        egui::Color32::from_rgb(200, 200, 100),
                                    ),
                                    BacktrackRole::Origin => (
                                        "🎯 Origin",
                                        egui::Color32::from_rgb(240, 140, 60),
                                    ),
                                };

                                ui.colored_label(role_color, role_icon);
                                ui.label(&entry.node_name);

                                // File: basename only, truncated to 24 chars
                                let file_display = entry
                                    .file_path
                                    .as_deref()
                                    .and_then(|p| std::path::Path::new(p).file_name())
                                    .and_then(|n| n.to_str())
                                    .unwrap_or("—");
                                let truncated = if file_display.len() > 24 {
                                    format!("…{}", &file_display[file_display.len() - 23..])
                                } else {
                                    file_display.to_string()
                                };
                                ui.label(truncated)
                                    .on_hover_text(entry.file_path.as_deref().unwrap_or("—"));

                                ui.label(format!("{}", entry.block));
                                ui.label(format!("{}", entry.page_in_block));
                                ui.label(format!("0x{:04X}", entry.offset_in_page));
                                ui.monospace(format!("{:02X}", entry.byte_value));

                                if ui
                                    .button("Jump ↗")
                                    .on_hover_text("Open this location in the Hex Tab")
                                    .clicked()
                                {
                                    jump_request = Some((entry.node_id, entry.absolute_offset));
                                }

                                ui.end_row();
                            }

                            if let Some((nid, off)) = jump_request {
                                self.jump_to_hex = Some((nid, off));
                            }
                        });
                });
        });
    }
}
