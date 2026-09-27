//! Pattern Writer Tab
//!
//! Provides a GUI for writing the DrResearch sector-number / XOR / ECC pattern
//! to a raw block device (SD card, SSD, NVMe, …) or an image file.
//!
//! The actual writing is delegated to `initpattern.pl` (Linux/macOS) or
//! `initpattern.exe` (Windows).  Both accept identical arguments:
//!
//!   initpattern  <device>  [<size> MB|GB]  [<data-area-size-in-bytes>]
//!
//! The tab streams the subprocess stdout/stderr into a scrollable log view and
//! shows a progress bar parsed from lines of the form:
//!   "Status: Sector NNN (M GB)"

use egui::{
    Color32, FontFamily, FontId, ProgressBar, RichText, ScrollArea, TextEdit, Ui,
    Vec2,
};
use std::io::{BufRead, BufReader};
use std::process::{Child, ChildStdout, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;

// ── helpers ──────────────────────────────────────────────────────────────────

/// Return the initpattern binary / script to use on this platform.
fn initpattern_command() -> (&'static str, &'static [&'static str]) {
    #[cfg(target_os = "windows")]
    {
        ("initpattern.exe", &[])
    }
    #[cfg(not(target_os = "windows"))]
    {
        ("perl", &["initpattern.pl"])
    }
}

// ── shared state between UI thread and background reader thread ───────────────

#[derive(Default)]
struct SharedOutput {
    lines: Vec<String>,
    /// 0.0 … 1.0, None = indeterminate
    progress: Option<f32>,
    total_sectors: Option<u64>,
    finished: bool,
    error: Option<String>,
}

// ── public tab state ──────────────────────────────────────────────────────────

/// Runtime status of the pattern-write operation
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum WriterStatus {
    Idle,
    Running,
    Finished,
    Failed,
}

impl Default for WriterStatus {
    fn default() -> Self {
        Self::Idle
    }
}

/// Persistent state for the Pattern Writer tab
pub struct PatternWriterTabState {
    // ── user-editable fields ──────────────────────────────────────────────
    /// Target device path, e.g. "/dev/sdb" or "\\.\PhysicalDrive1"
    pub device_path: String,
    /// Optional size override (leave empty to auto-detect from device)
    pub size_override: String,
    /// Data-area size in bytes (power of 2, default 8192)
    pub data_area_size: String,
    /// Whether the user has confirmed the big scary warning
    pub confirmed_warning: bool,

    // ── process management ────────────────────────────────────────────────
    status: WriterStatus,
    child: Option<Arc<Mutex<Option<Child>>>>,
    shared: Arc<Mutex<SharedOutput>>,

    // ── UI helpers ────────────────────────────────────────────────────────
    /// Auto-scroll log to the bottom
    auto_scroll: bool,
    /// Parsed sectors at last render (for display even before next poll)
    last_progress_line: String,
    /// Cached block-device list (populated on first render)
    device_list: Option<Vec<String>>,
}

impl Default for PatternWriterTabState {
    fn default() -> Self {
        Self::new()
    }
}

impl PatternWriterTabState {
    pub fn new() -> Self {
        Self {
            device_path: String::new(),
            size_override: String::new(),
            data_area_size: "8192".to_string(),
            confirmed_warning: false,

            status: WriterStatus::Idle,
            child: None,
            shared: Arc::new(Mutex::new(SharedOutput::default())),

            auto_scroll: true,
            last_progress_line: String::new(),
            device_list: None,
        }
    }

    // ── command building ──────────────────────────────────────────────────

    fn build_args(&self) -> Vec<String> {
        let (prog, leading) = initpattern_command();
        let mut args: Vec<String> = leading.iter().map(|s| s.to_string()).collect();

        // When using "perl", prog is "perl" and leading is &["initpattern.pl"].
        // We want: perl initpattern.pl <device> [size] [datasize]
        // When on Windows, prog is "initpattern.exe" with no leading args.
        let _ = prog; // used below in spawn

        args.push(self.device_path.trim().to_string());

        let size = self.size_override.trim();
        if !size.is_empty() {
            args.push(size.to_string());
        }

        let da = self.data_area_size.trim();
        if !da.is_empty() && da != "8192" {
            // Only pass explicitly when changed from default
            if size.is_empty() {
                // initpattern requires size arg before data-area arg
                // If no size was given, we cannot pass data-area — warn user
            } else {
                args.push(da.to_string());
            }
        }

        args
    }

    fn build_display_command(&self) -> String {
        let (prog, leading) = initpattern_command();
        let mut parts = vec![prog.to_string()];
        parts.extend(leading.iter().map(|s| s.to_string()));
        parts.extend(self.build_args());
        // build_args already puts leading items from initpattern_command leading slice,
        // so we just combine prog + args
        let args = self.build_args();
        let mut full = vec![prog.to_string()];
        let leading_strs: Vec<String> = leading.iter().map(|s| s.to_string()).collect();
        // Combine: prog [leading...] [device] [size] [datasize]
        // build_args already prepends leading items, so:
        full.extend(args);
        let _ = (parts, leading_strs);
        full.join(" ")
    }

    // ── process lifecycle ─────────────────────────────────────────────────

    fn start_writing(&mut self) {
        if self.device_path.trim().is_empty() {
            return;
        }

        // Reset shared output
        {
            let mut out = self.shared.lock().unwrap();
            *out = SharedOutput::default();
        }

        self.status = WriterStatus::Running;
        self.last_progress_line.clear();

        let (prog, leading) = initpattern_command();
        let args = self.build_args();

        let display_cmd = self.build_display_command();
        {
            let mut out = self.shared.lock().unwrap();
            out.lines.push(format!("$ {}", display_cmd));
            out.lines.push(String::new());
        }

        // Spawn the child process with merged stdout+stderr
        let mut cmd = Command::new(prog);
        cmd.args(leading);
        cmd.args(&args);
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped()); // capture separately so we don't lose errors

        let child_result = cmd.spawn();

        match child_result {
            Err(e) => {
                let mut out = self.shared.lock().unwrap();
                out.error = Some(format!("Failed to start process: {}", e));
                out.finished = true;
                self.status = WriterStatus::Failed;
            }
            Ok(mut child) => {
                let stdout: ChildStdout = child.stdout.take().expect("stdout piped");
                let stderr_pipe = child.stderr.take().expect("stderr piped");

                let shared_stdout = self.shared.clone();
                let shared_stderr = self.shared.clone();

                // Stdout reader thread
                thread::spawn(move || {
                    let reader = BufReader::new(stdout);
                    for line in reader.lines() {
                        let line = line.unwrap_or_default();
                        let mut out = shared_stdout.lock().unwrap();

                        // Parse progress lines: "Status: Sector NNN (M GB)"
                        if line.starts_with("Status: Sector ") {
                            let rest = &line["Status: Sector ".len()..];
                            if let Some(sector_str) = rest.split_whitespace().next() {
                                if let Ok(sector) = sector_str.parse::<u64>() {
                                    if let Some(total) = out.total_sectors {
                                        if total > 0 {
                                            out.progress = Some((sector as f32 / total as f32).clamp(0.0, 1.0));
                                        }
                                    } else {
                                        out.progress = Some(0.0);
                                    }
                                }
                            }
                        }

                        // Capture device size for progress denominator
                        // "Device Size: NNN (MMgb)" or "Image Size: NNN (MMgb)"
                        if line.contains("Device Size:") || line.contains("Image Size:") {
                            // extract leading bytes value
                            let rest = line.trim_start_matches(|c: char| !c.is_ascii_digit());
                            if let Some(size_str) = rest.split_whitespace().next() {
                                if let Ok(bytes) = size_str.parse::<u64>() {
                                    out.total_sectors = Some(bytes / 512);
                                }
                            }
                        }

                        out.lines.push(line);
                    }
                });

                // Stderr reader thread — append to same log but prefix with "[err]"
                thread::spawn(move || {
                    let reader = BufReader::new(stderr_pipe);
                    for line in reader.lines() {
                        let line = line.unwrap_or_default();
                        let mut out = shared_stderr.lock().unwrap();
                        out.lines.push(format!("[stderr] {}", line));
                    }
                });

                let child_arc = Arc::new(Mutex::new(Some(child)));
                self.child = Some(child_arc.clone());

                // Waiter thread — sets finished flag when child exits
                let shared_waiter = self.shared.clone();
                thread::spawn(move || {
                    let mut guard = child_arc.lock().unwrap();
                    if let Some(ref mut c) = *guard {
                        match c.wait() {
                            Ok(status) => {
                                let mut out = shared_waiter.lock().unwrap();
                                out.finished = true;
                                out.progress = Some(if status.success() { 1.0 } else { 0.0 });
                                if !status.success() {
                                    out.error = Some(format!(
                                        "Process exited with status: {}",
                                        status
                                    ));
                                } else {
                                    out.lines.push(String::new());
                                    out.lines.push("✓ Pattern written successfully.".to_string());
                                }
                            }
                            Err(e) => {
                                let mut out = shared_waiter.lock().unwrap();
                                out.finished = true;
                                out.error = Some(format!("Wait failed: {}", e));
                            }
                        }
                    }
                });
            }
        }
    }

    fn stop_writing(&mut self) {
        if let Some(ref child_arc) = self.child {
            if let Ok(mut guard) = child_arc.lock() {
                if let Some(ref mut c) = *guard {
                    let _ = c.kill();
                }
            }
        }
        {
            let mut out = self.shared.lock().unwrap();
            out.finished = true;
            out.lines.push(String::new());
            out.lines.push("[Stopped by user]".to_string());
        }
        self.status = WriterStatus::Idle;
    }

    /// Poll background thread state.  Call once per frame before drawing.
    fn poll(&mut self) {
        if self.status != WriterStatus::Running {
            return;
        }
        let out = self.shared.lock().unwrap();
        if out.finished {
            if out.error.is_some() {
                self.status = WriterStatus::Failed;
            } else {
                self.status = WriterStatus::Finished;
            }
        }
        // Update last progress line for display
        if let Some(prog) = out.progress {
            if let Some(total) = out.total_sectors {
                let done = (prog * total as f32) as u64;
                self.last_progress_line = format!(
                    "{:.1}%  ({} / {} sectors, {:.1} GB written)",
                    prog * 100.0,
                    done,
                    total,
                    (done * 512) as f64 / 1e9
                );
            } else {
                self.last_progress_line = format!("{:.1}%", prog * 100.0);
            }
        }
    }

    // ── list block devices (Linux only) ──────────────────────────────────

    #[cfg(target_os = "linux")]
    fn list_block_devices() -> Vec<String> {
        // Determine which disk hosts the root filesystem so we can exclude it.
        // `lsblk -no PKNAME <root-source>` returns the parent disk name (e.g. "sda").
        // If it returns nothing (root is directly on a disk, not a partition) we fall
        // back to stripping the trailing digit from the source device name.
        let os_disk: Option<String> = (|| {
            // Step 1: find the block device that backs "/"
            let fm = std::process::Command::new("findmnt")
                .args(["-n", "-o", "SOURCE", "/"])
                .output()
                .ok()?;
            let source = String::from_utf8_lossy(&fm.stdout).trim().to_string();
            if source.is_empty() {
                return None;
            }

            // Step 2: ask lsblk for the parent disk of that device
            let pk = std::process::Command::new("lsblk")
                .args(["-no", "PKNAME", &source])
                .output()
                .ok()?;
            let pkname = String::from_utf8_lossy(&pk.stdout).trim().to_string();
            if !pkname.is_empty() {
                return Some(pkname);
            }

            // Step 3: fallback — strip trailing digits from the source basename
            // e.g. "/dev/sda1" → "sda", "/dev/nvme0n1p1" → strip "p1" → "nvme0n1"
            let basename = std::path::Path::new(&source)
                .file_name()?
                .to_str()?
                .to_string();
            // Strip trailing partition suffix: digits, or "p" followed by digits
            let stripped = basename.trim_end_matches(|c: char| c.is_ascii_digit());
            let stripped = stripped.trim_end_matches('p');
            if !stripped.is_empty() {
                Some(stripped.to_string())
            } else {
                Some(basename)
            }
        })();

        let out = std::process::Command::new("lsblk")
            .args(["-dno", "NAME,SIZE,TYPE,TRAN,MODEL"])
            .output();

        match out {
            Ok(o) if o.status.success() => {
                let text = String::from_utf8_lossy(&o.stdout);
                text.lines()
                    .filter(|l| !l.trim().is_empty())
                    .filter(|l| {
                        let name = l.split_whitespace().next().unwrap_or("");
                        // Exclude loop devices
                        if name.starts_with("loop") {
                            return false;
                        }
                        // Exclude the OS disk
                        if let Some(ref od) = os_disk {
                            if name == od.as_str() {
                                return false;
                            }
                        }
                        true
                    })
                    .map(|l| {
                        let name = l.split_whitespace().next().unwrap_or("?");
                        format!("/dev/{} — {}", name, l.trim())
                    })
                    .collect()
            }
            _ => vec!["(lsblk not available)".to_string()],
        }
    }

    #[cfg(not(target_os = "linux"))]
    fn list_block_devices() -> Vec<String> {
        // Placeholder on other OSes — on Windows you'd use wmic/PowerShell
        vec!["(device listing not implemented on this OS)".to_string()]
    }

    // ── main UI ───────────────────────────────────────────────────────────

    pub fn show_ui(&mut self, ctx: &egui::Context) {
        self.poll();

        let is_running = self.status == WriterStatus::Running;

        egui::TopBottomPanel::top("pw_toolbar").show(ctx, |ui| {
            self.render_toolbar(ui, is_running);
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            self.render_main(ui, ctx, is_running);
        });
    }

    fn render_toolbar(&mut self, ui: &mut Ui, is_running: bool) {
        ui.horizontal_wrapped(|ui| {
            ui.heading(
                RichText::new("Pattern Writer")
                    .color(Color32::from_rgb(255, 200, 50))
                    .strong(),
            );

            ui.separator();

            // Start / Stop button
            if is_running {
                let stop_btn = ui.add(
                    egui::Button::new(RichText::new("⏹  Stop").color(Color32::WHITE))
                        .fill(Color32::from_rgb(180, 40, 40)),
                );
                if stop_btn.clicked() {
                    self.stop_writing();
                }
            } else {
                let can_start = self.confirmed_warning && !self.device_path.trim().is_empty();
                let label = match self.status {
                    WriterStatus::Finished => "▶  Run Again",
                    WriterStatus::Failed => "▶  Retry",
                    _ => "▶  Write Pattern",
                };
                let start_btn = ui.add_enabled(
                    can_start,
                    egui::Button::new(RichText::new(label).color(Color32::BLACK))
                        .fill(Color32::from_rgb(50, 200, 80)),
                );
                if start_btn.clicked() {
                    self.start_writing();
                }
                if !can_start && !is_running {
                    ui.colored_label(
                        Color32::from_rgb(180, 180, 60),
                        "← confirm warning & set device first",
                    );
                }
            }

            ui.separator();

            // Status badge
            let (badge_text, badge_color) = match self.status {
                WriterStatus::Idle => ("IDLE", Color32::from_rgb(120, 120, 120)),
                WriterStatus::Running => ("RUNNING", Color32::from_rgb(50, 180, 255)),
                WriterStatus::Finished => ("DONE ✓", Color32::from_rgb(50, 230, 100)),
                WriterStatus::Failed => ("FAILED ✗", Color32::from_rgb(255, 80, 60)),
            };
            ui.label(RichText::new(badge_text).color(badge_color).strong());

            if self.status == WriterStatus::Running {
                let out = self.shared.lock().unwrap();
                if let Some(p) = out.progress {
                    drop(out);
                    ui.separator();
                    ui.add(
                        ProgressBar::new(p)
                            .desired_width(200.0)
                            .text(self.last_progress_line.clone()),
                    );
                }
            }
        });
    }

    fn render_main(&mut self, ui: &mut Ui, ctx: &egui::Context, is_running: bool) {
        // Layout: left column (config) | right column (log)
        let avail = ui.available_size();
        let left_w = (avail.x * 0.38).min(380.0).max(260.0);
        let right_w = avail.x - left_w - 8.0;

        ui.horizontal(|ui| {
            // ── LEFT: Configuration ──────────────────────────────────
            ui.allocate_ui_with_layout(
                Vec2::new(left_w, avail.y),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    self.render_config(ui, is_running);
                },
            );

            ui.add(egui::Separator::default().vertical());

            // ── RIGHT: Log output ────────────────────────────────────
            ui.allocate_ui_with_layout(
                Vec2::new(right_w, avail.y),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    self.render_log(ui, ctx, is_running);
                },
            );
        });
    }

    fn render_config(&mut self, ui: &mut Ui, is_running: bool) {
        let mono = FontId::new(12.0, FontFamily::Monospace);

        // ── Warning banner ───────────────────────────────────────────────
        let warn_color = Color32::from_rgb(255, 160, 20);
        ui.group(|ui| {
            ui.set_width(ui.available_width());
            ui.label(
                RichText::new("⚠  WARNING")
                    .color(warn_color)
                    .strong()
                    .size(14.0),
            );
            ui.label(
                RichText::new(
                    "This tool OVERWRITES the entire target device.\n\
                     ALL DATA WILL BE LOST.  Use only on blank donor\n\
                     drives intended for NAND recovery experiments.",
                )
                .color(Color32::from_rgb(230, 180, 100))
                .size(11.0),
            );
            ui.add_space(4.0);
            ui.checkbox(
                &mut self.confirmed_warning,
                RichText::new("I understand — this is a donor drive")
                    .color(warn_color)
                    .strong(),
            );
        });

        ui.add_space(8.0);

        // ── Device path ───────────────────────────────────────────────────
        ui.label(RichText::new("Target device or image file:").strong());
        // Snapshot the current value so we can use it as the widget ID (a changing
        // ID forces egui to discard its stale internal buffer when the path is set
        // externally, e.g. by clicking a device in the list below).
        let device_path_snapshot = self.device_path.clone();
        ui.add_enabled_ui(!is_running, |ui| {
            ui.add(
                TextEdit::singleline(&mut self.device_path)
                    .id(egui::Id::new("device_path_edit").with(&device_path_snapshot))
                    .font(mono.clone())
                    .hint_text(if cfg!(windows) {
                        r"\\.\PhysicalDrive1"
                    } else {
                        "/dev/sdX  or  /path/to/image.bin"
                    })
                    .desired_width(ui.available_width()),
            );
        });

        // ── Quick-pick block devices ──────────────────────────────────────
        ui.add_space(4.0);
        ui.label(
            RichText::new("Available block devices:")
                .color(Color32::from_rgb(160, 160, 160)),
        );
        ui.group(|ui| {
            ui.set_width(ui.available_width());

            // Populate cache once; auto-select when exactly one device is present.
            if self.device_list.is_none() {
                let devs = Self::list_block_devices();
                if devs.len() == 1 {
                    let path = devs[0]
                        .split(" — ")
                        .next()
                        .unwrap_or(&devs[0])
                        .trim()
                        .to_string();
                    if self.device_path.is_empty() {
                        self.device_path = path;
                    }
                }
                self.device_list = Some(devs);
            }

            let devs = self.device_list.as_deref().unwrap_or(&[]);
            if devs.is_empty() {
                ui.label(
                    RichText::new("(no devices found)")
                        .color(Color32::from_rgb(120, 120, 120))
                        .font(mono.clone()),
                );
            }
            for dev in devs {
                let path = dev
                    .split(" — ")
                    .next()
                    .unwrap_or(dev)
                    .trim()
                    .to_string();
                let is_selected = self.device_path.trim() == path.as_str();
                let label = egui::SelectableLabel::new(
                    is_selected,
                    RichText::new(dev)
                        .font(mono.clone())
                        .color(if is_selected {
                            Color32::WHITE
                        } else {
                            Color32::from_rgb(160, 220, 255)
                        }),
                );
                if ui
                    .add_enabled(!is_running, label)
                    .on_hover_text("Click to select this device")
                    .clicked()
                {
                    self.device_path = path;
                }
            }
        });

        ui.add_space(8.0);

        // ── Optional size override ────────────────────────────────────────
        ui.label(
            RichText::new("Size override (optional):").strong(),
        );
        ui.label(
            RichText::new("Leave empty to detect from device.  Examples: 1000MB  or  256GB")
                .color(Color32::from_rgb(160, 160, 160))
                .size(10.5),
        );
        ui.add_enabled_ui(!is_running, |ui| {
            ui.add(
                TextEdit::singleline(&mut self.size_override)
                    .font(mono.clone())
                    .hint_text("e.g. 1000MB  or  128GB  (leave empty for device size)")
                    .desired_width(ui.available_width()),
            );
        });

        ui.add_space(8.0);

        // ── Data area size ───────────────────────────────────────────────
        ui.label(RichText::new("Data-area size (bytes):").strong());
        ui.label(
            RichText::new(
                "Must be a multiple of 512.  Common values: 512, 2048, 4096, 8192.\n\
                 Only effective when a size override is also given.",
            )
            .color(Color32::from_rgb(160, 160, 160))
            .size(10.5),
        );
        ui.add_enabled_ui(!is_running, |ui| {
            ui.horizontal_wrapped(|ui| {
                for &preset in &[512u32, 1024, 2048, 4096, 8192, 16384] {
                    if ui
                        .selectable_label(
                            self.data_area_size.trim() == preset.to_string(),
                            preset.to_string(),
                        )
                        .clicked()
                    {
                        self.data_area_size = preset.to_string();
                    }
                }
            });
            ui.add(
                TextEdit::singleline(&mut self.data_area_size)
                    .font(mono.clone())
                    .hint_text("8192")
                    .desired_width(ui.available_width()),
            );
        });

        ui.add_space(8.0);

        // ── Generated command preview ────────────────────────────────────
        ui.separator();
        ui.label(RichText::new("Command that will be executed:").color(Color32::from_rgb(160, 160, 160)));
        let cmd_text = self.build_display_command();
        ui.add(
            TextEdit::multiline(&mut cmd_text.as_str())
                .font(mono.clone())
                .desired_width(ui.available_width())
                .desired_rows(2)
                .interactive(false),
        );

        ui.add_space(8.0);

        // ── Pattern layout explanation ───────────────────────────────────
        ui.separator();
        ui.collapsing("Pattern layout", |ui| {
            let da: u64 = self.data_area_size.trim().parse().unwrap_or(8192);
            let eccreal = (da / 512) + 1;
            let majority = 7u64;
            let border0 = 512 * 1024 * 2u64;       // 512 MB in sectors
            let border7 = 1024 * 1024 * 2u64;
            let borderf = 1280 * 1024 * 2u64;
            let borderphi = 1536 * 1024 * 2u64;
            let borderecc = borderphi + eccreal * eccreal * majority * da * 8 + 1;

            let rows: &[(&str, u64, u64, &str)] = &[
                ("Sector-number pattern", 0, border0, "Sector IDs for FTL recovery"),
                ("XOR constant 0x00",    border0, border7, "Zero fill"),
                ("XOR constant 0x77",    border7, borderf, "0x77 fill"),
                ("XOR constant 0xFF",    borderf, borderphi, "0xFF fill"),
                ("ECC test vectors",     borderphi, borderecc, "LDPC/ECC identification"),
                ("Sector-number pattern (tail)", borderecc, 0, "Remainder of device"),
            ];

            egui::Grid::new("pattern_layout_grid")
                .num_columns(4)
                .spacing([8.0, 2.0])
                .striped(true)
                .show(ui, |ui| {
                    ui.label(RichText::new("Region").strong());
                    ui.label(RichText::new("Start (sectors)").strong());
                    ui.label(RichText::new("End (sectors)").strong());
                    ui.label(RichText::new("Purpose").strong());
                    ui.end_row();

                    for (name, start, end, purpose) in rows {
                        ui.label(*name);
                        if *start == 0 {
                            ui.label("0");
                        } else {
                            ui.label(format!("{}", start));
                        }
                        if *end == 0 {
                            ui.label("(device end)");
                        } else {
                            ui.label(format!("{}", end));
                        }
                        ui.label(*purpose);
                        ui.end_row();
                    }
                });

            ui.add_space(4.0);
            ui.label(
                RichText::new(format!(
                    "Minimum device size for DA={} B: {:.1} GB",
                    da,
                    (borderecc * 512) as f64 / 1e9
                ))
                .color(Color32::from_rgb(180, 200, 255)),
            );
        });
    }

    fn render_log(&mut self, ui: &mut Ui, ctx: &egui::Context, is_running: bool) {
        let mono = FontId::new(11.5, FontFamily::Monospace);

        ui.horizontal(|ui| {
            ui.label(RichText::new("Output log").strong());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.small_button("Clear").clicked() {
                    if let Ok(mut out) = self.shared.lock() {
                        out.lines.clear();
                    }
                }
                ui.checkbox(&mut self.auto_scroll, "Auto-scroll");
            });
        });

        ui.separator();

        // Progress bar (inside the log panel when running)
        {
            let out = self.shared.lock().unwrap();
            if is_running || self.status == WriterStatus::Finished || self.status == WriterStatus::Failed {
                if let Some(p) = out.progress {
                    drop(out);
                    ui.add(
                        ProgressBar::new(p)
                            .desired_width(ui.available_width())
                            .text(self.last_progress_line.clone()),
                    );
                    ui.add_space(2.0);
                }
            } else {
                drop(out);
            }
        }

        // Error banner
        {
            let out = self.shared.lock().unwrap();
            if let Some(ref err) = out.error {
                let err_clone = err.clone();
                drop(out);
                ui.colored_label(Color32::from_rgb(255, 80, 60), format!("Error: {}", err_clone));
                ui.add_space(2.0);
            } else {
                drop(out);
            }
        }

        // Scrollable log
        let avail_h = ui.available_height();
        let scroll_id = egui::Id::new("pw_log_scroll");
        let mut scroll = ScrollArea::vertical()
            .id_source(scroll_id)
            .max_height(avail_h)
            .auto_shrink([false, false]);

        if self.auto_scroll {
            scroll = scroll.stick_to_bottom(true);
        }

        scroll.show(ui, |ui| {
            let out = self.shared.lock().unwrap();
            let lines_snapshot: Vec<String> = out.lines.clone();
            drop(out);

            for line in &lines_snapshot {
                // Colour-code special lines
                let color = if line.starts_with("$") {
                    Color32::from_rgb(100, 210, 255) // command echo — cyan
                } else if line.starts_with("[stderr]") {
                    Color32::from_rgb(255, 160, 80) // stderr — orange
                } else if line.starts_with("Error") || line.starts_with("ERROR") {
                    Color32::from_rgb(255, 80, 60) // error — red
                } else if line.starts_with("WARNING") || line.starts_with("Warning") {
                    Color32::from_rgb(255, 200, 50) // warning — yellow
                } else if line.starts_with("Status: Sector") {
                    Color32::from_rgb(160, 220, 140) // progress — green
                } else if line.starts_with("✓") || line.contains("successfully") || line.contains("Done") || line.contains("done") {
                    Color32::from_rgb(80, 230, 120) // success — bright green
                } else {
                    Color32::from_rgb(210, 210, 210) // normal
                };

                ui.label(RichText::new(line).font(mono.clone()).color(color));
            }
        });

        // Request repaint while running (so the log updates without waiting for interaction)
        if is_running {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
    }
}
