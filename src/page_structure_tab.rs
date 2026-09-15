//! Page Structure Editor Tab
//!
//! Interactive editor for defining the byte-level layout of a NAND page.
//! Each page is divided into named, typed segments (DATA, ECC, SA, SA-ECC, UNKNOWN).
//!
//! This state is shared via `Arc<Mutex<PageStructureTabState>>` with:
//! - The Hex Viewer tab (for segment coloring and quick-jump dropdown)
//! - The Workflow Editor (reads active `page_length` to drive the size validator)

use crate::hex_tab::{parse_case_file, PageSegmentKind, PageStructureRecord};
use egui::{
    vec2, Color32, RichText, Rounding, Stroke, Ui,
};

// ── Segment kind ─────────────────────────────────────────────────────────────

/// The logical role of a byte range within a NAND page
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SegmentKind {
    Data,
    Ecc,
    Sa,
    SaEcc,
    Unknown,
}

impl SegmentKind {
    /// All variants in display order
    pub fn all() -> &'static [SegmentKind] {
        &[
            SegmentKind::Data,
            SegmentKind::Ecc,
            SegmentKind::Sa,
            SegmentKind::SaEcc,
            SegmentKind::Unknown,
        ]
    }

    /// Human-readable label used in the UI
    pub fn label(self) -> &'static str {
        match self {
            SegmentKind::Data    => "DATA",
            SegmentKind::Ecc     => "ECC",
            SegmentKind::Sa      => "SA",
            SegmentKind::SaEcc   => "SA-ECC",
            SegmentKind::Unknown => "UNKNOWN",
        }
    }

    /// Name written into `.case` XML
    pub fn case_name(self) -> &'static str {
        self.label()
    }

    /// egui display color
    pub fn color(self) -> Color32 {
        match self {
            SegmentKind::Data    => Color32::from_rgb(59, 130, 246),   // blue
            SegmentKind::Ecc     => Color32::from_rgb(249, 115,  22),  // orange
            SegmentKind::Sa      => Color32::from_rgb(34, 197,  94),   // green
            SegmentKind::SaEcc   => Color32::from_rgb(20, 184, 166),   // teal
            SegmentKind::Unknown => Color32::from_rgb(100, 100, 100),  // gray
        }
    }

    /// Convert to the `PageSegmentKind` used by `hex_tab`
    pub fn to_hex_kind(self) -> PageSegmentKind {
        match self {
            SegmentKind::Data    => PageSegmentKind::Data,
            SegmentKind::Ecc     => PageSegmentKind::Ecc,
            SegmentKind::Sa      => PageSegmentKind::SpareArea,
            SegmentKind::SaEcc   => PageSegmentKind::SpareArea,
            SegmentKind::Unknown => PageSegmentKind::Other,
        }
    }

    /// Parse from a `.case` XML name
    fn from_case_name(s: &str) -> Self {
        match s.to_uppercase().as_str() {
            "DATA" | "DATA AREA"             => SegmentKind::Data,
            "ECC"                            => SegmentKind::Ecc,
            "SA" | "SPARE" | "OOB"          => SegmentKind::Sa,
            "SA-ECC" | "SAECC"              => SegmentKind::SaEcc,
            _                                => SegmentKind::Unknown,
        }
    }
}

// ── PageSegment ───────────────────────────────────────────────────────────────

/// A single typed byte range inside one NAND page
#[derive(Debug, Clone)]
pub struct PageSegment {
    pub kind: SegmentKind,
    /// Size in bytes of this segment
    pub size: u32,
    /// Temporary UI edit buffer for the size field
    pub size_edit: String,
}

impl PageSegment {
    pub fn new(kind: SegmentKind, size: u32) -> Self {
        Self { kind, size, size_edit: size.to_string() }
    }

    pub fn sync_edit_from_size(&mut self) {
        self.size_edit = self.size.to_string();
    }
}

// ── PageStructureTabState ─────────────────────────────────────────────────────

/// Shared state for the Page Structure Editor tab.
///
/// Passed as `Arc<Mutex<PageStructureTabState>>` to the Hex Viewer and Workflow Editor.
pub struct PageStructureTabState {
    /// Ordered list of page segments
    pub segments: Vec<PageSegment>,
    /// Expected total page size (synced from the active workflow Input node)
    pub target_page_size: u32,
    /// Path for import/export
    pub case_file_path: String,
    /// Status / feedback message shown in the UI
    pub status_msg: String,
    /// Whether the status message is an error (red) or info (green)
    pub status_is_error: bool,
    /// Generated case XML (cached, refreshed after each edit)
    case_xml_cache: String,
    /// Whether the XML cache is valid
    xml_dirty: bool,
    /// Index of the segment currently being dragged (resize handle)
    drag_segment: Option<usize>,
}

impl Default for PageStructureTabState {
    fn default() -> Self {
        Self::new()
    }
}

impl PageStructureTabState {
    pub fn new() -> Self {
        Self {
            segments: Vec::new(),
            target_page_size: 0,
            case_file_path: "download.case".to_string(),
            status_msg: String::new(),
            status_is_error: false,
            case_xml_cache: String::new(),
            xml_dirty: true,
            drag_segment: None,
        }
    }

    // ── Derived data ──────────────────────────────────────────────────────────

    /// Total byte size of all segments
    pub fn total_size(&self) -> u32 {
        self.segments.iter().map(|s| s.size).sum()
    }

    /// Compute start byte offset of segment at `index`
    pub fn segment_start(&self, index: usize) -> u32 {
        self.segments[..index].iter().map(|s| s.size).sum()
    }

    /// Convert segments into `PageStructureRecord` list for the Hex Viewer
    pub fn to_page_records(&self) -> Vec<PageStructureRecord> {
        let mut records = Vec::with_capacity(self.segments.len());
        let mut offset: u64 = 0;
        for seg in &self.segments {
            if seg.size == 0 {
                continue;
            }
            records.push(PageStructureRecord {
                name: seg.kind.case_name().to_string(),
                start: offset,
                stop: offset + seg.size as u64 - 1,
                kind: seg.kind.to_hex_kind(),
            });
            offset += seg.size as u64;
        }
        records
    }

    // ── Import / Export ───────────────────────────────────────────────────────

    /// Import segments from a `.case` XML string.
    /// Overwrites the current segment list.
    pub fn import_from_case_xml(&mut self, xml: &str) {
        let records = parse_case_file(xml);
        if records.is_empty() {
            self.status_msg = "No valid <Record> entries found in XML.".to_string();
            self.status_is_error = true;
            return;
        }
        // Derive segments from consecutive records
        self.segments.clear();
        for rec in &records {
            let size = (rec.stop - rec.start + 1) as u32;
            let kind = SegmentKind::from_case_name(&rec.name);
            self.segments.push(PageSegment::new(kind, size));
        }
        self.xml_dirty = true;
        self.status_msg = format!("Imported {} segments.", records.len());
        self.status_is_error = false;
    }

    /// Generate `.case` XML from current segments
    pub fn generate_case_xml(&mut self) -> &str {
        if !self.xml_dirty {
            return &self.case_xml_cache;
        }
        let page_size = self.total_size();
        let mut xml = String::from("<?xml version=\"1.0\"?>\n<Project>\n  <Records>\n");
        let mut offset: u64 = 0;
        for seg in &self.segments {
            if seg.size == 0 {
                continue;
            }
            let stop = offset + seg.size as u64 - 1;
            xml.push_str(&format!(
                "    <Record StructureDefinitionName=\"{}\" StartAddress=\"{}\" StopAddress=\"{}\" />\n",
                seg.kind.case_name(),
                offset,
                stop
            ));
            offset += seg.size as u64;
        }
        xml.push_str("  </Records>\n  <Records>\n");
        xml.push_str(&format!(
            "    <Record StructureDefinitionName=\"Page\" StartAddress=\"0\" StopAddress=\"{}\" />\n",
            page_size.saturating_sub(1)
        ));
        xml.push_str(&format!(
            "  </Records>\n  <Page_size>{}</Page_size>\n</Project>",
            page_size
        ));
        self.case_xml_cache = xml;
        self.xml_dirty = false;
        &self.case_xml_cache
    }

    // ── UI ────────────────────────────────────────────────────────────────────

    /// Render the Page Structure Editor inside the given egui context.
    /// `active_page_length` comes from the workflow editor's active Input node.
    pub fn show_ui(&mut self, ctx: &egui::Context, active_page_length: Option<u32>) {
        // Sync target from workflow if available
        if let Some(pl) = active_page_length {
            if pl > 0 {
                self.target_page_size = pl;
            }
        }

        egui::SidePanel::right("ps_xml_panel")
            .resizable(true)
            .default_width(420.0)
            .min_width(260.0)
            .show(ctx, |ui| {
                self.draw_xml_panel(ui);
            });

        egui::CentralPanel::default().show(ctx, |ui| {
            self.draw_editor_panel(ui, active_page_length);
        });
    }

    // ── Editor panel (left / central) ─────────────────────────────────────────

    fn draw_editor_panel(&mut self, ui: &mut Ui, active_page_length: Option<u32>) {
        let total = self.total_size();
        let target = self.target_page_size;
        let matches = target > 0 && total == target;
        let overflow = target > 0 && total > target;

        ui.horizontal(|ui| {
            ui.heading("Page Structure Editor");
            ui.add_space(16.0);
            if target > 0 {
                let (icon, msg, color) = if matches {
                    ("✔", format!("Total {total} B = page size {target} B"), Color32::from_rgb(34, 197, 94))
                } else if overflow {
                    ("⚠", format!("Total {total} B > page size {target} B  (+{})", total - target), Color32::from_rgb(239, 68, 68))
                } else {
                    ("⚠", format!("Total {total} B < page size {target} B  ({} missing)", target - total), Color32::from_rgb(249, 115, 22))
                };
                ui.colored_label(color, format!("{icon}  {msg}"));
            } else {
                ui.label(format!("Total: {total} B  (no workflow active)"));
            }
        });

        ui.separator();

        // ── Visual layout bar ─────────────────────────────────────────────────
        self.draw_layout_bar(ui, total);

        ui.add_space(8.0);
        ui.separator();

        // ── Scrollable body (segment table + controls) ────────────────────────
        // The heading, status indicator, and layout bar above are pinned.
        // Everything below scrolls so long segment lists stay accessible.
        let mut changed = false;

        egui::ScrollArea::vertical()
            .id_source("ps_editor_scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                // ── Segment table ──────────────────────────────────────────────
                ui.label(RichText::new("Segments").strong());
                ui.add_space(4.0);

                let mut remove_idx: Option<usize> = None;
                let mut move_up: Option<usize> = None;
                let mut move_dn: Option<usize> = None;

                egui::Grid::new("ps_seg_grid")
                    .num_columns(7)
                    .spacing([6.0, 4.0])
                    .striped(true)
                    .show(ui, |ui| {
                        // Header row
                        ui.label(RichText::new("#").weak());
                        ui.label(RichText::new("Type").weak());
                        ui.label(RichText::new("Size (bytes)").weak());
                        ui.label(RichText::new("Start").weak());
                        ui.label(RichText::new("End").weak());
                        ui.label(RichText::new("Reorder").weak());
                        ui.label(RichText::new("").weak());
                        ui.end_row();

                        let mut offset: u32 = 0;
                        let n = self.segments.len();
                        for i in 0..n {
                            let seg = &self.segments[i];
                            let seg_start = offset;
                            let seg_end = offset + seg.size.saturating_sub(1);
                            offset += seg.size;

                            // Index
                            ui.label(format!("{}", i + 1));

                            // Type dropdown
                            let current_kind = self.segments[i].kind;
                            egui::ComboBox::from_id_source(format!("ps_kind_{i}"))
                                .selected_text(current_kind.label())
                                .width(90.0)
                                .show_ui(ui, |ui| {
                                    for &k in SegmentKind::all() {
                                        let sel = ui.selectable_label(
                                            current_kind == k,
                                            RichText::new(k.label()).color(k.color()),
                                        );
                                        if sel.clicked() {
                                            self.segments[i].kind = k;
                                            changed = true;
                                        }
                                    }
                                });

                            // Size input
                            let resp = ui.add(
                                egui::TextEdit::singleline(&mut self.segments[i].size_edit)
                                    .desired_width(80.0)
                                    .hint_text("bytes"),
                            );
                            if resp.changed() {
                                if let Ok(v) = self.segments[i].size_edit.parse::<u32>() {
                                    self.segments[i].size = v;
                                    changed = true;
                                }
                            }
                            if resp.lost_focus() {
                                self.segments[i].size_edit = self.segments[i].size.to_string();
                            }

                            // Start / End (read-only display)
                            ui.label(format!("0x{:X}", seg_start));
                            ui.label(format!("0x{:X}", seg_end));

                            // Reorder buttons
                            ui.horizontal(|ui| {
                                if ui.small_button("↑").on_hover_text("Move up").clicked() && i > 0 {
                                    move_up = Some(i);
                                }
                                if ui.small_button("↓").on_hover_text("Move down").clicked() && i + 1 < n {
                                    move_dn = Some(i);
                                }
                            });

                            // Remove
                            if ui.small_button("×").on_hover_text("Remove segment").clicked() {
                                remove_idx = Some(i);
                            }

                            ui.end_row();
                        }
                    });

                // Apply mutations after the grid borrow ends
                if let Some(i) = remove_idx {
                    self.segments.remove(i);
                    changed = true;
                }
                if let Some(i) = move_up {
                    self.segments.swap(i, i - 1);
                    changed = true;
                }
                if let Some(i) = move_dn {
                    self.segments.swap(i, i + 1);
                    changed = true;
                }

                ui.add_space(6.0);

                // ── Add segment buttons ────────────────────────────────────────
                ui.horizontal_wrapped(|ui| {
                    ui.label("Add:");
                    for &k in SegmentKind::all() {
                        let btn = egui::Button::new(
                            RichText::new(format!("+ {}", k.label()))
                                .color(Color32::WHITE)
                                .small(),
                        )
                        .fill(k.color())
                        .rounding(Rounding::same(4.0));
                        if ui.add(btn).clicked() {
                            let remaining = if target > 0 {
                                target.saturating_sub(self.total_size())
                            } else {
                                0
                            };
                            self.segments.push(PageSegment::new(k, remaining));
                            changed = true;
                        }
                    }
                });

                ui.add_space(8.0);

                // ── Target page size override ──────────────────────────────────
                ui.horizontal(|ui| {
                    ui.label("Target page size (bytes):");
                    let mut ts = self.target_page_size.to_string();
                    if ui.add(egui::TextEdit::singleline(&mut ts).desired_width(80.0)).changed() {
                        if let Ok(v) = ts.parse::<u32>() {
                            self.target_page_size = v;
                        }
                    }
                    if active_page_length.is_some() {
                        ui.label(RichText::new("(synced from workflow)").weak().small());
                    }
                });

                // ── Status message ─────────────────────────────────────────────
                if !self.status_msg.is_empty() {
                    ui.add_space(4.0);
                    let color = if self.status_is_error {
                        Color32::from_rgb(239, 68, 68)
                    } else {
                        Color32::from_rgb(34, 197, 94)
                    };
                    ui.colored_label(color, &self.status_msg);
                }

                // Bottom padding so the last row isn't flush against the edge.
                ui.add_space(12.0);
            }); // end ScrollArea

        if changed {
            self.xml_dirty = true;
        }
    }

    // ── Visual page layout bar ────────────────────────────────────────────────

    /// Width in pixels of the drag-handle hit zone on each side of a boundary.
    const HANDLE_HIT_HALF: f32 = 6.0;
    /// Minimum segment size in bytes when resizing via drag.
    const MIN_SEGMENT_BYTES: u32 = 1;

    /// Compute the pixel X position of each inter-segment boundary within `rect`.
    /// Returns a Vec of length `segments.len() - 1`.
    fn boundary_x_positions(&self, rect: egui::Rect, total: u32) -> Vec<f32> {
        // Mirror the largest-remainder pixel allocation used when painting,
        // so drag handles land exactly on painted boundaries.
        let bar_w = rect.width();
        let visible: Vec<u32> = self.segments.iter()
            .filter(|s| s.size > 0)
            .map(|s| s.size)
            .collect();
        let n = visible.len();
        if n == 0 {
            return Vec::new();
        }

        let mut px_widths: Vec<i32> = vec![1; n];
        let guaranteed = n as i32;
        let remaining_px = (bar_w.round() as i32 - guaranteed).max(0);
        let remaining_ideal: f32 = bar_w - guaranteed as f32;

        let mut remainders: Vec<(usize, f32)> = visible.iter().enumerate()
            .map(|(i, &sz)| {
                let ideal = sz as f32 / total as f32 * bar_w;
                let proportional = if remaining_ideal > 0.0 {
                    (ideal - 1.0).max(0.0) / remaining_ideal * remaining_px as f32
                } else {
                    0.0
                };
                let floor = proportional.floor() as i32;
                px_widths[i] += floor;
                (i, proportional - floor as f32)
            })
            .collect();

        let distributed: i32 = px_widths.iter().sum();
        let leftover = (bar_w.round() as i32 - distributed).max(0);
        remainders.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        for (i, _) in remainders.iter().take(leftover as usize) {
            px_widths[*i] += 1;
        }

        // Boundaries are the right edges of all but the last segment.
        let mut positions = Vec::with_capacity(n.saturating_sub(1));
        let mut x = rect.left();
        for w in px_widths.iter().take(n.saturating_sub(1)) {
            x += *w as f32;
            positions.push(x.min(rect.right()));
        }
        positions
    }

    fn draw_layout_bar(&mut self, ui: &mut Ui, total: u32) {
        let bar_h = 48.0;
        // Use max_rect width rather than available_width so the bar respects
        // the side panel reservation even on the first frame.
        let available_w = (ui.max_rect().width() - 8.0).max(0.0);

        if total == 0 || available_w < 10.0 {
            ui.label(RichText::new("(Add segments to see layout)").weak().italics());
            return;
        }

        // Sense both hover and drag so we can resize boundary handles.
        let (rect, response) = ui.allocate_exact_size(
            vec2(available_w, bar_h),
            egui::Sense::click_and_drag(),
        );

        // ── Drag logic ────────────────────────────────────────────────────────

        // On drag start: find which boundary (if any) was grabbed.
        if response.drag_started() {
            if let Some(press_pos) = response.interact_pointer_pos() {
                let boundaries = self.boundary_x_positions(rect, total);
                self.drag_segment = boundaries.iter().enumerate().find_map(|(i, &bx)| {
                    if (press_pos.x - bx).abs() <= Self::HANDLE_HIT_HALF {
                        Some(i) // boundary between segment[i] and segment[i+1]
                    } else {
                        None
                    }
                });
            }
        }

        // While dragging: redistribute bytes between the two neighbours.
        if response.dragged() {
            if let Some(boundary_idx) = self.drag_segment {
                let delta_px = response.drag_delta().x;
                if delta_px != 0.0 && boundary_idx + 1 < self.segments.len() {
                    // Convert pixel delta to byte delta.
                    let bytes_per_px = total as f32 / available_w;
                    let delta_bytes = (delta_px * bytes_per_px).round() as i64;

                    let left_size = self.segments[boundary_idx].size as i64;
                    let right_size = self.segments[boundary_idx + 1].size as i64;

                    let new_left = (left_size + delta_bytes)
                        .clamp(Self::MIN_SEGMENT_BYTES as i64, left_size + right_size - Self::MIN_SEGMENT_BYTES as i64);
                    let new_right = left_size + right_size - new_left;

                    self.segments[boundary_idx].size = new_left as u32;
                    self.segments[boundary_idx].sync_edit_from_size();
                    self.segments[boundary_idx + 1].size = new_right as u32;
                    self.segments[boundary_idx + 1].sync_edit_from_size();
                    self.xml_dirty = true;
                }
            }
        }

        // On drag release: clear active handle.
        if response.drag_stopped() {
            self.drag_segment = None;
        }

        // ── Painting ─────────────────────────────────────────────────────────

        let painter = ui.painter();

        // Background
        painter.rect_filled(rect, Rounding::same(4.0), Color32::from_rgb(30, 30, 40));

        // Draw segments using a Bresenham-style pixel allocation:
        //
        //   - Each segment's ideal width = (size / total) * bar_width  (float)
        //   - We accumulate the ideal right-edge position and snap to integer
        //     pixels, so rounding errors never accumulate across segments.
        //   - Any segment with size > 0 gets at least 1 pixel.  The "debt"
        //     created by forcing tiny segments up to 1 px is absorbed by the
        //     next segment that has spare pixels to give.
        //
        // Result: total rendered width == rect.width() exactly; no overflow.

        // First pass — compute each segment's minimum (1) and ideal pixel widths.
        struct SegPx { ideal: f32, px: i32 }
        let bar_w = rect.width();
        let mut seg_px: Vec<SegPx> = self.segments.iter()
            .filter(|s| s.size > 0)
            .map(|s| SegPx {
                ideal: s.size as f32 / total as f32 * bar_w,
                px: 1, // guaranteed minimum
            })
            .collect();

        // Total pixels already "spent" on guarantees
        let guaranteed: i32 = seg_px.len() as i32;
        // Remaining pixels to distribute proportionally
        let remaining_px = (bar_w.round() as i32 - guaranteed).max(0);
        // Distribute remaining pixels by largest-remainder method.
        let remaining_ideal: f32 = bar_w - guaranteed as f32;
        // First, compute floor allocations into a separate vec to avoid
        // simultaneous borrow of seg_px.
        let floors_and_fracs: Vec<(i32, f32)> = seg_px.iter()
            .map(|s| {
                let proportional = if remaining_ideal > 0.0 {
                    (s.ideal - 1.0).max(0.0) / remaining_ideal * remaining_px as f32
                } else {
                    0.0
                };
                let floor = proportional.floor() as i32;
                (floor, proportional - floor as f32)
            })
            .collect();
        let mut remainders: Vec<(usize, f32)> = floors_and_fracs.iter().enumerate()
            .map(|(i, &(floor, frac))| { seg_px[i].px += floor; (i, frac) })
            .collect();
        // Hand out leftover pixels to the largest remainders first
        let distributed: i32 = seg_px.iter().map(|s| s.px).sum();
        let leftover = (bar_w.round() as i32 - distributed).max(0);
        remainders.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        for (i, _) in remainders.iter().take(leftover as usize) {
            seg_px[*i].px += 1;
        }

        // Second pass — paint using the computed pixel widths.
        let n_visible = seg_px.len();
        let mut px_x = rect.left();
        let mut seg_px_iter = seg_px.iter();
        for (i, seg) in self.segments.iter().enumerate() {
            if seg.size == 0 {
                continue;
            }
            let spx = seg_px_iter.next().unwrap();
            let w = spx.px as f32;
            let x0 = px_x;
            let x1 = (px_x + w).min(rect.right());
            px_x += w;

            let seg_rect = egui::Rect::from_min_max(
                egui::pos2(x0, rect.top()),
                egui::pos2(x1, rect.bottom()),
            );

            // Fill — dim if a *different* segment's boundary is being dragged.
            let fill_color = if self.drag_segment.is_some_and(|d| d != i && d + 1 != i) {
                seg.kind.color().gamma_multiply(0.55)
            } else {
                seg.kind.color()
            };

            painter.rect_filled(
                seg_rect.shrink(0.5),
                Rounding::same(if i == 0 { 4.0 } else { 0.0 }),
                fill_color,
            );

            // Label (if wide enough)
            if w > 30.0 {
                painter.text(
                    seg_rect.center(),
                    egui::Align2::CENTER_CENTER,
                    format!("{}\n{} B", seg.kind.label(), seg.size),
                    egui::FontId::proportional(10.0),
                    Color32::WHITE,
                );
            }
        }
        let _ = n_visible;

        // Draw boundary handles and set resize cursor when hovering one.
        let hover_pos = response.hover_pos().or_else(|| response.interact_pointer_pos());
        let boundaries = self.boundary_x_positions(rect, total);

        for (i, &bx) in boundaries.iter().enumerate() {
            let is_active = self.drag_segment == Some(i);
            let is_hovered = hover_pos.is_some_and(|p| (p.x - bx).abs() <= Self::HANDLE_HIT_HALF);

            let handle_color = if is_active {
                Color32::WHITE
            } else if is_hovered {
                Color32::from_rgb(220, 220, 220)
            } else {
                Color32::from_rgba_premultiplied(180, 180, 180, 100)
            };

            let handle_w = if is_active || is_hovered { 2.5 } else { 1.0 };

            painter.line_segment(
                [egui::pos2(bx, rect.top() + 2.0), egui::pos2(bx, rect.bottom() - 2.0)],
                Stroke::new(handle_w, handle_color),
            );

            // Resize cursor when hovering a handle zone.
            if is_hovered || is_active {
                ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
            }
        }

        // Overflow / exact-fit border
        if self.target_page_size > 0 && total > self.target_page_size {
            painter.rect_stroke(rect, Rounding::same(4.0), Stroke::new(2.0, Color32::from_rgb(239, 68, 68)));
        } else if self.target_page_size > 0 && total == self.target_page_size {
            painter.rect_stroke(rect, Rounding::same(4.0), Stroke::new(1.5, Color32::from_rgb(34, 197, 94)));
        }

        // Hover tooltip: show segment info, or boundary info while dragging.
        if let Some(hp) = hover_pos {
            // Check if near a boundary first.
            let near_boundary = boundaries.iter().enumerate().find(|(_, &bx)| {
                (hp.x - bx).abs() <= Self::HANDLE_HIT_HALF
            });

            if let Some((bi, _)) = near_boundary {
                if bi + 1 < self.segments.len() {
                    egui::show_tooltip_at_pointer(ui.ctx(), egui::Id::new("ps_bar_tip"), |ui| {
                        ui.label(format!(
                            "Drag to resize  ←  {} | {} →",
                            self.segments[bi].kind.label(),
                            self.segments[bi + 1].kind.label(),
                        ));
                        ui.label(format!(
                            "{} B  |  {} B",
                            self.segments[bi].size,
                            self.segments[bi + 1].size,
                        ));
                    });
                }
            } else if !response.dragged() {
                // Regular segment tooltip.
                let frac = ((hp.x - rect.left()) / available_w).clamp(0.0, 1.0);
                let byte_at = (frac * total as f32) as u32;
                let mut off: u32 = 0;
                for seg in &self.segments {
                    if byte_at < off + seg.size {
                        egui::show_tooltip_at_pointer(ui.ctx(), egui::Id::new("ps_bar_tip"), |ui| {
                            ui.label(format!(
                                "{}: 0x{:X}..0x{:X}  ({} B)",
                                seg.kind.label(),
                                off,
                                off + seg.size - 1,
                                seg.size,
                            ));
                        });
                        break;
                    }
                    off += seg.size;
                }
            }
        }
    }

    // ── XML / Import panel (right side) ──────────────────────────────────────

    fn draw_xml_panel(&mut self, ui: &mut Ui) {
        ui.heading("Import / Export");
        ui.separator();

        // File path
        ui.horizontal(|ui| {
            ui.label("File:");
            ui.add(egui::TextEdit::singleline(&mut self.case_file_path)
                .desired_width(ui.available_width() - 4.0)
                .hint_text("path/to/file.case"));
        });

        ui.horizontal(|ui| {
            if ui.button("📂 Load .case file").clicked() {
                self.load_case_file();
            }
            if ui.button("💾 Save .case file").clicked() {
                self.save_case_file();
            }
        });

        ui.add_space(8.0);

        egui::ScrollArea::vertical()
            .id_source("ps_xml_scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                // XML preview / paste area
                ui.label(RichText::new("Generated .case XML:").strong());
                let _ = self.generate_case_xml();
                let mut xml_copy = self.case_xml_cache.clone();
                let panel_w = ui.available_width();
                ui.add(
                    egui::TextEdit::multiline(&mut xml_copy)
                        .desired_width(panel_w)
                        .desired_rows(18)
                        .font(egui::TextStyle::Monospace),
                );

                ui.add_space(4.0);

                // Paste & Import
                ui.label(RichText::new("Paste XML to import:").strong());
                egui::CollapsingHeader::new("Paste XML here ▼")
                    .default_open(false)
                    .show(ui, |ui| {
                        use std::cell::RefCell;
                        thread_local! {
                            static PASTE_BUF: RefCell<String> = RefCell::new(String::new());
                        }
                        let paste_w = ui.available_width();
                        let xml_to_import = PASTE_BUF.with(|paste_buf| {
                            let mut paste_buf = paste_buf.borrow_mut();
                            ui.add(
                                egui::TextEdit::multiline(&mut *paste_buf)
                                    .desired_width(paste_w)
                                    .desired_rows(8)
                                    .hint_text("Paste .case XML here, then click Import")
                                    .font(egui::TextStyle::Monospace),
                            );
                            if ui.button("⬆ Import from pasted XML").clicked() {
                                let xml_clone = paste_buf.clone();
                                paste_buf.clear();
                                Some(xml_clone)
                            } else {
                                None
                            }
                        });
                        if let Some(xml) = xml_to_import {
                            self.import_from_case_xml(&xml);
                        }
                    });

                ui.add_space(8.0);
            });
    }

    // ── File I/O helpers ──────────────────────────────────────────────────────

    fn load_case_file(&mut self) {
        let path = self.case_file_path.trim().to_string();
        if path.is_empty() {
            self.status_msg = "Enter a file path first.".to_string();
            self.status_is_error = true;
            return;
        }
        match std::fs::read_to_string(&path) {
            Ok(content) => self.import_from_case_xml(&content),
            Err(e) => {
                self.status_msg = format!("Failed to read {path}: {e}");
                self.status_is_error = true;
            }
        }
    }

    fn save_case_file(&mut self) {
        let path = self.case_file_path.trim().to_string();
        if path.is_empty() {
            self.status_msg = "Enter a file path first.".to_string();
            self.status_is_error = true;
            return;
        }
        let xml = self.generate_case_xml().to_string();
        match std::fs::write(&path, &xml) {
            Ok(_) => {
                self.status_msg = format!("Saved to {path}");
                self.status_is_error = false;
            }
            Err(e) => {
                self.status_msg = format!("Failed to write {path}: {e}");
                self.status_is_error = true;
            }
        }
    }
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_state() -> PageStructureTabState {
        let mut s = PageStructureTabState::new();
        s.segments = vec![
            PageSegment::new(SegmentKind::Data,    512),
            PageSegment::new(SegmentKind::Ecc,     400),
            PageSegment::new(SegmentKind::Sa,        8),
            PageSegment::new(SegmentKind::SaEcc,     8),
        ];
        s
    }

    #[test]
    fn test_total_size() {
        let s = sample_state();
        assert_eq!(s.total_size(), 512 + 400 + 8 + 8);
    }

    #[test]
    fn test_segment_start() {
        let s = sample_state();
        assert_eq!(s.segment_start(0),   0);
        assert_eq!(s.segment_start(1), 512);
        assert_eq!(s.segment_start(2), 912);
        assert_eq!(s.segment_start(3), 920);
    }

    #[test]
    fn test_to_page_records() {
        let s = sample_state();
        let recs = s.to_page_records();
        assert_eq!(recs.len(), 4);
        assert_eq!(recs[0].name, "DATA");
        assert_eq!(recs[0].start, 0);
        assert_eq!(recs[0].stop,  511);
        assert_eq!(recs[1].name, "ECC");
        assert_eq!(recs[1].start, 512);
        assert_eq!(recs[1].stop,  911);
        assert_eq!(recs[2].name, "SA");
        assert_eq!(recs[2].start, 912);
        assert_eq!(recs[2].stop,  919);
        assert_eq!(recs[3].name, "SA-ECC");
        assert_eq!(recs[3].start, 920);
        assert_eq!(recs[3].stop,  927);
    }

    #[test]
    fn test_generate_case_xml() {
        let mut s = sample_state();
        let xml = s.generate_case_xml().to_string();
        assert!(xml.contains("StructureDefinitionName=\"DATA\""));
        assert!(xml.contains("StartAddress=\"0\""));
        assert!(xml.contains("StopAddress=\"511\""));
        assert!(xml.contains("StructureDefinitionName=\"ECC\""));
        assert!(xml.contains("StartAddress=\"512\""));
        assert!(xml.contains("<Page_size>928</Page_size>"));
    }

    #[test]
    fn test_import_from_case_xml() {
        let xml = r#"<?xml version="1.0"?>
<Project>
  <Records>
    <Record StructureDefinitionName="DATA" StartAddress="0" StopAddress="511" />
    <Record StructureDefinitionName="ECC" StartAddress="512" StopAddress="911" />
    <Record StructureDefinitionName="SA" StartAddress="912" StopAddress="919" />
  </Records>
</Project>"#;
        let mut s = PageStructureTabState::new();
        s.import_from_case_xml(xml);
        assert_eq!(s.segments.len(), 3);
        assert_eq!(s.segments[0].kind, SegmentKind::Data);
        assert_eq!(s.segments[0].size, 512);
        assert_eq!(s.segments[1].kind, SegmentKind::Ecc);
        assert_eq!(s.segments[1].size, 400);
        assert_eq!(s.segments[2].kind, SegmentKind::Sa);
        assert_eq!(s.segments[2].size, 8);
    }

    #[test]
    fn test_xml_roundtrip() {
        let mut s = sample_state();
        let xml = s.generate_case_xml().to_string();
        let mut s2 = PageStructureTabState::new();
        s2.import_from_case_xml(&xml);
        assert_eq!(s2.total_size(), s.total_size());
        assert_eq!(s2.segments.len(), s.segments.len());
        for (a, b) in s.segments.iter().zip(s2.segments.iter()) {
            assert_eq!(a.kind, b.kind);
            assert_eq!(a.size, b.size);
        }
    }
}
